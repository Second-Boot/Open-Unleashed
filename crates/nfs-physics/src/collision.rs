//! Uniform-grid triangle soup for ground-height and ray queries.

use glam::Vec3;

#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    /// Caller-defined surface id (e.g. material index) returned by queries.
    pub surface: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    pub surface: u32,
}

/// Static triangle grid over the XZ plane.
#[derive(Debug, Clone)]
pub struct TriangleGrid {
    tris: Vec<Triangle>,
    cells: Vec<Vec<u32>>,
    min: Vec3,
    cell_size: f32,
    nx: usize,
    nz: usize,
}

impl TriangleGrid {
    pub fn build(tris: Vec<Triangle>, cell_size: f32) -> Self {
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for t in &tris {
            for p in [t.a, t.b, t.c] {
                min = min.min(p);
                max = max.max(p);
            }
        }
        if tris.is_empty() {
            min = Vec3::ZERO;
            max = Vec3::ZERO;
        }
        let nx = (((max.x - min.x) / cell_size).ceil() as usize).max(1);
        let nz = (((max.z - min.z) / cell_size).ceil() as usize).max(1);
        let mut cells = vec![Vec::new(); nx * nz];
        for (i, t) in tris.iter().enumerate() {
            let (x0, z0) = Self::cell_of(min, cell_size, nx, nz, t.a.min(t.b).min(t.c));
            let (x1, z1) = Self::cell_of(min, cell_size, nx, nz, t.a.max(t.b).max(t.c));
            for z in z0..=z1 {
                for x in x0..=x1 {
                    cells[z * nx + x].push(i as u32);
                }
            }
        }
        Self { tris, cells, min, cell_size, nx, nz }
    }

    fn cell_of(min: Vec3, cs: f32, nx: usize, nz: usize, p: Vec3) -> (usize, usize) {
        let x = (((p.x - min.x) / cs).floor().max(0.0) as usize).min(nx - 1);
        let z = (((p.z - min.z) / cs).floor().max(0.0) as usize).min(nz - 1);
        (x, z)
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Nearest intersection along `dir` from `origin` within `max_dist`, searching the cells the
    /// ray's XZ bounding box covers (adequate for short wheel/wall rays).
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<Hit> {
        if self.tris.is_empty() {
            return None;
        }
        let end = origin + dir * max_dist;
        let (x0, z0) = Self::cell_of(self.min, self.cell_size, self.nx, self.nz, origin.min(end));
        let (x1, z1) = Self::cell_of(self.min, self.cell_size, self.nx, self.nz, origin.max(end));
        let mut best: Option<Hit> = None;
        for z in z0..=z1 {
            for x in x0..=x1 {
                for &ti in &self.cells[z * self.nx + x] {
                    let t = &self.tris[ti as usize];
                    if let Some((d, n)) = ray_triangle(origin, dir, t)
                        && d <= max_dist
                        && best.map(|b| d < b.distance).unwrap_or(true)
                    {
                        best = Some(Hit { point: origin + dir * d, normal: n, distance: d, surface: t.surface });
                    }
                }
            }
        }
        best
    }

    /// Ground under (x, z): highest surface below `y_from` within `depth`.
    pub fn ground(&self, x: f32, y_from: f32, z: f32, depth: f32) -> Option<Hit> {
        self.raycast(Vec3::new(x, y_from, z), Vec3::NEG_Y, depth)
    }
}

/// Möller–Trumbore, two-sided. Returns (distance, unit normal facing the ray origin).
fn ray_triangle(o: Vec3, d: Vec3, t: &Triangle) -> Option<(f32, Vec3)> {
    let e1 = t.b - t.a;
    let e2 = t.c - t.a;
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t.a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = e2.dot(q) * inv;
    if dist <= 0.0 {
        return None;
    }
    let mut n = e1.cross(e2).normalize();
    if n.dot(d) > 0.0 {
        n = -n;
    }
    Some((dist, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32) -> Vec<Triangle> {
        let (a, b, c, d) = (Vec3::new(-10.0, y, -10.0), Vec3::new(10.0, y, -10.0), Vec3::new(10.0, y, 10.0), Vec3::new(-10.0, y, 10.0));
        vec![Triangle { a, b, c, surface: 7 }, Triangle { a, b: c, c: d, surface: 7 }]
    }

    #[test]
    fn ground_hit_on_flat_quad() {
        let g = TriangleGrid::build(quad(1.5), 5.0);
        let h = g.ground(0.5, 10.0, -3.0, 20.0).unwrap();
        assert!((h.point.y - 1.5).abs() < 1e-5);
        assert!((h.normal - Vec3::Y).length() < 1e-5);
        assert_eq!(h.surface, 7);
        assert!(g.ground(50.0, 10.0, 50.0, 20.0).is_none());
    }

    #[test]
    fn picks_nearest_surface() {
        let mut t = quad(0.0);
        t.extend(quad(2.0));
        let g = TriangleGrid::build(t, 5.0);
        let h = g.ground(0.0, 10.0, 0.0, 20.0).unwrap();
        assert!((h.point.y - 2.0).abs() < 1e-5);
    }
}
