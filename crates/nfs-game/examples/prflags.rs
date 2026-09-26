//! Diagnostic: the `pr` records' flag word against their material's render method and `Rs` block
//! — the static drawer 0x450DE0 files a part into the view's lists by `flags` (0x450F30…0x450F9B:
//! bit 1 the ascending list `+4`, bit 0 the descending `+0xC`, bit 4 the descending `+8`, bit 2
//! `+0xC` only while `[0x6197E6]`; none of them, not drawn). `RE/RENDERING.md` "The view's lists".
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
        tracks = tables.tracks.iter().map(|t| t.file.clone()).collect();
    }
    let mut all: BTreeMap<(u16, String, Option<[i32; 6]>), usize> = BTreeMap::new();
    for t in &tracks {
        let Ok(a) = TrackAssets::load(&paths, t) else { continue };
        let Ok(mats) = a.materials() else { continue };
        let methods = a.crp.render_methods(&a.crp_data).unwrap_or_default();
        let Ok(arts) = a.crp.articles(&a.crp_data) else { continue };
        let mut per: BTreeMap<u16, usize> = BTreeMap::new();
        let (mut groups, mut multi, mut mixed) = (0usize, 0usize, 0usize);
        for art in &arts {
            let Ok(g) = a.crp.article_geometry(&a.crp_data, art) else { continue };
            // how many (LOD, material) groups hold more than one part, and how many of those mix lists
            let mut by: BTreeMap<(u8, u16), Vec<u16>> = BTreeMap::new();
            for p in &g.primitives {
                by.entry((p.lod, p.material)).or_default().push(p.flags & 0x17);
            }
            for v in by.values() {
                groups += 1;
                multi += (v.len() > 1) as usize;
                mixed += v.iter().any(|f| *f != v[0]) as usize;
            }
            for p in &g.primitives {
                *per.entry(p.flags).or_default() += 1;
                let m = mats.get(p.material as usize);
                let method = m.map(|m| m.render_method.clone()).unwrap_or_default();
                let rs = m.and_then(|m| nfs_assets::track::render_states(&methods, m));
                *all.entry((p.flags, method, rs)).or_default() += 1;
            }
        }
        println!("{t}: {per:x?}  (lod, material) groups {groups}, with several parts {multi}, mixing lists {mixed}");
    }
    println!("\n(flags, method, Rs) → parts over the tracks:");
    for ((f, m, rs), n) in &all {
        println!("  {f:#06x} {m:<16} {rs:?}  {n}");
    }
}
