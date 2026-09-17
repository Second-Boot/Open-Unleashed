//! Diagnostic: where the original's enclosure test (0x473330, `race-logic.md` §12) says the car is
//! under a tunnel — the lowest track patch more than 3.5 m above the slice centre, and whether its
//! flags word has bit 15 (a drivable road material).
//! `cargo run -p nfs-game --example tunnels -- monaco1 alps autobahn`
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in std::env::args().skip(1) {
        let Ok(track) = TrackAssets::load(&paths, &name) else { continue };
        let Ok(g) = track.build_geometry() else { continue };
        let Ok(mats) = track.materials() else { continue };
        let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
        let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
        let Ok(t) = SliceTable::from_crp(&track.crp, &track.crp_data) else { continue };
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut roofed = 0usize;
        for (i, s) in t.slices.iter().enumerate() {
            let p = s.centre;
            let hit = grid.ceiling(p.x, p.y, p.z, 3.5, 200.0);
            let word = hit.map(|h| (h.surface & 0xFFFF) as u16).unwrap_or(0);
            if word != 0 {
                roofed += 1;
            }
            if word & 0x8000 != 0 {
                match runs.last_mut() {
                    Some(r) if r.1 + 1 >= i => r.1 = i,
                    _ => runs.push((i, i)),
                }
            }
        }
        let total: usize = runs.iter().map(|(a, b)| b - a + 1).sum();
        println!("{name}: {} slices, {roofed} with anything overhead, {total} in tunnel, runs {:?}", t.slices.len(), runs.iter().take(12).collect::<Vec<_>>());
    }
}
