//! CRP article geometry → engine meshes.
//!
//! Conventions (from `RE/notes/crp-geometry.md`): positions are world metres, Y up; file triangle
//! winding is clockwise seen from +Y (D3D front face), so we emit `v0, v2, v1` for a CCW renderer;
//! `df` colours are one RGBA u32 per triangle corner (byte order LOW confidence, treated as
//! little-endian A,B,G,R → we read bytes as R,G,B,A which matches white 0xFFFFFFFF either way).

use nfs_formats::crp::{ArticleGeometry, Primitive};
use nfs_renderer::{MeshData, Vertex};
use std::collections::BTreeMap;

/// A mesh with the material it uses.
#[derive(Debug, Clone)]
pub struct MaterialMesh {
    pub material: u16,
    pub mesh: MeshData,
}

/// Convert one article's primitives at the given LOD into per-material meshes.
///
/// Primitive types (RE/notes/crp-geometry.md §4): 3 with index arrays = indexed triangle list;
/// 3 without index arrays = sequential triangle list (CONFIRMED count%3==0, e.g. "Docker2");
/// **1 = indexed triangle strip** with the usual degenerate stitches (CONFIRMED against the data:
/// expanded as a strip the 506 shipped type-1 primitives have a mean normal agreement of 0.93
/// between neighbouring faces, against −0.84 as a fan); 4 = sequential quads of 4 vertices
/// (signs/billboards, HIGH). Anything else is skipped and counted in `skipped`.
pub fn article_meshes(art: &ArticleGeometry, lod: u8, skipped: &mut usize) -> Vec<MaterialMesh> {
    let mut by_mat: BTreeMap<u16, MeshData> = BTreeMap::new();
    for p in art.primitives.iter().filter(|p| p.lod == lod) {
        // corners of the expanded triangle list, each as a position in the primitive's own index
        // arrays (which is also what indexes the per-corner colours)
        // a primitive without index arrays runs one corner per vertex, as many as the colour stream
        // (the `df` window) says — that window is what tells the engine how long the primitive is
        let seq = if p.colors.is_empty() { p.positions.len() } else { p.colors.len().min(p.positions.len()) };
        let corners: Vec<usize> = match (p.prim_type, p.vertex_indices.is_empty()) {
            (3, false) => (0..p.vertex_indices.len()).collect(),
            (3, true) if seq % 3 == 0 => (0..seq).collect(),
            (1, false) => strip_corners(p.vertex_indices.len(), |k| p.vertex_indices[k] as usize),
            (1, true) => strip_corners(seq, |k| k),
            (4, true) if seq % 4 == 0 => {
                let mut v = Vec::new();
                for q in 0..seq / 4 {
                    let b = q * 4;
                    v.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
                }
                v
            }
            _ => {
                *skipped += 1;
                continue;
            }
        };
        let mesh = by_mat.entry(p.material).or_default();
        append_primitive(mesh, p, &corners);
    }
    by_mat.into_iter().map(|(material, mesh)| MaterialMesh { material, mesh }).collect()
}

/// Expand a triangle strip into triangle-list corner positions, keeping the file's winding and
/// dropping the degenerate triangles that stitch separate strips together (17.6 % of the shipped
/// type-1 triangles are such stitches).
fn strip_corners(n: usize, index_at: impl Fn(usize) -> usize) -> Vec<usize> {
    let mut out = Vec::new();
    for k in 0..n.saturating_sub(2) {
        let t = if k % 2 == 0 { [k, k + 1, k + 2] } else { [k + 1, k, k + 2] };
        let (a, b, c) = (index_at(t[0]), index_at(t[1]), index_at(t[2]));
        if a == b || b == c || a == c {
            continue;
        }
        out.extend_from_slice(&t);
    }
    out
}

fn append_primitive(mesh: &mut MeshData, p: &Primitive, corner_at: &[usize]) {
    let n = corner_at.len();
    let index_at = |k: usize| -> usize { if p.vertex_indices.is_empty() { corner_at[k] } else { p.vertex_indices[corner_at[k]] as usize } };
    let uv_at = |k: usize| -> usize { if p.uv_indices.is_empty() { corner_at[k] } else { p.uv_indices[corner_at[k]] as usize } };
    for tri in 0..n / 3 {
        let corners = [tri * 3, tri * 3 + 2, tri * 3 + 1]; // flip winding to CCW
        let pos = |k: usize| glam::Vec3::from(p.positions[index_at(k)]);
        let face_n = {
            let a = pos(tri * 3);
            let b = pos(tri * 3 + 1);
            let c = pos(tri * 3 + 2);
            // file winding is CW from +Y, so (c-a)×(b-a) points up for upward-facing faces
            let nrm = (c - a).cross(b - a);
            if nrm.length_squared() > 0.0 { nrm.normalize() } else { glam::Vec3::Y }
        };
        for k in corners {
            let vi = index_at(k);
            let ui = uv_at(k);
            let normal = p.normals.get(vi).copied().unwrap_or(face_n.to_array());
            let uv = p.uvs.get(ui).copied().unwrap_or([0.0, 0.0]);
            let c = p.colors.get(corner_at[k]).copied().unwrap_or(0xFFFF_FFFF).to_le_bytes();
            let base = mesh.vertices.len() as u32;
            mesh.vertices.push(Vertex {
                position: p.positions[vi],
                normal,
                uv,
                color: [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0],
            });
            mesh.indices.push(base);
        }
    }
}
