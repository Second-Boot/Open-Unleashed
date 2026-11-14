//! Diagnostic: the ground colour a car is lit by (`World::car_light`'s `g`, 0x4746A0) along a line, with the hit
//! triangle's surface word and material against the drivable patch's colour (`TriangleGrid::patch_color`, what the car is lit by) —
//! for the car brightening over the finish line's chequered road.
//! `groundcol <track> x0 y z0 x1 z1 n`
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
        let (x, z) = (x0 + (x1 - x0) * s, z0 + (z1 - z0) * s);
        match grid.ground(x, y + 2.0, z, 20.0) {
            Some(h) => {
                let m = (h.surface >> 16) as usize;
                println!(
                    "({x:8.2},{z:9.2}) y {:7.3} ray colour {:.3} {:.3} {:.3} surface {:#010x} mat {:?} | patch colour {:?}",
                    h.point.y,
                    h.color[0],
                    h.color[1],
                    h.color[2],
                    h.surface,
                    mats.get(m).map(|m| m.texture_name()),
                    grid.patch_color(glam::Vec3::new(x, y, z)).map(|c| [c[0], c[1], c[2]])
                );
            }
            None => println!("({x:8.2},{z:9.2}) no ground"),
        }
    }
}
