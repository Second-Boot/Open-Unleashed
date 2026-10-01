//! Per-OS directories: where the port keeps its saves and where it looks for the game data.
//! (Platform code only — nothing here is a rule of the original.)

use std::path::PathBuf;

/// The user's home directory (`HOME`; on Windows `USERPROFILE`).
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The port's own data directory, the parent of `SaveData` and `career.json`.
///
/// * macOS: `~/Library/Application Support/PorscheUnleashed`
/// * Windows: `%APPDATA%\PorscheUnleashed`
/// * Linux and the rest: `$XDG_DATA_HOME/PorscheUnleashed`, else `~/.local/share/PorscheUnleashed`
///
/// Falls back to `./PorscheUnleashed` when no home can be found. `NFS5_SAVE_DIR` is handled by the
/// callers (it replaces `SaveData` itself).
pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        let base = home_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("Library").join("Application Support").join("PorscheUnleashed")
    } else if cfg!(windows) {
        let base = std::env::var_os("APPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join("AppData").join("Roaming")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("PorscheUnleashed")
    } else {
        let base = std::env::var_os("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local").join("share")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("PorscheUnleashed")
    }
}

/// The places the game data may live, in search order (the first that holds `GameData` and
/// `FEData` wins): `game/` beside the executable (Windows / Linux packages), the macOS bundle's
/// `../Resources/game`, then the developer's `~/Games/NFS-Porsche-Unleashed[/game_info/data]`.
pub fn game_data_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        v.push(dir.join("game"));
        v.push(dir.join("../Resources/game"));
    }
    if let Some(home) = home_dir() {
        v.push(home.join("Games/NFS-Porsche-Unleashed/game_info/data"));
        v.push(home.join("Games/NFS-Porsche-Unleashed"));
    }
    v
}

/// The developer's install, for tests and examples (`$NFS5_GAME_PATH`, else the home fallbacks);
/// `None` when absent.
pub fn dev_game_root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NFS5_GAME_PATH").map(PathBuf::from)
        && p.join("GameData").is_dir()
    {
        return Some(p);
    }
    let home = home_dir()?;
    ["Games/NFS-Porsche-Unleashed/game_info/data", "Games/NFS-Porsche-Unleashed"].iter().map(|r| home.join(r)).find(|p| p.join("GameData").is_dir())
}
