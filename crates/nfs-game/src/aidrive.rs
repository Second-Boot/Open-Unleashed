//! The drive model traffic and police cars actually run (`RE/notes/traffic.md` §27.2).
//!
//! §14.5 described `0x408840`, which writes the velocity vector outright, and concluded that traffic is
//! kinematic. It is not: the fork at 0x4083F8 only reaches `0x408840` when `car+0x7E != 0`, and that
//! byte is written `0` at 0x41719D, 0x49B409, 0x49B51F and 0x4A1664 and nowhere else in the image. Every
//! AI car therefore takes the other branch — a **point mass with a yaw degree of freedom**, driven by
//! two P-cascades whose gains come from `trafcfg.dat`:
//!
//! 1. `0x4090C0` picks the point the car is aiming at, either an explicit world aim point or a node on
//!    the road `round((0.7·|v| + 3) / 6)` slices ahead, offset by the lane target;
//! 2. `0x408F20` turns that into a heading error in **turns**;
//! 3. `0x409570` decides whether the car is in control;
//! 4. `0x409720` (out of control) or `0x409C70` (in control) writes a body-frame force and a yaw torque;
//! 5. `0x40A1D0` rotates them to world space and integrates `v += dt·F`, `ω += dt·T`.
//!
//! Ported here: the aim point, the heading error, the in-control test and the integrator, plus the
//! out-of-control controller, all CONFIRMED. The in-control controller `0x409C70` is not ported yet.

use crate::race::SliceTable;
use glam::{Mat3, Vec3};
use nfs_formats::trafcfg::{AiPhysics, TrafficConfig};

/// The state of one AI-driven car the model reads and writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiCarState {
    pub position: Vec3,
    /// rows are the body's right / up / forward axes in world space (`car+0x364/0x370/0x37C`)
    pub orientation: Mat3,
    pub velocity: Vec3,
    /// turns per second, as the rest of the port stores it
    pub angular_velocity: Vec3,
    /// `car+0x08`
    pub slice: usize,
    /// `car+0xEB0`: +1 along the route, −1 against it
    pub dir: f32,
    /// `car+0x104C`: the gear / heading sense, +1 forward and −1 reversing out of trouble
    pub sense: f32,
    /// `car+0xEC0` signed speed along the route
    pub speed: f32,
    /// `car+0xEC4` octagonal speed magnitude
    pub speed_est: f32,
    /// `car+0xEB8` target speed
    pub target_speed: f32,
    /// `car+0xED0` lateral offset from the centreline
    pub lateral: f32,
    /// `car+0x1074` lateral target
    pub lateral_target: f32,
    /// `car+0x1064..0x106C` explicit world aim point, zero when following the road
    pub aim_point: Vec3,
    /// `car+0x1088` the yaw-acceleration clamp (2.6 from 0x406F80)
    pub max_yaw_acc: f32,
    /// `car+0x1084` the lateral-acceleration clamp (grip · 9.8 from 0x406FA2)
    pub max_lat_acc: f32,
}

/// `0x4090C0` — where the car is aiming, in world space.
pub fn aim_direction(table: &SliceTable, cfg: &TrafficConfig, car: &AiCarState) -> Vec3 {
    if car.aim_point != Vec3::ZERO {
        return car.aim_point - car.position;
    }
    // n = round((0.7·|speed| + 3) / 6) slices, clamped by the config's lookahead thirds
    let n = ((cfg.look_ahead_factor * car.speed.abs() + 3.0) / 6.0).round() as i32;
    let n = n.clamp(cfg.min_lookahead / 3, cfg.max_lookahead / 3).max(1);
    let s = table.add(car.slice, n * (car.dir * car.sense) as i32);
    let sl = &table.slices[s];
    let point = sl.centre + sl.lateral.normalize_or_zero() * car.lateral_target;
    point - car.position
}

/// `0x408F20` — the heading error in **turns**, in −0.5..0.5. Positive means the car points to the
/// right of where it wants to go.
pub fn heading_error(car: &AiCarState, aim: Vec3) -> f32 {
    let fwd = car.orientation.row(2);
    let a = if car.sense >= 0.0 { fwd.x.atan2(fwd.z) } else { (-fwd.x).atan2(-fwd.z) } / std::f32::consts::TAU;
    let b = aim.x.atan2(aim.z) / std::f32::consts::TAU;
    let mut e = a - b;
    if e < -0.5 {
        e += 1.0;
    } else if e > 0.5 {
        e -= 1.0;
    }
    e
}

/// `0x409570` — is the car out of control? `road` is the slice's A/B road extents. The speed floor
/// comes from the **out-of-control** chain's `vel_limit_range` (0x409637 reads `[0x5E6D68]`, which is
/// `0x5E6D48 + 0x20`), so with the shipped file it is 10 − 3 = 7 m/s.
pub fn out_of_control(car: &AiCarState, ooc: &AiPhysics, heading_err: f32, road: (f32, f32)) -> bool {
    heading_err.abs() > 0.25
        || car.sense < 0.0
        || car.speed.abs() < ooc.vel_limit_range - 3.0
        || car.lateral < -road.0
        || car.lateral > road.1
        || (body_lateral_speed(car).abs() > 0.4 * car.speed_est && car.speed.abs() >= 15.0)
}

/// `car+0xD58` — the body-frame lateral speed.
pub fn body_lateral_speed(car: &AiCarState) -> f32 {
    car.velocity.dot(car.orientation.row(0))
}

/// `car+0xD60` — the body-frame forward speed.
pub fn body_forward_speed(car: &AiCarState) -> f32 {
    car.velocity.dot(car.orientation.row(2))
}

/// The body-frame command the controllers produce: a lateral and forward force and a yaw torque.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DriveCommand {
    pub lateral: f32,
    pub forward: f32,
    pub yaw: f32,
}

/// `0x409720` — the out-of-control controller, in full.
pub fn out_of_control_command(car: &AiCarState, cfg: &AiPhysics, heading_err: f32, accel_rate: f32, brake_force: f32) -> DriveCommand {
    let wrong_way = car.dir * car.speed < 0.0 && car.sense >= 0.0;
    // desired angular velocity from the heading error, then the lateral velocity that goes with it
    let mut dav = (-2.0 * cfg.dangle_to_dav * heading_err).clamp(-cfg.max_dav, cfg.max_dav);
    let mut dlv = (10.0 * dav).clamp(-cfg.max_dlvel, cfg.max_dlvel).min(car.speed_est);
    if car.speed.abs() < cfg.vel_limit_range {
        let l = (cfg.lat_vel_limit_factor * car.speed_est).abs();
        dlv = dlv.clamp(-l, l);
        let a = (cfg.ang_vel_limit_factor * car.speed_est).abs();
        dav = dav.clamp(-a, a);
    }
    let yaw_rate = car.angular_velocity.dot(car.orientation.row(1));
    let mut aa = -(yaw_rate - dav) * cfg.dav_to_aa;
    if car.speed.abs() > 18.0 {
        aa *= 0.5;
    }
    aa = aa.clamp(-car.max_yaw_acc, car.max_yaw_acc);
    let la = (-(body_lateral_speed(car) - dlv) * cfg.dlvel_to_clacc).clamp(-car.max_lat_acc, car.max_lat_acc);
    // longitudinal: accelerate while the forward speed is between the dead band and the target
    let want = car.sense * car.target_speed * car.dir;
    let v = body_forward_speed(car);
    let mut fwd = if v > -2.0 && v < want.max(2.0) && want != 0.0 {
        accel_rate * car.sense
    } else if v.abs() > 0.0 {
        -v.signum() * brake_force
    } else {
        0.0
    };
    let mut la = la;
    let mut aa = aa;
    if wrong_way {
        let k = if car.speed_est > 20.0 && car.speed_est < 50.0 { 0.25 } else { 0.75 };
        fwd *= k;
        aa *= k;
        la *= 0.375;
    }
    DriveCommand { lateral: la, forward: fwd, yaw: aa }
}

/// `0x409C70` — the in-control controller (§27.14).
///
/// The surprise here is that the lateral half is **not** a lane-position controller. The lane error
/// `car+0xED0 − car+0x1074` is computed and then discarded (the value never leaves the FPU stack), and
/// the heading error does not enter the lateral force at all. What the controller actually does is damp
/// the velocity component **perpendicular to the desired direction**, so the lane is held only through
/// the aim point: `car+0x1074` moves the point `0x4090C0` aims at, and this kills whatever velocity is
/// not pointing at it. `dlpos_to_dlvel`, `max_dlvel` and `lat_vel_limit_factor` therefore have no
/// effect on an in-control car.
///
/// `grip` is `(1 + 0.0015·forwardSpeed) · car+0x10BC`, the post-collision ramp; `boost` is
/// `car+0x108C` when it exceeds 1.
pub fn in_control_command(car: &AiCarState, cfg: &AiPhysics, heading_err: f32, aim: Vec3, grip: f32, boost: f32, accel_rate: f32, brake_force: f32) -> DriveCommand {
    let d = aim.normalize_or_zero();
    // the desired direction rotated a quarter turn in the ground plane
    let perp = Vec3::new(d.z, d.y, -d.x);
    let k = if boost > 1.0 { boost } else { 1.0 };
    let (a, b) = (grip * car.max_lat_acc * k, grip * cfg.dlvel_to_clacc * k);
    let lateral = (-car.velocity.dot(perp) * b).clamp(-a, a);

    let mut dav = (-2.0 * cfg.dangle_to_dav * heading_err).clamp(-cfg.max_dav, cfg.max_dav);
    if car.speed.abs() < cfg.vel_limit_range {
        let l = (cfg.ang_vel_limit_factor * car.speed_est).abs();
        dav = dav.clamp(-l, l);
    }
    let yaw_rate = car.angular_velocity.dot(car.orientation.row(1));
    let yaw = (-(yaw_rate - dav) * cfg.dav_to_aa).clamp(-car.max_yaw_acc, car.max_yaw_acc);

    // the throttle window is a plain comparison of the route-signed speeds, with no dead band and no
    // gear factor; the brake, like the drive force, acts along the car's own forward axis, so its sign
    // comes from the body-frame speed and not from the route-signed one
    let forward = if car.dir * car.speed < car.dir * car.target_speed { accel_rate } else { -body_forward_speed(car).signum() * brake_force };
    DriveCommand { lateral, forward, yaw }
}

/// `0x40A1D0` — rotate the body-frame command into world space and integrate it. `dt` is the AI dt.
pub fn integrate(car: &mut AiCarState, cmd: &DriveCommand, dt: f32) {
    let right = car.orientation.row(0);
    let fwd = car.orientation.row(2);
    let up = car.orientation.row(1);
    car.velocity += (right * cmd.lateral + fwd * cmd.forward) * dt;
    car.angular_velocity += up * (cmd.yaw * dt);
    // `0x40A1D0` damps or zeroes the yaw rate at walking pace
    let v = body_forward_speed(car);
    if v.abs() < 1.0 {
        car.angular_velocity -= up * car.angular_velocity.dot(up);
    } else {
        let k = (0.15 * v.abs()).min(1.0);
        let yaw = car.angular_velocity.dot(up);
        car.angular_velocity += up * (yaw * k - yaw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::{Section, Slice, SLICE_SPACING};

    fn straight(n: usize) -> SliceTable {
        let slices = (0..n)
            .map(|i| Slice {
                centre: Vec3::new(0.0, 0.0, i as f32 * SLICE_SPACING),
                up: Vec3::Y,
                forward: Vec3::Z,
                lateral: Vec3::X,
                lane_mask: 0,
                lanes_a: 2,
                lanes_b: 2,
                width_a: 11.0,
                width_b: 11.0,
                lane_width_a: 4.0,
                lane_width_b: 4.0,
            })
            .collect();
        SliceTable::build(slices, vec![Section { first: 0, last: n - 1, f0: 0.0, f1: 1.0 }])
    }

    fn car() -> AiCarState {
        AiCarState {
            position: Vec3::ZERO,
            orientation: Mat3::IDENTITY,
            velocity: Vec3::new(0.0, 0.0, 16.0),
            angular_velocity: Vec3::ZERO,
            slice: 10,
            dir: 1.0,
            sense: 1.0,
            speed: 16.0,
            speed_est: 16.0,
            target_speed: 16.5,
            lateral: 0.0,
            lateral_target: 0.0,
            aim_point: Vec3::ZERO,
            max_yaw_acc: 2.6,
            max_lat_acc: 9.8,
        }
    }

    #[test]
    fn the_aim_point_walks_the_road_and_an_explicit_one_overrides_it() {
        let t = straight(60);
        let cfg = TrafficConfig::default();
        let mut c = car();
        c.position = t.slices[10].centre;
        // at 16 m/s the lookahead is round((0.7·16 + 3)/6) = 2 slices, inside the 4..10 clamp -> 4
        let aim = aim_direction(&t, &cfg, &c);
        assert!(aim.z > 0.0 && aim.x.abs() < 1e-3, "{aim:?}");
        assert!((aim.length() - 4.0 * SLICE_SPACING).abs() < 1e-3, "four slices ahead: {}", aim.length());
        // a lane target moves it sideways
        c.lateral_target = 3.0;
        assert!((aim_direction(&t, &cfg, &c).x - 3.0).abs() < 1e-3);
        // an explicit aim point wins outright
        c.aim_point = Vec3::new(100.0, 0.0, 100.0);
        assert_eq!(aim_direction(&t, &cfg, &c), Vec3::new(100.0, 0.0, 100.0) - c.position);
    }

    #[test]
    fn the_heading_error_is_in_turns_and_signed() {
        let c = car();
        // aiming straight ahead is no error
        assert!(heading_error(&c, Vec3::Z).abs() < 1e-6);
        // aiming to the car's right is a negative error (the car must turn right to match)
        let e = heading_error(&c, Vec3::new(1.0, 0.0, 1.0));
        assert!((e + 0.125).abs() < 1e-5, "eighth of a turn: {e}");
        // and it wraps rather than running to ±1
        let back = heading_error(&c, Vec3::new(0.0, 0.0, -1.0));
        assert!(back.abs() <= 0.5 + 1e-6);
    }

    #[test]
    fn control_is_lost_by_heading_speed_or_leaving_the_road() {
        // the test reads the out-of-control chain's vel_limit_range, so the floor is 10 − 3 = 7 m/s
        let cfg = TrafficConfig::default().out_of_control;
        let c = car();
        assert!(!out_of_control(&c, &cfg, 0.1, (11.0, 11.0)));
        assert!(out_of_control(&c, &cfg, 0.3, (11.0, 11.0)), "a quarter turn off is out of control");
        let mut slow = c;
        slow.speed = 7.5;
        assert!(!out_of_control(&slow, &cfg, 0.0, (11.0, 11.0)));
        slow.speed = 6.5;
        assert!(out_of_control(&slow, &cfg, 0.0, (11.0, 11.0)), "below vel_limit_range − 3");
        let mut off = c;
        off.lateral = 12.0;
        assert!(out_of_control(&off, &cfg, 0.0, (11.0, 11.0)), "off the road");
        let mut sliding = c;
        sliding.velocity = Vec3::new(8.0, 0.0, 16.0);
        assert!(out_of_control(&sliding, &cfg, 0.0, (11.0, 11.0)), "sliding sideways at speed");
    }

    #[test]
    fn the_out_of_control_controller_steers_toward_the_aim() {
        let cfg = TrafficConfig::default().out_of_control;
        let c = car();
        // pointing left of the aim: the yaw torque pushes the nose back
        let right = out_of_control_command(&c, &cfg, -0.1, 0.25, 8.0);
        let left = out_of_control_command(&c, &cfg, 0.1, 0.25, 8.0);
        assert!(right.yaw * left.yaw < 0.0, "the torque reverses with the error");
        assert!(right.yaw.abs() <= c.max_yaw_acc && left.yaw.abs() <= c.max_yaw_acc);
        // below the target speed the car accelerates, above it the car brakes
        assert!(out_of_control_command(&c, &cfg, 0.0, 0.25, 8.0).forward > 0.0);
        let mut fast = c;
        fast.velocity = Vec3::new(0.0, 0.0, 40.0);
        fast.speed = 40.0;
        assert!(out_of_control_command(&fast, &cfg, 0.0, 0.25, 8.0).forward < 0.0);
    }

    #[test]
    fn the_in_control_controller_damps_sideways_velocity() {
        let cfg = TrafficConfig::default().in_control;
        let mut c = car();
        // aiming straight down the road with a pure forward velocity: nothing to damp
        let straight_cmd = in_control_command(&c, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0);
        assert!(straight_cmd.lateral.abs() < 1e-5, "{straight_cmd:?}");
        // sliding to the car's right: the force pushes back to the left, and is clamped
        c.velocity = Vec3::new(5.0, 0.0, 16.0);
        let sliding = in_control_command(&c, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0);
        assert!(sliding.lateral < 0.0, "{sliding:?}");
        assert!(sliding.lateral >= -c.max_lat_acc);
        c.velocity = Vec3::new(-5.0, 0.0, 16.0);
        assert!(in_control_command(&c, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).lateral > 0.0);
        // the lane target does not enter the lateral force at all — only the aim point does
        let mut off_lane = car();
        off_lane.lateral = 4.0;
        off_lane.lateral_target = -4.0;
        assert_eq!(in_control_command(&off_lane, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).lateral, straight_cmd.lateral);
        // below the target speed it accelerates, above it it brakes
        assert_eq!(in_control_command(&car(), &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).forward, 0.25);
        let mut fast = car();
        fast.speed = 20.0;
        assert_eq!(in_control_command(&fast, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).forward, -8.0);
    }

    #[test]
    fn the_integrator_moves_velocity_and_spin_by_dt() {
        let mut c = car();
        let cmd = DriveCommand { lateral: 2.0, forward: 1.0, yaw: 0.5 };
        let v0 = c.velocity;
        integrate(&mut c, &cmd, 1.0 / 32.0);
        assert!((c.velocity.x - (v0.x + 2.0 / 32.0)).abs() < 1e-6);
        assert!((c.velocity.z - (v0.z + 1.0 / 32.0)).abs() < 1e-6);
        // at 16 m/s the yaw damping factor is 1.0, so the whole torque lands
        assert!((c.angular_velocity.y - 0.5 / 32.0).abs() < 1e-6);
        // at walking pace the yaw rate is zeroed instead
        let mut slow = car();
        slow.velocity = Vec3::new(0.0, 0.0, 0.5);
        slow.angular_velocity = Vec3::new(0.0, 0.3, 0.0);
        integrate(&mut slow, &cmd, 1.0 / 32.0);
        assert!(slow.angular_velocity.y.abs() < 1e-6);
    }
}
