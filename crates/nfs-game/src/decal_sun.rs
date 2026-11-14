//! ENHANCEMENT E51 (CORSICA-SUN): the sun a track's ground-shadow decals were drawn for, for a day track whose sky has none.
//!
//! **Why.** On four day tracks (autobahn, canyon = Corsica, foothills, forest) the sky configuration puts the sun object straight
//! overhead and off (`Sky/<track>.bin` dome offset (0, 0), `SkyBin::sun_active` false; 0x461090) and the cars' light, which the
//! race takes from it, travels straight down (0x43AC8D, `TrackWorld::sun_direction` = -Y; `RE/notes/lighting.md` section 6a).
//! E9 has no sun there. But Corsica's scenery carries 65 `sh80` ground-shadow decals (`Ltreeshade*` / `Shadow*`: the depth-biased
//! `S_Normal` blocks, `thrash.md` section 6d.3), the shade of its trees and posts laid on the road and the verges - and they lie
//! to one side of their objects, as if the track had a sun after all.
//!
//! **The rule (data, no name list).** The scenery's vertices (every non-ground, non-decal world part, every placed library prop)
//! are clustered on a 1.5 m grid (8-connected cells) into single objects (a cliff chunk's mesh carries many crossed-sheet trees and
//! shrubs, each its own cluster); an upright cluster (3-30 m tall, under 12 m across) is a tree / post. Each decal is paired with
//! the upright object nearest its footprint's centre (within the footprint grown by 12 m); the offset from the object's foot to
//! the footprint's centre is the decal's shade direction. The azimuth is the length-weighted mean of those offsets, kept only
//! when there are at least [`MIN_DECALS`] and they agree (the mean's length over the mean length, [`MIN_CONCENTRATION`]).
//!
//! **Check of the rule on a track with a sky sun:** castle's 13 decals give azimuth 136 deg; its sky sun travels at 136 deg
//! (`examples/decal_sun.rs`, which prints both). Farmland's 23 give 23 deg against its sun's -26 deg (a 17-degree-high sun:
//! shadows 3x their height, the footprint centres far from their objects; concentration 0.64). Corsica: 65 decals, 13 deg,
//! concentration 0.54 (37 of 65 within -30..60 deg). Forest has 2 decals and autobahn / foothills none: no direction there
//! (forest takes the light of its bake instead, `crate::bake_light`).
//!
//! **Elevation: not in the data.** A footprint's centre lies about `h / 2 / tan(elevation)` from an object `h` tall, but the
//! decals' `offset / h` is spread far too wide to read one (Corsica 0.03 .. 10.8, median 0.44; castle 0.48 .. 3.5 for one
//! 41-degree sun): the footprints are whole draped terrain patches, not the shade's outline. So the elevation is
//! [`ELEVATION_DEG`], castle's sky sun, the one track where the decals' sun is checked against a real one. APPROXIMATION.
//! (Tried: the castle-scaled median, 67 degrees on Corsica: the car's shadow then lies almost wholly under the body and, with
//! E43's contact shadow handing the sun's share to it, the car looked less grounded than with the original's blob.)
//!
//! Render only: nothing reads it but the renderer's E51 frame (`Scene::car_sun_shadow`).

use glam::{Mat4, Vec2, Vec3};
use nfs_assets::track::TrackGeometry;
use nfs_formats::crp::Material;
use std::collections::{HashMap, HashSet};

/// The fewest decals paired with an object for a direction (Corsica 65, castle 13, forest 2).
pub const MIN_DECALS: usize = 8;
/// The least agreement of the decals' directions (the length of their mean unit offset, weighted by length; 1 = all one way).
pub const MIN_CONCENTRATION: f32 = 0.4;
/// The elevation of the decals' sun, degrees: castle's sky sun (`Sky/Castle.bin`, 0x461090: 41 degrees), whose decals give its
/// azimuth to the degree. APPROXIMATION (the decals carry no elevation of their own, see the module doc).
pub const ELEVATION_DEG: f32 = 41.0;
/// The grid cell (m) the scenery's vertices are clustered on.
const CELL: f32 = 1.5;

/// What the decals say: the light's travel direction (unit, downward) and the evidence for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalSun {
    /// The direction the light travels (from the sun to the ground).
    pub direction: Vec3,
    /// The shade's azimuth, `atan2(x, z)` in degrees.
    pub azimuth: f32,
    /// The elevation used, degrees.
    pub elevation: f32,
    /// How many decals were paired with an object.
    pub decals: usize,
    /// Their agreement (0..1).
    pub concentration: f32,
}

/// An article's world matrix: its `tr`, or (an animated `Anqt` article with no `tr`) its animation's frame 0, as the track's
/// `WAnimBundle` item places it (`crate::world::anim_matrix`, 0.1 s a frame, looping).
fn placement(art: &nfs_assets::track::ArticleMeshes) -> Mat4 {
    match &art.animation {
        Some(t) if art.transform == Mat4::IDENTITY => crate::world::anim_matrix(t, 0.0, 0.1, true),
        _ => art.transform,
    }
}

/// The measurement of the module doc, and per paired decal its azimuth (degrees) and offset / height (for the diagnostic).
pub fn measure(geometry: &TrackGeometry, materials: &[Material]) -> (Option<DecalSun>, Vec<(f32, f32)>) {
    let is_decal = |i: u16| materials.get(i as usize).is_some_and(|m| m.texture_name().starts_with("sh80"));
    let ground = |i: u16| materials.get(i as usize).is_some_and(|m| m.flags & 0xF != 0);
    let mut decals: Vec<(Vec3, Vec3)> = Vec::new();
    let mut points: Vec<Vec3> = Vec::new();
    for art in &geometry.world {
        let Some((_, meshes)) = art.lods.first() else { continue };
        let tr = placement(art);
        for m in meshes {
            if m.mesh.vertices.is_empty() {
                continue;
            }
            if is_decal(m.material) {
                let (lo, hi) = m.mesh.vertices.iter().map(|v| tr.transform_point3(Vec3::from(v.position))).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p), h.max(p)));
                decals.push((lo, hi));
            } else if !ground(m.material) {
                points.extend(m.mesh.vertices.iter().map(|v| tr.transform_point3(Vec3::from(v.position))));
            }
        }
    }
    if decals.len() < MIN_DECALS {
        return (None, Vec::new());
    }
    for inst in &geometry.instances {
        let Some(art) = geometry.library.iter().find(|a| a.tag == inst.tag) else { continue };
        let Some((_, meshes)) = art.lods.first() else { continue };
        let tr = inst.transform * Mat4::from_scale(inst.scale);
        points.extend(meshes.iter().flat_map(|m| m.mesh.vertices.iter()).map(|v| tr.transform_point3(Vec3::from(v.position))));
    }
    let objects = upright_objects(&points);
    let mut offsets: Vec<(Vec2, f32)> = Vec::new();
    for (lo, hi) in &decals {
        let centre = Vec2::new((lo.x + hi.x) * 0.5, (lo.z + hi.z) * 0.5);
        let near = objects
            .iter()
            .filter(|o| o.0.x >= lo.x - 12.0 && o.0.x <= hi.x + 12.0 && o.0.y >= lo.z - 12.0 && o.0.y <= hi.z + 12.0 && o.1 >= lo.y - 8.0 && o.1 <= hi.y + 8.0)
            .min_by(|a, b| (a.0 - centre).length().total_cmp(&(b.0 - centre).length()));
        if let Some(o) = near {
            let off = centre - o.0;
            if off.length() > 0.3 {
                offsets.push((off, o.2));
            }
        }
    }
    let per_decal: Vec<(f32, f32)> = offsets.iter().map(|(o, h)| (o.x.atan2(o.y).to_degrees(), o.length() / h)).collect();
    if offsets.len() < MIN_DECALS {
        return (None, per_decal);
    }
    let total: f32 = offsets.iter().map(|(o, _)| o.length()).sum();
    let mean = offsets.iter().map(|(o, _)| *o).sum::<Vec2>() / total;
    let concentration = mean.length();
    if concentration < MIN_CONCENTRATION {
        return (None, per_decal);
    }
    let elevation = ELEVATION_DEG;
    let flat = mean.normalize();
    let (ce, se) = (elevation.to_radians().cos(), elevation.to_radians().sin());
    let direction = Vec3::new(flat.x * ce, -se, flat.y * ce).normalize();
    (Some(DecalSun { direction, azimuth: flat.x.atan2(flat.y).to_degrees(), elevation, decals: offsets.len(), concentration }), per_decal)
}

/// The upright objects among the scenery's points: (foot xz, foot y, height), clusters 3-30 m tall and under 12 m across.
fn upright_objects(points: &[Vec3]) -> Vec<(Vec2, f32, f32)> {
    let key = |p: Vec3| ((p.x / CELL).floor() as i32, (p.z / CELL).floor() as i32);
    // per cell: lowest y, highest y, xz sum, count
    let mut cells: HashMap<(i32, i32), (f32, f32, Vec2, u32)> = HashMap::new();
    for p in points {
        let e = cells.entry(key(*p)).or_insert((f32::MAX, f32::MIN, Vec2::ZERO, 0));
        e.0 = e.0.min(p.y);
        e.1 = e.1.max(p.y);
        e.2 += Vec2::new(p.x, p.z);
        e.3 += 1;
    }
    // a fixed visiting order (the map's own is random per process): the result must not depend on it
    let mut keys: Vec<(i32, i32)> = cells.keys().copied().collect();
    keys.sort_unstable();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for k in keys {
        if !seen.insert(k) {
            continue;
        }
        let mut stack = vec![k];
        let (mut lo, mut hi, mut sum, mut n) = (f32::MAX, f32::MIN, Vec2::ZERO, 0u32);
        let (mut bmin, mut bmax) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
        while let Some(c) = stack.pop() {
            let e = cells[&c];
            lo = lo.min(e.0);
            hi = hi.max(e.1);
            sum += e.2;
            n += e.3;
            bmin = (bmin.0.min(c.0), bmin.1.min(c.1));
            bmax = (bmax.0.max(c.0), bmax.1.max(c.1));
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let q = (c.0 + dx, c.1 + dz);
                    if cells.contains_key(&q) && seen.insert(q) {
                        stack.push(q);
                    }
                }
            }
        }
        let wide = ((bmax.0 - bmin.0).max(bmax.1 - bmin.1) + 1) as f32 * CELL;
        if (3.0..30.0).contains(&(hi - lo)) && wide < 12.0 {
            out.push((sum / n as f32, lo, hi - lo));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cluster_is_one_upright_object() {
        // a 10 m post of points at (5, 0, 5) and a flat sheet far away
        let mut p: Vec<Vec3> = (0..20).map(|i| Vec3::new(5.0, i as f32 * 0.5, 5.0)).collect();
        p.extend((0..10).map(|i| Vec3::new(100.0 + i as f32, 0.0, 0.0)));
        let o = upright_objects(&p);
        assert_eq!(o.len(), 1);
        assert!((o[0].0 - Vec2::new(5.0, 5.0)).length() < 1e-4 && o[0].1 == 0.0 && (o[0].2 - 9.5).abs() < 1e-4);
    }

    /// With the game data: Corsica's decals give a sun (about 13 degrees azimuth), castle's agree with its sky sun, and the
    /// tracks with too few decals give none. Skips without the data.
    #[test]
    fn the_decals_give_corsicas_sun() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else { return };
        let run = |name: &str| {
            let t = nfs_assets::track::TrackAssets::load(&paths, name).unwrap();
            measure(&t.build_geometry().unwrap(), &t.materials().unwrap()).0
        };
        let c = run("canyon").expect("Corsica's decals give a sun");
        assert!((5.0..25.0).contains(&c.azimuth) && c.decals >= 50, "{c:?}");
        let k = run("castle").expect("castle's decals give a sun");
        assert!((k.azimuth - 136.0).abs() < 10.0, "{k:?}");
        assert!(run("forest").is_none());
    }
}
