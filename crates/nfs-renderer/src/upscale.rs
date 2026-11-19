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

#[cfg(target_os = "macos")]
mod metalfx;

/// The internal size of the three upscaling presets, quality / balanced / performance: AMD FSR's ratios (1.5x, 1.7x, 2x), the one
/// table of FSR (menu levels 2..4, `fsr.rs`) and of MetalFX (E34: temporal levels 2..4, spatial levels 5..7), 67 % / 59 % / 50 % in the menu.
pub(crate) const RENDER_SCALES: [f32; 3] = [1.0 / 1.5, 1.0 / 1.7, 1.0 / 2.0];

/// The internal size of preset `n` (0 = quality, 1 = balanced, anything else performance) of [`RENDER_SCALES`].
pub(crate) fn preset_scale(n: usize) -> f32 {
    RENDER_SCALES[n.min(RENDER_SCALES.len() - 1)]
}

#[cfg(target_os = "macos")]
mod metalfx_temporal;

/// ENHANCEMENT UPSCALE-SOFT: the extra (negative) LOD bias of a TEMPORAL upscaler's main view, on top of the resolution term of
/// [`mip_bias`]. The jittered frames that the temporal filter accumulates cover each window pixel with several sub-pixel samples, so
/// textures may be sampled finer than one internal pixel (FSR 2 / 3's integration guide: log2(render / display) - 1.0; Apple's
/// MetalFX guidance: the resolution term alone). See `RE/notes/ENHANCEMENTS.md` (UPSCALE-SOFT) for the stills it was chosen on.
pub(crate) const TEMPORAL_MIP_EXTRA: f32 = -0.5;

/// ENHANCEMENT UPSCALE-SOFT: the texture LOD bias added to every material's own while the main view is drawn at `render_w` for a
/// window `display_w` wide by an upscaler (`temporal`: FSR 3.1 / MetalFX temporal; else MetalFX's spatial scaler). The resolution
/// term `log2(render / display)` picks the mip for the window's pixel, not for the smaller internal one (without it the scene is
/// textured as if the window were that small, and the upscaler cannot bring the detail back: the softness seen at 67 / 59 / 50 %);
/// FSR 1's guide gives the same values for a spatial scaler (-0.58 at 67 %, -1.0 at 50 %). A temporal one adds
/// [`TEMPORAL_MIP_EXTRA`], at 1:1 too. Never used at level 0 or without an upscaler (nothing is added then).
pub(crate) fn mip_bias(render_w: u32, display_w: u32, temporal: bool) -> f32 {
    let ratio = (render_w.max(1) as f32 / display_w.max(1) as f32).min(1.0);
    ratio.log2() + if temporal { TEMPORAL_MIP_EXTRA } else { 0.0 }
}

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
    /// ENHANCEMENT ENH-UPSCALER: the post-upscale sharpening strength (`None` = off); only FSR has one inside (RCAS), the default does nothing (MetalFX is sharpened by `sharpen.rs` instead)
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

/// What [`UpscaleImages::new`] makes: the `surface`'s format of the input image, and the input (internal) and output (window) sizes.
#[derive(Clone, Copy)]
pub(crate) struct UpscaleSpec {
    pub surface: wgpu::TextureFormat,
    pub input: (u32, u32),
    pub output: (u32, u32),
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
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, up: &dyn Upscaler, spec: UpscaleSpec) -> Self {
        let UpscaleSpec { surface, input: (iw, ih), output: (ow, oh) } = spec;
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
