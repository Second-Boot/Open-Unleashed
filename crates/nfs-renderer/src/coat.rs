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
/// The alpha between mip `m` and `m + 1` (`MIP_ALPHA[m + 1] - MIP_ALPHA[m]`, written as the shader's `coat_lod` divides by it:
/// the difference of the two `f32`s is not these values to the bit).
pub(crate) const MIP_SPAN: [f32; MIPS as usize - 1] = [0.03, 0.05, 0.12, 0.25];
/// The coat's GGX alpha at an E8 level (1 новый, 2 обычный, 3 потёртый): the mips 1 / 2 / 3 exactly.
pub(crate) fn level_alpha(level: u8) -> f32 {
    match level {
        0 => 0.0,
        1 => MIP_ALPHA[1],
        2 => MIP_ALPHA[2],
        _ => MIP_ALPHA[3],
    }
}
/// Taps of one texel per filtered mip 1..: a narrow lobe (the sharp mips) reads nearly one place of the smooth source, so it needs
/// fewer taps than the wide ones (ENHANCEMENT: the one count of 32 for every mip before).
const SAMPLES: [f32; FILTERED as usize] = [8.0, 16.0, 32.0, 32.0];
/// The least residual alpha squared between two mips (a lobe of no width would divide by zero in the importance sampling).
const MIN_RESIDUAL_SQ: f32 = 1e-6;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    a: [f32; 4],
    b: [f32; 4],
}

/// The pass's shader: `coat.wgsl` with the full-screen triangle and a full turn (`wgsl.rs`) and the cube's face table
/// (`classes.rs`).
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::ANGLES, wgsl::IGN, &crate::classes::faces_wgsl(), include_str!("coat.wgsl")])
}

/// The cube's faces (a layer set of the array: E8's, or E49's own after them).
const FACES: u32 = 6;
/// The filtered mips: 1 .. [`MIPS`].
const FILTERED: u32 = MIPS - 1;

pub(crate) struct CoatFilter {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    sampler: wgpu::Sampler,
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    /// one slot of [`Params`] per (mip 1.., face), `stride` apart (a dynamic offset)
    params: wgpu::Buffer,
    stride: u64,
    /// PERF: the cube size and the faces' `1 / tan(half fov)` the slots hold (they depend on nothing else, so both layer sets
    /// read the same slots and they are written once per cube, not per call: two calls of a frame no longer write the buffer
    /// twice before one submit)
    params_for: Option<(u32, u32)>,
    /// PERF: the views and bind groups of the cube's layer sets, made once per cube (the cube is a new texture when its size or
    /// format changes)
    sets: Vec<LayerSet>,
}

/// One layer set's (the six faces from `base`) views of the cube: the source mip of each filtered mip, bound, and the target of
/// each (filtered mip, face).
struct LayerSet {
    array: wgpu::Texture,
    base: u32,
    /// per filtered mip `m` (1..): the bind group reading mip `m - 1`
    src: Vec<wgpu::BindGroup>,
    /// per (filtered mip, face): mip `m` of layer `base + face`
    dst: Vec<wgpu::TextureView>,
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
        let params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("coat prefilter params"), size: stride * (FILTERED * FACES) as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { module, layout, pipeline_layout, sampler, pipelines: HashMap::new(), params, stride, params_for: None, sets: Vec::new() }
    }

    /// The slot of (filtered mip `m`, face `f`).
    fn slot(m: u32, f: u32) -> u32 {
        (m - 1) * FACES + f
    }

    /// The slots for a cube of `size`² with faces of `1 / tan(half fov)` = `tan_inv`: per mip its size, the lobe's residual width
    /// (the widths add like variances), the face.
    fn write_params(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, size: u32, tan_inv: f32) {
        if self.params_for == Some((size, tan_inv.to_bits())) {
            return;
        }
        let mut data = vec![0u8; (self.stride * (FILTERED * FACES) as u64) as usize];
        for m in 1..MIPS {
            let (alpha, prev) = (MIP_ALPHA[m as usize], MIP_ALPHA[m as usize - 1]);
            let residual = (alpha * alpha - prev * prev).max(MIN_RESIDUAL_SQ).sqrt();
            for f in 0..FACES {
                let p = Params { a: [f as f32, (size >> m).max(1) as f32, residual, tan_inv], b: [SAMPLES[m as usize - 1], 0.0, 0.0, 0.0] };
                let o = (Self::slot(m, f) as u64 * self.stride) as usize;
                data[o..o + std::mem::size_of::<Params>()].copy_from_slice(bytemuck::bytes_of(&p));
            }
        }
        gfx::belt_write(belt, encoder, &self.params, &data);
        self.params_for = Some((size, tan_inv.to_bits()));
    }

    /// The views and bind groups of `array`'s layer set from `base` (made the first time; a new cube drops the old one's).
    fn layer_set(&mut self, device: &wgpu::Device, array: &wgpu::Texture, base: u32) -> usize {
        self.sets.retain(|s| s.array == *array);
        if let Some(i) = self.sets.iter().position(|s| s.base == base) {
            return i;
        }
        let src = (1..MIPS)
            .map(|m| {
                let view = array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), base_mip_level: m - 1, mip_level_count: Some(1), base_array_layer: base, array_layer_count: if base == 0 { None } else { Some(FACES) }, ..Default::default() });
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("coat prefilter"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.params, offset: 0, size: wgpu::BufferSize::new(std::mem::size_of::<Params>() as u64) }) },
                    ],
                })
            })
            .collect();
        let dst = (1..MIPS)
            .flat_map(|m| (0..FACES).map(move |f| (m, f)))
            .map(|(m, f)| array.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_mip_level: m, mip_level_count: Some(1), base_array_layer: base + f, array_layer_count: Some(1), ..Default::default() }))
            .collect();
        self.sets.push(LayerSet { array: array.clone(), base, src, dst });
        self.sets.len() - 1
    }

    /// Filter mips 1.. of the faces in `mask` (bit `i` = face `i`) of `array` (`format`, `size`², [`MIPS`] mips, six layers),
    /// reading mip 0 and the mips filtered before; `tan_inv` = `1 / tan(half fov)` of a face. ENHANCEMENT E49: `base` = the first
    /// of the six layers (6: the player's faces of the twelve-layer cube; 0 = E8's cube as ever).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record(&mut self, device: &wgpu::Device, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, array: &wgpu::Texture, format: wgpu::TextureFormat, size: u32, mask: u8, tan_inv: f32, base: u32) {
        if mask == 0 {
            return;
        }
        self.write_params(belt, encoder, size, tan_inv);
        let set = self.layer_set(device, array, base);
        let (module, pipeline_layout) = (&self.module, &self.pipeline_layout);
        let pipeline = self.pipelines.entry(format).or_insert_with(|| {
            Fullscreen::new("coat prefilter", pipeline_layout, module).build(device, &[gfx::target(format, None)])
        });
        let set = &self.sets[set];
        for m in 1..MIPS {
            for f in 0..FACES {
                if mask & (1 << f) == 0 {
                    continue;
                }
                let slot = Self::slot(m, f);
                let mut pass = gfx::color_pass_untimed(encoder, "coat prefilter", &set.dst[slot as usize], wgpu::LoadOp::Clear(wgpu::Color::BLACK));
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &set.src[m as usize - 1], &[slot * self.stride as u32]);
                pass.draw(0..3, 0..1);
            }
        }
    }
}
