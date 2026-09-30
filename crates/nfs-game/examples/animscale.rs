//! Scale / quaternion-norm / frame-count census of the `WAnimBundle` articles' `Anqt` tracks.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{cull_articles, track_flares, ArticleDrawer};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in TrackAssets::list(&paths) {
        let Ok(t) = TrackAssets::load(&paths, &name) else { continue };
        let arts = t.crp.articles(&t.crp_data).unwrap();
        let flares = track_flares(&t);
        for (i, c) in cull_articles(&t).iter().enumerate() {
            let Some(c) = c else { continue };
            if c.drawer != ArticleDrawer::Anim {
                continue;
            }
            let Some(tr) = t.crp.article_animation(&t.crp_data, &arts[i]) else {
                println!("{name} {} NO TRACK", c.name);
                continue;
            };
            let (mut lo, mut hi) = ([9e9f32; 3], [-9e9f32; 3]);
            let mut qn = (9e9f32, -9e9f32);
            for f in &tr.frames {
                for k in 0..3 {
                    lo[k] = lo[k].min(f.scale[k]);
                    hi[k] = hi[k].max(f.scale[k]);
                }
                let n = f.rotation.iter().map(|x| x * x).sum::<f32>().sqrt();
                qn = (qn.0.min(n), qn.1.max(n));
            }
            let nfl = flares.iter().filter(|f| f.article == i).count();
            let anqt = arts[i].level(b"Anqt").unwrap();
            println!("{name} {:?} frames {} scale {lo:?}..{hi:?} qnorm {qn:?} anqt_len/52 {} flares {nfl}", c.name, tr.frames.len(), anqt.data_len() / 52);
        }
    }
}
