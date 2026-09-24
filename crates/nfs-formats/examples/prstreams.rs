//! Diagnostic: how often each `pr` stream kind occurs over a CRP's articles, with the tag spelt out.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let Ok(arts) = crp.articles(&data) else { continue };
        let mut kinds = std::collections::BTreeMap::new();
        for art in &arts {
            for pr in art.parts.iter().filter(|e| e.is_part(b"pr")) {
                let Ok(b) = crp.entry_data(&data, pr) else { continue };
                let Ok(rec) = nfs_formats::crp::PrimitiveRecord::parse(b, pr.data_offset()) else { continue };
                for s in &rec.streams {
                    let t = s.kind.to_be_bytes();
                    let tag: String = if t.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'$') { t.iter().map(|&c| c as char).collect() } else { format!("{:#x}", s.kind) };
                    *kinds.entry(tag).or_insert(0u32) += 1;
                }
            }
        }
        if std::env::var("PR_DUMP").is_ok() {
            for art in arts.iter().take(3) {
                for pr in art.parts.iter().filter(|e| e.is_part(b"pr")).take(2) {
                    let Ok(b) = crp.entry_data(&data, pr) else { continue };
                    let Ok(rec) = nfs_formats::crp::PrimitiveRecord::parse(b, pr.data_offset()) else { continue };
                    println!("pr material {} streams {:?}", rec.material, rec.streams.iter().map(|s| (s.slot, s.kind, s.byte_len, s.lod, s.fmt)).collect::<Vec<_>>());
                }
            }
        }
        println!("# {p}");
        for (t, n) in kinds {
            println!("{t:>8}: {n}");
        }
    }
}
