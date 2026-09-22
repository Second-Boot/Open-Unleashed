//! Diagnostic: a track `mt` record's length and its extension dwords, for the layer parameters.
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
            if b.len() < 48 { continue }
            let rm: String = b[16..32].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            let ext: Vec<String> = b[44..].chunks_exact(4).map(|w| format!("{:#x}", u32::from_le_bytes([w[0], w[1], w[2], w[3]]))).collect();
            let line = format!("{rm} ({len}B): {}", ext.join(" "));
            if seen.insert(line.clone()) { println!("{line}"); }
        }
    }
}
