//! The dialogs' draw — `FEDialog`'s slot 15 **0x512A40** and `FEDialogButton`'s **0x514DF0**
//! (`RE/notes/controls.md` §4.147). The state is `crate::dialog::Dialog`; this is only the
//! painting, over this screen's painters.
//!
//! Every colour of both goes through the fade — `0x4D87F0(c, [+0xDC])` — so the dialog is drawn
//! whole and the alpha of what it pushed is then scaled by `fade / 255`, the same thing to within
//! a rounding of the original's per-colour blend.

use super::{tint, LayoutScreen, DIALOG_BUTTON_FONT, DIALOG_TEXT_FONT};
use crate::dialog::Dialog;
use nfs_formats::locale::Strings;
use nfs_renderer::Sprite;

fn argb(c: u32) -> [f32; 4] {
    tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8])
}

impl LayoutScreen {
    /// The height of a dialog's text block — 0x523250's `[+4]`, which the wrap 0x523600 builds as
    /// `(line height + 2)` a line (0x523912…0x523932), wrapped at **300** (0x512792).
    pub fn dialog_text_height(&self, text: &str) -> f32 {
        let Some(f) = self.fonts.get(DIALOG_TEXT_FONT) else { return 0.0 };
        super::wrap_lines(f, text, 300.0).len() as f32 * (f.line_px() + 2.0)
    }

    /// Slot 15, 0x512A40, and the buttons' 0x514DF0 — `pressed` is 0x5299C0, a button held.
    pub fn dialog_sprites(&self, d: &Dialog, aspect: f32, strings: &Strings, pressed: bool) -> Vec<Sprite> {
        let mut out = Vec::new();
        let th = self.dialog_text_height(&d.text);
        let (x, y, w, h, _) = d.frame(th);
        // 0x512AE5: the body, the gradient 0x4D96B0 over the frame grown by 5 each side —
        // `(x, y, w, h, cUL, cUR, cBL, cBR)`, steel blue across the top, pale yellow the bottom
        let (top, bot) = (argb(0xFF82A2B0), argb(0xE5FED66B));
        self.gouraud(&mut out, aspect, [[x - 5.0, y - 5.0], [x + w + 5.0, y - 5.0], [x - 5.0, y + h + 5.0], [x + w + 5.0, y + h + 5.0]], [top, top, bot, bot]);
        // 0x512AF5: the buttons, each its own draw 0x514DF0
        for (i, b) in d.buttons.iter().enumerate() {
            let (bx, by, bw, bh) = d.button_rect(i, th);
            // the flat fill 0x4D9280: 0x4D87C0 amber when hovered, 0x4D87B0 dark red otherwise
            let fill = if b.hovered { [0xFF, 0xBB, 0x00, 0xFF] } else { [0xBC, 0x00, 0x00, 0xFF] };
            out.push(self.quad(aspect, bx, by, bw, bh, tint(fill)));
            // the outline 0x4D9200, black (0x4D87E0): four one-pixel bars
            let black = tint([0, 0, 0, 0xFF]);
            out.push(self.quad(aspect, bx, by, bw, 1.0, black));
            out.push(self.quad(aspect, bx, by + bh - 1.0, bw, 1.0, black));
            out.push(self.quad(aspect, bx, by, 1.0, bh, black));
            out.push(self.quad(aspect, bx + bw - 1.0, by, 1.0, bh, black));
            // the label, centred at `x + w·0.5`, `y + 2`, white (0x514F46)
            if let Some(f) = self.fonts.get(DIALOG_BUTTON_FONT) {
                let label = strings.get(b.label).unwrap_or_default();
                self.text(&mut out, aspect, f, label, bx, by + 2.0, bw, 1, [1.0, 1.0, 1.0, 1.0]);
            }
            // then the bevel when hovered and held (0x514FA6), else the drop shadow and the glass
            if b.hovered && pressed {
                self.bevel(&mut out, aspect, bx, by, bw, bh, 3.0);
            } else {
                self.drop_shadow(&mut out, aspect, bx, by, bw, bh, 4.0, 4.0);
                self.glass(&mut out, aspect, bx, by, bw, bh, &tint);
            }
        }
        // 0x512B65: the text, black, centred in its 300-wide block at (x+10, y+10)
        if let Some(f) = self.fonts.get(DIALOG_TEXT_FONT) {
            self.wrapped_text(&mut out, aspect, f, &d.text, x + 10.0, y + 10.0, 300.0, 1, [0.0, 0.0, 0.0, 1.0], None);
        }
        // the four bevel bands 0x4D98D0, each `(UL, UR, BL, BR)` with its colour
        let bands: [([[f32; 2]; 4], [u32; 4]); 4] = [
            // 0x512C29, top
            ([[x - 5.0, y - 5.0], [x + w + 5.0, y - 5.0], [x, y], [x + w, y]], [0xCCCFDCE0, 0xB2DCE6E9, 0xCCCFDCE0, 0xB29DBED0]),
            // 0x512CED, left
            ([[x - 5.0, y - 5.0], [x, y], [x - 5.0, y + h + 5.0], [x, y + h]], [0xB29CD1FF, 0x809CD1FF, 0xB2FFFFFF, 0x80FFFFFF]),
            // 0x512DCF, right
            ([[x + w, y], [x + w + 5.0, y - 5.0], [x + w, y + h], [x + w + 5.0, y + h + 5.0]], [0xB2193B6A, 0xE5193B6A, 0xB22A1711, 0xE52A1711]),
            // 0x512EB1, bottom
            ([[x, y + h], [x + w, y + h], [x - 5.0, y + h + 5.0], [x + w + 5.0, y + h + 5.0]], [0xB261492A, 0xB261492A, 0xE561492A, 0xE561492A]),
        ];
        for (p, c) in bands {
            self.gouraud(&mut out, aspect, p, c.map(argb));
        }
        // 0x512F04: the drop shadow under the whole frame
        self.drop_shadow(&mut out, aspect, x - 5.0, y - 5.0, w + 10.0, h + 10.0, 4.0, 4.0);
        let fade = d.fade as f32 / 255.0;
        for s in &mut out {
            s.color[3] *= fade;
        }
        out
    }
}
