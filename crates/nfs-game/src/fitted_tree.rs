//! The world's fitted spatial tree — `WTrack+0x10` (`RE/RENDERING.md` "The static items'
//! registration", `RE/notes/COVERAGE-2026-09-26.md` §14.32 / §14.36 / §14.72 A1).
//!
//! One tree serves the track walk (the node pass 0x47F400 over the items registered into it) and
//! the skid marks (`WLineBundle`'s cells, 0x479810); both read it through this module.
//!
//! * **The root** — 0x47B660's tail (0x47B8A2…0x47B94F): the world's box is the union of the
//!   *eight box corners* every placed item answers (`getBounds`, slot 2 0x4893A0, folded per axis
//!   at 0x47B6E2…0x47B75C); `L = max(Δx, Δz)`, the root a square of side `L` centred on the box in
//!   x and z (`min − (L − Δ)·0.5`), its y from `min.y − 1.0` with the height `Δy + 100.0`
//!   (`[tree+8]`, `[tree+0x80]`), and the per-level cell sizes `[tree+0x10+4k]` halving from `L`
//!   (13 halvings, 0x47B932…0x47B94F).
//! * **The items the tree is built from** — not the item centres RENDERING.md once said: 0x47B660's
//!   second loop (0x47B762…0x47B88B) walks every item `(bundle, i)`, casts the bundle to
//!   `WArticleBundle` (0x5A134B), takes the item's cursor `[bundle+0x30] + 64·i` (0x477C30), finds
//!   its `Base` part (0x43C6F0), the **first LOD record** `Base + 0x64 + 12·[Base+8]`, the `vt`
//!   part of that record's LOD (`"vt"` with the LOD in the low half, `[0x5E5084]`) and pushes a
//!   pointer to **every vertex** of it (16-byte records, `x, y, z, 1`; 0x47B80A…0x47B868) into
//!   the vector 0x47BE30 splits. So the split rule's "32 or more items" counts *vertices*.
//! * **The split** — 0x47BE30: a node's own point list is divided by the quadrant of each point
//!   against the node's centre (0x47C3A0), `q = (x ≥ cx) + 2·(z ≥ cz)`; each quadrant gets a child
//!   when the node's level is under 13 and (level ≤ 1, or **the node's own list** holds 32 or more
//!   points — 0x47BF69 compares the input list `ebx`, not the quadrant's, so a node has all four
//!   children or none, which is what 0x479810's `count == 4` test and 0x489930's `n` presuppose).
//! * **Node geometry** — 0x47C2C0 / 0x47C3A0, from the node's key (level, x, z): the cell is
//!   `[tree+0x10+4·level]`, `min = x·cell + root.min` (x87 at 53 bits, one `fstp` to a float),
//!   `max = cell + min`, centre `(x + 0.5)·cell + root.min`.
//!
//! Nodes are numbered in the order 0x47BE30 allocates them (pre-order: a child is allocated, then
//! its whole subtree, then the next quadrant), which is also the order of their addresses from
//! the pool 0x47C180 while it hands out fresh blocks — the assumption the skid cells' `lower_bound`
//! over node addresses (0x47A300) rests on (`skidmarks`).

/// One node of the tree: its key (`level`, `ix`, `iz`), its allocation number `id` and its (up to
/// four) children, slot `q = (x ≥ cx) + 2·(z ≥ cz)` (`+0x10 … +0x1C`).
#[derive(Debug, Clone)]
pub struct Node {
    pub level: u32,
    pub ix: u32,
    pub iz: u32,
    pub id: u32,
    pub children: [Option<Box<Node>>; 4],
}

impl Node {
    /// Byte `+0`'s bit 0: the node has children.
    pub fn internal(&self) -> bool {
        self.children.iter().any(Option::is_some)
    }
}

/// The fitted tree.
#[derive(Debug, Clone)]
pub struct FittedTree {
    /// `[tree+4]`, `[tree+8]`, `[tree+0xC]`: the root's minimum corner
    pub min: [f32; 3],
    /// `[tree+0x10]`: the root's side
    pub size: f32,
    /// `[tree+0x80]`: the root's height
    pub height: f32,
    /// `[tree+0x10+4k]`: the cell of level `k`
    cells: [f32; 14],
    root: Node,
    nodes: u32,
}

const SPLIT_MIN: usize = 32;
const MAX_LEVEL: u32 = 13;

impl FittedTree {
    /// 0x47B660's root from the items' corner box `bmin..bmax` (per axis min / max over every
    /// item's eight corners) and 0x47BE30's split over `points` (each vertex's `(x, z)`).
    pub fn new(bmin: [f32; 3], bmax: [f32; 3], points: &[(f32, f32)]) -> Self {
        // 0x47B8A2…0x47B909: the differences are stored as floats, `L` the larger, the minima
        // `min − (L − Δ)·0.5` (x87 intermediates at 53 bits, one store)
        let dx = (bmax[0] as f64 - bmin[0] as f64) as f32;
        let dz = (bmax[2] as f64 - bmin[2] as f64) as f32;
        let l = if dx < dz { dz } else { dx };
        let min_x = (bmin[0] as f64 - (l as f64 - dx as f64) * 0.5) as f32;
        let min_y = (bmin[1] as f64 - 1.0) as f32;
        let min_z = (bmin[2] as f64 - (l as f64 - dz as f64) * 0.5) as f32;
        let height = ((bmax[1] as f64 - bmin[1] as f64) as f32 as f64 + 100.0) as f32;
        let mut cells = [0.0f32; 14];
        cells[0] = l;
        for k in 1..14 {
            cells[k] = cells[k - 1] * 0.5;
        }
        let mut t = Self { min: [min_x, min_y, min_z], size: l, height, cells, root: Node { level: 0, ix: 0, iz: 0, id: 0, children: Default::default() }, nodes: 1 };
        let mut pts = points.to_vec();
        let mut scratch = vec![(0.0f32, 0.0f32); pts.len()];
        let mut next_id = 1u32;
        let mut root = Node { level: 0, ix: 0, iz: 0, id: 0, children: Default::default() };
        t.split(&mut root, &mut pts, &mut scratch, &mut next_id);
        t.root = root;
        t.nodes = next_id;
        t
    }

    /// 0x47BE30 over one node's own point list.
    fn split(&self, node: &mut Node, pts: &mut [(f32, f32)], scratch: &mut [(f32, f32)], next_id: &mut u32) {
        let (cx, cz) = self.centre(node);
        // the four quadrants' point counts, then the points moved into contiguous runs
        let quad = |p: &(f32, f32)| (p.0 >= cx) as usize + 2 * (p.1 >= cz) as usize;
        let mut count = [0usize; 4];
        for p in pts.iter() {
            count[quad(p)] += 1;
        }
        let mut start = [0usize; 4];
        for q in 1..4 {
            start[q] = start[q - 1] + count[q - 1];
        }
        let mut fill = start;
        for p in pts.iter() {
            let q = quad(p);
            scratch[fill[q]] = *p;
            fill[q] += 1;
        }
        pts.copy_from_slice(&scratch[..pts.len()]);
        for q in 0..4 {
            // 0x47BF69: the count compared with 32 is the node's OWN list (`ebx`, the input the partition
            // loop 0x47BED4…0x47BF45 walks), not the quadrant's — all four children or none
            let creates = node.level < MAX_LEVEL && (node.level <= 1 || pts.len() >= SPLIT_MIN);
            if creates {
                let mut child = Node { level: node.level + 1, ix: 2 * node.ix + (q as u32 & 1), iz: 2 * node.iz + (q as u32 >> 1), id: *next_id, children: Default::default() };
                *next_id += 1;
                let (a, b) = (start[q], start[q] + count[q]);
                let (sub, sub_scratch) = (&mut pts[a..b], &mut scratch[a..b]);
                self.split(&mut child, sub, sub_scratch, next_id);
                node.children[q] = Some(Box::new(child));
            }
        }
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    /// How many nodes the tree holds (the ids run `0..count`).
    pub fn node_count(&self) -> u32 {
        self.nodes
    }

    /// `[tree+0x10+4·level]`.
    pub fn cell(&self, level: u32) -> f32 {
        self.cells[level.min(13) as usize]
    }

    /// 0x47C2C0: the node's box, `(min.x, min.z)` and `(max.x, max.z)`.
    pub fn bounds(&self, n: &Node) -> ((f32, f32), (f32, f32)) {
        let cell = self.cell(n.level);
        let min_x = (n.ix as f64 * cell as f64 + self.min[0] as f64) as f32;
        let min_z = (n.iz as f64 * cell as f64 + self.min[2] as f64) as f32;
        let max_x = (cell as f64 + min_x as f64) as f32;
        let max_z = (cell as f64 + min_z as f64) as f32;
        ((min_x, min_z), (max_x, max_z))
    }

    /// 0x47C3A0: the node's centre in x and z.
    pub fn centre(&self, n: &Node) -> (f32, f32) {
        let cell = self.cell(n.level);
        let cx = ((n.ix as f64 + 0.5) * cell as f64 + self.min[0] as f64) as f32;
        let cz = ((n.iz as f64 + 0.5) * cell as f64 + self.min[2] as f64) as f32;
        (cx, cz)
    }

    /// The circle the node pass tests (0x47F400: the centre of the node's square, radius
    /// `width · 0.7071`).
    pub fn circle(&self, n: &Node) -> (f32, f32, f32) {
        let (min, max) = self.bounds(n);
        ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5, 0.7071 * (max.0 - min.0))
    }

    /// 0x47BDC0: from the root, step into the child that holds **all** of `corners` while the node
    /// is internal and exactly one child does (0x47C490's inclusive branch: `min < p ≤ max` on both
    /// axes); the smallest node holding the whole set.
    pub fn locate(&self, corners: &[(f32, f32)]) -> &Node {
        let mut node = &self.root;
        loop {
            if !node.internal() {
                return node;
            }
            let mut found = None;
            let mut n = 0;
            for q in 0..4 {
                if n >= 2 {
                    break;
                }
                let Some(c) = &node.children[q] else { continue };
                let (min, max) = self.bounds(c);
                if corners.iter().all(|p| min.0 < p.0 && min.1 < p.1 && p.0 <= max.0 && p.1 <= max.1) {
                    found = Some(q);
                    n += 1;
                }
            }
            match (n, found) {
                (1, Some(q)) => node = node.children[q].as_deref().expect("a child"),
                _ => return node,
            }
        }
    }

    /// 0x489930: the budgeted spread of a footprint `hull` from `start` (`weight` 1, `spent` 0):
    /// at an internal node, the children (of those that exist) whose box the hull overlaps
    /// (0x5A0790, [`polygon_rect_overlap`]) are counted, `n`, and
    /// * `n = 1` — recurse into it (nothing spent);
    /// * `n = 2` or `3` — recurse into every one while `weight + spent < 8`, else push this node;
    /// * `n = 4` — recurse into all while this node's level is under 2, else push this node;
    /// * `n = 0` — push this node when `spent`'s low byte is 0 (0x4899F8: `al = byte spent`), and
    ///   **nothing** otherwise (the recursion's mask is empty);
    ///
    /// with `weight + 1` and, for `n ≥ 2`, `spent + n·weight` passed on (0x4899DD / 0x4899F1). A
    /// leaf is pushed as it is. Nodes come out in push order.
    pub fn spread<'a>(&'a self, start: &'a Node, hull: &[(f32, f32)]) -> Vec<&'a Node> {
        let mut out = Vec::new();
        self.spread_at(start, hull, 1, 0, &mut out);
        out
    }

    fn spread_at<'a>(&'a self, node: &'a Node, hull: &[(f32, f32)], weight: u32, spent: u32, out: &mut Vec<&'a Node>) {
        if !node.internal() {
            out.push(node);
            return;
        }
        let mut mask = [false; 4];
        let mut n = 0u32;
        for q in 0..4 {
            if let Some(c) = &node.children[q] {
                let (min, max) = self.bounds(c);
                if polygon_rect_overlap(hull, min, max) {
                    mask[q] = true;
                    n += 1;
                }
            }
        }
        let (recurse, new_spent) = match n {
            0 => (spent & 0xFF != 0, spent),
            1 => (true, spent),
            2 | 3 => (weight + spent < 8, spent + n * weight),
            _ => (node.level < 2, spent + n * weight),
        };
        if !recurse {
            out.push(node);
            return;
        }
        for q in 0..4 {
            if mask[q] {
                self.spread_at(node.children[q].as_deref().expect("a child"), hull, weight + 1, new_spent, out);
            }
        }
    }

    /// 0x47BC20: the node an item's bounding sphere `(x, y, z, r)` registers into (the dynamic
    /// path of 0x489530, bundles whose `+5` is set): the root when the sphere is not wholly inside
    /// the root's box, else a descent by the sphere's centre that stops at the first node whose
    /// centre is nearer than `r` on the nearer axis, or whose child slot is empty.
    pub fn node_for_sphere(&self, x: f32, y: f32, z: f32, r: f32) -> &Node {
        let (xd, yd, zd, rd) = (x as f64, y as f64, z as f64, r as f64);
        let (l, h) = (self.size as f64, self.height as f64);
        if !(self.min[0] as f64 <= xd - rd && self.min[1] as f64 <= yd - rd && self.min[2] as f64 <= zd - rd) {
            return &self.root;
        }
        // 0x47BC88…0x47BCDC: x against `L + min.x` at full precision, y and z against sums
        // stored as floats
        let top_y = (h + self.min[1] as f64) as f32 as f64;
        let top_z = (self.min[2] as f64 + l) as f32 as f64;
        if xd + rd > l + self.min[0] as f64 || yd + rd > top_y || zd + rd > top_z {
            return &self.root;
        }
        let mut node = &self.root;
        loop {
            if !node.internal() {
                return node;
            }
            let (cx, cz) = self.centre(node);
            let (dx, dz) = ((xd - cx as f64).abs(), (zd - cz as f64).abs());
            let near = if dz < dx { dz } else { dx };
            if near < rd {
                return node;
            }
            let q = (xd >= cx as f64) as usize + 2 * (zd >= cz as f64) as usize;
            match &node.children[q] {
                Some(c) => node = c,
                None => return node,
            }
        }
    }

    /// 0x47BAD0: the node a position falls in, descending while the node is internal and fewer
    /// than `cap` steps were taken (the skid marks' chosen level), stopping at an empty child slot;
    /// `None` outside the root's box.
    pub fn descend(&self, x: f32, y: f32, z: f32, cap: u32) -> Option<&Node> {
        let (xd, yd, zd) = (x as f64, y as f64, z as f64);
        if !(self.min[0] as f64 <= xd && self.min[1] as f64 <= yd && self.min[2] as f64 <= zd) {
            return None;
        }
        let top_y = (self.height as f64 + self.min[1] as f64) as f32 as f64;
        let top_z = (self.min[2] as f64 + self.size as f64) as f32 as f64;
        if xd > self.size as f64 + self.min[0] as f64 || yd > top_y || zd > top_z {
            return None;
        }
        let mut node = &self.root;
        let mut steps = 0;
        while node.internal() && steps < cap {
            let (cx, cz) = self.centre(node);
            let q = (xd >= cx as f64) as usize + 2 * (zd >= cz as f64) as usize;
            steps += 1;
            match &node.children[q] {
                Some(c) => node = c,
                None => break,
            }
        }
        Some(node)
    }

    /// Every node in allocation order (pre-order), for tests and censuses.
    pub fn walk(&self, f: &mut dyn FnMut(&Node)) {
        fn go(n: &Node, f: &mut dyn FnMut(&Node)) {
            f(n);
            for c in n.children.iter().flatten() {
                go(c, f);
            }
        }
        go(&self.root, f);
    }
}

/// The epsilon 0x5A0480 tests squared distances against (`[0x5C18E4]`, `1e-8f`).
const HULL_EPS: f64 = 9.999_999_939_225_29e-9;

/// 0x5A0480 (`count`, `points`, `out`): the convex hull of the points' `(x, z)`, built by adding
/// each point in turn (the input order matters).
///
/// The first point starts the list; the first later point farther than `√1e-8` from it is the
/// second (`0x5A04F0…0x5A0544`). Every later point is tested against each edge `j → j+1` of the
/// list (wrapping): `flag[j] = bx·az − bz·ax ≥ 0` with `a = next − cur`, `b = point − cur`
/// (`0x5A0563…0x5A0602`); with no false flag the point is inside and dropped; else the (circular)
/// run of false flags starts at `s` (`0x5A0617…0x5A064E`) and the list becomes `point`, then the
/// vertices from `s + m` round to `s` (`m` the number of false flags; `0x5A064F…0x5A06D2`), with
/// each vertex less than `√1e-8` from the one before it dropped (`0x5A06D4…0x5A0769`). All the
/// subtractions are stored as floats, the products and their sum kept at 53 bits.
pub fn convex_hull(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let Some(&first) = pts.first() else { return Vec::new() };
    let mut out = vec![first];
    let mut i = 1;
    // 0x5A04F0: while the list holds one point
    while i < pts.len() && out.len() == 1 {
        let p = pts[i];
        let dx = (p.0 as f64 - out[0].0 as f64) as f32;
        let dz = (p.1 as f64 - out[0].1 as f64) as f32;
        if dz as f64 * dz as f64 + dx as f64 * dx as f64 > HULL_EPS {
            out.push(p);
        }
        i += 1;
    }
    for &p in &pts[i..] {
        let h = out.len();
        let mut flag = vec![false; h];
        let mut m = 0usize;
        for j in 0..h {
            let (cur, nxt) = (out[j], out[(j + 1) % h]);
            let ax = (nxt.0 as f64 - cur.0 as f64) as f32;
            let az = (nxt.1 as f64 - cur.1 as f64) as f32;
            let bx = (p.0 as f64 - cur.0 as f64) as f32;
            let bz = (p.1 as f64 - cur.1 as f64) as f32;
            let cross = bx as f64 * az as f64 - bz as f64 * ax as f64;
            // `fcomp` then `test ah, 1`: C0 is set for less-than and for an unordered compare
            flag[j] = !(cross < 0.0 || cross.is_nan());
            if !flag[j] {
                m += 1;
            }
        }
        if m == 0 {
            continue;
        }
        // the start of the circular run of false flags
        let s = if flag[0] {
            (1..h).find(|&k| !flag[k]).unwrap_or(1)
        } else if flag[h - 1] {
            0
        } else {
            let mut e = h - 1;
            loop {
                let b = flag[e - 1];
                e -= 1;
                if b {
                    break;
                }
            }
            e + 1
        };
        let c = h - m + 1;
        let mut tmp = Vec::with_capacity(c + 1);
        tmp.push(p);
        for t in 0..c {
            tmp.push(out[(s + m + t) % h]);
        }
        // 0x5A06D4…0x5A0769: drop a vertex within √eps of the one before it
        let mut n_new = tmp.len();
        let mut kept = vec![tmp[0]];
        let mut k = 1;
        while kept.len() < n_new && k < tmp.len() {
            let dx = (tmp[k - 1].0 as f64 - tmp[k].0 as f64) as f32;
            let dz = (tmp[k - 1].1 as f64 - tmp[k].1 as f64) as f32;
            if dz as f64 * dz as f64 + dx as f64 * dx as f64 > HULL_EPS {
                kept.push(tmp[k]);
            } else {
                n_new -= 1;
            }
            k += 1;
        }
        out = kept;
    }
    out
}

/// 0x5A0790 (`count`, `polygon`, `min`, `max`): does the polygon touch the rectangle `min..max`?
/// Three tests in turn, each answering true at once:
/// 1. a polygon point inside the rectangle, both sides inclusive (0x5A07A9…0x5A07E7);
/// 2. the rectangle's minimum corner inside the polygon — every edge `p → q` has
///    `(q−min).x·(p−min).z − (q−min).z·(p−min).x ≥ 0` (0x5A07F1…0x5A085F);
/// 3. some polygon edge `p + t·e` meets some rectangle edge `r + s·d` (0x5A086B…0x5A0B2D): with
///    `w = d.x·e.z − d.z·e.x`, `u = (r−p).z·e.x − (r−p).x·e.z`, `v = (r−p).z·d.x − (r−p).x·d.z` —
///    for `w > 0` when `0 ≤ u ≤ w` and `0 ≤ v ≤ w`; for `w < 0` when `w ≤ u ≤ 0` and `w ≤ v ≤ 0`;
///    for `w = 0` (parallel — collinearity is **not** checked) when the polygon edge's x range
///    (horizontal rectangle edge) or z range (vertical one) reaches the rectangle edge's span,
///    tested with the edge's start point and then its end point.
///
/// A polygon with one point stops after test 1 (the original reads a null table for it).
pub fn polygon_rect_overlap(poly: &[(f32, f32)], min: (f32, f32), max: (f32, f32)) -> bool {
    let n = poly.len();
    if n == 0 {
        return true;
    }
    // 1. a polygon point inside the rectangle
    if poly.iter().any(|p| !(p.0 < min.0) && p.0 <= max.0 && !(p.1 < min.1) && p.1 <= max.1) {
        return true;
    }
    if n < 2 {
        return false;
    }
    // 2. the rectangle's minimum corner inside the polygon
    let mut inside = true;
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let px = (p.0 as f64 - min.0 as f64) as f32;
        let pz = (p.1 as f64 - min.1 as f64) as f32;
        let qx = (q.0 as f64 - min.0 as f64) as f32;
        let qz = (q.1 as f64 - min.1 as f64) as f32;
        let cross = qx as f64 * pz as f64 - qz as f64 * px as f64;
        inside = !(cross < 0.0 || cross.is_nan());
        if !inside {
            break;
        }
    }
    if inside {
        return true;
    }
    // 3. the edges against the rectangle's four
    let dx = (max.0 as f64 - min.0 as f64) as f32;
    let dz = (max.1 as f64 - min.1 as f64) as f32;
    let neg_dx = (min.0 as f64 - max.0 as f64) as f32;
    let neg_dz = (min.1 as f64 - max.1 as f64) as f32;
    // per rectangle edge: start `(rx0, rz0)` and direction `(rdx, rdz)`
    let edges: [((f32, f32), (f32, f32)); 4] = [((min.0, min.1), (dx, 0.0)), ((max.0, min.1), (0.0, dz)), ((max.0, max.1), (neg_dx, 0.0)), ((min.0, max.1), (0.0, neg_dz))];
    for i in 0..n {
        let (p, q) = (poly[i], poly[(i + 1) % n]);
        let ex = (q.0 as f64 - p.0 as f64) as f32;
        let ez = (q.1 as f64 - p.1 as f64) as f32;
        for &((rx0, rz0), (rdx, rdz)) in &edges {
            let t1 = (rz0 as f64 - p.1 as f64) as f32;
            let t2 = (rx0 as f64 - p.0 as f64) as f32;
            let u = (t1 as f64 * ex as f64 - t2 as f64 * ez as f64) as f32;
            let v = (t1 as f64 * rdx as f64 - t2 as f64 * rdz as f64) as f32;
            let w = (rdx as f64 * ez as f64 - rdz as f64 * ex as f64) as f32;
            if w > 0.0 {
                if !(u < 0.0) && u <= w && !(v < 0.0) && v <= w {
                    return true;
                }
            } else if w < 0.0 {
                if u <= 0.0 && !(u < w) && v <= 0.0 && !(v < w) {
                    return true;
                }
            } else if rdz == 0.0 {
                // parallel to a horizontal rectangle edge: the polygon edge's x range
                let reach = rx0 as f64 + rdx as f64;
                if !(p.0 < rx0) && !(reach < p.0 as f64) {
                    return true;
                }
                let qx = (ex as f64 + p.0 as f64) as f32;
                if !(qx < rx0) && !(reach < qx as f64) {
                    return true;
                }
            } else {
                // parallel to a vertical one: the z range
                let reach = rz0 as f64 + rdz as f64;
                if !(p.1 < rz0) && !(reach < p.1 as f64) {
                    return true;
                }
                let qz = (ez as f64 + p.1 as f64) as f32;
                if !(qz < rz0) && !(reach < qz as f64) {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_hulls_to_its_four_corners_in_the_original_order() {
        // the eight corners of an axis-aligned box (0x4893A0's order): x/z pairs repeat
        let c = [(0.0, 0.0), (4.0, 0.0), (4.0, 0.0), (0.0, 0.0), (0.0, 2.0), (4.0, 2.0), (4.0, 2.0), (0.0, 2.0)];
        let h = convex_hull(&c);
        assert_eq!(h.len(), 4);
        for p in [(0.0, 0.0), (4.0, 0.0), (4.0, 2.0), (0.0, 2.0)] {
            assert!(h.contains(&p), "{p:?} in {h:?}");
        }
    }

    #[test]
    fn a_point_inside_the_hull_changes_nothing() {
        let c = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (5.0, 5.0)];
        assert_eq!(convex_hull(&c).len(), 4);
    }

    #[test]
    fn a_rotated_box_overlaps_only_what_it_touches() {
        // a thin diagonal strip from (0,0) to (100,100), width ~2: its axis-aligned box covers the
        // rectangle (60..100, 0..20), the strip itself does not reach it
        let strip = [(0.0, 1.0), (1.0, 0.0), (100.0, 99.0), (99.0, 100.0)];
        let h = convex_hull(&strip);
        assert!(polygon_rect_overlap(&h, (0.0, 0.0), (10.0, 10.0)));
        assert!(!polygon_rect_overlap(&h, (60.0, 0.0), (100.0, 20.0)));
        assert!(polygon_rect_overlap(&h, (45.0, 45.0), (55.0, 55.0)));
    }

    #[test]
    fn a_rectangle_wholly_inside_the_polygon_overlaps() {
        let h = convex_hull(&[(0.0, 0.0), (100.0, 0.0), (100.0, 100.0), (0.0, 100.0)]);
        assert!(polygon_rect_overlap(&h, (40.0, 40.0), (60.0, 60.0)));
        assert!(!polygon_rect_overlap(&h, (200.0, 200.0), (300.0, 300.0)));
    }

    #[test]
    fn the_tree_splits_by_vertex_count_and_locates_a_box() {
        // 4 x 4 clusters of 40 points across a 160 m square: every quadrant to level 3 holds >= 32
        let mut pts = Vec::new();
        for cx in 0..4 {
            for cz in 0..4 {
                for k in 0..40 {
                    pts.push((cx as f32 * 40.0 + 10.0 + (k % 5) as f32, cz as f32 * 40.0 + 10.0 + (k / 5) as f32));
                }
            }
        }
        let t = FittedTree::new([0.0, 0.0, 0.0], [160.0, 5.0, 160.0], &pts);
        assert_eq!(t.size, 160.0);
        assert_eq!(t.min, [0.0, -1.0, 0.0]);
        let mut levels = [0u32; 14];
        t.walk(&mut |n| levels[n.level as usize] += 1);
        assert_eq!(levels[..3], [1, 4, 16]);
        // a box wholly inside one level-2 cell (40 m) is found there or deeper
        let n = t.locate(&[(10.0, 10.0), (20.0, 10.0), (20.0, 20.0), (10.0, 20.0)]);
        assert!(n.level >= 2);
        // a box across the middle stays at the root
        let n = t.locate(&[(70.0, 70.0), (90.0, 90.0)]);
        assert_eq!(n.level, 0);
    }

    #[test]
    fn a_sphere_nearer_the_plane_than_its_radius_stays_above_it() {
        let mut pts = Vec::new();
        for i in 0..64 {
            for j in 0..64 {
                pts.push((i as f32 * 2.5, j as f32 * 2.5));
            }
        }
        let t = FittedTree::new([0.0, 0.0, 0.0], [160.0, 5.0, 160.0], &pts);
        // at the root's centre plane with a radius over the distance: the root itself
        assert_eq!(t.node_for_sphere(80.0, 1.0, 80.0, 1.0).level, 0);
        // well inside a quadrant: deeper
        assert!(t.node_for_sphere(20.0, 1.0, 20.0, 0.5).level >= 2);
        // outside the box: the root
        assert_eq!(t.node_for_sphere(1000.0, 1.0, 1000.0, 0.5).level, 0);
    }
}
