//! The track's collision props — `Car_propPass` 0x498DE0 and the two object classes the track
//! loader 0x4750B0 builds for it (`RE/notes/suspension-collision.md` §9): a `WTreeElem_CylinderObject`
//! for an article whose `animdefs.txt` definition says `kCylinder`, a `WTreeElem_BoxObject` for
//! `kBox`. The cars collide with them after their walls, in the collision pass.
use glam::Vec3;

#[derive(Debug, Clone, PartialEq)]
pub enum PropShape {
    /// `+0x34`, the radius — at least 0.25 (0x4866B3…0x4866D1)
    Cylinder { radius: f32 },
    /// `+0x34…+0x58`, the four footprint corners in world space (0x474CB0 through the article's
    /// transform), and `+0x64`, the horizontal half-diagonal of the AABB × 1.05 (0x486C47…0x486CA3)
    Box { corners: [Vec3; 4], radius: f32 },
}

/// One collision object.
#[derive(Debug, Clone, PartialEq)]
pub struct Prop {
    pub shape: PropShape,
    /// `+0x08…+0x1C`, the instance's AABB (0x486340 from the eight corners)
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
    /// `+0x20`: what the event word `0x50000 | +0x20` carries (0x498DAD, 0x498A3B)
    pub class: u32,
    /// `None` for a definition without pieces: slot 8 (0x487290 → 0x4767D0 → 0x476380) then always
    /// answers 2, a solid hit (`def+0x18 == 0`, 0x4763BC…0x4763C5).
    pub breakable: Option<Breakable>,
}

/// A prop whose `animdefs.txt` definition has pieces (`BBOX`, the crate stacks): the answer of
/// 0x4767D0 by the instance record's lead player `rec+0x1C`.
#[derive(Debug, Clone, PartialEq)]
pub struct Breakable {
    /// the definition's `speed=` values and pieces per band (`AnimDef::group`)
    pub bands: Vec<f32>,
    pub band_pieces: Vec<usize>,
    /// per piece: whether its `QuatGroup` names an article with an `Anim` part whose two words
    /// are not both 0 (0x476501…0x47651F) — 0x476380 returns false at the first that does not
    pub resolved: Vec<bool>,
    /// The hit that started the pieces: the game tick and the car's velocity `car+0x33C`
    /// (0x498E61). Once set the record has its lead player and every later answer is 0.
    pub started: std::cell::Cell<Option<(u32, Vec3)>>,
    /// the prop's article in the track's `.crp` (its instance record)
    pub article: usize,
}

impl Breakable {
    /// 0x4767D0's answer to a car at `velocity` on `tick`: 0 once the lead player exists; else
    /// 0x476380 runs — `2` (solid) when the group's first piece does not resolve (nothing starts),
    /// otherwise the lead player is made with the first piece and the answer is `1`, or `2` when a
    /// later piece of the group does not resolve (0x476380 returns false after building the ones
    /// before it).
    pub fn answer(&self, tick: u32, velocity: Vec3) -> u32 {
        if self.started.get().is_some() {
            return 0;
        }
        let group = nfs_formats::animdefs::band_group(&self.bands, &self.band_pieces, velocity.length());
        if group.is_empty() || !self.resolved.get(group.start).copied().unwrap_or(false) {
            return 2;
        }
        self.started.set(Some((tick, velocity)));
        if group.clone().all(|i| self.resolved.get(i).copied().unwrap_or(false)) { 1 } else { 2 }
    }
}

impl Prop {
    /// Slot 1, 0x486490: the AABB's middle.
    pub fn centre(&self) -> Vec3 {
        (self.aabb_min + self.aabb_max) * 0.5
    }

    /// Slot 5: 0x4866E0 / 0x486D10.
    pub fn radius(&self) -> f32 {
        match self.shape {
            PropShape::Cylinder { radius } | PropShape::Box { radius, .. } => radius,
        }
    }

    /// 0x486510(pos, 5.0): the AABB meets `pos ± r` on x and z.
    pub fn near(&self, pos: Vec3, r: f32) -> bool {
        pos.x + r >= self.aabb_min.x && pos.x - r <= self.aabb_max.x && pos.z + r >= self.aabb_min.z && pos.z - r <= self.aabb_max.z
    }
}

/// `0x489CC0`: the x-z crossing of `a` (whose point is returned) and `b`, both ends of both
/// parameters included; `None` for parallel segments.
pub fn segments_cross(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> Option<Vec3> {
    let (ax, az) = (a1.x - a0.x, a1.z - a0.z);
    let (bx, bz) = (b1.x - b0.x, b1.z - b0.z);
    let den = bz * ax - bx * az;
    if den == 0.0 {
        return None;
    }
    let (dz, dx) = (a0.z - b0.z, a0.x - b0.x);
    let ua = (bx * dz - bz * dx) / den;
    if !(0.0..=1.0).contains(&ua) {
        return None;
    }
    let ub = (ax * dz - az * dx) / den;
    if !(0.0..=1.0).contains(&ub) {
        return None;
    }
    Some(a0 + (a1 - a0) * ua)
}

/// The box's slot 6, 0x486FB0(seg, …, &normal, &point): each of its edges `i → i+1` against the
/// car's edge `seg`. The answer is the number of crossings; the first sets the point, the edge's
/// normal `(e.z, 0, −e.x)` and the distance from the car edge's start; a later one moves the point
/// only when nearer — without keeping the new distance — and always takes the normal
/// (0x4871AD…0x487253).
pub fn box_edge_test(corners: &[Vec3; 4], s0: Vec3, s1: Vec3) -> (u32, Vec3, Vec3) {
    let (mut count, mut point, mut normal) = (0u32, Vec3::ZERO, Vec3::ZERO);
    let mut best = 0.0f32;
    for i in 0..4 {
        let (c0, c1) = (corners[i], corners[(i + 1) % 4]);
        let Some(hit) = segments_cross(c0, c1, s0, s1) else { continue };
        count += 1;
        let e = Vec3::new(c1.x - c0.x, c1.y - c0.y, c1.z - c0.z).normalize_or_zero();
        let n = Vec3::new(e.z, 0.0, -e.x);
        let dist = (hit - s0).length();
        if count == 1 {
            normal = n;
            point = hit;
            best = dist;
        } else {
            if dist < best {
                point = hit;
            }
            normal = n;
        }
    }
    (count, point, normal)
}

/// 0x474CB0: the footprint of a box article from its vertices — the pair farthest apart in x-z
/// (every ordered pair, the first strictly larger kept), the points moved by that pair's outer
/// point (`(p + p)·0.5`, the image's own quirk), the rotation about y from 0° by 0.1° while under
/// 90° whose x-z box has the least area (strictly), and that box's corners `(min.x, min.z)`,
/// `(min.x, max.z)`, `(max.x, max.z)`, `(max.x, min.z)` at y 0 turned back and moved by the
/// centre's x and z. The caller takes them through the article's transform.
pub fn box_footprint(points: &[Vec3]) -> [Vec3; 4] {
    if points.is_empty() {
        return [Vec3::ZERO; 4];
    }
    let mut far = 0usize;
    let mut best = 0.0f32;
    for (i, p) in points.iter().enumerate() {
        for q in points {
            let (dx, dz) = (q.x - p.x, q.z - p.z);
            let d = dx * dx + dz * dz;
            if d > best {
                best = d;
                far = i;
            }
        }
    }
    let c = points[far];
    let local: Vec<Vec3> = points.iter().map(|p| *p - c).collect();
    // `glam`'s y rotation by the angle in degrees, as 0x539BD0 builds it; the same one turns the
    // corners back, so its sense does not matter
    let rot = |deg: f32, v: Vec3| glam::Quat::from_rotation_y(deg.to_radians()) * v;
    let mut angle = 0.0f32;
    let (mut best_area, mut best_angle, mut lo, mut hi) = (1.0e10f32, 0.0f32, Vec3::ZERO, Vec3::ZERO);
    while angle < 90.0 {
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &local {
            let r = rot(angle, *p);
            mn = mn.min(r);
            mx = mx.max(r);
        }
        let area = (mx.z - mn.z) * (mx.x - mn.x);
        if area < best_area {
            best_area = area;
            best_angle = angle;
            lo = mn;
            hi = mx;
        }
        angle += 0.1;
    }
    let corners = [Vec3::new(lo.x, 0.0, lo.z), Vec3::new(lo.x, 0.0, hi.z), Vec3::new(hi.x, 0.0, hi.z), Vec3::new(hi.x, 0.0, lo.z)];
    corners.map(|k| {
        let r = rot(-best_angle, k);
        Vec3::new(r.x + c.x, 0.0, r.z + c.z)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x4767D0: the first hit starts the pieces (1), every later one passes (0); a group whose
    /// first piece does not resolve stays solid (2) and starts nothing.
    #[test]
    fn breakable_answers() {
        let b = Breakable { bands: vec![0.0], band_pieces: vec![2], resolved: vec![true, true], started: Default::default(), article: 0 };
        assert_eq!(b.answer(100, Vec3::new(0.0, 0.0, 10.0)), 1);
        assert_eq!(b.started.get(), Some((100, Vec3::new(0.0, 0.0, 10.0))));
        assert_eq!(b.answer(101, Vec3::new(0.0, 0.0, 10.0)), 0);
        let bad = Breakable { resolved: vec![false, true], ..Breakable { bands: vec![0.0], band_pieces: vec![2], resolved: vec![], started: Default::default(), article: 0 } };
        assert_eq!(bad.answer(5, Vec3::X * 3.0), 2);
        assert_eq!(bad.started.get(), None);
        // a later piece missing: started, but this hit is still solid
        let half = Breakable { bands: vec![0.0], band_pieces: vec![2], resolved: vec![true, false], started: Default::default(), article: 0 };
        assert_eq!(half.answer(5, Vec3::X * 3.0), 2);
        assert_eq!(half.answer(6, Vec3::X * 3.0), 0);
    }

    #[test]
    fn segments_cross_inclusive_and_parallel() {
        let hit = segments_cross(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.5, 0.0, -1.0), Vec3::new(0.5, 0.0, 1.0));
        assert!((hit.unwrap() - Vec3::new(0.5, 0.0, 0.0)).length() < 1e-6);
        // touching at an end counts
        assert!(segments_cross(Vec3::ZERO, Vec3::X, Vec3::X, Vec3::new(1.0, 0.0, 1.0)).is_some());
        assert!(segments_cross(Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)).is_none());
    }

    /// An axis-aligned 4 × 2 slab: the least-area rectangle is the slab itself, found at 0°.
    #[test]
    fn footprint_of_an_axis_aligned_slab() {
        let mut pts = Vec::new();
        for x in [-2.0f32, 2.0] {
            for z in [-1.0f32, 1.0] {
                for y in [0.0f32, 1.0] {
                    pts.push(Vec3::new(x + 10.0, y, z + 5.0));
                }
            }
        }
        let f = box_footprint(&pts);
        let (mn, mx) = f.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        assert!((mn.x - 8.0).abs() < 1e-3 && (mx.x - 12.0).abs() < 1e-3, "{f:?}");
        assert!((mn.z - 4.0).abs() < 1e-3 && (mx.z - 6.0).abs() < 1e-3, "{f:?}");
    }

    #[test]
    fn box_edge_test_counts_crossings() {
        let c = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 2.0), Vec3::new(2.0, 0.0, 0.0)];
        // straight through: two crossings
        let (n2, _, _) = box_edge_test(&c, Vec3::new(-1.0, 0.0, 1.0), Vec3::new(3.0, 0.0, 1.0));
        assert_eq!(n2, 2);
        // into it: one crossing, on the x = 0 edge, whose normal is (e.z, 0, −e.x) of e = +z
        let (n1, p, n) = box_edge_test(&c, Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0));
        assert_eq!(n1, 1);
        assert!((p - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-6);
        assert!((n - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-6, "{n:?}");
    }
}
