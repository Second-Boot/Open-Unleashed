struct Globals {
    view_proj: mat4x4<f32>,
    sun_dir: vec4<f32>,
    ambient: vec4<f32>,
    cam_pos: vec4<f32>,
    headlight: mat4x4<f32>,
    headlight_on: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
// `night.psh`'s `nght`, 128 × 128 bytes packed four to a u32
@group(0) @binding(1) var<uniform> night_map: array<vec4<u32>, 1024>;

fn night_byte(i: i32) -> f32 {
    let w = night_map[i / 16][(i / 4) % 4];
    return f32((w >> (8u * u32(i % 4))) & 0xFFu);
}

// 0x452910: the vertex taken into the headlight frame, then `row = z·1.28` (1…126), `col = x·3.62`
// (−62…62), `w = (nght[row][col + 64] − |y·32|) / 256`, all rounded as `fistp` does; 0x453279:
// a positive weight adds `round(128·w)` to each channel, clamped; either way the alpha is forced to
// 0xFF (0x45335D, 0x453377)
fn headlit(world: vec3<f32>, c: vec4<f32>) -> vec4<f32> {
    let p = globals.headlight * vec4<f32>(world, 1.0);
    let row = i32(round(p.z * 1.28));
    let col = i32(round(p.x * 3.62));
    if row <= 0 || row >= 127 || col <= -63 || col >= 63 {
        return vec4<f32>(c.rgb, 1.0);
    }
    let y = abs(round(p.y * 32.0));
    let w = (night_byte(row * 128 + col + 64) - y) * 0.00390625;
    if w <= 0.0 {
        return vec4<f32>(c.rgb, 1.0);
    }
    let add = round(128.0 * w);
    return vec4<f32>(min(round(c.rgb * 255.0) + vec3<f32>(add), vec3<f32>(255.0)) / 255.0, 1.0);
}

struct Object {
    model: mat4x4<f32>,
    env_mix: vec4<f32>,
    light: vec4<f32>,
    lit_a: vec4<f32>,
    lit_d: vec4<f32>,
    lit_dir: vec4<f32>,
};
@group(1) @binding(0) var<uniform> object: Object;

@group(2) @binding(0) var t_diffuse: texture_2d<f32>;
@group(2) @binding(1) var s_diffuse: sampler;
@group(2) @binding(2) var t_env: texture_2d<f32>;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) world: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    out.normal = normalize((object.model * vec4<f32>(in.normal, 0.0)).xyz);
    // env_mix.zw carries a per-object uv offset (the sky layers scroll with it)
    out.uv = in.uv + object.env_mix.zw;
    out.color = in.color;
    if globals.headlight_on.x > 0.5 && object.env_mix.y > 0.5 {
        out.color = headlit(world.xyz, in.color);
    }
    out.world = world.xyz;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(t_diffuse, s_diffuse, in.uv);
    let n = normalize(in.normal);
    let diffuse = max(dot(n, normalize(globals.sun_dir.xyz)), 0.0);
    // the per-object light multiplies the ambient term only: the original keeps one ambient slot
    // per `.map` header colour and lights the car from the ground patch it stands on
    var light = globals.ambient.x * object.light.rgb + vec3<f32>(1.0 - globals.ambient.x) * diffuse;
    // `R3DLightLayer` (0x446530): the slot's colour, plus its partner along the reversed light
    // direction where the normal faces it, clamped per channel. The original does this per
    // vertex; here it is per pixel of the interpolated normal.
    if object.lit_a.w > 0.5 {
        // a zero direction normalises to NaN in the original (0x4E8791), whose sign bit sends every
        // vertex to the ambient arm (0x446944) — so no directional term
        var d = 0.0;
        if length(object.lit_dir.xyz) > 0.0 {
            d = max(dot(n, -normalize(object.lit_dir.xyz)), 0.0);
        }
        light = min(object.lit_a.rgb + object.lit_d.rgb * d, vec3<f32>(1.0));
    }
    var base = tex * in.color;
    let mix_amt = object.env_mix.x;
    if mix_amt > 0.0 {
        // sphere-map lookup of the reflected view vector (approximation of the original's env layer)
        let v = normalize(globals.cam_pos.xyz - in.world);
        let r = reflect(-v, n);
        let m = 2.0 * sqrt(r.x * r.x + r.y * r.y + (r.z + 1.0) * (r.z + 1.0)) + 1e-4;
        let env = textureSample(t_env, s_diffuse, vec2<f32>(r.x / m + 0.5, 0.5 - r.y / m));
        base = vec4<f32>(mix(base.rgb, env.rgb, mix_amt), base.a);
    }
    return vec4<f32>(base.rgb * light, base.a * object.light.a);
}
