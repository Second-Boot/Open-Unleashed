//! Diagnostic: parse every shipped `Track/Sky/*.txt`.
use nfs_assets::GamePaths;
use nfs_formats::sky::SkyFile;

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.game_data().join("Track").join("Sky");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("txt")).unwrap_or(false)).collect();
    files.sort();
    // also check the path the port takes: TrackAssets pre-filters the lines
    if let Ok(t) = nfs_assets::track::TrackAssets::load(&paths, "coastal") {
        println!("coastal sky_params {} lines, textures {:?}", t.sky_params.len(), t.sky_textures.iter().map(|x| x.name.trim().to_string()).collect::<Vec<_>>());
        match SkyFile::parse(&t.sky_params.join("\n")) {
            Ok(f) => println!("  parsed {} blocks, HIGH = {:?}", f.blocks.len(), f.for_capability("HIGH").map(|b| (&b.capability, b.kind, b.regions[0].layers.iter().map(|l| l.map.clone()).collect::<Vec<_>>()))),
            Err(e) => println!("  parse error: {e}"),
        }
    }
    for p in files {
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        match SkyFile::parse(&text) {
            Ok(f) => {
                println!("{}: {} blocks", p.file_name().unwrap().to_string_lossy(), f.blocks.len());
                for b in &f.blocks {
                    let a = &b.regions[0];
                    println!(
                        "   {:<7} {:?} sun {} move {:>5} warp {}/{} clouds {:?} layers {:?}",
                        b.capability,
                        b.kind,
                        b.blocks_sun,
                        a.move_ratio,
                        a.warp_speed,
                        a.warp_amount,
                        if b.clouds.is_empty() { vec![a.cloud.clone()] } else { b.clouds.clone() },
                        a.layers.iter().map(|l| format!("{:?} {:?}×{:?} wind {}@{}", l.map, l.scale[0], l.scale[1], l.wind_speed, l.wind_direction)).collect::<Vec<_>>()
                    );
                }
            }
            Err(e) => println!("{}: {e}", p.file_name().unwrap().to_string_lossy()),
        }
    }
}
