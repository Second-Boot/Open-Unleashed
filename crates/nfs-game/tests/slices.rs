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
    let (pos, rot, slice) = grid_pose(&table, t.start_forward.as_ref(), 0, 1, &nfs_game::race::GridRule { linear: true, derby_cars: None });
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
    let walls = nfs_physics::walls::WallSet::build(edges.segments.iter().map(|s| (s.p1, s.p2, s.tag)), 30.0);
    assert_eq!(walls.len(), 7694);
    // point known to be 0.99 m from segment (199.6,68.9,-529.6)-(190.1,70.2,-535.2)
    let hit = walls.nearest(Vec3::new(193.9, 66.6, -531.8), 1.3);
    eprintln!("nearest: {hit:?}");
    assert!(hit.is_some());
    // start line centre is ~8 m from the edges: no hit with a 1.3 m radius
    assert!(walls.nearest(Vec3::new(-145.5, 0.85, 227.1), 1.3).is_none());
}

#[test]
fn vehicle_wall_response_synthetic() {
    use nfs_physics::{specs::CarSpecs, vehicle::Vehicle, walls::WallSet};
    let Ok(paths) = GamePaths::locate() else { return };
    let sim = nfs_assets::car::CarAssets::load_sim(&paths, "993coupe36").unwrap();
    let specs = CarSpecs::from_sim(&sim, false);
    // wall along z at x = 2 (edge direction +z ⇒ normal (−1, 0, 0) points toward −x, the car's side)
    let walls = WallSet::build(std::iter::once(([2.0, 0.0, -20.0], [2.0, 0.0, 20.0], 0x10)), 30.0);
    // the sweep runs after integration: the +x corners (x ≈ pos.x + 0.9) have just passed the edge
    let mut v = Vehicle::new(Vec3::new(1.5, 0.3, 0.0), glam::Mat3::IDENTITY, &specs, 0.75, 0.3);
    v.velocity = Vec3::new(64.0, 0.0, 0.0); // 2 m per tick
    let hit = v.collide_walls(&walls, &|_| None, 0.0, 0.0, 0.0);
    eprintln!("hit={hit} pos={:?} vel={:?}", v.position, v.velocity);
    assert!(hit);
    assert!(v.velocity.x < 0.0 && v.velocity.x > -10.0, "restitution 0.1 expected, got {:?}", v.velocity);
    assert!(v.position.x < 1.5, "pushed back off the wall: {:?}", v.position);
}

#[test]
fn monaco1_lap_chains_all_sections() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "monaco1").unwrap();
    let table = SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    assert_eq!(table.sections.len(), 4);
    // the street circuit is stored as four sections; the lap runs 0 → 2 → 1 → 3
    assert_eq!(table.lap_len, 623);
    assert_eq!(table.route[141], 557);
    let mut max_gap = 0.0f32;
    for p in 0..table.lap_len {
        let a = table.slices[table.route[p]].centre;
        let b = table.slices[table.route[(p + 1) % table.lap_len]].centre;
        max_gap = max_gap.max((a - b).length());
    }
    assert!(max_gap < 15.0, "max gap along the lap {max_gap}"); // 13.3 m at the section 3 → 0 join
    assert_eq!(table.add(140, 1), 557);
    // stepping back across a node lands two slices short: 0x4817D0 keeps `s − edge − 1` as the
    // remainder in both directions (0x481877 / 0x48187B) and adds it to the exit link's slice, the
    // segment's last (140), so 557 − 1 is 138
    assert_eq!(table.add(557, -1), 138);
    assert!((table.frac(557) - 141.0 / 623.0).abs() < 1e-4);
    // find_slice lands on the chained section, not on the file-order neighbour
    let probe = table.slices[560].centre + Vec3::new(0.5, 0.0, 0.5);
    assert_eq!(table.find_slice(probe, 138), 560);
    // point-to-point: section 0 alone is the lap
    let alps = TrackAssets::load(&paths, "alps").unwrap();
    let a = SliceTable::from_crp(&alps.crp, &alps.crp_data).unwrap();
    assert_eq!(a.route.len(), 1795);
    // and it does not wrap: past a dead end 0x4817D0 hands back the slice it was given (0x4818E8)
    assert_eq!(a.add(1794, 1), 1794);
}

/// A car on one of a track's branch roads is tracked onto that road, not snapped to the main lap
/// (`0x481400`'s whole-array hill-climb plus the junction step of `0x481190`).
#[test]
fn a_car_on_a_shortcut_is_tracked_onto_it() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // castle drives three of its nineteen sections; the rest are real roads the tracker used to
    // ignore, 1492 slices of them
    let t = TrackAssets::load(&paths, "castle").unwrap();
    let table = SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    assert_eq!(table.slices.len(), 3443);
    assert_eq!(table.lap_len, 1951);
    let off_lap = (0..table.slices.len()).filter(|s| !table.route.contains(s)).count();
    assert_eq!(off_lap, 1492);

    // every off-lap slice the tracker is handed answers with itself, or with a slice of the same
    // road within a few metres — never with a far-away lap slice
    let mut worst = 0.0f32;
    let mut checked = 0;
    for s in (0..table.slices.len()).step_by(17) {
        let got = table.find_slice(table.slices[s].centre, s);
        let d = (table.slices[got].centre - table.slices[s].centre).length();
        worst = worst.max(d);
        checked += 1;
    }
    assert!(checked > 150);
    assert!(worst < 4.0, "worst mis-track {worst} m");

    // and the junction catchments exist: castle's nodes claim slices, and a claimed slice names the
    // nearest node
    let claimed = table.junctions.near_node.iter().filter(|n| n.is_some()).count();
    assert!(claimed > 200, "only {claimed} slices are in a junction catchment");
    let lists: usize = table.junctions.node_slices.iter().map(|v| v.len()).sum();
    assert!(lists >= claimed);
}
