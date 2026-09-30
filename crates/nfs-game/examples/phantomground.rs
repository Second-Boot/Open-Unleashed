//! is there drivable ground around a point? `phantomground <track> <x> <z>` prints the
//! ground height and surface nibble on a 4 m grid around it (±16 m) and the nearest slice.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let track = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = track.build_geometry().unwrap();
    let mats = track.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let (x0, z0): (f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    for dz in (-4..=4).rev() {
        let mut row = String::new();
        for dx in -4..=4 {
            let (x, z) = (x0 + dx as f32 * 4.0, z0 + dz as f32 * 4.0);
            match grid.ground(x, 400.0, z, 800.0) {
                Some(h) => row.push_str(&format!(" {:6.1}/{:x}", h.point.y, h.surface & 0xF)),
                None => row.push_str("      .  "),
            }
        }
        println!("z {:7.1} {row}", z0 + dz as f32 * 4.0);
    }
    let t = SliceTable::from_crp(&track.crp, &track.crp_data).unwrap();
    let d = |s: &nfs_game::race::Slice| (s.centre.x - x0).hypot(s.centre.z - z0);
    let best = t.slices.iter().enumerate().min_by(|p, q| d(p.1).total_cmp(&d(q.1))).unwrap();
    println!("nearest slice {} at {:.1?} width {:.1}/{:.1} dist {:.1}", best.0, best.1.centre, best.1.width_a, best.1.width_b, d(best.1));
}
