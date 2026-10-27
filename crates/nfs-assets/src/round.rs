//! ENHANCEMENT E42 «скругление деталей»: a render-only, procedural refinement of the round parts of a car.
//!
//! The original's parts are very coarse (the 993's wheel is a 16-gon, the mirrors and lamps a dozen triangles) but their normals are
//! authored and smooth. Two refiners work on the expanded meshes of [`crate::mesh::article_meshes`] (three own vertices per triangle):
//!
//! * [`refine_wheel`]: a wheel is a surface of revolution about its local X axis, so every triangle is cut into `n x n` and the
//!   new points are placed by interpolating the corners' CYLINDRICAL coordinates (x, radius, angle): the 16-gon becomes a true circle
//!   of the same radius (the corners keep their place), the flat faces stay flat, the authored normals are interpolated in the
//!   (axial, radial, tangential) frame so they stay axisymmetric, and the UVs stay the original's linear mapping (barycentric).
//!   Triangles that are not part of the surface of revolution (the 993's square rim billboard) are copied unchanged.
//! * [`refine_pn`]: curved PN triangles (Vlachos 2001) from the authored positions and normals. Edges that are open (one triangle),
//!   shared by more than two, or shared with a different normal on either end (an authored hard edge) are left STRAIGHT on both
//!   sides, so a refined part has no cracks inside and keeps its outline where it meets an unrefined neighbour. Only for the
//!   whitelisted round parts ([`part_kind`]): the research (`models_research/REPORT.md` §2.2) shows PN rippling on the stacked body
//!   layers.
//!
//! Both return a [`Resample`], the recipe of the new vertices as barycentric blends of the source triangle's corners, so a part's other
//! per-vertex streams (the damage deltas, the zones) are carried through the same way: [`Resample::blend3`] / [`Resample::nearest`].
//! Nothing here reads or writes the simulation; the caller applies it after the car's measurements (box, tyre radius).

use glam::Vec3;
use nfs_renderer::{MeshData, Vertex};
use std::collections::HashMap;

/// How a part is refined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// a wheel / tyre: resample the circle ([`refine_wheel`])
    Wheel,
    /// a mirror, lamp lens, exhaust tip, steering wheel: curved PN triangles ([`refine_pn`])
    Smooth,
}

/// The whitelist of round parts, by article name with the digit / letter variant suffix stripped (`Light2a` -> `Light`, `SideMirrorLeft`
/// -> `SideMirror`). Wheels are not named: the caller passes `is_wheel` (the article's type 0x0D / 0x0E). See `ENHANCEMENTS.md` E42 for the
/// per-car census.
pub fn part_kind(name: &str, is_wheel: bool) -> Option<Kind> {
    if is_wheel {
        return Some(Kind::Wheel);
    }
    let base = family(name);
    ROUND_PARTS.iter().any(|p| *p == base).then_some(Kind::Smooth)
}

/// The articles refined with PN (after [`family`]): the side mirrors, the lamp lenses, the exhaust tips, the steering wheel, the roof siren.
pub const ROUND_PARTS: &[&str] = &["SideMirror", "GtMirror", "Light", "HeadLight", "Exhaust", "Steer", "Siren"];

/// An article name without the trailing variant marks: `Light2a` -> `Light`, `Exhaust1` -> `Exhaust`, `SideMirrorLeft` -> `SideMirror`.
pub fn family(name: &str) -> &str {
    let n = name.trim_end();
    let n = n.strip_suffix("Left").or_else(|| n.strip_suffix("Right")).unwrap_or(n);
    let n = n.trim_end_matches(|c: char| c.is_ascii_digit());
    // a single lower-case variant letter after digits (`Light2a`, `Light4a`, `Steer1`, `Exhaust1`)
    let n2 = n.strip_suffix(|c: char| c.is_ascii_lowercase()).filter(|s| s.ends_with(|c: char| c.is_ascii_digit())).unwrap_or(n);
    n2.trim_end_matches(|c: char| c.is_ascii_digit())
}

/// One new vertex: the source triangle's three vertex indices (into the source mesh's `vertices`) and the barycentric weights.
#[derive(Debug, Clone)]
pub struct Resample {
    pub src: Vec<([u32; 3], [f32; 3])>,
    /// the largest distance a new point moved from the flat (linear) position, metres (PN only)
    pub moved: f32,
}

impl Resample {
    /// A per-vertex vector stream (positions, deltas) blended linearly.
    pub fn blend3(&self, v: &[[f32; 3]]) -> Vec<[f32; 3]> {
        self.src
            .iter()
            .map(|(s, w)| {
                let mut o = [0.0f32; 3];
                for c in 0..3 {
                    for k in 0..3 {
                        o[c] += v[s[k] as usize][c] * w[k];
                    }
                }
                o
            })
            .collect()
    }

    /// A per-vertex tag (the damage zone): the corner with the largest weight.
    pub fn nearest<T: Copy>(&self, v: &[T]) -> Vec<T> {
        self.src
            .iter()
            .map(|(s, w)| {
                let k = if w[0] >= w[1] && w[0] >= w[2] { 0 } else if w[1] >= w[2] { 1 } else { 2 };
                v[s[k] as usize]
            })
            .collect()
    }
}

/// The grid of one triangle cut into `n x n`: vertex `(i, j)` (`0 <= i + j <= n`) at barycentric `(1 - u - v, u, v)`, `u = i / n`,
/// `v = j / n`; the triangles keep the source winding. Pushes the indices into `out` for vertices starting at `base`.
fn grid_indices(n: usize, base: u32, out: &mut Vec<u32>) {
    // row j holds n + 1 - j vertices; the rows before it hold sum_{r < j} (n + 1 - r) = j (n + 1) - j (j - 1) / 2
    let row = |j: usize| -> u32 { base + (j * (n + 1) - (j * j.saturating_sub(1)) / 2) as u32 };
    let at = |i: usize, j: usize| -> u32 { row(j) + i as u32 };
    for j in 0..n {
        for i in 0..n - j {
            out.extend_from_slice(&[at(i, j), at(i + 1, j), at(i, j + 1)]);
            if i + 1 + j < n {
                out.extend_from_slice(&[at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
            }
        }
    }
}

/// The (u, v) grid points in row order (the order [`grid_indices`] assumes).
fn grid_points(n: usize) -> impl Iterator<Item = [f32; 3]> {
    (0..=n).flat_map(move |j| (0..=n - j).map(move |i| {
        let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
        [1.0 - u - v, u, v]
    }))
}

/// `sum_k w[k] * c[k]` over three 3-vectors.
fn blend(w: [f32; 3], c: [[f32; 3]; 3]) -> [f32; 3] {
    let mut o = [0.0f32; 3];
    for k in 0..3 {
        for m in 0..3 {
            o[k] += w[m] * c[m][k];
        }
    }
    o
}

/// A vertex with the UV and colour blended by `w`; the caller sets the position and normal.
fn mix_vertex(v: [&Vertex; 3], w: [f32; 3]) -> Vertex {
    let uv = [0, 1].map(|c| v[0].uv[c] * w[0] + v[1].uv[c] * w[1] + v[2].uv[c] * w[2]);
    let color = [0, 1, 2, 3].map(|c| v[0].color[c] * w[0] + v[1].color[c] * w[1] + v[2].color[c] * w[2]);
    Vertex { position: [0.0; 3], normal: [0.0; 3], uv, color }
}

/// Resample a wheel mesh (surface of revolution about local X) so its circle has at least `min_segments` segments. `None` when the
/// mesh is not such a surface or already round enough (then the caller keeps the original). `max_n` caps the cut.
pub fn refine_wheel(mesh: &MeshData, min_segments: u32, max_n: usize) -> Option<(MeshData, Resample)> {
    const EPS_R: f32 = 1e-3;
    const MAX_SPAN: f32 = 1.05; // rad: a triangle spanning more than 60 degrees about the axle is not a sector of the circle
    let ntri = mesh.indices.len() / 3;
    if ntri == 0 {
        return None;
    }
    let cyl = |v: &Vertex| -> (f32, f32, f32) { (v.position[0], v.position[1].hypot(v.position[2]), v.position[2].atan2(v.position[1])) };
    let wrap = |a: f32| -> f32 {
        let t = std::f32::consts::TAU;
        a - t * (a / t).round()
    };
    // each triangle's corners (x, r, theta unwrapped) or None when it does not belong to the surface of revolution
    let mut tris: Vec<Option<[(f32, f32, f32); 3]>> = Vec::with_capacity(ntri);
    for t in 0..ntri {
        let c: [(f32, f32, f32); 3] = [0, 1, 2].map(|k| cyl(&mesh.vertices[mesh.indices[3 * t + k] as usize]));
        let Some(r0) = c.iter().position(|q| q.1 > EPS_R) else {
            tris.push(None);
            continue;
        };
        let t0 = c[r0].2;
        let mut u = c;
        for q in u.iter_mut() {
            if q.1 > EPS_R {
                q.2 = t0 + wrap(q.2 - t0);
            } else {
                q.2 = t0;
            }
        }
        // the spread of the angles among the corners that have one
        let (lo, hi) = u.iter().filter(|q| q.1 > EPS_R).fold((f32::MAX, f32::MIN), |(l, h), q| (l.min(q.2), h.max(q.2)));
        let span = hi - lo;
        if span <= MAX_SPAN {
            tris.push(Some(u));
        } else {
            tris.push(None);
        }
    }
    // billboards: drop the triangles whose largest radius is far over the typical one
    let mut radii: Vec<f32> = tris.iter().flatten().flat_map(|c| c.iter().map(|q| q.1)).collect();
    if radii.is_empty() {
        return None;
    }
    radii.sort_by(|a, b| a.total_cmp(b));
    let rmax = radii[radii.len() * 98 / 100];
    for t in tris.iter_mut() {
        if t.as_ref().is_some_and(|c| c.iter().any(|q| q.1 > rmax * 1.08)) {
            *t = None;
        }
    }
    // the silhouette ring decides: the angular step of the triangles that reach the outer radius (the tread and the outer annulus)
    let span = tris
        .iter()
        .flatten()
        .filter(|c| c.iter().any(|q| q.1 > rmax * 0.9))
        .map(|c| {
            let ang: Vec<f32> = c.iter().filter(|q| q.1 > EPS_R).map(|q| q.2).collect();
            ang.iter().cloned().fold(f32::MIN, f32::max) - ang.iter().cloned().fold(f32::MAX, f32::min)
        })
        .fold(0.0f32, f32::max);
    if span <= 1e-4 {
        return None;
    }
    // segments the original has about the axle = 2 pi / span; cut so that the circle has min_segments
    let have = std::f32::consts::TAU / span;
    let n = (min_segments as f32 / have - 0.02).ceil() as usize;
    if n <= 1 {
        return None;
    }
    let n = n.min(max_n);
    let mut out = MeshData::default();
    let mut rs = Resample { src: Vec::new(), moved: 0.0 };
    let pts: Vec<[f32; 3]> = grid_points(n).collect();
    for t in 0..ntri {
        let s: [u32; 3] = [0, 1, 2].map(|k| mesh.indices[3 * t + k]);
        let v: [&Vertex; 3] = s.map(|i| &mesh.vertices[i as usize]);
        match &tris[t] {
            None => {
                let base = out.vertices.len() as u32;
                for k in 0..3 {
                    out.vertices.push(*v[k]);
                    let mut w = [0.0; 3];
                    w[k] = 1.0;
                    rs.src.push((s, w));
                }
                out.indices.extend_from_slice(&[base, base + 1, base + 2]);
            }
            Some(c) => {
                // corner normals in the (axial, radial, tangential) frame of their own angle
                let nfr: [[f32; 3]; 3] = [0, 1, 2].map(|k| {
                    let nn = Vec3::from(v[k].normal);
                    let (st, ct) = c[k].2.sin_cos();
                    [nn.x, nn.y * ct + nn.z * st, -nn.y * st + nn.z * ct]
                });
                let base = out.vertices.len() as u32;
                for w in &pts {
                    let x = w[0] * c[0].0 + w[1] * c[1].0 + w[2] * c[2].0;
                    let r = w[0] * c[0].1 + w[1] * c[1].1 + w[2] * c[2].1;
                    let (mut wt, mut ws) = (0.0f32, 0.0f32);
                    for k in 0..3 {
                        if c[k].1 > EPS_R {
                            wt += w[k] * c[k].2;
                            ws += w[k];
                        }
                    }
                    let th = if ws > 1e-6 { wt / ws } else { c.iter().find(|q| q.1 > EPS_R).map_or(0.0, |q| q.2) };
                    let (st, ct) = th.sin_cos();
                    let nf = blend(*w, nfr);
                    let normal = Vec3::new(nf[0], nf[1] * ct - nf[2] * st, nf[1] * st + nf[2] * ct);
                    let normal = if normal.length_squared() > 1e-12 { normal.normalize() } else { Vec3::from(v[0].normal) };
                    let mut mv = mix_vertex(v, *w);
                    mv.position = [x, r * ct, r * st];
                    mv.normal = normal.to_array();
                    out.vertices.push(mv);
                    rs.src.push((s, *w));
                }
                grid_indices(n, base, &mut out.indices);
            }
        }
    }
    Some((out, rs))
}

// ---- PN triangles ----

type PosKey = [i32; 3];

fn key(p: [f32; 3]) -> PosKey {
    p.map(|x| (x * 1.0e4).round() as i32)
}

/// Refine `mesh` with PN triangles cut `n x n`; `None` when nothing would move by more than 2 mm (flat parts) or `n < 2`.
/// `crease_cos`: two normals at a shared edge end whose dot is below it make the edge a hard one (left straight).
pub fn refine_pn(mesh: &MeshData, n: usize, crease_cos: f32) -> Option<(MeshData, Resample)> {
    let ntri = mesh.indices.len() / 3;
    if n < 2 || ntri == 0 {
        return None;
    }
    let vert = |t: usize, k: usize| &mesh.vertices[mesh.indices[3 * t + k] as usize];
    // edges: per welded pair, how many triangles hold it and whether their normals agree at both ends
    struct Edge {
        count: u32,
        hard: bool,
        na: Vec3,
        nb: Vec3,
    }
    let mut edges: HashMap<(PosKey, PosKey), Edge> = HashMap::new();
    for t in 0..ntri {
        for k in 0..3 {
            let (a, b) = (vert(t, k), vert(t, (k + 1) % 3));
            let (ka, kb) = (key(a.position), key(b.position));
            if ka == kb {
                continue;
            }
            let (ek, na, nb) = if ka < kb { ((ka, kb), Vec3::from(a.normal), Vec3::from(b.normal)) } else { ((kb, ka), Vec3::from(b.normal), Vec3::from(a.normal)) };
            let e = edges.entry(ek).or_insert(Edge { count: 0, hard: false, na, nb });
            e.count += 1;
            if e.count > 1 && (e.na.dot(na) < crease_cos || e.nb.dot(nb) < crease_cos) {
                e.hard = true;
            }
        }
    }
    let straight = |a: &Vertex, b: &Vertex| -> bool {
        let (ka, kb) = (key(a.position), key(b.position));
        if ka == kb {
            return true;
        }
        let ek = if ka < kb { (ka, kb) } else { (kb, ka) };
        edges.get(&ek).is_none_or(|e| e.count != 2 || e.hard)
    };
    let pts: Vec<[f32; 3]> = grid_points(n).collect();
    let mut out = MeshData::default();
    let mut rs = Resample { src: Vec::new(), moved: 0.0 };
    let mut moved = 0.0f32;
    for t in 0..ntri {
        let s: [u32; 3] = [0, 1, 2].map(|k| mesh.indices[3 * t + k]);
        let v: [&Vertex; 3] = [vert(t, 0), vert(t, 1), vert(t, 2)];
        let p: [Vec3; 3] = v.map(|q| Vec3::from(q.position));
        let nm: [Vec3; 3] = v.map(|q| Vec3::from(q.normal).normalize_or_zero());
        let area = (p[1] - p[0]).cross(p[2] - p[0]).length();
        let base = out.vertices.len() as u32;
        let degenerate = area < 1e-10 || nm.iter().any(|q| q.length_squared() < 0.5);
        if degenerate {
            for k in 0..3 {
                out.vertices.push(*v[k]);
                let mut w = [0.0; 3];
                w[k] = 1.0;
                rs.src.push((s, w));
            }
            out.indices.extend_from_slice(&[base, base + 1, base + 2]);
            continue;
        }
        // control points
        let sx = [straight(v[0], v[1]), straight(v[1], v[2]), straight(v[2], v[0])];
        let ctl = |i: usize, j: usize, st: bool| -> Vec3 {
            if st {
                (2.0 * p[i] + p[j]) / 3.0
            } else {
                let w = (p[j] - p[i]).dot(nm[i]);
                (2.0 * p[i] + p[j] - w * nm[i]) / 3.0
            }
        };
        let b210 = ctl(0, 1, sx[0]);
        let b120 = ctl(1, 0, sx[0]);
        let b021 = ctl(1, 2, sx[1]);
        let b012 = ctl(2, 1, sx[1]);
        let b102 = ctl(2, 0, sx[2]);
        let b201 = ctl(0, 2, sx[2]);
        let e = (b210 + b120 + b021 + b012 + b102 + b201) / 6.0;
        let vv = (p[0] + p[1] + p[2]) / 3.0;
        let b111 = e + (e - vv) * 0.5;
        let nctl = |i: usize, j: usize, st: bool| -> Vec3 {
            let d = p[j] - p[i];
            if st {
                (nm[i] + nm[j]).normalize_or_zero()
            } else {
                let vij = 2.0 * d.dot(nm[i] + nm[j]) / d.dot(d).max(1e-12);
                (nm[i] + nm[j] - vij * d).normalize_or_zero()
            }
        };
        let n110 = nctl(0, 1, sx[0]);
        let n011 = nctl(1, 2, sx[1]);
        let n101 = nctl(2, 0, sx[2]);
        for w in &pts {
            let (ww, u, vq) = (w[0], w[1], w[2]);
            let pos = p[0] * ww * ww * ww
                + p[1] * u * u * u
                + p[2] * vq * vq * vq
                + b210 * (3.0 * ww * ww * u)
                + b120 * (3.0 * ww * u * u)
                + b201 * (3.0 * ww * ww * vq)
                + b021 * (3.0 * u * u * vq)
                + b102 * (3.0 * ww * vq * vq)
                + b012 * (3.0 * u * vq * vq)
                + b111 * (6.0 * ww * u * vq);
            let nor = nm[0] * ww * ww + nm[1] * u * u + nm[2] * vq * vq + n110 * (ww * u) + n011 * (u * vq) + n101 * (ww * vq);
            let nor = if nor.length_squared() > 1e-12 { nor.normalize() } else { (nm[0] * ww + nm[1] * u + nm[2] * vq).normalize_or_zero() };
            let flat = p[0] * ww + p[1] * u + p[2] * vq;
            moved = moved.max((pos - flat).length());
            let mut mv = mix_vertex(v, *w);
            mv.position = pos.to_array();
            mv.normal = nor.to_array();
            out.vertices.push(mv);
            rs.src.push((s, *w));
        }
        grid_indices(n, base, &mut out.indices);
    }
    if moved < 2.0e-3 {
        return None;
    }
    rs.moved = moved;
    Some((out, rs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_count(n: usize) -> usize {
        (n + 1) * (n + 2) / 2
    }

    fn vtx(p: [f32; 3], n: [f32; 3], uv: [f32; 2]) -> Vertex {
        Vertex { position: p, normal: n, uv, color: [1.0; 4] }
    }

    #[test]
    fn family_strips_variants() {
        assert_eq!(family("Light2a"), "Light");
        assert_eq!(family("Light1"), "Light");
        assert_eq!(family("Light2 "), "Light");
        assert_eq!(family("Exhaust1"), "Exhaust");
        assert_eq!(family("SideMirrorLeft"), "SideMirror");
        assert_eq!(family("SideMirror"), "SideMirror");
        assert_eq!(family("Steer1"), "Steer");
        assert_eq!(family("SteerLogo"), "SteerLogo");
        assert_eq!(part_kind("SteerLogo", false), None);
        assert_eq!(part_kind("Body", false), None);
        assert_eq!(part_kind("DecalHood", false), None);
        assert_eq!(part_kind("HeadLight", false), Some(Kind::Smooth));
        assert_eq!(part_kind("WheelFront1", true), Some(Kind::Wheel));
    }

    #[test]
    fn grid_covers_the_triangle() {
        for n in 1..6 {
            let mut idx = Vec::new();
            grid_indices(n, 0, &mut idx);
            assert_eq!(idx.len(), 3 * n * n);
            assert_eq!(*idx.iter().max().unwrap() as usize, grid_count(n) - 1);
        }
    }

    /// a 8-gon tread (a cylinder wall, radius 0.3, x in -0.1..0.1) + a flat disc fan on the +x side
    fn drum(segs: usize) -> MeshData {
        let mut m = MeshData::default();
        let r = 0.3f32;
        let at = |i: usize, x: f32| -> ([f32; 3], f32) {
            let a = std::f32::consts::TAU * i as f32 / segs as f32;
            ([x, r * a.cos(), r * a.sin()], a)
        };
        for i in 0..segs {
            let (p0, a0) = at(i, -0.1);
            let (p1, a1) = at(i + 1, -0.1);
            let (p2, _) = at(i, 0.1);
            let (p3, _) = at(i + 1, 0.1);
            let nrm = |a: f32| [0.0, a.cos(), a.sin()];
            let uvx = |i: usize| i as f32 / segs as f32;
            let q = [
                vtx(p0, nrm(a0), [uvx(i), 0.0]),
                vtx(p1, nrm(a1), [uvx(i + 1), 0.0]),
                vtx(p2, nrm(a0), [uvx(i), 1.0]),
                vtx(p3, nrm(a1), [uvx(i + 1), 1.0]),
            ];
            for t in [[0, 1, 2], [2, 1, 3]] {
                for k in t {
                    m.indices.push(m.vertices.len() as u32);
                    m.vertices.push(q[k]);
                }
            }
            // the cap fan
            let c = vtx([0.1, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.5]);
            for v in [c, vtx(p2, [1.0, 0.0, 0.0], [0.5, 0.5]), vtx(p3, [1.0, 0.0, 0.0], [0.5, 0.5])] {
                m.indices.push(m.vertices.len() as u32);
                m.vertices.push(v);
            }
        }
        m
    }

    #[test]
    fn wheel_circle_is_exact_and_closed() {
        let src = drum(8);
        let (out, rs) = refine_wheel(&src, 32, 8).expect("refined");
        assert_eq!(out.vertices.len(), rs.src.len());
        // every new vertex of the tread (|x| < 0.1 inclusive of the wall) lies on the circle of the same radius, every cap vertex inside it
        let mut worst = 0.0f32;
        for v in &out.vertices {
            let r = v.position[1].hypot(v.position[2]);
            assert!(r <= 0.3 + 1e-5, "radius {r}");
            if v.position[0].abs() < 0.0999 {
                worst = worst.max((r - 0.3).abs());
            }
            let n = Vec3::from(v.normal);
            assert!((n.length() - 1.0).abs() < 1e-4);
        }
        assert!(worst < 1e-5, "tread off the circle by {worst}");
        // the corners keep their place: every source position is among the output positions
        for v in &src.vertices {
            assert!(out.vertices.iter().any(|o| (Vec3::from(o.position) - Vec3::from(v.position)).length() < 1e-5));
        }
        // the cylinder's normals stay radial
        for v in out.vertices.iter().filter(|v| v.position[0].abs() < 0.0999) {
            let radial = Vec3::new(0.0, v.position[1], v.position[2]).normalize();
            assert!(radial.dot(Vec3::from(v.normal)) > 0.9999);
        }
        // the angular step of the tread is at most 1/4 of the original's (8 -> 32+ segments)
        let max_a = out.indices.chunks(3).filter(|t| t.iter().any(|&i| out.vertices[i as usize].position[0] < 0.0)).map(|t| {
            let a: Vec<f32> = t.iter().map(|&i| out.vertices[i as usize]).filter(|v| v.position[1].hypot(v.position[2]) > 1e-4).map(|v| v.position[2].atan2(v.position[1])).collect();
            let w = |d: f32| (d + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            a.iter().map(|x| w(x - a[0]).abs()).fold(0.0f32, f32::max)
        }).fold(0.0f32, f32::max);
        assert!(max_a < std::f32::consts::TAU / 32.0 + 1e-3, "{max_a}");
        // no crack: the edges of the refined mesh that are boundary are only the open ends of the drum (the tread's rim at x = -0.1)
    }

    #[test]
    fn already_round_wheel_is_left_alone() {
        assert!(refine_wheel(&drum(64), 32, 8).is_none());
    }

    #[test]
    fn pn_rounds_a_closed_part_without_cracks_and_keeps_open_edges() {
        // an octahedron with sphere normals: closed and smooth, so PN pushes the faces out towards the unit sphere
        let n = |p: Vec3| p.normalize().to_array();
        let ax = [Vec3::X, Vec3::Y, Vec3::Z, -Vec3::X, -Vec3::Y, -Vec3::Z];
        let faces = [[0, 1, 2], [1, 3, 2], [3, 4, 2], [4, 0, 2], [1, 0, 5], [3, 1, 5], [4, 3, 5], [0, 4, 5]];
        let mut m = MeshData::default();
        for f in faces {
            for k in f {
                m.indices.push(m.vertices.len() as u32);
                m.vertices.push(vtx(ax[k].to_array(), n(ax[k]), [k as f32, 0.0]));
            }
        }
        let (o, rs) = refine_pn(&m, 4, 0.99).expect("refined");
        assert_eq!(o.vertices.len(), 8 * grid_count(4));
        assert_eq!(rs.src.len(), o.vertices.len());
        let rad: Vec<f32> = o.vertices.iter().map(|v| Vec3::from(v.position).length()).collect();
        assert!(rad.iter().all(|r| *r > 0.74 && *r < 1.0 + 1e-3));
        // the face centres moved out from the flat 0.577 towards the sphere
        let flat_mid = 1.0 / 3.0f32.sqrt();
        assert!(rad.iter().cloned().fold(f32::MAX, f32::min) > flat_mid + 0.15);
        // no crack: every point of an edge of the octahedron exists in both triangles that hold it (same position)
        for v in o.vertices.iter().filter(|v| v.position.iter().filter(|c| c.abs() < 1e-6).count() == 1) {
            let same = o.vertices.iter().filter(|w| (Vec3::from(w.position) - Vec3::from(v.position)).length() < 1e-5).count();
            assert!(same >= 2, "an edge point with no partner: {:?}", v.position);
        }
        // uv blends barycentrically
        let w = rs.src[1].1;
        let s = rs.src[1].0;
        let want = w[0] * m.vertices[s[0] as usize].uv[0] + w[1] * m.vertices[s[1] as usize].uv[0] + w[2] * m.vertices[s[2] as usize].uv[0];
        assert!((o.vertices[1].uv[0] - want).abs() < 1e-5);
        // a single triangle's edges are open (one triangle holds them): straight, nothing moves
        let mut one = MeshData::default();
        for k in 0..3 {
            one.indices.push(k as u32);
            one.vertices.push(vtx(ax[k].to_array(), n(ax[k]), [0.0, 0.0]));
        }
        assert!(refine_pn(&one, 4, 0.99).is_none());
    }

    #[test]
    fn flat_part_is_skipped() {
        let mut m = MeshData::default();
        for (k, p) in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].iter().enumerate() {
            m.vertices.push(vtx(*p, [0.0, 0.0, 1.0], [0.0, 0.0]));
            m.indices.push(k as u32);
        }
        assert!(refine_pn(&m, 3, 0.99).is_none());
    }

    #[test]
    fn resample_carries_streams() {
        let rs = Resample { src: vec![([0, 1, 2], [0.5, 0.25, 0.25]), ([0, 1, 2], [0.1, 0.2, 0.7])], moved: 0.0 };
        let v = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 8.0, 0.0]];
        let b = rs.blend3(&v);
        assert_eq!(b[0], [1.0, 2.0, 0.0]);
        assert_eq!(rs.nearest(&[10u8, 20, 30]), vec![10, 30]);
    }
}
