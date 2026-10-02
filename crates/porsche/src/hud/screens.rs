//! The HUD's result pages, dialogs, buttons and tables.

use crate::*;

impl Hud {
    /// `HTable` (ctor 0x42E8B0, draw 0x42EA50) inside the billboard box: the column widths it is
    /// given, centred — `left = x0 + (w − Σwidths)/2`, `top = y0 + (h − (rows + 1)·20)/2` — with a
    /// 20-px header row filled 0xFFFAB400, the body filled 0x40FFFFFF, the selected row 0x80FAB400,
    /// black grid lines (`[0x5B3064]`), and every cell's text black (`unviers9b`, 0xFF000000) at
    /// `cell left + 4`, centred vertically in its row. A cell with a colour draws an 18 × 6 swatch
    /// at `(x, row top + 7)` and its text 22 px further right (0x42F0D0; black when the colour has no
    /// alpha byte). The first header is the empty string at `0x5B2E08`. `unviers9b` is the HUD's
    /// `FONT_SMALL`, drawn from its own atlas.
    pub(crate) fn table(&self, out: &mut Vec<Sprite>, aspect: f32, widths: &[f32], head: &[String], rows: &[Vec<(String, Option<[f32; 4]>)>], selected: Option<usize>) {
        let (bx, by, bw, bh) = BILLBOARD;
        let n = 8.0;
        let total: f32 = widths.iter().sum();
        let left = (bx + ((bw - total) / 2.0).trunc()).trunc();
        let top = by + ((bh - (n + 1.0) * 20.0) / 2.0).trunc();
        let argb = |c: u32| [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
        out.push(self.quad(aspect, left, top, total, 20.0, argb(0xFFFA_B400)));
        out.push(self.quad(aspect, left, top + 20.0, total, n * 20.0, argb(0x40FF_FFFF)));
        if let Some(k) = selected {
            out.push(self.quad(aspect, left, top + 20.0 * (k as f32 + 1.0), total, 20.0, argb(0x80FA_B400)));
        }
        let black = argb(0xFF00_0000);
        let full = (n + 1.0) * 20.0;
        // the header's raised 4-px bevel 0x436420(left, top, total, 20, 4): white 0xCC → 0x19 inward
        // along the top and left (0x5B3478, 0x5B34A8), black 0x19 → 0xCC outward along the right and
        // bottom (0x5B3488, 0x5B3498); four full strips drawn top, right, bottom, left
        // (0x4366CC…0x4366FF, read by emulating its stores 2026-09-24)
        let [_, _, dark_v, dark_h, light_v, light_h] = self.button_ramps;
        let rev = |mut sp: Sprite| {
            std::mem::swap(&mut sp.uv_min, &mut sp.uv_max);
            sp
        };
        let b = 4.0;
        out.push(rev(Self::ramp(aspect, light_v, left, top, total, b, true)));
        out.push(rev(Self::ramp(aspect, dark_h, left + total - b, top, b, 20.0, false)));
        out.push(rev(Self::ramp(aspect, dark_v, left, top + 20.0 - b, total, b, true)));
        out.push(rev(Self::ramp(aspect, light_h, left, top, b, 20.0, false)));
        // the grid (0x42EB3E…0x42ED15, 0x42ED1D…0x42EE03): five strokes in `[0x5B3064]` black, each
        // 0x4359A0 between two corners of the header quad `A` and the body quad `B` (0x435920 builds
        // `(x,y) (x+w,y) (x+w,y+h) (x,y+h)`): A0→B3 the left edge, A1→B2 the right, A0→A1 the top,
        // A3→A2 the header's underline, B3→B2 the bottom — and `columns − 1` column
        // separators that start **below the header** (0x4359E0 moves the top end down 0x14) and run to
        // the bottom. There is no line per row: the only loop over rows draws text. This port drew a
        // line under every row and ran the separators through the header.
        for yy in [top, top + 20.0, top + full] {
            out.push(self.quad(aspect, left, yy, total, 1.0, black));
        }
        out.push(self.quad(aspect, left, top, 1.0, full, black));
        out.push(self.quad(aspect, left + total, top, 1.0, full, black));
        let mut cx = left;
        for w in widths.iter().take(widths.len().saturating_sub(1)) {
            cx += w;
            out.push(self.quad(aspect, cx, top + 20.0, 1.0, full - 20.0, black));
        }
        let th = FONT_SMALL;
        let row_text = |y: f32| y + ((20.0 - EXT_SMALL) / 2.0).trunc();
        let mut cx = left;
        for (i, h) in head.iter().enumerate() {
            self.text(out, aspect, h, cx + 4.0, row_text(top), th, black);
            cx += widths.get(i).copied().unwrap_or(0.0);
        }
        for (k, row) in rows.iter().take(n as usize).enumerate() {
            let ry = top + 20.0 * (k as f32 + 1.0);
            let mut cx = left;
            for (i, (t, colour)) in row.iter().enumerate() {
                let mut tx = cx + 4.0;
                if let Some(c) = colour {
                    // 0x42F0D0: the fill is the colour, or opaque black when it has no alpha byte
                    // (0x42F0F2…0x42F0FE), then its four edges stroked in `[0x5B306C]` = 0xFF000000
                    let fill = if c[3] > 0.0 { *c } else { black };
                    let (sx, sy) = (tx, ry + 7.0);
                    out.push(self.quad(aspect, sx, sy, 18.0, 6.0, fill));
                    out.push(self.quad(aspect, sx, sy, 18.0, 1.0, black));
                    out.push(self.quad(aspect, sx, sy + 5.0, 18.0, 1.0, black));
                    out.push(self.quad(aspect, sx, sy, 1.0, 6.0, black));
                    out.push(self.quad(aspect, sx + 17.0, sy, 1.0, 6.0, black));
                    tx += 22.0;
                }
                self.text(out, aspect, t, tx, row_text(ry), th, black);
                cx += widths.get(i).copied().unwrap_or(0.0);
            }
        }
    }

    /// Results table (HBillBoard, hud.md §5/§9): rank, NAME, CAR, BEST LAP, TIME; dashes for no time.
    /// `rows` = (name, car, best lap ticks, total ticks) in finishing order; `player` = the player's row.
    /// `HRaceSummary` 0x42BCD0 / 0x42C080: the billboard's four columns and eight ordinal rows.
    /// The column headers are `FEStrings` **18111…18114** (`NAME`, `CAR`, `BEST LAP`, `TIME`), the
    /// row labels **18116…18123** (`1st`…`8th`), and a car with no time prints
    /// `"--------------"` rather than a dash. `title` is the billboard's, by game type: 18130
    /// `Race Summary`, 18129 `Single Race`, 18169 `Knockout`, 18175 `Quick Knockout`
    /// (`RE/notes/hud.md` §5). The box's own geometry is still this port's — see the same section.
    #[allow(clippy::too_many_arguments)]
    pub fn results_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        rows: &[SummaryRow],
        player: usize,
        bb: &Billboard,
        head: &[String],
        widths: &[f32],
        best_lap: bool,
        points: bool,
        ord: &[String],
    ) {
        if self.font.is_none() {
            return;
        }
        if !self.billboard(out, aspect, bb) {
            return;
        }
        // 0x42C1D9…0x42C1F4: the summary selects the player's row — `HTable` slot 2 (0x42F290,
        // `+0x7AC`) is given `car+0xCD4`, the 1-based final place, i.e. the row under the header
        let none = || "--------------".to_string();
        let rows: Vec<Vec<(String, Option<[f32; 4]>)>> = rows
            .iter()
            .enumerate()
            .map(|(k, r)| {
                // the name is cut to 31 characters (`strncpy …, 0x1F`), the car's short name to 15
                // and marked with its paint swatch (`HTable` slot 5 at 0x42C3DD)
                let mut cells = vec![
                    (ord.get(k).cloned().unwrap_or_default(), None),
                    (r.name.chars().take(31).collect::<String>(), None),
                    (r.car.chars().take(15).collect::<String>(), Some(r.colour)),
                ];
                if best_lap {
                    cells.push((r.best.map(|t| format_time_dword(t, false)).unwrap_or_else(none), None));
                }
                cells.push((r.total.map(|t| format_time_dword(t, false)).unwrap_or_else(none), None));
                // POINTS: `TOURNAMENTRACEPOINTS[row]` (0x42C4AE), which 0x4EC689 sets to 8, 7, … 1
                if points {
                    cells.push(((8 - k.min(7)).to_string(), None));
                }
                cells
            })
            .collect();
        let mut header = vec![String::new()];
        header.extend(head.iter().cloned());
        self.table(out, aspect, widths, &header, &rows, Some(player));
    }

    /// `HCongrats` 0x41D050, `t` ticks after it opened, for `place` 1–3. Per frame: the frame lines
    /// 0x41D3B0, the three circles 0x41D8B0, then the digit.
    /// * Lines: a horizontal and a vertical line through the centre across the whole screen, colour
    ///   0x66FFFFFF (0x41D3B9…0x41D48A); then, in the place colour, two of the four outer arms (screen
    ///   edge to circle) and two of the four radii (centre to circle, `r = _ftol(128·scale·0.5)`),
    ///   picked per ring step by the tables at `[esp+0x28]` / `+0x3C` / `+0x4C` / `+0x38`
    ///   (0x41D63B, 0x41D694, 0x41D815, 0x41D89A — each read after its own stack shift).
    /// * Circles: `circlefull`, `circleoutline`, `congrats1`, each 128 × `scale`, centred and
    ///   untinted — opaque white, `HGraphicVisual`'s default vertex colour (0x422E70…0x422E83).
    ///   `congrats1` is mirrored per ring step: 0x423C70 sets `+0xEC` and 0x423D60 `+0xED` from the
    ///   byte tables at 0x41D976, the draw 0x423A10 picks UV set `(+0xED≠0) + 2·(+0xEC≠0)`, and
    ///   0x4238F0 builds the sets normal / mirrored in x / mirrored in y / both — so the steps show
    ///   y, both, normal, y, x.
    /// * The ring counter `+0x70` steps every nine ticks through five states (period 8 with a strict
    ///   `>`, count 5, 0x41DDE6), starting one step in because its stamp starts at 0.
    /// * Race place (`tournament == false`): the solid digit in the place colour, alpha
    ///   `min(t·255/128, 255)` (0x41D282…0x41D29D → 0x41DBA0).
    /// * Tournament place: before 0xB0 ticks an outline digit rolls through 1…8, one step every five
    ///   ticks (the counter at `+0x88`, period 4 with a strict `>`), in the place colour, with the
    ///   previous digit ghosted at alpha 0x80 on the step (0x41D9F0); from 0xB0 to 0xC0 the white
    ///   solid digit at ×2, ×4, ×6 its size (`_ftol((t − 176) · 0.0625 · −3)` picks from
    ///   {2, 4, 6}, 0x41DC70); from 0xC0 the solid digit in the place colour.
    pub fn congrats_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, place: usize, t: u64, tournament: bool) {
        if !(1..=3).contains(&place) {
            return;
        }
        // on the design screen (its centre, its `W·0.2` corner, lines to its edges), then back into
        // the layout's coordinates
        let (sw, sh) = Self::screen();
        let (ox, oy) = Self::origin();
        let (cx, cy, scale) = congrats_layout(t, tournament, sw, sh);
        let (cx, cy) = (cx - ox, cy - oy);
        let (l, r, top, bot) = Self::edges();
        let rgb = CONGRATS_RGB[place - 1];
        let col = |a: u32| {
            [((rgb >> 16) & 0xFF) as f32 / 255.0, ((rgb >> 8) & 0xFF) as f32 / 255.0, (rgb & 0xFF) as f32 / 255.0, a as f32 / 255.0]
        };
        let seg = |out: &mut Vec<Sprite>, (x0, y0): (f32, f32), (x1, y1): (f32, f32), color: [f32; 4]| {
            let (x, y) = (x0.min(x1), y0.min(y1));
            let mut l = self.image(aspect, self.white, x, y, (x1 - x0).abs().max(1.0), (y1 - y0).abs().max(1.0));
            l.color = color;
            out.push(l);
        };
        let white66 = [1.0, 1.0, 1.0, 0x66 as f32 / 255.0];
        seg(out, (l, cy), (r, cy), white66);
        seg(out, (cx, top), (cx, bot), white66);
        let ring_step = ((1 + t / 9) % 5) as usize;
        let rr = ((128.0f32 * scale * 0.5) as i64) as f32;
        // outer arms: 0 left, 1 bottom, 2 top, 3 right; radii: 0 up, 1 down, 2 left, 3 right
        let outer = [((l, cy), (cx - rr, cy)), ((cx, cy + rr), (cx, bot)), ((cx, top), (cx, cy - rr)), ((cx + rr, cy), (r, cy))];
        let inner = [((cx, cy), (cx, cy - rr)), ((cx, cy), (cx, cy + rr)), ((cx, cy), (cx - rr, cy)), ((cx, cy), (cx + rr, cy))];
        const OUTER: [[usize; 2]; 5] = [[0, 1], [0, 1], [0, 2], [2, 3], [3, 1]];
        const INNER: [[usize; 2]; 5] = [[0, 3], [0, 3], [3, 1], [2, 1], [0, 2]];
        for k in OUTER[ring_step] {
            seg(out, outer[k].0, outer[k].1, col(0xFF));
        }
        for k in INNER[ring_step] {
            seg(out, inner[k].0, inner[k].1, col(0xFF));
        }
        let r = 128.0 * scale;
        for tex in [self.congrats.full, self.congrats.ring].into_iter().flatten() {
            out.push(self.image(aspect, tex, cx - r * 0.5, cy - r * 0.5, r, r));
        }
        if let Some(tex) = self.congrats.cut {
            // UV set per step: 2 (y), 3 (both), 0, 2 (y), 1 (x)
            let set = [2, 3, 0, 2, 1][ring_step];
            let mut sp = self.image(aspect, tex, cx - r * 0.5, cy - r * 0.5, r, r);
            if set & 1 != 0 {
                (sp.uv_min[0], sp.uv_max[0]) = (1.0, 0.0);
            }
            if set & 2 != 0 {
                (sp.uv_min[1], sp.uv_max[1]) = (1.0, 0.0);
            }
            out.push(sp);
        }
        let put = |out: &mut Vec<Sprite>, tex: Option<TextureId>, (w, h): (f32, f32), k: f32, color: [f32; 4]| {
            if let Some(tex) = tex {
                let (w, h) = (w * k, h * k);
                let mut sp = self.image(aspect, tex, cx - w * 0.5, cy - h * 0.5, w, h);
                sp.color = color;
                out.push(sp);
            }
        };
        let i = place - 1;
        if !tournament {
            let a = ((t * 255) / 128).min(255) as u32;
            put(out, self.congrats.solid[i], CONGRATS_SOLID[i], scale, col(a));
        } else if t < 0xB0 {
            let roll = ((1 + t / 5) % 8) as usize;
            put(out, self.congrats.outline[roll], CONGRATS_OUTLINE[roll], scale, col(0xFF));
            if t % 5 == 0 {
                let prev = (roll + 7) % 8;
                put(out, self.congrats.outline[prev], CONGRATS_OUTLINE[prev], scale, col(0x80));
            }
        } else if t < 0xC0 {
            let k = ((t - 176) as f32 * 0.0625 * -3.0) as i64;
            let mult = [2.0, 4.0, 6.0][(-k).clamp(0, 2) as usize];
            put(out, self.congrats.solid[i], CONGRATS_SOLID[i], mult, [1.0; 4]);
        } else {
            put(out, self.congrats.solid[i], CONGRATS_SOLID[i], scale, col(0xFF));
        }
    }

    /// `HMissionSummary` 0x428940: an `HBillBoard` titled 18134 "Factory Driver" over 18135 "Mission
    /// Summary" (0x42894F / 0x428963), the narrator's portrait `factorypeople.fsh[CHARACTER]`
    /// (54×64, 0x428A19) with the name 18176 + CHARACTER under it (`0x4700 +`, 0x428BBE, 66 px
    /// below, 0x428C1B), the result lines typed out from `(W/2 − w/4, H/2 − h/4)`, and the "race
    /// again" button (18109, `+0x124`). The box is the dialogs' shared 500 × 325.
    #[allow(clippy::too_many_arguments)]
    pub fn mission_summary_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, bb: &Billboard, narrator: Option<usize>, name: &str, lines: &[String], race_again: &str, typed: Option<usize>, button: (bool, bool)) {
        if self.font.is_none() {
            return;
        }
        let (x, _, _, h) = BILLBOARD;
        if !self.billboard(out, aspect, bb) {
            return;
        }
        // portrait 54×64 at (origin.x + 0x14, H/2 − 64) (0x428A19), the name 66 px under it
        // (0x428C1B), the "race again" button at (W/2 − 71, H/2 + h/2 − 4·19) (0x428A2F…0x428A6F)
        let py = HUD_REF_H / 2.0 - 64.0;
        if let Some(tex) = narrator.and_then(|n| self.portraits.get(n).copied().flatten()) {
            out.push(self.image(aspect, tex, x + 20.0, py, 54.0, 64.0));
        }
        // the name (`+0x144`, 0x428B7B…0x428C22): `unviers9b` in 0xCC000000, centred under the
        // 54-px portrait (`[0x5B2C80]`), or at `x + 2` when it is wider (flag `+0x148`)
        let nh = FONT_SMALL;
        let nw = self.text_width(name, nh);
        let nx = if nw < 54.0 { (x + 20.0 + (54.0 - nw) * 0.5).trunc() } else { x + 20.0 + 2.0 };
        self.text(out, aspect, name, nx, py + 66.0, nh, [0.0, 0.0, 0.0, 0xCC as f32 / 255.0]);
        let (bx, by) = Self::mission_button();
        self.button_sprites(out, aspect, bx, by, race_again, button.0, button.1);
        // the result text (`+0x12C`, 0x428AEB…0x428B06): `unviers9b` in 0xFF000000, one visual moved
        // line by line from `(W/2 − w/4, H/2 − h/4)` (`+0x130`/`+0x134`, 0x428B15…0x428B67), each line
        // `measured height · [0x5B2CC0] 1.5` below the last (0x4291FC…0x429210), and **typed out**: the
        // lines before `+0x138` whole, the current one's first `+0x13C` characters, one more each
        // draw (0x429233…0x429293). `typed` is the characters shown so far (None: all).
        let (_, _, bw0, _) = BILLBOARD;
        let lx = (HUD_REF_W / 2.0 - (bw0 / 4.0).trunc()).trunc();
        let mut ly = (HUD_REF_H / 2.0 - (h / 4.0).trunc()).trunc();
        let th = FONT_SMALL;
        let mut left = typed.unwrap_or(usize::MAX);
        for l in lines {
            let n = l.chars().count();
            let shown: String = l.chars().take(left.min(n)).collect();
            self.text(out, aspect, &shown, lx, ly, th, [0.0, 0.0, 0.0, 1.0]);
            ly += EXT_SMALL * 1.5;
            if left <= n {
                break;
            }
            left -= n;
        }
    }

    /// An `HBillBoard` frame (layout 0x41AE20, draw 0x41A7A0): the box, a 22-px strip along its top
    /// and one along `h − 44` (0x435920 twice, `[0x5B265C]` = 22.0), the left text at `left + 10`
    /// and the right one right-aligned at `right − 10` (`[0x5B241C]` = 10.0), each centred
    /// vertically in its strip (`(22 − text height)·0.5`), all four in `UniversC12b`, white.
    /// The text is drawn only once the box is fully open (state 3, 0x41AC11). The text height is the measured extent `EXT_MAIN` (0x431400).
    ///
    /// **Opening** (0x41A7A0 state 1): the box starts 40 px tall (`+0xE8 = 0x28`, 0x41A69B) and
    /// grows by `h/5` (`+0xEC`, 65) each frame, centred, until it reaches `h` (state 3); only then
    /// are its texts and the page's own content drawn (0x41AC11, and each page's `0x42A410`
    /// test). Returns whether it is fully open.
    pub(crate) fn billboard(&self, out: &mut Vec<Sprite>, aspect: f32, bb: &Billboard) -> bool {
        let (x, _, w, full) = BILLBOARD;
        let h = self.billboard_h.get().min(full);
        let y = ((HUD_REF_H - h) / 2.0).trunc();
        // the box (`+0x9C`, 0x41A890…0x41A89C): its four corners from 0x5B2858 — 0xB2FEDD81 at
        // the top, 0x8082A8B0 at the bottom
        let tex_quad = |tex: TextureId, x: f32, y: f32, w: f32, h: f32, uv_min: [f32; 2], uv_max: [f32; 2]| {
            let (pos, size) = Self::place(aspect, x, y, w, h);
            Sprite { texture: tex, pos, size, uv_min, uv_max, color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None }
        };
        out.push(tex_quad(self.billboard_fill, x, y, w, h, [0.5, 0.25], [0.5, 0.75]));
        // the two 22-px strips (0x41A97D…0x41AC02): each a pair of black quads, 0x5B2898 on the
        // left (alpha 0x80 at its outer edge to 0xFF inward) and 0x5B28A8 on the right (the
        // mirror), meeting at a moving point. 0x41A8A1…0x41A91E: `v = ftol(fmod([0x606AC8], 32)) ·
        // w >> 5`, so the point crosses the whole width every 32 ticks, and `+0x110` flips each time
        // `v` wraps (it starts 0 at the constructor); the top strip meets at `left + v` while the
        // flag is clear and `right − v` while it is set, the bottom one the other way round
        // (0x41AA75…0x41AA83)
        let (clock, born) = self.billboard_clock.get();
        let v = ((clock % 32) as f32 * w / 32.0).trunc();
        let flipped = (clock / 32).saturating_sub(born / 32) % 2 == 1;
        for (k, sy) in [y, y + h - 44.0].into_iter().enumerate() {
            if sy < y || sy + 22.0 > y + h {
                continue;
            }
            let from_left = flipped == (k == 1);
            let split = if from_left { x + v } else { x + w - v };
            if split > x {
                out.push(tex_quad(self.strip_shade, x, sy, split - x, 22.0, [0.25, 0.5], [0.75, 0.5]));
            }
            if split < x + w {
                out.push(tex_quad(self.strip_shade, split, sy, x + w - split, 22.0, [0.75, 0.5], [0.25, 0.5]));
            }
        }
        if h < full {
            return false;
        }
        // once open, under the texts (the `HTextVisual`s draw after their parent), (0x41ACC1…0x41AD81): the raised 4-px border 0x435A20(x, y, w, h, 1, 4) outside
        // the box — white along the top and left, black along the bottom and right, 0x05 at the outer
        // edge to 0x4C against the box — and the gloss 0x435DA0(…, 100) over both 22-px strips
        // 0x435A20(x, y, w, h, 1, 4), read 2026-09-24 by emulating its stores: four **mitred**
        // trapezoids from the outer rectangle to the box's edge, drawn top, right, bottom, left
        // (0x435D05…0x435D3B), with the mode-1 tables 0x5B3398…0x5B33C8 — white 0x05 at the outer
        // edge to 0x4C at the box on the top and left, black the same on the right and bottom
        self.bevel(out, aspect, (x, y, w, h), 4.0, 1);
        self.gloss(out, aspect, x, y, w, 22.0);
        self.gloss(out, aspect, x, y + h - 44.0, w, 22.0);
        let white = [1.0, 1.0, 1.0, 1.0];
        let th = FONT_MAIN;
        let strip = |top: f32| (top + (22.0 - EXT_MAIN) * 0.5).trunc();
        self.text(out, aspect, bb.left, x + 10.0, strip(y), th, white);
        let rw = self.text_width(bb.right, th);
        self.text(out, aspect, bb.right, x + w - 10.0 - rw, strip(y), th, white);
        if let Some((esc, space)) = bb.hints {
            let by = strip(y + h - 44.0);
            self.text(out, aspect, esc, x + 10.0, by, th, white);
            if let Some(sp) = space {
                let sw = self.text_width(sp, th);
                self.text(out, aspect, sp, x + w - 10.0 - sw, by, th, white);
            }
        }
        true
    }

    /// One `HButtonVisual` (ctor 0x41B140, geometry 0x41B520, draw 0x41B340): a 142 × 19 box
    /// filled **0xFFBC0000** (red) with its label in white, or — while the mouse is over it
    /// (`+0x100`, set from the cursor 0x5372A0(6) against the box, with sound 9 on entering,
    /// 0x41B3BA…0x41B3CE) — filled **0xFFFAB400** with the label in **0xFF000000**; the fill is
    /// 0x419F50 and the outline strokes 0x419E90 in 0xFF000000 (`[0x5B28F8]`). The original picks
    /// with the mouse only, and so does this port (`App::hud_click`); the arrow keys and Enter it
    /// also takes on the pause menu and the quit question are its own, and the button they select
    /// wears the hover look (PORT-DEVIATION D10 (PORT-DEVIATIONS.md) [0x41B3BA…0x41B3CE, 0x419F50] — the original has no keyboard selection there); the
    /// `+0x103` dressing (0x436130 / 0x435DA0 / 0x435F80) is drawn below.
    /// A two-texel gradient texture stretched over a rectangle, sampled texel centre to texel
    /// centre, top to bottom (`vertical`) or left to right.
    pub(crate) fn ramp(aspect: f32, tex: TextureId, x: f32, y: f32, w: f32, h: f32, vertical: bool) -> Sprite {
        let (pos, size) = Self::place(aspect, x, y, w, h);
        let (uv_min, uv_max) = if vertical { ([0.5, 0.25], [0.5, 0.75]) } else { ([0.25, 0.5], [0.75, 0.5]) };
        Sprite { texture: tex, pos, size, uv_min, uv_max, color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None }
    }

    /// **0x435A20(x, y, w, h, style, b)**: four mitred `b`-px trapezoids outside the box, drawn top,
    /// right, bottom, left, each from its outer edge (two corners) to the box's edge (two corners)
    /// in the style's tables — style 0 0x5B3358…0x5B3388 (top and left 0x05FFFFFF outside to
    /// 0x4C000000 against the box, right and bottom 0x4C000000 outside to 0x05FFFFFF — sunken; the
    /// rear-view mirror's), style 1 0x5B3398…0x5B33C8 (top and left 0x05FFFFFF → 0x4CFFFFFF, right
    /// and bottom 0x05000000 → 0x4C000000 — raised; the billboard's). Read by emulating its stores
    /// 2026-09-24 (`hud.md` §5).
    pub(crate) fn bevel(&self, out: &mut Vec<Sprite>, aspect: f32, (x, y, w, h): (f32, f32, f32, f32), b: f32, style: u8) {
        let (o0, o1) = ((x - b, y - b), (x + w + b, y + h + b));
        let (i0, i1) = ((x, y), (x + w, y + h));
        let argb = |c: u32| [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
        // (outer, inner) per side, top / right / bottom / left
        let tables: [(u32, u32); 4] = match style {
            0 => [(0x05FF_FFFF, 0x4C00_0000), (0x4C00_0000, 0x05FF_FFFF), (0x4C00_0000, 0x05FF_FFFF), (0x05FF_FFFF, 0x4C00_0000)],
            _ => [(0x05FF_FFFF, 0x4CFF_FFFF), (0x0500_0000, 0x4C00_0000), (0x0500_0000, 0x4C00_0000), (0x05FF_FFFF, 0x4CFF_FFFF)],
        };
        let sides = [
            [(o0.0, o0.1), (o1.0, o0.1), (i1.0, i0.1), (i0.0, i0.1)],
            [(o1.0, o0.1), (o1.0, o1.1), (i1.0, i1.1), (i1.0, i0.1)],
            [(o1.0, o1.1), (o0.0, o1.1), (i0.0, i1.1), (i1.0, i1.1)],
            [(o0.0, o1.1), (o0.0, o0.1), (i0.0, i0.1), (i0.0, i1.1)],
        ];
        for (pts, (co, ci)) in sides.into_iter().zip(tables) {
            let (co, ci) = (argb(co), argb(ci));
            self.trapezoid(out, aspect, pts, [co, co, ci, ci]);
        }
    }

    /// A four-cornered polygon in per-corner colours, as two triangles (0x419F90's quads).
    fn trapezoid(&self, out: &mut Vec<Sprite>, aspect: f32, p: [(f32, f32); 4], c: [[f32; 4]; 4]) {
        let pt = |(x, y): (f32, f32)| Self::place(aspect, x, y, 0.0, 0.0).0;
        for (a, b2, d) in [(0, 1, 2), (0, 2, 3)] {
            let mut sp = self.quad(aspect, 0.0, 0.0, 0.0, 0.0, c[a]);
            sp.tri = Some(([pt(p[a]), pt(p[b2]), pt(p[d])], [c[a], c[b2], c[d]]));
            out.push(sp);
        }
    }

    /// The gloss 0x435DA0(x, y, w, h, 100): white 127 → 0 over the top half, black 0 → 127 below.
    pub(crate) fn gloss(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32) {
        let half = (h / 2.0).trunc();
        out.push(Self::ramp(aspect, self.button_ramps[0], x, y, w, half, true));
        out.push(Self::ramp(aspect, self.button_ramps[1], x, y + half, w, h - half, true));
    }

    /// `hovered` is `+0x100`; `pressed` is the pointer's button held (`[p+0x10]` of 0x5372A0(6),
    /// 0x41B386), which with `hovered` gives the sunken bevel (0x41B4B2…0x41B4D9).
    #[allow(clippy::too_many_arguments)]
    fn button_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, label: &str, selected: bool, pressed: bool) {
        let (bw, bh) = HUD_BUTTON;
        let argb = |c: u32| [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
        let (fill, text) = if selected { (0xFFFA_B400, 0xFF00_0000) } else { (0xFFBC_0000, 0xFFFF_FFFF) };
        out.push(self.quad(aspect, x, y, bw, bh, argb(fill)));
        let edge = argb(0xFF00_0000);
        out.push(self.quad(aspect, x, y, bw, 1.0, edge));
        out.push(self.quad(aspect, x, y + bh - 1.0, bw, 1.0, edge));
        out.push(self.quad(aspect, x, y, 1.0, bh, edge));
        out.push(self.quad(aspect, x + bw - 1.0, y, 1.0, bh, edge));
        // `+0x103`, set by every in-race dialog's buttons (the sixth constructor argument, 1 at
        // 0x42A4EC and its siblings): hovered **with the button held** (or `+0x102`, which only
        // 0x430CD6 sets), the 4-px bevel 0x436130 (`[0x5CC910]` = 4) — dark
        // 0xCC → 0x19 on the top and left, light 0x19 → 0xCC on the bottom and right; otherwise the
        // gloss 0x435DA0(…, 100); then the 5 × 5 drop shadow 0x435F80 (0xCC → 0x19 away from the box).
        // 0x436130's four quads, read 2026-09-24 by emulating its stores, are plain strips — the top
        // and bottom the full width, the sides the full height — drawn top, right, bottom, left
        // with the tables 0x5B3438 / 0x5B3448 / 0x5B3458 / 0x5B3468.
        let ramp = |tex: TextureId, x: f32, y: f32, w: f32, h: f32, vertical: bool| Self::ramp(aspect, tex, x, y, w, h, vertical);
        let [_, _, dark_v, dark_h, light_v, light_h] = self.button_ramps;
        let b = 4.0;
        if selected && pressed {
            out.push(ramp(dark_v, x, y, bw, b, true));
            out.push(ramp(light_h, x + bw - b, y, b, bh, false));
            out.push(ramp(light_v, x, y + bh - b, bw, b, true));
            out.push(ramp(dark_h, x, y, b, bh, false));
        } else {
            self.gloss(out, aspect, x, y, bw, bh);
        }
        out.push(ramp(dark_h, x + bw, y + 5.0, 5.0, bh, false));
        out.push(ramp(dark_v, x + 5.0, y + bh, bw, 5.0, true));
        let tw = self.text_width(label, FONT_MAIN);
        self.text(out, aspect, label, x + (bw - tw) / 2.0, (y + (bh - EXT_MAIN) / 2.0).trunc(), FONT_MAIN, argb(text));
    }

    /// `HPauseMenu` (0x42A440): the billboard titled FE 18128 with three buttons — 18108 continue,
    /// 18109 race again, 18110 quit — at `x = W/2 − 71` and `y = ftol(H/2 − 1.5·step) + k·step`
    /// with `step = 19 · 1.6` (0x42A482…0x42A587, each `ftol`'d from the previous one).
    pub fn pause_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, bb: &Billboard, labels: &[String; 3], sel: usize, pressed: bool) {
        if !self.billboard(out, aspect, bb) {
            return;
        }
        for (k, (l, (bx, by))) in labels.iter().zip(Self::pause_buttons()).enumerate() {
            self.button_sprites(out, aspect, bx, by, l, k == sel, pressed);
        }
    }

    /// `HPauseMenu`'s three buttons' top-left corners (0x42A482…0x42A587).
    pub(crate) fn pause_buttons() -> [(f32, f32); 3] {
        let (bw, bh) = HUD_BUTTON;
        let step = bh * 1.6;
        let y0 = (HUD_REF_H / 2.0 - step * 3.0 * 0.5).trunc();
        let y1 = (y0 + step).trunc();
        let x = HUD_REF_W / 2.0 - bw / 2.0;
        [(x, y0), (x, y1), (x, (y1 + step).trunc())]
    }

    /// `HYesNoOverlay`'s yes / no buttons, two `ftol`'d steps under its two text lines (0x437482…
    /// 0x43760E).
    pub(crate) fn yes_no_buttons() -> [(f32, f32); 2] {
        let (bw, bh) = HUD_BUTTON;
        let step = bh * 1.4;
        let mut y = (HUD_REF_H / 2.0 - step * 4.0 * 0.5).trunc();
        for _ in 0..2 {
            y = (y + step).trunc();
        }
        let x = HUD_REF_W / 2.0 - bw / 2.0;
        [(x, y), (x, (y + step).trunc())]
    }

    /// **`HReplaySave`** (ctor 0x42E040, draw 0x42E2A0): the billboard `"" | 18136 "Save Replay"`
    /// with no hints (0x42E077, 500 × 325 `[0x5B2F78]`), and a column at `x = W/2 − 142/2` with
    /// `step = 19 · 1.4` (`fild [0x5B2F84]`, `fmul [0x5B2938]`): the name field 0x430C30 at
    /// `y0 = ftol(H/2 − 3·step/2)` (0x42E0C2…0x42E0D2), the buttons 18137 / 18138 (0x41B140, the
    /// sixth argument 1) at `ftol(y0 + step)` and one step below that, and the label 18170 in
    /// `UniversC12b` / **0xC0000000** at `ftol(that − 3·step)` (0x42E1C4…0x42E206) — 172, 200, 226,
    /// 252 at 640 × 480. The field is an `HButtonVisual` whose label is its own buffer `+0x108`
    /// with the plate `+0xFC` = 0xFFFAB400 (0x430CC8) and `+0x102` set (0x430CD6): always the
    /// orange plate, the black text and the sunken bevel; its draw 0x430D80 adds the caret `"I"`
    /// (0x5CD088) in 0xFF808080 (0x430C9D) one pixel right of the text's end (0x41B270 +1),
    /// shown and hidden every time `[0x606AC8]` has run 16 past the last flip (0x430D8C…0x430DB1,
    /// shown first, `+0x154 = 1` 0x430C74). The draw order is the box, the field, the two buttons,
    /// then the label (0x42E2A3…0x42E305), all once the box is open.
    #[allow(clippy::too_many_arguments)]
    pub fn replay_save_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, bb: &Billboard, label: &str, name: &str, caret: bool, buttons: [&str; 2], hover: Option<usize>, pressed: bool) {
        if !self.billboard(out, aspect, bb) {
            return;
        }
        let [field, b0, b1, label_y] = Self::replay_save_rows();
        let x = HUD_REF_W / 2.0 - HUD_BUTTON.0 / 2.0;
        self.button_sprites(out, aspect, x, field, name, true, true);
        if caret && self.font.is_some() {
            // 0x41B270: the text's own x (an integer) plus its measured width, `ftol`'d, then +1
            let tw = self.text_width(name, FONT_MAIN);
            let tx = (x + (HUD_BUTTON.0 - tw) / 2.0).trunc();
            let ty = (field + (HUD_BUTTON.1 - EXT_MAIN) / 2.0).trunc();
            self.text(out, aspect, "I", (tx + tw).trunc() + 1.0, ty, FONT_MAIN, [0x80 as f32 / 255.0, 0x80 as f32 / 255.0, 0x80 as f32 / 255.0, 1.0]);
        }
        for (k, (l, by)) in buttons.iter().zip([b0, b1]).enumerate() {
            self.button_sprites(out, aspect, x, by, l, hover == Some(k), pressed);
        }
        self.text(out, aspect, label, x, label_y, FONT_MAIN, [0.0, 0.0, 0.0, 0xC0 as f32 / 255.0]);
    }

    /// `HReplaySave`'s rows: the field, the two buttons and the label (0x42E0C2…0x42E206), each
    /// `ftol`'d from the one before — the step is a `float` (`fst [esp+0x10]`) and the label's
    /// `3·step` its extended product (`fmul [0x5B2600]`, stored as a float at 0x42E0B5).
    fn replay_save_rows() -> [f32; 4] {
        let step = (HUD_BUTTON.1 as f64 * 1.4) as f32;
        let three = (HUD_BUTTON.1 as f64 * 1.4 * 3.0) as f32;
        let field = ((HUD_REF_H / 2.0) as f64 - three as f64 * 0.5).trunc() as f32;
        let b0 = (field + step).trunc();
        let b1 = (b0 + step).trunc();
        [field, b0, b1, (b1 - three).trunc()]
    }

    /// `HReplaySave`'s two buttons' top-left corners.
    pub(crate) fn replay_save_buttons() -> [(f32, f32); 2] {
        let [_, b0, b1, _] = Self::replay_save_rows();
        let x = HUD_REF_W / 2.0 - HUD_BUTTON.0 / 2.0;
        [(x, b0), (x, b1)]
    }

    /// `HMissionSummary`'s "race again" button: `(W/2 − 71, H/2 + h/2 − 4·19)` (0x428A2F…0x428A6F).
    pub(crate) fn mission_button() -> (f32, f32) {
        let (bw, bh) = HUD_BUTTON;
        (HUD_REF_W / 2.0 - bw / 2.0, HUD_REF_H / 2.0 + (BILLBOARD.3 / 2.0).trunc() - 4.0 * bh)
    }

    /// `HButtonVisual`'s hover test (0x41B376…0x41B3B8): the pointer 0x5372A0(6) — whole pixels,
    /// `fild` — **strictly** inside the box `+0x14`/`+0x18` … `+0x24`/`+0x48`, i.e. `(x, y)` to
    /// `(x + 142, y + 19)`. Its answer is `+0x100`.
    pub(crate) fn over_button(at: (f32, f32), cursor: Option<(f32, f32)>) -> bool {
        let (bw, bh) = HUD_BUTTON;
        cursor.is_some_and(|(cx, cy)| {
            let (px, py) = (cx.floor(), cy.floor());
            px > at.0 && px < at.0 + bw && py > at.1 && py < at.1 + bh
        })
    }

    /// `HYesNoOverlay` (0x437440): the billboard ("" | 18174 "Confirmation", no hints) with a
    /// column at `x = W/2 − 71`, `step = 19 · 1.4` (`[0x5B2938]`), `y0 = ftol(H/2 − 2·step)`
    /// (`[0x5B2404]` = 4.0): two text lines (`+0x120`, `+0x124`, `UniversC12b` in **0xC0000000**)
    /// and then the 18141 "yes" / 18142 "no" buttons, each `ftol` one step below the last —
    /// 186 / 212 / 238 / 264 (0x437482…0x43760E). The question is 18171 on the first line.
    #[allow(clippy::too_many_arguments)]
    pub fn yes_no_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, bb: &Billboard, lines: [&str; 2], buttons: [&str; 2], sel: usize, pressed: bool) {
        if !self.billboard(out, aspect, bb) {
            return;
        }
        let (bw, bh) = HUD_BUTTON;
        let step = bh * 1.4;
        let x = HUD_REF_W / 2.0 - bw / 2.0;
        let mut y = (HUD_REF_H / 2.0 - step * 4.0 * 0.5).trunc();
        for l in lines {
            self.text(out, aspect, l, x, y, FONT_MAIN, [0.0, 0.0, 0.0, 0xC0 as f32 / 255.0]);
            y = (y + step).trunc();
        }
        for (k, (b, (bx, by))) in buttons.iter().zip(Self::yes_no_buttons()).enumerate() {
            self.button_sprites(out, aspect, bx, by, b, k == sel, pressed);
        }
    }

    /// A one-message box (`HInfo` 0x4282F0): one `HTextVisual` (`+0x124`, 0x428347…0x428366) in
    /// `UniversC12b` and **0xFF000000**, given no box to wrap in; its setter 0x428410 measures the text
    /// (0x431400) and centres it on the billboard, `x + w/2 − text w·0.5`, `y + h/2 − text h·0.5`
    /// (0x42846C…0x4284C0), once the box is open (0x4283E5). This port broke the text into 44-character
    /// lines in white from the top-left. The text height is the measured extent `EXT_MAIN` (0x431400).
    pub fn notice_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, bb: &Billboard, text: &str) {
        if self.font.is_none() {
            return;
        }
        let (x, y, w, h) = BILLBOARD;
        if !self.billboard(out, aspect, bb) {
            return;
        }
        let th = FONT_MAIN;
        let tw = self.text_width(text, th);
        let tx = (x + (w / 2.0).trunc() - tw * 0.5).trunc();
        let ty = (y + (h / 2.0).trunc() - EXT_MAIN * 0.5).trunc();
        self.text(out, aspect, text, tx, ty, th, [0.0, 0.0, 0.0, 1.0]);
    }

    /// The tournament standings (`HTournamentStandings` 0x431860, rows 0x431D33…0x431E46): ordinal,
    /// driver name, points or total time, winnings; the player's row selected (0x431E1A). Same box
    /// as the summary — `BILLBOARD`, 500 × 325 centred (0x431897 → 0x41A510, 0x41B100).
    pub fn standings_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        rows: &[(String, String, String)],
        player: Option<usize>,
        bb: &Billboard,
        head: &[String; 3],
        timed: bool,
        ord: &[String],
        winnings: Option<(&nfs_game::career::Winnings, [String; 4], Option<(f32, f32)>)>,
    ) {
        if self.font.is_none() {
            return;
        }
        if !self.billboard(out, aspect, bb) {
            return;
        }
        // 0x4319A1…0x4319BD: widths (40, 90, 100, 100) for points, (40, 90, 120, 100) for a
        // time-ranked club event (the RALLY TIME header set)
        let widths: &[f32] = if !timed { &[40.0, 90.0, 100.0, 100.0] } else { &[40.0, 90.0, 120.0, 100.0] };
        let rows: Vec<Vec<(String, Option<[f32; 4]>)>> = rows
            .iter()
            .enumerate()
            .map(|(k, (name, mid, money))| vec![(ord.get(k).cloned().unwrap_or_default(), None), (name.clone(), None), (mid.clone(), None), (money.clone(), None)])
            .collect();
        let mut header = vec![String::new()];
        header.extend(head.iter().cloned());
        self.table(out, aspect, widths, &header, &rows, player);
        // 0x431AF6…0x431B70: once the table is drawn, `HTable` 0x42F2A0(the player's row `+0x1EC`,
        // column 3) asks whether the pointer (0x5372A0(6), `+4`/`+8`) is **strictly** inside that
        // row's WINNINGS cell; if so 0x42F320 gives the cell's rectangle and `HWinnings` is placed
        // on its middle (0x431B37…0x431B63, `(x1 − x0)/2 + x0` as a signed halving) and drawn
        if let (Some((w, labels, Some((px, py)))), Some(row)) = (winnings, player) {
            let (x0, x1, y0, y1) = Self::table_cell(widths, row, 3);
            if px > x0 && px < x1 && py > y0 && py < y1 {
                let mid = |a: f32, b: f32| (((b - a) as i32) / 2 + a as i32) as f32;
                self.winnings_sprites(out, aspect, w, &labels, mid(x0, x1), mid(y0, y1));
            }
        }
    }

    /// `HTable` 0x42F320(row, column): a body cell's rectangle — `x0 = left + Σ widths[..column]`,
    /// `x1 = x0 + widths[column]`, `y0 = top + 20·(row + 1)`, `y1 = top + 20·(row + 2)` — with the
    /// table's `left` / `top` as [`Hud::table`] lays it out (`+0x788` / `+0x78C`). 0x42F2A0 tests the
    /// same bounds.
    pub(crate) fn table_cell(widths: &[f32], row: usize, column: usize) -> (f32, f32, f32, f32) {
        let (bx, by, bw, bh) = BILLBOARD;
        let total: f32 = widths.iter().sum();
        let left = (bx + ((bw - total) / 2.0).trunc()).trunc();
        let top = by + ((bh - 9.0 * 20.0) / 2.0).trunc();
        let x0 = left + widths.iter().take(column).sum::<f32>();
        let x1 = x0 + widths.get(column).copied().unwrap_or(0.0);
        let y0 = top + 20.0 * (row as f32 + 1.0);
        (x0, x1, y0, y0 + 20.0)
    }

    /// **`HWinnings`** (vtable 0x5B34D8; ctor 0x436860, layout 0x436DD0, draw 0x436990) — the
    /// tooltip over the player's WINNINGS cell, with the point `(cx, cy)` 0x431B63 gives it.
    ///
    /// * Two `HTextVisual`s in `unviers9b` (0x420600) and white (−1): `+0x10` left-aligned and
    ///   `+0x14` right-aligned (its eighth constructor argument 1 → `+0x18`, which 0x4312B0 answers by
    ///   moving the text left by its measured width, 0x4312D2…0x43131C). The row pitch `+0x64` is
    ///   `ftol(h·0.5 + h)` (0x4368E8…0x4368FD) of the measured height — the extent 0x431400 reports,
    ///   `EXT_SMALL` — so 19.
    /// * Layout 0x436DD0(cx, cy): the box is 200 wide (`+0x1C`) with its **left edge** at `cx`; its top
    ///   is `cy − (n − 1)·pitch` with `n = TOURNAMENTNUMRACESCOMPLETED`, pulled up to `H − ((2n + 3)·pitch
    ///   + 20) − 10` when `(2n + 3)·pitch + 20` below it would pass the screen's bottom (0x436DD5…
    ///   0x436E17); the quad `+0x24…+0x60` is `(x, y) (x + 200, y) (x + 200, y + h) (x, y + h)`.
    /// * Draw 0x436990: for each event slot with money, `"%s %s "` of FE 18165 "Event" and the
    ///   number, then `"%s "` of the prize; `"%s %s "` of FE 18166 "Safe Bonus" and the number, then
    ///   the bonus (0x4369C3…0x436B9E); on the last race FE 18167 "Tournament Bonus" and its amount
    ///   (0x436BBA…0x436CB6); always FE 18168 "Total" and the total (0x436CB7…0x436D8F) — the two FE
    ///   strings used as the format itself. Row `r` has its label at `(ftol(x + 10), ftol(pitch·r +
    ///   y + 10))` and its amount right-aligned at `x + 10 + 180` (`[0x5B241C]` 10.0, `+0xB4`), the
    ///   amounts `itoa` base 10 (0x5A19AC).
    /// * Last, the draw moves the box's bottom corners to `pitch + ` the Total row's y `+ 10`
    ///   (0x436D94…0x436DB0) and fills it through 0x419F90 in the four corners at 0x5CD20C — all
    ///   **0x80000000**, black at half alpha. It is queued after the texts, but the HUD pass flushes
    ///   the quad batch (0x41A330) before the text batch (0x4672A0 at 0x42592F / 0x425B1F /
    ///   0x425B86), the order every tray's text relies on, so the box lies under them (MEDIUM).
    fn winnings_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, w: &nfs_game::career::Winnings, labels: &[String; 4], cx: f32, cy: f32) {
        let pitch = (EXT_SMALL * 0.5 + EXT_SMALL).trunc();
        let n = w.completed as f32;
        let (bw, tall) = (200.0, (2.0 * n + 3.0) * pitch + 20.0);
        let mut y = cy - (n - 1.0) * pitch;
        let screen_bottom = Self::edges().3;
        if y + tall > screen_bottom {
            y = screen_bottom - tall - 10.0;
        }
        let x = cx;
        let mut lines: Vec<(String, u32)> = Vec::new();
        for &(k, prize, bonus) in &w.events {
            lines.push((format!("{} {} ", labels[0], k), prize));
            lines.push((format!("{} {} ", labels[1], k), bonus));
        }
        if let Some(b) = w.tournament_bonus {
            lines.push((labels[2].clone(), b));
        }
        lines.push((labels[3].clone(), w.total));
        let row_y = |r: usize| (pitch * r as f32 + y + 10.0).trunc();
        let bottom = row_y(lines.len() - 1) + pitch + 10.0;
        out.push(self.quad(aspect, x, y, bw, bottom - y, [0.0, 0.0, 0.0, 128.0 / 255.0]));
        let white = [1.0, 1.0, 1.0, 1.0];
        let lx = (x + 10.0).trunc();
        for (r, (label, amount)) in lines.iter().enumerate() {
            let ry = row_y(r);
            self.text(out, aspect, label, lx, ry, FONT_SMALL, white);
            let s = format!("{amount} ");
            let sw = self.text_width(&s, FONT_SMALL);
            self.text(out, aspect, &s, (lx + 180.0 - sw).trunc(), ry, FONT_SMALL, white);
        }
    }

    /// The post-race records table (`HTrackRecords` draw 0x433CF0): ordinal, NAME, CAR, TIME for the
    /// eight entries, the selected row in HUD orange. It is an `HTable` like the summary, so it is
    /// drawn in the same box — `BILLBOARD`, 500 × 325 centred (0x433874 → 0x41A510, 0x41B100).
    pub fn records_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        rec: &nfs_game::records::HudRecords,
        bb: &Billboard,
        head: &[String; 3],
        ord: &[String],
    ) {
        if self.font.is_none() {
            return;
        }
        if !self.billboard(out, aspect, bb) {
            return;
        }
        // 0x43394C: widths (40, 100, 120, 100)
        let rows: Vec<Vec<(String, Option<[f32; 4]>)>> = rec
            .rows
            .iter()
            .enumerate()
            .map(|(k, (name, car, time))| vec![(ord.get(k).cloned().unwrap_or_default(), None), (name.clone(), None), (car.clone(), None), (format_ticks(*time as u64), None)])
            .collect();
        let mut header = vec![String::new()];
        header.extend(head.iter().cloned());
        self.table(out, aspect, &[40.0, 100.0, 120.0, 100.0], &header, &rows, rec.highlight);
    }
}
