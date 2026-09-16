//! Diagnostic: main-lap route summary for every track.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in TrackAssets::list(&paths) {
        let Ok(track) = TrackAssets::load(&paths, &name) else { println!("{name}: load failed"); continue };
        let t = match SliceTable::from_crp(&track.crp, &track.crp_data) { Ok(t) => t, Err(e) => { println!("{name}: {e}"); continue } };
        let n = t.lap_len;
        let mut max_gap = 0.0f32;
        let mut worst = 0;
        for p in 0..n - 1 {
            let g = (t.slices[t.route[p]].centre - t.slices[t.route[p + 1]].centre).length();
            if g > max_gap { max_gap = g; worst = p; }
        }
        let close = (t.slices[t.route[n - 1]].centre - t.slices[t.route[0]].centre).length();
        let chain: Vec<usize> = t.route.iter().map(|&s| t.sections.iter().position(|sec| s >= sec.first && s <= sec.last).unwrap()).fold(Vec::new(), |mut v, sec| { if v.last() != Some(&sec) { v.push(sec); } v });
        println!("{name:10} slices={:5} sections={} lap={n:5} chain={chain:?} max_gap={max_gap:6.1} at {worst} closure={close:7.1}", t.slices.len(), t.sections.len());
    }
}
