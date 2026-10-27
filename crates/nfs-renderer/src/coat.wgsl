// ENHANCEMENT ENH-COAT (E8): the prefiltered mip chain of the environment cube. One fragment pass per (mip, face):
// the texel's direction is the normal = the view direction N = V, and the pass averages the source mip (the one above)
// over a GGX lobe of width `alpha` (importance sampled: `Hammersley(i)` turned per texel), in linear light and written
// back in the cube's display values. The cube's faces are the shader's own (`shader.wgsl` `refl_sample`):
// +X -X +Z -Z +Y -Y, 96 degrees across (`1 / tan(half fov)` = `p.a.w`), screen right = `up x fwd`.

struct Params {
    // x the face, y the target's size in texels, z the lobe's alpha, w 1 / tan(half fov)
    a: vec4<f32>,
    // x the sample count
    b: vec4<f32>,
}

@group(0) @binding(0) var src: texture_2d_array<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> p: Params;

const GAMMA: f32 = 2.2;

fn face_fwd(face: i32) -> vec3<f32> {
    switch face {
        case 0: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 1: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2: { return vec3<f32>(0.0, 0.0, 1.0); }
        case 3: { return vec3<f32>(0.0, 0.0, -1.0); }
        case 4: { return vec3<f32>(0.0, 1.0, 0.0); }
        default: { return vec3<f32>(0.0, -1.0, 0.0); }
    }
}

fn face_up(face: i32) -> vec3<f32> {
    switch face {
        case 4: { return vec3<f32>(0.0, 0.0, 1.0); }
        case 5: { return vec3<f32>(0.0, 0.0, -1.0); }
        default: { return vec3<f32>(0.0, 1.0, 0.0); }
    }
}

// the same face pick and projection as `refl_sample` in shader.wgsl
fn cube_sample(d: vec3<f32>) -> vec3<f32> {
    let a = abs(d);
    var face = 0;
    if a.x >= a.y && a.x >= a.z {
        if d.x < 0.0 {
            face = 1;
        }
    } else if a.z >= a.y {
        face = 2;
        if d.z < 0.0 {
            face = 3;
        }
    } else {
        face = 4;
        if d.y < 0.0 {
            face = 5;
        }
    }
    let fwd = face_fwd(face);
    let up = face_up(face);
    let right = cross(up, fwd);
    let z = max(dot(d, fwd), 0.0001);
    let u = 0.5 + 0.5 * dot(d, right) / z * p.a.w;
    let v = 0.5 - 0.5 * dot(d, up) / z * p.a.w;
    return textureSampleLevel(src, smp, vec2<f32>(u, v), face, 0.0).rgb;
}

fn radical_inverse(i0: u32) -> f32 {
    var i = i0;
    i = (i << 16u) | (i >> 16u);
    i = ((i & 0x55555555u) << 1u) | ((i & 0xAAAAAAAAu) >> 1u);
    i = ((i & 0x33333333u) << 2u) | ((i & 0xCCCCCCCCu) >> 2u);
    i = ((i & 0x0F0F0F0Fu) << 4u) | ((i & 0xF0F0F0F0u) >> 4u);
    i = ((i & 0x00FF00FFu) << 8u) | ((i & 0xFF00FF00u) >> 8u);
    return f32(i) * 2.3283064365386963e-10;
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let face = i32(p.a.x);
    let size = p.a.y;
    let alpha = p.a.z;
    let tan_half = 1.0 / p.a.w;
    let su = pos.x / size;
    let sv = pos.y / size;
    let fwd = face_fwd(face);
    let up = face_up(face);
    let right = cross(up, fwd);
    let n = normalize(fwd + right * ((2.0 * su - 1.0) * tan_half) + up * ((1.0 - 2.0 * sv) * tan_half));
    // a tangent frame of the normal
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.99);
    let t1 = normalize(cross(helper, n));
    let t2 = cross(n, t1);
    // a per-texel turn of the pattern, so the taps make noise rather than a fixed ring
    let rot = fract(sin(dot(pos.xy + vec2<f32>(f32(face) * 17.0, 3.0), vec2<f32>(12.9898, 78.233))) * 43758.5453) * 6.2831853;
    let count = u32(p.b.x);
    let a2 = alpha * alpha;
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let xi = vec2<f32>((f32(i) + 0.5) / f32(count), radical_inverse(i));
        let phi = 6.2831853 * xi.x + rot;
        let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
        let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
        let h = normalize(t1 * (sin_t * cos(phi)) + t2 * (sin_t * sin(phi)) + n * cos_t);
        // V = N, so L = 2 (V.H) H - V
        let l = 2.0 * dot(n, h) * h - n;
        let nl = dot(n, l);
        if nl > 0.0 {
            let c = min(max(cube_sample(l), vec3<f32>(0.0)), vec3<f32>(60.0));
            acc += pow(c, vec3<f32>(GAMMA)) * nl;
            wsum += nl;
        }
    }
    let lin = acc / max(wsum, 0.0001);
    return vec4<f32>(pow(lin, vec3<f32>(1.0 / GAMMA)), 1.0);
}
