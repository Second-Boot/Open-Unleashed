//! The race HUD's option files — the profile's `<Name>1.hud` (cfg1) and `<Name>2.hud` (cfg2).
//!
//! `HUD_init` 0x41E170 → 0x41F7A0 reads `<SaveData\><Name>1.hud` and `<Name>2.hud` (the profile
//! name `[0x691DC8]`, 0x501270), each falling back to `XXhud1XX.hud` / `XXhud2XX.hud` (a copy of the
//! shipped `FEData/Data/nfs5-1.hud` / `nfs5-2.hud`, byte for byte on the server's install) — two
//! record tables of equal count: cfg1 records are `0x1E0` bytes and start with the layout's name
//! (`"Config 1"` … `"Config 4"`, the table 0x5CCB38), cfg2 records `0x68` bytes: `+0x28` / `+0x2C` the
//! record's resolution and `+0x30 …` the fourteen **Display options** (SPEEDOMETER, SPEEDUNITS,
//! HUDTACHOMETER, DAMAGEDISPLAY, MAP, MAPSCALE, OPPONENTLIST, SPLITTIMES, DISPLAYCOLORS,
//! DRIVERSNAPSHOT, TRANSLUCENCY, REARVIEWMIRRROR, RECORDTIME, GEAR). The record in use is the first
//! one whose cfg1 name is the chosen `Display.ScreenLayout`'s and whose resolution is the race's
//! (0x41F7A0, `[0x60C95C]`); none: 0x41EBF0 appends a copy of the layout's own 640×480 record
//! (`[0x60C958]` = the layout index) with the resolution written into `+0x28` / `+0x2C` (cfg1: the
//! name written again) and rewrites both files. The Display screen fills its fourteen widgets from
//! that record on entry (0x4CB6A0 → 0x421030 … 0x4211B0) and writes them back through 0x4CB860
//! (setters 0x420E10 … 0x421010) and 0x41EA50 (both files, `[0x60C964]` / `[0x60C968]`) when it is
//! applied (slot 9 0x4CB820) and **when the layout combo changes** (0x4CB9A0: the old layout's
//! options are stored, HUD_init runs for the new one and the widgets are refilled from its record).
//!
//! This module is the tables and their rules; `main.rs` decides when to read and write them.

use std::path::{Path, PathBuf};

/// cfg1 record size (0x41F7A0's `/ 0x1E0`).
pub const CFG1_REC: usize = 0x1E0;
/// cfg2 record size (0x41EA50's `* 0x68`).
pub const CFG2_REC: usize = 0x68;
/// Offset of the fourteen option words in a cfg2 record (`+0x30`).
pub const OPT_OFF: usize = 0x30;
/// Number of Display options in a record.
pub const OPT_COUNT: usize = 14;

/// A layout's name in cfg1 (0x5CCB38: "Config 1" … "Config 4").
pub fn layout_name(layout: usize) -> String {
    format!("Config {}", layout.min(3) + 1)
}

/// The two tables as loaded (or as the shipped files are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudCfg {
    pub cfg1: Vec<u8>,
    pub cfg2: Vec<u8>,
}

impl HudCfg {
    /// Records in both tables (the original takes cfg1's count for both, 0x41F7A0).
    pub fn count(&self) -> usize {
        (self.cfg1.len() / CFG1_REC).min(self.cfg2.len() / CFG2_REC)
    }

    fn name_of(&self, i: usize) -> &[u8] {
        let r = &self.cfg1[i * CFG1_REC..(i + 1) * CFG1_REC];
        let n = r.iter().position(|b| *b == 0).unwrap_or(r.len());
        &r[..n]
    }

    fn word(&self, i: usize, off: usize) -> u32 {
        let o = i * CFG2_REC + off;
        u32::from_le_bytes([self.cfg2[o], self.cfg2[o + 1], self.cfg2[o + 2], self.cfg2[o + 3]])
    }

    /// 0x41F7A0's search: the first record named `layout`'s whose resolution is `(w, h)`.
    pub fn find(&self, layout: usize, w: u32, h: u32) -> Option<usize> {
        let name = layout_name(layout);
        (0..self.count()).find(|&i| self.name_of(i) == name.as_bytes() && self.word(i, 0x28) == w && self.word(i, 0x2C) == h)
    }

    /// 0x41EBF0: no record for `(layout, w, h)` — append a copy of record `layout` (the layout's own
    /// 640×480 one) to both tables, the name written into the cfg1 copy and the resolution into
    /// cfg2's `+0x28` / `+0x2C`. Returns the new record's index.
    ///
    /// **APPROXIMATION:** 0x41EBF0 then runs 0x41EFF0 on the pair, which re-derives the new record's
    /// anchored positions for `(w, h)`; those fields stay a copy here (the port takes the positions from
    /// its own anchor rule, `Hud::anchor_passes`, and reads none of them from the file).
    pub fn create(&mut self, layout: usize, w: u32, h: u32) -> usize {
        let n = self.count();
        let src = layout.min(n.saturating_sub(1));
        // both tables stay whole records and of one count
        self.cfg1.truncate(n * CFG1_REC);
        self.cfg2.truncate(n * CFG2_REC);
        let mut r1 = self.cfg1[src * CFG1_REC..(src + 1) * CFG1_REC].to_vec();
        let name = layout_name(layout);
        r1[..name.len()].copy_from_slice(name.as_bytes());
        r1[name.len()] = 0;
        let mut r2 = self.cfg2[src * CFG2_REC..(src + 1) * CFG2_REC].to_vec();
        r2[0x28..0x2C].copy_from_slice(&w.to_le_bytes());
        r2[0x2C..0x30].copy_from_slice(&h.to_le_bytes());
        self.cfg1.extend_from_slice(&r1);
        self.cfg2.extend_from_slice(&r2);
        n
    }

    /// The record's fourteen options.
    pub fn opts(&self, i: usize) -> [u32; OPT_COUNT] {
        std::array::from_fn(|k| self.word(i, OPT_OFF + 4 * k))
    }

    /// 0x4CB860's setter for option `slot`.
    pub fn set_opt(&mut self, i: usize, slot: usize, v: u32) {
        let o = i * CFG2_REC + OPT_OFF + 4 * slot;
        self.cfg2[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// The options of `(layout, w, h)`, or — the record not made yet — of the layout's own record
    /// (what 0x41EBF0's copy starts with).
    pub fn opts_for(&self, layout: usize, w: u32, h: u32) -> Option<[u32; OPT_COUNT]> {
        let n = self.count();
        if n == 0 {
            return None;
        }
        Some(self.opts(self.find(layout, w, h).unwrap_or(layout.min(n - 1))))
    }

    /// 0x41E170's HUD_init for `(layout, w, h)`: the record's index, made when missing.
    pub fn record(&mut self, layout: usize, w: u32, h: u32) -> usize {
        match self.find(layout, w, h) {
            Some(i) => i,
            None => self.create(layout, w, h),
        }
    }
}

/// `<dir><name>1.hud` / `<dir><name>2.hud` (0x41F7A0's `%s%s%s`).
pub fn profile_paths(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    (dir.join(format!("{name}1.hud")), dir.join(format!("{name}2.hud")))
}

/// Load the tables: each file of the profile when it exists, else the shipped one (the `XXhud?XX`
/// fallback). Returns whether the profile's cfg2 file existed — the master of the options once the
/// Display screen has been applied.
pub fn load(profile: Option<(&Path, &str)>, shipped_dir: &Path) -> Option<(HudCfg, bool)> {
    let read = |own: Option<PathBuf>, shipped: &str| -> Option<(Vec<u8>, bool)> {
        if let Some(p) = own
            && let Ok(d) = std::fs::read(&p)
            && !d.is_empty()
        {
            return Some((d, true));
        }
        std::fs::read(shipped_dir.join(shipped)).ok().map(|d| (d, false))
    };
    let (p1, p2) = match profile {
        Some((dir, name)) => {
            let (a, b) = profile_paths(dir, name);
            (Some(a), Some(b))
        }
        None => (None, None),
    };
    let (cfg1, _) = read(p1, "nfs5-1.hud")?;
    let (cfg2, from_profile) = read(p2, "nfs5-2.hud")?;
    Some((HudCfg { cfg1, cfg2 }, from_profile))
}

/// 0x41EA50: write both tables to the profile's files.
pub fn save(cfg: &HudCfg, dir: &Path, name: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let (p1, p2) = profile_paths(dir, name);
    std::fs::write(p1, &cfg.cfg1[..cfg.count() * CFG1_REC])?;
    std::fs::write(p2, &cfg.cfg2[..cfg.count() * CFG2_REC])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> HudCfg {
        let mut cfg1 = Vec::new();
        let mut cfg2 = Vec::new();
        for (i, o) in [[2u32, 0, 2, 1, 1, 0, 1, 1, 0, 0, 0, 1, 1, 1], [2, 0, 2, 3, 3, 0, 1, 1, 0, 0, 0, 1, 1, 1], [2, 0, 2, 2, 1, 0, 2, 1, 1, 0, 1, 0, 1, 1], [1, 0, 1, 0, 1, 0, 0, 1, 1, 0, 1, 0, 1, 0]].iter().enumerate() {
            let mut r1 = vec![0u8; CFG1_REC];
            let n = layout_name(i);
            r1[..n.len()].copy_from_slice(n.as_bytes());
            cfg1.extend(r1);
            let mut r2 = vec![0u8; CFG2_REC];
            r2[0x28..0x2C].copy_from_slice(&640u32.to_le_bytes());
            r2[0x2C..0x30].copy_from_slice(&480u32.to_le_bytes());
            for (k, v) in o.iter().enumerate() {
                r2[OPT_OFF + 4 * k..OPT_OFF + 4 * k + 4].copy_from_slice(&v.to_le_bytes());
            }
            cfg2.extend(r2);
        }
        HudCfg { cfg1, cfg2 }
    }

    #[test]
    fn layouts_keep_their_own_options() {
        let mut c = shipped();
        // the shipped Config 2 has the damage display on its third choice and the map rotating
        assert_eq!(c.opts_for(1, 640, 480).unwrap()[3], 3);
        let i = c.record(0, 640, 480);
        c.set_opt(i, 5, 4);
        assert_eq!(c.opts_for(0, 640, 480).unwrap()[5], 4, "Config 1's MAPSCALE");
        assert_eq!(c.opts_for(1, 640, 480).unwrap()[5], 0, "Config 2 keeps its own");
    }

    #[test]
    fn a_new_resolution_copies_the_layouts_record() {
        let mut c = shipped();
        let i = c.record(2, 640, 480);
        c.set_opt(i, 7, 0);
        assert_eq!(c.count(), 4);
        // 0x41EBF0: a copy of record `layout` with the resolution; the user's change is in it
        let j = c.record(2, 800, 600);
        assert_eq!((c.count(), j), (5, 4));
        assert_eq!(c.opts(j)[7], 0);
        assert_eq!(c.find(2, 800, 600), Some(4));
        // later edits at 800×600 do not reach the 640×480 record
        c.set_opt(j, 7, 1);
        assert_eq!(c.opts_for(2, 640, 480).unwrap()[7], 0);
        assert_eq!(c.opts_for(2, 800, 600).unwrap()[7], 1);
        assert_eq!(c.cfg1.len() / CFG1_REC, c.cfg2.len() / CFG2_REC);
    }

    #[test]
    fn files_round_trip() {
        let dir = std::env::temp_dir().join(format!("hud_cfg_test_{}", std::process::id()));
        let mut c = shipped();
        let i = c.record(1, 800, 600);
        c.set_opt(i, 0, 1);
        save(&c, &dir, "DRIVER").unwrap();
        let (back, from_profile) = load(Some((&dir, "DRIVER")), &dir).unwrap();
        assert!(from_profile);
        assert_eq!(back, c);
        std::fs::remove_dir_all(&dir).ok();
    }
}
