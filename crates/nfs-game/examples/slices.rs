//! Diagnostic: dump slice geometry (heading change, widths, section) for a track range.
//! `cargo run -p nfs-game --example slices -- alps 280 340`
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let track = TrackAssets::load(&paths, &args[0]).unwrap();
    let t = SliceTable::from_crp(&track.crp, &track.crp_data).unwrap();
    let (a, b): (usize, usize) = (args[1].parse().unwrap(), args[2].parse().unwrap());
    println!("slices={} lap_len={} sections={:?}", t.slices.len(), t.lap_len, t.sections.iter().map(|s| (s.first, s.last)).collect::<Vec<_>>());
    for i in a..b.min(t.slices.len()) {
        let s = &t.slices[i];
        let ang = |k: i32| {
            let j = t.add(i, k);
            t.slices[j].forward.dot(s.forward).clamp(-1.0, 1.0).acos().to_degrees()
        };
        let next = &t.slices[t.add(i, 1)];
        let gap = (next.centre - s.centre).length();
        println!("{i:4} pos=({:7.1},{:6.1},{:7.1}) gap={gap:5.1} w=({:4.1},{:4.1}) turn@5={:5.1} @10={:5.1} @20={:5.1} @30={:5.1} lanes={:?}", s.centre.x, s.centre.y, s.centre.z, s.width_a, s.width_b, ang(5), ang(10), ang(20), ang(30), (s.lanes_a, s.lanes_b, s.lane_mask));
    }
}
