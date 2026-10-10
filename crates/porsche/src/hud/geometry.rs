//! The HUD's loading, anchors, text placement and slide helpers.

use crate::*;

impl Hud {
    /// `night` picks the `*N.fsh` faces and needles the original switches to when the night flag
    /// `word[0x628A28]` is set (`RE/notes/hud.md` §3.2).
    pub(crate) fn load(
        r: &mut Renderer,
        paths: &GamePaths,
        dials: Option<(nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec, nfs_formats::fedata::DialSpec)>,
        night: bool,
        // `Display.SpeedUnits` → `SPEEDUNITS`: the FE copies the car's km/h or mph dial group into
        // the options (0x4EBA8E…, `hud.md` §3.1) and the face comes from `speedK` or `speedM` (§3.2)
        mph: bool,
    ) -> Option<Self> {
        let dir = paths.game_data().join("DashHud");
        let load_idx = |r: &mut Renderer,
                        file: &str,
                        index: Option<usize>,
                        entry: Option<&str>|
         -> Option<(TextureId, f32, f32)> {
            let p = GamePaths::find_file(&dir, file)?;
            let d = std::fs::read(p).ok()?;
            let f = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let i = match (index, entry) {
                (Some(i), _) if i < f.entries.len() => i,
                (_, Some(e)) => f.entries.iter().position(|x| x.name.trim() == e)?,
                _ => return None,
            };
            let img = f.decode(&d, i).ok()?;
            Some((
                r.upload_texture(&TextureData::single(img.width, img.height, img.rgba)),
                img.width as f32,
                img.height as f32,
            ))
        };
        // defaults = '94 993 (style 4 / tach 6 / needle 2, pivot 18) when the car record is unknown
        // (with no car record the mph group is not known either: the km/h default stands in).
        // PORT-ONLY (2026-09-30) — not an approximation of a case of the original: the
        // original has no launch without a car record (every car it can launch is a row of
        // `nfs5.car`, the very list the front end offers), and every in-game launch path of the
        // port sets `self.dials` from the car in hand (`Some(car.dial_kmh, dial_mph, dial_tach)` at
        // the five launch sites); `None` is only a command-line launch of a sim name that is in no
        // `nfs5.car` record, which the original cannot express.
        let (kmh, tach_spec) = dials.map(|(k, m, t)| (if mph { m } else { k }, t)).unwrap_or((
            nfs_formats::fedata::DialSpec {
                style: 4,
                total_units: 300,
                needle_style: 2,
                start_deg: -40,
                deg_per_unit_x1000: 910,
                needle_pivot: 18,
            },
            nfs_formats::fedata::DialSpec {
                style: 6,
                total_units: 800,
                needle_style: 2,
                start_deg: -18,
                deg_per_unit_x1000: 320,
                needle_pivot: 18,
            },
        ));
        let needle_style = tach_spec.needle_style.max(0) as usize;
        let pivot = tach_spec.needle_pivot.max(0) as f32;
        let mph = mph && dials.is_some();
        let (speed_f, tach_f, n2_f, n_f) = match (night, mph) {
            (true, false) => ("speedKN.fsh", "tachN.fsh", "needle2N.fsh", "needleN.fsh"),
            (true, true) => ("speedMN.fsh", "tachN.fsh", "needle2N.fsh", "needleN.fsh"),
            (false, false) => ("speedK.fsh", "tach.fsh", "needle2.fsh", "needle.fsh"),
            (false, true) => ("speedM.fsh", "tach.fsh", "needle2.fsh", "needle.fsh"),
        };
        let fallback = if mph { "200m" } else { "300k" };
        let dial = load_idx(r, speed_f, Some(kmh.style.max(0) as usize), Some(fallback))?.0;
        let n = load_idx(r, n2_f, Some(needle_style), Some("need"))?;
        let hub = (needle_style == 0)
            .then(|| load_idx(r, n2_f, None, Some("cent")).map(|h| h.0))
            .flatten();
        let tach = load_idx(
            r,
            tach_f,
            Some(tach_spec.style.max(0) as usize),
            Some("964"),
        )
        .zip(load_idx(r, n_f, Some(needle_style), Some("need")))
        .map(|(t, nn)| (t.0, (nn.0, nn.1, nn.2, pivot)));
        let digits = [
            load_idx(r, "321go.fsh", None, Some("3Sol")).map(|x| x.0),
            load_idx(r, "321go.fsh", None, Some("2Sol")).map(|x| x.0),
            load_idx(r, "321go2.fsh", None, Some("1Sol")).map(|x| x.0),
        ];
        // `H321Go` 0x4184F0 draws `circlefull` then `circleoutline`, both 128×128 — and they are
        // **their own files** (`circlefull.fsh` holds one entry `circ`, `circleoutline.fsh` one
        // called `full`), not entries of `321go.fsh`. This port asked `321go.fsh` for them, got
        // nothing, and drew no rings at all until 2026-09-23.
        let dial_ring = load_idx(r, "circletach.fsh", Some(0), None).map(|t| t.0);
        let radar_car = load_idx(r, "radar.fsh", Some(0), None).map(|t| t.0);
        let ping_dot = load_idx(r, "pingmeter.fsh", Some(0), None).map(|t| t.0);
        let rings = load_idx(r, "circlefull.fsh", Some(0), None)
            .zip(load_idx(r, "circleoutline.fsh", Some(0), None))
            .map(|(a, b)| (a.0, b.0));
        let mut load_font = |name: &str| {
            GamePaths::find_file(&dir, name)
                .and_then(|p| std::fs::read(p).ok())
                .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
                .map(|f| BitmapFont::upload(r, f))
        };
        let font = load_font("UniversC12b.ffn");
        let font_small = load_font("unviers9b.ffn");
        let font_italic = load_font("unviers9bi.ffn");
        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        let damage_parts: [Option<TextureId>; 7] = std::array::from_fn(|i| load_idx(r, "car.fsh", Some(i), None).map(|t| t.0));
        let damage_gauge = (
            r.upload_texture(&TextureData::single(2, 1, vec![0x05, 0x82, 0x28, 0xB2, 0xFF, 0xFF, 0x00, 0xB2])),
            r.upload_texture(&TextureData::single(2, 1, vec![0xFF, 0xFF, 0x00, 0xB2, 0xDC, 0x19, 0x05, 0xB2])),
        );
        let tray_fill = r.upload_texture(&TextureData::single(1, 2, vec![0, 0, 0, 63, 0, 0, 0, 255]));
        let billboard_fill = r.upload_texture(&TextureData::single(1, 2, vec![0xFE, 0xDD, 0x81, 0xB2, 0x82, 0xA8, 0xB0, 0x80]));
        let strip_shade = r.upload_texture(&TextureData::single(2, 1, vec![0, 0, 0, 0x80, 0, 0, 0, 0xFF]));
        let button_ramps = [
            r.upload_texture(&TextureData::single(1, 2, vec![255, 255, 255, 127, 255, 255, 255, 0])),
            r.upload_texture(&TextureData::single(1, 2, vec![0, 0, 0, 0, 0, 0, 0, 127])),
            r.upload_texture(&TextureData::single(1, 2, vec![0, 0, 0, 0xCC, 0, 0, 0, 0x19])),
            r.upload_texture(&TextureData::single(2, 1, vec![0, 0, 0, 0xCC, 0, 0, 0, 0x19])),
            r.upload_texture(&TextureData::single(1, 2, vec![255, 255, 255, 0x19, 255, 255, 255, 0xCC])),
            r.upload_texture(&TextureData::single(2, 1, vec![255, 255, 255, 0x19, 255, 255, 255, 0xCC])),
        ];
        Some(Self {
            dial,
            needle: (n.0, n.1, n.2, pivot),
            hub,
            kmh,
            tach_spec,
            digits,
            rings,
            font,
            font_small,
            font_italic,
            tach,
            white,
            tray_fill,
            damage_parts,
            damage_gauge,
            billboard_fill,
            strip_shade,
            button_ramps,
            billboard_h: std::cell::Cell::new(BILLBOARD.3),
            billboard_clock: std::cell::Cell::new((0, 0)),
            display: std::cell::Cell::new(Hud::display_colour(0)),
            dial_ring,
            radar_car,
            ping_dot,
            boxes: std::cell::Cell::new(true),
            trays_in: std::cell::Cell::new(true),
            map: None,
            map_route: Vec::new(),
            map_frame: std::cell::Cell::new([0.0, 0.0, 1.0, 0.0, 1.0]),
            map_drawn: std::cell::RefCell::new(None),
            map_scratch: std::cell::RefCell::new(Vec::new()),
            text_runs: Default::default(),
            map_grid: std::cell::OnceCell::new(),
            map_check: std::cell::RefCell::new(None),
            map_tex: std::cell::RefCell::new(None),
            replay_glyphs: std::array::from_fn(|i| load_idx(r, "replay.fsh", Some(i), None)),
            congrats: CongratsTex {
                solid: [
                    load_idx(r, "congrats3.fsh", None, Some("1.tg")).map(|x| x.0),
                    load_idx(r, "congrats4.fsh", None, Some("2.tg")).map(|x| x.0),
                    load_idx(r, "congrats4.fsh", None, Some("3.tg")).map(|x| x.0),
                ],
                outline: [
                    ("321go2.fsh", "1OL"),
                    ("321go.fsh", "2OL"),
                    ("321go.fsh", "3OL"),
                    ("congrats2.fsh", "4OL"),
                    ("congrats2.fsh", "5OL"),
                    ("congrats2.fsh", "6OL"),
                    ("congrats2.fsh", "7OL"),
                    ("congrats3.fsh", "8OL"),
                ]
                .map(|(f, e)| load_idx(r, f, None, Some(e)).map(|x| x.0)),
                full: load_idx(r, "circlefull.fsh", Some(0), None).map(|x| x.0),
                cut: load_idx(r, "congrats1.fsh", Some(0), None).map(|x| x.0),
                ring: load_idx(r, "circleoutline.fsh", Some(0), None).map(|x| x.0),
            },
            portraits: std::array::from_fn(|i| load_idx(r, "factorypeople.fsh", Some(i), None).map(|x| x.0)),
        })
    }

    /// `HTrackMap`'s whole-track view (0x43277E…0x432849, 0x433440): the route's extent is
    /// `max(ftol x-span, ftol z-span)` with the min / max **starting at 0** (so the world origin is
    /// always inside), the centre `ftol(min + span/2)` per axis, and the view reaches `extent/2 +
    /// 200` (`[0x5B2484]`) either side of it — that is what `mapscale=4` (zoom 4.15·100 + 185 =
    /// 600 = 0x41EA40's value) selects, and the shipped default. The map itself is drawn every frame
    /// by [`Hud::map_update`].
    pub(crate) fn set_map(&mut self, r: &mut Renderer, table: &SliceTable) {
        let pts: Vec<(f32, f32)> = table
            .route
            .iter()
            .map(|&i| (table.slices[i].centre.x, table.slices[i].centre.z))
            .collect();
        if pts.len() < 2 {
            return;
        }
        let (mut x0, mut x1, mut z0, mut z1) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for &(x, z) in &pts {
            x0 = x0.min(x);
            x1 = x1.max(x);
            z0 = z0.min(z);
            z1 = z1.max(z);
        }
        let (sx, sz) = ((x1 - x0).trunc(), (z1 - z0).trunc());
        let extent = sx.max(sz);
        let (cx, cz) = ((x0 + sx * 0.5).trunc(), (z0 + sz * 0.5).trunc());
        let half = extent * 0.5 + 200.0;
        let tex = r.upload_texture(&TextureData::single(MAP_PX, MAP_PX, vec![0u8; (MAP_PX * MAP_PX * 4) as usize]));
        self.map = Some((tex, [cx, cz, half]));
        self.map_route = pts;
        // a new texture (zero-filled) and a new route: nothing drawn into it yet
        *self.map_drawn.borrow_mut() = None;
    }

    /// Redraw the map for this frame. `heading` is the player car's forward `(x, z)` when the map
    /// rotates (`MAP` 1 or 3, 0x41E770): 0x433440 turns the top-down view by `atan2(x, z)·360/2π −
    /// 90` degrees (0x5335A6…0x4335B4) about the view's centre. `None` is the static, north-up
    /// view (0x433440 with neither MAP test true builds no second rotation at all — the same
    /// camera as the rotating one, with that step skipped, `[0x65741C]` unread).
    ///
    /// **The on-screen sense, read 2026-09-27.** `0x539AD0`'s multiply is `dest = A·B` (0x5398B0:
    /// `dest[i][j] = Σ_k A[i][k]·B[k][j]`, row-vector `v' = v·M`), called `esi = esi · new` each
    /// time, so the chain is `M = T · R(−90°, x) · R(head, z)` applied to a row vector in that
    /// order. `R(θ, x)`'s rows are `(1,0,0), (0,c,s), (0,−s,c)` (`race-logic.md` §"0x539BD0(90,
    /// 1, 0, 0)"); `R(θ, z)`'s, by the same Rodrigues build cycled onto the other axis, are
    /// `(c,s,0), (−s,c,0), (0,0,1)`. Composing: `T·R(−90,x)` sends world `(x, y, z) → (x, z, −y)`,
    /// then `R(head,z)` turns that `(x, z)` pair by `head` — `camX = x·cos(head) − z·sin(head)`,
    /// `camY = x·sin(head) + z·cos(head)`. With `head` omitted (the static view) `camX = x`,
    /// `camY = z` — and the static picture is confirmed `x` right, `z` down, so this render's
    /// screen `u`, `v` are `camX`, `camY` **un-flipped** (no extra sign next to either — the
    /// D3D-style viewport Y-flip one would expect is not here, or is cancelled by another one in
    /// the render-target blit; matching the confirmed static output is what settles it, not an
    /// assumption about the viewport). So for the rotating view, with `head` in this atan2-turns
    /// convention (`sin(head) = fx`, `cos(head) = fz` for the normalised heading `(fx, fz)`):
    /// `u = x·fz − z·fx`, `v = x·fx + z·fz` — the heading vector maps to `(u, v) = (0, +1)`: **the
    /// car's own heading is put straight down**, not up (the previous reading, kept until now).
    /// *(Corrected 2026-09-27: `map_px` and the static fallback heading below were `(0, −1)` —
    /// straight up — matching neither this derivation nor, other than by an accidental double
    /// wrong turn, the static case once actually run through the same formula.)*
    pub(crate) fn map_update(&self, r: &mut Renderer, heading: Option<(f32, f32)>, route: bool, follow: Option<(f32, f32, f32)>) {
        let Some((tex, [mut cx, mut cz, mut half])) = self.map else { return };
        // a zoom other than the whole-track 600 puts the camera over the player car
        // (`car+0x330`) with that zoom as its reach (0x4334BF…0x4334F2, 0x433613)
        if let Some((x, z, reach)) = follow {
            (cx, cz, half) = (x, z, reach);
        }
        let scale = (MAP_PX as f32 * 0.5) / half;
        let (fx, fz) = match heading {
            Some((x, z)) if x.hypot(z) > 1e-4 => {
                let l = x.hypot(z);
                (x / l, z / l)
            }
            // the static view is `R(head, z)` never built at all, i.e. `head = 0`: `(sin 0, cos
            // 0) = (0, 1)`, not `(0, −1)` (2026-09-27, with the sense correction above)
            _ => (0.0, 1.0),
        };
        self.map_frame.set([cx, cz, scale, fx, fz]);
        // The picture is a pure function of the route's **pixel endpoints** (`map_px` truncated to
        // the 120 px target) and the constant grid, so a frame whose endpoints all land on the
        // same pixels as the picture the texture already holds would redraw the same pixels: the
        // CPU raster and the upload are skipped (performance work A9, 2026-10-01).
        let pts: &[(f32, f32)] = if route { &self.map_route } else { &[] };
        let mut now = std::mem::take(&mut *self.map_scratch.borrow_mut());
        now.clear();
        now.extend(pts.iter().map(|&(x, z)| {
            let a = Self::map_px([cx, cz, scale, fx, fz], x, z);
            (a.0 as i32, a.1 as i32)
        }));
        if self.map_drawn.borrow().as_ref() == Some(&now) {
            // **TEST GLUE** `NFS_MAP_VERIFY=1`: the skipped redraw must equal what the texture holds
            if nfs_game::config::cfg().nfs_map_verify.is_set() {
                assert!(self.map_check.borrow().as_deref() == Some(self.map_image(&now).as_slice()), "map redraw skipped but its picture changed");
            }
            *self.map_scratch.borrow_mut() = now;
            return;
        }
        // the picture is drawn into the kept buffer (the grid copied over it, then the route), not a
        // new allocation per redraw
        let mut td = self.map_tex.borrow_mut().take().unwrap_or_else(|| TextureData::single(MAP_PX, MAP_PX, Vec::new()));
        self.map_image_into(&now, &mut td.mips[0]);
        if nfs_game::config::cfg().nfs_map_verify.is_set() {
            *self.map_check.borrow_mut() = Some(td.mips[0].clone());
        }
        r.update_texture(tex, &td);
        *self.map_tex.borrow_mut() = Some(td);
        // what was drawn becomes the key; the old key's buffer is the next frame's scratch
        let old = self.map_drawn.replace(Some(now));
        *self.map_scratch.borrow_mut() = old.unwrap_or_default();
    }

    /// The map picture for the route's pixel endpoints `px` (empty when the route is not drawn):
    /// the grid (0x432C5F…0x432D7D), then the route line. A pure function of `px` and the grid —
    /// which is what lets [`Hud::map_update`] skip an unchanged redraw.
    fn map_image(&self, px: &[(i32, i32)]) -> Vec<u8> {
        let mut img = Vec::new();
        self.map_image_into(px, &mut img);
        img
    }

    /// [`Hud::map_image`] into `img` (its old contents are replaced; its capacity is kept).
    fn map_image_into(&self, px: &[(i32, i32)], img: &mut Vec<u8>) {
        // the grid, before the route (0x432C5F…0x432D7D, every mode and zoom): `w / 20` cells
        // (`w` = cfg1 `+0xE4` = 120, 20 = cfg1 `+0x10C` through 0x420470), so five lines each way,
        // a line every `20 · target / w` of the render target (= every 20 px of the 120 px tray)
        // through 0x466050 in the colour `[this+0xCC]` = 0x420450 = cfg1 `+0x30`, the map tint
        // 0x4CFFFFFF (`hud.md` §5): white at alpha 0x4C. Nothing of it was drawn before (HM).
        let grid = self.map_grid.get_or_init(|| {
            let mut img = vec![0u8; (MAP_PX * MAP_PX * 4) as usize];
            for i in 1..(MAP_PX / 20) {
                let k = (i * 20) as usize;
                for j in 0..MAP_PX as usize {
                    for (x, y) in [(k, j), (j, k)] {
                        let o = (y * MAP_PX as usize + x) * 4;
                        img[o..o + 4].copy_from_slice(&[255, 255, 255, 0x4C]);
                    }
                }
            }
            img
        });
        img.clear();
        img.extend_from_slice(grid);
        let mut plot = |x: i32, y: i32| {
            if x >= 0 && y >= 0 && (x as u32) < MAP_PX && (y as u32) < MAP_PX {
                let o = ((y as u32 * MAP_PX + x as u32) * 4) as usize;
                img[o..o + 4].copy_from_slice(&[255, 255, 255, 220]);
            }
        };
        // radar mode draws no route line (0x4336CC skips it)
        if px.len() < 2 {
            return;
        }
        for w in px.windows(2).chain(std::iter::once(&[px[px.len() - 1], px[0]][..])) {
            let ((ax, ay), (bx, by)) = (w[0], w[1]);
            if (ax - bx).abs() > MAP_PX as i32 / 2 || (ay - by).abs() > MAP_PX as i32 / 2 {
                continue; // point-to-point tracks: no closing line
            }
            let steps = (ax - bx).abs().max((ay - by).abs()).max(1);
            for t in 0..=steps {
                plot(ax + (bx - ax) * t / steps, ay + (by - ay) * t / steps);
            }
        }
    }

    /// World `(x, z)` → map pixels for the frame transform `[cx, cz, scale, fx, fz]`: `u = x·fz −
    /// z·fx`, `v = x·fx + z·fz` (0x433440's `R(−90°, x)` then `R(head, z)`, un-flipped into the
    /// render target — [`Hud::map_update`]'s doc). The heading `(fx, fz)` points **down** on the
    /// map (for the static `(0, 1)` this is the plain `x` right, `z` down of the north-up
    /// picture).
    pub(crate) fn map_px(f: [f32; 5], x: f32, z: f32) -> (f32, f32) {
        let [cx, cz, scale, fx, fz] = f;
        let (dx, dz) = (x - cx, z - cz);
        let u = dx * fz - dz * fx;
        let v = dx * fx + dz * fz;
        let m = MAP_PX as f32 * 0.5;
        (m + u * scale, m + v * scale)
    }

    /// Screen rotation (turns, clockwise positive) for a dial value — `hud.md` §3.3, H32 settled:
    /// the table `HDial` precomputes is filled backwards, so the needle sweeps clockwise by
    /// `deg_per_unit` per unit and every face, classic or Boxster, follows the same rule.
    pub(crate) fn needle_turns(spec: &nfs_formats::fedata::DialSpec, value: i32) -> f32 {
        spec.screen_cw_deg(value) / 360.0
    }

    /// The render target's size in pixels (`[0x60C900]` × `[0x60C81C]`, the original's `W` × `H`),
    /// set by the frame before it builds the HUD ([`Hud::set_screen`]); 640 × 480 until then.
    pub(crate) fn screen() -> (f32, f32) {
        let (w, h) = HUD_PX.with(|c| c.get());
        (w as f32, h as f32)
    }

    /// Tell the HUD the render target's size in pixels (the game's resolution, the window's
    /// physical framebuffer).
    pub(crate) fn set_screen(w: u32, h: u32) {
        HUD_PX.with(|c| c.set((w.max(1), h.max(1))));
    }

    /// Where the 640 × 480 design screen's top-left corner lies on the render target:
    /// `(W/2 − 320, H/2 − 240)` (integer halves). Every page the original **centres** on the screen
    /// (the billboard dialogs 0x41AE20 `((W − w)/2, (H − h)/2)`, the countdown, `HWrongWay`,
    /// `HCongrats`, …) is laid out from `W/2`, `H/2`; in design coordinates that is the same
    /// formula with `320`, `240`, so this shift is exact for them.
    pub(crate) fn origin() -> (f32, f32) {
        let (w, h) = HUD_PX.with(|c| c.get());
        ((w / 2) as f32 - 320.0, (h / 2) as f32 - 240.0)
    }

    /// The replay bar's cells and frame (`0x42DEF0`: from the render target's own `W` and `H`,
    /// [`nfs_game::replay::control_bar`]) in design coordinates.
    pub(crate) fn bar_geometry() -> ([(f32, f32); 7], (f32, f32)) {
        let (w, h) = Self::screen();
        let (ox, oy) = Self::origin();
        let (mut cells, frame) = nfs_game::replay::control_bar(w, h);
        for c in &mut cells {
            *c = (c.0 - ox, c.1 - oy);
        }
        (cells, (frame.0 - ox, frame.1 - oy))
    }

    /// The render target's edges in design coordinates: `(left, top, right, bottom)`.
    pub(crate) fn edges() -> (f32, f32, f32, f32) {
        let (w, h) = Self::screen();
        let (ox, oy) = Self::origin();
        (-ox, -oy, w - ox, h - oy)
    }

    /// Map a rectangle of the HUD's own **640×480** design space into sprite coordinates, in
    /// **constant pixels**: a design pixel is one pixel of the render target at any resolution,
    /// the design screen sits at [`Hud::origin`] (`hud.md` §2.2; the original keeps every element's
    /// pixel size; the port's HiDPI mapping, 2026-10-01). The elements the original anchors to an edge go
    /// through [`Hud::anchor`] / [`Hud::slide`], which return design coordinates relative to that
    /// origin. `aspect` is kept only for the callers' signatures.
    pub(crate) fn place(_aspect: f32, x: f32, y: f32, w: f32, h: f32) -> ([f32; 2], [f32; 2]) {
        let (sw, sh) = Self::screen();
        let (ox, oy) = Self::origin();
        ([(x + ox) / sw, (y + oy) / sh], [w / sh, h / sh])
    }

    /// **0x41E480**, the rule 0x41EBF0 / 0x41EFF0 apply to every element's 640×480 position when the
    /// game runs at another resolution (`hud.md` §2.2), on the render target `W × H`: above
    /// `W = 640` only (0x41E49C), an element left of the centre line that crosses it is re-centred,
    /// `W/2 − w/2` (signed halves, 0x41E4B6…0x41E4C6), one right of it moves with the right edge,
    /// `W + x − 640` (0x41E4EC / 0x41E4FD), one below the middle with the bottom, `H + y − 480`
    /// (0x41E504; only reached inside that branch), and the far edges are clamped at every
    /// resolution (0x41E50B…0x41E521).
    fn anchor_rule(sw: i32, sh: i32, x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
        let (mut ax, mut ay) = (x, y);
        if sw > 640 {
            if ax < 320 {
                if ax + w > 320 {
                    ax = sw / 2 - w / 2;
                }
            } else if ax > 320 {
                ax = sw + ax - 640;
            }
            if ay > 240 {
                ay = sh + ay - 480;
            }
        }
        if ax + w > sw {
            ax = sw - w;
        }
        if ay + h > sh {
            ay = sh - h;
        }
        (ax, ay)
    }

    /// An element's pixel position at the render target: 0x41EFF0's block (`x640 · y480` through
    /// 0x41E480, stored as `float(n / W)`, `float(n / H)`) and then 0x41FB40's `ftol(frac · W)`,
    /// `ftol(frac · H)` (0x41FB4C…0x41FB91). The divide and the product run at the x87's 53-bit
    /// precision (`CW` 0x027F, the ambient value the tick leaves), so the float round trip
    /// costs a pixel wherever the stored `float` falls just below the integer — `587 / 800` →
    /// 586. `passes == 2` is **HGear**, which 0x41EFF0 anchors twice (block 11, the copy-paste bug,
    /// `hud.md` §2.2): the second pass reads the stored fraction back as `ftol(frac · 640)`,
    /// `ftol(frac · 480)` and anchors again. **Measured on the original** (`scratchpad/HP`): the gear
    /// tray sits at y 564 / 732 / 685 at 800×600 / 1024×768 / 1280×720 — exactly this chain — and
    /// the text labels of the time / position trays follow the truncated x. **Not matched**: the
    /// time tray's left edge sits at the untruncated 587 (800 px) / 1067 (1280 px) — the tray quad
    /// evidently uses the float `frac · W`.
    /// Returned in design coordinates (pixel − [`Hud::origin`]), as [`Hud::place`] takes them.
    pub(crate) fn anchor_passes(x: f32, y: f32, w: f32, h: f32, passes: u8) -> (f32, f32) {
        let (sw, sh) = HUD_PX.with(|c| c.get());
        let (sw, sh) = (sw as i32, sh as i32);
        // N01a: a record that exists is used as stored (0x41F7A0 / 0x41FB40: `ftol(frac · W)`) —
        // 0x41EFF0's anchoring ran when the record was made, not now
        if let Some((fx, fy)) = HUD_STORED.with(|t| t.borrow().iter().find(|e| e.0 == x && e.1 == y && e.2 == w).map(|e| e.3)) {
            let (px, py) = ((fx as f64 * sw as f64) as i32, (fy as f64 * sh as f64) as i32);
            let (ox, oy) = Self::origin();
            return (px as f32 - ox, py as f32 - oy);
        }
        let (wi, hi) = (w as i32, h as i32);
        let (mut ax, mut ay) = Self::anchor_rule(sw, sh, x as i32, y as i32, wi, hi);
        let store = |n: i32, d: i32| (n as f64 / d as f64) as f32;
        let (mut fx, mut fy) = (store(ax, sw), store(ay, sh));
        for _ in 1..passes {
            let (x2, y2) = ((fx as f64 * 640.0) as i32, (fy as f64 * 480.0) as i32);
            (ax, ay) = Self::anchor_rule(sw, sh, x2, y2, wi, hi);
            (fx, fy) = (store(ax, sw), store(ay, sh));
        }
        let (px, py) = ((fx as f64 * sw as f64) as i32, (fy as f64 * sh as f64) as i32);
        let (ox, oy) = Self::origin();
        (px as f32 - ox, py as f32 - oy)
    }

    /// The stored positions of the record in use ([`HudCfg::stored_positions`] order), keyed by the
    /// element's 640×480 design rectangle in `SCREEN_LAYOUTS[layout]` (the draw code hands
    /// [`Hud::anchor_passes`] only that rectangle; the width disambiguates the elements that share a
    /// position). `None`: the size has no record, [`Hud::anchor_passes`] derives.
    pub(crate) fn set_stored(layout: usize, stored: Option<[(f32, f32); 13]>) {
        let l = &SCREEN_LAYOUTS[layout.min(3)];
        HUD_STORED.with(|t| {
            let mut t = t.borrow_mut();
            t.clear();
            let Some(s) = stored else { return };
            let map = MAP_PX as f32;
            let at = [
                (l.gear, 85.0), (l.speed, 73.0), (l.time, 73.0), (l.tach_digital, 73.0), (l.tach, 128.0), (l.speedometer, 128.0),
                (l.position, 138.0), (l.track_map, map), (l.leader, 59.0), (l.rearview, 240.0), (l.multi_time, 73.0),
                ((0.0, 0.0), 127.0), ((0.0, 0.0), 124.0),
            ];
            for (k, ((x, y), w)) in at.into_iter().enumerate() {
                // the damage trays: 127 wide is HGraphicDamage (+0x1B0), 124 wide HTextDamage (+0x1A0)
                let f = match k {
                    11 => s[12],
                    12 => s[11],
                    _ => s[k],
                };
                t.push((x, y, w, f));
            }
        });
    }

    pub(crate) fn anchor(_aspect: f32, x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
        Self::anchor_passes(x, y, w, h, 1)
    }

    /// A design-space quad in a flat colour.
    pub(crate) fn quad(&self, aspect: f32, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Sprite {
        let (pos, size) = Self::place(aspect, x, y, w, h);
        Sprite {
            texture: self.white,
            pos,
            size,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color,
            rotation: 0.0,
            pivot: [0.5, 0.5],
            tri: None, corner_colors: None,
            depth: None,
        }
    }

    /// A design-space textured quad.
    pub(crate) fn image(&self, aspect: f32, tex: TextureId, x: f32, y: f32, w: f32, h: f32) -> Sprite {
        let (pos, size) = Self::place(aspect, x, y, w, h);
        Sprite {
            texture: tex,
            pos,
            size,
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: 0.0,
            pivot: [0.5, 0.5],
            tri: None, corner_colors: None,
            depth: None,
        }
    }

    /// **The pen of every HUD text** after the original's own screen mapping. The HUD gives its
    /// texts integer pixel positions (`fild` at 0x431356 / 0x43135E); Thrash's text draw 0x536A60
    /// puts them through the HUD's 2-D camera (the ortho camera 0x467180 builds: `S = diag(2/W, −2/H)`
    /// scaling, a `(−1, +1)` translation, the ortho flip — so `X' = f32(x · f32(2/W) − 1)`,
    /// `Y' = f32(y · f32(2/H) − 1)`, 0x53A330 stored as floats) and back to pixels with the
    /// viewport 0x538DA0 stored (`A = W`, `B = H`, origin 0): `Xs = floor((X' + 1) · A · 0.5)`,
    /// `Ys = floor((Y' + 1) · B · 0.5)` — the **ortho arm** of 0x536A60 (0x536B36…0x536BA8: `[cam+0x220] == 1`,
    /// which 0x5392D0 sets at 0x5392DA, called by the HUD pass's 0x467180 at 0x4259AD / 0x4259F4; `OX` = `OY` = 0
    /// for the full-screen viewport). The float round trip lands one pixel low for some `(x, W)`: at 800 wide a pen at 591 / 666 / 720 / 5 comes
    /// out 590 / 665 / 719 / 4, at 1280×720 a pen y of 16 / 62 comes out 15 / 61. Measured on the
    /// original's frames (`scratchpad/HR/o`, four resolutions, every static label: time, position,
    /// gear, damage: 6 labels × 4 resolutions, x and y, 24 of 24 within the best-match residual); the camera's matrices are read, not run — the 2-D camera
    /// `+0x210…+0x21C` terms (1, 1, 0, 0 for a full-screen viewport) are taken from the formulas of
    /// 0x538DA0, not from a captured value (MEDIUM). Evaluated at the x87's 53-bit precision.
    ///
    /// The pen is the element's *final* pixel (anchored, slid), so [`Hud::text`] only records the
    /// string ([`PENDING_TEXT`]) and [`Hud::resolve_text`] moves its glyphs by `snap(pen) − pen` once
    /// the element's shift is known (`slide_passes`; whatever is left at the end of the HUD pass is
    /// unanchored, shift 0).
    pub(crate) fn snap_pen(px: f32, py: f32, w: f32, h: f32) -> (f32, f32) {
        let snap = |v: f32, s: f32| -> f32 {
            let (vi, frac) = (v.trunc(), v.fract());
            let sx = (2.0f64 / s as f64) as f32;
            let xp = (vi as f64 * sx as f64 - 1.0) as f32;
            ((xp as f64 + 1.0) * (s as f32) as f64 * 0.5).floor() as f32 + frac
        };
        (snap(px, w), snap(py, h))
    }

    /// Move the glyphs of every string recorded by [`Hud::text`] that lies in `sprites` by the
    /// original's pen snap ([`Hud::snap_pen`]) at the pen's pixel `design pen + shift + origin`.
    pub(crate) fn resolve_text(sprites: &mut [Sprite], shift: (f32, f32)) {
        let (w, h) = Self::screen();
        let (ox, oy) = Self::origin();
        PENDING_TEXT.with(|p| {
            p.borrow_mut().retain(|rec| {
                let Some(i) = sprites.iter().position(|s| s.texture == rec.texture && s.pos == rec.pos) else { return true };
                let (px, py) = (rec.pen[0] + shift.0 + ox, rec.pen[1] + shift.1 + oy);
                let (sx, sy) = Self::snap_pen(px, py, w, h);
                let (dx, dy) = ((sx - px) / w, (sy - py) / h);
                if dx != 0.0 || dy != 0.0 {
                    let end = (i + rec.count).min(sprites.len());
                    for s in sprites[i..end].iter_mut() {
                        s.pos = [s.pos[0] + dx, s.pos[1] + dy];
                    }
                }
                false
            })
        });
    }

    /// One line of text at a design-space position, `h` pixels tall, laid out glyph by glyph in the
    /// same space so the spacing is the original's whatever the window's shape.
    pub(crate) fn text(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        s: &str,
        x: f32,
        y: f32,
        h: f32,
        color: [f32; 4],
    ) {
        let Some((run, h)) = self.text_run(s, h) else { return };
        let f = self.run_font(&run);
        let scale = h / f.line_px();
        let glyphs = &run.glyphs;
        let n0 = out.len();
        let mut first: Option<[f32; 2]> = None;
        for g in glyphs {
            let (pos, size) = Self::place(
                aspect,
                x + g.x * scale,
                y + g.y * scale,
                g.w * scale,
                g.h * scale,
            );
            first.get_or_insert(pos);
            out.push(Sprite {
                texture: f.texture(),
                pos,
                size,
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                color,
                rotation: 0.0,
                pivot: [0.0, 0.0],
                tri: None, corner_colors: None,
                depth: None,
            });
        }
        if let Some(pos) = first {
            let count = out.len() - n0;
            PENDING_TEXT.with(|p| p.borrow_mut().push(PendingText { texture: f.texture(), pos, count, pen: [x, y] }));
        }
    }

    /// The font a size stands for: `FONT_SMALL` is `unviers9b` and `FONT_ITALIC` `unviers9bi`, each at
    /// its own line height, when loaded and when it has every glyph of `s` (the italic one holds only
    /// digits and a few signs); anything else, or a fallback, is `UniversC12b` at `h` (the italic's
    /// fallback at `FONT_SMALL`).
    fn font_for(&self, s: &str, h: f32) -> Option<(&BitmapFont, f32)> {
        fn own<'a>(f: &'a Option<BitmapFont>, s: &str) -> Option<(&'a BitmapFont, f32)> {
            f.as_ref().filter(|f| s.chars().all(|c| c == ' ' || f.has_glyph(c))).map(|f| (f, f.line_px()))
        }
        if h == FONT_SMALL
            && let Some(r) = own(&self.font_small, s)
        {
            return Some(r);
        }
        if h == FONT_ITALIC {
            if let Some(r) = own(&self.font_italic, s) {
                return Some(r);
            }
            return self.font.as_ref().map(|f| (f, FONT_SMALL));
        }
        self.font.as_ref().map(|f| (f, h))
    }

    /// Width of `s` drawn `h` pixels tall by [`Hud::text`], in design pixels, as the HUD measures
    /// it: `HTextVisual`'s measure 0x431400 → 0x536970 → 0x53ECE0, whose third output is the box of
    /// the glyph quads reaching to the pen's end (`BitmapFont::measure`). It is the only width the
    /// HUD takes — every layout, centring and fit (the forty 0x431400 call sites, 0x41AF70…0x4373AC)
    /// and the move of a right-aligned `HTextVisual` (`+0x18 == 1`, 0x4312D2…0x43131C). This port
    /// had used the pen's advance, which differs where a glyph's quad starts right of the pen or
    /// ends past it.
    pub(crate) fn text_width(&self, s: &str, h: f32) -> f32 {
        self.text_run(s, h).map_or(0.0, |(run, h)| run.width * h / self.run_font(&run).line_px())
    }

    /// The layout of `s` for a size `h` ([`Hud::font_for`]'s font), from [`Hud::text_runs`] or made
    /// and kept: the same glyphs and width `BitmapFont::place` / `measure` give, and the height the
    /// run is scaled to. The cache is dropped whole past 4096 strings (the clock texts vary).
    fn text_run(&self, s: &str, h: f32) -> Option<(std::rc::Rc<TextRun>, f32)> {
        let class = if h == FONT_SMALL { 0 } else if h == FONT_ITALIC { 1 } else { 2 };
        if let Some(run) = self.text_runs.borrow()[class].get(s) {
            #[cfg(debug_assertions)]
            {
                // debug builds check every hit against a fresh layout
                let (f, eff) = self.font_for(s, h).expect("a cached string has its font");
                debug_assert!(f.place(s).0 == run.glyphs && f.measure(s).0 == run.width && eff == run.fixed_h.unwrap_or(h), "stale text run for {s:?}");
            }
            return Some((run.clone(), run.fixed_h.unwrap_or(h)));
        }
        let (f, eff) = self.font_for(s, h)?;
        let font = [Some(&self.font), Some(&self.font_small), Some(&self.font_italic)]
            .iter()
            .position(|o| o.is_some_and(|o| o.as_ref().is_some_and(|x| std::ptr::eq(x, f))))
            .unwrap_or(0) as u8;
        let fixed_h = (class != 2).then_some(eff);
        let run = std::rc::Rc::new(TextRun { font, fixed_h, glyphs: f.place(s).0, width: f.measure(s).0 });
        let mut cache = self.text_runs.borrow_mut();
        if cache.iter().map(|m| m.len()).sum::<usize>() > 4096 {
            cache.iter_mut().for_each(|m| m.clear());
        }
        cache[class].insert(s.to_string(), run.clone());
        Some((run, fixed_h.unwrap_or(h)))
    }

    /// The font a [`TextRun`] was laid out in.
    fn run_font(&self, run: &TextRun) -> &BitmapFont {
        let f = match run.font {
            1 => &self.font_small,
            2 => &self.font_italic,
            _ => &self.font,
        };
        f.as_ref().expect("a cached run's font is loaded")
    }

    /// Move the sprites an element pushed since `n0` by its slide (hud.md, "The slide", 0x434630 /
    /// 0x4346A0), or drop them when the trays are down. The edge code of the tray `(x, y, w, h)`
    /// against the screen centre: `x ≥ W/2` → the right edge, else `x + w > W/2` → the top or
    /// bottom edge, else the left, top or bottom by `y ≥ H/2`; sliding in, `x = −w + t·(x + w)/11`
    /// (left), `W − t·(W − x)/11` (right), `y = −h + t·(y + h)/11` (top), `H − t·(H − y)/11`
    /// (bottom), each held at its target; sliding out the same paths back. The element is first
    /// moved to its anchored place ([`Hud::anchor`]), and the slide runs on the design screen's
    /// whole width.
    pub(crate) fn slide(out: &mut Vec<Sprite>, n0: usize, aspect: f32, rect: (f32, f32, f32, f32), s: TraySlide) {
        Self::slide_passes(out, n0, aspect, rect, s, 1)
    }

    /// [`Hud::slide`] for an element the original anchors `passes` times (HGear: 2).
    pub(crate) fn slide_passes(out: &mut Vec<Sprite>, n0: usize, aspect: f32, (x, y, w, h): (f32, f32, f32, f32), s: TraySlide, passes: u8) {
        let (ox, oy) = Self::origin();
        let (ax, ay) = Self::anchor_passes(x, y, w, h, passes);
        let (x0, y0) = (ax - x, ay - y);
        let (xi, yi, wi, hi, t) = ((ax + ox) as i32, (ay + oy) as i32, w as i32, h as i32, s.t);
        let (sw, sh) = HUD_PX.with(|c| c.get());
        let (sw, sh) = (sw as i32, sh as i32);
        // **TEST GLUE** (not in the original): `NFS_HUD_BOXES=1` prints each anchored element's
        // pixel box (logical pixels) — the measurement of `hud.md` §2.2's rule at a resolution
        static BOXES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *BOXES.get_or_init(|| nfs_game::config::cfg().nfs_hud_boxes.is_set()) {
            eprintln!("HUDBOX design=({x},{y},{w},{h}) screen={sw}x{sh} box=({xi},{yi},{wi},{hi})");
        }
        let (right, bottom) = (xi >= sw / 2, yi >= sh / 2);
        let across = !right && xi + wi > sw / 2;
        let (dx, dy) = match s.state {
            0 => {
                Self::resolve_text(&mut out[n0..], (0.0, 0.0));
                out.truncate(n0);
                return;
            }
            1 if across && !bottom => (0, (-hi + t * (yi + hi) / 11).min(yi) - yi),
            1 if across => (0, (sh - t * (sh - yi) / 11).max(yi) - yi),
            1 if right => ((sw - t * (sw - xi) / 11).max(xi) - xi, 0),
            1 => ((-wi + t * (xi + wi) / 11).min(xi) - xi, 0),
            2 if across && !bottom => (0, (yi - t * (yi + hi) / 11).max(-hi) - yi),
            2 if across => (0, (yi + t * (sh - yi) / 11).min(sh) - yi),
            2 if right => ((xi + t * (sw - xi) / 11).min(sw) - xi, 0),
            2 => ((xi - t * (xi + wi) / 11).max(-wi) - xi, 0),
            _ => (0, 0),
        };
        let (dx, dy) = (x0 + dx as f32, y0 + dy as f32);
        Self::resolve_text(&mut out[n0..], (dx, dy));
        if (dx, dy) == (0.0, 0.0) {
            return;
        }
        let (o, _) = Self::place(aspect, 0.0, 0.0, 0.0, 0.0);
        let (p, _) = Self::place(aspect, dx, dy, 0.0, 0.0);
        let d = [p[0] - o[0], p[1] - o[1]];
        for sp in &mut out[n0..] {
            sp.pos = [sp.pos[0] + d[0], sp.pos[1] + d[1]];
            if let Some((t, _)) = sp.tri.as_mut() {
                for v in t {
                    *v = [v[0] + d[0], v[1] + d[1]];
                }
            }
        }
    }
}
