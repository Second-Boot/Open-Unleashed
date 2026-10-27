//! ENHANCEMENT E42 census: for every shipped car the parts `nfs_assets::round` refines (wheels by the article's type, the
//! whitelisted round parts by name) at LOD 1..=4: source triangles -> refined triangles at the two levels.
//!   cargo run -p nfs-assets --example round_census [car ...]
use nfs_assets::{car::CarAssets, mesh::article_meshes, round, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.root.join("GameData/CarModel");
    let mut cars: Vec<String> = std::env::args().skip(1).collect();
    if cars.is_empty() {
        cars = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.unwrap().file_name().into_string().unwrap().strip_suffix(".crp").map(|s| s.to_string())).collect();
        cars.sort();
    }
    let (mut tot_src, mut tot_soft, mut tot_strong) = (0usize, 0usize, 0usize);
    for car in cars {
        let Ok(c) = CarAssets::load(&paths, &car) else { continue };
        let mut line = Vec::new();
        let (mut s0, mut s1, mut s2) = (0usize, 0usize, 0usize);
        for a in c.crp.articles(&c.crp_data).unwrap() {
            let Ok(g) = c.crp.article_geometry(&c.crp_data, &a) else { continue };
            let ty = g.base.as_ref().map(|b| b.tag[0]).unwrap_or(0);
            let is_wheel = matches!(ty, 0x0D | 0x0E);
            let Some(kind) = round::part_kind(&g.name, is_wheel) else { continue };
            for l in 1..=4u8 {
                let mut sk = 0;
                for mm in article_meshes(&g, l, &mut sk) {
                    let src = mm.mesh.indices.len() / 3;
                    let r = |level: u8| -> usize {
                        if level == 1 && l == 4 {
                            return src;
                        }
                        let o = match kind {
                            round::Kind::Wheel => round::refine_wheel(&mm.mesh, if level == 1 { 32 } else { 64 }, if level == 1 { 4 } else { 6 }),
                            round::Kind::Smooth => round::refine_pn(&mm.mesh, if level == 1 { 2 } else { 3 }, 0.9998),
                        };
                        o.map_or(src, |(m, _)| m.indices.len() / 3)
                    };
                    let (a1, a2) = (r(1), r(2));
                    if l == 1 {
                        if let round::Kind::Smooth = kind
                            && let Some((_, rs)) = round::refine_pn(&mm.mesh, 2, 0.9998)
                        {
                            line.push(format!("[{} moves {:.1} mm]", g.name.trim(), rs.moved * 1000.0));
                        }
                        line.push(format!("{}{}:{}->{}/{}", if is_wheel { "W:" } else { "" }, g.name.trim(), src, a1, a2));
                    }
                    s0 += src;
                    s1 += a1;
                    s2 += a2;
                }
            }
        }
        println!("{car}: LOD1-4 {s0} -> {s1} (soft) / {s2} (strong) tris; LOD1 parts: {}", line.join(" "));
        tot_src += s0;
        tot_soft += s1;
        tot_strong += s2;
    }
    println!("TOTAL {tot_src} -> {tot_soft} / {tot_strong}");
}
