//! Diagnostic: the distinct `.edg` wall words per track, to see whether any land inside
//! [0x10, 0x60] without being a multiple of 0x10 (those would take the 0x1F "unknown" sound path).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    let mut odd = 0usize;
    for t in &tables.tracks {
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        let Some(e) = a.edges.as_ref() else { continue };
        let mut hist: BTreeMap<u32, usize> = BTreeMap::new();
        for s in &e.segments {
            *hist.entry(if (0x10..=0x60).contains(&(s.tag & 0xFF)) { s.tag & 0xFF } else { u32::MAX }).or_default() += 1;
        }
        let in_range_odd: usize = hist.iter().filter(|(k, _)| **k != u32::MAX && **k % 0x10 != 0).map(|(_, n)| *n).sum();
        odd += in_range_odd;
        println!("{:<10} {:?}  odd-in-range {in_range_odd}", t.file, hist.iter().map(|(k, n)| if *k == u32::MAX { format!("out×{n}") } else { format!("0x{k:02X}×{n}") }).collect::<Vec<_>>());
    }
    println!("total words inside [0x10,0x60] that are not a multiple of 0x10: {odd}");
}
