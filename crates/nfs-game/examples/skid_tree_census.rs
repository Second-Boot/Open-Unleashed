//! Census for `RE/notes/particles.md` §10 / `COVERAGE-2026-09-26.md` §14.32 / §14.72 (A1): the world's fitted tree as the original builds it (`nfs_game::fitted_tree`, 0x47B660 /
//! 0x47BE30 — the root the union of the placed items' box corners, the split the vertices of their
//! first-LOD `vt` parts, `nfs_game::world::tree_population`) and `WLineBundle`'s cells cut from it
//! (0x479810, `nfs_game::skidmarks`): how deep it goes, where a node has some but not all of its
//! four children (the "coarsening" case), and how many of those sit above the chosen level —
//! where the position → node → `lower_bound` lookup (0x47BAD0 / 0x47A300) answers a cell other
//! than the node's own.

use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::fitted_tree::{FittedTree, Node};

#[derive(Default)]
struct Census {
    nodes: usize,
    full: usize,
    partial: usize,
    leaves: usize,
    max_level: u32,
    /// nodes above the chosen level with 1…3 children: a cell that keeps descendants
    mixed_above_chosen: usize,
    cells: usize,
}

fn walk(t: &FittedTree, n: &Node, chosen: u32, c: &mut Census) {
    c.nodes += 1;
    c.max_level = c.max_level.max(n.level);
    let k = n.children.iter().flatten().count();
    match k {
        0 => c.leaves += 1,
        4 => c.full += 1,
        _ => c.partial += 1,
    }
    if n.level < chosen && (1..4).contains(&k) {
        c.mixed_above_chosen += 1;
    }
    if !(n.level < chosen && k == 4) {
        c.cells += 1;
    }
    for ch in n.children.iter().flatten() {
        walk(t, ch, chosen, c);
    }
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = TrackAssets::list(&paths);
    }
    for name in &tracks {
        let Ok(track) = TrackAssets::load(&paths, name) else { continue };
        let arts = nfs_game::world::cull_articles(&track);
        let pop = nfs_game::world::tree_population(&track, &arts);
        if pop.points.is_empty() {
            println!("{name}: no items");
            continue;
        }
        let tree = FittedTree::new(pop.min, pop.max, &pop.points);
        let mut level = 0;
        while level < 13 && tree.cell(level) >= 100.0 {
            level += 1;
        }
        let chosen = level.saturating_sub(1);
        let mut c = Census::default();
        walk(&tree, tree.root(), chosen, &mut c);
        println!(
            "{name}: {} points, root {:.0} m, chosen level {chosen}, {} nodes ({} full, {} partial, {} leaves), max level {}, {} cells, {} nodes above the chosen level with 1-3 children",
            pop.points.len(),
            tree.size,
            c.nodes,
            c.full,
            c.partial,
            c.leaves,
            c.max_level,
            c.cells,
            c.mixed_above_chosen
        );
    }
}
