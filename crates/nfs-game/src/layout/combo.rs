//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// An open combo's drop-down list — `FeComboBox` slot 27, 0x4C5810, the stretch a list box
    /// jumps over (0x4C58D6, 0x4C6555).
    ///
    /// The rect is the `selectionbox` part's, `[this+0x178]`: `pos=0,data.height` and
    /// `size=data.width,data.selectionboxmaxheight` from `ComboBox.ctr`, its **width** reduced by
    /// 0x4DB030's 16 while the list scrolls (0x4C542D / 0x4C544A through the setter 0x4E2BF0 —
    /// the only geometry the code touches, so the **height stays the template's**).
    ///
    /// * the panel is a gouraud rect, 0x4D96B0 at 0x4C6605, amber `0xffffbb00` across the top and
    ///   `0x4D8850(amber, 0xb282a8b0, t)` across the bottom, both through the fade (0x4D87F0),
    ///   with `t = [this+0x1C4] · 255 / [this+0x1C0]` — the rows' height over the box's, so how
    ///   full the list is (0x4C655C…0x4C658C). Its width takes the 16 back (0x4C65F1);
    /// * then the same rect outlined in black — `0x4D87E0` of the fade through the four-bar
    ///   0x4D9200 (0x4C66A6…0x4C66E0), this one at the plain width;
    /// * then the rows, from the shared loop at 0x4C66E8, the first being the scrollbar's own
    ///   position `[this+0x1BC]->slot10()`. Their colours are §4.42's three, and `ComboBox.ctr`
    ///   defines none of them — so the text is black, which is what the amber panel wants.
    ///
    /// The combo drop-down's own scrollbar — **not** the list box's `NFS5.VerticalSlider`.
    /// `FeComboBox` builds it at 0x4C516A through **0x4DADB0**, a class of its own (vtable
    /// 0x5B6A64) whose six arrows come out of `WeeIcons.fsh` by tag, and **0x4DAF10** lays it out
    /// from `(x, y, h)` in literals:
    ///
    /// ```text
    /// 0x4DAF2A  trough    [+0x10C…0x118] = (x,      y,        16, h)
    /// 0x4DAF33  up arrow  [+0x12C…0x138] = (x,      y,        16, 13)
    /// 0x4DAF91  down      [+0x11C…0x128] = (x,      y + h-13, 16, 13)
    /// 0x4DAF24  knob      [+0xFC…0x108]  = (x + 1,  y + 13,   14, 30)
    /// 0x4DAFC5  and the knob's height is redone: max(16, h - 2·13 - count)
    /// ```
    ///
    /// and **0x4DB530** paints it: the trough filled `0x80FEDD81` through the fade (0x4DB592)
    /// with §4.21's bevel at a thickness of 3 over it (0x4DB5DB), then the knob through
    /// **0x5215C0** with a thickness of 7 (0x4DB622) — whose own body, the one 0x4D96B0 in it
    /// (0x5216DA), is a vertical ramp from **0xFF9CD1FF** to **0xFFFEDD81**, pale blue over pale
    /// cream.
    ///
    /// 0x5215C0's four 0x4D98D0 bands over that body — 0x521748, 0x5217A0, 0x5217C8 and 0x5217FB —
    /// **are drawn**, by `bevel_knob` below, with the palette §4.97 and §4.98 pinned. This comment
    /// used to say they were not, and that "its palette is 0x521250's exactly (§4.77) … putting
    /// §4.77's there would be a substitution": both halves were stale, and the second was also
    /// wrong. 0x5215C0 mixes **seventeen** colours, not eight; its first eight *are* 0x521250's,
    /// but four of them are dead here (0x80000000 at 0x5215FC and 0xB2FFFFFF, 0xCCFFFFFF,
    /// 0xCC000000 at 0x521620…0x521636, each left in `eax` and clobbered by the next call), and
    /// nine more follow. Re-read from the image 2026-09-21 and it agrees with §4.98 exactly. The knob's travel is the
    /// port's own scroll position, the same reading §4.45 makes for the list box.
    pub(super) fn combo_scrollbar(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, h: f32, rows: usize, first: usize, tint: &dyn Fn([u8; 4]) -> [f32; 4]) {
        const W: f32 = 16.0;
        const ARROW_H: f32 = COMBO_ARROW_H;
        out.push(self.quad(aspect, x, y, W, h, tint([0xFE, 0xDD, 0x81, 0x80])));
        self.bevel(out, aspect, FeRect::new(x, y, W, h), 3.0);
        if let Some(a) = self.combo_arrows {
            // `[this+0xEC]` picks the frame: the resting `ARup`/`A2up`, the hovered `ARov`/`A2ov`
            // or the held `ARin`/`A2in` — 0x4DB820 and 0x4DB86B choose between them by comparing
            // it against the region the hit test 0x4DB2F0 wrote. §4.101.
            let frame = |base: usize, region: u32| {
                a[base + match self.combo_scroll {
                    Some((_, r, held)) if r == region => usize::from(held) + 1,
                    _ => 0,
                }]
            };
            for (n, ay) in [(frame(0, 1), y), (frame(3, 2), y + h - ARROW_H)] {
                let Some(&(tex, aw, ah)) = self.shapes.get(&("weeicons.fsh".to_string(), n)) else { continue };
                let (pos, size) = self.place(aspect, x, ay, aw, ah);
                out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
            }
        }
        // the knob: its height is 0x4DAFC5's, and its top 0x4DB220's from the first row
        let (ky, knob_h, _) = combo_knob(y, h, rows, first);
        let n = knob_h.round().max(1.0) as usize;
        for d in 0..n {
            let t = (d as f32 + 0.5) / n as f32;
            let c: [f32; 4] = std::array::from_fn(|j| {
                let (a, b) = (tint([0x9C, 0xD1, 0xFF, 0xFF]), tint([0xFE, 0xDD, 0x81, 0xFF]));
                a[j] + (b[j] - a[j]) * t
            });
            out.push(self.quad(aspect, x + 1.0, ky + d as f32 * knob_h / n as f32, 14.0, (knob_h / n as f32).max(0.5), c));
        }
        // and §4.98's outline **over** the body, the way 0x5215C0 lays its four quads after the
        // one 0x4D96B0, at the thickness 7 that 0x4DB622 passes
        self.bevel_knob(out, aspect, FeRect::new(x + 1.0, ky, 14.0, knob_h), 7.0);
        // Then two three-pixel slivers, one above the knob and one below, through the **clipped**
        // gouraud rect 0x4D94A0 that §4.29's glint sweep also uses — clipped to the trough:
        //
        // ```text
        // 0x4DB6E6  (troughX + 1, knobY - 3,        14, 3)  0xFFFFFFFF -> 0x19FFFFFF
        // 0x4DB783  (troughX + 1, knobY + knobH,    14, 3)  0x19000000 -> 0xCC000000
        // ```
        //
        // — a lit edge over the knob and a shadow under it, the 14.0 and 3.0 being the literals
        // 0x41600000 and 0x40400000 the pushes carry. §4.100.
        for (sy, top, bot) in [
            (ky - 3.0, tint([0xff, 0xff, 0xff, 0xff]), tint([0xff, 0xff, 0xff, 0x19])),
            (ky + knob_h, tint([0, 0, 0, 0x19]), tint([0, 0, 0, 0xcc])),
        ] {
            for d in 0..3 {
                let sy = sy + d as f32;
                // **0x4D94A0's clip box, pinned 2026-09-21.** 0x4DB65D…0x4DB67A builds it from the
                // widget's own four-int rectangles — the ones the hit test 0x4DB2F0 walks — plus
                // the screen origin in `ebx`/`ebp` that every coordinate here carries:
                //
                // ```text
                // left   = trough.x                  ([this+0x10C] + ebp)
                // top    = upArrow.y + upArrow.h     ([this+0x130] + [this+0x138] + ebx)
                // right  = trough.x + trough.w       ([this+0x10C] + [this+0x114] + ebp)
                // bottom = downArrow.y               ([this+0x120] + ebx)
                // ```
                //
                // — the trough's **interior**, between the two arrows, not the widget's box. The
                // port used to clip to the whole box, which differs only when the knob is at an
                // extreme: there a sliver would show over an arrow. §4.100.
                if !combo_sliver_visible(sy, y, h) {
                    continue;
                }
                let t = (d as f32 + 0.5) / 3.0;
                let c: [f32; 4] = std::array::from_fn(|j| top[j] + (bot[j] - top[j]) * t);
                out.push(self.quad(aspect, x + 1.0, sy, 14.0, 1.0, c));
            }
        }
    }

    /// The scrollbar geometry of the open drop-down `i` — `(x, y, height, positions, first)`, the
    /// four numbers `combo_scrollbar` draws from. `None` when the list is short enough not to
    /// scroll.
    ///
    /// `positions` is **not** the row count. `FeComboBox`'s slot 1 (0x4C5460) is the same shared
    /// tick §4.45/§4.59 read for the list box: it walks up from the last row while the heights
    /// fit and hands the scrollbar model `rowsWalkedUp + 2` (0x4C550A) — one first-row position
    /// past the last full page — and that `+2` count is what `slot9`/`slot24` (§4.59) answer back
    /// as the knob's travel divisor, for a combo exactly as for a list box. Until this fix the
    /// combo's own geometry passed the plain row count instead, which is why the combo scrollbar's
    /// knob height, travel and hit-test disagreed with the list box's for the same shape of list.
    pub(super) fn combo_scroll_geom(&self, i: usize) -> Option<(f32, f32, f32, usize, usize)> {
        let e = self.layout.elements.get(i)?;
        let (rows, _) = self.lists.get(&i)?;
        if rows.is_empty() {
            return None;
        }
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(160.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        if full < max_h {
            return None;
        }
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let height = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        let sbw = self.attr_num(e, "selectionboxwidth").unwrap_or(0.0) as f32;
        let grow = (sbw - width).max(0.0);
        let left = self.attr_num(e, "selectionboxslidedirection").unwrap_or(0.0) as i32 == -1;
        let (x, y) = self.position_of(e);
        let px = if left { x - grow } else { x };
        let positions = self.last_first_row(i, rows) + 2;
        Some((px + width + grow - LIST_SCROLLBAR_W, y + height, max_h, positions, self.scroll.get(&i).copied().unwrap_or(0)))
    }

    /// `FeListScrollBar::hitTest` 0x4DB2F0 — which of the bar's four rectangles the pointer is in.
    /// It tries them in a fixed order, each with the same `x <= px <= x + w && y <= py <= y + h`
    /// pair of `fcompp` runs, and writes the answer to `[this+0xF0]`:
    ///
    /// ```text
    /// 0x4DB2F7  [ecx+0x12C] -> 1   ; the up arrow      (0x4DB371)
    /// 0x4DB388  [ecx+0x11C] -> 2   ; the down arrow    (0x4DB3F5)
    /// 0x4DB40C  [ecx+0x0FC] -> 3   ; the knob          (0x4DB479)
    /// 0x4DB490  [ecx+0x10C] -> 4   ; the trough        (0x4DB4FD)
    /// 0x4DB510              -> 0   ; none of them
    /// ```
    ///
    /// The knob is tested **before** the trough, so a press on the knob never counts as the track
    /// — which is what lets arm 3 (drag) and arm 4 (page) of the press handler 0x4DB040 differ.
    /// The four rectangles are the ones the constructor 0x4DAF10 laid out (§4.90).
    ///
    /// A drop-down open inside an embedded screen (a tab panel) is held by that screen alone —
    /// `set_open_combo` keeps only indices of this screen's own elements in `open_combo` — so with
    /// none open here, ask each embedded screen at the panel's own offset, the same routing
    /// `combo_row_at` uses (2026-09-29's nested-panel fix). (history: `game-history.md` §23.1)
    pub fn combo_scroll_at(&self, px: f32, py: f32) -> Option<(usize, u32)> {
        if let Some(i) = self.open_combo {
            let (x, y, h, rows, first) = self.combo_scroll_geom(i)?;
            return match scrollbar_region(x, y, h, rows, first, px, py) {
                0 => None,
                n => Some((i, n)),
            };
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if !self.closed.contains(&k)
                && let Some(e) = self.layout.elements.get(k)
            {
                let (gx, gy) = self.position_of(e);
                if let Some((j, r)) = g.combo_scroll_at(px - gx, py - gy) {
                    return Some((base + j, r));
                }
            }
            base += g.span();
        }
        None
    }

    /// A press on the open combo's scrollbar **knob** (region 3) or **track** (region 4) —
    /// `FeListScrollBar::onPress` 0x4DB040's other two arms (§4.46, §4.101). Returns
    /// `(index, grabY, grabPosition)`, the drag anchor 0x4DB9B0 steps from on every move —
    /// `[this+0xE4]` and `[this+0xE8]`:
    ///
    /// ```text
    /// arm 3  0x4DB0B0  [+0xE0], [+0xE4] = ev.x, ev.y          ; the pointer
    ///        0x4DB0C1  [+0xE8] = model->slot10()              ; the position it is on
    /// arm 4  0x4DB0D5  [+0xE0], [+0xE4] = [+0xFC], [+0x100]   ; the knob's own top-left
    ///        0x4DB0F0  [+0xE8] = model->slot10()
    ///        0x4DB0F6  [+0x100] = ev.y, then the hit test again -> region 3
    /// ```
    ///
    /// Region 3 anchors to the **pointer's own** y; region 4 to the **knob's current top**. Arm 4
    /// then asks for a synthetic move (`bl = 1` at 0x4DB101 → 0x4D6140 at 0x4DB119, a type-7
    /// `MouseMoveEvent` at the current position, controls.md §4.40), which reaches the drag slot
    /// 0x4DB9B0 at once — so a press on the trough moves the list by `(pointer − knobTop)`
    /// straight away; `App::fe_left_button` runs that first step right after the grab.
    pub fn combo_scroll_grab(&self, index: usize, region: u32, py: f32) -> Option<(usize, f32, usize)> {
        if let Some((k, j)) = self.route(index) {
            let (_, gy) = self.position_of(self.layout.elements.get(k)?);
            let (_, y, at) = self.groups.get(&k)?.combo_scroll_grab(j, region, py - gy)?;
            return Some((index, y + gy, at));
        }
        let (_, y, h, positions, first) = self.combo_scroll_geom(index)?;
        let grab_y = if region == 4 { combo_knob(y, h, positions, first).0 } else { py };
        Some((index, grab_y, first))
    }

    /// The open combo scrollbar's knob travel (in whole design pixels) and position count — the
    /// numbers `combo_knob` (0x4DB220) places the knob with, which the drag 0x4DB9B0 divides by:
    /// `model->slot11([+0xE8] + (y − [+0xE4]) · count / travel)` (`combo_drag_pos`).
    pub fn combo_scroll_track(&self, index: usize) -> Option<(i32, usize)> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.combo_scroll_track(j);
        }
        let (_, y, h, positions, first) = self.combo_scroll_geom(index)?;
        Some((combo_knob(y, h, positions, first).2, positions))
    }

    /// Put the open combo's first-drawn row at an absolute position, clamped — what the
    /// scrollbar's knob drag does (0x4D0F20's inverse of the placement, §4.46), mirroring
    /// `set_list_scroll`.
    pub fn set_combo_scroll_pos(&mut self, index: usize, at: i32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_combo_scroll_pos(j, at);
            }
            return;
        }
        let Some((rows, _)) = self.lists.get(&index) else { return };
        let positions = self.last_first_row(index, rows) as i32 + 2;
        self.scroll.insert(index, at.clamp(0, positions - 1) as usize);
    }

    /// Which combo has its drop-down open, if any.
    pub fn open_combo(&self) -> Option<usize> {
        self.open_combo
    }

    /// 0x4C39F0, `[0x65B710] = 1`: the colour editor's latch, raised by the combo commit when the
    /// chosen row is the custom one (0x4C6D6C). The box keeps focus — 0x4C6DD9 asks the class's
    /// slot 28 (0x4C46F0, "not the custom row") and skips the unfocus 0x4C7DA0 when it answers 0.
    pub fn raise_picker_latch(&mut self) {
        self.picker_latch = true;
    }

    /// The open drop-down's row under the pointer — `[this+0x1A8]`, which the commit at 0x4C6CF7
    /// reads once the release lands inside the selection box (0x4E2970 on `[this+0x178]`). The
    /// walk is the move handler's, 0x4C6F60: from the scrollbar's position `[+0x1BC]->slot10()`
    /// (0x4C6FC7, the drop-down's first drawn row), each row its accessor height (slot 26,
    /// 0x4C6FF6), the first whose running bottom is past the pointer (0x4C7013, signed `jge`) —
    /// an **absolute** row, which the commit hands to the accessor's slot 11 as it is (0x4C6D82).
    /// `(combo, row)`.
    pub fn combo_row_at(&self, px: f32, py: f32) -> Option<(usize, usize)> {
        // A drop-down open inside an embedded screen (a tab panel — `Options` → `graphics.lay` →
        // `graphicsettings.lay`'s six combos) is held by that screen alone: `set_open_combo`
        // keeps only indices of this screen's own elements in `open_combo`. So with none open
        // here, ask each embedded screen at the panel's own offset and hand its answer back in
        // the flattened index space. (history: `game-history.md` §23.2)
        let Some(i) = self.open_combo else {
            let mut keys: Vec<usize> = self.groups.keys().copied().collect();
            keys.sort_unstable();
            let mut base = self.layout.elements.len();
            for k in keys {
                let g = &self.groups[&k];
                if !self.closed.contains(&k)
                    && let Some(e) = self.layout.elements.get(k)
                {
                    let (gx, gy) = self.position_of(e);
                    if let Some((j, r)) = g.combo_row_at(px - gx, py - gy) {
                        return Some((base + j, r));
                    }
                }
                base += g.span();
            }
            return None;
        };
        if let Some((k, j)) = self.route(i) {
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            return self.groups.get(&k)?.combo_row_at(px - gx, py - gy).filter(|(g, _)| *g == j).map(|(_, r)| (i, r));
        }
        let e = self.layout.elements.get(i)?;
        let (x, y) = self.position_of(e);
        let (rx, ry, rw, rh) = self.drop_down_rect(e, i, x, y)?;
        if !(px >= rx && px < rx + rw && py >= ry && py < ry + rh) {
            return None;
        }
        let (rows, _) = self.lists.get(&i)?;
        let first = self.scroll.get(&i).copied().unwrap_or(0);
        let mut acc = ry;
        rows.iter().enumerate().skip(first).find(|(_, r)| {
            acc += r.height();
            py < acc
        }).map(|(n, _)| (i, n))
    }

    /// Whether a release at `(px, py)` on the **closed** combo `index` lands in its caption
    /// strip — `FeComboBox` slot 9, 0x4C6B7E…0x4C6BB8: inside `backrect` (0x4E2970 on
    /// `[+0x17C]`, `left ≤ x < right`, `top ≤ y < bottom`) and `x < backrect.left + [+0x1F0]`
    /// (0x4E2B20, signed `jge`), [`Self::combo_strip`]. Such a release does not open the list.
    pub fn in_combo_strip(&self, index: usize, px: f32, py: f32) -> bool {
        if let Some((k, j)) = self.route(index) {
            let (Some(g), Some(ek)) = (self.groups.get(&k), self.layout.elements.get(k)) else { return false };
            let (gx, gy) = self.position_of(ek);
            return g.in_combo_strip(j, px - gx, py - gy);
        }
        let Some(e) = self.layout.elements.get(index) else { return false };
        let (x, y) = self.position_of(e);
        let w = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let h = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        px >= x && px < x + w && py >= y && py < y + h && px < x + self.combo_strip(e)
    }

    /// The drop-down's panel, the `selectionbox` rect `[+0x178]`: `(x, y, rows' width, h)` — the
    /// same arithmetic `drop_down` draws with. `h` is **the rows' height, clamped to
    /// `data.selectionboxmaxheight`**: `FeComboBox`'s slot 1 0x4C5460 ends each tick with
    /// 0x4E2C20(selectionbox, h) at 0x4C57D5 — 0x4E2C20 replaces the size pair's **height** — `h`
    /// the 16-tick ramp `min((now − [+0x1D4])·[+0x1C4]/16, [+0x1C4])` (0x4C5626…0x4C564D) of the
    /// clamped rows' height `[+0x1C4]` (0x4C53F3), and 0 while the combo is not focused
    /// (0x4C53C4 zeroes `[+0x1C4]`). The port takes the settled end.
    pub(super) fn drop_down_rect(&self, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32) -> Option<(f32, f32, f32, f32)> {
        let (rows, _) = self.lists.get(&i)?;
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(160.0) as f32;
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let height = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        let scrolls = full >= max_h;
        let sbw = self.attr_num(e, "selectionboxwidth").unwrap_or(0.0) as f32;
        let grow = (sbw - width).max(0.0);
        let left = self.attr_num(e, "selectionboxslidedirection").unwrap_or(0.0) as i32 == -1;
        let (px, py) = (if left { x - grow } else { x }, y + height);
        let width = width + grow;
        let rect_w = if scrolls { width - LIST_SCROLLBAR_W } else { width };
        Some((px, py, rect_w, if scrolls { max_h } else { full }))
    }

    /// Remember where the pointer is on the open drop-down's scrollbar, and whether it is held, so
    /// the arrows can show `ARov`/`A2ov` and `ARin`/`A2in` (§4.101).
    pub fn set_combo_scroll(&mut self, at: Option<(usize, u32, bool)>) {
        self.combo_scroll = at;
    }
}
