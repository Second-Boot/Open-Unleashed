//! ENHANCEMENT E37: AMD FidelityFX Super Resolution 3.1 (temporal), through the vendored `wgpu-ffx`
//! (`third_party/wgpu-ffx`: FidelityFX SDK 1.1.4's compute shaders as SPIR-V, ported to wgpu 30).
//!
//! The backend of the [`super::Upscaler`] seam for the temporal scalers: it takes the temporal foundation's
//! inputs (`temporal::Inputs`: the float scene, depth, motion vectors, the two masks, the jitter, the camera
//! numbers; conventions in `RE/notes/ENHANCEMENTS.md`, "Temporal foundation") at the internal size and writes a
//! **float `Rgba16Float` image of the display's size**, which the post chain (E27 / the depth of field / E10 / E21 / the tone
//! map) then takes as the scene (`gpu.rs`, `encode_enhanced`).
//!
//! Context flags and constants, and why:
//!
//! * `HIGH_DYNAMIC_RANGE`: **on**. The scene is the engine's display-referred (gamma-encoded) colour: 0..1 for the
//!   picture but overshooting 1 on the sparks, the flare cores and the glare on paint (E11, E18), and the bloom
//!   (E10, which runs AFTER the upscale) feeds on exactly that overshoot. FSR's HDR option is the one made for
//!   values beyond 1 (it weights the history blend with a tonemapped copy of the colour, which on display-referred
//!   values is only a weight, not an output transform) and nothing in the scene is pre-exposed, so it costs
//!   nothing; the LDR option assumes 0..1 and would be free to clip those highlights before the bloom.
//! * `AUTO_EXPOSURE`: on, no exposure texture (FSR measures the luma pyramid's mean itself); `pre_exposure` = 1
//!   (nothing is pre-exposed: E21's eye adaptation and the tone map act on the image AFTER the upscale).
//! * motion vectors at render size, no jitter cancellation, no `DISPLAY_RESOLUTION_MOTION_VECTORS`; depth is
//!   standard (0 near .. 1 far), finite: no `DEPTH_INVERTED` / `DEPTH_INFINITE`. All as the foundation writes them.
//! * RCAS sharpening on at [`SHARPNESS`].
//!
//! The SPIR-V goes through naga (`wgpu`'s SPIR-V front end) on every backend; the vendored crate strips the
//! `Coherent` decoration naga 30 rejects (see `third_party/README.md`).

use super::Upscaler;
use crate::temporal::Inputs;
use wgpu_ffx::{FsrContext, FsrContextFlags, FsrContextInfo, FsrDispatchFlags, FsrDispatchInfo, FsrView};

/// RCAS sharpening strength (0 = none .. 1 = most): FSR's own recommendation range is about 0.2 .. 0.8; 0.3 is a
/// light touch that counters the temporal filter's softness without ringing on the road markings and the foliage.
/// The default of the menu's sharpness row (E39 level 2); the row sets the strength (ENH-UPSCALER).
pub const SHARPNESS: f32 = 0.3;

/// The internal size per menu level: 1 = native (FSR as a temporal anti-aliaser), 2 = quality 1.5x, 3 = balanced
/// 1.7x, 4 = performance 2x (AMD's presets' ratios).
pub fn render_scale_of(level: u8) -> f32 {
    match level {
        0 | 1 => 1.0,
        2 => super::preset_scale(0),
        3 => super::preset_scale(1),
        _ => super::preset_scale(2),
    }
}

/// The scratch images `FsrDispatchInfo` wants at the render size (FSR writes and shares them with later effects).
struct Scratch {
    render: [u32; 2],
    dilated_depth: wgpu::Texture,
    dilated_motion_vectors: wgpu::Texture,
    reconstructed_previous_depth: wgpu::Buffer,
}

pub struct Fsr3 {
    ctx: FsrContext,
    /// the view is for these sizes (render, display); a change makes a new one (history gone)
    view: Option<(FsrView, [u32; 2], [u32; 2])>,
    scratch: Option<Scratch>,
    last: Option<std::time::Instant>,
    /// the history must be dropped at the next dispatch (a camera cut the renderer cannot see: the app said so)
    cut: bool,
    /// the dispatch failed once (logged once)
    warned: bool,
    /// ENHANCEMENT ENH-UPSCALER: the RCAS strength (`None` = off), set by the renderer from E39 before each dispatch
    sharpness: Option<f32>,
}

impl Fsr3 {
    /// Build the context (compiles the nine compute pipelines). A device that cannot do it (too few storage
    /// textures per stage, a missing feature, a refused pipeline) answers with the reason, caught from the validation error scope.
    /// (Metal has no read-write `Rg16Float` storage image: the vendored SPD pyramid uses `Rgba16Float`, ENH-UPSCALER.)
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        let limits = device.limits();
        // the SPD passes bind 8 storage images (frame info, six mips of the pyramid, the half-size depth)
        if limits.max_storage_textures_per_shader_stage < 8 {
            return Err(format!("the device allows {} storage textures per stage, FSR 3 needs 8", limits.max_storage_textures_per_shader_stage));
        }
        let need = wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM | wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::CLEAR_TEXTURE;
        if !device.features().contains(need) {
            return Err(format!("the device lacks features FSR 3 needs: {:?}", need - device.features()));
        }
        let mut flags = FsrContextFlags::AUTO_EXPOSURE | FsrContextFlags::HIGH_DYNAMIC_RANGE;
        if device.features().contains(wgpu::Features::PASSTHROUGH_SHADERS) {
            flags |= FsrContextFlags::SPIRV_PASSTHROUGH;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let ctx = FsrContext::new(FsrContextInfo { device: device.clone(), flags });
        if let Some(err) = pollster::block_on(scope.pop()) {
            return Err(format!("{err}"));
        }
        // one whole dispatch on 64x64 images inside an error scope: a device that builds the pipelines but then refuses a
        // bind group, a clear or a dispatch (the library panics on those) falls back here, not in the middle of a race
        probe(device, queue, &ctx)?;
        Ok(Self { ctx, view: None, scratch: None, last: None, cut: false, warned: false, sharpness: Some(SHARPNESS) })
    }

    fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, render: [u32; 2], disp: [u32; 2]) -> Result<(), String> {
        if self.view.as_ref().is_none_or(|(_, r, d)| *r != render || *d != disp) {
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let view = self.ctx.create_view(queue, render, disp);
            if let Some(err) = pollster::block_on(scope.pop()) {
                return Err(format!("{err}"));
            }
            tracing::info!("FSR 3.1 (ENHANCEMENT E37): {}x{} -> {}x{}, {:.1} MB of history", render[0], render[1], disp[0], disp[1], view.estimated_memory_usage() as f64 / 1048576.0);
            self.view = Some((view, render, disp));
            self.last = None;
        }
        if self.scratch.as_ref().is_none_or(|s| s.render != render) {
            let make = |label, format| crate::gfx::texture_2d(device, label, render[0], render[1], format, wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING);
            self.scratch = Some(Scratch {
                render,
                dilated_depth: make("fsr dilated depth", wgpu::TextureFormat::R32Float),
                dilated_motion_vectors: make("fsr dilated motion vectors", wgpu::TextureFormat::Rg16Float),
                reconstructed_previous_depth: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("fsr reconstructed previous depth"),
                    size: render[0] as u64 * render[1] as u64 * 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
            });
        }
        Ok(())
    }
}

impl Upscaler for Fsr3 {
    fn name(&self) -> &'static str {
        "AMD FSR 3.1 (wgpu-ffx)"
    }

    fn render_scale(&self, level: u8) -> f32 {
        render_scale_of(level)
    }

    fn output_format(&self) -> wgpu::TextureFormat {
        crate::gpu::HDR_FORMAT
    }

    fn output_usage(&self) -> wgpu::TextureUsages {
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
    }

    fn reset_history(&mut self) {
        self.cut = true;
    }

    fn set_sharpness(&mut self, strength: Option<f32>) {
        self.sharpness = strength;
    }

    fn encode(&mut self, _: &wgpu::Device, _: &wgpu::Texture, _: &wgpu::Texture) -> Option<wgpu::CommandBuffer> {
        None
    }

    fn encode_temporal(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, inputs: &Inputs, output: &wgpu::Texture) -> Option<wgpu::CommandBuffer> {
        let render = inputs.render_size;
        let disp = [output.width(), output.height()];
        if let Err(why) = self.ensure(device, queue, render, disp) {
            if !self.warned {
                tracing::warn!("FSR 3.1 (ENHANCEMENT E37) cannot allocate: {why}");
                self.warned = true;
            }
            return None;
        }
        let now = std::time::Instant::now();
        // the time since the last dispatch in milliseconds (FSR wants >= 1); the first frame / after a pause: one 60 Hz frame
        let dt = self.last.map_or(16.6, |t| (now - t).as_secs_f32() * 1000.0).clamp(1.0, 100.0);
        self.last = Some(now);
        let scratch = self.scratch.as_ref()?;
        let info = FsrDispatchInfo {
            color: inputs.color.clone(),
            depth: inputs.depth.clone(),
            motion_vectors: inputs.motion_vectors.clone(),
            exposure: None,
            reactive_mask: Some(inputs.reactive_mask.clone()),
            transparency_and_composition: Some(inputs.transparency_and_composition.clone()),
            dilated_depth: scratch.dilated_depth.clone(),
            dilated_motion_vectors: scratch.dilated_motion_vectors.clone(),
            reconstructed_previous_depth: scratch.reconstructed_previous_depth.clone(),
            output: output.clone(),
            jitter_offset: inputs.jitter_offset,
            // the vectors are pixels of the render size already (the foundation's convention)
            motion_vector_scale: [1.0, 1.0],
            render_size: render,
            upscale_size: disp,
            enable_sharpening: self.sharpness.is_some(),
            sharpness: self.sharpness.unwrap_or(SHARPNESS),
            frame_time_delta: dt,
            pre_exposure: inputs.pre_exposure,
            reset_history: inputs.reset_history || std::mem::take(&mut self.cut),
            camera_near: inputs.camera_near,
            camera_far: inputs.camera_far,
            camera_fov_y: inputs.camera_fov_y,
            view_space_to_meters_factor: 1.0,
            flags: FsrDispatchFlags::empty(),
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fsr 3.1") });
        let view = &mut self.view.as_mut()?.0;
        match self.ctx.dispatch(view, &mut encoder, &info) {
            Ok(()) => Some(encoder.finish()),
            Err(e) => {
                if !self.warned {
                    tracing::warn!("FSR 3.1 (ENHANCEMENT E37) dispatch refused: {e}");
                    self.warned = true;
                }
                None
            }
        }
    }
}


/// A dispatch on tiny images (the library's own smoke test): `Err` with the validation message when the device refuses any part.
fn probe(device: &wgpu::Device, queue: &wgpu::Queue, ctx: &FsrContext) -> Result<(), String> {
    let (rs, us) = ([64u32, 64], [96u32, 96]);
    let tex = |label, size: [u32; 2], format, usage| crate::gfx::texture_2d(device, label, size[0], size[1], format, usage);
    let sampled = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    let storage = wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING;
    let info = FsrDispatchInfo {
        color: tex("fsr probe color", rs, crate::gpu::HDR_FORMAT, sampled),
        depth: tex("fsr probe depth", rs, wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT),
        motion_vectors: tex("fsr probe vectors", rs, crate::temporal::MV_FORMAT, sampled),
        exposure: None,
        reactive_mask: Some(tex("fsr probe reactive", rs, crate::temporal::MASK_FORMAT, sampled)),
        transparency_and_composition: Some(tex("fsr probe composition", rs, crate::temporal::MASK_FORMAT, sampled)),
        dilated_depth: tex("fsr probe dilated depth", rs, wgpu::TextureFormat::R32Float, storage),
        dilated_motion_vectors: tex("fsr probe dilated vectors", rs, wgpu::TextureFormat::Rg16Float, storage),
        reconstructed_previous_depth: device.create_buffer(&wgpu::BufferDescriptor { label: Some("fsr probe depth buffer"), size: rs[0] as u64 * rs[1] as u64 * 4, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
        output: tex("fsr probe output", us, crate::gpu::HDR_FORMAT, storage),
        jitter_offset: [0.25, -0.25],
        motion_vector_scale: [1.0, 1.0],
        render_size: rs,
        upscale_size: us,
        enable_sharpening: true,
        sharpness: SHARPNESS,
        frame_time_delta: 16.6,
        pre_exposure: 1.0,
        reset_history: true,
        camera_near: 0.5,
        camera_far: 700.0,
        camera_fov_y: 1.0,
        view_space_to_meters_factor: 1.0,
        flags: FsrDispatchFlags::empty(),
    };
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut view = ctx.create_view(queue, rs, us);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fsr probe") });
        let r = ctx.dispatch(&mut view, &mut encoder, &info);
        queue.submit([encoder.finish()]);
        r
    }));
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let err = pollster::block_on(scope.pop());
    match (ran, err) {
        (Ok(Ok(())), None) => Ok(()),
        (_, Some(e)) => Err(format!("{e}")),
        (Ok(Err(e)), None) => Err(format!("{e}")),
        (Err(_), None) => Err("the probe dispatch panicked".into()),
    }
}
