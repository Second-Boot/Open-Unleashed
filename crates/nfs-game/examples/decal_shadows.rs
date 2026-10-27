//! Diagnostic (ENH-DBLSH): where a track's `sh80` ground-shadow decals sit, and what else is in their article.
use nfs_assets::{track::TrackAssets, GamePaths};
use glam::Vec3;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let mats = t.materials().unwrap();
    let g = t.build_geometry().unwrap();
    let is_sh = |i: u16| mats.get(i as usize).is_some_and(|m| m.texture_name().starts_with("sh80"));
    let table = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let s0 = table.slices[0].centre;
    println!("slice0 {s0:?} of {}", table.slices.len());
    let (mut arts, mut decal_parts, mut only_decal) = (0, 0, 0);
    for art in &g.world {
        for (lod, meshes) in art.lods.iter().take(1) {
            let d: Vec<_> = meshes.iter().filter(|m| is_sh(m.material)).collect();
            if d.is_empty() { continue; }
            arts += 1; decal_parts += d.len();
            let others: Vec<_> = meshes.iter().filter(|m| !is_sh(m.material)).collect();
            if others.is_empty() { only_decal += 1; }
            let bb = |ps: Vec<[f32; 3]>| { let mut lo = Vec3::splat(f32::MAX); let mut hi = Vec3::splat(f32::MIN); for v in ps { let p = art.transform.transform_point3(Vec3::from(v)); lo = lo.min(p); hi = hi.max(p);} (lo, hi) };
            let (dl, dh) = bb(d.iter().flat_map(|m| m.mesh.vertices.iter().map(|v| v.position)).collect());
            let (ol, oh) = bb(others.iter().flat_map(|m| m.mesh.vertices.iter().map(|v| v.position)).collect());
            let dc=(dl+dh)*0.5; let mut best=(f32::MAX,0); for (i,sl) in table.slices.iter().enumerate(){ let dd=(sl.centre-dc).length(); if dd<best.0{best=(dd,i)} } if arts <= 60 { println!("art {} dist-to-road {:.0} at slice {} centre {:.0?} others {}", art.index, best.0, best.1, dc, others.len()); } if false { println!("art {} lod {} decals {} others {} | decal bb {:.0?}..{:.0?} (h {:.1}) | others bb {:.0?}..{:.0?} size {:.0?}", art.index, lod, d.len(), others.len(), dl, dh, dh.y-dl.y, ol, oh, oh-ol); }
        }
    }
    println!("{}: articles with sh80 parts {arts}, parts {decal_parts}, articles with ONLY decals {only_decal}", a[0]);
}
