//! Census for the open cull-tree question (`185de5d0`, `COVERAGE
//! §14.32`/`RENDERING.md`): the original's fitted cull tree prunes a whole node by its **fixed**
//! geometric quadrant box (`0x47F400`/`0x47F310`, split `0x47BE30`) before ever reaching a
//! per-item test; the port's direct per-item test (`TrackWorld::article_draws`/`walk_item`,
//! `nfs_game::world::CullView::sphere_visible`) has no node pass at all, so it can only draw the
//! original's set **or more** (`RENDERING.md` "The node pass 0x47F400 and the fitted tree's split
//! 0x47BE30"). The one documented direction of divergence: "an item whose footprint lies wholly
//! past `far` (or wholly behind the wedge) while its bounding sphere reaches inside — a large,
//! flat item seen at the edge of the draw distance" — because a node's own circle is centred on
//! its **fixed** quadrant box, not on the items inside it, so a big, sparsely-populated node whose
//! centre sits well past `far` can test "wholly outside" even while one of its own items, near the
//! node's edge, sits well within `far` of the eye. `COVERAGE §14.32.1` reasoned this out but never
//! measured it against real per-track item data — this does.
//!
//! **The two quantities the node test and the port's own per-item test share exactly**
//! (`RENDERING.md` "The footprint — 0x47F550/0x47F210" and "0x47F310"): `far` = `this+0x9C` =
//! `cam+0x124` = `[0x6197CC]`, the very draw distance `nfs_game::world::CullView::draw` already
//! carries and `walk_item` already tests against; `h` (the wedge's half-angle, turned `±h` about
//! `y`) = `cam+0x130`, "the view's half-angle... the horizontal one" — the same quantity the
//! port's own `CullView::tan_h` approximates. So the two tests differ only in *shape* (a node's
//! circle-vs-wedge test, pure XZ, vs. the port's true pyramidal frustum) and in *centre* (the
//! node's fixed box centre vs. an item's own true centre) — never in scale.
//!
//! Per shipped track: load every placed article's real cull sphere
//! (`nfs_game::world::cull_articles`, `Base+0x2C…`/`+0x3C`'s centre/radius, transformed for an
//! instance), build `0x47BE30`'s exact split rule over the item centres (mirrors
//! `skid_tree_census.rs`), and for candidate eye positions along the track's own `.lsp` racing
//! line (real driving positions, not synthetic ones) with the camera aimed straight at each item
//! (the setup most favourable to the item passing its own test, so a negative census here is not
//! an artefact of a badly-aimed camera) at real per-slice draw distances
//! (`nfs_formats::key::DrawKeys::far_distance`, detail 1.0 — the largest, most permissive setting,
//! so a negative census is not an artefact of an unrealistically short one either), check: does the
//! item's own sphere (`true_visible`, a direct copy of `CullView::sphere_visible` plus
//! `walk_item`'s own extra draw-distance test — that function is private, so it is re-derived here
//! rather than exposed just for this script) ever pass while the item's own tree node (found by
//! descending `0x47BE30`'s split with the item's centre, its box the node's fixed geometric
//! quadrant per `0x47C2C0`) is wholly pruned (`wholly_pruned`, `0x47F310`'s far-circle-and-wedge
//! rule) by the *same* eye/forward/`h`/`far`?
//!
//! **2026-09-30:** the registration is the original's, now in
//! `nfs_game::fitted_tree` / `nfs_game::world::build_article_nodes` — the tree is built from the
//! placed articles' first-LOD `vt` vertices over the box-corner root (`tree_population`), the
//! static and mid-ground articles are spread by their hull through 0x5A0790's polygon-rectangle
//! test (0x47BDC0 / 0x489930), and the instance and anim articles sit in the one node 0x47BC20
//! answers for their sphere. The node pass runs in the game (`world::NODE_PRUNE`, on); this census
//! only measures whether a node the article is registered in ever drops it while its own sphere
//! passes the per-item test. `h` is swept over a band around the documented default half-angle
//! (`race-logic.md` §11, `SetSlotView`'s 37.5°/aspect).

use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::cull_articles;

/// `0x47F310`: is the circle `(centre, r)` **wholly outside** the far circle (radius `far`, from
/// `eye`) or wholly behind one side of the `±h`-degree ground wedge around `forward` (both xz-only,
/// as the original's own node test is — "`d` = the xz distance")? `forward` is a unit xz vector.
fn wholly_pruned(eye: (f32, f32), forward: (f32, f32), h_deg: f32, far: f32, centre: (f32, f32), r: f32) -> bool {
    let (dx, dz) = (centre.0 - eye.0, centre.1 - eye.1);
    let d = (dx * dx + dz * dz).sqrt();
    let e = d - far;
    if e > 0.0 && e > r {
        return true;
    }
    let h = h_deg.to_radians();
    let rot = |v: (f32, f32), a: f32| (v.0 * a.cos() - v.1 * a.sin(), v.0 * a.sin() + v.1 * a.cos());
    let perp = |v: (f32, f32)| (-v.1, v.0);
    for side in [1.0f32, -1.0] {
        let f = rot(forward, side * h);
        let n = (perp(f).0 * side, perp(f).1 * side);
        let s = n.0 * dx + n.1 * dz;
        if s > 0.0 && s > r {
            return true;
        }
    }
    false
}

/// The port's own per-item test, copied rather than exposed (`CullView::sphere_visible` is
/// private to `nfs_game::world`): the true frustum (`0x539480`, far plane a fixed **700**,
/// `RENDERING.md` "the camera object's 700 (0x4677FC)") AND `walk_item`'s own extra draw-distance
/// test against the real per-slice `far` (`[0x6197CC]`) — the two together are the port's actual,
/// combined per-item admit rule.
#[allow(clippy::too_many_arguments)]
fn true_visible(eye: Vec3, forward: Vec3, right: Vec3, up: Vec3, tan_h: f32, tan_v: f32, near: f32, far: f32, centre: Vec3, r: f32) -> bool {
    let rel = centre - eye;
    let depth = rel.dot(forward);
    let a = near - depth;
    const FRUSTUM_FAR: f32 = 700.0;
    if a > 0.0 {
        if a > r {
            return false;
        }
    } else if depth - FRUSTUM_FAR > r {
        return false;
    }
    for (x, t) in [(rel.dot(right), tan_h), (rel.dot(up), tan_v)] {
        let cos = 1.0 / (1.0 + t * t).sqrt();
        let p = x - depth * t;
        if p > 0.0 {
            if p * cos > r {
                return false;
            }
        } else {
            let q = x + depth * t;
            if q < 0.0 && -q * cos > r {
                return false;
            }
        }
    }
    let d2 = rel.length_squared();
    let s = (d2.sqrt() - r).max(0.0);
    s * s < far * far
}

fn sanity_check() {
    // forward +z, dead ahead at 100 m within a 300 m far and a 45 deg wedge: never pruned.
    assert!(!wholly_pruned((0.0, 0.0), (0.0, 1.0), 45.0, 300.0, (0.0, 100.0), 1.0));
    // 90 degrees to the side, same distance: wholly outside a 45 deg wedge.
    assert!(wholly_pruned((0.0, 0.0), (0.0, 1.0), 45.0, 300.0, (100.0, 0.0), 1.0));
    // dead ahead but past far by more than its own radius: wholly outside the far circle.
    assert!(wholly_pruned((0.0, 0.0), (0.0, 1.0), 45.0, 300.0, (0.0, 350.0), 1.0));
    // dead ahead, past far, but its own radius reaches back inside: not pruned.
    assert!(!wholly_pruned((0.0, 0.0), (0.0, 1.0), 45.0, 300.0, (0.0, 320.0), 30.0));
}

fn main() {
    sanity_check();
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = TrackAssets::list(&paths);
    }
    let h_sweep = [35.0f32, 45.0, 55.0];
    let mut total_checks = 0u64;
    let mut total_divergences = 0u64;
    for t in &tracks {
        let Ok(track) = TrackAssets::load(&paths, t) else { continue };
        let arts = cull_articles(&track);
        let items: Vec<(Vec3, f32)> = arts.iter().flatten().map(|a| (a.centre, a.radius)).collect();
        if items.is_empty() {
            println!("{t}: no articles");
            continue;
        }
        let Some(line) = track.racing_line(&paths, false) else {
            println!("{t}: no racing line, skipped");
            continue;
        };
        if line.points.is_empty() {
            continue;
        }
        let population = nfs_game::world::tree_population(&track, &arts);
        let tree = nfs_game::fitted_tree::FittedTree::new(population.min, population.max, &population.points);
        let (registered, _) = nfs_game::world::build_article_nodes(&tree, &arts);
        let registered: Vec<&Vec<nfs_game::world::RegisteredNode>> = arts.iter().zip(&registered).filter(|(a, _)| a.is_some()).map(|(_, r)| r).collect();

        let mut track_checks = 0u64;
        let mut track_divergences = 0u64;
        let mut examples: Vec<String> = Vec::new();
        let n_pts = line.points.len();

        for (item_idx, (centre, radius)) in items.iter().enumerate() {
            let node_circles: Vec<((f32, f32), f32)> = registered[item_idx].iter().map(|&(_, cx, cz, r)| ((cx, cz), r)).collect();

            // Sample eye positions along the real racing line (every 3rd point keeps this fast).
            for (pi, p) in line.points.iter().enumerate().step_by(3) {
                let eye = Vec3::new(p[0], centre.y, p[1]);
                let to_item = Vec3::new(centre.x - eye.x, 0.0, centre.z - eye.z);
                if to_item.length_squared() < 1.0 {
                    continue; // degenerate: eye already at the item
                }
                let forward = to_item.normalize();
                let right = forward.cross(Vec3::Y).normalize_or_zero();
                if right.length_squared() < 1e-6 {
                    continue;
                }
                let up = right.cross(forward).normalize();
                let slice = (pi * 399 / n_pts.max(1)).min(399);
                let far = track.draw_keys_forward.far_distance(slice, 1.0);

                for &h in &h_sweep {
                    total_checks += 1;
                    track_checks += 1;
                    let tan_h = h.to_radians().tan();
                    let tan_v = tan_h * 0.75;
                    let item_true = true_visible(eye, forward, right, up, tan_h, tan_v, 1.0, far, *centre, *radius);
                    if !item_true {
                        continue;
                    }
                    // The port keeps the article iff *any* of its registered nodes passes
                    // (`0x47F400`'s tree walk visits a node iff every ancestor also passes, which
                    // a per-node independent check reproduces exactly: a child's fixed circle is
                    // always inscribed in its parent's, so "parent wholly pruned" already implies
                    // "child wholly pruned" — `COVERAGE-2026-09-26.md` §14.36).
                    let pruned = node_circles.iter().all(|&(nc, nr)| wholly_pruned((eye.x, eye.z), (forward.x, forward.z), h, far, nc, nr));
                    if pruned {
                        total_divergences += 1;
                        track_divergences += 1;
                        if examples.len() < 3 {
                            examples.push(format!(
                                "item {item_idx} ({:?}, r={:.1}) at pt {pi} eye=({:.0},{:.0}) far={far:.0} h={h:.0} nodes={}",
                                centre,
                                radius,
                                eye.x,
                                eye.z,
                                node_circles.len()
                            ));
                        }
                    }
                }
            }
        }
        println!(
            "{t}: {} articles, {} racing-line pts, {track_checks} eye/item/h checks, {track_divergences} divergences",
            items.len(),
            line.points.len()
        );
        for e in &examples {
            println!("    divergence: {e}");
        }
    }
    println!("\ntotal checks: {total_checks}, total divergences: {total_divergences}");
}
