use super::*;

/// `physics.md` §4 item 4, corrected 2026-09-21: the `1.75 − 0.025·speed` factor applies
/// **below** 30 m/s (0x49F5F0 compares, 0x49F5FB's `test ah,1; je` skips when `speed >= 30`).
///
/// This used to assert a closure it defined itself — a copy of the formula, not the port — so
/// it could never catch the port drifting. It now drives [`Vehicle::set_wheel_steer`] at full
/// lock and reads the wheel back (2026-09-21, found by the unused `use super::*`).
/// 0x49F6CC: `car+0xAC8` follows the wheel angle half way each step; a reset clears it.
#[test]
fn smoothed_steering() {
    let specs = super::tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.set_wheel_steer(&specs, 127.0, 10.0, 10.0);
    let a = v.wheels[0].steer;
    assert!((v.steer_smoothed[0] - a * 0.5).abs() < 1e-5);
    v.set_wheel_steer(&specs, 127.0, 10.0, 10.0);
    assert!((v.steer_smoothed[0] - a * 0.75).abs() < 1e-5);
    v.rearm_after_reset();
    assert_eq!(v.steer_smoothed, [0.0; 2]);
}

#[test]
fn the_steering_taper_is_a_low_speed_boost_not_a_high_speed_cut() {
    let specs = super::tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    // the wheel at full lock, less the fixed offset, at one speed (below 80 m/s, so the
    // `1/(v·0.0125)` arm stays out of it)
    let mut lock = |speed: f32| {
        v.set_wheel_steer(&specs, 127.0, speed, speed);
        v.wheels[0].steer - specs.steer_offset
    };
    let (rest, at30, at70) = (lock(0.0), lock(30.0), lock(70.0));
    assert!(rest > at70, "the factor is a boost below 30 m/s: {rest} at rest vs {at70} at 70 m/s");
    assert!((at30 - at70).abs() < 1e-4, "and flat from 30 m/s up: {at30} vs {at70}");
    // the old reading went negative, which is what took the steering away at speed
    assert!(at70 > 0.0);
    // where the 20-unit clamp does not bite, the boost at rest is exactly 1.75
    if specs.max_steer * 1.75 <= 20.0 {
        assert!((rest / at70 - 1.75).abs() < 1e-3, "rest/fast = {}", rest / at70);
    }
}

/// A bare `fistp` rounds to nearest with ties to even — 0x49F3EE and 0x49F4C9 both use it
/// where `physics.md` §4 writes `int()`.
#[test]
fn the_fistp_helper_rounds_to_nearest_with_ties_to_even() {
    assert_eq!(super::fistp(0.4), 0);
    assert_eq!(super::fistp(0.6), 1);
    assert_eq!(super::fistp(-0.6), -1);
    // ties go to the even neighbour, both signs
    assert_eq!(super::fistp(0.5), 0);
    assert_eq!(super::fistp(1.5), 2);
    assert_eq!(super::fistp(2.5), 2);
    assert_eq!(super::fistp(-0.5), 0);
    assert_eq!(super::fistp(-1.5), -2);
    assert_eq!(super::fistp(-2.5), -2);
    // and it is not truncation: this is the case `as i32` gets wrong
    assert_ne!(super::fistp(0.9), 0.9_f32 as i32);
}

/// And the roll split: `speed < 10 → (speed+30)·0.025`, otherwise `(speed+70)·0.0125`
/// (0x49F2F0 / 0x49F301). Continuous at 10, which is why only the branch order settles it.
#[test]
fn the_roll_factor_arms_meet_at_ten_metres_a_second() {
    let r = |speed: f32| if speed < 10.0 { (speed + 30.0) * 0.025 } else { (speed + 70.0) * 0.0125 };
    assert!((r(9.99) - 1.0).abs() < 0.001);
    assert!((r(10.0) - 1.0).abs() < 1e-6);
    // below 10 the slope is twice as steep as above it
    assert!((r(0.0) - 0.75).abs() < 1e-6);
    assert!((r(50.0) - 1.5).abs() < 1e-6);
}

/// `vehicle.rs:2448`, corrected 2026-09-27: `physics_step` reads the kept `car+0x35C`
/// ([`Vehicle::speed_est`], `Car_ground`'s estimate stored on the *previous* tick), not a
/// fresh [`Vehicle::horizontal_speed`] of `self.velocity` — 0x49F2EA is a bare
/// `fld dword ptr [esi+0x35c]`. Isolated on the roll term, which the branch `speed < 10.0`
/// alone decides: with `vl.x == 0` a front wheel's `slip_vel.x` is exactly `roll`. Here
/// `speed_est` (5, the `<10` arm) and `self.velocity`'s own horizontal speed (50, the `>=10`
/// arm) disagree, so the two arms give visibly different wheel slip — the recompute-from-
/// velocity bug picked the wrong one.
#[test]
fn physics_step_reads_the_kept_speed_estimate_not_the_velocity() {
    let specs = super::tests::specs_993();
    let mut v = Vehicle::bare(Vec3::ZERO, Mat3::IDENTITY, specs.wheelbase, 1300.0, 0.8, 0.3);
    v.angular_velocity.y = 1.0;
    // world velocity along z only (vl.x stays 0 under the identity frame, isolating roll);
    // its own horizontal_speed() is 50 — the >=10 arm, the wrong one if recomputed here.
    v.velocity = Vec3::new(0.0, 0.0, 50.0);
    // the kept estimate from the previous tick's `Car_ground` — the <10 arm, the right one.
    v.speed_est = 5.0;
    v.physics_step(&specs);
    let yaw = specs.yaw_lever * 1.0 * 32.0 * -0.5;
    let expected = (5.0 + 30.0) * yaw * 0.025;
    let wrong = (50.0 + 70.0) * yaw * 0.0125;
    assert!(
        (v.wheels[0].slip_vel[0] - expected).abs() < 1e-3,
        "front wheel lat = {}, expected {expected} (from speed_est)",
        v.wheels[0].slip_vel[0]
    );
    assert!(
        (v.wheels[0].slip_vel[0] - wrong).abs() > 1e-3,
        "must not match {wrong}, the value a recompute from velocity would give"
    );
}
