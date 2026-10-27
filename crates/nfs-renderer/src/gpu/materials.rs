//! Materials and the bind groups they draw with: create / duplicate, the headlight / projector maps, the globals bind group,
//! the section maps' sky bind groups, the per-material bind groups and their rebuilds (ENHANCEMENT E2 anisotropy, trilinear),
//! the draw-state setters (mirror view, FakeSort mode, gamma). Moved out of gpu.rs as it was.

use crate::gfx;
use super::*;

impl Core {
    pub fn create_material(&mut self, desc: MaterialDesc) -> MaterialId {
        let texture = desc.texture.unwrap_or(self.white);
        let env = desc.env_texture.unwrap_or(self.white);
        let sampler = desc.clamp[0] as usize | (desc.clamp[1] as usize) << 1;
        let bind_group = self.material_bind_group(texture, sampler, env);
        let bind_group_point = desc.false_sort.then(|| self.material_bind_group_point(texture, sampler, env));
        self.materials.push(GpuMaterial {
            bind_group,
            bind_group_point,
            alpha_blend: desc.alpha_blend,
            blend_pair: desc.blend_pair.min(3),
            device_states: desc.device_states,
            sampler,
            lod_bias: desc.lod_bias,
            cull_back: desc.cull_back,
            view_cull: desc.view_cull,
            depth_bias: desc.depth_bias,
            false_sort: desc.false_sort,
            depth_test: desc.depth_test,
            env_mix: if desc.env_texture.is_some() { desc.env_mix } else { 0.0 },
            env_variant: desc.env_variant.min(2),
            sky_map: desc.sky_map,
            texture,
            env,
            reflect: desc.reflect,
        });
        MaterialId(self.materials.len() as u32 - 1)
    }

    /// A new material identical to `id` but sampling `texture` in its place — same blend/cull/lod
    /// state and specular map, a fresh bind group. Used to give one traffic/cop car instance its
    /// own decal-page material when `nfs_game::world::CarModel::instantiate` clones the shared
    /// model's damage resources so two differently-damaged instances in one frame no longer alias
    /// (`RE/notes/car-model.md` §13.1: this renderer batches every frame's `DrawItem`s and reads
    /// each `MeshId`/`TextureId`/`MaterialId` once at `render()`, unlike the original's per-car
    /// synchronous blit 0x572B40).
    pub fn duplicate_material_texture(&mut self, id: MaterialId, texture: TextureId) -> MaterialId {
        let Some(m) = self.materials.get(id.0 as usize) else { return id };
        let (sampler, env, false_sort, reflect) = (m.sampler, m.env, m.false_sort, m.reflect);
        let (alpha_blend, blend_pair, device_states, lod_bias, cull_back, view_cull, depth_bias, depth_test, env_mix, env_variant, sky_map) =
            (m.alpha_blend, m.blend_pair, m.device_states, m.lod_bias, m.cull_back, m.view_cull, m.depth_bias, m.depth_test, m.env_mix, m.env_variant, m.sky_map);
        let bind_group = self.material_bind_group(texture, sampler, env);
        let bind_group_point = false_sort.then(|| self.material_bind_group_point(texture, sampler, env));
        self.materials.push(GpuMaterial {
            bind_group,
            bind_group_point,
            alpha_blend,
            blend_pair,
            device_states,
            sampler,
            lod_bias,
            cull_back,
            view_cull,
            depth_bias,
            false_sort,
            depth_test,
            env_mix,
            env_variant,
            sky_map,
            texture,
            env,
            reflect,
        });
        MaterialId(self.materials.len() as u32 - 1)
    }

    /// Point every reflective material at another environment map — the original re-binds the car's
    /// environment layers whenever the `.map` section under it changes (`RE/notes/lighting.md` §3).
    /// `night.psh`'s `nght` bitmap, 128 × 128 bytes, for the headlight projector.
    pub fn set_headlight_map(&mut self, map: &[u8]) {
        if map.len() == 128 * 128 {
            self.queue.write_buffer(&self.headlight_map_buf, 0, map);
        }
    }

    /// The car shadow projector's texture, `[0x6115CC]+0x18C` (`RE/notes/crp-geometry.md` §8);
    /// `None` for none.
    pub fn set_projector_texture(&mut self, tex: Option<TextureId>) {
        self.proj_tex = tex;
        self.rebuild_globals_bg();
    }

    /// `HEAD` of `Render/common.fsh`, what `Headlights::LayerUVLight` projects (0x452645 fetches it
    /// by that four-character code); `None` for none.
    pub fn set_headlight_texture(&mut self, tex: Option<TextureId>) {
        self.head_tex = tex;
        self.rebuild_globals_bg();
    }

    pub(super) fn rebuild_globals_bg(&mut self) {
        self.globals_bg = self.make_globals_bg(None);
    }

    /// The globals bind group; `soft` is ENHANCEMENT E12's depth image of the pass about to be drawn
    /// (`None`: the 1 x 1 dummy).
    pub(super) fn make_globals_bg(&self, soft: Option<&wgpu::TextureView>) -> wgpu::BindGroup {
        let pick = |t: Option<TextureId>| match t.and_then(|t| self.textures.get(t.0 as usize)) {
            Some(t) => &t.view,
            None => &self.proj_default_view,
        };
        let (proj, head) = (pick(self.proj_tex), pick(self.head_tex));
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.headlight_map_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(proj) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.proj_samplers[!self.trilinear as usize]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(head) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(self.refl.as_ref().map_or(&self.refl_dummy_view, |c| &c.array_view)) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(self.csm.view()) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&self.csm.sampler) },
                wgpu::BindGroupEntry { binding: 8, resource: self.csm.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(soft.unwrap_or(&self.soft_dummy_view)) },
                wgpu::BindGroupEntry { binding: 16, resource: self.probe.as_ref().map_or(&self.probe_dummy, |p| &p.uniform).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(self.spot.view()) },
                wgpu::BindGroupEntry { binding: 11, resource: self.spot.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 20, resource: self.lamp.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 24, resource: self.contact.uniform.as_entire_binding() },
            ],
        })
    }

    /// The view about to be drawn is the rear-view mirror's: `[0x6115CC]+0x190` is set from the
    /// view's type (`[[0x628130]+0x10]+0x2C == 1`, 0x43AD39…0x43AD4E; 0x4691D0 makes the mirror's
    /// view with type 1), and the specular and sky-map layers then bind no texture and skip their
    /// passes (0x445578, 0x445B69).
    /// The same flag keeps out the draws only the main view makes (`DrawItem::main_view_only`).
    pub fn set_mirror_view(&mut self, on: bool) {
        self.no_reflections = on;
    }

    /// The bind group for a section map, made the first time a car binds it.
    pub(super) fn sky_bind_group(&mut self, tex: TextureId) {
        let i = tex.0 as usize;
        if self.sky_bgs.get(i).is_some_and(|b| b.is_some()) {
            return;
        }
        let bg = gfx::view_bg(&self.device, "sky map", &self.sky_layout, &self.textures[tex.0 as usize].view);
        if self.sky_bgs.len() <= i {
            self.sky_bgs.resize_with(i + 1, || None);
        }
        self.sky_bgs[i] = Some(bg);
    }

    /// A material's bind group: its texture, the sampler for its address modes under the current
    /// mip filter, and the environment map.
    pub(super) fn material_bind_group(&self, texture: TextureId, sampler: usize, env: TextureId) -> wgpu::BindGroup {
        // ENHANCEMENT ENH-LIVETEX: the HD twin while E41 is on (the original's id otherwise, master's path)
        let texture = self.shown_texture(texture);
        // ENHANCEMENT E2: on, trilinear + anisotropic for every material; off, a texture with a
        // generated chain keeps to level 0 (what the original has), the rest is master's sampler
        let e2 = self.enhancements.is_on(crate::E2);
        let generated = self.textures.get(texture.0 as usize).is_some_and(|t| t.generated.is_some());
        let sampler = if e2 {
            &self.aniso_samplers[sampler]
        } else if generated {
            &self.level0_samplers[sampler]
        } else {
            &self.samplers[sampler | (!self.trilinear as usize) << 2]
        };
        self.material_bind_group_with(texture, sampler, env)
    }

    /// The same bind group with `MAG`/`MINFILTER` `POINT` (Thrash 7 ← 0) — `LayerFalseSort1`'s
    /// other arm, for a `FakeSort` material only.
    pub(super) fn material_bind_group_point(&self, texture: TextureId, sampler: usize, env: TextureId) -> wgpu::BindGroup {
        let texture = self.shown_texture(texture); // ENHANCEMENT ENH-LIVETEX
        // ENHANCEMENT E2 off: a generated chain is not sampled (see `material_bind_group`)
        let generated = self.textures.get(texture.0 as usize).is_some_and(|t| t.generated.is_some());
        let s = if generated && !self.enhancements.is_on(crate::E2) { &self.point_level0_samplers[sampler] } else { &self.point_samplers[sampler] };
        self.material_bind_group_with(texture, s, env)
    }

    pub(super) fn material_bind_group_with(&self, texture: TextureId, sampler: &wgpu::Sampler, env: TextureId) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.textures[texture.0 as usize].view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.textures[env.0 as usize].view) },
                // `WRAP` / `WRAP` (index 0: no clamp); sampled at level 0, so the mip half is moot
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) },
            ],
        })
    }

    /// ENHANCEMENT E2: the level changed (0 off, 1 / 2 / 3 = 4x / 8x / 16x): rebuild the
    /// anisotropic samplers for it and every material's bind groups. Once per change, in the menu
    /// or on the F12 toggle — the texture data is untouched, so nothing is re-uploaded.
    pub(super) fn apply_e2(&mut self, level: u8) {
        let want = match level {
            0 => 1,
            1 => 4,
            2 => 8,
            _ => 16,
        };
        let clamp = if self.aniso_supported { want } else { 1 };
        if clamp != self.aniso_clamp {
            self.aniso_clamp = clamp;
            self.aniso_samplers = std::array::from_fn(|i| make_scene_sampler(&self.device, i & 1 != 0, i & 2 != 0, wgpu::MipmapFilterMode::Linear, clamp, None));
        }
        tracing::info!(level, anisotropy = clamp, supported = self.aniso_supported, "ENHANCEMENT E2");
        self.rebuild_material_bind_groups(None);
    }

    /// Every material's bind groups made again (E2's samplers changed, or ENHANCEMENT ENH-LIVETEX flipped the textures E41 shows).
    /// `only`: just the materials whose own (original) texture is this one.
    pub(super) fn rebuild_material_bind_groups(&mut self, only: Option<TextureId>) {
        for i in 0..self.materials.len() {
            let m = &self.materials[i];
            if only.is_some_and(|t| t != m.texture) {
                continue;
            }
            let (texture, sampler, env, point) = (m.texture, m.sampler, m.env, m.bind_group_point.is_some());
            let bind_group = self.material_bind_group(texture, sampler, env);
            let bind_group_point = point.then(|| self.material_bind_group_point(texture, sampler, env));
            self.materials[i].bind_group = bind_group;
            self.materials[i].bind_group_point = bind_group_point;
        }
    }

    /// `[0x69DD1D]`, the mip filter's promotion (0x535B40 / 0x535A60, `RE/notes/thrash.md` §4):
    /// set, every texture with mips is sampled trilinear; clear, `POINT` between the levels.
    /// 0x44ED10 sets it only at `SPECIALFX` 3 on a card that took `setstate(0x0B, 3)`
    /// (`[0x6197A6]`, taken as accepted); the front end's 0x44F020 clears it.
    pub fn set_trilinear(&mut self, on: bool) {
        if self.trilinear == on {
            return;
        }
        self.trilinear = on;
        for i in 0..self.materials.len() {
            let m = &self.materials[i];
            self.materials[i].bind_group = self.material_bind_group(m.texture, m.sampler, m.env);
        }
        self.rebuild_globals_bg();
    }

    /// `LayerFalseSort1`/`2`'s per-frame pick (`main.rs`'s `App::fakesort_mode`, [`FakeSortMode`]).
    pub fn set_fakesort_mode(&mut self, mode: FakeSortMode) {
        self.fakesort_mode = mode;
    }

    /// State 0x2E's value as the game passes it; `dx7z.dll` 0x600051EE…0x6000522E: `≤ 0` → 0,
    /// not `< 4` → 4.
    pub fn set_gamma(&mut self, g: f32) {
        self.gamma = if g <= 0.0 { 0.0 } else if g < 4.0 { g } else { 4.0 };
    }
}
