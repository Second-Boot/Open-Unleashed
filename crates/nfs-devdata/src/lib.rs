//! Where the developer's copy of the original game data lives — the one rule for tests and examples
//! (nothing here is a rule of the original). A dependency of the `[dev-dependencies]` of the crates below
//! `nfs-assets` (`nfs-formats`, `nfs-audio`, ...), and of `nfs_assets::platform` (which re-exports it).
//! Without the data every test using it skips itself ([`need_game!`]).

use std::path::PathBuf;

/// The default install folders under the user's home, in search order (the data root holds `GameData`).
pub const HOME_FALLBACKS: [&str; 2] = ["Games/NFS-Porsche-Unleashed/game_info/data", "Games/NFS-Porsche-Unleashed"];

/// The user's home directory (`HOME`; on Windows `USERPROFILE`).
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The developer's install, for tests and examples (`$NFS5_GAME_PATH`, else the home fallbacks, the first
/// that holds `GameData`); `None` when absent.
pub fn dev_game_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NFS5_GAME_PATH").map(PathBuf::from)
        && p.join("GameData").is_dir()
    {
        return Some(p);
    }
    let home = home_dir()?;
    HOME_FALLBACKS.iter().map(|r| home.join(r)).find(|p| p.join("GameData").is_dir())
}

/// The game root, or print `skipping: original game data not found` and `return` from the calling test.
#[macro_export]
macro_rules! need_game {
    () => {
        match $crate::dev_game_root() {
            Some(r) => r,
            None => {
                eprintln!("skipping: original game data not found");
                return;
            }
        }
    };
}
