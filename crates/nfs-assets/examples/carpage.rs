//! Diagnostic: build a car's texture pages for the front end and for a race (`CarStyle::front_end`)
//! and write one page of each as PNG — `carpage <model> <page, 1-based> <outdir> [licence [number]]`
//! (with a licence the page builder's text is drawn too, `car-model.md` §5.3).
use nfs_assets::{car::CarAssets, car::CarStyle, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let mut car = CarAssets::load(&paths, &a[0]).unwrap();
    if let Some(l) = a.get(3) {
        car.page_text = Some(nfs_assets::car::PageText { license: l.clone(), race_number: a.get(4).and_then(|n| n.parse().ok()).unwrap_or(-1) });
    }
    let n: usize = a[1].parse().unwrap();
    for fe in [true, false] {
        let mut style = CarStyle::from_tpg(&car.tpg, 1);
        style.front_end = fe;
        let pages = car.build_pages(&paths, [200, 0, 0], &style).unwrap();
        let p = &pages[n - 1];
        let out = format!("{}/{}_page{}_{}.png", a[2], a[0], n, if fe { "fe" } else { "race" });
        image::save_buffer(&out, &p.rgba, p.width, p.height, image::ColorType::Rgba8).unwrap();
        println!("{out} {}x{} alpha {}", p.width, p.height, p.is_alpha);
    }
}
