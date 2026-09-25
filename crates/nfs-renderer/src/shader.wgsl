struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    sun_dir: vec4<f32>,
    ambient: vec4<f32>,
    cam_pos: vec4<f32>,
    headlight: mat4x4<f32>,
    headlight_on: vec4<f32>,
    beacon: mat4x4<f32>,
    beacon_on: vec4<f32>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
// `night.psh`'s `nght`, 128 × 128 bytes packed four to a u32
@group(0) @binding(1) var<uniform> night_map: array<vec4<u32>, 1024>;
// the car shadow projector's texture (`[0x6115CC]+0x18C`)
@group(0) @binding(2) var t_proj: texture_2d<f32>;
@group(0) @binding(3) var s_proj: sampler;
@group(0) @binding(4) var t_head: texture_2d<f32>;

fn night_byte(i: i32) -> f32 {
    let w = night_map[i / 16][(i / 4) % 4];
    return f32((w >> (8u * u32(i % 4))) & 0xFFu);
}

// 0x452910: the vertex taken into the headlight frame, then `row = z·1.28` (1…126), `col = x·3.62`
// (−62…62), `w = (nght[row][col + 64] − |y·32|) / 256`, all rounded as `fistp` does; 0x453279:
// a positive weight adds `round(128·w)` to each channel, clamped; either way the alpha is forced to
// 0xFF (0x45335D, 0x453377)
fn headlight_w(world: vec3<f32>) -> f32 {
    let p = globals.headlight * vec4<f32>(world, 1.0);
    let row = i32(round(p.z * 1.28));
    let col = i32(round(p.x * 3.62));
    if row <= 0 || row >= 127 || col <= -63 || col >= 63 {
        return 0.0;
    }
    let y = abs(round(p.y * 32.0));
    return max((night_byte(row * 128 + col + 64) - y) * 0.00390625, 0.0);
}

fn headlit(world: vec3<f32>, c: vec4<f32>) -> vec4<f32> {
    let w = headlight_w(world);
    if w <= 0.0 {
        return vec4<f32>(c.rgb, 1.0);
    }
    let add = round(128.0 * w);
    return vec4<f32>(min(round(c.rgb * 255.0) + vec3<f32>(add), vec3<f32>(255.0)) / 255.0, 1.0);
}

// `Headlights::LayerUVLight` 0x453CA0 (serialised class code 0x8004): the vertex goes into the
// same headlight frame `[0x61BD0C]` the vertex light uses (0x453D73 → 0x539F50), then
// `w = z + [0x5CEC50]` = z + 16 and `i = [0x5B24A8] / w` = 1/w (0x453E10…0x453E2E). `i > 0.1`
// ([0x5B2604]) clamps to `i = 0.1`, `w = 10` (0x453E45); a negative `i` — the point is behind the
// light — gives `u = x · 0.1 + 0.5`, `v = 0` (0x453E61…0x453E83, [0x5B23F0] = 0.5); otherwise
// `u = i·x + 0.5`, `v = i·y + 0.5` (0x453E8F…0x453EB5). The texture is `HEAD` of
// `Render/common.fsh`, fetched by that four-character code at 0x452645 into `[0x61E1D4]` and
// handed to the pass at 0x453D4D.
//
// The layer refuses to run unless `byte[0x61C16C]` says the vertex-light layer lit something
// (0x453CCB), so a piece entirely outside the beam takes no decal at all; here the gate is the
// same 0x452910 weight but **per vertex**, not per piece, which is the one place the footprint
// can differ from the original's.
//
// The pass's combine is read (`RE/notes/thrash.md` §6c): its `Rs` block, the `Opaque` method's
// data block under the second pass's key 0x52730003, is `{0, 2, −1, 1, 0}` — blending on, and state 0x38 = −1, which 0x540550 turns
// into Thrash 0x194 ← 7 and 0x195 ← 6, `D3DBLEND_DESTCOLOR` / `D3DBLEND_SRCCOLOR` in `dx7z.dll`
// (0x60005538 / 0x6000562B): the frame becomes `2 · src · dst`. The source is `HEAD` × the vertex
// colour, which in that pass is `LayerFullAmbient`'s white block (0x448C30, layer id 5 — the
// colour stream the draw copies, 0x540E1D). So the decal **multiplies the pass-1 pixel by
// 2 × `HEAD`**, 132/255 being the neutral grey and the pools brightening by up to 1.6. Gap,
// recorded not filled: 0x453EE1 multiplies the vertex's first float by `w`, not read.
// APPROXIMATION: the coordinates are clamped, which keeps the pools from tiling over the world
// (`HEAD`'s own address mode is not read).
fn uvlight(world: vec3<f32>) -> vec3<f32> {
    let p = globals.headlight * vec4<f32>(world, 1.0);
    let w = p.z + 16.0;
    var i = 1.0 / w;
    if i > 0.1 {
        i = 0.1;
    }
    var uv = vec2<f32>(p.x * 0.1 + 0.5, 0.0);
    if i >= 0.0 {
        uv = vec2<f32>(i * p.x + 0.5, i * p.y + 0.5);
    }
    let c = textureSampleLevel(t_head, s_proj, clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
    // textures are bytes (no sRGB decode), so the sample is the stored value the original
    // multiplies — 132/255 is the neutral point of the 2× (`DESTCOLOR` / `SRCCOLOR`)
    return 2.0 * c;
}

struct Object {
    model: mat4x4<f32>,
    env_mix: vec4<f32>,
    light: vec4<f32>,
    lit_a: vec4<f32>,
    lit_d: vec4<f32>,
    lit_dir: vec4<f32>,
    shadow_u: vec4<f32>,
    shadow_v: vec4<f32>,
    shadow_on: vec4<f32>,
};
@group(1) @binding(0) var<uniform> object: Object;

@group(2) @binding(0) var t_diffuse: texture_2d<f32>;
@group(2) @binding(1) var s_diffuse: sampler;
@group(2) @binding(2) var t_env: texture_2d<f32>;

// 0x452A50, the police beacon: the same `nght` map through the car's spinning frame — `c = z·3.5`
// ([0x5CEC6C]) kept strictly inside −127…127, `s = x·3.5` inside −63…63, and the row folded by its
// sign (0x452B0B: a negative `c` *subtracts* `c·128`), so `w = (nght[|c|·128 + s + 64] − y·16) /
// 512` ([0x5CEC70], [0x5CEC68]) with no absolute value on `y`, unlike the headlight's. Only a
// positive `w` adds (`fcomp [0x5B23F8]` = 0.0): each channel of the colour `+= round(channel·w)`
// clamped to 255 and the alpha forced to 0xFF (0x452BCD…0x452C9C). The colour is **red**
// ([0x61E1C4] = 0xFFFF0000) where the projected `z` is positive and **blue** ([0x61E1C8] =
// 0xFF0000FF) where its sign bit is set (0x452B95…0x452BA9), so the beam sweeps as the frame turns.
// Gap: the original leaves the weight of an out-of-range vertex at the *previous* vertex's (the
// scratch at `[ebp+8]` is zeroed once before the loop, 0x452ABB, exactly as the headlight's
// 0x4529B1 does) — a sequential artefact a shader cannot carry, so out-of-range vertices are unlit
// here, as they already are for the headlight.
fn beaconlit(world: vec3<f32>, c: vec4<f32>) -> vec4<f32> {
    let p = globals.beacon * vec4<f32>(world, 1.0);
    let row = i32(round(p.z * 3.5));
    let col = i32(round(p.x * 3.5));
    if row <= -127 || row >= 127 || col <= -63 || col >= 63 {
        return vec4<f32>(c.rgb, 1.0);
    }
    let w = (night_byte(abs(row) * 128 + col + 64) - round(p.y * 16.0)) * 0.001953125;
    if w <= 0.0 {
        return vec4<f32>(c.rgb, 1.0);
    }
    var tint = vec3<f32>(255.0, 0.0, 0.0);
    if p.z < 0.0 {
        tint = vec3<f32>(0.0, 0.0, 255.0);
    }
    return vec4<f32>(min(round(c.rgb * 255.0) + round(tint * w), vec3<f32>(255.0)) / 255.0, 1.0);
}

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
    /// 0x452910's weight, so the fragment can tell whether this vertex took headlight
    @location(4) head_w: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    // Thrash state 0x18, the depth bias (`lit_dir.w`, a pull in view depth): `dx7z.dll` adds it to
    // the screen z alone, so the vertex slides toward the eye along its own ray and stays where it
    // is on screen (`RE/notes/thrash.md` §6d.3); a vertex nearer than the pull is left alone
    let pull = object.lit_dir.w;
    if pull > 0.0 {
        let depth = -(globals.view * world).z;
        if depth > pull {
            let eye = globals.cam_pos.xyz;
            out.clip = globals.view_proj * vec4<f32>(eye + (world.xyz - eye) * (1.0 - pull / depth), 1.0);
        }
    }
    out.normal = normalize((object.model * vec4<f32>(in.normal, 0.0)).xyz);
    // env_mix.zw carries a per-object uv offset (the sky layers scroll with it)
    out.uv = in.uv + object.env_mix.zw;
    out.color = in.color;
    if object.env_mix.y > 0.5 {
        if globals.headlight_on.x > 0.5 {
            out.color = headlit(world.xyz, in.color);
        }
        if globals.beacon_on.x > 0.5 {
            out.color = beaconlit(world.xyz, out.color);
        }
    }
    out.world = world.xyz;
    out.head_w = 0.0;
    if object.shadow_on.y > 0.5 && globals.headlight_on.x > 0.5 {
        out.head_w = headlight_w(world.xyz);
    }
    return out;
}

// An opaque draw covers its pixel whatever the texture's alpha says: the original's back buffer
// has no alpha channel, and a car page keeps its texels' alpha (a wheel face's clear corners), which
// must not punch holes in a screenshot or in an off-screen view the front end lays over its screen.
//
// Both run, for a material with the race's states (`shadow_on.w`, the car's and the track's), the
// alpha test — `ALPHAFUNC` `GREATER` from the device (0x6000A50E, `[0x600180E8]` = 5) and the
// reference **10** the race sets through Thrash 0x24 (`dx7z.dll` 0x60003F9C; 0x443615, 0x452085,
// `RE/notes/thrash.md` §6d.4): a fragment whose alpha is 10/255 or less is dropped, colour and depth
// alike.
const ALPHA_REF: f32 = 10.5 / 255.0;

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if object.shadow_on.w > 0.5 && c.a < ALPHA_REF {
        discard;
    }
    return vec4<f32>(c.rgb, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if object.shadow_on.w > 0.5 && c.a < ALPHA_REF {
        discard;
    }
    return c;
}

fn shade(in: VsOut) -> vec4<f32> {
    // the material's mip LOD bias, Thrash state 8 (`RE/notes/thrash.md` §7), in `shadow_on.z`
    let tex = textureSampleBias(t_diffuse, s_diffuse, in.uv, object.shadow_on.z);
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
        // `R3DSpecularTextureLayer` 0x445520 (the layer class code 0x8100): the normal goes into
        // eye space through a matrix whose translation rows are zeroed (0x4456E6…0x445714), and
        // then, with `t = |z| + 1.0` ([0x5B2750]) and `r` the reciprocal square root of `t` —
        // the magic-constant estimate 0x5F400000 with one Newton step of 1.47 / 0.47
        // ([0x5B38D8] / [0x5B38E0]), which this port takes exactly — the coordinates are
        // `u = x·r·0.5 + 0.5` and `v = y·r·0.5 + 0.5` (0x4457C5…0x445828, [0x5B23F0] = 0.5).
        let ne = (globals.view * vec4<f32>(n, 0.0)).xyz;
        let r = inverseSqrt(abs(ne.z) + 1.0) * 0.5;
        let env = textureSample(t_env, s_diffuse, vec2<f32>(ne.x * r + 0.5, ne.y * r + 0.5));
        base = vec4<f32>(mix(base.rgb, env.rgb, mix_amt), base.a);
    }
    // `R3DShadowMapTextureLayer` (0x446130): the projector's planes over the world position.
    // APPROXIMATION: the pass's combine is not read — the texture (`sh80`, a flat dark colour under
    // its alpha) is laid over the lit colour by its alpha
    var lit = base.rgb * light;
    // the headlight decal, `Opaque`'s second pass: the first pass's pixel × 2 · `HEAD`
    if object.shadow_on.y > 0.5 && globals.headlight_on.x > 0.5 && in.head_w > 0.0 {
        lit = lit * uvlight(in.world);
    }
    if object.shadow_on.x > 0.5 {
        let uv = vec2<f32>(dot(object.shadow_u.xyz, in.world) + object.shadow_u.w, dot(object.shadow_v.xyz, in.world) + object.shadow_v.w);
        // `sh80`'s material asks for a LOD bias of −1 (`mt+0x40`, `RE/notes/thrash.md` §7)
        let sh = textureSampleBias(t_proj, s_proj, uv, -1.0);
        lit = mix(lit, sh.rgb, sh.a);
    }
    return vec4<f32>(lit, base.a * object.light.a);
}
