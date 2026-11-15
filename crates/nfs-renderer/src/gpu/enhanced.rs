//! The enhanced frame: the gamma / timing wrapper, the `enh_*` selectors (samples, float scene, SMAA, blur, depth of field,
//! FSR / MetalFX, render scale), the temporal view's state, the multisampled targets, the post chain `encode_enhanced` and
//! the macOS EDR output. Moved out of gpu.rs as it was.

use crate::gfx::{PostCtx, RtImage};
use super::*;
use crate::prof::Stage;

/// `NFS_GPU_TIME` (diagnostic): the frames one logged mean is taken over.
const GPU_TIME_FRAMES: u32 = 30;

impl Core {
    /// [`Self::encode_with_gamma_inner`], timed when `NFS_GPU_TIME=1` (diagnostic, off by default):
    /// the frame's encode + GPU time, the device waited for after the submit, averaged over [`GPU_TIME_FRAMES`] frames.
    pub(super) fn encode_with_gamma(&mut self, scene: &Scene, camera: &Camera, out: FrameOut) {
        self.whole_frame(|core| core.encode_with_gamma_inner(scene, camera, out));
    }

    /// One frame: what the command list recorded ahead of the main view submitted first ([`Self::submit_ahead_of_main`]),
    /// then `record` (the main view; its post chain is submitted apart, `encode_enhanced`), then the frame's last submit
    /// ([`Self::submit_frame`]) and the profiler's frame end; timed when `NFS_GPU_TIME=1`.
    fn whole_frame(&mut self, record: impl FnOnce(&mut Self)) {
        let t0 = self.gpu_time_log.then(std::time::Instant::now);
        self.submit_ahead_of_main();
        self.frame_main = true;
        record(self);
        self.frame_main = false;
        self.submit_frame();
        crate::prof::end_frame(&self.device);
        if let Some(t0) = t0 {
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            self.gpu_time_acc.0 += crate::prof::elapsed_ms(t0);
            self.gpu_time_acc.1 += 1;
            if self.gpu_time_acc.1 == GPU_TIME_FRAMES {
                tracing::info!(avg_ms = self.gpu_time_acc.0 / f64::from(GPU_TIME_FRAMES), enh = %self.enhancements.describe(), "frame encode + gpu time");
                self.gpu_time_acc = (0.0, 0);
            }
        }
    }

    /// Draw a frame into `out.view` (`width × height`, the surface's format): straight in at gamma
    /// 1, otherwise into the gamma pass's own image and then through the ramp into `out.view`.
    /// `out.can_copy`: `out.tex` (or the gamma pass's own image) may be copied from — the lens
    /// flare's blocks read the frame **before** the ramp, as `THRASH_readrect` reads the back
    /// buffer and the ramp is the display's.
    pub(super) fn encode_with_gamma_inner(&mut self, scene: &Scene, camera: &Camera, out: FrameOut) {
        let FrameOut { view: target, tex: target_tex, can_copy, depth, width, height, aspect } = out;
        // ENHANCEMENT E1 / E3: a multisampled and / or scaled 3D image; with both at level 0 this
        // function is master's below
        // ENHANCEMENT E7: ... and / or drawn into a float image and tone-mapped
        let hdr = self.enh_hdr(scene);
        // ENHANCEMENT ENH-TEMPORAL: the main view of a temporal frame renders at one sample (a temporal upscaler replaces
        // MSAA; E1's level is overridden for this frame, the setting is not changed)
        self.up.frame_no += 1;
        // ENHANCEMENT E37: FSR 3.1 (built here at first use) replaces MSAA, E34's scaler, E3's scale and E35's SMAA
        let fsr = self.enh_fsr(scene, hdr.is_some());
        self.up.fsr_active = fsr.is_some();
        let temporal = self.temporal_frame(scene, hdr.is_some());
        // ENHANCEMENT E34: the upscaler's internal scale replaces E3's
        let up = if fsr.is_some() { None } else { self.enh_upscale(scene) };
        self.up.upscale_active = up.is_some();
        let (samples, scale) = (if temporal { 1 } else { self.enh_samples(hdr.is_some()) }, fsr.or(up).unwrap_or_else(|| self.enh_scale()));
        // ENHANCEMENT E35: SMAA needs the tone-mapped image of the scaled path (the 3D image, then the blit)
        let smaa = self.enh_smaa(scene) && fsr.is_none();
        if samples > 1 || scale != 1.0 || hdr.is_some() || smaa {
            self.encode_enhanced(scene, camera, out, EnhancedFrame { samples, scale, hdr, temporal });
            return;
        }
        if self.gamma == 1.0 {
            self.encode_frame(FrameDesc { scene, clear: scene.clear_color, with_sprites: true, camera, view: target, target: target_tex, can_copy, depth, aspect, depth_far: VIEW_FAR, ms: None });
            return;
        }
        let mut pass = match self.gamma_pass.take() {
            Some(p) => p,
            None => GammaPass::new(&self.device, self.config.format),
        };
        let (src_tex, src) = pass.source(&self.device, self.config.format, width, height);
        self.encode_frame(FrameDesc { scene, clear: scene.clear_color, with_sprites: true, camera, view: &src, target: &src_tex, can_copy: true, depth, aspect, depth_far: VIEW_FAR, ms: None });
        let mut encoder = self.frame_cmds.take(&self.device);
        pass.resolve(&mut self.belt, &mut encoder, target, self.gamma);
        self.frame_cmds.park(encoder);
        self.gamma_pass = Some(pass);
    }

    /// ENHANCEMENT ENH-TEMPORAL: this frame is a temporal main view — the foundation is on (`NFS_TEMPORAL_DEBUG`, or a temporal
    /// upscaler wants it, `temporal_want`) and the frame is the race's main view in the float scene (E7 on; the mirror, the environment
    /// faces, the front end and the 2D screens are not).
    pub(super) fn temporal_frame(&self, scene: &Scene, hdr: bool) -> bool {
        // ENHANCEMENT E37: FSR 3.1 raises the flag itself while its switch is on (and the backend did not fail)
        (self.temporal.is_some() || self.temporal_want().is_some()) && hdr && scene.eye && !scene.items.is_empty() && !self.no_reflections
    }

    /// ENHANCEMENT ENH-TEMPORAL: start a temporal main-view frame of the internal size `size` (the display being `display_width`
    /// wide): the jitter, the targets, the object pass's bind group. The pass built for the debug view is made on first use.
    pub(super) fn temporal_begin(&mut self, camera: &Camera, aspect: f32, mirror: bool, size: (u32, u32), display_width: u32) -> crate::temporal::Cur {
        if self.temporal_gpu.is_none() {
            self.temporal_gpu = Some(crate::temporal::Gpu::new(&self.device, &self.globals_layout, &self.material_layout, &self.sky_layout, &self.object_buf, std::mem::size_of::<ObjectUniform>() as u64, MAX_OBJECTS));
        }
        let t = self.temporal.get_or_insert_with(|| crate::temporal::Temporal::new(crate::temporal::Config { view: crate::temporal::View::Plain, no_obj: false, no_mv: false, no_id: crate::dbg::flags().temporal_noid }));
        let info = t.begin(camera, aspect, mirror, size, display_width);
        if t.targets.as_ref().is_none_or(|x| x.size != size) {
            t.targets = Some(crate::temporal::Targets::new(&self.device, size));
        }
        let tg = t.targets.as_ref().expect("temporal targets");
        crate::temporal::Cur {
            mv: tg.mv_view.clone(),
            reactive: tg.reactive_view.clone(),
            tc: tg.tc_view.clone(),
            // MetalFX temporal (`upscale/metalfx_temporal.rs`) reads the colour, depth, vectors and reactive mask only
            tc_read: !(self.up.fsr_active && self.up.fsr_kind.is_some_and(|k| k != crate::upscale::UpscalerKind::Fsr3)),
            objects_bg: self.temporal_gpu.as_ref().expect("temporal gpu").objects_bg.clone(),
            jitter_ndc: info.jitter_ndc,
            jitter_px: info.jitter_px,
            phase_count: info.phase_count,
            size,
            reset: info.reset,
            frame: info.frame,
        }
    }

    /// ENHANCEMENT E1: the sample count the level asks for, the highest the adapter supports for
    /// both formats that is `<=` it; 1 = no multisampling (level 0, or nothing supported).
    pub(super) fn enh_samples(&self, hdr: bool) -> u32 {
        if !self.enhancements.is_on(crate::E1) {
            return 1;
        }
        let supported = if hdr { self.ms_supported_hdr } else { self.ms_supported };
        let want = crate::msaa_samples_wanted(self.enhancements.level(crate::E1));
        [8u32, 4, 2].into_iter().find(|&n| n <= want && supported & n != 0).unwrap_or(1)
    }

    /// ENHANCEMENT E7: the tone-map mode of the main 3D pass — 0 = "as the original" (level 1), 1 =
    /// ACES (level 2) — or `None` for the original's 8-bit path: the switch is off, the device cannot
    /// do the float target, or the scene has no 3D item (a front-end screen of sprites only stays 2D
    /// and untouched).
    pub(super) fn enh_hdr(&self, scene: &Scene) -> Option<u32> {
        if !self.enhancements.is_on(crate::E7) || !self.hdr_ok || scene.items.is_empty() {
            return None;
        }
        Some(crate::tonemap_mode(self.enhancements.level(crate::E7)))
    }

    /// ENHANCEMENT E35: SMAA this frame — the switch on and a scene with a 3D item (a front-end screen of sprites
    /// only stays 2D and untouched, like E7).
    pub(super) fn enh_smaa(&self, scene: &Scene) -> bool {
        self.enhancements.is_on(crate::E35) && !scene.items.is_empty()
    }

    /// ENHANCEMENT E27: the motion blur's level, the previous frame and whether this frame can have it (the float
    /// scene of the race's main view, the switch on).
    pub(super) fn enh_blur(&self, scene: &Scene, hdr: bool) -> Option<(u8, crate::MotionPrev)> {
        if !hdr || !self.enhancements.is_on(crate::E27) || scene.items.is_empty() {
            return None;
        }
        scene.motion.map(|m| (self.enhancements.level(crate::E27), m))
    }

    /// ENHANCEMENTS E7 / E31: this frame's depth of field, when the scene brings a lens (the float scene only): the photo
    /// mode's (PORT-DEVIATION D46, set by the application only with E7 on) or a replay's (E31).
    pub(super) fn enh_dof(&self, scene: &Scene, hdr: bool) -> Option<crate::Dof> {
        if !hdr || scene.items.is_empty() {
            return None;
        }
        scene.dof
    }

    /// ENHANCEMENTS E10 / E28: the bloom's level and the lens dirt on its last add (the tone map's key), `None` = no bloom
    /// (`hdr`: the float scene; `dbg_only`: a temporal debug view, no chain).
    pub(super) fn bloom_key(&self, hdr: bool, dbg_only: bool) -> Option<(u8, bool)> {
        let level = if hdr && !dbg_only { self.enhancements.level(crate::E10) } else { 0 };
        (level > 0).then(|| (level, self.enhancements.is_on(crate::E28)))
    }

    /// ENHANCEMENTS E27 / the depth of field (photo mode, E31) / E40 / E10 / E28: which passes of the post chain run this frame (`size`: the chain's;
    /// `hdr`: the float scene; `dbg_only`: a temporal debug view, no chain). The eye adaptation (E21) is the caller's.
    pub(super) fn post_plan(&self, scene: &Scene, camera: &Camera, aspect: f32, hdr: bool, dbg_only: bool, size: (u32, u32)) -> chain::PostPlan {
        // ENHANCEMENT E40: the driver view's windscreen of a rainy track, its glass on the screen
        let drops = scene.drops.filter(|_| self.enhancements.is_on(crate::E40) && hdr && !scene.mirror && !scene.items.is_empty() && !dbg_only).and_then(|dr| {
            let look = crate::drops::Look { strength: 1.0, sky: [scene.clear_color[0] as f32, scene.clear_color[1] as f32, scene.clear_color[2] as f32], width: size.0, height: size.1 };
            crate::drops::glass(&dr, camera, aspect).map(|gl| (dr, gl, look))
        });
        chain::PostPlan {
            blur: self.enh_blur(scene, hdr).filter(|_| !dbg_only),
            dof: self.enh_dof(scene, hdr).filter(|_| !dbg_only),
            drops,
            bloom_level: self.bloom_key(hdr, dbg_only).map_or(0, |b| b.0),
            dirt: self.enhancements.is_on(crate::E28),
            eye: false,
        }
    }

    /// ENHANCEMENT E37: the internal scale of the main view when FSR 3.1 takes the frame - the switch on, the backend built
    /// (here, at first use; unavailable = logged once, then it behaves as off) and the frame a temporal main view (the race's
    /// float scene). The history is dropped when the last FSR frame was not the call before (a menu, a loading screen, another
    /// view in between).
    pub(super) fn enh_fsr(&mut self, scene: &Scene, hdr: bool) -> Option<f32> {
        if !hdr || !scene.eye || scene.items.is_empty() || self.no_reflections {
            return None;
        }
        let (kind, sw) = self.temporal_want()?;
        if self.up.fsr_kind != Some(kind) {
            // the other backend's frame (a level change between FSR and MetalFX): its objects go, the output image is remade
            self.up.fsr = None;
            self.up.fsr_out = None;
            self.up.fsr_kind = None;
        }
        if self.up.fsr.is_none() {
            match crate::upscale::create(kind, &self.device, &self.queue) {
                Ok(u) => {
                    tracing::info!("upscaler (ENHANCEMENT {}): {}", crate::enhancements::REGISTRY[sw.0 as usize].token, u.name());
                    self.up.fsr = Some(u);
                    self.up.fsr_kind = Some(kind);
                    *self.temporal_state_mut(kind) = BackendState::Ready;
                }
                Err(why) => {
                    tracing::warn!("upscaler (ENHANCEMENT {}) unavailable, behaving as off: {why}", crate::enhancements::REGISTRY[sw.0 as usize].token);
                    *self.temporal_state_mut(kind) = BackendState::Unavailable;
                    return None;
                }
            }
        }
        if self.up.fsr_last_frame + 1 != self.up.frame_no {
            self.fsr_cut();
        }
        self.up.fsr_last_frame = self.up.frame_no;
        self.up.fsr.as_ref().map(|u| u.render_scale(self.enhancements.level(sw)))
    }

    /// ENHANCEMENT E37 / E34: the temporal upscaler this frame wants - FSR 3.1 (E37), else on macOS MetalFX's temporal scaler (E34 levels
    /// 1..=4; the levels above are the spatial ones) - unless that backend was found unavailable or failed.
    pub(super) fn temporal_want(&self) -> Option<(crate::upscale::UpscalerKind, crate::enhancements::Switch)> {
        if self.enhancements.is_on(crate::E37) && self.up.fsr_state != BackendState::Unavailable {
            return Some((crate::upscale::UpscalerKind::Fsr3, crate::E37));
        }
        #[cfg(target_os = "macos")]
        if (1..=crate::E34_TEMPORAL_LEVELS).contains(&self.enhancements.level(crate::E34)) && self.up.mfxt_state != BackendState::Unavailable {
            return Some((crate::upscale::UpscalerKind::MetalFxTemporal, crate::E34));
        }
        None
    }

    pub(super) fn temporal_state_mut(&mut self, kind: crate::upscale::UpscalerKind) -> &mut BackendState {
        match kind {
            crate::upscale::UpscalerKind::Fsr3 => &mut self.up.fsr_state,
            #[cfg(target_os = "macos")]
            _ => &mut self.up.mfxt_state,
        }
    }

    /// The temporal backend in use failed: it behaves as off from the next frame.
    pub(super) fn temporal_dead(&mut self) {
        if let Some(kind) = self.up.fsr_kind {
            *self.temporal_state_mut(kind) = BackendState::Unavailable;
        }
    }

    /// ENHANCEMENT E37: a camera cut: FSR's history and the vectors' previous frame are dropped.
    pub(super) fn fsr_cut(&mut self) {
        if let Some(f) = self.up.fsr.as_mut() {
            f.reset_history();
        }
        if let Some(t) = self.temporal.as_mut() {
            t.cut();
        }
    }

    /// ENHANCEMENT E34: the internal scale of the main 3D image when the upscaler is on and available (the backend
    /// is built here at first use; unavailable = logged once, then it behaves as off).
    #[cfg(target_os = "macos")]
    pub(super) fn enh_upscale(&mut self, scene: &Scene) -> Option<f32> {
        // E34's levels 1..=4 are the temporal ones (`temporal_want`); the spatial ones come after
        if self.enhancements.level(crate::E34) <= crate::E34_TEMPORAL_LEVELS || scene.items.is_empty() {
            return None;
        }
        if self.up.upscale_state == BackendState::Untried {
            match crate::upscale::create(crate::upscale::UpscalerKind::MetalFx, &self.device, &self.queue) {
                Ok(u) => {
                    tracing::info!("upscaler (ENHANCEMENT E34): {}", u.name());
                    self.up.upscaler = Some(u);
                    self.up.upscale_state = BackendState::Ready;
                }
                Err(why) => {
                    tracing::warn!("upscaler (ENHANCEMENT E34) unavailable, behaving as off: {why}");
                    self.up.upscale_state = BackendState::Unavailable;
                }
            }
        }
        self.up.upscaler.as_ref().map(|u| u.render_scale(self.enhancements.level(crate::E34)))
    }

    #[cfg(not(target_os = "macos"))]
    pub(super) fn enh_upscale(&mut self, _scene: &Scene) -> Option<f32> {
        None
    }

    /// ENHANCEMENT E3: the render scale of the level (level 0 = 1.0, the original 1:1).
    pub(super) fn enh_scale(&self) -> f32 {
        crate::render_scale(self.enhancements.level(crate::E3))
    }

    /// ENHANCEMENT E1: the multisampled colour and depth of a `width × height` 3D pass (kept per
    /// size); `None` for 1 sample.
    pub(super) fn ms_targets(&mut self, width: u32, height: u32, samples: u32, hdr: bool) -> Option<Ms> {
        if samples <= 1 {
            return None;
        }
        let key = (width, height, samples | if hdr { HDR_KEY as u32 } else { 0 });
        if self.ms_cache.len() > TARGET_CACHE_SIZES && !self.ms_cache.contains_key(&key) {
            self.ms_cache.clear();
        }
        let device = &self.device;
        let format = if hdr { HDR_FORMAT } else { self.config.format };
        let (color, depth) = self
            .ms_cache
            .entry(key)
            .or_insert_with(|| {
                let make = |label: &str, format| {
                    device
                        .create_texture(&wgpu::TextureDescriptor {
                            label: Some(label),
                            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                            mip_level_count: 1,
                            sample_count: samples,
                            dimension: wgpu::TextureDimension::D2,
                            format,
                            // the depth of a multisampled pass is read by E12's converter
                            usage: if format == DEPTH_FORMAT { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING } else { wgpu::TextureUsages::RENDER_ATTACHMENT },
                            view_formats: &[],
                        })
                        .create_view(&Default::default())
                };
                (make("msaa colour", format), make("msaa depth", DEPTH_FORMAT))
            })
            .clone();
        Some(Ms { color, depth, samples })
    }

    /// ENHANCEMENT E1 / E3: a frame whose 3D image is multisampled (`samples > 1`) and / or drawn at
    /// `scale` of the window's size and scaled to it. The 2D sprites (HUD, front end) are drawn at the
    /// window's size after the scaled 3D image, 1 sample; the gamma ramp runs last on the whole.
    pub(super) fn encode_enhanced(&mut self, scene: &Scene, camera: &Camera, out: FrameOut, how: EnhancedFrame) {
        let FrameOut { view: target, tex: target_tex, can_copy, depth, width, height, aspect } = out;
        let EnhancedFrame { samples, scale, hdr, temporal } = how;
        let mut pass = None;
        // the window-sized image the frame is composed in: the target itself at gamma 1
        let (dest, dest_tex, dest_copy) = if self.gamma == 1.0 {
            (target.clone(), target_tex.clone(), can_copy)
        } else {
            let mut p = match self.gamma_pass.take() {
                Some(p) => p,
                None => GammaPass::new(&self.device, self.config.format),
            };
            let (t, v) = p.source(&self.device, self.config.format, width, height);
            pass = Some(p);
            (v, t, true)
        };
        // ENHANCEMENT E37: FSR 3.1 replaces SMAA (and MSAA, the scale)
        let fsr_on = self.up.fsr_active;
        let smaa = self.enh_smaa(scene) && !fsr_on;
        if scale == 1.0 && hdr.is_none() && !smaa && !fsr_on {
            let ms = self.ms_targets(width, height, samples, false);
            self.encode_frame(FrameDesc { scene, clear: scene.clear_color, with_sprites: true, camera, view: &dest, target: &dest_tex, can_copy: dest_copy, depth, aspect, depth_far: VIEW_FAR, ms: ms.as_ref() });
        } else {
            let max = self.device.limits().max_texture_dimension_2d;
            let (iw, ih) = (((width as f32 * scale).round() as u32).clamp(1, max), ((height as f32 * scale).round() as u32).clamp(1, max));
            // ENHANCEMENT ENH-TEMPORAL: a debug view that shows the vectors / masks as they are: no post chain over it
            let dbg_only = temporal && self.temporal.as_ref().is_some_and(|t| matches!(t.cfg.view, crate::temporal::View::Velocity | crate::temporal::View::Reactive | crate::temporal::View::Reproj));
            // ENHANCEMENT E21: the eye adaptation of the race's main view (the float scene, the switch on)
            let eye = hdr.is_some() && scene.eye && self.enhancements.is_on(crate::E21) && !dbg_only;
            if eye && self.post.eye.is_none() {
                self.post.eye = Some(crate::adapt::EyePass::new(&self.device, &self.sprite_layout));
            }
            // ENHANCEMENT E19: dither the float image's rounding to 8 bits (needs the float image)
            let dither = hdr.is_some() && self.enhancements.is_on(crate::E19);
            // ENHANCEMENT E33: the tone map also writes the highlight gain map for the EDR present pass
            let edr = hdr.is_some() && self.edr.pass;
            // ENHANCEMENT E10 (PERF): the tone map adds the bloom's last level (the chain is made here: the tone map takes its group's
            // layout)
            let bloom = self.bloom_key(hdr.is_some(), dbg_only);
            if bloom.is_some() && self.post.bloom.is_none() {
                self.post.bloom = Some(crate::bloom::BloomPass::new(&self.device, &self.sprite_layout));
            }
            let key = ToneKey { hdr, dither, eye, edr, bloom };
            let mut blit = match self.scale_blit.take() {
                Some(b) if b.key == key => b,
                _ => {
                    let eye_layout = if eye { self.post.eye.as_ref().map(|e| &e.state_layout) } else { None };
                    let bloom_layout = if bloom.is_some() { self.post.bloom.as_ref().map(|b| &b.tone_layout) } else { None };
                    ScaleBlit::new(&self.device, self.config.format, &self.sprite_layout, key, eye_layout, bloom_layout)
                }
            };
            let (rt_tex, rt_view) = blit.target(&self.device, &self.sprite_layout, &self.sprite_sampler, iw, ih);
            let ms = self.ms_targets(iw, ih, samples, hdr.is_some());
            // the single-sample depth of the scaled 3-D image; a multisampled frame draws and reads its samples' depth only
            // (`FrameTargets::depth`, `chain::depth_src`), so it takes no image of its own
            let depth3 = match &ms {
                Some(m) => m.depth.clone(),
                None => self.depth_for(iw, ih),
            };
            // ENHANCEMENT E37: the post chain (E27 / the depth of field / E10 / E21 / the tone map) works at this size: the window's with FSR
            // (it runs on FSR's output), the internal one otherwise
            let (pw, ph) = if fsr_on { (width, height) } else { (iw, ih) };
            // the lens probe reads this image, at the probe's pixel in its (scaled) size
            // ENHANCEMENT E7: `cur_hdr` makes `encode_frame` build for the float image
            // ENHANCEMENT ENH-TEMPORAL: the jitter, the vector / mask targets, the temporal pipeline set
            self.cur_temporal = if temporal { Some(self.temporal_begin(camera, aspect, scene.mirror, (iw, ih), width)) } else { None };
            self.cur_hdr = hdr;
            // ENHANCEMENT UPSCALE-SOFT: an upscaler's view (FSR 3.1 / MetalFX temporal, MetalFX spatial) samples its textures for the
            // window's pixel (`upscale::mip_bias`); E3's plain scale and the debug TAA keep the materials' own bias
            self.cur_lod_shift = if fsr_on {
                crate::upscale::mip_bias(iw, width, true)
            } else if self.up.upscale_active && self.up.upscaler.is_some() {
                crate::upscale::mip_bias(iw, width, false)
            } else {
                0.0
            };
            self.encode_frame(FrameDesc { scene, clear: scene.clear_color, with_sprites: false, camera, view: &rt_view, target: &rt_tex, can_copy: true, depth: &depth3, aspect, depth_far: VIEW_FAR, ms: ms.as_ref() });
            self.cur_lod_shift = 0.0;
            self.cur_hdr = None;
            // PERF (picture identical): the main view's 3-D image goes to the GPU before its post chain is encoded, so the GPU
            // draws it while the CPU records the chain (the queue runs the submits in order; RE/notes/RENDER-AUDIT-2026-11-15.md §12)
            if self.frame_main {
                self.submit_frame();
            }
            // the post chain follows the 3-D image's passes in the frame's encoder
            let mut encoder = self.frame_cmds.take(&self.device);
            let mut sprites = std::mem::take(&mut self.frame_scratch.sprites);
            let n = self.upload_sprites(&mut encoder, scene, true, aspect, &mut sprites);
            self.frame_scratch.sprites = sprites;
            // the scene image the post chain carries: the scaled 3-D image until a pass makes one of its own
            let mut chain = chain::PostChain::new(blit.image_bind_group(), &rt_view);
            let depth_src = chain::depth_src(ms.as_ref(), &depth3);
            // ENHANCEMENT ENH-TEMPORAL: the debug pass over the float scene (the TAA resolve, or a debug view of the vectors / masks)
            if let (Some(t), Some(tp)) = (self.cur_temporal.take(), self.temporal.as_mut()) {
                if let Some(tg) = tp.targets.as_ref() {
                    self.temporal_inputs = Some(crate::temporal::Inputs {
                        color: rt_tex.clone(),
                        depth: depth3.texture().clone(),
                        motion_vectors: tg.mv.clone(),
                        reactive_mask: tg.reactive.clone(),
                        transparency_and_composition: tg.tc.clone(),
                        jitter_offset: t.jitter_px,
                        render_size: [iw, ih],
                        reset_history: t.reset,
                        camera_near: camera.near,
                        camera_far: camera.far,
                        camera_fov_y: camera.fov_y_radians,
                        pre_exposure: 1.0,
                    });
                }
                let (view, no_mv) = (tp.cfg.view, tp.cfg.no_mv);
                if !fsr_on {
                    let pass = tp.pass.get_or_insert_with(|| crate::temporal::Pass::new(&self.device, view, no_mv));
                    let mut cx = PostCtx { device: &self.device, queue: &self.queue, layout: &self.sprite_layout, sampler: &self.sprite_sampler, encoder: &mut encoder, belt: &mut self.belt, depth: depth_src, size: (pw, ph), camera, aspect, mirror: scene.mirror };
                    chain.out = pass.run(&mut cx, &t, &rt_tex, &rt_view, &depth3);
                }
                // TEST GLUE (`--perf-bench`): the post chain's stage marks (nothing is recorded without the bench)
                crate::prof::mark(&mut encoder, Stage::TemporalPass);
            }
            // ENHANCEMENT E37: FSR 3.1 over the float scene and its vectors / masks, into a float image of the window's size (a
            // command buffer of its own, run after the frame's encoder so far - the scene - and before the one that carries the
            // post chain); the post chain takes the result as the scene. A dispatch that fails switches the backend off for good
            // (this frame keeps the stale image)
            let mut fsr_ok = false;
            if fsr_on {
                let out_size = (width, height);
                if !self.up.fsr_out.as_ref().is_some_and(|o| o.size == out_size) {
                    let up = self.up.fsr.as_ref().expect("fsr backend");
                    let img = RtImage::new(&self.device, "fsr output", &self.sprite_layout, &self.sprite_sampler, width, height, up.output_format(), up.output_usage() | wgpu::TextureUsages::TEXTURE_BINDING);
                    crate::upscale::mark_written(&self.device, &self.queue, &img.tex);
                    self.up.fsr_out = Some(img);
                }
                let RtImage { tex: out_tex, view: out_view, bg: out_bg, .. } = self.up.fsr_out.as_ref().expect("fsr output");
                match (self.temporal_inputs.as_ref(), self.up.fsr.as_mut()) {
                    (Some(inp), Some(up)) => {
                        // ENHANCEMENT ENH-UPSCALER: FSR's RCAS strength follows E39 (MetalFX: the RCAS pass below, UPSCALE-SOFT)
                        up.set_sharpness(crate::enhancements::fsr_sharpness(self.enhancements.level(crate::E39)));
                        match up.encode_temporal(&self.device, &self.queue, inp, out_tex) {
                            Some(cb) => {
                                let scene = std::mem::replace(&mut encoder, self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("post chain") }));
                                self.frame_cmds.close(scene);
                                self.frame_cmds.push(cb);
                                fsr_ok = true;
                                chain.out = Some((out_bg.clone(), out_view.clone()));
                            }
                            None => {
                                tracing::warn!("upscaler (ENHANCEMENT E37 / E34) failed, behaving as off from the next frame");
                                self.temporal_dead();
                            }
                        }
                    }
                    _ => self.temporal_dead(),
                }
                // ENHANCEMENT E39 (UPSCALE-SOFT): MetalFX's temporal scaler has no sharpening of its own: RCAS over its float output
                // (window size, before the post chain), at E39's strength. FSR's RCAS is inside the backend
                let mfx = self.up.fsr_kind.is_some_and(|k| k != crate::upscale::UpscalerKind::Fsr3);
                if let (true, true, Some(strength)) = (mfx, fsr_ok, crate::enhancements::fsr_sharpness(self.enhancements.level(crate::E39))) {
                    let format = self.up.fsr.as_ref().expect("temporal backend").output_format();
                    let sh = self.up.sharpen.get_or_insert_with(|| crate::sharpen::Sharpen::new(&self.device, &self.sprite_layout));
                    let src = &self.up.fsr_out.as_ref().expect("temporal output").bg;
                    chain.out = Some(sh.run_float(&self.device, &mut encoder, &self.sprite_layout, &self.sprite_sampler, src, (width, height), format, strength));
                }
            }
            // ENHANCEMENTS E27 / the depth of field (photo mode, E31) / E40 / E10 / E21: the post chain over the scene (`chain`), at the chain's size
            let plan = chain::PostPlan { eye, ..self.post_plan(scene, camera, aspect, hdr.is_some(), dbg_only, (pw, ph)) };
            {
                let mut cx = PostCtx { device: &self.device, queue: &self.queue, layout: &self.sprite_layout, sampler: &self.sprite_sampler, encoder: &mut encoder, belt: &mut self.belt, depth: depth_src, size: (pw, ph), camera, aspect, mirror: scene.mirror };
                self.post.run(&mut cx, plan, &mut chain, &rt_tex, &rt_view);
            }
            crate::prof::mark(&mut encoder, Stage::PostChain);
            let (img_bg, _) = chain.current();
            // ENHANCEMENT E33: the tone map's second output, at the size of the image the tone map writes (the internal one with E34)
            let gain_size = if self.up.upscale_active && self.up.upscaler.is_some() { (iw, ih) } else { (width, height) };
            let gain_view = if edr { self.edr.gain_view(&self.device, gain_size) } else { None };
            let gain = gain_view.as_ref();
            // the tone map's groups after the image's: ENHANCEMENT E21's eye, E10's bloom (the chain this frame made)
            let tone_eye = if eye { self.post.eye.as_ref().map(|e| e.tone_group()) } else { None };
            let tone_bloom = if bloom.is_some() { self.post.bloom.as_ref().and_then(|b| b.tone()).map(|t| t.1) } else { None };
            let tone_groups: Vec<&wgpu::BindGroup> = tone_eye.into_iter().chain(tone_bloom).collect();
            let tone_eye = &tone_groups[..];
            // the frame's command buffers in submit order: ENHANCEMENT E37: FSR's runs before the encoder that carries the post
            // chain over its output; ENHANCEMENT E34: the tone map's encoder and the upscaler's own run before the last
            let mut sprites_done = false;
            if self.up.upscale_active && self.up.upscaler.is_some() {
                // ENHANCEMENT E34: tone map into the internal-size 8-bit image, (E35: SMAA over it at the INTERNAL size),
                // the upscaler (a command buffer of its own, between the two wgpu encoders), then a plain blit of its
                // output over the window image
                let up = self.up.upscaler.as_mut().expect("upscaler");
                if !self.up.upscale_images.as_ref().is_some_and(|i| i.size == (iw, ih, width, height)) {
                    self.up.upscale_images = Some(crate::upscale::UpscaleImages::new(&self.device, &self.queue, &self.sprite_layout, &self.sprite_sampler, up.as_ref(), self.config.format, iw, ih, width, height));
                }
                let imgs = self.up.upscale_images.as_ref().expect("upscale images");
                if smaa {
                    let sm = self.smaa.get_or_insert_with(|| crate::smaa::Smaa::new(&self.device, &self.queue, self.config.format));
                    let ldr = sm.ldr(&self.device, iw, ih);
                    blit.draw(&mut encoder, &ldr, tone_eye, img_bg, gain);
                    sm.run(&mut encoder, &imgs.input_view);
                } else {
                    blit.draw(&mut encoder, &imgs.input_view, tone_eye, img_bg, gain);
                }
                crate::prof::mark(&mut encoder, Stage::ToneMap);
                let first = std::mem::replace(&mut encoder, self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upscale present") }));
                // the upscaler's own command buffer runs between `first` and this encoder: its time is this mark's
                crate::prof::mark(&mut encoder, Stage::Upscaler);
                self.frame_cmds.close(first);
                let scaled = up.encode(&self.device, &imgs.input, &imgs.output);
                let (present_bg, scaled_ok) = match scaled {
                    Some(cb) => {
                        self.frame_cmds.push(cb);
                        (&imgs.output_bg, true)
                    }
                    None => {
                        // the scaler failed: this frame's image is stretched bilinearly (E3's way)
                        tracing::warn!("upscaler (ENHANCEMENT E34) failed to encode, blitting bilinearly");
                        (&imgs.input_bg, false)
                    }
                };
                // ENHANCEMENT E39 (UPSCALE-SOFT): the spatial scaler's output is sharpened by RCAS on its way to the window image
                // (it takes the place of the present blit, a 1:1 copy); off (E39 = 0) or a failed scale = the plain blit
                match crate::enhancements::fsr_sharpness(self.enhancements.level(crate::E39)).filter(|_| scaled_ok) {
                    Some(strength) => {
                        let sh = self.up.sharpen.get_or_insert_with(|| crate::sharpen::Sharpen::new(&self.device, &self.sprite_layout));
                        sh.run(&self.device, &mut encoder, present_bg, &dest, self.config.format, false, strength);
                    }
                    None => {
                        let present = self.up.upscale_present.get_or_insert_with(|| ScaleBlit::new(&self.device, self.config.format, &self.sprite_layout, ToneKey::PLAIN, None, None));
                        present.draw(&mut encoder, &dest, &[], Some(present_bg), None);
                    }
                }
                crate::prof::mark(&mut encoder, Stage::UpscalePresent);
            } else if smaa {
                // ENHANCEMENT E35: the tone map writes the window-sized image SMAA reads; SMAA writes the destination
                let sm = self.smaa.get_or_insert_with(|| crate::smaa::Smaa::new(&self.device, &self.queue, self.config.format));
                let ldr = sm.ldr(&self.device, width, height);
                blit.draw(&mut encoder, &ldr, tone_eye, img_bg, gain);
                sm.run(&mut encoder, &dest);
                crate::prof::mark(&mut encoder, Stage::ToneMap);
            } else if n > 0 && gain.is_none() && !scene.sprites.iter().take(n).any(|s| s.depth.is_some()) {
                // PERF (picture identical): the plain path draws the 2D in the tone map's own pass, after its triangle (the
                // sprite pass would load the image the tone map just stored); only without E33's gain map (two targets, a
                // copy of the frame between) and with no depth-state sprite (the pass has no depth image)
                sprites_done = blit.draw_then(&mut encoder, &dest, tone_eye, img_bg, gain, |pass| self.draw_sprites(pass, scene, n, false));
                crate::prof::mark(&mut encoder, Stage::ToneMap);
            } else {
                blit.draw(&mut encoder, &dest, tone_eye, img_bg, gain);
                crate::prof::mark(&mut encoder, Stage::ToneMap);
            }
            // ENHANCEMENT E33: the native-size frame (after SMAA / the upscale and its blit) before the 2D, to tell the
            // pixels the HUD / sprites touch from the 3D's own
            if gain_view.is_some() {
                self.edr.copy_pre_2d(&mut encoder, &dest_tex, width, height);
            }
            if !sprites_done {
                self.sprite_pass(&mut encoder, scene, n, &dest, depth);
            }
            crate::prof::mark(&mut encoder, Stage::Sprites);
            self.frame_cmds.park(encoder);
            self.scale_blit = Some(blit);
        }
        if let Some(p) = pass {
            let mut encoder = self.frame_cmds.take(&self.device);
            p.resolve(&mut self.belt, &mut encoder, target, self.gamma);
            self.frame_cmds.park(encoder);
            self.gamma_pass = Some(p);
        }
    }

    /// ENHANCEMENT E33: one frame through the EDR chain (see `edr.rs`). The frame is encoded exactly as for the 8-bit
    /// surface, into the EDR module's own 8-bit image, with the gamma ramp held back (the present pass applies it
    /// after the 2D, so that the "pixel untouched by the 2D" test compares like with like).
    /// `size` / `aspect` / `depth`: the window frame's.
    #[cfg(target_os = "macos")]
    pub(super) fn render_edr(&mut self, scene: &Scene, camera: &Camera, surface_view: &wgpu::TextureView, size: (u32, u32), aspect: f32, depth: &wgpu::TextureView) {
        let (w, h) = size;
        let (tex, view, ..) = self.edr.targets(&self.device, self.config.format, (w, h));
        let gamma = self.gamma;
        self.gamma = 1.0;
        self.edr.pass = true;
        self.edr.gain_used = false;
        // the present pass follows the frame in its encoder: in its last submit
        self.whole_frame(|core| {
            core.encode_with_gamma_inner(scene, camera, FrameOut { view: &view, tex: &tex, depth, can_copy: true, width: w, height: h, aspect });
            core.edr.pass = false;
            core.gamma = gamma;
            let mut encoder = core.frame_cmds.take(&core.device);
            core.edr.present(&core.device, &mut core.belt, &mut encoder, surface_view, gamma);
            core.frame_cmds.park(encoder);
        });
        // diagnostic: `NFS_EDR_PROBE=<prefix>` in a windowed run measures frame 20 (the E34 / E35 paths do not run in a still)
        self.edr_frames += 1;
        if self.edr_frames == 20
            && let Some(prefix) = crate::dbg::flags().edr_probe.as_deref()
        {
            let pre2d = self.edr.pre2d();
            self.edr_probe_stats(&tex, &pre2d, gamma, true, prefix);
        }
    }

}
