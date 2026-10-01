//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// The timeline's five columns, in the order 0x507B4F walks them (§4.122).
    pub fn set_chronicle_columns(&mut self, cols: Vec<(u32, usize)>) {
        self.chronicle_columns = cols;
        for g in self.groups.values_mut() {
            g.set_chronicle_columns(self.chronicle_columns.clone());
        }
    }

    /// The pack and the per-cell entries the timeline's icons come from (§4.123).
    pub fn set_chronicle_icons(&mut self, pack: String, cells: std::collections::BTreeMap<(usize, usize), (usize, usize)>) {
        self.chronicle_icons = (pack, cells);
    }

    /// One caption per timeline column, the `фото`-row entry's own `0x6F` text (§4.173).
    pub fn set_chronicle_captions(&mut self, captions: Vec<Option<String>>) {
        self.chronicle_captions = captions;
    }

    /// `FEChroniclePictureControl`'s four mouse slots — the class `NFS5.ChroniclePicture`
    /// instantiates, vtable **0x5B8C74**:
    ///
    /// ```text
    /// 0x508250  slot 5  [+0x154] = [+0x14C]; [+0x158] = [+0x150]     ; remember the offset
    ///                   [+0x140] = 1; [+0x144] = ev.x; [+0x148] = ev.y
    /// 0x5082A0  slot 8  while [+0x140]:
    ///                     [+0x14C] = [+0x154] - ev.x + [+0x144]
    ///                     [+0x150] = [+0x158] - ev.y + [+0x148]
    /// 0x5082F0  slot 7  the base leave 0x4C7C50 (clears the hover flag), then [+0x140] = 0
    /// 0x508290  slot 9  [+0x140] = 0
    /// ```
    ///
    /// so the offset moves **against** the pointer, the way a document scrolls under a hand, and
    /// the bind 0x508220 starts all five at zero. §4.119.
    ///
    /// **Slot 7 (0x5082F0), COVERAGE §14.15's one honest gap in this class, now ported.** The
    /// window's own move handler (`Window::handleEvent`'s move arm, 0x5289D9, §4.56) recomputes
    /// the hit test on every device move and, whenever the hit widget changes away from the one
    /// that had been hovered, sends it a type-6 **leave** event before the move event itself is
    /// dispatched — regardless of capture, which this class's slot 5 never takes (unlike
    /// `NFS5.DEADZONE`'s press, §4.40, it does not call `setFocused`, so §4.57's capture-on-focus
    /// never applies to a chronicle-picture drag). So as soon as the pointer's box test
    /// (`Control::containsPoint` 0x525A90, both edges inclusive, §4.56) fails for this widget, the
    /// leave event's slot 7 clears `[+0x140]` — and slot 6 ("enter", 0x4C7B90, not overridden by
    /// this class) never sets it back, so the drag stays stopped for the rest of the press even if
    /// the pointer wanders back over the picture while the button is still held. `picture_drag_to`
    /// below checks the same box before applying the pan, for the identical outcome.
    pub fn picture_press(&mut self, index: usize, x: f32, y: f32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.picture_press(j, x, y);
            }
            return;
        }
        // slot 5 copies the pan as the **draw** last left it, and the draw clamps it in place
        // (0x5084E8…0x508516), so a drag starts from the clamped value, never from a raw one
        let (ox, oy) = self.chronicle_pan(index).map_or((0.0, 0.0), |p| (p.0, p.1));
        self.picture_drag = Some((index, x, y, ox, oy));
    }

    /// `FEChroniclePicture::draw`'s pan, clamped the way the draw clamps it, with the size of the
    /// whole picture: `(pan_x, pan_y, picture_w, picture_h)`.
    ///
    /// 0x5084AC takes the widget's box (0x4C8670: the local rect of vtable slot `0x34` plus the
    /// parent's origin — **not** moved by the transition), and 0x5084C7…0x5084E2 take
    /// `picture − box` on each axis as the furthest the pan can go. The stored pan is then
    /// clamped to that (0x5084E4, 0x5084EE) and **afterwards** to 0 (0x508504, 0x50850E), so a
    /// picture no bigger than its box never moves. `None` when no picture is bound.
    pub(crate) fn chronicle_pan(&self, index: usize) -> Option<(f32, f32, f32, f32)> {
        let e = self.layout.elements.get(index)?;
        let &(_, pw, ph) = self.shapes.get(self.logos.get(&index)?)?;
        let (bw, bh) = self.box_of(e, 0);
        let (rx, ry) = self.picture_pan.get(&index).copied().unwrap_or((0.0, 0.0));
        Some((rx.min(pw - bw).max(0.0), ry.min(ph - bh).max(0.0), pw, ph))
    }

    /// Slot 8, 0x5082A0 — the pan follows the pointer while the button is held, unless slot 7,
    /// 0x5082F0, has just ended it: the pointer leaving this widget's own hit box (the box
    /// `hovered_scan` tests it against, both edges inclusive — `Control::containsPoint` 0x525A90,
    /// §4.56) stands in for the window's leave event, and once it fires the drag does not resume
    /// even if the pointer comes back while the button is still held (slot 6, "enter", is not
    /// overridden by this class, so nothing sets `[+0x140]` back to 1 — see `picture_press`'s doc).
    pub fn picture_drag_to(&mut self, x: f32, y: f32) {
        if let Some((i, gx, gy, ox, oy)) = self.picture_drag {
            let inside = self.layout.elements.get(i).is_some_and(|e| {
                let (bx, by) = self.position_of(e);
                let (bw, bh) = self.box_hit(e);
                bw >= 1.0 && bh >= 1.0 && x >= bx && x <= bx + bw && y >= by && y <= by + bh
            });
            if inside {
                self.picture_pan.insert(i, (ox - x + gx, oy - y + gy));
            } else {
                self.picture_drag = None;
            }
        }
        for g in self.groups.values_mut() {
            g.picture_drag_to(x, y);
        }
    }

    /// Slot 9, 0x508290.
    pub fn picture_release(&mut self) {
        self.picture_drag = None;
        for g in self.groups.values_mut() {
            g.picture_release();
        }
    }

    /// Tooling only (`NFS_FE_CLICKS=drag:…`): the flattened index and current pan of whichever
    /// `NFS5.ChroniclePicture` a drag is still active for — `None` once slot 7 (0x5082F0) has
    /// cleared it, which is exactly the observation the diagnostic checks for a drag that left
    /// the widget's box.
    pub fn dragging_picture(&self) -> Option<(usize, f32, f32)> {
        if let Some((i, ..)) = self.picture_drag {
            return self.picture_pan.get(&i).map(|&(px, py)| (i, px, py));
        }
        self.groups.values().find_map(|g| g.dragging_picture())
    }
}
