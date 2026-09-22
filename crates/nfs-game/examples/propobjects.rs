//! Diagnostic: the track articles the loader 0x4750B0 turns into collision objects — every article
//! whose `Base` byte 0 lacks 0x40 and whose `Base+0x44` tag is set, with the `animdefs.txt`
//! definition 0x47ED90 finds for it (an unknown tag gets the **first** definition, `ENDW`, since
//! the search falls back to `vector[0]` and 0x47DCF0 appends in file order), grouped by the
//! definition's `collideType` (`suspension-collision.md` §9).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    for t in &tables.tracks {
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        let mut by: BTreeMap<(String, String), usize> = BTreeMap::new();
        for art in a.crp.articles(&a.crp_data).unwrap_or_default() {
            let Ok(g) = a.crp.article_geometry(&a.crp_data, &art) else { continue };
            let Some(b) = g.base.as_ref() else { continue };
            if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
                continue;
            }
            let tag: String = b.tag.iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            let kind = match a.animdefs.by_fourcc(b.tag_u32()) {
                Some(d) => d.collide_type.clone(),
                None => "ENDW-fallback (kBox)".to_string(),
            };
            *by.entry((kind, tag)).or_default() += 1;
        }
        let summary: Vec<String> = by.iter().map(|((k, tag), n)| format!("{tag}:{k}×{n}")).collect();
        println!("{:<11} {}", t.file, summary.join(" "));
    }
}
