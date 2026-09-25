//! Skill-test scene parsing against the shipped `skidpad_st1.scn` (mission 0M01).
use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::skill::SkillTest;

#[test]
fn skidpad_st1_layout() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "skidpad").unwrap();
    let scene = t.load_scene("_st1.scn").unwrap().expect("skidpad_st1.scn");
    let st = SkillTest::from_scene(&scene, "skidpad");
    // RE/notes/skill-tests.md §13: 5 checkpoints + finish, 6 cones, 5 arrows; start heading −Z
    assert_eq!(st.checkpoints.iter().filter(|c| c.kind == 0).count(), 5);
    assert_eq!(st.checkpoints.iter().filter(|c| c.kind == 1).count(), 1);
    assert_eq!(st.checkpoints.iter().filter(|c| c.kind == 2).count(), 1);
    assert_eq!(st.cones.len(), 6);
    assert_eq!(st.arrows.len(), 5);
    let (pos, fwd) = st.start_pose().unwrap();
    assert!((pos - Vec3::new(-6.844955, 0.0, 147.24066)).length() < 1e-3);
    assert!((fwd - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-3);
    // links point at arrow elements, sequence ids are 1..=6 in order
    assert_eq!(st.initial_arrow(), Some(13));
    let seqs: Vec<i32> = st.checkpoints.iter().filter(|c| c.kind != 2).map(|c| c.sequence).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
    // every link points at an arrow element; one checkpoint (seq 4) carries no link
    assert!(st.checkpoints.iter().all(|c| c.link.map(|l| st.arrows.contains(&l)).unwrap_or(true)));
    assert_eq!(st.checkpoints.iter().filter(|c| c.link.is_none()).count(), 2);
    assert!(st.skidpad);
}

/// The shipped cones as the smackables the race builds (`nfs_physics::smackable`): six on the
/// `0M01` course, each a 20 kg pylon (class 0x10), seated on the pad and asleep; a car driven
/// into one at 15 m/s wakes it, knocks it over (the solver's `car+0x42C`) and sends it off.
#[test]
fn skidpad_st1_cones_are_smackables() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut t = TrackAssets::load(&paths, "skidpad").unwrap();
    let scene = t.load_scene("_st1.scn").unwrap().expect("skidpad_st1.scn");
    t.extra_scenes.push(scene);
    let g = t.build_geometry().unwrap();
    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
    let spawns: Vec<_> = g
        .instances
        .iter()
        .filter_map(|i| g.library.iter().find(|a| a.tag == i.tag).and_then(|a| nfs_game::world::smackable_spawn(&t, a, i)))
        .collect();
    assert_eq!(spawns.len(), 6);
    let s = &spawns[0];
    eprintln!("cone points {:?}", s.points);
    assert!(!s.points.is_empty() && s.points.len() <= nfs_physics::smackable::MAX_POINTS);
    assert_eq!((s.mass, s.audio_type), (20.0, 4));
    let mut cone = nfs_physics::smackable::Smackable::new(s.position, s.orientation, s.points.clone(), s.mass, s.audio_type, &grid);
    eprintln!("half extents {:?} height {} / {}", cone.body.half_extents, cone.body.body_height, cone.body.min_height);
    assert!(cone.asleep && cone.body.material_class == 0x10);
    assert!(cone.body.body_height.abs() < 0.05, "seated on the pad: {}", cone.body.body_height);
    // a car-sized box coming at it along x
    let fwd = Vec3::X;
    let start = s.position - fwd * 6.0 + Vec3::Y * 0.6;
    let mut car = nfs_physics::vehicle::Vehicle::bare(start, glam::Mat3::from_cols(Vec3::NEG_Z, Vec3::Y, Vec3::X).transpose(), 2.4, 1300.0, 0.8, 0.3);
    car.half_extents = Vec3::new(0.9, 0.6, 2.1);
    car.velocity = fwd * 15.0;
    let mut hit = false;
    for _ in 0..64 {
        cone.damp(&[car.position]);
        cone.ground(&grid);
        let mut others = [(&mut car, 1300.0f32 / 128.0)];
        hit |= !cone.collide(&mut others, &mut |_| {}).is_empty();
        cone.post_impact();
        for _ in 0..2 {
            car.integrate64();
            cone.integrate();
        }
    }
    assert!(hit && cone.body.knocked_over);
    eprintln!("cone after a second: {:?} v {:?}", cone.body.position, cone.body.velocity);
    assert!((cone.body.position - s.position).length() > 3.0, "{:?}", cone.body.position);
    assert!(cone.body.position.y.is_finite() && cone.body.position.y < s.position.y + 5.0, "{:?}", cone.body.position);
}
