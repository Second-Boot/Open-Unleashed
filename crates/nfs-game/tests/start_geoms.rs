//! The start scene's GEOM elements (`RE/notes/race-logic.md` §3.1, §3.4): 0x46D1D0 places the
//! `CGeomElem`s of the list `WTrack::load` built — `_BStart.scn` in a backwards race, `_FStart.scn`
//! otherwise, and in a skill test only its `_ST<N>.scn` (0x46D330 and 0x46D600 are exclusive).
use nfs_assets::{track::TrackAssets, GamePaths};

#[test]
fn coastal_places_the_geoms_of_the_start_scene_it_loads() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut t = TrackAssets::load(&paths, "coastal").unwrap();
    let fnsh = u32::from_le_bytes(*b"FNSH");
    let placed = |t: &TrackAssets| -> Vec<glam::Vec3> {
        let g = t.build_geometry().unwrap();
        g.instances.iter().filter(|i| i.source == 0 && i.tag == fnsh).map(|i| i.transform.w_axis.truncate()).collect()
    };
    // coastal_fstart.scn: two banners at the forward finish, x ≈ 2315
    let fwd = placed(&t);
    assert_eq!(fwd.len(), 2);
    assert!(fwd.iter().all(|p| p.x > 2000.0), "{fwd:?}");
    // coastal_bstart.scn: four at the backwards finish, x ≈ −110
    t.reverse = true;
    let back = placed(&t);
    assert_eq!(back.len(), 4);
    assert!(back.iter().all(|p| p.x < 0.0), "{back:?}");
    // a skill test loads its own scene instead of either
    t.extra_scenes.push(nfs_formats::scn::Scene::parse("").unwrap());
    assert!(placed(&t).is_empty());
}
