//! Diagnostic: the op mask a track material hands its render method (`mt+0x0C`, the second
//! argument of the method builder 0x44BF70, passed at 0x44AB8B), and what its primitives' `df`
//! colours are. An op is built only when its mask (`op+0xC`) is 0 or shares a bit with it
//! (0x44C1B0…0x44C1BC) — so a material with bit 0x10 builds the method's `Am` op
//! (`LayerAmbient` 0x8007 by day, `Headlights::LayerVertexLight_Ambient` 0x8011 by night) and
//! not its `df` / `NV` ops (mask 8): its vertex colour is the `Am` block, not the stream.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let masks: Vec<(String, u32)> = crp
            .material_entries
            .iter()
            .filter(|e| e.is_part(b"mt"))
            .filter_map(|e| crp.entry_data(&data, e).ok())
            .map(|b| (b[16..32].iter().take_while(|c| **c != 0).map(|c| *c as char).collect(), u32::from_le_bytes([b[12], b[13], b[14], b[15]])))
            .collect();
        // (method, mask) -> (materials, corners, non-white corners, mean rgb)
        let mut tally = std::collections::BTreeMap::<(String, u32), (usize, usize, usize, [f64; 3])>::new();
        for (m, k) in &masks {
            tally.entry((m.clone(), *k)).or_default().0 += 1;
        }
        let Ok(arts) = crp.articles(&data) else { continue };
        let mut odd = std::collections::BTreeMap::<(u32, String), usize>::new();
        for art in &arts {
            let Ok(g) = crp.article_geometry(&data, art) else { continue };
            for pr in g.primitives.iter().filter(|p| p.lod == 0) {
                let Some((m, k)) = masks.get(pr.material as usize) else { continue };
                let t = tally.entry((m.clone(), *k)).or_default();
                for c in &pr.colors {
                    let b = c.to_le_bytes();
                    t.1 += 1;
                    t.2 += (*c != 0xFFFF_FFFF) as usize;
                    if *k & 0x10 != 0 && *c != 0xFFFF_FFFF {
                        *odd.entry((*c, format!("{} {m}", g.name))).or_insert(0usize) += 1;
                    }
                    for i in 0..3 {
                        t.3[i] += b[i] as f64;
                    }
                }
            }
        }
        println!("# {p}");
        for ((m, k), (n, corners, dark, sum)) in tally {
            let mean = sum.map(|s| if corners > 0 { s / corners as f64 } else { 0.0 });
            println!("{m:<16} mask {k:#07x} x{n:<4} corners {corners:<6} non-white {dark:<6} mean {:.0} {:.0} {:.0}", mean[0], mean[1], mean[2]);
        }
        if !odd.is_empty() {
            let v: Vec<String> = odd.iter().map(|((c, a), n)| format!("{c:#010x} x{n} ({a})")).collect();
            println!("  `df` other than white under an `Am` mask: {}", v.join(", "));
        }
    }
}
