//! ENHANCEMENT E9: the mode of a frame's shadow cascades ([`ShadowMode`], [`CarLayers`]) and its tuning: the strengths, the
//! gate constants (FOREST-TREESH, GATE-REL), the moon's levels, the levels' map sizes.

use super::*;

/// How many cascades the maps hold at most (the shader's arrays).
pub(in crate::gpu) const MAX_CASCADES: usize = 4;
/// How strongly a shadow darkens the track's baked colour at full visibility loss (0..1): the track has
/// no separate direct light, so this stands for the sun's share of the baked brightness.
pub(in crate::gpu) const GROUND_STRENGTH: f32 = 0.6;
/// The car's floor of its direct term in the sun's shadow: never fully black, a quarter of the direct term (the sky's own
/// scatter). (Was the shader's default for a `params.w` of 0.)
pub(in crate::gpu) const SUN_CAR_FLOOR: f32 = 0.25;
/// ENHANCEMENT E50: the moonlight per level (index 1 weak, 2 normal): the share of the baked night colour a moon shadow takes at
/// most (the track), and the cold term added to the cars' light (times `MOON_COLOUR`, `n.l` and the map's visibility).
/// APPROXIMATION (enhancement content, by eye on monaco1 / industrial stills): no original counterpart.
pub(in crate::gpu) const MOON_STRENGTH: [f32; 3] = [0.0, 0.22, 0.35];
pub(in crate::gpu) const MOON_CAR: [f32; 3] = [0.0, 0.08, 0.15];
/// ENHANCEMENT E50: the moon's cold colour (a pale blue of luminance about 0.8), the soft edge's radius in texels (a 16-tap disc),
/// and the cars' floor of the direct term (`params.w`; not reached: a moon frame leaves the cars' own direct term unshadowed).
pub(in crate::gpu) const MOON_COLOUR: [f32; 3] = [0.66, 0.8, 1.0];
pub(in crate::gpu) const MOON_SOFT: f32 = 2.5;
pub(in crate::gpu) const MOON_FLOOR: f32 = 0.4;
/// ENHANCEMENT E50: the strongest moon level (`MOON_STRENGTH` / `MOON_CAR`'s last index).
pub(super) const MOON_MAX_LEVEL: u8 = 2;
/// ENHANCEMENT E51: how many of the near cascades get a cars-only layer (level 1: 0-40 and 40-140 m; a car's shadow beyond is a
/// few pixels).
pub(in crate::gpu) const CAR_CASCADES: usize = 2;
/// GATE-REL: `Csm::params.z` of a frame whose track's gate is relative to its bake light (`crate::BakedGate::Relative`; the shader
/// tests it against one half).
pub(super) const GATE_REL: f32 = 2.0;
/// GATE-REL (APPROXIMATION): the drivable ground's lit level, as a share of `BakeLight::ground` (its 90th percentile). At 1.0 a dim
/// forest road under the trees counts as baked shade and takes nearly no shadow (the trees before the covered bridge cast none, as
/// with the absolute gate; by comparing stills of the forest); 0.65 put it near the
/// ground's median (forest 1.92 x 0.65 = 1.25 against a median of 1.24). SHADOW-UNIFY (2026-11-18, by sheet): 0.80 keeps the
/// forest canopy's dappled stripes and still shades the road under the trees before the covered bridge and at the rock / bush seam.
pub(super) const GATE_REL_GROUND_SCALE: f32 = 0.80;
/// ENHANCEMENT ENH-SHPOP: where the last cascade starts to fade out, as a share of its end (was 0.85: a 33 m ramp at 220 m)
pub(super) const FADE_START: f32 = 0.6;

/// ENHANCEMENT ENH-FE3D: the front-end cascade's fade start, as a share of its split (`params.y`).
pub(super) const FOCUS_FADE_START: f32 = 0.85;
/// The levels' map sizes and cascade ends (view depth, m): level 1, then level 2 and up. ENHANCEMENT ENH-SHPOP: the last cascade
/// reaches far (the old 140 / 220 m ends made shadows pop in as they came in range).
pub(super) const LEVEL_1: (u32, &[f32]) = (1024, &[40.0, 140.0, 400.0]);
pub(super) const LEVEL_2: (u32, &[f32]) = (2048, &[25.0, 80.0, 220.0, 520.0]);

/// The level's map size, cascade ends (view depth, m).
pub(in crate::gpu) fn level_config(level: u8) -> (u32, &'static [f32]) {
    if level == 1 { LEVEL_1 } else { LEVEL_2 }
}

/// ENHANCEMENT E9: which light a frame's cascades are drawn for, and how (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::gpu) enum ShadowMode {
    /// ENHANCEMENT ENH-FE3D: a front-end car's one soft cascade ([`crate::SunShadow::focus`])
    Focus(crate::ShadowFocus),
    /// the race's sun: the sky's (E9), or a sunless track's data sun with its scenery (SUN-FULL); `casters`: which scenery parts
    /// cast (SHADOW-UNIFY: the shape rule, [`terrain_like`]); `baked_gate` (FOREST-TREESH, GATE-REL): E9's "already baked dark"
    /// gate on the track (`Relative` only with the track's bake light)
    Sun { casters: crate::SceneryCasters, baked_gate: crate::BakedGate },
    /// ENHANCEMENT E51 (CORSICA-SUN): a sunless track's data sun with the cars alone: the near cascades only, the scenery casts
    /// nothing (its shade is baked / in the decals)
    /// `small` (CANYON-SMALLSH, 2026-11-18): the small upright scenery parts (signs, posts, rails, walls, bushes: `SceneryCasters::Small`)
    /// that also cast into the same near cascades; everything else of the frame (cascades, strength, gate, floor, the cars' shadow) is
    /// the cars-only frame's own
    CarsOnly { small: Option<crate::SceneryCasters> },
    /// ENHANCEMENT E50: the moon at its level (1 weak, 2 normal: faint, its own soft edge)
    Moon { level: u8 },
}

/// SHADOW-UNIFY (2026-11-18: one shadow mode for every sun track): the sky sun's baked-dark gate is GATE-REL's relative one,
/// as the data sun's ([`ShadowMode::pick`] falls back to the absolute gate when the track has no bake light, a data rule).
/// (The comparison switch `NFS_SKY_SUN_GATE` was removed 2026-11-19, REFACTOR-0119.)
const SKY_SUN_GATE: crate::BakedGate = crate::BakedGate::Relative;

/// ENHANCEMENT E9 (SHADOW-FLOOR, 2026-11-18; APPROXIMATION, an enhancement's look): a sun frame's shadow may not darken a
/// receiver below the bake's own ambient share. The bake light's fit (`luma = a + b max(0, n.L)`, [`crate::BakeLight`] `base` /
/// `gain`) says a face turned away from the bake's light keeps `a` of a lit face's `a + b`: that is the light a shadow leaves. So
/// the frame's darkness (`params.x`, [`GROUND_STRENGTH`]: the track receivers and the cars' shadows on them, E51's cars-only frame
/// too) is `min(strength, 1 - a / (a + b))`; without a bake light (no fit, night) the strength stays. One data rule for every sun
/// track instead of one darkness for all: a bake with much ambient light (a dusk, an overcast-like bake) takes lighter shadows.
pub(super) fn floored_strength(strength: f32, bake: Option<&crate::BakeLight>) -> f32 {
    bake.filter(|b| b.base + b.gain > 1e-4).map_or(strength, |b| strength.min(1.0 - (b.base / (b.base + b.gain)).clamp(0.0, 1.0)))
}

/// GATE-REL: `Csm::bake` / `bake_k` of a frame: (toward the bake light, its base), (its gain, the ground's factor, 0, 0).
pub(super) fn bake_uniform(bake: Option<&crate::BakeLight>) -> [[f32; 4]; 2] {
    bake.map_or([[0.0; 4]; 2], |b| {
        [
            [b.to_light.x, b.to_light.y, b.to_light.z, b.base],
            [b.gain, b.ground * GATE_REL_GROUND_SCALE, 0.0, 0.0],
        ]
    })
}

impl ShadowMode {
    /// The light and the mode of a frame: the first light of `lights` that is set and whose switch is on, in
    /// [`crate::ShadowLights`]' order (the sun, the data sun, E50's moon).
    pub(super) fn pick(lights: &crate::ShadowLights, e: &crate::Enhancements) -> Option<(crate::SunShadow, Self)> {
        // GATE-REL: the relative gate needs the track's bake light
        let gate = |g: crate::BakedGate| if g == crate::BakedGate::Relative && lights.bake.is_none() { crate::BakedGate::Absolute } else { g };
        if let Some(s) = lights.sun {
            return Some((s, s.focus.map_or(Self::Sun { casters: crate::scenery_casters(), baked_gate: gate(SKY_SUN_GATE) }, Self::Focus)));
        }
        // ENHANCEMENT E51 (CORSICA-SUN): the cars-only frame with E51 on; E9 / E51 (SUN-FULL): the full frame with E9 alone, the
        // scenery casters by the frame's rule (SHADOW-UNIFY: the shape rule, as on a sky-sun track)
        if let Some(d) = lights.data_sun {
            match d.scenery {
                // CANYON-SMALLSH: the decals' sun's frame: the cars-only frame with the small parts added (with E51 on, as it)
                Some(small @ crate::SceneryCasters::Small { .. }) if e.is_on(crate::E51) => return Some((d.light, Self::CarsOnly { small: Some(small) })),
                Some(crate::SceneryCasters::Small { .. }) => {}
                Some(casters) => return Some((d.light, Self::Sun { casters, baked_gate: gate(d.baked_gate) })),
                None if e.is_on(crate::E51) => return Some((d.light, Self::CarsOnly { small: None })),
                None => {}
            }
        }
        // (SHADOW-OVERCAST: at most the track's cap, `ShadowLights::moon_level_cap`)
        let level = e.level(crate::E50).min(lights.moon_level_cap.unwrap_or(MOON_MAX_LEVEL));
        lights.moon.filter(|_| level > 0).map(|s| (s, Self::Moon { level: level.min(MOON_MAX_LEVEL) }))
    }

    /// The class the shader tests (`csm.misc.y`, `classes::SHADOW_*`).
    pub(super) fn shader_class(&self) -> f32 {
        match self {
            Self::Sun { .. } | Self::CarsOnly { .. } => classes::SHADOW_SUN,
            Self::Moon { .. } => classes::SHADOW_MOON,
            Self::Focus(_) => classes::SHADOW_FOCUS,
        }
    }

    /// `params`: x the track's strength, y where the last cascade (ending at `last`) starts to fade out, z [`GATE_REL`] when the
    /// track's baked-dark gate is relative (GATE-REL), else 0, w the cars' floor of the direct term.
    pub(super) fn params(&self, last: f32) -> [f32; 4] {
        match *self {
            Self::Sun { baked_gate: crate::BakedGate::Relative, .. } => [GROUND_STRENGTH, last * FADE_START, GATE_REL, SUN_CAR_FLOOR],
            Self::Sun { .. } | Self::CarsOnly { .. } => [GROUND_STRENGTH, last * FADE_START, 0.0, SUN_CAR_FLOOR],
            Self::Moon { level } => [MOON_STRENGTH[usize::from(level)], last * FADE_START, 0.0, MOON_FLOOR],
            // ENHANCEMENT ENH-FE3D: the front-end car's own strength
            Self::Focus(f) => [f.strength, last * FOCUS_FADE_START, 0.0, SUN_CAR_FLOOR],
        }
    }

    /// Which scenery parts cast in this mode (the moon's and the front end's frames: the default rule, [`crate::scenery_casters`]).
    pub(super) fn casters(&self) -> crate::SceneryCasters {
        match *self {
            Self::Sun { casters, .. } => casters,
            Self::CarsOnly { small: Some(small) } => small,
            _ => crate::scenery_casters(),
        }
    }

    /// ENHANCEMENT E9 / E51 (SHADOW-OVERCAST): whether the sky data's precipitation weakens this mode's shadows
    /// ([`crate::ShadowLights::precipitation`]): the race's sun and data sun, not the front end's focus or the moon
    pub(super) fn fades_with_precipitation(&self) -> bool {
        matches!(self, Self::Sun { .. } | Self::CarsOnly { .. })
    }

    /// ENHANCEMENT ENH-DBLSH: whether a track's `sh80` decals give way to this frame's shadows: the sun's (not the front end's focus,
    /// not E50's moon, whose shadows are too faint to stand in for them, not the cars-only frame: no
    /// scenery shadow stands in for the decals)
    pub(super) fn decal_fades(&self) -> bool {
        matches!(self, Self::Sun { .. })
    }

    /// ENHANCEMENT E9: whether the car's `SHAD` patch gives way to the sun's car shadow in the open (the race's sun and the cars-only
    /// frame; not the front end's focus or the moon, whose car shadow is too faint to stand in for it)
    pub(super) fn takes_car_patch(&self) -> bool {
        matches!(self, Self::Sun { .. } | Self::CarsOnly { .. })
    }
}

/// ENHANCEMENT E51: the cars-only layers of a frame (the near cascades again, with the cars alone as casters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gpu) enum CarLayers {
    /// none: E51 off, or not the race's sun
    None,
    /// CORSICA-SUN: the cars-only frame's cascades are themselves the cars-only layers (no extra layers)
    Own,
    /// after the cascades, one layer per near cascade; `sun_share` (level 1, CARSH-FADE): each car's shadow scaled by its measured
    /// share of the sun (`carsun.rs`; without compute shaders level 1 draws as level 2)
    Extra { sun_share: bool },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHADOW-FLOOR: the darkness held to the bake's ambient share; no bake light, no floor
    #[test]
    fn the_shadow_floor_follows_the_bake() {
        let b = |base: f32, gain: f32| crate::BakeLight { to_light: Vec3::Y, base, gain, ground: 1.5 };
        let forest = floored_strength(GROUND_STRENGTH, Some(&b(0.384, 0.335)));
        assert!((forest - (1.0 - 0.384 / 0.719)).abs() < 1e-6, "{forest}");
        assert_eq!(floored_strength(GROUND_STRENGTH, Some(&b(0.1, 0.9))), GROUND_STRENGTH, "a strong bake keeps the strength");
        assert_eq!(floored_strength(GROUND_STRENGTH, None), GROUND_STRENGTH);
        assert_eq!(floored_strength(GROUND_STRENGTH, Some(&b(0.0, 0.0))), GROUND_STRENGTH);
    }

    /// SHADOW-REFACTOR: the mode a set of lights and switches gives, in the lights' order (the sun, the data sun, E50), as the
    /// flags of before decided it.
    #[test]
    fn the_shadow_mode_follows_the_lights_and_switches() {
        use crate::{DataSun, Enhancements, SceneryCasters, ShadowLights, SunShadow, E50, E51};
        let s = SunShadow::race(Vec3::new(0.3, -0.8, 0.2));
        let mut e = Enhancements::ORIGINAL;
        let pick = |l: &ShadowLights, e: &Enhancements| ShadowMode::pick(l, e).map(|(_, m)| m);
        let sun = ShadowLights { sun: Some(s), data_sun: Some(DataSun { light: s, scenery: None, baked_gate: crate::BakedGate::Absolute }), ..ShadowLights::NONE };
        assert_eq!(pick(&sun, &e), Some(ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute }));
        let cars = DataSun { light: s, scenery: None, baked_gate: crate::BakedGate::Absolute };
        let sunless = ShadowLights { data_sun: Some(cars), ..ShadowLights::NONE };
        // E51 off: nothing; E51: the cars-only frame
        assert_eq!(pick(&sunless, &e), None);
        e.set_level(E51, 1);
        assert_eq!(pick(&sunless, &e), Some(ShadowMode::CarsOnly { small: None }));
        // CANYON-SMALLSH: the small parts ride on the cars-only frame (E51 on); with E51 off the frame is not drawn
        let small = SceneryCasters::Small { radius: 15.0, height: 8.0 };
        let with_small = ShadowLights { data_sun: Some(DataSun { light: s, scenery: Some(small), baked_gate: crate::BakedGate::Relative }), ..ShadowLights::NONE };
        assert_eq!(pick(&with_small, &e), Some(ShadowMode::CarsOnly { small: Some(small) }));
        e.set_level(E51, 0);
        assert_eq!(pick(&with_small, &e), None);
        e.set_level(E51, 1);
        assert_eq!(ShadowMode::CarsOnly { small: Some(small) }.params(100.0), ShadowMode::CarsOnly { small: None }.params(100.0), "the cars' shadow's strength and gate are the cars-only frame's");
        assert!(!ShadowMode::CarsOnly { small: Some(small) }.decal_fades());
        // SUN-FULL does not need E51 on; its casters' rule rides along (SHADOW-UNIFY: the shape rule)
        e.set_level(E51, 0);
        let full = ShadowLights { data_sun: Some(DataSun { light: s, scenery: Some(SceneryCasters::Shape), baked_gate: crate::BakedGate::Absolute }), ..ShadowLights::NONE };
        assert_eq!(pick(&full, &e), Some(ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute }));
        // GATE-REL: the relative gate with the track's bake light; without one it is the absolute gate
        let rel = DataSun { light: s, scenery: Some(SceneryCasters::Shape), baked_gate: crate::BakedGate::Relative };
        let bake = crate::BakeLight { to_light: Vec3::Y, base: 0.3, gain: 0.3, ground: 1.5 };
        let m = pick(&ShadowLights { data_sun: Some(rel), bake: Some(bake), ..ShadowLights::NONE }, &e).unwrap();
        assert_eq!(m, ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Relative });
        assert_eq!(m.params(100.0)[2], GATE_REL);
        let m = pick(&ShadowLights { data_sun: Some(rel), ..ShadowLights::NONE }, &e).unwrap();
        assert_eq!(m, ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute });
        let u = bake_uniform(Some(&bake));
        assert_eq!(u[1][..2], [0.3, 1.5 * GATE_REL_GROUND_SCALE]);
        // the moon at E50's level
        let night = ShadowLights { moon: Some(s), ..ShadowLights::NONE };
        assert_eq!(pick(&night, &e), None);
        e.set_level(E50, 2);
        assert_eq!(pick(&night, &e), Some(ShadowMode::Moon { level: 2 }));
        // SHADOW-OVERCAST: a track's cap holds the level down, never up, and a cap of 0 is no moon
        let capped = ShadowLights { moon_level_cap: Some(1), ..night };
        assert_eq!(pick(&capped, &e), Some(ShadowMode::Moon { level: 1 }));
        e.set_level(E50, 1);
        assert_eq!(pick(&capped, &e), Some(ShadowMode::Moon { level: 1 }));
        e.set_level(E50, 0);
        assert_eq!(pick(&capped, &e), None);
        e.set_level(E50, 2);
        assert_eq!(pick(&ShadowLights { moon_level_cap: Some(0), ..night }, &e), None);
        // which modes fade the decals and take the car's patch
        let modes = [ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute }, ShadowMode::CarsOnly { small: None }, ShadowMode::Moon { level: 1 }];
        assert_eq!(modes.map(|m| m.decal_fades()), [true, false, false]);
        assert_eq!(modes.map(|m| m.takes_car_patch()), [true, true, false]);
        // SHADOW-OVERCAST: the precipitation weakens the sun's and the data sun's shadows, not the moon's
        assert_eq!(modes.map(|m| m.fades_with_precipitation()), [true, true, false]);
    }
}
