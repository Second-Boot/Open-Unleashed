//! Diagnostic: every distinct `mt` record of a CRP as raw dwords, with its render method.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let mut seen = std::collections::BTreeSet::new();
        for e in crp.material_entries.iter().filter(|e| e.is_part(b"mt")) {
            let (off, len) = (e.data_offset(), e.data_len());
            let Some(b) = data.get(off..off + len) else { continue };
            let rm: String = b.get(16..32).unwrap_or(&[]).iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            let words: Vec<String> = b.chunks_exact(4).enumerate().filter(|(k, _)| !(4..8).contains(k)).map(|(_, w)| format!("{:#x}", u32::from_le_bytes([w[0], w[1], w[2], w[3]]))).collect();
            if seen.insert((rm.clone(), words.clone())) {
                println!("{rm} ({len}): {}", words.join(" "));
            }
        }
    }
}
