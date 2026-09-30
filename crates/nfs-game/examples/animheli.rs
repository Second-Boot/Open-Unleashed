//! per `Anqt` frame, where an anim article's origin sits vs the drivable ground under it.
//! `animheli <track> <article-substring>`
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{anim_matrix, cull_articles, ArticleDrawer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[1]).unwrap();
    let g = t.build_geometry().unwrap();
    let mats = t.materials().unwrap();
    let flags: Vec<u32> = mats.iter().map(|m| m.flags).collect();
    let grid = g.collision_grid(20.0, &|m| flags.get(m as usize).copied().unwrap_or(0));
    let arts = t.crp.articles(&t.crp_data).unwrap();
    for (i, c) in cull_articles(&t).iter().enumerate() {
        let Some(c) = c else { continue };
        if c.drawer != ArticleDrawer::Anim || !c.name.to_lowercase().contains(&a[2].to_lowercase()) {
            continue;
        }
        let tr = t.crp.article_animation(&t.crp_data, &arts[i]).unwrap();
        println!("{} frames {} sphere r {:.1}", c.name, tr.frames.len(), c.radius);
        let geo = t.crp.article_geometry(&t.crp_data, &arts[i]).unwrap();
        let (mut lo, mut hi) = (glam::Vec3::splat(9e9), glam::Vec3::splat(-9e9));
        let lod0 = geo.primitives.iter().map(|p| p.lod).min().unwrap_or(0);
        for pr in geo.primitives.iter().filter(|p| p.lod == lod0) {
            for v in &pr.positions {
                lo = lo.min(glam::Vec3::from(*v));
                hi = hi.max(glam::Vec3::from(*v));
            }
        }
        println!("  tr {:?} n_tr {}", geo.transform, geo.transforms.len());
        println!("  own-frame vertex box lod{lod0}: {lo:?} .. {hi:?}");
        let step: usize = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(1);
        for f in (0..tr.frames.len()).step_by(step) {
            let m = anim_matrix(&tr, f as f32 * 0.1, 0.1, true);
            let p = m.w_axis.truncate();
            let gh = grid.ground(p.x, p.y + 400.0, p.z, 800.0).map(|h| h.point.y);
            let low = (0..8).map(|k| m.transform_point3(glam::Vec3::new(if k & 1 == 0 { lo.x } else { hi.x }, lo.y, if k & 2 == 0 { lo.z } else { hi.z })).y).fold(9e9f32, f32::min);
            println!("  lowest-vertex y {low:.1} (above ground {:?})", gh.map(|v| ((low - v) * 10.0).round() / 10.0));
            println!("frame {f:4} t {:6.1}s pos ({:8.1},{:7.1},{:8.1}) ground {:?} above {:?}", f as f32 * 0.1, p.x, p.y, p.z, gh.map(|v| (v * 10.0).round() / 10.0), gh.map(|v| ((p.y - v) * 10.0).round() / 10.0));
        }
    }
}
