use super::*;

fn car(pos: Vec3, vel: Vec3, yaw: f32) -> Vehicle {
    let specs = super::tests::specs_993();
    let rot = Mat3::from_rotation_y(yaw).transpose();
    let mut v = Vehicle::new(pos, rot, &specs, 0.8, 0.3);
    v.velocity = vel;
    v
}

/// `0x497B10`: each pass moves the pair 1/32 m apart along the line of centres, split by mass,
/// until `0x496FD0` finds the boxes clear — at most 32 passes, so a deep interlock is not
/// cleared in one call.
#[test]
fn separation_moves_a_thirty_second_a_pass() {
    let mut a = car(Vec3::ZERO, Vec3::ZERO, 0.0);
    let x = a.half_extents.x * 2.0 - 0.05;
    let mut b = car(Vec3::new(x, 0.0, 0.0), Vec3::ZERO, 0.0);
    assert!(box_contact(&a, &b).is_some(), "the pair has to start overlapping");
    let before = b.position.x - a.position.x;
    collide_cars(&mut a, 1.0, &mut b, 3.0);
    assert!(box_contact(&a, &b).is_none());
    let moved = (b.position.x - a.position.x) - before;
    // two passes of 1/32 clear a 5 cm overlap; A (the lighter) takes three quarters of each
    assert!((moved - 2.0 / 32.0).abs() < 1e-4, "moved {moved}");
    assert!((a.position.x + 2.0 * 0.75 / 32.0).abs() < 1e-4, "{:?}", a.position);
}

/// `0x496FD0`'s first hit is a face centre of B inside A, with B's face normal — here B's −x
/// face, pointing from B toward A.
#[test]
fn box_contact_samples_faces_first() {
    let a = car(Vec3::ZERO, Vec3::ZERO, 0.0);
    let x = a.half_extents.x * 2.0 - 0.05;
    let b = car(Vec3::new(x, 0.0, 0.0), Vec3::ZERO, 0.0);
    let (p, n) = box_contact(&a, &b).unwrap();
    assert!((n - Vec3::NEG_X).length() < 1e-5, "{n:?}");
    assert!((p.x - (x - b.half_extents.x)).abs() < 1e-5, "{p:?}");
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

/// 0x496F2C…0x496F88: an impulse leaves both cars' `car+0x440` at the midpoint of their
/// positions (taken after the separation, before `Damage`).
#[test]
fn car_car_contact_point_is_the_midpoint() {
    let mut a = car(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 8.0), 0.0);
    let mut b = car(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, -8.0), 0.0);
    let (pa, pb) = (a.position, b.position);
    assert!(collide_cars(&mut a, 1000.0, &mut b, 1000.0).is_some());
    assert_eq!(a.last_contact, b.last_contact);
    // the separation moved them first, so the midpoint is of the separated pair
    assert!((a.last_contact - (pa + pb) * 0.5).length() < 0.1, "{:?}", a.last_contact);
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

/// 0x496CC3…0x496D4D / 0x496F38…0x496FB9: the damage normal is `normalize(A.pos − B.pos)` for
/// A and its negation for B — a rear-end shunt dents the chaser's nose and the leader's tail.
#[test]
fn a_rear_end_shunt_damages_the_chasers_nose_and_the_leaders_tail() {
    let mut a = car(Vec3::ZERO, Vec3::new(0.0, 0.0, 10.0), 0.0);
    let mut b = car(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO, 0.0);
    collide_cars(&mut a, 2000.0, &mut b, 1000.0);
    assert!(a.damage[1] > 0.0 && a.damage[5] == 0.0, "A: {:?}", a.damage);
    assert!(b.damage[5] > 0.0 && b.damage[1] == 0.0, "B: {:?}", b.damage);
}
