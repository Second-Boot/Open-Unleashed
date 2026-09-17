//! Diagnostic: the `RMth` render-method records of a car or track, and how a primitive's stream
//! `slot` numbers line up with them (`RE/notes/crp-geometry.md` §4).
use nfs_assets::{car::CarAssets, track::TrackAssets, GamePaths};

fn dump(crp: &nfs_formats::crp::Crp, data: &[u8]) {
    for e in crp.level.iter().filter(|e| e.is_level(b"RMth")) {
        let Ok(b) = crp.entry_data(data, e) else { continue };
        let name: String = b[..16.min(b.len())].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
        let hdr = u32::from_le_bytes([b[16], b[17], b[18], b[19]]);
        let ops = u32::from_le_bytes([b[20], b[21], b[22], b[23]]);
        println!("{name:<16} passes {hdr} ops {ops} ({} bytes)", b.len());
        let mut o = 32;
        let mut i = 0;
        while o + 16 <= b.len() {
            let a = u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let c = u32::from_le_bytes([b[o + 4], b[o + 5], b[o + 6], b[o + 7]]);
            let d = u32::from_le_bytes([b[o + 8], b[o + 9], b[o + 10], b[o + 11]]);
            let e = u16::from_le_bytes([b[o + 12], b[o + 13]]);
            let tag: String = [b[o + 15] as char, b[o + 14] as char].iter().collect();
            let tag = if b[o + 14].is_ascii_graphic() { tag } else { "  ".into() };
            println!("   {i:>2}: {tag}  a {a:#07x}  b {c:#07x}  c {d:#07x}  d {e}");
            o += 16;
            i += 1;
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let what = a.first().map(|s| s.as_str()).unwrap_or("993");
    if let Ok(c) = CarAssets::load(&paths, what) {
        println!("== car {what}");
        dump(&c.crp, &c.crp_data);
    } else if let Ok(t) = TrackAssets::load(&paths, what) {
        println!("== track {what}");
        dump(&t.crp, &t.crp_data);
    }
}
