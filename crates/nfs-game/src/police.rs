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
/// `mgr+0x198` init — a record cannot be reused within 2560 ticks (40 s), and the initialiser makes
/// every point unusable for the first 40 s of a race.
pub const SPAWN_COOLDOWN: i64 = 2560;
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

/// The `.cop` records plus the state `CopManager` keeps about them.
#[derive(Debug, Clone)]
pub struct CopManager {
    pub file: CopFile,
    /// `mgr+0x198` — the tick each record was last handed out
    last_use: Vec<i64>,
    /// `mgr+0x328` — the perp's search cursor, in route positions
    cursor: Option<i32>,
}

impl CopManager {
    pub fn new(file: CopFile) -> Self {
        let n = file.records.len();
        Self { file, last_use: vec![-SPAWN_COOLDOWN; n], cursor: None }
    }

    pub fn is_empty(&self) -> bool {
        self.file.records.is_empty()
    }

    /// `p = 16 / nRecords`, doubled for a clean perp and forced to certainty on the hardest skill
    /// (§25.5). With the shipped files that is 0.76 on autobahn and ≥ 1 everywhere else.
    pub fn spawn_probability(&self, clean: bool, skill: u8) -> f32 {
        if skill >= 2 {
            return 1.0;
        }
        let n = self.file.records.len().max(1) as f32;
        (16.0 / n) * if clean { 2.0 } else { 1.0 }
    }

    /// `0x405440` — the spawn-point search. Returns the index of a usable record, having stamped its
    /// cooldown. `humans` are the route positions of the human cars, for the 172 m veto.
    ///
    /// `seen` is `pai+0x70[role]`, the cops already assigned to this perp, and it does two things:
    /// `if seen >= lim: continue` caps how many ever come out — `lim` is 2 for a wanted perp
    /// ([`HEAT_LIM_ROLE0`]) and, for a clean one, 1 only while no cop is inside
    /// [`CLEAN_PERP_CLEARANCE`] (`cop_near`, which is `0x403120(perp.slice, 500.0) == 0`) — and
    /// `seen != 0` then makes reinforcements free of the probability roll (0x405620). The port has
    /// no claim system, so the caller passes the number of **active** cops: in the original a cop
    /// spawns into state 2 and claims as soon as it acquires the perp (0x403FCA), which is at once.
    #[allow(clippy::too_many_arguments)]
    pub fn find_spawn(&mut self, table: &SliceTable, perp_slice: usize, perp_speed: f32, tick: i64, roll01: f32, clean: bool, skill: u8, humans: &[usize], seen: usize, cop_near: bool) -> Option<usize> {
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
        let n = table.lap_len as i32;
        let dir = if perp_speed < 0.0 { -1 } else { 1 };
        let marker = table.add(perp_slice, SPAWN_LEAD_SLICES * dir);
        let mp = table.route_pos.get(marker).copied().unwrap_or(0) as i32;
        let prev = match self.cursor {
            Some(c) => c,
            None => {
                // the first query only arms the cursor
                self.cursor = Some(mp);
                return None;
            }
        };
        self.cursor = Some(mp);
        // the slices the marker swept since the last query, in travel order
        let mut sweep = mp - prev;
        if sweep > n / 2 {
            sweep -= n;
        } else if sweep < -n / 2 {
            sweep += n;
        }
        if sweep == 0 || sweep.abs() >= MAX_SWEEP {
            return None;
        }
        let step = sweep.signum();
        let mut p = prev;
        let mut passed = Vec::new();
        while p != mp {
            p = (p + step).rem_euclid(n);
            passed.push(table.route[p as usize]);
        }

        let accept = seen != 0 || roll01 < self.spawn_probability(clean, skill);
        for slice in passed {
            for (i, r) in self.file.records.iter().enumerate() {
                if !r.spawnable() || r.slice() as usize != slice {
                    continue;
                }
                if tick - self.last_use[i] < SPAWN_COOLDOWN {
                    continue;
                }
                // a human too close to the spawn point would see the car appear
                if humans.iter().any(|&h| track_distance(table, slice, h).abs() < HUMAN_VETO) {
                    continue;
                }
                if !accept {
                    continue;
                }
                self.last_use[i] = tick;
                return Some(i);
            }
        }
        None
    }

    /// Where and how a record places its car (§25.10 step 5): the lateral offset from the centreline
    /// and whether the car is rolling. `route_dir` is `REVERSE ? −1 : +1`.
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
        m.cursor = Some(96);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
        m.cursor = Some(96);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), Some(0), "and is free again after it");
        // a jump of six slices or more is too big a sweep to search
        m.cursor = Some(90);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 9000 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), None);
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
        m.cursor = Some(96);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 2, true), None, "lim[0] is 2");
        // clean: one, and only while no cop is inside 500 m (0x403120)
        let mut m = mgr(&[100], true);
        assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, true, 0, &[], 0, false), None, "arms the cursor");
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, true, 0, &[], 0, true), None, "a cop is already close by");
        m.cursor = Some(96);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, true, 0, &[], 0, false), Some(0));
        m.cursor = Some(96);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000 + SPAWN_COOLDOWN, 0.0, true, 0, &[], 1, false), None, "one patrol is the limit");
    }

    #[test]
    fn a_human_next_to_the_point_vetoes_it() {
        let t = ring(400);
        let mut m = mgr(&[100], true);
        m.cursor = Some(96);
        // a human 20 slices (120 m) from slice 100 is inside the 172 m veto
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[120], 1, true), None);
        // 40 slices (240 m) away is outside it
        m.cursor = Some(96);
        assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[140], 1, true), Some(0));
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
        assert!(p.cars[0].active && p.cars[0].lights, "a deployed cop has its lights on");
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

    #[test]
    fn the_dispatch_picks_the_documented_behaviour() {
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 0)], 0, false);
        p.deploy(&t, 0, 0, 0);
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
        // only becomes a chaser when the order arrives.
        let t = ring(400);
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
        let mut p = Police::new(file, vec![CopCar::new(0, Vec3::new(0.9, 0.6, 2.1), 1500.0, 2)], 0, false);
        assert!(p.deploy_as(&t, 0, 0, 1000, false));
        assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights, "patrolling, lights off");
        let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        p.update(&t, 1002, &tgt, &[]);
        assert_eq!(p.cars[0].mode, None, "a patrol car runs no chase behaviour");
        assert!(p.cars[0].target_speed.abs() < 30.0, "traffic pace, not the pursuit base: {}", p.cars[0].target_speed);
        // the order turns it into a chaser
        p.set_wanted(true);
        assert!(p.cars[0].chasing && p.cars[0].lights);
        p.update(&t, 1004, &tgt, &[]);
        assert!(p.cars[0].mode.is_some(), "now it chases");
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
/// `chase+0x90 > 160` — two and a half seconds unable to reach the target's lane.
pub const STUCK_LIMIT: i32 = 160;

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
/// `0x5CBEA0[level]+0x0C` — how long the close-in mode and the boost budget last, in ticks.
pub const CLOSE_IN_TICKS: [u64; 3] = [0x200, 0x300, 0x480];

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
    /// The cop is chasing (`AIHigh_Cop` state 4). A cop that spawned while the perp was clean
    /// patrols instead (states 2/3) until an order arrives — §17.4.
    pub chasing: bool,
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
    pub position: Vec3,
    pub orientation: Mat3,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
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
    /// while un-pursued for over 12.5 s (`perpAI+0xAC > 0x320`). That counter is reset the instant
    /// a chaser is claimed and the player's AI runs before any cop's, so with one human the rule
    /// can never fire (`traffic.md` §17.5) — hence no writer here either.
    pub blackout_until: u64,
    /// `chase+0x90`
    pub stuck_ticks: i32,
    /// `car+0x104C` and its timers — the gear the drive model runs in (`aidrive::Gear`)
    pub gear: aidrive::Gear,
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
}

impl CopCar {
    pub fn new(model: usize, half_extents: Vec3, mass: f32, level: usize) -> Self {
        Self {
            gear: aidrive::Gear::default(),
            model,
            active: false,
            lights: false,
            chasing: false,
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
            level,
            scan: 0,
            mode: None,
        }
    }

    pub fn speed_estimate(&self) -> f32 {
        oct_norm(self.velocity.x, self.velocity.z)
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
    /// `WEATHER` (`[0x657A24] > 0`). `0x4081B0` scales the longitudinal rate by **0.9** and it does
    /// so *after* the traffic/1.0 split at 0x4081FC, so it applies to a cop as much as to a traffic
    /// car (0x40825C…0x40826E, `[0x5B25E8]` = 0.9). The shipped `fe.txt` says `weather = 0`, so it
    /// is off in a normal race — and it is the option, not the track's night flag.
    pub weather: bool,
    one_way: bool,
}

impl Police {
    pub fn new(file: CopFile, cars: Vec<CopCar>, skill: u8, one_way: bool) -> Self {
        Self { manager: CopManager::new(file), cars, cfg: nfs_formats::trafcfg::TrafficConfig::default(), phys: CopPhysics::default(), violation: Violation::None, route_dir: 1.0, skill, one_way, weather: false, budget: duty_budget(1, false) }
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

    /// `0x40CE80` — the rubber band, suppressed near a human and blacked out after one has been fast
    /// while un-pursued.
    fn apply_boost(&mut self, i: usize, tick: u64, gap_to_human: f32) {
        let c = &mut self.cars[i];
        c.boost_engine = 1.0;
        c.boost_speed = 1.0;
        if c.boost_budget <= 0 || tick < c.blackout_until {
            return;
        }
        if gap_to_human.abs() < BOOST_SUPPRESS_AHEAD.max(BOOST_SUPPRESS_BEHIND) {
            return;
        }
        c.boost_engine = BOOST_ENGINE;
        c.boost_speed = BOOST_SPEED;
        c.boost_budget -= AI_TICKS;
    }

    /// One AI tick for every active cop (§27.6's dispatch and the three behaviours).
    pub fn update(&mut self, table: &SliceTable, tick: u64, target: &ChaseTarget, others: &[LaneCar]) {
        // `AIHigh+0x60`: the chasers are re-sorted by track distance (0x403C00) and each one's place
        // in that queue picks its formation slot. **Rank 0 is the cop furthest ahead**: the insert
        // loop walks the roster down while the entry it passes is more than twelve metres *behind*
        // the target (`0x40E740(cop, perp.slice)` is the signed gap from the perp to that cop,
        // 6 m a slice, wrapped into half a lap, and 0x403C66 compares it with −12). CONFIRMED —
        // which is also what slot 0 wants, since it sits 8 m ahead of the target.
        let mut order: Vec<usize> = (0..self.cars.len()).filter(|&i| self.cars[i].active).collect();
        order.sort_by(|&a, &b| track_distance(table, self.cars[b].slice, target.slice).partial_cmp(&track_distance(table, self.cars[a].slice, target.slice)).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, &i) in order.iter().enumerate() {
            self.cars[i].rank = rank;
        }
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                continue;
            }
            // §17.5 give-up: too far, too long unable to reach the target's lane, or the race is over
            let d = crate::traffic::oct_distance(self.cars[i].position, target.position);
            if d > GIVE_UP_DISTANCE || self.cars[i].stuck_ticks > STUCK_LIMIT || target.finished {
                self.retire(i);
                continue;
            }
            // A cop that spawned while the perp was clean patrols (§17.4 states 2/3): it drives the
            // road like traffic does and never runs the chase. The original moves it to state 4 on
            // an *order*, and claims a slot on the perp as it goes (0x403BD0 bumps the perp's
            // per-role chaser count and inserts the cop into its roster; 0x403BA0 gives the slot
            // back). That roster is bookkeeping on the target rather than a gate on who may chase,
            // so the port promotes every patrol car at once and ranks them by the same sort —
            // MEDIUM only in *when* each cop is told, not in what it then does.
            if !self.cars[i].chasing {
                self.patrol(table, tick, i, target, others);
                continue;
            }

            // §27.5 geometry
            let gap = track_distance(table, self.cars[i].slice, target.slice);
            let lateral_err = self.cars[i].lateral - target.lateral;
            let sign_speed = if self.cars[i].speed >= 0.0 { 1.0 } else { -1.0 };
            let sign_lat = if lateral_err >= 0.0 { 1.0 } else { -1.0 };
            self.cars[i].aim_point = Vec3::ZERO;
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
            // CATCH-UP (0x40D026) and PURSUE (0x40D6EE) point the car at the target;
            // CLOSE-QUARTERS writes no `cop+0xEAC` at all, so a cop holding a slot ahead of the
            // target must not turn round. Every behaviour spends the boost.
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
                    self.cars[i].slot_ticks = if lat == want_lat && p == want_lon { self.cars[i].slot_ticks + AI_TICKS } else { 0 };
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
                    let v = pursue_cap(self.cars[i].level, d);
                    speed = if self.cars[i].dir >= 0.0 { speed.min(v) } else { speed.max(-v) };
                    if hard {
                        self.cars[i].aim_point = target.position;
                        self.cars[i].aim_lateral = target.lateral;
                    }
                }
            }
            // §27.7's U-turn: asking for the opposite direction makes `0x409720`'s throttle window
            // fail (it requires `car+0xEAC == car+0xEB0`), so the cop brakes to a stop, flips and
            // drives back. The port expresses the same thing by dropping the target speed to zero.
            if self.cars[i].requested_dir != self.cars[i].dir {
                speed = 0.0;
            }
            self.cars[i].target_speed = speed;

            // §12/§23 lane pass, with §27.10's +15 toward the target's lane
            self.lane_pass(table, i, target, others);

            self.drive(table, tick, i);
        }
    }

    /// The lane scorers plus `0x40DA00`'s bias toward the target's lane.
    /// One tick of a patrolling cop: the traffic speed ladder, the same lane pass and the same drive
    /// model, with no chase behaviour at all (§17.4 states 2/3).
    fn patrol(&mut self, table: &SliceTable, tick: u64, i: usize, target: &ChaseTarget, others: &[LaneCar]) {
        let speed = self.base_target_speed(&self.cars[i], target.speed);
        {
            let c = &mut self.cars[i];
            c.mode = None;
            c.target_speed = speed;
            c.requested_dir = c.dir;
            c.boost_engine = 1.0;
            c.boost_speed = 1.0;
            c.aim_point = Vec3::ZERO;
        }
        self.lane_pass(table, i, target, others);
        self.drive(table, tick, i);
    }

    /// §27.2's drive model, the same one traffic runs.
    fn drive(&mut self, table: &SliceTable, tick: u64, i: usize) {
        let sl = table.slices[self.cars[i].slice.min(table.slices.len() - 1)].clone();
        let st_road = (sl.width_a, sl.width_b);
        // §27.2a, exactly as for traffic: trim while reversing, then flip the gear every 32 ticks
        // for as long as the car is off the lanes and tipped (or already backing up)
        let tick = tick as i64;
        {
            let c = &mut self.cars[i];
            let lane = lane_of(&sl, c.lateral);
            let up_y = c.orientation.row(1).y;
            let lat_vel = c.velocity.dot(sl.lateral.normalize_or_zero());
            let (speed, target) = aidrive::reverse_trim(&mut c.gear, tick, c.lateral, c.lateral_target, lat_vel, c.target_speed);
            c.target_speed = speed;
            c.lateral_target = target;
            if aidrive::in_trouble(&mut c.gear, lane, sl.lanes_a as i32, sl.lanes_b as i32, up_y, crate::traffic::AI_TICKS) {
                aidrive::toggle_gear(&mut c.gear, tick, c.speed, aidrive::GEAR_PERIOD);
            }
        }
        let mut st = self.cars[i].as_ai_state(self.phys.max_lat_acc, self.phys.max_yaw_acc);
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
        aidrive::integrate(&mut st, &cmd, 1.0 / 32.0);
        let c = &mut self.cars[i];
        c.velocity = st.velocity;
        c.angular_velocity = st.angular_velocity;
        // `0x408360` only grants the requested direction once the car is slow enough to turn round
        if c.speed_estimate() < 8.889 {
            c.dir = c.requested_dir;
        }
    }

    /// Promote every patrol car to a chaser — what an order from the perp's claim system does in the
    /// original (§17.4 state 3 → 4).
    pub fn set_wanted(&mut self, wanted: bool) {
        for c in self.cars.iter_mut().filter(|c| c.active) {
            if wanted && !c.chasing {
                c.chasing = true;
                c.lights = true;
            }
        }
    }

    fn lane_pass(&mut self, table: &SliceTable, i: usize, target: &ChaseTarget, others: &[LaneCar]) {
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
        // §27.10: +15 to whichever side the target is on, while the lane corridor is open
        let gap = track_distance(table, me.slice, target.slice);
        if gap.abs() <= 500.0 {
            match target.lane.cmp(&me.lane) {
                std::cmp::Ordering::Greater => sc.score[2] += 15.0,
                std::cmp::Ordering::Less => sc.score[0] += 15.0,
                std::cmp::Ordering::Equal => {}
            }
        }
        sc.score_blockers(&me);
        sc.score_abreast(table, &me, &cars);
        sc.score_lane_exists(table, &me, &mut scan, AI_TICKS);
        sc.score_narrowing(table, &me);
        sc.score_shoulders(table, &me, &cars);
        let (slot, lane) = sc.pick(&me);
        let lateral_target = sc.lateral_target(table, &me, lane, slot, &cars);
        let capped = sc.capped_target(&me, lane, lateral_target, 0.0, &cars);
        let c = &mut self.cars[i];
        c.scan = scan;
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
            match ground.and_then(|g| g.ground(c.position.x, c.position.y + 2.0, c.position.z, 6.0)) {
                Some(h) => {
                    c.position.y = h.point.y + crate::traffic::RIDE_HEIGHT;
                    let up = h.normal.normalize_or_zero();
                    let f0 = c.orientation.row(2);
                    let fwd = (f0 - up * f0.dot(up)).normalize_or_zero();
                    let right = up.cross(fwd).normalize_or_zero();
                    if fwd.length_squared() > 0.5 && right.length_squared() > 0.5 {
                        c.orientation = Mat3::from_cols(right, fwd.cross(right).normalize_or_zero(), fwd).transpose();
                    }
                }
                None => c.position.y = sl.centre.y + crate::traffic::RIDE_HEIGHT,
            }
            c.velocity.y = 0.0;
        }
    }

    /// Put a cop on the road at a spawn point (§25.10 step 5).
    pub fn deploy(&mut self, table: &SliceTable, i: usize, record: usize, tick: u64) -> bool {
        self.deploy_as(table, i, record, tick, true)
    }

    /// `chasing = false` puts the cop on patrol (§17.4 states 2/3): it drives the road at traffic
    /// pace with its lights off until the perp is wanted.
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
        c.position = sl.centre + sl.lateral.normalize_or_zero() * lateral + up;
        c.orientation = Mat3::from_cols(right, fwd.cross(right).normalize_or_zero(), fwd).transpose();
        c.velocity = fwd * speed.abs();
        c.angular_velocity = Vec3::ZERO;
        c.active = true;
        c.chasing = chasing;
        c.lights = chasing;
        c.aim_point = Vec3::ZERO;
        c.stuck_ticks = 0;
        // `0x5CBEA0[level]+0x0C` — the boost budget in ticks, 8 / 12 / 18 s
        c.boost_budget = [0x200, 0x300, 0x480][c.level.min(2)];
        c.blackout_until = tick;
        tracing::debug!(cop = i, slice, lateral, moving, "police deployed");
        true
    }

    fn retire(&mut self, i: usize) {
        let c = &mut self.cars[i];
        c.active = false;
        c.lights = false;
        c.velocity = Vec3::ZERO;
        c.angular_velocity = Vec3::ZERO;
        c.mode = None;
        tracing::debug!(cop = i, "police gave up");
    }
}

/// `[0x5E6654]` — the 64 Hz ticks one AI step covers.
const AI_TICKS: i32 = 2;
