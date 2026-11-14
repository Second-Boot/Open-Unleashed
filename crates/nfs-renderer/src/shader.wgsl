struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    sun_dir: vec4<f32>,
    ambient: vec4<f32>,
    cam_pos: vec4<f32>,
    headlight: mat4x4<f32>,
    // x 1 while the projector lights, y 1 while the HEAD decal does, z ENHANCEMENT E24: 1 - the projector's gain
    headlight_on: vec4<f32>,
    beacon: mat4x4<f32>,
    beacon_on: vec4<f32>,
    fog: vec4<f32>,
    sky_fog: vec4<f32>,
    // ENHANCEMENT E8: .x the scene's reflection weight (0 = off), .y 1 / tan(half fov) of an environment face
    refl: vec4<f32>,
    // ENHANCEMENT E12: .xy the pass's near / far planes (view depth from the stored depth), .z 1 while the soft depth image is bound
    soft: vec4<f32>,
    // ENHANCEMENT E17: .x 1 = water reflection on, .y 1 = there is a sun; the sky colours the water reflects
    // without an environment map; .xyz the unit vector to the sun
    water: vec4<f32>,
    water_zenith: vec4<f32>,
    water_horizon: vec4<f32>,
    water_sun: vec4<f32>,
    // ENHANCEMENT E18: .xyz the unit vector to the sun, .w 1 = the lit particles take it
    fx_sun: vec4<f32>,
    // ENHANCEMENT E19: .xyz the unit vector to the sun, .w the fog's sun in-scatter strength (0 = off)
    fogsun: vec4<f32>,
    // ENHANCEMENT E26: .x the rain's strength (0 = off: 1 on, 1.5 strong), .y the race clock in seconds, .z the number of lamps, .w the wetness at the camera
    rain: vec4<f32>,
    // the sky the wet road reflects: straight up / at the horizon
    rain_zenith: vec4<f32>,
    rain_horizon: vec4<f32>,
    // the lights whose streaks the road shows: xyz the position + w its reach squared (0 = 4000), and the colour x intensity
    rain_lamp_pos: array<vec4<f32>, 12>,
    rain_lamp_col: array<vec4<f32>, 12>,
    // the wetness edges (planes along the track): pos.xyz + ramp half width, dir.xyz + wetness before, val.x wetness after
    rain_edge_pos: array<vec4<f32>, 8>,
    rain_edge_dir: array<vec4<f32>, 8>,
    rain_edge_val: array<vec4<f32>, 8>,
    // ENHANCEMENT ENH-TEMPORAL: .xy the jitter in NDC (the shift already applied to `view_proj`), .zw the render size in pixels
    temporal: vec4<f32>,
    // ENHANCEMENT E49: .x 1 = the clear coat reads the near lookup, .y 1 = the player's faces (layers 6..11) are drawn, .z the
    // proxy box's weight at the cube's eye; the box (centre + half length, forward + half width, lateral + floor, up + ceiling);
    // each face's eye, [6] the newest
    near: vec4<f32>,
    near_box: array<vec4<f32>, 4>,
    near_eye: array<vec4<f32>, 7>,
};
@group(0) @binding(0) var<uniform> globals: Globals;
// `night.psh`'s `nght`, 128 × 128 bytes packed four to a u32
@group(0) @binding(1) var<uniform> night_map: array<vec4<u32>, 1024>;
// the car shadow projector's texture (`[0x6115CC]+0x18C`)
@group(0) @binding(2) var t_proj: texture_2d<f32>;
@group(0) @binding(3) var s_proj: sampler;
@group(0) @binding(4) var t_head: texture_2d<f32>;
// ENHANCEMENT E8: the six faces of the player's car's environment (+X, -X, +Z, -Z, +Y, -Y)
@group(0) @binding(5) var t_refl: texture_2d_array<f32>;
// ENHANCEMENT E12: the opaque part's raw depth (R32Float, single sample), read per pixel
@group(0) @binding(9) var t_soft: texture_2d<f32>;
// ENHANCEMENT E16: the car ambient probe, the SH coefficients (rgb) of the irradiance normalised to a upper-hemisphere-mean luminance of 1
@group(0) @binding(16) var<uniform> probe_sh: array<vec4<f32>, 9>;

// ENHANCEMENT E9: the sun's shadow cascades (`gpu/csm.rs`). `dir.w` is the cascade count, 0 = off (every
// path below is then skipped and the shading is the original's).
struct Csm {
    mats: array<mat4x4<f32>, 4>,
    // per cascade: x a texel in metres, y the depth range in metres, z the view depth it ends at
    info: array<vec4<f32>, 4>,
    // xyz the way the light travels, w the cascade count
    dir: vec4<f32>,
    // x the track's shadow strength, y the view depth where the last cascade fades out, z (E25) the soft kernel's
    // spacing in texels (0 = the 3 x 3 original), w (E25) the car's floor of the direct term (0 = 0.25)
    params: vec4<f32>,
    // ENHANCEMENT E9 (SHADOWFIX): xyz the light the cars are lit with (the shadow's own is lifted to a higher sun), w the
    // cover gate (1 = open sky; the shadows fade out under cover)
    orig: vec4<f32>,
    // ENHANCEMENT ENH-FE3D: x the front-end car's soft edge radius in texels (0 = the race's kernels)
    extra: vec4<f32>,
    // ENHANCEMENT E9 (ENH-TUNPOOL): x the covered boxes' count; per box 5 vec4: (centre, half length), (forward, half width),
    // (lateral, weight at the back face), (up, weight at the front face), (ceiling height over the centre, nominal half length, 0, 0)
    cover_n: vec4<f32>,
    cover: array<vec4<f32>, 320>,
    // ENHANCEMENT E50: x the moon's track strength (0 = not a moon frame), y the cars' cold term, z the soft edge's radius in texels
    moon: vec4<f32>,
    // ENHANCEMENT E50: rgb the moon's colour
    moon_col: vec4<f32>,
    // ENHANCEMENT E51: x how many near cascades have a cars-only layer (0 = none), y the first of those layers, z 1 = each car's
    // shadow scaled by its share of the sun (level 1, CARSH-FADE), w how many cars `carsh_cars` lists
    carsh: vec4<f32>,
    // ENHANCEMENT E51 (CARSH-FADE): per measured car its body's centre (xyz) and the radius (w) a receiver's ray toward the sun
    // passes it within
    carsh_cars: array<vec4<f32>, 16>,
    // ENHANCEMENT E51 (CARSH-FADE): the cars' eased shares of their own shadow (1 all .. 0 none), four to a vec4 (`gpu/carsun.rs`)
    carsh_k: array<vec4<f32>, 4>,
};
@group(0) @binding(6) var t_shadow: texture_depth_2d_array;
@group(0) @binding(7) var s_shadow: sampler_comparison;
@group(0) @binding(8) var<uniform> csm: Csm;

// ENHANCEMENT E15: the lit headlights as spotlights (`gpu/spot.rs`). `params.x` is the lamp count, 0 = off
// (the shading is then the original's).
struct Spots {
    // xyz the lamp, w its intensity
    pos: array<vec4<f32>, 8>,
    // xyz the way it shines
    dir: array<vec4<f32>, 8>,
    // x the count, y cos of the outer half angle, z cos of the inner, w the range
    params: vec4<f32>,
    // x how many of the first lamps share the shadow map (0 = none), y the world size of a shadow texel at 1 m, z near, w far
    shadow: vec4<f32>,
    mat: mat4x4<f32>,
    // xyz the shadow's origin, w one texel in map coordinates
    origin: vec4<f32>,
};
@group(0) @binding(10) var t_spot: texture_depth_2d;
@group(0) @binding(11) var<uniform> spots: Spots;

// ENHANCEMENT E23: the street lamps as point lights (`gpu/lamps.rs`). `params.x` is the count, 0 = off.
struct Lamps {
    // xyz the lamp, w its radius
    pos: array<vec4<f32>, 32>,
    // rgb the colour, the brightness folded in; w (ENHANCEMENT E47) the lamp's shadow slot + 1, 0 = no shadow
    col: array<vec4<f32>, 32>,
    // x the count, z the gain, w (ENHANCEMENT E47) one shadow texel in map coordinates
    params: vec4<f32>,
    // ENHANCEMENT E47: per shadow slot the lamp's frustum, and x the world size of a texel at 1 m, y near, z far, w the strength
    smat: array<mat4x4<f32>, 4>,
    sinfo: array<vec4<f32>, 4>,
};
@group(0) @binding(20) var<uniform> lamps: Lamps;
// ENHANCEMENT E47: the lamps' car shadow maps (`gpu/lamps.rs`), one layer per slot
@group(0) @binding(25) var t_lamp_shadow: texture_depth_2d_array;

// ENHANCEMENT E43: the near cars' contact shadow (`gpu/contact.rs`). `params.x` is the car count, 0 = off (the shading is then the
// original's); `params.y` the strength (weaker where E9's sun shadow is drawn).
struct Contacts {
    // xyz the model origin on the road under the car, w the box's half width
    pos: array<vec4<f32>, 16>,
    // xyz the body's right axis, w the box's half length
    ax: array<vec4<f32>, 16>,
    // xyz the up axis, w the tyre radius
    ay: array<vec4<f32>, 16>,
    az: array<vec4<f32>, 16>,
    // the tyres' patch centres in the body's (x, z): (front left, front right) then (rear left, rear right) per car
    wheels: array<vec4<f32>, 32>,
    params: vec4<f32>,
};
@group(0) @binding(24) var<uniform> contacts: Contacts;

// ENHANCEMENT E43: the signed distance (m) to a rounded rectangle of half size `h` (negative inside)
fn contact_sdf(q: vec2<f32>, h: vec2<f32>) -> f32 {
    let rc = min(h.x, h.y) * 0.4;
    let p = abs(q) - (h - vec2<f32>(rc));
    return length(max(p, vec2<f32>(0.0))) + min(max(p.x, p.y), 0.0) - rc;
}

// ENHANCEMENT E43: how much of the light the cars' contact shadows leave at a road point (1 = all): the ambient occlusion of the
// car's underside. Per car, in its own frame: the body's underside is a rounded rectangle (the box's footprint) hovering `h` over the
// road (the clearance + `params.z`, `gpu/contact.rs` SOFTEN); the share of a cosine-weighted sky it hides from a point at signed distance `s` inside its edge (negative outside) is the
// half-plane's, `0.5 + (asin t + t sqrt(1 - t^2)) / pi`, `t = s / sqrt(s^2 + h^2)` (1 deep under the car, 0.5 at the outline, 0.09
// at `h` outside, 0.02 at `2 h`): the shadow stays inside the footprint and is soft over about the clearance. The body's sides add a
// faint short skirt (`params.w` at the outline, SKIRT); each tyre a tight crease where the rubber meets the road. A bigger clearance (a jump, a ramp) widens and thins
// it. Only faces that look up in the car's frame take it. (CONTACT-LAMPS: replaces a 0.7 / 0.3 exponential pair reaching ~1 m beyond
// the outline at full strength, which read as a separate thick dark patch at night between the street lamps.)
fn contact_keep(world: vec3<f32>, n: vec3<f32>) -> f32 {
    var keep = 1.0;
    let cnt = i32(contacts.params.x);
    for (var i = 0; i < cnt; i = i + 1) {
        let c = contacts.pos[i];
        let axs = contacts.ax[i];
        let ays = contacts.ay[i];
        let d = world - c.xyz;
        let reach = c.w + axs.w + 1.5;
        if dot(d, d) > reach * reach {
            continue;
        }
        let facing = smoothstep(0.5, 0.85, dot(n, ays.xyz));
        if facing <= 0.0 {
            continue;
        }
        let q = vec3<f32>(dot(d, axs.xyz), dot(d, ays.xyz), dot(d, contacts.az[i].xyz));
        let r = ays.w;
        // the clearance under the body's underside, and how a bigger one thins the shadow out (a jump)
        let gap = max(0.5 * r - q.y, 0.03);
        let kg = 1.0 / (1.0 + (gap / 0.6) * (gap / 0.6));
        let sd = contact_sdf(q.xz, vec2<f32>(c.w, axs.w));
        let h = gap + contacts.params.z;
        let t = -sd / sqrt(sd * sd + h * h);
        let under = 0.5 + (asin(t) + t * sqrt(max(1.0 - t * t, 0.0))) * 0.31830988;
        let body = 1.0 - (1.0 - under) * (1.0 - contacts.params.w * exp(-max(sd, 0.0) / 0.35));
        // the tyres: a tight patch with a short halo; a tyre in the air (the road far below the car's plane) has neither
        let gw = max(-q.y, 0.0) / 0.3;
        let kw = 1.0 / (1.0 + gw * gw);
        var wh = 0.0;
        for (var k = 0; k < 2; k = k + 1) {
            let w = contacts.wheels[2 * i + k];
            let e0 = (q.xz - w.xy) / vec2<f32>(0.55 * r, 0.95 * r);
            let e1 = (q.xz - w.zw) / vec2<f32>(0.55 * r, 0.95 * r);
            let a0 = dot(e0, e0);
            let a1 = dot(e1, e1);
            wh = max(wh, max(0.4 * exp(-a0 * 3.0) + 0.6 * exp(-a0 * 9.0), 0.4 * exp(-a1 * 3.0) + 0.6 * exp(-a1 * 9.0)));
        }
        let occ = 1.0 - (1.0 - body * kg) * (1.0 - wh * kw);
        keep = keep * (1.0 - contacts.params.y * facing * occ);
    }
    return keep;
}

// ENHANCEMENT E43: 1 where a listed car's footprint stands (the original's blob gives way there), 0 beyond ~0.9 m outside it
fn contact_presence(world: vec3<f32>) -> f32 {
    var p = 0.0;
    let cnt = i32(contacts.params.x);
    for (var i = 0; i < cnt; i = i + 1) {
        let c = contacts.pos[i];
        let axs = contacts.ax[i];
        let d = world - c.xyz;
        let reach = c.w + axs.w + 3.5;
        if dot(d, d) > reach * reach {
            continue;
        }
        let s = contact_sdf(vec2<f32>(dot(d, axs.xyz), dot(d, contacts.az[i].xyz)), vec2<f32>(c.w, axs.w));
        p = max(p, 1.0 - smoothstep(0.15, 0.9, s));
    }
    return p;
}

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
    // ENHANCEMENT E24: `headlight_on.z` = 1 - the projector's gain (0 = the original's, whole)
    let add = round(128.0 * w * (1.0 - globals.headlight_on.z));
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
// (`ClampToEdge`, `Linear`, level 0) carries it. *(Until 2026-09-26 this was an approximation
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
    // .w ENHANCEMENT E9: what the item does with the sun's shadow, RECV_NONE / RECV_CAR / RECV_TRACK / RECV_DECAL (the class
    // constants are emitted in front of this file from `classes.rs`)
    lit_d: vec4<f32>,
    lit_dir: vec4<f32>,
    shadow_u: vec4<f32>,
    shadow_v: vec4<f32>,
    shadow_on: vec4<f32>,
    sky_u: vec4<f32>,
    sky_v: vec4<f32>,
    sky_on: vec4<f32>,
    // ENHANCEMENT E8: .x the material's reflection class (REFL_NONE, REFL_PAINT, REFL_GLASS; E17 REFL_WATER, E26 REFL_WET_ROAD /
    // REFL_SKID), .y its strength
    refl: vec4<f32>,
    // ENHANCEMENT E12: .x the fade distance in metres (0 = none), .y SOFT_FADE_WHITE = the multiplying pair (fade toward white),
    // SOFT_FADE_BLACK the colour toward black, SOFT_FADE_ALPHA alpha only; E15 / E23: .z RECV_*; ENH-TEMPORAL: .w ROLE_*
    soft: vec4<f32>,
    // ENHANCEMENT E11: .x the HDR boost of a flare layer (0 / 1 = none)
    // ENHANCEMENT E18: .y a spark's gain above white (0 = none), .z the lit particle kind (PART_LIT centre-lit, PART_LIT_WORLD per
    // fragment)
    hdr: vec4<f32>,
    // `view_proj · model`, multiplied out in f64 by the host (PORT-DEVIATION D37)
    mvp: mat4x4<f32>,
};
// every object of the pass sits in one read-only storage buffer, bound once per pass; a draw names
// its object by `first_instance` (`instance_index`), which the vertex stage hands to the fragment
// stage as a flat varying. `object` is a private copy loaded at the head of each entry point, so
// the maths below reads the same values as it did from the per-draw dynamic-offset uniform.
@group(1) @binding(0) var<storage, read> objects: array<Object>;
var<private> object: Object;
// ENHANCEMENT ENH-TEMPORAL: the reprojection matrices (this frame's UN-jittered clip -> the previous frame's clip) of the
// temporal pass: 0 the static world's, 1 the sky's (the camera's rotation alone), then one per item that moved of its own.
// Only the `_t` fragment entry points read it; the temporal pipeline layout is the one that has the binding.
@group(1) @binding(1) var<storage, read> reproj: array<mat4x4<f32>>;

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
    /// `R3DLightLayer`'s colour, made per vertex as the layer makes it (0x446530) and interpolated
    @location(8) vlight: vec3<f32>,
    /// `R3DSpecularTextureLayer`'s coordinates, per vertex (0x445520)
    @location(9) env_uv: vec2<f32>,
};

// `R3DLightLayer` 0x446530, one vertex: `A = block[slot] · 255 · p0` and, where the normal faces the
// light (`n · (−255 · L)` not negative, 0x446944), `A + D · d` with `D = block[slot + 4] ⊙ +0x74 · p1`;
// each channel is rounded to nearest by the 1.5 · 2^23 trick (`[0x5CDE94]` / `[0x5CDE98]` added, the
// low 22 bits kept: 0x446670…0x44669E, 0x4469A2…0x4469CD) and clamped to 255. The colour lands in the
// vertex's diffuse dword, so the device interpolates it between vertices.
fn layer_light(n: vec3<f32>) -> vec3<f32> {
    // a zero direction normalises to NaN in the original (0x4E8791), whose sign bit sends every
    // vertex to the ambient arm (0x446944) — so no directional term
    var d = 0.0;
    if length(object.lit_dir.xyz) > 0.0 {
        d = max(dot(n, -normalize(object.lit_dir.xyz)), 0.0);
    }
    return min(floor(object.lit_a.rgb * 255.0 + object.lit_d.rgb * 255.0 * d + vec3<f32>(0.5)), vec3<f32>(255.0)) / 255.0;
}

@vertex
fn vs_main(in: VsIn, @builtin(instance_index) inst: u32) -> VsOut {
    object = objects[inst];
    var out: VsOut;
    out.obj = inst;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = object.mvp * vec4<f32>(in.position, 1.0);
    // Thrash state 0x18, the depth bias (`lit_dir.w`, an offset of the screen z): with the DX7
    // driver the game keeps it itself (0x540500 / 0x5404A0 store `b · 2^-15` at `ctx+0x1180C` once
    // 0x5492DC has seen "DX7" in `THRASH_about`) and adds it to the screen z of its own
    // pre-transformed vertex, `sz = (Z · far/(far − near) − near·far/(far − near)) · (1/Z) + bias`
    // (0x540CE0, 0x541B0B…0x541B45; `RE/notes/thrash.md` §6d.3) — the Direct3D depth that this
    // pass's projection makes too, so the offset is `bias · w` on the clip z: the vertex keeps its
    // place on screen and its `w`, and the depth buffer compares exactly `sz + bias`. A vertex the
    // original clips (outside near…far, tested on the unbiased Z by 0x5B0660) is left to the
    // clipper here as well.
    // APPROXIMATION (device behaviour only): dx7z hands the device `sz + bias` unclamped and
    // unclipped — its vertex copy (0x60006B41…, 0x60007008…0x60007056) only `fadd`s the stored bias,
    // the whole flush 0x60006F20 / quad path 0x60007650 holds no float compare, and the one draw
    // 0x60006A50 is `DrawIndexedPrimitive` (vtable +0x68) with the flags word [0x60031A60] never
    // written (0, so no DONOTCLIP) on FVF 452 = pre-transformed. What the Direct3D runtime / card
    // then does with a screen z outside [0, 1] (clip at the plane, or clamp) is not in any binary
    // that can be read (`RE/notes/thrash.md` §6d.3, "Near-plane vertices"): here it is held at the
    // near / far plane, so the vertex is neither clipped nor wrapped (a negative bias at the near end
    // leaves it in front of everything, as the earlier pull did).
    let dz = object.lit_dir.w;
    if dz != 0.0 && out.clip.w > 0.0 && out.clip.z >= 0.0 && out.clip.z <= out.clip.w {
        out.clip.z = clamp(out.clip.z + dz * out.clip.w, 0.0, out.clip.w);
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
    // the light and the reflection's coordinates are made per vertex, the way the layers make them
    out.vlight = vec3<f32>(1.0);
    if object.lit_a.w > 0.5 {
        out.vlight = layer_light(out.normal);
    }
    let vne = (globals.view * vec4<f32>(out.normal, 0.0)).xyz;
    let vr = inverseSqrt(abs(vne.z) + 1.0) * 0.5;
    out.env_uv = vec2<f32>(vne.x * vr + 0.5, vne.y * vr + 0.5);
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
    // ENHANCEMENT E19: the sun's glow in the fog — the fog colour (the original's, the base) is pulled toward
    // warm sunlight by a Henyey-Greenstein phase of the angle between the view ray and the sun (g = 0.7),
    // normalised to 1 straight at the sun and cut to 0 from 90 degrees away on, so away from the sun the
    // result is exactly the original's fog. Only where there is fog (this function returned already when
    // there is none) and not for the sun's own quad (`sky_on.z`).
    //@if FOGSUN
    if globals.fogsun.w > 0.0 && object.sky_on.z <= 0.0 {
        let v = normalize(in.world - globals.cam_pos.xyz);
        let cs = dot(v, globals.fogsun.xyz);
        let g = 0.7;
        let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cs, 1.5);
        let hg0 = (1.0 - g * g) / pow(1.0 + g * g, 1.5);
        let hg1 = (1.0 - g * g) / pow((1.0 - g) * (1.0 - g), 1.5);
        let ph = clamp((hg - hg0) / (hg1 - hg0), 0.0, 1.0);
        let glow = mix(fog.rgb, vec3<f32>(1.0, 0.86, 0.62), globals.fogsun.w * ph);
        return vec4<f32>(mix(glow, c.rgb, f), c.a);
    }
    //@endif
    return vec4<f32>(mix(fog.rgb, c.rgb, f), c.a);
}

// ENHANCEMENT E9: how much of the sun reaches `world` through cascade `c` (0 shadowed .. 1 lit): the
// point is pushed along the normal by a texel or so (more where the surface turns from the light), the
// depth is held a texel's worth below the stored one, and a 3 x 3 grid of hardware-bilinear compares
// softens the edge over about four texels.
fn csm_tap(c: i32, world: vec3<f32>, n: vec3<f32>, ndl: f32) -> f32 {
    let info = csm.info[c];
    let off = n * (info.x * (0.6 + 1.6 * (1.0 - clamp(ndl, 0.0, 1.0))));
    let p = csm.mats[c] * vec4<f32>(world + off, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || p.z > 1.0 {
        return 1.0;
    }
    var step = 1.0 / f32(textureDimensions(t_shadow).x);
    let depth = p.z - info.x * 1.2 / info.y;
    var sum = 0.0;
    //@if MOON
    // ENHANCEMENT E50: the moon's soft edge: a 16-tap Vogel disc of hardware-bilinear compares, `csm.moon.z` texels across the radius.
    // PERF: four taps on the disc's rim first; when they agree (all lit or all in shadow, nearly every pixel) the disc would too
    if csm.moon.x > 0.0 {
        let rim = csm.moon.z * step * 0.7071;
        for (var k = 0; k < 4; k++) {
            sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * rim, c, depth);
        }
        if sum < 0.001 || sum > 3.999 {
            return sum * 0.25;
        }
        sum = 0.0;
        for (var i = 0; i < 16; i++) {
            let fi = f32(i);
            let rr = sqrt((fi + 0.5) / 16.0) * csm.moon.z * step;
            let a = fi * 2.3999632;
            sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(cos(a), sin(a)) * rr, c, depth);
        }
        return sum / 16.0;
    }
    //@endif
    // ENHANCEMENT ENH-FE3D: the front-end car's soft shadow: a Vogel disc of 24 hardware-bilinear compares, turned per pixel
    // by interleaved gradient noise (the edge dithers instead of banding), `extra.x` texels across the radius
    if csm.extra.x > 0.0 {
        let rot = 6.2831853 * fract(52.9829189 * fract(dot(world.xz * 211.0 + world.y * 97.0, vec2<f32>(0.06711056, 0.00583715))));
        for (var i = 0; i < 24; i++) {
            let fi = f32(i);
            let rr = sqrt((fi + 0.5) / 24.0) * csm.extra.x * step;
            let a = fi * 2.3999632 + rot;
            sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(cos(a), sin(a)) * rr, c, depth);
        }
        var soft = sum / 24.0;
        // ENHANCEMENT ENH-GARAGE: a tight core (`extra.y` texels, 12 taps, another turn) carries the contact: the shadow is the
        // darker of the penumbra (its darkness scaled by `extra.z`) and the whole-strength core
        if csm.extra.y > 0.0 {
            var core = 0.0;
            for (var i = 0; i < 12; i++) {
                let fi = f32(i);
                let rr = sqrt((fi + 0.5) / 12.0) * csm.extra.y * step;
                let a = fi * 2.3999632 + rot + 1.3;
                core += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(cos(a), sin(a)) * rr, c, depth);
            }
            soft = min(1.0 - (1.0 - soft) * csm.extra.z, core / 12.0);
        }
        return soft;
    }
    // ENHANCEMENT E25 (`csm.params.z` > 1): the overcast light's wide soft edge, a 5 x 5 grid spaced `z` texels
    if csm.params.z > 1.0 {
        step = step * csm.params.z;
        for (var dy = -2; dy <= 2; dy++) {
            for (var dx = -2; dx <= 2; dx++) {
                sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(f32(dx), f32(dy)) * step, c, depth);
            }
        }
        return sum / 25.0;
    }
    // PERF: the four corner taps first (each bilinear tap reads a 2 x 2 texel block, so the four cover the whole
    // 3 x 3 grid): when they agree - all lit or all in shadow, nearly every pixel - the other five would too
    var corners = 0.0;
    for (var k = 0; k < 4; k++) {
        corners += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * step, c, depth);
    }
    // the far cascades' texels are coarse and their shadows far: the four corners are the soft edge there
    if corners < 0.001 || corners > 3.999 || c >= 1 {
        return corners * 0.25;
    }
    sum = corners;
    sum += textureSampleCompareLevel(t_shadow, s_shadow, uv, c, depth);
    sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(-step, 0.0), c, depth);
    sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(step, 0.0), c, depth);
    sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(0.0, -step), c, depth);
    sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(0.0, step), c, depth);
    return sum / 9.0;
}

// ENHANCEMENT E9 (ENH-TUNPOOL): how covered a world point is, 0..1: inside a covered road slice's box (a tunnel's or a gallery's
// interior, from the track's own overhead flags, `CoverBox`), whatever the camera is doing. The sun does not reach there and the
// baked colour has its own light in it (the lamps' pools), so the sun's shadow must not darken it a second time.
fn cover_at(world: vec3<f32>) -> f32 {
    let count = i32(csm.cover_n.x);
    var w = 0.0;
    for (var i = 0; i < count; i++) {
        let a = csm.cover[5 * i];
        let b = csm.cover[5 * i + 1];
        let c = csm.cover[5 * i + 2];
        let d = csm.cover[5 * i + 3];
        let e = csm.cover[5 * i + 4];
        let r = world - a.xyz;
        let l = dot(r, b.xyz);
        if abs(l) > a.w || abs(dot(r, c.xyz)) > b.w {
            continue;
        }
        let h = dot(r, d.xyz);
        if h < -2.0 || h > e.x {
            continue;
        }
        w = max(w, clamp(mix(c.w, d.w, (l + e.y) / (2.0 * e.y)), 0.0, 1.0));
        if w >= 1.0 {
            break;
        }
    }
    return w;
}

// ENHANCEMENT E9 (SHADOW-REAL): how much of the sun's light the shadow is drawn for at a world point: the cover gate (1 in
// the open; with the covered boxes known it shuts only while the camera is deep inside a covered run), times the point's own openness (0 inside a covered
// road slice's box: a tunnel ends at its mouth in space, not when the camera leaves it)
fn csm_share(world: vec3<f32>) -> f32 {
    var share = csm.orig.w;
    if csm.cover_n.x > 0.5 {
        share = share * (1.0 - cover_at(world));
    }
    return share;
}

// ENHANCEMENT E9 (SHADOW-REAL): the sun's shadow at a point in its two parts: x the cascades' visibility (0 shadowed .. 1 lit),
// y the share of the sun's light it is drawn for (`csm_share`, faded out toward the last cascade's end). The cascade is chosen by
// the point's view depth, blended into the next over the last eighth of its range. A covered point reads no map at all.
fn csm_split(world: vec3<f32>, n: vec3<f32>) -> vec2<f32> {
    let count = i32(csm.dir.w);
    let vd = -(globals.view * vec4<f32>(world, 1.0)).z;
    let last = csm.info[count - 1].z;
    let share = csm_share(world);
    if vd >= last || share <= 0.0 {
        return vec2<f32>(1.0, 0.0);
    }
    let ndl = dot(n, -csm.dir.xyz);
    var c = 0;
    for (var i = 0; i < count - 1; i++) {
        if vd >= csm.info[i].z {
            c = i + 1;
        }
    }
    var v = csm_tap(c, world, n, ndl);
    if c + 1 < count {
        let end = csm.info[c].z;
        let t = clamp((vd - end * 0.875) / (end * 0.125), 0.0, 1.0);
        if t > 0.0 {
            v = mix(v, csm_tap(c + 1, world, n, ndl), t);
        }
    }
    let f = clamp((vd - csm.params.y) / max(last - csm.params.y, 0.001), 0.0, 1.0);
    return vec2<f32>(v, share * (1.0 - f));
}

// how much of the sun reaches `world` (0 shadowed .. 1 lit; 1 under cover, beyond the cascades and with the gate shut)
fn csm_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let t = csm_split(world, n);
    return 1.0 - t.y * (1.0 - t.x);
}

//@if CARSH
// ENHANCEMENT E51: how much of the sun reaches `world` past the CARS alone (1 = no car's shadow here, or beyond the cars-only
// cascades): the cars-only layer of the cascade the point's view depth picks, E9's normal offset and depth bias, the four corner taps
// of its kernel. Level 1 (`carsh.z`, CARSH-FADE): where that finds a car's shadow, it is drawn at the share of the sun the car
// itself gets (`car_sun_share`: one value per car, measured on the map and eased in time - never cut texel by texel).
fn car_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let all = i32(csm.dir.w);
    let vd = -(globals.view * vec4<f32>(world, 1.0)).z;
    var c = 0;
    for (var i = 0; i < all - 1; i++) {
        if vd >= csm.info[i].z {
            c = i + 1;
        }
    }
    if c >= i32(csm.carsh.x) || vd >= csm.info[all - 1].z {
        return 1.0;
    }
    let info = csm.info[c];
    let ndl = dot(n, -csm.dir.xyz);
    let off = n * (info.x * (0.6 + 1.6 * (1.0 - clamp(ndl, 0.0, 1.0))));
    let p = csm.mats[c] * vec4<f32>(world + off, 1.0);
    let uv = vec2<f32>(p.x * 0.5 + 0.5, 0.5 - p.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || p.z > 1.0 {
        return 1.0;
    }
    let step = 1.0 / f32(textureDimensions(t_shadow).x);
    let depth = p.z - info.x * 1.2 / info.y;
    let layer = i32(csm.carsh.y) + c;
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * step, layer, depth);
    }
    // ENHANCEMENT E51 (CARSH-FADE): only where a car's shadow is found (most of the road reads no car: the cheap taps above decide)
    let v = sum * 0.25;
    if csm.carsh.z > 0.5 && sum < 3.999 {
        return 1.0 - car_sun_share(world) * (1.0 - v);
    }
    return v;
}

// ENHANCEMENT E51 (CARSH-FADE, level 1): the share of its own shadow the car whose shadow falls on `world` draws (1 all .. 0 none):
// the measured car whose body the point's ray toward the sun passes nearest (within its radius); 1 when none does (a car past the
// list's 16 keeps the level-2 shadow). A car stands in the scenery's broad shade (a gorge, a wall's, a cliff's) or it does not: the
// value is one per car, so its shadow fades as a whole, never in patches.
fn car_sun_share(world: vec3<f32>) -> f32 {
    let l = -csm.dir.xyz;
    var best = 1e9;
    var k = 1.0;
    for (var i = 0; i < i32(csm.carsh.w); i++) {
        let c = csm.carsh_cars[i];
        let d = c.xyz - world;
        let along = dot(d, l);
        let perp = dot(d, d) - along * along;
        if along > -1.0 && perp < c.w * c.w && perp < best {
            best = perp;
            k = csm.carsh_k[i >> 2u][i & 3];
        }
    }
    return k;
}
//@endif

// ENHANCEMENT E15: how much of the player's lamp pair reaches `world` (0 shadowed .. 1 lit): the shadow
// frustum's perspective map, the point pushed along its normal by about a texel at its distance, the
// depth held a few centimetres (in metres of view depth, turned into the stored depth's units) below
// the stored one, a 3 x 3 grid of hardware-bilinear compares.
fn spot_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let o = spots.origin.xyz;
    let dist = length(world - o);
    let texel = spots.shadow.y * dist;
    let ndl = clamp(dot(n, normalize(o - world)), 0.0, 1.0);
    let p = spots.mat * vec4<f32>(world + n * (texel * (1.0 + 2.0 * (1.0 - ndl))), 1.0);
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
    // PERF: the four corner taps first (their 2 x 2 blocks cover the whole 3 x 3 grid); when they agree the other five would too
    let st = spots.origin.w;
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * st, depth);
    }
    if sum > 0.001 && sum < 3.999 {
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv, depth);
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(-st, 0.0), depth);
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(st, 0.0), depth);
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(0.0, -st), depth);
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(0.0, st), depth);
        sum = sum / 9.0;
    } else {
        sum = sum * 0.25;
    }
    // the frustum's border fades out so the shadowed disc has no hard rim
    let edge = max(abs(ndc.x), abs(ndc.y));
    return mix(sum, 1.0, smoothstep(0.85, 1.0, edge));
}

// ENHANCEMENT E15: the irradiance of the lit lamps at `world` for a surface with normal `n`: per lamp a smooth
// cone, a range falloff `(1 - d / range)^2 / (1 + (d / 25)^2)`, `n.l` raised to the floor `floor_` (the track
// has no lighting model, see `gpu/spot.rs`), a warm white; the first two lamps take the shadow map.
// ENHANCEMENT E23: the street lamps' irradiance at a point: a smooth falloff to the radius, a downward
// cone (full below the lamp, gone toward the horizon: `l.y` is the cosine of the angle off straight
// down seen from the lamp), `n.l` with a floor as for the spots.
fn lamp_light(world: vec3<f32>, n: vec3<f32>, floor_: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let count = i32(lamps.params.x);
    for (var i = 0; i < count; i = i + 1) {
        let radius = lamps.pos[i].w;
        let lv = lamps.pos[i].xyz - world;
        let dist = length(lv);
        if dist >= radius {
            continue;
        }
        let l = lv / max(dist, 0.001);
        let cone = smoothstep(0.1, 0.55, l.y);
        let r = 1.0 - dist / radius;
        let fall = r * r * (3.0 - 2.0 * r);
        var w = cone * fall * mix(floor_, 1.0, clamp(dot(n, l), 0.0, 1.0));
        // ENHANCEMENT E47: a lamp with a shadow slot is shadowed by the cars (only its own term)
        //@if LSHAD
        if lamps.col[i].w > 0.5 && w > 0.0 {
            w = w * lamp_visibility(i32(lamps.col[i].w) - 1, lamps.pos[i].xyz, world, n);
        }
        //@endif
        sum += lamps.col[i].rgb * w;
    }
    return sum * lamps.params.z;
}

// ENHANCEMENT E47: how much of lamp slot `k`'s light (the lamp at `lamp`) reaches `world` (0 shadowed .. 1 lit), mixed toward 1 by the
// slot's strength: the slot's perspective map, the point pushed along its normal by about a texel, the depth held a few
// centimetres (in metres, turned into the stored depth's units) below the stored one, E15's 3 x 3 grid with its four-tap early-out,
// the frustum's border fading to lit. Outside the frustum nothing is shadowed (it holds every caster's cone).
fn lamp_visibility(k: i32, lamp: vec3<f32>, world: vec3<f32>, n: vec3<f32>) -> f32 {
    let info = lamps.sinfo[k];
    let dist = length(world - lamp);
    let texel = info.x * dist;
    let ndl = clamp(dot(n, (lamp - world) / max(dist, 0.001)), 0.0, 1.0);
    let p = lamps.smat[k] * vec4<f32>(world + n * (texel * (1.0 + 2.0 * (1.0 - ndl))), 1.0);
    let near = info.y;
    let far = info.z;
    if p.w <= near {
        return 1.0;
    }
    let ndc = p.xyz / p.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let bias = (0.04 + 1.5 * texel) * far * near / ((far - near) * p.w * p.w);
    let depth = ndc.z - bias;
    let st = lamps.params.w;
    var sum = 0.0;
    for (var t = 0; t < 4; t++) {
        sum += textureSampleCompareLevel(t_lamp_shadow, s_shadow, uv + vec2<f32>(f32(t & 1) * 2.0 - 1.0, f32(t >> 1) * 2.0 - 1.0) * st, k, depth);
    }
    if sum > 0.001 && sum < 3.999 {
        sum += textureSampleCompareLevel(t_lamp_shadow, s_shadow, uv, k, depth);
        sum += textureSampleCompareLevel(t_lamp_shadow, s_shadow, uv + vec2<f32>(-st, 0.0), k, depth);
        sum += textureSampleCompareLevel(t_lamp_shadow, s_shadow, uv + vec2<f32>(st, 0.0), k, depth);
        sum += textureSampleCompareLevel(t_lamp_shadow, s_shadow, uv + vec2<f32>(0.0, -st), k, depth);
        sum += textureSampleCompareLevel(t_lamp_shadow, s_shadow, uv + vec2<f32>(0.0, st), k, depth);
        sum = sum / 9.0;
    } else {
        sum = sum * 0.25;
    }
    let edge = max(abs(ndc.x), abs(ndc.y));
    let vis = mix(sum, 1.0, smoothstep(0.85, 1.0, edge));
    return mix(1.0, vis, info.w);
}

fn spot_light(world: vec3<f32>, n: vec3<f32>, floor_: f32) -> vec3<f32> {
    // PERF: the shadow lookup happens once, and only for a fragment one of the shadowed lamps reaches (below)
    var vis = -1.0;
    var e = vec3<f32>(0.0);
    let count = i32(spots.params.x);
    for (var i = 0; i < count; i++) {
        let lv = spots.pos[i].xyz - world;
        let dist = max(length(lv), 0.3);
        let l = lv / dist;
        let cone = smoothstep(spots.params.y, spots.params.z, dot(-l, spots.dir[i].xyz));
        if cone <= 0.0 {
            continue;
        }
        let r = clamp(1.0 - dist / spots.params.w, 0.0, 1.0);
        if r <= 0.0 {
            continue;
        }
        let d25 = dist / 25.0;
        var s = spots.pos[i].w * cone * r * r / (1.0 + d25 * d25) * mix(floor_, 1.0, clamp(dot(n, l), 0.0, 1.0));
        if f32(i) < spots.shadow.x {
            if vis < 0.0 {
                vis = spot_visibility(world, n);
            }
            s = s * vis;
        }
        e += s * vec3<f32>(1.0, 0.9, 0.7);
    }
    return e;
}

// ENHANCEMENT E7: an 8-bit target holds the fragment output to 0...1 (and so does a blend's source
// factor); a float target would keep the overshoot of the additive layers (the headlight decal's
// 2 x HEAD, the specular and sky-map overlays). `globals.ambient.w` 0 holds it likewise, so the float
// image the "as the original" tone map reads is the 8-bit pipeline's frame; 1 lets the ACES tone map
// have the range. With an 8-bit target the clamp is a no-op.
fn held(c0: vec4<f32>) -> vec4<f32> {
    var c = c0;
    // ENHANCEMENT E11: the sun's / lens flare's bright core above white; the glow (low values) keeps
    // its shape, `m` = 1 (the layer's full colour) gets the whole boost
    //@if HDRB
    if object.hdr.x > 1.0 {
        let m = clamp(max(c.r, max(c.g, c.b)), 0.0, 1.0);
        // ENH-TUNE: only the disc (m ~ 1) is lifted — the thin rays of the star texture (SUNA) sit lower
        // and keep their own brightness, so bloom does not melt the star into a round glow (m^8)
        let m2 = m * m;
        let m8 = m2 * m2 * m2 * m2;
        c = vec4<f32>(c.rgb * (1.0 + (object.hdr.x - 1.0) * m8), c.a);
    }
    //@endif
    if globals.ambient.w > 0.5 {
        return c;
    }
    return clamp(c, vec4<f32>(0.0), vec4<f32>(1.0));
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade(in, false);
    if object.shadow_on.w > 0.0 && c.a * 255.0 < object.shadow_on.w {
        discard;
    }
    return held(vec4<f32>(fogged(in, c).rgb, 1.0));
}

// ENHANCEMENT E1: the opaque pipeline for an alpha-TESTED draw (foliage, fences) on a multisampled
// target, with alpha-to-coverage: the test's threshold (`shadow_on.w`, reference + ½ in bytes) stays
// where the original has it, but the edge is a sharpened coverage ramp (one pixel wide, by the
// alpha's screen-space slope) instead of a discard, so the cut-out edge is antialiased.
@fragment
fn fs_opaque_a2c(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade(in, false);
    let a255 = c.a * 255.0;
    let w = max(fwidth(a255), 0.0001);
    var cov = 1.0;
    if object.shadow_on.w > 0.0 {
        cov = clamp((a255 - object.shadow_on.w) / w + 0.5, 0.0, 1.0);
    }
    return held(vec4<f32>(fogged(in, c).rgb, cov));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade(in, false);
    if object.shadow_on.w > 0.0 && c.a * 255.0 < object.shadow_on.w {
        discard;
    }
    return held(soften(in, particle_fx(in, fogged(in, c))));
}

// ENHANCEMENT ENH-TEMPORAL: the main view's fragment entry points of the temporal pipeline set. Next to the colour they
// write (1) the motion vector, (2) the reactive mask and (3) the transparency-and-composition mask (`temporal.rs`).
// The colour is what `fs_opaque` / `fs_main` write, bit for bit.
//
// Motion vector = previous - current, in pixels of the render size, y DOWN (FSR's convention, `motionVectorScale` 1). The
// current position is the pixel's own UN-jittered clip position: the clip transform carries the jitter (`globals.temporal.xy`
// in NDC), so the pixel at NDC `n` shows the point whose un-jittered NDC is `n - jitter`; that clip position goes through the
// item's reprojection matrix (built in f64 on the CPU) to the previous frame's clip.
override PAIR: u32 = 0u;

struct FsOutT {
    @location(0) c: vec4<f32>,
    // .xy the vector, .w 1 = write it (read by the blend of a pipeline that does not replace)
    @location(1) mv: vec4<f32>,
    @location(2) re: vec4<f32>,
    @location(3) tc: vec4<f32>,
};

fn velocity(in: VsOut) -> vec2<f32> {
    let size = globals.temporal.zw;
    let w = 1.0 / in.clip.w;
    let ndc = vec2<f32>(in.clip.x / size.x * 2.0 - 1.0, 1.0 - in.clip.y / size.y * 2.0) - globals.temporal.xy;
    let p = reproj[u32(object.hdr.w)] * vec4<f32>(ndc * w, in.clip.z * w, w);
    if p.w <= 1e-6 {
        return vec2<f32>(0.0);
    }
    let q = p.xy / p.w;
    return vec2<f32>((q.x - ndc.x) * 0.5 * size.x, (ndc.y - q.y) * 0.5 * size.y);
}

// how much of the pixel the blended draw covers: its alpha, and for the multiplying pair (3) how much it darkens
fn coverage(c: vec4<f32>) -> f32 {
    if PAIR == 3u {
        return clamp(1.0 - dot(c.rgb, vec3<f32>(0.3333333)), 0.0, 1.0);
    }
    return clamp(c.a, 0.0, 1.0);
}

// the transparency-and-composition mask of a surface whose look is not its own: water (class 3), glass (2), a wet road
// (4) and a skid mark on one (5), `.w` the wetness
fn composition(alpha: f32) -> f32 {
    let cls = object.refl.x;
    if cls > REFL_WATER - 0.5 && cls < REFL_WATER + 0.5 {
        return alpha;
    }
    if cls > REFL_GLASS - 0.5 && cls < REFL_GLASS + 0.5 {
        return alpha;
    }
    if cls > REFL_WET_ROAD - 0.5 && cls < REFL_SKID + 0.5 {
        return 0.5 * clamp(object.refl.w, 0.0, 1.0);
    }
    return 0.0;
}

@fragment
fn fs_opaque_t(in: VsOut) -> FsOutT {
    object = objects[in.obj];
    let c = shade(in, false);
    if object.shadow_on.w > 0.0 && c.a * 255.0 < object.shadow_on.w {
        discard;
    }
    var o: FsOutT;
    o.c = held(vec4<f32>(fogged(in, c).rgb, 1.0));
    o.mv = vec4<f32>(velocity(in), 0.0, 1.0);
    o.re = vec4<f32>(0.0);
    o.tc = vec4<f32>(composition(1.0), 0.0, 0.0, 0.0);
    return o;
}

// `object.soft.w`: 0 = a blended item that is opaque-like (it writes depth: the pipeline replaces the vector and leaves
// the masks alone), 1 = a translucent effect (reactive coverage), 2 = a sky layer (writes the vector, no mask)
@fragment
fn fs_main_t(in: VsOut) -> FsOutT {
    object = objects[in.obj];
    let c = shade(in, false);
    if object.shadow_on.w > 0.0 && c.a * 255.0 < object.shadow_on.w {
        discard;
    }
    var o: FsOutT;
    let col = held(soften(in, particle_fx(in, fogged(in, c))));
    o.c = col;
    let kind = object.soft.w;
    o.mv = vec4<f32>(velocity(in), 0.0, select(0.0, 1.0, kind > ROLE_SKY - 0.5));
    let fx = kind > ROLE_OPAQUE + 0.5 && kind < ROLE_EFFECT + 0.5;
    o.re = vec4<f32>(select(0.0, coverage(col), fx), 0.0, 0.0, 0.0);
    o.tc = vec4<f32>(select(0.0, composition(col.a), fx), 0.0, 0.0, 0.0);
    return o;
}

// ENHANCEMENT ENH-TEMPORAL: `fs_layers` on the temporal set (the blended pair 0 draws only; the layers' coverage is `col.a`)
@fragment
fn fs_layers_t(in: VsOut) -> FsOutT {
    object = objects[in.obj];
    var o: FsOutT;
    let col = held(layers_color(in));
    o.c = col;
    let kind = object.soft.w;
    o.mv = vec4<f32>(velocity(in), 0.0, select(0.0, 1.0, kind > ROLE_SKY - 0.5));
    let fx = kind > ROLE_OPAQUE + 0.5 && kind < ROLE_EFFECT + 0.5;
    o.re = vec4<f32>(select(0.0, coverage(col), fx), 0.0, 0.0, 0.0);
    o.tc = vec4<f32>(select(0.0, composition(col.a), fx), 0.0, 0.0, 0.0);
    return o;
}

// ENHANCEMENT E18: particles in the float scene, on top of the original's colours. A spark (`hdr.y` > 0) goes far
// above white (the bloom, E10, then glows). A lit particle (`hdr.z` 2 = by its centre, 3 = per fragment) takes the
// sun's side: unshadowed it is 1.0 (the original) up to 1.3 where the view looks toward the sun (a forward-scatter
// lobe, `dot(view, to_sun)^6`), and in the sun's shadow (E9's map, one cheap sample) 0.72 plus a little of the lobe.
// Smoke never goes past 1.3 x its own colour: it is not turned into a light.
fn particle_fx(in: VsOut, c: vec4<f32>) -> vec4<f32> {
    var o = c;
    //@if PFX
    if object.hdr.y > 0.0 {
        o = vec4<f32>(c.rgb * object.hdr.y, c.a);
    } else if object.hdr.z > PART_LIT - 0.5 && globals.fx_sun.w > 0.5 {
        let to_sun = globals.fx_sun.xyz;
        let v = normalize(in.world - globals.cam_pos.xyz);
        let lobe = pow(max(dot(v, to_sun), 0.0), 6.0);
        var vis = 1.0;
        //@if CSM
        if csm.dir.w > 0.5 {
            var p = in.world;
            if object.hdr.z < PART_LIT_WORLD - 0.5 {
                p = object.model[3].xyz;
            }
            vis = csm_visibility(p, to_sun);
        }
        //@endif
        let lit = mix(0.72 + 0.1 * lobe, 1.0 + 0.3 * lobe, vis);
        o = vec4<f32>(c.rgb * lit, c.a);
    }
    //@endif
    return o;
}

// ENHANCEMENT E12: a world sprite fades by the depth between it and the opaque geometry behind it:
// `saturate((scene − own) / distance)` in view space, the scene's depth from `t_soft` (raw, through
// the pass's planes), the sprite's own from the fragment's 1 / w. Off (`soft.x` 0): the colour as it is.
fn soften(in: VsOut, c: vec4<f32>) -> vec4<f32> {
    //@if !SOFT
    return c;
    //@endif
    //@if SOFT
    if object.soft.x <= 0.0 || globals.soft.z < 0.5 {
        return c;
    }
    let d = textureLoad(t_soft, vec2<i32>(in.clip.xy), 0).r;
    let near = globals.soft.x;
    let far = globals.soft.y;
    let scene_z = near * far / max(far - d * (far - near), 0.0001);
    let own_z = 1.0 / in.clip.w;
    let f = clamp((scene_z - own_z) / object.soft.x, 0.0, 1.0);
    var o = c;
    o.a = c.a * f;
    if object.soft.y > SOFT_FADE_WHITE - 0.5 {
        o = vec4<f32>(mix(vec3<f32>(1.0), c.rgb, f), c.a);
    } else if object.soft.y > SOFT_FADE_ALPHA + 0.5 {
        o = vec4<f32>(c.rgb * f, o.a);
    }
    return o;
    //@endif
}

// ENHANCEMENT E16: the probe's irradiance at normal `n` (world space), second-order SH: `Y00`, `Y1-1 = y`,
// `Y10 = z`, `Y11 = x`, `xy`, `yz`, `3 z^2 - 1`, `xz`, `x^2 - y^2` with the standard constants. Mean luminance over y > 0 is 1.
fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    var e = probe_sh[0].xyz * 0.282095;
    e += probe_sh[1].xyz * (0.488603 * n.y) + probe_sh[2].xyz * (0.488603 * n.z) + probe_sh[3].xyz * (0.488603 * n.x);
    e += probe_sh[4].xyz * (1.092548 * n.x * n.y) + probe_sh[5].xyz * (1.092548 * n.y * n.z);
    e += probe_sh[6].xyz * (0.315392 * (3.0 * n.z * n.z - 1.0)) + probe_sh[7].xyz * (1.092548 * n.x * n.z);
    e += probe_sh[8].xyz * (0.546274 * (n.x * n.x - n.y * n.y));
    return max(e, vec3<f32>(0.0));
}

// ENHANCEMENT E8: the environment at direction `d` (world space). The face is picked by the major axis
// and read through the same basis the face was drawn with (`gpu::REFL_FACES`: look along `fwd`, screen
// right = `up × fwd` in the game's left-handed world, the clip x being flipped, `Camera::view_projection`),
// with `1 / tan(half fov)` = `globals.refl.y` (the face is drawn a little wider than 90 degrees).
fn refl_sample(d: vec3<f32>) -> vec3<f32> {
    return refl_sample_mip(d, 0.0);
}

// ENHANCEMENT ENH-COAT: the same at an integer mip of the cube (mip m holds the radiance convolved with a GGX lobe of alpha
// 0 / 0.03 / 0.08 / 0.2 / 0.45: `coat.rs` `MIP_ALPHA`)
fn refl_sample_mip(d: vec3<f32>, mip: f32) -> vec3<f32> {
    let a = abs(d);
    var face = 0;
    var fwd = vec3<f32>(1.0, 0.0, 0.0);
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if a.x >= a.y && a.x >= a.z {
        if d.x < 0.0 {
            face = 1;
            fwd = vec3<f32>(-1.0, 0.0, 0.0);
        }
    } else if a.z >= a.y {
        face = 2;
        fwd = vec3<f32>(0.0, 0.0, 1.0);
        if d.z < 0.0 {
            face = 3;
            fwd = vec3<f32>(0.0, 0.0, -1.0);
        }
    } else {
        face = 4;
        fwd = vec3<f32>(0.0, 1.0, 0.0);
        up = vec3<f32>(0.0, 0.0, 1.0);
        if d.y < 0.0 {
            face = 5;
            fwd = vec3<f32>(0.0, -1.0, 0.0);
            up = vec3<f32>(0.0, 0.0, -1.0);
        }
    }
    let right = cross(up, fwd);
    let z = max(dot(d, fwd), 0.0001);
    let u = 0.5 + 0.5 * dot(d, right) / z * globals.refl.y;
    let v = 0.5 - 0.5 * dot(d, up) / z * globals.refl.y;
    return textureSampleLevel(t_refl, s_proj, vec2<f32>(u, v), face, mip).rgb;
}

// ENHANCEMENT ENH-COAT: the mip of the cube that holds a GGX lobe of width `alpha` (the mips' alphas: 0, 0.03, 0.08, 0.2,
// 0.45 = `coat.rs` `MIP_ALPHA`), a linear blend between the two nearest, read trilinearly by hand (the sampler's own mip
// filter may be `nearest`)
fn coat_lod(alpha: f32) -> f32 {
    if alpha <= 0.03 {
        return alpha / 0.03;
    }
    if alpha <= 0.08 {
        return 1.0 + (alpha - 0.03) / 0.05;
    }
    if alpha <= 0.2 {
        return 2.0 + (alpha - 0.08) / 0.12;
    }
    return min(3.0 + (alpha - 0.2) / 0.25, 4.0);
}

fn refl_sample_lod(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let l0 = floor(lod);
    let a = refl_sample_mip(d, l0);
    if lod - l0 < 0.001 {
        return a;
    }
    return mix(a, refl_sample_mip(d, l0 + 1.0), lod - l0);
}

//@if NEAR
// ENHANCEMENT E49: the face the direction `d` falls in (the major axis, `gpu::REFL_FACES` order)
fn near_face(d: vec3<f32>) -> i32 {
    let a = abs(d);
    if a.x >= a.y && a.x >= a.z {
        return select(0, 1, d.x < 0.0);
    }
    if a.z >= a.y {
        return select(2, 3, d.z < 0.0);
    }
    return select(4, 5, d.y < 0.0);
}

// ENHANCEMENT E49: where `d` falls in face `face` (the basis `refl_sample_mip` reads the faces through); `d` may lean a little
// past the face's middle 90 degrees (the face is drawn 96 wide)
fn near_face_uv(d: vec3<f32>, face: i32) -> vec2<f32> {
    var fwd = vec3<f32>(1.0, 0.0, 0.0);
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if face == 1 {
        fwd = vec3<f32>(-1.0, 0.0, 0.0);
    } else if face == 2 {
        fwd = vec3<f32>(0.0, 0.0, 1.0);
    } else if face == 3 {
        fwd = vec3<f32>(0.0, 0.0, -1.0);
    } else if face == 4 {
        fwd = vec3<f32>(0.0, 1.0, 0.0);
        up = vec3<f32>(0.0, 0.0, 1.0);
    } else if face == 5 {
        fwd = vec3<f32>(0.0, -1.0, 0.0);
        up = vec3<f32>(0.0, 0.0, -1.0);
    }
    let right = cross(up, fwd);
    let z = max(dot(d, fwd), 0.0001);
    return vec2<f32>(0.5 + 0.5 * dot(d, right) / z * globals.refl.y, 0.5 - 0.5 * dot(d, up) / z * globals.refl.y);
}

// ENHANCEMENT E49: the environment seen from the shading point `p` along `r`, at mip `lod`. Inside the proxy box (a covered
// stretch's, `gpu::near_proxy`: walls, floor and ceiling) the ray is cut by the box and the face is read toward the hit from the
// eye that face was drawn from (the faces are drawn a few a frame as the car moves), so the tunnel's walls, ceiling and lamp
// rows stand where they are and slide over the body as it drives (a parallax-corrected, box-projected lookup); `globals.near.z`
// (the box's weight at the eye, ramped over a portal's 6 m) blends it into the plain direction, and a point outside the box (a
// car farther along) reads the plain direction. `own` (the player's car) reads layers 6..11, the faces with the near cars.
fn near_env(p: vec3<f32>, r: vec3<f32>, lod: f32, own: bool) -> vec3<f32> {
    return near_env4(p, r, lod, own).rgb;
}

// ENHANCEMENT E49: `near_env` with the faces' alpha — NEAR-REFL2: the share of drawn geometry (not sky) in the lobe, the mask of
// `gpu/refl.rs` `near_mask` filtered with the colour (`coat.wgsl`)
fn near_env4(p: vec3<f32>, r: vec3<f32>, lod: f32, own: bool) -> vec4<f32> {
    var w = globals.near.z;
    var hit = p;
    if w > 0.0 {
        let q = p - globals.near_box[0].xyz;
        let f = globals.near_box[1].xyz;
        let l = globals.near_box[2].xyz;
        let u = globals.near_box[3].xyz;
        let pl = vec3<f32>(dot(q, f), dot(q, l), dot(q, u));
        let rl = vec3<f32>(dot(r, f), dot(r, l), dot(r, u));
        let lo = vec3<f32>(-globals.near_box[0].w, -globals.near_box[1].w, globals.near_box[2].w);
        let hi = vec3<f32>(globals.near_box[0].w, globals.near_box[1].w, globals.near_box[3].w);
        if all(pl > lo) && all(pl < hi) {
            // the exit through the box: per axis the plane ahead, the nearest of the three
            let rs = select(rl, vec3<f32>(1e-5), abs(rl) < vec3<f32>(1e-5));
            let tf = max((lo - pl) / rs, (hi - pl) / rs);
            hit = p + r * max(min(tf.x, min(tf.y, tf.z)), 0.0);
        } else {
            w = 0.0;
        }
    }
    let face = near_face(mix(r, normalize(hit - globals.near_eye[6].xyz + r * 1e-4), w));
    let d = mix(r, normalize(hit - globals.near_eye[face].xyz + r * 1e-4), w);
    let uv = near_face_uv(d, face);
    var layer = face;
    if own && globals.near.y > 0.5 {
        layer = face + 6;
    }
    let l0 = floor(lod);
    let a = textureSampleLevel(t_refl, s_proj, uv, layer, l0);
    if lod - l0 < 0.001 {
        return a;
    }
    return mix(a, textureSampleLevel(t_refl, s_proj, uv, layer, l0 + 1.0), lod - l0);
}
//@endif

// ENHANCEMENT ENH-COAT: Karis' analytic fit of the split-sum environment BRDF (UE4 mobile, `EnvBRDFApprox`): the scale and
// the bias a specular colour F0 gets, `F0 * x + y`, for a perceptual roughness and `n.v`
fn env_brdf(rough: f32, nv: f32) -> vec2<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    return vec2<f32>(-1.04, 1.04) * a004 + r.zw;
}

// ENHANCEMENT ENH-COAT: blending in linear light (the game's values are display values, gamma about 2.2): a 4 % reflection
// of the sky over a dark paint is a few percent of the sky's *light*, which is as much as the paint's own
const COAT_GAMMA: f32 = 2.2;

// ENHANCEMENT E49 (NEAR-REFL2): the cube's mip the sphere-map overlay reads (GGX alpha 0.03: soft like the 256 px `SPC2` / `SPC3`
// spread over every direction, sharp enough for the tunnel's lamps; mip 2 was compared and read flat), and the mip of the up face
// whose centre stands for "how covered is the sky over the car" (alpha 0.45, a wide cone). APPROXIMATION: both by eye.
const NEAR_OVER_LOD: f32 = 1.0;
const NEAR_OVER_UP_MIP: i32 = 4;
// ENHANCEMENT E49 (NEAR-REFL4): the painted sky's hue, the unit opponent-colour vector (r - (g + b) / 2, sqrt(3) / 2 (g - b)) of
// `SPC2`'s alpha-weighted mean 55/68/77 (`carcmn.fsh`, measured in NEAR-REFL3): (-17.5, -7.79) / 19.16, 204 degrees
const NEAR_SPC_HUE: vec2<f32> = vec2<f32>(-0.9135, -0.4069);

// ENHANCEMENT E17: the slope of a gentle procedural ripple at world point `p` (metres), `ph` the water
// material's own UV-scroll phase in turns (it wraps at 1, so every term turns a whole number of times per
// wrap): four crossing waves of 2..7 m, the total slope at most ~0.03, so the original's still water only
// shimmers. No texture: a sum of cosines.
fn water_slope(p: vec2<f32>, ph: f32) -> vec2<f32> {
    let t = ph * 6.2831853;
    var s = vec2<f32>(0.0);
    s += vec2<f32>(0.80, 0.60) * (0.011 * cos(dot(p, vec2<f32>(0.80, 0.60)) * 1.9 + t));
    s += vec2<f32>(-0.55, 0.83) * (0.009 * cos(dot(p, vec2<f32>(-0.55, 0.83)) * 2.7 - 2.0 * t));
    s += vec2<f32>(0.20, -0.98) * (0.007 * cos(dot(p, vec2<f32>(0.20, -0.98)) * 1.1 + t));
    s += vec2<f32>(-0.95, -0.31) * (0.005 * cos(dot(p, vec2<f32>(-0.95, -0.31)) * 3.4 + 3.0 * t));
    return s;
}

// ENHANCEMENT E26: two cheap hashes and a value noise for the puddle mask and the splashes' random cells
fn rain_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q = q + dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

fn rain_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(rain_hash(i), rain_hash(i + vec2<f32>(1.0, 0.0)), u.x), mix(rain_hash(i + vec2<f32>(0.0, 1.0)), rain_hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// ENHANCEMENT E26: the splash rings on a wet road. A grid of 1.4 m cells; each cell holds one drop that
// loops (period about 1.1 s, a random phase and place in the cell); its ring widens to 0.35 m and fades.
// `.xy` the slope the ring gives the surface, `.z` its brightness 0..1; only a share of the cells drop
// (`density`), and nothing further than 24 m from the eye (the rings would alias).
fn rain_rings(p: vec2<f32>, t: f32, density: f32, dist: f32) -> vec3<f32> {
    let fade = 1.0 - smoothstep(10.0, 24.0, dist);
    if fade <= 0.0 || density <= 0.0 {
        return vec3<f32>(0.0);
    }
    let cell = floor(p / 1.4);
    let h = rain_hash(cell);
    if h > density {
        return vec3<f32>(0.0);
    }
    let h2 = rain_hash(cell + vec2<f32>(7.1, 3.7));
    let h3 = rain_hash(cell + vec2<f32>(1.9, 9.3));
    let centre = (cell + vec2<f32>(0.3 + 0.4 * h2, 0.3 + 0.4 * h3)) * 1.4;
    let age = fract(t * 0.9 + h2 * 5.3 + h3);
    let d = p - centre;
    let r = length(d);
    let ring_r = age * 0.34;
    let w = 0.03 + 0.03 * age;
    let ring = exp(-((r - ring_r) / w) * ((r - ring_r) / w)) * (1.0 - age) * (1.0 - age);
    let dir = d / max(r, 0.001);
    return vec3<f32>(dir * ring * 0.35 * fade, ring * fade);
}

// A blended material with the specular / sky-map / projector passes (`CarWindow`, the alpha pages of
// `CarExtEnv`): the original draws the base pass first and each of the others after it, every one
// blended `SRCALPHA` / `INVSRCALPHA` over what is already in the target (`RE/notes/lighting.md`
// §3.1: the pass blends by the overlay texel's own alpha, whatever the base pass's alpha was). The
// result of the sequence over the target `d` is `d · Π(1 − aᵢ) + Σ cᵢ · aᵢ · Π(1 − aⱼ, j after i)`,
// which this entry writes as one premultiplied colour and the coverage `1 − Π(1 − aᵢ)` for the
// pipeline's `ONE` / `INVSRCALPHA` — not as the overlays mixed into the base colour, which weighs
// them by the base's alpha. The base pass's alpha test drops only the base layer; an overlay's own
// `GREATER 10` drops that overlay.
fn layers_color(in: VsOut) -> vec4<f32> {
    let c = shade(in, true);
    var fog = globals.fog;
    if object.sky_on.y > 0.5 {
        fog = globals.sky_fog;
    }
    if fog.w <= 0.0 {
        return c;
    }
    var d = max(-(globals.view * vec4<f32>(in.world, 1.0)).z, 0.0);
    if object.sky_on.z > 0.0 {
        d = object.sky_on.z;
    }
    // the fog is per layer (`f · cᵢ + (1 − f) · fog`), so on the premultiplied sum
    // `f · C + (1 − f) · fog · A`
    let f = exp(-fog.w * d);
    return vec4<f32>(c.rgb * f + fog.rgb * (1.0 - f) * c.a, c.a);
}

@fragment
fn fs_layers(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    return held(layers_color(in));
}

fn shade(in: VsOut, layered: bool) -> vec4<f32> {
    // ENHANCEMENT E17: the screen-space slope of the surface, taken where the control flow is uniform
    let dwx = dpdx(in.world);
    let dwy = dpdy(in.world);
    // the material's mip LOD bias, Thrash state 8 (`RE/notes/thrash.md` §7), in `shadow_on.z`
    let tex = textureSampleBias(t_diffuse, s_diffuse, in.uv, object.shadow_on.z);
    let n = normalize(in.normal);
    let diffuse = max(dot(n, normalize(globals.sun_dir.xyz)), 0.0);
    // the per-object light multiplies the ambient term only: the original keeps one ambient slot
    // per `.map` header colour and lights the car from the ground patch it stands on
    // ENHANCEMENT E9: how much of the sun reaches a car pixel (1 = all, or no shadows)
    var sun_vis = 1.0;
    var light = globals.ambient.x * object.light.rgb + vec3<f32>(1.0 - globals.ambient.x) * diffuse;
    // `R3DLightLayer` (0x446530): made per vertex, rounded to 8 bits (`layer_light`, the vertex
    // stage) and interpolated by the device, as the original does (`RE/notes/lighting.md` §6a)
    if object.lit_a.w > 0.5 {
        light = in.vlight;
        //@if CSM|PROBE
        // ENHANCEMENT E9 / E16: the sun's shadow scales only the directional term and the probe only the ambient
        // slot, so while either is on the layer is made per pixel from its two parts (the original's per-vertex,
        // rounded `in.vlight` above stays the off path)
        if csm.dir.w > 0.5 || (globals.refl.z > 0.0 && object.refl.z > 0.5) {
            // a zero direction normalises to NaN in the original (0x4E8791), whose sign bit sends every
            // vertex to the ambient arm (0x446944) — so no directional term
            var d = 0.0;
            if length(object.lit_dir.xyz) > 0.0 {
                d = max(dot(n, -normalize(object.lit_dir.xyz)), 0.0);
            }
            // ENHANCEMENT E9: only the directional term is darkened by the sun's shadow, and only while the
            // car's light IS the sun (not the straight-down light of a covered stretch); the ambient slot stays
            var sh = 1.0;
            if csm.dir.w > 0.5 && object.lit_d.w > RECV_NONE + 0.5 && object.lit_d.w < RECV_CAR + 0.5 && dot(normalize(object.lit_dir.xyz), csm.orig.xyz) > 0.999 {
                // never fully black: a floor of a quarter of the direct term (the sky's own scatter)
                var floor_ = 0.25;
                if csm.params.w > 0.0 {
                    floor_ = csm.params.w;
                }
                sh = floor_ + (1.0 - floor_) * csm_visibility(in.world, n);
                sun_vis = sh;
                //@if MOON
                // ENHANCEMENT E50: a moon frame leaves the original's own direct term unshadowed (on Industrial it comes from the same
                // direction, but in the bake that is the street's warm light, not the moon's); the moon's cold term below has the shadow
                if csm.moon.x > 0.0 {
                    sh = 1.0;
                    sun_vis = 1.0;
                }
                //@endif
            }
            // ENHANCEMENT E16: the ambient slot's colour takes the surroundings' direction and colour (`mix` towards
            // the probe by `globals.refl.z`; the probe's mean luminance is 1, so the slot's level stays the original's)
            var amb = object.lit_a.rgb;
            if globals.refl.z > 0.0 && object.refl.z > 0.5 {
                amb = amb * max(mix(vec3<f32>(1.0), sh_irradiance(n), globals.refl.z), vec3<f32>(0.0));
            }
            light = min(amb + object.lit_d.rgb * (d * sh), vec3<f32>(1.0));
            //@if MOON
            // ENHANCEMENT E50: the moon's weak cold direct term on the cars (its own shadow: the cascades), added to the original's light
            if csm.moon.y > 0.0 && object.lit_d.w > RECV_NONE + 0.5 && object.lit_d.w < RECV_CAR + 0.5 {
                let dm = max(dot(n, -csm.orig.xyz), 0.0);
                if dm > 0.0 {
                    light = light + csm.moon_col.rgb * (csm.moon.y * dm * csm_visibility(in.world, n));
                }
            }
            //@endif
        }
        //@endif
    }
    // ENHANCEMENT E43: the near cars' contact shadow on the track (`sky_on.w` 1: a receiver), the sky's occlusion `ao` (1 = none)
    //@if CONTACT
    let contact_rx = contacts.params.x > 0.5 && object.sky_on.w > 0.5;
    //@endif
    var ao = 1.0;
    //@if CONTACT
    if contact_rx {
        ao = contact_keep(in.world, n);
    }
    //@endif
    // ENHANCEMENT E9: the track has no direct term of its own (baked vertex light x texture), so a shadow
    // scales its colour by `1 - s (1 - visibility)`, `s` the strength x how much the face looks at the sun
    // x how bright the baked colour is (a baked shadow is not darkened twice)
    //@if CSM
    if csm.dir.w > 0.5 && object.lit_d.w > RECV_TRACK - 0.5 {
        let facing = mix(0.35, 1.0, smoothstep(0.0, 0.25, dot(n, -csm.dir.xyz)));
        var bright = smoothstep(0.3, 0.7, dot(in.color.rgb, vec3<f32>(0.299, 0.587, 0.114)));
        // ENHANCEMENT ENH-GARAGE (`extra.x` > 0, the front end's focus shadow): the room's floor is baked dark, the car's
        // shadow must still read there: no "already dark" gate
        if csm.extra.x > 0.0 {
            bright = 1.0;
        }
        // ENHANCEMENT E25 (`csm.params.z` > 1): the sunless tracks' baked colours are dim (a forest floor, wet asphalt), so the
        // "already baked dark" gate would switch the shadow off nearly everywhere; here it only eases it a little
        if csm.params.z > 1.0 {
            bright = mix(0.6, 1.0, bright);
        }
        //@if MOON
        // ENHANCEMENT E50: the moon's share of a night's baked colour is the dim part of it: a pool of lamp light (bright in the bake)
        // is the lamp's, not the moon's, so the gate turns round (full below a luma of 0.25, none above 0.6)
        if csm.moon.x > 0.0 {
            bright = 1.0 - smoothstep(0.25, 0.6, dot(in.color.rgb, vec3<f32>(0.299, 0.587, 0.114)));
        }
        //@endif
        let a0 = csm.params.x * facing;
        let s = a0 * bright;
        var want = s > 0.0 || ao < 1.0;
        //@if CARSH
        // ENHANCEMENT E51: the cars' own shadow is wanted on a dimly baked face too
        let carsh = csm.carsh.x > 0.5 && s < a0;
        want = want || carsh;
        //@endif
        if want {
            // ENHANCEMENT E9 (SHADOW-REAL): x the cascades' visibility, y the share of the sun the shadow is drawn for here (0 under
            // a covered box, beyond the cascades)
            let t = csm_split(in.world, n);
            var keep_sun = 1.0 - s * t.y * (1.0 - t.x);
            //@if CARSH
            // ENHANCEMENT E51: the cars' shadow passes the "baked dark" gate (a car's shadow is never baked into the road): the point
            // keeps the darker of the gated shadow of everything and the ungated shadow of the cars alone (in full light, `bright` 1,
            // the two agree and nothing changes)
            if carsh && t.y > 0.0 {
                keep_sun = min(keep_sun, 1.0 - a0 * t.y * (1.0 - car_visibility(in.world, n)));
            }
            //@endif
            light = light * keep_sun;
            // ENHANCEMENT E25 x E43 (LAMP-TUNE): under the overcast light both are the sky's occlusion by the same car (E25's soft
            // shadow of the tilted overcast light, E43's of the underside): the point keeps the darker of the two, not their product
            // (the product darkened the car's own shadow twice: 0.4 x 0.15 under the body, a heavy black patch on the sunless tracks)
            if ao < 1.0 && csm.params.z > 1.0 {
                ao = min(ao, keep_sun) / max(keep_sun, 0.0001);
            }
            // ENHANCEMENT E43 (SHADOW-REAL): the contact shadow is the sky's occlusion, so it takes only the sky's (ambient) share of
            // the light, `1 - a`, and the sun's (direct) share `a` keeps the sun's visibility: of the light left at this point the
            // occlusion leaves `((1 - a) ao + a v) / ((1 - a) + a v)`. In the sun's light (v 1) it is a faint crease at the tyres and
            // sills; inside the car's own sun shadow (v 0) it is whole and deepens it under the body: one shadow, the car grounded.
            // `a` is E9's strength x facing x the share drawn here, without the "baked dark" gate (the map knows where the car
            // blocks the sun even where the bake is dim). Under cover, at night, beyond the cascades (a 0) it is the whole occlusion,
            // as before. Not under E25's overcast light (`params.z` > 1): its soft shadow is itself a sky occlusion (the product).
            if ao < 1.0 && csm.params.z <= 1.0 {
                var a = a0 * t.y;
                //@if MOON
                // ENHANCEMENT E50: the moon's direct share is gated like its shadow (under a lamp the baked light is the lamp's)
                if csm.moon.x > 0.0 {
                    a = s * t.y;
                }
                //@endif
                let sky = 1.0 - a;
                ao = (sky * ao + a * t.x) / max(sky + a * t.x, 0.0001);
            }
        }
    }
    //@endif
    //@if CONTACT
    if contact_rx {
        light = light * ao;
    }
    //@endif
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
        let env = textureSampleLevel(t_env, s_env, in.env_uv, 0.0);
        if env.a * 255.0 > 10.0 {
            env_over = env;
        }
        //@if NEAR
        // ENHANCEMENT E49 (NEAR-REFL2): on the player's own car (`refl.w`) the sphere map's sky gives way to what the live faces
        // see where that is not sky: the cube along the reflected view vector (box-projected, the faces with the near cars) at a
        // soft mip, its geometry share `a` (the faces' mask: 1 track / objects / cars, 0 sky) the weight. The texel's alpha (the
        // layer's strength) and the `x light` below stay the original's; where the faces see sky the overlay is the original's
        // exactly. Rays below the horizon (lower panels, which see the road) take the share of the sky that is covered over the
        // car instead (the up face's coarsest mip): in the open they keep the original's sky, under a roof they show the road.
        if globals.near.x > 0.0 && object.refl.w > 0.5 && env_over.a > 0.0 {
            let rv = reflect(normalize(in.world - globals.cam_pos.xyz), n);
            let seen = near_env4(in.world, rv, NEAR_OVER_LOD, true);
            let up = textureSampleLevel(t_refl, s_proj, vec2<f32>(0.5, 0.5), select(4, 10, globals.near.y > 0.5), f32(NEAR_OVER_UP_MIP)).a;
            let cover = clamp(mix(up, seen.a, smoothstep(-0.25, 0.0, rv.y)), 0.0, 1.0);
            let live = clamp(seen.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
            if globals.near.x > 1.5 && globals.near.x < 2.5 {
                // NEAR-REFL3: on a night track (`Scene::near_sky` = `NearSky::Night`) the sphere map's day sky (`SPC2` / `SPC3` are the same
                // cloudy day sky on every track, `carcmn.fsh` from `gamedata\carmodel\`, 0x4437BB…0x4437CC / 0x441AB2…0x441AEF) is
                // wrong in the sky parts too: they show the faces' own sky (the sky pass is in them) as well. Its colour is the
                // live one; its strength is the texel's alpha scaled by how bright the live sky is against the painted texel
                // (`lum(live) / lum(spc)`, at most 1): a black night sky lays no veil over the paint (it reflects nothing), a lit
                // window, a lamp or the glowing sky of an industrial night as strongly as the original's sky. Geometry
                // (`cover`) keeps NEAR-REFL2's full strength. APPROXIMATION: the luminance ratio as the strength (by eye).
                let k = vec3<f32>(0.299, 0.587, 0.114);
                let s = clamp(dot(live, k) / max(dot(env_over.rgb, k), 0.004), 0.0, 1.0);
                env_over = vec4<f32>(live, env_over.a * mix(s, 1.0, cover));
            } else if globals.near.x > 2.5 {
                // NEAR-REFL4: on a day / dusk track (`Scene::near_sky` = `NearSky::Hue`) the sky parts take the faces' own sky's
                // hue where it departs from the painted sky's: the opponent-colour hue of the live texel against the map's
                // (`NEAR_SPC_HUE`, the alpha-weighted mean of `SPC2` 55/68/77; `SPC3` 73/83/92 has the same hue within 4
                // degrees), weight 0 within 20 degrees, 1 past 45, and 0 for a live sky too grey to carry a hue. So the
                // blue / grey-blue / grey day skies (forest, canyon, autobahn) keep the original's overlay, a pink / orange /
                // peach / lavender dusk tints it. The colour is the live chroma `live / lum(live)` at the texel's own
                // luminance and the texel's alpha stays the strength: the paint is veiled exactly as much as by the original's
                // sky, never brighter (the sun disc, white in the faces, has no hue and gives the texel). Geometry (`cover`)
                // keeps NEAR-REFL2's rule. APPROXIMATION: the 20..45 degree fade, the 0.05..0.15 saturation gate and the
                // 0.004..0.02 luminance fade (by eye).
                let k = vec3<f32>(0.299, 0.587, 0.114);
                let ll = dot(live, k);
                let oc = vec2<f32>(live.r - 0.5 * (live.g + live.b), 0.8660254 * (live.g - live.b));
                let along = dot(oc, NEAR_SPC_HUE) / max(length(oc), 1e-5);
                let w = (1.0 - smoothstep(0.7071, 0.9397, along)) * smoothstep(0.05, 0.15, length(oc) / max(ll, 0.004))
                    * smoothstep(0.004, 0.02, ll);
                let hue = clamp(live * (dot(env_over.rgb, k) / max(ll, 0.004)), vec3<f32>(0.0), vec3<f32>(1.0));
                env_over = vec4<f32>(mix(mix(env_over.rgb, hue, w), live, cover), env_over.a);
            } else {
                env_over = vec4<f32>(mix(env_over.rgb, live, cover), env_over.a);
            }
            // TEST GLUE (`NFS_NEAR_DEBUG=5`): the overlay alone, red the weight, green the ray's own geometry share, blue the up face's
            if globals.near.w > 2.5 {
                env_over = vec4<f32>(cover, seen.a, up, 1.0);
            }
        }
        //@endif
    }
    var lit = base.rgb * light;
    // `layered`: `lit` is the premultiplied sum of the layers so far, `cov` its coverage
    let base_a = base.a * object.light.a;
    var cov = 1.0;
    if layered {
        cov = base_a;
        if object.shadow_on.w > 0.0 && base_a * 255.0 < object.shadow_on.w {
            cov = 0.0;
        }
        lit = lit * cov;
    }
    if layered {
        lit = lit * (1.0 - env_over.a) + env_over.rgb * light * env_over.a;
        cov = cov * (1.0 - env_over.a) + env_over.a;
    } else {
        lit = mix(lit, env_over.rgb * light, env_over.a);
    }
    // `R3DSkyMapTextureLayer`, pass 2 (option mask 2 on `CarExtEnv` / `CarExtEnvNS`, mask 0 on
    // `CarWindow`): the same three-op pass as the specular one — the `Rs` block `{1, 2, 0, 1, 0,
    // −1}` (`rmthpasses`: key 0x52730006), the layer and the draw — so the section map × the light
    // colour, laid over by its texel alpha under the alpha test `GREATER 10`. The `.env` maps are
    // loaded like `carcmn.fsh` (0x440AAA → 0x44D270), so `CLAMP`, level 0.
    if object.sky_on.x > 0.5 {
        let sky = textureSampleLevel(t_sky, s_proj, in.sky_uv, 0.0);
        if sky.a * 255.0 > 10.0 {
            if layered {
                lit = lit * (1.0 - sky.a) + sky.rgb * light * sky.a;
                cov = cov * (1.0 - sky.a) + sky.a;
            } else {
                lit = mix(lit, sky.rgb * light, sky.a);
            }
        }
    }
    // `R3DShadowMapTextureLayer` (0x446130), pass 3 with the same `Rs` block: the projector's
    // texture (`sh80`, a flat dark colour under an alpha of 0…253) × the light colour, blended over
    // by its alpha under the same `GREATER 10` test
    // the headlight decal, `Opaque`'s second pass: the first pass's pixel × 2 · `HEAD`
    // (0x453CA3: only while lamp 0 of the camera's car is lit, `headlight_on.y`)
    if object.shadow_on.y > 0.5 && globals.headlight_on.y > 0.5 && in.head_w > 0.0 {
        var head = uvlight(in.head_uvw);
        if globals.headlight_on.z > 0.0 {
            // ENHANCEMENT E24: a car's lamps fading in or out at a tunnel mouth
            head = mix(vec3<f32>(1.0), head, 1.0 - globals.headlight_on.z);
        }
        lit = lit * head;
    }
    // ENHANCEMENT E15: the headlight spots on top of what the original made (its beam, the `HEAD` decal),
    // under the car's blob shadow; `soft.z` 1 a car (a small floor), 2 a track item (a larger one)
    //@if SPOT|LAMPS
    if object.soft.z > RECV_NONE + 0.5 && (spots.params.x > 0.5 || lamps.params.x > 0.5) {
        // (`layered`: the lights add to the premultiplied sum by the coverage)
        var tcov = 1.0;
        if layered {
            tcov = cov;
        }
        var floor_ = 0.45;
        if object.soft.z < RECV_CAR + 0.5 {
            floor_ = 0.1;
        }
        //@if SPOT
        if spots.params.x > 0.5 {
            lit = lit + tex.rgb * spot_light(in.world, n, floor_) * 1.25 * tcov;
        }
        //@endif
        // ENHANCEMENT E23: the street lamps, the same receiver model
        //@if LAMPS
        if lamps.params.x > 0.5 {
            var car_k = 1.0;
            if object.soft.z < RECV_CAR + 0.5 {
                car_k = 0.6;
            }
            lit = lit + tex.rgb * lamp_light(in.world, n, floor_) * car_k * tcov;
        }
        //@endif
    }
    //@endif
    if object.shadow_on.x > 0.5 {
        let uv = vec2<f32>(dot(object.shadow_u.xyz, in.world) + object.shadow_u.w, dot(object.shadow_v.xyz, in.world) + object.shadow_v.w);
        // `sh80`'s material asks for a LOD bias of −1 (`mt+0x40`, `RE/notes/thrash.md` §7)
        let sh = textureSampleBias(t_proj, s_proj, uv, -1.0);
        if sh.a * 255.0 > 10.0 {
            // ENHANCEMENT E9 (SHADOWFIX): where the sun's shadow map draws the car's shadow (a sun, in the open, E9 on) the
            // original's blob under the car gives way to it entirely (2026-10-03: two shadows under one car; was 30 %
            // left as a contact darkening); under cover (gate 0), at night and with E9 off the blob is the original's, whole
            var blob = sh.a;
            // (not under E25's overcast light, `params.z` > 0: its car shadow is too faint to stand in for the blob)
            // (the garage's `FESH` blob, `extra.x` > 0: ENH-GARAGE fades it below, once the focus shadow has a contact core)
            // ENHANCEMENT E9 (SHADOW-REAL): by the point's own cover (`csm_share`), not by where the camera is
            //@if CSM
            if csm.dir.w > 0.5 && csm.extra.x <= 0.0 && csm.params.z <= 0.0 {
                blob = blob * (1.0 - csm_share(in.world));
            }
            // ENHANCEMENT ENH-GARAGE: the garage's `FESH` blob gives way to the focus shadow (`extra.w`, its core carries the contact)
            if csm.dir.w > 0.5 && csm.extra.x > 0.0 {
                blob = blob * (1.0 - csm.extra.w);
            }
            //@endif
            // ENHANCEMENT E43: the same where a car's contact shadow stands in for it (under cover, at night, E9 off, E25's overcast:
            // where E9's own gate has not already taken it)
            //@if CONTACT
            if contacts.params.x > 0.5 && csm.extra.x <= 0.0 {
                blob = blob * (1.0 - contact_presence(in.world));
            }
            //@endif
            if layered {
                // the blended material's projector pass over the layers so far (`fs_layers`), by the (enhancement-eased) coverage
                lit = lit * (1.0 - blob) + sh.rgb * light * blob;
                cov = cov * (1.0 - blob) + blob;
            } else {
                lit = mix(lit, sh.rgb * light, blob);
            }
        }
    }
    // (`layered`: `lit` stays the premultiplied sum of the layers and `alpha` is their coverage; the enhancements below that
    // work on a straight colour take it out and put it back)
    var alpha = base_a;
    if layered {
        alpha = cov;
    }
    //@if CSM
    // ENHANCEMENT ENH-DBLSH (`lit_d.w` 3): a track's `sh80` ground-shadow decal gives way to the sun's shadow of the object
    // over it where the cascades cover it (open sky; fades back in toward the last cascade's end, where there is no shadow)
    if csm.dir.w > 0.5 && object.lit_d.w > RECV_DECAL - 0.5 {
        let vd = -(globals.view * vec4<f32>(in.world, 1.0)).z;
        let last = csm.info[i32(csm.dir.w) - 1].z;
        let f = clamp((vd - csm.params.y) / max(last - csm.params.y, 0.001), 0.0, 1.0);
        // ENHANCEMENT E9 (SHADOW-REAL): the gate by the decal's own cover (`csm_share`)
        let g = csm_share(in.world);
        if layered {
            lit = lit * (1.0 - g * (1.0 - f));
        }
        alpha = alpha * (1.0 - g * (1.0 - f));
    }
    //@endif
    // ENHANCEMENT E8 / ENH-COAT: the car paint's clear coat (a physically based one, Filament / Unreal style) and the glass.
    // Paint (class 1: a `CarExtEnv` material with a reflection strength) is two layers: the base paint (`lit`: the original's
    // lit colour with its sphere-map / sky-map highlights) under a dielectric coat of F0 0.04 (IOR 1.5). The coat takes
    // `Fc` of the light — the split-sum directional albedo `F0 * A + B` (`env_brdf`, Karis' fit) for the coat's roughness — and
    // gives it back as the prefiltered environment at the reflected view vector: `lit = lit (1 - Fc) + Fc env`, in linear
    // light, so the paint keeps its colour (it is attenuated, never filled with the environment's). The coat's roughness is
    // the level's GGX alpha (`globals.refl.w`: 0.03 / 0.08 / 0.2), the lobe's width = a mip of the cube. The reflection
    // fades under the facet's own horizon (Filament: `1 + 1.3 r.n_geom`, squared: a smooth normal of a flat low-poly panel
    // must not reflect what is behind the panel) and where the normal faces the ground (specular occlusion: sills do not glow
    // with the road). Glass (class 2, `CarWindow`) is blended: its coverage grows by `Fc` (F0 0.04, a smooth lobe) and the
    // colour carries the reflection. Radiance is not clamped here (the sun above 1: a glint; `held` clamps an 8-bit target at
    // the end). Not in the rear-view mirror's pass (`ambient.z`), not on an alpha-tested-out texel.
    //@if COAT
    if object.refl.x > REFL_PAINT - 0.5 && object.refl.x < REFL_GLASS + 0.5 && globals.refl.x > 0.0 && globals.ambient.z < 0.5 && (object.shadow_on.w <= 0.0 || alpha * 255.0 >= object.shadow_on.w) {
        var v = normalize(globals.cam_pos.xyz - in.world);
        let nv = clamp(dot(n, v), 0.001, 1.0);
        let r = reflect(-v, n);
        var ng = normalize(cross(dwx, dwy));
        if dot(ng, v) < 0.0 {
            ng = -ng;
        }
        let glass = object.refl.x > REFL_GLASS - 0.5;
        var alpha_g = globals.refl.w;
        if glass {
            alpha_g = 0.03;
        }
        let ab = env_brdf(sqrt(alpha_g), nv);
        var fc = (0.04 * ab.x + ab.y) * globals.refl.x;
        // ENHANCEMENT ENH-GRILLE: the coat lies on PAINT. A texel of the page that is (near) black is not paint but an opening or
        // black plastic (the rear engine grille's slats, 0x00 between grey bars): a 4 % sky over a black texel is ~40 / 255 after the
        // display gamma, and the dark slats turned into a flat grey plate. The weight rises from 0 at max(rgb) 0.03 to 1 at 0.15
        // (a dark paint is well above it); glass keeps its own coverage rule.
        if !glass {
            fc = fc * smoothstep(0.03, 0.15, max(tex.r, max(tex.g, tex.b)));
        }
        var horizon = clamp(1.0 + 1.3 * dot(r, ng), 0.0, 1.0);
        horizon = horizon * horizon;
        // specular occlusion: the lower hemisphere's share, from how far down the surface and the ray face
        let down = smoothstep(0.0, -0.6, n.y) * 0.5 + smoothstep(0.05, -0.5, r.y) * 0.4;
        let occ = 1.0 - clamp(down, 0.0, 0.8);
        //@if !NEAR
        let env = min(refl_sample_lod(r, coat_lod(alpha_g)), vec3<f32>(64.0)) * horizon * occ;
        //@endif
        //@if NEAR
        // ENHANCEMENT E49: from the shading point (the box-projected lookup), the player's own car from the faces with the near cars
        var env_in = vec3<f32>(0.0);
        if globals.near.x > 0.0 {
            env_in = near_env(in.world, r, select(coat_lod(alpha_g), 0.0, globals.near.w > 1.5), object.refl.w > 0.5);
        } else {
            env_in = refl_sample_lod(r, coat_lod(alpha_g));
        }
        let env = min(env_in, vec3<f32>(64.0)) * horizon * occ;
        // TEST GLUE (`NFS_NEAR_DEBUG`): the paint shows the reflection alone
        if globals.near.w > 0.5 && globals.near.w < 2.5 {
            fc = 1.0;
        }
        //@endif
        if glass {
            let k = clamp(fc, 0.0, 0.95);
            let a = alpha + k * (1.0 - alpha);
            if layered {
                // premultiplied: the reflection adds `env k` over the part of the target the layers leave through
                lit = lit + env * k * (1.0 - alpha);
            } else {
                lit = (lit * alpha + env * k * (1.0 - alpha)) / a;
            }
            alpha = a;
        } else {
            var straight = lit;
            if layered {
                straight = lit / max(alpha, 0.0001);
            }
            let lin = pow(max(straight, vec3<f32>(0.0)), vec3<f32>(COAT_GAMMA)) * (1.0 - fc) + pow(env, vec3<f32>(COAT_GAMMA)) * fc;
            lit = pow(lin, vec3<f32>(1.0 / COAT_GAMMA));
            if layered {
                lit = lit * alpha;
            }
        }
    }
    //@endif
    // ENHANCEMENT E17: water (class 3: a flat water material, `nfs_assets::track::is_water`). The original
    // texture and its scroll stay as the base; over it a Fresnel-weighted reflection of the sky (the E8
    // environment map when E8 is on, else the sky's own colours), a sun glint (above 1.0: HDR where the
    // target keeps it, E7) and a faint moving ripple that only tilts the reflection. Level surfaces only
    // (a waterfall or a river on a slope is left as the original draws it).
    //@if WATER
    if object.refl.x > REFL_WATER - 0.5 && globals.water.x > 0.5 && abs(cross(dwx, dwy).y) > 0.8 * length(cross(dwx, dwy)) {
        let vv = globals.cam_pos.xyz - in.world;
        let dist = length(vv);
        let v = vv / max(dist, 0.001);
        // the ripple fades out with distance (it would alias to noise) and is the original's still sea up close
        let amp = 1.0 - smoothstep(60.0, 220.0, dist);
        // level 2 («сильнее», `globals.water.w` = 1): a stronger ripple, a higher reflection weight, a brighter glint
        let wst = globals.water.w;
        let sl = water_slope(in.world.xz, object.env_mix.w) * amp * (1.0 + 1.6 * wst);
        let nw = normalize(vec3<f32>(-sl.x, 1.0, -sl.y));
        let nv = clamp(dot(nw, v), 0.0, 1.0);
        // water's F0 is 0.02; the weight is held below 1 so the texture always shows through
        let fres = 0.02 + 0.98 * pow(1.0 - nv, 5.0);
        let k = clamp(fres * (0.9 + 0.8 * wst) + 0.10 * wst, 0.0, 0.8 + 0.1 * wst);
        var r = reflect(-v, nw);
        r.y = max(r.y, 0.02);
        var env = vec3<f32>(0.0);
        if globals.refl.x > 0.0 && globals.water.z > 0.5 {
            env = refl_sample(r);
        } else {
            env = mix(globals.water_horizon.rgb, globals.water_zenith.rgb, smoothstep(0.0, 0.55, r.y));
        }
        var glint = vec3<f32>(0.0);
        if globals.water.y > 0.5 {
            let sd = max(dot(r, globals.water_sun.xyz), 0.0);
            // a hard core and a wide sheen, both stronger toward grazing views (the sun path on the sea)
            glint = vec3<f32>(1.0, 0.93, 0.78) * (pow(sd, 6000.0) * 5.0 + pow(sd, 500.0) * 0.2 + pow(sd, 60.0) * 0.10 * wst) * (0.25 + 0.75 * fres) * (1.0 + 1.5 * wst);
        }
        lit = mix(lit, env, k) + glint;
        alpha = clamp(alpha + (1.0 - alpha) * min(k + max(glint.r, max(glint.g, glint.b)), 1.0), 0.0, 1.0);
    }
    //@endif
    //@if RAIN
    // ENHANCEMENT E26: the wet road (class 4: a tarmac part, `object.refl.w` its slice's wetness 0..1 from the
    // sky `.bin`'s field 5). Over what the original made: (1) the albedo darkens (to ~60 %, more in a puddle);
    // (2) a soft Fresnel reflection of the sky (the sky pass's colours, no geometry: it is a dusk, not a
    // mirror, so the weight is held under 0.5 and a rough floor lets the texture show); (3) the lights near the
    // camera (cars' lamps) streak vertically in it - the reflection is matched to each lamp in azimuth (narrow)
    // and elevation (long toward the eye, as a rough surface smears it); (4) gentle puddles from world-space
    // noise, more where it is wetter; (5) splash rings near the eye. Dry parts (`wet` 0) never get here.
    // ENHANCEMENT E26 (skid marks, class 5 = `object.refl.x` 5): on a wet stretch the rubber of a skid is not black: the
    // tyre wiped the water film off, so the strip darkens much less (~0.4 of its dry opacity at full wetness) and shows
    // the same sky / lamp reflection as the wet road around it (a lighter, glossy streak). Dry: skipped.
    if object.refl.x > REFL_WET_ROAD - 0.5 && globals.rain.x > 0.0 {
        let skid = object.refl.x > REFL_SKID - 0.5;
        // the part's own wetness (its centre's slice); near a step of the original's field 5 the fragment takes the
        // ramp of the nearest edge instead (a part is longer than the step, and the original's step is one slice wide)
        var wet = clamp(object.refl.w, 0.0, 1.0);
        var near_edge = 80.0;
        for (var i = 0u; i < u32(globals.rain.w); i = i + 1u) {
            let off = in.world - globals.rain_edge_pos[i].xyz;
            let s = dot(off, globals.rain_edge_dir[i].xyz);
            if abs(s) < near_edge && dot(off, off) < 8100.0 {
                near_edge = abs(s);
                let hw = globals.rain_edge_pos[i].w;
                wet = mix(globals.rain_edge_dir[i].w, globals.rain_edge_val[i].x, smoothstep(-hw, hw, s));
            }
        }
        if wet > 0.004 {
        let g = globals.rain.x;
        let vv = globals.cam_pos.xyz - in.world;
        let dist = length(vv);
        let v = vv / max(dist, 0.001);
        let pn = rain_noise(in.world.xz * 0.19) * 0.62 + rain_noise(in.world.xz * 0.61 + vec2<f32>(17.0, 5.0)) * 0.38;
        // puddles grow with the wetness and only form in real rain (a damp 0.5 stretch has few)
        let pud = smoothstep(0.60 - 0.16 * wet, 0.74 - 0.16 * wet, pn) * smoothstep(0.4, 0.9, wet);
        let wetf = clamp(wet * g, 0.0, 1.0);
        // the albedo darkens less where it is bright (the lane markings stay readable)
        let luma = dot(base.rgb, vec3<f32>(0.299, 0.587, 0.114));
        let keep = smoothstep(0.22, 0.45, luma);
        if !skid {
            lit = lit * mix(1.0, mix(0.85, 1.0, keep), wetf) * mix(1.0, 0.78, pud * wetf * (1.0 - keep));
        }
        // the splash rings: density by the wetness; they tilt the normal a little and glint
        let rg = rain_rings(in.world.xz, globals.rain.y, 0.16 * wet * g, dist);
        let nw = normalize(vec3<f32>(rg.x, 1.0, rg.y));
        let nv = clamp(dot(nw, v), 0.0, 1.0);
        let fres = 0.05 + 0.95 * pow(1.0 - nv, 5.0);
        // far, grazing road reflects more; the puddles most
        var k = clamp(fres * (0.40 + 0.55 * pud) * wetf, 0.0, 0.55) * (1.0 - 0.5 * keep);
        if skid {
            // the wiped streak is smoother than the road round it: a stronger reflection, whatever the rubber's colour
            k = clamp(fres * 0.9 * wetf + 0.6 * wetf, 0.0, 0.85);
        }
        let r = reflect(-v, nw);
        let sky = mix(globals.rain_horizon.rgb, globals.rain_zenith.rgb, smoothstep(0.0, 0.6, max(r.y, 0.0)));
        lit = mix(lit, sky * 0.85, k) + sky * rg.z * 0.22 * wetf;
        // the lamps' streaks: azimuth difference narrow, elevation difference long on the eye's side
        var streak = vec3<f32>(0.0);
        let rh = normalize(r.xz + vec2<f32>(0.0001, 0.0));
        for (var i = 0u; i < u32(globals.rain.z); i = i + 1u) {
            let d = globals.rain_lamp_pos[i].xyz - in.world;
            let dl = length(d);
            let dh = normalize(d.xz + vec2<f32>(0.0001, 0.0));
            if dot(rh, dh) > 0.0 {
                let az = rh.x * dh.y - rh.y * dh.x;
                let dv = r.y - d.y / dl;
                var sv = 0.06;
                if dv > 0.0 {
                    sv = 0.2;
                }
                let sa = 0.035 + 0.02 * pud;
                let e = exp(-(az / sa) * (az / sa) - (dv / sv) * (dv / sv));
                // AUTOBAHN-LAMPS: the light's own reach (`w` = reach squared; 0 = the street lamps' 4000): a car's
                // tail light no longer streaks as far and as bright as a street lamp
                let r2 = select(4000.0, globals.rain_lamp_pos[i].w, globals.rain_lamp_pos[i].w > 0.0);
                streak += globals.rain_lamp_col[i].rgb * (e / (1.0 + dl * dl / r2));
            }
        }
        lit = lit + streak * (0.13 + 0.22 * fres) * (0.35 + 0.65 * pud) * wetf;
        if skid {
            // the opacity falls (the alpha test's reference keeps its texels: it reads this alpha) and the reflection
            // lifts it again where the streak glints
            let floor_a = min(alpha, object.shadow_on.w / 255.0 + 0.004);
            alpha = max(alpha * mix(1.0, 0.8, wetf), floor_a);
            alpha = clamp(alpha + (1.0 - alpha) * k * 0.5 * min(alpha * 5.0, 1.0), 0.0, 1.0);
            lit = lit + sky * 0.3 * wetf;
        }
        }
    }
    //@endif
    return vec4<f32>(lit, alpha);
}
