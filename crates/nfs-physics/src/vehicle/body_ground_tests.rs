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
    v.body_ground_collide_mass(specs.mass, Vec3::Y, plane, 0.5);
    assert!(v.velocity.y > -8.0, "downward speed reduced: {:?}", v.velocity);
    assert!(v.position.y >= 0.2, "pushed out of the ground: {}", v.position.y);
    assert!(v.angular_velocity.abs().max_element() <= 0.75 + 1e-6, "ω clamped: {:?}", v.angular_velocity);
    assert!(v.impact > 0.0);
    // a light landing leaves too little residual normal speed to damage the body
    // (`Damage_byNormal` needs an effect > 10, i.e. `4·|vel·n| > 10`)
    assert!(v.damage.iter().all(|d| *d == 0.0));
}

/// `GroundImpulse` 0x495F30 with a contact that is not closing (`j < 0.001`, 0x496303): the
/// velocity and the spin only lose a tenth — no friction, no pull toward the ground.
#[test]
fn a_separating_ground_contact_only_damps() {
    let specs = tests::specs_993();
    let flipped = Mat3::from_rotation_z(std::f32::consts::PI).transpose();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.2, 0.0), flipped, &specs, 0.8, 0.3);
    v.velocity = Vec3::new(3.0, 2.0, 0.0);
    v.angular_velocity = Vec3::new(0.1, 0.0, 0.0);
    v.cos_tilt = -1.0;
    v.body_ground_collide_mass(specs.mass, Vec3::Y, Vec3::new(0.0, 0.25, 0.0), 0.5);
    assert!((v.velocity - Vec3::new(2.7, 1.8, 0.0)).length() < 1e-5, "{:?}", v.velocity);
    assert!((v.angular_velocity - Vec3::new(0.09, 0.0, 0.0)).length() < 1e-6, "{:?}", v.angular_velocity);
}

/// The settle (0x4963E1…0x4964C0) tests `j` in the original's units (kg / 128) against 10:
/// a 7000 kg smackable landing at 1 m/s has `j` well past it and is not settled, a 20 kg one is.
#[test]
fn the_ground_settle_is_on_j() {
    let land = |mass: f32| {
        let mut v = Vehicle::bare(Vec3::new(0.0, 0.0, 0.0), Mat3::IDENTITY, 0.0, mass, 0.0, 0.0);
        v.smackable = true;
        v.body_points = vec![Vec3::new(-0.1, 0.0, -0.1), Vec3::new(0.1, 0.0, -0.1), Vec3::new(-0.1, 0.0, 0.1), Vec3::new(0.1, 0.0, 0.1)];
        v.velocity = Vec3::new(0.0, -1.0, 0.0);
        v.min_height = 0.0;
        v.body_ground_collide_mass(mass, Vec3::Y, Vec3::ZERO, 0.5);
        v.velocity.y
    };
    // j = 0.3 · 1 / (0.25 / m) = 1.2 m: the light one settles (×0.95), the heavy one does not
    let (light, heavy) = (land(20.0), land(7000.0));
    assert!((light - (-1.0 + 0.6) * 0.95).abs() < 1e-4, "{light}");
    assert!((heavy - (-1.0 + 0.6)).abs() < 1e-4, "{heavy}");
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
    v.body_ground_collide_mass(specs.mass, Vec3::Y, Vec3::ZERO, 0.5);
    assert!(v.damage[8] > 0.0, "roof zone damaged: {:?}", v.damage);
}

/// `Damage`'s crash response (0x49481A): a heavy wall hit on the front hops the car
/// (`a·0.125/3` on the velocity's `y`, doubled for `.TINYCAR`) and pitches it back; the
/// body-ground contact (flag 0) does neither; a light hit only knocks the spin.
/// 0x494E84 → 0x494E87: a roof contact that also leans to a side damages the roof **and** the
/// ring zone; a normal on a ±0.1 boundary picks no ring zone (0x495002).
#[test]
fn roof_contact_falls_through_to_the_ring() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.damage_by_normal(60.0, Vec3::new(0.0, -0.8, -0.6), 0, 1.0);
    assert_eq!(v.damage[8], 30.0, "roof");
    assert_eq!(v.damage[1], 30.0, "and the front");
    let mut w = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    w.damage_by_normal(60.0, Vec3::new(0.1, 0.0, -0.99), 0, 1.0);
    assert_eq!(w.damage, [0.0; 10], "x exactly 0.1: no zone");
}

/// 0x49453C: under `DERBY` the amount is ×1.5 before the cap at 100.
#[test]
fn derby_damage_is_one_and_a_half() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.derby = true;
    v.damage_by_normal(60.0, Vec3::new(0.0, 0.0, -1.0), 0, 1.0);
    assert_eq!(v.damage[1], 90.0);
    v.damage_by_normal(80.0, Vec3::new(0.0, 0.0, -1.0), 0, 1.0);
    assert_eq!(v.damage[1], 100.0);
}

/// 0x494521…0x494529: a car that has finished (`car+0xB50 != 0`) takes no zone damage.
#[test]
fn a_finished_car_takes_no_zone_damage() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.race_state = RaceState::Finished;
    v.damage_by_normal(60.0, Vec3::new(0.0, 0.0, -1.0), 0, 1.0);
    assert_eq!(v.damage, [0.0; 10]);
}

#[test]
fn crash_response_follows_the_flag() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.damage_by_normal(200.0, Vec3::new(0.0, 0.0, -1.0), 0, 1.0);
    assert_eq!(v.velocity, Vec3::ZERO);
    assert_eq!(v.angular_velocity, Vec3::ZERO);
    v.damage_by_normal(200.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
    assert!((v.velocity.y - 200.0 * 0.125 / 3.0).abs() < 1e-3, "{:?}", v.velocity);
    // zone 1 (front): pitch −m·k/2 with k = min(25/32, 2), m = 1.2, then clamped to 0.75
    let k = 25.0f32 / 32.0;
    assert!((v.angular_velocity.x + 1.2 * k * 0.5).abs() < 1e-4, "{:?}", v.angular_velocity);
    assert!((v.angular_velocity.y + 1.2 * k * 0.5).abs() < 1e-4);
    let mut t = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
    t.tiny = true;
    t.damage_by_normal(200.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
    assert!((t.velocity.y - 400.0 * 0.125 / 3.0).abs() < 1e-3);
    let mut l = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
    l.damage_by_normal(20.0, Vec3::new(0.0, 0.0, 1.0), 1, 1.0);
    assert_eq!(l.velocity, Vec3::ZERO);
    // zone 5 (rear): pitch +k/2, yaw −k/2 with k = min(20/32, 0.33)
    assert!((l.angular_velocity.x - 0.165).abs() < 1e-5, "{:?}", l.angular_velocity);
    assert!((l.angular_velocity.y + 0.165).abs() < 1e-5);
}

/// 0x40F740's countdown: a heavy hit starts it; an upright live car stops it; a dead car runs
/// it out and is reset, and the reset re-arms the alive count at 1.
#[test]
fn crash_countdown_and_dead_car() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.cos_tilt = 1.0;
    v.damage_by_normal(100.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
    assert_eq!(v.reset_countdown, 0xC0);
    assert!(v.engine_alive != 0, "100 does not kill");
    assert!(!v.reset_countdown_tick(false));
    assert_eq!(v.reset_countdown, 0, "upright and alive: stopped");
    v.damage_by_normal(130.0, Vec3::new(0.0, 0.0, -1.0), 1, 1.0);
    assert_eq!(v.engine_alive, 0, "past 120: dead");
    let mut n = 0;
    while !v.reset_countdown_tick(false) {
        n += 1;
        assert!(n < 0xC0);
    }
    assert_eq!(n, 0xBF);
    v.reset_to(Vec3::ZERO, Mat3::IDENTITY);
    assert_eq!(v.engine_alive, 1);
    v.reset_countdown_start(Vec3::Y, false);
    assert_eq!(v.reset_countdown, 0, "upright on the slice: no restart");
    v.orientation = Mat3::from_rotation_z(std::f32::consts::FRAC_PI_2);
    v.reset_countdown_start(Vec3::Y, false);
    assert_eq!(v.reset_countdown, 0xC0, "on its side: started");
}

/// 0x40F7EB's gates: not before tick 0x301, not at 27.6 on `car+0x35C`, not once finished, and
/// a request that goes through holds the next one off for 0xA0 passes.
#[test]
fn reset_request_gates_and_hold() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    assert!(!v.reset_request(true, 0x300));
    v.velocity = Vec3::new(27.6, 0.0, 0.0);
    assert!(!v.reset_request(true, 0x400));
    // the estimate of (20, 20) is 25, under 27.6, though the true speed is 28.3
    v.velocity = Vec3::new(20.0, 0.0, 20.0);
    assert!(v.reset_request(true, 0x400));
    assert_eq!(v.reset_hold, 0x9F);
    for _ in 0..0x9F {
        assert!(!v.reset_request(true, 0x400));
    }
    assert!(v.reset_request(true, 0x400), "the hold has run out");
    v.race_state = RaceState::Finished;
    v.reset_hold = 0;
    assert!(!v.reset_request(true, 0x400), "finished");
}

/// No body point below the plane: nothing happens.
#[test]
fn airborne_car_is_untouched() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::new(0.0, 10.0, 0.0), Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.velocity = Vec3::new(1.0, -2.0, 3.0);
    let before = (v.position, v.velocity, v.angular_velocity);
    v.body_ground_collide_mass(specs.mass, Vec3::Y, Vec3::ZERO, 0.5);
    assert_eq!((v.position, v.velocity, v.angular_velocity), before);
}
