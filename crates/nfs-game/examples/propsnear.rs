//! Diagnostic: the collision props (`world::track_props`) within `r` metres of a point —
//! `propsnear <track> <x> <z> [r]`.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let (x, z): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    let r: f32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(15.0);
    for p in nfs_game::world::track_props(&t) {
        let c = p.centre();
        let d = ((c.x - x).powi(2) + (c.z - z).powi(2)).sqrt();
        if d < r {
            println!("d {d:6.2} centre ({:.1} {:.1} {:.1}) aabb x {:.1}..{:.1} z {:.1}..{:.1} breakable {} {:?}", c.x, c.y, c.z, p.aabb_min.x, p.aabb_max.x, p.aabb_min.z, p.aabb_max.z, p.breakable.is_some(), p.shape);
        }
    }
}
