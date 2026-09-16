//! BIGF archives (`.viv` files).
//!
//! Status: **CONFIRMED** on `GameData/Track/Data/camera.viv`, `Sounds/sd6a.viv`,
//! `Music/zzzymus.viv`, `Speech/zzzxeng.viv` (see `RE/FORMATS.md`).
//!
//! ```text
//! +0   "BIGF"
//! +4   u32 BE  total archive size
//! +8   u32 BE  entry count
//! +12  u32 BE  header size (offset of first file's data)
//! +16  entries: { u32 BE offset, u32 BE size, NUL-terminated name }
//! ```
//!
//! Note: some `.viv` files (e.g. `Track/Data/tr00a.viv`) use a different, smaller container
//! whose header starts with `C0 FB 00 80`; that format is not handled here (HYPOTHESIS: 24-bit
//! offset/size entries — see `RE/HYPOTHESES.md`).

use crate::{u32_be, FormatError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigfEntry {
    pub name: String,
    pub offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone)]
pub struct Bigf {
    pub declared_size: u32,
    pub header_size: u32,
    pub entries: Vec<BigfEntry>,
}

impl Bigf {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(b"BIGF") {
            return Err(FormatError::UnknownMagic {
                what: "BIGF",
                offset: 0,
                expected: b"BIGF".to_vec(),
                found: data.iter().take(4).copied().collect(),
            });
        }
        let declared_size = u32_be(data, 4, "BIGF size")?;
        let count = u32_be(data, 8, "BIGF count")?;
        let header_size = u32_be(data, 12, "BIGF header size")?;
        let mut entries = Vec::with_capacity(count as usize);
        let mut pos = 16usize;
        for _ in 0..count {
            let offset = u32_be(data, pos, "BIGF entry offset")?;
            let size = u32_be(data, pos + 4, "BIGF entry size")?;
            pos += 8;
            let start = pos;
            while *data.get(pos).ok_or(FormatError::Truncated { what: "BIGF entry name", offset: pos, needed: 1 })? != 0 {
                pos += 1;
            }
            let name = data[start..pos].iter().map(|&b| b as char).collect();
            pos += 1;
            let end = offset as usize + size as usize;
            if end > data.len() {
                return Err(FormatError::InvalidOffset { what: "BIGF entry data", offset: end, len: data.len() });
            }
            entries.push(BigfEntry { name, offset, size });
        }
        Ok(Self { declared_size, header_size, entries })
    }

    pub fn get<'a>(&self, data: &'a [u8], name: &str) -> Option<&'a [u8]> {
        self.entries
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .map(|e| &data[e.offset as usize..(e.offset + e.size) as usize])
    }

    pub fn data<'a>(&self, data: &'a [u8], entry: &BigfEntry) -> &'a [u8] {
        &data[entry.offset as usize..(entry.offset + entry.size) as usize]
    }
}
