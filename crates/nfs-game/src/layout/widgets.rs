//! One method per widget class — the arms of `LayoutScreen::sprites_reporting`'s class match,
//! moved out of it one at a time (2026-09-21) so that a class's draw and the addresses it was read
//! from sit together and can be found by name.
//!
//! Each arm moved **byte for byte** apart from three mechanical changes: its indentation drops by
//! twelve spaces (it is a method now, not a match arm inside a loop), the labelled break out of
//! the arm becomes `return`, and the arm's mutable borrow of the screen's sprite list becomes the
//! `out` parameter, which is already a `&mut Vec<Sprite>`. The
//! comment block that stood above the arm stands above the method. Every move is checked against
//! 97 golden screenshots of the shipped layouts, byte for byte.
use super::*;

/// Everything one widget's draw reads from the loop in `sprites_reporting`: which widget, where
/// it is this frame (transition offset included) and how big, the transition alpha, and the
/// screen-wide inputs the loop was called with.
#[derive(Clone, Copy)]
pub(super) struct WidgetDraw<'a> {
    pub(super) i: usize,
    pub(super) e: &'a nfs_formats::lay::Element,
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
    pub(super) talpha: f32,
    pub(super) aspect: f32,
    pub(super) strings: &'a Strings,
    pub(super) selected: Option<usize>,
    pub(super) hovered: Option<usize>,
    pub(super) accessor: &'a dyn Fn(&str) -> Option<String>,
}

impl LayoutScreen {
    // `FEGouraudIndicatorBar::draw` (0x4CA690) asks its source for a number, then at
    // 0x4CA71A picks the span it paints: with `goingleft` clear it is `0 ..= value`,
    // and with it set `100 - value ..= 100`, so the mirrored column of the car-compare
    // screen fills from its right edge. The value is a per cent because the widget's
    // own colour stops are named `c0`, `c50` and `c100`.
    pub(super) fn draw_gouraud_indicator_bar(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, aspect, .. } = d;
        let Some(&value) = self.bars.get(&i) else { return };
        let value = value.clamp(0, 100) as f32;
        let going_left = e.get("goingleft").is_some_and(|v| v.trim() != "0");
        let (lo, hi) = if going_left { (100.0 - value, 100.0) } else { (0.0, value) };
        if hi <= lo {
            return;
        }
        // The three stops are the colour at 0, 50 and 100 per cent of the **box**, not
        // of the filled part, and the span really is split at fifty: the shared bar
        // painter **0x521CB0** draws it as two gouraud pieces, `c0`→`c50` up to the
        // 50.0f at [0x5B251C] and `c50`→`c100` past it, each clamped to the filled
        // part (0x521D5A…0x521E3D). CONFIRMED against 0x4CA690, which hands 0x521CB0
        // the box, the span and the three colours, and against the bind 0x4CA1C0.
        //
        // One rule the shipped screens never exercise, kept because it is the class's:
        // if the widget gives **no `c50`** the bind promotes the third stop into the
        // second and clears the third (0x4CA227…0x4CA23D), and 0x521CB0's `test ebx,
        // ebx` at 0x521CE1 then takes the two-stop path, blending its own midpoint
        // with `0x4D8850(c0, c100, 0x80)` — half way. All 42 shipped bars carry all
        // three, so this is the port being right rather than lucky.
        let stop = |k: &str| e.color(k).map(tint);
        let c0 = stop("c0").unwrap_or([1.0; 4]);
        // the promotion: no `c50` and the third stop becomes the second, leaving two
        let (c50, c100) = match (stop("c50"), stop("c100")) {
            (Some(a), Some(b)) => (a, Some(b)),
            (None, Some(b)) | (Some(b), None) => (b, None),
            (None, None) => (c0, None),
        };
        let at = |p: f32| -> [f32; 4] {
            // two stops left: one ramp end to end, which is what blending the middle
            // one half way (`0x80`) comes to
            let Some(c100) = c100 else {
                return std::array::from_fn(|j| c0[j] + (c50[j] - c0[j]) * (p / 100.0));
            };
            let (a, b, t) = if p < 50.0 { (c0, c50, p / 50.0) } else { (c50, c100, (p - 50.0) / 50.0) };
            std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
        };
        // 41 of the 42 shipped bars carry `hasshadow=1` and the template's own value
        // is 1, so all but the one with an empty value throw a shadow (§4.79)
        let shade = (self.attr_num(e, "hasshadow").unwrap_or(0.0) != 0.0).then_some(going_left);
        self.gouraud_bar(out, aspect, x, y, w, h, lo, hi, &at, shade);
    }
}

impl LayoutScreen {
    // `NFS5.CheckBox` keeps **two** entries of its pack, `onIndex` and `offIndex`,
    // and `CheckBox.ctr`'s one SHAPE part is written `shapeIndex.value=data.onIndex`
    // — the ticked one, as a default. Which is drawn is the code's:
    // `FECheckBox::refresh` **0x4C3730** asks the `boolAccessor` (`[this+0x144]`,
    // vtable +0x6C) and writes `[this+0x14C]` (`data.onIndex`) or `[this+0x150]`
    // (`data.offIndex`) into `checkBoxPic.shapeIndex` (`[this+0x148]`, through
    // 0x5271F0); 0x4C36E0 is the per-frame watch that calls it when the bool moves.
    //
    // All ten shipped boxes are on `repair.lay` with a `QUICKFIX.<category>` bool, and
    // each of those is registered with a default index of **0** — false — at
    // 0x4F2584 and its nine neighbours, so the repair screen opens with every box
    // clear. The port keeps the same basket — `repair_ticks` in `main.rs`, handed to this
    // screen as `checks` by `bind_checks` — so a box shows what the player has ticked. *(This
    // said "the port has no per-category repair basket" until 2026-09-21; it had one.)*
    pub(super) fn draw_check_box(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, aspect, .. } = d;
        let on = self.checks.get(&i).copied().unwrap_or(false);
        let key = if on { "onIndex" } else { "offIndex" };
        let Some(index) = e.int(key).filter(|n| *n >= 0).map(|n| n as usize) else { return };
        let Some(file) = e.get("shapePathName").filter(|v| !v.is_empty()) else { return };
        let Some(&(tex, aw, ah)) = self.shapes.get(&(file.to_string(), index)) else { return };
        let (pos, size) = self.place(aspect, x, y, aw, ah);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
    }

    // `NFS5.CarSettingSlider` is the template's transparent track (`color.value` is
    // `0x00000000`) and a 6×11 knob from `weeicons.fsh` entry 8, whose `pos.value` is
    // `0,0` and `HIDDEN|READONLY` because the code moves it. The scrollbar helper the
    // class owns does that at **0x4D0C0C…0x4D0C68**, the horizontal branch of
    // `[helper+0x130] == 0`:
    //
    // ```text
    // track = width(track part)          // 0x4E2B90
    // knob  = width(knob part)
    // dx    = count > 1 ? index * (track - knob) / (count - 1) : 0
    // knob.x = knobBase.x + dx           // [helper+0x114]
    // ```
    //
    // — and the vertical branch (0x4D0BBE…0x4D0C06) is the same on `y` over
    // `height(track) - height(knob)` (0x4E2BA0) against `[helper+0x118]`. All five
    // shipped sliders are `width=90 height=13` against a 6×11 knob, so they are
    // horizontal. The index and the count come from the accessor's own registration
    // (`career::car_setup_index`).
    //
    // The knob is one of **three** parts, and the state picks which: the helper binds
    // `knob.pos`, `hoverknob.pos` and `hovermousedownknob.pos` (0x4D0831…0x4D08AA, into
    // `[helper+0xEC/+0xF0/+0xF4]`) and moves all three to the same place (0x5276E0 at
    // 0x4D0C7B, 0x4D0C8F, 0x4D0CA5); the class's draw, slot 15, **is** 0x4C8260, which walks
    // the part list of the **current** state (0x525830: `[[this+0xD4]] + 0x525570()·4`) —
    // `[state.normal]` the knob (`weeicons.fsh` 8), `[state.hover]` `hoverknob` (34),
    // `[state.focused]` `hovermousedownknob` (35) — [`drawn_state`](Self::drawn_state).
    pub(super) fn draw_car_setting_slider(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, aspect, selected, hovered, .. } = d;
        let (idx, count) = self.knobs.get(&i).copied().unwrap_or((0, 1));
        for (px, py, file, index, _) in self.shape_parts_at(e, &self.drawn_state(e, i, selected, hovered), None) {
            let Some(&(tex, dw, dh)) = self.shapes.get(&(file, index)) else { continue };
            let dx = if count > 1 { (idx * (w - dw).max(0.0) as i32 / (count - 1)) as f32 } else { 0.0 };
            let (pos, size) = self.place(aspect, x + px + dx, y + py, dw, dh);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
    }

    // `ComboBox.ctr` (the class is `NFS5.ComboBoxSlider`, 86 widgets) and
    // `ColorBox.ctr` (11) are the same widget, and it is a **dark-red bar**, not a
    // caption floating on the background:
    //
    // * `backrect` fills `0,0`…`data.width,data.height` with `0xFFBC0000`,
    //   `HIDDEN|READONLY` so no layout changes it — the same dark red as a
    //   `NFS5.GOTOBUTTON`'s pill (§4.3);
    // * `labelleft` puts the widget's own `stringID` at `5,2` in `universC11b.ffn`,
    //   white;
    // * `arrowbutton` is `weeicons.fsh` entry **14** at
    //   `data.width-arrowbutton.size.width-4,4`, which `shape_parts` already places;
    // * `selectionbox` is the drop-down and is not in `[state.normal]`.
    //
    // The value is the one piece the template does not settle: its `label` part is
    // `text.value=%data.accessor%` at `data.width-10-label.size.width-3,2` in
    // `universC11m.ffn` with `foreColor=0x00000000` — a **transparent** colour, so the
    // class draws the value from code (`FeComboBox`, the class `NFS5.ListBox` shares)
    // and the part is the editor's preview. Where the code puts it is read further down —
    // 0x4C6054 for the caption and 0x4C6237 for the value — and the port follows that.
    // *(This said "the port keeps its own right-aligned draw … APPROXIMATION, as before"
    // until 2026-09-21, eighty lines above the reading that retired it; the two only came
    // to sit in one function when this arm was moved out of the class match.)*
    /// The caption's font `[+0x190]`: 0x4C555E picks `univers8b.ffn` (`[+0x184]`) under
    /// `data.usesmallfont`, else `[+0x188]`, `universC11b.ffn` — which is `labelleft`'s own
    /// `fontPathName` in both templates.
    fn combo_caption_font(&self, e: &nfs_formats::lay::Element) -> Option<&BitmapFont> {
        let small = self.attr_num(e, "usesmallfont").is_some_and(|v| v != 0.0);
        (!small)
            .then(|| self.controls.get(&e.class).and_then(|c| c.part("labelleft")).and_then(|p| self.part_attr(e, p, "fontPathName")).map(|f| f.to_lowercase()))
            .flatten()
            .or_else(|| small.then(|| SMALL_FONT.to_string()))
            .and_then(|f| self.fonts.get(&f))
            .or_else(|| self.fonts.get(DEFAULT_FONT))
    }

    /// `[+0x1F0]`, the **caption strip**'s width, set by slot 1 every tick (0x4C5594…0x4C55E5):
    /// 0 unless `data.nolabel` (`[+0x1FC]`) is 0 and the `labelleft` part (`[+0x1A0]`) is there;
    /// then the caption's measured width (0x536970 in the caption font `[+0x190]`) plus 6.0,
    /// truncated (`_ftol` 0x5A0F98). Settled 2026-09-28: the string measured is `0x4C55C9`'s call
    /// to `0x4E7310`, which is just `return [part+0x19C]` — the `labelleft` part's own cached
    /// text, the same field [`part_text`](Self::part_text) documents. That field's `LOCALIZEDTEXT`
    /// load (0x4E66E0 → 0x4E06D0) already answers id 0 with the empty string 0x5E8E50 (read
    /// 2026-09-26, layout.rs §"LOCALIZEDTEXT's load"), so a combo with `stringID=0` measures an
    /// empty caption in the original too — width 0, strip 6, matching this port's own
    /// `.filter(|n| *n != 0)`.
    pub(crate) fn combo_strip(&self, e: &nfs_formats::lay::Element) -> f32 {
        if self.attr_num(e, "nolabel").unwrap_or(0.0) != 0.0 {
            return 0.0;
        }
        let text = e.string_id().filter(|n| *n != 0).and_then(|n| self.fe_strings.get(n)).unwrap_or("");
        let Some(f) = self.combo_caption_font(e) else { return 0.0 };
        (f.measure(text).0 + 6.0).trunc()
    }

    pub(super) fn draw_combo(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>, overlay: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, aspect, strings, selected, hovered, accessor, .. } = d;
        self.glinty_box(out, aspect, e, i, x, y, selected, hovered, tint);
        // The combo's own bevel, 0x4C69DF — the last call site of 0x521050 whose
        // arguments were not all read (§4.58, §4.68). Its gate is four tests at
        // 0x4C6947…0x4C698D: not a list box (`[+0x1DD]`), hovered (0x4C7C90), a
        // **button held** (0x5299C0) and not disabled (0x4C7E40) — a pressed
        // decoration. Its rect is `x` from the `selectionbox`'s left (0x4C6961), `y`
        // from the widget's box's **bottom** — `[0x4C8670()+0xC]`, copied at
        // 0x4C58D2 and the only write to that slot — `w` the `backrect`'s width
        // (0x4C69BE) and `h` the two rects' heights together (0x4C6998, 0x4C69A5),
        // which is `data.height` plus the selection box's height — the rows' height slot 1 sets
        // every tick (0x4C57D5 → 0x4E2C20), **0 while the combo holds no focus** (0x4C53C4) and
        // the clamped rows' height while its list is open (`drop_down_rect`). *(Corrected
        // 2026-09-27: this added `data.selectionboxmaxheight` always, on §4.44's "nothing sets
        // the height". The gate's first test, 0x4C6947, is the list-box byte, not "open".)*
        // Thickness **3**.
        if self.pressed_on(i, hovered) {
            let open_h = if self.open_combo == Some(i) { self.drop_down_rect(e, i, x, y).map_or(0.0, |r| r.3) } else { 0.0 };
            self.bevel(out, aspect, x, y + h, w, h + open_h, 3.0);
        }
        let state = self.state_for(e, i, selected, hovered);
        if self.open_combo == Some(i) {
            // a colour box's list is an ordinary drop-down of swatches (§4.171); the
            // editor replaces it once the custom row has raised the latch (0x4C3D0C:
            // focus and `[0x65B710]`) — §4.66, §4.86
            if self.open_picker() == Some(i) {
                self.color_picker(overlay, aspect, x, y, w, h, tint);
            } else {
                self.drop_down(overlay, aspect, e, i, x, y, tint);
            }
        }
        for (px, py, pw, ph, col) in self.state_rects(e, state) {
            out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(col)));
        }
        // `data.dropshadow == 2` replaces the `backrect`'s plain fill with §4.22's red
        // ramp, 0x521900 — the combo has its own call at 0x4C5EC9, on the
        // `data.nolabel != 4` path, over the same `backrect` box. Three of the 86
        // sliders carry 2 and all three are `nolabel=2`, so all three reach it. §4.78.
        // **Under the text**: 0x4C5EC9 comes before the stems (0x4C5F50…), the caption
        // (0x4C6054) and the value (0x4C6237). *(Until 2026-09-27 the port drew it after
        // both, which blanked the CD player's three combos on `audioreal.lay`.)*
        if self.attr_num(e, "dropshadow") == Some(2.0) {
            let (l, r) = (tint([0xe0, 0x0c, 0x0c, 0xff]), tint([0x82, 0, 0, 0xff]));
            let (x0, y0, x1, y1) = (x + 1.0, y + 1.0, x + w - 1.0, y + h - 1.0);
            self.gouraud(out, aspect, [[x0, y0], [x1, y0], [x0, y1], [x1, y1]], [l, r, l, r]);
        }
        // The same stems the buttons have (§4.24), from the same draw — a combo slider
        // and a colour box share `FeComboBox`'s slot 1, and 0x4C5F18…0x4C5FB2 is its
        // copy of the pair: 0x4D9280 twice, one pixel wide, in 0x4D87E0 of the fade
        // (black), each `backrect.height + stem` tall. The left one at the widget's
        // own x (0x4C5F4F), the right at `x + backrect.width` plus **one more pixel
        // when the list is scrolling** (0x4C5F93 reads the `[this+0x1DC]` flag and
        // adds its truth to the x) — a closed combo never scrolls, so the port adds
        // nothing. `backrect` is `data.width` x `data.height` in `ComboBox.ctr`.
        //
        // Unlike the buttons' pair this one is not behind `glintybox`: the draw
        // reaches it whichever way the background above it went (0x4C5E8C's two arms
        // on `dropshadow == 2` both fall into it). **It is not on the `nolabel == 4`
        // path**: 0x4C5A29 `jne 0x4C5E74` sends every other value to the background and
        // the stems, and the `nolabel == 4` arm ends at 0x4C5E6F `jmp 0x4C64FB`, past
        // them. *(Until 2026-09-27 the port drew the pair on the eleven `nolabel=4`
        // sliders too.)*
        //
        // A third one-pixel bar follows, 0x4C5FB7…0x4C5FF6: black again, `backrect.height`
        // tall (0x4E2BA0), at `backrect.left + [+0x1F0]` — the end of the caption strip
        // (`combo_strip`), which a release on a closed combo treats as a step, not an open
        // (0x4C6BB8, `App` side).
        if self.attr_num(e, "nolabel") != Some(4.0) {
            let stem = tint([0, 0, 0, 0xff]);
            let bh = self.attr_num(e, "height").unwrap_or(0.0) as f32;
            let bw = self.attr_num(e, "width").unwrap_or(0.0) as f32;
            let at = |k: &str| self.attr_num(e, k).unwrap_or(0.0) as f32 + bh;
            out.push(self.quad(aspect, x, y, 1.0, at("stemleftheight"), stem));
            out.push(self.quad(aspect, x + bw, y, 1.0, at("stemrightheight"), stem));
            out.push(self.quad(aspect, x + self.combo_strip(e), y, 1.0, bh, stem));
        }
        // The arrow is **not** drawn from a state's part list: the family's slot 1 / 15 / 27
        // (0x4C5460 / 0x4C6A60 / 0x4C5810) never reach 0x4C8260; the bind looks `arrowbutton`
        // up by name (0x4C535D → `[+0x19C]`) and the draw paints that part's entry itself,
        // through 0x4D9AF0, whenever the widget is not hidden (0x4C5818: 0x4C7ED0) — at
        // 0x4C5DB4 on the `nolabel == 4` path and 0x4C62BA on the other. So the part is
        // fixed, whatever the state (every non-hidden state of `ComboBox.ctr` /
        // `ColorBox.ctr` lists it anyway).
        //
        // **X, read to the instruction, 0x4C6260…0x4C629E (`nolabel != 4` only —
        // 0x4C5DA1's other arm still centres on `data.width`, untouched):** `ecx = [esi+0x19c]`
        // ("arrowbutton"'s own field), `0x4d87d0(0,0,ebx)` picks its current-state frame index,
        // `0x4e6a90(part, state)` answers that frame's entry (`edi -= [eax+0x14]`, the frame's
        // own **width** — the same `aw` this loop already reads off `self.shapes`), `0x4e2b90`
        // on `[esi+0x17c]` (`backrect`) gives `edi`'s starting value — **`backrect.width`**
        // (the same object the stems above measure) — and `lea ecx,[edi+eax-3]` is the sum:
        // `backrect.width − entry.width − 3`. The template's own `pos` (`ComboBox.ctr` /
        // `ColorBox.ctr`, `data.width−arrowbutton.size.width−4,4`) is one pixel short — the
        // 1-px shift every combo's golden shows, now closed.
        //
        // **Y, read the same range, `base` settled 2026-09-28 (mechanical stack simulator,
        // `RE/notes/layouts.md` "Combo arrow `y` base, settled"):** `H = 0x4e2ba0` on the same
        // `backrect` (its **height**, `h` below — `ComboBox.ctr`'s `backrect.size =
        // data.width,data.height`, and `h` is that same `data.height`), `eh = [ebp+0x18]` (the
        // same frame's **height**, `ah` below), `(H−eh)` halved by the `cdq`/`sub`/`sar`
        // rounding-toward-zero idiom (Rust's `i32` `/` truncates the same way), `+ base + 1`.
        // `base` is `[esp+0x2c]` at 0x4C6274 — a spill slot this draw function (0x4C5810…)
        // reuses for several unrelated locals (the stem-height floats at 0x4C610D/0x4C611B,
        // among others) across ~250 intervening instructions with no `ret`, so the *literal*
        // text `[esp+0x2c]` names a different physical slot every time a `push`/`call` in
        // between shifts the true offset — resolved with a real stack simulator
        // (`.scratch_re/esp_walk.py`, a CFG-and-esp-delta walk, not a hand trace): the write
        // that survives to 0x4C6274 is 0x4C5916 (`mov [esp+0x24], ebp`, at its own delta),
        // `ebp = 0x4e2b50(backrect, &offset)` (`backrect.pos.y` (0, `ComboBox.ctr`'s
        // `backrect.pos.value=0,0`) `+ offset.y`, `offset` = **0x4C8670(this)'s own box** —
        // the same fully-resolved box `widgets.rs`'s scrollbar code above cites, so
        // `offset.y = box.top` — **the widget's own on-screen `y`, task 1's settled chain**)
        // `+ L2`, `L2 = 0x4C8760(this)`'s vertical output seeded with this widget's own
        // `0x525310` **y** and nudged only while `0x4E3640`'s scroll-drag flag bits 8/0x10 are
        // set — a transient per-frame input state, 0 at rest (every scripted still is
        // non-dragging) — so `base = box.top = y` at rest, exactly the widget's own already-
        // resolved on-screen `y`. Ported: the fixed template `y` (`…,4`) is replaced by the
        // real formula on the `nolabel != 4` path (the scroll-drag nudge is not modelled — no
        // shipped still has a combo mid-drag).
        let arrow_at = out.len();
        for (px, py, file, index, flip) in self.shape_parts_in(e, &["arrowbutton".to_string()], None) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            // the template's own `x` is `data.width − arrowbutton.size.width − 4`; 0x4C629E's
            // is one pixel further right (`nolabel == 4`'s own centring, 0x4C5DA1, is untouched)
            let px = if self.attr_num(e, "nolabel") != Some(4.0) { w - aw - 3.0 } else { px };
            let py = if self.attr_num(e, "nolabel") != Some(4.0) { 1.0 + ((h as i32 - ah as i32) / 2) as f32 } else { py };
            let (pos, size) = self.place(aspect, x + px, y + py, aw, ah);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        // the caption, where the template puts it
        if let Some(c) = self.controls.get(&e.class)
            && let Some(p) = c.part("labelleft")
            && let Some(t) = e.string_id().filter(|n| *n != 0).and_then(|n| strings.get(n)).filter(|t| !t.is_empty())
            && let Some(col) = self.part_colour(e, p, "foreColor").filter(|c| c[3] != 0)
        {
            let (px, py) = self
                .part_attr(e, p, "pos")
                .and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)))
                .unwrap_or((0, 0));
            // **`data.usesmallfont`** puts both strings in `univers8b.ffn`. The class
            // keeps two hard-coded fonts — `[this+0x184]` is `univers8b.ffn`
            // (0x4C4E8E) and `[this+0x188]` `universC11b.ffn` (0x4C4EC6) — and
            // 0x4C555E picks between them into the caption's slot `[this+0x190]`,
            // which is the font the caption draw reads at 0x4C604A. The value's slot
            // `[this+0x194]` takes the same `univers8b.ffn` when the flag is set and
            // the template's `label` font (0x4C532E) when it is not. Twenty-nine of
            // the 46 shipped sliders that carry the key set it.
            let small = self.attr_num(e, "usesmallfont").is_some_and(|v| v != 0.0);
            let f = (!small)
                .then(|| self.part_attr(e, p, "fontPathName").map(|f| f.to_lowercase()))
                .flatten()
                .or_else(|| small.then(|| SMALL_FONT.to_string()))
                .and_then(|f| self.fonts.get(&f))
                .or_else(|| self.fonts.get(DEFAULT_FONT));
            if let Some(f) = f {
                self.text(out, aspect, f, t, x + px as f32, y + py as f32, 0.0, 0, tint(col));
            }
        }
        // And the value, which the class draws itself — `FeComboBox`'s own draw has
        // two text calls, and they settle both:
        //
        // * **0x4C6054** puts the caption at `(x + 5, y + 2)` with align **0** in the
        //   font at `[this+0x190]` — the template's `labelleft`, same place;
        // * **0x4C6237** puts the value with align **2** anchored at
        //   `x + width(this) - 12` (0x4C6220, over 0x4E2B90's width) at the widget's
        //   own `y`, in the second font `[this+0x194]`.
        //
        // The template's `label` part says `data.width-10-label.size.width-3,2`, which
        // is the same right edge one pixel further in and two rows down; the code is
        // what runs, so the port follows the code. That retires the approximation this
        // arm used to carry.
        //
        // **A colour box shows no value text.** Past the caption, 0x4C60FA tests the class's
        // `[this+0x1DE]` (set for `NFS5.ColorBox` alone, §4.44) and draws, in place of the
        // value, a **20 × 10 swatch** of the accessor's current colour — slot 29 0x4C4BF0 →
        // `FEColourAccessor` 0x4B94C0, `[+0x2C]` — at `(x + width − 40, y + 4)`, `width` the
        // `backrect`'s (0x4E2B90 on `[+0x17C]`, the same the value's `x + width − 12` uses),
        // filled `0x4D87F0(colour, fade)` through 0x4D9280 (0x4C6154) and outlined black
        // `0x4D87E0(fade)` through 0x4D9200 (0x4C618B); then 0x4C6193 jumps past the value
        // draw to the arrow (0x4C623F). `nolabel == 1` is tested first (0x4C6062) and keeps
        // the left-aligned value — no shipped colour box has it (all eleven carry 0). §4.171.4.
        if e.class == "NFS5.ColorBox" && self.attr_num(e, "nolabel") != Some(1.0) {
            if let Some(c) = self.swatches.get(&i) {
                let (sx, sy, sw, sh) = (x + w - 40.0, y + 4.0, 20.0, 10.0);
                let black = tint([0, 0, 0, 0xff]);
                out.push(self.quad(aspect, sx, sy, sw, sh, tint(*c)));
                for (bx, by, bw, bh) in [(sx, sy, sw, 1.0), (sx, sy + sh - 1.0, sw, 1.0), (sx, sy, 1.0, sh), (sx + sw - 1.0, sy, 1.0, sh)] {
                    out.push(self.quad(aspect, bx, by, bw, bh, black));
                }
            }
        } else if let Some(v) = e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) {
            let f = self
                .controls
                .get(&e.class)
                .filter(|_| !self.attr_num(e, "usesmallfont").is_some_and(|v| v != 0.0))
                .and_then(|c| c.part("label"))
                .and_then(|p| self.part_attr(e, p, "fontPathName"))
                .map(|f| f.to_lowercase())
                .or_else(|| self.attr_num(e, "usesmallfont").is_some_and(|v| v != 0.0).then(|| SMALL_FONT.to_string()))
                .and_then(|f| self.fonts.get(&f))
                .or_else(|| self.font_for(e));
            let Some(f) = f else { return };
            // **`nolabel == 4`** is a second box, and eleven of the shipped sliders
            // ask for it — all on `advgraphics.lay`, `graphicsettings.lay` and
            // `audioreal.lay`, the same eleven that carry a non-zero
            // `GraphicsSpecialCaseWidth`. `FeComboBox` takes that branch at 0x4C5A26
            // and draws, immediately to the **right** of the slider at
            // `x + data.width` (`[this+0x1C8]`, bound at 0x4C4FDE):
            //
            // * a fill that wide and the slider's own height, **0xA0444444**
            //   (0x4C5A34, through 0x4D9280 at 0x4C5A80);
            // * the same box outlined in **black** — 0x4D87E0 of the fade at
            //   0x4C5A86, through the four-bar 0x4D9200 at 0x4C5AC9;
            // * and the value **centred** in it, at
            //   `x + data.width + GraphicsSpecialCaseWidth/2` and `y + 2`, in
            //   **0xAAFFFFFF** and the same `[this+0x194]` font the ordinary draw uses
            //   (0x4C5B30…0x4C5BD0).
            //
            // and the **`"N/A"`** of `FEStrings` 299 that 0x4C5B64 puts there
            // instead when the widget is **disabled** — 0x4C5AD3 chooses between the
            // two on 0x4C7E40, `[this+0x10C] & 0x10` or the accessor's own byte
            // (§4.65, §4.112). That arm was unreachable until §4.113 gave this port a
            // disabled flag; it is drawn now. §4.125.
            let gscw = self.attr_num(e, "GraphicsSpecialCaseWidth").unwrap_or(0.0) as f32;
            let colour = if selected == Some(i) { [1.0, 0.85, 0.2, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
            // **`nolabel == 1`** — all fourteen of them on `display.lay`, and all
            // fourteen with `stringID=0`, so they have no caption of their own — puts
            // the **value** where the caption would go instead: 0x4C6080 takes the
            // `label` part's string, 0x4C609F its x as `x + 5` and `y + 2`, 0x4C6098
            // align **0**, and 0x4C60AE the **caption's** font `[this+0x190]` rather
            // than the value's `[this+0x194]`, before jumping into the ordinary
            // draw's last two pushes at 0x4C6236. So it is left-aligned in the
            // caption's place, not right-aligned against the arrow.
            if self.attr_num(e, "nolabel") == Some(1.0) {
                let small = self.attr_num(e, "usesmallfont").is_some_and(|q| q != 0.0);
                let cf = (!small)
                    .then(|| self.controls.get(&e.class).and_then(|c| c.part("labelleft")).and_then(|p| self.part_attr(e, p, "fontPathName")).map(|f| f.to_lowercase()))
                    .flatten()
                    .or_else(|| small.then(|| SMALL_FONT.to_string()))
                    .and_then(|f| self.fonts.get(&f))
                    .unwrap_or(f);
                self.text(out, aspect, cf, &v, x + 5.0, y + 2.0, 0.0, 0, colour);
                return;
            }
            if self.attr_num(e, "nolabel") == Some(4.0) && gscw >= 1.0 {
                let bw = self.attr_num(e, "width").unwrap_or(0.0) as f32;
                let bh = self.attr_num(e, "height").unwrap_or(0.0) as f32;
                let (bx, black) = (x + bw, tint([0, 0, 0, 0xff]));
                out.push(self.quad(aspect, bx, y, gscw, bh, tint([0x44, 0x44, 0x44, 0xa0])));
                out.push(self.quad(aspect, bx, y, gscw, 1.0, black));
                out.push(self.quad(aspect, bx, y + bh - 1.0, gscw, 1.0, black));
                out.push(self.quad(aspect, bx, y, 1.0, bh, black));
                out.push(self.quad(aspect, bx + gscw - 1.0, y, 1.0, bh, black));
                let shown = match state == "disabled" {
                    true => strings.get(299).unwrap_or_default().to_string(),
                    false => v.clone(),
                };
                self.text(out, aspect, f, &shown, bx + (gscw / 2.0).trunc(), y + 2.0, 0.0, 1, tint([0xff, 0xff, 0xff, 0xaa]));
                // **Then a depth mask over the slider's own box** (read 2026-09-27): 0x4DAA10
                // (0x4C5C39); a fill one pixel inside the `backrect` (`x + 1, y + 1, w − 2, h − 2`,
                // 0x4C5C3E…0x4C5C93) as the stencil, in 0x4D87F0(0xFFBC0000, fade) — or
                // 0x4D87F0(0x30FF0000, fade) when disabled (0x4C599C), 0x4D87C0(fade) amber when
                // hovered or focused (0x4C59AD…0x4C59C2); 0x4DAAC0 (0x4C5C9B); the **same string
                // again, black** (0x4D87F0(0x4D87E0(fade), 0x30 disabled / 0xFF), 0x4C59E0…0x4C59F5)
                // at the same centre in the special box (0x4C5CA2…0x4C5D2E); 0x4DAB00 (0x4C5D36). So
                // the black copy shows only where the string reaches back into the slider's box —
                // a string wider than `GraphicsSpecialCaseWidth` (172 / 208 shipped), which none of
                // the shipped values is. Read and not added: the box's own black outline just
                // before, 0x4D9200 at 0x4C5C31 — the port paints this family's box from the
                // template's parts, which the original's 0x4C5810 never reaches (§4.35).
                let mask = super::fe_depth::mask_begin();
                let disabled = state == "disabled";
                let fill = if disabled {
                    [0xff, 0, 0, 0x30]
                } else if hovered == Some(i) || self.open_combo == Some(i) {
                    [0xff, 0xbb, 0, 0xff]
                } else {
                    [0xbc, 0, 0, 0xff]
                };
                let stencil = out.len();
                out.push(self.quad(aspect, x + 1.0, y + 1.0, bw - 2.0, bh - 2.0, tint(fill)));
                super::fe_depth::apply(&mut out[stencil..], mask.stencil);
                let inside = out.len();
                self.text(out, aspect, f, &shown, bx + (gscw / 2.0).trunc(), y + 2.0, 0.0, 1, tint([0, 0, 0, if disabled { 0x30 } else { 0xff }]));
                super::fe_depth::apply(&mut out[inside..], mask.inside);
                // the arrow comes after the mask (0x4C5DB4, past 0x4DAB00): the port drew it
                // above, so the mask's draws go in under it
                let masked: Vec<Sprite> = out.drain(stencil..).collect();
                out.splice(arrow_at..arrow_at, masked);
                return;
            }
            // **`nolabel == 2` draws no value at all**: past the caption, 0x4C6198
            // `cmp eax, 2` / `je 0x4C623F` jumps over the value's 0x4DA350 straight to
            // the arrow. The seven shipped are `audioreal.lay`'s CD player (`CDPlayer.View`,
            // `.Options`, `.Title`), `GameCtrl.lay`'s `Controller.AllControllers`,
            // `deadzone.lay`'s `Controller.AllPads`, `PeerToPeerIP.lay`'s `Comms.IPList` and
            // `modem.lay`'s `Comms.PhoneList`; a closed one reads as its caption alone.
            // *(Until 2026-09-27 the port right-aligned the value against the arrow, and
            // "осталось времени трека" / "играть последовательно" ran out of the box.)*
            if self.attr_num(e, "nolabel") != Some(2.0) {
                self.text(out, aspect, f, &v, x + w - 12.0, y, 0.0, 2, colour);
            }
        }
        // and last of all, over everything it has just drawn — the label included —
        // the inset highlight 0x521810 (0x4C5E6A in the closed arm, 0x4C636D in the
        // other), over the **`backrect`** part: `0x4E2B90([this+0x17C])` is
        // `[[part+4]+0x38]`, its width, and `0x4E2BA0` `[+0x3C]`, its height, and
        // `ComboBox.ctr` fixes that part at `pos=0,0` size `data.width,data.height`
        // `HIDDEN|READONLY`, so it is the widget's own box. The same (x, y) pair
        // feeds the box's own one-pixel outline at 0x4C5C31.
        //
        // Both arms skip it when the widget is pressed (0x4C5DC5/0x4C5DCE and
        // 0x4C62CB/0x4C62D4, which is where §4.58's bevel goes instead) and send
        // `data.dropshadow == 2` to 0x521970 instead — `[this+0x209]`, which the bind
        // sets at 0x4C5064 out of the same three-way split the buttons use
        // (0x4C505A…0x4C5088). Three of the 86 sliders carry 2; `ListBox.ctr` has no
        // `dropshadow` at all and `ColorBox.ctr`'s is 1, so both leave the byte 0 and
        // take this. §4.76.
        if !self.pressed_on(i, hovered) {
            if self.attr_num(e, "dropshadow") == Some(2.0) {
                self.bevel_up(out, aspect, x, y, w, h, 3.0);
            } else {
                self.glass(out, aspect, x, y, w, h, &tint);
            }
        }
    }

    // A `NFS5.GOTOBUTTON` is a **dark-red pill with white text**, not red text on
    // nothing. `GoToButton.ctr` says so outright and every one of the 153 shipped
    // buttons carries the colour:
    //
    // * `[state.normal] parts=bg,label,mediumlabel,smalllabel,data`, and `bg` is a
    //   RECT at `1,1` sized `data.width-2,data.height-2` filled with `data.maincolor`
    //   — whose default is `0xFFBC0000` and whose value in the files is that same dark
    //   red on 150 of the 153 (two carry `0x00000000`, which is transparent);
    // * `[state.selected]` and `[state.hover]` swap `bg` for `hoverbg`, the same
    //   rectangle filled with `data.hovercolor` — default `0xffffBB00`, amber. That is
    //   what the original highlights with, so this port no longer has to invent one;
    // * the caption is drawn **three times**, at three places in three fonts, and
    //   `data.medium` picks which one is opaque: `label` at `5,2` in `Universc11b` is
    //   `data.medium?0x00000000:0xffffffff`, `mediumlabel` centred in
    //   `Universc11m` is `(data.medium&1)?0xffffffff:0x00000000`, and `smalllabel` in
    //   `univers8b` — at `5,2` when `medium&4`, centred otherwise — is
    //   `(data.medium&4)?0xffffffff:(data.medium&2)?0xffffffff:0x00000000`. The
    //   shipped `medium` values are 0 (96), 1 (50) and 4 (6).
    //
    // An alpha of zero is invisible, which is how two of the three labels stay out of
    // the way; `BottomBarButton.ctr`'s labels, which really are coloured, all carry a
    // `0xff` alpha instead.
    //
    // Its draw, **0x4C90C0**, puts three more things round that pill, and this port
    // now draws two of them (`RE/notes/controls.md` §4.22). In order:
    //
    // * `dropshadow == 2` — six of the 153 — gets **0x521900** *under* the parts
    //   (0x4C914A). Not reversed, so not drawn;
    // * the template's parts, through 0x4C8260 (0x4C918E);
    // * the **bevel** 0x521050 with a thickness of 3 (0x4C91E4) — ungated: every
    //   button that is fully shown and not mid-transition gets it;
    // * `dropshadow == 1` — the template's own default, and what 132 of the 153 carry
    //   — gets the **drop shadow** 0x520F00 at an offset of (4, 4) (0x4C91FE).
    //
    // `dropshadow` reaches two separate bytes: 1 sets `[this+0x14C]` and 2 sets
    // `[this+0x14D]` (0x4C8F8C…0x4C8FBA), which is why the two are different draws.
    pub(super) fn draw_goto_button(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, aspect, strings, selected, hovered, .. } = d;
        let state = self.state_for(e, i, selected, hovered);
        // `dropshadow == 2` — six of the 153 — gets **0x521900** *under* the parts
        // (0x4C914A), and that is `0x4D96B0(x+1, y+1, w−2, h−2, …)` with the four
        // corners **bright, dark, bright, dark** — 0xFFE00C0C at the left edge and
        // 0xFF820000 at the right (0x52192B…0x52195D), a horizontal red ramp inset by
        // a pixel. Which corner each colour belongs to is settled by 0x4D96B0 itself:
        // it lays the four vertices out (x, y), (x+w, y), (x+w, y+h), (x, y+h) and
        // copies the colours in the order c0, c1, c3, c2 (0x4D973C…0x4D97AF), so the
        // argument order is upper-left, upper-right, **bottom-left**, bottom-right.
        if self.attr_num(e, "dropshadow") == Some(2.0) {
            let (l, r) = (tint([0xe0, 0x0c, 0x0c, 0xff]), tint([0x82, 0, 0, 0xff]));
            let (x0, y0, x1, y1) = (x + 1.0, y + 1.0, x + w - 1.0, y + h - 1.0);
            self.gouraud(out, aspect, [[x0, y0], [x1, y0], [x0, y1], [x1, y1]], [l, r, l, r]);
        }
        self.template_widget(out, aspect, e, state, x, y, strings);
        if self.pressed_on(i, hovered) {
            self.bevel(out, aspect, x, y, w, h, 3.0);
        } else if self.attr_num(e, "dropshadow") == Some(2.0) {
            // the six `dropshadow == 2` buttons take 0x521970 instead (0x4C9291) —
            // §4.77's raised bevel, thickness 3
            self.bevel_up(out, aspect, x, y, w, h, 3.0);
        } else {
            // and the second half of the tail (0x4C923F…0x4C92D9): a button that is
            // **not** pressed takes the inset highlight 0x521810 over its own box —
            // unless `[this+0x14D]`, `data.dropshadow == 2`, which sends it to
            // 0x521970 instead (0x4C9291). Six of the 153 go that way; the other 147
            // get this. §4.75.
            self.glass(out, aspect, x, y, w, h, &tint);
        }
        if self.attr_num(e, "dropshadow") == Some(1.0) {
            self.drop_shadow(out, aspect, x, y, w, h, 4.0, 4.0);
        }
        // **and the last of 0x4C90C0's tail.** A button that is **hovered**
        // (0x4C92DE → 0x4C7C90, `[this+0x10C] & 4` — bit 2 of the state mask, §4.109,
        // where this note used to say "selected") and carries `data.dropshadow == 2`
        // (`[this+0x14D]`, 0x4C92E7) gets a one-pixel outline around its own box in
        // **white at the widget's fade** — 0x4D87D0 at 0x4C92F6 through the four-bar
        // 0x4D9200 at 0x4C932F. Six buttons can ever show it, the same six §4.77's
        // raised bevel belongs to. §4.99.
        if hovered == Some(i) && self.attr_num(e, "dropshadow") == Some(2.0) {
            let white = tint([0xff, 0xff, 0xff, 0xff]);
            for (bx, by, bw, bh) in [(x, y, w, 1.0), (x, y + h - 1.0, w, 1.0), (x, y, 1.0, h), (x + w - 1.0, y, 1.0, h)] {
                out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), white));
            }
        }
        // **The stems** (0x4C9341…0x4C93D9), the thin verticals that hang off the ends
        // of a menu button and join it to the next: two calls to the one-bar primitive
        // 0x4D9280, each **one pixel wide**, at the button's left edge and at
        // `left + width` (0x4C93B4 differences the box and adds the widget's own x
        // back), starting at the widget's `y` and running
        // `stemleftheight + height` / `stemrightheight + height` down — the draw
        // spells that `stem − box.top + box.bottom` (0x4C9362, 0x4C939B). The colour
        // is 0x4D87E0 of the fade, which is `fade << 24`: **black**.
        //
        // They are drawn when `[this+0x150]` is set, and the activate (0x4C9030) sets
        // it exactly when `data.glintybox` is not −1 (0x4C9048). None of the 153
        // shipped buttons carries −1 — 66 carry 1, 40 carry 2, 35 carry 5 and 12
        // leave it empty, which is the template's own 1 — so all of them have stems.
        //
        // `[this+0x150]` is not a flag, though: it is a **glinty rectangle of the
        // button's own box** (§4.60), and the button's draw paints it at 0x4C9461 — a
        // black one-pixel outline and §4.27's corner flourish. It is not drawn here
        // because 0x51FDE0 returns at once unless a device byte is set, and which way
        // that falls needs the original running (`RE/AUDIT.md` §E).
        // `stemleftheight` and `stemrightheight` default to 0, and 0 still draws: the
        // bar is then the button's own height.
        if self.attr_num(e, "glintybox").unwrap_or(1.0) != -1.0 {
            let stem = tint([0, 0, 0, 0xff]);
            let at = |k: &str| self.attr_num(e, k).unwrap_or(0.0) as f32 + h;
            out.push(self.quad(aspect, x, y, 1.0, at("stemleftheight"), stem));
            out.push(self.quad(aspect, x + w, y, 1.0, at("stemrightheight"), stem));
        }
        // and the glinty box itself, which is the last thing the draw does (0x4C9461)
        self.glinty_box(out, aspect, e, i, x, y, selected, hovered, tint);
    }

    // Two more that are only their template, and both had their caption in the wrong
    // place because the template puts it relative to a **part**, which needs the
    // measured extents of §2:
    //
    // * `BottomBarButton.ctr` (22): `label` sits at `picture.size.width+data.space,8`
    //   — to the right of the `logoAn.fsh` icon the button carries as `startframe`,
    //   by the block's own `space`, and eight pixels down. `bg` is `0x00000000`, so
    //   there is no bar behind it;
    // * `Graphic Tab Button.ctr` (5): `label` is
    //   `((selectedbg.size.width-label.size.width)/2)+data.textoffsetx,data.textoffsety`
    //   — centred over the tab's own highlight art and then nudged by the two offsets
    //   the block carries (`textoffsetx` is 24, 3, 0, −1 and −22 on the five shipped tabs).
    pub(super) fn draw_bar_or_graphic_tab(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, aspect, strings, selected, hovered, .. } = d;
        let state = self.state_for(e, i, selected, hovered);
        let frame = (e.class == "NFS5.BOTTOMBARBUTTON").then(|| self.bar_button_frame(e, hovered == Some(i), self.pressed_on(i, hovered))).flatten();
        let first = out.len();
        self.template_widget_at(out, aspect, e, state, x, y, strings, frame);
        if e.class != "NFS5.BOTTOMBARBUTTON" {
            return;
        }
        // **`NFS5.BOTTOMBARBUTTON`'s draw 0x4C9B30 is a depth mask with a hover glint in it**
        // (read 2026-09-27): the icon (`[+0x178]`, 0x4C9C1A) first, outside it; 0x4DAA10
        // (0x4C9C22); the caption — the part of the state, `[+0x174]` disabled, `[+0x170]`
        // hovered, `[+0x16C]` otherwise, painted through its vtable +0x14 (0x4C9C5A) — as the
        // stencil; 0x4DAAC0 (0x4C9C5D); then, only while the button is hovered (0x4C7C90), not
        // disabled (0x4C7E40), not hidden (0x4C7ED0), at rest (0x4C8820 = 0) and 0x4E35C0 false,
        // the `weeicons.fsh` `GLNT` entry the bind looked up (0x4C95E4…0x4C95FE → `[+0x1DC]`) at
        // x = the ticks since the sweep started, y = the button's own, in 0x4D87D0 of the fade
        // (0x4C9D40…0x4C9D65); 0x4DAB00. Its timer (`[+0x276]` running, `[+0x278]` stamp,
        // `[+0x27C]` wait) is the car logo's: a wait of `rand() % 128` (0x4C9D1A, the bind's
        // 0x4C9616), a run of 640 ticks (0x4E3550) — `glints::Sweep` with `logo` — and it only
        // moves while those gates pass. So the sweep shows on the caption's letters alone.
        // 0x4C9616 is this class's own INUSE activate (its slot list carries 21, `controls.md`
        // §1350, the same slot the other four `activate_glints` rows answer), so *within its own
        // screen* it belongs in that same widget-order pass, not on the first hovered draw —
        // `LayoutScreen::activate_glints` now makes the four `NFS5.BOTTOMBARBUTTON` sweeps there
        // (`Sweep::new(0, true)`, the car logo's convention) instead of lazily on first hover.
        // *(Tried and reverted 2026-09-28, moving only this one call: the bottom bar was still a
        // second, known-late offender then — `main.rs` built it after the target screen's own
        // load (`bar_oval.rs`'s marker) — so the four buttons' activates still landed after the
        // target screen's, and every later screen's `crt_rand()` draws shifted by four calls that
        // had not shifted before (48/97 goldens differed). Fixed together with that marker:
        // `open_layout` and the `--layout` start-up path now build the bar — and so bind these
        // four sweeps — before loading the opened screen, matching 0x4D3420's own order (the bar's
        // constructor 0x4D7E70 at 0x4D39C4, the first layout 0x4D67D0 only after).)*
        let rects = self.state_rects(e, state).len();
        let shapes = self.shape_parts_at(e, state, frame).into_iter().filter(|(_, _, f, n, _)| self.shapes.contains_key(&(f.clone(), *n))).count();
        let mask = super::fe_depth::mask_begin();
        let caption = (first + rects + shapes).min(out.len());
        super::fe_depth::apply(&mut out[caption..], mask.stencil);
        if hovered == Some(i) && state != "disabled" && self.widget_progress() * 255 / TRANS_FULL == 0 {
            let glint = out.len();
            self.sweep_glint(i, true, out, aspect, y, 1.0);
            super::fe_depth::apply(&mut out[glint..], mask.inside);
        }
    }

    // The same shape three more times, 19 widgets: a `0xFFBC0000` box inside four
    // one-pixel black bars, with a white caption. `Tab Button.ctr` puts its
    // `LOCALIZEDTEXT` at `5,2` in `Universc11b.ffn` and swaps `bg` for `hoverbg`
    // (`0xffffbb00`) when it is the open tab; `CameraSelect.ctr` centres a `TEXT`
    // part in `Univers7b.ffn` — the camera number the block carries as `text` — and
    // `CameraPicture.ctr` adds `optart.fsh` entry `data.picturenum` over the box.
    pub(super) fn draw_tab_or_camera(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, talpha, aspect, strings, selected, hovered, .. } = d;
        let state = self.state_for(e, i, selected, hovered);
        // **`NFS5.CAMERAPICTURE` draws a part no state lists.** Its bind 0x4BF81E
        // looks `"shp"` up by name (0x525620, 0x525600) and keeps its art in
        // `[this+0x14C]` (0x4BF849), and the draw puts it at the widget's own place
        // through 0x4D9B30 (0x4BFA9B) **before** it runs the template's parts
        // (0x4BFAA6). `CameraPicture.ctr` gives `shp` `shapePathName=optart.fsh` and
        // `shapeIndex=data.picturenum`, and it is in none of the six state lists — so
        // the part walk never reaches it and only this arm can. `cameras.lay`'s four
        // widgets carry `picturenum` 1…4. §4.129.
        if e.class == "NFS5.CAMERAPICTURE"
            && let Some(n) = self.attr_num(e, "picturenum")
            && let Some(&(tex, aw, ah)) = self.shapes.get(&(CAMERA_PICTURES.to_string(), n.max(0.0) as usize))
        {
            let (pos, size) = self.place(aspect, x, y, aw, ah);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        self.template_widget(out, aspect, e, state, x, y, strings);
        if e.class == "NFS5.TABBUTTON" {
            self.glinty_box(out, aspect, e, i, x, y, selected, hovered, tint);
        }
        // The tab buttons' draw 0x4D0350 draws the parts first (0x4D036D) and then,
        // **when `data.dropshadow` is set** (`[this+0x144]`, bound at 0x4D00E0), the
        // bevel 0x521050 over its own box with a thickness of 3 — that in the steady
        // state, 0x4D0440; a tab that is still fading gets 0x520F00 with (4, 4)
        // instead, at 0x4D048D, which this port has no transition state to reach.
        // `Tab Button.ctr` carries `dropshadow.value=1` `HIDDEN|READONLY`, so all 11
        // get it; `Graphic Tab Button.ctr` carries 0, so none of its 5 do.
        //
        // The two camera classes have draws of their own and both end the same way,
        // with the bevel and **no** `dropshadow` to gate it — only the two visibility
        // tests every caller makes. `NFS5.CAMERASELECT` (0x4BF610) gives it the
        // widget's own box (0x4BF69D…0x4BF6CA); `NFS5.CAMERAPICTURE` (0x4BFA20) gives
        // it a fixed **15 x 13**, the two float constants 0x41700000 and 0x41500000
        // pushed at 0x4BFAC8 and 0x4BFACD.
        // **A tab button at rest gets the drop shadow, not nothing.** 0x4D03D6 gates
        // the whole tail on `data.dropshadow`; then 0x4D03E6 asks 0x4C7CB0 — hovered
        // with the button held (§4.55) — and 0x4D03F1 asks 0x4C7E40, which is
        // `isDisabled` and **not** "transitioning" as this comment used to say. The
        // pressed, enabled tab gets the bevel at 0x4D0440; every other tab gets
        // 0x520F00 with (4, 4) at 0x4D044A. `Tab Button.ctr` carries
        // `dropshadow.value=1 HIDDEN|READONLY`, so all 11 have one.
        if e.class == "NFS5.TABBUTTON"
            && self.attr_num(e, "dropshadow").is_some_and(|v| v != 0.0)
            && !self.pressed_on(i, hovered)
        {
            self.drop_shadow(out, aspect, x, y, w, h, 4.0, 4.0);
        }
        // All three end the same way: pressed takes §4.21's bevel, anything else
        // takes the inset highlight 0x521810 over the same box — 0x4D04A8 for a tab
        // (still inside `data.dropshadow`'s own gate at 0x4D03D6), 0x4BF711 for
        // `NFS5.CAMERASELECT` and 0x4BFB0B for `NFS5.CAMERAPICTURE`, whose box is the
        // fixed 15 x 13 either way. §4.75.
        let down = self.pressed_on(i, hovered);
        match e.class.as_str() {
            "NFS5.TABBUTTON" if self.attr_num(e, "dropshadow").is_some_and(|v| v != 0.0) => {
                if down {
                    self.bevel(out, aspect, x, y, w, h, 3.0);
                } else {
                    self.glass(out, aspect, x, y, w, h, &tint);
                }
            }
            "NFS5.CAMERASELECT" => {
                if down {
                    self.bevel(out, aspect, x, y, w, h, 3.0);
                } else {
                    self.glass(out, aspect, x, y, w, h, &tint);
                }
            }
            "NFS5.CAMERAPICTURE" => {
                if down {
                    self.bevel(out, aspect, x, y, 15.0, 13.0, 3.0);
                } else {
                    self.glass(out, aspect, x, y, 15.0, 13.0, &tint);
                }
            }
            _ => {}
        }
    }
}

impl LayoutScreen {
    // **`NFS5.ANIMSHOWCASEPICTURE`** cycles the four pictures of `show.fsh`, each
    // beside its own label. `FEAnimShowcasePicture`'s bind 0x4BDB90 allocates two
    // four-entry arrays — `[this+0x140]` from entries **0…3** and `[this+0x144]` from
    // **4…7** of `"%sshow.fsh"` (0x4BDBC5) — and the draw 0x4BDDC0 runs a two-state
    // machine off `TIMER_getTicks` (0x53C2E0):
    //
    // ```text
    // 0x4BDFDC  resting ([+0x150] == 0): when now - [+0x154] passes [+0x158],
    //           [+0x14C] = ([+0x148] + 1) % 4, [+0x150] = 1, [+0x158] = 0x40
    // 0x4BE059  and it draws [+0x140][[+0x148]] at full alpha
    // 0x4BDE17  fading: t = (now - [+0x154]) * 255 / [+0x158]
    // 0x4BDE97  [+0x140][[+0x14C]] at alpha t, [+0x140][[+0x148]] at 255 - t
    // 0x4BDF94  when it passes, [+0x148] = [+0x14C], [+0x150] = 0, [+0x158] = 0x80
    // ```
    //
    // so a frame rests for **0x80 = 128 ticks** and cross-fades for **0x40 = 64** —
    // one second and a half at the front end's 128 Hz. The label goes at
    // `x + picture.width` (0x4BDF72 adds the entry's `+0x14`), the same side-by-side
    // pair `NFS5.CarLogo` draws. §4.127.
    pub(super) fn draw_anim_showcase_picture(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, talpha, aspect, strings, .. } = d;
        // **A depth mask** (read 2026-09-27): 0x4DAA10 (0x4BDE12) before the pictures, which are
        // its stencil (0x4BDE97…0x4BE0A8, white at the fade and the cross-fade's alpha), then
        // 0x4DAAC0 (0x4BE0B0) and the widget's text runs (below), 0x4DAB00 (0x4BE10C).
        let mask = super::fe_depth::mask_begin();
        let stencil = out.len();
        const REST: i64 = 0x80;
        const FADE: i64 = 0x40;
        let ticks = (self.clock_ms as u64 * 128 / 1000) as i64;
        let phase = ticks.rem_euclid(REST + FADE);
        let frame = (ticks.div_euclid(REST + FADE)).rem_euclid(4) as usize;
        let draw = |out: &mut Vec<Sprite>, n: usize, a: f32| {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(SHOWCASE_ANIM.to_string(), n)) else { return };
            let Some(&(lt, lw, lh)) = self.shapes.get(&(SHOWCASE_ANIM.to_string(), n + 4)) else { return };
            for (t, dx, w2, h2) in [(tex, 0.0, aw, ah), (lt, aw, lw, lh)] {
                let (pos, size) = self.place(aspect, x + dx, y, w2, h2);
                out.push(Sprite { texture: t, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, a * talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
            }
        };
        if phase < REST {
            draw(out, frame, 1.0);
        } else {
            let t = (phase - REST) as f32 * 255.0 / FADE as f32 / 255.0;
            draw(out, frame, 1.0 - t);
            draw(out, (frame + 1) % 4, t);
        }
        super::fe_depth::apply(&mut out[stencil..], mask.stencil);
        // Inside the mask, the `[+0x15C]` = **five** text runs of 0x5208E0 — the same objects as
        // `NFS5.MOVIECIRCLE`'s "NEED FOR SPEED" runs (`movie_overlay`), built by the bind over the
        // box `{200, 0, 400, 200}` (0x4BDCE2…0x4BDD2B) in `Univers7b` / `Univers11b` (0x4BDD48,
        // 0x4BDD5E; `[+0x1CC]` = 2) — each spelling **the widget's `label` string**, which the bind
        // copies to `[+0x164]` (0x4C8B60 `"label"`, 0x4E7310, 0x4BDCC4…0x4BDCEA), in
        // `0x4D87F0(0xA0FFFFFF, fade)` (0x4BE0B6): so the caption drifts across the pictures, cut
        // to their opaque texels. Not drawn by the port before. APPROXIMATIONS as the circle's
        // runs: a generator of their own (seed 1) and 16 ms steps.
        let text = e.string_id().filter(|n| *n != 0).and_then(|n| strings.get(n)).unwrap_or_default().to_string();
        if !text.is_empty() {
            let b = crate::movie_overlay::Box4 { left: 200, top: 0, right: 400, bottom: 200 };
            let now = self.clock_ms as i64;
            let draws = {
                let mut map = self.movie_overlays.borrow_mut();
                let o = map.entry(i).or_insert_with(|| crate::movie_overlay::MovieOverlay::runs_only(b, 5, 0, 1));
                o.advance(now);
                o.draws()
            };
            let inside = out.len();
            for d in &draws {
                self.draw_run(out, aspect, d, &text, [1.0, 1.0, 1.0, 0xA0 as f32 / 255.0]);
            }
            super::fe_depth::apply(&mut out[inside..], mask.inside);
        }
    }

    // `NFS5.ChroniclePicture` (0x508310) lays the grid out at the widget's position, each
    // tile at **its own size** and clipped to the widget's box by 0x4D9E70, offset by the
    // pan the drag keeps at `+0x14C` / `+0x150` (clamped, `chronicle_pan`). The port
    // composes the grid into one texture (`set_chronicle_picture`) and clips that. The
    // box is 458×286 on both screens, and so is the picture in 140 of the 183 packs — but
    // 43 are bigger (`MA601a.fsh` is five rows, 458×650; `MC951a.fsh` 626 wide), and those
    // the original shows a box-sized window of and lets the player drag, where this port
    // used to squeeze the whole picture into the box.
    //
    // 0x4D9E70 is the front end's clipped blit (ten callers): `(entry, x, y, w, h, clip
    // l, t, r, b, colour, flip, flag)`. Per axis it cuts what lies past the far edge
    // (`x + entry.w > r` → visible `r − x`, 0x4D9EAB…0x4D9ECE) and before the near one
    // (`x < l` → cut `l − x`, `x = l`, 0x4D9ED6…0x4D9EFA), rejects the tile when the
    // visible extent is negative or the cut exceeds `w` (0x4D9F5D…0x4D9FAD), and takes the
    // texture coordinates in the same proportions (`u0 + du·cut/w`, `u0 + du·visible/w`,
    // 0x4DA14D…0x4DA1FE). Unlike 0x4D9B30 it applies no 1/512 inset. The colour is the
    // widget's tint (0x528C90, opaque white on both screens) with the transition's fade
    // (0x4C8840) folded in by 0x4D87F0 at 0x508361.
    pub(super) fn draw_chronicle_or_showcase_picture(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, talpha, aspect, hovered, .. } = d;
        let Some(key) = self.logos.get(&i) else { return };
        if e.class == "NFS5.ChroniclePicture" {
            let Some(&(tex, _, _)) = self.shapes.get(key) else { return };
            let Some((px, py, pw, ph)) = self.chronicle_pan(i) else { return };
            // the picture moves with the transition (0x4C8760 on the position, 0x50834A), the
            // clip rectangle does not (0x4C8670)
            let (cx, cy) = self.position_of(e);
            let (l, t) = (x - px, y - py);
            let (x0, x1) = (l.max(cx), (l + pw).min(cx + w));
            let (y0, y1) = (t.max(cy), (t + ph).min(cy + h));
            if x1 <= x0 || y1 <= y0 {
                return;
            }
            let (pos, size) = self.place(aspect, x0, y0, x1 - x0, y1 - y0);
            out.push(Sprite {
                texture: tex,
                pos,
                size,
                uv_min: [(x0 - l) / pw, (y0 - t) / ph],
                uv_max: [(x1 - l) / pw, (y1 - t) / ph],
                color: [1.0, 1.0, 1.0, talpha],
                rotation: 0.0,
                pivot: [0.5, 0.5],
                tri: None,
                depth: None,
            });
            return;
        }
        // `NFS5.ChroniclePictureIcon` keeps **two** entries, 0 and 1, and 0x508819
        // picks between them on 0x4C7C90 — `[this+0x10C] & 4`, which is bit 2 of the
        // state mask, **hover** (§4.109). Entry 1 is therefore the hovered frame, not
        // "the entry the reader is on"; the host binds entry 0 and the next one is
        // taken here, exactly as `NFS5.CarLogo`'s pair is.
        let key = &if e.class == "NFS5.ChroniclePictureIcon" && hovered == Some(i) && self.shapes.contains_key(&(key.0.clone(), key.1 + 1)) {
            (key.0.clone(), key.1 + 1)
        } else {
            key.clone()
        };
        let Some(&(tex, aw, ah)) = self.shapes.get(key) else { return };
        let (dw, dh) = (if w > 0.0 { w } else { aw }, if h > 0.0 { h } else { ah });
        let (pos, size) = self.place(aspect, x, y, dw, dh);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        // `NFS5.ChroniclePictureIcon`'s draw ends with the bevel over **the art's own
        // size** — 0x508906 and 0x508910 read `[entry+0x18]` and `[entry+0x14]`, the
        // height and width of the `.fsh` entry it just drew, not the widget's box —
        // at the widget's position, thickness 3, ungated (0x508927). Its two
        // neighbours here have draws of their own that never call the painter.
        // — and when it is *not* pressed, the **drop shadow** instead: 0x508931's arm
        // puts 0x520F00 at (4, 4) over that same art-sized box (0x508954), which is
        // the other half of a tail §4.23 read only the first half of. §4.78.
        if e.class == "NFS5.ChroniclePictureIcon" {
            if self.pressed_on(i, hovered) {
                self.bevel(out, aspect, x, y, aw, ah, 3.0);
            } else {
                self.drop_shadow(out, aspect, x, y, aw, ah, 4.0, 4.0);
            }
        }
    }

    pub(super) fn draw_car_logo(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, h, aspect, .. } = d;
        // `NFS5.CarLogo::draw` 0x4C1B70, CONFIRMED: the pair is drawn side by side —
        // the first entry at the anchor, the second immediately to its right — and the
        // whole thing is vertically centred in the widget's own box. `align` moves the
        // anchor the same way the text rule does: 0 keeps `pos`, 1 pulls back half the
        // combined width, 2 the full width (0x4C1BE9…0x4C1C22). A second entry only one
        // pixel high is a placeholder: the original zeroes its width and skips it
        // (0x4C1BDC). Nothing is drawn until the port has bound a car to the widget.
        let Some((file, base)) = self.logos.get(&i) else { return };
        let Some(&(ta, aw, ah)) = self.shapes.get(&(file.clone(), *base)) else { return };
        let b = self.shapes.get(&(file.clone(), base + 1)).copied();
        let (bw, bh, tb) = match b {
            Some((t, w, h)) if h != 1.0 => (w, h, Some(t)),
            _ => (0.0, 0.0, None),
        };
        let left = match e.int("align").unwrap_or(0) {
            1 => x - ((aw + bw) * 0.5).trunc(),
            2 => x - (aw + bw),
            _ => x,
        };
        let top = y + ((h - ah) * 0.5).trunc();
        // **A depth mask** (0x4C1BCB 0x4DAA10 … 0x4C1CE0 0x4DAAC0 … 0x4C1D6B / 0x4C1DB2 0x4DAB00):
        // the two logo entries are the stencil — drawn in `0x4D87D0(fade)`, white at the
        // widget's fade (0x4C1C7E, 0x4C1CAC), so they write depth wherever their art's alpha
        // passes the test — and the glint is drawn inside it: the sweep shows **on the logo
        // alone**, not across the screen.
        let mask = super::fe_depth::mask_begin();
        let stencil = out.len();
        let (pos, size) = self.place(aspect, left, top, aw, ah);
        out.push(Sprite { texture: ta, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        if let Some(tb) = tb {
            let (pos, size) = self.place(aspect, left + aw, top, bw, bh);
            out.push(Sprite { texture: tb, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        // The glint, 0x4C1CE0…0x4C1DAF, and it is the **same sweep the header has**
        // (§4.25): the `GLNT` entry of `weeicons.fsh` that the activate looks up by
        // tag at 0x4C13E3, at x = the elapsed tick count (0x4C1D35 stores it in the
        // local the sprite's first argument reads at 0x4C1DA1) and y = the widget's
        // own (0x4C1BBB). One difference: the colour goes through 0x4D87F0 a second
        // time with **0x40** (0x4C1D7E), so the logo's sweep is a quarter as strong as
        // the header's.
        let glint = out.len();
        super::fe_depth::apply(&mut out[stencil..glint], mask.stencil);
        self.sweep_glint(i, true, out, aspect, y, 0x40 as f32 / 255.0);
        super::fe_depth::apply(&mut out[glint..], mask.inside);
    }

    // `FEKnockoutTrackList::draw` (0x4FA190) is a **seven-slot** strip: the widget's box
    // is divided by the `1/7` at 0x5B7E78 (0x4FA241), and the first `n` slots — `n` from
    // the accessor's vtable `+0x6C`, the opponent count — are filled with the literal
    // `0x80FEDD81` the draw pushes at 0x4FA1FA, which is also what the `.lay` block's
    // own `color` says. The row `Knockout.TrackNumber` is on is redrawn in the menu
    // amber `0x4D87C0` gives (0x4FA296), and half-alpha black rules (`0x4D87E0` at
    // `0x80`) separate the rows and mark **four** columns, at `x + 0x0B`, `x + 0x71`,
    // `x + 0xA4` and `x + 0xCC` (0x4FA35B, 0x4FA39F, 0x4FA3D5, 0x4FA424), each the filled
    // height tall; last, the filled strip is outlined in full-fade black, 0x4D9200 of
    // `0x4D87E0(fade)` over `(x, y, w, n·h/7)` (0x4FA43A…0x4FA45A). *(Until 2026-09-27 the
    // port drew the first two columns and no outline.)* A `Knockout.TrackNumber` at or past
    // `n` is put back on row 0 by the draw itself (0x4FA27B…0x4FA28C) — the binding does it.
    pub(super) fn draw_knockout_track_list(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, x, y, w, h, talpha, aspect, .. } = d;
        let Some(&(rows, current)) = self.ko_strip.get(&i) else { return };
        if rows == 0 || w <= 0.0 || h <= 0.0 {
            return;
        }
        let slot = h / 7.0;
        let filled = slot * rows as f32;
        let half = talpha * 128.0 / 255.0;
        let fill = [0xFE as f32 / 255.0, 0xDD as f32 / 255.0, 0x81 as f32 / 255.0, half];
        out.push(self.quad(aspect, x, y, w, filled, fill));
        if current < rows {
            out.push(self.quad(aspect, x, y + slot * current as f32, w, slot, [1.0, 0xBB as f32 / 255.0, 0.0, talpha]));
        }
        let rule = [0.0, 0.0, 0.0, half];
        for r in 1..rows {
            out.push(self.quad(aspect, x, y + slot * r as f32, w, 1.0, rule));
        }
        for dx in [11.0, 113.0, 164.0, 204.0] {
            out.push(self.quad(aspect, x + dx, y, 1.0, filled, rule));
        }
        let black = [0.0, 0.0, 0.0, talpha];
        for (bx, by, bw, bh) in [(x, y, w, 1.0), (x, y + filled - 1.0, w, 1.0), (x, y, 1.0, filled), (x + w - 1.0, y, 1.0, filled)] {
            out.push(self.quad(aspect, bx, by, bw, bh, black));
        }
    }

    // `FEChronicleMainBoxControl::draw` (0x507990) starts with five bars, in this
    // order, all 454 wide from the widget box's own left edge and all but the outline
    // at half alpha (0x4D87F0 scales by the 0x80 pushed at 0x5079FB, 0x507A62,
    // 0x507AA7 and 0x507AEC):
    //
    // | y | size | what |
    // |---|---|---|
    // | +0x26 | 454×16 | white (0x4D87D0) |
    // | +0x26 | 455×16 | the outline of that bar, black (0x4D87E0 is `alpha << 24`), 0x4D9200 |
    // | +0x10 | 454×20 | 0xBC0000 (0x4D87B0) |
    // | +0x26 | 454×60 | 0xBC0000 |
    // | +0x64 | 454×60 | 0xBC0000 |
    //
    // Then the timeline over them. 0x507B4F walks the years from the accessor's
    // own (`(value − 0x76C) % 100`, so `+0x76C` is 1900), prints each as `"%d"` at
    // `x + col + 0x2D` (0x507BDF), outlines a 90×20 box per year (0x507C1A) and asks
    // 0x507340 for up to three chronicle entries a year to put icons in it — drawn at
    // the end of this method off `chronicle_columns` (§4.122). *(This block said "not
    // drawn … the port has no chronicle year index to walk" until 2026-09-21, ten lines
    // above the code that draws it.)*
    pub(super) fn draw_chronicle_main_box(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { e, x, y, talpha, aspect, .. } = d;
        let half = |c: [f32; 3]| [c[0], c[1], c[2], talpha * 128.0 / 255.0];
        let red = [0xBC as f32 / 255.0, 0.0, 0.0];
        out.push(self.quad(aspect, x, y + 38.0, 454.0, 16.0, half([1.0, 1.0, 1.0])));
        let black = [0.0, 0.0, 0.0, talpha];
        for (bx, by, bw, bh) in [(x, y + 38.0, 455.0, 1.0), (x, y + 38.0 + 15.0, 455.0, 1.0), (x, y + 38.0, 1.0, 16.0), (x + 454.0, y + 38.0, 1.0, 16.0)] {
            out.push(self.quad(aspect, bx, by, bw, bh, black));
        }
        out.push(self.quad(aspect, x, y + 16.0, 454.0, 20.0, half(red)));
        out.push(self.quad(aspect, x, y + 38.0, 454.0, 60.0, half(red)));
        out.push(self.quad(aspect, x, y + 100.0, 454.0, 60.0, half(red)));
        // **and the timeline over them**, 0x507B4F. One cell per column, 90 wide at a
        // pitch of 91 and 61 apart down from `+0x26` — the two numbers §4.120's hit
        // test divides by. The year goes in centred (`push 1`, the align, at
        // 0x507BCA) at `x + col + 0x2D` over 0x4DA530 (0x507BF3), and the 90x20
        // outline goes 0x10 below it through 0x4D9200 (0x507C09, 0x507C15 pushes
        // 20.0 and 0x507C1A 90.0). §4.122.
        if let Some(f) = self.font_for(e) {
            let hot = self.cursor.and_then(|(px, py)| chronicle_cell(px, py, x, y));
            for (col, (year, _)) in self.chronicle_columns.iter().take(5).enumerate() {
                let cx = x + col as f32 * 91.0;
                // the year, centred in its column, and the 90x20 cell 0x10 under it
                // — which lands on the 454x20 bar the draw put at `+0x10` (§4.123)
                self.text(out, aspect, f, &format!("{}", 1900 + year), cx + 45.0, y + 2.0, 0.0, 1, black);
                for (bx, by, bw, bh) in [(cx, y + 16.0, 90.0, 1.0), (cx, y + 35.0, 90.0, 1.0), (cx, y + 16.0, 1.0, 20.0), (cx + 89.0, y + 16.0, 1.0, 20.0)] {
                    out.push(self.quad(aspect, bx, by, bw, bh, black));
                }
                // and **two** icon cells of 90 x 60, 0x3E apart (0x507DD8), which is
                // where the two 454x60 bars are; the icon goes in at (+1, +1)
                for r in 0..2 {
                    let by = y + 38.0 + r as f32 * 62.0;
                    let on = hot == Some((col, r));
                    if let Some((rest, over)) = self.chronicle_icons.1.get(&(col, r))
                        && let Some(&(tex, aw, ah)) = self.shapes.get(&(self.chronicle_icons.0.clone(), if on { *over } else { *rest }))
                    {
                        let (pos, size) = self.place(aspect, cx + 1.0, by + 1.0, aw, ah);
                        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
                    }
                    for (bx, by, bw, bh) in [(cx, by, 90.0, 1.0), (cx, by + 59.0, 90.0, 1.0), (cx, by, 1.0, 60.0), (cx + 89.0, by, 1.0, 60.0)] {
                        out.push(self.quad(aspect, bx, by, bw, bh, black));
                    }
                }
            }
        }
    }

    // `NFS5.AnimatingPicture`'s own tick, **0x4BD8F0** (§4.53): a frame out of the run
    // its bind read — `data.start_frame` into `[+0x144]`, `data.end_frame` into
    // `[+0x148]`, `data.numticks` into `[+0x154]` (forced to 1 when it is 0) and
    // `data.mindelay` / `data.maxdelay` into `[+0x15C]` / `[+0x160]`.
    //
    // The class alternates between waiting and playing. Idle, it starts once the tick
    // passes `[+0x150] + [+0x158]`, the delay it is sitting out; playing, it runs until
    // the tick passes `[+0x150] + [+0x154]` and then draws a new delay,
    // `mindelay + rand() % (maxdelay - mindelay)` (0x4BD981), and goes back to idle.
    // While it plays the frame is
    // `start + (tick - started) · (end - start) / numticks` clamped to the pair
    // (0x4BD9DD…0x4BDA1E).
    //
    // **All five shipped ones are the same**: `logoAn.fsh`, frames 0…29 over 256 ticks
    // — two seconds at 128 Hz — with `mindelay` and `maxdelay` both 0, so there is no
    // wait at all and the run simply repeats. That is what the port draws, and it is
    // the whole of the class's shipped use.
    pub(super) fn draw_animating_picture(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, aspect, .. } = d;
        let Some(file) = e.get("shapePathName").filter(|f| !f.is_empty()) else { return };
        let a = e.int("start_frame").unwrap_or(0).max(0) as usize;
        let b = e.int("end_frame").unwrap_or(0).max(0) as usize;
        // the widget's own clock (`glints::Anim`, bind 0x4BD8BE, tick 0x4BD8F0): its wait
        // `mindelay + rand() % (maxdelay − mindelay)` when `maxdelay > mindelay`, the run until the
        // tick passes `numticks`, and with `maxdelay` 0 the run simply starts again
        let n = e.int("numticks").filter(|v| *v > 0).unwrap_or(1);
        let lo = e.int("mindelay").unwrap_or(0);
        let hi = e.int("maxdelay").unwrap_or(0);
        let now = self.ticks() as i64;
        let frame = self.step_glint(
            i,
            now,
            || crate::layout::glints::Glint::Anim(crate::layout::glints::Anim::new(0, lo, hi)),
            |g, t, _| match g {
                crate::layout::glints::Glint::Anim(an) => an.step(t, a as i64, b as i64, n, lo, hi),
                _ => a as i64,
            },
        )
        .max(0) as usize;
        if let Some(&(tex, aw, ah)) = self.shapes.get(&(file.to_string(), frame)) {
            let (dw, dh) = (if w > 0.0 { w } else { aw }, if h > 0.0 { h } else { ah });
            let (pos, size) = self.place(aspect, x, y, dw, dh);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: tint([255, 255, 255, 255]), rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
    }

    // **None of these four is a registered class.** The only `*Rectangle*` string in
    // the whole executable is `NFS5.GlintyRectangle` (0x5DD847) — there is no
    // `NFS5.Rectangle` and no `NFS5.OutlineRectangle`, and no `NFS5.GLINTYLINE`
    // registration either beyond 0x5222E0's, whose class does nothing but the parts.
    // CONFIRMED by exhaustion: they are drawn by the template and nothing else.
    //
    // Which settles `NFS5.OutlineRectangle`, and the port had it **backwards**:
    // `OutlineRectangle.ctr`'s `area` is the whole box with `color.value=0x8000ff00`
    // and **no flags**, so the block's `color` fills the interior, while `top`,
    // `bottom`, `left` and `right` are one-pixel bars at `0xff000000`
    // `HIDDEN|READONLY` — always **black**. The port was drawing four hairlines in
    // the widget's colour and no fill. All 195 shipped ones carry a colour, and they
    // are fills: 82 a 30 % white wash, 31 fully transparent, 16 half grey, 12 amber,
    // 11 the dark red. `NFS5.BEVELEDOPTIONBOX` is the same shape.
    // The same exhaustion settles four more, 811 widgets between them: neither
    // `REAL.LABEL` (306), `NFS5.PICTURE` (484), `NFS5.NonLocalizedLabel` (11) nor
    // `NFS5.ExtraCircle` (10) is a registered class, so each is its template and
    // nothing else.
    //
    // `Label.ctr` is one `LOCALIZEDTEXT` part at
    // **`pos.value=(0-label.size.width)/2*data.align,0`** in `univers8b.ffn` and
    // `0xffffffff`, all three overridable — and that expression *is* the alignment
    // rule, written out: `align` 0 keeps the position, 1 pulls back half the string's
    // width, 2 the whole width. This port used to reproduce it by anchoring in a box,
    // which only agreed because the box came out zero; now it is the template's own
    // arithmetic over the measured extent. All 306 carry `stringID`, `align` and
    // `foreColor`; all 11 non-localised labels carry `text` and `foreColor`.
    // `NFS5.HEADER` (41) and `NFS5.SUBHEADER` (5) are their template's one
    // `LOCALIZEDTEXT` part as well: `Header.ctr` puts the class at `pos.value=6,4` and
    // the label at `0,0` in `Univers17b.ffn` and `0xffffffff`, and the class's own
    // slot 20 (0x4CAE60) does nothing but bind that part. Its slot 21 (0x4CADC0) loads
    // a glint — see `RE/notes/controls.md` §4.11 — which is not drawn here.
    //
    // Returns whether the widget is **erased** rather than undrawn (the two cases at the end), so
    // the screen loop does not report it as a gap. `first` is `out`'s length on entry — the same
    // value the loop takes before the class match, since nothing is pushed in between.
    pub(super) fn draw_template_classes(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) -> bool {
        let WidgetDraw { i, e, x, y, w, h, aspect, strings, selected, hovered, .. } = d;
        let first = out.len();
        let mut closed = false;
        let state = self.state_for(e, i, selected, hovered);
        // **`NFS5.HEADER`'s draw 0x4CAEA0 is a depth mask**: 0x4DAA10 (0x4CAEB6) before its text
        // (0x4DA350 at 0x4CAFD8, or the spaced run 0x522050 at 0x4CB007 while it slides in), so
        // the letters are the stencil, written wherever their glyphs' alpha passes the test; then
        // — only at rest, the fade 0 (0x4CB01C) — 0x4DAAC0 (0x4CB024) and the glint, which lands
        // **on the letters alone**; 0x4DAB00 (0x4CB0B4 / 0x4CB0F9). Off rest the draw leaves by
        // 0x4DA9D0 without the second half (0x4CB0FE), the depth stepped all the same.
        let header = (e.class == "NFS5.HEADER").then(super::fe_depth::mask_begin);
        self.template_widget(out, aspect, e, state, x, y, strings);
        // The header's glint, **0x4CB024…0x4CB0F6** — its colour is 0x4D87D0 of
        // the fade, so white at full (§4.25).
        if let Some(m) = header {
            super::fe_depth::apply(&mut out[first..], m.stencil);
            let glint = out.len();
            self.sweep_glint(i, false, out, aspect, y, 1.0);
            super::fe_depth::apply(&mut out[glint..], m.inside);
        }
        // `NFS5.GLINTYLINE`'s sweep, on top of the bar its template just drew
        // (§4.28, §4.29). The long axis is the one the sweep runs along: `x` when
        // `data.horizontal` is set — the class's own test is `!= 0`, so its 2 counts
        // as set — and `y` when it is not.
        if e.class == "NFS5.GLINTYLINE" {
            // The sweep's own timer and draws (`glints::Line`, 0x51FB0A…0x51FC24): a run of
            // `rand() % 3 + 3` pixels a tick in a coin's direction, from one width outside one end
            // to one width past the other, then `rand() % 128` ticks of rest.
            let horizontal = self.attr_num(e, "horizontal").unwrap_or(1.0) != 0.0;
            let len = if horizontal { w } else { h };
            let now = self.ticks() as i64;
            let run = self.step_glint(
                i,
                now,
                || {
                    let short = if horizontal { h } else { w };
                    crate::layout::glints::Glint::Line(crate::layout::glints::Line::new(0, short as i32, len as i64))
                },
                |g, t, _| match g {
                    crate::layout::glints::Glint::Line(l) => l.step(t).map(|p| (p, l.width, l.dir)),
                    _ => None,
                },
            );
            if let Some((along, width, dir)) = run {
                if width >= 1
                    && let Some((s, l, c0, c1)) = glint_sweep(len, along as f32, width as f32, dir == 1)
                {
                    let (a, b) = (tint(c0), tint(c1));
                    let (px, py, pw, ph) = if horizontal { (x + s, y, l, 1.0) } else { (x, y + s, 1.0, l) };
                    let pts = [[px, py], [px + pw, py], [px, py + ph], [px + pw, py + ph]];
                    let cols = if horizontal { [a, b, a, b] } else { [a, a, b, b] };
                    self.gouraud(out, aspect, pts, cols);
                }
            }
        }
        // `NFS5.BEVELEDOPTIONBOX` is the one class whose draw is the vtable's own
        // 0x520E50: the template's parts (0x4C8260 at 0x520E5E) and then the shared
        // bevel 0x521050 with a thickness of **3** (0x520EBD), over the widget's box
        // (0x4C8670) in its fade colour. So it is its amber fill and black frame with
        // a sunken bevel inside them, not the fill alone.
        if e.class == "NFS5.BEVELEDOPTIONBOX" {
            self.bevel(out, aspect, x, y, w, h, 3.0);
        }
        // Erased, not undrawn, in two cases the original agrees with:
        //
        // * a box whose every rectangle is **fully transparent** — four of the 362
        //   `NFS5.Rectangle`s carry an alpha of zero;
        // * a label whose `stringID` the shipped table carries **empty** — 12300 and
        //   19809, the two `REAL.LABEL`s of `RE/AUDIT.md` F.
        if out.len() == first
            && let Some(c) = self.controls.get(&e.class)
        {
            let parts = c.state_parts(state);
            let rects = parts.iter().filter_map(|n| c.part(n)).any(|p| p.kind() == nfs_formats::ctr::PartKind::Rect);
            let blank_string = e.string_id().is_some_and(|id| strings.get(id) == Some(""));
            if rects || blank_string {
                closed = true;
            }
        }
        closed
    }
}

impl LayoutScreen {
    // the tab strip this group embeds, drawn at the group's own place. Its `pos` is
    // 0,0 on all four screens that use one, so the strip lands in the screen's own
    // coordinates.
    pub(super) fn draw_radio_group(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>, blank: &mut Vec<(String, String)>) {
        let WidgetDraw { i, x, y, aspect, strings, accessor, .. } = d;
        let Some(g) = self.groups.get(&i) else { return };
        out.extend(self.embedded(g, aspect, strings, accessor, blank, x, y));
    }

    // `FETrackSmall2dSpline::draw` (0x4FCD80). The widget cycles through the events of
    // the tournament its accessor names: `update` (0x4FCB50) divides the milliseconds
    // since the list was built by 256 for the index and restarts when that reaches the
    // count, and keeps a second counter for the fade — `m = elapsed % 256`, `m*8` while
    // `m < 0x20`, `0x7FF - m*8` while `m > 0xE0`, untouched in between, so it holds at
    // 0xF8 for the middle of each step (0x4FCB5D…0x4FCB91).
    //
    // Each step draws three things: the map, in amber (`0x4D87C0` is
    // `(alpha << 24) | 0xFFBB00`) at the fade's alpha; **one** marker, in white
    // (`0x4D87D0`, `0xFFFFFF`) at the widget's own alpha; and beside them the whole
    // event list, `"-"` at x=352 and the track's name at x=356, 15 pixels apart from
    // y=280, the current one amber and the rest white (0x4FCF20…0x4FCFE9). The map and
    // the marker take their positions from the `.fsh` entry, not from the `.lay`.
    pub(super) fn draw_track_small_spline(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, talpha, aspect, .. } = d;
        let Some(evs) = self.track_maps.get(&i).filter(|e| !e.is_empty()) else { return };
        let cur = (self.clock_ms / 256) as usize % evs.len();
        let m = self.clock_ms % 256;
        let ramp = if m < 0x20 {
            m * 8
        } else if m > 0xE0 {
            0x7FF - m * 8
        } else {
            0xF8
        };
        let ev = &evs[cur];
        let picture = |out: &mut Vec<Sprite>, art: Option<(TextureId, f32, f32, f32, f32)>, color: [f32; 4]| {
            let Some((tex, aw, ah, ax, ay)) = art else { return };
            let (pos, size) = self.place(aspect, ax, ay, aw, ah);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        };
        picture(out, ev.map, [1.0, 0xBB as f32 / 255.0, 0.0, talpha * ramp as f32 / 255.0]);
        picture(out, ev.marker, [1.0, 1.0, 1.0, talpha]);
        if let Some(f) = self.font_for(e) {
            for (row, ev) in evs.iter().enumerate() {
                let ry = 280.0 + 15.0 * row as f32;
                let c = if row == cur { [1.0, 0xBB as f32 / 255.0, 0.0, talpha] } else { [1.0, 1.0, 1.0, talpha] };
                self.text(out, aspect, f, "-", 352.0, ry, 0.0, 0, c);
                self.text(out, aspect, f, &ev.name, 356.0, ry, 0.0, 0, c);
            }
        }
    }

    // `NFS5.PRIZERACEMAP` and `NFS5.FACTORYMAP` share one draw (0x51A360, both vtables
    // at +0x3C): entry 0 of the pack the widget loaded, at the widget's own place and
    // the art's own size, in white at the transition alpha. Which pack that is comes
    // from the setter — the era for the prize map (0x506430: `mclass`, `mgold` or
    // `mmod` by the current tournament's `+4`, 0x504800) and the mission code at
    // `fac+0x3F` for the factory map (0x51A310, the first mission whose `+0xE8` is 0).
    pub(super) fn draw_race_or_factory_map(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, x, y, talpha, aspect, .. } = d;
        let Some(key) = self.logos.get(&i) else { return };
        let Some(&(tex, aw, ah)) = self.shapes.get(key) else { return };
        let (pos, size) = self.place(aspect, x, y, aw, ah);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
    }

    // a widget that is only a place, with its picture put there from code: one entry
    // of a `.fsh` pack — see [`set_pack_picture`](Self::set_pack_picture)
    // The player's portrait and the Factory Driver licence card. `NFS5.PLAYERPICTURE`
    // (0x4CFC00) and `NFS5.BADGE` (0x4CF3E0) put their art down at the widget box's
    // top-left corner at the art's own size; the two `MINI` classes (0x4CFC90,
    // 0x4CF610) call the blit's eight-argument form (0x4D9B30) with the box's own
    // width and height instead, so they stretch. All four take the widget's box from
    // 0x4C8670, which answers `(x1, y1, x2, y2)`.
    // `NFS5.FACTORYPIC` (0x51A210) joins them: one face from `FactoryPeople.fsh` at the
    // widget's own place and the art's size.
    // `NFS5.HUD` (0x4CB970) joins them for its first piece: entry 0 of `hud.fsh`, the
    // 240×180 `disp` schematic, at the widget box's top-left at its own size.
    pub(super) fn draw_pack_pictures(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, talpha, aspect, .. } = d;
        let Some(key) = self.logos.get(&i) else { return };
        let Some(&(tex, aw, ah)) = self.shapes.get(key) else { return };
        let stretch = e.class.starts_with("NFS5.MINI");
        let (dw, dh) = if stretch && w > 0.0 && h > 0.0 { (w, h) } else { (aw, ah) };
        let (pos, size) = self.place(aspect, x, y, dw, dh);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        let Some((name, rank, colour)) = self.badges.get(&i) else { return };
        // the shadow: entry 0 at x+244 and entry 1 at y+131 (0x4CF499, 0x4CF4C4). The
        // original also gates it on the machine (0x4E35D0: at least 0x2100000 bytes of
        // total physical memory, `GlobalMemoryStatus` at 0x53C3D0 — not a start-up
        // timer, as this said until 2026-09-26), on the low-end-card gate `[0x5D9D38]`
        // and on the `FENOUSELESSART` option `[0x657E60]`; a default install on a machine
        // with memory to spare passes all three, so the shadow is always drawn.
        for (entry, dx, dy) in [(0usize, 244.0, 0.0), (1, 0.0, 131.0)] {
            let Some(&(t, sw, sh)) = self.shapes.get(&(LICENSE_SHADOW.to_string(), entry)) else { continue };
            let (pos, size) = self.place(aspect, x + dx, y + dy, sw, sh);
            out.push(Sprite { texture: t, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        // the name, centred (the fourth argument of 0x4DA350 is the alignment: 1 pulls
        // back half the text's width, 2 the whole of it — 0x4DA3D7…0x4DA405) at
        // x+122, y+11 (0x4CF513, 0x4CF522), and the rank at x+98, y+50 in black
        // (0x4CF5C1, 0x4CF5C4, 0x4CF5D0)
        let mut c = *colour;
        c[3] *= talpha;
        if let Some(f) = self.fonts.get(BADGE_NAME_FONT).or_else(|| self.font_for(e)) {
            self.text(out, aspect, f, name, x + 122.0, y + 11.0, 0.0, 1, c);
        }
        if let Some(f) = self.fonts.get(BADGE_RANK_FONT).or_else(|| self.font_for(e)) {
            self.text(out, aspect, f, rank, x + 98.0, y + 50.0, 0.0, 0, [0.0, 0.0, 0.0, talpha]);
        }
    }

    // `NFS5.MOVIEBOX` (0x4CD640) hands the drawing to the player object at `+0x1BC`;
    // the picture goes at the widget's own box.
    // `NFS5.MOVIECIRCLE` plays one too, and its draw **0x4CDAE0** puts it in the same
    // place: it adds `[this+0x22C]` and `[this+0x230]` — the widget's own `pos`,
    // which its bind reads at 0x4CDA61 — to the screen offset, and hands the box to
    // the player at `[this+0x1C4]`. `RE/notes/controls.md` §4.33.
    pub(super) fn draw_movie(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, talpha, aspect, .. } = d;
        // **`NFS5.MOVIECIRCLE`'s draw 0x4CDAE0 is a depth mask** (`RE/notes/layouts.md` "Clip = a
        // depth mask"): 0x4DAA10 steps the kit's depth `[0x65B954]` nearer and the nine
        // `stencilshape` parts are painted there (vtable +0x14, 0x4CDB2C…0x4CDB57), writing depth
        // where the alpha test keeps them; 0x4DAAC0 flushes, sets `ZFUNC` `GREATEREQUAL` (Thrash
        // 0x28 ← 6 through 0x537830) and steps the depth 0.001 back; then the clip (0x4CDB61 →
        // 0x4CD640), the overlay and the credits roll (0x4CDB72 → 0x4CDF40) land only where the
        // parts wrote depth; 0x4DAB00 puts the compare back. The rest of the frame's 2D draws sit
        // at 0.99998 over a depth cleared to 1.0 (0x4DA890), both behind the mask — the port's
        // plain sprites write no depth, which leaves the 1.0 and gives the same test.
        // `NFS5.MOVIEBOX` has no parts and no mask.
        let circle = e.class == "NFS5.MOVIECIRCLE";
        // the kit's depth stepped for this mask from where the frame's earlier masks left it, and
        // the device's alpha test as it stands (`layout::fe_depth`: 10 as the frame's room left it,
        // 1 after a list box — the credits' first port took the device's initial 0)
        let mask = circle.then(super::fe_depth::mask_begin);
        let c = e.color("tint").map(tint).unwrap_or([1.0, 1.0, 1.0, 1.0]);
        if let Some(m) = mask {
            let start = out.len();
            self.draw_stencil_parts(e, x, y, aspect, c, out);
            super::fe_depth::apply(&mut out[start..], m.stencil);
        }
        let start = out.len();
        if let Some(m) = self.movies.get(&i) {
            // the box is the widget's `size` (0x4CD034…0x4CD05E into `[+0x14C]`, `[+0x150]`,
            // handed to the player's constructor 0x4E1EB0 as its `[+4]`, `[+8]`); a widget with
            // no size falls back on the clip's own (the port's; no shipped widget lacks one)
            let (dw, dh) = (if w > 0.0 { w } else { m.size.0 }, if h > 0.0 { h } else { m.size.1 });
            let v_max = movie_blit_v(dw, dh, m.size);
            let (pos, size) = self.place(aspect, x, y, dw, dh);
            // **`data.tint`** is the widget base's own colour, `[this+0x120]`, bound at
            // 0x4C7652 and read back by the getter 0x528C90 that ten draws call —
            // `NFS5.MOVIEBOX`'s among them, at 0x4CD644. Its default when the
            // attribute is absent is `0x4D87D0(0xFF)`, **opaque white** (0x4C7632), so
            // it changes nothing for the widgets that do not carry it. Six do, all of
            // them movie widgets, and five of those carry `0xFFFFFFFF` — the default
            // written out. **One** differs: `credits.lay`'s `ID_MOVIECIRCLE` at
            // `0xff55afbe`, a pale blue over the credits clip. §4.83.
            out.push(Sprite { texture: m.tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, v_max], color: [c[0], c[1], c[2], c[3] * talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        if let Some(m) = mask {
            // the overlay's lines, flashes and runs, then the credits roll last (0x4CE0CD)
            self.draw_movie_overlay(i, e, talpha, aspect, out);
            let credits = self.layout.elements.iter().any(|x| x.get("callbacklist").is_some_and(|c| c.eq_ignore_ascii_case("ID_CREDITS")));
            if credits {
                let ((px, py), (pw, ph)) = (e.position(), e.size());
                let b = crate::movie_overlay::Box4 { left: px as i32, top: py as i32, right: (px + pw) as i32, bottom: (py + ph) as i32 };
                self.draw_credits_roll(b, self.clock_ms as i64, talpha, aspect, out);
            }
            super::fe_depth::apply(&mut out[start..], m.inside);
        }
    }

    /// `NFS5.MOVIECIRCLE`'s nine `stencilshape` parts, the widget at `(x, y)`, in the draw's
    /// order: `[+0x1C4]` (`stencilshape1`) first, then `[+0x1C8…+0x1E4]` (0x4CDB2C…0x4CDB57; bound
    /// by name 0x4CD898…0x4CDA4D). The part list is fixed — not the `.lay` block's state nor the
    /// live one (all three shipped circles say `test1`, which lists the nine). `stencilshape1` is
    /// a `RECT` (`0xFF000000`, 256 × 232), the other eight `SHAPE`s of `movieoval.fsh` (alpha 0 /
    /// 255), a part's `flip` mirroring its art (bit 0 across, bit 1 down). **Both kinds take the
    /// widget's fade** (read 2026-09-27): `FEShapePart`'s paint (vtable 0x5B6E64 slot 5, 0x4E6F60)
    /// draws in the widget's tint — `0x528C90`, `tint_c` — with its alpha times 0x4C8840's fade
    /// over 255 (0x4E6F89…0x4E6FBF, while 0x4E3660 — `[0x657E58]`, 0 — allows it), and
    /// `FERectPart`'s (vtable 0x5B6E0C slot 5, 0x4E68C0) its own colour the same way unless the
    /// fade is 0xFF (0x4E68CB…0x4E6915): the screen loop's `talpha`. So a fading circle's stencil
    /// thins with it, and the alpha test stops it writing depth as it goes.
    fn draw_stencil_parts(&self, e: &nfs_formats::lay::Element, x: f32, y: f32, aspect: f32, tint_c: [f32; 4], out: &mut Vec<Sprite>) {
        if let Some(c) = self.controls.get(&e.class) {
            for p in (1..=9).filter_map(|k| c.part(&format!("stencilshape{k}"))) {
                if p.kind() != nfs_formats::ctr::PartKind::Rect {
                    continue;
                }
                let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
                let (Some((px, py)), Some((pw, ph)), Some(col)) = (at("pos"), at("size"), self.part_colour(e, p, "color")) else { continue };
                out.push(self.quad(aspect, x + px as f32, y + py as f32, pw as f32, ph as f32, tint(col)));
            }
        }
        for (px, py, file, index, flip) in self.shape_parts_in(e, &stencil_parts(), None) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            let (pos, size) = self.place(aspect, x + px, y + py, aw, ah);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: tint_c, rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
    }

    /// **0x4CDF40**, after the clip and the oval: the overlay's lines, flashes and runs in three
    /// tints — 0x4CFFFFFF, 0xA0FFFFFF, 0x30FFFFFF, or on the credits screen (`ID_CREDITS`, the one
    /// screen class whose slot 0x2C answers true, 0x510E90) 0x26FAB400, 0x50FAB400, 0x18FAB400 —
    /// each through 0x4D87F0 with the widget's fade. The box is the widget's own `pos` and `size`
    /// (0x4CDC9A). `crate::movie_overlay` has the objects.
    fn draw_movie_overlay(&self, i: usize, e: &nfs_formats::lay::Element, talpha: f32, aspect: f32, out: &mut Vec<Sprite>) {
        use crate::movie_overlay::{Box4, Draw, MovieOverlay, RUN_TEXT};
        let ((px, py), (pw, ph)) = (e.position(), e.size());
        let b = Box4 { left: px as i32, top: py as i32, right: (px + pw) as i32, bottom: (py + ph) as i32 };
        let now = self.clock_ms as i64;
        let mut map = self.movie_overlays.borrow_mut();
        let o = map.entry(i).or_insert_with(|| MovieOverlay::new(b, 0, 1));
        o.advance(now);
        let credits = self.layout.elements.iter().any(|x| x.get("callbacklist").is_some_and(|c| c.eq_ignore_ascii_case("ID_CREDITS")));
        let argb = |v: u32| [(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8];
        let (line, flash, run) = if credits { (0x26FAB400, 0x50FAB400, 0x18FAB400) } else { (0x4CFFFFFF, 0xA0FFFFFF, 0x30FFFFFF) };
        let faded = |v: u32| {
            let mut c = tint(argb(v));
            c[3] *= talpha;
            c
        };
        let (line_c, flash_c) = (faded(line), faded(flash));
        for d in o.draws() {
            match d {
                Draw::Bar { x, y, w, h } => out.push(self.quad(aspect, x, y, w, h, line_c)),
                Draw::Sweep { horizontal, cross, s_lo, len, along, width, forward } => {
                    // the sweep's bright end is 0xDDFFFFFF through the line tint's alpha (0x51FBA1)
                    if let Some((s, l, c0, c1)) = crate::layout::glint_sweep(len, along, width, forward) {
                        let k = line_c[3];
                        let (a, bcol) = (tint(c0), tint(c1));
                        let (a, bcol) = ([a[0], a[1], a[2], a[3] * k], [bcol[0], bcol[1], bcol[2], bcol[3] * k]);
                        let (qx, qy, qw, qh) = if horizontal { (s_lo + s, cross, l, 1.0) } else { (cross, s_lo + s, 1.0, l) };
                        let pts = [[qx, qy], [qx + qw, qy], [qx, qy + qh], [qx + qw, qy + qh]];
                        let cols = if horizontal { [a, bcol, a, bcol] } else { [a, a, bcol, bcol] };
                        self.gouraud(out, aspect, pts, cols);
                    }
                }
                Draw::Flash { frame, cx, cy } => {
                    let Some(&(tex, fw, fh)) = self.shapes.get(&(crate::layout::MOVIE_FLASHES.to_string(), frame)) else { continue };
                    let (pos, size) = self.place(aspect, cx - (fw / 2.0).trunc(), cy - (fh / 2.0).trunc(), fw, fh);
                    out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: flash_c, rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
                }
                d @ Draw::Run { .. } => self.draw_run(out, aspect, &d, RUN_TEXT, faded(run)),
            }
        }
    }

    /// One text run of 0x5208E0 (`movie_overlay::Draw::Run`) spelling `text` in `tint`.
    fn draw_run(&self, out: &mut Vec<Sprite>, aspect: f32, d: &crate::movie_overlay::Draw, text: &str, tint: [f32; 4]) {
        let crate::movie_overlay::Draw::Run { font, x, y, alpha, gap, align, vertical } = *d else { return };
        let Some(f) = self.fonts.get(crate::layout::RUN_FONTS[font.min(1)]) else { return };
        // 0x520A9A: the run tint's own alpha scaled by the fade the run computed, then
        // the widget's
        let mut c = tint;
        c[3] *= alpha as f32 / 255.0;
        // 0x522050: a letter at a time, each advancing by its own 0x536970 measure —
        // the width across, the height (the glyph's own span) down — plus the gap
        let sizes: Vec<f32> = text.chars().map(|ch| {
            let (w, h) = f.measure(&ch.to_string());
            if vertical { h } else { w }
        }).collect();
        let total: f32 = sizes.iter().sum::<f32>() + (text.chars().count() as i32 * gap) as f32;
        let (mut cx, mut cy) = (x, y);
        let shift = match align {
            1 => total,
            2 => total * 0.5,
            _ => 0.0,
        };
        if vertical {
            cy = (cy - shift).trunc();
        } else {
            cx = (cx - shift).trunc();
        }
        for (ch, sz) in text.chars().zip(sizes) {
            // every letter (0x522050 has no box test); the caller's depth mask cuts them
            self.text(out, aspect, f, &ch.to_string(), cx, cy, 0.0, 0, c);
            if vertical {
                cy = (cy + sz + gap as f32).trunc();
            } else {
                cx = (cx + sz + gap as f32).trunc();
            }
        }
    }

    /// **0x4CE0F0**, the credits roll: `FEStrings` 6000 … 6467 down the box, centred, from the
    /// string `[+0x23C]` less `[+0x244]` pixels. The speed is the credits screen's `[0x5D8BEC]` —
    /// −3 while `ID_CREDSBACK` is hovered, +3 on `ID_CREDSFWD`, 1 otherwise (0x50EB0D…0x50EB38). A
    /// line starting `#` or `!` drops the marker and takes the heading font 3 px lower and 2 px
    /// taller, `*` drops it and takes the body font, the rest take the body font; each line is
    /// `max(height + 2, 12)` tall (0x4CE47B…0x4CE495, `pitch` below). With the yraGyraG code the line
    /// `"Gary Steinke"` is 10 taller and has `"(my cat's name is mittens)"` 13 below it, in the
    /// heading font at 0xC0 of the colour (0x4CE348…0x4CE3B7). The colour is white at the widget's
    /// fade (0x4D87D0).
    ///
    /// A line's height is 0x536970's fourth output, 0x53ECE0's `maxY − minY` of the string's glyph
    /// quads (`BitmapFont::measure().1`) — not the font's line step.
    ///
    /// **Every** line the loop reaches is drawn, whether it starts above the box (the one
    /// scrolled part way off the top) or ends below it (the last): the loop runs while the lines'
    /// sum is short of the box's height (0x4CE4A0) and has no test of its own. What cuts them is
    /// the movie circle's depth mask (0x4DAAC0 … 0x4DAB00, `draw_movie`): a glyph shows only over
    /// the pixels the nine `stencilshape` parts wrote depth to — the oval, 113…527 × 94…454 on the
    /// credits screen, which is not the widget's box.
    fn draw_credits_roll(&self, b: crate::movie_overlay::Box4, now: i64, talpha: f32, aspect: f32, out: &mut Vec<Sprite>) {
        use crate::movie_overlay::{credits_wrap, CreditsRoll};
        let (Some(body), Some(head)) = (self.fonts.get(crate::layout::ROLL_FONTS[0]), self.fonts.get(crate::layout::ROLL_FONTS[1])) else { return };
        let gary = self.cheat_bits & (1 << 6) != 0;
        let text = |id: i32| self.fe_strings.get(id as u32).unwrap_or_default().to_string();
        // A line's pitch is `max(round(h) + 2, 12)` — `fistp`, round to nearest (0x4CE1FF,
        // 0x4CE47E) — with `h` 0x536970's height of the string in the line's font (+2 for a
        // heading, +10 for Gary under bit 6). The two measures differ in one thing: the step
        // (0x4CE16B…0x4CE1F9) drops a heading's `#` / `!` before measuring (`inc edi` at
        // 0x4CE1E8), the row loop (0x4CE3FC…0x4CE478) measures the string **as stored**, marker
        // and all — and neither drops a `*` (the draw alone skips it, 0x4CE2D7).
        let pitch = |id: i32, drop_heading_marker: bool| -> i32 {
            let t = text(id);
            let h = if t.starts_with('#') || t.starts_with('!') {
                head.measure(if drop_heading_marker { &t[1..] } else { &t }).1 + 2.0
            } else if gary && t == "Gary Steinke" {
                body.measure(&t).1 + 10.0
            } else {
                body.measure(&t).1
            };
            (h.round_ties_even() as i32 + 2).max(12)
        };
        let hovered = self.hovered().and_then(|k| self.layout.elements.get(k)).and_then(|e| e.get("name")).unwrap_or("");
        let speed = match hovered {
            "ID_CREDSBACK" => -3,
            "ID_CREDSFWD" => 3,
            _ => 1,
        };
        let mut slot = self.credits_roll.borrow_mut();
        let roll = slot.get_or_insert_with(|| CreditsRoll::new(0));
        roll.advance(now, speed, |id| pitch(id, true));
        let white = [1.0, 1.0, 1.0, talpha];
        let centre = ((b.right - b.left) / 2 + b.left) as f32;
        let mut acc = 0;
        let mut id = roll.id;
        while acc < b.bottom - b.top {
            id = credits_wrap(id);
            let t = text(id);
            let y = (b.top - roll.offset + acc) as f32;
            if let Some(rest) = t.strip_prefix('#').or_else(|| t.strip_prefix('!')) {
                self.text(out, aspect, head, rest, centre, y + 3.0, 0.0, 1, white);
            } else if let Some(rest) = t.strip_prefix('*') {
                self.text(out, aspect, body, rest, centre, y, 0.0, 1, white);
            } else {
                self.text(out, aspect, body, &t, centre, y, 0.0, 1, white);
                if gary && t == "Gary Steinke" {
                    let c = [1.0, 1.0, 1.0, talpha * 0xC0 as f32 / 255.0];
                    self.text(out, aspect, head, "(my cat's name is mittens)", centre, y + 13.0, 0.0, 1, c);
                }
            }
            acc += pitch(id, false);
            id += 1;
        }
    }

    // **`NFS5.ACCOUNTTHING`** is the bottom bar's money readout, and it is its own
    // class because it sets the characters by hand. `FEAccountThing`'s bind 0x4BD550
    // takes the font **`Univers7b`** (0x4DBEB0) into `[this+0x148]` and the
    // `Tournament.Data` accessor into `[this+0x140]`; the draw 0x4BD5A0 runs the
    // template's parts first (0x4C8260), asks the accessor for `"CASH"` — which
    // 0x506250 answers with `0x4DBF80(0x5012D0())`, the money with its commas — and
    // then walks the string **backwards** from `box.right − 1`:
    //
    // ```text
    // 0x4BD652  x = box.right - 1                     ; [eax+8], less one
    // 0x4BD66C  for i = strlen - 1 down to 0:
    // 0x4BD675    the single character into a two-byte buffer
    // 0x4BD6BA    y = box.top + 3
    // 0x4BD6DB    0x4DA530(font, x, y, 2, 0x4D87D0(tint), &ch)   ; right-aligned
    // 0x4BD6EF    x -= (ch == ',') ? 3 : 9
    // ```
    //
    // so it is a fixed pitch of **9** a glyph and **3** a comma, in white at the
    // widget's own fade. §4.128.
    pub(super) fn draw_account_thing(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, talpha, aspect, strings, selected, hovered, accessor, .. } = d;
        self.template_widget(out, aspect, e, self.state_for(e, i, selected, hovered), x, y, strings);
        let Some(f) = self.fonts.get("univers7b.ffn").or_else(|| self.font_for(e)) else { return };
        let Some(v) = accessor("Tournament.Data(CASH)") else { return };
        let mut cx = x + w - 1.0;
        for ch in v.chars().rev() {
            self.text(out, aspect, f, &ch.to_string(), cx, y + 3.0, 0.0, 2, [1.0, 1.0, 1.0, talpha]);
            cx -= if ch == ',' { 3.0 } else { 9.0 };
        }
    }

    // `NFS5.CONTROLLERSETUP`'s row, 0x51C670 up to the box. The class's constructor
    // loads its own font, `"%sunivers8b.ffn"`, into `[this+0x140]` (0x51C420), and
    // the row's text ignores the `.lay`'s `align` and colours alike:
    //
    // ```text
    // 0x51C6DC  focused (0x4C7DD0) -> FEStrings[0x25B] and straight to the draw, arc -1
    // 0x51C722  otherwise by the binding's kind [binding+0x14]; a keyboard key (kind 3,
    //           0x51C73B) is FEStrings[0xD0] "keyboard %s" over 0x51C050's name and arc
    // 0x51C9A9  box = 0x4C8670 (left, top, right, bottom)
    // 0x51C9D5  arc != -1: sprite = weeicons.fsh "arc<n>" (0x4D7B20)
    // 0x51CA4C    0x4D9AF0(sprite, x + (right-left) - sprite.w - 4, y + 2,
    //                      0x4D87F0(0x4D87D0(fade), 0xD0))
    // 0x51CA60    and the text's right edge moves in by sprite.w + 4
    // 0x51CAB2  0x4DA530(font, x + (right-left) - 4 [- sprite.w - 4], y + 2, 2,
    //                    0x4D87D0(fade), buffer)                 ; right-aligned, white
    // ```
    //
    // A focused row asks for a key instead of showing one — `FEStrings[0x25B]`, 603,
    // "Please enter new control..." (0x51C6E5). §4.117. The arc of a key binding is the
    // arrow's direction around the circle: up 0, right 2, down 4, left 6 (0x51C173,
    // 0x51C1A3, 0x51C1C6, 0x51C18D), in place of a name, which 0x51C050 leaves empty.
    #[allow(clippy::too_many_arguments)]
    fn controller_setup_row(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        e: &nfs_formats::lay::Element,
        state: &str,
        x: f32,
        y: f32,
        w: f32,
        talpha: f32,
        strings: &Strings,
        accessor: &dyn Fn(&str) -> Option<String>,
    ) {
        let acc = e.get("accessor").filter(|a| !a.is_empty());
        let (v, arc) = if state == "focused" {
            (strings.get(603).unwrap_or_default().to_string(), None)
        } else {
            let v = acc.and_then(accessor).unwrap_or_default();
            let arc = acc.and_then(|a| accessor(&format!("{a}(ARC)"))).and_then(|n| n.parse::<usize>().ok());
            (v, arc)
        };
        let mut right = x + w - 4.0;
        if let Some(n) = arc
            && let Some(slot) = self.arcs.and_then(|a| a.get(n).copied())
            && let Some(&(tex, sw, sh)) = self.shapes.get(&("weeicons.fsh".to_string(), slot))
        {
            let (pos, size) = self.place(aspect, x + w - sw - 4.0, y + 2.0, sw, sh);
            let color = [1.0, 1.0, 1.0, talpha * 0xD0 as f32 / 255.0];
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
            right -= sw + 4.0;
        }
        let Some(f) = self.fonts.get("univers8b.ffn").or_else(|| self.font_for(e)) else { return };
        self.text(out, aspect, f, &v, right, y + 2.0, 0.0, 2, [1.0, 1.0, 1.0, talpha]);
    }
}

impl LayoutScreen {
    // `FEGouraudBox::draw` **0x4C9F60…0x4CA095**. It is one gouraud-shaded quad whose
    // four corners are placed **independently**, and the port had been drawing an
    // axis-aligned rectangle — wrong for the 270-odd of the 576 shipped boxes that
    // carry a corner offset.
    //
    // The draw takes the widget's position (0x525360) and box, and its fade
    // (0x4C8840), then pushes four `(colour, x, y)` triples to the quad primitive
    // 0x4D98D0 — bottom-right first, then bottom-left, top-right, top-left:
    //
    // ```text
    //   UL = (x + ULoffX,     y + ULoffY)          colour cUL   [this+0x150/0x154]
    //   UR = (x + URoffX + w, y + URoffY)                 cUR   [this+0x158/0x15C]
    //   BL = (x + BLoffX,     y + BLoffy + h)             cBL   [this+0x160/0x164]
    //   BR = (x + BRoffX + w, y + BRoffY + h)             cBR   [this+0x168/0x16C]
    // ```
    //
    // — the eight offsets bound at 0x4C9E8A…0x4C9F1A, and the file really does spell
    // the third one `BLoffy` with a small `y` (the string at 0x5D865C). Each colour
    // goes through 0x4D87F0 first, which leaves it alone at a fade of 0xFF and
    // otherwise scales *its own alpha byte* by the fade — the same thing this port
    // does to every sprite after the fact.
    //
    // Drawn as the primitive's two per-corner triangles (`gouraud`); this port stepped it
    // into a 16 × 16 grid of flat cells until 2026-09-24.
    pub(super) fn draw_gouraud_box(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { e, x, y, w, h, aspect, .. } = d;
        let c = |k: &str| e.color(k).map(tint).unwrap_or([0.0, 0.0, 0.0, 0.0]);
        let (ul, ur, bl, br) = (c("cUL"), c("cUR"), c("cBL"), c("cBR"));
        let off = |k: &str| e.int(k).unwrap_or(0) as f32;
        let corner = [
            [x + off("ULoffX"), y + off("ULoffY")],
            [x + off("URoffX") + w, y + off("URoffY")],
            [x + off("BLoffX"), y + off("BLoffy") + h],
            [x + off("BRoffX") + w, y + off("BRoffY") + h],
        ];
        self.gouraud(out, aspect, corner, [ul, ur, bl, br]);
    }

    // `FEGlintyRectangle`'s constructor (0x522530) reads **`width`, `height`, `col` and
    // `animate`** — not `size` and `color`, which is why one of these draws nothing when
    // it is read like a plain rectangle. It hands them to 0x51FDA0 in that order, and
    // the draw (0x51FDE0) passes the widget's own position and that width and height to
    // **0x4D9200, which is an outline**: four calls to the one-bar primitive 0x4D9280
    // with a thickness of `1.0f` — top, bottom, left and right. So this is a one-pixel
    // frame, which is what makes it a frame around each bar of `repair.lay` (92×8 over
    // a 90×6 bar) and around the whole list (261×163 over the panel), rather than a
    // fill that would bury them.
    //
    // The outline is **verified** against the code: `FEGlintyRectangle::draw` 0x51FDE0
    // blends `col` with the fade (0x4D87F0) and hands the widget's position and that
    // width and height to **0x4D9200**, which is four calls to the one-bar primitive
    // 0x4D9280 — `(x, y, w, 1)`, `(x, y + h − 1, w, 1)`, `(x, y, 1, h)` and
    // `(x + w − 1, y, 1, h)`, the inset coming from the 1.0f at [0x5B24A8]. That is
    // this arm, bar for bar.
    //
    // Not drawn: the flourish. Its **timing** is settled — 0x51FE65…0x51FEDA: when it
    // is not running and `now − [this+0x14] > [this+0x18]` it starts, runs for
    // `rand()%32 + 64` ticks, then waits `rand()%256 + 256` before the next. `animate`
    // picks which of four to play and **5 means a fresh random one of the four each
    // time** (0x51FE8E) — which is what 93 of the 181 shipped ones carry. The four
    // flourishes' own shapes were not traced, so they are left out rather than guessed
    // at, exactly as the `NFS5.CarLogo` glint is.
    //
    // One more thing the draw does that this port does not: 0x51FDE9 returns before
    // **anything** when 0x4E35E0 answers true — `[0x6197AA] == 0`, a global that also
    // gates the glinty line (0x51FAA9) and the bar primitives. With it clear the whole
    // widget disappears, outline and all.
    pub(super) fn draw_glinty_rectangle(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, aspect, .. } = d;
        // exactly the three keys the ctor reads, and no fallback: `col`, `width` and
        // `height`. All 181 shipped ones carry those three and **none** carries
        // `color` or `size`, so a fallback could only ever disagree with the class.
        let Some(c) = e.color("col") else { return };
        let gw = e.int("width").map(|v| v as f32).unwrap_or(0.0).max(1.0);
        let gh = e.int("height").map(|v| v as f32).unwrap_or(0.0).max(1.0);
        let (c, t) = (tint(c), 1.0);
        out.push(self.quad(aspect, x, y, gw, t, c));
        out.push(self.quad(aspect, x, y + gh - t, gw, t, c));
        out.push(self.quad(aspect, x, y, t, gh, c));
        out.push(self.quad(aspect, x + gw - t, y, t, gh, c));
        // and the flourish that runs round that outline (§4.27). `animate` is 0 for
        // "none" (0x51FE31), 1…4 for which corner, and **5 for a fresh `rand() % 4 + 1`
        // every run** (0x51FE8E) — 93 of the 181 ask for 5. The port has no per-widget
        // generator, so it spreads those over the four by the widget's own index:
        // APPROXIMATION, and the only invention-shaped choice here.
        // A run may start only with the fade at 0 (0x522622…0x52262C); the object is built by
        // the bind, at the screen's open.
        let fade_zero = self.widget_progress() * 255 / TRANS_FULL == 0;
        match self.attr_num(e, "animate").unwrap_or(0.0) as i32 {
            v @ 1..=5 => self.glinty_flourish(i, Some(0), &|_| fade_zero, v, out, aspect, x, y, gw as i32, gh as i32),
            _ => {}
        }
    }

    // A panel draws **only** the layout its `filename` names, in the panel's own place.
    // Its slot-4 0x4CEBC0 — the same slot that is `FeListBox`'s combined tick-and-draw
    // 0x4C5460…0x4C691B — returns at once when `[panel+0x14C]` is null and otherwise
    // ticks the loaded screen, runs its draw and unloads it once it is down
    // (0x4E0E10 → `[panel+0x70]` = 0x4CEB00). It never reaches the base part draw, so
    // the `area` RECT of `Panel.ctr` is not painted — which the files agree with:
    // 22 of the 24 shipped panels carry the template's own `color.value=0x8000ff00`
    // unchanged, a half-transparent green that is nowhere on screen in the game.
    pub(super) fn draw_panel(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>, blank: &mut Vec<(String, String)>) -> bool {
        let WidgetDraw { i, x, y, aspect, strings, accessor, .. } = d;
        // a closed panel — a tab not selected, or one the screen hid (0x4CEC50) after it was
        // loaded — draws nothing and is not a gap
        if self.closed.contains(&i) {
            return true;
        }
        let Some(g) = self.groups.get(&i) else { return false };
        out.extend(self.embedded(g, aspect, strings, accessor, blank, x, y));
        false
    }

    // `NFS5.3DCAR`'s car is an offscreen view the size of the screen, not a sprite:
    // where the car sits in that view comes from the camera, not from the widget's
    // rectangle. It is drawn **here**, in the widget's own place in the section order,
    // which is where the original draws it — the class has no special pass.
    //
    // It used to be appended after the whole screen instead, because on
    // `carcompare.lay` the two `DarkOval.fsh` pictures (sections 89 and 90) would
    // otherwise cover the two cars (sections 64 and 65). They do not: the later
    // section is painted **first** (§4.67), so the ovals go under the cars. That
    // reading was an artefact of the inverted draw order.
    pub(super) fn draw_3d_car(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, talpha, aspect, .. } = d;
        if let Some(&tex) = self.car_views.get(&i) {
            let design = DESIGN.0 / DESIGN.1;
            let (fw, x0) = if aspect > design { (design / aspect, (1.0 - design / aspect) * 0.5) } else { (1.0, 0.0) };
            out.push(Sprite { texture: tex, pos: [x0, 0.0], size: [fw * aspect, 1.0], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
    }

    // `deadzone.lay`'s eight axis bars. The class draws through its own slot 15
    // (0x51B5F0) and **that draw returns before it paints anything when no analogue
    // controller is enumerated**: 0x51B606 calls 0x51F2B0, which walks the device list
    // (0x532E10), and `je 0x51BB08` leaves — so with no pad, drawing nothing here is the
    // original's own behaviour rather than a gap, and this method says so by returning
    // `true`. With a pad, `deadzone_bars` draws what `RE/notes/controls.md` §4.17 reads —
    // the two gradient bands through 0x521CB0 and the bars over them — off the devices
    // the port enumerates (`AUDIT.md` §F row 10). *(This block said "this port has no pad
    // enumeration behind the front end … read and not ported" until 2026-09-21, directly
    // above the call that draws them.)*
    //
    // Its own `area` RECT is the `0x8000ff00` placeholder, and it paints none of its
    // template anyway (slot 15 never chains to the base part walk at 0x520E50). The
    // dark track that is visible on the screen is a separate `NFS5.Rectangle` of
    // `0x66000000` that the layout puts over the same 300x9 box in the next section.
    pub(super) fn draw_deadzone(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) -> bool {
        let WidgetDraw { e, x, y, w, h, aspect, .. } = d;
        self.deadzone_bars(out, aspect, e, x, y, w, h, tint);
        // with no analogue device the draw returns before painting anything
        // (0x51B60D), which is not a gap
        !self.any_analogue()
    }

    // The text boxes: `InputBox.ctr`'s `state.normal` is `unselbkgd` — a
    // `data.width`×14 rect filled `0xFFBC0000`, the same dark red `ControllerSetup.ctr`
    // gives its rows as `regcolor` — four one-pixel black `bgoutline` rects, and
    // `untext`, whose text is the widget's accessor. The chat boxes are built the same
    // way. Their `state.focused` swaps in `selbkgd`/`hitext` and a `cursor`: focus is the
    // flag of `RE/notes/controls.md` §4.109, and the caret is drawn near the end of this
    // method (§4.155). *(This said "which this port has no focus or caret for" until
    // 2026-09-21.)*
    pub(super) fn draw_text_boxes(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, w, h, talpha, aspect, strings, selected, hovered, accessor } = d;
        let state = self.state_for(e, i, selected, hovered);
        for (px, py, pw, ph, c) in self.rect_parts_in(e, state) {
            out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(c)));
        }
        // **`NFS5.CHATDISPLAYBOX` is a list.** Its bind 0x4C2B60 takes the font
        // `Univers8b` into `[this+0x140]`, `data.spacing` into `[+0x144]` and
        // **`data.height / data.spacing`** into `[+0x148]` (0x4C2BD7's `idiv`) — the
        // number of rows that fit — and the draw 0x4C2F50 walks them:
        //
        // ```text
        // 0x4C2FCC  first = [this+0x150]->slot10()        ; the accessor's index
        // 0x4C2FEC  for i = first .. first + [+0x148]:
        // 0x4C3000    y = (i - first) * [+0x144] + box.top + 2
        // 0x4C303A    x = box.left + 1 + 3.0               ; [0x5B2600]
        // 0x4C304F    0x4DA350(font, x, y, 0, 0x4D87E0(fade), row, 1.0, 1.0)
        // ```
        //
        // so the rows are **black** at the widget's fade, `spacing` apart. They are the
        // chat's lines, `[0x65B6A8]` (0x4C2FD1), which `set_chat_lines` puts here — not
        // `Comms.PlayerList`, which the box only asks for names. §4.131, §4.151.
        if e.class == "NFS5.CHATDISPLAYBOX"
            && let Some(cf) = self.fonts.get("univers8b.ffn").or_else(|| self.font_for(e))
            && let Some((rows, cur)) = self.lists.get(&i)
        {
            let spacing = self.attr_num(e, "spacing").unwrap_or(14.0) as f32;
            let fit = if spacing > 0.0 { (self.attr_num(e, "height").unwrap_or(0.0) as f32 / spacing) as usize } else { 0 };
            let first = cur.unwrap_or(0);
            for (k, row) in rows.iter().skip(first).take(fit).enumerate() {
                if let ListRow::Text(t) = row {
                    self.text(out, aspect, cf, t, x + 4.0, y + 2.0 + k as f32 * spacing, 0.0, 0, [0.0, 0.0, 0.0, talpha]);
                }
            }
            // **And its scrollbar, ported 2026-09-21 (§4.131).** 0x4C2F91 asks the
            // accessor — the box's own `SliderBar` range, `0 … max(0, rows − fit)` — for
            // its count and 0x4C2F94's `cmp eax, 1 ; jle` skips the bar unless that is
            // **more than one**, i.e. unless the lines overflow the box *(corrected
            // 2026-09-25: this tested the row count itself)*; 0x4C2FA6 then calls 0x4DB530
            // with `x = y = 0` (both `xor`ed at 0x4C2F8B and 0x4C2F8D), so the
            // sub-object's rectangles are already absolute.
            //
            // Those rectangles come from the same layout the combo's do: the chat box
            // builds the sub-object at 0x4C2C85 through **0x4DADB0**, which installs
            // `FeListScrollBar`'s vtable 0x5B6A64 and calls **0x4DAF10** at 0x4DADF9 —
            // the constructor §4.75 reads out, with its own literals 13 (the arrow) and
            // 16 (the width). 0x4C2C74 takes that 16 off the list's width through
            // 0x4DB030 (`edi -= eax` at 0x4C2C79), so the bar owns the rightmost
            // sixteen pixels of the box over its whole height — exactly the combo's
            // arrangement, which is why `combo_scrollbar` serves both.
            let count = rows.len().saturating_sub(fit) + 1;
            if count > 1 {
                self.combo_scrollbar(out, aspect, x + w - LIST_SCROLLBAR_W, y, h, count, first, &tint);
            }
        }
        // The three input boxes share one class — `NFS5.INPUTBOX`,
        // `NFS5.CHATINPUTBOX` and `NFS5.CHARVECTACCESSORINPUTBOX` all write vtable
        // 0x5B60AC — and its draw **0x4CC950** puts a drop shadow at (4, 4) over the
        // box (0x4CCA6F…0x4CCAA2) unless `data.noshadow` is set, which the bind reads
        // into `[this+0x155]` (0x4CC4DD) and leaves **0** by default. Of the eight
        // shipped ones three carry `noshadow=0`, three leave it absent and two — the
        // `NFS5.CHARVECTACCESSORINPUTBOX`es — carry 1, so six get it.
        //
        // The caret of 0x4CC9D7 **is** drawn, below: it blinks on
        // `TIMER_getTicks() & 0x3F < 0x20` and only while the box has focus
        // (0x4C7DD0, flag 0x40), and this port has both — the front end's clock and
        // §4.109's focus flag. *(This comment said "this port has neither" until
        // 2026-09-21, ninety lines above the code that draws it.)*
        // `NFS5.CHATDISPLAYBOX` is a different class (vtable 0x5B5A58, draw
        // 0x4C2F50) and has no shadow.
        // **`NFS5.CHATINPUTBOX` is not one of them.** It writes vtable 0x5B5AC8 and
        // has a draw of its own, **0x4C33C0**, whose whole tail is one test: focused
        // (0x4C7DD0) **or** pressed (0x4C7C90 and 0x5299C0) gets §4.21's bevel over
        // the box with a thickness of 3 (0x4C34F8), and anything else gets nothing —
        // no shadow and no highlight. This port has no focus in the front end, so
        // only the pressed half of that test is here.
        if e.class == "NFS5.CHATINPUTBOX" {
            // 0x4C34A0…0x4C34F8: **focused or pressed** takes the bevel, and nothing
            // else takes anything. Focus is a runtime flag now (§4.109), so the
            // first half of that test is drawn as well.
            if state == "focused" || self.pressed_on(i, hovered) {
                self.bevel(out, aspect, x, y, w, h, 3.0);
            }
        } else if matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX") {
            // and 0x4CC950's own tail is a three-way. A box that is **pressed** —
            // hovered (`[this+0x10C] & 4` — bit 2, §4.109 — 0x4C7C90) with a button down
            // (`[0x69CAE8]`, 0x5299C0, which the pump sets at 0x5297E3) — takes the
            // bevel at 0x4CCAF5 and returns. Otherwise it puts down the drop shadow
            // unless `data.noshadow` (0x4CCA57) and then, **always**, the inset
            // highlight 0x521810 at 0x4CCAE3.
            if self.pressed_on(i, hovered) {
                self.bevel(out, aspect, x, y, w, h, 3.0);
            } else {
                if self.attr_num(e, "noshadow").unwrap_or(0.0) == 0.0 {
                    self.drop_shadow(out, aspect, x, y, w, h, 4.0, 4.0);
                }
                self.glass(out, aspect, x, y, w, h, &tint);
            }
        }
        // `NFS5.CONTROLLERSETUP`'s own draw, **0x51C670**, ends like the buttons':
        // the bevel 0x521050 over the widget's box with a thickness of 3 (0x51CB16),
        // ungated but for the two visibility tests at 0x51CACE and 0x51CAD9. Its box
        // comes from 0x4C8670 at 0x51C9B0 and its position from the `pos + offset`
        // pair of 0x51C6B3. Eighteen widgets, and the only class in this arm that is
        // registered at all — the text boxes are their template and nothing more.
        // — and, when it is *not* pressed, the inset highlight 0x521810 over the
        // same box (0x51CB64, the arm 0x51CACE and 0x51CAD9 jump to). §4.75.
        // The gate is **focused or pressed**, and not disabled — 0x51CABC asks
        // 0x4C7DD0 first and only falls to 0x4C7CB0 when that says no (0x51CAC5),
        // and 0x51CAD2 sends a disabled row to the highlight arm too. §4.117.
        if e.class == "NFS5.CONTROLLERSETUP" {
            // the text and the arc come first (0x51C9A9…0x51CAB2), the box over them
            self.controller_setup_row(out, aspect, e, state, x, y, w, talpha, strings, accessor);
            let lit = (state == "focused" || self.pressed_on(i, hovered)) && state != "disabled";
            if lit {
                self.bevel(out, aspect, x, y, w, h, 3.0);
            } else {
                self.glass(out, aspect, x, y, w, h, &tint);
            }
            // and 0x51C670 returns there: no caret, which is the input boxes' (0x4CC9D7)
            return;
        }
        // `NFS5.CHATDISPLAYBOX`'s draw 0x4C2F50 is the chat's rows and its scrollbar, above, and
        // nothing else — its `data.accessor` (`Comms.PlayerList`) only names players. *(Until
        // 2026-09-25 it fell through to the input boxes' value and printed that list's first row
        // — "empty" — in the chat once the list had rows.)*
        if e.class == "NFS5.CHATDISPLAYBOX" {
            return;
        }
        let Some(f) = self.font_for(e) else { return };
        // an unanswered accessor is an empty box, not a missing one — the caret still
        // blinks in it, which is what a name field waiting to be typed into looks like
        let v = match self.edited(i) {
            Some(t) => t.to_string(),
            None => e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor).unwrap_or_default(),
        };
        // **`data.showstars` masks the value.** The bind reads the attribute's int
        // into `[this+0x154]` (0x4CC4C1, default 0 at 0x4CC495) and the draw, when it
        // is set, walks the accessor's string `[[this+0x144]+0x3C]` and writes
        // **0x2A**, `'*'`, once per character into its own buffer
        // (0x4CC880…0x4CC8B5). One of the eight shipped boxes carries it:
        // `accountlogin.lay`'s `COMMS.PASSWORD`. §4.88.
        let v = if self.attr_num(e, "showstars").unwrap_or(0.0) != 0.0 {
            "*".repeat(v.chars().count())
        } else {
            v
        };
        // `[state.normal]` draws `untext`, white; `[state.focused]` draws `hitext`,
        // `0xFF000000` — black over the amber `selbkgd`
        let ink = match self.controls.get(&e.class).and_then(|c| c.part(if state == "focused" { "hitext" } else { "untext" })).and_then(|p| p.value("foreColor")).and_then(parse_color) {
            Some(c) => tint(c),
            None => [1.0, 1.0, 1.0, 1.0],
        };
        self.text(out, aspect, f, &v, x, y, w, e.int("align").unwrap_or(0), ink);
        // The caret, 0x4CC9D7: while the box has focus (0x4C7DD0) and
        // `TIMER_getTicks() & 0x3F < 0x20` — half of a 64-tick period at the front
        // end's 128 Hz, so 1 Hz on, 1 Hz off — the draw calls the part the bind
        // resolved into `[this+0x14C]` (0x4CC50B, the part named **`cursor`**) at the
        // widget's own place. `InputBox.ctr` puts that part at
        // `((data.width - hitext.size.width)/2)+hitext.size.width+1, 3`, one pixel
        // wide and `data.height-6` tall, in `0xff000000`.
        //
        // **All three classes draw it, and two of them draw an underscore, not a
        // bar.** `NFS5.CHATINPUTBOX` has a draw of its own, 0x4C33C0, and its tail
        // 0x4C343D…0x4C3489 is the same block instruction for instruction — the same
        // `isFocused`, the same `and eax, 0x8000003F` / `cmp eax, 0x20`, the same
        // `[this+0x14C]` — because its bind 0x4C32D3 starts by calling the input
        // box's own 0x4CC430, which is where that field is resolved. So the part each
        // class names is whatever **its** `.ctr` calls `cursor`, and the three differ:
        //
        // | template | `cursor` | `pos` |
        // |---|---|---|
        // | `InputBox.ctr` | `RECT`, `1 x data.height-6`, `0xff000000` | `((data.width - hitext.size.width)/2)+hitext.size.width+1, 3` |
        // | `CharVectInputBox.ctr` | `TEXT` `_` in `univers8b.ffn`, `0xFF000000` | the same x, `2` |
        // | `ChatInputBox.ctr` | `TEXT` `_` in `univers8b.ffn`, `0xFF000000` | `4+hitext.size.width+1, 2` |
        //
        // and all three reduce to **one pixel past the right edge of the text**: each
        // template's `cursor.pos.x` is its own `hitext.pos.x` plus `hitext.size.width`
        // plus one (`hitext` is at `(data.width - hitext.size.width)/2` on the first
        // two and at `4` on the chat box). So the caret is placed off the string this
        // port has just drawn rather than off a formula, which keeps it against the
        // text whatever alignment the widget ends up with; everything else — the
        // shape, the colour, the size and the `y` — is read out of the part.
        if state == "focused" && (self.clock_ms * 128 / 1000) & 0x3F < 0x20 {
            let Some(p) = self.controls.get(&e.class).and_then(|c| c.part("cursor")).cloned() else { return };
            let tw = f.place(&v).1;
            // `hitext.size.width` is the width of the string just drawn, not the
            // template's `"Text Here"` placeholder, so it is answered here; every
            // other name goes to the normal resolver.
            let at = |expr: &str| {
                nfs_formats::ctr::eval_pair(expr, &|n: &str| match n {
                    "hitext.size.width" => nfs_formats::ctr::Macro::Text((tw as i32).to_string()),
                    _ => self.reference(e, n, 0, true),
                })
            };
            let align = e.int("align").unwrap_or(0);
            let left = match align {
                1 => x + w * 0.5 - tw * 0.5,
                2 => x + w - tw,
                _ => x,
            }
            .round();
            let cx = left + tw + 1.0;
            let cy = y + p.value("pos").and_then(&at).map(|(_, b)| b as f32).unwrap_or(3.0);
            match p.kind() {
                nfs_formats::ctr::PartKind::Text => {
                    let cf = p.value("fontPathName").and_then(|n| self.fonts.get(&n.to_ascii_lowercase())).unwrap_or(f);
                    let ink = p.value("foreColor").and_then(parse_color).map(&tint).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                    self.text(out, aspect, cf, p.value("text").unwrap_or("_"), cx, cy, 0.0, 0, ink);
                }
                _ => {
                    let (cw, ch) = p.value("size").and_then(&at).unwrap_or((1, 8));
                    let colour = p.value("color").and_then(parse_color).unwrap_or([0, 0, 0, 255]);
                    out.push(self.quad(aspect, cx, cy, (cw as f32).max(1.0), (ch as f32).max(1.0), tint(colour)));
                }
            }
        }
    }

    pub(super) fn draw_list_box(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { i, e, x, y, aspect, .. } = d;
        // `FeListBox::draw` 0x4C64FE…0x4C691B with `ListBox.ctr`. The panel is the
        // template's `selectionbox` part — `pos=0,1`, `size=data.width,
        // data.selectionboxmaxheight` — filled with 0x10FFFFFF (0x4C6636). When the
        // rows are taller than it (0x4C53F3) a scrollbar appears and takes 16 px off
        // the rect's width (0x4C5421 with 0x4DB030). Rows are the accessor's own: 14 px each
        // (0x4BC700), the string at (+3, +2) over a bar of the row's colour
        // (0x4BC350). The row loop's own test is at its foot (§4.49), so the row that
        // crosses the bottom edge is drawn — and cut there by the depth mask below.
        let Some((rows, current)) = self.lists.get(&i) else { return };
        let Some(f) = self.font_for(e) else { return };
        let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
        let full: f32 = rows.iter().map(|r| r.height()).sum();
        // **The panel is the `selectionbox` part's own rect and keeps its full
        // height.** `refreshRows` 0x4C53A0 clamps `[this+0x1C4]`, which is the height
        // of the *rows*, and the only geometry it ever changes is the rect's **width**
        // — 0x4C542D / 0x4C544A call 0x4E2BF0, which reads the old size pair and
        // replaces the width alone. So a short list still paints the whole box. (An
        // earlier reading of §4.16 clamped the panel too; that was wrong.)
        let scrolls = full >= max_h;
        let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
        let panel_h = max_h;
        let mut panel_w = width;
        if scrolls {
            panel_w -= LIST_SCROLLBAR_W;
        }
        let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)).map(|(a, b)| (x + a as f32, y + b as f32)).unwrap_or((x, y)),
            None => (x, y),
        };
        // 0x4C6617…0x4C667B: `0x4D9280(x, y, width + 16 when it scrolls, height,
        // 0x10FFFFFF)` — the 16 the rect gave up is taken back for the fill, so the
        // bar spans the whole widget and the scrollbar is drawn over its right edge.
        //
        // **And that fill is a depth mask's stencil** (read 2026-09-27): 0x4DAA10 (0x4C6516)
        // before it, then `THRASH_setstate(0x24, 0)` (0x4C6683) — the alpha test **off** for the
        // flush 0x4DAAC0 (0x4C668D) does, so the 16-alpha fill writes depth over its whole rect —
        // then `THRASH_setstate(0x24, 1)` (0x4C6696): the rows, and the scrollbar (0x4DB530 at
        // 0x4C693D), are drawn `GREATEREQUAL` with the test at 1 until 0x4DAB00 (0x4C6942). **So the
        // row that overhangs the box is cut at its bottom edge** — by the mask, not by the loop,
        // which has no test of its own (§4.49). And the reference stays 1 for the rest of the
        // frame (`fe_depth`).
        let mask = super::fe_depth::mask_begin();
        let stencil = out.len();
        out.push(self.quad(aspect, px, py, width, panel_h, [1.0, 1.0, 1.0, 16.0 / 255.0]));
        super::fe_depth::apply(&mut out[stencil..], nfs_renderer::SpriteDepth { alpha_test: None, ..mask.stencil });
        super::fe_depth::set_alpha_test(Some(1));
        let inside_from = out.len();
        let inside = nfs_renderer::SpriteDepth { alpha_test: Some(1), ..mask.inside };
        // The scrollbar the list grows when it clamps is a `NFS5.VerticalSlider`, and
        // `VerticalSlider.ctr` says what it is: a `track` the full height in
        // **0xffffbb00**, `weeicons.fsh` entry **13** at `0,7` for the up arrow, entry
        // **14** at the bottom for the down arrow, and a `knob` of
        // `decrement_button.size.width × 15` in **0x80ffffff** starting at
        // `decrement_button.size.height`. The knob then moves by the scrollbar helper's
        // own rule, the vertical branch of 0x4D0BBE:
        // `dy = index · (trackHeight − knobHeight) / (count − 1)`.
        //
        // It lives in the sixteen pixels the panel gave up: 0x4C5421 takes exactly
        // 0x4DB030's **16** off the selection box's width and leaves its x alone, so
        // the strip freed is the one to its right. The bar is drawn at the strip's
        // left edge — **settled 2026-09-28** (was APPROXIMATION), below. The constructor
        // that plants the four boxes, 0x4DAF10 (§4.75), is a clean 3-arg `(x, y, h)`
        // `ret 0xC` — `trough.x = x` unmodified, `trough.y = y`, no width argument at all
        // (0x10 is a constant).
        //
        // **`x = parent.+0x38 + self0.+0x38 + rect.x`, `y = self0.+0x3c + rect.y`** —
        // refreshRows' tail (0x4C5796…0x4C57C9), unwound arg-by-arg 2026-09-27: `self0 =
        // *[esi+0x178]` (the selection-box rect object), `parent = *(self0+4)`, and `rect`
        // is the one stack argument both single-thiscall-arg getters (0x4E2B50, then
        // 0x4E2B30, `y` before `x`, each `ret 4`) take — **0x4C8670's own return**.
        //
        // **0x4C8670 read to its `ret 4`, 2026-09-27, and settled 2026-09-28** (the
        // "unread virtual call plus two string-keyed helpers" this marker used to carry):
        // `[this+0x34]` is the widget's **own** box, `{left, top, right, bottom}` — a
        // virtual call, no name lookup. `0x5A134B` is **not** a named lookup either — its
        // signature `(ptr, vfDelta, srcType, targetType, isRef)`, its SEH prologue and its
        // type-descriptor-flag dispatch to 0x5A14CD/0x5A1527/0x5A1620 are the compiler's own
        // `__RTDynamicCast`, the exact same helper `layouts.md`'s Tabs entry already names
        // (`[[this+0xD0]+0xD0]` cast to `FERadioGroup`) — so this call is
        // `dynamic_cast<T*>([this+0xD0])`, and `[this+0xD0]` is the widget's own
        // **parent/owner** pointer (`controls.md` lines 406/407/430, the same field).
        // **`T` settled 2026-09-28** by dumping the call's own pushed immediates
        // (0x4c868f/0x4c8697) and reading them as `TypeDescriptor`s (`vfptr`/`spare`/mangled
        // `name`, straight out of `.data`, exactly like the already-committed `FERadioGroup`
        // one at 0x5D9030) rather than the "lazy-bound cache cell / bootstrap thunk" this
        // marker guessed: `0x5D75E8` (`TargetType`) is `".?AVControl@FERT@@"` = **`T =
        // FERT::Control`**, `0x5D75C8` (`SrcType`) is `".?AVTarget@FERT@@"` = `FERT::Target`
        // (`owner`'s own static type) — `Control@FERT` is this codebase's already-documented
        // "FERT part/control root" (`RE/drafts/round2/fe-widgets.md` §1.2, vtable 0x5BAFD4),
        // the common base of every widget and every screen, so the cast asks "is my owner a
        // widget/window" (true for an ordinary widget's screen or panel). `0x525310` is
        // **not** a lookup either: read to its own `ret 4`, `*(this+0xE0)`'s own `+0x38`/
        // `+0x3c` (a pointer indirection, `mov ecx,[ecx+0xE0]` then `[ecx+0x38]`/`[ecx+0x3c]`)
        // — the **same** position pair `self0`/`parent` are read through above, called here on
        // **`owner`**, not the widget itself: `point = owner`'s own on-screen position. So
        // `0x4C8670` hands back the widget's own box **translated** by its owner's position:
        // `rect.x = box.left + point.x`, `rect.y = box.top + point.y` (0x4C86BF…0x4C86E2,
        // both edges of each axis shifted the same amount).
        //
        // **`point`, and hence the whole formula, needs no separate port code**: of the
        // shipped `.lay` files that reach here, a scrollbar's widget is either on a plain
        // top-level screen (`point` = (0, 0), the screen's own rest position) or inside a
        // `NFS5.PANEL` sub-layout (`audioreal`/`cdplayer` under `Audio.lay`,
        // `deadzone`/`gamectrl` under `Controllers.lay`, `display`/`graphicsettings`/
        // `advgraphics` under `Options.lay`/`graphics.lay`) — and for those, `owner` is the
        // sub-layout's own screen, whose position `0x4CECE0` sets to the panel's own
        // placement (`controls.md` §4.1) — i.e. `point` is the panel's placement. `layout.rs`'s
        // `embedded()` (the panel draw) already applies exactly that translation, once, to a
        // sub-layout's whole finished sprite list rather than once per `0x4C8670` call, so this
        // port's `px, py` (from `x, y` — already the widget's fully-resolved on-screen position,
        // panel offset included where it applies) **is** `rect` above for every shipped case.
        // `X = parent.x + self0.x + rect.x` settles to what `px, py` already compute — "the
        // strip's left edge" is the formula's own answer, not an untraced inference. The knob's
        // index is the port's own scroll position now (§4.45), which is what
        // `[this+0x1BC]->slot10()` is in the original.
        if scrolls {
            let sx = px + panel_w;
            let art = |n: usize| self.shapes.get(&("weeicons.fsh".to_string(), n)).copied();
            let (up, down) = (art(13), art(14));
            let track_w = down.map(|(_, w, _)| w).or(up.map(|(_, w, _)| w)).unwrap_or(LIST_SCROLLBAR_W);
            out.push(self.quad(aspect, sx, py, track_w, panel_h, tint([0xff, 0xbb, 0x00, 0xff])));
            if let Some((tex, aw, ah)) = up {
                let (pos, size) = self.place(aspect, sx, py + 7.0, aw, ah);
                out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
            }
            if let Some((tex, aw, ah)) = down {
                let (pos, size) = self.place(aspect, sx, py + panel_h - ah, aw, ah);
                out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
            }
            // the knob's own travel, straight out of `list_scrollbar` so that where
            // it is drawn and where a drag puts it are the same arithmetic
            let knob_w = up.map(|(_, w, _)| w).unwrap_or(track_w);
            let ky = self.list_scrollbar(i).map(|b| b.knob.1).unwrap_or(py);
            out.push(self.quad(aspect, sx, ky, knob_w, LIST_KNOB_H, tint([0xff, 0xff, 0xff, 0x80])));
        }
        // `textcolor` is black and the two highlights amber and dark red, all three
        // straight out of `ListBox.ctr`; a highlighted row's own text turns white
        // (0x4C67A6 → 0x4D87D0).
        let text_c = self.attr(e, "textcolor").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
        let sel_c = self.attr(e, "highlightcolorselected").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0xBC, 0, 0, 255]);
        let hov_c = self.attr(e, "highlightcolorhover").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]);
        // The hovered row. `FeListBox`'s slot 8 — its mouse-move handler, 0x4C6F60 —
        // box-tests the pointer (0x4E2970), then walks the rows accumulating each
        // one's height (`accessor->slot26`, 0x4C6FF6) and takes the **first** row whose
        // running bottom is past the pointer's y (0x4C7013), leaving it in
        // `[this+0x1A8]` with `[this+0x1A4]` as "there is one". The draw asks those two
        // *before* it asks about the selected row (0x4C674C jumps past 0x4C677C), so
        // **hover wins over selection**, and its bar is `data.highlightcolorhover`
        // with the row's text turned white (0x4C676A, 0x4C6772).
        //
        // The original measures from the widget's own box (0x4C8670); the port
        // measures from the selection box, one pixel below it.
        let hover_row = self.cursor.and_then(|(cx, cy)| {
            if !(cx >= px && cx < px + panel_w && cy >= py && cy < py + panel_h) {
                // off the list. 0x4C6F00 clears the row *unless* the pointer has
                // landed on this list's own scrollbar, so keep the last one then.
                return match (self.combo_scroll, self.combo_hover.get()) {
                    (Some((c, _, _)), Some((h, n))) if c == i && h == i => Some(n),
                    _ => {
                        self.combo_hover.set(None);
                        None
                    }
                };
            }
            let mut acc = py;
            let first = self.scroll.get(&i).copied().unwrap_or(0);
            let n = rows.iter().skip(first).position(|r| {
                acc += r.height();
                cy < acc
            }).map(|n| n + first);
            self.combo_hover.set(n.map(|n| (i, n)));
            n
        });
        // The row loop starts at the scrollbar's own position — 0x4C66E8 takes
        // `[this+0x1BC]->slot10()` for its first row (§4.45)
        let first = self.scroll.get(&i).copied().unwrap_or(0).min(rows.len().saturating_sub(1));
        // **The loop does not stop at the edge.** Its test is at its *foot*, 0x4C6915:
        // a row is drawn, its height added, and only then is the running total compared
        // with `[this+0x1C4]` — so the row that crosses the bottom edge is drawn. The port
        // used to stop one row early. *(Corrected 2026-09-27: this said the row "overhangs"
        // and that 0x4DAA10 / 0x4DAAC0 were "a z step, not a scissor" — they are the depth
        // mask above, which cuts that row at the fill's bottom edge.)*
        let mut acc = 0.0f32;
        let mut ry = py;
        // `Parts.AllParts` overrides the row draw (0x515620, slot 15 of vtable
        // 0x5BA270) and its tail is the same two-arm one every button here has: a
        // row that is **pressed** — selected with a mouse button down, 0x51570D and
        // 0x515711 — takes §4.21's bevel (0x51572C), and every other row takes the
        // inset highlight 0x521810 (0x515746), over the row's own 13-pixel bar. This
        // port has no pressed row in a list, so only the second arm is here. §4.75.
        let parts_glass = e.get("accessor").is_some_and(|a| a.starts_with("Parts.AllParts"));
        // `FEAllPartsAccessor::drawRow` 0x515620 puts a **one-pixel, 14-tall black
        // rule** at the `ID_PRICEPOS` anchor on every row, before it looks at the
        // section at all: 0x515641 finds the widget by name through 0x4D63E0,
        // 0x51569B takes `0x4D87E0(fade)` — black — and 0x5156AC pushes 14.0 and
        // 1.0 into 0x4D9280. It is the column the price is written against. §4.124.
        let price_x = parts_glass.then(|| self.anchor_x("ID_PRICEPOS")).flatten();
        // and the row's own bar: `section < 2` — the headings and the car's own parts,
        // which is the **second** of the two lists this port builds (§4.114) — takes a
        // 13-tall bar in `0x4D87B0`, the dark red 0xBC0000, when the caller hands it
        // no colour (0x5156DE, 0x5156FE). The shop's rows take a one-pixel rule
        // instead (0x51574E).
        let mut headers = 0usize;
        for (n, row) in rows.iter().enumerate().skip(first) {
            if acc > panel_h {
                break;
            }
            acc += row.height();
            let (bar, fg) = match (hover_row == Some(n), *current == Some(n)) {
                (true, _) if hov_c.is_some() => (hov_c, [1.0, 1.0, 1.0, 1.0]),
                (_, true) => (Some(sel_c), [1.0, 1.0, 1.0, 1.0]),
                _ => (None, tint(text_c)),
            };
            match row {
                ListRow::Text(t) => {
                    if let Some(c) = bar {
                        out.push(self.quad(aspect, px, ry, panel_w, LIST_ROW_H, tint(c)));
                    }
                    if parts_glass {
                        if bar.is_none() {
                            out.push(self.quad(aspect, px, ry, panel_w, 13.0, tint([0xBC, 0, 0, 0xFF])));
                        }
                        self.glass(out, aspect, px, ry, panel_w, 13.0, &tint);
                    }
                    self.text(out, aspect, f, t, px + 3.0, ry + 2.0, 0.0, 0, fg);
                }
                // a columned row draws no bar of its own — `Track.Records` reduces the
                // colour it is handed to its alpha alone (0x4FB328) and places each
                // column at another widget's x, at the row's own y (no +2)
                ListRow::Columns(cells) => {
                    if let Some(c) = bar {
                        out.push(self.quad(aspect, px, ry, panel_w, row.height(), tint(c)));
                    } else if parts_glass && headers >= 2 {
                        out.push(self.quad(aspect, px, ry, panel_w, 13.0, tint([0xBC, 0, 0, 0xFF])));
                        self.glass(out, aspect, px, ry, panel_w, 13.0, &tint);
                    }
                    for cell in cells {
                        // a cell with no anchor of its own sits at the panel's left edge
                        let ax = if cell.anchor.is_empty() { px } else { self.anchor_x(&cell.anchor).unwrap_or(px) };
                        let align = if cell.dx.is_sign_negative() { 2 } else { 0 };
                        let mut c = text_c;
                        if cell.fade != 0xFF {
                            c[3] = ((c[3] as u32 * cell.fade as u32) / 255) as u8;
                        }
                        self.text(out, aspect, f, &cell.text, ax + cell.dx, ry, 0.0, align, tint(c));
                    }
                }
                // `drawPartsHeading` 0x518DE0: a black rule at the top and another 13
                // pixels down, with the name centred two pixels below the first
                ListRow::Header(t) => {
                    headers += 1;
                    let black = [0.0, 0.0, 0.0, 1.0];
                    out.push(self.quad(aspect, px, ry, panel_w, 1.0, black));
                    out.push(self.quad(aspect, px, ry + PART_HEADER_H, panel_w, 1.0, black));
                    self.text(out, aspect, f, t, px, ry + 2.0, panel_w, 1, tint(text_c));
                    // and over it 0x521400's bevel, `(x, y, w, 13.0, fade, 2)` (0x518E7F…)
                    self.bevel_heading(out, aspect, px, ry, panel_w, PART_HEADER_H, 2.0);
                }
                // the garage list's grouping heading (§4.144): the same two rules 13
                // apart, but drawn by the row painter itself — 0x4F5D67 puts a bar at
                // the row's y and 0x4F5D8D another at `y + [0x5B7734]` (13.0), with
                // `FEStrings[era or class]` between them at 0x4F5DB3
                // `FEColourAccessor`'s rows (0x4B9390): the bar, then the swatch or the
                // custom row's string — a list box hands the same 3.0 inset (0x4C688F)
                ListRow::Swatch(_) | ListRow::CustomColour(_) => {
                    if let Some(c) = bar {
                        out.push(self.quad(aspect, px, ry, panel_w, row.height(), tint(c)));
                    }
                    self.colour_row(out, aspect, f, row, px, ry, 3.0, &tint);
                }
                ListRow::GroupHeading(t) => {
                    let black = [0.0, 0.0, 0.0, 1.0];
                    out.push(self.quad(aspect, px, ry, panel_w, 1.0, black));
                    out.push(self.quad(aspect, px, ry + PART_HEADER_H, panel_w, 1.0, black));
                    self.text(out, aspect, f, t, px, ry + 2.0, panel_w, 1, tint(text_c));
                    // and 0x521400's bevel one pixel down, `(x, y + 1, w, 12.0, fade, 2)`
                    // (0x4F5DF1…0x4F5E0C)
                    self.bevel_heading(out, aspect, px, ry + 1.0, panel_w, 12.0, 2.0);
                }
            }
            if let Some(cx) = price_x {
                out.push(self.quad(aspect, cx, ry, 1.0, 14.0, tint([0, 0, 0, 0xFF])));
            }
            ry += row.height();
        }
        super::fe_depth::apply(&mut out[inside_from..], inside);
    }
}

impl LayoutScreen {
    // the screen itself: a full-screen fill
    pub(super) fn draw_screen_fill(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) {
        let WidgetDraw { e, aspect, .. } = d;
        // **The screen's own `color` is dead**, and the template says so rather than
        // the port guessing it: `Black Screen 640x480.scr` gives the class one drawn
        // part, `[part.canvas]`, a `RECT` at `0,0` sized
        // `control.size.width,control.size.height` — `size.value=640,480` — whose
        // `color.value` is the literal **`0xff000000`**, and its `data` part's
        // `attributes.value` is exactly `screen,room,callbacklist,musicfile,
        // canplaygame`. `color` is not among them, so the `0xff0000ff` and
        // `0xff004080` three of the 98 shipped `[section.0]` blocks carry are editor
        // leftovers that nothing reads. CONFIRMED, where this used to be an
        // APPROXIMATION argued from the files looking the same.
        //
        // Only the bottom-most screen fills, and only when there is nothing behind it:
        // a screen with a 3D room behind it (`RE/notes/frontend-3d.md`) would
        // otherwise paint the room out. That part is still the port's own rule —
        // `Window::load` builds the descriptor at `[window+0x11C]` (0x52862F) and
        // binds it (0x528651), but the widget loop starts at **section 1** and
        // `Window::draw` walks `[window+0x10C]` alone, so nothing in the window paints
        // this canvas.
        if self.background.is_none() && !self.overlay {
            let c = self
                .controls
                .get(&e.class)
                .and_then(|c| c.part("canvas"))
                .and_then(|p| p.value("color"))
                .and_then(parse_color)
                .map(tint)
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            out.push(self.quad(aspect, 0.0, 0.0, DESIGN.0, DESIGN.1, c));
        }
    }

    // Every class without a method of its own: the art its template places, else the
    // widget's own picture, then its string or accessor value. Returns whether the widget
    // is erased rather than undrawn — the blank-string case near the end.
    pub(super) fn draw_default(&self, d: WidgetDraw<'_>, out: &mut Vec<Sprite>) -> bool {
        let WidgetDraw { i, e, x, y, w, h, aspect, strings, selected, hovered, accessor, .. } = d;
        let mut closed = false;
        // Not drawn here: the template's `RECT` parts. They are the widget's regions,
        // not its paint — `GoToButton.ctr`'s `bg` is `data.maincolor` over the whole
        // box, which is the colour the button's *text* uses, and filling it puts a
        // solid red bar behind every label on `QuickRace.lay`. A class that really is
        // a box draws its rects from its own arm, as `NFS5.CONTROLLERSETUP` does.
        // the art the widget's class template carries, at the places the template puts
        // it — `NFS5.CARSETTINGBUTTON`'s arrows, the popup circles, the tab buttons — from
        // the part list of the state the widget is in **now**: every class that reaches this
        // arm and whose states list different art draws through 0x4C8260 → 0x525830 —
        // `NFS5.CARSETTINGBUTTON` (slot 15 of 0x5B5970 is 0x4C8260: `picnonsel` / `picsel` on
        // hover / `picseldown` focused), `NFS5.PLAYERDRIVERPICTURE` (0x4CF7C0 → 0x4CF7CE),
        // `NFS5.CHRONICLESLIDER` (0x4C8260) and the pure-template `NFS5.POPUPCIRCLE` (the base
        // draw 0x520E50 → 0x520E5E). Until 2026-09-27 this took the `.lay` block's `state=`,
        // so no car-setting arrow ever showed its hover or pressed art.
        let mut from_template = false;
        for (px, py, file, index, flip) in self.shape_parts_at(e, &self.drawn_state(e, i, selected, hovered), None) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            let (pos, size) = self.place(aspect, x + px, y + py, aw, ah);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
            from_template = true;
        }
        // anything with a picture draws it; anything with a string draws that.
        // Only when the class template placed none — otherwise the same art twice.
        if !from_template
            && let Some((file, index)) = e.shape()
            && let Some(&(tex, aw, ah)) = self.shapes.get(&(file.to_string(), index))
        {
            // A picture is drawn at its art's own size: every piece of the oval frames
            // carries `size=50,150` in the file while the `.fsh` entries are 256×256,
            // 89×82 … and each entry's own signed offset is exactly minus the widget's
            // `pos` (`CnstLyr.fsh` entry 4 is at (−131, −120), its widget at 131,120).
            // HIGH, from the data.
            let (pos, size) = self.place(aspect, x, y, aw, ah);
            // `flip` mirrors the art: 1 in x, 2 in y, 3 in both. HIGH — the attribute is
            // an int on the picture widget (0x52AA4E) whose consumer was not traced, but
            // it is what makes the oval frames of the background screens close up: the
            // four corners of `screenempty` are the same `.fsh` entry with flip 0/1/2/3.
            let flip = e.int("flip").unwrap_or(0).max(0);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        // `NFS5.PLAYERDRIVERPICTURE`'s draw **0x4CF7C0**, after the template's picture
        // (0x4C8260): the box is `0x4C8670`'s — this class autosizes (§4.62), so it is the
        // union of its parts, the 54×64 `ALLdrivr.fsh` entry, and not the `size=50,150` the
        // `.lay` blocks carry (which the port drew the shadow over until 2026-09-25).
        //
        // 1. `[0x5D8F2C] == data.ournumber` (0x4CF83B; `[this+0x140]`, bound at 0x4CF7A8): the
        //    chosen picture's frame 0x521B60 with a thickness of **5** (0x4CF84B).
        // 2. pressed (0x4CF886 0x4C7C90, 0x4CF88F 0x5299C0): the bevel 0x521050 with 3, and
        //    the draw ends (0x4CF8DE).
        // 3. otherwise, unless it is the chosen one (0x4CF8E1), the drop shadow 0x520F00 with
        //    (4, 4) at 0x4CF92A. §4.126, §4.162.
        if e.class == "NFS5.PLAYERDRIVERPICTURE" {
            let (bw, bh) = self.box_hit(e);
            let chosen = self.attr_num(e, "ournumber").unwrap_or(0.0) as i32 == self.driver_picture;
            if chosen {
                self.picture_frame(out, aspect, x, y, bw, bh, 5.0);
            }
            if self.pressed_on(i, hovered) {
                self.bevel(out, aspect, x, y, bw, bh, 3.0);
            } else if !chosen {
                self.drop_shadow(out, aspect, x, y, bw, bh, 4.0, 4.0);
            }
        }
        if code_placed(&e.class) {
            return closed;
        }
        let Some(f) = self.font_for(e) else { return closed };
        // (A `NFS5.ComboBoxSlider` never reaches here — §4.14 gave it its own arm,
        // which places its value the way the code does, at `x + width - 12`.)
        // an accessor watch shows a value, everything else its own `stringID`
        // case-insensitively: the class is `NFS5.AccessorWatch` on most screens but
        // `NFS5.WORDWRAPACCESSORWATCH` where the text has to wrap, and a `contains`
        // that respected case never matched the second one
        let text = if e.class.to_ascii_lowercase().contains("accessorwatch") {
            match e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) {
                Some(v) => v,
                None => return closed,
            }
        } else if let Some(t) = e.get("text").filter(|t| !t.is_empty()) {
            // a literal string in the file rather than a `FEStrings` id.
            // `Non-Localized Label.ctr` is nothing but a `TEXT` part whose `text.value`
            // is the placeholder "Put Text Here", which the widget's own `text` key
            // replaces — `audioreal.lay`'s per-cent signs are `text=%`.
            t.to_string()
        } else {
            // `stringID=0` is the shipped "Hello, World!" placeholder (marked DO NOT
            // TRANSLATE in `FEStrings.csv`): those widgets get their text from code.
            match e.string_id().filter(|id| *id != 0).and_then(|id| strings.get(id)) {
                Some(t) if !t.is_empty() => t.to_string(),
                // the table **has** the id and its text is blank: the shipped
                // `FEStrings.csv` leaves 11110, 12300 and 19809 empty in every
                // language column, so those four widgets draw nothing in the original
                // too. Erased, not undrawn.
                Some(_) => {
                    closed = true;
                    return closed;
                }
                None => return closed,
            }
        };
        // the highlighted button uses its own `hovercolor`, as the original does
        let color = if selected == Some(i) {
            // a widget with no `hovercolor` of its own is highlighted anyway, so the
            // player can see where the selection is — APPROXIMATION
            e.color("hovercolor").or_else(|| e.color("maincolor")).map(tint).unwrap_or([1.0, 0.85, 0.2, 1.0])
        } else {
            e.color("foreColor")
                .or_else(|| e.color("maincolor"))
                .or_else(|| e.color("color"))
                .or_else(|| self.part_value(e, "foreColor").and_then(parse_color))
                .map(tint)
                .unwrap_or([1.0, 1.0, 1.0, 1.0])
        };
        // a widget with no box of its own anchors on its own text (align 0 then works
        // out to the position in the file, which is what the button classes carry)
        let box_w = if w > 0.0 { w } else { 0.0 };
        // The text sits at the widget's own `pos`, not centred in its box. The files
        // say so: on `tournament.lay` every value and its label share a `y` — the
        // `REAL.LABEL` at 299,114 and the `NFS5.AccessorWatch` at 302,114, and so on
        // down the panel — and the labels carry no `size` at all. Centring a watch in
        // its `size=100,30` box pushed every value a row below its label, while
        // `PRIZEMONEY`, whose box is `43,11` and so too short to centre in, lined up.
        let _ = h;
        // An accessor watch is its template's one `TEXT` part, and the class's only
        // job is to keep that part's string up to date: `FEAccessorWatch::tick`
        // **0x4BD230** updates the accessor (vtable +8), asks it for the plain text
        // (+0x10) or, when the widget carries a modifier, the modified one (+0x14),
        // and writes the answer into the part's `text` attribute with 0x526980 —
        // which is why `accessor Watch.ctr` marks that attribute `HIDDEN|READONLY`
        // and gives it the placeholder `WATCH`. An accessor that answers **null**
        // gets the literal `CANTFINDSTRING` (0x4BD2B0); one that answers an empty
        // string gets nothing, which is the knockout rows' case.
        //
        // So its position is the template's, `(0-label.size.width)/2*data.align,0`,
        // over the width of the string it is showing *now* — the same expression
        // `Label.ctr` carries (§4.8), and the same integer division. Everything else
        // keeps the port's own anchor.
        let watch_pos = self
            .controls
            .get(&e.class)
            .filter(|_| {
                let c = e.class.to_ascii_lowercase();
                // not the wrapping one: `WordWrapAccessorWatch.ctr` pins its label at
                // `0,0` and the wrap is the whole point of the class
                c.contains("accessorwatch") && !c.contains("wordwrap")
            })
            .and_then(|c| c.state_parts(e.get("state").unwrap_or("normal")).iter().filter_map(|n| c.part(n)).find(|p| p.kind() == nfs_formats::ctr::PartKind::Text))
            .and_then(|p| {
                // the tick's write (0x4BD32C) is what the part measures from now on (0x4E7160)
                self.watch_text.borrow_mut().insert(i, text.clone());
                // 0x536970's width, truncated (`_ftol` at 0x4E7280) — not the pen's advance
                let tw = f.measure(&text).0.trunc();
                let v = p.value("pos")?;
                nfs_formats::ctr::eval_pair(v, &|n| match n {
                    "label.size.width" => nfs_formats::ctr::Macro::Text((tw as i32).to_string()),
                    other => self.reference(e, other, 0, true),
                })
            });
        if let Some((px, py)) = watch_pos {
            self.text(out, aspect, f, &text, x + px as f32, y + py as f32, 0.0, 0, color);
        } else if e.class.eq_ignore_ascii_case("NFS5.WORDWRAPACCESSORWATCH") {
            // the change fade and the typing's restart (`WrapWatch`, 0x522EC0 / 0x522D60)
            let now = self.ticks() as i64;
            let animating = self.attr_num(e, "animating").unwrap_or(0.0) != 0.0;
            let mut watches = self.wrap_watch.borrow_mut();
            let w = watches.entry(i).or_insert_with(|| crate::layout::WrapWatch { text: text.clone(), changing: None, restart: None });
            let before = w.restart;
            w.tick(&text, now, animating);
            // a rebuild restamps the typing (0x52301A) and leaves its wait as it was
            let budget = self.typewriter(i, e, w.restart.filter(|_| w.restart != before));
            let mut color = color;
            color[3] *= w.fade(now);
            if budget.is_none_or(|(n, _)| n != 0) {
                self.wrapped_text(out, aspect, f, &w.text, x, y, box_w, e.int("align").unwrap_or(0), color, budget);
            }
        } else if e.class.to_ascii_lowercase().contains("wordwrap") {
            let budget = self.typewriter(i, e, None);
            if budget.is_none_or(|(n, _)| n != 0) {
                self.wrapped_text(out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color, budget);
            }
        } else {
            self.text(out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color);
        }
        closed
    }
}

/// `NFS5.MOVIECIRCLE`'s nine stencil parts in the order its draw holds them — `[+0x1C4]` then
/// `[+0x1C8…+0x1E4]`, bound by name at 0x4CD898…0x4CDA4D and painted whatever the state
/// (0x4CDB2C…0x4CDB57).
fn stencil_parts() -> Vec<String> {
    (1..=9).map(|k| format!("stencilshape{k}")).collect()
}

/// `FEMovie`'s frame blit **0x4E2820** (slot 1, vtable 0x5B6BE4), after the upload 0x536510: the
/// frame goes at `origin + ([+0x14], [+0x18])` — the widget's `pos` — and
///
/// * when the box is **shorter** than the clip and **wider** than it (`[+8] < [+0x10]` and
///   `[+4] > [+0xC]`, signed, 0x4E2885 / 0x4E28A3) through the clipped blit 0x4D9E70 at
///   `w × clip height`, cut to the rectangle `ftol(x), ftol(y), ftol(x + w), ftol(y + h)` — the
///   frame stretched across and shown 1:1 down to the box's foot, its texture cut in proportion;
/// * otherwise through 0x4D9B30 at the box's `w × h` — the frame stretched to the box.
///
/// The entry covers the clip's own `width/256 × height/256` of the 256×256 surface
/// (0x4E219C…0x4E21B3), so `v = 1` is the frame's last row. Returns the `v` the quad of height
/// `box_h` reaches: 1, or `box_h / clip height` on the clipped arm. The 454×280 and 416×348 boxes
/// are at least their clips (192×144, 208×144) both ways, so they stretch; `chronicle.lay`'s
/// 454×46 strip (`othersource=chroniclemain`, the group's `ch*.mad`) is shorter and wider, so it
/// shows the clip's top 46 rows stretched across — not the whole frame squeezed into the strip.
fn movie_blit_v(box_w: f32, box_h: f32, clip: (f32, f32)) -> f32 {
    if box_h < clip.1 && box_w > clip.0 { box_h / clip.1 } else { 1.0 }
}

#[cfg(test)]
mod movie_blit_tests {
    #[test]
    fn a_movie_box_stretches_unless_it_is_shorter_and_wider_than_the_clip() {
        assert_eq!(super::movie_blit_v(454.0, 280.0, (192.0, 144.0)), 1.0);
        assert_eq!(super::movie_blit_v(416.0, 348.0, (192.0, 144.0)), 1.0);
        // shorter and wider: 1:1 rows, cut at the box's foot
        assert_eq!(super::movie_blit_v(454.0, 46.0, (192.0, 144.0)), 46.0 / 144.0);
        // shorter but not wider: 0x4E28A9's `jle` takes the stretch
        assert_eq!(super::movie_blit_v(192.0, 46.0, (192.0, 144.0)), 1.0);
    }
}
