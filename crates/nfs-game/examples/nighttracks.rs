//! Diagnostic: which tracks the headlight render layers mark as night (H35).
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let tables = nfs_game::career::CareerTables::load(&paths).unwrap();
    for t in &tables.tracks {
        println!("{} name {} {}", t.file, t.name_string_id, t.display_name);
        let Ok(a) = TrackAssets::load(&paths, &t.file) else { continue };
        let amb = nfs_game::world::MapLight::from_map(&a.map).ambient;
        let ml = nfs_game::world::MapLight::from_map(&a.map); println!("{:<11} night {:<5}  map ambient {:.2} {:.2} {:.2} tint {:.2} decal {:.2}", t.file, a.is_night(), amb[0], amb[1], amb[2], ml.tint[0], ml.light_decal[0]);
    }
}
