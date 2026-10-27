//! ENHANCEMENT E34 (temporal levels): the MetalFX temporal scaler (`MTLFXTemporalScaler`), macOS only.
//!
//! The backend of the [`super::Upscaler`] seam for the temporal scalers on a Mac (what `fsr.rs` is on Windows / Linux): it takes the
//! temporal foundation's inputs (`temporal::Inputs`: the float scene at the render size, depth, motion vectors, the reactive mask, the
//! jitter; conventions in `RE/notes/ENHANCEMENTS.md`, "Temporal foundation") and writes an `Rgba16Float` image of the display's size that the
//! post chain (E27 / the depth of field / E10 / E21 / the tone map) then takes as the scene. Same interop as the spatial scaler (`metalfx.rs`): the raw
//! textures through `as_hal` (one guard at a time), the scaler encoded on the Metal command buffer of a wgpu encoder used only through
//! `as_hal_mut`, finished into a `CommandBuffer` the caller submits before the encoder that carries the post chain.

use super::metalfx::raw_texture;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2_metal::{MTLDevice, MTLPixelFormat};
use objc2_metal_fx::{MTLFXTemporalScaler, MTLFXTemporalScalerBase, MTLFXTemporalScalerDescriptor};

/// The internal size per E34 level 1..4 (the temporal ones): 1 = native (temporal anti-aliasing), 2 = quality (67 %), 3 = balanced (59 %),
/// 4 = performance (50 %).
pub fn render_scale_of(level: u8) -> f32 {
    match level {
        0 | 1 => 1.0,
        2 => 0.67,
        3 => 0.59,
        _ => 0.5,
    }
}

type Scaler = Retained<ProtocolObject<dyn MTLFXTemporalScaler>>;

/// Apple's convention, from `MTLFXTemporalScalerBase.motionVectorScaleX`: with the scale 1 each vector points to the pixel's place in the
/// previous frame, in pixels of the colour texture, y down: an object that moves down and right by 10 px has (-10, -10). That is the temporal
/// foundation's convention exactly (render px, `previous - current`, y down), so the scale is (1, 1).
/// The signs that turn the foundation's jitter (the image's shift in render px, x right / y down) into Apple's `jitterOffsetX/Y`: measured, see
/// `RE/notes/ENHANCEMENTS.md` E34 (a static scene's 24th frame is closest to the native frame, and steadiest, with the shift passed as it is).
const JITTER_SIGN: [f32; 2] = [1.0, 1.0];

const MV_SCALE: [f32; 2] = [1.0, 1.0];

pub struct MetalFxTemporal {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    /// the scaler for (render w, h, display w, h); a change makes a new one (history gone)
    scaler: Option<((u32, u32, u32, u32), Scaler)>,
    cut: bool,
    warned: bool,
    /// the jitter's signs, see [`JITTER_SIGN`]
    jitter_sign: [f32; 2],
    mask: bool,
}

// the Core (and with it this) moves to the render thread once, before any frame; the Metal objects are thread-safe
unsafe impl Send for MetalFxTemporal {}

impl MetalFxTemporal {
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        if AnyClass::get(c"MTLFXTemporalScalerDescriptor").is_none() {
            return Err("MetalFX is not present (macOS older than 13)".to_string());
        }
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }.ok_or_else(|| "the device is not a Metal device".to_string())?;
        let raw = hal.raw_device().clone();
        drop(hal);
        if !unsafe { MTLFXTemporalScalerDescriptor::supportsDevice(&raw) } {
            return Err("this GPU does not support the MetalFX temporal scaler".to_string());
        }
        Ok(Self { device: raw, scaler: None, cut: false, warned: false, jitter_sign: JITTER_SIGN, mask: true })
    }

    fn scaler_for(&mut self, key: (u32, u32, u32, u32)) -> Option<&Scaler> {
        if self.scaler.as_ref().is_none_or(|(k, _)| *k != key) {
            let (iw, ih, ow, oh) = key;
            let desc = unsafe { MTLFXTemporalScalerDescriptor::new() };
            unsafe {
                desc.setColorTextureFormat(MTLPixelFormat::RGBA16Float);
                desc.setDepthTextureFormat(MTLPixelFormat::Depth32Float);
                desc.setMotionTextureFormat(MTLPixelFormat::RG16Float);
                desc.setOutputTextureFormat(MTLPixelFormat::RGBA16Float);
                desc.setInputWidth(iw as usize);
                desc.setInputHeight(ih as usize);
                desc.setOutputWidth(ow as usize);
                desc.setOutputHeight(oh as usize);
                // the foundation's reactive mask (R8Unorm: translucent effects write their coverage) is MetalFX's reactive mask
                if self.mask {
                    desc.setReactiveMaskTextureEnabled(true);
                    desc.setReactiveMaskTextureFormat(MTLPixelFormat::R8Unorm);
                }
                // the scaler's pipelines are built before `new...` returns: with the default (asynchronous) start the first frames came out
                // as nothing (a scripted still is frame 0)
                desc.setRequiresSynchronousInitialization(true);
                // no exposure texture: nothing is pre-exposed (the exposure is 1)
                desc.setAutoExposureEnabled(false);
            }
            let s = unsafe { desc.newTemporalScalerWithDevice(&self.device) }?;
            tracing::info!("MetalFX temporal scaler (ENHANCEMENT E34): {iw}x{ih} -> {ow}x{oh}");
            self.scaler = Some((key, s));
        }
        self.scaler.as_ref().map(|(_, s)| s)
    }
}

impl super::Upscaler for MetalFxTemporal {
    fn name(&self) -> &'static str {
        "MetalFX temporal"
    }

    fn render_scale(&self, level: u8) -> f32 {
        render_scale_of(level)
    }

    fn output_format(&self) -> wgpu::TextureFormat {
        crate::gpu::HDR_FORMAT
    }

    fn output_usage(&self) -> wgpu::TextureUsages {
        // MTLTextureUsageShaderWrite (+ RenderTarget, as the spatial output)
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT
    }

    fn reset_history(&mut self) {
        self.cut = true;
    }

    fn encode(&mut self, _: &wgpu::Device, _: &wgpu::Texture, _: &wgpu::Texture) -> Option<wgpu::CommandBuffer> {
        None
    }

    fn encode_temporal(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue, inputs: &crate::temporal::Inputs, output: &wgpu::Texture) -> Option<wgpu::CommandBuffer> {
        let [iw, ih] = inputs.render_size;
        let (ow, oh) = (output.width(), output.height());
        let scaler = self.scaler_for((iw, ih, ow, oh))?.clone();
        let (color, depth, mv, out) = (raw_texture(&inputs.color)?, raw_texture(&inputs.depth)?, raw_texture(&inputs.motion_vectors)?, raw_texture(output)?);
        let mask = if self.mask { Some(raw_texture(&inputs.reactive_mask)?) } else { None };
        let reset = inputs.reset_history || std::mem::take(&mut self.cut);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("metalfx temporal") });
        let sign = self.jitter_sign;
        let done = unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|e| {
                let cmd = e?.raw_command_buffer()?;
                scaler.setColorTexture(Some(&color));
                scaler.setDepthTexture(Some(&depth));
                scaler.setMotionTexture(Some(&mv));
                scaler.setReactiveMaskTexture(mask.as_deref());
                scaler.setOutputTexture(Some(&out));
                scaler.setInputContentWidth(iw as usize);
                scaler.setInputContentHeight(ih as usize);
                // the foundation's jitter is the image's shift in render px (x right, y down), in [-0.5, 0.5]
                scaler.setJitterOffsetX(sign[0] * inputs.jitter_offset[0]);
                scaler.setJitterOffsetY(sign[1] * inputs.jitter_offset[1]);
                scaler.setMotionVectorScaleX(MV_SCALE[0]);
                scaler.setMotionVectorScaleY(MV_SCALE[1]);
                scaler.setPreExposure(inputs.pre_exposure);
                scaler.setReset(reset);
                // the foundation's depth: standard, 0 near .. 1 far
                scaler.setDepthReversed(false);
                scaler.encodeToCommandBuffer(cmd);
                Some(())
            })
        };
        if done.is_none() {
            if !self.warned {
                tracing::warn!("MetalFX temporal (ENHANCEMENT E34) could not encode");
                self.warned = true;
            }
            return None;
        }
        Some(encoder.finish())
    }
}
