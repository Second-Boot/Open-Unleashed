//! Diagnostic: the track articles 0x4772F0 turns into smackables — every article of the
//! instance bundle (one with a `tr`, 0x47859E) whose `Base` byte 0 lacks 0x40 and whose
//! `animdefs.txt` definition (0x47ED90) says `kSmackable` — by tag, with how many are library
//! articles (`Base` flag 0x8000) and how many carry kind-3 `ef` parts. 0x4888B0 collects the
//! scene bundle's and then these into `[WTrack+0x34]`, at most 125 (0x488926), and 0x4A17F0 makes
//! a car record of each (`suspension-collision.md` §9, "The track's smackables").
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    let only: Vec<String> = std::env::args().skip(1).collect();
    for t in &tables.tracks {
        if !only.is_empty() && !only.contains(&t.file) {
            continue;
        }
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        // tag -> (placed, library, with kind-3 ef)
        let mut by: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
        let (mut placed, mut library) = (0, 0);
        for art in a.crp.articles(&a.crp_data).unwrap_or_default() {
            let Ok(g) = a.crp.article_geometry(&a.crp_data, &art) else { continue };
            let Some(b) = g.base.as_ref() else { continue };
            if g.transform.is_none() || b.flags & 0x40 != 0 {
                continue;
            }
            let Some(d) = a.animdefs.lookup(b.tag_u32()) else { continue };
            if d.collide_type != "kSmackable" {
                continue;
            }
            let ef3 = art
                .parts
                .iter()
                .filter(|p| p.is_part(b"ef"))
                .any(|p| a.crp.entry_data(&a.crp_data, p).is_ok_and(|d| d.len() >= 4 && u32::from_le_bytes([d[0], d[1], d[2], d[3]]) == 3));
            let e = by.entry(d.tag.clone()).or_default();
            if b.is_library() {
                e.1 += 1;
                library += 1;
            } else {
                e.0 += 1;
                placed += 1;
            }
            if ef3 && !b.is_library() {
                e.2 += 1;
            }
        }
        let summary: Vec<String> = by.iter().filter(|(_, (n, _, _))| *n > 0).map(|(tag, (n, _, e))| format!("{tag}:{n}{}", if *e > 0 { format!(" (ef3 {e})") } else { String::new() })).collect();
        println!("{:<11} placed {placed} (library {library}): {}", t.file, summary.join(" "));
        if !only.is_empty() {
            // the order `TrackWorld::track_smackable(k)` / `NFS_SMACK_DRIVE=k` count in
            let arts = a.crp.articles(&a.crp_data).unwrap_or_default();
            for (k, i) in nfs_game::world::track_smackable_articles(&a).into_iter().enumerate() {
                let g = a.crp.article_geometry(&a.crp_data, &arts[i]).ok();
                let tag = g.as_ref().and_then(|g| g.base.as_ref()).map(|b| String::from_utf8_lossy(&b.tag).to_string()).unwrap_or_default();
                let tr = g.as_ref().and_then(|g| g.transform).map(|m| glam::Mat4::from_cols_array(&m)).unwrap_or(glam::Mat4::IDENTITY);
                let m3 = glam::Mat3::from_mat4(tr);
                let lens = [m3.x_axis.length(), m3.y_axis.length(), m3.z_axis.length()];
                println!("  k={k} article #{i} {tag} {} det {:.3} axes {lens:.3?} pos {:.1?}", g.map(|g| g.name).unwrap_or_default(), m3.determinant(), tr.w_axis.truncate());
            }
        }
    }
}
