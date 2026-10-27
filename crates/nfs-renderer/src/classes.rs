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

/// The mesh shader's source: the class constants, then `shader.wgsl`.
///
/// `full` = false is the shader of the all-off path (`NFS_ENH=off`): the lines between `//@enh-only-begin` and
/// `//@enh-only-end` (the ENH-DBLSH decal fade, the E26 wet-road block and the E9 / E16 per-pixel light layer, all inert at level 0) are cut. Left in, they
/// change the Metal compiler's instruction order for the shared expression of `shade()` (one pixel of the alps chase still,
/// (750, 102), differed from master's by 1/255; RE/notes/ENHANCEMENTS.md, ENH-OFF-PIXEL), so the original path must not
/// contain them. With any switch on, `full` = true keeps them.
pub(crate) fn mesh_shader_source(full: bool) -> String {
    let src = include_str!("shader.wgsl");
    if full {
        return crate::wgsl::compose(&[&wgsl_consts(), src]);
    }
    let mut kept = String::with_capacity(src.len());
    let mut cut = false;
    for line in src.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("//@enh-only-begin") {
            cut = true;
        } else if t == "//@enh-only-end" {
            cut = false;
        } else if !cut {
            kept.push_str(line);
        }
    }
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
