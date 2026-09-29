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
//! **2026-09-30 update (`COVERAGE-2026-09-26.md`
//! §14.36):** this now runs the *real* multi-node registration — `0x47BDC0`'s starting-node search
//! plus `0x489930`'s budgeted footprint spread (`locate`/`spread`, mirroring
//! `nfs_game::world::cull_tree_locate`/`spread_nodes`) fed the article's *real* footprint box
//! (`nfs_game::world::CullArticle::footprint_min`/`footprint_max` — `Base`'s own `bbox_min`/
//! `bbox_max`, `0x477F40`, the `MidGround` `×2.25` scale folded in, `0x47D010`; **not** a
//! sphere-derived square any more) instead of the single centre-node stand-in the previous pass
//! used, and an article is pruned only when *every* one of its registered nodes is wholly pruned.
//! Reproducing `NNN`'s own `alps 993 --autotest 900` regression with this — the real spread, the
//! real box — still loses the same road-side scenery: the coverage gap is not the footprint-shape
//! approximation, it is `0x489930`'s own recursion silently skipping a geometrically-overlapping
//! sibling quadrant that has no child (never split, `0x47BE30`'s 32-item threshold) — see
//! `nfs_game::world::NODE_PRUNE`'s own doc for the full, to-the-instruction account. The switch
//! stays off. `h` is swept over a band around the documented default half-angle (`race-logic.md`
//! §11, `SetSlotView`'s 37.5°/aspect) since its exact value was not traced to the instruction.

use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::cull_articles;

#[derive(Clone, Copy, Debug)]
struct Box2 {
    min: (f32, f32),
    max: (f32, f32),
}

fn quadrant_of(b: Box2, q: usize) -> Box2 {
    let (cx, cz) = ((b.min.0 + b.max.0) * 0.5, (b.min.1 + b.max.1) * 0.5);
    match q {
        0 => Box2 { min: b.min, max: (cx, cz) },
        1 => Box2 { min: (cx, b.min.1), max: (b.max.0, cz) },
        2 => Box2 { min: (b.min.0, cz), max: (cx, b.max.1) },
        _ => Box2 { min: (cx, cz), max: b.max },
    }
}

/// `0x47BE30`'s split rule verbatim (`skid_tree_census.rs`, `RE/RENDERING.md`,
/// `COVERAGE-2026-09-26.md` §14.32.1): a quadrant gets a child only when the node's level is under
/// 13 and (level <= 1, unconditionally, or the quadrant holds 32 or more item centres).
struct Node {
    children: [Option<Box<Node>>; 4],
}

fn build(b: Box2, level: u32, items: &[(f32, f32)]) -> Node {
    let (cx, cz) = ((b.min.0 + b.max.0) * 0.5, (b.min.1 + b.max.1) * 0.5);
    let mut quad: [Vec<(f32, f32)>; 4] = Default::default();
    for &p in items {
        let q = (p.0 >= cx) as usize + 2 * ((p.1 >= cz) as usize);
        quad[q].push(p);
    }
    let mut children: [Option<Box<Node>>; 4] = [None, None, None, None];
    for (q, ch) in children.iter_mut().enumerate() {
        let split = level < 13 && (level <= 1 || quad[q].len() >= 32);
        if split {
            *ch = Some(Box::new(build(quadrant_of(b, q), level + 1, &quad[q])));
        }
    }
    Node { children }
}

/// `0x47BDC0`: the smallest node's own fixed box that still fully holds the footprint box `f`,
/// descending while the node is internal and exactly one child fully contains it (mirrors
/// `nfs_game::world::cull_tree_locate`).
fn locate(node: &Node, b: Box2, level: u32, f: Box2) -> (&Node, Box2, u32) {
    let (cx, cz) = ((b.min.0 + b.max.0) * 0.5, (b.min.1 + b.max.1) * 0.5);
    let (fcx, fcz) = ((f.min.0 + f.max.0) * 0.5, (f.min.1 + f.max.1) * 0.5);
    let q = (fcx >= cx) as usize + 2 * ((fcz >= cz) as usize);
    if let Some(ch) = &node.children[q] {
        let qb = quadrant_of(b, q);
        if f.min.0 >= qb.min.0 && f.max.0 <= qb.max.0 && f.min.1 >= qb.min.1 && f.max.1 <= qb.max.1 {
            return locate(ch, qb, level + 1, f);
        }
    }
    (node, b, level)
}

fn footprint_overlaps(f: Box2, b: Box2) -> bool {
    f.max.0 >= b.min.0 && f.min.0 <= b.max.0 && f.max.1 >= b.min.1 && f.min.1 <= b.max.1
}

/// `0x489930`: the budgeted footprint spread from `0x47BDC0`'s starting node (mirrors
/// `nfs_game::world::spread_nodes` — see that function's doc for the exact `n`/weight/`spent`
/// rule read to the instruction).
fn spread(node: &Node, b: Box2, level: u32, weight: u32, spent: u32, f: Box2, out: &mut Vec<Box2>) {
    let is_internal = node.children.iter().any(Option::is_some);
    if !is_internal {
        out.push(b);
        return;
    }
    let mut overlaps = [false; 4];
    let mut n: u32 = 0;
    for (q, ov) in overlaps.iter_mut().enumerate() {
        if node.children[q].is_some() {
            let qb = quadrant_of(b, q);
            if footprint_overlaps(f, qb) {
                *ov = true;
                n += 1;
            }
        }
    }
    let recurse = match n {
        0 => false,
        1 => true,
        2 | 3 => weight + spent < 8,
        _ => level < 2,
    };
    if !recurse {
        out.push(b);
        return;
    }
    let new_spent = if n == 1 { spent } else { spent + n * weight };
    for (q, &ov) in overlaps.iter().enumerate() {
        if ov {
            spread(node.children[q].as_ref().unwrap(), quadrant_of(b, q), level + 1, weight + 1, new_spent, f, out);
        }
    }
}

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
        let items: Vec<(Vec3, f32, Box2)> = arts
            .iter()
            .flatten()
            .map(|a| (a.centre, a.radius, Box2 { min: (a.footprint_min.x, a.footprint_min.z), max: (a.footprint_max.x, a.footprint_max.z) }))
            .collect();
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

        let centres_xz: Vec<(f32, f32)> = items.iter().map(|(c, _, _)| (c.x, c.z)).collect();
        let (mut min, mut max) = ((f32::INFINITY, f32::INFINITY), (f32::NEG_INFINITY, f32::NEG_INFINITY));
        for &(x, z) in &centres_xz {
            min = (min.0.min(x), min.1.min(z));
            max = (max.0.max(x), max.1.max(z));
        }
        let (dx, dz) = (max.0 - min.0, max.1 - min.1);
        let l = dx.max(dz).max(1.0);
        let root = Box2 { min: (min.0 - (l - dx) * 0.5, min.1 - (l - dz) * 0.5), max: (0.0, 0.0) };
        let root = Box2 { min: root.min, max: (root.min.0 + l, root.min.1 + l) };
        let tree = build(root, 0, &centres_xz);

        let mut track_checks = 0u64;
        let mut track_divergences = 0u64;
        let mut examples: Vec<String> = Vec::new();
        let n_pts = line.points.len();

        for (item_idx, (centre, radius, fbox)) in items.iter().enumerate() {
            let (start, sbox, slevel) = locate(&tree, root, 0, *fbox);
            let mut boxes = Vec::new();
            spread(start, sbox, slevel, 1, 0, *fbox, &mut boxes);
            let node_circles: Vec<((f32, f32), f32)> = boxes
                .iter()
                .map(|b| (((b.min.0 + b.max.0) * 0.5, (b.min.1 + b.max.1) * 0.5), 0.7071 * (b.max.0 - b.min.0)))
                .collect();

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
