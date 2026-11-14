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
    ("ROLE_OPAQUE", ROLE_OPAQUE),
    ("ROLE_EFFECT", ROLE_EFFECT),
    ("ROLE_SKY", ROLE_SKY),
    ("SOFT_FADE_ALPHA", SOFT_FADE_ALPHA),
    ("SOFT_FADE_BLACK", SOFT_FADE_BLACK),
    ("SOFT_FADE_WHITE", SOFT_FADE_WHITE),
    ("PART_LIT", PART_LIT),
    ("PART_LIT_WORLD", PART_LIT_WORLD),
];

/// [`WGSL`] as WGSL module-scope constants, one per line (`const REFL_WATER: f32 = 3.0;`).
pub(crate) fn wgsl_consts() -> String {
    let mut s = String::from("// the object classes, emitted from `classes.rs`\n");
    for (name, v) in WGSL {
        s += &format!("const {name}: f32 = {v:?};\n");
    }
    s
}

/// PERF (PERF-B): the mesh shader's optional parts, one bit each. A part is the code of one enhancement in `shader.wgsl`, between
/// `//@if NAME` and `//@endif` (`A|B`: either; `!A`: not); a shader built without a part has none of its code, so a GPU compiles
/// and allocates registers only for what the frame can run (`mesh_features`). Every part's code is already gated at run time by a
/// uniform that is 0 while its switch is off (the fill, `gpu/fill.rs`, and the passes that clear their uniform), so cutting it
/// changes nothing drawn; the kept parts' text is the same, line for line.
pub(crate) mod feat {
    /// ENHANCEMENT E9 (with E25, ENH-FE3D / ENH-GARAGE, ENH-DBLSH, SHADOW-REAL): the sun's shadow cascades (`csm.dir.w` 0 without)
    pub const CSM: u32 = 1 << 0;
    /// ENHANCEMENT E16: the car ambient probe (`globals.refl.z` 0 without)
    pub const PROBE: u32 = 1 << 1;
    /// ENHANCEMENT E43: the contact shadow (`contacts.params.x` 0 without)
    pub const CONTACT: u32 = 1 << 2;
    /// ENHANCEMENT E15: the headlight spots (`spots.params.x` 0 without)
    pub const SPOT: u32 = 1 << 3;
    /// ENHANCEMENT E23: the street lamps (`lamps.params.x` 0 without)
    pub const LAMPS: u32 = 1 << 4;
    /// ENHANCEMENT E47: the lamps' car shadows (`lamps.col[i].w` 0 without)
    pub const LSHAD: u32 = 1 << 5;
    /// ENHANCEMENT E8 / ENH-COAT: the paint's clear coat and the glass (`globals.refl.x` 0 without)
    pub const COAT: u32 = 1 << 6;
    /// ENHANCEMENT E17: water (`globals.water.x` 0 without)
    pub const WATER: u32 = 1 << 7;
    /// ENHANCEMENT E26: the wet road (`globals.rain.x` 0 without)
    pub const RAIN: u32 = 1 << 8;
    /// ENHANCEMENT E19: the fog's sun glow (`globals.fogsun.w` 0 without)
    pub const FOGSUN: u32 = 1 << 9;
    /// ENHANCEMENT E18: particles in HDR (`object.hdr.y` / `.z` 0 without)
    pub const PFX: u32 = 1 << 10;
    /// ENHANCEMENT E12: soft particles (`object.soft.x` 0 without)
    pub const SOFT: u32 = 1 << 11;
    /// ENHANCEMENT E11: the sun's HDR boost (`object.hdr.x` 0 without)
    pub const HDRB: u32 = 1 << 12;
    /// ENHANCEMENT E50: the moonlight through E9's cascades (`csm.moon.x` 0 without)
    pub const MOON: u32 = 1 << 13;
    /// ENHANCEMENT E51: the cars-only layers of E9's near cascades (`csm.carsh.x` 0 without)
    pub const CARSH: u32 = 1 << 14;
    /// ENHANCEMENT E49: the clear coat reads the near faces and the covered box (`globals.near.x` 0 without; inside COAT)
    pub const NEAR: u32 = 1 << 15;
    #[cfg(test)]
    pub const ALL: u32 = (1 << 16) - 1;
    /// The names `shader.wgsl` uses.
    pub const NAMES: [(&str, u32); 16] = [
        ("CSM", CSM),
        ("PROBE", PROBE),
        ("CONTACT", CONTACT),
        ("SPOT", SPOT),
        ("LAMPS", LAMPS),
        ("LSHAD", LSHAD),
        ("COAT", COAT),
        ("WATER", WATER),
        ("RAIN", RAIN),
        ("FOGSUN", FOGSUN),
        ("PFX", PFX),
        ("SOFT", SOFT),
        ("HDRB", HDRB),
        ("MOON", MOON),
        ("CARSH", CARSH),
        ("NEAR", NEAR),
    ];
    /// The parts only an item with a reflection class runs (`object.refl.x`: `gpu/fill.rs` `item_variant`)
    pub const SURFACE: u32 = COAT | WATER | RAIN | NEAR;
    /// The parts only a particle (E18's role), a soft sprite or an HDR-boosted flare runs (`object.hdr`, `.soft.x`)
    pub const EFFECT: u32 = PFX | SOFT | HDRB;
    /// Both: what an item with neither draws without
    #[cfg(test)]
    pub const PER_ITEM: u32 = SURFACE | EFFECT;
    /// What the rear-view mirror's pass never runs (`Core::no_reflections`: no cascades, contact shadows, lamps or spots, and
    /// `globals.ambient.z` 1 skips the coat)
    pub const NOT_IN_MIRROR: u32 = CSM | CONTACT | SPOT | LAMPS | LSHAD | COAT | MOON | CARSH | NEAR;
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
    crate::wgsl::compose(&[&wgsl_consts(), &kept])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emitted constants are the Rust values exactly (a float literal WGSL reads back to the same bits), each name once,
    /// and every name is used by the shader (an unused one would be a class the two sides no longer share).
    #[test]
    fn the_emitted_constants_are_the_rust_values() {
        let src = wgsl_consts();
        let shader = include_str!("shader.wgsl");
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
        assert!(mesh_shader_source(feat::ALL) == crate::wgsl::compose(&[&wgsl_consts(), &unmarked]), "the full shader is not shader.wgsl");
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

    /// The values the uniform fills and the shader always used (pinned: a change here is a change of both sides).
    #[test]
    fn the_class_values_are_pinned() {
        assert_eq!([REFL_NONE, REFL_PAINT, REFL_GLASS, REFL_WATER, REFL_WET_ROAD, REFL_SKID], [0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!([RECV_NONE, RECV_CAR, RECV_TRACK, RECV_DECAL], [0.0, 1.0, 2.0, 3.0]);
        assert_eq!([ROLE_OPAQUE, ROLE_EFFECT, ROLE_SKY], [0.0, 1.0, 2.0]);
        assert_eq!([SOFT_FADE_ALPHA, SOFT_FADE_BLACK, SOFT_FADE_WHITE], [0.0, 1.0, 2.0]);
        assert_eq!([PART_LIT, PART_LIT_WORLD], [2.0, 3.0]);
    }
}
