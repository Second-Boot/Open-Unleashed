//! Diagnostic (ENH-LBEAM): the translucent-sheet articles (alpha under one half) of every track with their geometry.
use nfs_assets::{track::TrackAssets, GamePaths};
fn main() {
    let paths = GamePaths::locate().unwrap();
    let only: Vec<String> = std::env::args().skip(1).collect();
    for name in TrackAssets::list(&paths) {
        if !only.is_empty() && !only.contains(&name) {
            continue;
        }
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let mats = t.materials().unwrap();
        let g = t.build_geometry().unwrap();
        for art in &g.world {
            for (lod, meshes) in art.lods.iter().take(1) {
                for mm in meshes {
                    let Some(m) = mats.get(mm.material as usize) else { continue };
                    let Some(tx) = t.textures.iter().find(|x| x.name == m.texture_name()) else { continue };
                    let Some(px) = tx.mips.first() else { continue };
                    let any = px.chunks_exact(4).any(|p| p[3] > 10);
                    let all = px.chunks_exact(4).all(|p| p[3] < 128);
                    if !(tx.has_alpha && any && all) {
                        continue;
                    }
                    if tx.name.starts_with("wtr") {
                        continue;
                    }
                    let n = mm.mesh.vertices.len();
                    println!("{name}: art {} {:?} lod {lod} tex {} rm {} verts {n} idx {} anim {:?} tag {:#x}", art.index, art.name, tx.name, m.render_method, mm.mesh.indices.len(), art.animation.as_ref().map(|a| a.frames.len()), art.tag);
                    println!("  texture {}x{}", tx.width, tx.height);
                    let (w, h) = (tx.width as usize, tx.height as usize);
                    for gy in 0..16 {
                        let mut line = String::new();
                        for gx in 0..16 {
                            let p = &px[((gy * h / 16) * w + gx * w / 16) * 4..][..4];
                            line += &format!("{:3}/{:3},{:3},{:3} ", p[3], p[0], p[1], p[2]);
                        }
                        println!("  {line}");
                    }
                    println!("  transform {:?}", art.transform);
                    if n < 40 {
                        for v in &mm.mesh.vertices {
                            println!("  v {:?} uv {:?}", v.position, v.uv);
                        }
                        println!("  idx {:?}", mm.mesh.indices);
                    }
                    if let Some(a) = &art.animation {
                        for f in a.frames.iter().take(3) {
                            println!("  frame {:?}", f);
                        }
                    }
                }
            }
        }
    }
}
