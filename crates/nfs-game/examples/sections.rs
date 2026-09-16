//! Diagnostic: section endpoints and their nearest connections.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let track = TrackAssets::load(&paths, &args[0]).unwrap();
    let t = SliceTable::from_crp(&track.crp, &track.crp_data).unwrap();
    println!("{}: slices={} sections={}", args[0], t.slices.len(), t.sections.len());
    for (i, s) in t.sections.iter().enumerate() {
        let a = &t.slices[s.first];
        let b = &t.slices[s.last];
        // nearest section start to this section's end (continuing forward)
        let mut best = (f32::MAX, 0usize);
        for (j, o) in t.sections.iter().enumerate() {
            let c = &t.slices[o.first];
            let d = (c.centre - (b.centre + b.forward * 6.0)).length();
            if d < best.0 { best = (d, j); }
        }
        println!("sec {i}: {}..{} ({} slices) start=({:.0},{:.0},{:.0}) end=({:.0},{:.0},{:.0}) -> next start sec {} at {:.1} m; lanes first={:?}", s.first, s.last, s.last - s.first + 1, a.centre.x, a.centre.y, a.centre.z, b.centre.x, b.centre.y, b.centre.z, best.1, best.0, (a.lanes_a, a.lanes_b, a.lane_mask));
    }
}
