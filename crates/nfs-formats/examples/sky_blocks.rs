//! Census: how many blocks of each shipped sky file `SkyFile::for_level` selects at each
//! `SKYRENDERING` level (0x45FB80's matching walk).
use nfs_formats::sky::SkyFile;

fn main() {
    for p in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&p) else { continue };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let lines: Vec<String> = text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
        let Ok(f) = SkyFile::parse(&lines.join("\n")) else {
            println!("{p}: parse failed");
            continue;
        };
        let counts: Vec<usize> = (0..4).map(|l| f.for_level(l).len()).collect();
        println!("{}: blocks {} per level {:?}", p.rsplit('/').next().unwrap(), f.blocks.len(), counts);
    }
}
