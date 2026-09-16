//! Track asset bundle: every original file that makes up one track, parsed but not yet
//! converted to meshes (mesh conversion lands once the CRP geometry layout is established —
//! see `RE/notes/crp-geometry.md`).

use crate::{fmt_err, read, texture::{decode_pack, TextureImage}, AssetError, GamePaths};
use crate::mesh::{article_meshes, MaterialMesh};
use nfs_formats::{crp::{Crp, Material}, edg::EdgeFile, ini::Ini, refpack, scn::Scene};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ArticleMeshes {
    pub name: String,
    pub meshes: Vec<MaterialMesh>,
    /// World transform (from the `tr` part, row-major; identity when absent).
    pub transform: glam::Mat4,
    /// Scene-object tag (`Base+68`), zero when none.
    pub tag: u32,
}

/// One placement of a library article requested by a `.scn` GEOM element.
#[derive(Debug, Clone, Copy)]
pub struct PropInstance {
    pub tag: u32,
    pub transform: glam::Mat4,
}

impl TrackGeometry {
    /// Collision triangles from all world articles (placed objects included) and instanced props,
    /// in world space, with the material index as surface id.
    pub fn collision_grid(&self, cell_size: f32) -> nfs_physics::collision::TriangleGrid {
        use nfs_physics::collision::{Triangle, TriangleGrid};
        let mut tris = Vec::new();
        let mut push = |art: &ArticleMeshes, m: glam::Mat4| {
            for mm in &art.meshes {
                let v = &mm.mesh.vertices;
                for idx in mm.mesh.indices.chunks_exact(3) {
                    let p = |i: u32| m.transform_point3(glam::Vec3::from(v[i as usize].position));
                    tris.push(Triangle { a: p(idx[0]), b: p(idx[1]), c: p(idx[2]), surface: mm.material as u32 });
                }
            }
        };
        for art in &self.world {
            push(art, art.transform);
        }
        for inst in &self.instances {
            if let Some(art) = self.library.iter().find(|a| a.tag == inst.tag) {
                push(art, inst.transform * art.transform);
            }
        }
        TriangleGrid::build(tris, cell_size)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrackGeometry {
    /// Static world articles (geometry already in world space) and placed objects (`tr` applied).
    pub world: Vec<ArticleMeshes>,
    /// Library articles (Base flag 0x8000), local space; instanced via `instances`.
    pub library: Vec<ArticleMeshes>,
    /// GEOM elements from the start-grid and `_st*.scn` files.
    pub instances: Vec<PropInstance>,
    pub skipped_primitives: usize,
}

/// `tr` part: 4×4 row-major with translation in row 3 (D3D convention) → column-major glam matrix.
fn tr_to_mat4(m: &[f32; 16]) -> glam::Mat4 {
    glam::Mat4::from_cols_array(m)
}

/// `.scn` element position + 3×3 row-major orientation → world matrix.
pub fn scn_transform(e: &nfs_formats::scn::SceneElement) -> glam::Mat4 {
    let r = e.orientation.unwrap_or([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    // rows are the local axes expressed in world space (HYPOTHESIS consistent with the tr layout)
    glam::Mat4::from_cols(
        glam::Vec4::new(r[0][0], r[0][1], r[0][2], 0.0),
        glam::Vec4::new(r[1][0], r[1][1], r[1][2], 0.0),
        glam::Vec4::new(r[2][0], r[2][1], r[2][2], 0.0),
        glam::Vec4::new(e.position[0], e.position[1], e.position[2], 1.0),
    )
}

#[derive(Debug)]
pub struct TrackAssets {
    pub name: String,
    pub dir: PathBuf,
    /// Decompressed `.crp` bytes (parts are resolved lazily against this buffer).
    pub crp_data: Vec<u8>,
    pub crp: Crp,
    pub textures: Vec<TextureImage>,
    pub env_textures: Vec<TextureImage>,
    pub map: Ini,
    pub start_forward: Option<Scene>,
    pub start_backward: Option<Scene>,
    pub cameras: Option<Scene>,
    /// `<track>_st<N>.scn` stage/scenario scenes (which one a race uses is not yet known).
    pub stages: Vec<Scene>,
    /// `Track/<track>.edg` wall/edge segments (may be absent: the game rebuilds it from the mesh).
    pub edges: Option<EdgeFile>,
    /// `Track/Sky/<Track>.fsh` layers (`st1a` sky tile, `sc1a` clouds, `horz` horizon strips, ...).
    pub sky_textures: Vec<TextureImage>,
    /// Non-comment lines of `Track/Sky/<Track>.txt` (see RE/FORMATS.md; semantics partially known).
    pub sky_params: Vec<String>,
}

impl TrackAssets {
    pub fn load(paths: &GamePaths, name: &str) -> Result<Self, AssetError> {
        let dir = paths.track_dir();
        let file = |suffix: &str| -> Result<PathBuf, AssetError> {
            let n = format!("{name}{suffix}");
            GamePaths::find_file(&dir, &n).ok_or_else(|| AssetError::MissingFile { dir: dir.clone(), name: n })
        };
        let crp_path = file(".crp")?;
        let crp_data = refpack::maybe_decompress(&read(&crp_path)?).map_err(fmt_err(&crp_path))?.into_owned();
        let crp = Crp::parse(&crp_data).map_err(fmt_err(&crp_path))?;

        let fsh_path = file(".fsh")?;
        let fsh_data = refpack::maybe_decompress(&read(&fsh_path)?).map_err(fmt_err(&fsh_path))?.into_owned();
        let textures = decode_pack(&fsh_path, &fsh_data)?;

        let env_path = file(".env")?;
        let env_data = refpack::maybe_decompress(&read(&env_path)?).map_err(fmt_err(&env_path))?.into_owned();
        let env_textures = decode_pack(&env_path, &env_data)?;

        let map_path = file(".map")?;
        let map = Ini::parse(&String::from_utf8_lossy(&read(&map_path)?));

        let scene = |suffix: &str| -> Result<Option<Scene>, AssetError> {
            match file(suffix) {
                Ok(p) => Ok(Some(Scene::parse(&String::from_utf8_lossy(&read(&p)?)).map_err(fmt_err(&p))?)),
                Err(_) => Ok(None),
            }
        };
        let mut stages = Vec::new();
        let prefix = format!("{}_st", name.to_lowercase());
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let fname = p.file_name().and_then(|n| n.to_str()).map(|n| n.to_lowercase()).unwrap_or_default();
            if fname.starts_with(&prefix) && fname.ends_with(".scn") {
                stages.push(Scene::parse(&String::from_utf8_lossy(&read(&p)?)).map_err(fmt_err(&p))?);
            }
        }
        let edges = match file(".edg") {
            Ok(p) => Some(EdgeFile::parse(&read(&p)?).map_err(fmt_err(&p))?),
            Err(_) => None,
        };
        let sky_dir = dir.join("Sky");
        let (sky_textures, sky_params) = match (GamePaths::find_file(&sky_dir, &format!("{name}.fsh")), GamePaths::find_file(&sky_dir, &format!("{name}.txt"))) {
            (Some(fsh), txt) => {
                let d = refpack::maybe_decompress(&read(&fsh)?).map_err(fmt_err(&fsh))?.into_owned();
                let params = match txt {
                    Some(t) => String::from_utf8_lossy(&read(&t)?).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect(),
                    None => Vec::new(),
                };
                (decode_pack(&fsh, &d)?, params)
            }
            _ => (Vec::new(), Vec::new()),
        };
        Ok(Self {
            name: name.to_string(),
            dir: dir.clone(),
            crp_data,
            crp,
            textures,
            env_textures,
            map,
            start_forward: scene("_fstart.scn")?,
            start_backward: scene("_bstart.scn")?,
            cameras: scene("_cameras.scn")?,
            stages,
            edges,
            sky_textures,
            sky_params,
        })
    }

    /// Materials of the track CRP in `pr.material` order.
    pub fn materials(&self) -> Result<Vec<Material>, AssetError> {
        self.crp.materials(&self.crp_data).map_err(fmt_err(&self.dir.join(format!("{}.crp", self.name))))
    }

    /// Static track geometry at the lowest LOD of every article, grouped per article and material.
    /// Articles carrying a `tr` matrix are library objects instanced by `.scn` files (mapping not yet
    /// known) and are returned separately in `TrackGeometry::library`.
    pub fn build_geometry(&self) -> Result<TrackGeometry, AssetError> {
        let crp_path = self.dir.join(format!("{}.crp", self.name));
        let arts = self.crp.articles(&self.crp_data).map_err(fmt_err(&crp_path))?;
        let mut world = Vec::new();
        let mut library = Vec::new();
        let mut skipped = 0usize;
        for a in &arts {
            let g = self.crp.article_geometry(&self.crp_data, a).map_err(fmt_err(&crp_path))?;
            let Some(lod) = g.primitives.iter().map(|p| p.lod).min() else { continue };
            let meshes = article_meshes(&g, lod, &mut skipped);
            let transform = g.transform.as_ref().map(tr_to_mat4).unwrap_or(glam::Mat4::IDENTITY);
            let (is_lib, tag) = g.base.as_ref().map(|b| (b.is_library(), b.tag_u32())).unwrap_or((false, 0));
            let item = ArticleMeshes { name: g.name.clone(), meshes, transform, tag };
            if is_lib { library.push(item) } else { world.push(item) }
        }
        let mut instances = Vec::new();
        for scene in self.start_forward.iter().chain(self.stages.iter()) {
            for e in scene.of_kind(&nfs_formats::scn::ElementKind::Geom) {
                // GEOM v5 param line: "0 <id> 0 0 -1 1 1 sx sy sz" — id at index 1 (v4: 8 tokens, same prefix)
                let Some(id) = Scene::param_u32(e, 0, 1) else { continue };
                instances.push(PropInstance { tag: id, transform: scn_transform(e) });
            }
        }
        Ok(TrackGeometry { world, library, instances, skipped_primitives: skipped })
    }

    /// Names of all tracks present in the installation (files `<name>.crp` in `GameData/Track`).
    pub fn list(paths: &GamePaths) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(paths.track_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension()?.eq_ignore_ascii_case("crp")).then(|| p.file_stem()?.to_str().map(|s| s.to_lowercase()))?
            })
            .collect();
        v.sort();
        v
    }
}
