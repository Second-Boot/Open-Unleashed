//! ENHANCEMENT E8 / E16 / ENH-COAT: the reflection cube — its faces drawn or uploaded, its prefiltered mips, the car-ambient
//! probe's strength. Moved out of gpu.rs as it was.

use crate::gfx;
use super::*;

impl Core {
    /// ENHANCEMENT E8: draw one face of the player's car's environment — `scene` seen through `camera`
    /// (the car's position, the face's direction, `REFL_TAN_HALF`) into a `size`² target, then copy it
    /// into layer `face` of the array. The array is made (or made again when `size` changes) here; the
    /// map is used once all six faces have been drawn. A pass of its own, submitted in call order, so
    /// the main frame that follows samples the finished face. MSAA is not used (1 sample, `size`² is
    /// small and the faces are filtered when read).
    pub(crate) fn render_refl_face(&mut self, scene: &Scene, camera: &Camera, face: u32, size: u32, clear: [f64; 4], depth_far: f32) {
        let t0 = self.gpu_time_log.then(std::time::Instant::now);
        let size = size.clamp(16, 1024);
        let face = face.min(5);
        // ENHANCEMENT ENH-COAT: the faces are drawn in the float scene's format when the main view is float (the clear coat
        // reflects the sun above white); `cur_hdr` makes `encode_frame` build for it, like the mirror's pass
        let float = self.enh_hdr(scene);
        self.ensure_refl_cube(size, float.is_some());
        let (tex, view) = {
            let c = self.refl.as_ref().expect("refl cube");
            (c.face.clone(), c.face_view.clone())
        };
        let depth = self.depth_for(size, size);
        let was_mirror = std::mem::replace(&mut self.no_reflections, false);
        crate::prof::ctx(" [env face]");
        self.cur_hdr = float;
        self.refl_building = true;
        self.encode_frame(scene, clear, false, camera, &view, &tex, false, &depth, 1.0, depth_far, None);
        self.refl_building = false;
        self.cur_hdr = None;
        crate::prof::ctx("");
        self.no_reflections = was_mirror;
        let c = self.refl.as_mut().expect("refl cube");
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("refl face copy") });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: &c.array, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: face }, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
        c.faces_done |= 1 << face;
        c.stale |= 1 << face;
        // ENHANCEMENT E16: project the faces into the car ambient's SH (after every face update)
        if self.enhancements.is_on(crate::E16) && self.enhancements.is_on(crate::E8) && self.compute_supported && c.faces_done == 0x3F {
            let built = self.probe.is_some();
            let p = self.probe.get_or_insert_with(|| crate::probe::Probe::new(&self.device));
            let (array_view, size) = (c.base_view.clone(), c.size);
            p.update(&self.device, &self.queue, &mut encoder, &array_view, size, 1.0 / REFL_TAN_HALF);
            if !built {
                self.rebuild_globals_bg();
            }
        }
        crate::prof::resolve(&mut encoder);
        self.queue.submit(Some(encoder.finish()));
        // `NFS_GPU_TIME`: a face's cost is added to the main frame's average (steady state: one frame = main + its faces)
        if let Some(t0) = t0 {
            let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
            self.gpu_time_acc.0 += t0.elapsed().as_secs_f64() * 1000.0;
        }
    }

    /// ENHANCEMENT E8: make the environment cube `size`² (made again when the size or the format changed; the probe starts
    /// over). ENHANCEMENT ENH-COAT: `float` = `Rgba16Float` texels (the float scene), and a chain of [`crate::coat::MIPS`] mips
    /// the clear coat's roughness picks from.
    pub(super) fn ensure_refl_cube(&mut self, size: u32, float: bool) {
        let format = if float { HDR_FORMAT } else { self.config.format };
        if self.refl.as_ref().is_none_or(|c| c.size != size || c.format != format) {
            let array = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("refl faces"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
                mip_level_count: crate::coat::MIPS,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let array_view = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            let base_view = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
            let tex = gfx::texture_2d(&self.device, "refl face", size, size, format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
            let face_view = tex.create_view(&Default::default());
            self.refl = Some(ReflCube { size, format, array, array_view, base_view, stale: 0, face: tex, face_view, faces_done: 0 });
            if self.coat.is_none() {
                self.coat = Some(crate::coat::CoatFilter::new(&self.device));
            }
            if let Some(p) = self.probe.as_mut() {
                p.reset();
            }
            self.rebuild_globals_bg();
        }
    }

    /// ENHANCEMENT ENH-COAT: the prefiltered mips of the faces drawn since the last run, once all six are in (the filter of
    /// one face reads its neighbours' mip 0). Its own submit, before the frame that reads the mips.
    pub(super) fn refl_prefilter(&mut self) {
        let (Some(c), Some(coat)) = (self.refl.as_mut(), self.coat.as_mut()) else { return };
        if c.stale == 0 || c.faces_done != 0x3F {
            return;
        }
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("coat prefilter") });
        coat.record(&self.device, &self.queue, &mut encoder, &c.array, c.format, c.size, c.stale, 1.0 / REFL_TAN_HALF);
        c.stale = 0;
        self.queue.submit(Some(encoder.finish()));
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
        self.ensure_refl_cube(size, float);
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
                data.extend(px.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
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
        c.faces_done = 0x3F;
        c.stale = 0x3F;
        if self.enhancements.is_on(crate::E16) && self.enhancements.is_on(crate::E8) && self.compute_supported {
            let built = self.probe.is_some();
            let p = self.probe.get_or_insert_with(|| crate::probe::Probe::new(&self.device));
            let (array_view, size) = (c.base_view.clone(), c.size);
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("studio probe") });
            p.update(&self.device, &self.queue, &mut encoder, &array_view, size, 1.0 / REFL_TAN_HALF);
            self.queue.submit(Some(encoder.finish()));
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
        if !ready { 0.0 } else if self.enhancements.level(crate::E16) >= 2 { crate::probe::STRENGTH_STRONG } else { crate::probe::STRENGTH }
    }
}
