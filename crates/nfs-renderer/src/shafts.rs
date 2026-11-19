//! ENHANCEMENT E13 — light shafts (god rays): a screen-space radial blur of the sky mask toward the
//! sun's screen position (GPU Gems 3 ch. 13), added into the float scene before the tone map.
//!
//! The mask is the 3D pass's depth: a pixel at the far value is sky (the sky stage draws without a
//! depth test or write). Along the ray from a pixel to the sun every tap that lands on sky adds
//! light by how close it is to the sun, so a ray appears only where the sky is open between the
//! pixel and the sun — through the trees, past a cliff, at a tunnel mouth — and a hill that hides
//! the sun lets none through. The pass is additive over the whole frame, sky included: the sky takes
//! light only in proportion to what stands between it and the sun, so the beams show in the air.

use crate::gfx::{self, Keyed, OverScene, PostTarget, RaysLabels, RaysPipes, UniformBuf, Variants};
use crate::wgsl::{self, DepthKind};

/// The sun's screen position and strength for the pass (`Scene::shafts`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shafts {
    /// the sun's point in the pass's image, 0..1, y down (it may lie outside: the sun off the screen)
    pub uv: [f32; 2],
    /// 0..1: the app's fade (the sun's distance beyond the screen edge)
    pub strength: f32,
    /// the light's colour (display-referred values, 0..1)
    pub tint: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    sun: [f32; 2],
    strength: f32,
    near: f32,
    /// rgb = tint, w = the pass's far plane
    tint: [f32; 4],
}

const WGSL: &str = include_str!("shafts.wgsl");

/// The pass's shader for a depth `kind`.
pub(crate) fn source(kind: DepthKind) -> String {
    wgsl::compose_scene_depth(kind, &[WGSL])
}

type Pipes = RaysPipes;

const LABELS: RaysLabels = RaysLabels { rays_pass: "light shafts (rays, half size)", rays_prof: "light shafts rays (half)", comp_pass: "light shafts (composite)", comp_prof: "light shafts composite" };

/// What [`ShaftsPass`]'s bind groups are made of: the pipelines' key, the depth, the rays image.
type ShaftsBgKey = ((DepthKind, u32), wgpu::TextureView, wgpu::TextureView);

/// The pipelines (one per sample count of the target, built when first needed), the half-size image and the uniform.
pub(crate) struct ShaftsPass {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    ubuf: UniformBuf<Uniform>,
    sampler: wgpu::Sampler,
    pipes: Variants<(DepthKind, u32), Pipes>,
    /// the rays' and the composite's bind groups, kept while the pipelines, the depth and the rays image hold
    bgs: Keyed<ShaftsBgKey, (wgpu::BindGroup, wgpu::BindGroup)>,
    /// the half-size rays image of the last full size
    image: Option<PostTarget>,
}

impl ShaftsPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ubuf = UniformBuf::new(device, "shafts");
        let sampler = gfx::linear_clamp_sampler(device, None);
        Self { device: device.clone(), format, ubuf, sampler, pipes: Variants::default(), bgs: Keyed::default(), image: None }
    }

    /// `kind`: what the depth is ([`DepthKind`]); `samples`: the colour's sample count (1 when the scene is already resolved).
    fn build(device: &wgpu::Device, format: wgpu::TextureFormat, kind: DepthKind, samples: u32) -> Pipes {
        let module = gfx::wgsl_module(device, "shafts", &source(kind));
        let depth = kind.layout_entry();
        let uniform = gfx::bgl_uniform(2, wgpu::ShaderStages::FRAGMENT);
        let rays_tex = gfx::bgl_float(3, true);
        let rays_samp = gfx::bgl_sampler(4, wgpu::SamplerBindingType::Filtering);
        let rays_bgl = gfx::bgl(device, "shafts rays", &[depth, uniform]);
        let comp_bgl = gfx::bgl(device, "shafts composite", &[depth, uniform, rays_tex, rays_samp]);
        RaysPipes::build(device, &module, "shafts", rays_bgl, comp_bgl, format, samples)
    }

    /// Record the pass into `encoder`: the rays at half size, then added over `at.colour` (loaded; `at.resolve` when it is
    /// multisampled; `at.keep` = store the multisampled samples for a later pass), reading `at.depth`.
    pub(crate) fn draw(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, s: &Shafts, planes: [f32; 2], at: &OverScene) {
        let OverScene { depth, kind, .. } = *at;
        let samples = at.samples.max(1);
        let key = (kind, samples);
        let (device, format) = (&self.device, self.format);
        let pipes = self.pipes.get(key, || Self::build(device, format, kind, samples));
        let size = (depth.texture().width(), depth.texture().height());
        let rays_view = &PostTarget::ensure(&mut self.image, device, "shafts rays", size, PostTarget::HALF, gfx::RAYS_FORMAT).view;
        self.ubuf.write(belt, encoder, &Uniform { sun: s.uv, strength: s.strength, near: planes[0], tint: [s.tint[0], s.tint[1], s.tint[2], planes[1]] });
        let (ubuf_res, sampler) = (&self.ubuf.buf, &self.sampler);
        let (bg1, bg2) = self.bgs.get((key, depth.clone(), rays_view.clone()), || {
            let depth_entry = gfx::bg_view(kind.binding(), depth);
            let ubuf = gfx::bg_buf(2, ubuf_res);
            let bg1 = gfx::bind_group(device, "shafts rays", &pipes.rays_bgl, &[depth_entry.clone(), ubuf.clone()]);
            let bg2 = gfx::bind_group(device, "shafts composite", &pipes.comp_bgl, &[depth_entry, ubuf, gfx::bg_view(3, rays_view), gfx::bg_sampler(4, sampler)]);
            (bg1, bg2)
        });
        pipes.record(encoder, LABELS, rays_view, at, bg1, bg2);
    }
}
