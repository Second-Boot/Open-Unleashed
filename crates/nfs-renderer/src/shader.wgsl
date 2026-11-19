// (Components no code reads, written by `gpu/fill.rs`: `sun_dir.w`, `ambient.y`, `cam_pos.w`, `headlight_on.w`, `beacon_on.yzw`,
// `soft.w`; `Object::refl.y`; `Csm::misc.zw`, `moon.w`. They stay while the Rust layouts have them.)
struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    sun_dir: vec4<f32>,
    // x the scene's ambient share, z 1 in the rear-view mirror's view (`[0x6115CC]+0x190`), w ENHANCEMENT E7: 1 = the output keeps
    // values above 1
    ambient: vec4<f32>,
    cam_pos: vec4<f32>,
    headlight: mat4x4<f32>,
    // x 1 while the projector lights, y 1 while the HEAD decal does, z ENHANCEMENT E24: 1 - the projector's gain
    headlight_on: vec4<f32>,
    beacon: mat4x4<f32>,
    // x 1 while the police beacon lights
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
@group(0) @binding(B_GLOBALS) var<uniform> globals: Globals;
// `night.psh`'s `nght`, 128 × 128 bytes packed four to a u32
@group(0) @binding(B_NIGHT_MAP) var<uniform> night_map: array<vec4<u32>, 1024>;
// the car shadow projector's texture (`[0x6115CC]+0x18C`)
@group(0) @binding(B_PROJ_TEX) var t_proj: texture_2d<f32>;
@group(0) @binding(B_PROJ_SAMPLER) var s_proj: sampler;
@group(0) @binding(B_HEAD_TEX) var t_head: texture_2d<f32>;
// ENHANCEMENT E8: the six faces of the player's car's environment (+X, -X, +Z, -Z, +Y, -Y)
@group(0) @binding(B_REFL) var t_refl: texture_2d_array<f32>;
// ENHANCEMENT E12: the opaque part's raw depth (R32Float, single sample), read per pixel
@group(0) @binding(B_SOFT) var t_soft: texture_2d<f32>;
// ENHANCEMENT E16: the car ambient probe, the SH coefficients (rgb) of the irradiance normalised to a upper-hemisphere-mean luminance of 1
@group(0) @binding(B_PROBE_SH) var<uniform> probe_sh: array<vec4<f32>, SH_COEFFS>;

// ENHANCEMENT E9: the sun's shadow cascades (`gpu/csm/`). `dir.w` is the cascade count, 0 = off (every
// path below is then skipped and the shading is the original's).
struct Csm {
    mats: array<mat4x4<f32>, 4>,
    // per cascade: x a texel in metres, y the depth range in metres, z the view depth it ends at
    info: array<vec4<f32>, 4>,
    // xyz the way the light travels, w the cascade count
    dir: vec4<f32>,
    // x the track's shadow strength, y the view depth where the last cascade fades out, z 1 = the track's baked-dark gate off
    // (FOREST-TREESH), 2 = relative to the track's bake light (GATE-REL), else 0, w the car's floor of the direct term
    params: vec4<f32>,
    // ENHANCEMENT E9 (SHADOWFIX): xyz the light the cars are lit with (the shadow's own is lifted to a higher sun), w the
    // cover gate (1 = open sky; the shadows fade out under cover)
    orig: vec4<f32>,
    // ENHANCEMENT ENH-FE3D: the front-end car's soft edge (x) and contact core (y) radii in texels, the penumbra's share (z), the
    // `FESH` blob's fade (w)
    extra: vec4<f32>,
    // x (ENHANCEMENT E9, ENH-TUNPOOL) the covered boxes' count, y (SHADOW-REFACTOR) the frame's mode (`SHADOW_*`, `csm_mode_is`);
    // per box 5 vec4: (centre, half length), (forward, half width), (lateral, weight at the back face), (up, weight at the front
    // face), (ceiling height over the centre, nominal half length, 0, 0)
    misc: vec4<f32>,
    // GATE-REL (`gpu/csm/mode.rs` `bake_uniform`): xyz toward the track's bake light, w its base luma; then x its gain, y the drivable
    // ground's factor (zw 0)
    bake: vec4<f32>,
    bake_k: vec4<f32>,
    // ENHANCEMENT E9 (FOREST-E9PERF): the world box round every covered box (xyz least, xyz most): outside it a point is in none
    cover_lo: vec4<f32>,
    cover_hi: vec4<f32>,
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
@group(0) @binding(B_CSM_MAP) var t_shadow: texture_depth_2d_array;
@group(0) @binding(B_CSM_SAMPLER) var s_shadow: sampler_comparison;
@group(0) @binding(B_CSM) var<uniform> csm: Csm;

// ENHANCEMENT E15: the lit headlights as spotlights (`gpu/spot.rs`). `params.x` is the lamp count, 0 = off
// (the shading is then the original's).
struct Spots {
    // xyz the lamp, w its intensity
    pos: array<vec4<f32>, MAX_SPOTS>,
    // xyz the way it shines
    dir: array<vec4<f32>, MAX_SPOTS>,
    // x the count, y cos of the outer half angle, z cos of the inner, w the range
    params: vec4<f32>,
    // x how many of the first lamps share the shadow map (0 = none), y the world size of a shadow texel at 1 m, z near, w far
    shadow: vec4<f32>,
    mat: mat4x4<f32>,
    // xyz the shadow's origin, w one texel in map coordinates
    origin: vec4<f32>,
};
@group(0) @binding(B_SPOT_MAP) var t_spot: texture_depth_2d;
@group(0) @binding(B_SPOTS) var<uniform> spots: Spots;

// ENHANCEMENT E23: the street lamps as point lights (`gpu/lamps.rs`). `params.x` is the count, 0 = off.
struct Lamps {
    // xyz the lamp, w its radius
    pos: array<vec4<f32>, MAX_LAMPS>,
    // rgb the colour, the brightness folded in; w (ENHANCEMENT E47) the lamp's shadow slot + 1, 0 = no shadow
    col: array<vec4<f32>, MAX_LAMPS>,
    // x the count, z the gain, w (ENHANCEMENT E47) one shadow texel in map coordinates
    params: vec4<f32>,
    // ENHANCEMENT E47: per shadow slot the lamp's frustum, and x the world size of a texel at 1 m, y near, z far, w the strength
    smat: array<mat4x4<f32>, LAMP_SHADOW_SLOTS>,
    sinfo: array<vec4<f32>, LAMP_SHADOW_SLOTS>,
};
@group(0) @binding(B_LAMPS) var<uniform> lamps: Lamps;
// ENHANCEMENT E47: the lamps' car shadow maps (`gpu/lamps.rs`), one layer per slot
@group(0) @binding(B_LAMP_SHADOW) var t_lamp_shadow: texture_depth_2d_array;

// ENHANCEMENT E43: the near cars' contact shadow (`gpu/contact.rs`). `params.x` is the car count, 0 = off (the shading is then the
// original's); `params.y` the strength (weaker where E9's sun shadow is drawn).
struct Contacts {
    // xyz the model origin on the road under the car, w the box's half width
    pos: array<vec4<f32>, MAX_CONTACTS>,
    // xyz the body's right axis, w the box's half length
    ax: array<vec4<f32>, MAX_CONTACTS>,
    // xyz the up axis, w the tyre radius
    ay: array<vec4<f32>, MAX_CONTACTS>,
    az: array<vec4<f32>, MAX_CONTACTS>,
    // the tyres' patch centres in the body's (x, z): (front left, front right) then (rear left, rear right) per car
    wheels: array<vec4<f32>, 2u * MAX_CONTACTS>,
    params: vec4<f32>,
};
@group(0) @binding(B_CONTACTS) var<uniform> contacts: Contacts;

// ENHANCEMENT E43: the contact shadow's shape (APPROXIMATION, enhancement content by eye, CONTACT-LAMPS: `gpu/contact.rs`):
// the footprint's corner rounding (a share of the smaller half size); the reach (m beyond the box's half sizes) of a car's
// occlusion and of its footprint's presence (the blob's fade); the up-facing ramp (`n` against the car's up axis); the clearance
// over which a jump thins the shadow out (m) and the least clearance; the body sides' skirt falloff (m); a tyre's patch: the
// clearance over which it lifts off (m), its ellipse (shares of the tyre radius across / along) and its two lobes (weights, falloffs);
// the presence's ramp (m outside the outline)
const CONTACT_CORNER: f32 = 0.4;
const CONTACT_REACH: f32 = 1.5;
const CONTACT_PRESENCE_REACH: f32 = 3.5;
const CONTACT_FACING: vec2<f32> = vec2<f32>(0.5, 0.85);
const CONTACT_GAP_SCALE: f32 = 0.6;
const CONTACT_MIN_GAP: f32 = 0.03;
const CONTACT_SKIRT_FALL: f32 = 0.35;
const CONTACT_TYRE_LIFT: f32 = 0.3;
const CONTACT_TYRE_ELLIPSE: vec2<f32> = vec2<f32>(0.55, 0.95);
const CONTACT_TYRE_LOBES: vec4<f32> = vec4<f32>(0.4, 3.0, 0.6, 9.0);
const CONTACT_PRESENCE_RAMP: vec2<f32> = vec2<f32>(0.15, 0.9);
// 1 / pi (the half-plane's occluded share)
const INV_PI: f32 = 0.31830988;

// ENHANCEMENT E43: the signed distance (m) to a rounded rectangle of half size `h` (negative inside)
fn contact_sdf(q: vec2<f32>, h: vec2<f32>) -> f32 {
    let rc = min(h.x, h.y) * CONTACT_CORNER;
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
        let reach = c.w + axs.w + CONTACT_REACH;
        if dot(d, d) > reach * reach {
            continue;
        }
        let facing = smoothstep(CONTACT_FACING.x, CONTACT_FACING.y, dot(n, ays.xyz));
        if facing <= 0.0 {
            continue;
        }
        let q = vec3<f32>(dot(d, axs.xyz), dot(d, ays.xyz), dot(d, contacts.az[i].xyz));
        let r = ays.w;
        // the clearance under the body's underside, and how a bigger one thins the shadow out (a jump)
        let gap = max(0.5 * r - q.y, CONTACT_MIN_GAP);
        let kg = 1.0 / (1.0 + (gap / CONTACT_GAP_SCALE) * (gap / CONTACT_GAP_SCALE));
        let sd = contact_sdf(q.xz, vec2<f32>(c.w, axs.w));
        let h = gap + contacts.params.z;
        let t = -sd / sqrt(sd * sd + h * h);
        let under = 0.5 + (asin(t) + t * sqrt(max(1.0 - t * t, 0.0))) * INV_PI;
        let body = 1.0 - (1.0 - under) * (1.0 - contacts.params.w * exp(-max(sd, 0.0) / CONTACT_SKIRT_FALL));
        // the tyres: a tight patch with a short halo; a tyre in the air (the road far below the car's plane) has neither
        let gw = max(-q.y, 0.0) / CONTACT_TYRE_LIFT;
        let kw = 1.0 / (1.0 + gw * gw);
        var wh = 0.0;
        for (var k = 0; k < 2; k = k + 1) {
            let w = contacts.wheels[2 * i + k];
            let e0 = (q.xz - w.xy) / vec2<f32>(CONTACT_TYRE_ELLIPSE.x * r, CONTACT_TYRE_ELLIPSE.y * r);
            let e1 = (q.xz - w.zw) / vec2<f32>(CONTACT_TYRE_ELLIPSE.x * r, CONTACT_TYRE_ELLIPSE.y * r);
            let a0 = dot(e0, e0);
            let a1 = dot(e1, e1);
            let lb = CONTACT_TYRE_LOBES;
            wh = max(wh, max(lb.x * exp(-a0 * lb.y) + lb.z * exp(-a0 * lb.w), lb.x * exp(-a1 * lb.y) + lb.z * exp(-a1 * lb.w)));
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
        let reach = c.w + axs.w + CONTACT_PRESENCE_REACH;
        if dot(d, d) > reach * reach {
            continue;
        }
        let s = contact_sdf(vec2<f32>(dot(d, axs.xyz), dot(d, contacts.az[i].xyz)), vec2<f32>(c.w, axs.w));
        p = max(p, 1.0 - smoothstep(CONTACT_PRESENCE_RAMP.x, CONTACT_PRESENCE_RAMP.y, s));
    }
    return p;
}

// (`BYTE`, a colour channel's top value in bytes: `wgsl::BYTE`)

fn night_byte(i: i32) -> f32 {
    let w = night_map[i / 16][(i / 4) % 4];
    return f32((w >> (8u * u32(i % 4))) & 0xFFu);
}

// 0x452910: the vertex taken into the headlight frame, then `row = z·1.28` (1…126), `col = x·3.62`
// (−62…62), `w = (nght[row][col + 64] − |y·32|) / 256`, all rounded as `fistp` does; 0x453279:
// a positive weight adds `round(128·w)` to each channel, clamped; either way the alpha is forced to
// 0xFF (0x45335D, 0x453377). The scales of 0x452910 / 0x453279 (as read above): the map row and column per metre of the
// headlight frame's z / x, the height's weight per metre, the byte weight's 1 / 256, the channel gain of `round(128·w)`
const HEAD_ROW_SCALE: f32 = 1.28;
const HEAD_COL_SCALE: f32 = 3.62;
const HEAD_Y_SCALE: f32 = 32.0;
const HEAD_W_SCALE: f32 = 0.00390625;
const HEAD_GAIN: f32 = 128.0;
fn headlight_w(world: vec3<f32>) -> f32 {
    let p = globals.headlight * vec4<f32>(world, 1.0);
    let row = i32(round(p.z * HEAD_ROW_SCALE));
    let col = i32(round(p.x * HEAD_COL_SCALE));
    if row <= 0 || row >= 127 || col <= -63 || col >= 63 {
        return 0.0;
    }
    let y = abs(round(p.y * HEAD_Y_SCALE));
    return max((night_byte(row * 128 + col + 64) - y) * HEAD_W_SCALE, 0.0);
}

fn headlit(world: vec3<f32>, c: vec4<f32>) -> vec4<f32> {
    let w = headlight_w(world);
    if w <= 0.0 {
        return vec4<f32>(c.rgb, 1.0);
    }
    // ENHANCEMENT E24: `headlight_on.z` = 1 - the projector's gain (0 = the original's, whole)
    let add = round(HEAD_GAIN * w * (1.0 - globals.headlight_on.z));
    return vec4<f32>(min(round(c.rgb * BYTE) + vec3<f32>(add), vec3<f32>(BYTE)) / BYTE, 1.0);
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
// The layer's numbers (as read above): `w = z + 16` ([0x5CEC50]), the clamp of `i` above 0.1 ([0x5B2604]) and the `w` that clamp
// and the behind-the-light arm write, and that arm's `u = x · 0.1 + 0.5`
const UVLIGHT_Z_SHIFT: f32 = 16.0;
const UVLIGHT_I_MAX: f32 = 0.1;
const UVLIGHT_W_HELD: f32 = 10.0;
const UVLIGHT_BEHIND_U: f32 = 0.1;
fn uvlight_vertex(world: vec3<f32>) -> vec3<f32> {
    let p = globals.headlight * vec4<f32>(world, 1.0);
    var w = p.z + UVLIGHT_Z_SHIFT;
    var i = 1.0 / w;
    if i > UVLIGHT_I_MAX {
        i = UVLIGHT_I_MAX;
        w = UVLIGHT_W_HELD;
    }
    var uv = vec2<f32>(p.x * UVLIGHT_BEHIND_U + 0.5, 0.0);
    if i >= 0.0 {
        uv = vec2<f32>(i * p.x + 0.5, i * p.y + 0.5);
    } else {
        w = UVLIGHT_W_HELD;
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

// `struct Object` (the items' uniforms) is `classes.rs` `OBJECT_WGSL`, put in front of this file (the shadow pass's too)
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
// Its scales (as read above): the row / column per metre ([0x5CEC6C]), and the height's weight per metre and the byte weight's
// 1 / 512 ([0x5CEC68], [0x5CEC70])
const BEACON_SCALE: f32 = 3.5;
const BEACON_Y_SCALE: f32 = 16.0;
const BEACON_W_SCALE: f32 = 0.001953125;
fn beaconlit(world: vec3<f32>, c: vec4<f32>) -> vec4<f32> {
    let p = globals.beacon * vec4<f32>(world, 1.0);
    let row = i32(round(p.z * BEACON_SCALE));
    let col = i32(round(p.x * BEACON_SCALE));
    if row <= -127 || row >= 127 || col <= -63 || col >= 63 {
        return vec4<f32>(c.rgb, 1.0);
    }
    let w = (night_byte(abs(row) * 128 + col + 64) - round(p.y * BEACON_Y_SCALE)) * BEACON_W_SCALE;
    if w <= 0.0 {
        return vec4<f32>(c.rgb, 1.0);
    }
    var tint = vec3<f32>(BYTE, 0.0, 0.0);
    if p.z < 0.0 {
        tint = vec3<f32>(0.0, 0.0, BYTE);
    }
    return vec4<f32>(min(round(c.rgb * BYTE) + round(tint * w), vec3<f32>(BYTE)) / BYTE, 1.0);
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
    return min(floor(object.lit_a.rgb * BYTE + object.lit_d.rgb * BYTE * d + vec3<f32>(0.5)), vec3<f32>(BYTE)) / BYTE;
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

// ENHANCEMENT E19: the fog's sun glow (APPROXIMATION, enhancement content by eye): the Henyey-Greenstein phase's asymmetry and its
// power (the phase's own 3 / 2), the warm sunlight's colour
const FOGSUN_G: f32 = 0.7;
const HG_POW: f32 = 1.5;
const FOGSUN_GLOW: vec3<f32> = vec3<f32>(1.0, 0.86, 0.62);

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
        let g = FOGSUN_G;
        let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * cs, HG_POW);
        let hg0 = (1.0 - g * g) / pow(1.0 + g * g, HG_POW);
        let hg1 = (1.0 - g * g) / pow((1.0 - g) * (1.0 - g), HG_POW);
        let ph = clamp((hg - hg0) / (hg1 - hg0), 0.0, 1.0);
        let glow = mix(fog.rgb, FOGSUN_GLOW, globals.fogsun.w * ph);
        return vec4<f32>(mix(glow, c.rgb, f), c.a);
    }
    //@endif
    return vec4<f32>(mix(fog.rgb, c.rgb, f), c.a);
}

// the guards: the least distance (m) a vector is divided by, the least of any other divisor (a depth, a weight, a slope), the
// least component of a ray (E49's box exit), the nudge that keeps a direction off zero, the least clip w of a reprojected point,
// the least step of a mip blend that reads two mips, and a distance past everything
const MIN_DIST: f32 = 0.001;
const MIN_DIV: f32 = 0.0001;
const MIN_RAY: f32 = 1e-5;
const RAY_NUDGE: f32 = 1e-4;
const MIN_CLIP_W: f32 = 1e-6;
const MIN_LOD_STEP: f32 = 0.001;
const FAR_AWAY: f32 = 1e9;
// a third (the multiplying pair's coverage: the mean of the darkening)
const THIRD: f32 = 0.3333333;

// SHADOW-REFACTOR: the numbers the shadow lookups share (E9's cascades, E15's headlight map, E47's lamp maps)
// a bilinear comparison kernel's four corner taps agree (all lit or all in shadow) below / above these sums
const TAPS_NONE: f32 = 0.001;
const TAPS_ALL: f32 = 3.999;
// a 3 x 3 kernel's taps
const GRID_TAPS: f32 = 9.0;
// E9: the receiver's push along its normal (texels: the base, and more as the surface turns from the light) and the depth held a
// texel's worth below the stored one
const CSM_OFFSET_BASE: f32 = 0.6;
const CSM_OFFSET_TURN: f32 = 1.6;
const CSM_DEPTH_BIAS: f32 = 1.2;
// (a Vogel disc's step, the golden angle, and a whole turn: `GOLDEN_ANGLE` / `TAU`, `wgsl::ANGLES`)
// E50: the moon's disc taps, and where its rim taps stand (1 / sqrt 2 of the radius on each axis: on the rim)
const MOON_TAPS: i32 = 16;
const MOON_RIM: f32 = 0.7071;
// ENH-FE3D / ENH-GARAGE: the focus shadow's disc taps, its core's, the core's extra turn, and the noise's scale on the world point
const FOCUS_TAPS: i32 = 24;
const FOCUS_CORE_TAPS: i32 = 12;
const FOCUS_CORE_TURN: f32 = 1.3;
const FOCUS_NOISE_XZ: f32 = 211.0;
const FOCUS_NOISE_Y: f32 = 97.0;
// E15 / E47: the receiver's push along its normal in texels at its distance (1 + this x the turn from the light), the depth bias in
// texels on top of the light's own metres, where the frustum's border starts fading to lit
const PERSP_OFFSET_TURN: f32 = 2.0;
const PERSP_BIAS_TEXELS: f32 = 1.5;
const PERSP_EDGE_FADE: f32 = 0.85;
const SPOT_BIAS_M: f32 = 0.05;
const LAMP_BIAS_M: f32 = 0.04;

// E9's track receiver: how much a face turned from the light still takes (a floor, full from this `n.l`), the "already baked dark"
// gate's luma ramp, E50's turned-round ramp (a lamp's pool is the lamp's light, not the moon's)
const FACING_FLOOR: f32 = 0.35;
const FACING_FULL: f32 = 0.25;
const BAKED_DARK_LO: f32 = 0.3;
const BAKED_DARK_HI: f32 = 0.7;
const MOON_LIT_LO: f32 = 0.25;
const MOON_LIT_HI: f32 = 0.6;
// E9: the car's light is the sun's when its direction is within this cosine of the cascades' light (`csm.orig`)
const SUN_SAME_COS: f32 = 0.999;
// (the luma of a colour, Rec. 601: `LUMA`, `classes.rs` `SH_LUMA`)

// SHADOW-REFACTOR: a kernel's corner tap `k` (0..3): (-1, -1), (1, -1), (-1, 1), (1, 1)
fn tap_corner(k: i32) -> vec2<f32> {
    return vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0);
}

// whether a kernel's four corner taps (their sum) agree: all lit or all in shadow (nearly every pixel; the other taps would too)
fn taps_agree(sum4: f32) -> bool {
    return sum4 < TAPS_NONE || sum4 > TAPS_ALL;
}

// the sum of the four corner taps `st` apart (map units) of layer `layer` at `uv` against `depth` (each a hardware-bilinear compare)
fn pcf_corners(t: texture_depth_2d_array, layer: i32, uv: vec2<f32>, st: f32, depth: f32) -> f32 {
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        sum += textureSampleCompareLevel(t, s_shadow, uv + tap_corner(k) * st, layer, depth);
    }
    return sum;
}

// a 3 x 3 kernel's other five taps (the centre, then the cross) added to `sum`, its corners' (the four corners' 2 x 2 blocks cover
// the whole grid)
fn pcf_cross(sum: f32, t: texture_depth_2d_array, layer: i32, uv: vec2<f32>, st: f32, depth: f32) -> f32 {
    var s = sum;
    s += textureSampleCompareLevel(t, s_shadow, uv, layer, depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(-st, 0.0), layer, depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(st, 0.0), layer, depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(0.0, -st), layer, depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(0.0, st), layer, depth);
    return s;
}

// `pcf_corners` / `pcf_cross` of a plain map (E15's)
fn pcf_corners_2d(t: texture_depth_2d, uv: vec2<f32>, st: f32, depth: f32) -> f32 {
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        sum += textureSampleCompareLevel(t, s_shadow, uv + tap_corner(k) * st, depth);
    }
    return sum;
}

fn pcf_cross_2d(sum: f32, t: texture_depth_2d, uv: vec2<f32>, st: f32, depth: f32) -> f32 {
    var s = sum;
    s += textureSampleCompareLevel(t, s_shadow, uv, depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(-st, 0.0), depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(st, 0.0), depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(0.0, -st), depth);
    s += textureSampleCompareLevel(t, s_shadow, uv + vec2<f32>(0.0, st), depth);
    return s;
}

// a `count`-tap Vogel disc of cascade `layer` at `uv`, `texels` texels (of `step` map units) across the radius, turned by `turn` and
// `turn2`: the sum of its hardware-bilinear compares
fn pcf_disc(layer: i32, uv: vec2<f32>, depth: f32, count: i32, texels: f32, step: f32, turn: f32, turn2: f32) -> f32 {
    var sum = 0.0;
    for (var i = 0; i < count; i++) {
        let fi = f32(i);
        let rr = sqrt((fi + 0.5) / f32(count)) * texels * step;
        let a = fi * GOLDEN_ANGLE + turn + turn2;
        sum += textureSampleCompareLevel(t_shadow, s_shadow, uv + vec2<f32>(cos(a), sin(a)) * rr, layer, depth);
    }
    return sum;
}

// ENHANCEMENT E9 (SHADOW-REFACTOR): whether this frame's cascades are of mode `m` (`SHADOW_*`, `gpu/csm/mode.rs` `ShadowMode`)
fn csm_mode_is(m: f32) -> bool {
    return abs(csm.misc.y - m) < 0.5;
}

// ENHANCEMENT E9: where `world` falls in cascade `c`: the point pushed along the normal by a texel or so (more where the surface
// turns from the light); xy the map's uv, z the depth held a texel's worth below the stored one, w 1 inside the map (0: outside,
// lit)
fn csm_coord(c: i32, world: vec3<f32>, n: vec3<f32>, ndl: f32) -> vec4<f32> {
    let info = csm.info[c];
    let off = n * (info.x * (CSM_OFFSET_BASE + CSM_OFFSET_TURN * (1.0 - clamp(ndl, 0.0, 1.0))));
    let p = csm.mats[c] * vec4<f32>(world + off, 1.0);
    let uv = ndc_uv(p.xy);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || p.z > 1.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(uv, p.z - info.x * CSM_DEPTH_BIAS / info.y, 1.0);
}

// ENHANCEMENT E9: the view depth of a world point, the cascade it picks (the last one whose end it has not passed) and how far the
// last cascade has faded out there (0 .. 1 at its end)
fn csm_view_depth(world: vec3<f32>) -> f32 {
    return -(globals.view * vec4<f32>(world, 1.0)).z;
}

fn csm_cascade(vd: f32) -> i32 {
    var c = 0;
    for (var i = 0; i < i32(csm.dir.w) - 1; i++) {
        if vd >= csm.info[i].z {
            c = i + 1;
        }
    }
    return c;
}

fn csm_far_fade(vd: f32) -> f32 {
    let last = csm.info[i32(csm.dir.w) - 1].z;
    return clamp((vd - csm.params.y) / max(last - csm.params.y, MIN_DIST), 0.0, 1.0);
}

// ENHANCEMENT E9: how much of the sun reaches `world` through cascade `c` (0 shadowed .. 1 lit): `csm_coord`, then a 3 x 3 grid of
// hardware-bilinear compares softens the edge over about four texels (the mode's own kernel for the moon, the front end).
fn csm_tap(c: i32, world: vec3<f32>, n: vec3<f32>, ndl: f32) -> f32 {
    let q = csm_coord(c, world, n, ndl);
    if q.w < 0.5 {
        return 1.0;
    }
    let uv = q.xy;
    let depth = q.z;
    let step = 1.0 / f32(textureDimensions(t_shadow).x);
    //@if MOON
    // ENHANCEMENT E50: the moon's soft edge: a 16-tap Vogel disc of hardware-bilinear compares, `csm.moon.z` texels across the radius.
    // PERF: four taps on the disc's rim first; when they agree (all lit or all in shadow, nearly every pixel) the disc would too
    if csm_mode_is(SHADOW_MOON) {
        let rim = pcf_corners(t_shadow, c, uv, csm.moon.z * step * MOON_RIM, depth);
        if taps_agree(rim) {
            return rim * 0.25;
        }
        return pcf_disc(c, uv, depth, MOON_TAPS, csm.moon.z, step, 0.0, 0.0) / f32(MOON_TAPS);
    }
    //@endif
    //@if FOCUS
    // ENHANCEMENT ENH-FE3D: the front-end car's soft shadow: a Vogel disc, turned per pixel by interleaved gradient noise (the edge
    // dithers instead of banding), `extra.x` texels across the radius
    if csm_mode_is(SHADOW_FOCUS) {
        let rot = TAU * ign(world.xz * FOCUS_NOISE_XZ + world.y * FOCUS_NOISE_Y);
        var soft = pcf_disc(c, uv, depth, FOCUS_TAPS, csm.extra.x, step, rot, 0.0) / f32(FOCUS_TAPS);
        // ENHANCEMENT ENH-GARAGE: a tight core (`extra.y` texels, another turn) carries the contact: the shadow is the darker of
        // the penumbra (its darkness scaled by `extra.z`) and the whole-strength core
        if csm.extra.y > 0.0 {
            let core = pcf_disc(c, uv, depth, FOCUS_CORE_TAPS, csm.extra.y, step, rot, FOCUS_CORE_TURN);
            soft = min(1.0 - (1.0 - soft) * csm.extra.z, core / f32(FOCUS_CORE_TAPS));
        }
        return soft;
    }
    //@endif
    // PERF: the four corner taps first; when they agree the other five would too
    let corners = pcf_corners(t_shadow, c, uv, step, depth);
    // the far cascades' texels are coarse and their shadows far: the four corners are the soft edge there
    if taps_agree(corners) || c >= 1 {
        return corners * 0.25;
    }
    return pcf_cross(corners, t_shadow, c, uv, step, depth) / GRID_TAPS;
}

// ENHANCEMENT E9 (ENH-TUNPOOL): how covered a world point is, 0..1: inside a covered road slice's box (a tunnel's or a gallery's
// interior, from the track's own overhead flags, `CoverBox`), whatever the camera is doing. The sun does not reach there and the
// baked colour has its own light in it (the lamps' pools), so the sun's shadow must not darken it a second time.
fn cover_at(world: vec3<f32>) -> f32 {
    let count = i32(csm.misc.x);
    var w = 0.0;
    // PERF (FOREST-E9PERF, picture identical): a point outside the world box round all the boxes (`cover_lo` / `cover_hi`) is in
    // none of them; the loop below ran for every fragment of every receiver (under the forest's canopy, layers of foliage cards)
    if any(world < csm.cover_lo.xyz) || any(world > csm.cover_hi.xyz) {
        return 0.0;
    }
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
    if csm.misc.x > 0.5 {
        share = share * (1.0 - cover_at(world));
    }
    return share;
}

// ENHANCEMENT E9 (SHADOW-REAL): the sun's shadow at a point in its two parts: x the cascades' visibility (0 shadowed .. 1 lit),
// y the share of the sun's light it is drawn for (`csm_share`, faded out toward the last cascade's end). The cascade is chosen by
// the point's view depth, blended into the next over the last eighth of its range. A covered point reads no map at all.
fn csm_split(world: vec3<f32>, n: vec3<f32>) -> vec2<f32> {
    let count = i32(csm.dir.w);
    let vd = csm_view_depth(world);
    // (PERF, FOREST-E9PERF: beyond the cascades first, without the covered boxes' loop)
    if vd >= csm.info[count - 1].z {
        return vec2<f32>(1.0, 0.0);
    }
    let share = csm_share(world);
    if share <= 0.0 {
        return vec2<f32>(1.0, 0.0);
    }
    let ndl = dot(n, -csm.dir.xyz);
    let c = csm_cascade(vd);
    var v = csm_tap(c, world, n, ndl);
    if c + 1 < count {
        let end = csm.info[c].z;
        let t = clamp((vd - end * CASCADE_BLEND_START) / (end * CASCADE_BLEND), 0.0, 1.0);
        if t > 0.0 {
            v = mix(v, csm_tap(c + 1, world, n, ndl), t);
        }
    }
    return vec2<f32>(v, share * (1.0 - csm_far_fade(vd)));
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
    let vd = csm_view_depth(world);
    let c = csm_cascade(vd);
    if c >= i32(csm.carsh.x) || vd >= csm.info[i32(csm.dir.w) - 1].z {
        return 1.0;
    }
    let q = csm_coord(c, world, n, dot(n, -csm.dir.xyz));
    if q.w < 0.5 {
        return 1.0;
    }
    let step = 1.0 / f32(textureDimensions(t_shadow).x);
    let sum = pcf_corners(t_shadow, i32(csm.carsh.y) + c, q.xy, step, q.z);
    // ENHANCEMENT E51 (CARSH-FADE): only where a car's shadow is found (most of the road reads no car: the cheap taps above decide)
    let v = sum * 0.25;
    if csm.carsh.z > 0.5 && sum < TAPS_ALL {
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
    var best = FAR_AWAY;
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

// ENHANCEMENT E15 / E47 (SHADOW-REFACTOR: one core for both): where the clip-space point `p` (the receiver pushed along its normal)
// falls in a perspective shadow map of planes `near` / `far`: xy the uv, z the depth held `bias_m` metres plus `PERSP_BIAS_TEXELS`
// texels (`texel` m at its distance) below the stored one (turned into the stored depth's units), w the frustum border's measure
// (max |ndc| of x, y; -1 outside the frustum: lit)
fn persp_coord(p: vec4<f32>, near: f32, far: f32, texel: f32, bias_m: f32) -> vec4<f32> {
    if p.w <= near {
        return vec4<f32>(-1.0);
    }
    let ndc = p.xyz / p.w;
    let uv = ndc_uv(ndc.xy);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return vec4<f32>(-1.0);
    }
    let bias = (bias_m + PERSP_BIAS_TEXELS * texel) * far * near / ((far - near) * p.w * p.w);
    return vec4<f32>(uv, ndc.z - bias, max(abs(ndc.x), abs(ndc.y)));
}

// a perspective map's visibility `sum` faded to lit over the frustum's border (`edge`, `persp_coord`'s w), so the shadowed disc has no
// hard rim
fn persp_visibility(sum: f32, edge: f32) -> f32 {
    return mix(sum, 1.0, smoothstep(PERSP_EDGE_FADE, 1.0, edge));
}

// ENHANCEMENT E15: how much of the player's lamp pair reaches `world` (0 shadowed .. 1 lit): the shadow frustum's perspective map
// (`persp_coord`), a 3 x 3 grid of hardware-bilinear compares with the four-corner early-out.
fn spot_visibility(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let o = spots.origin.xyz;
    let dist = length(world - o);
    let texel = spots.shadow.y * dist;
    let ndl = clamp(dot(n, normalize(o - world)), 0.0, 1.0);
    let q = persp_coord(spots.mat * vec4<f32>(world + n * (texel * (1.0 + PERSP_OFFSET_TURN * (1.0 - ndl))), 1.0), spots.shadow.z, spots.shadow.w, texel, SPOT_BIAS_M);
    if q.w < 0.0 {
        return 1.0;
    }
    let st = spots.origin.w;
    var sum = pcf_corners_2d(t_spot, q.xy, st, q.z);
    if taps_agree(sum) {
        sum = sum * 0.25;
    } else {
        sum = pcf_cross_2d(sum, t_spot, q.xy, st, q.z) / GRID_TAPS;
    }
    return persp_visibility(sum, q.w);
}

// ENHANCEMENT E15: the irradiance of the lit lamps at `world` for a surface with normal `n`: per lamp a smooth
// cone, a range falloff `(1 - d / range)^2 / (1 + (d / 25)^2)`, `n.l` raised to the floor `floor_` (the track
// has no lighting model, see `gpu/spot.rs`), a warm white; the first two lamps take the shadow map.
// ENHANCEMENT E23: the street lamps' irradiance at a point: a smooth falloff to the radius, a downward
// cone (full below the lamp, gone toward the horizon: `l.y` is the cosine of the angle off straight
// down seen from the lamp), `n.l` with a floor as for the spots.
fn lamp_light(world: vec3<f32>, n: vec3<f32>, floor_: f32) -> vec4<f32> {
    var sum = vec3<f32>(0.0);
    // MONACO-TUNNEL: the share of the baked colour the baked lamps' shadows take here (`.w` of the result)
    var loss = 0.0;
    let count = i32(lamps.params.x);
    for (var i = 0; i < count; i = i + 1) {
        // (a negative radius: a baked lamp, MONACO-TUNNEL)
        let radius = abs(lamps.pos[i].w);
        let lv = lamps.pos[i].xyz - world;
        let dist = length(lv);
        if dist >= radius {
            continue;
        }
        let l = lv / max(dist, MIN_DIST);
        let cone = smoothstep(LAMP_CONE_START, LAMP_CONE_END, l.y);
        let r = 1.0 - dist / radius;
        let fall = r * r * (3.0 - 2.0 * r);
        var w = cone * fall * mix(floor_, 1.0, clamp(dot(n, l), 0.0, 1.0));
        // ENHANCEMENT E47: a lamp with a shadow slot is shadowed by the cars (only its own term)
        //@if LSHAD
        // (PERF, MONACO-TUNNEL round 2: where the lamp reaches with under 3 % of its light (the outer band of its reach) its shadow cannot
        // show; the lookup is skipped there)
        if lamps.col[i].w > 0.5 && w > 0.0 && cone * fall > LAMP_SHADOW_MIN_LIGHT {
            let vis = lamp_visibility(i32(lamps.col[i].w) - 1, lamps.pos[i].xyz, world, n);
            w = w * vis;
            // MONACO-TUNNEL: a lamp whose light the bake holds takes its share of the baked colour too
            if lamps.pos[i].w < 0.0 {
                loss += lamps.params.y * cone * fall * (1.0 - vis);
            }
        }
        //@endif
        sum += lamps.col[i].rgb * w;
    }
    return vec4<f32>(sum * lamps.params.z, min(loss, BAKED_SHADOW_MAX));
}

// ENHANCEMENT E47 (PERF, MONACO-TUNNEL round 2): the least share of a lamp's light at a point for its shadow to be looked up there
const LAMP_SHADOW_MIN_LIGHT: f32 = 0.03;

// ENHANCEMENT E47: how much of lamp slot `k`'s light (the lamp at `lamp`) reaches `world` (0 shadowed .. 1 lit), mixed toward 1 by the
// slot's strength: the slot's perspective map (`persp_coord`), E15's 3 x 3 grid with its four-tap early-out. Outside the frustum
// nothing is shadowed (it holds every caster's cone).
fn lamp_visibility(k: i32, lamp: vec3<f32>, world: vec3<f32>, n: vec3<f32>) -> f32 {
    let info = lamps.sinfo[k];
    let dist = length(world - lamp);
    let texel = info.x * dist;
    let ndl = clamp(dot(n, (lamp - world) / max(dist, MIN_DIST)), 0.0, 1.0);
    let q = persp_coord(lamps.smat[k] * vec4<f32>(world + n * (texel * (1.0 + PERSP_OFFSET_TURN * (1.0 - ndl))), 1.0), info.y, info.z, texel, LAMP_BIAS_M);
    if q.w < 0.0 {
        return 1.0;
    }
    let st = lamps.params.w;
    var sum = pcf_corners(t_lamp_shadow, k, q.xy, st, q.z);
    if taps_agree(sum) {
        sum = sum * 0.25;
    } else {
        sum = pcf_cross(sum, t_lamp_shadow, k, q.xy, st, q.z) / GRID_TAPS;
    }
    return mix(1.0, persp_visibility(sum, q.w), info.w);
}

// ENHANCEMENT E15 (MONACO-TUNNEL): the headlight's foreground taper, `spot_light`. APPROXIMATION (by the stills: the patch over the
// bonnet in monaco1's tunnel, `RE/notes/ENHANCEMENTS.md` E15 MONACO-TUNNEL)
const SPOT_NEAR_FLOOR: f32 = 0.15;
const SPOT_NEAR_END: f32 = 20.0;
// ENHANCEMENT E15 (APPROXIMATION, enhancement content by eye): the least distance to a lamp (m), the falloff's `(d / 25)^2` (m), the
// lamps' warm white; the light's gain on a receiver; the `n.l` floor of a track item and of a car (the track has no lighting model,
// `gpu/spot.rs`); ENHANCEMENT E23: a car's share of the street lamps' light
const SPOT_MIN_DIST: f32 = 0.3;
const SPOT_FALL_DIST: f32 = 25.0;
const SPOT_TINT: vec3<f32> = vec3<f32>(1.0, 0.9, 0.7);
const SPOT_GAIN: f32 = 1.25;
const FLOOR_TRACK: f32 = 0.45;
const FLOOR_CAR: f32 = 0.1;
const LAMP_CAR_GAIN: f32 = 0.6;

fn spot_light(world: vec3<f32>, n: vec3<f32>, floor_: f32) -> vec3<f32> {
    // PERF: the shadow lookup happens once, and only for a fragment one of the shadowed lamps reaches (below)
    var vis = -1.0;
    var e = vec3<f32>(0.0);
    let count = i32(spots.params.x);
    for (var i = 0; i < count; i++) {
        let lv = spots.pos[i].xyz - world;
        let dist = max(length(lv), SPOT_MIN_DIST);
        let l = lv / dist;
        let cone = smoothstep(spots.params.y, spots.params.z, dot(-l, spots.dir[i].xyz));
        if cone <= 0.0 {
            continue;
        }
        let r = clamp(1.0 - dist / spots.params.w, 0.0, 1.0);
        if r <= 0.0 {
            continue;
        }
        let d25 = dist / SPOT_FALL_DIST;
        // MONACO-TUNNEL: the foreground, seen just over the bonnet, takes a part of the light (a dipped beam puts its hot spot
        // near the cut-off far ahead and keeps the road under the car's nose dim; the plain falloff made the first metres the
        // brightest, a clipped patch on the lane markings that hid the road): SPOT_NEAR_FLOOR at the lamp, whole from SPOT_NEAR_END
        let fore = mix(SPOT_NEAR_FLOOR, 1.0, smoothstep(0.0, SPOT_NEAR_END, dist));
        var s = spots.pos[i].w * cone * r * r / (1.0 + d25 * d25) * fore * mix(floor_, 1.0, clamp(dot(n, l), 0.0, 1.0));
        if f32(i) < spots.shadow.x {
            if vis < 0.0 {
                vis = spot_visibility(world, n);
            }
            s = s * vis;
        }
        e += s * SPOT_TINT;
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

// ENHANCEMENT E9 (FOREST-E9PERF, picture identical): the alpha test decided before `shade` instead of after it. A draw whose final
// alpha is `shade`'s base alpha (`alpha_is_base`: alpha-tested, and none of the classes that change it later: glass, water, the wet
// road and its skids, a ground decal) fails or passes the test on that base alpha alone (`base_a255`: the same texel and products as
// `shade`'s), so a texel the test throws away is discarded before the sun's shadow, the lights and the rest are worked out for it.
// Only in the entries that discard after `shade` anyway (`fs_opaque`, `fs_main` and their temporal twins: the blended foliage, the
// reflection faces' cut-outs, a target without MSAA); under the forest's canopy their cut-out texels paid for the whole receiver
// (E9's cascades and covered boxes) only to be thrown away.
//@if CSM
fn alpha_is_base() -> bool {
    return object.shadow_on.w > 0.0 && object.refl.x < REFL_GLASS - 0.5 && object.lit_d.w < RECV_DECAL - 0.5;
}

fn base_a255(in: VsOut) -> f32 {
    let tex = textureSampleBias(t_diffuse, s_diffuse, in.uv, object.shadow_on.z);
    let base = tex * in.color;
    return base.a * object.light.a * BYTE;
}
//@endif

// The alpha-tested `shade` of the entries that discard after it (`fs_opaque`, `fs_main` and their temporal twins; `object` is
// set): the early test on the base alpha (ENHANCEMENT E9, FOREST-E9PERF: `alpha_is_base`; the sample is taken before the
// condition, in uniform control flow), `shade`, then the test on the final alpha.
fn shade_tested(in: VsOut) -> vec4<f32> {
    //@if CSM
    let a0 = base_a255(in);
    if alpha_is_base() && a0 < object.shadow_on.w {
        discard;
    }
    //@endif
    let c = shade(in, false);
    if object.shadow_on.w > 0.0 && c.a * BYTE < object.shadow_on.w {
        discard;
    }
    return c;
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade_tested(in);
    return held(vec4<f32>(fogged(in, c).rgb, 1.0));
}

// ENHANCEMENT E1: the opaque pipeline for an alpha-TESTED draw (foliage, fences) on a multisampled
// target, with alpha-to-coverage: the test's threshold (`shadow_on.w`, reference + ½ in bytes) stays
// where the original has it, but the edge is a sharpened coverage ramp (one pixel wide, by the
// alpha's screen-space slope) instead of a discard, so the cut-out edge is antialiased.
@fragment
fn fs_opaque_a2c(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    // (FOREST-E9PERF: no early discard here: a `discard` in this entry, which has none, cost more than it saved — measured 0.3-0.4 ms
    // slower in the open and under the canopy alike; the GPU's early depth test is worth more than the cut-out texels' shading)
    let c = shade(in, false);
    let a255 = c.a * BYTE;
    let w = max(fwidth(a255), MIN_DIV);
    var cov = 1.0;
    if object.shadow_on.w > 0.0 {
        cov = clamp((a255 - object.shadow_on.w) / w + 0.5, 0.0, 1.0);
    }
    return held(vec4<f32>(fogged(in, c).rgb, cov));
}

// ENHANCEMENT E1 (FOLIAGE-OVERDRAW): an alpha-tested opaque draw whose test can never fail (every texel of its texture alpha 255, every
// vertex alpha 1, the object's own opacity 1, the alpha is `shade`'s base alpha: `Core::solid_draws`). `fs_opaque_a2c` would give it
// full coverage everywhere, `fs_opaque` would never discard: the same colour without either, so the GPU keeps its hidden-surface
// removal (a coverage output or a `discard` turns it off for the draw, and every covered layer of the scene was shaded).
@fragment
fn fs_opaque_solid(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade(in, false);
    return held(vec4<f32>(fogged(in, c).rgb, 1.0));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    object = objects[in.obj];
    let c = shade_tested(in);
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
    if p.w <= MIN_CLIP_W {
        return vec2<f32>(0.0);
    }
    let q = p.xy / p.w;
    return vec2<f32>((q.x - ndc.x) * 0.5 * size.x, (ndc.y - q.y) * 0.5 * size.y);
}

// how much of the pixel the blended draw covers: its alpha, and for the multiplying pair (3) how much it darkens
fn coverage(c: vec4<f32>) -> f32 {
    if PAIR == 3u {
        return clamp(1.0 - dot(c.rgb, vec3<f32>(THIRD)), 0.0, 1.0);
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

// the output of a blended entry on the temporal set: the colour `col` and the masks of the item's role (`object.soft.w`)
fn temporal_out(in: VsOut, col: vec4<f32>) -> FsOutT {
    var o: FsOutT;
    o.c = col;
    let kind = object.soft.w;
    o.mv = vec4<f32>(velocity(in), 0.0, select(0.0, 1.0, kind > ROLE_SKY - 0.5));
    let fx = kind > ROLE_OPAQUE + 0.5 && kind < ROLE_EFFECT + 0.5;
    o.re = vec4<f32>(select(0.0, coverage(col), fx), 0.0, 0.0, 0.0);
    o.tc = vec4<f32>(select(0.0, composition(col.a), fx), 0.0, 0.0, 0.0);
    return o;
}

@fragment
fn fs_opaque_t(in: VsOut) -> FsOutT {
    object = objects[in.obj];
    let c = shade_tested(in);
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
    let c = shade_tested(in);
    return temporal_out(in, held(soften(in, particle_fx(in, fogged(in, c)))));
}

// ENHANCEMENT ENH-TEMPORAL: `fs_layers` on the temporal set (the blended pair 0 draws only; the layers' coverage is `col.a`)
@fragment
fn fs_layers_t(in: VsOut) -> FsOutT {
    object = objects[in.obj];
    return temporal_out(in, held(layers_color(in)));
}

// ENHANCEMENT E18: particles in the float scene, on top of the original's colours. A spark (`hdr.y` > 0) goes far
// above white (the bloom, E10, then glows). A lit particle (`hdr.z` 2 = by its centre, 3 = per fragment) takes the
// sun's side: unshadowed it is 1.0 (the original) up to 1.3 where the view looks toward the sun (a forward-scatter
// lobe, `dot(view, to_sun)^6`), and in the sun's shadow (E9's map, one cheap sample) 0.72 plus a little of the lobe.
// Smoke never goes past 1.3 x its own colour: it is not turned into a light.
// (the lobe's power; a particle's light in the shadow and the lobe's share there; the lobe's share in the sun)
const PART_LOBE_POWER: f32 = 6.0;
const PART_SHADE: f32 = 0.72;
const PART_SHADE_LOBE: f32 = 0.1;
const PART_SUN_LOBE: f32 = 0.3;
fn particle_fx(in: VsOut, c: vec4<f32>) -> vec4<f32> {
    var o = c;
    //@if PFX
    if object.hdr.y > 0.0 {
        o = vec4<f32>(c.rgb * object.hdr.y, c.a);
    } else if object.hdr.z > PART_LIT - 0.5 && globals.fx_sun.w > 0.5 {
        let to_sun = globals.fx_sun.xyz;
        let v = normalize(in.world - globals.cam_pos.xyz);
        let lobe = pow(max(dot(v, to_sun), 0.0), PART_LOBE_POWER);
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
        let lit = mix(PART_SHADE + PART_SHADE_LOBE * lobe, 1.0 + PART_SUN_LOBE * lobe, vis);
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
    let scene_z = linear_z(d, near, far);
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
    var e = probe_sh[0].xyz * SH_Y00;
    e += probe_sh[1].xyz * (SH_Y1 * n.y) + probe_sh[2].xyz * (SH_Y1 * n.z) + probe_sh[3].xyz * (SH_Y1 * n.x);
    e += probe_sh[4].xyz * (SH_Y2 * n.x * n.y) + probe_sh[5].xyz * (SH_Y2 * n.y * n.z);
    e += probe_sh[6].xyz * (SH_Y20 * (3.0 * n.z * n.z - 1.0)) + probe_sh[7].xyz * (SH_Y2 * n.x * n.z);
    e += probe_sh[8].xyz * (SH_Y22 * (n.x * n.x - n.y * n.y));
    return max(e, vec3<f32>(0.0));
}

// ENHANCEMENT E8: where direction `d` (world space) falls in face `face` of the cube: read through the same basis the face was
// drawn with (`gpu::REFL_FACES`: look along `fwd`, screen right = `up × fwd` in the game's left-handed world, the clip x being
// flipped, `Camera::view_projection`), with `1 / tan(half fov)` = `globals.refl.y` (the face is drawn a little wider than 90
// degrees; ENHANCEMENT E49: `d` may lean a little past the face's middle 90 degrees)
fn face_uv(d: vec3<f32>, face: i32) -> vec2<f32> {
    let fwd = face_fwd(face);
    let up = face_up(face);
    let right = cross(up, fwd);
    let z = max(dot(d, fwd), MIN_DIV);
    return vec2<f32>(0.5 + 0.5 * dot(d, right) / z * globals.refl.y, 0.5 - 0.5 * dot(d, up) / z * globals.refl.y);
}

// ENHANCEMENT E8: the environment at direction `d`, in the face its major axis picks (`face_of`)
fn refl_sample(d: vec3<f32>) -> vec3<f32> {
    let face = face_of(d);
    return textureSampleLevel(t_refl, s_proj, face_uv(d, face), face, 0.0).rgb;
}

// ENHANCEMENT ENH-COAT: the mip of the cube that holds a GGX lobe of width `alpha` (the mips' alphas: 0, 0.03, 0.08, 0.2,
// 0.45 = `coat.rs` `MIP_ALPHA`), a linear blend between the two nearest, read trilinearly by hand (the sampler's own mip
// filter may be `nearest`)
fn coat_lod(alpha: f32) -> f32 {
    if alpha <= COAT_MIP_ALPHA[1] {
        return alpha / COAT_MIP_SPAN[0];
    }
    if alpha <= COAT_MIP_ALPHA[2] {
        return 1.0 + (alpha - COAT_MIP_ALPHA[1]) / COAT_MIP_SPAN[1];
    }
    if alpha <= COAT_MIP_ALPHA[3] {
        return 2.0 + (alpha - COAT_MIP_ALPHA[2]) / COAT_MIP_SPAN[2];
    }
    return min(3.0 + (alpha - COAT_MIP_ALPHA[3]) / COAT_MIP_SPAN[3], COAT_LAST_MIP);
}

// ENHANCEMENT ENH-COAT: the environment at direction `d` at mip `lod` of the cube (mip m holds the radiance convolved with a GGX lobe
// of alpha `COAT_MIP_ALPHA[m]`, `coat.rs`): the two integer mips around it blended (the face and its point picked once)
fn refl_sample_lod(d: vec3<f32>, lod: f32) -> vec3<f32> {
    let face = face_of(d);
    let uv = face_uv(d, face);
    let l0 = floor(lod);
    let a = textureSampleLevel(t_refl, s_proj, uv, face, l0).rgb;
    if lod - l0 < MIN_LOD_STEP {
        return a;
    }
    return mix(a, textureSampleLevel(t_refl, s_proj, uv, face, l0 + 1.0).rgb, lod - l0);
}

//@if NEAR

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
            let rs = select(rl, vec3<f32>(MIN_RAY), abs(rl) < vec3<f32>(MIN_RAY));
            let tf = max((lo - pl) / rs, (hi - pl) / rs);
            hit = p + r * max(min(tf.x, min(tf.y, tf.z)), 0.0);
        } else {
            w = 0.0;
        }
    }
    let face = face_of(mix(r, normalize(hit - globals.near_eye[6].xyz + r * RAY_NUDGE), w));
    let d = mix(r, normalize(hit - globals.near_eye[face].xyz + r * RAY_NUDGE), w);
    let uv = face_uv(d, face);
    var layer = face;
    if own && globals.near.y > 0.5 {
        layer = face + i32(NEAR_OWN_LAYER);
    }
    let l0 = floor(lod);
    let a = textureSampleLevel(t_refl, s_proj, uv, layer, l0);
    if lod - l0 < MIN_LOD_STEP {
        return a;
    }
    return mix(a, textureSampleLevel(t_refl, s_proj, uv, layer, l0 + 1.0), lod - l0);
}
//@endif

// ENHANCEMENT ENH-COAT: Karis' analytic fit of the split-sum environment BRDF (UE4 mobile, `EnvBRDFApprox`): the scale and
// the bias a specular colour F0 gets, `F0 * x + y`, for a perceptual roughness and `n.v`
const ENV_BRDF_C0: vec4<f32> = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
const ENV_BRDF_C1: vec4<f32> = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
const ENV_BRDF_EXP2: f32 = 9.28;
const ENV_BRDF_AB: vec2<f32> = vec2<f32>(-1.04, 1.04);
fn env_brdf(rough: f32, nv: f32) -> vec2<f32> {
    let c0 = ENV_BRDF_C0;
    let c1 = ENV_BRDF_C1;
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-ENV_BRDF_EXP2 * nv)) * r.x + r.y;
    return ENV_BRDF_AB * a004 + r.zw;
}

// ENHANCEMENT ENH-COAT: blending in linear light (the game's values are display values, gamma about 2.2): a 4 % reflection
// of the sky over a dark paint is a few percent of the sky's *light*, which is as much as the paint's own
const COAT_GAMMA: f32 = 2.2;
// ENHANCEMENT ENH-COAT (APPROXIMATION, enhancement content by eye unless named): the least `n.v`; the glass's lobe (GGX alpha,
// mip 1's) and its most coverage; the coat's F0 (a dielectric of IOR 1.5); ENH-GRILLE's ramp of the texel's max(rgb) (a black
// texel is no paint); the horizon fade's factor (Filament's 1.3); the specular occlusion's ramps (the normal's down-facing end,
// the ray's start / end), the ray's share and the most occlusion; the most radiance the reflection takes
const COAT_MIN_NV: f32 = 0.001;
const COAT_GLASS_ALPHA: f32 = 0.03;
const COAT_GLASS_MAX: f32 = 0.95;
const COAT_F0: f32 = 0.04;
const COAT_GRILLE: vec2<f32> = vec2<f32>(0.03, 0.15);
const COAT_HORIZON: f32 = 1.3;
const COAT_OCC_N_END: f32 = -0.6;
const COAT_OCC_R: vec2<f32> = vec2<f32>(0.05, -0.5);
const COAT_OCC_R_SHARE: f32 = 0.4;
const COAT_OCC_MAX: f32 = 0.8;
const COAT_ENV_MAX: f32 = 64.0;

// ENHANCEMENT E49 (NEAR-REFL2): the cube's mip the sphere-map overlay reads (GGX alpha 0.03: soft like the 256 px `SPC2` / `SPC3`
// spread over every direction, sharp enough for the tunnel's lamps; mip 2 was compared and read flat), and the mip of the up face
// whose centre stands for "how covered is the sky over the car" (alpha 0.45, a wide cone). APPROXIMATION: both by eye.
const NEAR_OVER_LOD: f32 = 1.0;
const NEAR_OVER_UP_MIP: i32 = 4;
// ENHANCEMENT E49 (NEAR-REFL4): the painted sky's hue, the unit opponent-colour vector (r - (g + b) / 2, sqrt(3) / 2 (g - b)) of
// `SPC2`'s alpha-weighted mean 55/68/77 (`carcmn.fsh`, measured in NEAR-REFL3): (-17.5, -7.79) / 19.16, 204 degrees
const NEAR_SPC_HUE: vec2<f32> = vec2<f32>(-0.9135, -0.4069);
// the up face (+Y, `gpu::REFL_FACES`), and NEAR-REFL4's numbers (APPROXIMATION, by eye, as read in the comment at their use): the
// hue fade's cosines (45 and 20 degrees off the painted hue), the saturation gate, the least luminance and the luminance fade's end;
// the opponent-colour axis' sqrt(3) / 2
const NEAR_UP_FACE: i32 = 4;
const NEAR_HUE_COS: vec2<f32> = vec2<f32>(0.7071, 0.9397);
const NEAR_SAT_GATE: vec2<f32> = vec2<f32>(0.05, 0.15);
const NEAR_MIN_LUM: f32 = 0.004;
const NEAR_LUM_FADE_END: f32 = 0.02;
const HALF_SQRT3: f32 = 0.8660254;

// ENHANCEMENT E17: the slope of a gentle procedural ripple at world point `p` (metres), `ph` the water
// material's own UV-scroll phase in turns (it wraps at 1, so every term turns a whole number of times per
// wrap): four crossing waves of 2..7 m, the total slope at most ~0.03, so the original's still water only
// shimmers. No texture: a sum of cosines.
// The waves (APPROXIMATION, enhancement content by eye): xy the direction, z the slope's amplitude, w the wavenumber (rad / m); their
// phases turn 1, -2, 1 and 3 times per wrap (written in `water_slope`)
const WATER_WAVE_0: vec4<f32> = vec4<f32>(0.80, 0.60, 0.011, 1.9);
const WATER_WAVE_1: vec4<f32> = vec4<f32>(-0.55, 0.83, 0.009, 2.7);
const WATER_WAVE_2: vec4<f32> = vec4<f32>(0.20, -0.98, 0.007, 1.1);
const WATER_WAVE_3: vec4<f32> = vec4<f32>(-0.95, -0.31, 0.005, 3.4);
fn water_slope(p: vec2<f32>, ph: f32) -> vec2<f32> {
    let t = ph * TAU;
    var s = vec2<f32>(0.0);
    s += WATER_WAVE_0.xy * (WATER_WAVE_0.z * cos(dot(p, WATER_WAVE_0.xy) * WATER_WAVE_0.w + t));
    s += WATER_WAVE_1.xy * (WATER_WAVE_1.z * cos(dot(p, WATER_WAVE_1.xy) * WATER_WAVE_1.w - 2.0 * t));
    s += WATER_WAVE_2.xy * (WATER_WAVE_2.z * cos(dot(p, WATER_WAVE_2.xy) * WATER_WAVE_2.w + t));
    s += WATER_WAVE_3.xy * (WATER_WAVE_3.z * cos(dot(p, WATER_WAVE_3.xy) * WATER_WAVE_3.w + 3.0 * t));
    return s;
}

// ENHANCEMENT E17's surface (APPROXIMATION, enhancement content by eye): a level face's least share of the slope's normal that is
// vertical; the ripple's fade (m) and level 2's extra ripple; the Fresnel term (F0 of water, 1 - F0) and Schlick's power (E26's too);
// the reflection's weight (base, level 2's extra, level 2's lift; the most, level 2's extra); the reflected ray's least height; the
// sky's ramp toward the zenith; the sun glint's colour, its three lobes' powers and gains, its grazing ramp and level 2's extra
const WATER_LEVEL: f32 = 0.8;
const WATER_RIPPLE_FADE: vec2<f32> = vec2<f32>(60.0, 220.0);
const WATER_RIPPLE_STRONG: f32 = 1.6;
const WATER_FRESNEL: vec2<f32> = vec2<f32>(0.02, 0.98);
const SCHLICK_POW: f32 = 5.0;
const WATER_K_BASE: f32 = 0.9;
const WATER_K_STRONG: f32 = 0.8;
const WATER_K_LIFT: f32 = 0.10;
const WATER_K_MAX: f32 = 0.8;
const WATER_K_MAX_STRONG: f32 = 0.1;
const WATER_MIN_RAY_Y: f32 = 0.02;
const WATER_SKY_RAMP: f32 = 0.55;
const WATER_GLINT_TINT: vec3<f32> = vec3<f32>(1.0, 0.93, 0.78);
const WATER_GLINT_POW: vec3<f32> = vec3<f32>(6000.0, 500.0, 60.0);
const WATER_GLINT_GAIN: vec3<f32> = vec3<f32>(5.0, 0.2, 0.10);
const WATER_GLINT_GRAZE: vec2<f32> = vec2<f32>(0.25, 0.75);
const WATER_GLINT_STRONG: f32 = 1.5;

// ENHANCEMENT E26: two cheap hashes and a value noise for the puddle mask and the splashes' random cells
// (Hoskins' `hash12`: its multipliers and its offset)
const RAIN_HASH_K: vec2<f32> = vec2<f32>(0.1031, 0.1030);
const RAIN_HASH_ADD: f32 = 33.33;
fn rain_hash(p: vec2<f32>) -> f32 {
    var q = fract(p * RAIN_HASH_K);
    q = q + dot(q, q.yx + RAIN_HASH_ADD);
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
// The rings' numbers (APPROXIMATION, enhancement content by eye): the fade with the distance (m), the cell (m), the two seeds of a
// cell's other hashes, the drop's margin and spread in its cell (shares of the cell), the loop's rate (1 / s) and the phase's mix of
// the hashes, the ring's widest radius (m), its width (m: at the start, its growth), the slope's gain
const RAIN_RING_FADE: vec2<f32> = vec2<f32>(10.0, 24.0);
const RAIN_CELL: f32 = 1.4;
const RAIN_SEED_A: vec2<f32> = vec2<f32>(7.1, 3.7);
const RAIN_SEED_B: vec2<f32> = vec2<f32>(1.9, 9.3);
const RAIN_DROP_MARGIN: f32 = 0.3;
const RAIN_DROP_SPREAD: f32 = 0.4;
const RAIN_RING_RATE: f32 = 0.9;
const RAIN_PHASE_MIX: f32 = 5.3;
const RAIN_RING_RADIUS: f32 = 0.34;
const RAIN_RING_WIDTH: vec2<f32> = vec2<f32>(0.03, 0.03);
const RAIN_RING_SLOPE: f32 = 0.35;
fn rain_rings(p: vec2<f32>, t: f32, density: f32, dist: f32) -> vec3<f32> {
    let fade = 1.0 - smoothstep(RAIN_RING_FADE.x, RAIN_RING_FADE.y, dist);
    if fade <= 0.0 || density <= 0.0 {
        return vec3<f32>(0.0);
    }
    let cell = floor(p / RAIN_CELL);
    let h = rain_hash(cell);
    if h > density {
        return vec3<f32>(0.0);
    }
    let h2 = rain_hash(cell + RAIN_SEED_A);
    let h3 = rain_hash(cell + RAIN_SEED_B);
    let centre = (cell + vec2<f32>(RAIN_DROP_MARGIN + RAIN_DROP_SPREAD * h2, RAIN_DROP_MARGIN + RAIN_DROP_SPREAD * h3)) * RAIN_CELL;
    let age = fract(t * RAIN_RING_RATE + h2 * RAIN_PHASE_MIX + h3);
    let d = p - centre;
    let r = length(d);
    let ring_r = age * RAIN_RING_RADIUS;
    let w = RAIN_RING_WIDTH.x + RAIN_RING_WIDTH.y * age;
    let ring = exp(-((r - ring_r) / w) * ((r - ring_r) / w)) * (1.0 - age) * (1.0 - age);
    let dir = d / max(r, MIN_DIST);
    return vec3<f32>(dir * ring * RAIN_RING_SLOPE * fade, ring * fade);
}

// ENHANCEMENT E26's wet road (APPROXIMATION, enhancement content by eye): the nearest wetness edge's start (m) and reach squared
// (90 m); the least wetness drawn; the puddle noise's two frequencies (1 / m), mix and shift (m), its ramp and the wetness' shift of
// it, the wetness ramp of the puddles; the luma ramp that keeps bright markings; the albedo's darkening (a road, a puddle); the
// rings' density; the Fresnel term (F0 of a wet road, 1 - F0); the reflection's weight (base, puddle, most) and a skid's (by
// the Fresnel, the wetness, most); the sky's ramp to the zenith and its gain, the rings' glint; a lamp's streak: the azimuth's
// nudge off zero, the elevation spread below / above the eye's side, the azimuth spread (base, puddle), the default reach squared
// (a street lamp's, `gpu/fill.rs`), the gain (base, by the Fresnel, base, by the puddle); a skid's alpha margin
// over its test's reference, its opacity's fall, the lift's ramp and the sheen
const RAIN_EDGE_FAR: f32 = 80.0;
const RAIN_EDGE_REACH2: f32 = 8100.0;
const RAIN_MIN_WET: f32 = 0.004;
const RAIN_PUDDLE_FREQ: vec2<f32> = vec2<f32>(0.19, 0.61);
const RAIN_PUDDLE_MIX: vec2<f32> = vec2<f32>(0.62, 0.38);
const RAIN_PUDDLE_SHIFT: vec2<f32> = vec2<f32>(17.0, 5.0);
const RAIN_PUDDLE_RAMP: vec2<f32> = vec2<f32>(0.60, 0.74);
const RAIN_PUDDLE_WET_SHIFT: f32 = 0.16;
const RAIN_PUDDLE_WET: vec2<f32> = vec2<f32>(0.4, 0.9);
const RAIN_KEEP_LUMA: vec2<f32> = vec2<f32>(0.22, 0.45);
const RAIN_DARKEN: f32 = 0.85;
const RAIN_PUDDLE_DARKEN: f32 = 0.78;
const RAIN_RING_DENSITY: f32 = 0.16;
const RAIN_FRESNEL: vec2<f32> = vec2<f32>(0.05, 0.95);
const RAIN_K_BASE: f32 = 0.40;
const RAIN_K_PUDDLE: f32 = 0.55;
const RAIN_K_MAX: f32 = 0.55;
const RAIN_SKID_K: vec3<f32> = vec3<f32>(0.9, 0.6, 0.85);
const RAIN_SKY_RAMP: f32 = 0.6;
const RAIN_SKY_GAIN: f32 = 0.85;
const RAIN_RING_GLINT: f32 = 0.22;
const RAIN_AZ_NUDGE: f32 = 0.0001;
const RAIN_STREAK_BELOW: f32 = 0.06;
const RAIN_STREAK_ABOVE: f32 = 0.2;
const RAIN_STREAK_AZ: vec2<f32> = vec2<f32>(0.035, 0.02);
const RAIN_LAMP_REACH2: f32 = 4000.0;
const RAIN_STREAK_GAIN: vec4<f32> = vec4<f32>(0.13, 0.22, 0.35, 0.65);
const RAIN_SKID_ALPHA_MARGIN: f32 = 0.004;
const RAIN_SKID_FADE: f32 = 0.8;
const RAIN_SKID_LIFT_RAMP: f32 = 5.0;
const RAIN_SKID_SHEEN: f32 = 0.3;

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

// `shade`'s sections of the enhancements that work on its colour `lit0` and coverage `alpha0` (the premultiplied sum of the
// layers and its coverage when `layered`): each returns the new `vec4(lit, alpha)`; the call sites in `shade` carry the comments.
//@if COAT
fn shade_coat(in: VsOut, layered: bool, n: vec3<f32>, tex: vec4<f32>, dwx: vec3<f32>, dwy: vec3<f32>, lit0: vec3<f32>, alpha0: f32) -> vec4<f32> {
    var lit = lit0;
    var alpha = alpha0;
    if object.refl.x > REFL_PAINT - 0.5 && object.refl.x < REFL_GLASS + 0.5 && globals.refl.x > 0.0 && globals.ambient.z < 0.5 && (object.shadow_on.w <= 0.0 || alpha * BYTE >= object.shadow_on.w) {
        var v = normalize(globals.cam_pos.xyz - in.world);
        let nv = clamp(dot(n, v), COAT_MIN_NV, 1.0);
        let r = reflect(-v, n);
        var ng = normalize(cross(dwx, dwy));
        if dot(ng, v) < 0.0 {
            ng = -ng;
        }
        let glass = object.refl.x > REFL_GLASS - 0.5;
        var alpha_g = globals.refl.w;
        if glass {
            alpha_g = COAT_GLASS_ALPHA;
        }
        let ab = env_brdf(sqrt(alpha_g), nv);
        var fc = (COAT_F0 * ab.x + ab.y) * globals.refl.x;
        // ENHANCEMENT ENH-GRILLE: the coat lies on PAINT. A texel of the page that is (near) black is not paint but an opening or
        // black plastic (the rear engine grille's slats, 0x00 between grey bars): a 4 % sky over a black texel is ~40 / 255 after the
        // display gamma, and the dark slats turned into a flat grey plate. The weight rises from 0 at max(rgb) 0.03 to 1 at 0.15
        // (a dark paint is well above it); glass keeps its own coverage rule.
        if !glass {
            fc = fc * smoothstep(COAT_GRILLE.x, COAT_GRILLE.y, max(tex.r, max(tex.g, tex.b)));
        }
        var horizon = clamp(1.0 + COAT_HORIZON * dot(r, ng), 0.0, 1.0);
        horizon = horizon * horizon;
        // specular occlusion: the lower hemisphere's share, from how far down the surface and the ray face
        let down = smoothstep(0.0, COAT_OCC_N_END, n.y) * 0.5 + smoothstep(COAT_OCC_R.x, COAT_OCC_R.y, r.y) * COAT_OCC_R_SHARE;
        let occ = 1.0 - clamp(down, 0.0, COAT_OCC_MAX);
        //@if !NEAR
        let env = min(refl_sample_lod(r, coat_lod(alpha_g)), vec3<f32>(COAT_ENV_MAX)) * horizon * occ;
        //@endif
        //@if NEAR
        // ENHANCEMENT E49: from the shading point (the box-projected lookup), the player's own car from the faces with the near cars
        var env_in = vec3<f32>(0.0);
        if globals.near.x > 0.0 {
            env_in = near_env(in.world, r, select(coat_lod(alpha_g), 0.0, globals.near.w > NEAR_DEBUG_SHARP - 0.5), object.refl.w > 0.5);
        } else {
            env_in = refl_sample_lod(r, coat_lod(alpha_g));
        }
        let env = min(env_in, vec3<f32>(COAT_ENV_MAX)) * horizon * occ;
        // TEST GLUE (`NFS_NEAR_DEBUG`): the paint shows the reflection alone
        if globals.near.w > NEAR_DEBUG_REFL - 0.5 && globals.near.w < NEAR_DEBUG_SHARP + 0.5 {
            fc = 1.0;
        }
        //@endif
        if glass {
            let k = clamp(fc, 0.0, COAT_GLASS_MAX);
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
                straight = lit / max(alpha, MIN_DIV);
            }
            let lin = pow(max(straight, vec3<f32>(0.0)), vec3<f32>(COAT_GAMMA)) * (1.0 - fc) + pow(env, vec3<f32>(COAT_GAMMA)) * fc;
            lit = pow(lin, vec3<f32>(1.0 / COAT_GAMMA));
            if layered {
                lit = lit * alpha;
            }
        }
    }
    return vec4<f32>(lit, alpha);
}
//@endif

//@if WATER
fn shade_water(in: VsOut, dwx: vec3<f32>, dwy: vec3<f32>, lit0: vec3<f32>, alpha0: f32) -> vec4<f32> {
    var lit = lit0;
    var alpha = alpha0;
    if object.refl.x > REFL_WATER - 0.5 && globals.water.x > 0.5 && abs(cross(dwx, dwy).y) > WATER_LEVEL * length(cross(dwx, dwy)) {
        let vv = globals.cam_pos.xyz - in.world;
        let dist = length(vv);
        let v = vv / max(dist, MIN_DIST);
        // the ripple fades out with distance (it would alias to noise) and is the original's still sea up close
        let amp = 1.0 - smoothstep(WATER_RIPPLE_FADE.x, WATER_RIPPLE_FADE.y, dist);
        // level 2 («сильнее», `globals.water.w` = 1): a stronger ripple, a higher reflection weight, a brighter glint
        let wst = globals.water.w;
        let sl = water_slope(in.world.xz, object.env_mix.w) * amp * (1.0 + WATER_RIPPLE_STRONG * wst);
        let nw = normalize(vec3<f32>(-sl.x, 1.0, -sl.y));
        let nv = clamp(dot(nw, v), 0.0, 1.0);
        // water's F0 is 0.02; the weight is held below 1 so the texture always shows through
        let fres = WATER_FRESNEL.x + WATER_FRESNEL.y * pow(1.0 - nv, SCHLICK_POW);
        let k = clamp(fres * (WATER_K_BASE + WATER_K_STRONG * wst) + WATER_K_LIFT * wst, 0.0, WATER_K_MAX + WATER_K_MAX_STRONG * wst);
        var r = reflect(-v, nw);
        r.y = max(r.y, WATER_MIN_RAY_Y);
        var env = vec3<f32>(0.0);
        if globals.refl.x > 0.0 && globals.water.z > 0.5 {
            env = refl_sample(r);
        } else {
            env = mix(globals.water_horizon.rgb, globals.water_zenith.rgb, smoothstep(0.0, WATER_SKY_RAMP, r.y));
        }
        var glint = vec3<f32>(0.0);
        if globals.water.y > 0.5 {
            let sd = max(dot(r, globals.water_sun.xyz), 0.0);
            // a hard core and a wide sheen, both stronger toward grazing views (the sun path on the sea)
            glint = WATER_GLINT_TINT * (pow(sd, WATER_GLINT_POW.x) * WATER_GLINT_GAIN.x + pow(sd, WATER_GLINT_POW.y) * WATER_GLINT_GAIN.y + pow(sd, WATER_GLINT_POW.z) * WATER_GLINT_GAIN.z * wst) * (WATER_GLINT_GRAZE.x + WATER_GLINT_GRAZE.y * fres) * (1.0 + WATER_GLINT_STRONG * wst);
        }
        lit = mix(lit, env, k) + glint;
        alpha = clamp(alpha + (1.0 - alpha) * min(k + max(glint.r, max(glint.g, glint.b)), 1.0), 0.0, 1.0);
    }
    return vec4<f32>(lit, alpha);
}
//@endif

//@if RAIN
fn shade_wet_road(in: VsOut, base_rgb: vec3<f32>, lit0: vec3<f32>, alpha0: f32) -> vec4<f32> {
    var lit = lit0;
    var alpha = alpha0;
    if object.refl.x > REFL_WET_ROAD - 0.5 && globals.rain.x > 0.0 {
        let skid = object.refl.x > REFL_SKID - 0.5;
        // the part's own wetness (its centre's slice); near a step of the original's field 5 the fragment takes the
        // ramp of the nearest edge instead (a part is longer than the step, and the original's step is one slice wide)
        var wet = clamp(object.refl.w, 0.0, 1.0);
        var near_edge = RAIN_EDGE_FAR;
        for (var i = 0u; i < u32(globals.rain.w); i = i + 1u) {
            let off = in.world - globals.rain_edge_pos[i].xyz;
            let s = dot(off, globals.rain_edge_dir[i].xyz);
            if abs(s) < near_edge && dot(off, off) < RAIN_EDGE_REACH2 {
                near_edge = abs(s);
                let hw = globals.rain_edge_pos[i].w;
                wet = mix(globals.rain_edge_dir[i].w, globals.rain_edge_val[i].x, smoothstep(-hw, hw, s));
            }
        }
        if wet > RAIN_MIN_WET {
        let g = globals.rain.x;
        let vv = globals.cam_pos.xyz - in.world;
        let dist = length(vv);
        let v = vv / max(dist, MIN_DIST);
        let pn = rain_noise(in.world.xz * RAIN_PUDDLE_FREQ.x) * RAIN_PUDDLE_MIX.x + rain_noise(in.world.xz * RAIN_PUDDLE_FREQ.y + RAIN_PUDDLE_SHIFT) * RAIN_PUDDLE_MIX.y;
        // puddles grow with the wetness and only form in real rain (a damp 0.5 stretch has few)
        let pud = smoothstep(RAIN_PUDDLE_RAMP.x - RAIN_PUDDLE_WET_SHIFT * wet, RAIN_PUDDLE_RAMP.y - RAIN_PUDDLE_WET_SHIFT * wet, pn) * smoothstep(RAIN_PUDDLE_WET.x, RAIN_PUDDLE_WET.y, wet);
        let wetf = clamp(wet * g, 0.0, 1.0);
        // the albedo darkens less where it is bright (the lane markings stay readable)
        let luma = dot(base_rgb, LUMA);
        let keep = smoothstep(RAIN_KEEP_LUMA.x, RAIN_KEEP_LUMA.y, luma);
        if !skid {
            lit = lit * mix(1.0, mix(RAIN_DARKEN, 1.0, keep), wetf) * mix(1.0, RAIN_PUDDLE_DARKEN, pud * wetf * (1.0 - keep));
        }
        // the splash rings: density by the wetness; they tilt the normal a little and glint
        let rg = rain_rings(in.world.xz, globals.rain.y, RAIN_RING_DENSITY * wet * g, dist);
        let nw = normalize(vec3<f32>(rg.x, 1.0, rg.y));
        let nv = clamp(dot(nw, v), 0.0, 1.0);
        let fres = RAIN_FRESNEL.x + RAIN_FRESNEL.y * pow(1.0 - nv, SCHLICK_POW);
        // far, grazing road reflects more; the puddles most
        var k = clamp(fres * (RAIN_K_BASE + RAIN_K_PUDDLE * pud) * wetf, 0.0, RAIN_K_MAX) * (1.0 - 0.5 * keep);
        if skid {
            // the wiped streak is smoother than the road round it: a stronger reflection, whatever the rubber's colour
            k = clamp(fres * RAIN_SKID_K.x * wetf + RAIN_SKID_K.y * wetf, 0.0, RAIN_SKID_K.z);
        }
        let r = reflect(-v, nw);
        let sky = mix(globals.rain_horizon.rgb, globals.rain_zenith.rgb, smoothstep(0.0, RAIN_SKY_RAMP, max(r.y, 0.0)));
        lit = mix(lit, sky * RAIN_SKY_GAIN, k) + sky * rg.z * RAIN_RING_GLINT * wetf;
        // the lamps' streaks: azimuth difference narrow, elevation difference long on the eye's side
        var streak = vec3<f32>(0.0);
        let rh = normalize(r.xz + vec2<f32>(RAIN_AZ_NUDGE, 0.0));
        for (var i = 0u; i < u32(globals.rain.z); i = i + 1u) {
            let d = globals.rain_lamp_pos[i].xyz - in.world;
            let dl = length(d);
            let dh = normalize(d.xz + vec2<f32>(RAIN_AZ_NUDGE, 0.0));
            if dot(rh, dh) > 0.0 {
                let az = rh.x * dh.y - rh.y * dh.x;
                let dv = r.y - d.y / dl;
                var sv = RAIN_STREAK_BELOW;
                if dv > 0.0 {
                    sv = RAIN_STREAK_ABOVE;
                }
                let sa = RAIN_STREAK_AZ.x + RAIN_STREAK_AZ.y * pud;
                let e = exp(-(az / sa) * (az / sa) - (dv / sv) * (dv / sv));
                // AUTOBAHN-LAMPS: the light's own reach (`w` = reach squared; 0 = the street lamps' 4000): a car's
                // tail light no longer streaks as far and as bright as a street lamp
                let r2 = select(RAIN_LAMP_REACH2, globals.rain_lamp_pos[i].w, globals.rain_lamp_pos[i].w > 0.0);
                streak += globals.rain_lamp_col[i].rgb * (e / (1.0 + dl * dl / r2));
            }
        }
        lit = lit + streak * (RAIN_STREAK_GAIN.x + RAIN_STREAK_GAIN.y * fres) * (RAIN_STREAK_GAIN.z + RAIN_STREAK_GAIN.w * pud) * wetf;
        if skid {
            // the opacity falls (the alpha test's reference keeps its texels: it reads this alpha) and the reflection
            // lifts it again where the streak glints
            let floor_a = min(alpha, object.shadow_on.w / BYTE + RAIN_SKID_ALPHA_MARGIN);
            alpha = max(alpha * mix(1.0, RAIN_SKID_FADE, wetf), floor_a);
            alpha = clamp(alpha + (1.0 - alpha) * k * 0.5 * min(alpha * RAIN_SKID_LIFT_RAMP, 1.0), 0.0, 1.0);
            lit = lit + sky * RAIN_SKID_SHEEN * wetf;
        }
        }
    }
    return vec4<f32>(lit, alpha);
}
//@endif

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
            let car_rx = object.lit_d.w > RECV_NONE + 0.5 && object.lit_d.w < RECV_CAR + 0.5;
            var moon_frame = false;
            //@if MOON
            // ENHANCEMENT E50: a moon frame leaves the original's own direct term unshadowed (on Industrial it comes from the same
            // direction, but in the bake that is the street's warm light, not the moon's); the moon's cold term below has the shadow
            moon_frame = csm_mode_is(SHADOW_MOON);
            //@endif
            var sh = 1.0;
            if csm.dir.w > 0.5 && car_rx && !moon_frame && dot(normalize(object.lit_dir.xyz), csm.orig.xyz) > SUN_SAME_COS {
                // never fully black: the mode's floor of the direct term (`params.w`: a quarter in the sun, the sky's own scatter)
                sh = csm.params.w + (1.0 - csm.params.w) * csm_visibility(in.world, n);
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
            if moon_frame && car_rx {
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
        let facing = mix(FACING_FLOOR, 1.0, smoothstep(0.0, FACING_FULL, dot(n, -csm.dir.xyz)));
        // the "already baked dark" gate (a shadow baked into the colour is not darkened twice), by the mode
        let luma = dot(in.color.rgb, LUMA);
        var bright = smoothstep(BAKED_DARK_LO, BAKED_DARK_HI, luma);
        // GATE-REL (`params.z` > 0, `gpu/csm/mode.rs` GATE_REL; APPROXIMATION, an enhancement's look): the gate by the baked luma against the lit
        // level the track's bake light gives this orientation (`nfs_game::bake_light`: `base + gain max(0, n.L)`, times the ground's
        // factor on drivable ground, `lit_d.w`'s RECV_GROUND_MARK): a face darker than that is baked in shade (no second darkening),
        // a merely dark one lit in the bake (a mossy rock, a dim forest road in a sun patch) takes the whole shadow
        // `baked` (the cap mode): the bake's own shade counts toward the shadow: the point is darkened only down
        // to the shadow's level of a lit face, `min(1, keep / baked)` with `baked` = the luma / lit level, at most 1
        var baked = 1.0;
        if csm.params.z > 0.5 {
            var lit = csm.bake.w + csm.bake_k.x * max(dot(n, csm.bake.xyz), 0.0);
            if fract(object.lit_d.w) > RECV_GROUND_MARK * 0.5 {
                lit = lit * csm.bake_k.y;
            }
            let ratio = luma / max(lit, MIN_BAKE_LIT);
            baked = clamp(ratio, MIN_BAKE_LIT, 1.0);
            bright = 1.0;
        }
        //@if FOCUS
        // ENHANCEMENT ENH-GARAGE (the front end's focus shadow): the room's floor is baked dark, the car's shadow must still read
        // there: no gate
        if csm_mode_is(SHADOW_FOCUS) {
            bright = 1.0;
        }
        //@endif
        //@if MOON
        // ENHANCEMENT E50: the moon's share of a night's baked colour is the dim part of it: a pool of lamp light (bright in the bake)
        // is the lamp's, not the moon's, so the gate turns round (full below a luma of 0.25, none above 0.6)
        if csm_mode_is(SHADOW_MOON) {
            bright = 1.0 - smoothstep(MOON_LIT_LO, MOON_LIT_HI, luma);
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
            // GATE-REL's cap mode (`baked` 1 elsewhere: no change)
            keep_sun = min(1.0, keep_sun / baked);
            //@if CARSH
            // ENHANCEMENT E51: the cars' shadow passes the "baked dark" gate (a car's shadow is never baked into the road): the point
            // keeps the darker of the gated shadow of everything and the ungated shadow of the cars alone (in full light, `bright` 1,
            // the two agree and nothing changes)
            if carsh && t.y > 0.0 {
                keep_sun = min(keep_sun, 1.0 - a0 * t.y * (1.0 - car_visibility(in.world, n)));
            }
            //@endif
            light = light * keep_sun;
            // ENHANCEMENT ENH-FE3D x E43 (LAMP-TUNE): under the front end's soft focus light both would be the sky's occlusion by the
            // same car: the point keeps the darker of the two, not their product (it has no contact shadows; the rule was E25's overcast
            // light's too, until E25 was removed 2026-11-16)
            let soft_key = csm_mode_is(SHADOW_FOCUS);
            if ao < 1.0 && soft_key {
                ao = min(ao, keep_sun) / max(keep_sun, MIN_DIV);
            }
            // ENHANCEMENT E43 (SHADOW-REAL): the contact shadow is the sky's occlusion, so it takes only the sky's (ambient) share of
            // the light, `1 - a`, and the sun's (direct) share `a` keeps the sun's visibility: of the light left at this point the
            // occlusion leaves `((1 - a) ao + a v) / ((1 - a) + a v)`. In the sun's light (v 1) it is a faint crease at the tyres and
            // sills; inside the car's own sun shadow (v 0) it is whole and deepens it under the body: one shadow, the car grounded.
            // `a` is E9's strength x facing x the share drawn here, without the "baked dark" gate (the map knows where the car
            // blocks the sun even where the bake is dim). Under cover, at night, beyond the cascades (a 0) it is the whole occlusion,
            // as before. Not under the focus light: its soft shadow is itself a sky occlusion (the min above).
            if ao < 1.0 && !soft_key {
                var a = a0 * t.y;
                //@if MOON
                // ENHANCEMENT E50: the moon's direct share is gated like its shadow (under a lamp the baked light is the lamp's)
                if csm_mode_is(SHADOW_MOON) {
                    a = s * t.y;
                }
                //@endif
                let sky = 1.0 - a;
                ao = (sky * ao + a * t.x) / max(sky + a * t.x, MIN_DIV);
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
            let up = textureSampleLevel(t_refl, s_proj, vec2<f32>(0.5, 0.5), select(NEAR_UP_FACE, NEAR_UP_FACE + i32(NEAR_OWN_LAYER), globals.near.y > 0.5), f32(NEAR_OVER_UP_MIP)).a;
            let cover = clamp(mix(up, seen.a, smoothstep(-0.25, 0.0, rv.y)), 0.0, 1.0);
            let live = clamp(seen.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
            if globals.near.x > NEAR_SKY_NIGHT - 0.5 && globals.near.x < NEAR_SKY_NIGHT + 0.5 {
                // NEAR-REFL3: on a night track (`Scene::near_sky` = `NearSky::Night`) the sphere map's day sky (`SPC2` / `SPC3` are the same
                // cloudy day sky on every track, `carcmn.fsh` from `gamedata\carmodel\`, 0x4437BB…0x4437CC / 0x441AB2…0x441AEF) is
                // wrong in the sky parts too: they show the faces' own sky (the sky pass is in them) as well. Its colour is the
                // live one; its strength is the texel's alpha scaled by how bright the live sky is against the painted texel
                // (`lum(live) / lum(spc)`, at most 1): a black night sky lays no veil over the paint (it reflects nothing), a lit
                // window, a lamp or the glowing sky of an industrial night as strongly as the original's sky. Geometry
                // (`cover`) keeps NEAR-REFL2's full strength. APPROXIMATION: the luminance ratio as the strength (by eye).
                let s = clamp(dot(live, LUMA) / max(dot(env_over.rgb, LUMA), NEAR_MIN_LUM), 0.0, 1.0);
                env_over = vec4<f32>(live, env_over.a * mix(s, 1.0, cover));
            } else if globals.near.x > NEAR_SKY_HUE - 0.5 {
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
                let k = LUMA;
                let ll = dot(live, k);
                let oc = vec2<f32>(live.r - 0.5 * (live.g + live.b), HALF_SQRT3 * (live.g - live.b));
                let along = dot(oc, NEAR_SPC_HUE) / max(length(oc), MIN_RAY);
                let w = (1.0 - smoothstep(NEAR_HUE_COS.x, NEAR_HUE_COS.y, along)) * smoothstep(NEAR_SAT_GATE.x, NEAR_SAT_GATE.y, length(oc) / max(ll, NEAR_MIN_LUM))
                    * smoothstep(NEAR_MIN_LUM, NEAR_LUM_FADE_END, ll);
                let hue = clamp(live * (dot(env_over.rgb, k) / max(ll, NEAR_MIN_LUM)), vec3<f32>(0.0), vec3<f32>(1.0));
                env_over = vec4<f32>(mix(mix(env_over.rgb, hue, w), live, cover), env_over.a);
            } else {
                env_over = vec4<f32>(mix(env_over.rgb, live, cover), env_over.a);
            }
            // TEST GLUE (`NFS_NEAR_DEBUG=5`): the overlay alone, red the weight, green the ray's own geometry share, blue the up face's
            if globals.near.w > NEAR_DEBUG_OVERLAY - 0.5 {
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
        if object.shadow_on.w > 0.0 && base_a * BYTE < object.shadow_on.w {
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
        var floor_ = FLOOR_TRACK;
        if object.soft.z < RECV_CAR + 0.5 {
            floor_ = FLOOR_CAR;
        }
        // ENHANCEMENT E23: the street lamps, the same receiver model
        var lamp_add = vec3<f32>(0.0);
        //@if LAMPS
        if lamps.params.x > 0.5 {
            var car_k = 1.0;
            if object.soft.z < RECV_CAR + 0.5 {
                car_k = LAMP_CAR_GAIN;
            }
            let ll = lamp_light(in.world, n, floor_);
            lamp_add = tex.rgb * ll.rgb * car_k * tcov;
            // MONACO-TUNNEL: the baked lamps' shadows take their share of a track item's baked colour (a car's light is not baked)
            if object.soft.z > RECV_CAR + 0.5 {
                lit = lit * (1.0 - ll.w);
            }
        }
        //@endif
        //@if SPOT
        if spots.params.x > 0.5 {
            lit = lit + tex.rgb * spot_light(in.world, n, floor_) * SPOT_GAIN * tcov;
        }
        //@endif
        lit = lit + lamp_add;
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
            // (only in the sun's mode: E50's moon casts car shadows too faint to stand in for the blob; the
            // garage's `FESH` blob: ENH-GARAGE fades it below, once the focus shadow has a contact core)
            // ENHANCEMENT E9 (SHADOW-REAL): by the point's own cover (`csm_share`), not by where the camera is
            //@if CSM
            if csm.dir.w > 0.5 && csm_mode_is(SHADOW_SUN) {
                blob = blob * (1.0 - csm_share(in.world));
            }
            //@if FOCUS
            // ENHANCEMENT ENH-GARAGE: the garage's `FESH` blob gives way to the focus shadow (`extra.w`, its core carries the contact)
            if csm.dir.w > 0.5 && csm_mode_is(SHADOW_FOCUS) {
                blob = blob * (1.0 - csm.extra.w);
            }
            //@endif
            //@endif
            // ENHANCEMENT E43: the same where a car's contact shadow stands in for it (under cover, at night, E9 off, a track without a sun:
            // where E9's own gate has not already taken it)
            //@if CONTACT
            if contacts.params.x > 0.5 && !csm_mode_is(SHADOW_FOCUS) {
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
        let f = csm_far_fade(csm_view_depth(in.world));
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
    {
        let sc = shade_coat(in, layered, n, tex, dwx, dwy, lit, alpha);
        lit = sc.xyz;
        alpha = sc.w;
    }
    //@endif
    // ENHANCEMENT E17: water (class 3: a flat water material, `nfs_assets::track::is_water`). The original
    // texture and its scroll stay as the base; over it a Fresnel-weighted reflection of the sky (the E8
    // environment map when E8 is on, else the sky's own colours), a sun glint (above 1.0: HDR where the
    // target keeps it, E7) and a faint moving ripple that only tilts the reflection. Level surfaces only
    // (a waterfall or a river on a slope is left as the original draws it).
    //@if WATER
    {
        let sc = shade_water(in, dwx, dwy, lit, alpha);
        lit = sc.xyz;
        alpha = sc.w;
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
    {
        let sc = shade_wet_road(in, base.rgb, lit, alpha);
        lit = sc.xyz;
        alpha = sc.w;
    }
    //@endif
    return vec4<f32>(lit, alpha);
}
