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
    fog: vec4<f32>,
    sky_fog: vec4<f32>,
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
// 2 × `HEAD`**, 132/255 being the neutral grey and the pools brightening by up to 1.6.
//
// **The divide is per pixel** (read 2026-09-26). After the coordinates, 0x453EDB…0x453EE5 loads the
// vertex's `w` (`[0x61C17C + 16·k]`: `z + 16`, or the 10.0 the clamp and the behind-the-light arm
// write) and multiplies it into the float at `edi` = `ctx+0x440` (0x453D05 / 0x453D26), the
// vertex's **`rhw`**: the pipeline vertex starts at `ctx+0x434` (0x5408F0 hands that to 0x5385B0,
// 0x540985), and the finishing loop writes `rhw = [0x5E1980] / z` = 1 / view depth at `+0xC`
// (0x5B12AE…0x5B12EA). The decal pass has no `LayerCoords` op after the layer, and the draw
// 0x540CE0 writes only `+0x10`, `+0x14` and the UVs, so the scaled `rhw` reaches the card. `rhw`
// is what the card's perspective-correct interpolation weights the UVs by (D3D7's
// `TEXTUREPERSPECTIVE`, on by default — whether the game ever turns it off is not read; the depth test uses `sz`,
// Thrash state 4 never asks for a W-buffer — `thrash.md` §6), so across a triangle the texel is
// `Σ b·rhw·w·u / Σ b·rhw·w`, and with `u·w = x + 0.5·w`, `v·w = y + 0.5·w` that is the projection
// divided **per pixel**: the classic projective texture. Here the vertex stage hands on `(u·w,
// v·w, w)` — the GPU's own interpolation weights by `1 / clip.w`, the same view depth — and the
// fragment divides. A triangle whose corners took different arms (clamped, behind) interpolates
// between them exactly as the card does.
//
// `HEAD` is sampled `CLAMP` both ways (read 2026-09-26): `common.fsh` is opened by 0x448B10 through
// 0x44D270, which hands each entry to 0x44D380 with no state block (0x44D2E9 pushes 0), so the
// loader 0x44D550 leaves the texture alone (0x44D558) and the constructor's default block 0x5379D0
// stands — `{0, 1, 0, 0, 0, 0, 0}`: `LINEAR`, mip `NONE`, and Thrash 0x41 / 0x42 = 0, which
// `dx7z.dll` makes `D3DTADDRESS_CLAMP` (0x60003DB6 / 0x60003E1E push 3). Nothing on the layer's path
// touches the block: 0x453D4D only stores the texture pointer in the slot, and unlike the
// specular binder 0x440940 there is no 0x537A70 / 0x536470 call; the draw's stage binder 0x536240
// applies the texture's own block. So the pools do not tile — the projector sampler `s_proj`
// (`ClampToEdge`, `Linear`, level 0) carries it. *(Until 2026-09-26 this was an APPROXIMATION
// clamping the coordinates in the shader.)*
fn uvlight_vertex(world: vec3<f32>) -> vec3<f32> {
    let p = globals.headlight * vec4<f32>(world, 1.0);
    var w = p.z + 16.0;
    var i = 1.0 / w;
    if i > 0.1 {
        i = 0.1;
        w = 10.0;
    }
    var uv = vec2<f32>(p.x * 0.1 + 0.5, 0.0);
    if i >= 0.0 {
        uv = vec2<f32>(i * p.x + 0.5, i * p.y + 0.5);
    } else {
        w = 10.0;
    }
    // 0x453EE1: `rhw ·= w`
    return vec3<f32>(uv * w, w);
}

fn uvlight(uvw: vec3<f32>) -> vec3<f32> {
    let c = textureSampleLevel(t_head, s_proj, uvw.xy / uvw.z, 0.0).rgb;
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
    sky_u: vec4<f32>,
    sky_v: vec4<f32>,
    sky_on: vec4<f32>,
};
// every object of the pass sits in one read-only storage buffer, bound once per pass; a draw names
// its object by `first_instance` (`instance_index`), which the vertex stage hands to the fragment
// stage as a flat varying. `object` is a private copy loaded at the head of each entry point, so
// the maths below reads the same values as it did from the per-draw dynamic-offset uniform.
@group(1) @binding(0) var<storage, read> objects: array<Object>;
var<private> object: Object;

@group(2) @binding(0) var t_diffuse: texture_2d<f32>;
@group(2) @binding(1) var s_diffuse: sampler;
@group(2) @binding(2) var t_env: texture_2d<f32>;
@group(2) @binding(3) var s_env: sampler;
// the car's section map for `R3DSkyMapTextureLayer` (`[0x6115CC]+0xD0 + 4·bucket`)
@group(3) @binding(0) var t_sky: texture_2d<f32>;

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
    /// `R3DSkyMapTextureLayer`'s coordinates, made per vertex as the layer makes them
    @location(5) sky_uv: vec2<f32>,
    /// `Headlights::LayerUVLight`'s `(u·w, v·w, w)`, divided per fragment (0x453EE1)
    @location(6) head_uvw: vec3<f32>,
    /// the draw's object, `instance_index` (index into `objects`)
    @location(7) @interpolate(flat) obj: u32,
};

@vertex
fn vs_main(in: VsIn, @builtin(instance_index) inst: u32) -> VsOut {
    object = objects[inst];
    var out: VsOut;
    out.obj = inst;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = globals.view_proj * world;
    // Thrash state 0x18, the depth bias (`lit_dir.w`, a pull in view depth): `dx7z.dll` adds it to
    // the screen z alone, so the vertex slides toward the eye along its own ray and stays where it
    // is on screen (`RE/notes/thrash.md` §6d.3). The driver adds the bias to the screen z as it
    // is, so a pull past the near plane leaves the original's vertex at z ≤ 0 — in front of
    // everything (the lamp glare's `−20000 · b · K / d²` within some 8 m); here it stops just
    // inside the plane (`globals.ambient.y`), where it is not clipped and still passes any depth
    // A positive `b` (some car materials' 1, 3, 5) pushes the vertex away the same way.
    let pull = object.lit_dir.w;
    if pull != 0.0 {
        let depth = -(globals.view * world).z;
        if depth > 0.0 {
            let pulled = max(depth - pull, globals.ambient.y * 1.001);
            if pulled != depth {
                let eye = globals.cam_pos.xyz;
                out.clip = globals.view_proj * vec4<f32>(eye + (world.xyz - eye) * (pulled / depth), 1.0);
            }
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
    // `R3DSkyMapTextureLayer` 0x445B10 (class 0x8101), per vertex (0x445DDC…0x445E42): the point
    // `normal + 3.33 · position` of the part's own streams — the layer's first reference is the
    // `nm` stream (id 1, key 6), added as it is, its second the `vt` stream (id 1, key 7), scaled
    // by `[0x6115CC]+0` = 3.33 (0x440783) — goes through the object's matrix, less the car's
    // `car+0x330`, and onto the slice's lateral and forward axes (the transposed `car+0x3B8`
    // frame, 0x43A862…0x43A8A4); the planes carry that and the section's 1 / `texturesize`,
    // `aspect` and the offsets (`RE/notes/lighting.md` §3.2)
    out.sky_uv = vec2<f32>(0.0);
    if object.sky_on.x > 0.5 {
        let p = (object.model * vec4<f32>(in.normal + 3.33 * in.position, 1.0)).xyz;
        out.sky_uv = vec2<f32>(dot(object.sky_u.xyz, p) + object.sky_u.w, dot(object.sky_v.xyz, p) + object.sky_v.w);
    }
    out.head_w = 0.0;
    out.head_uvw = vec3<f32>(0.0, 0.0, 1.0);
    if object.shadow_on.y > 0.5 && globals.headlight_on.x > 0.5 {
        out.head_w = headlight_w(world.xyz);
        out.head_uvw = uvlight_vertex(world.xyz);
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
// alike. `shadow_on.w` carries the draw's reference + ½ in bytes (0: no test) — 10 but for a draw
// that inherits a particle system's 1 (0x454456, `DrawItem::alpha_ref`).

// The weather fog (`Scene::fog`, `Scene::sky_fog`; 0x45D2E0 / 0x45D3B0): Direct3D 7's table fog
// `EXP` on the vertex's W, the view depth in metres (`nfs_renderer::Fog`), `f = exp(−density · d)`
// and the pixel `f · c + (1 − f) · fog`, ahead of the blend. The sky pass's items take its own fog
// (the eye it moves to the origin keeps the view depth). A density of 0 is the fog off.
fn fogged(in: VsOut, c: vec4<f32>) -> vec4<f32> {
    var fog = globals.fog;
    if object.sky_on.y > 0.5 {
        fog = globals.sky_fog;
    }
    if fog.w <= 0.0 {
        return c;
    }
    var d = max(-(globals.view * vec4<f32>(in.world, 1.0)).z, 0.0);
    // .z of `sky_on`: an item whose W is its own (the sun's screen quad, `DrawItem::fog_w`)
    if object.sky_on.z > 0.0 {
        d = object.sky_on.z;
    }
    let f = exp(-fog.w * d);
    return vec4<f32>(mix(fog.rgb, c.rgb, f), c.a);
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade(in);
    if object.shadow_on.w > 0.0 && c.a * 255.0 < object.shadow_on.w {
        discard;
    }
    return vec4<f32>(fogged(in, c).rgb, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade(in);
    if object.shadow_on.w > 0.0 && c.a * 255.0 < object.shadow_on.w {
        discard;
    }
    return fogged(in, c);
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
    // `CarExtEnv`'s pass 1 (option mask 2): its `Rs` block is `{1, 2, 0, 1, 0, −1}` — gouraud,
    // blending on, pair 0 `SRCALPHA` / `INVSRCALPHA` — and its ops are only that block, the
    // `R3DSpecularTextureLayer` and the draw, so it draws the environment texture × the colour
    // stream pass 0 left (the `R3DLightLayer` colour, alpha 0xFF) over the first pass, weighted by
    // the environment texel's own alpha (the `.env` maps carry one: coastal's Y2T1 averages 18/255,
    // Y2T2 35/255) under the device's alpha test `GREATER 10`. `env_mix.x` only says the material
    // has the pass (`rmthpasses`, `RE/notes/lighting.md` §3.1).
    // `globals.ambient.z` is `[0x6115CC]+0x190`, set in the rear-view mirror's view: the layer
    // binds no texture there and skips its pass (0x445578 → 0x4455AE)
    var env_over = vec4<f32>(0.0);
    if object.env_mix.x > 0.0 && globals.ambient.z < 0.5 {
        // `R3DSpecularTextureLayer` 0x445520 (the layer class code 0x8100): the normal goes into
        // eye space through a matrix whose translation rows are zeroed (0x4456E6…0x445714), and
        // then, with `t = |z| + 1.0` ([0x5B2750]) and `r` the reciprocal square root of `t` —
        // the magic-constant estimate 0x5F400000 with one Newton step of 1.47 / 0.47
        // ([0x5B38D8] / [0x5B38E0]), which this port takes exactly — the coordinates are
        // `u = x·r·0.5 + 0.5` and `v = y·r·0.5 + 0.5` (0x4457C5…0x445828, [0x5B23F0] = 0.5).
        // The texture is `SPC2` / `SPC3` of `carcmn.fsh` in a race, `FE  ` / `FESH` in the front end
        // (`SPC1` / `FEDU` are not in the file), loaded with no state block of its own (0x44D2E9
        // pushes 0, so 0x44D550 leaves it), so it keeps the texture constructor's default 0x5379D0
        // — `LINEAR`, mip filter `NONE` (0x535BC0 → 0x5379D0, `thrash.md` §7) — except the address
        // modes, which the binder 0x440940 sets to 1, `WRAP` (0x537A70(1)): `s_env` at level 0.
        // *(Until 2026-09-26 this sampled `CLAMP`, the default block's.)*
        let ne = (globals.view * vec4<f32>(n, 0.0)).xyz;
        let r = inverseSqrt(abs(ne.z) + 1.0) * 0.5;
        let env = textureSampleLevel(t_env, s_env, vec2<f32>(ne.x * r + 0.5, ne.y * r + 0.5), 0.0);
        if env.a * 255.0 > 10.0 {
            env_over = env;
        }
    }
    var lit = base.rgb * light;
    lit = mix(lit, env_over.rgb * light, env_over.a);
    // `R3DSkyMapTextureLayer`, pass 2 (option mask 2 on `CarExtEnv` / `CarExtEnvNS`, mask 0 on
    // `CarWindow`): the same three-op pass as the specular one — the `Rs` block `{1, 2, 0, 1, 0,
    // −1}` (`rmthpasses`: key 0x52730006), the layer and the draw — so the section map × the light
    // colour, laid over by its texel alpha under the alpha test `GREATER 10`. The `.env` maps are
    // loaded like `carcmn.fsh` (0x440AAA → 0x44D270), so `CLAMP`, level 0.
    if object.sky_on.x > 0.5 {
        let sky = textureSampleLevel(t_sky, s_proj, in.sky_uv, 0.0);
        if sky.a * 255.0 > 10.0 {
            lit = mix(lit, sky.rgb * light, sky.a);
        }
    }
    // `R3DShadowMapTextureLayer` (0x446130), pass 3 with the same `Rs` block: the projector's
    // texture (`sh80`, a flat dark colour under an alpha of 0…253) × the light colour, blended over
    // by its alpha under the same `GREATER 10` test
    // the headlight decal, `Opaque`'s second pass: the first pass's pixel × 2 · `HEAD`
    // (0x453CA3: only while lamp 0 of the camera's car is lit, `headlight_on.y`)
    if object.shadow_on.y > 0.5 && globals.headlight_on.y > 0.5 && in.head_w > 0.0 {
        lit = lit * uvlight(in.head_uvw);
    }
    if object.shadow_on.x > 0.5 {
        let uv = vec2<f32>(dot(object.shadow_u.xyz, in.world) + object.shadow_u.w, dot(object.shadow_v.xyz, in.world) + object.shadow_v.w);
        // `sh80`'s material asks for a LOD bias of −1 (`mt+0x40`, `RE/notes/thrash.md` §7)
        let sh = textureSampleBias(t_proj, s_proj, uv, -1.0);
        if sh.a * 255.0 > 10.0 {
            lit = mix(lit, sh.rgb * light, sh.a);
        }
    }
    return vec4<f32>(lit, base.a * object.light.a);
}
