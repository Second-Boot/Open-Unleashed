//! The front end's code-built pop-ups — `FEDialog` (vtable 0x5BA060) and its kinds
//! (`RE/notes/controls.md` §4.147). The state machine lives here; the drawing is
//! `LayoutScreen::dialog_sprites`, which has the painters.
//!
//! * **Geometry** (0x512630, 0x512750, 0x513DA0): `x = 160`, `w = 320`, the text wrapped at 300 in
//!   `Univers10b.ffn`; `th = text height + 20`; `h = th + 21` for a one-button dialog (0x51387C)
//!   and `th + 42` for a two-button one (0x513E5D, 0x513E66); `y = 240 − h/2`.
//! * **Buttons** (`FEDialogButton`, 0x513B0C / 0x513B9D): at `(x + 120, y + th)` and `(x + 120,
//!   y + th + 21)`, 80 × 18; their input is slot 8 0x515060 (hover, sound 6 on entering) and
//!   slot 9 0x515130 (a release while hovered: sound 5, clicked).
//! * **Fade** (0x512980): eight alpha units a tick up to 0xFF while wanted up, down to 0 after.
//! * **Answers**: the two-button tick 0x514050 — the first button is "yes", the second "no",
//!   either dismisses; the one-button tick 0x5138D0 — the button dismisses, "yes".

/// What kind of `FEDialog` it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    /// `FEDialogOneItemSelfDeleting` / `…NonSelfDeleting` — one button, 702 "ok" by default
    One,
    /// `FEDialogTwoItem` — two buttons, 702 "ok" / 703 "cancel" by default
    Two,
}

/// One button: its `FEStrings` id, and the hover and clicked bytes `[+0xEC]`, `[+0xF4]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DialogButton {
    pub label: u32,
    pub hovered: bool,
    clicked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    pub kind: DialogKind,
    /// the text, already resolved from its `FEStrings` id
    pub text: String,
    pub buttons: Vec<DialogButton>,
    /// `[+0xD8]` — wanted up
    pub up: bool,
    /// `[+0xDC]` — the fade, 0…0xFF
    pub fade: u32,
    /// `[+0xF0]` — the tick the current fade started
    stamp: u32,
    /// `[+0xF8]` / `[+0xF9]` — answered, and the answer
    answered: bool,
    yes: bool,
}

impl Dialog {
    /// A dialog with its text and button labels (`0` takes the kind's default, as 0x513B14 /
    /// 0x513BA5 do: 702 "ok", 703 "cancel").
    pub fn new(kind: DialogKind, text: String, labels: [u32; 2]) -> Self {
        let pick = |l: u32, d: u32| if l == 0 { d } else { l };
        let buttons = match kind {
            DialogKind::One => vec![DialogButton { label: pick(labels[0], 702), hovered: false, clicked: false }],
            DialogKind::Two => vec![
                DialogButton { label: pick(labels[0], 702), hovered: false, clicked: false },
                DialogButton { label: pick(labels[1], 703), hovered: false, clicked: false },
            ],
        };
        Self { kind, text, buttons, up: false, fade: 0, stamp: 0, answered: false, yes: false }
    }

    /// Slot 13, 0x5128A0: up, the fade restarting now, both latches clear.
    pub fn raise(&mut self, now: u32) {
        self.up = true;
        self.stamp = now;
        self.answered = false;
        self.yes = false;
    }

    /// Slot 14, 0x512950: down, the fade restarting now.
    pub fn dismiss(&mut self, now: u32) {
        self.up = false;
        self.stamp = now;
    }

    /// Slot 16, 0x5124C0 — gone: not wanted and fully faded.
    pub fn gone(&self) -> bool {
        !self.up && self.fade == 0
    }

    /// Slot 18, 0x512500 — answered, read and cleared; and slot 19's answer with it.
    pub fn take_answer(&mut self) -> Option<bool> {
        if !self.answered {
            return None;
        }
        self.answered = false;
        Some(self.yes)
    }

    /// The tick: the fade (0x512980) and then the kind's own (0x514050 / 0x5138D0).
    pub fn tick(&mut self, now: u32) {
        let e = now.wrapping_sub(self.stamp) as i64;
        if self.up {
            if self.fade < 0xFF {
                self.fade = (e * 8).clamp(0, 0xFF) as u32;
            }
        } else if self.fade > 0 {
            self.fade = (0xFF - e * 8).clamp(0, 0xFF) as u32;
        }
        let first = self.buttons.first().is_some_and(|b| b.clicked);
        let second = self.buttons.get(1).is_some_and(|b| b.clicked);
        if first {
            self.buttons[0].clicked = false;
            self.answered = true;
            self.yes = true;
            self.dismiss(now);
        } else if second {
            self.buttons[1].clicked = false;
            self.answered = true;
            self.yes = false;
            self.dismiss(now);
        }
    }

    /// The dialog's frame `(x, y, w, h)` and `th`, for a text block `text_h` tall.
    pub fn frame(&self, text_h: f32) -> (f32, f32, f32, f32, f32) {
        let th = (text_h + 20.0).trunc();
        let h = match self.kind {
            DialogKind::One => th + 21.0,
            DialogKind::Two => th + 42.0,
        };
        let y = (240 - (h as i32) / 2) as f32;
        (160.0, y, 320.0, h, th)
    }

    /// Button `i`'s rectangle `(x, y, 80, 18)`.
    pub fn button_rect(&self, i: usize, text_h: f32) -> (f32, f32, f32, f32) {
        let (x, y, _, _, th) = self.frame(text_h);
        (x + 120.0, y + th + 21.0 * i as f32, 80.0, 18.0)
    }

    /// Slot 8 on each button, 0x515060: the hover byte from the pointer; returns the sounds to
    /// play (6 on entering).
    pub fn pointer(&mut self, px: f32, py: f32, text_h: f32) -> Vec<u8> {
        let mut out = Vec::new();
        for i in 0..self.buttons.len() {
            let (x, y, w, h) = self.button_rect(i, text_h);
            let inside = px >= x && px <= x + w && py >= y && py <= y + h;
            if inside && !self.buttons[i].hovered {
                out.push(6);
            }
            self.buttons[i].hovered = inside;
        }
        out
    }

    /// Slot 9 on each button, 0x515130: a release over a hovered button clicks it with sound 5.
    /// Returns whether a button took the release (0x512F10 stops at the first that does).
    pub fn release(&mut self) -> Option<u8> {
        for b in &mut self.buttons {
            if b.hovered {
                b.clicked = true;
                return Some(5);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_button_dialog_fades_answers_and_goes() {
        let mut d = Dialog::new(DialogKind::Two, "Are you sure?".into(), [700, 701]);
        d.raise(100);
        d.tick(100 + 16);
        assert_eq!(d.fade, 128);
        d.tick(100 + 40);
        assert_eq!(d.fade, 0xFF);
        // geometry: one 12 px line of text -> th 32, h 74, y 203
        let (x, y, w, h, th) = d.frame(12.0);
        assert_eq!((x, y, w, h, th), (160.0, 203.0, 320.0, 74.0, 32.0));
        assert_eq!(d.button_rect(1, 12.0), (280.0, 256.0, 80.0, 18.0));
        assert_eq!(d.pointer(300.0, 260.0, 12.0), vec![6]);
        assert_eq!(d.release(), Some(5));
        d.tick(200);
        assert_eq!(d.take_answer(), Some(false));
        assert_eq!(d.take_answer(), None);
        d.tick(200 + 32);
        assert!(d.gone());
    }
}
