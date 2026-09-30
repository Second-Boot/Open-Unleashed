//! Census: the articles `WAnimBundle` claims (an `Anqt` level and no `tr#0`, not a library
//! article) on every shipped track — name, tag, frame count, and how far the track moves the
//! article from frame 0 (position spread, largest rotation angle from frame 0) — against what
//! `TrackWorld` animates (the `Locomotive` alone).
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{cull_articles, ArticleDrawer};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in TrackAssets::list(&paths) {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let arts = t.crp.articles(&t.crp_data).unwrap();
        let cull = cull_articles(&t);
        for (i, c) in cull.iter().enumerate() {
            let Some(c) = c else { continue };
            if c.drawer != ArticleDrawer::Anim {
                continue;
            }
            let track = t.crp.article_animation(&t.crp_data, &arts[i]);
            let (frames, spread, angle) = match &track {
                Some(tr) => {
                    let f0 = tr.frames[0];
                    let spread = tr.frames.iter().map(|f| ((f.position[0] - f0.position[0]).powi(2) + (f.position[1] - f0.position[1]).powi(2) + (f.position[2] - f0.position[2]).powi(2)).sqrt()).fold(0.0f32, f32::max);
                    let angle = tr.frames.iter().map(|f| 2.0 * (f.rotation.iter().zip(&f0.rotation).map(|(a, b)| a * b).sum::<f32>().abs().min(1.0)).acos().to_degrees()).fold(0.0f32, f32::max);
                    (tr.frames.len(), spread, angle)
                }
                None => (0, 0.0, 0.0),
            };
            let f0 = track.as_ref().map(|t| t.frames[0].position);
            println!("{name}: article {i} {:?} centre {:.0?} radius {:.1}: {frames} frames, frame-0 position {f0:.0?}, position spread {spread:.2} m, rotation up to {angle:.1} deg", c.name, c.centre, c.radius);
        }
    }
}
