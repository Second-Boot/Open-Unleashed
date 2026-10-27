//! Diagnostic (E23): the placed articles of a track whose name contains a word (default LAMP / LIGHT), with their footprint.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::cull_articles;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let words: Vec<String> = if a.len() > 1 { a[1..].iter().map(|s| s.to_uppercase()).collect() } else { vec!["LAMP".into(), "LIGHT".into()] };
    for c in cull_articles(&t).iter().flatten() {
        let n = c.name.to_uppercase();
        if words.iter().any(|w| n.contains(w)) {
            let (lo, hi) = (c.footprint_min, c.footprint_max);
            println!("{:?} {:?} size {:.1} {:.1} {:.1} top {:.1} centre {:.0} {:.0} {:.0}", c.name, c.drawer, hi.x - lo.x, hi.y - lo.y, hi.z - lo.z, hi.y, (hi.x + lo.x) / 2.0, (hi.y + lo.y) / 2.0, (hi.z + lo.z) / 2.0);
        }
    }
}
