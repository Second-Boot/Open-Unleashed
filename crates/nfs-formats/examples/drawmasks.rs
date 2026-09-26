//! Diagnostic: the `Base+78` pass mask and the `Base+81` / `Base+82` flags of every car article,
//! with the LODs it has primitives in (`RE/notes/car-model.md`, audit 2026-09-26). The car draw
//! 0x444E00 runs `R3DCarLOD::draw` 0x43C830 per pass with a mask (0x80, 0x50, 0x20; 0x10 / 0x40
//! in the driver view, 0x30 for `+0x471`), and a part is drawn when `Base+78 & mask & 0xF0`.
//! Usage: `drawmasks <crp>…` — prints `mask flags81/82 name: lods (count)`.
use nfs_formats::refpack;
fn main() {
    let mut by: std::collections::BTreeMap<(u8, u8, u8, String), (std::collections::BTreeSet<u8>, usize)> = Default::default();
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        for a in crp.articles(&data).unwrap_or_default() {
            let Ok(g) = crp.article_geometry(&data, &a) else { continue };
            let Some(b) = &g.base else { continue };
            if g.primitives.is_empty() {
                continue;
            }
            let e = by.entry((b.draw_bytes[0], b.draw_bytes[3], b.draw_bytes[4], g.name.trim().to_string())).or_default();
            e.0.extend(g.primitives.iter().map(|p| p.lod));
            e.1 += 1;
        }
    }
    for ((m, f81, f82, n), (lods, c)) in by {
        println!("{m:02x} {f81}/{f82} {n}: {lods:?} ({c})");
    }
}
