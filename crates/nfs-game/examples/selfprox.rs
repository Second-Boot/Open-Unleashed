//! Diagnostic: route points that come close to a much later part of the route (parallel roads).
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let track = TrackAssets::load(&paths, &args[0]).unwrap();
    let t = SliceTable::from_crp(&track.crp, &track.crp_data).unwrap();
    let n = t.lap_len;
    let mut last_report = 0usize;
    for p in 0..n {
        let a = t.slices[t.route[p]].centre;
        for q in (p + 40)..n {
            let b = t.slices[t.route[q]].centre;
            let dx = a.x - b.x;
            let dz = a.z - b.z;
            if dx * dx + dz * dz < 30.0 * 30.0 && p >= last_report + 20 {
                let fa = t.slices[t.route[p]].forward;
                let fb = t.slices[t.route[q]].forward;
                println!("route {p:5} (slice {:5}) near route {q:5} (slice {:5}) d={:5.1} m  dot(fwd)={:5.2}  pos=({:.0},{:.0},{:.0})", t.route[p], t.route[q], (dx * dx + dz * dz).sqrt(), fa.dot(fb), a.x, a.y, a.z);
                last_report = p;
                break;
            }
        }
    }
}
