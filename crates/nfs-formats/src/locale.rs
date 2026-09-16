//! `FEData/Locale/FEStrings.csv` — the front-end string table.
//!
//! Latin-1 CSV with CRLF (and stray CR / NEL) line ends; column 0 = numeric string id, column 1 =
//! English text, further columns = other languages. Quoted fields follow the usual CSV rules
//! (`""` = literal quote, commas and line breaks allowed inside quotes). Ids referenced by the
//! career tables (`RE/notes/career.md`) are looked up here.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct Strings {
    pub by_id: HashMap<u32, String>,
}

impl Strings {
    /// Parse the CSV, keeping the column `lang` (0 = English, 1 = French, 2 = German, …).
    pub fn parse(data: &[u8], lang: usize) -> Strings {
        let text: String = data.iter().map(|&b| b as char).collect();
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
}
