//! Diagnostic: the raw `pr` records of one article, streams and all — for the type-1 question
//! (`RE/notes/crp-geometry.md` §4).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    for art in t.crp.articles(&t.crp_data).unwrap() {
        let Ok(g) = t.crp.article_geometry(&t.crp_data, &art) else { continue };
        if !g.name.to_lowercase().contains(&a[1].to_lowercase()) {
            continue;
        }
        println!("== {}", g.name);
        for part in &art.parts {
            if !part.is_part(b"pr") {
                continue;
            }
            let Ok(bytes) = t.crp.entry_data(&t.crp_data, part) else { continue };
            let Ok(rec) = nfs_formats::crp::PrimitiveRecord::parse(bytes, part.data_offset()) else { continue };
            println!(
                "  pr#{:<3} type {} flags {:#06x} mat {:>3} idx_arrays {} index_count {}",
                part.part_index(),
                rec.prim_type,
                rec.flags,
                rec.material,
                rec.index_arrays.len(),
                rec.index_count
            );
            println!("      index arrays: {:?}", rec.index_arrays.iter().map(|(t, o)| format!("{}{}@{o}", t[0] as char, t[1] as char)).collect::<Vec<_>>());
            for s in &rec.streams {
                println!("      stream slot {:>10} off {:>6} len {:>5} kind {:#06x} lod {} fmt {:#06x}", s.slot as i32, s.byte_offset, s.byte_len, s.kind, s.lod, s.fmt);
            }
        }
    }
}
