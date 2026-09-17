//! Diagnostic: a car's `BndP` collision body points (`RE/notes/car-model.md` §12).
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let c = CarAssets::load(&paths, a.first().map(|s| s.as_str()).unwrap_or("993")).unwrap();
    for w in c.crp.window_points(&c.crp_data).unwrap() {
        println!("Wndo slot {} = {}: {} points", w.style_slot, w.style_value, w.points.len());
        for p in &w.points {
            println!("      ({:>6.3}, {:>6.3}, {:>6.3})", p[0], p[1], p[2]);
        }
    }
    let recs = c.crp.bound_points(&c.crp_data).unwrap();
    let mut all: Vec<[f32; 3]> = Vec::new();
    for r in &recs {
        let ext = |k: usize| {
            let (lo, hi) = r.points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[k]), hi.max(p[k])));
            format!("{lo:>6.2}..{hi:>6.2}")
        };
        println!("slot {} = {}: {:>2} points, x {} y {} z {}", r.style_slot, r.style_value, r.points.len(), ext(0), ext(1), ext(2));
        if r.style_slot == 0 {
            all.extend(r.points.iter().copied());
        }
    }
    all.sort_by(|a, b| a[1].total_cmp(&b[1]));
    println!("default body: {} points, the four lowest (the wheel contact set):", all.len());
    for p in all.iter().take(4) {
        println!("   ({:>6.3}, {:>6.3}, {:>6.3})", p[0], p[1], p[2]);
    }
}
