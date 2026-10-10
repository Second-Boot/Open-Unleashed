use super::*;

#[test]
fn roughness_by_surface_and_tilt() {
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.cos_tilt = 1.0;
    v.speed_est = 30.0;
    // smooth surfaces (asphalt = 1) have no roughness at all
    v.wheels[0].surface = 1;
    for _ in 0..200 {
        assert_eq!(v.roughness(0), 0.0);
    }
    // a rough surface (3) draws offsets within ±amplitude (0.1 m) and varies over distance
    v.wheels[1].surface = 3;
    let mut seen: Vec<f32> = Vec::new();
    for _ in 0..200 {
        let r = v.roughness(1);
        assert!(r.abs() <= 0.1 + 1e-6, "amplitude {r}");
        seen.push(r);
    }
    assert!(seen.iter().any(|r| *r != 0.0), "some offset is produced");
    // a tilted car gets none (cosTilt < 0.86)
    v.cos_tilt = 0.5;
    assert_eq!(v.roughness(1), 0.0);
}

#[test]
fn drafting_only_behind_a_close_car_at_speed() {
    // §4.3 / 0x49D9B0: nothing below 15 m/s, nothing outside v/3 metres or 7.7 degrees, and at
    // most half the drag
    let fwd = Vec3::Z;
    let me = Vec3::ZERO;
    assert_eq!(draft_factor(me, fwd, 10.0, &[Vec3::new(0.0, 0.0, 5.0)]), 1.0, "too slow");
    assert_eq!(draft_factor(me, fwd, 30.0, &[Vec3::new(0.0, 0.0, 15.0)]), 1.0, "the car is 15 m away, the window is v/3 = 10");
    assert_eq!(draft_factor(me, fwd, 30.0, &[Vec3::new(5.0, 0.0, 5.0)]), 1.0, "45 degrees off the heading");
    let close = draft_factor(me, fwd, 30.0, &[Vec3::new(0.0, 0.0, 2.0)]);
    assert!((0.5..1.0).contains(&close), "drag cut to {close}");
    // directly behind is no help at all
    assert_eq!(draft_factor(me, fwd, 30.0, &[Vec3::new(0.0, 0.0, -2.0)]), 1.0);
}

#[test]
fn the_turbo_spools_and_bleeds() {
    // 0x49E6C4: a turbo rises 0.05 a tick toward max((rpm - redline/4)/(redline/4) (at most 1)
    // x throttle, rpm/redline/2 x throttle), and falls the same way
    let mut specs = tests::specs_993();
    specs.induction = 1;
    specs.redline = 6000.0;
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.throttle = 1.0;
    v.rpm = 5000.0;
    for _ in 0..40 {
        v.boost_update(&specs);
    }
    let held = v.boost;
    assert!(held > 0.4 && held <= 1.0, "spooled to {held}");
    // off the throttle it falls back
    v.throttle = 0.0;
    for _ in 0..40 {
        v.boost_update(&specs);
    }
    assert!(v.boost < 0.01, "bled to {}", v.boost);
    // a supercharger rises at once and falls by 0.1
    specs.induction = 2;
    v.throttle = 1.0;
    v.rpm = 5000.0;
    v.boost_update(&specs);
    assert!(v.boost > 0.4, "instant: {}", v.boost);
    // and a naturally aspirated engine never boosts
    specs.induction = 0;
    v.boost = 0.0;
    v.boost_update(&specs);
    assert_eq!(v.boost, 0.0);
}

#[test]
fn a_damaged_engine_misfires() {
    // §4.3: `misfire = engineDamage/300 > rand01`, and a misfire over the rev target kills the
    // drive
    let specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    // an undamaged engine never misfires, whatever the draw
    v.misfire_roll = 0.0;
    assert_eq!(v.engine_wear(&specs), 0.0);
    // a wrecked one misfires whatever the draw: (300 + 1.5·300) / 2 over 300
    v.damage = [300.0; 10];
    let dmg = v.engine_wear(&specs) / 300.0;
    assert_eq!(dmg, 1.25, "engine damage {dmg}");
}

#[test]
fn the_engine_figure_counts_zone_one_and_divides_by_one_more_part() {
    // 0x4938A0: `(zone1 + Σ part terms) / (1 + n)`; 0x493BC0: `Σ / n` only when n > 1
    let mut specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    v.damage = [0.0; 10];
    v.damage[1] = 40.0;
    let (sum, n) = v.part_sum(&specs, &[1]);
    assert_eq!(n, 1);
    assert_eq!(v.engine_wear(&specs), (40.0 + sum) / 2.0);
    v.damage = [20.0; 10];
    let one = v.suspension_wear(&specs);
    assert_eq!(one, v.part_sum(&specs, &[4, 5, 8]).0);
    let spring = specs.parts.iter().find(|p| p.kind == 4).copied().unwrap();
    specs.parts.push(crate::specs::InstalledPart { kind: 5, ..spring });
    specs.parts.push(crate::specs::InstalledPart { kind: 8, ..spring });
    assert_eq!(v.suspension_wear(&specs) * 3.0, v.part_sum(&specs, &[4, 5, 8]).0);
}

#[test]
fn each_part_averages_its_own_zone_mask() {
    // 0x4938A0 family: every installed part of the type adds `(1.5 − 0.005·cond) · mean over
    // its own +0x48 mask + state` — two brake parts on different corners see different zones
    let mut specs = tests::specs_993();
    let mut v = Vehicle::new(Vec3::ZERO, Mat3::IDENTITY, &specs, 0.8, 0.3);
    let mut front = [false; 10];
    front[0] = true;
    front[2] = true;
    let mut rear = [false; 10];
    rear[4] = true;
    rear[6] = true;
    specs.parts.retain(|p| p.kind != 2);
    specs.parts.push(crate::specs::InstalledPart::new(2, front, 100.0, 3.0));
    specs.parts.push(crate::specs::InstalledPart::new(2, rear, 80.0, 0.0));
    v.damage = [0.0; 10];
    v.damage[0] = 40.0;
    v.damage[2] = 20.0;
    v.damage[6] = 10.0;
    // front: 1.0 · 30 + 3 ; rear: 1.1 · 5 + 0
    assert!((v.brakes_wear(&specs) - (30.0 + 3.0 + 1.1 * 5.0)).abs() < 1e-4, "{}", v.brakes_wear(&specs));
    // a part whose mask is empty adds only its state (the `edi == 0` path: sum 0, no divide)
    specs.parts.push(crate::specs::InstalledPart::new(2, [false; 10], 100.0, 7.0));
    assert!((v.brakes_wear(&specs) - (38.5 + 7.0)).abs() < 1e-4);
}
