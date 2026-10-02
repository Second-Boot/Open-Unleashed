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
//! (setters 0x420E10 … 0x421010) and 0x41EA50 (both files, `[0x60C964]` / `[0x60C968]`) when the
//! HUD widget is told its screen is no longer in use (`NOTINUSE`, slot 22 0x4CB820, gated by the flag
//! `[this+0x218]` the `INUSE` slot 21 0x4CB650 sets) and **when the layout combo changes** (0x4CB9A0: the old
//! layout's options are stored, HUD_init runs for the new one and the widgets are refilled from its
//! record). Between those moments the options live in the widgets' accessors only ([`HudCfg::staged`]).
//!
//! This module is the tables and their rules; `app/fe_choices.rs` decides when to read and write them.

use std::path::{Path, PathBuf};

/// cfg1 record size (0x41F7A0's `/ 0x1E0`).
pub const CFG1_REC: usize = 0x1E0;
/// cfg2 record size (0x41EA50's `* 0x68`).
pub const CFG2_REC: usize = 0x68;
/// Offset of the fourteen option words in a cfg2 record (`+0x30`).
pub const OPT_OFF: usize = 0x30;
/// Number of Display options in a record.
pub const OPT_COUNT: usize = 14;
/// TRANSLUCENCY's slot among the fourteen (`cfg2 +0x58`).
pub const TRANSLUCENCY: usize = 10;
/// The byte of a cfg1 record 0x420F30 writes: the top byte of the dword at `+0x14`.
pub const CFG1_ALPHA: usize = 0x17;

/// A layout's name in cfg1 (0x5CCB38: "Config 1" … "Config 4").
pub fn layout_name(layout: usize) -> String {
    format!("Config {}", layout.min(3) + 1)
}

/// The cfg2 options the Display screen's thirteen accessors carry, in the order of the widgets
/// `[this+0x144 … 0x174]` (0x4CB3E0) and of the setters 0x4CB860 calls (0x420E10 … 0x421010): every
/// option but DRIVERSNAPSHOT (index 9), which no accessor holds and 0x4CB860 never writes.
pub const ACCESSOR_OPTS: [usize; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11, 12, 13];

/// The two tables as loaded (or as the shipped files are).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudCfg {
    pub cfg1: Vec<u8>,
    pub cfg2: Vec<u8>,
    /// The Display screen's accessor values changed since they were last filled from a record
    /// (`INUSE` 0x4CB650, the layout change 0x4CB9A0): what the original holds in the accessors
    /// alone until 0x4CB860 stores them.
    pub staged: [Option<u32>; OPT_COUNT],
    /// `[this+0x218]` of the HUD widget: set by `INUSE` (0x4CB656), cleared by `NOTINUSE` (0x4CB84D).
    pub armed: bool,
}

impl HudCfg {
    pub fn new(cfg1: Vec<u8>, cfg2: Vec<u8>) -> Self {
        Self { cfg1, cfg2, staged: [None; OPT_COUNT], armed: false }
    }

    /// The widget's `INUSE` (slot 21 0x4CB650): the flag armed, every accessor refilled from the
    /// current record (0x4CB6F4…0x4CB814) — nothing staged.
    pub fn in_use(&mut self) {
        self.armed = true;
        self.staged = [None; OPT_COUNT];
    }

    /// An accessor's value: what it holds (staged), else the record's option.
    pub fn accessor_value(&self, layout: usize, w: u32, h: u32, slot: usize) -> Option<u32> {
        self.staged[slot].or_else(|| self.opts_for(layout, w, h).map(|o| o[slot]))
    }

    /// 0x4CB860 on the record of `(layout, w, h)` (made first when missing, 0x41EBF0): the thirteen
    /// accessors' values into the options, the staged ones among them; the accessors are now the
    /// record's.
    pub fn commit(&mut self, layout: usize, w: u32, h: u32) {
        let i = self.record(layout, w, h);
        for slot in ACCESSOR_OPTS {
            if let Some(v) = self.staged[slot] {
                self.set_opt(i, slot, v);
            }
        }
        // 0x420F30 (Translucency's setter, the tenth of 0x4CB860's thirteen, 0x4CB923): besides
        // `cfg2[+0x58] = value` it rewrites the top byte of the cfg1 record's dword `[+0x14]`
        // (0x420F5F…0x420F99 / 0x420FAD…0x420FC6): `[0x5CCB34]` = 0x80 for 0, cleared otherwise.
        // The byte is what 0x41E5C0 reads for the trays' box (hud.md §6), and it goes into
        // `<Name>1.hud` with the record.
        let alpha = if self.opts(i)[TRANSLUCENCY] == 0 { 0x80 } else { 0 };
        self.cfg1[i * CFG1_REC + CFG1_ALPHA] = alpha;
        self.staged = [None; OPT_COUNT];
    }

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
    /// 0x41EBF0 then runs 0x41EFF0 on the pair (read to the instruction, N01, 2026-10-02: fourteen
    /// unrolled blocks, each `x640 = ftol(f · 640)`, `y480 = ftol(f · 480)`, 0x41E480 on the
    /// `[0x60C900] × [0x60C81C]` HUD_init stored — the record's own `(w, h)` — and `float(n / W)` back;
    /// block 11 re-does HGear). The anchored fields of the copy stay a copy here **on purpose**: the
    /// port reads no position from the file; `Hud::anchor_passes` (hud/geometry.rs) is that same
    /// 0x41EFF0 block plus 0x41FB40's `ftol(frac · W)`, evaluated at draw time for the real
    /// `W × H`, which is the value the original would have stored for this very `(w, h)`.
    /// **N01a (read to the instruction, HUD-FU 2026-10-02):** 0x41EFF0 runs only here, when a record
    /// is made; a record that already exists is read by 0x41F7A0 and **used as stored**. The port
    /// does the same: [`HudCfg::stored_positions`] hands the stored fractions of an existing record
    /// to `Hud::set_stored` (hud/geometry.rs), and only a size with no record is derived at draw time.
    /// The shipped `nfs5-2.hud`'s "Config 2" @ 1600×1200 is the one record that is not the
    /// derivation (HTach `y` +10, HLeader `x` +5, `hud.md` §2.2), so it is drawn where the file has it.
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

    /// The record's stored element positions (cfg1 `+0x188…+0x1D8` and cfg2 `+0xC/+0x10`,
    /// `+0x20/+0x24`, the f32 fractions 0x41FB40 multiplies by `W` / `H`), keyed by the element's
    /// 640×480 design position in `SCREEN_LAYOUTS` order: `[gear, speed, time, tach_digital, tach,
    /// speedometer, position, track_map, leader, rearview, multi_time, text_damage, graphic_damage]`.
    pub fn stored_positions(&self, i: usize) -> [(f32, f32); 13] {
        let f1 = |off: usize| {
            let o = i * CFG1_REC + off;
            let g = |k: usize| f32::from_le_bytes([self.cfg1[o + k], self.cfg1[o + k + 1], self.cfg1[o + k + 2], self.cfg1[o + k + 3]]);
            (g(0), g(4))
        };
        let f2 = |off: usize| (f32::from_bits(self.word(i, off)), f32::from_bits(self.word(i, off + 4)));
        [
            f1(0x188), f1(0x190), f1(0x198), f2(0x20), f1(0x1B8), f1(0x1C0), f1(0x1A8), f1(0x1C8), f1(0x1D0), f1(0x1D8),
            f2(0x0C), f1(0x1A0), f1(0x1B0),
        ]
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
    Some((HudCfg::new(cfg1, cfg2), from_profile))
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
        HudCfg::new(cfg1, cfg2)
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

    /// 0x4CB650 / 0x4CB860 / 0x4CB820 / 0x4CB9A0: the accessors hold the values until they are
    /// stored, and a store reaches the record that was current when it ran.
    #[test]
    fn accessor_values_are_stored_at_the_commit_only() {
        let mut c = shipped();
        c.in_use();
        c.staged[5] = Some(4);
        assert_eq!(c.accessor_value(0, 640, 480, 5), Some(4), "the accessor");
        assert_eq!(c.opts_for(0, 640, 480).unwrap()[5], 0, "the record is untouched until 0x4CB860");
        c.commit(0, 640, 480);
        assert_eq!(c.opts_for(0, 640, 480).unwrap()[5], 4);
        assert_eq!(c.staged, [None; OPT_COUNT]);
        // the layout change: the old layout's record takes the values, the new one's own are read
        c.staged[7] = Some(0);
        c.commit(1, 640, 480);
        assert_eq!(c.opts_for(1, 640, 480).unwrap()[7], 0);
        assert_eq!(c.accessor_value(2, 640, 480, 7), Some(1), "Config 3's own");
        // DRIVERSNAPSHOT has no accessor: a stray stage never reaches the record
        c.staged[9] = Some(1);
        c.commit(0, 640, 480);
        assert_eq!(c.opts_for(0, 640, 480).unwrap()[9], 0);
    }

    /// 0x420F30: a commit rewrites the alpha byte of the record's cfg1 `[+0x14]` from TRANSLUCENCY.
    #[test]
    fn translucency_sets_the_cfg1_alpha_byte() {
        let mut c = shipped();
        let i = c.record(0, 640, 480);
        c.cfg1[i * CFG1_REC + 0x14..i * CFG1_REC + 0x18].copy_from_slice(&0x7F11_2233u32.to_le_bytes());
        c.in_use();
        c.commit(0, 640, 480);
        // Config 1 has TRANSLUCENCY 0: the byte becomes [0x5CCB34] = 0x80, the rest stays
        assert_eq!(&c.cfg1[i * CFG1_REC + 0x14..i * CFG1_REC + 0x18], &0x8011_2233u32.to_le_bytes());
        c.staged[TRANSLUCENCY] = Some(1);
        c.commit(0, 640, 480);
        assert_eq!(&c.cfg1[i * CFG1_REC + 0x14..i * CFG1_REC + 0x18], &0x0011_2233u32.to_le_bytes());
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

    /// N01a: an existing record's positions are read from the fields 0x41FB40 reads (cfg1 `+0x188…`,
    /// cfg2 `+0x0C` / `+0x20`) and a missing one has none.
    #[test]
    fn stored_positions_are_the_records_fields() {
        let mut c = shipped();
        let i = c.record(1, 1600, 1200);
        let put = |c: &mut HudCfg, off: usize, x: f32, y: f32| {
            let o = i * CFG1_REC + off;
            c.cfg1[o..o + 4].copy_from_slice(&x.to_le_bytes());
            c.cfg1[o + 4..o + 8].copy_from_slice(&y.to_le_bytes());
        };
        put(&mut c, 0x1B8, 0.92, 0.8608333);
        put(&mut c, 0x1D0, 0.08125, 0.0);
        let o = i * CFG2_REC + 0x20;
        c.cfg2[o..o + 4].copy_from_slice(&0.8375f32.to_le_bytes());
        let p = c.stored_positions(i);
        assert_eq!(p[4], (0.92, 0.8608333), "HTach");
        assert_eq!(p[8], (0.08125, 0.0), "HLeader");
        assert_eq!(p[3].0, 0.8375, "HTachDigital x");
        assert_eq!(c.find(1, 1600, 1200), Some(i));
        assert_eq!(c.find(1, 1280, 720), None);
    }
}
