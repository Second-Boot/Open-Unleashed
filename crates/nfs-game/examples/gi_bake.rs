//! ENHANCEMENT E32: bake (or re-bake) a track's bounce light and print the time and the sample statistics.
//! `cargo run --release -p nfs-game --example gi_bake -- <track>...` (the cache goes to `nfs_game::gi::cache_dir()`,
//! `NFS_GI_DIR` to override; `--force` ignores an existing cache). Not every world article the game
//! skips (breakables, smackables) is skipped here: the game's own input differs on those tracks, so this
//! is for timing and statistics, not for filling the game's cache.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::gi;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let force = args.iter().any(|a| a == "--force");
    let paths = GamePaths::locate().expect("game data");
    for name in args.iter().filter(|a| !a.starts_with("--")) {
        let t = TrackAssets::load(&paths, name).expect("track");
        if t.is_night() {
            println!("{name}: night track, no bake");
            continue;
        }
        let g = t.build_geometry().expect("geometry");
        let t0 = std::time::Instant::now();
        let (input, parts) = gi::build(&t, &g, &|a| a.animation.is_some(), gi::to_sun(&t));
        let prep = t0.elapsed().as_secs_f32();
        let tb = std::time::Instant::now();
        let _ = gi::Bvh::build(&input.occ);
        println!("bvh {:.2} s", tb.elapsed().as_secs_f32());
        let t1 = std::time::Instant::now();
        let samples = if force { gi::bake(&input) } else { gi::load_or_bake(name, &input).0 };
        let secs = t1.elapsed().as_secs_f32();
        let n = samples.len().max(1) as f32;
        let mean = samples.iter().fold([0.0f32; 5], |a, s| [a[0] + s.bounce[0], a[1] + s.bounce[1], a[2] + s.bounce[2], a[3] + s.sky, a[4] + s.sun]).map(|x| x / n);
        println!(
            "{name}: receivers {} parts {} triangles {} | input {prep:.2} s, bake {secs:.2} s | mean bounce {:.3} {:.3} {:.3} sky {:.2} sun {:.2} | sky colour {:?} sun {:?}",
            input.recv_pos.len(),
            parts.len(),
            input.occ.tris.len(),
            mean[0],
            mean[1],
            mean[2],
            mean[3],
            mean[4],
            gi::sky_colour(&t),
            gi::to_sun(&t)
        );
    }
}
