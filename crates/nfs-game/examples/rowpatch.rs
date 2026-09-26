//! Diagnostic: `rowpatch <track> <log>` — for each `autotest` row of a run log (ANSI codes stripped),
//! the drivable patch under the player (`find_patch_height`): where a car left the road.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let text = std::fs::read_to_string(&a[1]).unwrap();
    for l in text.lines() {
        let Some(i) = l.find("autotest") else { continue };
        let l = &l[i..];
        let Some(pi) = l.find("pos=Vec3(") else { continue };
        let v: Vec<f32> = l[pi + 9..].trim_end_matches(')').split(',').map(|x| x.trim().trim_end_matches(')').parse().unwrap_or(0.0)).collect();
        let tt: String = l.chars().filter(|c| c.is_ascii()).collect::<String>();
        let tn = tt.split("t=").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).unwrap_or("?").to_string();
        let p = glam::Vec3::new(v[0], v[1], v[2]);
        println!("t={tn} pos=({:.1},{:.1},{:.1}) patch={:?}", p.x, p.y, p.z, grid.find_patch_height(p).map(|x| x.1));
    }
}
