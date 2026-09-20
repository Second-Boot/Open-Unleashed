//! The driver-view camera keys of the shipped cars.
use nfs_assets::GamePaths;

/// The driver view's eye point, from the car's own `.tpg` (`RE/notes/car-model.md` §3). The file's
/// axes are the body frame's: +x right, +y up, +z forward (`DriverCam::from_tpg`).
#[test]
fn driver_camera_keys_read_from_the_tpg() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.car_model_dir();
    let load = |name: &str| -> Option<nfs_assets::car::DriverCam> {
        let t = std::fs::read(dir.join(format!("{name}.tpg"))).ok()?;
        let ini = nfs_formats::ini::Ini::parse(&String::from_utf8_lossy(&t));
        nfs_assets::car::DriverCam::from_tpg(&ini, 1)
    };

    // a left-hand-drive road car: the driver sits to the left of centre and a little behind the
    // wheelbase midpoint the body origin is at
    let c = load("993").expect("993.tpg");
    assert!((c.position.x - -0.3828).abs() < 1e-3, "left of centre: {}", c.position.x);
    assert!((c.position.y - 0.3975).abs() < 1e-3);
    assert!((c.position.z - -0.1550).abs() < 1e-3, "behind the body origin: {}", c.position.z);
    assert!((c.rotation.x - 3.65).abs() < 1e-2, "a few degrees of downward pitch");

    // the front-engined 928's driver sits further back still, the GT1's — a race car with the
    // cockpit pushed forward — ahead of the origin: +z is forward
    assert!(load("928").expect("928.tpg").position.z < -0.5);
    assert!(load("gt1").expect("gt1.tpg").position.z > 0.2);

    // the right-hand-drive GT1 sits on the other side, lower, and looks further down
    let g = load("gt1").expect("gt1.tpg");
    assert!(g.position.x > 0.0, "right of centre: {}", g.position.x);
    assert!(g.position.y < c.position.y, "a race car's eye point is lower");
    assert!(g.rotation.x > c.rotation.x);

    // and its style 0 has its own pair of keys, which win over the plain ones
    let t = std::fs::read(dir.join("gt1.tpg")).unwrap();
    let ini = nfs_formats::ini::Ini::parse(&String::from_utf8_lossy(&t));
    let s0 = nfs_assets::car::DriverCam::from_tpg(&ini, 0).unwrap();
    assert!((s0.position.x - 0.2884).abs() < 1e-3);
    assert!((s0.rotation.x - 13.3).abs() < 1e-2);
}

/// A mirrored race negates the **x** of both triples — `RCar::load` 0x441E16…0x441E41
/// (`RE/notes/car-model.md`). The driver moves to the other side of the car.
#[test]
fn a_mirrored_race_puts_the_driver_on_the_other_side() {
    let dc = nfs_assets::car::DriverCam { position: glam::Vec3::new(-0.38, 0.4, -0.15), rotation: glam::Vec3::new(2.0, 1.0, 0.0) };
    let m = dc.mirrored();
    assert_eq!(m.position.x, 0.38);
    assert_eq!(m.rotation.x, -2.0);
    // and nothing else moves
    assert_eq!((m.position.y, m.position.z), (dc.position.y, dc.position.z));
    assert_eq!((m.rotation.y, m.rotation.z), (dc.rotation.y, dc.rotation.z));
    // twice is the identity
    assert_eq!(m.mirrored().position, dc.position);
}
