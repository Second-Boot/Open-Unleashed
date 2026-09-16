//! Track records (`RE/notes/modes.md` §6, CONFIRMED): one 8-entry table per track seeded from
//! `nfs5.trk`, holding total race times on point-to-point tracks and best laps on circuits, in
//! 64 Hz ticks. Only the human player enters the table; insertion is a strict `<` with shift
//! (`Records_insert` 0x4FAFC0). The original persists the table as `SaveData\nfs5.trk`; this port
//! keeps a JSON copy next to the career save.

use nfs_assets::GamePaths;
use nfs_formats::career::parse_track_table;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecordEntry {
    pub driver: String,
    pub car: String,
    /// 64 Hz ticks.
    pub time: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrackRecords {
    pub track_id: u32,
    pub file: String,
    pub circuit: bool,
    pub entries: Vec<RecordEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Records {
    pub tracks: Vec<TrackRecords>,
}

/// `Time_format` 0x4356E0: `m:ss.hh` with `hh = (ticks & 63) · 100 / 64`.
pub fn format_ticks(ticks: u64) -> String {
    let s = ticks / 64;
    let (m, sec) = (s / 60, s % 60);
    let hh = ((ticks & 63) * 100) / 64;
    if m > 60 { "59:59.99".to_string() } else { format!("{m}:{sec:02}.{hh:02}") }
}

impl Records {
    pub fn default_save_path() -> PathBuf {
        let base = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        base.join("Library").join("Application Support").join("PorscheUnleashed").join("records.json")
    }

    /// Defaults from `FEData/Data/nfs5.trk`, overlaid by the saved table when present.
    pub fn load(paths: &GamePaths, save: &Path) -> Self {
        let defaults = std::fs::read(paths.fe_data().join("Data").join("nfs5.trk"))
            .ok()
            .and_then(|d| parse_track_table(&d).ok())
            .map(|rows| Records {
                tracks: rows
                    .iter()
                    .map(|t| TrackRecords {
                        track_id: t.id,
                        file: t.file.to_lowercase(),
                        circuit: t.circuit,
                        entries: t.records.iter().map(|r| RecordEntry { driver: r.driver.clone(), car: r.car.clone(), time: r.time }).collect(),
                    })
                    .collect(),
            })
            .unwrap_or_default();
        match std::fs::read(save).ok().and_then(|d| serde_json::from_slice::<Records>(&d).ok()) {
            Some(mut saved) => {
                // tracks missing from the save keep their defaults
                for t in &defaults.tracks {
                    if !saved.tracks.iter().any(|s| s.track_id == t.track_id) {
                        saved.tracks.push(t.clone());
                    }
                }
                saved
            }
            None => defaults,
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)
    }

    pub fn track(&self, file: &str) -> Option<&TrackRecords> {
        self.tracks.iter().find(|t| t.file.eq_ignore_ascii_case(file))
    }

    /// Insert the player's time for `file`: first index with `time < entry.time` (strict), shift,
    /// at most 8 entries. Returns the 0-based rank when the time entered the table.
    pub fn insert(&mut self, file: &str, driver: &str, car: &str, time: u32) -> Option<usize> {
        let t = self.tracks.iter_mut().find(|t| t.file.eq_ignore_ascii_case(file))?;
        let k = t.entries.iter().position(|e| time < e.time).unwrap_or(t.entries.len());
        if k >= 8 {
            return None;
        }
        t.entries.insert(k, RecordEntry { driver: driver.to_string(), car: car.chars().take(17).collect(), time });
        t.entries.truncate(8);
        Some(k)
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

    #[test]
    fn insert_strict_with_shift() {
        let mut r = Records { tracks: vec![TrackRecords { track_id: 8, file: "coastal".into(), circuit: false, entries: (0..8).map(|i| RecordEntry { driver: format!("d{i}"), car: "c".into(), time: 1000 + i * 100 }).collect() }] };
        assert_eq!(r.insert("coastal", "me", "993", 1000), Some(1), "equal time goes behind the holder");
        assert_eq!(r.tracks[0].entries.len(), 8);
        assert_eq!(r.tracks[0].entries[1].driver, "me");
        assert_eq!(r.tracks[0].entries[7].driver, "d6", "last entry dropped");
        assert_eq!(r.insert("coastal", "slow", "993", 5000), None);
        assert_eq!(r.insert("coastal", "fast", "993", 10), Some(0));
        assert_eq!(r.insert("nowhere", "x", "y", 1), None);
    }
}
