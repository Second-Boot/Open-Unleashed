//! Diagnostic (PA-CARLIGHT-O1): count the track's quad patches by winding (the byte 0x4853C0 stores at
//! `patch+8`: 1 when all four edge tests against the centre are <= 0, else 0), and list the patches around a point.
//! `quadwind <track> [x z]`
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_physics::collision::PatchKind;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let tris = grid.triangles();
    let quad_prims = |arts: &[nfs_assets::track::ArticleMeshes]| arts.iter().flat_map(|a| a.meshes().iter()).filter(|m| m.patch_kind == PatchKind::Quad).count();
    println!("quad primitives: world {} (collides {}), library {}", quad_prims(&g.world), g.world.iter().filter(|a| a.collides).flat_map(|a| a.meshes().iter()).filter(|m| m.patch_kind == PatchKind::Quad).count(), quad_prims(&g.library));
    println!("grid triangles from quad primitives: {}", tris.iter().filter(|t| t.source.map(|s| s.kind == PatchKind::Quad).unwrap_or(false)).count());
    let (mut tri_n, mut q1, mut q0) = (0, 0, 0);
    let at = if a.len() >= 3 { Some((a[1].parse::<f32>().unwrap(), a[2].parse::<f32>().unwrap())) } else { None };
    for p in grid.patches() {
        match p.kind {
            PatchKind::Triangle => tri_n += 1,
            PatchKind::Quad => {
                let (t0, t1) = (&tris[p.tris[0] as usize], &tris[p.tris[1] as usize]);
                let v = [t0.a, t0.c, t0.b, t1.b]; // v0 v1 v2 v3
                let c = p.centroid;
                let edge = |a: glam::Vec3, b: glam::Vec3| (a.x - b.x) * (c.z - b.z) - (a.z - b.z) * (c.x - b.x);
                let w = !(edge(v[0], v[1]) > 0.0 || edge(v[1], v[2]) > 0.0 || edge(v[2], v[3]) > 0.0 || edge(v[3], v[0]) > 0.0);
                if w { q1 += 1 } else { q0 += 1 }
                if let Some((x, z)) = at {
                    let (e0, e1) = (p.extent.0, p.extent.1);
                    if x >= e0.0 && x <= e1.0 && z >= e0.1 && z <= e1.1 {
                        println!("quad winding={} surface={:#x} v0={:?} v1={:?} v2={:?} v3={:?}", w as u8, t0.surface & 0xFFFF, v[0], v[1], v[2], v[3]);
                    }
                }
            }
        }
    }
    println!("triangles {tri_n}, quads winding1 {q1}, winding0 {q0}");
}
