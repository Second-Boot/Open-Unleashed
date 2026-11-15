//! ENHANCEMENT E14 — corner shading: a weak screen-space ambient occlusion (GTAO, `ao.wgsl`) over the
//! opaque part of the race's main 3D pass. It reads the pass's depth (E12's `R32Float` conversion,
//! the nearest MSAA sample), rebuilds the normals from it (no normal pre-pass), and multiplies the
//! opaque colour before the blended stage is drawn. Four passes: the occlusion, its horizontal and
//! vertical blur (half size) and the multiply, which does the depth-aware upsample itself.

use crate::gfx::{self, Fullscreen, Keyed, PostTarget, UniformBuf, Variants};
use crate::wgsl;

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
/// The occlusion's power (`ao.wgsl` `p.n.z`).
const POWER: f32 = 1.5;

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

/// The half-resolution ping-pong pair (the raw occlusion, the blur's two steps); the apply pass reads `a` and upsamples it itself.
#[derive(Default)]
struct Images {
    a: Option<PostTarget>,
    b: Option<PostTarget>,
}

/// What the four passes' bind groups are made of: the converted depth, the occlusion image (its identity), the multisampled depth.
type BgKey = (wgpu::TextureView, wgpu::TextureView, Option<wgpu::TextureView>);

/// The four passes' bind groups, in order: the occlusion, the two blurs, the apply.
type Bgs = [wgpu::BindGroup; 4];

/// The pass's shader: `ao.wgsl` with the full-screen triangle (`wgsl.rs`).
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::LINEAR_Z, include_str!("ao.wgsl")])
}

pub(crate) struct Ssao {
    module: wgpu::ShaderModule,
    buf: UniformBuf<Params>,
    layout_ao: wgpu::BindGroupLayout,
    layout_blur: wgpu::BindGroupLayout,
    layout_ms: wgpu::BindGroupLayout,
    pipe_ao: wgpu::RenderPipeline,
    pipe_blur: wgpu::RenderPipeline,
    pipe_blur_v: wgpu::RenderPipeline,
    /// the apply pipelines, per (colour format, samples)
    apply: Variants<(wgpu::TextureFormat, u32), wgpu::RenderPipeline>,
    images: Images,
    /// the passes' bind groups, kept while the images they bind hold
    bgs: Keyed<BgKey, Bgs>,
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
        let buf = UniformBuf::new(device, "ao params");
        let pipe_ao = Self::pipeline(device, &module, &layout_ao, "fs_ao", AO_FORMAT, 1, None);
        let pipe_blur = Self::pipeline(device, &module, &layout_blur, "fs_blur", AO_FORMAT, 1, None);
        let pipe_blur_v = Self::pipeline(device, &module, &layout_blur, "fs_blur_v", AO_FORMAT, 1, None);
        Self { module, buf, layout_ao, layout_blur, layout_ms, pipe_ao, pipe_blur, pipe_blur_v, apply: Variants::default(), images: Images::default(), bgs: Keyed::default() }
    }

    fn pipeline(device: &wgpu::Device, module: &wgpu::ShaderModule, bgl: &wgpu::BindGroupLayout, fs: &str, format: wgpu::TextureFormat, samples: u32, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
        let pl = gfx::pipeline_layout(device, fs, &[bgl]);
        Fullscreen::new(fs, &pl, module).fs(fs).samples(samples).build(device, &[gfx::target(format, blend)])
    }

    /// Record the occlusion passes and the multiply over `colour` (`format`, `samples`; the
    /// multisampled colour of the pass when MSAA is on). `soft` is the converted depth image
    /// (`SoftDepth::convert`), `depth_ms` the pass's multisampled depth when `samples > 1`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn run(&mut self, device: &wgpu::Device, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, level: u8, camera: &crate::Camera, aspect: f32, colour: &wgpu::TextureView, format: wgpu::TextureFormat, samples: u32, soft: &wgpu::TextureView, depth_ms: Option<&wgpu::TextureView>, size: (u32, u32)) {
        let Some([radius, strength, slices, steps]) = level_params(level) else { return };
        let half = |slot: &mut Option<PostTarget>, label| PostTarget::ensure(slot, device, label, size, PostTarget::HALF, AO_FORMAT).view.clone();
        let (a, b) = (half(&mut self.images.a, "ao half a"), half(&mut self.images.b, "ao half b"));
        let tan_y = (0.5 * camera.fov_y_radians).tan();
        let params = Params {
            cam: [tan_y * aspect, tan_y, camera.near, camera.far],
            ao: [radius, strength, FADE[0], FADE[1]],
            n: [slices, steps, POWER, 0.0],
            size: [size.0 as f32, size.1 as f32, 1.0 / size.0 as f32, 1.0 / size.1 as f32],
            half: [size.0.div_ceil(2) as f32, size.1.div_ceil(2) as f32, 0.0, 0.0],
        };
        self.buf.write(belt, encoder, &params);
        let ms = samples > 1;
        let (module, layout_ms, layout_blur) = (&self.module, &self.layout_ms, &self.layout_blur);
        let pipe = self.apply.get((format, samples), || Self::pipeline(device, module, if ms { layout_ms } else { layout_blur }, if ms { "fs_apply_ms" } else { "fs_apply_1" }, format, samples, Some(gfx::blend::MULTIPLY_KEEP_ALPHA)));
        let depth_ms = depth_ms.filter(|_| ms);
        let (buf, layout_ao) = (&self.buf.buf, &self.layout_ao);
        let bgs = self.bgs.get((soft.clone(), a.clone(), depth_ms.cloned()), || {
            let bind = |layout: &wgpu::BindGroupLayout, extra: &[wgpu::BindGroupEntry]| {
                let mut e = vec![wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }, view(1, soft)];
                e.extend_from_slice(extra);
                device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("ao"), layout, entries: &e })
            };
            [
                bind(layout_ao, &[]),
                bind(layout_blur, &[view(2, &a)]),
                bind(layout_blur, &[view(2, &b)]),
                match depth_ms {
                    Some(d) => bind(layout_ms, &[view(2, &a), view(3, d)]),
                    None => bind(layout_blur, &[view(2, &a)]),
                },
            ]
        });
        let white = wgpu::LoadOp::Clear(wgpu::Color::WHITE);
        // PERF: the occlusion and its two blur steps at half size, then the apply, which upsamples (depth-aware) to full size itself
        gfx::fullscreen_pass(encoder, "ao pass (half)", "ao pass (half)", &a, white, &self.pipe_ao, &[&bgs[0]]);
        gfx::fullscreen_pass(encoder, "ao blur h (half)", "ao blur h (half)", &b, white, &self.pipe_blur, &[&bgs[1]]);
        gfx::fullscreen_pass(encoder, "ao blur v (half)", "ao blur v (half)", &a, white, &self.pipe_blur_v, &[&bgs[2]]);
        // multiply: result = source * destination, the destination's alpha kept
        gfx::fullscreen_pass(encoder, "ao apply pass", "ao apply pass", colour, wgpu::LoadOp::Load, pipe, &[&bgs[3]]);
    }
}
