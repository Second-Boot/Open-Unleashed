
struct U { reproj: mat4x4<f32>, p: vec4<f32>, keep: vec4<f32>, q: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(2) @binding(0) var<uniform> u: U;
// (the blur's tuned values, ENHANCEMENT E27; not the original's)
// the player's car ellipse's soft edge (its radii's share), the cockpit / hood's depth ramp (m: nearer moves with the camera)
const KEEP_EDGE: vec2<f32> = vec2<f32>(0.85, 1.15);
const COCKPIT_DEPTH: vec2<f32> = vec2<f32>(1.5, 3.5);
// the least clip w of a reprojected pixel, the least move (uv) and streak (pixels) worth a blur
const MIN_W: f32 = 0.0001;
const MIN_MOVE: f32 = 0.00001;
const MIN_STREAK_PX: f32 = 0.6;
// the taps along the streak
const TAPS: i32 = 12;
// a tap's weight by its depth over the pixel's (nearer does not smear over it), the least pixel depth of the ratio (m)
const TAP_DEPTH: vec2<f32> = vec2<f32>(0.55, 0.95);
const MIN_DEPTH: f32 = 0.05;
// the distance in front of the camera of a depth-buffer value (the projection's own inverse)
fn lin(z: f32) -> f32 {
    return proj_dist(z, u.p.z, u.p.w);
}
// 0 inside the player's car, 1 outside (a soft edge)
fn keep_w(uv: vec2<f32>) -> f32 {
    if u.keep.z <= 0.0 {
        return 1.0;
    }
    return smoothstep(KEEP_EDGE.x, KEEP_EDGE.y, length((uv - u.keep.xy) / u.keep.zw));
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let c = textureSampleLevel(t, s, uv, 0.0);
    let dims = u.q.yz;
    let z = dload(vec2<i32>(in.pos.xy), vec2<i32>(dims));
    let dl = lin(z);
    // nearer than 1.5..3.5 m is the cockpit / hood, which moves with the camera
    let k = smoothstep(COCKPIT_DEPTH.x, COCKPIT_DEPTH.y, dl) * keep_w(uv);
    if k <= 0.0 {
        return c;
    }
    let ndc = vec4<f32>(ndc_of_uv(uv), z, 1.0);
    let q = u.reproj * ndc;
    if q.w < MIN_W {
        return c;
    }
    let pn = q.xy / q.w;
    // where the pixel came from, as a move of the uv
    let v = vec2<f32>(ndc.x - pn.x, -(ndc.y - pn.y)) * 0.5;
    let len = length(vec2<f32>(v.x * u.q.x, v.y)) * u.p.x * k;
    if len < MIN_MOVE {
        return c;
    }
    let ve = v * (u.p.x * k * min(1.0, u.p.y / len));
    if length(ve * dims) < MIN_STREAK_PX {
        return c;
    }
    var sum = c.rgb;
    var wsum = 1.0;
    for (var i = 0; i < TAPS; i = i + 1) {
        let tt = (f32(i) + 0.5) / f32(TAPS) - 0.5;
        let suv = uv + ve * tt;
        let sc = clamp(vec2<i32>(suv * dims), vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
        let dt = lin(dload(sc, vec2<i32>(dims)));
        // a tap nearer than the pixel does not smear over it; one inside the car is not taken
        let w = smoothstep(TAP_DEPTH.x, TAP_DEPTH.y, dt / max(dl, MIN_DEPTH)) * keep_w(suv);
        sum = sum + textureSampleLevel(t, s, suv, 0.0).rgb * w;
        wsum = wsum + w;
    }
    return vec4<f32>(sum / wsum, c.a);
}
