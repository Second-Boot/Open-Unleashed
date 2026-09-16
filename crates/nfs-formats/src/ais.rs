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
        let points = body.chunks_exact(8).map(|c| [f32::from_le_bytes([c[0], c[1], c[2], c[3]]), f32::from_le_bytes([c[4], c[5], c[6], c[7]])]).collect();
        Ok(Self { header, points })
    }
}
