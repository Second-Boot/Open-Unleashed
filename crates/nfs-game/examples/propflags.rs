//! Diagnostic: the `Base` flags of a track's tagged articles, grouped — which carry the library bit
//! 0x8000 and which the byte-0 bit 0x40 the prop loader 0x4750B0 skips (0x47515A).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let mut by: BTreeMap<u32, usize> = BTreeMap::new();
    for art in t.crp.articles(&t.crp_data).unwrap() {
        let Ok(g) = t.crp.article_geometry(&t.crp_data, &art) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.tag_u32() == 0 {
            continue;
        }
        *by.entry(b.flags).or_default() += 1;
    }
    for (f, n) in by {
        println!("flags {f:#010x} library {} skip40 {} ×{n}", f & 0x8000 != 0, f & 0x40 != 0);
    }
}
