//! Two spatial helpers the render-side culling shares: the nearest road slice of a point ([`SliceIndex`]) and the main view's frustum as
//! a sphere test ([`ViewCone`]). Pure geometry: nothing here reads or writes the simulation.

use glam::Vec3;

/// PERF: the main view's frustum as a cone (eye, axes, the half-angle tangents), for the question "can a light with this
/// reach touch what the camera sees": a sphere test, conservative (the sides use the exact plane distance), no pop-in
/// because a light whose sphere is outside lights nothing inside.
#[derive(Clone, Copy)]
pub struct ViewCone {
    eye: Vec3,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
    tan_x: f32,
    tan_y: f32,
    far: f32,
}

/// The margin on the frustum's half-angle tangents: 10 %.
const ANGLE_MARGIN: f32 = 1.1;

impl ViewCone {
    /// The frustum of a camera at `eye` looking along `fwd` (`right` and `up` its other axes, all unit), with the half-angle tangents
    /// `tan_x` / `tan_y` and the far plane `far`.
    pub fn new(eye: Vec3, fwd: Vec3, right: Vec3, up: Vec3, tan_x: f32, tan_y: f32, far: f32) -> Self {
        Self { eye, fwd, right, up, tan_x, tan_y, far }
    }

    /// Whether a sphere (`centre`, `radius`) intersects the frustum (with a 10 % margin on the angles).
    pub fn sees(&self, centre: Vec3, radius: f32) -> bool {
        let v = centre - self.eye;
        let z = v.dot(self.fwd);
        if z < -radius || z > self.far + radius {
            return false;
        }
        let (x, y) = (v.dot(self.right).abs(), v.dot(self.up).abs());
        let (tx, ty) = (self.tan_x * ANGLE_MARGIN, self.tan_y * ANGLE_MARGIN);
        x <= z.max(0.0) * tx + radius * (1.0 + tx * tx).sqrt() && y <= z.max(0.0) * ty + radius * (1.0 + ty * ty).sqrt()
    }
}

/// PERF: the nearest road slice of a point, the answer of the scan it replaces — `slices.iter().map(|s|
/// s.centre.distance_squared(p)).min_by(total_cmp)`, the FIRST slice of the smallest distance — without visiting every slice
/// for every lamp (thousands x thousands at a race's start). The centres are ordered by x once; a query walks outward from
/// `p.x` on both sides and stops a side where `dx * dx` alone exceeds the best distance: the distance is the same
/// `distance_squared` (so the same bits) and, its other terms being >= 0, never below that side's `dx * dx` (rounding is
/// monotonic), so nothing past the stop can be nearer or tie. Ties go to the lower index, as `min_by` does. A centre or a
/// point that is not finite falls back to the scan itself.
pub struct SliceIndex {
    centres: Vec<Vec3>,
    /// slice indices by centre x (then by index)
    by_x: Vec<u32>,
    finite: bool,
}

impl SliceIndex {
    pub fn new(centres: Vec<Vec3>) -> Self {
        let mut by_x: Vec<u32> = (0..centres.len() as u32).collect();
        by_x.sort_by(|&a, &b| centres[a as usize].x.total_cmp(&centres[b as usize].x).then(a.cmp(&b)));
        let finite = centres.iter().all(|c| c.is_finite());
        Self { centres, by_x, finite }
    }

    /// The scan the index replaces (the fallback, and the tests' reference).
    pub fn scan(centres: &[Vec3], p: Vec3) -> Option<usize> {
        centres.iter().enumerate().map(|(i, c)| (i, c.distance_squared(p))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
    }

    /// The index of the slice nearest to `p` (`None` without slices).
    pub fn nearest(&self, p: Vec3) -> Option<usize> {
        if self.centres.is_empty() {
            return None;
        }
        if !self.finite || !p.is_finite() {
            return Self::scan(&self.centres, p);
        }
        let k = self.by_x.partition_point(|&i| self.centres[i as usize].x < p.x);
        let mut best = (f32::INFINITY, usize::MAX);
        // right of `p.x` in ascending x, then left of it in descending x
        for &i in &self.by_x[k..] {
            if !self.visit(i as usize, p, &mut best) {
                break;
            }
        }
        for &i in self.by_x[..k].iter().rev() {
            if !self.visit(i as usize, p, &mut best) {
                break;
            }
        }
        Some(best.1)
    }

    /// Slice `i` against the best `(distance, index)` so far; `false` = past it on this side nothing can be nearer.
    fn visit(&self, i: usize, p: Vec3, best: &mut (f32, usize)) -> bool {
        let c = self.centres[i];
        let dx = c.x - p.x;
        if dx * dx > best.0 {
            return false;
        }
        let d = c.distance_squared(p);
        if d < best.0 || (d == best.0 && i < best.1) {
            *best = (d, i);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sphere in front is seen, one behind or far to the side is not, the margin keeps the edge's.
    #[test]
    fn the_view_cone_is_a_conservative_sphere_test() {
        let c = ViewCone::new(Vec3::ZERO, Vec3::Z, Vec3::X, Vec3::Y, 1.0, 1.0, 100.0);
        assert!(c.sees(Vec3::new(0.0, 0.0, 10.0), 1.0));
        assert!(!c.sees(Vec3::new(0.0, 0.0, -10.0), 1.0));
        assert!(c.sees(Vec3::new(0.0, 0.0, -0.5), 1.0));
        assert!(!c.sees(Vec3::new(0.0, 0.0, 120.0), 1.0));
        assert!(!c.sees(Vec3::new(50.0, 0.0, 10.0), 1.0));
        // on the edge of the 45 degree cone: inside by the 10 % margin
        assert!(c.sees(Vec3::new(10.5, 0.0, 10.0), 0.1));
    }

    /// Every query of `pts` answered as the scan answers it.
    fn same(centres: &[Vec3], pts: &[Vec3]) -> usize {
        let idx = SliceIndex::new(centres.to_vec());
        for p in pts {
            assert_eq!(idx.nearest(*p), SliceIndex::scan(centres, *p), "{p:?}");
        }
        pts.len()
    }

    /// ties (the same centre twice, a point midway), an empty table, a non-finite point.
    #[test]
    fn the_index_breaks_ties_as_the_scan() {
        let c = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(-2.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 5.0)];
        same(&c, &[Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 3.0, 0.0), Vec3::ZERO, Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 2.5), Vec3::new(f32::NAN, 0.0, 0.0), Vec3::new(f32::INFINITY, 0.0, 0.0)]);
        assert_eq!(SliceIndex::new(Vec::new()).nearest(Vec3::ONE), None);
        // a non-finite centre: the scan itself
        same(&[Vec3::new(f32::NAN, 0.0, 0.0), Vec3::ONE], &[Vec3::ZERO]);
    }

    /// on the real tracks' slices (skipped without the game data) the index picks the scan's slice for
    /// every centre, points round them where lamps stand (beside and above the road), points midway between neighbours (the
    /// ties) and a deterministic scatter over the track's box.
    #[test]
    fn the_index_picks_the_scans_slice_on_real_tracks() {
        let Ok(paths) = nfs_assets::GamePaths::locate() else {
            eprintln!("skipping: no game data");
            return;
        };
        let mut checked = 0;
        for track in ["alps", "monaco1", "coastal", "autobahn"] {
            let Ok(t) = nfs_assets::track::TrackAssets::load(&paths, track) else { continue };
            let Ok(table) = crate::race::SliceTable::from_crp(&t.crp, &t.crp_data) else { continue };
            let c: Vec<Vec3> = table.slices.iter().map(|s| s.centre).collect();
            let mut pts = Vec::new();
            for (i, s) in table.slices.iter().enumerate() {
                pts.push(s.centre);
                let side = s.forward.cross(Vec3::Y).normalize_or_zero();
                for (a, h) in [(8.0, 6.0), (-8.0, 6.0), (14.0, 9.0), (-3.0, 12.0)] {
                    pts.push(s.centre + side * a + Vec3::Y * h);
                }
                if let Some(n) = c.get(i + 1) {
                    pts.push((s.centre + *n) * 0.5);
                }
            }
            let (lo, hi) = c.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
            let mut r = 0x1234_5678u32;
            let mut next = || {
                r = r.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (r >> 8) as f32 / (1u32 << 24) as f32
            };
            for _ in 0..4000 {
                pts.push(lo + (hi - lo) * Vec3::new(next(), next(), next()));
            }
            checked += same(&c, &pts);
        }
        eprintln!("{checked} queries");
        assert!(checked > 0, "no track loaded");
    }
}
