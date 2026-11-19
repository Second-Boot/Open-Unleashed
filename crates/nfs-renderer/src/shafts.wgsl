
struct U { sun: vec2<f32>, strength: f32, near: f32, tint: vec4<f32> };
@group(0) @binding(2) var<uniform> u: U;
@group(0) @binding(3) var rays: texture_2d<f32>;
@group(0) @binding(4) var rays_s: sampler;

// 1 where the depth is the sky's (nothing drawn), else 0
fn sky(p: vec2<i32>) -> f32 {
    return select(0.0, 1.0, depth_at(p) >= 0.99999);
}

const TAPS: i32 = 40;
// how much of the way to the sun a ray covers, the per-tap decay, the radius of the sun's glow
// (screen heights) and the overall gain
const DENSITY: f32 = 0.95;
const DECAY: f32 = 0.975;
const GLOW: f32 = 0.3;
const GAIN: f32 = 0.42;

// PERF: the rays are computed at half size (this pass), into an image the composite pass adds over the scene
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    // the centre of the 2 x 2 full-size pixels this half-size pixel stands for
    let fp = pos.xy * 2.0;
    let uv = fp / dims;
    let aspect = dims.x / dims.y;
    let step = (u.sun - uv) * (DENSITY / f32(TAPS));
    // a fixed per-pixel offset hides the banding of 40 taps (static: nothing crawls)
    let jit = ign(pos.xy);
    var p = uv + step * jit;
    var w = 1.0;
    var sum = 0.0;
    for (var i = 0; i < TAPS; i++) {
        let q = vec2<i32>(p * dims);
        if p.x >= 0.0 && p.y >= 0.0 && p.x < 1.0 && p.y < 1.0 {
            let d = length((p - u.sun) * vec2<f32>(aspect, 1.0)) / GLOW;
            sum += sky(q) * w / (1.0 + d * d * 3.0);
        }
        w *= DECAY;
        p += step;
    }
    // the rays fade with the pixel's own distance to the sun (a falloff centred on it)
    let dp = length((uv - u.sun) * vec2<f32>(aspect, 1.0)) / 0.5;
    let l = sum / f32(TAPS) * GAIN * u.strength / (1.0 + dp * dp * 2.0);
    return vec4<f32>(u.tint.rgb * l, 0.0);
}

// the composite: the half-size rays, bilinear, over the full-size scene, but not on the cockpit's own panels and
// pillars (closer than 1.5 m to the eye), which take no ray
@fragment fn fs_comp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let far = u.tint.w;
    let z = linear_z(depth_at(vec2<i32>(pos.xy)), u.near, far);
    if z < 1.5 {
        return vec4<f32>(0.0);
    }
    let dims = vec2<f32>(depth_dims());
    return vec4<f32>(textureSampleLevel(rays, rays_s, pos.xy / dims, 0.0).rgb, 0.0);
}
