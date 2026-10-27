//! ENHANCEMENT E44 — the lighthouse beam as soft volumetric light.
//!
//! The original draws the coastal lighthouse's beam as two crossed translucent sheets (`BOX01` / `OBJECT01`, material
//! `lths`, texture alpha 12..116 of 255, `SRCALPHA` / `INVSRCALPHA`, a 401-frame rotation about the lamp): from outside a flat
//! box with straight borders and a flat cut-off end. With E44 and E7 on the sheets are not drawn and this pass paints the same
//! beam in the air instead: a full-screen fragment per view ray integrates the in-scattered light through a soft cone round
//! the beam's own axis (the sheets' apex = the lamp, their long axis, their length), with a Gaussian cross-section (a narrow
//! core and a wide faint halo), a falloff along the axis that ends smoothly beyond the sheets' length, a forward-leaning phase
//! function (brighter when the beam points at the eye) and the scene's depth (the beam stops at a hill or a car, so it is hidden
//! behind them). The result is laid over the float scene with the original's own blend (`SRCALPHA` / `INVSRCALPHA`: the beam
//! never adds more than its own colour), so it is as soft against a bright dusk sky as the original's texture was.
//!
//! The beam's TRANSFORM (apex, axis, rotation) is the original's: the app takes it from the animated article's matrix each frame
//! (`TrackWorld::lbeams`). Only the picture changes; nothing here reads or writes the simulation, a replay or the net.

use crate::gfx::{self, Fullscreen, Keyed, OverScene};
use crate::wgsl::{self, DepthKind};
use glam::Vec3;

/// The most beams a track has (the coastal lighthouse: one, built of two crossed sheets).
pub const MAX_LBEAMS: usize = 2;

/// One beam, in world space for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LBeam {
    /// the lamp: the sheets' apex
    pub apex: Vec3,
    /// unit, the beam's long axis
    pub axis: Vec3,
    /// the sheets' length along the axis (m)
    pub length: f32,
    /// the core's standard deviation at the lamp (m)
    pub sigma0: f32,
    /// how fast it grows along the axis (m per m)
    pub spread: f32,
}

/// ENHANCEMENT E44: the beams of the main race view (read only with E44 and E7 on).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LBeams {
    pub n: u8,
    pub beams: [LBeam; MAX_LBEAMS],
    /// 0..1: how dark the sky is (the beam shows a touch stronger at dusk and at night)
    pub dark: f32,
}

/// How far past the sheets' length the beam reaches before it has faded to nothing, in lengths.
const REACH: f32 = 3.0;
/// The on-axis opacity seen from the side near the lamp (the texture's own peak 116 / 255 = 0.45, two sheets in series ~0.7; the
/// soft cone's opacity through its core is this number times the profile) and the dusk gain (1 + this x darkness).
const PEAK: f32 = 1.5;
const DUSK_GAIN: f32 = 0.5;

impl LBeams {
    /// The strength factor of this frame (dusk / night a touch stronger).
    pub fn strength(&self) -> f32 {
        1.0 + DUSK_GAIN * self.dark.clamp(0.0, 1.0)
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    inv_vp: [[f32; 4]; 4],
    /// xyz the eye, w the near plane
    cam: [f32; 4],
    /// xyz the view's forward axis, w the far plane
    fwd: [f32; 4],
    /// x the beam count, y the strength, z the opacity peak, w the reach in lengths
    air: [f32; 4],
    /// xyz the apex, w the length
    apex: [[f32; 4]; MAX_LBEAMS],
    /// xyz the axis, w the core sigma at the lamp
    axis: [[f32; 4]; MAX_LBEAMS],
    /// x the sigma's growth per metre
    shape: [[f32; 4]; MAX_LBEAMS],
}

/// One frame's inputs of the pass.
pub(crate) struct Frame {
    pub inv_vp: glam::Mat4,
    pub eye: Vec3,
    pub fwd: Vec3,
    pub planes: [f32; 2],
    pub beams: LBeams,
}

const WGSL: &str = r#"
struct U {
    inv_vp: mat4x4<f32>,
    cam: vec4<f32>,
    fwd: vec4<f32>,
    air: vec4<f32>,
    apex: array<vec4<f32>, 2>,
    axis: array<vec4<f32>, 2>,
    shape: array<vec4<f32>, 2>,
};
@group(0) @binding(2) var<uniform> u: U;

const STEPS: i32 = 32;
const BEAM_COLOUR: vec3<f32> = vec3<f32>(1.0, 0.97, 0.88);
const INV_SQRT_2PI: f32 = 0.39894228;

// the distance along the view ray to the scene, from the stored depth (the sky: a long way)
fn scene_dist(d: f32, dir: vec3<f32>) -> f32 {
    let near = u.cam.w;
    let far = u.fwd.w;
    if d >= 0.99999 {
        return 100000.0;
    }
    let z = near * far / max(far - d * (far - near), 0.0001);
    return z / max(dot(dir, u.fwd.xyz), 0.05);
}

fn view_dir(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let pn = u.inv_vp * vec4<f32>(ndc, 0.0, 1.0);
    return normalize(pn.xyz / pn.w - u.cam.xyz);
}

// a forward-leaning scattering phase, 1 at right angles; `c` = the cosine between the light's travel (the axis) and the
// direction to the eye: 1 when the beam points at the eye
fn phase(c: f32) -> f32 {
    let g = 0.4;
    let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * c, 1.5);
    return clamp(0.7 + 0.3 * hg / 0.6724, 0.7, 1.5);
}

// the cross-section: a narrow core and a wide faint halo, each normalised so that a ray straight through the beam sideways
// integrates to the along-axis opacity
fn profile(q2: f32, sigma: f32) -> f32 {
    let s2 = sigma * sigma;
    let w = 2.6 * sigma;
    let w2 = w * w;
    return INV_SQRT_2PI * (0.75 * exp(-0.5 * q2 / s2) / sigma + 0.25 * exp(-0.5 * q2 / w2) / w);
}

// the opacity per unit length of the axis at distance s from the lamp: rises from the lantern, decays toward the end of the
// sheets, and is gone smoothly past it
fn axial(s: f32, len: f32) -> f32 {
    let rise = 1.0 - exp(-s / 0.4);
    let fall = exp(-1.5 * s / len);
    let tail = 1.0 - smoothstep(0.7 * u.air.w * len, u.air.w * len, s);
    return rise * fall * tail;
}

// the optical depth of beam `b` along the ray, within the scene distance `ts`
fn beam_tau(b: i32, eye: vec3<f32>, dir: vec3<f32>, ts: f32, jit: f32) -> f32 {
    let apex = u.apex[b].xyz;
    let len = u.apex[b].w;
    let a = u.axis[b].xyz;
    let s0 = u.axis[b].w;
    let k = u.shape[b].x;
    let reach = u.air.w * len;
    let w = eye - apex;
    let wa = dot(w, a);
    let da = dot(dir, a);
    let dp = dir - a * da;
    let wp = w - a * wa;
    // the bounding cylinder: three of the halo's sigmas at the far end
    let rb = 3.0 * 2.6 * (s0 + k * reach);
    let qa = dot(dp, dp);
    let qb = dot(wp, dp);
    let qc = dot(wp, wp) - rb * rb;
    var t0 = 0.0;
    var t1 = ts;
    if qa < 1e-8 {
        if qc > 0.0 {
            return 0.0;
        }
    } else {
        let disc = qb * qb - qa * qc;
        if disc <= 0.0 {
            return 0.0;
        }
        let r = sqrt(disc);
        t0 = max(t0, (-qb - r) / qa);
        t1 = min(t1, (-qb + r) / qa);
    }
    // the slab 0 <= s <= reach
    if abs(da) > 1e-6 {
        let ta = (0.0 - wa) / da;
        let tb = (reach - wa) / da;
        t0 = max(t0, min(ta, tb));
        t1 = min(t1, max(ta, tb));
    } else if wa < 0.0 || wa > reach {
        return 0.0;
    }
    if t1 <= t0 {
        return 0.0;
    }
    let dt = (t1 - t0) / f32(STEPS);
    var sum = 0.0;
    for (var i = 0; i < STEPS; i++) {
        let t = t0 + (f32(i) + jit) * dt;
        let s = wa + da * t;
        if s <= 0.0 {
            continue;
        }
        let pw = wp + dp * t;
        let sigma = s0 + k * s;
        sum += axial(s, len) * profile(dot(pw, pw), sigma) * smoothstep(8.0, 60.0, t);
    }
    return sum * dt * phase(-da);
}

// the optical depth of all the beams at the scene distance of one depth value
fn tau_at(d: f32, dir: vec3<f32>, eye: vec3<f32>, jit: f32) -> f32 {
    let ts = scene_dist(d, dir);
    var tau = 0.0;
    for (var b = 0; b < i32(u.air.x); b++) {
        tau += beam_tau(b, eye, dir, ts, jit);
    }
    return tau;
}

@fragment fn fs(@builtin(position) pos: vec4<f32>//SAMPLE_ARG) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    let dir = view_dir(pos.xy / dims);
    let eye = u.cam.xyz;
    let jit = fract(52.9829189 * fract(dot(pos.xy, vec2<f32>(0.06711056, 0.00583715))));
    // ENHANCEMENT ENH-LBEAM2: the beam is occluded per depth SAMPLE (a multisampled depth image: per sample of the colour when the
    // pass writes multisampled colour, else the mean over the samples of the pixel), so a foliage silhouette has no beam-coloured
    // fringe: the pixel's beam is the share of its samples that see it
    var tau = 0.0;
//TAU
    if tau <= 0.0 {
        discard;
    }
    tau = tau * u.air.y * u.air.z;
    // a soft cap: a beam straight in the eye glares but never becomes an opaque white disc
    let a = 0.7 * (1.0 - exp(-tau / 0.7));
    // the core also adds light (the float scene keeps the overshoot): premultiplied colour
    let glow = BEAM_COLOUR * (0.25 * min(tau, 1.0));
    return vec4<f32>(BEAM_COLOUR * a + glow, a);
}
"#;

/// The fragment's `tau` by depth kind: one depth value, the sample the fragment is for (`sample_index`), or the mean of the pixel's samples.
const TAU_1: &str = "tau = tau_at(depth_at(vec2<i32>(pos.xy)), dir, eye, jit);";
const TAU_SAMPLE: &str = "tau = tau_at(textureLoad(depth_ms, vec2<i32>(pos.xy), i32(si)), dir, eye, jit);";
const TAU_MEAN: &str = "let n = i32(textureNumSamples(depth_ms));
    var dmin = 1.0;
    var dmax = 0.0;
    for (var i = 0; i < n; i++) {
        let d = textureLoad(depth_ms, vec2<i32>(pos.xy), i);
        dmin = min(dmin, d);
        dmax = max(dmax, d);
    }
    if dmin == dmax {
        tau = tau_at(dmin, dir, eye, jit);
    } else {
        for (var i = 0; i < n; i++) {
            tau += tau_at(textureLoad(depth_ms, vec2<i32>(pos.xy), i), dir, eye, jit);
        }
        tau = tau / f32(n);
    }";

/// The pass's shader for a depth `kind` and a target of `samples` samples.
pub(crate) fn source(kind: DepthKind, samples: u32) -> String {
    let tau = match kind {
        DepthKind::Multi if samples > 1 => TAU_SAMPLE,
        DepthKind::Multi => TAU_MEAN,
        _ => TAU_1,
    };
    let src = WGSL.replace("//TAU", tau).replace("//SAMPLE_ARG", if kind == DepthKind::Multi && samples > 1 { ", @builtin(sample_index) si: u32" } else { "" });
    wgsl::compose(&[wgsl::FULLSCREEN_VS, kind.wgsl(), &src])
}

struct Pipes {
    pipe: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

/// The pipelines (one per depth kind and sample count of the target, built when first needed) and the uniform.
pub(crate) struct LBeamPass {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    ubuf: wgpu::Buffer,
    pipes: std::collections::HashMap<(DepthKind, u32), Pipes>,
    /// the pass's bind group, kept while the pipelines and the depth hold
    bg: Keyed<((DepthKind, u32), wgpu::TextureView), wgpu::BindGroup>,
}

impl LBeamPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ubuf = gfx::uniform_buf::<Uniform>(device, "lbeam");
        Self { device: device.clone(), format, ubuf, pipes: Default::default(), bg: Keyed::default() }
    }

    fn build(&self, kind: DepthKind, samples: u32) -> Pipes {
        let module = gfx::wgsl_module(&self.device, "lbeam", &source(kind, samples));
        let depth = kind.layout_entry();
        let uniform = gfx::bgl_uniform(2, wgpu::ShaderStages::FRAGMENT);
        let bgl = gfx::bgl(&self.device, "lbeam", &[depth, uniform]);
        // the original's blend (SRCALPHA / INVSRCALPHA) on the colour; the target's alpha is left alone
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        let pl = gfx::pipeline_layout(&self.device, "lbeam", &[&bgl]);
        let pipe = Fullscreen::new("lbeam", &pl, &module).samples(samples).build(&self.device, &[gfx::target(self.format, Some(blend))]);
        Pipes { pipe, bgl }
    }

    /// Record the pass into `encoder`: over `colour` (loaded; `resolve` when it is multisampled; `keep` = store the multisampled
    /// samples for a later pass), reading `depth`.
    pub(crate) fn draw(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, f: &Frame, at: &OverScene) {
        let OverScene { depth, kind, colour, resolve, samples, keep } = *at;
        let samples = samples.max(1);
        let key = (kind, samples);
        if !self.pipes.contains_key(&key) {
            let p = self.build(kind, samples);
            self.pipes.insert(key, p);
        }
        let pipes = &self.pipes[&key];
        let mut u = Uniform {
            inv_vp: f.inv_vp.to_cols_array_2d(),
            cam: [f.eye.x, f.eye.y, f.eye.z, f.planes[0]],
            fwd: [f.fwd.x, f.fwd.y, f.fwd.z, f.planes[1]],
            air: [f.beams.n as f32, f.beams.strength(), PEAK, REACH],
            apex: [[0.0; 4]; MAX_LBEAMS],
            axis: [[0.0; 4]; MAX_LBEAMS],
            shape: [[0.0; 4]; MAX_LBEAMS],
        };
        for (i, b) in f.beams.beams.iter().take(f.beams.n as usize).enumerate() {
            u.apex[i] = [b.apex.x, b.apex.y, b.apex.z, b.length];
            u.axis[i] = [b.axis.x, b.axis.y, b.axis.z, b.sigma0];
            u.shape[i] = [b.spread, 0.0, 0.0, 0.0];
        }
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));
        let (device, ubuf) = (&self.device, &self.ubuf);
        let bg = self.bg.get((key, depth.clone()), || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lbeam"),
                layout: &pipes.bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: kind.binding(), resource: wgpu::BindingResource::TextureView(depth) },
                    wgpu::BindGroupEntry { binding: 2, resource: ubuf.as_entire_binding() },
                ],
            })
        });
        let mut pass = gfx::color_pass_resolve(encoder, "lighthouse beam", "lighthouse beam", colour, resolve, wgpu::LoadOp::Load, if samples > 1 && !keep { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store });
        pass.set_pipeline(&pipes.pipe);
        pass.set_bind_group(0, bg, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        // mat4 + 3 vec4 + 3 x 2 vec4
        assert_eq!(std::mem::size_of::<Uniform>(), 64 + 3 * 16 + 3 * 2 * 16);
    }

    #[test]
    fn dusk_is_a_touch_stronger() {
        let day = LBeams { dark: 0.1, ..Default::default() };
        let dusk = LBeams { dark: 0.6, ..Default::default() };
        assert!(dusk.strength() > day.strength() && dusk.strength() <= 1.5);
    }
}
