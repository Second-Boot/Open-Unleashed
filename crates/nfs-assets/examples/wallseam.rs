//! Diagnostic: the track room's picture wall (`FEData/Models/trackwall.crp`, `Panel1a…12a`):
//! every panel's primitives (material, positions, UVs) and `tr` matrices, to measure the gap
//! between neighbouring panels (FE-WALLSEAM).
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.fe_data().join("Models");
    let car = CarAssets::load_in(&dir, "trackwall").unwrap();
    for a in car.crp.articles(&car.crp_data).unwrap() {
        let Ok(g) = car.crp.article_geometry(&car.crp_data, &a) else { continue };
        println!("== {} base {:?}", g.name, g.base.as_ref().map(|b| (b.tag, b.draw_bytes)));
        for (l, t) in &g.transforms {
            println!("  tr lod {l}: {:?}", t);
        }
        for p in &g.primitives {
            println!("  prim lod {} type {} flags {:#x} mt {} nv {} nuv {} idx {} uvidx {}", p.lod, p.prim_type, p.flags, p.material, p.positions.len(), p.uvs.len(), p.vertex_indices.len(), p.uv_indices.len());
            for (i, v) in p.positions.iter().enumerate() {
                println!("    v{i} {:.5} {:.5} {:.5}", v[0], v[1], v[2]);
            }
            for (i, v) in p.uvs.iter().enumerate() {
                println!("    t{i} {:.5} {:.5}", v[0], v[1]);
            }
            println!("    vi {:?}", p.vertex_indices);
            println!("    ti {:?}", p.uv_indices);
        }
    }
}
