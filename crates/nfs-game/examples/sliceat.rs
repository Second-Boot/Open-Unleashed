//! Diagnostic: one slice's centre, axes and section — for checking a tracker that disagrees with
//! the geometry.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = nfs_assets::GamePaths::locate().unwrap();
    let t = nfs_assets::track::TrackAssets::load(&paths, &a[0]).unwrap();
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    for s in a[1..].iter().filter_map(|v| v.parse::<usize>().ok()) {
        let sl = &table.slices[s];
        let sec = table.sections.iter().position(|x| s >= x.first && s <= x.last);
        println!(
            "slice {s:5} centre=({:8.1},{:7.1},{:8.1}) fwd=({:5.2},{:5.2}) lat=({:5.2},{:5.2}) lanes={}/{} widths={:.1}/{:.1} section={:?} route_pos={:?}",
            sl.centre.x, sl.centre.y, sl.centre.z, sl.forward.x, sl.forward.z, sl.lateral.x, sl.lateral.z,
            sl.lanes_a, sl.lanes_b, sl.lane_width_a, sl.lane_width_b, sec, table.route_pos.get(s)
        );
    }
}
