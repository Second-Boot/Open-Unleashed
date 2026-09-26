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
    /// The record's first dword, as stored. The game keeps only its **low byte** as the wall word
    /// (see [`EdgeSegment::loaded`]): 0x10 metal, 0x20 wood, 0x30 snow, 0x40 ice, 0x50 earth,
    /// 0x60 rock; 0 (most edges) falls back to rock (`WallImpulse` 0x494000: `w ∉ [0x10, 0x60] ⇒
    /// 0x60`). The upper bytes are leftover memory (monaco1 / monaco2 / skidpad on every record),
    /// not part of the word.
    pub tag: u32,
    pub p1: [f32; 3],
    pub p2: [f32; 3],
}

impl EdgeSegment {
    /// The edge as the game builds it from the record (`.edg` reader 0x487E80 → the edge item's
    /// constructor 0x485100, vtable 0x5B484C): the wall word at `item+8` is the record's **first
    /// byte** zero-extended (`movzx cx, byte [rec]` at 0x487F0D, stored as a u16 at 0x4851F4; read
    /// back by `Tracker::edgeHit` 0x474242 into `[0x628C88]`), so the upper three bytes of the
    /// tag dword — non-zero on every record of monaco1 / monaco2 / skidpad and on one record of
    /// most other tracks — are ignored. And an edge whose x or z extent exceeds 100 (strict,
    /// 0x4851A4 / 0x4851BB) is collapsed to `p2 → p2 + (1, 0, 0)` (0x4851C8…0x4851E5); no shipped
    /// `.edg` has one (longest extent under 100 on all fifteen).
    pub fn loaded(&self) -> ([f32; 3], [f32; 3], u32) {
        let word = self.tag & 0xFF;
        let (ex, ez) = ((self.p1[0] - self.p2[0]).abs(), (self.p1[2] - self.p2[2]).abs());
        if ex > 100.0 || ez > 100.0 {
            let p = self.p2;
            return (p, [p[0] + 1.0, p[1], p[2]], word);
        }
        (self.p1, self.p2, word)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wall_word_is_the_low_byte_and_long_edges_collapse() {
        // monaco1's records carry junk in the upper bytes: 0x…10 is metal to the game (0x487F0D)
        let s = EdgeSegment { tag: 0x0012_3410, p1: [0.0, 1.0, 0.0], p2: [5.0, 2.0, 5.0] };
        assert_eq!(s.loaded(), ([0.0, 1.0, 0.0], [5.0, 2.0, 5.0], 0x10));
        // an extent above 100 in x or z: p2 → p2 + (1, 0, 0) (0x4851C8)
        let s = EdgeSegment { tag: 0x60, p1: [0.0, 0.0, 0.0], p2: [3.0, 4.0, 100.5] };
        assert_eq!(s.loaded(), ([3.0, 4.0, 100.5], [4.0, 4.0, 100.5], 0x60));
        // exactly 100 stays (strict compare, 0x4851A4)
        let s = EdgeSegment { tag: 0, p1: [0.0, 0.0, 0.0], p2: [100.0, 0.0, 0.0] };
        assert_eq!(s.loaded().1, [100.0, 0.0, 0.0]);
    }
}
