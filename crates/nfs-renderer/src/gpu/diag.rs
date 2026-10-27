//! ENHANCEMENT E33 diagnostics, macOS only (the EDR output exists there alone): `NFS_EDR_PROBE=<path prefix>` draws a
//! frame through the EDR chain into an image of its own, reads the surface values back and logs / writes what they are
//! (`edr_probe`, `edr_probe_stats`, the half-float read-back `readback_f16`). Moved out of `readback.rs` as they were;
//! the module is `cfg(target_os = "macos")` instead of each function.

use crate::gfx;
use super::*;

impl Core {
    /// ENHANCEMENT E33 diagnostic: draw `scene` through the EDR chain (frame image -> present pass) into an
    /// `Rgba16Float` image of its own, read the values back and log what they are: the brightest value, how many pixels
    /// are above SDR white, the mean linear light against the plain 8-bit frame's, and that every pixel the 2D touched
    /// equals the plain sRGB decode of its byte (the HUD is exactly SDR). Writes `<prefix>_edr.ppm` (values to 1.0 as
    /// sRGB, values above 1.0 as an orange-to-white glow scaled to the cap) and `<prefix>_mask.ppm` (the 2D's pixels).
    pub(super) fn edr_probe(&mut self, scene: &Scene, camera: &Camera, w: u32, h: u32, prefix: &str) {
        let was_on = self.edr.on;
        let (tex, view, pre2d) = self.edr.targets(&self.device, self.config.format, (w, h));
        let depth = self.depth_for(w, h);
        let gamma = self.gamma;
        self.gamma = 1.0;
        self.edr.pass = true;
        self.edr.gain_used = false;
        self.encode_with_gamma(scene, camera, &view, &tex, true, &depth, w, h, w as f32 / h as f32);
        self.edr.pass = false;
        self.gamma = gamma;
        self.edr_probe_stats(&tex, &pre2d, w, h, gamma, was_on, prefix);
    }

    /// ENHANCEMENT E33 diagnostic, the second half of [`Self::edr_probe`]: the present pass of the frame already in
    /// `tex` (with `pre2d` its copy before the 2D) into an image of its own, read back and logged. Also called from
    /// the window path once (frame 20) under `NFS_EDR_PROBE`, so the MetalFX / SMAA paths can be measured too.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn edr_probe_stats(&mut self, tex: &wgpu::Texture, pre2d: &wgpu::Texture, w: u32, h: u32, gamma: f32, was_on: bool, prefix: &str) {
        let out = gfx::texture_2d(&self.device, "edr probe", w, h, HDR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let out_view = out.create_view(&Default::default());
        self.edr.present(&self.device, &self.queue, &out_view, gamma);
        let gain_used = self.edr.gain_used;
        let f = self.readback_f16(&out, w, h);
        let frame8 = self.readback(tex, w, h);
        let before = self.readback(pre2d, w, h);
        let (Ok(f), Ok(frame8), Ok(before)) = (f, frame8, before) else {
            tracing::warn!("E33 probe: read-back failed");
            return;
        };
        let dec = |b: u8| {
            let v = b as f32 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        let n = (w * h) as usize;
        let (mut max_v, mut above, mut sum_e, mut sum_s, mut hud, mut hud_bad, mut hud_max) = (0f32, 0usize, 0f64, 0f64, 0usize, 0usize, 0f32);
        let mut img = Vec::with_capacity(n * 3);
        let mut mask = Vec::with_capacity(n * 3);
        // `_heat.ppm`: the surface's max channel over its SDR decode's (red, 1..2 -> 0..255) and the max channel / 3
        // (green), linear and unclamped, so a step in the EDR lift shows as a step (`_edr`'s orange only marks `> 1`)
        let mut heat = Vec::with_capacity(n * 3);
        // `_hdr.ppm`: the surface values at half exposure (white = 0.5), so what is above white shows as a picture
        let mut hdr = Vec::with_capacity(n * 3);
        for i in 0..n {
            let e = [f[i * 4], f[i * 4 + 1], f[i * 4 + 2]];
            let m8 = [frame8[i * 4], frame8[i * 4 + 1], frame8[i * 4 + 2]];
            let touched = frame8[i * 4..i * 4 + 3] != before[i * 4..i * 4 + 3];
            let em = e[0].max(e[1]).max(e[2]);
            max_v = max_v.max(em);
            if em > 1.0005 {
                above += 1;
            }
            sum_e += (e[0] + e[1] + e[2]) as f64 / 3.0;
            sum_s += (dec(m8[0]) + dec(m8[1]) + dec(m8[2])) as f64 / 3.0;
            if touched {
                hud += 1;
                let d = (0..3).map(|c| (e[c] - dec(m8[c])).abs()).fold(0f32, f32::max);
                hud_max = hud_max.max(em);
                if d > 1.0 / 1024.0 {
                    hud_bad += 1;
                }
            }
            let enc = |v: f32| {
                let v = v.clamp(0.0, 1.0);
                (255.0 * if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }).round() as u8
            };
            if em > 1.0005 {
                let t = ((em - 1.0) / (crate::edr::CAP - 1.0)).clamp(0.0, 1.0);
                img.extend_from_slice(&[255, (140.0 + 115.0 * t) as u8, (30.0 + 225.0 * t) as u8]);
            } else {
                img.extend_from_slice(&[enc(e[0]), enc(e[1]), enc(e[2])]);
            }
            let sm = dec(m8[0]).max(dec(m8[1])).max(dec(m8[2]));
            hdr.extend_from_slice(&[enc(e[0] * 0.5), enc(e[1] * 0.5), enc(e[2] * 0.5)]);
            heat.extend_from_slice(&[(255.0 * (em / sm.max(0.02) - 1.0)).clamp(0.0, 255.0) as u8, (255.0 * em / 3.0).clamp(0.0, 255.0) as u8, 0]);
            let g = if touched { 255 } else { 0 };
            mask.extend_from_slice(&[g, g, g]);
        }
        let ppm = |name: &str, data: &[u8]| {
            let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
            out.extend_from_slice(data);
            let _ = std::fs::write(format!("{prefix}_{name}.ppm"), out);
        };
        ppm("edr", &img);
        ppm("mask", &mask);
        ppm("heat", &heat);
        ppm("hdr", &hdr);
        tracing::info!(
            w,
            h,
            gain_map_used = gain_used,
            surface_on = was_on,
            max_value = max_v,
            ceiling = self.edr.ceiling(),
            pixels_above_white = above,
            share_above = above as f64 / n as f64,
            mean_linear_edr = sum_e / n as f64,
            mean_linear_sdr = sum_s / n as f64,
            mean_ratio = sum_e / sum_s.max(1e-9),
            pixels_2d = hud,
            pixels_2d_not_sdr = hud_bad,
            max_value_on_2d = hud_max,
            "E33 EDR probe of the frame (surface values read back)"
        );
    }

    /// An `Rgba16Float` image read back as f32 RGBA.
    pub(super) fn readback_f16(&self, target: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<f32>> {
        let bytes_per_row = (8 * width).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback f16"), size: (bytes_per_row * height) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
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
        for row in 0..height {
            let r = &data[(row * bytes_per_row) as usize..(row * bytes_per_row + width * 8) as usize];
            for h in r.chunks_exact(2) {
                out.push(f16_to_f32(u16::from_le_bytes([h[0], h[1]])));
            }
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }
}
