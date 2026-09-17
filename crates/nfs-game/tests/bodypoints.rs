//! The collision body a car actually carries (`BndP`, `RE/notes/car-model.md` §12) in the physics
//! body frame.
use nfs_assets::{car::CarAssets, GamePaths};

fn points(model: &str) -> Option<Vec<glam::Vec3>> {
    let paths = GamePaths::locate().ok()?;
    let car = CarAssets::load(&paths, model).ok()?;
    Some(nfs_game::world::car_body_points(&car))
}

#[test]
fn every_car_has_the_same_default_body() {
    for model in ["993", "356_1", "Boxster", "GT1"] {
        let Some(p) = points(model) else {
            eprintln!("skipping: no game data");
            return;
        };
        assert_eq!(p.len(), 24, "{model}");
        // sorted by height, and the four lowest are the wheel contact quad on the body origin's plane
        assert!(p.windows(2).all(|w| w[0].y <= w[1].y), "{model}: not sorted by height");
        assert!(p[..4].iter().all(|q| q.y.abs() < 0.01), "{model}: the lowest four are not on y = 0");
        // the quad is symmetric across the car and spans a plausible wheelbase and track
        let (xmin, xmax) = p[..4].iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.x), b.max(q.x)));
        let (zmin, zmax) = p[..4].iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.z), b.max(q.z)));
        assert!((xmin + xmax).abs() < 0.05, "{model}: wheel quad off centre");
        assert!((1.2..2.2).contains(&(xmax - xmin)), "{model}: track {} m", xmax - xmin);
        assert!((1.8..2.8).contains(&(zmax - zmin)), "{model}: wheelbase {} m", zmax - zmin);
        // the roof is the highest point and sits where a car's roof sits
        assert!((0.8..1.6).contains(&p[23].y), "{model}: roof at {} m", p[23].y);
    }
}

/// The body frame is the physics one: +x right, +z forward. The front box reaches further forward
/// than the wheel quad does, and the rear box further back.
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
