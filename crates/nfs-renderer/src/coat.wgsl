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
// a texel's per-pixel turn of the tap pattern: the shared interleaved gradient noise (`wgsl.rs` `ign`) of the pixel, the face shifting it
const HASH_FACE_SHIFT: f32 = 17.0;
const HASH_SHIFT_Y: f32 = 3.0;
// `radical_inverse`'s scale, 2^-32
const INV_2_POW_32: f32 = 2.3283064365386963e-10;
// the tangent frame's helper axis turns from +Y to +X where the normal is within this cosine of +-Y
const HELPER_TURN: f32 = 0.99;
// the radiance a tap keeps at most (the sun's disc, a lamp: a lobe is not one texel's glare), and the least weight / divisor
const TAP_MAX: f32 = 60.0;
const MIN_DIV: f32 = 0.0001;

// the same face pick and projection as `refl_sample` in shader.wgsl
fn cube_sample(d: vec3<f32>) -> vec4<f32> {
    let face = face_of(d);
    let fwd = face_fwd(face);
    let up = face_up(face);
    let right = cross(up, fwd);
    let z = max(dot(d, fwd), MIN_DIV);
    let u = 0.5 + 0.5 * dot(d, right) / z * p.a.w;
    let v = 0.5 - 0.5 * dot(d, up) / z * p.a.w;
    return textureSampleLevel(src, smp, vec2<f32>(u, v), face, 0.0);
}

fn radical_inverse(i0: u32) -> f32 {
    var i = i0;
    i = (i << 16u) | (i >> 16u);
    i = ((i & 0x55555555u) << 1u) | ((i & 0xAAAAAAAAu) >> 1u);
    i = ((i & 0x33333333u) << 2u) | ((i & 0xCCCCCCCCu) >> 2u);
    i = ((i & 0x0F0F0F0Fu) << 4u) | ((i & 0xF0F0F0F0u) >> 4u);
    i = ((i & 0x00FF00FFu) << 8u) | ((i & 0xFF00FF00u) >> 8u);
    return f32(i) * INV_2_POW_32;
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
    let helper = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > HELPER_TURN);
    let t1 = normalize(cross(helper, n));
    let t2 = cross(n, t1);
    // a per-texel turn of the pattern, so the taps make noise rather than a fixed ring
    let rot = ign(pos.xy + vec2<f32>(f32(face) * HASH_FACE_SHIFT, HASH_SHIFT_Y)) * TAU;
    let count = u32(p.b.x);
    let a2 = alpha * alpha;
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    // ENHANCEMENT E49 (NEAR-REFL2): the faces' alpha (the geometry mask, `gpu/refl.rs` `near_mask`) filtered by the same lobe
    var asum = 0.0;
    for (var i = 0u; i < count; i = i + 1u) {
        let xi = vec2<f32>((f32(i) + 0.5) / f32(count), radical_inverse(i));
        let phi = TAU * xi.x + rot;
        let cos_t = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
        let sin_t = sqrt(max(1.0 - cos_t * cos_t, 0.0));
        let h = normalize(t1 * (sin_t * cos(phi)) + t2 * (sin_t * sin(phi)) + n * cos_t);
        // V = N, so L = 2 (V.H) H - V
        let l = 2.0 * dot(n, h) * h - n;
        let nl = dot(n, l);
        if nl > 0.0 {
            let s = cube_sample(l);
            let c = min(max(s.rgb, vec3<f32>(0.0)), vec3<f32>(TAP_MAX));
            asum += s.a * nl;
            acc += pow(c, vec3<f32>(GAMMA)) * nl;
            wsum += nl;
        }
    }
    let lin = acc / max(wsum, MIN_DIV);
    return vec4<f32>(pow(lin, vec3<f32>(1.0 / GAMMA)), asum / max(wsum, MIN_DIV));
}
