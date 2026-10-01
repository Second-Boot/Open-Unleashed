//! Track slices, progress, laps, wrong-way and start grid — from `RE/notes/race-logic.md`
//! (CONFIRMED unless noted). Slices are 80-byte records in the track CRP `SimD` level entry,
//! 6.0 m apart along the road; `SimT` gives the slice count per section (main lap first).

use glam::{Mat3, Vec3};
use nfs_formats::crp::{Crp, CrpEntry};
use nfs_formats::scn::{ElementKind, Scene};

pub const SLICE_SPACING: f32 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slice {
    pub centre: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
    /// up × forward — the car's right-hand side ("A" side)
    pub lateral: Vec3,
    pub lane_mask: u32,
    pub lanes_a: u32,
    pub lanes_b: u32,
    pub width_a: f32,
    pub width_b: f32,
    pub lane_width_a: f32,
    pub lane_width_b: f32,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub first: usize,
    pub last: usize,
    /// entry / exit lap fractions
    pub f0: f32,
    pub f1: f32,
}

#[derive(Debug, Clone)]
pub struct SliceTable {
    pub slices: Vec<Slice>,
    pub sections: Vec<Section>,
    /// Slice indices of the main lap in driving order: section 0 followed by the sections its exit
    /// junction leads to (monaco1: 0 → 2 → 1 → 3; point-to-point tracks: section 0 only). The
    /// original builds the same chain from the junction records (0x480900 / 0x480B40, MEDIUM);
    /// here a section's exit is linked to the section start within `JUNCTION_RADIUS` of it.
    pub route: Vec<usize>,
    /// slice → position in `route`; branch slices map to the nearest lap slice.
    pub route_pos: Vec<u32>,
    /// slices of the main lap (= `route.len()`)
    pub lap_len: usize,
    /// the derived junction graph (`race-logic.md` §10): segments, nodes and the branch weights
    pub junctions: crate::junctions::Junctions,
    /// true when this table is the reversed view of the track, so a step along the lap route is a step
    /// **backwards** through the junction graph
    pub backwards: bool,
    /// `LINEAR_TRACK [0x657494]`: a point-to-point track. Only [`Self::frac`]'s backwards arm reads it
    /// (0x4162E4: the `1.0 → 0.0` wrap is for a circuit). The caller sets it after `from_crp`.
    pub linear: bool,
}

/// Maximum gap between a section's last slice (advanced one spacing) and the next section's first.
pub const JUNCTION_RADIUS: f32 = 20.0;

/// `[0x628BB8] − (90 − 6)²` = 8100 − 7056 (0x4811CF). `0x481190` keeps the catchment slices whose
/// squared distance from the junction is within this much of the car's own — a ring around the node
/// at the car's own radius, which is what separates one branch from another.
pub const BRANCH_WINDOW: f32 = 1044.0;

impl SliceTable {
    pub fn from_crp(crp: &Crp, data: &[u8]) -> Result<Self, String> {
        let simd = crp.level.iter().find(|e| e.is_level(b"SimD")).ok_or("Could not find sim data in data group (no SimD)")?;
        let bytes = crp.entry_data(data, simd).map_err(|e| e.to_string())?;
        let n = match simd {
            CrpEntry::Level { b, .. } => *b as usize,
            _ => 0,
        };
        if bytes.len() < n * 80 {
            return Err(format!("SimD: {} bytes for {n} slices", bytes.len()));
        }
        let f = |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let u = |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        let v3 = |o: usize| Vec3::new(f(o), f(o + 4), f(o + 8));
        let mut slices = Vec::with_capacity(n);
        for i in 0..n {
            let b = i * 80;
            slices.push(Slice {
                centre: v3(b + 4),
                up: v3(b + 0x10),
                forward: v3(b + 0x1C),
                lateral: v3(b + 0x28),
                lane_mask: u(b + 0x34),
                lanes_a: u(b + 0x38),
                lanes_b: u(b + 0x3C),
                width_a: f(b + 0x40),
                width_b: f(b + 0x44),
                lane_width_a: f(b + 0x48),
                lane_width_b: f(b + 0x4C),
            });
        }
        let counts: Vec<usize> = match crp.level.iter().find(|e| e.is_level(b"SimT")) {
            Some(e) => {
                let d = crp.entry_data(data, e).map_err(|x| x.to_string())?;
                d.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize).collect()
            }
            None => vec![n],
        };
        let mut sections = Vec::new();
        let mut first = 0;
        for c in counts {
            if c == 0 {
                continue;
            }
            sections.push(Section { first, last: first + c - 1, f0: 0.0, f1: 1.0 });
            first += c;
        }
        if sections.is_empty() {
            sections.push(Section { first: 0, last: n.max(1) - 1, f0: 0.0, f1: 1.0 });
        }
        Ok(Self::build(slices, sections))
    }

    /// Build the junction graph and lay the main lap out along it (`race-logic.md` §10).
    ///
    /// The original derives the graph from the slice geometry at load: a segment is cut wherever another
    /// segment's end passes within 16 m of it in plan view, the ends that share a 16 m circle become
    /// junction nodes, and the **main lap is the longest forward-link walk out of node 0** — not
    /// "section 0". The node count this produces matches every shipped `<track>.jnc` exactly, which is
    /// the check the original itself applies before it will load that file's branch weights.
    pub fn build(slices: Vec<Slice>, mut sections: Vec<Section>) -> Self {
        let centres: Vec<Vec3> = slices.iter().map(|s| s.centre).collect();
        let ranges: Vec<(usize, usize)> = sections.iter().map(|s| (s.first, s.last)).collect();
        let junctions = crate::junctions::Junctions::build(&centres, &ranges);
        let chain: Vec<(usize, usize)> = junctions.lap_route.iter().map(|&i| (junctions.segments[i].first, junctions.segments[i].last)).collect();
        let route: Vec<usize> = chain.iter().flat_map(|&(a, b)| a..=b).collect();
        let lap_len = route.len().max(1);
        let mut route_pos = vec![u32::MAX; slices.len()];
        for (p, &s) in route.iter().enumerate() {
            route_pos[s] = p as u32;
        }
        // lap fractions per section, from where its slices sit on the lap; branch sections keep 0..1
        for sec in sections.iter_mut() {
            let (a, b) = (route_pos[sec.first], route_pos[sec.last]);
            if a != u32::MAX && b != u32::MAX && a <= b {
                sec.f0 = a as f32 / lap_len as f32;
                sec.f1 = (b + 1) as f32 / lap_len as f32;
            }
        }
        for s in 0..slices.len() {
            if route_pos[s] == u32::MAX {
                let c = slices[s].centre;
                let mut best = (f32::MAX, 0u32);
                for (p, &r) in route.iter().enumerate() {
                    let d = slices[r].centre - c;
                    let d2 = d.x * d.x + d.z * d.z;
                    if d2 < best.0 {
                        best = (d2, p as u32);
                    }
                }
                route_pos[s] = best.1;
            }
        }
        Self { slices, sections, route, route_pos, lap_len, junctions, backwards: false, linear: false }
    }

    /// The same track driven backwards: the lap route runs the other way and every slice's
    /// forward / lateral axes are flipped (widths and lane counts swap sides). Geometry only — the
    /// original's backwards mode (`[0x657430]`, direction multiplier `[0x5E753C]`) is covered in
    /// `RE/notes/modes.md`.
    pub fn reversed(&self) -> SliceTable {
        let mut t = self.clone();
        for sl in &mut t.slices {
            sl.forward = -sl.forward;
            sl.lateral = -sl.lateral;
            std::mem::swap(&mut sl.width_a, &mut sl.width_b);
            std::mem::swap(&mut sl.lane_width_a, &mut sl.lane_width_b);
            std::mem::swap(&mut sl.lanes_a, &mut sl.lanes_b);
        }
        t.route.reverse();
        let n = t.route.len();
        for p in t.route_pos.iter_mut() {
            *p = (n as u32 - 1).saturating_sub(*p);
        }
        for (p, &s) in t.route.iter().enumerate() {
            t.route_pos[s] = p as u32;
        }
        // the junction graph is geometric, so it survives the flip unchanged — but walking it now runs
        // the other way, which `add_graph`'s callers express by asking for a negative delta. The lap
        // route above is what carries the reversal; `backwards` records it for them.
        t.backwards = !t.backwards;
        t
    }

    fn d2(&self, i: usize, p: Vec3) -> f32 {
        let c = self.slices[i].centre;
        let dx = c.x - p.x;
        let dz = c.z - p.z;
        dx * dx + dz * dz
    }

    /// `SliceTable::findSlice` 0x481400: hill-climb in the xz plane from `prev`, with a coarse global
    /// scan over every 8th slice when farther than 128 m. Restricted to the main lap (junction branch
    /// picking not implemented — MEDIUM).
    /// `0x4816C0` — the slice of one section nearest `pos` (2-D, as the tracker measures), **hill-
    /// climbed from `from`**, not scanned.
    ///
    /// 0x4816C0 is a thin wrapper: it reads the section record's first (`+0`) and last (`+0x10`)
    /// slice and calls **0x4812C0(first, last, pos, from)**, which
    ///
    /// * keeps `from` as the seed when `first <= from <= last` (0x4812D4…0x4812DA), and otherwise
    ///   seeds at whichever **end** of the section is nearer to `pos` (0x4812DC…0x481335);
    /// * then walks ±1 while a neighbour is nearer, **bounded by the section**: the backward step
    ///   stops at `first` (`cmp edx, [esp+0x14]; jle` at 0x4813A1) and the loop ends on reaching
    ///   `last` (`cmp [esp+0x20], edx; jne` at 0x4813D4).
    ///
    /// *This used to be a linear scan of the whole section, which let a car snap to any slice of
    /// it — on monaco4 an opponent jumped 82 -> 463 where the route doubles back near itself
    /// (2026-09-23).*
    /// **The "section" here is a junction graph segment, not a `SimT` section.** `SliceTable::build`
    /// 0x480140 seeds `this+0x18` with the `SimT` list and then **splits it at every crossing**
    /// (`splitSegmentsAtCrossings` 0x480900, `race-logic.md` §10.1), and it is that split list
    /// 0x481C80 indexes and `car+0xC` (`preferredSection`, the chosen link's `link+4`) points into.
    /// This port kept the unsplit `SimT` sections here until 2026-09-23, so a preferred **segment**
    /// index selected an unrelated slice range: on industrial (23 sections, 29 segments) that pinned
    /// cars 120 m from their own slice.
    pub fn nearest_in_section(&self, section: usize, pos: Vec3, from: usize) -> Option<usize> {
        let s = self.junctions.segments.get(section)?;
        let n = self.slices.len();
        if n == 0 {
            return None;
        }
        let (first, last) = (s.first.min(n - 1), s.last.min(n - 1));
        if first > last {
            return None;
        }
        let mut p = if (first..=last).contains(&from) {
            from
        } else if self.d2(first, pos) <= self.d2(last, pos) {
            first
        } else {
            last
        };
        // each step strictly shortens the distance, so this ends; the bound is only a guard
        for _ in 0..=(last - first) {
            let d = self.d2(p, pos);
            if p < last && self.d2(p + 1, pos) < d {
                p += 1;
                continue;
            }
            if p > first && self.d2(p - 1, pos) < d {
                p -= 1;
                continue;
            }
            break;
        }
        Some(p)
    }

    /// `0x481C80` — the section a slice belongs to.
    /// 0x481C80 — the **segment** a slice is in, over the split list (see [`Self::nearest_in_section`]).
    fn section_of(&self, slice: usize) -> Option<usize> {
        self.junctions.segments.iter().position(|s| (s.first..=s.last).contains(&slice))
    }

    /// `Tracker::update` 0x474440 with a `preferredSection`: when the car has chosen a branch, its
    /// slice is resolved against that section as well, and the nearer of the two candidates wins.
    /// Without a preference (or on a single-section table) this is plain [`Self::find_slice`].
    pub fn track_slice(&self, pos: Vec3, prev: usize, preferred: Option<usize>) -> usize {
        // 0x481C60's count is the same split list (`(end − begin) / 20` over `this+0x18`)
        let Some(sec) = preferred.filter(|_| self.junctions.segments.len() > 1) else { return self.find_slice(pos, prev) };
        // 0x474444…0x4744A6: **both** candidates are 0x4816C0 calls seeded at the car's current
        // slice — one for the section that slice is in (0x481C80), one for the preferred section —
        // and the nearer centre wins. The plain `findSlice` runs only when there is no preference
        // or the table has one section (0x474460 / 0x474474).
        let here = self.section_of(prev.min(self.slices.len().saturating_sub(1)));
        let a = here.and_then(|h| self.nearest_in_section(h, pos, prev));
        let b = self.nearest_in_section(sec, pos, prev);
        match (a, b) {
            (Some(a), Some(b)) => {
                if self.d2(b, pos) < self.d2(a, pos) {
                    b
                } else {
                    a
                }
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => self.find_slice(pos, prev),
        }
    }

    pub fn find_slice(&self, pos: Vec3, prev: usize) -> usize {
        let n = self.slices.len();
        if n == 0 {
            return prev;
        }
        let d2 = |i: usize| self.d2(i % n, pos);
        let mut p = prev.min(n - 1);
        // farther than 128 m from where it was: a coarse sweep of every eighth slice (0x481468)
        if d2(p) > 16384.0 {
            let mut best = (f32::MAX, p);
            let mut i = 0;
            while i < n {
                let d = d2(i);
                if d < best.0 {
                    best = (d, i);
                }
                i += 8;
            }
            p = best.1;
        }
        // 0x4814A5: a slice inside a junction's catchment re-seeds the search onto whichever branch
        // of that junction the car is actually on, which is what lets the tracker follow a shortcut
        if let Some(node) = self.junctions.near_node.get(p).copied().flatten() {
            p = self.resolve_branch(pos, node, p);
        }
        // the hill-climb, over the whole slice array and wrapping at its end (0x4814C5)
        for _ in 0..1000 {
            let d = d2(p);
            let next = (p + 1) % n;
            let prev_i = (p + n - 1) % n;
            let dn = d2(next);
            let dp = d2(prev_i);
            if dn < d && dn <= dp {
                p = next;
            } else if dp < d {
                p = prev_i;
            } else {
                break;
            }
        }
        p
    }

    /// `0x481190` — which slice of a junction's catchment the car is on.
    ///
    /// The node's list is scanned for the entries whose distance from the node is within
    /// [`BRANCH_WINDOW`] of the car's own, and the nearest of those wins. The scan is the
    /// original's: it walks forward to the first entry inside the window and stops at the first one
    /// past it, which matters because the list is a concatenation of per-link runs that each walk
    /// away from the node. Two quirks are kept: running off the end returns the last entry, and a
    /// window that admits nothing returns slice 0.
    fn resolve_branch(&self, pos: Vec3, node: usize, prev: usize) -> usize {
        let Some(list) = self.junctions.node_slices.get(node) else { return prev };
        if list.is_empty() {
            return prev;
        }
        let Some(n) = self.junctions.nodes.get(node) else { return prev };
        let dx = n.position.x - pos.x;
        let dz = n.position.z - pos.z;
        let d2c = dx * dx + dz * dz;
        let lo = (d2c - BRANCH_WINDOW).max(0.0);
        let hi = d2c + BRANCH_WINDOW;
        let mut i = 0;
        while i < list.len() && list[i].1 < lo {
            i += 1;
        }
        if i == list.len() {
            return list[list.len() - 1].0;
        }
        let mut best = (f32::MAX, 0usize);
        while i < list.len() && list[i].1 < hi {
            let d = self.d2(list[i].0, pos);
            if d < best.0 {
                best = (d, list[i].0);
            }
            i += 1;
        }
        best.1
    }

    /// Lap fraction of a slice (0x416290); branch slices use their nearest lap slice.
    ///
    /// Forward: `p / N` on the route position. **Backwards** the original never reverses its table:
    /// it takes the raw fraction `raw / N` and returns `1 − raw / N` (0x4162E4…0x4162E9, `[0x657430]`),
    /// and on a circuit (`[0x657494] == 0`) a result of exactly 1.0 becomes 0.0 (0x4162F3…0x416302).
    /// This table's route runs the other way (`p = N − 1 − raw`), so the fraction is rebuilt from the
    /// raw slice — `(N − 1 − p) / N` (the earlier reading) sat one slice, 6 m, from the original's and
    /// put the lap line between the wrong pair of slices (`race-logic.md` §1.5a).
    pub fn frac(&self, slice: usize) -> f32 {
        let p = self.route_pos.get(slice).copied().unwrap_or(0);
        if self.backwards {
            let raw = (self.lap_len as u32).saturating_sub(1).saturating_sub(p);
            if raw == 0 && !self.linear {
                return 0.0;
            }
            return 1.0 - raw as f32 / self.lap_len as f32;
        }
        p as f32 / self.lap_len as f32
    }

    /// Slice arithmetic along the main lap (wrapping), following the section chain (0x4817D0 for
    /// the lap's own junctions; branch links are not followed).
    pub fn add(&self, slice: usize, delta: i32) -> usize {
        if self.junctions.segments.is_empty() {
            let n = self.lap_len as i32;
            let p = self.route_pos.get(slice).copied().unwrap_or(0) as i32;
            return self.route[(p + delta).rem_euclid(n) as usize];
        }
        self.add_graph(slice, delta, None)
    }

    /// Load `<track>.jnc`'s branch weights. The original rejects the file unless its size is exactly
    /// `36 × nNodes`; so does this.
    pub fn apply_jnc(&mut self, data: &[u8]) -> bool {
        self.junctions.apply_jnc(data)
    }

    /// `junctionWithin` **0x4816F0** in the table's own frame: a positive `delta` always means "ahead
    /// along the lap", whichever way this table runs.
    pub fn junction_within(&self, slice: usize, delta: i32) -> Option<usize> {
        let d = if self.backwards { -delta } else { delta };
        self.junctions.junction_within(slice, d)
    }

    /// The link direction that means "onward along the lap" in this table's frame (`link+0xC`).
    pub fn graph_want(&self) -> usize {
        if self.backwards { 1 } else { 0 }
    }

    /// `addSlices` **0x4817D0** (`race-logic.md` §10, `traffic.md` §23.0) — slice arithmetic along the
    /// real junction graph rather than along the precomputed lap route.
    ///
    /// Walking off the end of a segment crosses that segment's junction node and continues into one of
    /// its links: the caller's `link` when it has chosen a branch, otherwise the **last** link whose
    /// direction matches the way we are walking. A dead end clamps, which is what makes a
    /// point-to-point track stop at its ends instead of wrapping.
    pub fn add_graph(&self, slice: usize, delta: i32, mut link: Option<usize>) -> usize {
        // on a reversed table a positive step along the lap is a negative step through the graph
        let delta = if self.backwards { -delta } else { delta };
        let j = &self.junctions;
        let Some(mut seg) = j.segments.iter().position(|s| slice >= s.first && slice <= s.last) else {
            return slice;
        };
        let mut s = slice as i32 + delta;
        // the original loops until the position lands inside a segment; bound it so a cyclic graph and a
        // huge delta cannot spin for ever
        for _ in 0..64 {
            let (first, last) = (j.segments[seg].first as i32, j.segments[seg].last as i32);
            if s >= first && s <= last {
                return (s.max(0) as usize).min(self.slices.len().saturating_sub(1));
            }
            let edge = if s > last { last } else { first } as usize;
            let Some(node) = j.slice_node.get(edge).copied().flatten() else { return edge };
            let rest = s - edge as i32 - 1;
            let want = if delta <= 0 { 1 } else { 0 };
            let k = match link.take() {
                Some(k) if k < j.nodes[node].links.len() => k,
                _ => {
                    // the original's loop does not break, so the LAST matching link wins
                    match j.nodes[node].links.iter().rposition(|l| l.dir() == want) {
                        Some(k) => k,
                        None => return slice,
                    }
                }
            };
            let l = j.nodes[node].links[k];
            s = j.segments[l.segment].end_slice(l.end) as i32 + rest;
            seg = l.segment;
        }
        (s.max(0) as usize).min(self.slices.len().saturating_sub(1))
    }
}

/// What 0x4A7660's gates let the wrong-way counter `car+0xD30` do this step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrongWayGate {
    /// counts while the car heads against the slice
    #[default]
    Count,
    /// holds its value — `word car+0x418` (more than one wheel airborne, suspension-collision.md
    /// §1.3) or `car+0x1104` set
    Hold,
    /// cleared — finished, AI-driven, post-finish or a mission
    Reset,
}

/// 0x4A7734…0x4A7744: the "wrong way" cue 0x4AAE70 sounds when `car+0xD30` reaches 64, 96 and 128
/// (`≥ 0x40`, a multiple of 32, `< 0x94`).
pub fn wrong_way_cue(steps: u32) -> bool {
    steps >= 0x40 && steps & 0x1F == 0 && steps < 0x94
}

/// Per-car race bookkeeping (`car+0x530..`).
#[derive(Debug, Clone, Default)]
pub struct RaceProgress {
    pub slice: usize,
    pub laps: u32,
    pub unlaps: u32,
    pub progress: f32,
    pub prev_progress: f32,
    pub lap_start_tick: u64,
    pub lap_times: Vec<u64>,
    pub wrong_way_ticks: u32,
    /// `car+0xD30` as the original counts it: 0x4A7660 runs in the odd-tick group 0x4A8120, so one
    /// step per two 64 Hz ticks while the car heads against the slice.
    pub wrong_way_steps: u32,
    /// what 0x4A7660's gates allow this step — set by the caller before `update`
    pub wrong_way_gate: WrongWayGate,
    pub finished: bool,
    /// Lap fraction the car started from (needed for the projected time of a car that is still
    /// racing when the race ends, `Race_finalize` 0x416AC0).
    pub start_progress: f32,
    /// A point-to-point track's bounds (`car+0xB04`, `car+0xB08`, 0x410979…0x410A0A): the finish
    /// fraction and the fraction below which the car is sent back to its start. `None` on a circuit,
    /// which counts laps by crossing the line instead.
    pub linear: Option<(f32, f32)>,
    /// Set by [`Self::update`] when a car on a point-to-point track falls below its start fraction:
    /// 0x4174A2…0x4174B8 then calls 0x40F540(car, 3, 0) — the caller's to act on.
    pub behind_start: bool,
    /// `car+0xAFC`, the slice the car started on (0x410973) — where 0x40F540's mode 3 puts it back.
    pub start_slice: usize,
    /// `car+0xB14`: the tick the car's slice last changed while it was racing (0x4164E0…, in the
    /// `car+8 != car+0xB18` block of 0x416310), the tick it finished (0x416721) or was knocked
    /// out (0x416649…0x4166FD). The ranking table breaks a progress tie on it ([`ranking`]).
    pub tick: u64,
}

impl RaceProgress {
    pub fn new(start_slice: usize, table: &SliceTable) -> Self {
        let p = table.frac(start_slice);
        Self { slice: start_slice, progress: p, prev_progress: p, start_progress: p, start_slice, ..Default::default() }
    }

    /// `Car_raceInit` 0x410979…0x410A0A on a point-to-point track (`LINEAR_TRACK [0x657494]`): the
    /// start fraction `car+0xB00` is the car's own start slice's, the reset bound `car+0xB08` is
    /// that less `[0x5B2738]` 0.005, and the finish `car+0xB04` is the fraction of the slice of
    /// the scene's type-1 trigger (0x46D290(1, 0)) — or, with none, the 0.98 0x41076F set first.
    pub fn set_linear(&mut self, table: &SliceTable, finish_slice: Option<usize>) {
        let start = table.frac(self.slice);
        let finish = finish_slice.map_or(0.98, |s| table.frac(s));
        self.linear = Some((finish, start - 0.005));
    }

    /// Update at 32 Hz (even ticks): tracker, `Lap_check` 0x417440, `Race_updateProgress` 0x416310,
    /// wrong-way 0x4A7660. Returns true when a lap was completed this call.
    pub fn update(&mut self, table: &SliceTable, pos: Vec3, forward: Vec3, tick: u64, num_laps: u32) -> bool {
        self.update_tracked(table, pos, forward, tick, num_laps, None)
    }

    /// A skill test's car (`[0x6573EC] == 4`): the per-car wrapper 0x4169B0 calls the checkpoint
    /// progression 0x416170 **instead of** `Race_updateProgress` 0x416310 (0x4169FE…0x416A14), so
    /// neither the progress, the laps (`Lap_check` 0x417440 is 0x416310's, 0x41657F; its other
    /// caller 0x4179A4 is for cars without `car+0x52C` bit 0) nor `car+0xB14` move — only the
    /// tracker's slice does. The finish and its tick are 0x416170's (the caller's).
    pub fn track_only(&mut self, table: &SliceTable, pos: Vec3, preferred: Option<usize>) {
        self.slice = table.track_slice(pos, self.slice, preferred);
    }

    /// The same with the tracker's `preferredSection` (`race-logic.md` §1.4): a car that has taken a
    /// junction branch resolves its slice against that branch's section too.
    pub fn update_tracked(&mut self, table: &SliceTable, pos: Vec3, forward: Vec3, tick: u64, num_laps: u32, preferred: Option<usize>) -> bool {
        let prev_slice = self.slice;
        self.slice = table.track_slice(pos, self.slice, preferred);
        // finished / eliminated cars leave the progress accounting (0x4169B0: `state >= 2 ⇒ return`),
        // so their lap list and rank freeze at the finish
        if self.finished {
            return false;
        }
        // `car+0xB14 = gGameTick` whenever the slice changed (0x4164C0…0x416577)
        if self.slice != prev_slice {
            self.tick = tick;
        }
        let frac = table.frac(self.slice);
        let new = self.laps as f32 + frac - self.unlaps as f32;
        let fn_ = frac;
        let fp = self.prev_progress.rem_euclid(1.0);
        let mut lap_done = false;
        if let Some((finish, below)) = self.linear {
            // `Lap_check` 0x417440 on a linear track (0x41748D…0x4174C4): a lap — the race, on a
            // point-to-point track — once the progress is past the finish fraction; below the
            // start bound the car goes back to its start. Nothing happens while it does not move.
            if new != self.prev_progress {
                if new > finish {
                    self.laps += 1;
                    self.lap_times.push(tick - self.lap_start_tick);
                    self.lap_start_tick = tick;
                    lap_done = true;
                    if self.laps >= num_laps {
                        self.finished = true;
                    }
                } else if new < below {
                    self.behind_start = true;
                }
            }
        } else if fn_ > 0.5 && fp < 0.1 && self.prev_progress != new {
            self.unlaps += 1;
        } else if fn_ < 0.1 && fp > 0.5 {
            if self.unlaps > 0 {
                self.unlaps -= 1;
            } else {
                self.laps += 1;
                self.lap_times.push(tick - self.lap_start_tick);
                self.lap_start_tick = tick;
                lap_done = true;
                if self.laps >= num_laps {
                    self.finished = true;
                }
            }
        }
        self.prev_progress = new;
        self.progress = self.laps as f32 + frac - self.unlaps as f32;
        if self.finished {
            // The finish, 0x416721…0x416838 (`Race_updateProgress` 0x416310, the tick the lap
            // counter reaches the race's laps): `car+0xB50 = 2`, `car+0xB10 = (laps timed + 1) ·
            // nSlicesLap`, and `car+0xAF8` is **rewritten** from the car's slice — the same
            // section-interpolated fraction 0x416290 gives — as the bare fraction on a
            // point-to-point track (0x4167B9, no lap added: `lap + frac` drops by 1.0) and as
            // `(float)car+0xB20 + frac` on a circuit (0x416830…0x416838, the laps just timed; no
            // `− unlaps`, which is 0 on the crossing that counted the lap, so the value is unchanged
            // there). From then on `car+0xB50 != 0` skips the recompute (0x416365), so this is what
            // the ranking table 0x608B38, `0x417710`'s two sorts and every other `+0xAF8` reader see.
            self.progress = if self.linear.is_some() { frac } else { self.lap_times.len() as f32 + frac };
            // `car+0xB14 = gGameTick` (0x416727)
            self.tick = tick;
        }
        // 0x4A7660: a car that has finished (`car+0xB50 == 2`), is AI-driven, is in the
        // post-finish state or races a mission (`[0x6573E8] == 9`) clears its counter, and so does
        // one heading along the slice; one heading against it counts on — unless it is airborne
        // (`word car+0x418`) or crash-inhibited (`car+0x1104`), when the counter holds
        // (0x4A76E5…0x4A774D)
        let gate = self.wrong_way_gate;
        if gate == WrongWayGate::Reset || forward.dot(table.slices[self.slice].forward) >= 0.0 {
            self.wrong_way_ticks = 0;
            self.wrong_way_steps = 0;
        } else if gate == WrongWayGate::Count {
            self.wrong_way_ticks += 1;
            if tick & 1 == 1 {
                self.wrong_way_steps += 1;
            }
        }
        lap_done
    }
}

/// **The ranking table `0x608B38`** — the race order, `(progress car+0xAF8, tick car+0xB14)` per
/// racer in the order the cars are updated; returns the racers' indices, leader first.
///
/// 0x416140 empties the table (index −1, progress −100.0) and every racer's `Race_updateProgress`
/// 0x416310 then inserts itself at 0x4168EF…0x4169A0: before the first entry whose progress it
/// **exceeds** (`fcomp` / `test ah, 0x41` / `je`, 0x41690B), or whose progress it **equals** with an
/// **earlier** tick (`test ah, 0x40`, then `cmp eax, ebx / jl`, 0x416923…0x41692E); otherwise after
/// it — so a full tie keeps the update order. That is a stable sort on (progress desc, tick asc).
/// `Race_finalize` 0x416AC0 gives every racer `car+0xCD4 = 1 + its index here` (0x416CB8…0x416CD2,
/// 0x416F04…0x416F1D), the place the results, the tournament and the knockouts read. A finished
/// car is ranked on the progress its finish froze (0x416721…0x416838, [`RaceProgress::update_tracked`])
/// — not on its time — and a knocked-out one on its progress less 0.01 (0x416649…0x4166FD), so both rank
/// by where they are, like everyone else.
pub fn ranking(cars: &[(f32, u64)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..cars.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, ta) = cars[a];
        let (pb, tb) = cars[b];
        pb.partial_cmp(&pa).unwrap_or(std::cmp::Ordering::Equal).then(ta.cmp(&tb))
    });
    order
}

/// The slice of the start scene's finish line — its type-1 trigger (0x46D290(1, 0), read by
/// `Car_raceInit` at 0x4109CF for a point-to-point track's finish fraction).
pub fn finish_slice(table: &SliceTable, start_scene: Option<&Scene>) -> Option<usize> {
    let s = start_scene?;
    s.of_kind(&ElementKind::Trigger)
        .find(|e| e.params.first().and_then(|p| p.first()).map(|t| *t as i32) == Some(1))
        .map(|e| table.find_slice(Vec3::from(e.position), 0))
}

/// What `Grid_slot` 0x410580 and `Car_raceInit` 0x410730 read besides the table and the scene.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GridRule {
    /// `LINEAR_TRACK [0x657494]` (`trk+0x6E != 1`): a point-to-point track
    pub linear: bool,
    /// `FEGAME_TYPE [0x6573E8] == 3 && DERBY [0x657400]` — the network smash-up — with `NUMCARS`
    /// `[0x658080]`, the divisor of its spacing (0x4105DB…0x41060D)
    pub derby_cars: Option<u32>,
}

/// What `Race_updateProgress` 0x416310's `DERBY` arm (0x416407…0x41646A, game type 3 only) does with one
/// racer still racing (`car+0xB50 == 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerbyOutcome {
    /// nothing: the car races on
    Racing,
    /// all eight ring zones past the limit: `car+0xB50 = 3`, `car+0xB14 = tick`, `car+0xB24 = 0`
    /// (0x416447…0x416460), then `0x48DA60(tick − 0x1C0, tick)`
    Out,
    /// every racer but this one is out: `car+0xB50 = 2`, `car+0xB14 = tick` (0x4164A8…0x4164B8), then
    /// the same `0xB24 = 0` and report (0x416458…0x41646A)
    Wins,
}

/// `[0x5B2408]`, the zone limit the knockout compares `car+0x4E4 + 4k` (k = 0…7) against.
pub const DERBY_ZONE_LIMIT: f32 = 30.0;

/// The `DERBY` arm for one racer (`network.md` §11.18, `suspension-collision.md` §6): `zones` are the ten
/// damage zones `car+0x4E4…` (the first eight are the ring, 0x41641F…0x416440: `fcomp [0x5B2408] ;
/// test ah, 0x41 ; jne` counts a zone only when it is **above** the limit); `out` is how many of the
/// `racers` (`[0x658084]`, the racer array `0x5E8C00[]`) are already in state 3 (0x416477…0x416499);
/// the last one not out wins (`out < racers − 1` leaves the car racing, 0x41649F…0x4164A2).
pub fn derby_outcome(zones: &[f32], out: usize, racers: usize) -> DerbyOutcome {
    let over = zones.iter().take(8).filter(|z| **z > DERBY_ZONE_LIMIT).count();
    if over == 8 {
        DerbyOutcome::Out
    } else if out + 1 < racers {
        DerbyOutcome::Racing
    } else {
        DerbyOutcome::Wins
    }
}

/// `Car_raceInit`'s start slice (0x410896…0x410924): the slice the tracker puts the scene's type-2
/// trigger on (0x46D290(2, 0), `setSlice(1)` then 0x474440 at its position); with no such
/// trigger, a point-to-point track whose lap has at least 100 slices starts at **50**, every other
/// track at **1** (`setge / dec / and 0xCF / add 0x32`, 0x410917…0x410921 — `[0x628BC0]+4` is the
/// lap's slice count, §1.3).
pub fn start_slice(table: &SliceTable, start_scene: Option<&Scene>, linear: bool) -> usize {
    start_scene
        .and_then(|s| {
            s.of_kind(&ElementKind::Trigger).find(|e| e.params.first().and_then(|p| p.first()).map(|t| *t as i32) == Some(2)).map(|e| table.find_slice(Vec3::from(e.position), 0))
        })
        .unwrap_or(if linear && table.lap_len >= 100 { 50 } else { 1 })
}

/// `Grid_slot` 0x410580: the slice and the slice-local offset `{lateral, 0.5, 0}` for
/// `Car_placeAtSlice` 0x49B090. `grid_pos` is `.STARTINGPOS − 1` (`[0x658200 + idx·0x580]`,
/// 1 = pole, `modes.md` §1 — so 0 here is the pole) and `n` the racers `[0x5E8AE8]`.
///
/// ```text
/// dir = REVERSE [0x657430] ? −1 : +1                                  ; 0x410592…0x4105C2
/// REVERSE on a circuit (!LINEAR [0x657494]): start = addSlices(0, −start, −1)      ; 0x4105A4
/// g = STARTINGPOS                                                     ; 0x4105C2…0x4105D5
/// slice = addSlices(start, dir·k, −1),  k = derby ? (n − g − 1)·(0xEA / NUMCARS) + 1   ; 0x4105E0
///                                        : n > 2 ? 3·(n − g) + 1 : 1                   ; 0x41060F
/// side = g & 1, flipped (1 − side) when car+0xEAC · [0x5E753C] == −1   ; 0x410638…0x410656
/// slices[1].lanesA != 0 (0x40E650):  lateral = side ? +½·laneWidthB : −½·laneWidthA   ; 0x4106D5
/// else (the alternate, one-way mode):  T = wA·nA + wB·nB, c = ½T − wA·nA, w = T / (nA + nB),
///                                      lateral = side ? c + w : c − w                 ; 0x410661
/// out = {lateral, 0.5, 0}                                            ; 0x41070F…0x410718
/// ```
///
/// `car+0xEAC` is `REVERSE ? −1 : +1` (`Car_raceInit` 0x410789…0x41079B, before this call) and
/// `[0x5E753C]` the constant 1 (0x40E670, run by the race load 0x414E57 → 0x4068A0 before the reset
/// 0x414F12 → `Cars_resetAll` → `Car_raceInit`): **the side flips in every backwards race**. The
/// port's backwards table (`SliceTable::reversed`) already negates `lateral` and swaps the A / B
/// widths and lane counts, which is the same flip — `+½·wB` along the original lateral is
/// `−½·wA` along the reversed one, and `c ± w` turns into `c' ∓ w` — so here the side stays
/// `g & 1` in the table's own frame and lands where the original's does. Likewise `dir`: a
/// positive step along the reversed table is the original's negative one, and the circuit's
/// mirrored start `addSlices(0, −start)` is `add(0, +start)` here. `slices[1].lanesA` is read in
/// the original's frame (`lanes_b` of a reversed table). No shipped track has `lanesA == 0` on
/// slice 1 (every one is 1/1 or 2/2), so the alternate mode is never taken with the shipped data;
/// the `max(1)` on its lane count only keeps the port from dividing by zero where the original
/// would (`fidiv`, 0x4106C3).
pub fn grid_slot(table: &SliceTable, start: usize, grid_pos: u32, n: u32, rule: &GridRule) -> (usize, Vec3) {
    let start = if table.backwards && !rule.linear { table.add(0, start as i32) } else { start };
    let g = grid_pos as i32 + 1;
    let n = n as i32;
    let k = match rule.derby_cars {
        Some(cars) => (n - g - 1) * (0xEA / cars.max(1) as i32) + 1,
        None if n > 2 => 3 * (n - g) + 1,
        None => 1,
    };
    let slice = table.add(start, k);
    let side = g & 1 == 1;
    let s = &table.slices[slice];
    let s1 = &table.slices[1.min(table.slices.len() - 1)];
    let lanes_a1 = if table.backwards { s1.lanes_b } else { s1.lanes_a };
    let lateral = if lanes_a1 != 0 {
        if side { 0.5 * s.lane_width_b } else { -0.5 * s.lane_width_a }
    } else {
        let a = s.lane_width_a * s.lanes_a as f32;
        let total = a + s.lane_width_b * s.lanes_b as f32;
        let c = 0.5 * total - a;
        let w = total / (s.lanes_a + s.lanes_b).max(1) as f32;
        if side { c + w } else { c - w }
    };
    (slice, Vec3::new(lateral, 0.5, 0.0))
}

/// Start pose from the grid rule (`Grid_slot` 0x410580 + `Car_placeAtSlice` 0x49B090's frame
/// before the drop): the slice-local offset put in the world along the slice's own axes.
/// `grid_pos` 0 = pole (`.STARTINGPOS − 1`), `n` racers. Returns (position, orientation rows
/// x/up/forward, slice).
pub fn grid_pose(table: &SliceTable, start_scene: Option<&Scene>, grid_pos: u32, n: u32, rule: &GridRule) -> (Vec3, Mat3, usize) {
    let start = start_slice(table, start_scene, rule.linear);
    let (slice, off) = grid_slot(table, start, grid_pos, n, rule);
    let s = &table.slices[slice];
    let pos = s.centre + s.lateral * off.x + s.up * off.y;
    let fwd = s.forward.normalize();
    let up = s.up.normalize();
    let right = up.cross(fwd).normalize();
    let up = fwd.cross(right).normalize();
    (pos, Mat3::from_cols(right, up, fwd).transpose(), slice)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_wrong_way_cue_fires_at_64_96_128() {
        let hits: Vec<u32> = (0..200).filter(|&n| wrong_way_cue(n)).collect();
        assert_eq!(hits, vec![64, 96, 128]);
    }

    use super::*;

    /// Synthetic circular lap of `n` slices, radius r, 6 m apart.
    fn ring(n: usize) -> SliceTable {
        let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
        let slices = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let centre = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                let forward = Vec3::new(-a.sin(), 0.0, a.cos());
                Slice { centre, up: Vec3::Y, forward, lateral: Vec3::Y.cross(forward), lane_mask: 0, lanes_a: 1, lanes_b: 1, width_a: 5.0, width_b: 5.0, lane_width_a: 5.0, lane_width_b: 5.0 }
            })
            .collect();
        SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
    }

    /// `Grid_slot` 0x410580: `STARTINGPOS = grid_pos + 1`, 3 slices a car, the pole furthest
    /// ahead; the side is `STARTINGPOS & 1`, the `+0x4C` half-lane for odd.
    #[test]
    fn grid_slot_spaces_by_startingpos_and_alternates_sides() {
        let t = ring(100);
        let rule = GridRule::default();
        // four racers from slice 10: STARTINGPOS 1 (pole) → 3·3 + 1 = 10 ahead, 4 (last) → 1
        assert_eq!(grid_slot(&t, 10, 0, 4, &rule), (20, Vec3::new(2.5, 0.5, 0.0)));
        assert_eq!(grid_slot(&t, 10, 3, 4, &rule), (11, Vec3::new(-2.5, 0.5, 0.0)));
        // two racers: both one slice past the line
        assert_eq!(grid_slot(&t, 10, 1, 2, &rule).0, 11);
        // the smash-up spacing: (n − g − 1)·(0xEA / NUMCARS) + 1
        let derby = GridRule { linear: true, derby_cars: Some(8) };
        assert_eq!(grid_slot(&t, 10, 0, 4, &derby).0, 10 + 2 * 29 + 1);
    }

    /// The network draw's `.STARTINGPOS` values are 0…n−1 (0x4B4FD0's pool is 0-based) and `Grid_slot`
    /// takes them as `g` itself: the 0 (`grid_pos` `u32::MAX`, i.e. `g − 1`) sits **one row ahead
    /// of the pole** (`g = 1`), on the even side.
    #[test]
    fn a_drawn_startingpos_of_zero_stands_one_row_ahead_of_the_pole() {
        let t = ring(100);
        let rule = GridRule::default();
        let pole = grid_slot(&t, 10, 0, 4, &rule);
        let zero = grid_slot(&t, 10, u32::MAX, 4, &rule);
        assert_eq!(pole.0, 10 + 10);
        assert_eq!(zero.0, 10 + 3 * 4 + 1, "k = 3·(n − 0) + 1");
        assert_eq!(zero.1, Vec3::new(-2.5, 0.5, 0.0), "g & 1 == 0: the other side");
    }

    /// Backwards: the original flips the side (`car+0xEAC · [0x5E753C] == −1`) in its own frame
    /// and mirrors a circuit's start (`addSlices(0, −start)`); on the port's reversed table the
    /// same world position comes out of the unflipped rule.
    #[test]
    fn grid_slot_backwards_lands_where_the_originals_flip_puts_it() {
        let mut fwd = ring(100);
        for s in &mut fwd.slices {
            s.lane_width_a = 4.0;
            s.lane_width_b = 6.0;
        }
        let back = fwd.reversed();
        let circuit = GridRule::default();
        for g in 0..4u32 {
            // the original, by hand: start = addSlices(0, −1) = 99, dir −1, side flipped
            let gp = g as i32 + 1;
            let slice = fwd.add(fwd.add(0, -1), -(3 * (4 - gp) + 1));
            let side = 1 - (gp & 1);
            let lat = if side == 1 { 0.5 * 6.0 } else { -0.5 * 4.0 };
            let want = fwd.slices[slice].centre + fwd.slices[slice].lateral * lat;
            let (s, off) = grid_slot(&back, 1, g, 4, &circuit);
            assert_eq!(s, slice, "grid {g}");
            let got = back.slices[s].centre + back.slices[s].lateral * off.x;
            assert!((got - want).length() < 1e-4, "grid {g}: {got:?} vs {want:?}");
        }
    }

    /// The alternate mode (`slices[1].lanesA == 0`, 0x40E650): `c ± w` with `T = wA·nA + wB·nB`,
    /// `c = ½T − wA·nA`, `w = T / (nA + nB)`.
    #[test]
    fn grid_slot_alternate_mode_centres_on_the_road() {
        let mut t = ring(100);
        for s in &mut t.slices {
            (s.lanes_a, s.lanes_b, s.lane_width_a, s.lane_width_b) = (0, 3, 4.0, 4.0);
        }
        let rule = GridRule::default();
        // T = 12, c = 6, w = 4: odd STARTINGPOS 10, even 2
        assert_eq!(grid_slot(&t, 10, 0, 4, &rule).1, Vec3::new(10.0, 0.5, 0.0));
        assert_eq!(grid_slot(&t, 10, 1, 4, &rule).1, Vec3::new(2.0, 0.5, 0.0));
    }

fn two_branch_table() -> SliceTable {
        // 20 slices of main route along +z, then 10 of a branch 50 m to the side
        let mut slices = Vec::new();
        for i in 0..20 {
            slices.push(slice_at(Vec3::new(0.0, 0.0, i as f32 * SLICE_SPACING)));
        }
        for i in 0..10 {
            slices.push(slice_at(Vec3::new(50.0, 0.0, i as f32 * SLICE_SPACING)));
        }
        let sections = vec![Section { first: 0, last: 19, f0: 0.0, f1: 1.0 }, Section { first: 20, last: 29, f0: 0.0, f1: 1.0 }];
        let route: Vec<usize> = (0..20).collect();
        let mut route_pos = vec![0usize; slices.len()];
        for (i, s) in route.iter().enumerate() {
            route_pos[*s] = i;
        }
        SliceTable { slices, sections, lap_len: 20, route, route_pos: route_pos.iter().map(|v| *v as u32).collect(), junctions: crate::junctions::Junctions::default(), backwards: false, linear: false }
    }

    fn slice_at(centre: Vec3) -> Slice {
        Slice { centre, up: Vec3::Y, forward: Vec3::Z, lateral: Vec3::X, lane_mask: 0, lanes_a: 2, lanes_b: 2, width_a: 8.0, width_b: 8.0, lane_width_a: 4.0, lane_width_b: 4.0 }
    }

#[test]
    fn the_preferred_section_wins_when_it_is_nearer() {
        // `race-logic.md` §1.4: with a preferred section the tracker also asks that section for its
        // nearest slice and keeps whichever candidate is closer.
        let t = two_branch_table();
        let on_branch = t.slices[t.sections[1].first + 2].centre;
        // the plain search covers the whole slice array now (0x481400), so a car parked on the
        // branch is tracked onto the branch whether or not a section is preferred
        assert_eq!(t.find_slice(on_branch, 0), t.sections[1].first + 2);
        assert_eq!(t.track_slice(on_branch, 0, Some(1)), t.sections[1].first + 2);
        // and a preference that is further away changes nothing
        let on_main = t.slices[3].centre;
        assert_eq!(t.track_slice(on_main, 0, Some(1)), t.find_slice(on_main, 0));
        assert_eq!(t.find_slice(on_main, 0), 3);
    }

    #[test]
    fn reversed_table_runs_the_other_way() {
        let t = ring(100);
        let r = t.reversed();
        assert_eq!(r.route[0], t.route[99]);
        assert_eq!(r.add(t.route[99], 1), t.route[98]);
        assert!((r.slices[10].forward + t.slices[10].forward).length() < 1e-6);
        // 0x416290 backwards: 1 − raw/N on the RAW slice (raw 99 → 0.01), and raw 0 → 1.0 → 0.0 on a circuit
        assert!((r.frac(t.route[99]) - 0.01).abs() < 1e-6);
        assert_eq!(r.frac(t.route[0]), 0.0);
        assert!((r.frac(t.route[1]) - 0.99).abs() < 1e-6);
        let mut lin = r.clone();
        lin.linear = true;
        assert_eq!(lin.frac(t.route[0]), 1.0);
        assert_eq!(r.find_slice(t.slices[57].centre, 60), 57);
        // driving along the reversed route counts laps forward
        let mut p = RaceProgress::new(r.route[0], &r);
        let mut laps = 0;
        let mut lap_steps = Vec::new();
        for step in 1..=200usize {
            let s = r.route[step % 100];
            if p.update(&r, r.slices[s].centre, r.slices[s].forward, step as u64, 3) {
                laps += 1;
                lap_steps.push(s);
            }
        }
        assert_eq!(laps, 2);
        // 0x416290 on the raw table: the lap line is crossed on entering RAW slice 0 (1 − 1/N →
        // 0.0), not one slice later on entering raw N−1 as the reversed-table fraction had it
        assert_eq!(lap_steps, vec![t.route[0], t.route[0]]);
        assert_eq!(p.wrong_way_ticks, 0);
    }

    #[test]
    fn find_slice_tracks_position() {
        let t = ring(200);
        assert_eq!(t.find_slice(t.slices[57].centre + Vec3::new(1.0, 0.0, 1.0), 50), 57);
        // far away: global scan
        assert_eq!(t.find_slice(t.slices[150].centre, 3), 150);
    }

    #[test]
    fn laps_count_forward_and_ignore_backward_wobble() {
        let t = ring(100);
        let mut p = RaceProgress::new(0, &t);
        let mut lap_events = 0;
        // drive one and a half laps forward
        for step in 1..=150usize {
            let s = step % 100;
            let pos = t.slices[s].centre;
            if p.update(&t, pos, t.slices[s].forward, step as u64, 3) {
                lap_events += 1;
            }
        }
        assert_eq!(lap_events, 1);
        assert_eq!(p.laps, 1);
        assert!(p.progress > 1.4 && p.progress < 1.6, "{}", p.progress);
        // reverse across the line and come back: no extra lap
        let mut p = RaceProgress::new(2, &t);
        for s in [2usize, 1, 0, 99, 98, 99, 0, 1, 2, 3] {
            p.update(&t, t.slices[s].centre, t.slices[s].forward, 1, 3);
        }
        assert_eq!(p.laps, 0);
        assert_eq!(p.unlaps, 0);
    }

    #[test]
    fn a_point_to_point_race_ends_past_the_finish_fraction() {
        // 0x41748D: past `car+0xB04` the lap (and the one-lap race) is done; below `car+0xB08`
        // the car is flagged to go back to its start
        let t = ring(100);
        let mut p = RaceProgress::new(5, &t);
        p.set_linear(&t, Some(90));
        let (finish, below) = p.linear.unwrap();
        assert!((finish - t.frac(90)).abs() < 1e-6 && (below - (t.frac(5) - 0.005)).abs() < 1e-6);
        for s in 6..=90usize {
            assert!(!p.update(&t, t.slices[s].centre, t.slices[s].forward, s as u64, 1));
        }
        assert!(p.update(&t, t.slices[91].centre, t.slices[91].forward, 91, 1));
        assert!(p.finished && p.laps == 1);
        // 0x4167B9: the finish rewrites `car+0xAF8` as the bare fraction — the lap is not added
        assert_eq!(p.progress, t.frac(91));
        // and it stays there (0x416365 skips a finished car)
        p.update(&t, t.slices[93].centre, t.slices[93].forward, 92, 1);
        assert_eq!(p.progress, t.frac(91));
        // and backing up behind the start raises the flag instead of an un-lap
        let mut q = RaceProgress::new(5, &t);
        q.set_linear(&t, None);
        q.update(&t, t.slices[3].centre, t.slices[3].forward, 1, 1);
        assert!(q.behind_start && q.laps == 0 && q.unlaps == 0);
    }

    #[test]
    fn wrong_way_detected() {
        let t = ring(100);
        let mut p = RaceProgress::new(10, &t);
        p.update(&t, t.slices[10].centre, -t.slices[10].forward, 1, 3);
        assert_eq!(p.wrong_way_ticks, 1);
        p.update(&t, t.slices[10].centre, t.slices[10].forward, 2, 3);
        assert_eq!(p.wrong_way_ticks, 0);
    }
}

#[cfg(test)]
mod finish_tests {
    use super::*;

    /// A finished car stops accumulating laps and keeps its finishing progress (0x4169B0).
    #[test]
    fn progress_freezes_after_the_finish() {
        let n = 100;
        let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
        let slices: Vec<Slice> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let centre = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                let forward = Vec3::new(-a.sin(), 0.0, a.cos());
                Slice { centre, up: Vec3::Y, forward, lateral: Vec3::Y.cross(forward), lane_mask: 0, lanes_a: 1, lanes_b: 1, width_a: 5.0, width_b: 5.0, lane_width_a: 5.0, lane_width_b: 5.0 }
            })
            .collect();
        let t = SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }]);
        let mut p = RaceProgress::new(0, &t);
        for step in 1..=250usize {
            let s = step % n;
            p.update(&t, t.slices[s].centre, t.slices[s].forward, step as u64, 1);
        }
        assert!(p.finished);
        assert_eq!(p.laps, 1);
        assert_eq!(p.lap_times.len(), 1, "no laps after the finish");
        // 0x416838 on a circuit: laps timed + the fraction of the slice it crossed on
        assert_eq!(p.progress, 1.0 + t.frac(0));
    }

    /// 0x4A7660: heading against the slice counts, an airborne car holds, a gated one clears.
    #[test]
    fn wrong_way_counter_follows_its_gates() {
        let n = 100;
        let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
        let slices: Vec<Slice> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let centre = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                let forward = Vec3::new(-a.sin(), 0.0, a.cos());
                Slice { centre, up: Vec3::Y, forward, lateral: Vec3::Y.cross(forward), lane_mask: 0, lanes_a: 1, lanes_b: 1, width_a: 5.0, width_b: 5.0, lane_width_a: 5.0, lane_width_b: 5.0 }
            })
            .collect();
        let t = SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }]);
        let mut p = RaceProgress::new(10, &t);
        let back = |p: &mut RaceProgress, tick: u64| p.update(&t, t.slices[10].centre, -t.slices[10].forward, tick, 3);
        for tick in 1..=4 {
            back(&mut p, tick);
        }
        assert_eq!(p.wrong_way_steps, 2);
        p.wrong_way_gate = WrongWayGate::Hold;
        back(&mut p, 5);
        assert_eq!(p.wrong_way_steps, 2, "held while airborne");
        p.wrong_way_gate = WrongWayGate::Reset;
        back(&mut p, 7);
        assert_eq!(p.wrong_way_steps, 0, "cleared when gated");
    }
}

/// `car+0xB50`, the finish state (`race-logic.md` §2): 0 racing, 2 finished, 3 knocked out.
pub const STATE_RACING: u8 = 0;
pub const STATE_FINISHED: u8 = 2;
pub const STATE_OUT: u8 = 3;

/// What `Race_finalize` 0x416AC0 reads of one racer: its progress `car+0xAF8`, the start and
/// finish fractions `car+0xB00` / `car+0xB04` (0 and 1.0 on a circuit, 0x4109A3; the car's own
/// start slice's and the finish trigger's on a point-to-point track, 0x41098A / 0x410A0A), the
/// slice-change / finish tick `car+0xB14`, the state `car+0xB50`, the race time `car+0xB24` (the
/// finish tick less 448, 0x416840, or 0 once knocked out, 0x41668D), the lap times `car+0xB28[]`
/// (`gGameTick − car+0xB24` at each line, 0x416626…0x416630),
/// the live position `car+0xB48` (1-based) and the ten damage zones `car+0x4E4`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FinalizeIn {
    pub progress: f32,
    pub start: f32,
    pub finish: f32,
    pub tick: u64,
    pub state: u8,
    /// `+0xB24`, a signed dword
    pub time: i32,
    /// `+0xB28[]`, signed dwords
    pub lap_times: Vec<i32>,
    pub position: u32,
    pub damage: [f32; 10],
}

impl FinalizeIn {
    /// The fields as a [`RaceProgress`] holds them; the state is the caller's (the port keeps the
    /// knock-out beside the progress, `RaceCar::eliminated` / `App::player_eliminated`). The race
    /// time `+0xB24` of a finished car is its lap times' sum (the finish tick less 448), 0 for any
    /// other state — a car still racing gets its projection from [`race_finalize`].
    pub fn from_progress(p: &RaceProgress, state: u8, position: u32, damage: [f32; 10]) -> Self {
        // on a circuit 0x4109A3 writes `+0xB00 = 0`, `+0xB04 = 1.0`
        let (start, finish) = match p.linear {
            Some((finish, _)) => (p.start_progress, finish),
            None => (0.0, 1.0),
        };
        // the finish leaves `+0xB24` = the finish tick (the lap start just stored, 0x41663D) less
        // 448 (0x416840…0x41684C) — the race time from the green light. *(The port used the lap
        // times' sum, which counts the first lap from tick 0 — `RaceProgress::lap_start_tick`'s
        // start, the countdown included — and so came out 448 ticks longer.)*
        let time = if state == STATE_FINISHED { (p.tick as i32).wrapping_sub(0x1C0) } else { 0 };
        Self { progress: p.progress, start, finish, tick: p.tick, state, time, lap_times: p.lap_times.iter().map(|&l| l as i32).collect(), position, damage }
    }
}

/// One racer's result record — `car+0xCC4 … +0xD2C`, which only `Race_finalize` 0x416AC0 writes
/// (`+0xCD4` at 0x416CD2 / 0x416F1D, `+0xCD8` at 0x416B1F / 0x416CE2 / 0x416F2B, `+0xC98` in its
/// three copy loops; the one other writer, 0x426B81…0x426BF6, is the developer "finish 8th" swap).
/// The result pages, the HUD tally 0x427660, the mission summary 0x4293F0 and the FE's 0x4EDF00
/// (through the copy at 0x5E75A0, `modes.md` §7.1) read these, never the live car.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RacerResult {
    /// `+0xCD4`: 1 + the car's row in the ranking table 0x608B38, 0 when it is not in it
    pub place: u32,
    /// `+0xCD8`: the total time in ticks as the signed dword the original keeps, 0 for none (the
    /// summary prints dashes, 0x42C41C); negative for a projected time (see [`fistp_time`])
    pub time: i32,
    /// `+0xCD0`: the finish state as finalised
    pub state: u8,
    /// `+0xCFC`: the best lap (signed dword; the compares at 0x416D7E / 0x416FD0 are `jge`)
    pub best_lap: i32,
    /// `+0xCDC[NUM_LAPS]`: the lap times
    pub laps: Vec<i32>,
    /// `+0xC98[10]`: the damage zones at the race's end (0x416B47 / 0x416D39 / 0x416F4D copy
    /// `car + eax − 0x7B8`, i.e. `+0x4E4…`)
    pub damage: [f32; 10],
}

/// `fistp` of a value `fstp`ped to a dword first (0x416BF9…0x416C03): round to nearest, even on
/// a tie, into the **signed** dword the original stores at `[car+0xB24]` (0x416C0B, 0x416EC7): a
/// negative time (`(+0xB14 − 448)·laps/ratio` with `+0xB14 < 448`, a car ahead of the human's
/// start by ≥ 0.5 % whose last slice change is still before the race start 0x1C0) is kept as it
/// is, and only ∞ / NaN / an overflow (≥ 2³¹) give the integer indefinite 0x80000000. The time
/// formatter 0x4356E0 prints the absolute value (with a sign only when its flag is set, which
/// none of the race-result printers sets — [`nfs_game::records::format_time_dword`]).
/// *(Until 2026-09-30 the port folded a negative or indefinite result into 0, "no time".)*
fn fistp_time(v: f32) -> i32 {
    let r = v.round_ties_even();
    if r.is_finite() && (-2_147_483_648.0..2_147_483_648.0).contains(&r) { r as i32 } else { i32::MIN }
}

/// The projected total of a car still racing, the "ratio" arm (the human's 0x416B96…0x416C03, the
/// opponent's on a point-to-point track 0x416E1F…0x416E92):
///
/// ```text
/// num   = progress − start ; if num <= 0.001 (fcom, test ah,0x41): num = 0.001
/// ratio = num / (finish − start)
/// x     = ratio < 0.005 (fcomp, test ah,1) ? ratio · 250000 : (+0xB14 − 448)
/// time  = fistp((float)(NUM_LAPS · x / ratio))
/// ```
///
/// The stack juggling at 0x416BE5…0x416BF7 is what makes a car that has hardly moved come out at
/// `NUM_LAPS · 250000` ticks, not at a scaled-up ratio. *(Until 2026-09-26 the port had
/// `ratio *= 250000` and then `elapsed / ratio`, measured `elapsed` to the current tick rather than
/// to `+0xB14`, took the finish fraction as 1.0 and truncated.)*
pub fn projected_ratio(progress: f32, start: f32, finish: f32, tick: u64, num_laps: u32) -> i32 {
    let mut num = progress as f64 - start as f64;
    if num <= 0.001f32 as f64 {
        num = 0.001f32 as f64;
    }
    let ratio = num / (finish as f64 - start as f64);
    let elapsed = tick as i64 - 0x1C0;
    let x = if ratio < 0.005f32 as f64 { ratio * 250_000.0 } else { elapsed as f64 };
    fistp_time((num_laps as f64 * x / ratio) as f32)
}

/// The opponent's circuit arm 0x416E9A…0x416EC2: `(+0xB14 − 448) / (progress / NUM_LAPS)`, no
/// clamp.
pub fn projected_circuit(progress: f32, tick: u64, num_laps: u32) -> i32 {
    let elapsed = (tick as i64 - 0x1C0) as f64;
    fistp_time((elapsed / (progress as f64 / num_laps as f64)) as f32)
}

/// `Race_finalize(2)` 0x416AC0 as 0x417010 calls it at race over. `humans` are the first entries
/// of `cars` (0x5E8C00, `[0x5E8BFC]`), the opponents the rest (0x5E8BD4, `[0x5E8BF8]`); `order`
/// is the ranking table 0x608B38 as indices into `cars`; `tick` is `[0x606AC4]`; `linear` is
/// `LINEAR_TRACK [0x657494]` and `lap_knockout` is `RACE_TYPE == 3`. The records come back in the
/// order of `cars`, and `cars` is left as the function leaves the live cars (a still-racing
/// opponent finalised to state 2 with its time, or knocked out to 3).
///
/// Not carried: the skill-test arm (`RACE_TYPE == 4`, 0x416B14…0x416B6D — the human's time, state,
/// traffic hits and damage, no place, no opponents), the `flag == 1` arm of an early leave (the
/// port files nothing then), a human still in state 1 (only that arm makes one), the traffic-hit
/// counts `+0xCC4…+0xCCC` and the race-end fold of the damage into the parts `0x493CF0(h, 0)`,
/// which the port runs when the car is parked (`career.md` §11).
pub fn race_finalize(cars: &mut [FinalizeIn], humans: usize, order: &[usize], num_laps: u32, tick: u64, linear: bool, lap_knockout: bool) -> Vec<RacerResult> {
    let place = |i: usize| order.iter().position(|&k| k == i).map_or(0, |r| r as u32 + 1);
    let n = num_laps as usize;
    let human_start = cars.first().map_or(0.0, |c| c.start);
    let lap_copy = |c: &FinalizeIn| -> Vec<i32> { (0..n).map(|j| c.lap_times.get(j).copied().unwrap_or(0)).collect() };
    let mut out = Vec::with_capacity(cars.len());
    for (i, c) in cars.iter_mut().enumerate() {
        let mut best;
        if i < humans {
            // 0x416CF8: the human's best starts at its first lap `+0xB28[0]`, and 0x416D65… takes
            // a lap below it that is not 0 — so with no lap at all it stays 0
            best = c.lap_times.first().copied().unwrap_or(0);
        } else {
            if c.state == STATE_RACING {
                if lap_knockout && c.position != 1 {
                    // 0x416DE8…0x416E0B: a lap knockout puts every car not leading out, time 0
                    c.tick = tick;
                    c.time = 0;
                    c.state = STATE_OUT;
                } else {
                    // 0x416E16: the ratio arm with **the first human's** `+0xB00` on a
                    // point-to-point track, the plain one on a circuit; state 2; an empty first
                    // lap takes the projection (0x416ED9…0x416EE5)
                    let t = if linear { projected_ratio(c.progress, human_start, c.finish, c.tick, num_laps) } else { projected_circuit(c.progress, c.tick, num_laps) };
                    c.time = t;
                    c.state = STATE_FINISHED;
                    match c.lap_times.first_mut() {
                        Some(l) if *l == 0 => *l = t,
                        None => c.lap_times.push(t),
                        _ => {}
                    }
                }
            }
            // 0x416F6D: 99999, then any lap below it that is positive (0x416F86…)
            best = 99_999;
        }
        let laps = lap_copy(c);
        // 0x416D65… / 0x416FB9…: `+0xCDC[j] = +0xB28[j]`, and one below the best that is not 0
        for &l in &laps {
            if l < best && l != 0 {
                best = l;
            }
        }
        out.push(RacerResult { place: place(i), time: c.time, state: c.state, best_lap: best, laps, damage: c.damage });
    }
    out
}

#[cfg(test)]
mod projection_tests {
    use super::*;

    #[test]
    fn projected_time_extrapolates_the_pace() {
        // circuit (start 0, finish 1): half of a 2-lap race done 60 s after the start → 120 s
        assert_eq!(projected_ratio(1.0, 0.0, 1.0, 448 + 60 * 64, 2), 120 * 64);
        assert_eq!(projected_circuit(1.0, 448 + 60 * 64, 2), 120 * 64);
        // point-to-point: 40 % of the way in 40 s → 100 s
        assert_eq!(projected_ratio(0.4, 0.0, 1.0, 448 + 40 * 64, 1), 100 * 64);
        // a car that started mid-track is measured from its start fraction to the finish's:
        // 0.2 → 0.6 of 0.2 → 0.98 in 40 s → 40 · 0.78 / 0.4 = 78 s
        assert!((projected_ratio(0.6, 0.2, 0.98, 448 + 40 * 64, 1) as i64 - 78 * 64).abs() <= 1);
        // a car that has hardly moved: the x87 stack gives NUM_LAPS · 250000 (0x416BE5…0x416BF7)
        assert_eq!(projected_ratio(0.0, 0.0, 1.0, 5000, 3), 750_000);
    }

    #[test]
    fn a_projection_before_the_race_start_is_a_negative_dword() {
        // a car ahead of the human's start by more than 0.5 % whose last slice change
        // (`+0xB14`) is still before 448: `(100 − 448) · 1 / 0.6` = −580, kept signed (0x416C0B)
        assert_eq!(projected_ratio(0.6, 0.0, 1.0, 100, 1), -580);
        // the circuit arm: `(100 − 448) / (1.0 / 1)` = −348
        assert_eq!(projected_circuit(1.0, 100, 1), -348);
        // a circuit car with no progress divides by 0: ∞ → the integer indefinite 0x80000000
        assert_eq!(projected_circuit(0.0, 1000, 1), i32::MIN);
        // 10⁷ ticks · laps overflows the dword too
        assert_eq!(projected_circuit(1.0, 3_000_000_000, 1), i32::MIN);
    }

    #[test]
    fn a_still_racing_car_finalised_with_a_negative_time() {
        let mut cars = vec![
            FinalizeIn { progress: 1.0, finish: 1.0, tick: 900, state: STATE_OUT, position: 2, ..Default::default() },
            FinalizeIn { progress: 0.6, finish: 1.0, tick: 100, state: STATE_RACING, position: 1, ..Default::default() },
        ];
        let r = race_finalize(&mut cars, 1, &[1, 0], 1, 1300, true, false);
        // point-to-point: measured from the first human's start 0.0, (100 − 448) / 0.6 = −580
        assert_eq!((r[1].time, r[1].state), (-580, STATE_FINISHED));
        // the empty first lap takes the projection (0x416EE5) and, `jge` at 0x416FD0, a negative
        // lap is below the 99999 and not 0: it becomes the best lap
        assert_eq!((r[1].laps.clone(), r[1].best_lap), (vec![-580], -580));
    }

    #[test]
    fn race_over_records() {
        // the player finished second, one opponent finished first, one is still racing
        let mut cars = vec![
            FinalizeIn { progress: 2.0, finish: 1.0, tick: 900, state: STATE_FINISHED, time: 450, lap_times: vec![230, 220], position: 2, ..Default::default() },
            FinalizeIn { progress: 2.0, finish: 1.0, tick: 880, state: STATE_FINISHED, time: 432, lap_times: vec![216, 216], position: 1, ..Default::default() },
            FinalizeIn { progress: 1.5, finish: 1.0, tick: 1200, state: STATE_RACING, lap_times: vec![300], position: 3, ..Default::default() },
        ];
        let r = race_finalize(&mut cars, 1, &[1, 0, 2], 2, 1300, false, false);
        assert_eq!((r[0].place, r[0].time, r[0].best_lap, r[0].state), (2, 450, 220, STATE_FINISHED));
        assert_eq!((r[1].place, r[1].best_lap), (1, 216));
        // (1200 − 448) / (1.5 / 2) = 1002.67 → 1003, state 2, best lap its one lap
        assert_eq!((r[2].place, r[2].time, r[2].state, r[2].best_lap), (3, 1003, STATE_FINISHED, 300));
        assert_eq!(r[2].laps, vec![300, 0]);
        // a lap knockout: the one still racing and not leading is out, time 0, best 99999
        let mut cars = vec![
            FinalizeIn { progress: 2.0, finish: 1.0, tick: 900, state: STATE_FINISHED, time: 450, lap_times: vec![230, 220], position: 1, ..Default::default() },
            FinalizeIn { progress: 0.5, finish: 1.0, tick: 700, state: STATE_RACING, position: 2, ..Default::default() },
        ];
        let r = race_finalize(&mut cars, 1, &[0, 1], 2, 1300, false, true);
        assert_eq!((r[1].time, r[1].state, r[1].best_lap), (0, STATE_OUT, 99_999));
        assert_eq!(cars[1].tick, 1300);
        // a human with no lap: best lap 0 (0x416CF8); an opponent with an empty first lap takes
        // its projection there
        let mut cars = vec![
            FinalizeIn { progress: 0.9, finish: 1.0, tick: 900, state: STATE_OUT, position: 2, ..Default::default() },
            FinalizeIn { progress: 0.5, finish: 1.0, tick: 448 + 640, state: STATE_RACING, position: 1, ..Default::default() },
        ];
        let r = race_finalize(&mut cars, 1, &[1, 0], 1, 1300, false, false);
        assert_eq!(r[0].best_lap, 0);
        assert_eq!((r[1].time, r[1].best_lap), (1280, 1280));
    }
}

#[cfg(test)]
mod ranking_tests {
    use super::{derby_outcome, ranking, DerbyOutcome};

    #[test]
    fn the_ranking_table_is_progress_then_the_earlier_tick() {
        // a finisher frozen at 2.003 (slice 1 when it was seen past the line), one at 2.001 that
        // got there first, a car knocked out at 1.99 and one still racing at 1.995
        let cars = [(2.001, 500), (1.99, 480), (2.003, 510), (1.995, 470)];
        assert_eq!(ranking(&cars), vec![2, 0, 3, 1]);
        // equal progress: the earlier `car+0xB14` first; a full tie keeps the update order
        assert_eq!(ranking(&[(1.5, 20), (1.5, 10), (1.5, 10)]), vec![1, 2, 0]);
    }

    #[test]
    fn the_smash_up_knocks_a_car_out_at_eight_zones_over_thirty_and_the_last_one_wins() {
        // 0x41641F…0x416440: the ring zones 0…7 strictly above 30 — the roof (8) and the underside (9)
        // do not count, and 30 itself is not over
        let mut z = [31.0f32; 10];
        assert_eq!(derby_outcome(&z, 0, 3), DerbyOutcome::Out);
        z[7] = 30.0;
        assert_eq!(derby_outcome(&z, 0, 3), DerbyOutcome::Racing, "seven over is not out");
        z[7] = 31.0;
        z[8] = 0.0;
        z[9] = 0.0;
        assert_eq!(derby_outcome(&z, 0, 3), DerbyOutcome::Out, "the roof and the floor are not in the count");
        // 0x416477…0x4164A2: `out < racers − 1` races on; the last one not out wins
        let fresh = [0.0f32; 10];
        assert_eq!(derby_outcome(&fresh, 1, 3), DerbyOutcome::Racing);
        assert_eq!(derby_outcome(&fresh, 2, 3), DerbyOutcome::Wins);
        assert_eq!(derby_outcome(&fresh, 0, 1), DerbyOutcome::Wins, "a lone racer wins at once (0 < 0 fails)");
        // a car whose own eight zones are over is out before the count is looked at
        assert_eq!(derby_outcome(&z, 2, 3), DerbyOutcome::Out);
    }
}
