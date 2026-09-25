//! Police — the parts of `RE/notes/traffic.md` §17 and §25 that decide **whether** the player is
//! pursued and **where** a police car appears.
//!
//! The pursuit trigger is `AIHigh_Perp::check` 0x403990: a violation plus a cop-class car that is
//! actually in the world. The speed limit is a flat 22 m/s everywhere — `0x40AF60` ignores the slice it
//! is handed and returns `[0x5B265C] = 22.0` (§17.3). There is no heat counter gating the start; heat
//! only decides how many cops and how good they are.
//!
//! The spawn search is `0x405440`: a marker 31 slices ahead of the perp sweeps the track, and a `.cop`
//! record whose slice the marker just passed is taken if it is off its 40 s cooldown, no human is within
//! 172 m of it, and a probability roll of `16/nRecords` succeeds.

use crate::race::SliceTable;
use nfs_formats::cop::{CopFile, CopRecord};

/// `0x40AF60` — the "speed limit", the same everywhere (`[0x5B265C]`).
pub const SPEED_LIMIT: f32 = 22.0;
/// `[0x5B25C8]` — a cop gives up past this track distance from its target.
pub const GIVE_UP_DISTANCE: f32 = 216.0;
/// The marker runs this many slices ahead of the perp (0x405440).
pub const SPAWN_LEAD_SLICES: i32 = 31;
/// `0xA00` — the cooldown of a `.cop` record. `0x40E4B0` rejects record `k` while
/// `gGameTick − lastUse[k] <= 0xA00` (0x40E56F…0x40E584: `[0x606AC4]` less `mgr+0x198[k]`, then a
/// **signed** `cmp …, 0xA00 ; jle`), so a record handed out at tick `t` is free again from `t + 2561`.
/// `CopManager::load` 0x40E3B0 fills `lastUse[]` with `0xFFFFF600` = −2560 (0x40E3C7 / 0x40E3D1), so
/// at the start of a race the test is `gGameTick + 2560 <= 2560`, which holds **only at tick 0**:
/// every spawn point is usable from the first tick after it. *(Until 2026-09-25 this said the points
/// were blocked for the first 40 s; the port also tested `< 2560`, one tick short of the original.)*
pub const SPAWN_COOLDOWN: i64 = 0xA00;
/// A spawn point is vetoed when a human is within this track distance of it (`0x407CC0`).
pub const HUMAN_VETO: f32 = 172.0;
/// The sweep since the last query must be this short, or the search is skipped.
pub const MAX_SWEEP: i32 = 6;
/// A clean perp only gets a cop when every active cop is further than this away (`0x403120`, which
/// returns 0 as soon as one cop's signed track gap `0x40E740` is inside the distance).
pub const CLEAN_PERP_CLEARANCE: f32 = 500.0;
/// `lim[role]` for role 0 out of the **pinned** heat record — chain 0, record 0 of `0x5CBD20`
/// (`+0x9C` is written only by the ctor 0x405C40 and 0x405CE0, both with the same value, so the
/// chain never advances; §25.5). Dumped from the image: `lim = {2, 0, 0}`, and every cop in this
/// build is role 0 (§17.1), so **two** cops may be assigned to a wanted perp.
pub const HEAT_LIM_ROLE0: usize = 2;
/// The race must be this far in before a pursuit may start (0x403990).
pub const PURSUIT_START_TICK: u64 = 0x200;

/// `AIHigh_Perp+0x78` — what the player is wanted for. 0 is clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Violation {
    #[default]
    None,
    Speeding,
    WrongWay,
    LightContact,
    HeavyContact,
}

impl Violation {
    pub fn is_clean(self) -> bool {
        self == Violation::None
    }
}

/// `[0x5E7544]`, the one-way (alternate lane-width) flag. `0x40E670`, run once by the race load
/// (0x4068D8), stores `[[0x628BC0]+0xC]+0x88 == 0` (0x40E670…0x40E68F): the slice array's record 1
/// (stride 0x50) at `+0x38`, **lanesA** of slice 1 in the track's own frame. The original never
/// reorders or mirrors its slice table for a backwards race (`REVERSE [0x657430]` is a sign the
/// readers apply), so it is the same flag either way; the port's backwards table
/// (`SliceTable::reversed`) swaps each slice's A / B lane counts in place, so there the original's
/// lanesA is `lanes_b` — as `race::grid_slot` already reads it. *(Until 2026-09-25 `main.rs` read
/// `lanes_a` off the reversed table for the police, i.e. the original's lanesB in a backwards race.
/// No shipped track has `lanesA == 0` on slice 1, so nothing observable changes with that data.)*
pub fn one_way_road(table: &SliceTable) -> bool {
    table.slices.get(1).map(|s| if table.backwards { s.lanes_b } else { s.lanes_a } == 0).unwrap_or(false)
}

/// `AIHigh_Perp::check` 0x403990 (§17.3). `speed` is `car+0xEC0` — **`velocity · slice forward`**,
/// written at 0x4178BE…0x4178E6 from `car+0x3D0..0x3D8` and the world velocity, for every car
/// (0x408810's gate is `car+0x52C & 0x800` or `car+0x7E == 0`, and `+0x7E` is never anything but 0).
/// `lane` is `car+0x1020`, the centre lane 0x40EB30 puts the car in. `route_dir` is the direction of
/// the race, `one_way` the `[0x5E7544]` flag, `hit_cop` whether the player hit a cop within the last
/// 15 ticks and `hit_effect` that impact's magnitude (`car+0x458`). `current` is the violation
/// already being served. Returns the new violation, which is only *committed* when a cop is in the
/// world and the race is past tick 0x200.
pub fn check_violation(current: Violation, speed: f32, route_dir: f32, one_way: bool, lane: i32, hit_cop: bool, hit_effect: f32) -> Violation {
    let mut v = current;
    if hit_cop {
        v = if hit_effect > 20.0 {
            Violation::HeavyContact
        } else if current == Violation::None {
            Violation::LightContact
        } else {
            current
        };
    }
    // an existing pursuit only escalates on a heavy hit
    if current != Violation::None && v != Violation::HeavyContact {
        return current;
    }
    if speed.abs() > SPEED_LIMIT && v == Violation::None {
        v = Violation::Speeding;
    }
    // **Both** arms of 0x403A72, read to the instruction 2026-09-23. `[0x5E7544]` (the alternate
    // lane-width mode) picks between them:
    //
    // * set — 0x403A7B multiplies the speed by `[0x657430] ? -1 : +1` and 0x403AA0 keeps only the
    //   arm where the product is **below** zero: the car is making progress against the race;
    // * clear — 0x403AC1 takes the product through `[0x5E753C]`, which 0x40E680 sets to the
    //   constant **1**, so it is the plain `car+0xEC0`; then 0x403ADE pairs its sign with the
    //   **side of the centreline** the car is on: forward (`>= 0`) needs `lane < 7`, backwards
    //   needs `lane >= 7`. `lane_of` bases the negative side at 6 and the positive at 7, so the
    //   test is "the side does not match the direction of travel" — the oncoming lanes — and it
    //   holds in either race direction, `car+0xEC0` being signed by the slice's own forward.
    //
    // Both then need **more** than 4 m/s (`[0x5B2404]`, 0x403B00 / 0x403B33: `test ah, 0x41` is
    // *below or equal* and the `jne` jumps past the type store). This port had only the `one_way`
    // arm, so on every track where that flag is clear no wrong-way offence was ever raised.
    let wrong_way = if one_way {
        route_dir * speed < 0.0
    } else if speed >= 0.0 {
        lane < 7
    } else {
        lane >= 7
    };
    if wrong_way && speed.abs() > 4.0 && v == Violation::None {
        v = Violation::WrongWay;
    }
    v
}

/// `AIHigh_Perp::update` 0x4059C0, the perp's own AI tick (the tail of `tickHeat` 0x405D70, run for
/// the human every tick while any cop exists). Once the perp has finished (`car+0xB50 >= 2`,
/// 0x4059D1) it resets the heat and **clears the violation** (`+0x78 = 0`, 0x405A28) — on every tick
/// from then on — and does not call `check`; otherwise it runs `check` 0x403990 (`check`, here
/// [`check_violation`]). That store is the only way a violation ever goes back to clean
/// (`traffic.md` §17.5c). The heat itself has no reader (§17.2b) and is not modelled.
pub fn perp_update(current: Violation, finished: bool, check: impl FnOnce(Violation) -> Violation) -> Violation {
    if finished {
        Violation::None
    } else {
        check(current)
    }
}

/// The `.cop` records plus the state `CopManager` keeps about them.
#[derive(Debug, Clone)]
pub struct CopManager {
    pub file: CopFile,
    /// `mgr+0x198` — the tick each record was last handed out
    last_use: Vec<i64>,
    /// `AIHigh_Perp+0x88`, one per perp (index into the racers) — the marker's last position, a
    /// **raw slice index** (0x40554E / 0x405554 store 0x4817D0's result as it is). The ctor 0x405CBB
    /// seeds it with the perp's own slice `car+8`, 31 slices behind the first marker, so the first
    /// query sweeps too far and only moves it; the port seeds it the first time it sees the perp
    /// ([`CopManager::seed_cursor`], from [`Police::set_racers`]), or at that first query.
    cursors: Vec<Option<i32>>,
    /// `mgr+0x328[perpIndex]` — the per-perp index into the sorted records that `0x40E4B0` walks.
    /// `CopManager::load` 0x40E3B0 zeroes all nine (0x40E3D3…0x40E3E2).
    rec_cursors: Vec<usize>,
}

impl CopManager {
    pub fn new(mut file: CopFile) -> Self {
        // 0x40E416: `qsort(recs, count, 4, 0x40E5F0)`, ascending `rec+0x04` — `0x40E4B0`'s walk
        // depends on it. The parser already sorts; a hand-built file (the tests) may not be.
        file.records.sort_by_key(|r| r.slice());
        let n = file.records.len();
        Self { file, last_use: vec![-SPAWN_COOLDOWN; n], cursors: Vec::new(), rec_cursors: Vec::new() }
    }

    fn cursor_mut(&mut self, perp: usize) -> &mut Option<i32> {
        if self.cursors.len() <= perp {
            self.cursors.resize(perp + 1, None);
        }
        &mut self.cursors[perp]
    }

    /// The perp ctor's `+0x88 = car+8` (0x405CBB): the marker cursor starts at the perp's own slice.
    /// A cursor already set is left alone. MEDIUM: the port seeds it on the first tick it sees the
    /// racer (on the grid); where the ctor runs against the grid placement is not read.
    pub fn seed_cursor(&mut self, perp: usize, slice: usize) {
        self.cursor_mut(perp).get_or_insert(slice as i32);
    }

    /// `0x40E4B0(mgr, perpIndex, slice)`, read to the instruction (0x40E4B0…0x40E591). The perp's
    /// record cursor `mgr+0x328[perp]` walks the slice-sorted records: **up** while the record under
    /// it is below `slice` (0x40E4DC…0x40E519, stopping at `count − 1`), else **down** while it is
    /// above (0x40E526…0x40E55D, stopping at 0) — neither when there is one record. The record it
    /// stops on must sit **exactly** at `slice` (0x40E56A), be off its cooldown (`gGameTick −
    /// lastUse <= 0xA00` rejects, 0x40E57E) and not be type 2 (0x40E586); anything else is −1.
    /// Only the record under the cursor is looked at, so of two records at one slice the one the
    /// walk reaches first is the only candidate (no shipped file has two at one slice).
    fn record_at(&mut self, perp: usize, slice: i32, tick: i64) -> Option<usize> {
        let n = self.file.records.len();
        if n == 0 {
            return None;
        }
        if self.rec_cursors.len() <= perp {
            self.rec_cursors.resize(perp + 1, 0);
        }
        let key = |k: usize| self.file.records[k].slice();
        let mut cur = self.rec_cursors[perp];
        if key(cur) < slice {
            if n > 1 {
                while cur != n - 1 {
                    cur += 1;
                    if key(cur) >= slice {
                        break;
                    }
                }
            }
        } else if key(cur) > slice && n > 1 {
            while cur != 0 {
                cur -= 1;
                if key(cur) <= slice {
                    break;
                }
            }
        }
        self.rec_cursors[perp] = cur;
        if key(cur) != slice || tick - self.last_use[cur] <= SPAWN_COOLDOWN {
            return None;
        }
        if matches!(self.file.records[cur], CopRecord::Point { kind: 2, .. }) {
            return None;
        }
        Some(cur)
    }

    pub fn is_empty(&self) -> bool {
        self.file.records.is_empty()
    }

    /// `p = 16 / nRecords`, doubled for a clean perp and forced to certainty on the hardest skill
    /// (§25.5). With the shipped files that is 0.76 on autobahn and ≥ 1 everywhere else. The skill
    /// test is `cmp [0x6573F4], 2; jne` (0x4054EB): exactly 2, nothing above it.
    pub fn spawn_probability(&self, clean: bool, skill: u8) -> f32 {
        if skill == 2 {
            return 1.0;
        }
        let n = self.file.records.len().max(1) as f32;
        (16.0 / n) * if clean { 2.0 } else { 1.0 }
    }

    /// `0x405440` — the spawn-point search. Returns the index of a usable record, having stamped its
    /// cooldown. `humans` are the slices of the human cars, for the 172 m veto.
    ///
    /// `seen` is `pai+0x70[role]`, the cops that have claimed this perp ([`Police::claims`]), and
    /// it does two things: `if seen >= lim: continue` caps how many come out — `lim` is 2 for a
    /// wanted perp ([`HEAT_LIM_ROLE0`]) and, for a clean one, 1 only while no cop is inside
    /// [`CLEAN_PERP_CLEARANCE`] (`cop_near`, which is `0x403120(perp.slice, 500.0) == 0`) — and
    /// `seen != 0` then makes reinforcements free of the probability roll (0x4055FA). A clean
    /// perp is never claimed, so for it the 500 m clearance is the only cap.
    ///
    /// This is the query for the human (perp 0) with a fixed roll; the real pass over every perp is
    /// [`CopManager::find_spawn_for`] through [`Police::tick`].
    #[allow(clippy::too_many_arguments)]
    pub fn find_spawn(&mut self, table: &SliceTable, perp_slice: usize, perp_speed: f32, tick: i64, roll01: f32, clean: bool, skill: u8, humans: &[usize], seen: usize, cop_near: bool) -> Option<usize> {
        let r16 = (roll01 * 65536.0).clamp(0.0, 65535.0) as u32;
        self.find_spawn_for(table, 0, perp_slice, perp_speed, tick, &mut || r16, clean, skill, humans, seen, cop_near)
    }

    /// `0x405440`'s body for one perp (index `perp` into the racers, whose own cursor `+0x88` it
    /// moves). `rand16` is the shared LCG (`rng::rand16`), stepped once for **every** usable record
    /// the sweep finds, before the human veto (0x4055BA…0x4055DC, then 0x4055EE), and compared as
    /// `(r · 100) >> 16 < (int)(p · 100)` (0x4055A8…0x405604).
    #[allow(clippy::too_many_arguments)]
    pub fn find_spawn_for(&mut self, table: &SliceTable, perp: usize, perp_slice: usize, perp_speed: f32, tick: i64, rand16: &mut dyn FnMut() -> u32, clean: bool, skill: u8, humans: &[usize], seen: usize, cop_near: bool) -> Option<usize> {
        if self.file.records.is_empty() {
            return None;
        }
        // 0x4055F1's gate, before the cursor is touched (0x405600 writes `pai+0x88` after it)
        let lim = if clean {
            if HEAT_LIM_ROLE0 > 0 && !cop_near { 1 } else { 0 }
        } else {
            HEAT_LIM_ROLE0
        };
        if seen >= lim {
            return None;
        }
        // 0x40551A: `car+0xEC0` below zero → −1, else +1; the marker is 0x4817D0(slice, 31·dir, −1),
        // through the junction graph (the port's table is reversed for a backwards race, so `+1`
        // steps along the race either way — the same slice the original reaches)
        let dir = if perp_speed < 0.0 { -1 } else { 1 };
        let marker = table.add(perp_slice, SPAWN_LEAD_SLICES * dir) as i32;
        // 0x40554E…0x405588, on **raw slice indices**: `lo = min(old, marker)`, `hi = max(…)`, the
        // cursor takes the marker, and the sweep is searched only for `0 < hi − lo < 6`. No
        // wrapping: across a lap seam or a branch the difference is large and the query is skipped
        // (the next one starts from the new marker).
        let cursor = self.cursor_mut(perp);
        let old = *cursor.get_or_insert(perp_slice as i32);
        *cursor = Some(marker);
        let (lo, hi) = if old < marker { (old, marker) } else { (marker, old) };
        if lo >= hi || hi - lo >= MAX_SWEEP {
            return None;
        }
        // `(int)(f · 100)` by `fistp` (0x4055B7) — the FPU's round-to-nearest, not a truncation
        // (HIGH: the control word is the CRT default; nothing in the race sets the rounding mode)
        let threshold = (self.spawn_probability(clean, skill) * 100.0).round_ties_even() as i32;
        // 0x40558E…0x40560C: `s = lo … hi − 1`, ascending whichever way the perp goes
        for s in lo..hi {
            let Some(k) = self.record_at(perp, s, tick) else { continue };
            let r100 = ((rand16() * 100) >> 16) as i32;
            // 0x407CC0: a human too close to the spawn point would see the car appear —
            // `|0x40E800(human.slice, s)| < 172.0`, on raw slice indices
            if humans.iter().any(|&h| slice_gap(table, h as i32, s).abs() < HUMAN_VETO) {
                continue;
            }
            if seen != 0 || r100 < threshold {
                // 0x40E5A0: stamp the record and hand it out
                self.last_use[k] = tick;
                return Some(k);
            }
        }
        None
    }

    /// Where and how a record places its car (§25.10 step 5): the lateral offset from the centreline
    /// and whether the car is rolling. `route_dir` is `REVERSE ? −1 : +1`.
    ///
    /// A type-3 record gives `None`: in the original it starts `AIHigh_Cop` state 5 with an
    /// `AIState_Offroad` (0x404155 → ctor 0x40DBA0, update 0x40DCB0, trigger 0x40DC50), a cop waiting
    /// at the record's own pose. No shipped `.cop` file has a type-3 record (all 90 are type 1), so
    /// that path never runs and is not ported — `RE/AUDIT.md` §C.
    pub fn placement(&self, table: &SliceTable, index: usize, route_dir: f32) -> Option<(usize, f32, bool)> {
        let r = self.file.records.get(index)?;
        let CopRecord::Point { slice, moving, .. } = *r else { return None };
        let slice = (slice.max(0) as usize).min(table.slices.len().saturating_sub(1));
        let sl = &table.slices[slice];
        let (width, lanes) = if route_dir > 0.0 { (sl.lane_width_b, sl.lanes_b as f32) } else { (sl.lane_width_a, sl.lanes_a as f32) };
        let lateral = if moving {
            // the centre of the outermost lane on the race-direction side
            width * (lanes - 0.5) * route_dir.signum()
        } else {
            // 2 m outside the road edge
            (width * lanes + 2.0) * route_dir.signum()
        };
        Some((slice, lateral, moving))
    }
}

/// `0x40E800(a, b)`, read to the instruction (0x40E800…0x40E852): the **raw** slice-index
/// difference `a − b`, wrapped by the track's total slice count `[track+0x00]` (every section,
/// branches included — `race-logic.md` §2): `d −= n` when `d > 0 && d > n/2`, `d += n` when
/// `d < 0 && d < −(n/2)` (`n/2` by `cdq; sub; sar`, i.e. truncated), then `× 6.0` (`[0x5B2460]`).
/// No lap route is involved, so a branch slice is as far from a main-lap slice as their indices are.
pub fn slice_gap(table: &SliceTable, a: i32, b: i32) -> f32 {
    let n = table.slices.len() as i32;
    let half = n / 2;
    let mut d = a - b;
    if d > 0 {
        if d > half {
            d -= n;
        }
    } else if d < 0 && d < -half {
        d += n;
    }
    d as f32 * 6.0
}

/// Signed track distance in metres between two slices, wrapped to half a lap.
pub fn track_distance(table: &SliceTable, a: usize, b: usize) -> f32 {
    let n = table.lap_len as i32;
    let (pa, pb) = (table.route_pos.get(a).copied().unwrap_or(0) as i32, table.route_pos.get(b).copied().unwrap_or(0) as i32);
    let mut d = pa - pb;
    if d > n / 2 {
        d -= n;
    } else if d < -n / 2 {
        d += n;
    }
    d as f32 * 6.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::{Section, Slice, SLICE_SPACING};
    use glam::Vec3;
    use nfs_formats::cop::CopRecord;

    fn ring(n: usize) -> SliceTable {
        let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
        let slices = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let forward = Vec3::new(-a.sin(), 0.0, a.cos());
                Slice {
                    centre: Vec3::new(a.cos() * r, 0.0, a.sin() * r),
                    up: Vec3::Y,
                    forward,
                    lateral: Vec3::Y.cross(forward),
                    lane_mask: 0,
                    lanes_a: 2,
                    lanes_b: 2,
                    width_a: 11.0,
                    width_b: 11.0,
                    lane_width_a: 4.0,
                    lane_width_b: 4.0,
                }
            })
            .collect();
        SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
    }

    fn mgr(slices: &[i32], moving: bool) -> CopManager {
        CopManager::new(CopFile { records: slices.iter().map(|&s| CopRecord::Point { kind: 1, slice: s, moving }).collect() })
    }

    /// Test setup: put a just-deployed cop straight into a lit state-4 chase, skipping the two
    /// ticks (placement, acquire) the original takes to get there.
    fn chase_now(p: &mut Police, i: usize) {
        p.cars[i].placed = false;
        p.build_chase(i);
        p.cars[i].lights = true;
    }

    #[test]
    fn the_speed_limit_is_flat_and_wrong_way_is_the_side_of_the_centreline() {
        // 22 m/s = 79.2 km/h, everywhere. Lane 7 is the first on the positive side, which a car
        // going forward is entitled to.
        assert_eq!(check_violation(Violation::None, 21.0, 1.0, false, 7, false, 0.0), Violation::None);
        assert_eq!(check_violation(Violation::None, 23.0, 1.0, false, 7, false, 0.0), Violation::Speeding);
        // the two-way arm, 0x403ADE: forward on the oncoming side, or backwards on its own side
        assert_eq!(check_violation(Violation::None, 10.0, 1.0, false, 6, false, 0.0), Violation::WrongWay);
        assert_eq!(check_violation(Violation::None, -10.0, 1.0, false, 7, false, 0.0), Violation::WrongWay);
        assert_eq!(check_violation(Violation::None, -10.0, 1.0, false, 6, false, 0.0), Violation::None, "backwards on the oncoming side is its own side");
        // and it needs **more** than 4 m/s, on either arm
        assert_eq!(check_violation(Violation::None, 4.0, 1.0, false, 6, false, 0.0), Violation::None);
        assert_eq!(check_violation(Violation::None, -4.0, 1.0, true, 7, false, 0.0), Violation::None);
        // the one-way arm ignores the lane and asks only for progress against the race
        assert_eq!(check_violation(Violation::None, -10.0, 1.0, true, 6, false, 0.0), Violation::WrongWay);
        assert_eq!(check_violation(Violation::None, 10.0, 1.0, true, 6, false, 0.0), Violation::None);
        // hitting a cop: light under an impact of 20, heavy above it
        assert_eq!(check_violation(Violation::None, 5.0, 1.0, false, 7, true, 10.0), Violation::LightContact);
        assert_eq!(check_violation(Violation::None, 5.0, 1.0, false, 7, true, 30.0), Violation::HeavyContact);
        // an existing pursuit only escalates on a heavy hit
        assert_eq!(check_violation(Violation::Speeding, 30.0, 1.0, false, 7, false, 0.0), Violation::Speeding);
        assert_eq!(check_violation(Violation::Speeding, 30.0, 1.0, false, 7, true, 30.0), Violation::HeavyContact);
    }

    /// §27.10 `0x40DA00`: the bias and the stuck timer hang on the lane corridor at both ends.
    /// `0x40E670` reads slice 1's lanesA in the track's own frame, which a reversed table keeps in
    /// `lanes_b`.
    #[test]
    fn the_one_way_flag_is_read_in_the_tracks_own_frame() {
        let mut t = ring(40);
        t.slices[1].lanes_a = 0;
        assert!(one_way_road(&t));
        let back = t.reversed();
        assert_eq!((back.slices[1].lanes_a, back.slices[1].lanes_b), (2, 0));
        assert!(one_way_road(&back), "the original's slice 1 still has no A lanes");
        t.slices[1].lanes_a = 2;
        t.slices[1].lanes_b = 0;
        assert!(!one_way_road(&t) && !one_way_road(&t.reversed()));
    }

    #[test]
    fn the_lane_bias_follows_the_corridor_at_both_ends() {
        let mut t = ring(400);
        // lanes 4..9 exist everywhere (two lanes a side)
        let all = (4..=9).fold(0u32, |m, l| m | crate::lanes::lane_bit(l));
        for s in t.slices.iter_mut() {
            s.lane_mask = all;
        }
        // whole at the target's slice: clear, no push
        assert_eq!(lane_bias(&t, 10, 5, 20, 8, -60.0), LaneBias::Clear);
        // broken at the target's slice only: push toward it
        t.slices[20].lane_mask &= !crate::lanes::lane_bit(6);
        assert_eq!(lane_bias(&t, 10, 5, 20, 8, -60.0), LaneBias::Toward);
        // broken at both: the stuck timer counts
        t.slices[10].lane_mask &= !crate::lanes::lane_bit(7);
        assert_eq!(lane_bias(&t, 10, 5, 20, 8, -60.0), LaneBias::Blocked);
        // past 500 m, or a lane off the road, and nothing happens at all
        assert_eq!(lane_bias(&t, 10, 5, 20, 8, 501.0), LaneBias::Skip);
        assert_eq!(lane_bias(&t, 10, 3, 20, 8, -60.0), LaneBias::Skip);
    }

    #[test]
    fn a_spawn_point_is_taken_once_the_marker_sweeps_past_it() {
        let t = ring(400);
        let mut m = mgr(&[100, 200, 300], true);
        // the first query only arms the cursor: the marker sits 31 slices ahead, so at slice 91
        assert_eq!(m.find_spawn(&t, 60, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
        // a five-slice step sweeps 92..96 — nothing there
        assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
        // the next one sweeps 97..101 and passes the record at 100
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), Some(0));
        // the record is then on a 40 s cooldown, so sweeping it again finds nothing
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), None, "`<= 0xA00` still holds 2560 ticks on");
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5001 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), Some(0), "and is free again the tick after");
        // a jump of six slices or more is too big a sweep to search
        m.cursors = vec![Some(90)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 9000 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), None);
    }

    /// 0x40E3B0 starts every `lastUse` at −2560, so `gGameTick − lastUse <= 0xA00` holds at tick 0
    /// and at no later tick: the points are live from tick 1, not after 40 s.
    #[test]
    fn the_initial_cooldown_covers_tick_zero_only() {
        let t = ring(400);
        let mut m = mgr(&[100], true);
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 0, 0.0, false, 0, &[], 1, true), None, "tick 0: 0 + 2560 <= 2560");
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 1, 0.0, false, 0, &[], 1, true), Some(0), "tick 1: 2561 > 2560");
    }

    /// §25.5's `if seen >= lim: continue`, with the pinned heat record's `lim[0] = 2`.
    #[test]
    fn only_two_cops_are_ever_assigned_to_a_wanted_perp() {
        let t = ring(400);
        // records at 100 and 105; the marker runs 31 slices ahead of the perp
        let mut m = mgr(&[100, 105], true);
        // wanted: the first comes out on the roll, the second free of it (`seen != 0`), a third never
        assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, false, 0, &[], 0, false), None, "arms the cursor at 96");
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 0, false), Some(0), "sweeps 97..101");
        assert_eq!(m.find_spawn(&t, 75, 20.0, 5000, 1.0, false, 0, &[], 1, true), Some(1), "a reinforcement skips the roll");
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 2, true), None, "lim[0] is 2");
        // clean: one, and only while no cop is inside 500 m (0x403120)
        let mut m = mgr(&[100], true);
        assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, true, 0, &[], 0, false), None, "arms the cursor");
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, true, 0, &[], 0, true), None, "a cop is already close by");
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, true, 0, &[], 0, false), Some(0));
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000 + SPAWN_COOLDOWN, 0.0, true, 0, &[], 1, false), None, "lim is 1 for a clean perp (a clean perp is never claimed in practice: the 500 m test is the cap)");
    }

    #[test]
    fn a_human_next_to_the_point_vetoes_it() {
        let t = ring(400);
        let mut m = mgr(&[100], true);
        m.cursors = vec![Some(96)];
        // a human 20 slices (120 m) from slice 100 is inside the 172 m veto
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[120], 1, true), None);
        // 40 slices (240 m) away is outside it
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[140], 1, true), Some(0));
    }

    #[test]
    fn the_veto_distance_wraps_raw_indices_by_the_total_slice_count() {
        // a 400-slice lap plus a 100-slice branch section (indices 400…499): `[track+0]` is 500
        let mut t = ring(400);
        let extra: Vec<Slice> = t.slices[..100].to_vec();
        t.slices.extend(extra);
        t.route_pos.extend(0..100u32);
        assert_eq!(t.lap_len, 400);
        // 10 − 490 = −480 wraps by 500 to 20 slices: 120 m, inside the 172 m veto (wrapping by the
        // lap's 400 would give 80 slices, 480 m)
        assert_eq!(slice_gap(&t, 10, 490), 120.0);
        assert!(slice_gap(&t, 10, 490).abs() < HUMAN_VETO);
        // 250 is exactly half: not wrapped (`d > n/2`), −250 neither (`d < −(n/2)`)
        assert_eq!(slice_gap(&t, 300, 50), 1500.0);
        assert_eq!(slice_gap(&t, 50, 300), -1500.0);
        assert_eq!(slice_gap(&t, 301, 50), -1494.0);
        // an odd count halves by truncation: n = 401, n/2 = 200
        let mut u = ring(400);
        u.slices.push(u.slices[0]);
        assert_eq!(slice_gap(&u, 200, 0), 1200.0);
        assert_eq!(slice_gap(&u, 201, 0), -1200.0);
    }

    #[test]
    fn the_placement_follows_the_moving_flag() {
        let t = ring(200);
        let rolling = mgr(&[50], true);
        let (s, lat, moving) = rolling.placement(&t, 0, 1.0).unwrap();
        assert_eq!(s, 50);
        assert!(moving);
        assert_eq!(lat, 4.0 * 1.5, "the centre of the outer lane");
        let parked = mgr(&[50], false);
        let (_, lat, moving) = parked.placement(&t, 0, 1.0).unwrap();
        assert!(!moving);
        assert_eq!(lat, 4.0 * 2.0 + 2.0, "2 m outside the road edge");
        // driving the track backwards puts it on the other side
        assert!(parked.placement(&t, 0, -1.0).unwrap().1 < 0.0);
    }

    #[test]
    fn the_pursue_ladder_caps_by_distance_and_level() {
        // far away the cop is allowed 60 m/s, on top of the player it is told to stop
        assert_eq!(pursue_cap(0, 200.0), 60.0);
        assert_eq!(pursue_cap(0, 120.0), 50.0);
        assert_eq!(pursue_cap(0, 5.0), 0.0);
        // the hardest level is faster everywhere
        assert!(pursue_cap(2, 200.0) > pursue_cap(0, 200.0));
        assert!(pursue_cap(2, 7.0) > pursue_cap(0, 7.0));
        // and the middle level closes the last stretch more carefully than the easiest
        assert!(pursue_cap(1, 15.0) < pursue_cap(0, 15.0));
    }

    #[test]
    fn a_chase_gives_up_past_216_metres() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)], 0, false);
        assert!(p.deploy(&t, 0, 0, 1000));
        assert!(p.cars[0].active && !p.cars[0].lights && !p.cars[0].chasing, "state 1 places a cop in state 2, unlit");
        chase_now(&mut p, 0);
        // a target right beside it keeps the chase alive
        let near = ChaseTarget { position: p.cars[0].position, slice: 50, lateral: 0.0, lane: 7, speed: 30.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 1000, &near, &[]);
        assert!(p.cars[0].active);
        // 400 m away it gives up
        let far = ChaseTarget { position: t.slices[250].centre, slice: 250, ..near };
        p.update(&t, 1001, &far, &[]);
        assert!(!p.cars[0].active);
        assert!(!p.cars[0].lights);
    }

    /// 0x407690 → 0x49B090 and the drop: the deployed cop's origin `car+0x330` is `car+0x3AC`
    /// above the model's ground, and the model (`draw_origin`, 0x43A410) stands on the road —
    /// at deploy and after the 64 Hz step.
    #[test]
    fn a_deployed_cop_stands_on_the_road_with_its_origin_half_the_box_up() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: false }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.63, 2.1), 1500.0, 0)], 0, false);
        assert!(p.deploy(&t, 0, 0, 1000));
        let c = &p.cars[0];
        let road = t.slices[50].centre.y;
        assert!((c.position.y - (road + crate::traffic::RIDE_HEIGHT + 0.63)).abs() < 1e-4, "origin {}", c.position.y);
        assert!((c.draw_origin().y - (road + crate::traffic::RIDE_HEIGHT)).abs() < 1e-4, "model ground {}", c.draw_origin().y);
        p.integrate(&t, 1.0 / 64.0);
        let c = &p.cars[0];
        assert!((c.draw_origin().y - (t.slices[c.slice].centre.y + crate::traffic::RIDE_HEIGHT)).abs() < 1e-4);
    }

    /// `Susp_update` 0x499A70 through the cop's trackers: a probe stands on a patch only when it
    /// has a surface nibble and lies within 4 m of the car, else on the car tracker's kept plane —
    /// so a cop that runs off a drivable slope onto a steep face with no surface (a cliff) keeps the
    /// slope's plane and stays upright, where the old any-triangle probe stood it on its side.
    #[test]
    fn a_cop_off_the_road_keeps_the_last_drivable_plane() {
        use nfs_physics::collision::{Triangle, TriangleGrid};
        let h = |x: f32| 0.1 * x;
        let (p0, p1, p2, p3) = (Vec3::new(-40.0, h(-40.0), -40.0), Vec3::new(40.0, h(40.0), -40.0), Vec3::new(40.0, h(40.0), 40.0), Vec3::new(-40.0, h(-40.0), 40.0));
        // the drivable slope for x < 40, a steep face with surface word 0 beyond it
        let (q1, q2) = (Vec3::new(44.0, h(40.0) + 20.0, -40.0), Vec3::new(44.0, h(40.0) + 20.0, 40.0));
        let tris = vec![Triangle::new(p0, p1, p2, 1), Triangle::new(p0, p2, p3, 1), Triangle::new(p1, q1, q2, 0), Triangle::new(p1, q2, p2, 0)];
        let grid = TriangleGrid::build(tris, 10.0);
        let t = ring(400);
        let mut p = Police::new(CopFile { records: vec![] }, vec![CopCar::new(0, Vec3::new(0.9, 0.63, 2.1), 1500.0, 0)], 0, false);
        let c = &mut p.cars[0];
        c.active = true;
        c.position = Vec3::new(30.0, h(30.0) + c.rest_height(), 0.0);
        c.velocity = Vec3::new(20.0, 0.0, 0.0);
        let n = Vec3::new(-0.1, 1.0, 0.0).normalize();
        for _ in 0..40 {
            p.integrate_on(&t, 1.0 / 64.0, Some(&grid));
            let c = &p.cars[0];
            assert!(c.orientation.row(1).dot(n) > 0.999, "up {:?} at x {}", c.orientation.row(1), c.position.x);
        }
        let c = &p.cars[0];
        assert!(c.position.x > 42.0, "ran onto the face: x {}", c.position.x);
        // on the extension of the slope's plane, not on the face (or the old slice-height fallback)
        let want = h(c.position.x) + c.rest_height() / n.y;
        assert!((c.position.y - want).abs() < 1e-2, "y {} vs {}", c.position.y, want);
        assert_eq!(c.airborne_count, 0);
    }

    /// 0x40854C / 0x4085EC: a cop on its side with `car+0x418 == 0`, or with a zero target speed,
    /// is only damped by 0.94 and does not steer.
    #[test]
    fn a_cop_on_its_side_or_without_a_target_speed_is_only_damped() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)], 0, false);
        p.deploy(&t, 0, 0, 0);
        let v0 = p.cars[0].velocity;
        p.cars[0].orientation = Mat3::from_rotation_z(1.4) * p.cars[0].orientation;
        p.cars[0].angular_velocity = Vec3::new(0.0, 0.5, 0.0);
        p.drive(&t, 100);
        assert!((p.cars[0].velocity - v0 * 0.94).length() < 1e-4);
        assert!((p.cars[0].angular_velocity.y - 0.49).abs() < 1e-5);
        // once hit (`car+0x418` stepped) the tipped car drives again
        p.cars[0].airborne_count = 1;
        let v1 = p.cars[0].velocity;
        p.drive(&t, 101);
        assert!((p.cars[0].velocity - v1 * 0.94).length() > 1e-3);
        // upright again, a zero target speed damps
        let mut p2 = Police::new(CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] }, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)], 0, false);
        p2.deploy(&t, 0, 0, 0);
        p2.cars[0].target_speed = 0.0;
        let v2 = p2.cars[0].velocity;
        p2.drive(&t, 100);
        assert!((p2.cars[0].velocity - v2 * 0.94).length() < 1e-4);
    }

    #[test]
    fn the_dispatch_picks_the_documented_behaviour() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)], 0, false);
        p.deploy(&t, 0, 0, 0);
        chase_now(&mut p, 0);
        // a long way behind and not closing: catch up, flat out
        p.cars[0].speed = 30.0;
        let far = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 30.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 100, &far, &[]);
        assert_eq!(p.cars[0].mode, Some(ChaseMode::CatchUp), "gap {} m", track_distance(&t, 10, 40));
        // right behind a slow car: pursue, and the ladder caps the speed well under the 100 m/s base
        p.cars[0].active = true;
        p.cars[0].slice = 38;
        p.cars[0].position = t.slices[38].centre;
        p.cars[0].speed = 5.0;
        let slow = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 2.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 101, &slow, &[]);
        assert!(matches!(p.cars[0].mode, Some(ChaseMode::Pursue { .. })), "{:?}", p.cars[0].mode);
        assert!(p.cars[0].target_speed.abs() < 60.0, "capped to {}", p.cars[0].target_speed);
    }

    #[test]
    fn close_quarters_drives_the_formation_slot() {
        // §27.8: slot 0 is 8 m ahead of the target, so a cop behind it chases at 1.4x the target's
        // speed, and once it is inside the slot it eases off instead.
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)], 0, false);
        p.deploy(&t, 0, 0, 0);
        chase_now(&mut p, 0);
        let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };

        // behind the target, travelling the same way, close enough not to be a catch-up
        p.cars[0].slice = 36;
        p.cars[0].position = t.slices[36].centre;
        p.cars[0].speed = 20.0;
        p.cars[0].lateral = 0.0;
        p.update(&t, 200, &tgt, &[]);
        assert_eq!(p.cars[0].mode, Some(ChaseMode::CloseQuarters));
        assert_eq!(p.cars[0].rank, 0);
        assert!((p.cars[0].target_speed - 1.4 * 20.0).abs() < 1e-3, "behind the slot it chases: {}", p.cars[0].target_speed);

        // sitting in the slot ahead of the target: the pace comes off the distance table
        p.cars[0].slice = 42;
        p.cars[0].position = t.slices[42].centre;
        p.cars[0].ram_until = 0;
        p.update(&t, 400, &tgt, &[]);
        assert_eq!(p.cars[0].mode, Some(ChaseMode::CloseQuarters));
        assert!(p.cars[0].target_speed.abs() < 1.4 * 20.0, "inside the slot it eases off: {}", p.cars[0].target_speed);
        assert!(p.cars[0].target_speed.abs() >= 5.0, "never below the 5 m/s floor: {}", p.cars[0].target_speed);
        assert_eq!(p.cars[0].requested_dir, 1.0, "close quarters never asks for a U-turn");
    }

    #[test]
    fn the_close_quarters_tables_are_the_originals() {
        assert_eq!(close_quarters_factor(10.0), 0.95);
        assert_eq!(close_quarters_factor(40.0), 0.8);
        assert_eq!(close_quarters_factor(120.0), 0.75);
        assert_eq!(close_quarters_factor(300.0), 0.6);
        // slot 0 is ahead, slot 1 ahead and to the left, slot 2 beside, slots 3..5 behind
        assert_eq!(sector(FORMATION[0][0][2], -2.2, 2.2), 1);
        assert_eq!(sector(FORMATION[0][0][0], -0.9, 0.9), 0);
        assert_eq!(sector(FORMATION[0][1][0], -0.9, 0.9), -1);
        assert_eq!(sector(FORMATION[0][2][0], -0.9, 0.9), 1);
        assert_eq!(sector(FORMATION[0][2][2], -2.2, 2.2), 0);
        assert_eq!(sector(FORMATION[0][3][2], -2.2, 2.2), -1);
    }

    #[test]
    fn a_cop_that_spawns_while_the_perp_is_clean_patrols() {
        // §17.4: a cop deployed with no wanted perp drives the road like traffic (states 2/3) and
        // only becomes a chaser on the first tick of its own that finds the perp wanted (0x405220).
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        assert!(p.deploy_as(&t, 0, 0, 1000, false));
        assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights, "patrolling, lights off");
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 1002, &tgt, &[]);
        assert_eq!(p.cars[0].mode, None, "a patrol car runs no chase behaviour");
        assert!(p.cars[0].target_speed.abs() < 30.0, "traffic pace, not the pursuit base: {}", p.cars[0].target_speed);
        p.update(&t, 1004, &tgt, &[]);
        assert!(!p.cars[0].chasing, "a clean perp is never acquired");
        // the perp offends: the cop finds out in its own next tick (state 2's 0x405220)
        p.set_wanted(true);
        assert!(!p.cars[0].chasing, "set_wanted only marks the perp");
        p.update(&t, 1006, &tgt, &[]);
        assert!(p.cars[0].chasing && p.cars[0].mode.is_some(), "state 2 built the chase and ran it");
        assert!(!p.cars[0].lights, "the building tick's update runs with the lights off");
        p.update(&t, 1008, &tgt, &[]);
        assert!(p.cars[0].lights, "state 4's head lights it on the next tick");
    }

    /// `car+0xEAC` on patrol: `AIState_Normal` (0x40B5A0) never writes it, and 0x408360 grants it
    /// below 8.889 m/s before the drive chain runs (0x4083B6…0x4083CF).
    #[test]
    fn a_patrol_keeps_the_requested_direction_and_the_grant_waits_for_8_889() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        assert!(p.deploy_as(&t, 0, 0, 1000, false));
        assert_eq!((p.cars[0].dir, p.cars[0].requested_dir), (1.0, 1.0), "0x407890: car+0xEB0 = car+0xEAC = side");
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        // too fast to turn round: the request stands and is not granted
        p.cars[0].velocity = p.cars[0].orientation.row(2) * 20.0;
        p.cars[0].requested_dir = -1.0;
        p.update(&t, 1002, &tgt, &[]);
        assert_eq!((p.cars[0].dir, p.cars[0].requested_dir), (1.0, -1.0));
        // stopped: granted at the head of this tick's drive
        p.cars[0].velocity = Vec3::ZERO;
        p.update(&t, 1004, &tgt, &[]);
        assert_eq!((p.cars[0].dir, p.cars[0].requested_dir), (-1.0, -1.0));
    }

    /// §17.5b: state 1 places a cop in state 2 (rolling) or 3 (parked) and runs that state's update;
    /// a wanted perp is acquired the next tick (chase update unlit) and the lights come on the tick
    /// after.
    #[test]
    fn a_fresh_cop_takes_two_ticks_to_a_lit_chase() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.set_wanted(true);
        assert!(p.deploy_as(&t, 0, 0, 1000, true));
        p.update(&t, 1000, &tgt, &[]);
        assert!(!p.cars[0].chasing && !p.cars[0].lights && p.cars[0].mode.is_none(), "the placement tick is AIState_Normal");
        p.update(&t, 1002, &tgt, &[]);
        assert!(p.cars[0].chasing && p.cars[0].mode.is_some() && !p.cars[0].lights, "acquired, chased, unlit");
        p.update(&t, 1004, &tgt, &[]);
        assert!(p.cars[0].lights);
    }

    /// §17.5e: the AI pass only writes targets; the drive 0x408360 is a Sc32 entry that runs after
    /// the whole pass, so `Police::ai` leaves every cop's velocity alone and `Police::drive` moves it.
    #[test]
    fn the_ai_pass_writes_targets_and_the_drive_moves_the_car() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let cars: Vec<CopCar> = (0..2).map(|_| CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)).collect();
        let mut p = Police::new(file, cars, 0, false);
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        assert!(p.deploy_as(&t, 0, 0, 1000, false));
        p.cars[0].placed = false;
        let v0 = p.cars[0].velocity;
        p.ai(&t, 1002, &tgt, &[], &[60], &mut || 0xFFFF);
        assert_eq!(p.cars[0].velocity, v0, "the decision does not drive");
        assert!(p.cars[0].target_speed != 0.0);
        p.drive(&t, 1002);
        assert!(p.cars[0].velocity != v0, "the Sc32 drive does");
        assert!(!p.cars[1].active);
    }

    /// §17.5e: 0x406720 runs a cop's update only when `0x414B10` row 0 says it is due — every fourth
    /// 32 Hz tick within 240 m of a racer, every eighth beyond, phased by the slot — and `0x40CD70`
    /// charges the boost the previous update left on by that update's `[0x5E6654]` (4 / 8).
    #[test]
    fn the_ai_pass_gates_a_cop_and_its_counters_step_by_the_update() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        p.slot_base = 3;
        assert!(p.deploy(&t, 0, 0, 900));
        chase_now(&mut p, 0);
        p.cars[0].boost_budget = 100;
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        let ai = |p: &mut Police, tick: u64| p.ai(&t, tick, &tgt, &[], &[60], &mut || 0xFFFF);
        // slot 3: `(0x103 + tick/2) & 3 == 3` at tick 1000; the boost comes on, nothing is charged yet
        ai(&mut p, 1000);
        assert_eq!((p.cars[0].boost_engine, p.cars[0].boost_budget), (BOOST_ENGINE, 100));
        for tick in [1002, 1004, 1006] {
            let before = p.cars[0].clone();
            ai(&mut p, tick);
            assert_eq!(p.cars[0], before, "not due at {tick}");
        }
        ai(&mut p, 1008);
        assert_eq!(p.cars[0].boost_budget, 96, "charged [0x5E6654] = 4 for the update that had it on");
        // beyond 240 m of every racer: 4 Hz, `[0x5E6654]` = 8, due when `(0x103 + tick/2) & 7 == 7`
        p.cars[0].racer_distance = 300.0;
        ai(&mut p, 1012);
        assert_eq!(p.cars[0].boost_budget, 96, "(0x103 + 506) & 7 = 5: not due");
        ai(&mut p, 1016);
        assert_eq!(p.cars[0].boost_budget, 88, "(0x103 + 508) & 7 = 7: due, charged 8");
        // the tracker writes `car+0x78` for a cop in the world
        p.track_racers(&[p.cars[0].position + Vec3::new(30.0, 0.0, 40.0)]);
        assert_eq!(p.cars[0].racer_distance, 40.0 + 0.25 * 30.0);
    }

    /// 0x408360's own gate, row 1 (`traffic::drive_due`): within 160 m of a racer the cop drives on
    /// every 32 Hz tick, one tick's worth; beyond it on every other, phased by its slot, with
    /// `dt` 1/16 — in between its velocity is left alone.
    #[test]
    fn the_cop_drive_is_gated_by_row_1() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        p.slot_base = 4;
        assert!(p.deploy(&t, 0, 0, 900));
        p.cars[0].target_speed = 30.0;
        let mut driven = 0;
        for tick in 1000..1008 {
            let v = p.cars[0].velocity;
            p.drive(&t, tick);
            driven += usize::from(p.cars[0].velocity != v);
        }
        assert_eq!(driven, 8, "near a racer: every 32 Hz tick");
        p.cars[0].racer_distance = 200.0;
        let mut due = Vec::new();
        for tick in 1008..1016 {
            let v = p.cars[0].velocity;
            p.drive(&t, tick);
            if p.cars[0].velocity != v {
                due.push(tick);
            }
        }
        // slot 4: `(0x104 + tick/2) & 1 == 1` when tick/2 is odd
        assert_eq!(due, vec![1010, 1011, 1014, 1015]);
    }

    /// `AIState_Stop` mode 1 (0x40CAB0): a parked cop holds its lateral at zero speed while the perp
    /// is clean, and leaves the stop as soon as it builds a chase.
    #[test]
    fn a_parked_cop_stays_put_until_it_chases() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: false }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        assert!(p.deploy_as(&t, 0, 0, 1000, false));
        assert!(p.cars[0].parked);
        let lat = p.cars[0].lateral;
        for k in 0..4 {
            p.update(&t, 1000 + 2 * k, &tgt, &[]);
            assert_eq!((p.cars[0].target_speed, p.cars[0].lateral_target), (0.0, lat));
        }
        p.set_wanted(true);
        p.update(&t, 1010, &tgt, &[]);
        assert!(p.cars[0].chasing && !p.cars[0].parked && p.cars[0].mode.is_some());
    }

    /// §17.4 state 4 → 9 → 2 → 4: a stuck chase is not given up but restarted through
    /// `AIState_GotoSlice` to the reference car's slice (`[[car+0xED4]+8]`) and one tick of
    /// `AIState_Normal`.
    #[test]
    fn a_stuck_chase_goes_through_goto_slice_and_back() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        p.racers = vec![RacerRef { position: t.slices[60].centre, slice: 60, progress: 1.0, ..Default::default() }];
        assert!(p.deploy(&t, 0, 0, 1000));
        chase_now(&mut p, 0);
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.cars[0].stuck_ticks = STUCK_LIMIT + 1;
        p.cars[0].boost_budget = 3;
        p.update(&t, 1002, &tgt, &[]);
        let c = &p.cars[0];
        assert!(c.active, "the cop is not dropped");
        assert_eq!(c.goto_slice, Some(60), "state 9, to the reference car's slice");
        assert_eq!((c.mode, c.stuck_ticks, c.boost_engine), (None, 0, 1.0), "the chase is gone and its dtor reset the boost");
        assert!(c.lights, "state 9 does not touch the lights");
        // the goal is 60 m off: still driving to it, with no give-up test in state 9
        p.update(&t, 1004, &tgt, &[]);
        assert_eq!(p.cars[0].goto_slice, Some(60));
        // within 10 m of it (`0x40E330(state, 10.0)`): `AIState_Normal`, state 2; the lights stay
        // on until state 2's own head
        p.cars[0].slice = 59;
        p.cars[0].position = t.slices[59].centre;
        p.update(&t, 1004, &tgt, &[]);
        assert!(p.cars[0].goto_slice.is_none() && !p.cars[0].chasing && p.cars[0].lights);
        // the perp is still wanted, so state 2 clears the lights and builds a fresh chase with a
        // fresh budget
        p.update(&t, 1006, &tgt, &[]);
        assert!(p.cars[0].chasing && p.cars[0].mode.is_some() && !p.cars[0].lights, "chasing again, unlit");
        assert!(p.cars[0].boost_budget > 3, "the new chase restarted the budget (less this tick's spend)");
    }

    /// §17.5c: `0x4071E0` measures to the reference car `car+0xED4` — the racer that led when the cop
    /// was placed (0x40DFB0) — not to the target.
    #[test]
    fn the_give_up_distance_is_to_the_leading_racer_latched_at_deploy() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        // the human at slice 60, an opponent ahead of it at slice 80
        p.racers = vec![RacerRef { position: t.slices[60].centre, slice: 60, progress: 1.0, ..Default::default() }, RacerRef { position: t.slices[80].centre, slice: 80, progress: 2.0, ..Default::default() }];
        assert!(p.deploy(&t, 0, 0, 1000));
        assert_eq!(p.cars[0].reference, Some(1), "the leader, not the human");
        chase_now(&mut p, 0);
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 1002, &tgt, &[]);
        assert!(p.cars[0].active);
        // the latch does not follow the race order: the human taking the lead changes nothing
        p.racers[0].progress = 3.0;
        // the opponent pulls 400 m away while the target stays beside the cop: given up
        p.racers[1].position = t.slices[120].centre;
        p.update(&t, 1004, &tgt, &[]);
        assert!(!p.cars[0].active, "0x4071E0 against the latched leader");
    }

    /// §17.5d: `0x405220` walks every racer, but an opponent's violation is never checked, so the
    /// cop takes the human — even with an opponent right beside it — and claims it.
    #[test]
    fn a_cop_never_acquires_an_opponent() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2), CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        let r = |slice: usize| RacerRef { position: t.slices[slice].centre, slice, progress: slice as f32 / 400.0, laps_timed: 0, speed: 20.0 };
        p.set_racers(vec![r(70), r(51)]);
        assert!(p.deploy_as(&t, 0, 0, 1000, false));
        assert!(p.deploy_as(&t, 1, 0, 1000, false));
        let tgt = ChaseTarget { position: t.slices[70].centre, slice: 70, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 1002, &tgt, &[]);
        p.update(&t, 1004, &tgt, &[]);
        assert!(p.cars.iter().all(|c| !c.chasing && c.target.is_none()), "a clean field: nobody to acquire");
        assert_eq!((p.claims(0), p.claims(1)), (0, 0));
        p.set_wanted(true);
        p.update(&t, 1006, &tgt, &[]);
        assert!(p.cars.iter().all(|c| c.chasing && c.target == Some(0)), "the human, not the nearer opponent");
        assert_eq!((p.claims(0), p.claims(1)), (2, 0));
        p.set_wanted(false);
        p.update(&t, 1008, &tgt, &[]);
        assert_eq!(p.claims(0), 0, "the empty-handed 0x405220 releases");
    }

    /// §17.5d: `0x405440` walks the racers from the top of `0x5E8BAC` and an opponent is a clean
    /// perp — so a patrol comes out 31 slices ahead of **it**, one per 500 m, whatever the human does.
    #[test]
    fn a_patrol_spawns_ahead_of_an_opponent() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 233, moving: true }, CopRecord::Point { kind: 1, slice: 43, moving: true }] };
        let cars: Vec<CopCar> = (0..3).map(|_| CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)).collect();
        let mut p = Police::new(file, cars, 0, false);
        let r = |slice: usize, progress: f32| RacerRef { position: t.slices[slice].centre, slice, progress, laps_timed: 0, speed: 20.0 };
        let mut draws = 0;
        let tgt = |slice: usize| ChaseTarget { position: t.slices[slice].centre, slice, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        // the human at 10, the opponent ahead at 200: both cursors start at the racers' own slices
        // (0x405CBB), so the first query's 31-slice sweep is too long and only moves them
        p.set_racers(vec![r(10, 0.025), r(200, 0.5)]);
        p.tick(&t, 5000, &tgt(10), &[], &[10], &mut || { draws += 1; 0 });
        assert_eq!(p.active_count(), 0, "the first query only moves each cursor");
        // both move 3 slices: the opponent is on top of 0x5E8BAC and its marker sweeps 231..233
        // first (`lo … hi − 1`, 0x40558E)
        p.set_racers(vec![r(13, 0.0325), r(203, 0.5075)]);
        p.tick(&t, 5002, &tgt(13), &[], &[13], &mut || { draws += 1; 0 });
        assert!(p.cars[0].active && p.cars[0].slice == 233, "cop 0 at the opponent's record");
        assert!(p.cars[1].active && p.cars[1].slice == 43, "cop 1 at the human's: the opponent has a cop inside 500 m now, the human's cursor has not moved yet");
        assert!(!p.cars[2].active);
        assert_eq!(draws, 2, "one LCG step per usable record");
        // nobody is wanted, so nobody claims — and the opponent is never acquired
        let tgt = tgt(13);
        p.update(&t, 5004, &tgt, &[]);
        p.update(&t, 5006, &tgt, &[]);
        assert!(p.cars[0].target.is_none() && p.claims(1) == 0);
    }

    /// `0x406720` runs the cops in index order, so a cop that gives up (0x4071E0) before a
    /// higher-index idle cop searches is already out of `0x403120`'s 500 m test for it.
    #[test]
    fn state_one_runs_in_index_order_among_the_other_cops() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 43, moving: true }] };
        let cars: Vec<CopCar> = (0..2).map(|_| CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)).collect();
        let mut p = Police::new(file, cars, 0, false);
        let r = |slice: usize| RacerRef { position: t.slices[slice].centre, slice, progress: slice as f32 / 400.0, laps_timed: 0, speed: 20.0 };
        let tgt = |slice: usize| ChaseTarget { position: t.slices[slice].centre, slice, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        // cop 0 is out, far down the track from the (clean) human, and within 216 m of it in the
        // plane (the give-up test measures octdist to the reference car — here the target)
        p.set_racers(vec![r(10)]);
        assert!(p.deploy_as(&t, 0, 0, 0, false));
        p.cars[0].slice = 250;
        p.cars[0].position = t.slices[10].centre;
        p.tick(&t, 1000, &tgt(10), &[], &[10], &mut || 0);
        assert!(p.cars[0].active && !p.cars[1].active, "the first query only moves the human's cursor");
        // next tick cop 0 sits inside 500 m of the human's slice but is past 216 m of its reference
        // car, so it gives up in its own update — which runs before cop 1's state 1, so cop 1 finds
        // no cop near the human and comes out at 43 (run the other way round, `0x403120` would
        // still see cop 0 and `lim` would be 0)
        p.cars[0].slice = 13;
        p.cars[0].position = Vec3::new(5000.0, 0.0, 5000.0);
        p.set_racers(vec![r(13)]);
        p.tick(&t, 1002, &tgt(13), &[], &[13], &mut || 0);
        assert!(!p.cars[0].active, "cop 0 gave up");
        assert!(p.cars[1].active && p.cars[1].slice == 43, "cop 1 placed the same tick");
    }

    /// `0x40E4B0` looks only at the record under the perp's cursor: of two records at one slice the
    /// walk from below stops on the first, and while that one cools down the other is never found.
    #[test]
    fn the_record_cursor_sees_one_record_per_slice() {
        let t = ring(400);
        let mut m = mgr(&[100, 100, 200], true);
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), Some(0));
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5002, 0.0, false, 0, &[], 1, true), None, "record 1 is behind record 0");
        // the record cursor walks up to record 2 for the one at 200
        m.cursors = vec![Some(196)];
        assert_eq!(m.find_spawn(&t, 170, 20.0, 5004, 0.0, false, 0, &[], 1, true), Some(2));
        // a sweep that starts below 100 walks past both 100s down to record 0 (still cooling)…
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5006, 0.0, false, 0, &[], 1, true), None);
        // …but one whose first slice is 100 stops on the first record at or below it: record 1
        m.rec_cursors = vec![2];
        m.cursors = vec![Some(100)];
        assert_eq!(m.find_spawn(&t, 74, 20.0, 5008, 0.0, false, 0, &[], 1, true), Some(1), "from above the walk stops on record 1");
        // and a sweep scans `lo … hi − 1` (0x40558E): the marker's own slice waits for the next query
        let mut m = mgr(&[101], true);
        m.cursors = vec![Some(96)];
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), None, "96..100");
        assert_eq!(m.find_spawn(&t, 71, 20.0, 5000, 0.0, false, 0, &[], 1, true), Some(0), "101");
    }

    /// `0x417710`: the reference car is the top of `0x5E8B88`, sorted by `car+0xAF8 − car+0xB20`
    /// with a stable bubble sort — so a tie keeps the order the previous sort left, and a racer a
    /// lap down but further round its lap is on top.
    #[test]
    fn the_reference_order_is_lap_progress_and_ties_keep_the_last_order() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        let r = |slice: usize, progress: f32, laps: u32| RacerRef { position: t.slices[slice].centre, slice, progress, laps_timed: laps, speed: 20.0 };
        // a tie at the start: registration order, so the later racer is on top
        p.set_racers(vec![r(60, 0.5, 0), r(60, 0.5, 0)]);
        assert_eq!(p.leading_racer(), Some(1));
        // racer 0 moves ahead, then they tie again: 0 stays on top, the sort does not swap equals
        p.set_racers(vec![r(61, 0.6, 0), r(60, 0.5, 0)]);
        assert_eq!(p.leading_racer(), Some(0));
        p.set_racers(vec![r(62, 0.7, 0), r(62, 0.7, 0)]);
        assert_eq!(p.leading_racer(), Some(0), "the tie keeps the last order");
        // the key subtracts the timed laps: 1.3 − 1 = 0.3 is below 0.8 − 0 = 0.8
        p.set_racers(vec![r(80, 1.3, 1), r(70, 0.8, 0)]);
        assert_eq!(p.leading_racer(), Some(1), "progress into the lap, not the race position");
        assert!(p.deploy(&t, 0, 0, 1000));
        assert_eq!(p.cars[0].reference, Some(1));
    }

    /// §17.5b: a perp with no violation left is dropped by state 4's own `0x405220` (0x40471F →
    /// 0x4053E9…0x405424) and the cop falls to state 2 the same tick (0x404795 → 0x4047D9), lights
    /// still on, driving the Normal; state 2's head puts them out next tick and nothing re-acquires.
    #[test]
    fn a_perp_that_turns_clean_is_dropped_to_state_2() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        assert!(p.deploy(&t, 0, 0, 1000));
        chase_now(&mut p, 0);
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 1002, &tgt, &[]);
        assert!(p.cars[0].chasing && p.cars[0].mode.is_some());
        p.set_wanted(false);
        p.update(&t, 1004, &tgt, &[]);
        let c = &p.cars[0];
        assert!(c.active && !c.chasing && c.mode.is_none(), "state 2, not given up");
        assert!(c.lights, "state 4's head lit it; only state 2's head clears it");
        assert_eq!((c.boost_engine, c.boost_speed, c.aim_point), (1.0, 1.0, Vec3::ZERO), "the chase's dtor 0x40CC10");
        p.update(&t, 1006, &tgt, &[]);
        assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights, "patrolling, unlit");
    }

    /// §17.5c: the perp's tick clears the violation once it has finished, and a chasing cop drops
    /// to state 2 (0x40479F / the empty target) instead of giving up; it patrols from then on.
    #[test]
    fn a_finished_perp_is_clean_and_its_chaser_patrols() {
        assert_eq!(perp_update(Violation::Speeding, true, |_| unreachable!("0x4059C0 skips check")), Violation::None);
        assert_eq!(perp_update(Violation::None, false, |_| Violation::WrongWay), Violation::WrongWay);
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        assert!(p.deploy(&t, 0, 0, 1000));
        chase_now(&mut p, 0);
        // even with the perp still marked wanted, 0x40479F alone drops a finished target
        let done = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: true };
        p.update(&t, 1002, &done, &[]);
        assert!(p.cars[0].active && !p.cars[0].chasing && p.cars[0].mode.is_none(), "state 2, not retired");
        // the perp's own tick has cleared it: no re-acquire, the cop keeps patrolling
        p.set_wanted(!perp_update(Violation::Speeding, true, |v| v).is_clean());
        for k in 0..3 {
            p.update(&t, 1004 + 2 * k, &done, &[]);
            assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights);
        }
    }

    #[test]
    fn the_duty_budget_caps_how_many_cops_are_out() {
        // §17.2: three at once with a single racer, four with opponents, two in a two-player game
        assert_eq!(duty_budget(1, false), 3);
        assert_eq!(duty_budget(4, false), 4);
        assert_eq!(duty_budget(2, true), 2);
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let cars: Vec<CopCar> = (0..4).map(|_| CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)).collect();
        let mut p = Police::new(file, cars, 0, false);
        assert_eq!(p.budget, 3, "a single racer");
        for i in 0..4 {
            p.deploy(&t, i, 0, 100);
        }
        // …but the budget caps nothing: pass 1 puts every cop on duty because `car+0xECC & 4` is
        // never set in this build, so all four cars may be out.
        assert_eq!(p.active_count(), 4);
        assert_eq!(COP_LEVEL[0], 2, "a role-0 cop is always level 2");
    }

    #[test]
    fn the_probability_matches_the_shipped_files() {
        // autobahn ships 21 records -> 16/21
        let a = mgr(&(0..21).map(|i| i * 100).collect::<Vec<_>>(), true);
        assert!((a.spawn_probability(false, 0) - 16.0 / 21.0).abs() < 1e-6);
        assert!(a.spawn_probability(true, 0) > 1.0, "a clean perp doubles it");
        assert_eq!(a.spawn_probability(false, 2), 1.0, "the hardest skill is certain");
        // 0x4054EB compares with 2 for equality: any other value keeps 16 / nRecords
        assert!((a.spawn_probability(false, 3) - 16.0 / 21.0).abs() < 1e-6);
        // every other shipped track has 16 or fewer, so the roll always succeeds
        let c = mgr(&[100, 300, 500, 800, 1100], true);
        assert!(c.spawn_probability(false, 0) >= 1.0);
    }
}

// ---------------------------------------------------------------------------------------------
// The cop car itself: the chase state of `AIState_Chase` (§27.4–§27.10) on top of the same drive
// model and lane machinery traffic uses.
// ---------------------------------------------------------------------------------------------

use crate::aidrive::{self, AiCarState};
use crate::lanes::{lane_of, lane_span, LaneCar, LaneScratch, LaneSelf, PERSONALITIES};
use glam::{Mat3, Vec3};

/// `[0x5B265C]` — `0x40AF70`'s police branch hands the chase this before any cap. `car+0x1024` is a
/// hardcoded 100.0 (0x406E4F) and `car+0x109C` the rubber-band multiplier.
pub const PURSUIT_BASE_SPEED: f32 = 100.0;
/// `0x40CE80` — the rubber band while the boost budget lasts.
pub const BOOST_ENGINE: f32 = 3.0;
pub const BOOST_SPEED: f32 = 2.5;
/// `chase+0x80` / `chase+0x84` for role 0 (`0x5CBFC0`): the boost is cut this close to a human.
pub const BOOST_SUPPRESS_AHEAD: f32 = 20.0;
pub const BOOST_SUPPRESS_BEHIND: f32 = 20.0;
/// `chase+0x8C` — a blackout lasts 15 s.
pub const BLACKOUT_TICKS: u64 = 0x3C0;
/// `chase+0x90 > 160` — five seconds unable to reach the target's lane: `0x40DA00` adds
/// `[0x5E6654]`, the 32 Hz ticks each (8 Hz) update covers. *(Until 2026-09-25 this said two and
/// a half, the port counting 2 on every 32 Hz tick.)*
pub const STUCK_LIMIT: i32 = 160;
/// `0x40E330(state, 10.0)` (0x405048 pushes `0x41200000`) — state 9 has arrived inside 10 m.
pub const GOTO_SLICE_ARRIVED: f32 = 10.0;

/// What `0x40DA00(chase)` does in one tick — §27.10. It is the only writer of the stuck timer
/// `chase+0x90` besides the ctor, and the only source of the +15 toward the target's lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneBias {
    /// an early return (0x40DA27 / the four lane-range tests to 0x40DB0D): the timer is untouched
    Skip,
    /// 0x40DB07: the corridor is whole at the target's slice — the timer is zeroed and nothing is added
    Clear,
    /// 0x40DAED: the corridor is broken at the cop's slice too — `chase+0x90 += [0x5E6654]`
    Blocked,
    /// 0x40DAC0…0x40DADF: broken at the target's slice only — the timer is zeroed and
    /// `0x401380(cop, tgtLane)` runs three times, +15 to the side the target is on
    Toward,
}

/// `0x40EAB0(slice, laneA, laneB)` — non-zero when some lane from `laneA` to `laneB` inclusive does
/// **not** exist at that slice: `((1 << (span+1)) − 1) & (~rec+0x34 >> (22 − max))`, and `rec+0x34`
/// carries bit `22 − lane` for each lane that exists (`lanes::lane_bit`).
pub fn corridor_broken(sl: &crate::race::Slice, lane_a: i32, lane_b: i32) -> bool {
    (lane_a.min(lane_b)..=lane_a.max(lane_b)).any(|l| !crate::lanes::lane_exists(sl, l))
}

/// `0x40DA00(chase)` — read to the instruction 2026-09-25 (0x40DA00…0x40DB12). `gap` is
/// `chase+0x68`, the lanes are `car+0x1020`. It returns without a word when `|gap| > 500`
/// (`[0x5B24E0]`, `test ah, 0x41 / je` at 0x40DA24) or when either car's lane is outside its own
/// slice's `[6 − lanesA, lanesB + 7]` (`rec+0x38` / `rec+0x3C`). Then `a = 0x40EAB0(tgtSlice, tgtLane,
/// copLane)` and `b = 0x40EAB0(copSlice, tgtLane, copLane)` decide it: `a == 0` clears, `b != 0`
/// counts, and only `a != 0, b == 0` pushes the cop toward the target's lane. So the bias is **not**
/// a standing +15 whenever the target is within 500 m, which is what the port did until this date.
pub fn lane_bias(table: &SliceTable, cop_slice: usize, cop_lane: i32, tgt_slice: usize, tgt_lane: i32, gap: f32) -> LaneBias {
    if gap.abs() > 500.0 {
        return LaneBias::Skip;
    }
    let last = table.slices.len().saturating_sub(1);
    let (cs, ts) = (&table.slices[cop_slice.min(last)], &table.slices[tgt_slice.min(last)]);
    let on_road = |sl: &crate::race::Slice, lane: i32| lane >= 6 - sl.lanes_a as i32 && lane <= sl.lanes_b as i32 + 7;
    if !on_road(cs, cop_lane) || !on_road(ts, tgt_lane) {
        return LaneBias::Skip;
    }
    if !corridor_broken(ts, tgt_lane, cop_lane) {
        LaneBias::Clear
    } else if corridor_broken(cs, tgt_lane, cop_lane) {
        LaneBias::Blocked
    } else {
        LaneBias::Toward
    }
}

/// `0x405AD0`'s duty budget (§17.2): **3** when the race has a single racer, **2** in a two-player
/// game and **4** otherwise.
///
/// It is **not** a cap on how many cops are out, because the pass that spends it never runs in this
/// build: pass 1 hands duty to every cop whose `car+0xECC & 4` is clear, and that bit is never set
/// anywhere in `.text` (0x406DF2 zeroes the word and the only `or`s into it are bit 1). The budget
/// and the heat record's `want[role]` only gate pass 2, which is reached solely when pass 1 skipped
/// a cop. So every cop car in the race is on duty, and this function is kept for the record.
pub fn duty_budget(racers: usize, two_players: bool) -> usize {
    if racers == 1 {
        3
    } else if two_players {
        2
    } else {
        4
    }
}

/// `heatRec+0x18 + 4·role` (0x4053D6) — the chase difficulty of a cop of that role. The heat record
/// is always chain 0's first record (`perpAI+0x9C` is written only by the init at 0x405C64/0x405CE0,
/// and `perpAI+0x90` only ever gets `0x5CBD20`), whose `level` pair is `(2, 0)`. Role 0 is the
/// default (`role = (car+0x52C & 0x40) ? 1 : 0`), so **a cop is level 2** whatever the race skill is.
pub const COP_LEVEL: [usize; 2] = [2, 0];

/// `0x40D680`'s distance ladder, by skill level. The entries are the speeds for
/// `d > 150 / 100 / 50 / 25 / 10 / 6 / else`.
pub const PURSUE_SPEED: [[f32; 7]; 3] = [[60.0, 50.0, 40.0, 35.0, 20.0, 3.0, 0.0], [60.0, 50.0, 40.0, 35.0, 10.0, 3.0, 0.0], [80.0, 70.0, 50.0, 40.0, 14.0, 6.0, 0.0]];

/// `0x40D680`'s cap for a distance and a skill level.
pub fn pursue_cap(level: usize, d: f32) -> f32 {
    let t = &PURSUE_SPEED[level.min(2)];
    if d > 150.0 {
        t[0]
    } else if d > 100.0 {
        t[1]
    } else if d > 50.0 {
        t[2]
    } else if d > 25.0 {
        t[3]
    } else if d > 10.0 {
        t[4]
    } else if d > 6.0 {
        t[5]
    } else {
        t[6]
    }
}

/// `0x5CBEE8[level][rank]` (§17.4): where each chaser should sit relative to the target, in metres
/// — `(x = lateral, y, z = longitudinal)`. Rank is the cop's place in the target's queue.
pub const FORMATION: [[[f32; 3]; 6]; 3] = [
    [[0.0, 0.0, 8.0], [-6.0, 0.0, 8.0], [6.0, 0.0, 0.0], [0.0, 0.0, -10.0], [0.0, 0.0, -10.0], [0.0, 0.0, -10.0]],
    [[0.0, 0.0, 5.0], [-4.0, 0.0, 5.0], [4.0, 0.0, 5.0], [0.0, 0.0, -5.0], [0.0, 0.0, -5.0], [0.0, 0.0, -5.0]],
    [[0.0, 0.0, 5.0], [-4.0, 0.0, 5.0], [4.0, 0.0, 5.0], [0.0, 0.0, -5.0], [0.0, 0.0, -5.0], [0.0, 0.0, -5.0]],
];

/// `0x5CBEA0[level]+0x08` — state 4 switches to the close-in (ram) mode inside this gap (§27.4).
pub const CLOSE_IN_GAP: [f32; 3] = [13.0, 15.0, 18.0];
/// `0x5CBEA0[level]+0x0C` — how long the close-in (ram) mode lasts, in ticks: 0x4046C3 hands it to
/// `0x40CD50(chase, 1, ticks)`.
pub const CLOSE_IN_TICKS: [u64; 3] = [0x200, 0x300, 0x480];
/// `0x5CBEA0[level]+0x10` — the boost budget `chase+0x7C`, in 32 Hz ticks of boost (`0x40CD70`
/// charges `[0x5E6654]` per update that had it on): 16 / 16 / 32 s. *(Until 2026-09-25 this said
/// 8 / 8 / 16 s, counting 64 Hz ticks.)* State 2 loads it
/// at 0x404392 (`[edx*4 + 0x5CBEB0]`, `edx = 5·level`) and passes it as the Chase ctor's fourth
/// argument, which 0x40CB96 stores at `+0x7C`. *(Corrected 2026-09-25: the port used the `+0x0C`
/// column, the ram duration, 0x480 against 0x400.)*
pub const BOOST_BUDGET: [i32; 3] = [0x200, 0x200, 0x400];

/// `0x40D150`'s speed factor by |gap| (§27.8): the cop eases off as it closes.
pub fn close_quarters_factor(gap: f32) -> f32 {
    let d = gap.abs();
    if d < 30.0 {
        0.95
    } else if d < 100.0 {
        0.8
    } else if d < 150.0 {
        0.75
    } else if d < 200.0 {
        0.7
    } else {
        0.6
    }
}

/// `MODE == −2`'s factor by difficulty (`chase+0x88`).
pub const CLOSE_QUARTERS_SLOW: [f32; 3] = [0.79, 0.73, 0.65];

/// −1 / 0 / +1: below `lo`, between, above `hi`. The three sector tests of §27.8 are this with
/// different windows.
pub fn sector(v: f32, lo: f32, hi: f32) -> i32 {
    if v < lo {
        -1
    } else if v > hi {
        1
    } else {
        0
    }
}

/// What the chase needs to know about the car it is after.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChaseTarget {
    pub position: Vec3,
    pub slice: usize,
    pub lateral: f32,
    pub lane: i32,
    /// `car+0xEC0`, signed along the route
    pub speed: f32,
    /// `car+0x3A8` / `car+0x3B0` — the target's half width and half length, which are the windows
    /// the §27.8 sector tests use.
    pub half_width: f32,
    pub half_length: f32,
    pub finished: bool,
}

/// A racer — a car with `car+0x52C & 1`, the human (0x40F291) and every opponent (0x40F32E, which
/// is **not** gated on `COPS`) — in registration order, i.e. `0x5E8E2C[]`'s: index 0 is the human.
/// Each one is a perp (`AIHigh_Human` / `AIHigh_Opponent`, both built on `AIHigh_BasicPerp` by
/// 0x405C30), and each can be a cop's **reference car** `car+0xED4` (`car+0x52C & 0x100`, which an
/// opponent has once `COPS` is on, 0x40F33F — always, when there are cops).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RacerRef {
    pub position: Vec3,
    pub slice: usize,
    /// `car+0xAF8` — `lap + frac − unlaps`
    pub progress: f32,
    /// `car+0xB20` — the laps already timed (`Race_updateProgress` 0x416643 stores the lap counter
    /// there when it takes the lap time), which `0x417710` subtracts for the `0x5E8B88` order
    pub laps_timed: u32,
    /// `car+0xEC0`, signed along the route — the spawn search's direction of travel (0x40551A)
    pub speed: f32,
}

impl RacerRef {
    /// `0x417710`'s first key (0x41772D…0x417745): `car+0xAF8 − (float)car+0xB20`.
    fn order_key(&self) -> f32 {
        self.progress - self.laps_timed as f32
    }
}

/// `0x417710`'s bubble sort run to completion over `order` (indices into `racers`), ascending by
/// `key`, swapping only on **strictly** less (`fcompp` / `test ah, 1` at 0x417745; `test ah, 0x41`
/// at 0x4177A6 for the second array) — so it is stable, and equal keys keep the order they had after
/// the previous sort (initially the registration order, 0x40F189). The arrays hold every real car,
/// but a stable sort orders the racers among themselves the same whatever else is in them.
fn stable_order(order: &mut Vec<usize>, racers: &[RacerRef], key: impl Fn(&RacerRef) -> f32) {
    if order.len() != racers.len() {
        *order = (0..racers.len()).collect();
    }
    order.sort_by(|&a, &b| key(&racers[a]).partial_cmp(&key(&racers[b])).unwrap_or(std::cmp::Ordering::Equal));
}

/// Which of the three behaviours a tick picked, for tracing and tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChaseMode {
    CatchUp,
    CloseQuarters,
    Pursue { hard: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CopCar {
    /// index into the caller's model list
    pub model: usize,
    pub active: bool,
    /// `car+0xECC & 2` — lights on, which is also what switches the target speed to 100 m/s
    pub lights: bool,
    /// The cop is chasing (`AIHigh_Cop` state 4). Otherwise it patrols (states 2/3) and builds its
    /// chase on the first tick of its own that finds the perp wanted (`0x405220`) — §17.4, §17.5b.
    pub chasing: bool,
    /// State 3's `AIState_Stop` mode 1 (0x40CAB0): a cop placed from a **parked** record
    /// (`+0x10 == 0`) holds its lateral at zero speed until it builds a chase.
    pub parked: bool,
    /// State 1 placed the cop this tick and already ran the new state's update (0x404243); the
    /// state 2/3 head and its `0x405220` only run from the next tick on.
    pub placed: bool,
    pub slice: usize,
    /// `car+0xEB0`
    pub dir: f32,
    /// `car+0xEAC` — the direction the chase asks for; the car turns round to obey it
    pub requested_dir: f32,
    pub lateral: f32,
    pub lateral_target: f32,
    /// `car+0xEC0` signed along the route
    pub speed: f32,
    pub target_speed: f32,
    /// `car+0x330`, the body origin: `half_extents.y` (`car+0x3AC`) above the model's ground
    /// plane, the model drawn back down at [`Self::draw_origin`] (as traffic's, `traffic.md` §29)
    pub position: Vec3,
    pub orientation: Mat3,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    /// `car+0x3A8…+0x3B0`: half the model box 0x445090 takes — a cop (`car+0x52C` = 0x22 / 0x62,
    /// bit 0x10 clear at 0x4450D2) takes a racer's, `[this+0xB0][4]` = **LOD 5**
    /// (`nfs_game::world::car_body`), not traffic's LOD 1
    pub half_extents: Vec3,
    pub mass: f32,
    /// `car+0x1064..0x106C` and `car+0x1070` — the ram aim point
    pub aim_point: Vec3,
    pub aim_lateral: f32,
    /// `car+0x1098` / `car+0x109C` — the engine and speed multipliers
    pub boost_engine: f32,
    pub boost_speed: f32,
    /// `chase+0x7C` — the boost budget in ticks
    pub boost_budget: i32,
    /// `chase+0x8C`. The original's only writer besides the ctor is the state-4 head 0x404571,
    /// which blacks the rubber band out for 15 s when *some* human has been faster than 20 m/s
    /// while un-pursued for over 25 s (`perpAI+0xAC > 0x320`, in `[0x5E6654]` units of 1/32 s —
    /// 12.5 s until 2026-09-25, read as 64 Hz ticks). That counter is reset the instant
    /// a chaser is claimed and the player's AI runs before any cop's, so with one human the rule
    /// can never fire (`traffic.md` §17.5) — hence no writer here either.
    pub blackout_until: u64,
    /// `chase+0x90`
    pub stuck_ticks: i32,
    /// `AIState_GotoSlice+0x08` — the slice a cop in `AIHigh_Cop` state 9 is driving to. Set only by
    /// the state-4 stuck branch (0x404513: the **reference car's** slice at that moment), cleared on
    /// arrival.
    pub goto_slice: Option<usize>,
    /// `car+0xED4`, the reference car, as an index into [`Police::racers`]. For a cop it is written
    /// once per deploy, by the `AIState_Purgatory` dtor 0x40DFB0 that state 1 runs when it places the
    /// car (0x40E023…0x40E048): the first car with `car+0x52C & 0x100` walking `0x5E8B88` down from
    /// the top, i.e. the racer furthest **into its current lap** (`Police::leading_racer`). Nothing rewrites it while the cop is out (0x407030 and
    /// 0x407290 are traffic's). `None` stands for "the target", which is what the port has when the
    /// caller supplies no racers — with a single perp that is the same car.
    pub reference: Option<usize>,
    /// `AIHigh_Cop+0x58`, the perp this cop has claimed, as an index into [`Police::racers`]
    /// (`0x5E8E2C` order, 0 = the human). Set by `0x405220` when it hands the cop a new perp (and
    /// the claim 0x403BD0 counts it in that perp's `+0x70[role]`, [`Police::claims`]); cleared by
    /// the release 0x403BA0 — the empty-handed `0x405220` (0x4053E9…0x405424), the state-4 exit
    /// 0x4047D9, the GotoSlice (0x40503A), state 1's placement (0x4041E6) and the give-ups (0x404402, 0x404942).
    pub target: Option<usize>,
    /// `car+0x104C` and its timers — the gear the drive model runs in (`aidrive::Gear`)
    pub gear: aidrive::Gear,
    /// `car+0xD80`, the brake-light counter (`aidrive::brake_light`)
    pub brake_light: u8,
    /// `chase+0x60`-side bookkeeping for §27.8: this cop's rank in the target's queue, the
    /// formation offset that rank implies and the two desired sectors it maps to.
    pub rank: usize,
    /// `chase+0x78` — ticks spent standing in the formation slot.
    pub slot_ticks: i32,
    /// `chase+0x70` / `+0x74` — the close-in (ram) mode and when it lapses.
    pub ram_until: u64,
    /// `chase+0x88` — the skill level that picks the PURSUE ladder
    pub level: usize,
    /// `car+0x101C` — the lane-existence scan window
    scan: i32,
    pub mode: Option<ChaseMode>,
    /// `car+0x78` — 0x417190's octagonal x–z distance to the nearest racer, written by the cop's
    /// tracker 0x49AF70 (Sc32 priority 6, after the AI pass) while it is in the world
    /// ([`Police::track_racers`]); the AI pass's gate [`crate::traffic::ai_pass_due`] reads it.
    pub racer_distance: f32,
    /// The four ground probes `Susp_update` 0x499A70 takes (0x499AD4…0x499B13): body points 0…3
    /// of the `BndP` set after 0x49ABC0's sort and quadrant deal — FL, FR, RL, RR in the model's own
    /// axes, lowered by `car+0x3AC` (`nfs_game::world::car_body`; a cop takes the `Susp_update` path
    /// like a racer, only `car+0x52C & 0x80` forks, `suspension-collision.md` §4.2 item 9). The race
    /// sets them from the cop's model; [`CopCar::new`] starts from the box's bottom corners at the
    /// wheel-quad height (APPROXIMATION, for callers without a model — the unit tests).
    pub probes: [Vec3; 4],
    /// The car tracker `car+0x400` / `car+0x3E8` (0x49AF70 → `updateZone` + 0x499520), whose plane
    /// is `Susp_update`'s fallback for a probe with no usable patch.
    pub tracker: nfs_physics::collision::PatchTracker,
    /// The wheel trackers `wheel_i + 4` `Susp_update` runs `updateZone` on.
    pub wheel_trackers: [nfs_physics::collision::PatchTracker; 4],
    /// The trackers were seated since the last deploy — `Car_placeAtSlice` 0x49B090's tracker part
    /// (`PatchTracker::seat`), which the port runs on the first ground step after [`Police::deploy_as`]
    /// (the deploy has no collision grid).
    seated: bool,
    /// `word car+0x418`: `Susp_update` stores `nAir > 1` (0x49A8AA…0x49A8C0) and the car–car impulse
    /// steps it on both cars (0x496F1E / 0x496F25, the caller's collision pass). 0x408360 reads it
    /// at 0x408543: only while it is 0 does the "on its side" exit fire.
    pub airborne_count: u16,
}

impl CopCar {
    pub fn new(model: usize, half_extents: Vec3, mass: f32, level: usize) -> Self {
        Self {
            gear: aidrive::Gear::default(),
            brake_light: 0,
            model,
            active: false,
            lights: false,
            chasing: false,
            parked: false,
            placed: false,
            slice: 0,
            dir: 1.0,
            requested_dir: 1.0,
            lateral: 0.0,
            lateral_target: 0.0,
            speed: 0.0,
            target_speed: 0.0,
            position: Vec3::ZERO,
            orientation: Mat3::IDENTITY,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            half_extents,
            mass,
            aim_point: Vec3::ZERO,
            aim_lateral: 0.0,
            boost_engine: 1.0,
            boost_speed: 1.0,
            boost_budget: 0,
            blackout_until: 0,
            stuck_ticks: 0, rank: 0, slot_ticks: 0, ram_until: 0,
            goto_slice: None,
            reference: None,
            target: None,
            level,
            scan: 0,
            mode: None,
            racer_distance: 0.0,
            probes: {
                let (x, y, z) = (half_extents.x, -(crate::traffic::RIDE_HEIGHT + half_extents.y), half_extents.z);
                [Vec3::new(-x, y, z), Vec3::new(x, y, z), Vec3::new(-x, y, -z), Vec3::new(x, y, -z)]
            },
            tracker: nfs_physics::collision::PatchTracker::default(),
            wheel_trackers: [nfs_physics::collision::PatchTracker::default(); 4],
            seated: false,
            airborne_count: 0,
        }
    }

    pub fn speed_estimate(&self) -> f32 {
        oct_norm(self.velocity.x, self.velocity.z)
    }

    /// Where the model is drawn, as for every `R3DCar`: `car+0x330 − M[1]·car+0x3AC` (slot 5
    /// 0x43A410, the draw 0x443490 at 0x4434D7) — `crate::traffic::TrafficCar::draw_origin`'s rule.
    pub fn draw_origin(&self) -> Vec3 {
        self.position - self.orientation.row(1) * self.half_extents.y
    }

    /// The origin's height above the ground plane at rest, along the plane normal: the `BndP`
    /// wheel quad (model `y` = −[`crate::traffic::RIDE_HEIGHT`] on every police model too) lowered
    /// by `car+0x3AC` (0x4421BE) is what `BodyHeight` 0x49A900 measures and both the placement's
    /// drop (0x49B4A4…0x49B4BC) and `Car_ground`'s resting branch take off `car+0x334` — so on a
    /// plane of normal `n` the origin settles `rest_height / n.y` above the plane point under it.
    pub fn rest_height(&self) -> f32 {
        crate::traffic::RIDE_HEIGHT + self.half_extents.y
    }

    pub fn as_lane_car(&self, table: &SliceTable) -> LaneCar {
        let sl = &table.slices[self.slice.min(table.slices.len() - 1)];
        LaneCar {
            slice: self.slice,
            lane: lane_of(sl, self.lateral),
            lane_mask: lane_span(sl, self.lateral, self.half_extents.x),
            speed: self.speed,
            speed_est: self.speed_estimate(),
            half_len: self.half_extents.z,
            lateral: self.lateral,
            half_span: self.half_extents.x,
            is_human: false,
            is_racer: false,
        }
    }

    fn as_ai_state(&self, max_lat_acc: f32, max_yaw_acc: f32) -> AiCarState {
        AiCarState {
            position: self.position,
            orientation: self.orientation,
            velocity: self.velocity,
            angular_velocity: self.angular_velocity,
            slice: self.slice,
            dir: self.dir,
            requested_dir: self.requested_dir,
            sense: self.gear.sense,
            speed: self.speed,
            speed_est: self.speed_estimate(),
            target_speed: self.target_speed,
            lateral: self.lateral,
            lateral_target: self.lateral_target,
            aim_point: self.aim_point,
            max_yaw_acc,
            max_lat_acc,
        }
    }
}

fn oct_norm(x: f32, z: f32) -> f32 {
    let (x, z) = (x.abs(), z.abs());
    x.max(z) + 0.25 * x.min(z)
}

/// The physics constants a cop's own `.sim` supplies, same shape as traffic's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CopPhysics {
    pub brake_force: f32,
    pub max_lat_acc: f32,
    pub max_yaw_acc: f32,
}

impl Default for CopPhysics {
    fn default() -> Self {
        Self { brake_force: 9.0, max_lat_acc: 9.8, max_yaw_acc: 2.6 }
    }
}

/// The whole police side of a race: the spawn manager, the cars and the chase.
#[derive(Debug, Clone)]
pub struct Police {
    pub manager: CopManager,
    pub cars: Vec<CopCar>,
    pub cfg: nfs_formats::trafcfg::TrafficConfig,
    pub phys: CopPhysics,
    /// `AIHigh_Perp+0x78` — what the player is currently wanted for
    pub violation: Violation,
    /// `[0x657430]` — the race runs backwards (the port's table is already reversed, so this is +1)
    pub route_dir: f32,
    /// `SKILL [0x6573F4]`
    pub skill: u8,
    /// `0x405AD0`'s duty budget — kept for the record; see [`duty_budget`] for why it caps nothing.
    pub budget: usize,
    /// `AIHigh_Perp+0x78 != 0` — the perp is wanted, which is all `0x405220` needs to hand a patrol
    /// car its target (`Police::set_wanted`)
    pub wanted: bool,
    /// `WEATHER` (`[0x657A24] > 0`). `0x4081B0` scales the longitudinal rate by **0.9** and it does
    /// so *after* the traffic/1.0 split at 0x4081FC, so it applies to a cop as much as to a traffic
    /// car (0x40825C…0x40826E, `[0x5B25E8]` = 0.9). The shipped `fe.txt` says `weather = 0`, so it
    /// is off in a normal race — and it is the option, not the track's night flag.
    pub weather: bool,
    /// The racers (`RacerRef`) in registration order — the perps, and the cars a cop can take as
    /// its reference car — refreshed by the caller every tick through [`Police::set_racers`] before
    /// the spawn pass and `update`; index 0 is the human.
    pub racers: Vec<RacerRef>,
    /// `0x5E8B88[]` restricted to the racers: ascending by `car+0xAF8 − car+0xB20`, stable across
    /// ticks (`0x417710`'s first sort). The purgatory dtor 0x40DFB0 walks it from the top.
    race_order: Vec<usize>,
    /// `0x5E8BAC[]` restricted to the racers: ascending by `car+0xAF8`, stable across ticks
    /// (`0x417710`'s second sort). The spawn search 0x405440 walks it from the top.
    spawn_order: Vec<usize>,
    one_way: bool,
    /// The racer slot of cop 0 — the cops come after the humans and the opponents (0x4ECF00:
    /// `edi = nP + nOpp` at 0x4ED103 / 0x4ED1ED, `traffic.md` §17.5d). It is the phase of the AI
    /// pass's gate ([`crate::traffic::ai_pass_due`], `car+0` = `slot | 0x100`); the caller sets it.
    pub slot_base: usize,
}

impl Police {
    pub fn new(file: CopFile, cars: Vec<CopCar>, skill: u8, one_way: bool) -> Self {
        Self { manager: CopManager::new(file), cars, cfg: nfs_formats::trafcfg::TrafficConfig::default(), phys: CopPhysics::default(), violation: Violation::None, route_dir: 1.0, skill, one_way, wanted: false, weather: false, racers: Vec::new(), race_order: Vec::new(), spawn_order: Vec::new(), budget: duty_budget(1, false), slot_base: 0 }
    }

    /// This tick's racers, and `0x417710` (called at 0x4179BF at the end of the per-tick car pass):
    /// both race-order arrays re-sorted, each keeping its own order from the last call for ties.
    pub fn set_racers(&mut self, racers: Vec<RacerRef>) {
        // each perp's marker cursor starts at its own slice (the perp ctor, 0x405CBB)
        for (p, r) in racers.iter().enumerate() {
            self.manager.seed_cursor(p, r.slice);
        }
        self.racers = racers;
        stable_order(&mut self.race_order, &self.racers, RacerRef::order_key);
        stable_order(&mut self.spawn_order, &self.racers, |r| r.progress);
    }

    /// `car+0xED4`'s position and slice for cop `i`: the racer it latched at deploy, or the target.
    fn reference_of(&self, i: usize, target: &ChaseTarget) -> (Vec3, usize) {
        match self.cars[i].reference.and_then(|r| self.racers.get(r)) {
            Some(r) => (r.position, r.slice),
            None => (target.position, target.slice),
        }
    }

    /// 0x40DFB0's pick (0x40E00F…0x40E048): walking `0x5E8B88` from index `[0x5E8B58]−1` down, the
    /// first car with `car+0x52D & 1` — the racer with the **largest `car+0xAF8 − car+0xB20`**, which
    /// is the progress *into the current lap* (`lap − unlaps + frac − laps timed`), not the race
    /// position: on a circuit a racer a lap down but further round the lap is "ahead" here. On a
    /// point-to-point track `+0xB20` stays 0 until the finish, and it is the plain progress. Equal
    /// keys go to whichever of them the stable sort has left later — the registration order at the
    /// start (the later-registered racer), then whatever order the last change of lead left.
    fn leading_racer(&self) -> Option<usize> {
        let mut order = self.race_order.clone();
        stable_order(&mut order, &self.racers, RacerRef::order_key);
        order.last().copied()
    }

    /// `AIHigh_Perp+0x78 != 0` for racer `p`. **Only the human's can ever be set.** An opponent's
    /// perp is built like the human's (0x405910 → 0x405C30 → 0x403CB0: `+0x78 = 0`, the heat record,
    /// `+0x84`, the cursor `+0x88`), but its violation is never checked: `check` 0x403990 (vtable slot
    /// +8) has one caller, `call [eax+8]` at 0x405A3B in 0x4059C0, reached only from `tickHeat`
    /// 0x405D70, whose callers are `AIHigh_Human::update` 0x405693 and `AIHigh_Opponent::update`'s
    /// stateId-2 arm 0x4058B0 — dead, an opponent's stateId goes 0 → 0xB and stays (§17.9). The
    /// other two `[reg+8]` calls in the module (0x406113, 0x406422) are on an `AIState`. So an
    /// opponent is a clean perp for the whole race.
    fn perp_wanted(&self, p: usize) -> bool {
        p == 0 && self.wanted
    }

    /// The perps: the racers, or — when the caller supplies none (unit tests) — the human alone.
    fn perp_count(&self) -> usize {
        self.racers.len().max(1)
    }

    fn perp_slice(&self, p: usize, target: &ChaseTarget) -> usize {
        self.racers.get(p).map(|r| r.slice).unwrap_or(target.slice)
    }

    /// `AIHigh_Perp+0x70[role]` for racer `p` (role 0 — every cop in this build): the cops that have
    /// claimed it (0x403BD0 increments, 0x403BA0 decrements).
    pub fn claims(&self, p: usize) -> usize {
        self.cars.iter().filter(|c| c.active && c.target == Some(p)).count()
    }

    /// `AIHigh_Cop::acquire` 0x405220 for cop `i` (§17.5c): pass 1 over `0x5E8E2C` (the racers in
    /// registration order) takes the nearest perp whose claims — less this cop's own — are under
    /// `lim[role]` (`HEAT_LIM_ROLE0` for a wanted perp, 0 for a clean one); failing that, pass 2 the
    /// nearest **wanted** perp whatever its claims. Nearest is `|0x40E6E0(perp, cop)|`, the wrapped
    /// track gap, below a start of 10 000 m and strictly, so an equal gap keeps the earlier racer.
    /// Opponents are never wanted ([`Police::perp_wanted`]), so neither pass ever takes one: this
    /// returns the human or nothing.
    fn acquire(&self, table: &SliceTable, i: usize, target: &ChaseTarget) -> Option<usize> {
        let cop = &self.cars[i];
        let gap = |p: usize| track_distance(table, self.perp_slice(p, target), cop.slice).abs();
        let mut best = None;
        let mut best_d = 10_000.0;
        for p in 0..self.perp_count() {
            let lim = if self.perp_wanted(p) { HEAT_LIM_ROLE0 } else { 0 };
            let have = self.claims(p) - usize::from(cop.target == Some(p));
            if have < lim && gap(p) < best_d {
                best = Some(p);
                best_d = gap(p);
            }
        }
        if best.is_none() {
            for p in 0..self.perp_count() {
                if self.perp_wanted(p) && gap(p) < best_d {
                    best = Some(p);
                    best_d = gap(p);
                }
            }
        }
        best
    }

    /// `AIHigh_Cop::findSpawnPoint` 0x405440 (§25.5, §17.5d): walk `0x5E8BAC` — the racers by
    /// progress — from the top (0x40545D, `[0x5E8B58]−1` down), take every car with `car+0x52C & 1`,
    /// the human **and the opponents**, and return the first record any of them yields. Each perp
    /// has its own cursor, its own `seen` (its claims) and its own clean/wanted `lim`; an opponent is
    /// always clean, so it gets `lim = 1` whenever no cop is within 500 m of it — an ambient patrol
    /// 186 m up the road from **it**, still vetoed within 172 m of the human (`0x407CC0` looks at
    /// the humans only). `humans` are the humans' slices.
    fn find_spawn_point(&mut self, table: &SliceTable, tick: u64, humans: &[usize], rand16: &mut dyn FnMut() -> u32) -> Option<usize> {
        for k in (0..self.spawn_order.len()).rev() {
            let p = self.spawn_order[k];
            let Some(r) = self.racers.get(p).copied() else { continue };
            let clean = !self.perp_wanted(p);
            let seen = self.claims(p);
            // `0x403120(perp.slice, 500.0)`: a cop out of purgatory inside 500 m of the perp
            let cop_near = self.cars.iter().any(|c| c.active && track_distance(table, c.slice, r.slice).abs() < CLEAN_PERP_CLEARANCE);
            if let Some(rec) = self.manager.find_spawn_for(table, p, r.slice, r.speed, tick as i64, rand16, clean, self.skill, humans, seen, cop_near) {
                tracing::debug!(tick, perp = p, perp_slice = r.slice, clean, seen, record = rec, "police spawn point");
                return Some(rec);
            }
        }
        None
    }

    pub fn active_count(&self) -> usize {
        self.cars.iter().filter(|c| c.active).count()
    }

    /// `0x40AF70`'s police branch: a cop with its lights on drives at `100 · boost`, and everything
    /// else is a cap the behaviour applies afterwards. Lights off falls back to the traffic ladder.
    fn base_target_speed(&self, c: &CopCar, reference_speed: f32) -> f32 {
        if c.lights {
            PURSUIT_BASE_SPEED * c.boost_speed * c.dir
        } else {
            crate::traffic::target_speed(c.dir, self.route_dir, reference_speed, 1.0)
        }
    }

    /// `0x40CE80(chase, checkHumans)`, read to the instruction (0x40CE80…0x40CF6D) — the rubber
    /// band: with budget left (`chase+0x7C > 0`, 0x40CE8B) and past the blackout (`gGameTick >=
    /// chase+0x8C`, 0x40CE9E) it sets `car+0x1098 = 3.0`, `car+0x109C = 2.5`, then puts both back to
    /// 1.0 for a human whose `0x40E6E0(cop, human) · car+0xEB0` is in `(0, chase+0x80)` or
    /// `(−chase+0x84, 0)` — strictly, so a zero gap keeps the boost. It spends nothing: the budget
    /// is charged by the next update's `0x40CD70` ([`Police::spend_boost`]).
    fn apply_boost(&mut self, i: usize, tick: u64, gap_to_human: f32) {
        let c = &mut self.cars[i];
        if c.boost_budget <= 0 || tick < c.blackout_until {
            return;
        }
        c.boost_engine = BOOST_ENGINE;
        c.boost_speed = BOOST_SPEED;
        let g = gap_to_human * c.dir;
        if (g > 0.0 && g < BOOST_SUPPRESS_AHEAD) || (g < 0.0 && g > -BOOST_SUPPRESS_BEHIND) {
            c.boost_engine = 1.0;
            c.boost_speed = 1.0;
        }
    }

    /// `0x40CD70`'s tail (0x40CE36…0x40CE72): if the last update left the boost on (`car+0x1098 !=
    /// 1.0`) the budget `chase+0x7C` is charged `[0x5E6654]` — the 32 Hz ticks this update covers —
    /// and `car+0x1098` / `+0x109C` go back to 1.0 before `0x40CE80` decides again. *(Until
    /// 2026-09-25 the port charged 2 on the tick it lit the boost, every 32 Hz tick.)*
    fn spend_boost(&mut self, i: usize, ai_ticks: i32) {
        let c = &mut self.cars[i];
        if c.boost_engine != 1.0 {
            c.boost_budget -= ai_ticks;
        }
        c.boost_engine = 1.0;
        c.boost_speed = 1.0;
    }

    /// One AI tick for every **active** cop (§27.6's dispatch and the three behaviours), with no
    /// state-1 search for the idle ones, then every cop's drive ([`Police::drive`]) — the unit
    /// tests' entry. The race runs [`Police::ai`] and [`Police::drive`] apart.
    /// Every cop runs on every call, as if due, with `[0x5E6654]` = [`TEST_AI_TICKS`].
    pub fn update(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar]) {
        self.run(table, tick, target, others, None, &|_| Some(TEST_AI_TICKS));
        self.drive(table, tick);
    }

    /// One AI tick for every cop, **in index order** — `0x406720` walks `0x5E6774[]` and calls each
    /// car's `AIHigh` update in turn (0x40672C…0x406785), the racers first and the cops after them,
    /// so a cop out of the world runs its state 1 (`AIHigh_Cop::update` 0x40403F: the duty bit
    /// §17.2a, which every cop has, and `+0x20 != 1`, which always holds; then `0x405440`) between
    /// the updates of the cops around it. What a lower-index cop did this tick — a claim, a release,
    /// a give-up — is what a higher-index cop's search sees, and a cop placed here counts at once for
    /// the next one's `0x403120`. Each idle cop asks on its own; a query that moves a cursor and
    /// finds nothing leaves the next one nothing to sweep unless something changed in between.
    /// `humans` are the humans' slices (the 172 m veto), `rand16` the shared LCG.
    /// *(Until 2026-09-25 the port ran every idle cop's search before any cop's update and stopped
    /// at the first empty query.)*
    ///
    /// This is the AI half only — `AIHigh_Cop::update` 0x403E40 and the `AIState` it runs write the
    /// targets `car+0xEAC` / `car+0xEB4` / `car+0xEB8` and nothing that moves the car. The drive
    /// 0x408360 is each cop's `car+0xE1C` on the Sc32 list at priority 0x1E (`traffic.md` §17.5e),
    /// which runs after the whole AI pass: [`Police::drive`]. *(Until 2026-09-25 (later) the port
    /// drove each cop right after its decision, so a later cop's lane pass saw an earlier cop's new
    /// velocity.)*
    ///
    /// **The gate.** `AIHigh_Cop` keeps the base ctor's `+0x10 = 0` (0x406790; only
    /// `AIHigh_Opponent`'s ctor sets it, 0x40593D), so 0x406720 runs a cop's update only when
    /// `0x414B10(1, 0, car+0x78, car+0, &0x5E6650, &0x5E6654, &0x5E66A0, car+0x10B8)` says it is due
    /// (0x406745…0x406770) — [`crate::traffic::ai_pass_due`], row 0: **8 Hz** within 240 m of the
    /// nearest racer, 4 Hz beyond, staggered by the slot `slot_base + i`; a cop has no racer bit, so
    /// `car+0x10B8` is 0. That holds in every state, state 1's search included. Each counter the
    /// update steps moves by the `[0x5E6654]` it returns — 4 or 8 32 Hz ticks, 1 in the first 15
    /// game ticks: the boost budget `chase+0x7C` (0x40CE46), the formation timer `chase+0x78`
    /// (0x40D2A3), the stuck timer `chase+0x90` (0x40DAED) and the lane-existence scan
    /// `car+0x101C` (§23.4). *(Until 2026-09-25 every cop updated on every 32 Hz tick and each of
    /// these stepped by 2, so the stuck limit of 160 — 5 s — was reached in 2.5 s and the 16 s /
    /// 32 s boost budgets lasted 8 s / 16 s.)*
    pub fn ai(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar], humans: &[usize], rand16: &mut dyn FnMut() -> u32) {
        let (base, dist): (usize, Vec<f32>) = (self.slot_base, self.cars.iter().map(|c| c.racer_distance).collect());
        self.run(table, tick, target, others, Some((humans, rand16)), &|i| crate::traffic::ai_pass_due(dist[i], false, tick, base + i));
    }

    /// The AI half with every cop due on every call, `[0x5E6654]` = [`TEST_AI_TICKS`], then
    /// [`Police::drive`] — the unit tests' entry with the searches.
    pub fn tick(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar], humans: &[usize], rand16: &mut dyn FnMut() -> u32) {
        self.run(table, tick, target, others, Some((humans, rand16)), &|_| Some(TEST_AI_TICKS));
        self.drive(table, tick);
    }

    /// `0x417190`, run by each cop's tracker 0x49AF70 at Sc32 priority 6 (after the AI pass):
    /// `car+0x78` = the octagonal x–z distance to the nearest racer (`0x5E8E2C[]`, from 1000,
    /// `[0x5B2468]` = 0.25), for the cops in the world — the same rule as
    /// `TrafficSet::track_racers`. An idle cop keeps the distance it last had.
    pub fn track_racers(&mut self, racers: &[Vec3]) {
        for c in self.cars.iter_mut().filter(|c| c.active) {
            c.racer_distance = racers.iter().map(|&r| crate::traffic::oct_distance(c.position, r)).fold(1000.0, f32::min);
        }
    }

    /// The drive half of a tick: `0x408360` for every cop in the world, in index order — a cop's
    /// `car+0xE1C` on the Sc32 list at priority 0x1E, inserted car by car in `0x5E8B34` order after
    /// the human's `Car_simulate` and the opponents' (returning) entries and before the traffic's
    /// (0x411277…0x411349, `traffic.md` §17.5e). It reads the targets this tick's AI pass wrote —
    /// or an earlier one's, for a cop the pass skipped — and writes the velocities the 64 Hz
    /// integrators move.
    ///
    /// **The gate.** 0x408360 heads with its own `0x414B10(1, 1, car+0x78, car+0, &[0x5E6D00],
    /// &[0x5E6D78], &[0x5E6D7C], car+0x10B8)` (0x408373…0x40839E, `traffic.md` §14.3) and returns
    /// unless the car is due — for a cop as for traffic, the only class test before it being the
    /// racer bit (0x408366): row 1, **32 Hz** (`[0x5E6D78]` = 1, `dt` = 1/32) while `car+0x78`
    /// ([`CopCar::racer_distance`], written by [`Police::track_racers`]) is under 160 m, **16 Hz**
    /// (`[0x5E6D78]` = 2, `dt` = 1/16, mask 1) beyond, phased by the slot `slot_base + i`
    /// ([`crate::traffic::drive_due`]). In between the cop keeps the velocity it has. `[0x5E6D78]`
    /// is what `car+0x1080` (0x40A5E7) and the brake light (0x409AC8 / 0x409B65, 0x40A0BC /
    /// 0x40A124) step by. *(Until 2026-09-25 every cop drove on every 32 Hz tick with `dt` 1/32 and
    /// both counters stepping by 2 — twice the original's rate near a racer.)*
    pub fn drive(&mut self, table: &SliceTable, tick: u64) {
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                continue;
            }
            if let Some((dt, n_ticks)) = crate::traffic::drive_due(self.cars[i].racer_distance, false, tick, self.slot_base + i) {
                self.drive_car(table, tick, i, dt, n_ticks);
            }
        }
    }

    fn run(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar], mut spawn: Option<(&[usize], &mut dyn FnMut() -> u32)>, due: &dyn Fn(usize) -> Option<i32>) {
        // `AIHigh+0x60`: the chasers are re-sorted by track distance (0x403C00) and each one's place
        // in that queue picks its formation slot. **Rank 0 is the cop furthest ahead**: the insert
        // loop walks the roster down while the entry it passes is more than twelve metres *behind*
        // the target (`0x40E740(cop, perp.slice)` is the signed gap from the perp to that cop,
        // 6 m a slice, wrapped into half a lap, and 0x403C66 compares it with −12). CONFIRMED —
        // which is also what slot 0 wants, since it sits 8 m ahead of the target.
        // A cop in state 9 has given its slot back (0x40503A → 0x403BA0), so it is not in the queue,
        // and neither is one with no target: the slot is claimed by `0x405220` (0x4053C3), i.e. by
        // a chaser or a patrol car that builds its chase this tick — not by one state 1 has just
        // placed.
        let wanted = self.wanted;
        let mut order: Vec<usize> = (0..self.cars.len())
            .filter(|&i| {
                let c = &self.cars[i];
                c.active && c.goto_slice.is_none() && !c.placed && (c.chasing || wanted)
            })
            .collect();
        order.sort_by(|&a, &b| track_distance(table, self.cars[b].slice, target.slice).partial_cmp(&track_distance(table, self.cars[a].slice, target.slice)).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, &i) in order.iter().enumerate() {
            self.cars[i].rank = rank;
        }
        for i in 0..self.cars.len() {
            // 0x406745…0x406770: the AI pass's gate, and `[0x5E6654]` for this update
            let Some(ai_ticks) = due(i) else { continue };
            if !self.cars[i].active {
                // state 1: search, and on a record place the car (0x407890, [`Police::deploy_as`])
                // and run the new state's update at once (0x404243) — the `placed` arm below
                let Some((humans, rand16)) = spawn.as_mut() else { continue };
                let Some(rec) = self.find_spawn_point(table, tick, humans, &mut **rand16) else { continue };
                let wanted = self.wanted;
                if !self.deploy_as(table, i, rec, tick, wanted) {
                    continue;
                }
            }
            // State 9, `AIState_GotoSlice` (arm 0x40501E): the cop gives its slot on the perp back
            // (0x403BA0) and drops the target (`+0x58 = 0`), then asks `0x40E330(state, 10.0)` —
            // `|0x40E740(car, state+0x08)| < 10`, the signed track gap to the goal slice. Once it
            // holds, an `AIState_Normal` (0x40B5D0) replaces the GotoSlice, the state becomes **2**
            // and 0x405092 runs the *new* state's update in the same tick, which is the patrol drive.
            // Until then the GotoSlice keeps driving. The arm has none of state 4's give-up tests,
            // so this comes before them.
            if let Some(goal) = self.cars[i].goto_slice {
                if track_distance(table, self.cars[i].slice, goal).abs() < GOTO_SLICE_ARRIVED {
                    self.cars[i].goto_slice = None;
                    self.cars[i].chasing = false;
                    // `AIState_Normal::update` 0x40B5A0 never writes `car+0xEAC`, so the direction
                    // the GotoSlice asked for (0x40E1F8 / 0x40E207) stands, and 0x408360 keeps
                    // granting it below 8.889 m/s until the next chase writes its own.
                    // The lights stay on through this tick (nothing in state 9 or `AIState_Normal`
                    // touches them); state 2's head (0x4042DA) clears them next tick, before
                    // `0x405220` re-acquires the still-wanted perp and a fresh chase is built.
                    self.patrol(table, i, target, others, ai_ticks);
                    tracing::debug!(cop = i, "police GotoSlice reached; back to state 2");
                } else {
                    self.goto_slice(table, i, target, others, ai_ticks);
                }
                continue;
            }
            // State 1's type-1 arm (0x40419B…0x4042BE) placed the cop this tick and ran the new
            // state's update itself (0x404243) — `AIState_Normal` (state 2) or `AIState_Stop` mode 1
            // (state 3), lights off from state 1's head (0x403F4A) — then returned: no acquire and
            // no give-up test until the next tick. §17.5b.
            if std::mem::take(&mut self.cars[i].placed) {
                self.patrol(table, i, target, others, ai_ticks);
                continue;
            }
            let mut built_parked = false;
            if self.cars[i].chasing {
                // state 4's head (0x40444C…0x404455) sets `car+0xECC & 2`, the lights, before its
                // give-up tests
                self.cars[i].lights = true;
                // State 4 runs `0x405220` itself (0x40471F). With no wanted perp left both of its
                // passes come up empty (pass 1's `lim` is 0 for a clean perp, pass 2 asks for
                // `perp+0x78 != 0`), and since the cop still holds a target it releases the slot
                // (0x403BA0) and clears `+0x58` (0x4053E9…0x405424). 0x404795 then sees the target
                // gone and takes 0x4047D9, which builds an
                // `AIState_Normal`, deletes the chase (its dtor 0x40CC10 resets the aim point and
                // the boost) and sets **state 2** — no `car+0xEAC` write, the lights left on this
                // tick. The give-up test (0x404840) still runs, and then 0x405092 runs the Normal's
                // update, i.e. the patrol drive. 0x40479F sends a **finished** target
                // (`tgt+0xB50 >= 2`) down the same exit; with one human it is the drop that fires
                // first, since the perp's own tick (0x4059C0, before any cop's) has already
                // cleared `+0x78` — see [`perp_update`].
                // A different perp would re-rank and re-aim the chase (0x404728…0x40477F, 0x40CCA0);
                // only the human can be wanted, so the same one is kept (`best == old`, 0x40536F).
                let got = self.acquire(table, i, target);
                if got.is_some() {
                    self.cars[i].target = got;
                }
                if got.is_none() || target.finished {
                    self.drop_chase(i);
                    tracing::debug!(cop = i, "police target dropped; back to state 2");
                }
            } else {
                // State 2's head (0x4042DA) / state 3's (0x404D75) clear the lights; then
                // `0x405220` (0x4042F8 / 0x404D86) looks for a perp. With one human its second pass
                // (0x4052FA…) takes the wanted perp whatever the slot count says, so every patrol
                // car gets it; the chase is built (0x4043A8 / 0x404E4B, state 4) and its update runs
                // this very tick (0x405092 / 0x404E70) with the lights still off. The orders
                // (`AIHigh+0x20`) that could also move state 3 are never set in this build (§17.5b).
                self.cars[i].lights = false;
                if let Some(p) = self.acquire(table, i, target) {
                    // 0x4053AC / 0x4053C3: `+0x58` and the claim on the perp
                    self.cars[i].target = Some(p);
                    built_parked = self.cars[i].parked;
                    self.build_chase(i);
                }
            }
            // §17.5 give-up: too far (`0x4071E0` — in state 4 at 0x40484D, in state 2 at 0x4043D6
            // also right after it built a chase, in state 3 at 0x404E80 only when it built none).
            // A finished perp is **not** a give-up: the cop dropped to state 2 above and patrols
            // until this test sends it to purgatory. *(Until 2026-09-25 the port retired it.)*
            // `0x4071E0(car)` measures to `car+0xED4`, the reference car — the racer that led when
            // the cop was placed — not to the target: octdist over X/Z (`+0x330` / `+0x338`, the
            // 0.25 at `[0x5B2468]`) `> 216.0`. *(Until 2026-09-25 the port measured to the target.)*
            let (ref_pos, _) = self.reference_of(i, target);
            if crate::traffic::oct_distance(self.cars[i].position, ref_pos) > GIVE_UP_DISTANCE && !built_parked {
                self.retire(i);
                continue;
            }
            if !self.cars[i].chasing {
                self.patrol(table, i, target, others, ai_ticks);
                continue;
            }
            // Too long unable to reach the target's lane (`chase+0x90 > 160`, counted by `0x40DA00`):
            // the original does **not** drop the cop. State 4 (0x4044FC…0x404548) builds an
            // `AIState_GotoSlice(car, [[car+0xED4]+8], 0)` — the **reference car's** slice — deletes
            // the chase through `[vt+4]`, and sets state 9. The rest of the state-4 arm then runs on
            // (its give-up tests are the ones above) and 0x405092 drives the GotoSlice this very
            // tick. The port retired the cop here until 2026-09-25 and then restarted the chase in
            // place, without the GotoSlice and Normal ticks in between, and until 2026-09-25 (later)
            // aimed the GotoSlice at the cop's own slice, reading `car+0xED4` as the cop itself.
            // (0x404551… keeps using `ebx`, the chase it has just freed — 0x40CCA0 / 0x40CD50 write
            // into freed memory; nothing reads it again, so there is nothing to port.)
            if self.cars[i].stuck_ticks > STUCK_LIMIT {
                let (_, goal) = self.reference_of(i, target);
                let c = &mut self.cars[i];
                c.goto_slice = Some(goal);
                // state 9 gives its slot back and drops the target (0x40503A → 0x403BA0)
                c.target = None;
                c.stuck_ticks = 0;
                // the Chase's deleting dtor 0x40CC10 zeroes `car+0x1064…0x1070`, `+0x1078`, `+0x107C`
                // (the ram aim point) and puts `car+0x1098` / `+0x109C` back to 1.0
                c.aim_point = Vec3::ZERO;
                c.boost_engine = 1.0;
                c.boost_speed = 1.0;
                tracing::debug!(cop = i, "police stuck 160 ticks: GotoSlice to the reference car's slice (state 9)");
                self.goto_slice(table, i, target, others, ai_ticks);
                continue;
            }

            // §27.5 geometry
            let gap = track_distance(table, self.cars[i].slice, target.slice);
            let lateral_err = self.cars[i].lateral - target.lateral;
            let sign_speed = if self.cars[i].speed >= 0.0 { 1.0 } else { -1.0 };
            let sign_lat = if lateral_err >= 0.0 { 1.0 } else { -1.0 };
            self.cars[i].aim_point = Vec3::ZERO;
            // 0x40CD70's tail: charge the boost the last update left on, and reset it
            self.spend_boost(i, ai_ticks);
            // §27.4 / 0x4046CE: inside the level's close-in gap the chase latches the ram mode for
            // 8 / 12 / 18 s, and `0x40CD50` only latches it while it is not already running.
            let lvl = self.cars[i].level.min(2);
            if tick >= self.cars[i].ram_until && gap.abs() < CLOSE_IN_GAP[lvl] {
                self.cars[i].ram_until = tick + CLOSE_IN_TICKS[lvl];
            }

            // §27.6 dispatch
            let dv = self.cars[i].speed - target.speed;
            let adv = dv.abs();
            let near = gap.abs() * 0.025;
            let slow = target.speed.abs() < 6.667;
            let mode = if near > adv && gap.abs() > 140.0 {
                ChaseMode::CatchUp
            } else if sign_lat * sign_speed == 1.0 && !slow {
                ChaseMode::CloseQuarters
            } else if sign_speed * gap >= 0.0 && !slow {
                ChaseMode::CatchUp
            } else {
                ChaseMode::Pursue { hard: adv > 26.67 && gap.abs() > 80.0 }
            };
            self.cars[i].mode = Some(mode);
            // CATCH-UP (0x40D049 / 0x40D055) and PURSUE (0x40D6FE / 0x40D70A) point the car at the
            // target — `chase+0x68 > 0` → −1, else +1, behind `gGameTick > chase+0x44`, which only
            // the ctor writes (0), so always; CLOSE-QUARTERS writes no `cop+0xEAC` at all, so a cop
            // holding a slot ahead of the target keeps whatever was asked last. Every behaviour
            // spends the boost.
            if !matches!(mode, ChaseMode::CloseQuarters) {
                self.cars[i].requested_dir = if gap > 0.0 { -1.0 } else { 1.0 };
            }
            self.apply_boost(i, tick, gap);

            let mut speed = self.base_target_speed(&self.cars[i], target.speed);
            match mode {
                // §27.7: flat out, no cap at all
                ChaseMode::CatchUp => {}
                // §27.8, the formation controller in full: compare the sector the cop is in with
                // the one its formation slot wants, pick a speed to close the difference and bias
                // the lateral aim toward the slot.
                ChaseMode::CloseQuarters => {
                    let lvl = self.cars[i].level.min(2);
                    let dir = self.cars[i].dir;
                    let (hw, hl) = (target.half_width, target.half_length);
                    let ram = tick < self.cars[i].ram_until;
                    let offset = FORMATION[lvl][self.cars[i].rank.min(5)];
                    // `chase+0x58` / `+0x5C`: the sectors the offset itself falls in. In the ram mode
                    // the slot is "dead on" and the boost is not suppressed (0x40D194).
                    let (want_lon, want_lat) = if ram { (0, 0) } else { (sector(offset[2], -hl, hl), sector(offset[0], -hw, hw)) };
                    let lat = sector(dir * lateral_err, -hw, hw);
                    let g = dir * gap;
                    let p = sector(g, 2.0 - hl, hl + 2.0);
                    let lon = sector(g, -(hl + 2.0), hl + 2.0);
                    self.cars[i].slot_ticks = if lat == want_lat && p == want_lon { self.cars[i].slot_ticks + ai_ticks } else { 0 };
                    let (mut ok, mut m, mut sub) = (true, 0, 0);
                    if want_lon * lon == -1 {
                        m = if want_lat != 0 { want_lat } else { -1 };
                    } else if want_lat * lat == -1 && lon == 0 {
                        sub = -1;
                        ok = false;
                    } else if want_lat == 0 && lon == 0 && !ram {
                        ok = false;
                    } else if want_lon == 1 && lon == 1 && lat == 0 && gap < 20.0 {
                        sub = -2;
                    }
                    // the target speed the slot implies
                    let mut want = target.speed.abs();
                    if p < want_lon {
                        want *= 1.4; // behind the slot: chase
                    } else if p > want_lon || sub == -1 {
                        want *= close_quarters_factor(gap); // ahead of it: ease off
                    } else if gap.abs() <= 20.0 {
                        if sub == -2 {
                            want *= CLOSE_QUARTERS_SLOW[lvl];
                        }
                    } else {
                        want *= close_quarters_factor(gap);
                    }
                    // `cop+0xEB8 = dir · max(5, min(S, |cop+0xEB8|))`
                    speed = dir * want.min(speed.abs()).max(5.0);
                    if ok {
                        let y = if ram {
                            self.cars[i].aim_lateral = 30.0;
                            target.lateral
                        } else {
                            self.cars[i].aim_lateral = 15.0;
                            // `+ dir·(half width + 6)` when the cop has to swing out, else the
                            // slot's own lateral offset; then clamped onto the road
                            let y = target.lateral + if m != 0 { dir * (hw + 6.0) } else { dir * offset[0] };
                            let sl = &table.slices[self.cars[i].slice.min(table.slices.len() - 1)];
                            y.min(sl.width_b).max(-sl.width_a)
                        };
                        self.cars[i].lateral_target = y;
                    }
                }
                // §27.9: the distance ladder is a cap on the 100 m/s base, and `hard` rams
                ChaseMode::Pursue { hard } => {
                    let d = crate::traffic::oct_distance(self.cars[i].position, target.position);
                    let v = pursue_cap(self.cars[i].level, d);
                    speed = if self.cars[i].dir >= 0.0 { speed.min(v) } else { speed.max(-v) };
                    if hard {
                        self.cars[i].aim_point = target.position;
                        self.cars[i].aim_lateral = target.lateral;
                    }
                }
            }
            // §27.7's U-turn needs nothing here: asking for the opposite direction makes the
            // throttle windows fail (`car+0xEAC == car+0xEB0` at 0x409AAF / 0x40A09B, which
            // `aidrive` carries), so the cop brakes to a stop, 0x408360 flips it and it drives
            // back. `car+0xEB8` itself keeps what `0x40AF70` and the behaviour gave it. *(Until
            // 2026-09-25 the port also zeroed the target speed here, which the original does not;
            // the lane pass saw the zero.)*
            self.cars[i].target_speed = speed;

            // §12/§23 lane pass, with §27.10's `0x40DA00` in front of the scorers: every behaviour
            // calls it (0x40D07F, 0x40D653, 0x40D985), and it runs the stuck timer
            self.lane_pass(table, i, target, others, true, ai_ticks);
        }
    }

    /// The lane scorers plus `0x40DA00`'s bias toward the target's lane.
    /// One tick of a patrolling cop: the traffic speed ladder, the same lane pass and the same drive
    /// model, with no chase behaviour at all (§17.4 states 2/3).
    ///
    /// `AIState_Normal::update` 0x40B5A0 writes no `car+0xEAC`: the requested direction is whatever
    /// the last writer left — the placement 0x407890 (the car's own direction) after a deploy, the
    /// GotoSlice after state 9. *(Until 2026-09-25 this reset it to `car+0xEB0` every tick.)*
    fn patrol(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, others: &[LaneCar], ai_ticks: i32) {
        if self.cars[i].parked {
            // `AIState_Stop::update` 0x40CAB0, mode 1 (0x404295 → `+0x0C = 1`): `car+0xEB8 = 0`,
            // `car+0xEB4 = car+0xED0` and nothing else — no `0x40AF70`, no lane pass
            let c = &mut self.cars[i];
            c.mode = None;
            c.target_speed = 0.0;
            c.lateral_target = c.lateral;
            return;
        }
        let speed = self.base_target_speed(&self.cars[i], target.speed);
        {
            let c = &mut self.cars[i];
            c.mode = None;
            c.target_speed = speed;
            c.boost_engine = 1.0;
            c.boost_speed = 1.0;
            c.aim_point = Vec3::ZERO;
        }
        // `AIState_Normal::update` 0x40B5A0 is `0x40AF70`, `0x401000`, `0x401060`, `0x4012B0` and
        // nothing else: no `0x40DA00`, so a patrol car gets no pull toward the target's lane
        self.lane_pass(table, i, target, others, false, ai_ticks);
    }

    /// One tick of `AIState_GotoSlice::update` **0x40E1D0** (state 9). `0x40E7A0(goal, car)` is
    /// `(goal − car.slice)·6` wrapped into half a lap; its sign sets `car+0xEAC` (`< 0` → −1, else
    /// +1, 0x40E1E5…0x40E207), and then the same four calls as `AIState_Normal` run — `0x40AF70`,
    /// `0x401000`, `0x401060`, `0x4012B0` — with no `0x40DA00`. The tail from 0x40E235 caps
    /// `car+0xEB8` by the gap (`< 4 → 4`, `< 20 → 20`, `< 40 → 40`, `< 80 → 80`, else 200 m/s:
    /// `[0x5B240C]`/`[0x5B2404]`, `[0x5B251C]`/`[0x5B2470]`, `[0x5B248C]`/`[0x5B2410]`,
    /// `[0x5B25C4]`/`[0x5B26D4]`, default `[0x5B2484]`) only when `state+0x0C` is set, and the one
    /// constructor call (0x40452C) passes **0**, so it never runs and is not ported.
    ///
    /// The lights stay on (only state 2's head clears them), so `0x40AF70` still hands out the
    /// pursuit base speed, at the 1.0 boost the chase's dtor left.
    fn goto_slice(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, others: &[LaneCar], ai_ticks: i32) {
        let Some(goal) = self.cars[i].goto_slice else { return };
        let d = track_distance(table, goal, self.cars[i].slice);
        // a zero gap is +1 in the original's slice order, which a backwards race's reversed table
        // runs the other way
        let req = if d > 0.0 {
            1.0
        } else if d < 0.0 {
            -1.0
        } else if table.backwards {
            -1.0
        } else {
            1.0
        };
        // the U-turn is the throttle gates' (§27.7), as in the chase
        let speed = self.base_target_speed(&self.cars[i], target.speed);
        {
            let c = &mut self.cars[i];
            c.mode = None;
            c.requested_dir = req;
            c.target_speed = speed;
        }
        self.lane_pass(table, i, target, others, false, ai_ticks);
    }

    /// §27.2's drive model, the same one traffic runs — `0x408360` for cop `i`.
    /// `dt` / `n_ticks` are `[0x5E6D7C]` / `[0x5E6D78]` from the gate ([`Police::drive`]).
    fn drive_car(&mut self, table: &SliceTable, tick: u64, i: usize, dt: f32, n_ticks: i32) {
        // `0x408360` grants the requested direction first (0x4083B6…0x4083CF: `car+0xEB0 =
        // car+0xEAC` while `car+0xEC4 < 8.889`, `[0x5B2430]`) and only then runs the drive chain
        // (0x40A900, 0x4082E0, …) — so a U-turn asked for by this tick's state (0x406720 runs
        // before the Sc32 scheduler, `timing.md` §7) is granted in time for this tick's throttle
        // gate. *(Until 2026-09-25 the port granted it after integrating, one tick late.)*
        {
            let c = &mut self.cars[i];
            if c.speed_estimate() < 8.889 {
                c.dir = c.requested_dir;
            }
        }
        let sl = table.slices[self.cars[i].slice.min(table.slices.len() - 1)].clone();
        let st_road = (sl.width_a, sl.width_b);
        // The exits of 0x40852A…0x408646, taken before the drive chain (`traffic.md` §21.7): a cop
        // whose `car+0x418` is 0 and whose up axis `car+0x374` is below 0.3 (`[0x5B25F0]`,
        // 0x40854C) — on its side or its roof — or whose target speed `car+0xEB8` is exactly 0
        // (0x4085EC) is only damped: `vel ×= 0.94` (`[0x5B25F4]`… 0x40863C, 0x532360) and
        // `ω.y ×= 0.98` (0x408512), and returns. The damaged-out exit (`car+0x540`, 0.99) has no
        // writer for a cop and the finished-racer exit needs `car+0x52C & 1`; the knocked-over
        // one (`car+0x42C == 1`, 0.98) needs `Damage`, which the port's cops do not take (gap).
        // *(Until 2026-09-25 the cops drove through all of them.)*
        {
            let c = &mut self.cars[i];
            if (c.airborne_count == 0 && c.orientation.row(1).y < 0.3) || c.target_speed == 0.0 {
                c.velocity *= 0.94;
                c.angular_velocity.y *= 0.98;
                return;
            }
        }
        // §27.2a, exactly as for traffic: trim while reversing, then flip the gear every 32 ticks
        // for as long as the car is off the lanes and tipped (or already backing up)
        let tick = tick as i64;
        let lane_target = {
            let c = &mut self.cars[i];
            let lane = lane_of(&sl, c.lateral);
            let up_y = c.orientation.row(1).y;
            let lat_vel = c.velocity.dot(sl.lateral.normalize_or_zero());
            // `0x40A470(car, 1)` (0x40842C) copies the lane target `car+0xEB4` into the one the aim
            // point follows, `car+0x1074`, on every drive; 0x408E30 clamps `car+0xEB8` and zeroes
            // `car+0x1074` while reversing — not `car+0xEB4`, so the lane pass's target is back on
            // the next drive. *(Until 2026-09-25 the port wrote the zero into `car+0xEB4`.)*
            let (speed, mut target) = aidrive::reverse_trim(&mut c.gear, tick, c.lateral, c.lateral_target, lat_vel, c.target_speed);
            c.target_speed = speed;
            // 0x408665…0x40868A: `(int)car+0xEB0 · car+0xEC0 < 0` — the car moving against its own
            // direction, as in a U-turn — with the gear not reversed: `car+0x1074 = 0`, so it aims
            // at the centre line. *(Not in the port before 2026-09-25: a cop turning round after a
            // deploy kept aiming at its old lane on the far side, ran wide off the road.)*
            if c.dir * c.speed < 0.0 && c.gear.sense >= 0.0 {
                target = 0.0;
            }
            // `car+0x1080` steps by `[0x5E6D78]` (0x40A5E7)
            if aidrive::in_trouble(&mut c.gear, lane, sl.lanes_a as i32, sl.lanes_b as i32, up_y, n_ticks) {
                aidrive::toggle_gear(&mut c.gear, tick, c.speed, aidrive::GEAR_PERIOD);
            }
            target
        };
        let mut st = self.cars[i].as_ai_state(self.phys.max_lat_acc, self.phys.max_yaw_acc);
        st.lateral_target = lane_target;
        let aim = aidrive::aim_direction(table, &self.cfg, &st);
        let err = aidrive::heading_error(&st, aim);
        // `0x4081B0`'s speed curve is the traffic column only: a cop's base rate is a flat 1.0,
        // scaled by the pursuit boost `car+0x1098` and then, whatever the car's class, by 0.9 under
        // `WEATHER` (0x40825C). Two further factors of that function are **not** applied here and
        // are left out deliberately rather than approximated:
        //   * the HARD_BRAKE script action (`0x40AC60(car+0xE3C, 4)` → ×1.5 at 0x40829B) — the port
        //     runs `scripts.bin` for traffic, not for cops;
        //   * the early-race boost `0x40AEF0` (×`1 + opponents·0.2` in the first 30 s, and a cop
        //     takes the *full* factor because its `car+0x10D0` ordinal is 0). It compares
        //     `human+0xAF8` with `car+0xAF8`, the **lap fraction**, and neither `ChaseTarget` nor
        //     `CopCar` carries one — only a slice index, which is not the same thing across a lap
        //     boundary. It also needs more than one opponent, so it cannot fire in a pursuit race
        //     with none.
        let accel = crate::traffic::accel_rate_for(false, st.speed, self.cars[i].boost_engine)
            * if self.weather { 0.9 } else { 1.0 };
        let bend = aidrive::road_bend(table, self.cars[i].slice, self.cars[i].dir);
        let cmd = if aidrive::out_of_control(&st, &self.cfg.out_of_control, err, st_road, bend) {
            aidrive::out_of_control_command(&st, &self.cfg.out_of_control, err, accel, self.phys.brake_force)
        } else {
            let grip = 1.0 + 0.0015 * aidrive::body_forward_speed(&st);
            aidrive::in_control_command(&st, &self.cfg.in_control, err, aim, grip, self.cars[i].boost_engine, accel, self.phys.brake_force)
        };
        {
            let c = &self.cars[i];
            tracing::trace!(cop = i, tick, slice = c.slice, dir = c.dir, lat = c.lateral, lat_t = st.lateral_target, speed = c.speed, tgt = c.target_speed, up_y = c.orientation.row(1).y, pos = ?c.position, err, sense = c.gear.sense, "cop drive");
        }
        // the step covers `[0x5E6D7C]` (the integrator 0x40A1D0's dt)
        aidrive::integrate(&mut st, &cmd, dt);
        let c = &mut self.cars[i];
        // `[0x5E6D78]`: the 32 Hz ticks this step covers
        c.brake_light = aidrive::brake_light(c.brake_light, cmd.braking, n_ticks as u8);
        c.velocity = st.velocity;
        c.angular_velocity = st.angular_velocity;
    }

    /// `AIHigh_Perp+0x78`: whether the perp is wanted. A patrol car picks that up in its own next
    /// tick (`update`, state 2/3's `0x405220`). *(Until 2026-09-25 this promoted every patrol car
    /// on the spot; the chase is now built where the original builds it, which also covers a cop
    /// state 1 has only just placed — it waits for its first state-2/3 tick.)*
    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    /// State 2 → 4 (0x4043A8) / 3 → 4 (0x404E4B): a new `AIState_Chase` (ctor 0x40CB60) — the boost
    /// budget `+0x7C` from `0x5CBEA0[lvl]+0x10`, the blackout `+0x8C = 0` (0x40CBD8), the stuck
    /// timer `+0x90 = 0` (0x40CBF1) and `car+0x1098` / `+0x109C = 1.0` (0x40CBA6). The lights are
    /// not touched: they come on at the next tick's state-4 head.
    fn build_chase(&mut self, i: usize) {
        let c = &mut self.cars[i];
        c.chasing = true;
        c.parked = false;
        c.boost_budget = BOOST_BUDGET[c.level.min(2)];
        c.blackout_until = 0;
        c.stuck_ticks = 0;
        c.boost_engine = 1.0;
        c.boost_speed = 1.0;
    }

    /// State 4 → 2 (0x4047D9): the target is gone, an `AIState_Normal` replaces the chase, whose
    /// deleting dtor 0x40CC10 zeroes `car+0x1064…0x1070`, `+0x1078`, `+0x107C` and puts
    /// `car+0x1098` / `+0x109C` back to 1.0. `car+0xEAC` is not written (the 0x4047CA store sits on
    /// the dead order branch), and the lights are left to state 2's head next tick.
    fn drop_chase(&mut self, i: usize) {
        let c = &mut self.cars[i];
        c.chasing = false;
        c.target = None;
        c.mode = None;
        c.aim_point = Vec3::ZERO;
        c.boost_engine = 1.0;
        c.boost_speed = 1.0;
    }

    /// The §12/§23 scorers. `chase` adds `0x40DA00` (§27.10), which only `AIState_Chase`'s three
    /// behaviours call, and which is also what counts `chase+0x90`.
    fn lane_pass(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, others: &[LaneCar], chase: bool, ai_ticks: i32) {
        let sl = table.slices[self.cars[i].slice.min(table.slices.len() - 1)];
        let mut cars: Vec<LaneCar> = others.to_vec();
        cars.extend(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.as_lane_car(table)));
        let c = &self.cars[i];
        let me = LaneSelf {
            slice: c.slice,
            lane: lane_of(&sl, c.lateral),
            lateral: c.lateral,
            dir: c.dir,
            speed: c.speed,
            target_speed: c.target_speed,
            half_width: c.half_extents.x,
            half_len: c.half_extents.z,
            // a cop is `car+0x52C & 0x20`, not `& 0x10`: it pays the opponent's penalties, not traffic's
            is_traffic: false,
            travelled: 1000.0,
        };
        let mut sc = LaneScratch::default();
        let mut scan = c.scan;
        sc.find_blockers(table, &me, &cars, &PERSONALITIES[0]);
        sc.score_own_side(&me, self.one_way, true);
        // §27.10 `0x40DA00`: +15 to whichever side the target is on (`0x401380` × 3), but only when
        // the lane corridor between the two cars is broken at the target's slice and whole at the
        // cop's; the stuck timer counts while it is broken at both. `chase+0x68` is the longitudinal
        // gap 0x40CD70 measures; the port stands the track gap in for it, as the dispatch does.
        let mut stuck = self.cars[i].stuck_ticks;
        if chase {
            let gap = track_distance(table, me.slice, target.slice);
            match lane_bias(table, me.slice, me.lane, target.slice, target.lane, gap) {
                LaneBias::Skip => {}
                LaneBias::Clear => stuck = 0,
                LaneBias::Blocked => stuck += ai_ticks,
                LaneBias::Toward => {
                    stuck = 0;
                    match target.lane.cmp(&me.lane) {
                        std::cmp::Ordering::Greater => sc.score[2] += 15.0,
                        std::cmp::Ordering::Less => sc.score[0] += 15.0,
                        std::cmp::Ordering::Equal => {}
                    }
                }
            }
        }
        sc.score_blockers(&me);
        sc.score_abreast(table, &me, &cars);
        sc.score_lane_exists(table, &me, &mut scan, ai_ticks);
        sc.score_narrowing(table, &me);
        sc.score_shoulders(table, &me, &cars);
        let (slot, lane) = sc.pick(&me);
        let lateral_target = sc.lateral_target(table, &me, lane, slot, &cars);
        let capped = sc.capped_target(&me, lane, lateral_target, 0.0, &cars);
        let c = &mut self.cars[i];
        c.scan = scan;
        c.stuck_ticks = stuck;
        // a ram aim point overrides the lane target
        if c.aim_point == Vec3::ZERO {
            c.lateral_target = lateral_target;
        }
        c.target_speed = capped;
    }

    /// The 64 Hz half: move, turn and re-derive the route frame, exactly as traffic does.
    pub fn integrate(&mut self, table: &SliceTable, dt: f32) {
        self.integrate_on(table, dt, None)
    }

    /// The same step with the track's collision grid: a cop keeps `car+0xE20 = Car_ground` 0x49BD40
    /// exactly as traffic does (§14.1), so it follows the surface under it.
    pub fn integrate_on(&mut self, table: &SliceTable, dt: f32, ground: Option<&nfs_physics::collision::TriangleGrid>) {
        for c in self.cars.iter_mut().filter(|c| c.active) {
            c.position += c.velocity * dt;
            let yaw = c.angular_velocity.y * dt * std::f32::consts::TAU;
            if yaw != 0.0 {
                c.orientation = (Mat3::from_rotation_y(yaw) * c.orientation.transpose()).transpose();
            }
            c.slice = table.find_slice(c.position, c.slice);
            let sl = &table.slices[c.slice];
            let lat_axis = sl.lateral.normalize_or_zero();
            c.lateral = (c.position - sl.centre).dot(lat_axis);
            c.speed = c.velocity.dot(sl.forward.normalize_or_zero());
            let rest = c.rest_height();
            let Some(g) = ground else {
                c.position.y = sl.centre.y + rest;
                c.velocity.y = 0.0;
                continue;
            };
            // `Car_ground` 0x49BD40 → `Susp_update` 0x499A70's height and normal. The car tracker
            // first (0x49AF70, Sc32 priority 6): `updateZone(pos)` and, on a new patch, the plane
            // 0x499520 — or, on the first step after a deploy, `Car_placeAtSlice`'s seat.
            if c.seated {
                c.tracker.track(g, c.position);
            } else {
                c.tracker.seat(g, c.position);
                c.seated = true;
            }
            // Each probe (0x499AD4…): `updateZone(probe)` on its wheel tracker; a patch counts only
            // with a surface nibble and within 4 m of `car+0x334` (0x494480, `[0x5B2404]`, exactly 4
            // is usable) — otherwise the car tracker's kept plane (`car+0x3E8` when `+0x3EC > 0.1`,
            // else `(0,1,0)`, through `car+0x400`). No roughness: 0x499730 returns 0 below
            // `cosTilt` 0.86 and the port has no per-wheel state for it (APPROXIMATION).
            // *(Until 2026-09-25 the port took the highest triangle of any kind 2…8 m under the
            // model's ground plane and turned the body onto its normal — on coastal a cop that ran
            // off the shoulder met the cliff face (surface word 0, normal y 0.26), was stood on its
            // side, and then kept that tilt over the drop at the slice's height.)*
            let m = c.orientation;
            let mut contact = [Vec3::ZERO; 4];
            let mut n_air = 0;
            for (k, wt) in c.wheel_trackers.iter_mut().enumerate() {
                let p = c.probes[k];
                let probe = c.position + m.row(0) * p.x + m.row(1) * p.y + m.row(2) * p.z;
                wt.update_zone(g, probe);
                let h = match wt.patch_height(probe.x, probe.z) {
                    Some(h) if wt.word & 0xF != 0 && (h - c.position.y).abs() <= 4.0 => h,
                    _ => c.tracker.plane_height(probe.x, probe.z),
                };
                // §3.4: a probe more than 9.8/48 above its ground hangs
                if h - probe.y < -0.204_166_7 {
                    n_air += 1;
                }
                contact[k] = Vec3::new(probe.x, h, probe.z);
            }
            // `N = normalize(normalize(F) × normalize(R))`, `F` front − rear and `R` right − left of
            // the contacts, `(0,1,0)` when it points down; `N.y < 0.5` counts every probe hanging
            let f = ((contact[0] + contact[1]) - (contact[2] + contact[3])) * 0.5;
            let r = ((contact[1] + contact[3]) - (contact[0] + contact[2])) * 0.5;
            let mut up = f.normalize_or_zero().cross(r.normalize_or_zero()).normalize_or_zero();
            if up.y < 0.0 || up == Vec3::ZERO {
                up = Vec3::Y;
            }
            if up.y < 0.5 {
                n_air = 4;
            }
            c.airborne_count = u16::from(n_air > 1);
            // The body on that plane through `Σc/4` (0x49A8CF…0x49A8DD): the port's resting arm —
            // the origin `rest_height / N.y` above the plane under it and the body turned onto `N`
            // keeping its heading (APPROXIMATION of `BodyHeight` 0x49A900 + the resting drop and
            // 0x49BA30, as traffic's, `traffic.md` §29; no landing, no tilted arm, `vel.y` zeroed).
            let q = (contact[0] + contact[1] + contact[2] + contact[3]) * 0.25;
            let ny = up.y.max(0.1);
            c.position.y = q.y - ((c.position.x - q.x) * up.x + (c.position.z - q.z) * up.z) / ny + rest / ny;
            let f0 = c.orientation.row(2);
            let fwd = (f0 - up * f0.dot(up)).normalize_or_zero();
            let right = up.cross(fwd).normalize_or_zero();
            if fwd.length_squared() > 0.5 && right.length_squared() > 0.5 {
                c.orientation = Mat3::from_cols(right, fwd.cross(right).normalize_or_zero(), fwd).transpose();
            }
            tracing::trace!(slice = c.slice, lat = c.lateral, patch = c.tracker.word & 0xF, up_y = up.y, air = n_air, y = c.position.y, "cop ground");
            c.velocity.y = 0.0;
        }
    }

    /// Put a cop on the road at a spawn point (§25.10 step 5).
    pub fn deploy(&mut self, table: &SliceTable, i: usize, record: usize, tick: u64) -> bool {
        self.deploy_as(table, i, record, tick, true)
    }

    /// State 1's type-1 arm (0x40419B…0x4042BE): the cop is placed (0x407890) in **state 2**
    /// (`AIState_Normal`, a rolling record) or **state 3** (`AIState_Stop` mode 1, a parked one), with
    /// its lights off (0x403F4A), and runs that state's update this tick — never state 4. `chasing`
    /// says whether the perp is wanted (`AIHigh_Perp+0x78`, as `set_wanted`); a wanted perp is
    /// acquired on the cop's next tick, whose chase update runs unlit, and the tick after that lights
    /// it (§17.5b). *(Until 2026-09-25 `chasing = true` put the cop straight into a lit chase.)*
    pub fn deploy_as(&mut self, table: &SliceTable, i: usize, record: usize, tick: u64, chasing: bool) -> bool {
        let Some((slice, lateral, moving)) = self.manager.placement(table, record, self.route_dir) else { return false };
        let sl = &table.slices[slice.min(table.slices.len() - 1)];
        let dir = self.route_dir;
        let fwd = (sl.forward * dir).normalize_or_zero();
        let up = sl.up.normalize_or_zero();
        let right = up.cross(fwd).normalize_or_zero();
        let speed = if moving { crate::traffic::target_speed(dir, self.route_dir, 0.0, 1.0) } else { 0.0 };
        let c = &mut self.cars[i];
        c.slice = slice;
        c.dir = dir;
        c.requested_dir = dir;
        c.lateral = lateral;
        c.lateral_target = lateral;
        c.speed = speed;
        c.target_speed = speed;
        // 0x407690: `car+0x7F = 1`, then `Car_placeAtSlice` 0x49B090 with `{lateral, 1.0, 0}`
        // (0x4076D8…0x4076E8) — a cop is not an opponent (`car+0x52C & 8` clear), so the offset is
        // slice-local: `centre + lateral·L + 1.0·N`; the car alive, the body is then dropped by
        // `car+0x424` (0x49B4A4…0x49B4BC), which leaves the origin `car+0x330` `rest_height`
        // along the normal above the road. APPROXIMATION: the drop is taken against the slice's
        // own plane (the placement point is on it — `lateral ⟂ up`), not the tracker patch
        // `BodyHeight` 0x49A900 measures; the next `integrate_on` settles it on the patch.
        c.position = sl.centre + sl.lateral.normalize_or_zero() * lateral + Vec3::Y * (c.rest_height() / up.y.max(0.1));
        c.orientation = Mat3::from_cols(right, fwd.cross(right).normalize_or_zero(), fwd).transpose();
        c.velocity = fwd * speed.abs();
        c.angular_velocity = Vec3::ZERO;
        c.active = true;
        c.chasing = false;
        // state 1 releases any claim before it places the car (0x4041E6 → 0x403BA0)
        c.target = None;
        c.parked = !moving;
        c.placed = true;
        c.lights = false;
        c.aim_point = Vec3::ZERO;
        c.stuck_ticks = 0;
        c.goto_slice = None;
        // `Car_placeAtSlice` 0x49B090 seats the trackers on the placement point (0x49B14E…) —
        // done on the next ground step, which has the grid
        c.tracker = nfs_physics::collision::PatchTracker::new(c.position);
        c.wheel_trackers = [nfs_physics::collision::PatchTracker::default(); 4];
        c.seated = false;
        c.airborne_count = 0;
        // the purgatory dtor 0x40DFB0 run by the placement: `car+0xED4` := the top racer of `0x5E8B88`
        let leader = self.leading_racer();
        let c = &mut self.cars[i];
        c.reference = leader;
        // `0x5CBEA0[level]+0x10` — the boost budget in 32 Hz ticks, 16 / 16 / 32 s
        c.boost_budget = BOOST_BUDGET[c.level.min(2)];
        c.blackout_until = tick;
        self.wanted = chasing;
        tracing::debug!(cop = i, slice, lateral, moving, "police deployed");
        true
    }

    fn retire(&mut self, i: usize) {
        let c = &mut self.cars[i];
        c.active = false;
        c.target = None;
        c.lights = false;
        c.velocity = Vec3::ZERO;
        c.angular_velocity = Vec3::ZERO;
        c.mode = None;
        tracing::debug!(cop = i, "police gave up");
    }
}

/// `[0x5E6654]` for the unit tests' entries ([`Police::update`], [`Police::tick`]), which run every
/// cop on every call: 4, the 8 Hz step the AI pass's gate gives a cop within 240 m of a racer.
pub const TEST_AI_TICKS: i32 = 4;
