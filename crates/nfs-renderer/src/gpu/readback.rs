//! Reading the GPU back: the lens flare's probe reads (ENHANCEMENT E7's half-float decode), the offscreen renders, stills and
//! images (`render_offscreen`, `render_to_image`, `render_still`) and the size a still gets on a device (`still_size`);
//! the macOS EDR probe diagnostics (`NFS_EDR_PROBE`) are in `diag.rs`.

use crate::gfx;
use super::*;

impl Core {
    /// Let the surface be copied from, for the lens flare's frame read-back. Off (the default) the
    /// surface is a render attachment alone.
    pub fn set_lens_enabled(&mut self, on: bool) {
        if self.lens.enabled == on {
            return;
        }
        self.lens.enabled = on;
        self.config.usage = if on { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::RENDER_ATTACHMENT };
        self.configure_surface();
        if !on {
            self.lens.pending.clear();
        }
    }

    /// Collect the lens-flare read-backs that have finished (their copies ran and their buffers are
    /// mapped) into [`Self::lens_done`]; the cost of each is what this takes, in whole milliseconds.
    pub(super) fn collect_lens(&mut self) {
        if self.lens.pending.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        let _ = self.device.poll(wgpu::PollType::Poll);
        // (in place: the pending list keeps its storage)
        let mut i = 0;
        while i < self.lens.pending.len() {
            match self.lens.pending[i].rx.try_recv() {
                Ok(Ok(())) => {
                    let p = self.lens.pending.remove(i);
                    let mut pixels = [0u32; 16];
                    {
                        let data = p.buf.slice(..).get_mapped_range().expect("mapped range");
                        let bgra = is_bgra(self.config.format);
                        for (row, chunk) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
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
                    if self.lens.free.len() < LENS_POOL {
                        self.lens.free.push(p.buf);
                    }
                    self.lens.done.push(LensRead { reference: p.reference, pixels, cost_ms: t0.elapsed().as_millis() as u32 });
                }
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.lens.pending.remove(i);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
            }
        }
        // a read-back that never completes does not pile up
        let n = self.lens.pending.len();
        if n > LENS_PENDING_MAX {
            self.lens.pending.drain(..n - LENS_PENDING_MAX);
        }
    }

    /// The finished read-backs, taken.
    pub(crate) fn take_lens(&mut self) -> Vec<LensRead> {
        std::mem::take(&mut self.lens.done)
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
        let (px, py) = ((probe.x / crate::LENS_SCREEN[0] * w as f32).round_ties_even() as i32 - 2, (probe.y / crate::LENS_SCREEN[1] * h as f32).round_ties_even() as i32 - 2);
        // 0x10003320: `max(v, 0)`, then `v + 4 >= size` → `size − 5`
        let clamp = |v: i32, size: i32| {
            let v = v.max(0);
            if v + 4 >= size { size - 5 } else { v }
        };
        let (x, y) = (clamp(px, w), clamp(py, h));
        let buf = self.lens.free.pop().unwrap_or_else(|| self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("lens read-back"), size: LENS_ROW * 4, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false }));
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d { x: x as u32, y: y as u32, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(LENS_ROW as u32), rows_per_image: Some(4) } },
            kit::extent(4, 4),
        );
        // mapped after the frame's submit ([`Self::lens_map_staged`])
        self.lens.staged.push((buf, reference, target.format() == HDR_FORMAT));
    }

    /// Ask for the mapping of every block copied this frame; call after the submit.
    pub(super) fn lens_map_staged(&mut self) {
        for (buf, reference, half) in std::mem::take(&mut self.lens.staged) {
            let (tx, rx) = std::sync::mpsc::channel();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.lens.pending.push(LensPending { buf, reference, half, rx });
        }
    }

    /// The depth target for a pass of this size: one per size, kept (every pass clears it first,
    /// so sharing it between offscreen passes is the same as a fresh texture each); at most [`TARGET_CACHE_SIZES`] sizes are
    /// kept (a window being dragged asks for a new size every frame).
    pub(super) fn depth_for(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        if self.depth_cache.len() > TARGET_CACHE_SIZES && !self.depth_cache.contains_key(&(width, height)) {
            self.depth_cache.clear();
        }
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
                let sprite_bg = Some(gfx::tex_sampler_bg(&self.device, "offscreen tex", &self.layouts.sprite, &view, &self.samplers.sprite));
                self.res.textures.push(GpuTexture { _texture: texture, view, sprite_bg, generated: None, size: (width, height), min_alpha: 0 });
                let id = TextureId(self.res.textures.len() as u32 - 1);
                self.offscreen.insert(id.0, (width, height));
                id
            }
        };
        let depth = self.depth_for(width, height);
        let view = self.res.textures[id.0 as usize].view.clone();
        // the scene's items and lights as they are, its own clear colour replaced and no sprites
        let target = self.res.textures[id.0 as usize]._texture.clone();
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
                Some(b) if b.key == ToneKey::float(mode) => b,
                _ => ScaleBlit::new(&self.device, self.config.format, &self.layouts.sprite, ToneKey::float(mode), None, None),
            };
            let (rt_tex, rt_view) = blit.target(&self.device, &self.layouts.sprite, &self.samplers.sprite, width, height);
            let ms = self.ms_targets(width, height, self.enh_samples(true), true);
            self.cur_hdr = hdr;
            self.encode_frame(FrameDesc { scene, clear, with_sprites: false, camera, view: &rt_view, target: &rt_tex, can_copy: false, depth: &depth, aspect, depth_far: far, ms: ms.as_ref() });
            self.cur_hdr = None;
            // the mirror's tone map follows its pass in the frame's encoder
            let mut encoder = self.frame_cmds.take(&self.device);
            blit.draw(&mut encoder, &view, &[], None, None);
            self.frame_cmds.park(encoder);
            self.scale_blit_mirror = Some(blit);
            crate::prof::ctx("");
            return id;
        }
        let ms = self.ms_targets(width, height, self.enh_samples(false), false);
        self.encode_frame(FrameDesc { scene, clear, with_sprites: false, camera, view: &view, target: &target, can_copy: false, depth: &depth, aspect, depth_far: far, ms: ms.as_ref() });
        crate::prof::ctx("");
        id
    }

    pub fn render_to_image(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
        let target = gfx::texture_2d(&self.device, "offscreen", width, height, self.config.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let view = target.create_view(&Default::default());
        let depth = self.depth_for(width, height);
        // `NFS_GPU_TIME`: the same frame again 60 times, so a still has a timing (diagnostic only)
        for _ in 0..if self.diag.gpu_time.on() { 60 } else { 0 } {
            self.encode_with_gamma(scene, camera, FrameOut::of(&view, &target, &depth));
        }
        self.encode_with_gamma(scene, camera, FrameOut::of(&view, &target, &depth));
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

    /// TEST GLUE (`--perf-bench`): start the bench — the stage marks on (`false`: the device has no timestamp queries,
    /// only the CPU is timed).
    pub fn bench_begin(&mut self) -> bool {
        crate::prof::marks_begin(&self.device, &self.queue)
    }

    /// TEST GLUE (`--perf-bench`): the bench is over (the marks off, its target dropped).
    pub fn bench_end(&mut self) {
        crate::prof::marks_end();
        self.diag.bench_target = None;
    }

    /// TEST GLUE (`--perf-bench`): a bench frame's start (before the mirror and the environment faces of the frame).
    pub fn bench_frame_start(&mut self) -> std::time::Instant {
        crate::prof::bench_frame_begin()
    }

    /// TEST GLUE (`--perf-bench`): the main view of a bench frame, drawn as [`Self::render_to_image`] draws it (one
    /// `encode_with_gamma` into a `width x height` image of the surface's format) but into a target kept between frames
    /// and with no read-back.
    pub fn bench_frame(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32) {
        if !self.diag.bench_target.as_ref().is_some_and(|t| t.0 == width && t.1 == height) {
            let target = gfx::texture_2d(&self.device, "bench target", width, height, self.config.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
            let view = target.create_view(&Default::default());
            self.diag.bench_target = Some((width, height, target, view));
        }
        let (_, _, target, view) = self.diag.bench_target.clone().expect("bench target");
        let depth = self.depth_for(width, height);
        self.encode_with_gamma(scene, camera, FrameOut::of(&view, &target, &depth));
    }

    /// TEST GLUE (`--perf-bench`): the throughput phase's frame end: no wait for this frame, only for the one `depth` frames
    /// back (`inflight` holds the frames' last submits), as a swap chain of `depth` images lets the CPU run ahead.
    pub fn bench_frame_pipelined(&mut self, inflight: &mut std::collections::VecDeque<wgpu::SubmissionIndex>, depth: usize) {
        if let Some(s) = self.last_submit.clone() {
            inflight.push_back(s);
        }
        while inflight.len() > depth {
            let s = inflight.pop_front().expect("in flight");
            let _ = self.device.poll(wgpu::PollType::Wait { submission_index: Some(s), timeout: None });
            self.collect_lens();
        }
    }

    /// TEST GLUE (`--perf-bench`): wait for every frame of the throughput phase.
    pub fn bench_wait_idle(&mut self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        self.collect_lens();
    }

    /// TEST GLUE (`--perf-bench`): a bench frame's end — the CPU time, the wait for the device, the GPU stages read back.
    pub fn bench_frame_finish(&mut self, t0: std::time::Instant) -> crate::prof::BenchSample {
        let s = crate::prof::bench_frame_end(&self.device, &self.queue, t0);
        // the device was waited for, so the frame's lens blocks are mapped (as after `render_to_image`'s read-back)
        self.collect_lens();
        s
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
        } else if px > 16_000_000 && e.level(crate::E1) > crate::Enhancements::E1_2X {
            e.set_level(crate::E1, crate::Enhancements::E1_2X);
        }
        self.enhancements = e;
        let r = self.render_to_image(scene, camera, w, h);
        self.enhancements = saved;
        self.depth_cache.remove(&(w, h));
        r.map(|px| (w, h, px))
    }

    /// Copies a `COPY_SRC` texture to the host as tightly packed RGBA8.
    pub(super) fn readback(&self, target: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<u8>> {
        let mut px = self.read_texture(target, width, height, 4)?;
        if is_bgra(self.config.format) {
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
        }
        Ok(px)
    }

    /// The `width x height` texels of a `COPY_SRC` texture of `bytes_per_px` bytes a texel, tightly packed rows (the device
    /// waited for).
    pub(super) fn read_texture(&self, target: &wgpu::Texture, width: u32, height: u32, bytes_per_px: u32) -> Result<Vec<u8>> {
        debug_assert!(self.frame_cmds.is_empty(), "a read-back after the frame's submit");
        let row = bytes_per_px * width;
        let bytes_per_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (bytes_per_row * height) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            super::kit::level_copy(target, 0),
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) } },
            super::kit::extent(width, height),
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
        let mut out = Vec::with_capacity((row * height) as usize);
        for r in data.chunks(bytes_per_row as usize).take(height as usize) {
            out.extend_from_slice(&r[..row as usize]);
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}

/// PORT-DEVIATION D46 (photo mode): the most pixels a still has (an 8-bit target, its depth and the read-back buffer of
/// 40 MP are ~0.5 GB; ENHANCEMENT E7's float image and its post chain at that size fit a 6 GB device).
pub const STILL_MAX_PIXELS: u64 = 40_000_000;

/// PORT-DEVIATION D46: the size a still of `width x height` gets on a device whose largest 2D texture is `max_dim` and
/// largest buffer `max_buffer` bytes — the request itself when it fits, else scaled down with its aspect kept.
pub fn still_size(width: u32, height: u32, max_dim: u32, max_buffer: u64) -> (u32, u32) {
    let (w, h) = (width.max(1), height.max(1));
    // the read-back buffer's rows are padded to 256 bytes: keep a margin under the limit
    let max_px = (STILL_MAX_PIXELS as f64).min(max_buffer as f64 / 4.0 * 0.85);
    let k = (max_dim as f64 / w.max(h) as f64).min((max_px / (w as f64 * h as f64)).sqrt()).min(1.0);
    if k < 1.0 { (((w as f64 * k).floor() as u32).max(1), ((h as f64 * k).floor() as u32).max(1)) } else { (w, h) }
}

#[cfg(test)]
mod still_tests {
    use super::{still_size, STILL_MAX_PIXELS};

    #[test]
    fn a_still_fits_the_device() {
        // a 1x / 2x still of a 1280 x 800 window fits as asked
        assert_eq!(still_size(1280, 800, 8192, 1 << 28), (1280, 800));
        assert_eq!(still_size(2560, 1600, 8192, 1 << 28), (2560, 1600));
        // 4x of 1920 x 1080 fits the 8192 side and the pixel cap
        assert_eq!(still_size(7680, 4320, 8192, 1 << 28), (7680, 4320));
        // 4x of 2560 x 1600 is past the 8192 texture side and the 40 MP cap: scaled down, the aspect kept
        let (w, h) = still_size(10240, 6400, 8192, 1 << 28);
        assert!(w <= 8192 && w as u64 * h as u64 <= STILL_MAX_PIXELS && (w as f32 / h as f32 - 1.6).abs() < 0.01, "{w}x{h}");
        // the buffer limit: 256 MB at 4 bytes a pixel, less the margin
        let (w, h) = still_size(8000, 5000, 16384, 1 << 28);
        assert!((w as u64 * h as u64 * 4) as f64 <= (1u64 << 28) as f64 * 0.85 && (w as f32 / h as f32 - 1.6).abs() < 0.01, "{w}x{h}");
        // the pixel cap
        let (w, h) = still_size(16000, 10000, 16384, 1 << 34);
        assert!(w as u64 * h as u64 <= STILL_MAX_PIXELS, "{w}x{h}");
        assert_eq!(still_size(0, 0, 8192, 1 << 28), (1, 1));
    }
}
