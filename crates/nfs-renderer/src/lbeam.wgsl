
struct U {
    inv_vp: mat4x4<f32>,
    cam: vec4<f32>,
    fwd: vec4<f32>,
    air: vec4<f32>,
    apex: array<vec4<f32>, MAX_LBEAMS_N>,
    axis: array<vec4<f32>, MAX_LBEAMS_N>,
    shape: array<vec4<f32>, MAX_LBEAMS_N>,
};
@group(0) @binding(2) var<uniform> u: U;

const STEPS: i32 = 32;
const BEAM_COLOUR: vec3<f32> = vec3<f32>(1.0, 0.97, 0.88);
const INV_SQRT_2PI: f32 = 0.39894228;
// (the tuned shape of the beam, ENH-LBEAM; not the original's)
// a stored depth at or above this is the sky, this far away (m); the least cosine of the ray to the view's axis
const SKY_DEPTH: f32 = 0.99999;
const SKY_DIST: f32 = 100000.0;
const MIN_AXIS_COS: f32 = 0.05;
// the scattering phase: Henyey-Greenstein's g and exponent, its value at right angles (the normaliser, (1 - g^2) / (1 + g^2)^1.5),
// and the result held to [PHASE_MIN, PHASE_MAX] from PHASE_MIN + PHASE_SPAN * the normalised lobe
const PHASE_G: f32 = 0.4;
const HG_POW: f32 = 1.5;
const PHASE_NORM: f32 = 0.6724;
const PHASE_MIN: f32 = 0.7;
const PHASE_SPAN: f32 = 0.3;
const PHASE_MAX: f32 = 1.5;
// the cross-section: the core's share (the halo has the rest)
const CORE_SHARE: f32 = 0.75;
// along the axis: the rise from the lantern (m), the decay over the length, the tail's start as a share of the reach
const RISE_LEN: f32 = 0.4;
const FALL_RATE: f32 = 1.5;
const TAIL_START: f32 = 0.7;
// a ray along the cylinder's axis / across the slab (the quadratic's and the slab's degenerate cases)
const EPS_AXIAL: f32 = 1e-8;
const EPS_SLAB: f32 = 1e-6;
// the beam fades in from the eye over these distances (m)
const NEAR_FADE: vec2<f32> = vec2<f32>(8.0, 60.0);
// the soft cap of the opacity (the scale of its exponential and its ceiling)
const CAP: f32 = 0.7;

// the distance along the view ray to the scene, from the stored depth (the sky: a long way)
fn scene_dist(d: f32, dir: vec3<f32>) -> f32 {
    let near = u.cam.w;
    let far = u.fwd.w;
    if d >= SKY_DEPTH {
        return SKY_DIST;
    }
    let z = linear_z(d, near, far);
    return z / max(dot(dir, u.fwd.xyz), MIN_AXIS_COS);
}

// a forward-leaning scattering phase, 1 at right angles; `c` = the cosine between the light's travel (the axis) and the
// direction to the eye: 1 when the beam points at the eye
fn phase(c: f32) -> f32 {
    let g = PHASE_G;
    let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * c, HG_POW);
    return clamp(PHASE_MIN + PHASE_SPAN * hg / PHASE_NORM, PHASE_MIN, PHASE_MAX);
}

// the cross-section: a narrow core and a wide faint halo, each normalised so that a ray straight through the beam sideways
// integrates to the along-axis opacity
fn profile(q2: f32, sigma: f32) -> f32 {
    let s2 = sigma * sigma;
    let w = HALO_N * sigma;
    let w2 = w * w;
    return INV_SQRT_2PI * (CORE_SHARE * exp(-0.5 * q2 / s2) / sigma + 0.25 * exp(-0.5 * q2 / w2) / w);
}

// the opacity per unit length of the axis at distance s from the lamp: rises from the lantern, decays toward the end of the
// sheets, and is gone smoothly past it
fn axial(s: f32, len: f32) -> f32 {
    let rise = 1.0 - exp(-s / RISE_LEN);
    let fall = exp(-FALL_RATE * s / len);
    let tail = 1.0 - smoothstep(TAIL_START * u.air.w * len, u.air.w * len, s);
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
    let rb = BOUND_N * HALO_N * (s0 + k * reach);
    let qa = dot(dp, dp);
    let qb = dot(wp, dp);
    let qc = dot(wp, wp) - rb * rb;
    var t0 = 0.0;
    var t1 = ts;
    if qa < EPS_AXIAL {
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
    if abs(da) > EPS_SLAB {
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
        sum += axial(s, len) * profile(dot(pw, pw), sigma) * smoothstep(NEAR_FADE.x, NEAR_FADE.y, t);
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
    let jit = ign(pos.xy);
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
    let a = CAP * (1.0 - exp(-tau / CAP));
    // the core also adds light (the float scene keeps the overshoot): premultiplied colour
    let glow = BEAM_COLOUR * (0.25 * min(tau, 1.0));
    return vec4<f32>(BEAM_COLOUR * a + glow, a);
}
