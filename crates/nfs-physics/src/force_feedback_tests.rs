//! Expected values are worked by hand from the instructions cited at each check
//! (`RE/notes/force-feedback.md`).
use super::*;

const LV: FfLevels = FfLevels { total: 50, road: 50, engine: 50, collision: 50, grip: 50 };

fn car() -> FfCar {
    FfCar {
        speed: 20.0,
        body_height: 0.0,
        air_count: 0,
        surface: 1,
        tyre_damage: 0.0,
        engine_damage: 0.0,
        wrecked: false,
        rpm: 4000.0,
        pitch_rate: 0.0,
        car_11c0: 0.0,
        words: [3600.0, -720.0, 800.0],
    }
}

fn is_spring(c: &FfCommand) -> bool {
    matches!(c, FfCommand::Spring { .. })
}

#[test]
fn road_ladder_rows() {
    // 0x41291E…0x412A91: each row is `mph < threshold`
    assert_eq!(road_ladder(4.9), (6, 2500));
    assert_eq!(road_ladder(5.0), (6, 3000));
    assert_eq!(road_ladder(12.0), (7, 3500));
    assert_eq!(road_ladder(35.0), (10, 5000));
    assert_eq!(road_ladder(59.9), (11, 4500));
    assert_eq!(road_ladder(150.0), (15, 3500));
    assert_eq!(road_ladder(199.9), (16, 3500));
    assert_eq!(road_ladder(200.0), (17, 3000));
}

#[test]
fn engine_formula() {
    // 0x4127A0: rpm 7000 → Hz = ftol(7000/375 + 10) = 28; q = 2000/180 = 11;
    // base = min(1700 + (10 << 11), 5000) + 3000 = 8000; period = ftol(1e6/28 + 0.5) = 35714
    let mut ff = ForceFeedback::new(true, false);
    let mut out = Vec::new();
    let mut c = car();
    c.rpm = 7000.0;
    ff.engine(&c, 50, &mut out);
    assert_eq!(out, vec![FfCommand::Periodic { effect: FfEffect::Engine, magnitude: 8000, hz: 28, period_us: 35714, gain: 50 }]);
    // rpm 4000: Hz = ftol(10.667 + 10) = 20, m = 0 → 10 + 1700 + 3000 = 4710, damage 0.5 → +50
    out.clear();
    c.rpm = 4000.0;
    c.engine_damage = 0.5;
    ff.engine(&c, 50, &mut out);
    assert_eq!(out, vec![FfCommand::Periodic { effect: FfEffect::Engine, magnitude: 4760, hz: 20, period_us: 50000, gain: 50 }]);
    // rpm 5400: q = 400/180 = 2 → 40 + 1700 + 3000 = 4740, Hz = ftol(14.4 + 10) = 24
    out.clear();
    c.rpm = 5400.0;
    c.engine_damage = 0.0;
    ff.engine(&c, 50, &mut out);
    assert!(matches!(out[0], FfCommand::Periodic { magnitude: 4740, hz: 24, .. }));
}

#[test]
fn magic_divide_is_division_by_180() {
    for m in 0..=30_000 {
        assert_eq!(engine_shift(m), m / 180, "m = {m}");
    }
}

#[test]
fn rearm_window_is_one_ninth_over_the_duration() {
    // 0x412D19: (dur << 7) · 0x4A90BE59 >> 50 = dur · 128 / 900000
    assert_eq!(rearm_ticks(500_000), 71);
    assert_eq!(rearm_ticks(100_000), 14);
    assert_eq!(rearm_ticks(500), 0);
}

#[test]
fn words_formula() {
    // 0x49F505…0x49F5CD, slip_sum 0.1, specs[0xF4] 1: t = 0.4
    let w = ff_words(true, 0.1, 1.0, 0.0, 2.0, 10, 30.0);
    assert!((w[2] - 800.0).abs() < 0.01, "{w:?}"); // |0.4·2000|
    assert!((w[1] - 720.0).abs() < 0.01, "{w:?}"); // (10 + 2)·0.6·100
    assert!((w[0] - 3600.0).abs() < 0.05, "{w:?}"); // 0.6 · 30 · 200 (speed ≤ 50)
    // saturated at 0.8, above 50 m/s the grip word is (1 − t)·10000
    let w = ff_words(true, 1.0, 1.0, 0.0, 0.0, 0, 60.0);
    assert!((w[2] - 1600.0).abs() < 0.01 && (w[0] - 2000.0).abs() < 0.01 && w[1] == 0.0, "{w:?}");
    // the noise enters the slip word ×200 and the sum is made absolute (0x49F574)
    let w = ff_words(true, 0.0, 1.0, -3.0, 0.0, 0, 10.0);
    assert!((w[2] - 600.0).abs() < 0.01, "{w:?}");
    // no front contact: all three 0 (0x49F5CF)
    assert_eq!(ff_words(false, 0.1, 1.0, 1.0, 1.0, 1, 30.0), [0.0; 3]);
}

#[test]
fn steering_force_formula() {
    // 0x412730: −x · 32500 clamped to ±10000, ftol (truncation), absolute alongside
    let mut c = car();
    c.car_11c0 = 0.1;
    assert_eq!(steering_force(&c), (-3250, 3250));
    c.car_11c0 = -0.5;
    assert_eq!(steering_force(&c), (10000, 10000));
    c.car_11c0 = 0.5;
    assert_eq!(steering_force(&c), (-10000, 10000));
    // the branch 0x412744 (never reached by the driver): the pitch rate · 0.125
    c.air_count = 1;
    c.pitch_rate = 0.8;
    assert_eq!(steering_force(&c), (-3250, 3250));
}

#[test]
fn collision_jolt_values() {
    let mut ff = ForceFeedback::new(true, false);
    let c = car();
    // atan2(1, 1) = 0.125 turns → 22.5 → bare fistp ties to even = 22; magnitude 3 → duration
    // 30000 clamped to 100000, magnitude 3000 clamped to 5000 (0x41234C, 0x412368)
    let out = ff.collision_jolt(&c, 0x40000, Vec3::new(1.0, 0.0, -1.0), 3.0, LV, false, 1000);
    assert_eq!(
        out,
        vec![FfCommand::Jolt { magnitude: 5000, duration_us: 100_000, direction: 22, attack_level: 5000, attack_time: 50_000, fade_level: 5000, fade_time: 95_000, gain: 50 }]
    );
    // inside the re-arm window (14 ticks): nothing; after it: fires
    assert!(ff.collision_jolt(&c, 0x40000, Vec3::new(1.0, 0.0, -1.0), 3.0, LV, false, 1013).is_empty());
    let out = ff.collision_jolt(&c, 0x40000, Vec3::new(-1.0, 0.0, -1.0), 3.0, LV, false, 1014);
    // atan2(−1, 1) = −0.125 → + 2 = 1.875 → 337.5 → 338 (ties to even)
    assert!(matches!(out[0], FfCommand::Jolt { direction: 338, .. }));
    // a big hit: duration 10000·mag, magnitude clamps at 10000; and the duration clamps at 500000
    let out = ForceFeedback::new(true, false).collision_jolt(&c, 0x40000, Vec3::new(1.0, 0.0, -1.0), 20.0, LV, false, 0);
    assert!(matches!(out[0], FfCommand::Jolt { magnitude: 10000, duration_us: 200_000, .. }));
    let out = ForceFeedback::new(true, false).collision_jolt(&c, 0x40000, Vec3::new(1.0, 0.0, -1.0), 80.0, LV, false, 0);
    assert!(matches!(out[0], FfCommand::Jolt { magnitude: 10000, duration_us: 500_000, .. }));
    // gates: below 1.0, a replay, no total level, no device, a collision level of 0
    let mut ff = ForceFeedback::new(true, false);
    assert!(ff.collision_jolt(&c, 0x40000, Vec3::X, 0.9, LV, false, 0).is_empty());
    assert!(ff.collision_jolt(&c, 0x40000, Vec3::X, 5.0, LV, true, 0).is_empty());
    assert!(ff.collision_jolt(&c, 0x40000, Vec3::X, 5.0, FfLevels { total: 0, ..LV }, false, 0).is_empty());
    assert!(ff.collision_jolt(&c, 0x40000, Vec3::X, 5.0, FfLevels { collision: 0, ..LV }, false, 0).is_empty());
    assert!(ForceFeedback::new(false, false).collision_jolt(&c, 0x40000, Vec3::X, 5.0, LV, false, 0).is_empty());
}

#[test]
fn scrape_at_a_standstill() {
    // 0x412393: kind 0x10000 forces the direction to 0 and returns on a speed of exactly 0
    let mut ff = ForceFeedback::new(true, false);
    let mut c = car();
    c.speed = 0.0;
    assert!(ff.collision_jolt(&c, 0x10000, Vec3::new(1.0, 0.0, -1.0), 5.0, LV, false, 0).is_empty());
    c.speed = 3.0;
    let out = ff.collision_jolt(&c, 0x10000, Vec3::new(1.0, 0.0, -1.0), 5.0, LV, false, 0);
    assert!(matches!(out[0], FfCommand::Jolt { direction: 0, magnitude: 5000, duration_us: 100_000, .. }));
}

#[test]
fn wheel_quantises_the_direction() {
    // 0x412D5A…0x412DA6: 0 / 180 / 360 alternate with the clock's parity, (0, 180) → 90, the rest → 270
    for (now, dir0, want) in [(10, 22, 90), (10, 338, 270), (10, 0, 270), (11, 0, 90), (11, 180, 90), (10, 360, 270)] {
        let mut out = Vec::new();
        ForceFeedback::new(true, true).fire_jolt(dir0, 100_000, 5000, LV, now, &mut out);
        assert!(matches!(out[0], FfCommand::Jolt { direction, .. } if direction == want), "now {now} dir {dir0}");
    }
    // a pad keeps the direction as it is
    let mut out = Vec::new();
    ForceFeedback::new(true, false).fire_jolt(22, 100_000, 5000, LV, 10, &mut out);
    assert!(matches!(out[0], FfCommand::Jolt { direction: 22, .. }));
}

#[test]
fn driver_order_and_dedupe() {
    let mut ff = ForceFeedback::new(true, false);
    // 20 m/s = 44.8 mph; surface 1 is a stop case: no road effect, the spring and the engine run
    let out = ff.tick(&car(), LV, false, 0);
    assert!(is_spring(&out[0]) && matches!(out[1], FfCommand::Periodic { effect: FfEffect::Engine, .. }) && out.len() == 2, "{out:?}");
    // surface 4 (+1 Hz, +1000) on the < 60 mph row (11, 4500): road first, then the spring, then the
    // engine; a second tick with the same pair leaves the road alone (0x412B79…0x412B8F)
    let mut c = car();
    c.surface = 4;
    let out = ff.tick(&c, LV, false, 2);
    assert!(matches!(out[0], FfCommand::Periodic { effect: FfEffect::Road, magnitude: 5500, hz: 12, period_us: 83333, gain: 50 }), "{out:?}");
    assert!(is_spring(&out[1]) && matches!(out[2], FfCommand::Periodic { effect: FfEffect::Engine, .. }));
    assert_eq!(ff.tick(&c, LV, false, 4).len(), 2);
    // a standstill (< 2 mph) stops the road once
    let mut s = c;
    s.speed = 0.5;
    assert_eq!(ff.tick(&s, LV, false, 6)[0], FfCommand::Stop(FfEffect::Road));
    assert!(!ff.tick(&s, LV, false, 8).contains(&FfCommand::Stop(FfEffect::Road)));
}

#[test]
fn spring_conditions() {
    let mut ff = ForceFeedback::new(true, false);
    let out = ff.tick(&car(), LV, false, 0);
    // words {3600, −720, 800}: esi = ftol(ftol(3600) + 720) = 4320, offset ftol(800); condition 1 is
    // the 0x412730 value, 0 (car+0x11C0)
    assert_eq!(out[0], FfCommand::Spring { x: FfCondition { offset: 800, coefficient: 4320 }, y: FfCondition { offset: 0, coefficient: 0 }, gain: 50 });
}

#[test]
fn airborne_and_landing() {
    let mut ff = ForceFeedback::new(true, false);
    let mut c = car();
    c.surface = 4;
    assert!(!ff.tick(&c, LV, false, 0).is_empty());
    // `car+0x418` set: stops the road and the spring, arms the landing jolt (0x4124AC)
    c.air_count = 1;
    assert_eq!(ff.tick(&c, LV, false, 2), vec![FfCommand::Stop(FfEffect::Road), FfCommand::Stop(FfEffect::Grip)]);
    assert!(ff.tick(&c, LV, false, 4).is_empty());
    // the first grounded tick: the jolt (45 when the clock is odd), then the spring is held off
    // for two ticks (0x412568…0x41257B, 0x412707)
    c.air_count = 0;
    let out = ff.tick(&c, LV, false, 5);
    assert_eq!(
        out[0],
        FfCommand::Jolt { magnitude: 10000, duration_us: 500, direction: 45, attack_level: 10000, attack_time: 250, fade_level: 10000, fade_time: 500 - 10000, gain: 50 }
    );
    assert!(!out.iter().any(is_spring));
    assert!(!ff.tick(&c, LV, false, 7).iter().any(is_spring));
    assert!(ff.tick(&c, LV, false, 9).iter().any(is_spring));
    // an even clock lands at 315
    let mut ff = ForceFeedback::new(true, false);
    c.air_count = 1;
    ff.tick(&c, LV, false, 0);
    c.air_count = 0;
    assert!(matches!(ff.tick(&c, LV, false, 10)[0], FfCommand::Jolt { direction: 315, .. }));
    // a landing at a standstill fires no jolt (0x412530)
    let mut ff = ForceFeedback::new(true, false);
    c.air_count = 1;
    ff.tick(&c, LV, false, 0);
    c.air_count = 0;
    c.speed = 0.0;
    assert!(!ff.tick(&c, LV, false, 10).iter().any(|x| matches!(x, FfCommand::Jolt { .. })));
    // body height below 0 (0x412426): stop once, then silence
    let mut ff = ForceFeedback::new(true, false);
    c.speed = 20.0;
    ff.tick(&c, LV, false, 0);
    c.body_height = -0.1;
    assert_eq!(ff.tick(&c, LV, false, 2), vec![FfCommand::Stop(FfEffect::Road), FfCommand::Stop(FfEffect::Grip)]);
    assert!(ff.tick(&c, LV, false, 4).is_empty());
}

#[test]
fn gates() {
    let c = car();
    assert!(ForceFeedback::new(false, false).tick(&c, LV, false, 0).is_empty());
    assert!(ForceFeedback::new(true, false).tick(&c, LV, true, 0).is_empty());
    assert!(ForceFeedback::new(true, false).tick(&c, FfLevels { total: 0, ..LV }, false, 0).is_empty());
    // a level of 0 skips that effect only (0x4125D8, 0x4125F5, 0x41270D)
    let mut c = car();
    c.surface = 4;
    let out = ForceFeedback::new(true, false).tick(&c, FfLevels { road: 0, grip: 0, engine: 0, ..LV }, false, 0);
    assert!(out.is_empty());
}

#[test]
fn surface_table_and_damage() {
    // 0x412C98: 1 and 8 stop, 6 keeps; 9 only −3 Hz; tyre damage raises the class (≥ 30 → 3,
    // ≥ 45 → 2 when below 2), ≥ 60 or a wreck forces 7 (−5 Hz, +2000)
    let mut c = car();
    c.speed = 10.0; // 22.4 mph → (9, 4500)
    for (surface, dmg, wrecked, expect) in [
        (9, 0.0, false, Some((6, 4500))),
        (5, 0.0, false, Some((13, 3000))),
        (3, 0.0, false, Some((7, 3500))),
        (2, 0.0, false, Some((10, 4000))),
        (10, 0.0, false, Some((10, 4000))),
        (11, 0.0, false, Some((4, 6500))),
        (4, 65.0, false, Some((4, 6500))),
        (1, 0.0, true, Some((4, 6500))),
        (1, 31.0, false, Some((7, 3500))),
        (1, 0.0, false, None),
        (8, 0.0, false, None),
        (0, 0.0, false, None),
        (12, 0.0, false, None),
    ] {
        c.surface = surface;
        c.tyre_damage = dmg;
        c.wrecked = wrecked;
        let mut ff = ForceFeedback::new(true, false);
        let mut out = Vec::new();
        ff.road(&c, 50, &mut out);
        match expect {
            Some((hz, mag)) => assert!(matches!(out[0], FfCommand::Periodic { hz: h, magnitude: m, .. } if h == hz && m == mag), "surface {surface} dmg {dmg}: {out:?}"),
            None => assert!(out.is_empty(), "surface {surface}"),
        }
    }
    // class 6 leaves a playing effect exactly as it is, and records the pair
    let mut ff = ForceFeedback::new(true, false);
    c.surface = 4;
    c.tyre_damage = 0.0;
    c.wrecked = false;
    let mut out = Vec::new();
    ff.road(&c, 50, &mut out);
    c.surface = 6;
    out.clear();
    ff.road(&c, 50, &mut out);
    assert!(out.is_empty());
}
