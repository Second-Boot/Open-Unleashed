//! ENHANCEMENT E33 diagnostics, macOS only (the EDR output exists there alone): `NFS_EDR_PROBE=<path prefix>` draws a
//! frame through the EDR chain into an image of its own, reads the surface values back and logs / writes what they are
//! (`edr_probe`, `edr_probe_stats`, the half-float read-back `readback_f16`). Moved out of `readback.rs` as they were;
//! the module is `cfg(target_os = "macos")` instead of each function.

use crate::gfx;
use super::*;

/// The sRGB transfer function's constants (IEC 61966-2-1): the linear segment's knee (encoded / linear), its slope, the
/// power segment's offset, scale and exponent.
const SRGB_KNEE_ENC: f32 = 0.04045;
const SRGB_KNEE_LIN: f32 = 0.003_130_8;
const SRGB_SLOPE: f32 = 12.92;
const SRGB_OFFSET: f32 = 0.055;
const SRGB_SCALE: f32 = 1.055;
const SRGB_GAMMA: f32 = 2.4;
/// A surface value counts as above SDR white past this (half a 10-bit step of slack).
const ABOVE_WHITE: f32 = 1.0005;
/// The most a 2D pixel may differ from its byte's plain decode and still count as SDR.
const HUD_TOLERANCE: f32 = 1.0 / 1024.0;
/// `_edr.ppm`'s glow for values above white: green and blue from these at white to 255 at the cap.
const GLOW_GREEN: f32 = 140.0;
const GLOW_BLUE: f32 = 30.0;
/// `_hdr.ppm`'s exposure, and the darkest SDR decode `_heat.ppm` divides by.
const HDR_EXPOSURE: f32 = 0.5;
const HEAT_FLOOR: f32 = 0.02;
/// A channel's largest byte, as a float.
const BYTE_MAX: f32 = 255.0;

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
        self.encode_with_gamma(scene, camera, FrameOut::of(&view, &tex, &depth));
        self.edr.pass = false;
        self.gamma = gamma;
        self.edr_probe_stats(&tex, &pre2d, gamma, was_on, prefix);
    }

    /// ENHANCEMENT E33 diagnostic, the second half of [`Self::edr_probe`]: the present pass of the frame already in
    /// `tex` (with `pre2d` its copy before the 2D) into an image of its own, read back and logged. Also called from
    /// the window path once (frame 20) under `NFS_EDR_PROBE`, so the MetalFX / SMAA paths can be measured too.
    /// (`tex` and `pre2d` are the window frame's size.)
    pub(super) fn edr_probe_stats(&mut self, tex: &wgpu::Texture, pre2d: &wgpu::Texture, gamma: f32, was_on: bool, prefix: &str) {
        let (w, h) = (tex.width(), tex.height());
        let out = gfx::texture_2d(&self.device, "edr probe", w, h, HDR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
        let out_view = out.create_view(&Default::default());
        let mut encoder = self.frame_cmds.take(&self.device);
        self.edr.present(&self.device, &mut self.belt, &mut encoder, &out_view, gamma);
        self.frame_cmds.park(encoder);
        self.submit_frame();
        let gain_used = self.edr.gain_used;
        let f = self.readback_f16(&out, w, h);
        let frame8 = self.readback(tex, w, h);
        let before = self.readback(pre2d, w, h);
        let (Ok(f), Ok(frame8), Ok(before)) = (f, frame8, before) else {
            tracing::warn!("E33 probe: read-back failed");
            return;
        };
        let dec = |b: u8| {
            let v = b as f32 / BYTE_MAX;
            if v <= SRGB_KNEE_ENC { v / SRGB_SLOPE } else { ((v + SRGB_OFFSET) / SRGB_SCALE).powf(SRGB_GAMMA) }
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
            if em > ABOVE_WHITE {
                above += 1;
            }
            sum_e += (e[0] + e[1] + e[2]) as f64 / 3.0;
            sum_s += (dec(m8[0]) + dec(m8[1]) + dec(m8[2])) as f64 / 3.0;
            if touched {
                hud += 1;
                let d = (0..3).map(|c| (e[c] - dec(m8[c])).abs()).fold(0f32, f32::max);
                hud_max = hud_max.max(em);
                if d > HUD_TOLERANCE {
                    hud_bad += 1;
                }
            }
            let enc = |v: f32| {
                let v = v.clamp(0.0, 1.0);
                (BYTE_MAX * if v <= SRGB_KNEE_LIN { v * SRGB_SLOPE } else { SRGB_SCALE * v.powf(1.0 / SRGB_GAMMA) - SRGB_OFFSET }).round() as u8
            };
            if em > ABOVE_WHITE {
                let t = ((em - 1.0) / (crate::edr::CAP - 1.0)).clamp(0.0, 1.0);
                img.extend_from_slice(&[255, (GLOW_GREEN + (BYTE_MAX - GLOW_GREEN) * t) as u8, (GLOW_BLUE + (BYTE_MAX - GLOW_BLUE) * t) as u8]);
            } else {
                img.extend_from_slice(&[enc(e[0]), enc(e[1]), enc(e[2])]);
            }
            let sm = dec(m8[0]).max(dec(m8[1])).max(dec(m8[2]));
            hdr.extend_from_slice(&[enc(e[0] * HDR_EXPOSURE), enc(e[1] * HDR_EXPOSURE), enc(e[2] * HDR_EXPOSURE)]);
            heat.extend_from_slice(&[(BYTE_MAX * (em / sm.max(HEAT_FLOOR) - 1.0)).clamp(0.0, BYTE_MAX) as u8, (BYTE_MAX * em / 3.0).clamp(0.0, BYTE_MAX) as u8, 0]);
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
        Ok(self.read_texture(target, width, height, 8)?.chunks_exact(2).map(|h| f16_to_f32(u16::from_le_bytes([h[0], h[1]]))).collect())
    }
}
