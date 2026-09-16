//! Wall collision against edge segments (APPROXIMATION — the original's collision response
//! 0x49AEC0 / collision world `[obj+0x38]` have not been read; the segment data itself is the
//! game's own `.edg` cache). Segments are treated as vertical walls in the xz plane.

use glam::{Vec2, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Wall {
    pub a: Vec2,
    pub b: Vec2,
    pub y_min: f32,
    pub y_max: f32,
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
    pub fn build(segments: impl Iterator<Item = ([f32; 3], [f32; 3])>, cell: f32) -> Self {
        let walls: Vec<Wall> = segments
            .map(|(p1, p2)| Wall { a: Vec2::new(p1[0], p1[2]), b: Vec2::new(p2[0], p2[2]), y_min: p1[1].min(p2[1]) - 3.0, y_max: p1[1].max(p2[1]) + 3.0 })
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
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &wi in &self.cells[z * self.nx + x] {
                    let w = &self.walls[wi as usize];
                    if p.y < w.y_min || p.y > w.y_max {
                        continue;
                    }
                    let ab = w.b - w.a;
                    let t = ((q - w.a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
                    let c = w.a + ab * t;
                    let d = q - c;
                    let dist = d.length();
                    if dist < radius && best.map(|b| radius - dist > b.1).unwrap_or(true) {
                        let n = if dist > 1e-5 { d / dist } else { Vec2::new(-ab.y, ab.x).normalize_or_zero() };
                        best = Some((n, radius - dist));
                    }
                }
            }
        }
        best
    }
}
