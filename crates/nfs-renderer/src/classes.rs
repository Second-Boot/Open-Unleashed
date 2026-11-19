//! The per-object class numbers the Rust side writes into [`crate::gpu::ObjectUniform`] and `shader.wgsl` tests.
//! One list, [`WGSL`], feeds both sides: the Rust code uses the constants below, and the mesh shader's source is
//! [`WGSL`] emitted as WGSL `const`s ([`wgsl_consts`]) in front of `shader.wgsl` (`gpu/init.rs`), so a number cannot drift
//! between the two. The values are the ones the code always used; the shader keeps its half-way thresholds (`> C - 0.5`,
//! `< C + 0.5`), only spelled with the names.
//!
//! Every class here is ENHANCEMENT data: with [`crate::Enhancements::ORIGINAL`] the fields that carry them are 0 or are not
//! read (`gpu/fill.rs`, `original_objects_are_masters`).

/// `ObjectUniform::refl.x` — ENHANCEMENT E8: the material's own reflection class ([`crate::Reflect::class`]): none.
pub(crate) const REFL_NONE: f32 = 0.0;
/// ENHANCEMENT E8: a car's body paint (`CarExtEnv`), the clear coat.
pub(crate) const REFL_PAINT: f32 = crate::Reflect::PAINT as f32;
/// ENHANCEMENT E8: a car's glass (`CarWindow`).
pub(crate) const REFL_GLASS: f32 = crate::Reflect::GLASS as f32;
/// ENHANCEMENT E17: a horizontal water item while the switch and the sky are there.
pub(crate) const REFL_WATER: f32 = 3.0;
/// ENHANCEMENT E26: a wet road part (`refl.w` its wetness).
pub(crate) const REFL_WET_ROAD: f32 = 4.0;
/// ENHANCEMENT E26: a skid strip on a wet road part.
pub(crate) const REFL_SKID: f32 = 5.0;

/// `ObjectUniform::lit_d.w` (ENHANCEMENT E9) and `soft.z` (ENHANCEMENT E15 / E23): what the item does with the sun's
/// shadow / the spots — none.
pub(crate) const RECV_NONE: f32 = 0.0;
/// a car: its direct term takes the shadow / the spots
pub(crate) const RECV_CAR: f32 = 1.0;
/// a track item (or a front-end room piece): its baked colour does
pub(crate) const RECV_TRACK: f32 = 2.0;
/// `lit_d.w` only, ENHANCEMENT ENH-DBLSH: a track `sh80` decal under a caster (the shader fades it out with the shadow)
pub(crate) const RECV_DECAL: f32 = 3.0;
/// ENHANCEMENT E9 (GATE-REL): added to `lit_d.w`'s [`RECV_TRACK`] for a drivable-ground item (it receives and does not cast): the
/// relative gate takes the ground's own lit level there. The class tests compare with half steps, so the mark leaves them alone.
pub(crate) const RECV_GROUND_MARK: f32 = 0.25;
/// ENHANCEMENT E9 (GATE-REL): the least lit level the relative gate divides by.
pub(crate) const MIN_BAKE_LIT: f32 = 0.05;

/// `ObjectUniform::soft.w` — ENHANCEMENT ENH-TEMPORAL: the item's role for the masks: an opaque-like item.
pub(crate) const ROLE_OPAQUE: f32 = 0.0;
/// a translucent effect (reactive coverage)
pub(crate) const ROLE_EFFECT: f32 = 1.0;
/// a sky layer (writes the vector, no mask)
pub(crate) const ROLE_SKY: f32 = 2.0;

/// `ObjectUniform::soft.y` — ENHANCEMENT E12: how a soft sprite fades: its alpha only.
pub(crate) const SOFT_FADE_ALPHA: f32 = 0.0;
/// its colour toward black (the shader's branch; no item sets it today)
pub(crate) const SOFT_FADE_BLACK: f32 = 1.0;
/// its colour toward white: the multiplying pair (blend pair 3)
pub(crate) const SOFT_FADE_WHITE: f32 = 2.0;

/// `ObjectUniform::hdr.z` — ENHANCEMENT E18: a lit particle by its centre ([`crate::PARTICLE_LIT`]).
pub(crate) const PART_LIT: f32 = crate::PARTICLE_LIT as f32;
/// a lit particle per fragment ([`crate::PARTICLE_LIT_WORLD`]).
pub(crate) const PART_LIT_WORLD: f32 = crate::PARTICLE_LIT_WORLD as f32;

/// `Csm::misc.y` — ENHANCEMENT E9 (SHADOW-REFACTOR): the frame's shadow mode (`gpu/csm/mode.rs` `ShadowMode`): the race's sun (the
/// sky's, a sunless track's data sun, E51's cars-only frame). 0, so a zeroed uniform reads as no special mode.
pub(crate) const SHADOW_SUN: f32 = 0.0;
// 1.0 was E25's overcast light (removed 2026-11-16, SHADOW-OVERCAST); the other values keep theirs
/// ENHANCEMENT E50: the moon (its soft disc, the turned-round gate, the cars' cold term)
pub(crate) const SHADOW_MOON: f32 = 2.0;
/// ENHANCEMENT ENH-FE3D: a front-end car's focus shadow (the jittered disc, no "baked dark" gate, the `FESH` blob's own fade)
pub(crate) const SHADOW_FOCUS: f32 = 3.0;

/// SHADOW-REFACTOR: numbers both sides use (not classes, but the same reason to have them once). ENHANCEMENT E9 / E51: a cascade's
/// view-depth range blends into the next one's over its last eighth (`shader.wgsl` `csm_split`, `gpu/carsun.rs` per car).
pub(crate) const CASCADE_BLEND: f32 = 0.125;
pub(crate) const CASCADE_BLEND_START: f32 = 0.875;
/// ENHANCEMENT E23 / E47: a street lamp's downward cone, `smoothstep(start, end, l.y)` of the direction to the lamp (`shader.wgsl`
/// `lamp_light`, `gpu/lamps.rs` `lamp_score`).
pub(crate) const LAMP_CONE_START: f32 = 0.1;
pub(crate) const LAMP_CONE_END: f32 = 0.55;

/// `GlobalsUniform::near.x` — ENHANCEMENT E49: what the player's sphere-map overlay's sky parts show (`Scene::near_sky`, 0 = the
/// near lookup off): the original's painted sky (NEAR-REFL2: only the geometry parts are live; the shader's other arm, not emitted).
pub(crate) const NEAR_SKY_ORIGINAL: f32 = 1.0;
/// NEAR-REFL3, a night track: the live sky, its strength scaled by its brightness
pub(crate) const NEAR_SKY_NIGHT: f32 = 2.0;
/// NEAR-REFL4, a day / dusk track: the live sky's hue at the painted texel's strength and luminance
pub(crate) const NEAR_SKY_HUE: f32 = 3.0;
/// `near.w` — TEST GLUE (`NFS_NEAR_DEBUG`): the paint shows the reflection alone (this level and [`NEAR_DEBUG_SHARP`]), read at
/// mip 0 (from [`NEAR_DEBUG_SHARP`] on), the overlay's weights as a colour (from [`NEAR_DEBUG_OVERLAY`] on)
pub(crate) const NEAR_DEBUG_REFL: f32 = 1.0;
pub(crate) const NEAR_DEBUG_SHARP: f32 = 2.0;
pub(crate) const NEAR_DEBUG_OVERLAY: f32 = 3.0;

/// ENHANCEMENT E49: the player's own six faces (the near cars drawn in) follow E8's six in the cube's layers.
pub(crate) const NEAR_OWN_LAYER: u32 = 6;

/// ENHANCEMENT E23 / E47: the street lamps' shadow slots, one depth layer each (`gpu/lamps.rs`, `Lamps::smat` / `sinfo`).
pub(crate) const LAMP_SHADOW_SLOTS: usize = 4;

/// Every class constant by its WGSL name (the same as the Rust name).
pub(crate) const WGSL: &[(&str, f32)] = &[
    ("REFL_NONE", REFL_NONE),
    ("REFL_PAINT", REFL_PAINT),
    ("REFL_GLASS", REFL_GLASS),
    ("REFL_WATER", REFL_WATER),
    ("REFL_WET_ROAD", REFL_WET_ROAD),
    ("REFL_SKID", REFL_SKID),
    ("RECV_NONE", RECV_NONE),
    ("RECV_CAR", RECV_CAR),
    ("RECV_TRACK", RECV_TRACK),
    ("RECV_DECAL", RECV_DECAL),
    ("RECV_GROUND_MARK", RECV_GROUND_MARK),
    ("MIN_BAKE_LIT", MIN_BAKE_LIT),
    ("ROLE_OPAQUE", ROLE_OPAQUE),
    ("ROLE_EFFECT", ROLE_EFFECT),
    ("ROLE_SKY", ROLE_SKY),
    ("SOFT_FADE_ALPHA", SOFT_FADE_ALPHA),
    ("SOFT_FADE_BLACK", SOFT_FADE_BLACK),
    ("SOFT_FADE_WHITE", SOFT_FADE_WHITE),
    ("PART_LIT", PART_LIT),
    ("PART_LIT_WORLD", PART_LIT_WORLD),
    ("SHADOW_SUN", SHADOW_SUN),
    ("SHADOW_MOON", SHADOW_MOON),
    ("SHADOW_FOCUS", SHADOW_FOCUS),
    ("CASCADE_BLEND", CASCADE_BLEND),
    ("CASCADE_BLEND_START", CASCADE_BLEND_START),
    ("LAMP_CONE_START", LAMP_CONE_START),
    ("LAMP_CONE_END", LAMP_CONE_END),
    ("BAKED_SHADOW_MAX", crate::BAKED_SHADOW_MAX),
    ("NEAR_SKY_NIGHT", NEAR_SKY_NIGHT),
    ("NEAR_SKY_HUE", NEAR_SKY_HUE),
    ("NEAR_DEBUG_REFL", NEAR_DEBUG_REFL),
    ("NEAR_DEBUG_SHARP", NEAR_DEBUG_SHARP),
    ("NEAR_DEBUG_OVERLAY", NEAR_DEBUG_OVERLAY),
    // ENHANCEMENT ENH-COAT: the cube's last mip (`coat_lod`'s ceiling)
    ("COAT_LAST_MIP", (crate::coat::MIPS - 1) as f32),
];

/// The array sizes and layer numbers both sides use, by their WGSL name (`u32` constants: the uniforms' arrays, the cube's layers).
pub(crate) const WGSL_SIZES: &[(&str, usize)] = &[
    // ENHANCEMENT E15: `Spots::pos` / `dir` (`gpu/spot.rs` `SpotUniform`)
    ("MAX_SPOTS", crate::MAX_SPOTS),
    // ENHANCEMENT E23 / E47: `Lamps::pos` / `col`, `smat` / `sinfo` (`gpu/lamps.rs` `LampUniform`)
    ("MAX_LAMPS", crate::MAX_LAMPS),
    ("LAMP_SHADOW_SLOTS", LAMP_SHADOW_SLOTS),
    // ENHANCEMENT E43: `Contacts`' per-car arrays (and two tyre pairs a car, `wheels`) (`gpu/contact.rs` `ContactUniform`)
    ("MAX_CONTACTS", crate::MAX_CONTACTS),
    // ENHANCEMENT E16: the probe's SH coefficients (`probe_sh`, `probe.rs`)
    ("SH_COEFFS", crate::probe::SH_COEFFS),
    // ENHANCEMENT E49: the first of the player's six faces in the twelve-layer cube (`gpu/refl.rs`)
    ("NEAR_OWN_LAYER", NEAR_OWN_LAYER as usize),
];

/// The `f32` tables both sides use: ENHANCEMENT ENH-COAT, the GGX alpha each mip of the cube holds and the widths between them
/// (`coat_lod`: `coat.rs` `MIP_ALPHA` / `MIP_SPAN`).
const WGSL_TABLES: &[(&str, &[f32])] = &[("COAT_MIP_ALPHA", &crate::coat::MIP_ALPHA), ("COAT_MIP_SPAN", &crate::coat::MIP_SPAN)];

/// [`WGSL`], [`WGSL_SIZES`] and [`WGSL_TABLES`] as WGSL module-scope constants, one per line (`const REFL_WATER: f32 = 3.0;`,
/// `const MAX_LAMPS: u32 = 32u;`, `const COAT_MIP_ALPHA: array<f32, 5> = array<f32, 5>(0.0, ...);`).
pub(crate) fn wgsl_consts() -> String {
    let mut s = String::from("// the object classes, emitted from `classes.rs`\n");
    for (name, v) in WGSL {
        s += &format!("const {name}: f32 = {v:?};\n");
    }
    for (name, n) in WGSL_SIZES {
        s += &format!("const {name}: u32 = {n}u;\n");
    }
    for (name, n) in binding::NAMES {
        s += &format!("const {name}: u32 = {n}u;\n");
    }
    for (name, t) in WGSL_TABLES {
        let items: Vec<String> = t.iter().map(|v| format!("{v:?}")).collect();
        s += &format!("const {name}: array<f32, {n}> = array<f32, {n}>({});\n", items.join(", "), n = t.len());
    }
    s
}

/// The constants the mesh shader and the E16 probe share (`probe::source` puts it in front of `probe.wgsl`): the real
/// spherical-harmonic basis of bands 0..2 (`Y00`; `Y1m`; `Y2-2`, `Y2-1`, `Y21`; `Y20` of `3 z^2 - 1`; `Y22` of `x^2 - y^2`) and the
/// luma of a colour (Rec. 601, the weights the shader's gates were tuned with).
pub(crate) const SH_LUMA: &str = "
// the real SH basis constants (bands 0..2) and the luma of a colour (Rec. 601), `classes.rs` `SH_LUMA`
const SH_Y00: f32 = 0.282095;
const SH_Y1: f32 = 0.488603;
const SH_Y2: f32 = 1.092548;
const SH_Y20: f32 = 0.315392;
const SH_Y22: f32 = 0.546274;
const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);
";

/// The items' uniforms as the mesh shader and the shadow pass read them (`gpu::ObjectUniform`, `wgsl_structs_match_the_rust_uniforms`):
/// one text for both, put in front of each.
pub(crate) const OBJECT_WGSL: &str = "
struct Object {
    model: mat4x4<f32>,
    env_mix: vec4<f32>,
    light: vec4<f32>,
    lit_a: vec4<f32>,
    // .w ENHANCEMENT E9: what the item does with the sun's shadow, RECV_NONE / RECV_CAR / RECV_TRACK / RECV_DECAL (the class
    // constants are emitted in front of the mesh shader from `classes.rs`)
    lit_d: vec4<f32>,
    lit_dir: vec4<f32>,
    shadow_u: vec4<f32>,
    shadow_v: vec4<f32>,
    shadow_on: vec4<f32>,
    sky_u: vec4<f32>,
    sky_v: vec4<f32>,
    sky_on: vec4<f32>,
    // ENHANCEMENT E8: .x the material's reflection class (REFL_NONE, REFL_PAINT, REFL_GLASS; E17 REFL_WATER, E26 REFL_WET_ROAD /
    // REFL_SKID), .y its strength (not read), .z E16: 1 = the car ambient takes the probe, .w E26: the wetness / E49: 1 = the player's
    // own faces
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
";

/// ENHANCEMENT E8: the environment cube's face table as WGSL, emitted from `gpu::REFL_FACES` (the layer order +X, -X, +Z, -Z,
/// +Y, -Y): `face_fwd(f)` / `face_up(f)`, a face's view direction and up vector (screen right = `up x fwd`), and `face_of(d)`, the
/// face a direction falls in (its major axis; a tie goes to x, then z). The mesh shader and the coat prefilter read the faces
/// through it (and the E16 probe, `probe::source`).
pub(crate) fn faces_wgsl() -> String {
    let v = |a: [f32; 3]| format!("vec3<f32>({:?}, {:?}, {:?})", a[0], a[1], a[2]);
    let mut s = String::from("\n// the environment faces (`gpu::REFL_FACES`, `classes.rs` `faces_wgsl`)\nfn face_fwd(f: i32) -> vec3<f32> {\n    switch f {\n");
    let last = crate::REFL_FACES.len() - 1;
    for (i, (fwd, _)) in crate::REFL_FACES.iter().enumerate() {
        let case = if i == last { "default".to_string() } else { format!("case {i}") };
        s += &format!("        {case}: {{ return {}; }}\n", v(*fwd));
    }
    s += "    }\n}\nfn face_up(f: i32) -> vec3<f32> {\n    switch f {\n";
    let common = crate::REFL_FACES[0].1;
    for (i, (_, up)) in crate::REFL_FACES.iter().enumerate() {
        if *up != common {
            s += &format!("        case {i}: {{ return {}; }}\n", v(*up));
        }
    }
    s += &format!("        default: {{ return {}; }}\n    }}\n}}\n", v(common));
    s += "fn face_of(d: vec3<f32>) -> i32 {\n    let a = abs(d);\n    if a.x >= a.y && a.x >= a.z {\n        return select(0, 1, d.x < 0.0);\n    }\n    if a.z >= a.y {\n        return select(2, 3, d.z < 0.0);\n    }\n    return select(4, 5, d.y < 0.0);\n}\n";
    s
}

/// The mesh shader's globals group (group 0): each binding's number, for both sides. `shader.wgsl` declares its bindings with the
/// `B_*` names ([`wgsl_consts`] emits them); `Core::make_globals_bg` and the passes' layout entries (`gpu/csm/`, `spot.rs`,
/// `lamps.rs`, `contact.rs`) take the same constants.
pub(crate) mod binding {
    macro_rules! bindings {
        ($($(#[$doc:meta])* $name:ident = $n:literal,)*) => {
            $($(#[$doc])* pub const $name: u32 = $n;)*
            /// The WGSL names (`B_` and the Rust name).
            pub const NAMES: [(&str, u32); [$(stringify!($name)),*].len()] = [$((concat!("B_", stringify!($name)), $name)),*];
        };
    }

    bindings! {
        /// the frame's `Globals`
        GLOBALS = 0,
        /// `night.psh`'s `nght` map
        NIGHT_MAP = 1,
        /// the car shadow projector's texture and the projector / cube sampler
        PROJ_TEX = 2,
        PROJ_SAMPLER = 3,
        /// `HEAD`, the headlight decal
        HEAD_TEX = 4,
        /// ENHANCEMENT E8: the environment cube
        REFL = 5,
        /// ENHANCEMENT E9: the cascades' map, comparison sampler and uniform
        CSM_MAP = 6,
        CSM_SAMPLER = 7,
        CSM = 8,
        /// ENHANCEMENT E12: the soft sprites' depth image
        SOFT = 9,
        /// ENHANCEMENT E15: the headlight pair's map and the spots
        SPOT_MAP = 10,
        SPOTS = 11,
        /// ENHANCEMENT E16: the probe's SH
        PROBE_SH = 16,
        /// ENHANCEMENT E23: the street lamps
        LAMPS = 20,
        /// ENHANCEMENT E43: the contact shadows
        CONTACTS = 24,
        /// ENHANCEMENT E47: the lamps' shadow maps
        LAMP_SHADOW = 25,
    }
}

/// PERF (PERF-B): the mesh shader's optional parts, one bit each. A part is the code of one enhancement in `shader.wgsl`, between
/// `//@if NAME` and `//@endif` (`A|B`: either; `!A`: not); a shader built without a part has none of its code, so a GPU compiles
/// and allocates registers only for what the frame can run (`mesh_features`). Every part's code is already gated at run time by a
/// uniform that is 0 while its switch is off (the fill, `gpu/fill.rs`, and the passes that clear their uniform), so cutting it
/// changes nothing drawn; the kept parts' text is the same, line for line.
pub(crate) mod feat {
    /// One list makes the parts: `NAME = bit` gives `pub const NAME: u32 = 1 << bit`, the [`NAMES`] table `shader.wgsl`'s markers
    /// are looked up in, and (tests) [`ALL`].
    macro_rules! parts {
        ($($(#[$doc:meta])* $name:ident = $bit:literal,)*) => {
            $($(#[$doc])* pub const $name: u32 = 1 << $bit;)*
            /// The names `shader.wgsl` uses.
            pub const NAMES: [(&str, u32); [$(stringify!($name)),*].len()] = [$((stringify!($name), $name)),*];
            /// Every part.
            #[cfg(test)]
            pub const ALL: u32 = 0 $(| $name)*;
        };
    }

    parts! {
        /// ENHANCEMENT E9 (with ENH-FE3D / ENH-GARAGE, ENH-DBLSH, SHADOW-REAL): the sun's shadow cascades (`csm.dir.w` 0 without)
        CSM = 0,
        /// ENHANCEMENT E16: the car ambient probe (`globals.refl.z` 0 without)
        PROBE = 1,
        /// ENHANCEMENT E43: the contact shadow (`contacts.params.x` 0 without)
        CONTACT = 2,
        /// ENHANCEMENT E15: the headlight spots (`spots.params.x` 0 without)
        SPOT = 3,
        /// ENHANCEMENT E23: the street lamps (`lamps.params.x` 0 without)
        LAMPS = 4,
        /// ENHANCEMENT E47: the lamps' car shadows (`lamps.col[i].w` 0 without)
        LSHAD = 5,
        /// ENHANCEMENT E8 / ENH-COAT: the paint's clear coat and the glass (`globals.refl.x` 0 without)
        COAT = 6,
        /// ENHANCEMENT E17: water (`globals.water.x` 0 without)
        WATER = 7,
        /// ENHANCEMENT E26: the wet road (`globals.rain.x` 0 without)
        RAIN = 8,
        /// ENHANCEMENT E19: the fog's sun glow (`globals.fogsun.w` 0 without)
        FOGSUN = 9,
        /// ENHANCEMENT E18: particles in HDR (`object.hdr.y` / `.z` 0 without)
        PFX = 10,
        /// ENHANCEMENT E12: soft particles (`object.soft.x` 0 without)
        SOFT = 11,
        /// ENHANCEMENT E11: the sun's HDR boost (`object.hdr.x` 0 without)
        HDRB = 12,
        /// ENHANCEMENT E50: the moonlight through E9's cascades (`csm.moon.x` 0 without)
        MOON = 13,
        /// ENHANCEMENT E51: the cars-only layers of E9's near cascades (`csm.carsh.x` 0 without)
        CARSH = 14,
        /// ENHANCEMENT E49: the clear coat reads the near faces and the covered box (`globals.near.x` 0 without; inside COAT)
        NEAR = 15,
        /// ENHANCEMENT ENH-FE3D / ENH-GARAGE (SHADOW-REFACTOR): the front-end car's focus shadow (`csm.misc.y` `SHADOW_FOCUS`; a race
        /// frame never runs it)
        FOCUS = 16,
    }
    /// The parts only an item with a reflection class runs (`object.refl.x`: `gpu/fill.rs` `ItemVariant::of`)
    pub const SURFACE: u32 = COAT | WATER | RAIN | NEAR;
    /// The parts only a particle (E18's role), a soft sprite or an HDR-boosted flare runs (`object.hdr`, `.soft.x`)
    pub const EFFECT: u32 = PFX | SOFT | HDRB;
    /// Both: what an item with neither draws without
    #[cfg(test)]
    pub const PER_ITEM: u32 = SURFACE | EFFECT;
    /// What the rear-view mirror's pass never runs (`Core::no_reflections`: no cascades, contact shadows, lamps or spots, and
    /// `globals.ambient.z` 1 skips the coat)
    pub const NOT_IN_MIRROR: u32 = CSM | CONTACT | SPOT | LAMPS | LSHAD | COAT | MOON | CARSH | NEAR | FOCUS;
}

/// PERF (PERF-B): the parts of the mesh shader a set of switches can run (see [`feat`]); the original's set has none.
pub(crate) fn mesh_features(e: &crate::Enhancements) -> u32 {
    let mut f = 0;
    for (on, bit) in [
        (e.is_on(crate::E9), feat::CSM),
        (e.is_on(crate::E16) && e.is_on(crate::E8), feat::PROBE),
        (e.is_on(crate::E43), feat::CONTACT),
        (e.is_on(crate::E15), feat::SPOT),
        (e.is_on(crate::E23), feat::LAMPS),
        (e.is_on(crate::E23) && e.is_on(crate::E47), feat::LSHAD),
        (e.is_on(crate::E8), feat::COAT),
        (e.is_on(crate::E17), feat::WATER),
        (e.is_on(crate::E26), feat::RAIN),
        (e.is_on(crate::E19), feat::FOGSUN),
        (e.is_on(crate::E18), feat::PFX),
        (e.is_on(crate::E12), feat::SOFT),
        (e.is_on(crate::E11), feat::HDRB),
        (e.is_on(crate::E9) && e.is_on(crate::E50), feat::MOON),
        (e.is_on(crate::E9) && e.is_on(crate::E51), feat::CARSH),
        (e.is_on(crate::E49) && e.is_on(crate::E8), feat::NEAR),
        (e.is_on(crate::E9), feat::FOCUS),
    ] {
        if on {
            f |= bit;
        }
    }
    f
}

/// Whether a `//@if` condition holds for `feats`: names joined by `|` (any of them), or `!NAME`.
fn cond_holds(cond: &str, feats: u32) -> bool {
    let bit = |n: &str| feat::NAMES.iter().find(|(k, _)| *k == n).unwrap_or_else(|| panic!("shader.wgsl: unknown part `{n}`")).1;
    if let Some(n) = cond.strip_prefix('!') {
        return feats & bit(n.trim()) == 0;
    }
    cond.split('|').any(|n| feats & bit(n.trim()) != 0)
}

/// The mesh shader's source: the class constants, then `shader.wgsl` with the parts in `feats` ([`feat`]) and without the others.
///
/// With `feats` 0 it is the shader of the all-off path (`NFS_ENH=off`): the original's code only. (History, ENH-OFF-PIXEL: inert
/// enhancement code left in the all-off shader changed the Metal compiler's instruction order for the shared expression of
/// `shade()` — one pixel of the alps chase still differed from master's by 1/255, RE/notes/ENHANCEMENTS.md — so the original path
/// must not contain it.) The marker lines themselves are dropped.
pub(crate) fn mesh_shader_source(feats: u32) -> String {
    let src = include_str!("shader.wgsl");
    let mut kept = String::with_capacity(src.len());
    // one entry per open `//@if`: whether its lines are kept (an enclosing cut block cuts the inner ones too)
    let mut stack: Vec<bool> = Vec::new();
    for line in src.split_inclusive('\n') {
        let t = line.trim();
        if let Some(c) = t.strip_prefix("//@if ") {
            let outer = stack.last().copied().unwrap_or(true);
            stack.push(outer && cond_holds(c.trim(), feats));
        } else if t == "//@endif" {
            stack.pop().expect("shader.wgsl: //@endif without //@if");
        } else if stack.last().copied().unwrap_or(true) {
            kept.push_str(line);
        }
    }
    assert!(stack.is_empty(), "shader.wgsl: an //@if is not closed");
    // (`ign`: the front-end focus shadow's per-pixel turn, ENH-FE3D; the turns: the Vogel discs, the water's ripple)
    crate::wgsl::compose(&[&wgsl_consts(), crate::wgsl::IGN, crate::wgsl::ANGLES, crate::wgsl::BYTE, crate::wgsl::NDC_UV, crate::wgsl::LINEAR_Z, SH_LUMA, &faces_wgsl(), OBJECT_WGSL, &kept])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The emitted constants are the Rust values exactly (a float literal WGSL reads back to the same bits), each name once,
    /// and every name is used by the shader (an unused one would be a class the two sides no longer share).
    #[test]
    fn the_emitted_constants_are_the_rust_values() {
        let src = wgsl_consts();
        // (the shader with its `struct Object`, whose comments name classes)
        let shader = format!("{}{OBJECT_WGSL}", include_str!("shader.wgsl"));
        let shader = shader.as_str();
        for (name, v) in WGSL {
            let line = src.lines().find(|l| l.starts_with(&format!("const {name}:"))).unwrap_or_else(|| panic!("{name} not emitted"));
            let lit = line.rsplit("= ").next().unwrap().trim_end_matches(';');
            assert_eq!(lit.parse::<f32>().unwrap().to_bits(), v.to_bits(), "{name}");
            assert_eq!(src.matches(&format!("const {name}:")).count(), 1, "{name}");
            assert!(shader.contains(name), "{name} is not used by shader.wgsl");
            assert!(!shader.contains(&format!("const {name}")), "{name} is declared in shader.wgsl itself");
        }
    }

    /// PERF-B: with every part the shader is `shader.wgsl` as it is, only without the marker lines; every part's name is used by
    /// the shader, and each part cuts something.
    #[test]
    fn the_mesh_shader_parts() {
        let src = include_str!("shader.wgsl");
        // (the `//@if !NAME` blocks are what a shader without the part has instead: not nested, cut with every part)
        let mut unmarked = String::new();
        let mut in_not = false;
        for l in src.split_inclusive('\n') {
            let t = l.trim();
            if t.starts_with("//@if !") {
                in_not = true;
            } else if t == "//@endif" {
                in_not = false;
            } else if !t.starts_with("//@if ") && !in_not {
                unmarked.push_str(l);
            }
        }
        assert!(mesh_shader_source(feat::ALL) == crate::wgsl::compose(&[&wgsl_consts(), crate::wgsl::IGN, crate::wgsl::ANGLES, crate::wgsl::BYTE, crate::wgsl::NDC_UV, crate::wgsl::LINEAR_Z, SH_LUMA, &faces_wgsl(), OBJECT_WGSL, &unmarked]), "the full shader is not shader.wgsl");
        let all = mesh_shader_source(feat::ALL);
        for (name, bit) in feat::NAMES {
            assert!(src.contains(&format!("//@if {name}")) || src.contains(&format!("|{name}")), "{name} not used by shader.wgsl");
            // a part makes a difference to the full set or to the original's (PROBE shares its block with CSM, LSHAD sits in LAMPS)
            let without = mesh_shader_source(feat::ALL & !bit);
            assert!(without.len() < all.len() || mesh_shader_source(bit).len() > mesh_shader_source(0).len(), "{name} cuts nothing");
        }
        assert!(mesh_shader_source(0).len() < mesh_shader_source(feat::CSM).len());
        // the original's set has no part
        assert_eq!(mesh_features(&crate::Enhancements::ORIGINAL), 0);
    }

    /// The size in bytes of the mesh shader's struct `name` (every part in: the layout naga gives it, the one the GPU reads).
    pub(crate) fn mesh_struct_size(name: &str) -> u32 {
        let src = mesh_shader_source(feat::ALL);
        let module = naga::front::wgsl::parse_str(&src).expect("the mesh shader parses");
        let (_, ty) = module.types.iter().find(|(_, t)| t.name.as_deref() == Some(name)).unwrap_or_else(|| panic!("no struct {name}"));
        ty.inner.size(module.to_ctx())
    }

    /// The pieces `probe.wgsl` holds word for word (it is validated on its own, `wgsl.rs` `every_source`), so the probe, the coat
    /// prefilter and the mesh shader read one face table and one SH basis.
    #[test]
    fn the_shared_pieces_are_one() {
        let probe = crate::probe::source();
        assert!(probe.contains(SH_LUMA) && probe.contains(&faces_wgsl()), "the probe is composed of the shader's pieces");
        assert!(crate::coat::source().contains(&faces_wgsl()));
        assert!(crate::gpu::shadow_source().contains(OBJECT_WGSL) && mesh_shader_source(0).contains(OBJECT_WGSL));
    }

    /// `face_of` picks face `i` for `REFL_FACES[i]`'s own direction (the table's order is the pick's), and the emitted table is it.
    #[test]
    fn the_face_table_is_refl_faces() {
        let pick = |d: [f32; 3]| {
            let a = d.map(f32::abs);
            if a[0] >= a[1] && a[0] >= a[2] {
                return if d[0] < 0.0 { 1 } else { 0 };
            }
            if a[2] >= a[1] {
                return if d[2] < 0.0 { 3 } else { 2 };
            }
            if d[1] < 0.0 { 5 } else { 4 }
        };
        let src = faces_wgsl();
        for (i, (fwd, up)) in crate::REFL_FACES.iter().enumerate() {
            assert_eq!(pick(*fwd), i, "face {i}");
            assert!(src.contains(&format!("vec3<f32>({:?}, {:?}, {:?})", fwd[0], fwd[1], fwd[2])), "face {i}'s direction");
            assert!(src.contains(&format!("vec3<f32>({:?}, {:?}, {:?})", up[0], up[1], up[2])), "face {i}'s up");
        }
        assert_eq!(crate::REFL_FACES[4].0, [0.0, 1.0, 0.0], "`NEAR_UP_FACE` (shader.wgsl) is the +Y face");
    }

    /// The emitted sizes and tables are the Rust values (each name once), and the coat's spans are the differences of its alphas.
    #[test]
    fn the_emitted_sizes_and_tables() {
        let src = wgsl_consts();
        for (name, n) in WGSL_SIZES {
            assert_eq!(src.matches(&format!("const {name}: u32 = {n}u;")).count(), 1, "{name}");
        }
        for m in 0..crate::coat::MIP_SPAN.len() {
            let d = crate::coat::MIP_ALPHA[m + 1] - crate::coat::MIP_ALPHA[m];
            assert!((d - crate::coat::MIP_SPAN[m]).abs() < 1e-6, "mip {m}: {d} vs {}", crate::coat::MIP_SPAN[m]);
        }
        assert!(src.contains("const COAT_MIP_ALPHA: array<f32, 5> = array<f32, 5>(0.0, 0.03, 0.08, 0.2, 0.45);"));
        // every globals binding is declared by its name, once
        let shader = include_str!("shader.wgsl");
        for (name, n) in binding::NAMES {
            assert_eq!(src.matches(&format!("const {name}: u32 = {n}u;")).count(), 1, "{name}");
            assert_eq!(shader.matches(&format!("@group(0) @binding({name})")).count(), 1, "{name}");
        }
    }

    /// The values the uniform fills and the shader always used (pinned: a change here is a change of both sides).
    #[test]
    fn the_class_values_are_pinned() {
        assert_eq!([REFL_NONE, REFL_PAINT, REFL_GLASS, REFL_WATER, REFL_WET_ROAD, REFL_SKID], [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!([RECV_NONE, RECV_CAR, RECV_TRACK, RECV_DECAL], [0.0, 1.0, 2.0, 3.0]);
        assert_eq!([ROLE_OPAQUE, ROLE_EFFECT, ROLE_SKY], [0.0, 1.0, 2.0]);
        assert_eq!([SOFT_FADE_ALPHA, SOFT_FADE_BLACK, SOFT_FADE_WHITE], [0.0, 1.0, 2.0]);
        assert_eq!([PART_LIT, PART_LIT_WORLD], [2.0, 3.0]);
        assert_eq!([SHADOW_SUN, SHADOW_MOON, SHADOW_FOCUS], [0.0, 2.0, 3.0]);
        assert_eq!(feat::ALL, (1 << 17) - 1, "the parts' bits are 0..16, each once");
        assert_eq!(CASCADE_BLEND_START + CASCADE_BLEND, 1.0);
    }
}
