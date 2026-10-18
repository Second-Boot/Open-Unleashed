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

use crate::bytes::read_u16;
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
    /// The header dword at `+0xC`. Bit 2 selects the three-colour palette (0x594980 → 0x594B50),
    /// bit 4 the four-colour one (0x594C30); `(flags >> 16) & 3` the glyph lookup (0x53C736).
    pub flags: u32,
    /// The signed bytes at `+0x10` / `+0x11`: the pen's start, `font+4` / `font+8` (0x53C6FC), added
    /// to the draw position before the first glyph (0x53DAEB…0x53DB06).
    pub origin: [i8; 2],
    /// The line break's step, `font+0x14` = the signed bytes `+0x12` plus `+0x13` (0x53C70A…0x53C71A):
    /// a `'\n'` puts the pen back at the line's start and moves it down by this (0x53DB60…0x53DB77).
    pub line_step: i32,
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
            glyphs.push(Glyph { ch: read_u16(r, 0), width: r[2], height: r[3], x: read_u16(r, 4), y: read_u16(r, 6), advance: r[8] as i8, offset_x: r[9] as i8, offset_y: r[10] as i8 });
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
        let flags = u32_le(data, 12, "FNTF flags")?;
        let origin = [*data.get(16).unwrap_or(&0) as i8, *data.get(17).unwrap_or(&0) as i8];
        let line_step = *data.get(0x12).unwrap_or(&0) as i8 as i32 + *data.get(0x13).unwrap_or(&0) as i8 as i32;
        Ok(Self { glyphs, atlas_width, atlas_height, atlas, line_height, flags, origin, line_step })
    }

    /// The record whose code is exactly `ch`'s code point — no encoding, no case swap. What the
    /// game draws goes through [`Font::lookup`].
    pub fn glyph(&self, ch: char) -> Option<&Glyph> {
        let c = ch as u32;
        self.glyphs.iter().find(|g| g.ch as u32 == c)
    }

    /// The record keyed `code` — 0x53D920: record `code − 0x20` when its key matches, else a
    /// binary search of the sorted table (0x53D970). Every shipped table is sorted with unique
    /// keys, so both are this exact match.
    pub fn glyph_code(&self, code: u16) -> Option<&Glyph> {
        self.glyphs.iter().find(|g| g.ch == code)
    }

    /// The glyph the text draw 0x53DAB0 takes for `ch`, CONFIRMED:
    ///
    /// * the string is read a **byte** at a time, zero-extended (0x573AB0) — the reader
    ///   0x53C736…0x53C77F installs for a font whose `(flags >> 16) & 3` is 0 or 3, with at most
    ///   255 records and a first key ≤ 0xFF, which is every shipped `.ffn` (the others, UTF-16
    ///   0x573A90 and a double-byte reader 0x573AE0, have no font to serve). So `ch` is looked up
    ///   by its Windows-1252 byte ([`crate::locale::to_cp1252`]): `'ô'` is 0xF4, `'’'` 0x92;
    /// * a key the font lacks is tried once more in the other case, 0x53E270: `A…Z`, `À…Ö`,
    ///   `Ø…Þ` +0x20, `a…z`, `à…ö`, `ø…þ` −0x20; any other byte (`ß`, `ÿ`, `×`, `÷`, the
    ///   punctuation) swaps to 0, which the caller takes as "none" (0x53DBE7);
    /// * still nothing → `None`: the character is **skipped** — no quad and the pen does not move
    ///   (0x53DBE9 / 0x53DBFD jump back to the reader).
    ///
    /// `None` too for a char no Windows-1252 byte stands for (the original's strings are bytes, so
    /// such a char cannot arise there (settled, class D: the caller's skip, 0x53DBFD, is the `None` arm).
    pub fn lookup(&self, ch: char) -> Option<&Glyph> {
        let b = crate::locale::to_cp1252(ch)?;
        self.glyph_code(b as u16).or_else(|| match case_swap(b) {
            0 => None,
            s => self.glyph_code(s as u16),
        })
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

/// 0x53E270: the other case of a Windows-1252 letter, 0 for anything else.
pub fn case_swap(b: u8) -> u8 {
    match b {
        0x41..=0x5A | 0xC0..=0xD6 | 0xD8..=0xDE => b + 0x20,
        0x61..=0x7A | 0xE0..=0xF6 | 0xF8..=0xFE => b - 0x20,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_swap_is_0x53e270() {
        assert_eq!(case_swap(b'A'), b'a');
        assert_eq!(case_swap(b'z'), b'Z');
        assert_eq!(case_swap(0xC0), 0xE0); // À → à
        assert_eq!(case_swap(0xF4), 0xD4); // ô → Ô
        assert_eq!(case_swap(0xFE), 0xDE); // þ → Þ
        // outside the six ranges: no other case
        for b in [0xD7u8, 0xF7, 0xDF, 0xFF, b'0', b' ', 0x92, 0x8A, 0x9A] {
            assert_eq!(case_swap(b), 0, "{b:#04X}");
        }
    }

    fn font(keys: &[u16]) -> Font {
        let glyphs = keys.iter().map(|&ch| Glyph { ch, width: 1, height: 1, x: 0, y: 0, advance: 1, offset_x: 0, offset_y: 0 }).collect();
        Font { glyphs, atlas_width: 2, atlas_height: 2, atlas: vec![0; 2], line_height: 1, flags: 0, origin: [0, 0], line_step: 0 }
    }

    #[test]
    fn lookup_takes_the_byte_then_the_other_case() {
        let f = font(&[0x41, 0x92, 0xD4]);
        assert_eq!(f.lookup('A').map(|g| g.ch), Some(0x41));
        assert_eq!(f.lookup('a').map(|g| g.ch), Some(0x41)); // swapped
        assert_eq!(f.lookup('\u{2019}').map(|g| g.ch), Some(0x92)); // cp1252 0x92, not U+2019
        assert_eq!(f.lookup('ô').map(|g| g.ch), Some(0xD4)); // 0xF4 missing → Ô
        assert!(f.lookup('b').is_none());
        assert!(f.lookup('\u{2018}').is_none()); // 0x91: no other case
        assert!(f.lookup('\u{4E00}').is_none());
    }

    /// Characterisation guard for the table / header reads: a synthetic two-glyph font file, the
    /// parse output hashed, and the failure texts of a short table and a short atlas.
    #[test]
    fn parse_output_is_pinned() {
        let mut d = vec![0u8; 0x36 + 16 + 8];
        d[0..4].copy_from_slice(b"FNTF");
        d[10..12].copy_from_slice(&2u16.to_le_bytes());
        d[12..16].copy_from_slice(&0x0005_0004u32.to_le_bytes());
        d[16] = 0xFE;
        d[17] = 3;
        d[0x12] = 0xFF;
        d[0x13] = 2;
        d[20..24].copy_from_slice(&0x20u32.to_le_bytes());
        d[28..32].copy_from_slice(&0x36u32.to_le_bytes());
        let g = |ch: u16, w: u8, h: u8, x: u16, y: u16, a: i8, ox: i8, oy: i8| {
            let mut r = Vec::new();
            r.extend(ch.to_le_bytes());
            r.extend([w, h]);
            r.extend(x.to_le_bytes());
            r.extend(y.to_le_bytes());
            r.extend([a as u8, ox as u8, oy as u8]);
            r
        };
        d[0x20..0x2B].copy_from_slice(&g(0x41, 7, 9, 0x102, 0x304, -1, 2, -3));
        d[0x2B..0x36].copy_from_slice(&g(0x92, 200, 130, 0xFFFE, 1, 127, -128, 5));
        d[0x36] = 0x7A;
        d[0x3A..0x3C].copy_from_slice(&4u16.to_le_bytes());
        d[0x3C..0x3E].copy_from_slice(&4u16.to_le_bytes());
        for b in &mut d[0x36 + 16..] {
            *b = 0xA5;
        }
        let f = Font::parse(&d).unwrap();
        let h = format!("{f:?}").bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
        assert_eq!(h, PIN);
        assert!(matches!(Font::parse(&d[..0x30]), Err(FormatError::Truncated { what: "FNTF glyph", offset: 0x2B, needed: 11 })));
        assert!(matches!(Font::parse(&d[..0x36 + 16 + 7]), Err(FormatError::Truncated { what: "FNTF atlas pixels", offset: 0x46, needed: 8 })));
    }

    const PIN: u64 = 382026314622361756;
}
