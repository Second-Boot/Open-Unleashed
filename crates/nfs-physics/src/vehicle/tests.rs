/// 0x498AA0 zeroes the car's and the pole's `y` (0x498B2C / 0x498B34): a pole 15 m above the
/// car (the monaco5 harbour tunnel) pushes it in the x-z plane only.
#[test]
fn a_pole_pushes_in_the_xz_plane() {
    let specs = specs_993();
    let mut v = Vehicle::new(Vec3::new(0.0, -4.0, 0.0), Mat3::IDENTITY, &specs, 0.7, 0.3);
    v.velocity = Vec3::new(1.0, 1.1, 0.0);
    let pole = crate::props::Prop {
        shape: crate::props::PropShape::Cylinder { radius: 0.25 },
        aabb_min: Vec3::new(0.3, 9.0, -0.25),
        aabb_max: Vec3::new(0.8, 13.0, 0.25),
        class: 1,
        breakable: None,
        drive_through: false,
        article: 0,
    };
    assert!(v.collide_props(&[pole], 0).is_some());
    assert!(v.velocity.y.abs() <= 1.1, "no vertical push: {:?}", v.velocity);
    assert_eq!(v.position.y, -4.0);
    assert!(v.velocity.x < 1.0, "pushed away from the pole: {:?}", v.velocity);
}

#[test]
fn brake_lamps_follow_the_latch() {
    // 0x43937A: the throttle byte while the reverse latch is set, the brake byte otherwise
    let specs = specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
    v.automatic = true;
    v.gear = 0;
    v.reverse_latch = true;
    (v.throttle_byte, v.brake_byte) = (0xFF, 0); // the throttle key is down
    assert!(v.brake_lamp());
    v.reverse_latch = false; // the throttle byte is not the brake byte
    assert!(!v.brake_lamp());
}

use super::*;
use crate::collision::{Triangle, TriangleGrid};

/// `0x4A1472…0x4A14A6`: the half extents are half the span of the body points, per axis.
#[test]
fn the_half_extents_are_half_the_body_points_span() {
    let specs = specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
    v.body_points = vec![
        Vec3::new(-0.8, 0.0, -2.0),
        Vec3::new(0.9, 0.0, -2.0),
        Vec3::new(0.9, 1.2, 2.2),
        Vec3::new(-0.8, 1.2, 2.2),
    ];
    v.fit_half_extents();
    assert_eq!(v.half_extents, Vec3::new(0.85, 0.6, 2.1));
    // no points, no change — 0x4A13CD's loop runs `car+0x32C` times and that is zero
    let before = v.half_extents;
    v.body_points.clear();
    v.fit_half_extents();
    assert_eq!(v.half_extents, before);
}

/// A racer's body (0x445090 + 0x4421BE): the origin `car+0x330` goes `M[1]·car+0x3AC` above
/// where the car was placed, the model is drawn back there (`R3DCar` slot 5 0x43A410), and
/// the world points — `pos + Mᵀ·p` of the lowered points — are where they were.
/// `LoadTransfer` 0x4A1900 on a flat, still car: the wet test picks the wet table for every
/// wheel (0x4A198E / 0x4A1995), row 1 class 0 being 1.0 dry and 0.9 wet.
#[test]
fn a_wet_track_reads_the_wet_grip_table() {
    let specs = specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
    v.frame = Mat3::IDENTITY;
    for w in &mut v.wheels {
        w.surface = 1;
    }
    v.load_transfer(&specs);
    let dry = v.wheels.map(|w| w.limit);
    v.wet_track = true;
    v.load_transfer(&specs);
    for (w, d) in v.wheels.iter().zip(dry) {
        assert!(w.wet);
        assert!((w.limit - 0.9 * d).abs() < 1e-5, "{} vs {}", w.limit, d);
    }
}

#[test]
fn a_racers_origin_is_lifted_and_drawn_back_down() {
    let specs = specs_993();
    let tilt = Mat3::from_rotation_z(0.2).transpose();
    let mut v = Vehicle::new(Vec3::new(1.0, 2.0, 3.0), tilt, &specs, 0.7, 0.3);
    assert_eq!(v.draw_origin(), v.position, "no body yet: drawn at the origin");
    let ground = [Vec3::new(-0.7, 0.0, 1.2), Vec3::new(0.7, 0.0, 1.2), Vec3::new(-0.7, 0.0, -1.1), Vec3::new(0.7, 0.0, -1.1)];
    let before: Vec<Vec3> = ground.iter().map(|p| v.position + tilt.transpose() * *p).collect();
    let half = Vec3::new(0.9, 0.63, 2.1);
    v.set_car_body(ground.iter().map(|p| *p - Vec3::Y * half.y).collect(), half);
    assert!((v.position - (Vec3::new(1.0, 2.0, 3.0) + tilt.row(1) * 0.63)).length() < 1e-6);
    assert!((v.draw_origin() - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-6);
    for (p, b) in v.body_points.iter().zip(&before) {
        assert!((v.position + tilt.transpose() * *p - *b).length() < 1e-5);
    }
    // setting it again does not lift twice
    v.set_car_body(v.body_points.clone(), half);
    assert!((v.draw_origin() - Vec3::new(1.0, 2.0, 3.0)).length() < 1e-6);
}

/// A plane tilted by `grade` (rise per run) about x, so +z is uphill.
/// 0x49BA30 for a racer: always orthonormalised about the ground normal, and the lateral row
/// keeps the body's sense (`z = M[0] × N`, `x = N × z`) on a 45° slope as on a gentle one.
#[test]
fn a_racers_physics_frame_is_orthonormal_about_the_normal() {
    let specs = specs_993();
    for grade in [0.2, 1.0] {
        let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
        v.ground_contact(&slope_grid(grade), &specs);
        let f = v.frame;
        assert!((f.row(1) - v.ground_normal).length() < 1e-6);
        assert!(f.row(0).dot(f.row(1)).abs() < 1e-5 && f.row(2).dot(f.row(1)).abs() < 1e-5, "grade {grade}: {f:?}");
        assert!(f.row(0).x > 0.99, "grade {grade}: lateral row {:?}", f.row(0));
        assert!(f.row(2).z > 0.0, "grade {grade}: forward row {:?}", f.row(2));
    }
}

fn slope_grid(grade: f32) -> TriangleGrid {
    let s = 2000.0;
    let y = |z: f32| z * grade;
    let (a, b, c, d) = (
        Vec3::new(-s, y(-s), -s),
        Vec3::new(s, y(-s), -s),
        Vec3::new(s, y(s), s),
        Vec3::new(-s, y(s), s),
    );
    TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0)
}

fn settled_993() -> (CarSpecs, TriangleGrid, Vehicle) {
    let specs = specs_993();
    let grid = flat_grid();
    let idle = Controls { throttle: 0, brake: 0, steer: 0, handbrake: false, command: 0 };
    let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.7, 0.3);
    v.race_started = true;
    v.ticks_alive = 0x100;
    v.engine_alive = 0x100;
    v.automatic = false;
    // settle onto the plane: the arms only run with the rear axle in contact
    for t in 0..64 {
        if t % 2 == 0 {
            v.step32(&idle, &specs, &grid);
        }
        v.step64(&specs);
    }
    (specs, grid, v)
}

/// 0x49ED94…0x49EDE2's middle rung, in the **driving** arm (`diff > 0`): the wheels turning
/// far faster than the engine (`slip < [0x5B4A4C]` = −200) step the revs **up** by 200 — no
/// snap to the wheel rpm — and the drive is the throttle's share.
#[test]
fn under_target_steps_up_toward_the_wheels() {
    let (specs, grid, mut v) = settled_993();
    let full = Controls { throttle: 252, brake: 0, steer: 0, handbrake: false, command: 0 };
    v.gear = 3;
    v.requested_gear = 3;
    v.velocity = Vec3::new(0.0, 0.0, 20.0);
    v.rpm = 0.0;
    v.step32(&full, &specs, &grid);
    assert_eq!(v.engine_arm, EngineArm::Driving, "under its target");
    assert!(v.rpm >= 199.0 && v.rpm <= 201.0, "one rung up, not a snap to the wheels: {}", v.rpm);
}

/// `AutoShift` 0x49DAF0: at or below half throttle only the kick-down can happen (0x49DC38),
/// and it leaves `car+0xD81` alone (0x49DC92…0x49DCA4); above it the upshift needs
/// `now > up`.
#[test]
fn autoshift_holds_at_part_throttle_and_kicks_down_without_d81() {
    let specs = specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
    let at = |v: &mut Vehicle, gear: usize, rpm_in_gear: f32, throttle: f32| {
        let s = rpm_in_gear / specs.ratio_rpm_per_mps[gear];
        v.velocity = Vec3::new(0.0, 0.0, s);
        v.v_local = v.velocity;
        v.gear = gear;
        v.prev_gear = 1;
        v.rpm = 20000.0;
        v.throttle = throttle;
        v.slip_tangent = 0.0;
    };
    // at the redline in third on a light throttle: no upshift
    at(&mut v, 3, specs.redline, 0.4);
    v.auto_shift(&specs);
    assert_eq!(v.gear, 3);
    // the same on a full throttle: up, and D81 is the old gear
    at(&mut v, 3, specs.redline, 1.0);
    v.auto_shift(&specs);
    assert_eq!((v.gear, v.prev_gear, v.downshift_blip), (4, 3, false));
    // off the throttle and slow for fourth: the kick-down to third, D81 untouched
    at(&mut v, 4, 0.3 * specs.redline, 0.0);
    v.auto_shift(&specs);
    assert_eq!((v.gear, v.prev_gear, v.downshift_blip, v.shift_timer), (3, 1, true, specs.shift_ticks));
}

/// 0x49EC13…0x49ED45: throttle shut at speed (`diff < 0`) is the over-run arm — the revs step
/// toward the wheels by at most `81920 · specs[0x170 + 4·gear]` and the drive is negative,
/// `−a · specs[0x98] · min(1 + 0.02·gearbox, 5)`: engine braking.
#[test]
fn over_run_brakes_and_steps_the_revs_by_the_cap() {
    let (specs, grid, mut v) = settled_993();
    let idle = Controls { throttle: 0, brake: 0, steer: 0, handbrake: false, command: 0 };
    v.gear = 3;
    v.requested_gear = 3;
    v.velocity = Vec3::new(0.0, 0.0, 30.0);
    v.rpm = 0.0;
    let before = v.velocity.z;
    v.step32(&idle, &specs, &grid);
    assert_eq!(v.engine_arm, EngineArm::Overrun, "over-run");
    let cap = specs.mps_per_rpm[3] * 81920.0;
    assert!((v.rpm - cap).abs() < 1.0, "revs {} step by the cap {cap}", v.rpm);
    assert!(v.velocity.z < before, "engine braking slows the car");
}
/// Diagnostic: how fast a 993 gets going from rest, on the flat and up a 14% climb.
#[test]
#[ignore]
fn launch_profile() {
    for grade in [0.0f32, 0.14] {
        let specs = specs_993();
        let grid = slope_grid(grade);
        let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.7, 0.3);
        v.ai_driven = true;
        v.game_tick = 0x400;
        v.race_started = true;
        let c = Controls { throttle: 255, brake: 0, steer: 0, handbrake: false, command: 0 };
        for t in 0..(5 * 64) {
            if t % 2 == 0 {
                v.step32(&c, &specs, &grid);
            }
            v.step64(&specs);
            if t % 32 == 0 {
                println!("grade {grade:.2} t={:.1}s  {:5.1} km/h  rpm {:6.0} gear {} contact {} arm {}", t as f32 / 64.0, v.speed() * 3.6, v.rpm, v.gear, v.wheels.iter().filter(|w| w.contact).count(), v.engine_arm as u8);
            }
        }
    }
}

fn flat_grid() -> TriangleGrid {
    let s = 2000.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0)
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
    // the heading change summed over half-second steps: the car may turn past 90° in 3 s
    // (it does once over-run engine braking slows it, 0x49EC13), where one cross product of
    // the first and last heading would read the sine of the whole turn
    let mut turn = 0.0;
    for _ in 0..6 {
        let h0 = v.forward();
        run(&mut v, &specs, &grid, Controls { throttle: 120, steer: 126, ..Default::default() }, 0.5);
        turn += h0.cross(v.forward()).y.asin();
    }
    eprintln!("turn (cross.y) after 3 s of right steer: {turn:.3}, speed {:.1} km/h", v.speed() * 3.6);
    assert!(turn.abs() > 0.2, "car did not turn");
    // positive steer turns the nose toward body +x (cross.y > 0). Whether +x is screen-right
    // depends on the model frame (RE/notes/car-model.md: +X = car's left) — the input layer maps it.
    assert!(turn > 0.0, "positive steer should turn toward +x (cross.y = {turn})");
}

/// The RALLY nibble rule, against the two inline forms it replaced.
#[test]
fn the_rally_nibble_helper_equals_the_inline_forms() {
    for rally in [false, true] {
        for n in 0..16usize {
            let old_body = if rally && (n == 1 || n == 10) { 2 } else { n };
            let new_body = if super::rally_reads_as_two(n, rally) { 2 } else { n };
            assert_eq!(old_body, new_body);
            let word = 0x1234_5670u32 | n as u32;
            let old_wheel = if rally && (n == 1 || n == 10) { 2 } else { word & 0xFFFF };
            let new_wheel = if super::rally_reads_as_two(n, rally) { 2 } else { word & 0xFFFF };
            assert_eq!(old_wheel, new_wheel);
        }
    }
}

/// The enum field types keep the original's stored values (`car+0xB50`, `car+0x438`, the
/// diagnostic engine arm).
#[test]
fn state_enums_keep_the_original_values() {
    assert_eq!([RaceState::Racing as u8, RaceState::Finished as u8, RaceState::Out as u8], [0, 2, 3]);
    assert_eq!(
        [ContactKind::None, ContactKind::Ground, ContactKind::Wall, ContactKind::PropOrCar, ContactKind::Bush].map(ContactKind::word),
        [0, 0x30000, 0x40000, 0x50000, 0x60000]
    );
    assert_eq!(
        [EngineArm::None, EngineArm::Clutch, EngineArm::Overrun, EngineArm::AtTarget, EngineArm::Misfire, EngineArm::Driving].map(|a| a as u8),
        [0, 1, 2, 3, 4, 5]
    );
    let specs = specs_993();
    let v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.7, 0.3);
    assert_eq!((v.race_state, v.contact_kind, v.engine_arm), (RaceState::Racing, ContactKind::None, EngineArm::None));
}
