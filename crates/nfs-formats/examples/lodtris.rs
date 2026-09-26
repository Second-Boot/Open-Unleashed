//! Diagnostic: a car model's vertex count per LOD and per `Base+78` pass bit (`RE/notes/car-model.md`,
//! audit 2026-09-26: the LOD pick 0x439F90 and the passes of 0x444E00), and every article that has
//! primitives at a LOD without a `tr` part for that LOD while it has one for another — 0x43C8F8
//! looks up `"tr"` + the LOD with no fallback and draws such a part in the car's own frame.
//! Usage: `lodtris <crp>…` (the vertex table for the first file only).
use nfs_formats::refpack;
fn main() {
    for (k, p) in std::env::args().skip(1).enumerate() {
        let raw = std::fs::read(&p).unwrap();
        let data = refpack::maybe_decompress(&raw).unwrap();
        let crp = nfs_formats::crp::Crp::parse(&data).unwrap();
        let mut t: std::collections::BTreeMap<(u8, u8), usize> = Default::default();
        for a in crp.articles(&data).unwrap_or_default() {
            let Ok(g) = crp.article_geometry(&data, &a) else { continue };
            let Some(b) = &g.base else { continue };
            let lods: std::collections::BTreeSet<u8> = g.primitives.iter().map(|p| p.lod).collect();
            for l in &lods {
                if !g.transforms.is_empty() && !g.transforms.iter().any(|(i, _)| i == l) {
                    let c = |lod: u8| {
                        let v: Vec<[f32; 3]> = g.primitives.iter().filter(|q| q.lod == lod).flat_map(|q| q.positions.iter().copied()).collect();
                        let n = v.len().max(1) as f32;
                        [0, 1, 2].map(|k| v.iter().map(|q| q[k]).sum::<f32>() / n)
                    };
                    let t0 = g.transforms[0];
                    println!("{p}: {} lod {l} has no tr (has {:?}); centre {:?}, lod {} centre {:?} + t {:?}", g.name, g.transforms.iter().map(|t| t.0).collect::<Vec<_>>(), c(*l), t0.0, c(t0.0), &t0.1[12..15]);
                }
            }
            for pr in &g.primitives {
                *t.entry((pr.lod, b.draw_bytes[0] & 0xF0)).or_default() += pr.positions.len();
            }
        }
        if k == 0 {
            for ((lod, m), n) in t {
                println!("lod {lod} mask {m:02x}: {n} verts");
            }
        }
    }
}
