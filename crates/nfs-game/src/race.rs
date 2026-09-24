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
        Self { slices, sections, route, route_pos, lap_len, junctions, backwards: false }
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
    pub fn frac(&self, slice: usize) -> f32 {
        self.route_pos.get(slice).copied().unwrap_or(0) as f32 / self.lap_len as f32
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

    /// `Race_finalize` 0x416AC0 (CONFIRMED): the finishing time of a car that is still racing when
    /// the race ends is extrapolated from how far it got. `elapsed` = ticks since the start
    /// (`tick − 448`), `circuit` selects the two formulas of the original.
    pub fn projected_ticks(&self, elapsed: u64, num_laps: u32, circuit: bool) -> u64 {
        if self.finished {
            return self.lap_times.iter().sum();
        }
        let laps = num_laps.max(1) as f32;
        if circuit {
            let frac = (self.progress / laps).max(1e-4);
            (elapsed as f32 / frac) as u64
        } else {
            let span = (1.0 - self.start_progress).max(1e-4);
            let mut ratio = (self.progress - self.start_progress).max(0.001) / span;
            if ratio < 0.005 {
                ratio *= 250_000.0;
            }
            (laps * elapsed as f32 / ratio) as u64
        }
    }

    /// Update at 32 Hz (even ticks): tracker, `Lap_check` 0x417440, `Race_updateProgress` 0x416310,
    /// wrong-way 0x4A7660. Returns true when a lap was completed this call.
    pub fn update(&mut self, table: &SliceTable, pos: Vec3, forward: Vec3, tick: u64, num_laps: u32) -> bool {
        self.update_tracked(table, pos, forward, tick, num_laps, None)
    }

    /// The same with the tracker's `preferredSection` (`race-logic.md` §1.4): a car that has taken a
    /// junction branch resolves its slice against that branch's section too.
    pub fn update_tracked(&mut self, table: &SliceTable, pos: Vec3, forward: Vec3, tick: u64, num_laps: u32, preferred: Option<usize>) -> bool {
        self.slice = table.track_slice(pos, self.slice, preferred);
        // finished / eliminated cars leave the progress accounting (0x4169B0: `state >= 2 ⇒ return`),
        // so their lap list and rank freeze at the finish
        if self.finished {
            return false;
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
        if forward.dot(table.slices[self.slice].forward) < 0.0 {
            self.wrong_way_ticks += 1;
            if tick & 1 == 1 {
                self.wrong_way_steps += 1;
            }
        } else {
            self.wrong_way_ticks = 0;
            self.wrong_way_steps = 0;
        }
        lap_done
    }
}

/// The slice of the start scene's finish line — its type-1 trigger (0x46D290(1, 0), read by
/// `Car_raceInit` at 0x4109CF for a point-to-point track's finish fraction).
pub fn finish_slice(table: &SliceTable, start_scene: Option<&Scene>) -> Option<usize> {
    let s = start_scene?;
    s.of_kind(&ElementKind::Trigger)
        .find(|e| e.params.first().and_then(|p| p.first()).map(|t| *t as i32) == Some(1))
        .map(|e| table.find_slice(Vec3::from(e.position), 0))
}

/// Start pose from the grid rule (`Grid_slot` 0x410580 + `Car_placeAtSlice` 0x49B090).
/// `grid_pos` 0 = pole, `n` racers. Returns (position, orientation rows x/up/forward, slice).
pub fn grid_pose(table: &SliceTable, start_scene: Option<&Scene>, grid_pos: u32, n: u32) -> (Vec3, Mat3, usize) {
    // start slice = slice nearest the type-2 trigger, else 1
    let start_slice = start_scene
        .and_then(|s| {
            s.of_kind(&ElementKind::Trigger).find(|e| e.params.first().and_then(|p| p.first()).map(|t| *t as i32) == Some(2)).map(|e| table.find_slice(Vec3::from(e.position), 0))
        })
        .unwrap_or(1);
    let delta = if n > 2 { 3 * (n as i32 - grid_pos as i32) + 1 } else { 1 };
    let slice = table.add(start_slice, delta);
    let s = &table.slices[slice];
    let lateral = if grid_pos & 1 == 1 { 0.5 * s.lane_width_b } else { -0.5 * s.lane_width_a };
    let pos = s.centre + s.lateral * lateral + s.up * 0.5;
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
        SliceTable { slices, sections, lap_len: 20, route, route_pos: route_pos.iter().map(|v| *v as u32).collect(), junctions: crate::junctions::Junctions::default(), backwards: false }
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
        assert!((r.frac(t.route[99]) - 0.0).abs() < 1e-6);
        assert_eq!(r.find_slice(t.slices[57].centre, 60), 57);
        // driving along the reversed route counts laps forward
        let mut p = RaceProgress::new(r.route[0], &r);
        let mut laps = 0;
        for step in 1..=200usize {
            let s = r.route[step % 100];
            if p.update(&r, r.slices[s].centre, r.slices[s].forward, step as u64, 3) {
                laps += 1;
            }
        }
        assert_eq!(laps, 2);
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
        assert!((p.progress - 1.0).abs() < 0.01);
    }
}

#[cfg(test)]
mod projection_tests {
    use super::*;

    fn progress(p: f32, start: f32) -> RaceProgress {
        RaceProgress { progress: p, start_progress: start, ..Default::default() }
    }

    #[test]
    fn projected_time_extrapolates_the_pace() {
        // circuit: half of a 2-lap race done in 60 s → projected 120 s
        let p = progress(1.0, 0.0);
        assert_eq!(p.projected_ticks(60 * 64, 2, true), 120 * 64);
        // point-to-point: 40 % of the way in 40 s → 100 s
        let p = progress(0.4, 0.0);
        assert_eq!(p.projected_ticks(40 * 64, 1, false), 100 * 64);
        // a car that started mid-track is measured from its own start fraction
        // 0.2 → 0.6 of the way (half the remaining span) in 40 s → 80 s
        let p = progress(0.6, 0.2);
        assert!((p.projected_ticks(40 * 64, 1, false) as i64 - 80 * 64).abs() <= 2);
        // finished cars report their real total
        let mut f = progress(1.0, 0.0);
        f.finished = true;
        f.lap_times = vec![1000, 1100];
        assert_eq!(f.projected_ticks(9999, 2, true), 2100);
    }
}
