//! Traffic cars — ported from `RE/notes/traffic.md` (§9 placement, §10 despawn, §13 init,
//! §14 motion, §17.6 target speed) plus `0x408D20` read directly for the lateral controller.
//!
//! A traffic car's **drive** is a point mass with a yaw degree of freedom: the AI writes its
//! velocity and yaw rate every drive tick (0x408360 → 0x409720 / 0x409C70 → 0x40A1D0, §27.2; the
//! kinematic `Traffic_driveKinematic` 0x408840 is dead code, §14.5) and the shared 64 Hz
//! integrators `0x49B7A0` / `0x49B8E0` move it. There is no tyre or engine model behind it — but
//! its **ground contact** is every car's: `Car_installHandlers` 0x410D60 leaves `car+0xE20 =
//! Car_ground` 0x49BD40 in place (§14.1), so the four wheel probes, `Susp_update` 0x499A70's
//! pitch / roll and roughness, `BodyHeight` and the resting drop run on the car's own
//! `RigidBody` — [`TrafficSet::bodies`], one `nfs_physics::vehicle::Vehicle` per car, as the
//! cops' (`police.rs`, 2026-09-26), synced in and out of the kinematic record around each use.
//!
//! Ported here:
//! * the purgatory countdown `1 + rand_below(0x5CC4A0[nTraffic])` and its bus multiplier — §9.1;
//! * spawn 60–65 slices (360–390 m) from the reference car on a coin-flipped side, with an
//!   independent coin flip for the car's own direction of travel unless the road is one-way
//!   (`[0x5E7544] = slices[1].lanesA == 0`) — §9.4;
//! * lane choice and lateral target from the slice's lane counts and widths — §9.5;
//! * the pose `slices[s].pos + {lateral, 1.0, 0.0}` in slice-local axes, heading along the slice — §9.6;
//! * target speed 16.5 m/s, or the oncoming ladder keyed on the reference car's speed, scaled by the
//!   per-spawn factor `{1.0, 0.9, 0.8, 0.7}` with the 13.4 / 8.9 / 8.889 floors — §17.6;
//! * the longitudinal rate curve `clamp(0.25 − |v|/56, 0, 0.25)` m/s², integrated with the AI dt — §14.4;
//! * the lateral controller `0x408D20`: below 25 m/s the remaining lane error is applied to the
//!   position outright, above it as a velocity term clamped to the car's own speed;
//! * despawn at 400 m (octagonal metric) from the reference car, and near both ends of a linear
//!   track, then back to purgatory — §10.
//!
//! The lane **scoring** and lane changes of §12 are ported too, in `crate::lanes` — the blockers and
//! caps of §12.4, all six scorers of §23.1–§23.6, the pick of §12.5, the lateral target of §12.6 and
//! the speed cap of §12.7.
//!
//! The reaction to a police car (0x406020 states 2 / 3 with `0x405ED0` and `AIState_Idle` 0x40CAB0 —
//! stop dead, pull over or ignore it) is here, fed [`TrafficSet::cops`] by the caller; the cops
//! themselves are `crate::police` (§17). The reference car `car+0xED4` is a chase target of
//! [`TrafficSet::chase_targets`] — the player, and under `COPS` the opponents too (`traffic.md`
//! §34.5). Audit against the binary: `traffic.md` §34 (2026-09-26). Three things that look like gaps
//! are not: the AI level-of-detail gates of §14.3 / §17.5e **are** modelled ([`ai_pass_due`] for the
//! AI pass, [`drive_due`] for the drive), the
//! personality lane offset `car+0xEC8` is always zero for traffic (it draws personality row 0), and
//! the knocked-over hand-back to the rigid body (`car+0x42C == 1`, 0x4084C3) is dead code for traffic
//! (§21.7) — what replaces it, the two damping exits, is here.
//!
//! The crash reset (2026-09-26): `Car_postImpact` 0x410280 is on every car's Sc32 list at priority
//! 0x32 (0x411305) and, for a car without the smackable bit, runs 0x40A730, 0x40F740 and 0x40FFF0
//! (0x410417…0x410424) — the only gate is `car+0x7D`, which `RigidBody_init` zeroes (0x49B522) and
//! only the world-prop damper 0x4A1030 ever sets. 0x40F740's countdown `car+0x1100` therefore runs for
//! a traffic car ([`TrafficSet::post_impact`]): armed to 0xC0 by `Damage` 0x494500 on an impact past
//! 80 (0x4948E6 — the car-car solver's call 0x496F8E on the traffic car's own impact, and the wall
//! collider's; `Vehicle::crash_response` on the port's traffic body) and by 0x40FBA2 when the car's up
//! is under 0.1 along the slice's (`car+0x370 · car+0x3C4`) or it is off the drivable surface
//! (`car+0x464 == 0`); it loses 1 a tick, 9 off the surface, an upright (`car+0x3A0 > 0.75`) and
//! alive (`car+0xDB0`) car clears it, and at zero the car is reset in place — `0x40F540(car, 0, 0)`
//! ([`TrafficSet::reset_to_slice`]). `car+0xDB0` is 1 for a traffic car too — `Car_raceInit` 0x410730
//! runs for every car in `0x5E8B34` (0x4110D3…0x4110EB) and stores it at 0x410BD6 — and only an impact
//! past 120 (0x4948C1) or a body-ground contact that leaves the car upside down (0x49C01B) clear it;
//! no spawn path re-arms it (writers: 0x410BD6, 0x4948C1, 0x49C01B, 0x49DD34, 0x49E3E5), so a killed
//! traffic car stays dead through purgatory until a reset's 0x49E220. The purgatory clears the
//! countdown on its exit (0x40DFBA) and every update (0x40E145). The 0x408750 net at the end of the
//! drive (0x4086E6) resets in place the same way, `0x40F540(car, 1, 0)` — the port used to send that
//! car back to purgatory.
//!
//! *(Until 2026-09-26 the traffic was kinematic — never rolled, its up followed the ground normal,
//! and a reset stood it on the slice plane for the next ground snap to seat. With
//! [`TrafficSet::bodies`] the car-car impulse's spin, `Susp_update`'s pitch / roll and 0x49B090's
//! placement — two `Susp_update`s with the `BodyHeight` drop between — are the body's own,
//! `nfs_physics::vehicle::Vehicle::place_at_slice`.)*

use crate::aidrive::{self, AiCarState};
use crate::scripts::{ScriptRunner, Trigger};
use crate::lanes::{lane_of, lane_span, LaneCar, LaneScratch, LaneSelf, PERSONALITIES};
use crate::race::SliceTable;
use glam::{Mat3, Vec3};

/// Same-direction cruise speed (`[0x5B2660]`).
pub const BASE_SPEED: f32 = 16.5;
/// Maximum simultaneously active traffic cars (`[0x5CC480]`, §11).
pub const MAX_ACTIVE: usize = 5;
/// Despawn distance (`[0x5B25C4]`), in the octagonal metric.
pub const DESPAWN_DISTANCE: f32 = 400.0;
/// Spawn distance in slices (`0x3C + rand_below(6)`, §9.4).
pub const SPAWN_GAP: (u32, u32) = (60, 6);
/// AI timestep — `[0x5CC750]`, the 32 Hz rate traffic runs at inside ~160 m (§14.3).
pub const AI_DT: f32 = 1.0 / 32.0;

/// `Lod_shouldUpdate` 0x414B10 as the drive `0x408360` calls it (0x408373…0x40839E) —
/// `0x414B10(1, 1, car+0x78, car+0, &[0x5E6D00], &[0x5E6D78], &[0x5E6D7C], car+0x10B8)`, read to
/// the instruction 2026-09-25 (`traffic.md` §14.3, corrected). Row 1 of the byte table `0x5CC770` is
/// `{6 ×10, 5 ×10}`; `lod = min(_ftol(dist) / 16, 19)` (0x5A0F98 truncates, the `cdq / and 0xF / sar 4`
/// divides towards zero; 0 when `force`), so `k = 6` — 32 Hz, `nTicks = 0x5CC71C[6] = 1`,
/// `dt = 0x5CC738[6] = 1/32`, mask 0 — while `car+0x78 < 160` and `k = 5` — 16 Hz, `nTicks = 2`,
/// `dt = 1/16`, mask 1 — beyond. Due when `(car+0 + gGameTick/2) & mask == mask` (0x414B83…0x414B9D),
/// `car+0` being `slot | 0x100` (`RigidBody_init` 0x49B510); in the first 15 game ticks it always
/// passes with `nTicks = [0x5CC734] = 1`, `dt = [0x5CC750] = 1/32` (0x414BA1). Returns
/// `([0x5E6D7C], [0x5E6D78])`: the AI dt and the **32 Hz** ticks it covers. `distance` is `car+0x78`
/// (0x417190: the octagonal x–z distance to the nearest racer, from 1000), `force` `car+0x10B8`
/// (`flags & 1`, a racer — never a traffic car).
pub fn drive_due(distance: f32, force: bool, tick: u64, slot: usize) -> Option<(f32, i32)> {
    if tick <= 15 {
        return Some((AI_DT, 1));
    }
    let lod = if force { 0 } else { (distance as i32 / 16).min(19) };
    let (mask, dt, n_ticks) = if lod < 10 { (0u64, AI_DT, 1) } else { (1, 1.0 / 16.0, 2) };
    ((((slot | 0x100) as u64 + tick / 2) & mask) == mask).then_some((dt, n_ticks))
}
/// Purgatory delay table `0x5CC4A0`, indexed by the number of registered traffic cars (§9.1).
pub const RESPAWN_TABLE: [u32; 12] = [25, 50, 75, 100, 200, 400, 500, 800, 900, 1000, 1100, 1200];
/// The AI does not count the purgatory delay down before this tick: 0x40E140 steps it while
/// `gGameTick ≥ 0x3C0` (0x40E160).
pub const SPAWN_GATE_TICK: u64 = 0x3C0;

/// `Lod_shouldUpdate` 0x414B10 as the AI pass `0x406720` calls it (0x406745…0x406770) for an `AIHigh`
/// whose `+0x10` is 0 — the human's, a cop's and a traffic car's; only `AIHigh_Opponent`'s ctor sets
/// it (0x40593D). `0x414B10(1, 0, car+0x78, car+0, …, car+0x10B8)`: row 0 of `0x5CC770` is `{4 ×15,
/// 3 ×5}`, `lod = min(_ftol(dist) / 16, 19)` (0 when `force`), so `k = 4` — **8 Hz**, `nTicks = 4`,
/// mask 3 — inside 240 m and `k = 3` — 4 Hz, `nTicks = 8`, mask 7 — beyond; the first 15 game ticks
/// always pass with `nTicks = 1`. Due when `((car+0) + gGameTick/2) & mask == mask`, `car+0` being
/// `slot | 0x100` (`RigidBody_init` 0x49B510). Returns `[0x5E6654]`, the 32 Hz ticks the update covers
/// (`traffic.md` §17.5e). `distance` is `car+0x78` (0x417190: the octagonal x–z distance to the
/// nearest racer, from 1000; 0 for a racer), `force` is `car+0x10B8` (`flags & 1`, a racer).
pub fn ai_pass_due(distance: f32, force: bool, tick: u64, slot: usize) -> Option<i32> {
    if tick <= 15 {
        return Some(1);
    }
    let lod = if force { 0 } else { (distance as i32 / 16).min(19) };
    let (mask, n_ticks) = if lod < 15 { (3u64, 4) } else { (7, 8) };
    ((((slot | 0x100) as u64 + tick / 2) & mask) == mask).then_some(n_ticks)
}

/// Octagonal distance `max(|dx|, |dz|) + 0.25·min(|dx|, |dz|)` — the original's XZ distance
/// approximation, used for both the despawn test and `car+0xEC4` (0x40EEE5).
pub fn oct_distance(a: Vec3, b: Vec3) -> f32 {
    oct_norm(a.x - b.x, a.z - b.z)
}

fn oct_norm(x: f32, z: f32) -> f32 {
    let (x, z) = (x.abs(), z.abs());
    x.max(z) + 0.25 * x.min(z)
}

/// Target speed of a traffic car (`0x40AF70`, branch 0x40B199, §17.6). `reference_speed` is the
/// speed of the car this one is paired with; `race_dir` is −1 when the race runs backwards.
/// The result carries the sign of `dir`.
pub fn target_speed(dir: f32, race_dir: f32, reference_speed: f32, speed_factor: f32) -> f32 {
    let mut v = BASE_SPEED;
    if dir != race_dir {
        let s = reference_speed.abs();
        v = if s < 13.3333 {
            16.5
        } else if s < 26.6667 {
            13.3333
        } else if s < 40.0 {
            8.8889
        } else if s < 55.5556 {
            4.4444
        } else if s < 71.1111 {
            2.2222
        } else {
            2.77
        };
    }
    // x = max(max(v·factor, v − 13.4), 8.9), then max(x·1.0, 8.88889)
    let x = (v * speed_factor).max(v - 13.4).max(8.9);
    x.max(8.88889) * dir
}

/// Longitudinal rate `0x4081B0(car, v)`. The speed-dependent curve `0.25 − |v|/56`, clamped to
/// `0..0.25`, is applied **only to traffic** (`car+0x52C & 0x10`, tested at 0x4081FC); every other AI
/// car — a cop above all — starts from a flat 1.0. **This function is only the base and the
/// `car+0x1098` pursuit boost**; the rest of 0x4081B0's chain — `× car+0x10DC` (a compile-time 1.0),
/// `× 0.9` under `WEATHER` (zeroed by the race load — [`TrafficSet::weather`]), `× car+0x1028` for a cop (also a compile-time 1.0), `× 1.5` while the
/// HARD_BRAKE script action is up, and `× 0x40AEF0`'s early-race boost — is applied by each caller,
/// because what each of them has to hand differs.
/// (Read 2026-09-17 while porting the chase; `ai.md` §5.2.3 quotes only the traffic column.)
pub fn accel_rate_for(is_traffic: bool, speed: f32, boost: f32) -> f32 {
    let base = if is_traffic { (0.25 - speed.abs() / 56.0).clamp(0.0, 0.25) } else { 1.0 };
    base * boost
}

/// The traffic column of `accel_rate_for`.
pub fn accel_rate(speed: f32) -> f32 {
    accel_rate_for(true, speed, 1.0)
}

/// `0x40AEF0` — the early-race boost `0x4081B0` multiplies a Normal-state car's accel rate by
/// (`ai.md` §5.2 rule 3, CONFIRMED from the code): more than one opponent, the game mode is not 1,
/// the first 30 s (`gGameTick < 0x780`), exactly one human, **and the human is behind this car**
/// (`human+0xAF8 < car+0xAF8`). Then `1 + (opponents − car+0x10D0)·0.2`, where `car+0x10D0` is the
/// car's ordinal among the flag-8 cars — 0 for traffic and cops, which never enter that loop
/// (0x40B3EF), so they get the full `1 + opponents·0.2`.
pub fn early_boost(opponents: u32, tick: u64, humans: u32, human_progress: f32, car_progress: f32, ordinal: u32) -> f32 {
    if opponents <= 1 || tick >= 0x780 || humans != 1 || !(human_progress < car_progress) {
        return 1.0;
    }
    1.0 + (opponents as f32 - ordinal as f32) * 0.2
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reference {
    pub position: Vec3,
    pub slice: usize,
    /// laps the reference car (the player) has completed: its `car+0xAF8` is `+0x530 + frac −
    /// +0x534` (0x416310), which 0x40AEF0's early-race boost and 0x401580's script triggers compare
    /// with a traffic car's bare lap fraction
    pub laps: u32,
    /// `ref+0xEC0`
    pub speed: f32,
    /// `ref+0xEB0`
    pub dir: f32,
    /// `human+0xD86` — the player's horn button, bit 1 of `[0x5E9664]` copied by 0x4991E0
    /// (`physics.md` §8); 0x401580 reads it for the `human honk` script trigger 6 (0x401724).
    pub honking: bool,
}

/// What `0x405ED0` reads of one police car (`0x5E8AC4[0 … [0x5E8ABC])`, registration order).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CopView {
    /// `car+8`
    pub slice: usize,
    /// `car+0xEC0`
    pub speed: f32,
    /// `!(car+0xECC & 4)` — not in purgatory (the purgatory ctor sets the bit, 0x40DF3F; its exit
    /// clears it, 0x40DFDA)
    pub in_world: bool,
    /// `AIHigh_Cop+0x58 != 0` — the cop has claimed a perp
    pub has_target: bool,
}

/// `AIHigh_Traffic` state 3's `AIState_Idle` (vtable 0x5B252C, ctor 0x40B510 + `+0xC = 1`; update
/// 0x40CAB0) — what a traffic car does while a cop is near (`traffic.md` §11, §34.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopMode {
    /// `+0xC = 1`: stop dead where it is (`car+0xEB8 = 0`, `car+0xEB4 = car+0xED0`)
    Dead,
    /// `+0xC = 0`, `+8` = the lateral to pull over to (`0x40CB40`), in the port's frame
    PullOver(f32),
}

/// One chase target, `0x5E8B10[0 … [0x5E8AB8])` — the cars with `car+0x52C & 0x100`: the human,
/// and the opponents only under `COPS [0x657414]` (`Car_register` 0x40F336, §8). A traffic car's
/// reference `car+0xED4` is always one of them (`traffic.md` §34.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChaseTarget {
    /// `car+0x330`
    pub position: Vec3,
    /// `car+8`
    pub slice: usize,
    /// `car+0xEC0`, signed along the route
    pub speed: f32,
    /// `car+0xEB0`
    pub dir: f32,
    /// `car+0x528` — its place in the race order `0x5E8B88[]` as the previous even tick's
    /// `0x417710` left it ([`crate::race_order::RaceOrder`]; the caller writes it)
    pub place: usize,
}

impl From<Reference> for ChaseTarget {
    /// The human as a chase target; its order key is only compared with other chase targets'.
    fn from(r: Reference) -> Self {
        Self { position: r.position, slice: r.slice, speed: r.speed, dir: r.dir, place: 0 }
    }
}

/// What the caller knows about one traffic car before it is placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficSpec {
    /// index into the caller's model list
    pub model: usize,
    /// `car+0x50C` is `SBUS`/`TBUS` — a bus arms its purgatory delay five times longer (§9.1)
    pub bus: bool,
    /// `car+0x3A8/0x3AC/0x3B0`, half the model's body bounding box (`traffic.md` §21.4)
    pub half_extents: Vec3,
}

impl TrafficSpec {
    /// `RigidBody_init` 0x49B5D3 overrides the sim's mass for traffic with `9 · hx·hy·hz` in units of
    /// 128 kg — a density of 1152 kg per cubic metre of full box (`traffic.md` §21.3).
    pub fn mass(&self) -> f32 {
        9.0 * self.half_extents.x * self.half_extents.y * self.half_extents.z * 128.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrafficCar {
    /// index into the caller's model list
    pub model: usize,
    /// `car+0x08`
    pub slice: usize,
    /// `car+0x528` — this car's place in `0x5E8B88[]` as the previous even tick's `0x417710` left
    /// it ([`crate::race_order::RaceOrder`]); the caller writes it before [`TrafficSet::ai_on`]
    pub place: usize,
    /// The game tick of the last oncoming honk this car drew (0x401237's `HonkPattern(car, 2,
    /// 0x20, 8)` on the even tick's AI pass); the audio update of the odd tick that follows programs
    /// the slot's honk timer from it (`nfs_audio::ai_engine::HonkTimer`)
    pub honk_tick: Option<u64>,
    /// `car+0xAF8` — for a traffic car the **bare lap fraction** of its slice, with no lap count:
    /// 0x4169B0 (Sc64 priority 0x19, even ticks) writes `frac(car+8)` for a car without the racer
    /// bit (0x416A21…0x416AA1: the section interpolation of 0x481B70, `1 − f` under `REVERSE` and a
    /// circuit's `1.0` stored as 0), and 0 while `car+0x7F == 0` (0x416AAC). 0x406020 zeroes the
    /// traffic car's `+0x530` / `+0x534` on every update, so nothing counts laps for it. Readers:
    /// `0x40E9E0` (metres = `nSlicesLap · +0xAF8 · 6`, the §12.9 800 m gate and triggers, §23.2's
    /// 600 m `late`), 0x40AEF0 (the early-race boost) and 0x417710 (the race-order sort)
    pub progress: f32,
    /// `car+0xEB0`: +1 along the route, −1 against it (oncoming)
    pub dir: f32,
    /// `car+0x1074`: lateral target from the centreline, metres
    pub lateral_target: f32,
    /// `car+0xED0`: the lateral offset the car actually has
    pub lateral: f32,
    /// `car+0xEF0`: lateral rate
    pub lateral_rate: f32,
    /// `car+0x101C` — the "the lane layout changed, keep scanning" window (§23.4)
    scan: i32,
    /// `car+0xE3C` — the `scripts.bin` runner (§30)
    pub script: ScriptRunner,
    /// the tick a human last hit this car, for the collision trigger
    pub hit_by_human_tick: u64,
    /// `car+0x418` — collisions since this car was placed. `0x406CD0` clears it on every (re)spawn,
    /// and nothing else clears it for a traffic car, which is what disables the "on its roof" exit
    /// for the rest of a car's life once it has been hit (§21.7).
    pub collisions: u32,
    /// `car+0x104C` and its timers — the gear the drive model runs in (`aidrive::Gear`)
    pub gear: aidrive::Gear,
    /// `car+0xD80`, the brake-light counter (`aidrive::brake_light`)
    pub brake_light: u8,
    /// `car+0xEC0`: speed along `dir`
    pub speed: f32,
    /// `car+0xEB8`
    pub target_speed: f32,
    /// `car+0xEEC`: per-spawn speed factor
    pub speed_factor: f32,
    /// `car+0xEE8`: purgatory countdown, in 32 Hz ticks (it steps by the AI pass's `[0x5E6654]`)
    pub respawn_ticks: i32,
    /// `car+0x7F`: out of purgatory and being drawn
    pub active: bool,
    /// `car+0x78` — the octagonal x–z distance to the nearest racer, which 0x417190 writes from the
    /// tracker 0x49AF70 (Sc32 priority 6) only while the car is in the world (0x49AF75 bails on
    /// `car+0x7F == 0`); the car record starts zeroed. [`TrafficSet::track_racers`].
    pub racer_distance: f32,
    /// a bus respawns five times less often (`car+0x50C` = "SBUS" / "TBUS", §9.1)
    pub bus: bool,
    /// `car+0x1100` — 0x40F740's crash countdown (module docs), in 32 Hz ticks
    pub reset_countdown: i32,
    /// `AIHigh_Traffic` state 3 — stopped for a cop (`Some`) — against state 2, driving (`None`)
    pub stopped: Option<StopMode>,
    /// `car+0xED4`, the reference car, as an index into [`TrafficSet::chase_targets`] (0 = the
    /// human, which the purgatory ctor 0x40DE73 stores). Rebound by the purgatory's exit (0x40E00F…,
    /// the leading chase target), the spawn (0x4072D4, 0x407472) and the retire test (0x4071BF).
    pub reference: usize,
    /// `AIHigh_Traffic+0x18`: the car rolled "ignore the cop" (0x406327) and keeps driving until no
    /// cop is near (0x40633D clears it). Never reset by a respawn — it lives on the `AIHigh`.
    pub ignore_cops: bool,
    /// `car+0xDB0 != 0` — the car is alive: 1 from `Car_raceInit` (0x410BD6), 0 after an impact past
    /// 120 (`Damage` 0x4948C1), re-armed only by a reset's 0x49E220 (0x49E3E5)
    pub alive: bool,
    /// `car+0x464 == 0` — no drivable patch under `car+0x330` (the body tracker's word, 0x49AFD8),
    /// which [`TrafficSet::integrate_on`] answers from the collision grid
    pub off_surface: bool,
    /// `car+0x3A0` — the up axis along the ground normal the car is seated on
    pub cos_tilt: f32,
    /// `car+0x3C4`, the body tracker's patch normal (`Tracker::normal` 0x474420, `race-logic.md`
    /// §6) — what 0x40FB6D dots the car's up with. `None` until a body has tracked a patch; the
    /// arm then uses the slice's up (the unit tests' fallback).
    pub ground_normal: Option<Vec3>,
    /// how many times 0x40F540 put this car back on its slice (diagnostic)
    pub resets: u32,
    /// `car+0xD4C…+0xD54`, the body-frame force the AI controller wrote (0x409720 / 0x409C70:
    /// lateral, 0, forward) — `Susp_update`'s dive / roll filters read it; the drive's damped
    /// exits zero it (0x4084CE…, 0x4085FF…)
    pub accel_local: Vec3,
    /// `car+0xD40…+0xD48`, the world acceleration 0x40A1D0 derives from it (0x40A236 / 0x40A27B)
    /// — `Susp_update`'s lateral term (0x49A163); the damped exits leave it alone
    pub accel_world: Vec3,
    /// collision box half-extents and the mass derived from them (§21.3, §21.4): half the LOD-1
    /// model box of 0x445090 (`[this+0xB0][0]` for `car+0x52C & 0x10`, 0x4450CC), which holds the
    /// model origin (`nfs_game::world::car_model_box`)
    pub half_extents: Vec3,
    pub mass: f32,
    /// `car+0x330`, the body origin: `half_extents.y` (`car+0x3AC`) above the model's ground
    /// plane, the model drawn back down at [`Self::draw_origin`]
    pub position: Vec3,
    pub orientation: Mat3,
    pub velocity: Vec3,
    /// turns per second
    pub angular_velocity: Vec3,
}

impl TrafficCar {
    pub fn new(spec: TrafficSpec) -> Self {
        Self {
            gear: aidrive::Gear::default(),
            brake_light: 0,
            model: spec.model,
            half_extents: spec.half_extents,
            mass: spec.mass(),
            slice: 0,
            place: 0,
            honk_tick: None,
            progress: 0.0,
            dir: 1.0,
            lateral_target: 0.0,
            lateral: 0.0,
            lateral_rate: 0.0,
            scan: 0,
            // a traffic car's personality is row 0 after the `% 5` of §23.10, i.e. Nemesis
            script: ScriptRunner::new(0),
            hit_by_human_tick: 0,
            collisions: 0,
            speed: 0.0,
            target_speed: 0.0,
            speed_factor: 1.0,
            respawn_ticks: 1,
            active: false,
            racer_distance: 0.0,
            bus: spec.bus,
            reset_countdown: 0,
            stopped: None,
            ignore_cops: false,
            reference: 0,
            alive: true,
            off_surface: false,
            cos_tilt: 1.0,
            ground_normal: None,
            resets: 0,
            accel_local: Vec3::ZERO,
            accel_world: Vec3::ZERO,
            position: Vec3::ZERO,
            orientation: Mat3::IDENTITY,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
        }
    }

    /// Where the model is drawn, as for every `R3DCar`: `car+0x330 − M[1]·car+0x3AC` (slot 5
    /// 0x43A410, the draw 0x443490 at 0x4434D7) — the traffic model's box being LOD 1's.
    pub fn draw_origin(&self) -> Vec3 {
        self.position - self.orientation.row(1) * self.half_extents.y
    }

    /// The origin's height above the ground plane at rest, **along the plane normal**: the `BndP`
    /// wheel quad (model `y` = −[`RIDE_HEIGHT`]) lowered by `car+0x3AC` (0x4421BE) is what
    /// `BodyHeight` 0x49A900 measures and `Car_ground`'s resting branch drops onto the plane
    /// (`pos.y −= car+0x424`, along world `y`, `suspension-collision.md` §4.2) — so on a plane of
    /// normal `n` the origin settles `rest_height / n.y` above the plane point under it.
    pub fn rest_height(&self) -> f32 {
        RIDE_HEIGHT + self.half_extents.y
    }

    /// How the drive model of §27.2 sees this car.
    pub fn as_ai_state(&self, phys: &TrafficPhysics) -> AiCarState {
        AiCarState {
            position: self.position,
            orientation: self.orientation,
            velocity: self.velocity,
            angular_velocity: self.angular_velocity,
            slice: self.slice,
            dir: self.dir,
            // `car+0xEAC`, which `AI_resetState` 0x406D50 sets equal to `car+0xEB0` and only a
            // behaviour asking for a U-turn moves. No traffic behaviour in this port does, so it
            // stays equal and 0x409AAF's gate is always open — as it is in the original for a
            // traffic car that has not been told to turn round.
            requested_dir: self.dir,
            // `car+0x104C`, the gear the oscillator of §27.2a leaves it in
            sense: self.gear.sense,
            speed: self.speed,
            speed_est: self.speed_estimate(),
            target_speed: self.target_speed,
            lateral: self.lateral,
            lateral_target: self.lateral_target,
            aim_point: Vec3::ZERO,
            max_yaw_acc: phys.max_yaw_acc,
            max_lat_acc: phys.max_lat_acc,
        }
    }

    /// `car+0xEC4` — the octagonal magnitude of the velocity (0x40EEE5).
    pub fn speed_estimate(&self) -> f32 {
        oct_norm(self.velocity.x, self.velocity.z)
    }

    /// How the lane scorers see this car.
    pub fn as_lane_car(&self, table: &SliceTable) -> LaneCar {
        let sl = &table.slices[self.slice];
        LaneCar {
            slice: self.slice,
            lane: lane_of(sl, self.lateral),
            lane_mask: lane_span(sl, self.lateral, self.half_extents.x),
            speed: self.speed,
            speed_est: self.speed_estimate(),
            half_len: self.half_extents.z,
            lateral: self.lateral,
            // a traffic car is always square to the slice, so its road-projected half width
            // (`car+0x538`, 0x417680) is just its body half width
            half_span: self.half_extents.x,
            is_human: false,
            is_racer: false,
        }
    }
}

/// The original's random number generator (`Rand_seed` 0x4A4780, drawn inline at e.g. 0x4010B5):
/// a 16-bit multiplicative LCG `state = (state · 123456789) mod 2^16` whose draw is bits 8..23 of
/// the full 32-bit product. `[0x5D102C]` is the multiplier, `[0x5D1028]` the state; seeding warms it
/// up `seed mod 500` times from 0x3ADE68B1.
#[derive(Debug, Clone)]
pub struct Lcg(pub u32);

impl Lcg {
    pub const MUL: u32 = 123_456_789;

    pub fn new(seed: u32) -> Self {
        let mut state: u32 = 0x3ADE_68B1;
        for _ in 0..(seed % 500) {
            state = state.wrapping_mul(Self::MUL) & 0xFFFF;
        }
        Self(state)
    }

    /// One draw: `(state·M >> 8) & 0xFFFF`, with the state kept to 16 bits.
    pub fn rand16(&mut self) -> u32 {
        let prod = self.0.wrapping_mul(Self::MUL);
        self.0 = prod & 0xFFFF;
        (prod >> 8) & 0xFFFF
    }

    /// `(rand16 · n) >> 16` — the original's "random below n".
    pub fn below(&mut self, n: u32) -> u32 {
        (self.rand16() * n) >> 16
    }
}

/// The per-car constants the drive model needs that come from the car's `.sim` (`default.sim` for a
/// traffic car, §21.1): `car+0x1084 = grip · 9.8`, the brake force behind `car+0x1014`, and the yaw
/// clamp `car+0x1088`, which is the immediate 2.6 written by 0x406F80.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficPhysics {
    pub brake_force: f32,
    pub max_lat_acc: f32,
    pub max_yaw_acc: f32,
}

impl Default for TrafficPhysics {
    fn default() -> Self {
        Self { brake_force: 8.0, max_lat_acc: 9.8, max_yaw_acc: 2.6 }
    }
}

/// Where the body's origin sits above the surface under it while a traffic car rests on the
/// ground: the model's lowest `BndP` points, at `y = −0.001` on every traffic and police model
/// (−0.002 on `eas4`, `eas10`; `nfs-game/examples/bndp.rs`), laid on the plane. The port used 1 m,
/// a leftover of the slice height, and every traffic and police car floated a metre up.
/// *(Corrected 2026-09-25:)* this said a traffic car is `car+0x52C & 0x80` and so takes `Susp_AI`
/// 0x4998C0, whose `BodyGroundCollide` 0x496610 is what lays those points on the plane. `0x80` is
/// the smackables' flag; the traffic's is 0x10 (`ai.md` §6), so a traffic car goes through
/// `Susp_update` 0x499A70 like any other (`suspension-collision.md` §4.2). That path rests the car
/// at the same height: upright, `BodyHeight` 0x49A900 puts `+0x424` at `½(mean + min)` of the four
/// lowest body points — the `BndP` wheel quad, sorted first by 0x49ABC0 (`car-model.md` §12) — and
/// `Car_ground` 0x49BD40's resting branch drops the body by it (`pos.y −= +0x424`), so the quad sits
/// on the plane. *(2026-09-26: with [`TrafficSet::bodies`] the car runs that `Car_ground` itself —
/// the per-wheel contacts, `Susp_update`'s pitch / roll and roughness included; this constant is
/// the ride height the body is built with and the no-body fallback's rest.)*
///
/// *(2026-09-25:)* this is the height of the **model's** ground plane; the body origin
/// `car+0x330` (`TrafficCar::position`) sits `car+0x3AC` — half the LOD-1 box, `half_extents.y` —
/// higher still, because the `BndP` loader lowered every point by it (0x4421BE) — see
/// [`TrafficCar::rest_height`].
pub const RIDE_HEIGHT: f32 = 0.001;

#[derive(Debug, Clone)]
pub struct TrafficSet {
    /// `[0x5E8BF8]` — how many AI opponents are in the race, for the §5.2 rule 3 boost.
    pub opponents: u32,
    /// `RACE_TYPE == 1`, the cops race of the Fuzzyfuz code: 0x40AEF0's early-race boost is skipped
    /// (`modes.md` §3).
    pub cops_race: bool,
    /// `WEATHER` (`[0x657A24] > 0`): the longitudinal rate is scaled by 0.9 (0x40825C…0x40826E,
    /// `ai.md` §5.2.3). `[0x657A24]` is the option registry's **`WEATHER`** (id 0x1E, record
    /// 0x5D1F90) — *not* the per-track night flag `word[0x628A28]`; the port used to drive it from
    /// the night flag and slowed traffic on every night track.
    ///
    /// **Always false in a race, whatever `fe.txt` says — CONFIRMED (2026-09-26, `traffic.md`
    /// §33.4).** The race load 0x4B67B0 calls 0x414DB0 (0x4B685F), which stores 0 into
    /// `[0x657A24]` (0x414DF9) after the track load 0x468FB0's one read (0x469047, the spec-file
    /// index) and before every in-race reader: this rate, the personalities' `wipeOutFreq` index
    /// (0x407E52, loaded later by the same 0x414DB0 → 0x4068F0 → 0x407EF0) and the audio preload
    /// 0x4A88A0 (0x4A89A8). The only other writers are 0x475C80's (0x475D4D / 0x475A6D, both inside
    /// the track load). So the registry's `weather` is not read here; the field stays for the tests.
    pub weather: bool,
    pub cars: Vec<TrafficCar>,
    /// `scripts.bin`, shared by every car
    pub scripts: nfs_formats::scripts::Scripts,
    /// `trafcfg.dat`'s two gain chains
    pub cfg: nfs_formats::trafcfg::TrafficConfig,
    pub phys: TrafficPhysics,
    pub rng: Lcg,
    /// `[0x5E7544]`: `slices[1].lanesA == 0` in the track's own frame (0x40E670) — a one-way road
    /// carries no oncoming traffic. [`crate::police::one_way_road`] reads it: on the port's reversed
    /// table of a backwards race that lanesA is `lanes_b`.
    pub one_way: bool,
    /// −1 when the race runs backwards (`REVERSE [0x657430]`).
    pub race_dir: f32,
    /// `LINEAR_TRACK [0x657494]` — point-to-point, so slice arithmetic clamps and the ends despawn.
    pub linear: bool,
    /// the racer slot of car 0: traffic comes after the humans, the opponents and the cops
    /// (0x4ECF00, `traffic.md` §17.5d) — the phase of the AI pass's gate, [`ai_pass_due`]
    pub slot_base: usize,
    /// the spawn gate's no-spawn slice windows for this track ([`no_spawn_windows`], 0x40E0AD)
    pub no_spawn: &'static [(usize, usize)],
    /// `[0x657A30] != 0`, the option registry's **`TIME`** (id 0x21, `modes.md`) — the time of
    /// day, the night word of the track spec record (`traffic.md` §33.3): the `human honk`
    /// trigger 6 is only raised while it is 0 (0x40172E). Its only other readers pick and build
    /// that spec record (0x469041, 0x475A82), whose file `Tr00.bin` is not shipped, so the honk
    /// gate is its one effect on a race. `fe.txt`'s `time` (shipped 0), set by `porsche`.
    pub time_mode: bool,
    /// One `RigidBody` per car — the car record `Car_ground` 0x49BD40 (`car+0xE20`), the
    /// placement 0x49B090 and the collision pass 0x498FF0 run on; empty until
    /// [`Self::set_bodies`] (the unit tests keep none and fall back to the slice's height).
    pub bodies: Vec<nfs_physics::vehicle::Vehicle>,
    /// `default.sim` — what `Car_loadSim` 0x49C6C0 falls back to for a car with no sim name
    /// (§21.1): the suspension constants `Car_ground` reads (`specs[0x114…0x138]`).
    pub specs: Option<nfs_physics::specs::CarSpecs>,
    /// The police cars as `0x405ED0` sees them this AI pass (the caller fills it before
    /// [`Self::ai_on`]; the cops' own AI runs earlier in the same even tick, §17.5e). Empty — no
    /// `COPS` — and no traffic car ever stops.
    pub cops: Vec<CopView>,
    /// `0x5E8B10[]` this AI pass, in registration order: the human first, then — under `COPS` only
    /// — the opponents. Empty (the unit tests): the `Reference` passed to [`Self::ai_on`] is the one.
    pub chase_targets: Vec<ChaseTarget>,
    /// `car+0x330` of each car the caller passes as `others` to [`Self::ai_on`], in the same order —
    /// for `0x40E860`'s fine step inside ±12 m (`lanes::gap_fine`). Shorter than `others` (the unit
    /// tests): the missing ones use the slice gap alone.
    pub other_positions: Vec<Vec3>,
    /// `car+0x528` of each car the caller passes as `others`, in the same order — the place
    /// `0x5E8B88[]` gave it at the end of the previous even tick ([`crate::race_order::RaceOrder`]).
    /// Shorter than `others` (the unit tests): the lane pass sorts the cars itself instead.
    pub other_places: Vec<usize>,
    /// `0x5E8BAC[]`, the second race-order array, as indices into the key list [`Self::overtakes`]
    /// takes (registration order: the human, the opponents, the cops, the traffic)
    pub overtake_order: Vec<usize>,
    /// `car+0x550` per car of that list — the car it last swapped places with
    pub swap_partner: Vec<Option<usize>>,
    /// `car+0x554` per car — the game tick of its last swap
    pub swap_tick: Vec<Option<u64>>,
    /// where this set's car 0 sits in that list
    pub overtake_base: usize,
}

/// The spawn gate `0x40E060`'s track windows (0x40E0AD…0x40E134): `TRACK_NAME [0x657444]`'s first
/// four bytes, `| 0x20202020`, compared with `"cany"` (0x796E6163) and `"indu"` (0x75646E69); on
/// those tracks no traffic car is placed while the reference car's slice (`word [car+0xED4] + 8`)
/// lies in one of these inclusive ranges — `cmp / jl / cmp / jle` on the signed slice. Every other
/// track has none. (The windows sit on the tracks' tunnels, `traffic.md` §3 — HIGH.)
pub fn no_spawn_windows(track: &str) -> &'static [(usize, usize)] {
    const CANYON: [(usize, usize); 2] = [(0x6B1, 0x6EA), (0x732, 0x756)];
    const INDUSTRIAL: [(usize, usize); 4] = [(0x676, 0x725), (0x8DF, 0x902), (0x938, 0x96B), (0x99D, 0x9CE)];
    // the dword read takes whatever follows a shorter name, the string's NUL included; `| 0x20`
    // turns a NUL into a space, which matches neither key
    let mut key = [0u8; 4];
    for (k, b) in key.iter_mut().zip(track.bytes()) {
        *k = b;
    }
    match key.map(|b| b | 0x20) {
        [b'c', b'a', b'n', b'y'] => &CANYON,
        [b'i', b'n', b'd', b'u'] => &INDUSTRIAL,
        _ => &[],
    }
}

impl TrafficSet {
    /// `models` = one entry per traffic car the race was set up with.
    pub fn new(models: &[TrafficSpec], table: &SliceTable, race_dir: f32, linear: bool, seed: u32) -> Self {
        // `[0x5E7544]` (0x40E670): the original's slice 1 lanesA, the same flag in either direction.
        // *(Until 2026-09-25 this read `lanes_a` off the reversed table — the original's lanesB in a
        // backwards race — the bug `police::one_way_road` fixed for the cops, bd6aa8d. No shipped
        // track has `lanesA == 0` on slice 1, so nothing observable changes with that data.)*
        let one_way = crate::police::one_way_road(table);
        let mut rng = Lcg::new(seed);
        let delay = RESPAWN_TABLE[models.len().min(RESPAWN_TABLE.len() - 1)];
        let cars = models
            .iter()
            .map(|&spec| {
                let mut c = TrafficCar::new(spec);
                c.respawn_ticks = (1 + rng.below(delay) as i32) * if spec.bus { 5 } else { 1 };
                c
            })
            .collect();
        Self { cars, opponents: 0, cops_race: false, weather: false, scripts: nfs_formats::scripts::Scripts::default(), rng, one_way, race_dir, linear, cfg: nfs_formats::trafcfg::TrafficConfig::default(), phys: TrafficPhysics::default(), slot_base: 0, no_spawn: &[], time_mode: false, bodies: Vec::new(), specs: None, cops: Vec::new(), chase_targets: Vec::new(), other_positions: Vec::new(), other_places: Vec::new(), overtake_order: Vec::new(), swap_partner: Vec::new(), swap_tick: Vec::new(), overtake_base: 0 }
    }

    /// The cars' bodies: `RigidBody_init` 0x49B510 for each traffic car, with `default.sim`'s
    /// constants, the model's `BndP` points less `car+0x3AC` (`points[model]`,
    /// `nfs_game::world::car_body`) and the LOD-1 half box (`TrafficSpec::half_extents`, 0x445090).
    /// `car+0x52C` bit 4 (`is_traffic`) is what `Damage_byNormal` checks before the roof zone; an
    /// AI car's human-only arms are off.
    pub fn set_bodies(&mut self, specs: &nfs_physics::specs::CarSpecs, points: &[Vec<Vec3>]) {
        self.bodies = self
            .cars
            .iter()
            .map(|c| {
                let mut v = nfs_physics::vehicle::Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, specs, c.half_extents.x, RIDE_HEIGHT);
                v.ai_driven = true;
                v.is_traffic = true;
                v.body_lift = 0.0;
                // `RigidBody_init` 0x49B510: `car+0x84 = 1.0`. Only the racers' `LoadTransfer`
                // 0x4A1900 (inside `Car_simulate`, which a traffic car never runs — `traffic.md`
                // §21.5) puts 1.16 there. *(Until 2026-09-26 the bodies kept `Vehicle::new`'s 1.16.)*
                v.gravity_scale = 1.0;
                v.set_car_body(points.get(c.model).cloned().unwrap_or_default(), c.half_extents);
                v.position = c.position;
                v
            })
            .collect();
        self.specs = Some(specs.clone());
    }

    /// The kinematic record into car `i`'s body before the physics reads it: `car+0x330`,
    /// `+0x364`, `+0x33C`, `+0x388`, the body-frame velocity `car+0xD58…` (0x40A1D0 → 0x49F0C0),
    /// the AI's force `car+0xD4C…` and acceleration `car+0xD40`, the crash countdown `car+0x1100`
    /// and the alive flag `car+0xDB0`.
    pub fn sync_in(&mut self, i: usize) {
        let (Some(c), Some(b)) = (self.cars.get(i), self.bodies.get_mut(i)) else { return };
        b.position = c.position;
        b.orientation = c.orientation;
        b.velocity = c.velocity;
        b.angular_velocity = c.angular_velocity;
        b.v_local = c.orientation * c.velocity;
        b.accel_local = c.accel_local;
        b.accel_world = c.accel_world;
        b.half_extents = c.half_extents;
        b.reset_countdown = c.reset_countdown;
        b.engine_alive = c.alive as u32;
    }

    /// …and back out after it (the ground contact, a placement, the collision pass): the pose and
    /// velocities, `car+0x1100`, `car+0xDB0`, `car+0x3A0` (the up axis along the ground normal)
    /// and `car+0x464` (the body tracker's patch nibble — 0 off the surface).
    pub fn sync_out(&mut self, i: usize) {
        let (Some(c), Some(b)) = (self.cars.get_mut(i), self.bodies.get(i)) else { return };
        c.position = b.position;
        c.orientation = b.orientation;
        c.velocity = b.velocity;
        c.angular_velocity = b.angular_velocity;
        c.reset_countdown = b.reset_countdown;
        c.alive = b.engine_alive != 0;
        c.cos_tilt = b.cos_tilt;
        c.off_surface = b.tracker.word & 0xF == 0;
        if b.tracker.normal != Vec3::ZERO {
            c.ground_normal = Some(b.tracker.normal);
        }
    }

    /// Each car's `Car_ground` 0x49BD40 (`car+0xE20`, Sc32 priority 0x1E — after its drive):
    /// `Vehicle::ai_ground_step` — `Susp_update` 0x499A70 on the four wheel trackers, the pitch /
    /// roll rates, `BodyHeight`, then the resting branch — thinned by the ground gate's LOD rows
    /// (`nfs_physics::vehicle::ground_due`, `car+0x78` = [`TrafficCar::racer_distance`], the slot's
    /// phase). With no bodies (the unit tests) nothing runs; [`Self::integrate_on`] then holds the
    /// slice's height.
    pub fn ground(&mut self, grid: &nfs_physics::collision::TriangleGrid, tick: u64) {
        let Some(specs) = self.specs.clone() else { return };
        for i in 0..self.cars.len().min(self.bodies.len()) {
            if !self.cars[i].active {
                continue;
            }
            self.sync_in(i);
            let slot = self.slot_base + i;
            let b = &mut self.bodies[i];
            b.tracker.track(grid, b.position);
            if let Some((rate_hz, n_ticks)) = nfs_physics::vehicle::ground_due(self.cars[i].racer_distance, false, tick, slot) {
                b.ai_ground_step(grid, &specs, rate_hz, n_ticks, false);
                tracing::trace!(car = i, tick, patch = b.tracker.word & 0xF, cos_tilt = b.cos_tilt, air = b.airborne_count, h424 = b.body_height, y = b.position.y, "traffic ground");
            }
            self.sync_out(i);
        }
    }

    /// Set the track the race runs on, for the spawn gate's windows ([`no_spawn_windows`]).
    pub fn set_track(&mut self, name: &str) {
        self.no_spawn = no_spawn_windows(name);
    }

    pub fn active_count(&self) -> usize {
        self.cars.iter().filter(|c| c.active).count()
    }

    /// Slice arithmetic (`addSlices`): wrapping on a circuit, clamped at the ends of a linear track.
    fn add_slices(&self, table: &SliceTable, slice: usize, delta: i32) -> usize {
        if !self.linear {
            return table.add(slice, delta);
        }
        let n = table.lap_len as i32;
        let p = table.route_pos.get(slice).copied().unwrap_or(0) as i32;
        table.route[(p + delta).clamp(0, n - 1) as usize]
    }

    /// One 32 Hz tick: the AI pass ([`TrafficSet::ai`]) and then the drive ([`TrafficSet::drive`]),
    /// back to back — the unit tests' entry. The race runs the two halves apart, the AI with every
    /// other car's before any car's Sc32 step, as `GameTick` 0x414FF0 does (`traffic.md` §17.5d).
    pub fn update(&mut self, table: &SliceTable, tick: u64, reference: Reference, others: &[LaneCar]) {
        self.ai(table, tick, reference, others);
        self.drive(table, tick, reference, None, &[]);
    }

    /// [`Self::ai_on`] without a collision grid — a spawn then stands the car on the slice's own
    /// plane (the unit tests).
    pub fn ai(&mut self, table: &SliceTable, tick: u64, reference: Reference, others: &[LaneCar]) {
        self.ai_on(table, tick, reference, others, None)
    }

    /// The AI half of a tick: `AIHigh_Traffic::update` 0x406020 for every car in index order, which
    /// `0x406720` runs from `GameTick` 0x4150A9 on an even tick — purgatory and the spawn (§9, §11),
    /// the retire tests, and `AIState_Normal::update` 0x40B5A0's target speed, scripts and lane pass
    /// (§12, §23), which write only the targets `car+0xEB4` / `car+0xEB8`. `tick` is the 64 Hz game
    /// tick; `others` are the cars the lane scorers must see that this set does not own.
    ///
    /// Each car's update is thinned by the AI pass's own gate, [`ai_pass_due`] — 8 Hz inside 240 m
    /// of the nearest racer, 4 Hz beyond — and its counters step by the `[0x5E6654]` it returns.
    /// `grid` is the track's collision grid for the spawn's `Car_placeAtSlice` 0x49B090.
    pub fn ai_on(&mut self, table: &SliceTable, tick: u64, reference: Reference, others: &[LaneCar], grid: Option<&nfs_physics::collision::TriangleGrid>) {
        for i in 0..self.cars.len() {
            let Some(ai_ticks) = ai_pass_due(self.cars[i].racer_distance, false, tick, self.slot_base + i) else { continue };
            // `AIHigh_Traffic::update` 0x406020. Every arm ends in `0x40B4D0` on the state object
            // the arm leaves in place, which steps the scripts (a car in the world only) and runs
            // that state's update **in the same call** — so a new state runs at once.
            if !self.cars[i].active {
                // state 1 (0x40610E): the spawn gate `0x40E060` first — the delay `car+0xEE8` must
                // have run out, and if five cars are already out it re-arms the delay to 1 and says
                // no; on canyon / industrial it also says no, without re-arming, while the
                // reference car — `car+0xED4`, which the purgatory ctor 0x40DE10 set to racer 0,
                // the player — is in one of the track's windows — then, if it passes, a new
                // `AIState_Normal`, `Traffic_spawn` 0x407820, and 0x40B4D0 runs the new state's
                // update (0x406167…0x40616D) — the lane pass below. The arm also asks
                // `AIHigh+0x1C == 0` (0x40611E, and 0x4061D0 / 0x4063F4 in state 2): always true —
                // the only store to it is `AIHigh_Traffic`'s ctor's 0 (0x4064A8; every `[reg+0x1C]`
                // store in 0x401000…0x420000 and every `0x5E6774` read was checked).
                let mut spawned = false;
                if self.cars[i].respawn_ticks <= 0 {
                    if self.active_count() >= MAX_ACTIVE {
                        self.cars[i].respawn_ticks = 1; // 0x40E135
                    } else if self.no_spawn.iter().any(|&(a, b)| (a..=b).contains(&reference.slice)) {
                        tracing::debug!(car = i, slice = reference.slice, "traffic spawn gate: tunnel window");
                    } else {
                        self.spawn(i, table, reference, grid);
                        spawned = true;
                    }
                }
                if !spawned {
                    // 0x406460 → 0x40B4D0 → the purgatory update 0x40E140
                    self.purgatory_countdown(i, tick, ai_ticks);
                    continue;
                }
            } else if self.should_retire(i, table, reference) {
                // state 2 (0x40617A): `0x407030` says retire → a new purgatory (0x40DE10, which
                // arms the delay) and 0x40B4D0 runs its update at once (0x4061C3): the countdown
                // takes its first step in the same call
                self.retire(i);
                self.purgatory_countdown(i, tick, ai_ticks);
                continue;
            } else if self.police_reaction(i, table, reference, ai_ticks) {
                // states 2 / 3 with a cop near: `AIState_Idle`'s update ran instead of the lane pass
                continue;
            }
            // §14.4 longitudinal: step toward the target speed, never overshooting
            let (dir, factor) = (self.cars[i].dir, self.cars[i].speed_factor);
            // 0x40AF70 reads the car's own reference `car+0xED4`
            let target = target_speed(dir, self.race_dir, self.reference_of(i, reference).speed, factor).abs();
            // §12 / §23: score the three candidate lanes, move to the best one and cap the target
            // speed for the change. A traffic car's personality is row 0 (§23.10), whose lane offsets
            // are all zero, so there is no in-lane swerve to add.
            // in registration order (0x40F189): the caller's human, opponents and cops, then this
            // set's cars — the order `find_blockers`' race-order walk keeps for a tie
            let mut cars: Vec<LaneCar> = others.to_vec();
            cars.extend(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.as_lane_car(table)));
            // `car+0x528` of each entry of `cars` — the race-order places of the previous even tick
            // (0x401FA2 walks `0x5E8B88[]` from the car's own); without the others' (the unit tests)
            // the walk sorts the cars itself
            let places: Vec<usize> = self
                .other_places
                .iter()
                .copied()
                .chain(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.place))
                .collect();
            let placed = self.other_places.len() == others.len();
            let sl = table.slices[self.cars[i].slice];
            let c = &self.cars[i];
            let me = LaneSelf {
                slice: c.slice,
                lane: lane_of(&sl, c.lateral),
                lateral: c.lateral,
                dir: c.dir,
                speed: c.speed,
                target_speed: target * c.dir,
                half_width: c.half_extents.x,
                half_len: c.half_extents.z,
                is_traffic: true,
                // `0x40E9E0(car)` = `[0x628BC0]+4` (nSlicesLap) · `car+0xAF8` · 6.0 ([0x5B2460]) — for
                // a traffic car the metres from the lap's start to its slice, not the distance it drove
                travelled: metres(table, c.progress),
            };
            let (my_scan, hit_tick) = (c.scan, c.hit_by_human_tick);
            // `0x40B4D0` (0x40B4D0…0x40B509): the script runner steps **first** — `0x40AB80(car+0xE3C,
            // [0x5E6654])` while `car+0x7F`, not a human, and `word car+0x41A == 0` (always: the only
            // store to it is `RigidBody_init`'s 0, 0x49B714) — and only then does it call the state's
            // update (`call [edx]`, 0x40B509), whose 0x401580 raises this call's requests. The port
            // stepped after the requests, starting a script one AI update early (audit 2026-09-26).
            {
                let scripts = std::mem::take(&mut self.scripts);
                let before = self.cars[i].script.running();
                self.cars[i].script.step(&scripts, ai_ticks);
                if let Some(id) = self.cars[i].script.running()
                    && before != Some(id)
                {
                    tracing::debug!(car = i, id, "traffic script starts");
                }
                self.scripts = scripts;
            }
            // 0x401060's head (0x40106C…0x401116): `s = addSlices(car+8, 2·car+0x520, −1)`; a traffic
            // car (`52C & 0x10`) re-rolls only when `s` is even, with probability `(rand16 ·
            // pers[+0x1C]) >> 16 == 1`, and then draws `car+0xEC8 = pers[0xC + 4·(rand >> 8 & 3)]` —
            // all zero in row 0, but the draws are taken. `car+0x520` is the racer slot. (The port's
            // `add_slices` runs along the race route; ±2·slot slices keep the parity either way.)
            {
                let s = self.add_slices(table, self.cars[i].slice, 2 * (self.slot_base + i) as i32);
                if s % 2 == 0 && self.rng.below(PERSONALITIES[0].reroll) == 1 {
                    let _offset = PERSONALITIES[0].lane_offsets[(self.rng.rand16() & 3) as usize];
                }
            }
            // §12.9: ask for a script when the human does something worth reacting to, then step the
            // runner. `0x401580` returns at once while `0x40E9E0(car) < 800` ([0x5B2414], 0x4015A8):
            // a traffic car on the first 800 m of the lap (its `+0xAF8` being the bare fraction), and
            // on the call that places it, whose `+0xAF8` is still the purgatory's 0.
            {
                let travelled = me.travelled;
                // `0x40E9E0(human)`: the player's `+0xAF8` counts its laps
                let human_m = metres(table, reference.laps as f32 + table.frac(reference.slice));
                let (my_lane, my_dir, my_speed, my_slice) = (me.lane, me.dir, me.speed, me.slice);
                let time_mode = self.time_mode;
                let mut requests: Vec<(i32, Trigger)> = Vec::new();
                if travelled >= 800.0 {
                    if tick.saturating_sub(hit_tick) < 15 {
                        requests.push((0, Trigger::Collision));
                    }
                    for (hi, h) in cars.iter().enumerate().filter(|(_, h)| h.is_human) {
                        let d = crate::lanes::gap_between(table, h.slice, my_slice).abs();
                        // a much slower human close ahead
                        if my_dir * h.speed + 40.0 < my_dir * my_speed && d < 12.0 {
                            requests.push((hi as i32, Trigger::OpponentWhizzedBy));
                        }
                        // in the same lane and close: 2 when `0x40E9E0(car) < 0x40E9E0(human)`, else 3
                        // (0x401676…0x40169F) — metres along the lap, not the gap along the road
                        if d < 12.0 && h.lane == my_lane {
                            requests.push((hi as i32, if travelled < human_m { Trigger::HumanBlocksOpponent } else { Trigger::OpponentBlocksHuman }));
                        }
                        // 4 / 5 (0x4016B7…0x40170B): `human+0x550 == car` and `gGameTick −
                        // car+0x554 < 15` — **not** a collision: 0x417710's second sort (0x417783…
                        // 0x4177F9) writes both fields on every swap, so this is "the human's last
                        // swap in the `+0xAF8` order was with this car, and this car swapped within
                        // 15 ticks" — an overtake ([`Self::overtakes`]). The port used the collision
                        // tick until the audit of 2026-09-26. 4 when `0x40E9E0(human) <
                        // 0x40E9E0(car)`, else 5 (0x4016D2…0x4016FB)
                        let me_flat = self.overtake_base + i;
                        let swapped = self.swap_partner.first().copied().flatten() == Some(me_flat)
                            && self.swap_tick.get(me_flat).copied().flatten().is_some_and(|t| tick.saturating_sub(t) < 15);
                        if swapped {
                            requests.push((hi as i32, if human_m < travelled { Trigger::OpponentPassesHuman } else { Trigger::HumanPassesOpponent }));
                        }
                        // trigger 6, `human honk` (0x401713…0x40174E): the slice distance under
                        // 30 m (`[0x5B2408]`), the human's horn byte `human+0xD86` set, and the
                        // `TIME` option `[0x657A30]` clear. The port has one human, the reference.
                        if d < 30.0 && reference.honking && !time_mode {
                            requests.push((hi as i32, Trigger::HumanHonks));
                        }
                    }
                }
                // the requests go in after this call's step (above): a trigger raised now starts
                // its script on the car's next AI update
                let c = &mut self.cars[i];
                for (target, trigger) in requests {
                    tracing::debug!(car = i, ?trigger, "traffic script trigger");
                    c.script.request(target, trigger, tick as i64);
                }
            }

            // 0x401171…0x40123C, after the triggers: a traffic car travelling against the race
            // (`car+0xEB0 != (REVERSE ? −1 : 1)`), with cars in the race, draws `(rand16·1000) >> 16`
            // and, below 5 and while `|car+0xEC0| > 0.2` (`[0x5B23F4]`; `test ah, 0x41; jne` skips
            // `<= 0.2`), sounds `HonkPattern(car, 2, 0x20, 8)` 0x4A6DE0. The `[0x5E6674] >= −3` test
            // before it reads the scratch 0x401000 has just zeroed (0x401F20 fills it later, at
            // 0x401246), so it always passes; `[0x657404]` (split screen) is never 1 here. The call
            // finds the car's audio slot and programs its honk timer (0x4A6DE0) — here the tick is
            // recorded (`honk_tick`) and the odd tick's audio update programs the timer from it.
            // *(Until 2026-09-26 the draw was only logged.)*
            if self.cars[i].dir != 1.0 {
                let r = self.rng.below(1000);
                if r < 5 && self.cars[i].speed.abs() > 0.2 {
                    tracing::debug!(car = i, "traffic honks: HonkPattern(car, 2, 0x20, 8)");
                    self.cars[i].honk_tick = Some(tick);
                }
            }
            let mut sc = LaneScratch::default();
            let one_way = self.one_way;
            let mut scan = my_scan;
            // world positions in the order of `cars` for 0x40E860's fine step; an `others` entry the
            // caller gave no position for falls back to the slice gap (NaN is never within ±12 m of
            // anything the fine step would accept, so it is filtered out below)
            let positions: Vec<Vec3> = (0..others.len())
                .map(|k| self.other_positions.get(k).copied().unwrap_or(Vec3::NAN))
                .chain(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.position))
                .collect();
            let fine_ok = positions.iter().all(|p| p.is_finite());
            sc.find_blockers_placed(table, &me, &cars, &PERSONALITIES[0], fine_ok.then_some((self.cars[i].position, positions.as_slice())), placed.then_some((self.cars[i].place, places.as_slice())));
            sc.score_own_side(&me, one_way, true);
            // §12.3's script reactions push the same lane scores the scorers do
            {
                let r = &self.cars[i].script;
                // SWERVE (0x40146E…0x4014E7): the weave runs while the target is **more** than 12 m
                // away — `fcomp [0x5B240C]; test ah, 0x41; jne skip` at 0x40149B…0x4014A6 skips
                // `|d| <= 12` (the port had `< 12`, after the note). No shipped script uses SWERVE
                // (`traffic.md` §30.x), so this is dead data either way.
                if let Some(t) = r.query(nfs_formats::scripts::mask::SWERVE)
                    && let Some(o) = cars.get(t.max(0) as usize)
                    && crate::lanes::gap_between(table, o.slice, me.slice).abs() > 12.0
                {
                    // a deterministic 1 Hz weave off the script's own timer, not a random draw:
                    // `0x40AC80(script)`, `cdq; and edx, 0x1F; add; sar 5` = the signed `/ 32`
                    if (r.timer() / 32) & 1 == 1 {
                        sc.score[0] += 30.0;
                    } else {
                        sc.score[2] += 30.0;
                    }
                }
                if let Some(t) = r.query(nfs_formats::scripts::mask::SWIPE)
                    && let Some(o) = cars.get(t.max(0) as usize)
                    // `0x40E860(car, o)` (0x401503: `push edi` the target, `push esi` the car) is
                    // positive when **this car** is ahead — `gap_between(car, o)`; the port had the
                    // arguments the other way round, i.e. the sign flipped. The slice gap alone
                    // (APPROXIMATION inside ±12 m; dead data for traffic: row 0 has no SWIPE)
                    && crate::lanes::gap_between(table, me.slice, o.slice) * me.dir < 4.0
                {
                    match o.lane.cmp(&me.lane) {
                        std::cmp::Ordering::Greater => sc.score[2] += 5.0,
                        std::cmp::Ordering::Less => sc.score[0] += 5.0,
                        std::cmp::Ordering::Equal => {}
                    }
                }
            }
            sc.score_blockers(&me);
            sc.score_abreast(table, &me, &cars);
            sc.score_lane_exists(table, &me, &mut scan, ai_ticks);
            sc.score_narrowing(table, &me);
            sc.score_shoulders(table, &me, &cars);
            let (slot, lane) = sc.pick(&me);
            if lane != me.lane {
                tracing::debug!(car = i, from = me.lane, to = lane, scores = ?sc.score, "traffic lane change");
            }
            let lateral_target = sc.lateral_target(table, &me, lane, slot, &cars);
            if (lateral_target - sc.lane_centre(table, &me, lane)).abs() > 0.01 {
                tracing::debug!(car = i, lane, lateral_target, centre = sc.lane_centre(table, &me, lane), "traffic squeezes past");
            }
            let capped = sc.capped_target(&me, lane, lateral_target, self.cars[i].lateral_rate, &cars);
            let c = &mut self.cars[i];
            c.scan = scan;
            c.lateral_target = lateral_target;
            c.target_speed = capped;
        }
    }

    /// `0x417710`'s second sort (0x417783…0x4177F9), at the end of `0x417800` on an even tick — after
    /// the Sc32 list, before the Sc64 progress update: `0x5E8BAC[]` bubble-sorted ascending by
    /// `car+0xAF8` (a swap when the earlier key is **greater**, `test ah, 0x41; jne` skipping `<=`),
    /// and on every swap both cars record each other in `+0x550` and the tick in `+0x554`. `keys` is
    /// every car's `car+0xAF8` in registration order (the human, the opponents, the cops, the
    /// traffic — `0x40F189`, which is also the array's initial order); `base` is where this set's
    /// car 0 sits in it. 0x401580's triggers 4 / 5 read the result.
    pub fn overtakes(&mut self, keys: &[f32], tick: u64, base: usize) {
        let n = keys.len();
        if self.overtake_order.len() != n {
            self.overtake_order = (0..n).collect();
            self.swap_partner = vec![None; n];
            self.swap_tick = vec![None; n];
        }
        self.overtake_base = base;
        loop {
            let mut swapped = false;
            for j in 0..n.saturating_sub(1) {
                let (a, b) = (self.overtake_order[j], self.overtake_order[j + 1]);
                if keys[a] > keys[b] {
                    self.swap_partner[a] = Some(b);
                    self.swap_partner[b] = Some(a);
                    self.swap_tick[a] = Some(tick);
                    self.swap_tick[b] = Some(tick);
                    self.overtake_order.swap(j, j + 1);
                    swapped = true;
                }
            }
            if !swapped {
                break;
            }
        }
    }

    /// The chase targets this pass: [`Self::chase_targets`], or the human alone.
    fn targets(&self, reference: Reference) -> Vec<ChaseTarget> {
        if self.chase_targets.is_empty() { vec![reference.into()] } else { self.chase_targets.clone() }
    }

    /// Car `i`'s reference `car+0xED4` as a chase target.
    fn reference_of(&self, i: usize, reference: Reference) -> ChaseTarget {
        let t = self.targets(reference);
        t[self.cars[i].reference.min(t.len() - 1)]
    }

    /// The chase targets in `0x5E8B88[]`'s order — by the place each holds in the array the previous
    /// even tick's `0x417710` sorted ([`crate::race_order::RaceOrder`], carried 2026-09-26; before
    /// that a fresh sort on the key, ties in registration order).
    fn race_order(targets: &[ChaseTarget]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..targets.len()).collect();
        order.sort_by_key(|&a| targets[a].place);
        order
    }

    /// `0x405ED0(this, &out)` — the nearest cop in the world by `|0x40E6E0(car, cop)|` (the first of
    /// equals, from 20000; `car+0xECC & 4` skipped, 0x405F0F), and whether it counts: within 75 m
    /// (`[0x5B257C]`) and either moving faster than 2 m/s (`[0x5B2580]`, 0x405FDB…0x405FF7) or,
    /// slower, with a perp claimed (`AIHigh+0x58`, 0x405FC0…0x405FC5). Exactly 2 m/s counts
    /// neither way. The `*out` flag is not read by 0x406020.
    fn cop_near(&self, i: usize, table: &SliceTable) -> bool {
        let slice = self.cars[i].slice;
        let mut best: Option<(&CopView, f32)> = None;
        for c in self.cops.iter().filter(|c| c.in_world) {
            let d = crate::lanes::gap_between(table, slice, c.slice).abs();
            if d < best.map_or(20000.0, |b| b.1) {
                best = Some((c, d));
            }
        }
        let Some((cop, d)) = best else { return false };
        let v = cop.speed.abs();
        d < 75.0 && ((v < 2.0 && cop.has_target) || v > 2.0)
    }

    /// `AIHigh_Traffic::update` 0x406020's states 2 and 3 around a cop (`traffic.md` §11, read to the
    /// instruction 2026-09-26, §34.4), for a car in the world whose retire test has said no. Returns
    /// true when the car is (now) stopped and `AIState_Idle`'s update 0x40CAB0 ran in place of the lane
    /// pass; false when `AIState_Normal`'s update is to run — including a car that has just left
    /// state 3, whose new Normal runs in the same call (0x406456 → 0x406460).
    ///
    /// * State 2 (0x4061D0…0x40634C): `0x405ED0` finds no cop → `+0x18 = 0`, drive. A cop and `+0x18
    ///   == 0` → `r = (rand16 · 10) >> 16` (0x4061F6…0x406227): `r < 1` → `AIState_Idle` mode 1, stop
    ///   dead; `r < 8` → `AIState_Idle` + `0x40CB40(extent)`, pull over; else `+0x18 = 1` and drive. The
    ///   extent (0x4062CE…0x406307) is `laneWidthB · lanesB` when `car+0xEB0 == 1`, else `laneWidthA ·
    ///   lanesA`, of the car's slice — **positive in both** (no `fchs`), so an oncoming car pulls over
    ///   across the road (the original's own oddity, kept). In the port's frame that is the car's own
    ///   side's product, negated on a backwards table (which swaps the sides and negates the
    ///   lateral; `SliceTable::backwards`).
    /// * State 3 (0x40634D…0x4063DE): a cop still near (or `car+0x52C & 0x400`, never traffic) → stay
    ///   stopped; else a new `AIState_Normal`, state 2, run at once.
    ///
    /// Either way 0x40B4D0 steps the script runner first (§34.3).
    fn police_reaction(&mut self, i: usize, table: &SliceTable, reference: Reference, ai_ticks: i32) -> bool {
        let near = self.cop_near(i, table);
        if self.cars[i].stopped.is_none() {
            if !near {
                self.cars[i].ignore_cops = false;
                return false;
            }
            if self.cars[i].ignore_cops {
                return false;
            }
            let r = self.rng.below(10);
            let c = &mut self.cars[i];
            if r >= 8 {
                c.ignore_cops = true;
                tracing::debug!(car = i, "traffic ignores the cop");
                return false;
            }
            let mode = if r < 1 {
                StopMode::Dead
            } else {
                let sl = &table.slices[c.slice];
                let extent = if c.dir == 1.0 { sl.lane_width_b * sl.lanes_b as f32 } else { sl.lane_width_a * sl.lanes_a as f32 };
                // the port's backwards table (`SliceTable::backwards`; `race_dir` stays +1 there)
                // negates the lateral
                StopMode::PullOver(if table.backwards { -extent } else { extent })
            };
            c.stopped = Some(mode);
            tracing::debug!(car = i, ?mode, "traffic stops for a cop");
        } else if !near {
            self.cars[i].stopped = None;
            tracing::debug!(car = i, "traffic drives on: no cop near");
            return false;
        }
        // 0x40B4D0: the script step, then `AIState_Idle::update` 0x40CAB0
        let scripts = std::mem::take(&mut self.scripts);
        self.cars[i].script.step(&scripts, ai_ticks);
        self.scripts = scripts;
        let race_dir = self.race_dir;
        let ref_speed = self.reference_of(i, reference).speed;
        let c = &mut self.cars[i];
        match c.stopped {
            Some(StopMode::Dead) => {
                c.target_speed = 0.0;
                c.lateral_target = c.lateral;
            }
            Some(StopMode::PullOver(x)) => {
                // `d = car+0xED0 − this+8`: `d <= −1` or `d >= 1` (0x40CAD7…0x40CAFA) → 0x40AF70's
                // target speed × 0.25 (`[0x5B2468]`); inside the metre, 0
                let d = c.lateral - x;
                c.target_speed = if d <= -1.0 || d >= 1.0 { target_speed(c.dir, race_dir, ref_speed, c.speed_factor) * 0.25 } else { 0.0 };
                c.lateral_target = x;
            }
            None => unreachable!(),
        }
        true
    }

    /// `AIState_Purgatory::update` 0x40E140: for a car without the cop bit, once `gGameTick ≥
    /// 0x3C0` (0x40E160 `cmp`, `jl` skips), `car+0xEE8 −= [0x5E6654]` — the AI pass's step. There is
    /// no floor: the spawn gate 0x40E060 only asks `car+0xEE8 > 0`.
    ///
    /// It first stores `car+0x1100 = car+0x43C = 0` (0x40E145 / 0x40E14E), as the purgatory's exit
    /// 0x40DFB0 does (0x40DFBA / 0x40DFC7). `car+0x43C` has no effect: it is only
    /// ever stored 0 (0x40DFC7, 0x40E14E, 0x40F45B, 0x410D0A, 0x49B6F2) and 0x49900E only decrements
    /// it while positive, so 0x498DE0's bail on it (0x498DE9) never fires — dead. `car+0x1100` is the
    /// crash countdown (module docs), which 0x40FBA2 would keep re-arming on a parked car left off
    /// the surface; this clear is what keeps it from running out there.
    fn purgatory_countdown(&mut self, i: usize, tick: u64, ai_ticks: i32) {
        self.cars[i].reset_countdown = 0;
        if tick >= SPAWN_GATE_TICK {
            self.cars[i].respawn_ticks -= ai_ticks;
        }
    }

    /// `0x417190` from each car's tracker 0x49AF70 (Sc32 priority 6, after the AI pass): `car+0x78`,
    /// the octagonal x–z distance to the nearest racer (`0x5E8E2C[0 … [0x5E8AE8])`, from 1000 —
    /// `[0x5B2468]` = 0.25), for the cars in the world only.
    pub fn track_racers(&mut self, racers: &[Vec3]) {
        for c in self.cars.iter_mut().filter(|c| c.active) {
            c.racer_distance = racers.iter().map(|&r| oct_distance(c.position, r)).fold(1000.0, f32::min);
        }
    }

    /// The drive half of a tick: `0x408360`, a traffic car's `car+0xE1C` on the Sc32 list at
    /// priority 0x1E (`traffic.md` §14.1, §21.5), which runs after the whole AI pass. It reads
    /// the targets [`TrafficSet::ai`] wrote and writes the velocities the 64 Hz integrators move.
    /// `grid` and `occupied` (the racers', the cops' and the smackables' slices) serve the 0x408750 net's reset
    /// ([`TrafficSet::reset_to_slice`]) at the end of a driven step (0x4086E6).
    pub fn drive(&mut self, table: &SliceTable, tick: u64, reference: Reference, grid: Option<&nfs_physics::collision::TriangleGrid>, occupied: &[usize]) {
        for i in 0..self.cars.len() {
            // a car placed by this tick's AI pass is driven too: 0x408360 only asks for
            // `car+0x7F` (0x408442), which the new `AIState_Normal`'s ctor set (0x40B605)
            if !self.cars[i].active {
                continue;
            }
            let sl = table.slices[self.cars[i].slice];
            // `0x4081B0`'s SPEED_BURST branch multiplies the rate by 1.5 (mask 4, not HARD_BRAKE)
            let burst = if self.cars[i].script.query(nfs_formats::scripts::mask::SPEED_BURST).is_some() { 1.5 } else { 1.0 };
            let c = &mut self.cars[i];

            // §14.3's row-1 gate at the top of the drive 0x408360 (0x408394): 32 Hz while the car
            // is within 160 m of the nearest racer (`car+0x78`, which the tracker 0x49AF70 wrote at
            // Sc32 priority 6 this tick), 16 Hz with twice the timestep beyond, staggered by the
            // slot; in between the car keeps its last velocity. The AI pass has its own gate
            // (row 0, [`ai_pass_due`]).
            let Some((ai_dt, n_ticks)) = drive_due(c.racer_distance, false, tick, self.slot_base + i) else { continue };

            // §21.7's exits, taken before `0x408360` drives at all: a car lying on its side that has
            // never been hit, or one whose target speed is zero, is only damped — `vel *= k`,
            // `ω.y *= 0.98` — and does not steer. The damaged-out and player exits cannot fire for
            // traffic (§21.7).
            let up_y = c.orientation.row(1).y;
            let damp = if c.collisions == 0 && up_y < 0.3 {
                Some(0.94)
            } else if c.target_speed == 0.0 {
                Some(0.94)
            } else {
                None
            };
            if let Some(k) = damp {
                // 0x4084CE… / 0x4085FF…: the AI's force `car+0xD4C…` is zeroed on the way out
                c.accel_local = Vec3::ZERO;
                c.velocity *= k;
                c.angular_velocity.y *= 0.98;
                continue;
            }

            // §27.2: the drive model. The kinematic `0x408840` of §14.5 is dead code — `car+0x7E` is
            // never non-zero — so the car is a point mass with a yaw degree of freedom driven by two
            // P-cascades whose gains come from `trafcfg.dat`.
            // §27.2a: `AI_drive` runs the reverse trim, then asks whether the car is in trouble
            // and, if it is, flips the gear every 32 ticks. An upright car that is still inside the
            // slice's lane extent never enters this at all.
            let lane = lane_of(&sl, c.lateral);
            let (trim_speed, trim_target) = aidrive::reverse_trim(&mut c.gear, tick as i64, c.lateral, c.lateral_target, c.lateral_rate, c.target_speed);
            c.target_speed = trim_speed;
            c.lateral_target = trim_target;
            // `car+0x1080` steps by `[0x5E6D78]` (0x40A5E7), the 32 Hz ticks this drive step covers
            if aidrive::in_trouble(&mut c.gear, lane, sl.lanes_a as i32, sl.lanes_b as i32, up_y, n_ticks) {
                aidrive::toggle_gear(&mut c.gear, tick as i64, c.speed, aidrive::GEAR_PERIOD);
            }
            let mut st = c.as_ai_state(&self.phys);
            let aim = aidrive::aim_direction(table, &self.cfg, &st);
            let err = aidrive::heading_error(&st, aim);
            let road = (sl.width_a, sl.width_b);
            // 0x40AF24: `human+0xAF8` (laps counted) against this car's bare lap fraction
            let boost = if self.cops_race { 1.0 } else { early_boost(self.opponents, tick, 1, reference.laps as f32 + table.frac(reference.slice), c.progress, 0) };
            let rate = accel_rate(st.speed) * burst * boost * if self.weather { 0.9 } else { 1.0 };
            let bend = aidrive::road_bend(table, c.slice, c.dir);
            let cmd = if aidrive::out_of_control(&st, &self.cfg.out_of_control, err, road, bend) {
                aidrive::out_of_control_command(&st, &self.cfg.out_of_control, err, rate, self.phys.brake_force)
            } else {
                // `grip` is the post-collision ramp, 1.0 for an undamaged car
                let grip = 1.0 + 0.0015 * aidrive::body_forward_speed(&st);
                aidrive::in_control_command(&st, &self.cfg.in_control, err, aim, grip, 1.0, rate, self.phys.brake_force)
            };
            tracing::trace!(car = i, err, ooc = aidrive::out_of_control(&st, &self.cfg.out_of_control, err, road, bend), lat = st.lateral, lat_t = st.lateral_target, speed = st.speed, yaw = st.angular_velocity.y, cmd_lat = cmd.lateral, cmd_yaw = cmd.yaw, "traffic drive");
            aidrive::integrate(&mut st, &cmd, ai_dt);
            // `[0x5E6D78]`: the 32 Hz ticks this drive step covers (0x409AC8 / 0x409B65 in
            // 0x409720, 0x40A0BC / 0x40A124 in 0x409C70)
            c.brake_light = aidrive::brake_light(c.brake_light, cmd.braking, n_ticks as u8);
            c.velocity = st.velocity;
            c.angular_velocity = st.angular_velocity;
            // the controller's `car+0xD4C…` (lateral, 0, forward; 0x40A18F / 0x40A1B9) and
            // 0x40A1D0's `car+0xD40…` = that force in the world (0x40A236 / 0x40A27B)
            c.accel_local = Vec3::new(cmd.lateral, 0.0, cmd.forward);
            c.accel_world = c.orientation.row(0) * cmd.lateral + c.orientation.row(2) * cmd.forward;
            c.lateral_rate = c.velocity.dot(sl.lateral.normalize_or_zero());
            // 0x408750 at the end of the driven step (0x4086E6): `0x40F540(car, 1, 0)` — the two
            // damping exits above return before it (0x408529)
            if self.is_out_of_bounds(i, table) {
                tracing::debug!(car = i, vel = ?self.cars[i].velocity, lateral = self.cars[i].lateral, "traffic insane: 0x408750 reset");
                self.reset_to_slice(i, table, grid, occupied, 1);
            }
        }
    }

    /// The post-impact pass, `Car_postImpact` 0x410280 → 0x40F740 at Sc32 priority 0x32 for a
    /// traffic car (module docs): the crash countdown 0x40FB13…0x40FB6A and its arm 0x40FB6D…0x40FBA2.
    /// The countdown runs on the impact `Damage` armed it with (the port's traffic body carries that
    /// into [`TrafficCar::reset_countdown`] and [`TrafficCar::alive`] after the collision pass) and
    /// on the roof / off-surface arm here. A car in purgatory is skipped: the purgatory's update
    /// clears the countdown ([`Self::purgatory_countdown`]) faster than it can run out.
    pub fn post_impact(&mut self, table: &SliceTable, grid: Option<&nfs_physics::collision::TriangleGrid>, occupied: &[usize]) {
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                continue;
            }
            let c = &mut self.cars[i];
            let mut reset = false;
            if c.reset_countdown > 0 {
                c.reset_countdown -= 1;
                if c.off_surface {
                    c.reset_countdown -= 8;
                }
                if c.cos_tilt > 0.75 && c.alive {
                    c.reset_countdown = 0;
                } else if c.reset_countdown <= 0 {
                    reset = true;
                }
            }
            if reset {
                tracing::debug!(car = i, alive = self.cars[i].alive, off_surface = self.cars[i].off_surface, tilt = self.cars[i].cos_tilt, "traffic crash countdown: 0x40F540 reset");
                self.reset_to_slice(i, table, grid, occupied, 0);
            }
            // 0x40FB6D: `car+0x370 · car+0x3C4 < 0.1` or off the surface, read after the reset.
            // `car+0x3C4` is the body tracker's patch normal, not the slice's up (audit 2026-09-26,
            // `traffic.md` §34.13); the slice's up only without a body
            let c = &mut self.cars[i];
            if c.reset_countdown <= 0 {
                let normal = c.ground_normal.unwrap_or_else(|| table.slices[c.slice].up.normalize_or_zero());
                if c.orientation.row(1).dot(normal) < 0.1 || c.off_surface {
                    c.reset_countdown = 0xC0;
                }
            }
        }
    }

    /// `Car_resetToSlice` 0x40F540 for a traffic car (`race-logic.md` §7): `car+0x1104 = car+0x1100
    /// = 0` (0x40F597); `car+0x540` is 0 for a traffic car, so the early return after it never
    /// fires; mode 0's "the car is fine" exit (0x40F5BF…0x40F632) is dead code for every car —
    /// it needs both `car+0x464 != 0` and `car+0x12 != 0`, and `car+0x12` is the body tracker's
    /// "no patch" byte (tracker `+0xA`, 1 only when `findPatch` failed, 0x4740B1, when the patch
    /// word `+0x24` — `car+0x2C`, whose low nibble is `car+0x464` — is 0). Then the free-slice
    /// search from the car's slice (0x40F480: a drivable patch under the slice centre and no live
    /// car of `0x5E8B34` — the racers, the cops and this set's other cars — nor any smackable of
    /// `0x5E8C24`, asleep or not, on it; `occupied` carries all of those but this set's own),
    /// stepping one slice along the route (0x4817D0), 0x49E220
    /// (re-arms `car+0xDB0`), `Car_placeAtSlice` 0x49B090 with a zero offset (0x40F6DE: the
    /// racing-line point is the opponents' only) and the direction `car+0xEAC`, the velocity
    /// zeroed (0x40F712…0x40F71E) and 0x40F400 — spin, `+0x418`, `+0x35C` and the impact record.
    pub fn reset_to_slice(&mut self, i: usize, table: &SliceTable, grid: Option<&nfs_physics::collision::TriangleGrid>, occupied: &[usize], mode: u8) {
        self.cars[i].reset_countdown = 0;
        let mut s = self.cars[i].slice;
        for _ in 0..table.lap_len.max(1) {
            let drivable = grid.is_none_or(|g| g.find_patch(table.slices[s].centre) & 0xF != 0);
            let taken = occupied.contains(&s) || self.cars.iter().enumerate().any(|(j, o)| j != i && o.active && o.slice == s);
            if drivable && !taken {
                break;
            }
            s = self.add_slices(table, s, 1);
        }
        let sl = table.slices[s];
        let dir = self.cars[i].dir;
        let (fwd, _, up) = axes(&sl, dir);
        let rest = self.cars[i].rest_height();
        self.cars[i].slice = s;
        match (grid, self.specs.clone()) {
            (Some(g), Some(specs)) if i < self.bodies.len() => {
                // 0x49E220 (re-arms `car+0xDB0`, the drivetrain and the AI's forces), then
                // `Car_placeAtSlice` 0x49B090 with the zero offset and `car+0xEAC`
                self.cars[i].accel_local = Vec3::ZERO;
                self.cars[i].accel_world = Vec3::ZERO;
                self.cars[i].alive = true;
                self.sync_in(i);
                let b = &mut self.bodies[i];
                b.reset_countdown = 0;
                b.rearm_after_reset();
                for w in &mut b.wheels {
                    *w = nfs_physics::vehicle::Wheel::default();
                }
                b.place_at_slice(g, &specs, sl.centre, sl.forward.normalize_or_zero(), Vec3::ZERO, false, dir < 0.0);
                self.sync_out(i);
            }
            _ => {
                let c = &mut self.cars[i];
                c.position = sl.centre + up * rest;
                c.orientation = heading(fwd, up);
            }
        }
        let c = &mut self.cars[i];
        c.alive = true;
        c.velocity = Vec3::ZERO;
        c.angular_velocity = Vec3::ZERO;
        c.speed = 0.0;
        c.lateral = 0.0;
        c.lateral_rate = 0.0;
        c.collisions = 0;
        c.cos_tilt = 1.0;
        c.off_surface = false;
        c.resets += 1;
        tracing::debug!(car = i, slice = s, mode, "traffic reset to slice");
    }

    /// The shared 64 Hz integrators (`0x49B7A0` position, `0x49B8E0` orientation) plus the
    /// slice/lateral tracking of `0x49AF70`.
    pub fn integrate(&mut self, table: &SliceTable, dt: f32) {
        self.integrate_on(table, dt, None)
    }

    /// The same step with the track's collision grid. The ground contact itself is
    /// [`Self::ground`] (`Car_ground` 0x49BD40 on the car's body, Sc32); here the shared 64 Hz
    /// integrators move the record — `0x49B7A0` the position, `0x49B8E0` the orientation by the
    /// whole `ω/64` (`nfs_physics::vehicle::orientation_step`) — and `0x49AF70` re-finds the slice.
    /// Without bodies (the unit tests) the car is held at the slice's height and yawed about the
    /// world up — a port fallback with no original counterpart.
    pub fn integrate_on(&mut self, table: &SliceTable, dt: f32, ground: Option<&nfs_physics::collision::TriangleGrid>) {
        self.integrate_tick(table, dt, ground, None)
    }

    /// [`Self::integrate_on`] with the 64 Hz game tick: `car+0xAF8` is written by `car+0xE18` =
    /// 0x4169B0 (Sc64 priority 0x19, even ticks, `traffic.md` §17.5d) **before** the integrators
    /// (priority 0x1E) move the car, from the slice the tracker found last tick — so the lap
    /// fraction the readers see (0x40E9E0's metres, the boost, the order sort) trails the move by
    /// up to two ticks. Without a tick (the unit tests) it is written every call, after the move,
    /// as the port did until 2026-09-26.
    pub fn integrate_tick(&mut self, table: &SliceTable, dt: f32, ground: Option<&nfs_physics::collision::TriangleGrid>, tick: Option<u64>) {
        let bodied = !self.bodies.is_empty() && ground.is_some();
        for c in self.cars.iter_mut().filter(|c| c.active) {
            if let Some(t) = tick
                && t % 2 == 0
            {
                c.progress = table.frac(c.slice);
            }
            c.position += c.velocity * dt;
            if bodied {
                c.orientation = nfs_physics::vehicle::orientation_step(c.orientation, c.angular_velocity * dt);
            } else {
                // yaw about the world up, the angular velocity being in turns per second
                let yaw = c.angular_velocity.y * dt * std::f32::consts::TAU;
                if yaw != 0.0 {
                    c.orientation = (Mat3::from_rotation_y(yaw) * c.orientation.transpose()).transpose();
                }
            }
            c.slice = table.find_slice(c.position, c.slice);
            // `car+0xAF8`, 0x4169B0's traffic arm: the bare lap fraction of the slice. The port's
            // `frac` is the slice's place on the lap route of the (already reversed) table, as for
            // the racers — on even ticks before the move (above); every call after it only without
            // a tick
            if tick.is_none() {
                c.progress = table.frac(c.slice);
            }
            let sl = &table.slices[c.slice];
            let lat_axis = sl.lateral.normalize_or_zero();
            // `car+0xED0 = 0x4173C0(car)` — the lateral offset re-derived from the world position, and
            // `car+0xEC0` the speed along the route (`0x417800`)
            c.lateral = (c.position - sl.centre).dot(lat_axis);
            c.speed = c.velocity.dot(sl.forward.normalize_or_zero());
            if bodied {
                continue;
            }
            c.position.y = sl.centre.y + c.rest_height();
            c.cos_tilt = c.orientation.row(1).y;
            // `car+0x464 = car+0x2C & 0xF` (0x49AFD8): the body tracker's patch word at `car+0x330`
            // — `findPatch` 0x473240, the drivable patch whose plan contains the point. Without a
            // grid nothing can say, and the car is taken as on the surface.
            c.off_surface = ground.is_some_and(|g| g.find_patch(c.position) & 0xF == 0);
            c.velocity.y = 0.0;
        }
    }

    /// `0x408750` — the sanity net at the end of every driven step, read to the instruction
    /// 2026-09-26: `vel.z > 115` (`[0x5B25FC]`, 0x408758) or `vel.x > 115` (0x40876D) — signed, no
    /// `fabs` — or the octagonal speed `car+0x35C > 115` while `car+0x7E == 0` (0x408780…0x408798;
    /// `+0x7E` is never set for a traffic car, §14.5), or the lateral `car+0xED0` past the slice's
    /// **`+0x40`** width (`width_a`, used for both sides: 0x4087B2 `fchs`, `fsub 35`; then the same
    /// word `fadd 35`, 0x4087CF) by 35 m (`[0x5B25F8]`). Then `0x40F540(car, 1, 0)`
    /// ([`Self::reset_to_slice`], mode 1 — no exit). The port used to test `|vel|` and `width_b` on
    /// the positive side, and sent the car back to purgatory instead of the in-place reset.
    fn is_out_of_bounds(&self, i: usize, table: &SliceTable) -> bool {
        let c = &self.cars[i];
        if c.velocity.z > 115.0 || c.velocity.x > 115.0 || c.speed_estimate() > 115.0 {
            return true;
        }
        // `+0x40` is the track's own A width; the port's backwards table has swapped A and B (and
        // negated the lateral, which the symmetric test does not see), so there it is `width_b`
        // (audit 2026-09-26, `traffic.md` §34.10)
        let sl = &table.slices[c.slice];
        let w = if table.backwards { sl.width_b } else { sl.width_a };
        c.lateral < -w - 35.0 || c.lateral > w + 35.0
    }

    /// `0x407030` — within 5 slices of either end of a linear track, or more than 400 m from the
    /// reference `car+0xED4` in the octagonal metric **and** from every chase target (§10; read to the
    /// instruction 2026-09-26, 0x407030…0x4071D7): past 400 m it walks `0x5E8B10[0 … [0x5E8AB8])` in
    /// order and rebinds `car+0xED4` to the first within 400 m (0x4071B2…0x4071BF), keeping the car.
    fn should_retire(&mut self, i: usize, table: &SliceTable, reference: Reference) -> bool {
        let c = &self.cars[i];
        if self.linear {
            let p = table.route_pos.get(c.slice).copied().unwrap_or(0) as i32;
            if p <= 5 || (table.lap_len as i32 - 1 - p) <= 5 {
                return true;
            }
        }
        let pos = c.position;
        if oct_distance(pos, self.reference_of(i, reference).position) <= DESPAWN_DISTANCE {
            return false;
        }
        match self.targets(reference).iter().position(|t| oct_distance(pos, t.position) <= DESPAWN_DISTANCE) {
            Some(k) => {
                self.cars[i].reference = k;
                false
            }
            None => true,
        }
    }

    fn retire(&mut self, i: usize) {
        let delay = RESPAWN_TABLE[self.cars.len().min(RESPAWN_TABLE.len() - 1)];
        let bus = self.cars[i].bus;
        let d = (1 + self.rng.below(delay) as i32) * if bus { 5 } else { 1 };
        let c = &mut self.cars[i];
        c.active = false;
        // a new `AIState_Purgatory`, state 1, from state 2 or 3 alike (0x40618A / 0x4063F9); the
        // spawn gives it a new `AIState_Normal`, state 2
        c.stopped = None;
        // the purgatory ctor: `car+0xED4 = 0x5E8B34[0]`, the human (0x40DE6D…0x40DE73)
        c.reference = 0;
        // `car+0x7F = 0` (0x40DE60), so 0x4169B0 stores `car+0xAF8 = 0` (0x416AAC)
        c.progress = 0.0;
        c.velocity = Vec3::ZERO;
        c.speed = 0.0;
        c.respawn_ticks = d;
        tracing::debug!(car = i, delay = d, "traffic retire");
    }

    /// `Traffic_spawn` 0x407820 (§9.3–§9.6), in the original's order.
    fn spawn(&mut self, i: usize, table: &SliceTable, reference: Reference, grid: Option<&nfs_physics::collision::TriangleGrid>) {
        // The draws in the original's order (audit 2026-09-26, `traffic.md` §34.2): every one of them
        // is taken whatever it is then used for — the port used to skip four.
        self.rng.rand16(); // 0x407820 steps the LCG once and discards it
        // 0x403000: `car+0xEC8 = pers[0xC + 4·(rand >> 8 & 3)]` — the personality's lane offset, all
        // zero in the traffic's row 0 (§23.10), but the draw is taken (0x403000…0x403019)
        let _lane_offset = PERSONALITIES[0].lane_offsets[(self.rng.rand16() & 3) as usize];

        // 0x40B330: the per-spawn speed factor
        let factor = [1.0f32, 0.9, 0.8, 0.7][(self.rng.rand16() & 3) as usize];

        // The purgatory's exit 0x40DFB0 ran first (0x406146 → `[vtable+4]`): `car+0xED4` = the chase
        // target that leads the race order — `0x5E8B88` walked down from the top, the first with
        // `car+0x52C & 0x100` (0x40E00F…0x40E048). The spawn's step 1 overwrites it at once.
        let targets = self.targets(reference);
        let order = Self::race_order(&targets);
        // 0x407290 step 1 (0x407293…0x4072D4): `car+0xED4 = 0x5E8B10[(rand16 · [0x5E8AB8]) >> 16]`, a
        // random chase target (the human alone unless `COPS`)
        let mut r = self.rng.below(targets.len() as u32) as usize;
        // step 2: the group basis direction — the coin is **always** drawn (0x4072DA…0x40733E), then
        // a reference faster than 30 m/s (`[0x5B2408]`) overrides it with its own `car+0xEB0`
        let coin = if self.rng.below(1000) >= 500 { 1.0 } else { -1.0 };
        let b = if targets[r].speed.abs() > 30.0 { targets[r].dir } else { coin };
        // step 3 (0x407360…0x407493): from the reference's place in the race order (`ref+0x528`)
        // step through `0x5E8B88` in direction `b`, wrapping, once round; a chase target other than
        // the car and the current `car+0xED4` becomes the reference while it lies within 360 m
        // (`[0x5B25CC]`, octagonal) of the slice 66 slices from the current reference in direction
        // `b`, and on that side of it (`0x40E6E0(o, ref) · b > 0`); the first that fails ends the walk.
        // Only the chase targets' cyclic order matters, the other cars being skipped.
        let m = order.len() as i32;
        let start = order.iter().position(|&k| k == r).unwrap_or(0) as i32;
        for k in 1..m {
            let o = order[(start + k * b as i32).rem_euclid(m) as usize];
            if o == r {
                continue;
            }
            let s2 = self.add_slices(table, targets[r].slice, 66 * b as i32);
            if oct_distance(table.slices[s2].centre, targets[o].position) > 360.0 {
                break;
            }
            if crate::lanes::gap_between(table, targets[o].slice, targets[r].slice) * b <= 0.0 {
                break;
            }
            r = o;
        }
        self.cars[i].reference = r;
        let reference = targets[r];
        // step 4: the car's own direction of travel — an independent coin flip, **always** drawn
        // (0x407499…0x4074F3), then overridden on a one-way road (0x4074F5…0x40750A)
        let coin = if self.rng.below(1000) >= 500 { 1.0 } else { -1.0 };
        let dir = if self.one_way { self.race_dir } else { coin };
        // step 5: the slice, 60..65 ahead or behind
        let gap = (SPAWN_GAP.0 + self.rng.below(SPAWN_GAP.1)) as f32;
        let mut slice = self.add_slices(table, reference.slice, (gap * b) as i32);
        if slice == reference.slice {
            slice = table.route[2.min(table.lap_len - 1)];
        }

        // 0x4075A0: the lane and the lateral target
        let sl = table.slices[slice];
        let lateral = if dir == 1.0 {
            let k = 1 + self.rng.below(sl.lanes_b.max(1));
            sl.lane_width_b * (k as f32 - 0.25)
        } else {
            let k = 1 + self.rng.below(sl.lanes_a.max(1));
            sl.lane_width_a * (0.5 - k as f32)
        };

        let target = target_speed(dir, self.race_dir, reference.speed, factor).abs();
        let (fwd, lat_axis, up) = axes(&sl, dir);
        let c = &mut self.cars[i];
        c.slice = slice;
        c.dir = dir;
        c.speed_factor = factor;
        c.lateral_target = lateral;
        c.lateral = lateral;
        c.lateral_rate = 0.0;
        c.target_speed = target * dir;
        c.speed = target * dir; // `car+0xEC0 = car+0xEB8` — a traffic car spawns already at speed
        c.accel_local = Vec3::ZERO;
        c.accel_world = Vec3::ZERO;
        match (grid, self.specs.clone()) {
            (Some(g), Some(specs)) if i < self.bodies.len() => {
                // 0x407690 (§9.6): `Car_placeAtSlice` 0x49B090 (car, slice, {car+0xEB4, 1.0, 0},
                // car+0xEB0) — the slice-local offset in the frame the tracker's patch normal
                // gives, then the body dropped by `BodyHeight` onto that patch
                // (`Vehicle::place_at_slice`); the velocity `|car+0xEC0| · car.forward` after it
                self.sync_in(i);
                let b = &mut self.bodies[i];
                b.rearm_after_reset();
                for w in &mut b.wheels {
                    *w = nfs_physics::vehicle::Wheel::default();
                }
                b.place_at_slice(g, &specs, sl.centre, sl.forward.normalize_or_zero(), Vec3::new(lateral, 1.0, 0.0), false, dir < 0.0);
                self.sync_out(i);
                let c = &mut self.cars[i];
                c.velocity = c.orientation.row(2) * target;
            }
            _ => {
                let c = &mut self.cars[i];
                c.position = sl.centre + lat_axis * lateral + up;
                c.orientation = heading(fwd, up);
                // `fwd` already carries the direction, so the magnitude goes in here
                c.velocity = fwd * target;
            }
        }
        let c = &mut self.cars[i];
        // 0x4077EA…0x4077FB: `car+0x1074 = car+0xEB4 = car+0xED0 = 0x4173C0(car)` — the lateral target
        // and the lateral are re-derived from where the placement actually put the body
        let lat = (c.position - sl.centre).dot(lat_axis);
        c.lateral = lat;
        c.lateral_target = lat;
        // 0x406CD0 (0x4077E4): the spin `car+0x388` zeroed, the gear `car+0x104C = 1` and its timers
        // `car+0x1050` / `car+0x1048` cleared (0x406CF9…0x406D2E) — `car+0x1080` is not touched. The
        // port kept a retired car's gear, so one that went to purgatory reversing came back reversing.
        c.angular_velocity = Vec3::ZERO;
        c.gear.sense = 1.0;
        c.gear.since = 0;
        c.gear.period = 0;
        // `car+0xAF8` keeps the purgatory's 0 until 0x4169B0 runs at Sc64, after this AI pass — so
        // the lane pass and the script triggers of the call that places the car see 0 m
        c.active = true;
        // `0x406CD0` clears the collision counter on every (re)spawn (§21.7)
        c.collisions = 0;
        // the purgatory's exit 0x40DFB0 stores `car+0x1100 = 0` (0x40DFBA)
        c.reset_countdown = 0;
        c.off_surface = false;
        c.cos_tilt = 1.0;
        tracing::debug!(car = i, slice, dir, lateral, lane = lane_of(&sl, lateral), speed = target, "traffic spawn");
    }
}

/// `0x40E9E0(car)`: `nSlicesLap · car+0xAF8 · 6.0` metres (`[0x628BC0]+4`, `[0x5B2460]`).
pub fn metres(table: &SliceTable, progress: f32) -> f32 {
    table.lap_len as f32 * progress * 6.0
}

/// Slice-local axes for a car travelling in `dir`: (forward, lateral, up).
fn axes(sl: &crate::race::Slice, dir: f32) -> (Vec3, Vec3, Vec3) {
    let fwd = (sl.forward * dir).normalize_or_zero();
    (fwd, sl.lateral.normalize_or_zero(), sl.up.normalize_or_zero())
}

/// Row-major rotation with the car's forward along `fwd` and its up as close to `up` as possible.
fn heading(fwd: Vec3, up: Vec3) -> Mat3 {
    let right = up.cross(fwd).normalize_or_zero();
    let up = fwd.cross(right).normalize_or_zero();
    Mat3::from_cols(right, up, fwd).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::{Section, Slice, SLICE_SPACING};

    fn ring(n: usize) -> SliceTable {
        let r = n as f32 * SLICE_SPACING / std::f32::consts::TAU;
        let slices = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let centre = Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                let forward = Vec3::new(-a.sin(), 0.0, a.cos());
                Slice {
                    centre,
                    up: Vec3::Y,
                    forward,
                    lateral: Vec3::Y.cross(forward),
                    lane_mask: 0,
                    lanes_a: 2,
                    lanes_b: 2,
                    width_a: 8.0,
                    width_b: 8.0,
                    lane_width_a: 4.0,
                    lane_width_b: 4.0,
                }
            })
            .collect();
        SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
    }

    fn models(n: usize) -> Vec<TrafficSpec> {
        (0..n).map(|i| TrafficSpec { model: i, bus: false, half_extents: Vec3::new(0.85, 0.6, 2.1) }).collect()
    }

    #[test]
    fn the_early_race_boost_follows_rule_3() {
        // §5.2 rule 3 / 0x40AEF0: only with more than one opponent, one human, inside the first 30 s
        // and while the human is behind the car
        assert_eq!(early_boost(5, 100, 1, 0.1, 0.5, 0), 2.0);
        assert_eq!(early_boost(1, 100, 1, 0.1, 0.5, 0), 1.0, "a single opponent");
        assert_eq!(early_boost(5, 0x780, 1, 0.1, 0.5, 0), 1.0, "after 30 s");
        assert_eq!(early_boost(5, 100, 2, 0.1, 0.5, 0), 1.0, "two humans");
        assert_eq!(early_boost(5, 100, 1, 0.9, 0.5, 0), 1.0, "the human is ahead");
        // the ordinal shrinks it; a traffic car's is 0, an opponent's is its place in the flag-8 loop
        assert!((early_boost(5, 100, 1, 0.1, 0.5, 3) - 1.4).abs() < 1e-6);
    }

    #[test]
    fn the_drive_gate_halves_the_rate_past_160_metres_from_a_racer() {
        // 0x414B10 row 1: inside 160 m of the nearest racer every 32 Hz tick drives the car, one
        // 32 Hz tick at a time; the distance is truncated, not rounded, so 159.9 m is still lod 9
        assert_eq!(drive_due(50.0, false, 1000, 7), Some((AI_DT, 1)));
        assert_eq!(drive_due(159.9, false, 1001, 7), Some((AI_DT, 1)));
        // beyond it every other 32 Hz tick, with twice the timestep and two ticks' worth of counters
        let far: Vec<_> = (1000..1016).filter_map(|t| drive_due(160.0, false, t, 7)).collect();
        assert_eq!(far.len(), 8, "eight of sixteen 64 Hz ticks");
        assert!(far.iter().all(|(dt, n)| (*dt - 1.0 / 16.0).abs() < 1e-6 && *n == 2));
        // the phase is the slot (`car+0 = slot | 0x100`): due when `slot + tick/2` is odd
        assert_eq!(drive_due(200.0, false, 1000, 7), Some((1.0 / 16.0, 2)));
        assert_eq!(drive_due(200.0, false, 1000, 6), None);
        assert_eq!(drive_due(200.0, false, 1002, 6), Some((1.0 / 16.0, 2)));
        // a racer's force flag is lod 0; the first 15 game ticks always pass
        assert_eq!(drive_due(5000.0, true, 1000, 6), Some((AI_DT, 1)));
        assert_eq!(drive_due(400.0, false, 14, 6), Some((AI_DT, 1)));
    }

    #[test]
    fn a_traffic_car_sits_on_the_ground_under_it() {
        use nfs_physics::collision::{Triangle, TriangleGrid};
        // a plane through the slice plane, rising 1 m over 10 m in +x
        let h = |x: f32| 0.1 * x;
        let tri = |a: Vec3, b: Vec3, c: Vec3| Triangle::new(a, b, c, 1);
        let (p0, p1, p2, p3) = (Vec3::new(-20.0, h(-20.0), -20.0), Vec3::new(20.0, h(20.0), -20.0), Vec3::new(20.0, h(20.0), 20.0), Vec3::new(-20.0, h(-20.0), 20.0));
        let grid = TriangleGrid::build(vec![tri(p0, p1, p2), tri(p0, p2, p3)], 10.0);

        let t = ring(400);
        let mut set = TrafficSet::new(&models(7), &t, 1.0, false, 1);
        // the `BndP` body points as `RCar::load` leaves them: the wheel quad `RIDE_HEIGHT` under
        // the model's ground, lowered by `car+0x3AC` (`half_extents.y`)
        let half = set.cars[0].half_extents;
        let quad = [(-0.8, 1.3), (0.8, 1.3), (-0.8, -1.3), (0.8, -1.3)].map(|(x, z)| Vec3::new(x, -(half.y + RIDE_HEIGHT), z));
        set.set_bodies(&crate::police::test_specs(), &[quad.to_vec()]);
        // put one car on the plane as the spawn does (`Car_placeAtSlice` 0x49B090 with the slice
        // centre at x = 5 and a 1 m lift); the spawner itself is exercised by the other tests
        let c = &mut set.cars[0];
        c.active = true;
        c.slice = 0;
        c.orientation = Mat3::IDENTITY;
        c.velocity = Vec3::ZERO;
        c.angular_velocity = Vec3::ZERO;
        set.sync_in(0);
        let specs = crate::police::test_specs();
        set.bodies[0].place_at_slice(&grid, &specs, Vec3::new(5.0, h(5.0), 0.0), Vec3::Z, Vec3::new(0.0, 1.0, 0.0), false, false);
        set.sync_out(0);
        // `Car_ground` at 32 Hz and the integrators at 64 Hz, until the body has settled
        for tick in 0..256u64 {
            if tick % 2 == 0 {
                set.ground(&grid, tick);
            }
            set.integrate_on(&t, 1.0 / 64.0, Some(&grid));
        }
        let c = &set.cars[0];
        // the origin `car+0x330` rests about `RIDE_HEIGHT + car+0x3AC` along the normal above the
        // plane, and the model is drawn with its ground plane on the surface
        let n = Vec3::new(-0.1, 1.0, 0.0).normalize();
        let d = c.draw_origin();
        let gap = (d - Vec3::new(d.x, h(d.x), d.z)).dot(n);
        assert!(gap.abs() < 0.05, "model ground on the plane: {gap} (origin {:?})", c.position);
        // the body leans with the surface instead of staying world-up
        let up = c.orientation.row(1);
        assert!(up.y > 0.9 && up.x.abs() + up.z.abs() > 0.02, "body up {up:?}");
        assert!(c.cos_tilt > 0.9 && !c.off_surface, "tilt {} off {}", c.cos_tilt, c.off_surface);
        // without a grid it falls back to the slice height, as it did before
        set.cars[0].position = Vec3::new(5.0, 0.0, 0.0);
        set.integrate_on(&t, 1.0 / 64.0, None);
        let c = &set.cars[0];
        assert!((c.position.y - (t.slices[c.slice].centre.y + c.rest_height())).abs() < 1e-3);
    }

    #[test]
    fn cars_spawn_around_the_reference_and_retire_behind_it() {
        let t = ring(400);
        let mut set = TrafficSet::new(&models(7), &t, 1.0, false, 12345);
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 20.0, dir: 1.0, honking: false };
        // nothing spawns before the race is under way
        set.update(&t, 100, r, &[]);
        assert_eq!(set.active_count(), 0);
        for tick in 961..3000 {
            set.update(&t, tick, r, &[]);
            set.integrate(&t, 1.0 / 64.0);
        }
        assert!(set.active_count() > 0 && set.active_count() <= MAX_ACTIVE);
        for c in set.cars.iter().filter(|c| c.active) {
            let d = c.slice as i32;
            let gap = d.min(400 - d);
            assert!(gap <= 70, "spawned {gap} slices from the reference and stayed near it");
            assert!(c.speed.abs() > 8.0, "cruising at {}", c.speed);
            assert!(c.lateral.abs() > 0.5 && c.lateral.abs() < 12.0, "lane offset {}", c.lateral);
            assert!((c.position - t.slices[c.slice].centre).length() < 14.0, "stays on the road");
        }
        // moving the reference to the far side of the ring retires every one of them; a car whose
        // purgatory delay has already expired may spawn again in the same tick, but near the new
        // reference, never at the old place. The AI pass's gate (`ai_pass_due`) runs each car on one
        // 32 Hz tick in four, so give it a whole cycle.
        let far = Reference { position: t.slices[200].centre, slice: 200, laps: 0, speed: 20.0, dir: 1.0, honking: false };
        for tick in (3000..3008).step_by(2) {
            set.update(&t, tick, far, &[]);
        }
        for c in &set.cars {
            if c.active {
                assert!(oct_distance(c.position, far.position) < DESPAWN_DISTANCE);
            }
        }
    }

    #[test]
    fn the_ai_pass_gate_is_8_hz_near_a_racer_and_4_hz_past_240_metres() {
        // 0x414B10 row 0: a traffic car in slot 5 inside 240 m is due on one even tick in four
        let due = |d: f32, force: bool, slot: usize| (1000u64..1064).step_by(2).filter_map(|t| ai_pass_due(d, force, t, slot)).collect::<Vec<_>>();
        assert_eq!(due(239.9, false, 5), vec![4; 8]);
        assert_eq!(due(240.0, false, 5), vec![8; 4]);
        // a racer's force flag maps to lod 0, which row 0 still makes 8 Hz
        assert_eq!(due(5000.0, true, 0), vec![4; 8]);
        // the phase is the slot: due when `(slot | 0x100) + tick/2 ≡ 3 (mod 4)`; 0x100 + 500 ≡ 0
        assert_eq!(ai_pass_due(0.0, false, 1000, 3), Some(4));
        assert_eq!(ai_pass_due(0.0, false, 1000, 2), None);
        // the first 15 game ticks always pass, covering one 32 Hz tick
        assert_eq!(ai_pass_due(1000.0, false, 14, 2), Some(1));
    }

    #[test]
    fn the_spawn_gate_is_asked_before_the_countdown_steps() {
        // 0x406020 state 1: the gate 0x40E060 tests `car+0xEE8 > 0` first, and only a car it turns
        // away runs the countdown 0x40E140 — so a delay that runs out is acted on at the next update
        let t = ring(400);
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 20.0, dir: 1.0, honking: false };
        set.cars[0].respawn_ticks = 3;
        // slot 0 is due when `0x100 + tick/2 ≡ 3 (mod 4)`: ticks 1030, 1038, …
        set.ai(&t, 1030, r, &[]);
        assert!(!set.cars[0].active);
        assert_eq!(set.cars[0].respawn_ticks, -1, "stepped by 4, with no floor");
        set.ai(&t, 1038, r, &[]);
        assert!(set.cars[0].active, "placed on the next update");
        // a retire takes the first countdown step in the same call (0x4061C3)
        let far = Reference { position: t.slices[200].centre, slice: 200, ..r };
        // the purgatory ctor's draw, `1 + rand_below(0x5CC4A0[1] = 50)`
        let armed = 1 + set.rng.clone().below(RESPAWN_TABLE[1]) as i32;
        set.ai(&t, 1046, far, &[]);
        assert!(!set.cars[0].active);
        assert_eq!(set.cars[0].respawn_ticks, armed - 4, "armed and stepped once");
        // before 0x3C0 nothing counts down
        let mut early = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        early.cars[0].respawn_ticks = 3;
        early.ai(&t, 0x3BE, r, &[]);
        assert_eq!(early.cars[0].respawn_ticks, 3);
    }

    #[test]
    fn a_traffic_cars_progress_is_the_bare_lap_fraction() {
        // 0x4169B0: `car+0xAF8 = frac(slice)` with no lap count, 0 while `car+0x7F == 0`; 0x40E9E0
        // turns it into metres from the lap's start
        let t = ring(400);
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 2, speed: 20.0, dir: 1.0, honking: false };
        set.cars[0].respawn_ticks = 0;
        set.ai(&t, 1030, r, &[]);
        assert!(set.cars[0].active);
        assert_eq!(set.cars[0].progress, 0.0, "the purgatory's 0 until the Sc64 pass");
        set.integrate(&t, 1.0 / 64.0);
        let c = &set.cars[0];
        assert_eq!(c.progress, t.frac(c.slice), "no laps, whatever the reference has done");
        assert!((metres(&t, c.progress) - t.route_pos[c.slice] as f32 * 6.0).abs() < 1e-2);
        // retiring it parks it at 0 again
        let far = Reference { position: t.slices[200].centre, slice: 200, ..r };
        set.ai(&t, 1038, far, &[]);
        assert!(!set.cars[0].active);
        assert_eq!(set.cars[0].progress, 0.0);
    }

    #[test]
    fn no_traffic_is_placed_while_the_player_is_in_a_tunnel_window() {
        // 0x40E0AD…0x40E134: the name's first four bytes `| 0x20202020`
        assert_eq!(no_spawn_windows("canyon"), &[(0x6B1, 0x6EA), (0x732, 0x756)]);
        assert_eq!(no_spawn_windows("Industrial").len(), 4);
        assert!(no_spawn_windows("coastal").is_empty());
        assert!(no_spawn_windows("ind").is_empty(), "a NUL | 0x20 is a space");
        let t = ring(2600);
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        set.set_track("canyon");
        let at = |slice: usize| Reference { position: t.slices[slice].centre, slice, laps: 0, speed: 20.0, dir: 1.0, honking: false };
        set.cars[0].respawn_ticks = 0;
        // slot 0 is due on ticks 1030, 1038, …; inside the window the gate says no without
        // re-arming, and the purgatory countdown goes on stepping
        set.ai(&t, 1030, at(0x6EA), &[]);
        assert!(!set.cars[0].active);
        assert_eq!(set.cars[0].respawn_ticks, -4);
        set.ai(&t, 1038, at(0x732), &[]);
        assert!(!set.cars[0].active);
        set.ai(&t, 1046, at(0x6EB), &[]);
        assert!(set.cars[0].active, "just past the first window");
    }

    #[test]
    fn the_box_sets_the_mass() {
        // the measured eas1 box, against the density that reproduces authored kerb masses
        let s = TrafficSpec { model: 0, bus: false, half_extents: Vec3::new(0.818, 0.545, 2.200) };
        assert!((s.mass() - 1130.0).abs() < 10.0, "{}", s.mass());
        // the Tbus box is four times heavier again
        let bus = TrafficSpec { model: 0, bus: true, half_extents: Vec3::new(1.013, 1.174, 4.227) };
        assert!((bus.mass() - 5791.0).abs() < 30.0, "{}", bus.mass());
    }

    #[test]
    fn one_way_roads_have_no_oncoming_traffic() {
        let mut t = ring(200);
        for s in &mut t.slices {
            s.lanes_a = 0;
        }
        let mut set = TrafficSet::new(&models(6), &t, 1.0, false, 7);
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 10.0, dir: 1.0, honking: false };
        for tick in 961..2000 {
            set.update(&t, tick, r, &[]);
            set.integrate(&t, 1.0 / 64.0);
        }
        assert!(set.active_count() > 0);
        assert!(set.cars.iter().filter(|c| c.active).all(|c| c.dir > 0.0));
    }

    /// `[0x5E7544]` is slice 1's lanesA in the track's own frame, so a backwards race on the same
    /// road keeps it: the reversed table carries that count in `lanes_b`.
    #[test]
    fn the_one_way_flag_survives_a_backwards_race() {
        let mut t = ring(200);
        t.slices[1].lanes_a = 0;
        assert!(TrafficSet::new(&models(2), &t, 1.0, false, 7).one_way);
        assert!(TrafficSet::new(&models(2), &t.reversed(), 1.0, false, 7).one_way);
        t.slices[1].lanes_a = 2;
        t.slices[1].lanes_b = 0;
        assert!(!TrafficSet::new(&models(2), &t, 1.0, false, 7).one_way);
        assert!(!TrafficSet::new(&models(2), &t.reversed(), 1.0, false, 7).one_way);
    }

    #[test]
    fn both_directions_appear_on_a_two_way_road() {
        let t = ring(600);
        let mut set = TrafficSet::new(&models(7), &t, 1.0, false, 99);
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 10.0, dir: 1.0, honking: false };
        let (mut same, mut onc) = (0, 0);
        for tick in 961..6000 {
            set.update(&t, tick, r, &[]);
            set.integrate(&t, 1.0 / 64.0);
            for c in set.cars.iter().filter(|c| c.active) {
                if c.dir > 0.0 {
                    same += 1;
                } else {
                    onc += 1;
                }
            }
        }
        assert!(same > 0 && onc > 0, "same={same} oncoming={onc}");
        // an oncoming car drives on the negative side, a same-direction car on the positive side
        for c in set.cars.iter().filter(|c| c.active) {
            assert_eq!(c.lateral_target > 0.0, c.dir > 0.0);
        }
    }

    #[test]
    fn linear_tracks_retire_cars_at_both_ends() {
        let t = ring(120);
        let mut set = TrafficSet::new(&models(3), &t, 1.0, true, 5);
        let r = Reference { position: t.slices[3].centre, slice: 3, laps: 0, speed: 5.0, dir: 1.0, honking: false };
        for tick in 961..1400 {
            set.update(&t, tick, r, &[]);
            set.integrate(&t, 1.0 / 64.0);
        }
        for c in set.cars.iter().filter(|c| c.active) {
            let p = t.route_pos[c.slice] as i32;
            assert!(p > 5 && p < 114, "slice {p} is too near an end of a linear track");
        }
    }

    /// Places car 0 on slice 10 of a ring, driving along the route, as `spawn` would.
    #[test]
    fn a_cop_within_75_metres_stops_most_traffic() {
        // 0x406020 state 2 / 0x405ED0: a moving cop 30 m away; the roll `(rand16·10) >> 16` stops
        // one car in ten dead, pulls seven over to `laneWidthB · lanesB` and lets two drive on
        let t = ring(300);
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 10.0, dir: 1.0, honking: false };
        let cop = |slice: usize, speed: f32, has_target: bool| CopView { slice, speed, in_world: true, has_target };
        let (mut dead, mut pulled, mut ignored) = (0, 0, 0);
        for seed in 0..400 {
            let mut set = TrafficSet::new(&models(1), &t, 1.0, false, seed);
            placed(&mut set, &t, 10);
            set.cars[0].racer_distance = 100.0;
            set.cops = vec![cop(15, 12.0, false)];
            set.ai(&t, 1030, r, &[]);
            let c = &set.cars[0];
            match c.stopped {
                Some(StopMode::Dead) => {
                    dead += 1;
                    assert_eq!(c.target_speed, 0.0);
                    assert_eq!(c.lateral_target, c.lateral);
                }
                Some(StopMode::PullOver(x)) => {
                    pulled += 1;
                    assert_eq!(x, 4.0 * 2.0, "laneWidthB · lanesB");
                    assert_eq!(c.lateral_target, x);
                    // more than a metre off: 0x40AF70's speed at a quarter
                    assert!((c.target_speed - target_speed(1.0, 1.0, 10.0, c.speed_factor) * 0.25).abs() < 1e-5);
                }
                None => {
                    assert!(c.ignore_cops);
                    ignored += 1;
                }
            }
        }
        assert!(dead > 20 && dead < 70, "{dead}");
        assert!(pulled > 240 && pulled < 320, "{pulled}");
        assert!(ignored > 50 && ignored < 110, "{ignored}");

        // no cop near: a stopped car drives on in the same call, and the latch clears
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 1);
        placed(&mut set, &t, 10);
        set.cars[0].stopped = Some(StopMode::Dead);
        set.cars[0].ignore_cops = true;
        set.ai(&t, 1030, r, &[]);
        assert_eq!(set.cars[0].stopped, None);
        assert!(set.cars[0].target_speed > 0.0);
        set.ai(&t, 1038, r, &[]);
        assert!(!set.cars[0].ignore_cops);

        // which cops count: 75 m, and a cop at 2 m/s or slower only with a perp claimed
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 1);
        placed(&mut set, &t, 10);
        for (c, near) in [
            (cop(22, 12.0, false), true),
            (cop(23, 12.0, false), false),
            (cop(12, 1.0, false), false),
            (cop(12, 1.0, true), true),
            (cop(12, 2.0, true), false),
            (CopView { in_world: false, ..cop(12, 12.0, false) }, false),
        ] {
            set.cops = vec![c];
            assert_eq!(set.cop_near(0, &t), near, "{c:?}");
        }
        // the nearest cop decides, even when it does not count
        set.cops = vec![cop(20, 12.0, false), cop(12, 1.0, false)];
        assert!(!set.cop_near(0, &t));
    }

    #[test]
    fn the_reference_is_a_chase_target_the_spawn_walks_to_the_front_and_the_retire_rebinds() {
        // `COPS`: the opponents are chase targets too (0x40F336). 0x407290 moves the reference to the
        // chase target ahead in direction `b` while it is within 360 m of the slice 66 on
        let t = ring(400);
        let human = ChaseTarget { position: t.slices[0].centre, slice: 0, speed: 35.0, dir: 1.0, place: 0 };
        let opp = ChaseTarget { position: t.slices[50].centre, slice: 50, speed: 35.0, dir: 1.0, place: 1 };
        let r = Reference { position: human.position, slice: 0, laps: 0, speed: 35.0, dir: 1.0, honking: false };
        for seed in 0..40 {
            let mut set = TrafficSet::new(&models(1), &t, 1.0, false, seed);
            set.chase_targets = vec![human, opp];
            set.spawn(0, &t, r, None);
            let c = &set.cars[0];
            assert_eq!(c.reference, 1, "the walk ends on the opponent ahead");
            assert!((110..=115).contains(&c.slice), "60..65 slices past the opponent: {}", c.slice);
        }
        // 0x407030: past 400 m from its reference the car rebinds to the first chase target within
        // 400 m and stays; from none of them, it retires
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 1);
        set.chase_targets = vec![human, ChaseTarget { position: t.slices[200].centre, slice: 200, ..opp }];
        placed(&mut set, &t, 205);
        set.cars[0].reference = 0;
        assert!(!set.should_retire(0, &t, r));
        assert_eq!(set.cars[0].reference, 1);
        set.chase_targets[1] = ChaseTarget { position: t.slices[100].centre, slice: 100, ..opp };
        assert!(set.should_retire(0, &t, r));
    }

    #[test]
    fn triggers_4_and_5_are_the_order_swaps_of_0x417710_not_collisions() {
        // 0x417783…0x4177F9: both cars of a swap record each other (+0x550) and the tick (+0x554)
        let t = ring(600);
        let mut set = TrafficSet::new(&models(2), &t, 1.0, false, 3);
        set.overtakes(&[0.30, 0.40, 0.35], 100, 1);
        assert_eq!(set.overtake_order, vec![0, 2, 1]);
        assert_eq!(set.swap_partner, vec![None, Some(2), Some(1)]);
        // the human (key 0.45) passes both traffic cars in one sort: it bubbles up past each, and
        // its last partner is the higher of them
        set.overtakes(&[0.45, 0.40, 0.35], 102, 1);
        assert_eq!(set.overtake_order, vec![2, 1, 0]);
        assert_eq!(set.swap_partner[0], Some(1));
        assert_eq!(set.swap_tick[2], Some(102));

        // the trigger: script 4 of row 0 made visible, the car past 800 m of the lap, the human's
        // last swap with it inside 15 ticks — and no collision at all
        set.scripts.table[4 * 8] = nfs_formats::scripts::action::HONK_HORN;
        set.scripts.table[4 * 8 + 4] = 2;
        set.scripts.table[5 * 8] = nfs_formats::scripts::action::HONK_HORN;
        set.scripts.table[5 * 8 + 4] = 2;
        placed(&mut set, &t, 200);
        set.cars[0].progress = t.frac(200);
        set.cars[0].racer_distance = 24.0;
        let r = Reference { position: t.slices[260].centre, slice: 260, laps: 0, speed: 10.0, dir: 1.0, honking: false };
        let human = LaneCar { slice: 260, lane: 8, lane_mask: lane_bit_for(8), speed: 10.0, speed_est: 10.0, half_len: 2.1, lateral: 6.0, half_span: 0.9, is_human: true, is_racer: true };
        set.ai(&t, 110, r, &[human]);
        set.ai(&t, 118, r, &[human]);
        assert!(set.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_some(), "an overtake within 15 ticks");
        // 15 ticks on, nothing new
        let mut set2 = TrafficSet::new(&models(2), &t, 1.0, false, 3);
        set2.scripts = set.scripts.clone();
        set2.overtakes(&[0.30, 0.40, 0.35], 100, 1);
        set2.overtakes(&[0.45, 0.40, 0.35], 102, 1);
        placed(&mut set2, &t, 200);
        set2.cars[0].progress = t.frac(200);
        set2.cars[0].racer_distance = 24.0;
        set2.ai(&t, 126, r, &[human]);
        set2.ai(&t, 134, r, &[human]);
        assert!(set2.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_none());
    }

    fn lane_bit_for(l: i32) -> u32 {
        1 << l
    }

    fn placed(set: &mut TrafficSet, t: &SliceTable, slice: usize) {
        let sl = t.slices[slice];
        let (fwd, _, up) = axes(&sl, 1.0);
        let c = &mut set.cars[0];
        c.active = true;
        c.slice = slice;
        c.dir = 1.0;
        c.position = sl.centre + up * c.rest_height();
        c.orientation = heading(fwd, up);
        c.velocity = fwd * 10.0;
        c.speed = 10.0;
        c.target_speed = 10.0;
        c.alive = true;
    }

    #[test]
    fn the_crash_countdown_runs_out_only_for_a_dead_or_tilted_car() {
        // 0x40FB13…0x40FBA2: an upright, alive car clears the countdown at once; a dead one
        // counts it down, nine a tick off the surface, and is reset in place at zero
        let t = ring(300);
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        placed(&mut set, &t, 10);
        set.cars[0].reset_countdown = 0xC0;
        set.post_impact(&t, None, &[]);
        assert_eq!(set.cars[0].reset_countdown, 0, "upright and alive: cleared");
        assert_eq!(set.cars[0].resets, 0);
        // `Damage` past 120 killed it: 0xC0 ticks to the reset on the surface
        set.cars[0].alive = false;
        set.cars[0].reset_countdown = 0xC0;
        for _ in 0..0xBF {
            set.post_impact(&t, None, &[]);
        }
        assert_eq!(set.cars[0].reset_countdown, 1);
        assert_eq!(set.cars[0].resets, 0);
        set.post_impact(&t, None, &[]);
        assert_eq!(set.cars[0].resets, 1, "reset at zero");
        assert!(set.cars[0].alive, "0x49E220 re-arms car+0xDB0");
        assert_eq!(set.cars[0].velocity, Vec3::ZERO);
        assert_eq!(set.cars[0].reset_countdown, 0);
        // off the surface the countdown loses nine a tick and 0x40FBA2 re-arms it at once
        set.cars[0].alive = false;
        set.cars[0].off_surface = true;
        set.post_impact(&t, None, &[]);
        assert_eq!(set.cars[0].reset_countdown, 0xC0, "armed by the off-surface test");
        set.post_impact(&t, None, &[]);
        assert_eq!(set.cars[0].reset_countdown, 0xC0 - 9);
    }

    #[test]
    fn a_reset_takes_the_next_free_slice_along_the_route() {
        // 0x40F480 walks the live cars for `+8 == s`; 0x4817D0 steps one slice on
        let t = ring(300);
        let mut set = TrafficSet::new(&models(2), &t, 1.0, false, 3);
        placed(&mut set, &t, 10);
        set.cars[1].active = true;
        set.cars[1].slice = 11;
        set.reset_to_slice(0, &t, None, &[10, 12], 1);
        assert_eq!(set.cars[0].slice, 13, "10 and 12 are racers', 11 is the other traffic car's");
        assert_eq!(set.cars[0].resets, 1);
        assert_eq!(set.cars[0].collisions, 0, "0x40F400 clears car+0x418");
        let sl = &t.slices[13];
        assert!((set.cars[0].position - sl.centre).dot(sl.lateral.normalize()).abs() < 1e-4, "on the centre line");
        assert!(set.cars[0].orientation.row(2).dot(sl.forward.normalize()) > 0.999, "heading along the slice");
        // an oncoming car is placed facing the other way (`car+0xEAC`, the flip of 0x49B0B9)
        set.cars[0].dir = -1.0;
        set.reset_to_slice(0, &t, None, &[], 0);
        assert!(set.cars[0].orientation.row(2).dot(t.slices[13].forward.normalize()) < -0.999);
    }

    #[test]
    fn the_insane_net_resets_in_place_instead_of_retiring() {
        // 0x408750 at the end of the driven step: 35 m past the slice's `+0x40` width, or a
        // velocity component past 115 (signed — a car flung backwards is caught by `+0x35C`)
        let t = ring(300);
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        placed(&mut set, &t, 10);
        set.cars[0].lateral = -8.0 - 35.5;
        assert!(set.is_out_of_bounds(0, &t));
        set.cars[0].lateral = 8.0 + 34.5;
        assert!(!set.is_out_of_bounds(0, &t));
        set.cars[0].lateral = 0.0;
        set.cars[0].velocity = Vec3::new(-200.0, 0.0, 0.0);
        assert!(set.is_out_of_bounds(0, &t), "the octagonal speed catches the sign the axis test misses");
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 10.0, dir: 1.0, honking: false };
        set.cars[0].racer_distance = 50.0;
        set.drive(&t, 1000, r, None, &[]);
        assert!(set.cars[0].active, "still in the world");
        assert_eq!(set.cars[0].resets, 1);
        assert_eq!(set.cars[0].velocity, Vec3::ZERO);
    }

    #[test]
    fn the_human_honk_asks_for_script_6_within_thirty_metres() {
        // 0x401713…0x40174E: `d < 30`, `human+0xD86`, `[0x657A30] == 0`; past the 800 m gate
        let t = ring(600);
        let mut set = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        // a trigger 6 script that does something, so the runner shows the request: row 0's is
        // `END` in the shipped file, which the runner drops at once
        set.scripts.table[6 * 8] = nfs_formats::scripts::action::HONK_HORN;
        set.scripts.table[6 * 8 + 4] = 2;
        placed(&mut set, &t, 200);
        set.cars[0].progress = t.frac(200);
        let human = |slice: usize| {
            let sl = &t.slices[slice];
            LaneCar { slice, lane: lane_of(sl, 2.0), lane_mask: lane_span(sl, 2.0, 0.9), speed: 10.0, speed_est: 10.0, half_len: 2.1, lateral: 2.0, half_span: 0.9, is_human: true, is_racer: true }
        };
        let r = |honking: bool| Reference { position: t.slices[196].centre, slice: 196, laps: 0, speed: 10.0, dir: 1.0, honking };
        set.cars[0].racer_distance = 24.0;
        set.ai(&t, 1030, r(false), &[human(196)]);
        assert!(set.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_none(), "no horn, no request");
        set.ai(&t, 1038, r(true), &[human(196)]);
        // 0x40B4D0 steps the runner before the update that raises the request, so the script
        // starts on the car's next AI update
        assert!(set.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_none(), "requested, not yet started");
        set.ai(&t, 1046, r(false), &[human(196)]);
        assert!(set.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_some(), "the honk within 24 m");
        // `TIME` set: never
        let mut set2 = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        set2.scripts = set.scripts.clone();
        set2.time_mode = true;
        placed(&mut set2, &t, 200);
        set2.cars[0].progress = t.frac(200);
        set2.cars[0].racer_distance = 24.0;
        set2.ai(&t, 1030, r(true), &[human(196)]);
        set2.ai(&t, 1038, r(true), &[human(196)]);
        assert!(set2.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_none());
        // 36 m away: not
        let mut set3 = TrafficSet::new(&models(1), &t, 1.0, false, 3);
        set3.scripts = set.scripts.clone();
        placed(&mut set3, &t, 200);
        set3.cars[0].progress = t.frac(200);
        set3.cars[0].racer_distance = 36.0;
        let far = Reference { position: t.slices[194].centre, slice: 194, laps: 0, speed: 10.0, dir: 1.0, honking: true };
        set3.ai(&t, 1030, far, &[human(194)]);
        set3.ai(&t, 1038, far, &[human(194)]);
        assert!(set3.cars[0].script.query(nfs_formats::scripts::mask::HONK_HORN).is_none());
    }
}
