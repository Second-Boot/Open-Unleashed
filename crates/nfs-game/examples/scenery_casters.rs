//! Diagnostic (FOREST-TREESH, ENHANCEMENT E9): a track's scenery parts that E9 could take as sun-shadow casters (`world.rs`
//! `upload_track_lods`: a world article part whose material is not drivable ground, every instance prop), with each part's
//! world bounding sphere (the renderer's `mesh_bounds`: the box's centre, the farthest vertex), its height, texture and
//! whether it stands within `radius` m of a point. usage: `scenery_casters <track> [x,y,z] [reach m]`
use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};

fn bounds(ps: &[Vec3]) -> (Vec3, f32, Vec3, Vec3) {
    let (lo, hi) = ps.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(*p), h.max(*p)));
    let c = (lo + hi) * 0.5;
    (c, ps.iter().map(|p| (*p - c).length()).fold(0.0, f32::max), lo, hi)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let mats = t.materials().unwrap();
    let g = t.build_geometry().unwrap();
    let at = a.get(1).map(|s| {
        let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
        Vec3::new(v[0], v[1], v[2])
    });
    let reach: f32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(150.0);
    let tex = |m: u16| mats.get(m as usize).map_or(String::from("?"), |m| m.texture_name());
    let surf = |m: u16| mats.get(m as usize).map_or(0, |m| m.flags & 0xF);
    for art in &g.world {
        let Some((lod, meshes)) = art.lods.first() else { continue };
        for mm in meshes {
            let ps: Vec<Vec3> = mm.mesh.vertices.iter().map(|v| art.transform.transform_point3(Vec3::from(v.position))).collect();
            if ps.is_empty() {
                continue;
            }
            let (c, r, lo, hi) = bounds(&ps);
            if at.is_some_and(|p| (c - p).length() - r > reach) {
                continue;
            }
            println!(
                "art {:5} lod {lod} surf {:2} cast {} r {:6.1} h {:5.1} c {:7.1} {:7.1} {:7.1} y {:6.1}..{:6.1} tris {:5} {}",
                art.index, surf(mm.material), u8::from(surf(mm.material) == 0), r, hi.y - lo.y, c.x, c.y, c.z, lo.y, hi.y, mm.mesh.indices.len() / 3, tex(mm.material)
            );
        }
    }
    for inst in &g.instances {
        let Some(art) = g.library.iter().find(|l| l.tag == inst.tag) else { continue };
        let m = inst.transform * glam::Mat4::from_scale(inst.scale);
        let Some((_, meshes)) = art.lods.first() else { continue };
        for mm in meshes {
            let ps: Vec<Vec3> = mm.mesh.vertices.iter().map(|v| m.transform_point3(Vec3::from(v.position))).collect();
            if ps.is_empty() {
                continue;
            }
            let (c, r, lo, hi) = bounds(&ps);
            if at.is_some_and(|p| (c - p).length() - r > reach) {
                continue;
            }
            println!("inst {:?} r {:6.1} h {:5.1} c {:7.1} {:7.1} {:7.1} {}", inst.tag, r, hi.y - lo.y, c.x, c.y, c.z, tex(mm.material));
        }
    }
}
