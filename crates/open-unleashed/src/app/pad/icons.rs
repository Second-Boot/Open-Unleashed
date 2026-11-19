//! **Pad button icons** (PORT-DEVIATION D64): while a standard pad is connected the
//! menus show its button beside the action it triggers. The art is `crate::app::pad::glyphs`; this is the state
//! (which pad, the atlas) and the front end's bottom bar. The race's dialogs draw theirs from the HUD
//! (`Hud::pad_glyph`). No pad = no sprite: the screens are exactly the original's.

use crate::*;
use crate::app::pad::glyphs::{Glyph, PadIcons};
use nfs_game::layout::design_place;
use nfs_input::gamepad::PadKind;

/// The icon's side in the front end's 640 x 480 design rows.
const FE_ICON: f32 = 17.0;
/// The bar's middle row: `bottombar.lay` section 7's black rectangle is 24 rows from row 456, and the captions are centred on it.
const FE_BAR_MID: f32 = 468.0;
/// The gap between a bar button's box and its icon.
const FE_GAP: f32 = 3.0;

impl App {
    /// The kind of pad to show icons for: `NFS_PAD_KIND` (test glue: as if such a pad were connected), else the
    /// standard pad that sent the latest input (the first one before any input).
    pub(crate) fn pad_kind_now(&self) -> Option<PadKind> {
        nfs_game::config::cfg().nfs_pad_kind.as_str().and_then(PadKind::from_name).or_else(|| self.gamepads.pad_kind())
    }

    /// Once a frame: the icons for the current pad, the atlas uploaded the first time one is wanted. A pad
    /// unplugged takes them away the same frame (`pad_kind` is `None` with no standard pad).
    pub(crate) fn pad_icons_refresh(&mut self) {
        let Some(kind) = self.pad_kind_now() else {
            self.pad_icons = None;
            if let Some(h) = &self.hud {
                h.pad_icons.set(None);
            }
            return;
        };
        if self.draws.pad_atlas.is_none()
            && let Some(r) = self.renderer.as_mut()
        {
            let (w, h, levels) = crate::app::pad::glyphs::atlas();
            self.draws.pad_atlas = Some(r.upload_texture_mips(w, h, &levels));
        }
        self.pad_icons = self.draws.pad_atlas.map(|tex| PadIcons { tex, kind });
        if let Some(h) = &self.hud {
            h.pad_icons.set(self.pad_icons);
        }
    }

    /// The front end's bottom bar: the pad's key beside each button that is on show and enabled — Back (B),
    /// Race (Start; only where the screen can start one), Options (Y), exit (View) — D54. Nothing while a
    /// dialog is up (the pad answers the dialog then) or a screen slides (the keys are dropped).
    pub(crate) fn fe_pad_glyphs(&self, aspect: f32) -> Vec<Sprite> {
        let (Some(icons), Some(bar)) = (self.pad_icons, self.fe.layout_bottom.as_ref()) else { return Vec::new() };
        if self.fe.dialog.is_some() || self.fe_input_swallowed() {
            return Vec::new();
        }
        let can_play = self.fe.layout_preview.as_ref().is_some_and(|l| l.can_play());
        let mut out = Vec::new();
        for (name, glyph) in [
            ("ID_BACKBUTTON", Glyph::Face(1)),
            ("ID_PLAYGAMEBUTTON", Glyph::Start),
            ("ID_OPTIONSBUTTON", Glyph::Face(2)),
            ("ID_EXITBUTTON", Glyph::View),
        ] {
            if name == "ID_PLAYGAMEBUTTON" && !can_play {
                continue;
            }
            let Some((x, _, w, _)) = bar.live_widget_rect(name) else { continue };
            // before its button; the back button starts at the screen's left edge, so on a 4:3 window (no pillar
            // box to stand in) its icon goes after the caption instead
            let before = x - FE_GAP - FE_ICON;
            let gx = if before >= 0.0 || aspect > 640.0 / 480.0 { before } else { x + w + FE_GAP };
            let (pos, size) = design_place(aspect, gx, FE_BAR_MID - FE_ICON * 0.5, FE_ICON, FE_ICON);
            out.push(icons.sprite(glyph, pos, size));
        }
        out
    }
}
