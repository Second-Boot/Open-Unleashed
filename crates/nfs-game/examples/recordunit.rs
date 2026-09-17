//! Diagnostic: dump the default track-record table so the spacing shows the unit.
use nfs_assets::GamePaths;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    for t in &tables.tracks {
        let times: Vec<u32> = t.records.iter().map(|r| r.time).collect();
        println!("{:<10} {:?}", t.file, times);
        println!("           ticks {:?}", times.iter().map(|v| format!("{}:{:05.2}", v / 3840, (v % 3840) as f32 / 64.0)).collect::<Vec<_>>());
        println!("           csecs {:?}", times.iter().map(|v| format!("{}:{:05.2}", v / 6000, (v % 6000) as f32 / 100.0)).collect::<Vec<_>>());
    }
}
