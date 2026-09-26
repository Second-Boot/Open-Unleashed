//! Diagnostic: the articles that carry kind-3 `ef` parts (particle emitters) on a track, by
//! drawer class — `tr` (instance), `Anim` / `Anqt` (animated), library — for the question which of
//! them the instance drawer's animated branch 0x45023D…0x4502C9 reaches (`RE/RENDERING.md`).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for track in std::env::args().skip(1) {
        let t = TrackAssets::load(&paths, &track).unwrap();
        let mut rows = Vec::new();
        for (i, art) in t.crp.articles(&t.crp_data).unwrap().iter().enumerate() {
            let n = art
                .parts
                .iter()
                .filter(|p| p.is_part(b"ef"))
                .filter(|p| t.crp.entry_data(&t.crp_data, p).is_ok_and(|d| d.len() >= 4 && u32::from_le_bytes([d[0], d[1], d[2], d[3]]) == 3))
                .count();
            if n == 0 {
                continue;
            }
            let g = t.crp.article_geometry(&t.crp_data, art).ok();
            let lib = g.as_ref().and_then(|g| g.base.as_ref()).is_some_and(|b| b.is_library());
            let anim = art.level(b"Anim").is_some();
            let anqt = art.level(b"Anqt").is_some();
            let tr = g.as_ref().is_some_and(|g| g.transform.is_some());
            let tag = g.as_ref().and_then(|g| g.base.as_ref()).map(|b| b.tag).unwrap_or([0; 4]);
            let def = t.animdefs.by_fourcc(u32::from_le_bytes(tag)).map(|d| format!("{} {}", d.tag, d.collide_type)).unwrap_or_default();
            rows.push(format!("  #{i} {} ef3={n} tr={tr} Anim={anim} Anqt={anqt} lib={lib} tag={:?} def={def}", g.map(|g| g.name).unwrap_or_default(), String::from_utf8_lossy(&tag)));
        }
        println!("{track}: {} articles", rows.len());
        for r in rows {
            println!("{r}");
        }
    }
}
