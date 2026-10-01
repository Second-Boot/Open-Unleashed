//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// The font a widget draws its text in: its own `fontPathName`, or the one its class template
    /// gives the part it draws (a header's is `Univers17b.ffn`), or the front end's default.
    pub(super) fn font_for(&self, e: &nfs_formats::lay::Element) -> Option<&BitmapFont> {
        let named = e
            .get("fontPathName")
            .filter(|f| !f.is_empty())
            .or_else(|| self.part_value(e, "fontPathName"))
            .map(|f| f.to_lowercase());
        named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))
    }

    /// Draw one line of text inside the widget box `(x, y, w, h)` with the original's alignment
    /// rule (0x5232AD → 0x4DA3D7): the anchor is the box's left edge / midpoint / right edge and the
    /// string is placed at the anchor, half a width back, or a full width back, rounded to pixels.
    /// Glyphs are drawn at their native size in the 640×480 design space.
    /// Lay `s` out inside `w`, breaking between words, and draw the lines `line_px` apart. Used by
    /// `data.animating`'s typewriter — how many characters of a wordwrap widget's text are on
    /// screen yet, or `None` when the widget does not animate.
    ///
    /// The flag is bound to a byte — `[this+0x144]` on `NFS5.WORDWRAPCONTROL` (0x5228AC) and
    /// `[this+0x180]` on `NFS5.WORDWRAPACCESSORWATCH` (0x522B90); the two names were swapped here
    /// until 2026-09-21, and the sizes settle it — the control is 0x160 bytes and cannot hold a
    /// field at +0x180, while the watch is 0x1A8. Nothing here keys on the offsets, so only the
    /// comment was wrong. It drives three things:
    ///
    /// ```text
    /// 0x5229A0  activate:  [+0x14C] = [+0x150] = now;  [+0x158] = animating ? 0x97 : 0
    /// 0x522A5B  each draw: not transitioning (0x4C8820 == 0) -> [+0x150] = now
    /// 0x522A6E  [+0x158] == 0 -> the ordinary draw
    /// 0x522A83  else now - [+0x14C] <= [+0x158] -> **draw nothing this frame**
    /// 0x522A87  else [+0x14C] = [+0x150] = now, [+0x158] = 0, and still nothing this frame
    /// 0x522AE2  the draw hands ([+0x144], [+0x14C], [+0x150]) to the text object 0x523290
    /// 0x5232E5  which, when the flag is set, takes a budget of ([+0x150] - [+0x14C]) / 3
    ///           (0x5232FB's 0x55555556) and reveals that many characters
    /// ```
    ///
    /// So a widget with `animating=1` is **blank for 0x97 = 151 ticks** — 1.18 s at 128 Hz — and
    /// then types itself out at **one character every three ticks**, about 43 a second. Five of
    /// the eleven shipped wordwrap widgets carry it, and they are the narrative screens:
    /// `missionbrief`, `levelvictory`, `factdone`, `tournament` and `VictoryMessage`.
    ///
    /// The stamps are the widget's own ([`Typing`]): the typing counts from the tick of the first
    /// draw that finds the wait over, and holds while the screen is in a transition (0x4C8820).
    /// *(Until 2026-09-26 this counted from tick 152 exactly and ignored the transition, an
    /// APPROXIMATION "one frame wide".)* The activation is the screen's open, tick 0 of its clock.
    /// `NFS_FE_CLOCK` preloads that clock with no frames behind it, so a widget first drawn past
    /// the wait takes the wait as having ended on its last tick (152) — the diagnostic's stand-in
    /// for the frames it skipped, which the game never meets (its first draw is at tick 0…2).
    ///
    /// The answer is the character budget and the ticks into the current character,
    /// `([+0x150] − [+0x14C]) % 3` — the remainder the same signed divide leaves in `edx`
    /// (0x52331D), which fades the leading letter in (`wrapped_text`).
    pub(super) fn typewriter(&self, i: usize, e: &nfs_formats::lay::Element, restart: Option<i64>) -> Option<(usize, i64)> {
        let animating = self.attr_num(e, "animating").unwrap_or(0.0) != 0.0;
        let now = self.ticks() as i64;
        let mut all = self.typing.borrow_mut();
        let t = all.entry(i).or_insert_with(|| {
            let t = Typing::new(0, animating);
            if t.wait != 0 && now > t.wait {
                return Typing { start: t.wait + 1, last: t.wait + 1, wait: 0 };
            }
            t
        });
        if let Some(at) = restart {
            t.restart(at);
        }
        // 0x4C8820: the fade's `p·255 / 512`, zero only at the very ends of a transition
        let transitioning = self.widget_progress() * 255 / TRANS_FULL != 0;
        let shown = t.draw(now, transitioning);
        if !animating {
            return None;
        }
        Some((shown.unwrap_or(0), ((t.last - t.start) % 3).max(0)))
    }

    /// The progress a widget of this screen transitions with — 0x4E0D90(1) as 0x4C8760 and
    /// 0x4C8820 read it.
    pub(super) fn widget_progress(&self) -> i32 {
        self.widget_state().1
    }

    /// Which way a widget of this screen transitions and how far — 0x4C8760 / 0x4C8840: the
    /// state is the widget's screen's `[+0x12C]`, and when that screen is up (2) the one on show
    /// (0x4D63C0, the front screen of the stack); the progress is 0x4E0D90(1), the larger of the
    /// screen's own and the front screen's (0x4D6300). So a panel's sub-screen at rest moves with
    /// the screen it is on, and one of its own transitions runs on top of the screen's.
    pub(super) fn widget_state(&self) -> (bool, i32) {
        let (parent_out, parent_p) = self.inherited.get();
        match self.transition {
            Transition::Idle => (parent_out, parent_p),
            t => (matches!(t, Transition::Out { .. }), t.progress().max(parent_p)),
        }
    }

    /// `NFS5.WORDWRAPACCESSORWATCH` — the class the front end uses where a value is a sentence
    /// rather than a figure: a tournament's description, a part's, a mission briefing.
    ///
    /// The break is the original's, **0x523600** (`RE/notes/controls.md` §4.51), and the lines sit
    /// the last line's measured height + 2 apart ([`wrap_metrics`], 0x523916 / 0x523576) — not the
    /// font's line height, as this said before.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn wrapped_text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4], budget: Option<(usize, i64)>) {
        if w <= 0.0 {
            self.text(out, aspect, f, s, x, y, w, align, color);
            return;
        }
        let mut ly = y;
        // `budget` is `data.animating`'s character allowance — 0x523327's loop walks the line
        // vector and, for each line, draws it whole while `strlen(line) <= ebx` and takes `ebx`
        // off the budget (0x523340…0x52336E), or cuts one line at `ebx` characters and stops
        // (0x523572 zeroes the budget). §4.87.
        let (mut left, rem) = budget.unwrap_or((usize::MAX, 0));
        let lines = wrap_lines(f, s, w);
        let (pitch, _) = wrap_metrics(f, &lines);
        for line in lines {
            if left == 0 {
                break;
            }
            let chars: Vec<char> = line.chars().collect();
            if chars.len() <= left {
                left -= chars.len();
                self.text(out, aspect, f, &line, x, ly, w, align, color);
            } else {
                self.typed_line(out, aspect, f, &chars, left, rem, x, ly, w, align, color);
                left = 0;
            }
            ly += pitch;
        }
    }

    /// The line the typewriter is in, 0x52337D…0x523569 (read 2026-09-27). With `n` the
    /// characters of it shown, the NUL goes at `n − 1` (0x523392): the first `n − 1` are drawn
    /// with 0x4DA530, and the **`n`th alone**, `ch`, the leading letter. The anchor `x0` is
    /// `_ftol` of the aligned x (`x`, `x + w/2`, `x + w`: 0x5232A9…0x5232CD, 0x523395), and per
    /// alignment (0x5233A2's switch; `p1`/`p3` are 0x536970's minX / width):
    ///
    /// ```text
    /// 0  the prefix at x0, the letter at _ftol(x0 + p3(prefix) − 1)            0x523478…0x5234AA
    /// 1  the letter at _ftol(p3(prefix)/2 + x0), the prefix at
    ///    _ftol(x0 − p1(line from n + 1)/2)                                      0x5233FB…0x52346F
    /// 2  the letter at x0, the prefix at _ftol(x0 − p1(line from n + 1))        0x5233B9…0x5233EF
    /// ```
    ///
    /// — the second measure starts one character past the letter (`[ebx+ebp+1]`) and reads the
    /// box's **minX**, not its width (`[esp+0x1C]` / `[esp+0x14]` are the first output), as
    /// written. Then, unless 0x4E35C0 answers true or the object has no `GLNT` (`[+0xDC]`, 0x523105),
    /// **a depth mask** (0x5234EC 0x4DAA10 … 0x523530 0x4DAAC0 … 0x523569 0x4DAB00): the letter is
    /// the stencil, in the text's colour at alpha × `rem·255/3` / 255 (0x4D87F0 of the ticks into the
    /// letter, 0x5234F1…0x523511 — so it fades in over its three ticks, and at 0 it is not drawn
    /// and writes no depth), and inside it the `weeicons.fsh` `GLNT` at `(letter x − 18, y)` in
    /// `0x4D87D0(alpha)` (0x523535…0x523561): a glint on the letter being typed. Without the
    /// mask's conditions the letter is not drawn at all (0x52356E restores it).
    #[allow(clippy::too_many_arguments)]
    fn typed_line(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, chars: &[char], n: usize, rem: i64, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        let prefix: String = chars[..n - 1].iter().collect();
        let ch = chars[n - 1].to_string();
        let rest: String = chars.get(n + 1..).map(|r| r.iter().collect()).unwrap_or_default();
        let x0 = match align {
            1 => x + w * 0.5,
            2 => x + w,
            _ => x,
        }
        .trunc();
        let (px, cx) = match align {
            1 => ((x0 - f.measure_box(&rest).0 * 0.5).trunc(), (f.measure_box(&prefix).2 * 0.5 + x0).trunc()),
            2 => ((x0 - f.measure_box(&rest).0).trunc(), x0),
            _ => (x0, (x0 + f.measure_box(&prefix).2 - 1.0).trunc()),
        };
        let align = align.clamp(0, 2);
        self.text(out, aspect, f, &prefix, px, y, 0.0, align, color);
        let Some(g) = self.glint.and_then(|n| self.shapes.get(&("weeicons.fsh".to_string(), n))).copied() else { return };
        let mask = fe_depth::mask_begin();
        // 0x4D87F0: alpha × k / 255 with `mul 0x80808081` (unsigned, truncating), k = rem·255/3
        let a8 = (color[3].clamp(0.0, 1.0) * 255.0).round() as u32;
        let k = (rem.clamp(0, 2) * 255 / 3) as u32;
        let letter = [color[0], color[1], color[2], (a8 * k / 255) as f32 / 255.0];
        let stencil = out.len();
        self.text(out, aspect, f, &ch, cx, y, 0.0, align, letter);
        fe_depth::apply(&mut out[stencil..], mask.stencil);
        let (tex, gw, gh) = g;
        let (pos, size) = self.place(aspect, cx - 18.0, y, gw, gh);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, a8 as f32 / 255.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: Some(mask.inside) });
    }

    pub(super) fn text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        let (glyphs, width) = f.place(s);
        let anchor = match align {
            1 => x + w * 0.5,
            2 => x + w,
            _ => x,
        };
        let left = match align {
            1 => anchor - width * 0.5,
            2 => anchor - width,
            _ => anchor,
        }
        .round();
        let top = y.round();
        // `NFS_TEXT_PROBE=1` draws the same glyphs in white — the A/B that characterised the
        // faint-text fault in `RE/AUDIT.md` F2.
        let probe = std::env::var("NFS_TEXT_PROBE").is_ok();
        let color = if probe { [1.0, 1.0, 1.0, 1.0] } else { color };
        let solid = std::env::var("NFS_TEXT_SOLID").is_ok();
        for g in &glyphs {
            let (pos, size) = self.place(aspect, left + g.x, top + g.y, g.w, g.h);
            // `NFS_TEXT_FULLUV=1` opens the glyph's UV window to the whole sheet — with
            // `NFS_TEXT_BIG` it shows the sheet, without it nothing (`AUDIT.md` F2)
            let full = std::env::var("NFS_TEXT_FULLUV").is_ok();
            let (uv0, uv1) = if full {
                ([0.0, 0.0], [1.0, 1.0])
            } else if solid {
                let c = [(g.uv_min[0] + g.uv_max[0]) * 0.5, (g.uv_min[1] + g.uv_max[1]) * 0.5];
                ([c[0] - 0.0001, c[1] - 0.0001], [c[0] + 0.0001, c[1] + 0.0001])
            } else {
                (g.uv_min, g.uv_max)
            };
            // `NFS_TEXT_SOLID=1` swaps the atlas for the 1×1 white texture and collapses the UV
            // window to one texel, so a glyph quad becomes a plain filled rectangle — the probe
            // that showed the quads composite at about 8 % whatever they sample (`AUDIT.md` F2).
            // `NFS_TEXT_BIG=1` draws the quads four times the size
            let size = if std::env::var("NFS_TEXT_BIG").is_ok() { [size[0] * 4.0, size[1] * 4.0] } else { size };
            let tex = if solid { self.white } else { f.texture() };
            out.push(Sprite { texture: tex, pos, size, uv_min: uv0, uv_max: uv1, color, rotation: 0.0, pivot: [0.0, 0.0], tri: None, corner_colors: None, depth: None });
            // `NFS_TEXT_AB=1` repeats each glyph one line lower **in red**, same texture, same UV
            // window, same size — so one image carries the widget's text twice and the only
            // difference between the two is `color.rgb`. The red copy draws and the black original
            // does not (`AUDIT.md` F2).
            if std::env::var("NFS_TEXT_AB").is_ok() {
                out.push(Sprite { texture: f.texture(), pos: [pos[0], pos[1] + size[1] * 1.2], size, uv_min: uv0, uv_max: uv1, color: [1.0, 0.0, 0.0, 1.0], rotation: 0.0, pivot: [0.0, 0.0], tri: None, corner_colors: None, depth: None });
            }
        }
    }
}
