//! Diagnostic: which CRP primitives the mesh builder cannot turn into triangles, across every
//! shipped track and car (`RE/notes/crp-geometry.md` §4).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    let mut kinds: BTreeMap<(u16, bool), usize> = BTreeMap::new();
    let mut names: BTreeMap<u16, Vec<String>> = BTreeMap::new();
    let mut total = 0usize;
    let mut skipped_by_builder = 0usize;
    let mut skipped_verts = 0usize;
    let mut both = 0usize;
    let mut type1_arts = 0usize;
    let mut counts1: BTreeMap<usize, usize> = BTreeMap::new();
    let mut rest: BTreeMap<(u16, bool, usize), usize> = BTreeMap::new();
    let mut pairs: BTreeMap<(u16, usize, usize, usize), usize> = BTreeMap::new();
    let mut examples: Vec<String> = Vec::new();
    let mut seqshape: BTreeMap<(u16, bool, bool, bool), usize> = BTreeMap::new();
    for t in &tables.tracks {
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        let Ok(arts) = a.crp.articles(&a.crp_data) else { continue };
        for art in &arts {
            let Ok(art) = a.crp.article_geometry(&a.crp_data, art) else { continue };
            for lod in 0..4u8 {
                let has1 = art.primitives.iter().any(|p| p.lod == lod && p.prim_type == 1);
                if has1 {
                    type1_arts += 1;
                    if art.primitives.iter().any(|p| p.lod == lod && p.prim_type != 1) {
                        both += 1;
                    }
                }
            }
            let mut sk = 0;
            for lod in 0..4u8 {
                let before = sk;
                let _ = nfs_assets::mesh::article_meshes(&art, lod, &mut sk);
                if sk > before {
                    for p in art.primitives.iter().filter(|p| p.lod == lod) {
                        let ok = match (p.prim_type, p.vertex_indices.is_empty()) {
                            (3, false) | (1, false) => true,
                            (3, true) => p.positions.len() % 3 == 0 && p.positions.len() <= 256,
                            (4, true) => p.positions.len() % 4 == 0 && p.positions.len() <= 256,
                            _ => false,
                        };
                        if !ok {
                            *rest.entry((p.prim_type, p.vertex_indices.is_empty(), p.positions.len())).or_default() += 1;
                            *pairs.entry((p.prim_type, p.positions.len(), p.colors.len(), p.uvs.len())).or_default() += 1;
                            if p.prim_type == 3 && p.positions.len() == 8 && p.colors.len() == 12 && examples.len() < 4 {
                                examples.push(format!("{} {}", t.file, art.name));
                            }
                        }
                    }
                }
            }
            skipped_by_builder += sk;
            for p in &art.primitives {
                total += 1;
                *kinds.entry((p.prim_type, p.vertex_indices.is_empty())).or_default() += 1;
                if p.vertex_indices.is_empty() {
                    *seqshape.entry((p.prim_type, p.positions.len() == p.colors.len(), p.colors.len() % 3 == 0, p.colors.len() % 4 == 0)).or_default() += 1;
                }
                if p.prim_type == 1 {
                    skipped_verts += p.positions.len();
                    *counts1.entry(p.colors.len()).or_default() += 1;
                }
                if p.prim_type != 3 {
                    let e = names.entry(p.prim_type).or_default();
                    let n = format!("{}/{} lod{} {}v", t.file, art.name, p.lod, p.positions.len());
                    if !e.contains(&n) {
                        e.push(n);
                    }
                }
            }
        }
    }
    // cars as well
    let mut car_skipped = 0usize;
    let mut car_prims = 0usize;
    if let Ok(rd) = std::fs::read_dir(paths.car_model_dir()) {
        let mut names: Vec<String> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("crp")).unwrap_or(false))
            .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
            .collect();
        names.sort();
        for n in &names {
            let Ok(c) = nfs_assets::car::CarAssets::load(&paths, n) else { continue };
            let Ok(arts) = c.crp.articles(&c.crp_data) else { continue };
            for art in &arts {
                let Ok(g) = c.crp.article_geometry(&c.crp_data, art) else { continue };
                car_prims += g.primitives.len();
                for lod in 0..4u8 {
                    let _ = nfs_assets::mesh::article_meshes(&g, lod, &mut car_skipped);
                }
            }
        }
        println!("{car_prims} primitives in {} cars, {car_skipped} skipped", names.len());
    }
    println!("{total} primitives in {} tracks", tables.tracks.len());
    println!("skipped by the mesh builder: {skipped_by_builder} primitives, {skipped_verts} vertices");
    println!("type-1 articles that also carry a drawn primitive at the same LOD: {both} of {type1_arts}");
    for ((k, seq), n) in &kinds {
        println!("  type {k} {} ×{n}", if *seq { "without index arrays" } else { "indexed          " });
    }
    let mut c1: BTreeMap<usize, usize> = BTreeMap::new();
    for (c, n) in &counts1 {
        *c1.entry(*c).or_default() += n;
    }
    let mut v: Vec<_> = c1.iter().collect();
    v.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!("  type-1 corner counts (df entries): {:?}", v.iter().take(16).map(|(c, n)| format!("{c}×{n}")).collect::<Vec<_>>());
    println!("  of them even {} / multiple of 3 {} / multiple of 4 {}",
        c1.iter().filter(|(c, _)| *c % 2 == 0).map(|(_, n)| n).sum::<usize>(),
        c1.iter().filter(|(c, _)| *c % 3 == 0).map(|(_, n)| n).sum::<usize>(),
        c1.iter().filter(|(c, _)| *c % 4 == 0).map(|(_, n)| n).sum::<usize>());
    let mut r: Vec<_> = rest.iter().collect();
    r.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!("  still skipped: {:?}", r.iter().take(14).map(|((t, seq, n), c)| format!("type{t}{} {n}v ×{c}", if *seq { " seq" } else { " idx" })).collect::<Vec<_>>());
    let mut q: Vec<_> = pairs.iter().collect();
    q.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    println!("  skipped, positions/colours/uvs: {:?}", q.iter().take(14).map(|((t, v, c, u), n)| format!("type{t} {v}v {c}c {u}uv ×{n}")).collect::<Vec<_>>());
    println!("  non-indexed shapes (type, verts==cols, cols%3, cols%4): {:?}", seqshape.iter().map(|(k, n)| format!("{k:?}×{n}")).collect::<Vec<_>>());
    for (k, v) in &names {
        println!("  type {k}: {:?}", v.iter().take(14).collect::<Vec<_>>());
    }
}
