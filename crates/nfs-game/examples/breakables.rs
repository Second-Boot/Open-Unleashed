//! Diagnostic: the track articles whose `animdefs.txt` definition has pieces (0x476380 would start
//! them on a hit), with each article's name, flags, `Anim` words and the first article whose tag
//! is each piece's `BaseGroup` / `QuatGroup` (0x478640 searches the instances by tag).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let names: Vec<(String, u32, Option<[u32; 2]>)> = arts
        .iter()
        .map(|art| {
            let g = t.crp.article_geometry(&t.crp_data, art).ok();
            let n = g.as_ref().map(|g| g.name.clone()).unwrap_or_default();
            let tag = g.as_ref().and_then(|g| g.base.as_ref()).map(|b| b.tag_u32()).unwrap_or(0);
            let anim = art.level(b"Anim").and_then(|e| t.crp.entry_data(&t.crp_data, e).ok()).filter(|d| d.len() >= 8).map(|d| {
                [u32::from_le_bytes(d[0..4].try_into().unwrap()), u32::from_le_bytes(d[4..8].try_into().unwrap())]
            });
            (n, tag, anim)
        })
        .collect();
    if a.len() > 1 {
        // one article in detail: its `tr`, vertex box and the first / last Anqt frames
        for (i, art) in arts.iter().enumerate() {
            if !names[i].0.eq_ignore_ascii_case(&a[1]) {
                continue;
            }
            let g = t.crp.article_geometry(&t.crp_data, art).unwrap();
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &g.primitives {
                for v in &p.positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(v[k]);
                        hi[k] = hi[k].max(v[k]);
                    }
                }
            }
            let parts: Vec<String> = art.parts.iter().map(|e| e.tag_str()).collect();
            println!("  parts {parts:?}");
            println!("#{i} {} flags {:#x} tr {:?}\n  box {lo:?} {hi:?}", g.name, g.base.as_ref().map(|b| b.flags).unwrap_or(0), g.transform);
            if let Some(tr) = t.crp.article_animation(&t.crp_data, art) {
                for k in [0, 1, tr.frames.len() / 2, tr.frames.len() - 1] {
                    println!("  frame {k}: {:?}", tr.frames[k]);
                }
            }
        }
        return;
    }
    for (i, art) in arts.iter().enumerate() {
        let Ok(g) = t.crp.article_geometry(&t.crp_data, art) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        let Some(d) = t.animdefs.by_fourcc(b.tag_u32()) else { continue };
        if d.pieces.is_empty() || b.flags & 0x40 != 0 {
            continue;
        }
        println!("#{i} {} tag {} flags {:#x} anim {:?} tr {:?}", g.name, String::from_utf8_lossy(&b.tag), b.flags, names[i].2, g.transform.map(|m| [m[12], m[13], m[14]]));
        for (speed, p) in &d.pieces {
            let find = |n: &Option<String>| n.as_ref().map(|n| names.iter().position(|x| x.1 == u32::from_le_bytes(n.as_bytes()[..4].try_into().unwrap())).map(|k| (k, names[k].0.clone(), names[k].2)));
            println!("   piece {:?}: base {:?} {:?}  quat {:?} {:?}", speed, p.base_group, find(&p.base_group), p.quat_group, find(&p.quat_group));
        }
    }
}
