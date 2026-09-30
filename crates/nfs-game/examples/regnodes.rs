//! Census: how many nodes of the fitted tree each placed article is registered
//! into — `nfs_game::world::build_article_nodes` (hull spread through 0x5A0790 for the static and
//! mid-ground articles, 0x47BC20's one node for the instance and anim ones) — by drawer, and how
//! many articles 0x489930 leaves in **no** node (its `n = 0` with a non-zero `spent`).
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{build_article_nodes, cull_articles, tree_population, ArticleDrawer};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = TrackAssets::list(&paths);
    }
    for name in tracks {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let arts = cull_articles(&t);
        let pop = tree_population(&t, &arts);
        let tree = nfs_game::fitted_tree::FittedTree::new(pop.min, pop.max, &pop.points);
        let (nodes, _) = build_article_nodes(&tree, &arts);
        let mut by: std::collections::BTreeMap<&str, [usize; 6]> = Default::default();
        for (a, n) in arts.iter().zip(&nodes) {
            let Some(a) = a else { continue };
            let k = match a.drawer {
                ArticleDrawer::Static => "static",
                ArticleDrawer::MidGround { .. } => "midground",
                ArticleDrawer::Instance => "instance",
                ArticleDrawer::Anim => "anim",
            };
            by.entry(k).or_default()[n.len().min(5)] += 1;
        }
        println!("{name}: {} nodes; registered nodes per article [0, 1, 2, 3, 4, 5+]:", tree.node_count());
        for (k, h) in by {
            println!("    {k:<9} {h:?}");
        }
    }
}
