//! Diagnostic: every `RMth` record's pass table — sixteen bytes a pass from `+0x1C`, the third
//! dword the option mask 0x44C185 tests against `[0x6197F8]` — and the ops' own `+0xC` words.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        println!("# {p}");
        for e in crp.level.iter().filter(|e| e.is_level(b"RMth")) {
            let (off, len) = (e.data_offset(), e.data_len());
            let Some(b) = data.get(off..off + len) else { continue };
            let d = |o: usize| b.get(o..o + 4).map_or(0, |w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
            let name: String = b[..16].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
            let (passes, ops) = (d(0x10) as usize, d(0x14) as usize);
            print!("{name:<16} passes {passes} ops {ops} +18={:#x} |", d(0x18));
            for k in 0..passes {
                let o = 0x1C + 16 * k;
                print!(" [{} {} m{:#x} {:#x}]", d(o), d(o + 4), d(o + 8), d(o + 12));
            }
            let base = 0x1C + 16 * passes;
            let ops: Vec<String> = (0..ops).map(|i| base + 16 * i).map(|o| format!("{:#x}:{:#x}/{:#x}", d(o), d(o + 4), d(o + 12))).collect();
            println!("  size {len} expect {}  ops {}", base + 16 * ops.len(), ops.join(" "));
        }
    }
}
