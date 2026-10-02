//! Diagnostic: every shipped damage patch (`<model>D.fsh` / `<model>L.fsh`) with a threshold above 0 — the ones
//! `blend_patch` copies (0x53D570) instead of alpha-blending (0x5400B0) — and the totals.
use nfs_assets::{car::damage_patches, GamePaths};
fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.car_model_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.ends_with(".tpg")).map(|n| n[..n.len() - 4].to_string()).collect();
    names.sort();
    let (mut total, mut thr) = (0, 0);
    for m in names {
        for light in [false, true] {
            let Ok(p) = damage_patches(&paths, &m, light) else { continue };
            total += p.len();
            for q in p.iter().filter(|q| q.threshold > 0.0) {
                thr += 1;
                println!("{m} light={light} zone {} page {} thr {} {}x{}", q.zone, q.page, q.threshold, q.image.width, q.image.height);
            }
        }
    }
    println!("total {total} thr>0 {thr}");
}
