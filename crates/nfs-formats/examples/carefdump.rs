//! Dump a car's `ef` records — the lamp glare sprites (`particles.md` §4, `car-model.md`):
//! `cargo run -p nfs-formats --example carefdump -- <car.crp>`
use nfs_formats::crp::Crp;

fn main() {
    let path = std::env::args().nth(1).expect("car.crp");
    let raw = std::fs::read(&path).unwrap();
    let data = nfs_formats::refpack::maybe_decompress(&raw).unwrap().into_owned();
    let crp = Crp::parse(&data).unwrap();
    for (ai, a) in crp.articles(&data).unwrap().iter().enumerate() {
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let b = crp.entry_data(&data, p).unwrap();
            if b.len() < 88 {
                println!("art {ai} ef#{} len {}", p.part_index(), b.len());
                continue;
            }
            let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
            let name = a.level(b"Name").and_then(|n| crp.entry_string(&data, n).ok()).unwrap_or_default();
            let base = a.level(b"Base").and_then(|n| crp.entry_data(&data, n).ok()).unwrap_or(&[]);
            println!("  {name} base[0x50] slot {:?} base[0x4B] channel {:?}", base.get(0x50), base.get(0x4B));
            // the article's `tr` parts — 0x59F220 looks up `tr` + the LOD for the glare's own frame
            for t in a.parts.iter().filter(|t| t.is_part(b"tr")) {
                if let Ok(m) = crp.entry_data(&data, t)
                    && m.len() >= 64
                {
                    let g = |o: usize| f32::from_le_bytes(m[o..o + 4].try_into().unwrap());
                    println!(
                        "    tr#{} rows ({:.3} {:.3} {:.3}) ({:.3} {:.3} {:.3}) ({:.3} {:.3} {:.3}) t ({:.3} {:.3} {:.3})",
                        t.part_index(),
                        g(0), g(4), g(8), g(16), g(20), g(24), g(32), g(36), g(40), g(48), g(52), g(56)
                    );
                }
            }
            println!(
                "art {ai:3} ef#{} lod {} type {} pos ({:.3} {:.3} {:.3} w {:.3}) a1 ({:.3} {:.3} {:.3}) a2 ({:.3} {:.3} {:.3}) +30 {:08x} {:08x} {:08x} {:08x} col {:08x} {:08x} +48 {:08x} flags {:02x?}",
                p.part_index(), p.lod(), u(0),
                f(8), f(12), f(16), f(20), f(24), f(28), f(32), f(40), f(44), f(48),
                u(56), u(60), u(64), u(68), u(72), u(76), u(80), &b[84..88]
            );
        }
    }
}
