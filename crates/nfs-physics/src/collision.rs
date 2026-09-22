//! Uniform-grid triangle soup for ground-height and ray queries.

use glam::Vec3;

#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    /// Caller-defined surface id (e.g. material index) returned by queries.
    pub surface: u32,
    /// The three vertex colours, 0..255 per channel. The original interpolates the ground patch's
    /// vertex colours at the car's position to light the car (0x474700, `lighting.md`); a hit
    /// returns the same value interpolated over the triangle.
    pub colors: [[u8; 4]; 3],
}

impl Triangle {
    /// A triangle with white vertices (for callers that have no colours).
    pub fn new(a: Vec3, b: Vec3, c: Vec3, surface: u32) -> Self {
        Self { a, b, c, surface, colors: [[255; 4]; 3] }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    pub surface: u32,
    /// The triangle's vertex colours interpolated at the hit, 0..1 per channel.
    pub color: [f32; 4],
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

    /// Mean vertex colour of the triangles a predicate accepts (used to normalise the ground light
    /// a car picks up against the track's own average).
    pub fn mean_color(&self, keep: &dyn Fn(u32) -> bool) -> [f32; 3] {
        let mut sum = [0.0f64; 3];
        let mut n = 0.0f64;
        for t in &self.tris {
            if !keep(t.surface) {
                continue;
            }
            for c in &t.colors {
                for k in 0..3 {
                    sum[k] += c[k] as f64 / 255.0;
                }
            }
            n += 3.0;
        }
        if n == 0.0 {
            return [1.0; 3];
        }
        [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32]
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
                    if let Some((d, n, u, v)) = ray_triangle(origin, dir, t)
                        && d <= max_dist
                        && best.map(|b| d < b.distance).unwrap_or(true)
                    {
                        let w = 1.0 - u - v;
                        let ch = |k: usize| (t.colors[0][k] as f32 * w + t.colors[1][k] as f32 * u + t.colors[2][k] as f32 * v) / 255.0;
                        best = Some(Hit {
                            point: origin + dir * d,
                            normal: n,
                            distance: d,
                            surface: t.surface,
                            color: [ch(0), ch(1), ch(2), ch(3)],
                        });
                    }
                }
            }
        }
        best
    }

    /// `Tracker::findPatch` 0x473240 as `Tracker::updateZone` 0x474060 uses it: of the patches
    /// in the point's cell whose **plan** contains it (0x486090 / 0x485780 test the edges in the
    /// x-z plane only, 0x4860B9…0x486176) and whose flags word has a surface nibble
    /// (`word & 0xF`, 0x4732CC), the one whose height at the point is nearest `p.y` — at any
    /// distance (0x4732DE…0x473303). The word, or 0 when there is none (0x474060's
    /// `zoneWord = patch ? word : 0`).
    ///
    /// MEDIUM on two counts: the port's patches are the primitives' triangles, so a quad is
    /// its two halves and its height their planes', and the tracker's `keep` (a cached patch
    /// that still contains the point is kept) is not carried — the nearest is taken every time.
    pub fn find_patch(&self, p: Vec3) -> u32 {
        if self.tris.is_empty() {
            return 0;
        }
        let (cx, cz) = Self::cell_of(self.min, self.cell_size, self.nx, self.nz, p);
        let mut best: Option<(f32, u32)> = None;
        for &ti in &self.cells[cz * self.nx + cx] {
            let t = &self.tris[ti as usize];
            if t.surface & 0xF == 0 {
                continue;
            }
            let side = |a: Vec3, b: Vec3| (b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x);
            let (d0, d1, d2) = (side(t.a, t.b), side(t.b, t.c), side(t.c, t.a));
            let inside = (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0);
            if !inside {
                continue;
            }
            let n = (t.b - t.a).cross(t.c - t.a);
            if n.y.abs() < 1e-9 {
                continue;
            }
            let h = t.a.y - (n.x * (p.x - t.a.x) + n.z * (p.z - t.a.z)) / n.y;
            let d = (h - p.y).abs();
            if best.map(|(bd, _)| d < bd).unwrap_or(true) {
                best = Some((d, t.surface));
            }
        }
        best.map(|(_, w)| w).unwrap_or(0)
    }

    /// The same query as [`Self::find_patch`], also answering the chosen patch's **plane height**
    /// at the point — what `Car_wallCollide` 0x495020 needs: after `updateZone(corner, 1)` it
    /// evaluates the patch plane through vertex 0 (`Tracker::point(0)` 0x4745F0 and
    /// `Tracker::normal()` 0x474420) at the corner,
    /// `h = p0.y - ((p.x - p0.x)·n.x + (p.z - p0.z)·n.z) / n.y` (0x4952A0…0x4952B6), and skips the
    /// corner when `|h - car+0x334| <= 4.0` (`[0x5B2404]`, 0x4952B9…0x4952D9).
    ///
    /// `None` when no drivable patch contains the point in plan — then the wall test runs.
    pub fn find_patch_height(&self, p: Vec3) -> Option<(u32, f32)> {
        if self.tris.is_empty() {
            return None;
        }
        let (cx, cz) = Self::cell_of(self.min, self.cell_size, self.nx, self.nz, p);
        let mut best: Option<(f32, u32, f32)> = None;
        for &ti in &self.cells[cz * self.nx + cx] {
            let t = &self.tris[ti as usize];
            if t.surface & 0xF == 0 {
                continue;
            }
            let side = |a: Vec3, b: Vec3| (b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x);
            let (d0, d1, d2) = (side(t.a, t.b), side(t.b, t.c), side(t.c, t.a));
            let inside = (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0);
            if !inside {
                continue;
            }
            let n = (t.b - t.a).cross(t.c - t.a);
            if n.y.abs() < 1e-9 {
                continue;
            }
            let h = t.a.y - (n.x * (p.x - t.a.x) + n.z * (p.z - t.a.z)) / n.y;
            let d = (h - p.y).abs();
            if best.map(|(bd, _, _)| d < bd).unwrap_or(true) {
                best = Some((d, t.surface, h));
            }
        }
        best.map(|(_, w, h)| (w, h))
    }

    /// Ground under (x, z): highest surface below `y_from` within `depth`.
    pub fn ground(&self, x: f32, y_from: f32, z: f32, depth: f32) -> Option<Hit> {
        self.raycast(Vec3::new(x, y_from, z), Vec3::NEG_Y, depth)
    }

    /// What is over the point: the **lowest** surface more than `clearance` above `(x, y, z)`,
    /// within `depth` of it.
    ///
    /// This is the shape of the original's enclosure query 0x473330, which walks the patches of the
    /// quadtree cell the position falls in, keeps those whose height above the position is more
    /// than 3.5 m, and returns the flags word of the lowest one (`race-logic.md` §12).
    pub fn ceiling(&self, x: f32, y: f32, z: f32, clearance: f32, depth: f32) -> Option<Hit> {
        self.raycast(Vec3::new(x, y + clearance, z), Vec3::Y, depth)
    }
}

/// Möller–Trumbore, two-sided. Returns (distance, unit normal facing the ray origin, u, v) where
/// `u`/`v` are the barycentric weights of `b` and `c`.
fn ray_triangle(o: Vec3, d: Vec3, t: &Triangle) -> Option<(f32, Vec3, f32, f32)> {
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
    Some((dist, n, u, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(y: f32) -> Vec<Triangle> {
        let (a, b, c, d) = (Vec3::new(-10.0, y, -10.0), Vec3::new(10.0, y, -10.0), Vec3::new(10.0, y, 10.0), Vec3::new(-10.0, y, 10.0));
        vec![Triangle::new(a, b, c, 7), Triangle::new(a, c, d, 7)]
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

    /// `Tracker::findPatch` 0x473240: the drivable patch nearest in height over the point, at any
    /// height; a surface nibble of 0 is skipped; nothing under the point answers 0.
    #[test]
    fn find_patch_takes_the_nearest_drivable_patch() {
        let mut t = quad(0.0);
        t.extend(quad(40.0).into_iter().map(|mut q| {
            q.surface = 0x13;
            q
        }));
        t.extend(quad(3.0).into_iter().map(|mut q| {
            q.surface = 0x20;
            q
        }));
        let g = TriangleGrid::build(t, 5.0);
        assert_eq!(g.find_patch(Vec3::new(1.0, 2.0, 1.0)), 7, "the wall class at 3 m is not a surface");
        assert_eq!(g.find_patch(Vec3::new(1.0, 30.0, 1.0)), 0x13, "no height window");
        assert_eq!(g.find_patch(Vec3::new(1.0, -100.0, 1.0)), 7);
        assert_eq!(g.find_patch(Vec3::new(30.0, 0.0, 30.0)), 0, "off the patches");
    }
}
