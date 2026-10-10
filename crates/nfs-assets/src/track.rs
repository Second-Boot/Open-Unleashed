//! Track asset bundle: every original file that makes up one track, parsed but not yet
//! converted to meshes (mesh conversion lands once the CRP geometry layout is established —
//! see `RE/notes/crp-geometry.md`).

use crate::{fmt_err, read, read_pack, texture::{decode_pack, decode_pack_mipped, TextureImage}, AssetError, GamePaths};
use crate::mesh::{article_parts, MaterialMesh};
use nfs_formats::{crp::{layer_class, tags, Crp, Material}, edg::EdgeFile, ini::Ini, scn::Scene};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ArticleMeshes {
    pub name: String,
    /// The lowest LOD's meshes, one per `pr` part in the file's order ([`crate::mesh::article_parts`]).
    pub meshes: Vec<MaterialMesh>,
    /// Every LOD the article carries, ascending by its number (`pr.lod`, `RE/notes/crp-geometry.md`
    /// §2.1), each with its per-part meshes — the drawer's LOD pick (`Base+100`'s records,
    /// 0x450DE0) chooses one of these per frame. The first is [`Self::meshes`]. *(Until 2026-09-26
    /// merged per material.)*
    pub lods: Vec<(u8, Vec<MaterialMesh>)>,
    /// World transform (from the `tr` part, row-major; identity when absent).
    pub transform: glam::Mat4,
    /// Scene-object tag (`Base+68`), zero when none.
    pub tag: u32,
    /// `Anim` / `Anqt` rotation track when the article has one (`RE/notes/crp-geometry.md` §7).
    pub animation: Option<nfs_formats::crp::AnimTrack>,
    /// The article's position in the `.crp`'s article list.
    pub index: usize,
    /// Whether the article's bundle owns a `PCollideArticleBundle` (`[bundle+0xC]`): `0x488090`
    /// (`insertAll(C)`, at 0x488364…0x48839F) `dynamic_cast`s that pointer per bundle and calls
    /// `0x4750B0` only when it is non-null, and the three constructor call sites of the
    /// `PCollideArticleBundle` (0x474B90) are the ends of `WInstanceBundle::Load` (0x47860F),
    /// `WStaticBundle::Load` (0x483796) and `WSceneBundle`'s spawn (0x4830A2). `WAnimBundle`
    /// (`Anqt` articles) and `WMidGroundBundle` (`Base & 0x6000`) keep the `0` `0x489160` writes
    /// (0x48916D), so they contribute **no** collision patch and no `kCylinder`/`kBox` object
    /// (`RE/notes/COVERAGE-2026-09-26.md` §14.80).
    pub collides: bool,
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
pub const HEADLIGHT_LAYERS: [u32; 3] = [layer_class::VERTEX_LIGHT, layer_class::VERTEX_LIGHT_OPAQUE, layer_class::VERTEX_LIGHT_AMBIENT];

/// The raw bytes of every `RMth` record of the file whose data lies inside `data`.
fn rmth_records<'a>(crp: &'a Crp, data: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
    crp.level.iter().filter(|e| e.is_level(tags::RMTH)).filter_map(move |e| data.get(e.data_offset()..e.data_offset() + e.data_len()))
}

/// The render methods whose `RMth` record holds (past its 16-byte name) a dword `class` accepts,
/// by the method's name.
fn methods_with_class(crp: &Crp, data: &[u8], class: impl Fn(u32) -> bool) -> std::collections::HashSet<String> {
    rmth_records(crp, data)
        .filter(|bytes| bytes.len() > 16 && bytes[16..].chunks_exact(4).any(|w| class(u32::from_le_bytes([w[0], w[1], w[2], w[3]]))))
        .map(|bytes| bytes[..16].iter().take_while(|b| **b != 0).map(|b| *b as char).collect())
        .collect()
}

/// The render methods that build `Headlights::LayerVertexLight` (0x8002) or `…_Opaque` (0x8003) —
/// both slot 0 **0x453110**, which adds the headlight projection to each vertex's colour
/// (`RE/notes/lighting.md` §5). By the method's name, the key an `mt` names it by.
pub fn vertex_light_methods(crp: &Crp, data: &[u8]) -> std::collections::HashSet<String> {
    methods_with_class(crp, data, |w| matches!(w, layer_class::VERTEX_LIGHT | layer_class::VERTEX_LIGHT_OPAQUE))
}

/// The flat vertex colour of every material whose method builds, for its `op_mask`, an `Am` op —
/// `RCustomLayers::LayerAmbient` **0x8007** by day or `Headlights::LayerVertexLight_Ambient`
/// **0x8011** by night — keyed by the material's index. Both write the first dword of the op's
/// slot over every vertex's colour (0x448C83…0x448C97; 0x45360B, then 0x453BDF with the lamps off
/// or 0x453763…0x4537D6 with the headlight added and clamped), and the slot is the method's own
/// `Am` data block (0x44C313…0x44C366 → 0x44A580). The same op mask drops the `df` colours
/// (`LayerColours`, mask 8), so the class-0 draw's copy of the stream (0x540E1D) never runs:
/// such a material is drawn **in this colour, whatever its `df` says**. Every shipped track's
/// block is 0xFFFFFFFF, and the op is built for the materials with bit 0x10 (0x30010, 0x13;
/// `RE/notes/lighting.md` §5.4).
pub fn ambient_colours(crp: &Crp, data: &[u8]) -> std::collections::HashMap<u16, u32> {
    let (Ok(methods), Ok(materials)) = (crp.render_methods(data), crp.materials(data)) else { return Default::default() };
    // the first method of each name, as `methods.iter().find(..)` would pick it
    let mut by_name = std::collections::HashMap::with_capacity(methods.len());
    for (k, r) in methods.iter().enumerate() {
        by_name.entry(r.name.as_str()).or_insert(k);
    }
    materials
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let method = &methods[*by_name.get(m.render_method.as_str())?];
            // pass masks are only ever set on later passes; the `Am` op sits in pass 1 (mask 0)
            let op = method.built_ops(0, m.op_mask).find(|o| matches!(o.class, layer_class::AMBIENT | layer_class::VERTEX_LIGHT_AMBIENT))?;
            let b = method.block(op.key)?;
            Some((i as u16, u32::from_le_bytes(b.get(..4)?.try_into().ok()?)))
        })
        .collect()
}

/// `RCustomLayers::LayerUVAnimate`'s own parameters for a material whose render method builds it
/// — class **0x8001** (`RE/drafts/round1/render-core.md` §4.4, CONFIRMED). Mode 0 scrolls the V
/// texture coordinate: `t = TIMER_getTicks() · rate · 1e-4`, `v += t − floor(t)` — a steady wrap,
/// water. Mode 2 wobbles U sinusoidally: `th = (TIMER_getTicks() + phase) · rate · 2e-4` turns,
/// `u += sin(th) · 0.06` — flags/banners, `phase` a per-article offset (the source UV pointer's
/// low 12 bits, 0x448E12) so identical articles do not wobble in step; the port cannot recover
/// that raw address and uses `phase = 0` for every article (APPROXIMATION — every flag using one
/// material wobbles in lockstep, where the original staggers them by whatever their allocator
/// happened to place their UV data at). **Observed on the original (2026-10-01 SV, Wine, canyon — the
/// only shipped track with a mode-2 material, `S_UVAnimate`; gdb at 0x448DF7, `esi & 0xFFF`):** one
/// layer object served every flag and the phase differed per article — 20 distinct values in 47 calls
/// (0x280 … 0xFC0, all multiples of 16: 0xFB0, 0x610, 0xCE0, 0x960, 0x600, 0x480, …), i.e. the stagger is
/// real and large (up to 4095 ticks = 32 s of wobble), but the values are heap addresses the port
/// cannot reproduce. Kept at 0 rather than invent a spread. The layer's own data block `[this+4]` is
/// `{u32 mode, i32 rate, …}` (0x448CE9); the port reads only those two dwords.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UvAnimate {
    pub mode: u32,
    pub rate: i32,
}

/// [`UvAnimate`]'s parameters for one material, from its render method's block (or the material's
/// own override block, the same `[mat+0x10]`-first rule [`render_states`] documents).
pub fn uv_animate(methods: &[nfs_formats::crp::RenderMethod], m: &Material) -> Option<UvAnimate> {
    let method = methods.iter().find(|r| r.name == m.render_method)?;
    let op = method.built_ops(0, m.op_mask).find(|o| o.class == layer_class::UV_ANIMATE)?;
    let own = m.blocks.iter().find(|(k, _)| method.resolve(*k) == Some(op.key)).map(|(_, b)| b.as_slice());
    let b = own.or_else(|| method.block(op.key))?;
    if b.len() < 8 {
        return None;
    }
    Some(UvAnimate { mode: u32::from_le_bytes(b[0..4].try_into().ok()?), rate: i32::from_le_bytes(b[4..8].try_into().ok()?) })
}

/// The `Rs` block a material's first pass applies — Thrash states **6, 0x0A, 0x38, 2, 0x18** and a
/// closing −1 (`LayerState` 0x53F780 → 0x540660, `RE/notes/thrash.md` §6) — when its render
/// method carries one: the first `LayerState` (class 8) op the material builds, and the block its
/// slot holds at draw time. 0x44A250 lays the material object's own list (`[mat+0x10]`) over the
/// method's slots first, so the material's `mt` block under that key wins
/// ([`Material::blocks`], built at 0x44AD3B…0x44ADFF); otherwise it is the method's data block
/// (0x44C313…0x44C366 → 0x44A580). The shipped tracks' blocks: `Normal`, `FakeSort`, `FullAmbient`,
/// `UVAnimate`, `FA_UVAnim` `{1, 2, 0, 1, 0}` (blending on, `SRCALPHA` / `INVSRCALPHA`, the view's
/// cull), `Opaque` `{1, 0, 0, 1, 0}` (off), `S_Normal` / `S_UVAnimate` `{1, 2, 0, 1, −3}`, and every
/// `…MatState` material's own `{1, 2, 0, 0, 0}` — blending on, **no culling** — as are the own blocks
/// of 108 `FakeSort` materials and one `Opaque` (monaco3), which override their methods'.
pub fn render_states(methods: &[nfs_formats::crp::RenderMethod], m: &Material) -> Option<[i32; 6]> {
    let method = methods.iter().find(|r| r.name == m.render_method)?;
    let op = method.built_ops(0, m.op_mask).find(|o| o.class == layer_class::STATE)?;
    let own = m.blocks.iter().find(|(k, _)| method.resolve(*k) == Some(op.key)).map(|(_, b)| b.as_slice());
    let b = own.or_else(|| method.block(op.key))?;
    (b.len() >= 24).then(|| std::array::from_fn(|k| i32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])))
}

/// The render methods that build `Headlights::LayerUVLight` — the serialised class code **0x8004**
/// (its allocator 0x449C6D takes the jump table's index 3 to the constructor 0x453C60, which sets
/// the vtable 0x5B3DAC). Two of them sit in every night track's `Opaque` method and none in a day
/// track's, so the decal is live in shipped content (`RE/notes/lighting.md` §5.2).
pub fn uv_light_methods(crp: &Crp, data: &[u8]) -> std::collections::HashSet<String> {
    methods_with_class(crp, data, |w| w == layer_class::UV_LIGHT)
}

/// `GameData/Render/night.psh`'s one entry, `nght` — 128 × 128 bytes the headlight projector
/// 0x452910 indexes directly, read at `entry + 0x10` (0x45261B).
pub fn night_map(paths: &crate::GamePaths) -> Option<Vec<u8>> {
    let d = std::fs::read(crate::GamePaths::find_file(&paths.render_dir(), "night.psh")?).ok()?;
    if d.get(0..4)? != b"SHPP" {
        return None;
    }
    let count = u32::from_le_bytes(d.get(8..12)?.try_into().ok()?) as usize;
    (0..count).find_map(|i| {
        let e = 16 + 8 * i;
        (d.get(e..e + 4)? == b"nght").then_some(())?;
        let off = u32::from_le_bytes(d.get(e + 4..e + 8)?.try_into().ok()?) as usize;
        d.get(off + 0x10..off + 0x10 + 128 * 128).map(<[u8]>::to_vec)
    })
}

/// Does this CRP's `RMth` table build a headlight layer?
pub fn uses_headlight_layers(crp: &Crp, data: &[u8]) -> bool {
    rmth_records(crp, data).any(|bytes| bytes.chunks_exact(4).any(|w| HEADLIGHT_LAYERS.contains(&u32::from_le_bytes([w[0], w[1], w[2], w[3]]))))
}

impl TrackGeometry {
    /// Collision triangles from all world articles (placed objects included) and instanced props,
    /// in world space, with the material index as surface id.
    /// `surface_of(material)` = the CRP `mt` flags word (surface type = `& 0xF`, 0 = not drivable —
    /// CONFIRMED, RE/notes/suspension-collision.md §7); the material index is kept in the upper bits.
    pub fn collision_grid(&self, cell_size: f32, surface_of: &dyn Fn(u16) -> u32) -> nfs_physics::collision::TriangleGrid {
        use nfs_physics::collision::{PatchSource, Triangle, TriangleGrid};
        let mut tris = Vec::new();
        let mut push = |art: &ArticleMeshes, m: glam::Mat4| {
            for mm in &art.meshes {
                let v = &mm.mesh.vertices;
                let surface = (surface_of(mm.material) & 0xFFFF) | ((mm.material as u32) << 16);
                // Primitive identity (quadtree prerequisite 1, `RE/RENDERING.md` "What the tree
                // holds"): tag every triangle with the `pr` primitive and patch ctor it came from
                // so `TriangleGrid::patches` can recount a quad's two triangles as the one entry
                // the original's `0x4853C0` builds — this changes no triangle geometry/order.
                let source = Some(PatchSource { article: art.index, primitive: mm.prim_index, kind: mm.patch_kind });
                for idx in mm.mesh.indices.chunks_exact(3) {
                    let p = |i: u32| m.transform_point3(glam::Vec3::from(v[i as usize].position));
                    // the vertex colours ride along: the original lights a car from the patch it
                    // stands on (0x474700, `RE/notes/lighting.md`)
                    let c = |i: u32| v[i as usize].color.map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8);
                    tris.push(Triangle { a: p(idx[0]), b: p(idx[1]), c: p(idx[2]), surface, colors: [c(idx[0]), c(idx[1]), c(idx[2])], source });
                }
            }
        };
        for art in self.world.iter().filter(|a| a.collides) {
            push(art, art.transform);
        }
        for inst in &self.instances {
            if let Some(art) = self.library.iter().find(|a| a.tag == inst.tag) {
                // the scene record's matrix — the element's placement and per-cent scale, no `tr`
                // (0x482EA0 / 0x46B020, 0x482BF1; `RE/RENDERING.md` "The instance and scene items").
                // *(Until 2026-09-26 this took the article's `tr` and not the scale.)* Whether the
                // scene items join the ground's triangles at all is not read (port behaviour).
                push(art, inst.transform * glam::Mat4::from_scale(inst.scale));
            }
        }
        TriangleGrid::build(tris, cell_size)
    }
}

#[derive(Debug, Clone, Default)]
pub struct TrackGeometry {
    /// Static world articles (geometry already in world space) and placed objects (`tr` applied).
    pub world: Vec<ArticleMeshes>,
    /// Library articles (Base flag 0x8000), local space; instanced via `instances`. Their own
    /// [`ArticleMeshes::transform`] (`tr`) is **not** part of an instance's placement: the scene
    /// bundle's record holds the element's matrix alone (0x482BF1 / 0x482EA0, the draw 0x450380).
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

/// The thread's result; a worker that panicked panics here too, as the sequential load would have.
fn join_scoped<T>(h: std::thread::ScopedJoinHandle<'_, T>) -> T {
    h.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// `GamePaths::find_file` for one directory looked up many times: the exact spelling is tried first
/// (a stat), and on a miss the directory is listed **once**, not once per lookup, and the names kept
/// by lower-cased ASCII. The first entry in listing order wins, as in `find_file`.
struct DirLookup {
    dir: PathBuf,
    listing: std::sync::OnceLock<std::collections::HashMap<String, PathBuf>>,
}

impl DirLookup {
    fn new(dir: &std::path::Path) -> Self {
        Self { dir: dir.to_path_buf(), listing: std::sync::OnceLock::new() }
    }

    fn find(&self, name: &str) -> Option<PathBuf> {
        let direct = self.dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        let listing = self.listing.get_or_init(|| {
            let mut map = std::collections::HashMap::new();
            if let Ok(rd) = std::fs::read_dir(&self.dir) {
                for p in rd.flatten().map(|e| e.path()) {
                    if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
                        map.entry(n.to_ascii_lowercase()).or_insert(p);
                    }
                }
            }
            map
        });
        listing.get(&name.to_ascii_lowercase()).cloned()
    }
}

#[derive(Debug)]
pub struct TrackAssets {
    pub name: String,
    pub dir: PathBuf,
    /// Decompressed `.crp` bytes (parts are resolved lazily against this buffer).
    pub crp_data: Vec<u8>,
    pub crp: Crp,
    /// The `.crp` file as found on disk (for error messages).
    crp_path: PathBuf,
    /// The track's `.fsh` and `.env` pixels (RGBA8, with their mip chains). Only the GPU upload reads
    /// them: [`Self::release_textures`] drops them once it is done (some 25-35 MB of a track) and
    /// [`Self::ensure_textures`] reads them again for the next upload (a restart, a replay pass).
    pub textures: Vec<TextureImage>,
    pub env_textures: Vec<TextureImage>,
    textures_released: bool,
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
    /// `REVERSE [0x657430]` for the race this track is loaded for: which start scene the race
    /// loads (0x46D600: `_BStart.scn` when set, else `_FStart.scn`) and so whose GEOM elements are
    /// placed. Set by the caller before [`TrackAssets::build_geometry`].
    pub reverse: bool,
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
        let lookup = DirLookup::new(&dir);
        let file = |suffix: &str| -> Result<PathBuf, AssetError> {
            let n = format!("{name}{suffix}");
            lookup.find(&n).ok_or_else(|| AssetError::MissingFile { dir: dir.clone(), name: n })
        };
        let sky_dir = dir.join("Sky");
        let sky_lookup = DirLookup::new(&sky_dir);

        // PORT-DEVIATION D7 (RE/notes/PORT-DEVIATIONS.md): the .crp read + unpack + parse, the track's
        // .fsh / .env decode and the sky pack's decode run on their own threads while this one reads
        // the small files; every result is kept and the errors are raised afterwards in the order the
        // sequential load met them, so the same file fails first and nothing is ordered by completion.
        // The original loads sequentially (load steps 0x4A4A70).
        let (crp_res, tex_res, sky_res, map_res, edges_res, scene_forward, scene_backward, scene_cameras, animdefs, draw_keys, sky_bin) = std::thread::scope(|s| {
            let crp_h = s.spawn(|| -> Result<(PathBuf, Vec<u8>, Crp), AssetError> {
                let crp_path = file(".crp")?;
                let crp_data = read_pack(&crp_path)?;
                let crp = Crp::parse(&crp_data).map_err(fmt_err(&crp_path))?;
                Ok((crp_path, crp_data, crp))
            });
            let tex_h = s.spawn(|| Self::decode_textures(&lookup, &dir, name));
            let sky_h = s.spawn(|| -> Result<(Vec<TextureImage>, Vec<String>), AssetError> {
                let (fsh, txt) = (sky_lookup.find(&format!("{name}.fsh")), sky_lookup.find(&format!("{name}.txt")));
                Ok(match (fsh, txt) {
                    (Some(fsh), txt) => {
                        let d = read_pack(&fsh)?;
                        let params = match txt {
                            Some(t) => String::from_utf8_lossy(&read(&t)?).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect(),
                            None => Vec::new(),
                        };
                        (decode_pack(&fsh, &d)?, params)
                    }
                    _ => (Vec::new(), Vec::new()),
                })
            });

            let map_res = file(".map").and_then(|map_path| Ok(Ini::parse(&String::from_utf8_lossy(&read(&map_path)?))));
            // one `animdefs.txt` for every track, beside the track files
            let animdefs = lookup
                .find("animdefs.txt")
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
            let edges_res = match file(".edg") {
                Ok(p) => read(&p).and_then(|d| EdgeFile::parse(&d).map(Some).map_err(fmt_err(&p))),
                Err(_) => Ok(None),
            };
            let sky_bin = sky_lookup
                .find(&format!("{name}.bin"))
                .or_else(|| sky_lookup.find("default.bin"))
                .and_then(|p| read(&p).ok())
                .and_then(|d| nfs_formats::sky::SkyBin::parse(&d).ok());
            let scene_forward = scene("_fstart.scn");
            let scene_backward = scene("_bstart.scn");
            let draw_keys = (keys("_forward.key"), keys("_backwards.key"));
            let scene_cameras = scene("_cameras.scn");
            (join_scoped(crp_h), join_scoped(tex_h), join_scoped(sky_h), map_res, edges_res, scene_forward, scene_backward, scene_cameras, animdefs, draw_keys, sky_bin)
        });

        let (crp_path, crp_data, crp) = crp_res?;
        let (textures, env_textures) = tex_res?;
        let map = map_res?;
        let edges = edges_res?;
        let (sky_textures, sky_params) = sky_res?;
        Ok(Self {
            name: name.to_string(),
            dir: dir.clone(),
            crp_path,
            crp_data,
            crp,
            textures,
            env_textures,
            textures_released: false,
            map,
            start_forward: scene_forward?,
            start_backward: scene_backward?,
            draw_keys_forward: draw_keys.0,
            draw_keys_backward: draw_keys.1,
            cameras: scene_cameras?,
            extra_scenes: Vec::new(),
            reverse: false,
            animdefs,
            edges,
            sky_textures,
            sky_params,
            sky_bin,
        })
    }

    /// The track's `.fsh` (pack key 0, the one with a mip chain: 0x4877BD, `texture::decode_pack_mipped`)
    /// and `.env` pixels.
    fn decode_textures(lookup: &DirLookup, dir: &std::path::Path, name: &str) -> Result<(Vec<TextureImage>, Vec<TextureImage>), AssetError> {
        let file = |suffix: &str| -> Result<PathBuf, AssetError> {
            let n = format!("{name}{suffix}");
            lookup.find(&n).ok_or_else(|| AssetError::MissingFile { dir: dir.to_path_buf(), name: n })
        };
        let fsh_path = file(".fsh")?;
        let fsh_data = read_pack(&fsh_path)?;
        let textures = decode_pack_mipped(&fsh_path, &fsh_data)?;
        let env_path = file(".env")?;
        let env_data = read_pack(&env_path)?;
        let env_textures = decode_pack(&env_path, &env_data)?;
        Ok((textures, env_textures))
    }

    /// Drop the texture pixels after the GPU upload (nothing else reads them).
    // PORT-DEVIATION D7 (RE/notes/PORT-DEVIATIONS.md): the CPU copies are dropped after upload and re-read by ensure_textures before a re-upload; host memory only, nothing drawn or simulated changes; the original loads sequentially (load steps 0x4A4A70).
    pub fn release_textures(&mut self) {
        self.textures = Vec::new();
        self.env_textures = Vec::new();
        self.textures_released = true;
    }

    /// Read the texture pixels again if [`Self::release_textures`] dropped them, for the next upload.
    pub fn ensure_textures(&mut self) -> Result<(), AssetError> {
        if self.textures_released {
            let (textures, env_textures) = Self::decode_textures(&DirLookup::new(&self.dir), &self.dir, &self.name)?;
            self.textures = textures;
            self.env_textures = env_textures;
            self.textures_released = false;
        }
        Ok(())
    }

    /// Load `Track/<track><suffix>` (e.g. `_st1.scn`) as a scene.
    pub fn load_scene(&self, suffix: &str) -> Result<Option<Scene>, AssetError> {
        let Some(p) = GamePaths::find_file(&self.dir, &format!("{}{}", self.name, suffix)) else { return Ok(None) };
        let text = String::from_utf8_lossy(&std::fs::read(&p).map_err(|e| AssetError::Io { path: p.clone(), source: e })?).into_owned();
        Ok(Some(Scene::parse(&text).map_err(fmt_err(&p))?))
    }

    /// Materials of the track CRP in `pr.material` order.
    pub fn materials(&self) -> Result<Vec<Material>, AssetError> {
        self.crp.materials(&self.crp_data).map_err(fmt_err(&self.crp_path))
    }

    /// Static track geometry at the lowest LOD of every article, grouped per article and material.
    /// Articles carrying a `tr` matrix are library objects instanced by `.scn` files (mapping not yet
    /// known) and are returned separately in `TrackGeometry::library`.
    pub fn build_geometry(&self) -> Result<TrackGeometry, AssetError> {
        let crp_path = &self.crp_path;
        let arts = self.crp.articles(&self.crp_data).map_err(fmt_err(crp_path))?;
        let mut world = Vec::new();
        let mut library = Vec::new();
        let mut skipped = 0usize;
        let flat = ambient_colours(&self.crp, &self.crp_data);
        for (index, a) in arts.iter().enumerate() {
            let g = self.crp.article_geometry(&self.crp_data, a).map_err(fmt_err(crp_path))?;
            let mut lod_numbers: Vec<u8> = g.primitives.iter().map(|p| p.lod).collect();
            lod_numbers.sort_unstable();
            lod_numbers.dedup();
            if lod_numbers.is_empty() {
                continue;
            }
            let mut lods = Vec::new();
            for &lod in &lod_numbers {
                let mut meshes = article_parts(&g, lod, &mut skipped);
                // an `Am` material's colour is its method's block, not its `df` stream
                for mm in &mut meshes {
                    if let Some(c) = flat.get(&mm.material) {
                        // 0x448C83 / 0x448C97: `LayerAmbient` stores the block's first dword **verbatim** over every
                        // vertex's colour slot — the same slot the `df` stream's dwords fill (0x540E1D), a D3D
                        // `0xAARRGGBB` (file bytes B, G, R, A; `mesh::append_primitive`). So the red channel is
                        // the third byte. *(Until 2026-10-02 read RGBA; unseen, every shipped block is 0xFFFFFFFF.)*
                        let [b, g, r, a] = c.to_le_bytes();
                        let c = [r, g, b, a].map(|x| x as f32 / 255.0);
                        mm.mesh.vertices.iter_mut().for_each(|v| v.color = c);
                    }
                }
                lods.push((lod, meshes));
            }
            let meshes = lods[0].1.clone();
            let transform = g.transform.as_ref().map(tr_to_mat4).unwrap_or(glam::Mat4::IDENTITY);
            let (is_lib, tag) = g.base.as_ref().map(|b| (b.is_library(), b.tag_u32())).unwrap_or((false, 0));
            let animation = self.crp.article_animation(&self.crp_data, a);
            let collides = !matches!(self.crp.bundle_of(&self.crp_data, a), Ok(nfs_formats::crp::BundleKind::Anim | nfs_formats::crp::BundleKind::MidGround));
            let item = ArticleMeshes { name: g.name.clone(), meshes, lods, transform, tag, animation, index, collides };
            if is_lib { library.push(item) } else { world.push(item) }
        }
        let mut instances = Vec::new();
        // 0x46D1D0 places every `CGeomElem` of the combined list `[0x628168]` (the cast to
        // 0x5CFA48 at 0x46D212), and `WTrack::load` 0x4876E0 fills that list with the generic
        // scenes and then **either** the skill test's `_ST<N>.scn` (0x46D330, game mode 4) **or**
        // the start scene (0x46D600) — `_BStart.scn` in a backwards race, `_FStart.scn` otherwise.
        // *(Until 2026-09-26 the port always placed `_FStart`'s geoms, in a skill test too: the
        // forward finish banners stood at the wrong end of a backwards race and the backwards
        // ones were never drawn.)*
        let start = if !self.extra_scenes.is_empty() {
            None
        } else if self.reverse {
            self.start_backward.as_ref()
        } else {
            self.start_forward.as_ref()
        };
        for scene in start.iter() {
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

#[cfg(test)]
mod dir_lookup_tests {
    use super::*;

    /// The cached listing answers like `GamePaths::find_file`: the exact spelling, else the entry
    /// that differs only in ASCII case, else nothing.
    #[test]
    fn matches_find_file() {
        let dir = std::env::temp_dir().join(format!("nfs-dirlookup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Alps_AUDIO.scn"), b"x").unwrap();
        std::fs::write(dir.join("alps.crp"), b"x").unwrap();
        let lookup = DirLookup::new(&dir);
        for name in ["alps.crp", "ALPS.CRP", "alps_audio.scn", "Alps_Audio.SCN", "alps.fsh", "alps_missing.key"] {
            assert_eq!(lookup.find(name), GamePaths::find_file(&dir, name), "{name}");
        }
        assert!(lookup.find("alps_audio.scn").is_some());
        assert!(lookup.find("alps.fsh").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod headlight_tests {
    use super::*;

    /// `night.psh`'s bitmap is the one 0x452910 indexes, and only a night track's methods build
    /// the vertex-light layers (`RE/notes/lighting.md` §5).
    #[test]
    fn night_map_and_vertex_light_methods() {
        let Ok(paths) = crate::GamePaths::locate() else { return };
        let m = night_map(&paths).expect("night.psh");
        assert_eq!(m.len(), 128 * 128);
        assert_eq!(m[64 * 128 + 64], 255, "the beam's centre");
        assert_eq!(m[0], 0);
        let night = TrackAssets::load(&paths, "monaco1").unwrap();
        let lit = vertex_light_methods(&night.crp, &night.crp_data);
        assert!(lit.contains("Normal") && lit.contains("Opaque"), "{lit:?}");
        let day = TrackAssets::load(&paths, "alps").unwrap();
        assert!(vertex_light_methods(&day.crp, &day.crp_data).is_empty());
    }

    /// The shared `RMth` scan answers exactly what the three separate scans (kept here as the
    /// reference) did, on every shipped track.
    #[test]
    fn rmth_scans_match_the_separate_reference_scans() {
        let Ok(paths) = crate::GamePaths::locate() else { return };
        let words = |bytes: &[u8]| -> Vec<u32> { bytes.chunks_exact(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect() };
        let reference = |t: &TrackAssets, pred: &dyn Fn(u32) -> bool| -> std::collections::HashSet<String> {
            t.crp
                .level
                .iter()
                .filter(|e| e.is_level(b"RMth"))
                .filter_map(|e| t.crp_data.get(e.data_offset()..e.data_offset() + e.data_len()))
                .filter(|bytes| bytes.len() > 16 && words(&bytes[16..]).into_iter().any(pred))
                .map(|bytes| bytes[..16].iter().take_while(|b| **b != 0).map(|b| *b as char).collect())
                .collect()
        };
        for name in ["alps", "monaco1", "canyon", "industrial", "coastal"] {
            let Ok(t) = TrackAssets::load(&paths, name) else { continue };
            assert_eq!(vertex_light_methods(&t.crp, &t.crp_data), reference(&t, &|w| matches!(w, 0x8002 | 0x8003)), "{name}");
            assert_eq!(uv_light_methods(&t.crp, &t.crp_data), reference(&t, &|w| w == 0x8004), "{name}");
            let any_headlight = t.crp.level.iter().filter(|e| e.is_level(b"RMth")).filter_map(|e| t.crp_data.get(e.data_offset()..e.data_offset() + e.data_len())).any(|b| words(b).into_iter().any(|w| HEADLIGHT_LAYERS.contains(&w)));
            assert_eq!(t.is_night(), any_headlight, "{name}");
        }
    }

    /// Exactly the materials with op-mask bit 0x10 build an `Am` op — `…_Ambient` 0x8011 by
    /// night, `LayerAmbient` 0x8007 by day — and every shipped `Am` block is opaque white.
    #[test]
    fn ambient_ops_follow_the_op_mask() {
        let Ok(paths) = crate::GamePaths::locate() else { return };
        for (name, class) in [("monaco1", 0x8011), ("alps", 0x8007)] {
            let t = TrackAssets::load(&paths, name).unwrap();
            let flat = ambient_colours(&t.crp, &t.crp_data);
            let methods = t.crp.render_methods(&t.crp_data).unwrap();
            let mats = t.materials().unwrap();
            assert!(!flat.is_empty());
            for (i, m) in mats.iter().enumerate() {
                let am = methods.iter().find(|r| r.name == m.render_method).is_some_and(|r| r.ops.iter().any(|o| o.class == class));
                assert_eq!(flat.contains_key(&(i as u16)), am && m.op_mask & 0x10 != 0, "{name} {i} {} {:#x}", m.render_method, m.op_mask);
            }
            assert!(flat.values().all(|&c| c == 0xFFFF_FFFF));
        }
    }

    /// Where a track method carries an `Rs` block, its blend state (0x0A = 2) says exactly what
    /// the port's former rule by name said — blend unless the method is `Opaque`. The `…MatState`
    /// methods carry none, and every one of their materials carries its own; so do 108 `FakeSort`
    /// ones (`mt+4` class 0x281) and one of monaco3's `Opaque` — all `{1, 2, 0, 0, 0}`, blending on
    /// and no culling.
    #[test]
    fn track_render_states_from_the_method_blocks() {
        let Ok(paths) = crate::GamePaths::locate() else { return };
        let mut own = 0;
        for name in TrackAssets::list(&paths) {
            let t = TrackAssets::load(&paths, &name).unwrap();
            let methods = t.crp.render_methods(&t.crp_data).unwrap();
            for m in t.materials().unwrap() {
                let rs = render_states(&methods, &m).unwrap_or_else(|| panic!("{name} {}", m.render_method));
                let method = methods.iter().find(|r| r.name == m.render_method).unwrap();
                if m.blocks.iter().any(|(k, _)| method.resolve(*k) == Some(0x5273_0001)) {
                    assert_eq!(rs, [1, 2, 0, 0, 0, -1], "{name} {}", m.render_method);
                    own += 1;
                } else {
                    assert!(!m.render_method.ends_with("MatState"), "{name} {}", m.render_method);
                    assert_eq!(rs[1] == 2, m.render_method != "Opaque", "{name} {}", m.render_method);
                    assert_eq!((rs[3], rs[5]), (1, -1));
                    assert_eq!(rs[4], if m.render_method.starts_with("S_") { -3 } else { 0 }, "{name} {}", m.render_method);
                }
            }
        }
        assert!(own > 0);
    }
}

#[cfg(test)]
mod quad_identity_tests {
    //! Quadtree prerequisite 1 (quad identity, `RE/RENDERING.md` "What
    //! the tree holds"): synthetic geometry, no game data needed, exercised through the real
    //! `article_parts` → `collision_grid` → `TriangleGrid::patches` path.
    use super::*;
    use nfs_formats::crp::{ArticleGeometry, Primitive};

    fn primitive(prim_type: u16, positions: Vec<[f32; 3]>) -> Primitive {
        Primitive { lod: 0, prim_type, flags: 0, material: 0, key_hi: 0x8000, sphere: [0.0; 4], positions, normals: vec![], uvs: vec![], colors: vec![], vertex_indices: vec![], uv_indices: vec![], vt_first: 0 }
    }

    /// One `prim_type == 4` quad and one `prim_type == 3` triangle in the same article: through
    /// `article_parts` and `collision_grid`, the quad's two triangles must come back as ONE patch
    /// (the original's `0x4853C0` builds one entry per quad, `RE/notes/
    /// suspension-collision.md` §1.2), the triangle as one patch of one triangle, and each must
    /// carry the article's own index — not lost by expanding the quad at parse time.
    #[test]
    fn a_quad_and_a_triangle_survive_collision_grid_as_two_patches() {
        let quad = primitive(4, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0]]);
        let tri = primitive(3, vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 2.0]]);
        let geometry = ArticleGeometry { name: "test".into(), base: None, transform: None, transforms: vec![], primitives: vec![quad, tri] };
        let mut skipped = 0;
        let meshes = article_parts(&geometry, 0, &mut skipped);
        assert_eq!(skipped, 0);
        assert_eq!(meshes.len(), 2);
        assert_eq!(meshes[0].patch_kind, nfs_physics::collision::PatchKind::Quad);
        assert_eq!(meshes[0].prim_index, 0);
        assert_eq!(meshes[1].patch_kind, nfs_physics::collision::PatchKind::Triangle);
        assert_eq!(meshes[1].prim_index, 1);

        let article = ArticleMeshes { name: "test".into(), meshes, lods: vec![], transform: glam::Mat4::IDENTITY, tag: 0, animation: None, index: 5, collides: true };
        let geometry = TrackGeometry { world: vec![article], library: vec![], instances: vec![], skipped_primitives: 0 };
        let grid = geometry.collision_grid(20.0, &|_| 1);
        let patches = grid.patches();
        assert_eq!(patches.len(), 2, "the quad's pair collapses to one patch, not two");

        let quad_patch = &patches[0];
        assert_eq!(quad_patch.kind, nfs_physics::collision::PatchKind::Quad);
        assert_eq!(quad_patch.article, 5, "the article's own index, not lost");
        assert_eq!(quad_patch.primitive, 0);
        assert_eq!(quad_patch.tris.len(), 2, "one patch, its two triangle indices");
        // ¼·Σ of the quad's four corners (0x4855C0), not a mean over both triangles' six corners
        assert!((quad_patch.centroid - glam::Vec3::new(0.5, 0.0, 0.5)).length() < 1e-6);

        let tri_patch = &patches[1];
        assert_eq!(tri_patch.kind, nfs_physics::collision::PatchKind::Triangle);
        assert_eq!(tri_patch.article, 5);
        assert_eq!(tri_patch.primitive, 1);
        assert_eq!(tri_patch.tris.len(), 1);
        assert!((tri_patch.centroid - glam::Vec3::new(2.0 / 3.0, 0.0, 2.0 / 3.0)).length() < 1e-6);
    }
}
