//! Diagnostic: the first `ef` parts of one kind on a track, as dwords and floats.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (track, kind, n) = (&a[0], a[1].parse::<u32>().unwrap(), a.get(2).and_then(|v| v.parse().ok()).unwrap_or(4usize));
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, track).unwrap();
    let mut shown = 0;
    for art in t.crp.articles(&t.crp_data).unwrap() {
        for p in art.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = t.crp.entry_data(&t.crp_data, p) else { continue };
            if d.len() < 4 || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != kind || shown >= n {
                continue;
            }
            shown += 1;
            let g = t.crp.article_geometry(&t.crp_data, &art).ok();
            println!("{} len {} tr {:?}", g.as_ref().map(|g| g.name.clone()).unwrap_or_default(), d.len(), g.as_ref().and_then(|g| g.transform));
            for (i, w) in d.chunks_exact(4).enumerate() {
                let u = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
                let tag: String = w.iter().map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '.' }).collect();
                println!("  +{:#04x} {u:#010x} {:>12.4} {tag}", i * 4, f32::from_bits(u));
            }
        }
    }
}
