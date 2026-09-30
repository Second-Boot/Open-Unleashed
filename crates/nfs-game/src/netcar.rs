//! **0x4A0680 — what the receiver does to a *stale* remote car** (`RE/notes/network.md` §11.21).
//!
//! A remote car whose block lag has run past its limits (`car+0x70`, `Remote::stale`) is no longer
//! placed by its packets (0x49B9AB…0x49B9F1: `car+0xDA8 = 1`, the apply 0x4A0A40 is skipped). Its
//! controller copy 0x4991E0 then takes the **AI arm** — 0x40B650 / 0x40BFE0 write the throttle
//! `car+0xD9C`, the steering `+0xDA0` and the brake `+0xD9D`, the very routines an opponent's
//! `AIState_Racing::update` runs (`crate::ai::AiDriver::controls`) — and, with the block held
//! (`car+0xDAC`) and the car stale (`+0x70 == 1`), calls 0x4A0680 (0x499243…0x499262), which
//! compares the car's progress with where the last block's velocity would have carried it and either
//! puts the car back on the road or brakes it. Read to the instruction (Ghidra pseudo-C and the
//! disassembly of 0x4A0680…0x4A0A31); constants from `.rdata`:
//!
//! | address | value | use |
//! |---|---|---|
//! | `[0x5B3CA0]` | 0.01 | the block's velocity `i16` scale |
//! | `[0x5B2828]` | 1/64 | one tick |
//! | `[0x5B2460]` | 6.0 | a slice's length, m |
//! | `[0x5B4A60]` | 250.0 | the progress error past which the car is put on the road |
//! | `[0x5B49D4]` | 1/6 | metres → slices |
//! | `[0x5B24A8]` | 1.0 | the two "standing still" speed limits |
//! | `[0x5B2400]` | 5.0 | the error above which a standing car is put on the road |
//! | `[0x5B23F0]` | 0.5 | the correction's ratio of speed error to distance error |
//! | `[0x5B241C]` | 10.0 | the error floor of the brake rule |
//! | `[0x5B4A90]` | 25.5 | metres → the brake byte |
//!
//! The rule, with `N = [0x628BC0]+4` the lap's slice count (`SliceTable::lap_len`), `b` the block
//! (`+0x3C` tick, `+0x4A` velocity ×0.01, `+0x50` position `P`), `age = gGameTick − tick`:
//!
//! 1. the tracker copy of `car+8` is updated at `P` (0x474440: `SliceTable::track_slice` with the car's
//!    slice and preferred section) and gives the slice `sb`; `along = slice(sb)+0x1C · v` (0x532910);
//! 2. `err = (0x416290(sb) + along · (1/64) · age / (6N) − car+0xAF8) · 6N` — the block's progress
//!    carried on by its speed, less the car's own, in metres. **`car+0xAF8` counts laps**
//!    (`lap + frac − unlaps`), where 0x416290 is the bare fraction, so from the second lap on the error is
//!    a whole lap's length too large and the first rule below fires: the original's own arithmetic;
//! 3. `|err| > 250`: the car is put on the road — `s = 0x4817D0(sb, round(err/6), −1)`, `ω = 0`,
//!    `v = slice(s)+0x1C · along`, the forward row `car+0x37C = slice(s)+0x1C`, `car+0x330 = P`, then
//!    0x4A02A0 ([`nfs_physics::vehicle::Vehicle::net_stale_place`]);
//! 4. otherwise, only when `e = −err > 0` (the car is **ahead** of the block's progress), with `own` the
//!    car's speed along its own slice `slice(car+8)+0x1C · car+0x33C` and `dv = along − own`:
//!    a. `|dv| < 1`, `|own| < 1` and `e > 5` (both standing): put on the road as in 3 but on `sb` itself
//!       (0x4A0930…0x4A0994);
//!    b. else when `dv ≤ 0` or `dv ≤ e/2`: the brake rule — `e' = max(e, 10)`, `car+0xD9D = (i8) round(e'·25.5)`
//!       (the low byte: 255 at the floor, wrapping above 10.0 m), `car+0xD9C −= min(car+0xD9C, round(e'·25.5))` — a
//!       throttle byte can never exceed that, so the throttle is **0**.
//!
//! Backwards races (NQ, `network.md` §11.23): **the original never reverses its slice table** — `[0x657430]` is
//! tested by 0x416290 (`frac = 1 − frac`, and 0.0 for 1.0 on a circuit, 0x4162E4…0x416302), by the tracker, the
//! AI and `Grid_slot`, but **not by 0x4A0680 itself** (the function's only `[0x657430]`-dependent input is its call
//! of 0x416290). Its `+0x1C` row and its 0x4817D0 walk (`d > 0` = the raw table's forward) are therefore the *raw*
//! ones: on a backwards race the block's `along` is negative while the sender drives (the car is carried the wrong
//! way, the put-on-road walk goes the wrong way and the car is faced along the raw forward — the original's own
//! arithmetic, kept). This port's reversed table ([`SliceTable::reversed`]) has the rows negated and the walk
//! inverted, so the raw forward is `−forward` and a raw step `d` is `add_graph(s, −d)` (`add_graph` negates it again
//! on a reversed table). `frac` is the reversed table's own (`(N−1−p)/N`, the original's `1 − p/N`, one slice
//! apart); the car's `car+0xAF8` (`RaceProgress`) is the same table's, so the offset cancels in `err`.
//! Exercised by a two-instance backwards run (§11.23).

use glam::Vec3;
use nfs_physics::vehicle::NetBlock;

use crate::race::SliceTable;

/// `[0x5B4A60]`
pub const PUT_ON_ROAD_METRES: f64 = 250.0;

/// What 0x4A0680 decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StalePlace {
    /// nothing: the AI arm's controls stand
    Left,
    /// the car is put on the road (0x4A0803 / 0x4A0930): its new position is the block's `P`, its
    /// forward row and velocity direction the slice's raw `+0x1C`, its speed `along`
    PutOnRoad { forward: Vec3, along: f32 },
    /// the brake rule: throttle 0 and this brake byte
    Brake(u8),
}

/// The decision of 0x4A0680 (see the module docs). `car_slice` is `word car+8`, `preferred` `car+0xC`,
/// `car_velocity` `car+0x33C`, `progress` `car+0xAF8`, `now` `[0x606AC4]`. Answers [`StalePlace::Left`]
/// for a car with no block (`+0x3C == 0`, 0x4A068F).
pub fn stale_place(table: &SliceTable, b: &NetBlock, now: i32, car_slice: usize, preferred: Option<usize>, car_velocity: Vec3, progress: f32) -> StalePlace {
    if b.tick == 0 || table.slices.is_empty() {
        return StalePlace::Left;
    }
    let sign = if table.backwards { -1.0f32 } else { 1.0 };
    let raw_dir = |s: usize| table.slices[s.min(table.slices.len() - 1)].forward * sign;
    let age = now.wrapping_sub(b.tick);
    // 0x474440 on the copy of the tracker at `P`
    let sb = table.track_slice(b.position, car_slice.min(table.slices.len() - 1), preferred);
    let dir_b = raw_dir(sb);
    // 0x532910: the dot product, stored as an `f32` (`fstp [ebp-4]`)
    let along = dir_b.dot(b.velocity);
    // the x87 chain runs in extended precision; `f64` for the arithmetic and `f32` where the code stores
    let lap_m = table.lap_len as f64 * 6.0;
    let err = ((table.frac(sb) as f64 + (along as f64 * 0.015625 * age as f64) / lap_m) - progress as f64) * lap_m;
    if err.abs() > PUT_ON_ROAD_METRES {
        // `fmul [0x5B49D4]`, `fstp` to `f32`, `fistp` (round to nearest even)
        let d = ((err * f32::from_bits(0x3E2A_AAAB) as f64) as f32).round_ties_even() as i32;
        let s = table.add_graph(sb, (sign as i32) * d, None);
        return StalePlace::PutOnRoad { forward: raw_dir(s), along };
    }
    if !(-err > 0.0) {
        return StalePlace::Left;
    }
    let e = (-err) as f32; // `fst [ebp+8]`
    let own = raw_dir(car_slice).dot(car_velocity);
    let dv = along - own;
    if dv.abs() < 1.0 && own.abs() < 1.0 && e > 5.0 {
        return StalePlace::PutOnRoad { forward: dir_b, along };
    }
    if dv <= 0.0 || e * 0.5 >= dv {
        let e = if e <= 10.0 { 10.0 } else { e };
        let r = (e * 25.5f32).round_ties_even() as i32;
        return StalePlace::Brake((r & 0xFF) as u8);
    }
    StalePlace::Left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::junctions::Junctions;
    use crate::race::Slice;

    /// 100 slices in a line along +z, 6 m apart, one lap of them.
    fn line() -> SliceTable {
        let slices: Vec<Slice> = (0..100)
            .map(|i| Slice {
                centre: Vec3::new(0.0, 0.0, i as f32 * 6.0),
                up: Vec3::Y,
                forward: Vec3::Z,
                lateral: Vec3::X,
                lane_mask: 0,
                lanes_a: 1,
                lanes_b: 1,
                width_a: 8.0,
                width_b: 8.0,
                lane_width_a: 4.0,
                lane_width_b: 4.0,
            })
            .collect();
        SliceTable { slices, sections: Vec::new(), route: (0..100).collect(), route_pos: (0..100).collect(), lap_len: 100, junctions: Junctions::default(), backwards: false, linear: false }
    }

    fn block(tick: i32, z: f32, vz: f32) -> NetBlock {
        NetBlock { tick, airborne: false, yaw_rate: 0.0, matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], velocity: Vec3::new(0.0, 0.0, vz), position: Vec3::new(0.0, 0.0, z) }
    }

    #[test]
    fn a_car_with_no_block_is_left_alone() {
        let t = line();
        assert_eq!(stale_place(&t, &block(0, 60.0, 20.0), 100, 10, None, Vec3::ZERO, 0.1), StalePlace::Left);
    }

    /// The car is behind the block's progress (`err > 0`): the rule does nothing.
    #[test]
    fn a_car_behind_the_block_is_left_to_the_ai() {
        let t = line();
        // block at slice 30 (frac 0.3, z = 180), the car at slice 10 (progress 0.1)
        assert_eq!(stale_place(&t, &block(100, 180.0, 20.0), 100, 10, None, Vec3::new(0.0, 0.0, 20.0), 0.10), StalePlace::Left);
    }

    /// A car more than 250 m from the block's progress is put on the road, at the slice the error names.
    #[test]
    fn a_car_far_from_the_block_is_put_on_the_road() {
        let t = line();
        // the block stands at slice 50 (300 m), the car's progress is 0.1 (60 m): err = +240 m … below the limit
        assert!(matches!(stale_place(&t, &block(100, 300.0, 0.0), 100, 10, None, Vec3::ZERO, 0.10), StalePlace::Left));
        // progress 0.0: err = +300 m, over 250 — put on the road at `add(50, round(300/6)) = slice 99`
        match stale_place(&t, &block(100, 300.0, 0.0), 100, 0, None, Vec3::ZERO, 0.0) {
            StalePlace::PutOnRoad { forward, along } => {
                assert_eq!(forward, Vec3::Z);
                assert_eq!(along, 0.0);
            }
            other => panic!("{other:?}"),
        }
    }

    /// Ahead of the block by more than the floor with the block slower: the brake rule, throttle 0 and
    /// the byte `round(e' · 25.5)` (255 at the floor of 10 m).
    #[test]
    fn a_car_ahead_of_the_block_brakes() {
        let t = line();
        // block at slice 30 (frac 0.3) standing; the car's progress 0.35 → err = −30 m, e = 30; the car does 20 m/s
        match stale_place(&t, &block(100, 180.0, 0.0), 100, 35, None, Vec3::new(0.0, 0.0, 20.0), 0.35) {
            StalePlace::Brake(b) => assert_eq!(b, ((30.0f32 * 25.5).round_ties_even() as i32 & 0xFF) as u8),
            other => panic!("{other:?}"),
        }
        // e = 6 m: the floor of 10 gives 255
        match stale_place(&t, &block(100, 180.0, 0.0), 100, 31, None, Vec3::new(0.0, 0.0, 20.0), 0.306) {
            StalePlace::Brake(b) => assert_eq!(b, 0xFF),
            other => panic!("{other:?}"),
        }
    }

    /// Both standing and more than 5 m ahead: put on the road at the block's own slice.
    #[test]
    fn two_standing_cars_more_than_five_metres_apart_are_put_on_the_block() {
        let t = line();
        match stale_place(&t, &block(100, 180.0, 0.0), 100, 33, None, Vec3::ZERO, 0.33) {
            StalePlace::PutOnRoad { forward, along } => {
                assert_eq!(forward, Vec3::Z);
                assert_eq!(along, 0.0);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A reversed table: the raw forward is `−forward`, so a block driving backwards (raw −z) has a negative
    /// `along`, and the brake rule and the put-on-road forward row are the raw ones (NQ, `network.md` §11.23).
    #[test]
    fn a_backwards_table_reads_the_raw_rows() {
        let t = line().reversed();
        // block at z = 180 (raw slice 30, reversed frac 1 − 30/100 = 0.70, 0x416290), driving raw −z at 20 m/s, age 0
        let b = block(100, 180.0, -20.0);
        // the car's progress 0.75: err = (0.70 − 0.75) · 600 = −30 m, e = 30; both do −20 raw: dv = 0 → the brake rule
        let s = stale_place(&t, &b, 100, 25, None, Vec3::new(0.0, 0.0, -20.0), 0.75);
        assert_eq!(s, StalePlace::Brake(((30.0f32 * 25.5).round_ties_even() as i32 & 0xFF) as u8));
        // 300 m out: put on the road with the raw forward (+z: against the travel direction) and `along = −20`
        match stale_place(&t, &b, 100, 25, None, Vec3::ZERO, 0.0) {
            StalePlace::PutOnRoad { forward, along } => {
                assert_eq!(forward, Vec3::Z);
                assert_eq!(along, -20.0);
            }
            other => panic!("{other:?}"),
        }
    }

    /// The block's speed carries its progress on by `along · age / 64`: 64 ticks late at 20 m/s is 20 m.
    #[test]
    fn the_blocks_progress_is_carried_on_by_its_age() {
        let t = line();
        // block at z = 180 (slice 30) doing 24 m/s, 64 ticks old: expected progress 0.3 + 24/600 = 0.34; the car is at 0.34 + 0.001
        let s = stale_place(&t, &block(36, 180.0, 24.0), 100, 34, None, Vec3::new(0.0, 0.0, 24.0), 0.341);
        // e = 0.6 m, dv = 0: the brake rule (`dv <= 0`), floor 10 → 255
        assert_eq!(s, StalePlace::Brake(0xFF));
    }
}
