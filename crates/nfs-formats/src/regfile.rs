//! Registry exports (`regedit` `.reg` files) — where the port finds the Windows registry values
//! the original reads. The install keeps its own in `game_info/regs/*.reg` (UTF-16LE with a BOM,
//! `Windows Registry Editor Version 5.00`); the port has no registry and never writes one.
//!
//! Only what the port reads is decoded: a `"name"="text"` value is [`RegValue::Sz`], every other
//! form (`dword:`, `hex:`, `hex(n):`) is [`RegValue::Other`]. Key paths and value names compare
//! without regard to ASCII case, as the registry's do.

/// The original's own key: 0x467487 builds its registry object from `[0x5D6F1C]` "Electronic
/// Arts" and `[0x5D6F20]` "Need for Speed - Porsche Unleashed", and 0x4B71C1 formats
/// `"SOFTWARE\%s\%s"` (0x5D7064) under `HKEY_LOCAL_MACHINE` (0x80000002, 0x4B71DE).
pub const GAME_KEY: &str = r"HKEY_LOCAL_MACHINE\SOFTWARE\Electronic Arts\Need for Speed - Porsche Unleashed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    /// `"name"="text"` — REG_SZ.
    Sz(String),
    /// any other type, kept undecoded
    Other(String),
}

#[derive(Debug, Clone, Default)]
pub struct RegFile {
    keys: Vec<(String, Vec<(String, RegValue)>)>,
}

impl RegFile {
    /// Parse one export: UTF-16LE when it starts with the BOM `FF FE`, else bytes as Latin-1
    /// (`REGEDIT4`).
    pub fn parse(bytes: &[u8]) -> RegFile {
        let text = if bytes.starts_with(&[0xFF, 0xFE]) {
            let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| crate::bytes::read_u16(c, 0)).collect();
            String::from_utf16_lossy(&units)
        } else {
            bytes.iter().map(|&b| b as char).collect()
        };
        let mut f = RegFile::default();
        for line in text.lines() {
            let line = line.trim();
            if let Some(key) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                f.keys.push((key.to_string(), Vec::new()));
            } else if let (Some(rest), Some((_, values))) = (line.strip_prefix('"'), f.keys.last_mut()) {
                let Some((name, data)) = rest.split_once("\"=") else { continue };
                let value = match data.strip_prefix('"').and_then(|d| d.strip_suffix('"')) {
                    Some(s) => RegValue::Sz(s.replace("\\\\", "\\").replace("\\\"", "\"")),
                    None => RegValue::Other(data.to_string()),
                };
                values.push((name.to_string(), value));
            }
        }
        f
    }

    /// Append another export's keys (a later file's value wins, as importing it would).
    pub fn merge(&mut self, other: RegFile) {
        self.keys.extend(other.keys);
    }

    /// `name` under `key`, the last one any merged file set.
    pub fn value(&self, key: &str, name: &str) -> Option<&RegValue> {
        self.keys
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(key))
            .flat_map(|(_, v)| v.iter())
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
            .next_back()
    }
}

/// **0x4B73F0** — the language index from the registry value `Language` (the name `[0x5D6F64]`).
/// A missing value is written as `"English"` (REG_SZ, 0x4B7434…0x4B7474) and read again, so it
/// counts as `English`. The text maps by exact, case-sensitive `strcmp` (0x4B74DD…0x4B76E6):
/// `English` 0, `German` 1, `French` 2, `Spanish` 3, `Italian` 4, `Swedish` 5, `PortBrzl` 6;
/// anything else is 0 — a value of another type too (its raw bytes meet the same compares). A
/// failed query (missing, or longer than the 0xFF-byte buffer) takes the write-and-retry arm; the
/// name matches none of the seven either way.
pub fn language_index(v: Option<&RegValue>) -> u16 {
    let s = match v {
        None => "English",
        Some(RegValue::Sz(s)) => s.as_str(),
        Some(_) => return 0,
    };
    ["English", "German", "French", "Spanish", "Italian", "Swedish", "PortBrzl"].iter().position(|n| *n == s).unwrap_or(0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xFF, 0xFE];
        b.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        b
    }

    #[test]
    fn reads_the_install_shape() {
        let f = RegFile::parse(&utf16(
            "Windows Registry Editor Version 5.00\r\n\r\n[HKEY_LOCAL_MACHINE\\Software\\Electronic Arts\\Need for Speed - Porsche Unleashed]\r\n\"Install Dir\"=\"C:\\\\NFSPU\"\r\n\"Language\"=\"German\"\r\n\r\n[HKEY_LOCAL_MACHINE\\Software\\Electronic Arts\\Need for Speed - Porsche Unleashed\\1.0]\r\n\"Language\"=dword:00000001\r\n",
        ));
        assert_eq!(f.value(GAME_KEY, "Install Dir"), Some(&RegValue::Sz("C:\\NFSPU".into())));
        assert_eq!(language_index(f.value(GAME_KEY, "language")), 1);
        let sub = format!("{GAME_KEY}\\1.0");
        assert_eq!(language_index(f.value(&sub, "Language")), 0);
    }

    #[test]
    fn language_names_are_exact() {
        let sz = |s: &str| RegValue::Sz(s.into());
        assert_eq!(language_index(None), 0);
        assert_eq!(language_index(Some(&sz("PortBrzl"))), 6);
        assert_eq!(language_index(Some(&sz("Swedish"))), 5);
        // case-sensitive strcmp: nfs5.ini's lower-case spelling would not match
        assert_eq!(language_index(Some(&sz("german"))), 0);
        assert_eq!(language_index(Some(&sz("Russian"))), 0);
    }

    /// Guard for the UTF-16 decode: a surrogate pair, and an odd trailing byte that is dropped.
    #[test]
    fn utf16_decode_edges() {
        let mut b = utf16("[K]\r\n\"a\"=\"\u{1F600}x\"\r\n");
        b.push(0x41);
        let f = RegFile::parse(&b);
        assert_eq!(f.value("k", "A"), Some(&RegValue::Sz("\u{1F600}x".into())));
        let lossy = RegFile::parse(&[0xFF, 0xFE, 0x00, 0xD8, 0x5B, 0x00, 0x4B, 0x00, 0x5D, 0x00, 0x0A, 0x00, 0x22, 0x00, 0x61, 0x00, 0x22, 0x00, 0x3D, 0x00, 0x22, 0x00, 0x62, 0x00, 0x22, 0x00]);
        assert_eq!(lossy.value("K", "a"), None); // the lone surrogate becomes U+FFFD before '[': not a key line
    }
}
