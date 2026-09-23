//! Diagnostic: where the tracker puts a car at (x, z) that came from slice `prev`, with and without
//! a preferred segment — `find_slice`, `track_slice` and the junction catchment behind them.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = nfs_assets::GamePaths::locate().unwrap();
    let t = nfs_assets::track::TrackAssets::load(&paths, &a[0]).unwrap();
    let mut table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    if a.iter().any(|v| v == "--backwards") {
        table = table.reversed();
    }
    let (x, z): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let prev: usize = a[3].parse().unwrap();
    let y = table.slices[prev].centre.y;
    let pos = glam::Vec3::new(x, y, z);
    let d = |s: usize| (table.slices[s].centre - pos).length();
    let near = table.junctions.near_node.get(prev).copied().flatten();
    println!("prev {prev} at {:.0} m, near_node={near:?}, slice_node={:?}", d(prev), table.junctions.slice_node.get(prev).copied().flatten());
    let fs = table.find_slice(pos, prev);
    println!("find_slice -> {fs} at {:.0} m", d(fs));
    for pref in [None, Some(15usize)] {
        let s = table.track_slice(pos, prev, pref);
        println!("track_slice(pref={pref:?}) -> {s} at {:.0} m", d(s));
    }
    let mut best = (f32::MAX, 0usize);
    for i in 0..table.slices.len() {
        let dd = d(i);
        if dd < best.0 { best = (dd, i); }
    }
    println!("true nearest slice {} at {:.1} m", best.1, best.0);
}
