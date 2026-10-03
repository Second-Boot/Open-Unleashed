//! Diagnostic (FRINGE): per render method, the alpha textures a track's materials name.
use nfs_assets::{track::TrackAssets, GamePaths};
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &a[0]).unwrap();
    let mats = t.materials().unwrap();
    let mut tally = std::collections::BTreeMap::<(String, String), usize>::new();
    for m in &mats {
        let Some(x) = t.textures.iter().find(|x| x.name == m.texture_name()) else { continue };
        if !x.has_alpha {
            continue;
        }
        *tally.entry((m.render_method.clone(), x.name.clone())).or_default() += 1;
    }
    for ((rm, tx), n) in tally {
        println!("{rm:14} {tx} x{n}");
    }
}
