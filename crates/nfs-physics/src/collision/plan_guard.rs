//! Guard for the plan-containment helper: the old inline bodies of `find_patch`,
//! `find_patch_height` and `patches_where`, kept verbatim as references and compared on random
//! grids (integer coordinates, so points land exactly on edges and vertices too).

use super::*;

struct Lcg(u32);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0 >> 8
    }
    fn coord(&mut self, span: u32) -> f32 {
        (self.next() % span) as f32 - (span / 2) as f32
    }
}

fn reference_find_patch(g: &TriangleGrid, p: Vec3) -> u32 {
    if g.tris.is_empty() {
        return 0;
    }
    let (cx, cz) = TriangleGrid::cell_of(g.min, g.cell_size, g.nx, g.nz, p);
    let mut best: Option<(f32, u32)> = None;
    for &ti in &g.cells[cz * g.nx + cx] {
        let t = &g.tris[ti as usize];
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

fn reference_find_patch_height(g: &TriangleGrid, p: Vec3) -> Option<(u32, f32)> {
    if g.tris.is_empty() {
        return None;
    }
    let (cx, cz) = TriangleGrid::cell_of(g.min, g.cell_size, g.nx, g.nz, p);
    let mut best: Option<(f32, u32, f32)> = None;
    for &ti in &g.cells[cz * g.nx + cx] {
        let t = &g.tris[ti as usize];
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

fn reference_patches_where(g: &TriangleGrid, p: Vec3, keep: impl Fn(u32) -> bool) -> Vec<(u32, u32)> {
    if g.tris.is_empty() {
        return Vec::new();
    }
    let (cx, cz) = TriangleGrid::cell_of(g.min, g.cell_size, g.nx, g.nz, p);
    let mut out = Vec::new();
    for &ti in &g.cells[cz * g.nx + cx] {
        let t = &g.tris[ti as usize];
        if !keep(t.surface) {
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
        out.push((t.surface, (t.a.y - (n.x * (p.x - t.a.x) + n.z * (p.z - t.a.z)) / n.y).to_bits()));
    }
    out
}

fn bits(v: impl Iterator<Item = (u32, f32)>) -> Vec<(u32, u32)> {
    v.map(|(s, h)| (s, h.to_bits())).collect()
}

#[test]
fn the_three_plan_queries_equal_their_reference_bodies_on_random_grids() {
    let mut r = Lcg(12345);
    let mut hits = 0;
    for round in 0..40 {
        let tris: Vec<Triangle> = (0..60)
            .map(|_| {
                let a = Vec3::new(r.coord(20), r.coord(6), r.coord(20));
                let b = Vec3::new(r.coord(20), r.coord(6), r.coord(20));
                let c = Vec3::new(r.coord(20), r.coord(6), r.coord(20));
                // surface words with and without a nibble, bit 15 set and clear
                let surface = (r.next() % 4) * 0x8000 + r.next() % 17;
                Triangle::new(a, b, c, surface)
            })
            .collect();
        let g = TriangleGrid::build(tris, [3.0, 5.0, 10.0, 40.0][round % 4]);
        for _ in 0..400 {
            let p = Vec3::new(r.coord(24), r.coord(8), r.coord(24));
            assert_eq!(g.find_patch(p), reference_find_patch(&g, p), "find_patch at {p:?}");
            let (a, b) = (g.find_patch_height(p), reference_find_patch_height(&g, p));
            assert_eq!(a.map(|(w, h)| (w, h.to_bits())), b.map(|(w, h)| (w, h.to_bits())), "find_patch_height at {p:?}");
            hits += a.is_some() as u32;
            assert_eq!(bits(g.patches_at(p).map(|(t, h)| (t.surface, h))), reference_patches_where(&g, p, |w| w & 0xF != 0), "patches_at {p:?}");
            let ceiling = |w: u32| w & 0x8000 != 0 && w & 0xF == 0;
            assert_eq!(bits(g.patches_where(p, ceiling).map(|(t, h)| (t.surface, h))), reference_patches_where(&g, p, ceiling), "patches_where {p:?}");
        }
    }
    assert!(hits > 500, "the random grids hit often enough to mean something: {hits}");
}

/// The shared cell index against the expression it replaced (three copies), including NaN, huge
/// and negative inputs.
#[test]
fn grid_cell_equals_the_old_inline_expression() {
    let old = |min: (f32, f32), cell: f32, nx: usize, nz: usize, x: f32, z: f32| {
        ((((x - min.0) / cell).floor().max(0.0) as usize).min(nx - 1), (((z - min.1) / cell).floor().max(0.0) as usize).min(nz - 1))
    };
    let mut r = Lcg(7);
    let specials = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30, -1e30, 0.0, -0.0];
    for i in 0..20_000 {
        let (nx, nz) = (1 + (r.next() % 9) as usize, 1 + (r.next() % 9) as usize);
        let cell = [0.5, 3.0, 10.0, 37.5][i % 4];
        let min = (r.coord(50), r.coord(50));
        let (x, z) = if i % 50 == 0 { (specials[i / 50 % 7], r.coord(90)) } else { (r.coord(90) * 0.37, r.coord(90) * 0.41) };
        assert_eq!(grid_cell(min.0, min.1, cell, nx, nz, x, z), old(min, cell, nx, nz, x, z));
    }
}
