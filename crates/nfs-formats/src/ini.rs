//! INI-style text files used throughout the game data: `Track/<t>.map`, `CarModel/<c>.tpg`,
//! `CarModel/<c>.clr`, `FEData/Layouts/*.lay`.
//!
//! Status: CONFIRMED text grammar (`[section]`, `key=value`, CRLF, `;`/`//` comments observed in
//! `.lay`). Duplicate keys inside a section are preserved in order (`.map` repeats `texture1=`).

#[derive(Debug, Clone, Default, PartialEq)]
pub struct IniSection {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

impl IniSection {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.entries.iter().filter(move |(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
    pub fn get_f32(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }
    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.get(key)?.trim().parse().ok()
    }
    /// Parse a comma-separated list of floats (e.g. `tint=1.0,0.57,0.57,0.57`).
    pub fn get_f32_list(&self, key: &str) -> Option<Vec<f32>> {
        self.get(key)?.split(',').map(|s| s.trim().parse().ok()).collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ini {
    pub sections: Vec<IniSection>,
}

impl Ini {
    pub fn parse(text: &str) -> Self {
        let mut sections: Vec<IniSection> = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with("//") || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let name = rest.trim_end_matches(']').trim().to_string();
                sections.push(IniSection { name, entries: Vec::new() });
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                if sections.is_empty() {
                    sections.push(IniSection::default());
                }
                sections.last_mut().unwrap().entries.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        Self { sections }
    }

    pub fn section(&self, name: &str) -> Option<&IniSection> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_sections_and_duplicates() {
        let ini = Ini::parse("[header]\r\ncount=2\r\n[section0]\r\ntexture1=1\r\ntexture1=2\r\ntint=1,0.5\r\n");
        assert_eq!(ini.section("header").unwrap().get_i64("count"), Some(2));
        let s = ini.section("section0").unwrap();
        assert_eq!(s.get_all("texture1").collect::<Vec<_>>(), vec!["1", "2"]);
        assert_eq!(s.get_f32_list("tint"), Some(vec![1.0, 0.5]));
    }
}
