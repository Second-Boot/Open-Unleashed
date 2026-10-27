//! ENHANCEMENT E34: the MetalFX spatial scaler (`MTLFXSpatialScaler`), macOS only.
//!
//! Interop with wgpu 30's Metal backend: the raw `MTLDevice` comes from `Device::as_hal`, the raw
//! textures from `Texture::as_hal`, and the scaler is encoded onto the `MTLCommandBuffer` of a wgpu
//! command encoder used *only* through `as_hal_mut` (wgpu refuses to mix its own passes with raw
//! encoding in one encoder), finished into a `CommandBuffer` that the caller submits between the
//! encoder that wrote the input and the one that reads the output. Metal's own hazard tracking orders
//! the three command buffers on the queue. The scaler is colour-only (no depth / motion vectors, no
//! jitter): nothing in the scene has to change, and the MSAA resolve before it is untouched.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2::Message;
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_metal_fx::{MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerColorProcessingMode, MTLFXSpatialScalerDescriptor};

/// The raw `MTLTexture` of a wgpu texture, as an owned handle.
///
/// ENHANCEMENT E34 (the lock fix): `Texture::as_hal` returns a guard that holds wgpu's snatch lock (read) for as long as it lives;
/// a second `as_hal` while the first guard is alive re-enters that lock ("snatch lock acquired recursively" in a debug build, and
/// in release a deadlock waiting to happen once the device wants the write lock, e.g. a texture being destroyed). Taking one
/// guard at a time and keeping only the retained Metal handle (the wgpu texture itself stays alive in the caller) cannot nest.
pub(super) fn raw_texture(t: &wgpu::Texture) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
    let g = unsafe { t.as_hal::<wgpu::hal::api::Metal>() }?;
    Some(g.raw_handle().retain())
}

type Scaler = Retained<ProtocolObject<dyn MTLFXSpatialScaler>>;

pub struct MetalFxSpatial {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    /// the scaler for (input w, h, output w, h, input format, output format); rebuilt when one changes
    scaler: Option<((u32, u32, u32, u32, wgpu::TextureFormat, wgpu::TextureFormat), Scaler)>,
}

// the Core (and with it this) moves to the render thread once, before any frame; the Metal objects are thread-safe
unsafe impl Send for MetalFxSpatial {}

fn pixel_format(f: wgpu::TextureFormat) -> Option<MTLPixelFormat> {
    match f {
        wgpu::TextureFormat::Bgra8Unorm => Some(MTLPixelFormat::BGRA8Unorm),
        wgpu::TextureFormat::Rgba8Unorm => Some(MTLPixelFormat::RGBA8Unorm),
        _ => None,
    }
}

impl MetalFxSpatial {
    /// `Err(reason)` when the framework class is missing (macOS < 13), the device is not Metal or the device does
    /// not support the scaler.
    pub fn new(device: &wgpu::Device) -> Result<Self, String> {
        // the class lookup first: on a system without MetalFX the descriptor's `class!` would panic
        if AnyClass::get(c"MTLFXSpatialScalerDescriptor").is_none() {
            return Err("MetalFX is not present (macOS older than 13)".to_string());
        }
        let hal = unsafe { device.as_hal::<wgpu::hal::api::Metal>() }.ok_or_else(|| "the device is not a Metal device".to_string())?;
        let raw = hal.raw_device().clone();
        drop(hal);
        if !unsafe { MTLFXSpatialScalerDescriptor::supportsDevice(&raw) } {
            return Err("this GPU does not support the MetalFX spatial scaler".to_string());
        }
        Ok(Self { device: raw, scaler: None })
    }

    fn scaler_for(&mut self, key: (u32, u32, u32, u32, wgpu::TextureFormat, wgpu::TextureFormat)) -> Option<&Scaler> {
        if self.scaler.as_ref().is_none_or(|(k, _)| *k != key) {
            let (iw, ih, ow, oh, fi, fo) = key;
            let desc = unsafe { MTLFXSpatialScalerDescriptor::new() };
            unsafe {
                desc.setInputWidth(iw as usize);
                desc.setInputHeight(ih as usize);
                desc.setOutputWidth(ow as usize);
                desc.setOutputHeight(oh as usize);
                desc.setColorTextureFormat(pixel_format(fi)?);
                desc.setOutputTextureFormat(pixel_format(fo)?);
                // the scene is the tone-mapped, display-referred 8-bit picture (gamma-encoded)
                desc.setColorProcessingMode(MTLFXSpatialScalerColorProcessingMode::Perceptual);
            }
            let s = unsafe { desc.newSpatialScalerWithDevice(&self.device) }?;
            tracing::info!("MetalFX spatial scaler (ENHANCEMENT E34): {iw}x{ih} -> {ow}x{oh}");
            self.scaler = Some((key, s));
        }
        self.scaler.as_ref().map(|(_, s)| s)
    }
}

impl super::Upscaler for MetalFxSpatial {
    fn name(&self) -> &'static str {
        "MetalFX spatial"
    }

    fn render_scale(&self, level: u8) -> f32 {
        // E34's levels 5..7 (the «simple» spatial ones, after the four temporal ones)
        match level {
            5 => 0.67,
            6 => 0.59,
            _ => 0.5,
        }
    }

    fn output_format(&self) -> wgpu::TextureFormat {
        // a storage-capable format (Bgra8Unorm storage needs a feature); the blit that reads it swizzles for free
        wgpu::TextureFormat::Rgba8Unorm
    }

    fn output_usage(&self) -> wgpu::TextureUsages {
        // MTLTextureUsageShaderWrite (and RenderTarget for good measure: harmless)
        wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT
    }

    fn encode(&mut self, device: &wgpu::Device, input: &wgpu::Texture, output: &wgpu::Texture) -> Option<wgpu::CommandBuffer> {
        let (iw, ih) = (input.width(), input.height());
        let (ow, oh) = (output.width(), output.height());
        let scaler = self.scaler_for((iw, ih, ow, oh, input.format(), output.format()))?.clone();
        let tin = raw_texture(input)?;
        let tout = raw_texture(output)?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("metalfx") });
        let done = unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|e| {
                let cmd = e?.raw_command_buffer()?;
                scaler.setColorTexture(Some(&tin));
                scaler.setOutputTexture(Some(&tout));
                scaler.setInputContentWidth(iw as usize);
                scaler.setInputContentHeight(ih as usize);
                scaler.encodeToCommandBuffer(cmd);
                Some(())
            })
        };
        done?;
        Some(encoder.finish())
    }
}
