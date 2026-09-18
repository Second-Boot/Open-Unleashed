//! Which sample codecs the shipped banks actually use (`RE/notes/audio.md` §2).
use nfs_assets::GamePaths;
use nfs_formats::{bigf::Bigf, bnk::Bnk};
use std::collections::BTreeMap;

fn scan(name: &str, data: &[u8], hist: &mut BTreeMap<String, usize>, four: &mut Vec<String>) {
    if data.len() < 4 {
        return;
    }
    if &data[0..4] == b"BNKl" {
        let Ok(b) = Bnk::parse(data) else { return };
        for s in b.slots.iter().flatten() {
            let c = format!("{:?}", s.header.codec());
            *hist.entry(c.clone()).or_default() += 1;
            if c.contains("Four") || c.contains("Unknown") {
                four.push(name.to_string());
            }
        }
    } else if data.starts_with(b"BIGF") || data.starts_with(b"BIG4") {
        let Ok(dir) = Bigf::parse(data) else { return };
        for e in &dir.entries {
            let (o, n) = (e.offset as usize, e.size as usize);
            if o + n <= data.len() {
                scan(&e.name, &data[o..o + n], hist, four);
            }
        }
    }
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut hist = BTreeMap::new();
    let mut four = Vec::new();
    for dir in [paths.game_data().join("Sounds"), paths.game_data().join("Speech")] {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(d) = std::fs::read(e.path()) else { continue };
            scan(&e.file_name().to_string_lossy(), &d, &mut hist, &mut four);
        }
    }
    for (k, v) in &hist {
        println!("{k:<12} × {v}");
    }
    four.sort();
    four.dedup();
    println!("banks with an undecoded codec: {four:?}");
}
