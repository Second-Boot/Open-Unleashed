//! Census: how often does the track walk's gate (`nfs_game::world::CellWalk`, 0x47F890 over
//! `WLineBundle`'s cells: node pass, frustum, `Vis `, `s² < far²`) drop the cell of a ground point
//! that is **on screen and within the marks' 100 m** — the eye on the racing line (1.2 m over the
//! drivable ground), looking along it, the points on a 5 m grid across ±40° and 3…100 m. A skid
//! mark laid there would be drawn by the port before this change and is not by the original.
//! `skidwalk [track…]`. The tree is the world's without the `.scn` items (`tree_population`).
use glam::Vec3;
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::skidmarks::SkidMarks;
use nfs_game::world::{cull_articles, track_occluders, tree_population, CellDrop, CellWalk, CullView};
use std::sync::Arc;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut tracks: Vec<String> = std::env::args().skip(1).collect();
    if tracks.is_empty() {
        tracks = TrackAssets::list(&paths);
    }
    for name in tracks {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let arts = cull_articles(&t);
        let pop = tree_population(&t, &arts);
        let tree = Arc::new(nfs_game::fitted_tree::FittedTree::new(pop.min, pop.max, &pop.points));
        let marks = SkidMarks::for_tree(tree);
        let occluders = track_occluders(&t);
        let g = t.build_geometry().unwrap();
        let mats = t.materials().unwrap();
        let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
        let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
        let Some(line) = t.racing_line(&paths, false) else { continue };
        let n = line.points.len();
        if n < 4 {
            continue;
        }
        let (mut points, mut drops, mut eyes, mut eyes_hit) = (0u64, [0u64; 4], 0u64, 0u64);
        for i in (0..n).step_by(3) {
            let p = line.points[i];
            let q = line.points[(i + 3) % n];
            let fwd = Vec3::new(q[0] - p[0], 0.0, q[1] - p[1]).normalize_or_zero();
            let Some(h) = grid.ground(p[0], 2000.0, p[1], 4000.0).map(|h| h.point.y) else { continue };
            if fwd == Vec3::ZERO {
                continue;
            }
            let right = fwd.cross(Vec3::Y).normalize_or_zero();
            let view = CullView { eye: Vec3::new(p[0], h + 1.2, p[1]), forward: fwd, right, up: right.cross(fwd).normalize_or_zero(), tan_h: 0.92, tan_v: 0.69, near: 0.5, draw: 700.0 };
            let walk = CellWalk::new(&view, &occluders);
            eyes += 1;
            let mut hit = false;
            for d in (3..=100).step_by(5) {
                let half = (d as f32 * 0.84).min(90.0);
                let mut s = -half;
                while s <= half {
                    let at = view.eye + fwd * d as f32 + right * s;
                    let (dx, dz) = (at.x - view.eye.x, at.z - view.eye.z);
                    if (dx * dx + dz * dz).sqrt() <= 100.0 {
                        let ground = Vec3::new(at.x, h, at.z);
                        if let Some((cx, _, cz, r, nodes)) = marks.cell_at(ground) {
                            points += 1;
                            if let Err(why) = walk.gate(cx, h, cz, r, nodes) {
                                drops[why as usize] += 1;
                                hit = true;
                            }
                        }
                    }
                    s += 5.0;
                }
            }
            eyes_hit += hit as u64;
        }
        let total: u64 = drops.iter().sum();
        println!(
            "{name}: {eyes} eyes, {points} on-screen ground points within 100 m, {total} in a cell the walk drops ({:.2} %): {} by the node pass, {} frustum, {} Vis, {} distance; {eyes_hit} eyes lose some. eyes at y over the ground; {} cells",
            100.0 * total as f64 / points.max(1) as f64,
            drops[CellDrop::Nodes as usize],
            drops[CellDrop::Frustum as usize],
            drops[CellDrop::Occluded as usize],
            drops[CellDrop::Distance as usize],
            0
        );
    }
}
