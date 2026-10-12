//! The world quadtree — `WTrack+0x24` (`RE/RENDERING.md` "What the tree holds, and the order a
//! query hands it back") — the collision index `WTrack::load` builds once at level load and this
//! port never built. Read to the instruction 2026-09-27/28 (addresses below verified against
//! `nfs5.exe` with `dasm.py`):
//!
//! - **Root**: `0x483800 QuadTree::init` is called with half-extent `0x46000000` = 8192.0 f32 and
//!   15 levels, so the square is ±8192 m, cell 0 = 16384 m. **CONFIRMED** at 0x484B23…0x484B7F /
//!   0x4848DB…0x484935 / 0x483E0F…0x483E68: a node's box is rebuilt from its key every time it is
//!   needed, `cell = table[level]`, `x0 = ix·cell − 8192`, `z0 = iz·cell − 8192`.
//! - **Level cap 12** (4 m leaf, even though the cell table holds 15): `0x483DFB`
//!   `and eax, 0xF0000000; cmp eax, 0xC0000000; je 0x484314` — at level 12 `split()` just returns
//!   (`pop`s and `ret`s) without touching the node. **CONFIRMED** by disassembly.
//! - **Split threshold 0x30**: `0x4850A6` `cmp ecx, 0x30; jbe 0x4850B2` where `ecx` is the leaf's
//!   entry count (`(end−begin)/8`, an 8-byte-per-entry vector) — the split (`call 0x483DF0`) fires
//!   only once the count exceeds 0x30, i.e. on the 49th entry. **CONFIRMED**; the vector's own
//!   capacity growth (`cmp ecx, 0x32` at 0x484FEC, unrelated: it decides whether the *backing
//!   array* needs to grow to fit 0x190 bytes = 50 slots) is a different check that this
//!   first pass mistook for the split test — corrected before it reached a note.
//! - **Split preserves order**: `0x483DF0` re-inserts the leaf's own list, front to back, into the
//!   new children (`0x483E8A…`, the loop over `[edi]..[edi+4]`), then frees the list and nulls
//!   `+0x14`. **CONFIRMED** (matches the existing note; not re-derived byte for byte here).
//! - **Insert's accept test** `0x484320`: an element's centroid (vtable slot 1, `call [eax+4]`)
//!   against the node's own box; if that fails, the element's extent (vtable slot 2,
//!   `call [edx+8]`) is tested for overlap instead — so a straddling element reaches every
//!   quadrant its box touches. **Read to the instruction 2026-09-28** (`RE/RENDERING.md` "The
//!   accept test, read 2026-09-28" has the full trace):
//!   - the centroid test is **closed on both ends of both axes** —
//!     `x0 <= centroid.x <= x1 && z0 <= centroid.z <= z1` — not the query's asymmetric
//!     strict-low rule; `y` is never read (`0x48439a`…`0x4843d4`, four `fcomp`/`test ah,{1,0x41}`
//!     pairs against the node's own `x0`/`x1`/`z0`/`z1`, rebuilt from the key exactly as the
//!     module doc above already gives). This is [`Bounds::contains_point`], now fixed to match
//!     (it used to be half-open on the high side, `< x1`/`< z1`, an unread guess).
//!   - the extent test (only run when the centroid test fails) is a standard 2-D AABB overlap,
//!     also xz-only: `elem.min.x < x1 && elem.min.z < z1 && elem.max.x >= x0 && elem.max.z >= z0`
//!     (`0x4843d6`…`0x484447`) — this is [`Bounds::overlaps_extent`], which already matched.
//! - **Quadrant order and the child-key rule**: `0x483BD0 QuadNode::init(parentKey, quadrant)` —
//!   `level+1`, `x'=2x`, `z'=2z`, then `+1` on `x` for quadrants 1 and 3 and on `z` for 0 and 1.
//!   `0x484AE0 insert` creates children in the order **2** (`+0x0C`, `push 2` at 0x484C59 — no
//!   `+1` on either axis: low x, low z), **0** (`+0x04`, `push 0`(`xor ebx,ebx`) at 0x484D49: `+1`
//!   on z only: low x, high z), **1** (`+0x08`, `push 1` at 0x484E42: `+1` on both: high x, high
//!   z), **3** (`+0x10`, `push 3` at 0x484F65: `+1` on x only: high x, low z).
//! - **The segment query** `0x4848B0`: for a childless node, appends it to the visitor list
//!   (`0x484A95`) and returns — a leaf. For a node with children, computes the node's **centre**
//!   (`(index+0.5)·cell − 8192`, the `fadd [0x5B23F0]` = 0.5 at 0x484909/0x484923, distinct from
//!   the plain corner math `insert`/`accept` use) and the query segment's xz min/max, then
//!   descends, in this exact order: **`+0x0C`** (low x, low z, 0x4849E3), **`+0x04`** (low x, high
//!   z, 0x484A0F), **`+0x10`** (high x, low z, 0x484A5D), and the tail loop into **`+0x08`** (high
//!   x, high z, 0x484A89 → `jmp 0x4848C3`) — matching the child-creation order above one-for-one.
//!   Every comparison is `fcomp`/`fnstsw`/`test ah,1`, a strict `<`: a coordinate exactly at the
//!   centre fails the low-half test and only the high half is descended. **CONFIRMED** (this
//!   session re-read the full function; the existing note already had this right).
//!
//! **What still blocks building the real tree from this port's data (2026-09-28, prerequisite 1
//! closed):** the original fills the tree once, at load, `insertAll(C)` (0x4883BC)
//! then `insertAll(A)` (0x4883F5) — the `.crp` bundles' patches and `kCylinder`/`kBox` objects in
//! `0x4750B0`'s per-bundle walk over `[WTrack+0x14]`, then every `.edg` edge in file order. Two
//! things this port's asset pipeline did not keep:
//!
//! 1. ~~**Primitive identity.**~~ **Kept, 2026-09-28.** `0x4750B0` builds one **patch** entry per
//!    quad or triangle *shape* — a quad (ctor `0x4853C0`, 4 vertex pointers) or a triangle
//!    (`0x485C50`, 3) counts as **one** leaf entry each, even when several quads share one
//!    `prim_type == 4` `pr` record ("sequential quads", `crp-geometry.md` §4). This port's
//!    `nfs_assets::mesh::article_parts` (`crates/nfs-assets/src/mesh.rs`) still expands every quad
//!    into **two** triangles for rendering, but now tags each `MaterialMesh` it returns with
//!    `patch_kind`/`prim_index`; `TrackGeometry::collision_grid`
//!    (`crates/nfs-assets/src/track.rs`) carries that onto every [`crate::collision::Triangle`] as
//!    a [`crate::collision::PatchSource`], and [`crate::collision::TriangleGrid::patches`]
//!    reconstructs one [`crate::collision::Patch`] per original entry — a quad's pair recounted as
//!    one, with the address-confirmed centre formula. See that method's doc for the formula, the
//!    still-unread patch-*extent* gap, and its unit test; `RE/RENDERING.md` "The tree, built" has
//!    the fuller account.
//! 2. **Bundle-native order** (open, quadtree prerequisite 2). `0x4750B0` is called once per
//!    bundle, walking `[WTrack+0x14]`'s bundle list (confirmed: the two call sites are
//!    0x488201, inside the world's own per-bundle loop, and 0x48839F, immediately before
//!    `insertAll(C)` 0x4883BC itself); a track can carry several `W*Bundle` instances
//!    (`RE/RENDERING.md` "The `W*Bundle` base"). This port's `nfs_game::world::track_props`
//!    (`crates/nfs-game/src/world.rs`) instead walks `track.crp.articles()` — the `.crp`'s **one
//!    flat article list** — with no per-bundle membership or sub-order kept at all, so there is no
//!    way to recover which bundle an article came from or where in that bundle's own walk it sat.
//!
//! Gap 2 is upstream of this module, in `nfs-game`'s parsing, not something a consumer of the tree
//! can work around. Rebuilding the tree with the port's *current* prop list while it's still open
//! would not reproduce `WTrack+0x24`'s leaf boundaries or its query order — it would just be a
//! second, differently-wrong approximation, so this module is not wired to real track data yet.
//! It stands as the verified structure and the split/query rules, tested against the facts
//! above; `crates/nfs-physics/src/walls.rs:177`/`:220` and `crates/open-unleashed/src/main.rs`'s prop-push
//! comment keep their approximation markers and now point
//! here for what would need to change upstream first.

/// vtable slots 1 and 2 (`insert`'s accept test, `0x484320`): the element's centroid in the xz
/// plane, and — only asked when the centroid test fails a child's box — its own xz extent
/// (`(min, max)`), so an element straddling a split reaches every quadrant its box touches.
pub trait Entry {
    fn centroid(&self) -> (f32, f32);
    fn extent(&self) -> ((f32, f32), (f32, f32));
}

/// The root half-extent, `0x46000000` = 8192.0 (`0x483800`'s caller, `WTrack::load` 0x487876).
pub const HALF_EXTENT: f32 = 8192.0;
/// The depth cap (`0x483DFB`): the finest cell is `2·8192 / 2^12` = 4.0 m.
pub const LEVEL_CAP: u32 = 12;
/// A leaf splits once its entry count exceeds this (`0x4850A6`) — i.e. on the 49th entry.
pub const SPLIT_THRESHOLD: usize = 0x30;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Bounds {
    x0: f32,
    z0: f32,
    x1: f32,
    z1: f32,
}

impl Bounds {
    fn root() -> Self {
        Bounds { x0: -HALF_EXTENT, z0: -HALF_EXTENT, x1: HALF_EXTENT, z1: HALF_EXTENT }
    }

    fn centre(&self) -> (f32, f32) {
        (0.5 * (self.x0 + self.x1), 0.5 * (self.z0 + self.z1))
    }

    /// The four children in the creation/query order `0x484AE0`/`0x4848B0` use: low-x/low-z,
    /// low-x/high-z, high-x/high-z, high-x/low-z (offsets `+0x0C, +0x04, +0x08, +0x10`).
    fn child(&self, quadrant: Quadrant) -> Bounds {
        let (cx, cz) = self.centre();
        match quadrant {
            Quadrant::LowXLowZ => Bounds { x0: self.x0, z0: self.z0, x1: cx, z1: cz },
            Quadrant::LowXHighZ => Bounds { x0: self.x0, z0: cz, x1: cx, z1: self.z1 },
            Quadrant::HighXHighZ => Bounds { x0: cx, z0: cz, x1: self.x1, z1: self.z1 },
            Quadrant::HighXLowZ => Bounds { x0: cx, z0: self.z0, x1: self.x1, z1: cz },
        }
    }

    /// `0x484320`'s centroid test (`0x48439a`…`0x4843d4`): closed on both ends of both axes —
    /// `x0 <= p.x <= x1 && z0 <= p.z <= z1`. CONFIRMED by disassembly 2026-09-28: each bound's
    /// `fcomp`/`fnstsw`/`test ah,{1,0x41}` rejects strictly outside (`test ah,1`, C0 only) on the
    /// low side and strictly outside (`test ah,0x41`, C0-or-C3) on the high side, so equality on
    /// *either* end survives — asymmetric with [`Node::query`]'s strict-low descent rule below,
    /// which is a different function (`0x4848B0`) with its own comparisons. This used to be
    /// `< x1`/`< z1` (half-open, an unread guess); a point exactly on a shared boundary is now
    /// accepted by every neighbouring quadrant that shares it, matching the original — see
    /// `accept_test_is_closed_on_both_bounds` below.
    fn contains_point(&self, p: (f32, f32)) -> bool {
        p.0 >= self.x0 && p.0 <= self.x1 && p.1 >= self.z0 && p.1 <= self.z1
    }

    /// `0x484320`'s extent test (`0x4843d6`…`0x484447`, run only when [`Self::contains_point`]
    /// fails): a standard 2-D AABB overlap — `elem.min < node.max` (strict, `test ah,1`) and
    /// `elem.max >= node.min` (non-strict, `test ah,1`/`je`), both axes. CONFIRMED by
    /// disassembly 2026-09-28 — already matched this implementation exactly, unread until now.
    fn overlaps_extent(&self, ext: ((f32, f32), (f32, f32))) -> bool {
        let ((minx, minz), (maxx, maxz)) = ext;
        maxx >= self.x0 && minx < self.x1 && maxz >= self.z0 && minz < self.z1
    }
}

/// The order `0x484AE0`'s children are created in and `0x4848B0`'s query descends them — the same
/// order for both (verified: `push 2/0/1/3` at the four `0x483BD0` call sites line up
/// one-for-one with the query's `+0x0C/+0x04/+0x08.."tail"/+0x10` recursion, modulo the query's
/// `+0x08` step being a tail loop rather than a fourth explicit recursive call).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quadrant {
    LowXLowZ,
    LowXHighZ,
    HighXHighZ,
    HighXLowZ,
}

impl Quadrant {
    /// A fixed storage slot for `Node::Split`'s `children` array — arbitrary in itself (the
    /// binary's four child pointers sit at fixed struct offsets, this is this port's stand-in for
    /// that), but shared by every array below so insertion and query agree on which physical child
    /// a quadrant is.
    fn slot(self) -> usize {
        match self {
            Quadrant::LowXLowZ => 0,
            Quadrant::LowXHighZ => 1,
            Quadrant::HighXHighZ => 2,
            Quadrant::HighXLowZ => 3,
        }
    }
}

/// Any order works for creation (every quadrant is tested regardless), kept here in the same
/// order the binary creates children for reference.
const CHILD_ORDER: [Quadrant; 4] = [Quadrant::LowXLowZ, Quadrant::LowXHighZ, Quadrant::HighXHighZ, Quadrant::HighXLowZ];
/// `0x4848B0`'s descent order: low-x/low-z, low-x/high-z, high-x/low-z, high-x/high-z (the last a
/// tail loop rather than a fourth explicit recursive call) — this order is load-bearing for the
/// port's three quadtree-order consumers, unlike `CHILD_ORDER` above.
const QUERY_ORDER: [Quadrant; 4] = [Quadrant::LowXLowZ, Quadrant::LowXHighZ, Quadrant::HighXLowZ, Quadrant::HighXHighZ];

enum Node {
    Leaf { entries: Vec<usize> },
    Split { children: Box<[Node; 4]> },
}

impl Node {
    fn leaf() -> Self {
        Node::Leaf { entries: Vec::new() }
    }

    /// `0x484AE0`: append to a childless node (splitting past [`SPLIT_THRESHOLD`] entries, capped
    /// at [`LEVEL_CAP`]); otherwise pass to every child `0x484320` accepts.
    fn insert<E: Entry>(&mut self, bounds: Bounds, level: u32, entries: &[E], idx: usize) {
        match self {
            Node::Leaf { entries: list } => {
                list.push(idx);
                if list.len() > SPLIT_THRESHOLD && level < LEVEL_CAP {
                    self.split(bounds, level, entries);
                }
            }
            Node::Split { children } => {
                let centroid = entries[idx].centroid();
                let extent = entries[idx].extent();
                for quadrant in CHILD_ORDER {
                    let cb = bounds.child(quadrant);
                    if cb.contains_point(centroid) || cb.overlaps_extent(extent) {
                        children[quadrant.slot()].insert(cb, level + 1, entries, idx);
                    }
                }
            }
        }
    }

    /// `0x483DF0`: turn a leaf into four children and re-insert its own list, in order, into them.
    /// A no-op at [`LEVEL_CAP`] (`0x483DFB`'s check) — the caller only calls this when `level <
    /// LEVEL_CAP`, matching the binary's own guard.
    fn split<E: Entry>(&mut self, bounds: Bounds, level: u32, entries: &[E]) {
        let Node::Leaf { entries: list } = self else { return };
        let list = std::mem::take(list);
        let mut children: [Node; 4] = [Node::leaf(), Node::leaf(), Node::leaf(), Node::leaf()];
        for &idx in &list {
            let centroid = entries[idx].centroid();
            let extent = entries[idx].extent();
            for quadrant in CHILD_ORDER {
                let cb = bounds.child(quadrant);
                if cb.contains_point(centroid) || cb.overlaps_extent(extent) {
                    children[quadrant.slot()].insert(cb, level + 1, entries, idx);
                }
            }
        }
        *self = Node::Split { children: Box::new(children) };
    }

    /// `0x4848B0`: a childless node is a leaf — append it and return. Otherwise, in query order,
    /// descend a child only when the segment's box could reach that half (a coordinate exactly at
    /// the node's centre goes to the high half only — the low test is a strict `<`).
    ///
    /// The recursive form, kept as the reference [`SegmentIter`] is tested against
    /// (`quadtree/order_guard.rs`).
    #[cfg(test)]
    fn query_reference(&self, bounds: Bounds, seg_min: (f32, f32), seg_max: (f32, f32), out: &mut Vec<Vec<usize>>) {
        match self {
            Node::Leaf { entries } => out.push(entries.clone()),
            Node::Split { children } => {
                let (cx, cz) = bounds.centre();
                for quadrant in QUERY_ORDER {
                    if quadrant_reached(quadrant, cx, cz, seg_min, seg_max) {
                        children[quadrant.slot()].query_reference(bounds.child(quadrant), seg_min, seg_max, out);
                    }
                }
            }
        }
    }
}

/// Does the segment's box reach `quadrant` of a node centred at `(cx, cz)`? The low half is a strict
/// `<` on the segment's minimum, the high half `>=` on its maximum (`0x4848B0`'s `fcomp` /
/// `test ah,1` pairs).
#[inline]
fn quadrant_reached(quadrant: Quadrant, cx: f32, cz: f32, seg_min: (f32, f32), seg_max: (f32, f32)) -> bool {
    let low_x = matches!(quadrant, Quadrant::LowXLowZ | Quadrant::LowXHighZ);
    let low_z = matches!(quadrant, Quadrant::LowXLowZ | Quadrant::HighXLowZ);
    let x_reaches = if low_x { seg_min.0 < cx } else { seg_max.0 >= cx };
    let z_reaches = if low_z { seg_min.1 < cz } else { seg_max.1 >= cz };
    x_reaches && z_reaches
}

/// One split node on [`SegmentIter`]'s path: its children, its box and the next [`QUERY_ORDER`]
/// position to try.
#[derive(Clone, Copy)]
struct Frame<'a> {
    children: &'a [Node; 4],
    bounds: Bounds,
    next: usize,
}

/// `0x4848B0` as a lazy walk: the entries of every leaf under the segment's box, leaf by leaf in
/// [`QUERY_ORDER`], each leaf's entries in insertion order — the same sequence the recursive
/// `Node::query_reference` collects, with no allocation (a split node exists only below
/// [`LEVEL_CAP`], so the path is at most that many frames).
pub struct SegmentIter<'a> {
    stack: [Option<Frame<'a>>; LEVEL_CAP as usize + 1],
    depth: usize,
    leaf: std::slice::Iter<'a, usize>,
    seg_min: (f32, f32),
    seg_max: (f32, f32),
}

impl<'a> SegmentIter<'a> {
    fn enter(&mut self, node: &'a Node, bounds: Bounds) {
        match node {
            Node::Leaf { entries } => self.leaf = entries.iter(),
            Node::Split { children } => {
                self.stack[self.depth] = Some(Frame { children, bounds, next: 0 });
                self.depth += 1;
            }
        }
    }
}

impl Iterator for SegmentIter<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        loop {
            if let Some(&i) = self.leaf.next() {
                return Some(i);
            }
            if self.depth == 0 {
                return None;
            }
            let frame = self.stack[self.depth - 1].as_mut().expect("a frame below depth");
            if frame.next == QUERY_ORDER.len() {
                self.depth -= 1;
                continue;
            }
            let quadrant = QUERY_ORDER[frame.next];
            frame.next += 1;
            let (children, bounds) = (frame.children, frame.bounds);
            let (cx, cz) = bounds.centre();
            if quadrant_reached(quadrant, cx, cz, self.seg_min, self.seg_max) {
                self.enter(&children[quadrant.slot()], bounds.child(quadrant));
            }
        }
    }
}

/// `WTrack+0x24`: the collision quadtree, `0x483800`'s ±8192 m / 12-level tree. Built from a list
/// of entries in the caller's own insertion order (the original's is `insertAll(C)` then
/// `insertAll(A)`, `0x488090`'s tail) — see the module doc for why this port has nothing faithful
/// to build it from yet.
pub struct QuadTree {
    root: Node,
}

impl QuadTree {
    /// `0x4883BC`/`0x4883F5`: insert every entry, in order — the caller decides what "in order"
    /// means (the original: bundle-walk patches and objects, then `.edg` edges).
    pub fn build<E: Entry>(entries: &[E]) -> Self {
        let mut root = Node::leaf();
        for idx in 0..entries.len() {
            root.insert(Bounds::root(), 0, entries, idx);
        }
        QuadTree { root }
    }

    /// `0x4848B0`: every childless node under the segment's xz box, in the quadrant order above;
    /// within a leaf, entries keep the tree's own insertion order (`0x483DF0` re-inserts in order,
    /// `0x484AE0` only ever appends).
    pub fn query_segment(&self, p0: (f32, f32), p1: (f32, f32)) -> Vec<usize> {
        self.segment_iter(p0, p1).collect()
    }

    /// [`Self::query_segment`] without the allocations: the same sequence, walked lazily.
    pub fn segment_iter(&self, p0: (f32, f32), p1: (f32, f32)) -> SegmentIter<'_> {
        let seg_min = (p0.0.min(p1.0), p0.1.min(p1.1));
        let seg_max = (p0.0.max(p1.0), p0.1.max(p1.1));
        let mut it = SegmentIter { stack: [None; LEVEL_CAP as usize + 1], depth: 0, leaf: [].iter(), seg_min, seg_max };
        it.enter(&self.root, Bounds::root());
        it
    }

    /// The recursive query, collected leaf by leaf (the reference for [`Self::segment_iter`]).
    #[cfg(test)]
    fn query_segment_reference(&self, p0: (f32, f32), p1: (f32, f32)) -> Vec<usize> {
        let seg_min = (p0.0.min(p1.0), p0.1.min(p1.1));
        let seg_max = (p0.0.max(p1.0), p0.1.max(p1.1));
        let mut leaves = Vec::new();
        self.root.query_reference(Bounds::root(), seg_min, seg_max, &mut leaves);
        leaves.into_iter().flatten().collect()
    }
}

#[cfg(test)]
mod order_guard;

#[cfg(test)]
mod tests {
    use super::*;

    struct Point(f32, f32);
    impl Entry for Point {
        fn centroid(&self) -> (f32, f32) {
            (self.0, self.1)
        }
        fn extent(&self) -> ((f32, f32), (f32, f32)) {
            ((self.0, self.1), (self.0, self.1))
        }
    }

    /// `0x4850A6`: 0x30 (48) entries stay a leaf; the 49th (the 0x31st) pushes the count over the
    /// threshold and triggers `0x483DF0`.
    #[test]
    fn splits_past_forty_eight_entries_not_before() {
        // 48 points, all in the same quadrant so they'd land in one leaf either way — this only
        // tests the *count* at which the node stops being one leaf.
        let pts: Vec<Point> = (0..48).map(|i| Point(1.0 + i as f32 * 0.001, 1.0)).collect();
        let tree = QuadTree::build(&pts);
        assert!(matches!(tree.root, Node::Leaf { .. }), "48 entries must not split");
        assert_eq!(tree.query_segment((0.0, 0.0), (2.0, 2.0)).len(), 48);

        let pts: Vec<Point> = (0..49).map(|i| Point(1.0 + i as f32 * 0.001, 1.0)).collect();
        let tree = QuadTree::build(&pts);
        assert!(matches!(tree.root, Node::Split { .. }), "49 entries must split");
        assert_eq!(tree.query_segment((0.0, 0.0), (2.0, 2.0)).len(), 49);
    }

    /// `0x484AE0`/`0x4848B0`: children created and visited low-x/low-z, low-x/high-z,
    /// high-x/low-z, high-x/high-z (offsets `+0x0C, +0x04, +0x10, +0x08`).
    #[test]
    fn quadrant_order_matches_the_binary() {
        // one point per quadrant of the root, placed to force a split (49 points total, 48 of
        // them stacked in the low-x/low-z quadrant so the root splits, then one per quadrant to
        // check the query's visiting order across the whole root box). -99/99, not -100/100:
        // `x + 8192` must not be a multiple of the level-12 cell (4 m) or the stacked point would
        // sit exactly on a level-12 boundary and (correctly, per the now-closed-both-ends accept
        // test above) be accepted into more than one of that level's leaves, which is a different
        // thing from what this test means to check (the query's own visiting order).
        let mut pts: Vec<Point> = (0..48).map(|_| Point(-99.0, -99.0)).collect();
        pts.push(Point(-99.0, -99.0)); // low x, low z            -> visited 1st
        pts.push(Point(-99.0, 99.0)); // low x, high z            -> visited 2nd
        pts.push(Point(99.0, -99.0)); // high x, low z            -> visited 3rd
        pts.push(Point(99.0, 99.0)); // high x, high z            -> visited 4th
        let tree = QuadTree::build(&pts);
        let order = tree.query_segment((-8192.0, -8192.0), (8192.0, 8192.0));
        // the 48 stacked low/low points all land in the leaf visited first; the last four
        // entries (indices 48..52) must come back in this order regardless of the stack.
        let tail: Vec<usize> = order.into_iter().filter(|&i| i >= 48).collect();
        assert_eq!(tail, vec![48, 49, 50, 51]);
    }

    /// `0x4848B0`'s query descent (unchanged here) is a strict `<` on the low side, so
    /// a *query* box that stops exactly at the centre only ever descends into the high half.
    #[test]
    fn query_descent_at_the_centre_line_is_strict_low() {
        let mut pts: Vec<Point> = (0..48).map(|_| Point(-100.0, -100.0)).collect();
        pts.push(Point(-1.0, -1.0)); // idx 48, inside low/low, away from any boundary
        let tree = QuadTree::build(&pts);
        // a query box that never reaches x>=0 or z>=0 must not descend into the high half at all
        let high_high = tree.query_segment((0.0, 0.0), (8192.0, 8192.0));
        assert!(!high_high.contains(&48), "a point strictly inside low/low is not visited from a high-only query");
    }

    /// `0x484320`'s centroid test is **closed on both ends of both axes** (`0x48439a`…`0x4843d4`:
    /// each bound rejects only strictly outside, `test ah,1`/`test ah,0x41` — see
    /// [`Bounds::contains_point`]'s doc), unlike [`Node::query`]'s strict-low descent rule above,
    /// which is a different function (`0x4848B0`) with its own comparisons. A point exactly on
    /// the shared corner of the root's four children is therefore accepted by every one of
    /// them — `insert` stores a boundary-exact element redundantly in more than one leaf, rather
    /// than picking a single side the way the query's descent does.
    #[test]
    fn accept_test_is_closed_on_both_bounds_at_a_shared_corner() {
        let root = Bounds::root();
        let centre = root.centre(); // (0.0, 0.0), shared by all four direct children
        for q in CHILD_ORDER {
            assert!(root.child(q).contains_point(centre), "{q:?} must accept the shared corner point");
        }
    }

    /// `0x483DFB`: level 12 never splits, however many entries a leaf holds — 4 m is the finest
    /// cell there is.
    #[test]
    fn level_cap_stops_splitting_at_four_metres() {
        // stack 200 coincident points so the same cell would split past 0x30 at any depth below
        // the cap; they must all end up in one node no smaller than 4 m, i.e. the tree depth is
        // bounded (no stack overflow, no infinite recursion) and every point is still returned.
        let pts: Vec<Point> = (0..200).map(|_| Point(1.0, 1.0)).collect();
        let tree = QuadTree::build(&pts);
        assert_eq!(tree.query_segment((0.0, 0.0), (2.0, 2.0)).len(), 200);
    }
}
