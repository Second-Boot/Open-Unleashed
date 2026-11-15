// ENHANCEMENT E16 — the car's ambient probe: the six faces of E8's environment map projected to
// second-order spherical harmonics (9 coefficients x rgb), convolved to irradiance, normalised so the
// upper-hemisphere mean of its luminance is 1, and smoothed in time.
//
// `sh_partial`: one workgroup per face (16 x 16 threads = 16 x 16 cells of the face's tangent plane
// [-1, 1]^2, each cell the mean of 4 x 4 taps of the face), the cell's radiance x SH basis x solid angle,
// reduced in shared memory. `sh_final`: one thread sums the six faces, applies the cosine lobe
// (pi, 2 pi / 3, pi / 4), divides by the mean's luminance, and blends into the stored state.

@group(0) @binding(0) var t_faces: texture_2d_array<f32>;
@group(0) @binding(1) var<storage, read_write> partial: array<vec4<f32>, 54>;
// 9 x (coefficient rgb, 0), then (initialised, 0, 0, 0)
@group(0) @binding(2) var<storage, read_write> state: array<vec4<f32>, 10>;

struct Params {
    // x the face size in texels, y 1 / tan(half fov) of a face, z the blend factor of this update, w 1 = take the result as it is
    p: vec4<f32>,
};
@group(0) @binding(3) var<uniform> params: Params;

var<workgroup> red: array<vec3<f32>, 256>;

// (the SH basis / luma constants and the face table are the mesh shader's pieces, `classes.rs` `SH_LUMA` / `faces_wgsl`, put in
// front of this file by `probe::source`)

// the cosine lobe's band weights (l = 0, 1, 2: pi, 2 pi / 3, pi / 4), the upper hemisphere's mean of the band-1 y term
// (SH_Y1 / 2), and the least luminance of a mean that is normalised (below: a black map, the flat probe)
const LOBE_L0: f32 = 3.14159265;
const LOBE_L1: f32 = 2.0943951;
const LOBE_L2: f32 = 0.78539816;
const UPPER_Y1: f32 = 0.244301;
const MIN_LUM: f32 = 0.002;
// a face's tangent plane is cut into CELLS x CELLS cells (one thread each), each the mean of TAPS x TAPS taps; the tap grid's
// offset from a cell's centre (taps), and the cells' per-workgroup reduction (half of CELLS x CELLS)
const CELLS: f32 = 16.0;
const TAPS: i32 = 4;
const TAP_CENTRE: f32 = 1.5;
const TAP_COUNT: f32 = 16.0;
const REDUCE_HALF: u32 = 128u;
// a tangent-plane cell's solid angle: its area / (1 + a^2 + b^2)^(3/2)
const SOLID_ANGLE_POW: f32 = 1.5;

fn basis(d: vec3<f32>, k: u32) -> f32 {
    switch k {
        case 0u: { return SH_Y00; }
        case 1u: { return SH_Y1 * d.y; }
        case 2u: { return SH_Y1 * d.z; }
        case 3u: { return SH_Y1 * d.x; }
        case 4u: { return SH_Y2 * d.x * d.y; }
        case 5u: { return SH_Y2 * d.y * d.z; }
        case 6u: { return SH_Y20 * (3.0 * d.z * d.z - 1.0); }
        case 7u: { return SH_Y2 * d.x * d.z; }
        default: { return SH_Y22 * (d.x * d.x - d.y * d.y); }
    }
}

@compute @workgroup_size(16, 16)
fn sh_partial(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let face = wg.x;
    let fwd = face_fwd(i32(face));
    let up = face_up(i32(face));
    let right = cross(up, fwd);
    let size = i32(params.p.x);
    let k = params.p.y;
    let cw = 2.0 / CELLS;
    // the cell's centre on the tangent plane, and the 4 x 4 taps inside it
    let a0 = (f32(lid.x) + 0.5) * cw - 1.0;
    let b0 = (f32(lid.y) + 0.5) * cw - 1.0;
    var sum = vec3<f32>(0.0);
    for (var ty = 0; ty < TAPS; ty++) {
        for (var tx = 0; tx < TAPS; tx++) {
            let a = a0 + (f32(tx) - TAP_CENTRE) * cw * 0.25;
            let b = b0 + (f32(ty) - TAP_CENTRE) * cw * 0.25;
            let u = 0.5 + 0.5 * a * k;
            let v = 0.5 - 0.5 * b * k;
            let xy = clamp(vec2<i32>(i32(u * f32(size)), i32(v * f32(size))), vec2<i32>(0), vec2<i32>(size - 1));
            // ENHANCEMENT ENH-COAT: the float cube holds radiance above 1 (the sun, lamps); the ambient takes it at white, as it did
            sum += min(textureLoad(t_faces, xy, i32(face), 0).rgb, vec3<f32>(1.0));
        }
    }
    let c = sum / TAP_COUNT;
    let d = normalize(fwd + a0 * right + b0 * up);
    let w = cw * cw / pow(1.0 + a0 * a0 + b0 * b0, SOLID_ANGLE_POW);
    for (var i = 0u; i < 9u; i++) {
        red[tid] = c * (basis(d, i) * w);
        workgroupBarrier();
        for (var s = REDUCE_HALF; s > 0u; s = s >> 1u) {
            if tid < s {
                red[tid] += red[tid + s];
            }
            workgroupBarrier();
        }
        if tid == 0u {
            partial[face * 9u + i] = vec4<f32>(red[0], 0.0);
        }
        workgroupBarrier();
    }
}

@compute @workgroup_size(1)
fn sh_final() {
    var e: array<vec3<f32>, 9>;
    for (var i = 0u; i < 9u; i++) {
        var l = vec3<f32>(0.0);
        for (var f = 0u; f < 6u; f++) {
            l += partial[f * 9u + i].xyz;
        }
        // the cosine lobe: l = 0, 1, 2
        var a = LOBE_L0;
        if i >= 1u && i <= 3u {
            a = LOBE_L1;
        } else if i >= 4u {
            a = LOBE_L2;
        }
        e[i] = l * a;
    }
    // normalise by the mean over the UPPER hemisphere (y > 0), the half of the normals a car shows (no
    // underside): (1 / 2 pi) * (2 pi Y00 E0 + pi * 0.488603 E(y)); the l = 2 terms integrate to 0 over it
    let mean = e[0] * SH_Y00 + e[1] * UPPER_Y1;
    let lum = dot(mean, LUMA);
    var out: array<vec3<f32>, 9>;
    if lum > MIN_LUM {
        for (var i = 0u; i < 9u; i++) {
            out[i] = e[i] / lum;
        }
    } else {
        // no light at all (a black map): the flat probe, irradiance 1 everywhere
        for (var i = 0u; i < 9u; i++) {
            out[i] = vec3<f32>(0.0);
        }
        out[0] = vec3<f32>(1.0 / SH_Y00);
    }
    let first = state[9].x < 0.5 || params.p.w > 0.5;
    for (var i = 0u; i < 9u; i++) {
        var prev = state[i].xyz;
        if first {
            prev = out[i];
        }
        state[i] = vec4<f32>(mix(prev, out[i], params.p.z), 0.0);
    }
    state[9] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
}
