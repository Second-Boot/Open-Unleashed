//! Diagnostic: write the entries of a car's embedded `sf#k` packs whose names contain a filter
//! as PNGs — `carsf <model> <k> <filter> <outdir>`.
use nfs_assets::{car::CarAssets, texture::decode_pack, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let car = CarAssets::load(&paths, &a[0]).unwrap();
    let files = car.crp.embedded_texture_files(&car.crp_data).unwrap();
    let k: usize = a[1].parse().unwrap();
    let fsh = nfs_formats::fsh::Fsh::parse(files[k]).unwrap();
    for (i, e) in fsh.entries.iter().enumerate() {
        if let Ok(d) = fsh.decode(files[k], i) {
            println!("fmt {} {:?}", e.name.trim(), d.source_format);
        }
    }
    let imgs = decode_pack(std::path::Path::new("embedded"), files[k]).unwrap();
    for t in imgs.iter().filter(|t| t.name.contains(a[2].as_str())) {
        let px = &t.mips[0];
        let amin = px.chunks_exact(4).map(|c| c[3]).min().unwrap_or(0);
        println!("{} {}x{} alpha min {amin} place {:?}", t.name.trim(), t.width, t.height, t.place);
        let _ = image::save_buffer(format!("{}/{}_{}.png", a[3], a[0], t.name.trim()), px, t.width, t.height, image::ColorType::Rgba8);
    }
}
