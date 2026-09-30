//! Debug: the registration of one article by name prefix, step by step.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::fitted_tree::{convex_hull, polygon_rect_overlap, FittedTree, Node};
use nfs_game::world::{cull_articles, tree_population};

fn dump(tree: &FittedTree, n: &Node, hull: &[(f32, f32)], depth: usize, maxd: usize) {
    let (min, max) = tree.bounds(n);
    let kids: Vec<usize> = (0..4).filter(|&q| n.children[q].is_some()).collect();
    let ov: Vec<usize> = (0..4).filter(|&q| n.children[q].as_ref().is_some_and(|c| { let (a, b) = tree.bounds(c); polygon_rect_overlap(hull, a, b) })).collect();
    println!("{}node {} level {} box ({:.0},{:.0})-({:.0},{:.0}) children {:?} overlapped {:?}", "  ".repeat(depth), n.id, n.level, min.0, min.1, max.0, max.1, kids, ov);
    if depth < maxd {
        for q in ov {
            dump(tree, n.children[q].as_deref().unwrap(), hull, depth + 1, maxd);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &args[1]).unwrap();
    let arts = cull_articles(&t);
    let pop = tree_population(&t, &arts);
    println!("population {} points, box {:?} .. {:?}", pop.points.len(), pop.min, pop.max);
    let tree = FittedTree::new(pop.min, pop.max, &pop.points);
    println!("tree min {:?} size {} height {}", tree.min, tree.size, tree.height);
    for (i, a) in arts.iter().enumerate() {
        let Some(a) = a else { continue };
        if !a.name.starts_with(args[2].as_str()) {
            continue;
        }
        let (lo, hi) = (a.footprint_min, a.footprint_max);
        let corners = [(lo.x, lo.z), (hi.x, lo.z), (hi.x, lo.z), (lo.x, lo.z), (lo.x, hi.z), (hi.x, hi.z), (hi.x, hi.z), (lo.x, hi.z)];
        let hull = convex_hull(&corners);
        println!("article {i} {:?} {:?} centre {:?} r {} footprint {:?}..{:?} hull {:?}", a.name, a.drawer, a.centre, a.radius, lo, hi, hull);
        let start = tree.locate(&corners);
        println!("start node {} level {}", start.id, start.level);
        dump(&tree, start, &hull, 0, 6);
        let regs = tree.spread(start, &hull);
        println!("registered {:?}", regs.iter().map(|n| (n.id, n.level)).collect::<Vec<_>>());
        // this article's own vertices
        let arts_raw = t.crp.articles(&t.crp_data).unwrap();
        let art = &arts_raw[i];
        let base = art.level(b"Base").and_then(|e| t.crp.entry_data(&t.crp_data, e).ok()).unwrap();
        let k0 = u32::from_le_bytes([base[8], base[9], base[10], base[11]]) as usize;
        let o = 0x64 + 12 * k0;
        let lod = u16::from_le_bytes([base[o + 8], base[o + 9]]);
        let lods: Vec<u16> = (0..u32::from_le_bytes([base[12], base[13], base[14], base[15]]) as usize).map(|k| u16::from_le_bytes([base[0x64 + 12 * k + 8], base[0x64 + 12 * k + 9]])).collect();
        let vts: Vec<(u8, usize)> = art.parts.iter().filter(|p| p.is_part(b"vt")).map(|p| (p.part_index() as u8, p.data_len() / 16)).collect();
        println!("first LOD {lod}, LOD records {lods:?}, vt parts (index, vertices) {vts:?}");
        break;
    }
}
