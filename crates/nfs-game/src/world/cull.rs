//! The original's cull walk and its data: the view and its occluders, the per-article LODs, the cell walk, the node pruning,
//! the article's registered tree nodes and the population of the fitted tree. Moved out of `world.rs` as it was.

use super::*;

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
    /// ENHANCEMENT E5: the draw-distance factor (1.0 = the original). `draw` arrives already scaled
    /// by it; the walk scales the rest of the distance gates the same way: the frustum's far
    /// ([`FRUSTUM_FAR`]), a mid-ground article's band and every LOD's distance.
    pub reach: f32,
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
pub(super) fn segments_cross(p0: [f32; 2], p1: [f32; 2], q0: [f32; 2], q1: [f32; 2]) -> bool {
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
        } else if depth - FRUSTUM_FAR * self.reach > r {
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
    pub(super) fn footprint(&self) -> ([f32; 2], f32) {
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
    pub(super) fn far_from(&self, c: [f32; 2], r2: f32) -> bool {
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
pub(super) fn walk_item(v: &CullView, near: &[&Occluder], drawer: ArticleDrawer, centre: Vec3, radius: f32, lods: &[CullLod]) -> Option<ArticleDraw> {
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
        ArticleDrawer::MidGround { band } => {
            let band = band * v.reach;
            (d2 < band * band).then_some(d2)?
        }
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
    let reach = v.reach as f64;
    let lod = lods.iter().position(|l| key < (l.distance as f64 * reach) * (l.distance as f64 * reach))?;
    Some(ArticleDraw { lod, value: rounded, order: (0, 0) })
}

/// One view's walk over the items of `WLineBundle` (the skid marks' cells): see
/// [`TrackWorld::cell_walk`].
pub struct CellWalk<'a> {
    pub(super) v: CullView,
    pub(super) near: Vec<&'a Occluder>,
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
pub(super) const NODE_PRUNE: bool = true;

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
/// zeroes `[+0x18]` at 0x482987 and sets it at 0x482C76 / 0x482C97.)* Settled (closed cd8003d8; the port's `parked_scene_slots` does this): a scene
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

    /// `master`'s `CullView::sphere_visible` and `walk_item`, verbatim but for the name (before ENHANCEMENT E5's `reach`).
    fn master_sphere_visible(v: &CullView, c: Vec3, r: f32) -> bool {
        let rel = c - v.eye;
        let depth = rel.dot(v.forward);
        let a = v.near - depth;
        if a > 0.0 {
            if a > r {
                return false;
            }
        } else if depth - FRUSTUM_FAR > r {
            return false;
        }
        for (x, t) in [(rel.dot(v.right), v.tan_h), (rel.dot(v.up), v.tan_v)] {
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

    fn master_walk_item(v: &CullView, near: &[&Occluder], drawer: ArticleDrawer, centre: Vec3, radius: f32, lods: &[CullLod]) -> Option<ArticleDraw> {
        if !master_sphere_visible(v, centre, radius) {
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

    /// ENHANCEMENT E5: with `reach == 1.0` (the original, `NFS_ENH=off`) the walk is master's bit for bit —
    /// `FRUSTUM_FAR * 1.0`, `band * 1.0` and `distance as f64 * 1.0` are exact in IEEE. The culled list feeds the item order
    /// the renderer draws, so it must not move. Many pseudo-random items (a fixed LCG) plus the exact edges of every gate.
    #[test]
    fn walk_with_reach_1_equals_master_walk() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move |lo: f32, hi: f32| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            lo + (hi - lo) * ((seed >> 40) as f32 / (1u64 << 24) as f32)
        };
        let lods = [
            CullLod { distance: 40, effects: true, first: 0, last: 1, lod: 0 },
            CullLod { distance: 150, effects: true, first: 1, last: 2, lod: 1 },
            CullLod { distance: 420, effects: false, first: 2, last: 3, lod: 2 },
            CullLod { distance: 0xFFFF, effects: true, first: 3, last: 4, lod: 3 },
        ];
        let wall = Occluder { raised: false, a: [-30.0, 60.0], d: [60.0, 0.0], len2: 3600.0, inv_len2: 1.0 / 3600.0 };
        let raised = Occluder { raised: true, a: [80.0, -40.0], d: [0.0, 300.0], len2: 90000.0, inv_len2: 1.0 / 90000.0 };
        let occ: [&[&Occluder]; 3] = [&[], &[&wall], &[&wall, &raised]];
        let drawers = [ArticleDrawer::Static, ArticleDrawer::MidGround { band: 600.0 }, ArticleDrawer::MidGround { band: 700.0 }, ArticleDrawer::Instance, ArticleDrawer::Anim];
        let mut seen = [0usize; 2];
        for _ in 0..400 {
            let yaw = rnd(-3.2, 3.2);
            let forward = Vec3::new(yaw.sin(), rnd(-0.3, 0.1), yaw.cos()).normalize();
            let right = forward.cross(Vec3::Y).normalize();
            let up = right.cross(forward);
            let v = CullView { eye: Vec3::new(rnd(-500.0, 500.0), rnd(0.0, 50.0), rnd(-500.0, 500.0)), forward, right, up, tan_h: rnd(0.4, 1.2), tan_v: rnd(0.3, 0.9), near: rnd(0.1, 1.0), draw: rnd(100.0, 900.0), reach: 1.0 };
            for _ in 0..40 {
                let r = rnd(0.5, 250.0);
                let dir = (forward * rnd(-0.2, 1.0) + right * rnd(-1.0, 1.0) + up * rnd(-0.5, 0.5)).normalize_or_zero();
                // a random distance, or one on the far / band / draw edges (± the radius)
                let d = match rnd(0.0, 4.0) as u32 {
                    0 => FRUSTUM_FAR + r,
                    1 => 600.0,
                    2 => v.draw + r,
                    _ => rnd(0.0, 1200.0),
                };
                let c = v.eye + dir * d;
                for drawer in drawers {
                    for near in occ {
                        let (a, b) = (walk_item(&v, near, drawer, c, r, &lods), master_walk_item(&v, near, drawer, c, r, &lods));
                        assert_eq!(a.map(|x| (x.lod, x.value.to_bits())), b.map(|x| (x.lod, x.value.to_bits())), "{v:?} {drawer:?} c {c} r {r}");
                        seen[a.is_some() as usize] += 1;
                    }
                }
            }
            // the sphere test alone, on its far edge
            let c = v.eye + forward * (FRUSTUM_FAR + 3.0);
            assert_eq!(v.sphere_visible(c, 3.0), master_sphere_visible(&v, c, 3.0));
        }
        // both outcomes were exercised
        assert!(seen[0] > 1000 && seen[1] > 1000, "{seen:?}");
    }

    /// Locked against `cull_tree_node_prune_census.rs`'s own `sanity_check` (same formula,
    /// independently re-derived here to catch a sign regression in either copy).
    #[test]
    fn node_prune_sides_and_far() {
        // `right`/`up` follow `main.rs`'s own construction: `right = forward.cross(up)`, so
        // `forward = +Z, up = +Y` gives `right = -X` (not `+X` — the wedge's side normals below
        // must use the *same* handedness `CullView.right` already carries everywhere else).
        let base = CullView { eye: Vec3::ZERO, forward: Vec3::Z, right: Vec3::NEG_X, up: Vec3::Y, tan_h: 1.0, tan_v: 1.0, near: 1.0, draw: 300.0, reach: 1.0 };
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
pub(super) fn article_base(track: &TrackAssets, a: &nfs_formats::crp::Article) -> Option<(Vec3, f32, Vec<CullLod>)> {
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

#[cfg(test)]
mod cull_tests {
    use super::*;

    fn view() -> CullView {
        CullView { eye: Vec3::ZERO, forward: Vec3::Z, right: Vec3::X, up: Vec3::Y, tan_h: 1.0, tan_v: 0.75, near: 0.5, draw: 400.0, reach: 1.0 }
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
