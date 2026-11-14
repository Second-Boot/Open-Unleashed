// ENHANCEMENT E51 (CARSH-FADE, level 1 «где светит солнце»): how much of each near car's own sun shadow is drawn (1 all .. 0
// none), decided per CAR from E9's sun shadow map and eased in time (`gpu/carsun.rs`). One workgroup per car, 64 threads = 64
// sample points: 48 on three horizontal rings round the car (3 x 16) at `consts.y` over its road point, 16 on a 4 x 4 grid inside
// the body. A point is in the scenery's shade when something of the full cascade stands more than `consts.x` metres nearer the sun
// than the point (or than the car's own surface over it, the cars-only layer's depth: a car is never an occluder here), read as
// 4 bilinear comparison taps `consts.z` metres apart. Thread 0 turns the two shares into the car's target
// `1 - smoothstep(fade.xy, body) x smoothstep(fade.zw, rings)` (the car itself in the shade AND most of the ground round it: a
// gorge, a wall's shade, a cliff's; a post, a tree or a house that shades the car alone keeps its shadow) and eases the car's
// state toward it (`misc.w` this frame's share of the way; a car new to the list takes its target as it is).

@group(0) @binding(0) var t_shadow: texture_depth_2d_array;
@group(0) @binding(1) var s_shadow: sampler_comparison;

struct Params {
    // the cascades' light matrices and their (texel m, depth range m, split, 0), as `Csm` holds them
    mats: array<mat4x4<f32>, 4>,
    info: array<vec4<f32>, 4>,
    // x the cars, y how many near cascades have a cars-only layer, z the first of those layers, w the ease's share this frame
    misc: vec4<f32>,
    // x the occluder's least distance toward the sun (m), y the rings' height over the road (m), z the taps' spacing (m), w 0
    consts: vec4<f32>,
    // x / y the body's share of shade that starts / completes the fade, z / w the rings'
    fade: vec4<f32>,
    // the rings' radii (m)
    rings: vec4<f32>,
    // per car 4 vec4: (road point xyz, key), (forward xyz, half length), (right xyz, half width), (cascade, blend into the next, new 1, 0)
    cars: array<vec4<f32>, 64>,
};
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read_write> state: array<f32, 256>;
@group(0) @binding(4) var<storage, read_write> out_k: array<vec4<f32>, 4>;

var<workgroup> shade: array<vec2<f32>, 64>;

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
    return vec2<f32>(1.0 - lit * 0.25, 1.0);
}

@compute @workgroup_size(64)
fn car_sun(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_index) lid: u32) {
    let i = i32(wid.x);
    let base = params.cars[4 * i];
    let fwd = params.cars[4 * i + 1];
    let right = params.cars[4 * i + 2];
    let sel = params.cars[4 * i + 3];
    var p: vec3<f32>;
    if lid < 48u {
        let ring = lid / 16u;
        let a = (f32(lid % 16u) + 0.5 * f32(ring)) * (6.2831853 / 16.0);
        p = base.xyz + vec3<f32>(cos(a) * params.rings[ring], params.consts.y, sin(a) * params.rings[ring]);
    } else {
        let g = lid - 48u;
        let gx = (f32(g % 4u) - 1.5) / 1.5 * 0.8;
        let gz = (f32(g / 4u) - 1.5) / 1.5 * 0.8;
        p = base.xyz + right.xyz * (gx * right.w) + fwd.xyz * (gz * fwd.w) + vec3<f32>(0.0, 0.6, 0.0);
    }
    let c = i32(sel.x);
    var v = shade_at(c, p);
    if sel.y > 0.0 && c + 1 < 4 {
        let n = shade_at(c + 1, p);
        v = mix(v, n, sel.y);
    }
    shade[lid] = vec2<f32>(v.x * v.y, v.y);
    workgroupBarrier();
    if lid == 0u {
        var ring = vec2<f32>(0.0);
        var body = vec2<f32>(0.0);
        for (var k = 0; k < 48; k++) {
            ring += shade[k];
        }
        for (var k = 48; k < 64; k++) {
            body += shade[k];
        }
        let rs = select(0.0, ring.x / ring.y, ring.y > 0.5);
        let bs = select(0.0, body.x / body.y, body.y > 0.5);
        let goal = 1.0 - smoothstep(params.fade.x, params.fade.y, bs) * smoothstep(params.fade.z, params.fade.w, rs);
        let key = u32(base.w) & 255u;
        var s = state[key];
        if sel.z > 0.5 {
            s = goal;
        } else {
            s += (goal - s) * params.misc.w;
        }
        state[key] = s;
        out_k[i >> 2][i & 3] = s;
    }
}
