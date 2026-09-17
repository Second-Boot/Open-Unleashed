//! Diagnostic: is a CRP type-1 primitive a triangle strip or a triangle fan? Builds both from every
//! type-1 primitive in the shipped tracks and scores them by how consistent the face normals are and
//! how large the triangles come out (`RE/notes/crp-geometry.md` §4).
use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};

fn tri_normal(a: Vec3, b: Vec3, c: Vec3) -> Option<Vec3> {
    let n = (b - a).cross(c - a);
    (n.length() > 1e-6).then(|| n.normalize())
}

/// mean |dot| of consecutive face normals, mean area, and the share of degenerate triangles
fn score(tris: &[[Vec3; 3]]) -> (f32, f32, f32) {
    let mut normals = Vec::new();
    let mut area = 0.0;
    let mut degen = 0;
    for t in tris {
        match tri_normal(t[0], t[1], t[2]) {
            Some(n) => normals.push(n),
            None => degen += 1,
        }
        area += (t[1] - t[0]).cross(t[2] - t[0]).length() * 0.5;
    }
    let dots: Vec<f32> = normals.windows(2).map(|w| w[0].dot(w[1])).collect();
    let mean_dot = if dots.is_empty() { 0.0 } else { dots.iter().sum::<f32>() / dots.len() as f32 };
    (mean_dot, area / tris.len().max(1) as f32, degen as f32 / tris.len().max(1) as f32)
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    let (mut s_dot, mut f_dot, mut s_area, mut f_area, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0usize);
    let (mut s_deg, mut f_deg) = (0.0f32, 0.0f32);
    for t in &tables.tracks {
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        let Ok(arts) = a.crp.articles(&a.crp_data) else { continue };
        for art in &arts {
            let Ok(g) = a.crp.article_geometry(&a.crp_data, art) else { continue };
            for p in g.primitives.iter().filter(|p| p.prim_type == 1) {
                let idx = if p.vertex_indices.is_empty() { (0..p.positions.len().min(256) as u8).collect() } else { p.vertex_indices.clone() };
                if idx.len() < 3 {
                    continue;
                }
                let v = |k: usize| Vec3::from(p.positions[idx[k] as usize]);
                let strip: Vec<[Vec3; 3]> = (0..idx.len() - 2)
                    .map(|k| if k % 2 == 0 { [v(k), v(k + 1), v(k + 2)] } else { [v(k + 1), v(k), v(k + 2)] })
                    .collect();
                let fan: Vec<[Vec3; 3]> = (0..idx.len() - 2).map(|k| [v(0), v(k + 1), v(k + 2)]).collect();
                let (sd, sa, sg) = score(&strip);
                let (fd, fa, fg) = score(&fan);
                s_dot += sd;
                f_dot += fd;
                s_area += sa;
                f_area += fa;
                s_deg += sg;
                f_deg += fg;
                n += 1;
            }
        }
    }
    let n = n as f32;
    println!("{n} type-1 primitives");
    println!("  strip: mean normal agreement {:.3}, mean triangle area {:.2} m², degenerate {:.1}%", s_dot / n, s_area / n, 100.0 * s_deg / n);
    println!("  fan:   mean normal agreement {:.3}, mean triangle area {:.2} m², degenerate {:.1}%", f_dot / n, f_area / n, 100.0 * f_deg / n);
}
