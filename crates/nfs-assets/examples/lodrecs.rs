//! Diagnostic: per LOD, the articles a `Base` LOD record lists (what `R3DCarLOD` 0x43C1B0 puts in the object) against the
//! articles that have primitives tagged with that LOD (what the port draws from). Usage: `lodrecs <car>`.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let c = CarAssets::load(&paths, &a[0]).unwrap();
    for lod in 1u16..=7 {
        let (mut by_rec, mut by_prim, mut rec_only) = (0, 0, Vec::new());
        for ar in c.crp.articles(&c.crp_data).unwrap() {
            let Ok(g) = c.crp.article_geometry(&c.crp_data, &ar) else { continue };
            let Some(b) = g.base.as_ref() else { continue };
            let rec = b.lods.iter().any(|l| l.lod == lod);
            let prim = g.primitives.iter().any(|p| p.lod as u16 == lod);
            by_rec += rec as u32;
            by_prim += prim as u32;
            if rec && !prim {
                rec_only.push(format!("{}(mask {:#x})", g.name, b.draw_bytes[0]));
            }
        }
        println!("LOD {lod}: records {by_rec}, prims {by_prim}, record without prims: {}", rec_only.join(" "));
    }
}
