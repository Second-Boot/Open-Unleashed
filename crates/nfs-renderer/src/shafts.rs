//! ENHANCEMENT E13 — light shafts (god rays): a screen-space radial blur of the sky mask toward the
//! sun's screen position (GPU Gems 3 ch. 13), added into the float scene before the tone map.
//!
//! The mask is the 3D pass's depth: a pixel at the far value is sky (the sky stage draws without a
//! depth test or write). Along the ray from a pixel to the sun every tap that lands on sky adds
//! light by how close it is to the sun, so a ray appears only where the sky is open between the
//! pixel and the sun — through the trees, past a cliff, at a tunnel mouth — and a hill that hides
//! the sun lets none through. The pass is additive over the whole frame, sky included: the sky takes
//! light only in proportion to what stands between it and the sun, so the beams show in the air.

use crate::gfx::{self, Fullscreen, Keyed, OverScene};
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

const WGSL: &str = r#"
struct U { sun: vec2<f32>, strength: f32, near: f32, tint: vec4<f32> };
@group(0) @binding(2) var<uniform> u: U;
@group(0) @binding(3) var rays: texture_2d<f32>;
@group(0) @binding(4) var rays_s: sampler;

// 1 where the depth is the sky's (nothing drawn), else 0
fn sky(p: vec2<i32>) -> f32 {
    return select(0.0, 1.0, depth_at(p) >= 0.99999);
}

const TAPS: i32 = 40;
// how much of the way to the sun a ray covers, the per-tap decay, the radius of the sun's glow
// (screen heights) and the overall gain
const DENSITY: f32 = 0.95;
const DECAY: f32 = 0.975;
const GLOW: f32 = 0.3;
const GAIN: f32 = 0.42;

// PERF: the rays are computed at half size (this pass), into an image the composite pass adds over the scene
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    // the centre of the 2 x 2 full-size pixels this half-size pixel stands for
    let fp = pos.xy * 2.0;
    let uv = fp / dims;
    let aspect = dims.x / dims.y;
    let step = (u.sun - uv) * (DENSITY / f32(TAPS));
    // a fixed per-pixel offset hides the banding of 40 taps (static: nothing crawls)
    let jit = ign(pos.xy);
    var p = uv + step * jit;
    var w = 1.0;
    var sum = 0.0;
    for (var i = 0; i < TAPS; i++) {
        let q = vec2<i32>(p * dims);
        if p.x >= 0.0 && p.y >= 0.0 && p.x < 1.0 && p.y < 1.0 {
            let d = length((p - u.sun) * vec2<f32>(aspect, 1.0)) / GLOW;
            sum += sky(q) * w / (1.0 + d * d * 3.0);
        }
        w *= DECAY;
        p += step;
    }
    // the rays fade with the pixel's own distance to the sun (a falloff centred on it)
    let dp = length((uv - u.sun) * vec2<f32>(aspect, 1.0)) / 0.5;
    let l = sum / f32(TAPS) * GAIN * u.strength / (1.0 + dp * dp * 2.0);
    return vec4<f32>(u.tint.rgb * l, 0.0);
}

// the composite: the half-size rays, bilinear, over the full-size scene, but not on the cockpit's own panels and
// pillars (closer than 1.5 m to the eye), which take no ray
@fragment fn fs_comp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let far = u.tint.w;
    let z = u.near * far / max(far - depth_at(vec2<i32>(pos.xy)) * (far - u.near), 0.0001);
    if z < 1.5 {
        return vec4<f32>(0.0);
    }
    let dims = vec2<f32>(depth_dims());
    return vec4<f32>(textureSampleLevel(rays, rays_s, pos.xy / dims, 0.0).rgb, 0.0);
}
"#;

/// The pass's shader for a depth `kind`.
pub(crate) fn source(kind: DepthKind) -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::IGN, kind.wgsl(), WGSL])
}

/// The rays' image format: the tinted light, half size (no blend, no alpha needed).
const RAYS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct Pipes {
    /// pass 1: the rays at half size into the image
    rays: wgpu::RenderPipeline,
    rays_bgl: wgpu::BindGroupLayout,
    /// pass 2: the image added over the scene at full size
    comp: wgpu::RenderPipeline,
    comp_bgl: wgpu::BindGroupLayout,
}

/// What [`ShaftsPass`]'s bind groups are made of: the pipelines' key, the depth, the rays image.
type ShaftsBgKey = ((DepthKind, u32), wgpu::TextureView, wgpu::TextureView);

/// The pipelines (one per sample count of the target, built when first needed), the half-size image and the uniform.
pub(crate) struct ShaftsPass {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    ubuf: wgpu::Buffer,
    sampler: wgpu::Sampler,
    pipes: std::collections::HashMap<(DepthKind, u32), Pipes>,
    /// the rays' and the composite's bind groups, kept while the pipelines, the depth and the rays image hold
    bgs: Keyed<ShaftsBgKey, (wgpu::BindGroup, wgpu::BindGroup)>,
    /// the half-size rays image of the last full size
    image: Option<((u32, u32), wgpu::Texture, wgpu::TextureView)>,
}

impl ShaftsPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ubuf = gfx::uniform_buf::<Uniform>(device, "shafts");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() });
        Self { device: device.clone(), format, ubuf, sampler, pipes: Default::default(), bgs: Keyed::default(), image: None }
    }

    /// `kind`: what the depth is ([`DepthKind`]); `samples`: the colour's sample count (1 when the scene is already resolved).
    fn build(&self, kind: DepthKind, samples: u32) -> Pipes {
        let module = gfx::wgsl_module(&self.device, "shafts", &source(kind));
        let depth = kind.layout_entry();
        let uniform = gfx::bgl_uniform(2, wgpu::ShaderStages::FRAGMENT);
        let rays_tex = gfx::bgl_tex(3, wgpu::TextureSampleType::Float { filterable: true }, false);
        let rays_samp = gfx::bgl_sampler(4, wgpu::SamplerBindingType::Filtering);
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| gfx::bgl(&self.device, label, entries);
        let rays_bgl = layout("shafts rays", &[depth, uniform]);
        let comp_bgl = layout("shafts composite", &[depth, uniform, rays_tex, rays_samp]);
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let pipe = |label: &str, fs: &str, bgl: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>, samples: u32, mask: wgpu::ColorWrites| {
            let pl = gfx::pipeline_layout(&self.device, label, &[bgl]);
            Fullscreen::new(label, &pl, &module).fs(fs).samples(samples).build(&self.device, &[Some(wgpu::ColorTargetState { format, blend, write_mask: mask })])
        };
        let rays = pipe("shafts rays", "fs", &rays_bgl, RAYS_FORMAT, None, 1, wgpu::ColorWrites::ALL);
        let comp = pipe("shafts composite", "fs_comp", &comp_bgl, self.format, Some(wgpu::BlendState { color: add, alpha: add }), samples, wgpu::ColorWrites::COLOR);
        Pipes { rays, rays_bgl, comp, comp_bgl }
    }

    /// Record the pass into `encoder`: the rays at half size, then added over `at.colour` (loaded; `at.resolve` when it is
    /// multisampled; `at.keep` = store the multisampled samples for a later pass), reading `at.depth`.
    pub(crate) fn draw(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, s: &Shafts, planes: [f32; 2], at: &OverScene) {
        let OverScene { depth, kind, colour, resolve, samples, keep } = *at;
        let samples = samples.max(1);
        let key = (kind, samples);
        if !self.pipes.contains_key(&key) {
            let p = self.build(kind, samples);
            self.pipes.insert(key, p);
        }
        let (w, h) = (depth.texture().width(), depth.texture().height());
        if self.image.as_ref().is_none_or(|i| i.0 != (w, h)) {
            let t = gfx::texture_2d(&self.device, "shafts rays", w.div_ceil(2).max(1), h.div_ceil(2).max(1), RAYS_FORMAT, gfx::RT_USAGE);
            let v = t.create_view(&Default::default());
            self.image = Some(((w, h), t, v));
        }
        let rays_view = &self.image.as_ref().expect("rays image").2;
        let pipes = &self.pipes[&key];
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&Uniform { sun: s.uv, strength: s.strength, near: planes[0], tint: [s.tint[0], s.tint[1], s.tint[2], planes[1]] }));
        let (device, ubuf_res, sampler) = (&self.device, &self.ubuf, &self.sampler);
        let (bg1, bg2) = self.bgs.get((key, depth.clone(), rays_view.clone()), || {
            let depth_entry = wgpu::BindGroupEntry { binding: kind.binding(), resource: wgpu::BindingResource::TextureView(depth) };
            let ubuf = wgpu::BindGroupEntry { binding: 2, resource: ubuf_res.as_entire_binding() };
            let bg1 = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("shafts rays"), layout: &pipes.rays_bgl, entries: &[depth_entry.clone(), ubuf.clone()] });
            let bg2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shafts composite"),
                layout: &pipes.comp_bgl,
                entries: &[depth_entry, ubuf, wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(rays_view) }, wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(sampler) }],
            });
            (bg1, bg2)
        });
        {
            let mut pass = gfx::color_pass(encoder, "light shafts (rays, half size)", "light shafts rays (half)", rays_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
            pass.set_pipeline(&pipes.rays);
            pass.set_bind_group(0, bg1, &[]);
            pass.draw(0..3, 0..1);
        }
        let mut pass = gfx::color_pass_resolve(encoder, "light shafts (composite)", "light shafts composite", colour, resolve, wgpu::LoadOp::Load, if samples > 1 && !keep { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store });
        pass.set_pipeline(&pipes.comp);
        pass.set_bind_group(0, bg2, &[]);
        pass.draw(0..3, 0..1);
    }
}
