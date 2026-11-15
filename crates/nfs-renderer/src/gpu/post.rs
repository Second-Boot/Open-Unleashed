//! The full-screen passes at the end of the frame: `GammaPass` (the display gamma of the 8-bit path) and `ScaleBlit` (ENHANCEMENT
//! E3 / E7: the scaled image laid over the window, the ACES tone map, the dither, the eye adaptation and the EDR gain), with
//! their WGSL. Moved out of gpu.rs as it was.

use crate::gfx::{self, Fullscreen, RtImage};
use crate::wgsl;
use super::*;

/// ENHANCEMENT E7: the ACES tone map's exposure, the scale of the linear light before the curve (written into the WGSL as
/// its float literal by [`with_tone`]).
const ACES_EXPOSURE: f32 = 0.75;

/// ENHANCEMENT E7: the marker line a tone-map entry point (`fs`, E33's `fs_edr`) holds where [`with_tone`] puts the tone map
/// proper: `c` (the display-referred colour) to `mapped` (exposed linear light through the curve, normalised so that 1.0 stays
/// white).
const TONE_MARK: &str = "//TONE";

/// ENHANCEMENT E7: `src` with every [`TONE_MARK`] line replaced by the tone map's two statements at that line's indent: one
/// text for both entry points. The emitted source is the hand-written one's, byte for byte.
fn with_tone(src: &str) -> String {
    let e = format!("{ACES_EXPOSURE:?}");
    src.split('\n')
        .map(|line| match line.trim_start() {
            TONE_MARK => {
                let indent = &line[..line.len() - TONE_MARK.len()];
                format!("{indent}let lin = pow(max(c.rgb, vec3<f32>(0.0)), vec3<f32>(DISPLAY_GAMMA)) * {e};\n{indent}let mapped = aces(lin) / aces(vec3<f32>({e}));")
            }
            _ => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
    target: Option<GammaTarget>,
}

/// The image the frame is drawn into before the ramp, and the ramp pass's bind group over it (and the uniform).
struct GammaTarget {
    size: (u32, u32),
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    bg: wgpu::BindGroup,
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
        let uniform = gfx::uniform_buf::<[f32; 4]>(device, "gamma");
        Self { pipeline, layout, uniform, target: None }
    }

    /// The image the frame is drawn into before the ramp, `width × height`.
    pub(super) fn source(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        if self.target.as_ref().is_none_or(|t| t.size != (width, height)) {
            let tex = gfx::texture_2d(device, "gamma source", width, height, format, gfx::RT_USAGE | wgpu::TextureUsages::COPY_SRC);
            let view = tex.create_view(&Default::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gamma"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: self.uniform.as_entire_binding() }],
            });
            self.target = Some(GammaTarget { size: (width, height), tex, view, bg });
        }
        self.target.as_ref().map(|t| (t.tex.clone(), t.view.clone())).expect("gamma source")
    }

    /// The ramp over the frame's image into `target`, recorded in the frame's `encoder` (its uniform through the belt).
    pub(super) fn resolve(&self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, g: f32) {
        let Some(GammaTarget { bg, .. }) = self.target.as_ref() else { return };
        gfx::belt_write(belt, encoder, &self.uniform, bytemuck::cast_slice(&[g, 0.0f32, 0.0, 0.0]));
        gfx::fullscreen_pass(encoder, "gamma", "gamma ramp", target, wgpu::LoadOp::Load, &self.pipeline, &[bg]);
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
    /// what the pipeline is built for (a blit of another key is made anew)
    pub(super) key: ToneKey,
    /// the surface's format, the image's when it is not float
    format: wgpu::TextureFormat,
    /// the image, kept while the size holds
    image: Option<RtImage>,
}

/// What a [`ScaleBlit`]'s pipeline is built for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct ToneKey {
    /// ENHANCEMENT E7: the tone-map mode of the float image, `None` = an 8-bit image
    pub hdr: Option<u32>,
    /// ENHANCEMENT E19: the pass adds the interleaved-gradient noise (+-0.5 LSB) before the 8-bit rounding
    pub dither: bool,
    /// ENHANCEMENT E21: the tone map also takes the eye adaptation's exposure (group 1)
    pub eye: bool,
    /// ENHANCEMENT E33: the tone map has the second output, the highlight gain map (`edr.rs`)
    pub edr: bool,
    /// ENHANCEMENT E10: the tone map adds the bloom's last level (`bloom::TONE_WGSL`, the group after the eye's): its level and
    /// E28's dirt
    pub bloom: Option<(u8, bool)>,
}

impl ToneKey {
    /// The plain blit of an 8-bit image (E34's present blit).
    pub const PLAIN: Self = Self { hdr: None, dither: false, eye: false, edr: false, bloom: None };

    /// How many groups the tone map takes after the image's (group 0): the eye's, the bloom's.
    pub fn groups(self) -> usize {
        usize::from(self.eye) + usize::from(self.bloom.is_some())
    }

    /// The tone map of a float image in `mode`, nothing else (the rear-view mirror's).
    pub const fn float(mode: u32) -> Self {
        Self { hdr: Some(mode), ..Self::PLAIN }
    }
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
// Narkowicz's fit of the ACES filmic curve: `x (A x + B) / (x (C x + D) + E)`
const ACES_A: f32 = 2.51;
const ACES_B: f32 = 0.03;
const ACES_C: f32 = 2.43;
const ACES_D: f32 = 0.59;
const ACES_E: f32 = 0.14;
fn aces(x: vec3<f32>) -> vec3<f32> {
    return (x * (ACES_A * x + ACES_B)) / (x * (ACES_C * x + ACES_D) + ACES_E);
}
// ENHANCEMENT E19: the noise's span in LSB (inside +-0.425: a value on an 8-bit step is never moved), an 8-bit step's scale
const DITHER_LSB: f32 = 0.85;
const BYTE: f32 = 255.0;
// the display gamma the frame's values are encoded with (to light and back)
const DISPLAY_GAMMA: f32 = 2.2;
fn dither(c: vec4<f32>, p: vec2<f32>) -> vec4<f32> {
    if DITHER == 0u {
        return c;
    }
    let n = ign(p);
    return vec4<f32>(c.rgb + (n - 0.5) * DITHER_LSB / BYTE, c.a);
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(t, s, in.uv, 0.0);
    //EYE_APPLY
    if MODE == 0u {
        return dither(c, in.pos.xy);
    }
    // the frame's values are the original's display-referred ones: to light (2.2), exposed, through
    // the curve (normalised so that 1.0 stays white), back to display values
    //TONE
    return dither(vec4<f32>(pow(clamp(mapped, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / DISPLAY_GAMMA)), c.a), in.pos.xy);
}
"#;

/// The gamma ramp's shader.
pub(crate) fn gamma_source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, GAMMA_WGSL])
}

/// The scale blit's shader, with the eye adaptation's exposure (`eye`), the EDR entry point (`edr`) and / or the bloom's last
/// add (`bloom`).
pub(crate) fn blit_source(eye: bool, edr: bool, bloom: bool) -> String {
    // ENHANCEMENT E21: the eye adaptation's exposure scales the image before the curve (identity at 1)
    // ENHANCEMENT E33: the EDR entry point `fs_edr` (a second output) is appended only for the EDR pass
    // ENHANCEMENT E10 (PERF): the bloom's last add, in the group after the eye's, onto the image before the exposure (both entry
    // points' `//EYE_APPLY` line is where the image is first read)
    let tone = if bloom { crate::bloom::tone_source(1 + u32::from(eye)) } else { String::new() };
    let base = wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::IGN, &tone, BLIT_WGSL, if edr { crate::edr::TONE_EDR_WGSL } else { "" }]);
    let bloom_apply = if bloom { "c = vec4<f32>(c.rgb + bloom_add(in.uv), c.a);\n    " } else { "" };
    let eye_apply = if eye { "c = vec4<f32>(c.rgb * eye_exposure(), c.a);" } else { "" };
    let base = if eye { base.replace("//EYE_BINDINGS", crate::adapt::TONE_BINDINGS) } else { base };
    let src = if eye || bloom { base.replace("//EYE_APPLY", &format!("{bloom_apply}{eye_apply}")) } else { base };
    with_tone(&src).replace("EDR_CAP", &format!("{:?}", crate::edr::CAP)).replace("EDR_KNEE", &format!("{:?}", crate::edr::KNEE))
}

impl ScaleBlit {
    /// `eye_layout`: the eye adaptation's group (`key.eye`); `bloom_layout`: ENHANCEMENT E10's last add's (`key.bloom`).
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat, layout: &wgpu::BindGroupLayout, key: ToneKey, eye_layout: Option<&wgpu::BindGroupLayout>, bloom_layout: Option<&wgpu::BindGroupLayout>) -> Self {
        debug_assert_eq!(key.eye, eye_layout.is_some(), "the eye adaptation's group comes with its key");
        debug_assert_eq!(key.bloom.is_some(), bloom_layout.is_some(), "the bloom's group comes with its key");
        let ToneKey { hdr, dither, edr, .. } = key;
        let module = gfx::wgsl_module(device, "scale blit", &blit_source(eye_layout.is_some(), edr, bloom_layout.is_some()));
        let layouts: Vec<&wgpu::BindGroupLayout> = std::iter::once(layout).chain(eye_layout).chain(bloom_layout).collect();
        let pl = gfx::pipeline_layout(device, "scale blit", &layouts);
        let targets = if edr { vec![gfx::target(format, None), gfx::target(HDR_FORMAT, None)] } else { vec![gfx::target(format, None)] };
        let mut constants = vec![("MODE", hdr.unwrap_or(0) as f64), ("DITHER", dither as u32 as f64)];
        if let Some((level, dirt)) = key.bloom.filter(|_| bloom_layout.is_some()) {
            constants.extend(crate::bloom::tone_constants(level, dirt));
        }
        let pipeline = Fullscreen::new("scale blit", &pl, &module).fs(if edr { "fs_edr" } else { "fs" }).constants(&constants).build(device, &targets);
        Self { pipeline, key, format, image: None }
    }

    /// The scaled image, `width x height`.
    pub(super) fn target(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let format = if self.key.hdr.is_some() { HDR_FORMAT } else { self.format };
        let i = RtImage::ensure(&mut self.image, device, "scaled 3d", layout, sampler, width, height, format, gfx::RT_USAGE | wgpu::TextureUsages::COPY_SRC);
        (i.tex.clone(), i.view.clone())
    }

    /// ENHANCEMENT E10: the image's bind group (a source for the bloom's bright pass)
    pub(super) fn image_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.image.as_ref().map(|i| &i.bg)
    }

    /// The scaled image over all of `dest`; `groups`: the tone map's groups after the image's ([`ToneKey::groups`]: ENHANCEMENT
    /// E21's eye, E10's bloom).
    /// ENHANCEMENT E27: `source` = an image other than the blit's own (the motion-blurred one)
    /// ENHANCEMENT E33: `gain` = the highlight gain map, the pass's second output (an EDR blit only)
    pub(super) fn draw(&self, encoder: &mut wgpu::CommandEncoder, dest: &wgpu::TextureView, groups: &[&wgpu::BindGroup], source: Option<&wgpu::BindGroup>, gain: Option<&wgpu::TextureView>) {
        self.draw_then(encoder, dest, groups, source, gain, |_| {});
    }

    /// [`Self::draw`], then `after` in the same pass (the frame's 2D over the tone-mapped image); `false` = nothing to draw (no
    /// image, or not the key's groups), the pass not opened and `after` not run.
    pub(super) fn draw_then(&self, encoder: &mut wgpu::CommandEncoder, dest: &wgpu::TextureView, groups: &[&wgpu::BindGroup], source: Option<&wgpu::BindGroup>, gain: Option<&wgpu::TextureView>, after: impl FnOnce(&mut wgpu::RenderPass<'_>)) -> bool {
        // ENHANCEMENT E34: the plain present blit has no image of its own, only a `source`
        let Some(bg) = source.or(self.image.as_ref().map(|i| &i.bg)) else { return false };
        if groups.len() != self.key.groups() {
            debug_assert!(false, "the tone map got {} groups, its key takes {}", groups.len(), self.key.groups());
            return false;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scale blit"),
            color_attachments: &if let (true, Some(g)) = (self.key.edr, gain) {
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
        for (i, g) in (1u32..).zip(groups) {
            pass.set_bind_group(i, *g, &[]);
        }
        pass.draw(0..3, 0..1);
        after(&mut pass);
        true
    }
}

