//! `FEData/Locale/<key>.loc` — a car's own strings, which the showcase screens read.
//!
//! The catalogue record's `locale_key` (+0x4D) names the file, and `Showcase.Text(<n>)` on
//! `chassis.lay`, `general.lay` and `glancestats.lay` is entry `n` of it.
//!
//! The container is three chunk kinds, each `"TAG", u32 size, …`:
//!
//! * **`LOCH`** at 0 — the header: `+8` a version, `+0xC` the number of languages, then that many
//!   absolute offsets, one `LOCL` block each. The shipped files carry seven, which is the same
//!   seven columns `FEStrings.csv` has.
//! * **`LOCI`** — an index of `(entry << 16) | offset` pairs, the offsets running 4, 6, 8, … in
//!   step with the entry number.
//! * **`LOCL`** — one language: `+0xC` the entry count, then that many `u32` offsets **relative to
//!   the block**, each to a NUL-terminated string. The table ends exactly where the first string
//!   begins (`0x10 + 44*4 = 0xC0`, and `1948356.loc`'s first offset is `0xC0`), which is what
//!   confirms the layout.
//!
//! CONFIRMED from the shipped files.

use crate::FormatError;

/// The four-byte tags, in the order a file carries them.
pub const HEADER: &[u8; 4] = b"LOCH";
pub const INDEX: &[u8; 4] = b"LOCI";
pub const LANGUAGE: &[u8; 4] = b"LOCL";

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// One car's strings in one language.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocFile {
    pub entries: Vec<String>,
}

impl LocFile {
    /// Parse the `lang`-th language block (0 is English, the column `FEStrings.csv` also leads
    /// with). A language the file does not have falls back to the first.
    pub fn parse(d: &[u8], lang: usize) -> Result<Self, FormatError> {
        let what = "loc";
        if d.get(0..4) != Some(HEADER) {
            return Err(FormatError::UnknownMagic { what, offset: 0, expected: HEADER.to_vec(), found: d.get(0..4).unwrap_or_default().to_vec() });
        }
        let langs = u32_at(d, 0xC).unwrap_or(0) as usize;
        let pick = if lang < langs { lang } else { 0 };
        let block = u32_at(d, 0x10 + pick * 4).ok_or(FormatError::Truncated { what, offset: 0x10 + pick * 4, needed: 4 })? as usize;
        if d.get(block..block + 4) != Some(LANGUAGE) {
            return Err(FormatError::UnknownMagic { what, offset: block, expected: LANGUAGE.to_vec(), found: d.get(block..block + 4).unwrap_or_default().to_vec() });
        }
        let count = u32_at(d, block + 0xC).unwrap_or(0) as usize;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let off = u32_at(d, block + 0x10 + i * 4).unwrap_or(0) as usize + block;
            let end = d[off.min(d.len())..].iter().position(|&b| b == 0).map(|n| off + n).unwrap_or(d.len());
            entries.push(d.get(off..end).unwrap_or_default().iter().map(|&b| crate::locale::cp1252(b)).collect());
        }
        Ok(Self { entries })
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str).filter(|s| !s.is_empty())
    }
}
