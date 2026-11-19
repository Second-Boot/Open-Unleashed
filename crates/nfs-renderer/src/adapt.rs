//! ENHANCEMENT E21 — eye adaptation on the float scene (E7), applied in the tone-map pass.
//!
//! **Not the original.** The original's exposure is flat: leaving a tunnel is a hard cut in brightness.
//! Here a one-pixel pass measures the frame's mean log-luminance (a fixed 16 x 16 grid of bilinear taps
//! of the float image) and keeps two smoothed copies of it on the GPU: a quick one `F` (half-life
//! 0.12 s, the scene as it is now) and a slow one `S` (half-life 0.6 s, what the eye has got used to).
//! The tone map multiplies the image by `2^clamp(k (F - S), -cap, cap)`: when the scene turns
//! brighter than the eye is used to (a tunnel exit) the image is a little overexposed and settles
//! within a few seconds, when it turns darker (an entry) it is a little dark and lifts. A steady scene
//! has `F = S`, so the exposure is exactly 1: nothing changes where the average brightness does not.
//! Capped at +-0.5 EV. The state (two `Rgba32Float` pixels, ping-pong) never leaves the GPU, so there is
//! no read-back; the first frame (and a changed image size after a resize) starts at `F = S`.
//!
//! All plain wgpu / WGSL (a render pass into a 1 x 1 `Rgba32Float`, read with `textureLoad`).

use crate::gfx::{self, Fullscreen, PostCtx, Variants};
use crate::wgsl;
use std::time::Instant;

/// Half-lives in seconds of the quick and the slow average.
const HALF_LIFE_FAST: f32 = 0.12;
const HALF_LIFE_SLOW: f32 = 0.6;
/// EV per EV of difference, and the cap in EV.
const GAIN: f32 = 0.35;
pub(crate) const CAP_EV: f32 = 0.5;
/// The longest frame step the smoothing believes (a stall must not snap the adaptation).
const MAX_DT: f32 = 0.25;

/// The state's format: the quick and the slow average (x, y), one pixel.
const STATE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// The parameters (`params` / `eye_params`, two vec4): the quick and the slow share of this frame, 1 on a fresh start, -; the
/// gain, the cap.
type Params = [f32; 8];

/// The WGSL that the tone-map pass needs when the eye is on: the state and its parameters.
pub(crate) const TONE_BINDINGS: &str = r#"
@group(1) @binding(0) var eye_state: texture_2d<f32>;
@group(1) @binding(1) var<uniform> eye_params: array<vec4<f32>, 2>;
fn eye_exposure() -> f32 {
    let st = textureLoad(eye_state, vec2<i32>(0, 0), 0);
    let ev = clamp(eye_params[1].x * (st.x - st.y), -eye_params[1].y, eye_params[1].y);
    return exp2(ev);
}
"#;

const MEASURE_WGSL: &str = r#"
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(1) @binding(0) var prev: texture_2d<f32>;
@group(1) @binding(1) var<uniform> params: array<vec4<f32>, 2>;
// the taps: a GRID x GRID lattice; a channel is capped at BRIGHT (a lamp or the sun does not swing the mean), a luminance
// floored at DARK (log2 of black)
const GRID: i32 = 16;
const BRIGHT: f32 = 4.0;
const DARK: f32 = 1.0 / 128.0;
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    var sum = 0.0;
    for (var j = 0; j < GRID; j = j + 1) {
        for (var i = 0; i < GRID; i = i + 1) {
            let uv = (vec2<f32>(f32(i), f32(j)) + 0.5) / f32(GRID);
            var c = textureSampleLevel(t, s, uv, 0.0).rgb;
            //BLOOM_MEASURE
            c = min(c, vec3<f32>(BRIGHT));
            let l = dot(c, LUMA_709);
            sum = sum + log2(max(l, DARK));
        }
    }
    let m = sum / f32(GRID * GRID);
    let old = textureLoad(prev, vec2<i32>(0, 0), 0);
    if params[0].z > 0.5 {
        return vec4<f32>(m, m, 0.0, 1.0);
    }
    return vec4<f32>(mix(old.x, m, params[0].x), mix(old.y, m, params[0].y), 0.0, 1.0);
}
"#;

/// The measure pass's shader; `bloom`: ENHANCEMENT E10's last add is the tone map's, so the measure adds it to its taps itself
/// (group [`BLOOM_GROUP`], `bloom::TONE_WGSL`'s `bloom_coarse`): the eye sees the scene the tone map shows, as when the add was
/// a pass of its own before the measure.
pub(crate) fn measure_source(bloom: bool) -> String {
    if !bloom {
        return wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::LUMA_709, MEASURE_WGSL]);
    }
    let tone = crate::bloom::tone_source(BLOOM_GROUP);
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::LUMA_709, &tone, &MEASURE_WGSL.replace("//BLOOM_MEASURE", "c = c + bloom_coarse(uv);")])
}

/// ENHANCEMENT E10: the measure pass's group of the bloom's last add (after the image and the state).
const BLOOM_GROUP: u32 = 2;

/// ENHANCEMENT E10: the bloom chain the measure adds this frame: its group (and layout), the level and E28's dirt.
#[derive(Clone, Copy)]
pub(crate) struct Bloom<'a> {
    pub layout: &'a wgpu::BindGroupLayout,
    pub group: &'a wgpu::BindGroup,
    pub level: u8,
    pub dirt: bool,
}

pub(crate) struct EyePass {
    /// the measure without the bloom
    pipeline: wgpu::RenderPipeline,
    /// ENHANCEMENT E10: the measure with the bloom's add, per level and dirt (built at first use)
    bloom_pipes: Variants<(u8, bool), wgpu::RenderPipeline>,
    image_layout: wgpu::BindGroupLayout,
    /// group 1 of the tone-map pass and of the measure pass: the state texture + the parameters
    pub state_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    states: [(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup); 2],
    /// the state the last frame wrote
    cur: usize,
    last: Option<Instant>,
    size: (u32, u32),
    /// test glue: `NFS_EYE_DT=<seconds>` steps the smoothing by that much every frame (a scripted run's frames
    /// are not at real time); the original has no such thing and neither does a normal run
    fixed_dt: Option<f32>,
}

impl EyePass {
    pub fn new(device: &wgpu::Device, image_layout: &wgpu::BindGroupLayout) -> Self {
        let state_layout = gfx::bgl(device, "eye state", &[gfx::bgl_float(0, false), gfx::bgl_uniform(1, wgpu::ShaderStages::FRAGMENT)]);
        let module = gfx::wgsl_module(device, "eye measure", &measure_source(false));
        let pl = gfx::pipeline_layout(device, "eye measure", &[image_layout, &state_layout]);
        let pipeline = Fullscreen::new("eye measure", &pl, &module).build(device, &[gfx::target(STATE_FORMAT, None)]);
        let params = gfx::uniform_buf::<Params>(device, "eye params");
        let make = || {
            let t = gfx::texture_2d(device, "eye state", 1, 1, STATE_FORMAT, gfx::RT_USAGE);
            let v = t.create_view(&Default::default());
            let bg = gfx::bind_group(device, "eye state", &state_layout, &[gfx::bg_view(0, &v), gfx::bg_buf(1, &params)]);
            (t, v, bg)
        };
        let states = [make(), make()];
        Self { pipeline, bloom_pipes: Variants::default(), image_layout: image_layout.clone(), state_layout, params, states, cur: 0, last: None, size: (0, 0), fixed_dt: crate::dbg::flags().eye_dt }
    }

    /// The bind group (group 1) the tone-map pass reads the state the last `encode` wrote through.
    pub fn tone_group(&self) -> &wgpu::BindGroup {
        &self.states[self.cur].2
    }

    /// Measure the float image (`image_bg`, `cx.size`; ENHANCEMENT E10: plus the `bloom` the tone map adds to it) and step the
    /// adaptation: the new state is written to the other texture and becomes the current one.
    pub fn encode(&mut self, cx: &mut PostCtx, image_bg: &wgpu::BindGroup, bloom: Option<Bloom>) {
        let (width, height) = cx.size;
        let now = Instant::now();
        let dt = self.fixed_dt.unwrap_or_else(|| self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32())).min(MAX_DT);
        // a first frame or a new image size starts at rest (no jump from a size change)
        let init = self.last.is_none() || self.size != (width, height);
        self.last = Some(now);
        self.size = (width, height);
        let (af, as_) = (alpha(dt, HALF_LIFE_FAST), alpha(dt, HALF_LIFE_SLOW));
        let p: Params = [af, as_, init as u32 as f32, 0.0, GAIN, CAP_EV, 0.0, 0.0];
        gfx::belt_write(cx.belt, cx.encoder, &self.params, bytemuck::cast_slice(&p));
        let encoder = &mut *cx.encoder;
        let (prev, next) = (self.cur, 1 - self.cur);
        let target = &self.states[next].1;
        match bloom {
            Some(b) => {
                let (device, image_layout, state_layout) = (cx.device, &self.image_layout, &self.state_layout);
                let pipeline = self.bloom_pipes.get((b.level, b.dirt), || {
                    let module = gfx::wgsl_module(device, "eye measure (bloom)", &measure_source(true));
                    let pl = gfx::pipeline_layout(device, "eye measure (bloom)", &[image_layout, state_layout, b.layout]);
                    Fullscreen::new("eye measure (bloom)", &pl, &module).constants(&crate::bloom::tone_constants(b.level, b.dirt)).build(device, &[gfx::target(STATE_FORMAT, None)])
                });
                gfx::fullscreen_pass(encoder, "eye measure", "eye adaptation", target, wgpu::LoadOp::Clear(wgpu::Color::BLACK), pipeline, &[image_bg, &self.states[prev].2, b.group]);
            }
            None => gfx::fullscreen_pass(encoder, "eye measure", "eye adaptation", target, wgpu::LoadOp::Clear(wgpu::Color::BLACK), &self.pipeline, &[image_bg, &self.states[prev].2]),
        }
        self.cur = next;
    }
}

/// The share a half-life `half` takes of a step of `dt` seconds.
fn alpha(dt: f32, half: f32) -> f32 {
    1.0 - 0.5f32.powf(dt / half)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_is_a_half_life() {
        assert_eq!(alpha(0.0, 0.6), 0.0);
        assert!((alpha(0.6, 0.6) - 0.5).abs() < 1e-6);
        assert!(alpha(0.12, HALF_LIFE_FAST) > alpha(0.12, HALF_LIFE_SLOW));
    }

    #[test]
    fn exposure_is_capped() {
        // a 6 EV jump never moves the image by more than the cap
        assert_eq!((GAIN * 6.0).clamp(-CAP_EV, CAP_EV), CAP_EV);
        assert_eq!((GAIN * -6.0).clamp(-CAP_EV, CAP_EV), -CAP_EV);
    }
}
