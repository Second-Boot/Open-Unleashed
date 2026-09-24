//! Diagnostic: the names (and sizes) of a track's textures, filtered by a substring.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let f = a.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
    for x in &t.textures {
        if x.name.to_lowercase().contains(&f) {
            let px = &x.mips[0];
            let (mut rmin, mut rmax, mut amin, mut amax) = (255u8, 0u8, 255u8, 0u8);
            for c in px.chunks_exact(4) {
                rmin = rmin.min(c[0]);
                rmax = rmax.max(c[0]);
                amin = amin.min(c[3]);
                amax = amax.max(c[3]);
            }
            println!("{:?} {}x{} mips {} alpha {} red {rmin}..{rmax} a {amin}..{amax}", x.name, x.width, x.height, x.mips.len(), x.has_alpha);
        }
    }
}
