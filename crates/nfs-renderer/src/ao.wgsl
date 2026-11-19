// ENHANCEMENT E14 (corner shading, weak GTAO): screen-space ambient occlusion from the opaque
// part's depth. Ground-truth AO (horizon based) after Intel's XeGTAO, as ported in Bevy's
// `bevy_pbr/src/ssao` (MIT / Apache-2.0, https://github.com/bevyengine/bevy): the slice / horizon
// integral and the falloff are theirs; adapted here to a half-size occlusion with the normal
// rebuilt from the depth (no normal pre-pass), a fixed 4 x 4 interleaved pattern (no temporal
// accumulation, so nothing flickers), a depth-aware blur and upsample (the upsample inside the apply).
//
// Passes: `fs_ao` (depth -> raw visibility, half size), `fs_blur` / `fs_blur_v` (the horizontal and
// the vertical blur, half size), `fs_apply_1` /
// `fs_apply_ms` (the depth-aware upsample to full size and a multiply over the opaque colour; the multisampled one shades per sample so a
// sample of another surface, or of the sky, keeps its colour: no halo at silhouettes).
struct Params {
    // tan(half fov x), tan(half fov y), near, far
    cam: vec4<f32>,
    // radius (m), strength (0..1), fade start (m), fade end (m)
    ao: vec4<f32>,
    // slices, steps per side, power, 0
    n: vec4<f32>,
    // width, height, 1 / width, 1 / height
    size: vec4<f32>,
    // the half-resolution images' width, height
    half: vec4<f32>,
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var t_depth: texture_2d<f32>;
@group(0) @binding(2) var t_ao: texture_2d<f32>;
@group(0) @binding(3) var t_ms: texture_depth_multisampled_2d;

const HALF_PI: f32 = 1.57079633;
const BAYER = array<u32, 16>(0u, 8u, 2u, 10u, 12u, 4u, 14u, 6u, 3u, 11u, 1u, 9u, 15u, 7u, 13u, 5u);

fn view_z(d: f32) -> f32 {
    return linear_z(d, p.cam.z, p.cam.w);
}
fn clamp_px(q: vec2<i32>) -> vec2<i32> {
    return clamp(q, vec2<i32>(0), vec2<i32>(p.size.xy) - vec2<i32>(1));
}
fn raw_d(q: vec2<i32>) -> f32 {
    return textureLoad(t_depth, clamp_px(q), 0).r;
}
fn load_z(q: vec2<i32>) -> f32 {
    return view_z(raw_d(q));
}
// the view-space position (x right, y up, z forward) of pixel centre `c` at view depth `z`
fn view_pos(c: vec2<f32>, z: f32) -> vec3<f32> {
    let ndc = vec2<f32>(c.x * p.size.z * 2.0 - 1.0, 1.0 - c.y * p.size.w * 2.0);
    return vec3<f32>(ndc.x * p.cam.x * z, ndc.y * p.cam.y * z, z);
}
fn at(c: vec2<f32>) -> vec3<f32> {
    let q = vec2<i32>(floor(c));
    return view_pos(vec2<f32>(q) + 0.5, load_z(q));
}
const SKY: f32 = 0.99999;
// the radius on the screen: at most this many pixels, under this many nothing is done
const MAX_RADIUS_PX: f32 = 48.0;
const MIN_RADIUS_PX: f32 = 2.0;
// the falloff's share of the radius (XeGTAO's default 0.615)
const FALLOFF_RANGE: f32 = 0.615;
// the first sample's offset from the pixel (px)
const MIN_OFFSET_PX: f32 = 1.3;
// the apply's per-sample depth tolerance: relative, absolute (m)
const SAME_SURFACE_REL: f32 = 0.03;
const SAME_SURFACE_ABS: f32 = 0.05;

@fragment
fn fs_ao(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // PERF: this pass runs at half size; it measures the occlusion at the full-size pixel (2x + 1)
    let hpx = vec2<i32>(pos.xy);
    let px = hpx * 2 + vec2<i32>(1);
    let d0 = raw_d(px);
    let z0 = view_z(d0);
    if d0 >= SKY || z0 > p.ao.w {
        return vec4<f32>(1.0);
    }
    let radius = p.ao.x;
    let rpx = min(radius * p.size.y * 0.5 / p.cam.y / z0, MAX_RADIUS_PX);
    if rpx < MIN_RADIUS_PX {
        return vec4<f32>(1.0);
    }
    let c = vec2<f32>(px) + 0.5;
    let P = view_pos(c, z0);
    // the normal from the nearer neighbour on each axis (so it does not smear over a depth edge)
    let pl = at(c + vec2<f32>(-1.0, 0.0));
    let pr = at(c + vec2<f32>(1.0, 0.0));
    let pt = at(c + vec2<f32>(0.0, -1.0));
    let pb = at(c + vec2<f32>(0.0, 1.0));
    var hd = P - pl;
    if abs(pr.z - z0) < abs(pl.z - z0) {
        hd = pr - P;
    }
    var vd = P - pt;
    if abs(pb.z - z0) < abs(pt.z - z0) {
        vd = pb - P;
    }
    let N = normalize(cross(hd, vd));
    let V = normalize(-P);

    let idx = u32(hpx.x & 3) + 4u * u32(hpx.y & 3);
    let n_slice = (f32(BAYER[idx]) + 0.5) / 16.0;
    let n_step = (f32(BAYER[(idx * 7u + 5u) & 15u]) + 0.5) / 16.0;

    // the falloff: samples between 38 % and 100 % of the radius count less and less
    let fall_range = FALLOFF_RANGE * radius;
    let fall_mul = -1.0 / fall_range;
    let fall_add = radius * (1.0 - FALLOFF_RANGE) / fall_range + 1.0;

    let slices = i32(p.n.x);
    let steps = i32(p.n.y);
    var vis = 0.0;
    for (var sl = 0; sl < slices; sl++) {
        let phi = (f32(sl) + n_slice) * PI / f32(slices);
        let cs = cos(phi);
        let sn = sin(phi);
        let omega = vec2<f32>(cs, -sn);
        let dir = vec3<f32>(cs, sn, 0.0);
        let ortho = dir - dot(dir, V) * V;
        let axis = normalize(cross(dir, V));
        let proj_n = N - axis * dot(N, axis);
        let proj_len = length(proj_n);
        if proj_len < 0.0001 {
            vis += 1.0;
            continue;
        }
        let sign_n = sign(dot(ortho, proj_n));
        let cos_n = clamp(dot(proj_n, V) / proj_len, -1.0, 1.0);
        let n = sign_n * acos(cos_n);
        let low0 = cos(n + HALF_PI);
        let low1 = cos(n - HALF_PI);
        var h_cos0 = low0;
        var h_cos1 = low1;
        for (var st = 0; st < steps; st++) {
            var s = (f32(st) + n_step) / f32(steps);
            s = s * s;
            let off = MIN_OFFSET_PX + s * (rpx - MIN_OFFSET_PX);
            let s0 = at(c + omega * off) - P;
            let s1 = at(c - omega * off) - P;
            let l0 = length(s0);
            let l1 = length(s1);
            let w0 = clamp(l0 * fall_mul + fall_add, 0.0, 1.0);
            let w1 = clamp(l1 * fall_mul + fall_add, 0.0, 1.0);
            h_cos0 = max(h_cos0, mix(low0, dot(s0, V) / max(l0, 0.0001), w0));
            h_cos1 = max(h_cos1, mix(low1, dot(s1, V) / max(l1, 0.0001), w1));
        }
        var h0 = -acos(clamp(h_cos1, -1.0, 1.0));
        var h1 = acos(clamp(h_cos0, -1.0, 1.0));
        h0 = n + clamp(h0 - n, -HALF_PI, HALF_PI);
        h1 = n + clamp(h1 - n, -HALF_PI, HALF_PI);
        let sin_n = sin(n);
        let arc0 = (cos_n + 2.0 * h0 * sin_n - cos(2.0 * h0 - n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin_n - cos(2.0 * h1 - n)) * 0.25;
        vis += proj_len * (arc0 + arc1);
    }
    vis = clamp(vis / f32(slices), 0.0, 1.0);
    return vec4<f32>(pow(vis, p.n.z), 0.0, 0.0, 1.0);
}

// the depth-aware weight of a neighbour at view depth `zs` for a pixel at `zc`
fn bw(zs: f32, zc: f32) -> f32 {
    return clamp(1.0 - abs(zs - zc) / (0.03 * zc + 0.05), 0.0, 1.0);
}

fn clamp_h(q: vec2<i32>) -> vec2<i32> {
    return clamp(q, vec2<i32>(0), vec2<i32>(p.half.xy) - vec2<i32>(1));
}
// the full-size pixel a half-size one stands for
fn full_of(q: vec2<i32>) -> vec2<i32> {
    return q * 2 + vec2<i32>(1);
}

// a 5-tap depth-aware blur along one axis of the half-size image (`hpx` its pixel)
fn blur_h(hpx: vec2<i32>, horizontal: bool) -> f32 {
    let zc = load_z(full_of(hpx));
    var sum = 0.0;
    var wsum = 0.0;
    for (var i = -2; i <= 2; i++) {
        let q = clamp_h(hpx + select(vec2<i32>(0, i), vec2<i32>(i, 0), horizontal));
        let w = select(1.0, 0.5, abs(i) == 2) * select(1.0, bw(load_z(full_of(q)), zc), i != 0);
        sum += w * textureLoad(t_ao, q, 0).r;
        wsum += w;
    }
    return sum / wsum;
}

@fragment
fn fs_blur(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(blur_h(vec2<i32>(pos.xy), true), 0.0, 0.0, 1.0);
}

@fragment
fn fs_blur_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(blur_h(vec2<i32>(pos.xy), false), 0.0, 0.0, 1.0);
}

// the full-size occlusion at pixel `px` (view depth `zc`): the four nearest half-size texels, each by its bilinear weight times
// how near its depth is to this pixel's (so the occlusion of a far surface never bleeds over a silhouette). Evaluated by the
// apply passes themselves (PERF: no full-size occlusion image, no pass of its own; the value is no longer rounded to 8 bits).
fn up_vis(px: vec2<i32>, zc: f32) -> f32 {
    let hp = (vec2<f32>(px) + 0.5) * 0.5 - 0.5;
    let base = vec2<i32>(floor(hp));
    let f = hp - floor(hp);
    var sum = 0.0;
    var wsum = 0.0;
    var near = 1.0;
    var near_w = -1.0;
    for (var k = 0; k < 4; k++) {
        let o = vec2<i32>(k & 1, k >> 1);
        let q = clamp_h(base + o);
        let bl = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let w = bl * bw(load_z(full_of(q)), zc);
        let a = textureLoad(t_ao, q, 0).r;
        sum += w * a;
        wsum += w;
        if bl > near_w {
            near_w = bl;
            near = a;
        }
    }
    return select(near, sum / wsum, wsum > 0.001);
}

// the multiplier for a surface at view depth `zs` whose full-size occlusion is `vis`
fn multiplier(vis: f32, zs: f32) -> vec4<f32> {
    if zs > p.ao.w {
        return vec4<f32>(1.0);
    }
    let fade = 1.0 - smoothstep(p.ao.z, p.ao.w, zs);
    let m = 1.0 - p.ao.y * fade * (1.0 - vis);
    return vec4<f32>(m, m, m, 1.0);
}

@fragment
fn fs_apply_1(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(pos.xy);
    let d = raw_d(px);
    if d >= SKY {
        return vec4<f32>(1.0);
    }
    let zc = view_z(d);
    return multiplier(up_vis(px, zc), zc);
}

@fragment
fn fs_apply_ms(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // PERF: one multiplier per pixel (not per sample): a pixel whose samples are not all of one surface (a silhouette,
    // the sky's edge) is left alone, so no sample takes the occlusion of another surface
    let px = vec2<i32>(pos.xy);
    let zc = load_z(px);
    let n = i32(textureNumSamples(t_ms));
    for (var k = 0; k < n; k++) {
        let ds = textureLoad(t_ms, px, k);
        if ds >= SKY || abs(view_z(ds) - zc) > SAME_SURFACE_REL * zc + SAME_SURFACE_ABS {
            return vec4<f32>(1.0);
        }
    }
    return multiplier(up_vis(px, zc), zc);
}
