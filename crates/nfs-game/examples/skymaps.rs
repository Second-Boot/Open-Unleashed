//! Census: for every shipped track, every sky map the sky file's blocks of
//! `SKYRENDERING` levels 0..=3 name (both regions, every layer) — is the entry found in the
//! track's `Sky/<Track>.fsh`, and is it a 16 × 16 `0x7D` (what `Sky::upload`'s `map` reads)?
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::sky::SkyFile;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let (mut total, mut odd) = (0, 0);
    for name in ["alps", "autobahn", "canyon", "castle", "coastal", "farmland", "foothills", "forest", "industrial", "monaco1", "monaco2", "monaco3", "monaco4", "monaco5", "skidpad"] {
        let Ok(t) = TrackAssets::load(&paths, name) else {
            println!("{name}: load failed");
            continue;
        };
        let Ok(f) = SkyFile::parse(&t.sky_params.join("\n")) else {
            println!("{name}: sky file parse failed");
            continue;
        };
        for level in 0..4 {
            for b in f.for_level(level) {
                for (ri, region) in b.regions.iter().enumerate() {
                    for l in &region.layers {
                        let Some(m) = l.map.as_deref() else { continue };
                        total += 1;
                        match t.sky_textures.iter().find(|x| x.name.trim() == m) {
                            Some(x) if x.width == 16 && x.height == 16 && x.code == 0x7D => {}
                            Some(x) => {
                                odd += 1;
                                println!("{name} level {level} region {ri} map {m}: {}x{} code {:#x}", x.width, x.height, x.code);
                            }
                            None => {
                                odd += 1;
                                println!("{name} level {level} region {ri} map {m}: not in the .fsh");
                            }
                        }
                    }
                }
            }
        }
    }
    println!("maps named {total}, not a 16x16 0x7D: {odd}");
}
