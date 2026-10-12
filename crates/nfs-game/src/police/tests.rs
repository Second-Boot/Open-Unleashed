use super::*;
use crate::race::Slice;
use crate::test_support::Road;
use glam::Vec3;
use nfs_formats::cop::CopRecord;

fn ring(n: usize) -> SliceTable {
    crate::test_support::ring_of(n, Road::two_lanes(11.0))
}

fn mgr(slices: &[i32], moving: bool) -> CopManager {
    CopManager::new(CopFile { records: slices.iter().map(|&s| CopRecord::Point { kind: 1, slice: s, moving }).collect() })
}

/// A cop's `.sim` for the tests: the 993 Carrera 3.6 values of
/// `GameData/Simulation/CarData/993coupe36.sim` (the same fixture `nfs_physics`'s tests use).
pub(crate) fn specs() -> CarSpecs {
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

/// A cop car of half height `hy` (the box 0x445090 halves) at chase level `level`.
fn cop(hy: f32, level: usize) -> CopCar {
    CopCar::new(0, Vec3::new(0.9, hy, 2.1), &specs(), level)
}

/// A flat drivable square, surface 1, `s` metres to a side.
fn flat_grid(s: f32) -> nfs_physics::collision::TriangleGrid {
    use nfs_physics::collision::{Triangle, TriangleGrid};
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0)
}

/// Test setup: put a just-deployed cop straight into a lit state-4 chase, skipping the two
/// ticks (placement, acquire) the original takes to get there. The tracker gets the hardest
/// skill's gain 1.0, so its first step reads the target exactly wherever the test puts it; the
/// formation slot is the rank-0 slot against the tests' 0.9 × 2.2 m target.
fn chase_now(p: &mut Police, i: usize) {
    p.cars[i].placed = false;
    let c = &mut p.cars[i];
    c.chasing = true;
    c.parked = false;
    c.lights = true;
    c.boost_budget = BOOST_BUDGET[c.level.min(2)];
    c.tracker = ChaseTracker { k: 1.0, ..Default::default() };
    c.formation = FORMATION[c.level.min(2)][c.rank.min(5)];
    c.want_lat = sector(c.formation[0], -0.9, 0.9);
    c.want_lon = sector(c.formation[2], -2.2, 2.2);
}

#[test]
fn the_speed_limit_is_flat_and_wrong_way_is_the_side_of_the_centreline() {
    // 22 m/s = 79.2 km/h, everywhere. Lane 7 is the first on the positive side, which a car
    // going forward is entitled to.
    assert_eq!(check_violation(Violation::None, 21.0, 1.0, false, 7, false, 0.0), Violation::None);
    assert_eq!(check_violation(Violation::None, 23.0, 1.0, false, 7, false, 0.0), Violation::Speeding);
    // the two-way arm, 0x403ADE: forward on the oncoming side, or backwards on its own side
    assert_eq!(check_violation(Violation::None, 10.0, 1.0, false, 6, false, 0.0), Violation::WrongWay);
    assert_eq!(check_violation(Violation::None, -10.0, 1.0, false, 7, false, 0.0), Violation::WrongWay);
    assert_eq!(check_violation(Violation::None, -10.0, 1.0, false, 6, false, 0.0), Violation::None, "backwards on the oncoming side is its own side");
    // and it needs **more** than 4 m/s, on either arm
    assert_eq!(check_violation(Violation::None, 4.0, 1.0, false, 6, false, 0.0), Violation::None);
    assert_eq!(check_violation(Violation::None, -4.0, 1.0, true, 7, false, 0.0), Violation::None);
    // the one-way arm ignores the lane and asks only for progress against the race
    assert_eq!(check_violation(Violation::None, -10.0, 1.0, true, 6, false, 0.0), Violation::WrongWay);
    assert_eq!(check_violation(Violation::None, 10.0, 1.0, true, 6, false, 0.0), Violation::None);
    // hitting a cop: light under an impact of 20, heavy above it
    assert_eq!(check_violation(Violation::None, 5.0, 1.0, false, 7, true, 10.0), Violation::LightContact);
    assert_eq!(check_violation(Violation::None, 5.0, 1.0, false, 7, true, 30.0), Violation::HeavyContact);
    // an existing pursuit only escalates on a heavy hit
    assert_eq!(check_violation(Violation::Speeding, 30.0, 1.0, false, 7, false, 0.0), Violation::Speeding);
    assert_eq!(check_violation(Violation::Speeding, 30.0, 1.0, false, 7, true, 30.0), Violation::HeavyContact);
}

/// §27.10 `0x40DA00`: the bias and the stuck timer hang on the lane corridor at both ends.
/// `0x40E670` reads slice 1's lanesA in the track's own frame, which a reversed table keeps in
/// `lanes_b`.
#[test]
fn the_one_way_flag_is_read_in_the_tracks_own_frame() {
    let mut t = ring(40);
    t.slices[1].lanes_a = 0;
    assert!(one_way_road(&t));
    let back = t.reversed();
    assert_eq!((back.slices[1].lanes_a, back.slices[1].lanes_b), (2, 0));
    assert!(one_way_road(&back), "the original's slice 1 still has no A lanes");
    t.slices[1].lanes_a = 2;
    t.slices[1].lanes_b = 0;
    assert!(!one_way_road(&t) && !one_way_road(&t.reversed()));
}

#[test]
fn the_lane_bias_follows_the_corridor_at_both_ends() {
    let mut t = ring(400);
    // lanes 4..9 exist everywhere (two lanes a side)
    let all = (4..=9).fold(0u32, |m, l| m | crate::lanes::lane_bit(l));
    for s in t.slices.iter_mut() {
        s.lane_mask = all;
    }
    // whole at the target's slice: clear, no push
    assert_eq!(lane_bias(&t, 10, 5, 20, 8, -60.0), LaneBias::Clear);
    // broken at the target's slice only: push toward it
    t.slices[20].lane_mask &= !crate::lanes::lane_bit(6);
    assert_eq!(lane_bias(&t, 10, 5, 20, 8, -60.0), LaneBias::Toward);
    // broken at both: the stuck timer counts
    t.slices[10].lane_mask &= !crate::lanes::lane_bit(7);
    assert_eq!(lane_bias(&t, 10, 5, 20, 8, -60.0), LaneBias::Blocked);
    // past 500 m, or a lane off the road, and nothing happens at all
    assert_eq!(lane_bias(&t, 10, 5, 20, 8, 501.0), LaneBias::Skip);
    assert_eq!(lane_bias(&t, 10, 3, 20, 8, -60.0), LaneBias::Skip);
}

#[test]
fn a_spawn_point_is_taken_once_the_marker_sweeps_past_it() {
    let t = ring(400);
    let mut m = mgr(&[100, 200, 300], true);
    // the first query only arms the cursor: the marker sits 31 slices ahead, so at slice 91
    assert_eq!(m.find_spawn(&t, 60, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
    // a five-slice step sweeps 92..96 — nothing there
    assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
    // the next one sweeps 97..101 and passes the record at 100
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), Some(0));
    // the record is then on a 40 s cooldown, so sweeping it again finds nothing
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), None);
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), None, "`<= 0xA00` still holds 2560 ticks on");
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5001 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), Some(0), "and is free again the tick after");
    // a jump of six slices or more is too big a sweep to search
    m.cursors = vec![Some(90)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 9000 + SPAWN_COOLDOWN, 0.0, false, 0, &[], 1, true), None);
}

/// 0x40E3B0 starts every `lastUse` at −2560, so `gGameTick − lastUse <= 0xA00` holds at tick 0
/// and at no later tick: the points are live from tick 1, not after 40 s.
#[test]
fn the_initial_cooldown_covers_tick_zero_only() {
    let t = ring(400);
    let mut m = mgr(&[100], true);
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 0, 0.0, false, 0, &[], 1, true), None, "tick 0: 0 + 2560 <= 2560");
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 1, 0.0, false, 0, &[], 1, true), Some(0), "tick 1: 2561 > 2560");
}

/// §25.5's `if seen >= lim: continue`, with the pinned heat record's `lim[0] = 2`.
#[test]
fn only_two_cops_are_ever_assigned_to_a_wanted_perp() {
    let t = ring(400);
    // records at 100 and 105; the marker runs 31 slices ahead of the perp
    let mut m = mgr(&[100, 105], true);
    // wanted: the first comes out on the roll, the second free of it (`seen != 0`), a third never
    assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, false, 0, &[], 0, false), None, "arms the cursor at 96");
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 0, false), Some(0), "sweeps 97..101");
    assert_eq!(m.find_spawn(&t, 75, 20.0, 5000, 1.0, false, 0, &[], 1, true), Some(1), "a reinforcement skips the roll");
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 2, true), None, "lim[0] is 2");
    // clean: one, and only while no cop is inside 500 m (0x403120)
    let mut m = mgr(&[100], true);
    assert_eq!(m.find_spawn(&t, 65, 20.0, 5000, 0.0, true, 0, &[], 0, false), None, "arms the cursor");
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, true, 0, &[], 0, true), None, "a cop is already close by");
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, true, 0, &[], 0, false), Some(0));
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000 + SPAWN_COOLDOWN, 0.0, true, 0, &[], 1, false), None, "lim is 1 for a clean perp (a clean perp is never claimed in practice: the 500 m test is the cap)");
}

#[test]
fn a_human_next_to_the_point_vetoes_it() {
    let t = ring(400);
    let mut m = mgr(&[100], true);
    m.cursors = vec![Some(96)];
    // a human 20 slices (120 m) from slice 100 is inside the 172 m veto
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[120], 1, true), None);
    // 40 slices (240 m) away is outside it
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[140], 1, true), Some(0));
}

#[test]
fn the_veto_distance_wraps_raw_indices_by_the_total_slice_count() {
    // a 400-slice lap plus a 100-slice branch section (indices 400…499): `[track+0]` is 500
    let mut t = ring(400);
    let extra: Vec<Slice> = t.slices[..100].to_vec();
    t.slices.extend(extra);
    t.route_pos.extend(0..100u32);
    assert_eq!(t.lap_len, 400);
    // 10 − 490 = −480 wraps by 500 to 20 slices: 120 m, inside the 172 m veto (wrapping by the
    // lap's 400 would give 80 slices, 480 m)
    assert_eq!(slice_gap(&t, 10, 490), 120.0);
    assert!(slice_gap(&t, 10, 490).abs() < HUMAN_VETO);
    // 250 is exactly half: not wrapped (`d > n/2`), −250 neither (`d < −(n/2)`)
    assert_eq!(slice_gap(&t, 300, 50), 1500.0);
    assert_eq!(slice_gap(&t, 50, 300), -1500.0);
    assert_eq!(slice_gap(&t, 301, 50), -1494.0);
    // an odd count halves by truncation: n = 401, n/2 = 200
    let mut u = ring(400);
    u.slices.push(u.slices[0]);
    assert_eq!(slice_gap(&u, 200, 0), 1200.0);
    assert_eq!(slice_gap(&u, 201, 0), -1200.0);
}

#[test]
fn the_placement_follows_the_moving_flag() {
    let t = ring(200);
    let rolling = mgr(&[50], true);
    let (s, lat, moving) = rolling.placement(&t, 0, 1.0).unwrap();
    assert_eq!(s, 50);
    assert!(moving);
    assert_eq!(lat, 4.0 * 1.5, "the centre of the outer lane");
    let parked = mgr(&[50], false);
    let (_, lat, moving) = parked.placement(&t, 0, 1.0).unwrap();
    assert!(!moving);
    assert_eq!(lat, 4.0 * 2.0 + 2.0, "2 m outside the road edge");
    // driving the track backwards puts it on the other side
    assert!(parked.placement(&t, 0, -1.0).unwrap().1 < 0.0);
}

#[test]
fn the_pursue_ladder_caps_by_distance_and_level() {
    // far away the cop is allowed 60 m/s, on top of the player it is told to stop
    assert_eq!(pursue_cap(0, 200.0), 60.0);
    assert_eq!(pursue_cap(0, 120.0), 50.0);
    assert_eq!(pursue_cap(0, 5.0), 0.0);
    // the hardest level is faster everywhere
    assert!(pursue_cap(2, 200.0) > pursue_cap(0, 200.0));
    assert!(pursue_cap(2, 7.0) > pursue_cap(0, 7.0));
    // and the middle level closes the last stretch more carefully than the easiest
    assert!(pursue_cap(1, 15.0) < pursue_cap(0, 15.0));
}

#[test]
fn a_chase_gives_up_past_216_metres() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    assert!(p.deploy(&t, 0, 0, 1000));
    assert!(p.cars[0].active && !p.cars[0].lights && !p.cars[0].chasing, "state 1 places a cop in state 2, unlit");
    chase_now(&mut p, 0);
    // a target right beside it keeps the chase alive
    let near = ChaseTarget { position: p.cars[0].position, slice: 50, lateral: 0.0, lane: 7, speed: 30.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 1000, &near, &[]);
    assert!(p.cars[0].active);
    // 400 m away it gives up
    let far = ChaseTarget { position: t.slices[250].centre, slice: 250, ..near };
    p.update(&t, 1001, &far, &[]);
    assert!(!p.cars[0].active);
    assert!(!p.cars[0].lights);
}

/// 0x407690 → 0x49B090 and the drop: the deployed cop's origin `car+0x330` is `car+0x3AC`
/// above the model's ground, and the model (`draw_origin`, 0x43A410) stands on the road —
/// at deploy and after the 64 Hz step.
#[test]
fn a_deployed_cop_stands_on_the_road_with_its_origin_half_the_box_up() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: false }] };
    let mut p = Police::new(file, vec![cop(0.63, 0)], 0, false);
    assert!(p.deploy(&t, 0, 0, 1000));
    let c = &p.cars[0];
    let road = t.slices[50].centre.y;
    assert!((c.position.y - (road + crate::traffic::RIDE_HEIGHT + 0.63)).abs() < 1e-4, "origin {}", c.position.y);
    assert!((c.draw_origin().y - (road + crate::traffic::RIDE_HEIGHT)).abs() < 1e-4, "model ground {}", c.draw_origin().y);
    p.integrate(&t, 1.0 / 64.0);
    let c = &p.cars[0];
    assert!((c.draw_origin().y - (t.slices[c.slice].centre.y + crate::traffic::RIDE_HEIGHT)).abs() < 1e-4);
}

/// `Susp_update` 0x499A70 through the cop's trackers: a probe stands on a patch only when it
/// has a surface nibble and lies within 4 m of the car, else on the car tracker's kept plane —
/// so a cop that runs off a drivable slope onto a steep face with no surface (a cliff) keeps the
/// slope's plane and stays upright, where the old any-triangle probe stood it on its side.
#[test]
fn a_cop_off_the_road_keeps_the_last_drivable_plane() {
    use nfs_physics::collision::{Triangle, TriangleGrid};
    let h = |x: f32| 0.1 * x;
    let (p0, p1, p2, p3) = (Vec3::new(-40.0, h(-40.0), -40.0), Vec3::new(40.0, h(40.0), -40.0), Vec3::new(40.0, h(40.0), 40.0), Vec3::new(-40.0, h(-40.0), 40.0));
    // the drivable slope for x < 40, a steep face with surface word 0 beyond it
    let (q1, q2) = (Vec3::new(44.0, h(40.0) + 20.0, -40.0), Vec3::new(44.0, h(40.0) + 20.0, 40.0));
    let tris = vec![Triangle::new(p0, p1, p2, 1), Triangle::new(p0, p2, p3, 1), Triangle::new(p1, q1, q2, 0), Triangle::new(p1, q2, p2, 0)];
    let grid = TriangleGrid::build(tris, 10.0);
    let t = ring(400);
    let mut p = Police::new(CopFile { records: vec![] }, vec![cop(0.63, 0)], 0, false);
    let n = Vec3::new(-0.1, 1.0, 0.0).normalize();
    {
        let c = &mut p.cars[0];
        c.active = true;
        // placed on the slope, level with it (as `Car_placeAtSlice` leaves a car), rolling +x
        let right = Vec3::new(1.0, 0.1, 0.0).normalize();
        c.orientation = Mat3::from_cols(right, n, Vec3::Z).transpose();
        c.position = Vec3::new(20.0, h(20.0) + c.rest_height() / n.y, 0.0);
        c.velocity = right * 20.0;
        c.sync_body_in();
        c.body.tracker.seat(&grid, c.position);
    }
    // 20 m/s for 1.25 s: 32 Hz ground steps between the 64 Hz moves
    for k in 0..80 {
        if k % 2 == 0 {
            p.ground(&t, &grid, 32 + k);
        }
        p.integrate_at(&t, 1.0 / 64.0, true);
        let c = &p.cars[0];
        assert!(c.orientation.row(1).dot(n) > 0.99, "up {:?} at x {}", c.orientation.row(1), c.position.x);
    }
    let c = &p.cars[0];
    assert!(c.position.x > 42.0, "ran onto the face: x {}", c.position.x);
    // on the extension of the slope's plane, not on the face (or the old slice-height fallback)
    let want = h(c.position.x) + c.rest_height() / n.y;
    assert!((c.position.y - want).abs() < 0.05, "y {} vs {}", c.position.y, want);
    assert_eq!(c.body.airborne_count, 0);
}

/// 0x408750 → `0x40F540(car, 1, 0)`: a cop 35 m outside the road's half-width (or past 115 m/s)
/// at the end of its drive tick is put back on its own slice's centre, still, on the patch,
/// facing the direction it asked for.
#[test]
fn the_sanity_net_puts_a_cop_that_left_the_road_back_on_its_slice() {
    let t = ring(400);
    let grid = flat_grid(5000.0);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    assert!(p.deploy_as(&t, 0, 0, 0, false, Some(&grid)));
    chase_now(&mut p, 0);
    let sl = t.slices[10];
    // 50 m off the road's edge (half-width 11): outside the 35 m net
    p.cars[0].lateral = 11.0 + 36.0;
    p.cars[0].position = sl.centre + sl.lateral.normalize() * 47.0 + Vec3::Y * p.cars[0].rest_height();
    p.cars[0].target_speed = 20.0;
    p.cars[0].body.knocked_over = true;
    p.cars[0].body.airborne_count = 3;
    // the knocked-over exit returns before the net (0x4084C3 precedes 0x4086E6)
    p.drive(&t, 100, Some(CopWorld { grid: &grid, occupied: &[] }));
    assert!(p.cars[0].lateral > 40.0);
    p.cars[0].body.knocked_over = false;
    p.drive(&t, 102, Some(CopWorld { grid: &grid, occupied: &[] }));
    let c = &p.cars[0];
    assert_eq!(c.slice, 10);
    assert!(c.lateral.abs() < 1e-3, "lateral {}", c.lateral);
    assert!((c.position - sl.centre).with_y(0.0).length() < 1e-3, "pos {:?}", c.position);
    assert!((c.position.y - c.rest_height()).abs() < 1e-3, "on the patch: y {}", c.position.y);
    assert_eq!(c.velocity, Vec3::ZERO);
    assert_eq!(c.angular_velocity, Vec3::ZERO);
    assert!(c.orientation.row(2).dot(sl.forward.normalize()) > 0.999);
    // 0x40F400: `+0x418`, `+0x42C`, `+0x430` cleared
    assert_eq!(c.body.airborne_count, 0);
    assert!(!c.body.knocked_over && c.body.impact == 0.0);
    // a taken slice is skipped (0x40F480): the next free one along the race
    p.cars[0].lateral = 11.0 + 36.0;
    p.cars[0].position = sl.centre + sl.lateral.normalize() * 47.0 + Vec3::Y * p.cars[0].rest_height();
    p.cars[0].target_speed = 20.0;
    p.drive(&t, 104, Some(CopWorld { grid: &grid, occupied: &[10, 11] }));
    assert_eq!(p.cars[0].slice, 12);
}

/// 0x4084C3: a cop `Damage` knocked over (`car+0x42C == 1`) only damps by 0.98 until a reset
/// clears the flag; and `Damage` itself sets the flag on a car that is not a smackable.
#[test]
fn a_knocked_over_cop_only_damps_until_it_is_reset() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let grid = flat_grid(5000.0);
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    assert!(p.deploy_as(&t, 0, 0, 0, false, Some(&grid)));
    // an impact of 100 through `Damage_byNormal` with a wall's flag: `+0x42C = 1`, `+0x418 = 1`,
    // the countdown 0xC0
    p.cars[0].body.damage_by_normal(100.0, Vec3::Z, 1, 1370.0);
    assert!(p.cars[0].body.knocked_over);
    assert_eq!(p.cars[0].body.airborne_count, 1);
    assert_eq!(p.cars[0].body.reset_countdown, 0xC0);
    p.cars[0].velocity = Vec3::new(0.0, 0.0, 10.0);
    p.cars[0].angular_velocity = Vec3::new(0.0, 0.5, 0.0);
    p.cars[0].accel_local = Vec3::new(1.0, 0.0, 2.0);
    p.drive(&t, 100, None);
    let c = &p.cars[0];
    assert!((c.velocity - Vec3::new(0.0, 0.0, 9.8)).length() < 1e-5, "{:?}", c.velocity);
    assert!((c.angular_velocity.y - 0.49).abs() < 1e-5);
    assert_eq!(c.accel_local, Vec3::ZERO);
    // the countdown (0x40F740) stops on an upright, alive car — and the cop stays knocked
    // over: nothing but a reset clears `car+0x42C`
    p.cars[0].body.cos_tilt = 1.0;
    p.post_impact(&t, &grid, &[]);
    assert_eq!(p.cars[0].body.reset_countdown, 0);
    assert!(p.cars[0].body.knocked_over);
    // a dead car (`car+0xDB0 == 0`, an impact past 120) runs it down and is reset
    p.cars[0].body.damage_by_normal(130.0, Vec3::Z, 1, 1370.0);
    assert_eq!(p.cars[0].body.engine_alive, 0);
    for _ in 0..0xC0 {
        p.post_impact(&t, &grid, &[]);
    }
    let c = &p.cars[0];
    assert!(!c.body.knocked_over, "reset by 0x40F540 mode 0");
    assert_eq!(c.body.engine_alive, 1);
    assert_eq!(c.velocity, Vec3::ZERO);
}

/// `Car_placeAtSlice` 0x49B090 on the deploy: the trackers are seated on the slice centre and
/// the body is dropped by `BodyHeight` against the **patch**, so on a road lying 0.8 m below the
/// slice plane the cop is on the road from its first tick, not a metre above it.
#[test]
fn a_deployed_cop_is_dropped_onto_the_patch_not_the_slice_plane() {
    use nfs_physics::collision::{Triangle, TriangleGrid};
    let t = ring(400);
    let s = 5000.0;
    let (a, b, c, d) = (Vec3::new(-s, -0.8, -s), Vec3::new(s, -0.8, -s), Vec3::new(s, -0.8, s), Vec3::new(-s, -0.8, s));
    let grid = TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: false }] };
    let mut p = Police::new(file, vec![cop(0.63, 0)], 0, false);
    assert!(p.deploy_as(&t, 0, 0, 1000, false, Some(&grid)));
    let c = &p.cars[0];
    assert!((c.draw_origin().y - (-0.8 + crate::traffic::RIDE_HEIGHT)).abs() < 1e-3, "model ground {}", c.draw_origin().y);
    assert!((c.position.y - (-0.8 + crate::traffic::RIDE_HEIGHT + 0.63)).abs() < 1e-3, "origin {}", c.position.y);
    // and the ground step keeps it there, upright, with the countdown clear
    for k in 0..16u64 {
        p.ground(&t, &grid, 1000 + 2 * k);
        p.integrate_at(&t, 1.0 / 64.0, true);
        p.integrate_at(&t, 1.0 / 64.0, true);
    }
    let c = &p.cars[0];
    assert!((c.draw_origin().y - (-0.8 + crate::traffic::RIDE_HEIGHT)).abs() < 0.02, "model ground {}", c.draw_origin().y);
    assert!(c.orientation.row(1).y > 0.999);
}

/// 0x40854C / 0x4085EC: a cop on its side with `car+0x418 == 0`, or with a zero target speed,
/// is only damped by 0.94 and does not steer.
#[test]
fn a_cop_on_its_side_or_without_a_target_speed_is_only_damped() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    p.deploy(&t, 0, 0, 0);
    let v0 = p.cars[0].velocity;
    p.cars[0].orientation = Mat3::from_rotation_z(1.4) * p.cars[0].orientation;
    p.cars[0].angular_velocity = Vec3::new(0.0, 0.5, 0.0);
    p.drive(&t, 100, None);
    assert!((p.cars[0].velocity - v0 * 0.94).length() < 1e-4);
    assert!((p.cars[0].angular_velocity.y - 0.49).abs() < 1e-5);
    // once hit (`car+0x418` stepped) the tipped car drives again
    p.cars[0].body.airborne_count = 1;
    let v1 = p.cars[0].velocity;
    p.drive(&t, 101, None);
    assert!((p.cars[0].velocity - v1 * 0.94).length() > 1e-3);
    // upright again, a zero target speed damps
    let mut p2 = Police::new(CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] }, vec![cop(0.6, 0)], 0, false);
    p2.deploy(&t, 0, 0, 0);
    p2.cars[0].target_speed = 0.0;
    let v2 = p2.cars[0].velocity;
    p2.drive(&t, 100, None);
    assert!((p2.cars[0].velocity - v2 * 0.94).length() < 1e-4);
}

#[test]
fn the_dispatch_picks_the_documented_behaviour() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    p.deploy(&t, 0, 0, 0);
    chase_now(&mut p, 0);
    // a long way behind and not closing: catch up, flat out
    p.cars[0].speed = 30.0;
    let far = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 30.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 100, &far, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::CatchUp), "gap {} m", track_distance(&t, 10, 40));
    // right behind a slow car: pursue, and the ladder caps the speed well under the 100 m/s base
    p.cars[0].active = true;
    p.cars[0].slice = 38;
    p.cars[0].position = t.slices[38].centre;
    p.cars[0].speed = 5.0;
    let slow = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 2.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 101, &slow, &[]);
    // within 80 m and not closing faster than 26.67 m/s: the ram (0x40D0D6…0x40D10D)
    assert_eq!(p.cars[0].mode, Some(ChaseMode::Pursue { hard: true }));
    assert_eq!(p.cars[0].aim_point, slow.position, "0x40D9A5: the aim point is the target");
    assert!(p.cars[0].target_speed.abs() < 60.0, "capped to {}", p.cars[0].target_speed);
    // closing fast, or more than 80 m away: no ram
    p.cars[0].speed = 40.0;
    p.update(&t, 102, &slow, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::Pursue { hard: false }));
    assert_eq!(p.cars[0].aim_point, Vec3::ZERO);
}

/// 0x40D09B: CLOSE-QUARTERS is `chase+0x60 · chase+0x64 == 1` — the target's direction of travel
/// times the cop's — whichever side of the target the cop is on.
#[test]
fn close_quarters_needs_both_cars_going_the_same_way_on_either_side() {
    let t = ring(400);
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 2.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    for (lat, cop_v, want) in [(-2.0, 20.0, true), (6.0, 20.0, true), (-2.0, -20.0, false)] {
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
        p.deploy(&t, 0, 0, 0);
        chase_now(&mut p, 0);
        p.cars[0].slice = 36;
        p.cars[0].position = t.slices[36].centre;
        p.cars[0].lateral = lat;
        p.cars[0].speed = cop_v;
        p.update(&t, 200, &tgt, &[]);
        assert_eq!(p.cars[0].mode == Some(ChaseMode::CloseQuarters), want, "lateral {lat}, speed {cop_v}: {:?}", p.cars[0].mode);
    }
}

/// 0x403650 / 0x4036F0: the tracker starts on the target's raw values (its lateral difference
/// mirrored), then follows by `x += (raw − x)·k` with `k = 0x5CBEDC[SKILL]`; `0x40E920` refines
/// the slice gap inside ±12 m by the forward projection.
#[test]
fn the_chase_tracker_lags_by_the_skill_gain() {
    let t = ring(400);
    let mut c = cop(0.6, 2);
    c.slice = 30;
    c.position = t.slices[30].centre;
    c.lateral = 1.0;
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 3.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    let mut tr = ChaseTracker::new(&t, TRACKER_GAIN[0], &c, &tgt);
    assert_eq!((tr.gap, tr.slice, tr.lateral, tr.speed), (60.0, 40, 3.0, 20.0));
    assert_eq!(tr.lat_diff, -2.0, "0x4036B0: cop − target, the step's sign reversed");
    let faster = ChaseTarget { speed: 40.0, ..tgt };
    tr.step(&t, &c, &faster);
    assert!((tr.speed - 21.0).abs() < 1e-4, "0.05 of the way: {}", tr.speed);
    assert!((tr.lat_diff - (-2.0 + 0.05 * 4.0)).abs() < 1e-5);
    assert_eq!(tr.slice, 40, "round(60 / 6) = 10 slices ahead of the cop");
    let mut exact = ChaseTracker { k: TRACKER_GAIN[2], ..tr };
    exact.step(&t, &c, &faster);
    assert_eq!((exact.speed, exact.lateral), (40.0, 3.0));
    // far: the slice gap; within 12 m: the projection on the cop's slice forward
    assert_eq!(fine_gap(&t, 30, c.position, 40, tgt.position), -60.0);
    let near = t.slices[31].centre;
    let g = fine_gap(&t, 30, c.position, 31, near);
    assert!(g < 0.0 && (g + 6.0).abs() < 0.1, "about a slice behind: {g}");
}

/// 0x403BD0 / 0x403C00 / 0x403BA0: a claimant enters at the bottom and moves up past holes and
/// past cops more than 12 m behind it; a release leaves a hole the others walk up through.
#[test]
fn the_roster_ranks_with_a_12_metre_hysteresis() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, (0..3).map(|_| cop(0.6, 2)).collect(), 0, false);
    for (i, s) in [(0, 50), (1, 51), (2, 60)] {
        p.cars[i].slice = s;
        p.cars[i].target = Some(0);
    }
    assert_eq!(p.roster_claim(&t, 0), 0, "an empty roster: straight to the top");
    assert_eq!(p.roster_claim(&t, 1), 1, "6 m ahead of cop 0 is not enough to pass it");
    assert_eq!(p.roster_claim(&t, 2), 0, "60 m ahead: past both");
    assert_eq!((p.cars[0].roster_pos, p.cars[1].roster_pos), (1, 2), "each passed cop moved down one");
    // cop 0 falls back: it never moves down by itself, and cop 1 passes it on its own tick
    p.cars[0].slice = 40;
    assert_eq!(p.roster_bubble(&t, 0), 1);
    assert_eq!(p.roster_bubble(&t, 1), 1);
    assert_eq!(p.cars[0].roster_pos, 2);
    // cop 2 lets go: a hole, which cop 1 walks up through whatever the gaps
    p.release(2);
    assert_eq!(p.roster_bubble(&t, 1), 0);
}

/// 0x40466B…0x4046CE: the ram mode needs the slot held past `0x5CBEA0[lvl]+0` ticks and the cop
/// inside both windows, not just the longitudinal one.
#[test]
fn the_ram_mode_wants_the_slot_held_and_both_windows() {
    let t = ring(400);
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    for (held, lat, want) in [(0, 1.0, false), (5, 1.0, true), (5, 19.0, false), (4, 1.0, false)] {
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
        p.deploy(&t, 0, 0, 0);
        chase_now(&mut p, 0);
        p.cars[0].slice = 40;
        p.cars[0].position = t.slices[40].centre;
        p.cars[0].speed = 20.0;
        (p.cars[0].slot_ticks, p.cars[0].chase_lat_err, p.cars[0].chase_gap) = (held, lat, 3.0);
        p.update(&t, 300, &tgt, &[]);
        assert_eq!(p.cars[0].ram, want, "held {held}, lateral {lat}");
    }
}

/// 0x403850 / 0x4046D3…0x404719: a cop alongside and just ahead of the target, on a passing
/// roll, re-aims at the offset `{0, 0, 0}` and rams for 0x20 ticks; the LCG is stepped whatever.
#[test]
fn the_nudge_rams_a_cop_alongside_on_a_roll() {
    assert_eq!((nudge_roll(4, 0), nudge_roll(4, 2031), nudge_roll(4, 2032)), (true, true, false), "thr 31: (r·1000)>>16 < 31");
    assert!(nudge_roll(8, 4031) && !nudge_roll(8, 4064), "thr 62 at 4 Hz");
    let t = ring(400);
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 1.5, finished: false };
    for (roll, want) in [(0u32, true), (0xFFFF, false)] {
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
        p.deploy(&t, 0, 0, 0);
        chase_now(&mut p, 0);
        p.set_wanted(true);
        let sl = t.slices[41];
        p.cars[0].slice = 41;
        p.cars[0].lateral = 2.5;
        p.cars[0].position = sl.centre + sl.lateral.normalize() * 2.5;
        p.cars[0].speed = 20.0;
        let mut steps = 0;
        p.tick(&t, 300, &tgt, &[], &[], &mut || {
            steps += 1;
            roll
        });
        assert_eq!(steps, 1, "one LCG step per state-4 tick");
        assert_eq!(p.cars[0].ram, want);
        if want {
            assert_eq!((p.cars[0].formation, p.cars[0].ram_until), ([0.0; 3], 300 + NUDGE_TICKS));
        }
    }
}

/// 0x40CD70 / 0x40D642: a chase holds `car+0x1078` / `+0x107C` — 1 m at bias 1 on every tick,
/// CLOSE-QUARTERS' aim at 15 — and `0x402790` lets it win only when both body edges' lanes are
/// still the vote's argmax with the bias added.
#[test]
fn a_chase_holds_its_lateral_through_0x402790() {
    let t = ring(400);
    let me = LaneSelf { slice: 40, lane: 7, lateral: 0.5, dir: 1.0, speed: 20.0, target_speed: 20.0, half_width: 0.9, half_len: 2.2, is_traffic: false, travelled: 1000.0 };
    let mut sc = LaneScratch::default();
    // at 1 m the edges (−0.15, 2.15) fall in lanes 6 and 8 — slots 0 and 2 of a car in lane 7 —
    // and each wins a level vote with its +1
    assert!(sc.override_holds(&t, &me, 1.0, 1.0));
    sc.score = [0.0, 2.0, 0.0];
    assert!(!sc.override_holds(&t, &me, 1.0, 1.0), "the bias cannot lift a slot past a better one");
    // 4 m: both edges in lane 8, slot 2
    sc.score = [0.0, 0.0, 0.0];
    assert!(sc.override_holds(&t, &me, 4.0, 1.0));
    sc.score = [0.0, 0.0, -1.0];
    assert!(!sc.override_holds(&t, &me, 4.0, 1.0), "a tie goes to the lower slot");
    // the chase writes it
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    p.deploy(&t, 0, 0, 0);
    chase_now(&mut p, 0);
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.cars[0].slice = 36;
    p.cars[0].position = t.slices[36].centre;
    p.cars[0].speed = 20.0;
    p.update(&t, 200, &tgt, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::CloseQuarters));
    assert_eq!(p.cars[0].lane_bias, 15.0, "CLOSE-QUARTERS aims at bias 15");
    p.cars[0].slice = 10;
    p.cars[0].position = t.slices[10].centre;
    p.update(&t, 204, &tgt, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::CatchUp));
    assert_eq!((p.cars[0].lane_override, p.cars[0].lane_bias), (1.0, 1.0), "0x40CD70's 1 m at bias 1");
}

/// 0x40AF9A…0x40AFEC: the lit target speed stores `car+0x108C = car+0x109C` and the gravity
/// scale `car+0x84 = max(car+0x109C, 1)`; a cop starts at `RigidBody_init`'s 1.0.
#[test]
fn the_lit_target_speed_sets_the_rate_multiplier_and_the_gravity_scale() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    assert_eq!((p.cars[0].rate_mul, p.cars[0].body.gravity_scale), (1.0, 1.0));
    p.deploy(&t, 0, 0, 0);
    chase_now(&mut p, 0);
    // far behind: CATCH-UP with the boost on (no human within 20 m)
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.cars[0].speed = 20.0;
    p.update(&t, 200, &tgt, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::CatchUp));
    assert_eq!((p.cars[0].rate_mul, p.cars[0].body.gravity_scale), (BOOST_SPEED, BOOST_SPEED));
}

/// 0x40AFF6…0x40B198 for a cop: the oncoming ladder on the reference car's speed, floored at
/// 8.8889 with none of traffic's `car+0xEEC` terms.
#[test]
fn the_unlit_cop_ladder_floors_at_8_889() {
    assert_eq!(unlit_target_speed(1.0, 1.0, 60.0), 16.5, "going the race's way");
    assert_eq!(unlit_target_speed(-1.0, 1.0, 30.0), -8.888_889, "traffic's 8.9 floor is not a cop's");
    assert_eq!(unlit_target_speed(-1.0, 1.0, 60.0), -8.888_889);
    assert_eq!(unlit_target_speed(-1.0, 1.0, 20.0), -13.333_333);
}

/// 0x40D194…0x40D19F: in the ram mode CLOSE-QUARTERS re-runs the rubber band without the human
/// test, so a ramming cop keeps its boost right behind the player.
#[test]
fn a_ramming_cop_keeps_its_boost_next_to_the_human() {
    let t = ring(400);
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    for (ram, want) in [(false, 1.0), (true, BOOST_SPEED)] {
        let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
        let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
        p.deploy(&t, 0, 0, 0);
        chase_now(&mut p, 0);
        p.cars[0].slice = 38;
        p.cars[0].position = t.slices[38].centre;
        p.cars[0].speed = 20.0;
        (p.cars[0].ram, p.cars[0].ram_until) = (ram, if ram { 10_000 } else { 0 });
        p.update(&t, 300, &tgt, &[]);
        assert_eq!(p.cars[0].mode, Some(ChaseMode::CloseQuarters));
        assert_eq!(p.cars[0].rate_mul, want, "ram {ram}");
    }
}

#[test]
fn close_quarters_drives_the_formation_slot() {
    // §27.8: slot 0 is 8 m ahead of the target, so a cop behind it chases at 1.4x the target's
    // speed, and once it is inside the slot it eases off instead.
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 10, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 0)], 0, false);
    p.deploy(&t, 0, 0, 0);
    chase_now(&mut p, 0);
    let tgt = ChaseTarget { position: t.slices[40].centre, slice: 40, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };

    // behind the target, travelling the same way, close enough not to be a catch-up
    p.cars[0].slice = 36;
    p.cars[0].position = t.slices[36].centre;
    p.cars[0].speed = 20.0;
    p.cars[0].lateral = 0.0;
    p.update(&t, 200, &tgt, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::CloseQuarters));
    assert_eq!(p.cars[0].rank, 0);
    assert!((p.cars[0].target_speed - 1.4 * 20.0).abs() < 1e-3, "behind the slot it chases: {}", p.cars[0].target_speed);

    // sitting in the slot ahead of the target: the pace comes off the distance table
    p.cars[0].slice = 42;
    p.cars[0].position = t.slices[42].centre;
    p.cars[0].ram_until = 0;
    p.update(&t, 400, &tgt, &[]);
    assert_eq!(p.cars[0].mode, Some(ChaseMode::CloseQuarters));
    assert!(p.cars[0].target_speed.abs() < 1.4 * 20.0, "inside the slot it eases off: {}", p.cars[0].target_speed);
    assert!(p.cars[0].target_speed.abs() >= 5.0, "never below the 5 m/s floor: {}", p.cars[0].target_speed);
    assert_eq!(p.cars[0].requested_dir, 1.0, "close quarters never asks for a U-turn");
}

#[test]
fn the_close_quarters_tables_are_the_originals() {
    assert_eq!(close_quarters_factor(10.0), 0.95);
    assert_eq!(close_quarters_factor(40.0), 0.8);
    assert_eq!(close_quarters_factor(120.0), 0.75);
    assert_eq!(close_quarters_factor(300.0), 0.6);
    // slot 0 is ahead, slot 1 ahead and to the left, slot 2 beside, slots 3..5 behind
    assert_eq!(sector(FORMATION[0][0][2], -2.2, 2.2), 1);
    assert_eq!(sector(FORMATION[0][0][0], -0.9, 0.9), 0);
    assert_eq!(sector(FORMATION[0][1][0], -0.9, 0.9), -1);
    assert_eq!(sector(FORMATION[0][2][0], -0.9, 0.9), 1);
    assert_eq!(sector(FORMATION[0][2][2], -2.2, 2.2), 0);
    assert_eq!(sector(FORMATION[0][3][2], -2.2, 2.2), -1);
}

#[test]
fn a_cop_that_spawns_while_the_perp_is_clean_patrols() {
    // §17.4: a cop deployed with no wanted perp drives the road like traffic (states 2/3) and
    // only becomes a chaser on the first tick of its own that finds the perp wanted (0x405220).
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    assert!(p.deploy_as(&t, 0, 0, 1000, false, None));
    assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights, "patrolling, lights off");
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 1002, &tgt, &[]);
    assert_eq!(p.cars[0].mode, None, "a patrol car runs no chase behaviour");
    assert!(p.cars[0].target_speed.abs() < 30.0, "traffic pace, not the pursuit base: {}", p.cars[0].target_speed);
    p.update(&t, 1004, &tgt, &[]);
    assert!(!p.cars[0].chasing, "a clean perp is never acquired");
    // the perp offends: the cop finds out in its own next tick (state 2's 0x405220)
    p.set_wanted(true);
    assert!(!p.cars[0].chasing, "set_wanted only marks the perp");
    p.update(&t, 1006, &tgt, &[]);
    assert!(p.cars[0].chasing && p.cars[0].mode.is_some(), "state 2 built the chase and ran it");
    assert!(!p.cars[0].lights, "the building tick's update runs with the lights off");
    p.update(&t, 1008, &tgt, &[]);
    assert!(p.cars[0].lights, "state 4's head lights it on the next tick");
}

/// `car+0xEAC` on patrol: `AIState_Normal` (0x40B5A0) never writes it, and 0x408360 grants it
/// below 8.889 m/s before the drive chain runs (0x4083B6…0x4083CF).
#[test]
fn a_patrol_keeps_the_requested_direction_and_the_grant_waits_for_8_889() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    assert!(p.deploy_as(&t, 0, 0, 1000, false, None));
    assert_eq!((p.cars[0].dir, p.cars[0].requested_dir), (1.0, 1.0), "0x407890: car+0xEB0 = car+0xEAC = side");
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    // too fast to turn round: the request stands and is not granted
    p.cars[0].velocity = p.cars[0].orientation.row(2) * 20.0;
    p.cars[0].requested_dir = -1.0;
    p.update(&t, 1002, &tgt, &[]);
    assert_eq!((p.cars[0].dir, p.cars[0].requested_dir), (1.0, -1.0));
    // stopped: granted at the head of this tick's drive
    p.cars[0].velocity = Vec3::ZERO;
    p.update(&t, 1004, &tgt, &[]);
    assert_eq!((p.cars[0].dir, p.cars[0].requested_dir), (-1.0, -1.0));
}

/// §17.5b: state 1 places a cop in state 2 (rolling) or 3 (parked) and runs that state's update;
/// a wanted perp is acquired the next tick (chase update unlit) and the lights come on the tick
/// after.
#[test]
fn a_fresh_cop_takes_two_ticks_to_a_lit_chase() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.set_wanted(true);
    assert!(p.deploy_as(&t, 0, 0, 1000, true, None));
    p.update(&t, 1000, &tgt, &[]);
    assert!(!p.cars[0].chasing && !p.cars[0].lights && p.cars[0].mode.is_none(), "the placement tick is AIState_Normal");
    p.update(&t, 1002, &tgt, &[]);
    assert!(p.cars[0].chasing && p.cars[0].mode.is_some() && !p.cars[0].lights, "acquired, chased, unlit");
    p.update(&t, 1004, &tgt, &[]);
    assert!(p.cars[0].lights);
}

/// §17.5e: the AI pass only writes targets; the drive 0x408360 is a Sc32 entry that runs after
/// the whole pass, so `Police::ai` leaves every cop's velocity alone and `Police::drive` moves it.
#[test]
fn the_ai_pass_writes_targets_and_the_drive_moves_the_car() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let cars: Vec<CopCar> = (0..2).map(|_| cop(0.6, 2)).collect();
    let mut p = Police::new(file, cars, 0, false);
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    assert!(p.deploy_as(&t, 0, 0, 1000, false, None));
    p.cars[0].placed = false;
    let v0 = p.cars[0].velocity;
    p.ai(&t, 1002, &tgt, &[], &[60], &mut || 0xFFFF, None);
    assert_eq!(p.cars[0].velocity, v0, "the decision does not drive");
    assert!(p.cars[0].target_speed != 0.0);
    p.drive(&t, 1002, None);
    assert!(p.cars[0].velocity != v0, "the Sc32 drive does");
    assert!(!p.cars[1].active);
}

/// §17.5e: 0x406720 runs a cop's update only when `0x414B10` row 0 says it is due — every fourth
/// 32 Hz tick within 240 m of a racer, every eighth beyond, phased by the slot — and `0x40CD70`
/// charges the boost the previous update left on by that update's `[0x5E6654]` (4 / 8).
#[test]
fn the_ai_pass_gates_a_cop_and_its_counters_step_by_the_update() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    p.slot_base = 3;
    assert!(p.deploy(&t, 0, 0, 900));
    chase_now(&mut p, 0);
    p.cars[0].boost_budget = 100;
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    let ai = |p: &mut Police, tick: u64| p.ai(&t, tick, &tgt, &[], &[60], &mut || 0xFFFF, None);
    // slot 3: `(0x103 + tick/2) & 3 == 3` at tick 1000; the boost comes on, nothing is charged yet
    ai(&mut p, 1000);
    assert_eq!((p.cars[0].boost_engine, p.cars[0].boost_budget), (BOOST_ENGINE, 100));
    for tick in [1002, 1004, 1006] {
        let before = p.cars[0].clone();
        ai(&mut p, tick);
        assert_eq!(p.cars[0], before, "not due at {tick}");
    }
    ai(&mut p, 1008);
    assert_eq!(p.cars[0].boost_budget, 96, "charged [0x5E6654] = 4 for the update that had it on");
    // beyond 240 m of every racer: 4 Hz, `[0x5E6654]` = 8, due when `(0x103 + tick/2) & 7 == 7`
    p.cars[0].racer_distance = 300.0;
    ai(&mut p, 1012);
    assert_eq!(p.cars[0].boost_budget, 96, "(0x103 + 506) & 7 = 5: not due");
    ai(&mut p, 1016);
    assert_eq!(p.cars[0].boost_budget, 88, "(0x103 + 508) & 7 = 7: due, charged 8");
    // the tracker writes `car+0x78` for a cop in the world
    p.track_racers(&[p.cars[0].position + Vec3::new(30.0, 0.0, 40.0)]);
    assert_eq!(p.cars[0].racer_distance, 40.0 + 0.25 * 30.0);
}

/// 0x408360's own gate, row 1 (`traffic::drive_due`): within 160 m of a racer the cop drives on
/// every 32 Hz tick, one tick's worth; beyond it on every other, phased by its slot, with
/// `dt` 1/16 — in between its velocity is left alone.
#[test]
fn the_cop_drive_is_gated_by_row_1() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    p.slot_base = 4;
    assert!(p.deploy(&t, 0, 0, 900));
    p.cars[0].target_speed = 30.0;
    let mut driven = 0;
    for tick in 1000..1008 {
        let v = p.cars[0].velocity;
        p.drive(&t, tick, None);
        driven += usize::from(p.cars[0].velocity != v);
    }
    assert_eq!(driven, 8, "near a racer: every 32 Hz tick");
    p.cars[0].racer_distance = 200.0;
    let mut due = Vec::new();
    for tick in 1008..1016 {
        let v = p.cars[0].velocity;
        p.drive(&t, tick, None);
        if p.cars[0].velocity != v {
            due.push(tick);
        }
    }
    // slot 4: `(0x104 + tick/2) & 1 == 1` when tick/2 is odd
    assert_eq!(due, vec![1010, 1011, 1014, 1015]);
}

/// `AIState_Stop` mode 1 (0x40CAB0): a parked cop holds its lateral at zero speed while the perp
/// is clean, and leaves the stop as soon as it builds a chase.
#[test]
fn a_parked_cop_stays_put_until_it_chases() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: false }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    assert!(p.deploy_as(&t, 0, 0, 1000, false, None));
    assert!(p.cars[0].parked);
    let lat = p.cars[0].lateral;
    for k in 0..4 {
        p.update(&t, 1000 + 2 * k, &tgt, &[]);
        assert_eq!((p.cars[0].target_speed, p.cars[0].lateral_target), (0.0, lat));
    }
    p.set_wanted(true);
    p.update(&t, 1010, &tgt, &[]);
    assert!(p.cars[0].chasing && !p.cars[0].parked && p.cars[0].mode.is_some());
}

/// §17.4 state 4 → 9 → 2 → 4: a stuck chase is not given up but restarted through
/// `AIState_GotoSlice` to the reference car's slice (`[[car+0xED4]+8]`) and one tick of
/// `AIState_Normal`.
#[test]
fn a_stuck_chase_goes_through_goto_slice_and_back() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    p.racers = vec![RacerRef { position: t.slices[60].centre, slice: 60, progress: 1.0, ..Default::default() }];
    assert!(p.deploy(&t, 0, 0, 1000));
    chase_now(&mut p, 0);
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.cars[0].stuck_ticks = STUCK_LIMIT + 1;
    p.cars[0].boost_budget = 3;
    p.update(&t, 1002, &tgt, &[]);
    let c = &p.cars[0];
    assert!(c.active, "the cop is not dropped");
    assert_eq!(c.goto_slice, Some(60), "state 9, to the reference car's slice");
    assert_eq!((c.mode, c.stuck_ticks, c.boost_engine), (None, 0, 1.0), "the chase is gone and its dtor reset the boost");
    assert!(c.lights, "state 9 does not touch the lights");
    // the goal is 60 m off: still driving to it, with no give-up test in state 9
    p.update(&t, 1004, &tgt, &[]);
    assert_eq!(p.cars[0].goto_slice, Some(60));
    // within 10 m of it (`0x40E330(state, 10.0)`): `AIState_Normal`, state 2; the lights stay
    // on until state 2's own head
    p.cars[0].slice = 59;
    p.cars[0].position = t.slices[59].centre;
    p.update(&t, 1004, &tgt, &[]);
    assert!(p.cars[0].goto_slice.is_none() && !p.cars[0].chasing && p.cars[0].lights);
    // the perp is still wanted, so state 2 clears the lights and builds a fresh chase with a
    // fresh budget
    p.update(&t, 1006, &tgt, &[]);
    assert!(p.cars[0].chasing && p.cars[0].mode.is_some() && !p.cars[0].lights, "chasing again, unlit");
    assert!(p.cars[0].boost_budget > 3, "the new chase restarted the budget (less this tick's spend)");
}

/// §17.5c: `0x4071E0` measures to the reference car `car+0xED4` — the racer that led when the cop
/// was placed (0x40DFB0) — not to the target.
#[test]
fn the_give_up_distance_is_to_the_leading_racer_latched_at_deploy() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    // the human at slice 60, an opponent ahead of it at slice 80
    p.racers = vec![RacerRef { position: t.slices[60].centre, slice: 60, progress: 1.0, ..Default::default() }, RacerRef { position: t.slices[80].centre, slice: 80, progress: 2.0, ..Default::default() }];
    assert!(p.deploy(&t, 0, 0, 1000));
    assert_eq!(p.cars[0].reference, Some(1), "the leader, not the human");
    chase_now(&mut p, 0);
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 1002, &tgt, &[]);
    assert!(p.cars[0].active);
    // the latch does not follow the race order: the human taking the lead changes nothing
    p.racers[0].progress = 3.0;
    // the opponent pulls 400 m away while the target stays beside the cop: given up
    p.racers[1].position = t.slices[120].centre;
    p.update(&t, 1004, &tgt, &[]);
    assert!(!p.cars[0].active, "0x4071E0 against the latched leader");
}

/// §17.5d: `0x405220` walks every racer, but an opponent's violation is never checked, so the
/// cop takes the human — even with an opponent right beside it — and claims it.
#[test]
fn a_cop_never_acquires_an_opponent() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2), cop(0.6, 2)], 0, false);
    let r = |slice: usize| RacerRef { position: t.slices[slice].centre, slice, progress: slice as f32 / 400.0, laps_timed: 0, speed: 20.0 };
    p.set_racers(vec![r(70), r(51)]);
    assert!(p.deploy_as(&t, 0, 0, 1000, false, None));
    assert!(p.deploy_as(&t, 1, 0, 1000, false, None));
    let tgt = ChaseTarget { position: t.slices[70].centre, slice: 70, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 1002, &tgt, &[]);
    p.update(&t, 1004, &tgt, &[]);
    assert!(p.cars.iter().all(|c| !c.chasing && c.target.is_none()), "a clean field: nobody to acquire");
    assert_eq!((p.claims(0), p.claims(1)), (0, 0));
    p.set_wanted(true);
    p.update(&t, 1006, &tgt, &[]);
    assert!(p.cars.iter().all(|c| c.chasing && c.target == Some(0)), "the human, not the nearer opponent");
    assert_eq!((p.claims(0), p.claims(1)), (2, 0));
    p.set_wanted(false);
    p.update(&t, 1008, &tgt, &[]);
    assert_eq!(p.claims(0), 0, "the empty-handed 0x405220 releases");
}

/// §17.5d: `0x405440` walks the racers from the top of `0x5E8BAC` and an opponent is a clean
/// perp — so a patrol comes out 31 slices ahead of **it**, one per 500 m, whatever the human does.
#[test]
fn a_patrol_spawns_ahead_of_an_opponent() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 233, moving: true }, CopRecord::Point { kind: 1, slice: 43, moving: true }] };
    let cars: Vec<CopCar> = (0..3).map(|_| cop(0.6, 2)).collect();
    let mut p = Police::new(file, cars, 0, false);
    let r = |slice: usize, progress: f32| RacerRef { position: t.slices[slice].centre, slice, progress, laps_timed: 0, speed: 20.0 };
    let mut draws = 0;
    let tgt = |slice: usize| ChaseTarget { position: t.slices[slice].centre, slice, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    // the human at 10, the opponent ahead at 200: both cursors start at the racers' own slices
    // (0x405CBB), so the first query's 31-slice sweep is too long and only moves them
    p.set_racers(vec![r(10, 0.025), r(200, 0.5)]);
    p.tick(&t, 5000, &tgt(10), &[], &[10], &mut || { draws += 1; 0 });
    assert_eq!(p.active_count(), 0, "the first query only moves each cursor");
    // both move 3 slices: the opponent is on top of 0x5E8BAC and its marker sweeps 231..233
    // first (`lo … hi − 1`, 0x40558E)
    p.set_racers(vec![r(13, 0.0325), r(203, 0.5075)]);
    p.tick(&t, 5002, &tgt(13), &[], &[13], &mut || { draws += 1; 0 });
    assert!(p.cars[0].active && p.cars[0].slice == 233, "cop 0 at the opponent's record");
    assert!(p.cars[1].active && p.cars[1].slice == 43, "cop 1 at the human's: the opponent has a cop inside 500 m now, the human's cursor has not moved yet");
    assert!(!p.cars[2].active);
    assert_eq!(draws, 2, "one LCG step per usable record");
    // nobody is wanted, so nobody claims — and the opponent is never acquired
    let tgt = tgt(13);
    p.update(&t, 5004, &tgt, &[]);
    p.update(&t, 5006, &tgt, &[]);
    assert!(p.cars[0].target.is_none() && p.claims(1) == 0);
}

/// `0x406720` runs the cops in index order, so a cop that gives up (0x4071E0) before a
/// higher-index idle cop searches is already out of `0x403120`'s 500 m test for it.
#[test]
fn state_one_runs_in_index_order_among_the_other_cops() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 43, moving: true }] };
    let cars: Vec<CopCar> = (0..2).map(|_| cop(0.6, 2)).collect();
    let mut p = Police::new(file, cars, 0, false);
    let r = |slice: usize| RacerRef { position: t.slices[slice].centre, slice, progress: slice as f32 / 400.0, laps_timed: 0, speed: 20.0 };
    let tgt = |slice: usize| ChaseTarget { position: t.slices[slice].centre, slice, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    // cop 0 is out, far down the track from the (clean) human, and within 216 m of it in the
    // plane (the give-up test measures octdist to the reference car — here the target)
    p.set_racers(vec![r(10)]);
    assert!(p.deploy_as(&t, 0, 0, 0, false, None));
    p.cars[0].slice = 250;
    p.cars[0].position = t.slices[10].centre;
    p.tick(&t, 1000, &tgt(10), &[], &[10], &mut || 0);
    assert!(p.cars[0].active && !p.cars[1].active, "the first query only moves the human's cursor");
    // next tick cop 0 sits inside 500 m of the human's slice but is past 216 m of its reference
    // car, so it gives up in its own update — which runs before cop 1's state 1, so cop 1 finds
    // no cop near the human and comes out at 43 (run the other way round, `0x403120` would
    // still see cop 0 and `lim` would be 0)
    p.cars[0].slice = 13;
    p.cars[0].position = Vec3::new(5000.0, 0.0, 5000.0);
    p.set_racers(vec![r(13)]);
    p.tick(&t, 1002, &tgt(13), &[], &[13], &mut || 0);
    assert!(!p.cars[0].active, "cop 0 gave up");
    assert!(p.cars[1].active && p.cars[1].slice == 43, "cop 1 placed the same tick");
}

/// `0x40E4B0` looks only at the record under the perp's cursor: of two records at one slice the
/// walk from below stops on the first, and while that one cools down the other is never found.
#[test]
fn the_record_cursor_sees_one_record_per_slice() {
    let t = ring(400);
    let mut m = mgr(&[100, 100, 200], true);
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), Some(0));
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5002, 0.0, false, 0, &[], 1, true), None, "record 1 is behind record 0");
    // the record cursor walks up to record 2 for the one at 200
    m.cursors = vec![Some(196)];
    assert_eq!(m.find_spawn(&t, 170, 20.0, 5004, 0.0, false, 0, &[], 1, true), Some(2));
    // a sweep that starts below 100 walks past both 100s down to record 0 (still cooling)…
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5006, 0.0, false, 0, &[], 1, true), None);
    // …but one whose first slice is 100 stops on the first record at or below it: record 1
    m.rec_cursors = vec![2];
    m.cursors = vec![Some(100)];
    assert_eq!(m.find_spawn(&t, 74, 20.0, 5008, 0.0, false, 0, &[], 1, true), Some(1), "from above the walk stops on record 1");
    // and a sweep scans `lo … hi − 1` (0x40558E): the marker's own slice waits for the next query
    let mut m = mgr(&[101], true);
    m.cursors = vec![Some(96)];
    assert_eq!(m.find_spawn(&t, 70, 20.0, 5000, 0.0, false, 0, &[], 1, true), None, "96..100");
    assert_eq!(m.find_spawn(&t, 71, 20.0, 5000, 0.0, false, 0, &[], 1, true), Some(0), "101");
}

/// `0x417710`: the reference car is the top of `0x5E8B88`, sorted by `car+0xAF8 − car+0xB20`
/// with a stable bubble sort — so a tie keeps the order the previous sort left, and a racer a
/// lap down but further round its lap is on top.
#[test]
fn the_reference_order_is_lap_progress_and_ties_keep_the_last_order() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    let r = |slice: usize, progress: f32, laps: u32| RacerRef { position: t.slices[slice].centre, slice, progress, laps_timed: laps, speed: 20.0 };
    // a tie at the start: registration order, so the later racer is on top
    p.set_racers(vec![r(60, 0.5, 0), r(60, 0.5, 0)]);
    assert_eq!(p.leading_racer(), Some(1));
    // racer 0 moves ahead, then they tie again: 0 stays on top, the sort does not swap equals
    p.set_racers(vec![r(61, 0.6, 0), r(60, 0.5, 0)]);
    assert_eq!(p.leading_racer(), Some(0));
    p.set_racers(vec![r(62, 0.7, 0), r(62, 0.7, 0)]);
    assert_eq!(p.leading_racer(), Some(0), "the tie keeps the last order");
    // the key subtracts the timed laps: 1.3 − 1 = 0.3 is below 0.8 − 0 = 0.8
    p.set_racers(vec![r(80, 1.3, 1), r(70, 0.8, 0)]);
    assert_eq!(p.leading_racer(), Some(1), "progress into the lap, not the race position");
    assert!(p.deploy(&t, 0, 0, 1000));
    assert_eq!(p.cars[0].reference, Some(1));
}

/// §17.5b: a perp with no violation left is dropped by state 4's own `0x405220` (0x40471F →
/// 0x4053E9…0x405424) and the cop falls to state 2 the same tick (0x404795 → 0x4047D9), lights
/// still on, driving the Normal; state 2's head puts them out next tick and nothing re-acquires.
#[test]
fn a_perp_that_turns_clean_is_dropped_to_state_2() {
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    assert!(p.deploy(&t, 0, 0, 1000));
    chase_now(&mut p, 0);
    let tgt = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
    p.update(&t, 1002, &tgt, &[]);
    assert!(p.cars[0].chasing && p.cars[0].mode.is_some());
    p.set_wanted(false);
    p.update(&t, 1004, &tgt, &[]);
    let c = &p.cars[0];
    assert!(c.active && !c.chasing && c.mode.is_none(), "state 2, not given up");
    assert!(c.lights, "state 4's head lit it; only state 2's head clears it");
    assert_eq!((c.boost_engine, c.boost_speed, c.aim_point), (1.0, 1.0, Vec3::ZERO), "the chase's dtor 0x40CC10");
    p.update(&t, 1006, &tgt, &[]);
    assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights, "patrolling, unlit");
}

/// §17.5c: the perp's tick clears the violation once it has finished, and a chasing cop drops
/// to state 2 (0x40479F / the empty target) instead of giving up; it patrols from then on.
#[test]
fn a_finished_perp_is_clean_and_its_chaser_patrols() {
    assert_eq!(perp_update(Violation::Speeding, true, |_| unreachable!("0x4059C0 skips check")), Violation::None);
    assert_eq!(perp_update(Violation::None, false, |_| Violation::WrongWay), Violation::WrongWay);
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let mut p = Police::new(file, vec![cop(0.6, 2)], 0, false);
    assert!(p.deploy(&t, 0, 0, 1000));
    chase_now(&mut p, 0);
    // even with the perp still marked wanted, 0x40479F alone drops a finished target
    let done = ChaseTarget { position: t.slices[60].centre, slice: 60, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: true };
    p.update(&t, 1002, &done, &[]);
    assert!(p.cars[0].active && !p.cars[0].chasing && p.cars[0].mode.is_none(), "state 2, not retired");
    // the perp's own tick has cleared it: no re-acquire, the cop keeps patrolling
    p.set_wanted(!perp_update(Violation::Speeding, true, |v| v).is_clean());
    for k in 0..3 {
        p.update(&t, 1004 + 2 * k, &done, &[]);
        assert!(p.cars[0].active && !p.cars[0].chasing && !p.cars[0].lights);
    }
}

#[test]
fn the_duty_budget_caps_how_many_cops_are_out() {
    // §17.2: three at once with a single racer, four with opponents, two in a two-player game
    assert_eq!(duty_budget(1, false), 3);
    assert_eq!(duty_budget(4, false), 4);
    assert_eq!(duty_budget(2, true), 2);
    let t = ring(400);
    let file = CopFile { records: vec![CopRecord::Point { kind: 1, slice: 50, moving: true }] };
    let cars: Vec<CopCar> = (0..4).map(|_| cop(0.6, 0)).collect();
    let mut p = Police::new(file, cars, 0, false);
    assert_eq!(p.budget, 3, "a single racer");
    for i in 0..4 {
        p.deploy(&t, i, 0, 100);
    }
    // …but the budget caps nothing: pass 1 puts every cop on duty because `car+0xECC & 4` is
    // never set in this build, so all four cars may be out.
    assert_eq!(p.active_count(), 4);
    assert_eq!(COP_LEVEL[0], 2, "a role-0 cop is always level 2");
}

#[test]
fn the_probability_matches_the_shipped_files() {
    // autobahn ships 21 records -> 16/21
    let a = mgr(&(0..21).map(|i| i * 100).collect::<Vec<_>>(), true);
    assert!((a.spawn_probability(false, 0) - 16.0 / 21.0).abs() < 1e-6);
    assert!(a.spawn_probability(true, 0) > 1.0, "a clean perp doubles it");
    assert_eq!(a.spawn_probability(false, 2), 1.0, "the hardest skill is certain");
    // 0x4054EB compares with 2 for equality: any other value keeps 16 / nRecords
    assert!((a.spawn_probability(false, 3) - 16.0 / 21.0).abs() < 1e-6);
    // every other shipped track has 16 or fewer, so the roll always succeeds
    let c = mgr(&[100, 300, 500, 800, 1100], true);
    assert!(c.spawn_probability(false, 0) >= 1.0);
}

/// Refactor guard (2026-10-10, T1 a): traffic and cops share one ring for 600 ticks, each fed the
/// other's lane cars, so every lane pass of both runs with the others' list, the places and the
/// fine positions. Pins the draws of both generators (the set's own `Lcg` and the cops' closure) and
/// the end poses — a rewrite of the per-car lane-pass context (cars / places / positions, their
/// order, the skipped self) that changes one draw or one tie shows up here without game data.
fn shared_ring_digest(with_places: bool) -> (u32, u32, u64) {
    use crate::traffic::{CopView, Reference, TrafficSet};
    let t = ring(200);
    let mut set = TrafficSet::new(&crate::test_support::traffic_models(7), &t, 1.0, false, 12345);
    set.slot_base = 3;
    let file = CopFile { records: [10, 30, 150].iter().map(|&s| CopRecord::Point { kind: 1, slice: s, moving: true }).collect() };
    let mut p = Police::new(file, (0..3).map(|_| cop(0.6, 1)).collect(), 0, false);
    for i in 0..3 {
        p.deploy(&t, i, i, 900);
    }
    chase_now(&mut p, 0);
    let mut cop_rng = crate::traffic::Lcg::new(77);
    for tick in 961u64..2161 {
        let s = (tick as usize / 40) % 200;
        let r = Reference { position: t.slices[s].centre, slice: s, laps: 0, speed: 20.0, dir: 1.0, honking: false };
        let tgt = ChaseTarget { position: r.position, slice: s, lateral: 0.0, lane: 7, speed: 20.0, half_width: 0.9, half_length: 2.2, finished: false };
        if tick % 2 == 0 {
            // the lists each side is handed: the other set's cars, registration order
            let cop_lane: Vec<LaneCar> = p.cars.iter().filter(|c| c.active).map(|c| c.as_lane_car(&t)).collect();
            let traffic_lane: Vec<LaneCar> = set.cars.iter().filter(|c| c.active).map(|c| c.as_lane_car(&t)).collect();
            if with_places {
                set.other_positions = p.cars.iter().filter(|c| c.active).map(|c| c.position).collect();
                set.other_places = (0..cop_lane.len()).map(|k| (10 - k) * 3 + (tick as usize / 2) % 3).collect();
                p.other_positions = set.cars.iter().filter(|c| c.active).map(|c| c.position).collect();
                p.other_places = (0..traffic_lane.len()).map(|k| (10 - k) * 2 + (tick as usize / 2) % 2).collect();
            }
            set.cops = p.cars.iter().map(|c| CopView { slice: c.slice, speed: c.speed, in_world: c.active, has_target: c.chasing }).collect();
            p.ai(&t, tick, &tgt, &traffic_lane, &[], &mut || cop_rng.rand16(), None);
            set.ai(&t, tick, r, &cop_lane);
        }
        set.drive(&t, tick, r, None, &[]);
        p.drive(&t, tick, None);
        set.integrate(&t, 1.0 / 64.0);
        p.integrate(&t, 1.0 / 64.0);
    }
    // the pose checksum: the f32 bits of every car's position and lateral, FNV-1a
    let mut h = 0xCBF2_9CE4_8422_2325u64;
    let mut eat = |x: u32| {
        h ^= x as u64;
        h = h.wrapping_mul(0x100_0000_01B3);
    };
    for c in &set.cars {
        eat(c.active as u32);
        eat(c.slice as u32);
        for v in [c.position.x, c.position.y, c.position.z, c.lateral, c.speed, c.target_speed, c.lateral_target] {
            eat(v.to_bits());
        }
    }
    for c in &p.cars {
        eat(c.active as u32);
        eat(c.slice as u32);
        for v in [c.position.x, c.position.y, c.position.z, c.lateral, c.speed, c.target_speed, c.lateral_target] {
            eat(v.to_bits());
        }
    }
    (set.rng.0, cop_rng.0, h)
}

#[test]
fn traffic_and_cops_on_a_shared_ring_keep_their_draws_and_poses() {
    let a = shared_ring_digest(false);
    let b = shared_ring_digest(true);
    assert_eq!(shared_ring_digest(false), a, "deterministic");
    assert_eq!(a, (24509, 10773, 5548681200507787554)); // the same on every platform since the test ring uses portable maths
    assert_eq!(b, a, "the places and positions of the others change no draw here");
}

/// Timing of the shared-ring scenario (`cargo test -p nfs-game --lib -- --ignored --nocapture ring_timing`):
/// the per-AI-pass allocations of the traffic and the cops show up here. Not an assertion.
#[test]
#[ignore]
fn ring_timing() {
    let t0 = std::time::Instant::now();
    for _ in 0..20 {
        shared_ring_digest(true);
    }
    eprintln!("ring_timing: {:?} for 20 x 1200 ticks", t0.elapsed());
}

/// Refactor guard (P4): `leading_racer` is the last of a stable ascending sort on the order key, so
/// the later of equal keys wins. Compared with that definition (the old implementation, kept here as
/// the reference) over every assignment of four key values — equal keys and a NaN among them — to up to
/// six racers, and every starting order of the array.
#[test]
fn the_leading_racer_is_the_last_of_the_stable_sort() {
    fn reference(order: &[usize], racers: &[RacerRef]) -> Option<usize> {
        let mut order = order.to_vec();
        stable_order(&mut order, racers, RacerRef::order_key);
        order.last().copied()
    }
    let keys = [0.0f32, 1.0, 2.5, f32::NAN];
    let mut checked = 0;
    for n in 0..=5usize {
        for code in 0..keys.len().pow(n as u32) {
            let racers: Vec<RacerRef> = (0..n).map(|k| RacerRef { progress: keys[(code / keys.len().pow(k as u32)) % keys.len()], ..Default::default() }).collect();
            // the identity order, its reverse and a rotation
            let orders: Vec<Vec<usize>> = vec![(0..n).collect(), (0..n).rev().collect(), (0..n).map(|k| (k + 1) % n.max(1)).collect(), Vec::new()];
            for order in orders {
                let mut p = Police::new(CopFile { records: vec![] }, vec![], 0, false);
                p.racers = racers.clone();
                p.race_order = order.clone();
                let want = reference(&order, &racers);
                assert_eq!(p.leading_racer(), want, "keys {:?} order {order:?}", racers.iter().map(|r| r.progress).collect::<Vec<_>>());
                checked += 1;
            }
        }
    }
    assert!(checked > 5000);
}
