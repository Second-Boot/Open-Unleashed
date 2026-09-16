//! Diagnostic: ground probes around a point (surface ids / heights).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let (x, y, z): (f32, f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap());
    for (dx, dz) in [(0.0, 0.0), (5.0, 0.0), (-5.0, 0.0), (0.0, 5.0), (0.0, -5.0), (15.0, 0.0), (0.0, 15.0)] {
        let h = grid.ground(x + dx, y + 20.0, z + dz, 200.0);
        println!("({:.0},{:.0}): {:?}", x + dx, z + dz, h.map(|h| (h.point.y, h.surface, h.surface & 0xF)));
    }
}
