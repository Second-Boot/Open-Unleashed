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
//! [`eval`] evaluates one the way the executable does — macro expansion, then the 32-bit integer
//! evaluator 0x52F830 — against a resolver that answers each name with its macro **text**
//! (`crate::expr`, where every rule carries its address).
//!
//! Status: CONFIRMED as a reading of the data (all 72 files parse); the evaluator was read to the
//! instruction on 2026-09-26 (`RE/notes/layouts.md`).

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
        let id = self.id.trim();
        let is = |n: &str| id.eq_ignore_ascii_case(n);
        if is("DATA") {
            PartKind::Data
        } else if is("RECT") {
            PartKind::Rect
        } else if is("SHAPE") {
            PartKind::Shape
        } else if is("LOCALIZEDTEXT") {
            PartKind::LocalizedText
        } else if is("TEXT") {
            PartKind::Text
        } else {
            PartKind::Control
        }
    }

    /// `<key>.value` — the default for that key.
    pub fn value(&self, key: &str) -> Option<&str> {
        crate::lc::with_lower(key, ".value", |k| self.fields.get(k).map(|s| s.as_str()))
    }

    /// `<key>.type` — `INT`, `STRING`, `COLOR`, …
    pub fn ty(&self, key: &str) -> Option<&str> {
        crate::lc::with_lower(key, ".type", |k| self.fields.get(k).map(|s| s.as_str()))
    }

    /// `<key>.flags` — `HIDDEN`, `READONLY` or both.
    pub fn flags(&self, key: &str) -> Option<&str> {
        crate::lc::with_lower(key, ".flags", |k| self.fields.get(k).map(|s| s.as_str()))
    }

    /// The `.lay` key that overrides this attribute, and whether it may be overridden at all.
    ///
    /// The `.ctr` parser 0x52BC90 reads four lines per attribute — `<key>.value`,
    /// `<key>.screenkey`, `<key>.label` and `<key>.flags` — and keeps the **screen key** in
    /// `[attr+0x1C]`, defaulting to the attribute's own name when the file gives none
    /// (0x52BF43…0x52BF53). A separate byte `[attr+0x34]` starts at 1 (0x52BFB4) and is cleared
    /// when the flags contain **`HIDDEN`** (`strstr` at 0x52BFBA…0x52BFEF); 0x52C000 looks the
    /// screen key up in the `.lay` block (0x5240A0) **only when that byte is set**.
    ///
    /// So: a part attribute takes the block's value for its screen key unless it is `HIDDEN`, and
    /// falls back to its own `<key>.value`. `Graphic Tab Button.ctr` is the one class that uses
    /// the indirection — its `selectedbg` part takes its pack from the widget's
    /// `selectedbgSmallPathName` and its position from `highlightsmallpos`.
    pub fn screen_key(&self, key: &str) -> Option<String> {
        if self.flags(key).is_some_and(|f| f.to_ascii_uppercase().contains("HIDDEN")) {
            return None;
        }
        Some(match crate::lc::with_lower(key, ".screenkey", |k| self.fields.get(k)) {
            Some(k) if !k.trim().is_empty() => k.trim().to_string(),
            _ => key.to_string(),
        })
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
        crate::lc::with_lower(key, ".value", |k| self.fields.get(k).map(|s| s.as_str()))
    }

    pub fn part(&self, name: &str) -> Option<&Part> {
        crate::lc::with_lower(name, "", |k| self.parts.get(k))
    }

    /// The `DATA` part — the one that holds the attributes a `.lay` block sets.
    pub fn data(&self) -> Option<&Part> {
        self.parts.values().find(|p| p.kind() == PartKind::Data)
    }

    /// The parts drawn in `state`, in the order that state lists them; the parts the `[control]`
    /// section lists when the state is unknown.
    /// Whether the file carries a `[state.<name>]` section for this state. A name it does not
    /// carry never reaches 0x525580, so the widget keeps the ctor's own full part list.
    pub fn has_state(&self, state: &str) -> bool {
        crate::lc::with_lower(state, "", |k| self.states.contains_key(k))
    }

    pub fn state_parts(&self, state: &str) -> &[String] {
        crate::lc::with_lower(state, "", |k| self.states.get(k)).map(|v| v.as_slice()).unwrap_or(&self.parts_order)
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
            // **Both keys are mandatory.** The registry builder 0x52C430 pulls `id` and
            // `description` out of section 0 (0x52C635 / 0x52C644) and skips the file unless
            // both came back non-null — `test ebp,ebp; je 0x52C718` at 0x52C656 and
            // `test ebx,ebx; je 0x52C718` at 0x52C65E. Of the 77 shipped `*.ctr` / `*.scr`
            // files six register nothing: five are zero bytes (`3DTrack.Ctr`,
            // `ComboBoxSlider.ctr`, `GotoRect.ctr`, `GraphicsComboBoxSlider.ctr`,
            // `TrackLarge2dSpline.ctr`) and `UserNameInputBox.ctr` is a comment that says so.
            // 71 register. The key is the `id=` line, never the file name — `ComboBox.ctr`
            // carries `NFS5.ComboBoxSlider`, which is 97 of the shipped widgets, while the
            // file actually called `ComboBoxSlider.ctr` is empty.
            if !c.id.is_empty() && !c.description.is_empty() {
                lib.controls.insert(c.id.to_lowercase(), c);
            }
        }
        lib
    }

    pub fn get(&self, class: &str) -> Option<&Control> {
        crate::lc::with_lower(class, "", |k| self.controls.get(k))
    }

    pub fn len(&self) -> usize {
        self.controls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.controls.is_empty()
    }
}

pub use crate::expr::{Macro, eval, eval_pair};

#[cfg(test)]
mod tests {
    use super::*;

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
