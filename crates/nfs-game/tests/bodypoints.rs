//! The collision body a car actually carries (`BndP`, `RE/notes/car-model.md` §12) in the physics
//! body frame.
use nfs_assets::{car::CarAssets, GamePaths};

fn points(model: &str) -> Option<Vec<glam::Vec3>> {
    styled(model, 0)
}

/// The same, for one body style — `nfs5.car +0x1C0`, 0-based (`car-model.md`, `applyStyle`
/// 0x437B00).
fn styled(model: &str, variant: i16) -> Option<Vec<glam::Vec3>> {
    let paths = GamePaths::locate().ok()?;
    let car = CarAssets::load(&paths, model).ok()?.with_style(variant);
    Some(nfs_game::world::car_body_points(&car))
}

#[test]
fn every_car_has_the_same_default_body() {
    // The GT1 carries four more: a `slot 42 = 0` record, and its `[style1]` names no slots at
    // all, so slot 42 holds the `memset`'s zero and the record is **on** (0x4420B9). Those four
    // points sit at z 2.0…2.4, y ≈ 0.95, x ±1.0 — the fixed rear wing. Before the style array
    // was wired through, every style-selected record was dropped and every car got 24.
    for (model, want) in [("993", 24), ("356_1", 24), ("Boxster", 24), ("GT1", 28)] {
        let Some(p) = points(model) else {
            eprintln!("skipping: no game data");
            return;
        };
        assert_eq!(p.len(), want, "{model}");
        // sorted by height (0x49ABC0) — the first four then permuted by quadrant — and the four
        // lowest are the wheel contact quad on the body origin's plane
        assert!(p[3..].windows(2).all(|w| w[0].y <= w[1].y), "{model}: not sorted by height");
        assert!(p[..4].iter().all(|q| p[4..].iter().all(|r| q.y <= r.y)), "{model}: the first four are not the lowest");
        // FL, FR, RL, RR by the signs of x and z
        for (i, q) in p[..4].iter().enumerate() {
            assert_eq!((q.x > 0.0, q.z > 0.0), (i % 2 == 1, i < 2), "{model}: probe {i} at {q:?}");
        }
        assert!(p[..4].iter().all(|q| q.y.abs() < 0.01), "{model}: the lowest four are not on y = 0");
        // the quad is symmetric across the car and spans a plausible wheelbase and track
        let (xmin, xmax) = p[..4].iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.x), b.max(q.x)));
        let (zmin, zmax) = p[..4].iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.z), b.max(q.z)));
        assert!((xmin + xmax).abs() < 0.05, "{model}: wheel quad off centre");
        assert!((1.2..2.2).contains(&(xmax - xmin)), "{model}: track {} m", xmax - xmin);
        assert!((1.8..2.8).contains(&(zmax - zmin)), "{model}: wheelbase {} m", zmax - zmin);
        // the roof is the highest point and sits where a car's roof sits
        assert!((0.8..1.6).contains(&p[p.len() - 1].y), "{model}: highest point at {} m", p[p.len() - 1].y);
    }
}

/// The points are the model's own axes, used unturned as the physics body frame (0x442080,
/// 0x49AE30): both boxes overhang the wheel quad, whichever end they are on.
#[test]
fn the_body_is_oriented_like_the_physics_frame() {
    let Some(p) = points("993") else { return };
    let (zmin, zmax) = p.iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.z), b.max(q.z)));
    let (wheel_min, wheel_max) = p[..4].iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.z), b.max(q.z)));
    assert!(zmax > wheel_max + 0.2, "no front overhang: {zmax} vs {wheel_max}");
    assert!(zmin < wheel_min - 0.2, "no rear overhang: {zmin} vs {wheel_min}");
    // a 993 is about 4.25 m long and 1.8 m wide
    assert!((3.8..4.6).contains(&(zmax - zmin)), "length {} m", zmax - zmin);
}


/// The 993's two spoiler records are `slot 16` values 1 and 2, and which of them the car carries
/// is the body style's business: `[style1]` (Carrera coupé) names no slot 16, so it holds zero and
/// neither is on; `[style8]` (GT2 Racecar) sets `geometry=16, type=2`, so the tall one is.
#[test]
fn a_993s_spoiler_comes_with_its_body_style() {
    let Some(coupe) = styled("993", 0) else {
        eprintln!("skipping: no game data");
        return;
    };
    let Some(gt2) = styled("993", 7) else { return };
    assert_eq!(coupe.len(), 24, "the Carrera coupé has no spoiler");
    assert_eq!(gt2.len(), 28, "the GT2 racecar does");
    // and the four extra points are above and behind the coupé's rearmost
    let coupe_zmin = coupe.iter().fold(f32::MAX, |a, q| a.min(q.z));
    let extra: Vec<_> = gt2.iter().filter(|q| q.z < coupe_zmin + 0.01 || q.y > 1.05).collect();
    assert!(extra.len() >= 4, "the spoiler points are not where a spoiler is: {extra:?}");
}

/// `car+0x3A8…+0x3B0` of a racer — half the LOD-5 model box, the box holding the origin (0x445090
/// → 0x43DE90) — and the `BndP` points lowered by its `y` (0x4421BE): the wheel quad sits
/// `car+0x3AC` below the body origin.
#[test]
fn a_racers_body_origin_is_half_the_model_box_up() {
    for model in ["993", "356_1", "Boxster", "GT1"] {
        let Ok(paths) = GamePaths::locate() else { return };
        let Ok(car) = CarAssets::load(&paths, model) else { return };
        let car = car.with_style(0);
        let (lo, hi) = nfs_game::world::car_model_box(&car, 5);
        let (pts, half) = nfs_game::world::car_body(&car, false);
        eprintln!("{model}: box {lo:?} … {hi:?}, half {half:?}, wheel quad y {}", pts[0].y);
        assert!(lo.y.abs() < 0.05, "{model}: the model stands on y = 0 ({lo:?})");
        assert!((0.4..0.8).contains(&half.y), "{model}: half height {}", half.y);
        assert!(pts[..4].iter().all(|q| (q.y + half.y).abs() < 0.01), "{model}: wheel quad not at −halfH");
        let (tp, th) = nfs_game::world::car_body(&car, true);
        assert!((th - half * 0.4).length() < 1e-5 && (tp[0].y + th.y).abs() < 0.01, "{model}: tiny");
    }
}
