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
//! are not: the AI level-of-detail gate of §14.3 **is** modelled (`lod_step`, used by `ai_step`), the
//! personality lane offset `car+0xEC8` is always zero for traffic (it draws personality row 0), and
//! the knocked-over hand-back to the rigid body (`car+0x42C == 1`, 0x4084C3) is dead code for traffic
//! (§21.7) — what replaces it, the two damping exits, is here.

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

/// `Lod_shouldUpdate` 0x414B10 for a traffic car (§14.3, CONFIRMED). The class-1 row of
/// `0x5CC770` is `{6×10, 5×10}`, so `lod = clamp(round(dist/16), 0, 19)` gives `k = 6` — 32 Hz,
/// `dt = 1/32`, mask 0 — inside ~160 m of the camera and `k = 5` — 16 Hz, `dt = 1/16`, mask 1 —
/// beyond it. The gate is `((tick/2 + phase) & mask) == mask`, the phase staggering the cars, and
/// the first 15 ticks always run at 32 Hz.
pub fn lod_step(distance: f32, tick: u64, phase: usize) -> Option<(f32, i32)> {
    if tick <= 15 {
        return Some((AI_DT, AI_TICKS));
    }
    let lod = ((distance / 16.0).round() as i32).clamp(0, 19);
    let (mask, dt, ticks) = if lod < 10 { (0u64, AI_DT, AI_TICKS) } else { (1, 1.0 / 16.0, 4) };
    (((tick / 2 + phase as u64) & mask) == mask).then_some((dt, ticks))
}
/// Purgatory delay table `0x5CC4A0`, indexed by the number of registered traffic cars (§9.1).
pub const RESPAWN_TABLE: [u32; 12] = [25, 50, 75, 100, 200, 400, 500, 800, 900, 1000, 1100, 1200];
/// The AI does not start counting down the purgatory delay before this tick (0x40E140).
pub const SPAWN_GATE_TICK: u64 = 0x3C0;
/// `[0x5E6654]` — the 64 Hz ticks one AI step covers, which is what the purgatory delay counts down
/// by (§9.1). At the full 32 Hz rate that is 2, so `0x5CC4A0[7] = 800` is ~6 s on average.
pub const AI_TICKS: i32 = 2;

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
    /// laps the reference car has completed — a traffic car starts counting from the same lap so its
    /// race-order progress (`car+0xAF8`, kept by 0x4169B0 for traffic too) is comparable
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
    /// `car+0xAF8` — progress in laps, so the car takes its place in the race order
    pub progress: f32,
    laps: u32,
    prev_frac: f32,
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
    /// `car+0xEC0`: speed along `dir`
    pub speed: f32,
    /// `car+0xEB8`
    pub target_speed: f32,
    /// `car+0xEEC`: per-spawn speed factor
    pub speed_factor: f32,
    /// `car+0xEE8`: purgatory countdown, AI ticks
    pub respawn_ticks: i32,
    /// `car+0x7F`: out of purgatory and being drawn
    pub active: bool,
    /// a bus respawns five times less often (`car+0x50C` = "SBUS" / "TBUS", §9.1)
    pub bus: bool,
    /// collision box half-extents and the mass derived from them (§21.3, §21.4)
    pub half_extents: Vec3,
    pub mass: f32,
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
            model: spec.model,
            half_extents: spec.half_extents,
            mass: spec.mass(),
            slice: 0,
            progress: 0.0,
            laps: 0,
            prev_frac: 0.0,
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
            bus: spec.bus,
            position: Vec3::ZERO,
            orientation: Mat3::IDENTITY,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
        }
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

/// Where the body sits above the surface under it while a traffic car is resting on the ground.
/// The original derives it from the suspension; the port has none for a traffic car, so it keeps
/// the 1 m the slice height used to carry — APPROXIMATION.
pub const RIDE_HEIGHT: f32 = 1.0;

#[derive(Debug, Clone)]
pub struct TrafficSet {
    /// `[0x5E8BF8]` — how many AI opponents are in the race, for the §5.2 rule 3 boost.
    pub opponents: u32,
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
    /// `[0x5E7544]`: `slices[1].lanesA == 0` — a one-way road carries no oncoming traffic.
    pub one_way: bool,
    /// −1 when the race runs backwards (`REVERSE [0x657430]`).
    pub race_dir: f32,
    /// `LINEAR_TRACK [0x657494]` — point-to-point, so slice arithmetic clamps and the ends despawn.
    pub linear: bool,
}

impl TrafficSet {
    /// `models` = one entry per traffic car the race was set up with.
    pub fn new(models: &[TrafficSpec], table: &SliceTable, race_dir: f32, linear: bool, seed: u32) -> Self {
        let one_way = table.slices.get(1).map(|s| s.lanes_a == 0).unwrap_or(false);
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
        Self { cars, opponents: 0, weather: false, scripts: nfs_formats::scripts::Scripts::default(), rng, one_way, race_dir, linear, cfg: nfs_formats::trafcfg::TrafficConfig::default(), phys: TrafficPhysics::default() }
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

    /// One AI tick (32 Hz, §14.3/§14.4/§14.5). `tick` is the 64 Hz game tick; `others` are the cars the
    /// lane scorers must see that this set does not own (the player and the opponents).
    pub fn update(&mut self, table: &SliceTable, tick: u64, reference: Reference, others: &[LaneCar]) {
        for i in 0..self.cars.len() {
            if !self.cars[i].active {
                if tick > SPAWN_GATE_TICK {
                    // the purgatory countdown stops at 0: a slot whose delay has run out but which
                    // cannot spawn yet (five cars are already out) simply waits there
                    self.cars[i].respawn_ticks = (self.cars[i].respawn_ticks - AI_TICKS).max(0);
                }
                if self.cars[i].respawn_ticks <= 0 && self.active_count() < MAX_ACTIVE {
                    self.spawn(i, table, reference);
                }
                continue;
            }
            if self.should_retire(i, table, reference) || self.is_out_of_bounds(i, table) {
                self.retire(i);
                continue;
            }
            // §14.4 longitudinal: step toward the target speed, never overshooting
            let (dir, factor) = (self.cars[i].dir, self.cars[i].speed_factor);
            let target = target_speed(dir, self.race_dir, reference.speed, factor).abs();
            // §12 / §23: score the three candidate lanes, move to the best one and cap the target
            // speed for the change. A traffic car's personality is row 0 (§23.10), whose lane offsets
            // are all zero, so there is no in-lane swerve to add.
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
                travelled: c.progress * table.lap_len as f32 * 6.0,
            };
            let (my_scan, hit_tick) = (c.scan, c.hit_by_human_tick);
            // §12.9: ask for a script when the human does something worth reacting to, then step the
            // runner. The first 800 m of a car's life are exempt.
            {
                let travelled = me.travelled;
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
                        // in the same lane and close: who is in front decides which script
                        if d < 12.0 && h.lane == my_lane {
                            let ahead = crate::lanes::gap_between(table, h.slice, my_slice) * my_dir > 0.0;
                            requests.push((hi as i32, if ahead { Trigger::HumanBlocksOpponent } else { Trigger::OpponentBlocksHuman }));
                        }
                        // the contact pair of triggers: the original tests the human's own
                        // `+0x550`/`car+0x554` bookkeeping, which is the same collision the port
                        // timestamps in `hit_by_human_tick`; which of the two fires depends on who
                        // is further along
                        if tick.saturating_sub(hit_tick) < 15 {
                            let ahead = crate::lanes::gap_between(table, h.slice, my_slice) * my_dir > 0.0;
                            requests.push((hi as i32, if ahead { Trigger::HumanPassesOpponent } else { Trigger::OpponentPassesHuman }));
                        }
                    }
                }
                let scripts = std::mem::take(&mut self.scripts);
                let c = &mut self.cars[i];
                for (target, trigger) in requests {
                    c.script.request(target, trigger, tick as i64);
                }
                c.script.step(&scripts, AI_TICKS);
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
            sc.score_lane_exists(table, &me, &mut scan, AI_TICKS);
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
            // `0x4081B0`'s SPEED_BURST branch multiplies the rate by 1.5 (mask 4, not HARD_BRAKE)
            let burst = if self.cars[i].script.query(nfs_formats::scripts::mask::SPEED_BURST).is_some() { 1.5 } else { 1.0 };
            let c = &mut self.cars[i];
            c.scan = scan;
            c.lateral_target = lateral_target;
            c.target_speed = capped;

            // §14.3's LOD gate sits inside the *drive* routine 0x408360, not in the AI state
            // machine that 0x406720 runs: a car more than ~152 m from the camera drives at 16 Hz
            // with twice the timestep, staggered by its index, and keeps its last velocity in
            // between. The lane pass above is the AI's and runs every 32 Hz tick regardless.
            let Some((ai_dt, _)) = lod_step(c.position.distance(reference.position), tick, i) else { continue };

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
            if aidrive::in_trouble(&mut c.gear, lane, sl.lanes_a as i32, sl.lanes_b as i32, up_y, AI_TICKS) {
                aidrive::toggle_gear(&mut c.gear, tick as i64, c.speed, aidrive::GEAR_PERIOD);
            }
            let mut st = c.as_ai_state(&self.phys);
            let aim = aidrive::aim_direction(table, &self.cfg, &st);
            let err = aidrive::heading_error(&st, aim);
            let road = (sl.width_a, sl.width_b);
            let boost = early_boost(self.opponents, tick, 1, reference.laps as f32 + table.frac(reference.slice), c.progress, 0);
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
            // race-order progress, with the same lap-wrap rule the racers use
            let frac = table.frac(c.slice);
            if frac < 0.1 && c.prev_frac > 0.5 {
                c.laps += 1;
            } else if frac > 0.5 && c.prev_frac < 0.1 {
                c.laps = c.laps.saturating_sub(1);
            }
            c.prev_frac = frac;
            c.progress = c.laps as f32 + frac;
            let sl = &table.slices[c.slice];
            let lat_axis = sl.lateral.normalize_or_zero();
            // `car+0xED0 = 0x4173C0(car)` — the lateral offset re-derived from the world position, and
            // `car+0xEC0` the speed along the route (`0x417800`)
            c.lateral = (c.position - sl.centre).dot(lat_axis);
            c.speed = c.velocity.dot(sl.forward.normalize_or_zero());
            // ground contact: the highest surface under the car within 6 m of 2 m above it
            match ground.and_then(|g| g.ground(c.position.x, c.position.y + 2.0, c.position.z, 6.0)) {
                Some(h) => {
                    c.position.y = h.point.y + RIDE_HEIGHT;
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
                None => c.position.y = sl.centre.y + RIDE_HEIGHT,
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
        c.laps = reference.laps;
        c.prev_frac = table.frac(slice);
        c.progress = c.laps as f32 + c.prev_frac;
        c.active = true;
        // `0x406CD0` clears the collision counter on every (re)spawn (§21.7)
        c.collisions = 0;
        tracing::debug!(car = i, slice, dir, lateral, lane = lane_of(&sl, lateral), speed = target, "traffic spawn");
    }
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
    fn the_lod_gate_halves_the_rate_past_160_metres() {
        // §14.3: inside ~160 m every 32 Hz tick drives the car; beyond it every other one, with
        // twice the timestep.
        assert_eq!(lod_step(50.0, 1000, 0), Some((AI_DT, AI_TICKS)));
        // the boundary is where `round(d/16)` reaches 10, i.e. 152 m
        assert_eq!(lod_step(151.0, 1000, 0), Some((AI_DT, AI_TICKS)));
        assert!(lod_step(153.0, 1001, 0).is_none() || lod_step(153.0, 1000, 0).is_none());
        let far: Vec<_> = (1000..1016).filter_map(|t| lod_step(200.0, t, 0)).collect();
        assert_eq!(far.len(), 8, "eight of sixteen 64 Hz ticks");
        assert!(far.iter().all(|(dt, n)| (*dt - 1.0 / 16.0).abs() < 1e-6 && *n == 4));
        // the phase staggers the cars: two neighbours do not drive on the same tick
        let a: Vec<bool> = (1000..1008).map(|t| lod_step(200.0, t, 0).is_some()).collect();
        let b: Vec<bool> = (1000..1008).map(|t| lod_step(200.0, t, 1).is_some()).collect();
        assert_ne!(a, b);
        // and the first ticks of a race always run at the full rate
        assert_eq!(lod_step(400.0, 4, 3), Some((AI_DT, AI_TICKS)));
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
        assert!((c.position.y - (h(5.0) + RIDE_HEIGHT)).abs() < 0.05, "on the slope: {} vs {}", c.position.y, h(5.0) + RIDE_HEIGHT);
        // the body leans with the surface instead of staying world-up
        let up = c.orientation.row(1);
        assert!(up.y > 0.9 && up.x.abs() + up.z.abs() > 0.02, "body up {up:?}");
        // without a grid it falls back to the slice height, as it did before
        set.cars[0].position = Vec3::new(5.0, 0.0, 0.0);
        set.integrate_on(&t, 1.0 / 64.0, None);
        let c = &set.cars[0];
        assert!((c.position.y - (t.slices[c.slice].centre.y + RIDE_HEIGHT)).abs() < 1e-3);
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
        // reference, never at the old place
        let far = Reference { position: t.slices[200].centre, slice: 200, laps: 0, speed: 20.0, dir: 1.0 };
        set.update(&t, 3000, far, &[]);
        for c in &set.cars {
            if c.active {
                assert!(oct_distance(c.position, far.position) < DESPAWN_DISTANCE);
            }
        }
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
