//! Print a `.mad` header, and the tables the decoder reads out of `nfs5.exe`.
//!
//! `cargo run -p nfs-formats --example madinfo -- <nfs5.exe> [file.mad ...]`
fn main() {
    let mut args = std::env::args().skip(1);
    let Some(exe) = args.next() else {
        eprintln!("usage: madinfo <nfs5.exe> [file.mad ...]");
        return;
    };
    match std::fs::read(&exe).map_err(|e| e.to_string()).and_then(|d| nfs_formats::mad::MadTables::from_exe(&d).map_err(|e| e.to_string())) {
        Ok(t) => {
            println!("tables from {exe}:");
            println!("  quant[0..8]    {:?}", &t.quant[..8]);
            println!("  prescale[0..4] {:?}", &t.prescale[..4]);
            println!("  scan[0..8]     {:?}", &t.scan[..8]);
            let nonzero = |v: &[u32]| v.iter().filter(|e| **e != 0).count();
            println!("  flat           {} dwords, {} filled", t.flat.len(), nonzero(&t.flat));
            for (name, at, n) in [("primary", 0usize, 512usize), ("escape_a", 512, 256), ("escape_b", 768, 256), ("motion", 1024, 64)] {
                let part = &t.flat[at..at + n];
                let holes: Vec<usize> = part.iter().enumerate().filter(|(_, e)| **e == 0).map(|(i, _)| i).collect();
                println!("  {name:<9} {n:5} entries, {} filled, first holes {:?}", nonzero(part), &holes[..holes.len().min(4)]);
            }
        }
        Err(e) => println!("{exe}: {e}"),
    }
    for p in args {
        match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|d| nfs_formats::mad::MadHeader::parse(&d).map_err(|e| e.to_string())) {
            Ok(h) => {
                println!("{p}: {} {}x{} period={} quant={}", String::from_utf8_lossy(&h.magic), h.width, h.height, h.period, h.quant);
                let d = std::fs::read(&p).unwrap();
                let mut counts: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
                let mut total = 0usize;
                for c in nfs_formats::mad::chunks(&d) {
                    let e = counts.entry(String::from_utf8_lossy(&c.tag).into_owned()).or_default();
                    e.0 += 1;
                    e.1 += c.data.len();
                    total += c.data.len();
                }
                println!("  chunks: {counts:?}  covering {total} of {} bytes", d.len());
            }
            Err(e) => println!("{p}: {e}"),
        }
    }
}
