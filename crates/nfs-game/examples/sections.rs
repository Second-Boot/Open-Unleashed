//! Diagnostic: a track's slice sections and the lap route order.
fn main() {
    let name = std::env::args().nth(1).unwrap_or_else(|| "monaco4".into());
    let paths = nfs_assets::GamePaths::locate().unwrap();
    let t = nfs_assets::track::TrackAssets::load(&paths, &name).unwrap();
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    println!("{name}: {} slices, {} sections", table.slices.len(), table.sections.len());
    for (i, s) in table.sections.iter().enumerate() {
        println!("  section {i}: slices {}..={}", s.first, s.last);
    }
    println!("  route (section order): {:?}", table.junctions.lap_route);
}
