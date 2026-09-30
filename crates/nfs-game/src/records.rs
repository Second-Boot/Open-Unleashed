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

/// `Time_format` 0x4356E0: `mm:ss.hh` with `hh = (ticks & 63) · 100 / 64`. Each of the three fields
/// gets a leading `"0"` of its own when it is below ten (0x4357B4, 0x43582B, 0x4358A0), and more
/// than sixty minutes clamps to `59:59.99` (0x4358E7). CONFIRMED.
pub fn format_ticks(ticks: u64) -> String {
    let s = ticks / 64;
    let (m, sec) = (s / 60, s % 60);
    let hh = ((ticks & 63) * 100) / 64;
    if m > 60 { "59:59.99".to_string() } else { format!("{m:02}:{sec:02}.{hh:02}") }
}

/// `Time_format` 0x4356E0 over a signed dword, the way the original runs it (`sign` is its third
/// argument, the flag at 0x4356F5): with the flag a `"+"` (0x5CD200, time ≥ 0) or `"-"` (0x5CD1FC)
/// opens the string; the magnitude is `|t|` (0x43572E…0x435738, `neg` for a time ≤ 0), split with
/// the truncating `fild · 1/64 · _ftol` (0x43573C…0x435746) and `idiv 60` into minutes / seconds,
/// the hundredths from `(|t| & 0x8000003F)` fixed up as a signed remainder, · 100 / 64 (0x435776…
/// 0x435797). More than 60 minutes appends `"59:59.99"` (0x4358E7) — after the sign. Every field
/// below ten gets a `"0"` first and is written with the C `itoa` (so the one absurd input,
/// `0x80000000`, whose `neg` stays negative, prints its negative fields as the original does).
/// The race-result printers pass no flag (`push 0` at 0x42B348 — the in-race table's finished
/// car's `+0xB24` —, 0x42B382, 0x42C401 / 0x42C428 — `HRaceSummary`'s BEST LAP and TIME cells —,
/// 0x42A315, 0x431796; only 0x42B488, a `fistp` difference, pushes 1): a negative time is drawn as
/// its absolute value there. CONFIRMED (2026-09-30).
pub fn format_time_dword(t: i32, sign: bool) -> String {
    let mut out = String::new();
    if sign {
        out.push(if t < 0 { '-' } else { '+' });
    }
    let v = if t > 0 { t } else { t.wrapping_neg() };
    let secs = v / 64;
    let (m, sec) = (secs / 60, secs % 60);
    // 0x435776: `and eax, 0x8000003F`; a negative result is fixed up to the signed remainder
    let mut low = (v as u32 & 0x8000_003F) as i32;
    if low < 0 {
        low = (low.wrapping_sub(1) | !0x3F).wrapping_add(1);
    }
    let hh = (low * 100) / 64;
    if m > 60 {
        out.push_str("59:59.99");
        return out;
    }
    let field = |out: &mut String, n: i32| {
        if n < 10 {
            out.push('0');
        }
        out.push_str(&n.to_string());
    };
    field(&mut out, m);
    out.push(':');
    field(&mut out, sec);
    out.push('.');
    field(&mut out, hh);
    out
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

    /// `RESETRECORDS`, 0x4FB500: 0x4FB1A0 copies `FEData\Data\nfs5.trk` over the save
    /// (`CopyFileA` with fail-if-exists 0, so it overwrites), 0x4FAFB0 frees the table and
    /// 0x4FAE20 reads it back.
    pub fn reset(paths: &GamePaths, save: &Path) -> Self {
        let seed_path = paths.fe_data().join("Data").join("nfs5.trk");
        if let Err(e) = std::fs::copy(&seed_path, save) {
            tracing::warn!(error = %e, "track records not reset");
        }
        Self::load(paths, save)
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

/// The post-race "Track Records" / "Lap Records" table, `HTrackRecords` (0x433830). CONFIRMED.
#[derive(Debug, Clone, PartialEq)]
pub struct HudRecords {
    /// `LINEAR_TRACK`: the title is 18133 "Track Records" when set, 18158 "Lap Records" when not
    /// (0x433833…0x433846, `neg / sbb / and 0xE7 / add 0x46EE`).
    pub linear: bool,
    /// the eight `(driver, car, time)` rows the table prints, after the player's insert
    pub rows: Vec<(String, String, u32)>,
    /// the row drawn selected (`HTable [+0x7AC]`), if any
    pub highlight: Option<usize>,
    /// `+0x125` / `+0x126` (0x433C73…0x433CD4): on a point-to-point track the total is not 0 and
    /// not above the first `TRACKRECORDS` after the insert (`jg` skips), on a circuit the best lap
    /// likewise against the first `LAPRECORDS` — the draw 0x4339C0 then says **kTrackRecord**
    /// (0x4A6540) or **kLapRecord** (0x4A6600, which wins) once, on its first frame fully open
    pub first: bool,
}

/// Build that table the way the HUD does, from the track's records as they were before the race.
///
/// `HTrackRecords::insertPlayer` 0x433A20 works on the race's own copies `TRACKRECORDS1..8`
/// `[0x657498]` and `LAPRECORDS1..8` `[0x6574B8]`, which 0x4ECB70 filled with the **same** eight
/// times. On a point-to-point track it inserts the player's total `car+0xCD8` into the first, on a
/// circuit the best lap `car+0xCFC` into the second, at the first slot where the time is
/// **strictly less and not zero** (0x433A58 `jge` skip, `test edx, edx / jne`) — the zero test the
/// persistent `Records_insert` 0x4FAFC0 does not have — shifting the slots below down one and the
/// names and cars with them. The draw 0x433CF0 prints the array it inserted into, the name cut to
/// 31 characters (0x433E09 `push 0x1F`), and selects the **first** row `k` with
/// `TRACKRECORDS[k] == total || LAPRECORDS[k] == best` (0x433ED9…0x433EF4) — an equality test, not
/// the insert's index.
pub fn hud_records(before: &[TrackRecord], linear: bool, driver: &str, car: &str, total: u32, best: u32) -> HudRecords {
    let mut names: Vec<(String, String)> = before.iter().map(|e| (e.driver.clone(), e.car.clone())).collect();
    let mut track: Vec<u32> = before.iter().map(|e| e.time).collect();
    let mut lap = track.clone();
    let time = if linear { total } else { best };
    let arr = if linear { &mut track } else { &mut lap };
    if let Some(k) = arr.iter().position(|e| time < *e && time != 0) {
        arr.insert(k, time);
        arr.truncate(before.len());
        names.insert(k, (driver.chars().take(31).collect(), car.to_string()));
        names.truncate(before.len());
    }
    let highlight = (0..track.len()).find(|&k| track[k] == total || lap[k] == best);
    let first = if linear { total != 0 && track.first().is_some_and(|&t| total <= t) } else { best != 0 && lap.first().is_some_and(|&t| best <= t) };
    let shown = if linear { &track } else { &lap };
    let rows = names
        .into_iter()
        .zip(shown.iter())
        .map(|((d, c), t)| (d.chars().take(31).collect(), c, *t))
        .collect();
    HudRecords { linear, rows, highlight, first }
}

#[cfg(test)]
mod tests {
    #[test]
    fn time_format_takes_a_signed_dword_like_0x4356e0() {
        use super::format_time_dword as f;
        // a positive time prints as `format_ticks` does
        for t in [0u64, 1, 63, 64, 3839, 3840, 123_456, 3_840 * 60] {
            assert_eq!(f(t as i32, false), super::format_ticks(t), "{t}");
        }
        // no flag: the absolute value, no sign; the flag adds "+" / "-" (0x5CD200 / 0x5CD1FC)
        assert_eq!(f(-580, false), "00:09.06");
        assert_eq!(f(580, true), "+00:09.06");
        assert_eq!(f(-580, true), "-00:09.06");
        assert_eq!(f(0, true), "+00:00.00");
        // more than an hour clamps, after the sign
        assert_eq!(f(-(3_840 * 61), true), "-59:59.99");
        // `0x80000000`: `neg` leaves it negative, so the minutes and seconds are negative and the
        // hundredths come out 0 (0x435776…0x435785)
        assert_eq!(f(i32::MIN, false), "0-559240:0-32.00");
    }

    use super::*;

    #[test]
    fn format_matches_the_original() {
        assert_eq!(format_ticks(10496), "02:44.00");
        assert_eq!(format_ticks(3999), "01:02.48");
        assert_eq!(format_ticks(11350), "02:57.34");
        assert_eq!(format_ticks(63), "00:00.98");
        // over an hour clamps rather than rolling over
        assert_eq!(format_ticks(61 * 60 * 64), "59:59.99");
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

    /// 0x433A20 / 0x433CF0: the player's lap goes in above the first slower one, a zero time
    /// never does, and the selected row is found by equality.
    #[test]
    fn the_hud_records_table() {
        let rec = |d: &str, t: u32| TrackRecord { driver: d.into(), car: "911".into(), time: t };
        let before = vec![rec("A", 100), rec("B", 200), rec("C", 300)];
        let h = hud_records(&before, false, "ME", "356", 9_999, 150);
        assert_eq!(h.rows.iter().map(|r| r.2).collect::<Vec<_>>(), vec![100, 150, 200]);
        assert_eq!(h.rows[1].0, "ME");
        assert_eq!(h.highlight, Some(1));
        assert!(!h.linear);
        assert!(!h.first, "second is no record");
        // equal to the first counts (`jg` skips only above it), though the strict insert leaves it out
        assert!(hud_records(&before, false, "ME", "356", 0, 100).first);
        assert!(hud_records(&before, true, "ME", "356", 99, 500).first);
        // a zero lap is not inserted (the persistent table would take it)
        let z = hud_records(&before, false, "ME", "356", 0, 0);
        assert_eq!(z.rows.iter().map(|r| r.2).collect::<Vec<_>>(), vec![100, 200, 300]);
        assert_eq!(z.highlight, None);
        assert!(!z.first, "a zero time is no record");
        // point-to-point: the total goes into the other copy
        let l = hud_records(&before, true, "ME", "356", 250, 50);
        assert_eq!(l.rows.iter().map(|r| r.2).collect::<Vec<_>>(), vec![100, 200, 250]);
        assert_eq!(l.highlight, Some(2));
    }
}
