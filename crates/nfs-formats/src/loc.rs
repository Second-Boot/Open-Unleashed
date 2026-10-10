//! `.loc` string tables: `FEData/Locale/FEStrings.loc` (the front end's strings, `"%sFEStrings.loc"`
//! 0x4E0532), `GameData/DashHud/hud.loc` (the race HUD's, `"%shud.loc"` 0x41E1BF) and
//! `FEData/Locale/<key>.loc` (a car's own strings, `"%s%s.loc"` 0x508C3F, which the showcase
//! screens read).
//!
//! The car's file: the catalogue record's `locale_key` (+0x4D) names it, and `Showcase.Text(<n>)`
//! on `chassis.lay`, `general.lay` and `glancestats.lay` is entry `n` of it.
//!
//! The container is three chunk kinds, each `"TAG", u32 size, …`, as the reader **0x536DA0** and
//! the language setter **0x536D80** use it (CONFIRMED, `RE/notes/hud.md` "FEStrings.loc"):
//!
//! * **`LOCH`** at 0 — the header: `+4` the **offset of the `LOCI` index** (the header's own size,
//!   0x2C; 0x536DAB adds it to the file's base), `+8` flags, bit 0 = "ids go through the index"
//!   (0x536DA5; 1 in every shipped file), `+0xC` a **u16** language count (7, which nothing reads)
//!   and `+0xE` a **u16 slot for the current language** — 0x536D80(file, 0, lang) writes it into
//!   the loaded file — then from `+0x10` one absolute `u32` offset per language, each to a `LOCL`.
//! * **`LOCI`** — `+8` the entry count, then from `+0x10` that many `(u16 id, u16 index)` pairs
//!   sorted by id; 0x536DF0 finds an id with the CRT `bsearch` 0x5A2F23 (compare 0x536E30, the
//!   difference of the two u16s) and answers the index zero-extended, or −1. A car file's ids run
//!   4, 6, 8, … with the index 0, 1, 2, …; `FEStrings.loc`'s and `hud.loc`'s are the string ids.
//!   *(Corrected 2026-09-27: this doc read the pairs as `(entry << 16) | offset`, `+8` as a version
//!   and `+0xC` as a u32 count.)*
//! * **`LOCL`** — one language: `+0xC` the entry count, then that many `u32` offsets **relative to
//!   the block**, each to a NUL-terminated byte string. The table ends exactly where the first
//!   string begins (`0x10 + 44*4 = 0xC0`, and `1948356.loc`'s first offset is `0xC0`).
//!
//! The strings are raw bytes in the code page of the fonts that draw them (Windows-1251 in this
//! install); the port carries each byte as the char [`crate::locale::cp1252`] makes of it, and
//! [`crate::locale::to_cp1252`] gives the same byte back to the glyph lookup.

use crate::bytes::{get_u16, get_u32, read_u16};
use crate::FormatError;

/// The four-byte tags, in the order a file carries them.
pub const HEADER: &[u8; 4] = b"LOCH";
pub const INDEX: &[u8; 4] = b"LOCI";
pub const LANGUAGE: &[u8; 4] = b"LOCL";

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
        let langs = get_u32(d, 0xC).unwrap_or(0) as usize;
        let pick = if lang < langs { lang } else { 0 };
        let block = get_u32(d, 0x10 + pick * 4).ok_or(FormatError::Truncated { what, offset: 0x10 + pick * 4, needed: 4 })? as usize;
        if d.get(block..block + 4) != Some(LANGUAGE) {
            return Err(FormatError::UnknownMagic { what, offset: block, expected: LANGUAGE.to_vec(), found: d.get(block..block + 4).unwrap_or_default().to_vec() });
        }
        let count = get_u32(d, block + 0xC).unwrap_or(0) as usize;
        let mut entries = Vec::with_capacity(crate::cap_hint(count, d.len(), 4));
        for i in 0..count {
            let off = get_u32(d, block + 0x10 + i * 4).unwrap_or(0) as usize + block;
            let end = d[off.min(d.len())..].iter().position(|&b| b == 0).map(|n| off + n).unwrap_or(d.len());
            entries.push(d.get(off..end).unwrap_or_default().iter().map(|&b| crate::locale::cp1252(b)).collect());
        }
        Ok(Self { entries })
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str).filter(|s| !s.is_empty())
    }
}

/// A `.loc` file as the game's string lookup **0x536DA0** reads it: the whole file, the language
/// slot 0x536D80 sets, and [`LocTable::get`].
///
/// ```text
/// 0x536DA0(file, id):
///   index = [file+8] & 1 ? 0x536DF0(id, file + [file+4]) : id          ; 0x536DA5…0x536DC0
///   block = file + [file + 0x10 + 4·(u16)[file+0xE]]                  ; 0x536DC4…0x536DCE
///   0 <= index < [block+0xC] (signed, 0x536DD3/0x536DD8) ? block + [block + 0x10 + 4·index] : NULL
/// 0x536DF0: bsearch((u16)id, LOCI+0x10, [LOCI+8], 4, 0x536E30) -> (u16)pair.index, miss -> −1
/// ```
///
/// Nothing checks the block's `LOCL` tag or the language against `+0xC`: a header offset that does
/// not lead to a block reads whatever bytes are there. The port does the same arithmetic and
/// answers `None` where the arithmetic would leave the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocTable {
    data: Vec<u8>,
    language: u16,
}

impl LocTable {
    /// Take a whole `.loc` file. Only the `LOCH` tag and the header's length are checked (the game
    /// checks nothing; the port uses the tag to tell a table from a missing or foreign file).
    pub fn parse(data: Vec<u8>) -> Result<Self, FormatError> {
        if data.get(0..4) != Some(HEADER) {
            return Err(FormatError::UnknownMagic { what: "loc", offset: 0, expected: HEADER.to_vec(), found: data.get(0..4).unwrap_or_default().to_vec() });
        }
        if data.len() < 0x10 {
            return Err(FormatError::Truncated { what: "loc", offset: 0, needed: 0x10 });
        }
        // before 0x536D80 runs the slot is what the file holds (0 in every shipped one)
        let language = read_u16(&data, 0xE);
        Ok(Self { data, language })
    }

    /// `+0xC` as a u16: the number of language offsets the header lists (7 in every shipped file).
    pub fn language_count(&self) -> u16 {
        read_u16(&self.data, 0xC)
    }

    /// 0x536D80(file, 0, lang): the language every later [`LocTable::get`] reads.
    pub fn set_language(&mut self, lang: u16) {
        self.language = lang;
    }

    pub fn language(&self) -> u16 {
        self.language
    }

    /// The absolute offset of language `lang`'s block as the header gives it.
    pub fn block_offset(&self, lang: u16) -> Option<u32> {
        get_u32(&self.data, 0x10 + 4 * lang as usize)
    }

    fn index_base(&self) -> Option<usize> {
        get_u32(&self.data, 4).map(|o| o as usize)
    }

    /// The `LOCI` pairs in file order: `(id, index)`.
    pub fn index(&self) -> Vec<(u16, u16)> {
        let Some(base) = self.index_base() else { return Vec::new() };
        let n = get_u32(&self.data, base + 8).unwrap_or(0) as usize;
        (0..n)
            .map_while(|i| {
                let o = base + 0x10 + 4 * i;
                Some((get_u16(&self.data, o)?, get_u16(&self.data, o + 2)?))
            })
            .collect()
    }

    /// The ids the index holds, each once, in file (ascending) order.
    pub fn ids(&self) -> Vec<u16> {
        let mut v: Vec<u16> = self.index().into_iter().map(|(id, _)| id).collect();
        v.dedup();
        v
    }

    /// 0x536DF0: the index for `id` through the CRT `bsearch` 0x5A2F23 — the MSVC algorithm step
    /// for step, so a duplicated id (`FEStrings.loc` lists 539 twice) resolves to the pair it lands
    /// on — or −1 when there is none.
    fn search(&self, id: u32) -> i32 {
        let Some(base) = self.index_base() else { return -1 };
        // the key is the argument's low word (0x536E3F reads a u16 through the key pointer)
        let key = (id & 0xFFFF) as i32;
        let count = i64::from(get_u32(&self.data, base + 8).unwrap_or(0));
        let pair = |i: i64| -> Option<(i32, i32)> {
            let o = base + 0x10 + 4 * usize::try_from(i).ok()?;
            Some((i32::from(get_u16(&self.data, o)?), i32::from(get_u16(&self.data, o + 2)?)))
        };
        let (mut lo, mut hi, mut num) = (0i64, count - 1, count);
        while lo <= hi {
            let half = num / 2;
            if half == 0 {
                // 0x5A2F95: one element left — it or nothing
                return match pair(lo) {
                    Some((k, ix)) if num != 0 && k == key => ix,
                    _ => -1,
                };
            }
            let odd = num & 1 != 0;
            let mid = lo + if odd { half } else { half - 1 };
            let Some((k, ix)) = pair(mid) else { return -1 };
            match key - k {
                0 => return ix,
                r if r < 0 => {
                    hi = mid - 1;
                    num = if odd { half } else { half - 1 };
                }
                _ => {
                    lo = mid + 1;
                    num = half;
                }
            }
        }
        -1
    }

    /// 0x536DA0(file, id): the string's bytes (without the NUL) in the current language; `None`
    /// is the NULL the game gets back — an id the index lacks, an index outside the block's count.
    pub fn get(&self, id: u32) -> Option<&[u8]> {
        let flags = get_u32(&self.data, 8)?;
        let index = if flags & 1 != 0 { self.search(id) } else { id as i32 };
        let block = get_u32(&self.data, 0x10 + 4 * self.language as usize)? as usize;
        let count = get_u32(&self.data, block + 0xC)? as i32;
        if index < 0 || index >= count {
            return None;
        }
        let off = (get_u32(&self.data, block + 0x10 + 4 * index as usize)? as usize).checked_add(block)?;
        let tail = self.data.get(off..)?;
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        Some(&tail[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-language table built by hand: the index maps ids 5, 9, 9, 20 to 0, 1, 2, 3.
    fn table() -> Vec<u8> {
        fn block(ss: &[&[u8]]) -> Vec<u8> {
            let mut d = Vec::new();
            let len = 0x10 + 4 * ss.len() + ss.iter().map(|s| s.len() + 1).sum::<usize>();
            d.extend_from_slice(b"LOCL");
            d.extend_from_slice(&(len as u32).to_le_bytes());
            d.extend_from_slice(&0u32.to_le_bytes());
            d.extend_from_slice(&(ss.len() as u32).to_le_bytes());
            let mut at = 0x10 + 4 * ss.len();
            for s in ss {
                d.extend_from_slice(&(at as u32).to_le_bytes());
                at += s.len() + 1;
            }
            for s in ss {
                d.extend_from_slice(s);
                d.push(0);
            }
            d
        }
        let pairs = [(5u16, 0u16), (9, 1), (9, 2), (20, 3)];
        let b0 = block(&[b"five", b"nine a", b"nine b", &[0xC0, 0xE1]]);
        let b1 = block(&[b"cinq"]);
        let lang0 = 0x2C + 0x10 + 4 * pairs.len();
        let mut d = Vec::new();
        d.extend_from_slice(b"LOCH");
        d.extend_from_slice(&0x2Cu32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&(lang0 as u32).to_le_bytes());
        d.extend_from_slice(&((lang0 + b0.len()) as u32).to_le_bytes());
        d.resize(0x2C, 0);
        d.extend_from_slice(b"LOCI");
        d.extend_from_slice(&((0x10 + 4 * pairs.len()) as u32).to_le_bytes());
        d.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        for (k, v) in pairs {
            d.extend_from_slice(&k.to_le_bytes());
            d.extend_from_slice(&v.to_le_bytes());
        }
        assert_eq!(d.len(), lang0);
        d.extend(b0);
        d.extend(b1);
        d
    }

    #[test]
    fn lookup_by_id_in_the_chosen_language() {
        let mut t = LocTable::parse(table()).unwrap();
        assert_eq!(t.language_count(), 2);
        assert_eq!(t.get(5), Some(&b"five"[..]));
        // the bytes come back as they are
        assert_eq!(t.get(20), Some(&[0xC0u8, 0xE1][..]));
        // an id the index lacks is NULL; only the low 16 bits are compared (0x536E30)
        assert_eq!(t.get(6), None);
        assert_eq!(t.get(5 + 0x10000), Some(&b"five"[..]));
        // bsearch over four pairs probes mid = 0 + (2 − 1) = 1 first: the first 9
        assert_eq!(t.get(9), Some(&b"nine a"[..]));
        assert_eq!(t.ids(), vec![5, 9, 20]);
        // language 1 has one entry: index 0 reads, index 3 is past its count
        t.set_language(1);
        assert_eq!(t.get(5), Some(&b"cinq"[..]));
        assert_eq!(t.get(20), None);
    }

    #[test]
    fn not_a_table() {
        assert!(LocTable::parse(b"LOCX\0\0\0\0\0\0\0\0\0\0\0\0".to_vec()).is_err());
        assert!(LocTable::parse(b"LOCH".to_vec()).is_err());
    }
}
