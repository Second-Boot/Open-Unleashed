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
        crate::lc::with_lower(key, "", |k| self.fields.get(k).map(|s| s.as_str()))
    }

    /// A `x,y` pair (`pos`, `size`).
    /// A `x,y` value. Each half is an **expression**, not a literal: 0x527C0C splits the string at
    /// the comma and hands each half to the evaluator 0x52DC40, writing the two results to
    /// `[attr+0x38]` and `[attr+0x3C]`. Five shipped blocks rely on it —
    /// `usedcarparts.lay` §0 and `usedcars.lay` §19 `size=440+8,175+8`, `usedcardamage.lay` §0
    /// `size=563+6,175+6`, `usedcars.lay` §11 `size=276+6,175+6` and `cdplayer.lay` §18
    /// `size=155+78,16` — which a plain `parse()` drops, falling back to the class default.
    pub fn pair(&self, key: &str) -> Option<(f32, f32)> {
        let v = self.get(key)?;
        let (a, b) = v.split_once(',')?;
        Some((number(a)? as f32, number(b)? as f32))
    }

    /// An integer value. 0x527D60: an empty string is 0, a `0x` prefix goes to the hex parser
    /// 0x5242D0, and anything else goes through the same evaluator 0x52DC40.
    /// The components a comma-separated value actually yields, following the front end's own
    /// tokeniser at **0x4B58E9**: it collects characters up to the next comma or NUL, dropping
    /// spaces, tabs and carriage returns, and at 0x4B5936 it **skips an empty token outright** —
    /// no parse, no store, and the running count is not advanced. So `"150,"` yields **one**
    /// value, not two, and a consumer that writes one component per parsed value leaves the rest
    /// of its destination alone. `RE/notes/controls.md` §4.116.
    pub fn parts_of(&self, key: &str) -> Vec<f32> {
        let Some(v) = self.get(key) else { return Vec::new() };
        v.split(',')
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .filter_map(|t| number(t).map(|v| v as f32))
            .collect()
    }

    pub fn int(&self, key: &str) -> Option<i64> {
        let v = self.get(key)?.trim();
        if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            i64::from_str_radix(h, 16).ok()
        } else {
            v.parse().ok().or_else(|| number(v).map(|v| v as i64))
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

/// One component of a `.lay` value as a number: a plain decimal as it is, anything else through
/// the evaluator (`crate::expr`). A `.lay` value has no macro dictionary to draw on at this level,
/// so every name is left unanswered, and an **empty** component is no number at all — the port's
/// reading of the tokeniser 0x4B58E9, which skips an empty token (§4.116), not of the evaluator,
/// which would read it as 0.
fn number(t: &str) -> Option<f64> {
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().or_else(|| crate::expr::eval(t, &|_| crate::expr::Macro::Unanswerable).map(f64::from))
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
