//! ENHANCEMENT E14 — corner shading: a weak screen-space ambient occlusion (GTAO, `ao.wgsl`) over the
//! opaque part of the race's main 3D pass. It reads the pass's depth (E12's `R32Float` conversion,
//! the nearest MSAA sample), rebuilds the normals from it (no normal pre-pass), and multiplies the
//! opaque colour before the blended stage is drawn.

use crate::gfx::{self, Fullscreen};
use crate::wgsl;
use std::collections::HashMap;

const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// Radius in metres, the largest darkening (0..1), slices, steps per side of a level
/// (`ENHANCEMENTS.md` E14). The fade starts at 25 m and ends at 45 m.
pub(crate) fn level_params(level: u8) -> Option<[f32; 4]> {
    match level {
        0 => None,
        // weak: the track's vertex colours already carry occlusion, so only a light touch
        1 => Some([1.0, 0.30, 2.0, 6.0]),
        _ => Some([1.4, 0.55, 3.0, 8.0]),
    }
}
const FADE: [f32; 2] = [25.0, 45.0];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    cam: [f32; 4],
    ao: [f32; 4],
    n: [f32; 4],
    size: [f32; 4],
    /// the half-resolution images' width, height (PERF: the occlusion and its blur run at half size)
    half: [f32; 4],
}

struct Images {
    size: (u32, u32),
    /// half-resolution ping-pong pair (the raw occlusion, the blur's two steps)
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    /// the full-resolution occlusion the apply pass reads (the depth-aware upsample of `a`)
    up: wgpu::TextureView,
    _t: [wgpu::Texture; 3],
}

/// The pass's shader: `ao.wgsl` with the full-screen triangle (`wgsl.rs`).
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, include_str!("ao.wgsl")])
}

pub(crate) struct Ssao {
    module: wgpu::ShaderModule,
    buf: wgpu::Buffer,
    layout_ao: wgpu::BindGroupLayout,
    layout_blur: wgpu::BindGroupLayout,
    layout_ms: wgpu::BindGroupLayout,
    pipe_ao: wgpu::RenderPipeline,
    pipe_blur: wgpu::RenderPipeline,
    pipe_blur_v: wgpu::RenderPipeline,
    pipe_up: wgpu::RenderPipeline,
    /// the apply pipelines, per (colour format, samples)
    apply: HashMap<(wgpu::TextureFormat, u32), wgpu::RenderPipeline>,
    images: Option<Images>,
}

fn view(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(v) }
}
fn tex(binding: u32) -> wgpu::BindGroupLayoutEntry {
    gfx::bgl_tex(binding, wgpu::TextureSampleType::Float { filterable: false }, false)
}

impl Ssao {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = gfx::wgsl_module(device, "ao", &source());
        let uniform = gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT);
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| gfx::bgl(device, label, entries);
        let layout_ao = layout("ao", &[uniform, tex(1)]);
        let layout_blur = layout("ao blur", &[uniform, tex(1), tex(2)]);
        let ms_depth = gfx::bgl_tex(3, wgpu::TextureSampleType::Depth, true);
        let layout_ms = layout("ao apply ms", &[uniform, tex(1), tex(2), ms_depth]);
        let buf = gfx::uniform_buf::<Params>(device, "ao params");
        let pipe_ao = Self::pipeline(device, &module, &layout_ao, "fs_ao", AO_FORMAT, 1, None);
        let pipe_blur = Self::pipeline(device, &module, &layout_blur, "fs_blur", AO_FORMAT, 1, None);
        let pipe_blur_v = Self::pipeline(device, &module, &layout_blur, "fs_blur_v", AO_FORMAT, 1, None);
        let pipe_up = Self::pipeline(device, &module, &layout_blur, "fs_up", AO_FORMAT, 1, None);
        Self { module, buf, layout_ao, layout_blur, layout_ms, pipe_ao, pipe_blur, pipe_blur_v, pipe_up, apply: HashMap::new(), images: None }
    }

    fn pipeline(device: &wgpu::Device, module: &wgpu::ShaderModule, bgl: &wgpu::BindGroupLayout, fs: &str, format: wgpu::TextureFormat, samples: u32, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
        let pl = gfx::pipeline_layout(device, fs, &[bgl]);
        Fullscreen::new(fs, &pl, module).fs(fs).samples(samples).build(device, &[gfx::target(format, blend)])
    }

    /// Record the occlusion passes and the multiply over `colour` (`format`, `samples`; the
    /// multisampled colour of the pass when MSAA is on). `soft` is the converted depth image
    /// (`SoftDepth::convert`), `depth_ms` the pass's multisampled depth when `samples > 1`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, level: u8, camera: &crate::Camera, aspect: f32, colour: &wgpu::TextureView, format: wgpu::TextureFormat, samples: u32, soft: &wgpu::TextureView, depth_ms: Option<&wgpu::TextureView>, size: (u32, u32)) {
        let Some([radius, strength, slices, steps]) = level_params(level) else { return };
        if self.images.as_ref().is_none_or(|i| i.size != size) {
            let make = |label: &str, w: u32, h: u32| {
                let t = gfx::texture_2d(device, label, w.max(1), h.max(1), AO_FORMAT, gfx::RT_USAGE);
                (t.create_view(&Default::default()), t)
            };
            let (hw, hh) = (size.0.div_ceil(2), size.1.div_ceil(2));
            let (a, t0) = make("ao half a", hw, hh);
            let (b, t1) = make("ao half b", hw, hh);
            let (up, t2) = make("ao full", size.0, size.1);
            self.images = Some(Images { size, a, b, up, _t: [t0, t1, t2] });
        }
        let images = self.images.as_ref().expect("ao images");
        let tan_y = (0.5 * camera.fov_y_radians).tan();
        let params = Params {
            cam: [tan_y * aspect, tan_y, camera.near, camera.far],
            ao: [radius, strength, FADE[0], FADE[1]],
            n: [slices, steps, 1.5, 0.0],
            size: [size.0 as f32, size.1 as f32, 1.0 / size.0 as f32, 1.0 / size.1 as f32],
            half: [size.0.div_ceil(2) as f32, size.1.div_ceil(2) as f32, 0.0, 0.0],
        };
        queue.write_buffer(&self.buf, 0, bytemuck::bytes_of(&params));
        let bind = |layout: &wgpu::BindGroupLayout, extra: &[wgpu::BindGroupEntry]| {
            let mut e = vec![wgpu::BindGroupEntry { binding: 0, resource: self.buf.as_entire_binding() }];
            e.extend_from_slice(extra);
            device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("ao"), layout, entries: &e })
        };
        let full = |encoder: &mut wgpu::CommandEncoder, label: &'static str, target: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, bg: &wgpu::BindGroup, load: wgpu::LoadOp<wgpu::Color>| {
            let mut pass = gfx::color_pass(encoder, label, label, target, load);
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        };
        // PERF: the occlusion and its two blur steps at half size, then one depth-aware upsample to full size
        let bg = bind(&self.layout_ao, &[view(1, soft)]);
        full(encoder, "ao pass (half)", &images.a, &self.pipe_ao, &bg, wgpu::LoadOp::Clear(wgpu::Color::WHITE));
        let bg = bind(&self.layout_blur, &[view(1, soft), view(2, &images.a)]);
        full(encoder, "ao blur h (half)", &images.b, &self.pipe_blur, &bg, wgpu::LoadOp::Clear(wgpu::Color::WHITE));
        let bg = bind(&self.layout_blur, &[view(1, soft), view(2, &images.b)]);
        full(encoder, "ao blur v (half)", &images.a, &self.pipe_blur_v, &bg, wgpu::LoadOp::Clear(wgpu::Color::WHITE));
        let bg = bind(&self.layout_blur, &[view(1, soft), view(2, &images.a)]);
        full(encoder, "ao upsample", &images.up, &self.pipe_up, &bg, wgpu::LoadOp::Clear(wgpu::Color::WHITE));
        // multiply: result = source * destination, the destination's alpha kept
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Dst, dst_factor: wgpu::BlendFactor::Zero, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let ms = samples > 1;
        let (module, layout_ms, layout_blur) = (&self.module, &self.layout_ms, &self.layout_blur);
        let layout = if ms { layout_ms } else { layout_blur };
        let pipe = self.apply.entry((format, samples)).or_insert_with(|| Self::pipeline(device, module, layout, if ms { "fs_apply_ms" } else { "fs_apply_1" }, format, samples, Some(multiply)));
        let bg = match depth_ms.filter(|_| ms) {
            Some(d) => bind(&self.layout_ms, &[view(1, soft), view(2, &images.up), view(3, d)]),
            None => bind(&self.layout_blur, &[view(1, soft), view(2, &images.up)]),
        };
        full(encoder, "ao apply pass", colour, pipe, &bg, wgpu::LoadOp::Load);
    }
}
