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
//! Strips live **per cell** of the world's quadtree (0x47B660 builds it: the root is the square
//! `L = max(Δx, Δz)` of the world's box, centred on it, halved level by level; the marks use the
//! smallest level whose cell is still 100 or more, 0x479972…0x4799D0), 20 to a cell, and a wheel
//! that leaves its cell opens a new strip in the next (0x4793AE…0x479452). A strip holds 192
//! points on a world under 1200 across, else 12 (0x47993F…0x47995C).
//!
//! The world's box is the box of the port's collision triangles; the original's is its bundles'
//! boxes (0x47B6C6…0x47B89C). Checked 2026-09-24: the box of every drawn article and instance
//! comes out the same (skidpad 750 m, monaco1 1818 m across), so the level and the 192 / 12
//! capacity agree. APPROXIMATION left: the original finds a cell's record with a
//! `lower_bound` over the cells that hold geometry (0x47A300), the port keys a cell by its index.

use crate::particles::SURFACE_ARGB;
use glam::Vec3;

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
    cell: Option<(i32, i32)>,
    /// `+0x0C`: the kind it was last given
    kind: usize,
    /// `+0x10`: the strip state it asks for, 4 for kind 6 and 8 for the rest
    code: u8,
    /// `+0x14`: calls above the threshold
    counter: u32,
    /// `+0x20`: 0 running, 1 a new strip, 2 stopped, 3 the kind changed
    state: u8,
}

/// The marks' cells: the quadtree's root corner and the chosen level's cell size.
#[derive(Debug, Clone, Copy)]
struct Grid {
    x0: f32,
    z0: f32,
    cell: f32,
}

impl Grid {
    fn cell_of(&self, p: Vec3) -> (i32, i32) {
        (((p.x - self.x0) / self.cell).floor() as i32, ((p.z - self.z0) / self.cell).floor() as i32)
    }
}

/// One cell's record (0x50 bytes in the original): its strips, `+0x3C` the strips set free and not
/// yet handed out, `+0x4C` the ring's cursor.
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

type StripRef = ((i32, i32), usize);

#[derive(Debug, Clone)]
pub struct SkidMarks {
    grid: Grid,
    capacity: usize,
    rings: std::collections::HashMap<(i32, i32), Ring>,
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
    /// A world of no size: one endless cell, 192 points a strip.
    pub fn new() -> Self {
        Self::for_world(Vec3::ZERO, Vec3::ZERO)
    }

    /// 0x479900 over the world's box: the quadtree's root square and level (0x47B8A2…0x47B94F,
    /// 0x479972…0x4799D0 — the last level whose cell is still `[0x5B242C]` 100 or more, at most
    /// the 13 the tree keeps) and the strips' capacity (0x47993F…0x47995C).
    pub fn for_world(min: Vec3, max: Vec3) -> Self {
        let (dx, dz) = (max.x - min.x, max.z - min.z);
        let l = dx.max(dz);
        let mut level: u32 = 0;
        while level < 13 && l / (1u32 << level) as f32 >= 100.0 {
            level += 1;
        }
        let level = level.saturating_sub(1);
        // a world of no size (no track yet) is one cell
        let cell = if l > 0.0 { l / (1u32 << level) as f32 } else { f32::INFINITY };
        let grid = Grid { x0: min.x - (l - dx) * 0.5, z0: min.z - (l - dz) * 0.5, cell };
        let capacity = if l < 1200.0 { MAX_CAPACITY } else { 12 };
        Self { grid, capacity, rings: Default::default(), emitters: Vec::new() }
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
    fn alloc(&mut self, cell: (i32, i32), code: u8, owner: usize) -> Option<StripRef> {
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
        if strength <= a {
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
    fn the_grid_follows_the_world_box() {
        // a 5 km world: cells of 5000 / 2^5 = 156.25 (the next level would be 78), 12 points a strip
        let m = SkidMarks::for_world(Vec3::new(0.0, 0.0, 0.0), Vec3::new(5000.0, 50.0, 3000.0));
        assert_eq!((m.grid.cell, m.capacity), (156.25, 12));
        assert_eq!(m.grid.z0, -1000.0);
        // a small one keeps 192
        assert_eq!(SkidMarks::for_world(Vec3::ZERO, Vec3::new(800.0, 0.0, 600.0)).capacity, 192);
    }

    #[test]
    fn crossing_a_cell_opens_a_new_strip() {
        let mut m = SkidMarks::for_world(Vec3::ZERO, Vec3::new(5000.0, 0.0, 5000.0));
        m.set_width(0, 0.1);
        for i in 0..40 {
            m.emit(0, Vec3::new(150.0 + i as f32, 0.0, 10.0), 1.0, 1, Vec3::X, 0.5, 2.0);
        }
        // x from 150 to 189 crosses 156.25: two strips, the second seeded with the first's head
        assert_eq!(m.rings.len(), 2);
    }
}
