//! Wall geometry: the `.edg` cache's edge segments, treated as vertical walls in the xz plane.
//!
//! The *response* is the original's and lives in `vehicle::collide_walls`
//! (`Car_wallCollide` 0x495020 + `WallImpulse` 0x494000, `RE/notes/suspension-collision.md`
//! §6.1/§6.2). What is still unread is the original's collision **world** — the broad phase at
//! 0x49AEC0 over `[obj+0x38]` — so this module does its own segment lookup over the same `.edg`
//! data the game caches.

use crate::quadtree::Entry;
use crate::track_tree::{TrackTree, TreeItem};
use glam::{Vec2, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Wall {
    pub a: Vec2,
    pub b: Vec2,
    /// The segment's own mid height. `Tracker::edgeHit` 0x4740D0 keeps, among the edges a segment
    /// **crosses in xz**, the one whose endpoint-midpoint height is nearest the moving segment's —
    /// a tie-break, *not* a filter. This port used to reject any edge whose height was more than
    /// 3 m from the car, which is why a car that had dropped into a dip drove straight through the
    /// wall line and out of the world (`RE/notes/physics.md` §4.1c-bis, found 2026-09-23).
    pub y_mid: f32,
    /// The two ends' heights, lower and higher.
    pub y_lo: f32,
    pub y_hi: f32,
    /// The heights of the ends `a` and `b` themselves (the camera pushes 0x464500 take the
    /// edge's direction as the **3-D** unit `p1 − p0`, 0x532330 + 0x5328D0).
    pub ya: f32,
    pub yb: f32,
    /// Wall material class 0..5 = metal, wood, snow, ice, earth, rock (`WallImpulse` 0x494000
    /// byte tables 0x49442C / 0x4943BC over the `.edg` word 0x10..0x60; other words → rock).
    pub class: u8,
}

/// The quadtree accept test's two vtable calls (`0x484320`, `RE/RENDERING.md` "The accept test,
/// read 2026-09-28") for a `.edg` edge object (`WTreeElem_Edge`, vtable `0x5B484C`), read to the
/// instruction 2026-09-28: slot 1 (`0x4852B0`) is `0.5·(p1 + p2)` per axis, and slot 2 (`0x485340`)
/// is the componentwise xz min/max of the same two endpoints — both functions read only offsets
/// `+0` (x) and `+8` (z) of each 16-byte-stride endpoint, `y` (`+4`) untouched, exactly like the
/// patch getters (`crate::collision`) and the accept test itself. [`Wall::a`]/[`Wall::b`] are
/// already the edge's two xz endpoints (the original's `p1`/`p2`), so no new field is needed.
impl Entry for Wall {
    fn centroid(&self) -> (f32, f32) {
        let m = (self.a + self.b) * 0.5;
        (m.x, m.y)
    }

    fn extent(&self) -> ((f32, f32), (f32, f32)) {
        let min = self.a.min(self.b);
        let max = self.a.max(self.b);
        ((min.x, min.y), (max.x, max.y))
    }
}

/// `.edg` wall word → material class (tables 0x49442C / 0x4943BC, CONFIRMED from the bytes;
/// the class order metal/wood/snow/ice/earth/rock is HIGH from the per-track word statistics).
/// The class is the CRP material's `mt.flags & 0xF0` carried into the `.edg` word by the rebuild path
/// 0x4750B0 (`RE/notes/suspension-collision.md` §7). `WallImpulse` clamps anything outside
/// `[0x10, 0x60]` to 0x60, and no shipped track has a word inside that range which is not a multiple
/// of 0x10, so the game's seventh ("unknown") table entry is unreachable and rock is the fallback.
pub fn wall_class(word: u32) -> u8 {
    match word {
        0x10 => class::METAL,
        0x20 => class::WOOD,
        0x30 => class::SNOW,
        0x40 => class::ICE,
        0x50 => class::EARTH,
        _ => class::ROCK,
    }
}

/// The wall material classes [`wall_class`] returns (and `Wall::class`, `Vehicle::last_wall_class`
/// carry to the sound code as a number).
pub mod class {
    pub const METAL: u8 = 0;
    pub const WOOD: u8 = 1;
    pub const SNOW: u8 = 2;
    pub const ICE: u8 = 3;
    pub const EARTH: u8 = 4;
    pub const ROCK: u8 = 5;
}

#[derive(Debug, Clone)]
pub struct WallSet {
    walls: Vec<Wall>,
    cells: Vec<Vec<u32>>,
    min: Vec2,
    cell: f32,
    nx: usize,
    nz: usize,
}

impl WallSet {
    /// Build from `(p1, p2, wall word)` segments (`.edg` records; barrier boxes use 0x60).
    pub fn build(segments: impl Iterator<Item = ([f32; 3], [f32; 3], u32)>, cell: f32) -> Self {
        let walls: Vec<Wall> = segments
            .map(|(p1, p2, word)| Wall {
                a: Vec2::new(p1[0], p1[2]),
                b: Vec2::new(p2[0], p2[2]),
                y_mid: (p1[1] + p2[1]) * 0.5,
                y_lo: p1[1].min(p2[1]),
                y_hi: p1[1].max(p2[1]),
                ya: p1[1],
                yb: p2[1],
                class: wall_class(word),
            })
            .filter(|w| (w.a - w.b).length_squared() > 1e-6)
            .collect();
        let mut min = Vec2::splat(f32::MAX);
        let mut max = Vec2::splat(f32::MIN);
        for w in &walls {
            min = min.min(w.a).min(w.b);
            max = max.max(w.a).max(w.b);
        }
        if walls.is_empty() {
            min = Vec2::ZERO;
            max = Vec2::ZERO;
        }
        let nx = (((max.x - min.x) / cell).ceil() as usize).max(1);
        let nz = (((max.y - min.y) / cell).ceil() as usize).max(1);
        let mut cells = vec![Vec::new(); nx * nz];
        let idx = |p: Vec2| crate::collision::grid_cell(min.x, min.y, cell, nx, nz, p.x, p.y);
        for (i, w) in walls.iter().enumerate() {
            let (x0, z0) = idx(w.a.min(w.b));
            let (x1, z1) = idx(w.a.max(w.b));
            for z in z0..=z1 {
                for x in x0..=x1 {
                    cells[z * nx + x].push(i as u32);
                }
            }
        }
        Self { walls, cells, min, cell, nx, nz }
    }

    pub fn len(&self) -> usize {
        self.walls.len()
    }
    pub fn is_empty(&self) -> bool {
        self.walls.is_empty()
    }

    /// The `.edg` segments in file order (`insertAll(A)`'s own order, plus any `extra_walls` the
    /// caller appended after) — what `nfs_game::world` hands to [`crate::track_tree::TrackTree`].
    pub fn walls(&self) -> &[Wall] {
        &self.walls
    }

    /// Nearest wall within `radius` of point `p` (xz) whose height span contains `y`.
    /// Returns (penetration normal pointing away from the wall, penetration depth).
    pub fn nearest(&self, p: Vec3, radius: f32) -> Option<(Vec2, f32)> {
        if self.walls.is_empty() {
            return None;
        }
        let q = Vec2::new(p.x, p.z);
        let idx = |v: Vec2| crate::collision::grid_cell(self.min.x, self.min.y, self.cell, self.nx, self.nz, v.x, v.y);
        let (x0, z0) = idx(q - Vec2::splat(radius));
        let (x1, z1) = idx(q + Vec2::splat(radius));
        let mut best: Option<(Vec2, f32)> = None;
        let mut best_key: Option<(f32, f32)> = None;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &wi in &self.cells[z * self.nx + x] {
                    let w = &self.walls[wi as usize];
                    let ab = w.b - w.a;
                    let t = ((q - w.a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                    let c = w.a + ab * t;
                    let d = q - c;
                    let dist = d.length();
                    if dist < radius {
                        // among those in range, the one nearest in height (0x4740D0's tie-break),
                        // and then the deepest
                        let key = ((w.y_mid - p.y).abs(), -(radius - dist));
                        if best_key.map(|b| key < b).unwrap_or(true) {
                            let n = if dist > 1e-5 { d / dist } else { Vec2::new(-ab.y, ab.x).normalize_or_zero() };
                            best_key = Some(key);
                            best = Some((n, radius - dist));
                        }
                    }
                }
            }
        }
        best
    }
}

impl WallSet {
    /// `Tracker::edgeHit` 0x4740D0 on the segment `p0 → p1`: of the edges it crosses in xz (0x489CC0,
    /// both parameters in `[0, 1]`), the one whose endpoint-mid height is nearest the segment's mid
    /// height `(p0.y + p1.y)/2` (0x474185…0x4741BF). Returns (edge normal `normalize(−d.z, 0, d.x)`,
    /// projection of `p1` onto the edge line (0x489F10), material class).
    ///
    /// `candidates` is the caller's own edge order — [`crate::vehicle::Vehicle::collide_walls`]
    /// hands it the [`TrackTree`]'s `query_segment(p0, p1)`, filtered to [`TreeItem::Wall`]
    /// indices, exactly the order `0x4848B0`'s descent visits them in. A tie in height (equal
    /// `key`, the `<` below is strict) now goes to whichever candidate came **first in that
    /// order** — `0x474191…0x47419D`'s own `fcomp`/`test ah,1`, matching the original instead of
    /// this port's former `t`-along-the-segment tie-break (an unread guess, corrected
    /// 2026-09-28 — see `RE/RENDERING.md` "The tree, built").
    pub fn segment_hit(&self, p0: Vec3, p1: Vec3, candidates: impl IntoIterator<Item = usize>) -> Option<(Vec2, Vec2, u8)> {
        if self.walls.is_empty() {
            return None;
        }
        let (a, b) = (Vec2::new(p0.x, p0.z), Vec2::new(p1.x, p1.z));
        let mut best: Option<(f32, Vec2, Vec2, u8)> = None;
        for wi in candidates {
            let w = &self.walls[wi];
            let d = w.b - w.a;
            let r = b - a;
            let denom = r.x * d.y - r.y * d.x;
            if denom.abs() < 1e-8 {
                continue;
            }
            let s = a - w.a;
            let t = (s.y * d.x - s.x * d.y) / denom; // along the motion segment
            let u = (s.y * r.x - s.x * r.y) / denom; // along the wall
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                // 0x4740D0 keeps the crossed edge nearest **in height** to the segment's middle
                // (0x474191…0x47419D; `p1.y` alone until 2026-09-26); a tie now goes to the first
                // candidate in `candidates`' own order (the strict `<` below never replaces an
                // equal-key `best`).
                let key = (w.y_mid - (p0.y + p1.y) * 0.5).abs();
                if best.as_ref().map(|b| key < b.0).unwrap_or(true) {
                    let n = Vec2::new(-d.y, d.x).normalize_or_zero();
                    // 0x489F10 projects onto the edge's infinite **line** — `t = (p − e0)·d /
                    // d·d` with no clamp (0x489F43…0x489F84) — so near a joint the push stays
                    // perpendicular to the wall. The port clamped `t` to the segment until
                    // 2026-09-26, which there turned the push along the wall and let a car
                    // scraping it at speed work through the wall line (monaco5, skidpad)
                    let proj = w.a + d * ((b - w.a).dot(d) / d.length_squared());
                    best = Some((key, n, proj, w.class));
                }
            }
        }
        best.map(|(_, n, q, c)| (n, q, c))
    }
}

impl WallSet {
    /// **The camera's wall push 0x464500** on the eye `eye` of a camera following a car at `car`
    /// whose ground height is `car+0x41C` = `ground`: for each edge near the segment (0x473E00),
    /// the xz crossing of `eye → car` with the edge (0x489CC0: both parameters within [0, 1]); an
    /// edge whose higher end is more than `[0x5B38EC]` 3.5 below `ground`, or whose lower end is
    /// more than 3.5 above it, is passed over unless `any_height` (`[0x5CF584]` = 0, Castle); else
    /// the eye moves to the crossing plus the edge's left normal `n = (−e.z, 0, e.x)`, its height
    /// `2·y₀ − crossing.y` with `y₀` the eye's height on entry (0x4646BE…0x4646DE), and the next
    /// edge is tested from there.
    ///
    /// `e` is the edge's **3-D** unit direction `normalize(p1 − p0)` (0x532330, then 0x5328D0 whose
    /// length 0x532B20 takes all three components), so on a sloped edge `n` is shorter than 1 — the
    /// port normalised in xz until 2026-09-27.
    ///
    /// **When no edge moved the eye** (0x464768…0x46476E), a second pass (0x464774…0x464961): the
    /// edges that cross the 1.0 circle about the eye in xz (0x473BA0 with radius 1.0, whose test is
    /// 0x489DF0 — a root of `|p0 + t·(p1 − p0) − eye|² = 1` in [0, 1], so an edge lying wholly
    /// inside the circle is *not* listed); for each, the probe `eye → eye − n` (the current eye)
    /// is crossed with the edge (0x489CC0), the same height gate applies, and the eye becomes the
    /// crossing plus `n`, its height `y₀` (0x46494A…0x464951). This keeps the eye off a wall it is
    /// already standing beside. Returns whether either pass moved the eye.
    ///
    /// Wired to the real tree 2026-09-28 (`RE/RENDERING.md` "The tree, built"): `tree` is the
    /// track's own [`TrackTree`], `insertAll(C)`-then-`insertAll(A)`-ordered; both passes below
    /// take their candidate edges from `tree.query_segment(...)`, in the query's own leaf-then-
    /// insertion order (`0x4848B0`), each list taken **once**, before either pass's loop starts —
    /// same as the original: an edge lying in more than one leaf the segment's box touches is
    /// listed once per leaf (`0x473E00`/`0x473BA0`), so it can move the eye more than once, which
    /// this port used to avoid with its own deduplicating grid (`RE/RENDERING.md`'s approximation,
    /// now settled — the two passes' own doc above already describes what happens when a wall
    /// moves the eye more than once, and it happens in the original too).
    pub fn camera_push(&self, eye: &mut Vec3, car: Vec3, ground: f32, any_height: bool, tree: &TrackTree) -> bool {
        if self.walls.is_empty() {
            return false;
        }
        let y0 = eye.y;
        let gated = |w: &Wall| !any_height && (w.y_hi + 3.5 < ground || w.y_lo - 3.5 > ground);
        // 0x489CC0 with A = `from → to` and B = the edge: the parameter along A, both in [0, 1]
        let cross = |from: Vec3, to: Vec3, w: &Wall| -> Option<f32> {
            let (dax, daz) = (to.x - from.x, to.z - from.z);
            let (dbx, dbz) = (w.b.x - w.a.x, w.b.y - w.a.y);
            let denom = dax * dbz - dbx * daz;
            if denom == 0.0 {
                return None;
            }
            let (wx, wz) = (from.x - w.a.x, from.z - w.a.y);
            let u = (wz * dbx - wx * dbz) / denom;
            let v = (wz * dax - wx * daz) / denom;
            ((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)).then_some(u)
        };
        // 0x532330 (p1 − p0) and 0x5328D0 (3-D normalise), then `(−e.z, 0, e.x)`
        let normal = |w: &Wall| {
            let e = Vec3::new(w.b.x - w.a.x, w.yb - w.ya, w.b.y - w.a.y);
            let e = e / e.length();
            Vec3::new(-e.z, 0.0, e.x)
        };
        let wall_items = |it: TreeItem| match it {
            TreeItem::Wall(i) => Some(i),
            _ => None,
        };
        let mut moved = false;
        let segment_edges: Vec<usize> = tree.query_segment((eye.x, eye.z), (car.x, car.z)).filter_map(wall_items).collect();
        for wi in segment_edges {
            let w = &self.walls[wi];
            let Some(u) = cross(*eye, car, w) else { continue };
            if gated(w) {
                continue;
            }
            let hit = *eye + (car - *eye) * u;
            let n = normal(w);
            *eye = Vec3::new(hit.x + n.x, 2.0 * y0 - hit.y, hit.z + n.z);
            moved = true;
        }
        if moved {
            return true;
        }
        // the second pass: 0x473BA0(eye, 1.0), the list taken once at the eye on entry
        let q = Vec2::new(eye.x, eye.z);
        let near: Vec<usize> = tree
            .query_segment((q.x - 1.0, q.y - 1.0), (q.x + 1.0, q.y + 1.0))
            .filter_map(wall_items)
            .filter(|&wi| circle_cross(&self.walls[wi], q, 1.0))
            .collect();
        for wi in near {
            let w = &self.walls[wi];
            let n = normal(w);
            let Some(u) = cross(*eye, *eye - n, w) else { continue };
            if gated(w) {
                continue;
            }
            let hit = *eye - n * u;
            *eye = Vec3::new(hit.x + n.x, y0, hit.z + n.z);
            moved = true;
        }
        moved
    }
}

/// **0x489DF0** `(x0, z0, x1, z1, px, pz, r, &t1, &t2)`: whether the xz segment `p0 → p1` crosses
/// the circle of radius `r` about `(px, pz)` — the roots `t = (−b ∓ √(b² − 4ac)) / 2a` of
/// `|p0 + t·d − p|² = r²` (`a = d·d`, `b = 2·d·(p0 − p)`), true when the first lies in [0, 1] or
/// else the second does (0x489EC8…0x489F07). A negative discriminant gives a NaN root (the plain
/// `fsqrt` 0x5327D0), which no comparison accepts; a segment wholly inside the circle has
/// `t1 < 0 < 1 < t2` and is refused.
fn circle_cross(w: &Wall, p: Vec2, r: f32) -> bool {
    let d = w.b - w.a;
    let a = d.x * d.x + d.y * d.y;
    let b = 2.0 * (d.x * (w.a.x - p.x) + d.y * (w.a.y - p.y));
    let c = w.a.x * w.a.x + w.a.y * w.a.y + p.x * p.x + p.y * p.y - 2.0 * (w.a.x * p.x + w.a.y * p.y) - r * r;
    let s = (b * b - 4.0 * a * c).sqrt();
    let t1 = (-b - s) / (2.0 * a);
    let t2 = (s - b) / (2.0 * a);
    (0.0..=1.0).contains(&t1) || (0.0..=1.0).contains(&t2)
}

#[cfg(test)]
mod camera_push_tests {
    use super::*;

    /// A `TrackTree` over exactly `set`'s own walls, in file order — a test fixture stand-in for
    /// `nfs_game::world`'s real `insertAll(C)`-then-`insertAll(A)` order (no patches/props here).
    fn tree_for(set: &WallSet) -> TrackTree {
        let order: Vec<TreeItem> = (0..set.len()).map(TreeItem::Wall).collect();
        TrackTree::build(&[], &[], set.walls(), &order)
    }

    #[test]
    fn a_wall_between_the_car_and_the_eye_pulls_the_eye_in_front_of_it() {
        // a wall along x at z = 5, drawn left to right; the car at z = 10, the eye behind the wall
        let set = WallSet::build([([-10.0, 0.0, 5.0], [10.0, 0.0, 5.0], 0x60)].into_iter(), 30.0);
        let tree = tree_for(&set);
        let mut eye = Vec3::new(0.0, 3.0, 0.0);
        assert!(set.camera_push(&mut eye, Vec3::new(0.0, 1.0, 10.0), 0.0, false, &tree));
        // crossing at z = 5 (y = 2 there), plus the left normal (0, 0, 1): z = 6; y = 2·3 − 2 = 4
        assert!((eye - Vec3::new(0.0, 4.0, 6.0)).length() < 1e-5, "{eye}");
        // an edge far below the car's ground is passed over, except on Castle
        let mut eye = Vec3::new(0.0, 3.0, 0.0);
        assert!(!set.camera_push(&mut eye, Vec3::new(0.0, 1.0, 10.0), 10.0, false, &tree));
        assert!(set.camera_push(&mut eye, Vec3::new(0.0, 1.0, 10.0), 10.0, true, &tree));
    }

    #[test]
    fn an_eye_beside_a_wall_is_stepped_off_it() {
        // the same wall, its left normal n = (0, 0, 1); an eye 0.5 on the normal's side of the
        // wall line, the car beside it (the first pass finds no crossing)
        let set = WallSet::build([([-10.0, 0.0, 5.0], [10.0, 0.0, 5.0], 0x60)].into_iter(), 30.0);
        let tree = tree_for(&set);
        let mut eye = Vec3::new(0.0, 3.0, 5.5);
        // the probe eye → eye − n = (0, 3, 4.5) reaches the wall at z = 5; the eye becomes z = 6
        assert!(set.camera_push(&mut eye, Vec3::new(3.0, 1.0, 8.0), 0.0, false, &tree));
        assert!((eye - Vec3::new(0.0, 3.0, 6.0)).length() < 1e-5, "{eye}");
        // an eye on the other side is not moved: its probe points away from the wall
        let mut eye = Vec3::new(0.0, 3.0, 4.5);
        assert!(!set.camera_push(&mut eye, Vec3::new(3.0, 1.0, 4.0), 0.0, false, &tree));
        // a short edge wholly inside the 1.0 circle is not listed (0x489DF0)
        let set = WallSet::build([([-0.2, 0.0, 5.0], [0.2, 0.0, 5.0], 0x60)].into_iter(), 30.0);
        let tree = tree_for(&set);
        let mut eye = Vec3::new(0.0, 3.0, 5.5);
        assert!(!set.camera_push(&mut eye, Vec3::new(3.0, 1.0, 8.0), 0.0, false, &tree));
    }

    #[test]
    fn a_sloped_edge_gives_a_shorter_normal() {
        // e = normalize(20, 20, 0): n = (0, 0, 0.7071); crossing at (0, 2, 5) → eye z = 5.7071
        let set = WallSet::build([([-10.0, -10.0, 5.0], [10.0, 10.0, 5.0], 0x60)].into_iter(), 30.0);
        let tree = tree_for(&set);
        let mut eye = Vec3::new(0.0, 3.0, 0.0);
        assert!(set.camera_push(&mut eye, Vec3::new(0.0, 1.0, 10.0), 0.0, true, &tree));
        assert!((eye.z - (5.0 + std::f32::consts::FRAC_1_SQRT_2)).abs() < 1e-5, "{eye}");
    }
}
