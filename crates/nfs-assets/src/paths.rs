//! Locate the (read-only) original game installation. See `docs/en/game-data.md`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GamePaths {
    pub root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("original game data not found: copy the GameData and FEData folders of your own installed copy into `game/` beside the executable, or set NFS5_GAME_PATH to the folder that holds them (see README). Tried: {tried:?}")]
pub struct GameNotFound {
    pub tried: Vec<PathBuf>,
}

impl GamePaths {
    /// Resolve the game root: `$NFS5_GAME_PATH`, then `game/` beside the executable, an app bundle's own
    /// `Resources/game`, then the documented default locations ([`crate::platform::game_data_candidates`]).
    pub fn locate() -> Result<Self, GameNotFound> {
        let mut tried = Vec::new();
        if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
            let p = PathBuf::from(p);
            if Self::looks_like_root(&p) {
                return Ok(Self { root: p });
            }
            tried.push(p);
        }
        // `game/` beside the executable (Windows / Linux packages), a macOS bundle's
        // `Contents/Resources/game`, then the developer's home fallbacks.
        for p in crate::platform::game_data_candidates() {
            if Self::looks_like_root(&p) {
                return Ok(Self { root: p.canonicalize().unwrap_or(p) });
            }
            tried.push(p);
        }
        Err(GameNotFound { tried })
    }

    fn looks_like_root(p: &Path) -> bool {
        p.join("GameData").is_dir() && p.join("FEData").is_dir()
    }

    pub fn game_data(&self) -> PathBuf {
        self.root.join("GameData")
    }
    pub fn fe_data(&self) -> PathBuf {
        self.root.join("FEData")
    }
    pub fn track_dir(&self) -> PathBuf {
        self.game_data().join("Track")
    }
    pub fn car_model_dir(&self) -> PathBuf {
        self.game_data().join("CarModel")
    }
    /// `GameData/Render` — the shared effect textures (`common.fsh`: `SKDP` tyre dust, `SHAD`,
    /// `GLAR`, `SUNA`, `SPLH`/`SPLG`, `HEAD`, …).
    pub fn render_dir(&self) -> PathBuf {
        self.game_data().join("Render")
    }
    pub fn car_data_dir(&self) -> PathBuf {
        self.game_data().join("Simulation").join("CarData")
    }
    /// The install's registry exports, merged in name order: every `*.reg` in `regs/` beside the
    /// game root (`game_info/regs` for `game_info/data`) or inside it. The port has no Windows
    /// registry; these files hold the values the installer wrote there. Empty when none exist.
    pub fn registry(&self) -> nfs_formats::regfile::RegFile {
        let mut out = nfs_formats::regfile::RegFile::default();
        let dirs = [self.root.parent().map(|p| p.join("regs")), Some(self.root.join("regs"))];
        for dir in dirs.into_iter().flatten() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            let mut files: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("reg")))
                .collect();
            files.sort();
            for f in files {
                if let Ok(b) = std::fs::read(&f) {
                    out.merge(nfs_formats::regfile::RegFile::parse(&b));
                }
            }
        }
        out
    }

    /// **0x4B73F0**'s language index: the `Language` string under the game's key
    /// ([`nfs_formats::regfile::GAME_KEY`]), mapped by [`nfs_formats::regfile::language_index`]
    /// (missing → `"English"` → 0). This install's `reg.reg` says `"English"`.
    pub fn language(&self) -> u16 {
        let reg = self.registry();
        nfs_formats::regfile::language_index(reg.value(nfs_formats::regfile::GAME_KEY, "Language"))
    }

    /// Case-insensitive lookup of a file inside `dir` (the original data mixes cases, and macOS
    /// default volumes are case-insensitive but we do not rely on that).
    pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
        nfs_formats::ci_file::find_file(dir, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This install's `game_info/regs/reg.reg` holds `"Language"="English"` under the game's key.
    #[test]
    fn the_install_registry_language() {
        let Ok(p) = GamePaths::locate() else { return };
        let reg = p.registry();
        if p.root.parent().is_some_and(|d| d.join("regs").join("reg.reg").is_file()) {
            let v = reg.value(nfs_formats::regfile::GAME_KEY, "Language");
            assert_eq!(v, Some(&nfs_formats::regfile::RegValue::Sz("English".into())));
        }
        assert_eq!(p.language(), 0);
    }
}
