//! Diagnostic: how each track's alpha textures spread their alpha — the texels at 0 (which the
//! device's alpha test `GREATER 0` drops, `RE/notes/thrash.md` §6b), at 255, and in between (the
//! ones a depth-writing blended draw lets hide what lies behind) — tallied by the render method of
//! the materials that name them, over the texels the textures cover.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let names: Vec<String> = std::env::args().skip(1).collect();
    let names = if names.is_empty() { TrackAssets::list(&paths) } else { names };
    for name in names {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let mats = t.materials().unwrap_or_default();
        // method -> (textures, texels at 0, at 255, between)
        let mut tally = std::collections::BTreeMap::<String, (usize, u64, u64, u64)>::new();
        let mut seen = std::collections::HashSet::new();
        for m in &mats {
            let Some(x) = t.textures.iter().find(|x| x.name == m.texture_name()) else { continue };
            if !x.has_alpha || !seen.insert((m.render_method.clone(), x.name.clone())) {
                continue;
            }
            let e = tally.entry(m.render_method.clone()).or_default();
            e.0 += 1;
            for c in x.mips[0].chunks_exact(4) {
                match c[3] {
                    0 => e.1 += 1,
                    255 => e.2 += 1,
                    _ => e.3 += 1,
                }
            }
        }
        println!("# {name}");
        for (m, (n, a0, a255, mid)) in tally {
            let all = (a0 + a255 + mid).max(1) as f64;
            println!("  {m:<16} textures {n:<4} alpha 0 {:5.1}%  255 {:5.1}%  between {:5.1}%", 100.0 * a0 as f64 / all, 100.0 * a255 as f64 / all, 100.0 * mid as f64 / all);
        }
    }
}
