//! `CRDl` engine-sound tables (`.elt/.ect/.clt/.cct` inside `Sounds/sd*.viv`).
//!
//! Layout CONFIRMED (loader 0x4AD05F..0x4AD3E9, `RE/notes/audio.md` §4); semantics of the two
//! 512-entry tables are a HYPOTHESIS (A = volume, B = pitch versus a 512-bin engine variable).
//!
//! ```text
//! +0    "CRDl"  +4 u8 version = 2  +5..+7 {0, 0, 100}
//! +8    u16 20, u16 15, u32 500, u32 200, u16 100, u16 40, u32 300, u32 100   (constant tuning params, UNKNOWN)
//! +0x20 u8 id[8]   bank slot ids, 0xFF = unused (stop at the first 0xFF)
//! +0x28 256 zero bytes
//! +0x128 u32 off_a[8], +0x148 u32 off_b[8]   self-relative offsets of table A / B per id
//! records: 1024 B per id = table_a[512] u8 + table_b[512] u8
//! ```

use crate::{u32_le, FormatError};

#[derive(Debug, Clone)]
pub struct EngineEntry {
    /// bank slot (patch id)
    pub slot: u8,
    /// 512-bin crossfade/volume curve (0..0x7F)
    pub table_a: [u8; 512],
    /// 512-bin ramp (0..0x7F)
    pub table_b: [u8; 512],
}

impl EngineEntry {
    /// First and last bin with a non-zero volume (the loader scans the same way).
    pub fn active_range(&self) -> Option<(usize, usize)> {
        let first = self.table_a.iter().position(|&v| v != 0)?;
        let last = self.table_a.iter().rposition(|&v| v != 0)?;
        Some((first, last))
    }
    pub fn centre_bin(&self) -> f32 {
        self.active_range().map(|(a, b)| (a + b) as f32 * 0.5).unwrap_or(256.0)
    }
}

#[derive(Debug, Clone)]
pub struct Crdl {
    pub version: u8,
    pub entries: Vec<EngineEntry>,
}

impl Crdl {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(b"CRDl") {
            return Err(FormatError::UnknownMagic { what: "CRDl", offset: 0, expected: b"CRDl".to_vec(), found: data.iter().take(4).copied().collect() });
        }
        let version = data[4];
        if version != 2 {
            return Err(FormatError::UnsupportedVersion { what: "CRDl", found: version as u32, supported: "2" });
        }
        let ids = data.get(0x20..0x28).ok_or(FormatError::Truncated { what: "CRDl ids", offset: 0x20, needed: 8 })?;
        let mut entries = Vec::new();
        for (i, &slot) in ids.iter().enumerate() {
            if slot == 0xFF {
                break;
            }
            let oa = u32_le(data, 0x128 + 4 * i, "CRDl offA")? as usize + 0x128 + 4 * i;
            let ob = u32_le(data, 0x148 + 4 * i, "CRDl offB")? as usize + 0x148 + 4 * i;
            let ta = data.get(oa..oa + 512).ok_or(FormatError::Truncated { what: "CRDl table A", offset: oa, needed: 512 })?;
            let tb = data.get(ob..ob + 512).ok_or(FormatError::Truncated { what: "CRDl table B", offset: ob, needed: 512 })?;
            let mut e = EngineEntry { slot, table_a: [0; 512], table_b: [0; 512] };
            e.table_a.copy_from_slice(ta);
            e.table_b.copy_from_slice(tb);
            entries.push(e);
        }
        Ok(Self { version, entries })
    }
}
