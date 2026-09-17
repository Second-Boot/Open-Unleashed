//! `GameData/Track/Cops/<track>.cop` — the police spawn points (`RE/notes/traffic.md` §25).
//!
//! `{u32 count}` followed by `count` records whose size is decided by their leading type dword:
//! type 1 and 2 are 20 bytes, type 3 is 72, type 5 is `0x40 + 20·subCount`. Every record in every
//! shipped file is type 1: a slice index plus a "rolling or parked" flag. `CopManager::load` 0x40E3B0
//! sorts them ascending by slice, which is the key the spawn search walks.
//!
//! An unknown type makes the original's parse loop advance by zero bytes and hang (0x40E440's default
//! arm), so this parser treats it as an error instead.

use crate::{u32_le, FormatError};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CopRecord {
    /// Types 1 and 2 — the only ones any shipped file uses. Type 2 is parsed and then rejected by the
    /// spawn search (0x40E4B0), so it never places a car.
    Point {
        kind: u32,
        /// `+0x04` — the slice the cop appears at
        slice: i32,
        /// `+0x10` — non-zero: rolling in the outer lane; zero: parked 2 m outside the road edge
        moving: bool,
    },
    /// Type 3 — an explicit world placement. No shipped file contains one.
    Placed { slice: i32, direction: i32, position: [f32; 3], speed_kmh: i32, goto_slice: i32 },
    /// Type 5 — a group with trailing 20-byte sub-records whose contents are UNKNOWN.
    Group { sub_count: u32 },
}

impl CopRecord {
    pub fn slice(&self) -> i32 {
        match self {
            CopRecord::Point { slice, .. } => *slice,
            CopRecord::Placed { slice, .. } => *slice,
            CopRecord::Group { .. } => 0,
        }
    }

    /// Type 2 records are parsed but never spawn a car.
    pub fn spawnable(&self) -> bool {
        !matches!(self, CopRecord::Point { kind: 2, .. } | CopRecord::Group { .. })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CopFile {
    /// sorted ascending by slice, as `CopManager::load` leaves them
    pub records: Vec<CopRecord>,
}

impl CopFile {
    pub fn parse(data: &[u8]) -> Result<CopFile, FormatError> {
        let what = ".cop";
        let count = u32_le(data, 0, what)? as usize;
        if count > 1000 {
            return Err(FormatError::UnexpectedValue { what, offset: 0, expected: "a sane record count".into(), found: count.to_string() });
        }
        let f = |o: usize| -> Result<f32, FormatError> { Ok(f32::from_bits(u32_le(data, o, what)?)) };
        let i = |o: usize| -> Result<i32, FormatError> { Ok(u32_le(data, o, what)? as i32) };
        let mut records = Vec::with_capacity(count);
        let mut p = 4usize;
        for _ in 0..count {
            let kind = u32_le(data, p, what)?;
            let (rec, size) = match kind {
                1 | 2 => (CopRecord::Point { kind, slice: i(p + 4)?, moving: i(p + 0x10)? != 0 }, 0x14),
                3 => (
                    CopRecord::Placed {
                        slice: i(p + 4)?,
                        direction: i(p + 8)?,
                        position: [f(p + 0x0C)?, f(p + 0x10)?, f(p + 0x14)?],
                        speed_kmh: i(p + 0x3C)?,
                        goto_slice: i(p + 0x44)?,
                    },
                    0x48,
                ),
                5 => {
                    let sub = u32_le(data, p + 0x38, what)?;
                    (CopRecord::Group { sub_count: sub }, 0x40 + 20 * sub as usize)
                }
                // the original's parse loop would advance by zero bytes here and spin for ever
                other => return Err(FormatError::UnexpectedValue { what, offset: p, expected: "record type 1, 2, 3 or 5".into(), found: other.to_string() }),
            };
            records.push(rec);
            p += size;
        }
        records.sort_by_key(|r| r.slice());
        Ok(CopFile { records })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(slice: i32, moving: i32) -> Vec<u8> {
        let mut v = Vec::new();
        for x in [1i32, slice, 0, 0, moving] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }

    #[test]
    fn parses_and_sorts_type_one_records() {
        let mut d = 3u32.to_le_bytes().to_vec();
        d.extend(point(1500, 1));
        d.extend(point(100, 0));
        d.extend(point(900, 1));
        let f = CopFile::parse(&d).unwrap();
        assert_eq!(f.records.len(), 3);
        assert_eq!(f.records.iter().map(|r| r.slice()).collect::<Vec<_>>(), vec![100, 900, 1500], "sorted by slice");
        assert_eq!(f.records[0], CopRecord::Point { kind: 1, slice: 100, moving: false });
        assert!(f.records[0].spawnable());
    }

    #[test]
    fn an_unknown_type_is_an_error_not_a_hang() {
        let mut d = 1u32.to_le_bytes().to_vec();
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&[0; 16]);
        assert!(CopFile::parse(&d).is_err());
        // type 2 parses but never spawns
        let mut d2 = 1u32.to_le_bytes().to_vec();
        let mut p = point(10, 1);
        p[0] = 2;
        d2.extend(p);
        assert!(!CopFile::parse(&d2).unwrap().records[0].spawnable());
    }
}
