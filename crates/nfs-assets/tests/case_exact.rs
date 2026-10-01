//! Linux (case-sensitive file systems): every file the port opens through a **direct** path
//! (`root.join("GameData/…")`, not `GamePaths::find_file`) must match the on-disk case exactly.
//! macOS volumes are usually case-insensitive, so this checks the exact name with `read_dir`
//! instead of `exists()`. Skipped without the game's files.
//!
//! The list is every direct `join` of a fixed data name in the crates' non-test code (audited
//! 2026-10-01); names built from data (car models, tracks, sounds, art) go through `find_file`.

use nfs_assets::platform::dev_game_root;
use std::path::Path;

/// `true` when every component of `rel` exists under `root` with exactly this spelling.
fn exact(root: &Path, rel: &str) -> bool {
    let mut cur = root.to_path_buf();
    for part in rel.split('/') {
        let Ok(rd) = std::fs::read_dir(&cur) else { return false };
        if !rd.flatten().any(|e| e.file_name().to_str() == Some(part)) {
            return false;
        }
        cur.push(part);
    }
    true
}

#[test]
fn the_fixed_names_the_port_joins_directly_match_the_disk_case() {
    let Some(root) = dev_game_root() else {
        eprintln!("skipping: original game data not found");
        return;
    };
    let direct = [
        "fe.txt",
        "nfs5.exe",
        "GameData/Sounds/track.viv",
        "GameData/Sounds/zzzwzzz.viv",
        "GameData/Sounds/SND_coll.bnk",
        "GameData/Sounds/fesfx.bnk",
        "GameData/Sounds/lilfesfx.bnk",
        "GameData/Sounds/game.spl",
        "GameData/Sounds/replay.spl",
        "GameData/Speech/zzzxeng.viv",
        "GameData/Music/zzzymus.viv",
        "GameData/Track/castle_audio.scn",
        "GameData/CarModel/carcmn.fsh",
        "GameData/CarModel/packages.ini",
        "GameData/Render/flare.dat",
        "GameData/Render/particle.dat",
        "GameData/Render/carpart.dat",
        "GameData/DashHud/pic16.fsh",
        "FEData/Data/nfs5.car",
        "FEData/Data/nfs5.prt",
        "FEData/Data/nfs5.per",
        "FEData/Data/nfs5.fac",
        "FEData/Data/nfs5.trk",
        "FEData/Data/nfs5.trn",
        "FEData/Data/nfs5-1.hud",
        "FEData/Data/nfs5-2.hud",
    ];
    let missing: Vec<&str> = direct.iter().copied().filter(|r| !exact(&root, r)).collect();
    assert!(missing.is_empty(), "not found with exactly this case under {}: {missing:?}", root.display());
}

/// `find_file` must keep recovering from a wrong case — the engine sound of the `boxster25` sim is
/// `Boxster25.spl` on disk.
#[test]
fn find_file_recovers_a_wrong_case() {
    let Some(root) = dev_game_root() else { return };
    let dir = root.join("GameData").join("Sounds");
    if exact(&root, "GameData/Sounds/Boxster25.spl") {
        let p = nfs_assets::GamePaths::find_file(&dir, "boxster25.spl").expect("case-insensitive lookup");
        assert!(p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("Boxster25.spl")));
    }
}
