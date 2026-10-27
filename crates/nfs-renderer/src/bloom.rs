//! ENHANCEMENT E10 — bloom on the float scene (E7), before the tone map.
//!
//! **Not the original.** The original has no bloom: its lamps, the sun and the specular layers are
//! just bright pixels clamped to the 8-bit back buffer. With E7 on the 3D scene sits in an
//! `Rgba16Float` image, so a bright-pass of it can be blurred and added back before the tone-map pass
//! reads it. The chain is Bevy's approach (bevy_core_pipeline `bloom`, MIT / Apache-2.0, after Jimenez,
//! "Next Generation Post Processing in Call of Duty: Advanced Warfare"): a soft-knee bright-pass with
//! a Karis-weighted 13-tap downsample into the first mip (half size), the same 13-tap downsample down
//! a chain of mips, then a 3x3 tent upsample added mip by mip back up (blend One + One), and the
//! last mip added onto the scene with the level's intensity. All of it is plain wgpu / WGSL
//! (`Rgba16Float` render + blend + filter, which `hdr_ok` already checked for E7).
//!
//! The pass runs between the resolved float image and the tone map. The lens-flare probe reads the
//! image before it (inside `encode_frame`), so the probe is untouched; the rear-view mirror, the HUD
//! and every 2D screen never see it.

use crate::gfx::{self, Fullscreen, PostCtx, RtImage};
use crate::wgsl;
use crate::gpu::HDR_FORMAT;

/// Bloom's three numbers for a level: bright-pass threshold and knee (display-referred luminance;
/// 1.0 is the original's white) and the intensity added back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Params {
    pub threshold: f32,
    pub knee: f32,
    pub intensity: f32,
}

/// Level 1 «слабо»: only what is at or above the original's white — lamps, the sun, additive flares
/// and specular overshoots. Level 2 «средне»: a lower bar and a stronger add.
pub(crate) fn params(level: u8) -> Params {
    match level {
        0 => Params { threshold: 1.0, knee: 0.0, intensity: 0.0 },
        1 => Params { threshold: 0.92, knee: 0.08, intensity: 0.25 },
        _ => Params { threshold: 0.82, knee: 0.18, intensity: 0.40 },
    }
}

const WGSL: &str = r#"
// threshold / knee / intensity come in as pipeline constants
override THRESHOLD: f32 = 1.0;
override KNEE: f32 = 0.1;
override INTENSITY: f32 = 1.0;
// ENHANCEMENT E28: the lens dirt's gain on the bloom's last add
override DIRT: f32 = 0.0;
@group(1) @binding(0) var dirt_t: texture_2d<f32>;
@group(1) @binding(1) var dirt_s: sampler;
// ... and the wide mip of the chain the dirt scatters
@group(2) @binding(0) var wide_t: texture_2d<f32>;
@group(2) @binding(1) var wide_s: sampler;
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t, s, uv, 0.0).rgb;
}
fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}
// the soft-knee bright pass (the quadratic knee of Unity's / Bevy's prefilter); also capped so a
// single hot texel cannot flicker the whole chain
fn bright(c0: vec3<f32>) -> vec3<f32> {
    let c = min(max(c0, vec3<f32>(0.0)), vec3<f32>(8.0));
    let l = lum(c);
    var rq = clamp(l - THRESHOLD + KNEE, 0.0, 2.0 * KNEE);
    rq = rq * rq / (4.0 * KNEE + 0.0001);
    let w = max(rq, l - THRESHOLD) / max(l, 0.0001);
    return c * max(w, 0.0);
}
// the 13 taps of the Call of Duty downsample, `first` = with the bright pass and Karis weights
fn down13(uv: vec2<f32>, first: bool) -> vec3<f32> {
    let px = 1.0 / vec2<f32>(textureDimensions(t));
    var o = array<vec2<f32>, 13>(
        vec2<f32>(-2.0, 2.0), vec2<f32>(0.0, 2.0), vec2<f32>(2.0, 2.0),
        vec2<f32>(-2.0, 0.0), vec2<f32>(0.0, 0.0), vec2<f32>(2.0, 0.0),
        vec2<f32>(-2.0, -2.0), vec2<f32>(0.0, -2.0), vec2<f32>(2.0, -2.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0));
    var w = array<f32, 13>(
        0.03125, 0.0625, 0.03125,
        0.0625, 0.125, 0.0625,
        0.03125, 0.0625, 0.03125,
        0.125, 0.125, 0.125, 0.125);
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0; i < 13; i = i + 1) {
        var v = tap(uv + o[i] * px);
        var k = w[i];
        if first {
            v = bright(v);
            k = k / (1.0 + lum(v));
        }
        sum = sum + v * k;
        wsum = wsum + k;
    }
    return sum / max(wsum, 0.00001);
}
@fragment fn fs_down_first(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(down13(in.uv, true), 1.0);
}
@fragment fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(down13(in.uv, false), 1.0);
}
// the 3x3 tent, the source being the smaller mip
@fragment fn fs_up(in: VsOut) -> @location(0) vec4<f32> {
    let px = 1.0 / vec2<f32>(textureDimensions(t));
    var r = tap(in.uv) * 4.0;
    r = r + (tap(in.uv + vec2<f32>(-px.x, 0.0)) + tap(in.uv + vec2<f32>(px.x, 0.0)) + tap(in.uv + vec2<f32>(0.0, -px.y)) + tap(in.uv + vec2<f32>(0.0, px.y))) * 2.0;
    r = r + tap(in.uv + vec2<f32>(-px.x, -px.y)) + tap(in.uv + vec2<f32>(px.x, -px.y)) + tap(in.uv + vec2<f32>(-px.x, px.y)) + tap(in.uv + vec2<f32>(px.x, px.y));
    return vec4<f32>(r / 16.0 * INTENSITY, 0.0);
}
// ENHANCEMENT E28 (ENH-SUNEDGE): the wide mip (1/16 of the scene) read with a cubic B-spline (four bilinear taps,
// GPU Gems 2 ch. 20) instead of one bilinear tap. Bilinear over a 16x magnification is piecewise linear: its slope
// jumps on every texel line, and the dirt multiplies the glow by up to DIRT x INTENSITY (17.5), so round the HDR sun
// those jumps drew straight vertical / horizontal creases (the texel grid) into the dirt's glow. The B-spline is C2.
fn wide_bspline(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(wide_t));
    let p = uv * size - 0.5;
    let i = floor(p);
    let f = p - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    // the two bilinear sample points per axis (texel centres at +0.5)
    let c0 = (i - 0.5 + w1 / g0) / size;
    let c1 = (i + 1.5 + w3 / g1) / size;
    let s00 = textureSampleLevel(wide_t, wide_s, vec2<f32>(c0.x, c0.y), 0.0).rgb;
    let s10 = textureSampleLevel(wide_t, wide_s, vec2<f32>(c1.x, c0.y), 0.0).rgb;
    let s01 = textureSampleLevel(wide_t, wide_s, vec2<f32>(c0.x, c1.y), 0.0).rgb;
    let s11 = textureSampleLevel(wide_t, wide_s, vec2<f32>(c1.x, c1.y), 0.0).rgb;
    return (s00 * g0.x + s10 * g1.x) * g0.y + (s01 * g0.x + s11 * g1.x) * g1.y;
}
// ENHANCEMENT E28: the same last add, plus the dirt lit by the wide glow (the chain's coarse mip): black where
// nothing is bright, and the farther from a light the weaker
@fragment fn fs_fin_dirt(in: VsOut) -> @location(0) vec4<f32> {
    let px = 1.0 / vec2<f32>(textureDimensions(t));
    var r = tap(in.uv) * 4.0;
    r = r + (tap(in.uv + vec2<f32>(-px.x, 0.0)) + tap(in.uv + vec2<f32>(px.x, 0.0)) + tap(in.uv + vec2<f32>(0.0, -px.y)) + tap(in.uv + vec2<f32>(0.0, px.y))) * 2.0;
    r = r + tap(in.uv + vec2<f32>(-px.x, -px.y)) + tap(in.uv + vec2<f32>(px.x, -px.y)) + tap(in.uv + vec2<f32>(-px.x, px.y)) + tap(in.uv + vec2<f32>(px.x, px.y));
    let d = textureSampleLevel(dirt_t, dirt_s, in.uv, 0.0).r;
    let wide = wide_bspline(in.uv);
    return vec4<f32>(r / 16.0 * INTENSITY + wide * (d * DIRT * INTENSITY), 0.0);
}
"#;

/// The bloom's shader.
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, WGSL])
}

pub(crate) struct BloomPass {
    pub level: u8,
    /// ENHANCEMENT E28: the last add goes through the lens dirt
    pub dirt: bool,
    down_first: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    fin: wgpu::RenderPipeline,
    size: (u32, u32),
    mips: Vec<RtImage>,
}

impl BloomPass {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, level: u8, dirt: bool) -> Self {
        let p = params(level);
        let module = gfx::wgsl_module(device, "bloom", &source());
        let pl = gfx::pipeline_layout(device, "bloom", &[layout]);
        // ENHANCEMENT E28: the dirt's texture + sampler is the sprite layout again, as group 1
        let pl_dirt = gfx::pipeline_layout(device, "bloom dirt", &[layout, layout, layout]);
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let make = |fs: &str, blend: Option<wgpu::BlendState>, intensity: f32| {
            Fullscreen::new(fs, if fs == "fs_fin_dirt" { &pl_dirt } else { &pl }, &module)
                .fs(fs)
                .constants(&[("THRESHOLD", p.threshold as f64), ("KNEE", p.knee as f64), ("INTENSITY", intensity as f64), ("DIRT", crate::camfx::DIRT_GAIN as f64)])
                .build(device, &[gfx::target(HDR_FORMAT, blend)])
        };
        Self {
            level,
            dirt,
            down_first: make("fs_down_first", None, 1.0),
            down: make("fs_down", None, 1.0),
            up: make("fs_up", Some(add), 1.0),
            fin: make(if dirt { "fs_fin_dirt" } else { "fs_up" }, Some(add), p.intensity),
            size: (0, 0),
            mips: Vec::new(),
        }
    }

    fn ensure(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32) {
        if self.size == (width, height) && !self.mips.is_empty() {
            return;
        }
        self.mips.clear();
        let (mut w, mut h) = ((width / 2).max(1), (height / 2).max(1));
        for _ in 0..MIPS {
            self.mips.push(RtImage::new(device, "bloom mip", layout, sampler, w, h, HDR_FORMAT, gfx::RT_USAGE));
            if w <= 4 || h <= 4 {
                break;
            }
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        self.size = (width, height);
    }

    /// Bloom of `scene` (its bind group `scene_bg`, size `cx.size`) added onto `scene_view`.
    pub fn encode(&mut self, cx: &mut PostCtx, scene_bg: &wgpu::BindGroup, scene_view: &wgpu::TextureView, dirt_bg: Option<&wgpu::BindGroup>) {
        let (width, height) = cx.size;
        self.ensure(cx.device, cx.layout, cx.sampler, width, height);
        let encoder = &mut *cx.encoder;
        let run = |encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, src: &wgpu::BindGroup, dst: &wgpu::TextureView, load: bool, dirt: Option<(&wgpu::BindGroup, &wgpu::BindGroup)>| {
            let mut pass = gfx::color_pass(encoder, "bloom", "bloom", dst, gfx::load_or_clear(load));
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, src, &[]);
            if let Some((d, wide)) = dirt {
                pass.set_bind_group(1, d, &[]);
                pass.set_bind_group(2, wide, &[]);
            }
            pass.draw(0..3, 0..1);
        };
        let n = self.mips.len();
        run(encoder, &self.down_first, scene_bg, &self.mips[0].view, false, None);
        for i in 1..n {
            run(encoder, &self.down, &self.mips[i - 1].bg, &self.mips[i].view, false, None);
        }
        for i in (1..n).rev() {
            run(encoder, &self.up, &self.mips[i].bg, &self.mips[i - 1].view, true, None);
        }
        run(encoder, &self.fin, &self.mips[0].bg, scene_view, true, if self.dirt { dirt_bg.map(|d| (d, &self.mips[WIDE.min(n - 1)].bg)) } else { None });
    }
}

/// ENHANCEMENT E28: the mip of the chain (1/16 of the scene) whose glow the lens dirt scatters.
const WIDE: usize = 3;

/// The most mips of the chain (the first is half the scene's size).
const MIPS: usize = 6;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_get_stronger() {
        assert_eq!(params(0).intensity, 0.0);
        assert!(params(1).intensity < params(2).intensity);
        assert!(params(1).threshold > params(2).threshold);
    }
}
