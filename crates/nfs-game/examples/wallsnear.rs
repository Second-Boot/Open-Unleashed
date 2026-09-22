//! Diagnostic: the track's `.edg` wall segments near a point — for chasing a car that left the world.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let (x, z): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let r: f32 = a.get(3).and_then(|v| v.parse().ok()).unwrap_or(60.0);
    let mut hits: Vec<(f32, [f32; 3], [f32; 3], u32)> = Vec::new();
    for e in t.edges.iter() {
        for s in &e.segments {
            let mid = [(s.p1[0] + s.p2[0]) * 0.5, (s.p1[2] + s.p2[2]) * 0.5];
            let d = ((mid[0] - x).powi(2) + (mid[1] - z).powi(2)).sqrt();
            if d <= r {
                hits.push((d, s.p1, s.p2, s.tag));
            }
        }
    }
    hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    hits.dedup_by(|a, b| a.1 == b.1 && a.2 == b.2);
    for (d, p1, p2, tag) in hits.iter().take(16) {
        println!("  d={d:6.1}  ({:7.1},{:7.1}) -> ({:7.1},{:7.1})  tag={tag:#x}", p1[0], p1[2], p2[0], p2[2]);
    }
    println!("{}: {} wall segments within {r} of ({x}, {z})", a[0], hits.len());
}
