//! `animnear <track> <record.csv>` — for each `WAnimBundle` article of the track, the moment the
//! recorded car (`--record`, columns tick … x y z) is nearest to the article's origin as its
//! player puts it (`anim_matrix`, race seconds = tick / 64), and how near (stills).
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{anim_matrix, cull_articles, ArticleDrawer};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[1]).unwrap();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let rows: Vec<(f32, [f32; 3], [f32; 3])> = std::fs::read_to_string(&a[2])
        .unwrap()
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').collect();
            Some((f.first()?.parse::<f32>().ok()? / 64.0, [f.get(5)?.parse().ok()?, f.get(6)?.parse().ok()?, f.get(7)?.parse().ok()?], [f.get(8)?.parse().ok()?, f.get(9)?.parse().ok()?, f.get(10)?.parse().ok()?]))
        })
        .collect();
    for (i, c) in cull_articles(&t).iter().enumerate() {
        let Some(c) = c else { continue };
        if c.drawer != ArticleDrawer::Anim {
            continue;
        }
        let tr = t.crp.article_animation(&t.crp_data, &arts[i]).unwrap();
        let best = rows
            .iter()
            .map(|&(s, p, _)| {
                let o = anim_matrix(&tr, s, 0.1, true).w_axis.truncate();
                (((o.x - p[0]).powi(2) + (o.y - p[1]).powi(2) + (o.z - p[2]).powi(2)).sqrt(), s, p, o)
            })
            .min_by(|x, y| x.0.total_cmp(&y.0))
            .unwrap();
        println!("{} {:?}: nearest {:.0} m at t = {:.1} s, car {:.0?}, article {:.0?}", a[1], c.name, best.0, best.1, best.2, best.3);
        // nearest while the article is ahead of the car (within ~40 degrees of its velocity)
        let ahead = rows
            .iter()
            .filter_map(|&(s, p, v)| {
                let o = anim_matrix(&tr, s, 0.1, true).w_axis.truncate();
                let d = [o.x - p[0], o.y - p[1], o.z - p[2]];
                let (dl, vl) = ((d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(), (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt());
                (vl > 5.0 && (d[0] * v[0] + d[1] * v[1] + d[2] * v[2]) > 0.75 * dl * vl).then_some((dl, s, p))
            })
            .min_by(|x, y| x.0.total_cmp(&y.0));
        if let Some(b) = ahead {
            println!("    ahead of the car: {:.0} m at t = {:.1} s, car {:.0?}", b.0, b.1, b.2);
        }
    }
}
