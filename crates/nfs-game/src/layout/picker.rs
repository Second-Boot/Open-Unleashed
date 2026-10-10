//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    pub(super) fn drop_down(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let Some((rows, _)) = self.lists.get(&i) else { return };
        if rows.is_empty() {
            return;
        }
        let Some(f) = self.font_for(e) else { return };
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(160.0) as f32;
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let height = self.attr_num(e, "height").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        // `refreshRows` 0x4C53F3: the rows' height, clamped, and the scrollbar that clamping raises
        let (rows_h, scrolls) = if full >= max_h { (max_h, true) } else { (full, false) };
        // **The panel is `data.selectionboxwidth` wide when that is wider than the widget**, and
        // `data.selectionboxslidedirection` says which way it grows. `FeComboBox::refreshRows`
        // 0x4C5607…0x4C5743 animates both the height and the width off the tick clock:
        //
        // ```text
        // 0x4C5626  h    = min((now - [+0x1D4]) * [+0x1C4] / 16, [+0x1C4])   ; the rows' height
        // 0x4C5669  grow = clamp((now - [+0x1D4]) * (sbw - w) / 16, 0, sbw - w)
        // 0x4C56B9  if (sbw < w) grow = 0
        // 0x4C56C1  if (!focused) both count back down — the panel retracts the same way
        // 0x4C56F6  [+0x1D0] == -1 -> 0x4E2BB0(selectionbox, x0 - grow)      ; grows leftwards
        // 0x4C5736  0x4E2BF0(selectionbox, w - <16 while scrolling> + grow)  ; the new width
        // ```
        //
        // so it reaches `max(data.width, data.selectionboxwidth)` after **16 ticks**. This port
        // draws the settled end of that ramp, which is what a screen at rest shows.
        //
        // The three offsets this rests on are **not** what `§4.44` had: 0x4C4FCB…0x4C5002 push
        // each name *before* storing the previous call's answer, so `[+0x1C8]` is `data.width`,
        // `[+0x1CC]` is `data.selectionboxwidth` and `[+0x1D0]` is
        // `data.selectionboxslidedirection`, one slot along from the old table.
        let sbw = self.attr_num(e, "selectionboxwidth").unwrap_or(0.0) as f32;
        let grow = (sbw - width).max(0.0);
        let left = self.attr_num(e, "selectionboxslidedirection").unwrap_or(0.0) as i32 == -1;
        let (px, py) = (if left { x - grow } else { x }, y + height);
        let width = width + grow;
        let rect_w = if scrolls { width - LIST_SCROLLBAR_W } else { width };
        let t = if max_h > 0.0 { (rows_h * 255.0 / max_h).round().clamp(0.0, 255.0) as u8 } else { 0 };
        let amber = [0xff, 0xbb, 0x00, 0xff];
        let low = [0x82, 0xa8, 0xb0, 0xb2];
        // 0x4D8850, the per-channel lerp the front end uses everywhere: `a + (b - a) * t / 255`
        let mix = |a: [u8; 4], b: [u8; 4]| -> [u8; 4] {
            std::array::from_fn(|j| {
                let (a, b) = (a[j] as i32, b[j] as i32);
                (a + (b - a) * t as i32 / 255) as u8
            })
        };
        let bottom = mix(amber, low);
        let (ca, cb) = (tint(amber), tint(bottom));
        // **The panel is a depth mask's stencil** (read 2026-09-27): 0x4DAA10 (0x4C6516), the
        // gouraud (0x4C6605), 0x4DAAC0 (0x4C660D — the combo's arm jumps past the list box's
        // alpha-test pair, 0x4C6612 → 0x4C669C), then the outline, the rows and the scrollbar
        // (0x4C66E0, 0x4C670B…0x4C691B, 0x4C693D) and 0x4DAB00 (0x4C6942): everything after
        // the panel lands on the panel alone, and the row that overhangs its foot is cut there.
        // The depth is given when the overlay is appended (`overlay_mask`).
        let stencil = out.len();
        self.gouraud(
            out,
            aspect,
            // the rect's height is the rows' `h` 0x4C57D5 set (`drop_down_rect`) — *corrected
            // 2026-09-27: this drew `selectionboxmaxheight` whatever the list held (§4.44)*
            [[px, py], [px + rect_w + if scrolls { LIST_SCROLLBAR_W } else { 0.0 }, py], [px, py + rows_h], [px + rect_w + if scrolls { LIST_SCROLLBAR_W } else { 0.0 }, py + rows_h]],
            [ca, ca, cb, cb],
        );
        let inside = out.len();
        let black = tint([0, 0, 0, 255]);
        // the sixteen pixels the panel gave up carry the drop-down's own scrollbar (§4.90). Its
        // knob travel is the scrollbar model's own **count** (§4.59's `+2`), not the row total —
        // `combo_scroll_geom`'s doc comment has the read.
        if scrolls {
            self.combo_scrollbar(out, aspect, px + rect_w, py, max_h, self.last_first_row(i, rows) + 2, self.scroll.get(&i).copied().unwrap_or(0), &tint);
        }
        for (bx, by, bw, bh) in [(px, py, rect_w, 1.0), (px, py + rows_h - 1.0, rect_w, 1.0), (px, py, 1.0, rows_h), (px + rect_w - 1.0, py, 1.0, rows_h)] {
            out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), black));
        }
        let text_c = self.attr(e, "textcolor").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
        // the same foot-of-loop test as the list box's (0x4C6915): the last row is drawn and the
        // mask cuts it at the panel's foot (it "overhangs" until 2026-09-27); the
        // first row drawn is the scrollbar's position `[+0x1BC]->slot10()` (0x4C66F8, §4.44) —
        // *until 2026-09-27 the port drew from row 0 whatever the bar said*
        // `FEOpponentCarAccessor`'s own row draw, `0x4F58A0` (vtable `0x5B77D8` slot 15 —
        // `controls.md` §4.140; it shares the row-draw shape and call site (`0x4C68F5`)
        // `FEColourAccessor`'s `0x4B9390` does, confirmed by both taking the exact same
        // `(row, font, x, y, w, inset=3.0, text, bar)` args off `0x4C687D`'s branch). Read to
        // the instruction: the shared bar/text colour block just above that branch
        // (`0x4C675E`…`0x4C67AE`) leaves `bar` (`ebx`) at **0** — transparent — for every row
        // except the one currently under the pointer, which takes `data.highlightcolorhover`
        // (`[+0x1E8]`) through `0x4D8830`; `ID_OPPONENTCARBOX`'s own `ComboBoxSlider.ctr` sets
        // no such key (`QuickRace.lay`), so it keeps the code default `0x4D87B0(0xFF)` —
        // opaque **`0xBC0000`**, dark red. `0x4F58A0` always draws that bar (`0x4D9280`, its
        // own literal `14.0`, matching `LIST_ROW_H`) before its row text, unconditionally for
        // its five rows — the generic loop's own `sel_c` bar just below, gated on `current`
        // rather than the pointer, is `FeListBox`'s "selected row" arm (`[+0x1DD]`, §4.42),
        // which never fires for a `ComboBoxSlider` (`[+0x1DD]==0`) and so never reaches these
        // rows in the original either way. COVERAGE §14.31.
        let opponent_car_hover = self
            .attr(e, "accessor")
            .is_some_and(|a| a.starts_with("OpponentCar.Car"))
            .then(|| self.pointer.unwrap_or(self.cursor))
            .flatten();
        let mut acc = 0.0f32;
        let mut ry = py;
        let first = self.scroll.get(&i).copied().unwrap_or(0);
        for (n, row) in rows.iter().enumerate().skip(first) {
            if acc > max_h {
                break;
            }
            acc += row.height();
            if opponent_car_hover.is_some_and(|(cx, cy)| self.combo_row_at(cx, cy) == Some((i, n))) {
                out.push(self.quad(aspect, px, ry, rect_w, row.height(), tint([0xBC, 0x00, 0x00, 0xFF])));
            }
            // No "selected row" bar and no white text here: that arm of the shared row draw
            // (0x4C677C, §4.42) is gated on `[this+0x1DD]`, which `FeComboBox::FeComboBox`
            // (0x4C4CF0) sets to 1 only for `NFS5.ListBox` (0x4CCE5A) and to 0 for
            // `NFS5.ComboBoxSlider` (0x4C4CA7) and `NFS5.ColorBox` (0x4C38A3) — the only widgets
            // whose open list comes through here (`draw_combo`). A list box's own selected row
            // is `draw_list_box`'s. COVERAGE §14.34 (until 2026-09-30 the port drew the combo's
            // current row black with white text).
            let fg = tint(text_c);
            match row {
                ListRow::Text(t) => self.text(out, aspect, f, t, px + 3.0, ry + 2.0, 0.0, 0, fg),
                // 0x4C688F hands the row draw an inset of 3.0 (§4.171)
                ListRow::Swatch(_) | ListRow::CustomColour(_) => self.colour_row(out, aspect, f, row, px, ry, 3.0, &tint),
                // A grouped car list's heading (`garage_rows`/`catalog_rows`, §4.144): the same
                // two-black-rule-plus-bevel shape `draw_list_box`'s own `Header`/`GroupHeading`
                // arms already draw for `Parts.AllParts`/the garage list — this match had no arm
                // for either variant at all, so a combo's own drop-down (`FeComboBox`/
                // `FeListBox` share one row loop, §4.42/§4.43, but not this one — `draw_combo`
                // reaches `drop_down`, not `draw_list_box`) drew the heading's reserved height as
                // **blank space**: the row total came out right (`OwnedCars.Car`'s 42/28/14
                // already matched) but the era/class text and its two rules never appeared. Found
                // porting `GenericCar.Car`/`DealerCar.Car`/`GenericCar2.Car`'s own grouping
                // (`catalog_rows`) and fixed for both — the garage combo drew this same blank gap
                // before now, uncaught because no screenshot check had looked inside an *open*
                // grouped combo specifically.
                ListRow::Header(t, _) | ListRow::GroupHeading(t) => {
                    let black = tint([0, 0, 0, 0xFF]);
                    out.push(self.quad(aspect, px, ry, rect_w, 1.0, black));
                    out.push(self.quad(aspect, px, ry + PART_HEADER_H, rect_w, 1.0, black));
                    self.text(out, aspect, f, t, px, ry + 2.0, rect_w, 1, tint(text_c));
                    let bevel_y = if matches!(row, ListRow::GroupHeading(_)) { ry + 1.0 } else { ry };
                    let bevel_h = if matches!(row, ListRow::GroupHeading(_)) { 12.0 } else { PART_HEADER_H };
                    self.bevel_heading(out, aspect, FeRect::new(px, bevel_y, rect_w, bevel_h), 2.0);
                }
                ListRow::Columns(_) | ListRow::Racer(..) => {}
            }
            ry += row.height();
        }
        self.overlay_mask.set(Some((stencil, inside, out.len())));
    }

    /// `FEColourAccessor`'s row draw **0x4B9390**, the part after the row's bar: a colour row is a
    /// 20-wide swatch at `(x + inset, y + 2)`, `h − 4` tall, in the colour at the row's text alpha
    /// (`0x4D87F0(c, text >> 24)`, which leaves a colour alone at 0xFF), with a black outline
    /// (0x4D9200 of `0x4D87E0(alpha)`: four one-pixel bars); the custom row is `FEStrings` 0x263
    /// at `(x + inset, y + 2)`, align 0, in that black. Both take the row height from slot 26,
    /// the base 14 (0x4BC700). `RE/notes/controls.md` §4.171.
    pub(super) fn colour_row(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, row: &ListRow, x: f32, y: f32, inset: f32, tint: &impl Fn([u8; 4]) -> [f32; 4]) {
        let black = tint([0, 0, 0, 0xFF]);
        match row {
            ListRow::Swatch(c) => {
                let (sx, sy, sw, sh) = (x + inset, y + 2.0, 20.0, row.height() - 4.0);
                out.push(self.quad(aspect, sx, sy, sw, sh, tint(*c)));
                for (bx, by, bw, bh) in [(sx, sy, sw, 1.0), (sx, sy + sh - 1.0, sw, 1.0), (sx, sy, 1.0, sh), (sx + sw - 1.0, sy, 1.0, sh)] {
                    out.push(self.quad(aspect, bx, by, bw, bh, black));
                }
            }
            ListRow::CustomColour(t) => self.text(out, aspect, f, t, x + inset, y + 2.0, 0.0, 0, black),
            _ => {}
        }
    }

    /// `NFS5.ColorBox`'s custom colour editor — its own **slot 17**, 0x4C3C30, which draws when
    /// the widget holds focus and the class's global latch [0x65B710] is set.
    ///
    /// Everything hangs off the widget's own box `(l, t, r, b)` and the six rects the bind builds
    /// from it (§4.47, [`picker_rects`]). In order, and each one traced:
    ///
    /// 1. the box itself becomes the **title bar** — filled `0x4D87C0(fade)`, which is
    ///    `0xFFBB00` amber (0x4C3D79), outlined black (0x4C3D91), with `FEStrings` **610**,
    ///    "CUSTOM COLOR EDITOR", centred at `y + 2` in black and the `[+0x298]` font,
    ///    `Univers7b.ffn` (0x4C3DF2);
    /// 2. a **160-pixel** panel below it, the widget's width, gouraud amber `0xffffbb00` across
    ///    the top and `0xb282a8b0` across the bottom (0x4C3E33), outlined black (0x4C3E4E);
    /// 3. rect **A** is the **luminance ramp**: one 1-pixel column per x with the hue and the
    ///    saturation held and the third float sweeping 0…1 through 0x4D8900
    ///    (0x4C3E64…0x4C3F06), a black frame one pixel around it (0x4C3F73) and a **3-pixel**
    ///    black marker at `(A.x1 - A.x0 - 2)·l + A.x0 + 1` (0x4C3FEC);
    /// 4. rect **B** is the current colour, filled (0x4C4062) and outlined (0x4C40BD);
    /// 5. rect **C** is the sprite the factory looked up by the tag `COLB` (0x4C40EE), drawn white
    ///    at its top-left, with a black frame one pixel around it (0x4C4151) — the hue across it
    ///    and the saturation up it, which is what its mouse slots read back out of it (§4.66);
    /// 6. rect **D** is the **eight wells**, `[0x691D98 + n·4]` through 0x5019B0 — all
    ///    **zero in the image**, so they start empty — each filled and outlined black, and the
    ///    selected slot `[+0x284]` (**0** out of the factory, 0x4C38B3) outlined white (0x4C42A1);
    /// 7. rect **E** is two buttons, `FEStrings` **702** "ok" and **703** "cancel": dark red
    ///    `0x4D87B0(fade)` — amber `0x4D87C0` while `[this+0x218]`, the region the hit test left,
    ///    is **3** for the left one or **4** for the right (0x4C42D0, 0x4C42D7) — the label
    ///    centred at `y + 2` in white and the `[+0x294]` font, the §4.21 bevel at a thickness of
    ///    **3** over the hovered one (0x4C4414) and a black outline (0x4C44C9).
    ///
    /// The three colour floats are `[+0x288]`, `[+0x28C]` and `[+0x290]` — **hue, saturation and
    /// luminance**, because 0x4D8900 is an HLS→RGB (§4.66) — 0.5 each out of the factory
    /// (0x4C38B9). The port moves them from the pointer exactly as the class's three mouse slots
    /// do, so the editor is live.
    pub(super) fn color_picker(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let FeRect { x, y, w, h } = rect;
        let (l, t, r, b) = (x, y, x + w, y + h);
        let rects = picker_rects(l, t, r, b);
        let region = self.cursor.map(|(cx, cy)| picker_region(&rects, cx, cy)).unwrap_or(5);
        let black = tint([0, 0, 0, 255]);
        let white = tint([255, 255, 255, 255]);
        let outline = |out: &mut Vec<Sprite>, x: f32, y: f32, w: f32, h: f32, c: [f32; 4]| {
            for (bx, by, bw, bh) in [(x, y, w, 1.0), (x, y + h - 1.0, w, 1.0), (x, y, 1.0, h), (x + w - 1.0, y, 1.0, h)] {
                out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), c));
            }
        };
        let word = |c: u32| tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]);
        // 1 — the title bar
        out.push(self.quad(aspect, l, t, w, h, tint([0xff, 0xbb, 0x00, 0xff])));
        outline(out, l, t, w, h, black);
        if let Some(f) = self.fonts.get(PICKER_TITLE_FONT) {
            let s = self.fe_strings.get(610).unwrap_or_default();
            self.text(out, aspect, f, s, l, t + 2.0, w, 1, black);
        }
        // 2 — the panel
        const PANEL_H: f32 = 160.0;
        let amber = tint([0xff, 0xbb, 0x00, 0xff]);
        let muted = tint([0x82, 0xa8, 0xb0, 0xb2]);
        self.gouraud(out, aspect, [[l, b], [l + w, b], [l, b + PANEL_H], [l + w, b + PANEL_H]], [amber, amber, muted, muted]);
        outline(out, l, b, w, PANEL_H, black);
        let (ph, ps, pl) = self.picker.hsl;
        // 3 — the luminance ramp
        let (ax0, ay0, ax1, ay1) = rects[rect::A];
        let mut px = ax0;
        while px <= ax1 {
            let tt = (px - ax0) / (ax1 - ax0);
            out.push(self.quad(aspect, px, ay0, 1.0, ay1 - ay0, word(hls_to_rgb(ph, ps, tt))));
            px += 1.0;
        }
        outline(out, ax0 - 1.0, ay0 - 1.0, (ax1 - ax0) + 2.0, (ay1 - ay0) + 2.0, black);
        outline(out, ((ax1 - ax0 - 2.0) * pl).round() + ax0 + 1.0, ay0 - 2.0, 3.0, (ay1 - ay0) + 4.0, black);
        // 4 — the swatch
        let (bx0, by0, bx1, by1) = rects[rect::B];
        out.push(self.quad(aspect, bx0, by0, bx1 - bx0, by1 - by0, word(hls_to_rgb(ph, ps, pl))));
        outline(out, bx0, by0, bx1 - bx0, by1 - by0, black);
        // 5 — the `COLB` sprite
        let (cx0, cy0, cx1, cy1) = rects[rect::C];
        if let Some(n) = self.colb
            && let Some(&(tex, sw, sh)) = self.shapes.get(&("weeicons.fsh".to_string(), n))
        {
            let (pos, size) = self.place(aspect, cx0, cy0, sw, sh);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: white, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
        }
        outline(out, cx0 - 1.0, cy0 - 1.0, (cx1 - cx0) + 2.0, (cy1 - cy0) + 2.0, black);
        // 6 — the eight wells, and the selected one in white
        let (dx0, dy0, dx1, dy1) = rects[rect::D];
        let w8 = ((dx1 - dx0) / 8.0).trunc();
        for n in 0..8 {
            let sx = dx0 + n as f32 * w8;
            out.push(self.quad(aspect, sx, dy0, w8.max(1.0), dy1 - dy0, word(self.picker.wells[n])));
            outline(out, sx, dy0, w8.max(1.0), dy1 - dy0, black);
        }
        let sel = dx0 + self.picker.well.min(7) as f32 * w8;
        outline(out, sel, dy0, w8.max(1.0), dy1 - dy0, white);
        // 7 — the two buttons
        for (i, id, hot) in [(rect::E_LEFT, 702, 3), (rect::E_RIGHT, 703, 4)] {
            let (ex0, ey0, ex1, ey1) = rects[i];
            let on = region == hot;
            let fill = if on { [0xff, 0xbb, 0x00, 0xff] } else { [0xbc, 0x00, 0x00, 0xff] };
            out.push(self.quad(aspect, ex0, ey0, ex1 - ex0, ey1 - ey0, tint(fill)));
            if let Some(f) = self.fonts.get(PICKER_BUTTON_FONT) {
                let s = self.fe_strings.get(id).unwrap_or_default();
                self.text(out, aspect, f, s, ex0, ey0 + 2.0, ex1 - ex0, 1, white);
            }
            // pressed takes the bevel (0x4C4414, 0x4C4603) and anything else the inset
            // highlight (0x4C446B, 0x4C465A) — the same two-arm tail every button here has
            if on {
                self.bevel(out, aspect, FeRect::new(ex0, ey0, ex1 - ex0, ey1 - ey0), 3.0);
            } else {
                self.glass(out, aspect, FeRect::new(ex0, ey0, ex1 - ex0, ey1 - ey0), &tint);
            }
            outline(out, ex0, ey0, ex1 - ex0, ey1 - ey0, black);
        }
    }

    /// The colour editor, as the port's own state (§4.66) — the three floats, the selected well
    /// and the eight wells.
    pub fn picker(&self) -> Picker {
        self.picker
    }

    /// The eight wells are the global `[0x691D98]` (getter 0x5019B0, setter 0x5019C0), not the
    /// widget's: the profile's `PlayerInfo` loads them (0x501888) and every screen's editor shows
    /// and edits the same eight. The caller hands them in when a screen is bound.
    pub fn set_picker_wells(&mut self, wells: [u32; 8]) {
        self.picker.wells = wells;
    }

    /// The `NFS5.ColorBox` whose picker is up, if one is.
    ///
    /// The original's gate is two things at 0x4C3D0C: the widget holds **focus** and the class's
    /// global latch `[0x65B710]` is set. The latch is raised by `0x4C39F0`, called from one place
    /// — 0x4C6D6C, in the shared combo commit, and only for a widget whose constructor byte
    /// `[this+0x1DE]` is set, which is `NFS5.ColorBox` alone (§4.44): the widget asks its accessor
    /// for the index it calls custom (`0x4B94B0`: `row == accessor->[+0x24]()`, the colour count —
    /// a function-pointer member, not a vtable slot) and opens the editor when the row just chosen
    /// **is** that one, the `custom...` row of the list (§4.171). (history: `game-history.md` §25.1)
    /// **`data.allowcustom` decides whether there is a custom colour to open at all.** The bind
    /// 0x4C3BF0 hands it to the colour accessor at `[this+0x220]` through 0x4B9590, which is one
    /// instruction — `[accessor+0x30] = flag` — and the accessor's own count reads that byte
    /// twice, at 0x4B9232 and 0x4B9353, adding `0x5019D0() + 1` entries to the list when it is set
    /// and none when it is not. So a box with `allowcustom=0` offers the car's paint sets and
    /// nothing else. Two of the eleven shipped boxes carry 0 and they are `dealership.lay`'s
    /// `DealerCar.Colour` and `DealerCar.IntColour` — you cannot repaint a car you have not bought
    /// — while the paint shop's four and the five race screens' all carry 1. §4.86.
    pub fn open_picker(&self) -> Option<usize> {
        self.open_combo.filter(|_| self.picker_latch).filter(|i| {
            self.layout
                .elements
                .get(*i)
                .is_some_and(|e| e.class == "NFS5.ColorBox" && self.attr_num(e, "allowcustom").unwrap_or(0.0) != 0.0)
        })
    }

    /// The picker's box and its six rects, for the widget that has it up.
    fn picker_geometry(&self) -> Option<[(f32, f32, f32, f32); 6]> {
        let i = self.open_picker()?;
        let e = self.layout.elements.get(i)?;
        let (x, y) = self.position_of(e);
        // the same box the draw hangs the picker off — `NFS5.ColorBox` is one of the eight
        // templates with `autosize=0` (§4.62), so this is its `size` attribute
        let (w, h) = self.box_of(e, 0);
        Some(picker_rects(x, y, x + w, y + h))
    }

    /// Where the pointer is in the open picker, `[this+0x218]` — 5 when it is nowhere in it.
    pub fn picker_region(&self) -> u32 {
        match (self.picker_geometry(), self.cursor) {
            (Some(rects), Some((x, y))) => picker_region(&rects, x, y),
            _ => 5,
        }
    }

    /// The picker's **slot 5**, the button going down — 0x4C4710.
    ///
    /// It hit-tests, then, only while the widget is focused and the latch is up, moves whichever
    /// of the two float pairs the pointer is on: region **0** takes the hue from x across rect C
    /// and the saturation from `(C.y1 - y)` **up** it (0x4C474D…0x4C479D — the y term is inverted,
    /// the x term is not), region **1** takes the luminance from x across rect A
    /// (0x4C47A8…0x4C47CB). Whatever the region, it then packs the three with 0x4D8900 and writes
    /// the result into the selected well (0x4C47E6, 0x4C47F3) and tells the accessor
    /// (`slot 0x6C`).
    pub fn picker_press(&mut self, x: f32, y: f32) {
        let Some(rects) = self.picker_geometry() else { return };
        match picker_region(&rects, x, y) {
            0 => {
                let (x0, y0, x1, y1) = rects[rect::C];
                self.picker.hsl.0 = (x - x0) / (x1 - x0);
                self.picker.hsl.1 = (y1 - y) / (y1 - y0);
            }
            1 => {
                let (x0, _, x1, _) = rects[rect::A];
                self.picker.hsl.2 = (x - x0) / (x1 - x0);
            }
            _ => {}
        }
        self.commit_well();
    }

    /// The picker's **slot 8**, the pointer moving — 0x4C4830.
    ///
    /// The same two arms as the press, behind one more gate: **0x5299C0**, a mouse button is held
    /// (0x4C4863). The widget holds the capture while it is focused (§4.57), so the move reaches
    /// it wherever the pointer goes — but each arm still needs its own rect, so a drag that leaves
    /// rect C stops moving the hue and the saturation instead of running away with them.
    pub fn picker_move(&mut self, x: f32, y: f32) {
        if !self.pressed {
            return;
        }
        self.picker_press(x, y);
    }

    /// The picker's **slot 9**, the button coming up — 0x4C4960. Answers whether it closed.
    ///
    /// Region **3** is "ok": drop the latch, drop focus (0x4C7DA0) and hand the accessor back what
    /// it already has, `slot 0x28` into `slot 0x2C` (0x4C49AD…0x4C49BF). Region **4** is "cancel":
    /// drop both, then put all **eight** wells back from the copy the first drawn frame took
    /// (0x4C49E6…0x4C4A00) and hand the accessor the value it had then, `[this+0x29C]`
    /// (0x4C4A08). Both play sound **5** (0x4AADB0). Region **2** picks a well —
    /// `(x - D.x0)·8 / (D.x1 - D.x0)`, an integer divide clamped to `0…7` (0x4C4A2C…0x4C4A6E) —
    /// and, **only when that well is not zero** (0x4C4A7D), reads its colour back into the three
    /// floats with 0x4D8BD0.
    pub fn picker_release(&mut self, x: f32, y: f32) -> bool {
        let Some(rects) = self.picker_geometry() else { return false };
        match picker_region(&rects, x, y) {
            3 => return true,
            4 => {
                self.picker.wells = self.picker.saved;
                return true;
            }
            2 => {
                let (x0, _, x1, _) = rects[rect::D];
                let n = ((x - x0) as i32 * 8) / (x1 - x0) as i32;
                self.picker.well = n.clamp(0, 7) as usize;
                let c = self.picker.wells[self.picker.well];
                if c != 0 {
                    self.picker.hsl = rgb_to_hls(c);
                }
            }
            _ => {}
        }
        false
    }

    /// The tail every press and move runs: pack the three floats and write the selected well
    /// (0x4C47E3…0x4C47F8).
    fn commit_well(&mut self) {
        let (h, s, l) = self.picker.hsl;
        let w = self.picker.well.min(7);
        self.picker.wells[w] = hls_to_rgb(h, s, l);
    }

    /// Take the copy of the eight wells that **cancel** puts back. The original takes it in the
    /// first frame the picker draws and clears the guard whenever it is down (0x4C3CB3, 0x4C3CFA,
    /// 0x4C3D03), so it is one copy per opening — which is what this is.
    pub fn picker_opened(&mut self) {
        self.picker.saved = self.picker.wells;
    }
}
