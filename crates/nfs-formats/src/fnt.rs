//! `FNTF` bitmap fonts (`*.ffn` in `GameData/DashHud`, `FEData/Art`).
//!
//! Status: CONFIRMED by rendering digits from `UniversC12b.ffn` / `debug.ffn` (see `RE/FORMATS.md`).
//!
//! ```text
//! +0   "FNTF"
//! +4   u32 file size
//! +8   u16 101 (unknown, constant), u16 glyph count, u16, u16, u16, u16 flags, u32 0x20 = glyph table offset,
//! +24  u32 0, u32 atlas offset
//! +32  glyph records, 11 bytes: { u16 char, u8 width, u8 height, u16 x, u16 y, i8 advance, i8 offset_x, i8 offset_y }
//!
//! The last three fields are **signed** and are pure geometry: the quad builder 0x53DC53…0x53DCBC
//! reads `[rec+9]` and multiplies it by the font's X scale, `[rec+0xA]` by the Y scale, and passes
//! both as the glyph's offset from the pen; `[rec+8]` is then added to the pen's X (0x53DCD4).
//! CONFIRMED — and it explains the shipped data exactly: in `Univers8b.ffn` `'A'` is 9×9 at
//! offset (1, 1), `'m'` 10×7 at (1, 3) — the same baseline — `'o'` 8 tall at (1, 3) overshooting it
//! by one, `'g'` 10 tall at (1, 3) descending three below, and `','` 4 tall at (1, 7).
//! atlas: 16-byte block header { u8 0x7A, u24 0, u16 width, u16 height, 8×0 } then 4 bpp intensity/alpha,
//!        high nibble first, row-major (width × height / 2 bytes)
//! ```

use crate::{u16_le, u32_le, FormatError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub ch: u16,
    pub width: u8,
    pub height: u8,
    pub x: u16,
    pub y: u16,
    /// Pen advance in texels (signed).
    pub advance: i8,
    /// Left bearing: the quad's X offset from the pen.
    pub offset_x: i8,
    /// The quad's Y offset from the line's top — this is what puts descenders below the baseline.
    pub offset_y: i8,
}

#[derive(Debug, Clone)]
pub struct Font {
    pub glyphs: Vec<Glyph>,
    pub atlas_width: u16,
    pub atlas_height: u16,
    /// 4 bpp packed, high nibble first.
    pub atlas: Vec<u8>,
    /// max glyph height
    pub line_height: u8,
}

impl Font {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(b"FNTF") {
            return Err(FormatError::UnknownMagic { what: "FNTF", offset: 0, expected: b"FNTF".to_vec(), found: data.iter().take(4).copied().collect() });
        }
        let count = u16_le(data, 10, "FNTF glyph count")? as usize;
        let table = u32_le(data, 20, "FNTF table offset")? as usize;
        let atlas_off = u32_le(data, 28, "FNTF atlas offset")? as usize;
        let mut glyphs = Vec::with_capacity(count);
        for i in 0..count {
            let o = table + i * 11;
            let r = data.get(o..o + 11).ok_or(FormatError::Truncated { what: "FNTF glyph", offset: o, needed: 11 })?;
            glyphs.push(Glyph { ch: u16::from_le_bytes([r[0], r[1]]), width: r[2], height: r[3], x: u16::from_le_bytes([r[4], r[5]]), y: u16::from_le_bytes([r[6], r[7]]), advance: r[8] as i8, offset_x: r[9] as i8, offset_y: r[10] as i8 });
        }
        let code = *data.get(atlas_off).ok_or(FormatError::Truncated { what: "FNTF atlas", offset: atlas_off, needed: 16 })?;
        if code != 0x7A {
            return Err(FormatError::UnknownRecordType { what: "FNTF atlas block", offset: atlas_off, code: code as u32 });
        }
        let atlas_width = u16_le(data, atlas_off + 4, "FNTF atlas width")?;
        let atlas_height = u16_le(data, atlas_off + 6, "FNTF atlas height")?;
        let n = atlas_width as usize * atlas_height as usize / 2;
        let atlas = data.get(atlas_off + 16..atlas_off + 16 + n).ok_or(FormatError::Truncated { what: "FNTF atlas pixels", offset: atlas_off + 16, needed: n })?.to_vec();
        let line_height = glyphs.iter().map(|g| g.height).max().unwrap_or(0);
        Ok(Self { glyphs, atlas_width, atlas_height, atlas, line_height })
    }

    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        let c = ch as u32;
        self.glyphs.iter().find(|g| g.ch as u32 == c)
    }

    /// Atlas intensity (0..15) at (x, y).
    pub fn texel(&self, x: u32, y: u32) -> u8 {
        let idx = y as usize * self.atlas_width as usize + x as usize;
        let b = self.atlas[idx / 2];
        if idx.is_multiple_of(2) { b >> 4 } else { b & 15 }
    }

    /// Expand the atlas to RGBA8 (white with alpha = intensity × 17).
    pub fn atlas_rgba(&self) -> Vec<u8> {
        let (w, h) = (self.atlas_width as u32, self.atlas_height as u32);
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let a = self.texel(x, y) * 17;
                out.extend_from_slice(&[255, 255, 255, a]);
            }
        }
        out
    }
}
