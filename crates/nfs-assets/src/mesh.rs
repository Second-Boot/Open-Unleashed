//! CRP article geometry → engine meshes.
//!
//! Conventions (from `RE/notes/crp-geometry.md`): positions are world metres, Y up; file triangle
//! winding is clockwise seen from +Y (D3D front face), so we emit `v0, v2, v1` for a CCW renderer;
//! `df` colours are one RGBA u32 per triangle corner (byte order LOW confidence, treated as
//! little-endian A,B,G,R → we read bytes as R,G,B,A which matches white 0xFFFFFFFF either way).

use nfs_formats::crp::{prim_type, ArticleGeometry, Primitive};
use nfs_physics::collision::PatchKind;
use nfs_renderer::{MeshData, Vertex};
use std::collections::BTreeMap;

/// A mesh with the material it uses.
#[derive(Debug, Clone)]
pub struct MaterialMesh {
    pub material: u16,
    pub mesh: MeshData,
    /// The `pr` record's flag word (`+2`) — of the group's first part in [`article_meshes`]. The
    /// static drawer 0x450DE0 files a part into the view's lists by it (0x450F30…0x450F9B,
    /// `RE/RENDERING.md` "The view's lists").
    pub flags: u16,
    /// The `pr` record's word `+6` — the high half of its key in the ascending list `+4`
    /// (`([pr+4] & 0xFFFF0000) + round(d²)`, 0x450F37…0x450F44) — of the group's first part in
    /// [`article_meshes`].
    pub key_hi: u16,
    /// The `pr` record's sphere centre (`+8`), in the article's frame — the point whose square
    /// distance from the eye keys the part in those lists (0x450EF8…0x450F25).
    pub centre: [f32; 3],
    /// Which patch ctor the original builds this primitive's shapes with (`0x4853C0` quad /
    /// `0x485C50` triangle — [`PatchKind`]) — only [`article_parts`] (one `MaterialMesh` per `pr`
    /// primitive) sets this meaningfully, from `p.prim_type` (`4` = quad list, else triangle);
    /// [`article_meshes`] merges several primitives, possibly of different kinds, into one mesh
    /// per material, so its `patch_kind`/`prim_index` are not meaningful and are not read by
    /// anything (see the quad identity tests in `track.rs`).
    pub patch_kind: PatchKind,
    /// This primitive's own position in [`ArticleGeometry::primitives`] — set by [`article_parts`]
    /// only (see [`Self::patch_kind`]); `usize::MAX` from [`article_meshes`].
    pub prim_index: usize,
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
    let mut by_mat: BTreeMap<u16, MaterialMesh> = BTreeMap::new();
    for p in art.primitives.iter().filter(|p| p.lod == lod) {
        let Some(corners) = primitive_corners(p, skipped) else { continue };
        let mm = by_mat.entry(p.material).or_insert_with(|| MaterialMesh {
            material: p.material,
            mesh: MeshData::default(),
            flags: p.flags,
            key_hi: p.key_hi,
            centre: [p.sphere[0], p.sphere[1], p.sphere[2]],
            patch_kind: PatchKind::Triangle,
            prim_index: usize::MAX,
        });
        append_primitive(&mut mm.mesh, p, &corners);
    }
    by_mat.into_values().collect()
}

/// One article's primitives at the given LOD as the draw takes them: one mesh per `pr` record, in
/// the file's order — 0x450DE0 queues each part of the LOD's range on its own, keyed by its own
/// sphere (0x450EDB…0x450FCF). The primitive types are [`article_meshes`]'s.
pub fn article_parts(art: &ArticleGeometry, lod: u8, skipped: &mut usize) -> Vec<MaterialMesh> {
    let mut out = Vec::new();
    for (i, p) in art.primitives.iter().enumerate().filter(|(_, p)| p.lod == lod) {
        let Some(corners) = primitive_corners(p, skipped) else { continue };
        let mut mesh = MeshData::default();
        append_primitive(&mut mesh, p, &corners);
        // 0x4750B0 builds one patch per quad or triangle *shape*, not per raw `pr` record — a
        // `prim_type == 4` record can hold several quads ("sequential quads", `crp-geometry.md`
        // §4); `TrackGeometry::collision_grid` pairs this mesh's triangles back into quads two at
        // a time (`nfs_physics::collision::TriangleGrid::patches`), so one `patch_kind` per
        // primitive is enough — every quad in a quad-list record shares it.
        let patch_kind = if p.prim_type == prim_type::QUADS { PatchKind::Quad } else { PatchKind::Triangle };
        out.push(MaterialMesh { material: p.material, mesh, flags: p.flags, key_hi: p.key_hi, centre: [p.sphere[0], p.sphere[1], p.sphere[2]], patch_kind, prim_index: i });
    }
    out
}

/// The corners of a primitive's expanded triangle list (see [`article_meshes`]); `None`, counted
/// in `skipped`, for a shape the builder does not take.
fn primitive_corners(p: &Primitive, skipped: &mut usize) -> Option<Vec<usize>> {
    {
        // corners of the expanded triangle list, each as a position in the primitive's own index
        // arrays (which is also what indexes the per-corner colours)
        // a primitive without index arrays runs one corner per vertex, as many as the colour stream
        // (the `df` window) says — that window is what tells the engine how long the primitive is
        let seq = if p.colors.is_empty() { p.positions.len() } else { p.colors.len().min(p.positions.len()) };
        let corners: Vec<usize> = match (p.prim_type, p.vertex_indices.is_empty()) {
            (prim_type::LIST, false) => (0..p.vertex_indices.len()).collect(),
            (prim_type::LIST, true) if seq % 3 == 0 => (0..seq).collect(),
            (prim_type::STRIP, false) => strip_corners(p.vertex_indices.len(), |k| p.vertex_indices[k] as usize),
            (prim_type::STRIP, true) => strip_corners(seq, |k| k),
            (prim_type::QUADS, true) if seq % 4 == 0 => {
                let mut v = Vec::new();
                for q in 0..seq / 4 {
                    let b = q * 4;
                    v.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
                }
                v
            }
            _ => {
                *skipped += 1;
                return None;
            }
        };
        Some(corners)
    }
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
            // the `df` word is a D3D `0xAARRGGBB` (file bytes B, G, R, A): channel 0 is the third byte. Read the
            // other way round (RGBA, until 2026-10-02) the baked road light came out warm where the original's
            // is cool: the alps road and the 993's cabin against the server's frames (`REFERENCE-2026-10-02-SRVF.md`,
            // COCKPIT2), the cabin 62/53/56 against 62/53/53. The car light's `g` (0x4746A0) takes the same colours.
            let c = [c[2], c[1], c[0], c[3]];
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
