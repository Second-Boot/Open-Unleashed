//! Diagnostic: the render-method names a CRP carries (`RMth` level entries).
use nfs_formats::refpack;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let mut names: Vec<String> = Vec::new();
        let mut cur = String::new();
        for &b in data.iter() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                cur.push(b as char);
            } else {
                if cur.len() >= 5 {
                    names.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
        }
        names.sort();
        names.dedup();
        let interesting: Vec<&String> = names
            .iter()
            .filter(|n| n.contains("Light") || n.contains("Head") || n.contains("Night") || n.starts_with("S_") || n.contains("Ambient") || n.contains("Mat") || n.contains("Sort") || n.contains("Opaque"))
            .collect();
        println!("{}: {:?}", p.rsplit('/').next().unwrap(), interesting);
    }
}
