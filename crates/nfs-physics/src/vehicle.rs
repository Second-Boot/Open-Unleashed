//! Vehicle model reconstructed from `Car_physicsStep` 0x49F160 and friends (`RE/notes/physics.md`).
//!
//! Conventions (as in the original): angles and angular velocities in **turns** (1.0 = 360°),
//! steering angles in 1/256 turn, body frame x = right, y = up, z = forward; orientation rows are
//! body axes in world space; the "physics frame" is x-axis / ground normal / z-axis.
//!
//! Fidelity notes (see the notes file for each):
//! * CONFIRMED: scheduling/dt, gravity, torque lookup, rpm from wheels, drive/drag/brake, auto
//!   shift, steering ramp and angle law, load transfer, friction circle, integration.
//! * HIGH/MEDIUM: engine rpm heuristics, lateral tyre curve, yaw torque signs.
//! * Suspension / ground (`Susp_update` 0x499A70, `Car_ground` 0x49BD40, `RE/notes/suspension-collision.md`
//!   §3–4, CONFIRMED): wheel reference points probe the ground, pitch/roll are rate-driven kinematics,
//!   landing/rest rules with the surface bounce table. Roughness and damage wobble not modelled.
//! * Walls (`Car_wallCollide` 0x495020 / `WallImpulse` 0x494000, CONFIRMED): 8 body corners swept against
//!   the `.edg` segments, restitution 0.1, no friction, heuristic yaw (sign table MEDIUM).
//! * Not modelled yet: damage, drafting, turbo boost, misfire, body–ground impulse when tilted (damped
//!   instead), car–car impulse (separate approximation).

use crate::collision::TriangleGrid;
use crate::specs::{CarSpecs, CIRCLE_SOFTNESS, GRAVITY, GRIP_DRY, GRIP_WET, SURFACE_BOUNCE};
use glam::{Mat3, Vec3};
use std::f32::consts::TAU;

pub const DT32: f32 = 0.03125;
pub const DT64: f32 = 0.015625;

/// Controller values latched at 32 Hz (`0x5E9664..`): 0..254 pedals, ±126 steer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Controls {
    pub throttle: u8,
    pub brake: u8,
    pub steer: i32,
    pub handbrake: bool,
    /// Requested gear slot for manual boxes (0 = R, 1 = N, 2.. = 1st..); `None` = keep.
    pub gear_request: Option<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Wheel {
    pub contact: bool,
    pub wet: bool,
    /// rotation angle, turns in [0,1)
    pub rotation: f32,
    pub surface: usize,
    pub lateral: f32,
    pub longitudinal: f32,
    /// contact-patch slip velocities ×(−16): lateral, vertical, longitudinal
    pub slip_vel: [f32; 3],
    pub demand: f32,
    pub limit: f32,
    /// steer angle in 1/256 turn
    pub steer: f32,
    pub slip: f32,
    pub locked: bool,
    pub sliding: bool,
    /// world position (rendering)
    pub world_pos: Vec3,
    /// ground height under the probe minus probe height, clamped (`d_i`, wheel+0x64)
    pub height_d: f32,
    pub airborne: bool,
    pub compression: f32,
    /// `Roughness` 0x499730 state: previous / target offset and the distance phase
    /// (`wheel+0x74 / +0x78 / +0x7C`).
    pub rough_prev: f32,
    pub rough_target: f32,
    pub rough_phase: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Vehicle {
    pub position: Vec3,
    pub velocity: Vec3,
    /// rows: x-axis, up, forward
    pub orientation: Mat3,
    /// world, turns/s
    pub angular_velocity: Vec3,
    /// physics frame rows: body x, ground normal, body z
    pub frame: Mat3,
    pub ground_normal: Vec3,
    pub cos_tilt: f32,
    /// 0 on ground, >0.05 airborne (car+0x424)
    pub height_marker: f32,
    pub v_local: Vec3,
    pub w_local: Vec3,
    pub slip_tangent: f32,
    pub rpm: f32,
    /// gear slot: 0 R, 1 N, 2.. forward
    pub gear: usize,
    pub prev_gear: usize,
    pub shift_timer: u32,
    pub ignition_cut: u32,
    pub throttle_raw: u8,
    pub brake_raw: u8,
    pub throttle: f32,
    pub brake: f32,
    pub steer_target: i32,
    pub steer: i32,
    pub handbrake: bool,
    pub automatic: bool,
    /// Before the race start the original forces effective throttle 0 / brake full (0x49DD20,
    /// game tick < 0x1C0) without touching the raw controller bytes.
    pub race_started: bool,
    /// TCS (car-info +0x24) or AI car: a traction-limited wheel clamps its demand to the grip limit
    /// instead of spinning/locking (`Tyre` 0x4A20D0, CONFIRMED).
    pub traction_control: bool,
    /// Engine wheelspin state (`car+0xDB8 == 2`, clutch slipping / spinning rear axle).
    pub rear_spin: bool,
    /// Body damage per zone 0..100 (`car+0x4E4..`, `Damage` 0x494500): 0..7 ring (0 front-right,
    /// 1 front, 2 front-left, 3 left, 4 rear-left, 5 rear, 6 rear-right, 7 right), 8 roof.
    pub damage: [f32; 10],
    pub damage_enabled: bool,
    /// Steering pull from uneven front damage plus suspension-damage noise (`0x49F160`).
    pub damage_pull: f32,
    /// Material class of the last wall touched (`[0x628C88]` word → class, see `walls::wall_class`).
    pub last_wall_class: u8,
    /// `Damage` 0x494500: a corner / roof zone crossed 30 this tick → play the crash one-shot
    /// (`RE/notes/suspension-collision.md` §6.5, `sfx.md` §4.3). Cleared by the caller.
    pub crash_sound: bool,
    pub wheels: [Wheel; 4],
    pub accel_local: Vec3,
    pub lateral_filtered: f32,
    pub long_filtered: f32,
    pub gravity_scale: f32,
    pub ticks_alive: u32,
    /// nominal wheel offsets in body space (x right, y up, z forward) — from the model
    pub wheel_offsets: [Vec3; 4],
    pub ride_height: f32,
    /// dive / roll low-pass states (car+0xDC4 / +0xDC8)
    pub dive: f32,
    pub roll_lp: f32,
    /// previous ground height (car+0x41C), body height above the contact plane (car+0x424), min (car+0x428)
    pub ground_h: f32,
    pub body_height: f32,
    pub min_height: f32,
    /// half extents for wall corners (halfW, halfH, halfL)
    pub half_extents: Vec3,
    /// last wall impulse magnitude (car+0x430)
    pub impact: f32,
    /// LCG state (the original's `0x5D1028 × 0x5D102C`) for road roughness and the engine's noise.
    rng: u32,
}

impl Vehicle {
    pub fn new(position: Vec3, orientation: Mat3, specs: &CarSpecs, half_track: f32, wheel_radius: f32) -> Self {
        let hb = specs.wheelbase * 0.5;
        Self {
            position,
            velocity: Vec3::ZERO,
            orientation,
            angular_velocity: Vec3::ZERO,
            frame: orientation,
            ground_normal: Vec3::Y,
            cos_tilt: 1.0,
            height_marker: 0.0,
            v_local: Vec3::ZERO,
            w_local: Vec3::ZERO,
            slip_tangent: 0.0,
            rpm: 0.0,
            gear: 1,
            prev_gear: 1,
            shift_timer: 0,
            ignition_cut: 0,
            throttle_raw: 0,
            brake_raw: 0,
            throttle: 0.0,
            brake: 0.0,
            steer_target: 0,
            steer: 0,
            handbrake: false,
            automatic: true,
            race_started: true,
            traction_control: false,
            rear_spin: false,
            damage: [0.0; 10],
            damage_enabled: true,
            damage_pull: 0.0,
            last_wall_class: 5,
            crash_sound: false,
            wheels: [Wheel::default(); 4],
            accel_local: Vec3::ZERO,
            lateral_filtered: 0.0,
            long_filtered: 0.0,
            gravity_scale: 1.16,
            ticks_alive: 0,
            wheel_offsets: [
                Vec3::new(-half_track, wheel_radius, hb),
                Vec3::new(half_track, wheel_radius, hb),
                Vec3::new(-half_track, wheel_radius, -hb),
                Vec3::new(half_track, wheel_radius, -hb),
            ],
            ride_height: wheel_radius,
            dive: 0.0,
            roll_lp: 0.0,
            ground_h: position.y - wheel_radius,
            body_height: 0.0,
            min_height: 0.0,
            half_extents: Vec3::new(half_track + 0.15, 0.6, hb + 0.9),
            impact: 0.0,
            rng: 0x1234_5678,
        }
    }

    fn rand01(&mut self) -> f32 {
        // The original uses an LCG at 0x5D1028 (multiplier unknown); any uniform source is fine here.
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.rng >> 8) & 0xFFFF) as f32 * 1.525_878_9e-5
    }

    pub fn forward(&self) -> Vec3 {
        self.orientation.row(2)
    }
    pub fn speed(&self) -> f32 {
        self.velocity.length()
    }
    pub fn v_long(&self) -> f32 {
        self.v_local.z
    }

    /// `Car_processInput` 0x49DD20 (human car; CONFIRMED). Runs at 32 Hz before the physics step.
    fn process_input(&mut self, c: &Controls, specs: &CarSpecs) {
        self.throttle_raw = c.throttle;
        self.brake_raw = c.brake;
        self.steer_target = c.steer.clamp(-126, 126);
        self.handbrake = c.handbrake;
        // steering ramp: at most 12 per physics tick
        let d = self.steer_target - self.steer;
        self.steer += d.clamp(-12, 12);
        self.steer = self.steer.clamp(-127, 127);
        // pedals 0..1 (divisor 252)
        let mut thr = (self.throttle_raw as f32 / 252.0).min(1.0);
        let mut brk = (self.brake_raw as f32 / 252.0).min(1.0);
        if self.ticks_alive < 0x40 || !self.race_started {
            thr = 0.0;
            brk = 1.0;
        }
        // gear selection
        if self.automatic {
            if self.gear == 1 && self.ticks_alive >= 0x40 {
                self.gear = 2;
            }
            // reverse toggle when stopped with brake and no throttle
            let speed = self.speed();
            if speed < 0.2 && self.brake_raw > 0x80 && self.throttle_raw == 0 && self.shift_timer == 0 && self.gear >= 2 {
                self.gear = 0;
            }
            if self.gear == 0 {
                // in reverse the pedals are swapped: throttle drives backwards
                std::mem::swap(&mut thr, &mut brk);
                if self.throttle_raw > 0x80 && speed < 0.2 && self.brake_raw == 0 {
                    // throttle while stopped in reverse → back to drive (choose highest gear for speed)
                    self.gear = 2;
                    std::mem::swap(&mut thr, &mut brk);
                }
            }
        } else if let Some(g) = c.gear_request
            && (g as usize) < specs.gear_slots
            && g as usize != self.gear
            && self.shift_timer == 0
        {
            self.prev_gear = self.gear;
            self.gear = g as usize;
            self.shift_timer = specs.shift_ticks;
        }
        self.throttle = thr;
        self.brake = brk;
    }

    /// `AutoShift` 0x49DAF0 (CONFIRMED).
    fn auto_shift(&mut self, specs: &CarSpecs) {
        let r = self.rand01();
        let up = specs.redline * (0.9 + 0.1 * r);
        let down = specs.redline * 0.8;
        let mut top = specs.gear_slots - 1;
        if specs.redline * specs.mps_per_rpm[specs.gear_slots - 2] > specs.top_speed {
            top -= 1;
        }
        let vz = self.v_local.z.abs();
        let g = self.gear;
        let rpm_now = (vz * specs.ratio_rpm_per_mps[g] * 1.2).min(self.rpm);
        let ratio_prev = if g >= 1 { specs.ratio_rpm_per_mps[g - 1] } else { 0.0 };
        let mut new_gear = g;
        if g >= 2 && self.throttle >= 0.5 && rpm_now.abs() < specs.redline && vz * ratio_prev < down && g - 1 > 1 && self.slip_tangent.abs() < 0.1 {
            new_gear = g - 1; // kick-down
        } else if g >= 2 && rpm_now >= up && g < top {
            new_gear = g + 1;
        } else if g > 2 && vz * ratio_prev < down {
            new_gear = g - 1;
        }
        if new_gear != g {
            self.prev_gear = g;
            self.gear = new_gear;
            self.shift_timer = specs.shift_ticks;
        }
    }

    /// `Engine_update` 0x49E440 → drive − drag (HIGH; clutch-slip branch simplified).
    fn engine_update(&mut self, specs: &CarSpecs, rear_contact: bool) -> f32 {
        let vz = self.v_local.z;
        let a_drag = specs.drag * vz * vz * vz; // draft factor = 1 (single car)
        let gear = self.gear;
        let neutral = gear == 1;
        let rpm_target = if neutral || !rear_contact {
            ((specs.redline + 250.0) * self.throttle).min(specs.redline + 250.0)
        } else {
            ((specs.redline + 100.0) * self.throttle).min(specs.redline + 100.0)
        };
        // rev limiter
        if self.rpm > specs.redline && self.rpm < specs.redline + 250.0 {
            self.rpm = specs.redline + 50.0;
            self.ignition_cut = if gear > 1 && rear_contact { 3 * gear as u32 } else { 2 };
        }
        let mut throttle = self.throttle;
        let mut rpm_target = rpm_target;
        if self.ignition_cut > 0 {
            throttle = 0.0;
            rpm_target = 0.0;
            self.ignition_cut -= 1;
        }
        if neutral {
            if self.rpm < rpm_target {
                self.rpm += 250.0;
            } else if self.rpm > rpm_target {
                self.rpm -= if gear >= 4 { 200.0 } else { 100.0 };
            }
            self.rpm = self.rpm.max(0.0);
            return -a_drag;
        }
        if self.shift_timer > 0 || !rear_contact {
            if self.shift_timer > 0 {
                self.shift_timer -= 1;
            }
            self.rpm = (self.rpm - 100.0).max(0.0);
            return -a_drag;
        }
        if self.automatic {
            self.auto_shift(specs);
            if self.shift_timer > 0 {
                // shift just started: no drive this tick
                return -a_drag;
            }
        }
        let gear = self.gear;
        let rpm_wheel = specs.ratio_rpm_per_mps[gear] * vz;
        let rpm_c = self.rpm.clamp(0.0, specs.redline);
        let t = specs.torque_at(rpm_c);
        let mut a_drive = t * specs.accel_per_torque[gear] * (1.0 - self.part_damage(&specs.zone_masks.engine) / 300.0) * (1.0 - 0.005 * self.part_damage(&specs.zone_masks.gearbox));
        let mut diff = rpm_target - rpm_wheel.abs();
        if diff.abs() < 125.0 && rpm_target >= specs.redline - 300.0 {
            diff = 0.0;
        }
        let slip = self.rpm - rpm_wheel.abs();
        let launch = (gear > 1 && vz >= -0.1) || (gear == 0 && vz <= 0.1);
        self.rear_spin = launch && slip > 250.0;
        if launch && slip > 250.0 && throttle < 0.5 {
            // clutch slipping with little throttle: rpm falls toward wheel rpm
            self.rpm -= slip.min(100.0 * (1.0 - throttle) + 100.0);
        } else if launch && slip > 250.0 {
            self.rpm -= slip / 6.0;
        } else if diff > 0.0 {
            // engine wants to rev: rate limited
            let step = (81920.0 * specs.mps_per_rpm[gear]).min(-slip.min(0.0) + specs.rev_rate * diff);
            self.rpm = (self.rpm + step.max(0.0)).min(rpm_target);
            if rpm_wheel.abs() > self.rpm {
                self.rpm = rpm_wheel.abs();
            }
        } else if diff == 0.0 {
            self.rpm = rpm_wheel.abs();
        } else {
            // over target: fall toward wheel rpm, drive scaled by throttle
            if self.rpm > rpm_wheel.abs() + 200.0 {
                self.rpm -= 200.0;
            } else {
                self.rpm = rpm_wheel.abs();
            }
            a_drive *= throttle;
            self.rpm = self.rpm.min(rpm_target.max(rpm_wheel.abs()));
        }
        if self.rpm < 0.0 {
            self.rpm = 0.0;
            return -vz * 32.0 - a_drag;
        }
        self.rpm = self.rpm.min(1.25 * specs.redline);
        // Drive is proportional to throttle (the original scales `a_drive` by the throttle in the
        // over-target branch and by the clutch model otherwise; a linear throttle map is used for the
        // launch/rev-up branches — MEDIUM).
        a_drive * throttle - a_drag
    }

    /// `LoadTransfer` 0x4A1900 (CONFIRMED).
    fn load_transfer(&mut self, specs: &CarSpecs) {
        let g_eff = self.frame.row(1).y * specs.grip * GRAVITY;
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let tbl = if w.wet { &GRIP_WET } else { &GRIP_DRY };
            // AI cars (`0x52C & 8` / `0xDA8`) read a remapped μ column (0x4A196F..0x4A19BA, HIGH):
            // dry class 1 → column 0, wet class 0 → column 1
            let col = match (self.traction_control, w.wet, specs.class) {
                (true, false, 1) => 0,
                (true, true, 0) => 1,
                (_, _, c) => c,
            };
            let mu = tbl[w.surface & 15][col];
            let share = if i < 2 { specs.front_weight } else { 1.0 - specs.front_weight };
            w.limit = mu * share * g_eff * 0.5;
        }
        let mut dz = self.accel_local.z * specs.weight_transfer * (-1.0 / GRAVITY) * g_eff * (-0.2) / specs.transfer_div;
        if self.throttle > 0.0 && self.v_local.z >= 0.0 {
            dz *= (1.0 - 10.0 * self.slip_tangent.abs()).max(0.0);
        }
        self.wheels[0].limit -= dz;
        self.wheels[1].limit -= dz;
        self.wheels[2].limit += dz;
        self.wheels[3].limit += dz;
        let dx = self.accel_local.x * specs.weight_transfer * (-1.0 / GRAVITY) * g_eff * 0.2 / specs.transfer_div;
        let dx2 = dx / specs.lateral_div;
        self.wheels[0].limit -= dx;
        self.wheels[2].limit -= dx;
        self.wheels[1].limit += dx2;
        self.wheels[3].limit += dx2;
        self.gravity_scale = 1.16;
        let vz = self.v_local.z;
        if vz > 5.0 {
            let aero = 1.0 - 0.005 * self.part_damage(&specs.zone_masks.aero);
            let df = specs.downforce_front * vz * aero;
            let dr = specs.downforce_rear * vz * aero;
            self.wheels[0].limit += df;
            self.wheels[1].limit += df;
            self.wheels[2].limit += dr;
            self.wheels[3].limit += dr;
            if self.cos_tilt < 0.939 {
                self.gravity_scale += 8.0 * dr;
            }
        }
        for w in &mut self.wheels {
            w.limit = w.limit.max(0.0);
        }
    }

    /// Part damage (`0x4938A0` family, HIGH): mean zone damage over the part's zone mask (the
    /// installed-part condition terms are not modelled: stock parts, `1.5 − 0.005·100 = 1`).
    pub fn part_damage(&self, mask: &[bool; 10]) -> f32 {
        let (mut sum, mut n) = (0.0, 0);
        for (d, m) in self.damage.iter().zip(mask.iter()) {
            if *m {
                sum += d;
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { sum / n as f32 }
    }

    /// Average body damage in per cent (10 zones, `0x434FA0`).
    pub fn damage_avg(&self) -> f32 {
        self.damage.iter().sum::<f32>() / 10.0
    }

    /// `Damage_byNormal` 0x494DF0 (CONFIRMED): needs an effect > 10; the zone is chosen from the
    /// contact normal in the body frame (`x = n·right, y = n·up, z = n·forward`).
    pub fn damage_by_normal(&mut self, effect: f32, n_world: Vec3) {
        if effect <= 10.0 {
            return;
        }
        let m = self.orientation;
        let (x, y, z) = (n_world.dot(m.row(0)), n_world.dot(m.row(1)), n_world.dot(m.row(2)));
        let zone = if y < -0.5 {
            8
        } else if x.abs() < 0.1 {
            if z < -0.1 { 1 } else if z > 0.1 { 5 } else { return }
        } else if z.abs() < 0.1 {
            if x > 0.1 { 7 } else { 3 }
        } else if z < -0.1 {
            if x > 0.1 { 0 } else { 2 }
        } else if x > 0.1 {
            6
        } else {
            4
        };
        self.damage_zone(effect, zone);
    }

    /// `Damage` 0x494500 (CONFIRMED): `a = min(amount·0.5, 100)` (the ×1.5 branch belongs to the
    /// `[0x657400]` mode flag — off here, MEDIUM); the zone takes the maximum and each ring
    /// neighbour is raised to the mean of its two neighbours.
    pub fn damage_zone(&mut self, amount: f32, zone: usize) {
        if !self.damage_enabled || zone > 9 {
            return;
        }
        let a = (amount * 0.5).min(100.0);
        // crash sound for the corner / roof zones when the damage crosses 30
        if matches!(zone, 0 | 2 | 4 | 6 | 8) && a > 30.0 && self.damage[zone] < 30.0 {
            self.crash_sound = true;
        }
        self.damage[zone] = self.damage[zone].max(a);
        if zone <= 7 {
            for j in [(zone + 7) % 8, (zone + 1) % 8] {
                let other = if j == (zone + 1) % 8 { (j + 1) % 8 } else { (j + 7) % 8 };
                let v = 0.5 * (self.damage[zone] + self.damage[other]);
                self.damage[j] = self.damage[j].max(v);
            }
        }
    }

    /// `Tyre` 0x4A20D0 + `FrictionCircle` 0x4A1DC0 + `Slide` 0x4A1C70 — CONFIRMED
    /// (`RE/notes/tyre-slide.md` §2–4, ported formula by formula).
    ///
    /// Slip velocities are ×(−16) in the steered-wheel frame; forces are m/s² of car acceleration.
    /// Not modelled: tyre damage (`LIM = LIM0`), the per-car `.ABS` / `.TRACTION` registry flags
    /// (both off) and the `RALLY` flag. `traction_control` stands for the AI-driven flag `car+0xDA8`.
    fn tyre(&mut self, i: usize, specs: &CarSpecs) {
        let front = i < 2;
        let ai = self.traction_control;
        let rear_spin = self.rear_spin;
        let handbrake = self.handbrake;
        let vz = self.v_local.z;
        let throttle = self.throttle;
        let throttle_raw = self.throttle_raw;
        let brake_raw = self.brake_raw;
        let gear = self.gear;
        let class = specs.class;
        let k_class = CIRCLE_SOFTNESS[class];
        let tyre_damage = self.part_damage(&specs.zone_masks.tyres);
        let w = &mut self.wheels[i];
        let ang = specs.max_steer * w.steer / 17.0; // 1/256 turn
        let (mut u, mut v) = (w.slip_vel[0], w.slip_vel[2]);
        if ang != 0.0 {
            let a = ang / 256.0 * TAU;
            let (sn, cs) = a.sin_cos();
            let (x, z) = (u, v);
            u = x * cs - z * sn;
            v = x * sn + z * cs;
        }
        w.slip = 0.0;
        w.locked = false;
        w.lateral = 0.0;
        w.longitudinal = 0.0;
        // §2.2 demand gate: only a demand opposing the wheel motion is limited to the slip speed
        let mut d = w.demand;
        let mut opposing = false;
        if d < 0.0 && v < 0.0 {
            if throttle <= 0.25 || gear != 0 {
                d = d.max(v);
                opposing = true;
            }
        } else if d > 0.0 && v > 0.0 && (throttle <= 0.25 || gear <= 1) {
            d = d.min(v);
            opposing = true;
        }
        d *= specs.grip;
        let limit = w.limit * (1.0 - 0.005 * tyre_damage);
        // §2.3 over-limit decision: lock (Slide) or clamp, for braking-type demands / handbrake only
        if (opposing && d.abs() > limit) || (handbrake && !front && vz.abs() > 0.5) {
            let lock = if handbrake {
                true
            } else if 2.0 * (16.0 - limit) < vz || brake_raw < 0xEC || (vz.abs() < 5.0 && !rear_spin) {
                false
            } else if !ai {
                true
            } else {
                w.surface <= 1 && class != 2
            };
            if lock {
                // §4 Slide: kinetic friction opposing the slip velocity, no back-rotation (original)
                let (a, b) = (u.abs(), v.abs());
                let comb = a.max(b) + 0.25 * a.min(b);
                let lim2 = limit * (1.0 - 1.0 / k_class);
                w.slip = if comb > limit { (comb - limit) / 4.0 } else { 0.0 };
                w.slip = (0.1 * w.slip).min(1.0);
                let s = if comb > 0.01 { lim2 / comb } else { 0.0 };
                // safety net (not original): a tick's impulse may not reverse the slip velocity
                w.lateral = (s * u).clamp(-2.0 * a, 2.0 * a);
                w.longitudinal = (s * v).clamp(-2.0 * b, 2.0 * b);
                w.locked = true;
                w.sliding = true;
                w.demand = d;
                return;
            }
            d = d.clamp(-limit, limit);
        }
        w.demand = d;
        // §2.4 lateral force from the slip angle (1/256 turn)
        let lat = if v == 0.0 {
            0.0
        } else {
            let a = u.atan2(-0.5 * v) / TAU; // turns
            256.0 * if v < 0.0 { a } else if u > 0.0 { 0.5 - a } else if u < 0.0 { -0.5 - a } else { a }
        };
        if front {
            let f = lat.signum() * lat.abs().min(16.0) * limit / 12.0;
            w.lateral = if u.abs() + v.abs() < 32.0 { u.signum() * u.abs().min(f.abs()) } else { f };
        } else {
            let m = if lat.abs() <= 1.0 { 1.0 } else { lat.abs().min(2.0) };
            let f = lat.signum() * m * limit * 0.5;
            w.lateral = u.signum() * u.abs().min(f.abs());
        }
        w.longitudinal = d;
        // §3 friction circle
        let lon_w = w.longitudinal.abs() * 0.9;
        let lat_a = w.lateral.abs();
        let comb = lat_a.max(lon_w) + 0.25 * lat_a.min(lon_w);
        let spin = (w.longitudinal.abs() > limit && throttle_raw > 0x80) || (!front && rear_spin);
        let k = if !front && spin { 4.0 } else { k_class };
        if comb > limit && comb > 0.0 {
            let hard = ai && (w.surface > 1 || class == 2);
            if hard {
                let s = limit / comb;
                w.slip = 0.0;
                if gear > 2 || ai {
                    w.lateral *= s;
                }
                w.longitudinal *= s;
            } else {
                let e = (comb - limit).min(limit) / k;
                let s = (limit - e) / comb;
                w.slip = (3.0 * w.slip + (if front { 0.5 } else { 12.0 }) * (comb - limit)) / 4.0;
                w.lateral *= s;
                w.longitudinal *= s;
            }
        }
        if !front && spin {
            w.slip = comb;
        }
        w.slip = w.slip.min(1.0);
        w.sliding = w.slip > 0.5;
        if ang != 0.0 {
            let a = -ang / 256.0 * TAU;
            let (sn, cs) = a.sin_cos();
            let (x, z) = (w.lateral, w.longitudinal);
            w.lateral = x * cs - z * sn;
            w.longitudinal = x * sn + z * cs;
        }
    }

    /// `Roughness` 0x499730 (car, i) — per-wheel random road-height offset, CONFIRMED:
    /// amplitude and period by surface type, a new target drawn every `period` metres of travel and
    /// interpolated linearly in between. Returns 0 while the car is tilted (`cosTilt < 0.86`).
    fn roughness(&mut self, i: usize, speed_est: f32) -> f32 {
        if self.cos_tilt < 0.86 {
            return 0.0;
        }
        let (amp, period) = match self.wheels[i].surface & 0xF {
            3 | 5 | 9 => (0.1, 2.5),
            2 | 7 => (0.03, 0.25),
            4 | 10 => (0.03, 0.5),
            _ => (0.0, 0.5),
        };
        let w = &mut self.wheels[i];
        w.rough_phase += speed_est / 32.0;
        // 0x4997E4..0x49980B: a new target is drawn when the phase has run past the period, and also
        // on every tick of a smooth surface (amp < 0.01) — which keeps `prev`/`target` at ~0 there, so
        // a wheel leaving a rough patch does not carry a stale offset.
        if w.rough_phase >= period || amp < 0.01 {
            w.rough_phase = 0.0;
            w.rough_prev = w.rough_target;
            let r01 = self.rand01();
            let w = &mut self.wheels[i];
            w.rough_target = amp * (1.0 / 6.0 - r01);
        }
        let w = &self.wheels[i];
        w.rough_prev + (w.rough_target - w.rough_prev) * (w.rough_phase / period)
    }

    /// `BodyGroundCollide` 0x496610 + `GroundImpulse` 0x495F30 (`RE/notes/suspension-collision.md`
    /// §6.3, HIGH): body corners below the contact plane take a normal impulse with surface friction
    /// `k`. `invM = 1/mass`, `invI = 1/(6·mass)` (`Car_physInit` 0x49B510, CONFIRMED). The stored
    /// angular velocity is in turns/s and is converted to rad/s the way the original does.
    /// `car+0x428` (mean of the three lowest body points) is approximated by `body_height` (MEDIUM).
    fn body_ground_collide(&mut self, specs: &CarSpecs, n: Vec3, plane_pt: Vec3, k: f32) {
        let m = self.orientation;
        let (hw, hh, hl) = (self.half_extents.x, self.half_extents.y, self.half_extents.z);
        let (bx, by, bz) = (m.row(0), m.row(1), m.row(2));
        let (mut count, mut depth, mut centroid) = (0u32, 0.0f32, Vec3::ZERO);
        for sz in [-1.0, 1.0] {
            for sx in [-1.0, 1.0] {
                for sy in [-1.0, 1.0] {
                    let p = self.position + by * (sy * hh) + bz * (sz * hl) + bx * (sx * hw);
                    let d = (p - plane_pt).dot(n);
                    if d < 0.01 {
                        count += 1;
                        centroid += p;
                        depth = depth.min(d);
                    }
                }
            }
        }
        if count == 0 {
            return;
        }
        let c = centroid / count as f32;
        let inv_m = 1.0 / specs.mass.max(1.0);
        let inv_i = 1.0 / (6.0 * specs.mass.max(1.0));
        let mut w = self.angular_velocity * TAU; // rad/s
        let r = c - self.position;
        let v_c = self.velocity + w.cross(r);
        let vrel = v_c.dot(n);
        // GroundImpulse 0x495F30
        let denom = 0.25 * inv_m + inv_i * r.cross(n).length();
        let mut j = if denom > 1e-6 { -vrel / denom } else { 0.0 };
        let min_extent = hw.min(hh).min(hl);
        j *= if min_extent > 1.5 { 0.6 / min_extent } else { 0.4 };
        self.impact = self.impact.max(self.velocity.dot(n).abs());
        if k > 0.0 {
            // friction along the contact plane, limited so it cannot reverse the tangential motion
            let tang = v_c - n * vrel;
            if tang.length_squared() > 1e-8 {
                let t = -tang.normalize();
                let jf = (k * j).min(tang.length() / (0.5 * inv_m).max(1e-6));
                self.velocity += t * (jf * 0.5 * inv_m);
                w += r.cross(t) * (jf * 2.0 * inv_i);
            }
        }
        if k < 0.001 {
            self.velocity *= 0.9;
            w *= 0.9;
        } else {
            self.velocity += n * (j * 0.5 * inv_m);
            w += r.cross(n * j) * (2.0 * inv_i);
            // settle a car resting on its body
            if (self.cos_tilt < 0.2 || min_extent < 0.0) && self.body_height < 0.005 && vrel.abs() < 10.0 && w.abs().max_element() < 1.0 {
                w *= 0.75;
                self.velocity *= 0.95;
            }
        }
        let lift = (-depth).max(0.0);
        self.position += n * lift;
        for wh in &mut self.wheels {
            wh.world_pos += n * lift;
        }
        // ClampAngVel 0x4964D0: each component to ±0.75 turns/s for a normal car
        self.angular_velocity = (w / TAU).clamp(Vec3::splat(-0.75), Vec3::splat(0.75));
        let effect = 4.0 * self.velocity.dot(n).abs();
        self.damage_by_normal(effect, n);
    }

    /// `Susp_update` 0x499A70 + `Car_ground` 0x49BD40 (CONFIRMED, `RE/notes/suspension-collision.md` §3–4).
    /// Wheel reference points (tyre bottoms) probe the ground; pitch/roll rates are driven by the
    /// front/rear and left/right contact height differences; the body follows the contact plane.
    fn ground_contact(&mut self, grid: &TriangleGrid, specs: &CarSpecs) {
        let m = self.orientation; // rows = body axes in world space
        let mt = m.transpose();
        let up = m.row(1);
        let s_sign = if self.cos_tilt > 0.0 { 1.0 } else { -1.0 };
        let mut contacts = [Vec3::ZERO; 4];
        let mut susp_y = [0f32; 4];
        let mut n_air = 0;
        let mut heights = [0f32; 4];
        for i in 0..4 {
            let off = self.wheel_offsets[i];
            // tyre-bottom reference point (wheel centre minus the radius along body up)
            let world_pt = self.position + mt * Vec3::new(off.x, off.y - self.ride_height, off.z);
            let probe = world_pt + Vec3::new(0.0, specs.probe_offset * up.y, 0.0);
            let hit = grid.ground(probe.x, probe.y + 4.0, probe.z, 8.0 + 4.0);
            let h_i = match hit {
                Some(h) if h.surface & 0xF != 0 && (h.point.y - self.position.y).abs() < 4.0 => {
                    self.wheels[i].surface = (h.surface as usize) & 0xF;
                    h.point.y
                }
                _ => self.ground_h,
            };
            // road roughness (0x499730), added to the probed ground height
            let speed_est = {
                let (a, b) = (self.v_local.x.abs(), self.v_local.z.abs());
                a.max(b) + 0.25 * a.min(b)
            };
            let h_i = h_i + self.roughness(i, speed_est);
            let w = &mut self.wheels[i];
            heights[i] = h_i;
            contacts[i] = Vec3::new(probe.x, h_i, probe.z);
            w.world_pos = Vec3::new(probe.x, h_i + self.ride_height, probe.z);
            // compression (feeds the wheel render height only)
            let c = (h_i - probe.y) * up.y;
            if c > 0.0 {
                let cmax = if i < 2 { 0.9 } else { 0.75 } * (specs.bump_travel - specs.probe_offset);
                w.compression = c.min(cmax);
            } else {
                let c = c.max(-(if w.airborne { 1.65 } else { 1.6 }) * specs.bump_travel);
                w.compression = 0.5 * (c + w.compression);
            }
            let d_i = (h_i - probe.y).max(-0.30625);
            w.height_d = d_i;
            let v_y = s_sign * d_i * up.y;
            if d_i < -0.2041667 {
                w.airborne = true;
                n_air += 1;
                susp_y[i] = v_y * 0.8;
            } else {
                w.airborne = false;
                susp_y[i] = v_y;
            }
            w.contact = !w.airborne;
        }
        let h = (heights[0] + heights[1] + heights[2] + heights[3]) * 0.25;
        // ground normal from the contact points
        let f = ((contacts[0] + contacts[1]) - (contacts[2] + contacts[3])) * 0.5;
        let r = ((contacts[1] + contacts[3]) - (contacts[0] + contacts[2])) * 0.5;
        let mut n = f.normalize_or_zero().cross(r.normalize_or_zero()).normalize_or_zero();
        if n.y < 0.0 {
            n = -n;
        }
        if n.length_squared() < 0.5 {
            n = Vec3::Y;
        }
        if n.y < 0.5 {
            n_air = 4;
        }
        self.ground_normal = n;
        // pitch / roll rate kinematics
        if self.cos_tilt < 0.707 {
            self.dive *= 0.5;
            self.roll_lp *= 0.5;
        } else if n_air < 4 {
            let div = 8.0;
            let w_body = m * self.angular_velocity; // (pitch, yaw, roll) rates in body axes
            let p_in = ((susp_y[2] + susp_y[3]) - (susp_y[0] + susp_y[1])) / div;
            let a_l = self.accel_local.z;
            self.dive = 0.8 * self.dive + specs.dive_gain * a_l * if a_l > 0.0 { 0.125 } else { 0.175 };
            let p = (p_in - self.dive).clamp(-specs.pitch_clamp, specs.pitch_clamp);
            let r_in = ((susp_y[1] + susp_y[3]) - (susp_y[0] + susp_y[2])) / div;
            self.roll_lp = 0.8 * self.roll_lp + 0.25 * specs.roll_gain * self.accel_local.x;
            let rr = (r_in + (1.0 - self.throttle * 254.0 / 1275.0) * self.roll_lp).clamp(-specs.roll_clamp, specs.roll_clamp);
            let wp = (0.925 * specs.pitch_ret * w_body.x + p).clamp(-0.75, 0.75);
            let wr = (specs.roll_ret * w_body.z + rr).clamp(-0.75, 0.75);
            let wy = w_body.y.clamp(-0.75, 0.75);
            self.angular_velocity = mt * Vec3::new(wp, wy, wr);
        }
        // body height above the contact plane (BodyHeight 0x49A900): heights of the wheel points
        let mut mean = 0.0;
        let mut min = f32::MAX;
        for w in &self.wheels {
            let hh = -w.height_d;
            mean += hh * 0.25;
            min = min.min(hh);
        }
        // height_d is clamped at −0.30625 so an airborne car reads ≥ 0.2 here
        self.body_height = 0.5 * (mean + min);
        self.min_height = min;
        self.height_marker = if self.body_height > 0.05 { self.body_height } else { 0.0 };

        // ---- Car_ground 0x49BD40 (upright branch; tilted cars are damped instead of the body impulse) ----
        let dh = (h - self.ground_h) * 32.0;
        self.ground_h = h;
        if self.min_height > 0.005 {
            self.velocity.y -= GRAVITY / 32.0 * self.gravity_scale;
        }
        let closing = dh - self.velocity.y;
        let surf = self.wheels[0].surface & 15;
        // physics frame (0x49BA30): row0 body x, row1 ground normal, row2 body z, re-orthogonalised
        self.cos_tilt = self.ground_normal.dot(up);
        if self.cos_tilt >= 0.939 {
            if closing > 6.0 && self.body_height < -0.05 {
                // hard landing
                let lift = -0.05 - self.body_height;
                self.position.y += lift;
                self.body_height = -0.05;
                self.velocity.y = 0.4 * SURFACE_BOUNCE[surf][1] * closing + dh;
                self.angular_velocity = self.angular_velocity.clamp_length_max(0.75);
                self.impact = closing;
            } else if closing <= 6.0 && self.body_height < 0.005 {
                // resting / soft landing: follow the ground
                self.position.y -= self.body_height;
                self.body_height = 0.0;
                self.height_marker = 0.0;
                self.velocity.y = if self.velocity.y <= 0.0 || dh <= 0.0 { dh } else { self.velocity.y.max(dh) };
            }
        } else if self.body_height < 0.01 {
            // tilted car with the body on the ground: the impulse of §6.3 instead of plain damping
            let plane_pt = Vec3::new(self.position.x, h, self.position.z);
            self.body_ground_collide(specs, self.ground_normal, plane_pt, SURFACE_BOUNCE[surf][0]);
        }
        let n = self.ground_normal;
        let mut z = self.orientation.row(2);
        let mut x = self.orientation.row(0);
        if n.dot(up) <= 0.9 {
            x = z.cross(n).normalize_or_zero();
            z = n.cross(x).normalize_or_zero();
        }
        self.frame = Mat3::from_cols(x, n, z).transpose();
    }

    /// 32 Hz step: input, forces, velocity integration, ground contact (`Car_simulate`).
    pub fn step32(&mut self, controls: &Controls, specs: &CarSpecs, grid: &TriangleGrid) {
        self.ticks_alive += 1;
        self.process_input(controls, specs);
        let m = self.frame;
        // 0x628F9C = !(w0.air && w1.air && bodyHeight > 0.05), likewise for the rear axle
        let front_contact = !(self.wheels[0].airborne && self.wheels[1].airborne && self.body_height > 0.05);
        let rear_contact = !(self.wheels[2].airborne && self.wheels[3].airborne && self.body_height > 0.05);
        self.v_local = m * self.velocity;
        self.w_local = m * self.angular_velocity;
        let vl = self.v_local;
        self.slip_tangent = if vl.z.abs() > 5.0 { vl.x / vl.z } else { 0.0 };
        // contact-patch slip velocities (×−16) with the yaw/roll term
        let yaw = specs.yaw_lever * self.angular_velocity.y * 32.0 * -0.5;
        let speed = vl.x.abs().max(vl.z.abs()) + 0.25 * vl.x.abs().min(vl.z.abs());
        let roll = if speed > 10.0 { (speed + 30.0) * yaw * 0.025 } else { (speed + 70.0) * yaw * 0.0125 };
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let lat = -16.0 * vl.x + if i < 2 { roll } else { -roll };
            w.slip_vel = [lat, -16.0 * vl.y, -16.0 * vl.z];
        }
        let gravity_l = m * Vec3::new(0.0, -GRAVITY, 0.0);
        // steering angle law
        let g_off = ((gravity_l.x * 8.0) as i32).clamp(-64, 64);
        // damage pull (0x49F160): uneven front-corner damage plus suspension-damage noise every 4th tick
        let mut pull = (self.damage[2] - self.damage[0]) * 0.25;
        if self.ticks_alive.is_multiple_of(4) {
            let r = ((self.ticks_alive.wrapping_mul(1103515245).wrapping_add(12345) >> 16) & 0xFFFF) as f32 / 65535.0 - 0.5;
            pull += r * 0.5 * if speed > 15.0 { speed / 15.0 } else { 1.0 } * self.part_damage(&specs.zone_masks.suspension) * 0.01;
        }
        self.damage_pull = pull;
        let steer_int = (self.steer + pull as i32 + g_off).clamp(-127, 127) as f32;
        let mut y = specs.max_steer;
        if speed > 30.0 {
            y *= 1.75 - 0.025 * speed;
        }
        y = y.clamp(0.0, 20.0) * 17.0 / specs.max_steer;
        let mut s0 = steer_int * (1.0 - self.damage[0] / 512.0) * (y + specs.steer_offset) / 127.0;
        let mut s1 = steer_int * (1.0 - self.damage[2] / 512.0) * (y - specs.steer_offset) / 127.0;
        if vl.z > 80.0 {
            s0 /= vl.z * 0.0125;
            s1 /= vl.z * 0.0125;
        }
        self.wheels[0].steer = s0;
        self.wheels[1].steer = s1;
        self.wheels[2].steer = 0.0;
        self.wheels[3].steer = 0.0;
        let a_drive = self.engine_update(specs, rear_contact);
        if !front_contact && !rear_contact {
            self.accel_local = Vec3::ZERO;
            for w in &mut self.wheels {
                w.slip = 0.0;
                w.lateral = 0.0;
                w.longitudinal = 0.0;
            }
        } else {
            let mut a_brake = specs.brake_decel * self.brake * (1.0 - 0.005 * self.part_damage(&specs.zone_masks.brakes));
            a_brake = a_brake.min(vl.z.abs() * 32.0);
            let sign = -vl.z.signum();
            let a_brake = a_brake * if vl.z == 0.0 { 0.0 } else { sign };
            let a_bf = a_brake * specs.front_brake;
            self.load_transfer(specs);
            let f = specs.front_drive;
            let (l0, l1, l2, l3) = (self.wheels[0].limit, self.wheels[1].limit, self.wheels[2].limit, self.wheels[3].limit);
            let fs = (l0 + l1).max(1e-6);
            let rs = (l2 + l3).max(1e-6);
            self.wheels[0].demand = a_drive * f * l0 / fs + 0.5 * a_bf;
            self.wheels[1].demand = a_drive * f * l1 / fs + 0.5 * a_bf;
            self.wheels[2].demand = a_drive * (1.0 - f) * l2 / rs + 0.5 * (a_brake - a_bf);
            self.wheels[3].demand = a_drive * (1.0 - f) * l3 / rs + 0.5 * (a_brake - a_bf);
            for w in &mut self.wheels {
                w.lateral = 0.0;
                w.longitudinal = 0.0;
            }
            if front_contact {
                self.tyre(0, specs);
                self.tyre(1, specs);
            }
            if rear_contact {
                self.tyre(2, specs);
                self.tyre(3, specs);
            }
            let w = &self.wheels;
            let fx: f32 = w.iter().map(|w| w.lateral).sum();
            let fz: f32 = w.iter().map(|w| w.longitudinal).sum();
            self.accel_local = Vec3::new(fx, 0.0, fz * specs.inv_grip);
            // yaw torque (signs per 0x49F994..0x49F9E3; MEDIUM)
            let torque_y = ((w[1].lateral + w[0].lateral) - (w[2].lateral + w[3].lateral)) * specs.inv_yaw_lever
                - ((w[2].longitudinal + w[0].longitudinal) - w[1].longitudinal - w[3].longitudinal) * specs.inv_yaw_lever;
            // stability assist (0x49F0C0): gravity along slopes
            let mut local = self.accel_local;
            if vl.x.abs() < 1.0 || self.orientation.row(1).y < 0.833 {
                local.x += -19.6 * m.row(0).y;
            }
            local.z += -4.9 * m.row(2).y;
            let acc = m.transpose() * local;
            let angacc = m.transpose() * Vec3::new(0.0, torque_y, 0.0);
            self.velocity += acc * DT32;
            self.angular_velocity.x += angacc.x * DT32;
            self.angular_velocity.z += angacc.z * DT32;
            self.angular_velocity.y += angacc.y * (1.0 / 48.0);
            self.lateral_filtered = 0.25 * fx + 0.75 * self.lateral_filtered;
            self.long_filtered = 0.9 * self.long_filtered + 0.1 * self.accel_local.z;
            // coast yaw damping / handbrake
            if self.gear > 1 && self.throttle < 0.25 && speed > 30.0 && specs.grip <= 2.5 {
                let k = 1.0 - 0.05 * self.angular_velocity.y.abs();
                self.velocity.x *= k;
                self.velocity.z *= k;
            } else if self.handbrake && speed > 1.0 {
                self.velocity *= 0.5;
            }
        }
        // Car_simulate gate: not upright ⇒ damp (0x49FD30)
        if self.cos_tilt < 0.5 {
            self.velocity *= 0.95;
            if self.cos_tilt < 0.2 {
                self.angular_velocity.y *= 0.98;
            }
        }
        self.ground_contact(grid, specs);
    }

    /// 64 Hz integration (`0x49B7A0` / `0x49B8E0`) and wheel rotation (`0x4A2790`).
    pub fn step64(&mut self, specs: &CarSpecs) {
        self.position += self.velocity * DT64;
        let w = self.angular_velocity * DT64; // turns
        let rot = Mat3::from_rotation_y(w.y * TAU) * Mat3::from_rotation_x(w.x * TAU) * Mat3::from_rotation_z(w.z * TAU);
        // rows are body axes in world space: rotate each row
        let m = self.orientation;
        self.orientation = Mat3::from_cols(rot * m.row(0), rot * m.row(1), rot * m.row(2)).transpose();
        // wheel rotation
        let vz = self.v_local.z;
        for (i, wh) in self.wheels.iter_mut().enumerate() {
            let driven = if i < 2 { specs.front_drive >= 0.01 } else { specs.front_drive <= 0.99 };
            let v = if driven && self.gear != 1 && !wh.locked && specs.ratio_rpm_per_mps[self.gear] != 0.0 {
                self.rpm / specs.ratio_rpm_per_mps[self.gear]
            } else {
                vz
            };
            if !wh.locked {
                wh.rotation = (wh.rotation + v / specs.circumference / 64.0).rem_euclid(1.0);
            }
        }
    }

    /// `Car_wallCollide` 0x495020 + `WallImpulse` 0x494000 (CONFIRMED): the 8 body corners are swept
    /// over the last tick against the `.edg` edges; a crossing reflects the normal velocity with
    /// restitution 0.1 (no friction), pushes the body 1.05× back onto the edge line and sets a
    /// heuristic yaw rate. `drivable` tells whether a corner is over drivable road (skipped).
    pub fn collide_walls(&mut self, walls: &crate::walls::WallSet, drivable: &dyn Fn(Vec3) -> bool, _unused: f32, _b: f32, _f: f32) -> bool {
        let m = self.orientation;
        let (hw, hh, hl) = (self.half_extents.x, self.half_extents.y, self.half_extents.z);
        let (bx, by, bz) = (m.row(0), m.row(1), m.row(2));
        let mut corners = Vec::with_capacity(8);
        for sz in [-1.0, 1.0] {
            for sx in [-1.0, 1.0] {
                corners.push(self.position - by * hh + bz * (sz * hl) + bx * (sx * hw));
                corners.push(self.position + by * hh + bz * (sz * hl * 0.5) + bx * (sx * hw));
            }
        }
        let v_tick = self.velocity / 32.0;
        let mut max_j: f32 = 0.0;
        let mut hit = false;
        for p in corners {
            if drivable(p) {
                continue;
            }
            let p0 = p - v_tick;
            let Some((n2, q, class)) = walls.segment_hit(p0, p) else { continue };
            self.last_wall_class = class;
            let n = Vec3::new(n2.x, 0.0, n2.y);
            let push = Vec3::new(q.x - p.x, 0.0, q.y - p.z) * 1.01 * 1.05;
            let j = -self.velocity.dot(n);
            if j <= 0.0 {
                continue;
            }
            self.velocity += n * (1.1 * j);
            self.position += push;
            self.damage_by_normal(3.0 * j, n);
            for w in &mut self.wheels {
                w.world_pos += push;
            }
            // WallSpin 0x493F10 (sign table MEDIUM): spin away from the wall
            if !(j >= 15.0 && self.v_local.z >= 20.0) {
                let side = n.dot(bx);
                let spin = 0.25 * j.abs().min(side.abs());
                self.angular_velocity.y = -spin * side.signum() * self.v_local.z.signum();
            }
            self.velocity *= 0.998;
            max_j = max_j.max(j);
            hit = true;
        }
        self.impact = self.impact.max(max_j);
        hit
    }

    /// Reset onto the track at a slice pose (the game's "reset car" 0x40F540 mode 3 — placement only,
    /// the original's exact conditions are not reproduced).
    pub fn reset_to(&mut self, position: Vec3, orientation: Mat3) {
        self.position = position;
        self.orientation = orientation;
        self.frame = orientation;
        self.velocity = Vec3::ZERO;
        self.angular_velocity = Vec3::ZERO;
        self.rpm = 0.0;
        self.gear = if self.automatic { 2 } else { 1 };
        self.shift_timer = 0;
        for w in &mut self.wheels {
            *w = Wheel::default();
        }
    }

    /// Re-orthonormalise the orientation (every 32 ticks in the original, 0x4A4810).
    pub fn renormalise(&mut self) {
        let z = self.orientation.row(2).normalize_or_zero();
        let x = self.orientation.row(1).cross(z).normalize_or_zero();
        let y = z.cross(x);
        self.orientation = Mat3::from_cols(x, y, z).transpose();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::{Triangle, TriangleGrid};

    fn flat_grid() -> TriangleGrid {
        let s = 2000.0;
        let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
        TriangleGrid::build(vec![Triangle { a, b, c, surface: 1 }, Triangle { a, b: c, c: d, surface: 1 }], 100.0)
    }

    pub(super) fn specs_993() -> CarSpecs {
        // 993 Carrera 3.6 values from GameData/Simulation/CarData/993coupe36.sim (RE/PHYSICS.md)
        let mut sim = nfs_formats::sim::CarSim {
            name: "993".into(),
            mass_kg: 1370.0,
            wheelbase_mm: 2272.0,
            unknown_48: 0,
            induction: 0,
            gear_count: 6,
            unknown_54: 6,
            unknown_58_pct: 90.0,
            unknown_5c_pct: 0.0,
            reverse_ratio: -2.857,
            neutral: 0.0,
            gear_ratios: [3.818, 2.15, 1.56, 1.242, 1.024, 0.82],
            final_drive: 3.444,
            tyre_width_mm: 245.0,
            tyre_aspect: 45.0,
            rim_inches: 16.0,
            top_speed_mph: 168.0,
            redline_rpm: 6700.0,
            unknown_98_pct: 20.0,
            torque_curve: [122.0, 138.0, 150.0, 165.0, 201.0, 209.0, 209.0, 209.0, 224.0, 240.0, 243.0, 240.0, 232.0, 207.0, 161.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            unknown_tail: [0.0; 22],
        };
        sim.unknown_tail = [46.0, 1.9, 30.0, 16.5, 0.0, 9.5, 60.0, 1.0, 1.1, 77.5, 75.0, 1.0, 0.0, 110.0, 3.0, 2.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
        CarSpecs::from_sim(&sim, false)
    }

    fn run(v: &mut Vehicle, specs: &CarSpecs, grid: &TriangleGrid, c: Controls, seconds: f32) {
        let ticks = (seconds * 64.0) as u32;
        for t in 0..ticks {
            if t % 2 == 0 {
                v.step32(&c, specs, grid);
            }
            v.step64(specs);
            if t % 32 == 0 {
                v.renormalise();
            }
        }
    }

    #[test]
    fn derived_specs_match_notes() {
        let s = specs_993();
        assert!((s.circumference - 1.97).abs() < 0.05, "circumference {}", s.circumference);
        assert!((s.ratio_rpm_per_mps[2] - 3.444 * 3.818 * 60.0 / s.circumference).abs() < 1e-3);
        assert!((s.top_speed - 75.0).abs() < 0.1);
        assert!(s.drag > 0.0);
        assert_eq!(s.gear_slots, 8);
    }

    #[test]
    fn full_throttle_accelerates_and_shifts() {
        let specs = specs_993();
        let grid = flat_grid();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
        // settle (brake forced for the first 0x40 ticks)
        run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
        let c = Controls { throttle: 254, ..Default::default() };
        let mut t100 = None;
        for t in 0..(64 * 20) {
            if t % 2 == 0 {
                v.step32(&c, &specs, &grid);
            }
            v.step64(&specs);
            if t100.is_none() && v.velocity.length() >= 100.0 / 3.6 {
                t100 = Some(t as f32 / 64.0);
            }
        }
        let t100 = t100.expect("never reached 100 km/h");
        eprintln!("0-100 km/h in {t100:.2} s; after 20 s: {:.1} km/h gear {} rpm {:.0}", v.velocity.length() * 3.6, v.gear, v.rpm);
        assert!(t100 > 3.0 && t100 < 9.0, "0-100 in {t100}");
        assert!(v.gear >= 5);
        assert!(v.velocity.z > 0.0 && v.velocity.x.abs() < 1.0 && v.position.y < 1.0);
    }

    #[test]
    fn steering_turns_the_car() {
        let specs = specs_993();
        let grid = flat_grid();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
        run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
        run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 5.0);
        let heading0 = v.forward();
        run(&mut v, &specs, &grid, Controls { throttle: 120, steer: 126, ..Default::default() }, 3.0);
        let heading1 = v.forward();
        let turn = heading0.cross(heading1).y;
        eprintln!("turn (cross.y) after 3 s of right steer: {turn:.3}, speed {:.1} km/h", v.speed() * 3.6);
        assert!(turn.abs() > 0.2, "car did not turn");
        // positive steer turns the nose toward body +x (cross.y > 0). Whether +x is screen-right
        // depends on the model frame (RE/notes/car-model.md: +X = car's left) — the input layer maps it.
        assert!(turn > 0.0, "positive steer should turn toward +x (cross.y = {turn})");
    }
}

/// Car–car separation (APPROXIMATION until 0x4A0F80 is read): bodies are treated as circles of
/// `radius` in the xz plane; on overlap they are pushed apart equally and the closing normal
/// velocity is exchanged with restitution `bounce` (equal masses). Returns true on contact.
/// Car–car contact `0x498FF0` → `0x4980D0` → impulse `0x496860` (`RE/notes/suspension-collision.md`
/// §6.4, HIGH for the linear impulse). The bodies are tested as oriented boxes in the ground plane
/// (separating axes = both cars' right/forward axes; the original's 9-axis OBB test is 3-D, MEDIUM);
/// the contact normal is the axis of least penetration pointing from `a` to `b`.
///
/// Impulse: `vrel = (vA − vB)·n`, `j = vrel / (½·invM_A + ½·invM_B)`, `j *= ½`, applied as
/// `A.vel += n·j·invM_A` / `B.vel −= n·j·invM_B` (× 1.25 for hard hits, effect ≥ 80). The angular
/// terms of the original use the rigid-body inertia this model does not carry; the yaw response is
/// the wall-hit heuristic instead (MEDIUM). The bodies are then pushed apart along the centre line
/// by the penetration depth. Returns the effect magnitude when a contact was resolved.
pub fn collide_cars(a: &mut Vehicle, mass_a: f32, b: &mut Vehicle, mass_b: f32) -> Option<f32> {
    // pre-test 0x498FF0: axis-aligned distance against the sum of the bounding radii
    let r_a = a.half_extents.x.max(a.half_extents.z);
    let r_b = b.half_extents.x.max(b.half_extents.z);
    let d = b.position - a.position;
    if d.x.abs() >= r_a + r_b || d.z.abs() >= r_a + r_b || d.y.abs() >= a.half_extents.y + b.half_extents.y + 0.5 {
        return None;
    }
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z).normalize_or_zero();
    let axes = [flat(a.orientation.row(0)), flat(a.orientation.row(2)), flat(b.orientation.row(0)), flat(b.orientation.row(2))];
    let ext = |v: &Vehicle, axis: Vec3| -> f32 { (flat(v.orientation.row(0)).dot(axis) * v.half_extents.x).abs() + (flat(v.orientation.row(2)).dot(axis) * v.half_extents.z).abs() };
    let mut best: Option<(f32, Vec3)> = None;
    for axis in axes {
        if axis.length_squared() < 1e-6 {
            continue;
        }
        let dist = d.dot(axis);
        let overlap = ext(a, axis) + ext(b, axis) - dist.abs();
        if overlap <= 0.0 {
            return None;
        }
        if best.is_none_or(|(o, _)| overlap < o) {
            best = Some((overlap, if dist >= 0.0 { axis } else { -axis }));
        }
    }
    let (depth, n) = best?;
    // `CarCarImpulse` 0x496860 (`RE/notes/traffic.md` §21.6), symmetric and with no special case for
    // traffic. The contact point the original gets from its SAT is not recovered, so the midpoint of
    // the two centres stands in for it (APPROXIMATION) — which is also what the original uses for the
    // effect position. The solver works in rad/s, so the stored turns/s angular velocities are scaled
    // by 2π around it exactly as 0x498FF0 does.
    let (inv_a, inv_b) = (1.0 / mass_a.max(1.0), 1.0 / mass_b.max(1.0));
    let (inv6_a, inv6_b) = (inv_a / 6.0, inv_b / 6.0);
    let pt = (a.position + b.position) * 0.5;
    let (r_a, r_b) = (pt - a.position, pt - b.position);
    // the SAT above returns the normal pointing from A to B; the solver's `n` points the other way
    let ni = -n;
    let (c_a, c_b) = (r_a.cross(ni), r_b.cross(ni));
    let (w_a, w_b) = (a.angular_velocity * TAU, b.angular_velocity * TAU);
    let vrel = b.velocity.dot(ni) - a.velocity.dot(ni) - w_a.dot(c_a) + w_b.dot(c_b);
    let den = 0.5 * (c_a.length_squared() * inv6_a + c_b.length_squared() * inv6_b + inv_b + inv_a);
    let mut effect = 0.0;
    let j = if den > 1e-9 { vrel / den } else { 0.0 };
    if j >= 0.0 {
        let jj = j * 0.5;
        let imp = ni * jj;
        // impact magnitude 0x430: `min(4·|Δv|, |vclose·(mA+mB)|/m)`, floored at 9 when the other body
        // is lighter than 0.65 mass units
        let vclose = a.velocity.dot(ni) - b.velocity.dot(ni);
        let p = vclose * (mass_a + mass_b);
        let sp = 4.0 * (a.velocity - b.velocity).length();
        let eff_a = if mass_b > 0.65 { sp.min(p.abs() * inv_a) } else { 9.0 };
        let eff_b = if mass_a > 0.65 { sp.min(p.abs() * inv_b) } else { 9.0 };
        effect = eff_a.max(eff_b);
        a.velocity += imp * inv_a * if eff_a >= 80.0 { 1.25 } else { 1.0 };
        b.velocity -= imp * inv_b * if eff_b >= 80.0 { 1.25 } else { 1.0 };
        let (t_a, t_b) = (if eff_a >= 80.0 { 3.0 } else { 1.5 }, if eff_b >= 80.0 { 3.0 } else { 1.5 });
        let w_a = w_a + r_a.cross(imp) * (t_a * inv6_a);
        let w_b = w_b - r_b.cross(imp) * (t_b * inv6_b);
        // back to turns/s, then ClampAngVel 0x4964D0 on each component
        let clamp = |w: Vec3| (w / TAU).clamp(Vec3::splat(-0.75), Vec3::splat(0.75));
        a.angular_velocity = clamp(w_a);
        b.angular_velocity = clamp(w_b);
        a.impact = a.impact.max(eff_a);
        b.impact = b.impact.max(eff_b);
        a.damage_by_normal(eff_a, n);
        b.damage_by_normal(eff_b, -n);
    }
    // push apart along the centre line, split by mass
    let dir = flat(d);
    let dir = if dir.length_squared() < 1e-6 { n } else { dir };
    let share_a = inv_a / (inv_a + inv_b);
    let push = depth * 1.01;
    a.position -= dir * (push * share_a);
    b.position += dir * (push * (1.0 - share_a));
    for w in &mut a.wheels {
        w.world_pos -= dir * (push * share_a);
    }
    for w in &mut b.wheels {
        w.world_pos += dir * (push * (1.0 - share_a));
    }
    Some(effect)
}

#[cfg(test)]
mod car_car_tests {
    use super::*;

    fn car(pos: Vec3, vel: Vec3, yaw: f32) -> Vehicle {
        let specs = super::tests::specs_993();
        let rot = Mat3::from_rotation_y(yaw).transpose();
        let mut v = Vehicle::new(pos, rot, &specs, 0.8, 0.3);
        v.velocity = vel;
        v
    }

    #[test]
    fn head_on_equal_mass_shares_velocity_and_separates() {
        let mut a = car(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 8.0), 0.0);
        let mut b = car(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -8.0), 0.0);
        let hit = collide_cars(&mut a, 1000.0, &mut b, 1000.0);
        assert!(hit.is_some());
        // j = vrel/(invM) * 0.5 with equal masses: each car loses half the closing speed along n
        assert!((a.velocity.z - 0.0).abs() < 1e-3, "{:?}", a.velocity);
        assert!((b.velocity.z - 0.0).abs() < 1e-3, "{:?}", b.velocity);
        assert!(b.position.z - a.position.z > 3.0, "pushed apart");
    }

    #[test]
    fn separated_boxes_do_not_touch() {
        let mut a = car(Vec3::ZERO, Vec3::Z, 0.0);
        let mut b = car(Vec3::new(4.0, 0.0, 0.0), Vec3::ZERO, 0.0);
        assert!(collide_cars(&mut a, 1000.0, &mut b, 1000.0).is_none());
        // side by side just outside the widths
        let mut c = car(Vec3::new(2.5, 0.0, 0.0), Vec3::ZERO, 0.0);
        assert!(collide_cars(&mut a, 1000.0, &mut c, 1000.0).is_none());
    }

    #[test]
    fn the_impulse_conserves_momentum_and_clamps_the_spin() {
        let (ma, mb) = (2000.0f32, 1000.0f32);
        let mut a = car(Vec3::ZERO, Vec3::new(0.0, 0.0, 12.0), 0.0);
        let mut b = car(Vec3::new(0.6, 0.0, 3.0), Vec3::new(0.0, 0.0, 2.0), 0.4);
        a.angular_velocity = Vec3::new(0.0, 0.3, 0.0);
        let p0 = a.velocity * ma + b.velocity * mb;
        assert!(collide_cars(&mut a, ma, &mut b, mb).is_some());
        let p1 = a.velocity * ma + b.velocity * mb;
        // below the 80 "hard hit" threshold both sides take the same impulse, so momentum is conserved
        assert!((p1 - p0).length() < 1.0, "{p0:?} -> {p1:?}");
        // and each angular-velocity component stays inside ClampAngVel's ±0.75 turns/s
        for w in [a.angular_velocity, b.angular_velocity] {
            assert!(w.abs().max_element() <= 0.75 + 1e-6, "{w:?}");
        }
    }

    #[test]
    fn heavier_car_moves_less() {
        let mut a = car(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0), 0.0);
        let mut b = car(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, 0.0);
        collide_cars(&mut a, 2000.0, &mut b, 1000.0);
        assert!(a.velocity.z > 5.0 && b.velocity.z > 0.0 && b.velocity.z < 10.0, "{:?} {:?}", a.velocity, b.velocity);
    }
}

#[cfg(test)]
mod damage_tests {
    use super::*;

    #[test]
    fn zone_from_normal_and_ring_propagation() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        // hitting the front (normal points backwards into the car → z < −0.1 in the body frame)
        v.damage_by_normal(60.0, Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(v.damage[1], 30.0, "front zone takes min(60·0.5, 100)");
        assert_eq!(v.damage[0], 15.0, "ring neighbours get the mean of their neighbours");
        assert_eq!(v.damage[2], 15.0);
        assert_eq!(v.damage[5], 0.0);
        // small effects do nothing
        v.damage_by_normal(5.0, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(v.damage[5], 0.0);
        // roof
        v.damage_by_normal(300.0, Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(v.damage[8], 100.0);
        assert!((v.damage_avg() - (30.0 + 15.0 + 15.0 + 100.0) / 10.0).abs() < 1e-4);
        assert!(v.part_damage(&specs.zone_masks.brakes) > 0.0);
        assert_eq!(v.part_damage(&[false; 10]), 0.0);
    }
}

#[cfg(test)]
mod body_ground_tests {
    use super::*;

    /// A car dropped upside down onto flat ground is pushed out of the surface, loses its downward
    /// speed and has its angular velocity clamped (`BodyGroundCollide` 0x496610 / `ClampAngVel`).
    #[test]
    fn upside_down_car_is_pushed_out_and_settles() {
        let specs = tests::specs_993();
        let flipped = Mat3::from_rotation_z(std::f32::consts::PI).transpose();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.2, 0.0), flipped, &specs, 0.8, 0.3);
        v.velocity = Vec3::new(0.0, -8.0, 0.0);
        v.angular_velocity = Vec3::new(3.0, -4.0, 5.0);
        v.cos_tilt = -1.0;
        v.body_height = 0.0;
        let plane = Vec3::ZERO;
        v.body_ground_collide(&specs, Vec3::Y, plane, 0.5);
        assert!(v.velocity.y > -8.0, "downward speed reduced: {:?}", v.velocity);
        assert!(v.position.y >= 0.2, "pushed out of the ground: {}", v.position.y);
        assert!(v.angular_velocity.abs().max_element() <= 0.75 + 1e-6, "ω clamped: {:?}", v.angular_velocity);
        assert!(v.impact > 0.0);
        // a light landing leaves too little residual normal speed to damage the body
        // (`Damage_byNormal` needs an effect > 10, i.e. `4·|vel·n| > 10`)
        assert!(v.damage.iter().all(|d| *d == 0.0));
    }

    /// A hard drop onto the roof does damage the roof zone.
    #[test]
    fn hard_roof_impact_damages_the_roof() {
        let specs = tests::specs_993();
        let flipped = Mat3::from_rotation_z(std::f32::consts::PI).transpose();
        let mut v = Vehicle::new(Vec3::new(0.0, 0.2, 0.0), flipped, &specs, 0.8, 0.3);
        v.velocity = Vec3::new(0.0, -30.0, 0.0);
        v.cos_tilt = -1.0;
        v.body_height = 0.0;
        v.body_ground_collide(&specs, Vec3::Y, Vec3::ZERO, 0.5);
        assert!(v.damage[8] > 0.0, "roof zone damaged: {:?}", v.damage);
    }

    /// No body point below the plane: nothing happens.
    #[test]
    fn airborne_car_is_untouched() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::new(0.0, 10.0, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.velocity = Vec3::new(1.0, -2.0, 3.0);
        let before = (v.position, v.velocity, v.angular_velocity);
        v.body_ground_collide(&specs, Vec3::Y, Vec3::ZERO, 0.5);
        assert_eq!((v.position, v.velocity, v.angular_velocity), before);
    }
}

#[cfg(test)]
mod roughness_tests {
    use super::*;

    #[test]
    fn roughness_by_surface_and_tilt() {
        let specs = tests::specs_993();
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
        v.cos_tilt = 1.0;
        // smooth surfaces (asphalt = 1) have no roughness at all
        v.wheels[0].surface = 1;
        for _ in 0..200 {
            assert_eq!(v.roughness(0, 30.0), 0.0);
        }
        // a rough surface (3) draws offsets within ±amplitude (0.1 m) and varies over distance
        v.wheels[1].surface = 3;
        let mut seen: Vec<f32> = Vec::new();
        for _ in 0..200 {
            let r = v.roughness(1, 30.0);
            assert!(r.abs() <= 0.1 + 1e-6, "amplitude {r}");
            seen.push(r);
        }
        assert!(seen.iter().any(|r| *r != 0.0), "some offset is produced");
        // a tilted car gets none (cosTilt < 0.86)
        v.cos_tilt = 0.5;
        assert_eq!(v.roughness(1, 30.0), 0.0);
    }
}
