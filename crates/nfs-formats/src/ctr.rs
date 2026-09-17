//! `FEData/Controls/*.ctr` — the front-end's **widget class templates**.
//!
//! A `.lay` block only carries the keys the artist changed; everything else — what a widget is made
//! of, where each piece sits, what font and colour it uses, and the default of every attribute —
//! lives in one of the 72 `.ctr` files that ship beside the layouts. `Control Template.txt` in the
//! same directory is the authors' own commented blank. The format is the same plain `key=value`
//! text as a `.lay`, with `;` comments, in three kinds of section:
//!
//! * `[control]` — the class itself: its `id` (`NFS5.ListBox`, matching a `.lay` block's `id`), the
//!   list of `parts`, the `states` it supports, and its own `pos.value` / `size.value`.
//! * `[state.<name>]` — which parts are drawn in that state; the `.lay` block's `state` key (always
//!   `normal` in the shipped screens) picks one.
//! * `[part.<name>]` — one piece of the widget. `id=DATA` is the attribute block (its
//!   `attributes.value` lists the keys the `.lay` may set, each with a `<key>.type` and a
//!   `<key>.value` default); `RECT`, `SHAPE`, `TEXT` and `LOCALIZEDTEXT` are the drawable pieces,
//!   each with its own `pos.value`, `size.value`, `color.value`, `fontPathName.value` …
//!
//! Geometry values are little **expressions** rather than plain numbers, e.g. the list box's panel
//! is `size.value=data.width,data.selectionboxmaxheight` and an accessor watch's label sits at
//! `pos.value=(0-label.size.width)/2*data.align,0` — the alignment rule, written out in the data.
//! [`eval`] evaluates one against a resolver that answers `data.width`, `label.size.width`,
//! `control.size.height` and the like.
//!
//! Status: CONFIRMED as a reading of the data (all 72 files parse, every expression evaluates); the
//! executable's own use of them was traced only for the list box (`RE/notes/controls.md`).

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// the attribute block — no geometry of its own
    Data,
    Rect,
    Shape,
    /// a string from `FEStrings.csv`, by `stringID`
    LocalizedText,
    /// a literal string in `text.value`
    Text,
    /// a nested control class, named by its own `id`
    Control,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Part {
    pub name: String,
    /// `id=` inside the part: `DATA`, `RECT`, `SHAPE`, `LOCALIZEDTEXT`, `TEXT`, or a control class
    pub id: String,
    /// `attributes.value`, for a `DATA` part: the keys a `.lay` block may set
    pub attributes: Vec<String>,
    /// every `key=value` line of the section, keyed in lower case exactly as written
    /// (`pos.value`, `size.flags`, `foreColor.value`, …)
    pub fields: HashMap<String, String>,
}

impl Part {
    pub fn kind(&self) -> PartKind {
        match self.id.trim().to_ascii_uppercase().as_str() {
            "DATA" => PartKind::Data,
            "RECT" => PartKind::Rect,
            "SHAPE" => PartKind::Shape,
            "LOCALIZEDTEXT" => PartKind::LocalizedText,
            "TEXT" => PartKind::Text,
            _ => PartKind::Control,
        }
    }

    /// `<key>.value` — the default for that key.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.fields.get(&format!("{}.value", key.to_lowercase())).map(|s| s.as_str())
    }

    /// `<key>.type` — `INT`, `STRING`, `COLOR`, …
    pub fn ty(&self, key: &str) -> Option<&str> {
        self.fields.get(&format!("{}.type", key.to_lowercase())).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Control {
    /// `[control] id`, e.g. `NFS5.ListBox` — matches a `.lay` block's `id`
    pub id: String,
    pub description: String,
    /// `[control] parts`, in the order the file lists them
    pub parts_order: Vec<String>,
    /// `[state.<name>] parts` — the pieces drawn in that state
    pub states: HashMap<String, Vec<String>>,
    /// the `[control]` section's own keys (`pos.value`, `size.value`, `name.value`, …)
    pub fields: HashMap<String, String>,
    pub parts: HashMap<String, Part>,
}

impl Control {
    pub fn value(&self, key: &str) -> Option<&str> {
        self.fields.get(&format!("{}.value", key.to_lowercase())).map(|s| s.as_str())
    }

    pub fn part(&self, name: &str) -> Option<&Part> {
        self.parts.get(&name.to_lowercase())
    }

    /// The `DATA` part — the one that holds the attributes a `.lay` block sets.
    pub fn data(&self) -> Option<&Part> {
        self.parts.values().find(|p| p.kind() == PartKind::Data)
    }

    /// The parts drawn in `state`, in the order that state lists them; the parts the `[control]`
    /// section lists when the state is unknown.
    pub fn state_parts(&self, state: &str) -> &[String] {
        self.states.get(&state.to_lowercase()).map(|v| v.as_slice()).unwrap_or(&self.parts_order)
    }

    /// The default of a `DATA` attribute, e.g. `width` → `100`.
    pub fn default_of(&self, key: &str) -> Option<&str> {
        self.data()?.value(key)
    }

    pub fn parse(text: &str) -> Control {
        let mut c = Control::default();
        let mut section = String::new();
        let mut part: Option<Part> = None;
        for raw in text.lines() {
            let line = raw.split(';').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                if let Some(p) = part.take() {
                    c.parts.insert(p.name.clone(), p);
                }
                section = name.trim().to_lowercase();
                if let Some(pn) = section.strip_prefix("part.") {
                    part = Some(Part { name: pn.to_string(), ..Part::default() });
                }
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim().to_lowercase(), v.trim().to_string());
            match part.as_mut() {
                Some(p) => {
                    match k.as_str() {
                        "id" => p.id = v.clone(),
                        "attributes.value" => p.attributes = v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect(),
                        _ => {}
                    }
                    p.fields.insert(k, v);
                }
                None if section == "control" => {
                    match k.as_str() {
                        "id" => c.id = v.clone(),
                        "description" => c.description = v.clone(),
                        "parts" => c.parts_order = v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect(),
                        _ => {}
                    }
                    c.fields.insert(k, v);
                }
                None => {
                    if let Some(st) = section.strip_prefix("state.")
                        && k == "parts"
                    {
                        c.states.insert(st.to_string(), v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect());
                    }
                }
            }
        }
        if let Some(p) = part.take() {
            c.parts.insert(p.name.clone(), p);
        }
        c
    }
}

/// Every `.ctr` in `FEData/Controls`, keyed by the class id a `.lay` block names
/// (lower-cased — the files and the layouts disagree on case, e.g. `NFS5.ListBox` vs `NFS5.LISTBOX`).
#[derive(Debug, Clone, Default)]
pub struct ControlLibrary {
    controls: HashMap<String, Control>,
}

impl ControlLibrary {
    pub fn from_files<I: IntoIterator<Item = String>>(files: I) -> ControlLibrary {
        let mut lib = ControlLibrary::default();
        for text in files {
            let c = Control::parse(&text);
            if !c.id.is_empty() {
                lib.controls.insert(c.id.to_lowercase(), c);
            }
        }
        lib
    }

    pub fn get(&self, class: &str) -> Option<&Control> {
        self.controls.get(&class.to_lowercase())
    }

    pub fn len(&self) -> usize {
        self.controls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }
}

/// Evaluate one `.ctr` value expression. `resolve` answers a dotted reference (`data.width`,
/// `label.size.width`, `control.size.height`); an unknown one makes the whole expression `None`, so
/// a caller can tell "no value" from "zero".
///
/// The grammar is the subset the 72 shipped files use: decimal and `0x` literals, `+ - * /`, `&`,
/// the ternary `?:`, unary minus and parentheses.
pub fn eval(expr: &str, resolve: &dyn Fn(&str) -> Option<f64>) -> Option<f64> {
    let tokens = lex(expr)?;
    let mut p = Parser { t: &tokens, i: 0, resolve };
    let v = p.ternary()?;
    if p.i == p.t.len() { Some(v) } else { None }
}

/// Evaluate a `x,y` pair; a value with no comma resolves both halves from the single expression's
/// first component only (`size.value=454` — the file's own shorthand), and an empty value is `None`.
pub fn eval_pair(expr: &str, resolve: &dyn Fn(&str) -> Option<f64>) -> Option<(f64, f64)> {
    let (a, b) = split_pair(expr)?;
    Some((eval(a, resolve)?, match b {
        Some(b) => eval(b, resolve)?,
        None => 0.0,
    }))
}

/// Split `a,b` at the top-level comma (the expressions themselves never contain one).
fn split_pair(expr: &str) -> Option<(&str, Option<&str>)> {
    let e = expr.trim();
    if e.is_empty() {
        return None;
    }
    let mut depth = 0i32;
    for (i, ch) in e.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return Some((&e[..i], Some(&e[i + 1..]))),
            _ => {}
        }
    }
    Some((e, None))
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Name(String),
    Op(char),
}

fn lex(s: &str) -> Option<Vec<Tok>> {
    let b: Vec<char> = s.trim().chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            if c == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
                i += 2;
                while i < b.len() && b[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex: String = b[start + 2..i].iter().collect();
                out.push(Tok::Num(u64::from_str_radix(&hex, 16).ok()? as f64));
                continue;
            }
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                i += 1;
            }
            let n: String = b[start..i].iter().collect();
            out.push(Tok::Num(n.parse().ok()?));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '.') {
                i += 1;
            }
            out.push(Tok::Name(b[start..i].iter().collect::<String>().to_lowercase()));
        } else if "+-*/&?:()".contains(c) {
            out.push(Tok::Op(c));
            i += 1;
        } else {
            return None;
        }
    }
    Some(out)
}

struct Parser<'a> {
    t: &'a [Tok],
    i: usize,
    resolve: &'a dyn Fn(&str) -> Option<f64>,
}

impl Parser<'_> {
    fn eat(&mut self, c: char) -> bool {
        if self.t.get(self.i) == Some(&Tok::Op(c)) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn ternary(&mut self) -> Option<f64> {
        let c = self.bitand()?;
        if self.eat('?') {
            let a = self.ternary()?;
            if !self.eat(':') {
                return None;
            }
            let b = self.ternary()?;
            return Some(if c != 0.0 { a } else { b });
        }
        Some(c)
    }

    fn bitand(&mut self) -> Option<f64> {
        let mut v = self.sum()?;
        while self.eat('&') {
            let r = self.sum()?;
            v = ((v as i64) & (r as i64)) as f64;
        }
        Some(v)
    }

    fn sum(&mut self) -> Option<f64> {
        let mut v = self.product()?;
        loop {
            if self.eat('+') {
                v += self.product()?;
            } else if self.eat('-') {
                v -= self.product()?;
            } else {
                return Some(v);
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            if self.eat('*') {
                v *= self.unary()?;
            } else if self.eat('/') {
                let d = self.unary()?;
                // the originals are integer expressions; a divide by zero would be a broken file
                if d == 0.0 {
                    return None;
                }
                v = (v / d).trunc();
            } else {
                return Some(v);
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        if self.eat('-') {
            return Some(-self.unary()?);
        }
        if self.eat('+') {
            return self.unary();
        }
        if self.eat('(') {
            let v = self.ternary()?;
            if !self.eat(')') {
                return None;
            }
            return Some(v);
        }
        match self.t.get(self.i)? {
            Tok::Num(n) => {
                self.i += 1;
                Some(*n)
            }
            Tok::Name(n) => {
                let n = n.clone();
                self.i += 1;
                (self.resolve)(&n)
            }
            Tok::Op(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<f64> {
        None
    }

    #[test]
    fn arithmetic_matches_the_files_own_expressions() {
        assert_eq!(eval("640-(113+112)-79", &none), Some(336.0));
        assert_eq!(eval("548-(158+100)-116", &none), Some(174.0));
        // integer division, as the layout coordinates are whole pixels
        assert_eq!(eval("(0-100)/2*2", &none), Some(-100.0));
        assert_eq!(eval("0x80000000", &none), Some(2147483648.0));
    }

    #[test]
    fn references_and_the_ternary() {
        let r = |n: &str| match n {
            "data.width" => Some(438.0),
            "label.size.width" => Some(40.0),
            "data.align" => Some(1.0),
            "data.posx" => Some(10.0),
            "data.space" => Some(4.0),
            "picture.size.width" => Some(16.0),
            _ => None,
        };
        // the accessor watch's alignment rule, straight out of `accessor Watch.ctr`
        assert_eq!(eval("(0-label.size.width)/2*data.align", &r), Some(-20.0));
        assert_eq!(eval("data.width-10-label.size.width-3", &r), Some(385.0));
        assert_eq!(eval("(data.align&1)?data.posx-(label.size.width+picture.size.width+data.space):data.posx", &r), Some(-50.0));
        // an unresolved name is not zero
        assert_eq!(eval("data.height", &r), None);
    }

    #[test]
    fn pairs_split_at_the_top_level_comma() {
        let r = |n: &str| (n == "data.width").then_some(438.0);
        assert_eq!(eval_pair("data.width,1", &r), Some((438.0, 1.0)));
        assert_eq!(eval_pair("454", &r), Some((454.0, 0.0)));
        assert_eq!(eval_pair("", &r), None);
    }

    #[test]
    fn a_control_parses_into_parts_and_states() {
        let c = Control::parse(
            "; a comment\n[control]\nid=NFS5.ListBox\nparts=data,selectionbox\nsize.value=data.width,data.selectionboxmaxheight\n\n[state.normal]\nparts=data,selectionbox\n\n[part.data]\nid=DATA\nattributes.value=accessor,width\nwidth.type=INT\nwidth.value=100\n\n[part.selectionbox]\nid=RECT\npos.value=0,1\ncolor.value=0x4000ff00\n",
        );
        assert_eq!(c.id, "NFS5.ListBox");
        assert_eq!(c.parts_order, ["data", "selectionbox"]);
        assert_eq!(c.state_parts("normal"), ["data", "selectionbox"]);
        assert_eq!(c.default_of("width"), Some("100"));
        assert_eq!(c.data().unwrap().attributes, ["accessor", "width"]);
        let sb = c.part("selectionbox").unwrap();
        assert_eq!(sb.kind(), PartKind::Rect);
        assert_eq!(sb.value("pos"), Some("0,1"));
        assert_eq!(sb.value("color"), Some("0x4000ff00"));
    }
}
