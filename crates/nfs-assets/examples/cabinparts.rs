//! Diagnostic: the parts of a car per LOD with their draw bytes (mask `Base+78`, inside `+81`, inside-only `+82`)
//! and every primitive's vertex count / material / flags — the cabin passes of the car draw 0x444E00 pick from these.
//! Usage: `cabinparts <car> [name-substring]`.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let c = CarAssets::load(&paths, &a[0]).unwrap();
    let filter = a.get(1).cloned().unwrap_or_default();
    let mats = c.crp.materials(&c.crp_data).unwrap();
    for lod in 1u8..=4 {
        println!("== LOD {lod}");
        for ar in c.crp.articles(&c.crp_data).unwrap() {
            let Ok(g) = c.crp.article_geometry(&c.crp_data, &ar) else { continue };
            if !g.name.contains(&filter) {
                continue;
            }
            let prims: Vec<_> = g.primitives.iter().filter(|p| p.lod == lod).collect();
            if prims.is_empty() {
                continue;
            }
            let Some(b) = g.base.as_ref() else { continue };
            let list: Vec<String> = prims
                .iter()
                .map(|p| {
                    let m = mats.get(p.material as usize);
                    format!("{}v/m{}/f{:#x}[{} rs{:?}]", p.positions.len(), p.material, p.flags, m.map_or("", |m| m.render_method.as_str()), m.and_then(|m| m.rs))
                })
                .collect();
            if std::env::var_os("TRIS").is_some() {
                for p in &prims {
                    println!("   prim m{} type {} idx {} uvs {} cols {}", p.material, p.prim_type, p.vertex_indices.len(), p.uvs.len(), p.colors.len());
                    for t in 0..p.vertex_indices.len() / 3 {
                        let s: Vec<String> = (0..3)
                            .map(|k| {
                                let i = t * 3 + k;
                                let v = p.positions[p.vertex_indices[i] as usize];
                                let uv = p.uv_indices.get(i).and_then(|&u| p.uvs.get(u as usize)).copied().unwrap_or([f32::NAN; 2]);
                                format!("({:.3},{:.3},{:.3} uv {:.2},{:.2} c{:08x})", v[0], v[1], v[2], uv[0], uv[1], p.colors.get(i).copied().unwrap_or(0))
                            })
                            .collect();
                        println!("      {}", s.join(" "));
                    }
                }
            }
            if std::env::var_os("VERTS").is_some() {
                for p in &prims {
                    println!("   prim m{} type {} idx {} verts:", p.material, p.prim_type, p.vertex_indices.len());
                    for v in &p.positions {
                        println!("      {:.3} {:.3} {:.3}", v[0], v[1], v[2]);
                    }
                }
            }
            let recs: Vec<String> = b.lods.iter().map(|l| format!("[{}..{} lod{}]", l.first_pr, l.last_pr, l.lod)).collect();
            println!("{:<22} mask {:#04x} in {} inonly {}  {}  recs {}", g.name, b.draw_bytes[0], b.draw_bytes[3], b.draw_bytes[4], list.join(" "), recs.join(""));
        }
    }
}
