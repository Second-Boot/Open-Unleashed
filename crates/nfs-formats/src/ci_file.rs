//! Case-insensitive file lookup: the original's data mixes cases (`Boxster25.spl` for the
//! `boxster25` sim, `Coastal_Audio.scn`, …) and was made for Windows; Linux file systems are
//! case-sensitive, so a name built from data must be resolved against the directory listing.

use std::path::{Path, PathBuf};

/// `dir/name` when it exists with exactly that spelling, else the directory entry whose name
/// equals `name` ignoring ASCII case.
pub fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let rd = std::fs::read_dir(dir).ok()?;
    rd.flatten().map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).map(|n| n.eq_ignore_ascii_case(name)).unwrap_or(false))
}
