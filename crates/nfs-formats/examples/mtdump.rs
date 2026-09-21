//! Diagnostic: the car `mt` records' 236-byte extension (+76), as dwords, one line per distinct
//! render method, for reading the per-layer parameter blocks.
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
            if b.len() < 312 {
                continue;
            }
            let rm: String = b[16..32].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            let ext: Vec<String> = b[76..312]
                .chunks_exact(4)
                .map(|w| {
                    let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                    let f = f32::from_bits(v);
                    if v > 0x10000 && f.is_finite() && f.abs() > 1e-4 && f.abs() < 1e5 { format!("{f}") } else { format!("{v:#x}") }
                })
                .collect();
            if seen.insert((rm.clone(), ext.clone())) {
                println!("{rm}: {}", ext.join(" "));
            }
        }
    }
}
