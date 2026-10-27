//! Per-OS directories: where the port keeps its saves and where it looks for the game data.
//! (Platform code only — nothing here is a rule of the original.)

use std::path::PathBuf;

/// The user's home directory (`HOME`; on Windows `USERPROFILE`).
pub fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The port's own data directory, the parent of `SaveData` (and of the retired `career.json`,
/// read once by the migration `nfs_game::save::migrate_legacy_json`).
///
/// * macOS: `~/Library/Application Support/PorscheUnleashed`
/// * Windows: `%APPDATA%\PorscheUnleashed`
/// * Linux and the rest: `$XDG_DATA_HOME/PorscheUnleashed`, else `~/.local/share/PorscheUnleashed`
///
/// Falls back to `./PorscheUnleashed` when no home can be found. `NFS5_SAVE_DIR` is handled by the
/// callers (it replaces `SaveData` itself).
pub fn data_dir() -> PathBuf {
    // PORT-DEVIATION D41 (RE/notes/PORT-DEVIATIONS.md): the saves live in the per-OS data folder, not in the
    // original's `.\SaveData\` (install.txt directory entry 48, `[0x65B360]`) under the game folder, and an
    // existing original profile is not imported.
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
/// `../Resources/game`, `game/` beside the `.app` (macOS packages without the data), then
/// `~/Games/NFS-Porsche-Unleashed[/game_info/data]`.
pub fn game_data_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
        v.push(dir.join("game"));
        v.push(dir.join("../Resources/game"));
        if cfg!(target_os = "macos") {
            v.push(dir.join("../../../game"));
        }
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

/// PORT-DEVIATION D46 (photo mode): the folder the stills go to, `<the user's Pictures>/PorscheUnleashed`.
///
/// * macOS: `~/Pictures`
/// * Windows: the Pictures known folder (`SHGetKnownFolderPath(FOLDERID_Pictures)`), so a folder moved by the
///   user or by OneDrive is followed; `%USERPROFILE%\Pictures` only when the shell gives none
/// * Linux and the rest: `XDG_PICTURES_DIR` of `~/.config/user-dirs.dirs` (`$XDG_CONFIG_HOME`), else `~/Pictures`
///
/// Falls back to `./PorscheUnleashed` with no home. The folder is made by the caller. `NFS_PHOTO_DIR` (tests)
/// names the folder itself.
pub fn photo_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("NFS_PHOTO_DIR").filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    let Some(home) = home_dir() else { return PathBuf::from("PorscheUnleashed") };
    let pictures = if cfg!(windows) {
        known_pictures().unwrap_or_else(|| home.join("Pictures"))
    } else if cfg!(target_os = "macos") {
        home.join("Pictures")
    } else {
        let conf = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        std::fs::read_to_string(conf.join("user-dirs.dirs")).ok().and_then(|t| xdg_pictures_dir(&t, &home)).unwrap_or_else(|| home.join("Pictures"))
    };
    pictures.join("PorscheUnleashed")
}

/// Windows: the user's Pictures folder as the shell knows it (Explorer's "Pictures"); `None` when the call fails.
#[cfg(windows)]
fn known_pictures() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    #[repr(C)]
    struct Guid(u32, u16, u16, [u8; 8]);
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHGetKnownFolderPath(id: *const Guid, flags: u32, token: *mut core::ffi::c_void, path: *mut *mut u16) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(p: *mut core::ffi::c_void);
    }
    // FOLDERID_Pictures {33E28130-4E1E-4676-835A-98395C3BC3BB}
    const PICTURES: Guid = Guid(0x33E2_8130, 0x4E1E, 0x4676, [0x83, 0x5A, 0x98, 0x39, 0x5C, 0x3B, 0xC3, 0xBB]);
    let mut raw: *mut u16 = core::ptr::null_mut();
    // KF_FLAG_DEFAULT; the buffer is the caller's to free even when the call fails
    let hr = unsafe { SHGetKnownFolderPath(&PICTURES, 0, core::ptr::null_mut(), &mut raw) };
    let path = (hr >= 0 && !raw.is_null()).then(|| {
        let len = (0..).take_while(|&i| unsafe { *raw.add(i) } != 0).count();
        PathBuf::from(std::ffi::OsString::from_wide(unsafe { core::slice::from_raw_parts(raw, len) }))
    });
    unsafe { CoTaskMemFree(raw.cast()) };
    path.filter(|p| p.is_absolute())
}

#[cfg(not(windows))]
fn known_pictures() -> Option<PathBuf> {
    None
}

/// `XDG_PICTURES_DIR="$HOME/Pictures"` of a `user-dirs.dirs` text, with `$HOME` / `${HOME}` taken for `home`;
/// `None` when the line is missing or names a relative path.
pub fn xdg_pictures_dir(text: &str, home: &std::path::Path) -> Option<PathBuf> {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("XDG_PICTURES_DIR"))?;
    let v = line.split_once('=')?.1.trim().trim_matches('"');
    let h = home.to_string_lossy();
    let v = v.replace("${HOME}", &h).replace("$HOME", &h);
    let p = PathBuf::from(v);
    p.is_absolute().then_some(p)
}

#[cfg(test)]
mod photo_dir_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_xdg_pictures_line() {
        let home = Path::new("/home/a");
        let t = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_PICTURES_DIR=\"$HOME/Fotos\"\n";
        assert_eq!(xdg_pictures_dir(t, home), Some(PathBuf::from("/home/a/Fotos")));
        assert_eq!(xdg_pictures_dir("XDG_PICTURES_DIR=\"/mnt/pics\"", home), Some(PathBuf::from("/mnt/pics")));
        assert_eq!(xdg_pictures_dir("XDG_PICTURES_DIR=\"${HOME}/P\"", home), Some(PathBuf::from("/home/a/P")));
        assert_eq!(xdg_pictures_dir("XDG_PICTURES_DIR=\"rel\"", home), None);
        assert_eq!(xdg_pictures_dir("XDG_MUSIC_DIR=\"$HOME/M\"", home), None);
    }
}
