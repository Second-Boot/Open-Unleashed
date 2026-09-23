//! Diagnostic: the junction graph around a slice — its segment, the nodes at both ends and their
//! links, for checking a branch that leads nowhere.
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = nfs_assets::GamePaths::locate().unwrap();
    let t = nfs_assets::track::TrackAssets::load(&paths, &a[0]).unwrap();
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let j = &table.junctions;
    println!("{}: {} segments, {} nodes, lap_slices={}, lap_route={:?}", a[0], j.segments.len(), j.nodes.len(), j.lap_slices, j.lap_route);
    for s in a[1..].iter().filter_map(|v| v.parse::<usize>().ok()) {
        let seg = j.segments.iter().position(|g| s >= g.first && s <= g.last);
        println!("slice {s}: segment={seg:?} slice_node={:?} near_node={:?}", j.slice_node.get(s).copied().flatten(), j.near_node.get(s).copied().flatten());
        if let Some(gi) = seg {
            let g = &j.segments[gi];
            println!("  segment {gi}: {}..={} entry_node={:?} exit_node={:?} weight={:?}", g.first, g.last, g.entry_node, g.exit_node, g.weight);
            for (label, n) in [("entry", g.entry_node), ("exit", g.exit_node)] {
                if let Some(ni) = n {
                    let node = &j.nodes[ni];
                    println!("    {label} node {ni} at ({:.0},{:.0}) lap_slices={} links:", node.position.x, node.position.z, node.lap_slices);
                    for l in &node.links {
                        let sg = &j.segments[l.segment];
                        println!("      seg {:3} end {} -> slice {:5} (segment {}..={}, w={:?})", l.segment, l.end, sg.end_slice(l.end), sg.first, sg.last, sg.weight);
                    }
                }
            }
        }
    }
}
