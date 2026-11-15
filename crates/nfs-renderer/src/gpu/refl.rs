//! ENHANCEMENT E8 / E16 / ENH-COAT: the reflection cube — its faces drawn or uploaded, its prefiltered mips, the car-ambient
//! probe's strength. Moved out of gpu.rs as it was.

use crate::gfx;
use super::*;

/// The cube's faces (`REFL_FACES`), and ENHANCEMENT E49's first own layer (the player's faces follow E8's six).
const FACES: u32 = REFL_FACES.len() as u32;
const OWN: u32 = crate::classes::NEAR_OWN_LAYER;
/// TEST GLUE (`NFS_NEAR_DEBUG`): the shader's levels (`globals.near.w`, `classes.rs`)
const NEAR_DEBUG_SHARP: u8 = crate::classes::NEAR_DEBUG_SHARP as u8;
const NEAR_DEBUG_OVERLAY: u8 = crate::classes::NEAR_DEBUG_OVERLAY as u8;
/// A face texel's top value in bytes (an 8-bit cube's upload), and a second in ms (`NFS_GPU_TIME`'s sums)
const BYTE: f32 = 255.0;
const MS_PER_S: f64 = 1000.0;

impl Core {
    /// ENHANCEMENT E8: draw one face of the player's car's environment — `scene` seen through `camera`
    /// (the car's position, the face's direction, `REFL_TAN_HALF`) into a `size`² target, then copy it
    /// into layer `face` of the array. The array is made (or made again when `size` changes) here; the
    /// map is used once all six faces have been drawn. Recorded into the frame's encoder in call order, so
    /// the main frame that follows samples the finished face. MSAA is not used (1 sample, `size`² is
    /// small and the faces are filtered when read).
    ///
    /// ENHANCEMENT E49 (`near` = the near cars' scene): the cube has twelve layers. The face's pass keeps its depth and is copied
    /// into layer `face` (every car's coat, E16's probe); then `near` is drawn over the same colour and depth (no clear) and the
    /// result copied into layer `OWN + face` (6 + face: the player's own coat; with no car near it is the same face); the face's eye is kept
    /// for the parallax-corrected lookup. `None` = E8's face exactly as before.
    pub(crate) fn render_refl_face(&mut self, job: &crate::front::ReflFaceJob) {
        let crate::front::ReflFaceJob { ref scene, ref camera, face, size, clear, depth_far, ref near } = *job;
        let near = near.as_ref();
        let t0 = self.gpu_time_log.then(std::time::Instant::now);
        let size = size.clamp(16, 1024);
        let face = face.min(FACES - 1);
        // ENHANCEMENT ENH-COAT: the faces are drawn in the float scene's format when the main view is float (the clear coat
        // reflects the sun above white); `cur_hdr` makes `encode_frame` build for it, like the mirror's pass
        let float = self.enh_hdr(scene);
        self.ensure_refl_cube(size, float.is_some(), near.is_some());
        let (tex, view) = {
            let c = self.refl.as_ref().expect("refl cube");
            (c.face.clone(), c.face_view.clone())
        };
        let depth = self.depth_for(size, size);
        let was_mirror = std::mem::replace(&mut self.no_reflections, false);
        crate::prof::ctx(" [env face]");
        self.cur_hdr = float;
        self.refl_building = true;
        self.refl_near_pass = near.map(|_| NearPass::Keep);
        let face_frame = |scene| FrameDesc { scene, clear, with_sprites: false, camera, view: &view, target: &tex, can_copy: false, depth: &depth, aspect: 1.0, depth_far, ms: None };
        self.encode_frame(face_frame(scene));
        let copy_into = |encoder: &mut wgpu::CommandEncoder, array: &wgpu::Texture, layer: u32| {
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyTextureInfo { texture: array, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
        };
        // ENHANCEMENT E49: the near cars over the face: its pass first copies the face into its layer (`refl_pre_copy`, in the same
        // command buffer); with no car near the player's layer is the face itself (its mips are copied, not filtered again)
        let cars_drawn = near.is_some_and(|cars| !cars.items.is_empty());
        if let Some(cars) = near.filter(|_| cars_drawn) {
            crate::prof::ctx(" [env face cars]");
            self.refl_near_pass = Some(NearPass::Over);
            self.refl_pre_copy = Some(face);
            self.encode_frame(face_frame(cars));
        }
        self.refl_near_pass = None;
        self.refl_building = false;
        self.cur_hdr = None;
        crate::prof::ctx("");
        self.no_reflections = was_mirror;
        // the face's copies follow its passes in the frame's encoder
        let mut encoder = self.frame_cmds.take(&self.device);
        // ENHANCEMENT E49 (NEAR-REFL2): the face's alpha marks what was drawn (1) and where the sky is (0), before its copies
        if near.is_some() {
            let format = self.refl.as_ref().expect("refl cube").format;
            self.near_mask(&mut encoder, &view, &depth, format);
        }
        let c = self.refl.as_mut().expect("refl cube");
        if near.is_some() && !cars_drawn {
            copy_into(&mut encoder, &c.array, face);
        }
        copy_into(&mut encoder, &c.array, if near.is_some() { OWN + face } else { face });
        if near.is_some() {
            c.faces_done_own |= 1 << face;
            c.stale_own |= 1 << face;
            if cars_drawn {
                c.own_same &= !(1 << face);
            } else {
                c.own_same |= 1 << face;
            }
            c.eyes[face as usize] = camera.position;
            c.eye = camera.position;
        }
        c.faces_done |= 1 << face;
        c.stale |= 1 << face;
        // ENHANCEMENT E16: the car ambient's SH is projected once a frame, after the frame's faces (`Self::refl_probe`)
        c.probe_due |= c.faces_done == ALL_FACES;
        self.frame_cmds.park(encoder);
        // `NFS_GPU_TIME`: a face's cost is added to the main frame's average (steady state: one frame = main + its faces); the
        // diagnostic submits the face on its own to time it
        if let Some(t0) = t0 {
            self.submit_frame();
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            self.gpu_time_acc.0 += t0.elapsed().as_secs_f64() * MS_PER_S;
        }
    }

    /// ENHANCEMENT E49 (NEAR-REFL2): write the face's geometry mask into its alpha — 1 where the face's passes drew something with
    /// depth (the track, the roadside objects, the near cars), 0 where the depth is still the clear (the sky, which is drawn
    /// without depth, and the far horizon) — so the coat prefilter's mips carry the share of geometry in each lobe and the
    /// paint's sphere-map overlay knows where its sky is covered. One small full-screen pass, only the alpha written.
    fn near_mask(&mut self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, depth: &wgpu::TextureView, format: wgpu::TextureFormat) {
        if self.near_mask.as_ref().is_none_or(|m| m.0 != format) {
            let module = gfx::wgsl_module(&self.device, "near mask", &near_mask_source());
            let layout = gfx::bgl(&self.device, "near mask", &[gfx::bgl_tex(0, wgpu::TextureSampleType::Depth, false)]);
            let pl = gfx::pipeline_layout(&self.device, "near mask", &[&layout]);
            let target = Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALPHA });
            let pipe = gfx::Fullscreen::new("near mask", &pl, &module).build(&self.device, &[target]);
            self.near_mask = Some((format, pipe, layout));
        }
        let (_, pipe, layout) = self.near_mask.as_ref().expect("near mask");
        let bg = gfx::view_bg(&self.device, "near mask", layout, depth);
        let mut pass = gfx::color_pass(encoder, "near mask pass", "E49 face geometry mask", view, wgpu::LoadOp::Load);
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
    }

    /// ENHANCEMENT E8: make the environment cube `size`² (made again when the size or the format changed; the probe starts
    /// over). ENHANCEMENT ENH-COAT: `float` = `Rgba16Float` texels (the float scene), and a chain of [`crate::coat::MIPS`] mips
    /// the clear coat's roughness picks from. ENHANCEMENT E49: `near` = twelve layers (made again when it changes).
    pub(super) fn ensure_refl_cube(&mut self, size: u32, float: bool, near: bool) {
        let format = if float { HDR_FORMAT } else { self.config.format };
        if self.refl.as_ref().is_none_or(|c| c.size != size || c.format != format || c.near != near) {
            let array = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("refl faces"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: if near { 2 * FACES } else { FACES } },
                mip_level_count: crate::coat::MIPS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                // ENHANCEMENT E49: the player's layers take copies of the filtered mips of the others
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT | if near { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
                view_formats: &[],
            });
            let array_view = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            let base_view = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
            let tex = gfx::texture_2d(&self.device, "refl face", size, size, format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
            let face_view = tex.create_view(&Default::default());
            self.refl = Some(ReflCube { size, format, array, array_view, base_view, stale: 0, face: tex, face_view, faces_done: 0, near, stale_own: 0, faces_done_own: 0, own_same: 0, eyes: [glam::Vec3::ZERO; 6], eye: glam::Vec3::ZERO, probe_due: false });
            if self.coat.is_none() {
                self.coat = Some(crate::coat::CoatFilter::new(&self.device));
            }
            if let Some(p) = self.probe.as_mut() {
                p.reset();
            }
            self.rebuild_globals_bg();
        }
    }

    /// ENHANCEMENT E16: project the cube's six faces into the car ambient's SH, once a frame when a face of the full cube was
    /// drawn since the last projection (RENDER-AUDIT §8 item 2: it ran after every face, twice a frame; its blend per update is
    /// the two updates' together, [`crate::probe::BLEND_FRAME`], so the ambient follows the surroundings as fast as before).
    /// Recorded into `encoder` ahead of the frame that reads the uniform (with the coat prefilter, [`Self::refl_prefilter`]).
    fn refl_probe(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(c) = self.refl.as_mut().filter(|c| c.probe_due) else { return };
        c.probe_due = false;
        if !(self.enhancements.is_on(crate::E16) && self.enhancements.is_on(crate::E8) && self.compute_supported) {
            return;
        }
        let built = self.probe.is_some();
        let p = self.probe.get_or_insert_with(|| crate::probe::Probe::new(&self.device));
        p.update(&self.device, &mut self.belt, encoder, &c.base_view, c.size, crate::probe::BLEND_FRAME);
        if !built {
            self.rebuild_globals_bg();
        }
    }

    /// ENHANCEMENT ENH-COAT: the prefiltered mips of the faces drawn since the last run, once all six are in (the filter of
    /// one face reads its neighbours' mip 0). Recorded into `encoder` ahead of the frame that reads the mips; E16's probe of
    /// the frame's faces first ([`Self::refl_probe`]).
    pub(super) fn refl_prefilter(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.refl_probe(encoder);
        let (Some(c), Some(coat)) = (self.refl.as_mut(), self.coat.as_mut()) else { return };
        // ENHANCEMENT E49: the player's layers 6..11 like the others, once all six of them are in
        let own = c.near && c.stale_own != 0 && c.faces_done_own == ALL_FACES;
        if (c.stale == 0 || c.faces_done != ALL_FACES) && !own {
            return;
        }
        if c.stale != 0 && c.faces_done == ALL_FACES {
            coat.record(&self.device, &mut self.belt, encoder, &c.array, c.format, c.size, c.stale, 1.0 / REFL_TAN_HALF, 0);
            c.stale = 0;
        }
        if own {
            // a face with no car near is the same in both layers: its filtered mips are copied (after their filter above)
            coat.record(&self.device, &mut self.belt, encoder, &c.array, c.format, c.size, c.stale_own & !c.own_same, 1.0 / REFL_TAN_HALF, OWN);
            for f in 0..FACES {
                if c.stale_own & c.own_same & (1 << f) == 0 {
                    continue;
                }
                for m in 1..crate::coat::MIPS {
                    let s = (c.size >> m).max(1);
                    encoder.copy_texture_to_texture(
                        wgpu::TexelCopyTextureInfo { texture: &c.array, mip_level: m, origin: wgpu::Origin3d { x: 0, y: 0, z: f }, aspect: wgpu::TextureAspect::All },
                        wgpu::TexelCopyTextureInfo { texture: &c.array, mip_level: m, origin: wgpu::Origin3d { x: 0, y: 0, z: OWN + f }, aspect: wgpu::TextureAspect::All },
                        wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
                    );
                }
            }
            c.stale_own = 0;
        }
    }

    /// ENHANCEMENT E49: the near lookup's globals of `scene`'s frame (`fill::NearFrame`): the cube is the twelve-layer one with all its
    /// faces, E49 and E8 on, a view that takes the map (not the mirror, not a face being drawn); `None` = off (the globals' zeros).
    pub(super) fn near_frame(&self, scene: &Scene) -> Option<fill::NearFrame> {
        let c = self.refl.as_ref()?;
        let on = self.enhancements.is_on(crate::E49) && self.enhancements.is_on(crate::E8) && c.near && c.faces_done == ALL_FACES && scene.refl > 0.0 && !self.no_reflections && !self.refl_building;
        // TEST GLUE: `NFS_NEAR_DEBUG=1` shows the reflection alone on the paint; `=2` also without the box (the plain lookup); `=3` / `=4`
        // the same as a mirror (mip 0); `=5` (NEAR-REFL2) the sphere-map overlay's geometry weights alone (the shader's `env_over`)
        let debug = crate::dbg::flags().near_debug;
        let proxy = if debug % 2 == 0 && debug > 0 { None } else { near_proxy(&scene.cover, c.eye) };
        on.then(|| fill::NearFrame { own: c.faces_done_own == ALL_FACES, eyes: c.eyes, eye: c.eye, proxy, sky: scene.near_sky, debug: match debug { 5 => NEAR_DEBUG_OVERLAY, d if d >= 3 => NEAR_DEBUG_SHARP, d => u8::from(d > 0) } })
    }

    /// ENHANCEMENT ENH-FE3D: fill all six faces of the environment cube from the CPU — `faces` are six `size x size`
    /// RGBA8 images in `REFL_FACES` order — for a front-end view that has no room round the car (a studio the app
    /// paints). The map is used at once; the car ambient's probe (E16) is projected from it like from drawn faces.
    pub(crate) fn upload_refl_faces(&mut self, size: u32, faces: &[Vec<f32>]) {
        let size = size.clamp(16, 1024);
        if faces.len() != 6 || faces.iter().any(|f| f.len() != (size * size * 4) as usize) {
            return;
        }
        // ENHANCEMENT ENH-COAT: float texels (radiance above 1: a studio's soft box), or the surface's 8 bits
        let float = self.hdr_ok && self.enhancements.is_on(crate::E7);
        self.ensure_refl_cube(size, float, false);
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let c = self.refl.as_mut().expect("refl cube");
        for (face, px) in faces.iter().enumerate() {
            let mut data: Vec<u8> = Vec::new();
            if float {
                data.reserve(px.len() * 2);
                for v in px {
                    data.extend_from_slice(&f32_to_f16(*v).to_le_bytes());
                }
            } else {
                data.extend(px.iter().map(|v| (v.clamp(0.0, 1.0) * BYTE + 0.5) as u8));
                if bgra {
                    for t in data.chunks_exact_mut(4) {
                        t.swap(0, 2);
                    }
                }
            }
            let texel = if float { 8 } else { 4 };
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &c.array, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: face as u32 }, aspect: wgpu::TextureAspect::All },
                &data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * texel), rows_per_image: Some(size) },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
        }
        c.faces_done = ALL_FACES;
        c.stale = ALL_FACES;
        if self.enhancements.is_on(crate::E16) && self.enhancements.is_on(crate::E8) && self.compute_supported {
            let built = self.probe.is_some();
            let p = self.probe.get_or_insert_with(|| crate::probe::Probe::new(&self.device));
            let (array_view, size) = (c.base_view.clone(), c.size);
            let mut encoder = self.frame_cmds.take(&self.device);
            p.update(&self.device, &mut self.belt, &mut encoder, &array_view, size, crate::probe::BLEND);
            self.frame_cmds.park(encoder);
            if !built {
                self.rebuild_globals_bg();
            }
        }
    }

    /// ENHANCEMENT E16: the weight the car shader gives the probe's irradiance this frame (0 = the original's
    /// scalar ambient): E16 and E8 on, all six faces drawn, the probe projected at least once, a scene that
    /// takes the environment map (`Scene::refl`).
    pub(super) fn probe_strength(&self, scene: &Scene) -> f32 {
        let ready = self.enhancements.is_on(crate::E16) && self.enhancements.is_on(crate::E8) && scene.refl > 0.0 && self.probe.as_ref().is_some_and(|p| p.ready());
        if !ready { 0.0 } else if self.enhancements.level(crate::E16) >= crate::Enhancements::E16_STRONGER { crate::probe::STRENGTH_STRONG } else { crate::probe::STRENGTH }
    }
}

/// ENHANCEMENT E49 (NEAR-REFL2): the geometry mask of an environment face (`Core::near_mask`): the face's depth, single-sampled,
/// read texel by texel; anything nearer than the clear (1.0) is geometry.
pub(crate) fn near_mask_source() -> String {
    crate::wgsl::compose(&[
        crate::wgsl::FULLSCREEN_VS,
        r#"
@group(0) @binding(0) var depth: texture_depth_2d;

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let d = textureLoad(depth, vec2<i32>(pos.xy), 0);
    return vec4<f32>(0.0, 0.0, 0.0, select(0.0, 1.0, d < 1.0));
}
"#,
    ])
}
