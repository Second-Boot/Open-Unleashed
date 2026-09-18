//! Diagnose `SliceTable::find_slice` against every slice of a track.
fn main() -> anyhow::Result<()> {
    let paths = nfs_assets::paths::GamePaths::locate()?;
    let name = std::env::args().nth(1).unwrap_or_else(|| "castle".into());
    let t = nfs_assets::track::TrackAssets::load(&paths, &name)?;
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).map_err(|e| anyhow::anyhow!(e))?;
    let mut bad: Vec<(f32, usize, usize)> = Vec::new();
    for s in 0..table.slices.len() {
        let got = table.find_slice(table.slices[s].centre, s);
        let d = (table.slices[got].centre - table.slices[s].centre).length();
        if d > 7.0 {
            bad.push((d, s, got));
        }
    }
    bad.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("{}: {} slices, {} mis-tracked by > 7 m", name, table.slices.len(), bad.len());
    for (d, s, got) in bad.iter().take(12) {
        let node = table.junctions.near_node.get(*s).copied().flatten();
        let list = node.and_then(|n| table.junctions.node_slices.get(n)).map(|v| v.len()).unwrap_or(0);
        println!("  slice {s} -> {got} ({d:.1} m) near_node={node:?} list={list}");
    }
    Ok(())
}
