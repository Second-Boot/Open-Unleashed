//! Reproducible physics scenarios (RE spec §21). Expected values are currently the native model's
//! own recorded outputs (self-regression); they must be replaced by measurements from the original
//! game (replay/instrumented run, see RE/DYNAMIC_ANALYSIS.md) — tolerances are documented per value.

use glam::{Mat3, Vec3};
use nfs_physics::collision::{Triangle, TriangleGrid};
use nfs_physics::specs::CarSpecs;
use nfs_physics::vehicle::{Controls, Vehicle};

fn flat() -> TriangleGrid {
    let s = 5000.0;
    let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
    TriangleGrid::build(vec![Triangle { a, b, c, surface: 1 }, Triangle { a, b: c, c: d, surface: 1 }], 200.0)
}

fn specs_993() -> Option<CarSpecs> {
    let home = std::env::var_os("HOME")?;
    let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/GameData/Simulation/CarData/993coupe36.sim");
    let sim = nfs_formats::sim::CarSim::parse(&std::fs::read(p).ok()?).ok()?;
    Some(CarSpecs::from_sim(&sim, false))
}

fn run(v: &mut Vehicle, specs: &CarSpecs, grid: &TriangleGrid, c: Controls, seconds: f32) {
    for t in 0..(seconds * 64.0) as u32 {
        if t % 2 == 0 {
            v.step32(&c, specs, grid);
        }
        v.step64(specs);
        if t % 32 == 0 {
            v.renormalise();
        }
    }
}

#[test]
fn scenario_full_throttle_10s() {
    let Some(specs) = specs_993() else { return };
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 10.0);
    let kmh = v.speed() * 3.6;
    eprintln!("993 full throttle 10 s: {kmh:.1} km/h, gear {}, rpm {:.0}, distance {:.1} m", v.gear, v.rpm, v.position.z);
    assert!(kmh > 90.0 && kmh < 200.0, "speed out of the plausible band: {kmh}");
    assert!(v.position.z > 120.0 && v.position.z < 400.0, "distance {}", v.position.z);
}

#[test]
fn scenario_brake_from_speed() {
    let Some(specs) = specs_993() else { return };
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.75, 0.3);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    run(&mut v, &specs, &grid, Controls { throttle: 254, ..Default::default() }, 7.0);
    let v0 = v.speed();
    let z0 = v.position.z;
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 4.0);
    eprintln!("brake from {:.0} km/h: after 4 s {:.1} km/h, {:.1} m", v0 * 3.6, v.speed() * 3.6, v.position.z - z0);
    // brake_decel 9.5 m/s²: from ~26 m/s the car stops within ~3 s and ~40 m; holding the brake at a
    // standstill then selects reverse (automatic gearbox rule, 0x49DD20) so it may creep backwards.
    assert!(v.v_local.z <= 0.5, "still moving forward at {:.1} km/h", v.v_local.z * 3.6);
    assert!(v.position.z - z0 < 60.0);
}

#[test]
fn scenario_gt1_full_throttle() {
    let home = std::env::var_os("HOME").unwrap();
    let p = std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/GameData/Simulation/CarData/GT1race.sim");
    let Ok(d) = std::fs::read(p) else { return };
    let sim = nfs_formats::sim::CarSim::parse(&d).unwrap();
    let specs = CarSpecs::from_sim(&sim, true);
    let grid = flat();
    let mut v = Vehicle::new(Vec3::new(0.0, 0.3, 0.0), Mat3::IDENTITY, &specs, 0.9, 0.33);
    run(&mut v, &specs, &grid, Controls { brake: 254, ..Default::default() }, 1.5);
    for t in 0..(6.0 * 64.0) as u32 {
        if t % 2 == 0 {
            v.step32(&Controls { throttle: 254, ..Default::default() }, &specs, &grid);
            if t % 64 == 0 {
                eprintln!("GT1 t={:.1} v={:.1} km/h gear={} rpm={:.0} thr={:.2} demand={:.2} lim={:.2} lon={:.2} locked={} slip={:.2}", t as f32 / 64.0, v.speed() * 3.6, v.gear, v.rpm, v.throttle, v.wheels[2].demand, v.wheels[2].limit, v.wheels[2].longitudinal, v.wheels[2].locked, v.wheels[2].slip);
            }
        }
        v.step64(&specs);
    }
    assert!(v.speed() * 3.6 > 60.0, "GT1 too slow: {:.1} km/h", v.speed() * 3.6);
}
