//! Diagnostic: the surface format (`.tpg` `bpp`) of every texture page a shipped damage patch lands on,
//! and of every page of every car — is any 4444 (so the 565 blend 0x573F2A would run over a 4444 surface)?
use nfs_assets::{
    car::{damage_patches, CarAssets, CarStyle},
    GamePaths,
};
use std::collections::BTreeMap;
fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.car_model_dir();
    let mut names: Vec<String> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.ends_with(".tpg")).map(|n| n[..n.len() - 4].to_string()).collect();
    names.sort();
    let (mut hit, mut all): (BTreeMap<i64, usize>, BTreeMap<i64, usize>) = Default::default();
    for m in &names {
        let Ok(car) = CarAssets::load(&paths, m) else {
            println!("{m}: load failed");
            continue;
        };
        let style = CarStyle::from_tpg(&car.tpg, 1);
        let Ok(pages) = car.build_pages(&paths, [200, 0, 0], &style) else {
            println!("{m}: pages failed");
            continue;
        };
        for p in &pages {
            *all.entry(p.bpp).or_default() += 1;
        }
        for light in [false, true] {
            let Ok(p) = damage_patches(&paths, m, light) else { continue };
            for q in &p {
                match pages.get(q.page) {
                    Some(pg) => *hit.entry(pg.bpp).or_default() += 1,
                    None => println!("{m} light={light}: patch page {} outside {} pages", q.page, pages.len()),
                }
            }
        }
    }
    println!("cars {}; pages by bpp {:?}; damage patches by target page bpp {:?}", names.len(), all, hit);
}
