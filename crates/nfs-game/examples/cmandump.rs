//! Diagnostic: a track's five `CmAn` camera animations (`RE/notes/race-logic.md` §13) — the
//! header words 0x471850 / 0x471A50 read (`+0x24`, key count `+0x28`) and the 0x34-byte keys from
//! `+0x2C` that 0x470F70 interpolates.
use nfs_assets::{track::TrackAssets, GamePaths};

fn f(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "alps".into());
    let all = std::env::args().nth(2).is_some();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &name).unwrap();
    for e in t.crp.level.iter().filter(|e| e.is_level(b"CmAn")) {
        let Ok(b) = t.crp.entry_data(&t.crp_data, e) else { continue };
        let hdr: Vec<String> = (0..11).map(|i| format!("{:08x}", u(b, i * 4))).collect();
        let n = u(b, 0x28) as usize;
        println!("CmAn entry {} len {} hdr {} keys {n}", e.entry_index(), b.len(), hdr.join(" "));
        for k in 0..n {
            let o = 0x2C + k * 0x34;
            if o + 0x34 > b.len() {
                println!("  key {k} past the entry");
                break;
            }
            if !all && k > 3 && k + 3 < n {
                continue;
            }
            let v: Vec<String> = (0..13).map(|i| format!("{:9.3}", f(b, o + i * 4))).collect();
            println!("  {k:3} {}", v.join(" "));
        }
    }
}
