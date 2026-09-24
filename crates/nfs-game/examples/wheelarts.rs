//! Diagnostic: a car's wheel-typed articles (`Base` tag 0x0D / 0x0E) — name, rotation channel,
//! and per LOD the primitives' count and render methods.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let model = std::env::args().nth(1).unwrap_or_else(|| "993".into());
    let paths = GamePaths::locate().unwrap();
    let car = CarAssets::load(&paths, &model).unwrap();
    let mats = car.crp.materials(&car.crp_data).unwrap();
    for a in car.crp.articles(&car.crp_data).unwrap() {
        let Ok(g) = car.crp.article_geometry(&car.crp_data, &a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if !matches!(b.tag[0], 0x0D | 0x0E) {
            continue;
        }
        let mut by_lod = std::collections::BTreeMap::<u8, Vec<String>>::new();
        for p in &g.primitives {
            let rm = mats.get(p.material as usize).map(|m| format!("{}:{}", m.render_method, m.texture_name())).unwrap_or_default();
            by_lod.entry(p.lod).or_default().push(rm);
        }
        println!("{} tag {:02x?} chan {} lods {:?}", g.name, b.tag, b.rot_channel, by_lod.iter().map(|(l, v)| (*l, v.len())).collect::<Vec<_>>());
        if std::env::args().nth(2).is_some_and(|n| n == g.name) {
            println!("  tr lod1 {:?}", g.transform_for_lod(1));
            for p in g.primitives.iter().filter(|p| p.lod == 1) {
                let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
                for q in &p.positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(q[k]);
                        hi[k] = hi[k].max(q[k]);
                    }
                }
                let xs: std::collections::BTreeSet<i32> = p.positions.iter().map(|q| (q[0] * 1000.0).round() as i32).collect();
                let m = mats.get(p.material as usize);
                // per face plane (all three corners on one x), the UV box in page texels
                let mut faces = std::collections::BTreeMap::<i32, ([f32; 2], [f32; 2], usize)>::new();
                for t in 0..p.vertex_indices.len() / 3 {
                    let vi: Vec<usize> = (0..3).map(|k| p.vertex_indices[3 * t + k] as usize).collect();
                    let xs: Vec<i32> = vi.iter().map(|&i| (p.positions[i][0] * 1000.0).round() as i32).collect();
                    if xs[0] != xs[1] || xs[1] != xs[2] {
                        continue;
                    }
                    let e = faces.entry(xs[0]).or_insert(([f32::MAX; 2], [f32::MIN; 2], 0));
                    for k in 0..3 {
                        let uv = p.uvs[p.uv_indices[3 * t + k] as usize];
                        for c in 0..2 {
                            e.0[c] = e.0[c].min(uv[c] * 128.0);
                            e.1[c] = e.1[c].max(uv[c] * 128.0);
                        }
                    }
                    e.2 += 1;
                }
                println!("  faces (x mm → uv box px, tris): {faces:?}");
                println!("  pr type {} flags {:#x} mat {} {:?} tex {:?} verts {} tris {} x-levels(mm) {:?} lo {:?} hi {:?}", p.prim_type, p.flags, p.material, m.map(|m| &m.render_method), m.map(|m| m.texture_name()), p.positions.len(), p.vertex_indices.len() / 3, xs, lo, hi);
            }
        }
    }
}
