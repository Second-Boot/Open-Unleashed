//! The lap fraction of a slice (`SliceTable::frac`, 0x481B70's per-section interpolation) against the
//! plain route position over the lap length: on the main lap the two agree to float rounding.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::race::SliceTable;

#[test]
fn section_fraction_matches_route_position_on_the_lap() {
    let Ok(paths) = GamePaths::locate() else { return };
    for name in ["alps", "coastal", "monaco1", "monaco5", "skidpad", "autobahn"] {
        let Ok(t) = TrackAssets::load(&paths, name) else { continue };
        let Ok(table) = SliceTable::from_crp(&t.crp, &t.crp_data) else { continue };
        for &s in &table.route {
            let by_route = table.route_pos[s] as f32 / table.lap_len as f32;
            assert!((table.frac(s) - by_route).abs() < 1e-5, "{name} slice {s}: {} vs {by_route}", table.frac(s));
        }
    }
}
