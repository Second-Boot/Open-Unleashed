//! `FEData/Layouts/*.lay` — the front-end screen definitions.
//!
//! Plain text, one `[section.N]` block per widget, `key=value` lines inside it. Every block names its
//! widget class in `id` (`NFS5.PICTURE`, `REAL.LABEL`, `NFS5.GOTOBUTTON`, …) and its instance in `name`
//! (`ID_SCREEN`, `ID_PICTURE`, …). The first block of a screen is the screen itself
//! (`REAL.BLACK_640x480`) and carries the screen-level settings: the room, the music pattern and the
//! callback list. Coordinates are in the 640×480 design space.
//!
//! The file only says what a screen looks like; what a button *does* is in the executable. This parser
//! keeps every key verbatim so the consumer can decide.

use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Element {
    /// `id` — the widget class, e.g. `NFS5.GOTOBUTTON`
    pub class: String,
    /// `name` — the instance name, e.g. `ID_MERGEDLISTCARSETUP`
    pub name: String,
    pub fields: HashMap<String, String>,
}

impl Element {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(&key.to_lowercase()).map(|s| s.as_str())
    }

    /// A `x,y` pair (`pos`, `size`).
    pub fn pair(&self, key: &str) -> Option<(f32, f32)> {
        let v = self.get(key)?;
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        let v = self.get(key)?.trim();
        if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            i64::from_str_radix(h, 16).ok()
        } else {
            v.parse().ok()
        }
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.get(key)?.trim().parse().ok()
    }

    /// An `0xAARRGGBB` colour as linear-ish RGBA bytes in `[r, g, b, a]` order.
    pub fn color(&self, key: &str) -> Option<[u8; 4]> {
        let v = self.int(key)? as u64;
        Some([((v >> 16) & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, (v & 0xFF) as u8, ((v >> 24) & 0xFF) as u8])
    }

    pub fn position(&self) -> (f32, f32) {
        self.pair("pos").unwrap_or((0.0, 0.0))
    }

    /// `size`, or the `width`/`height` pair the button widgets use instead.
    pub fn size(&self) -> (f32, f32) {
        self.pair("size").unwrap_or_else(|| (self.float("width").unwrap_or(0.0), self.float("height").unwrap_or(0.0)))
    }

    /// `stringID` — an index into `FEStrings.csv`.
    pub fn string_id(&self) -> Option<u32> {
        self.int("stringID").map(|v| v as u32)
    }

    /// `shapePathName` / `shapeIndex` — an `.fsh` in `FEData/Art` and the entry inside it.
    pub fn shape(&self) -> Option<(&str, usize)> {
        let p = self.get("shapePathName")?;
        if p.is_empty() {
            return None;
        }
        Some((p, self.int("shapeIndex").unwrap_or(0).max(0) as usize))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    pub elements: Vec<Element>,
}

impl Layout {
    pub fn parse(text: &str) -> Layout {
        let mut elements = Vec::new();
        let mut cur: Option<Element> = None;
        for line in text.lines() {
            let line = line.trim_end_matches(['\r', '\n']);
            let t = line.trim();
            if t.starts_with('[') {
                if let Some(e) = cur.take() {
                    elements.push(e);
                }
                cur = Some(Element::default());
                continue;
            }
            let Some(e) = cur.as_mut() else { continue };
            let Some((k, v)) = t.split_once('=') else { continue };
            let (k, v) = (k.trim().to_lowercase(), v.trim().to_string());
            match k.as_str() {
                "id" => e.class = v.clone(),
                "name" => e.name = v.clone(),
                _ => {}
            }
            e.fields.insert(k, v);
        }
        if let Some(e) = cur {
            elements.push(e);
        }
        Layout { elements }
    }

    /// The screen block — the first element, which carries `room`, `musicfile` and the screen size.
    pub fn screen(&self) -> Option<&Element> {
        self.elements.first()
    }

    pub fn by_name(&self, name: &str) -> Option<&Element> {
        self.elements.iter().find(|e| e.name.eq_ignore_ascii_case(name))
    }

    pub fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a Element> {
        self.elements.iter().filter(move |e| e.class.eq_ignore_ascii_case(class))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[section.0]\r\nid=REAL.BLACK_640x480\r\nname=ID_SCREEN\r\nsize=640,480\r\ncolor=0xff0000ff\r\nroom=ID_GARAGE\r\nmusicfile=menu*\r\n\r\n[section.1]\r\nid=NFS5.GOTOBUTTON\r\nname=ID_GO\r\npos=101,71\r\nstringID=14707\r\nwidth=162\r\nheight=18\r\nmaincolor=0xFFBC0000\r\n\r\n[section.2]\r\nid=NFS5.PICTURE\r\nname=ID_PICTURE\r\npos=262,99\r\nsize=50,150\r\nshapePathName=Extras.fsh\r\nshapeIndex=3\r\n";

    #[test]
    fn parses_a_screen() {
        let l = Layout::parse(SAMPLE);
        assert_eq!(l.elements.len(), 3);
        let s = l.screen().unwrap();
        assert_eq!(s.class, "REAL.BLACK_640x480");
        assert_eq!(s.size(), (640.0, 480.0));
        assert_eq!(s.get("room"), Some("ID_GARAGE"));
        assert_eq!(s.get("musicfile"), Some("menu*"));
        // 0xff0000ff is opaque blue in AARRGGBB
        assert_eq!(s.color("color"), Some([0x00, 0x00, 0xFF, 0xFF]));

        let b = l.by_name("ID_GO").unwrap();
        assert_eq!(b.position(), (101.0, 71.0));
        assert_eq!(b.size(), (162.0, 18.0), "buttons carry width/height, not size");
        assert_eq!(b.string_id(), Some(14707));
        assert_eq!(b.color("maincolor"), Some([0xBC, 0x00, 0x00, 0xFF]));

        let p = l.of_class("NFS5.PICTURE").next().unwrap();
        assert_eq!(p.shape(), Some(("Extras.fsh", 3)));
        assert_eq!(p.size(), (50.0, 150.0));
        // an empty shape path is "no picture", not a file called ""
        let empty = Layout::parse("[s]\nid=NFS5.PICTURE\nshapePathName=\n");
        assert_eq!(empty.elements[0].shape(), None);
    }
}
