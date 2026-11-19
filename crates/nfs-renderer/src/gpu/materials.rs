//! Materials and the bind groups they draw with: create / duplicate, the headlight / projector maps, the globals bind group,
//! the section maps' sky bind groups, the per-material bind groups and their rebuilds (ENHANCEMENT E2 anisotropy, trilinear),
//! the draw-state setters (mirror view, FakeSort mode, gamma).

use crate::gfx;
use super::*;

/// State 0x2E's ceiling, `dx7z.dll` 0x600051EE…0x6000522E (`Core::set_gamma`).
const GAMMA_MAX: f32 = 4.0;

/// A scene sampler's place in the sampler tables (`Core::samplers` and the E2 / level-0 / point ones): the address modes (bit 0
/// `CLAMP` u, bit 1 `CLAMP` v; what `GpuMaterial::sampler` holds) and, in `Core::samplers` only, bit 2: `POINT` between the levels
/// (`[0x69DD1D]` clear, [`Core::set_trilinear`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SamplerKey {
    pub clamp: [bool; 2],
    pub point_mips: bool,
}

impl SamplerKey {
    /// `WRAP` / `WRAP`, the mips filtered.
    pub(super) const WRAP: Self = Self { clamp: [false; 2], point_mips: false };

    /// The key of an address index (`GpuMaterial::sampler`).
    pub(super) const fn of_address(a: usize) -> Self {
        Self { clamp: [a & 1 != 0, a & 2 != 0], point_mips: false }
    }

    /// The key of a `Core::samplers` index.
    pub(super) const fn of_index(i: usize) -> Self {
        Self { clamp: [i & 1 != 0, i & 2 != 0], point_mips: i & 4 != 0 }
    }

    /// The index in the tables of the address modes alone.
    pub(super) const fn address(self) -> usize {
        self.clamp[0] as usize | (self.clamp[1] as usize) << 1
    }

    /// The index in `Core::samplers`.
    pub(super) const fn index(self) -> usize {
        self.address() | (self.point_mips as usize) << 2
    }
}

impl Core {
    pub fn create_material(&mut self, desc: MaterialDesc) -> MaterialId {
        let texture = desc.texture.unwrap_or(self.res.white);
        let env = desc.env_texture.unwrap_or(self.res.white);
        let sampler = SamplerKey { clamp: desc.clamp, point_mips: false }.address();
        let bind_group = self.material_bind_group(texture, sampler, env);
        let bind_group_point = desc.false_sort.then(|| self.material_bind_group_point(texture, sampler, env));
        self.res.materials.push(GpuMaterial {
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
        MaterialId(self.res.materials.len() as u32 - 1)
    }

    /// A new material identical to `id` but sampling `texture` in its place — same blend/cull/lod
    /// state and specular map, a fresh bind group. Used to give one traffic/cop car instance its
    /// own decal-page material when `nfs_game::world::CarModel::instantiate` clones the shared
    /// model's damage resources so two differently-damaged instances in one frame no longer alias
    /// (`RE/notes/car-model.md` §13.1: this renderer batches every frame's `DrawItem`s and reads
    /// each `MeshId`/`TextureId`/`MaterialId` once at `render()`, unlike the original's per-car
    /// synchronous blit 0x572B40).
    pub fn duplicate_material_texture(&mut self, id: MaterialId, texture: TextureId) -> MaterialId {
        let Some(m) = self.res.materials.get(id.0 as usize) else { return id };
        let bind_group = self.material_bind_group(texture, m.sampler, m.env);
        let bind_group_point = m.false_sort.then(|| self.material_bind_group_point(texture, m.sampler, m.env));
        let copy = GpuMaterial { bind_group, bind_group_point, texture, ..m.clone() };
        self.res.materials.push(copy);
        MaterialId(self.res.materials.len() as u32 - 1)
    }

    /// Point every reflective material at another environment map — the original re-binds the car's
    /// environment layers whenever the `.map` section under it changes (`RE/notes/lighting.md` §3).
    /// `night.psh`'s `nght` bitmap, 128 × 128 bytes, for the headlight projector.
    pub fn set_headlight_map(&mut self, map: &[u8]) {
        if map.len() == NIGHT_MAP_BYTES {
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
        let pick = |t: Option<TextureId>| match t.and_then(|t| self.res.textures.get(t.0 as usize)) {
            Some(t) => &t.view,
            None => &self.dummies.proj_default_view,
        };
        GlobalsSet {
            globals: &self.globals_buf,
            night_map: &self.headlight_map_buf,
            proj: pick(self.proj_tex),
            proj_sampler: &self.samplers.proj[!self.modes.trilinear as usize],
            head: pick(self.head_tex),
            refl: self.env.cube.as_ref().map_or(&self.dummies.refl_view, |c| &c.array_view),
            csm: &self.csm,
            soft: soft.unwrap_or(&self.dummies.soft_view),
            probe_sh: self.env.probe.as_ref().map_or(&self.dummies.probe, |p| &p.uniform),
            spot: &self.spot,
            lamp: &self.lamp,
            contacts: &self.contact.buf,
        }
        .bind_group(&self.device, &self.layouts.globals)
    }

    /// The view about to be drawn is the rear-view mirror's: `[0x6115CC]+0x190` is set from the
    /// view's type (`[[0x628130]+0x10]+0x2C == 1`, 0x43AD39…0x43AD4E; 0x4691D0 makes the mirror's
    /// view with type 1), and the specular and sky-map layers then bind no texture and skip their
    /// passes (0x445578, 0x445B69).
    /// The same flag keeps out the draws only the main view makes (`DrawItem::main_view_only`).
    pub fn set_mirror_view(&mut self, on: bool) {
        self.modes.no_reflections = on;
    }

    /// The bind group for a section map, made the first time a car binds it.
    pub(super) fn sky_bind_group(&mut self, tex: TextureId) {
        let i = tex.0 as usize;
        if self.sky_bgs.get(i).is_some_and(|b| b.is_some()) {
            return;
        }
        let bg = gfx::view_bg(&self.device, "sky map", &self.layouts.sky, &self.res.textures[tex.0 as usize].view);
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
        let generated = self.res.textures.get(texture.0 as usize).is_some_and(|t| t.generated.is_some());
        let sampler = if e2 {
            &self.samplers.aniso[sampler]
        } else if generated {
            &self.samplers.level0[sampler]
        } else {
            &self.samplers.scene[SamplerKey { point_mips: !self.modes.trilinear, ..SamplerKey::of_address(sampler) }.index()]
        };
        self.material_bind_group_with(texture, sampler, env)
    }

    /// The same bind group with `MAG`/`MINFILTER` `POINT` (Thrash 7 ← 0) — `LayerFalseSort1`'s
    /// other arm, for a `FakeSort` material only.
    pub(super) fn material_bind_group_point(&self, texture: TextureId, sampler: usize, env: TextureId) -> wgpu::BindGroup {
        let texture = self.shown_texture(texture); // ENHANCEMENT ENH-LIVETEX
        // ENHANCEMENT E2 off: a generated chain is not sampled (see `material_bind_group`)
        let generated = self.res.textures.get(texture.0 as usize).is_some_and(|t| t.generated.is_some());
        let s = if generated && !self.enhancements.is_on(crate::E2) { &self.samplers.point_level0[sampler] } else { &self.samplers.point[sampler] };
        self.material_bind_group_with(texture, s, env)
    }

    pub(super) fn material_bind_group_with(&self, texture: TextureId, sampler: &wgpu::Sampler, env: TextureId) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.layouts.material,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.res.textures[texture.0 as usize].view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.res.textures[env.0 as usize].view) },
                // `WRAP` / `WRAP`; sampled at level 0, so the mip half is moot
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.samplers.scene[SamplerKey::WRAP.index()]) },
            ],
        })
    }

    /// ENHANCEMENT E2: the level changed (0 off, 1 / 2 / 3 = 4x / 8x / 16x): rebuild the
    /// anisotropic samplers for it and every material's bind groups. Once per change, in the menu
    /// or on the A/B toggle — the texture data is untouched, so nothing is re-uploaded.
    pub(super) fn apply_e2(&mut self, level: u8) {
        let want = match level {
            0 => 1,
            1 => 4,
            2 => 8,
            _ => 16,
        };
        let clamp = if self.samplers.aniso_supported { want } else { 1 };
        if clamp != self.samplers.aniso_clamp {
            self.samplers.aniso_clamp = clamp;
            self.samplers.aniso = std::array::from_fn(|i| {
                let k = SamplerKey::of_address(i);
                make_scene_sampler(&self.device, k.clamp[0], k.clamp[1], wgpu::MipmapFilterMode::Linear, clamp, None)
            });
        }
        tracing::info!(level, anisotropy = clamp, supported = self.samplers.aniso_supported, "ENHANCEMENT E2");
        self.rebuild_material_bind_groups(None);
    }

    /// Every material's bind groups made again (E2's samplers changed, or ENHANCEMENT ENH-LIVETEX flipped the textures E41 shows).
    /// `only`: just the materials whose own (original) texture is this one.
    pub(super) fn rebuild_material_bind_groups(&mut self, only: Option<TextureId>) {
        for i in 0..self.res.materials.len() {
            let m = &self.res.materials[i];
            if only.is_some_and(|t| t != m.texture) {
                continue;
            }
            let (texture, sampler, env, point) = (m.texture, m.sampler, m.env, m.bind_group_point.is_some());
            let bind_group = self.material_bind_group(texture, sampler, env);
            let bind_group_point = point.then(|| self.material_bind_group_point(texture, sampler, env));
            self.res.materials[i].bind_group = bind_group;
            self.res.materials[i].bind_group_point = bind_group_point;
        }
    }

    /// `[0x69DD1D]`, the mip filter's promotion (0x535B40 / 0x535A60, `RE/notes/thrash.md` §4):
    /// set, every texture with mips is sampled trilinear; clear, `POINT` between the levels.
    /// 0x44ED10 sets it only at `SPECIALFX` 3 on a card that took `setstate(0x0B, 3)`
    /// (`[0x6197A6]`, taken as accepted); the front end's 0x44F020 clears it.
    pub fn set_trilinear(&mut self, on: bool) {
        if self.modes.trilinear == on {
            return;
        }
        self.modes.trilinear = on;
        for i in 0..self.res.materials.len() {
            let m = &self.res.materials[i];
            self.res.materials[i].bind_group = self.material_bind_group(m.texture, m.sampler, m.env);
        }
        self.rebuild_globals_bg();
    }

    /// `LayerFalseSort1`/`2`'s per-frame pick (`main.rs`'s `App::fakesort_mode`, [`FakeSortMode`]).
    pub fn set_fakesort_mode(&mut self, mode: FakeSortMode) {
        self.modes.fakesort_mode = mode;
    }

    /// State 0x2E's value as the game passes it; `dx7z.dll` 0x600051EE…0x6000522E: `≤ 0` → 0,
    /// not `< 4` → 4 ([`GAMMA_MAX`]).
    pub fn set_gamma(&mut self, g: f32) {
        self.modes.gamma = if g <= 0.0 { 0.0 } else if g < GAMMA_MAX { g } else { GAMMA_MAX };
    }
}

/// What the globals group binds (`classes::binding`): the one list of its entries, for `Core::new`'s first bind group and every
/// rebuild ([`Core::make_globals_bg`]).
pub(super) struct GlobalsSet<'a> {
    pub globals: &'a wgpu::Buffer,
    pub night_map: &'a wgpu::Buffer,
    pub proj: &'a wgpu::TextureView,
    pub proj_sampler: &'a wgpu::Sampler,
    pub head: &'a wgpu::TextureView,
    pub refl: &'a wgpu::TextureView,
    pub csm: &'a csm::CsmState,
    pub soft: &'a wgpu::TextureView,
    pub probe_sh: &'a wgpu::Buffer,
    pub spot: &'a spot::SpotState,
    pub lamp: &'a lamps::LampState,
    pub contacts: &'a wgpu::Buffer,
}

impl GlobalsSet<'_> {
    pub(super) fn bind_group(&self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> wgpu::BindGroup {
        use crate::classes::binding as b;
        fn buf(binding: u32, buf: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: buf.as_entire_binding() }
        }
        fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
        }
        fn sampler(binding: u32, s: &wgpu::Sampler) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::Sampler(s) }
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout,
            entries: &[
                buf(b::GLOBALS, self.globals),
                buf(b::NIGHT_MAP, self.night_map),
                view(b::PROJ_TEX, self.proj),
                sampler(b::PROJ_SAMPLER, self.proj_sampler),
                view(b::HEAD_TEX, self.head),
                view(b::REFL, self.refl),
                view(b::CSM_MAP, self.csm.view()),
                sampler(b::CSM_SAMPLER, &self.csm.sampler),
                buf(b::CSM, &self.csm.uniform.buf),
                view(b::SOFT, self.soft),
                buf(b::PROBE_SH, self.probe_sh),
                view(b::SPOT_MAP, self.spot.view()),
                buf(b::SPOTS, &self.spot.uniform.buf),
                buf(b::LAMPS, &self.lamp.uniform.buf),
                view(b::LAMP_SHADOW, self.lamp.view()),
                buf(b::CONTACTS, self.contacts),
            ],
        })
    }
}
