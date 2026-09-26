//! `.spl` — the per-car engine-audio parameter block (`RE/notes/engine-audio.md` §3.1, CONFIRMED).
//!
//! `GameData/Sounds/<sim>.spl` is 13 little-endian u32 fields, copied verbatim into the engine-audio
//! object by `0x59DB50(path, obj, 0x34)` at 0x4ACDD1. The defaults hard-coded at 0x4ACCE8..0x4ACD2F
//! are used when the file is missing. `game.spl` is a different, longer block — fifteen global mixer
//! trims, one per sound category; see [`GameSpl`].

use crate::{u32_le, FormatError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spl(pub [u32; 13]);

impl Default for Spl {
    /// 0x4ACCE8..0x4ACD2F.
    fn default() -> Self {
        Self([50, 0, 70, 20, 64, 128, 128, 170, 80, 104, 25, 220, 30])
    }
}

impl Spl {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut v = [0u32; 13];
        for (i, f) in v.iter_mut().enumerate() {
            *f = u32_le(data, i * 4, ".spl")?;
        }
        Ok(Self(v))
    }

    /// Record volume scale for a view mode: `spl[2]` cockpit, `spl[0]` attached exterior, `spl[7]` detached.
    pub fn record_volume(&self, mode: u8) -> u32 {
        match mode {
            0 => self.0[2],
            1 => self.0[0],
            _ => self.0[7],
        }
    }

    /// Front/rear blend 0..0x80 (§3.3). `cos` is the listener→car direction against the car's forward,
    /// −1 in front of the car, +1 behind; it is only used for the detached modes.
    pub fn blend(&self, mode: u8, cos: f32) -> i32 {
        match mode {
            0 => self.0[3] as i32,
            1 => self.0[1] as i32,
            _ => {
                let b = self.0[4] as i32;
                let k = if cos < 0.0 { self.0[5] as i32 * b } else { (0x80 - b) * self.0[6] as i32 };
                // `(k · cos) >> 7 >> 16` with cos in 16.16
                b + (((k as f32 * cos.clamp(-1.0, 1.0) * 65536.0) as i64 >> 7 >> 16) as i32)
            }
        }
    }

    /// Aux-layer volume (road / wind / turbo) for a view mode; mode 0 is used shifted left by 4.
    pub fn aux_volume(&self, mode: u8) -> u32 {
        match mode {
            0 => self.0[11] << 4,
            1 => self.0[10],
            _ => self.0[12],
        }
    }

    /// `spl[9]` — rpm → bin scale. `bin = min(spl[9] · rpm/redline · 4, 511)`.
    pub fn bin(&self, rpm_frac: f32) -> usize {
        ((self.0[9] as f32 * rpm_frac.max(0.0) * 4.0) as usize).min(511)
    }

    /// `spl[8]` — playback ratio `spl[8] · (rpm/redline + 0.2) / 64`, times the doppler factor.
    pub fn pitch(&self, rpm_frac: f32, doppler: f32) -> f32 {
        self.0[8] as f32 * (rpm_frac.max(0.0) + 0.2) * doppler / 64.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_derived_values() {
        let s = Spl::default();
        assert_eq!(s.record_volume(0), 70);
        assert_eq!(s.record_volume(1), 50);
        assert_eq!(s.record_volume(3), 170);
        // 416 bins per redline, saturating at 511
        assert_eq!(s.bin(0.0), 0);
        assert_eq!(s.bin(0.5), 208);
        assert_eq!(s.bin(1.0), 416);
        assert_eq!(s.bin(2.0), 511);
        // 0.25× at rest, 1.0× at 0.6 redline, 1.5× at redline
        assert!((s.pitch(0.0, 1.0) - 0.25).abs() < 1e-6);
        assert!((s.pitch(0.6, 1.0) - 1.0).abs() < 1e-6);
        assert!((s.pitch(1.0, 1.0) - 1.5).abs() < 1e-6);
        // the shipped slopes put the blend at 0 in front of the car and 0x80 behind it
        assert_eq!(s.blend(2, -1.0), 0);
        assert_eq!(s.blend(2, 1.0), 0x80);
        assert_eq!(s.blend(2, 0.0), 64);
        assert_eq!(s.blend(0, 1.0), 20);
        assert_eq!(s.blend(1, -1.0), 0);
    }

    #[test]
    fn parses_a_52_byte_file() {
        let mut d = Vec::new();
        for i in 0..13u32 {
            d.extend_from_slice(&(i * 3).to_le_bytes());
        }
        let s = Spl::parse(&d).unwrap();
        assert_eq!(s.0[9], 27);
        assert!(Spl::parse(&d[..40]).is_err());
    }
}

/// Which block the race's audio init loads into `0x5D14B0`: **0x4A8660** builds
/// `[0x65B318] + name + ".spl"` with `name` `"replay"` (0x5CCE90) while `[0x5E99F4] >= 2` — a
/// playback — and `"game"` (0x5D1A0C) otherwise (0x4A86A1…0x4A86B5), then `0x59DB50(path, 0x5D14B0,
/// 0x3C)` (0x4A8713). The block stays until the next race's init, front end included.
static REPLAY_MIX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn mix_file(replay: bool) -> &'static str {
    if replay { "replay.spl" } else { "game.spl" }
}

/// The race audio init's choice (see [`REPLAY_MIX`]); `true` when the block changes.
pub fn select_mix(replay: bool) -> bool {
    REPLAY_MIX.swap(replay, std::sync::atomic::Ordering::Relaxed) != replay
}

/// `GameData/Sounds/game.spl` — the fifteen global mixer trims (`RE/notes/engine-audio.md` §6).
///
/// The block is copied over the compiled-in defaults at `0x5D14B0` when the file loads; every entry
/// but [`GameSpl::UNUSED_13`] has a reader, and each one scales one category against its master
/// level (`Audio.*Level`, 0..127).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameSpl(pub [u32; 15]);

impl Default for GameSpl {
    /// The values compiled into the executable at `0x5D14B0`, used when the file is missing.
    fn default() -> Self {
        Self([80, 127, 127, 127, 127, 95, 20, 127, 127, 127, 70, 70, 70, 0, 0])
    }
}

impl GameSpl {
    /// Index 13 (`0x5D14E4`) is never read by the game.
    pub const UNUSED_13: usize = 13;

    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let mut v = [0u32; 15];
        for (i, f) in v.iter_mut().enumerate() {
            *f = u32_le(data, i * 4, "game.spl")?;
        }
        Ok(Self(v))
    }

    /// Read the block the last race audio init chose ([`select_mix`]) — `<sounds>/game.spl`, or
    /// `replay.spl` for a playback — falling back to the executable's defaults.
    pub fn load(sounds_dir: &std::path::Path) -> Self {
        Self::load_mix(sounds_dir, REPLAY_MIX.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// `<sounds>/game.spl`, or `<sounds>/replay.spl` when `replay` (0x4A86A1…0x4A86B5).
    pub fn load_mix(sounds_dir: &std::path::Path, replay: bool) -> Self {
        std::fs::read(sounds_dir.join(mix_file(replay))).ok().and_then(|d| Self::parse(&d).ok()).unwrap_or_default()
    }

    /// `[0]` — streamed music: the volume is `MusicLevel · music() >> 7` (0x4A82F0). A `win`/`lose`/
    /// `vic` jingle bypasses the trim and plays at the raw level.
    pub fn music(&self) -> u32 {
        self.0[0]
    }
    /// `[4]` for a queued phrase whose record field `+0x18 < 4`, `[1]` otherwise (0x4B052A).
    pub fn speech(&self, short: bool) -> u32 {
        self.0[if short { 4 } else { 1 }]
    }
    /// `[2]` — front-end one-shots (0x4AADB0), where the SFX master is applied **twice**.
    pub fn fe_sfx(&self) -> u32 {
        self.0[2]
    }
    /// `[3]` — the track ambience bank.
    pub fn ambience(&self) -> u32 {
        self.0[3]
    }
    /// `[5]` — the looped voices started by 0x4AAFB0 in bank `[0x6564D0]`.
    pub fn loops_6564d0(&self) -> u32 {
        self.0[5]
    }
    /// `[6]` — the player's engine.
    pub fn engine(&self) -> u32 {
        self.0[6]
    }
    /// `[7]` — road roll (slots 0x11..0x1C).
    pub fn road_roll(&self) -> u32 {
        self.0[7]
    }
    /// `[8]` — horns (slots 3..5 of the player's engine bank).
    pub fn horn(&self) -> u32 {
        self.0[8]
    }
    /// `[9]` — everything else that goes through `Voice_play`.
    pub fn sfx(&self) -> u32 {
        self.0[9]
    }
    /// `[10]` — the AI / traffic engine bank.
    pub fn ai_engine(&self) -> u32 {
        self.0[10]
    }
    /// `[11]` — slot 0x10, the AI road-roll loop.
    pub fn ai_road(&self) -> u32 {
        self.0[11]
    }
    /// `[12]` — slots 0x0A / 0x0B.
    pub fn slots_0a0b(&self) -> u32 {
        self.0[12]
    }
    /// `[14]` — the turbo layer (slot 0x0F).
    pub fn turbo(&self) -> u32 {
        self.0[14]
    }
}

#[cfg(test)]
mod game_spl_tests {
    use super::GameSpl;

    #[test]
    fn shipped_block() {
        let s = GameSpl([52, 127, 127, 100, 127, 76, 16, 110, 96, 107, 60, 68, 102, 50, 68]);
        assert_eq!(s.music(), 52);
        assert_eq!(s.speech(true), 127);
        assert_eq!(s.engine(), 16);
        assert_eq!(s.ambience(), 100);
        // the executable's own defaults are louder for music and quieter for the engine
        assert_eq!(GameSpl::default().music(), 80);
        assert_eq!(GameSpl::default().engine(), 20);
    }
}

#[cfg(test)]
mod mix_tests {
    #[test]
    fn a_playback_loads_replay_spl() {
        assert_eq!(super::mix_file(false), "game.spl");
        assert_eq!(super::mix_file(true), "replay.spl");
        // the shipped pair differs in slots 0, 6, 7, 9, 10, 12 and 14 (`replay.md` §9)
        let dir = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Games/NFS-Porsche-Unleashed/game_info/data/GameData/Sounds");
        if dir.join("replay.spl").is_file() {
            let (g, r) = (super::GameSpl::load_mix(&dir, false), super::GameSpl::load_mix(&dir, true));
            assert_eq!((g.music(), r.music()), (52, 68));
            assert_eq!((g.engine(), r.engine()), (16, 10));
        }
    }
}
