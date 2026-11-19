// ENHANCEMENT E51 (CARSH-FADE, level 1 «где светит солнце»): how much of each near car's own sun shadow is drawn (1 all .. 0
// none), decided per CAR from E9's sun shadow map and eased in time (`gpu/carsun.rs`, which puts the sizes in front: `SAMPLES`,
// `RING_COUNT`, ...). One workgroup per car, one thread per sample point (64): 48 on three horizontal rings round the car (3 x 16)
// at `consts.y` over its road point, 16 on a 4 x 4 grid inside the body. A point is in the scenery's shade when something of the full cascade stands more than `consts.x` metres nearer the sun
// than the point (or than the car's own surface over it, the cars-only layer's depth: a car is never an occluder here), read as
// 4 bilinear comparison taps `consts.z` metres apart. Thread 0 turns the two shares into the car's target
// `1 - smoothstep(fade.xy, body) x smoothstep(fade.zw, rings)` (the car itself in the shade AND most of the ground round it: a
// gorge, a wall's shade, a cliff's; a post, a tree or a house that shades the car alone keeps its shadow) and eases the car's
// state toward it (`misc.w` this frame's share of the way; a car new to the list takes its target as it is).

@group(0) @binding(0) var t_shadow: texture_depth_2d_array;
@group(0) @binding(1) var s_shadow: sampler_comparison;

struct Params {
    // the cascades' light matrices and their (texel m, depth range m, split, 0), as `Csm` holds them
    mats: array<mat4x4<f32>, MAX_CASCADES>,
    info: array<vec4<f32>, MAX_CASCADES>,
    // x the cars, y how many near cascades have a cars-only layer, z the first of those layers, w the ease's share this frame
    misc: vec4<f32>,
    // x the occluder's least distance toward the sun (m), y the rings' height over the road (m), z the taps' spacing (m), w 0
    consts: vec4<f32>,
    // x / y the body's share of shade that starts / completes the fade, z / w the rings'
    fade: vec4<f32>,
    // the rings' radii (m)
    rings: vec4<f32>,
    // per car 4 vec4: (road point xyz, key), (forward xyz, half length), (right xyz, half width), (cascade, blend into the next, new 1, 0)
    cars: array<vec4<f32>, CARS_VEC4S>,
};
// the body grid's points reach this share of the half sizes, at this height over the road point (m); a ring's points stand half a
// step turned from the ring inside it
const BODY_INSET: f32 = 0.8;
const BODY_HEIGHT: f32 = 0.6;
const RING_STAGGER: f32 = 0.5;
// an occluder sample's comparison taps (its corners `consts.z` metres apart)
const TAPS: f32 = 4.0;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read_write> state: array<f32, SUN_CAR_KEYS>;
// the cars' shares, one f32 each (the same bytes as the csm uniform's `carsh_k` vec4s, which the copy fills). Not a vec4 array
// written a lane at a time: each car is its own workgroup, and on Metal a lane store into a vec4 that other workgroups write
// too acts as a whole-vector read-modify-write, so a car's lane could keep the last frame's share, a different one run to run
@group(0) @binding(4) var<storage, read_write> out_k: array<f32, SHARES>;

var<workgroup> shade: array<vec2<f32>, SAMPLES>;

// (shade 0..1, weight 0 / 1) of `p` in cascade `c`
fn shade_at(c: i32, p: vec3<f32>) -> vec2<f32> {
    let q = params.mats[c] * vec4<f32>(p, 1.0);
    let uv = vec2<f32>(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || q.z > 1.0 || q.z < 0.0 {
        return vec2<f32>(0.0, 0.0);
    }
    let info = params.info[c];
    let size = i32(textureDimensions(t_shadow).x);
    // the car's own surface over the point (cars-only layer): the reference is the nearer of it and the point
    var reference = q.z;
    if c < i32(params.misc.y) {
        let px = clamp(vec2<i32>(uv * f32(size)), vec2<i32>(0), vec2<i32>(size - 1));
        reference = min(reference, textureLoad(t_shadow, px, i32(params.misc.z) + c, 0));
    }
    reference -= params.consts.x / info.y;
    let h = params.consts.z * 0.5 / info.x / f32(size);
    var lit = 0.0;
    for (var k = 0; k < 4; k++) {
        let o = vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * h;
        lit += textureSampleCompareLevel(t_shadow, s_shadow, uv + o, c, reference);
    }
    return vec2<f32>(1.0 - lit / TAPS, 1.0);
}

@compute @workgroup_size(SAMPLES)
fn car_sun(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let i = i32(wid.x);
    let base = params.cars[4 * i];
    let fwd = params.cars[4 * i + 1];
    let right = params.cars[4 * i + 2];
    let sel = params.cars[4 * i + 3];
    var p: vec3<f32>;
    let ring_samples = RING_COUNT * RING_POINTS;
    if lid < ring_samples {
        let ring = lid / RING_POINTS;
        let a = (f32(lid % RING_POINTS) + RING_STAGGER * f32(ring)) * (TAU / f32(RING_POINTS));
        p = base.xyz + vec3<f32>(cos(a) * params.rings[ring], params.consts.y, sin(a) * params.rings[ring]);
    } else {
        // the grid's cells from -1 to 1 of the inset half sizes
        let g = lid - ring_samples;
        let mid = f32(BODY_GRID - 1u) * 0.5;
        let gx = (f32(g % BODY_GRID) - mid) / mid * BODY_INSET;
        let gz = (f32(g / BODY_GRID) - mid) / mid * BODY_INSET;
        p = base.xyz + right.xyz * (gx * right.w) + fwd.xyz * (gz * fwd.w) + vec3<f32>(0.0, BODY_HEIGHT, 0.0);
    }
    let c = i32(sel.x);
    var v = shade_at(c, p);
    if sel.y > 0.0 && c + 1 < MAX_CASCADES {
        let n = shade_at(c + 1, p);
        v = mix(v, n, sel.y);
    }
    shade[lid] = vec2<f32>(v.x * v.y, v.y);
    workgroupBarrier();
    if lid == 0u {
        var ring = vec2<f32>(0.0);
        var body = vec2<f32>(0.0);
        for (var k = 0u; k < ring_samples; k++) {
            ring += shade[k];
        }
        for (var k = ring_samples; k < SAMPLES; k++) {
            body += shade[k];
        }
        let rs = select(0.0, ring.x / ring.y, ring.y > 0.5);
        let bs = select(0.0, body.x / body.y, body.y > 0.5);
        let goal = 1.0 - smoothstep(params.fade.x, params.fade.y, bs) * smoothstep(params.fade.z, params.fade.w, rs);
        let key = u32(base.w) & (SUN_CAR_KEYS - 1u);
        var s = state[key];
        if sel.z > 0.5 {
            s = goal;
        } else {
            s += (goal - s) * params.misc.w;
        }
        state[key] = s;
        out_k[i] = s;
    }
}
