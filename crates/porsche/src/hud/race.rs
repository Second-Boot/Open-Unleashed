//! The in-race HUD: replay bar, map, trays, dials, damage and the sprite assembly.

use crate::*;

impl Hud {
    /// A blip's tint (the loop 0x433110, audited 2026-09-26). A racer takes its paint
    /// `[[car+0x558]+0x218]`, replaced by `0xFF202020` when **`(c & 0xFF0000) < 0x14`, `(c & 0xFF00)
    /// < 0x14` and `(c & 0xFF) < 0x14`** (0x4333AE…0x4333D3) — the masks are compared unshifted, so
    /// that is red 0, green 0 and blue under 0x14, not "every channel under 0x14" as this port had
    /// it. There is no case for the player: its blip is its own paint like any racer's.
    ///
    /// A cop (`car+0x52C & 0x20`) keeps a colour per car `[0x5CD158 + 4·i]` (0xC00000FF at load)
    /// and a stamp `[0x60FDA8 + 4·i]`: whenever the clock `[0x606AC4]` is **more than** 0x20 past
    /// the stamp (`sub; cmp ecx, 0x20; jle`, signed) the colour flips between 0xC00000FF and
    /// 0xC0FF0000 and the stamp is re-taken (0x43333E…0x433377) — [`Hud::cop_blip_step`], on the
    /// state `App::cop_blips` keeps. The corners then get `(c, other, other, c)`
    /// (0x43337E…0x4333DE: pushes `c`, `o`, `o`, `c` — 0x423D70's first argument is the first vertex
    /// slot `+0x5C`, the quad's upper-left, and the slots run UL, UR, LR, LL as 0x423C70 fills
    /// them), so the picture's left edge is `c` and its right edge `o`, shading across it
    /// ([`Sprite::corner_colors`]).
    pub(crate) fn blip_color(paint: [u8; 3], police: Option<u32>) -> [[f32; 4]; 4] {
        let argb = |c: u32| [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
        if let Some(c) = police {
            let other = if c == 0xC000_00FF { 0xC0FF_0000 } else { 0xC000_00FF };
            let (c, other) = (argb(c), argb(other));
            return [c, other, other, c];
        }
        let floored = if paint[0] == 0 && paint[1] == 0 && paint[2] < 0x14 {
            [0x20, 0x20, 0x20]
        } else {
            paint
        };
        [[floored[0] as f32 / 255.0, floored[1] as f32 / 255.0, floored[2] as f32 / 255.0, 1.0]; 4]
    }

    /// One map draw's pass over a cop's blip state (0x43333E…0x433377): when `tick − stamp > 0x20`
    /// (signed dwords) the colour flips — `0xC00000FF` ↔ `0xC0FF0000`, the `sub / neg / sbb / and /
    /// add` of 0x433361…0x433371 — and the stamp becomes the tick. Returns the colour drawn.
    pub(crate) fn cop_blip_step(state: &mut (u32, i32), tick: u64) -> u32 {
        let now = tick as u32 as i32;
        if now.wrapping_sub(state.1) > 0x20 {
            state.0 = if state.0 == 0xC000_00FF { 0xC0FF_0000 } else { 0xC000_00FF };
            state.1 = now;
        }
        state.0
    }

    /// **`HReplay`'s control bar** — `RE/notes/hud.md` §5.2. Seven 21×18 cells of pitch 51, centred
    /// and thirty pixels up from the bottom (`nfs_game::replay::control_bar`, which is 0x42DEF0's
    /// arithmetic), each holding a `replay.fsh` glyph centred in its own box.
    ///
    /// `paused` picks `play` over `paus` in cell 2 (the original keeps both widgets, `this+0xC8` and
    /// `this+0xCC`, and draws one — 0x42D31E switches on `this+0x199`), and cell 4 shows one of the
    /// four speed glyphs in cell 4 by the bar's own step `+0x174` (`BarState::glyph`). The frame the bar sits on is `this+0x164/+0x168`, three pixels up
    /// and left of the first cell. The widget is itself an `HTray` (vtable 0x5B2F3C, its slot 13
    /// 0x42DEF0 forwarding `(333, 24, 0)` to 0x4340A0), so the frame is a tray with no rule — but
    /// not an ordinary one (audit 2026-09-26): the constructor passes `+0xBC = 1` and `+0xC1 = 1`
    /// (0x42C919…0x42C93D), so the draw 0x434110 also lays the gloss 0x435DA0(x, y, w, h, pct) over
    /// the box (0x4342AC…0x4342C7, skipped with the box under `TRANSLUCENCY` ≠ 0), and `+0xBC`
    /// makes the tray **fade** in and out in place over 48 ticks (slot 14's `0x25 + 0xB`, the fill's
    /// alpha and the gloss's `pct` scaled by it, 0x4341C3…0x434200) instead of sliding. `pct` is that
    /// fade (`BarState::pct`, state 1 or 2; the bar opens and closes by `BarState::transition`,
    /// the auto-hide — 2026-09-27; until then it was drawn at 100 always).
    ///
    /// **The cells, read 2026-09-26** (0x42D1E0 / 0x42D4C0): the twelve widgets are `0x422DF0`
    /// pictures, not `HButtonVisual`s — no plate. Each glyph is tinted **0xFAB400** (the ctor's
    /// 0x423D70 calls, 0x42CEE3…0x42D033, and the draw's again at 0x42D2E6…0x42D2F9) with the alpha
    /// `pct·255/100`; each cell (0x42D4C0: the 21 × 18 box 0x435920, then four 0x4359A0 /
    /// 0x419E90 lines — top, the same 18 lower, and the two sides) is **outlined** in the same
    /// orange, at `pct·255/100` when the pointer is on it (0x42D6B0…0x42D6D5, the slot-9 test,
    /// edges inside) and at `pct·76/100` otherwise (0x42D5E6…0x42D5FC). State 3 would take `pct`
    /// from a local (0x42D2CE: the `push ecx` slot, `this`; 0x42D538: stack garbage) but the bar
    /// never reaches it — a fading tray is never moved to 3 (`BarState`); fully open is state 1 at
    /// 100 — 0xFF and 0x4C. `frozen` is `+0x199` (the play glyph, 0x42D31E), which moves
    /// with the replay pause `[0x606860]` (both flip on the pause cell and both are cleared as a
    /// playback starts, 0x413ECB / 0x426A89). The cells answer the pointer in
    /// `App::replay_bar_apply_press`, once per frame (`App::replay_bar_frame`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn replay_bar(&self, out: &mut Vec<Sprite>, aspect: f32, frozen: bool, network: bool, bar: &nfs_game::replay::BarState, pct: i32, cursor: Option<(f32, f32)>, tips: &[String; 7]) {
        use nfs_game::replay::{bar_cell_at, cell_glyph_origin, BAR, BAR_CELL, BAR_FRAME, BAR_PLAY};
        let (cells, frame) = Hud::bar_geometry();
        let (fw, fh) = BAR_FRAME;
        // the tray's fill alpha table and the gloss's 127 scaled by the fade (0x434234…0x43427B,
        // `a·pct/100`; 0x435EF5…0x435F24, `127·pct/100`)
        let n0 = out.len();
        self.tray(out, aspect, frame.0, frame.1, fw, fh);
        if self.boxes.get() {
            self.gloss(out, aspect, frame.0, frame.1, fw, fh);
        }
        let k = pct as f32 / 100.0;
        for sp in &mut out[n0..] {
            sp.color[3] *= k;
            if let Some((_, c)) = sp.tri.as_mut() {
                for v in c {
                    v[3] *= k;
                }
            }
        }
        // the pointer is whole pixels (0x5372A0(6) `+4` / `+8`)
        let over = cursor.and_then(|(x, y)| bar_cell_at(&cells, x.floor() as i32, y.floor() as i32)).filter(|&i| !(network && nfs_game::replay::is_cell_off_in_network(i)));
        // `(pct·255 / 100) << 24 | 0xFAB400` for the glyphs and the lit outline, `pct·76 / 100` for
        // the others (0x42D2D2…0x42D2EF, 0x42D53C…0x42D5FC; unsigned `mul` by 0x51EB851F, `>> 5`)
        let orange = |a: i32| [HUD_ORANGE[0], HUD_ORANGE[1], HUD_ORANGE[2], a as f32 / 255.0];
        let (lit, dim) = (orange(pct * 255 / 100), orange(pct * 76 / 100));
        for (i, cell) in cells.iter().enumerate() {
            // the network bar draws neither quit, speed nor save (0x42D3CD: `+0x198` jumps to 0x42D476)
            if network && nfs_game::replay::is_cell_off_in_network(i) {
                continue;
            }
            let c = BAR[i];
            // cell 2's `+0xC8` widget is glyph 1 (`paus`) — or, built with the network byte, glyph 10 (`next`,
            // 0x42CAC2); `play` (`+0xCC`) while `+0x199` is set (0x42D31E)
            let glyph = match i {
                2 if frozen => BAR_PLAY,
                2 if network => nfs_game::replay::BAR_NEXT,
                4 => bar.glyph(),
                _ => c.glyph,
            };
            let line = if over == Some(i) { lit } else { dim };
            let (w, h) = BAR_CELL;
            out.push(self.quad(aspect, cell.0, cell.1, w, 1.0, line));
            out.push(self.quad(aspect, cell.0, cell.1 + h, w, 1.0, line));
            out.push(self.quad(aspect, cell.0, cell.1, 1.0, h + 1.0, line));
            out.push(self.quad(aspect, cell.0 + w, cell.1, 1.0, h + 1.0, line));
            let Some((tex, _, _)) = self.replay_glyphs[glyph] else { continue };
            let (x, y) = cell_glyph_origin(*cell, c.box_w, c.box_h);
            let (pos, size) = Self::place(aspect, x, y, c.box_w, c.box_h);
            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: lit, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
        }
        // **The tooltips** 0x42DD10 (0x42D478, after the cells; read 2026-09-27): for each cell
        // whose hover count `+0x108 + 4i` has reached 0x28 (0x42DE23, `jl`), its string (see
        // `nfs_game::replay::bar_tooltip`) in the `HTextVisual` `+0x128` — `unviers9b` (0x420600),
        // black 0xFF000000, the layout's flags (0x42D04C…0x42D067) — over a box 0x435920 at
        // `(+0x16C + i·(21 + 30), +0x168 − 16)` (the cell's x, 19 above the cells), `ftol(w + 8)`
        // × 14 with `w` 0x431400's width, filled 0x40FFFFFF (0x419F50) — neither faded by `pct`;
        // the text at `x + 4`, `ftol((14 − h)·0.5 + y)` with `h` the measured height (13).
        for (i, cell) in cells.iter().enumerate() {
            if bar.hover[i] < 0x28 {
                continue;
            }
            let tip = &tips[i];
            let (x, y) = (cell.0, frame.1 - 16.0);
            let w = (self.text_width(tip, FONT_SMALL) + 8.0).trunc();
            out.push(self.quad(aspect, x, y, w, 14.0, [1.0, 1.0, 1.0, 0x40 as f32 / 255.0]));
            self.text(out, aspect, tip, x + 4.0, ((14.0 - EXT_SMALL) * 0.5 + y).trunc(), FONT_SMALL, [0.0, 0.0, 0.0, 1.0]);
        }
    }

    pub(crate) fn map_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        // `HTrackMap`'s tray position (`layout.track_map` — `(0, 358)` in all four shipped
        // 640×480 configs, so this never actually moves the map on screen today, but it is the
        // same place 0x41FB40 writes and is wired through for correctness)
        pos: (f32, f32),
        // every blip, the player last (the loop runs from the last car down to car 0, so the
        // player's is drawn on top): position, forward, half width / half length, colour
        blips: &[(Vec3, Vec3, [f32; 2], [[f32; 4]; 4])],
        // the footprint scale 0x4337D2…0x4337ED: 60 in the whole-track view (`+0x14A`, set by the
        // camera 0x433498 when the zoom is 0x41EA40's 600), 20 otherwise, 1 in radar mode (0x41E7C0)
        k: f32,
        // the counter and its `hud.loc` 0x13 (FE 19 "lap") label, drawn on the map (0x432EB0…
        // 0x43304C): `itoa(min(car+0xB20 + 1, laps))` + "/" (0x5CCDE0) in the display colour (`+0xD8`)
        // right-aligned against `itoa(laps)` in grey (`+0xDC`), both in `UniversC12b`. Where they sit
        // is the map's layout 0x432880: in the map's render target, which is the whole screen shown at
        // 120 × 120 (`[0x60C900]` / `[0x60C81C]` over cfg+0xE4 / +0xE8, 0x420990), the label at
        // `(cfg+0x34, H − (cfg+0x38 + h)/fy)` and the total's anchor at `W − cfg+0x34/fx` on the same
        // line (0x43296C…0x432A44) — on the map, the bottom edge 2 px under the text and the count
        // 5 px from the right; the label's x is not divided, so it lands at 5·120/W
        lap: Option<(&str, String, String)>,
        // the tray is mid-slide (0x42A410, state 1 or 2)
        sliding: bool,
    ) {
        let Some((tex, _)) = self.map else {
            return;
        };
        let (mx, my) = pos;
        let frame = self.map_frame.get();
        let m = MAP_PX as f32;
        // The draw 0x432A60 always calls the tray 0x434110 (0x432AE9), but `HTrackMap`'s tray is
        // built with `+0xA0 = 1` and `+0xC0 = 1` (0x431F0B…0x431F16), and with `+0xA0` set the tray
        // fills its box only while `+0xC0` is set **and** it is sliding (0x434182…0x4341B0). The map
        // itself (0x432B60, which also holds the lap counter) and the blips (0x433690) run only in
        // state 3 (0x432AF0…0x432B04). So a sliding map is an empty box and a map in place has no
        // box — this port drew the box under the map always, and the map while it slid
        // (audit 2026-09-26).
        if sliding {
            self.tray(out, aspect, mx, my, m, m);
            return;
        }
        // 0x432BA9…0x432C57: while TRANSLUCENCY is 0 the map's target first gets a full quad through
        // 0x4660A0, whose four vertex colours are the dword 0x3F000000 — black at alpha 0x3F
        if self.boxes.get() {
            // untextured: one pixel right and down of its vertices on the original's frames, like the
            // tray fill (`Hud::tray`) — measured at 640×480 / 800×600 / 1024×768 / 1280×720 (top edge
            // 359 / 479 / 646 / 599 for the model's 358 / 478 / 645 / 598). APPROXIMATION, empirical;
            // class C since 2026-10-02 (N07): dx7z.dll is not the cause, see `Hud::tray`.
            out.push(self.quad(aspect, mx + 1.0, my + 1.0, m, m, [0.0, 0.0, 0.0, 0x3F as f32 / 255.0]));
        }
        out.push(self.image(aspect, tex, mx, my, m, m));
        if let Some((label, current, total)) = &lap {
            let y = my + m - 2.0 - EXT_MAIN;
            self.text(out, aspect, label, 5.0 * m / HUD_REF_W, y, FONT_MAIN, HUD_GREY);
            let tw = self.text_width(total, FONT_MAIN);
            self.text(out, aspect, total, m - 5.0 - tw, y, FONT_MAIN, HUD_GREY);
            let cw = self.text_width(current, FONT_MAIN);
            self.text(out, aspect, current, m - 5.0 - tw - cw, y, FONT_MAIN, self.display());
        }
        // each blip is the car's footprint (0x433174…0x43331A → 0x423C00): a quad along its matrix,
        // `2·half width × 2·half length` times `k`, in the map's own projection
        let scale = frame[2];
        for &(p, fwd, half, colour) in blips {
            let (mx, my) = Self::map_px(frame, p.x, p.z);
            if mx < 0.0 || my < 0.0 || mx > m || my > m {
                continue;
            }
            let (w, l) = ((2.0 * half[0] * k * scale).max(1.0), (2.0 * half[1] * k * scale).max(1.0));
            // the car's forward in map pixels (right, down), measured clockwise from "up"
            let (tx, ty) = Self::map_px(frame, p.x + fwd.x, p.z + fwd.z);
            let turns = (tx - mx).atan2(-(ty - my)) / std::f32::consts::TAU;
            // the `radar.fsh` "Car" sprite tinted with the colours. The footprint is exactly `2·half`
            // (read 2026-10-02): 0x433110 builds the four corners itself — the car's position
            // `car+0x330` ± its right row `car+0x364` × `k·car+0x3A8` ± its forward row `car+0x37C` ×
            // `k·car+0x3B0` (0x433174…0x4332E0, 0x532360 scale / 0x532300 add / 0x532330 subtract) —
            // and 0x423C00 only copies those four points (x, y, z, 1.0) into the quad (0x423C04…0x423C5A);
            // nothing is mapped onto ±1. A different blip size on a frame is a different car's CRP box
            // (`car+0x3A8…0x3B0`, 0x445090), not a rule; the four are the vertex slots' (UL, UR, LR, LL),
            // one colour for a racer, `(c, other, other, c)` for a cop
            let mut q = match self.radar_car {
                Some(tex) => {
                    let mut i = self.image(aspect, tex, mx - w * 0.5, 358.0 + my - l * 0.5, w, l);
                    i.color = colour[0];
                    i
                }
                None => self.quad(aspect, mx - w * 0.5, 358.0 + my - l * 0.5, w, l, colour[0]),
            };
            if colour.iter().any(|c| *c != colour[0]) {
                q.corner_colors = Some(colour);
            }
            q.rotation = turns;
            out.push(q);
        }
    }

    /// HTray (0x434110): the fill is one quad (0x419F90) whose four vertex colours are the RGB
    /// table 0x5B32E4 (all black) under the alpha table 0x5B3304 = **63, 63, 255, 255** scaled by
    /// the fade (0…100, `vtable[+0x38]` while sliding), in the quad's own order top-left, top-right,
    /// bottom-right, bottom-left — a vertical gradient from translucent at the top to opaque black
    /// at the bottom (0x434234…0x43427B). This port filled the tray a flat 0.45 black. None of it is
    /// drawn with `TRANSLUCENCY` ≠ 0
    /// ([`Hud::boxes`], 0x41E5C0). The slide moves the whole element ([`Hud::slide`]).
    pub(crate) fn tray(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, h: f32) {
        if !self.boxes.get() {
            return;
        }
        // the fill quad of 0x419F90 (untextured, vertex colours) lands one pixel right and down of its
        // vertices on the original's frames — the tray's rule (0x419E90's line), its texts and the
        // textured quads stay on theirs. Measured at 640×480 / 800×600 / 1024×768 / 1280×720 (time,
        // position, gear, damage, map trays: every fill edge +1, the size unchanged). Not in nfs5.exe's
        // vertices (0x434310 writes `x`, `x + w`, `y`, `y + h`) and — read 2026-10-02 (N07) — **not in
        // dx7z.dll either**: its flush 0x60006F20 copies each pre-transformed vertex untouched except
        // `sz += [0x6001885C]` (state 0x18's bias, 0x60007022…0x6000707F) and hands the batch to
        // 0x6000D580, which subtracts **0.5 from every `sx` and `sy`** (`fsub` of the constant 0.5 at
        // 0x6001544C, 0x6000D58E…0x6000D59F; the other two callers 0x600082CA / 0x600087EE do the same
        // for the point and line paths) before `DrawIndexedPrimitive` (0x60006F77). That is Direct3D 7's
        // half-pixel compensation (pixel centres at integer coordinates, so a pixel-boundary quad is
        // pulled by half a pixel) and is **the same for the textured quads** (0x423A10 → 0x59F1D0), which
        // sit exactly on their integers in the Wine frames; both go through the one flush, so dx7z
        // cannot make only the untextured fills +1. The +1 is therefore the Direct3D → OpenGL layer of
        // the Wine capture (class C, platform): whether real Windows shows it is not known.
        // APPROXIMATION, empirical (matches the Wine reference frames).
        let (pos, size) = Self::place(aspect, x + 1.0, y + 1.0, w, h);
        out.push(Sprite {
            texture: self.tray_fill,
            pos,
            size,
            uv_min: [0.5, 0.25],
            uv_max: [0.5, 0.75],
            color: [1.0; 4],
            rotation: 0.0,
            pivot: [0.5, 0.5],
            tri: None, corner_colors: None,
            depth: None,
        });
        // No outline: the draw's only stroke is the rule (0x4342A4, one segment through 0x419E90's
        // two-vertex line batch) and the only frame the `+0xC1` gloss 0x435DA0, which no element's
        // constructor sets (0x433FF5). This port drew a four-edge outline in the edge colour, read
        // from a note that had taken the rule for it.
    }

    /// The display colour for a `DISPLAYCOLORS` index (0x420503…0x42051A; anything else is orange).
    pub(crate) fn display_colour(index: usize) -> [f32; 4] {
        let c: u32 = match index {
            1 => 0xCCFF_0606,
            2 => 0xCC1E_E605,
            _ => 0xCCFA_B400,
        };
        [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0]
    }

    pub(crate) fn display(&self) -> [f32; 4] {
        self.display.get()
    }

    /// The damage elements' title in a mission over its damage limit: **0xCCFFFF00** when the
    /// display colour is the red 0xCCFF0606, else **0xCCFF0000** — `HGraphicDamage` 0x422968…
    /// 0x422989 and `HTextDamage` 0x4302F5…0x430316 (`cmp eax, 0xCCFF0606`); otherwise the grey
    /// `+0x18` (0x4204B0). (`HTime`'s alert 0x41CA60 is plain 0xCCFF0000.) The port had red always.
    fn damage_title(&self, over_limit: bool) -> [f32; 4] {
        if !over_limit {
            return HUD_GREY;
        }
        let c: u32 = if self.display.get() == Self::display_colour(1) { 0xCCFF_FF00 } else { 0xCCFF_0000 };
        [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0]
    }

    /// The trays' outline and rule: the display colour masked to 24 bits with alpha 0x3F (0x4345F0).
    pub(crate) fn edge(&self) -> [f32; 4] {
        let [r, g, b, _] = self.display.get();
        [r, g, b, 0x3F as f32 / 255.0]
    }

    /// `HTray`'s rule (0x434280…0x4342A4): when the third argument of the geometry setter (vtable
    /// slot 13, 0x4340A0, stored at `+0x9C`) is positive and the tray is fully in (0x42A400 `== 3`),
    /// the draw adds a 1-px stroke (0x419E90) across the tray at `y + dy` in the outline colours.
    /// `HComboTextVisual` passes `h/2` (0x41C9F0 — HGear, HSpeed, HTime, HMultiTime, HTachDigital),
    /// HPosition `pad + rows·row` (0x42A84D, 0x42BBB1), HGraphicDamage and HTextDamage the line one
    /// pixel above their title, `(h − pad) − title height − pad/2` (0x422D2A, 0x42FE78). The port
    /// drew none of them. Only in state 3: a sliding tray has no rule (audit 2026-09-26 — the port
    /// had drawn it through the slides too).
    fn tray_rule(&self, out: &mut Vec<Sprite>, aspect: f32, x: f32, y: f32, w: f32, dy: f32) {
        if dy > 0.0 && self.boxes.get() && self.trays_in.get() {
            out.push(self.quad(aspect, x, y + dy, w, 1.0, self.edge()));
        }
    }

    /// `HComboTextVisual` (ctor 0x41C740, draw 0x41C8C0, layout 0x41C910): a tray with two
    /// `HTextVisual`s, **both `UniversC12b`** at scale 1 (0x41C7BA / 0x41C7F7, 0x41C8DD…0x41C906) —
    /// the value `+0xC8` in the display colour right-aligned at `x + w − 5` (`[0x4205A0]`), `y + 2`
    /// (`[0x4205C0]`), and the label `+0xC4` in grey at `x + 5` with its bottom `2` above the tray's
    /// (0x41C983…0x41C9DC, measured on 0x5CCAD0). This port drew the value in a larger size at its own
    /// offsets. `UniversC12b` is the HUD's `FONT_MAIN`, drawn from its own atlas.
    pub(crate) fn combo_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        (x, y, w, h): (f32, f32, f32, f32),
        value: &str,
        label: &str,
        color: [f32; 4],
    ) {
        self.tray(out, aspect, x, y, w, h);
        self.tray_rule(out, aspect, x, y, w, (h / 2.0).trunc());
        let th = FONT_MAIN;
        let vw = self.text_width(value, th);
        self.text(out, aspect, value, x + w - 5.0 - vw, y + 2.0, th, color);
        self.text(out, aspect, label, x + 5.0, y + h - 2.0 - EXT_MAIN, th, HUD_GREY);
    }

    /// `HWrongWay` (layout 0x437360, draw 0x436FCB…0x437345): FE 18159 with its top-left at
    /// `(W/2 − w/2, H/2)` — the text's top on the centre line, not centred on it — in 0xFFFAB400, and
    /// eight 5-px strokes (0x419E90) in the same colour making four corner brackets on the box
    /// `l = W/2 − w/2 − 5`, `r = W/2 + w/2 + 5`, `t = H/2 − 5`, `b = H/2 + h + 5` (`[0x5B2400]` = 5.0,
    /// each `_ftol`'d).
    fn wrong_way_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, s: &str) {
        // `UniversC12b` at its own size; the box takes the measured height (`EXT_MAIN`)
        let (cx, cy, h) = (HUD_REF_W / 2.0, HUD_REF_H / 2.0, EXT_MAIN);
        let w = self.text_width(s, FONT_MAIN);
        let x = (cx - w / 2.0).trunc();
        self.text(out, aspect, s, x, cy, FONT_MAIN, HUD_ORANGE);
        let (l, r) = ((cx - w / 2.0 - 5.0).trunc(), (cx + w / 2.0 + 5.0).trunc());
        let (t, b) = (cy - 5.0, (cy + h + 5.0).trunc());
        for (px, py, dx, dy) in [(l, t, 1.0, 1.0), (l, b, 1.0, -1.0), (r, t, -1.0, 1.0), (r, b, -1.0, -1.0)] {
            let hx = if dx > 0.0 { px } else { px - 5.0 };
            let vy = if dy > 0.0 { py } else { py - 5.0 };
            out.push(self.quad(aspect, hx, py, 5.0, 1.0, HUD_ORANGE));
            out.push(self.quad(aspect, px, vy, 1.0, 5.0, HUD_ORANGE));
        }
    }

    /// `HGraphicDamage` (ctor 0x421E00, layout 0x4229D0, draw 0x422470) — `DAMAGEDISPLAY == 1`, which
    /// `fe.txt` ships: a 127 × 105 tray at (0, 0) holding a top-down car. Each of the ten zones
    /// (`car+0x4E4…`, this port's `v.damage`) becomes a level `ftol(zone · 10 · 0.01)`; level 0
    /// tints a part 0x55058228, level k the k-th of ten colours green → yellow → red (0x422162…).
    /// The six parts (`car.fsh` 3, 0, 4, 5, 2, 1) sit at the backdrop's corner + their offsets
    /// (0x420AF0…0x420B40); parts 3, 4 and 5 are drawn again flipped vertically for the other
    /// side's zone at `y + offset − 1` (37, 43, 32 — 0x423C70's `[0x5CCC98]` = 1); then the backdrop
    /// (entry 6, tinted 0xCC000000) plain and flipped below itself. Under it a 6-px gauge in two
    /// gradients, "0%" / "100%" over its ends in `unviers9bi` and the display colour, and FE 12
    /// "damage" at the bottom in grey `UniversC12b` — red 0xCCFF0000 in a mission over its damage
    /// limit (0x42294D…). Each text is drawn in its own font's atlas (`FONT_ITALIC`, `FONT_MAIN`).
    pub(crate) fn damage_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, zones: &[f32; 10], labels: [&str; 3], over_limit: bool) {
        let (tx, ty, tw, th) = (0.0, 0.0, 127.0, 105.0);
        self.tray(out, aspect, tx, ty, tw, th);
        // the rule over the title: `(h − 2) − title height − 1` (0x422CFE…0x422D2A)
        self.tray_rule(out, aspect, tx, ty, tw, th - 2.0 - EXT_MAIN - 1.0);
        let argb = |c: u32| [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
        const LEVELS: [u32; 10] = [
            0xAA05_8228, 0xAA02_912D, 0xAA00_9B32, 0xAAFF_E100, 0xAAFF_CD05,
            0xAAFF_B405, 0xAAFF_A005, 0xAADC_6E05, 0xAADC_4605, 0xAADC_1905,
        ];
        let tint = |zone: usize| {
            let level = (zones[zone] * 10.0 * 0.01) as i32;
            if level <= 0 { argb(0x5505_8228) } else { argb(LEVELS[(level as usize - 1).min(9)]) }
        };
        let quad = |tex: TextureId, x: f32, y: f32, w: f32, h: f32, flip: bool, colour: [f32; 4]| {
            let (pos, size) = Self::place(aspect, x, y, w, h);
            let (uv_min, uv_max) = if flip { ([0.0, 1.0], [1.0, 0.0]) } else { ([0.0, 0.0], [1.0, 1.0]) };
            Sprite { texture: tex, pos, size, uv_min, uv_max, color: colour, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None }
        };
        let (bx, by) = (tx + 5.0, ty + 4.0);
        // (car.fsh entry, offset, quad size, zone, mirrored zone and its y offset)
        const PARTS: [(usize, f32, f32, f32, f32, usize, Option<(usize, f32)>); 6] = [
            (3, 2.0, 2.0, 52.0, 13.0, 2, Some((0, 37.0))),
            (0, 1.0, 5.0, 49.0, 44.0, 1, None),
            (4, 54.0, 1.0, 41.0, 9.0, 3, Some((7, 43.0))),
            (5, 95.0, 4.0, 24.0, 14.0, 4, Some((6, 32.0))),
            (2, 110.0, 17.0, 9.0, 19.0, 5, None),
            (1, 64.0, 9.0, 31.0, 35.0, 8, None),
        ];
        for (entry, gx, gy, w, h, zone, mirror) in PARTS {
            let Some(tex) = self.damage_parts[entry] else { continue };
            let (px, py) = (bx + gx, by + gy);
            out.push(quad(tex, px, py, w, h, false, tint(zone)));
            if let Some((mz, off)) = mirror {
                out.push(quad(tex, px, py + off - 1.0, w, h, true, tint(mz)));
            }
        }
        if let Some(tex) = self.damage_parts[6] {
            out.push(quad(tex, bx, by, 120.0, 27.0, false, argb(0xCC00_0000)));
            out.push(quad(tex, bx, by + 27.0 - 1.0, 120.0, 27.0, true, argb(0xCC00_0000)));
        }
        // the labels over the gauge at `y + 2 + 2·27 + 4`, the gauge 6 px lower plus a label height
        let label_y = ty + 2.0 + 54.0 + 4.0;
        let lh = FONT_ITALIC;
        self.text(out, aspect, labels[0], tx + 5.0, label_y, lh, self.display());
        let w100 = self.text_width(labels[1], lh);
        self.text(out, aspect, labels[1], tx + tw - 6.0 - w100, label_y, lh, self.display());
        let gy = label_y + 4.0 + EXT_ITALIC;
        let mid = tx + (tw / 2.0).trunc();
        out.push(quad(self.damage_gauge.0, tx + 5.0, gy, mid - tx - 5.0, 6.0, false, [1.0; 4]));
        out.push(quad(self.damage_gauge.1, mid, gy, tx + tw - 5.0 - mid, 6.0, false, [1.0; 4]));
        // correct the gauge textures' sampling to run texel centre to texel centre
        let n = out.len();
        for s in &mut out[n - 2..] {
            s.uv_min = [0.25, 0.5];
            s.uv_max = [0.75, 0.5];
        }
        let colour = self.damage_title(over_limit);
        self.text(out, aspect, labels[2], tx + 5.0, ty + th - 2.0 - EXT_MAIN, FONT_MAIN, colour);
    }

    /// `HTextDamage` (ctor 0x42F900, positions 0x430870, draw 0x430150) — `DAMAGEDISPLAY` 2 "bar graph"
    /// or 3 "percentages" (0x41E8D0): a 124 × 100 tray at (0, 0) with four rows `H/5` = 20 px apart,
    /// labels FE 13…16 (drivetrain, steering, body, handling) at `(x + 5, y + i·H/5)` in the grey of
    /// `+0x1C` and `unviers9b`, and FE 12 "damage" in grey `UniversC12b` at `(x + 5, h − 2 − its height)`
    /// under the tray's rule (0x42FE1F…0x42FE78). The four figures are `ftol` of 0x4938A0, 0x493960,
    /// 0x493C90 and 0x493BC0 (0x43017F…0x4301E9; the mean of the ten zones the draw also sums at
    /// 0x430212 is never used). **Percentages** (`[0x657DC8] == 3`): `itoa(v)` + "%" (0x5CCC90), "0%"
    /// (0x5CCC8C) for 0, right-aligned at `x + W − 5` in the display colour and `unviers9bi`
    /// (0x42FA4B…0x42FA73). **Bars** (0x430535…): a 3-px bar (0x420560) at `y + i·H/5 + 4 + label
    /// height` from `x + 5`, `v·(W − 2·5)/100` long in 0xFFD70A0F (0x420520), and the rest of the
    /// 114 px in 0xC8A58C0A (0x420540) unless `v == 100` (0x43076D). The draw's `edi == 4` branch is
    /// dead — the loop stops at 3 (0x430855). In a mission over its damage limit the title turns
    /// 0xCCFF0000 (0x4302DE…). Text heights are the draw sizes, as elsewhere in this HUD; a figure over
    /// 100 gives the rest a reversed quad (its right edge left of its left one), which the port draws
    /// over the same span — and **the original draws it too** (closed 2026-10-02): the figure is
    /// *reachable* (each is a sum of `(1.5 − 0.005·condition)·mean + state` over parts, 0x4938A0 family,
    /// `Vehicle::part_sum`, so a worn part over zones at their 100 cap reads above 100), the quad is
    /// appended by 0x419F50 → 0x419F90 in vertex order with no orientation test, and the flush
    /// 0x41A330 draws it through the HUD's `EAGL::Material` (made by 0x419D50 → 0x59F070), whose own
    /// default render-state block (0x5379F0: `{1, 2, 0, cull 0, bias 0, −1}`, `[material+0x48]`
    /// = block, slot `+0x44`) is applied by the layer 0x53F780 → 0x540660 before every draw; its
    /// cull setter 0x540590 gives `setstate(2, 0)`, which dx7z (0x60003E24) turns into "no cull":
    /// `[0x60058E1C] = 1`, and `_THRASH_drawquadmesh` (0x60007650) skips its signed-area test when
    /// that is 1 (0x600076E9). The view's own cull (`CW` in the race) reaches the device only while the
    /// block's cull is 1 (0x537890's `[0x5DED4C] == 1`), which the HUD's block never is.
    pub(crate) fn text_damage_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        values: [i32; 4],
        labels: [&str; 5],
        percentages: bool,
        over_limit: bool,
    ) {
        let (x, y, w, h) = (0.0, 0.0, 124.0, 100.0);
        let (lh, th) = (FONT_SMALL, FONT_MAIN);
        self.tray(out, aspect, x, y, w, h);
        self.tray_rule(out, aspect, x, y, w, h - 2.0 - EXT_MAIN - 1.0);
        let argb = |c: u32| [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
        let row_h = (h / 5.0).trunc();
        let span = w - 2.0 * 5.0;
        for (i, &v) in values.iter().enumerate() {
            let ry = y + row_h * i as f32;
            self.text(out, aspect, labels[i], x + 5.0, ry, lh, HUD_GREY);
            if percentages {
                let s = format!("{v}%");
                let sw = self.text_width(&s, FONT_ITALIC);
                self.text(out, aspect, &s, x + w - 5.0 - sw, ry, FONT_ITALIC, self.display());
            } else {
                let by = ry + 4.0 + EXT_SMALL;
                let end = x + 5.0 + (v * span as i32 / 100) as f32;
                out.push(self.quad(aspect, x + 5.0, by, end - x - 5.0, 3.0, argb(0xFFD7_0A0F)));
                if v != 100 {
                    let (l, r) = (end.min(x + 5.0 + span), end.max(x + 5.0 + span));
                    out.push(self.quad(aspect, l, by, r - l, 3.0, argb(0xC8A5_8C0A)));
                }
            }
        }
        let colour = self.damage_title(over_limit);
        self.text(out, aspect, labels[4], x + 5.0, y + h - 2.0 - EXT_MAIN, th, colour);
    }

    /// `HPosition` (ctor 0x42A790, layout 0x42B650, draw 0x42B000): the opponent list. A tray
    /// 138 wide at (502,1) and `(rows + 1) · 15 + 2·2` tall; each car's row sits at
    /// `(place − 1) · 15` below the top padding with an 8×8 swatch in its paint (0x419F50), the
    /// driver's name in grey fitted to 55 px (0x42AD6A…0x42AD87), and the gap right-aligned in the
    /// display colour 5 + 8 px in from the right edge; under the rows FE 17 "position" in grey and
    /// the player's place over the row count (`"%d" "/"` 0x42B5B1…0x42B609, then the count 0x42B61E).
    /// The fonts are `unviers9b` / `unviers9bi` / `UniversC12b` there, each drawn from its own atlas
    /// (`FONT_SMALL` / `FONT_ITALIC` / `FONT_MAIN`).
    ///
    /// **Positions, read 2026-09-25 from the layout 0x42B650** (pad `cfg1+0x34` = 5, `+0x38` = 2, row
    /// `cfg1+0xB8` = 15, row padding `cfg1+0x114` = 8 × 8): the name `+0x104[k]` at `(x + 5 + 5 + 8,
    /// y + 2 + 15k)` (0x42B6B5…0x42B6FA), the gap `+0x108[k]` right-aligned (its constructor's
    /// alignment 1, 0x42AA6A) at `x + 138 − 5 − 8` on the same top (0x42B70E…0x42B74A); the swatch
    /// quad `+0xC4…` 8 × 8 at `(x + 5, ftol(y + 2 + E·0.5 − 8/2))` with `E` the measured extent of
    /// `+0x10C`'s font — `EXT_MAIN`, so `y + 6` (0x42BA01…0x42BB4E) — moved down `(place − 1)·15` by
    /// the draw (0x42B1E6…0x42B21E). The footer's three texts share one top, `y + h − ftol(2 + E)`
    /// (0x42B7AC…0x42B7D6, 0x42B964…0x42B9DA); the label sits at `x + 5`, `"%d/"` at `ftol(ftol(x + w
    /// − 5 − W("4/")) − W₉("4")) − 3` and the count at that `+ W("4/")` (0x42B90A…0x42B994) — widths of
    /// the stand-in strings "4/" (0x5CCDDC, C12b) and "4" (0x5CCC38, `unviers9b`), not of the texts.
    pub(crate) fn opponent_list_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        pos: (f32, f32),
        rows: &[(usize, [f32; 4], String, String)],
        links: &[Option<LinkBadge>],
        place: usize,
        label: &str,
        derby: bool,
    ) {
        let (x, y) = pos;
        let (w, rh) = (138.0, 15.0);
        let h = (rows.len() as f32 + 1.0) * rh + 4.0;
        self.tray(out, aspect, x, y, w, h);
        // the rule over the footer row: `pad + rows·row` (0x42A83A…0x42A84D)
        self.tray_rule(out, aspect, x, y, w, 2.0 + rh * rows.len() as f32);
        for (k, (row, colour, name, value)) in rows.iter().enumerate() {
            let ry = y + 2.0 + rh * *row as f32;
            let swatch = (y + 2.0 + EXT_MAIN * 0.5 - 4.0).trunc() + rh * *row as f32;
            out.push(self.quad(aspect, x + 5.0, swatch, 8.0, 8.0, *colour));
            // the name loses a character at a time until its measured width (0x431400's third
            // output) is at most 55 (0x42AD6A…0x42AD87); an empty one becomes " "
            let mut name = name.clone();
            while !name.is_empty() && self.text_width(&name, FONT_SMALL) > 55.0 {
                name.pop();
            }
            self.text(out, aspect, &name, x + 18.0, ry, FONT_SMALL, HUD_GREY);
            let link = links.get(k).and_then(|l| l.as_ref());
            if let Some(LinkBadge { dnf: Some(dnf), .. }) = link {
                // game type 3, state 3 or 1 (0x42B279…0x42B2D0): FE 18191 in the display colour and
                // `UniversC12b` (`+0x118`, ctor 0x42AC05…0x42AC48: alignment 0), moved to the gap
                // visual's own position (0x42B29D…0x42B2AA) — the anchor `x + w − 13` the gap
                // draws right-aligned to, so this one runs **left** from it
                self.text(out, aspect, dnf, x + w - 13.0, ry, FONT_MAIN, self.display());
            } else {
                let vw = self.text_width(value, FONT_ITALIC);
                self.text(out, aspect, value, (x + w - 13.0 - vw).trunc(), ry, FONT_ITALIC, self.display());
            }
            // the link badge (0x42B4D1…0x42B536, game type 3 only): the 5 × 5 `pingmeter` dot tinted
            // 0x5B2D90[0x490140(race index)] on all four corners (0x423D70), at the gap visual's
            // position `+ (6, 4)` (0x42B50F…0x42B529: `[vtable + 8]` is its getter 0x4367A0)
            if let (Some(LinkBadge { colour, .. }), Some(dot)) = (link, self.ping_dot) {
                let mut sp = self.image(aspect, dot, x + w - 13.0 + 6.0, ry + 4.0, 5.0, 5.0);
                sp.color = *colour;
                out.push(sp);
            }
        }
        // the footer (0x42B569…0x42B639): FE 17 `+0x10C` in grey `UniversC12b`, `itoa(position)` + "/"
        // `+0x110` in the display colour and **`UniversC12b` too**, the count `+0x114` in grey
        // `unviers9b`, all three left-aligned on one top; the port had put the count 3 px lower and
        // right-aligned both values on their own widths
        let fy = y + h - (2.0 + EXT_MAIN).trunc();
        self.text(out, aspect, label, x + 5.0, fy, FONT_MAIN, HUD_GREY);
        // `DERBY [0x657400]` (the Smash Up code) stops after the label: 0x42B5A4…0x42B5AB jumps
        // past the place and the count to the return 0x42B63E
        if derby {
            return;
        }
        let slash = self.text_width("4/", FONT_MAIN);
        let px = ((x + w - 5.0 - slash).trunc() - self.text_width("4", FONT_SMALL)).trunc() - 3.0;
        self.text(out, aspect, &format!("{place}/"), px, fy, FONT_MAIN, self.display());
        self.text(out, aspect, &rows.len().to_string(), (px + slash).trunc(), fy, FONT_SMALL, HUD_GREY);
    }

    /// `HLeader` (ctor 0x4284D0, draw 0x4286F0) — the network race's leader. A tray 59 × 85 at
    /// (125, 0) (the cfg's `+0xF4`/`+0xF8` and `+0x1D0`), drawn (0x434110) only once the leader's
    /// picture is whole; the picture 54 × 64 (0x420C90) at the tray's `(x + 2, y + 2)` (0x428611,
    /// which moves the sprite 0x428585 built centred); and FE 18192 "Leader" in grey `UniversC12b`
    /// (0x4285B7…0x4285CB) at `x + 5` (0x4205A0) and `y + 85 − 2 − height` (0x42865C…0x42867C),
    /// the height being what 0x431400 measures, `EXT_MAIN`.
    pub(crate) fn leader_sprites(&self, out: &mut Vec<Sprite>, aspect: f32, pos: (f32, f32), tex: TextureId, label: &str) {
        let (x, y) = pos;
        let (w, h) = (59.0, 85.0);
        self.tray(out, aspect, x, y, w, h);
        out.push(self.image(aspect, tex, x + 2.0, y + 2.0, 54.0, 64.0));
        self.text(out, aspect, label, x + 5.0, (y + h - 2.0 - EXT_MAIN).trunc(), FONT_MAIN, HUD_GREY);
    }

    /// A dial: the 128 px face centred on `centre` (design pixels) with its needle on top.
    fn dial_sprites(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        face: TextureId,
        needle: (TextureId, f32, f32, f32),
        centre: [f32; 2],
        spec: &nfs_formats::fedata::DialSpec,
        value: i32,
    ) {
        // the ring under the face (0x42166A → `[+0xC8]` vtable slot 1), while TRANSLUCENCY is 0
        if self.boxes.get()
            && let Some(ring) = self.dial_ring
        {
            let mut r = self.image(aspect, ring, centre[0] - 64.0, centre[1] - 64.0, 128.0, 128.0);
            r.color = [0.0, 0.0, 0.0, 128.0 / 255.0];
            out.push(r);
        }
        out.push(self.image(
            aspect,
            face,
            centre[0] - 64.0,
            centre[1] - 64.0,
            128.0,
            128.0,
        ));
        let (tex, nw, nh, pivot) = needle;
        let piv_y = 1.0 - pivot / nh;
        let (pos, size) = Self::place(aspect, centre[0] - nw * 0.5, centre[1] - nh * piv_y, nw, nh);
        out.push(Sprite {
            texture: tex,
            pos,
            size,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: Self::needle_turns(spec, value),
            pivot: [0.5, piv_y],
            tri: None, corner_colors: None,
            depth: None,
        });
        if let Some(hub) = self.hub {
            out.push(self.image(aspect, hub, centre[0] - 10.0, centre[1] - 10.0, 20.0, 20.0));
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sprites(
        &self,
        aspect: f32,
        // the `Display.ScreenLayout` preset's element positions (`0x41FB40`, [`SCREEN_LAYOUTS`])
        layout: &HudLayout,
        speed_kmh: f32,
        rpm: f32,
        tick: u64,
        race: Option<&RaceProgress>,
        // `H321Go` 0x4184F0 returns before drawing anything when `FEGAME_TYPE [0x6573E8]` is 6, 7 or
        // 8 — the three **test drives** (dealer / used / owned, `modes.md` §0) — or when
        // `REPLAY [0x657424]` is set. The window itself is `[0x5B27D8] … [0x5B27E4]` = 256 … 448
        // ticks with the digit thresholds 256 / 320 / 384, which this port already had.
        countdown: bool,
        // `HTime` 0x4316F0: the cone penalty added to the time — `+0xE4` (the knocked cones) ×
        // `FACTORYDRIVERTCONEPENALTY [0x657830]` (seconds, or −1 when the mission has none) × 64
        time_penalty: i64,
        // `HTime` in `0xCCFF0000` (0x41CA60) instead of the display colour: a mission over its
        // damage limit (`0x434FA0(1)` on the live zones, 0x431710…0x431729) or the flash after a
        // cone (`+0xE0 = 16`, 0x431830)
        time_alert: bool,
        text: &HudText,
        // `HWrongWay` is up and in the visible half of its blink
        wrong_way: bool,
        // the `+4` trays' shared slide (`TraySlide`)
        slide: TraySlide,
        // `HTime`'s own: in a mission's playback it is drawn when the other trays are not (0x425A87)
        time_slide: TraySlide,
        // `car+0xB24` once the player has finished (`car+0xB50 == 2`): the race time, frozen
        finish_time: Option<u64>,
    ) -> Vec<Sprite> {
        let mut out = Vec::new();
        // dials: speedometer tray and tach at the layout's own position, 128 px faces (hud.md §2/§6)
        let (sx, sy) = layout.speedometer;
        let n0 = out.len();
        if text.speedometer == 2 {
            self.dial_sprites(
                &mut out,
                aspect,
                self.dial,
                self.needle,
                [sx + 64.0, sy + 64.0],
                &self.kmh,
                speed_kmh as i32,
            );
            Self::slide(&mut out, n0, aspect, (sx, sy, 128.0, 128.0), slide);
        }
        let (tx, ty) = layout.tach;
        let n0 = out.len();
        if text.tachometer == 2
            && let Some((td, tn)) = self.tach
        {
            self.dial_sprites(
                &mut out,
                aspect,
                td,
                tn,
                [tx + 64.0, ty + 64.0],
                &self.tach_spec,
                // `HTach` 0x42F660: `ftol(car+0xDB4 · [+0xA9D0] + 750.0) / 10` (0x42F678…0x42F6A2,
                // `[0x5B30D8]` = 750) — the dial's scale starts at idle, so the needle reads 750 at
                // rest and the redline at the redline. This port passed the bare rpm.
                (rpm * text.tach_scale + 750.0) as i32 / 10,
            );
            Self::slide(&mut out, n0, aspect, (tx, ty, 128.0, 128.0), slide);
        }
        if self.font.is_some() {
            // trays: digital speed 73×35, gear 85×35, time 73×35, position 138×15, each at the
            // layout's own position — each an `HComboTextVisual`, so each carries its `h/2` rule
            // (0x41C9F0)
            if text.speedometer == 1 {
                let (x, y) = layout.speed;
                let n0 = out.len();
                self.combo_sprites(&mut out, aspect, (x, y, 73.0, 35.0), &text.speed, &text.speed_label, self.display());
                Self::slide(&mut out, n0, aspect, (x, y, 73.0, 35.0), slide);
            }
            if text.gear_on {
                let (x, y) = layout.gear;
                let n0 = out.len();
                self.combo_sprites(&mut out, aspect, (x, y, 85.0, 35.0), &text.gear, &text.gear_label, self.display());
                Self::slide_passes(&mut out, n0, aspect, (x, y, 85.0, 35.0), slide, 2);
            }
            // `HTachDigital` (0x42F740, `HUDTACHOMETER == 1`): `ftol(car+0xDB4)` zero-padded to four
            // digits ("0000" below 1) under FE 18144, a 73 × 35 tray (`nfs5-2.hud`)
            if text.tachometer == 1 {
                let (x, y) = layout.tach_digital;
                let r = rpm as i32;
                let value = if r < 1 { "0000".to_string() } else { format!("{r:04}") };
                let n0 = out.len();
                self.combo_sprites(&mut out, aspect, (x, y, 73.0, 35.0), &value, &text.rpm_label, self.display());
                Self::slide(&mut out, n0, aspect, (x, y, 73.0, 35.0), slide);
            }
            if race.is_some() {
                let race_t = tick.saturating_sub(RACE_START_TICK);
                // race time tray 73×35, lap counter / position 138×15, each at the layout's own
                // position
                {
                    // `HTime` 0x4316F0 shows the **race** time, `clock − 448` (0x431756), plus the
                    // cone penalty, under FE 18 "time" (0x4317B0) — on every track. This port showed
                    // the lap time on circuits, labelled "lap time". Once the player has finished
                    // (`car+0xB50 == 2`, 0x43174D) it shows `car+0xB24`, which the finish sets to
                    // `tick − 448` (0x416840) — the time **freezes** and the tray stays up; nothing in
                    // the element or its caller 0x425950 hides it at the finish (audit 2026-09-27: the
                    // port had dropped the tray there). The base is clamped at 0 (0x431782…0x431794)
                    // before the penalty is added; the sum `_ftol(+0xE4 · CONEPENALTY · 64.0 + base)`
                    // (0x43176A…0x4317A1) is **not**: a mission with cones and no penalty seconds
                    // (`CONEPENALTY` −1) takes 64 ticks off per counted move and can go below 0.
                    // `Time_format` 0x4356E0 gets it with its sign flag 0 (0x431796): no sign is
                    // written (the "+" / "-" prefix 0x4356F7…0x435728 needs the flag), and the
                    // digits are of the magnitude (0x43572A…0x435738 `neg`, 0x43574B…0x435751) —
                    // −128 ticks reads "00:02.00". *(Until 2026-09-27 the port clamped the sum at 0.)*
                    let base = finish_time.unwrap_or(race_t);
                    let shown = (base as i64 + time_penalty).unsigned_abs();                    let colour = if time_alert { [1.0, 0.0, 0.0, 0xCC as f32 / 255.0] } else { self.display() };
                    let (x, y) = layout.time;
                    let n0 = out.len();
                    self.combo_sprites(&mut out, aspect, (x, y, 73.0, 35.0), &format_ticks(shown), &text.time_label, colour);
                    Self::slide(&mut out, n0, aspect, (x, y, 73.0, 35.0), time_slide);
                }
                // `HWrongWay` 0x436F70 (the blink state is the caller's, see `wrong_way_blink`)
                if wrong_way {
                    self.wrong_way_sprites(&mut out, aspect, &text.wrong_way);
                }
            }
        }
        // countdown: **H321Go 0x4184F0** (hud.md §4). The window is `256 <= clock <= 448`
        // (0x41852E `jl`, 0x41853A `jg`): the element still draws on tick 448 itself, digit "1" at
        // size 0 (audit 2026-09-27 — the port had stopped one tick early)
        if countdown && tick <= RACE_START_TICK {
            let idx = COUNTDOWN_TICKS.iter().rposition(|&t| tick >= t);
            if let Some(i) = idx {
                // the phase `t = ftol(fmod(clock, 64))` and the step `size = ftol((3t + 3)·5/64)`,
                // 0…15 (0x418590…0x4185BD)
                let t = (tick % 64) as i32;
                let size = ((3 * t + 3) * 5) / 64;
                if let Some((full, outline)) = self.rings {
                    // 0x4185D3…0x418624: `circleoutline` (+0x4A0) takes 0x80FAB400 and `circlefull`
                    // (+0x49C) 0x3FFAB400, and `circlefull` is drawn first
                    let mut ring = self.image(aspect, full, 320.0 - 64.0, 240.0 - 64.0, 128.0, 128.0);
                    ring.color = [HUD_ORANGE[0], HUD_ORANGE[1], HUD_ORANGE[2], 63.0 / 255.0];
                    out.push(ring);
                    let mut ring = self.image(aspect, outline, 320.0 - 64.0, 240.0 - 64.0, 128.0, 128.0);
                    ring.color = [HUD_ORANGE[0], HUD_ORANGE[1], HUD_ORANGE[2], 128.0 / 255.0];
                    out.push(ring);
                }
                // 0x419C50: two lines through the centre, across and down the whole screen, both
                // ends 0x80000000 (0x419E90 queues a line)
                let black = [0.0, 0.0, 0.0, 128.0 / 255.0];
                // (`W` is the design screen's, `640 + 2·margin`: both lines and the fan below reach
                // the window's edges, not the 640-wide layout's)
                let (el, et, er, eb) = Self::edges();
                out.push(self.quad(aspect, el, 240.0, er - el, 1.0, black));
                out.push(self.quad(aspect, 320.0, et, 1.0, eb - et, black));
                // 0x418670(digit, size): the digit objects' sizes are the table at 0x5CC838 — "1"
                // 41×55, "2" 44×57, "3" 45×58, outlines "1OL" 44×57, "2OL" 46×59, "3OL" 47×60 —
                // each placed centred on (320, 240). Digit 0 is "3" (+0x48C), 1 "2", 2 "1".
                const SOLID: [(f32, f32); 3] = [(45.0, 58.0), (44.0, 57.0), (41.0, 55.0)];
                const OUTLINE: [(f32, f32); 3] = [(47.0, 60.0), (46.0, 59.0), (44.0, 57.0)];
                // the constructor's second key table (`+0x204 + 0x14·size`, 0x417D8D…0x417FC4):
                // the solid digit's scale and its four-corner colour
                const SCALE: [f32; 16] = [0.0, 0.0, 0.0, 0.0, 0.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 1.0, 1.0, 1.0, 1.0];
                const ALPHA: [u8; 16] = [0x80, 0x80, 0x80, 0x80, 0x80, 0x3F, 0x3F, 0x3F, 0x3F, 0x80, 0x80, 0x80, 0xBF, 0xBF, 0x80, 0x80];
                let white = |a: u32| [1.0, 1.0, 1.0, (a & 0xFF) as f32 / 255.0];
                let centred = |(w, h): (f32, f32)| (320.0 - (w / 2.0).trunc(), 240.0 - (h / 2.0).trunc(), w, h);
                let solid = |d: usize| self.digits[d];
                let outline = |d: usize| self.congrats.outline[2 - d];
                let s = size as usize;
                // the solid digit, only while the scale is above 0: `ftol(w·scale)` wide, half of
                // that (truncated) either side of the centre (0x4186EB…0x4187F9)
                if SCALE[s] > 0.0
                    && let Some(tex) = solid(i)
                {
                    let (w, h) = SOLID[i];
                    let (w2, h2) = (((w * SCALE[s]) as i32 / 2) as f32, ((h * SCALE[s]) as i32 / 2) as f32);
                    let mut sp = self.image(aspect, tex, 320.0 - w2, 240.0 - h2, 2.0 * w2, 2.0 * h2);
                    sp.color = white(ALPHA[s] as u32);
                    out.push(sp);
                }
                // the previous digit again, 0x80FFFFFF, while `size ≤ 4` (0x418804…0x41884F). It
                // is not placed again: it keeps the quad of its last solid draw, scale 1 (sizes
                // 12…15), i.e. `cx ± trunc(w/2)` — one pixel narrower than `w` for an odd size
                // (0x41876D…0x4187DF); the port had drawn it at the full `w × h`
                if i > 0
                    && size <= 4
                    && let Some(tex) = solid(i - 1)
                {
                    let (w2, h2) = ((SOLID[i - 1].0 as i32 / 2) as f32, (SOLID[i - 1].1 as i32 / 2) as f32);
                    let mut sp = self.image(aspect, tex, 320.0 - w2, 240.0 - h2, 2.0 * w2, 2.0 * h2);
                    sp.color = white(0x80);
                    out.push(sp);
                }
                // the outline, `(size << 29) | 0xFFFFFF` (0x418866…0x4188AA)
                if let Some(tex) = outline(i) {
                    let (x, y, w, h) = centred(OUTLINE[i]);
                    let mut sp = self.image(aspect, tex, x, y, w, h);
                    sp.color = white(((size as u32) << 29) >> 24);
                    out.push(sp);
                }
                // and the previous outline fading, alpha `0x100 / size` in the top byte — 0 for
                // size 0 and 1 (0x4188B2…0x418905)
                if i > 0
                    && let Some(tex) = outline(i - 1)
                {
                    let a = if size == 0 { 0x100 } else { 0x100 / size as u32 };
                    let (x, y, w, h) = centred(OUTLINE[i - 1]);
                    let mut sp = self.image(aspect, tex, x, y, w, h);
                    sp.color = white(a);
                    out.push(sp);
                }
                // **the sweep 0x418910(digit, size)**: a fan of sixteen triangles from the screen
                // centre to the edge, clockwise from twelve o'clock in steps of W/4 along the top and
                // bottom and H/4 down the sides (the same fan for each digit, 48 records built on the
                // stack); triangles `16·digit … 16·digit + size` are drawn in 0x33000000 (0x419C13…
                // 0x419C34, 0x466100) — the dark wedge grows round the screen through the second
                const EDGE: [(f32, f32); 17] = [
                    (320.0, 0.0), (480.0, 0.0), (640.0, 0.0), (640.0, 120.0), (640.0, 240.0), (640.0, 360.0),
                    (640.0, 480.0), (480.0, 480.0), (320.0, 480.0), (160.0, 480.0), (0.0, 480.0), (0.0, 360.0),
                    (0.0, 240.0), (0.0, 120.0), (0.0, 0.0), (160.0, 0.0), (320.0, 0.0),
                ];
                // a point of the fan (in quarters of 640 × 480) on the render target: the fan runs
                // to the screen's own edges, `x/640 · W`, `y/480 · H` (design coordinates)
                let pt = |(x, y): (f32, f32)| Self::place(aspect, el + x / 640.0 * (er - el), et + y / 480.0 * (eb - et), 0.0, 0.0).0;
                for k in 0..=s.min(15) {
                    let mut sp = self.quad(aspect, 0.0, 0.0, 0.0, 0.0, [0.0, 0.0, 0.0, 0x33 as f32 / 255.0]);
                    sp.tri = Some(([pt((320.0, 240.0)), pt(EDGE[k]), pt(EDGE[k + 1])], [sp.color; 3]));
                    out.push(sp);
                }
            }
        }
        out
    }
}
