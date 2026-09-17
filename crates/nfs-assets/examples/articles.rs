//! Diagnostic: the articles of a car model at one LOD — name, type/variant and whether the port
//! treats them as wheels.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let c = CarAssets::load(&paths, &a[0]).unwrap();
    let lod: u8 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    for a in c.crp.articles(&c.crp_data).unwrap() {
        let Ok(g) = c.crp.article_geometry(&c.crp_data, &a) else { continue };
        let (ty, var) = g.base.as_ref().map(|b| (b.tag[0], b.tag[1])).unwrap_or((0, 0));
        if !g.primitives.iter().any(|p| p.lod == lod) {
            continue;
        }
        let mut skipped = 0;
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for mm in nfs_assets::mesh::article_meshes(&g, lod, &mut skipped) {
            for v in &mm.mesh.vertices {
                for k in 0..3 {
                    lo[k] = lo[k].min(v.position[k]);
                    hi[k] = hi[k].max(v.position[k]);
                }
            }
        }
        let tr = g.transform_for_lod(lod).map(|m| [m[12], m[13], m[14]]).unwrap_or([0.0; 3]);
        println!(
            "{:<22} type {ty:#04x}/{var} wheel={} tr=({:.3},{:.3},{:.3}) bbox y {:.3}..{:.3}",
            &g.name,
            matches!(ty, 0x0D | 0x0E),
            tr[0], tr[1], tr[2],
            lo[1], hi[1]
        );
    }
}
