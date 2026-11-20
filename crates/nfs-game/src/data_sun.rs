//! ENHANCEMENT E51 / E9 (CORSICA-SUN, SUN-ALL, SUN-FULL): the sun of a day track whose sky has none, from the track's data, and
//! what follows from it, by DATA RULES (SHADOW-UNIFY: no per-track table; formerly a `SUNLESS_DAY_TRACKS` list with
//! a choice per track name, and before that three name lists in three files).
//!
//! The four sunless day tracks (`canyon`, `forest`, `foothills`, `autobahn`) have the sky's sun object straight overhead and off,
//! so the cars' light travels straight down (0x43AC8D, `TrackWorld::sun_direction` = -Y) and E9 has no sun. The sun comes from
//! the data: the light its `sh80` ground-shadow decals were drawn for ([`crate::decal_sun`]: Corsica), else the light its baked
//! vertex colours were lit with when that bake is clearly directional ([`crate::bake_light`]: Schwarzwald). Nothing else: a track
//! whose data gives no direction has no data sun. Render only: nothing reads it but the renderer's shadow frame and the lamps.
//!
//! SHADOW-OVERCAST: foothills (Pyrenees) and autobahn have NO data sun. Earlier (SUN-ALL) they
//! took the bake's best fit below the thresholds (a per-track `weak_fit` flag: foothills 144 / 63 deg, r2 0.49 vs 0.34 overhead;
//! autobahn -135 / 78 deg, r2 0.48 vs 0.47), which drew a sharp car shadow beside the car 29-58 % dark under a twilight sky
//! (foothills: the bake has no direction at all, every upright azimuth bin 0.442) and under rain (autobahn). Their shadows are
//! E43's contact shadow, E14's AO and E47's lamp shadows. With no track left on it the weak-fit mechanism is gone.
//!
//! SHADOW-UNIFY (one shadow mode for every sun track, no per-track cases). APPROXIMATION (enhancement rules,
//! nothing of the original's: it has no sun there):
//! * a data sun's full frame is the sky sun's frame - the same casters' rule (`nfs_renderer::SceneryCasters::Shape`) and the
//!   same receivers' gate (`BakedGate::Relative`);
//! * FULL OR CARS-ONLY by the sun's source ([`SunSource`]): a sun measured from the track's own `sh80` ground-shadow decals takes
//!   E51's cars-only frame plus the small upright parts as casters (CANYON-SMALLSH, [`small_parts`]; the big scenery's shade is
//!   already drawn into the data as decals: Corsica, whose gorge walls shade the
//!   whole road on the full frame), a sun measured from the bake takes E9's full frame (a vertex bake has no shade of the
//!   trees, posts, houses: Schwarzwald). On the shipped tracks this gives the former per-track assignment (canyon cars-only, forest
//!   full); foothills and autobahn have no data sun at all;
//! * DUSK LAMPS ([`dusk_lamps`]): a day track with no sun from any source (no sky sun, no data sun) lights its open street lamps
//!   by day (E23, E47's lamp shadows): autobahn (AUTOBAHN-LAMPS) and foothills (8 open street lamps).

use glam::Vec3;
use nfs_assets::track::TrackGeometry;
pub use nfs_renderer::{BakedGate, SceneryCasters};
use nfs_formats::crp::Material;

/// Where a data sun was measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SunSource {
    /// the light the track's `sh80` ground-shadow decals were drawn for ([`crate::decal_sun`])
    Decals,
    /// the light the track's vertex colours were baked with ([`crate::bake_light`])
    Bake,
}

/// A sunless day track's sun from its data ([`measure`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataSun {
    /// The light's travel direction (unit, downward).
    pub direction: Vec3,
    /// where it was measured
    pub source: SunSource,
    /// `None`: E51's cars-only frame (only the cars cast); `Some(Small{..})` (CANYON-SMALLSH): the same frame with the small upright
    /// scenery parts casting too (the decals' sun); `Some(rule)` (SUN-FULL): E9's full frame along it, the scenery casting
    /// by `rule` (the sky sun's shape rule; a radius only by test glue). By the source ([`full_frame`], [`small_parts`]).
    pub scenery: Option<SceneryCasters>,
    /// With `scenery`: E9's "already baked dark" gate on the track: SHADOW-UNIFY: `Relative` as on every sun
    /// frame (the renderer falls back to `Absolute` when the track has no bake light); another only by test glue ([`scenery`]).
    pub baked_gate: BakedGate,
}

/// Whether the sky's sun travels straight down (the sunless day tracks' sky: its sun object overhead and off, 0x43AC8D).
fn straight_down(sun_direction: Vec3) -> bool {
    sun_direction.x.abs() + sun_direction.z.abs() <= 1e-3
}

/// The data sun of a track whose sky's sun travels `sun_direction` (`None` unless that is straight down and the data gives one).
pub fn measure(track: &nfs_assets::track::TrackAssets, geometry: &TrackGeometry, materials: &[Material], sun_direction: Vec3) -> Option<DataSun> {
    if !straight_down(sun_direction) {
        return None;
    }
    let m = crate::decal_sun::measure(geometry, materials).0;
    tracing::info!(?m, "the decals' sun (ENHANCEMENT E51)");
    let (direction, source) = m
        .map(|m| (m.direction, SunSource::Decals))
        .or_else(|| {
            let b = (!track.is_night()).then(|| crate::bake_light::fit(geometry, materials)).flatten();
            tracing::info!(?b, "the bake's light (ENHANCEMENT E51)");
            b.and_then(|b| b.accepted()).map(|d| (d, SunSource::Bake))
        })
        // TEST GLUE: `NFS_DATA_SUN=<azimuth>,<elevation>` (degrees, `bake_light::travel`) sets this sun by hand, for comparison stills
        // (its source stays the data's; a track without one counts as a bake sun)
        .map_or_else(|| test_glue_data_sun().map(|d| (d, SunSource::Bake)), |(d, s)| Some((test_glue_data_sun().unwrap_or(d), s)))?;
    let (scenery, baked_gate) = scenery(&track.name, source).map_or((None, BakedGate::Relative), |(r, g)| (Some(r), g));
    let sun = DataSun { direction, source, scenery, baked_gate };
    tracing::info!(?source, full = ?sun.scenery, ?baked_gate, "the data sun's frame (ENHANCEMENT E9 full / E51 cars-only)");
    Some(sun)
}

/// SHADOW-UNIFY: whether a data sun of `source` drives E9's full frame (a bake's sun) or E51's cars-only frame (the decals' sun:
/// the scenery's shade is drawn into the data already). See the module doc.
pub fn full_frame(source: SunSource) -> bool {
    source == SunSource::Bake
}

/// SHADOW-UNIFY (AUTOBAHN-LAMPS generalised): whether a track lights its open street lamps (E23) and their cars' shadows (E47)
/// by day: a day track with no sun from any source - its sky's sun straight down (none) and no data sun. APPROXIMATION
/// (enhancement content): the data has no dusk flag (autobahn is a day track by the original's own test: no headlight layers,
/// its cars drive unlit, which stays so); a sunless, data-sunless day is the overcast / rainy dusk the lamps stand for.
pub fn dusk_lamps(night: bool, sun_direction: Vec3, data_sun: Option<&DataSun>) -> bool {
    !night && straight_down(sun_direction) && data_sun.is_none()
}

/// The full E9 frame's casters' rule and baked-dark gate for `track` along its data sun of `source` ([`full_frame`]: the sky
/// sun's rule, `nfs_renderer::scenery_casters`, and the relative gate), `None` = cars-only. (The comparison switch
/// `NFS_DATA_SUN_FULL` was removed 2026-11-19, REFACTOR-0119.)
fn scenery(_track: &str, source: SunSource) -> Option<(SceneryCasters, BakedGate)> {
    if full_frame(source) { Some((nfs_renderer::scenery_casters(), BakedGate::Relative)) } else { small_parts(source) }
}

/// CANYON-SMALLSH: a decals' sun's frame is E51's cars-only frame plus the small upright scenery parts as casters
/// ([`SMALL_RADIUS`] / [`SMALL_HEIGHT`], `SceneryCasters::Small`). The `sh80` decals are whole draped terrain patches under tree groups,
/// so the data has no shade of its own for signs, posts, rails, roadside walls or bushes (canyon's audit, the removed example `canyon_smallsh`, see the history:
/// of 134 signs, 20 rails, 29 walls and 610 bushes none has a darker bake beside it in the sun's direction and only a few lie in a
/// decal's patch); the gorge walls and cliffs (broad or tall) still do not cast, the walls' shade being baked / in the decals. The
/// cars' shadow is the cars-only frame's own (`gpu/csm/mode.rs` `ShadowMode::CarsOnly { small }`). The gate rides along unused (that frame
/// has no relative gate). (The tuning switch `NFS_CARS_ONLY_SMALL` was removed 2026-11-19, REFACTOR-0119.)
fn small_parts(source: SunSource) -> Option<(SceneryCasters, BakedGate)> {
    if source != SunSource::Decals {
        return None;
    }
    Some((SceneryCasters::Small { radius: SMALL_RADIUS, height: SMALL_HEIGHT }, BakedGate::Relative))
}

/// The small parts' bounding radius (m) and height (m, mesh space) limits (CANYON-SMALLSH).
pub const SMALL_RADIUS: f32 = 15.0;
pub const SMALL_HEIGHT: f32 = 8.0;

/// TEST GLUE: `NFS_DATA_SUN=<azimuth>,<elevation>` — a hand-set sun for a sunless day track (comparison stills only).
fn test_glue_data_sun() -> Option<Vec3> {
    let v = crate::config::cfg().nfs_data_sun.as_str()?;
    let (a, e) = v.split_once(',')?;
    Some(crate::bake_light::travel(a.trim().parse().ok()?, e.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHADOW-UNIFY: the full frame by the sun's source (as before the unification: forest's bake sun full, canyon's decal sun
    /// cars-only)
    #[test]
    fn the_frame_follows_the_suns_source() {
        assert!(full_frame(SunSource::Bake));
        assert!(!full_frame(SunSource::Decals));
        assert_eq!(scenery("forest", SunSource::Bake), Some((SceneryCasters::Shape, BakedGate::Relative)));
        // the decals' sun: the cars-only frame with the small parts as casters (CANYON-SMALLSH)
        assert_eq!(scenery("canyon", SunSource::Decals), Some((SceneryCasters::Small { radius: SMALL_RADIUS, height: SMALL_HEIGHT }, BakedGate::Relative)));
    }

    /// SHADOW-UNIFY: the dusk lamps on a day track with no sun of any kind
    #[test]
    fn the_dusk_lamps_need_a_sunless_day() {
        let down = Vec3::NEG_Y;
        let sun = DataSun { direction: Vec3::new(0.3, -0.8, 0.2).normalize(), source: SunSource::Bake, scenery: None, baked_gate: BakedGate::Relative };
        assert!(dusk_lamps(false, down, None), "autobahn, foothills");
        assert!(!dusk_lamps(false, down, Some(&sun)), "forest, canyon");
        assert!(!dusk_lamps(false, Vec3::new(0.4, -0.8, 0.1), None), "a sky sun");
        assert!(!dusk_lamps(true, down, None), "a night track lights them anyway, not by this rule");
    }
}
