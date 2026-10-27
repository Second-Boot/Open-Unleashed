//! ENHANCEMENT E34: upscaling the 3D scene from a fraction of the window's size.
//!
//! The seam is [`Upscaler`]: a backend turns the scene's tone-mapped 8-bit image (the internal size,
//! `render_scale(level)` of the window) into a window-sized image. The backends: MetalFX's spatial scaler
//! (`metalfx.rs`, **macOS only**, compiled out elsewhere; E34) and, ENHANCEMENT E37, AMD FSR 3.1 through the
//! vendored `wgpu-ffx` (`fsr.rs`, every platform in the build, available on Windows / Linux): a TEMPORAL
//! backend ([`Upscaler::temporal`]) which does not take the tone-mapped image but the temporal foundation's
//! float scene, depth, vectors and masks, and writes a float image of the window's size that the post chain
//! then works on (`gpu.rs`, `encode_enhanced`).
//!
//! The pipeline with the scaler on (`gpu.rs`, `encode_enhanced`): scene (+ MSAA resolve, bloom, ...) ->
//! tone map into an 8-bit image of the internal size -> **the scaler** -> a plain blit of its output to
//! the window image -> the 2D pass (HUD, menus) at native size -> the gamma ramp. The scaler replaces
//! E3's scaling blit; with it unavailable the same blit scales bilinearly (E3's way).

// the seam is generic but its only backend is macOS-only: nothing uses it elsewhere
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_variables))]

use crate::gfx;

mod fsr;

/// ENHANCEMENT E37: `NFS_FSR_NAGA=1` keeps FSR's SPIR-V on naga's front end on Vulkan too (by default Vulkan takes it as it is, the
/// shader-module passthrough: the SPD image's `Coherent` decoration is kept).
pub(crate) fn fsr_naga_forced() -> bool {
    std::env::var("NFS_FSR_NAGA").is_ok_and(|v| !v.is_empty() && v != "0")
}

#[cfg(target_os = "macos")]
mod metalfx;
#[cfg(target_os = "macos")]
mod metalfx_temporal;

/// The upscaler behind a level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpscalerKind {
    #[cfg(target_os = "macos")]
    MetalFx,
    /// ENHANCEMENT E34 (temporal levels): `MTLFXTemporalScaler`
    #[cfg(target_os = "macos")]
    MetalFxTemporal,
    /// ENHANCEMENT E37: AMD FSR 3.1, temporal
    Fsr3,
}

/// A scaling backend. It records its work as a command buffer of its own (a backend that needs the
/// platform's raw command buffer cannot share wgpu's encoder), submitted between the encoder that wrote
/// the input and the one that reads the output.
pub trait Upscaler: Send {
    /// for the log
    fn name(&self) -> &'static str;
    /// the fraction of the window's size the scene is drawn at for the menu level (1..)
    fn render_scale(&self, level: u8) -> f32;
    /// the format the output image must have, and the extra usages it needs
    fn output_format(&self) -> wgpu::TextureFormat;
    fn output_usage(&self) -> wgpu::TextureUsages;
    /// scale `input` (the internal size) into `output` (the window's); `None` = it failed (the caller
    /// then blits bilinearly)
    fn encode(&mut self, device: &wgpu::Device, input: &wgpu::Texture, output: &wgpu::Texture) -> Option<wgpu::CommandBuffer>;
    /// ENHANCEMENT E37: a temporal backend (FSR 3.1) is not given the tone-mapped image of [`Self::encode`]: the main view is
    /// drawn jittered at 1 sample with vectors and masks (`temporal.rs`), [`Self::encode_temporal`] takes them and the post
    /// chain runs at the window's size on its output (`output_format` / `output_usage`).
    /// The temporal inputs of the frame just drawn into `output` (the window's size); `None` = it
    /// failed (the caller then switches the backend off)
    fn encode_temporal(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, _inputs: &crate::temporal::Inputs, _output: &wgpu::Texture) -> Option<wgpu::CommandBuffer> {
        None
    }
    /// ENHANCEMENT E37: drop the history at the next frame (a camera cut the renderer cannot see)
    fn reset_history(&mut self) {}
    /// ENHANCEMENT ENH-UPSCALER: the post-upscale sharpening strength (`None` = off); only FSR has one (RCAS), the default does nothing
    fn set_sharpness(&mut self, _strength: Option<f32>) {}
}

/// The backend of `kind`, or why the platform / device cannot (logged once by the caller).
pub fn create(kind: UpscalerKind, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Box<dyn Upscaler>, String> {
    match kind {
        #[cfg(target_os = "macos")]
        UpscalerKind::MetalFx => metalfx::MetalFxSpatial::new(device).map(|u| Box::new(u) as Box<dyn Upscaler>),
        #[cfg(target_os = "macos")]
        UpscalerKind::MetalFxTemporal => metalfx_temporal::MetalFxTemporal::new(device).map(|u| Box::new(u) as Box<dyn Upscaler>),
        UpscalerKind::Fsr3 => fsr::Fsr3::new(device, queue).map(|u| Box::new(u) as Box<dyn Upscaler>),
    }
}

/// ENHANCEMENT E34: mark a scaler's output image as initialised in wgpu's eyes. The scaler writes it through the raw Metal command buffer,
/// which wgpu cannot see: left alone, the first wgpu pass that READS the image finds it "never written", and wgpu zero-fills it right
/// before that pass, i.e. AFTER the scaler wrote it - the first frame came out black (a scripted still is frame 0). One clear pass,
/// submitted at once, makes the later reads trust the image. Costs one tiny submit when the image is made, never per frame.
pub(crate) fn mark_written(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) {
    let view = tex.create_view(&Default::default());
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("upscale output init") });
    drop(gfx::color_pass_untimed(&mut enc, "upscale output init", &view, wgpu::LoadOp::Clear(wgpu::Color::BLACK)));
    queue.submit([enc.finish()]);
}

/// The images the scaler works between, kept while the sizes hold: the tone-mapped input (internal size,
/// the surface's format) and the output (window size, the backend's format) with bind groups for the blit.
pub(crate) struct UpscaleImages {
    pub size: (u32, u32, u32, u32),
    pub input: wgpu::Texture,
    pub input_view: wgpu::TextureView,
    pub input_bg: wgpu::BindGroup,
    pub output: wgpu::Texture,
    pub output_bg: wgpu::BindGroup,
}

impl UpscaleImages {
    #[allow(clippy::too_many_arguments)]
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, up: &dyn Upscaler, surface: wgpu::TextureFormat, iw: u32, ih: u32, ow: u32, oh: u32) -> Self {
        let make = |label, w, h, format, usage| {
            gfx::texture_2d(device, label, w, h, format, usage)
        };
        let bind = |label, view: &wgpu::TextureView| {
            gfx::tex_sampler_bg(device, label, layout, view, sampler)
        };
        let input = make("upscale input", iw, ih, surface, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        let input_view = input.create_view(&Default::default());
        let output = make("upscale output", ow, oh, up.output_format(), up.output_usage() | wgpu::TextureUsages::TEXTURE_BINDING);
        mark_written(device, queue, &output);
        let output_view = output.create_view(&Default::default());
        let input_bg = bind("upscale input", &input_view);
        let output_bg = bind("upscale output", &output_view);
        Self { size: (iw, ih, ow, oh), input, input_view, input_bg, output, output_bg }
    }
}
