//! ENHANCEMENT E51 / E9 (CORSICA-SUN, SUN-ALL, SUN-FULL): the sun of a day track whose sky has none, from the track's data, and
//! the per-track choices for those tracks in ONE table ([`SUNLESS_DAY_TRACKS`], SHADOW-REFACTOR: before, three name lists
//! in three files: `bake_light::WEAK_FIT_TRACKS`, `world::DATA_SUN_FULL_TRACKS`, `street_lamps::DUSK_TRACKS`).
//!
//! The four sunless day tracks (`canyon`, `forest`, `foothills`, `autobahn`) have the sky's sun object straight overhead and off,
//! so the cars' light travels straight down (0x43AC8D, `TrackWorld::sun_direction` = -Y) and E9 has no sun. The sun comes from
//! the data: the light its `sh80` ground-shadow decals were drawn for ([`crate::decal_sun`]: Corsica), else the light its baked
//! vertex colours were lit with when that bake is clearly directional ([`crate::bake_light`]: Schwarzwald). Nothing else: a track
//! whose data gives no direction has no data sun. Render only: nothing reads it but the renderer's shadow frame.
//!
//! SHADOW-OVERCAST (2026-11-16): foothills (Pyrenees) and autobahn have NO data sun. Until then (SUN-ALL, 2026-11-14) they
//! took the bake's best fit below the thresholds (a per-track `weak_fit` flag: foothills 144 / 63 deg, r2 0.49 vs 0.34 overhead;
//! autobahn -135 / 78 deg, r2 0.48 vs 0.47), which drew a sharp car shadow beside the car 29-58 % dark under a twilight sky
//! (foothills: the bake has no direction at all, every upright azimuth bin 0.442) and under rain (autobahn). Their shadows are
//! E43's contact shadow, E14's AO and on autobahn E47's lamp shadows. With no track left on it the weak-fit mechanism is gone.

use glam::Vec3;
use nfs_assets::track::TrackGeometry;
use nfs_formats::crp::Material;

/// A sunless day track's sun from its data ([`measure`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataSun {
    /// The light's travel direction (unit, downward).
    pub direction: Vec3,
    /// `None`: E51's cars-only frame (only the cars cast); `Some(r)` (SUN-FULL): E9's full frame along it, the scenery casting up
    /// to a bounding radius of `r` m ([`SunlessTrack::scenery_radius`]).
    pub scenery_radius: Option<f32>,
}

/// One sunless day track's choices (2026-11-13 / 14 / 16; APPROXIMATION: enhancement content, nothing of the
/// original's: it has no sun there).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunlessTrack {
    /// the track's name (`TrackAssets::name`)
    pub name: &'static str,
    /// SUN-FULL (2026-11-14): `Some(r)`: the data sun drives E9's FULL frame (the scenery's shadows too: signs, posts, trees, fences
    /// and houses have no shade of their own in a vertex bake; E9's "baked dark" gate keeps the bake's own shade from being
    /// darkened twice, as on alps), the scenery casting up to a bounding radius of `r` m (E9's own limit is 60). Forest 15 m
    /// (APPROXIMATION, by eye on stills at 8 / 15 / 30 / 60 m: with 60 the slopes and the cliff (parts of 20-60 m) throw a
    /// 27-degree sun's shade over nearly the whole road, which the bake lights evenly - a contradiction, not a doubling; with 8 the
    /// trees beside the road cast nothing; 15 keeps the trees, signs, posts, fences and the covered bridge, 30 looks like 15 with
    /// more slope shade). `None`: E51's cars-only frame (canyon, foothills, autobahn: pending a look).
    pub scenery_radius: Option<f32>,
    /// AUTOBAHN-LAMPS (2026-11-13): the open street lamps light (E23) and cast the cars' shadows (E47) by day, as on a night track.
    /// The data gives no rule: autobahn is a day track by the original's own test (no headlight layers: its cars drive unlit,
    /// which stays so) and neither the sky colour, the `.map` light nor the road's baked colour sets its rainy dusk apart from
    /// the other day tracks (`ENHANCEMENTS.md` E47, AUTOBAHN-LAMPS).
    pub dusk_lamps: bool,
}

/// The sunless day tracks with a choice of their own (see [`SunlessTrack`]; foothills has none since SHADOW-OVERCAST, autobahn
/// only its lamps).
pub const SUNLESS_DAY_TRACKS: [SunlessTrack; 3] = [
    SunlessTrack { name: "canyon", scenery_radius: None, dusk_lamps: false },
    SunlessTrack { name: "forest", scenery_radius: Some(15.0), dusk_lamps: false },
    SunlessTrack { name: "autobahn", scenery_radius: None, dusk_lamps: true },
];

/// SUN-FULL's caster radius when a test glue list gives a track without one (m: E9's own limit).
const TEST_GLUE_RADIUS: f32 = 60.0;

/// `track`'s row of [`SUNLESS_DAY_TRACKS`], if it has one.
pub fn sunless_track(track: &str) -> Option<&'static SunlessTrack> {
    SUNLESS_DAY_TRACKS.iter().find(|t| t.name.eq_ignore_ascii_case(track))
}

/// The data sun of a track whose sky's sun travels `sun_direction` (`None` unless that is straight down and the data gives one).
pub fn measure(track: &nfs_assets::track::TrackAssets, geometry: &TrackGeometry, materials: &[Material], sun_direction: Vec3) -> Option<DataSun> {
    if sun_direction.x.abs() + sun_direction.z.abs() > 1e-3 {
        return None;
    }
    let m = crate::decal_sun::measure(geometry, materials).0;
    tracing::info!(?m, "the decals' sun (ENHANCEMENT E51)");
    let direction = m
        .map(|m| m.direction)
        .or_else(|| {
            let b = (!track.is_night()).then(|| crate::bake_light::fit(geometry, materials)).flatten();
            tracing::info!(?b, "the bake's light (ENHANCEMENT E51)");
            b.and_then(|b| b.accepted())
        })
        // TEST GLUE: `NFS_DATA_SUN=<azimuth>,<elevation>` (degrees, `bake_light::travel`) sets this sun by hand, for comparison stills
        .map_or_else(test_glue_data_sun, |d| Some(test_glue_data_sun().unwrap_or(d)))?;
    let sun = DataSun { direction, scenery_radius: scenery_radius(&track.name) };
    tracing::info!(full = ?sun.scenery_radius, "the data sun's frame (ENHANCEMENT E9 full / E51 cars-only)");
    Some(sun)
}

/// The full E9 frame's caster radius for `track` along its data sun ([`SunlessTrack::scenery_radius`]), `None` = cars-only.
/// TEST GLUE: `NFS_DATA_SUN_FULL=<track>[:<radius>],..` replaces the table's radii (`none` = none; no radius = 60), for comparison
/// stills.
fn scenery_radius(track: &str) -> Option<f32> {
    match crate::config::cfg().nfs_data_sun_full.as_str() {
        Some(list) => list.split(',').find_map(|e| {
            let (t, r) = e.trim().split_once(':').unwrap_or((e.trim(), ""));
            t.eq_ignore_ascii_case(track).then(|| r.parse().unwrap_or(TEST_GLUE_RADIUS))
        }),
        None => sunless_track(track).and_then(|t| t.scenery_radius),
    }
}

/// TEST GLUE: `NFS_DATA_SUN=<azimuth>,<elevation>` — a hand-set sun for a sunless day track (comparison stills only).
fn test_glue_data_sun() -> Option<Vec3> {
    let v = crate::config::cfg().nfs_data_sun.as_str()?;
    let (a, e) = v.split_once(',')?;
    Some(crate::bake_light::travel(a.trim().parse().ok()?, e.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// every row is a different track, each scenery radius within E9's own 60 m caster limit
    #[test]
    fn the_sunless_tracks_table() {
        for (i, t) in SUNLESS_DAY_TRACKS.iter().enumerate() {
            assert!(SUNLESS_DAY_TRACKS[..i].iter().all(|u| u.name != t.name), "{} twice", t.name);
            assert!(t.scenery_radius.is_none_or(|r| r > 0.0 && r <= TEST_GLUE_RADIUS), "{} {:?}", t.name, t.scenery_radius);
        }
        // the choices as recorded (2026-11-13 / 14 / 16; SHADOW-OVERCAST: foothills has no row, no data sun)
        let pick = |f: fn(&SunlessTrack) -> bool| SUNLESS_DAY_TRACKS.iter().filter(|t| f(t)).map(|t| t.name).collect::<Vec<_>>();
        assert!(sunless_track("foothills").is_none());
        assert_eq!(pick(|t| t.scenery_radius.is_some()), ["forest"]);
        assert_eq!(pick(|t| t.dusk_lamps), ["autobahn"]);
        assert_eq!(sunless_track("FOREST").and_then(|t| t.scenery_radius), Some(15.0));
        assert!(sunless_track("alps").is_none());
    }
}
