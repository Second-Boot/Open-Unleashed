//! The front end's and the HUD's string tables.
//!
//! The original reads **`FEStrings.loc`** and **`hud.loc`** ([`Strings::from_loc`],
//! `crate::loc::LocTable`); nothing in `nfs5.exe` names `FEStrings.csv`. The CSV is the port's
//! fallback for an install without the `.loc` (`nfs_game::layout::load_strings`):
//!
//! `FEData/Locale/FEStrings.csv` — Windows-1252 CSV with CRLF (and stray CR / NEL) line ends; column 0 = numeric string id, column 1 =
//! English text, further columns = other languages. Quoted fields follow the usual CSV rules
//! (`""` = literal quote, commas and line breaks allowed inside quotes). Ids referenced by the
//! career tables (`RE/notes/career.md`) are looked up here.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Strings {
    pub by_id: HashMap<u32, String>,
    /// The rows whose `Conditionals` column (9) names `fe` — the front end's own table
    /// (`festrings.h`), the first such row per id. The file carries the HUD table (`hud`) in the
    /// same id space and 36 ids are in both (`IDS_ERACONGRATS` 18164 is "Congratulations" for the
    /// front end and "Observe previous car" for the HUD); `by_id` keeps whichever row came last,
    /// and [`Strings::get`] asks this map first.
    pub fe_by_id: HashMap<u32, String>,
    /// The rows whose `Conditionals` names `hud` — the race HUD's own table (`hudstrings.h`), the
    /// first per id. Four ids are in both tables: 18161…18164 are `IDS_TESTDRIVER` … /
    /// `IDS_ERACONGRATS` for the front end and `IDS_WINNINGS`, `RALLY TIME`, the two observe
    /// hints for the HUD. [`Strings::hud`] asks this map first.
    pub hud_by_id: HashMap<u32, String>,
    /// Whether `hud_by_id` is the HUD's own `hud.loc` ([`Strings::from_loc`]): the HUD reads that
    /// table and nothing else (0x536DA0 on `[0x60C974]`, 0x41E160), so [`Strings::hud`] does not
    /// fall back to the front end's.
    pub hud_table: bool,
}

/// Windows-1252 → Unicode. The shipped text uses the 0x80..0x9F block (typographic quotes and
/// dashes) that Latin-1 leaves as control characters, so decoding byte-for-byte turns `I’m` into a
/// stray control code.
pub fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{8D}', '\u{017D}', '\u{8F}',
        '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{9D}', '\u{017E}', '\u{0178}',
    ];
    if (0x80..0xA0).contains(&b) { HIGH[(b - 0x80) as usize] } else { b as char }
}

/// Unicode → the Windows-1252 byte the original's text holds, the inverse of [`cp1252`]: the
/// strings the game draws are byte strings and its fonts are keyed by those bytes (0x573AB0 hands
/// the glyph lookup one unsigned byte at a time, `RE/notes/hud.md` "Glyph lookup"). A char below
/// U+0100 is its own byte — which also takes back the C1 controls a byte-for-byte (`b as char`)
/// decode leaves for 0x80…0x9F — and the typographic block maps back to 0x80…0x9F. `None` for a
/// char no Windows-1252 byte stands for (the original never holds one).
pub fn to_cp1252(ch: char) -> Option<u8> {
    let c = ch as u32;
    if c < 0x100 {
        return Some(c as u8);
    }
    (0x80u8..0xA0).find(|&b| cp1252(b) == ch)
}

/// A key the player typed → the char the game's own byte strings hold for it, or `None`.
///
/// The original takes typed text as `WM_CHAR` in the system's ANSI code page — one byte per key
/// (0x4CCC20's `0x20..0xFF` test runs on that byte) — and draws it with fonts keyed by bytes. The
/// shipped Russian text and fonts are Windows-1251 bytes, so on the Russian Windows this build
/// targets a Cyrillic key arrives as its 1251 byte. The port stores strings as the byte decoded
/// through [`cp1252`], so a Latin/Windows-1252 char maps to itself and a Cyrillic letter to
/// `cp1252(its Windows-1251 byte)`, which the byte-keyed font then draws as that letter.
/// APPROXIMATION: the original's mapping is whatever ANSI code page the Windows install has; this
/// assumes the Russian one (1251) for Cyrillic keys — on another code page the original would
/// accept fewer letters (it errs toward accepting Cyrillic the original would take only on a
/// Russian system).
pub fn typed_char(ch: char) -> Option<char> {
    if let Some(b) = to_cp1252(ch) {
        return Some(cp1252(b));
    }
    let b = match ch as u32 {
        c @ 0x0410..=0x044F => (c - 0x0410 + 0xC0) as u8,
        0x0401 => 0xA8,
        0x0451 => 0xB8,
        _ => return None,
    };
    Some(cp1252(b))
}

impl Strings {
    /// Parse the CSV, keeping the column `lang` (0 = English, 1 = French, 2 = German, …).
    pub fn parse(data: &[u8], lang: usize) -> Strings {
        let text: String = data.iter().map(|&b| cp1252(b)).collect();
        let mut by_id = HashMap::new();
        let mut fe_by_id = HashMap::new();
        let mut hud_by_id = HashMap::new();
        for row in csv_rows(&text) {
            let Some(id) = row.first().and_then(|f| f.trim().parse::<u32>().ok()) else { continue };
            if let Some(s) = row.get(1 + lang) {
                by_id.insert(id, s.clone());
                let tagged = |tag: &str| row.get(9).is_some_and(|c| c.split('|').any(|t| t.trim().eq_ignore_ascii_case(tag)));
                if tagged("fe") {
                    fe_by_id.entry(id).or_insert_with(|| s.clone());
                }
                if tagged("hud") {
                    hud_by_id.entry(id).or_insert_with(|| s.clone());
                }
            }
        }
        Strings { by_id, fe_by_id, hud_by_id, hud_table: false }
    }

    /// The original's tables: `FEStrings.loc` for the front end (the one table 0x4E04F0 puts in
    /// the list 0x4E06D0 walks, `[0x690714]`) and `hud.loc` for the HUD (0x41E1DD, `[0x60C974]`),
    /// each already set to its language (0x536D80). Every id of the index is looked up the way
    /// 0x536DA0 does (so a duplicated id holds what the bsearch finds) and kept when the lookup
    /// answers. The bytes are carried as [`cp1252`] chars — one char per byte, which
    /// [`to_cp1252`] and the glyph lookup turn back into the same byte, whatever code page the
    /// file is in (Windows-1251 here).
    pub fn from_loc(fe: &crate::loc::LocTable, hud: Option<&crate::loc::LocTable>) -> Strings {
        let read = |t: &crate::loc::LocTable| -> HashMap<u32, String> {
            t.ids().into_iter().filter_map(|id| Some((u32::from(id), t.get(u32::from(id))?.iter().map(|&b| cp1252(b)).collect()))).collect()
        };
        let fe_by_id = read(fe);
        Strings { by_id: fe_by_id.clone(), fe_by_id, hud_by_id: hud.map(read).unwrap_or_default(), hud_table: hud.is_some() }
    }


    /// The string for `id`: the front end's own row when the file has one (0x4E06D0 walks the
    /// loaded tables with the front end's first), else the only row there is.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.fe_by_id.get(&id).or_else(|| self.by_id.get(&id)).map(|s| s.as_str())
    }

    /// The string for `id` as the race HUD reads it. With `hud.loc` loaded ([`Strings::from_loc`])
    /// that table only — 0x536DA0(0x41E160(), id), NULL (`None`) for an id it lacks. From the CSV
    /// fallback: the `hud` row when the file has one, else the front end's (`get`) — the port's
    /// own rule for a table the original never reads. The four ids both tables use read the HUD's
    /// way (`IDS_WINNINGS` 18161 is "WINNINGS" / "ПРИЗ", not "Test Driver").
    pub fn hud(&self, id: u32) -> Option<&str> {
        let own = self.hud_by_id.get(&id).map(|s| s.as_str());
        if self.hud_table { own } else { own.or_else(|| self.get(id)) }
    }

    /// The text for `id` as the front end's accessor **0x4E06D0** answers it: id 0 is the shared
    /// empty string 0x5E8E50 (0x4E06D7…0x4E06E3, before any table is asked), an id no table
    /// holds is **`"Not Found"`** (0x5D9B38, 0x4E073D). Its third arm, `sprintf("%d", id)` while
    /// `[0x690718]` is set (0x4E06E4), is not ported: nothing writes that byte.
    pub fn text(&self, id: u32) -> String {
        if id == 0 {
            return String::new();
        }
        self.get(id).unwrap_or("Not Found").to_owned()
    }
}

fn csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => quoted = true,
            ',' => row.push(std::mem::take(&mut field)),
            '\r' | '\n' | '\u{85}' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An id both tables give reads the front end's row; a HUD-only id still reads.
    #[test]
    fn front_end_rows_win() {
        let csv = b"1,Congratulations,,,,,,,IDS_A,fe|needh,,!fe\r\n2,WINNINGS,,,,,,,IDS_W,hud,,!hud\r\n1,Observe previous car,,,,,,,IDS_B,hud,,!hud\r\n";
        let s = Strings::parse(csv, 0);
        assert_eq!(s.get(1), Some("Congratulations"));
        assert_eq!(s.get(2), Some("WINNINGS"));
    }

    #[test]
    fn quotes_and_line_ends() {
        let s = Strings::parse(b"String ID,ENGLISH\r\n0,\"Hello, World!\",x\r\n2,\"\"\"q\"\"\",y\r5,plain\n", 0);
        assert_eq!(s.get(0), Some("Hello, World!"));
        assert_eq!(s.get(2), Some("\"q\""));
        assert_eq!(s.get(5), Some("plain"));
        assert_eq!(s.text(9), "Not Found");
        // 0x4E06D7: id 0 answers "" even when the table has a row 0
        assert_eq!(s.text(0), "");
    }

    #[test]
    fn windows_1252_punctuation_decodes() {
        // 0x92 is the right single quote the shipped text uses for apostrophes
        assert_eq!(cp1252(0x92), '\u{2019}');
        assert_eq!(cp1252(0x93), '\u{201C}');
        assert_eq!(cp1252(0x97), '\u{2014}');
        // the Latin-1 range is unchanged
        assert_eq!(cp1252(b'A'), 'A');
        assert_eq!(cp1252(0xE9), '\u{E9}');
        let s = Strings::parse(b"1,I\x92m sorry\r\n", 0);
        assert_eq!(s.get(1), Some("I\u{2019}m sorry"));
    }

    #[test]
    fn to_cp1252_inverts_the_decode() {
        for b in 0..=255u8 {
            assert_eq!(to_cp1252(cp1252(b)), Some(b), "byte {b:#04X}");
            assert_eq!(to_cp1252(b as char), Some(b), "Latin-1 {b:#04X}");
        }
        assert_eq!(to_cp1252('\u{2019}'), Some(0x92));
        assert_eq!(to_cp1252('\u{0100}'), None);
    }
}
