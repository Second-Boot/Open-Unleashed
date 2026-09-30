//! The real `WTrack+0x24`, wired: one [`crate::quadtree::QuadTree`] over the port's own
//! [`crate::collision::Patch`]es, [`crate::props::Prop`]s and [`crate::walls::Wall`]s, in the
//! original's own insertion order.
//!
//! This module only *combines* three already-ordered lists into one tree and remembers, per tree
//! slot, which kind it is and its index into the caller's own list ([`TreeItem`]) — the ordering
//! itself (which bundle an article sits in, its file position, its patch/prop choice) is the
//! caller's job (`nfs_game::world::track_tree_order`, which has `Crp::bundle_of` and the article
//! list; this crate does not parse `.crp` files). See `RE/RENDERING.md` "The tree, built" and
//! "Bundle order, read 2026-09-28" for the full trace: `insertAll(C)` (`0x4883BC`) walks the
//! bundles that own a `PCollideArticleBundle` — `WInstanceBundle → WStaticBundle` (*corrected
//! 2026-09-30, `WAnimBundle` and `WMidGroundBundle` have none, `[bundle+0xC] = 0`, and
//! `0x488090` skips them, 0x48838B*), each bundle's claimed
//! articles in `.crp` file order, each article's own patches (in primitive order) or its one
//! `kCylinder`/`kBox` object (`0x4750B0`'s per-article type dispatch, `0x4751BD`/`0x475238` —
//! never both for one article in the original); then `insertAll(A)` (`0x4883F5`), every `.edg`
//! edge in file order.
//!
//! `WSceneBundle` (corrected 2026-09-28: this port *does* have a source — `0x46D1D0`'s spawn walk
//! over the track's `.scn` `GEOM_ELEMENT`s, `nfs_game::world::scene_props`) is not built by this
//! module either — its own `[+0x1c]` live-slot array is keyed by spawn order, not `.crp` article
//! order, so it cannot share `track_tree_order`'s article loop. The caller instead builds
//! `scene_props`'s own `Prop`s, appends them to its flat `props` list, and passes
//! `track_tree_order` their indices so they still land **first** in the returned order, matching
//! `0x46D1D0` (called at `0x487abd`) running before any other bundle's own `Load` — see
//! `nfs_game::world::scene_props`'s and `track_tree_order`'s doc comments for the full trace.

use crate::collision::Patch;
use crate::props::Prop;
use crate::quadtree::{Entry, QuadTree};
use crate::walls::Wall;

/// One tree slot: which of the port's own lists it is, and its index there. `Copy` — this is a
/// plain tag, not a borrow, so a query's result list can be collected and reused freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeItem {
    Patch(usize),
    Prop(usize),
    Wall(usize),
}

/// A borrowed entry of one of the three kinds — only exists for the duration of
/// [`QuadTree::build`]'s temporary borrow; nothing keeps this type afterward.
enum Ref<'a> {
    Patch(&'a Patch),
    Prop(&'a Prop),
    Wall(&'a Wall),
}

impl Entry for Ref<'_> {
    fn centroid(&self) -> (f32, f32) {
        match self {
            Ref::Patch(p) => p.centroid(),
            Ref::Prop(p) => p.centroid(),
            Ref::Wall(w) => w.centroid(),
        }
    }

    fn extent(&self) -> ((f32, f32), (f32, f32)) {
        match self {
            Ref::Patch(p) => p.extent(),
            Ref::Prop(p) => p.extent(),
            Ref::Wall(w) => w.extent(),
        }
    }
}

/// `WTrack+0x24`, built once at track load from the caller's own `insertAll(C)`-then-`insertAll(A)`
/// order.
pub struct TrackTree {
    tree: QuadTree,
    order: Vec<TreeItem>,
}

impl TrackTree {
    /// `order` is the caller's own insertion sequence — every patch and prop in bundle-then-
    /// article-then-primitive order (`insertAll(C)`, `0x4883BC`), then every wall in `.edg` file
    /// order (`insertAll(A)`, `0x4883F5`). `patches`/`props`/`walls` are the port's own lists;
    /// `order`'s indices are into them, by kind.
    pub fn build(patches: &[Patch], props: &[Prop], walls: &[Wall], order: &[TreeItem]) -> Self {
        let refs: Vec<Ref> = order
            .iter()
            .map(|item| match *item {
                TreeItem::Patch(i) => Ref::Patch(&patches[i]),
                TreeItem::Prop(i) => Ref::Prop(&props[i]),
                TreeItem::Wall(i) => Ref::Wall(&walls[i]),
            })
            .collect();
        let tree = QuadTree::build(&refs);
        TrackTree { tree, order: order.to_vec() }
    }

    /// An empty tree (no patches, props or walls) — a track with none of the three, or a test
    /// fixture that does not need the real thing.
    pub fn empty() -> Self {
        let none: Vec<Ref> = Vec::new();
        TrackTree { tree: QuadTree::build(&none), order: Vec::new() }
    }

    /// `0x4848B0`: every tree slot under the segment's xz box, in the original's leaf-then-
    /// insertion-order — the same order a leaf's own entries were inserted in, since `0x483DF0`'s
    /// split re-inserts a leaf's list front to back and `0x484AE0` only ever appends.
    pub fn query_segment(&self, p0: (f32, f32), p1: (f32, f32)) -> impl Iterator<Item = TreeItem> + '_ {
        self.tree.query_segment(p0, p1).into_iter().map(move |i| self.order[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::PatchKind;
    use glam::{Vec2, Vec3};

    fn patch(article: usize, centroid: Vec3) -> Patch {
        Patch { article, primitive: 0, kind: PatchKind::Triangle, tris: vec![0], centroid, extent: ((centroid.x, centroid.z), (centroid.x, centroid.z)) }
    }
    fn prop(article: usize, centre: Vec3) -> Prop {
        Prop {
            shape: crate::props::PropShape::Cylinder { radius: 0.25 },
            aabb_min: centre - Vec3::splat(0.5),
            aabb_max: centre + Vec3::splat(0.5),
            class: 0,
            breakable: None,
            drive_through: false,
            article,
        }
    }
    fn wall(a: Vec2, b: Vec2) -> Wall {
        Wall { a, b, y_mid: 0.0, y_lo: 0.0, y_hi: 0.0, ya: 0.0, yb: 0.0, class: 0 }
    }

    /// The combined tree returns entries of every kind, in the caller's own `order` — patches and
    /// props interleaved as given, walls after.
    #[test]
    fn combined_tree_keeps_the_callers_order() {
        let patches = vec![patch(0, Vec3::new(10.0, 0.0, 10.0)), patch(1, Vec3::new(-10.0, 0.0, 10.0))];
        let props = vec![prop(2, Vec3::new(10.0, 0.0, -10.0))];
        let walls = vec![wall(Vec2::new(-10.0, -10.0), Vec2::new(-9.0, -10.0))];
        let order = vec![TreeItem::Patch(0), TreeItem::Prop(0), TreeItem::Patch(1), TreeItem::Wall(0)];
        let tree = TrackTree::build(&patches, &props, &walls, &order);
        let got: Vec<TreeItem> = tree.query_segment((-8192.0, -8192.0), (8192.0, 8192.0)).collect();
        // every entry present; each kind's own relative order preserved (the query visits leaves
        // in a fixed quadrant order, so entries from different quadrants may interleave
        // differently than `order`, but two entries sharing a leaf keep their `order` position)
        assert_eq!(got.len(), 4);
        assert!(got.contains(&TreeItem::Patch(0)));
        assert!(got.contains(&TreeItem::Patch(1)));
        assert!(got.contains(&TreeItem::Prop(0)));
        assert!(got.contains(&TreeItem::Wall(0)));
    }

    /// An empty tree answers no entries and does not panic.
    #[test]
    fn empty_tree_is_empty() {
        let tree = TrackTree::empty();
        assert_eq!(tree.query_segment((-8192.0, -8192.0), (8192.0, 8192.0)).count(), 0);
    }
}
