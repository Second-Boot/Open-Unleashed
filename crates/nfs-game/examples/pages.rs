//! Diagnostic: which side of the car each texture page belongs to. The damage decals are keyed by
//! page (`RE/notes/car-model.md` §11), so this settles the left/right half of the zone ring.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let name = a.first().cloned().unwrap_or_else(|| "993".into());
    let c = CarAssets::load(&paths, &name).unwrap();
    let mats = c.crp.materials(&c.crp_data).unwrap();
    // model space: +X is the car's left (`car-model.md` §3 — the model frame mirrors the world in X and Z)
    let mut sum = [(0.0f64, 0usize); 8];
    // count only vertices clearly off the centreline, where the side is unambiguous
    let mut sides = [(0usize, 0usize); 8];
    for art in c.crp.articles(&c.crp_data).unwrap() {
        let Ok(g) = c.crp.article_geometry(&c.crp_data, &art) else { continue };
        for p in g.primitives.iter().filter(|p| p.lod == 1) {
            let Some(page) = mats.get(p.material as usize).map(|m| m.page_index() as usize) else { continue };
            if page >= sum.len() {
                continue;
            }
            for v in &p.positions {
                sum[page].0 += v[0] as f64;
                sum[page].1 += 1;
                if v[0] > 0.3 {
                    sides[page].0 += 1;
                } else if v[0] < -0.3 {
                    sides[page].1 += 1;
                }
            }
        }
    }
    println!("{name}: mean model x per page (+x = the car's left)");
    for (i, (s, n)) in sum.iter().enumerate() {
        if *n > 0 {
            let (l, r) = sides[i];
            println!("  page {i}: mean {:>7.3} over {n} vertices; off-centre {l} left / {r} right → {}", s / *n as f64, if l > r { "LEFT" } else { "right" });
        }
    }
}
