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

    /// The patch's `contains(pt)` (vtable +0xC): the point's plan inside the triangle's, edges
    /// included — the test [`TriangleGrid::patches_at`] makes.
    pub fn contains_xz(&self, p: Vec3) -> bool {
        let side = |a: Vec3, b: Vec3| (b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x);
        let (d0, d1, d2) = (side(self.a, self.b), side(self.b, self.c), side(self.c, self.a));
        (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
    }
}

/// A body's own ground tracker `car+8` (`suspension-collision.md` §1.1) with the plane
/// `car+0x3E8` / `car+0x400` that 0x499520 keeps from it — read 2026-09-25, CONFIRMED unless a
/// line says otherwise.
///
/// * [`Self::update_zone`] is `Tracker::updateZone(pos, keep = 1)` 0x474060: a cached patch that
///   still contains the point is kept and `changed` (`+9`) cleared; otherwise `findPatch`
///   0x473240 — the drivable patch (`word & 0xF`) under the point whose centre height is nearest `p.y`
///   ([`TriangleGrid::patches_at`]) — with `changed = (patch != old)`, the zone word `+0x24`
///   the patch's or 0. The normal `+0xC` (0x474420 → 0x474320) is the patch's; with no patch it
///   keeps the last one's.
/// * [`Self::refresh_plane`] is 0x499520: the plane point `car+0x400` becomes the **patch centre**
///   `¼·Σ point(i), i = 0…3` (0x49953C…0x4995F2) — for a triangle `point(3)` is `point(0)`
///   (0x486030's `i mod 3`), so `(2a + b + c)/4`; a zero result (no patch: `Tracker::point`
///   0x4745F0 answers the zero vector) restores the old one and returns (0x4995F8…0x499642);
///   else, when the zone changed, the plane normal `car+0x3E8` becomes the tracker's
///   (0x499643…0x49966D). The tangent `+0x3F4`, `+0x3DC` and the slice vectors `+0x3B8…` it also
///   writes have no reader in this port.
///
/// The car runs both from **0x49AF70** (`+0xE0C`, 32 Hz, priority 6 — before `Car_simulate` and
/// `Car_ground` at 0x1E; gated on `+0x7F`, `!+0x7D`, `+0x7C`), `Susp_AI` 0x4998C0 runs
/// `updateZone` again without the refresh, and a smackable's setup 0x4A1510 runs 0x4994F0
/// (`update` + `updateZone`) and then 0x499520.
///
/// MEDIUM: the port's patches are the primitives' triangles, so a quad of the original is two
/// patches here and its centre the chosen half's `(2a + b + c)/4`; the `+0xC` normal is the
/// triangle's, turned to face up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatchTracker {
    /// the cached patch `+0x28` (its corners and flags word)
    patch: Option<(Vec3, Vec3, Vec3, u32)>,
    /// `+9`, the zone changed on the last update
    pub changed: bool,
    /// `+0x24` (→ `car+0x2C`), the zone word: the patch's flags word, 0 with no patch
    pub word: u32,
    /// `+0xC`, the patch normal (`Tracker::normal`)
    pub normal: Vec3,
    /// `car+0x3E8`, the plane normal 0x499520 keeps
    pub plane_normal: Vec3,
    /// `car+0x400`, the plane point (the patch centre) 0x499520 keeps; `.y` is `car+0x404`
    pub plane_point: Vec3,
}

impl Default for PatchTracker {
    /// A tracker with no patch — a wheel's `wheel+4` before its first `updateZone`.
    fn default() -> Self {
        Self::new(Vec3::ZERO)
    }
}

impl PatchTracker {
    /// A tracker with no patch yet, its plane `(0, 1, 0)` through `point` (the port's start value;
    /// the original's is whatever the car record held).
    pub fn new(point: Vec3) -> Self {
        Self { patch: None, changed: false, word: 0, normal: Vec3::Y, plane_normal: Vec3::Y, plane_point: point }
    }

    /// `Tracker::setSlice(s)` 0x473210's patch half: the cached patch index `+4 = −1`, the zone
    /// word `+0x24` and the changed flags `+8…+0xA` cleared — so the next `updateZone` finds a
    /// patch afresh and reports a change. The normal `+0xC` and the car's plane are left alone.
    pub fn forget_patch(&mut self) {
        self.patch = None;
        self.changed = false;
        self.word = 0;
    }

    /// `Tracker::updateZone(p, 1)` 0x474060.
    pub fn update_zone(&mut self, grid: &TriangleGrid, p: Vec3) {
        if let Some((a, b, c, w)) = self.patch
            && Triangle::new(a, b, c, w).contains_xz(p)
        {
            self.changed = false;
            return;
        }
        let old = self.patch;
        // `findPatch` 0x473240: the patch whose centre height is nearest `p.y`, the first of equals
        let best = grid.patches_at(p).map(|(t, _)| t).fold(None::<&Triangle>, |b, t| match b {
            Some(b) if (patch_centre_y(b) - p.y).abs() <= (patch_centre_y(t) - p.y).abs() => Some(b),
            _ => Some(t),
        });
        self.patch = best.map(|t| (t.a, t.b, t.c, t.surface));
        self.changed = self.patch != old;
        if let Some((a, b, c, w)) = self.patch {
            self.word = w & 0xFFFF;
            let mut n = (b - a).cross(c - a).normalize_or_zero();
            if n.y < 0.0 {
                n = -n;
            }
            self.normal = n;
        } else {
            self.word = 0;
        }
    }

    /// The cached patch's plane height at `(x, z)` — `Tracker::point(0)` 0x4745F0 and
    /// `Tracker::normal()` 0x474420 into `p0.y − ((x − p0.x)·n.x + (z − p0.z)·n.z)/n.y`, the form
    /// 0x494480 and `Susp_update` (0x499C37…0x499C7B) evaluate. `None` with no patch.
    pub fn patch_height(&self, x: f32, z: f32) -> Option<f32> {
        let (p0, _, _, _) = self.patch?;
        let n = self.normal;
        Some(p0.y - ((x - p0.x) * n.x + (z - p0.z) * n.z) / n.y)
    }

    /// 0x499520, `Car_refreshGroundPlane`.
    pub fn refresh_plane(&mut self) {
        let centre = self.patch.map(|(a, b, c, _)| (a * 2.0 + b + c) * 0.25).unwrap_or(Vec3::ZERO);
        if centre == Vec3::ZERO {
            return;
        }
        self.plane_point = centre;
        if self.changed {
            self.plane_normal = self.normal;
        }
    }

    /// `Car_placeAtSlice` 0x49B090's tracker part (0x49B14E…0x49B1F9, 0x49B2D2…0x49B30D):
    /// `updateZone(p, 1)`, then the plane point `car+0x400` = the mean of the patch's
    /// `point(0…3)` (a triangle's `point(3)` is `point(0)`) and the plane normal `car+0x3E8` = the
    /// patch normal — both unconditionally, unlike 0x499520. With no patch the port keeps the old
    /// plane (the original averages whatever `point(i)` returns then — not traced).
    pub fn seat(&mut self, grid: &TriangleGrid, p: Vec3) {
        self.update_zone(grid, p);
        if let Some((a, b, c, _)) = self.patch {
            self.plane_point = (a * 2.0 + b + c) * 0.25;
            self.plane_normal = self.normal;
        }
    }

    /// 0x49AF70's part of it: `updateZone(pos, 1)` and, when the zone changed, 0x499520.
    pub fn track(&mut self, grid: &TriangleGrid, pos: Vec3) {
        self.update_zone(grid, pos);
        if self.changed {
            self.refresh_plane();
        }
    }

    /// `p.y − ((x − q.x)·n.x + (z − q.z)·n.z)/n.y` on the kept plane, `Susp_update`'s fallback
    /// for a wheel with no usable patch (0x499BC4…0x499C2E, no roughness added): `car+0x3E8` when
    /// `car+0x3EC > 0.1`, else `(0, 1, 0)`, through `car+0x400`.
    pub fn plane_height(&self, x: f32, z: f32) -> f32 {
        let q = self.plane_point;
        let n = self.fallback_normal();
        q.y - ((x - q.x) * n.x + (z - q.z) * n.z) / n.y
    }

    /// The normal of that fallback plane — `car+0x3E8` when `car+0x3EC > 0.1` (`[0x5B2604]`,
    /// 0x499BCA `test ah,0x41; jne`), else `(0, 1, 0)` (0x499BF3…0x499C01) — which the branch
    /// also leaves as the wheel's normal `wheel+0x4C` (0x499D0C…0x499D12: `n.x` is still on the
    /// FPU stack, `n.y`/`n.z` in `[esp+0x20]`/`[esp+0x24]`).
    pub fn fallback_normal(&self) -> Vec3 {
        if self.plane_normal.y > 0.1 { self.plane_normal } else { Vec3::Y }
    }
}

/// A patch's centre height, the key `findPatch` 0x473240 sorts by: the triangle's vtable +4
/// 0x485E40 answers `(a + b + c)/3` (`[0x5B2680]`).
fn patch_centre_y(t: &Triangle) -> f32 {
    (t.a.y + t.b.y + t.c.y) * (1.0 / 3.0)
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
    max: Vec3,
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
        Self { tris, cells, min, max, cell_size, nx, nz }
    }

    /// The box every triangle lies in (zero for an empty grid).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        (self.min, self.max)
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
    /// (`word & 0xF`, 0x4732CC), the one whose **centre** height is nearest `p.y` — at any
    /// distance (0x4732D2…0x473303: vtable +4 is the patch centre, `(a + b + c)/3` for a triangle
    /// 0x485E40, `¼·Σ` of a quad's four points 0x4855C0; `|centre.y − p.y|`, a strict `<` keeps
    /// the first of equals). The word, or 0 when there is none (0x474060's
    /// `zoneWord = patch ? word : 0`). *(Until 2026-09-26 the port compared the plane's height
    /// at the point.)*
    ///
    /// The containment is exact: each edge's x-z cross product is stored as a float and tested
    /// as a signed integer (`jg` / `jl` on its bits, the patch's winding byte `+8` choosing the
    /// sign), so a point on an edge is inside and there is no epsilon.
    ///
    /// MEDIUM on two counts: the port's patches are the primitives' triangles, so a quad is
    /// its two halves and its centre each half's, and the tracker's `keep` (a cached patch
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
            let d = (patch_centre_y(t) - p.y).abs();
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
            let d = (patch_centre_y(t) - p.y).abs();
            if best.map(|(bd, _, _)| d < bd).unwrap_or(true) {
                best = Some((d, t.surface, h));
            }
        }
        best.map(|(_, w, h)| (w, h))
    }

    /// Every patch of the point's cell with a surface nibble (`word & 0xF`) whose plan contains
    /// the point, with its plane height there — the list 0x473650 builds (its radius only picks
    /// the cells, 0x484630; the footprint test vt+0xC and the nibble 0x473705 pick the patches) and
    /// 0x48A060's `h = −(n·v0 … )/n.y` on it. Same MEDIUM as [`Self::find_patch`]: a quad is its two
    /// triangles.
    pub fn patches_at(&self, p: Vec3) -> impl Iterator<Item = (&Triangle, f32)> + '_ {
        self.patches_where(p, |w| w & 0xF != 0)
    }

    /// [`Self::patches_at`] with another test on the flags word — 0x473420, the ceiling list, keeps
    /// the words with bit 15 set and **no** surface nibble (0x4734D6…0x4734E5).
    pub fn patches_where(&self, p: Vec3, keep: impl Fn(u32) -> bool + 'static) -> impl Iterator<Item = (&Triangle, f32)> + '_ {
        let cell = if self.tris.is_empty() {
            &[][..]
        } else {
            let (cx, cz) = Self::cell_of(self.min, self.cell_size, self.nx, self.nz, p);
            &self.cells[cz * self.nx + cx][..]
        };
        cell.iter().filter_map(move |&ti| {
            let t = &self.tris[ti as usize];
            if !keep(t.surface) {
                return None;
            }
            let side = |a: Vec3, b: Vec3| (b.x - a.x) * (p.z - a.z) - (b.z - a.z) * (p.x - a.x);
            let (d0, d1, d2) = (side(t.a, t.b), side(t.b, t.c), side(t.c, t.a));
            let inside = (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0);
            if !inside {
                return None;
            }
            let n = (t.b - t.a).cross(t.c - t.a);
            if n.y.abs() < 1e-9 {
                return None;
            }
            Some((t, t.a.y - (n.x * (p.x - t.a.x) + n.z * (p.z - t.a.z)) / n.y))
        })
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

    /// 0x474060 + 0x499520: the plane point is the patch centre `(2a + b + c)/4`, kept while the
    /// cached patch still holds the point and kept again when the point leaves every patch.
    #[test]
    fn the_tracker_keeps_the_patch_centre() {
        let g = TriangleGrid::build(quad(1.5), 5.0);
        let mut t = PatchTracker::new(Vec3::ZERO);
        t.track(&g, Vec3::new(5.0, 2.0, -3.0));
        // the first triangle (a, b, c) = (−10,−10), (10,−10), (10,10): centre (−10+10+10−10)/4 …
        assert!(t.changed && t.word == 7);
        assert_eq!(t.plane_point, Vec3::new(0.0, 1.5, -5.0));
        assert_eq!(t.plane_normal, Vec3::Y);
        // still inside the same triangle: nothing changes
        t.track(&g, Vec3::new(6.0, 2.0, -4.0));
        assert!(!t.changed);
        // off every patch: no word, the old plane stays
        t.track(&g, Vec3::new(50.0, 2.0, 50.0));
        assert!(t.changed && t.word == 0);
        assert_eq!(t.plane_point, Vec3::new(0.0, 1.5, -5.0));
        assert!((t.plane_height(30.0, 30.0) - 1.5).abs() < 1e-6);
    }
}
