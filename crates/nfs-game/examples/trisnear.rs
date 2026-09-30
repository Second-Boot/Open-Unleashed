//! which articles' collision triangles overlap an xz box —
//! `trisnear <track> <x> <z> <half>`; lists per article (bundle kind, triangle count, y range).
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let (x0, z0, h): (f32, f32, f32) = (a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap());
    let mut by: BTreeMap<usize, (usize, f32, f32, u32)> = BTreeMap::new();
    for tr in grid.triangles() {
        let xs = [tr.a.x, tr.b.x, tr.c.x];
        let zs = [tr.a.z, tr.b.z, tr.c.z];
        let (lx, hx) = (xs.iter().cloned().fold(1e9, f32::min), xs.iter().cloned().fold(-1e9, f32::max));
        let (lz, hz) = (zs.iter().cloned().fold(1e9, f32::min), zs.iter().cloned().fold(-1e9, f32::max));
        if hx < x0 - h || lx > x0 + h || hz < z0 - h || lz > z0 + h {
            continue;
        }
        let Some(s) = tr.source else { continue };
        let ys = [tr.a.y, tr.b.y, tr.c.y];
        let e = by.entry(s.article).or_insert((0, 1e9, -1e9, 0));
        e.0 += 1;
        e.1 = e.1.min(ys.iter().cloned().fold(1e9, f32::min));
        e.2 = e.2.max(ys.iter().cloned().fold(-1e9, f32::max));
        e.3 |= 1 << (tr.surface & 0xF);
    }
    for (i, (n, lo, hi, sm)) in by {
        let name = g.world.iter().find(|w| w.index == i).map(|w| w.name.clone()).unwrap_or_else(|| "(instance/library)".into());
        println!("article {i:5} {:?} {name:<28} tris {n:4} y {lo:.1}..{hi:.1} surfaces {sm:#06x}", t.crp.bundle_of(&t.crp_data, &arts[i]).ok());
    }
}
