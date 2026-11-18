//! Diagnostic (GATE-REL, ENHANCEMENT E9): per track named on the command line, how the baked vertex colours stand against the
//! bake's own light model (`nfs_game::bake_light`, `luma = a + b max(0, n.L)`): quantiles of `luma / model` for the scenery's
//! faces and the ground's, and of the plain luma, so E9's relative "already baked dark" gate can be set from the data.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::bake_light::{face_predictions, fit};

fn q(v: &mut [f32]) -> String {
    v.sort_by(f32::total_cmp);
    if v.is_empty() {
        return "-".into();
    }
    [0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95].iter().map(|p| format!("{:.2}", v[((v.len() - 1) as f32 * p) as usize])).collect::<Vec<_>>().join(" ")
}

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in std::env::args().skip(1) {
        let t = TrackAssets::load(&paths, &name).unwrap();
        let g = t.build_geometry().unwrap();
        let m = t.materials().unwrap();
        let Some(f) = fit(&g, &m) else { continue };
        println!("{name}: bake az {:.0} el {:.0} r2 {:.2}: a {:.3} b {:.3}", f.azimuth, f.elevation, f.r2, f.intercept, f.slope);
        let (sc, gr) = face_predictions(&g, &m, &f);
        for (label, v) in [("scenery", sc), ("ground", gr)] {
            let mut ratio: Vec<f32> = v.iter().map(|(l, p, _)| l / p.max(0.05)).collect();
            let mut luma: Vec<f32> = v.iter().map(|x| x.0).collect();
            let mut nl: Vec<f32> = v.iter().map(|x| x.2).collect();
            println!("  {label} ({} faces) q05..q95: ratio {} | luma {} | n.L {}", v.len(), q(&mut ratio), q(&mut luma), q(&mut nl));
        }
    }
}
