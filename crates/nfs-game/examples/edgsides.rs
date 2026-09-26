//! Diagnostic: is every `.edg` wall segment a side of a drivable triangle? — `edgsides <track>`.
//! `WallImpulse` 0x494000 drops a wall push that lands off the road, which only works when the
//! wall line and the road's outline coincide (`suspension-collision.md` §6.2, 2026-09-26: they do,
//! bit for bit, on every shipped track checked). Also prints the drivable height range.
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::HashSet;

fn main() {
    let name = std::env::args().nth(1).expect("edgsides <track>");
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &name).unwrap();
    let g = t.build_geometry().unwrap();
    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let key = |v: glam::Vec3| (v.x.to_bits(), v.y.to_bits(), v.z.to_bits());
    let (mut sides, mut verts) = (HashSet::new(), HashSet::new());
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    // the same meshes `TrackGeometry::collision_grid` takes from the world articles
    for art in &g.world {
        for mm in &art.meshes {
            if flags.get(mm.material as usize).copied().unwrap_or(0) & 0xF == 0 {
                continue;
            }
            for idx in mm.mesh.indices.chunks_exact(3) {
                let p = |i: u32| art.transform.transform_point3(glam::Vec3::from(mm.mesh.vertices[i as usize].position));
                let v = [p(idx[0]), p(idx[1]), p(idx[2])];
                for k in 0..3 {
                    let (a, b) = (key(v[k]), key(v[(k + 1) % 3]));
                    sides.insert((a.min(b), a.max(b)));
                    verts.insert(a);
                    lo = lo.min(v[k].y);
                    hi = hi.max(v[k].y);
                }
            }
        }
    }
    let (mut total, mut side, mut ends) = (0, 0, 0);
    for s in t.edges.iter().flat_map(|e| e.segments.iter()) {
        let (a, b) = (key(glam::Vec3::from(s.p1)), key(glam::Vec3::from(s.p2)));
        total += 1;
        side += sides.contains(&(a.min(b), a.max(b))) as usize;
        ends += verts.contains(&a) as usize + verts.contains(&b) as usize;
        if !sides.contains(&(a.min(b), a.max(b))) {
            println!("  not a side: {:?} -> {:?}", s.p1, s.p2);
        }
    }
    println!("{name}: {total} segments, {side} a drivable triangle's side; {ends} of {} endpoints on a vertex; drivable y [{lo:.1}, {hi:.1}]", 2 * total);
}
