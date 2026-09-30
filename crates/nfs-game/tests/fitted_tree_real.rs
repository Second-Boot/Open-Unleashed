//! The world's fitted tree over real track data (`nfs_game::fitted_tree`, `world::tree_population`,
//! `skidmarks::SkidMarks::for_tree`; `RE/notes/particles.md` §10.3): the items' vertices and box
//! corners build one tree; the static and mid-ground articles register by their hull, the instance
//! and anim ones by their sphere; the skid marks' cells are cut from it.

use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::fitted_tree::FittedTree;
use nfs_game::skidmarks::SkidMarks;
use nfs_game::world::{build_article_nodes, cull_articles, tree_population, ArticleDrawer};
use std::sync::Arc;

#[test]
fn alps_tree_registration_and_skid_cells() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "alps").unwrap();
    let arts = cull_articles(&t);
    let pop = tree_population(&t, &arts);
    // the first-LOD `vt` vertices of the placed articles (`examples/vtcount.rs`: alps 52 211)
    assert!(pop.points.len() > 50_000, "{}", pop.points.len());
    let tree = FittedTree::new(pop.min, pop.max, &pop.points);
    // the root is the square over the items' box; the tree is fine where the vertices are dense
    assert!(tree.size > 6000.0 && tree.size < 6500.0, "{}", tree.size);
    assert!(tree.node_count() > 1000, "{}", tree.node_count());

    let (nodes, seq) = build_article_nodes(&tree, &arts);
    let mut unregistered = 0;
    for ((a, n), s) in arts.iter().zip(&nodes).zip(&seq) {
        let Some(a) = a else { continue };
        assert_ne!(*s, u32::MAX);
        match a.drawer {
            // one node, by the sphere (0x47BC20)
            ArticleDrawer::Instance | ArticleDrawer::Anim => assert_eq!(n.len(), 1, "{}", a.name),
            _ => {
                if n.is_empty() {
                    unregistered += 1;
                }
            }
        }
    }
    // 0x489930's `n = 0` gap leaves almost nothing out of the tree altogether
    assert!(unregistered <= 5, "{unregistered}");

    // the skid marks: every point of the racing line finds a cell, a strip laid there is drawn under
    // that cell's key
    let line = t.racing_line(&paths, false).expect("alps' racing line");
    let mut marks = SkidMarks::for_tree(Arc::new(tree));
    marks.set_width(0, 0.1);
    let mut last = Vec3::ZERO;
    for p in line.points.iter().take(400) {
        let at = Vec3::new(p[0], 0.5, p[1]);
        marks.emit(0, at, 1.0, 1, Vec3::X, 0.5, 2.0);
        last = at;
    }
    let cells: Vec<_> = marks.cells(last).collect();
    assert!(!cells.is_empty());
    for (key, strips) in &cells {
        assert!(!strips.is_empty());
        // a key is a rounded square distance: finite and not absurd for a 6 km world
        assert!(*key < 6500 * 6500, "{key}");
    }
}
