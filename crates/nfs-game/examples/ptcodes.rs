//! Diagnostic: which values the shipped banks give the PT patch codes (< 0x80), whose meaning lives
//! inside the EA sound library (`RE/notes/audio.md` §2).
use nfs_assets::GamePaths;
use nfs_formats::{bigf::Bigf, bnk::Bnk};
use std::collections::BTreeMap;

fn scan(name: &str, data: &[u8], hist: &mut BTreeMap<u8, BTreeMap<u32, usize>>, banks: &mut usize) {
    // per-bank summary for the volume/pan codes

    if data.len() < 4 {
        return;
    }
    if &data[0..4] == b"BNKl" {
        let Ok(b) = Bnk::parse(data) else { return };
        *banks += 1;
        let mut vols: Vec<u32> = Vec::new();
        let mut used = 0;
        for s in b.slots.iter().flatten() {
            used += 1;
            for (code, v) in &s.header.patch_params {
                *hist.entry(*code).or_default().entry(*v).or_default() += 1;
                if *code == 0x0E {
                    vols.push(*v);
                }
            }
        }
        if std::env::var_os("PER_BANK").is_some() {
            vols.sort_unstable();
            println!("    {name:<24} {used:>4} slots, code 0x0E on {:>4}: {:?}", vols.len(), vols.iter().step_by(vols.len().max(1) / 4 + 1).collect::<Vec<_>>());
        }
    } else if data.starts_with(b"BIGF") || data.starts_with(b"BIG4") {
        let Ok(dir) = Bigf::parse(data) else { return };
        for e in &dir.entries {
            let (o, n) = (e.offset as usize, e.size as usize);
            if o + n <= data.len() {
                scan(&e.name, &data[o..o + n], hist, banks);
            }
        }
    } else {
        let _ = name;
    }
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut hist: BTreeMap<u8, BTreeMap<u32, usize>> = BTreeMap::new();
    let mut banks = 0;
    for sub in ["Sounds", "Speech", "Music"] {
        let dir = paths.game_data().join(sub);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut paths_: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths_.sort();
        for p in paths_ {
            let Ok(d) = std::fs::read(&p) else { continue };
            scan(&p.file_name().unwrap().to_string_lossy(), &d, &mut hist, &mut banks);
        }
    }
    println!("{banks} banks");
    for (code, vals) in &hist {
        let mut v: Vec<_> = vals.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let total: usize = vals.values().sum();
        println!("  code {code:#04x} ×{total:<6} {:?}", v.iter().take(8).map(|(val, c)| format!("{val}×{c}")).collect::<Vec<_>>());
    }
}
