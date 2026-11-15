//! ENHANCEMENT E39 — sharpening after the MetalFX scalers (`RE/notes/ENHANCEMENTS.md`, UPSCALE-SOFT). **Not in the original**;
//! render-only (it reads the upscaled 3D image of the main view and writes a sharpened copy; nothing reaches the simulation, the
//! network or a replay). FSR 3.1 has its own RCAS pass inside the backend (`upscale/fsr.rs`); MetalFX has no sharpening control, so
//! its temporal output (the float scene, before the post chain) and its spatial output (the tone-mapped 8-bit image, as the present
//! blit) go through this pass when E39 is above 0. E39's level is the strength for both backends, mapped as FSR maps it.
//!
//! The method is AMD's Robust Contrast Adaptive Sharpening, `FsrRcasF` of FidelityFX Super Resolution 1 (`ffx_fsr1.h`,
//! <https://github.com/GPUOpen-Effects/FidelityFX-FSR>, Copyright (c) 2021 Advanced Micro Devices, Inc., MIT licence: "Permission is
//! hereby granted, free of charge, to any person obtaining a copy of this software ... to deal in the Software without restriction ...
//! The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.
//! THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND ..."), with its `FSR_RCAS_DENOISE` option on, as FSR 2 / 3 build
//! it. `sharpen.wgsl` is a WGSL rewrite of the 32-bit path: the same five-tap cross, limits, `FSR_RCAS_LIMIT` and noise damping.
//! What differs: full-precision reciprocals instead of the approximate ones; zero-range guards; and, for the float scene, the taps
//! go through the invertible `c / (1 + max(c))` (RCAS's limits assume 0..1; the float scene's highlights go above 1).

use crate::gfx::{self, Fullscreen, RtImage, Variants};
use crate::wgsl;

/// The pass's shader.
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, include_str!("sharpen.wgsl")])
}

/// RCAS's `con` for a 0..1 sharpness as FSR 3 maps it (`wgpu-ffx` `lib.rs`: stops = 2 - 2 s, con = 2^-stops).
pub(crate) fn rcas_con(sharpness: f32) -> f32 {
    (-(2.0 - 2.0 * sharpness.clamp(0.0, 1.0))).exp2()
}

/// The pipelines (one per target format, float flag and strength: E39 has four strengths) and the float output image of the
/// temporal case, kept while the size holds.
pub(crate) struct Sharpen {
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    /// the pipeline per (target format, float source, `CON`'s bits), built at its first use
    pipes: Variants<(wgpu::TextureFormat, bool, u32), wgpu::RenderPipeline>,
    image: Option<RtImage>,
}

impl Sharpen {
    /// `src_layout`: the sprite layout (texture at 0, sampler at 1) the sources' bind groups are made in.
    pub(crate) fn new(device: &wgpu::Device, src_layout: &wgpu::BindGroupLayout) -> Self {
        let module = gfx::wgsl_module(device, "sharpen", &source());
        let layout = gfx::pipeline_layout(device, "sharpen", &[src_layout]);
        Self { module, layout, pipes: Variants::default(), image: None }
    }

    fn pipe(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, hdr: bool, con: f32) -> &wgpu::RenderPipeline {
        let (layout, module) = (&self.layout, &self.module);
        self.pipes.get((format, hdr, con.to_bits()), || Fullscreen::new("sharpen", layout, module).constants(&[("CON", con as f64), ("HDR", hdr as u32 as f64)]).build(device, &[gfx::target(format, None)]))
    }

    /// Sharpen `src` (a bind group of the sprite layout, an image of `dest`'s size) into `dest` (`format`). `hdr`: the source is the
    /// float scene. One full-screen pass into `encoder`, timed as "sharpen".
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, src: &wgpu::BindGroup, dest: &wgpu::TextureView, format: wgpu::TextureFormat, hdr: bool, sharpness: f32) {
        let pipe = self.pipe(device, format, hdr, rcas_con(sharpness));
        gfx::fullscreen_pass(encoder, "sharpen pass", "sharpen", dest, wgpu::LoadOp::Clear(wgpu::Color::BLACK), pipe, &[src]);
    }

    /// The temporal case: sharpen the float scene `src` into this module's own float image of `size` and return that image (its bind
    /// group in `layout` and its view), for the post chain to carry on.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run_float(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, src: &wgpu::BindGroup, size: (u32, u32), format: wgpu::TextureFormat, sharpness: f32) -> (wgpu::BindGroup, wgpu::TextureView) {
        let (bg, view) = {
            let img = RtImage::ensure(&mut self.image, device, "sharpened 3d", layout, sampler, size.0, size.1, format, gfx::RT_USAGE);
            (img.bg.clone(), img.view.clone())
        };
        self.run(device, encoder, src, &view, format, true, sharpness);
        (bg, view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FSR 3's mapping: sharpness 1 = con 1 (the strongest), 0 = 2^-2; E39's default 0.3 = 2^-1.4.
    #[test]
    fn con_follows_fsr3() {
        assert_eq!(rcas_con(1.0), 1.0);
        assert_eq!(rcas_con(0.0), 0.25);
        assert!((rcas_con(0.3) - 2f32.powf(-1.4)).abs() < 1e-6);
    }
}
