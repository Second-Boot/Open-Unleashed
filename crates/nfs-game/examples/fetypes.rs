//! Diagnostic: the `fe` parts of every track's articles by their first dword — the kind 0x450D10
//! switches on (2, 3 = an effect 0x455150, 4 = a car shadow projector 0x456090).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    for t in &tables.tracks {
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        let mut by: BTreeMap<u32, usize> = BTreeMap::new();
        for art in a.crp.articles(&a.crp_data).unwrap_or_default() {
            for p in art.parts.iter().filter(|p| p.is_part(b"ef")) {
                let Ok(d) = a.crp.entry_data(&a.crp_data, p) else { continue };
                if d.len() >= 4 {
                    *by.entry(u32::from_le_bytes([d[0], d[1], d[2], d[3]])).or_default() += 1;
                }
            }
        }
        println!("{:<11} {:?}", t.file, by);
    }
}
