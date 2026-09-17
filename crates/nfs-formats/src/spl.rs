//! `.spl` — the per-car engine-audio parameter block (`RE/notes/engine-audio.md` §3.1, CONFIRMED).
//!
//! `GameData/Sounds/<sim>.spl` is 13 little-endian u32 fields, copied verbatim into the engine-audio
//! object by `0x59DB50(path, obj, 0x34)` at 0x4ACDD1. The defaults hard-coded at 0x4ACCE8..0x4ACD2F
//! are used when the file is missing. `game.spl` is a different, longer block; only its field 6
//! (engine master trim) and field 3 (ambience trim) are used here.

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
