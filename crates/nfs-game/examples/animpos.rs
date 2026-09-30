//! `animpos <track> <article-substring> <seconds>...` — where the `WAnimBundle` article's player
//! puts the article's `Base` centre at those race seconds (stills: fixed cameras).
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{anim_matrix, cull_articles, ArticleDrawer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[1]).unwrap();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    for (i, c) in cull_articles(&t).iter().enumerate() {
        let Some(c) = c else { continue };
        if c.drawer != ArticleDrawer::Anim || !c.name.to_lowercase().contains(&a[2].to_lowercase()) {
            continue;
        }
        let tr = t.crp.article_animation(&t.crp_data, &arts[i]).unwrap();
        for s in &a[3..] {
            let secs: f32 = s.parse().unwrap();
            let m = anim_matrix(&tr, secs, 0.1, true);
            println!("{} @ {secs}: origin {:.1?} x-axis {:.2?}", c.name, m.w_axis.truncate(), m.x_axis.truncate());
        }
    }
}
