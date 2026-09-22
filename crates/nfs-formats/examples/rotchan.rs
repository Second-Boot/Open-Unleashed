//! Diagnostic: which slot-9 rotation channels (`Base+75`) the shipped car models actually bind
//! parts to (`RE/notes/car-model.md`).
use nfs_formats::refpack;
fn main() {
    let mut hist: std::collections::BTreeMap<u8, usize> = Default::default();
    let mut wheels: std::collections::BTreeMap<u8, usize> = Default::default();
    let mut types: std::collections::BTreeMap<(u8, u32), usize> = Default::default();
    let mut per_file: std::collections::BTreeMap<String, std::collections::BTreeSet<u8>> = Default::default();
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let name = p.rsplit('/').next().unwrap().to_string();
        for a in crp.articles(&data).unwrap_or_default() {
            let Ok(g) = crp.article_geometry(&data, &a) else { continue };
            if let Some(b) = &g.base {
                if matches!(b.tag[0], 0x0D | 0x0E) {
                    *wheels.entry(b.rot_channel).or_default() += 1;
                }
                if b.rot_channel == 0 {
                    continue;
                }
                *hist.entry(b.rot_channel).or_default() += 1;
                if (18..=21).contains(&b.rot_channel) {
                    types.entry((b.rot_channel, b.tag[0] as u32)).and_modify(|n| *n += 1).or_insert(1usize);
                }
                per_file.entry(name.clone()).or_default().insert(b.rot_channel);
            }
        }
    }
    println!("channel: parts");
    for (c, n) in &hist {
        println!("  {c}: {n}");
    }
    println!("wheel-typed articles by channel: {wheels:?}");
    println!("channels 18..21 by part type: {types:?}");
    let mut sets: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (f, s) in per_file {
        sets.entry(format!("{s:?}")).or_default().push(f);
    }
    for (s, files) in sets {
        println!("{s} — {} files, e.g. {:?}", files.len(), &files[..files.len().min(4)]);
    }
}
