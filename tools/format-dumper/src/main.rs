//! Low-level dumps for reverse engineering.
//!
//! ```text
//! format-dumper crp <file.crp> level                 # level table
//! format-dumper crp <file.crp> materials             # mt records (hex + parsed)
//! format-dumper crp <file.crp> article <N> [tag]     # part run of article N; with tag: hex+float dump of each part
//! format-dumper floats <file> <offset> <count>       # f32 dump at offset
//! format-dumper hex <file> <offset> <len>
//! ```
use anyhow::{bail, Result};
use nfs_formats::{crp::{Crp, CrpEntry}, refpack};

fn hexdump(b: &[u8], base: usize) {
    for (i, row) in b.chunks(16).enumerate() {
        let hex: Vec<String> = row.iter().map(|x| format!("{x:02x}")).collect();
        let asc: String = row.iter().map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '.' }).collect();
        println!("{:08x}  {:<48}  {asc}", base + i * 16, hex.join(" "));
    }
}

fn floats(b: &[u8]) -> String {
    b.chunks_exact(4).map(|c| format!("{:.4}", f32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect::<Vec<_>>().join(" ")
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = a.first() else { bail!("see module docs for usage") };
    match cmd.as_str() {
        "hex" | "floats" => {
            let d = std::fs::read(&a[1])?;
            let d = refpack::maybe_decompress(&d)?;
            let off: usize = parse_num(&a[2]);
            let n: usize = parse_num(&a[3]);
            if cmd == "hex" { hexdump(&d[off..off + n], off) } else { println!("{}", floats(&d[off..off + n * 4])) }
        }
        "crp" => {
            let d = refpack::maybe_decompress(&std::fs::read(&a[1])?)?.into_owned();
            let c = Crp::parse(&d)?;
            match a.get(2).map(String::as_str) {
                Some("level") => {
                    for e in &c.level {
                        match e {
                            CrpEntry::Level { entry_index, tag, a, b, c: cc } => println!("{entry_index:5} {} a={a:#x} b={b} c={cc}", String::from_utf8_lossy(tag)),
                            CrpEntry::Part { .. } => {}
                        }
                    }
                }
                Some("materials") => {
                    for (i, e) in c.material_entries.iter().enumerate() {
                        let b = c.entry_data(&d, e)?;
                        println!("--- {i} {} len={} kind={:#x}", e.tag_str(), b.len(), e.kind_byte());
                        hexdump(&b[..b.len().min(96)], e.data_offset());
                    }
                }
                Some("article") => {
                    let n: usize = parse_num(&a[3]);
                    let arts = c.articles(&d)?;
                    let art = &arts[n];
                    let want = a.get(4);
                    for p in &art.parts {
                        let b = c.entry_data(&d, p)?;
                        let (idx, kind) = (p.part_index(), p.kind_byte());
                        println!("{:>5} #{idx:<5} len={:<6} kind={kind:#04x} b={} off={:#x}", p.tag_str(), b.len(), match p { CrpEntry::Part { b, .. } | CrpEntry::Level { b, .. } => *b }, p.data_offset());
                        if want.map(|w| w == &p.tag_str()).unwrap_or(false) {
                            hexdump(&b[..b.len().min(256)], p.data_offset());
                            println!("floats: {}", floats(&b[..b.len().min(128)]));
                        }
                    }
                }
                _ => bail!("crp subcommand: level | materials | article <N> [tag]"),
            }
        }
        _ => bail!("unknown command"),
    }
    Ok(())
}

fn parse_num(s: &str) -> usize {
    if let Some(h) = s.strip_prefix("0x") { usize::from_str_radix(h, 16).unwrap() } else { s.parse().unwrap() }
}
