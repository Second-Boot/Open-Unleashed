//! Census (/ node pass): how often does the node pass (0x47F400 over the registration of
//! `nfs_game::world::build_article_nodes`) drop an article that is **in view and close** — the
//! eye on the racing line, looking along it (the way a car drives), an article whose bounding
//! sphere passes the frustum and whose near edge is within `near` metres — because every node it
//! was registered into is wholly outside the ground wedge or the far circle? Those are the holes
//! the original's registration would leave beside the road.
#![allow(clippy::approx_constant)] // the original's constant is 0.7071

use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{build_article_nodes, cull_articles, node_wholly_pruned, tree_population, CullView};

// ---- the port's registration until 2026-09-30 (base 29c09d38), for comparison: a tree over one
// centre per article, an AABB overlap, `n = 0` always pushing
struct OldNode {
    children: [Option<Box<OldNode>>; 4],
}
fn quad(min: (f32, f32), max: (f32, f32), q: usize) -> ((f32, f32), (f32, f32)) {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    match q {
        0 => (min, (cx, cz)),
        1 => ((cx, min.1), (max.0, cz)),
        2 => ((min.0, cz), (cx, max.1)),
        _ => ((cx, cz), max),
    }
}
fn old_build(min: (f32, f32), max: (f32, f32), level: u32, items: &[(f32, f32)]) -> OldNode {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    let mut qd: [Vec<(f32, f32)>; 4] = Default::default();
    for &p in items {
        qd[(p.0 >= cx) as usize + 2 * ((p.1 >= cz) as usize)].push(p);
    }
    let mut children: [Option<Box<OldNode>>; 4] = [None, None, None, None];
    for (q, ch) in children.iter_mut().enumerate() {
        if level < 13 && (level <= 1 || qd[q].len() >= 32) {
            let (a, b) = quad(min, max, q);
            *ch = Some(Box::new(old_build(a, b, level + 1, &qd[q])));
        }
    }
    OldNode { children }
}
fn old_locate(n: &OldNode, min: (f32, f32), max: (f32, f32), level: u32, fmin: (f32, f32), fmax: (f32, f32)) -> (&OldNode, (f32, f32), (f32, f32), u32) {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    let q = (((fmin.0 + fmax.0) * 0.5) >= cx) as usize + 2 * ((((fmin.1 + fmax.1) * 0.5) >= cz) as usize);
    if let Some(ch) = &n.children[q] {
        let (a, b) = quad(min, max, q);
        if fmin.0 >= a.0 && fmax.0 <= b.0 && fmin.1 >= a.1 && fmax.1 <= b.1 {
            return old_locate(ch, a, b, level + 1, fmin, fmax);
        }
    }
    (n, min, max, level)
}
#[allow(clippy::too_many_arguments)]
fn old_spread(n: &OldNode, min: (f32, f32), max: (f32, f32), level: u32, weight: u32, spent: u32, fmin: (f32, f32), fmax: (f32, f32), out: &mut Vec<((f32, f32), (f32, f32))>) {
    if n.children.iter().all(Option::is_none) {
        out.push((min, max));
        return;
    }
    let mut ov = [false; 4];
    let mut k = 0;
    for q in 0..4 {
        if n.children[q].is_some() {
            let (a, b) = quad(min, max, q);
            if fmax.0 >= a.0 && fmin.0 <= b.0 && fmax.1 >= a.1 && fmin.1 <= b.1 {
                ov[q] = true;
                k += 1;
            }
        }
    }
    let rec = match k {
        0 => false,
        1 => true,
        2 | 3 => weight + spent < 8,
        _ => level < 2,
    };
    if !rec {
        out.push((min, max));
        return;
    }
    let ns = if k == 1 { spent } else { spent + k * weight };
    for q in 0..4 {
        if ov[q] {
            let (a, b) = quad(min, max, q);
            old_spread(n.children[q].as_ref().unwrap(), a, b, level + 1, weight + 1, ns, fmin, fmax, out);
        }
    }
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = TrackAssets::list(&paths);
    }
    let near = 60.0f32;
    for name in tracks {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let arts = cull_articles(&t);
        let pop = tree_population(&t, &arts);
        let tree = nfs_game::fitted_tree::FittedTree::new(pop.min, pop.max, &pop.points);
        let (nodes, _) = build_article_nodes(&tree, &arts);
        // the old registration
        let cen: Vec<(f32, f32)> = arts.iter().flatten().map(|a| (a.centre.x, a.centre.z)).collect();
        let (mut lo, mut hi) = ((f32::INFINITY, f32::INFINITY), (f32::NEG_INFINITY, f32::NEG_INFINITY));
        for &(x, z) in &cen {
            lo = (lo.0.min(x), lo.1.min(z));
            hi = (hi.0.max(x), hi.1.max(z));
        }
        let (dx, dz) = (hi.0 - lo.0, hi.1 - lo.1);
        let l = dx.max(dz).max(1.0);
        let rmin = (lo.0 - (l - dx) * 0.5, lo.1 - (l - dz) * 0.5);
        let rmax = (rmin.0 + l, rmin.1 + l);
        let old_tree = old_build(rmin, rmax, 0, &cen);
        let old_nodes: Vec<Vec<(f32, f32, f32)>> = arts
            .iter()
            .map(|a| {
                let Some(a) = a else { return Vec::new() };
                let (fmin, fmax) = ((a.footprint_min.x, a.footprint_min.z), (a.footprint_max.x, a.footprint_max.z));
                let (st, smin, smax, sl) = old_locate(&old_tree, rmin, rmax, 0, fmin, fmax);
                let mut out = Vec::new();
                old_spread(st, smin, smax, sl, 1, 0, fmin, fmax, &mut out);
                out.into_iter().map(|(a, b)| (((a.0 + b.0) * 0.5), ((a.1 + b.1) * 0.5), 0.7071 * (b.0 - a.0))).collect()
            })
            .collect();
        let mut old_drops = 0u64;
        let Some(line) = t.racing_line(&paths, false) else { continue };
        let n = line.points.len();
        if n < 4 {
            continue;
        }
        let (mut frames, mut frames_with_drop, mut drops, mut checked) = (0u64, 0u64, 0u64, 0u64);
        let mut example = String::new();
        for i in (0..n).step_by(2) {
            let p = line.points[i];
            let q = line.points[(i + 3) % n];
            let fwd = Vec3::new(q[0] - p[0], 0.0, q[1] - p[1]).normalize_or_zero();
            if fwd == Vec3::ZERO {
                continue;
            }
            // eye height: the article's own centre is far above; use the sphere's height so only the
            // ground plane distance matters (the node pass is xz only)
            let right = fwd.cross(Vec3::Y).normalize_or_zero();
            let slice = (i * 399 / n.max(1)).min(399);
            let far = t.draw_keys_forward.far_distance(slice, 1.0);
            let mut any = false;
            frames += 1;
            for ((a, ns), old) in arts.iter().zip(&nodes).zip(&old_nodes) {
                let Some(a) = a else { continue };
                if ns.is_empty() {
                    continue;
                }
                let eye = Vec3::new(p[0], a.centre.y.min(5.0).max(0.0), p[1]);
                let view = CullView { eye, forward: fwd, right, up: right.cross(fwd).normalize_or_zero(), tan_h: 0.92, tan_v: 0.69, near: 0.5, draw: far };
                let d = ((a.centre.x - eye.x).powi(2) + (a.centre.z - eye.z).powi(2)).sqrt();
                if d - a.radius > near || !view.sphere_visible(a.centre, a.radius) {
                    continue;
                }
                checked += 1;
                if !old.is_empty() && old.iter().all(|&(cx, cz, r)| node_wholly_pruned(&view, cx, cz, r)) {
                    old_drops += 1;
                }
                if ns.iter().all(|&(_, cx, cz, r)| node_wholly_pruned(&view, cx, cz, r)) {
                    drops += 1;
                    any = true;
                    if example.is_empty() {
                        example = format!("line point {i} eye ({:.0},{:.0}) article {:?} {:?} centre ({:.0},{:.0}) r {:.0} nodes {}", p[0], p[1], a.name, a.drawer, a.centre.x, a.centre.z, a.radius, ns.len());
                    }
                }
            }
            if any {
                frames_with_drop += 1;
            }
        }
        println!("{name}: {frames} eyes along the line, {checked} (eye, article) pairs in view within {near} m, {drops} dropped by the node pass (the registration until 2026-09-30: {old_drops}); {frames_with_drop} eyes lose at least one. {example}");
    }
}
