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
