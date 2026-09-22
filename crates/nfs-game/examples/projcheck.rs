//! Diagnostic: each kind-4 projector's `(u, v)` at its own point and on the ground below it.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let ps = nfs_game::world::track_projectors(&t);
    let geometry = t.build_geometry().unwrap();
    let flags: Vec<u32> = t.materials().unwrap().iter().map(|m| m.flags).collect();
    let tris = Some(geometry.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0)));
    for (k, p) in ps.iter().enumerate().take(12) {
        let uv = |x: f32, y: f32, z: f32| (p.u[0] * x + p.u[1] * y + p.u[2] * z + p.u[3], p.v[0] * x + p.v[1] * y + p.v[2] * z + p.v[3]);
        let g = tris.as_ref().and_then(|c| c.ground(p.point.x, p.point.y + 50.0, p.point.z, 200.0)).map(|h| h.point.y);
        let (u0, v0) = uv(p.point.x, p.point.y, p.point.z);
        let gv = g.map(|gy| uv(p.point.x, gy, p.point.z));
        println!("{k:2} r {:.1} pt {:?} uv@pt ({u0:.2},{v0:.2}) ground {g:?} uv@ground {gv:?} u {:?} v {:?}", p.radius, p.point, p.u, p.v);
    }
}
