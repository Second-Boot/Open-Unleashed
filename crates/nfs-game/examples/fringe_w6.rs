//! Diagnostic (`RE/notes/thrash.md` §6e): the `pr` record's word `+6` per part, by
//! (word, flags) — the static drawer 0x450DE0 puts `[pr+4] & 0xFFFF0000` (word `+6` << 16) on top of
//! `round(d²)` in the view-list key (0x450F37…0x450F44). `V=1` lists the parts whose word is not 0x8000.
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    for t in std::env::args().skip(1) {
        let a = TrackAssets::load(&paths, &t).unwrap();
        let arts = a.crp.articles(&a.crp_data).unwrap();
        let mats = a.materials().unwrap();
        let mut per: BTreeMap<(u16, u16), usize> = BTreeMap::new();
        let mut by_mat: BTreeMap<(u16, u16), usize> = BTreeMap::new();
        for art in &arts {
            for pr in art.parts.iter().filter(|p| p.is_part(b"pr")) {
                let b = a.crp.entry_data(&a.crp_data, pr).unwrap();
                let w6 = u16::from_le_bytes([b[6], b[7]]);
                let fl = u16::from_le_bytes([b[2], b[3]]);
                *per.entry((w6, fl)).or_default() += 1;
                // with `F=1`: every flag-2 part of a `FakeSort` material too (the fold 0x449AB2 reads their key)
                let fake = mats.get(u16::from_le_bytes([b[4], b[5]]) as usize).is_some_and(|m| m.render_method == "FakeSort");
                if w6 != 0x8000 || (fake && fl & 2 != 0 && std::env::var_os("F").is_some()) {
                    *by_mat.entry((w6, u16::from_le_bytes([b[4], b[5]]))).or_default() += 1;
                }
                if w6 != 0x8000 && std::env::var_os("V").is_some() {
                    let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                    println!("  w6 {w6:#06x} flags {fl:#06x} mat {} centre [{}, {}, {}]", u16::from_le_bytes([b[4], b[5]]), f(8), f(12), f(16));
                }
            }
        }
        println!("{t}: (word6, flags) -> {per:x?}");
        for ((w6, m), n) in by_mat {
            let mt = mats.get(m as usize);
            let (rm, tx) = mt.map(|x| (x.render_method.clone(), x.texture_name().to_string())).unwrap_or_default();
            println!("  word6 {w6:#06x} material {m} ({rm}, {tx}) x{n}");
        }
    }
}
