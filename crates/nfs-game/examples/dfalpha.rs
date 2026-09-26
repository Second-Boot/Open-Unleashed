//! Diagnostic: the `df` stream's alpha (the colour's high byte) against the material's `Rs`
//! blending and its texture's alpha. Stage 0 runs `ALPHAOP = MODULATE` (`RE/notes/thrash.md` §5,
//! value 8), so with blending on a part is as translucent as texel alpha × vertex alpha, and the
//! alpha test `GREATER 10` drops a corner-weighted fragment whatever the blend (§6d.4).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
        tracks = tables.tracks.iter().map(|t| t.file.clone()).collect();
    }
    // (blend on, texture has alpha) → (parts, parts with some corner alpha < 255, parts with some ≤ 10)
    let mut all: BTreeMap<(bool, bool), [usize; 3]> = BTreeMap::new();
    let mut names: BTreeMap<(bool, bool), BTreeMap<String, usize>> = BTreeMap::new();
    for t in &tracks {
        let Ok(a) = TrackAssets::load(&paths, t) else { continue };
        let Ok(mats) = a.materials() else { continue };
        let methods = a.crp.render_methods(&a.crp_data).unwrap_or_default();
        let alpha: BTreeMap<String, bool> = a.textures.iter().map(|x| (x.name.clone(), x.has_alpha)).collect();
        let Ok(arts) = a.crp.articles(&a.crp_data) else { continue };
        for art in &arts {
            let Ok(g) = a.crp.article_geometry(&a.crp_data, art) else { continue };
            for p in &g.primitives {
                let Some(m) = mats.get(p.material as usize) else { continue };
                let blend = nfs_assets::track::render_states(&methods, m).is_some_and(|rs| rs[1] == 2);
                let ta = alpha.get(&m.texture_name()).copied().unwrap_or(false);
                let e = all.entry((blend, ta)).or_default();
                e[0] += 1;
                let min = p.colors.iter().map(|c| (c >> 24) as u8).min().unwrap_or(255);
                if min < 255 {
                    e[1] += 1;
                    *names.entry((blend, ta)).or_default().entry(format!("{t}:{} {}", m.render_method, m.texture_name())).or_default() += 1;
                }
                if min <= 10 {
                    e[2] += 1;
                }
            }
        }
    }
    for ((b, ta), n) in &all {
        println!("blend {b} texture alpha {ta}: parts {}, with vertex alpha < 255 {}, with ≤ 10 {}", n[0], n[1], n[2]);
        if let Some(v) = names.get(&(*b, *ta)) {
            let mut v: Vec<_> = v.iter().collect();
            v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            println!("   {:?}", v.iter().take(12).collect::<Vec<_>>());
        }
    }
}
