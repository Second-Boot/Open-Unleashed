//! Locate the (read-only) original game installation. See `GAME_PATH.md`.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct GamePaths {
    pub root: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("original game installation not found; set NFS5_GAME_PATH (see GAME_PATH.md). Tried: {tried:?}")]
pub struct GameNotFound {
    pub tried: Vec<PathBuf>,
}

impl GamePaths {
    /// Resolve the game root: `$NFS5_GAME_PATH`, then the documented default locations.
    pub fn locate() -> Result<Self, GameNotFound> {
        let mut tried = Vec::new();
        if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
            let p = PathBuf::from(p);
            if Self::looks_like_root(&p) {
                return Ok(Self { root: p });
            }
            tried.push(p);
        }
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            for rel in ["Games/NFS-Porsche-Unleashed/game_info/data", "Games/NFS-Porsche-Unleashed"] {
                let p = home.join(rel);
                if Self::looks_like_root(&p) {
                    return Ok(Self { root: p });
                }
                tried.push(p);
            }
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
    /// Case-insensitive lookup of a file inside `dir` (the original data mixes cases, and macOS
    /// default volumes are case-insensitive but we do not rely on that).
    pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        let rd = std::fs::read_dir(dir).ok()?;
        rd.flatten().map(|e| e.path()).find(|p| {
            p.file_name().and_then(|n| n.to_str()).map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false)
        })
    }
}
