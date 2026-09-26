//! Diagnostic: every car article's `dz#lod` zone lists against its `vt#lod` and the damage part
//! `vt#(0x8000|lod)` (`RE/notes/car-model.md` §4.2, audit 2026-09-26: the deformation 0x43E390),
//! and the `Base+78 & 8` bit that 0x43E3D8 deforms by.
//! Usage: `dzcheck <crp>…`.
use nfs_formats::refpack;
fn main() {
    let (mut arts, mut bad, mut no_delta, mut zones_seen) = (0, 0, 0, std::collections::BTreeSet::new());
    let mut by_bit: std::collections::BTreeMap<(bool, bool), usize> = Default::default();
    for p in std::env::args().skip(1) {
        let raw = std::fs::read(&p).unwrap();
        let data = refpack::maybe_decompress(&raw).unwrap();
        let crp = nfs_formats::crp::Crp::parse(&data).unwrap();
        for a in crp.articles(&data).unwrap_or_default() {
            let Ok(g) = crp.article_geometry(&data, &a) else { continue };
            let bit = g.base.as_ref().is_some_and(|b| b.draw_bytes[0] & 8 != 0);
            let lods: std::collections::BTreeSet<u8> = g.primitives.iter().map(|q| q.lod).collect();
            let mut has_dz = false;
            for l in lods {
                let Ok(Some(dz)) = crp.damage_zones(&data, &a, l) else { continue };
                has_dz = true;
                arts += 1;
                let n = a.find(b"vt", u16::from(l)).and_then(|e| crp.entry_data(&data, e).ok()).map_or(0, |b| b.len() / 16);
                let total: usize = dz.zones.iter().map(|z| z.1.len()).sum();
                let max = dz.zones.iter().flat_map(|z| z.1.iter()).copied().max().unwrap_or(0) as usize;
                zones_seen.extend(dz.zones.iter().map(|z| z.0));
                if total != n || max >= n {
                    bad += 1;
                    println!("{p} {} lod {l}: {total} listed, {n} vertices, max index {max}", g.name);
                }
                if a.find(b"vt", 0x8000 | u16::from(l)).is_none() {
                    no_delta += 1;
                }
            }
            *by_bit.entry((bit, has_dz)).or_default() += 1;
        }
    }
    println!("{arts} article LODs with dz, {bad} mismatched, {no_delta} without vt#0x8000|lod; zones {zones_seen:?}");
    println!("(Base+78 & 8, has dz) → articles: {by_bit:?}");
}
