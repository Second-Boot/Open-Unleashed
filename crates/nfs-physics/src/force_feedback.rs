//! The original's force feedback, as pure arithmetic — **output only, it never feeds back into the
//! simulation** (`RE/notes/force-feedback.md`, read to the instruction 2026-10-02).
//!
//! The whole block is the `0x5E94xx` state of `0x4121B0` / `0x411D20` plus the per-tick driver
//! **0x4123E0**, a callback on the 32 Hz "Sc32" list (`[0x606AD0]`, priority 10 — it runs *before*
//! the car's physics step, priority 0x1E, so it reads what the previous tick left) for the one car
//! `[0x5E9414]` (the player's, `0x412150` / `0x4121B0`). It decides *which DirectInput effect gets
//! which parameters*; the platform library (0x533910: `SetParameters` + `Start`) does the rest, and
//! this module stops at that boundary: [`FfCommand`] is the argument list of one 0x533910 / 0x5338D0
//! call.
//!
//! Four effects are created per session (0x411D20, one per handle `0x5E9420 + 8·dev + {0,2,4,6}`):
//!
//! | handle | what | DirectInput type (0x5BB3B0 table) | gain block |
//! |---|---|---|---|
//! | `+0` | the collision jolt (0x412230 → 0x412CF0, and the landing jolt of the driver) | `GUID_ConstantForce`, polar direction, envelope | `[0x657C28]` |
//! | `+2` | the road rumble (0x4128B0) | `GUID_Square` periodic | `[0x657BE8]` |
//! | `+4` | the engine rumble (0x4127A0) | `GUID_Sine` periodic | `[0x657C08]` |
//! | `+6` | the grip / self-aligning spring (0x4123E0 itself, 0x412730) | `GUID_Spring`, two conditions | `[0x657C48]` |
//!
//! Every gain block's first dword is the percentage 0…100 of one `FF.*` level of
//! `forcefeedback.lay` (`FF.RoadVolume`, `FF.EngineVolume`, `FF.CollisionVolume`,
//! `FF.RoadGripVolume`; the order of the accessors' ids 0x8A…0x8D matches the blocks' order), ×100 in
//! 0x533910 (0x5339A7…0x5339C7) to DirectInput's 0…10000 `dwGain`; `FF.TotalVolume` (`[0x657BC8]`)
//! is the device-wide `DIPROP_FFGAIN` (0x5333A0, clamped to 0…100, ×100) and a gate (a jolt and the
//! driver both return when it is 0). **The binding of the accessors to these globals is the
//! one link not read** (no static writer of `0x657BC8…0x657C68` exists; they sit in the
//! option-global block next to `[0x657C84]` LANGUAGE) — CONFIRMED 2026-10-02: the static key table at 0x4B1A04 pairs each global with its key name (`FFTOTALVOLUME0` ↔ 0x657BC8 …), and the registry block base 0x6573E8 + the slot offsets of replay.md reproduce all five.

use glam::Vec3;

/// The four effects, by the handle slot of `0x5E9420 + 8·dev + slot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfEffect {
    /// slot 0, `GUID_ConstantForce`
    Collision,
    /// slot 2, `GUID_Square`
    Road,
    /// slot 4, `GUID_Sine`
    Engine,
    /// slot 6, `GUID_Spring`
    Grip,
}

/// One `DICONDITION` (24 bytes) as 0x4123E0 builds it: offset, then the two coefficients and the two
/// saturations all equal, dead band 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfCondition {
    pub offset: i32,
    pub coefficient: i32,
}

/// One call into the platform library, with the arguments the original passes. `gain` is the first
/// dword of the effect's block (0…100): 0x533910 multiplies it by 100 for `dwGain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfCommand {
    /// `0x5338D0(handle)` — `IDirectInputEffect::Stop`.
    Stop(FfEffect),
    /// `0x533910(handle, 1, &{magnitude, 0, 0, period_us}, 0, block, …)` on the road (square) or the
    /// engine (sine) effect: a `DIPERIODIC` with `lOffset = dwPhase = 0`. `hz` is the figure the
    /// period was made from (`_ftol(1e6 / hz + 0.5)`).
    Periodic { effect: FfEffect, magnitude: i32, hz: i32, period_us: i32, gain: i32 },
    /// `0x533910(handle+6, 1, &{cond0, cond1}, 0, 0x657C48, …)`: two `DICONDITION`s of the spring.
    Spring { x: FfCondition, y: FfCondition, gain: i32 },
    /// `0x533910(handle+0, 1, &mag, &dur, 0x657C28, 0, &dir, &env)` (0x412CF0): a constant force of
    /// `magnitude`, `duration_us`, polar `direction` in degrees (×100 for DirectInput), and the
    /// envelope `{attack_level, attack_time, fade_level, fade_time}` = `{mag, dur/2, mag, dur − mag}`
    /// (0x412DB0…0x412DE2 — the last is a duration minus a magnitude, as the binary has it).
    Jolt { magnitude: i32, duration_us: i32, direction: i32, attack_level: i32, attack_time: i32, fade_level: i32, fade_time: i32, gain: i32 },
}

/// The five `FF.*` levels, 0…100 each (the accessors' range, default 50).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfLevels {
    /// `[0x657BC8]`, `FF.TotalVolume`
    pub total: i32,
    /// `[0x657BE8]`, `FF.RoadVolume`
    pub road: i32,
    /// `[0x657C08]`, `FF.EngineVolume`
    pub engine: i32,
    /// `[0x657C28]`, `FF.CollisionVolume`
    pub collision: i32,
    /// `[0x657C48]`, `FF.RoadGripVolume`
    pub grip: i32,
}

/// What the producers read of the car (the offsets are the original's).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FfCar {
    /// `car+0x35C`, the horizontal speed estimate, m/s
    pub speed: f32,
    /// `car+0x424`, the body height above the contact plane (the driver tests `< 0`)
    pub body_height: f32,
    /// `word car+0x418`
    pub air_count: u16,
    /// `car+0x464`, the body tracker's surface nibble
    pub surface: u32,
    /// `0x493980`, tyre damage
    pub tyre_damage: f32,
    /// `0x4938A0`, engine damage
    pub engine_damage: f32,
    /// `car+0xDB0 == 0`, a dead car
    pub wrecked: bool,
    /// `car+0xDB4`, the engine rpm
    pub rpm: f32,
    /// `car+0xD64`, the pitch rate (read only while `air_count != 0`, which the driver never reaches)
    pub pitch_rate: f32,
    /// `car+0x11C0` — written only by the race reset (0x410D37, zero); read by 0x412730
    pub car_11c0: f32,
    /// `car+0x55C`, `+0x560`, `+0x564`: [`ff_words`]
    pub words: [f32; 3],
}

impl FfCar {
    /// What 0x4123E0 / 0x4128B0 / 0x4127A0 read, taken from the port's body at the point the
    /// original's callback runs (before this tick's physics step).
    pub fn from_vehicle(v: &crate::vehicle::Vehicle, specs: &crate::specs::CarSpecs) -> Self {
        Self {
            speed: v.speed_est,
            body_height: v.body_height,
            air_count: v.airborne_count,
            surface: v.surface_nibble() as u32,
            tyre_damage: v.tyres_wear(specs),
            engine_damage: v.engine_wear(specs),
            wrecked: v.engine_alive == 0,
            rpm: v.rpm,
            pitch_rate: v.w_local.x,
            car_11c0: 0.0, // only the race reset writes it (0x410D37), with zero
            words: v.ff_words,
        }
    }
}

/// `car+0x55C / +0x560 / +0x564`, written by `Car_physicsStep` 0x49F160 at 0x49F4F6…0x49F5DB (after
/// the damage pull, before the steering angle) — **read only by the force-feedback driver**.
///
/// * `front_contact` is `[0x628F9C]` (0x49F17C…0x49F1A9); without it all three are 0 (0x49F5CF).
/// * `slip_sum` is `car+0x934 + car+0x870`: the two front wheels' `wheel+0xB8` slip fractions
///   *as the previous `Tyre` left them* (this runs before `Tyre`, `tyre-slide.md` §1.5).
/// * `grip` is `specs[0xF4]`, `noise` the damage-pull noise `[ebp+8]` (0 on a tick that is not a
///   multiple of 4), `pull` the whole pull `[ebp-4]`, `g_off` the clamped banking offset `[ebp-8]`.
///
/// `t = min((w0 + w1)·4 / specs[0xF4], 0.8)` (`[0x5B2404]` = 4, `[0x5B24E8]` = 0.8);
/// `+0x564 = |t·2000 + noise·200|` (`[0x5B275C]`, `[0x5B2484]`); `+0x560 = (g_off + pull)·(1 − t)·100`;
/// `+0x55C = (1 − t)·speed·200` up to 50 m/s (`[0x5B251C]`), `(1 − t)·10000` above. The x87's
/// extended intermediates are carried in `f64` and rounded at the stores.
pub fn ff_words(front_contact: bool, slip_sum: f32, grip: f32, noise: f32, pull: f32, g_off: i32, speed: f32) -> [f32; 3] {
    if !front_contact {
        return [0.0; 3];
    }
    let t = (slip_sum as f64 * 4.0) / grip as f64;
    let cap = 0.8f32 as f64;
    let t = if cap <= t { cap } else { t };
    let w564_t = t as f32; // `fst dword [esi+0x564]`
    let w55c_t = (1.0 - t) as f32; // `fstp dword [esi+0x55C]`
    let w564 = (w564_t as f64 * 2000.0 + noise as f64 * 200.0).abs() as f32;
    let w560 = ((g_off as f64 + pull as f64) * w55c_t as f64 * 100.0) as f32;
    let w55c = if speed <= 50.0 { (w55c_t as f64 * speed as f64 * 200.0) as f32 } else { (w55c_t as f64 * 10000.0) as f32 };
    [w55c, w560, w564]
}

/// `_ftol` 0x5A0F98: truncation (`fldcw` with RC = 11), the low dword of the 64-bit result.
fn ftol(v: f64) -> i32 {
    v.trunc() as i64 as i32
}

/// A bare `fistp`: round to nearest, ties to even.
fn fistp(v: f64) -> i32 {
    v.round_ties_even() as i64 as i32
}

/// The road rumble's (Hz, magnitude) ladder, 0x4128B0…0x412A8C: `mph = |car+0x35C|·2.24` (stored as
/// an `f32`), below 2 no rumble at all.
pub fn road_ladder(mph: f32) -> (i32, i32) {
    const LADDER: [(f32, i32, i32); 12] = [
        (5.0, 6, 2500),
        (10.0, 6, 3000),
        (15.0, 7, 3500),
        (20.0, 8, 4000),
        (30.0, 9, 4500),
        (40.0, 10, 5000),
        (60.0, 11, 4500),
        (80.0, 12, 4000),
        (100.0, 13, 4000),
        (140.0, 14, 3500),
        (170.0, 15, 3500),
        (200.0, 16, 3500),
    ];
    for &(lim, hz, mag) in &LADDER {
        if mph < lim {
            return (hz, mag);
        }
    }
    (17, 3000)
}

/// What the surface class does to the road figure (the jump table 0x412C98, indexed `surface − 1`,
/// 11 entries; 0 and above 11 take the default).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Surface {
    Stop,
    Adjust(i32, i32),
    /// class 6: the effect is left exactly as it is
    Keep,
}

fn surface_rule(s: u32) -> Surface {
    match s {
        2 | 10 => Surface::Adjust(1, -500),
        3 => Surface::Adjust(-2, -1000),
        4 => Surface::Adjust(1, 1000),
        5 => Surface::Adjust(4, -1500),
        6 => Surface::Keep,
        7 | 11 => Surface::Adjust(-5, 2000),
        9 => Surface::Adjust(-3, 0),
        _ => Surface::Stop, // 1, 8, 0 and above 11
    }
}

/// `0x412CF0`'s re-arm window: `(dur << 7) · 0x4A90BE59 >> 50` (the signed magic divide by 900000).
pub fn rearm_ticks(duration_us: i32) -> i32 {
    let p = (duration_us.wrapping_shl(7) as i64) * 0x4A90_BE59i64;
    let e = ((p >> 32) as i32) >> 18;
    e + ((e as u32) >> 31) as i32
}

/// The `0x5E94xx` state and the driver.
#[derive(Debug, Clone)]
pub struct ForceFeedback {
    /// `[0x5E940C]`: a force-feedback device was found and bound (0x411D20).
    pub enabled: bool,
    /// `[0x5E9418]`: the car was in the air counter last tick; the next grounded one fires the landing jolt.
    landing_armed: bool,
    /// `[0x5E941C]`: the car is airborne (the road and the spring were stopped once).
    airborne: bool,
    /// `[0x5E9520]`: the road effect is playing; `[0x5E9524]` / `[0x5E9528]` its (Hz, magnitude).
    road_playing: bool,
    road_hz: i32,
    road_mag: i32,
    /// `[0x5E9530]`: the spring is playing.
    spring_playing: bool,
    /// `[0x5E9538]`: the 128 Hz clock before which no jolt may fire.
    jolt_next: i32,
    /// `[0x5E9540]`: ticks the spring is held off after a landing jolt.
    spring_hold: i32,
    /// `[0x5E9534]`: the surface class the road figure was last taken at.
    pub surface: u32,
    /// `(devtype & 0xFF00) == 0x600` (0x412D5A): the device is a wheel. The class code comes from the
    /// platform library (0x532E10), unread — a pad is not one.
    pub wheel: bool,
}

impl ForceFeedback {
    /// 0x4121B0 (session start: `[0x5E9534] = 1`, the flags cleared, 0x4123E0 registered) /
    /// 0x412150 (race arm). The effects' creation (0x411D20) is the backend's.
    pub fn new(enabled: bool, wheel: bool) -> Self {
        Self { enabled, landing_armed: false, airborne: false, road_playing: false, road_hz: 0, road_mag: 0, spring_playing: false, jolt_next: 0, spring_hold: 0, surface: 1, wheel }
    }

    /// `0x4123E0`, once per Sc32 tick, for the FF car. `replay` is `[0x657424] >= 2`; `now` the 128 Hz
    /// counter `[0x6559E0]`. Returns the platform calls in order.
    pub fn tick(&mut self, car: &FfCar, levels: FfLevels, replay: bool, now: i32) -> Vec<FfCommand> {
        let mut out = Vec::new();
        if !self.enabled || replay || levels.total == 0 {
            return out; // 0x4123F6, 0x41240E, 0x41241A
        }
        if car.body_height < 0.0 {
            // 0x412426…0x41249F: once, stop the road and the spring (the engine runs on)
            if !self.airborne {
                self.stop_road_and_spring(&mut out);
                self.airborne = true;
            }
            return out;
        }
        self.airborne = false; // 0x4124A6
        if car.air_count != 0 {
            // 0x4124AC…0x412517: stop the road and the spring and arm the landing jolt
            self.stop_road_and_spring(&mut out);
            self.landing_armed = true;
            return out;
        }
        if self.landing_armed {
            // 0x41251C…0x412581: the first grounded tick fires a jolt (dir 45 when the clock is odd,
            // else 315; 500 µs; 10000) if the car moves at all, and holds the spring for 2 ticks
            if car.speed != 0.0 {
                let dir = if now & 1 != 0 { 45 } else { 315 };
                self.fire_jolt(dir, 500, 10000, levels, now, &mut out);
                self.spring_hold = 2;
            }
            self.landing_armed = false;
        }
        // 0x412587…0x4125C6 (the `[0x5E952C]` countdown): nothing ever sets it, 0x412224 clears it.
        if levels.road != 0 {
            self.road(car, levels.road, &mut out); // 0x4125C8…0x4125E6
        }
        if self.spring_hold > 0 {
            self.spring_hold -= 1; // 0x412707
        } else {
            if levels.grip != 0 {
                self.spring(car, levels.grip, &mut out); // 0x4125F5…0x4126F4
            }
            self.spring_hold = 0;
        }
        if levels.engine != 0 {
            self.engine(car, levels.engine, &mut out); // 0x41270D
        }
        out
    }

    fn stop_road_and_spring(&mut self, out: &mut Vec<FfCommand>) {
        if self.road_playing {
            out.push(FfCommand::Stop(FfEffect::Road));
        }
        self.road_playing = false;
        self.road_hz = 0;
        self.road_mag = 0;
        if self.spring_playing {
            out.push(FfCommand::Stop(FfEffect::Grip));
        }
        self.spring_playing = false;
    }

    /// `0x4128B0`, the road rumble.
    fn road(&mut self, car: &FfCar, gain: i32, out: &mut Vec<FfCommand>) {
        let mph = (car.speed.abs() as f64 * 2.24f32 as f64) as f32;
        if mph < 2.0 {
            // 0x4128D8: stop and clear the recorded pair
            if self.road_playing {
                out.push(FfCommand::Stop(FfEffect::Road));
            }
            self.road_playing = false;
            self.road_hz = 0;
            self.road_mag = 0;
            return;
        }
        let (mut hz, mut mag) = road_ladder(mph);
        // 0x412A96…0x412B0D: the surface class, raised by tyre damage, forced to 7 when wrecked
        self.surface = car.surface;
        if car.tyre_damage >= 30.0 && self.surface < 3 {
            self.surface = 3;
        }
        if car.tyre_damage >= 45.0 && self.surface < 2 {
            self.surface = 2;
        }
        if car.tyre_damage >= 60.0 || car.wrecked {
            self.surface = 7;
        }
        match surface_rule(self.surface) {
            Surface::Stop => {
                if self.road_playing {
                    out.push(FfCommand::Stop(FfEffect::Road));
                }
                self.road_playing = false;
                self.road_hz = hz;
                self.road_mag = mag;
                return;
            }
            Surface::Keep => {}
            Surface::Adjust(dh, dm) => {
                hz += dh;
                mag += dm;
                if !(self.road_playing && self.road_hz == hz && self.road_mag == mag) {
                    // 0x412B91…0x412BE5: period = _ftol(1e6 / Hz + 0.5)
                    let period_us = ftol(1.0e6 / hz as f64 + 0.5);
                    out.push(FfCommand::Periodic { effect: FfEffect::Road, magnitude: mag, hz, period_us, gain });
                    self.road_playing = true;
                }
            }
        }
        // 0x412BEF: the pair is recorded whether or not the effect was touched (class 6 included)
        self.road_hz = hz;
        self.road_mag = mag;
    }

    /// `0x4127A0`, the engine rumble — every tick it is reached, the effect is updated.
    fn engine(&mut self, car: &FfCar, gain: i32, out: &mut Vec<FfCommand>) {
        let hz = ftol(car.rpm as f64 * 0.002_666_666_8f32 as f64 + 10.0);
        let m = ftol(car.rpm as f64 - 5000.0).max(0);
        let q = engine_shift(m);
        // `shl eax, cl` masks the count to 5 bits; `cmp eax, 0x1388; jbe` is unsigned
        let base = (10u32.wrapping_shl(q as u32 & 31)).wrapping_add(1700).min(5000) + 3000;
        let mag = ftol(car.engine_damage as f64 * 100.0 + base as f64) as u32;
        let mag = mag.min(10000) as i32;
        let period_us = ftol(1.0e6 / hz as f64 + 0.5);
        out.push(FfCommand::Periodic { effect: FfEffect::Engine, magnitude: mag, hz, period_us, gain });
    }

    /// The spring, 0x4123E0 0x412601…0x4126F4 with 0x412730.
    fn spring(&mut self, car: &FfCar, gain: i32, out: &mut Vec<FfCommand>) {
        let (signed, abs) = steering_force(car);
        let w55c = car.words[0] as f64;
        let w560 = car.words[1] as f64;
        let mut esi = ftol(w55c);
        if w560 < 0.0 {
            esi = ftol(esi as f64 - w560);
        } else if w560 > 0.0 {
            esi = ftol(esi as f64 + w560);
        }
        let edi = if car.words[2] != 0.0 { ftol(car.words[2] as f64) } else { 0 };
        out.push(FfCommand::Spring {
            x: FfCondition { offset: edi, coefficient: esi },
            y: FfCondition { offset: signed, coefficient: abs },
            gain,
        });
        self.spring_playing = true;
    }

    /// `0x412CF0` (with the arguments `0x412230` or the driver made): the re-arm window, the wheel's
    /// quantised direction, the envelope.
    fn fire_jolt(&mut self, mut dir: i32, duration_us: i32, magnitude: i32, levels: FfLevels, now: i32, out: &mut Vec<FfCommand>) {
        if levels.collision == 0 || now < self.jolt_next {
            return; // 0x412CFA, 0x412D0D
        }
        self.jolt_next = rearm_ticks(duration_us).wrapping_add(now);
        if self.wheel {
            // 0x412D5A…0x412DA6
            dir = if dir == 0 || dir == 360 || dir == 180 {
                if now & 1 != 0 { 90 } else { 270 }
            } else if dir > 0 && dir < 180 {
                90
            } else {
                270
            };
        }
        out.push(FfCommand::Jolt {
            magnitude,
            duration_us,
            direction: dir,
            attack_level: magnitude,
            attack_time: duration_us >> 1,
            fade_level: magnitude,
            fade_time: duration_us.wrapping_sub(magnitude),
            gain: levels.collision,
        });
    }

    /// `0x412230(car, point, magnitude)`, called by 0x40F740 (at 0x40F939) for the FF car after its
    /// gates (`car+0x52C & 0x400` clear, `car+0x430 > 0`). `kind` is `car+0x438 & 0xEFFF0000`,
    /// `body_point` the contact point `car+0x440` minus the car's position, in body axes
    /// (`R·d`, 0x4122A8…0x4122CB), `magnitude` `car+0x430`.
    #[allow(clippy::too_many_arguments)]
    pub fn collision_jolt(&mut self, car: &FfCar, kind: u32, body_point: Vec3, magnitude: f32, levels: FfLevels, replay: bool, now: i32) -> Vec<FfCommand> {
        let mut out = Vec::new();
        if !self.enabled || replay || levels.total == 0 {
            return out; // 0x41225C, 0x412269, 0x412276
        }
        if (magnitude as f64) < 1.0 {
            return out; // 0x412283, `[0x5B2750]` = 1.0
        }
        // atan2(b.x, −b.z) in **turns** (0x532860: fpatan · [0x5BB2C8] = 1/2π), then the arithmetic of
        // half-turns: + 2 when negative, − 2 above 2, × 180 — so only (0, 90] and (270, 360) occur
        let mut a = nfs_math::portable::atan2(body_point.x as f64, -(body_point.z as f64)) *0.159_154_943_091_895_35;
        if a < 0.0 {
            a += 2.0;
        }
        if a > 2.0 {
            a -= 2.0;
        }
        let a = (a * 180.0) as f32; // `fstp dword [ebp-8]`
        let mut dir = fistp(a as f64);
        if dir < 0 {
            dir += 360;
        }
        if dir > 360 {
            dir -= 360;
        }
        let dur = fistp(((magnitude as f64 * 10000.0) as f32) as f64).clamp(100_000, 500_000);
        let mag = fistp(magnitude as f64).wrapping_mul(1000).clamp(5000, 10000);
        if kind == 0x10000 {
            dir = 0; // 0x4123AA, before the test
            if car.speed == 0.0 {
                return out; // a scrape at a standstill
            }
        }
        self.fire_jolt(dir, dur, mag, levels, now, &mut out);
        out
    }

    /// 0x412030 / 0x412090 / 0x4120F0: the callers' stop-all (pause, a menu, the race's end).
    pub fn stop_all(&mut self) -> Vec<FfCommand> {
        let mut out = Vec::new();
        if self.enabled {
            out.extend([FfCommand::Stop(FfEffect::Engine), FfCommand::Stop(FfEffect::Road), FfCommand::Stop(FfEffect::Collision), FfCommand::Stop(FfEffect::Grip)]);
        }
        self.road_playing = false;
        self.spring_playing = false;
        out
    }
}

/// `m / 180` through the signed magic divide `0xB60B60B7` (0x4127DF…0x4127F9): `imul`, add the
/// operand to the high half, `sar 7`, add the sign bit.
pub fn engine_shift(m: i32) -> i32 {
    let hi = ((m as i64 * 0xB60B_60B7u32 as i32 as i64) >> 32) as i32;
    let q = hi.wrapping_add(m) >> 7;
    q + ((q as u32) >> 31) as i32
}

/// `0x412730`: the signed value and its magnitude. While `car+0x418` is zero (always, for the
/// driver) it is `ftol(clamp(−car+0x11C0 · 32500, ±10000))`; otherwise `−car+0xD64 · 0.125` goes
/// in place of `car+0x11C0`.
pub fn steering_force(car: &FfCar) -> (i32, i32) {
    let x = if car.air_count != 0 { car.pitch_rate as f64 * 0.125 } else { car.car_11c0 as f64 };
    let v = (-x * 32500.0f32 as f64).clamp(-10000.0, 10000.0);
    let s = ftol(v);
    (s, s.wrapping_abs())
}

#[cfg(test)]
#[path = "force_feedback_tests.rs"]
mod tests;
