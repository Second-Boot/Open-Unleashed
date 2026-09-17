//! Diagnostic: the `.key` draw-distance curve of a track.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in std::env::args().skip(1) {
        let t = TrackAssets::load(&paths, &name).unwrap();
        let k = &t.draw_keys_forward;
        let min = k.keys.iter().map(|k| k.distance).fold(f32::MAX, f32::min);
        let max = k.keys.iter().map(|k| k.distance).fold(f32::MIN, f32::max);
        println!("{name}: {} keys, {min:.0}..{max:.0} m, at slice 0/100/500: {:.0}/{:.0}/{:.0}", k.keys.len(), k.value_at(0), k.value_at(100), k.value_at(500));
    }
}
