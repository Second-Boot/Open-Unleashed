//! Census for `RE/notes/particles.md` §10.1 / `COVERAGE-2026-09-26.md` §14.32.2's open MEDIUM:
//! does any shipped track's fitted tree (`0x47BE30`'s split rule — a quadrant gets a child only
//! when the node's level is under 13 **and** (level ≤ 1, unconditionally, **or** the quadrant
//! holds 32 or more item centres)) ever produce a node where *some but not all four* quadrants
//! split — the "coarsening" case that makes `WLineBundle`'s skid-mark cells several sizes rather
//! than the port's fixed `L/2^k` squares (`particles.md` §0/§10, `0x479810`'s "has all four" read
//! test)?
//!
//! Items are one per placed world article and one per `.scn` GEOM instance — the same population
//! `TrackGeometry::collision_grid` turns into collision triangles (`particles.md` §0's own
//! APPROXIMATION: "the world's box is the port's collision triangles' box... the same items
//! `0x47B660` walks, only their vertices"). An item's centre is its own transformed mesh AABB's
//! centre in the xz plane — `0x47BE30` takes "a vector of pointers to the items' centres"
//! (`RE/RENDERING.md` "The node pass 0x47F400 and the fitted tree's split 0x47BE30").
//!
//! Root box: the same square the port's skid grid already uses (`nfs_game::skidmarks::for_world`'s
//! own box, `L = max(Δx, Δz)` of the world's box, centred on it) — read-only measurement, no game
//! state is touched.

use nfs_assets::{track::TrackAssets, GamePaths};

/// The world box's fitted-tree root — the same square `nfs_game::skidmarks::for_world` derives
/// (`particles.md` §0: `L = max(Δx, Δz)`, centred).
fn root_box(items: &[(f32, f32)]) -> ((f32, f32), (f32, f32)) {
    let (mut min, mut max) = ((f32::INFINITY, f32::INFINITY), (f32::NEG_INFINITY, f32::NEG_INFINITY));
    for &(x, z) in items {
        min = (min.0.min(x), min.1.min(z));
        max = (max.0.max(x), max.1.max(z));
    }
    let (dx, dz) = (max.0 - min.0, max.1 - min.1);
    let l = dx.max(dz);
    let cx0 = min.0 - (l - dx) * 0.5;
    let cz0 = min.1 - (l - dz) * 0.5;
    ((cx0, cz0), (cx0 + l, cz0 + l))
}

/// `0x47BE30`, read to the instruction (`RE/RENDERING.md`, `COVERAGE-2026-09-26.md` §14.32.1): a
/// node's own set is every item handed to it; a quadrant becomes a child only when `level < 13`
/// and (`level <= 1` or the quadrant holds `>= 32` item centres). A quadrant that fails gets no
/// child at all — those items stay represented only by this node.
struct Node {
    children: [Option<Box<Node>>; 4],
    /// items this node itself was built from (its own undivided set)
    count: usize,
    level: u32,
}

fn quadrant_of(min: (f32, f32), max: (f32, f32), q: usize) -> ((f32, f32), (f32, f32)) {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    match q {
        0 => (min, (cx, cz)),
        1 => ((cx, min.1), (max.0, cz)),
        2 => ((min.0, cz), (cx, max.1)),
        _ => ((cx, cz), max),
    }
}

fn build(min: (f32, f32), max: (f32, f32), level: u32, items: &[(f32, f32)]) -> Node {
    let (cx, cz) = ((min.0 + max.0) * 0.5, (min.1 + max.1) * 0.5);
    let mut quad: [Vec<(f32, f32)>; 4] = Default::default();
    for &p in items {
        let q = (p.0 >= cx) as usize + 2 * ((p.1 >= cz) as usize);
        quad[q].push(p);
    }
    let mut children: [Option<Box<Node>>; 4] = [None, None, None, None];
    for (q, ch) in children.iter_mut().enumerate() {
        let split = level < 13 && (level <= 1 || quad[q].len() >= 32);
        if split {
            let (qmin, qmax) = quadrant_of(min, max, q);
            *ch = Some(Box::new(build(qmin, qmax, level + 1, &quad[q])));
        }
    }
    Node { children, count: items.len(), level }
}

/// Walks the tree, counting nodes whose split is *partial* — some but not all four quadrants got a
/// child — the coarsening event `particles.md` §10.1 asks whether any shipped track ever produces.
struct Census {
    nodes: usize,
    full_split: usize,
    partial_split: usize,
    natural_leaf: usize,
    max_level_reached: u32,
    max_item_count: usize,
    partial_examples: Vec<(u32, usize, [usize; 4])>,
}

fn walk(node: &Node, min: (f32, f32), max: (f32, f32), c: &mut Census) {
    c.nodes += 1;
    c.max_level_reached = c.max_level_reached.max(node.level);
    c.max_item_count = c.max_item_count.max(node.count);
    let n_children = node.children.iter().filter(|c| c.is_some()).count();
    match n_children {
        4 => c.full_split += 1,
        0 => c.natural_leaf += 1,
        _ => {
            c.partial_split += 1;
            if c.partial_examples.len() < 5 {
                let counts: [usize; 4] = std::array::from_fn(|q| {
                    let (qmin, qmax) = quadrant_of(min, max, q);
                    let _ = (qmin, qmax);
                    node.children[q].as_ref().map(|b| b.count).unwrap_or(0)
                });
                c.partial_examples.push((node.level, node.count, counts));
            }
        }
    }
    for (q, ch) in node.children.iter().enumerate() {
        if let Some(ch) = ch {
            let (qmin, qmax) = quadrant_of(min, max, q);
            walk(ch, qmin, qmax, c);
        }
    }
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = TrackAssets::list(&paths);
    }
    let mut any_partial_anywhere = false;
    for t in &tracks {
        let Ok(track) = TrackAssets::load(&paths, t) else { continue };
        let Ok(geometry) = track.build_geometry() else { continue };
        let items = geometry.item_centres();
        if items.is_empty() {
            println!("{t}: no items");
            continue;
        }
        let (min, max) = root_box(&items);
        let root = build(min, max, 0, &items);
        let mut c = Census { nodes: 0, full_split: 0, partial_split: 0, natural_leaf: 0, max_level_reached: 0, max_item_count: 0, partial_examples: Vec::new() };
        walk(&root, min, max, &mut c);
        any_partial_anywhere |= c.partial_split > 0;
        println!(
            "{t}: {} items, root {:.0}m, {} nodes ({} full-split, {} partial-split, {} natural leaf), max level {}, max node item count {}",
            items.len(),
            (max.0 - min.0),
            c.nodes,
            c.full_split,
            c.partial_split,
            c.natural_leaf,
            c.max_level_reached,
            c.max_item_count,
        );
        for (level, count, quads) in &c.partial_examples {
            println!("    partial-split example: level {level}, node had {count} items, quadrants {quads:?}");
        }
    }
    println!("\nany track with a partial-split (coarsening) node: {any_partial_anywhere}");
}
