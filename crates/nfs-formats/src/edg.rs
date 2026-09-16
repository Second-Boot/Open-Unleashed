//! `Track/<track>.edg` — cached road-edge / wall collision segments (RE/notes/race-logic.md §6.3,
//! MEDIUM: the game rebuilds this file from the track mesh when it is missing).
//!
//! ```text
//! +0  u32 header (alps 29087 — not the record count; meaning UNKNOWN)
//! +4  records of 28 bytes: { u32 tag, f32 p1[3], f32 p2[3] }   (alps: 7694)
//! ```

use crate::{f32_le, u32_le, FormatError};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeSegment {
    /// Wall word: 0x10 metal, 0x20 wood, 0x30 snow, 0x40 ice, 0x50 earth, 0x60 rock; 0 (most
    /// edges) and stray pointer values (monaco tracks, skidpad) fall back to rock in the game
    /// (`WallImpulse` 0x494000: `w ∉ [0x10, 0x60] ⇒ 0x60`).
    pub tag: u32,
    pub p1: [f32; 3],
    pub p2: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct EdgeFile {
    pub header: u32,
    pub segments: Vec<EdgeSegment>,
}

impl EdgeFile {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let header = u32_le(data, 0, ".edg header")?;
        let body = &data[4..];
        if !body.len().is_multiple_of(28) {
            return Err(FormatError::SizeMismatch { what: ".edg records", declared: 28, actual: body.len() % 28 });
        }
        let mut segments = Vec::with_capacity(body.len() / 28);
        for i in 0..body.len() / 28 {
            let o = i * 28;
            let f = |k: usize| f32_le(body, o + 4 + k * 4, ".edg point");
            segments.push(EdgeSegment { tag: u32_le(body, o, ".edg tag")?, p1: [f(0)?, f(1)?, f(2)?], p2: [f(3)?, f(4)?, f(5)?] });
        }
        Ok(Self { header, segments })
    }
}
