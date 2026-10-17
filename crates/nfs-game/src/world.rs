//! Assembling renderer scenes from loaded assets (shared by the game app and the viewer tools).

use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, track::TrackAssets};
use nfs_formats::bytes;
use nfs_renderer::{DrawItem, LayerLight, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};
use std::collections::HashMap;

mod car_model;
pub use car_model::*;
#[cfg(test)]
use car_model::{KeyPlayer, GEAR_GESTURES, HANDBRAKE_OFF, HANDBRAKE_ON, REVERSE_OFF, REVERSE_ON};

/// The light a track item is drawn under: none of its own, so the shader's `min(A + D·d, 1)`
/// leaves the texture × vertex colour product as it is ([`TrackWorld::visible_items`]).
pub const TRACK_LIGHT: LayerLight = LayerLight { ambient: [1.0; 3], diffuse: [0.0; 3], direction: [0.0; 3] };

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

/// The drawer an article reaches the track walk 0x47F890 through. `WTrack::load` builds the
/// bundles Scene, Instance, Anim, MidGround, Static over one article array and the last four each
/// take theirs out of it: `WInstanceBundle` every article with a `tr` (0x47859E), `WAnimBundle` an
/// `Anqt` one, `WMidGroundBundle` `Base & 0x6000`, `WStaticBundle` the rest
/// (`RE/RENDERING.md` "The track draw's culling").
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArticleDrawer {
    /// `WStaticBundle`'s drawer 0x5B3CF0: every `ef` part of the article goes to 0x450D10.
    Static,
    /// `WMidGroundBundle`'s, the same drawer: its sphere's radius `max(2.25·r, 200)` (0x47CF00)
    /// and its band — **600** for `Base & 0x2000` (the ref flag 0x400000), else **700** (0x20)
    /// (0x47CE72…0x47CE8E; 0x47FA08 / 0x47FA3D).
    MidGround { band: f32 },
    /// `WInstanceBundle`'s drawer 0x5B3CB8 — slot 3 is `ret 8`, so only its draw 0x450380 runs
    /// `ef` parts: kind 2, within the chosen LOD's part range (0x450819 → 0x451940).
    Instance,
    /// `WAnimBundle`'s, the same drawer as the instances'.
    Anim,
}

/// One LOD record of `Base+0x64`: its switch distance `+0xA`, its byte `+1` (the `ef` parts run),
/// its part range `+4…+6` and the LOD number `+8` its `pr` parts carry (`crp-geometry.md` §2.1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CullLod {
    pub distance: u16,
    pub effects: bool,
    pub first: u16,
    pub last: u16,
    pub lod: u8,
}

/// A placed article as the walk sees it: its drawer and the sphere the drawer answers (the
/// bundle's slot 3).
#[derive(Debug, Clone, PartialEq)]
pub struct CullArticle {
    /// the article's `Name` (diagnostics)
    pub name: String,
    pub drawer: ArticleDrawer,
    pub centre: Vec3,
    pub radius: f32,
    /// The item's `getBounds` box (`0x4893A0`: slot 4 `0x477F40`'s `Base` `bbox_min`/`bbox_max`
    /// as eight corners, through slot 5's matrix for the bundles whose `+4` is set — the
    /// instances' `tr`): the bounds of its eight corners. It is the static / mid-ground items'
    /// registration footprint (`0x489701`) and every item's contribution to the world box
    /// (`0x47B660`). `MidGround` overrides slot 2 with `0x47D010`: the same box scaled `×2.25`
    /// (`[0x5B4774]`) about its own centre — **not** `Self::radius`'s `max(2.25·r, 200)` draw-time
    /// inflation (`0x47CF00`), a different, per-frame occlusion/distance quantity RENDERING.md
    /// keeps apart from this box.
    pub footprint_min: Vec3,
    pub footprint_max: Vec3,
    pub lods: Vec<CullLod>,
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

/// One `Vis ` segment as 0x487330 keeps it: `raised = a.y ≠ 0`, the start and the delta in xz,
/// `len²` and `1/len²`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Occluder {
    pub raised: bool,
    pub a: [f32; 2],
    pub d: [f32; 2],
    pub len2: f32,
    pub inv_len2: f32,
}

/// The view the walk culls with: the eye and the camera's axes, the tangents of the horizontal and
/// vertical half-angles, the near plane and the draw distance `[0x6197CC]` (the view's `+0x124`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CullView {
    pub eye: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub tan_h: f32,
    pub tan_v: f32,
    pub near: f32,
    pub draw: f32,
}

/// An article the walk draws this frame: the LOD its drawer picks and the value the walk hands the
/// drawer, rounded as the render queue carries it (`fistp`, 0x44FFC9 / 0x45101E) — the effect
/// system's `+0x10` (0x4551BF).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArticleDraw {
    pub lod: usize,
    pub value: f32,
    /// Where the node pass (0x47F400) reaches the article: the allocation number of the first
    /// visible node it is registered into (the walk lists visible nodes in tree order, 0x47F400's
    /// pre-order) and its place in the registration order (a node's refs in list order). The
    /// drawer queues an item at its first ref, so equal keys draw in this order.
    pub order: (u32, u32),
}

/// The 3-D camera's far plane as the frustum test 0x539480 has it (`[+0x1EC]`): taken as the
/// projection's **700** (`RE/notes/thrash.md` §6) — MEDIUM, its writer is not traced.
pub const FRUSTUM_FAR: f32 = 700.0;

/// The image's square root, `(bits >> 1) + 0x1FC00000` (0x47F347, 0x47FA72).
pub(crate) fn bit_root(x: f32) -> f32 {
    f32::from_bits(((x.to_bits() as i32 >> 1) + 0x1FC0_0000) as u32)
}

/// 0x489CC0: whether the xz segments `p0 → p1` and `q0 → q1` cross — both parameters in [0, 1],
/// a zero denominator none.
fn segments_cross(p0: [f32; 2], p1: [f32; 2], q0: [f32; 2], q1: [f32; 2]) -> bool {
    let r = [p1[0] - p0[0], p1[1] - p0[1]];
    let s = [q1[0] - q0[0], q1[1] - q0[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den == 0.0 {
        return false;
    }
    let w = [q0[0] - p0[0], q0[1] - p0[1]];
    let t = (w[0] * s[1] - w[1] * s[0]) / den;
    let u = (w[0] * r[1] - w[1] * r[0]) / den;
    (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)
}

impl CullView {
    /// 0x539480 for a perspective camera: the centre in view space (looking down −z), then the
    /// near / far pair and the two side pairs, each pair taken by the sign of its first form.
    /// The tangents and cosines `[+0xF8…+0x104]` are the port's own frustum's (MEDIUM).
    pub fn sphere_visible(&self, c: Vec3, r: f32) -> bool {
        let rel = c - self.eye;
        let depth = rel.dot(self.forward);
        let a = self.near - depth;
        if a > 0.0 {
            if a > r {
                return false;
            }
        } else if depth - FRUSTUM_FAR > r {
            return false;
        }
        for (x, t) in [(rel.dot(self.right), self.tan_h), (rel.dot(self.up), self.tan_v)] {
            let cos = 1.0 / (1.0 + t * t).sqrt();
            let p = x - depth * t;
            if p > 0.0 {
                if p * cos > r {
                    return false;
                }
            } else {
                let q = x + depth * t;
                if q < 0.0 && -q * cos > r {
                    return false;
                }
            }
        }
        true
    }

    /// 0x47F550's footprint: the eye and the four far corners `(±t·far, ±0.75·t·far, far)`
    /// (0x47F210) dropped on the ground; their bounds' centre and the largest `dx² + dz²` from it
    /// (the hull 0x5A0480 keeps the extremes, so its answer is theirs).
    fn footprint(&self) -> ([f32; 2], f32) {
        let (t, far) = (self.tan_h, self.draw);
        let mut pts = vec![self.eye];
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                pts.push(self.eye + self.forward * far + self.right * (sx * t * far) + self.up * (sy * 0.75 * t * far));
            }
        }
        let (mut lo, mut hi) = ([1e9f32; 2], [-1e9f32; 2]);
        for p in &pts {
            lo = [lo[0].min(p.x), lo[1].min(p.z)];
            hi = [hi[0].max(p.x), hi[1].max(p.z)];
        }
        let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        let r2 = pts.iter().map(|p| (p.x - c[0]).powi(2) + (p.z - c[1]).powi(2)).fold(0.0, f32::max);
        (c, r2)
    }
}

impl Occluder {
    /// 0x4873B0: 1 (true) when the square distance from `c` to the segment passes `r2`.
    fn far_from(&self, c: [f32; 2], r2: f32) -> bool {
        let (px, pz) = (c[0] - self.a[0], c[1] - self.a[1]);
        let t = (px * self.d[0] + pz * self.d[1]) * self.inv_len2;
        let (ex, ez) = (px - t * self.d[0], pz - t * self.d[1]);
        let along = if t >= 1.0 {
            (t - 1.0) * (t - 1.0) * self.len2
        } else if t > 0.0 {
            0.0
        } else {
            t * t * self.len2
        };
        ex * ex + ez * ez + along > r2
    }
}

/// One item of the walk 0x47F890 past the node pass: the sphere through the frustum (0x539480),
/// the `Vis ` occlusion (0x488B50 over the near segments; a mid-ground item only the raised ones),
/// the distance and the drawer's LOD (see [`TrackWorld::article_draws`]).
fn walk_item(v: &CullView, near: &[&Occluder], drawer: ArticleDrawer, centre: Vec3, radius: f32, lods: &[CullLod]) -> Option<ArticleDraw> {
    if !v.sphere_visible(centre, radius) {
        return None;
    }
    let mid = matches!(drawer, ArticleDrawer::MidGround { .. });
    let (eye, c) = ([v.eye.x, v.eye.z], [centre.x, centre.z]);
    if near.iter().any(|o| (!mid || o.raised) && segments_cross([o.a[0], o.a[1]], [o.a[0] + o.d[0], o.a[1] + o.d[1]], eye, c)) {
        return None;
    }
    let d2 = (centre - v.eye).length_squared();
    let value = match drawer {
        ArticleDrawer::MidGround { band } => (d2 < band * band).then_some(d2)?,
        _ => {
            let s = (bit_root(d2) - radius).max(0.0);
            (s * s < v.draw * v.draw).then_some(s * s)?
        }
    };
    let rounded = value.round_ties_even();
    let key = match drawer {
        ArticleDrawer::Static | ArticleDrawer::MidGround { .. } => value.trunc() as f64,
        ArticleDrawer::Instance | ArticleDrawer::Anim => rounded as f64,
    };
    let lod = lods.iter().position(|l| key < (l.distance as f64) * (l.distance as f64))?;
    Some(ArticleDraw { lod, value: rounded, order: (0, 0) })
}

/// One view's walk over the items of `WLineBundle` (the skid marks' cells): see
/// [`TrackWorld::cell_walk`].
pub struct CellWalk<'a> {
    v: CullView,
    near: Vec<&'a Occluder>,
}

impl<'a> CellWalk<'a> {
    /// The walk of view `v` over the `Vis ` segments `occluders` (the near ones, 0x488B00).
    pub fn new(v: &CullView, occluders: &'a [Occluder]) -> Self {
        let (fc, fr2) = v.footprint();
        Self { v: *v, near: occluders.iter().filter(|o| !o.far_from(fc, fr2)).collect() }
    }

    /// 0x47F890 for one cell record: the node pass over its registered nodes (0x47F400 — a cell
    /// registered nowhere is never reached), the sphere `(cx, cy, cz)`, `r` (slot 3 0x479FC0; `cy` the first mark's height, 0x47965D)
    /// through the frustum (0x539480), the `Vis ` occlusion against every near segment (the
    /// flags word of a line bundle's ref is 0: `bl` clear, 0x47F98B) and the distance,
    /// `s² < far²` with `s = max(√d² − r, 0)` — the value the queue carries, `round(s²)`.
    pub fn visit(&self, cx: f32, cy: f32, cz: f32, r: f32, nodes: &[RegisteredNode]) -> Option<u32> {
        self.gate(cx, cy, cz, r, nodes).ok()
    }

    /// [`Self::visit`] with the gate that dropped the cell.
    pub fn gate(&self, cx: f32, cy: f32, cz: f32, r: f32, nodes: &[RegisteredNode]) -> Result<u32, CellDrop> {
        let v = &self.v;
        if NODE_PRUNE && nodes.iter().all(|n| node_wholly_pruned(v, n.cx, n.cz, n.r)) {
            return Err(CellDrop::Nodes);
        }
        let centre = Vec3::new(cx, cy, cz);
        if !v.sphere_visible(centre, r) {
            return Err(CellDrop::Frustum);
        }
        let (eye, c) = ([v.eye.x, v.eye.z], [cx, cz]);
        if self.near.iter().any(|o| segments_cross([o.a[0], o.a[1]], [o.a[0] + o.d[0], o.a[1] + o.d[1]], eye, c)) {
            return Err(CellDrop::Occluded);
        }
        let s = (bit_root((centre - v.eye).length_squared()) - r).max(0.0);
        if s * s < v.draw * v.draw {
            Ok((s * s).round_ties_even() as u32)
        } else {
            Err(CellDrop::Distance)
        }
    }
}

/// Which gate of the walk dropped a cell ([`CellWalk::gate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellDrop {
    Nodes,
    Frustum,
    Occluded,
    Distance,
}

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

    /// Diagnostic: the `k`-th scene instance flare's point and direction in world space, whether it
    /// runs or not.
    pub fn instance_flare(&self, k: usize) -> Option<(Vec3, Vec3)> {
        let (slot, f) = self.instance_flares.get(k)?;
        let m = self.items.get(self.instance_culls.get(*slot)?.first_item)?.transform;
        Some((m.transform_point3(f.position), m.transform_vector3(f.direction)))
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
        placed.chain(instances).collect()
    }
}

/// The `Vis ` level record's segments (loader 0x488A60 → 0x487330): `b` records of two vec3.
pub fn track_occluders(track: &TrackAssets) -> Vec<Occluder> {
    let Some(e) = track.crp.level.iter().find(|e| e.is_level(b"Vis ")) else { return Vec::new() };
    let nfs_formats::crp::CrpEntry::Level { b: count, .. } = e else { return Vec::new() };
    let Ok(d) = track.crp.entry_data(&track.crp_data, e) else { return Vec::new() };
    let f = |o: usize| bytes::read_f32(d, o);
    (0..*count as usize)
        .take_while(|i| (i + 1) * 24 <= d.len())
        .map(|i| {
            let o = i * 24;
            let (a, b) = (Vec3::new(f(o), f(o + 4), f(o + 8)), Vec3::new(f(o + 12), f(o + 16), f(o + 20)));
            let dd = [b.x - a.x, b.z - a.z];
            let len2 = dd[0] * dd[0] + dd[1] * dd[1];
            Occluder { raised: a.y != 0.0, a: [a.x, a.z], d: dd, len2, inv_len2: 1.0 / len2 }
        })
        .collect()
}

/// Every placed article that carries `ef` parts, by `.crp` article index, with its drawer, the
/// drawer's sphere and its LOD records. The sphere: `Base+0x2C…` and `Base+0x3C` (0x477DB0); for an
/// instance through its `tr` — the centre transformed, the radius the largest component of
/// `M·(r, r, r)` (0x4786A0 — its player's own radius, slot 8 0x471150, is 0);
/// the mid-ground's `max(2.25·r, 200)` (0x47CF00).
///
/// A `WAnimBundle` article's sphere (ported 2026-09-30, `COVERAGE-2026-09-26.md`
/// §14.79) is `Base`'s through its player's matrix (0x477DB0 → slot 5 0x4777B0 → the player's slot 1
/// 0x4715A0 → 0x470E90): the entry built here is the one at frame 0 — what the registration
/// 0x487D72 sees, the player being at `ticks = 0` — and [`TrackWorld::animate`] replaces it with
/// the current frame's every tick. The vertices of these articles are in their own frame and the
/// `Anqt` frame carries the placement (alps' `RESCUEHELI` at (−476, 285, −3146), autobahn's
/// `carrage01…05` and `Locomotive` along up to 1 km of track, …; `nfs-game/examples/animbundle.rs`).
/// An `Anqt` article with no `Anim` part (none is shipped) plays like any other: the player's frame
/// count is the `Anqt` record's own (0x4779E0…, `CrpEntry::b`), no `Anim` part is looked up.
pub fn cull_articles(track: &TrackAssets) -> Vec<Option<CullArticle>> {
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return Vec::new() };
    arts.iter()
        .map(|a| {
            let g = track.crp.article_geometry(&track.crp_data, a).ok()?;
            let base = g.base.as_ref()?;
            if base.is_library() {
                return None;
            }
            let (centre, r, lods) = article_base(track, a)?;
            let (bbox_min, bbox_max) = (Vec3::from(base.bbox_min), Vec3::from(base.bbox_max));
            // `WAnimBundle`'s item (an `Anqt` article with no `tr`): its matrix is the player's
            // (slot 5 0x4777B0), at the registration's tick — frame 0 (the player is started at
            // `WAnimBundle::Load`, 0x477A0E, and the track's registration 0x487D72 follows at
            // `ticks = 0`; a state-0 player samples frame 0 itself, 0x4715A0 → 0x470F70(0, 0, 0))
            let anim0 = if g.transform.is_none() && a.level(b"Anqt").is_some() {
                track.crp.article_animation(&track.crp_data, a).map(|t| anim_matrix(&t, 0.0, WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME, WANIM_BUNDLE_DEFAULT_LOOPING))
            } else {
                None
            };
            let (drawer, centre, radius, footprint_min, footprint_max) = if let Some(t) = g.transform.as_ref().map(Mat4::from_cols_array).or(anim0) {
                let tr = t.transform_vector3(Vec3::splat(r)).max_element();
                // the item's `getBounds` (slot 2, 0x4893A0): slot 4's `Base` box as eight corners,
                // and — for the bundles whose `+4` byte is set (`WInstanceBundle` 0x478040,
                // `WAnimBundle` 0x477710, `WSceneBundle` 0x482840) — each corner moved through
                // slot 5's matrix (`call [eax+0x14]`, then 0x539F50 over the eight points,
                // 0x489486…0x4894A6). *(Read 2026-09-30. The 2026-09-26 note here said "no matrix
                // multiply anywhere in 0x4893A0" and that the original registers an instance's raw
                // box; it overlooked the `this[1]` branch. The transformed corners are what
                // 0x47B660's world box folds (0x47B6C6…0x47B75C); the instance is registered in the
                // tree by its sphere, not by this box — [`build_article_nodes`].)*
                let corners = [
                    Vec3::new(bbox_min.x, bbox_min.y, bbox_min.z),
                    Vec3::new(bbox_max.x, bbox_min.y, bbox_min.z),
                    Vec3::new(bbox_min.x, bbox_max.y, bbox_min.z),
                    Vec3::new(bbox_max.x, bbox_max.y, bbox_min.z),
                    Vec3::new(bbox_min.x, bbox_min.y, bbox_max.z),
                    Vec3::new(bbox_max.x, bbox_min.y, bbox_max.z),
                    Vec3::new(bbox_min.x, bbox_max.y, bbox_max.z),
                    Vec3::new(bbox_max.x, bbox_max.y, bbox_max.z),
                ]
                .map(|c| t.transform_point3(c));
                let (mut fmin, mut fmax) = (corners[0], corners[0]);
                for &c in &corners[1..] {
                    fmin = fmin.min(c);
                    fmax = fmax.max(c);
                }
                (if anim0.is_some() { ArticleDrawer::Anim } else { ArticleDrawer::Instance }, t.transform_point3(centre), tr, fmin, fmax)
            } else if a.level(b"Anqt").is_some() {
                (ArticleDrawer::Anim, centre, r, bbox_min, bbox_max)
            } else if base.flags & 0x6000 != 0 {
                // `0x47D010`: the box scaled ×2.25 about its own centre — `RE/RENDERING.md`
                // "The static items' registration" / `COVERAGE-2026-09-26.md` §14.36.
                let mid = (bbox_min + bbox_max) * 0.5;
                (
                    ArticleDrawer::MidGround { band: if base.flags & 0x2000 != 0 { 600.0 } else { 700.0 } },
                    centre,
                    (2.25 * r).max(200.0),
                    mid + (bbox_min - mid) * 2.25,
                    mid + (bbox_max - mid) * 2.25,
                )
            } else {
                (ArticleDrawer::Static, centre, r, bbox_min, bbox_max)
            };
            Some(CullArticle { name: g.name.clone(), drawer, centre, radius, footprint_min, footprint_max, lods })
        })
        .collect()
}

/// The node pass (`0x47F400`/`0x47F310`) over the fitted tree ([`crate::fitted_tree`]) with the
/// original's registration ([`build_article_nodes`]). On since 2026-09-30: the alps
/// tick-900 loss that kept it off (verge and mountain vanishing beside the car) was the port's own
/// side-plane test running on a non-unit normal (fixed in [`node_wholly_pruned`], COVERAGE
/// §14.37); with the fix the off/on frames on alps 900, coastal 300, monaco1 900 and autobahn 1400
/// match pixel-for-pixel in scenery (monaco1 differs only in opponent paint, run-to-run RNG).
const NODE_PRUNE: bool = true;

/// What 0x47B660 builds the fitted tree from ([`crate::fitted_tree`]'s module doc): the union of
/// every placed item's eight box corners (the root's box) and the `(x, z)` of every vertex of every
/// item's first-LOD `vt` part (what 0x47BE30 splits).
#[derive(Debug, Clone, Default)]
pub struct TreePopulation {
    pub points: Vec<(f32, f32)>,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// 0x47B660 over the items of `WInstanceBundle`, `WAnimBundle`, `WMidGroundBundle` and
/// `WStaticBundle` (the track's `.crp` articles that carry a `Base` and are not library articles —
/// [`cull_articles`]'s `Some` entries), read at `WTrack::load` (0x4879D0); `WSceneBundle`'s items
/// join through [`tree_population_with_scene`] (its count is set by 0x482970 at 0x482C76 /
/// 0x482C97 and its slots spawned at 0x487ABD, before 0x487CD8). Each item contributes its box corners' bounds — `getBounds`,
/// slot 2 0x4893A0, the corners `footprint_min/max` describe — and the vertices of the `vt` part
/// named by the LOD word of its first `Base` LOD record (`Base + 0x64 + 12·[Base+8]`, +8), all of
/// them (`[part+8]` vertices, 0x47B80A…0x47B868), in the part's own frame — an instance's vertices
/// are not moved by its `tr` here.
pub fn tree_population(track: &TrackAssets, articles: &[Option<CullArticle>]) -> TreePopulation {
    tree_population_with_scene(track, articles, &[])
}

/// [`tree_population`] with the `WSceneBundle`'s items too: the bundle's live slots
/// (`[+0x18]`, 0x482C76…0x482C97: `min(0x46D9C0(), 64)`) are filled by the spawn 0x46D1D0
/// (0x487ABD) *before* 0x47B660 runs (0x487CD8), so its items — the `.scn` GEOM elements, each a
/// library article at the record's matrix (`(crp article, world matrix)`) — are in the tree's
/// box and vertex population like any bundle's. *(§14.76 read the count as 0 there: 0x482970
/// zeroes `[+0x18]` at 0x482987 and sets it at 0x482C76 / 0x482C97.)* APPROXIMATION: a scene
/// with no element keeps 64 slots live, parked as `DFLT` at y 9999 in the original (`[+0x44]`,
/// 0x482C76): the caller ([`TrackWorld::upload`]) passes them in `scene` — the five Monaco tracks
/// and the skidpad have no start scene, so that is every race there.
pub fn tree_population_with_scene(track: &TrackAssets, articles: &[Option<CullArticle>], scene: &[(usize, Mat4)]) -> TreePopulation {
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return TreePopulation::default() };
    let (mut min, mut max) = ([1.0e9f32; 3], [-1.0e9f32; 3]);
    let mut points = Vec::new();
    let mut any = false;
    let fold = |a: &nfs_formats::crp::Article, lo: [f32; 3], hi: [f32; 3], min: &mut [f32; 3], max: &mut [f32; 3], points: &mut Vec<(f32, f32)>| {
        for k in 0..3 {
            min[k] = min[k].min(lo[k]);
            max[k] = max[k].max(hi[k]);
        }
        let Some(base) = a.level(b"Base").and_then(|e| track.crp.entry_data(&track.crp_data, e).ok()) else { return };
        let u32_at = |o: usize| bytes::get_u32(base, o).map_or(0, |v| v as usize);
        let o = 0x64 + 12 * u32_at(8);
        let Some(lod) = bytes::get_u16(base, o + 8) else { return };
        // a record with LOD `0xFFFF` takes the article's first `vt` (0x47B819…0x47B81E)
        let vt = if lod == 0xFFFF { a.parts.iter().find(|p| p.is_part(b"vt")) } else { a.find(b"vt", lod) };
        let Some(vt) = vt else { return };
        let Ok(d) = track.crp.entry_data(&track.crp_data, vt) else { return };
        let f = |o: usize| bytes::read_f32(d, o);
        points.extend((0..d.len() / 16).map(|i| (f(16 * i), f(16 * i + 8))));
    };
    for (a, c) in arts.iter().zip(articles) {
        let Some(c) = c else { continue };
        any = true;
        fold(a, c.footprint_min.to_array(), c.footprint_max.to_array(), &mut min, &mut max, &mut points);
    }
    for &(idx, m) in scene {
        let Some(a) = arts.get(idx) else { continue };
        let Some(base) = track.crp.article_geometry(&track.crp_data, a).ok().and_then(|g| g.base) else { continue };
        let (bmin, bmax) = (Vec3::from(base.bbox_min), Vec3::from(base.bbox_max));
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for k in 0..8 {
            let c = m.transform_point3(Vec3::new(if k & 1 == 0 { bmin.x } else { bmax.x }, if k & 2 == 0 { bmin.y } else { bmax.y }, if k & 4 == 0 { bmin.z } else { bmax.z }));
            lo = lo.min(c);
            hi = hi.max(c);
        }
        any = true;
        fold(a, lo.to_array(), hi.to_array(), &mut min, &mut max, &mut points);
    }
    if !any {
        return TreePopulation::default();
    }
    TreePopulation { points, min, max }
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

/// A node an article is registered into: its allocation number and its circle `(cx, cz, r)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegisteredNode {
    pub id: u32,
    pub cx: f32,
    pub cz: f32,
    pub r: f32,
}

/// The registration of every [`cull_articles`] entry (0x487D72's loop over the bundles in the
/// order Instance, Anim, MidGround, Static — `.crp` order inside each — into 0x489370 → 0x489530),
/// and each article's place in that order (the order the refs sit in a node's list).
///
/// Two paths, by the bundle's `+5` byte (0x48953C):
/// * **`+5` clear — `WMidGroundBundle` and `WStaticBundle`** (0x4896CA): the item's eight box
///   corners (`getBounds`, no matrix for these two), their hull (0x5A0480), 0x47BDC0's smallest
///   node holding all corners and 0x489930's budgeted spread from there
///   ([`crate::fitted_tree::FittedTree::locate`] / [`spread`](crate::fitted_tree::FittedTree::spread)).
/// * **`+5` set — `WInstanceBundle` and `WAnimBundle`** (ctors 0x478040 / 0x477710 set `+4` and
///   `+5`; 0x489547…0x4895F9): **one** node, 0x47BC20's answer for the item's bounding sphere
///   (slot 3, through the item's matrix) — never its box, and never a transformed box either
///   ([`crate::fitted_tree::FittedTree::node_for_sphere`]). *(This corrects the 2026-09-26
///   reading, which spread an instance's box through the hull path with a note that the original
///   read the box raw and untransformed: the hull path is not the instances' at all; the matrix
///   application in 0x4893A0 — `this[1]` set, slot 5's matrix, 0x539F50 over the eight corners —
///   is what 0x47B660's world box reads for them.)*
///
/// A `WAnimBundle` article is registered here at frame 0's sphere (the registration's tick); every
/// eighth rendered frame the ref is re-registered at the current sphere, moving to the end of its
/// new node's list when the node differs ([`TrackWorld::animate`], 0x488CC0 → 0x489370 → 0x489530).
pub fn build_article_nodes(tree: &crate::fitted_tree::FittedTree, articles: &[Option<CullArticle>]) -> (Vec<Vec<RegisteredNode>>, Vec<u32>) {
    let reg = |n: &crate::fitted_tree::Node| {
        let (cx, cz, r) = tree.circle(n);
        RegisteredNode { id: n.id, cx, cz, r }
    };
    let mut nodes = vec![Vec::new(); articles.len()];
    let mut seq = vec![u32::MAX; articles.len()];
    let mut next = 0u32;
    let rank = |d: ArticleDrawer| match d {
        ArticleDrawer::Instance => 0,
        ArticleDrawer::Anim => 1,
        ArticleDrawer::MidGround { .. } => 2,
        ArticleDrawer::Static => 3,
    };
    let mut order: Vec<usize> = (0..articles.len()).filter(|&i| articles[i].is_some()).collect();
    order.sort_by_key(|&i| (rank(articles[i].as_ref().map_or(ArticleDrawer::Static, |a| a.drawer)), i));
    for i in order {
        let Some(a) = articles[i].as_ref() else { continue };
        seq[i] = next;
        next += 1;
        match a.drawer {
            ArticleDrawer::Instance | ArticleDrawer::Anim => {
                nodes[i] = vec![reg(tree.node_for_sphere(a.centre.x, a.centre.y, a.centre.z, a.radius))];
            }
            ArticleDrawer::Static | ArticleDrawer::MidGround { .. } => {
                // 0x4893A0's corner order, x and z: min/min, max/min, max/min, min/min, then the
                // same four at max z
                let (lo, hi) = (a.footprint_min, a.footprint_max);
                let corners = [(lo.x, lo.z), (hi.x, lo.z), (hi.x, lo.z), (lo.x, lo.z), (lo.x, hi.z), (hi.x, hi.z), (hi.x, hi.z), (lo.x, hi.z)];
                let hull = crate::fitted_tree::convex_hull(&corners);
                let start = tree.locate(&corners);
                nodes[i] = tree.spread(start, &hull).into_iter().map(reg).collect();
            }
        }
    }
    (nodes, seq)
}

/// `0x47F310`: is a node's fixed circle `(ncx, ncz, nr)` **wholly outside** the ground wedge or the
/// far circle — pruned before any per-item test runs? `far`/the half-angle are the same
/// `v.draw`/`v.tan_h` the port's own per-item test already uses (`RE/RENDERING.md` "The footprint —
/// 0x47F550/0x47F210": both this test's and the frustum's angle and far distance are literally
/// `cam+0x130`/`cam+0x124` = `[0x6197CC]`, one and the same camera quantity). The steep-look
/// special case is carried too: `|forward.y| > 0.8` (`[0x5B37B8]`, 0x47F76B) sets `far = 50` and
/// skips the side test entirely (`this+0xA8 = 1`).
pub fn node_wholly_pruned(v: &CullView, ncx: f32, ncz: f32, nr: f32) -> bool {
    let (dx, dz) = (ncx - v.eye.x, ncz - v.eye.z);
    let steep = v.forward.y.abs() > 0.8;
    let far = if steep { 50.0 } else { v.draw };
    let d = (dx * dx + dz * dz).sqrt();
    if d - far > 0.0 && d - far > nr {
        return true;
    }
    if steep {
        return false;
    }
    for side in [1.0f32, -1.0] {
        let ex = v.forward.x + side * v.tan_h * v.right.x;
        let ez = v.forward.z + side * v.tan_h * v.right.z;
        let (nx, nz) = (-ez * side, ex * side);
        // `forward + tan_h·right` has length `sec h`; 0x47F310's side normal is unit length
        // (the xz forward renormalised by 0x5328D0, then rotated by the real angle h through
        // 0x539BD0/0x53A330), so the distance is divided back down — as `sphere_visible`
        // does with its `cos`. Without it the side test overshot by `sec h` (≈1.36 at
        // tan h≈0.92) and pruned visible nodes (alps tick 900, COVERAGE §14.37).
        let s = (nx * dx + nz * dz) / (nx * nx + nz * nz).sqrt();
        if s > 0.0 && s > nr {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod cull_tree_tests {
    use super::*;

    /// Locked against `cull_tree_node_prune_census.rs`'s own `sanity_check` (same formula,
    /// independently re-derived here to catch a sign regression in either copy).
    #[test]
    fn node_prune_sides_and_far() {
        // `right`/`up` follow `main.rs`'s own construction: `right = forward.cross(up)`, so
        // `forward = +Z, up = +Y` gives `right = -X` (not `+X` — the wedge's side normals below
        // must use the *same* handedness `CullView.right` already carries everywhere else).
        let base = CullView { eye: Vec3::ZERO, forward: Vec3::Z, right: Vec3::NEG_X, up: Vec3::Y, tan_h: 1.0, tan_v: 1.0, near: 1.0, draw: 300.0 };
        // dead ahead at 100 m, within far and the wedge: never pruned.
        assert!(!node_wholly_pruned(&base, 0.0, 100.0, 1.0));
        // 90 degrees to the side, same distance: wholly outside a 45 degree (tan_h = 1) wedge.
        assert!(node_wholly_pruned(&base, 100.0, 0.0, 1.0));
        // dead ahead, past far by more than its own radius: wholly outside the far circle.
        assert!(node_wholly_pruned(&base, 0.0, 350.0, 1.0));
        // dead ahead, past far, but its own radius reaches back inside: not pruned.
        assert!(!node_wholly_pruned(&base, 0.0, 320.0, 30.0));
        // steep look-down: far collapses to 50 m and the side test is skipped.
        let steep = CullView { forward: Vec3::new(0.0, -0.9, 0.436), ..base };
        assert!(node_wholly_pruned(&steep, 100.0, 0.0, 1.0)); // would fail the wedge if it ran, but far=50 alone prunes it
        assert!(!node_wholly_pruned(&steep, 0.0, 40.0, 1.0));
    }
}

/// An article's `Base` sphere in its own frame — `Base+0x2C…` and `Base+0x3C` — and its LOD
/// records `Base+0x64` (`Base+0xC` of them, 12 bytes each).
fn article_base(track: &TrackAssets, a: &nfs_formats::crp::Article) -> Option<(Vec3, f32, Vec<CullLod>)> {
    let g = track.crp.article_geometry(&track.crp_data, a).ok()?;
    let base = g.base.as_ref()?;
    let bytes = track.crp.entry_data(&track.crp_data, a.level(b"Base")?).ok()?;
    let lods = (0..base.lod_count as usize)
        .map_while(|i| {
            let o = 100 + 12 * i;
            let w = |k: usize| nfs_formats::bytes::read_u16(bytes, o + k);
            (o + 12 <= bytes.len()).then(|| CullLod { distance: w(10), effects: bytes[o + 1] != 0, first: w(4), last: w(6), lod: w(8) as u8 })
        })
        .collect();
    Some((Vec3::from(base.centre), base.radius, lods))
}

/// One kind-4 `ef` part: its sixteen floats from `+8`.
#[derive(Debug, Clone, PartialEq)]
pub struct Projector {
    /// the `.crp` index of the article carrying it, and the part's own index
    pub article: usize,
    pub part: u16,
    /// `[0…2]`, `[3]`
    pub point: Vec3,
    pub radius: f32,
    /// `[6…8]`, `[10]` and `[11…13]`, `[15]`: `u = u.xyz · world + u.w`, likewise `v` (0x446130)
    pub u: [f32; 4],
    pub v: [f32; 4],
    /// `+0x1C` of the part, the texture's name (0x44B000)
    pub texture: String,
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

/// The kind-4 `ef` parts of every placed article. The planes are taken as the file has them: the
/// `Shadow` articles that carry them have no `tr` on any shipped track (canyon 59, farmland 18,
/// castle 13), so they are in world space already; the point still goes through the transform.
pub fn track_projectors(track: &TrackAssets) -> Vec<Projector> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for (article, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x48 || bytes::read_u32(d, 0) != 4 {
                continue;
            }
            let f = |i: usize| {
                let o = 8 + 4 * i;
                bytes::read_f32(d, o)
            };
            out.push(Projector {
                article,
                part: p.part_index(),
                point: tr.transform_point3(Vec3::new(f(0), f(1), f(2))),
                radius: f(3),
                u: [f(6), f(7), f(8), f(10)],
                v: [f(11), f(12), f(13), f(15)],
                texture: d[0x1C..0x20].iter().map(|&c| c as char).collect(),
            });
        }
    }
    out
}

/// One flare a track article carries: its `ef` part's data from `+8`, taken through the article's
/// transform.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackFlare {
    /// the `.crp` index of the article carrying it, and the part's own index (the instance drawer
    /// runs only the parts of its LOD's range)
    pub article: usize,
    pub part: u16,
    /// `+0…+8`
    pub position: Vec3,
    /// `+0x10…+0x18` — what the draw turns towards the camera for its angle (0x451A0C…0x451A90)
    pub direction: Vec3,
    /// `+0x1C` — the `flare.dat` id
    pub id: u32,
    /// `+0x20` — the part's own blink phase (flag 0x10)
    pub phase: u32,
}

/// The kind-3 `ef` parts of every placed article, their data from `+8` through the article's
/// transform.
pub fn track_emitters(track: &TrackAssets) -> Vec<crate::track_particles::Emitter> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for (article, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x34 || bytes::read_u32(d, 0) != 3 {
                continue;
            }
            let f = |o: usize| bytes::read_f32(d, o);
            // the point through `tr` is where the running system emits (0x450980 → slot 4
            // 0x4550B0 with the placement's point, 0x450B0D…0x450B22); the prime's point and the
            // two directions are the part's own — 0x455030 copies the part's 48 bytes and
            // re-seats only the first 16 (0x455069, 0x4550B0), and the static creator hands the
            // constructor the part itself. On `tr`-less articles the two are the same numbers.
            let local = Vec3::new(f(8), f(0xC), f(0x10));
            out.push(crate::track_particles::Emitter {
                article,
                part: p.part_index(),
                position: tr.transform_point3(local),
                local,
                dir_a: Vec3::new(f(0x18), f(0x1C), f(0x20)),
                dir_b: Vec3::new(f(0x24), f(0x28), f(0x2C)),
                tag: bytes::read_u32(d, 0x30),
            });
        }
    }
    out
}

/// The kind-2 `ef` parts of every placed article (library articles are drawn only through
/// scenes — [`TrackWorld::running_flares`]; 0x450D10 runs for `RDrawArticleBundle`, the placed ones).
pub fn track_flares(track: &TrackAssets) -> Vec<TrackFlare> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for (article, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        out.extend(article_flares(track, article, a, tr));
    }
    out
}

/// The kind-2 `ef` parts of one article, their data from `+8` through `tr`.
fn article_flares(track: &TrackAssets, article: usize, a: &nfs_formats::crp::Article, tr: Mat4) -> Vec<TrackFlare> {
    let mut out = Vec::new();
    for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
        let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
        if d.len() < 0x2C || bytes::read_u32(d, 0) != 2 {
            continue;
        }
        let f = |o: usize| bytes::read_f32(d, o);
        let w = |o: usize| bytes::read_u32(d, o);
        out.push(TrackFlare {
            article,
            part: p.part_index(),
            position: tr.transform_point3(Vec3::new(f(8), f(0xC), f(0x10))),
            direction: tr.transform_vector3(Vec3::new(f(0x18), f(0x1C), f(0x20))),
            id: w(0x24),
            phase: w(0x28),
        });
    }
    out
}

/// One `[sectionN]` of a track's `.map`: the slice range it covers and the six environment-map ids
/// a car's `R3DSkyMapTextureLayer` takes inside it, open and covered (`RE/notes/lighting.md` §3.2 —
/// *corrected 2026-09-26*: these are the **sky-map** pass's textures, `[0x6115CC]+0xD0…+0xD8`; the
/// specular layer reads `+0xDC…+0xE4`, `carcmn.fsh`'s `SPC1…SPC3`).
///
/// The record is 0x48 bytes in the game (`race-logic.md` §6.4); `EnvSet::select(slice)` 0x441140 is
/// a linear search for the first section with `startslice ≤ slice ≤ endslice` and leaves the
/// current section **null** when none matches, and `EnvSet::texture(covered, layer)` 0x441180 reads
/// `ids[layer + 3·covered]`. The shipped sections mark exactly the covered stretches — monaco1's
/// (141, 244) is its tunnel — so a car's sky-map pass draws inside them and **nothing** anywhere else:
/// with no section `EnvSet::texture` answers 0 (0x4411AD), and the layer skips its pass's draw when
/// it has no texture (0x445B86…0x445BB1).
#[derive(Debug, Clone, Copy)]
pub struct MapSection {
    pub start: u32,
    pub end: u32,
    /// `[texture1.open, texture2.open, texture3.open, texture1.covered, …]` as `.env` SHPI names
    /// packed little-endian into a u32
    pub ids: [u32; 6],
    pub texture_size: f32,
    pub aspect: f32,
}

impl MapSection {
    /// All the sections of a `.map`, in file order.
    pub fn all(map: &nfs_formats::ini::Ini) -> Vec<Self> {
        let count = map.section("header").and_then(|h| h.get_i64("count")).unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let s = map.section(&format!("section{i}"))?;
                let id = |k: &str| s.get_i64(k).unwrap_or(0) as u32;
                Some(Self {
                    start: id("startslice"),
                    end: id("endslice"),
                    ids: [
                        id("texture1.open"),
                        id("texture2.open"),
                        id("texture3.open"),
                        id("texture1.covered"),
                        id("texture2.covered"),
                        id("texture3.covered"),
                    ],
                    texture_size: s.get_f32("texturesize").unwrap_or(6.0),
                    aspect: s.get_f32("aspect").unwrap_or(1.0),
                })
            })
            .collect()
    }
}

/// The five colours in a track's `.map` `[header]` (0x4409D0 parses them into the environment-map
/// object at `+0x00`, `+0x10`, `+0x20`, `+0x30`, `+0x40`, defaulting to 1 or 0.3).
///
/// They are the multipliers of the **car** light block: 0x43A8A6 samples the colour of the ground
/// patch under the car (0x4746A0 → 0x474700, a bilinear blend of the patch's four vertex colours)
/// and multiplies one slot of the car's light block by each of them (`RE/notes/lighting.md`).
/// Every shipped file has r = g = b, so each is really a scalar; `tint` runs 0.25…1.80 and
/// `ambient` 0.29…1.23.
#[derive(Debug, Clone, Copy)]
pub struct MapLight {
    pub tint: [f32; 3],
    pub ambient: [f32; 3],
    pub interior: [f32; 3],
    pub interior_diffuse: [f32; 3],
    pub light_decal: [f32; 3],
}

impl Default for MapLight {
    fn default() -> Self {
        Self { tint: [1.0; 3], ambient: [0.3; 3], interior: [0.3; 3], interior_diffuse: [0.3; 3], light_decal: [0.3; 3] }
    }
}

impl MapLight {
    /// Read the header. The files write each colour as **four** floats, alpha first.
    pub fn from_map(map: &nfs_formats::ini::Ini) -> Self {
        let mut out = Self::default();
        let Some(h) = map.section("header") else { return out };
        let get = |key: &str| -> Option<[f32; 3]> {
            let v = h.get_f32_list(key)?;
            (v.len() == 4).then(|| [v[1], v[2], v[3]])
        };
        if let Some(v) = get("tint") {
            out.tint = v;
        }
        for (dst, key) in [
            (&mut out.ambient, "ambient"),
            (&mut out.interior, "interior"),
            (&mut out.interior_diffuse, "interiordiffuse"),
            (&mut out.light_decal, "lightdecal"),
        ] {
            if let Some(v) = get(key) {
                *dst = v;
            }
        }
        out
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
            .map(|t| r.upload_texture_mips(t.width, t.height, &t.mips))
    })
}

impl TrackWorld {
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
        let g = self.collision.ground(pos.x, pos.y + 2.0, pos.z, 20.0).map(|h| [h.color[0], h.color[1], h.color[2]]).unwrap_or([1.0; 3]);
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
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp, lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: true, view_cull: true, depth_bias, false_sort })
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
        let mut track_lods = Self::upload_track_lods(r, &geometry, &|k| breakable_articles.contains(&k) || smackable_articles.contains(&k), &mut material_for);
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
            .map(|a| ((a[1] + a[2] + a[3]) / 3.0).clamp(0.2, 0.9))
            .unwrap_or(0.35);
        tracing::info!(items = items.len(), triangles = collision.triangle_count(), walls = walls.len(), "track uploaded");
        if !animated.is_empty() {
            tracing::info!(props = animated.len(), "animated props");
        }
        let map_light = MapLight::from_map(&track.map);
        let sun_direction = track.sky_bin.as_ref().map(|b| -Vec3::from(b.sun_position()).normalize_or_zero()).filter(|d| *d != Vec3::ZERO).unwrap_or(Vec3::NEG_Y);
        let map_sections = MapSection::all(&track.map);
        Self::mark_lit_items(track, &materials, &mat_ids, &mut items, &mut track_lods);
        // materials whose render method builds `RCustomLayers::LayerUVAnimate` (0x8001) — water
        // and flags/banners (`RE/drafts/round1/render-core.md` §4.4)
        let uv_animate: HashMap<MaterialId, nfs_assets::track::UvAnimate> = mat_ids
            .iter()
            .filter_map(|(id, mid)| materials.get(*id as usize).and_then(|m| nfs_assets::track::uv_animate(&methods, m)).map(|a| (*mid, a)))
            .collect();
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
            projector_registered,
            cull_articles,
            article_nodes,
            article_seq,
            track_lods,
            occluders,
            instance_culls,
            instance_regs: Vec::new(),
            instance_flares,
            item_cull,
            uv_animate,
            uv_plan: UvPlan::default(),
            anim_articles,
            next_seq,
        };
        world.register_instances();
        Ok(world)
    }

    /// Every track texture in one go: the pixels are written from worker threads, the ids are the
    /// same consecutive ones, in file order. Keyed by name: `(id, has_alpha)`.
    fn upload_track_textures(r: &mut Renderer, track: &TrackAssets) -> HashMap<String, (nfs_renderer::TextureId, bool)> {
        let mut tex_ids = HashMap::new();
        let items: Vec<(u32, u32, &[Vec<u8>])> = track.textures.iter().map(|t| (t.width, t.height, t.mips.as_slice())).collect();
        for (tex, id) in track.textures.iter().zip(r.upload_textures(&items)) {
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
        skip: &dyn Fn(usize) -> bool,
        material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId,
    ) -> Vec<TrackArticleGpu> {
        let mut track_lods: Vec<TrackArticleGpu> = Vec::new();
        for art in &geometry.world {
            if skip(art.index) {
                continue;
            }
            let lods = art
                .lods
                .iter()
                .map(|(lod, meshes)| {
                    (*lod, meshes.iter().map(|mm| DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), art.transform)).collect())
                })
                .collect();
            let parts = art.lods.iter().map(|(_, meshes)| meshes.iter().map(|mm| (mm.flags, mm.key_hi, art.transform.transform_point3(Vec3::from(mm.centre)))).collect()).collect();
            track_lods.push(TrackArticleGpu { article: art.index, lods, parts });
        }
        track_lods
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
                    items.push(DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), own.transform));
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
        for it in items.iter_mut().chain(track_lods.iter_mut().flat_map(|a| a.lods.iter_mut().flat_map(|(_, v)| v.iter_mut()))) {
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
                        items.push(DrawItem::new(*mesh, *material, world_tr));
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
        let env_items: Vec<(u32, u32, &[Vec<u8>])> = track.env_textures.iter().map(|t| (t.width, t.height, t.mips.as_slice())).collect();
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

    /// Diagnostic: the current sphere the walk answers for the `WAnimBundle` article named
    /// `name` (case-insensitive substring), and the article's name.
    pub fn anim_article_sphere(&self, name: &str) -> Option<(String, Vec3, f32)> {
        let name = name.to_lowercase();
        self.anim_articles.iter().find_map(|a| {
            let c = self.cull_articles.get(a.article)?.as_ref()?;
            c.name.to_lowercase().contains(&name).then(|| (c.name.clone(), c.centre, c.radius))
        })
    }

    /// `Graphics.Headlights` below Projected: no item takes the `HEAD` decal (`main.rs`).
    pub fn clear_uv_lit(&mut self) {
        for it in self.items.iter_mut().chain(self.track_lods.iter_mut().flat_map(|a| a.lods.iter_mut().flat_map(|(_, v)| v.iter_mut()))) {
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
            if let Some(g) = gpu.and_then(|g| self.track_lods.get_mut(g)) {
                for (_, items) in g.lods.iter_mut() {
                    for it in items.iter_mut() {
                        it.transform = m;
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
    /// a track item's UVs. APPROXIMATION: the original phase-offsets mode 2 by the layer's own UV
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
                .or_insert_with(|| by_index.get(&k).map(|a| a.meshes.iter().map(|mm| (r.upload_mesh(&mm.mesh), material_for(r, mm.material))).collect()).unwrap_or_default())
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
                    items.push(DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), own.transform));
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
        for b in &mut self.broken {
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
                        b.pieces.push(DrawItem::new(*mesh, *material, b.instance * m));
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
        }
        if let SmackableOwner::Article(article) = owner
            && let Some(a) = self.smack_anims.iter_mut().find(|a| a.article == article)
        {
            a.position = pose.w_axis.truncate();
        }
    }
}

/// A mission-scene instance's smackable, when its `animdefs.txt` definition (0x47ED90's lookup)
/// says `kSmackable` (0x4772F0's `def+0x10 == 3`): a car record of its own is built from it
/// (0x476ED0, then 0x4A17F0 / 0x4A1280) with its points from the library article's vertices in
/// the instance's frame. The article's own `tr` is **not** applied: 0x476ED0 reads only the
/// article's `Base` and `vt` (0x476F35, 0x476FAB), and 0x483360 takes the pose from the scene
/// record's matrix (0x4774F8…0x477515), which has no `tr` in it (0x482BF1 / 0x482EA0) — CONFIRMED.
/// (history: `game-history.md` §9.7) MEDIUM: the GEOM scale is folded
/// into the points and the instance's rotation and position kept as the body's pose — 0x476A80
/// splits the record matrix into the unscaled pose and the axis lengths `rec+0x58`, and where the
/// latter is used again was not read; and the vertices are the port's article meshes (the original
/// reads the `vt` stream).
pub fn smackable_spawn(track: &TrackAssets, art: &nfs_assets::track::ArticleMeshes, inst: &nfs_assets::track::PropInstance) -> Option<SmackableSpawn> {
    if inst.source != 1 {
        return None;
    }
    // the matched **template's** own tag (`art.tag`), not the element's unmatched fourcc — same
    // fourcc-miss fallback substitution as the draw loop's `loop_piece` and `scene_props`
    // (0x482ebe…0x482ec0, e517d04f), missed here until now: `art` is already the caller's
    // fallback-resolved article ([`TrackWorld::upload`]'s own `art` at the `smackable_spawn` call
    // site), so this lookup must follow it too. Unreached on shipped data (every mission-scene tag
    // matches one of the 70 shared library articles), same as the other two.
    let def = track.animdefs.lookup(art.tag).filter(|d| d.collide_type == "kSmackable")?;
    let local = Mat4::from_scale(inst.scale);
    let verts: Vec<Vec3> = art.meshes.iter().flat_map(|mm| mm.mesh.vertices.iter().map(|v| local.transform_point3(Vec3::from(v.position)))).collect();
    let (_, rot, pos) = inst.transform.to_scale_rotation_translation();
    // 0x483360: the record's points are centred and its position moves by the centre
    let mut points = nfs_physics::smackable::point_cloud(&verts);
    let position = nfs_physics::smackable::centre_record(&mut points, pos);
    Some(SmackableSpawn {
        owner: SmackableOwner::Element(inst.element),
        position,
        orientation: glam::Mat3::from_quat(rot).transpose(),
        points,
        mass: def.mass,
        audio_type: def.audio_type_index().unwrap_or(0),
    })
}

/// A track article's smackable (0x4772F0 over the instance bundle's items — every article with a
/// `tr`, 0x47859E — whose `Base` byte 0 lacks 0x40, 0x477377, and whose definition says
/// `kSmackable`, 0x4773EE): its pose is the item record's — the `tr`'s position and its rows,
/// normalised as 0x4A1510 does — and its points `SIGN`'s 0x476B30 (the vertices scaled by the
/// rows' lengths, `nfs_physics::smackable::sign_point_cloud`) or everyone else's 0x476ED0, which
/// takes no scale (`suspension-collision.md` §9 "The track's smackables"). MEDIUM: the vertices
/// are the port's lowest-LOD meshes (the original reads a `vt` stream: 0x476ED0 the article's,
/// 0x476B30 the one the LOD record `Base+0x64 + 12·Base[+4]` names).
pub fn track_smackable_spawn(track: &TrackAssets, art: &nfs_assets::track::ArticleMeshes) -> Option<SmackableSpawn> {
    let def = track.animdefs.lookup(art.tag).filter(|d| d.collide_type == "kSmackable")?;
    let tr = art.transform;
    let rows = [tr.x_axis.truncate(), tr.y_axis.truncate(), tr.z_axis.truncate()];
    let lengths = Vec3::new(rows[0].length(), rows[1].length(), rows[2].length());
    let verts: Vec<Vec3> = art.meshes.iter().flat_map(|mm| mm.mesh.vertices.iter().map(|v| Vec3::from(v.position))).collect();
    let mut points = if def.tag.eq_ignore_ascii_case("SIGN") { nfs_physics::smackable::sign_point_cloud(&verts, lengths) } else { nfs_physics::smackable::point_cloud(&verts) };
    // 0x483360: the record's points are centred and its position moves by the centre
    let position = nfs_physics::smackable::centre_record(&mut points, tr.w_axis.truncate());
    let unit = |v: Vec3| v.normalize_or_zero();
    Some(SmackableSpawn {
        owner: SmackableOwner::Article(art.index),
        position,
        orientation: glam::Mat3::from_cols(unit(rows[0]), unit(rows[1]), unit(rows[2])).transpose(),
        points,
        mass: def.mass,
        audio_type: def.audio_type_index().unwrap_or(0),
    })
}

/// What a smackable's body draws: a mission scene's GEOM element, or a track article.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SmackableOwner {
    Element(usize),
    Article(usize),
}

/// A smackable, as `nfs_physics::smackable::Smackable::new` takes it.
#[derive(Debug, Clone)]
pub struct SmackableSpawn {
    pub owner: SmackableOwner,
    pub position: Vec3,
    /// rows = the body axes, as `Vehicle::orientation`
    pub orientation: glam::Mat3,
    /// `nfs_physics::smackable::point_cloud` of the article, in the body's frame
    pub points: Vec<Vec3>,
    /// the definition's `mass` (kg)
    pub mass: f32,
    /// the definition's `audioType` index
    pub audio_type: u32,
}

impl SmackableSpawn {
    /// The authored pose, body to world.
    pub fn pose(&self) -> Mat4 {
        body_pose(self.position, self.orientation)
    }
}

/// A body's pose as a matrix from its position and its orientation (rows = the body axes).
pub fn body_pose(position: Vec3, orientation: glam::Mat3) -> Mat4 {
    Mat4::from_translation(position) * Mat4::from_mat3(orientation.transpose())
}

/// The default `secondsPerFrame`/loop rule `WAnimBundle::Load` 0x4777F0 gives a `WAnimBundle`
/// article's `AnimPlayer` (`0x471490`, vtable `0x5B460C`) when the article has no
/// `animdefs.txt SECTION:ANIM` piece of its own — the literal `0x3DCCCCCD` and the global constant
/// at `[0x5B45A4]` (`= 1`, its bit 0 the loop flag `AnimPlayer::sample` 0x470E90 tests) passed at
/// 0x4779DF..0x4779F7. CONFIRMED by direct disassembly, `COVERAGE-2026-09-26.md` §14.42 — see
/// [`TrackWorld::upload`]'s own note at the construction sites below for the full address trail
/// and the correction of an earlier pass's `SmackAnim` misidentification.
const WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME: f32 = 0.1;
/// See [`WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME`].
const WANIM_BUNDLE_DEFAULT_LOOPING: bool = true;

/// An `Anqt` track's matrix at `seconds`: `AnimPlayer::sample` 0x470E90's frame pair and blend
/// (0x470F70: the quaternion **lerped** component by component by 0x48A150 — no sign fix, no
/// renormalisation, so the blend of two distant unit quaternions is shorter than 1 — the position
/// and scale lerped), the rotation matrix of that quaternion by 0x48A1E0 (its formula, which
/// assumes a unit quaternion and is applied to whatever the lerp gave), each of the three rows
/// scaled by the frame's scale component (0x5329A0 — the local axes, i.e. `T · R · S` here) and
/// the position in the fourth row (0x539CD0). (AI 2026-09-30: this used a normalised
/// slerp before; the two differ where consecutive frames turn far — the rotor blades, the train.)
pub fn anim_matrix(track: &nfs_formats::crp::AnimTrack, seconds: f32, seconds_per_frame: f32, looping: bool) -> Mat4 {
    let (i, j, t) = track.sample(seconds, seconds_per_frame, looping);
    let (f0, f1) = (&track.frames[i], &track.frames[j]);
    let q: [f32; 4] = std::array::from_fn(|k| (f1.rotation[k] - f0.rotation[k]) * t + f0.rotation[k]);
    let pos = Vec3::from(f0.position).lerp(Vec3::from(f1.position), t);
    let scale = Vec3::from(f0.scale).lerp(Vec3::from(f1.scale), t);
    let scale = if scale.min_element() > 1e-4 { scale } else { Vec3::ONE };
    let rows = quat_rows_0x48a1e0(q);
    let m = glam::Mat3::from_cols(rows[0] * scale.x, rows[1] * scale.y, rows[2] * scale.z);
    Mat4::from_translation(pos) * Mat4::from_mat3(m)
}

/// 0x48A1E0: the three rows of the 3×3 matrix of `q = (x, y, z, w)` — the images of the local
/// axes — from its own `2·` products, without normalising `q`.
fn quat_rows_0x48a1e0(q: [f32; 4]) -> [Vec3; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let (x2, y2, z2) = (x + x, y + y, z + z);
    let xw = x2 * w;
    [
        Vec3::new(1.0 - y2 * y - z2 * z, y2 * x + z2 * w, z2 * x - y2 * w),
        Vec3::new(y2 * x - z2 * w, 1.0 - x2 * x - z2 * z, z2 * y + xw),
        Vec3::new(z2 * x + y2 * w, z2 * y - xw, 1.0 - x2 * x - y2 * y),
    ]
}

/// One `WAnimBundle` article the port moves ([`TrackWorld::animate_articles`]).
struct AnimArticle {
    /// its `.crp` article index (an entry of [`TrackWorld::cull_articles`])
    article: usize,
    track: nfs_formats::crp::AnimTrack,
    /// `Base`'s sphere in the article's own frame (0x477DB0's `Base+0x2C…`, `+0x3C`)
    centre: Vec3,
    radius: f32,
    /// its [`TrackWorld::track_lods`] entry, when it has draw items
    gpu: Option<usize>,
    /// this frame's matrix (the flares of its `ef` parts go through it, 0x450380)
    matrix: Mat4,
    /// the sphere centre the ref was last registered / compared with (record `+4…+0xC`)
    registered: Vec3,
}

/// One prop playing its own looping animation.
struct AnimatedProp {
    item: usize,
    /// the transform the article is placed with; the sampled frame is applied inside it
    base: Mat4,
    track: nfs_formats::crp::AnimTrack,
    seconds_per_frame: f32,
    looping: bool,
}

/// The `sound_articles` object's own `Anqt` track and `SoundDef`, kept alongside `TrackWorld` so
/// its caller (the game's audio update) can sample the *same* path every tick and feed
/// `nfs_audio::ambience::Ambience::sync_animated` — `engine-audio.md` §7.4's `p1 = obj.EvalPos(tick)`,
/// `p2 = obj.EvalPos(tick + 0x20)`. `ambience_index` is `None` until the caller has registered it
/// with `Ambience::add_animated` (track load and ambience load are two separate steps in the port).
pub struct AnimatedSound {
    pub def: nfs_formats::animdefs::SoundDef,
    pub label: String,
    pub track: nfs_formats::crp::AnimTrack,
    /// CONFIRMED, [`WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME`] for a `WAnimBundle` article without
    /// its own `animdefs.txt` piece — the *same clock* [`TrackWorld::animate`] draws this article
    /// with, so the visible motion and the ambient sound's doppler never drift apart.
    pub seconds_per_frame: f32,
    pub looping: bool,
    pub ambience_index: Option<usize>,
}

impl AnimatedSound {
    /// `p1 = EvalPos(tick)`, `p2 = EvalPos(tick + 0x20)` (§7.4) on this object's own path, in
    /// world units of ticks at the game's 64 Hz.
    pub fn eval_pair(&self, tick: u64) -> ([f32; 3], [f32; 3]) {
        let eval = |t: u64| {
            let (i, j, w) = self.track.sample(t as f32 / 64.0, self.seconds_per_frame, self.looping);
            let (f0, f1) = (&self.track.frames[i], &self.track.frames[j]);
            Vec3::from(f0.position).lerp(Vec3::from(f1.position), w)
        };
        (eval(tick).into(), eval(tick.wrapping_add(0x20)).into())
    }
}

/// A breakable prop drawn in the world (`TrackWorld::break_props`).
struct BrokenProp {
    /// its index in `TrackWorld::props`
    prop: usize,
    /// its own article's draw items, every LOD — drawn while it is whole, those of the LOD the walk
    /// answers its [`Self::slot`] with
    items: std::ops::Range<usize>,
    /// the instance's matrix, `world+0x40 + 56·id` (0x539CD0 at 0x476A54)
    instance: Mat4,
    /// `def+0x80`, already scaled by 0.001
    velocity_factor: f32,
    /// one per piece of the definition
    templates: Vec<PieceTemplate>,
    /// the pieces started (the record's entries `rec+0x34`, `rec+0x14` of them)
    players: Vec<PiecePlayer>,
    /// this frame's draw items of the pieces
    pieces: Vec<DrawItem>,
    /// this frame's sample of the lead player (`rec+0x1C`, piece 0's twin) — slot 5's
    /// `lead sample × instance` (0x4769E0 with −1) for the walk's sphere
    lead: Option<Mat4>,
    state: BreakState,
    /// its [`TrackWorld::instance_culls`] entry (none without a `Base`: drawn always)
    slot: Option<usize>,
}

/// A track smackable's anim-world record (0x4772F0): entry 0 and the lead are the one player
/// 0x471180 on its prop record (0x4775F7), whose sample 0x4711E0 follows the body.
#[derive(Debug, Clone)]
struct SmackAnim {
    /// the `.crp` article index — the record's own article, which `rec+8` takes (0x476876)
    article: usize,
    /// its [`TrackWorld::instance_culls`] entry (none without a `Base`: drawn always, never sampled)
    slot: Option<usize>,
    /// the prop record's position `+0x28…+0x30` — the item record's, where the body started
    home: Vec3,
    /// the body's position now (`car+0x330`, through the pose pointers 0x483590)
    position: Vec3,
    /// the player's state `+0xC`: 0, or 1 once a draw's sample found the body 1 cm off its place
    state: u8,
    /// `rec+8` set (0x476876) — kept for the race
    armed: bool,
}

impl BrokenProp {
    fn hides(&self, item: usize) -> bool {
        !matches!(self.state, BreakState::Whole) && self.items.contains(&item)
    }
}

#[derive(Debug, Clone, Copy)]
enum BreakState {
    Whole,
    /// started on game tick `start`; `lead_done` once the lead player has run out
    Playing { start: u32, lead_done: bool },
    Gone,
}

struct PieceTemplate {
    meshes: Vec<(MeshId, MaterialId)>,
    /// the `BaseGroup` article's `Base` sphere in its own frame (0x4786A0's entry-0 article)
    base: Option<(Vec3, f32)>,
    track: Option<nfs_formats::crp::AnimTrack>,
    seconds_per_frame: f32,
    flags: nfs_formats::animdefs::PieceFlags,
}

/// 0x471490's player: the track's `+0x10` heading (radians here) and `+0x14` drift.
struct PiecePlayer {
    template: usize,
    heading: f32,
    drift: Vec3,
}

/// `AnimPlayer::sample` 0x470E90 with 0x470F70 for a piece `ticks` game ticks after its start:
/// `rate = secondsPerFrame / (1/64)` ticks a frame (`[0x628938]+8`, 0x471700's 0.015625);
/// `f = ticks / rate`; a looping track wraps at `frames − 2`, a one-shot holds `frames − 1`; the
/// quaternion and the position are lerped component by component (0x48A150, **not** normalised),
/// the matrix built from that quaternion as it is (0x48A1E0) with the lerped scale; `DIRECTED`
/// (flag 8) turns it after about y by the heading (0x47108A…0x4710C0, `M·R`), `LANDFLAT` (flag
/// 0x10) before about y by `−heading·(1 − (i + frac)/frames)` (0x4710C3…0x47112C, `R·M`); the
/// translation then gains `drift · ticks · rate` (0x470F17…0x470F51). The answer's flag is
/// `i + 1 < frames`, the player still running.
fn sample_piece(track: &nfs_formats::crp::AnimTrack, seconds_per_frame: f32, flags: nfs_formats::animdefs::PieceFlags, heading: f32, drift: Vec3, ticks: u32) -> (Mat4, bool) {
    let n = track.frames.len();
    if n == 0 {
        return (Mat4::IDENTITY, false);
    }
    let rate = seconds_per_frame / 0.015625;
    let f = ticks as f32 / rate;
    let mut i = f as usize;
    let mut frac = f - i as f32;
    if flags.looping && n > 2 {
        i %= n - 2;
    } else if i + 2 >= n {
        i = n - 1;
        frac = 0.0;
    }
    let (a, b) = (&track.frames[i], &track.frames[(i + 1).min(n - 1)]);
    let lerp = |x: f32, y: f32| x + (y - x) * frac;
    let q = glam::Quat::from_xyzw(lerp(a.rotation[0], b.rotation[0]), lerp(a.rotation[1], b.rotation[1]), lerp(a.rotation[2], b.rotation[2]), lerp(a.rotation[3], b.rotation[3]));
    let pos = Vec3::from(a.position).lerp(Vec3::from(b.position), frac);
    let scale = Vec3::from(a.scale).lerp(Vec3::from(b.scale), frac);
    let mut m = Mat4::from_translation(pos) * Mat4::from_mat3(glam::Mat3::from_quat(q)) * Mat4::from_scale(scale);
    if flags.directed {
        // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable rotation, not glam/std; original rotation builder 0x539BD0 at 0x4710AC / 0x47111D.
        m = nfs_math::portable::mat4_rotation_y(heading) * m;
    }
    if flags.landflat {
        m *= nfs_math::portable::mat4_rotation_y(-heading * (1.0 - (i as f32 + frac) / n as f32));
    }
    let off = drift * ticks as f32 * rate;
    m.w_axis += off.extend(0.0);
    (m, i + 1 < n)
}

/// `[WTrack+0x34]` holds at most **125** smackables: 0x4888B0 stops adding at `size ≥ 0x7D`
/// (0x488926), the mission scene's first (0x487DEE), then the instance bundle's (0x487DFE).
pub const MAX_SMACKABLES: usize = 125;

/// The `.crp` articles 0x4772F0 makes smackables of among the instance bundle's items, in `.crp`
/// order (the bundle's item order): an article with a `tr` (`WInstanceBundle::Load` takes those,
/// 0x47859E) that is not a library template (`Base & 0x8000` — the scene bundle's,
/// `RENDERING.md` "The scene item's draw matrix"; whether a template record is itself a scene
/// item the anim world builds for is not read, so none is taken here), whose `Base` byte 0 lacks
/// 0x40 (0x477377) and whose definition (0x47ED90) says `kSmackable` (0x4773EE). The loader
/// 0x4750B0 builds no collision object for these (0x47533A), so 0x498DE0 never meets them
/// (`suspension-collision.md` §9 "The track's smackables").
pub fn track_smackable_articles(track: &TrackAssets) -> Vec<usize> {
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return Vec::new() };
    arts.iter()
        .enumerate()
        .filter_map(|(k, a)| {
            let g = track.crp.article_geometry(&track.crp_data, a).ok()?;
            let b = g.base.as_ref()?;
            (g.transform.is_some() && !b.is_library() && b.flags & 0x40 == 0 && track.animdefs.lookup(b.tag_u32()).is_some_and(|d| d.collide_type == "kSmackable")).then_some(k)
        })
        .collect()
}

/// The AABB fold 0x486340 (and 0x47B660) run: eight corners of a local box, each carried through
/// a placement matrix, min/maxed per axis. **Read 2026-09-27 (this pass): the source is resolved.**
/// `0x44FE70` builds an 8-byte `{ptr, byte=1, word}` handle whose `ptr` is not per-article (it is
/// the 2nd stack argument of the loader `0x4750B0` itself, read straight off `0x4750B0`'s own
/// frame at both use sites 0x4751F2/0x4752E9 — the same object for every article of the track);
/// the per-article piece is the `word` (the instance id, `esi+0x2e`). `0x486340`'s
/// `call [[handle.ptr]+8]` (0x48639A) — traced to **`0x4893A0`** (found by scanning `.rdata` for
/// the vtable holding `handle.ptr`'s class: `getBBox` sits at that vtable's slot 4, byte +0x10
/// (`0x477F40`; `crp-geometry.md` §2.2's "getTransform slot3 / getBBox slot4" — `0x477DB0` is
/// slot3), so slot 2, byte +8, is a *different* entry, one table row over: `0x4893A0`) — takes the
/// `this`/id and calls `getBBox` on itself (`call [eax+0x10]` at 0x4893BB, i.e. the vtable's own
/// slot 4) to fill two local `f32[3]`s, then combines their six components (x/y/z of each) into
/// the eight corners of the axis-aligned box **those two vectors already are** — no rotation, no
/// scale, in `0x4893A0` itself. `getBBox` (`0x477F40`) in turn reads `Base+0x14` / `Base+0x20` —
/// the article's own **stored** `bbox_min`/`bbox_max` fields (`crp-geometry.md` §2.2, CONFIRMED
/// equal to the LOD0 `vt` min/max for every article of two tracks tested there) — not a vertex
/// walk. So **the article's local box is `Base.bbox_min`/`Base.bbox_max`**, read once per article,
/// not folded from any mesh; the previous pass's "lowest-LOD vertex extent" stand-in was numerically
/// exact (by that same CONFIRMED equality) but attributed to the wrong field. `0x486340`'s own fold
/// (0x486404…0x486478) does not itself carry a rotation matrix — no `fmul`/`fadd` against `tr`
/// appears in it, only `fcomp`/conditional-store — so whatever rotation the placed instance gets is
/// applied by the *caller* of the constructor chain (the `kBox` branch's own footprint transform at
/// 0x475241…0x4752BE calls `0x474CB0` before `0x486C20`, confirming the transform-then-store pattern
/// there; the cylinder/AABB path's own transform site was not pinned further, so the port keeps
/// applying `tr` to the box's eight corners here, matching the general "rotated corners" rule
/// `main.rs`/f8198d01 already settled for the car/world-grid sibling **0x47B660**, and matching this
/// pass's own sweep — see `track_props` below — which found the choice makes no numeric difference
/// for any shipped rotated prop).
fn rotated_local_box_aabb(lo: Vec3, hi: Vec3, tr: Mat4) -> (Vec3, Vec3) {
    let corners = [
        Vec3::new(lo.x, lo.y, lo.z),
        Vec3::new(lo.x, lo.y, hi.z),
        Vec3::new(lo.x, hi.y, lo.z),
        Vec3::new(lo.x, hi.y, hi.z),
        Vec3::new(hi.x, lo.y, lo.z),
        Vec3::new(hi.x, lo.y, hi.z),
        Vec3::new(hi.x, hi.y, lo.z),
        Vec3::new(hi.x, hi.y, hi.z),
    ];
    corners
        .iter()
        .map(|c| tr.transform_point3(*c))
        .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(p), b.max(p)))
}

/// The collision object of one article placed by `tr` — the part [`track_props`] and
/// [`scene_props`] share once each has chosen its article, its `animdefs.txt` definition (`def`) and
/// its placement: `None` when the article has no geometry at its lowest LOD or its `collideType` is
/// neither `kCylinder` nor `kBox` (the callers then skip it, as before). `article` is the `.crp`
/// index the prop carries.
fn build_prop(tags: &[(u32, Option<[u32; 2]>)], g: &nfs_formats::crp::ArticleGeometry, b: &nfs_formats::crp::Base, def: &nfs_formats::animdefs::AnimDef, tr: Mat4, article: usize) -> Option<nfs_physics::props::Prop> {
    use nfs_physics::props::{Breakable, Prop, PropShape, box_footprint};
    let lod = g.primitives.iter().map(|p| p.lod).min()?;
    let mut skipped = 0;
    let local: Vec<Vec3> = article_meshes(g, lod, &mut skipped)
        .iter()
        .flat_map(|m| m.mesh.vertices.iter().map(|v| Vec3::from(v.position)))
        .collect();
    if local.is_empty() {
        return None;
    }
    // The AABB's own source is `Base.bbox_min`/`Base.bbox_max` (getBBox, 0x477F40, reading
    // Base+0x14/+0x20 — `rotated_local_box_aabb`'s doc above), not the lowest-LOD vertex fold
    // this pass replaces; `local` still feeds `box_footprint` below, a separate field.
    let (mn, mx) = rotated_local_box_aabb(Vec3::from(b.bbox_min), Vec3::from(b.bbox_max), tr);
    // 0x476380 finds each piece's `QuatGroup` among the instances by tag (0x478640 / 0x478670)
    // and gives up at one whose `Anim` part is missing or has both words 0 (0x476501…0x47651F)
    let breakable = (!def.pieces.is_empty()).then(|| Breakable {
        bands: def.bands.clone(),
        band_pieces: def.band_pieces.clone(),
        resolved: def
            .pieces
            .iter()
            .map(|(_, p)| first_by_tag(tags, group_tag(&p.quat_group)).and_then(|k| tags[k].1).is_some_and(|w| w != [0, 0]))
            .collect(),
        started: std::cell::Cell::new(None),
        article,
    });
    let shape = match def.collide_type.as_str() {
        "kCylinder" => PropShape::Cylinder { radius: 0.25 },
        "kBox" => {
            let corners = box_footprint(&local).map(|c| tr.transform_point3(c));
            let c = (mn + mx) * 0.5;
            let radius = Vec3::new(mx.x - c.x, 0.0, mx.z - c.z).length() * 1.05;
            PropShape::Box { corners, radius }
        }
        _ => return None,
    };
    let class = if matches!(shape, PropShape::Cylinder { .. }) { def.radius as i32 as u32 } else { 0 };
    Some(Prop { shape, aabb_min: mn, aabb_max: mx, class, breakable, drive_through: def.drive_through, article })
}

/// The track loader's collision objects (0x4750B0 at 0x475152…0x475302): every article of the
/// track's `.crp` whose `Base` byte 0 lacks 0x40 (0x47515A) and whose tag is set, through
/// `AnimDefs::lookup` (0x47ED90 — an unknown tag gets the file's first definition, `ENDW`):
///
/// * `kCylinder` → a cylinder whose radius is the constant **0.25** the loader pushes, with
///   `int(def.radius)` — `_ftol` 0x5A0F98 — where the base keeps the event class `+0x20`
///   (0x4751D5…0x47520B; the arguments are read as the image passes them);
/// * `kBox` → a box on 0x474CB0's footprint of the article's vertices, through its `tr`, class 0
///   (0x475241…0x475302), radius `|(max.x − c.x, 0, max.z − c.z)| × 1.05`.
///
/// The AABB is the axis-aligned box of the article's **local** box's eight corners, each carried
/// through the instance's placement `tr` (rotation and translation) — not the AABB of the
/// individually-transformed mesh vertices (which under-bounds a rotated article, since a rotated
/// point's extreme coordinate need not come from a vertex the local box itself reaches). Read
/// 2026-09-27: both object constructors call the shared base ctor 0x486340 first
/// (`WTreeElem_CylinderObject` 0x486690 at 0x4866AE, `WTreeElem_BoxObject` 0x486C20 at 0x486C42),
/// which calls a vtable+8 slot (0x48639A) that fills an 8×0x10-byte-stride buffer, then folds it
/// into a running per-axis min/max over exactly 8 iterations (0x486404…0x486478) — the identical
/// shape as the world-box fold `main.rs`/f8198d01 already traced at 0x47B6E2…0x47B760 below
/// 0x47B6C6's own vtable+8 call. No inflation follows either fold. The `kBox` shape's own four
/// XZ footprint corners (0x474CB0, [`box_footprint`]) are a separate field the loader builds and
/// transforms the same way (0x475241…0x4752BE) — stored at `+0x34…+0x58`, not fed back into the
/// AABB; only the box's *radius* (`|(max−c).xz| × 1.05`, 0x486C91) reads the AABB `mn`/`mx`.
///
/// **Read 2026-09-27, later (this pass): the local box's own source is resolved** — see
/// [`rotated_local_box_aabb`]'s doc. It is `Base.bbox_min`/`Base.bbox_max` (`getBBox`, 0x477F40,
/// itself called by the vtable+8 function 0x4893A0 above), not `local`'s vertex fold; `local` is
/// kept only for `box_footprint` below, an unrelated field. No shipped prop's `mn`/`mx` changes:
/// swept `coastal` (42 box/cylinder props), `alps` (293) and `monaco1` (64) — every one is
/// bit-identical to the old vertex-derived value (`crp-geometry.md` §2.2's own CONFIRMED equality
/// of `bbox_min`/`bbox_max` to the LOD0 `vt` extent already predicted this).
pub fn track_props(track: &TrackAssets) -> Vec<nfs_physics::props::Prop> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    let tags = article_tags(track, &arts);
    for (index, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
            continue;
        }
        // a `WAnimBundle` / `WMidGroundBundle` article has no `PCollideArticleBundle`
        // (`[bundle+0xC] = 0`, so `0x488090` never calls `0x4750B0` for it, 0x48838B) — no
        // collision object either
        if matches!(track.crp.bundle_of(&track.crp_data, a), Ok(nfs_formats::crp::BundleKind::Anim | nfs_formats::crp::BundleKind::MidGround)) {
            continue;
        }
        // A scene-library article (`'tr'` ∧ `Base.flags & 0x8000`, e.g. `EndWall`, `ExplodingBox`,
        // `Crabcrate_Rigid03`) is a *template* the `.scn` GEOM elements spawn from, not an object of
        // the track: `WInstanceBundle::Load` 0x4781A0 parks it in its auxiliary `+0x34` table, never
        // in the `+0x18`/`+0x1C` pair `0x4750B0` walks (`track_tree_order`, "Library articles are
        // excluded"). Its collision object exists only where a scene element places it
        // ([`scene_props`]). Without this gate `Car_propPass`' "every prop is tried" loop collided
        // with the template at its own `tr` — `EndWall`'s 12 m box at the world origin was an
        // invisible wall across the `0M01` skidpad course (`RE/notes/skill-tests.md` §14, FD-WALL).
        if track.crp.is_scene_member(&track.crp_data, a).unwrap_or(false) {
            continue;
        }
        let Some(def) = track.animdefs.lookup(b.tag_u32()) else { continue };
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        out.extend(build_prop(&tags, &g, b, def, tr, index));
    }
    out
}

/// `WSceneBundle`'s own tree contribution: the scene's `GEOM_ELEMENT`s spawned by `0x46D1D0`
/// (called once at `0x487abd`) → `0x483260` → **0x482EA0** (template by fourcc, `0x478640`; a
/// fourcc miss takes template index 0, `0x482ebe`…`0x482ec0`; at most 64 live slots, `0x482ec2`),
/// placed by the **element's** transform, never the article's `tr` (`0x482BF1`); the `0x4750B0`
/// walk then builds one [`Prop`] for a `kCylinder` / `kBox` template article and nothing for any
/// other `collideType`. Empty on every shipped non-skill-test race
/// (`scene_props_are_empty_on_a_shipped_race`); `ENDW` (`kBox`) appears only in a skill test's
/// `_ST<N>.scn`. The full trace (the cast count `0x46D9C0`, the `DFLT` fallback, the survey of the
/// shipped `.scn` files): `RE/notes/game-history.md` §33.
pub fn scene_props(track: &TrackAssets, geometry: &nfs_assets::track::TrackGeometry) -> Vec<nfs_physics::props::Prop> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    let tags = article_tags(track, &arts);
    for inst in geometry.instances.iter().take(64) {
        let Some(art) = geometry.library.iter().find(|a| a.tag == inst.tag).or_else(|| geometry.library.first()) else { continue };
        let Some(a) = arts.get(art.index) else { continue };
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
            continue;
        }
        // the matched **template's** own tag (`art.tag`), not the element's unmatched fourcc — the
        // fallback substitutes the whole `{tag, Article*}` pair (0x482EA0's own copy), so collision
        // behaviour follows the template that was actually spawned.
        let Some(def) = track.animdefs.lookup(art.tag) else { continue };
        // the element's own placement, never the library article's `tr` ("The scene bundle's
        // items": `0x482BF1` stores the template record as identity/zero).
        let tr = inst.transform * Mat4::from_scale(inst.scale);
        out.extend(build_prop(&tags, &g, b, def, tr, art.index));
    }
    out
}

/// The `insertAll(C)` half of `WTrack+0x24`'s real insertion order (`RE/RENDERING.md` "The tree,
/// built" / "Bundle order, read 2026-09-28"): `0x4750B0` is called once per bundle **that owns a
/// `PCollideArticleBundle`** (`0x488090`, 0x488364…0x48839F: `dynamic_cast` of `[bundle+0xC]`, a null
/// skips) — `WInstanceBundle` (0x47860F), `WStaticBundle` (0x483796) and `WSceneBundle`
/// (0x4830A2, below) — in the order `WInstanceBundle → WStaticBundle` here (`Crp::bundle_of`'s
/// variants; `WAnimBundle` keeps `[+0xC] = 0` from 0x489160 and `WMidGroundBundle::Load` writes it
/// 0 at 0x47CED2 / 0x47CEE7: **neither is walked** — `RE/notes/COVERAGE-2026-09-26.md`
/// §14.80; `WLineBundle` neither), each bundle's claimed articles in `.crp` file order,
/// and within one article either its patches (`0x4751BD`'s `type == 2` path, `TriangleGrid::
/// patches`' own primitive order — one [`nfs_physics::track_tree::TreeItem::Patch`] per patch) or
/// its one `kCylinder`/`kBox` object (`type == 1`, one [`nfs_physics::track_tree::TreeItem::Prop`])
/// — never both, in the original (`0x4751BD`/`0x475238`, read 2026-09-28).
///
/// **Library articles are excluded**, not merely reordered: `WInstanceBundle::Load` (`0x4781A0`)
/// copies a library survivor into its *auxiliary* `+0x34` table, not the generic `+0x18`/`+0x1c`
/// pair `0x4750B0`'s tree-fill walk actually reads (`RE/RENDERING.md` "Bundle order, read
/// 2026-09-28") — so a library article never reaches the tree through `WInstanceBundle` at all.
/// `world` (`TrackGeometry::world`, already the non-library split `TrackAssets::build_geometry`
/// makes) is exactly this port's stand-in for "every article the generic array can hold" — its own
/// `index` is the `.crp` article index [`nfs_physics::collision::Patch::article`] and
/// [`nfs_physics::props::Prop::article`] carry too.
///
/// **`WSceneBundle` is walked separately, first** (corrected 2026-09-28, `scene_props` above):
/// unlike the four bundles this function's own loop covers, its `[+0x1c]` live-slot array is not
/// keyed by `.crp` article file order at all — it is keyed by the *spawn order* of the track's
/// `.scn` `GEOM_ELEMENT`s (`0x46D1D0` over `[0x628168]`, `scene_props`'s own doc has the full
/// address trace), so this function cannot reproduce it from `arts`/`world` the way it does for
/// `Instance`/`Anim`/`MidGround`/`Static`. The caller instead computes [`scene_props`]' own output
/// once, up front, and passes this function the flat `props` indices it landed at (`scene`,
/// below) — this function only has to put them first, matching `0x487abd`'s own call to
/// `0x46D1D0` running before any of the other five bundles' ctor/`Load`/`insertAll(C)` calls
/// (`RE/RENDERING.md` "Bundle order, read 2026-09-28"'s "`WSceneBundle`'s own timing, settled").
/// `WLineBundle` contributes zero items regardless (established, unchanged).
pub fn track_tree_order(
    crp: &nfs_formats::crp::Crp,
    crp_data: &[u8],
    arts: &[nfs_formats::crp::Article],
    world: &[nfs_assets::track::ArticleMeshes],
    patches: &[nfs_physics::collision::Patch],
    props: &[nfs_physics::props::Prop],
    scene: &[usize],
) -> Vec<nfs_physics::track_tree::TreeItem> {
    use nfs_formats::crp::BundleKind;
    use nfs_physics::track_tree::TreeItem;
    let world_indices: std::collections::HashSet<usize> = world.iter().map(|a| a.index).collect();
    let mut patches_by_article: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (i, p) in patches.iter().enumerate() {
        patches_by_article.entry(p.article).or_default().push(i);
    }
    let mut prop_by_article: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for (i, p) in props.iter().enumerate() {
        prop_by_article.insert(p.article, i);
    }
    // `WSceneBundle` first (`0x487abd` runs before any of the other five bundles' `Load`, "Bundle
    // order, read 2026-09-28"'s "`WSceneBundle`'s own timing, settled") — the caller's own spawn
    // order, `scene_props`'s doc.
    let mut order: Vec<TreeItem> = scene.iter().copied().map(TreeItem::Prop).collect();
    // `WAnimBundle` and `WMidGroundBundle` are not walked: no `PCollideArticleBundle` (// `ArticleMeshes::collides`)
    for rank in [BundleKind::Instance, BundleKind::Static] {
        for (index, art) in arts.iter().enumerate() {
            if !world_indices.contains(&index) {
                continue; // a library article: not in the generic array `0x4750B0` walks
            }
            if crp.bundle_of(crp_data, art).ok() != Some(rank) {
                continue;
            }
            // GAP, not invented (see this function's own doc): the original's per-article type
            // dispatch (`0x4751BD`) never gives one article both a patch run and a prop, but this
            // port's `collision_grid` takes every `world` article's mesh into the triangle grid
            // regardless of its `animdefs.txt` collide type, so an article that is both a
            // `pr`-patch source and a `kCylinder`/`kBox` prop in this port's own data (the
            // original has no such article) contributes both here, patches first. No shipped
            // track's race goldens moved from this.
            if let Some(idxs) = patches_by_article.get(&index) {
                order.extend(idxs.iter().map(|&i| TreeItem::Patch(i)));
            }
            if let Some(&i) = prop_by_article.get(&index) {
                order.push(TreeItem::Prop(i));
            }
        }
    }
    order
}

/// Every article's `Base+0x44` tag and its `Anim` part's first two words (the frame count and the
/// vertex-frame count), in `.crp` order — what 0x478640 / 0x478670 search and 0x476380 reads.
fn article_tags(track: &TrackAssets, arts: &[nfs_formats::crp::Article]) -> Vec<(u32, Option<[u32; 2]>)> {
    arts.iter()
        .map(|art| {
            let tag = track.crp.article_geometry(&track.crp_data, art).ok().and_then(|g| g.base.map(|b| b.tag_u32())).unwrap_or(0);
            let anim = art.level(b"Anim").and_then(|e| track.crp.entry_data(&track.crp_data, e).ok()).filter(|d| d.len() >= 8).map(|d| {
                [bytes::read_u32(d, 0), bytes::read_u32(d, 4)]
            });
            (tag, anim)
        })
        .collect()
}

/// 0x478640: the first instance whose tag is `tag` (a linear search, `-1` when none).
fn first_by_tag(tags: &[(u32, Option<[u32; 2]>)], tag: u32) -> Option<usize> {
    tags.iter().position(|t| t.0 == tag)
}

/// A piece's group name as the parser keeps it: four characters upper-cased (0x5A262C at 0x47E8D7)
/// packed into the dword the search compares, 0 when the key is absent.
fn group_tag(name: &Option<String>) -> u32 {
    let Some(n) = name else { return 0 };
    let mut b = [0u8; 4];
    for (k, c) in n.bytes().take(4).enumerate() {
        b[k] = c.to_ascii_uppercase();
    }
    u32::from_le_bytes(b)
}

#[cfg(test)]
mod driver_tests {
    use super::*;

    /// The car draw's calls (the dll's 0x10001090): the driver from the dll's `0x10005094` below
    /// effective class 4, the body from LOD `eff` (`0x100050E0[7·detail + class]`), the wheels from
    /// `0x1000516C[class]` (or the traffic's `0x10005188`), none with the flag.
    #[test]
    fn race_passes_follow_the_tables() {
        let p = |lod, mask| CarPass { lod, mask, flag: false };
        assert_eq!(race_passes(2, 2, false), vec![p(2, 0x80), p(3, 0x50), p(1, 0x20)]);
        assert_eq!(race_passes(3, 0, false), vec![p(3, 0x80), p(4, 0x50), p(2, 0x20)]);
        assert_eq!(race_passes(5, 1, false), vec![p(6, 0x50), p(3, 0x20)]);
        assert_eq!(race_passes(3, 2, true), vec![p(3, 0x80), p(4, 0x50), p(1, 0x20)]);
        // the finer tiers draw a lower LOD for the same class: effective class 2 at tier 3, 1 at tier 4
        assert_eq!(race_passes(3, 3, false), vec![p(2, 0x80), p(3, 0x50), p(2, 0x20)]);
        assert_eq!(race_passes(3, 4, false), vec![p(1, 0x80), p(2, 0x50), p(2, 0x20)]);
        // the driver view: the cabin pass from LOD 2 (tiers 0 … 2) with the flag at class 2 and up,
        // the steering wheel, the hands from tier 2 up
        let f = |lod, mask| CarPass { lod, mask, flag: true };
        let c = |lod, flag| CarPass { lod, mask: 0x10, flag };
        assert_eq!(in_car_passes(0, 0, false), vec![c(2, false), f(1, 0x40)]);
        assert_eq!(in_car_passes(0, 1, false), vec![c(2, false), f(1, 0x40)]);
        assert_eq!(in_car_passes(3, 2, false), vec![c(2, true), f(1, 0x40), f(2, 0x80)]);
        assert_eq!(in_car_passes(3, 2, true), vec![c(2, true), f(1, 0x40)]);
        assert_eq!(in_car_passes(1, 4, false), vec![c(1, false), f(1, 0x40), f(1, 0x80)]);
    }

    /// `nfs5.dll`'s virtual 2-D screen (dll 0x10003840): (640, 480) at 4:3, (480 · r, 480) above,
    /// (640, 640 / r) below.
    #[test]
    fn virtual_screen_follows_the_aspect() {
        assert_eq!(virtual_screen(640.0, 480.0), (640.0, 480.0));
        assert_eq!(virtual_screen(1024.0, 768.0), (640.0, 480.0));
        let (w, h) = virtual_screen(1280.0, 800.0);
        assert!((w - 768.0).abs() < 1e-3 && h == 480.0);
        let (w, h) = virtual_screen(1280.0, 1024.0);
        assert!(w == 640.0 && (h - 512.0).abs() < 1e-3);
    }

    /// `nfs5.dll`'s virtual 2-D width (dll 0x10003840): 640 up to 4:3, `480 · w / h` above.
    #[test]
    fn virtual_width_follows_the_aspect() {
        assert_eq!(virtual_width(640.0, 480.0), 640.0);
        assert_eq!(virtual_width(1024.0, 768.0), 640.0);
        assert_eq!(virtual_width(800.0, 800.0), 640.0);
        assert!((virtual_width(1280.0, 800.0) - 768.0).abs() < 1e-3);
        assert!((virtual_width(1920.0, 1080.0) - 853.3333).abs() < 1e-2);
    }

    /// 0x43C8AC…0x43C8E9: the LOD, `Base+78 & mask & 0xF0`, and the two flag bytes — the hands
    /// (`Base+81` and `+82`) only ever in a flagged call.
    #[test]
    fn a_part_meets_a_pass_by_its_bytes() {
        let hand = PartInfo { lod: 1, mask: 0x81, inside: true, inside_only: true, move_channel: -1 };
        let driver = PartInfo { inside: false, inside_only: false, ..hand };
        let body = PartInfo { lod: 3, mask: 0x1A, inside: true, inside_only: false, move_channel: -1 };
        assert!(!hand.in_pass(&CarPass { lod: 1, mask: 0x80, flag: false }));
        assert!(hand.in_pass(&CarPass { lod: 1, mask: 0x80, flag: true }));
        assert!(driver.in_pass(&CarPass { lod: 1, mask: 0x80, flag: false }));
        assert!(!driver.in_pass(&CarPass { lod: 1, mask: 0x80, flag: true }));
        assert!(body.in_pass(&CarPass { lod: 3, mask: 0x50, flag: false }));
        assert!(!body.in_pass(&CarPass { lod: 3, mask: 0x20, flag: false }));
        assert!(!body.in_pass(&CarPass { lod: 2, mask: 0x50, flag: false }));
    }

    /// 0x445B10's coordinates: `1 / size` on the lateral axis, `aspect / size` on the forward one,
    /// and the offsets `0.5 + 0.1 · lat / 6`, `0.5 + fwd / 6` of the car against its slice centre.
    #[test]
    fn sky_planes_follow_the_layer() {
        let at = |pl: [f32; 4], p: Vec3| pl[0] * p.x + pl[1] * p.y + pl[2] * p.z + pl[3];
        let c = Vec3::new(100.0, 5.0, -40.0);
        let (lat, fwd) = (Vec3::X, Vec3::Z);
        let (u, v) = sky_planes(c, c, lat, fwd, 6.0, 0.5);
        assert!((at(u, c) - 0.5).abs() < 1e-5 && (at(v, c) - 0.5).abs() < 1e-5);
        assert!((at(u, c + Vec3::new(3.0, 7.0, 0.0)) - 1.0).abs() < 1e-5);
        assert!((at(v, c + Vec3::new(0.0, 0.0, 6.0)) - 1.0).abs() < 1e-5);
        let o = c + Vec3::new(6.0, 0.0, 6.0);
        let (u, v) = sky_planes(o, c, lat, fwd, 6.0, 0.5);
        assert!((at(u, o) - 0.6).abs() < 1e-5);
        assert!((at(v, o) - 1.5).abs() < 1e-5);
    }

    /// 0x470E90 for a one-shot piece: 31 frames at `playbackrate=5` run 1.28 ticks a frame and stop
    /// once frame 29 is reached; the drift grows as `ticks · rate`.
    #[test]
    fn piece_sampler_runs_out_and_drifts() {
        use nfs_formats::crp::{AnimFrame, AnimTrack};
        let frames = (0..31).map(|k| AnimFrame { rotation: [0.0, 0.0, 0.0, 1.0], position: [0.0, k as f32, 0.0], scale: [1.0; 3] }).collect();
        let track = AnimTrack { frames };
        let flags = nfs_formats::animdefs::PieceFlags::default();
        let (m, playing) = sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::X, 0);
        assert!(playing && m.w_axis.truncate().length() < 1e-6);
        // 16 ticks = 12.5 frames: y halfway between 12 and 13, x drift 16 · 1.28
        let (m, playing) = sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::X, 16);
        assert!(playing);
        assert!((m.w_axis.y - 12.5).abs() < 1e-4 && (m.w_axis.x - 16.0 * 1.28).abs() < 1e-3, "{m:?}");
        // 29 frames = 37.12 ticks: the last frame, held, and the player done
        let (m, playing) = sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::ZERO, 38);
        assert!(!playing && (m.w_axis.y - 30.0).abs() < 1e-6);
        assert!(sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::ZERO, 37).1);
    }

    /// `AnimatedSound::eval_pair` (`engine-audio.md` §7.4's `p1`/`p2`, the autobahn train's own
    /// ambient-sound path source): a straight-line 5-frame stand-in track, the real
    /// `WANIM_BUNDLE_DEFAULT_*` rate/loop rule — `p2` is one 0x20-tick (0.5 s) sample ahead of `p1`.
    #[test]
    fn animated_sound_samples_p1_and_p2_a_half_second_apart() {
        let frames = (0..5).map(|k| nfs_formats::crp::AnimFrame { rotation: [0.0, 0.0, 0.0, 1.0], position: [k as f32 * 10.0, 0.0, 0.0], scale: [1.0; 3] }).collect();
        let sound = AnimatedSound {
            def: nfs_formats::animdefs::SoundDef { patch_id: 833, range: 300.0, fade_in: 17.0, trigger: 0, fade_out: 0, min_repeat: 0, random_repeat: 0, volume: 100 },
            label: "Locomotive".to_string(),
            track: nfs_formats::crp::AnimTrack { frames },
            seconds_per_frame: WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME,
            looping: WANIM_BUNDLE_DEFAULT_LOOPING,
            ambience_index: None,
        };
        // tick 0: p1 at t=0 (frame 0, x=0), p2 at t=0x20/64=0.5 s = 5 frames in, wraps `% (5-2)=%3`
        let (p1, p2) = sound.eval_pair(0);
        assert_eq!(p1, [0.0, 0.0, 0.0]);
        assert_ne!(p1, p2, "p2 is a genuinely later sample, not a repeat of p1");
        // a full tick later, both samples have advanced
        let (p1b, _p2b) = sound.eval_pair(64);
        assert_ne!(p1, p1b);
    }

    /// 0x438B73…0x438C30: a player walks its table over the duration — frame, next and the
    /// fraction between — and stops, keeping its last frame, once the duration is out.
    #[test]
    fn key_player_walks_its_table() {
        let mut p = KeyPlayer::default();
        p.start(GEAR_GESTURES[0], 1000, 500);
        assert_eq!((p.frame, p.next, p.frac), (0, 10, 0.0));
        // half way: p = 0.5 · 10 = 5 → table[fistp(4.5)] = table[4], table[fistp(5.5)] = table[6]
        p.step(1250);
        assert_eq!((p.frame, p.next), (GEAR_GESTURES[0][4], GEAR_GESTURES[0][6]));
        assert!((p.frac - 1.0).abs() < 1e-6);
        p.step(1499);
        assert!(p.active());
        p.step(1500);
        assert!(!p.active());
        assert_eq!(p.frame, GEAR_GESTURES[0][9]);
    }

    /// Every gesture starts and ends on its steering frame, and the handbrake and reverse tables
    /// meet in the held frame (19 / 7) — the tables as the image has them.
    #[test]
    fn gesture_tables_round_trip() {
        for (i, g) in GEAR_GESTURES.iter().enumerate() {
            assert_eq!((g[0], *g.last().unwrap()), (i as u32, i as u32));
        }
        for i in 0..10 {
            assert_eq!(HANDBRAKE_ON[i][0], i as u32);
            assert_eq!(HANDBRAKE_ON[i][2], 19);
            assert_eq!(HANDBRAKE_OFF[i][0], 19);
            assert_eq!(HANDBRAKE_OFF[i][2], i as u32);
        }
        for i in 0..5 {
            assert_eq!(*REVERSE_ON[i].last().unwrap(), 7);
            assert_eq!(REVERSE_OFF[i][0], 7);
        }
    }
}

#[cfg(test)]
mod cull_tests {
    use super::*;

    fn view() -> CullView {
        CullView { eye: Vec3::ZERO, forward: Vec3::Z, right: Vec3::X, up: Vec3::Y, tan_h: 1.0, tan_v: 0.75, near: 0.5, draw: 400.0 }
    }

    /// 0x539480: a sphere ahead is in, one behind the near plane by more than its radius or past
    /// a side plane by more than its radius is out; a straddling one stays in.
    #[test]
    fn frustum_sphere() {
        let v = view();
        assert!(v.sphere_visible(Vec3::new(0.0, 0.0, 50.0), 1.0));
        assert!(!v.sphere_visible(Vec3::new(0.0, 0.0, -5.0), 1.0));
        assert!(v.sphere_visible(Vec3::new(0.0, 0.0, -5.0), 6.0));
        // 45° half-angle: x = 60 at depth 50 is 10·cos45 ≈ 7.07 outside the right plane
        assert!(!v.sphere_visible(Vec3::new(60.0, 0.0, 50.0), 7.0));
        assert!(v.sphere_visible(Vec3::new(60.0, 0.0, 50.0), 7.1));
        assert!(!v.sphere_visible(Vec3::new(-60.0, 0.0, 50.0), 7.0));
        assert!(!v.sphere_visible(Vec3::new(0.0, 0.0, FRUSTUM_FAR + 10.0), 5.0));
    }

    /// 0x489CC0 and 0x4873B0.
    #[test]
    fn occluder_tests() {
        assert!(segments_cross([-1.0, 5.0], [1.0, 5.0], [0.0, 0.0], [0.0, 10.0]));
        assert!(!segments_cross([-1.0, 15.0], [1.0, 15.0], [0.0, 0.0], [0.0, 10.0]));
        assert!(!segments_cross([0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]));
        let o = Occluder { raised: false, a: [0.0, 0.0], d: [10.0, 0.0], len2: 100.0, inv_len2: 0.01 };
        assert!(!o.far_from([5.0, 3.0], 10.0));
        assert!(o.far_from([5.0, 4.0], 10.0));
        assert!(o.far_from([13.0, 1.0], 9.0));
        assert!(!o.far_from([-2.0, 1.0], 5.0));
    }

    /// 0x47F550: the footprint of a camera looking along +z covers the eye and the far corners.
    #[test]
    fn footprint_bounds() {
        let (c, r2) = view().footprint();
        assert!((c[0]).abs() < 1e-3 && (c[1] - 200.0).abs() < 1e-3);
        assert!((r2 - (400.0f32 * 400.0 + 200.0 * 200.0)).abs() < 1.0);
    }
}

#[cfg(test)]
mod prop_box_tests {
    use super::*;

    /// An unrotated article: the eight-corner fold matches the plain per-vertex AABB (translation
    /// only moves the box, it does not enlarge it).
    #[test]
    fn unrotated_box_matches_vertex_aabb() {
        let (lo, hi) = (Vec3::new(-1.0, 0.0, -2.0), Vec3::new(1.0, 3.0, 2.0));
        let tr = Mat4::from_translation(Vec3::new(10.0, 0.0, -5.0));
        let (mn, mx) = rotated_local_box_aabb(lo, hi, tr);
        assert!((mn - Vec3::new(9.0, 0.0, -7.0)).length() < 1e-5, "{mn:?}");
        assert!((mx - Vec3::new(11.0, 3.0, -3.0)).length() < 1e-5, "{mx:?}");
    }

    /// 0x486340's fold takes the AABB of the local box's **rotated corners**, which for a mesh
    /// whose actual vertices never sit at the same box corner on both axes at once (a diamond, not
    /// a crate) reaches past the AABB of the individually-rotated mesh vertices the port used
    /// before this fix (f8198d01 / 0x47B6E2…0x47B760): the diamond's own vertices, turned 45°
    /// about y, only reach ±half_x = ±2/√2 ≈ ±1.41, but the local **box**'s corner
    /// (half_x, half_z) = (2, 0.5) turned the same way reaches ±(2 + 0.5)/√2 ≈ ±1.77.
    #[test]
    fn rotated_box_reaches_past_the_individually_rotated_vertices() {
        // a diamond: no single vertex sits at the box's corner (±2, ±0.5) on both axes at once,
        // but the box built from their extents (lo=(-2,0,-0.5), hi=(2,1,0.5)) does
        let local = [Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -0.5), Vec3::new(0.0, 1.0, 0.5)];
        let tr = Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let (lo, hi) = local.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        let (mn, mx) = rotated_local_box_aabb(lo, hi, tr);
        let want = (2.0f32 + 0.5) / std::f32::consts::SQRT_2;
        let old = local.iter().map(|p| tr.transform_point3(*p)).fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(p), b.max(p)));
        assert!((mx.x - want).abs() < 1e-4, "{mx:?} want {want}");
        assert!((mx.z - want).abs() < 1e-4, "{mx:?} want {want}");
        assert!((mn.x + want).abs() < 1e-4, "{mn:?} want {want}");
        assert!((mn.z + want).abs() < 1e-4, "{mn:?} want {want}");
        // the y range is untouched by a pure yaw
        assert!((mn.y - 0.0).abs() < 1e-5 && (mx.y - 1.0).abs() < 1e-5);
        // and the corner-based box is strictly looser than the pre-fix per-vertex one
        assert!(mx.x > old.1.x + 0.1, "new {mx:?} old {old:?}");
    }
}

#[cfg(test)]
mod anim_article_tests {
    use super::*;
    use nfs_formats::crp::{AnimFrame, AnimTrack};

    fn track() -> AnimTrack {
        let f = |x: f32, s: f32| AnimFrame { rotation: [0.0, 0.0, 0.0, 1.0], position: [x, 2.0 * x, 3.0], scale: [s; 3] };
        AnimTrack { frames: vec![f(10.0, 1.5), f(20.0, 1.5), f(30.0, 1.5), f(40.0, 1.5)] }
    }

    /// 0x470E90 / 0x470F70: at second 0 the frame-0 pose; half a frame on, the midpoint; the
    /// scale multiplies the rotation's rows (0x5329A0), the position is the fourth row (0x539CD0).
    #[test]
    fn matrix_is_frame_pose_blended_by_the_players_clock() {
        let t = track();
        let m0 = anim_matrix(&t, 0.0, 0.1, true);
        assert!((m0.w_axis.truncate() - Vec3::new(10.0, 20.0, 3.0)).length() < 1e-5);
        assert!((m0.x_axis.truncate() - Vec3::new(1.5, 0.0, 0.0)).length() < 1e-5);
        let m = anim_matrix(&t, 0.05, 0.1, true);
        assert!((m.w_axis.truncate() - Vec3::new(15.0, 30.0, 3.0)).length() < 1e-4);
        // a looping track wraps at `frames − 2` (0.2 s = frame 2 → frame 0)
        let w = anim_matrix(&t, 0.2, 0.1, true);
        assert!((w.w_axis.truncate() - Vec3::new(10.0, 20.0, 3.0)).length() < 1e-5);
    }

    /// 0x48A150 / 0x48A1E0: the quaternion is lerped, not slerped and not renormalised — a half-way
    /// blend of two frames 90° apart about Z is `(0, 0, sin 45°/2, (1 + cos 45°)/2)`-ish and its
    /// matrix is shorter than unit, unlike the slerp's; for a unit quaternion the formula is the
    /// ordinary rotation.
    #[test]
    fn rotation_is_the_lerped_quaternion_through_0x48a1e0() {
        let q = glam::Quat::from_rotation_y(0.7) * glam::Quat::from_rotation_x(0.3);
        let r = quat_rows_0x48a1e0([q.x, q.y, q.z, q.w]);
        let g = glam::Mat3::from_quat(q);
        assert!((r[0] - g.x_axis).length() < 1e-5 && (r[1] - g.y_axis).length() < 1e-5 && (r[2] - g.z_axis).length() < 1e-5);
        let z90 = glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let f = |q: glam::Quat| AnimFrame { rotation: [q.x, q.y, q.z, q.w], position: [0.0; 3], scale: [1.0; 3] };
        let t = AnimTrack { frames: vec![f(glam::Quat::IDENTITY), f(z90), f(z90), f(z90)] };
        let m = anim_matrix(&t, 0.05, 0.1, true);
        // lerp: q = (0, 0, 0.35355, 0.85355) → x row length < 1 (the slerp's would be 1)
        assert!(m.x_axis.truncate().length() < 0.99, "{}", m.x_axis.truncate().length());
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
