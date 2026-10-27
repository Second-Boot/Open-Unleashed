//! Reading the GPU back: the lens flare's probe reads (ENHANCEMENT E7's half-float decode), the offscreen renders, stills and
//! images (`render_offscreen`, `render_to_image`, `render_still`). Moved out of gpu.rs as it was;
//! the macOS EDR probe diagnostics (`NFS_EDR_PROBE`) are in `diag.rs`.

use crate::gfx;
use super::*;

impl Core {
    /// Let the surface be copied from, for the lens flare's frame read-back. Off (the default) the
    /// surface is a render attachment alone.
    pub fn set_lens_enabled(&mut self, on: bool) {
        if self.lens_enabled == on {
            return;
        }
        self.lens_enabled = on;
        self.config.usage = if on { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::RENDER_ATTACHMENT };
        self.configure_surface();
        if !on {
            self.lens_pending.clear();
        }
    }

    /// Collect the lens-flare read-backs that have finished (their copies ran and their buffers are
    /// mapped) into [`Self::lens_done`]; the cost of each is what this takes, in whole milliseconds.
    pub(super) fn collect_lens(&mut self) {
        if self.lens_pending.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut keep = Vec::new();
        for p in std::mem::take(&mut self.lens_pending) {
            match p.rx.try_recv() {
                Ok(Ok(())) => {
                    let mut pixels = [0u32; 16];
                    {
                        let data = p.buf.slice(..).get_mapped_range().expect("mapped range");
                        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
                        for (row, chunk) in pixels.chunks_exact_mut(4).enumerate() {
                            for (i, px) in chunk.iter_mut().enumerate() {
                                if p.half {
                                    // ENHANCEMENT E7: the float image's texel as the 8-bit surface would store it
                                    let o = row * LENS_ROW as usize + i * 8;
                                    let ch = |k: usize| (f16_to_f32(u16::from_le_bytes([data[o + 2 * k], data[o + 2 * k + 1]])).clamp(0.0, 1.0) * 255.0).round() as u8;
                                    let (r, g, b, a) = (ch(0), ch(1), ch(2), ch(3));
                                    *px = u32::from_le_bytes(if bgra { [b, g, r, a] } else { [r, g, b, a] });
                                } else {
                                    let o = row * LENS_ROW as usize + i * 4;
                                    *px = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                                }
                            }
                        }
                    }
                    p.buf.unmap();
                    self.lens_done.push(LensRead { reference: p.reference, pixels, cost_ms: t0.elapsed().as_millis() as u32 });
                }
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => keep.push(p),
            }
        }
        // a read-back that never completes does not pile up
        let n = keep.len();
        if n > 8 {
            keep.drain(..n - 8);
        }
        self.lens_pending = keep;
    }

    /// The finished read-backs, taken.
    pub(crate) fn take_lens(&mut self) -> Vec<LensRead> {
        std::mem::take(&mut self.lens_done)
    }

    /// PORT-DEVIATION D9 (PORT-DEVIATIONS.md) [0x460460 / dx7z 0x60003990, a blocking lock in the frame]:
    /// Copy the 4 × 4 block at the probe's pixel from `target` into a fresh staging buffer and map
    /// it; [`Self::collect_lens`] picks it up when the copy has run. The block's corner is clamped as
    /// `nfs5.dll`'s 0x10003320 does for every `THRASH_readrect` (the call at 0x4603FF / 0x460525 is
    /// routed through it): not below 0, and `corner + 4` below the width / height — else `W − 5`
    /// / `H − 5` (`dx7z.dll` 0x60003990 itself does no clipping).
    pub(super) fn lens_copy(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Texture, probe: LensProbe, reference: bool) {
        let (w, h) = (target.width() as i32, target.height() as i32);
        if w < 5 || h < 5 {
            return;
        }
        let (px, py) = ((probe.x / 640.0 * w as f32).round_ties_even() as i32 - 2, (probe.y / 480.0 * h as f32).round_ties_even() as i32 - 2);
        // 0x10003320: `max(v, 0)`, then `v + 4 >= size` → `size − 5`
        let clamp = |v: i32, size: i32| {
            let v = v.max(0);
            if v + 4 >= size { size - 5 } else { v }
        };
        let (x, y) = (clamp(px, w), clamp(py, h));
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("lens read-back"), size: LENS_ROW * 4, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d { x: x as u32, y: y as u32, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(LENS_ROW as u32), rows_per_image: Some(4) } },
            wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        );
        // mapped after the frame's submit ([`Self::lens_map_staged`])
        self.lens_staged.push((buf, reference, target.format() == HDR_FORMAT));
    }

    /// Ask for the mapping of every block copied this frame; call after the submit.
    pub(super) fn lens_map_staged(&mut self) {
        for (buf, reference, half) in std::mem::take(&mut self.lens_staged) {
            let (tx, rx) = std::sync::mpsc::channel();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.lens_pending.push(LensPending { buf, reference, half, rx });
        }
    }

    /// The depth target for a pass of this size: one per size, kept (every pass clears it first,
    /// so sharing it between offscreen passes is the same as a fresh texture each).
    pub(super) fn depth_for(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let device = &self.device;
        self.depth_cache.entry((width, height)).or_insert_with(|| Self::make_depth_sized(device, width, height)).clone()
    }

    /// Render one frame into an offscreen RGBA8 image (for screenshots and automated checks).
    /// Render a scene's meshes into an offscreen texture that the sprite pass can then draw, so a
    /// 3D view can sit **inside** the 2D screen in the right drawing order — what the front end's
    /// `NFS5.3DCAR` widgets need (`RE/notes/frontend-3d.md` §4). The clear is transparent, so only
    /// the geometry shows, unless `clear` gives an opaque colour (the in-race rear-view mirror).
    /// Passing the id of a texture this returned earlier reuses it.
    /// `depth_far` is the far plane of the pass's own screen z — the scale of the state 0x18 depth
    /// bias (`encode_frame`): `None` for the main view's 700, the rear view's 65000 (0x4691D0).
    pub fn render_offscreen(&mut self, scene: &Scene, camera: &Camera, spec: OffscreenSpec) -> TextureId {
        let OffscreenSpec { width, height, reuse, clear_colour, depth_far } = spec;
        crate::prof::ctx(" [mirror/offscreen]");
        let (width, height) = (width.max(1), height.max(1));
        let id = match reuse.filter(|t| self.offscreen.get(&t.0).is_some_and(|(w, h)| *w == width && *h == height)) {
            Some(t) => t,
            None => {
                let texture = gfx::texture_2d(&self.device, "offscreen 3d", width, height, self.config.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
                let view = texture.create_view(&Default::default());
                let sprite_bg = Some(gfx::tex_sampler_bg(&self.device, "offscreen tex", &self.sprite_layout, &view, &self.sprite_sampler));
                self.textures.push(GpuTexture { _texture: texture, view, sprite_bg, generated: None, size: (width, height) });
                let id = TextureId(self.textures.len() as u32 - 1);
                self.offscreen.insert(id.0, (width, height));
                id
            }
        };
        let depth = self.depth_for(width, height);
        let view = self.textures[id.0 as usize].view.clone();
        // the scene's items and lights as they are, its own clear colour replaced and no sprites
        let target = self.textures[id.0 as usize]._texture.clone();
        // ENHANCEMENT E7: the rear-view mirror (the pass with an opaque clear colour — a front-end car
        // view is a part of the 2D screen and stays as it was) is drawn into a float image and
        // tone-mapped like the main view, but only for the non-neutral tone map: "as the original"
        // is the identity, so the mirror keeps its 8-bit path
        // ENHANCEMENT ENH-FE3D: ... and a front-end car view too, at the contrasty level (the car and the room behind it share one curve)
        let hdr = self.enh_hdr(scene).filter(|&m| m != 0);
        let aspect = width as f32 / height as f32;
        let far = depth_far.unwrap_or(VIEW_FAR);
        let clear = clear_colour.unwrap_or([0.0, 0.0, 0.0, 0.0]);
        if let Some(mode) = hdr {
            let mut blit = match self.scale_blit_mirror.take() {
                Some(b) if b.hdr == hdr => b,
                _ => ScaleBlit::new(&self.device, self.config.format, &self.sprite_layout, Some(mode), false, None, false),
            };
            let (rt_tex, rt_view) = blit.target(&self.device, &self.sprite_layout, &self.sprite_sampler, width, height);
            let ms = self.ms_targets(width, height, self.enh_samples(true), true);
            self.cur_hdr = hdr;
            self.encode_frame(scene, clear, false, camera, &rt_view, &rt_tex, false, &depth, aspect, far, ms.as_ref());
            self.cur_hdr = None;
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("mirror tone map") });
            blit.draw(&mut encoder, &view, None, None, None);
            crate::prof::resolve(&mut encoder);
        self.queue.submit(Some(encoder.finish()));
            self.scale_blit_mirror = Some(blit);
            crate::prof::ctx("");
            return id;
        }
        let ms = self.ms_targets(width, height, self.enh_samples(false), false);
        self.encode_frame(scene, clear, false, camera, &view, &target, false, &depth, aspect, far, ms.as_ref());
        crate::prof::ctx("");
        id
    }

    pub fn render_to_image(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
        let target = gfx::texture_2d(&self.device, "offscreen", width, height, self.config.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&Default::default());
        let depth = self.depth_for(width, height);
        // `NFS_GPU_TIME`: the same frame again 60 times, so a still has a timing (diagnostic only)
        for _ in 0..if self.gpu_time_log { 60 } else { 0 } {
            self.encode_with_gamma(scene, camera, &view, &target, true, &depth, width, height, width as f32 / height as f32);
        }
        self.encode_with_gamma(scene, camera, &view, &target, true, &depth, width, height, width as f32 / height as f32);
        // ENHANCEMENT E33 diagnostic (`NFS_EDR_PROBE=<path prefix>`): the same frame through the EDR chain, read back
        #[cfg(target_os = "macos")]
        if let Some(prefix) = crate::dbg::flags().edr_probe.as_deref() {
            self.edr_probe(scene, camera, width, height, prefix);
        }
        let px = self.readback(&target, width, height);
        // the read-back above waited for the device, so the frame's lens blocks are mapped
        self.collect_lens();
        px
    }

    /// PORT-DEVIATION D46 (photo mode): a still of the scene at `width x height` — the window's size times 1, 2 or 4 —
    /// as RGBA8 rows, with its real size (the request is scaled down to the device's texture size, its buffer limit
    /// and [`STILL_MAX_PIXELS`]: `still_size`, master's). ENHANCEMENTS on top of master's still: the whole post chain of the frame runs at that size (the bloom, the depth of field,
    /// the tone map, the dither), so the still is a bigger frame and not a bigger copy of one; the window's render
    /// scale (E3) is off for it (the size already is the scale), and the MSAA steps down with the pixel count
    /// (4x above 16 MP -> 2x, above 32 MP none) to keep the float image and its samples in memory. The switches are
    /// put back at once, the sized depth image is dropped, and the next window frame rebuilds its own targets.
    pub fn render_still(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Result<(u32, u32, Vec<u8>)> {
        // PORT-DEVIATION D46: the size, as master's (`still_size`)
        let lim = self.device.limits();
        let (w, h) = super::still_size(width, height, lim.max_texture_dimension_2d, lim.max_buffer_size);
        let saved = self.enhancements;
        let mut e = saved;
        e.set_level(crate::E3, 0);
        // ENHANCEMENT E34: the still is rendered at its own size
        #[cfg(target_os = "macos")]
        e.set_level(crate::E34, 0);
        // ENHANCEMENT E37: ... and with no temporal history (a still is one frame): no FSR, the still's own AA
        e.set_level(crate::E37, 0);
        let px = w as u64 * h as u64;
        if px > 32_000_000 {
            e.set_level(crate::E1, 0);
        } else if px > 16_000_000 && e.level(crate::E1) > 1 {
            e.set_level(crate::E1, 1);
        }
        self.enhancements = e;
        let r = self.render_to_image(scene, camera, w, h);
        self.enhancements = saved;
        self.depth_cache.remove(&(w, h));
        r.map(|px| (w, h, px))
    }

    /// Copies a `COPY_SRC` texture to the host as tightly packed RGBA8.
    pub(super) fn readback(&self, target: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<u8>> {
        let bytes_per_row = (4 * width).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (bytes_per_row * height) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let data = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        for row in 0..height {
            let r = &data[(row * bytes_per_row) as usize..(row * bytes_per_row + width * 4) as usize];
            if bgra {
                for px in r.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                out.extend_from_slice(r);
            }
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}
