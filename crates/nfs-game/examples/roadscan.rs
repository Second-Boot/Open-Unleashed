//! Diagnostic: the drivable surface across a track's road, slice by slice — the surface nibble of
//! the patch under points spread across the slice's own width.
//! `cargo run -p nfs-game --example roadscan -- monaco1 30 60`
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
    let t = SliceTable::from_crp(&track.crp, &track.crp_data).unwrap();
    let (lo, hi): (usize, usize) = (a[1].parse().unwrap(), a[2].parse().unwrap());
    for i in lo..hi.min(t.slices.len()) {
        let s = &t.slices[i];
        let mut row = String::new();
        for k in -8..=8 {
            let off = k as f32 * 1.5;
            let p = s.centre + s.lateral * off;
            let h = grid.ground(p.x, p.y + 4.0, p.z, 30.0);
            row.push(match h.map(|h| h.surface & 0xF) {
                None => '.',
                Some(0) => 'x',
                Some(n) => char::from_digit(n, 16).unwrap(),
            });
        }
        println!("{i:4} w=({:.1},{:.1})  -12m [{row}] +12m", s.width_a, s.width_b);
    }
}
