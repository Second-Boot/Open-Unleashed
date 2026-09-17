//! Track records (`RE/notes/modes.md` §6 and `RE/notes/savegame.md` §7, CONFIRMED): one 8-entry
//! table per track, holding total race times on point-to-point tracks and best laps on circuits, in
//! 64 Hz ticks. Only the human player enters the table; insertion is a strict `<` with a shift
//! (`Records_insert` 0x4FAFC0).
//!
//! Persistence is the original's: the whole `nfs5.trk` array is written back to `SaveData\nfs5.trk`
//! (`TrackRecords` save module 0x4FAD80), seeded from `FEData\Data\nfs5.trk` when the file is
//! missing (0x4FAE20), and the records are **global across profiles** because the module puts no
//! bytes into the `.sav`. The only deviation is the directory: the original writes inside its own
//! installation, which this port treats as read-only reference material, so the file lives under
//! [`crate::save::save_dir`].

use nfs_assets::GamePaths;
use nfs_formats::career::{parse_track_table, TrackRecord, TrackRecordFile};
use std::path::{Path, PathBuf};

pub use nfs_formats::career::NO_RECORD_TIME;

/// One track's records, as the HUD and the results screen want them.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecords {
    pub track_id: u32,
    pub file: String,
    pub circuit: bool,
    pub entries: Vec<TrackRecord>,
}

/// `Time_format` 0x4356E0: `m:ss.hh` with `hh = (ticks & 63) · 100 / 64`.
pub fn format_ticks(ticks: u64) -> String {
    let s = ticks / 64;
    let (m, sec) = (s / 60, s % 60);
    let hh = ((ticks & 63) * 100) / 64;
    if m > 60 { "59:59.99".to_string() } else { format!("{m}:{sec:02}.{hh:02}") }
}

/// The `nfs5.trk` array plus the per-track metadata needed to address it by track file name.
#[derive(Debug, Clone)]
pub struct Records {
    /// The raw record array, exactly as `SaveData\nfs5.trk` holds it.
    pub file: TrackRecordFile,
    /// `(record index, track id, base file name, circuit flag)` for every record.
    meta: Vec<(usize, u32, String, bool)>,
}

impl Default for Records {
    fn default() -> Self {
        Self { file: TrackRecordFile { raw: Vec::new() }, meta: Vec::new() }
    }
}

impl Records {
    pub fn default_save_path() -> PathBuf {
        crate::save::save_dir().join("nfs5.trk")
    }

    /// 0x4FAE20: seed `SaveData\nfs5.trk` from `FEData\Data\nfs5.trk` when it does not exist, read
    /// it back and apply the empty-slot fixup. A save that cannot be read falls back to the seed.
    pub fn load(paths: &GamePaths, save: &Path) -> Self {
        let seed_path = paths.fe_data().join("Data").join("nfs5.trk");
        let seed = std::fs::read(&seed_path).ok();
        let data = match std::fs::read(save) {
            Ok(d) if !d.is_empty() => Some(d),
            _ => seed.clone(),
        };
        let raw = data.and_then(|d| TrackRecordFile::load(&d).ok()).or_else(|| seed.as_deref().and_then(|d| TrackRecordFile::load(d).ok()));
        let Some(file) = raw else { return Self::default() };
        // The record layout (ids, file names, the circuit flag) comes from the shipped table; the
        // save only ever differs in the record-holder slots.
        let meta = seed
            .as_deref()
            .and_then(|d| parse_track_table(d).ok())
            .map(|rows| rows.iter().enumerate().map(|(i, t)| (i, t.id, t.file.to_lowercase(), t.circuit)).collect())
            .unwrap_or_default();
        Self { file, meta }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if self.file.raw.is_empty() {
            return Ok(());
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, &self.file.raw)
    }

    fn find(&self, file: &str) -> Option<&(usize, u32, String, bool)> {
        self.meta.iter().find(|m| m.2.eq_ignore_ascii_case(file))
    }

    pub fn track(&self, file: &str) -> Option<TrackRecords> {
        let (i, id, name, circuit) = self.find(file)?;
        Some(TrackRecords { track_id: *id, file: name.clone(), circuit: *circuit, entries: self.file.entries(*i) })
    }

    /// Base file names of the circuit tracks (`trk+0x6E == 1`).
    pub fn circuits(&self) -> Vec<String> {
        self.meta.iter().filter(|m| m.3).map(|m| m.2.clone()).collect()
    }

    /// Insert the player's time for `file`; returns the 0-based rank when it entered the table.
    /// The car name is clipped the way the original's 52-byte field does.
    pub fn insert(&mut self, file: &str, driver: &str, car: &str, time: u32) -> Option<usize> {
        let id = self.find(file)?.1 as u16;
        self.file.insert(id, driver, car, time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_matches_the_original() {
        assert_eq!(format_ticks(10496), "2:44.00");
        assert_eq!(format_ticks(3999), "1:02.48");
        assert_eq!(format_ticks(11350), "2:57.34");
        assert_eq!(format_ticks(63), "0:00.98");
    }

    /// The insert/shift rule itself is tested against the shipped table in
    /// `nfs-formats/tests/real_data.rs`; here only the name → record lookup.
    #[test]
    fn lookup_by_track_file_name() {
        let mut raw = vec![0u8; 2 * 0x4A8];
        raw[0..2].copy_from_slice(&8u16.to_le_bytes());
        raw[0x4A8..0x4A8 + 2].copy_from_slice(&9u16.to_le_bytes());
        let mut r = Records { file: TrackRecordFile::load(&raw).unwrap(), meta: vec![(0, 8, "coastal".into(), false), (1, 9, "forest".into(), true)] };
        assert_eq!(r.track("COASTAL").unwrap().entries[0].driver, "n/a");
        assert_eq!(r.circuits(), ["forest"]);
        assert_eq!(r.insert("coastal", "ME", "911", 5000), Some(0));
        assert_eq!(r.track("coastal").unwrap().entries[0].time, 5000);
        assert_eq!(r.track("coastal").unwrap().entries[1].time, NO_RECORD_TIME);
        assert_eq!(r.insert("nowhere", "ME", "911", 5000), None);
    }
}
