//! Diagnostic: which material flag bits the track materials use, and which texture names carry the
//! wall class in bits 4..7 (`RE/notes/suspension-collision.md` §7).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = ["coastal", "alps", "castle", "AutoBahn", "farmland", "foothills", "Forest", "industrial", "canyon", "monaco1"].iter().map(|s| s.to_string()).collect();
    }
    let mut by_class: BTreeMap<u16, BTreeMap<String, usize>> = BTreeMap::new();
    for t in &tracks {
        let Ok(a) = TrackAssets::load(&paths, t) else { continue };
        let Ok(mats) = a.materials() else { continue };
        let mut bits = [0usize; 16];
        for m in &mats {
            for b in 0..16 {
                if m.flags & (1 << b) != 0 {
                    bits[b] += 1;
                }
            }
            let class = (m.flags & 0xF0) as u16;
            if class != 0 {
                let name: String = m.texture.iter().map(|&c| if c.is_ascii_graphic() { c as char } else { '?' }).collect();
                *by_class.entry(class).or_default().entry(name).or_default() += 1;
            }
        }
        println!("{t}: {} materials, bit counts {bits:?}", mats.len());
        // do bits 14 and 15 ever disagree?
        let only14 = mats.iter().filter(|m| m.flags & 0x4000 != 0 && m.flags & 0x8000 == 0).count();
        let only15 = mats.iter().filter(|m| m.flags & 0x8000 != 0 && m.flags & 0x4000 == 0).count();
        if only14 + only15 > 0 {
            println!("   bit 14 alone on {only14}, bit 15 alone on {only15}");
        }
        // are `+4`, `+32` anything but a restatement of the method's name?
        if std::env::var_os("FIELDS").is_some() {
            let mut by_method: BTreeMap<String, BTreeMap<(u32, u32), usize>> = BTreeMap::new();
            for m in &mats {
                *by_method.entry(m.render_method.clone()).or_default().entry((m.class, m.unknown_20)).or_default() += 1;
            }
            for (k, v) in by_method {
                println!("   {k:<18} (class, +32): {v:?}");
            }
        }
        // which render methods carry the bits nobody has named yet
        if std::env::var_os("BITS").is_some() {
            let mut by_bit: BTreeMap<u32, BTreeMap<String, usize>> = BTreeMap::new();
            for m in &mats {
                for b in [0u32, 1, 2, 3, 14, 15] {
                    if m.flags & (1 << b) != 0 {
                        *by_bit.entry(b).or_default().entry(format!("{} {:#x}", m.render_method, m.class)).or_default() += 1;
                    }
                }
            }
            for (b, v) in by_bit {
                println!("   bit {b}: {v:?}");
            }
        }
    }
    println!("\nwall class (flags & 0xF0) → texture-name prefixes:");
    for (c, names) in &by_class {
        let mut v: Vec<_> = names.iter().collect();
        v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        println!("  0x{c:02X}: {:?}", v.iter().take(14).map(|(s, n)| format!("{s}×{n}")).collect::<Vec<_>>());
    }
}
