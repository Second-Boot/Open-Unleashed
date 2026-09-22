//! Diagnostic: the ground normal and surface at a point, for chasing cars that will not drive away.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let (x, z): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    for dz in [-6.0, -4.0, -2.0, 0.0, 2.0, 4.0, 6.0] {
        if let Some(h) = grid.ground(x, 60.0, z + dz, 200.0) {
            println!("({x:.0},{:.0}) y={:.2} n={:?} surf={} up·n={:.3}", z + dz, h.point.y, h.normal, h.surface & 0xF, h.normal.dot(glam::Vec3::Y));
        } else {
            println!("({x:.0},{:.0}) no ground", z + dz);
        }
    }
}
