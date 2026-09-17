//! Diagnostic: decode one entry of an `.fsh` and report its alpha range (and optionally write a PNG).
use nfs_assets::{texture::decode_pack, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let path = if std::path::Path::new(&a[0]).is_file() {
        std::path::PathBuf::from(&a[0])
    } else {
        GamePaths::find_file(&paths.render_dir(), &a[0])
            .or_else(|| GamePaths::find_file(&paths.car_model_dir(), &a[0]))
            .or_else(|| GamePaths::find_file(&paths.game_data().join("DashHud"), &a[0]))
            .unwrap()
    };
    let data = std::fs::read(&path).unwrap();
    for t in decode_pack(&path, &data).unwrap() {
        if a.len() > 1 && !t.name.trim().eq_ignore_ascii_case(&a[1]) {
            continue;
        }
        let px = &t.mips[0];
        let (mut amin, mut amax) = (255u8, 0u8);
        let (mut rmin, mut rmax) = (255u8, 0u8);
        for c in px.chunks_exact(4) {
            amin = amin.min(c[3]);
            amax = amax.max(c[3]);
            rmin = rmin.min(c[0]);
            rmax = rmax.max(c[0]);
        }
        println!("{} {}x{} alpha {amin}..{amax} red {rmin}..{rmax}", t.name.trim(), t.width, t.height);
        if let Ok(dir) = std::env::var("FSH_OUT") {
            let n = format!("{dir}/{}_{}.png", a[0].replace('/', "_"), t.name.trim());
            let _ = image::save_buffer(&n, px, t.width, t.height, image::ColorType::Rgba8);
        }
    }
}
