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
    /// `Anim` / `Anqt` rotation track when the article has one (`RE/notes/crp-geometry.md` §7).
    pub animation: Option<nfs_formats::crp::AnimTrack>,
}

/// One placement of a library article requested by a `.scn` GEOM element.
#[derive(Debug, Clone, Copy)]
pub struct PropInstance {
    /// 0 = track scene, 1 = extra (mission) scene; `element` = index into that scene's elements.
    pub source: u8,
    pub element: usize,
    /// GEOM scale (v5: three per-cent values at tokens 7..9, v4: one at token 7).
    pub scale: glam::Vec3,
    pub tag: u32,
    pub transform: glam::Mat4,
}

/// The render-layer classes a `.crp`'s `RMth` records instantiate. The engine registers a factory
/// keyed by these codes (0x449B70, a jump table at 0x449F70), and three of the seventeen classes are
/// `Headlights::LayerVertexLight`, `…_Opaque` and `…_Ambient` — whose constructors are the only
/// places that set the game's night flag `word[0x628A28]` (0x449BF8 / 0x449C25 / 0x449C62).
/// So a track is a **night** track exactly when its render methods use one of them.
/// CONFIRMED (`RE/notes/lighting.md` §4).
pub const HEADLIGHT_LAYERS: [u32; 3] = [0x8002, 0x8003, 0x8011];

/// Does this CRP's `RMth` table build a headlight layer?
pub fn uses_headlight_layers(crp: &Crp, data: &[u8]) -> bool {
    crp.level
        .iter()
        .filter(|e| e.is_level(b"RMth"))
        .filter_map(|e| data.get(e.data_offset()..e.data_offset() + e.data_len()))
        .any(|bytes| {
            bytes
                .chunks_exact(4)
                .any(|w| HEADLIGHT_LAYERS.contains(&u32::from_le_bytes([w[0], w[1], w[2], w[3]])))
        })
}

impl TrackGeometry {
    /// Collision triangles from all world articles (placed objects included) and instanced props,
    /// in world space, with the material index as surface id.
    /// `surface_of(material)` = the CRP `mt` flags word (surface type = `& 0xF`, 0 = not drivable —
    /// CONFIRMED, RE/notes/suspension-collision.md §7); the material index is kept in the upper bits.
    pub fn collision_grid(&self, cell_size: f32, surface_of: &dyn Fn(u16) -> u32) -> nfs_physics::collision::TriangleGrid {
        use nfs_physics::collision::{Triangle, TriangleGrid};
        let mut tris = Vec::new();
        let mut push = |art: &ArticleMeshes, m: glam::Mat4| {
            for mm in &art.meshes {
                let v = &mm.mesh.vertices;
                let surface = (surface_of(mm.material) & 0xFFFF) | ((mm.material as u32) << 16);
                for idx in mm.mesh.indices.chunks_exact(3) {
                    let p = |i: u32| m.transform_point3(glam::Vec3::from(v[i as usize].position));
                    // the vertex colours ride along: the original lights a car from the patch it
                    // stands on (0x474700, `RE/notes/lighting.md`)
                    let c = |i: u32| v[i as usize].color.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
                    tris.push(Triangle { a: p(idx[0]), b: p(idx[1]), c: p(idx[2]), surface, colors: [c(idx[0]), c(idx[1]), c(idx[2])] });
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

/// GEOM element scale in per cent (`0 <id> 0 0 -1 1 1 sx sy sz`; v4 files carry one value).
pub fn geom_scale(e: &nfs_formats::scn::SceneElement) -> glam::Vec3 {
    let p = e.params.first().map(|v| v.as_slice()).unwrap_or(&[]);
    let sx = p.get(7).copied().unwrap_or(100.0);
    let sy = p.get(8).copied().unwrap_or(sx);
    let sz = p.get(9).copied().unwrap_or(sx);
    let f = |v: f32| if v > 0.0 { v / 100.0 } else { 1.0 };
    glam::Vec3::new(f(sx), f(sy), f(sz))
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
    /// `_forward.key` / `_backwards.key`: per-slice render draw distance (`race-logic.md` §6.1).
    pub draw_keys_forward: nfs_formats::key::DrawKeys,
    pub draw_keys_backward: nfs_formats::key::DrawKeys,
    /// Additional scenes whose GEOM props are instanced: the skill-test `_st<N>.scn` of the
    /// current mission (`RE/notes/skill-tests.md` §4 — only loaded for `RACE_TYPE 4`).
    pub extra_scenes: Vec<Scene>,
    /// `Track/animdefs.txt` — every scene-object tag's physical and animation definition
    /// (`RE/notes/crp-geometry.md` §7); shared by all tracks.
    pub animdefs: nfs_formats::animdefs::AnimDefs,
    /// `Track/<track>.edg` wall/edge segments (may be absent: the game rebuilds it from the mesh).
    pub edges: Option<EdgeFile>,
    /// `Track/Sky/<Track>.fsh` layers (`st1a` sky tile, `sc1a` clouds, `horz` horizon strips, ...).
    pub sky_textures: Vec<TextureImage>,
    /// Non-comment lines of `Track/Sky/<Track>.txt` (see RE/FORMATS.md; semantics partially known).
    pub sky_params: Vec<String>,
    /// `Track/Sky/<Track>.bin` — the dumped sky configuration; only its colour records are decoded
    /// (`RE/RENDERING.md`).
    pub sky_bin: Option<nfs_formats::sky::SkyBin>,
}

impl TrackAssets {
    /// Whether this is a night track: its render methods build a `Headlights::*` layer, which is
    /// what sets the game's night flag (`uses_headlight_layers`). The shipped night tracks are
    /// `industrial` and `monaco1…5`; the other nine have none.
    pub fn is_night(&self) -> bool {
        uses_headlight_layers(&self.crp, &self.crp_data)
    }

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
        // one `animdefs.txt` for every track, beside the track files
        let animdefs = GamePaths::find_file(&dir, "animdefs.txt")
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| nfs_formats::animdefs::AnimDefs::parse(&b.iter().map(|&c| nfs_formats::locale::cp1252(c)).collect::<String>()))
            .unwrap_or_default();

        let scene = |suffix: &str| -> Result<Option<Scene>, AssetError> {
            match file(suffix) {
                Ok(p) => Ok(Some(Scene::parse(&String::from_utf8_lossy(&read(&p)?)).map_err(fmt_err(&p))?)),
                Err(_) => Ok(None),
            }
        };
        let keys = |suffix: &str| -> nfs_formats::key::DrawKeys {
            file(suffix)
                .ok()
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|d| nfs_formats::key::DrawKeys::parse(&d).ok())
                .unwrap_or_default()
        };
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
        let sky_bin = GamePaths::find_file(&sky_dir, &format!("{name}.bin"))
            .or_else(|| GamePaths::find_file(&sky_dir, "default.bin"))
            .and_then(|p| read(&p).ok())
            .and_then(|d| nfs_formats::sky::SkyBin::parse(&d).ok());
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
            draw_keys_forward: keys("_forward.key"),
            draw_keys_backward: keys("_backwards.key"),
            cameras: scene("_cameras.scn")?,
            extra_scenes: Vec::new(),
            animdefs,
            edges,
            sky_textures,
            sky_params,
            sky_bin,
        })
    }

    /// Load `Track/<track><suffix>` (e.g. `_st1.scn`) as a scene.
    pub fn load_scene(&self, suffix: &str) -> Result<Option<Scene>, AssetError> {
        let Some(p) = GamePaths::find_file(&self.dir, &format!("{}{}", self.name, suffix)) else { return Ok(None) };
        let text = String::from_utf8_lossy(&std::fs::read(&p).map_err(|e| AssetError::Io { path: p.clone(), source: e })?).into_owned();
        Ok(Some(Scene::parse(&text).map_err(fmt_err(&p))?))
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
            let animation = self.crp.article_animation(&self.crp_data, a);
            let item = ArticleMeshes { name: g.name.clone(), meshes, transform, tag, animation };
            if is_lib { library.push(item) } else { world.push(item) }
        }
        let mut instances = Vec::new();
        for scene in self.start_forward.iter() {
            for e in scene.of_kind(&nfs_formats::scn::ElementKind::Geom) {
                // GEOM v5 param line: "0 <id> 0 0 -1 1 1 sx sy sz" — id at index 1 (v4: 8 tokens, same prefix)
                let Some(id) = Scene::param_u32(e, 0, 1) else { continue };
                instances.push(PropInstance { source: 0, element: 0, scale: geom_scale(e), tag: id, transform: scn_transform(e) });
            }
        }
        for scene in &self.extra_scenes {
            for (i, e) in scene.elements.iter().enumerate() {
                if e.kind != nfs_formats::scn::ElementKind::Geom {
                    continue;
                }
                let Some(id) = Scene::param_u32(e, 0, 1) else { continue };
                instances.push(PropInstance { source: 1, element: i, scale: geom_scale(e), tag: id, transform: scn_transform(e) });
            }
        }
        Ok(TrackGeometry { world, library, instances, skipped_primitives: skipped })
    }

    /// Racing line `Track/racer/work/<track><dir>.lsp` (dir 0 forward, 1 backward).
    pub fn racing_line(&self, paths: &GamePaths, backwards: bool) -> Option<nfs_formats::ais::RacingLine> {
        let dir = paths.track_dir().join("racer").join("work");
        let p = GamePaths::find_file(&dir, &format!("{}{}.lsp", self.name, backwards as u8))?;
        nfs_formats::ais::RacingLine::parse(&std::fs::read(p).ok()?).ok()
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
