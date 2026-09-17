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
        for row in csv_rows(&text) {
            let Some(id) = row.first().and_then(|f| f.trim().parse::<u32>().ok()) else { continue };
            if let Some(s) = row.get(1 + lang) {
                by_id.insert(id, s.clone());
            }
        }
        Strings { by_id }
    }

    pub fn get(&self, id: u32) -> Option<&str> {
        self.by_id.get(&id).map(|s| s.as_str())
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
