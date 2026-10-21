//! PA-G6: `Vehicle::collide_props` only sees the props in the track tree's leaves (0x473870), so
//! every prop the port builds must be in the tree — built the way `TrackWorld` builds it.

use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::track_id::TrackId;
use nfs_physics::track_tree::{TrackTree, TreeItem};

#[test]
fn every_prop_is_in_a_track_tree_leaf() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut report = Vec::new();
    for id in TrackId::ALL {
        let name = id.name();
        let Ok(t) = TrackAssets::load(&paths, name) else { continue };
        let Ok(geometry) = t.build_geometry() else { continue };
        let mut props = nfs_game::world::track_props(&t);
        let scene_start = props.len();
        props.extend(nfs_game::world::scene_props(&t, &geometry));
        let scene: Vec<usize> = (scene_start..props.len()).collect();
        let patches = geometry.collision_grid(20.0, &|_| 1).patches();
        let arts = t.crp.articles(&t.crp_data).unwrap();
        let order = nfs_game::world::track_tree_order(&t.crp, &t.crp_data, &arts, &geometry.world, &patches, &props, &scene);
        let tree = TrackTree::build(&patches, &props, &[], &order);
        let mut seen = vec![false; props.len()];
        for it in tree.query_segment((-8192.0, -8192.0), (8192.0, 8192.0)) {
            if let TreeItem::Prop(k) = it {
                seen[k] = true;
            }
        }
        let missing: Vec<usize> = (0..props.len()).filter(|&k| !seen[k]).collect();
        eprintln!("{name}: {} props ({} scene), {} missing from the tree", props.len(), scene.len(), missing.len());
        for k in missing {
            let a = props[k].article;
            let bundle = arts.get(a).and_then(|art| t.crp.bundle_of(&t.crp_data, art).ok());
            let in_world = geometry.world.iter().any(|w| w.index == a);
            report.push(format!("{name}: prop {k} article {a} bundle {bundle:?} in_world {in_world} aabb {:?}..{:?}", props[k].aabb_min, props[k].aabb_max));
        }
    }
    assert!(report.is_empty(), "props missing from the tree:\n{}", report.join("\n"));
}
