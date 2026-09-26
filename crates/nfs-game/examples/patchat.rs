//! Diagnostic: the drivable patch (`find_patch_height`) along a line — for chasing a wall push
//! that `WallImpulse` 0x494000 zeroes over non-road.
//! `patchat <track> x0 y z0 x1 z1 n`
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let f: Vec<f32> = a[1..].iter().map(|v| v.parse().unwrap()).collect();
    let (x0, y, z0, x1, z1, n) = (f[0], f[1], f[2], f[3], f[4], f[5] as usize);
    for i in 0..=n {
        let s = i as f32 / n as f32;
        let p = glam::Vec3::new(x0 + (x1 - x0) * s, y, z0 + (z1 - z0) * s);
        println!("({:8.2},{:8.2}): {:?}", p.x, p.z, grid.find_patch_height(p));
    }
}
