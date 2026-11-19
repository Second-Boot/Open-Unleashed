
struct U {
    inv_vp: mat4x4<f32>,
    cam: vec4<f32>,
    fwd: vec4<f32>,
    air: vec4<f32>,
    gain: vec4<f32>,
    lamp_pos: array<vec4<f32>, HAZE_LAMPS_N>,
    lamp_col: array<vec4<f32>, HAZE_LAMPS_N>,
};
struct Spots {
    pos: array<vec4<f32>, MAX_SPOTS_N>,
    dir: array<vec4<f32>, MAX_SPOTS_N>,
    params: vec4<f32>,
    shadow: vec4<f32>,
    mat: mat4x4<f32>,
    origin: vec4<f32>,
};
@group(0) @binding(2) var<uniform> u: U;
@group(0) @binding(3) var rays: texture_2d<f32>;
@group(0) @binding(6) var<uniform> spots: Spots;
@group(0) @binding(7) var t_spot: texture_depth_2d;
@group(0) @binding(8) var s_shadow: sampler_comparison;

const STEPS: i32 = STEPS_N;
const CONE_COLOUR: vec3<f32> = vec3<f32>(1.0, 0.9, 0.7);
// the sky's distance (m) and the stored depth from which a pixel is sky
const SKY_DIST: f32 = 1000.0;
const SKY_DEPTH: f32 = 0.99999;
// the beams: the nearest a point counts to a lamp (m), the distance scale of the inverse-square falloff (m), the fade-in
// in front of the lamp (m: no halo on the bonnet), the phase's lobe (Henyey-Greenstein g)
const LAMP_MIN_DIST: f32 = 0.3;
const FALLOFF_DIST: f32 = 25.0;
const FADE_IN: vec2<f32> = vec2<f32>(3.0, 9.0);
const HG_G: f32 = 0.45;
// the march's image is 1 / MARCH_DIV of the scene's size; a march texel reads the scene's depth at its block's pixel
// MARCH_TAP (one of the two middle ones; 0 at half size)
const MARCH_DIV: f32 = MARCH_DIV_N;
const MARCH_TAP: f32 = MARCH_DIV / 2.0 - 1.0;
// the composite: the nearest a pixel takes haze (m: the cockpit's panels and pillars), the distance compare's cap (m), its
// relative and absolute tolerance (m)
const NEAR_CUT: f32 = 1.5;
const MATCH_CAP: f32 = 80.0;
const MATCH_REL: f32 = 0.15;
const MATCH_ABS: f32 = 0.3;

// the distance along the view ray to the scene, from the stored depth (the sky: a long way)
fn scene_dist(d: f32, dir: vec3<f32>) -> f32 {
    let near = u.cam.w;
    let far = u.fwd.w;
    if d >= SKY_DEPTH {
        return SKY_DIST;
    }
    let z = linear_z(d, near, far);
    return z / max(dot(dir, u.fwd.xyz), 0.05);
}

// E15's shadow map of the player's lamp pair at a point of the air: the same lookup as the surfaces', the point
// pushed toward the lamp by a texel at its distance
fn spot_visibility(world: vec3<f32>) -> f32 {
    let o = spots.origin.xyz;
    let to = normalize(o - world);
    let dist = length(world - o);
    let texel = spots.shadow.y * dist;
    let p = spots.mat * vec4<f32>(world + to * texel, 1.0);
    let near = spots.shadow.z;
    let far = spots.shadow.w;
    if p.w <= near {
        return 1.0;
    }
    let ndc = p.xyz / p.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let bias = (0.05 + 1.5 * texel) * far * near / ((far - near) * p.w * p.w);
    let depth = ndc.z - bias;
    let st = spots.origin.w;
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * st, depth);
    }
    let edge = max(abs(ndc.x), abs(ndc.y));
    return mix(sum * 0.25, 1.0, smoothstep(0.85, 1.0, edge));
}

// the scattering phase of a headlight beam toward the eye: a flat part and a forward-leaning Henyey-Greenstein lobe
// (g 0.45); `c` is the cosine between the light's travel and the direction to the eye
fn phase(c: f32) -> f32 {
    let g = HG_G;
    let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * c, 1.5);
    return 0.5 + 0.5 * hg;
}

// PERF: the march at 1 / MARCH_DIV size; the alpha keeps the distance to the scene for the composite's depth-aware blur
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    let fp = min(floor(pos.xy) * MARCH_DIV + MARCH_TAP, dims - 1.0);
    let uv = (fp + 0.5) / dims;
    let dir = view_dir(uv);
    let ts = scene_dist(depth_at(vec2<i32>(fp)), dir);
    let tmax = min(ts, u.air.z);
    var sum = vec3<f32>(0.0);
    if tmax > 0.05 {
        // a static per-pixel offset hides the banding of the few steps (nothing crawls)
        let jit = ign(pos.xy);
        let eye = u.cam.xyz;
        let n_spots = i32(spots.params.x);
        let n_lamps = i32(u.air.w);
        let sig_beam = u.air.x;
        let sig_lamp = u.air.y;
        for (var i = 0; i < STEPS; i++) {
            let s = (f32(i) + jit) / f32(STEPS);
            let t = tmax * s * s;
            let dt = tmax * 2.0 * s / f32(STEPS);
            let p = eye + dir * t;
            var e = vec3<f32>(0.0);
            if sig_beam > 0.0 && n_spots > 0 {
                var vis = -1.0;
                for (var k = 0; k < n_spots; k++) {
                    let lv = p - spots.pos[k].xyz;
                    let d = max(length(lv), LAMP_MIN_DIST);
                    let l = lv / d;
                    let cone = smoothstep(spots.params.y, spots.params.z, dot(l, spots.dir[k].xyz));
                    if cone <= 0.0 {
                        continue;
                    }
                    let r = clamp(1.0 - d / spots.params.w, 0.0, 1.0);
                    if r <= 0.0 {
                        continue;
                    }
                    let d25 = d / FALLOFF_DIST;
                    // no halo on the bonnet: the beam's air starts a few metres ahead of the lamp and fades in over 3-9 m
                    var w = spots.pos[k].w * cone * r * r / (1.0 + d25 * d25) * smoothstep(FADE_IN.x, FADE_IN.y, d);
                    if f32(k) < spots.shadow.x {
                        if vis < 0.0 {
                            vis = spot_visibility(p);
                        }
                        w = w * vis;
                    }
                    // light travels along l; it reaches the eye along -dir
                    e += CONE_COLOUR * (w * phase(dot(l, -dir)));
                }
                e = e * (sig_beam * u.gain.x * exp(-sig_beam * 0.5 * t));
            }
            var g = vec3<f32>(0.0);
            if sig_lamp > 0.0 {
                for (var k = 0; k < n_lamps; k++) {
                    let d = length(p - u.lamp_pos[k].xyz);
                    let r = clamp(1.0 - d / u.lamp_pos[k].w, 0.0, 1.0);
                    g += u.lamp_col[k].rgb * (r * r);
                }
                g = g * (sig_lamp * u.gain.y);
            }
            sum += (e + g) * dt;
        }
    }
    return vec4<f32>(sum, min(ts, SKY_DIST));
}

// the composite: the four nearest march texels weighted bilinearly and by how well their distance matches this
// pixel's, over the full-size scene; the cockpit's own panels and pillars (closer than 1.5 m) take none
@fragment fn fs_comp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    let p = vec2<i32>(pos.xy);
    let dir = view_dir(pos.xy / dims);
    let ts = scene_dist(depth_at(p), dir);
    if ts < NEAR_CUT {
        return vec4<f32>(0.0);
    }
    let march_dims = vec2<i32>(textureDimensions(rays));
    // this pixel's place in the march's texel space (texel centres at integer + 0.5)
    let hp = pos.xy / MARCH_DIV - 0.5;
    let base = vec2<i32>(floor(hp));
    let f = hp - floor(hp);
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var k = 0; k < 4; k++) {
        let o = vec2<i32>(k & 1, k >> 1);
        let q = clamp(base + o, vec2<i32>(0), march_dims - 1);
        let tex = textureLoad(rays, q, 0);
        let bl = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let dd = abs(min(tex.a, MATCH_CAP) - min(ts, MATCH_CAP)) / (MATCH_REL * min(ts, MATCH_CAP) + MATCH_ABS);
        let w = bl * exp(-dd * dd) + 1e-4;
        acc += tex.rgb * w;
        wsum += w;
    }
    return vec4<f32>(acc / wsum, 0.0);
}
