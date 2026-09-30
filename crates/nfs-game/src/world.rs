//! Assembling renderer scenes from loaded assets (shared by the game app and the viewer tools).

use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, track::TrackAssets};
use nfs_renderer::{DrawItem, LayerLight, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};
use std::collections::HashMap;

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
    /// Item centres for `nfs_game::skidmarks::SkidMarks::for_world_items` — the same population
    /// [`Self::collision`] turns into triangles ([`nfs_assets::track::TrackGeometry::item_centres`],
    /// `RE/notes/particles.md` §10.1).
    pub skid_items: Vec<(f32, f32)>,
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
    /// The `.map` `ambient`'s mean, clamped 0.2…0.9 — the port's scene ambient for an item that
    /// has no light of its own. APPROXIMATION: not the original's; the track's own items no longer
    /// take it ([`TRACK_LIGHT`]), nor the sky (`Sky::draw_items`, `RE/notes/thrash.md` §6d.5), nor
    /// the cars' blob shadows (0x43B820, read 2026-09-27: its own primitive-helper `EAGL::Material`,
    /// 0x59F070/0x5379F0, carries no light layer at all and no read of any `.map` global — a
    /// constant, given `TRACK_LIGHT` in `push_shadow`). What is left on it in a race: the car
    /// materials with no `R3DLightLayer` of their own — the glass (`CarModel::draw_items_block`'s
    /// `material_light` lookup misses them, `RE/notes/lighting.md` §5.1) — which the original also
    /// shades by `ambient + (1 − ambient) · max(n·sun, 0)` with no light layer of their own.
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
    /// breakables below), plus — since 2026-09-30 — the one world article whose own `Anqt` track
    /// is not gated by `animdefs.txt` at all: autobahn's `"Locomotive"` (`sound_articles`,
    /// [`TrackWorld::upload`]). (The smackables are car records whose player only follows the
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
    /// Per [`Self::cull_articles`] entry: every fitted-cull-tree node `0x47BDC0`/`0x489930`'s
    /// budgeted footprint spread registers it into, each a fixed circle `(cx, cz, r)`, or an empty
    /// list for a `None` article — the node pass `RE/RENDERING.md` "The node pass 0x47F400 and the
    /// fitted tree's split 0x47BE30" describes (an article would survive when *any* of its nodes
    /// does — `0x47F400`'s own tree walk visits a node iff every ancestor's fixed circle also
    /// passes, proven equivalent to a per-node independent check here since a child's circumscribed
    /// circle is always inscribed in its parent's, `COVERAGE` §14.36). Built (from the articles'
    /// own footprint boxes, [`build_article_nodes`]) and kept correct, but the pass itself is not
    /// switched on ([`NODE_PRUNE`]'s own doc has the reason). Census:
    /// `nfs-game/examples/cull_tree_node_prune_census.rs`.
    article_nodes: Vec<Vec<(f32, f32, f32)>>,
    /// The track's own geometry, one entry per placed article (the breakables' apart), each LOD's
    /// draw items uploaded — what the walk's draw picks from per frame ([`Self::visible_items`]).
    /// *(Until 2026-09-26 every article's lowest LOD was merged per material into a few hundred
    /// meshes and clipped by the far plane alone.)*
    pub track_lods: Vec<TrackArticleGpu>,
    /// The `Vis ` segments the walk's occlusion test 0x488B50 uses.
    pub occluders: Vec<Occluder>,
    /// The instance drawer's items outside [`Self::cull_articles`]: one per breakable
    /// (`WInstanceBundle`'s item, drawn apart so that its pieces can replace it) and one per `.scn`
    /// GEOM instance (`WSceneBundle`'s slot) — the walk answers them after the articles
    /// ([`Self::article_draws`]).
    instance_culls: Vec<InstanceCull>,
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
    /// The registration's own footprint box (world xz), [`build_article_nodes`]'s input:
    /// `0x489701`'s "eight box corners" via `getBounds` (vtable slot 2, `0x4893A0`, shared by
    /// every bundle class but `MidGround`) calling `getBBox` (slot 4, `0x477F40`) — `Base`'s own
    /// `bbox_min`/`bbox_max` (`crp.rs`'s `Base::bbox_min`/`bbox_max`, the same `Base+20…+44` the
    /// draw-time sphere sits next to but is not derived from). `MidGround` overrides slot 2 with
    /// `0x47D010`: the same box scaled `×2.25` (`[0x5B4774]`) about its own centre — **not**
    /// `Self::radius`'s `max(2.25·r, 200)` draw-time inflation (`0x47CF00`), a different, per-frame
    /// occlusion/distance quantity RENDERING.md keeps apart from this box.
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
    /// per entry of [`Self::lods`], per draw item: its part's flag word and sphere centre in the
    /// world — what the static drawer queues it by ([`TrackWorld::visible_items`])
    pub parts: Vec<Vec<(u16, Vec3)>>,
}

/// The view's draw lists `[cam+0x54]` (built by 0x466380) that the track's drawers file into,
/// in the order the flush 0x466660 draws them (`RE/RENDERING.md` "The view's lists").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ViewList {
    /// `+4` (0x4668F0): ascending by key, under the alpha test `GREATER 10` (0x46666D) — the parts
    /// with `pr` flag 2
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
}

/// The 3-D camera's far plane as the frustum test 0x539480 has it (`[+0x1EC]`): taken as the
/// projection's **700** (`RE/notes/thrash.md` §6) — MEDIUM, its writer is not traced.
pub const FRUSTUM_FAR: f32 = 700.0;

/// The image's square root, `(bits >> 1) + 0x1FC00000` (0x47F347, 0x47FA72).
fn bit_root(x: f32) -> f32 {
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
    Some(ArticleDraw { lod, value: rounded })
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
        let (fc, fr2) = v.footprint();
        let near: Vec<&Occluder> = self.occluders.iter().filter(|o| !o.far_from(fc, fr2)).collect();
        let articles = self.cull_articles.iter().zip(self.article_nodes.iter()).map(|(a, nodes)| {
            let a = a.as_ref()?;
            if NODE_PRUNE && !nodes.is_empty() && nodes.iter().all(|&(ncx, ncz, nr)| node_wholly_pruned(v, ncx, ncz, nr)) {
                return None;
            }
            walk_item(v, &near, a.drawer, a.centre, a.radius, &a.lods)
        });
        let instances = self.instance_culls.iter().map(|ic| {
            let (c, r) = self.instance_sphere(ic)?;
            walk_item(v, &near, ArticleDrawer::Instance, c, r, &ic.lods)
        });
        articles.chain(instances).collect()
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
    /// `HYDR` / `PSTV` / `TRSH` prop (`nfs-game/examples/ef3anim.rs`). *(Corrected 2026-09-26:
    /// the earlier reading ran it on the start frame alone, "once per hit" — 0x4761FC, taken for
    /// the clear, is in the race-start reset; and until that day kind 3 ran on every drawn frame of
    /// such an article, as kind 2 does, whether struck or not.)* A track smackable's article is
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
        let placed = self.flares.iter().filter(|f| self.effect_runs(draws, f.article, f.part, 2).is_some()).cloned();
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
    let f = |o: usize| f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
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
/// the mid-ground's `max(2.25·r, 200)` (0x47CF00). APPROXIMATION: an anim article's sphere is
/// `Base`'s untransformed (the port does not animate them).
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
            let (drawer, centre, radius, footprint_min, footprint_max) = if let Some(t) = g.transform.as_ref().map(Mat4::from_cols_array) {
                let tr = t.transform_vector3(Vec3::splat(r)).max_element();
                // APPROXIMATION: the original's shared `getBounds` (`0x4893A0`) reads `Base`'s
                // `bbox_min`/`bbox_max` with no transform applied at all (read to the instruction:
                // no matrix multiply anywhere in `0x4893A0`'s body) — every other drawer's `Base`
                // data is already in placed/world coordinates so that is correct there, but an
                // `Instance` article's `Base` box is in the *same* pre-`tr` frame its centre/radius
                // are (`cull_articles`'s own `t.transform_point3(centre)` above), so registering it
                // untransformed would misplace it. The port transforms the box's 8 corners through
                // `t` and takes their axis-aligned min/max instead — direction: this can only ever
                // place the registration box *closer* to the article's true (transformed) position
                // than the original's untransformed one would, so if anything this APPROXIMATION
                // registers into nodes nearer the truth, not further from it; not resolved further
                // (`OBJ1037R`-sized items in practice — `nfs-game/examples/cull_tree_node_prune_census.rs`).
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
                (ArticleDrawer::Instance, t.transform_point3(centre), tr, fmin, fmax)
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

/// One node of the fitted cull tree's own split, `0x47BE30`'s rule (`RE/RENDERING.md` "The node
/// pass 0x47F400 and the fitted tree's split 0x47BE30", re-confirmed `COVERAGE-2026-09-26.md`
/// §14.32.1): a quadrant `q = (x >= cx) + 2*(z >= cz)` gets a child only when the node's level is
/// under 13 and (level <= 1, unconditionally, or the quadrant holds 32 or more item centres).
struct CullNode {
    children: [Option<Box<CullNode>>; 4],
}

fn cull_quadrant_of(min: (f32, f32), max: (f32, f32), q: usize) -> ((f32, f32), (f32, f32)) {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    match q {
        0 => (min, (cx, cz)),
        1 => ((cx, min.1), (max.0, cz)),
        2 => ((min.0, cz), (cx, max.1)),
        _ => ((cx, cz), max),
    }
}

fn cull_tree_build(min: (f32, f32), max: (f32, f32), level: u32, items: &[(f32, f32)]) -> CullNode {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    let mut quad: [Vec<(f32, f32)>; 4] = Default::default();
    for &p in items {
        let q = (p.0 >= cx) as usize + 2 * ((p.1 >= cz) as usize);
        quad[q].push(p);
    }
    let mut children: [Option<Box<CullNode>>; 4] = [None, None, None, None];
    for (q, ch) in children.iter_mut().enumerate() {
        let split = level < 13 && (level <= 1 || quad[q].len() >= 32);
        if split {
            let (qmin, qmax) = cull_quadrant_of(min, max, q);
            *ch = Some(Box::new(cull_tree_build(qmin, qmax, level + 1, &quad[q])));
        }
    }
    CullNode { children }
}

/// `0x47BDC0`: the smallest node's own **fixed** box that still fully holds the footprint box
/// `fmin..fmax`, descending from the root while the node is internal and *exactly one* child fully
/// contains it (`0x47C490`'s inclusive corner test, stopping the count at two): counts the
/// children whose box contains every corner, stops at 2, steps into the child only when the count
/// is exactly 1. A box fully contained by one quadrant of an axis-aligned split cannot also be
/// fully contained by a disjoint sibling, so picking the single candidate quadrant by the
/// footprint's own centre point and checking *that one* for full containment (rather than testing
/// all four, as `0x47BDC0` does) reaches the same node. Read to the instruction (`0x47BDC0`,
/// `0x47C490`'s `flag=1` inclusive branch, `0047bde3…0047be21`); the starting point
/// [`spread_nodes`] spreads out from.
fn cull_tree_locate(node: &CullNode, min: (f32, f32), max: (f32, f32), level: u32, fmin: (f32, f32), fmax: (f32, f32)) -> (&CullNode, (f32, f32), (f32, f32), u32) {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    let (fcx, fcz) = ((fmin.0 + fmax.0) * 0.5, (fmin.1 + fmax.1) * 0.5);
    let q = (fcx >= cx) as usize + 2 * ((fcz >= cz) as usize);
    if let Some(ch) = &node.children[q] {
        let (qmin, qmax) = cull_quadrant_of(min, max, q);
        if fmin.0 >= qmin.0 && fmax.0 <= qmax.0 && fmin.1 >= qmin.1 && fmax.1 <= qmax.1 {
            return cull_tree_locate(ch, qmin, qmax, level + 1, fmin, fmax);
        }
    }
    (node, min, max, level)
}

/// Whether the footprint box `fmin..fmax` overlaps a node's fixed box — `0x5A0790`'s
/// polygon-rectangle SAT test, reduced to a plain AABB overlap for the port's own axis-aligned
/// footprint (see [`spread_nodes`]'s own doc for the APPROXIMATION this stands in for).
fn footprint_overlaps(fmin: (f32, f32), fmax: (f32, f32), min: (f32, f32), max: (f32, f32)) -> bool {
    fmax.0 >= min.0 && fmin.0 <= max.0 && fmax.1 >= min.1 && fmin.1 <= max.1
}

/// `0x489930`: the budgeted footprint spread from `0x47BDC0`'s starting node (`node`/`min`/`max`/
/// `level`), read to the instruction (`00489930`…`00489b97`, jump table `0489b9c`). At an internal
/// node, count the children (of the up to four) the footprint *overlaps* (`n`, `0x5A0790` via
/// [`footprint_overlaps`]) and, by `n` (the jump table `0x489B9C`):
/// * `n = 0` (or a leaf, or an internal node none of whose children overlap) — push this node's own
///   box into the item's list and stop.
/// * `n = 1` — always recurse into the one overlapping child.
/// * `n = 2` or `3` — recurse into every overlapping child only while `weight + spent < 8`
///   (`0047c4b0/9899f8`'s `n-1` jump-table cases 1/2, `0x489B9C+4`/`+8`), else push this node.
/// * `n = 4` — recurse only while this node's own level (`+8`'s top nibble, `0x4899cb…0x4899da`) is
///   under 2, else push this node.
///
/// Each recursion passes `weight + 1` and, for `n >= 2`, `spent + n * weight` (`n = 1` spends
/// nothing, `0x4899dd/0x4899f1`, `0x489a2e…0x489a31`) — the exact weight/`spent` budget the
/// starting point's earlier single-node stand-in never reproduced (
/// `COVERAGE-2026-09-26.md` §14.36).
///
/// The footprint box is `CullArticle::footprint_min`/`footprint_max` — `Base`'s own `bbox_min`/
/// `bbox_max` (`0x477F40`, world xz already for every drawer but `Instance`), the `MidGround`
/// `×2.25` scale folded in by [`cull_articles`] (`0x47D010`), matching `0x489701`'s "eight box
/// corners" directly rather than standing in for them with a sphere-derived square.
///
/// APPROXIMATION, `Instance` drawer only: the original's shared `getBounds` (`0x4893A0`) applies no
/// transform to `Base`'s box at all, but an `Instance` article's `Base` box is in the same pre-`tr`
/// frame as its centre/radius, so the port transforms the box's 8 corners through `tr` and takes
/// their axis-aligned min/max instead of using it raw ([`cull_articles`]'s own doc on
/// `footprint_min`/`footprint_max` has the full reasoning and direction). Every other drawer's box
/// is `Base`'s own field, unapproximated.
fn spread_nodes(node: &CullNode, min: (f32, f32), max: (f32, f32), level: u32, weight: u32, spent: u32, fmin: (f32, f32), fmax: (f32, f32), out: &mut Vec<((f32, f32), (f32, f32))>) {
    let is_internal = node.children.iter().any(Option::is_some);
    if !is_internal {
        out.push((min, max));
        return;
    }
    let mut overlaps = [false; 4];
    let mut n: u32 = 0;
    for (q, ov) in overlaps.iter_mut().enumerate() {
        if node.children[q].is_some() {
            let (qmin, qmax) = cull_quadrant_of(min, max, q);
            if footprint_overlaps(fmin, fmax, qmin, qmax) {
                *ov = true;
                n += 1;
            }
        }
    }
    let recurse = match n {
        0 => false,
        1 => true,
        2 | 3 => weight + spent < 8,
        _ => level < 2, // n == 4
    };
    if !recurse {
        out.push((min, max));
        return;
    }
    let new_spent = if n == 1 { spent } else { spent + n * weight };
    for (q, &ov) in overlaps.iter().enumerate() {
        if ov {
            let (qmin, qmax) = cull_quadrant_of(min, max, q);
            spread_nodes(node.children[q].as_ref().unwrap(), qmin, qmax, level + 1, weight + 1, new_spent, fmin, fmax, out);
        }
    }
}

/// The node pass (`0x47F400`/`0x47F310`) over the fitted tree, with the exact `0x47BDC0`/`0x489930`
/// registration ([`spread_nodes`]). On since 2026-09-30: the alps tick-900 loss
/// that kept it off (verge and mountain vanishing beside the car) was the port's own side-plane
/// test running on a non-unit normal (fixed in [`node_wholly_pruned`], COVERAGE §14.37); with the
/// fix the off/on frames on alps 900, coastal 300, monaco1 900 and autobahn 1400 match
/// pixel-for-pixel in scenery (monaco1 differs only in opponent paint, run-to-run RNG). The
/// census's remaining divergences are the original's own registration gap (un-split quadrants
/// skipped by `0x489930`, COVERAGE §14.36) — the original drops those too.
const NODE_PRUNE: bool = true;
/// Per [`cull_articles`] entry, every node `0x47BDC0`/`0x489930`'s starting-node search plus
/// budgeted spread ([`cull_tree_locate`], [`spread_nodes`]) registers it into, each node's own
/// fixed circle `(cx, cz, r)` (`0x47C2C0`: the node's square box, centre and `0.7071 * width`) —
/// built once, from the articles' own centres and radii, over the same population the original
/// registers into the real tree.
fn build_article_nodes(articles: &[Option<CullArticle>]) -> Vec<Vec<(f32, f32, f32)>> {
    let centres_xz: Vec<(f32, f32)> = articles.iter().flatten().map(|a| (a.centre.x, a.centre.z)).collect();
    if centres_xz.is_empty() {
        return articles.iter().map(|_| Vec::new()).collect();
    }
    let (mut min, mut max) = ((f32::INFINITY, f32::INFINITY), (f32::NEG_INFINITY, f32::NEG_INFINITY));
    for &(x, z) in &centres_xz {
        min = (min.0.min(x), min.1.min(z));
        max = (max.0.max(x), max.1.max(z));
    }
    let (dx, dz) = (max.0 - min.0, max.1 - min.1);
    let l = dx.max(dz).max(1.0);
    let root_min = (min.0 - (l - dx) * 0.5, min.1 - (l - dz) * 0.5);
    let root_max = (root_min.0 + l, root_min.1 + l);
    let tree = cull_tree_build(root_min, root_max, 0, &centres_xz);
    articles
        .iter()
        .map(|a| {
            let Some(a) = a.as_ref() else { return Vec::new() };
            let fmin = (a.footprint_min.x, a.footprint_min.z);
            let fmax = (a.footprint_max.x, a.footprint_max.z);
            let (start, smin, smax, slevel) = cull_tree_locate(&tree, root_min, root_max, 0, fmin, fmax);
            let mut boxes = Vec::new();
            spread_nodes(start, smin, smax, slevel, 1, 0, fmin, fmax, &mut boxes);
            boxes.into_iter().map(|(nmin, nmax)| ((nmin.0 + nmax.0) * 0.5, (nmin.1 + nmax.1) * 0.5, 0.7071 * (nmax.0 - nmin.0))).collect()
        })
        .collect()
}

/// `0x47F310`: is a node's fixed circle `(ncx, ncz, nr)` **wholly outside** the ground wedge or the
/// far circle — pruned before any per-item test runs? `far`/the half-angle are the same
/// `v.draw`/`v.tan_h` the port's own per-item test already uses (`RE/RENDERING.md` "The footprint —
/// 0x47F550/0x47F210": both this test's and the frustum's angle and far distance are literally
/// `cam+0x130`/`cam+0x124` = `[0x6197CC]`, one and the same camera quantity). The steep-look
/// special case is carried too: `|forward.y| > 0.8` (`[0x5B37B8]`, 0x47F76B) sets `far = 50` and
/// skips the side test entirely (`this+0xA8 = 1`).
fn node_wholly_pruned(v: &CullView, ncx: f32, ncz: f32, nr: f32) -> bool {
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
            let w = |k: usize| u16::from_le_bytes([bytes[o + k], bytes[o + k + 1]]);
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
            if d.len() < 0x48 || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != 4 {
                continue;
            }
            let f = |i: usize| {
                let o = 8 + 4 * i;
                f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
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
            if d.len() < 0x34 || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != 3 {
                continue;
            }
            let f = |o: usize| f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
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
                tag: u32::from_le_bytes([d[0x30], d[0x31], d[0x32], d[0x33]]),
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
        if d.len() < 0x2C || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != 2 {
            continue;
        }
        let f = |o: usize| f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
        let w = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
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
            .map(|t| r.upload_texture(&TextureData { width: t.width, height: t.height, mips: t.mips.clone() }))
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
        let mut tex_ids = HashMap::new();
        for tex in &track.textures {
            let id = r.upload_texture(&TextureData { width: tex.width, height: tex.height, mips: tex.mips.clone() });
            tex_ids.insert(tex.name.clone(), (id, tex.has_alpha));
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
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp, lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: true, view_cull: true, depth_bias, false_sort })
            })
        };
        // The track's own geometry: the walk 0x47F890 draws each article on its own through its
        // drawer with the LOD the distance picks (`RE/RENDERING.md` "The track draw's culling"), so
        // every placed article keeps its own meshes per LOD and per material, at its own transform.
        // *(Until 2026-09-26 the lowest LODs were merged per material — a few hundred meshes clipped
        // by the far plane alone; the original's mid-ground reaches 600 / 700 m past the draw
        // distance and the near articles switch LOD.)*
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
        let mut track_lods: Vec<TrackArticleGpu> = Vec::new();
        for art in &geometry.world {
            if breakable_articles.contains(&art.index) || smackable_articles.contains(&art.index) || sound_articles.contains(&art.index) {
                continue;
            }
            let lods = art
                .lods
                .iter()
                .map(|(lod, meshes)| {
                    (*lod, meshes.iter().map(|mm| DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), art.transform)).collect())
                })
                .collect();
            let parts = art.lods.iter().map(|(_, meshes)| meshes.iter().map(|mm| (mm.flags, art.transform.transform_point3(Vec3::from(mm.centre)))).collect()).collect();
            track_lods.push(TrackArticleGpu { article: art.index, lods, parts });
        }
        let mut items = Vec::new();
        let mut instance_culls: Vec<InstanceCull> = Vec::new();
        let mut item_cull: Vec<Option<(usize, u8)>> = Vec::new();
        let broken = Self::breakables(r, track, &geometry, &props, &mut items, &mut instance_culls, &mut item_cull, &mut material_for);
        // each track smackable's article: an instance item of its own (the walk answers it through
        // 0x4786A0 with the body's pose, 0x450010 picks the LOD from its own records), every LOD
        // uploaded, drawn at `pose · home⁻¹ · tr` ([`Self::place_smackable`])
        let mut moved: HashMap<SmackableOwner, (Mat4, Vec<(usize, Mat4)>)> = HashMap::new();
        let mut smack_anims: Vec<SmackAnim> = Vec::new();
        {
            let arts = track.crp.articles(&track.crp_data).unwrap_or_default();
            for sp in &track_smackables {
                let SmackableOwner::Article(k) = sp.owner else { continue };
                let Some(own) = geometry.world.iter().find(|a| a.index == k) else { continue };
                let slot = arts.get(k).and_then(|a| article_base(track, a)).map(|(centre, radius, lods)| {
                    instance_culls.push(InstanceCull { centre, radius, lods, first_item: items.len(), broken: None });
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
        }
        // The `sound_articles` object above ("Locomotive"/`tag=TRAN`), drawn through
        // [`AnimatedProp`]/[`TrackWorld::animate`] like a looping library piece, but with its own
        // `Anqt` track directly (`base = IDENTITY`: the track's frames are already world-space —
        // `frame 0`/`frame 250` land on the same point, a closed loop) and no `.scn` instance.
        // CONFIRMED (2026-10-01, `COVERAGE-2026-09-26.md` §14.42 — settles the
        // `572752fc`'s open APPROXIMATION): the *default* `secondsPerFrame`/loop rule a
        // `WAnimBundle` article gets when it has no `animdefs.txt SECTION:ANIM` piece of its own.
        // `WAnimBundle::Load` 0x4777F0, at 0x4779DF..0x4779F7, constructs one player per claimed
        // article with `0x471490(this, frames, AnqtPtr, vertexFlag=0, flags=[0x5B45A4], 0.1f,
        // heading=0)` — `frames`/`AnqtPtr` from the article's own `Anqt` *part* record's `b`
        // (element count) and (flag-adjusted) `rel_offset` fields (`crp-geometry.md` §2, not a
        // separate `Anim` part — `WAnimBundle::Load` never searches for one), the literal float
        // `0x3DCCCCCD` (**0.1 s/frame, 10 fps** — not `0.1/playbackrate`: there is no piece to read
        // a `playbackrate` from) and the *global constant* read from `[0x5B45A4]` — read directly
        // off the shipped `.exe`'s `.rdata`, **= 1** — as the flags word `AnimPlayer::sample`
        // 0x470E90 tests bit 0 of for its loop flag: **always looping**, unconditionally, not a
        // per-article choice (unlike the two dockers below, whose loop bit and rate come from a
        // real `animdefs.txt SECTION:PIECE`'s `properties=LOOP` / `playbackrate`).
        // *(An earlier pass, `COVERAGE-2026-09-26.md` §14.40, read `0x471490` as the ctor of the
        // already-`CONFIRMED`-inert `SmackAnim` pose cache (`crp-geometry.md` §7.0b) and left the
        // rate/loop an APPROXIMATION on that basis. That was a misidentification, corrected this
        // pass: `0x471490` sets `[this] = 0x5B460C` — the *player* vtable `suspension-collision.md`
        // already names for the breakable-piece players (`RENDERING.md` "the player vtable
        // 0x5B460C's slot 8 is 0x471150 `fld 0.0`" — matches slot 8 of `0x5B460C` exactly), the
        // same class `AnimPlayer::sample` 0x470E90/0x470F70 already fully specifies
        // (`crp-geometry.md` §7) — not `SmackAnim`'s distinct vtable `0x5B45E4`.)*
        // The player is *started* once, immediately after construction, by `WAnimBundle::Load`
        // itself calling the player's slot 5 (`0x4716E0`, `start()`: `state=1`,
        // `start_tick=gGameTick`) at 0x477A0E — at **track load** (`main`'s per-session
        // `0x468FB0`, `replay.md` §10.1), which runs once and *before* every race "pass"'s own
        // `0x414EB0` zeroes `gGameTick` (`[0x606AC4]`). Since the player's per-draw sample
        // (`WAnimBundle`'s own slot 5, `getInstanceTransform` 0x4777B0 → the player's slot 1
        // 0x4715A0 → `AnimPlayer::sample` 0x470E90) clamps `ticks = max(0, gGameTick −
        // start_tick)` to ≥0, every race begins with `ticks = 0` regardless of `start_tick`'s
        // stale value — **phase 0 at every race start** — exactly what the port's own `self.tick =
        // 0` at race start (`porsche/src/main.rs`) already gives `animate(self.tick as f32 /
        // 64.0)`/`eval_pair(self.tick)` below. `0x4777B0` is called once per **draw** of the
        // article (no separate per-tick "advance" step exists for it), so its value only changes
        // once every simulation tick (`[0x628938]+8` = the 64 Hz `globalStep`, `1/64`) — driven by
        // the **global race clock**, not a draw-count or a wall clock.
        let mut animated: Vec<AnimatedProp> = Vec::new();
        let mut animated_sound: Option<AnimatedSound> = None;
        for art in geometry.world.iter().filter(|a| sound_articles.contains(&a.index)) {
            let Some(def) = track.animdefs.sound_by_fourcc(art.tag) else { continue };
            let Some(anim) = art.animation.clone() else { continue };
            let drawn = &art.lods[..art.lods.len().min(1)];
            for (_, meshes) in drawn {
                for mm in meshes {
                    items.push(DrawItem::new(r.upload_mesh(&mm.mesh), material_for(r, mm.material), Mat4::IDENTITY));
                    item_cull.push(None);
                    animated.push(AnimatedProp {
                        item: items.len() - 1,
                        base: Mat4::IDENTITY,
                        track: anim.clone(),
                        seconds_per_frame: WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME,
                        looping: WANIM_BUNDLE_DEFAULT_LOOPING,
                    });
                }
            }
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
        // every LOD of each library article, by tag (the first article with it, as 0x478640 finds
        // the template); every shipped one has a single LOD
        let mut lib_gpu: HashMap<u32, Vec<(u8, Vec<(MeshId, MaterialId)>)>> = HashMap::new();
        for art in &geometry.library {
            if lib_gpu.contains_key(&art.tag) {
                continue;
            }
            let lods = art.lods.iter().map(|(lod, meshes)| (*lod, meshes.iter().map(|mm| (r.upload_mesh(&mm.mesh), material_for(r, mm.material))).collect())).collect();
            lib_gpu.insert(art.tag, lods);
        }
        let arts = track.crp.articles(&track.crp_data).unwrap_or_default();
        let mut item_element = vec![None; items.len()];
        let mut missing_tags = std::collections::HashSet::new();
        let mut smackables: Vec<SmackableSpawn> = Vec::new();
        let mut instance_flares: Vec<(usize, TrackFlare)> = Vec::new();
        for inst in &geometry.instances {
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
                // and scene items"). *(Until 2026-09-26 the port applied the `tr` too — the cones
                // at 0.585 of their size.)*
                let world_tr = inst.transform * Mat4::from_scale(inst.scale);
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
                    instance_culls.push(InstanceCull { centre, radius, lods: clods, first_item: items.len(), broken: None });
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
        // every `.env` entry, keyed by its SHPI name as the `.map` sections spell it
        let mut env_by_id = HashMap::new();
        for t in &track.env_textures {
            let id = r.upload_texture(&TextureData { width: t.width, height: t.height, mips: t.mips.clone() });
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
        let flags: Vec<u32> = materials.iter().map(|m| m.flags).collect();
        let collision = geometry.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
        // the skid marks' fitted cell tree's own item population (`nfs_game::skidmarks`'s module
        // doc, `RE/notes/particles.md` §10.1) — the same items `collision` above turns into
        // triangles, kept here as centres for `SkidMarks::for_world_items`
        let skid_items = geometry.item_centres();
        // *(Until 2026-09-28 a skill test's `ENDW` barriers also joined this wall set as
        // synthesized footprint-edge segments — a pre-2026-09-28 port addition, not `.edg` data,
        // invented before `WSceneBundle`'s own tree contribution ([`scene_props`]) was read. That
        // duplicated the barrier: one course-closing box per `ENDW` element in the wall set *and*
        // one `kBox` [`nfs_physics::props::Prop`] in the tree for the same element. Removed —
        // `scene_props`'s tree entry is now the barrier's only collision object, matching the
        // original's one-object-per-spawned-element rule (`0x4750B0`'s per-article dispatch).)*
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
        let article_nodes = build_article_nodes(&cull_articles);
        let occluders = track_occluders(track);
        tracing::info!(articles = cull_articles.iter().flatten().count(), drawn = track_lods.len(), lods = track_lods.iter().map(|a| a.lods.len()).sum::<usize>(), occluders = occluders.len(), instances = instance_culls.len(), "track culling");
        Ok(Self {
            items,
            item_element,
            hidden: std::collections::HashSet::new(),
            env_map,
            collision,
            skid_items,
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
            track_lods,
            occluders,
            instance_culls,
            instance_flares,
            item_cull,
            uv_animate,
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
    pub fn animate(&mut self, seconds: f32) {
        for a in &self.animated {
            let (i, j, t) = a.track.sample(seconds, a.seconds_per_frame, a.looping);
            let (f0, f1) = (&a.track.frames[i], &a.track.frames[j]);
            let q0 = glam::Quat::from_xyzw(f0.rotation[0], f0.rotation[1], f0.rotation[2], f0.rotation[3]).normalize();
            let q1 = glam::Quat::from_xyzw(f1.rotation[0], f1.rotation[1], f1.rotation[2], f1.rotation[3]).normalize();
            let rot = q0.slerp(q1, t);
            let pos = Vec3::from(f0.position).lerp(Vec3::from(f1.position), t);
            let scale = Vec3::from(f0.scale).lerp(Vec3::from(f1.scale), t);
            let local = Mat4::from_scale_rotation_translation(if scale.min_element() > 1e-4 { scale } else { Vec3::ONE }, rot, pos);
            if let Some(item) = self.items.get_mut(a.item) {
                item.transform = a.base * local;
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
        let ticks = ticks128 as f32;
        for it in self.items.iter_mut().chain(self.track_lods.iter_mut().flat_map(|a| a.lods.iter_mut().flat_map(|(_, v)| v.iter_mut()))) {
            let Some(a) = self.uv_animate.get(&it.material) else { continue };
            it.uv_offset = match a.mode {
                0 => {
                    let t = ticks * a.rate as f32 * 1e-4;
                    [0.0, t - t.floor()]
                }
                2 => {
                    let th = ticks * a.rate as f32 * 2e-4;
                    [(th * std::f32::consts::TAU).sin() * 0.06, 0.0]
                }
                // not read (render-core.md §4.4): no shipped material uses another mode
                _ => [0.0, 0.0],
            };
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
    /// **The order** is the view's lists' (`RE/RENDERING.md` "The view's lists", read 2026-09-26):
    /// the static / mid-ground drawer 0x450DE0 queues each `pr` part of the chosen LOD on its own
    /// (0x450EDB…0x450FCF) keyed by `round(d²)`, `d` from the eye (`[[0x628130]+0x10]+0x34`) to the
    /// part's sphere centre (`pr+8`, 0x450EF8…0x450F25) — a flag-2 part in the list `+4`, drawn
    /// first and **nearest first**, a flag-1 or flag-4 part in `+0xC`, drawn after and **farthest
    /// first** ([`part_list`]); the instance drawer queues its whole item in `+0xC` with the walk's
    /// `round(v)` (0x44FFC0 → 0x466A90), and 0x450010 draws its parts together there. Equal keys keep
    /// the order they were queued in (each tree insert goes right on equal, 0x466933 / 0x466AD3;
    /// `0x466A90`'s walk compares the `fistp`-rounded `u32` key **unsigned**, `jbe`/`ja` at
    /// 0x466ad3/0x466aed — never signed, though every shipped key is non-negative regardless).
    /// So the flag-2 parts — every `Opaque` one, but also 7 000 blended `Normal` / `NormMatState` /
    /// `FullAmbient` / `FakeSort` ones — are drawn front to back before anything of `+0xC`: they come
    /// first here and are marked `early` (the renderer's opaque stage, in this order); the `+0xC`
    /// ones follow, farthest first, in the blended stage — merged now with the cars' and the skid
    /// marks' own `+0xC` entries by [`App::rebuild_scene`] ([`Self::track_draw_lists`]).
    /// APPROXIMATIONS: equal keys keep the port's article order, not the node pass's (the fitted
    /// tree is not built); an item with no `Base` (port fallback) goes last.
    pub fn visible_items(&self, draws: &[Option<ArticleDraw>], eye: Vec3) -> Vec<DrawItem> {
        let (mut near, mut far, rest) = self.track_draw_lists(draws, eye);
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
    pub fn track_draw_lists(&self, draws: &[Option<ArticleDraw>], eye: Vec3) -> (Vec<(u32, DrawItem)>, Vec<(u32, DrawItem)>, Vec<DrawItem>) {
        let key = |c: Vec3| (c - eye).length_squared().round_ties_even() as u32;
        let mut near: Vec<(u32, DrawItem)> = Vec::new();
        let mut far: Vec<(u32, DrawItem)> = Vec::new();
        let mut rest: Vec<DrawItem> = Vec::new();
        // the walk's draw of the track geometry: each drawn article at the LOD its drawer picked
        // (0x450DE0 / 0x450010 — the record's `pr` range is the parts of that LOD number); an
        // article the walk has no record of (no `Base`) is drawn at its lowest LOD (port fallback)
        for a in &self.track_lods {
            match self.cull_articles.get(a.article).map(|c| c.as_ref()) {
                Some(Some(c)) => {
                    let Some(d) = draws.get(a.article).copied().flatten() else { continue };
                    let Some(lod) = c.lods.get(d.lod).map(|l| l.lod) else { continue };
                    let Some(k) = a.lods.iter().position(|(n, _)| *n == lod) else { continue };
                    let parts = a.parts.get(k).map(|p| p.as_slice()).unwrap_or(&[]);
                    for (i, item) in a.lods[k].1.iter().enumerate() {
                        let (flags, centre) = parts.get(i).copied().unwrap_or((2, Vec3::ZERO));
                        // `FakeSortMode::PerItem`'s fold (0x449AB2, `[0x61975C] < 0x2710`
                        // unsigned): the port reuses this same `round(d²)` view-list key as the
                        // item's own distance test — see `DrawItem::fakesort_near`'s doc
                        match c.drawer {
                            ArticleDrawer::Static | ArticleDrawer::MidGround { .. } => match part_list(flags) {
                                Some(ViewList::Near) => {
                                    let k = key(centre);
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
        // the instance drawer's own items (the `.scn` GEOM instances): each at the LOD the walk
        // answered its entry with, none when it did not ([`Self::article_draws`]), queued with the
        // walk's value
        let n = self.cull_articles.len();
        for (k, (item, e)) in self.items.iter().zip(self.item_element.iter()).enumerate() {
            if e.is_some_and(|e| self.hidden.contains(&e)) || self.broken.iter().any(|b| b.hides(k)) {
                continue;
            }
            match self.item_cull.get(k).copied().flatten() {
                Some((slot, lod)) => {
                    let Some(d) = draws.get(n + slot).copied().flatten() else { continue };
                    if self.instance_culls.get(slot).and_then(|ic| ic.lods.get(d.lod)).is_some_and(|l| l.lod == lod) {
                        far.push((d.value as u32, *item));
                    }
                }
                None => rest.push(*item),
            }
        }
        // a breakable's pieces are the entries 0x450010 draws when the walk answers the item
        for b in &self.broken {
            match b.slot {
                Some(k) => {
                    if let Some(d) = draws.get(n + k).copied().flatten() {
                        far.extend(b.pieces.iter().map(|i| (d.value as u32, *i)));
                    }
                }
                None => rest.extend(b.pieces.iter().copied()),
            }
        }
        (near, far, rest)
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
        items: &mut Vec<DrawItem>,
        culls: &mut Vec<InstanceCull>,
        item_cull: &mut Vec<Option<(usize, u8)>>,
        material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId,
    ) -> Vec<BrokenProp> {
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
                culls.push(InstanceCull { centre, radius, lods, first_item: items.len(), broken: Some(out.len()) });
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
                let heading = local.x.atan2(local.z);
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
/// *(Until 2026-09-26 the port folded the `tr` into the points.)* MEDIUM: the GEOM scale is folded
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
    Some(SmackableSpawn {
        owner: SmackableOwner::Element(inst.element),
        position: pos,
        orientation: glam::Mat3::from_quat(rot).transpose(),
        points: nfs_physics::smackable::point_cloud(&verts),
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
    let points = if def.tag.eq_ignore_ascii_case("SIGN") { nfs_physics::smackable::sign_point_cloud(&verts, lengths) } else { nfs_physics::smackable::point_cloud(&verts) };
    let unit = |v: Vec3| v.normalize_or_zero();
    Some(SmackableSpawn {
        owner: SmackableOwner::Article(art.index),
        position: tr.w_axis.truncate(),
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
        m = Mat4::from_rotation_y(heading) * m;
    }
    if flags.landflat {
        m *= Mat4::from_rotation_y(-heading * (1.0 - (i as f32 + frac) / n as f32));
    }
    let off = drift * ticks as f32 * rate;
    m.w_axis += off.extend(0.0);
    (m, i + 1 < n)
}

/// GPU-resident car model at one LOD, split into body parts (with their pivots applied) and
/// wheels (kept separate so they can spin/steer). Model space (RE/notes/car-model.md §3) is
/// +Y up, −Z front, +X left; `MODEL_TO_BODY` rotates it into the physics body frame (+Z forward).
///
/// `Clone` is cheap: every field but the GPU ids (`MeshId`/`MaterialId`/`TextureId`, plain indices)
/// is small metadata or a `Vec` of them — no GPU memory is duplicated by cloning alone. Used by
/// [`Self::instantiate`].
#[derive(Clone)]
pub struct CarModel {
    /// The model's scale in its render matrix: 1, or **0.4** for a `.TINYCAR` (`[0x5B37C8]`, the
    /// scale 0x4433D2 builds into the car's matrix through 0x539B30)
    pub scale: f32,
    /// A mirrored race (`[0x65742C]`, the race option `MIRROR`): the car draw 0x4433A1…0x4433F8
    /// builds the model's x scale as `−m·s` with `m = −1` there (0x44339B) — the model flipped
    /// back across the body's x, so under the mirrored view a car still reads the right way round
    /// (plates, the driver's side) — and slot 9 negates the steer channels 1 / 2 `.y` (0x438471).
    pub mirror: bool,
    pub body: Vec<(MeshId, MaterialId, Mat4)>,
    /// Each material's `R3DLightLayer` parameters (`RE/notes/lighting.md` §6) — every shipped car
    /// material has them.
    material_light: HashMap<MaterialId, nfs_formats::crp::LayerLight>,
    /// The materials whose render method takes the shadow projector (layer class 0x8102,
    /// `R3DShadowMapTextureLayer`).
    shadowed: std::collections::HashSet<MaterialId>,
    /// The meshes each **model animation** morphs (`Base+77`, index 1…7), with every frame's
    /// positions: 1 the convertible's roof, 2 the `SpoilerW` wing, 3 the wipers, 4 / 5 / 6 the
    /// driver's right arm, left arm and body, 7 the aerial (`anims` example over every car).
    morph: [Vec<MorphPart>; ANIMS],
    /// `0x438190(n)`: animation `n`'s length in frames, 0 for a car without it
    anim_length: [u32; ANIMS],
    /// the time each animation was last set to (0x4380D0), so an unchanged one is not re-uploaded
    anim_time: [f32; ANIMS],
    roof_state: i32,
    driver: DriverAnim,
    /// the `{frame, next, frac}` each animation's meshes were last blended to
    blended: [Option<(usize, usize, f32)>; ANIMS],
    /// The **rotation channel** of each `body` entry — the article's `Base+75`, 0 for a part that
    /// never moves (`car-model.md`). `R3DCarLOD::draw` picks it at 0x43C98A and turns
    /// `channel_turns[n]` into `Rx·Ry·Rz` about the part's own origin.
    body_channel: Vec<u8>,
    /// The **draw order** of each `body` entry — its article's place in its LOD's part list after
    /// 0x43C1B0's sort. 0x43C1B0 lists a LOD's parts as `{article, lod, Base+70}` in file order and
    /// `std::sort`s them by the byte at `+8`, signed, `a < b` (0x43C35B → 0x43E710 / 0x43E910 /
    /// 0x43EA20, `cmp bl, cl` / `jge`), and `R3DCarLOD::draw` 0x43C830 walks them in that order
    /// (the `lod_list` of the car's load, sorted by [`nfs_formats::stl_sort::vc6_sort`]). On the
    /// 993 the wheels are 0x0A and the wheel shadows, the `Shadow.fsh` quads over the wheels'
    /// outer faces, 0x0B — drawn after the wheel they darken.
    body_order: Vec<u32>,
    /// Each `body` entry's LOD and pass bytes ([`PartInfo`]): the model holds **every** LOD, and
    /// the car draw 0x444E00 picks per pass which one a part is drawn from ([`CarPass`]).
    body_info: Vec<PartInfo>,
    /// `R3DCarLOD+0x10` of each LOD 1…7 — how many parts 0x43C1B0 listed for it (the admitted
    /// articles with a `Base` LOD record for that LOD); the LOD pick 0x43A0DC…0x43A117 stops at
    /// the first empty one
    lod_parts: [u32; 7],
    /// `R3DCarLOD+0x14 + 4m` of each LOD: whether a part of it rides translation channel `m`
    /// (`Base+72`, 0x43C66D…0x43C67C) — the gate 0x43E670 writes channel `m` behind
    lod_moves: [[bool; 4]; 7],
    /// `.tpg` `movespoilerextent` → `R3DCar+0x74` (0x441D08), in **turns**: −0.04 on the 996 and
    /// the GT3, −0.08 on the two Boxsters, and absent everywhere else.
    move_spoiler_extent: f32,
    /// `.tpg` `headlightextent` → `R3DCar+0x70` (0x441CF7), in **turns**: 0.19 on the 914, −0.18
    /// on the 928, 0.14 on the 944 — the three Porsches with pop-up headlights, and no others.
    headlight_extent: f32,
    spoiler: ChannelEase,
    headlight: ChannelEase,
    /// `.tpg` `dooraxis` → `R3DCar+0x2B0` (0x441C92), default (0, 1, 0): the axis channels 6 and 7
    /// turn about, x negated for 7 (0x43CA5A…0x43CA7D)
    door_axis: Vec3,
    /// `.tpg` `hoodextent` → `+0x68` (0x441CCC), default **0.15** turn; `trunkextent` → `+0x6C`
    /// (0x441CE6), default **−0.15** — the targets of the two lid easers
    hood_extent: f32,
    trunk_extent: f32,
    /// the lid easers at `car+0x64 + 0x80` (hood) and `+ 0x90` (engine lid) that slot 9 writes
    /// into channels 8 and 9 (0x4387CE…0x43891C)
    hood: ChannelEase,
    trunk: ChannelEase,
    /// `object + 0x2DC + 12n` — three floats a channel, **rotations in turns** about x, y and z
    /// (0x43C9C0 / 0x43C9D7 / 0x43C9F8 each multiply by `[0x5B25CC]` = 360.0 before the
    /// axis-angle build 0x539BD0). Only the x column is used so far: channel **16** is
    /// `MoveSpoiler` and **17** the pop-up `HeadLight`.
    pub channel_turns: [f32; 32],
    /// the same channels' `y` and `z` turns — the wheels' steer and camber, the steering wheel's
    /// and the dial needles' `z`, the siren's `y` (slot 9, `car-model.md` "Slot 9")
    channel_yz: [[f32; 2]; 32],
    /// `nfs5.car +0x2F0…+0x2FC` (the options 0x251…0x254 `.TACH0RPM`, `.TACH1000RPM`,
    /// `.SPEEDO0KM`, `.SPEEDO100KM`, sent at 0x4EF331…0x4EF37B into `carinfo+0x524…+0x530`): the
    /// dash needles' rest angle and scale, in turns
    dash: [f32; 4],
    /// the frame clock the race channels run on, ms (`TIMER_getTicks` is 128 Hz)
    clock_ms: u64,
    /// (pivot in body space, is_front, is_left, parts)
    pub wheels: Vec<CarWheelModel>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub wheel_radius: f32,
    pub half_track: f32,
    /// The car's lamps: every glare article's `ef` record and its mirrored copy
    /// (`RE/notes/particles.md` §4).
    pub lights: Vec<CarLight>,
    /// The car's texture pages before any damage, the decals of `<model>D.fsh` / `<model>L.fsh` and
    /// the page textures to rewrite when they change (`RE/notes/car-model.md` §11).
    damage_pages: Vec<nfs_assets::car::TexturePage>,
    damage_patches: Vec<nfs_assets::car::DamagePatch>,
    page_textures: Vec<nfs_renderer::TextureId>,
    /// `R3DCar+0x6A8`: the ten zone values ([`zone_damage`]) the car was last brought up to date
    /// with — 0x438230 compares them exactly every frame (0x438240…0x438259)
    damage_shown: [f32; 10],
    /// the meshes 0x43E390 crushes: every part of an article with `Base+78 & 8` and a `dz` part
    deform: Vec<DeformPart>,
}

/// One mesh of a crushable part (0x43E390): its undamaged vertices (`vt#lod`), each vertex's crush
/// vector (`vt#(0x8000|lod)`, `w` 0) and its damage zone (the `dz#lod` list it is in; 255 none).
#[derive(Clone)]
struct DeformPart {
    mesh: MeshId,
    base: Vec<Vertex>,
    /// its mesh's triangle list — kept so [`CarModel::instantiate`] can re-upload a pristine copy
    /// of this part for a traffic/cop car instance without reading the (possibly already crushed)
    /// GPU buffer back
    indices: Vec<u32>,
    delta: Vec<[f32; 3]>,
    zone: Vec<u8>,
}

/// `0x5B35AC` / `0x5B35D4`: the caps 0x4382E0 puts on each zone's damage, × 100 — every zone 1 on
/// a car the camera does not follow, and on the one it does (`carInfo+0x520 == 0x465D80(0)`, with
/// slot 0's view non-zero or `[0x627F43] & 8` — any view) **0** on zones 3, 7 and 8, the sides and
/// the roof.
const ZONE_CAP: [f32; 10] = [1.0; 10];
const OWN_ZONE_CAP: [f32; 10] = [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0];

/// **0x4382E0(zone)**: the zone's damage `car+0x4E4 + 4·zone` capped at `100 · cap` — `fst` of the
/// cap to a float, then the smaller (0x43831F…0x43837A, `test ah, 1`).
pub fn zone_damage(damage: &[f32; 10], own_car: bool) -> [f32; 10] {
    let caps = if own_car { OWN_ZONE_CAP } else { ZONE_CAP };
    std::array::from_fn(|z| {
        let cap = caps[z] * 100.0;
        if cap < damage[z] { cap } else { damage[z] }
    })
}

/// A lamp's `ef` record (`RE/notes/particles.md` §4), in the body frame.
#[derive(Debug, Clone, Copy)]
pub struct CarLight {
    /// `L+0x00`, the lamp's centre
    pub position: Vec3,
    /// `L+0x10` / `L+0x20`, its two half-axes
    pub axis1: Vec3,
    pub axis2: Vec3,
    /// `L+0x30`, the way it shines
    pub facing: Vec3,
    /// `L+0x40` / `L+0x44`, the two sprites' ARGB colours
    pub colour: [u32; 2],
    /// `L+0x4C`: 0 headlight, 1 brake, 2 reverse, 3 indicator, 4 glint (the 0x5CD73C row)
    pub kind: u8,
    /// the lamp slot `car+0x2D4[slot]` that switches it (0x443660)
    pub slot: u8,
    /// `L+0x4F`: the headlight that also throws the night beam 0x43D4F0
    pub night: bool,
    /// The article's own frame, model space (column form): its `tr` part for the LOD being drawn
    /// (0x59F220 looks up `"tr"` + `[this+8]`, 0x43D8FB…0x43D907), identity when it has none. The
    /// mirrored copy's is that matrix with its rotation transposed and its translation's x
    /// negated (0x43DB5B…0x43DC0D).
    pub frame: Mat4,
    /// The rotation channel turning the lamp inside that frame (0 = none): the article's
    /// `Base+0x4B` for the primary (0x43D943), `L+0x4D` for the mirrored copy (0x43DC36) — both
    /// only when the article has a `tr` and `Base+0x4B` is non-zero (0x43D94A / 0x43DC30).
    pub channel: u8,
}

/// Model animations are numbered 1…7 in the shipped cars; slot 0 is unused.
const ANIMS: usize = 8;

/// One of slot 9's eight keyframe players, `R3DCar+0x484 + 0x1C·n` (0x438B66…0x438C3D) — player
/// `n` drives animation `n`: `{count, table, start, duration, frame, next, frac}`.
#[derive(Debug, Clone, Copy, Default)]
struct KeyPlayer {
    table: &'static [u32],
    start_ms: u64,
    duration_ms: u64,
    frame: u32,
    next: u32,
    frac: f32,
}

impl KeyPlayer {
    fn active(&self) -> bool {
        !self.table.is_empty()
    }

    /// A gesture starting now: frame and next are the table's first two when it has two
    /// (0x438DF1…0x438E18), the fraction 0.
    fn start(&mut self, table: &'static [u32], now: u64, duration_ms: u64) {
        self.table = table;
        self.start_ms = now;
        self.duration_ms = duration_ms;
        if table.len() > 1 {
            self.frame = table[0];
            self.next = table[1];
        }
        self.frac = 0.0;
    }

    /// 0x438B73…0x438C30: while the elapsed time is under the duration, `p = elapsed / duration ·
    /// (count − 1)`, the frame `table[fistp(p − 0.5)]`, the next `table[fistp(p + 0.5)]` and the
    /// fraction `p − fistp(p − 0.5)`; past it the player stops, its last frame kept.
    fn step(&mut self, now: u64) {
        if !self.active() {
            return;
        }
        let elapsed = now.saturating_sub(self.start_ms);
        if elapsed >= self.duration_ms {
            self.table = &[];
            return;
        }
        let p = elapsed as f32 / self.duration_ms as f32 * (self.table.len() as f32 - 1.0);
        let i = (p - 0.5).round_ties_even().max(0.0) as usize;
        let j = (p + 0.5).round_ties_even().max(0.0) as usize;
        self.frame = self.table[i.min(self.table.len() - 1)];
        self.next = self.table[j.min(self.table.len() - 1)];
        self.frac = p - i as f32;
    }
}

/// The right arm's gear-change gestures, by the steering frame 0…9 (`0x5CD3AC`…`0x5CD4C0`, the
/// ten `{count, table}` pairs 0x438CD1…0x438D42 builds): from the wheel out to the lever's frames
/// 10…15 and back.
const GEAR_GESTURES: [&[u32]; 10] = [
    &[0, 10, 11, 12, 14, 15, 14, 12, 11, 10, 0],
    &[1, 10, 11, 12, 14, 15, 14, 12, 11, 10, 1],
    &[2, 11, 12, 14, 15, 14, 12, 11, 2],
    &[3, 11, 12, 14, 15, 14, 12, 11, 3],
    &[4, 12, 14, 15, 14, 12, 4],
    &[5, 12, 14, 15, 14, 12, 5],
    &[6, 13, 15, 13, 6],
    &[7, 13, 15, 13, 7],
    &[8, 13, 15, 13, 8],
    &[9, 13, 15, 13, 9],
];

/// The handbrake's, three frames each (`0x5CD2BC` pulling, `0x5CD334` letting go, 12 bytes apart,
/// 0x438EB0…0x438F85): to the lever at 16…18 and the held frame 19.
const HANDBRAKE_ON: [[u32; 3]; 10] =
    [[0, 16, 19], [1, 16, 19], [2, 16, 19], [3, 16, 19], [4, 16, 19], [5, 17, 19], [6, 17, 19], [7, 18, 19], [8, 18, 19], [9, 18, 19]];
const HANDBRAKE_OFF: [[u32; 3]; 10] =
    [[19, 16, 0], [19, 16, 1], [19, 16, 2], [19, 16, 3], [19, 16, 4], [19, 17, 5], [19, 17, 6], [19, 18, 7], [19, 18, 8], [19, 18, 9]];

/// The body's turn to look back, by the body frame 0…4 (`0x5CD568` into reverse, `0x5CD590` out
/// of it, `{count, table}` pairs, 0x43914F / 0x4391A3): toward frame 7 and back.
const REVERSE_ON: [&[u32]; 5] = [&[0, 1, 2, 6, 7], &[1, 2, 6, 7], &[2, 6, 7], &[3, 6, 7], &[4, 6, 7]];
const REVERSE_OFF: [&[u32]; 5] = [&[7, 6, 2, 1, 0], &[7, 6, 2, 1], &[7, 6, 2], &[7, 6, 3], &[7, 6, 4]];

/// What slot 9's driver half reads off the car each frame (0x438B2C…0x439352).
#[derive(Debug, Clone, Copy, Default)]
pub struct DriverInputs {
    /// `[0x6197C4]`, the car detail (`CARDETAIL`, 0x44EF80…0x44EFA7): under 1 the driver is posed,
    /// not animated
    pub detail: u8,
    /// channel 5's `.z`: `ch1.y + ch2.y` — the two front wheels' `+0xB4` steering values over 216,
    /// negated in a mirrored race (0x438461…0x4384EB) — clamped to ±4.5/23 (0x4385AF…0x4385EB)
    pub wheel_turns: f32,
    /// `car+0xD84`, the shift timer (ticks)
    pub shift_timer: u32,
    /// `car+0xB50 == 0` and neither pause flag (`[0x606A90]`, `[0x606A98]`) set (0x438CA0…0x438CC2)
    pub racing: bool,
    /// `car+0xD86`
    pub handbrake: bool,
    /// `car+0xD82 == 0`
    pub reverse: bool,
    /// `car+0xE04` / `+0xE08`, the filtered longitudinal and lateral accelerations
    pub long_acc: f32,
    pub lat_acc: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct DriverAnim {
    clock_ms: u64,
    players: [KeyPlayer; ANIMS],
    /// `R3DCar+0x564` / `+0x566`: the handbrake and reverse states the gestures were last started for
    handbrake: bool,
    reverse: bool,
}

/// One `{start, duration, from, to}` easer — the 16-byte record the original keeps per channel at
/// `object+0x64 + 16n` (`car-model.md`), with the clock kept as elapsed rather than a start tick.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelEase {
    from: f32,
    to: f32,
    elapsed: u32,
    duration: u32,
}

/// One float key out of a `.tpg` `[header]`, `default` when absent.
fn tpg_f32_or(tpg: &nfs_formats::ini::Ini, key: &str, default: f32) -> f32 {
    tpg.section("header").and_then(|h| h.get_f32_list(key)).and_then(|v| v.first().copied()).unwrap_or(default)
}

/// One float key out of a `.tpg` `[header]`, 0.0 when absent.
fn tpg_f32(tpg: &nfs_formats::ini::Ini, key: &str) -> f32 {
    tpg.section("header").and_then(|h| h.get_f32_list(key)).and_then(|v| v.first().copied()).unwrap_or(0.0)
}

#[derive(Clone)]
pub struct CarWheelModel {
    pub pivot: Vec3,
    pub is_front: bool,
    pub is_left: bool,
    /// The article's `Base+75` rotation channel. Every wheel-typed article of every shipped car is
    /// bound to **1…4** — the wheels that spin — or to **18…21**, the tread that takes the steer
    /// but not the spin, because 0x438669…0x438726 copy channels 1…4 into 18…21 with `.x` cleared
    /// (`cargo run -p nfs-formats --example rotchan`).
    pub channel: u8,
    /// the article's place in its LOD's sorted part list (see `CarModel::body_order`)
    pub order: u32,
    /// the LOD these parts are and the article's pass bytes
    pub info: PartInfo,
    pub parts: Vec<(MeshId, MaterialId, Mat4)>,
}

/// What the car draw needs of one article's `Base` record to place a part in a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartInfo {
    /// the LOD (1…7) the part's primitives belong to
    pub lod: u8,
    /// `Base+78`, the pass bits: `R3DCarLOD::draw` 0x43C830 draws the part in a call whose mask
    /// meets it, `Base+78 & mask & 0xF0` (0x43C8DF…0x43C8E9)
    pub mask: u8,
    /// `Base+81`: the part takes part in a call with the flag argument set (0x43C8C7)
    pub inside: bool,
    /// `Base+82`: the part stays out of a call without it (0x43C8D4) — the driver's hands
    pub inside_only: bool,
    /// `Base+72`, the translation channel, −1 for none (0x43C945)
    pub move_channel: i8,
}

impl PartInfo {
    /// Whether 0x43C830 draws this part in `pass` (0x43C8AC…0x43C8E9).
    pub fn in_pass(&self, pass: &CarPass) -> bool {
        self.lod == pass.lod && self.mask & pass.mask & 0xF0 != 0 && if pass.flag { self.inside } else { !self.inside_only }
    }
}

/// One `R3DCarLOD::draw` 0x43C830 call of the car draw **0x444E00**: the LOD object (`[R3DCar+0xB0]
/// [lod − 1]`), the mask its parts' `Base+78` must meet and the flag argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarPass {
    pub lod: u8,
    pub mask: u8,
    pub flag: bool,
}

/// The front end's two calls (0x444E86…0x444EBD): mask **0x50** from the LOD of the class
/// `R3DCar+0x2C0`, which the front end's branch of 0x439F90 zeroes every frame (0x439FF1), and mask
/// **0x20** from LOD 1 — no 0x80 call, so no driver.
pub const FRONT_END_PASSES: [CarPass; 2] = [CarPass { lod: 1, mask: 0x50, flag: false }, CarPass { lod: 1, mask: 0x20, flag: false }];

/// `0x5CDB28[class]`: the driver call's LOD object, class 0…3 only (0x444E5E…0x444E83)
const DRIVER_LOD: [u8; 7] = [0, 0, 0, 1, 1, 2, 3];
/// `0x5CDB44[7·CARDETAIL + class]`: the wheels' LOD object (0x444EF5…0x444F22)
const WHEEL_LOD: [[u8; 7]; 3] = [[0, 0, 1, 1, 1, 5, 6], [0, 0, 0, 0, 1, 5, 6], [0, 0, 0, 0, 1, 5, 6]];
/// `0x5CDB98[class]`: a traffic car's wheels (0x444EC0…0x444EF2)
const TRAFFIC_WHEEL_LOD: [u8; 7] = [0, 0, 1, 2, 2, 2, 2];
/// `0x5CD5B8[CARDETAIL]`: the class a racer or cop starts from (0x43A083)
const CLASS_BIAS: [i32; 4] = [3, 3, 2, 2];
/// `0x5CDB1C[CARDETAIL]`: the hands' LOD object in the driver view (0x444F9B)
const HANDS_LOD: [u8; 3] = [1, 0, 0];

/// The race's calls of 0x444E00 outside the driver view, for LOD class `class`: mask **0x80** (the
/// driver) from `0x5CDB28[class]` while `class < 4`, **0x50** (the body, its interior, the steering
/// wheel) from the class's own LOD, **0x20** (the wheels) from `0x5CDB44[7·CARDETAIL + class]`, or
/// for traffic (`car+0x52C & 0x10`) from `0x5CDB98[class]` — all without the flag, so a part with
/// `Base+82` (the hands) is left out.
pub fn race_passes(class: u8, detail: u8, traffic: bool) -> Vec<CarPass> {
    let c = (class as usize).min(6);
    let mut v = Vec::with_capacity(3);
    if c < 4 {
        v.push(CarPass { lod: DRIVER_LOD[c] + 1, mask: 0x80, flag: false });
    }
    v.push(CarPass { lod: c as u8 + 1, mask: 0x50, flag: false });
    let w = if traffic { TRAFFIC_WHEEL_LOD[c] } else { WHEEL_LOD[(detail as usize).min(2)][c] };
    v.push(CarPass { lod: w + 1, mask: 0x20, flag: false });
    v
}

/// 0x444F4E…0x444FB5: the camera's own car in views 0 and 1 (`[0x627F43] & 8`) — mask **0x10**
/// with the flag from LOD **2** (the shell and the cabin, the parts with `Base+81`), **0x40** with
/// the flag from LOD 1 (the steering wheel), and, at `CARDETAIL` above 0 and with none of the look
/// inputs 0x13 / 0x14 / 0x15 held (`[0x5E99B0]`, `[0x5E99B4]`, `[0x5E99AC]`), **0x80** with the
/// flag from `0x5CDB1C[CARDETAIL]` (LOD 1: the hands and a passenger).
pub fn in_car_passes(detail: u8, looking: bool) -> Vec<CarPass> {
    let mut v = vec![CarPass { lod: 2, mask: 0x10, flag: true }, CarPass { lod: 1, mask: 0x40, flag: true }];
    if detail > 0 && !looking {
        v.push(CarPass { lod: HANDS_LOD[(detail as usize).min(2)] + 1, mask: 0x80, flag: true });
    }
    v
}

/// What the LOD pick needs of a car beside its model: `car+0x52C`'s traffic (0x10) and opponent
/// (8) bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarKind {
    /// the player, a cop
    Racer,
    Opponent,
    Traffic,
}

/// A car's own LOD state, which the original keeps on its `R3DCar`: the class `+0x2C0` and the
/// translation channels' `.y` (`+0x6D0 + 12m`, 0x438210). The traffic and the cops share one
/// model per type, so this lives with each car.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarLodState {
    pub class: u8,
    pub travel: [f32; 4],
}

/// The screen the LOD pick projects on (0x539700 through the view `[[0x628130]+0x10]+0xC`) and its
/// width in pixels `[0x657A48]`.
#[derive(Debug, Clone, Copy)]
pub struct LodView {
    pub view_projection: Mat4,
    pub width: f32,
    pub height: f32,
    /// the render width `[0x657A48]` the reference 640 is taken over (0x43A2D7): the window's
    /// width, whichever view's viewport `width` / `height` are
    pub ref_width: f32,
}

/// The car's collision body points `car+0x8C` (`BndP`, `RE/notes/car-model.md` §12): the records
/// the body style admits, their points **as the loader 0x442080 stores them** — the model's own
/// axes, not turned into the body frame the model is drawn in (0x49AE30 uses them unturned with
/// the physics rows) — then 0x49ABC0's sort by height and quadrant permutation
/// ([`nfs_physics::vehicle::body_points_setup`]), so the first four are the ground probes FL, FR,
/// RL, RR. *(Corrected 2026-09-25: these went through `MODEL_TO_BODY`, x and z negated, which put
/// the front box behind and the wheel quad's longer half in front.)*
///
/// `y` here keeps the model's `y = 0` on the contact plane; the loader then subtracts `car+0x3AC`
/// — half the model box's height, set by 0x445090 before it (0x4420A3 → 0x442775) — which
/// [`car_body`] does, so a racer built from that carries the original's origin, half the model's
/// height above the ground (`Vehicle::set_car_body`).
pub fn car_body_points(car: &CarAssets) -> Vec<Vec3> {
    let Ok(recs) = car.crp.bound_points(&car.crp_data) else { return Vec::new() };
    // The loader's filter is `carinfo[0x56C + 4·slot] == value` (0x4420B9), i.e. the body style's
    // own array — not "slot 0 only", which is what this used to be and which left every
    // style-selected record out (the spoilers, the GT1 wing).
    let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
    let mut pts: Vec<Vec3> = recs
        .iter()
        .filter(|r| style.article_enabled(r.style_slot as u8, r.style_value as u8))
        .flat_map(|r| r.points.iter().map(|p| Vec3::from(*p)))
        .collect();
    nfs_physics::vehicle::body_points_setup(&mut pts);
    pts
}

/// The render model's local box that **0x445090** (`RCarPages::finishLoad`, tail of 0x4425B0,
/// called from `RCar::load` 0x442775) halves into `car+0x3A8 / +0x3AC / +0x3B0` — before the `BndP`
/// loader 0x442080 runs (0x4420A3 → 0x4425B0), so its `y` is what the loader subtracts.
///
/// 0x445090 asks the LOD object `[this+0xB0][4]` (`[this+0xB0][0]` for traffic, `car+0x52C &
/// 0x10`) for its box through vtable `+0x10`, **0x43DE90**; `d = max − min` (0x532330),
/// `car+0x3A8…+0x3B0 = 0.5·d` (0x532360), `× 0.4` for `.TINYCAR` (0x445140…0x44517C, `[0x5B37C8]`).
/// The seven LOD objects are built by 0x4425B0 through slot 17 (0x43ADB0: `+8 = i`, `i = 1…7`,
/// the count from slot 16 0x43ADA0), so `[4]` is **LOD 5** and `[0]` LOD 1. 0x43DE90 starts `min`
/// and `max` at **zero** (0x43DE99…0x43DEDA — the box always holds the model origin) and folds in,
/// for every article the LOD lists — 0x43C1B0: the style admits it (`carinfo[0x56C + 4·Base+68] ==
/// Base+69`, 0x43C2A1) and one of its `Base` LOD records carries this LOD (`word Base+0x6C + 12k`,
/// 0x43C2BC) — every vertex of its `vt#lod` part (count = the part entry's `b`, 16 bytes each),
/// through its `tr#lod` matrix when it has one (0x59F220 `'tr' | lod`, 0x53A330 `v·M`).
///
/// CONFIRMED for the code path; the port's reading of the `vt` part is the one its mesh loader
/// uses. A racer's box is LOD 5 ([`car_body`]); traffic's LOD 1 (`crates/porsche`'s traffic set-up,
/// `nfs_game::traffic::TrafficCar::draw_origin`).
pub fn car_model_box(car: &CarAssets, lod: u16) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::ZERO, Vec3::ZERO);
    let Ok(arts) = car.crp.articles(&car.crp_data) else { return (lo, hi) };
    let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
    for a in &arts {
        let Some(base) = a
            .level(b"Base")
            .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok().map(|d| (e, d)))
            .and_then(|(e, d)| nfs_formats::crp::Base::parse(d, e.data_offset()).ok())
        else {
            continue;
        };
        if !style.article_enabled(base.tag[0], base.tag[1]) || !base.lods.iter().any(|r| r.lod == lod) {
            continue;
        }
        let Some(vt) = a.find(b"vt", lod) else { continue };
        let Ok(bytes) = car.crp.entry_data(&car.crp_data, vt) else { continue };
        let count = match vt {
            nfs_formats::crp::CrpEntry::Part { b, .. } => (*b as usize).min(bytes.len() / 16),
            _ => 0,
        };
        let tr = a.find(b"tr", lod).and_then(|e| car.crp.entry_data(&car.crp_data, e).ok()).filter(|b| b.len() >= 64).map(|b| {
            let f = |i: usize| f32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
            // row-major, translation in row 3, `v' = v·M` — glam's column-major reading of the
            // same sixteen floats is that matrix transposed, which is `M` applied to a column
            Mat4::from_cols_array(&std::array::from_fn(|i| f(i)))
        });
        for k in 0..count {
            let f = |o: usize| f32::from_le_bytes([bytes[k * 16 + o], bytes[k * 16 + o + 1], bytes[k * 16 + o + 2], bytes[k * 16 + o + 3]]);
            let mut p = Vec3::new(f(0), f(4), f(8));
            if let Some(m) = tr {
                p = m.transform_point3(p);
            }
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    (lo, hi)
}

/// A racer's collision body as `RCar::load` leaves it, in the original's own frame: the half
/// extents `car+0x3A8…+0x3B0` of 0x445090 ([`car_model_box`], LOD 5, `× 0.4` for `.TINYCAR`) and
/// the `BndP` points of 0x442080 — the style's records, `× 0.4` for `.TINYCAR` (0x442140…), then
/// **`y −= car+0x3AC`** (0x4421BE, `fsub [ecx+0x3AC]`) — through 0x49ABC0's sort and quadrant deal
/// ([`nfs_physics::vehicle::body_points_setup`]; a constant shift leaves both unchanged).
///
/// So the body origin `car+0x330` sits `car+0x3AC` — half the model box's height, the box holding
/// the model's `y = 0` ground plane — above the plane the wheel quad stands on, and the renderer
/// draws the model that far below it (`R3DCar` slot 5 0x43A410, `Vehicle::draw_origin`). Racers
/// only: nothing re-cuts a racer's box afterwards (0x4A1280's `(max − min)/2` of the body points,
/// 0x4A1472…0x4A14A6, runs from 0x4107B6 only for `car+0x52C & 0x80`, a smackable).
pub fn car_body(car: &CarAssets, tiny: bool) -> (Vec<Vec3>, Vec3) {
    let (lo, hi) = car_model_box(car, 5);
    let scale = if tiny { 0.4 } else { 1.0 };
    let half = (hi - lo) * 0.5 * scale;
    let mut pts = car_body_points(car);
    for p in &mut pts {
        *p *= scale;
        p.y -= half.y;
    }
    (pts, half)
}

/// What a car's reflection passes draw with, set per race.
#[derive(Debug, Clone, Copy, Default)]
pub struct CarEnv {
    /// `R3DSpecularTextureLayer`'s three textures, `[0x6115CC]+0xDC…+0xE4` — [`car_specular_maps`]:
    /// `SPC1…3` in a race, `FEDU` / `FE  ` / `FESH` in the front end (0x441AF1). *(Until
    /// 2026-09-26 the front end's cars were uploaded with none.)*
    pub specular: [Option<nfs_renderer::TextureId>; 3],
    /// `[0x6197F8]` has bit 2 — `Graphics.CarChrome` ≥ Medium (0x44EF3A / 0x44EF5D) — so the
    /// `CarExtEnv` / `CarExtEnvNS` passes of mask 2 are built (`lighting.md` §5.3)
    pub chrome_passes: bool,
    /// Give each material its `Rs` block's depth bias (Thrash 0x18, `mt+276`) — set for the race,
    /// whose view's depth scale is read (the view's near and the camera object's far 700,
    /// `thrash.md` §6); the front end's 3D view's planes are not read, so its cars keep 0 there.
    pub rs_bias: bool,
}

pub const MODEL_TO_BODY: Mat4 = Mat4::from_cols(
    glam::Vec4::new(-1.0, 0.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 0.0, -1.0, 0.0),
    glam::Vec4::new(0.0, 0.0, 0.0, 1.0),
);

impl CarModel {
    pub fn upload(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: impl Into<nfs_assets::car::Paint>) -> anyhow::Result<Self> {
        Self::upload_with_env(r, car, paths, lod, paint, CarEnv::default(), false, None, None)
    }

    /// `env`: the specular layer's three textures and whether the option word builds the `CarExtEnv`
    /// passes ([`CarEnv`]).
    ///
    /// `race_roof`: in a race, the car's `.CABRIODOWN` option for slot 19's roof line
    /// ([`nfs_assets::car::CarStyle::apply_race_roof`]); `None` in the front end, whose branch
    /// (0x43B003) does not take it.
    ///
    /// `skidpad`: the `.tpg` entry test's `testdrive` context — `Some(Factory Driver mission)` on
    /// `skidpad`, `None` elsewhere ([`nfs_assets::car::CarStyle::skidpad`]).
    #[allow(clippy::too_many_arguments)]
    pub fn upload_with_env(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: impl Into<nfs_assets::car::Paint>, env: CarEnv, night: bool, race_roof: Option<u32>, skidpad: Option<bool>) -> anyhow::Result<Self> {
        // `[styleN]` is **1-based** and `CarAssets::style_index` is the raw 0-based `nfs5.car
        // +0x1C0`, which is what 0x437B33's `inc esi` turns into the section name.
        let mut style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
        // …and after the section `applyStyle` writes slot 10 (`car+0x594`) from the race number:
        // 1 when car-info `+0x510` is 0 or more, else 0 (0x437C09…0x437C2B; the front end's
        // 0x4D1800 does the same from the car's `DecalNum`). A car without page text is the
        // front end's or one whose number was not read, and −1 is what both hold.
        style.geometry.insert(10, u32::from(car.page_text.as_ref().is_some_and(|t| t.race_number >= 0)));
        // Slot 19 overlays the driver's variants onto the same array at model load, in its race
        // branch only (0x43AEA5…0x43AF6B; the front end's, 0x43B003, jumps past them), keyed on
        // the car's `.DRIVER` pick (`CarAssets::driver`).
        //
        // The other two things that block writes (`car-model.md`) are **left to the style**, and
        // deliberately: slot 42 (`SpoilerDown` / `SpoilerUp`) is zeroed by 0x43AEB2, which hides
        // both halves of the pop-up spoiler until the animation channel picks one, and the port
        // does not run that channel — so zeroing it would leave a hole in the engine lid where
        // the style's own `42 = 1`, the retracted spoiler, belongs. Slot 41 is recomputed from
        // the roof option in a race (0x43AE7A).
        match race_roof {
            Some(v) => {
                style.apply_race_roof(v);
                style.apply_driver_detail(car.driver.driver, car.driver.no_driver, car.driver.factory_driver);
            }
            // slot 19's front-end branch, 0x43B003: slot 41 and slot 6 to 0
            None => {
                style.geometry.insert(41, 0);
                style.geometry.insert(6, 0);
            }
        }
        // and both branches end in the overlay, 0x43B0B7…0x43B0DC, and in `slot58 =
        // carinfo+0x518`, the `.TINYCAR` flag (0x43B0E5)
        style.apply_overlay(&car.style_overlay);
        style.geometry.insert(58, u32::from(car.tiny));
        // the entry test's context (0x439C60): the front-end branch is the one without a race roof
        style.front_end = race_roof.is_none();
        style.night = night;
        style.skidpad = skidpad;
        tracing::info!(style = style.name, "car style");
        let paint: nfs_assets::car::Paint = paint.into();
        // `R3DCar+0xAA`, the exterior's HSV value `max(r, g, b)` (0x443CA0 from 0x443E18), which
        // picks the damage pack below (0x441FFD, 0x442080)
        let light_paint = paint.exterior.iter().copied().max().unwrap_or(0) > 0x50;
        let pages = car.build_pages(paths, paint, &style)?;
        let page_tex: Vec<(nfs_renderer::TextureId, bool)> = pages
            .iter()
            .map(|p| (r.upload_texture(&TextureData::single(p.width, p.height, p.rgba.clone())), p.is_window || p.is_alpha))
            .collect();
        // the damage decals, `L` for a paint brighter than 0x50, `D` otherwise
        let damage_patches = nfs_assets::car::damage_patches(paths, &car.model, light_paint).unwrap_or_default();
        let page_textures: Vec<nfs_renderer::TextureId> = page_tex.iter().map(|p| p.0).collect();
        let materials = car.crp.materials(&car.crp_data)?;
        let mut mat_ids: HashMap<u16, MaterialId> = HashMap::new();
        let mut material_light: HashMap<MaterialId, nfs_formats::crp::LayerLight> = HashMap::new();
        let mut shadowed: std::collections::HashSet<MaterialId> = std::collections::HashSet::new();
        let mut material_for = |r: &mut Renderer, id: u16| {
            if let Some(m) = mat_ids.get(&id) {
                return *m;
            }
            let m = materials.get(id as usize);
            let page = m.map(|m| m.page_index() as usize).and_then(|p| page_tex.get(p));
            let blend = m.map(|m| matches!(m.render_method.as_str(), "CarWindow" | "CarWheel")).unwrap_or(false) || page.map(|p| p.1).unwrap_or(false);
            // the specular and sky-map passes belong to `CarExtEnv` and `CarExtEnvNS` (passes of
            // option mask 2, built at CarChrome ≥ Medium) and to `CarWindow` (mask 0, always built)
            // — `rmthpasses`, `lighting.md` §5.3. *(Until 2026-09-26 only `CarExtEnv` reflected, and
            // at Low not even `CarWindow`.)*
            let reflective = m.is_some_and(|m| match m.render_method.as_str() {
                "CarExtEnv" | "CarExtEnvNS" => env.chrome_passes,
                "CarWindow" => true,
                _ => false,
            });
            // `R3DSpecularTextureLayer` and `R3DSkyMapTextureLayer` bucket the material's own
            // reflection strength to pick which of their three textures they take (0x44553F,
            // 0x445B33; `lighting.md` §3.1, §3.2 — the four strength dwords at `mt+284…+296` are equal
            // in every shipped material, so the one `reflection` stands for each layer's own). The
            // passes' combine is their `Rs` block `{1, 2, 0, 1, 0, −1}` (`rmthpasses`): alpha-blended
            // over the pixel by the texel's own alpha — `env_mix` is only the switch
            let variant = nfs_formats::crp::env_variant(m.map(|m| m.reflection).unwrap_or(0.0));
            // the specular texture is the car's own `SPC{bucket+1}` (0x441AB2), not a section map
            let spec = if reflective { env.specular[variant] } else { None };
            // the cull mode is the material's own `Rs` block, Thrash state 2 at `mt+272`: 1 the
            // view's own (the back faces, `thrash.md` §6d.2), 0 none — the port had culled every opaque one and
            // none of the blended, which with depth writes on let a wheel well's outer faces cover
            // the wheel
            let cull = m.and_then(|m| m.rs).map_or(!blend, |rs| rs[3] == 1);
            // A front-end room is drawn by the car's own class (`nfs_assets::car::CarAssets::
            // front_end_room`), so its `Rs` block is what `LayerState` hands the driver: Thrash
            // 0x0A = 2 blends every one of its materials, and 0x38 picks the pair — 0 the plain
            // alpha pair, **1 `SRCALPHA` / `ONE`** for the light pools and light cones on the 4444
            // pages (`Lightcast*`, `SpotLight`, `GarageSpot`'s `ligh` / `spot`), which add their light
            // to what is under them (`RE/notes/frontend-3d.md` §9). The cars keep the port's rule by
            // method and page: every shipped car block is `{1, 2, 0, …}`, the plain pair.
            let (blend, blend_pair) = match m.and_then(|m| m.rs) {
                Some(rs) if car.front_end_room => (rs[1] == 2, rs[2].clamp(0, 3) as u8),
                _ => (blend, 0),
            };
            // Thrash 0x18 from the same block, `rs[4]` (`LayerState` 0x53F780 → 0x540660 hands the
            // driver states 6, 0x0A, 0x38, 2 and 0x18, the last through 0x540500): over the shipped
            // cars −10, −3, −2, −1, 0, 1, 3 and 5 (`nfs-formats/examples/rsblock.rs`) — a pull of
            // `−b · (far − near) / 32768` in view depth, 2.1 cm a unit in the race's main view and
            // ~2 m in the rear view (far 65000). *(Until 2026-09-26 the cars took 0.)*
            let depth_bias = if env.rs_bias { m.and_then(|m| m.rs).map_or(0, |rs| rs[4]) } else { 0 };
            let mid = r.create_material(MaterialDesc { texture: page.map(|p| p.0), alpha_blend: blend, cull_back: cull, depth_test: true, env_texture: spec, env_mix: 1.0, env_variant: variant, sky_map: reflective, blend_pair, clamp: m.map_or([false; 2], |m| m.address.map(|a| a == 0)), lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: true, view_cull: false, depth_bias, false_sort: false });
            if let Some(l) = m.and_then(|m| m.light) {
                material_light.insert(mid, l);
            }
            // the methods with a pass of layer class **0x8102**, `R3DShadowMapTextureLayer` (the
            // factory's 0x449FB4 arm → vtable 0x5B3964) that the option word builds (`rmthpasses`
            // over every shipped car and room `.crp`, `lighting.md` §5.3): `CarExt` and `CarIntShd`
            // (pass 1, mask 0), `CarWindow` (pass 3, mask 0) and `CarExtEnv` (pass 3, mask 2 — built
            // at CarChrome ≥ Medium). `CarExtEnvNS` ("no shadow") lists it only in its dead mask-4
            // passes, `CarInt` and `CarWheel` not at all. *(Corrected 2026-09-26: this was
            // `CarExtEnv` / `CarExtEnvNS` / `CarWindow`, "the layer 0x8101" — 0x8101 is the sky map.)*
            if m.is_some_and(|m| match m.render_method.as_str() {
                "CarExt" | "CarIntShd" | "CarWindow" => true,
                "CarExtEnv" => env.chrome_passes,
                _ => false,
            }) {
                shadowed.insert(mid);
            }
            mat_ids.insert(id, mid);
            mid
        };
        let arts = car.crp.articles(&car.crp_data)?;
        let mut body = Vec::new();
        let mut body_channel: Vec<u8> = Vec::new();
        // each `body` entry's article index, for its place in `lod_list` after the loop
        let mut body_art: Vec<usize> = Vec::new();
        let mut wheel_art: Vec<usize> = Vec::new();
        let mut wheels = Vec::new();
        let mut skipped = 0;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut lights: Vec<CarLight> = Vec::new();
        let mut morph: [Vec<MorphPart>; ANIMS] = Default::default();
        let mut anim_length = [0u32; ANIMS];
        let mut wheel_radius: f32 = 0.3;
        let mut half_track: f32 = 0.7;
        let mut body_info: Vec<PartInfo> = Vec::new();
        let mut deform: Vec<DeformPart> = Vec::new();
        let mut lod_parts = [0u32; 7];
        let mut lod_moves = [[false; 4]; 7];
        // 0x43C1B0's 12-byte records `{article, lod record, Base+70}` of each LOD, in article order
        // (0x43C2D5…0x43C2F3: one per admitted article, the first `Base` LOD record for the LOD)
        let mut lod_list: [Vec<(usize, i8)>; 7] = Default::default();
        for (ai, a) in arts.iter().enumerate() {
            // 0x43C1B0's part list of each LOD: the articles the style admits with a `Base` LOD
            // record for it (0x43C2A1…0x43C2C9) — glares included — and, per part, its
            // translation channel `Base+72` into the LOD's gate `+0x14 + 4m` (0x43C66D…0x43C67C)
            if let Some(d) = a.level(b"Base").and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
                && d.len() > 0x48
                && style.article_enabled(d[0x44], d[0x45])
            {
                let u32_at = |o: usize| d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
                for k in u32_at(8)..u32_at(12) {
                    let o = 0x64 + 12 * k;
                    let Some(w) = d.get(o + 8..o + 10) else { break };
                    let l = u16::from_le_bytes([w[0], w[1]]) as usize;
                    if (1..=7).contains(&l) {
                        if !lod_list[l - 1].iter().any(|&(i, _)| i == ai) {
                            lod_list[l - 1].push((ai, d[0x46] as i8));
                        }
                        lod_parts[l - 1] += 1;
                        if let Ok(m) = usize::try_from(d[0x48] as i8)
                            && m < 4
                        {
                            lod_moves[l - 1][m] = true;
                        }
                    }
                }
            }
            // the glare articles hold no geometry, only an `ef` record (`particles.md` §4): its
            // lamp, the slot `Base[0x50]` that switches it (0x43C427 / 0x43D8C0), and a mirrored copy
            // across x with the slot `L+0x4E` (0x43DAC5…0x43DB32) when `L+0x48 & 8`. One `ef` per article in
            // every shipped model (0x43DDA5…0x43DE48 would walk on through consecutive ones).
            //
            // The effect pass 0x43D800 walks the LOD's part list, which 0x43C1B0 builds from the
            // articles the style admits (`carinfo[0x56C + 4·Base+68] == Base+69`, 0x43C2A1) that have
            // a `Base` LOD record for this LOD — the first `k` in `[Base+8, Base+0xC)` whose `word
            // Base+0x6C + 12k` is it (0x43C2AA…0x43C2C9); the part's `[article+0x48]` is that record
            // (0x43C42A…0x43C431), and the pass skips the part when the record's **byte +1** is 0
            // (0x43D863…0x43D872) before it looks for the `ef` (0x43D88B). Across every shipped
            // model the byte is 1 on each LOD record of an article with an `ef` and 0 on every
            // other, so it is the "has effects" flag; the style test is what drops lamps: glare
            // articles carry types 0, 1, 4, 5, 7, 18, 19, 20 and 27 with their variants, and one
            // of another variant was lit alongside the style's own.
            let effect_part = a
                .level(b"Base")
                .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
                .is_some_and(|d| {
                    let u32_at = |o: usize| d.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
                    d.len() > 0x45
                        && style.article_enabled(d[0x44], d[0x45])
                        && (u32_at(8)..u32_at(12))
                            .map(|k| 0x64 + 12 * k)
                            .find(|&o| d.get(o + 8..o + 10).is_some_and(|w| u16::from_le_bytes([w[0], w[1]]) == u16::from(lod)))
                            .is_some_and(|o| d.get(o + 1).is_some_and(|&b| b != 0))
                });
            if !effect_part && a.parts.iter().any(|p| p.is_part(b"ef")) {
                continue;
            }
            if let Some(ef) = a.parts.iter().find(|p| p.is_part(b"ef"))
                && let Ok(b) = car.crp.entry_data(&car.crp_data, ef)
                && b.len() >= 88
                && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 5
            {
                let f = |o: usize| f32::from_le_bytes([b[8 + o], b[9 + o], b[10 + o], b[11 + o]]);
                let v = |o: usize| Vec3::new(f(o), f(o + 4), f(o + 8));
                let u = |o: usize| u32::from_le_bytes([b[8 + o], b[9 + o], b[10 + o], b[11 + o]]);
                let slot = a
                    .level(b"Base")
                    .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
                    .and_then(|d| d.get(0x50).copied())
                    .unwrap_or(9);
                let to_body = |p: Vec3| MODEL_TO_BODY.transform_point3(p);
                let dir_body = |d: Vec3| MODEL_TO_BODY.transform_vector3(d);
                // the article's frame for this LOD: 0x59F220 finds the part `"tr"` + `[this+8]` — the
                // LOD itself, no fallback (0x59F238, `or eax, 0x74720000`) — and the pass multiplies
                // it in front of the car's matrix (0x43D915…0x43D929, `0x539B00`: `arg · this`),
                // then, while `Base+0x4B` names a channel, the channel's `Rx·Ry·Rz` in front of
                // that (0x43D943…0x43D9FE). Shipped: the pop-up headlights (channel 17), the cops'
                // sirens (22), a wheel glint (1), the lids' and doors' glints (6, 8, 9) — every
                // glare `tr` has an identity rotation, only a translation.
                let tr = a
                    .parts
                    .iter()
                    .find(|t| t.is_part(b"tr") && t.part_index() == u16::from(lod))
                    .and_then(|t| car.crp.entry_data(&car.crp_data, t).ok())
                    .filter(|m| m.len() >= 64)
                    .map(|m| Mat4::from_cols_array(&std::array::from_fn(|i| f32::from_le_bytes([m[4 * i], m[4 * i + 1], m[4 * i + 2], m[4 * i + 3]]))));
                let base_channel = a
                    .level(b"Base")
                    .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
                    .and_then(|d| d.get(0x4B).copied())
                    .unwrap_or(0);
                let channel = if tr.is_some() { base_channel } else { 0 };
                let lamp = CarLight {
                    position: to_body(v(0)),
                    axis1: dir_body(v(0x10)),
                    axis2: dir_body(v(0x20)),
                    facing: dir_body(v(0x30)),
                    colour: [u(0x40), u(0x44)],
                    kind: b[8 + 0x4C],
                    slot,
                    night: b[8 + 0x4F] != 0,
                    frame: tr.unwrap_or(Mat4::IDENTITY),
                    channel,
                };
                // the mirrored copy only when `L+0x48 & 8` (0x43DAB6…0x43DABA): 804 of the shipped
                // records carry 8 there and 126 carry 0, and those have no copy across x. (Until
                // 2026-09-26 every lamp was mirrored, and the LOD 1 / LOD 2 copies were folded
                // into one by position; the part list above now admits the LOD's own only.)
                lights.push(lamp);
                if b[8 + 0x48] & 8 != 0 {
                    let flip = |d: Vec3| Vec3::new(-d.x, d.y, d.z);
                    lights.push(CarLight {
                        position: flip(lamp.position),
                        axis1: -lamp.axis1,
                        axis2: flip(lamp.axis2),
                        facing: flip(lamp.facing),
                        slot: b[8 + 0x4E],
                        // 0x43DB5B…0x43DC0D: the `tr` copied with its translation zeroed, transposed
                        // (0x53A650), and `T(−t.x, t.y, t.z) · that` (0x539B80, 0x539B00) — in
                        // column form `Rᵀ · T`; the channel's value is `L+0x4D`'s (0x43DC36), the
                        // gate still the primary's `Base+0x4B` (0x43DC29)
                        frame: tr.map_or(Mat4::IDENTITY, |m| {
                            let t = m.w_axis.truncate();
                            Mat4::from_mat3(glam::Mat3::from_mat4(m).transpose()) * Mat4::from_translation(Vec3::new(-t.x, t.y, t.z))
                        }),
                        channel: if tr.is_some() && base_channel != 0 { b[8 + 0x4D] } else { 0 },
                        ..lamp
                    });
                }
                continue;
            }
            let g = car.crp.article_geometry(&car.crp_data, a)?;
            let (ty, variant) = g.base.as_ref().map(|b| (b.tag[0], b.tag[1])).unwrap_or((0, 0));
            if !style.article_enabled(ty, variant) {
                continue;
            }
            let is_wheel = matches!(ty, 0x0D | 0x0E);
            // an animated article's morph frames: each frame's geometry — 0x43C4FB probes `vt`
            // part `frame·0x10 + index` until one is missing, and the count is the animation's
            // length when it is the largest (0x43C54B…0x43C56F)
            let anim = g.base.as_ref().map(|b| b.anim as usize).filter(|&n| n > 0 && n < ANIMS).unwrap_or(0);
            let frame_geometry: Vec<nfs_formats::crp::ArticleGeometry> = if !is_wheel && anim != 0 {
                (1u16..).map_while(|f| car.crp.article_geometry_frame(&car.crp_data, a, f).ok()).collect()
            } else {
                Vec::new()
            };
            // 0x43E390 crushes the parts whose `Base+78` has bit 3 (0x43E3D8) from their
            // `vt#(0x8000|lod)` vectors by their `dz#lod` zones, into the buffer `*(part+0x40)`
            // points at. *(Settled 2026-09-27: a part that also morphs (the convertibles' roof,
            // `Base+77`) is not excluded there — 0x43E390 only gates on `Base+78 & 8` and a
            // non-null `part+0x40` (0x43E3D8, 0x43E3EF) — so it IS crushed. The two writers of
            // `*(part+0x40)`: the crush, 0x43E390, called from `R3DCar` slot 7's `0x438230(0)`
            // (0x4383CC); the morph blend, 0x43E120 (its own destination load at 0x43E225,
            // `ebp = *(part+0x40)`), called from the SAME slot's `0x438030` (0x4383D3) — right
            // after 0x438230, in the same per-frame update. Both are edge-triggered (0x438230 on a
            // changed zone value, 0x438030 on a queued, changed animation), so on a frame where
            // both trigger the morph runs last and wins; the crush persists on a stationary part
            // between hits and is erased the next time that part's animation changes. Ported: the
            // article now goes in both `deform` and `morph`; `App::update_damage_decals` already
            // calls `update_damage` before `update_animation` / `update_driver`
            // (`CarModel::blend`'s own dedup on `(frame, next, frac)` gives the edge-triggering).)*
            let damage_geometry = if g.base.as_ref().is_some_and(|b| b.draw_bytes[0] & 8 != 0) { car.crp.article_geometry_damage(&car.crp_data, a).ok() } else { None };
            // Every LOD the article has primitives in: the car draw 0x444E00 takes each pass from
            // its own LOD object (`CarPass`). *(Until 2026-09-26 the port built LOD 1 alone and
            // drew every part of it in every view.)*
            let lods: std::collections::BTreeSet<u8> = g.primitives.iter().map(|p| p.lod).filter(|l| (1..=7).contains(l)).collect();
            for l in lods {
                // 0x43C8F8: the part's `"tr"` + the LOD, **no fallback** — without one the part
                // is drawn in the car's own frame and the whole matrix block, the translation and
                // rotation channels with it, is skipped (0x43C902 → 0x43CAFA). Such parts are
                // modelled in car space: the 356's `DoorWindow` at LOD 4 sits where LOD 1's does
                // through its `tr` (`lodtris` example; 314 articles of the shipped cars, none at
                // LOD 1).
                let tr = g.transforms.iter().find(|(i, _)| *i == l).map(|t| Mat4::from_cols_array(&t.1));
                let local = tr.unwrap_or(Mat4::IDENTITY);
                let info = PartInfo {
                    lod: l,
                    mask: g.base.as_ref().map_or(0, |b| b.draw_bytes[0]),
                    inside: g.base.as_ref().is_some_and(|b| b.draw_bytes[3] != 0),
                    inside_only: g.base.as_ref().is_some_and(|b| b.draw_bytes[4] != 0),
                    move_channel: if tr.is_some() { g.base.as_ref().map_or(-1, |b| b.move_channel) } else { -1 },
                };
                let rot_channel = if tr.is_some() { g.base.as_ref().map_or(0, |b| b.rot_channel) } else { 0 };
                let roof_frames: Vec<Vec<MeshData>> = frame_geometry
                    .iter()
                    .map(|gf| {
                        let mut sk = 0;
                        article_meshes(gf, l, &mut sk).into_iter().map(|m| m.mesh).collect()
                    })
                    .collect();
                if !roof_frames.is_empty() {
                    anim_length[anim] = anim_length[anim].max(roof_frames.len() as u32 + 1);
                }
                // the crush vectors and zones laid out as the meshes are: the damage geometry, and
                // the geometry with each vertex's position replaced by its zone (the `dz` lists
                // index the whole `vt#lod`, a primitive's window starting at `vt_first`)
                let crush: Option<(Vec<nfs_assets::mesh::MaterialMesh>, Vec<nfs_assets::mesh::MaterialMesh>)> = damage_geometry.as_ref().and_then(|dg| {
                    let dz = car.crp.damage_zones(&car.crp_data, a, l).ok()??;
                    let n = dz.zones.iter().flat_map(|z| z.1.iter()).map(|&i| i as usize + 1).max().unwrap_or(0);
                    let mut zone_of = vec![255u8; n];
                    for (z, list) in &dz.zones {
                        for &i in list {
                            zone_of[i as usize] = u8::try_from(*z).unwrap_or(255);
                        }
                    }
                    let mut gz = g.clone();
                    for p in gz.primitives.iter_mut().filter(|p| p.lod == l) {
                        let first = p.vt_first as usize;
                        for (i, q) in p.positions.iter_mut().enumerate() {
                            *q = [zone_of.get(first + i).copied().unwrap_or(255) as f32, 0.0, 0.0];
                        }
                    }
                    let mut sk = 0;
                    Some((article_meshes(dg, l, &mut sk), article_meshes(&gz, l, &mut sk)))
                });
                let mut parts = Vec::new();
                let mut part_channels: Vec<u8> = Vec::new();
                for (k, mm) in article_meshes(&g, l, &mut skipped).into_iter().enumerate() {
                    let mut mesh = mm.mesh;
                    // the port's own measures (the model box, the tyre radius, the track) come
                    // from the reference LOD alone
                    if l == lod {
                        for v in &mut mesh.vertices {
                            let p = (MODEL_TO_BODY * local).transform_point3(Vec3::from(v.position));
                            if !is_wheel {
                                min = min.min(p);
                                max = max.max(p);
                            } else {
                                // the tyre's rolling radius is its extent about the hub, not the distance
                                // to a corner of the wheel's quad: the 993's wheel mesh spans ±0.3148 in
                                // both y and z, and the `.sim` tyre (16\" rim, 205/55) has a radius of
                                // 16·12.7 + 205·0.55 = 315.95 mm
                                let lp = Vec3::from(v.position);
                                wheel_radius = wheel_radius.max(lp.y.abs()).max(lp.z.abs());
                            }
                        }
                    }
                    let material = material_for(r, mm.material);
                    let id = r.upload_mesh(&mesh);
                    if !roof_frames.is_empty() {
                        let mut positions = vec![mm_positions(&mesh)];
                        for f in &roof_frames {
                            match f.get(k) {
                                Some(m) if m.vertices.len() == mesh.vertices.len() => positions.push(mm_positions(m)),
                                _ => break,
                            }
                        }
                        morph[anim].push(MorphPart { mesh: id, base: mesh.vertices.clone(), frames: positions });
                    }
                    if let Some((dm, zm)) = &crush
                        && let (Some(d), Some(z)) = (dm.get(k), zm.get(k))
                        && d.mesh.vertices.len() == mesh.vertices.len()
                        && z.mesh.vertices.len() == mesh.vertices.len()
                    {
                        deform.push(DeformPart {
                            mesh: id,
                            base: mesh.vertices.clone(),
                            indices: mesh.indices.clone(),
                            delta: d.mesh.vertices.iter().map(|v| v.position).collect(),
                            zone: z.mesh.vertices.iter().map(|v| v.position[0] as u8).collect(),
                        });
                    }
                    parts.push((id, material, local));
                    part_channels.push(rot_channel);
                }
                if parts.is_empty() {
                    continue;
                }
                if is_wheel && tr.is_some() {
                    let pivot = MODEL_TO_BODY.transform_point3(local.w_axis.truncate());
                    if l == lod {
                        half_track = half_track.max(pivot.x.abs());
                    }
                    wheels.push(CarWheelModel { pivot, is_front: ty == 0x0D, is_left: local.w_axis.x > 0.0, channel: rot_channel, order: u32::MAX, info, parts });
                    wheel_art.push(ai);
                } else {
                    body_art.extend(std::iter::repeat_n(ai, parts.len()));
                    body_info.extend(std::iter::repeat_n(info, parts.len()));
                    body.extend(parts);
                    body_channel.extend(part_channels);
                }
            }
        }
        // 0x43C1B0's sort of each LOD's list (0x43C31B…0x43C38A): the VC6 `std::sort` — introsort
        // 0x43E710, then 0x43E910 / 0x43EA20 — over the records, `less` = the signed byte at `+8`
        // (`cmp al, cl; jge` in the median 0x43E77D, `cmp [esi+8], cl; jge` in the partition
        // 0x43E7B1, `cmp bl, cl; jge` 0x43E950 / `cmp dl, bl; jge` 0x43E9D3 / 0x43EA49 in the
        // inserts, `cmp dl, [edi-4]; jge` / `cmp dl, bl; jge` in the heap 0x43EA90 / 0x43EBE0).
        // Above 16 parts it is not stable, so equal keys draw in the order it leaves them.
        let mut order_of: std::collections::HashMap<(u8, usize), u32> = std::collections::HashMap::new();
        for (l, list) in lod_list.iter_mut().enumerate() {
            nfs_formats::stl_sort::vc6_sort(list, |a, b| a.1 < b.1);
            for (rank, &(ai, _)) in list.iter().enumerate() {
                order_of.insert((l as u8 + 1, ai), rank as u32);
            }
        }
        // A part with primitives at a LOD but not in that LOD's list (no `Base` record for it)
        // is not drawn by the original at all; the port keeps it, last. APPROXIMATION (kept from
        // before); `unlisted` counts them.
        let mut unlisted = 0usize;
        let mut order = |lod: u8, ai: usize| {
            order_of.get(&(lod, ai)).copied().unwrap_or_else(|| {
                unlisted += 1;
                u32::MAX
            })
        };
        let body_order: Vec<u32> = body_info.iter().zip(&body_art).map(|(info, &ai)| order(info.lod, ai)).collect();
        for (w, &ai) in wheels.iter_mut().zip(&wheel_art) {
            w.order = order(w.info.lod, ai);
        }
        tracing::info!(unlisted, "car part order");
        tracing::info!(body_parts = body.len(), wheels = wheels.len(), lights = lights.len(), roof_parts = morph[1].len(), roof_length = anim_length[1], anims = ?anim_length, skipped, pages = pages.len(), wheel_radius, half_track, "car uploaded");
        tracing::info!(patches = damage_patches.len(), night, "damage decals");
        Ok(Self {
            scale: 1.0,
            mirror: false,
            body,
            body_channel,
            body_order,
            body_info,
            lod_parts,
            lod_moves,
            material_light,
            shadowed,
            morph,
            anim_length,
            anim_time: [-1.0; ANIMS],
            roof_state: 0,
            driver: DriverAnim::default(),
            blended: [None; ANIMS],
            channel_turns: [0.0; 32],
            channel_yz: [[0.0; 2]; 32],
            dash: [0.0; 4],
            clock_ms: 0,
            move_spoiler_extent: tpg_f32(&car.tpg, "movespoilerextent"),
            headlight_extent: tpg_f32(&car.tpg, "headlightextent"),
            spoiler: ChannelEase::default(),
            headlight: ChannelEase::default(),
            door_axis: car
                .tpg
                .section("header")
                .and_then(|h| h.get_f32_list("dooraxis"))
                .filter(|v| v.len() >= 3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .unwrap_or(Vec3::Y),
            hood_extent: tpg_f32_or(&car.tpg, "hoodextent", 0.15),
            trunk_extent: tpg_f32_or(&car.tpg, "trunkextent", -0.15),
            hood: ChannelEase::default(),
            trunk: ChannelEase::default(),
            wheels,
            bbox_min: min,
            bbox_max: max,
            wheel_radius,
            half_track,
            lights,
            damage_pages: pages,
            damage_patches,
            page_textures,
            damage_shown: [0.0; 10],
            deform,
        })
    }

    /// A private copy of this model's damage-mutable GPU resources, for one traffic/cop car
    /// instance of a shared catalogue model (`RE/notes/car-model.md` §13.1's residual gap).
    ///
    /// The original shares one upload per catalogue type too (`R3DCar` slot 7, 0x4383C0), but
    /// recomputes a drawn car's decals/crush from *its own* raw damage and draws it — synchronously,
    /// through the software blitter 0x572B40 — before the next car (0x4383CC/0x4383D3, no
    /// car-kind gate), so two cars of one model always show correctly despite sharing the upload.
    /// This port's renderer instead records a frame's `DrawItem`s and reads each `MeshId`/
    /// `TextureId`/`MaterialId` once at `render()`; without this, two *simultaneously visible*,
    /// differently damaged instances of one shared model would alias to whichever instance called
    /// [`Self::update_damage`] last that frame.
    ///
    /// Cost: a fresh vertex+index buffer per crushable part ([`DeformPart`], `self.deform.len()`
    /// of them — a handful of body panels, not the whole model) and a fresh texture plus a fresh
    /// material per damage-decal page actually referenced by [`nfs_assets::car::DamagePatch`]
    /// (typically one or two pages) — once per car at grid setup, not per frame. Everything else
    /// (LOD/wheel/light geometry, the environment/specular textures, every non-decal material) is
    /// `self.clone()`d as plain ids — cheap, no GPU allocation — and stays shared with the
    /// template this was instantiated from.
    pub fn instantiate(&self, r: &mut Renderer) -> Self {
        let mut c = self.clone();
        // the crushable parts: a fresh buffer holding the pristine (undamaged) positions —
        // `update_damage` overwrites it once this instance takes zone damage. Re-uploaded from
        // `base`/`indices`, not copied GPU-side, so a template already crushed by another
        // instance is not what this instance starts from.
        for p in &mut c.deform {
            p.mesh = r.upload_mesh(&MeshData { vertices: p.base.clone(), indices: p.indices.clone() });
        }
        // the decal pages any zone can touch: a fresh texture (pristine pixels) plus a fresh
        // material for every existing material that samples it, so its bind group is not shared
        // with the template's or another instance's
        let mut touched: Vec<usize> = c.damage_patches.iter().map(|p| p.page).collect();
        touched.sort_unstable();
        touched.dedup();
        if !touched.is_empty() {
            let mut tex_remap: HashMap<nfs_renderer::TextureId, nfs_renderer::TextureId> = HashMap::new();
            for page_index in touched {
                let (Some(base), Some(&old_tex)) = (c.damage_pages.get(page_index), c.page_textures.get(page_index)) else { continue };
                let new_tex = r.upload_texture(&TextureData::single(base.width, base.height, base.rgba.clone()));
                c.page_textures[page_index] = new_tex;
                tex_remap.insert(old_tex, new_tex);
            }
            let mut mat_remap: HashMap<MaterialId, MaterialId> = HashMap::new();
            let mut remap = |mid: &mut MaterialId, material_light: &mut HashMap<MaterialId, nfs_formats::crp::LayerLight>, shadowed: &mut std::collections::HashSet<MaterialId>| {
                if let Some(&nid) = mat_remap.get(mid) {
                    *mid = nid;
                    return;
                }
                let Some(&new_tex) = r.material_texture(*mid).as_ref().and_then(|t| tex_remap.get(t)) else { return };
                let nid = r.duplicate_material_texture(*mid, new_tex);
                if let Some(l) = material_light.get(mid).copied() {
                    material_light.insert(nid, l);
                }
                if shadowed.contains(mid) {
                    shadowed.insert(nid);
                }
                mat_remap.insert(*mid, nid);
                *mid = nid;
            };
            for (_, mid, _) in &mut c.body {
                remap(mid, &mut c.material_light, &mut c.shadowed);
            }
            for w in &mut c.wheels {
                for (_, mid, _) in &mut w.parts {
                    remap(mid, &mut c.material_light, &mut c.shadowed);
                }
            }
        }
        // this instance starts with no damage shown yet, matching a just-uploaded model — the
        // next `update_damage` call (before this instance's own `draw_items`) fills it in
        c.damage_shown = [0.0; 10];
        c
    }

    /// **0x438230(0)**, which `R3DCar` slot 7 runs every frame (0x4383CC): when any of the ten
    /// [`zone_damage`] values differs from the last ones kept at `+0x6A8` (an exact `fcomp`,
    /// 0x438240…0x438259), keep the new ones (0x4382A4…0x4382B7) and recompose the texture pages
    /// the damage decals touch from the car's **raw** zone damage `car+0x4E4` (0x4451E0 → 0x448010,
    /// `RE/notes/car-model.md` §11). `own_car`: the car the camera follows, whose side and roof
    /// zones are capped at 0 here — their changes alone do not trigger it. *(Until 2026-09-26 the
    /// port rebuilt whenever a zone's damage crossed a whole per cent.)*
    ///
    /// Before the pages, **0x43E390** crushes the body for every LOD (0x438273…0x4382A2): at
    /// `CARDETAIL` (`detail`) above 0 (0x43E3E2), each part with `Base+78 & 8`, per `dz` zone whose
    /// value changed (0x438390): `f = value · 0.02`, 1 above 1 (0x43E54D…0x43E571), and each of the
    /// zone's vertices `vt + f · vt#(0x8000|lod)` (0x53C5D0, 0x43E5D4…0x43E617). *(Until 2026-09-26
    /// the port never moved a vertex: damage was decals alone.)* The byte `part+0x50` it sets
    /// above `f = 0.5` (0x43E584) is not read here — its reader is not found.
    pub fn update_damage(&mut self, r: &mut Renderer, damage: &[f32; 10], own_car: bool, detail: u8) {
        let now = zone_damage(damage, own_car);
        if now == self.damage_shown {
            return;
        }
        let changed: [bool; 10] = std::array::from_fn(|z| now[z] != self.damage_shown[z]);
        self.damage_shown = now;
        if detail > 0 {
            let f: [f32; 10] = std::array::from_fn(|z| {
                let f = now[z] * 0.02;
                if f <= 1.0 { f } else { 1.0 }
            });
            for p in &self.deform {
                if !p.zone.iter().any(|&z| changed.get(z as usize).copied().unwrap_or(false)) {
                    continue;
                }
                let mut v = p.base.clone();
                for (k, out) in v.iter_mut().enumerate() {
                    let Some(&k_f) = f.get(p.zone[k] as usize) else { continue };
                    let d = p.delta[k];
                    out.position = [out.position[0] + k_f * d[0], out.position[1] + k_f * d[1], out.position[2] + k_f * d[2]];
                }
                r.update_mesh_vertices(p.mesh, &v);
            }
        }
        if self.damage_patches.is_empty() {
            return;
        }
        let touched: Vec<usize> = {
            let mut v: Vec<usize> = self.damage_patches.iter().map(|p| p.page).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        for page_index in touched {
            let Some(base) = self.damage_pages.get(page_index) else { continue };
            let Some(&tex) = self.page_textures.get(page_index) else { continue };
            let mut page = base.clone();
            for patch in self.damage_patches.iter().filter(|p| p.page == page_index) {
                nfs_assets::car::blend_patch(&mut page, patch, damage[patch.zone as usize]);
            }
            r.update_texture(tex, &TextureData::single(page.width, page.height, page.rgba));
        }
    }

    /// Slot 9's race branch for the parts this port animates (0x43891E…0x438B27, the race only —
    /// `[0x65B928]` set skips it) and slot 6's headlight easer.
    ///
    /// **The pop-up wing is animation 2**, the `SpoilerW` morph — not channel 16, which this said
    /// until 2026-09-22 (channel 16 is written only in the front end, from the roof: see
    /// [`Self::follow_roof`]). Above `[0x5B3698]` = 22.222 m/s (**80 km/h**) of `car+0xEC4`, and
    /// only once the easer at `car+0x64 + 0x170` has run its course (0x438950), it restarts from
    /// where it is toward animation 2's length (`[car+0x59C]`, 0x43895E) over **2000 ms**; below
    /// `[0x5B3694]` = 13.889 (**50**) the same toward 0 (0x4389D1…0x438A57). It re-arms whenever
    /// the gate holds and the last travel is over, even toward the value it already has — so a
    /// retract can wait out an idle travel. The value (0x438A5E…0x438AD1: `from + (to − from)·t`,
    /// `to` once `t` passes 1) goes to 0x4380D0(2, ·).
    ///
    /// **The aerial is animation 7**, and only a `.TINYCAR` moves it (0x438ADE…0x438B27):
    /// `min(0.067 · car+0xEC4, 1)` (`[0x5B3690]`), 0 in reverse (`car+0xD82 == 0`).
    ///
    /// **The headlights**, channel 17, are slot 6's easer at 0x43950E…0x439620: a **1000 ms**
    /// travel from the car's headlight bit — which `Car_init` freezes at 0x410C74 as
    /// `word[0x628A28] ? 3 : 0`, so it is the track's night flag and never changes during a race.
    pub fn update_animation(&mut self, r: &mut Renderer, dt_ms: u32, speed_mps: f32, headlights_on: bool, tiny: bool, reverse: bool) {
        // the wing's easer: advance, then (0x43892B) re-arm when the gate holds and it is done
        let e = &mut self.spoiler;
        if e.elapsed < e.duration {
            e.elapsed = e.elapsed.saturating_add(dt_ms);
        }
        let len = self.anim_length[2] as f32;
        let want = if speed_mps > 22.222 {
            Some(len)
        } else if speed_mps < 13.889 {
            Some(0.0)
        } else {
            None
        };
        if let Some(want) = want
            && e.elapsed >= e.duration
        {
            // 0x438970…0x4389CC: the travel's current value becomes the new start
            e.from = e.to;
            e.to = want;
            e.elapsed = 0;
            e.duration = 2000;
        }
        let v = if e.duration == 0 || e.elapsed >= e.duration {
            e.to
        } else {
            e.from + (e.to - e.from) * (e.elapsed as f32 / e.duration as f32)
        };
        self.set_anim(r, 2, v);
        if tiny {
            let t = if reverse { 0.0 } else { (speed_mps * 0.067).min(1.0) };
            self.set_anim(r, 7, t);
        }
        if self.headlight_extent != 0.0 {
            let e = &mut self.headlight;
            let want = if headlights_on { 1.0 } else { 0.0 };
            if e.elapsed < e.duration {
                e.elapsed = e.elapsed.saturating_add(dt_ms);
            } else if e.to != want {
                e.from = e.to;
                e.to = want;
                e.elapsed = 0;
                e.duration = 1000;
            }
            let t = if e.duration == 0 { 1.0 } else { (e.elapsed as f32 / e.duration as f32).min(1.0) };
            let v = e.from + (e.to - e.from) * t;
            let x = self.headlight_extent;
            self.set_channel(17, v * x);
        }
    }

    /// Slot 9's driver half (0x438B2C…0x439352, the race only): the right arm (animation 4), the
    /// left arm (5) and the body (6), `CarModel`'s eight keyframe players, and `R3DCar+0x564` /
    /// `+0x566`. `RE/notes/car-model.md` "Slot 9" has the whole of it.
    pub fn update_driver(&mut self, r: &mut Renderer, dt_ms: u32, d: &DriverInputs) {
        // under detail 1 the driver is posed and nothing else runs (0x438B2C…0x438B5A)
        if d.detail < 1 {
            self.set_anim(r, 4, 4.5);
            self.set_anim(r, 5, 4.5);
            self.set_anim(r, 6, 5.0);
            return;
        }
        self.driver.clock_ms += dt_ms as u64;
        let now = self.driver.clock_ms;
        for p in &mut self.driver.players {
            p.step(now);
        }
        // the steering frame: 23·ch5.z + 4.5 (0x438C4F…0x438C6B), 0…9 as a table index
        // (0x438CC8…0x438D7E: under 0 → 0, over 9 → 9, else fistp(f − 0.5))
        let f = d.wheel_turns.clamp(-4.5 / 23.0, 4.5 / 23.0) * 23.0 + 4.5;
        let index = |f: f32| -> usize {
            if f < 0.0 {
                0
            } else if f > 9.0 {
                9
            } else {
                (f - 0.5).round_ties_even().max(0.0) as usize
            }
        };
        self.set_anim(r, 5, f);
        // a gesture already running is copied into the arm's record (0x438C7A → 0x439035);
        // otherwise the gear change may start one (0x438C92…0x438E18), then the handbrake
        // (0x438E3A…0x438FEA), and on the frame one starts the arm is not written (0x438FF4)
        let arm = self.driver.players[4];
        if arm.active() {
            self.set_anim_keys(r, 4, arm.frame, arm.next, arm.frac);
        } else {
            if d.shift_timer > 0 && d.racing {
                let ms = (d.shift_timer as i64 * 2000 / 32) as u64;
                self.driver.players[4].start(GEAR_GESTURES[index(f)], now, ms);
            }
            if !self.driver.players[4].active() && d.handbrake != self.driver.handbrake {
                self.driver.handbrake = d.handbrake;
                let i = index(f);
                let t: &'static [u32] = if d.handbrake { &HANDBRAKE_ON[i] } else { &HANDBRAKE_OFF[i] };
                self.driver.players[4].start(t, now, 125);
            }
            if !self.driver.players[4].active() {
                self.set_anim(r, 4, if self.driver.handbrake { 19.0 } else { f });
            }
        }
        // the body: 2 − 0.1·car+0xE08, the lateral acceleration's dead band 0.1 (0x439091…0x4390D5)
        let lat = if d.lat_acc.abs() < 0.1 { 0.0 } else { -d.lat_acc };
        let b = 0.1 * lat + 2.0;
        let body = self.driver.players[6];
        if body.active() {
            // 0x4390DA → 0x4392F1
            self.set_anim_keys(r, 6, body.frame, body.next, body.frac);
            return;
        }
        if d.reverse != self.driver.reverse {
            self.driver.reverse = d.reverse;
            let i = if b < 0.0 {
                0
            } else if b > 4.0 {
                4
            } else {
                (b + 0.5) as usize
            };
            let t = if d.reverse { REVERSE_ON[i] } else { REVERSE_OFF[i] };
            self.driver.players[6].start(t, now, 250);
        }
        if self.driver.players[6].active() {
            // 0x439216: started this frame, not written
        } else if self.driver.reverse {
            // looking back over the shoulder (0x43922E…0x439241)
            self.set_anim(r, 6, 7.0);
        } else if d.long_acc <= -0.1 {
            // braking: the body frame toward frame 5 by 0.1·|car+0xE04| (0x43924D…0x4392D9)
            let frame = (b - 0.5).round_ties_even().max(0.0) as u32;
            self.set_anim_keys(r, 6, frame, 5, -0.1 * d.long_acc);
        } else {
            self.set_anim(r, 6, b);
        }
    }

    /// Slot 9's front-end branch for channel **16**, `MoveSpoiler` (0x438751…0x4387CC): it
    /// follows the **roof**, animation 1 (`car+0x57C` frame + `+0x584` fraction = its time `v`,
    /// `+0x588` its length `L`): `v · extent` over the first frame, the full `movespoilerextent`
    /// until the last, `(L − v) · extent` over it. So the 996's and the Boxster's rear lid turns
    /// out of the way while the roof folds, and back.
    pub fn follow_roof(&mut self) {
        if self.move_spoiler_extent == 0.0 || self.anim_length[1] == 0 {
            return;
        }
        let v = self.anim_time[1].max(0.0);
        let l = self.anim_length[1] as f32;
        let e = self.move_spoiler_extent;
        let t = if (0.0..1.0).contains(&v) {
            v * e
        } else if v < l - 1.0 {
            e
        } else {
            (l - v) * e
        };
        self.set_channel(16, t);
    }

    /// The rotation a channel's three floats compose to: `Rx·Ry·Rz`, each angle in **turns**
    /// (0x43C9C0 / 0x43C9D7 / 0x43C9F8, all `× 360` into the axis-angle build 0x539BD0, and
    /// accumulated in that order by the two 0x539AD0 multiplies at 0x43CA26 / 0x43CA38).
    ///
    /// The port only ever fills the x term, so the y and z rotations are identity here; they are
    /// written this way round so a later channel that uses them lands in the right order.
    fn channel_matrix(&self, n: u8) -> Mat4 {
        let t = self.channel_turns.get(n as usize).copied().unwrap_or(0.0);
        let [y, z] = self.channel_yz.get(n as usize).copied().unwrap_or([0.0; 2]);
        if t == 0.0 && y == 0.0 && z == 0.0 {
            return Mat4::IDENTITY;
        }
        // **Channels 6 and 7 are the doors** (0x43CA5A): they turn about the `.tpg` `dooraxis`,
        // normalised (0x5328D0), with its x negated for 7, by the channel's *y* in turns — through
        // 0x5326C0, the same axis-angle build 0x539BD0 uses for the others, so the same
        // convention. The door value is kept in `channel_turns` like the rest.
        if n == 6 || n == 7 {
            let mut a = self.door_axis;
            if n == 7 {
                a.x = -a.x;
            }
            return Mat4::from_axis_angle(a.normalize_or(Vec3::Y), t * std::f32::consts::TAU);
        }
        // Rx·Ry·Rz, in that order (0x43CA26 / 0x43CA38); the y and z senses follow the x one,
        // MEDIUM until the needles and the steering wheel have been seen turning the right way
        use std::f32::consts::TAU;
        Mat4::from_rotation_x(t * TAU) * Mat4::from_rotation_y(y * TAU) * Mat4::from_rotation_z(z * TAU)
    }

    /// A lamp's two frames in the body frame (lamp's body-frame data → car body), as the effect
    /// pass 0x43D800 builds them per `ef` record: the **glare's** `Rch · tr` in front of the car's
    /// matrix (0x43D8D1…0x43DA46; `Rch` = `Rx·Ry·Rz` of the channel's three turns, 0x43D952…
    /// 0x43D9FE) — the sprite, the reflection and the beam's rings go through it — and the
    /// **beam's** own, the same with the x term left out (0x43DA03 resets to identity and
    /// multiplies only `Ry·Rz`, 0x43DA0F…0x43DA46), through which 0x43D4F0 carries only the
    /// forward `(0, 0, −1)` (0x43D5A3…0x43D5F4): a pop-up headlight's glare tips with the lamp,
    /// its beam still points down the road. The three floats compose straight, with no door
    /// case (unlike the body's 0x43CA5A); the order and senses are `channel_matrix`'s.
    pub fn lamp_frames(&self, l: &CarLight) -> (Mat4, Mat4) {
        use std::f32::consts::TAU;
        let n = l.channel as usize;
        let (x, [y, z]) = if n == 0 {
            (0.0, [0.0; 2])
        } else {
            (self.channel_turns.get(n).copied().unwrap_or(0.0), self.channel_yz.get(n).copied().unwrap_or([0.0; 2]))
        };
        let yz = Mat4::from_rotation_y(y * TAU) * Mat4::from_rotation_z(z * TAU);
        let body = |m: Mat4| MODEL_TO_BODY * l.frame * m * MODEL_TO_BODY;
        (body(Mat4::from_rotation_x(x * TAU) * yz), body(yz))
    }

    /// Whether the car has pop-up headlights: `headlightextent`, `R3DCar+0x70`, non-zero only on
    /// the 914, 928 and 944. 0x439627 tests it: only then do the headlight slots take the ramp's
    /// value over it (0x43963B…0x43969E); with 0 they are 1.0 or 0 at once (0x43972D…0x43973E).
    pub fn has_headlight_extent(&self) -> bool {
        self.headlight_extent != 0.0
    }

    /// The dash needles' calibration from the car's `nfs5.car` record (see `dash`).
    pub fn set_dash(&mut self, tach0: f32, tach1000: f32, speedo0: f32, speedo100: f32) {
        self.dash = [tach0, tach1000, speedo0, speedo100];
    }

    /// Slot 9's race channels (0x438461…0x43874C) other than the wheels' own draw: the steering
    /// wheel, the two needles, the non-spinning wheel parts and the siren.
    ///
    /// `steer` is each front wheel's `+0xB4` over 216 (turns, negated in a mirrored race); `rpm`
    /// is `car+0xDB4`, `speed` `car+0xEC4`; `dt_ms` advances the clock 0x53C2E0 stands for.
    pub fn update_race_channels(&mut self, dt_ms: u32, steer: [f32; 2], spin: [f32; 4], rpm: f32, speed: f32) {
        self.clock_ms += dt_ms as u64;
        // `TIMER_getTicks` (0x53C2E0 → `[0x6B7C7C]`) counts at 128 Hz (`timing.md`)
        let clock = (self.clock_ms * 128 / 1000) as u32;
        let yz = &mut self.channel_yz;
        // 1, 2 `.y` = the fronts' steer; 1…4 `.x` = −the wheel's turn (`+0x88`); `.z` is the
        // wheel's `+0x70`, which this port's suspension does not produce (left at 0)
        for (k, s) in steer.iter().enumerate() {
            yz[1 + k][0] = *s;
        }
        for k in 0..4 {
            self.channel_turns[1 + k] = -spin[k];
        }
        // 11 `.z`: `.TACH0RPM + .TACH1000RPM · rpm · 0.001` (0x43853C…0x438561); 12 `.z`:
        // `.SPEEDO0KM + .SPEEDO100KM · car+0xEC4 · 0.144` (0x43856C…0x438593)
        let [t0, t1, s0, s1] = self.dash;
        yz[11][1] = t0 + t1 * rpm * 0.001;
        yz[12][1] = s0 + s1 * speed * 0.144;
        // 5 `.z`: the steering wheel, `ch1.y + ch2.y` within ±4.5/23 (0x43858F…0x4385FB)
        yz[5][1] = (steer[0] + steer[1]).clamp(-4.5 / 23.0, 4.5 / 23.0);
        // 18…21: channels 1…4 with `.x` = 0 — the tread and the shadow turn but do not spin
        // (0x438669…0x438726)
        for k in 0..4 {
            yz[18 + k] = yz[1 + k];
            self.channel_turns[18 + k] = 0.0;
        }
        // 22 `.y`: 0x53C2E0() · 1/128 — the cop's siren glare, a turn a second (0x43872C…0x43874C)
        yz[22][0] = (clock as f32 * 0.007_812_5).fract();
    }

    /// Channel 22 alone — the one slot 9 channel that reads no car state, only the clock — for the
    /// models every car of a type shares (traffic, the cops): their sirens turn as one, which is
    /// what the original's per-car copies do too.
    pub fn update_clock_channels(&mut self, dt_ms: u32) {
        self.clock_ms += dt_ms as u64;
        let clock = (self.clock_ms * 128 / 1000) as u32;
        self.channel_yz[22][0] = (clock as f32 * 0.007_812_5).fract();
    }

    /// The lids' two easers, 0x439A00 / 0x439AA0 (hood) and 0x439B30 / 0x439BD0 (engine lid):
    /// a travel already running is left alone (0x439A1E); otherwise it restarts from where it
    /// ended toward the extent (`open`) or 0, over **1000 ms** (0x439A81).
    pub fn set_lid(&mut self, hood: bool, open: bool) {
        let (e, target) = if hood {
            (&mut self.hood, if open { self.hood_extent } else { 0.0 })
        } else {
            (&mut self.trunk, if open { self.trunk_extent } else { 0.0 })
        };
        if e.elapsed < e.duration {
            return;
        }
        e.from = e.to;
        e.to = target;
        e.elapsed = 0;
        e.duration = 1000;
    }

    /// Advance the lid easers and write channels 8 and 9 (slot 9, 0x4387CE…0x43891C): the easer's
    /// `from + (to − from)·t`, and `to` once it has run its course. Front-end cars only; the race
    /// never opens a lid.
    pub fn update_lids(&mut self, dt_ms: u32) {
        for (n, e) in [(8u8, self.hood), (9u8, self.trunk)] {
            let v = if e.duration == 0 || e.elapsed >= e.duration {
                e.to
            } else {
                e.from + (e.to - e.from) * (e.elapsed as f32 / e.duration as f32)
            };
            self.set_channel(n, v);
        }
        for e in [&mut self.hood, &mut self.trunk] {
            if e.elapsed < e.duration {
                e.elapsed = e.elapsed.saturating_add(dt_ms);
            }
        }
    }

    /// Set one animation channel, in turns. Channel **16** is `MoveSpoiler`, **17** the pop-up
    /// `HeadLight`; both are `Base+75` bindings confirmed against every shipped model.
    pub fn set_channel(&mut self, n: u8, turns: f32) {
        if let Some(slot) = self.channel_turns.get_mut(n as usize) {
            *slot = turns;
        }
    }

    /// Set all three turns of one channel — `R3DCar` 0x443F30(n, &v), which copies the vector
    /// into `object + 0x2DC + 12n` whole.
    pub fn set_channel_vec(&mut self, n: u8, v: [f32; 3]) {
        if let (Some(x), Some(yz)) = (self.channel_turns.get_mut(n as usize), self.channel_yz.get_mut(n as usize)) {
            *x = v[0];
            *yz = [v[1], v[2]];
        }
    }

    /// Write `src` (RGBA, `w` × `h`) into texture page `page` (0-based, `tpage<page+1>` — the
    /// order of the model's page array `[R3DCar+0x78]`, 0x34 bytes an entry, 0x442F1B) at `(x, y)`
    /// the way the software blitter stores a texel in the page's format
    /// ([`nfs_assets::car::blitter_texel`]), clipped to the page, and upload it — what 0x534480
    /// (the page as the destination), a blit and 0x443C80 (the page's refresh, 0x447AF0) do. The
    /// page stays written: a later blit lands over it, as on the original's surface.
    #[allow(clippy::too_many_arguments)]
    pub fn blit_page(&mut self, r: &mut Renderer, page: usize, src: &[u8], w: u32, h: u32, x: i64, y: i64) {
        let (Some(p), Some(&tex)) = (self.damage_pages.get_mut(page), self.page_textures.get(page)) else { return };
        for row in 0..h as i64 {
            let py = y + row;
            if py < 0 || py >= p.height as i64 {
                continue;
            }
            for col in 0..w as i64 {
                let px = x + col;
                if px < 0 || px >= p.width as i64 {
                    continue;
                }
                let Some(s) = src.get(((row as u32 * w + col as u32) * 4) as usize..).and_then(|s| s.get(..4)) else { continue };
                let t = nfs_assets::car::blitter_texel(p.bpp, [s[0], s[1], s[2], s[3]]);
                let k = ((py as u32 * p.width + px as u32) * 4) as usize;
                p.rgba[k..k + 4].copy_from_slice(&t);
            }
        }
        r.update_texture(tex, &TextureData::single(p.width, p.height, p.rgba.clone()));
    }

    /// A channel's three turns — 0x443F10(n).
    pub fn channel_vec(&self, n: u8) -> [f32; 3] {
        let x = self.channel_turns.get(n as usize).copied().unwrap_or(0.0);
        let [y, z] = self.channel_yz.get(n as usize).copied().unwrap_or([0.0; 2]);
        [x, y, z]
    }

    /// Draw items for a car at `car_to_world` (body frame → world). `wheel_spin` in turns,
    /// `steer` in turns for the front wheels, positive to the right — take it from
    /// `Vehicle::wheel_steer_turns`, not from the raw `Wheel::steer`. `travel` is the car's four
    /// translation channels' `.y` ([`CarLodState::travel`]) and `passes` the car draw's calls
    /// ([`race_passes`], [`in_car_passes`], [`FRONT_END_PASSES`]).
    ///
    /// Each pass is one `R3DCarLOD::draw` 0x43C830: the parts of its LOD whose `Base+78` meets its
    /// mask ([`PartInfo::in_pass`]) in `Base+70` order (0x43C1B0's sort of that LOD's list), and
    /// the passes one after the other as 0x444E00 makes them — the driver, then the body, then the
    /// wheels.
    pub fn draw_items(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2], travel: [f32; 4], passes: &[CarPass]) -> Vec<DrawItem> {
        // 0x4433D2 / 0x4433BC: `(−m·s, s, −s)` — `MODEL_TO_BODY` scaled, its x turned back in a
        // mirrored race; and the steer channels negated there (0x438471)
        let car_to_world = car_to_world * Mat4::from_scale(Vec3::new(if self.mirror { -self.scale } else { self.scale }, self.scale, self.scale));
        let steer = if self.mirror { [-steer[0], -steer[1]] } else { steer };
        // the translation channel `Base+72` (0x43C945…0x43C982): `T(object + 0x6D0 + 12m)` into the
        // part's matrix after its `tr` and before its rotation — the only float of it written is
        // `.y` (0x43E6E1…0x43E6F1), so a lift along the model's (and the body's) up
        let lift = |m: i8| usize::try_from(m).ok().and_then(|m| travel.get(m).copied()).map_or(Mat4::IDENTITY, |y| Mat4::from_translation(Vec3::new(0.0, y, 0.0)));
        let mut out = Vec::with_capacity(self.body.len() / 4 + 4 * 8);
        for pass in passes {
            let mut items: Vec<(u32, DrawItem)> = Vec::new();
            for (i, (mesh, material, local)) in self.body.iter().enumerate() {
                let Some(info) = self.body_info.get(i).filter(|info| info.in_pass(pass)) else { continue };
                // A part bound to a rotation channel turns about **its own origin**: 0x43CA19…
                // 0x43CA38 compose Rx·Ry·Rz and 0x43CA43 multiplies that into the part's transform,
                // so the rotation is on the right of the part's own matrix.
                let local = &(*local * lift(info.move_channel));
                let m = match self.body_channel.get(i).copied().unwrap_or(0) {
                    0 => *local,
                    // 18…21 are channels 1…4 with `.x` cleared (0x438669…0x438726) — the wheel shadow
                    // and the parts that turn with the steer but do not spin. Taking them from the
                    // caller's `steer` instead of this model's own channels is what lets the traffic
                    // and the cops have them: those share one upload per car type.
                    n @ 18..=21 if n <= 19 => *local * Mat4::from_rotation_y(steer[(n - 18) as usize] * std::f32::consts::TAU),
                    20..=21 => *local,
                    // 5, the steering wheel: `.z = ch1.y + ch2.y` within ±4.5/23 (0x43858F…0x4385FB).
                    // Off the caller's `steer` for the same reason as 18…21 — the traffic and the cops
                    // share one model per car type and would otherwise hold the wheel straight.
                    5 => *local * Mat4::from_rotation_z((steer[0] + steer[1]).clamp(-4.5 / 23.0, 4.5 / 23.0) * std::f32::consts::TAU),
                    n => *local * self.channel_matrix(n),
                };
                items.push((self.body_order.get(i).copied().unwrap_or(u32::MAX), DrawItem::new(*mesh, *material, car_to_world * MODEL_TO_BODY * m)));
            }
            for w in self.wheels.iter().filter(|w| w.info.in_pass(pass)) {
                let idx = match (w.is_front, w.is_left) {
                    (true, true) => 0,
                    (true, false) => 1,
                    (false, true) => 2,
                    (false, false) => 3,
                };
                // The original draws its wheels through channels 1…4 like every other moving part:
                // `.x = −(wheel+0x88)` (0x438600…0x438660), in **model** space, which is the body
                // frame turned half a turn about y — so `+turn` about the body's x. This read
                // `−turn` until 2026-09-22 and rolled the wheels backwards.
                // a wheel article on channels 18…21 takes the steer but **not** the spin: `.x` of those
                // channels is cleared at 0x438669…0x438726
                let spin = if (18..=21).contains(&w.channel) {
                    Mat4::IDENTITY
                } else {
                    Mat4::from_rotation_x(wheel_spin[idx] * std::f32::consts::TAU)
                };
                // **Positive steer is a right turn** (the right arrow gives +1, and the car yaws right
                // on it), and in the body frame +X is right and +Z forward — so the wheel turns by
                // `+steer`: `from_rotation_y(θ)` carries forward (0,0,1) to (sin θ, 0, cos θ), toward
                // +X. This said `-steer` until 2026-09-21 and drew the front wheels turned the wrong
                // way while the car steered correctly — found in play.
                // `.y` is the wheel's own `+0xB4` over 216 (channels 1 and 2, 0x438461…0x4384EB); a
                // turn about y is the same in both frames
                let st = if w.is_front { Mat4::from_rotation_y(steer[if w.is_left { 0 } else { 1 }] * std::f32::consts::TAU) } else { Mat4::IDENTITY };
                for (mesh, material, local) in &w.parts {
                    // local has translation = pivot (every wheel `tr` is a pure translation); rotate
                    // about the pivot in model space, the lift before it: `tr · T · R`
                    let pivot = local.w_axis.truncate();
                    let m = lift(w.info.move_channel) * Mat4::from_translation(pivot) * MODEL_TO_BODY * st * spin * MODEL_TO_BODY.inverse() * Mat4::from_translation(-pivot) * *local;
                    items.push((w.order, DrawItem::new(*mesh, *material, car_to_world * MODEL_TO_BODY * m)));
                }
            }
            // the parts in the order 0x43C1B0's sort left the LOD's list (`body_order`, read
            // 2026-09-27: the VC6 `std::sort` of the whole list at load, reproduced by `vc6_sort`),
            // which 0x43C830 walks skipping the parts not in the pass (0x43C8AC…0x43C8E9) — so
            // the pass's parts keep their places in the sorted whole list. One article's meshes
            // share a place and keep their own order (the stable sort).
            items.sort_by_key(|(k, _)| *k);
            out.extend(items.into_iter().map(|(_, it)| it));
        }
        out
    }

    /// The class count the LOD pick scales by (0x43A0DC…0x43A117): the 1-based number of the first
    /// LOD whose part list is empty (`R3DCarLOD` slot 6, 0x43ADF0, `+0x10`), 7 when none is.
    fn lod_span(&self) -> i32 {
        self.lod_parts.iter().position(|&n| n == 0).map_or(7, |k| k as i32 + 1)
    }

    /// `R3DCar` slot 13, **0x439F90**, in a race past its camera test (0x43A060…0x43A3B0): the LOD
    /// class `+0x2C0`, or `None` when the car covers 3 pixels or less and is not drawn (0x43A2E8,
    /// the caller 0x443192).
    ///
    /// The class starts from `0x5CD5B8[CARDETAIL]` = 3, 3, 2 (and 1 more on a machine under 40 MB,
    /// `GlobalMemoryStatus` 0x53C3D0 — never here); a replay (`[0x657424] >= 2`) with `CARDETAIL`
    /// above 0 takes 1 off, an opponent (`car+0x52C & 8`) at `CARDETAIL` 0 adds 1, and a traffic car
    /// (`& 0x10`) starts from 0. The car's box — `car+0x3A8` as `(x, y, z)`, `(−2x, y, z)` and
    /// `(x, y, −z)` about the draw origin, through the car's rows (0x43A18D…0x43A206) — is projected
    /// (0x539700), and `size` = the bit-trick roots of the two edges' squared screen lengths from
    /// the first point, × 640 / screen width (0x43A26A…0x43A2E5). Then `t = max(1 − rsqrt₁₂₀ · √size,
    /// 0) · (N − bias − 1)` with the bit-trick `rsqrt(120)` (0x5F400000 − bits/2) and `N`
    /// [`Self::lod_span`]; `t · (t + 1) / 2` for all but traffic; the class is `bias + fistp(t −
    /// 0.5)`, within `0 … 7 − (CARDETAIL > 1 ? 2 : 1)` (0x43A305…0x43A3B0).
    #[allow(clippy::too_many_arguments)]
    pub fn lod_class(&self, view: &LodView, origin: Vec3, rotation: glam::Quat, half: Vec3, detail: u8, kind: CarKind, replay: bool) -> Option<u8> {
        let cd = detail as i32;
        let mut bias = CLASS_BIAS[(cd as usize).min(3)];
        if replay && cd > 0 {
            bias -= 1;
        } else if cd == 0 && kind == CarKind::Opponent {
            bias += 1;
        }
        if kind == CarKind::Traffic {
            bias = 0;
        }
        let span = self.lod_span() - bias;
        let screen = |p: Vec3| {
            let c = view.view_projection * (origin + rotation * p).extend(1.0);
            glam::Vec2::new(c.x / c.w * view.width * 0.5, c.y / c.w * view.height * 0.5)
        };
        let s0 = screen(Vec3::new(half.x, half.y, half.z));
        let s1 = screen(Vec3::new(-2.0 * half.x, half.y, half.z));
        let s2 = screen(Vec3::new(half.x, half.y, -half.z));
        let d1 = (s1 - s0).length_squared();
        let d2 = (s2 - s0).length_squared();
        // x87: the two roots summed and scaled on the stack, `fst` to a float for the root below
        let size = (bit_root(d1) as f64 + bit_root(d2) as f64) * (640.0 / view.ref_width as f64);
        if size <= 3.0 {
            return None;
        }
        let rsqrt_120 = f32::from_bits(0x5F40_0000 - (120f32.to_bits() >> 1));
        let mut t = 1.0 - rsqrt_120 as f64 * bit_root(size as f32) as f64;
        if t < 0.0 {
            t = 0.0;
        }
        t *= (span - 1) as f64;
        if kind != CarKind::Traffic {
            t *= (t + 1.0) * 0.5;
        }
        let k = ((t - 0.5) as f32).round_ties_even() as i32;
        let max = 7 - if cd > 1 { 2 } else { 1 };
        let class = bias + k;
        Some(if class >= max { max } else { class.max(0) } as u8)
    }

    /// The translation channels after a class: 0x43E670 over the LOD objects `0 … class − 1`
    /// (0x43A3B6…0x43A3F8; the driver view jumps there with the class it had), each writing channel
    /// `m`'s `.y` from wheel `[0x5B36F0 + 4m]`'s `+0x68` — `(0, 1, 2, 3)`, or `(1, 0, 3, 2)` in a
    /// mirrored race (`[0x65742C]`, 0x43E6B1…0x43E6C2) — while its gate says one of the LOD's parts
    /// rides it. So a car at class 0 keeps what it had: none of its LODs is walked.
    pub fn update_travel(&self, state: &mut CarLodState, compression: [f32; 4], mirror: bool) {
        let map = if mirror { [1, 0, 3, 2] } else { [0, 1, 2, 3] };
        for moves in self.lod_moves.iter().take(state.class as usize) {
            for m in 0..4 {
                if moves[m] {
                    state.travel[m] = compression[map[m]];
                }
            }
        }
    }
}

/// One mesh of a morphing article: its frame-0 vertices and every frame's positions.
#[derive(Clone)]
struct MorphPart {
    mesh: MeshId,
    base: Vec<Vertex>,
    frames: Vec<Vec<[f32; 3]>>,
}

fn mm_positions(m: &MeshData) -> Vec<[f32; 3]> {
    m.vertices.iter().map(|v| v.position).collect()
}

impl CarModel {
    /// **0x4D1990(1, state)**: the roof animation to `length · state / 255` frames (0x4D199F…
    /// 0x4D19BA), which 0x4380D0 splits as `frame = fistp(t − 0.5)`, the fraction the rest, and
    /// the draw (0x43E2FD…0x43E35B) blends each vertex `A + (B − A)·frac` between that frame and
    /// the next — the frame alone where there is no next. Past the last frame the part keeps it.
    pub fn set_roof(&mut self, r: &mut Renderer, state: i32) {
        if self.anim_length[1] == 0 || state == self.roof_state {
            return;
        }
        self.roof_state = state;
        let t = self.anim_length[1] as f32 * state as f32 * (1.0 / 255.0);
        self.set_anim(r, 1, t);
    }

    /// The roof state [`set_roof`](Self::set_roof) last took — what the front end's car object
    /// keeps at `+4` and 0x4D19E0(1) answers.
    pub fn roof_state(&self) -> i32 {
        self.roof_state
    }

    /// `0x438190(n)`: animation `n`'s length in frames, 0 when the car has none.
    pub fn anim_length(&self, n: usize) -> u32 {
        self.anim_length.get(n).copied().unwrap_or(0)
    }

    /// **0x4380D0(n, t)**: animation `n` to time `t` frames, clamped to `[0, length]`.
    pub fn set_anim(&mut self, r: &mut Renderer, n: usize, t: f32) {
        let len = self.anim_length(n);
        if len == 0 {
            return;
        }
        let t = t.clamp(0.0, len as f32);
        if t == self.anim_time[n] {
            return;
        }
        self.anim_time[n] = t;
        let frame = (t - 0.5).round_ties_even().max(0.0) as usize;
        let frac = t - frame as f32;
        self.blend(r, n, frame, frame + 1, frac);
    }

    /// Write animation `n`'s `{frame, next, frac}` straight into its record, as slot 9's keyframe
    /// copies do (0x439035…0x43908D for the right arm, 0x4392F1…0x43933C for the body): the draw
    /// (0x43E2FD…0x43E35B) blends each vertex from `frame` toward `next` by `frac`.
    fn set_anim_keys(&mut self, r: &mut Renderer, n: usize, frame: u32, next: u32, frac: f32) {
        if self.anim_length(n) == 0 {
            return;
        }
        // a key write is not a time: forget the last one so a later 0x4380D0 re-uploads
        self.anim_time[n] = -1.0;
        self.blend(r, n, frame as usize, next as usize, frac);
    }

    fn blend(&mut self, r: &mut Renderer, n: usize, frame: usize, next: usize, frac: f32) {
        if self.blended[n] == Some((frame, next, frac)) {
            return;
        }
        self.blended[n] = Some((frame, next, frac));
        for p in &self.morph[n] {
            let Some(a) = p.frames.get(frame.min(p.frames.len() - 1)) else { continue };
            let b = p.frames.get(next);
            let frac = if frame >= p.frames.len() { 0.0 } else { frac };
            let mut v = p.base.clone();
            for (i, out) in v.iter_mut().enumerate() {
                let pa = Vec3::from(a[i]);
                out.position = match b {
                    Some(b) => (pa + (Vec3::from(b[i]) - pa) * frac).to_array(),
                    None => a[i],
                };
            }
            r.update_mesh_vertices(p.mesh, &v);
        }
    }
}

/// A render object's light block, `obj+0x2F4` (`RE/notes/lighting.md` §6): seven colour slots —
/// slot 3 is the direction — and the material colour at `+0x74` that scales the directional half.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightBlock {
    /// slots 0, 1, 2, (3 unused here), 4, 5, 6 — rgb, the alpha the original carries is 1
    pub colours: [[f32; 3]; 7],
    /// slot 3, the way the light travels, in world space
    pub direction: Vec3,
    /// `+0x74`, rgb
    pub material: [f32; 3],
}

impl LightBlock {
    /// The 3D room's block, 0x4E84C0: with `flag` clear (the room's own models) slots 0–2 are the
    /// rig's ambient and 4–6 its sun colour, `/ 255` and faded, and the direction the rig's sun
    /// direction normalised (0x4E876D…0x4E87D7); with `flag` set (the car, 0x4EA08A / 0x4E8CB2)
    /// they are 0.3 and 0.7 grey, faded, and the direction (0, −0.707, 0.707) (0x4E85FE…). `+0x74`
    /// is white either way (0x4E8513). `fade` is `1 − progress / 512` (0x4E84DE).
    pub fn room(ambient: [f32; 3], sun: [f32; 3], sun_dir: Vec3, fade: f32, flag: bool) -> Self {
        let (a, s, dir) = if flag {
            ([0.3 * fade; 3], [0.7 * fade; 3], Vec3::new(0.0, f32::from_bits(0xbf34fdf4), f32::from_bits(0x3f34fdf4)))
        } else {
            (ambient.map(|c| c * fade / 255.0), sun.map(|c| c * fade / 255.0), sun_dir.normalize_or_zero())
        };
        Self { colours: [a, a, a, [0.0; 3], s, s, s], direction: dir, material: [1.0; 3] }
    }

    /// What `R3DLightLayer` 0x446530 makes of a material's `{p0, p1, slot}`: the slot's colour
    /// × p0, and its `slot + 4` partner × `+0x74` × p1 — nothing directional when `p1 <= 0`
    /// (0x446724). A slot past 2 would read the direction as a colour; none ships.
    pub fn layer_light(&self, l: nfs_formats::crp::LayerLight) -> Option<LayerLight> {
        let s = l.slot as usize;
        if s > 2 {
            return None;
        }
        let a = self.colours[s].map(|c| c * l.p0);
        let d = if l.p1 > 0.0 { [0, 1, 2].map(|k| self.colours[s + 4][k] * self.material[k] * l.p1) } else { [0.0; 3] };
        Some(LayerLight { ambient: a, diffuse: d, direction: self.direction.to_array() })
    }
}

impl CarModel {
    /// `draw_items` lit through each material's `R3DLightLayer` from `block`.
    /// `R3DShadowMapTextureLayer` for this car's items: the projector's planes on every item whose
    /// material carries the layer (0x440920 / 0x440910 set it per car at 0x43A70E).
    pub fn apply_projector(&self, items: &mut [DrawItem], planes: Option<([f32; 4], [f32; 4])>) {
        for it in items {
            it.shadow = if self.shadowed.contains(&it.material) { planes } else { None };
        }
    }

    /// `R3DSkyMapTextureLayer` for this car's items (`TrackWorld::sky_map`); the renderer draws
    /// the pass only on the items whose material has it (`MaterialDesc::sky_map`).
    pub fn apply_sky(&self, items: &mut [DrawItem], sky: Option<nfs_renderer::SkyMap>) {
        for it in items {
            it.sky = sky;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_items_block(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2], travel: [f32; 4], passes: &[CarPass], block: &LightBlock) -> Vec<DrawItem> {
        let mut out = self.draw_items(car_to_world, wheel_spin, steer, travel, passes);
        for it in &mut out {
            it.layer_light = self.material_light.get(&it.material).and_then(|l| block.layer_light(*l));
        }
        out
    }
}

#[allow(dead_code)]
fn flat_colour(i: u16) -> [f32; 4] {
    let h = (i as f32 * 0.618_034) % 1.0;
    let (s, v) = (0.45, 0.9);
    let k = (h * 6.0).floor();
    let f = h * 6.0 - k;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match k as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b, 1.0]
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
    use nfs_physics::props::{Breakable, Prop, PropShape, box_footprint};
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    let tags = article_tags(track, &arts);
    for (index, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
            continue;
        }
        let Some(def) = track.animdefs.lookup(b.tag_u32()) else { continue };
        let Some(lod) = g.primitives.iter().map(|p| p.lod).min() else { continue };
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        let mut skipped = 0;
        let local: Vec<Vec3> = article_meshes(&g, lod, &mut skipped)
            .iter()
            .flat_map(|m| m.mesh.vertices.iter().map(|v| Vec3::from(v.position)))
            .collect();
        if local.is_empty() {
            continue;
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
                .map(|(_, p)| first_by_tag(&tags, group_tag(&p.quat_group)).and_then(|k| tags[k].1).is_some_and(|w| w != [0, 0]))
                .collect(),
            started: std::cell::Cell::new(None),
            article: index,
        });
        let shape = match def.collide_type.as_str() {
            "kCylinder" => PropShape::Cylinder { radius: 0.25 },
            "kBox" => {
                let corners = box_footprint(&local).map(|c| tr.transform_point3(c));
                let c = (mn + mx) * 0.5;
                let radius = Vec3::new(mx.x - c.x, 0.0, mx.z - c.z).length() * 1.05;
                PropShape::Box { corners, radius }
            }
            _ => continue,
        };
        let class = if matches!(shape, PropShape::Cylinder { .. }) { def.radius as i32 as u32 } else { 0 };
        out.push(Prop { shape, aabb_min: mn, aabb_max: mx, class, breakable, drive_through: def.drive_through, article: index });
    }
    out
}

/// `WSceneBundle`'s own tree contribution — corrects `RE/RENDERING.md`'s former "no parsed source
/// for *which* library articles a track's scene places" (`ac7b3e6c`): this port
/// *does* have that source, `nfs_formats::scn` + [`TrackGeometry::instances`] (read this session,
/// disassembler addresses below).
///
/// **The read.** `0x46D1D0` (called once, at `0x487abd`, right after `WSceneBundle::Load`'s own
/// call `0x487a87` and before any other bundle's ctor/`Load`/`insertAll(C)` — unchanged from
/// "`WSceneBundle`'s own timing, settled") walks `[0x628168]` — the combined element vector
/// `Scene_parseFile` 0x46C220 fills, `RE/notes/race-logic.md` §3.1: the track's *generic* `.scn`
/// files (never any `GEOM_ELEMENT`s — swept all 78 shipped `.scn` files, zero exceptions) **then**
/// the one loaded start/skill-test file's elements — dynamic-casts each to `CGeomElem` (RTTI
/// `0x5CFA48`, the cast helper `0x5A134B` at `0x46D212`; `0x46D9C0` counts the same way and is
/// what decides `WSceneBundle::Load`'s own live-slot count, `min(that count, 64)`, `0x482c81`…
/// `0x482c97`), and for each survivor calls `0x483260` → **0x482EA0**(this=bundle, fourcc, pos,
/// 3×3): `0x478640`(this, fourcc) searches the bundle's own template-record array (one per
/// `'tr'∧library` article, `0x482970`'s gate) for a record whose stored tag equals the element's
/// own fourcc (`CGeomElem+0x7C`, `0x46AF50`) — **on no match it falls back to template index 0**
/// (`0x482ebe`…`0x482ec0`: `jge` keeps a `>= 0` index, else `xor eax,eax`), never a skip and never
/// a parked `DFLT` slot — then, only once the live cursor is still `< 0x40` (`0x482ec2`, the 64
/// cap; past it the spawn is silently dropped, no slot written, no cursor increment), copies the
/// matched template's `{tag, Article*}` pair and overwrites the record with the **element's own**
/// position and 3×3 (scaled by its per-cent values, `0x46B020`) — never the article's own `tr`
/// (`0x482BF1`: the template's stored record is the identity/zero, matching "The scene bundle's
/// items"). `0x46D9C0()`'s own count is exactly this same dynamic-cast tally, so on every shipped
/// track `N` (the live-slot table's size) equals `M` (the number of elements that pass the cast)
/// exactly whenever there is at least one template (always true — all 70 shared library articles
/// carry `'tr'∧library`) — **zero parked `DFLT` fillers survive spawning on any shipped race**,
/// correcting the former "plus `N−M` `DFLT` filler entries" framing (that framing was the honest
/// worst case before this session's read of `0x482c73`'s own `N`-formula; it is not what the
/// numbers give once `0x46D9C0()` is read).
///
/// Once spawned, the *same* `0x4750B0` walk every other bundle goes through (`RE/RENDERING.md`
/// "Bundle order, read 2026-09-28") decides the tree contribution by the matched **template
/// article's** `animdefs.txt` `collideType` — `kCylinder`/`kBox` build one [`Prop`], anything else
/// (every shipped scene tag surveyed: `FNSH`/`ARW1`-`ARW4` = `collideType=0`, `CONE` =
/// `kSmackable`) contributes nothing, same as a non-`kCylinder`/`kBox` article anywhere else in
/// the tree. A `kBox`/`kCylinder` scene prop's placement is the **element's** transform
/// (`inst.transform * Mat4::from_scale(inst.scale)`, [`crate::world::TrackGeometry::instances`]'s
/// own field, matching "The scene bundle's items"' `0x4828A0` → `0x539CD0` draw matrix), never the
/// library article's own `tr` — [`rotated_local_box_aabb`]/[`box_footprint`] take it in place of
/// `g.transform`.
///
/// **Real data (surveyed, all `*_fstart.scn`/`*_bstart.scn`, every shipped track): 2–5
/// `GEOM_ELEMENT`s per file, tags `FNSH`/`CONE`/`ARW1`-`ARW4` only** (`ENDW`, the one shared
/// article with `collideType=kBox`, appears only in a skill test's `_ST<N>.scn`, which
/// [`TrackAssets::build_geometry`] loads instead of the start file, never alongside it) — **none**
/// of `FNSH`/`CONE`/`ARW1`-`4` is `kCylinder`/`kBox`, so this returns an **empty** `Vec` for every
/// shipped, non-skill-test race: a measured fact
/// (`scene_props_are_empty_on_a_shipped_race`, below), not an assumption that lets the tree wiring
/// skip calling this. The former "the port parses no source" claim is corrected, not merely
/// softened: the source is [`TrackGeometry::instances`] plus [`AnimDefs::lookup`], already wired
/// for drawing; only the *tree*-facing use (this function, and its caller's insertion order) was
/// missing.
///
/// **The fourcc-miss fallback, implemented 2026-09-28** (was a documented, unreachable-on-real-
/// data divergence): a fourcc absent from the bundle's template records takes template index 0
/// (`0x482ebe`…`0x482ec0`, above) — the first `'tr'∧library` article in `.crp` order, i.e.
/// `geometry.library.first()` ([`TrackGeometry::build_geometry`]'s own `library.push` order
/// matches `0x482970`'s gate order). On every shipped track that first article is `DFLT`
/// (`animdefs.txt`: `collideType=kCylinder`), so the fallback is itself collidable — not a
/// no-op. Still unreachable on shipped data (every scene tag on every shipped track's start file
/// or skill test names one of the 70 shared articles), but no longer skipped: `art` below never
/// silently drops an unmatched element the way the pre-2026-09-28 code did.
/// The 64-slot cap is applied (`.take(64)`) though also unreachable (5 is the largest count found).
pub fn scene_props(track: &TrackAssets, geometry: &nfs_assets::track::TrackGeometry) -> Vec<nfs_physics::props::Prop> {
    use nfs_physics::props::{Breakable, Prop, PropShape, box_footprint};
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
        let Some(lod) = g.primitives.iter().map(|p| p.lod).min() else { continue };
        // the element's own placement, never the library article's `tr` ("The scene bundle's
        // items": `0x482BF1` stores the template record as identity/zero).
        let tr = inst.transform * Mat4::from_scale(inst.scale);
        let mut skipped = 0;
        let local: Vec<Vec3> = article_meshes(&g, lod, &mut skipped)
            .iter()
            .flat_map(|m| m.mesh.vertices.iter().map(|v| Vec3::from(v.position)))
            .collect();
        if local.is_empty() {
            continue;
        }
        let (mn, mx) = rotated_local_box_aabb(Vec3::from(b.bbox_min), Vec3::from(b.bbox_max), tr);
        let breakable = (!def.pieces.is_empty()).then(|| Breakable {
            bands: def.bands.clone(),
            band_pieces: def.band_pieces.clone(),
            resolved: def
                .pieces
                .iter()
                .map(|(_, p)| first_by_tag(&tags, group_tag(&p.quat_group)).and_then(|k| tags[k].1).is_some_and(|w| w != [0, 0]))
                .collect(),
            started: std::cell::Cell::new(None),
            article: art.index,
        });
        let shape = match def.collide_type.as_str() {
            "kCylinder" => PropShape::Cylinder { radius: 0.25 },
            "kBox" => {
                let corners = box_footprint(&local).map(|c| tr.transform_point3(c));
                let c = (mn + mx) * 0.5;
                let radius = Vec3::new(mx.x - c.x, 0.0, mx.z - c.z).length() * 1.05;
                PropShape::Box { corners, radius }
            }
            _ => continue,
        };
        let class = if matches!(shape, PropShape::Cylinder { .. }) { def.radius as i32 as u32 } else { 0 };
        out.push(Prop { shape, aabb_min: mn, aabb_max: mx, class, breakable, drive_through: def.drive_through, article: art.index });
    }
    out
}

/// The `insertAll(C)` half of `WTrack+0x24`'s real insertion order (`RE/RENDERING.md` "The tree,
/// built" / "Bundle order, read 2026-09-28"): `0x4750B0` is called once per bundle, in the fixed
/// order `WInstanceBundle → WAnimBundle → WMidGroundBundle → WStaticBundle` (`Crp::bundle_of`'s
/// four variants — a true partition of every *non-library* article; `WSceneBundle` and
/// `WLineBundle` are not walked here, below), each bundle's claimed articles in `.crp` file order,
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
    for rank in [BundleKind::Instance, BundleKind::Anim, BundleKind::MidGround, BundleKind::Static] {
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
                [u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])]
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

    /// 0x444E00's calls: the driver from `0x5CDB28` below class 4, the body from the class's LOD,
    /// the wheels from `0x5CDB44` (or the traffic's `0x5CDB98`), none with the flag.
    #[test]
    fn race_passes_follow_the_tables() {
        let p = |lod, mask| CarPass { lod, mask, flag: false };
        assert_eq!(race_passes(2, 2, false), vec![p(1, 0x80), p(3, 0x50), p(1, 0x20)]);
        assert_eq!(race_passes(3, 0, false), vec![p(2, 0x80), p(4, 0x50), p(2, 0x20)]);
        assert_eq!(race_passes(5, 1, false), vec![p(6, 0x50), p(6, 0x20)]);
        assert_eq!(race_passes(3, 2, true), vec![p(2, 0x80), p(4, 0x50), p(3, 0x20)]);
        // the driver view: the cabin of LOD 2, the steering wheel, the hands at CARDETAIL > 0
        let f = |lod, mask| CarPass { lod, mask, flag: true };
        assert_eq!(in_car_passes(0, false), vec![f(2, 0x10), f(1, 0x40)]);
        assert_eq!(in_car_passes(2, false), vec![f(2, 0x10), f(1, 0x40), f(1, 0x80)]);
        assert_eq!(in_car_passes(2, true), vec![f(2, 0x10), f(1, 0x40)]);
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
