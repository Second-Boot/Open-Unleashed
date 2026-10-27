//! ENHANCEMENT ENH-COAT (E8, the clear coat): the prefiltered mip chain of the environment cube.
//!
//! The six faces of E8's cube hold the scene at mip 0 (the app draws them, a few a frame). The paint's clear coat reads the
//! cube by the coat's roughness: mip `m` holds the radiance convolved with a GGX lobe of width [`MIP_ALPHA`]`[m]`, so a
//! lacquer of any of the menu's levels is one trilinear read. Each mip is made from the one above it (a lobe of the
//! residual width, so the widths add like variances), by a fragment pass per (mip, face) — a render pass, so it runs on every
//! backend (no compute / storage textures). Only the faces drawn since the last run are filtered. Cost: ~0.7 M taps a face
//! (see `ENHANCEMENTS.md` E8).

use crate::gfx::{self, Fullscreen};
use crate::wgsl;
use std::collections::HashMap;

/// Mip levels of the cube: 0 the raw faces, 1.. the prefiltered ones.
pub(crate) const MIPS: u32 = 5;
/// The GGX alpha each mip holds (mip 0 is the mirror). The menu's levels are exactly mips 1 / 2 / 3.
pub const MIP_ALPHA: [f32; MIPS as usize] = [0.0, 0.03, 0.08, 0.2, 0.45];
/// The coat's GGX alpha at an E8 level (1 новый, 2 обычный, 3 потёртый): the mips 1 / 2 / 3 exactly.
pub(crate) fn level_alpha(level: u8) -> f32 {
    match level {
        0 => 0.0,
        1 => MIP_ALPHA[1],
        2 => MIP_ALPHA[2],
        _ => MIP_ALPHA[3],
    }
}
/// Taps of one texel.
const SAMPLES: f32 = 32.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    a: [f32; 4],
    b: [f32; 4],
}

/// The pass's shader: `coat.wgsl` with the full-screen triangle (`wgsl.rs`).
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, include_str!("coat.wgsl")])
}

pub(crate) struct CoatFilter {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    params: wgpu::Buffer,
    stride: u64,
}

impl CoatFilter {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = gfx::wgsl_module(device, "coat prefilter", &source());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("coat prefilter"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) },
                    count: None,
                },
            ],
        });
        let pipeline_layout = gfx::pipeline_layout(device, "coat prefilter", &[&layout]);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("coat prefilter"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(std::mem::size_of::<Params>() as u64);
        let params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("coat prefilter params"), size: stride * (MIPS as u64 - 1) * 6, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { module, layout, pipeline_layout, sampler, pipelines: HashMap::new(), params, stride }
    }

    /// Filter mips 1.. of the faces in `mask` (bit `i` = face `i`) of `array` (`format`, `size`², [`MIPS`] mips, six layers),
    /// reading mip 0 and the mips filtered before; `tan_inv` = `1 / tan(half fov)` of a face.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, array: &wgpu::Texture, format: wgpu::TextureFormat, size: u32, mask: u8, tan_inv: f32) {
        if mask == 0 {
            return;
        }
        let (module, pipeline_layout) = (&self.module, &self.pipeline_layout);
        let pipeline = self.pipelines.entry(format).or_insert_with(|| {
            Fullscreen::new("coat prefilter", pipeline_layout, module).build(device, &[gfx::target(format, None)])
        });
        // the slots: one per (mip 1.., face); constant for a cube size
        let mut data = vec![0u8; (self.stride * (MIPS as u64 - 1) * 6) as usize];
        for m in 1..MIPS {
            let (alpha, prev) = (MIP_ALPHA[m as usize], MIP_ALPHA[m as usize - 1]);
            let residual = (alpha * alpha - prev * prev).max(1e-6).sqrt();
            for f in 0..6u32 {
                let p = Params { a: [f as f32, (size >> m).max(1) as f32, residual, tan_inv], b: [SAMPLES, 0.0, 0.0, 0.0] };
                let o = (((m - 1) * 6 + f) as u64 * self.stride) as usize;
                data[o..o + std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(&p));
            }
        }
        queue.write_buffer(&self.params, 0, &data);
        for m in 1..MIPS {
            let src = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), base_mip_level: m - 1, mip_level_count: Some(1), ..Default::default() });
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("coat prefilter"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.params, offset: 0, size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) }) },
                ],
            });
            for f in 0..6u32 {
                if mask & (1 << f) == 0 {
                    continue;
                }
                let dst = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_mip_level: m, mip_level_count: Some(1), base_array_layer: f, array_layer_count: Some(1), ..Default::default() });
                let mut pass = gfx::color_pass_untimed(encoder, "coat prefilter", &dst, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &bg, &[((m - 1) * 6 + f) * self.stride as u32]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
