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

/// The cop tests' `.sim` fixture, shared with the traffic tests.
#[cfg(test)]
pub(crate) use tests::specs as test_specs;

#[cfg(test)]
mod tests;

// ---------------------------------------------------------------------------------------------
// The cop car itself: the chase state of `AIState_Chase` (§27.4–§27.10) on top of the same drive
// model and lane machinery traffic uses.
// ---------------------------------------------------------------------------------------------

use crate::aidrive::{self, AiCarState};
use crate::lanes::{lane_of, lane_span, LaneCar, LaneScratch, LaneSelf, PERSONALITIES};
use glam::{Mat3, Vec3};
use nfs_physics::collision::TriangleGrid;
use nfs_physics::specs::CarSpecs;
use nfs_physics::vehicle::Vehicle;

/// `[0x5B25FC]` / `[0x5B25F8]` — the limits of 0x408750's sanity net: a velocity component or the
/// speed estimate past 115 m/s, or a lateral offset more than 35 m outside the road's half-width.
pub const SANITY_SPEED: f32 = 115.0;
pub const SANITY_OFF_ROAD: f32 = 35.0;

/// The world a cop's drive tick needs beyond the slice table: the collision grid (the reset's
/// `Car_placeAtSlice` 0x49B090 seats the trackers on it, 0x40F480 asks it for a drivable patch)
/// and the slices the other live cars stand on (`car+8` of every `0x5E8B34[]` car with
/// `car+0x7F`, 0x40F4D3…0x40F500 — the humans, the opponents and the traffic; the other cops are
/// added by [`Police`] itself).
#[derive(Clone, Copy)]
pub struct CopWorld<'a> {
    pub grid: &'a TriangleGrid,
    pub occupied: &'a [usize],
}

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

/// `0x40AF70`'s police mirror for a cop with its lights off (0x40AFF6…0x40B05C, then 0x40B157), read
/// to the instruction 2026-09-26: 16.5 m/s, or for a car not going the race's way the oncoming ladder
/// on `|[car+0xED4]+0xEC0|` (the 22.22 store at 0x40B04A is overwritten, as in traffic's copy); then —
/// the `flags & 0x10` block being traffic's only — straight to the floor `max(v, 8.8889)`
/// (`[0x5B2430]`, 0x40B157…0x40B178) and `× (int)car+0xEB0`. No `car+0xEEC` / `− 13.4` / 8.9 terms,
/// which `traffic::target_speed` has. *(Until 2026-09-26 the port used the traffic function, whose
/// 8.9 floor lifted the 8.8889 rung.)*
pub fn unlit_target_speed(dir: f32, race_dir: f32, reference_speed: f32) -> f32 {
    let mut v: f32 = 16.5;
    if dir != race_dir {
        let s = reference_speed.abs();
        // the image's constants: `[0x5B2554]` 13.333333, `[0x5B2650]` 26.666666, `[0x5B2410]` 40,
        // `[0x5B264C]` 55.555557, `[0x5B2648]` 71.111115; the rungs 0x41555555, 0x410E38E4,
        // 0x408E38E4, 0x400E38E4, 0x403147AE
        v = if s < 13.333_333 {
            16.5
        } else if s < 26.666_666 {
            13.333_333
        } else if s < 40.0 {
            8.888_889
        } else if s < 55.555_557 {
            4.444_444_7
        } else if s < 71.111_115 {
            2.222_222_3
        } else {
            2.77
        };
    }
    v.max(8.888_889) * dir
}

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
/// `0x5CBEA0[level]+0x00` — the slot-hold ticks (`chase+0x78`) the close-in test wants exceeded.
pub const CLOSE_IN_SLOT_TICKS: [i32; 3] = [10, 8, 4];
/// `0x5CBEA0[level]+0x04` — the close-in test's lateral window on `|chase+0x6C|`.
pub const CLOSE_IN_LATERAL: [f32; 3] = [10.0, 14.0, 18.0];
/// 0x40470E: the nudge's ram lasts `0x20` ticks.
pub const NUDGE_TICKS: u64 = 0x20;

/// The roll at the head of `0x403850(cop, p = 0.25, target)`, read to the instruction 2026-09-26
/// (0x403854…0x4038B1): `thr = fistp([0x5E6654] · p · [0x5B24B8] (1/32) · 1000)` — 31 at an 8 Hz
/// update, 62 at 4 Hz (round to nearest even) — and the LCG step `r` (the shared `rand16`),
/// passing when `(r · 1000) >> 16 < thr`: 0.25 per second of AI updates.
pub fn nudge_roll(ai_ticks: i32, r16: u32) -> bool {
    let thr = (ai_ticks as f32 * 0.25 * 0.03125 * 1000.0).round_ties_even() as i32;
    (((r16 * 1000) >> 16) as i32) < thr
}
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
    /// `car+0x528` — this cop's place in `0x5E8B88[]` as the previous even tick's `0x417710` left
    /// it ([`crate::race_order::RaceOrder`]); the caller writes it before [`Police::update`]
    pub place: usize,
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
    /// `car+0xAF8` — for a cop, as for a traffic car, the **bare lap fraction** of its slice:
    /// 0x4169B0 (`car+0xE18`, Sc64 priority 0x19, even ticks) writes `frac(car+8)` for a car
    /// without the racer bit (0x416A21…0x416AA1) and 0 while `car+0x7F == 0` (0x416AAC) — before
    /// the integrators (priority 0x1E) move the car. What 0x417710's two sorts read.
    pub progress: f32,
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
    /// `car+0x1064..0x106C` and `car+0x1070` — the explicit aim point (PURSUE's ram) and the lane
    /// offset that goes with it, which `0x4090C0` copies into the drive's `car+0x1074` (0x40954E)
    pub aim_point: Vec3,
    pub aim_lateral: f32,
    /// `car+0x1078` / `car+0x107C` — a held lateral offset and its score bias. While the bias is
    /// positive `0x402A40` first asks `0x402790` whether the offset still wins the lane vote and,
    /// if so, takes it as `car+0xEB4`. Written by the chase only: `0x40CD70` sets both to 1.0 every
    /// chase tick (0x40CE68 / 0x40CE72), CLOSE-QUARTERS to its aim and 15 / 30 (0x40D642 /
    /// 0x40D64B); the chase's dtor 0x40CC10 zeroes them.
    pub lane_override: f32,
    pub lane_bias: f32,
    /// `car+0x1098` / `car+0x109C` — the engine and speed multipliers
    pub boost_engine: f32,
    pub boost_speed: f32,
    /// `car+0x108C` — the lit `0x40AF70`'s copy of `car+0x109C` (0x40AFB2), which the drive
    /// controllers' brake force and in-control lateral gains take when it exceeds 1; `Car_raceInit`
    /// 0x410730 starts it (0x410A91), nothing else writes it
    pub rate_mul: f32,
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
    /// `AIHigh_Perp+0x4C[slot]` for this cop — its roster position (entry − 1), which 0x403C00 walks
    /// up from; left as it was by a release (0x403BA0 does not touch it)
    pub roster_pos: usize,
    /// `AIHigh_Cop+0x60` — this cop's rank in the target's queue, as the state-4 arm last saw it
    /// (0x404615 compares the roster's answer with it to decide whether to re-aim, 0x40CCA0).
    pub rank: usize,
    /// `chase+0x78` — ticks spent standing in the formation slot.
    pub slot_ticks: i32,
    /// `chase+0x70` — the close-in (ram) mode, set by `0x40CD50` only while it is clear and cleared
    /// by `0x40CD70` once `gGameTick >= chase+0x74` (0x40CE1C…0x40CE33) and by `0x40CCA0`.
    pub ram: bool,
    /// `chase+0x74` — the tick the ram mode lapses.
    pub ram_until: u64,
    /// `AIState_Chase+0x08…+0x40` — the smoothed target tracker (0x403620 / 0x403650 / 0x4036F0).
    pub tracker: ChaseTracker,
    /// `chase+0x4C…+0x54` — the formation offset `0x40CCA0` latched: `0x5CBEE8[level][rank]` when the
    /// chase is built or the rank changes outside the ram mode.
    pub formation: [f32; 3],
    /// `chase+0x58` / `chase+0x5C` — the longitudinal / lateral sector of [`Self::formation`] against
    /// the target's half length / half width, worked out by `0x40CCA0` (0x40CCD6…0x40CD3A).
    pub want_lon: i32,
    pub want_lat: i32,
    /// `chase+0x68` — the signed longitudinal gap `0x40E920(cop, tracker slice, tracker point)`,
    /// positive with the cop ahead of the tracked target point (`0x40CD70`).
    pub chase_gap: f32,
    /// `chase+0x6C` — `cop+0xED0 − chase+0x38`, the cop's lateral less the tracked target lateral.
    pub chase_lat_err: f32,
    /// `chase+0x88` — the skill level that picks the PURSUE ladder
    pub level: usize,
    /// `car+0x101C` — the lane-existence scan window
    scan: i32,
    pub mode: Option<ChaseMode>,
    /// `car+0x78` — 0x417190's octagonal x–z distance to the nearest racer, written by the cop's
    /// tracker 0x49AF70 (Sc32 priority 6, after the AI pass) while it is in the world
    /// ([`Police::track_racers`]); the AI pass's gate [`crate::traffic::ai_pass_due`] reads it.
    pub racer_distance: f32,
    /// The cop's car record as the physics sees it — the same `RigidBody` every car is
    /// (`RigidBody_init` 0x49B510, `traffic.md` §22): the body points (`car+0x8C`, the first four
    /// being `Susp_update` 0x499A70's probes, 0x499AD4…0x499B13 — `nfs_game::world::car_body`,
    /// [`CopCar::set_body`]), the car tracker `car+0x400` / `car+0x3E8` and the four wheel
    /// trackers, the wheels' compression / airborne / roughness state, `car+0x424` / `+0x428`,
    /// `word car+0x418` ([`Vehicle::airborne_count`]: `Susp_update` stores `nAir > 1`, the
    /// car–car impulse steps it, 0x408360 reads it at 0x408543), `car+0x42C`
    /// ([`Vehicle::knocked_over`], `Damage` 0x49489A → the 0x4084C3 exit), the crash countdown
    /// `car+0x1100` and the damage zones. The cop's `car+0xE20` is `Car_ground` 0x49BD40 like a
    /// racer's (`suspension-collision.md` §4.2 item 9), run here through
    /// [`Vehicle::ai_ground_step`] ([`Police::ground`]); the collision pass 0x498FF0 runs on it
    /// too (the caller's, `main.rs`). The kinematic truth — `car+0x330`, `+0x364`, `+0x33C`,
    /// `+0x388` — is this struct's own [`Self::position`], [`Self::orientation`],
    /// [`Self::velocity`], [`Self::angular_velocity`], copied in and out around each use
    /// ([`Self::sync_body_in`] / [`Self::sync_body_out`]).
    pub body: Vehicle,
    /// The cop's own `.sim` (§17.1) — the suspension constants `Car_ground` reads
    /// (`specs[0x114…0x138]`, `suspension-collision.md` §8) and the mass.
    pub specs: CarSpecs,
    /// `car+0xD4C…+0xD54`, the body-frame force the AI controller wrote (0x409720 / 0x409C70:
    /// lateral, 0, forward) — what `Susp_update`'s dive / roll filters read as `car+0xD4C` /
    /// `car+0xD54`; the drive's damped exits zero it (0x4084CE…, 0x4085FF…).
    pub accel_local: Vec3,
    /// `car+0xD40…+0xD48`, the world acceleration the AI integrator 0x40A1D0 derives from it
    /// (0x40A236 / 0x40A27B) — `Susp_update`'s lateral term reads it (0x49A163); the damped exits
    /// leave it alone.
    pub accel_world: Vec3,
}

impl CopCar {
    /// A cop car with its own `.sim` (`specs`, §17.1 — the mass is the sim's) and the half box
    /// 0x445090 takes. The body starts with the box's bottom corners at the wheel-quad height as
    /// its probes (APPROXIMATION, for callers without a model — the unit tests); the race gives it
    /// the model's `BndP` points through [`Self::set_body`].
    pub fn new(model: usize, half_extents: Vec3, specs: &CarSpecs, level: usize) -> Self {
        let mass = specs.mass;
        let mut body = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, specs, half_extents.x, crate::traffic::RIDE_HEIGHT);
        // an AI car: the human-only arms (0x493DD0 on a landing, the automatic's pedal swap) are
        // off, `car+0x52C & 1` clear
        body.ai_driven = true;
        body.traction_control = true;
        // `RigidBody_init` 0x49B510: `car+0x84 = 1.0`. Only the racers' `LoadTransfer` 0x4A1900
        // (Car_simulate) puts 1.16 there; a cop's changes only in the lit `0x40AF70`
        body.gravity_scale = 1.0;
        {
            let (x, y, z) = (half_extents.x, -(crate::traffic::RIDE_HEIGHT + half_extents.y), half_extents.z);
            body.set_car_body(vec![Vec3::new(-x, y, z), Vec3::new(x, y, z), Vec3::new(-x, y, -z), Vec3::new(x, y, -z)], half_extents);
        }
        body.position = Vec3::ZERO;
        Self {
            gear: aidrive::Gear::default(),
            brake_light: 0,
            model,
            place: 0,
            active: false,
            lights: false,
            chasing: false,
            parked: false,
            placed: false,
            slice: 0,
            progress: 0.0,
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
            lane_override: 0.0,
            lane_bias: 0.0,
            boost_engine: 1.0,
            boost_speed: 1.0,
            rate_mul: 1.0,
            boost_budget: 0,
            blackout_until: 0,
            stuck_ticks: 0, rank: 0, roster_pos: 0, slot_ticks: 0, ram: false, ram_until: 0,
            tracker: ChaseTracker::default(),
            formation: [0.0; 3],
            want_lon: 0,
            want_lat: 0,
            chase_gap: 0.0,
            chase_lat_err: 0.0,
            goto_slice: None,
            reference: None,
            target: None,
            level,
            scan: 0,
            mode: None,
            racer_distance: 0.0,
            body,
            specs: specs.clone(),
            accel_local: Vec3::ZERO,
            accel_world: Vec3::ZERO,
        }
    }

    /// The body `RCar::load` builds for the cop's model: the `BndP` points less `car+0x3AC`
    /// and the LOD-5 half box (`nfs_game::world::car_body`).
    pub fn set_body(&mut self, points: Vec<Vec3>, half_extents: Vec3) {
        self.half_extents = half_extents;
        self.body.position = Vec3::ZERO;
        self.body.body_lift = 0.0;
        self.body.set_car_body(points, half_extents);
        self.body.position = self.position;
    }

    /// The kinematic state into the car record before the physics reads it: `car+0x330`,
    /// `+0x364`, `+0x33C`, `+0x388`, and the body-frame velocity `car+0xD58…` (0x40A1D0 →
    /// 0x49F0C0 for an AI car), the AI's force `car+0xD4C…` and acceleration `car+0xD40`.
    pub fn sync_body_in(&mut self) {
        let b = &mut self.body;
        b.position = self.position;
        b.orientation = self.orientation;
        b.velocity = self.velocity;
        b.angular_velocity = self.angular_velocity;
        b.v_local = self.orientation * self.velocity;
        b.accel_local = self.accel_local;
        b.accel_world = self.accel_world;
        b.half_extents = self.half_extents;
    }

    /// …and back out after it (the ground contact, a placement, the collision pass).
    pub fn sync_body_out(&mut self) {
        let b = &self.body;
        self.position = b.position;
        self.orientation = b.orientation;
        self.velocity = b.velocity;
        self.angular_velocity = b.angular_velocity;
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

/// `0x5CBEDC[SKILL [0x6573F4]]` = `{0.05, 0.1, 1.0}` — the chase tracker's gain, the Chase ctor's
/// eighth argument (state 2 at 0x404360…0x40437C, state 3 the same). **This is where the race's skill
/// reaches the chase**: at the hardest skill the tracker follows the target exactly, below it the
/// cop works from a lagging estimate of where the target is.
pub const TRACKER_GAIN: [f32; 3] = [0.05, 0.1, 1.0];

/// `AIState_Chase+0x08`, the target tracker — read to the instruction 2026-09-26 (0x403620 ctor,
/// 0x403650 reset, 0x4036F0 step). Every field is a first-order filter `x += (raw − x)·k`, stepped
/// once per `0x40CD70`. The fields, by their offset in the chase:
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ChaseTracker {
    /// `+0x08` — the gain, [`TRACKER_GAIN`]
    pub k: f32,
    /// `+0x14` — the smoothed `0x40E6E0(target, cop)`, the target's signed slice gap ahead of the cop
    pub gap: f32,
    /// `+0x18` — the predicted target slice, `0x4817D0(cop.slice, round(gap / 6), −1)`
    pub slice: usize,
    /// `+0x1C…+0x24` — the smoothed `target.pos − cop.pos`
    pub rel: Vec3,
    /// `+0x28…+0x30` — the predicted target point, `cop.pos + rel`
    pub point: Vec3,
    /// `+0x34` — the smoothed `target+0xED0 − cop+0xED0`
    pub lat_diff: f32,
    /// `+0x38` — the predicted target lateral, `cop+0xED0 + lat_diff`
    pub lateral: f32,
    /// `+0x3C` — `0x40EB20(slice, lateral)`
    pub lane: i32,
    /// `+0x40` — the smoothed target speed `car+0xEC0`
    pub speed: f32,
}

impl ChaseTracker {
    /// `0x403620(this, cop, target, k)` → `0x403650(target)`: the raw values of this instant. One
    /// quirk carried as read: 0x4036B0…0x4036BF store `+0x34 = cop+0xED0 − target+0xED0`, the
    /// **opposite** sign of what the step filters toward (`target − cop`), so a slow tracker starts
    /// with its lateral difference mirrored.
    pub fn new(table: &SliceTable, k: f32, cop: &CopCar, tgt: &ChaseTarget) -> Self {
        Self {
            k,
            gap: track_distance(table, tgt.slice, cop.slice),
            slice: tgt.slice,
            rel: tgt.position - cop.position,
            point: tgt.position,
            lat_diff: cop.lateral - tgt.lateral,
            lateral: tgt.lateral,
            lane: tgt.lane,
            speed: tgt.speed,
        }
    }

    /// `0x4036F0` — one step (0x40CD79, the head of `0x40CD70`).
    pub fn step(&mut self, table: &SliceTable, cop: &CopCar, tgt: &ChaseTarget) {
        let k = self.k;
        self.gap += (track_distance(table, tgt.slice, cop.slice) - self.gap) * k;
        // `gap · [0x5B2420] (1/6)`, `fistp` (round to nearest), then 0x4817D0 along the race
        let n = (self.gap * (1.0 / 6.0)).round_ties_even() as i32;
        self.slice = table.add(cop.slice, n);
        self.rel += ((tgt.position - cop.position) - self.rel) * k;
        self.point = cop.position + self.rel;
        self.lat_diff += ((tgt.lateral - cop.lateral) - self.lat_diff) * k;
        self.lateral = cop.lateral + self.lat_diff;
        self.lane = lane_of(&table.slices[self.slice.min(table.slices.len() - 1)], self.lateral);
        self.speed += (tgt.speed - self.speed) * k;
    }
}

/// `0x40E920(car, slice, point)` — read to the instruction 2026-09-26 (0x40E920…0x40E9D3): the slice
/// gap `(car.slice − slice)·6`, and when that is within ±12 m (both ends inclusive, 0x40E96B /
/// 0x40E978) the finer `forward · (car.pos − point)` over x and z, `forward` being the car's slice
/// forward `car+0x3D0` (the axis `car+0xEC0` is measured along). Positive with the car ahead.
pub fn fine_gap(table: &SliceTable, car_slice: usize, car_pos: Vec3, slice: usize, point: Vec3) -> f32 {
    let d = track_distance(table, car_slice, slice);
    if !(-12.0..=12.0).contains(&d) {
        return d;
    }
    let f = table.slices[car_slice.min(table.slices.len() - 1)].forward.normalize_or_zero();
    let r = car_pos - point;
    f.x * r.x + f.z * r.z
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
    /// car (0x40825C…0x40826E, `[0x5B25E8]` = 0.9). The race load 0x414DB0 zeroes `WEATHER` at
    /// 0x414DF9 before any in-race reader, so it is off in every race — the option, not the night flag.
    pub weather: bool,
    /// The racers (`RacerRef`) in registration order — the perps, and the cars a cop can take as
    /// its reference car — refreshed by the caller every tick through [`Police::set_racers`] before
    /// the spawn pass and `update`; index 0 is the human.
    pub racers: Vec<RacerRef>,
    /// `car+0x528` of each car the caller passes as `others` to [`Police::update`], in the same
    /// order ([`crate::race_order::RaceOrder`]); shorter than `others` (the unit tests) and the lane
    /// pass sorts the cars itself instead.
    pub other_places: Vec<usize>,
    /// World positions of each car the caller passes as `others`, in the same order — `0x40E860`'s
    /// fine step needs `a+0x330`/`b+0x330` (`traffic.md` §34.6), which a `LaneCar` does not carry.
    /// Shorter than `others` (the unit tests) falls back to the slice-only gap for the missing tail,
    /// same as [`crate::traffic::TrafficSet::other_positions`].
    pub other_positions: Vec<Vec3>,
    /// `0x5E8B88[]` restricted to the racers: ascending by `car+0xAF8 − car+0xB20`, stable across
    /// ticks (`0x417710`'s first sort). The purgatory dtor 0x40DFB0 walks it from the top.
    race_order: Vec<usize>,
    /// `0x5E8BAC[]` restricted to the racers: ascending by `car+0xAF8`, stable across ticks
    /// (`0x417710`'s second sort). The spawn search 0x405440 walks it from the top.
    spawn_order: Vec<usize>,
    one_way: bool,
    /// `AIHigh_Perp+0x14…+0x48` of the human — the chaser roster, entries 1…6 (`+0x1C` + 8·k; entry 0
    /// at `+0x14` is never written), each the cop that holds it (`{slot, car}` in the original)
    roster: [Option<usize>; 7],
    /// The racer slot of cop 0 — the cops come after the humans and the opponents (0x4ECF00:
    /// `edi = nP + nOpp` at 0x4ED103 / 0x4ED1ED, `traffic.md` §17.5d). It is the phase of the AI
    /// pass's gate ([`crate::traffic::ai_pass_due`], `car+0` = `slot | 0x100`); the caller sets it.
    pub slot_base: usize,
}

impl Police {
    pub fn new(file: CopFile, cars: Vec<CopCar>, skill: u8, one_way: bool) -> Self {
        Self { manager: CopManager::new(file), cars, cfg: nfs_formats::trafcfg::TrafficConfig::default(), phys: CopPhysics::default(), violation: Violation::None, route_dir: 1.0, skill, one_way, wanted: false, weather: false, racers: Vec::new(), other_places: Vec::new(), other_positions: Vec::new(), race_order: Vec::new(), spawn_order: Vec::new(), budget: duty_budget(1, false), slot_base: 0, roster: [None; 7] }
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
    ///
    /// The lit arm, read to the instruction 2026-09-26 (0x40AF9A…0x40AFEC): `m = car+0x109C · 1.0`
    /// is stored as **`car+0x108C`** — the multiplier both drive controllers put on the brake force
    /// and the in-control one on its lateral gains when it exceeds 1 (0x409B46, 0x409E95, 0x40A105)
    /// — and `car+0x84`, the body's **gravity scale** `Car_ground` reads, becomes `m > 1 ? m : 1.0`
    /// (0x40AFBF / 0x40AFC9). Both keep their value while the lights are off. *(Until 2026-09-26 the
    /// port drove the controllers on `car+0x1098` and kept the racers' 1.16 gravity scale on a cop.)*
    ///
    /// The unlit arm is §17.6's ladder, whose oncoming branch reads `|[car+0xED4]+0xEC0|` (0x40B01A)
    /// — the **reference car's** speed, not the target's. *(Until 2026-09-26 the port passed the
    /// chase target's.)*
    fn base_target_speed(&mut self, i: usize, target: &ChaseTarget) -> f32 {
        let reference_speed = self.cars[i].reference.and_then(|r| self.racers.get(r)).map(|r| r.speed).unwrap_or(target.speed);
        let route_dir = self.route_dir;
        let c = &mut self.cars[i];
        if c.lights {
            c.rate_mul = c.boost_speed;
            c.body.gravity_scale = if c.rate_mul > 1.0 { c.rate_mul } else { 1.0 };
            PURSUIT_BASE_SPEED * c.rate_mul * c.dir
        } else {
            unlit_target_speed(c.dir, route_dir, reference_speed)
        }
    }

    /// `0x40CE80(chase, checkHumans)`, read to the instruction (0x40CE80…0x40CF6D) — the rubber
    /// band: with budget left (`chase+0x7C > 0`, 0x40CE8B) and past the blackout (`gGameTick >=
    /// chase+0x8C`, 0x40CE9E) it sets `car+0x1098 = 3.0`, `car+0x109C = 2.5`, then puts both back to
    /// 1.0 for a human whose `0x40E6E0(cop, human) · car+0xEB0` is in `(0, chase+0x80)` or
    /// `(−chase+0x84, 0)` — strictly, so a zero gap keeps the boost. It spends nothing: the budget
    /// is charged by the next update's `0x40CD70` ([`Police::spend_boost`]).
    ///
    /// `check_humans == false` (0x40CEBA…0x40CEC6: the flag is tested after the two stores) skips the
    /// suppression — CLOSE-QUARTERS calls it that way a second time in the ram mode (0x40D194…0x40D19F).
    fn apply_boost(&mut self, i: usize, tick: u64, gap_to_human: f32, check_humans: bool) {
        let c = &mut self.cars[i];
        if c.boost_budget <= 0 || tick < c.blackout_until {
            return;
        }
        c.boost_engine = BOOST_ENGINE;
        c.boost_speed = BOOST_SPEED;
        let g = gap_to_human * c.dir;
        if check_humans && ((g > 0.0 && g < BOOST_SUPPRESS_AHEAD) || (g < 0.0 && g > -BOOST_SUPPRESS_BEHIND)) {
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
        self.run(table, tick, target, others, None, &|_| Some(TEST_AI_TICKS), None);
        self.drive(table, tick, None);
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
    ///
    /// `grid` is the track's collision grid, for state 1's placement (`Car_placeAtSlice` 0x49B090).
    pub fn ai(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar], humans: &[usize], rand16: &mut dyn FnMut() -> u32, grid: Option<&TriangleGrid>) {
        let (base, dist): (usize, Vec<f32>) = (self.slot_base, self.cars.iter().map(|c| c.racer_distance).collect());
        self.run(table, tick, target, others, Some((humans, rand16)), &|i| crate::traffic::ai_pass_due(dist[i], false, tick, base + i), grid);
    }

    /// The AI half with every cop due on every call, `[0x5E6654]` = [`TEST_AI_TICKS`], then
    /// [`Police::drive`] — the unit tests' entry with the searches.
    pub fn tick(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar], humans: &[usize], rand16: &mut dyn FnMut() -> u32) {
        self.run(table, tick, target, others, Some((humans, rand16)), &|_| Some(TEST_AI_TICKS), None);
        self.drive(table, tick, None);
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
    ///
    /// `world` is what 0x408750's net at the end of the tick needs to re-place the car
    /// ([`CopWorld`]); without it (the unit tests' entries) the net is not run.
    pub fn drive(&mut self, table: &SliceTable, tick: u64, world: Option<CopWorld<'_>>) {
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                continue;
            }
            if let Some((dt, n_ticks)) = crate::traffic::drive_due(self.cars[i].racer_distance, false, tick, self.slot_base + i) {
                self.drive_car(table, tick, i, dt, n_ticks, world);
            }
        }
    }

    /// `Car_ground` 0x49BD40 for every cop in the world — its `car+0xE20` on the Sc32 list at
    /// priority 0x1E, inserted right after its drive `car+0xE1C` (`Cars_create` 0x411277…0x411349
    /// and 0x414C30's "after every equal priority", `traffic.md` §17.5e), so it runs after
    /// [`Police::drive`] and before the collision pass 0x498FF0 at 0x28. First the tracker
    /// 0x49AF70 (priority 6, every 32 Hz tick: `Tracker::update` / `updateZone` at `car+0x330` and,
    /// on a new patch, the plane 0x499520 — `PatchTracker::track`); then, when the LOD gate says
    /// the car is due (`Lod_shouldUpdate(1, 3, car+0x78, …)`, [`nfs_physics::vehicle::ground_due`]:
    /// 32 Hz within 96 m of a racer, 16 Hz to 128 m, 8 Hz beyond), `Susp_update` 0x499A70 and the
    /// arms of `Car_ground` on the cop's car record ([`Vehicle::ai_ground_step`]) — the four probes
    /// on their wheel trackers, the compression, the roughness, the pitch / roll rates into
    /// `car+0x388` (which the 64 Hz step [`Police::integrate`] then turns the body by), `car+0x418`,
    /// `BodyHeight`, the gravity while `car+0x428 > 0.005`, the tilted arm's `BodyGroundCollide`,
    /// the hard landing and the resting drop. *(Until 2026-09-26 the port stood the cop on the
    /// contact plane at every 64 Hz step with no pitch / roll rates, no roughness and no landing.)*
    ///
    /// MEDIUM: `car+0xD7F` (the throttle `Susp_update`'s roll term reads, 0x49A673) has no writer
    /// in the cop's drive chain (0x408000…0x40B000 store it nowhere; 0x49E220 zeroes it), so it is
    /// the record's 0 — the port's body keeps its 0.
    pub fn ground(&mut self, table: &SliceTable, grid: &TriangleGrid, tick: u64) {
        let _ = table;
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                continue;
            }
            let slot = self.slot_base + i;
            let c = &mut self.cars[i];
            c.sync_body_in();
            c.body.tracker.track(grid, c.body.position);
            if let Some((rate_hz, n_ticks)) = nfs_physics::vehicle::ground_due(c.racer_distance, false, tick, slot) {
                c.body.ai_ground_step(grid, &c.specs, rate_hz, n_ticks, true);
                tracing::trace!(cop = i, tick, patch = c.body.tracker.word & 0xF, cos_tilt = c.body.cos_tilt, air = c.body.airborne_count, h424 = c.body.body_height, y = c.body.position.y, "cop ground");
            }
            c.sync_body_out();
        }
    }

    /// `Car_postImpact` 0x410280 → 0x40F740's crash countdown for every cop in the world (its
    /// `car+0xE30`, Sc32 priority 0x32, after the collision pass; `physics.md` §4.1c): while
    /// `car+0x1100` runs — `Damage` loads it with 0xC0 on an impact past 80 — it loses a tick, eight
    /// more off the road surface (`car+0x464 == 0`, the car tracker's nibble); an upright, alive
    /// car stops it, otherwise at zero the car is reset to its slice (`0x40F540(car, 0, 0)`,
    /// [`Police::reset_at`]), and it restarts when the car's up is under 0.1 along the slice's or the
    /// car is off the surface (0x40FB6D…0x40FBA2). The same port decision as for the racers: mode
    /// 0's "the car is fine" exit (0x40F5BF…0x40F632) is **not** carried (`race-logic.md` §7 —
    /// carrying it parked two thirds of a field; it needs the original running), so a knocked-over
    /// cop that is dead comes back after its countdown where the original may leave it. MEDIUM.
    /// `occupied` as in [`CopWorld`].
    pub fn post_impact(&mut self, table: &SliceTable, grid: &TriangleGrid, occupied: &[usize]) {
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                continue;
            }
            let c = &mut self.cars[i];
            let off_surface = c.body.tracker.word & 0xF == 0;
            if c.body.reset_countdown_tick(off_surface) {
                tracing::debug!(cop = i, slice = c.slice, "cop crash countdown: reset to its slice (0x40F540 mode 0)");
                self.reset_at(table, grid, i, occupied);
            }
            let c = &mut self.cars[i];
            let up = table.slices[c.slice.min(table.slices.len() - 1)].up.normalize_or_zero();
            c.body.reset_countdown_start(up, off_surface);
        }
    }

    /// `Car_resetToSlice` 0x40F540 (car, mode 1 or 0 past its exits, 0) for cop `i`, read to the
    /// instruction (0x40F540…0x40F733): `car+0x1104 = car+0x1100 = 0` (0x40F597…0x40F59D; a cop's
    /// `car+0x540` is 0, so no early return); the slice is the car's own `car+8`
    /// (0x40F654) or, while 0x40F480 says it is taken, the next along the race (0x4817D0 with
    /// `[0x657430] ? −1 : 1` — +1 on this port's table, 0x40F666…0x40F691). 0x40F480: the slice
    /// centre is over a drivable patch (the car's tracker, `updateZone(centre, 1)`, then
    /// `car+0x2C & 0xF`, 0x40F4BF…0x40F4C8) and no other live car of `0x5E8B34[]` (0x40F4D3…0x40F500)
    /// and no smackable (0x40F502…0x40F523 — not carried, this port's props keep no slice) has
    /// `car+8 == s`. Then 0x49E220 (the drivetrain and the AI's forces / velocities: `car+0xD40…`,
    /// `+0xD4C…`, `+0xD58…`, `+0xD64…`, `+0xD7F` / `+0xD80` — `Vehicle::rearm_after_reset` and
    /// the fields the port's cop has), the offset `{0, 0, 0}` (a cop is not an opponent,
    /// 0x40F6A2) into `Car_placeAtSlice` 0x49B090 with `car+0xEAC` as the direction (0x40F6E6),
    /// `car+0x33C…+0x344 = 0` (0x40F712…0x40F71E) and 0x40F400: `car+0x388…` = 0, `+0xD64…`,
    /// `+0xD4C…`, `word +0x418`, `+0x42C`, `+0x450`, `+0x430`, `+0x434`, `+0x43C`, `+0x420`,
    /// `+0x35C`, `+0x1104`, `+0x40C` = 0. The camera call 0x4618B0 is the human's (`car+0x520 ==
    /// [0x65741C]`). The original searches without end; the port stops after one lap (a guard).
    fn reset_at(&mut self, table: &SliceTable, grid: &TriangleGrid, i: usize, occupied: &[usize]) {
        let others: Vec<usize> = self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.slice).collect();
        let mut s = self.cars[i].slice.min(table.slices.len() - 1);
        for _ in 0..table.lap_len.max(1) {
            if grid.find_patch(table.slices[s].centre) & 0xF != 0 && !occupied.contains(&s) && !others.contains(&s) {
                break;
            }
            s = table.add(s, 1);
        }
        let c = &mut self.cars[i];
        c.body.reset_countdown = 0;
        // 0x49E220
        c.body.rearm_after_reset();
        c.accel_local = Vec3::ZERO;
        c.accel_world = Vec3::ZERO;
        c.brake_light = 0;
        for w in &mut c.body.wheels {
            *w = nfs_physics::vehicle::Wheel::default();
        }
        // 0x49B090 with `car+0xEAC`
        let sl = &table.slices[s];
        c.slice = s;
        c.sync_body_in();
        c.body.place_at_slice(grid, &c.specs, sl.centre, sl.forward.normalize_or_zero(), Vec3::ZERO, false, c.requested_dir < 0.0);
        c.sync_body_out();
        // vel = 0 and 0x40F400
        c.velocity = Vec3::ZERO;
        c.angular_velocity = Vec3::ZERO;
        c.body.velocity = Vec3::ZERO;
        c.body.angular_velocity = Vec3::ZERO;
        c.body.v_local = Vec3::ZERO;
        c.body.w_local = Vec3::ZERO;
        c.body.airborne_count = 0;
        c.body.knocked_over = false;
        c.body.impact = 0.0;
        c.lateral = (c.position - sl.centre).dot(sl.lateral.normalize_or_zero());
        c.speed = 0.0;
        tracing::debug!(cop = i, slice = s, pos = ?c.position, "cop reset to slice (0x40F540)");
    }

    #[allow(clippy::too_many_arguments)]
    fn run(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar], mut spawn: Option<(&[usize], &mut dyn FnMut() -> u32)>, due: &dyn Fn(usize) -> Option<i32>, grid: Option<&TriangleGrid>) {
        // `AIHigh+0x60`, the rank, is each cop's place in the perp's roster (`0x403C00`, run by the
        // cop itself when it claims the perp and on each of its state-4 ticks — [`Police::roster_bubble`]),
        // not a sort of all the chasers. *(Until 2026-09-26 the port re-sorted every chasing cop by
        // its gap to the target at the head of every pass, without the 12 m hysteresis.)*
        for i in 0..self.cars.len() {
            // 0x406745…0x406770: the AI pass's gate, and `[0x5E6654]` for this update
            let Some(ai_ticks) = due(i) else { continue };
            if !self.cars[i].active {
                // state 1: search, and on a record place the car (0x407890, [`Police::deploy_as`])
                // and run the new state's update at once (0x404243) — the `placed` arm below
                let Some((humans, rand16)) = spawn.as_mut() else { continue };
                let Some(rec) = self.find_spawn_point(table, tick, humans, &mut **rand16) else { continue };
                let wanted = self.wanted;
                if !self.deploy_as(table, i, rec, tick, wanted, grid) {
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
                // 0x405024…0x405045: `+0x58` given back (0x403BA0) and cleared — once, the first
                // state-9 tick after the stuck one
                self.release(i);
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
                // The arm in the binary's order (0x4044FC…0x404719), before its `0x405220`: the stuck
                // test (acted on below, after the give-up test — the GotoSlice is only driven if
                // the cop is still out), then on the live chase the roster, the ram latch and the
                // nudge. The nudge's LCG step comes even on a stuck tick (0x4046E1 runs on the
                // freed chase), and before any drop or give-up. *(Until 2026-09-26 the port ran
                // them after the drop and give-up tests.)*
                let stuck = self.cars[i].stuck_ticks > STUCK_LIMIT;
                // 0x404600…0x404666: the roster answers this cop's rank; a changed rank re-aims the chase
                // at the new formation slot through `0x40CCA0` — but only outside the ram mode, so a
                // cop that is ramming keeps its slot (and `AIHigh+0x60` still takes the new rank).
                let lvl = self.cars[i].level.min(2);
                // (On a stuck tick the roster step still runs — it is the perp's object, 0x40460D —
                // while the re-aim, the latch and the nudge would write the freed chase.)
                let r = self.roster_bubble(table, i);
                if r != self.cars[i].rank {
                    self.cars[i].rank = r;
                    if !stuck && !self.cars[i].ram {
                        self.retarget(table, i, target, FORMATION[lvl][r.min(5)], tick, ai_ticks);
                    }
                }
                // §27.4, 0x40466B…0x4046CE, read to the instruction 2026-09-26: the chase latches the ram
                // mode for 8 / 12 / 18 s (`0x40CD50`, only while it is not already on) when the cop has
                // held its formation slot for **more** than `0x5CBEA0[lvl]+0x00` = 10 / 8 / 4 ticks of
                // `[0x5E6654]` (`chase+0x78`, `jle` skips), sits within `+0x04` = 10 / 14 / 18 m of the
                // tracked target laterally (`|chase+0x6C|`) and within `+0x08` = 13 / 15 / 18 m of it
                // along the road (`|chase+0x68|`), both strict. The arm reads what the last `0x40CD70` /
                // CLOSE-QUARTERS left. *(Until 2026-09-26 the port tested the longitudinal gap alone, so
                // any cop passing within 13–18 m of the target latched a ram.)*
                let hold = self.cars[i].slot_ticks > CLOSE_IN_SLOT_TICKS[lvl] && self.cars[i].chase_lat_err.abs() < CLOSE_IN_LATERAL[lvl];
                if !stuck && !self.cars[i].ram && hold && self.cars[i].chase_gap.abs() < CLOSE_IN_GAP[lvl] {
                    tracing::debug!(cop = i, tick, "police ram: close-in mode latched (0x4046CE)");
                    self.cars[i].ram = true;
                    self.cars[i].ram_until = tick + CLOSE_IN_TICKS[lvl];
                }
                // 0x4046D3…0x404719: the nudge. `0x403850(cop, 0.25, target)` rolls the shared LCG on
                // every state-4 tick and then asks whether the cop is alongside and just ahead; if so
                // and the ram mode is off, `0x40CCA0` re-aims the chase at the offset `{0, 0, 0}` —
                // right on the target, and latched until the next rank change — and `0x40CD50(chase, 1,
                // 0x20)` rams for half a second. (Without the LCG — the unit tests' `update` — no roll.)
                if let Some((_, rand16)) = spawn.as_mut() {
                    let r = rand16();
                    if !stuck && nudge_roll(ai_ticks, r) && self.alongside(table, i, target) && !self.cars[i].ram {
                        self.retarget(table, i, target, [0.0; 3], tick, ai_ticks);
                        self.cars[i].ram = true;
                        self.cars[i].ram_until = tick + NUDGE_TICKS;
                        tracing::debug!(cop = i, tick, "police nudge: dead-on ram for 0x20 ticks (0x404714)");
                    }
                }

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
                    // 0x4053AC / 0x4053C3: `+0x58` and the claim on the perp; 0x404316 the rank
                    self.cars[i].target = Some(p);
                    self.cars[i].rank = self.roster_claim(table, i);
                    built_parked = self.cars[i].parked;
                    self.build_chase(table, i, target, tick, ai_ticks);
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
                c.stuck_ticks = 0;
                // the Chase's deleting dtor 0x40CC10 zeroes `car+0x1064…0x1070`, `+0x1078`, `+0x107C`
                // (the ram aim point) and puts `car+0x1098` / `+0x109C` back to 1.0
                c.aim_point = Vec3::ZERO;
                c.boost_engine = 1.0;
                c.boost_speed = 1.0;
                c.ram = false;
                c.lane_override = 0.0;
                c.lane_bias = 0.0;
                // the slot is given back by state 9's own arm on the next tick (0x40503A)
                tracing::debug!(cop = i, "police stuck 160 ticks: GotoSlice to the reference car's slice (state 9)");
                self.goto_slice(table, i, target, others, ai_ticks);
                continue;
            }

            // `AIState_Chase::update` 0x40CF70 — `0x40CD70` first: the tracker, the geometry, the
            // aim point cleared, the ram mode lapsed, the boost charged and reset
            self.chase_geometry(table, i, target, tick, ai_ticks);
            let gap = self.cars[i].chase_gap;
            let lateral_err = self.cars[i].chase_lat_err;
            // `chase+0x64` = sign of the cop's `car+0xEC0` and `chase+0x60` = sign of the tracked
            // target speed `chase+0x40` (0x40CD81…0x40CDBB), `>= 0` giving +1 for both
            let sign_speed = if self.cars[i].speed >= 0.0 { 1.0 } else { -1.0 };
            let sign_tgt = if self.cars[i].tracker.speed >= 0.0 { 1.0 } else { -1.0 };

            // §27.6 dispatch: `dv` and `slow` read the target's own `car+0xEC0` (0x40CF9A, 0x40CFF4),
            // not the tracked one
            let dv = self.cars[i].speed - target.speed;
            let adv = dv.abs();
            let near = gap.abs() * 0.025;
            let slow = target.speed.abs() < 6.667;
            let mode = if near > adv && gap.abs() > 140.0 {
                ChaseMode::CatchUp
            } else if sign_tgt * sign_speed == 1.0 && !slow {
                // 0x40D09B…0x40D0B3: `chase+0x60 · chase+0x64 == 1` — the cop and the target travel
                // the same way. *(Until 2026-09-26 the port multiplied the cop's direction by the
                // sign of the lateral error `chase+0x6C`, so a cop to the left of a target going
                // its way never took CLOSE-QUARTERS.)*
                ChaseMode::CloseQuarters
            } else if sign_speed * gap >= 0.0 && !slow {
                ChaseMode::CatchUp
            } else {
                // 0x40D0D6…0x40D10D: `fcomp 26.67 ; test ah, 0x41 ; je push 0` — the flag is 0 when
                // `adv` is **above** 26.67, and the same idiom on `|chase+0x68|` against 80 (`[0x5B26D4]`),
                // so the ram is for a cop that is close and not closing fast. *(Until 2026-09-26 the
                // port — and `traffic.md` §17.5 / §27.6 — had both comparisons the other way round.)*
                ChaseMode::Pursue { hard: adv <= 26.67 && gap.abs() <= 80.0 }
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
            // `0x40CE80`'s human test is `0x40E6E0(cop, human)`, the plain slice gap
            let human_gap = track_distance(table, self.cars[i].slice, target.slice);
            self.apply_boost(i, tick, human_gap, true);
            // CLOSE-QUARTERS in the ram mode runs `0x40CE80(this, 0)` again (0x40D19F) — the boost
            // without the human suppression — before its `0x40AF70` (0x40D507). *(Until 2026-09-26
            // the port suppressed it near the human in the ram mode too.)*
            if matches!(mode, ChaseMode::CloseQuarters) && self.cars[i].ram {
                self.apply_boost(i, tick, human_gap, false);
            }

            let mut speed = self.base_target_speed(i, target);
            match mode {
                // §27.7: flat out, no cap at all; out of the slot (`chase+0x78 = 0`, 0x40D029)
                ChaseMode::CatchUp => self.cars[i].slot_ticks = 0,
                // §27.8, the formation controller in full: compare the sector the cop is in with
                // the one its formation slot wants, pick a speed to close the difference and bias
                // the lateral aim toward the slot.
                ChaseMode::CloseQuarters => {
                    let lvl = self.cars[i].level.min(2);
                    let dir = self.cars[i].dir;
                    let (hw, hl) = (target.half_width, target.half_length);
                    let ram = self.cars[i].ram;
                    let offset = self.cars[i].formation;
                    // `chase+0x58` / `+0x5C`: the sectors of the latched offset. In the ram mode the
                    // slot is "dead on" and the boost is not suppressed (0x40D194).
                    let (want_lon, want_lat) = if ram { (0, 0) } else { (self.cars[i].want_lon, self.cars[i].want_lat) };
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
                    // the target speed the slot implies, from the tracked `|chase+0x40|`
                    let mut want = self.cars[i].tracker.speed.abs();
                    if p < want_lon {
                        want *= 1.4; // behind the slot: chase
                    } else if p > want_lon || sub == -1 {
                        want *= close_quarters_factor(gap); // ahead of it: ease off
                    } else if gap <= 20.0 {
                        // 0x40D3BC: the **signed** `chase+0x68` against 20 (`test ah, 0x41 ; jne`), so a
                        // cop more than 20 m behind its slot keeps the target's speed here too.
                        // *(Until 2026-09-26 the port tested `|chase+0x68|` and eased such a cop off
                        // by the distance table.)* The branch at 0x40D3CC (`> 20` and `< 10`) is dead.
                        if sub == -2 {
                            want *= CLOSE_QUARTERS_SLOW[lvl];
                        }
                    } else {
                        want *= close_quarters_factor(gap);
                    }
                    // `cop+0xEB8 = dir · max(5, min(S, |cop+0xEB8|))`
                    speed = dir * want.min(speed.abs()).max(5.0);
                    if ok {
                        // `chase+0x38`, the tracked target lateral
                        let base = self.cars[i].tracker.lateral;
                        let (y, bias) = if ram {
                            (base, 30.0)
                        } else {
                            // `+ dir·(half width + 6)` when the cop has to swing out, else the
                            // slot's own lateral offset; then clamped onto the road
                            let y = base + if m != 0 { dir * (hw + 6.0) } else { dir * offset[0] };
                            // 0x40D5CF…0x40D638: clamped to `[−rec+0x48·rec+0x38, rec+0x4C·rec+0x3C]`
                            // of the cop's slice — lane width × lane count on each side, not the road
                            // half-widths `rec+0x40` / `+0x44` the port used until 2026-09-26
                            let sl = &table.slices[self.cars[i].slice.min(table.slices.len() - 1)];
                            (y.max(-(sl.lane_width_a * sl.lanes_a as f32)).min(sl.lane_width_b * sl.lanes_b as f32), 15.0)
                        };
                        // 0x40D642 / 0x40D64B: `car+0x1078 = y`, `car+0x107C = 15 / 30` — the lane
                        // pass that follows honours it through `0x402790`. *(Until 2026-09-26 the
                        // port wrote `y` into the lane target, which its lane pass then overwrote.)*
                        self.cars[i].lane_override = y;
                        self.cars[i].lane_bias = bias;
                    }
                }
                // §27.9: the distance ladder is a cap on the 100 m/s base, and `hard` rams
                ChaseMode::Pursue { hard } => {
                    let d = crate::traffic::oct_distance(self.cars[i].position, target.position);
                    let v = pursue_cap(self.cars[i].level, d);
                    speed = if self.cars[i].dir >= 0.0 { speed.min(v) } else { speed.max(-v) };
                    // out of the slot (`chase+0x78 = 0`, 0x40D6D6)
                    self.cars[i].slot_ticks = 0;
                    if hard {
                        // 0x40D9A7…0x40D9EF, after the lane pass: `car+0x1064… = target.pos`,
                        // `car+0x1070 = target+0xED0` — and `car+0x1078 = target+0xED0`, `+0x107C =
                        // 15`, which nothing reads before the next `0x40CD70` puts both back to 1.0
                        // (or the chase's dtor zeroes them), so they are not carried
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
        let speed = self.base_target_speed(i, target);
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
        let speed = self.base_target_speed(i, target);
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
    fn drive_car(&mut self, table: &SliceTable, tick: u64, i: usize, dt: f32, n_ticks: i32, world: Option<CopWorld<'_>>) {
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
        let sl = table.slices[self.cars[i].slice.min(table.slices.len() - 1)];
        let st_road = (sl.width_a, sl.width_b);
        // The exits of 0x4084C3…0x408646, taken before the drive chain (`traffic.md` §21.7), each
        // zeroing the AI's force / torque `car+0xD34…`, `+0xD4C…`, `+0xD70…` and damping:
        // * 0x4084C3 — knocked over, `car+0x42C == 1` (`Damage` 0x49489A on an impact past 80,
        //   [`Vehicle::knocked_over`]; only a reset clears it): `vel ×= 0.98` (0x408505) and
        //   `ω.y ×= 0.98` (0x408512). *(Not taken before 2026-09-26: the port's cops took no
        //   `Damage`.)*
        // * 0x40854C — `car+0x418 == 0` and the up axis `car+0x374` below 0.3 (`[0x5B25F0]`), on
        //   its side or its roof, and 0x4085EC — the target speed `car+0xEB8` exactly 0: `vel ×=
        //   0.94` (`[0x5B25F4]`, 0x40863C, 0x532360) and `ω.y ×= 0.98`. The damaged-out exit
        //   (`car+0x540`, 0.99) has no writer for a cop and the finished-racer exit needs
        //   `car+0x52C & 1`. *(Until 2026-09-25 the cops drove through all of them.)*
        {
            let c = &mut self.cars[i];
            let damp = if c.body.knocked_over {
                Some(0.98)
            } else if (c.body.airborne_count == 0 && c.orientation.row(1).y < 0.3) || c.target_speed == 0.0 {
                Some(0.94)
            } else {
                None
            };
            if let Some(k) = damp {
                c.accel_local = Vec3::ZERO;
                c.velocity *= k;
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
            // `0x408F20` → `0x4090C0` (0x4086A9, after the trim and the U-turn zero): with an
            // explicit aim point `car+0x1074 = car+0x1070` (0x40954E…0x409557). *(Until 2026-09-26
            // the port drove PURSUE's ram on the lane pass's target.)*
            if c.aim_point != Vec3::ZERO {
                target = c.aim_lateral;
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
        // `car+0x108C` (the lit 0x40AF70's `car+0x109C`): the brake force `car+0x1014→+0x80` is
        // multiplied by it when it exceeds 1 in both controllers (0x409B46 / 0x40A105), and the
        // in-control lateral pair too (0x409E95…0x409EB2)
        let rate_mul = self.cars[i].rate_mul;
        let brake = self.phys.brake_force * if rate_mul > 1.0 { rate_mul } else { 1.0 };
        let cmd = if aidrive::out_of_control(&st, &self.cfg.out_of_control, err, st_road, bend) {
            aidrive::out_of_control_command(&st, &self.cfg.out_of_control, err, accel, brake)
        } else {
            let grip = 1.0 + 0.0015 * aidrive::body_forward_speed(&st);
            aidrive::in_control_command(&st, &self.cfg.in_control, err, aim, grip, rate_mul, accel, brake)
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
        // the controller's `car+0xD4C…` (lateral, 0, forward; 0x40A18F / 0x40A1B9) and 0x40A1D0's
        // `car+0xD40…` = that force in the world (0x40A236 / 0x40A27B; `car+0xD70…` is 0)
        c.accel_local = Vec3::new(cmd.lateral, 0.0, cmd.forward);
        c.accel_world = c.orientation.row(0) * cmd.lateral + c.orientation.row(2) * cmd.forward;
        // 0x408750 at the end of every driving tick (0x4086E6), read to the instruction
        // (0x408750…0x408800): `car+0x344` (`vel.z`) above 115 (`[0x5B25FC]`), or `car+0x33C`
        // (`vel.x`) above 115, or the speed estimate `car+0x35C` above 115 with `car+0x7E == 0`
        // (always — `+0x7E` is only ever written 0) — **signed** compares, not magnitudes
        // (`fcomp; test ah,0x41; je`) — or the lateral offset `car+0xED0` below
        // `−[slice+0x40] − 35` or above `[slice+0x40] + 35` (`[0x5B25F8]`; the slice's own
        // half-width field, `width_a` in the track's frame) → `0x40F540(car, 1, 0)`
        // ([`Police::reset_at`]). *(Not run for a cop before 2026-09-26.)*
        let w_half = if table.backwards { sl.width_b } else { sl.width_a };
        let insane = c.velocity.z > SANITY_SPEED || c.velocity.x > SANITY_SPEED || c.speed_estimate() > SANITY_SPEED;
        let off_road = c.lateral < -(w_half + SANITY_OFF_ROAD) || c.lateral > w_half + SANITY_OFF_ROAD;
        if (insane || off_road) && let Some(w) = world {
            tracing::debug!(cop = i, tick, insane, off_road, lat = c.lateral, vel = ?c.velocity, "cop sanity net 0x408750");
            self.reset_at(table, w.grid, i, w.occupied);
        }
    }

    /// `AIHigh_Perp+0x78`: whether the perp is wanted. A patrol car picks that up in its own next
    /// tick (`update`, state 2/3's `0x405220`). *(Until 2026-09-25 this promoted every patrol car
    /// on the spot; the chase is now built where the original builds it, which also covers a cop
    /// state 1 has only just placed — it waits for its first state-2/3 tick.)*
    pub fn set_wanted(&mut self, wanted: bool) {
        self.wanted = wanted;
    }

    /// The rest of `0x403850` (0x4038B7…0x403937), after the roll: the cop is alongside and just
    /// ahead — `|cop+0xED0 − target+0xED0|` strictly between `target+0x3A8 + 1` and 4.0, and
    /// `0x40E860(cop, target) · cop+0xEB0` strictly between `2·(target+0x3B0 + 1)` and 12.0.
    /// `0x40E860` is `0x40E920`'s rule with the target's own position as the point ([`fine_gap`]).
    fn alongside(&self, table: &SliceTable, i: usize, target: &ChaseTarget) -> bool {
        let c = &self.cars[i];
        let lat = (c.lateral - target.lateral).abs();
        if !(target.half_width + 1.0 < lat && lat < 4.0) {
            return false;
        }
        let lon = fine_gap(table, c.slice, c.position, target.slice, target.position) * c.dir;
        2.0 * (target.half_length + 1.0) < lon && lon < 12.0
    }

    /// `0x403BD0(slot, car, role)` on the perp, read to the instruction 2026-09-26: `+0x70[role]++`
    /// (here [`Police::claims`]), the new pair stored at `+0x44` / `+0x48` — **entry 6**, the bottom
    /// of the roster — with the cop's position `+0x4C[slot] = 5`, then `0x403C00` bubbles it up.
    /// Whatever sat in entry 6 is overwritten (with at most four cops it is always empty by then).
    /// Returns the rank.
    fn roster_claim(&mut self, table: &SliceTable, i: usize) -> usize {
        self.roster[6] = Some(i);
        self.cars[i].roster_pos = 5;
        self.roster_bubble(table, i)
    }

    /// `0x403BA0(slot, car, role)`: `+0x70[role]--` and the cop's entry (`+0x1C + 8·pos`, entry
    /// `pos + 1`) set to −1 — a hole, which the cops below walk up through on their next state-4
    /// tick; the position itself is left as it was. Every caller tests `AIHigh+0x58` first, so it
    /// only runs for a cop that holds the perp; the target is cleared with it (`+0x58 = 0`).
    fn release(&mut self, i: usize) {
        if self.cars[i].target.take().is_some() {
            let e = (self.cars[i].roster_pos + 1).min(6);
            self.roster[e] = None;
        }
    }

    /// `0x403C00(slot, car)` on the perp, read to the instruction 2026-09-26 (0x403C00…0x403CAD) —
    /// the insertion step that ranks the chasers. From the cop's own position `pos` (entry `pos + 1`)
    /// it looks at the entry above: an empty one (−1) it always moves past; an occupied one only
    /// when that cop is more than 12 m **behind** it, `0x40E740(thatCop, thisCop.slice) ·
    /// perp+0xEB0 < −12` (0x403C45…0x403C71). Each move swaps the two entries and their positions
    /// (0x403C73…0x403C91); it stops at position 0 (entry 1) or at the first cop it cannot pass. The
    /// answer is the position — the rank, 0 being the cop furthest ahead. Only the calling cop
    /// moves up; another is moved down only when one passes it. Called by the claim (0x403BF5), by
    /// state 2 / 3 right after it (0x404316), and by state 4 every tick (0x404610), each cop in its
    /// own update. The perp's `car+0xEB0` is the race direction, +1 on this port's table.
    fn roster_bubble(&mut self, table: &SliceTable, i: usize) -> usize {
        let mut pos = self.cars[i].roster_pos;
        while pos > 0 {
            if let Some(j) = self.roster[pos] {
                // `· perp+0xEB0`, +1 here
                let behind = track_distance(table, self.cars[j].slice, self.cars[i].slice);
                if !(behind < -12.0) {
                    break;
                }
                self.cars[j].roster_pos = pos;
            }
            self.roster[pos + 1] = self.roster[pos];
            self.roster[pos] = Some(i);
            pos -= 1;
            self.cars[i].roster_pos = pos;
        }
        pos
    }

    /// State 2 → 4 (0x4043A8) / 3 → 4 (0x404E4B): a new `AIState_Chase`, ctor 0x40CB60 read to the
    /// instruction 2026-09-26 — the tracker `0x403620` with the gain `0x5CBEDC[SKILL]`, the boost
    /// budget `+0x7C` from `0x5CBEA0[lvl]+0x10`, then `0x40CCA0(target, 0x5CBEE8[lvl][rank])` (the
    /// formation latch, which runs `0x40CD70` — a tracker step and the geometry), then `car+0x1098`
    /// / `+0x109C = 1.0` (0x40CBA6), the blackout `+0x8C = 0` (0x40CBD8), `+0x44 = 0` and the stuck
    /// timer `+0x90 = 0` (0x40CBF1). The lights are not touched: they come on at the next tick's
    /// state-4 head. The state then runs the chase's update the same tick (0x405092), whose own
    /// `0x40CD70` steps the tracker a second time.
    fn build_chase(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, tick: u64, ai_ticks: i32) {
        let k = TRACKER_GAIN[(self.skill as usize).min(2)];
        let tracker = ChaseTracker::new(table, k, &self.cars[i], target);
        let c = &mut self.cars[i];
        c.chasing = true;
        c.parked = false;
        c.tracker = tracker;
        c.boost_budget = BOOST_BUDGET[c.level.min(2)];
        let offset = FORMATION[c.level.min(2)][c.rank.min(5)];
        self.retarget(table, i, target, offset, tick, ai_ticks);
        let c = &mut self.cars[i];
        c.boost_engine = 1.0;
        c.boost_speed = 1.0;
        c.blackout_until = 0;
        c.stuck_ticks = 0;
    }

    /// `0x40CCA0(chase, target, &offset)`, read to the instruction 2026-09-26: a different target
    /// resets the tracker (0x403650 — never in this port, where only the human is ever claimed and
    /// the chase is rebuilt for it); the offset is latched at `chase+0x4C`, its sectors worked out
    /// against the target's half width / length (`+0x5C`: `x < −hw` → −1, `x > hw` → +1; `+0x58` the
    /// same for `z` and `hl`, all strict), the ram mode, its deadline and the slot timer cleared
    /// (`+0x70`, `+0x74`, `+0x78`), and `0x40CD70` run.
    fn retarget(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, offset: [f32; 3], tick: u64, ai_ticks: i32) {
        let c = &mut self.cars[i];
        c.formation = offset;
        c.want_lat = sector(offset[0], -target.half_width, target.half_width);
        c.want_lon = sector(offset[2], -target.half_length, target.half_length);
        c.ram = false;
        c.ram_until = 0;
        c.slot_ticks = 0;
        self.chase_geometry(table, i, target, tick, ai_ticks);
    }

    /// `0x40CD70(chase)`, read to the instruction 2026-09-26 (0x40CD70…0x40CE7B): the tracker step
    /// (`0x4036F0`), `chase+0x6C = cop+0xED0 − chase+0x38`, `chase+0x68 = 0x40E920(cop, chase+0x18,
    /// chase+0x28)`, the aim point `car+0x1064…0x1070` zeroed, the ram mode lapsed once `gGameTick >=
    /// chase+0x74`, and the boost the last update left on charged and reset (0x40CE36…0x40CE72,
    /// [`Police::spend_boost`]). (`chase+0x60` / `+0x64`, the two direction signs, are read off the
    /// tracker and the car where the dispatch needs them.)
    fn chase_geometry(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, tick: u64, ai_ticks: i32) {
        let mut tr = self.cars[i].tracker;
        tr.step(table, &self.cars[i], target);
        let c = &mut self.cars[i];
        c.tracker = tr;
        c.chase_lat_err = c.lateral - tr.lateral;
        c.chase_gap = fine_gap(table, c.slice, c.position, tr.slice, tr.point);
        c.aim_point = Vec3::ZERO;
        c.aim_lateral = 0.0;
        // 0x40CE68 / 0x40CE72: `car+0x1078 = car+0x107C = 1.0` — a 1 m offset in the track's own
        // frame (the port's backwards table mirrors the lateral axis) at a bias of 1
        c.lane_override = if table.backwards { -1.0 } else { 1.0 };
        c.lane_bias = 1.0;
        if c.ram && tick >= c.ram_until {
            c.ram = false;
            c.ram_until = 0;
        }
        self.spend_boost(i, ai_ticks);
    }

    /// State 4 → 2 (0x4047D9): the target is gone, an `AIState_Normal` replaces the chase, whose
    /// deleting dtor 0x40CC10 zeroes `car+0x1064…0x1070`, `+0x1078`, `+0x107C` and puts
    /// `car+0x1098` / `+0x109C` back to 1.0. `car+0xEAC` is not written (the 0x4047CA store sits on
    /// the dead order branch), and the lights are left to state 2's head next tick.
    fn drop_chase(&mut self, i: usize) {
        self.release(i);
        let c = &mut self.cars[i];
        c.chasing = false;
        c.ram = false;
        c.lane_override = 0.0;
        c.lane_bias = 0.0;
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
        // `car+0x528` of each entry of `cars`, when the caller gave the others' (the race gives
        // them; the unit tests do not and the walk sorts the cars itself)
        let places: Vec<usize> = self
            .other_places
            .iter()
            .copied()
            .chain(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.place))
            .collect();
        let placed = self.other_places.len() == others.len();
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
        // world positions in the order of `cars` for 0x40E860's fine step (`traffic.md` §34.6): an
        // `others` entry the caller gave no position for falls back to the slice gap (NaN is never
        // within ±12 m of anything the fine step would accept, so it is filtered out below), same as
        // `TrafficSet::ai`'s `positions`. *(Until now `police.rs` called `find_blockers_placed` with
        // `None`, i.e. every lane-blocker gap was the coarse 6 m-per-slice one, §34.6's gap left for
        // this module.)*
        let positions: Vec<Vec3> = (0..others.len())
            .map(|k| self.other_positions.get(k).copied().unwrap_or(Vec3::NAN))
            .chain(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.position))
            .collect();
        let fine_ok = positions.iter().all(|p| p.is_finite());
        let me_pos = c.position;
        sc.find_blockers_placed(table, &me, &cars, &PERSONALITIES[0], fine_ok.then_some((me_pos, positions.as_slice())), placed.then_some((c.place, places.as_slice())));
        sc.score_own_side(&me, self.one_way, true);
        // §27.10 `0x40DA00`: +15 to whichever side the target is on (`0x401380` × 3), but only when
        // the lane corridor between the two cars is broken at the target's slice and whole at the
        // cop's; the stuck timer counts while it is broken at both. `chase+0x68` is the longitudinal
        // gap 0x40CD70 measured this tick (`CopCar::chase_gap`).
        let mut stuck = self.cars[i].stuck_ticks;
        if chase {
            let gap = self.cars[i].chase_gap;
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
        // 0x402A5A…0x402A7D: a positive `car+0x107C` first tries the held offset `car+0x1078`
        // (`0x402790`), which bypasses the squeeze-past and the lane centre when it wins
        let (held, bias) = (self.cars[i].lane_override, self.cars[i].lane_bias);
        let lateral_target = if bias > 0.0 && sc.override_holds(table, &me, held, bias) { held } else { sc.lateral_target(table, &me, lane, slot, &cars) };
        let capped = sc.capped_target(&me, lane, lateral_target, 0.0, &cars);
        let c = &mut self.cars[i];
        c.scan = scan;
        c.stuck_ticks = stuck;
        // `car+0xEB4` is written whatever the aim point; the drive's `0x4090C0` takes `car+0x1070`
        // instead when there is an explicit one. *(Until 2026-09-26 the port kept the old lane
        // target under an aim point.)*
        c.lateral_target = lateral_target;
        c.target_speed = capped;
    }

    /// The 64 Hz half — the two shared Sc64 integrators every car record runs (`car+0xE24` /
    /// `+0xE28`): `0x49B7A0` `pos += vel · 1/64` and `0x49B8E0`'s rotation by the **whole** `ω/64`
    /// ([`nfs_physics::vehicle::orientation_step`]: the pitch and roll rates `Susp_update` wrote
    /// into `car+0x388` turn the body, as for a racer; until 2026-09-26 the port turned a cop
    /// about its up axis only and stood it on the contact plane instead) — then the route frame:
    /// the slice, the lateral offset `car+0xED0` and the route speed `car+0xEC0`, exactly as
    /// traffic does. The ground contact is [`Police::ground`], on the 32 Hz list; without a grid
    /// (the unit tests' [`Police::integrate`] alone) a cop keeps the slice's height (port
    /// fallback).
    pub fn integrate(&mut self, table: &SliceTable, dt: f32) {
        self.integrate_at(table, dt, false)
    }

    /// [`Police::integrate`] with `grounded` saying whether [`Police::ground`] runs: when it does
    /// not (no collision grid), the height is the slice's (port fallback, no original counterpart).
    pub fn integrate_at(&mut self, table: &SliceTable, dt: f32, grounded: bool) {
        self.integrate_tick(table, dt, grounded, None)
    }

    /// [`Police::integrate_at`] with the 64 Hz game tick: on an even tick every cop's `car+0xAF8`
    /// ([`CopCar::progress`]) is written first, from the slice the tracker found last tick (0 for
    /// a cop out of the world), as 0x4169B0 does before the integrators — as
    /// `TrafficSet::integrate_tick` keeps the traffic's.
    pub fn integrate_tick(&mut self, table: &SliceTable, dt: f32, grounded: bool, tick: Option<u64>) {
        if let Some(t) = tick
            && t % 2 == 0
        {
            for c in self.cars.iter_mut() {
                c.progress = if c.active { table.frac(c.slice) } else { 0.0 };
            }
        }
        for c in self.cars.iter_mut().filter(|c| c.active) {
            c.position += c.velocity * dt;
            // 0x49B7A0 moves the four contacts `wheel+0x40` by the same `vel/64` as the body
            // (0x49B7F5…0x49B84C; every car but a smackable) — what the skid marks read after it
            if grounded {
                for w in &mut c.body.wheels {
                    w.world_pos += c.velocity * dt;
                }
            }
            c.orientation = nfs_physics::vehicle::orientation_step(c.orientation, c.angular_velocity * dt);
            c.slice = table.find_slice(c.position, c.slice);
            let sl = &table.slices[c.slice];
            let lat_axis = sl.lateral.normalize_or_zero();
            c.lateral = (c.position - sl.centre).dot(lat_axis);
            c.speed = c.velocity.dot(sl.forward.normalize_or_zero());
            if !grounded {
                c.position.y = sl.centre.y + c.rest_height();
                c.velocity.y = 0.0;
            }
        }
    }

    /// Put a cop on the road at a spawn point (§25.10 step 5).
    pub fn deploy(&mut self, table: &SliceTable, i: usize, record: usize, tick: u64) -> bool {
        self.deploy_as(table, i, record, tick, true, None)
    }

    /// State 1's type-1 arm (0x40419B…0x4042BE): the cop is placed (0x407890) in **state 2**
    /// (`AIState_Normal`, a rolling record) or **state 3** (`AIState_Stop` mode 1, a parked one), with
    /// its lights off (0x403F4A), and runs that state's update this tick — never state 4. `chasing`
    /// says whether the perp is wanted (`AIHigh_Perp+0x78`, as `set_wanted`); a wanted perp is
    /// acquired on the cop's next tick, whose chase update runs unlit, and the tick after that lights
    /// it (§17.5b). *(Until 2026-09-25 `chasing = true` put the cop straight into a lit chase.)*
    ///
    /// `grid` is the track's collision grid for `Car_placeAtSlice` 0x49B090; without one (the unit
    /// tests) the placement is taken against the slice's own plane (port fallback).
    pub fn deploy_as(&mut self, table: &SliceTable, i: usize, record: usize, tick: u64, chasing: bool, grid: Option<&TriangleGrid>) -> bool {
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
        // 0x407690: `car+0x7F = 1`, 0x40A990, then `Car_placeAtSlice` 0x49B090 (car, car+8,
        // {car+0xEB4, 1.0, 0}, car+0xEB0) (0x4076BC…0x4076E8) — a cop is not an opponent
        // (`car+0x52C & 8` clear), so the offset is slice-local: the placement point is `centre +
        // lateral·L + 1.0·N` in the frame the **tracker's patch normal** gives (0x49B22F…), the
        // trackers seated on the slice centre, and the body is then dropped by `BodyHeight`'s
        // `car+0x424` against that patch (0x49B493…0x49B4C7, `Vehicle::place_at_slice`). *(Until
        // 2026-09-26 the port placed the cop on the slice's own plane and settled it on the
        // patch at the next 64 Hz step: on a crowned or banked road it was drawn about a metre
        // above the road for the deploy tick.)*
        match grid {
            Some(g) => {
                c.body.rearm_after_reset();
                c.sync_body_in();
                c.body.place_at_slice(g, &c.specs, sl.centre, sl.forward.normalize_or_zero(), Vec3::new(lateral, 1.0, 0.0), false, dir < 0.0);
                c.sync_body_out();
                c.lateral = (c.position - sl.centre).dot(sl.lateral.normalize_or_zero());
                c.lateral_target = c.lateral;
            }
            None => {
                c.position = sl.centre + sl.lateral.normalize_or_zero() * lateral + Vec3::Y * (c.rest_height() / up.y.max(0.1));
                c.orientation = Mat3::from_cols(right, fwd.cross(right).normalize_or_zero(), fwd).transpose();
                c.body.tracker = nfs_physics::collision::PatchTracker::new(c.position);
                c.sync_body_in();
            }
        }
        // 0x4076ED…: the velocity along the car's forward row `car+0x37C` while `car+0xEC0 != 0`
        c.velocity = c.orientation.row(2) * speed.abs();
        c.angular_velocity = Vec3::ZERO;
        c.body.velocity = c.velocity;
        c.body.angular_velocity = Vec3::ZERO;
        c.accel_local = Vec3::ZERO;
        c.accel_world = Vec3::ZERO;
        c.active = true;
        c.chasing = false;
        // state 1 releases any claim before it places the car (0x4041E6 → 0x403BA0) — the
        // give-up that sent it to purgatory already did
        c.parked = !moving;
        c.placed = true;
        c.lights = false;
        c.aim_point = Vec3::ZERO;
        c.stuck_ticks = 0;
        c.goto_slice = None;
        // the spawn's 0x406CD0 (§13, through 0x407890): `word car+0x418 = 0`; the record is a
        // fresh car — no damage flag, no countdown
        c.body.airborne_count = 0;
        c.body.knocked_over = false;
        c.body.reset_countdown = 0;
        c.body.impact = 0.0;
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
        // the give-ups release the slot (0x404402 / 0x404942)
        self.release(i);
        let c = &mut self.cars[i];
        c.active = false;
        c.ram = false;
        c.lane_override = 0.0;
        c.lane_bias = 0.0;
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
