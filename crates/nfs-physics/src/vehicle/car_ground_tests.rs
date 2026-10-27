use super::*;
use crate::collision::Triangle;

/// A flat plane at y = 0, drivable (surface 1).
fn flat() -> TriangleGrid {
    let s = 50.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 10.0)
}

#[test]
fn a_wheel_off_every_patch_reads_surface_zero_and_the_kept_plane() {
    // road (surface 3) only in front of z = 0; the rear wheels hang over nothing
    let s = 50.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, 0.0), Vec3::new(s, 0.0, 0.0), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    let grid = TriangleGrid::build(vec![Triangle::new(a, b, c, 3), Triangle::new(a, c, d, 3)], 10.0);
    let specs = super::tests::specs_993();
    let mut v = Vehicle::bare(Vec3::new(0.0, 0.0, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    v.tracker.plane_point = Vec3::new(0.0, -0.5, 0.0);
    v.wheels[2].surface = 5;
    v.ground_contact(&grid, &specs);
    assert_eq!(v.wheels[0].surface, 3);
    assert_eq!(v.wheels[1].surface, 3);
    assert_eq!(v.wheels[2].surface, 0, "no patch: the tracker's zone word 0");
    assert_eq!(v.wheels[3].surface, 0);
    // the rear wheels stood on the kept plane `car+0x400` (0.5 m down: hanging)
    assert!(v.wheels[2].airborne && v.wheels[3].airborne && !v.wheels[0].airborne);
}

/// 0x49B090: the offset in the slice frame, then the drop by `car+0x424` puts the wheel quad
/// on the road and the origin `car+0x3AC` above it; an opponent after tick 0x1C0 takes the
/// offset as a world position.
#[test]
fn place_at_slice_drops_the_body_onto_the_road() {
    let grid = flat();
    let specs = super::tests::specs_993();
    let quad = [(-0.66, 1.2), (0.66, 1.2), (-0.66, -1.1), (0.66, -1.1)];
    let mut pts: Vec<Vec3> = quad.iter().map(|&(x, z)| Vec3::new(x, 0.0, z)).collect();
    pts.push(Vec3::new(0.0, 1.26, 0.0));
    pts.iter_mut().for_each(|p| p.y -= 0.631);
    body_points_setup(&mut pts);
    let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
    v.set_car_body(pts, Vec3::new(0.91, 0.631, 2.1));
    v.game_tick = 0;
    v.place_at_slice(&grid, &specs, SliceFrame { centre: Vec3::new(5.0, 0.0, 5.0), forward: Vec3::Z, raw: [Vec3::X, Vec3::Y, Vec3::Z] }, Vec3::new(1.5, 0.5, 0.0), true, false);
    assert!((v.position - Vec3::new(6.5, 0.631, 5.0)).length() < 1e-4, "{:?}", v.position);
    assert!((v.draw_origin().y).abs() < 1e-4);
    assert_eq!(v.body_height, 0.0);
    v.game_tick = 0x1C1;
    v.place_at_slice(&grid, &specs, SliceFrame { centre: Vec3::new(5.0, 0.0, 5.0), forward: Vec3::Z, raw: [Vec3::X, Vec3::Y, Vec3::Z] }, Vec3::new(-2.0, 0.0, 7.0), true, true);
    assert!((v.position - Vec3::new(-2.0, 0.631, 7.0)).length() < 1e-4, "{:?}", v.position);
    assert!((v.orientation.row(2) - Vec3::NEG_Z).length() < 1e-5, "flipped: forward negated");
}

/// 0x410430: the skill start puts `car+0x330` on the trigger (on the pad here), turns the
/// frame onto the patch normal with the trigger's `+0xC8` as forward, and one `Susp_update`
/// lifts the body until the wheel quad stands on the pad — the origin half the box up.
#[test]
fn place_at_trigger_stands_the_car_on_the_pad() {
    let grid = flat();
    let specs = super::tests::specs_993();
    let quad = [(-0.66, 1.2), (0.66, 1.2), (-0.66, -1.1), (0.66, -1.1)];
    let mut pts: Vec<Vec3> = quad.iter().map(|&(x, z)| Vec3::new(x, 0.0, z)).collect();
    pts.push(Vec3::new(0.0, 1.26, 0.0));
    pts.iter_mut().for_each(|p| p.y -= 0.631);
    body_points_setup(&mut pts);
    let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
    v.set_car_body(pts, Vec3::new(0.91, 0.631, 2.1));
    // the shipped `+0xC8` headings are unit and horizontal (`*_st*.scn`, line 6); 0x4A4810
    // only goes part of the way to orthonormal (`Vehicle::renormalise`), so the frame is as
    // unit as its inputs
    v.place_at_trigger(&grid, &specs, Vec3::new(-6.8, 0.0, 14.0), Vec3::new(0.0, 0.0, -1.0));
    assert!((v.position - Vec3::new(-6.8, 0.631, 14.0)).length() < 1e-4, "{:?}", v.position);
    assert!(v.draw_origin().y.abs() < 1e-4);
    assert!((v.orientation.row(2) - Vec3::NEG_Z).length() < 1e-5, "forward = +0xC8");
    assert!((v.orientation.row(1) - Vec3::Y).length() < 1e-5);
    assert_eq!(v.body_height, 0.0);
}

/// 0x4A4810: four passes of `M ← M·(I − D/8 + 3D²/32 − 5D³/64)`, `D = MᵀM − I` — an
/// orthonormal matrix is left alone, a stretched one is only brought part of the way back.
#[test]
fn the_renormalisation_is_the_originals_quarter_series() {
    let mut v = Vehicle::bare(Vec3::ZERO, Mat3::from_rotation_y(0.3), 2.3, 1300.0, 0.8, 0.3);
    let before = v.orientation;
    v.renormalise();
    assert!((v.orientation.row(0) - before.row(0)).length() < 1e-6);
    // rows 1 % too long: the square error 0.0201 falls by (¾)⁴-ish per call
    v.orientation = Mat3::IDENTITY * 1.01;
    v.renormalise();
    let e = v.orientation.row(0).length_squared() - 1.0;
    assert!(e > 0.004 && e < 0.008, "{e}");
}

/// `wheel+0x8C` keeps the whole word (0x499B84…0x499B8A); `RALLY` turns nibble 1 or 10 into
/// the dword 2 (0x499BA7).
#[test]
fn the_wheel_keeps_the_whole_zone_word() {
    let s = 50.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    let grid = TriangleGrid::build(vec![Triangle::new(a, b, c, 0x0101), Triangle::new(a, c, d, 0x0101)], 10.0);
    let specs = super::tests::specs_993();
    let mut v = Vehicle::bare(Vec3::ZERO, Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    v.ground_contact(&grid, &specs);
    assert!(v.wheels.iter().all(|w| w.zone == 0x0101 && w.surface == 1));
    v.rally = true;
    v.ground_contact(&grid, &specs);
    assert!(v.wheels.iter().all(|w| w.zone == 2 && w.surface == 2));
}

#[test]
fn body_setup_sorts_by_height_and_deals_the_lowest_four_by_quadrant() {
    // the 993's wheel quad as `BndP` stores it (model axes), a roof point and a box corner
    let mut p = vec![
        Vec3::new(0.0, 1.2, 0.0),
        Vec3::new(0.66, 0.0, -1.075),
        Vec3::new(-0.66, 0.0, -1.075),
        Vec3::new(0.9, 0.3, 2.0),
        Vec3::new(0.66, -0.001, 0.926),
        Vec3::new(-0.66, 0.0, 0.926),
    ];
    body_points_setup(&mut p);
    assert_eq!(
        &p[..4],
        &[Vec3::new(-0.66, 0.0, 0.926), Vec3::new(0.66, -0.001, 0.926), Vec3::new(-0.66, 0.0, -1.075), Vec3::new(0.66, 0.0, -1.075)]
    );
    assert_eq!(&p[4..], &[Vec3::new(0.9, 0.3, 2.0), Vec3::new(0.0, 1.2, 0.0)]);
    // a point on an axis reaches no slot, which stays zero; a second one in a slot wins
    let mut q = vec![Vec3::new(0.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(2.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, -1.0)];
    body_points_setup(&mut q);
    assert_eq!(q, vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, -1.0), Vec3::ZERO]);
}

#[test]
fn body_height_reads_the_first_four_points_upright_and_all_of_them_tilted() {
    let mut v = Vehicle::bare(Vec3::new(0.0, 1.0, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    // four tyre points 1 m below the centre at 0.1, 0.2, 0.3, 0.4 above the plane, then a
    // body point that pokes 0.05 below it
    v.body_points = vec![
        Vec3::new(-0.8, -0.9, 1.2),
        Vec3::new(0.8, -0.8, 1.2),
        Vec3::new(-0.8, -0.7, -1.2),
        Vec3::new(0.8, -0.6, -1.2),
        Vec3::new(0.0, -1.05, 0.0),
    ];
    v.cos_tilt = 1.0;
    v.body_height_on(Vec3::Y, Vec3::ZERO);
    // ½·(mean + min) of the first four, into both fields (0x49AA46 / 0x49AA4C)
    let want = 0.5 * (0.25 + 0.1);
    assert!((v.body_height - want).abs() < 1e-5 && v.min_height == v.body_height, "{} {}", v.body_height, v.min_height);
    v.cos_tilt = 0.9;
    v.body_height_on(Vec3::Y, Vec3::ZERO);
    assert!((v.body_height + 0.05).abs() < 1e-5, "{}", v.body_height);
    assert!((v.min_height - (-0.05 + 0.1 + 0.2) / 3.0).abs() < 1e-5, "{}", v.min_height);
}

/// 0x49C095…0x49C0E6: a car on its roof with damage on and the roof under 40 has the roof set
/// to 40 and the roll-over event raised, once.
#[test]
fn a_rolled_car_takes_forty_on_the_roof_once() {
    let mut v = Vehicle::bare(Vec3::new(0.0, 0.5, 0.0), Mat3::from_diagonal(Vec3::new(1.0, -1.0, -1.0)), 2.5, 1300.0, 0.8, 0.3);
    v.cos_tilt = -1.0;
    v.body_height = 0.0;
    v.min_height = 0.0;
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: Vec3::ZERO, surf: 1 }, 1300.0, None, &flat(), 1, true);
    assert!(v.rollover_sound && v.damage[8] == 40.0);
    v.rollover_sound = false;
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: Vec3::ZERO, surf: 1 }, 1300.0, None, &flat(), 1, true);
    assert!(!v.rollover_sound, "the roof is at 40 now");
    // the direction of travel follows the velocity, or the forward row at rest
    assert!(v.travel_dir.is_normalized());
}

/// 0x49C1C4…0x49C27D: a hard landing raises the event, clears the `+0x418` word when upright
/// and, for the human, 0x493DD0 clears the wheels' slip fractions `wheel+0xB8` (0x493E1C) —
/// not their airborne flags `wheel+0x84`, which it leaves alone.
#[test]
fn a_hard_landing_resets_the_human_car() {
    let mut v = Vehicle::bare(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    v.cos_tilt = 1.0;
    v.body_height = -0.2;
    v.min_height = -0.2;
    v.velocity.y = -10.0;
    v.airborne_count = 1;
    for w in &mut v.wheels {
        w.airborne = true;
        w.slip = 0.5;
    }
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: Vec3::ZERO, surf: 1 }, 1300.0, None, &flat(), 1, true);
    assert!(v.landing_sound.is_some_and(|c| c > 6.0));
    assert_eq!(v.airborne_count, 0);
    assert!(v.wheels.iter().all(|w| w.slip == 0.0 && w.airborne));
}

#[test]
fn a_tilted_car_off_its_body_falls_through_to_the_resting_arm() {
    // 0x49BF62: `car+0x424 < 0.001` fails, so the upright arm 0x49C19A sets the body on the
    // plane (`car+0x424 < 0.005`, closing ≤ 6)
    let grid = flat();
    let mut v = Vehicle::bare(Vec3::new(0.0, 0.003, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    v.cos_tilt = 0.8;
    v.body_height = 0.003;
    v.min_height = 0.003;
    v.velocity = Vec3::new(5.0, -0.1, 0.0);
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: Vec3::ZERO, surf: 1 }, 1300.0, None, &grid, 1, true);
    assert!(v.position.y.abs() < 1e-6, "{:?}", v.position);
    assert_eq!(v.body_height, 0.0);
    assert_eq!(v.velocity, Vec3::new(5.0, 0.0, 0.0));
}

#[test]
fn the_rollover_bounce_reads_the_velocity_from_before_the_impulse() {
    // 0x49C038: `vel.y −= bounce[surf] · vy` only while `vy < −2`; a body moving up gains nothing
    let grid = flat();
    let far = Vec3::new(0.0, -100.0, 0.0); // no body point reaches this plane: no impulse
    let mut v = Vehicle::bare(Vec3::new(0.0, 10.0, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    v.cos_tilt = 0.5;
    v.body_height = 0.0;
    v.min_height = 0.0;
    v.velocity = Vec3::new(0.0, 3.0, 0.0);
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: far, surf: 1 }, 1300.0, None, &grid, 1, true);
    assert_eq!(v.velocity.y, 3.0);
    v.velocity = Vec3::new(1.0, -4.0, 0.0);
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: far, surf: 1 }, 1300.0, None, &grid, 1, true);
    // surface 1's bounce is 0.7: −4 + 0.7·4
    assert!((v.velocity.y - (-4.0 + 0.7 * 4.0)).abs() < 1e-5, "{:?}", v.velocity);
    assert!((v.velocity.x - 0.95).abs() < 1e-6);
}

#[test]
fn a_hard_landing_clamps_the_spin_per_component() {
    // 0x49BBA0 clamps each of ω's components to ±0.75, not its length
    let grid = flat();
    let mut v = Vehicle::bare(Vec3::new(0.0, -0.2, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
    v.cos_tilt = 1.0;
    v.body_height = -0.2;
    v.min_height = -0.2;
    v.velocity = Vec3::new(0.0, -8.0, 0.0);
    v.angular_velocity = Vec3::new(0.7, -0.7, 2.0);
    v.car_ground_body(GroundContact { dh: 0.0, n: Vec3::Y, q: Vec3::ZERO, surf: 1 }, 1300.0, None, &grid, 1, true);
    assert_eq!(v.angular_velocity, Vec3::new(0.7, -0.7, 0.75));
    assert!((v.position.y + 0.05).abs() < 1e-6);
    assert!((v.velocity.y - 0.4 * 0.7 * 8.0).abs() < 1e-5, "{:?}", v.velocity);
}

/// 0x49B40C…0x49B41E: when 0x49AF70 finds the final position on another patch than the slice
/// centre's, its refresh copies the slice record's own vectors into `car+0x3B8…`, and 0x417260 /
/// 0x417310 copy them into the orientation and the physics frame — the raw, non-orthonormal slice
/// frame (alps grid slice 44, logged live in `REFERENCE-2026-10-22-PA-TRACE.md`), lateral and
/// forward negated under `flip`. On the centre's own patch nothing refreshes and the frame built at
/// 0x49B22F stays. (The flat grid's two triangles meet on `x = z`.)
#[test]
fn a_placed_car_takes_the_raw_slice_frame() {
    let grid = flat();
    let specs = super::tests::specs_993();
    let lat = Vec3::new(-0.924_635, 0.0, 0.380_855);
    let up = Vec3::new(0.012_049, 0.999_434, 0.031_408);
    let fwd = Vec3::new(-0.357_964, 0.033_64, -0.933_129);
    let rows = |v: &Vehicle| (v.orientation.row(0), v.orientation.row(1), v.orientation.row(2));
    let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
    // centre on (x > z), the corner pushed 1.5 m along the built lateral across `x = z`
    v.place_at_slice(&grid, &specs, SliceFrame { centre: Vec3::new(5.0, 0.0, 4.9), forward: fwd, raw: [lat, up, fwd] }, Vec3::new(1.5, 0.5, 0.0), false, false);
    assert_eq!(rows(&v), (lat, up, fwd));
    assert_eq!(v.frame, v.orientation);
    // flipped, from the other side of the line
    let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
    v.place_at_slice(&grid, &specs, SliceFrame { centre: Vec3::new(4.9, 0.0, 5.0), forward: fwd, raw: [lat, up, fwd] }, Vec3::new(-1.5, 0.5, 0.0), false, true);
    assert_eq!(rows(&v), (-lat, up, -fwd));
    // the same patch: the built frame (up = the patch normal, orthonormal)
    let mut v = Vehicle::bare(Vec3::new(0.0, 3.0, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
    v.place_at_slice(&grid, &specs, SliceFrame { centre: Vec3::new(20.0, 0.0, 0.0), forward: fwd, raw: [lat, up, fwd] }, Vec3::new(1.5, 0.5, 0.0), false, false);
    assert_eq!(v.orientation.row(1), Vec3::Y);
    assert!((v.orientation.row(0).dot(v.orientation.row(2))).abs() < 1e-6);
}

/// 0x49BA01…0x49BA1D: the byte countdown `car+0x80` (0 on a placed racer) runs 0x4A4810 on the
/// 256th 64 Hz rotation step and every 32nd after it — the original's first one on the alps grid
/// was at game tick 255, measured live.
#[test]
fn the_orientation_is_reorthonormalised_on_its_own_countdown() {
    let skew = Mat3::from_cols(Vec3::new(-0.924_635, 0.0, 0.380_855), Vec3::new(0.012_049, 0.999_434, 0.031_408), Vec3::new(-0.357_964, 0.033_64, -0.933_129)).transpose();
    let mut v = Vehicle::bare(Vec3::ZERO, skew, 2.3, 1300.0, 0.8, 0.3);
    for _ in 0..255 {
        v.integrate64();
    }
    assert_eq!(v.orientation, skew, "untouched for 255 steps");
    v.integrate64();
    assert_ne!(v.orientation, skew, "the 256th step re-orthonormalises");
    assert_eq!(v.orientation, renormalise_matrix(skew));
    let once = v.orientation;
    for _ in 0..31 {
        v.integrate64();
    }
    assert_eq!(v.orientation, once);
    v.integrate64();
    assert_eq!(v.orientation, renormalise_matrix(once), "then every 32nd");
}
