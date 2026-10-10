//! Skid marks — `WLineBundle` (vtable 0x5B4750, the one object at `[0x628B70]`), fed per wheel by
//! `Car_wheelEffects` 0x4A2880 through the emitter 0x478F30 and its append 0x479190, drawn by
//! 0x478AA0 (`RE/notes/particles.md` §0).
//!
//! Each wheel's emitter owns a **strip**: pairs of points `p ∓ half·(d.z, 0, −d.x)` across the
//! wheel's track, `d` the car's direction of travel, one ARGB per pair — the surface colour with
//! its alpha scaled by the emitter's strength. Between commits the newest pair follows the wheel
//! (it is dropped and laid again every call); every eighth call (`counter & [0x6197EC]`, 7 at
//! `SPECIALFX` 2 and 3) it stays and a new head is laid. A fresh strip opens with an invisible
//! pair, so a mark fades in; a change of surface closes the strip and opens another seeded with
//! the old head; a full strip continues into a new one the same way. Strips come out of a ring
//! that frees the oldest in-use strip when it runs out (0x478C10).
//!
//! Strips live **per cell** of the world's fitted tree ([`crate::fitted_tree`], `WTrack+0x10`; the
//! bundle's constructor 0x479900 takes it). The cells are cut by 0x479810 from the root down: a
//! node with all four children and a level under the **chosen level** — the last level whose cell
//! is still `[0x5B242C]` 100 or more (0x479972…0x4799D0) — hands its four children on; any other
//! node (at the chosen level, or a leaf) becomes a cell, its record
//! (0x478D90) keeping the node's key, the node's centre `(cx, cz)` and the sphere
//! `(cx, 0, cz)` of radius `√(2·(max.x − cx)²)` that slot 3 (0x479FC0) answers. A position finds
//! its cell in two steps (0x479190 / 0x478F30): 0x47BAD0 descends the tree from the root while the
//! node is internal and fewer steps than the chosen level were taken, stopping at an empty child
//! slot, and 0x47A300 — a `lower_bound` — takes the first cell record whose key is not below the
//! node's; the cell records are sorted by the nodes' *addresses*, which is the order 0x479810
//! creates them in (nodes come from the pool 0x47C180, handed out in the order 0x47BE30 allocates
//! them — see [`crate::fitted_tree`]). Every internal node has all four children (0x47BF69 compares the
//! node's own list with 32), so the node found is itself a cell whenever it lies at or above the
//! chosen level; the `lower_bound` stays as the original's own step. 20 strips to a cell, and a wheel that leaves its cell
//! opens a new strip in the next (0x4793AE…0x479452). A strip holds 192 points on a world under
//! 1200 across, else 12 (0x47993F…0x47995C).
//!
//! The tree's items are the vertices of the placed articles' first-LOD `vt` parts, its root the
//! union of their box corners (`crate::world::tree_population`, 0x47B660). **Observed on the
//! original (2026-10-01 SV, Wine, gdb at 0x47A300 and at the pool 0x47C180):** the cell table IS sorted
//! ascending by the key at `+0x34` (457 of 457 records on industrial, the whole table each call; 193
//! on alps), but the keys are two size classes of pool blocks (the 0x20 nodes of 0x47BF7E and the 0x10
//! list items of 0x47BFCD) and the pool hands them out in DEscending address order (LIFO free lists:
//! over the ascending keys the allocation index falls on 200 of 205 adjacent pairs), so "ascending
//! addresses from the pool" was wrong. It changes nothing the port draws: 0x479810 creates a record
//! for every node down to the chosen level, so every lookup of 0x479190 / 0x478F30 is an exact hit and
//! the `lower_bound` never lands on a neighbour; the order the port keys the cells in is immaterial and
//! is kept as the node id.

use crate::fitted_tree::{FittedTree, Node};
use crate::particles::SURFACE_ARGB;
use glam::Vec3;
use std::sync::Arc;

/// Strips to a cell (0x479780 builds 20; 0x478C10 wraps at 0x14).
pub const PER_CELL: usize = 20;
/// `[0x6197EC]` at `SPECIALFX` 2 and 3 (0x44EE9A, 0x44EEE3); the marks are off below that.
pub const COMMIT_MASK: u32 = 7;
/// 0x478AA0 draws a strip only when both ends are within `√[0x5B2764]` of the camera.
pub const DRAW_DISTANCE: f32 = 100.0;
/// The most points a strip can hold: `[0x628B60]` is 12 or this.
pub const MAX_CAPACITY: usize = 192;

#[derive(Debug, Clone, Default)]
pub struct Strip {
    /// the low nibble of the strip's first dword: 1 free, 8 a mark, 4 a kind-6 mark (0x478C10)
    state: u8,
    owner: Option<usize>,
    pub points: Vec<Vec3>,
    /// one ARGB per point (`strip+8`), the pair sharing it
    pub colours: Vec<u32>,
}

impl Strip {
    /// Drawn by 0x47A060 → 0x478AA0: in use and more than two points.
    pub fn visible(&self) -> bool {
        self.state != 1 && self.points.len() > 2
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Emitter {
    /// `+0x00`: half the mark's width
    half: f32,
    /// `+0x08`: the strip, `-1` for none
    strip: Option<StripRef>,
    /// `+0x04`: the cell, `-1` before the first call
    cell: Option<u32>,
    /// `+0x0C`: the kind it was last given
    kind: usize,
    /// `+0x10`: the strip state it asks for, 4 for kind 6 and 8 for the rest
    code: u8,
    /// `+0x14`: calls above the threshold
    counter: u32,
    /// `+0x20`: 0 running, 1 a new strip, 2 stopped, 3 the kind changed
    state: u8,
}

/// One cell record of `WLineBundle` (0x50 bytes): the tree node it is cut at (the key the
/// original's `lower_bound` compares, 0x47A300) and the sphere 0x478D90 keeps at `+0x24`:
/// `(cx, 0, cz)` of the node's centre and `√(2·(max.x − cx)²)` (0x478DE6…0x478E13), which the
/// bundle's slot 3 (0x479FC0) hands the track walk.
#[derive(Debug, Clone)]
struct Cell {
    node: u32,
    cx: f32,
    /// the sphere's `y` (`+0x28`): 0 from 0x478D90, set to the height of the first mark a cell
    /// takes (0x479190 at 0x47963A…0x47965D: while `|y| < 0.001`, `y = point.y`)
    cy: f32,
    cz: f32,
    r: f32,
    /// the nodes the cell record is registered into: the bundle's getBounds 0x47A000 is
    /// the cube `(cx, ·, cz) ± 0.1` (`[0x5B2604]`), through 0x4893A0's eight corners, the hull
    /// 0x5A0480, 0x47BDC0 and the budgeted spread 0x489930 like any `+5`-clear bundle's item
    /// (0x489530); the record `+0x24` centre is the node's own centre, so the cube straddles its
    /// four children and the spread goes down the four corners toward the centre
    nodes: Vec<crate::world::RegisteredNode>,
}

/// The marks' cells: the fitted tree, the chosen level and the cell table 0x479810 cuts from it,
/// in node-allocation order.
#[derive(Debug, Clone)]
struct Grid {
    tree: Arc<FittedTree>,
    chosen_level: u32,
    cells: Vec<Cell>,
}

impl Grid {
    /// 0x479972…0x4799D0 and 0x479810 over `tree`.
    fn new(tree: Arc<FittedTree>) -> Self {
        // the last level whose cell is 100 or more: step up while `cell[level] ≥ 100`, then one back
        let mut level = 0u32;
        while level < 13 && tree.cell(level) >= 100.0 {
            level += 1;
        }
        let chosen_level = level.saturating_sub(1);
        let mut cells = Vec::new();
        Self::cut(&tree, tree.root(), chosen_level, &mut cells);
        Self { tree, chosen_level, cells }
    }

    /// 0x479810: a node with all four children and a level under the chosen level recurses into
    /// them in slot order; any other node is a cell.
    fn cut(tree: &FittedTree, node: &Node, chosen: u32, out: &mut Vec<Cell>) {
        let children = node.children.iter().flatten().count();
        if node.level < chosen && children == 4 {
            for c in node.children.iter().flatten() {
                Self::cut(tree, c, chosen, out);
            }
            return;
        }
        let ((_, _), (max_x, _)) = tree.bounds(node);
        let (cx, cz) = tree.centre(node);
        let d = (max_x as f64 - cx as f64) as f32;
        let r = ((2.0 * d as f64 * d as f64) as f32 as f64).sqrt() as f32;
        // 0x47A000 (`fsub` / `fadd` of 0x3DCCCCCD to the record's centre, stored as floats), the
        // corner order of 0x4893A0 as `world::build_article_nodes` has it
        let (lo, hi) = ((cx - 0.1, cz - 0.1), (cx + 0.1, cz + 0.1));
        let corners = [(lo.0, lo.1), (hi.0, lo.1), (hi.0, lo.1), (lo.0, lo.1), (lo.0, hi.1), (hi.0, hi.1), (hi.0, hi.1), (lo.0, hi.1)];
        let hull = crate::fitted_tree::convex_hull(&corners);
        let start = tree.locate(&corners);
        let nodes = tree
            .spread(start, &hull)
            .into_iter()
            .map(|n| {
                let (cx, cz, r) = tree.circle(n);
                crate::world::RegisteredNode { id: n.id, cx, cz, r }
            })
            .collect();
        out.push(Cell { node: node.id, cx, cy: 0.0, cz, r, nodes });
    }

    /// 0x47BAD0 then 0x47A300: the index of the cell record for a position. A position outside the
    /// tree's box answers a null node (key 0), which finds the first record. When no record's key
    /// reaches the node's (a node past the last cell in allocation order) the original indexes
    /// past its table; the last cell stands in (APPROXIMATION).
    fn cell_of(&self, p: Vec3) -> u32 {
        let key = self.tree.descend(p.x, p.y, p.z, self.chosen_level).map_or(0, |n| n.id);
        let i = self.cells.partition_point(|c| c.node < key);
        i.min(self.cells.len().saturating_sub(1)) as u32
    }
}

/// One cell's ring of strips (`+0x38` of its record): its strips, `+0x3C` the strips set free and
/// not yet handed out, `+0x4C` the ring's cursor. `HashMap` key: the cell's index in the table
/// ([`Grid::cell_of`]'s answer, the original's `(record − table) / 0x50`).
#[derive(Debug, Clone)]
struct Ring {
    strips: Vec<Strip>,
    free: usize,
    cursor: usize,
}

impl Ring {
    fn new() -> Self {
        Self { strips: (0..PER_CELL).map(|_| Strip { state: 1, ..Strip::default() }).collect(), free: PER_CELL, cursor: 0 }
    }
}

type StripRef = (u32, usize);

#[derive(Debug, Clone)]
pub struct SkidMarks {
    grid: Grid,
    capacity: usize,
    rings: std::collections::HashMap<u32, Ring>,
    emitters: Vec<Emitter>,
}

impl Default for SkidMarks {
    fn default() -> Self {
        Self::new()
    }
}

/// The half width 0x478EA0 gives a wheel's emitter from the `.sim` tyre width in mm
/// (`carSpecs+0x84`): `× [0x5B4744] 0.0005`, then `× [0x5B246C] 1.5` with the effects byte
/// `[0x6197E7]` set, else `× [0x5B25E8] 0.9`.
pub fn half_width(tyre_width_mm: f32, effects: bool) -> f32 {
    tyre_width_mm * 0.0005 * if effects { 1.5 } else { 0.9 }
}

/// 0x479200…0x479234: the surface colour with **only its alpha** scaled by the strength —
/// `A' = ((A · k) >> 8) & 0xFF`, `k = rint(strength · 255)`, the fixed-point product wrapping the way
/// the 32-bit multiply does.
pub fn surface_colour(kind: usize, strength: f32) -> u32 {
    let [a, r, g, b] = SURFACE_ARGB[kind & 15];
    let k = (strength * 255.0).round_ties_even() as i64;
    let a = (((a as i64 * k) >> 8) & 0xFF) as u32;
    (a << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

impl SkidMarks {
    /// A world of no size, no items: one endless cell, 192 points a strip.
    pub fn new() -> Self {
        Self::for_world_items(&[], Vec3::ZERO, Vec3::ZERO)
    }

    /// 0x479900 over the world's box with no item population — the chosen level and capacity
    /// (below) but a tree that only ever carries its two mandatory levels (0x47BE30's `level <= 1`
    /// rule), so every real cell coarsens to `L/4`. Kept for callers with no track geometry to hand
    /// (tests, tools); [`Self::for_tree`] is the one `App`'s race load uses.
    pub fn for_world(min: Vec3, max: Vec3) -> Self {
        Self::for_world_items(&[], min, max)
    }

    /// [`Self::for_tree`] over a tree built from the world box `min..max` and `points` (each
    /// vertex's `(x, z)`, [`FittedTree::new`]).
    pub fn for_world_items(points: &[(f32, f32)], min: Vec3, max: Vec3) -> Self {
        Self::for_tree(Arc::new(FittedTree::new(min.to_array(), max.to_array(), points)))
    }

    /// 0x479900 over the world's fitted tree: the cells 0x479810 cuts at the chosen level, and the
    /// strips' capacity (0x47993F…0x47995C: 192 while the root's side is under
    /// `[0x5B4748]` 1200, else 12).
    pub fn for_tree(tree: Arc<FittedTree>) -> Self {
        let capacity = if tree.size < 1200.0 { MAX_CAPACITY } else { 12 };
        Self { grid: Grid::new(tree), capacity, rings: Default::default(), emitters: Vec::new() }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn clear(&mut self) {
        self.rings.clear();
        self.emitters.clear();
    }

    pub fn strips(&self) -> impl Iterator<Item = &Strip> {
        self.rings.values().flat_map(|r| r.strips.iter()).filter(|s| s.visible())
    }

    /// The drawn strips by cell, in cell-table order and each cell's strips in array order — the
    /// order the bundle's draw 0x47A060 walks them (`+0x38` array, 0x14 bytes a strip, 20 of them),
    /// with the cell's own queue key ([`Self::cell_key`]).
    pub fn cells<'a>(&'a self, eye: Vec3) -> impl Iterator<Item = (u32, Vec<&'a Strip>)> + 'a {
        let mut ids: Vec<u32> = self.rings.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter().map(move |id| (self.cell_key(id, eye), self.rings[&id].strips.iter().filter(|s| s.visible()).collect()))
    }

    /// The cell record a position finds (0x47BAD0 + 0x47A300) — the sphere and the registered nodes
    /// the walk sees (a census hook: `nfs-game/examples/skidwalk.rs`).
    pub fn cell_at(&self, p: Vec3) -> Option<(f32, f32, f32, f32, &[crate::world::RegisteredNode])> {
        let c = self.grid.cells.get(self.grid.cell_of(p) as usize)?;
        Some((c.cx, c.cy, c.cz, c.r, &c.nodes))
    }

    /// [`Self::cells`] through the track walk's gates (0x47F890 for an item of
    /// `WLineBundle`): `visit(cx, cz, r, registered nodes)` answers the value the walk hands the
    /// drawer's queue, or `None` for a cell the walk drops (`crate::world::CellWalk::visit`); the
    /// cells the walk keeps come in table order with their strips.
    pub fn cells_walked(&self, visit: impl Fn(f32, f32, f32, f32, &[crate::world::RegisteredNode]) -> Option<u32>) -> Vec<(u32, Vec<&Strip>)> {
        let mut ids: Vec<u32> = self.rings.keys().copied().collect();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|id| {
                let strips: Vec<&Strip> = self.rings[&id].strips.iter().filter(|s| s.visible()).collect();
                if strips.is_empty() {
                    return None;
                }
                let c = self.grid.cells.get(id as usize)?;
                Some((visit(c.cx, c.cy, c.cz, c.r, &c.nodes)?, strips))
            })
            .collect()
    }

    /// The `+0xC` key the walk hands the drawer's queue (0x44FFC0: `round(v)`) for a cell: the
    /// non-`bl` item's value of 0x47F890 (`RE/RENDERING.md` "The walk"), `s²` with
    /// `s = max(√d² − r, 0)` (the bit-trick root, [`crate::world::bit_root`]), `d²` the 3-D square
    /// distance from the eye to the cell's sphere `(cx, cy, cz)` of radius `r` (0x479FC0), rounded to
    /// even. The walk's own gates for a cell are [`Self::cells_walked`]'s.
    pub fn cell_key(&self, cell: u32, eye: Vec3) -> u32 {
        let Some(c) = self.grid.cells.get(cell as usize) else { return 0 };
        let d2 = (c.cx - eye.x).powi(2) + (c.cy - eye.y).powi(2) + (c.cz - eye.z).powi(2);
        let s = (crate::world::bit_root(d2) - c.r).max(0.0);
        (s * s).round_ties_even() as u32
    }

    /// Car_init's emitter per wheel (0x410FCD → 0x478EA0), `id` the port's own numbering.
    pub fn set_width(&mut self, id: usize, half: f32) {
        if self.emitters.len() <= id {
            self.emitters.resize(id + 1, Emitter::default());
        }
        self.emitters[id].half = half;
    }

    fn strip(&self, r: StripRef) -> &Strip {
        &self.rings[&r.0].strips[r.1]
    }

    fn strip_mut(&mut self, r: StripRef) -> &mut Strip {
        &mut self.rings.get_mut(&r.0).expect("a ring").strips[r.1]
    }

    /// 0x478C10 on a cell's ring (made by 0x479780 on first use): with nothing free, free the next
    /// in-use strip after the cursor; then hand out the next free strip from the cursor on, or
    /// none when every strip is taken. (The original also ages up to two `8` strips to `4` while
    /// fewer than two are free; nothing reads the difference but the freeing, which takes both,
    /// so it is not carried.)
    fn alloc(&mut self, cell: u32, code: u8, owner: usize) -> Option<StripRef> {
        let r = self.rings.entry(cell).or_insert_with(Ring::new);
        let n = r.strips.len();
        if r.free == 0 {
            let mut i = (r.cursor + 1) % n;
            loop {
                if r.strips[i].state & 0xC != 0 {
                    r.strips[i].state = 1;
                    r.strips[i].owner = None;
                    r.free += 1;
                    break;
                }
                i = (i + 1) % n;
                if i == r.cursor {
                    break;
                }
            }
        }
        let start = r.cursor;
        while r.strips[r.cursor].state != 1 {
            r.cursor = (r.cursor + 1) % n;
            if r.cursor == start {
                return None;
            }
        }
        let s = &mut r.strips[r.cursor];
        s.state = code;
        s.owner = Some(owner);
        s.points.clear();
        s.colours.clear();
        r.free = r.free.saturating_sub(1);
        Some((cell, r.cursor))
    }

    fn push(&mut self, strip: StripRef, a: Vec3, b: Vec3, colour: u32) {
        let s = self.strip_mut(strip);
        s.points.extend([a, b]);
        s.colours.extend([colour, colour]);
    }

    /// The emitter 0x478F30 for one wheel and one call: `strength` against the threshold `a`, scaled
    /// by `b`, `kind` the surface, `ground` where the mark is laid and `dir` the car's direction of
    /// travel (`car+0x348`). The caller gates it on the effects byte `[0x6197E7]`.
    #[allow(clippy::too_many_arguments)]
    pub fn emit(&mut self, id: usize, ground: Vec3, strength: f32, kind: usize, dir: Vec3, a: f32, b: f32) {
        if self.emitters.len() <= id {
            return;
        }
        // 0x478F40…0x478F4D: `fcomp` then `test ah, 0x41` — C0 or C3, which an unordered compare
        // sets too, so a NaN strength stops the emitter as `strength <= a` would not
        if !(strength > a) {
            self.emitters[id].state = 2;
            return;
        }
        let strength = (strength - a) * b;
        let code = if kind == 6 { 4 } else { 8 };
        let mut e = self.emitters[id];
        e.counter = e.counter.wrapping_add(1);
        if e.cell.is_none() {
            let cell = self.grid.cell_of(ground);
            e.cell = Some(cell);
            e.kind = kind;
            e.code = code;
            e.strip = self.alloc(cell, code, id);
            e.state = 1;
        } else if e.strip.is_some() && e.state != 2 {
            if kind != e.kind {
                e.kind = kind;
                e.code = code;
                e.state = 3;
            }
        } else {
            e.kind = kind;
            e.code = code;
            e.strip = e.cell.and_then(|c| self.alloc(c, code, id));
            e.state = 1;
        }
        self.emitters[id] = e;
        self.append(id, ground, dir, strength);
        self.emitters[id].state = 0;
    }

    /// 0x479190.
    fn append(&mut self, id: usize, p: Vec3, dir: Vec3, strength: f32) {
        let e = self.emitters[id];
        // 0x479625…0x47965D: the cell's sphere `y` (`+0x28`, 0 from the constructor) takes the
        // height of the point while it is still 0 (`0.001 > y > −0.001`)
        if let Some(c) = e.cell.and_then(|c| self.grid.cells.get_mut(c as usize))
            && c.cy < 0.001
            && c.cy > -0.001
        {
            c.cy = p.y;
        }
        let cap = self.capacity;
        let skip_commit = e.counter & COMMIT_MASK != 0;
        let w = e.half;
        let p0 = p + Vec3::new(-dir.z, 0.0, dir.x) * w;
        let p1 = p + Vec3::new(dir.z, 0.0, -dir.x) * w;
        let colour = surface_colour(e.kind, strength);
        let mut prev = None;
        let mut need_new = false;
        // states 0 and 3: the strip's last pair, and on a change of kind a closing pair in the
        // old colour with its top alpha bit cleared (0x479256…0x479392)
        if (e.state == 0 || e.state == 3)
            && let Some(s) = e.strip
            && self.strip(s).owner == Some(id)
            && self.strip(s).points.len() >= 2
        {
            let n = self.strip(s).points.len();
            prev = Some((self.strip(s).points[n - 2], self.strip(s).points[n - 1]));
            if e.state == 3 && cap.saturating_sub(n) >= 2 {
                let c = self.strip(s).colours[n - 2] & 0x7FFF_FFFF;
                self.push(s, p0, p1, c);
            }
        }
        // 0x4793AE…0x479452: a wheel that left its cell moves to the one it is in
        let here = self.grid.cell_of(p);
        let mut cell = e.cell.unwrap_or(here);
        if cell != here {
            cell = here;
            self.emitters[id].cell = Some(here);
            need_new = true;
        }
        // 0x479452…0x4794AC
        if e.state == 3 {
            need_new = true;
        }
        match e.strip {
            Some(s) if self.strip(s).state != 1 && self.strip(s).owner == Some(id) => {}
            _ => need_new = true,
        }
        let full = e.strip.is_none_or(|s| cap.saturating_sub(self.strip(s).points.len()) < 2);
        let mut strip = e.strip;
        if full || need_new {
            strip = self.alloc(cell, e.code, id);
            self.emitters[id].strip = strip;
            if let (Some(s), Some((a0, a1))) = (strip, prev) {
                let c = if e.state == 3 { colour & 0x7FFF_FFFF } else { colour };
                self.push(s, a0, a1, c);
            }
        }
        let Some(s) = strip else { return };
        // 0x479660…0x479769: between commits the head pair is laid again, not added
        if skip_commit && self.strip(s).points.len() >= 4 {
            let n = self.strip(s).points.len();
            let st = self.strip_mut(s);
            st.points.truncate(n - 2);
            st.colours.truncate(n - 2);
        }
        let c = if self.strip(s).points.is_empty() && e.state != 3 { colour & 0x00FF_FFFF } else { colour };
        self.push(s, p0, p1, c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_alpha_scales() {
        // tarmac: black, alpha 255; at full strength k = 255 gives (255·255) >> 8 = 254
        assert_eq!(surface_colour(1, 1.0), 0xFE00_0000);
        // dirt keeps its colour
        assert_eq!(surface_colour(3, 0.5) & 0x00FF_FFFF, 0x0078_5C27);
        // the 32-bit product wraps past k = 256
        assert_eq!(surface_colour(1, 2.0) >> 24, ((255 * 510) >> 8) & 0xFF);
    }

    #[test]
    fn a_mark_fades_in_and_commits_every_eighth_call() {
        let mut m = SkidMarks::new();
        m.set_width(0, half_width(245.0, true));
        for i in 0..17 {
            m.emit(0, Vec3::new(0.0, 0.0, -(i as f32)), 1.0, 1, Vec3::new(0.0, 0.0, -1.0), 0.5, 2.0);
        }
        let s = m.strips().next().expect("a strip");
        // the opening pair is invisible, the rest are the surface colour
        assert_eq!(s.colours[0] >> 24, 0);
        assert!(s.colours[2] >> 24 > 0);
        // 17 calls: the opening pair, commits at calls 8 and 16, and the head
        assert_eq!(s.points.len(), 2 * 4);
        // the pair straddles the wheel across its direction of travel
        assert!((s.points[0].x - 0.18375).abs() < 1e-4 && (s.points[1].x + 0.18375).abs() < 1e-4);
    }

    #[test]
    fn an_unordered_strength_stops_the_emitter() {
        // 0x478F4A `test ah, 0x41`: C0|C3, both set by a NaN compare
        let mut m = SkidMarks::new();
        m.set_width(0, 0.1);
        m.emit(0, Vec3::ZERO, 1.0, 1, Vec3::Z, 0.5, 2.0);
        m.emit(0, Vec3::ZERO, f32::NAN, 1, Vec3::Z, 0.5, 2.0);
        assert_eq!((m.emitters[0].state, m.emitters[0].counter), (2, 1));
    }

    #[test]
    fn below_the_threshold_it_stops_and_restarts_on_a_new_strip() {
        let mut m = SkidMarks::new();
        m.set_width(0, 0.1);
        for i in 0..4 {
            m.emit(0, Vec3::new(0.0, 0.0, i as f32), 1.0, 1, Vec3::Z, 0.5, 2.0);
        }
        m.emit(0, Vec3::ZERO, 0.2, 1, Vec3::Z, 0.5, 2.0);
        for i in 0..4 {
            m.emit(0, Vec3::new(5.0, 0.0, i as f32), 1.0, 1, Vec3::Z, 0.5, 2.0);
        }
        assert_eq!(m.strips().count(), 2);
    }

    #[test]
    fn the_root_box_and_chosen_level_follow_the_world_box() {
        // a 5 km world: the chosen level is 5 (5000/2^5 = 156.25 >= 100, /2^6 = 78.125 < 100), 12
        // points a strip; the root is the 5000 m square centred on the box
        let m = SkidMarks::for_world(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5000.0, 50.0, 3000.0));
        assert_eq!((m.grid.chosen_level, m.capacity), (5, 12));
        assert_eq!((m.grid.tree.min[0], m.grid.tree.min[2]), (0.0, -1000.0));
        assert_eq!((m.grid.tree.min[0] + m.grid.tree.size, m.grid.tree.min[2] + m.grid.tree.size), (5000.0, 4000.0));
        // a small one keeps 192
        assert_eq!(SkidMarks::for_world(Vec3::ZERO, Vec3::new(800.0, 0.0, 600.0)).capacity, 192);
    }

    #[test]
    fn crossing_a_cell_opens_a_new_strip() {
        // no items are registered here, so the tree only ever carries its two mandatory levels
        // (`TreeNode::build`'s `level <= 1` rule fires regardless of item count) — the coarsest
        // possible case `particles.md` §10.1 describes: every real cell is a level-2 quadrant,
        // `L/4` = 1250 m, boundaries at multiples of 1250 from the root's corner (0, 0)
        let mut m = SkidMarks::for_world(Vec3::ZERO, Vec3::new(5000.0, 0.0, 5000.0));
        m.set_width(0, 0.1);
        for i in 0..40 {
            m.emit(0, Vec3::new(1230.0 + i as f32, 0.0, 10.0), 1.0, 1, Vec3::X, 0.5, 2.0);
        }
        // x from 1230 to 1269 crosses 1250: two strips, the second seeded with the first's head
        assert_eq!(m.rings.len(), 2);
    }

    /// A dense track's item population reaches the chosen level's own fine cells (`0x479810`
    /// descends into a node only while it has all four children): with every quadrant well past
    /// the 32-item split threshold down to the chosen level, the cell boundary is `L/2^level`
    /// exactly like the port's old fixed grid — `particles.md` §10.1's "or" clause never fires
    /// when there is no locally sparse quadrant.
    #[test]
    fn a_dense_population_reaches_the_chosen_level_uncoarsened() {
        // a uniform 128 x 128 grid over the whole 1600 m box (12.5 m spacing, dividing every
        // quadrant boundary down to level 4 exactly): every quadrant at every level holds a large
        // exact share of the total, never falling under 32
        let mut items = Vec::new();
        for gx in 0..128 {
            for gz in 0..128 {
                items.push((gx as f32 * 12.5, gz as f32 * 12.5));
            }
        }
        let m = SkidMarks::for_world_items(&items, Vec3::ZERO, Vec3::new(1600.0, 0.0, 1600.0));
        // chosen level for a 1600 m world: 1600/2^4 = 100 >= 100, /2^5 = 50 < 100 -> level 4
        assert_eq!(m.grid.chosen_level, 4);
        // the level-4 cell size is 1600 / 16 = 100 m; two points either side of the x = 100
        // boundary land in different cells at that level (the tree is fully split that far —
        // every quadrant here holds hundreds of items)
        let a = m.grid.cell_of(Vec3::new(50.0, 0.0, 50.0));
        let b = m.grid.cell_of(Vec3::new(150.0, 0.0, 50.0));
        assert_ne!(a, b, "a dense grid does not coarsen the chosen level's own cell size");
    }
}
