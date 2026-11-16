//! ENHANCEMENT E51 (SUN-ALL): the light a track's baked vertex colours were lit with, for a day track whose sky has no sun and
//! whose `sh80` decals give none ([`crate::decal_sun`]).
//!
//! **Why.** Schwarzwald (`forest`), Pyrénées (`foothills`) and `autobahn` have the sun object straight overhead and off
//! (`Sky/<track>.bin` dome offset (0, 0), `SkyBin::sun_active` false, 0x461090), so the cars' light travels straight down
//! (0x43AC8D, `TrackWorld::sun_direction` = -Y) and E9 has no sun; forest has 2 `sh80` decals and the other two none, so
//! `decal_sun` gives no direction either. What is left in the data is the scenery's own lighting: the vertex colours the
//! tracks were shipped with are a bake, and a bake made with a directional light leaves its direction in them.
//!
//! **The rule (data, no name list).** Every face of the track's scenery (world parts whose material is not drivable ground,
//! `mt` `flags & 0xF` == 0, and not an `sh80` decal; first LOD) gives its outward normal (the side its stored vertex normals
//! point to) and the luma of its mean vertex colour. Faces are binned by normal ([`AZ_BIN`] degrees of azimuth x
//! [`NY_BIN`] of `n.y`, bins of at least [`MIN_BIN`] faces); a bin's value is its median luma, so the bin speaks for the
//! shading of that orientation and not for one texture, one cast shadow or one painted patch. Over the hemisphere (3-degree
//! grid) the light `L` is the one for which `luma = a + b max(0, n.L)`, `b > 0`, fits the bins best (weighted least squares,
//! weight `sqrt(faces)`; `r2` the fit's coefficient of determination). Taken only when the bake is clearly directional:
//! `r2` at least [`MIN_R2`] and at least [`MIN_GAIN`] over the same fit with the light straight overhead (a bake of a sky /
//! overhead light fits that one as well).
//!
//! **Validation (`examples/bake_light.rs`, every track).** Two independent cues agree: the Lambert fit above, and the cast
//! shadows in the ground's bake ([`cast_contrast`]: the ground faces that face `L` but are occluded towards it are darker
//! than the open ones by the most at the bake's light). Azimuth of the light's travel, Lambert / cast (elevation 30-40):
//! castle 141 / 128-150, alps -57 / -41..-50, coastal -15 / -12, farmland -84 / -98, forest -162 / 165-170, autobahn
//! -135 / -117..-136, foothills 144 / 110. **But the bake's light is not the sky's sun** on three of the four tracks that
//! have one: castle's sky sun travels at 136 deg / 41 deg up (the bake 141 / 63: a match), alps' at 6 / 44 (bake -57 / 54),
//! coastal's at -81 / 3 (dusk; bake -15 / 39), farmland's at -26 / 17 (bake -84 / 69). The artists' bake light and the
//! sky sun (the cars' light) were set apart there; on a sky-sun track E9 keeps the sky's (the cars are lit by it).
//! So what this gives is the light the scenery LOOKS lit by - on a track whose sky has none, the one direction in the data.
//!
//! **Which tracks pass** (r2 / overhead r2): forest 0.74 / 0.06 (the most directional bake of all the tracks: taken);
//! foothills 0.49 / 0.34 and autobahn 0.48 / 0.46 (a near-overhead bake, the cast cue 40 degrees off on foothills: none);
//! canyon 0.20 / 0.17 (none; its decals give the sun, `decal_sun`). With the sky-sun tracks for comparison: castle
//! 0.81 / 0.59, coastal 0.84 / 0.23, alps 0.45 / 0.21, farmland 0.56 / 0.47.
//!
//! **Elevation** is the fit's (forest 27 degrees; the bins' 80th-percentile luma instead of the median gives 42: the
//! elevation is the loosest part of the fit, APPROXIMATION).
//!
//! Render only: nothing reads it but the renderer's shadow frame (`ShadowLights::data_sun`, via `crate::data_sun`), like `decal_sun`'s.

use glam::{Vec2, Vec3};
use nfs_assets::track::TrackGeometry;
use nfs_formats::crp::Material;
use std::collections::HashMap;

/// The least fit (coefficient of determination over the normal bins) of a directional bake.
pub const MIN_R2: f32 = 0.6;
/// The least gain of the fit over the same fit with the light straight overhead.
pub const MIN_GAIN: f32 = 0.3;
/// A normal bin: degrees of azimuth.
const AZ_BIN: f32 = 15.0;
/// A normal bin: steps of `n.y`.
const NY_BIN: f32 = 0.1;
/// The fewest faces in a bin.
const MIN_BIN: usize = 20;

/// What the bake says.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BakeFit {
    /// The light's travel direction (unit, downward) of the best fit.
    pub direction: Vec3,
    /// Its azimuth, `atan2(x, z)` of the travel, degrees.
    pub azimuth: f32,
    /// Its elevation over the horizon, degrees.
    pub elevation: f32,
    /// The fit's coefficient of determination.
    pub r2: f32,
    /// The same fit with the light straight overhead.
    pub r2_overhead: f32,
    /// The normal bins fitted.
    pub bins: usize,
    /// The scenery faces binned.
    pub faces: usize,
}

impl BakeFit {
    /// The light, when the bake is clearly directional (module doc).
    pub fn accepted(&self) -> Option<Vec3> {
        (self.r2 >= MIN_R2 && self.r2 - self.r2_overhead >= MIN_GAIN).then_some(self.direction)
    }

}

/// The light's travel direction for an azimuth / elevation in degrees.
pub fn travel(azimuth: f32, elevation: f32) -> Vec3 {
    let (a, e) = (azimuth.to_radians(), elevation.to_radians());
    Vec3::new(a.sin() * e.cos(), -e.sin(), a.cos() * e.cos())
}

/// One face: outward normal, luma of the mean vertex colour, centroid.
struct Face {
    n: Vec3,
    luma: f32,
    centre: Vec3,
}

/// The track's faces: (scenery, ground) - first LOD of the world parts, `sh80` decals left out.
fn faces(geometry: &TrackGeometry, materials: &[Material], occluders: bool) -> (Vec<Face>, Vec<Face>, Vec<[Vec3; 3]>) {
    let is_decal = |i: u16| materials.get(i as usize).is_some_and(|m| m.texture_name().starts_with("sh80"));
    let ground = |i: u16| materials.get(i as usize).is_some_and(|m| m.flags & 0xF != 0);
    let (mut scenery, mut floor, mut tris) = (Vec::new(), Vec::new(), Vec::new());
    for art in &geometry.world {
        let Some((_, meshes)) = art.lods.first() else { continue };
        for m in meshes {
            if is_decal(m.material) {
                continue;
            }
            let gr = ground(m.material);
            let v = &m.mesh.vertices;
            for tri in m.mesh.indices.chunks_exact(3) {
                let p = [0, 1, 2].map(|k| art.transform.transform_point3(Vec3::from(v[tri[k] as usize].position)));
                let c = (p[1] - p[0]).cross(p[2] - p[0]);
                if c.length() < 2e-4 {
                    continue;
                }
                if occluders {
                    tris.push(p);
                }
                let mut n = c.normalize();
                let stored: Vec3 = tri.iter().map(|&i| Vec3::from(v[i as usize].normal)).sum();
                if (stored.length() > 1e-3 && n.dot(stored) < 0.0) || (stored.length() <= 1e-3 && gr && n.y < 0.0) {
                    n = -n;
                }
                let col: [f32; 3] = [0, 1, 2].map(|k| tri.iter().map(|&i| v[i as usize].color[k]).sum::<f32>() / 3.0);
                let f = Face { n, luma: 0.299 * col[0] + 0.587 * col[1] + 0.114 * col[2], centre: (p[0] + p[1] + p[2]) / 3.0 };
                if gr { floor.push(f) } else { scenery.push(f) }
            }
        }
    }
    for inst in geometry.instances.iter().filter(|_| occluders) {
        let Some(art) = geometry.library.iter().find(|a| a.tag == inst.tag) else { continue };
        let Some((_, meshes)) = art.lods.first() else { continue };
        let tr = inst.transform * glam::Mat4::from_scale(inst.scale);
        for m in meshes.iter().filter(|m| !is_decal(m.material)) {
            for tri in m.mesh.indices.chunks_exact(3) {
                tris.push([0, 1, 2].map(|k| tr.transform_point3(Vec3::from(m.mesh.vertices[tri[k] as usize].position))));
            }
        }
    }
    (scenery, floor, tris)
}

/// The weighted fit of `y = a + b x`, `x = max(0, n.L)`, over the bins: its r2 (0 when `b <= 0`).
fn r2(bins: &[(Vec3, f32, f32)], to_light: Vec3) -> f32 {
    let (mut sw, mut sx, mut sy, mut sxx, mut sxy, mut syy) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for (n, y, w) in bins {
        let (x, y, w) = (f64::from(n.dot(to_light).max(0.0)), f64::from(*y), f64::from(*w));
        sw += w;
        sx += w * x;
        sy += w * y;
        sxx += w * x * x;
        sxy += w * x * y;
        syy += w * y * y;
    }
    if sw <= 0.0 {
        return 0.0;
    }
    let cov = sxy / sw - (sx / sw) * (sy / sw);
    let (vx, vy) = (sxx / sw - (sx / sw).powi(2), syy / sw - (sy / sw).powi(2));
    if vx <= 1e-12 || vy <= 1e-12 || cov <= 0.0 {
        return 0.0;
    }
    (cov * cov / (vx * vy)) as f32
}

/// The normal bins of the scenery: (bin's normal, the `q` quantile of its lumas, weight).
fn bins(scenery: &[Face], q: f32) -> Vec<(Vec3, f32, f32)> {
    let mut map: HashMap<(i32, i32), Vec<f32>> = HashMap::new();
    for f in scenery {
        let k = ((f.n.x.atan2(f.n.z).to_degrees() / AZ_BIN).floor() as i32, ((f.n.y + 1.0) / NY_BIN).floor() as i32);
        map.entry(k).or_default().push(f.luma);
    }
    // a fixed order (the map's own is random per process)
    let mut keys: Vec<(i32, i32)> = map.keys().copied().collect();
    keys.sort_unstable();
    keys.into_iter()
        .filter_map(|k| {
            let mut v = map.remove(&k)?;
            if v.len() < MIN_BIN {
                return None;
            }
            v.sort_by(f32::total_cmp);
            let az = ((k.0 as f32 + 0.5) * AZ_BIN).to_radians();
            let ny = ((k.1 as f32 + 0.5) * NY_BIN - 1.0).clamp(-1.0, 1.0);
            let r = (1.0 - ny * ny).max(0.0).sqrt();
            Some((Vec3::new(az.sin() * r, ny, az.cos() * r), v[((v.len() - 1) as f32 * q) as usize], (v.len() as f32).sqrt()))
        })
        .collect()
}

/// The fit over the hemisphere on the `q`-quantile bins (0.5 = the rule; the example prints 0.8 beside it).
pub fn fit_quantile(geometry: &TrackGeometry, materials: &[Material], q: f32) -> Option<BakeFit> {
    let (scenery, _, _) = faces(geometry, materials, false);
    let b = bins(&scenery, q);
    if b.len() < 10 {
        return None;
    }
    let mut best = (0.0f32, 0.0f32, 0.0f32);
    for ai in 0..120 {
        let az = -180.0 + ai as f32 * 3.0;
        for ei in 1..30 {
            let el = ei as f32 * 3.0;
            let r = r2(&b, -travel(az, el));
            if r > best.0 {
                best = (r, az, el);
            }
        }
    }
    let (r, azimuth, elevation) = best;
    Some(BakeFit { direction: travel(azimuth, elevation), azimuth, elevation, r2: r, r2_overhead: r2(&b, Vec3::Y), bins: b.len(), faces: scenery.len() })
}

/// The measurement of the module doc.
pub fn fit(geometry: &TrackGeometry, materials: &[Material]) -> Option<BakeFit> {
    fit_quantile(geometry, materials, 0.5)
}

/// The cast-shadow cue (validation only, not used at load): of the ground faces facing the light (`n.L` > 0.2; every
/// `stride`-th), those with a track face between them and the light within 150 m against those without - (lit mean luma -
/// shaded mean luma, lit count, shaded count), per light travel direction given.
pub fn cast_contrast(geometry: &TrackGeometry, materials: &[Material], lights: &[Vec3], stride: usize) -> Vec<(f32, usize, usize)> {
    const CELL: f32 = 4.0;
    let (_, floor, tris) = faces(geometry, materials, true);
    let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for (i, t) in tris.iter().enumerate() {
        let (lo, hi) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
        for x in (lo.x / CELL).floor() as i32..=(hi.x / CELL).floor() as i32 {
            for z in (lo.z / CELL).floor() as i32..=(hi.z / CELL).floor() as i32 {
                cells.entry((x, z)).or_default().push(i as u32);
            }
        }
    }
    let mut stamp = vec![0u32; tris.len()];
    let mut epoch = 0u32;
    let mut occluded = |o: Vec3, d: Vec3| -> bool {
        epoch += 1;
        let flat = Vec2::new(d.x, d.z).length().max(1e-3);
        let steps = (150.0 * flat / (CELL * 0.5)).ceil() as i32 + 1;
        for s in 0..=steps {
            let p = o + d * (s as f32 * CELL * 0.5 / flat).min(150.0);
            for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
                let Some(list) = cells.get(&((p.x / CELL).floor() as i32 + dx, (p.z / CELL).floor() as i32 + dz)) else { continue };
                for &i in list {
                    if stamp[i as usize] == epoch {
                        continue;
                    }
                    stamp[i as usize] = epoch;
                    if ray_hit(o, d, &tris[i as usize]).is_some_and(|h| h > 0.05 && h < 150.0) {
                        return true;
                    }
                }
            }
            if p.y > o.y + 40.0 {
                break;
            }
        }
        false
    };
    lights
        .iter()
        .map(|t| {
            let l = -t.normalize();
            let (mut sl, mut nl, mut so, mut no) = (0f64, 0usize, 0f64, 0usize);
            for f in floor.iter().step_by(stride.max(1)) {
                if f.n.dot(l) < 0.2 {
                    continue;
                }
                if occluded(f.centre + f.n * 0.1, l) {
                    so += f64::from(f.luma);
                    no += 1;
                } else {
                    sl += f64::from(f.luma);
                    nl += 1;
                }
            }
            let mean = |s: f64, n: usize| if n > 0 { s / n as f64 } else { 0.0 };
            ((mean(sl, nl) - mean(so, no)) as f32, nl, no)
        })
        .collect()
}

/// Moeller-Trumbore: the ray's distance to the triangle, if it meets it.
fn ray_hit(o: Vec3, d: Vec3, t: &[Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(e2.dot(q) * inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lambert_bake_gives_its_light_back() {
        // bins of every orientation lit by one light: the fit finds it, and an overhead light fits worse
        let light = travel(40.0, 35.0);
        let mut b = Vec::new();
        for ai in 0..24 {
            for yi in 0..20 {
                let az = (ai as f32 * 15.0 + 7.5).to_radians();
                let ny = yi as f32 * 0.1 - 0.95;
                let r = (1.0 - ny * ny).sqrt();
                let n = Vec3::new(az.sin() * r, ny, az.cos() * r);
                b.push((n, 0.3 + 0.5 * n.dot(-light).max(0.0), 1.0));
            }
        }
        let best = (0..120)
            .flat_map(|ai| (1..30).map(move |ei| (-180.0 + ai as f32 * 3.0, ei as f32 * 3.0)))
            .max_by(|x, y| r2(&b, -travel(x.0, x.1)).total_cmp(&r2(&b, -travel(y.0, y.1))))
            .unwrap();
        assert!((best.0 - 40.0).abs() <= 3.0 && (best.1 - 35.0).abs() <= 3.0, "{best:?}");
        assert!(r2(&b, -light) > 0.999 && r2(&b, Vec3::Y) < 0.9);
    }

    #[test]
    fn a_ray_meets_a_triangle_in_front_only() {
        let t = [Vec3::new(-1.0, 1.0, -1.0), Vec3::new(1.0, 1.0, -1.0), Vec3::new(0.0, 1.0, 1.0)];
        assert!((ray_hit(Vec3::ZERO, Vec3::Y, &t).unwrap() - 1.0).abs() < 1e-5);
        assert!(ray_hit(Vec3::ZERO, -Vec3::Y, &t).is_none_or(|h| h < 0.0));
        assert!(ray_hit(Vec3::new(5.0, 0.0, 0.0), Vec3::Y, &t).is_none());
    }

    /// With the game data: Schwarzwald's bake gives a light (travel azimuth about -162, low), castle's agrees with its sky
    /// sun's azimuth, and the near-overhead bakes of foothills / autobahn give none. Skips without the data.
    #[test]
    fn the_bake_gives_schwarzwalds_light() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else { return };
        let run = |name: &str| {
            let t = nfs_assets::track::TrackAssets::load(&paths, name).unwrap();
            fit(&t.build_geometry().unwrap(), &t.materials().unwrap()).unwrap()
        };
        let f = run("forest");
        assert!(f.accepted().is_some() && (f.azimuth + 162.0).abs() < 10.0 && (15.0..50.0).contains(&f.elevation), "{f:?}");
        let k = run("castle");
        assert!((k.azimuth - 136.0).abs() < 10.0, "{k:?}");
        assert!(run("foothills").accepted().is_none());
        assert!(run("autobahn").accepted().is_none());
    }
}
