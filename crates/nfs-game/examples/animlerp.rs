//! the original lerps the quaternion (0x48A150, no normalise, no sign fix) — how far is that from
//! the port's slerp on the shipped tracks?  `animlerp` prints, per anim article, min dot of consecutive
//! frames and the smallest |lerp(q_i,q_{i+1},0.5)|.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{cull_articles, ArticleDrawer};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in TrackAssets::list(&paths) {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let arts = t.crp.articles(&t.crp_data).unwrap();
        for (i, c) in cull_articles(&t).iter().enumerate() {
            let Some(c) = c else { continue };
            if c.drawer != ArticleDrawer::Anim {
                continue;
            }
            let Some(tr) = t.crp.article_animation(&t.crp_data, &arts[i]) else { continue };
            let (mut mind, mut minn) = (9.0f32, 9.0f32);
            let mut flips = 0;
            for w in tr.frames.windows(2) {
                let (a, b) = (w[0].rotation, w[1].rotation);
                let d: f32 = (0..4).map(|k| a[k] * b[k]).sum();
                if d < 0.0 {
                    flips += 1;
                }
                mind = mind.min(d);
                let n = (0..4).map(|k| ((a[k] + b[k]) * 0.5).powi(2)).sum::<f32>().sqrt();
                minn = minn.min(n);
            }
            println!("{name} {:26} frames {:4} min dot {mind:.4} sign flips {flips:3} min |lerp mid| {minn:.4}", c.name, tr.frames.len());
        }
    }
}
