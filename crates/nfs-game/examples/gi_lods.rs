//! Diagnostic (E32): a track's world articles per LOD position / number — articles, triangles, vertices.
use nfs_assets::{track::TrackAssets, GamePaths};
fn main() {
    let name = std::env::args().nth(1).expect("track");
    let t = TrackAssets::load(&GamePaths::locate().expect("game data"), &name).expect("track");
    let g = t.build_geometry().expect("geometry");
    let mut by = std::collections::BTreeMap::new();
    let mut multi = 0;
    for a in &g.world {
        if a.lods.len() > 1 {
            multi += 1;
        }
        for (pos, (lod, m)) in a.lods.iter().enumerate() {
            let e = by.entry((pos, *lod)).or_insert((0usize, 0usize, 0usize));
            e.0 += 1;
            e.1 += m.iter().map(|x| x.mesh.indices.len() / 3).sum::<usize>();
            e.2 += m.iter().map(|x| x.mesh.vertices.len()).sum::<usize>();
        }
    }
    println!("articles {} multi-lod {}", g.world.len(), multi);
    for ((p, l), (n, tr, v)) in by {
        println!("pos {p} lod {l}: articles {n} tris {tr} verts {v}");
    }
}
