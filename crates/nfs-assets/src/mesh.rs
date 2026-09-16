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
/// 4 = sequential quads of 4 vertices (signs/billboards, HIGH); 1 = unknown (13 in skidpad,
/// "Econocar" — possibly lines) and is skipped, counted in `skipped`.
pub fn article_meshes(art: &ArticleGeometry, lod: u8, skipped: &mut usize) -> Vec<MaterialMesh> {
    let mut by_mat: BTreeMap<u16, MeshData> = BTreeMap::new();
    for p in art.primitives.iter().filter(|p| p.lod == lod) {
        let (vi, ui): (Vec<u8>, Vec<u8>) = match (p.prim_type, p.vertex_indices.is_empty()) {
            (3, false) => (p.vertex_indices.clone(), p.uv_indices.clone()),
            (3, true) if p.positions.len() % 3 == 0 && p.positions.len() <= 256 => {
                let seq: Vec<u8> = (0..p.positions.len() as u8).collect();
                (seq.clone(), seq)
            }
            (4, _) if p.positions.len() % 4 == 0 && p.positions.len() <= 256 => {
                let mut v = Vec::new();
                for q in 0..p.positions.len() / 4 {
                    let b = (q * 4) as u8;
                    v.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
                }
                (v.clone(), v)
            }
            _ => {
                *skipped += 1;
                continue;
            }
        };
        let mesh = by_mat.entry(p.material).or_default();
        append_primitive(mesh, p, &vi, &ui);
    }
    by_mat.into_iter().map(|(material, mesh)| MaterialMesh { material, mesh }).collect()
}

fn append_primitive(mesh: &mut MeshData, p: &Primitive, vertex_indices: &[u8], uv_indices: &[u8]) {
    let n = vertex_indices.len();
    for tri in 0..n / 3 {
        let corners = [tri * 3, tri * 3 + 2, tri * 3 + 1]; // flip winding to CCW
        let pos = |k: usize| glam::Vec3::from(p.positions[vertex_indices[k] as usize]);
        let face_n = {
            let a = pos(tri * 3);
            let b = pos(tri * 3 + 1);
            let c = pos(tri * 3 + 2);
            // file winding is CW from +Y, so (c-a)×(b-a) points up for upward-facing faces
            let nrm = (c - a).cross(b - a);
            if nrm.length_squared() > 0.0 { nrm.normalize() } else { glam::Vec3::Y }
        };
        for k in corners {
            let vi = vertex_indices[k] as usize;
            let ui = uv_indices.get(k).copied().unwrap_or(0) as usize;
            let normal = p.normals.get(vi).copied().unwrap_or(face_n.to_array());
            let uv = p.uvs.get(ui).copied().unwrap_or([0.0, 0.0]);
            let c = p.colors.get(k).copied().unwrap_or(0xFFFF_FFFF).to_le_bytes();
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
