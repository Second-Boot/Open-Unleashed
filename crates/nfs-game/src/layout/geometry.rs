//! The front end's pure widget geometry — scroll bars, combo knobs, dead-zone handles, the colour picker (a move out of the parent module, no logic changed).

use super::*;

/// `FeListScrollBar::hitTest` 0x4DB2F0 over the combo scrollbar's own rectangles, which the
/// constructor 0x4DAF10 lays out from the bar's `(x, y, 16, h)` (§4.90): the up arrow `(x, y, 16,
/// 13)`, the down arrow `(x, y + h - 13, 16, 13)`, the knob `(x + 1, ky, 14, knob_h)` and the
/// trough itself. The order is the binary's, and each test is inclusive at both ends — 0x4DB331
/// takes `x <= px` from `test ah, 1` after `fcompp` and 0x4DB346 takes `px <= x + w` from
/// `test ah, 0x41`. Returns the number 0x4DB2F0 writes to `[this+0xF0]`, 0 for a miss.
pub fn scrollbar_region(x: f32, y: f32, h: f32, rows: usize, first: usize, px: f32, py: f32) -> u32 {
    const ARROW_H: f32 = 13.0;
    let (ky, knob_h, _) = combo_knob(y, h, rows, first);
    for (n, (rx, ry, rw, rh)) in [
        (x, y, LIST_SCROLLBAR_W, ARROW_H),
        (x, y + h - ARROW_H, LIST_SCROLLBAR_W, ARROW_H),
        (x + 1.0, ky, 14.0, knob_h),
        (x, y, LIST_SCROLLBAR_W, h),
    ]
    .into_iter()
    .enumerate()
    {
        if px >= rx && px <= rx + rw && py >= ry && py <= ry + rh {
            return n as u32 + 1;
        }
    }
    0
}

/// `FeListScrollBar`'s knob — `(top, height, travel)` for a bar at `y`, `h` tall, whose model
/// answers `count` positions (slot 9) and sits on `first` (slot 10). The constructor 0x4DAF10 and
/// the re-place slot 11, **0x4DB220**, compute it in integers:
///
/// ```text
/// 0x4DB26A  knobH  = max(16, [+0x118] − 2·[+0x128] − count)   ; h − 2·13 − count
/// 0x4DB2B0  travel = [+0x118] − 2·[+0x128] − knobH
/// 0x4DB2CE  top    = [+0xF4] + (count ? first·travel / count : 0)   ; idiv, truncating
/// ```
///
/// with `[+0xF4]` the knob's resting top, `y + 13` (0x4DAF30 / 0x4DB016). The divisor is the
/// **count**, not `count − 1` — this is not `NFS5.VerticalSlider`'s placement (0x4D0BBE, §4.46),
/// so the knob never quite reaches the down arrow. The drag, 0x4DB9B0, is its exact inverse
/// (`App::fe_apply_combo_drag`). (history: `game-history.md` §31.1)
pub fn combo_knob(y: f32, h: f32, count: usize, first: usize) -> (f32, f32, i32) {
    const ARROW_H: i32 = COMBO_ARROW_H as i32;
    let (h, count) = (h as i32, count as i32);
    let knob_h = (h - 2 * ARROW_H - count).max(16);
    let travel = h - 2 * ARROW_H - knob_h;
    let dy = if count == 0 { 0 } else { first as i32 * travel / count };
    (y + ARROW_H as f32 + dy as f32, knob_h as f32, travel)
}

/// The drag step of `FeListScrollBar`'s mouse-move slot 8, **0x4DB9B0** — the position the model
/// is set to (before its own clamp, slot 11 = 0x4BC240) when the pointer is at `py` and the grab
/// was taken at `grab_y` on position `grab_at`:
///
/// ```text
/// 0x4DBA03  travel = [+0x118] − 2·[+0x128] − [+0x108]          ; h − 2·13 − knobH
/// 0x4DBA18  dy     = ev.y − [+0xE4]                            ; the grab's y
/// 0x4DBA24  pos    = [+0xE8] + dy · model->slot9() / travel    ; imul, idiv — truncating
/// 0x4DBA29  model->slot11(pos)
/// ```
///
/// The event's y is an integer, so both ends are taken to the design pixel first — the port's
/// cursor is sub-pixel (APPROXIMATION of the original's integer mouse position). `None` when the
/// travel is zero (the original would divide by zero there; it cannot arise, since the knob is at
/// most `h − 26` tall only when the bar has no travel and then nothing scrolls).
pub fn combo_drag_pos(py: f32, grab_y: f32, grab_at: usize, count: usize, travel: i32) -> Option<i32> {
    if travel == 0 {
        return None;
    }
    let dy = py.floor() as i32 - grab_y.floor() as i32;
    Some(grab_at as i32 + dy * count as i32 / travel)
}

/// The 150 ms auto-repeat's own arithmetic — `0x529220(this, 0x96, 1)`'s tick. Every held
/// scrollbar arrow shares one shape: `NFS5.VerticalSlider`'s own bar (0x4D0A97/0x4D0B40),
/// `NFS5.CARSETTINGBUTTON` (0x4C1F40/0x4C1F80) and the open combo's own `FeListScrollBar`
/// (`0x4DB040`/`0x4DB1D0`, §4.101) all arm the identical repeating timer on press and step once
/// per firing — `open_unleashed::App`'s `list_repeat` / `setting_repeat` / `combo_arrow_repeat` bank the
/// frame's `dt_ms` against the time already carried and take out whole 150 ms periods, exactly
/// the way the timer's own callback would fire once per elapsed period rather than at most once
/// per game tick. Returns `(steps, new accumulator)`.
pub fn repeat_steps(acc_ms: u32, dt_ms: u32, period_ms: u32) -> (u32, u32) {
    let acc = acc_ms + dt_ms;
    (acc / period_ms, acc % period_ms)
}

/// Where `NFS5.DEADZONE`'s live marker sits, relative to the box's left edge — 0x51B7FF…0x51B84B:
///
/// ```text
/// 0x51B7FF  eax = box.w - 10
/// 0x51B802  eax *= axis          ; the device's own reading, 0…65535
/// 0x51B819  fmul [0x5B25C0]      ; 1/65536
/// 0x51B84B  x = box.l + that
/// ```
///
/// The image sets no `DIPROP_RANGE` — 0x56FD70's only `SetProperty` passes GUID **1**,
/// `DIPROP_BUFFERSIZE` — so an absolute axis keeps DirectInput's default 0…65535, and the
/// ten-pixel marker lands exactly inside the box at either end.
pub fn deadzone_marker(width: i32, axis: i32) -> f32 {
    ((width - 10) * axis.clamp(0, 0xFFFF)) as f32 / 65536.0
}

/// The four handles' centres, relative to the box's left edge, **in the order the draw visits
/// them** — 1, 3, 2, 4 (0x51B883, 0x51B924, 0x51B9C3, 0x51BA64). `half` is `(box.w) / 2`
/// (0x51B712) and a value's pixel offset is `v * half / 128` (0x51B717, 0x51B732), so the `.Min`
/// pair rides either side of the centre and the `.Max` pair further out. Each bar is five wide
/// and starts **two** left of its centre (0x51B8B0).
pub fn deadzone_handles(width: i32, dz: [i32; 2]) -> [(u32, f32); 4] {
    let half = (width / 2).max(1);
    let pix = |v: i32| (v * half / 128) as f32;
    let c = half as f32;
    [(1, c - pix(dz[0])), (3, c + pix(dz[0])), (2, c - pix(dz[1])), (4, c + pix(dz[1]))]
}

/// One handle's new `[min, max]` — the two arms of **0x51BD10**, which keep the pair **five**
/// units apart and inside the accessors' own 0…0x80:
///
/// ```text
/// 0x51BD84  regions 1 and 3:  min = clamp(v, 0, max - 5)
/// 0x51BDC9  regions 2 and 4:  max = clamp(v, min + 5, 0x80)
/// ```
///
/// The jump table at 0x51BE14 is what pairs the four handles down to those two arms.
pub fn deadzone_clamp(region: u32, v: i32, dz: [i32; 2]) -> [i32; 2] {
    match region {
        1 | 3 => [v.min(dz[1] - 5).max(0), dz[1]],
        2 | 4 => [dz[0], v.max(dz[0] + 5).min(0x80)],
        _ => dz,
    }
}

/// The sixteen dead-zone accessors out of the factory: every `.Min` 0 and every `.Max` 0x80
/// (0x51E546, 0x51E550).
pub const DEADZONE_DEFAULT: [[i32; 2]; 8] = [[0, 0x80]; 8];

/// `NFS5.ColorBox`'s custom colour editor, as state — `RE/notes/controls.md` §4.47, §4.48, §4.66.
///
/// The class keeps three floats and an index in the widget, and the eight colours it edits in a
/// global array. 0x4D8900, which the mouse slots pack with, reads the three floats as **hue,
/// saturation and luminance**, not red, green and blue.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Picker {
    /// `[this+0x288]`, `[this+0x28C]`, `[this+0x290]` — hue, saturation, luminance, each
    /// **0.5 out of the factory** (0x4C38B9)
    pub hsl: (f32, f32, f32),
    /// `[this+0x284]`, the well the editor writes — **0** out of the factory (0x4C38B3)
    pub well: usize,
    /// `[0x691D98 + n·4]`, read by `0x5019B0(n)` and written by `0x5019C0(n, v)` — eight
    /// `0xAARRGGBB` words, **all zero in the image**
    pub wells: [u32; 8],
    /// `[this+0x2A0]`…, the copy the draw takes of all eight the first frame the picker is up
    /// (0x4C3CCE…0x4C3CFA, guarded by `[this+0x2C0]`, which the closed draw clears at 0x4C3D03)
    /// — what **cancel** puts back (0x4C49E6)
    pub(super) saved: [u32; 8],
}

impl Default for Picker {
    fn default() -> Self {
        Self { hsl: (0.5, 0.5, 0.5), well: 0, wells: [0; 8], saved: [0; 8] }
    }
}

/// The picker's six rects, `(x0, y0, x1, y1)` each, from the widget's own box — §4.47's table,
/// built by the bind 0x4C3A10. Indexed by [`rect`].
pub fn picker_rects(l: f32, t: f32, r: f32, b: f32) -> [(f32, f32, f32, f32); 6] {
    let _ = t;
    let half = ((r - l) / 2.0).trunc();
    [
        (l + 6.0, b + 6.0, l + 0x79 as f32, b + 0xE as f32),      // A, the luminance ramp
        (l + 0x7D as f32, b + 5.0, r - 5.0, b + 0xF as f32),      // B, the current colour
        (l + 6.0, b + 0x15 as f32, r - 6.0, b + 0x74 as f32),     // C, the hue/saturation field
        (l + 5.0, b + 0x7A as f32, r - 5.0, b + 0x84 as f32),     // D, the eight wells
        (l + 5.0, b + 0x89 as f32, l + half - 3.0, b + 0x9B as f32), // E left, "ok"
        (r - half + 3.0, b + 0x89 as f32, r - 5.0, b + 0x9B as f32), // E right, "cancel"
    ]
}

/// Names for [`picker_rects`]' six.
pub mod rect {
    pub const A: usize = 0;
    pub const B: usize = 1;
    pub const C: usize = 2;
    pub const D: usize = 3;
    pub const E_LEFT: usize = 4;
    pub const E_RIGHT: usize = 5;
}

/// The picker's hit test, `NFS5.ColorBox`'s own **0x4C4AD0**, which every one of its three mouse
/// slots runs first and leaves in `[this+0x218]`.
///
/// It tries the rects in a fixed order and stops at the first that contains the point —
/// **C → 0**, **A → 1**, E left → **3**, E right → **4**, D → **2** — and answers **5** when none
/// does. Rect **B**, the swatch, is never tested: it is not clickable. Each test is `jl`/`jg` on
/// both axes, so a rect is **inclusive on both edges**, like `Control::containsPoint` (§4.56).
pub fn picker_region(rects: &[(f32, f32, f32, f32); 6], x: f32, y: f32) -> u32 {
    let inside = |r: (f32, f32, f32, f32)| x >= r.0 && x <= r.2 && y >= r.1 && y <= r.3;
    for (i, region) in [(rect::C, 0u32), (rect::A, 1), (rect::E_LEFT, 3), (rect::E_RIGHT, 4), (rect::D, 2)] {
        if inside(rects[i]) {
            return region;
        }
    }
    5
}

/// **0x4D8900** — hue, saturation and luminance to a packed `0xAARRGGBB`, alpha forced to 0xFF.
///
/// The textbook HLS→RGB: `h` is scaled to degrees by **360.0** (0x5B25CC at 0x4D8904), the
/// midpoint pair is `m2 = l·(1+s)` when `l <= 0.5` and `l + s - l·s` otherwise (0x4D8926 /
/// 0x4D8932, the 0.5 at 0x5B23F0), `m1 = 2·l - m2` (0x4D8944), and a saturation of exactly **0**
/// returns `(l, l, l)` (0x4D8956). The per-channel helper is inlined three times, at `h + 120`,
/// `h` and `h - 120` (0x5B3D60), each wrapped into `[0, 360)` and then split at **60**, **180**
/// and **240** (0x5B26F0, 0x5B6938, 0x5B6930) with a slope of **1/60** (0x5B3E48). The three come
/// out as red, green and blue in that order and are packed `(v·255)` truncated toward zero
/// (0x5A0F98), red at bit 16 (0x4D8B91…0x4D8BCB).
pub fn hls_to_rgb(h: f32, s: f32, l: f32) -> u32 {
    let m2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let m1 = 2.0 * l - m2;
    let channel = |mut hue: f32| {
        if s == 0.0 {
            return l;
        }
        while hue > 360.0 {
            hue -= 360.0;
        }
        while hue < 0.0 {
            hue += 360.0;
        }
        if hue < 60.0 {
            m1 + (m2 - m1) * hue / 60.0
        } else if hue < 180.0 {
            m2
        } else if hue < 240.0 {
            m1 + (m2 - m1) * (240.0 - hue) / 60.0
        } else {
            m1
        }
    };
    let deg = h * 360.0;
    let (r, g, b) = (channel(deg + 120.0), channel(deg), channel(deg - 120.0));
    let byte = |v: f32| ((v * 255.0) as i32 & 0xff) as u32;
    0xff00_0000 | byte(r) << 16 | byte(g) << 8 | byte(b)
}

/// **0x4D8BD0** — the inverse, a packed `0xAARRGGBB` back to hue, saturation and luminance.
///
/// Each byte is scaled by **1/255** (0x5B3888), `l` is `(max + min)·0.5` (0x4D8CBC), a grey
/// (`max == min`) gives `h = s = 0` (0x4D8CD3), and otherwise `s` is `delta/(max+min)` below the
/// 0.5 midpoint and `delta/(2-(max+min))` above it (0x4D8D12 / 0x4D8D18). The hue is the usual
/// six-sector form — `(g-b)/delta`, `2 + (b-r)/delta`, `4 + (r-g)/delta` (0x4D8D43, 0x4D8D64,
/// 0x4D8D85) — then **×60** (0x5B6940), `+360` while negative (0x4D8DA4) and **×1/360**
/// (0x5B3E84), so the float the widget keeps is a fraction of a turn.
pub fn rgb_to_hls(c: u32) -> (f32, f32, f32) {
    let (r, g, b) = (((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l <= 0.5 { d / (max + min) } else { d / (2.0 - (max + min)) };
    let mut h = if r == max {
        (g - b) / d
    } else if g == max {
        2.0 + (b - r) / d
    } else {
        4.0 + (r - g) / d
    } * 60.0;
    while h < 0.0 {
        h += 360.0;
    }
    (h / 360.0, s, l)
}
