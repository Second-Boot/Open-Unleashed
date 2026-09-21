//! Diagnostic: the articles whose `Base+77` names a model animation (0x43E195 reads it as
//! `[[part+0x44] + 9]`), with their `vt` parts' indices — the frames 0x43C4FB counts.
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        let Ok(arts) = crp.articles(&data) else { continue };
        for (k, a) in arts.iter().enumerate() {
            let Some(base) = a.level(b"Base") else { continue };
            let off = base.data_offset();
            let Some(b) = data.get(off..off + base.data_len()) else { continue };
            if b.len() <= 77 || b[77] == 0 {
                continue;
            }
            let tag: String = b[68..72].iter().map(|c| if c.is_ascii_graphic() { *c as char } else { '.' }).collect();
            let vts: Vec<String> = a.parts.iter().filter(|e| e.is_part(b"vt")).map(|e| format!("{:#x}", e.part_index())).collect();
            println!("{}: art {k} tag {tag} anim {} b76 {} vt {:?}", p.rsplit('/').next().unwrap(), b[77], b[76], vts);
        }
    }
}
