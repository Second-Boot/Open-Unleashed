//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

/// A widget box in front-end pixels — the `(x, y, w, h)` the painters below take, as the
/// original passes them (four floats on the stack).
#[derive(Debug, Clone, Copy)]
pub(super) struct FeRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl FeRect {
    pub(super) const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

impl LayoutScreen {
    /// The front end's drop shadow, **0x520F00** — the widget's box offset by `(a, b)` and painted
    /// black, as the L of what the offset uncovers. `NFS5.GOTOBUTTON`'s draw calls it with
    /// **(4, 4)** (0x4C91FE, 0x4C9202) for every button whose `dropshadow` is **1**, which is the
    /// template's default and what 132 of the 153 shipped ones carry.
    ///
    /// Three colours, each through the fade (0x4D87F0): **0xCC000000** at the box's own edge,
    /// **0x19000000** at the far edge of the offset, and **0x0C000000** in the two corner wedges
    /// (0x520F19…0x520F40; the fourth mix, a fully transparent 0, is dropped unused). The four
    /// quads, in the order the function draws them:
    ///
    /// | | upper-left | upper-right | lower-left | lower-right |
    /// |---|---|---|---|---|
    /// | right strip (0x520FAF) | `(x+w, y+b)` CC | `(x+w+a, y+b)` 19 | `(x+w, y+h)` CC | `(x+w+a, y+h+b)` 19 |
    /// | bottom strip (0x520FEB) | `(x+2a, y+h)` CC | `(x+w, y+h)` CC | `(x+a, y+h+b)` 19 | `(x+w+a, y+h+b)` 19 |
    /// | lower-left wedge (0x521008) | `(x+a, y+h)` 0C | `(x+2a, y+h)` CC | `(x+a, y+h+b)` 0C | `(x+a, y+h+b)` 0C |
    /// | upper-right wedge (0x521036) | `(x+w, y+b)` 0C | `(x+w+a, y+b)` 0C | `(x+w, y+2b)` CC | `(x+w, y+2b)` CC |
    ///
    /// Behind the same global gate 0x4E35E0 as the bevel (0x520F03).
    pub(super) fn drop_shadow(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, a: f32, b: f32) {
        let FeRect { x, y, w, h } = rect;
        let (near, far, edge) = (tint([0, 0, 0, 0xcc]), tint([0, 0, 0, 0x19]), tint([0, 0, 0, 0x0c]));
        let (r, bot) = (x + w, y + h);
        self.gouraud(out, aspect, [[r, y + b], [r + a, y + b], [r, bot], [r + a, bot + b]], [near, far, near, far]);
        self.gouraud(out, aspect, [[x + 2.0 * a, bot], [r, bot], [x + a, bot + b], [r + a, bot + b]], [near, near, far, far]);
        self.gouraud(out, aspect, [[x + a, bot], [x + 2.0 * a, bot], [x + a, bot + b], [x + a, bot + b]], [edge, near, edge, edge]);
        self.gouraud(out, aspect, [[r, y + b], [r + a, y + b], [r, y + 2.0 * b], [r, y + 2.0 * b]], [edge, edge, near, near]);
    }

    /// The chosen driver picture's frame, **0x521B60** — one caller, `NFS5.PLAYERDRIVERPICTURE`'s
    /// draw at 0x4CF87C, with `(x, y, w, h, fade, 5)`. Two colours go through the fade (0x4D87F0):
    /// **0xFFFFFFFF** (0x521B6E) and **0x80FFFFFF** (0x521B7B). Four gouraud quads 0x4D98D0 ring
    /// the box `t` pixels wide, opaque white where they touch it and half white at the outside:
    ///
    /// | band | upper-left | upper-right | lower-left | lower-right |
    /// |---|---|---|---|---|
    /// | top (0x521BE2) | `(x−t, y−t)` 80 | `(x+w+t, y−t)` 80 | `(x, y)` FF | `(x+w, y)` FF |
    /// | bottom (0x521C26) | `(x, y+h)` FF | `(x+w, y+h)` FF | `(x−t, y+h+t)` 80 | `(x+w+t, y+h+t)` 80 |
    /// | left (0x521C63) | `(x−t, y−t)` 80 | `(x, y)` FF | `(x−t, y+h+t)` 80 | `(x, y+h)` FF |
    /// | right (0x521C9C) | `(x+w, y)` FF | `(x+w+t, y−t)` 80 | `(x+w, y+h)` FF | `(x+w+t, y+h+t)` 80 |
    ///
    /// Unlike 0x521050 and 0x520F00 it asks no global gate. CONFIRMED (symbolic walk of the stack).
    pub(super) fn picture_frame(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, t: f32) {
        let FeRect { x, y, w, h } = rect;
        let (inner, outer) = (tint([0xff, 0xff, 0xff, 0xff]), tint([0xff, 0xff, 0xff, 0x80]));
        let (r, b) = (x + w, y + h);
        self.gouraud(out, aspect, [[x - t, y - t], [r + t, y - t], [x, y], [r, y]], [outer, outer, inner, inner]);
        self.gouraud(out, aspect, [[x, b], [r, b], [x - t, b + t], [r + t, b + t]], [inner, inner, outer, outer]);
        self.gouraud(out, aspect, [[x - t, y - t], [x, y], [x - t, b + t], [x, b]], [outer, inner, outer, inner]);
        self.gouraud(out, aspect, [[r, y], [r + t, y - t], [r, b], [r + t, b + t]], [inner, outer, inner, outer]);
    }

    /// The front end's beveled frame, **0x521050** — a shared painter with twenty call sites, of
    /// which `NFS5.BEVELEDOPTIONBOX`'s (0x520EEC, thickness **3**) is the one this port draws so
    /// far. It takes `(x, y, w, h, fade, thickness)` as four floats and two ints, and paints four
    /// trapezoidal bands with the gouraud quad primitive 0x4D98D0 (§4.7's four `(x, y, colour)`
    /// triples, in the order upper-left, upper-right, lower-left, lower-right):
    ///
    /// | band | outer edge, opaque | inner edge, nearly clear |
    /// |---|---|---|
    /// | top | `(x+1, y+1)`…`(x+w−1, y+1)` **0xCC000000** | `(x+t+1, y+t+1)`…`(x+w−t−1, y+t+1)` **0x19000000** |
    /// | left | `(x+1, y+1)`…`(x+1, y+h−1)` 0xCC000000 | `(x+t, y+t+1)`…`(x+t, y+h−t−1)` 0x19000000 |
    /// | right | `(x+w−1, y+1)`…`(x+w−1, y+h−1)` **0xCCFFFFFF** | `(x+w−t−1, y+t+1)`…`(x+w−t−1, y+h−t−1)` **0x33FFFFFF** |
    /// | bottom | `(x−2, y+h−1)`…`(x+w−2, y+h−1)` 0xCCFFFFFF | `(x+t+1, y+h−t−1)`…`(x+w−t+1, y+h−t−1)` 0x33FFFFFF |
    ///
    /// Dark above and to the left, light below and to the right: a **sunken** bevel. The two the
    /// bottom band is shifted by — the −2 on its outer edge (the 2.0f at 0x5B2580, 0x521206 and
    /// 0x52121D) and the +1 where the right band has −1 — are the original's own asymmetry, kept.
    ///
    /// Three of the seven colours 0x521050 mixes are computed and dropped without being used
    /// (0xCC808080, 0x19FFFFFF and 0xB2FFFFFF — the results of the second, third and fifth
    /// 0x4D87F0 calls are overwritten before anything reads them), so only these four reach the
    /// screen. The whole thing is behind the global gate 0x4E35E0 that §4.9 records, the one the
    /// glinty classes share; this port draws those, so it draws this.
    ///
    /// Each band is one gouraud trapezoid, drawn as two per-corner triangles ([`Self::gouraud`]).
    /// **0x521810** — the inset highlight the text boxes and the dead-zone marker carry. Two
    /// gouraud halves through `0x4D96B0(x, y, w, h, c00, c10, c01, c11)`, inset one pixel and two
    /// smaller than the box:
    ///
    /// ```text
    /// 0x5218CE  top    (x+1, y+1,             w-2, (h-2)/2)  0x80FFFFFF over 0x00FFFFFF
    /// 0x5218F3  bottom (x+1, y+1 + (h-2)/2,   w-2, (h-2)/2)  0x00000000 over 0x91000000
    /// ```
    ///
    /// with 2.0 at [0x5B2580], 0.5 at [0x5B23F0] and 1.0 at [0x5B24A8], so the top half fades a
    /// half-white down to clear and the bottom half clear down to a near-opaque black — a lit
    /// upper edge and a shaded lower one.
    ///
    /// The gate 0x4E35E0 (`[0x6197AA] == 0`) has a second arm: a plain one-pixel black outline
    /// through 0x4D9200 at the widget's own fade (0x4D87E0, which is just `fade << 24`). This
    /// port takes the other arm, the same choice §4.21 makes for the bevel.
    ///
    /// Each half is one gouraud rectangle ([`Self::gouraud`]); the port stepped them in one-pixel
    /// strips until 2026-09-24.
    pub(super) fn glass(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, tint: &dyn Fn([u8; 4]) -> [f32; 4]) {
        let FeRect { x, y, w, h } = rect;
        let half = (h - 2.0) * 0.5;
        if half < 1.0 || w < 3.0 {
            return;
        }
        // each half one 0x4D96B0 rectangle, its top pair one colour and its bottom pair the other
        for (top, c0, c1) in [
            (y + 1.0, tint([0xFF, 0xFF, 0xFF, 0x80]), tint([0xFF, 0xFF, 0xFF, 0])),
            (y + 1.0 + half, tint([0, 0, 0, 0]), tint([0, 0, 0, 0x91])),
        ]
        {
            let (l, r, b) = (x + 1.0, x + w - 1.0, top + half);
            self.gouraud(out, aspect, [[l, top], [r, top], [l, b], [r, b]], [c0, c0, c1, c1]);
        }
    }

    /// **0x521970**, the *raised* bevel — §4.21's sunken one turned over. Six colours go through
    /// the fade at 0x521989…0x5219CB and the first two, 0xFFE00C0C and 0xFF820000 (which are
    /// 0x521900's own pair), are **dead**: each is left in `eax` and clobbered by the next call
    /// before anything reads it, exactly as three of 0x521050's are. The four that live are
    /// 0x521050's palette with **0x33000000** where it has 0x19000000, and they go into the same
    /// four twelve-argument 0x4D98D0 quads (§4.74):
    ///
    /// ```text
    /// band            outer edge                              inner edge
    /// left  0x521A6C  (x+1, y+1)…(x+1, b)        0xCCFFFFFF   (x+t+1, y+t+1)…(x+t+1, b-t)     0x33FFFFFF
    /// top   0x521AC2  (x+1, y+1)…(x+w, y+1)      0xCCFFFFFF   (x+t+1, y+t+1)…(x+w-t, y+t+1)   0xCCFFFFFF
    ///                            ramping to      0x33FFFFFF               ramping to          0x33FFFFFF
    /// right 0x521B00  (x+w-1, y+1)…(x+w-1, b)    0xCC000000   (x+w-1-t, y+t+1)…(x+w-1-t, b-t) 0x33000000
    /// bottom 0x521B4B (x+1, b)…(x+w, b)          0xCC000000   (x+t+1, b-t)…(x+w-t, b-t)       0x33000000
    /// ```
    ///
    /// with `b = y + h - 1`. Light on the **left and top**, dark on the right and bottom — the
    /// mirror of 0x521050 — and the top band alone also fades **along its length**, bright at the
    /// left corner and dim at the right, which is the one thing neither bevel's other bands do.
    ///
    /// Each band is one gouraud quad ([`Self::bevel_bands`]).
    pub(super) fn bevel_up(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, t: f32) {
        let FeRect { x, y, w, h } = rect;
        let (lit_out, lit_in) = (tint([0xff, 0xff, 0xff, 0xcc]), tint([0xff, 0xff, 0xff, 0x33]));
        let (dark_out, dark_in) = (tint([0, 0, 0, 0xcc]), tint([0, 0, 0, 0x33]));
        let (r, b) = (x + w, y + h - 1.0);
        // each band: the outer segment and its two end colours, then the inner segment and its two
        let bands = [
            ([x + 1.0, y + 1.0], [x + 1.0, b], [lit_out; 2], [x + t + 1.0, y + t + 1.0], [x + t + 1.0, b - t], [lit_in; 2]),
            ([x + 1.0, y + 1.0], [r, y + 1.0], [lit_out, lit_in], [x + t + 1.0, y + t + 1.0], [r - t, y + t + 1.0], [lit_out, lit_in]),
            ([r - 1.0, y + 1.0], [r - 1.0, b], [dark_out; 2], [r - 1.0 - t, y + t + 1.0], [r - 1.0 - t, b - t], [dark_in; 2]),
            ([x + 1.0, b], [r, b], [dark_out; 2], [x + t + 1.0, b - t], [r - t, b - t], [dark_in; 2]),
        ];
        self.bevel_bands(out, aspect, &bands, t);
    }

    /// **0x521400**, the list headings' bevel — `drawPartsHeading` 0x518DE0 hands it
    /// `(x, y, w, 13.0, fade, 2)` after the name (0x518E8A), and the car lists' row painters
    /// 0x4F3510 / 0x4F4AD0 / 0x4F5D10 `(x, y + 1, w, 12.0, fade, 2)` after each era or class
    /// heading (0x4F360C, 0x4F5E0C, …). Behind the glint gate 0x4E35E0 like §4.21's. Eleven
    /// colours go through the fade 0x4D87F0 (0x521418…0x5214A0) and five die in `eax`
    /// (0x80FFFFFF, 0x80000000, 0x00FFFFFF, 0, 0xB2FFFFFF); the six that live make four 0x4D98D0
    /// quads, `R = x + w`, `B = y + h`:
    ///
    /// ```text
    /// band            upper-left            upper-right          lower-left           lower-right
    /// top    0x5214FF (x, y)      19FFFFFF  (R, y)     19FFFFFF  (x+t, y+t) E5FFFFFF  (R-t, y+t) CCFFFFFF
    /// left   0x521546 (x, y)      B2000000  (x+t, y+t) 19000000  (x, B)     CC000000  (x+t, B-t) 19000000
    /// right  0x52156F (R-t, y+t)  19FFFFFF  (R, y)     E5FFFFFF  (R-t, B-t) 19FFFFFF  (R, B)     CCFFFFFF
    /// bottom 0x5215A5 (x+t, B-t)  B2000000  (R-t, B-t) CC000000  (x, B)     19000000  (R, B)     19000000
    /// ```
    ///
    /// — a white glow under the top edge and inside the right one, a dark one inside the left edge
    /// and above the bottom. CONFIRMED (symbolic walk of the stack, 2026-09-27).
    pub(super) fn bevel_heading(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, t: f32) {
        let FeRect { x, y, w, h } = rect;
        let c = |v: u32| tint([(v >> 16) as u8, (v >> 8) as u8, v as u8, (v >> 24) as u8]);
        let (r, b) = (x + w, y + h);
        let (w19, we5, wcc) = (c(0x19FF_FFFF), c(0xE5FF_FFFF), c(0xCCFF_FFFF));
        let (b19, bb2, bcc) = (c(0x1900_0000), c(0xB200_0000), c(0xCC00_0000));
        self.gouraud(out, aspect, [[x, y], [r, y], [x + t, y + t], [r - t, y + t]], [w19, w19, we5, wcc]);
        self.gouraud(out, aspect, [[x, y], [x + t, y + t], [x, b], [x + t, b - t]], [bb2, b19, bcc, b19]);
        self.gouraud(out, aspect, [[r - t, y + t], [r, y], [r - t, b - t], [r, b]], [w19, we5, w19, wcc]);
        self.gouraud(out, aspect, [[x + t, b - t], [r - t, b - t], [x, b], [r, b]], [bb2, bcc, b19, b19]);
    }

    /// What the four-band bevels share: each band is an **outer** segment with a colour at either
    /// end and an **inner** one the same — one call of the gouraud quad primitive 0x4D98D0 with the
    /// outer pair as its upper corners and the inner pair as its lower ones (§4.74), drawn through
    /// [`Self::gouraud`]. This port stepped each band into `t` strips across and twelve along until
    /// 2026-09-24; `t` is kept in the signature for the callers.
    #[allow(clippy::type_complexity)]
    fn bevel_bands(
        &self,
        out: &mut Vec<Sprite>,
        aspect: f32,
        bands: &[([f32; 2], [f32; 2], [[f32; 4]; 2], [f32; 2], [f32; 2], [[f32; 4]; 2])],
        _t: f32,
    ) {
        for &(a0, a1, ca, c0, c1, cb) in bands {
            self.gouraud(out, aspect, [a0, a1, c0, c1], [ca[0], ca[1], cb[0], cb[1]]);
        }
    }

    /// **0x5215C0**, the outline over the combo scrollbar's knob (0x4DB622, thickness **7**) —
    /// the last painter in the front end, and 0x521250's shape with a richer palette. Seventeen
    /// colours go through the fade and **seven die**: the 0x80000000 at 0x5215FC, the 0xB2FFFFFF,
    /// 0xCCFFFFFF and 0xCC000000 at 0x521620…0x521636, each left in `eax` and clobbered by the
    /// next call, and the two the body uses belong to §4.90's 0x4D96B0. The four that make the
    /// quads are:
    ///
    /// ```text
    /// band             outer edge                          inner edge
    /// top    0x521748  (x, y)…(x+w, y)                     (x+t, y+t)…(x+w-t, y+t)
    ///                  0xE5FFFFFF -> 0x80808080            0x00FFFFFF
    /// left   0x5217C8  (x, y)…(x, y+h)                     (x+t, y+t)…(x+t, y+h-t)
    ///                  0xE5FFFFFF -> 0x80FFFFFF            0x00FFFFFF
    /// right  0x5217FB  (x+w, y)…(x+w, y+h)                 (x+w-t, y+t)…(x+w-t, y+h-t)
    ///                  0x66000000 -> 0x50000000            0x00000000
    /// bottom 0x5217A0  (x, y+h)…(x+w, y+h)                 (x+t, y+h-t)…(x+w-t, y+h-t)
    ///                  0x800C0C0C -> 0xE5000000            0x00808080 -> 0x0C000000
    /// ```
    ///
    /// Light on the top and left, dark on the right and bottom — raised, like §4.77's two — on
    /// the box's own edge with no `-1` anywhere, and **every** band fading along its length.
    /// §4.98.
    pub(super) fn bevel_knob(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, t: f32) {
        let FeRect { x, y, w, h } = rect;
        let c = |v: [u8; 4]| tint(v);
        let (r, b) = (x + w, y + h);
        let clear_w = c([0xff, 0xff, 0xff, 0]);
        let bands = [
            ([x, y], [r, y], [c([0xff, 0xff, 0xff, 0xe5]), c([0x80, 0x80, 0x80, 0x80])], [x + t, y + t], [r - t, y + t], [clear_w; 2]),
            ([x, y], [x, b], [c([0xff, 0xff, 0xff, 0xe5]), c([0xff, 0xff, 0xff, 0x80])], [x + t, y + t], [x + t, b - t], [clear_w; 2]),
            ([r, y], [r, b], [c([0, 0, 0, 0x66]), c([0, 0, 0, 0x50])], [r - t, y + t], [r - t, b - t], [c([0, 0, 0, 0]); 2]),
            ([x, b], [r, b], [c([0x0c, 0x0c, 0x0c, 0x80]), c([0, 0, 0, 0xe5])], [x + t, b - t], [r - t, b - t], [c([0x80, 0x80, 0x80, 0]), c([0, 0, 0, 0x0c])]),
        ];
        self.bevel_bands(out, aspect, &bands, t);
    }

    /// **0x521250**, the outline every `NFS5.DEADZONE` handle that is *not* being dragged carries
    /// (0x51B916, 0x51B9B5, 0x51BA4E, and the fourth's `jne` arm) — §4.17's last gap.
    ///
    /// All **eight** colours its 0x4D87F0 calls mix reach the screen here, unlike 0x521050's and
    /// 0x521970's, and each of the four twelve-argument 0x4D98D0 quads (§4.74) fades along its
    /// own length:
    ///
    /// ```text
    /// band            outer edge                          inner edge
    /// top    0x52133B  (x, y)…(x+w, y)                     (x+t, y+t)…(x+w-t, y+t)
    ///                  0xE5FFFFFF -> 0x80FFFFFF            0x00FFFFFF -> 0x00000000
    /// left   0x521382  (x, y)…(x, y+h)                     (x+t, y+t)…(x+t, y+h-t)
    ///                  0xCCFFFFFF -> 0xB2FFFFFF            0x00FFFFFF
    /// right  0x5213B4  (x+w, y)…(x+w, y+h)                 (x+w-t, y+t)…(x+w-t, y+h-t)
    ///                  0x80000000 -> 0xCC000000            0x00000000
    /// bottom 0x5213E4  (x, y+h)…(x+w, y+h)                 (x+t, y+h-t)…(x+w-t, y+h-t)
    ///                  0x80000000 -> 0xCC000000            0x00FFFFFF -> 0x00000000
    /// ```
    ///
    /// White on the top and left, black on the right and bottom — raised, like 0x521970 — but on
    /// the widget's own edge rather than a pixel inside it, and with no `-1` anywhere.
    pub(super) fn bevel_glow(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, t: f32) {
        let FeRect { x, y, w, h } = rect;
        let c = |v: [u8; 4]| tint(v);
        let (r, b) = (x + w, y + h);
        let bands = [
            ([x, y], [r, y], [c([0xff, 0xff, 0xff, 0xe5]), c([0xff, 0xff, 0xff, 0x80])], [x + t, y + t], [r - t, y + t], [c([0xff, 0xff, 0xff, 0]), c([0, 0, 0, 0])]),
            ([x, y], [x, b], [c([0xff, 0xff, 0xff, 0xcc]), c([0xff, 0xff, 0xff, 0xb2])], [x + t, y + t], [x + t, b - t], [c([0xff, 0xff, 0xff, 0]); 2]),
            ([r, y], [r, b], [c([0, 0, 0, 0x80]), c([0, 0, 0, 0xcc])], [r - t, y + t], [r - t, b - t], [c([0, 0, 0, 0]); 2]),
            ([x, b], [r, b], [c([0, 0, 0, 0x80]), c([0, 0, 0, 0xcc])], [x + t, b - t], [r - t, b - t], [c([0xff, 0xff, 0xff, 0]), c([0, 0, 0, 0])]),
        ];
        self.bevel_bands(out, aspect, &bands, t);
    }

    pub(super) fn bevel(&self, out: &mut Vec<Sprite>, aspect: f32, rect: FeRect, t: f32) {
        let FeRect { x, y, w, h } = rect;
        let (dark_out, dark_in) = (tint([0, 0, 0, 0xcc]), tint([0, 0, 0, 0x19]));
        let (lit_out, lit_in) = (tint([0xff, 0xff, 0xff, 0xcc]), tint([0xff, 0xff, 0xff, 0x33]));
        let (r, b) = (x + w, y + h);
        let bands = [
            ([x + 1.0, y + 1.0], [r - 1.0, y + 1.0], dark_out, [x + t + 1.0, y + t + 1.0], [r - t - 1.0, y + t + 1.0], dark_in),
            ([x + 1.0, y + 1.0], [x + 1.0, b - 1.0], dark_out, [x + t, y + t + 1.0], [x + t, b - t - 1.0], dark_in),
            ([r - 1.0, y + 1.0], [r - 1.0, b - 1.0], lit_out, [r - t - 1.0, y + t + 1.0], [r - t - 1.0, b - t - 1.0], lit_in),
            ([x - 2.0, b - 1.0], [r - 2.0, b - 1.0], lit_out, [x + t + 1.0, b - t - 1.0], [r - t + 1.0, b - t - 1.0], lit_in),
        ];
        let n = (t.round() as usize).max(1);
        for (a0, a1, ca, b0, b1, cb) in bands {
            for d in 0..n {
                let (s0, s1) = (d as f32 / n as f32, (d + 1) as f32 / n as f32);
                let at = |s: f32, p: [f32; 2], q: [f32; 2]| [p[0] + (q[0] - p[0]) * s, p[1] + (q[1] - p[1]) * s];
                let corners = [at(s0, a0, b0), at(s0, a1, b1), at(s1, a0, b0), at(s1, a1, b1)];
                let (px, py) = (corners.iter().fold(f32::MAX, |m, c| m.min(c[0])), corners.iter().fold(f32::MAX, |m, c| m.min(c[1])));
                let cw = corners.iter().fold(f32::MIN, |m, c| m.max(c[0])) - px;
                let ch = corners.iter().fold(f32::MIN, |m, c| m.max(c[1])) - py;
                let s = (s0 + s1) * 0.5;
                let col = std::array::from_fn(|j| ca[j] + (cb[j] - ca[j]) * s);
                out.push(self.quad(aspect, px, py, cw.max(0.5), ch.max(0.5), col));
            }
        }
    }
}
