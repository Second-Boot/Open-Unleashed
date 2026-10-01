//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// The `NFS5.ListBox` widgets on this screen, with the accessor each one lists.
    pub fn list_boxes(&self) -> Vec<ListBoxWidget> {
        let mut out: Vec<ListBoxWidget> = self
            .layout
            .elements
            .iter()
            .enumerate()
            // `NFS5.CHATDISPLAYBOX` is not one: its rows are the chat's, and its accessor only
            // names the players (see `chat_boxes`). *(It was bound here until 2026-09-25.)*
            .filter(|(_, e)| e.class == "NFS5.ListBox")
            .map(|(index, e)| ListBoxWidget { index, accessor: self.attr(e, "accessor").map(str::to_string) })
            .collect();
        // …and the ones inside the panels this screen holds, in the flattened index space, as
        // `combo_sliders` does — `Audio.lay`'s CD player lists live in its playlist panel
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            for w in self.groups[&k].list_boxes() {
                out.push(ListBoxWidget { index: base + w.index, accessor: w.accessor });
            }
            base += self.groups[&k].span();
        }
        out
    }

    /// The `NFS5.CHATDISPLAYBOX` widgets on this screen (top level; the three shipped ones are).
    ///
    /// Their rows are **not** their accessor's. The bind 0x4C2B60 keeps two accessors: `[+0x154]`
    /// is `data.accessor` — `Comms.PlayerList` on all three shipped boxes — and is only asked for
    /// **names** (0x4C2530, 0x4C259A: 0x4BBB60 finds a player's row, slot 12 its text); `[+0x150]`
    /// is a range accessor of the box's own, `"%s.SliderBar"` (0x4C2BEF, 0x4BA310), which the
    /// scrollbar moves. The rows the draw 0x4C2F50 walks are the vector `[0x65B6A8]` of chat
    /// lines (0x4C2FD1), filled by 0x4C24F0 as lines arrive. *(Corrected 2026-09-25: §4.131 read
    /// the rows as the accessor's, and the box showed the player list.)*
    pub fn chat_boxes(&self) -> Vec<usize> {
        self.layout.elements.iter().enumerate().filter(|(_, e)| e.class == "NFS5.CHATDISPLAYBOX").map(|(i, _)| i).collect()
    }

    /// Fill a chat box with its lines — what **0x4C24F0** does with each one after formatting it:
    ///
    /// ```text
    /// 0x4C2600  at_end = slider.index == slider.count - 1
    /// 0x4C2625  wrap the line in "Univers8b" to [0x5D7DE0] - 8   ; 0x523080, the box's width
    ///           (0x4C29F7) less eight
    /// 0x4C267D  push every wrapped row onto [0x65B6A8]
    /// 0x4C2711  while there are more than 100 rows, drop the first
    /// 0x4C2764  slider range = 0 .. max(0, rows - fit)          ; fit = [+0x148], §4.131
    /// 0x4C27A2  at_end -> slider.index = the new last
    /// ```
    ///
    /// This port has no scrolling of its own for the box, so the slider is always at its end and
    /// the first row drawn is `max(0, rows − fit)`; its count, which the scrollbar's knob is laid
    /// out by, is that plus one.
    pub fn set_chat_lines(&mut self, index: usize, lines: &[String]) {
        let Some(e) = self.layout.elements.get(index) else { return };
        let (w, _) = self.box_of(e, 0);
        let spacing = self.attr_num(e, "spacing").unwrap_or(14.0) as f32;
        let fit = if spacing > 0.0 { (self.attr_num(e, "height").unwrap_or(0.0) as f32 / spacing) as usize } else { 0 };
        let Some(f) = self.fonts.get("univers8b.ffn") else { return };
        let mut rows: Vec<ListRow> = lines.iter().flat_map(|l| wrap_lines(f, l, w - 8.0)).map(ListRow::Text).collect();
        let over = rows.len().saturating_sub(100);
        rows.drain(..over);
        let first = rows.len().saturating_sub(fit);
        if rows.is_empty() {
            self.lists.remove(&index);
        } else {
            self.lists.insert(index, (rows, Some(first)));
        }
    }

    /// The combo sliders and colour boxes, which take the same row binding as a list box: their
    /// drop-down is drawn by the very same code (`FeComboBox` slot 27, 0x4C5810), reached from
    /// slot **17** rather than slot 15 so that it lands over the widgets drawn after it.
    pub fn combo_sliders(&self) -> Vec<ListBoxWidget> {
        let mut out: Vec<ListBoxWidget> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.ComboBoxSlider" || e.class == "NFS5.ColorBox")
            .map(|(index, e)| ListBoxWidget { index, accessor: self.attr(e, "accessor").map(str::to_string) })
            .collect();
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            for w in self.groups[&k].combo_sliders() {
                out.push(ListBoxWidget { index: base + w.index, accessor: w.accessor });
            }
            base += self.groups[&k].span();
        }
        out
    }

    /// Open one combo's list, or none. The index is the flattened one `combo_sliders` hands out.
    ///
    /// `FeComboBox::refreshRows` **0x4C53A0** is the rule: `[this+0x1C4]`, the height of the rows
    /// the drop-down draws, is the accessor's own total **only when the widget holds focus or is a
    /// list box** (0x4C53B1 asks 0x4C7DD0, 0x4C53BA asks the constructor's `[this+0x1DD]`), and
    /// **0** otherwise — so a combo that is not focused draws no list at all.
    pub fn set_open_combo(&mut self, index: Option<usize>) {
        if crate::config::cfg().nfs_fe_open.var().is_ok() {
            return;
        }
        let open = index.filter(|i| *i < self.layout.elements.len());
        if open != self.open_combo {
            self.picker_latch = false;
        }
        self.open_combo = open;
        let routed = index.and_then(|i| self.route(i));
        let keys: Vec<usize> = self.groups.keys().copied().collect();
        for k in keys {
            let inner = routed.filter(|(g, _)| *g == k).map(|(_, j)| j);
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_open_combo(inner);
            }
        }
    }

    /// Fill a list box with the rows its accessor would hand out, and say which row the accessor
    /// has selected (`current` — the row the original paints with `highlightcolorselected`).
    pub fn set_list_rows(&mut self, index: usize, rows: Vec<ListRow>, current: Option<usize>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_list_rows(j, rows, current);
            }
            return;
        }
        // `FEColourAccessor`'s `[+0x30]`, which the colour box's bind 0x4C3BF0 sets from
        // `data.allowcustom` through 0x4B9590: clear, the list is the car's colours alone —
        // no custom row and no saved colours (0x4B9262, 0x4B9374). §4.171.
        let mut rows = rows;
        if self.layout.elements.get(index).is_some_and(|e| e.class == "NFS5.ColorBox" && self.attr_num(e, "allowcustom").unwrap_or(0.0) == 0.0)
            && let Some(n) = rows.iter().position(|r| matches!(r, ListRow::CustomColour(_)))
        {
            rows.truncate(n);
        }
        if rows.is_empty() {
            self.lists.remove(&index);
            self.scroll.remove(&index);
        } else {
            self.scroll.entry(index).and_modify(|f| *f = (*f).min(rows.len().saturating_sub(1))).or_insert(0);
            self.lists.insert(index, (rows, current));
        }
    }

    /// A colour box's current colour — `FEColourAccessor`'s `[+0x2C]`, which the closed box reads
    /// through its slot 29 0x4C4BF0 → 0x4B94C0 and paints as a swatch (§4.171.4). `None` clears it.
    pub fn set_swatch(&mut self, index: usize, colour: Option<[u8; 4]>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_swatch(j, colour);
            }
            return;
        }
        match colour {
            Some(c) => self.swatches.insert(index, c),
            None => self.swatches.remove(&index),
        };
    }

    /// Whether the colour box bound to `accessor` offers the custom row and the saved colours —
    /// `FEColourAccessor`'s `[+0x30]`, which the box's bind 0x4C3BF0 sets from `data.allowcustom`
    /// through 0x4B9590. With no box on the screen the accessor keeps the constructor's **1**
    /// (0x4B91F0).
    pub fn colour_allows_custom(&self, accessor: &str) -> bool {
        self.layout
            .elements
            .iter()
            .find(|e| e.class == "NFS5.ColorBox" && self.attr(e, "accessor").is_some_and(|a| a.eq_ignore_ascii_case(accessor)))
            .is_none_or(|e| self.attr_num(e, "allowcustom").unwrap_or(0.0) != 0.0)
    }

    /// Whether the widget has an **integer** attribute `data.<key>` — what 0x5258C0 (the
    /// attribute lookup by `group.name`: 0x525620 the group, 0x52A370 / 0x52A360 the attribute,
    /// null when either is missing) followed by the cast 0x5A134B to `FERT::IntAttribute`
    /// (0x5D7868) answers. The attributes a widget carries are its template's `DATA` part, so
    /// the test is the `.ctr`'s `<key>.type=INT`, whatever value it or the `.lay` holds.
    /// A key the `.lay` sets that the template does not declare is taken as absent — UNREACHABLE
    /// for the one key ever asked (2026-09-30): the only caller asks `disablesound`, and
    /// all 18 `disablesound=` lines of the 98 shipped `.lay` files (15 layouts; 3 378 elements
    /// scanned) sit on `NFS5.ListBox` elements, whose `ListBox.ctr` declares `disablesound.type=INT`
    /// (the only `.ctr` of 75 that names it). No `.lay` sets it on a class that does not declare it.
    pub fn has_int_attr(&self, index: usize, key: &str) -> bool {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k).is_some_and(|g| g.has_int_attr(j, key));
        }
        self.layout
            .elements
            .get(index)
            .and_then(|e| self.controls.get(&e.class))
            .and_then(|c| c.data())
            .and_then(|d| d.ty(key))
            .is_some_and(|t| t.eq_ignore_ascii_case("INT"))
    }

    /// Move a list's first drawn row. The original keeps that position in a scrollbar object of
    /// its own — `[this+0x1BC]`, whose slot 10 the row loop asks for at 0x4C66F8 — and the
    /// scrollbar widget `[this+0x1B8]` (0x4C5175) moves it. This port keeps the position itself
    /// and moves it by the arrow the player clicks.
    pub fn scroll_list(&mut self, index: usize, delta: i32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.scroll_list(j, delta);
            }
            return;
        }
        let Some((rows, _)) = self.lists.get(&index) else { return };
        let last = self.last_first_row(index, rows);
        let at = self.scroll.entry(index).or_insert(0);
        *at = (*at as i32 + delta).clamp(0, last as i32 + 1) as usize;
    }

    /// The largest first row that still fills the box — walking up from the last row while the
    /// heights fit, which is the shape of `FeComboBox`'s own loop at 0x4C54EC…0x4C5508.
    pub(super) fn last_first_row(&self, index: usize, rows: &[ListRow]) -> usize {
        let Some(e) = self.layout.elements.get(index) else { return 0 };
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let mut acc = 0.0;
        let mut n = rows.len();
        while n > 0 {
            acc += rows[n - 1].height();
            if acc > max_h {
                break;
            }
            n -= 1;
        }
        n
    }

    /// A list's scrollbar, in design coordinates, when it has one.
    ///
    /// The regions are the ones `0x4D0CC0` hit-tests: **1** the decrement button `[+0xE0]`, **2**
    /// the increment button `[+0xE4]`, **3** the knob `[+0xDC]`, and 4 / 5 the track either side
    /// of it. `VerticalSlider.ctr` places them — `decrement_button` at `0,7` with `weeicons.fsh`
    /// **13**, `increment_button` at the bottom with **14**, the knob
    /// `decrement_button.size.width × 15`.
    pub fn list_scrollbar(&self, index: usize) -> Option<ListScrollbar> {
        if let Some((k, j)) = self.route(index) {
            let g = self.groups.get(&k)?;
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            let mut b = g.list_scrollbar(j)?;
            for r in [&mut b.up, &mut b.down, &mut b.knob] {
                r.0 += gx;
                r.1 += gy;
            }
            b.track_top += gy;
            return Some(b);
        }
        let e = self.layout.elements.get(index)?;
        let (rows, _) = self.lists.get(&index)?;
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        if full < max_h {
            return None;
        }
        let (x, y) = self.position_of(e);
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
            Some(v) => self.ev_pair(e, v, 0).map(|(a, b)| (x + a as f32, y + b as f32)).unwrap_or((x, y)),
            None => (x, y),
        };
        let sx = px + width - LIST_SCROLLBAR_W;
        let art = |n: usize| self.shapes.get(&("weeicons.fsh".to_string(), n)).copied();
        let (uw, uh) = art(13).map(|(_, w, h)| (w, h)).unwrap_or((LIST_SCROLLBAR_W, 8.0));
        let (dw, dh) = art(14).map(|(_, w, h)| (w, h)).unwrap_or((LIST_SCROLLBAR_W, 8.0));
        let base = py + uh;
        let span = (max_h - LIST_KNOB_H - uh).max(0.0);
        // `+2`, not `+1`: the list's tick hands the scrollbar `rowsWalkedUp + 2` (0x4C550A) and
        // the model keeps that as its **count** (`[model+0x14]`, slot 24 at 0x4BC310), so there is
        // one position past the last full page — the list can be scrolled one row further and show
        // a blank line. §4.59.
        let positions = self.last_first_row(index, rows) + 2;
        let at = self.scroll.get(&index).copied().unwrap_or(0);
        let dy = if positions > 1 { at as f32 * span / (positions - 1) as f32 } else { 0.0 };
        // A press on the **track** (regions 4 and 5, 0x4D0995) leaves the model alone and moves the
        // knob's own rect so its top sits at the pointer (0x4D0A0A / 0x4D0A18 → 0x5276E0), then
        // re-runs the hit test so the region becomes the knob. It lasts until the first move, which
        // puts the model somewhere and re-places the knob from it (slot 11).
        let ky = match self.knob_at {
            Some((i, y)) if i == index => y,
            _ => base + dy,
        };
        Some(ListScrollbar {
            up: (sx, py + 7.0, uw, uh),
            down: (sx, py + max_h - dh, dw, dh),
            knob: (sx, ky, uw, LIST_KNOB_H),
            track_top: base,
            span,
            positions,
            at,
        })
    }

    /// Put a list's first row at an absolute position, clamped — what the knob's drag does.
    pub fn set_list_scroll(&mut self, index: usize, at: i32) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_list_scroll(j, at);
            }
            return;
        }
        let Some((rows, _)) = self.lists.get(&index) else { return };
        let last = self.last_first_row(index, rows) as i32;
        self.scroll.insert(index, at.clamp(0, last + 1) as usize);
        // the model moved, so slot 11 re-places the knob from it and the parked position is gone
        self.knob_at = None;
    }

    /// Which row of a list the pointer is on, in the same walk `FeListBox`'s slot 8 uses —
    /// 0x4C6F60 box-tests the pointer (0x4E2970), then accumulates row heights and takes the
    /// **first** row whose running bottom is past the pointer's y (0x4C7013).
    ///
    /// This is the hovered row the draw highlights, and it is what a click lands on.
    pub fn list_row_at(&self, index: usize, x: f32, y: f32) -> Option<usize> {
        if let Some((k, j)) = self.route(index) {
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            return self.groups.get(&k)?.list_row_at(j, x - gx, y - gy);
        }
        let e = self.layout.elements.get(index)?;
        let (rows, _) = self.lists.get(&index)?;
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let panel_w = if full >= max_h { width - LIST_SCROLLBAR_W } else { width };
        let (wx, wy) = self.position_of(e);
        let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
            Some(v) => self.ev_pair(e, v, 0).map(|(a, b)| (wx + a as f32, wy + b as f32)).unwrap_or((wx, wy)),
            None => (wx, wy),
        };
        if !(x >= px && x < px + panel_w && y >= py && y < py + max_h) {
            return None;
        }
        let first = self.scroll.get(&index).copied().unwrap_or(0);
        let mut acc = py;
        rows.iter().skip(first).position(|r| {
            acc += r.height();
            y < acc
        }).map(|n| n + first)
    }

    /// `NFS5.KNOCKOUTTRACKLIST`'s slot 9, **0x4FA470**: the row under a release —
    /// `(ev.y − box.top) · 7 / (box.bottom − box.top)` (`lea eax,[ecx*8]; sub eax,ecx; cdq; idiv`,
    /// 0x4FA4AF…0x4FA4B9, truncating) over the widget's box (0x4C8670) — handed to the
    /// `[this+0x144]` accessor's `SetSelection` (slot 0x2C) when it is **below** the count
    /// `[this+0x140]` answers (slot 0x6C, `jge` at 0x4FA4BD); otherwise nothing, and the slot
    /// answers 0. There is no lower bound and no x test. `None` for any other widget, or a row
    /// at or past the count. The pointer is in design pixels, taken whole (APPROXIMATION: the
    /// original's are the event's integer pixels).
    pub fn ko_row_at(&self, index: usize, x: f32, y: f32) -> Option<usize> {
        if let Some((k, j)) = self.route(index) {
            let (gx, gy) = self.position_of(self.layout.elements.get(k)?);
            return self.groups.get(&k)?.ko_row_at(j, x - gx, y - gy);
        }
        let e = self.layout.elements.get(index)?;
        if e.class != "NFS5.KNOCKOUTTRACKLIST" {
            return None;
        }
        let &(rows, _) = self.ko_strip.get(&index)?;
        let (_, wy) = self.position_of(e);
        let (_, h) = self.box_of(e, 0);
        let (top, span) = (wy as i32, h as i32);
        if span == 0 {
            return None;
        }
        let row = (y.floor() as i32 - top).wrapping_mul(7) / span;
        usize::try_from(row).ok().filter(|r| *r < rows)
    }

    /// Park a list's knob where a press on the **track** put it, until the model moves
    /// (`RE/notes/controls.md` §4.46 — the tail of 0x4D0995). `None` gives it back to the model.
    pub fn park_knob(&mut self, index: usize, y: Option<f32>) {
        if let Some((k, j)) = self.route(index) {
            let dy = self.layout.elements.get(k).map(|e| self.position_of(e).1).unwrap_or(0.0);
            if let Some(g) = self.groups.get_mut(&k) {
                g.park_knob(j, y.map(|v| v - dy));
            }
            return;
        }
        self.knob_at = y.map(|v| (index, v));
    }

    /// Hand an `NFS5.3DCAR` the offscreen view its car was rendered into. The view covers the whole
    /// screen — the car's place in it comes from the camera, not from the widget's rectangle — and
    /// is drawn in the widget's own place in the layout order, which is what puts it over the art
    /// behind it and under the art in front.
    pub fn set_car_view(&mut self, index: usize, view: Option<TextureId>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_car_view(j, view);
            }
            return;
        }
        match view {
            Some(t) => self.car_views.insert(index, t),
            None => self.car_views.remove(&index),
        };
    }

    /// The offscreen view already handed to this widget, so the caller can render into it again.
    pub fn car_view(&self, index: usize) -> Option<TextureId> {
        self.car_views.get(&index).copied()
    }

    /// The `NFS5.3DCAR` widgets on this screen.
    pub fn car_3d(&self) -> Vec<Car3dWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.3DCAR")
            .map(|(index, e)| Car3dWidget {
                index,
                accessor: self.attr(e, "accessor").map(str::to_string),
                othersource: self.attr(e, "othersource").map(str::to_string),
                position_type: self.attr_num(e, "positiontype").unwrap_or(0.0) as i64,
                damage: self.attr_num(e, "damage").unwrap_or(0.0) != 0.0,
                // `transition_of`'s alpha is `1 − k / 255` with 0x4C8840's integer `k`
                alpha: (self.transition_of(e).1 * 255.0).round() as i32,
            })
            .collect()
    }

    /// The `NFS5.CarLogo` widgets on this screen, with the car source each one names.
    pub fn car_logos(&self) -> Vec<CarLogoWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CarLogo")
            .map(|(index, e)| CarLogoWidget {
                index,
                accessor: e.get("accessor").filter(|a| !a.is_empty()).map(str::to_string),
                othersource: e.get("othersource").filter(|a| !a.is_empty()).map(str::to_string),
            })
            .collect()
    }
}
