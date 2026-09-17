//! Diagnostic: which render-layer class codes (0x8001…0x8016, `RE/notes/lighting.md`) a CRP's
//! render methods use — the `Headlights::*` ones (0x8002, 0x8003, 0x8011) are what turns the
//! game's night flag on.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        // only the `RMth` level entries, not the whole file
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let mut counts = std::collections::BTreeMap::new();
        let mut methods = 0usize;
        for e in crp.level.iter().filter(|e| e.is_level(b"RMth")) {
            methods += 1;
            let (off, len) = (e.data_offset(), e.data_len());
            let Some(bytes) = data.get(off..off + len) else { continue };
            for w in bytes.chunks_exact(4) {
                let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                if (0x8001..=0x8016).contains(&v) {
                    *counts.entry(v).or_insert(0usize) += 1;
                }
            }
        }
        print!("{methods} methods, ");
        let night: usize = [0x8002u32, 0x8003, 0x8011].iter().map(|c| counts.get(c).copied().unwrap_or(0)).sum();
        println!("{}: night {night}, codes {:?}", p.rsplit('/').next().unwrap(), counts.iter().map(|(k, v)| (format!("{k:#x}"), v)).collect::<Vec<_>>());
    }
}
