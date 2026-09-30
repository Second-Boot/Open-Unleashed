//! `WTrack+0x24`, wired: `nfs_game::world::track_tree_order` must reproduce `insertAll(C)`'s own
//! bundle-by-bundle walk (`RE/RENDERING.md` "The tree, built" / "Bundle order, read 2026-09-28") —
//! `WInstanceBundle → WAnimBundle → WMidGroundBundle → WStaticBundle`, never revisiting an earlier
//! bundle once a later one has started, and accounting for every non-library article's own patch
//! or prop.

use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::crp::BundleKind;
use nfs_physics::track_tree::TreeItem;

#[test]
fn alps_tree_order_follows_the_bundle_partition() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "alps").unwrap();
    let geometry = t.build_geometry().unwrap();
    let props = nfs_game::world::track_props(&t);
    let collision = geometry.collision_grid(20.0, &|_| 1);
    let patches = collision.patches();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let order = nfs_game::world::track_tree_order(&t.crp, &t.crp_data, &arts, &geometry.world, &patches, &props, &[]);

    // every patch/prop of a `world` (non-library) article is accounted for exactly once — twice
    // for the rare article that is both a patch source and a `kCylinder`/`kBox` prop in this
    // port's own data, `track_tree_order`'s own doc comment's gap note (not the original's case,
    // which never has both for one article; unaffected by the count check below either way, since
    // both sides of the comparison include it the same number of times).
    let world_indices: std::collections::HashSet<usize> = geometry.world.iter().map(|a| a.index).collect();
    let expected_patches = patches.iter().filter(|p| world_indices.contains(&p.article)).count();
    let expected_props = props.iter().filter(|p| world_indices.contains(&p.article)).count();
    assert_eq!(order.len(), expected_patches + expected_props);
    assert!(order.len() > 1000, "alps should carry a large number of patches: {}", order.len());

    // the walk's own bundle rank, article by article, must be non-decreasing — the bundle-by-
    // bundle loop (`Instance → Anim → MidGround → Static`) never revisits an earlier bundle once
    // it has moved to a later one.
    let rank_of = |idx: usize| -> u8 {
        match t.crp.bundle_of(&t.crp_data, &arts[idx]).unwrap() {
            BundleKind::Instance => 0,
            BundleKind::Anim => 1,
            BundleKind::MidGround => 2,
            BundleKind::Static => 3,
        }
    };
    let ranks: Vec<u8> = order
        .iter()
        .map(|item| {
            let idx = match *item {
                TreeItem::Patch(i) => patches[i].article,
                TreeItem::Prop(i) => props[i].article,
                TreeItem::Wall(_) => unreachable!("track_tree_order never emits a Wall entry"),
            };
            rank_of(idx)
        })
        .collect();
    let mut sorted = ranks.clone();
    sorted.sort_unstable();
    assert_eq!(ranks, sorted, "insertAll(C)'s bundle order must be non-decreasing");
    // alps genuinely has articles in at least three of the four bundles (measured 2026-09-28:
    // 181 Instance / 3 Anim / 151 MidGround / 1417 Static, RE/RENDERING.md "Bundle order"), so a
    // real track exercises the ordering, not a vacuous single-bundle case.
    // `WAnimBundle` has no `PCollideArticleBundle` (`[bundle+0xC] = 0`, `0x488090`'s
    // `dynamic_cast` at 0x48838B skips it) — no patch and no prop of an `Anqt` article
    // and neither has `WMidGroundBundle` (its ctor 0x47CC80 goes through 0x489160 too; the three
    // `PCollideArticleBundle` constructions are the Instance, Static and Scene bundles')
    assert!(!ranks.contains(&1) && !ranks.contains(&2), "an Anim/MidGround-bundle article reached the collision tree");
    assert!(
        !patches.iter().any(|p| matches!(rank_of(p.article), 1 | 2)) && !props.iter().any(|p| matches!(rank_of(p.article), 1 | 2)),
        "an Anim/MidGround-bundle article's patch or prop exists"
    );
    assert!(ranks.contains(&0), "no Instance-bundle entries");
    assert!(ranks.contains(&3), "no Static-bundle entries");
}

/// `WSceneBundle` (`nfs_game::world::scene_props`, corrected 2026-09-28): every shipped, non-
/// skill-test race's start file places 2–5 `GEOM_ELEMENT`s, all tagged `FNSH`/`CONE`/`ARW1`-`ARW4`
/// (surveyed across every shipped `*_fstart.scn`/`*_bstart.scn`) — none of which is
/// `kCylinder`/`kBox` in `animdefs.txt` (`FNSH`/`ARW*` = `collideType=0`, `CONE` = `kSmackable`),
/// so the scene bundle contributes zero collidable `Prop`s on a real forward or backward race.
/// This is the *real* `N` the port's tree holds for the scene bundle — not "every spawned element
/// becomes a tree entry", the framing of `ac7b3e6c` and of this change both assumed
/// before `0x4751BD`/`0x475238`'s own collide-type gate was read for the scene's matched articles.
#[test]
fn alps_scene_bundle_contributes_no_collidable_props_on_a_shipped_race() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    for reverse in [false, true] {
        let mut t = TrackAssets::load(&paths, "alps").unwrap();
        t.reverse = reverse;
        let geometry = t.build_geometry().unwrap();
        assert!(!geometry.instances.is_empty(), "alps start scene should place at least one GEOM element");
        let scene = nfs_game::world::scene_props(&t, &geometry);
        assert!(scene.is_empty(), "reverse={reverse}: {scene:?}");
    }
}

/// A **skill test**'s `ENDW` barriers (`animdefs.txt`: `collideType=kBox`) are the real,
/// shipped case `alps_scene_bundle_contributes_no_collidable_props_on_a_shipped_race` says a plain
/// race never reaches: `canyon_st12.scn` (mission `1m02` "S-turn", `RE/notes/skill-tests.md` §13's
/// row 2, "ENDW 5") places 5 `ENDW` `GEOM_ELEMENT`s. `scene_props` must build **exactly one** `Prop`
/// per element — not two. This guards the 2026-09-28 fix: before it, the same 5 elements were
/// *also* turned into 5×4 = 20 wall-set edge segments by a pre-`scene_props` port addition
/// (`e25dc9a864`'s "ENDW barrier footprints join the wall set"), so the barrier had two collision
/// objects (a tree `Prop` and a wall) for one physical wall. That wall-set path is removed
/// (`crates/nfs-game/src/world.rs`'s `TrackWorld::upload`, the `WallSet::build` call); `scene_props`
/// is now the barrier's only collision object, matching the original's one-object-per-spawned-
/// element rule (`0x4750B0`'s per-article dispatch, `scene_props`'s own doc).
#[test]
fn canyon_st12_endw_barriers_are_not_duplicated() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut t = TrackAssets::load(&paths, "canyon").unwrap();
    let scene = t.load_scene("_st12.scn").unwrap().expect("canyon_st12.scn");
    t.extra_scenes.push(scene);
    let geometry = t.build_geometry().unwrap();
    let endw = u32::from_le_bytes(*b"ENDW");
    let endw_instances = geometry.instances.iter().filter(|i| i.source == 1 && i.tag == endw).count();
    assert_eq!(endw_instances, 5, "RE/notes/skill-tests.md §13 row 2: canyon_st12 carries 5 ENDW elements");

    let scene = nfs_game::world::scene_props(&t, &geometry);
    assert_eq!(scene.len(), endw_instances, "exactly one Prop per ENDW element, not two");
    assert!(
        scene.iter().all(|p| matches!(p.shape, nfs_physics::props::PropShape::Box { .. })),
        "every ENDW barrier is a kBox prop: {scene:?}"
    );

    // wired into the tree first, same as the non-skill-test case (`scene_indices_come_first_in_tree_order`)
    let props = nfs_game::world::track_props(&t);
    let scene_start = props.len();
    let mut props = props;
    props.extend(scene.clone());
    let scene_indices: Vec<usize> = (scene_start..props.len()).collect();
    let collision = geometry.collision_grid(20.0, &|_| 1);
    let patches = collision.patches();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let order = nfs_game::world::track_tree_order(&t.crp, &t.crp_data, &arts, &geometry.world, &patches, &props, &scene_indices);
    let leading: Vec<_> = order.iter().take(scene_indices.len()).collect();
    assert_eq!(leading.len(), 5);
    assert!(leading.iter().all(|item| matches!(item, TreeItem::Prop(i) if scene_indices.contains(i))), "the 5 ENDW props must lead the tree's own insertion order: {leading:?}");
}

/// The scene bundle is inserted **first** — before every `Instance`/`Anim`/`MidGround`/`Static`
/// entry — matching `0x487abd`'s call to `0x46D1D0` running before any other bundle's own `Load`
/// (`RE/RENDERING.md` "Bundle order, read 2026-09-28"). A synthetic scene-index list stands in for
/// a hypothetical track whose scene places a `kCylinder`/`kBox` article — unreached on any shipped
/// **race** (`alps_scene_bundle_contributes_no_collidable_props_on_a_shipped_race` above), though
/// not unreached overall: a shipped **skill test**'s `ENDW` barriers are exactly this case
/// (`canyon_st12_endw_barriers_are_not_duplicated` above, with real `Prop` data) — kept here too
/// since `track_tree_order` only needs the *indices*, not real `Prop` data, to prove the ordering.
#[test]
fn scene_indices_come_first_in_tree_order() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let t = TrackAssets::load(&paths, "alps").unwrap();
    let geometry = t.build_geometry().unwrap();
    let props = nfs_game::world::track_props(&t);
    // a synthetic scene prop appended after the real ones, as `TrackWorld::load` does
    let scene_indices = vec![props.len()];
    let collision = geometry.collision_grid(20.0, &|_| 1);
    let patches = collision.patches();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let order = nfs_game::world::track_tree_order(&t.crp, &t.crp_data, &arts, &geometry.world, &patches, &props, &scene_indices);
    assert_eq!(order.first(), Some(&TreeItem::Prop(props.len())), "the scene entry must lead the tree's own insertion order");
}
