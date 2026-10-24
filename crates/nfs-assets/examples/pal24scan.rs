//! Diagnostic: every SHPI entry of every shipped .fsh / .crp (embedded `sf` files) /
//! .viv / BIGF member whose block chain holds a palette block of code 0x24 (the converter 0x571160's
//! input), with the image format, size, the 0x24 block's width/height fields and whether `Fsh::decode`
//! succeeds. Usage: `pal24scan [dir]` (default: the located game dir).
use nfs_assets::GamePaths;
use nfs_formats::{bigf::Bigf, crp::Crp, fsh::Fsh, refpack};

fn scan_fsh(label: &str, data: &[u8], n: &mut [usize; 3]) {
    let Ok(fsh) = Fsh::parse(data) else { return };
    for (i, e) in fsh.entries.iter().enumerate() {
        n[0] += 1;
        for b in e.blocks.iter().filter(|b| b.code == 0x24) {
            n[1] += 1;
            let img = &e.blocks[0];
            let ok = fsh.decode(data, i).is_ok();
            if !ok {
                n[2] += 1;
            }
            println!("{label} entry {i} '{}' fmt {:#04x} {}x{} pal24 w={} h={} misc={:?} decode_ok={ok} blocks={:?}", e.name.trim(), img.format_code(), img.width, img.height, b.width, b.height, b.misc, e.blocks.iter().map(|b| b.code).collect::<Vec<_>>());
        }
    }
}

fn scan(label: &str, data: &[u8], n: &mut [usize; 3], depth: u32) {
    if depth > 4 {
        return;
    }
    let owned;
    let data = match refpack::maybe_decompress(data) {
        Ok(d) => {
            owned = d.into_owned();
            &owned[..]
        }
        Err(_) => data,
    };
    if data.starts_with(b"SHPI") {
        scan_fsh(label, data, n);
    } else if data.starts_with(b"BIGF") {
        if let Ok(b) = Bigf::parse(data) {
            for e in &b.entries {
                scan(&format!("{label}/{}", e.name), b.data(data, e), n, depth + 1);
            }
        }
    } else if label.to_lowercase().ends_with(".crp") {
        if let Ok(c) = Crp::parse(data) {
            if let Ok(v) = c.embedded_texture_files(data) {
                for (k, f) in v.iter().enumerate() {
                    scan(&format!("{label}#sf{k}"), f, n, depth + 1);
                }
            }
        }
    }
}

fn walk(dir: &std::path::Path, n: &mut [usize; 3]) {
    let mut v: Vec<_> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).collect();
    v.sort_by_key(|e| e.path());
    for e in v {
        let p = e.path();
        if p.is_dir() {
            walk(&p, n);
        } else if let Ok(d) = std::fs::read(&p) {
            scan(&p.to_string_lossy(), &d, n, 0);
        }
    }
}

fn main() {
    let dir = std::env::args().nth(1).map(std::path::PathBuf::from).unwrap_or_else(|| GamePaths::locate().unwrap().root.clone());
    let mut n = [0; 3];
    walk(&dir, &mut n);
    println!("entries {} with 0x24 palette {} decode fails {}", n[0], n[1], n[2]);
}
