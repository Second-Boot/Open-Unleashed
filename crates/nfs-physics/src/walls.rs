//! Wall geometry: the `.edg` cache's edge segments, treated as vertical walls in the xz plane.
//!
//! The *response* is the original's and lives in `vehicle::collide_walls`
//! (`Car_wallCollide` 0x495020 + `WallImpulse` 0x494000, `RE/notes/suspension-collision.md`
//! §6.1/§6.2). What is still unread is the original's collision **world** — the broad phase at
//! 0x49AEC0 over `[obj+0x38]` — so this module does its own segment lookup over the same `.edg`
//! data the game caches.

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
    /// Wall material class 0..5 = metal, wood, snow, ice, earth, rock (`WallImpulse` 0x494000
    /// byte tables 0x49442C / 0x4943BC over the `.edg` word 0x10..0x60; other words → rock).
    pub class: u8,
}

/// `.edg` wall word → material class (tables 0x49442C / 0x4943BC, CONFIRMED from the bytes;
/// the class order metal/wood/snow/ice/earth/rock is HIGH from the per-track word statistics).
/// The class is the CRP material's `mt.flags & 0xF0` carried into the `.edg` word by the rebuild path
/// 0x4750B0 (`RE/notes/suspension-collision.md` §7). `WallImpulse` clamps anything outside
/// `[0x10, 0x60]` to 0x60, and no shipped track has a word inside that range which is not a multiple
/// of 0x10, so the game's seventh ("unknown") table entry is unreachable and rock is the fallback.
pub fn wall_class(word: u32) -> u8 {
    match word {
        0x10 => 0,
        0x20 => 1,
        0x30 => 2,
        0x40 => 3,
        0x50 => 4,
        _ => 5,
    }
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
            .map(|(p1, p2, word)| Wall { a: Vec2::new(p1[0], p1[2]), b: Vec2::new(p2[0], p2[2]), y_mid: (p1[1] + p2[1]) * 0.5, class: wall_class(word) })
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
        let idx = |p: Vec2| ((((p.x - min.x) / cell).floor().max(0.0) as usize).min(nx - 1), (((p.y - min.y) / cell).floor().max(0.0) as usize).min(nz - 1));
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

    /// Nearest wall within `radius` of point `p` (xz) whose height span contains `y`.
    /// Returns (penetration normal pointing away from the wall, penetration depth).
    pub fn nearest(&self, p: Vec3, radius: f32) -> Option<(Vec2, f32)> {
        if self.walls.is_empty() {
            return None;
        }
        let q = Vec2::new(p.x, p.z);
        let idx = |v: Vec2| ((((v.x - self.min.x) / self.cell).floor().max(0.0) as usize).min(self.nx - 1), (((v.y - self.min.y) / self.cell).floor().max(0.0) as usize).min(self.nz - 1));
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
    /// First wall crossed by the xz segment `p0 → p1` (within the wall's height span at `p1.y`).
    /// Returns (edge normal `normalize(−d.z, 0, d.x)`, projection of `p1` onto the edge line,
    /// material class).
    pub fn segment_hit(&self, p0: Vec3, p1: Vec3) -> Option<(Vec2, Vec2, u8)> {
        if self.walls.is_empty() {
            return None;
        }
        let (a, b) = (Vec2::new(p0.x, p0.z), Vec2::new(p1.x, p1.z));
        let idx = |v: Vec2| ((((v.x - self.min.x) / self.cell).floor().max(0.0) as usize).min(self.nx - 1), (((v.y - self.min.y) / self.cell).floor().max(0.0) as usize).min(self.nz - 1));
        let (x0, z0) = idx(a.min(b));
        let (x1, z1) = idx(a.max(b));
        let mut best: Option<(f32, f32, Vec2, Vec2, u8)> = None;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &wi in &self.cells[z * self.nx + x] {
                    let w = &self.walls[wi as usize];
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
                        // 0x4740D0 keeps the crossed edge nearest **in height** to the segment;
                        // ties go to the one crossed first
                        let key = ((w.y_mid - p1.y).abs(), t);
                        if best.map(|b| key < (b.0, b.1)).unwrap_or(true) {
                            let n = Vec2::new(-d.y, d.x).normalize_or_zero();
                            let proj = w.a + d * (((b - w.a).dot(d)) / d.length_squared()).clamp(0.0, 1.0);
                            best = Some((key.0, t, n, proj, w.class));
                        }
                    }
                }
            }
        }
        best.map(|(_, _, n, q, c)| (n, q, c))
    }
}
