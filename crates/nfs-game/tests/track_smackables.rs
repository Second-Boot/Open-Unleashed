//! The track's own smackables (`RE/notes/suspension-collision.md` §9 "The track's smackables"):
//! castle's placed `kSmackable` articles become car records — the hydrants among them — and a car
//! driven into one pushes it off its place (what turns its player's state to 1, 0x4835B0).
use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};

#[test]
fn castle_hydrants_are_smackables_a_car_can_knock_over() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "castle").unwrap();
    let arts = nfs_game::world::track_smackable_articles(&t);
    // `nfs-game/examples/tracksmackables.rs`: 69 placed, none of the library templates
    assert_eq!(arts.len(), 69);
    let g = t.build_geometry().unwrap();
    let spawns: Vec<_> = arts.iter().filter_map(|k| g.world.iter().find(|a| a.index == *k)).filter_map(|a| nfs_game::world::track_smackable_spawn(&t, a)).collect();
    assert_eq!(spawns.len(), 69);
    let hydrant = g.world.iter().find(|a| a.index == arts[0]).unwrap();
    assert_eq!(hydrant.tag, u32::from_le_bytes(*b"HYDR"));
    let s = &spawns[0];
    // `HYDR`: 240 kg, `kAudMetalSign` (1 → class 0x12)
    assert_eq!((s.mass, s.audio_type), (240.0, 1));
    assert!(!s.points.is_empty() && s.points.len() <= nfs_physics::smackable::MAX_POINTS);
    // a `SIGN` takes 0x476B30: a slab, points on both sides of its face
    let sign = arts.iter().zip(&spawns).find(|(k, _)| g.world.iter().find(|a| a.index == **k).is_some_and(|a| a.tag == u32::from_le_bytes(*b"SIGN"))).map(|(_, s)| s).unwrap();
    assert!(sign.points.iter().any(|p| p.z > 0.1) && sign.points.iter().any(|p| p.z < -0.1), "{:?}", sign.points);

    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
    let mut prop = nfs_physics::smackable::Smackable::new(s.position, s.orientation, s.points.clone(), s.mass, s.audio_type, &grid);
    assert!(prop.asleep && prop.body.material_class == 0x12);
    let home = prop.body.position;
    let fwd = Vec3::X;
    let mut car = nfs_physics::vehicle::Vehicle::bare(home - fwd * 5.0 + Vec3::Y * 0.6, glam::Mat3::from_cols(Vec3::NEG_Z, Vec3::Y, Vec3::X).transpose(), 2.4, 1300.0, 0.8, 0.3);
    car.half_extents = Vec3::new(0.9, 0.6, 2.1);
    car.velocity = fwd * 12.0;
    let mut hit = false;
    for i in 0..64u64 {
        prop.track(&grid, &[car.position]);
        prop.damp(&[car.position]);
        prop.ground(&grid, 2 * i);
        let mut others = [(&mut car, 1300.0f32 / 128.0)];
        hit |= !prop.collide(&mut others, &mut |_| {}).is_empty();
        prop.post_impact();
        for _ in 0..2 {
            car.integrate64();
            prop.integrate();
        }
    }
    let moved = prop.body.position - home;
    assert!(hit, "the car met the hydrant");
    assert!(moved.x.abs() > 0.01 || moved.z.abs() > 0.01, "pushed off its place: {moved:?}");
}
