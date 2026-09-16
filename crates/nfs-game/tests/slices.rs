use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::{grid_pose, SliceTable, SLICE_SPACING};

#[test]
fn alps_slices() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "alps").unwrap();
    let table = SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    assert_eq!(table.slices.len(), 2105);
    assert_eq!(table.sections.iter().map(|s| s.last - s.first + 1).collect::<Vec<_>>(), vec![1795, 272, 22, 16]);
    assert_eq!(table.lap_len, 1795);
    // spacing ≈ 6 m along the main lap
    let mut total = 0.0;
    for i in 0..table.lap_len - 1 {
        total += (table.slices[i + 1].centre - table.slices[i].centre).length();
    }
    let mean = total / (table.lap_len - 1) as f32;
    assert!((mean - SLICE_SPACING).abs() < 0.3, "mean spacing {mean}");
    // unit vectors, lateral = up × forward
    let s = &table.slices[100];
    assert!((s.forward.length() - 1.0).abs() < 1e-3 && (s.up.length() - 1.0).abs() < 1e-3);
    assert!((s.up.cross(s.forward) - s.lateral).length() < 0.05, "lateral {:?} vs {:?}", s.lateral, s.up.cross(s.forward));
    // start trigger lies near its slice
    let (pos, rot, slice) = grid_pose(&table, t.start_forward.as_ref(), 0, 1);
    let trig = t.start_forward.as_ref().unwrap().of_kind(&nfs_formats::scn::ElementKind::Trigger).next().unwrap().position;
    let d = (pos - Vec3::from(trig)).length();
    eprintln!("start slice {slice} at {pos:?}, trigger at {trig:?}, distance {d:.1} m, forward {:?}", rot.row(2));
    assert!(d < 20.0);
}

#[test]
fn alps_walls_near_road_edge() {
    let Ok(paths) = GamePaths::locate() else { return };
    let t = TrackAssets::load(&paths, "alps").unwrap();
    let edges = t.edges.as_ref().unwrap();
    let walls = nfs_physics::walls::WallSet::build(edges.segments.iter().map(|s| (s.p1, s.p2)), 30.0);
    assert_eq!(walls.len(), 7694);
    // point known to be 0.99 m from segment (199.6,68.9,-529.6)-(190.1,70.2,-535.2)
    let hit = walls.nearest(Vec3::new(193.9, 66.6, -531.8), 1.3);
    eprintln!("nearest: {hit:?}");
    assert!(hit.is_some());
    // start line centre is ~8 m from the edges: no hit with a 1.3 m radius
    assert!(walls.nearest(Vec3::new(-145.5, 0.85, 227.1), 1.3).is_none());
}

#[test]
fn vehicle_wall_response_on_alps() {
    use nfs_physics::{specs::CarSpecs, vehicle::Vehicle};
    let Ok(paths) = GamePaths::locate() else { return };
    let t = TrackAssets::load(&paths, "alps").unwrap();
    let walls = nfs_physics::walls::WallSet::build(t.edges.as_ref().unwrap().segments.iter().map(|s| (s.p1, s.p2)), 30.0);
    let sim = nfs_assets::car::CarAssets::load_sim(&paths, "993coupe36").unwrap();
    let specs = CarSpecs::from_sim(&sim, false);
    let mut v = Vehicle::new(Vec3::new(193.82, 66.50, -535.45), glam::Mat3::IDENTITY, &specs, 0.75, 0.3);
    let hit = v.collide_walls(&walls, 0.93, 0.2, 0.15);
    eprintln!("hit={hit} pos={:?}", v.position);
    assert!(hit);
}
