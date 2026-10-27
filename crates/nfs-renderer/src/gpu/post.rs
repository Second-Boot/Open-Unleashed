//! The full-screen passes at the end of the frame: `GammaPass` (the display gamma of the 8-bit path) and `ScaleBlit` (ENHANCEMENT
//! E3 / E7: the scaled image laid over the window, the ACES tone map, the dither, the eye adaptation and the EDR gain), with
//! their WGSL. Moved out of gpu.rs as it was.

use crate::gfx::{self, Fullscreen, RtImage};
use crate::wgsl;
use super::*;

/// ENHANCEMENT E7: the ACES tone map's exposure (a WGSL float literal), the scale of the linear light before the curve.
const ACES_EXPOSURE: &str = "0.75";

/// The display ramp of state 0x2E as a last full-screen pass: `ramp[i] = min(0xFFFF, trunc(i ·
/// 257 · g))` — `dx7z.dll` scales the ramp it read at the mode set, which is the identity `i · 257`
/// on a primary surface nobody has touched — shown as `ramp / 0xFFFF` of full scale. The original
/// needs `DDCAPS2_PRIMARYGAMMA` (`[0x60033FA0]`, caps bit 0x20000, 0x60009115…0x60009123) and
/// otherwise ignores the state; the port, windowed, always applies it.
pub(super) struct GammaPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// the frame's own image, kept while the size holds
    target: Option<(u32, u32, wgpu::Texture, wgpu::TextureView, wgpu::BindGroup)>,
}

const GAMMA_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> g: vec4<f32>;
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(pos.xy), 0);
    let i = round(c.rgb * 255.0);
    let w = min(floor(i * 257.0 * g.x), vec3<f32>(65535.0));
    return vec4<f32>(w / 65535.0, c.a);
}
"#;

impl GammaPass {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = gfx::bgl(device, "gamma", &[gfx::bgl_tex(0, wgpu::TextureSampleType::Float { filterable: false }, false), gfx::bgl_uniform(1, wgpu::ShaderStages::FRAGMENT)]);
        let module = gfx::wgsl_module(device, "gamma", &gamma_source());
        let pl = gfx::pipeline_layout(device, "gamma", &[&layout]);
        let pipeline = Fullscreen::new("gamma", &pl, &module).build(device, &[gfx::target(format, None)]);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("gamma"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { pipeline, layout, uniform, target: None }
    }

    /// The image the frame is drawn into before the ramp, `width × height`.
    pub(super) fn source(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        if !self.target.as_ref().is_some_and(|(w, h, ..)| *w == width && *h == height) {
            let texture = gfx::texture_2d(device, "gamma source", width, height, format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
            let view = texture.create_view(&Default::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gamma"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: self.uniform.as_entire_binding() }],
            });
            self.target = Some((width, height, texture, view, bg));
        }
        self.target.as_ref().map(|(_, _, t, v, _)| (t.clone(), v.clone())).expect("gamma source")
    }

    pub(super) fn resolve(&self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, g: f32) {
        let Some((_, _, _, _, bg)) = self.target.as_ref() else { return };
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&[g, 0.0f32, 0.0, 0.0]));
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = gfx::color_pass(&mut enc, "gamma", "gamma ramp", target, wgpu::LoadOp::Load);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
        crate::prof::resolve(&mut enc);
        queue.submit(Some(enc.finish()));
    }
}

/// ENHANCEMENT E3 / E7: the scaled and / or float 3D image and the full-screen pass that lays it
/// over the window-sized 8-bit image. The sampler is the sprite pass's — bilinear, clamped — for both
/// directions: a render scale above 100 % is a bilinear downscale (at 200 % a 2 x 2 box), below
/// 100 % a bilinear upscale. With `hdr` the image is `Rgba16Float` and the pass is the tone map
/// (`MODE`: 0 = as the original, 1 = ACES); without it the image is the surface's format and the
/// pass a plain blit (`MODE` 0 is the identity).
pub(super) struct ScaleBlit {
    pipeline: wgpu::RenderPipeline,
    /// ENHANCEMENT E7: the tone-map mode of the float image, `None` = an 8-bit image
    pub(super) hdr: Option<u32>,
    /// ENHANCEMENT E21: the tone map also takes the eye adaptation's exposure (group 1)
    pub(super) eye: bool,
    /// ENHANCEMENT E19: the pass adds the interleaved-gradient noise (+-0.5 LSB) before the 8-bit rounding
    pub(super) dither: bool,
    /// ENHANCEMENT E33: the tone map has the second output, the highlight gain map (`edr.rs`)
    pub(super) edr: bool,
    /// the surface's format, the image's when it is not float
    format: wgpu::TextureFormat,
    /// the image, kept while the size holds
    image: Option<RtImage>,
}

const BLIT_WGSL: &str = r#"
// ENHANCEMENT E7: 0 = the identity (the image is the original's frame in its own 8-bit space, held to
// 0..1 by the target), 1 = the ACES filmic curve
override MODE: u32 = 0u;
// ENHANCEMENT E19: 1 = add +-0.5 LSB of interleaved gradient noise (Jimenez 2014) so the float image's
// smooth gradients do not band when rounded to 8 bits; a value already on an 8-bit step (flat colours,
// black, white) is unmoved because the noise stays inside +-0.425 (the half-float image's own error is
// up to 0.06 LSB, so the rounding does not flip)
override DITHER: u32 = 0u;
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
//EYE_BINDINGS
// Narkowicz's fit of the ACES filmic curve
fn aces(x: vec3<f32>) -> vec3<f32> {
    return (x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14);
}
fn dither(c: vec4<f32>, p: vec2<f32>) -> vec4<f32> {
    if DITHER == 0u {
        return c;
    }
    let n = ign(p);
    return vec4<f32>(c.rgb + (n - 0.5) * 0.85 / 255.0, c.a);
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(t, s, in.uv, 0.0);
    //EYE_APPLY
    if MODE == 0u {
        return dither(c, in.pos.xy);
    }
    // the frame's values are the original's display-referred ones: to light (2.2), exposed, through
    // the curve (normalised so that 1.0 stays white), back to display values
    let lin = pow(max(c.rgb, vec3<f32>(0.0)), vec3<f32>(2.2)) * EXPOSURE;
    let mapped = aces(lin) / aces(vec3<f32>(EXPOSURE));
    return dither(vec4<f32>(pow(clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)), c.a), in.pos.xy);
}
"#;

/// The gamma ramp's shader.
pub(crate) fn gamma_source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, GAMMA_WGSL])
}

/// The scale blit's shader, with the eye adaptation's exposure (`eye`) and / or the EDR entry point (`edr`).
pub(crate) fn blit_source(eye: bool, edr: bool) -> String {
    // ENHANCEMENT E21: the eye adaptation's exposure scales the image before the curve (identity at 1)
    // ENHANCEMENT E33: the EDR entry point `fs_edr` (a second output) is appended only for the EDR pass
    let base = wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::IGN, BLIT_WGSL, if edr { crate::edr::TONE_EDR_WGSL } else { "" }]);
    let src = if eye { base.replace("//EYE_BINDINGS", crate::adapt::TONE_BINDINGS).replace("//EYE_APPLY", "c = vec4<f32>(c.rgb * eye_exposure(), c.a);") } else { base };
    let cap = format!("{:?}", crate::edr::CAP);
    src.replace("EXPOSURE", ACES_EXPOSURE).replace("EDR_CAP", &cap).replace("EDR_KNEE", &format!("{:?}", crate::edr::KNEE))
}

impl ScaleBlit {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, layout: &wgpu::BindGroupLayout, hdr: Option<u32>, dither: bool, eye_layout: Option<&wgpu::BindGroupLayout>, edr: bool) -> Self {
        let module = gfx::wgsl_module(device, "scale blit", &blit_source(eye_layout.is_some(), edr));
        let layouts: Vec<&wgpu::BindGroupLayout> = match eye_layout {
            Some(e) => vec![layout, e],
            None => vec![layout],
        };
        let pl = gfx::pipeline_layout(device, "scale blit", &layouts);
        let targets = if edr { vec![gfx::target(format, None), gfx::target(HDR_FORMAT, None)] } else { vec![gfx::target(format, None)] };
        let pipeline = Fullscreen::new("scale blit", &pl, &module)
            .fs(if edr { "fs_edr" } else { "fs" })
            .constants(&[("MODE", hdr.unwrap_or(0) as f64), ("DITHER", dither as u32 as f64)])
            .build(device, &targets);
        Self { pipeline, hdr, eye: eye_layout.is_some(), dither, edr, format, image: None }
    }

    /// The scaled image, `width x height`.
    pub(super) fn target(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let format = if self.hdr.is_some() { HDR_FORMAT } else { self.format };
        let i = RtImage::ensure(&mut self.image, device, "scaled 3d", layout, sampler, width, height, format, gfx::RT_USAGE | wgpu::TextureUsages::COPY_SRC);
        (i.tex.clone(), i.view.clone())
    }

    /// ENHANCEMENT E10: the image's bind group (a source for the bloom's bright pass)
    pub(super) fn image_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.image.as_ref().map(|i| &i.bg)
    }

    /// The scaled image over all of `dest`.
    /// ENHANCEMENT E27: `source` = an image other than the blit's own (the motion-blurred one)
    /// ENHANCEMENT E33: `gain` = the highlight gain map, the pass's second output (an EDR blit only)
    pub(super) fn draw(&self, encoder: &mut wgpu::CommandEncoder, dest: &wgpu::TextureView, eye: Option<&wgpu::BindGroup>, source: Option<&wgpu::BindGroup>, gain: Option<&wgpu::TextureView>) {
        // ENHANCEMENT E34: the plain present blit has no image of its own, only a `source`
        let Some(bg) = source.or(self.image.as_ref().map(|i| &i.bg)) else { return };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scale blit"),
            color_attachments: &if let (true, Some(g)) = (self.edr, gain) {
                vec![
                    Some(wgpu::RenderPassColorAttachment { view: dest, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } }),
                    Some(wgpu::RenderPassColorAttachment { view: g, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } }),
                ]
            } else {
                vec![Some(wgpu::RenderPassColorAttachment { view: dest, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })]
            },
            depth_stencil_attachment: None,
            timestamp_writes: crate::prof::rp("tone map blit"),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bg, &[]);
        if let Some(e) = eye {
            pass.set_bind_group(1, e, &[]);
        }
        pass.draw(0..3, 0..1);
    }
}

