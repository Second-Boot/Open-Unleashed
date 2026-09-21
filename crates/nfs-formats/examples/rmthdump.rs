//! Diagnostic: every `RMth` record of a CRP as raw dwords, for reading the layer lists.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        for e in crp.level.iter().filter(|e| e.is_level(b"RMth")) {
            let (off, len) = (e.data_offset(), e.data_len());
            let Some(bytes) = data.get(off..off + len) else { continue };
            let name: String = bytes[..16].iter().take_while(|b| **b != 0).map(|b| *b as char).collect();
            println!("== {name} ({len} bytes)");
            for (k, w) in bytes[16..].chunks_exact(4).enumerate() {
                let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                let f = f32::from_bits(v);
                let s = if (0x8000..0x8200).contains(&v) { format!("CODE {v:#x}") } else if f.is_finite() && f.abs() > 1e-6 && f.abs() < 1e6 && v > 0x10000 { format!("{f}") } else { format!("{v}") };
                print!("[{}] {s}  ", 16 + 4 * k);
            }
            println!();
        }
    }
}
