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
//! * APPROXIMATION (original routine not read): suspension/ride height 0x499A70 → wheels are
//!   sampled on the ground under their nominal positions; car height = mean ground height.
//! * Not modelled yet: damage, drafting, turbo boost, misfire, collisions, rollover recovery.

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
    pub wheels: [Wheel; 4],
    pub accel_local: Vec3,
    pub lateral_filtered: f32,
    pub long_filtered: f32,
    pub gravity_scale: f32,
    pub ticks_alive: u32,
    /// nominal wheel offsets in body space (x right, y up, z forward) — from the model
    pub wheel_offsets: [Vec3; 4],
    pub ride_height: f32,
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
            rng: 0x1234_5678,
        }
    }

    fn rand01(&mut self) -> f32 {
        // The original uses an LCG at 0x5D1028 (multiplier unknown); any uniform source is fine here.
        self.rng = self.rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.rng >> 8) & 0xFFFF) as f32 * 1.52587890625e-05
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
            if speed < 0.2 && self.brake_raw > 0x80 && self.throttle_raw == 0 && self.shift_timer == 0 {
                if self.gear >= 2 {
                    self.gear = 0;
                } else if self.gear == 0 && false {
                    self.gear = 2;
                }
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
        let mut a_drive = t * specs.accel_per_torque[gear];
        let mut diff = rpm_target - rpm_wheel.abs();
        if diff.abs() < 125.0 && rpm_target >= specs.redline - 300.0 {
            diff = 0.0;
        }
        let slip = self.rpm - rpm_wheel.abs();
        let launch = (gear > 1 && vz >= -0.1) || (gear == 0 && vz <= 0.1);
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
            let mu = tbl[w.surface & 15][specs.class];
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
            let df = specs.downforce_front * vz;
            let dr = specs.downforce_rear * vz;
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

    /// `Tyre` 0x4A20D0 + `FrictionCircle` 0x4A1DC0 + `Slide` 0x4A1C70 (HIGH/MEDIUM).
    fn tyre(&mut self, i: usize, specs: &CarSpecs) {
        let w = &mut self.wheels[i];
        let ang = specs.max_steer * w.steer / 17.0; // 1/256 turn
        let (mut s_lat, mut s_lon) = (w.slip_vel[0], w.slip_vel[2]);
        if ang != 0.0 {
            let a = ang / 256.0 * TAU;
            let (sn, cs) = a.sin_cos();
            let (x, z) = (s_lat, s_lon);
            s_lat = x * cs - z * sn;
            s_lon = x * sn + z * cs;
        }
        let mut demand = w.demand;
        if demand != 0.0 && s_lon != 0.0 && demand.signum() != (-s_lon).signum() && self.throttle < 0.25 {
            demand = demand.signum() * demand.abs().min(s_lon.abs());
        }
        demand *= specs.grip;
        let limit = w.limit;
        let mut sliding_now = false;
        if demand.abs() > limit {
            // traction limited: wheel spins / locks → kinetic friction opposing the slip velocity
            sliding_now = true;
            w.locked = true;
            let lim = limit * (1.0 - 1.0 / CIRCLE_SOFTNESS[specs.class]);
            let denom = s_lat.abs() + 0.25 * s_lon.abs();
            let s = if denom > 1e-6 { lim / denom } else { 0.0 };
            w.lateral = s * s_lat;
            w.longitudinal = s * s_lon;
            w.slip = (w.slip * 0.9 + 0.1).min(1.0);
        } else {
            w.locked = false;
            // lateral: slip angle in 1/256 turn from the contact-patch velocities
            let lat = if s_lon == 0.0 {
                0.0
            } else {
                let a = (s_lat).atan2(-0.5 * s_lon) / TAU; // turns
                let mut l = (0.5 - a.abs()) * 256.0;
                if s_lat < 0.0 {
                    l = -l;
                }
                let mag = if w.sliding { l.abs().min(16.0) * limit / 12.0 } else { l.abs().min(2.0) * limit * 0.5 };
                mag.min(s_lat.abs()) * l.signum()
            };
            w.lateral = lat;
            w.longitudinal = demand;
            // friction circle
            let lat_a = w.lateral.abs();
            let lon_a = w.longitudinal.abs() * 0.9;
            let comb = lat_a.max(lon_a) + 0.25 * lat_a.min(lon_a);
            if comb > limit && comb > 0.0 {
                let k = CIRCLE_SOFTNESS[specs.class];
                let excess = (comb - limit) / k;
                let s = ((limit - excess) / comb).max(0.0);
                let contrib = (if w.sliding { 0.5 } else { 12.0 }) * excess / comb;
                w.slip = ((3.0 * w.slip + contrib) / 4.0).min(1.0);
                w.lateral *= s;
                w.longitudinal *= s;
                sliding_now = w.slip > 0.5;
            } else {
                w.slip = (3.0 * w.slip) / 4.0;
            }
        }
        w.sliding = sliding_now;
        if ang != 0.0 {
            let a = -ang / 256.0 * TAU;
            let (sn, cs) = a.sin_cos();
            let (x, z) = (w.lateral, w.longitudinal);
            w.lateral = x * cs - z * sn;
            w.longitudinal = x * sn + z * cs;
        }
    }

    /// Ground query under each nominal wheel position and under the body; builds the physics frame.
    /// APPROXIMATION of 0x49BD40 / 0x499A70 (suspension not read).
    fn ground_contact(&mut self, grid: &TriangleGrid) {
        let mut normal = Vec3::ZERO;
        let mut heights = [0f32; 4];
        let mut any = false;
        for i in 0..4 {
            let off = self.wheel_offsets[i];
            let wp = self.position + self.orientation.transpose() * Vec3::new(off.x, 0.0, off.z);
            let hit = grid.ground(wp.x, wp.y + 2.0, wp.z, 8.0);
            let w = &mut self.wheels[i];
            match hit {
                Some(h) => {
                    heights[i] = h.point.y;
                    w.surface = (h.surface as usize) & 15;
                    w.surface = 1; // TODO: material → surface-type map (RE/notes/race-logic.md pending); 1 = road
                    normal += h.normal;
                    any = true;
                    w.contact = wp.y - h.point.y <= 0.1;
                    w.world_pos = Vec3::new(wp.x, h.point.y + off.y, wp.z);
                }
                None => {
                    w.contact = false;
                    w.world_pos = wp + Vec3::Y * off.y;
                }
            }
        }
        if any {
            self.ground_normal = normal.normalize_or_zero();
            let mean = heights.iter().sum::<f32>() / 4.0;
            let ground = mean + self.ride_height;
            let dy = ground - self.position.y;
            if dy > -0.05 {
                // on or below ground: snap, damp vertical velocity (bounce table for the surface)
                self.position.y = ground.max(self.position.y.min(ground + 0.05));
                if self.velocity.y < 0.0 {
                    let b = SURFACE_BOUNCE[self.wheels[0].surface][0];
                    self.velocity.y = if self.velocity.y < -2.0 { -self.velocity.y * b * 0.4 } else { 0.0 };
                }
                self.position.y = ground;
                self.height_marker = 0.0;
                for w in &mut self.wheels {
                    w.contact = true;
                }
            } else {
                self.height_marker = -dy;
                for w in &mut self.wheels {
                    w.contact = false;
                }
            }
        } else {
            // No collision geometry under the car (off the mesh): keep it on the last known ground plane
            // instead of falling forever (native safeguard, not original behaviour).
            self.ground_normal = Vec3::Y;
            self.height_marker = 0.0;
            if self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
            for w in &mut self.wheels {
                w.contact = true;
            }
        }
        // physics frame (0x49BA30): row0 body x, row1 ground normal, row2 body z, re-orthogonalised
        let up = self.orientation.row(1);
        self.cos_tilt = self.ground_normal.dot(up);
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
        let front_contact = self.wheels[0].contact && self.wheels[1].contact && self.height_marker <= 0.05;
        let rear_contact = self.wheels[2].contact && self.wheels[3].contact && self.height_marker <= 0.05;
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
        let steer_int = (self.steer + g_off).clamp(-127, 127) as f32;
        let mut y = specs.max_steer;
        if speed > 30.0 {
            y *= 1.75 - 0.025 * speed;
        }
        y = y.min(20.0).max(0.0) * 17.0 / specs.max_steer;
        let mut s0 = steer_int * (y + specs.steer_offset) / 127.0;
        let mut s1 = steer_int * (y - specs.steer_offset) / 127.0;
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
            let mut a_brake = specs.brake_decel * self.brake;
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
        // gravity (0x49BD40): applied at 32 Hz
        self.velocity.y -= GRAVITY / 32.0 * self.gravity_scale;
        // angular damping when not upright (Car_simulate gate simplified)
        if self.cos_tilt < 0.5 {
            self.velocity *= 0.95;
            self.angular_velocity.y *= 0.98;
        }
        self.ground_contact(grid);
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

    /// Wall response (APPROXIMATION, see `crate::walls`): push the body out of any wall within its
    /// half-width and remove the inward velocity component with restitution `bounce` and tangential
    /// friction `friction`. Call once per 32 Hz step after `step32`.
    pub fn collide_walls(&mut self, walls: &crate::walls::WallSet, half_width: f32, bounce: f32, friction: f32) -> bool {
        let mut hit = false;
        let fwd = self.orientation.row(2);
        let half_len = self.wheel_offsets[0].z.abs();
        // test the body centre and both axle centres; radius includes a margin against tunnelling
        let radius = half_width + 0.4;
        for probe in [Vec3::ZERO, fwd * half_len, -fwd * half_len] {
            for _ in 0..2 {
                let p = self.position + probe;
                let Some((n2, depth)) = walls.nearest(p, radius) else { break };
                let n = Vec3::new(n2.x, 0.0, n2.y);
                self.position += n * depth;
                let vn = self.velocity.dot(n);
                if vn < 0.0 {
                    let vt = self.velocity - n * vn;
                    self.velocity = vt * (1.0 - friction) - n * vn * bounce;
                    // scrub some yaw rate so the car does not spin endlessly along the wall
                    self.angular_velocity.y *= 0.7;
                }
                hit = true;
            }
        }
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

    fn specs_993() -> CarSpecs {
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
