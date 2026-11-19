//! Assembling renderer scenes from loaded assets (shared by the game app and the viewer tools).

use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, track::TrackAssets};
use nfs_formats::bytes;
use nfs_renderer::{DrawItem, LayerLight, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};
use std::collections::HashMap;

mod car_model;
mod enhance;
mod props;
mod smackables;
mod lbeam;
mod lights;
mod anim;
mod cull;
mod hd_twins;
pub use car_model::*;
use enhance::*;
pub use props::*;
pub use smackables::*;
use lbeam::*;
pub use lights::*;
pub use anim::*;
pub use cull::*;
pub use hd_twins::*;

/// The light a track item is drawn under: none of its own, so the shader's `min(A + D·d, 1)`
/// leaves the texture × vertex colour product as it is ([`TrackWorld::visible_items`]).
pub const TRACK_LIGHT: LayerLight = LayerLight { ambient: [1.0; 3], diffuse: [0.0; 3], direction: [0.0; 3] };

/// The `.map` ambient's mean is clamped to this range ([`TrackWorld::ambient`]) ...
const AMBIENT_MIN: f32 = 0.2;
const AMBIENT_MAX: f32 = 0.9;
/// ... and a map with no usable `ambient` takes this.
const AMBIENT_DEFAULT: f32 = 0.35;
/// ENHANCEMENT ENH-DBLSH: a caster part whose half-diagonal (in world units) is up to this is one point box at its centre, a larger one its world box.
const DECAL_BOX_WHOLE: f32 = 60.0;
/// ENHANCEMENT ENH-DBLSH: a decal takes the caster box within this margin in x and z of its own box ...
const DECAL_MARGIN_XZ: f32 = 3.0;
/// ... this far below it ...
const DECAL_MARGIN_Y: f32 = 8.0;
/// ... and this far above it.
const DECAL_REACH_UP: f32 = 40.0;

/// GPU-resident track: static draw items plus the collision grid.
pub struct TrackWorld {
    /// Per item: the mission-scene element it belongs to (None for track geometry).
    pub item_element: Vec<Option<usize>>,
    /// Mission elements currently hidden (arrow markers not yet reached).
    pub hidden: std::collections::HashSet<usize>,
    pub items: Vec<DrawItem>,
    /// the `.env` "SKY " entry, else the first. *(2026-09-26: no longer bound to the cars — the
    /// specular layer draws `carcmn.fsh`'s `SPC` maps and the sky-map layer the section's, none
    /// outside a section; `RE/notes/lighting.md` §3.1, §3.2.)*
    pub env_map: Option<nfs_renderer::TextureId>,
    pub collision: nfs_physics::collision::TriangleGrid,
    /// The world's fitted spatial tree (`WTrack+0x10`, [`crate::fitted_tree`]): what the node pass
    /// walks and `WLineBundle`'s cells ([`crate::skidmarks::SkidMarks::for_tree`]) are cut from.
    pub fitted: std::sync::Arc<crate::fitted_tree::FittedTree>,
    /// The collision props the loader 0x4750B0 builds from the track's tagged articles
    /// ([`track_props`]) — `Car_propPass` 0x498DE0's objects.
    pub props: Vec<nfs_physics::props::Prop>,
    /// [`Self::collision`]'s own patches, one per original quad/triangle
    /// ([`nfs_physics::collision::TriangleGrid::patches`]) — kept alongside the grid because
    /// [`Self::tree`]'s slots index into this list by kind ([`nfs_physics::track_tree::TreeItem`]).
    pub patches: Vec<nfs_physics::collision::Patch>,
    pub walls: nfs_physics::walls::WallSet,
    /// `WTrack+0x24`, wired 2026-09-28: one [`nfs_physics::track_tree::TrackTree`] over
    /// [`Self::patches`], [`Self::props`] and [`Self::walls`], built in the original's own
    /// `insertAll(C)`-then-`insertAll(A)` order ([`track_tree_order`], `RE/RENDERING.md` "The
    /// tree, built" / "Bundle order, read 2026-09-28").
    pub tree: nfs_physics::track_tree::TrackTree,
    pub start: Vec3,
    /// The `.map` `ambient`'s mean, clamped 0.2…0.9 — the scene ambient the shader falls back on
    /// for a draw item with **no** `LayerLight` (`ambient + (1 − ambient) · max(n·sun, 0)`). Read
    /// 2026-09-30: **nothing in a race reaches it.** The track's items take
    /// [`TRACK_LIGHT`], the sky is unlit (`Sky::draw_items`, `RE/notes/thrash.md` §6d.5), the blob
    /// shadows take [`TRACK_LIGHT`] too (0x43B820 is its own primitive-helper `EAGL::Material`,
    /// 0x59F070/0x5379F0, no `R3DLightLayer`, no `.map` global), and **every car material carries
    /// an `R3DLightLayer`**: over the 44 shipped car `.crp`s (`nfs-formats/examples/mtlight.rs`) all
    /// 1578 `mt` records have the layer parameters (`Material::light` — `CarExt` 281, `CarExtEnv`
    /// 520, `CarInt` 530, `CarWheel` 165, and `CarWindow` 82, the glass), so
    /// `CarModel::draw_items_block` finds a light for each; the earlier note that the glass had none
    /// was wrong. The race's `block` (`World::car_light`) is always `Some`. A run with the scene
    /// ambient forced to 0 renders coastal's frame byte-identically, and a frame dump of the race
    /// scene lists 0 of 245 items without a `LayerLight`. What is left on this value is only an
    /// unreached fallback for an item nobody lights (a track or car built without the layer),
    /// where the original draws with whatever its material's own layer list says.
    pub ambient: f32,
    /// The `.map` header's five light colours (`RE/notes/lighting.md`).
    pub map_light: MapLight,
    /// The way the sun's light travels, `−normalize(SkyBin::sun_position)` (0x43AC8D…0x43AD36);
    /// straight down when the track has no sky configuration.
    pub sun_direction: Vec3,
    /// ENHANCEMENT E51 / E9 (CORSICA-SUN, SUN-ALL, SUN-FULL): on a day track whose sky has no sun (`sun_direction` straight down),
    /// the sun its data gives and the frame it drives ([`crate::data_sun`]); `None` elsewhere. Render only.
    pub data_sun: Option<crate::data_sun::DataSun>,
    /// ENHANCEMENT E23 / E47 (AUTOBAHN-LAMPS, SHADOW-UNIFY): a day track with no sun from any source lights its open street lamps
    /// by day ([`crate::data_sun::dusk_lamps`]). Render only.
    pub dusk_lamps: bool,
    /// ENHANCEMENT E9 (GATE-REL): on a day track the light its vertex colours were baked with, for E9's relative "already baked
    /// dark" gate ([`crate::bake_light::gate_light`]); `None` on a night track. Render only.
    pub bake_light: Option<nfs_renderer::BakeLight>,
    /// ENHANCEMENT E50 (SHADOW-OVERCAST): the sky's layers cover the sun (`SkyFile::blocks_the_sun`: Industrial); a night track so
    /// covered has no moon. Render only.
    pub sky_blocks_sun: bool,
    /// The race's smackables in `[WTrack+0x34]` order (0x4888B0): the mission scene's GEOM
    /// elements whose `animdefs.txt` definition says `kSmackable` (0x4772F0's `def+0x10 == 3`) —
    /// the skill test's cones — then the track's own placed `kSmackable` articles (signs,
    /// hydrants, pipes, sawhorses …), at most 125 in all (0x488926), with what their car records
    /// are built from (`nfs_physics::smackable`, `crate::smackables`).
    pub smackables: Vec<SmackableSpawn>,
    /// The draw items a smackable's body carries: by owner, the inverse of its authored pose and
    /// each item with its authored transform ([`Self::place_smackable`]).
    moved: HashMap<SmackableOwner, (Mat4, Vec<(usize, Mat4)>)>,
    /// The track smackables' side of the anim world (`suspension-collision.md` §9 "The track's
    /// smackables"): each one's player state and record `+8`.
    smack_anims: Vec<SmackAnim>,
    /// `.map` sections, and every `.env` entry by its SHPI name (`RE/notes/lighting.md` §3).
    pub map_sections: Vec<MapSection>,
    env_by_id: HashMap<u32, nfs_renderer::TextureId>,
    /// Props playing their own looping animation: an `.scn`-instanced library piece whose
    /// `animdefs.txt` definition loops it (`RE/notes/crp-geometry.md` §7 — the shipped file loops
    /// exactly two, the two dockers; every other `PIECE` animation there is played on impact, the
    /// breakables below). (history: `game-history.md` §9.1)
    /// (The smackables are car records whose player only follows the
    /// body, 0x471180 — none of their definitions has pieces.)
    animated: Vec<AnimatedProp>,
    /// The Locomotive's own path, for the game's audio update to sample every tick
    /// (`AnimatedSound::eval_pair`) and feed the ambient train sound's doppler. `None` on every
    /// track but Autobahn (`COVERAGE-2026-09-26.md` §14.38).
    pub animated_sound: Option<AnimatedSound>,
    /// The breakable props drawn in the world, with their pieces once a car has started them.
    broken: Vec<BrokenProp>,
    /// The kind-2 `ef` parts of the track's own articles — the flares 0x451940 draws
    /// (`RE/notes/crp-geometry.md` §8).
    pub flares: Vec<TrackFlare>,
    /// The kind-3 `ef` parts — the particle emitters 0x455150 starts (`particles.md` §6).
    pub emitters: Vec<crate::track_particles::Emitter>,
    /// The kind-4 `ef` parts — the car shadow projectors 0x456090 registers (`crp-geometry.md` §8)
    /// — and the one texture every shipped one names (`sh80`), uploaded.
    pub projectors: Vec<Projector>,
    pub projector_texture: Option<nfs_renderer::TextureId>,
    /// ENHANCEMENT ENH-LIVETEX: the originals' texture ids by name and the HD twins made next to them (`sync_hd`)
    tex_ids: HashMap<String, (nfs_renderer::TextureId, bool)>,
    hd: Option<HdSet>,
    /// Which projectors are in the map `[0x627DFC]`: 0x450DBD → 0x456090 puts one there when its
    /// article draws, and nothing takes it out before the track is unloaded (0x455FF0 ← 0x46912C).
    projector_registered: Vec<bool>,
    /// By `.crp` article index, the track walk's view of every placed article ([`CullArticle`],
    /// `RE/RENDERING.md` "The track draw's culling").
    pub cull_articles: Vec<Option<CullArticle>>,
    /// Per [`Self::cull_articles`] entry: every fitted-tree node the article is registered into
    /// ([`build_article_nodes`]: the hull spread for the static and mid-ground articles, 0x47BC20's
    /// one node for the instance and anim ones), each with its allocation number and fixed circle
    /// `(cx, cz, r)`, or an empty list for a `None` article — the node pass `RE/RENDERING.md` "The
    /// node pass 0x47F400 and the fitted tree's split 0x47BE30" describes (an article survives when
    /// *any* of its nodes does — `0x47F400`'s own tree walk visits a node iff every ancestor's fixed
    /// circle also passes, proven equivalent to a per-node independent check here since a child's
    /// circumscribed circle is always inscribed in its parent's, `COVERAGE` §14.36).
    article_nodes: Vec<Vec<RegisteredNode>>,
    /// Per [`Self::cull_articles`] entry: its place in the registration order
    /// ([`build_article_nodes`]) — the order refs sit in a node's list, which is the order the walk
    /// queues equal-node items in.
    article_seq: Vec<u32>,
    /// The track's own geometry, one entry per placed article (the breakables' apart), each LOD's
    /// draw items uploaded — what the walk's draw picks from per frame ([`Self::visible_items`]).
    /// (history: `game-history.md` §9.2)
    pub track_lods: Vec<TrackArticleGpu>,
    /// ENHANCEMENT E26: the centre (world space) of each road part's mesh (surface type 1): the point whose slice gives its wetness.
    pub road_centres: HashMap<MeshId, Vec3>,
    /// ENHANCEMENT E32: the baked bounce light's state (`crate::gi`); `None` on a night track and in the hard original.
    pub gi: Option<crate::gi::State>,
    /// The `Vis ` segments the walk's occlusion test 0x488B50 uses.
    pub occluders: Vec<Occluder>,
    /// The instance drawer's items outside [`Self::cull_articles`]: one per breakable
    /// (`WInstanceBundle`'s item, drawn apart so that its pieces can replace it) and one per `.scn`
    /// GEOM instance (`WSceneBundle`'s slot) — the walk answers them after the articles
    /// ([`Self::article_draws`]).
    instance_culls: Vec<InstanceCull>,
    /// per [`Self::instance_culls`] entry: its registration in the fitted tree
    instance_regs: Vec<InstanceReg>,
    /// Per item of [`Self::items`]: the [`Self::instance_culls`] entry it belongs to and the LOD
    /// number its meshes are of — drawn only when the walk answers that entry with that LOD.
    item_cull: Vec<Option<(usize, u8)>>,
    /// The kind-2 `ef` parts of the `.scn` GEOM instances' library articles, each with its
    /// [`Self::instance_culls`] entry and in the article's own frame: the scene drawer's 0x450380
    /// runs them (0x450883 → 0x451940) under the matrix it pushed for the item (0x450465…0x450477),
    /// within the LOD's part range — [`Self::running_flares`].
    instance_flares: Vec<(usize, TrackFlare)>,
    /// Materials whose render method builds `RCustomLayers::LayerUVAnimate` (class 0x8001) —
    /// water and flags/banners ([`Self::animate_uv`], `nfs_assets::track::uv_animate`,
    /// `RE/drafts/round1/render-core.md` §4.4).
    uv_animate: HashMap<MaterialId, nfs_assets::track::UvAnimate>,
    /// [`Self::animate_uv`]'s own bookkeeping: the animated items found once, and the reading last applied.
    uv_plan: UvPlan,
    /// The `WAnimBundle` articles the port moves ([`Self::animate_articles`]).
    anim_articles: Vec<AnimArticle>,
    /// ENHANCEMENT E44: the track's lighthouse beams ([`LBeamSrc`]) and whether their sheets are hidden now.
    lbeams: Vec<LBeamSrc>,
    lbeam_hidden: bool,
    /// The next registration number a moved ref takes.
    next_seq: u32,
}

/// An item of the instance drawer 0x5B3CB8 the port keeps apart from the `.crp` placement: its
/// article's `Base` sphere in the article's own frame and its LOD records (0x4786A0 / 0x450010 —
/// the LOD from the item's own article, the cursor 0x477C30); `first_item` is the draw item whose
/// transform is slot 5's matrix.
#[derive(Debug, Clone)]
struct InstanceCull {
    centre: Vec3,
    radius: f32,
    lods: Vec<CullLod>,
    first_item: usize,
    /// a breakable's [`TrackWorld::broken`] index: its matrix and, once its pieces play, its
    /// article come from there
    broken: Option<usize>,
    /// the `.crp` article of a `WInstanceBundle` item (a breakable, a track smackable): its place
    /// in the bundle's registration order; `None` for a `WSceneBundle` (`.scn`) item
    article: Option<usize>,
}

/// Where an instance / scene item sits in the fitted tree: the node 0x47BC20 answered for its
/// sphere at the last registration (0x489530's `+5` path), the number that orders it among the
/// refs of the node's list, and the sphere centre the registration compared against
/// (0x4895B4…0x4895E5).
#[derive(Debug, Clone, Copy)]
struct InstanceReg {
    node: RegisteredNode,
    seq: u32,
    centre: Vec3,
}

/// A placed article's uploaded LODs: by LOD number, the draw items (one per `pr` part, in the
/// file's order) at the article's own transform.
#[derive(Debug, Clone)]
pub struct TrackArticleGpu {
    /// the `.crp` article index — [`TrackWorld::cull_articles`]'s
    pub article: usize,
    pub lods: Vec<(u8, Vec<DrawItem>)>,
    /// per entry of [`Self::lods`], per draw item: its part's flag word, its key word (`pr+6`, the
    /// high half of the list-`+4` key) and its sphere centre in the world — what the static drawer
    /// queues it by ([`TrackWorld::visible_items`])
    pub parts: Vec<Vec<(u16, u16, Vec3)>>,
}

/// Where [`TrackWorld::animate_uv`] writes: an item of [`TrackWorld::items`] or one of a track
/// article's LOD items.
#[derive(Clone, Copy)]
enum UvTarget {
    Item(usize),
    Lod(usize, usize, usize),
}

/// The animated items of [`TrackWorld::animate_uv`], found on its first call, and the last reading.
#[derive(Default)]
struct UvPlan {
    shape: Option<(usize, usize)>,
    targets: Vec<(UvTarget, u32, i32)>,
    applied: Option<u32>,
}

/// The instance items [`TrackWorld::upload`] builds in its phases (breakables, smackables, the
/// scene's elements): the draw items, the walk's instance entries and each item's `(entry, LOD)`.
#[derive(Default)]
struct ItemLists {
    items: Vec<DrawItem>,
    instance_culls: Vec<InstanceCull>,
    item_cull: Vec<Option<(usize, u8)>>,
}

/// What [`TrackWorld::upload_scene_items`] returns besides the items themselves.
struct SceneItems {
    animated: Vec<AnimatedProp>,
    smackables: Vec<SmackableSpawn>,
    instance_flares: Vec<(usize, TrackFlare)>,
    /// the `WSceneBundle`'s items for the fitted tree's population: (article, world matrix)
    scene_items: Vec<(usize, Mat4)>,
}

/// One uploaded LOD of a library article: its LOD number and its meshes with their materials.
type LodMeshes = (u8, Vec<(MeshId, MaterialId)>);

/// The buffers [`TrackWorld::track_draw_lists_into`] fills, kept between frames.
#[derive(Default)]
pub struct TrackLists {
    /// list `+4`, unsorted: `(key, item)`
    pub near: Vec<(u32, DrawItem)>,
    /// the `+0xC` list's track-side entries, unsorted
    pub far: Vec<(u32, DrawItem)>,
    /// the port's no-`Base` fallback
    pub rest: Vec<DrawItem>,
    units: Vec<(Option<(u32, u32)>, QueueUnit)>,
}

/// One thing the walk queues, at its place in the node pass: a `.crp` article (`track_lods` index), a `.scn`
/// instance item (`items` index) or a breakable (`broken` index).
#[derive(Debug, Clone, Copy)]
enum QueueUnit {
    Lod(usize),
    Item(usize),
    Broken(usize),
}

/// The view's draw lists `[cam+0x54]` (built by 0x466380) that the track's drawers file into,
/// in the order the flush 0x466660 draws them (`RE/RENDERING.md` "The view's lists").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ViewList {
    /// `+4` (0x4668F0): ascending by key (the insert goes left on `key < node`, 0x466931 `cmp / jae`;
    /// the flush starts at the header's leftmost `+8`, 0x50E2C0, and steps in order, 0x423840), under
    /// the alpha test `GREATER 10` (0x46666D) — the parts with `pr` flag 2, keyed
    /// `(pr word +6 << 16) + round(d²)`
    Near,
    /// `+0xC` (0x466A90): descending by key, `GREATER 10` again (0x466703) — the parts with flag 1
    /// (and flag 4 while `[0x6197E6]`), the instance drawer's whole items (0x44FFC0) and the cars
    /// (0x443229)
    Far,
}

/// 0x450F30…0x450F9B: the list a static / mid-ground drawer's part goes to by its `pr` flag word
/// (`[part] >> 16`): 2 → `+4`, else 1 → `+0xC`, else 0x10 → `+8` (no shipped track part carries
/// it: `nfs-game/examples/prflags.rs`), else 4 → `+0xC` while `[0x6197E6]` (`SPECIALFX` ≥ 2 and
/// `[0x6197B0]` ≠ 0xE — taken as set, as for the projectors), else none: the part is not drawn.
fn part_list(flags: u16) -> Option<ViewList> {
    if flags & 2 != 0 {
        Some(ViewList::Near)
    } else if flags & 1 != 0 || flags & 4 != 0 {
        Some(ViewList::Far)
    } else {
        None
    }
}

/// `LayerFalseSort1`/`2`'s `mode 1` threshold, `[0x61975C]` against `0x2710` (0x449AB2, unsigned):
/// `nfs_renderer::FakeSortMode::PerItem`'s per-item fold.
const FAKESORT_NEAR_KEY: u32 = 10_000;

/// What a draw item with no recorded part (`TrackWorld::parts` shorter than the item list) is
/// queued by: the flag word 2 (the near list, [`part_list`]) with the usual key word 0x8000
/// (`pr+6`, the high half of the list-`+4` key: "0x8000 on most parts", 0x450F37…0x450F44) at the
/// origin.
const DEFAULT_PART: (u16, u16, Vec3) = (2, 0x8000, Vec3::ZERO);

impl TrackWorld {
    /// The track walk 0x47F890 for the `ef`-bearing articles, by `.crp` article index: the drawer's
    /// sphere through the frustum (0x539480), the `Vis ` occlusion (0x488B50 over the segments
    /// 0x488B00 keeps near the footprint), the distance — a mid-ground article's `d² < band²`
    /// drawn with `d²`, any other's `s² < draw²` with `s = max(√d² − r, 0)` — and the drawer's LOD:
    /// the first record with `trunc(v) < distance²` (0x450E36; the instance drawer's `v` comes
    /// rounded through the queue), none = not drawn.
    ///
    /// The node pass 0x47F400/0x47F310 over [`Self::cull_articles`] ([`Self::article_nodes`],
    /// [`node_wholly_pruned`], gated by [`NODE_PRUNE`]) would skip an article whose *every*
    /// registered node ([`build_article_nodes`], `0x47BDC0`'s starting node plus `0x489930`'s
    /// budgeted footprint spread — both now read to the instruction) is wholly outside the ground
    /// wedge or the far circle, before the per-item test runs. The registration itself is ported;
    /// the *switch* stays off — see [`NODE_PRUNE`]'s own doc for the real, disassembly-confirmed
    /// coverage gap that keeps it off. Not built for [`Self::instance_culls`] below either way
    /// (its footprint-hull registration was not measured).
    ///
    /// After the `.crp` articles come the instance drawer's items the port keeps apart, at
    /// `cull_articles.len() + k` in [`Self::instance_culls`] order: the `.scn` GEOM instances
    /// (`WSceneBundle`'s items — not `bl` items, 0x4892F0, so the ordinary occlusion and distance
    /// rule) and the breakables (`WInstanceBundle`'s), their spheres from this frame's matrices
    /// ([`Self::instance_sphere`]; `RE/RENDERING.md` "The instance and scene items").
    pub fn article_draws(&self, v: &CullView) -> Vec<Option<ArticleDraw>> {
        let mut out = Vec::new();
        self.article_draws_into(v, &mut out);
        out
    }

    /// [`Self::article_draws`] into a reusable buffer (cleared first).
    pub fn article_draws_into(&self, v: &CullView, out: &mut Vec<Option<ArticleDraw>>) {
        let (fc, fr2) = v.footprint();
        let near: Vec<&Occluder> = self.occluders.iter().filter(|o| !o.far_from(fc, fr2)).collect();
        let articles = self.cull_articles.iter().zip(self.article_nodes.iter()).zip(self.article_seq.iter()).map(|((a, nodes), &seq)| {
            let a = a.as_ref()?;
            // the first registered node the node pass does not drop (nodes are in tree order)
            let first = if NODE_PRUNE && !nodes.is_empty() { Some(nodes.iter().find(|n| !node_wholly_pruned(v, n.cx, n.cz, n.r))?.id) } else { nodes.first().map(|n| n.id) };
            let mut d = walk_item(v, &near, a.drawer, a.centre, a.radius, &a.lods)?;
            d.order = (first.unwrap_or(0), seq);
            Some(d)
        });
        let instances = self.instance_culls.iter().zip(self.instance_regs.iter()).map(|(ic, reg)| {
            let (c, r) = self.instance_sphere(ic)?;
            // the node pass drops an item whose registered node is wholly outside the wedge or the
            // far circle, as for an article (0x47F400); the list order is the registration's
            let RegisteredNode { id, cx: ncx, cz: ncz, r: nr } = reg.node;
            if NODE_PRUNE && node_wholly_pruned(v, ncx, ncz, nr) {
                return None;
            }
            let mut d = walk_item(v, &near, ArticleDrawer::Instance, c, r, &ic.lods)?;
            d.order = (id, reg.seq);
            Some(d)
        });
        out.clear();
        out.extend(articles.chain(instances));
    }

    /// The registration of the instance and scene items: 0x487D72's loop over the
    /// bundles calls 0x489370 for each — the scene bundle first (its ctor 0x482840 ran first, so
    /// its key `[bundle+0x28]`, the counter `[0x5D0608]` 0x48918C, is the smallest of the map
    /// 0x487D6D walks in order), the items spawned by 0x46D1D0 (0x487ABD) before it — and every
    /// item is put into the one node 0x47BC20 answers for its sphere (0x489530's `+5` path,
    /// 0x4895EB); [`Self::reregister_instances`] moves it when the sphere does.
    fn register_instances(&mut self) {
        let scene = self.instance_culls.iter().filter(|ic| ic.article.is_none()).count() as u32;
        // the scene items take the first numbers; the articles' registration numbers move up
        for q in self.article_seq.iter_mut().filter(|q| **q != u32::MAX) {
            *q += scene;
        }
        self.next_seq += scene;
        let mut next_scene = 0u32;
        let regs: Vec<InstanceReg> = self
            .instance_culls
            .iter()
            .map(|ic| {
                let (centre, r) = self.instance_sphere(ic).unwrap_or((ic.centre, ic.radius));
                let n = self.fitted.node_for_sphere(centre.x, centre.y, centre.z, r);
                let (cx, cz, nr) = self.fitted.circle(n);
                let seq = match ic.article {
                    Some(a) => self.article_seq.get(a).copied().filter(|&q| q != u32::MAX).unwrap_or(u32::MAX),
                    None => {
                        next_scene += 1;
                        next_scene - 1
                    }
                };
                InstanceReg { node: RegisteredNode { id: n.id, cx, cz, r: nr }, seq, centre }
            })
            .collect();
        self.instance_regs = regs;
    }

    /// 0x489370 over the instance and scene items on every eighth walk (0x488CC0 → 0x489530):
    /// the sphere's centre is compared with the stored one (three floats, 0x4895B4…0x4895E5) and
    /// only a changed centre is looked up again; a **different** node takes the ref at the end of
    /// its list (0x489850, 0x47C6F0).
    fn reregister_instances(&mut self) {
        for k in 0..self.instance_culls.len() {
            let Some((centre, r)) = self.instance_sphere(&self.instance_culls[k]) else { continue };
            if centre == self.instance_regs[k].centre {
                continue;
            }
            self.instance_regs[k].centre = centre;
            let n = self.fitted.node_for_sphere(centre.x, centre.y, centre.z, r);
            if n.id != self.instance_regs[k].node.id {
                let (cx, cz, nr) = self.fitted.circle(n);
                self.instance_regs[k].node = RegisteredNode { id: n.id, cx, cz, r: nr };
                self.instance_regs[k].seq = self.next_seq;
                self.next_seq += 1;
            }
        }
    }

    /// The track walk's gates for one view, for the items of `WLineBundle` — the skid marks' cells
    /// ([`crate::skidmarks::SkidMarks::cells_walked`]).
    pub fn cell_walk(&self, v: &CullView) -> CellWalk<'_> {
        CellWalk::new(v, &self.occluders)
    }

    /// 0x4786A0 for an item of [`Self::instance_culls`]: its article's `Base` sphere through slot
    /// 5's matrix (0x4780C0 / 0x4828A0) — the centre transformed, the radius the largest component
    /// of `M·(r, r, r) − M.t`. A GEOM instance's matrix is its first draw item's: the placement,
    /// with the looping animation or the smackable's body pose folded in (slot 5's `lead sample ×
    /// instance`, 0x4769E0). A breakable's is its `tr` while whole; once its pieces play, the
    /// first started piece's `Base` (the record's entry 0, 0x4786EC…0x478710) through `lead sample
    /// × tr` — the player's own radius (slot 8, 0x471150) is 0.
    fn instance_sphere(&self, ic: &InstanceCull) -> Option<(Vec3, f32)> {
        let (m, c, r) = match ic.broken {
            Some(k) => {
                let b = self.broken.get(k)?;
                match (&b.state, b.players.first()) {
                    (BreakState::Playing { .. }, Some(pl)) => {
                        let (c, r) = b.templates.get(pl.template).and_then(|t| t.base).unwrap_or((ic.centre, ic.radius));
                        (b.instance * b.lead.unwrap_or(Mat4::IDENTITY), c, r)
                    }
                    _ => (b.instance, ic.centre, ic.radius),
                }
            }
            None => (self.items.get(ic.first_item)?.transform, ic.centre, ic.radius),
        };
        Some((m.transform_point3(c), m.transform_vector3(Vec3::splat(r)).max_element()))
    }

    /// Whether the `ef` part `part` of kind `kind` of article `article` runs this frame, and with
    /// what value: the article is drawn, its LOD's byte `+1` is set, and its drawer runs that kind —
    /// the static / mid-ground drawer every part through 0x450D10; the instance / anim drawer kind 2
    /// (0x450883) within the LOD's part range (0x450819), and kind 3 only through 0x450980 on the
    /// animated branch of 0x450010 (0x45023D: the anim world's record `+8` set): from the first
    /// frame after a crate's pieces start or a smackable is pushed 1 cm off its place, **every
    /// drawn frame for the rest of the race** — `+8` is kept while the lead's state is 1 or 2
    /// (0x476810) and cleared only by the race start's reset 0x476170 (`suspension-collision.md`
    /// §9 "The track's smackables"). Every shipped kind-3 part on a `tr` article sits on a `BBOX` /
    /// `HYDR` / `PSTV` / `TRSH` prop (`nfs-game/examples/ef3anim.rs`). (history: `game-history.md` §9.3)
    /// A track smackable's article is
    /// drawn apart, so the walk's answer is its instance entry's (its sphere through the body's
    /// pose, 0x4786A0). The value is the walk's — *(settled 2026-09-27: 0x450980 does read the
    /// view's `+0xC` twice (its first argument, `[[esp+0x6C]+0xC]`, at 0x4509DC and again at
    /// 0x450ADD), but that field is `cam[+0xC]`, the per-frame `far · near / (far − near)` the
    /// view's build folds in every frame (0x467230…0x467261 off `[0x628130]`, `thrash.md` §6d.3)
    /// — the same `K` every other `C · K / d²` depth bias in the game shares (skids, particles,
    /// flares, the shadow, the lamp glare), not a distance at all. 0x450980 fild's it to a float and
    /// hands it straight to **0x455150** (`fstp` over the just-pushed slot, 0x450AED…0x450AFC) —
    /// the same helper the static / mid-ground drawer's kind-2 effects call the same way
    /// (0x450D10 → 0x450D8E, off the article's own `+0xC` there), to give the spawned effect
    /// instance its own future depth bias, not to gate it. The walk's distance — the article's
    /// squared `d2` ([`walk_item`]) — is the only distance either drawer computes, and it is what
    /// reaches the effect record (`ArticleDraw::value`, its doc above); `cam[+0xC]` is unrelated, so
    /// there was nothing to match here.)*
    pub fn effect_runs(&self, draws: &[Option<ArticleDraw>], article: usize, part: u16, kind: u32) -> Option<f32> {
        self.effect_run(draws, article, part, kind).map(|(v, _)| v)
    }

    /// [`Self::effect_runs`] with the trigger: `true` when the part goes through 0x450980, which
    /// hands the system's slot 3 (0x454420: emitting, period tick 0) the record's `+4` — 1 from
    /// the start on (0x476868), so every drawn frame (0x450B25…0x450B2F).
    pub fn effect_run(&self, draws: &[Option<ArticleDraw>], article: usize, part: u16, kind: u32) -> Option<(f32, bool)> {
        let apart = self.smack_anims.iter().find(|a| a.article == article).and_then(|a| a.slot);
        let (d, lod, drawer) = match apart {
            Some(slot) => {
                let d = draws.get(self.cull_articles.len() + slot).copied().flatten()?;
                (d, *self.instance_culls.get(slot)?.lods.get(d.lod)?, ArticleDrawer::Instance)
            }
            None => {
                let d = draws.get(article).copied().flatten()?;
                let a = self.cull_articles.get(article)?.as_ref()?;
                (d, *a.lods.get(d.lod)?, a.drawer)
            }
        };
        if !lod.effects {
            return None;
        }
        match drawer {
            ArticleDrawer::Static | ArticleDrawer::MidGround { .. } => Some((d.value, false)),
            ArticleDrawer::Instance | ArticleDrawer::Anim => {
                if !(lod.first..=lod.last).contains(&part) {
                    return None;
                }
                match kind {
                    2 => Some((d.value, false)),
                    3 => self.record_armed(article).then_some((d.value, true)),
                    _ => None,
                }
            }
        }
    }

    /// The anim world's record `+8` of the item whose article is `article`: set by 0x476810 once
    /// the lead's state is 1 and kept for the race — a crate from the frame its pieces start
    /// (the lead twin's state 1 at 0x4716E0 inside the hit, then 2, which keeps it), a track
    /// smackable from the update after the draw that found it pushed ([`Self::smackables_sample`]).
    fn record_armed(&self, article: usize) -> bool {
        self.broken
            .iter()
            .any(|b| !matches!(b.state, BreakState::Whole) && self.props.get(b.prop).and_then(|p| p.breakable.as_ref()).is_some_and(|x| x.article == article))
            || self.smack_anims.iter().any(|a| a.article == article && a.armed)
    }

    /// 0x476810 for the track's smackables, at the head of the frame's track draw (the prep
    /// 0x488CC0): a record whose lead is in state 1 and whose `+8` is clear gets `rec+4 = 1` and
    /// `rec+8` = its own article (0x476868…0x476876), and 0x4AF440 flags the nearest `Type`-1
    /// `CAudioElem` within 5 m — a byte nothing reads, so nothing is played.
    pub fn smackables_prep(&mut self) {
        for a in self.smack_anims.iter_mut().filter(|a| a.state == 1 && !a.armed) {
            a.armed = true;
            tracing::debug!(article = a.article, position = ?a.position, "smackable's record +8 set (0x476876): its kind-3 ef parts run from this frame (0x450980)");
        }
    }

    /// The draw's sample of each track smackable's player (0x450010's entry loop → 0x4769E0 →
    /// 0x4711E0) for the items the walk answered this frame: once the body has moved more than
    /// 0.01 in x or z off its record's place (0x4835B0, `[0x5B2BC0]`) the player's state turns 1
    /// (0x47139E) — for good, until the race's reset.
    pub fn smackables_sample(&mut self, draws: &[Option<ArticleDraw>]) {
        let n = self.cull_articles.len();
        for a in self.smack_anims.iter_mut().filter(|a| a.state != 1) {
            let Some(slot) = a.slot else { continue };
            if draws.get(n + slot).copied().flatten().is_none() {
                continue;
            }
            if (a.home.x - a.position.x).abs() > 0.01 || (a.home.z - a.position.z).abs() > 0.01 {
                a.state = 1;
                tracing::debug!(article = a.article, moved = ?(a.position - a.home), "smackable pushed off its place: its player's state 1 (0x4711E0 / 0x4835B0)");
            }
        }
    }

    /// Diagnostic: the `k`-th track smackable's article, its place (where its particle systems
    /// are) and its body's position now.
    pub fn track_smackable(&self, k: usize) -> Option<(usize, Vec3, Vec3)> {
        self.smack_anims.get(k).map(|a| (a.article, a.home, a.position))
    }

    /// ENHANCEMENT E23: every kind-2 flare of the track in world space, whether the walk runs it
    /// this frame or not (the placed articles' with an animated article's matrix as it stands, the
    /// `.scn` GEOM instances' through their item's matrix as placed at the upload). Where the street
    /// lamps are, for the lights; static data of the track (no per-frame walk).
    pub fn all_flares(&self) -> Vec<TrackFlare> {
        let placed = self.flares.iter().map(|f| match self.anim_articles.iter().find(|a| a.article == f.article) {
            Some(a) => TrackFlare { position: a.matrix.transform_point3(f.position), direction: a.matrix.transform_vector3(f.direction), ..f.clone() },
            None => f.clone(),
        });
        let instances = self.instance_flares.iter().filter_map(|(slot, f)| {
            let ic = self.instance_culls.get(*slot)?;
            let m = self.items.get(ic.first_item)?.transform;
            Some(TrackFlare { position: m.transform_point3(f.position), direction: m.transform_vector3(f.direction), ..f.clone() })
        });
        placed.chain(instances).collect()
    }

    /// The flares that run this frame, in world space: the placed articles' ([`Self::flares`],
    /// through [`Self::effect_runs`]) and the `.scn` GEOM instances' — the scene drawer's 0x450380
    /// runs an instance's kind-2 parts only when the walk answers its entry, the LOD's byte `+1` is
    /// set and the part lies in the LOD's range `+4…+6` (0x4507F4…0x450819, 0x450883), and 0x451940
    /// takes the part's point and direction through the matrix 0x450380 pushed for the item
    /// (0x450465…0x450477 — slot 5's, the looping animation or the smackable's pose folded in as for
    /// its draw items; the stack top at 0x451A0C…0x451A36). A hidden mission element is not drawn
    /// (its sphere is 10000 below the view in the original, 0x482E50), so neither are its flares.
    pub fn running_flares(&self, draws: &[Option<ArticleDraw>]) -> Vec<TrackFlare> {
        let mut out = Vec::new();
        self.running_flares_into(draws, &mut out);
        out
    }

    /// [`Self::running_flares`] into a caller's vector (cleared first): the per-frame path, which keeps one allocation across frames.
    pub fn running_flares_into(&self, draws: &[Option<ArticleDraw>], out: &mut Vec<TrackFlare>) {
        out.clear();
        let n = self.cull_articles.len();
        // a `WAnimBundle` article's flares are in its own frame: the drawer pushes the player's
        // matrix for the item (0x450380 → 0x451940, as for an instance)
        let placed = self.flares.iter().filter(|f| self.effect_runs(draws, f.article, f.part, 2).is_some()).map(|f| match self.anim_articles.iter().find(|a| a.article == f.article) {
            Some(a) => TrackFlare { position: a.matrix.transform_point3(f.position), direction: a.matrix.transform_vector3(f.direction), ..f.clone() },
            None => f.clone(),
        });
        let instances = self.instance_flares.iter().filter_map(|(slot, f)| {
            let d = draws.get(n + slot).copied().flatten()?;
            let ic = self.instance_culls.get(*slot)?;
            let lod = ic.lods.get(d.lod)?;
            if !lod.effects || !(lod.first..=lod.last).contains(&f.part) {
                return None;
            }
            if self.item_element.get(ic.first_item).copied().flatten().is_some_and(|e| self.hidden.contains(&e)) {
                return None;
            }
            let m = self.items.get(ic.first_item)?.transform;
            Some(TrackFlare { position: m.transform_point3(f.position), direction: m.transform_vector3(f.direction), ..f.clone() })
        });
        out.extend(placed.chain(instances));
    }
}

/// The `WSceneBundle`'s parked slots (0x482970 at 0x482C76, 0x482D3A…0x482D87, with `[+0x44]` set at
/// 0x487A51…0x487A5E): none while the scene has a GEOM element, else all 64 — a `DFLT` item at
/// (0, 9999.0, 0) (`0x461C3C00`) with the identity matrix (0x5BB2D0).
pub fn parked_scene_slots(geom_elements: usize) -> Vec<nfs_assets::track::PropInstance> {
    if geom_elements != 0 {
        return Vec::new();
    }
    let slot = nfs_assets::track::PropInstance { source: 0, element: 0, scale: Vec3::ONE, tag: u32::from_le_bytes(*b"DFLT"), transform: Mat4::from_translation(Vec3::new(0.0, 9999.0, 0.0)) };
    vec![slot; 64]
}

impl TrackWorld {
    /// 0x456160: the projector nearest to `pos` by `max(|dx|, |dz|) + 0.25 · min(|dx|, |dz|)`,
    /// below the running best from **400** and below its radius + 2.5.
    /// Only the registered ones are candidates ([`Self::register_projectors`]).
    pub fn projector_for(&self, pos: Vec3) -> Option<&Projector> {
        let mut best = 400.0f32;
        let mut found = None;
        for (p, _) in self.projectors.iter().zip(&self.projector_registered).filter(|(_, r)| **r) {
            let (dx, dz) = ((pos.x - p.point.x).abs(), (pos.z - p.point.z).abs());
            let d = dx.max(dz) + 0.25 * dx.min(dz);
            if d < best && d < p.radius + 2.5 {
                best = d;
                found = Some(p);
            }
        }
        found
    }

    /// 0x450D10's kind 4 for this frame's draws: a projector whose article the walk draws goes
    /// into the map `[0x627DFC]` (0x456090) and stays there. `[0x6197E6]` is taken as set.
    pub fn register_projectors(&mut self, draws: &[Option<ArticleDraw>]) {
        for k in 0..self.projectors.len() {
            if !self.projector_registered[k] && self.effect_runs(draws, self.projectors[k].article, self.projectors[k].part, 4).is_some() {
                self.projector_registered[k] = true;
            }
        }
    }
}

/// `R3DSkyMapTextureLayer`'s two planes for a car (0x445B10, `RE/notes/lighting.md` §3.2), over the
/// world point `p` the shader makes of `normal + 3.33 · position`:
///
/// ```text
/// u = ((p − o) · lat) / size          + 0.5 + 0.1 · ((o − c) · lat) / 6
/// v = ((p − o) · fwd) · aspect / size + 0.5 +       ((o − c) · fwd) / 6
/// ```
///
/// `o` is the car's `car+0x330`, copied to `[0x6115CC]+0xE8` (0x43A582); the matrix the layer uses
/// is the object's, then the translation by `−o` (0x445CB1) and the **transposed** `car+0x3B8`
/// frame — the slice's lateral, up and forward rows (0x43A862…0x43A8A4, `0x53A650` transposes) — of
/// which `u` takes column 0, `v` column 2 (0x445E04…0x445E42). `1 / size` is `[0x6115CC]+4` and
/// `aspect × 1 / size` its `+8` product (0x43A79D…0x43A7C1 from the section's `texturesize` /
/// `aspect`, 0x4411C0 / 0x4411E0). The offsets are `+0x144` = `(o − c) · lat / 6` and `+0x148` =
/// `(o − c) · fwd / 6` of the car's slice record (`c` its centre `+4`, 0x43A7C4…0x43A850, `[0x5B2420]`
/// = 1/6); the layer adds `0.1 · +0x144 + 0.5` and `+0x148 + 0.5` (0x445D2C…0x445D5D).
pub fn sky_planes(origin: Vec3, centre: Vec3, lateral: Vec3, forward: Vec3, texture_size: f32, aspect: f32) -> ([f32; 4], [f32; 4]) {
    let k = 1.0 / texture_size;
    let ka = aspect * k;
    let off = origin - centre;
    let u0 = 0.5 + 0.1 * (off.dot(lateral) / 6.0);
    let v0 = 0.5 + off.dot(forward) / 6.0;
    let u = lateral * k;
    let v = forward * ka;
    ([u.x, u.y, u.z, u0 - origin.dot(u)], [v.x, v.y, v.z, v0 - origin.dot(v)])
}

/// The three textures `RCar::load` hands the specular layer (0x441AA1…0x441B3E → 0x440940,
/// `[0x6115CC]+0xDC + 4·bucket`), all from `CarModel/carcmn.fsh` (the pack `[0x6115E8]`, loaded
/// from `"%scarcmn.fsh"` at 0x4437A9…0x4437E4 in both modes): `SPC1`, `SPC2`, `SPC3` in a race and
/// **`FEDU`, `FE  `, `FESH`** in the front end — the test is `[0x65B928] != 0` (0x441AA1…0x441AB0).
/// The shipped file has only `FESH`, `FE  `, `SPC2` and `SPC3`, so bucket 0 — a strength under
/// 0.05 — finds no texture (0x44D36D) in either mode and draws no specular pass.
pub fn car_specular_maps(r: &mut Renderer, paths: &nfs_assets::GamePaths, front_end: bool) -> [Option<nfs_renderer::TextureId>; 3] {
    let path = paths.car_model_dir().join("carcmn.fsh");
    let Ok(data) = std::fs::read(&path) else {
        return [None; 3];
    };
    let Ok(pack) = nfs_assets::texture::decode_pack(&path, &data) else {
        return [None; 3];
    };
    // the FourCCs 0x31/0x32/0x33435053 `SPC1…3` (0x441AB8…0x441AEA) or 0x55444546 `FEDU`,
    // 0x20204546 `FE  `, 0x48534546 `FESH` (0x441AF7…0x441B29); names compare trimmed
    let names = if front_end { ["FEDU", "FE", "FESH"] } else { ["SPC1", "SPC2", "SPC3"] };
    names.map(|name| {
        pack.iter()
            .find(|t| t.name.trim() == name)
            .map(|t| {
                // 8888 in the file, 4444 in the original's key-3 cache (0x44DFE0 → 0x44D7E0 → 0x5350E0 → 0x570540,
                // `nfs_assets::texture::cache_4444_from_8888`)
                r.upload_texture_mips(t.width, t.height, &t.cache_mips())
            })
    })
}

impl TrackWorld {
    /// ENHANCEMENT ENH-LIVETEX: E41 is on at a scale whose twins are not resident (the caller then needs the CPU textures of the track back:
    /// `TrackAssets::ensure_textures`).
    pub fn hd_pending(&self, r: &Renderer, track: &TrackAssets) -> bool {
        hd_scale_wanted(r).is_some_and(|scale| self.hd.as_ref().is_none_or(|h| h.scale != scale)) && hd_pack_for(r, track).is_some()
    }

    /// ENHANCEMENT ENH-LIVETEX: bring the HD twins in line with E41's level after a change (the A/B flip needs nothing: the renderer shows
    /// the twins or the originals by the switch). Level 0: nothing (a resident set stays for the way back). A level whose scale is not the
    /// resident one (first use, x2 <-> x4): that pack is loaded now next to the originals and replaces the old twins (a short hitch).
    pub fn sync_hd(&mut self, r: &mut Renderer, track: &TrackAssets) {
        if !self.hd_pending(r, track) {
            return;
        }
        let Some(pack) = hd_pack_for(r, track) else { return };
        let jobs = track_hd_jobs(track, &self.tex_ids);
        let set = upload_hd_twins(r, pack, &jobs, "track (live)");
        replace_hd_set(r, &mut self.hd, set);
    }

    /// The sky-map texture of layer 0 at `slice`: the `.map` section covering it, in its covered or
    /// open variant (`RE/notes/lighting.md` §3, §3.2). `None` outside every section, where the
    /// original binds none (0x4411AD) — *corrected 2026-09-26: not "the default maps"*.
    pub fn env_for(&self, slice: u32, covered: bool) -> Option<nfs_renderer::TextureId> {
        self.env_layers_for(slice, covered)[0]
    }

    /// All three of the section's environment textures — `EnvSet::texture` 0x441180 answers
    /// `ids[layer + 3·covered]` for the layers 0, 1 and 2, and the frame binds all three
    /// (0x43A73D…0x43A778, `RE/notes/lighting.md` §3.1).
    pub fn env_layers_for(&self, slice: u32, covered: bool) -> [Option<nfs_renderer::TextureId>; 3] {
        let Some(sec) = self.map_sections.iter().find(|s| slice >= s.start && slice <= s.end) else {
            return [None; 3];
        };
        std::array::from_fn(|l| self.env_by_id.get(&sec.ids[l + 3 * covered as usize]).copied())
    }

    /// A car's `R3DSkyMapTextureLayer` (`RE/notes/lighting.md` §3.2), built per car as 0x43A560
    /// does it: the section of the car's own slice (`car+8`, 0x43A71F → 0x441140), its three maps
    /// in the open or covered variant by the car's own enclosure test (0x473330's flags `& 0xC000`,
    /// 0x43A6E8), and the planes of [`sky_planes`] for the car's `car+0x330` (`origin`) against
    /// that slice's record. `None` when no map is bound — no section covers the slice — where the
    /// layer draws nothing. The caller gates it on `[0x6197CB]` (`Graphics.CarChrome` High).
    pub fn sky_map(&self, origin: Vec3, slice: usize, record: &crate::race::Slice) -> Option<nfs_renderer::SkyMap> {
        let sec = self.map_sections.iter().find(|s| slice as u32 >= s.start && slice as u32 <= s.end)?;
        let covered = self.overhead_flags(origin) & 0xC000 != 0;
        let textures: [Option<nfs_renderer::TextureId>; 3] = std::array::from_fn(|l| self.env_by_id.get(&sec.ids[l + 3 * covered as usize]).copied());
        if textures.iter().all(Option::is_none) {
            return None;
        }
        let (u, v) = sky_planes(origin, record.centre, record.lateral, record.forward, sec.texture_size, sec.aspect);
        Some(nfs_renderer::SkyMap { textures, u, v })
    }

    /// The original's enclosure test (0x473330, `race-logic.md` §12): the flags word of the lowest
    /// track patch more than **3.5 m** above the point, or 0 when there is none. `Tracker` patches
    /// carry the CRP `mt` flags word at `patch+0xA` (`suspension-collision.md` §7), which is the
    /// same word the port keeps in the low 16 bits of a collision triangle's surface id.
    pub fn overhead_flags(&self, pos: Vec3) -> u16 {
        self.collision.ceiling(pos.x, pos.y, pos.z, 3.5, 200.0).map(|h| (h.surface & 0xFFFF) as u16).unwrap_or(0)
    }

    /// The race car's light block, 0x43A8A6…0x43AD36 (`RE/notes/lighting.md` §6): the ground
    /// patch's colour `g` under `pos` (0x4746A0) times the `.map` header colours, slot by slot —
    /// 0 `g·ambient`, 1 `g·ambient·interior`, 2 `g·ambient·lightdecal`, 4 `g·tint`, 5
    /// `g·interiordiffuse`, 6 `g·tint·lightdecal` — `+0x74` white (0x43A5AC), and the direction
    /// straight down under cover (0x473330's flags `& 0xC000`, 0x43A6E8 / 0x43AB19) or else along
    /// the negated sun of the track's sky configuration (0x43AC8D, `SkyBin::sun_position`).
    pub fn car_light(&self, pos: Vec3) -> LightBlock {
        // 0x4746A0: the tracker's drivable patch (`word & 0xF`), not the topmost triangle — a road decal with no surface nibble
        // (the finish line's chequered strip) is not lit from; white with no patch (0x474700's default)
        let g = self.collision.patch_color(pos).map(|c| [c[0], c[1], c[2]]).unwrap_or([1.0; 3]);
        let m = &self.map_light;
        let mul = |a: [f32; 3], b: [f32; 3]| [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
        let amb = mul(g, m.ambient);
        let tint = mul(g, m.tint);
        let covered = self.overhead_flags(pos) & 0xC000 != 0;
        Self::car_light_block(amb, tint, g, m, covered, self.sun_direction)
    }

    fn car_light_block(amb: [f32; 3], tint: [f32; 3], g: [f32; 3], m: &MapLight, covered: bool, sun: Vec3) -> LightBlock {
        let mul = |a: [f32; 3], b: [f32; 3]| [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
        LightBlock {
            colours: [amb, mul(amb, m.interior), mul(amb, m.light_decal), [0.0; 3], tint, mul(g, m.interior_diffuse), mul(tint, m.light_decal)],
            direction: if covered { Vec3::NEG_Y } else { sun },
            material: [1.0; 3],
        }
    }

    /// Whether the car is under a tunnel or bridge deck — bit 15 of the patch overhead
    /// (`engine-audio.md` §4.7). Bits 14/15 mark the **drivable road** materials
    /// (`suspension-collision.md` §7), so a rock overhang or a roof does not count; road above you
    /// does.
    pub fn in_tunnel(&self, pos: Vec3) -> bool {
        self.overhead_flags(pos) & 0x8000 != 0
    }

    pub fn upload(r: &mut Renderer, track: &TrackAssets) -> anyhow::Result<Self> {
        let geometry = track.build_geometry()?;
        let materials = track.materials()?;
        let tex_ids = Self::upload_track_textures(r, track);
        // ENHANCEMENT E41 / ENH-LIVETEX: the originals are always uploaded as master does; with E41 on and a pack present, the pack's images
        // (x2 / x4 by the level) are uploaded NEXT TO them as their twins, and a material shows the twin while E41 is on (the sizes of
        // everything else, the UVs, stay the original's: the shader samples normalized UVs). Off: no pack lookup at all, nothing extra.
        let mut hd_set: Option<HdSet> = None;
        if let Some(pack) = hd_pack_for(r, track) {
            let jobs = track_hd_jobs(track, &tex_ids);
            hd_set = Some(upload_hd_twins(r, pack, &jobs, "track"));
        }
        let methods = track.crp.render_methods(&track.crp_data).unwrap_or_default();
        let mut mat_ids: HashMap<u16, MaterialId> = HashMap::new();
        let mut material_for = |r: &mut Renderer, id: u16| {
            *mat_ids.entry(id).or_insert_with(|| {
                let m = materials.get(id as usize);
                let tex = m.and_then(|m| tex_ids.get(&m.texture_name()));
                // blending is the `Rs` block's state 0x0A (2 = on, `thrash.md` §6c) — the material's
                // own block where its `mt` carries one (the `…MatState` materials, 0x44AD3B), else
                // its method's; with it on, an opaque texture draws as if it were off. Every shipped
                // material has a block; the rule by name stays only for a broken file
                let rs = m.and_then(|m| nfs_assets::track::render_states(&methods, m));
                let blend = |m: &nfs_formats::crp::Material| rs.map_or(m.render_method != "Opaque", |rs| rs[1] == 2);
                let alpha = tex.map(|t| t.1).unwrap_or(false) && m.map(blend).unwrap_or(true);
                // the cull, Thrash state 2 (`thrash.md` §6d.2): 1 is the view's own — back faces,
                // the other winding under `MIRROR` (`view_cull`) — and 0 none (the `…MatState`
                // materials' own blocks)
                let cull_back = rs.is_none_or(|rs| rs[3] == 1);
                // the depth bias, Thrash state 0x18 (`thrash.md` §6d.3): −3 on the `S_Normal` /
                // `S_UVAnimate` ground shadows (castle, coastal, farmland, forest, canyon), 0 else
                let depth_bias = rs.map_or(0, |rs| rs[4]);
                // `FakeSort`'s `LayerFalseSort1` / `2` (0x800F / 0x8010): drawn twice, depth writes
                // off and then on, at `SPECIALFX` 3 always and at 2 by the runtime pick; below 2
                // (0x44EC80's jump table, 0 and 1 unconditionally) one draw, texture filter
                // `POINT` instead — `Renderer::set_fakesort_mode` (`FakeSortMode`), gated on
                // `App::fakesort_mode` in `porsche/main.rs` (`thrash.md` §6d.4, read to the
                // instruction 2026-09-27). This flag only says the material *carries* the
                // `FakeSort` method; which of the two arms (or the runtime per-item fold,
                // `DrawItem::fakesort_near` set below) draws it is decided per frame/per item.
                let false_sort = m.and_then(|m| methods.iter().find(|r| r.name == m.render_method).map(|r| r.built_ops(0, m.op_mask).any(|o| o.class == 0x800F))).unwrap_or(false);
                // the material's address modes, states 0x41 / 0x42 (`thrash.md` §7): 0 clamps
                let clamp = m.map_or([false; 2], |m| m.address.map(|a| a == 0));
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp, lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: true, view_cull: true, depth_bias, false_sort, reflect: nfs_renderer::Reflect::NONE })
            })
        };
        // The track's own geometry: the walk 0x47F890 draws each article on its own through its
        // drawer with the LOD the distance picks (`RE/RENDERING.md` "The track draw's culling"), so
        // every placed article keeps its own meshes per LOD and per material, at its own transform.
        // (history: `game-history.md` §9.4)
        // `WSceneBundle`'s own tree contribution (`scene_props`'s doc: `0x46D1D0` over `[0x628168]`)
        // is appended after the crp-article props so every index a `Breakable`/`SmackableSpawn`
        // stored before this point (keyed by `.crp` article, not by position in `props`) stays
        // valid; the caller (`track_tree_order`, below) is told the appended range so it can
        // still put the scene bundle *first* in the tree's own insertion order, matching the
        // original (`0x487abd` runs before any other bundle's `Load`).
        let mut props = track_props(track);
        let scene_start = props.len();
        props.extend(scene_props(track, &geometry));
        let scene_indices: Vec<usize> = (scene_start..props.len()).collect();
        // a breakable's own article is drawn on its own so that its pieces can replace it
        let breakable_articles: std::collections::HashSet<usize> = props.iter().filter_map(|p| p.breakable.as_ref().map(|b| b.article)).collect();
        // the track's smackables (0x4772F0 over the instance bundle), after the mission scene's in
        // `[WTrack+0x34]` and at most 125 in all (0x4888B0, 0x488926): each is a car record whose
        // article is drawn apart, at its body's pose
        let scene_smackables = geometry
            .instances
            .iter()
            .filter(|i| i.source == 1 && geometry.library.iter().any(|a| a.tag == i.tag) && track.animdefs.lookup(i.tag).is_some_and(|d| d.collide_type == "kSmackable"))
            .count();
        let track_smackables: Vec<SmackableSpawn> = track_smackable_articles(track)
            .into_iter()
            .filter_map(|k| geometry.world.iter().find(|a| a.index == k))
            .filter_map(|a| track_smackable_spawn(track, a))
            .take(MAX_SMACKABLES.saturating_sub(scene_smackables))
            .collect();
        let smackable_articles: std::collections::HashSet<usize> = track_smackables
            .iter()
            .filter_map(|s| match s.owner {
                SmackableOwner::Article(k) => Some(k),
                SmackableOwner::Element(_) => None,
            })
            .collect();
        // The one world article whose `Base` tag carries an `animdefs.txt` `SECTION:Audio`
        // `SoundDef` (`engine-audio.md` §7.3/§7.4, `COVERAGE-2026-09-26.md` §14.38): on every
        // shipped track this is at most one article — autobahn's `"Locomotive"`, `tag=TRAN`. It
        // owns its own `Anqt` track (not an `animdefs.txt SECTION:ANIM`/`PIECE` — `TRAN` has none —
        // so it is *not* one of the two looping dockers this struct's `animated` field doc
        // describes; that doc predates this finding) and is drawn through the same
        // [`AnimatedProp`]/[`TrackWorld::animate`] machinery as those, not through `track_lods`'
        // fixed-transform path, so it is excluded from the static loop below.
        let sound_articles: std::collections::HashSet<usize> = geometry
            .world
            .iter()
            .filter(|a| a.animation.is_some() && track.animdefs.sound_by_fourcc(a.tag).is_some())
            .map(|a| a.index)
            .collect();
        // ENHANCEMENT E26: `road_centres`, the wet road's mesh centres
        let (mut track_lods, road_centres) = Self::upload_track_lods(r, &geometry, &materials, &|k| breakable_articles.contains(&k) || smackable_articles.contains(&k), &mut material_for);
        let mut lists = ItemLists::default();
        let broken = Self::breakables(r, track, &geometry, &props, &mut lists, &mut material_for);
        let (moved, smack_anims) = Self::upload_track_smackables(r, track, &geometry, &track_smackables, &mut lists, &mut material_for);
        // The `sound_articles` object above ("Locomotive"/`tag=TRAN`) only feeds the ambient
        // sound here ([`AnimatedSound`]); its picture is drawn like every other `WAnimBundle` article
        // (`track_lods`, [`TrackWorld::animate_articles`]). A `WAnimBundle` article with no
        // `animdefs.txt` piece plays at 0.1 s a frame, always looping (`WAnimBundle::Load` 0x4777F0,
        // 0x4779DF…0x4779F7: `0x471490(…, flags=[0x5B45A4] = 1, 0.1f, …)`), started at track load
        // (0x477A0E) and sampled on the race clock from phase 0. CONFIRMED; the full trace:
        // `RE/notes/game-history.md` §33.3.
        let animated_sound = Self::animated_sound(track, &geometry, &sound_articles);
        let lib_gpu = Self::upload_library(r, &geometry, &mut material_for);
        let arts = track.crp.articles(&track.crp_data).unwrap_or_default();
        let mut item_element = vec![None; lists.items.len()];
        let SceneItems { animated, smackables, instance_flares, scene_items } = Self::upload_scene_items(track, &geometry, &lib_gpu, &arts, &mut lists, &mut item_element);
        let ItemLists { mut items, instance_culls, item_cull } = lists;
        let (env_by_id, env_map) = Self::upload_env_textures(r, track);
        let flags: Vec<u32> = materials.iter().map(|m| m.flags).collect();
        let collision = geometry.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
        // (history: `game-history.md` §9.6)
        let walls = nfs_physics::walls::WallSet::build(track.edges.iter().flat_map(|e| e.segments.iter().map(|s| s.loaded())), 30.0);
        // `WTrack+0x24`, wired: `insertAll(C)` (patches + `kCylinder`/`kBox` props, bundle by
        // bundle, `RE/RENDERING.md` "Bundle order, read 2026-09-28") then `insertAll(A)` (every
        // `.edg` wall in file order, `WallSet::build`'s own input order above).
        let patches = collision.patches();
        let mut tree_order = track_tree_order(&track.crp, &track.crp_data, &arts, &geometry.world, &patches, &props, &scene_indices);
        tree_order.extend((0..walls.walls().len()).map(nfs_physics::track_tree::TreeItem::Wall));
        let tree = nfs_physics::track_tree::TrackTree::build(&patches, &props, walls.walls(), &tree_order);
        let start = track
            .start_forward
            .as_ref()
            .and_then(|s| s.of_kind(&nfs_formats::scn::ElementKind::Trigger).next())
            .map(|e| Vec3::from(e.position))
            .unwrap_or(Vec3::ZERO);
        let ambient = track
            .map
            .section("header")
            .and_then(|h| h.get_f32_list("ambient"))
            .filter(|a| a.len() == 4)
            .map(|a| ((a[1] + a[2] + a[3]) / 3.0).clamp(AMBIENT_MIN, AMBIENT_MAX))
            .unwrap_or(AMBIENT_DEFAULT);
        tracing::info!(items = items.len(), triangles = collision.triangle_count(), walls = walls.len(), "track uploaded");
        if !animated.is_empty() {
            tracing::info!(props = animated.len(), "animated props");
        }
        let map_light = MapLight::from_map(&track.map);
        let SunInfo { sun_direction, data_sun, dusk_lamps, bake_light, sky_blocks_sun } = sun_info(track, &geometry, &materials);
        let map_sections = MapSection::all(&track.map);
        Self::mark_lit_items(track, &materials, &mat_ids, &mut items, &mut track_lods);
        // materials whose render method builds `RCustomLayers::LayerUVAnimate` (0x8001) — water
        // and flags/banners (`RE/drafts/round1/render-core.md` §4.4)
        let uv_animate: HashMap<MaterialId, nfs_assets::track::UvAnimate> = mat_ids
            .iter()
            .filter_map(|(id, mid)| materials.get(*id as usize).and_then(|m| nfs_assets::track::uv_animate(&methods, m)).map(|a| (*mid, a)))
            .collect();
        let sheet_crp = tag_water_and_sheets(track, &materials, &methods, &mat_ids, &mut items, &mut track_lods);
        tag_decals_under_casters(&geometry, &materials, &methods, &mat_ids, &items, &mut track_lods);
        tracing::info!(props = props.len(), breakable = props.iter().filter(|p| p.breakable.is_some()).count(), "track props");
        let projectors = track_projectors(track);
        let projector_texture = projectors.first().and_then(|p| tex_ids.get(&p.texture)).map(|t| t.0);
        let projector_registered = vec![false; projectors.len()];
        let cull_articles = cull_articles(track);
        let population = tree_population_with_scene(track, &cull_articles, &scene_items);
        let fitted = std::sync::Arc::new(crate::fitted_tree::FittedTree::new(population.min, population.max, &population.points));
        let (article_nodes, article_seq) = build_article_nodes(&fitted, &cull_articles);
        let next_seq = article_seq.iter().filter(|&&q| q != u32::MAX).count() as u32;
        let anim_articles = Self::anim_articles(track, &cull_articles, &mut track_lods);
        let lbeams = find_lbeams(&geometry, &anim_articles, &sheet_crp, &track_lods);
        let gi = build_gi_state(track, &geometry, &track_lods, &anim_articles, &breakable_articles, &smackable_articles);
        let occluders = track_occluders(track);
        tracing::info!(articles = cull_articles.iter().flatten().count(), drawn = track_lods.len(), lods = track_lods.iter().map(|a| a.lods.len()).sum::<usize>(), occluders = occluders.len(), instances = instance_culls.len(), "track culling");
        let mut world = Self {
            items,
            item_element,
            hidden: std::collections::HashSet::new(),
            env_map,
            collision,
            fitted,
            props,
            patches,
            walls,
            tree,
            start,
            ambient,
            map_light,
            sun_direction,
            data_sun,
            dusk_lamps,
            bake_light,
            sky_blocks_sun,
            map_sections,
            env_by_id,
            smackables: smackables.into_iter().chain(track_smackables).take(MAX_SMACKABLES).collect(),
            moved,
            smack_anims,
            animated,
            animated_sound,
            broken,
            flares: track_flares(track),
            emitters: track_emitters(track),
            projectors,
            projector_texture,
            tex_ids,
            hd: hd_set,
            projector_registered,
            cull_articles,
            article_nodes,
            article_seq,
            track_lods,
            road_centres,
            gi,
            occluders,
            instance_culls,
            instance_regs: Vec::new(),
            instance_flares,
            item_cull,
            uv_animate,
            uv_plan: UvPlan::default(),
            anim_articles,
            lbeams,
            lbeam_hidden: false,
            next_seq,
        };
        world.register_instances();
        Ok(world)
    }

    /// Every track texture in one go: the pixels are written from worker threads, the ids are the
    /// same consecutive ones, in file order. Keyed by name: `(id, has_alpha)`.
    fn upload_track_textures(r: &mut Renderer, track: &TrackAssets) -> HashMap<String, (nfs_renderer::TextureId, bool)> {
        let mut tex_ids = HashMap::new();
        // ENHANCEMENT E2: a texture the data ships with ONE level gets a generated chain (alpha-tested
        // coverage kept for the cut-outs); sampled at level 0 only while E2 is off, so master's result
        let single = track.textures.iter().filter(|t| t.mips.len() == 1 && t.width.max(t.height) > 1).count();
        tracing::info!(single_level = single, total = track.textures.len(), "track textures without mips (ENHANCEMENT E2 generates their chains)");
        let items: Vec<(u32, u32, &[Vec<u8>], Option<nfs_renderer::MipAlpha>)> =
            track.textures.iter().map(|t| (t.width, t.height, t.mips.as_slice(), Some(nfs_renderer::MipAlpha::for_texture(t.has_alpha)))).collect();
        for (tex, id) in track.textures.iter().zip(r.upload_textures_gen(&items)) {
            tex_ids.insert(tex.name.clone(), (id, tex.has_alpha));
        }
        tex_ids
    }

    /// Every placed world article's meshes per LOD at its own transform (the walk 0x47F890 draws
    /// each through its drawer), but those `skip` names (the breakables', the smackables'), with
    /// each part's flag word, key word and world sphere centre.
    fn upload_track_lods(
        r: &mut Renderer,
        geometry: &nfs_assets::track::TrackGeometry,
        materials: &[nfs_formats::crp::Material],
        skip: &dyn Fn(usize) -> bool,
        material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId,
    ) -> (Vec<TrackArticleGpu>, HashMap<MeshId, Vec3>) {
        let mut track_lods: Vec<TrackArticleGpu> = Vec::new();
        let mut road_centres: HashMap<MeshId, Vec3> = HashMap::new();
        for art in &geometry.world {
            if skip(art.index) {
                continue;
            }
            let lods = art
                .lods
                .iter()
                .map(|(lod, meshes)| {
                    // ENHANCEMENT E9: every part takes a car's sun shadow, a part whose material is not drivable ground
                    // (surface type `flags & 0xF` = 0, `collision_grid`) also casts one
                    (*lod, meshes.iter().map(|mm| {
                        let mesh = r.upload_mesh(&mm.mesh);
                        let surface = materials.get(mm.material as usize).map(|m| m.flags & 0xF);
                        // ENHANCEMENT E26: surface type 1 (tarmac, the type the original's wet spray tests, `wheel_input`) is the road
                        // the rain wets; its mesh centre (world space) is kept for the slice lookup of its wetness
                        let road = if surface == Some(1) && !mm.mesh.vertices.is_empty() {
                            let (lo, hi) = nfs_math::aabb_of(mm.mesh.vertices.iter().map(|v| Vec3::from(v.position))).unwrap_or(nfs_math::EMPTY_AABB);
                            road_centres.insert(mesh, art.transform.transform_point3((lo + hi) * 0.5));
                            nfs_renderer::CSM_ROAD
                        } else {
                            0
                        };
                        DrawItem { csm: nfs_renderer::CSM_RECEIVE | road | if surface.is_some_and(|f| f != 0) { 0 } else { nfs_renderer::CSM_CAST }, ..DrawItem::new(mesh, material_for(r, mm.material), art.transform) }
                    }).collect())
                })
                .collect();
            let parts = art.lods.iter().map(|(_, meshes)| meshes.iter().map(|mm| (mm.flags, mm.key_hi, art.transform.transform_point3(Vec3::from(mm.centre)))).collect()).collect();
            track_lods.push(TrackArticleGpu { article: art.index, lods, parts });
        }
        (track_lods, road_centres)
    }

    /// Each track smackable's article: an instance item of its own (the walk answers it through
    /// 0x4786A0 with the body's pose, 0x450010 picks the LOD from its own records), every LOD
    /// uploaded, drawn at `pose · home⁻¹ · tr` ([`Self::place_smackable`]).
    #[allow(clippy::type_complexity)]
    fn upload_track_smackables(
        r: &mut Renderer,
        track: &TrackAssets,
        geometry: &nfs_assets::track::TrackGeometry,
        track_smackables: &[SmackableSpawn],
        lists: &mut ItemLists,
        material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId,
    ) -> (HashMap<SmackableOwner, (Mat4, Vec<(usize, Mat4)>)>, Vec<SmackAnim>) {
        let ItemLists { items, instance_culls, item_cull } = lists;
        let mut moved: HashMap<SmackableOwner, (Mat4, Vec<(usize, Mat4)>)> = HashMap::new();
        let mut smack_anims: Vec<SmackAnim> = Vec::new();
        let arts = track.crp.articles(&track.crp_data).unwrap_or_default();
        for sp in track_smackables {
            let SmackableOwner::Article(k) = sp.owner else { continue };
            let Some(own) = geometry.world.iter().find(|a| a.index == k) else { continue };
            let slot = arts.get(k).and_then(|a| article_base(track, a)).map(|(centre, radius, lods)| {
                instance_culls.push(InstanceCull { centre, radius, lods, first_item: items.len(), broken: None, article: Some(k) });
                instance_culls.len() - 1
            });
            let first = items.len();
            let drawn = if slot.is_some() { &own.lods[..] } else { &own.lods[..own.lods.len().min(1)] };
            for (lod, meshes) in drawn {
                for mm in meshes {
                    items.push(DrawItem { csm: nfs_renderer::CSM_RECEIVE | nfs_renderer::CSM_CAST, ..DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), own.transform) });
                    item_cull.push(slot.map(|s| (s, *lod)));
                }
            }
            moved.insert(sp.owner, (sp.pose().inverse(), (first..items.len()).map(|i| (i, own.transform)).collect()));
            smack_anims.push(SmackAnim { article: k, slot, home: sp.position, position: sp.position, state: 0, armed: false });
        }
        (moved, smack_anims)
    }

    /// The headlight flags of every item: `headlit` where the material's render method builds
    /// `Headlights::LayerVertexLight`, `uv_lit` where it builds `Headlights::LayerUVLight`.
    fn mark_lit_items(
        track: &TrackAssets,
        materials: &[nfs_formats::crp::Material],
        mat_ids: &HashMap<u16, MaterialId>,
        items: &mut [DrawItem],
        track_lods: &mut [TrackArticleGpu],
    ) {
        // the items whose render method builds `Headlights::LayerVertexLight` (0x8002 / 0x8003)
        let lit_methods = nfs_assets::track::vertex_light_methods(&track.crp, &track.crp_data);
        let lit: std::collections::HashSet<MaterialId> = mat_ids
            .iter()
            .filter(|(id, _)| materials.get(**id as usize).is_some_and(|m| lit_methods.contains(&m.render_method)))
            .map(|(_, mid)| *mid)
            .collect();
        // the items whose render method builds `Headlights::LayerUVLight` (0x8004) — the `HEAD`
        // decal of `lighting.md` §5.2, two layers in every night track's `Opaque` method
        let uv_methods = nfs_assets::track::uv_light_methods(&track.crp, &track.crp_data);
        let uv: std::collections::HashSet<MaterialId> = mat_ids
            .iter()
            .filter(|(id, _)| materials.get(**id as usize).is_some_and(|m| uv_methods.contains(&m.render_method)))
            .map(|(_, mid)| *mid)
            .collect();
        for it in all_items_mut(items, track_lods) {
            it.headlit = lit.contains(&it.material);
            it.uv_lit = uv.contains(&it.material);
        }
    }

    /// Every `WAnimBundle` article moves with its own player: its draw items start at
    /// frame 0's matrix, the same the registration used.
    fn anim_articles(track: &TrackAssets, cull_articles: &[Option<CullArticle>], track_lods: &mut [TrackArticleGpu]) -> Vec<AnimArticle> {
        let mut anim_articles: Vec<AnimArticle> = Vec::new();
        let arts = track.crp.articles(&track.crp_data).unwrap_or_default();
        for (i, c) in cull_articles.iter().enumerate() {
            let Some(c) = c.as_ref().filter(|c| c.drawer == ArticleDrawer::Anim) else { continue };
            let Some(track_anim) = arts.get(i).and_then(|a| track.crp.article_animation(&track.crp_data, a)) else { continue };
            let Some((centre, radius, _)) = arts.get(i).and_then(|a| article_base(track, a)) else { continue };
            let matrix = anim_matrix(&track_anim, 0.0, WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME, WANIM_BUNDLE_DEFAULT_LOOPING);
            let gpu = track_lods.iter().position(|g| g.article == i);
            if let Some(g) = gpu.and_then(|g| track_lods.get_mut(g)) {
                for (_, its) in g.lods.iter_mut() {
                    for it in its.iter_mut() {
                        it.transform = matrix;
                    }
                }
            }
            anim_articles.push(AnimArticle { article: i, track: track_anim, centre, radius, gpu, matrix, registered: c.centre });
        }
        anim_articles
    }

    /// The ambient sound of the one `WAnimBundle` article whose tag carries a `SoundDef` (see
    /// [`Self::upload`]'s `sound_articles`), at the bundle's default rate and loop.
    fn animated_sound(track: &TrackAssets, geometry: &nfs_assets::track::TrackGeometry, sound_articles: &std::collections::HashSet<usize>) -> Option<AnimatedSound> {
        let mut animated_sound: Option<AnimatedSound> = None;
        for art in geometry.world.iter().filter(|a| sound_articles.contains(&a.index)) {
            let Some(def) = track.animdefs.sound_by_fourcc(art.tag) else { continue };
            let Some(anim) = art.animation.clone() else { continue };
            if animated_sound.is_none() {
                animated_sound = Some(AnimatedSound {
                    def: *def,
                    label: art.name.clone(),
                    track: anim,
                    seconds_per_frame: WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME,
                    looping: WANIM_BUNDLE_DEFAULT_LOOPING,
                    ambience_index: None,
                });
            }
        }
        animated_sound
    }

    /// The library articles' meshes, every LOD, uploaded once per tag.
    fn upload_library(r: &mut Renderer, geometry: &nfs_assets::track::TrackGeometry, material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId) -> HashMap<u32, Vec<LodMeshes>> {
        // every LOD of each library article, by tag (the first article with it, as 0x478640 finds
        // the template); every shipped one has a single LOD
        let mut lib_gpu: HashMap<u32, Vec<LodMeshes>> = HashMap::new();
        for art in &geometry.library {
            if lib_gpu.contains_key(&art.tag) {
                continue;
            }
            let lods = art.lods.iter().map(|(lod, meshes)| (*lod, meshes.iter().map(|mm| (r.upload_mesh(&mm.mesh), material_for(r, mm.material))).collect())).collect();
            lib_gpu.insert(art.tag, lods);
        }
        lib_gpu
    }

    /// The `WSceneBundle`'s items (0x482EA0): each scene element (and the parked `DFLT` slots) drawn
    /// as its library article at the element's matrix, appended to `lists` with `item_element`
    /// kept parallel to `lists.items`.
    fn upload_scene_items(
        track: &TrackAssets,
        geometry: &nfs_assets::track::TrackGeometry,
        lib_gpu: &HashMap<u32, Vec<LodMeshes>>,
        arts: &[nfs_formats::crp::Article],
        lists: &mut ItemLists,
        item_element: &mut Vec<Option<usize>>,
    ) -> SceneItems {
        let ItemLists { items, instance_culls, item_cull } = lists;
        let mut animated: Vec<AnimatedProp> = Vec::new();
        let mut missing_tags = std::collections::HashSet::new();
        let mut smackables: Vec<SmackableSpawn> = Vec::new();
        let mut instance_flares: Vec<(usize, TrackFlare)> = Vec::new();
        // the `WSceneBundle`'s items for the fitted tree's population: (article, world matrix)
        let mut scene_items: Vec<(usize, Mat4)> = Vec::new();
        // A scene with no GEOM element at all keeps the bundle's 64 parked slots live
        // (`[bundle+0x44] = (0x46D9C0() == 0)`, 0x487A51…0x487A5E; 0x482970 then leaves the count
        // `[+0x18]` at 64, 0x482C76, instead of `min(0x46D9C0(), 64)`, 0x482C81…0x482C97): every
        // slot is a `DFLT` item at (0, 9999.0, 0) with the identity matrix (0x482D3A…0x482D87:
        // `0x461C3C00`, 0x5BB2D0), registered and counted in the tree's box and vertex population
        // like a real element. **Reachable**: the five Monaco tracks and the skidpad ship no
        // `_fstart` / `_bstart` scene (and 0x46D330 loads none for a skill test's scene 0 on a
        // circuit), so every race there has no GEOM element (the earlier "every shipped
        // start file has 2–5" counted the thirteen files that exist). Local to the render walk —
        // `geometry.instances` (the ground triangles, `scene_props`' collision props) is untouched.
        let parked = parked_scene_slots(geometry.instances.len());
        for inst in geometry.instances.iter().chain(parked.iter()) {
            // `0x478640`'s search over the bundle's template records: on no match, `0x482EA0`
            // falls back to template index 0 (`0x482ebe`…`0x482ec0`'s `jge`/`xor eax,eax`), never
            // a skip — the first `'tr'∧library` article in `.crp` order (`geometry.library`'s own
            // build order, `TrackGeometry::build_geometry`'s `library.push`, matching `0x482970`'s
            // gate order). On every shipped track that first entry is `DFLT`
            // (`animdefs.txt`: `collideType=kCylinder`) — read 2026-09-28. Unreachable on shipped
            // data (every scene tag on every shipped track names one of the 70 shared articles,
            // swept below and in [`scene_props`]'s own doc), but no longer an invented skip.
            let tag = if lib_gpu.contains_key(&inst.tag) { Some(inst.tag) } else { geometry.library.first().map(|a| a.tag) };
            if let Some((tag, lods)) = tag.and_then(|t| lib_gpu.get(&t).map(|l| (t, l))) {
                let art = geometry.library.iter().find(|a| a.tag == tag);
                // the scene record's matrix (slot 5, 0x4828A0 → 0x539CD0): the element's position
                // and its 3×3 scaled by the per-cent values (0x482EA0, 0x46B020) — and nothing of the
                // library article's own `tr`: the template's record is the identity (0x482BF1), and
                // the draw 0x450380 pushes this matrix (0x450465…0x450477) and draws the article's
                // `vt` as they are, with no `tr` lookup on the way (`RE/RENDERING.md` "The instance
                // and scene items"). (history: `game-history.md` §9.5)
                let world_tr = inst.transform * Mat4::from_scale(inst.scale);
                if let Some(a) = art {
                    scene_items.push((a.index, world_tr));
                }
                // a looping `animdefs.txt` piece makes this prop play its own track (the matched
                // template's own tag, `tag` above — the fourcc-miss fallback's substitution
                // applies here too, not just to the mesh)
                let loop_piece = track
                    .animdefs
                    .by_fourcc(tag)
                    .and_then(|d| d.resting_pieces().find(|p| p.flags.looping).cloned());
                // the scene bundle's slot (0x482EA0): the walk answers it through 0x4786A0 with the
                // library article's `Base` and picks the LOD from its records (0x450010).
                // The sphere goes through the same matrix, the record's (0x4786A0).
                let cull = art.and_then(|a| arts.get(a.index)).and_then(|a| article_base(track, a)).map(|(centre, radius, clods)| {
                    instance_culls.push(InstanceCull { centre, radius, lods: clods, first_item: items.len(), broken: None, article: None });
                    instance_culls.len() - 1
                });
                // with no `Base` the lowest LOD alone, drawn always (port fallback)
                let drawn = if cull.is_some() { &lods[..] } else { &lods[..lods.len().min(1)] };
                for (lod, parts) in drawn {
                    for (mesh, material) in parts {
                        items.push(DrawItem { csm: nfs_renderer::CSM_RECEIVE | nfs_renderer::CSM_CAST, ..DrawItem::new(*mesh, *material, world_tr) });
                        item_element.push((inst.source == 1).then_some(inst.element));
                        item_cull.push(cull.map(|k| (k, *lod)));
                        if let (Some(p), Some(t)) = (loop_piece.as_ref(), art.and_then(|a| a.animation.as_ref())) {
                            animated.push(AnimatedProp {
                                item: items.len() - 1,
                                base: world_tr,
                                track: t.clone(),
                                seconds_per_frame: p.seconds_per_frame(),
                                looping: p.flags.looping,
                            });
                        }
                    }
                }
                // its flares, drawn with the item (0x450380's `ef` loop); an instance the walk has
                // no entry for (no `Base`) has none here
                if let (Some(k), Some(a)) = (cull, art.and_then(|a| arts.get(a.index).map(|c| (a.index, c)))) {
                    instance_flares.extend(article_flares(track, a.0, a.1, Mat4::IDENTITY).into_iter().map(|f| (k, f)));
                }
                if let Some(sp) = art.and_then(|a| smackable_spawn(track, a, inst)) {
                    smackables.push(sp);
                }
                // an `ENDW` (or any other `kBox`/`kCylinder`) scene element's own collision prop is
                // built once, by [`scene_props`] into the tree (`WSceneBundle`'s own contribution,
                // real source, not the wall-set edges this used to invent here before 2026-09-28 —
                // `RE/RENDERING.md` "The tree, wired"'s skill-test paragraph).
            } else if inst.source == 1 {
                // unreachable on shipped data: `geometry.library` is never empty (every track
                // carries the 70 shared articles, so `tag` above is always `Some`)
                missing_tags.insert(inst.tag);
            }
        }
        if !instance_flares.is_empty() {
            tracing::info!(flares = instance_flares.len(), "scene instance flares");
        }
        if !missing_tags.is_empty() {
            tracing::warn!(tags = ?missing_tags.iter().map(|t| String::from_utf8_lossy(&t.to_le_bytes()).into_owned()).collect::<Vec<_>>(), "mission props without a library article");
        }
        SceneItems { animated, smackables, instance_flares, scene_items }
    }

    /// Every `.env` texture uploaded, keyed by its SHPI name as the `.map` sections spell it, and
    /// the sky map (`SKY`, else the first).
    fn upload_env_textures(r: &mut Renderer, track: &TrackAssets) -> (HashMap<u32, nfs_renderer::TextureId>, Option<nfs_renderer::TextureId>) {
        // every `.env` entry, keyed by its SHPI name as the `.map` sections spell it
        let mut env_by_id = HashMap::new();
        // key 2 (the track's `.env` / `.map`, 0x440A4F): the 16-bit cache of 0x44D7E0 keeps its 8888 entries as 4444
        // (`Y2T1`, `Y2T2`, `YT1`, `YT2` of coastal, read live: type 6 -> 7, thrash.md §6d.9) and a 565 one as 1555
        let env_mips: Vec<Vec<Vec<u8>>> = track.env_textures.iter().map(|t| t.cache_mips()).collect();
        let env_items: Vec<(u32, u32, &[Vec<u8>])> = track.env_textures.iter().zip(&env_mips).map(|(t, m)| (t.width, t.height, m.as_slice())).collect();
        let env_ids = r.upload_textures(&env_items);
        for (t, id) in track.env_textures.iter().zip(env_ids) {
            let mut name = [b' '; 4];
            for (i, c) in t.name.bytes().take(4).enumerate() {
                name[i] = c;
            }
            env_by_id.insert(u32::from_le_bytes(name), id);
        }
        let env_map = track
            .env_textures
            .iter()
            .position(|t| t.name.trim() == "SKY")
            .or(track.env_textures.first().map(|_| 0))
            .and_then(|i| {
                let mut name = [b' '; 4];
                for (k, c) in track.env_textures[i].name.bytes().take(4).enumerate() {
                    name[k] = c;
                }
                env_by_id.get(&u32::from_le_bytes(name)).copied()
            });
        (env_by_id, env_map)
    }

    /// ENHANCEMENT E44: the lighthouse beams of the track in world space for this frame (the animated article's own matrix: the
    /// rotation is the original's). Empty on a track without one.
    pub fn lbeams(&self) -> Vec<nfs_renderer::LBeam> {
        let mut out = Vec::new();
        self.lbeams_into(&mut out);
        out
    }

    /// [`Self::lbeams`] into a caller's vector (cleared first): the per-frame path.
    pub fn lbeams_into(&self, out: &mut Vec<nfs_renderer::LBeam>) {
        out.clear();
        out.extend(self.lbeams.iter().filter_map(|b| {
            let m = self.anim_articles.iter().find(|a| a.article == b.articles[0])?.matrix;
            Some(nfs_renderer::LBeam { apex: m.transform_point3(b.apex), axis: m.transform_vector3(b.axis).normalize_or_zero(), length: b.length, sigma0: b.sigma0, spread: b.spread })
        }));
    }

    /// ENHANCEMENT E44: hide (or show again) the original's translucent sheets of the lighthouse beams: the soft beam replaces them.
    /// An item flagged `shadow_only` is not drawn in the views (the sheets cast no shadow since ENH-BEAM, so they vanish).
    pub fn lbeam_hide(&mut self, hide: bool) {
        if self.lbeam_hidden == hide {
            return;
        }
        self.lbeam_hidden = hide;
        for b in &self.lbeams {
            for &(g, l, k) in &b.items {
                if let Some(it) = self.track_lods.get_mut(g).and_then(|a| a.lods.get_mut(l)).and_then(|(_, v)| v.get_mut(k)) {
                    it.shadow_only = hide;
                }
            }
        }
    }

    /// ENHANCEMENT E32 at the track's load: with the switch on, the samples are read or baked now (the
    /// loading waits) and the copies are uploaded, so the first frame is lit. Off: nothing.
    pub fn gi_prepare(&mut self, r: &mut Renderer, level: u8) {
        if level == 0 {
            return;
        }
        if let Some(gi) = self.gi.as_mut() {
            gi.prepare();
        }
        self.gi_update(r, level);
    }

    /// ENHANCEMENT E32: put the track's parts on the bounce-lit copies of their meshes at `level` (0 = the
    /// original meshes). The first call with the switch on reads the cache or starts the bake; the copies are
    /// uploaded once the samples are there and rewritten when the level changes. Drawing only.
    pub fn gi_update(&mut self, r: &mut Renderer, level: u8) {
        let Self { gi, track_lods, road_centres, .. } = self;
        let Some(gi) = gi.as_mut() else { return };
        let params = crate::gi::params(level);
        if params.is_none() || gi.poll().is_none() {
            if gi.active {
                for t in &gi.targets {
                    track_lods[t.g].lods[t.l].1[t.k].mesh = t.original;
                }
                gi.active = false;
            }
            return;
        }
        let (Some(p), Some(samples)) = (params, gi.samples.as_ref()) else { return };
        if gi.level != level {
            let t0 = std::time::Instant::now();
            let colours: Vec<[f32; 4]> = gi.targets.iter().flat_map(|t| t.part.mesh.vertices.iter().map(|v| v.color)).collect();
            let ctx = crate::gi::context(gi.sky, &colours, samples, gi.sun);
            for t in gi.targets.iter_mut() {
                let mut mesh = t.part.mesh.clone();
                for (j, v) in mesh.vertices.iter_mut().enumerate() {
                    if let Some(s) = t.part.samples.get(j).and_then(|&u| samples.get(u as usize)) {
                        v.color = crate::gi::shade(v.color, s, &p, &ctx);
                    }
                }
                match t.variant {
                    Some(id) => r.update_mesh_vertices(id, &mesh.vertices),
                    None => {
                        let id = r.upload_mesh(&mesh);
                        if let Some(c) = road_centres.get(&t.original).copied() {
                            road_centres.insert(id, c);
                        }
                        t.variant = Some(id);
                    }
                }
            }
            tracing::info!(level, parts = gi.targets.len(), ms = t0.elapsed().as_millis() as u64, "ENHANCEMENT E32: bounce light applied");
            gi.level = level;
        }
        if !gi.active {
            for t in &gi.targets {
                if let Some(v) = t.variant {
                    track_lods[t.g].lods[t.l].1[t.k].mesh = v;
                }
            }
            gi.active = true;
        }
    }

    /// `Graphics.Headlights` below Projected: no item takes the `HEAD` decal (`main.rs`).
    pub fn clear_uv_lit(&mut self) {
        for it in all_items_mut(&mut self.items, &mut self.track_lods) {
            it.uv_lit = false;
        }
    }

    /// Advance every looping prop animation to `seconds` since the race started
    /// (`AnimPlayer::sample` 0x470E90: the frame pair and the blend between them, then the frame's
    /// own rotation, offset and scale inside the placement the article was instanced with).
    /// `refresh` is the walk prep's every-eighth-call answer (0x488CC0's `[0x628C4C] & 7 == 0`,
    /// the caller's process-wide counter): the moving refs are re-registered
    /// ([`Self::reregister_articles`]).
    ///
    /// PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the original passes the race tick's own time; the
    /// port's caller passes a fractional tick between the previous tick and this one for a
    /// display faster than 64 Hz (at `alpha` 1 the same value, [`Self::animate`]'s `seconds` =
    /// tick / 64). Drawing only: nothing here is read by the simulation.
    pub fn animate(&mut self, seconds: f32, refresh: bool) {
        for a in &self.animated {
            let local = anim_matrix(&a.track, seconds, a.seconds_per_frame, a.looping);
            if let Some(item) = self.items.get_mut(a.item) {
                item.transform = a.base * local;
                item.motion_id = nfs_renderer::motion_item(a.item); // ENHANCEMENT E37 (ENH-MOTIONID)
            }
        }
        self.animate_articles(seconds);
        if refresh {
            self.reregister_articles();
        }
    }

    /// The `WAnimBundle` articles (`COVERAGE-2026-09-26.md` §14.79): each one's player
    /// sampled at `seconds` since the race start (slot 5 0x4777B0 → the player's slot 1 0x4715A0
    /// → 0x470E90; the default rate and loop [`WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME`]), the
    /// matrix put on every draw item of the article, its sphere `Base`'s through that matrix
    /// (0x477DB0 with `this[1]` set) — the sphere the walk answers this frame.
    fn animate_articles(&mut self, seconds: f32) {
        if self.anim_articles.is_empty() {
            return;
        }
        for k in 0..self.anim_articles.len() {
            let a = &self.anim_articles[k];
            let m = anim_matrix(&a.track, seconds, WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME, WANIM_BUNDLE_DEFAULT_LOOPING);
            let (centre, radius) = (m.transform_point3(a.centre), m.transform_vector3(Vec3::splat(a.radius)).max_element());
            let (article, gpu) = (a.article, a.gpu);
            if let (Some(gi), Some(g)) = (gpu, gpu.and_then(|g| self.track_lods.get_mut(g))) {
                for (l, (_, items)) in g.lods.iter_mut().enumerate() {
                    for (i, it) in items.iter_mut().enumerate() {
                        it.transform = m;
                        it.motion_id = nfs_renderer::motion_article(gi, l, i); // ENHANCEMENT E37 (ENH-MOTIONID)
                    }
                }
            }
            if let Some(Some(c)) = self.cull_articles.get_mut(article) {
                c.centre = centre;
                c.radius = radius;
            }
            self.anim_articles[k].matrix = m;
        }
    }

    /// 0x488CC0's every-eighth-call re-registration (0x489370 → 0x489530's `+5` path) of every
    /// `WAnimBundle` article: nothing to do while the sphere's centre is exactly the stored one
    /// (0x4895B4…0x4895E5); else the node 0x47BC20 answers, and only a *different* node moves the
    /// ref — to the end of its list (0x489850 / 0x47C6F0), so after every ref registered so far.
    /// 0x488CC0 is the head of **every** walk 0x47FB40 (0x47FB49) — the main view's and the
    /// mirror's — and counts them in one process-wide `[0x628C4C]` (never reset:), so the
    /// caller (`App`) keeps the counter and calls this on the eighth, whichever pass it is.
    pub fn reregister_articles(&mut self) {
        self.reregister_instances();
        for k in 0..self.anim_articles.len() {
            let article = self.anim_articles[k].article;
            let Some(Some(c)) = self.cull_articles.get(article) else { continue };
            let (centre, radius) = (c.centre, c.radius);
            if centre != self.anim_articles[k].registered {
                self.anim_articles[k].registered = centre;
                let node = self.fitted.node_for_sphere(centre.x, centre.y, centre.z, radius);
                if self.article_nodes.get(article).and_then(|n| n.first()).map(|n| n.id) != Some(node.id) {
                    let (cx, cz, r) = self.fitted.circle(node);
                    self.article_nodes[article] = vec![RegisteredNode { id: node.id, cx, cz, r }];
                    self.article_seq[article] = self.next_seq;
                    self.next_seq += 1;
                }
            }
        }
    }

    /// `RCustomLayers::LayerUVAnimate` (0x448CC0, `RE/drafts/round1/render-core.md` §4.4):
    /// `ticks128` is `TIMER_getTicks()`, the OS multimedia timer's 128 Hz counter — the port's own
    /// 64 Hz race tick doubled, the same reading the police beacon's rotation already uses
    /// (`App::rebuild_scene`, "The 128 Hz TIMER is two of this port's 64 Hz game ticks"). Mode 0
    /// (water) scrolls V by `t − ⌊t⌋`, `t = ticks · rate · 1e-4`; mode 2 (flags/banners) wobbles U
    /// by `sin(th · τ) · 0.06`, `th = ticks · rate · 2e-4` turns. Neither mode's bit 0x200 interlock
    /// (`+0x117E4`, "someone already owns the UVs") applies here — nothing else in the port writes
    /// a track item's UVs. PORT-DEVIATION D33: the original phase-offsets mode 2 by the layer's own UV
    /// source pointer's low 12 bits (0x448E12) so identical articles do not wobble in step; the
    /// port has no such pointer and uses phase 0 for every one — every flag on one material wobbles
    /// in lockstep.
    pub fn animate_uv(&mut self, ticks128: u32) {
        if self.uv_animate.is_empty() {
            return;
        }
        // the offsets are a function of the reading alone and nothing else writes them: the same
        // reading again (several frames within one game tick) rewrites what is already there
        if self.uv_plan.applied == Some(ticks128) {
            return;
        }
        // the items the animation touches, found once (the lists are not changed after the build):
        // the walk over every item of every LOD of the track and a map lookup each was the cost
        let shape = (self.items.len(), self.track_lods.len());
        if self.uv_plan.shape != Some(shape) {
            let mut targets = Vec::new();
            for (k, it) in self.items.iter().enumerate() {
                if let Some(a) = self.uv_animate.get(&it.material) {
                    targets.push((UvTarget::Item(k), a.mode, a.rate));
                }
            }
            for (g, a) in self.track_lods.iter().enumerate() {
                for (l, (_, v)) in a.lods.iter().enumerate() {
                    for (k, it) in v.iter().enumerate() {
                        if let Some(a) = self.uv_animate.get(&it.material) {
                            targets.push((UvTarget::Lod(g, l, k), a.mode, a.rate));
                        }
                    }
                }
            }
            self.uv_plan = UvPlan { shape: Some(shape), targets, applied: None };
        }
        let ticks = ticks128 as f32;
        for &(target, mode, rate) in &self.uv_plan.targets {
            let it = match target {
                UvTarget::Item(k) => &mut self.items[k],
                UvTarget::Lod(g, l, k) => &mut self.track_lods[g].lods[l].1[k],
            };
            it.uv_offset = match mode {
                0 => {
                    let t = ticks * rate as f32 * 1e-4;
                    [0.0, t - t.floor()]
                }
                2 => {
                    let th = ticks * rate as f32 * 2e-4;
                    // det-allow: UV scroll, drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
                    [(th * std::f32::consts::TAU).sin() * 0.06, 0.0]
                }
                // not read (render-core.md §4.4): no shipped material uses another mode
                _ => [0.0, 0.0],
            };
        }
        self.uv_plan.applied = Some(ticks128);
    }

    /// ENHANCEMENT E9 (SHADOWFIX): the sun shadows' casters the view walk left out — scenery within `reach` metres of
    /// `centre` that the walk did not draw this frame (outside the frustum, behind an occluder, past its LOD range) —
    /// as `shadow_only` items at their coarsest LOD, so a tree or a hillside just past the screen edge keeps its shadow
    /// instead of popping it in. Pushed after the frame's real items; the renderer draws them only into the shadow map.
    /// Not the original's (the original has no shadows); drawing only.
    pub fn shadow_casters(&self, draws: &[Option<ArticleDraw>], centre: Vec3, reach: f32, out: &mut Vec<DrawItem>) {
        let near = |c: Vec3, r: f32| (c - centre).length() - r < reach;
        for a in &self.track_lods {
            let Some(Some(c)) = self.cull_articles.get(a.article).map(|c| c.as_ref()) else { continue };
            if draws.get(a.article).copied().flatten().is_some() || !near(c.centre, c.radius) {
                continue;
            }
            let Some(lod) = c.lods.last().map(|l| l.lod) else { continue };
            let Some((_, parts)) = a.lods.iter().find(|(n, _)| *n == lod) else { continue };
            out.extend(parts.iter().filter(|i| i.csm & nfs_renderer::CSM_CAST != 0).map(|i| DrawItem { shadow_only: true, ..*i }));
        }
        let n = self.cull_articles.len();
        for (k, item) in self.items.iter().enumerate() {
            if item.csm & nfs_renderer::CSM_CAST == 0 {
                continue;
            }
            let Some((slot, lod)) = self.item_cull.get(k).copied().flatten() else { continue };
            let Some(ic) = self.instance_culls.get(slot) else { continue };
            if ic.broken.is_some() || draws.get(n + slot).copied().flatten().is_some() || !near(ic.centre, ic.radius) {
                continue;
            }
            if ic.lods.last().is_some_and(|l| l.lod == lod) {
                out.push(DrawItem { shadow_only: true, ..*item });
            }
        }
    }

    /// Draw items with the hidden mission elements filtered out.
    ///
    /// Every one is lit by its vertex colour alone ([`TRACK_LIGHT`]): no track render method
    /// carries an `R3DLightLayer` (0x8104, `RE/notes/lighting.md` §6), the `.map` header reaches
    /// only the car's light block and environment maps (§2), and the class-0 draw copies the `df`
    /// stream into the vertex colour untouched (0x540E1D) — or an `Am` op has written its flat
    /// colour there (§5.4) — for stage 0 to `MODULATE` with the texture (`thrash.md` §5).
    ///
    /// **The order** is the view's lists' (`RE/RENDERING.md` "The view's lists"): 0x450DE0 queues
    /// each `pr` part keyed by `round(d²)` (0x450EDB…0x450FCF) — flag 2 in list `+4`, nearest first
    /// and marked `early`; flag 1 / 4 in `+0xC`, farthest first ([`part_list`]); the instance
    /// drawer's whole item in `+0xC` (0x44FFC0 → 0x466A90). Equal keys keep the queueing order
    /// ([`ArticleDraw::order`], 0x466933 / 0x466AD3, unsigned compare); an item with no `Base` (port
    /// fallback) goes last. The full trace (the walk's
    /// pre-order 0x47F400, the re-registration of moved refs 0x489530):
    /// `RE/notes/game-history.md` §33.2.
    pub fn visible_items(&self, draws: &[Option<ArticleDraw>], eye: Vec3) -> Vec<DrawItem> {
        let TrackLists { mut near, mut far, rest, .. } = self.track_draw_lists(draws, eye);
        near.sort_by_key(|(k, _)| *k);
        far.sort_by_key(|(k, _)| std::cmp::Reverse(*k));
        near.into_iter()
            .map(|(_, i)| DrawItem { early: true, ..i })
            .chain(far.into_iter().map(|(_, i)| i))
            .chain(rest)
            .map(|i| DrawItem { layer_light: Some(TRACK_LIGHT), ..i })
            .collect()
    }

    /// The `+0xC` list's track-side entries, unsorted and un-flattened: the static / mid-ground
    /// drawer's flag-1 / flag-4 parts, the instance drawer's whole items and a breakable's pieces
    /// ([`Self::visible_items`]'s "the view's lists" doc), each still carrying its `round(d²)` /
    /// `round(v)` key. Split out so the race pass ([`App::rebuild_scene`]) can merge these with the
    /// cars' and the skid marks' own `+0xC` entries into **one** farthest-first list before the
    /// flush (`RE/RENDERING.md` "The view's lists"; the original's `0x466660`/`0x467293`) — the
    /// gap `RE/notes/thrash.md` §6d.6 left open: those three kinds shared one distance-sorted list
    /// in the original but drew in separate blocks here until now. `near` (list `+4`, unsorted) and
    /// `rest` (the port's no-`Base` fallback) are returned too, for [`Self::visible_items`]'s own
    /// use and for the mirror's independent walk ([`App::render_mirror`]).
    pub fn track_draw_lists(&self, draws: &[Option<ArticleDraw>], eye: Vec3) -> TrackLists {
        let mut lists = TrackLists::default();
        self.track_draw_lists_into(draws, eye, &mut lists);
        lists
    }

    /// [`Self::track_draw_lists`] into reusable buffers (cleared first): the per-frame callers keep one
    /// [`TrackLists`] so the lists do not allocate again every frame.
    pub fn track_draw_lists_into(&self, draws: &[Option<ArticleDraw>], eye: Vec3, lists: &mut TrackLists) {
        // 0x450EF8…0x450F25: `fistp` of the square distance from the eye to the part's centre
        // (round to nearest even; out of the i32 range the x87 stores the integer indefinite
        // 0x80000000)
        let key = |c: Vec3| {
            let d2 = (c - eye).length_squared().round_ties_even();
            if d2 < 2_147_483_648.0 { d2 as i32 as u32 } else { 0x8000_0000 }
        };
        let TrackLists { near, far, rest, units } = lists;
        near.clear();
        far.clear();
        rest.clear();
        units.clear();
        // the walk's draw of the track geometry: each drawn article at the LOD its drawer picked
        // (0x450DE0 / 0x450010 — the record's `pr` range is the parts of that LOD number); an
        // article the walk has no record of (no `Base`) is drawn at its lowest LOD (port fallback)
        // equal keys draw in the order the node pass queued them: articles by the first visible node
        // they are registered into (tree order), then by their place in the node's ref list
        // ([`ArticleDraw::order`]); the sorts that follow keep queueing order among equal keys
        // (only the articles the loop below can act on are sorted: one the walk did not draw
        // `continue`s there, and a stable sort of the subset keeps the relative order the full
        // stable sort gave it — the sort over every article of the track was most of this call)
        units.extend(self.track_lods.iter().enumerate().filter_map(|(g, a)| {
            let order = draws.get(a.article).copied().flatten().map(|d| d.order);
            let culled = matches!(self.cull_articles.get(a.article).map(|c| c.as_ref()), Some(Some(_)));
            (!culled || order.is_some()).then_some((order, QueueUnit::Lod(g)))
        }));
        // the instance drawer's own items (the `.scn` GEOM instances) and the breakables are refs of the same
        // tree (0x489530's `+5` path, one node each — [`Self::register_instances`] / [`Self::reregister_instances`]),
        // so the node pass queues them at their place among the articles: by the node and the registration
        // number of the ref ([`ArticleDraw::order`]), not after every article (WP-R)
        let n = self.cull_articles.len();
        for (k, (_, e)) in self.items.iter().zip(self.item_element.iter()).enumerate() {
            if e.is_some_and(|e| self.hidden.contains(&e)) || self.broken.iter().any(|b| b.hides(k)) {
                continue;
            }
            match self.item_cull.get(k).copied().flatten() {
                Some((slot, _)) => {
                    if let Some(d) = draws.get(n + slot).copied().flatten() {
                        units.push((Some(d.order), QueueUnit::Item(k)));
                    }
                }
                None => units.push((None, QueueUnit::Item(k))),
            }
        }
        for (bi, b) in self.broken.iter().enumerate() {
            match b.slot {
                Some(k) => {
                    if let Some(d) = draws.get(n + k).copied().flatten() {
                        units.push((Some(d.order), QueueUnit::Broken(bi)));
                    }
                }
                None => units.push((None, QueueUnit::Broken(bi))),
            }
        }
        units.sort_by_key(|(o, _)| *o);
        for &(_, u) in units.iter() {
            match u {
                QueueUnit::Lod(g) => {
                    let a = &self.track_lods[g];
                    match self.cull_articles.get(a.article).map(|c| c.as_ref()) {
                        Some(Some(c)) => {
                            let Some(d) = draws.get(a.article).copied().flatten() else { continue };
                            let Some(lod) = c.lods.get(d.lod).map(|l| l.lod) else { continue };
                            let Some(k) = a.lods.iter().position(|(n, _)| *n == lod) else { continue };
                            let parts = a.parts.get(k).map(|p| p.as_slice()).unwrap_or(&[]);
                            for (i, item) in a.lods[k].1.iter().enumerate() {
                                let (flags, key_hi, centre) = parts.get(i).copied().unwrap_or(DEFAULT_PART);
                                // `FakeSortMode::PerItem`'s fold (0x449AB2, `[0x61975C] < 0x2710`
                                // unsigned): the drawer writes the queued entry's own `+0xC` — this same
                                // view-list key — to the global just before each draw (0x450CE2, `edx =
                                // [item+0xC]`) — see `DrawItem::fakesort_near`'s doc
                                match c.drawer {
                                    ArticleDrawer::Static | ArticleDrawer::MidGround { .. } => match part_list(flags) {
                                        Some(ViewList::Near) => {
                                            // list `+4` only: the part's word `+6` is the key's high half,
                                            // `([pr+4] & 0xFFFF0000) + round(d²)` (0x450F37…0x450F44, 32-bit
                                            // wrap) — 0x8000 on most parts, so a near entry's key is never
                                            // under 0x2710 for the fold; 0x8001 / 0x8002 (layered foliage)
                                            // draw after every 0x8000 part. Read live 2026-10-03 (gdb at
                                            // 0x450F4C / 0x46668D, autobahn: the flush walks the keys
                                            // ascending, the 0x8001 / 0x8002 parts after the rest; `thrash.md` §6e)
                                            let k = ((key_hi as u32) << 16).wrapping_add(key(centre));
                                            near.push((k, DrawItem { fakesort_near: k < FAKESORT_NEAR_KEY, ..*item }));
                                        }
                                        Some(ViewList::Far) => {
                                            let k = key(centre);
                                            far.push((k, DrawItem { fakesort_near: k < FAKESORT_NEAR_KEY, ..*item }));
                                        }
                                        None => {}
                                    },
                                    ArticleDrawer::Instance | ArticleDrawer::Anim => {
                                        let k = d.value as u32;
                                        far.push((k, DrawItem { fakesort_near: k < FAKESORT_NEAR_KEY, ..*item }));
                                    }
                                }
                            }
                        }
                        _ => rest.extend(a.lods.first().map(|(_, v)| v.as_slice()).unwrap_or(&[]).iter().copied()),
                    }
                }
                QueueUnit::Item(k) => {
                    let item = &self.items[k];
                    match self.item_cull.get(k).copied().flatten() {
                        Some((slot, lod)) => {
                            let Some(d) = draws.get(n + slot).copied().flatten() else { continue };
                            if self.instance_culls.get(slot).and_then(|ic| ic.lods.get(d.lod)).is_some_and(|l| l.lod == lod) {
                                // 0x450010 writes `[0x61975C]` from the entry's own `+0xC` before it draws
                                // (0x4500BE) — the same key the entry was queued with
                                let k = d.value as u32;
                                far.push((k, DrawItem { fakesort_near: k < FAKESORT_NEAR_KEY, ..*item }));
                            }
                        }
                        None => rest.push(*item),
                    }
                }
                QueueUnit::Broken(bi) => {
                    let b = &self.broken[bi];
                    // a breakable's pieces are the entries 0x450010 draws when the walk answers the item
                    match b.slot {
                        Some(k) => {
                            if let Some(d) = draws.get(n + k).copied().flatten() {
                                let k = d.value as u32;
                                far.extend(b.pieces.iter().map(|i| (k, DrawItem { fakesort_near: k < FAKESORT_NEAR_KEY, ..*i })));
                            }
                        }
                        None => rest.extend(b.pieces.iter().copied()),
                    }
                }
            }
        }
    }

    /// The breakable props of the world (`BBOX`, the crate stacks): each world article whose prop
    /// has a `Breakable` is drawn on its own, and every piece of its definition gets its template
    /// — the `BaseGroup` article's meshes (0x478640 by tag, else the record's entry 0: the prop's own
    /// article for the first piece, the first piece's afterwards — 0x476471) and the track it plays:
    /// the `QuatGroup` article's `Anqt`, or the base article's when the `QuatGroup`'s `Anim` has
    /// vertex frames (its second word, 0x476525…0x476552).
    fn breakables(
        r: &mut Renderer,
        track: &TrackAssets,
        geometry: &nfs_assets::track::TrackGeometry,
        props: &[nfs_physics::props::Prop],
        lists: &mut ItemLists,
        material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId,
    ) -> Vec<BrokenProp> {
        let ItemLists { items, instance_culls: culls, item_cull } = lists;
        let Ok(arts) = track.crp.articles(&track.crp_data) else { return Vec::new() };
        let tags = article_tags(track, &arts);
        let by_index: HashMap<usize, &nfs_assets::track::ArticleMeshes> = geometry.world.iter().chain(geometry.library.iter()).map(|a| (a.index, a)).collect();
        let mut uploaded: HashMap<usize, Vec<(MeshId, MaterialId)>> = HashMap::new();
        let mut upload = |r: &mut Renderer, material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId, k: usize| -> Vec<(MeshId, MaterialId)> {
            uploaded
                .entry(k)
                .or_insert_with(|| by_index.get(&k).map(|a| a.meshes().iter().map(|mm| (r.upload_mesh(&mm.mesh), material_for(r, mm.material))).collect()).unwrap_or_default())
                .clone()
        };
        let mut out = Vec::new();
        for (pi, p) in props.iter().enumerate() {
            let Some(b) = p.breakable.as_ref() else { continue };
            let Some(own) = geometry.world.iter().find(|a| a.index == b.article) else { continue };
            let Some(def) = track.animdefs.lookup(own.tag) else { continue };
            // an item of `WInstanceBundle` (its record `world+0x40 + 56·id`, 0x476A54): the walk
            // answers it through 0x4786A0 and 0x450010 picks the LOD from its own article's records
            // (the cursor 0x477C30), so every LOD of the article is uploaded
            let slot = arts.get(own.index).and_then(|a| article_base(track, a)).map(|(centre, radius, lods)| {
                culls.push(InstanceCull { centre, radius, lods, first_item: items.len(), broken: Some(out.len()), article: Some(own.index) });
                culls.len() - 1
            });
            let first = items.len();
            let drawn = if slot.is_some() { &own.lods[..] } else { &own.lods[..own.lods.len().min(1)] };
            for (lod, meshes) in drawn {
                for mm in meshes {
                    items.push(DrawItem { csm: nfs_renderer::CSM_RECEIVE | nfs_renderer::CSM_CAST, ..DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), own.transform) });
                    item_cull.push(slot.map(|k| (k, *lod)));
                }
            }
            let mut entry0 = own.index;
            let mut templates = Vec::new();
            for (k, (_, piece)) in def.pieces.iter().enumerate() {
                let base = first_by_tag(&tags, group_tag(&piece.base_group)).unwrap_or(entry0);
                if k == 0 {
                    entry0 = base;
                }
                let quat = first_by_tag(&tags, group_tag(&piece.quat_group));
                let vertex_frames = quat.and_then(|q| tags[q].1).is_some_and(|w| w[1] > 0);
                let source = if vertex_frames { Some(base) } else { quat };
                let track_of = source.and_then(|k| by_index.get(&k)).and_then(|a| a.animation.clone());
                templates.push(PieceTemplate {
                    meshes: upload(r, &mut *material_for, base),
                    base: arts.get(base).and_then(|a| article_base(track, a)).map(|(c, r, _)| (c, r)),
                    track: track_of,
                    seconds_per_frame: piece.seconds_per_frame(),
                    flags: piece.flags,
                });
            }
            out.push(BrokenProp {
                prop: pi,
                items: first..items.len(),
                instance: own.transform,
                velocity_factor: def.velocity_factor,
                templates,
                players: Vec::new(),
                pieces: Vec::new(),
                lead: None,
                state: BreakState::Whole,
                slot,
            });
        }
        out
    }

    /// The breakables' side of the anim world on game tick `tick` (64 Hz): a prop a car has just
    /// started (`Breakable::started`) builds a player for each piece of its speed group (0x476380 —
    /// up to the first that does not resolve) and stops drawing its own article (the record's
    /// entry 0 now holds the first piece); each frame every player is sampled (0x4769E0: the
    /// piece's matrix, then the instance's); once the lead player — the first piece's twin at
    /// `rec+0x1C` — has run out (0x4715A0 answers 2 when the draw samples it through 0x450010's
    /// `rec+8` branch), the next update 0x476810 tears the pieces down (0x476710) and nothing of
    /// the prop is drawn again; its answer stays 0, so cars pass.
    pub fn break_props(&mut self, tick: u32) {
        for (bi, b) in self.broken.iter_mut().enumerate() {
            if let BreakState::Whole = b.state
                && let Some((start, velocity)) = self.props.get(b.prop).and_then(|p| p.breakable.as_ref()).and_then(|x| x.started.get())
            {
                let Some(br) = self.props[b.prop].breakable.as_ref() else { continue };
                let group = nfs_formats::animdefs::band_group(&br.bands, &br.band_pieces, velocity.length());
                // 0x476588…0x4765E0 / 0x476618…0x476641: the velocity in the instance's frame
                // (0x532A00 transposes its rotation, 0x532A40 applies it), its heading
                // `atan2(x, z)` in turns (0x532860) and the piece's drift `v·velocityFactor`
                let local = b.instance.inverse().transform_vector3(velocity);
                // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable atan2, not std; original 0x532860 (atan2 → turns, x87).
                let heading = nfs_math::portable::atan2f(local.x, local.z);
                for i in group {
                    if !br.resolved.get(i).copied().unwrap_or(false) {
                        break;
                    }
                    b.players.push(PiecePlayer { template: i, heading, drift: local * b.velocity_factor });
                }
                b.state = BreakState::Playing { start, lead_done: false };
            }
            if let BreakState::Playing { start, lead_done } = b.state {
                if lead_done {
                    b.state = BreakState::Gone;
                    b.players.clear();
                    b.pieces.clear();
                    b.lead = None;
                    continue;
                }
                let ticks = tick.saturating_sub(start);
                b.pieces.clear();
                let mut lead = true;
                for (k, pl) in b.players.iter().enumerate() {
                    let t = &b.templates[pl.template];
                    let Some(track) = t.track.as_ref() else { continue };
                    let (m, playing) = sample_piece(track, t.seconds_per_frame, t.flags, pl.heading, pl.drift, ticks);
                    if k == 0 {
                        lead = playing;
                        b.lead = Some(m);
                    }
                    for (mesh, material) in &t.meshes {
                        // ENHANCEMENT E37 (ENH-MOTIONID): the piece's own identity: this breakable and its place in the list
                        let motion_id = nfs_renderer::motion_piece(bi, b.pieces.len());
                        b.pieces.push(DrawItem { csm: nfs_renderer::CSM_RECEIVE | nfs_renderer::CSM_CAST, motion_id, ..DrawItem::new(*mesh, *material, b.instance * m) });
                    }
                }
                b.state = BreakState::Playing { start, lead_done: !lead };
            }
        }
    }

    /// Show exactly the mission elements in `shown` among `all` (arrow markers, 0x482F70).
    pub fn set_shown(&mut self, all: &[usize], shown: &[usize]) {
        for e in all {
            self.hidden.insert(*e);
        }
        for e in shown {
            self.hidden.remove(e);
        }
    }

    /// Draw a smackable's article where its body is: `0x483590` (from 0x4A1510) points the prop
    /// record's pose at the car record's own (`car+0x330` / `car+0x364`), so the renderer draws the
    /// article with the body's pose (`skill-tests.md` §5.3) — the player's sample is the pose times
    /// the inverse of the home pose (0x4711E0), then the item's own matrix (0x4769E0). `pose` is
    /// the body's, body to world; the items keep their authored transforms relative to the
    /// authored pose.
    pub fn place_smackable(&mut self, owner: SmackableOwner, pose: Mat4) {
        let Some(spawn) = self.smackables.iter().find(|s| s.owner == owner) else { return };
        let authored = spawn.pose();
        let items = &self.items;
        let item_element = &self.item_element;
        let (inv, list) = self.moved.entry(owner).or_insert_with(|| {
            let list = match owner {
                SmackableOwner::Element(element) => item_element.iter().enumerate().filter(|(_, e)| **e == Some(element)).map(|(i, _)| (i, items[i].transform)).collect(),
                // the article's items are registered at upload
                SmackableOwner::Article(_) => Vec::new(),
            };
            (authored.inverse(), list)
        });
        for (i, base) in list.iter() {
            self.items[*i].transform = pose * *inv * *base;
            self.items[*i].motion_id = nfs_renderer::motion_item(*i); // ENHANCEMENT E37 (ENH-MOTIONID)
        }
        if let SmackableOwner::Article(article) = owner
            && let Some(a) = self.smack_anims.iter_mut().find(|a| a.article == article)
        {
            a.position = pose.w_axis.truncate();
        }
    }
}


#[cfg(test)]
mod parked_slot_tests {
    use super::*;

    /// 0x482970: a scene with a GEOM element keeps `min(count, 64)` live slots and no parked ones;
    /// one without keeps all 64, each a `DFLT` at (0, 9999, 0) (0x482D3A…0x482D87).
    #[test]
    fn a_scene_without_geom_elements_parks_64_dflt_slots() {
        assert!(parked_scene_slots(1).is_empty() && parked_scene_slots(5).is_empty());
        let p = parked_scene_slots(0);
        assert_eq!(p.len(), 64);
        assert!(p.iter().all(|s| s.tag == u32::from_le_bytes(*b"DFLT") && s.transform.w_axis.truncate() == Vec3::new(0.0, 9999.0, 0.0) && s.scale == Vec3::ONE));
    }
}
