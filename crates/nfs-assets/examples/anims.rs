//! Diagnostic: every article that follows a model animation (`Base+77`, `car-model.md`), per car —
//! and every article bound to a rotation channel (`Base+75`) — `anims [model…]`, all `.crp`s in the car model directory when none is named.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let mut models: Vec<String> = std::env::args().skip(1).collect();
    if models.is_empty() {
        let mut v: Vec<String> = std::fs::read_dir(paths.car_model_dir())
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_suffix(".crp").map(str::to_string))
            .collect();
        v.sort();
        models = v;
    }
    for m in models {
        let Ok(c) = CarAssets::load(&paths, &m) else { continue };
        for a in c.crp.articles(&c.crp_data).unwrap() {
            let Ok(g) = c.crp.article_geometry(&c.crp_data, &a) else { continue };
            if let Some(b) = g.base.as_ref().filter(|b| b.anim != 0) {
                println!("{m:<10} anim {:>2} frames {:>2} {}", b.anim, b.anim_frames, g.name);
            }
            if let Some(b) = g.base.as_ref().filter(|b| b.rot_channel != 0) {
                println!("{m:<10} channel {:>2} {}", b.rot_channel, g.name);
            }
        }
    }
}
