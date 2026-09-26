//! Diagnostic: the library articles (`Base & 0x8000`, the `.scn` GEOM templates of `WSceneBundle`
//! 0x482970) of every track — their `tr`, their `ef` parts by kind — and the GEOM scales the
//! track's `.scn` files place them with.
//!
//! `cargo run -p nfs-game --example libsurvey [track…]`
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::scn::{ElementKind, Scene};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tracks = if args.is_empty() { TrackAssets::list(&paths) } else { args };
    let (mut total, mut with_tr, mut with_ef) = (0, 0, 0);
    for name in &tracks {
        let Ok(t) = TrackAssets::load(&paths, name) else { continue };
        let Ok(arts) = t.crp.articles(&t.crp_data) else { continue };
        let mut lib = Vec::new();
        for a in &arts {
            let Ok(g) = t.crp.article_geometry(&t.crp_data, a) else { continue };
            if !g.base.as_ref().is_some_and(|b| b.is_library()) {
                continue;
            }
            let mut kinds = Vec::new();
            for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
                if let Ok(d) = t.crp.entry_data(&t.crp_data, p)
                    && d.len() >= 4
                {
                    kinds.push((u32::from_le_bytes([d[0], d[1], d[2], d[3]]), p.part_index()));
                }
            }
            // the LOD records `Base+0x64` (12 bytes: byte +0, byte +1, part range +4…+6, LOD +8,
            // distance +0xA) of an `ef`-bearing article
            if !kinds.is_empty()
                && let Some(b) = a.level(b"Base").and_then(|e| t.crp.entry_data(&t.crp_data, e).ok())
            {
                let n = g.base.as_ref().map_or(0, |b| b.lod_count as usize);
                for i in 0..n {
                    let o = 100 + 12 * i;
                    if o + 12 > b.len() {
                        break;
                    }
                    let w = |k: usize| u16::from_le_bytes([b[o + k], b[o + k + 1]]);
                    println!("    {} LOD {i}: pr {} ef {} parts {}..={} lod {} dist {}", g.name, b[o], b[o + 1], w(4), w(6), w(8), w(10));
                }
            }
            let tr = g.transform.map(|m| {
                let id = (0..4).all(|r| (0..4).all(|c| (m[r * 4 + c] - if r == c { 1.0 } else { 0.0 }).abs() < 1e-6));
                (id, [m[0], m[5], m[10], m[12], m[13], m[14]])
            });
            total += 1;
            if tr.is_some_and(|(id, _)| !id) {
                with_tr += 1;
            }
            if !kinds.is_empty() {
                with_ef += 1;
            }
            let tag = g.base.as_ref().map(|b| String::from_utf8_lossy(&b.tag).into_owned()).unwrap_or_default();
            lib.push((tag, g.name.clone(), tr, kinds));
        }
        println!("{name}: {} library articles", lib.len());
        for (tag, n, tr, kinds) in &lib {
            if tr.is_some_and(|(id, _)| !id) || !kinds.is_empty() {
                println!("  {tag:>4} {n:<24} tr {:?} ef {kinds:?}", tr.map(|t| t.1));
            }
        }
        // the GEOM scales the track's scenes use
        let mut scales = std::collections::BTreeMap::<String, usize>::new();
        if let Ok(rd) = std::fs::read_dir(paths.track_dir()) {
            for e in rd.flatten() {
                let f = e.file_name().to_string_lossy().to_lowercase();
                if !f.starts_with(&format!("{}_", name.to_lowercase())) || !f.ends_with(".scn") {
                    continue;
                }
                let Ok(txt) = std::fs::read(e.path()) else { continue };
                let Ok(s) = Scene::parse(&String::from_utf8_lossy(&txt)) else { continue };
                let mut with_ef = std::collections::BTreeMap::<String, usize>::new();
                for g in s.of_kind(&ElementKind::Geom) {
                    let sc = nfs_assets::track::geom_scale(g);
                    *scales.entry(format!("{:?}", sc.to_array())).or_default() += 1;
                    let tag = String::from_utf8_lossy(&Scene::param_u32(g, 0, 1).unwrap_or(0).to_le_bytes()).into_owned();
                    if lib.iter().any(|(t, _, _, k)| *t == tag && !k.is_empty()) {
                        *with_ef.entry(tag).or_default() += 1;
                    }
                }
                if !with_ef.is_empty() {
                    println!("  {f}: GEOMs of ef-bearing articles {with_ef:?}");
                }
            }
        }
        println!("  GEOM scales: {scales:?}");
    }
    println!("total library articles {total}, non-identity tr {with_tr}, with ef parts {with_ef}");
}
