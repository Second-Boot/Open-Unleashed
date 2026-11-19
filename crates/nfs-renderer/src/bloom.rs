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
//! PERF (RENDER-AUDIT phase 3, 2026-11-15): that last add is not a pass of its own (a full-size read-modify-write of
//! the float scene) but part of the tone map ([`TONE_WGSL`]: the same tent and E28 dirt, summed in f32 where the pass stored
//! the sum in half floats); E21's eye measure, which used to read the scene after the add, adds the chain's first mip itself
//! ([`TONE_WGSL`]'s `bloom_coarse`).
//!
//! The chain runs between the resolved float image and the tone map. The lens-flare probe reads the
//! image before it (inside `encode_frame`), so the probe is untouched; the rear-view mirror, the HUD
//! and every 2D screen never see it.

use crate::gfx::{self, Fullscreen, PostCtx, RtImage, Variants};
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

/// The 3x3 tent of `tx` round `uv` (its texel size): the chain's upsample and the last add's.
const TENT_WGSL: &str = r#"
fn tent9(tx: texture_2d<f32>, sm: sampler, uv: vec2<f32>) -> vec3<f32> {
    let px = 1.0 / vec2<f32>(textureDimensions(tx));
    var r = textureSampleLevel(tx, sm, uv, 0.0).rgb * 4.0;
    r = r + (textureSampleLevel(tx, sm, uv + vec2<f32>(-px.x, 0.0), 0.0).rgb + textureSampleLevel(tx, sm, uv + vec2<f32>(px.x, 0.0), 0.0).rgb + textureSampleLevel(tx, sm, uv + vec2<f32>(0.0, -px.y), 0.0).rgb + textureSampleLevel(tx, sm, uv + vec2<f32>(0.0, px.y), 0.0).rgb) * 2.0;
    r = r + textureSampleLevel(tx, sm, uv + vec2<f32>(-px.x, -px.y), 0.0).rgb + textureSampleLevel(tx, sm, uv + vec2<f32>(px.x, -px.y), 0.0).rgb + textureSampleLevel(tx, sm, uv + vec2<f32>(-px.x, px.y), 0.0).rgb + textureSampleLevel(tx, sm, uv + vec2<f32>(px.x, px.y), 0.0).rgb;
    return r / 16.0;
}
"#;

const WGSL: &str = r#"
// threshold / knee come in as pipeline constants
override THRESHOLD: f32 = 1.0;
override KNEE: f32 = 0.1;
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t, s, uv, 0.0).rgb;
}
fn lum(c: vec3<f32>) -> f32 {
    return dot(c, LUMA_709);
}
// the soft-knee bright pass (the quadratic knee of Unity's / Bevy's prefilter); also capped (HOT) so a
// single hot texel cannot flicker the whole chain; EPS keeps the divisions finite
const HOT: f32 = 8.0;
const EPS: f32 = 0.0001;
fn bright(c0: vec3<f32>) -> vec3<f32> {
    let c = min(max(c0, vec3<f32>(0.0)), vec3<f32>(HOT));
    let l = lum(c);
    var rq = clamp(l - THRESHOLD + KNEE, 0.0, 2.0 * KNEE);
    rq = rq * rq / (4.0 * KNEE + EPS);
    let w = max(rq, l - THRESHOLD) / max(l, EPS);
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
    return vec4<f32>(tent9(t, s, in.uv), 0.0);
}
"#;

/// ENHANCEMENT E10: the bloom's last add as the tone map (and E21's measure) read it: group `BLOOM_GROUP` = the chain's
/// first mip (0), the sampler (1), E28's lens dirt (2) and the wide mip it scatters (3) ([`BloomPass::tone_layout`]); the
/// level's intensity and the dirt's gain (0 = no dirt) are the pipeline constants `BLOOM_INTENSITY` / `BLOOM_DIRT`.
pub(crate) const TONE_WGSL: &str = r#"
override BLOOM_INTENSITY: f32 = 0.0;
override BLOOM_DIRT: f32 = 0.0;
@group(BLOOM_GROUP) @binding(0) var bloom_t: texture_2d<f32>;
@group(BLOOM_GROUP) @binding(1) var bloom_s: sampler;
@group(BLOOM_GROUP) @binding(2) var bloom_dirt_t: texture_2d<f32>;
@group(BLOOM_GROUP) @binding(3) var bloom_wide_t: texture_2d<f32>;
// ENHANCEMENT E28 (ENH-SUNEDGE): the wide mip (1/16 of the scene) read with a cubic B-spline (four bilinear taps,
// GPU Gems 2 ch. 20) instead of one bilinear tap. Bilinear over a 16x magnification is piecewise linear: its slope
// jumps on every texel line, and the dirt multiplies the glow by up to DIRT x INTENSITY (17.5), so round the HDR sun
// those jumps drew straight vertical / horizontal creases (the texel grid) into the dirt's glow. The B-spline is C2.
fn wide_bspline(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(bloom_wide_t));
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
    let s00 = textureSampleLevel(bloom_wide_t, bloom_s, vec2<f32>(c0.x, c0.y), 0.0).rgb;
    let s10 = textureSampleLevel(bloom_wide_t, bloom_s, vec2<f32>(c1.x, c0.y), 0.0).rgb;
    let s01 = textureSampleLevel(bloom_wide_t, bloom_s, vec2<f32>(c0.x, c1.y), 0.0).rgb;
    let s11 = textureSampleLevel(bloom_wide_t, bloom_s, vec2<f32>(c1.x, c1.y), 0.0).rgb;
    return (s00 * g0.x + s10 * g1.x) * g0.y + (s01 * g0.x + s11 * g1.x) * g1.y;
}
// the last add: the first mip's tent at the level's intensity; ENHANCEMENT E28: plus the dirt lit by the wide glow (the
// chain's coarse mip): black where nothing is bright, and the farther from a light the weaker
fn bloom_add(uv: vec2<f32>) -> vec3<f32> {
    let r = tent9(bloom_t, bloom_s, uv) * BLOOM_INTENSITY;
    if BLOOM_DIRT == 0.0 {
        return r;
    }
    let d = textureSampleLevel(bloom_dirt_t, bloom_s, uv, 0.0).r;
    return r + wide_bspline(uv) * (d * BLOOM_DIRT * BLOOM_INTENSITY);
}
// ENHANCEMENT E21: the same add as the eye measure's taps read it: one bilinear tap of the first mip and of the wide one
// (the tent and the B-spline only smooth what a 16 x 16 grid of taps averages anyway)
fn bloom_coarse(uv: vec2<f32>) -> vec3<f32> {
    let r = textureSampleLevel(bloom_t, bloom_s, uv, 0.0).rgb * BLOOM_INTENSITY;
    if BLOOM_DIRT == 0.0 {
        return r;
    }
    let d = textureSampleLevel(bloom_dirt_t, bloom_s, uv, 0.0).r;
    return r + textureSampleLevel(bloom_wide_t, bloom_s, uv, 0.0).rgb * (d * BLOOM_DIRT * BLOOM_INTENSITY);
}
"#;

/// The bloom's shader.
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::LUMA_709, TENT_WGSL, WGSL])
}

/// ENHANCEMENT E10: the WGSL a pass that adds the bloom's last level needs ([`TONE_WGSL`] and the tent), its group `group`.
pub(crate) fn tone_source(group: u32) -> String {
    format!("{TENT_WGSL}{}", TONE_WGSL.replace("BLOOM_GROUP", &group.to_string()))
}

/// ENHANCEMENT E10 / E28: the pipeline constants of [`TONE_WGSL`] for `level` and the dirt (`dirt`).
pub(crate) fn tone_constants(level: u8, dirt: bool) -> [(&'static str, f64); 2] {
    [("BLOOM_INTENSITY", params(level).intensity as f64), ("BLOOM_DIRT", if dirt { crate::camfx::DIRT_GAIN as f64 } else { 0.0 })]
}

/// The chain's pipelines for one level (its numbers are pipeline constants).
struct Pipes {
    down_first: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
}

/// The bloom chain: the shader and its layouts (made once), the pipelines per level (built at the first frame of a level and
/// kept, so a toggle in the menu does not rebuild the module), the mips and the last add's group for the tone map.
pub(crate) struct BloomPass {
    module: wgpu::ShaderModule,
    pl: wgpu::PipelineLayout,
    /// the group [`TONE_WGSL`] reads: the first mip, the sampler, E28's dirt and the wide mip (the tone map's and E21's
    /// measure's layouts take it)
    pub tone_layout: wgpu::BindGroupLayout,
    pipes: Variants<u8, Pipes>,
    size: (u32, u32),
    mips: Vec<RtImage>,
    /// the group of [`Self::tone_layout`] over the current mips (and the dirt, when there is one)
    tone_bg: gfx::Keyed<(wgpu::TextureView, Option<wgpu::TextureView>), wgpu::BindGroup>,
}

impl BloomPass {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let module = gfx::wgsl_module(device, "bloom", &source());
        let pl = gfx::pipeline_layout(device, "bloom", &[layout]);
        let tone_layout = gfx::bgl(device, "bloom (tone map)", &[gfx::bgl_float(0, true), gfx::bgl_sampler(1, wgpu::SamplerBindingType::Filtering), gfx::bgl_float(2, true), gfx::bgl_float(3, true)]);
        Self { module, pl, tone_layout, pipes: Variants::default(), size: (0, 0), mips: Vec::new(), tone_bg: gfx::Keyed::default() }
    }

    fn build(device: &wgpu::Device, module: &wgpu::ShaderModule, pl: &wgpu::PipelineLayout, level: u8) -> Pipes {
        let p = params(level);
        let make = |fs: &str, blend: Option<wgpu::BlendState>| {
            Fullscreen::new(fs, pl, module).fs(fs).constants(&[("THRESHOLD", p.threshold as f64), ("KNEE", p.knee as f64)]).build(device, &[gfx::target(HDR_FORMAT, blend)])
        };
        Pipes { down_first: make("fs_down_first", None), down: make("fs_down", None), up: make("fs_up", Some(gfx::blend::ADD_KEEP_ALPHA)) }
    }

    fn ensure(&mut self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32) {
        if self.size == (width, height) && !self.mips.is_empty() {
            return;
        }
        self.mips.clear();
        let (mut w, mut h) = ((width / 2).max(1), (height / 2).max(1));
        for _ in 0..MIPS {
            self.mips.push(RtImage::new(device, "bloom mip", layout, sampler, w, h, HDR_FORMAT, gfx::RT_USAGE));
            if w <= SMALLEST_MIP || h <= SMALLEST_MIP {
                break;
            }
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        self.size = (width, height);
    }

    /// The chain of `level` (above 0) over `scene` (its bind group `scene_bg`, size `cx.size`), up to the first mip; the last add
    /// is the tone map's, through [`Self::tone`] (ENHANCEMENT E28: with `dirt`, the lens dirt's view, it scatters the wide mip
    /// through the dirt).
    pub fn encode(&mut self, cx: &mut PostCtx, level: u8, scene_bg: &wgpu::BindGroup, dirt: Option<&wgpu::TextureView>) {
        let (width, height) = cx.size;
        self.ensure(cx.device, cx.layout, cx.sampler, width, height);
        let (device, module, pl) = (cx.device, &self.module, &self.pl);
        let p = self.pipes.get(level, || Self::build(device, module, pl, level));
        let encoder = &mut *cx.encoder;
        let run = |encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, groups: &[&wgpu::BindGroup], dst: &wgpu::TextureView, load: bool| {
            gfx::fullscreen_pass(encoder, "bloom", "bloom", dst, gfx::load_or_clear(load), pipeline, groups);
        };
        let n = self.mips.len();
        run(encoder, &p.down_first, &[scene_bg], &self.mips[0].view, false);
        for i in 1..n {
            run(encoder, &p.down, &[&self.mips[i - 1].bg], &self.mips[i].view, false);
        }
        for i in (1..n).rev() {
            run(encoder, &p.up, &[&self.mips[i].bg], &self.mips[i - 1].view, true);
        }
        let (first, wide) = (&self.mips[0].view, &self.mips[WIDE.min(n - 1)].view);
        let (layout, sampler) = (&self.tone_layout, cx.sampler);
        // without the dirt its two slots hold the first mip (the shader does not read them: `BLOOM_DIRT` is 0)
        self.tone_bg.get((first.clone(), dirt.cloned()), || {
            gfx::bind_group(device, "bloom (tone map)", layout, &[gfx::bg_view(0, first), gfx::bg_sampler(1, sampler), gfx::bg_view(2, dirt.unwrap_or(first)), gfx::bg_view(3, if dirt.is_some() { wide } else { first })])
        });
    }

    /// The group (and its layout) the tone map and E21's measure read the last [`Self::encode`]'s chain through.
    pub fn tone(&self) -> Option<(&wgpu::BindGroupLayout, &wgpu::BindGroup)> {
        self.tone_bg.value().map(|bg| (&self.tone_layout, bg))
    }
}

/// The chain stops at the first mip this small on a side.
const SMALLEST_MIP: u32 = 4;

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
