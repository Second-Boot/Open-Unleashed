//! Traffic cars — ported from `RE/notes/traffic.md` (§9 placement, §10 despawn, §13 init,
//! §14 motion, §17.6 target speed) plus `0x408D20` read directly for the lateral controller.
//!
//! Traffic in this game is **kinematic**: the AI writes the velocity vector outright every AI tick
//! and zeroes the angular velocity (`Traffic_driveKinematic` 0x408840); the shared 64 Hz integrator
//! `0x49B7A0` then moves the car. There is no tyre or engine model behind a traffic car.
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
//! Not ported: police, which `crate::police` carries instead (§17). Three things that look like gaps
//! are not: the AI level-of-detail gates of §14.3 / §17.5e **are** modelled ([`ai_pass_due`] for the
//! AI pass, [`drive_due`] for the drive), the
//! personality lane offset `car+0xEC8` is always zero for traffic (it draws personality row 0), and
//! the knocked-over hand-back to the rigid body (`car+0x42C == 1`, 0x4084C3) is dead code for traffic
//! (§21.7) — what replaces it, the two damping exits, is here.
//!
//! Gap: the crash countdown `car+0x1100` that `Car_postImpact` 0x410280 → 0x40F740 runs for every car,
//! traffic included (`physics.md` §4.1c: armed to 0xC0 by an impact past 80 or by lying on the roof /
//! off the drivable surface, a reset 0x40F540 when it runs out), is not run for a traffic car; the
//! port's traffic has the 0x408750 net and the §21.7 exits only. The purgatory's clears of it
//! (0x40DFBA, 0x40E145) therefore have nothing to clear here.

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
/// `× 0.9` under `WEATHER`, `× car+0x1028` for a cop (also a compile-time 1.0), `× 1.5` while the
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
/// on the plane. APPROXIMATION in what is left: the port's kinematic traffic has no per-wheel
/// contacts, so none of `Susp_update`'s pitch / roll or per-wheel roughness.
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
    /// `WEATHER` (`[0x657A24] > 0`): the longitudinal rate is scaled by 0.9 (`ai.md` §5.2.3). It is
    /// 0 in the shipped options, so this is off in every normal race.
    /// `[0x657A24]`, which the option registry names **`WEATHER`** (id 0x1E, record 0x5D1F90) —
    /// *not* the per-track night flag `word[0x628A28]`. The shipped `fe.txt` and `fe_out.txt` both
    /// carry `weather = 0`, so in a normal race this is **false and the ×0.9 never applies**; the
    /// port used to drive it from the track's night flag and slowed traffic on every night track.
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
        Self { cars, opponents: 0, cops_race: false, weather: false, scripts: nfs_formats::scripts::Scripts::default(), rng, one_way, race_dir, linear, cfg: nfs_formats::trafcfg::TrafficConfig::default(), phys: TrafficPhysics::default(), slot_base: 0, no_spawn: &[] }
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
        self.drive(table, tick, reference);
    }

    /// The AI half of a tick: `AIHigh_Traffic::update` 0x406020 for every car in index order, which
    /// `0x406720` runs from `GameTick` 0x4150A9 on an even tick — purgatory and the spawn (§9, §11),
    /// the retire tests, and `AIState_Normal::update` 0x40B5A0's target speed, scripts and lane pass
    /// (§12, §23), which write only the targets `car+0xEB4` / `car+0xEB8`. `tick` is the 64 Hz game
    /// tick; `others` are the cars the lane scorers must see that this set does not own.
    ///
    /// Each car's update is thinned by the AI pass's own gate, [`ai_pass_due`] — 8 Hz inside 240 m
    /// of the nearest racer, 4 Hz beyond — and its counters step by the `[0x5E6654]` it returns.
    pub fn ai(&mut self, table: &SliceTable, tick: u64, reference: Reference, others: &[LaneCar]) {
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
                        self.spawn(i, table, reference);
                        spawned = true;
                    }
                }
                if !spawned {
                    // 0x406460 → 0x40B4D0 → the purgatory update 0x40E140
                    self.purgatory_countdown(i, tick, ai_ticks);
                    continue;
                }
            } else if self.should_retire(i, table, reference) || self.is_out_of_bounds(i, table) {
                // state 2 (0x40617A): `0x407030` says retire → a new purgatory (0x40DE10, which
                // arms the delay) and 0x40B4D0 runs its update at once (0x4061C3): the countdown
                // takes its first step in the same call
                self.retire(i);
                self.purgatory_countdown(i, tick, ai_ticks);
                continue;
            }
            // §14.4 longitudinal: step toward the target speed, never overshooting
            let (dir, factor) = (self.cars[i].dir, self.cars[i].speed_factor);
            let target = target_speed(dir, self.race_dir, reference.speed, factor).abs();
            // §12 / §23: score the three candidate lanes, move to the best one and cap the target
            // speed for the change. A traffic car's personality is row 0 (§23.10), whose lane offsets
            // are all zero, so there is no in-lane swerve to add.
            // in registration order (0x40F189): the caller's human, opponents and cops, then this
            // set's cars — the order `find_blockers`' race-order walk keeps for a tie
            let mut cars: Vec<LaneCar> = others.to_vec();
            cars.extend(self.cars.iter().enumerate().filter(|(j, o)| *j != i && o.active).map(|(_, o)| o.as_lane_car(table)));
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
            // §12.9: ask for a script when the human does something worth reacting to, then step the
            // runner. `0x401580` returns at once while `0x40E9E0(car) < 800` ([0x5B2414], 0x4015A8):
            // a traffic car on the first 800 m of the lap (its `+0xAF8` being the bare fraction), and
            // on the call that places it, whose `+0xAF8` is still the purgatory's 0.
            {
                let travelled = me.travelled;
                // `0x40E9E0(human)`: the player's `+0xAF8` counts its laps
                let human_m = metres(table, reference.laps as f32 + table.frac(reference.slice));
                let (my_lane, my_dir, my_speed, my_slice) = (me.lane, me.dir, me.speed, me.slice);
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
                        // the contact pair of triggers: the original tests the human's own
                        // `+0x550`/`car+0x554` bookkeeping, which is the same collision the port
                        // timestamps in `hit_by_human_tick`; 4 when `0x40E9E0(human) <
                        // 0x40E9E0(car)`, else 5 (0x4016D2…0x4016FB)
                        if tick.saturating_sub(hit_tick) < 15 {
                            requests.push((hi as i32, if human_m < travelled { Trigger::OpponentPassesHuman } else { Trigger::HumanPassesOpponent }));
                        }
                    }
                }
                let scripts = std::mem::take(&mut self.scripts);
                let c = &mut self.cars[i];
                for (target, trigger) in requests {
                    c.script.request(target, trigger, tick as i64);
                }
                // 0x40B4D0 steps the runner only while `word car+0x41A == 0` (0x40B4E6): always —
                // the only store to it is `RigidBody_init`'s 0 (0x49B714), no wider store covers it
                c.script.step(&scripts, ai_ticks);
                self.scripts = scripts;
            }

            let mut sc = LaneScratch::default();
            let one_way = self.one_way;
            let mut scan = my_scan;
            sc.find_blockers(table, &me, &cars, &PERSONALITIES[0]);
            sc.score_own_side(&me, one_way, true);
            // §12.3's script reactions push the same lane scores the scorers do
            {
                let r = &self.cars[i].script;
                if let Some(t) = r.query(nfs_formats::scripts::mask::SWERVE)
                    && let Some(o) = cars.get(t.max(0) as usize)
                    && crate::lanes::gap_between(table, o.slice, me.slice).abs() < 12.0
                {
                    // a deterministic 1 Hz weave off the script's own timer, not a random draw
                    if (r.timer() / 32) & 1 == 1 {
                        sc.score[0] += 30.0;
                    } else {
                        sc.score[2] += 30.0;
                    }
                }
                if let Some(t) = r.query(nfs_formats::scripts::mask::SWIPE)
                    && let Some(o) = cars.get(t.max(0) as usize)
                    && crate::lanes::gap_between(table, o.slice, me.slice) * me.dir < 4.0
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

    /// `AIState_Purgatory::update` 0x40E140: for a car without the cop bit, once `gGameTick ≥
    /// 0x3C0` (0x40E160 `cmp`, `jl` skips), `car+0xEE8 −= [0x5E6654]` — the AI pass's step. There is
    /// no floor: the spawn gate 0x40E060 only asks `car+0xEE8 > 0`.
    ///
    /// It first stores `car+0x1100 = car+0x43C = 0` (0x40E145 / 0x40E14E), as the purgatory's exit
    /// 0x40DFB0 does (0x40DFBA / 0x40DFC7). Neither has an effect in the port: `car+0x43C` is only
    /// ever stored 0 (0x40DFC7, 0x40E14E, 0x40F45B, 0x410D0A, 0x49B6F2) and 0x49900E only decrements
    /// it while positive, so 0x498DE0's bail on it (0x498DE9) never fires — dead; `car+0x1100` is the
    /// crash countdown the port does not run for traffic (module docs).
    fn purgatory_countdown(&mut self, i: usize, tick: u64, ai_ticks: i32) {
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
    pub fn drive(&mut self, table: &SliceTable, tick: u64, reference: Reference) {
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
            c.lateral_rate = c.velocity.dot(sl.lateral.normalize_or_zero());
        }
    }

    /// The shared 64 Hz integrators (`0x49B7A0` position, `0x49B8E0` orientation) plus the
    /// slice/lateral tracking of `0x49AF70`.
    pub fn integrate(&mut self, table: &SliceTable, dt: f32) {
        self.integrate_on(table, dt, None)
    }

    /// The same step with the track's collision grid, so the cars sit on the ground the way the
    /// original's do: `Car_installHandlers` 0x410D60 replaces a traffic car's drive routine but
    /// leaves `car+0xE20 = Car_ground` 0x49BD40 in place, and §4.2's resting branch puts the body
    /// on the surface (`pos.y −= 0x424`) and turns the physics frame onto the ground normal
    /// (`0x49BA30`). The port does the body half of that — it has no per-wheel suspension for a
    /// traffic car — and falls back to the slice height where the ray misses.
    pub fn integrate_on(&mut self, table: &SliceTable, dt: f32, ground: Option<&nfs_physics::collision::TriangleGrid>) {
        for c in self.cars.iter_mut().filter(|c| c.active) {
            c.position += c.velocity * dt;
            // yaw about the world up, the angular velocity being in turns per second
            let yaw = c.angular_velocity.y * dt * std::f32::consts::TAU;
            if yaw != 0.0 {
                c.orientation = (Mat3::from_rotation_y(yaw) * c.orientation.transpose()).transpose();
            }
            c.slice = table.find_slice(c.position, c.slice);
            // `car+0xAF8`, 0x4169B0's traffic arm: the bare lap fraction of the slice. The port's
            // `frac` is the slice's place on the lap route of the (already reversed) table, as for
            // the racers. APPROXIMATION: the original writes it on even ticks only, from the slice
            // before this tick's integration (priority 0x19 runs before the integrators' 0x1E); the
            // port every 64 Hz tick, after the move.
            c.progress = table.frac(c.slice);
            let sl = &table.slices[c.slice];
            let lat_axis = sl.lateral.normalize_or_zero();
            // `car+0xED0 = 0x4173C0(car)` — the lateral offset re-derived from the world position, and
            // `car+0xEC0` the speed along the route (`0x417800`)
            c.lateral = (c.position - sl.centre).dot(lat_axis);
            c.speed = c.velocity.dot(sl.forward.normalize_or_zero());
            // ground contact: the highest surface under the car within 6 m of 2 m above it
            // (from the model's ground plane: the probe keeps its old reach below the model)
            let base = c.position.y - c.half_extents.y;
            let rest = c.rest_height();
            match ground.and_then(|g| g.ground(c.position.x, base + 2.0, c.position.z, 6.0)) {
                Some(h) => {
                    c.position.y = h.point.y + rest / h.normal.normalize_or_zero().y.max(0.1);
                    // keep the heading, take the up axis from the ground normal; the rows of
                    // `orientation` are the body axes, as everywhere else in the port
                    let up = h.normal.normalize_or_zero();
                    let f0 = c.orientation.row(2);
                    let fwd = (f0 - up * f0.dot(up)).normalize_or_zero();
                    let right = up.cross(fwd).normalize_or_zero();
                    if fwd.length_squared() > 0.5 && right.length_squared() > 0.5 {
                        c.orientation = Mat3::from_cols(right, fwd.cross(right).normalize_or_zero(), fwd).transpose();
                    }
                }
                None => c.position.y = sl.centre.y + rest,
            }
            c.velocity.y = 0.0;
        }
    }

    /// `0x408750` — the sanity net at the end of every driving tick: a car thrown by a collision past
    /// 115 m/s on any axis, or more than 35 m outside the road, is put back on the track (the original
    /// calls the respawn `0x40F540`; the port sends it back to purgatory, which respawns it properly).
    fn is_out_of_bounds(&self, i: usize, table: &SliceTable) -> bool {
        let c = &self.cars[i];
        if c.velocity.x.abs() > 115.0 || c.velocity.z.abs() > 115.0 || c.speed_estimate() > 115.0 {
            return true;
        }
        let sl = &table.slices[c.slice];
        let half = if c.lateral >= 0.0 { sl.width_b } else { sl.width_a };
        c.lateral.abs() - half > 35.0
    }

    /// `0x407030` — 400 m from the reference in the octagonal metric, or within 5 slices of either
    /// end of a linear track (§10).
    fn should_retire(&self, i: usize, table: &SliceTable, reference: Reference) -> bool {
        let c = &self.cars[i];
        if self.linear {
            let p = table.route_pos.get(c.slice).copied().unwrap_or(0) as i32;
            if p <= 5 || (table.lap_len as i32 - 1 - p) <= 5 {
                return true;
            }
        }
        oct_distance(c.position, reference.position) > DESPAWN_DISTANCE
    }

    fn retire(&mut self, i: usize) {
        let delay = RESPAWN_TABLE[self.cars.len().min(RESPAWN_TABLE.len() - 1)];
        let bus = self.cars[i].bus;
        let d = (1 + self.rng.below(delay) as i32) * if bus { 5 } else { 1 };
        let c = &mut self.cars[i];
        c.active = false;
        // `car+0x7F = 0` (0x40DE60), so 0x4169B0 stores `car+0xAF8 = 0` (0x416AAC)
        c.progress = 0.0;
        c.velocity = Vec3::ZERO;
        c.speed = 0.0;
        c.respawn_ticks = d;
        tracing::debug!(car = i, delay = d, "traffic retire");
    }

    /// `Traffic_spawn` 0x407820 (§9.3–§9.6), in the original's order.
    fn spawn(&mut self, i: usize, table: &SliceTable, reference: Reference) {
        self.rng.rand16(); // 0x407820 steps the LCG once and discards it

        // 0x40B330: the per-spawn speed factor
        let factor = [1.0f32, 0.9, 0.8, 0.7][(self.rng.rand16() & 3) as usize];

        // 0x407290 step 2: the group basis direction, a fair coin unless the reference is fast
        let b = if reference.speed.abs() > 30.0 {
            reference.dir
        } else if self.rng.below(1000) >= 500 {
            1.0
        } else {
            -1.0
        };
        // step 4: the car's own direction of travel — an independent coin flip
        let dir = if self.one_way {
            self.race_dir
        } else if self.rng.below(1000) >= 500 {
            1.0
        } else {
            -1.0
        };
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
        c.position = sl.centre + lat_axis * lateral + up;
        c.orientation = heading(fwd, up);
        // `fwd` already carries the direction, so the magnitude goes in here
        c.velocity = fwd * target;
        // `car+0xAF8` keeps the purgatory's 0 until 0x4169B0 runs at Sc64, after this AI pass — so
        // the lane pass and the script triggers of the call that places the car see 0 m
        c.active = true;
        // `0x406CD0` clears the collision counter on every (re)spawn (§21.7)
        c.collisions = 0;
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
        let tri = |a: Vec3, b: Vec3, c: Vec3| Triangle::new(a, b, c, 0);
        let (p0, p1, p2, p3) = (Vec3::new(-20.0, h(-20.0), -20.0), Vec3::new(20.0, h(20.0), -20.0), Vec3::new(20.0, h(20.0), 20.0), Vec3::new(-20.0, h(-20.0), 20.0));
        let grid = TriangleGrid::build(vec![tri(p0, p1, p2), tri(p0, p2, p3)], 10.0);

        let t = ring(400);
        let mut set = TrafficSet::new(&models(7), &t, 1.0, false, 1);
        // put one car on the plane by hand; the spawner is exercised by the other tests
        let c = &mut set.cars[0];
        c.active = true;
        c.slice = 0;
        c.position = Vec3::new(5.0, 0.0, 0.0);
        c.orientation = Mat3::IDENTITY;
        c.velocity = Vec3::ZERO;
        c.angular_velocity = Vec3::ZERO;
        set.integrate_on(&t, 1.0 / 64.0, Some(&grid));
        let c = &set.cars[0];
        // the origin `car+0x330` rests `RIDE_HEIGHT + car+0x3AC` along the normal above the plane,
        // and the model is drawn with its ground plane on the surface
        let want = h(5.0) + c.rest_height() * 1.01f32.sqrt();
        assert!((c.position.y - want).abs() < 1e-3, "on the slope: {} vs {}", c.position.y, want);
        let n = Vec3::new(-0.1, 1.0, 0.0).normalize();
        let d = c.draw_origin();
        assert!(((d - Vec3::new(d.x, h(d.x), d.z)).dot(n) - RIDE_HEIGHT).abs() < 1e-3, "model ground on the plane");
        // the body leans with the surface instead of staying world-up
        let up = c.orientation.row(1);
        assert!(up.y > 0.9 && up.x.abs() + up.z.abs() > 0.02, "body up {up:?}");
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
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 20.0, dir: 1.0 };
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
        let far = Reference { position: t.slices[200].centre, slice: 200, laps: 0, speed: 20.0, dir: 1.0 };
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
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 20.0, dir: 1.0 };
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
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 2, speed: 20.0, dir: 1.0 };
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
        let at = |slice: usize| Reference { position: t.slices[slice].centre, slice, laps: 0, speed: 20.0, dir: 1.0 };
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
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 10.0, dir: 1.0 };
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
        let r = Reference { position: t.slices[0].centre, slice: 0, laps: 0, speed: 10.0, dir: 1.0 };
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
        let r = Reference { position: t.slices[3].centre, slice: 3, laps: 0, speed: 5.0, dir: 1.0 };
        for tick in 961..1400 {
            set.update(&t, tick, r, &[]);
            set.integrate(&t, 1.0 / 64.0);
        }
        for c in set.cars.iter().filter(|c| c.active) {
            let p = t.route_pos[c.slice] as i32;
            assert!(p > 5 && p < 114, "slice {p} is too near an end of a linear track");
        }
    }
}
