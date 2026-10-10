//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// The shared bar painter **0x521CB0**: the span `lo … hi` of the box, as one cell per per
    /// cent, coloured by `at`. `NFS5.GouraudIndicatorBar` and `NFS5.DEADZONE` both draw through it.
    pub(super) fn gouraud_bar(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, lo: f32, hi: f32, at: &dyn Fn(f32) -> [f32; 4], shadow: Option<bool>) {
        let FeRect { x, y, w, h } = rect;
        if hi <= lo {
            return;
        }
        let cell = w / 100.0;
        for p in lo as u32..hi as u32 {
            out.push(self.quad(aspect, x + p as f32 * cell, y, cell, h.max(1.0), at(p as f32 + 0.5)));
        }
        // …and, last, a **drop shadow** over what it has just painted — 0x521FC2…0x52203C, which
        // is what `data.hasshadow` turns on. It is the painter's tenth argument; the eleventh
        // picks the offset, and both arms go through 0x520F00:
        //
        // ```text
        // 0x521FAD  total = [F+0x20] + [F+0x1C], the two segments' widths together
        // 0x521FB1  total <= [0x5B2404] -> nothing
        // 0x521FC2  arg10 clear          -> nothing
        // 0x521FD2  arg11 set            -> 0x520F00(cursor, box.t, total, box.b - box.t, fade, -4, 4)
        // 0x522013  arg11 clear          -> the same with **3, 4**
        // ```
        //
        // `NFS5.GouraudIndicatorBar` passes `data.hasshadow` (`[this+0x160]`, 0x4CA735) for the
        // tenth and `data.goingleft` (`[this+0x15C]`, the same byte that mirrors the span) for the
        // eleventh, so a bar that runs leftwards throws its shadow leftwards. `NFS5.DEADZONE`
        // passes **1** for the tenth on both bands and 1 / 0 for the eleventh (0x51B739,
        // 0x51B7BC), which mirrors the pair about the bar's centre the way §4.17 describes.
        // §4.79.
        if let Some(mirrored) = shadow {
            let (a, b) = if mirrored { (-4.0, 4.0) } else { (3.0, 4.0) };
            self.drop_shadow(out, aspect, FeRect::new(x + lo * cell, y, (hi - lo) * cell, h.max(1.0)), a, b);
        }
    }

    /// `NFS5.DEADZONE`'s hit test, **0x51BBA0** — which of the bar's four handles the pointer is
    /// on, or 0 for none.
    ///
    /// The handles sit at `x0 + half ∓ minPx` and `x0 + half ∓ maxPx`, each with a **±2**
    /// tolerance, and they are tested in the order 1, 3, 2, 4 with no early exit, so a later hit
    /// wins (0x51BC28…0x51BC90). The jump table at 0x51BE14 then sends **1 and 3** to the `.Min`
    /// arm and **2 and 4** to the `.Max` one.
    fn deadzone_region(&self, e: &nfs_formats::lay::Element, i: usize, px: f32) -> u32 {
        let _ = i;
        let (x0, _) = self.position_of(e);
        let (w, _) = self.box_of(e, 0);
        let half = (w as i32 / 2).max(1);
        let n = self.attr_num(e, "hackindex").unwrap_or(0.0) as usize;
        let dz = self.deadzone.get(self.pad_sel).copied().unwrap_or(DEADZONE_DEFAULT)[n.min(7)];
        let px = px as i32;
        let pix = |v: i32| v * half / 128;
        let c = x0 as i32 + half;
        let mut hit = 0;
        for (centre, region) in [(c - pix(dz[0]), 1), (c + pix(dz[0]), 3), (c - pix(dz[1]), 2), (c + pix(dz[1]), 4)] {
            if px >= centre - 2 && px <= centre + 2 {
                hit = region;
            }
        }
        hit
    }

    /// `NFS5.DEADZONE`'s setter, **0x51BD10** — the value is **centre-relative**, and the two
    /// handles keep five units apart:
    ///
    /// ```text
    /// 0x51BD4F  half  = (box.r - box.l) / 2
    /// 0x51BD51  v     = |x - box.l - half| * 128 / half
    /// 0x51BD84  region 1 or 3: accessorMin = clamp(v, 0, accessorMax - 5)
    /// 0x51BDC9  region 2 or 4: accessorMax = clamp(v, accessorMin + 5, 0x80)
    /// ```
    fn deadzone_set(&mut self, e: &nfs_formats::lay::Element, region: u32, px: f32) {
        if region == 0 {
            return;
        }
        let (x0, _) = self.position_of(e);
        let (w, _) = self.box_of(e, 0);
        let half = (w as i32 / 2).max(1);
        let v = ((px as i32 - x0 as i32 - half).abs() * 128 / half).clamp(0, 0x80);
        let n = (self.attr_num(e, "hackindex").unwrap_or(0.0) as usize).min(7);
        let pad = self.pad_sel;
        if self.deadzone.len() <= pad {
            self.deadzone.resize(pad + 1, DEADZONE_DEFAULT);
        }
        self.deadzone[pad][n] = deadzone_clamp(region, v, self.deadzone[pad][n]);
    }

    /// The bar the pointer pressed on, and which handle — `FEDeadZone`'s slot 5, **0x51BB60**: it
    /// hit-tests (0x51BBA0), keeps the answer in `[this+0x140]`, takes focus (0x4C7D70) and sets
    /// the value straight away (0x51BD10). Answers whether it took the press.
    pub fn deadzone_press(&mut self, px: f32, py: f32) -> bool {
        let Some((i, region)) = self.deadzone_at(px, py) else { return false };
        self.dz_grab = Some((i, region));
        let e = self.layout.elements[i].clone();
        self.deadzone_set(&e, region, px);
        true
    }

    /// Slot 8, **0x51BCD0** — two arms. While `[this+0x140]` holds a handle it drags it
    /// (0x51BCEB → 0x51BD10); with nothing held it hit-tests instead and keeps the answer in
    /// `[this+0x144]` (0x51BCFD → 0x51BD02), which is the **hover** the draw colours amber.
    pub fn deadzone_drag(&mut self, px: f32, py: f32) {
        if let Some((i, region)) = self.dz_grab {
            let e = self.layout.elements[i].clone();
            self.deadzone_set(&e, region, px);
            return;
        }
        self.dz_hover = self.deadzone_at(px, py);
    }

    /// Slot 9, **0x51BCB0** — the release clears the focus and drops the grab.
    pub fn deadzone_release(&mut self) {
        self.dz_grab = None;
    }

    /// The `NFS5.GouraudIndicatorBar` under the pointer, if it is one the pointer may move —
    /// `FeGouraudIndicatorBar`'s slot 5, **0x4CA470**, and its slot 8, **0x4CA590**, which are the
    /// same routine twice:
    ///
    /// ```text
    /// 0x4CA476  [this+0x150] == 0 -> false        ; data.changeable, the bind at 0x4CA256
    /// 0x4CA49C  0x4E2AB0([this+0x140], ev.x, ev.y) -> false when the pointer misses the part
    /// 0x4CA4B1  box  = 0x4C8670(this)
    /// 0x4CA4E2  left = 0x4E2B20([this+0x140], &box)      ; the part's own left
    /// 0x4CA4F9  n    = accessor->slot9()                 ; how many values it has
    /// 0x4CA512  w    = 0x4E2B90([this+0x140])            ; the part's width
    /// 0x4CA51F  v    = trunc(n * (ev.x - left) / w)      ; 0x5A0F98 truncates
    /// 0x4CA535  v >= n -> v = n - 1
    /// ```
    ///
    /// `GouraudIndicatorBar.ctr`'s only rect, `area`, is `pos=0,0` sized
    /// `control.size.width,control.size.height` `HIDDEN|READONLY`, so the part **is** the widget's
    /// box and `left`/`w` are its own. What comes back here is the accessor's name and the
    /// fraction `(ev.x - x) / w`; the count and the clamp belong to whoever owns the accessor.
    ///
    /// Eleven of the 42 shipped bars carry `changeable=1` — the five volume levels of
    /// `audioreal.lay`, the five of `forcefeedback.lay` and `advgraphics.lay`'s gamma.
    pub fn bar_at(&self, px: f32, py: f32) -> Option<(usize, String, f32)> {
        for (i, e) in self.layout.elements.iter().enumerate() {
            if e.class != "NFS5.GouraudIndicatorBar" || self.attr_num(e, "changeable").unwrap_or(0.0) == 0.0 {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_of(e, 0);
            if !(px >= x && px <= x + w && py >= y && py <= y + h) || w <= 0.0 {
                continue;
            }
            let name = e.get("accessor").or_else(|| e.get("othersource")).filter(|v| !v.is_empty())?;
            return Some((i, name.to_string(), (px - x) / w));
        }
        // a bar inside an open panel (`advgraphics.lay`'s gamma under `graphics.lay`'s strip,
        // `audioreal.lay`'s levels, `forcefeedback.lay`'s five): the same test in the panel's own
        // frame, its answer in the flattened index space, as `combo_row_at` does. (history: `game-history.md` §27.1)
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if !self.closed.contains(&k)
                && let Some(e) = self.layout.elements.get(k)
            {
                let (gx, gy) = self.position_of(e);
                if let Some((j, name, f)) = g.bar_at(px - gx, py - gy) {
                    return Some((base + j, name, f));
                }
            }
            base += g.span();
        }
        None
    }

    /// Which `NFS5.DEADZONE` the pointer is inside, and on which handle.
    fn deadzone_at(&self, px: f32, py: f32) -> Option<(usize, u32)> {
        if !self.any_analogue() {
            return None;
        }
        for (i, e) in self.layout.elements.iter().enumerate() {
            if e.class != "NFS5.DEADZONE" {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_of(e, 0);
            if !(px >= x && px <= x + w && py >= y && py <= y + h) {
                continue;
            }
            let r = self.deadzone_region(e, i, px);
            if r != 0 {
                return Some((i, r));
            }
        }
        None
    }

    /// Tell the screen which camera slot is on — `Camera.Current`, the accessor
    /// `NFS5.CAMERASELECT` compares against its own `data.accessornum` (§4.89).
    pub fn set_camera_current(&mut self, n: usize) {
        self.camera_current = n;
        for g in self.groups.values_mut() {
            g.set_camera_current(n);
        }
    }

    /// Tell the screen the four `Camera.1 … Camera.4` values — what `NFS5.CAMERAPICTURE`'s draw
    /// 0x4BF870 tests each picture against (§4.89).
    pub fn set_camera_list(&mut self, list: [i32; 4]) {
        self.camera_list = list;
        for g in self.groups.values_mut() {
            g.set_camera_list(list);
        }
    }

    /// Load the fourteen pieces (`hud.fsh` entries 1…14, 0x4CB685 keeps all fifteen) into every
    /// screen of this tree that holds an `NFS5.HUD`.
    pub fn load_hud_preview_art(&mut self, r: &mut Renderer, paths: &GamePaths) {
        let keys: Vec<usize> = self.groups.keys().copied().collect();
        for k in keys {
            if let Some(g) = self.groups.get_mut(&k) {
                g.load_hud_preview_art(r, paths);
            }
        }
        if self.layout.elements.iter().any(|e| e.class == "NFS5.HUD") {
            for entry in 1..=14 {
                self.load_pack_entry(r, paths, "Art", "hud.fsh", entry);
            }
        }
    }

    /// The pieces of `NFS5.HUD`'s preview (see the `hud_preview` field).
    pub fn set_hud_preview(&mut self, pieces: Vec<(usize, f32, f32)>, kx: f32, ky: f32) {
        for g in self.groups.values_mut() {
            g.set_hud_preview(pieces.clone(), kx, ky);
        }
        self.hud_preview = (pieces, kx, ky);
    }

    /// The sixteen dead-zone values for the selected pad, `[min, max]` per axis.
    pub fn deadzone_values(&self) -> [[i32; 2]; 8] {
        self.deadzone.get(self.pad_sel).copied().unwrap_or(DEADZONE_DEFAULT)
    }

    /// Put them back — what the save block carries (`savegame.md` §472).
    pub fn set_deadzone_values(&mut self, pad: usize, v: [[i32; 2]; 8]) {
        if self.deadzone.len() <= pad {
            self.deadzone.resize(pad + 1, DEADZONE_DEFAULT);
        }
        self.deadzone[pad] = v;
    }

    /// Hand the front end the analogue devices it can see, and which one is selected. Empty —
    /// which is what this port has with no pad attached — makes `NFS5.DEADZONE` draw nothing, the
    /// same answer 0x51B60D gives.
    pub fn set_pads(&mut self, pads: Vec<FePad>, selected: usize) {
        self.deadzone.resize(pads.len().max(1), DEADZONE_DEFAULT);
        self.pad_sel = selected.min(pads.len().saturating_sub(1));
        self.pads = pads.clone();
        for g in self.groups.values_mut() {
            g.set_pads(pads.clone(), selected);
        }
    }

    /// Is any analogue device enumerated — `0x51F2B0`, which walks the device list and answers true
    /// at the first record whose class byte is **4** and whose info block's `[+4] & 1` is set.
    pub(super) fn any_analogue(&self) -> bool {
        self.pads.iter().any(|p| p.analogue)
    }

    /// The **n-th** analogue device — `0x51F1D0`, which counts only those records and answers the
    /// device index of the one the accessor's pad number names.
    fn nth_analogue(&self, n: usize) -> Option<&FePad> {
        self.pads.iter().filter(|p| p.analogue).nth(n)
    }

    /// `NFS5.DEADZONE`'s eight bars — its own **slot 15**, 0x51B5F0, read end to end
    /// (`RE/notes/controls.md` §4.17).
    ///
    /// ```text
    /// 0x51B606  no analogue device at all -> draw nothing          ; 0x51F2B0
    /// 0x51B622  device = 0x51F1D0(accessorMin->pad)
    /// 0x51B639  hackindex 0…7 -> capability 0x24…0x2B
    /// 0x51B687  0x51F7A0 gates on the axis bit and answers its value; false -> draw nothing
    /// 0x51B70B  half = (r - l) / 2
    /// 0x51B714  minPixels = accessorMin * half / 128
    /// 0x51B72D  maxPixels = accessorMax * half / 128
    /// 0x51B748  minPct = minPixels * 100 / half,  maxPct = maxPixels * 100 / half
    /// 0x51B784  band 1: box (l+1, t, l+1+half, b), span 100-maxPct … 100-minPct,
    ///                   stops 0xFFFDB306, 0xFFFFD76C, 0xFFE60000
    /// 0x51B7D0  band 2: box (l+half-1, t, l+2*half-1, b), span minPct … maxPct,
    ///                   stops 0xFFE60000, 0xFFFFD76C, 0xFFFDB306   — the triple reversed
    /// ```
    ///
    /// so the bar mirrors about its centre, and the factory's 0 / 0x80 lights both halves whole.
    pub(super) fn deadzone_bars(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, x: f32, y: f32, w: f32, h: f32, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        if !self.any_analogue() {
            return;
        }
        let n = self.attr_num(e, "hackindex").unwrap_or(0.0) as usize;
        if n > 7 {
            return;
        }
        let Some(pad) = self.nth_analogue(self.pad_sel) else { return };
        if pad.axes >> n & 1 == 0 {
            return;
        }
        let half = (w as i32 / 2).max(1);
        let dz = self.deadzone.get(self.pad_sel).copied().unwrap_or(DEADZONE_DEFAULT)[n];
        let pct = |v: i32| {
            let pixels = v * half / 128;
            (pixels * 100 / half) as f32
        };
        let (lo, hi) = (pct(dz[0]), pct(dz[1]));
        let ramp = |a: [u8; 4], b: [u8; 4], c: [u8; 4]| {
            let (a, b, c) = (tint(a), tint(b), tint(c));
            move |p: f32| -> [f32; 4] {
                let (s, t, u) = if p < 50.0 { (a, b, p / 50.0) } else { (b, c, (p - 50.0) / 50.0) };
                std::array::from_fn(|j| s[j] + (t[j] - s[j]) * u)
            }
        };
        const RED: [u8; 4] = [0xE6, 0x00, 0x00, 0xFF];
        const PALE: [u8; 4] = [0xFF, 0xD7, 0x6C, 0xFF];
        const ORANGE: [u8; 4] = [0xFD, 0xB3, 0x06, 0xFF];
        let hw = half as f32;
        self.gouraud_bar(out, aspect, FeRect::new(x + 1.0, y, hw, h), 100.0 - hi, 100.0 - lo, &ramp(ORANGE, PALE, RED), Some(true));
        self.gouraud_bar(out, aspect, FeRect::new(x + hw - 1.0, y, hw, h), lo, hi, &ramp(RED, PALE, ORANGE), Some(false));

        // The five bars over the gradient — 0x51B7FA…0x51BAE1. Each is a `0x4D9280` fill with an
        // outline primitive over it, and all five share the box's own height.
        //
        // ```text
        // 0x51B802  marker : x = box.l + (box.w - 10) * axis / 65536   (1/65536 at 0x5B25C0)
        // 0x51B852           0x4D9280(x, y, 10.0, h, 0x4D87C0(fade))   — always amber
        // 0x51B875           0x521810(x, y, 10.0, h, fade)             — its outline
        // 0x51B8B0  handle : x = box.l + half -+ pixels - 2,  5.0 wide
        // 0x51B889           0x4D87C0(fade) amber when [this+0x144] names it, else 0x4D87B0 red
        // 0x51B903           0x521050(x, y, 5.0, h, fade, 3) when [this+0x140] holds it,
        // 0x51B916           0x521250(...) otherwise
        // ```
        //
        // and the four handles are visited **1, 3, 2, 4** (0x51B883, 0x51B924, 0x51B9C3,
        // 0x51BA64) — the min pair either side of the centre first, then the max pair — which is
        // the order the hit test walks them in too.
        //
        // The marker's divisor pins the device's range: the image sets **no** `DIPROP_RANGE`
        // (0x56FD70's only `SetProperty` is `DIPROP_BUFFERSIZE`, GUID 1), so an absolute axis
        // keeps DirectInput's own 0…65535, and `(w - 10) * v / 65536` puts a ten-pixel marker
        // exactly inside the box at either end.
        let mx = x + deadzone_marker(w as i32, pad.values[n]);
        out.push(self.quad(aspect, mx, y, 10.0, h, tint([0xFF, 0xBB, 0x00, 0xFF])));
        // and 0x521810's inset highlight over it (0x51B875)
        self.glass(out, aspect, FeRect::new(mx, y, 10.0, h), &tint);
        for (region, centre) in deadzone_handles(w as i32, dz) {
            let hot = self.dz_hover.is_some_and(|(_, r)| r == region);
            let col = if hot { [0xFF, 0xBB, 0x00, 0xFF] } else { [0xBC, 0x00, 0x00, 0xFF] };
            let hx = x + centre - 2.0;
            out.push(self.quad(aspect, hx, y, 5.0, h, tint(col)));
            // the held handle takes 0x521050's sunken bevel and every other one 0x521250's
            // raised outline — §4.77 reads the second out band by band
            if self.dz_grab.is_some_and(|(_, r)| r == region) {
                self.bevel(out, aspect, FeRect::new(hx, y, 5.0, h), 3.0);
            } else {
                self.bevel_glow(out, aspect, FeRect::new(hx, y, 5.0, h), 3.0);
            }
        }
    }
}
