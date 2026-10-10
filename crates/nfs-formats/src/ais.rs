//! `Simulation/AICarData/<family>.ais` — AI car data (304 bytes): 32-byte family name + 68 f32.
//!
//! CONFIRMED (loader 0x49CA30, consumers 0x40B6F9/0x40BAC1, `RE/notes/ai.md` §2.1):
//! `f[0..24)` = curvature lookahead in slices indexed by `speed·0.2·carSpecs[0x13C]` (≤ 23);
//! `f[24..64)` = target speed (m/s) indexed by total bend angle / 3° (0..39); `f[64..68)` unused.

use crate::{f32_le, fixed_str, FormatError};

pub const AIS_SIZE: usize = 304;

#[derive(Debug, Clone, PartialEq)]
pub struct AiCarData {
    pub name: String,
    pub lookahead: [f32; 24],
    pub bend_speed: [f32; 40],
    pub unused: [f32; 4],
}

impl AiCarData {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.len() < AIS_SIZE {
            return Err(FormatError::Truncated { what: ".ais", offset: data.len(), needed: AIS_SIZE - data.len() });
        }
        let f = |i: usize| f32_le(data, 32 + i * 4, ".ais float");
        let mut lookahead = [0f32; 24];
        for (i, v) in lookahead.iter_mut().enumerate() {
            *v = f(i)?;
        }
        let mut bend_speed = [0f32; 40];
        for (i, v) in bend_speed.iter_mut().enumerate() {
            *v = f(24 + i)?;
        }
        let mut unused = [0f32; 4];
        for (i, v) in unused.iter_mut().enumerate() {
            *v = f(64 + i)?;
        }
        Ok(Self { name: fixed_str(data, 0, 32, ".ais name")?, lookahead, bend_speed, unused })
    }
}

/// `Track/racer/work/<track><0|1>.lsp` — racing line: `u32 (=1)` + n × `{f32 x, f32 z}` world
/// coordinates, one point per slice (CONFIRMED, `RE/notes/ai.md` §4).
#[derive(Debug, Clone, PartialEq)]
pub struct RacingLine {
    pub header: u32,
    pub points: Vec<[f32; 2]>,
}

impl RacingLine {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let header = crate::u32_le(data, 0, ".lsp header")?;
        let body = &data[4..];
        if !body.len().is_multiple_of(8) {
            return Err(FormatError::SizeMismatch { what: ".lsp points", declared: 8, actual: body.len() % 8 });
        }
        let points = body.chunks_exact(8).map(|c| [crate::bytes::read_f32(c, 0), crate::bytes::read_f32(c, 4)]).collect();
        Ok(Self { header, points })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv(s: &str) -> u64 {
        s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
    }

    /// Characterisation guard for the byte readers: a synthetic 304-byte record and a racing line,
    /// the whole parse output pinned by a hash, plus the failure texts.
    #[test]
    fn parse_output_is_pinned() {
        let mut d = vec![0u8; AIS_SIZE];
        d[..7].copy_from_slice(b"porsche");
        for i in 0..68 {
            d[32 + i * 4..36 + i * 4].copy_from_slice(&(i as f32 * 1.25 - 3.0).to_le_bytes());
        }
        assert_eq!(fnv(&format!("{:?}", AiCarData::parse(&d).unwrap())), PIN_AIS);
        assert!(matches!(AiCarData::parse(&d[..300]), Err(FormatError::Truncated { what: ".ais", offset: 300, needed: 4 })));

        let mut l = 1u32.to_le_bytes().to_vec();
        for i in 0..5 {
            l.extend((i as f32 * 0.5).to_le_bytes());
            l.extend((-(i as f32) * 7.25).to_le_bytes());
        }
        assert_eq!(fnv(&format!("{:?}", RacingLine::parse(&l).unwrap())), PIN_LSP);
        assert!(matches!(RacingLine::parse(&l[..3]), Err(FormatError::Truncated { what: ".lsp header", offset: 0, needed: 4 })));
        assert!(matches!(RacingLine::parse(&l[..8]), Err(FormatError::SizeMismatch { what: ".lsp points", declared: 8, actual: 4 })));
    }

    const PIN_AIS: u64 = 16739403588534398335;
    const PIN_LSP: u64 = 1050907368300943273;
}
