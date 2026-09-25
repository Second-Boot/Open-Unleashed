//! `FEData/Locale/FEStrings.csv` — the front-end string table.
//!
//! Windows-1252 CSV with CRLF (and stray CR / NEL) line ends; column 0 = numeric string id, column 1 =
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
        Strings { by_id, fe_by_id, hud_by_id }
    }


    /// The string for `id`: the front end's own row when the file has one (0x4E06D0 walks the
    /// loaded tables with the front end's first), else the only row there is.
    pub fn get(&self, id: u32) -> Option<&str> {
        self.fe_by_id.get(&id).or_else(|| self.by_id.get(&id)).map(|s| s.as_str())
    }

    /// The string for `id` as the race HUD reads it: the HUD table's row when the file has one,
    /// else the front end's (`get`). The HUD loads its own string table; the four ids both tables
    /// use read the HUD's way here (`IDS_WINNINGS` 18161 is "WINNINGS", not "Test Driver").
    pub fn hud(&self, id: u32) -> Option<&str> {
        self.hud_by_id.get(&id).map(|s| s.as_str()).or_else(|| self.get(id))
    }

    /// The text for `id`, or `#id` when the table lacks it.
    pub fn text(&self, id: u32) -> String {
        self.get(id).map(str::to_owned).unwrap_or_else(|| format!("#{id}"))
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
        assert_eq!(s.text(9), "#9");
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
}
