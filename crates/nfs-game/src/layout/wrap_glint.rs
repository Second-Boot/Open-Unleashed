//! The word wrap (`FeWordWrap` 0x523600) and the glinty-box piece geometry as pure functions (a move out of the parent module, no logic changed).

use super::*;

/// Break `s` into lines that fit `w`, the way `FeWordWrap` does it — **0x523600**.
///
/// The original does not fit words forward; it **guesses** a break and walks back to a space:
///
/// ```text
/// 0x523679  strip the trailing spaces
/// 0x5236C1  first line:  guess = start + trunc(width * 0.125) + 10     ; [0x5BAF60] is -0.125
/// 0x5236B7  after that:  guess = start + (the previous line's length) + 10
/// 0x5236E7  if the guess is past the end, the line is the whole remainder
/// 0x5236F5  else walk back to the last space, then back over the run of spaces
/// 0x52374F  measure the line, and while its width is **not strictly less** than the box
/// 0x523765  walk back another word and measure again
/// 0x52371D  a line with no space left in it is taken as it stands
/// ```
///
/// Two of those matter and the port used neither: a line can never reach **past** the guess, so a
/// narrow font breaks earlier than a greedy fit would; and the test is `<`, not `<=`, so a line
/// that comes out exactly the width of the box is one word too long.
///
/// The width tested is 0x536970's **third** output (`[esp+0x18]` against the box at 0x523754):
/// the box of the glyph quads widened to the pen's end and the low pen mark
/// (`BitmapFont::measure().0`), not the pen's advance the port used before.
pub fn wrap_lines(f: &BitmapFont, s: &str, w: f32) -> Vec<String> {
    wrap_lines_by(&|t: &str| f.measure(t).0, s, w)
}

/// The wrap object's line pitch and total height, as 0x523600 keeps them. Each committed line
/// was last measured by 0x536970 at 0x52374F, and its **height** — the fourth output, the span of
/// its glyph quads, not the font's line step — plus 2.0 (`[0x5B2580]`) is added to the total
/// `[+4]` and written to `[+0x10]` (0x523912…0x52393F). The total is what 0x523250 answers (the
/// tooltip's and the dialog's height); the draw 0x523290 steps every line down by `[+0x10]`
/// (0x523576, 0x5235DA), which is therefore the **last** line's height + 2.
pub fn wrap_metrics(f: &BitmapFont, lines: &[String]) -> (f32, f32) {
    let step = |l: &String| f.measure(l).1 + 2.0;
    let total = lines.iter().map(step).sum();
    (lines.last().map_or(0.0, step), total)
}

/// The same, against any metric — which is how it is tested without a renderer to upload a font.
pub fn wrap_lines_by(measure: &dyn Fn(&str) -> f32, s: &str, w: f32) -> Vec<String> {
    let text: Vec<char> = s.trim_end_matches(' ').chars().collect();
    let width = |c: &[char]| measure(&c.iter().collect::<String>());
    let mut out = Vec::new();
    let mut start = 0usize;
    // the first guess is the box's width over 8 — one character per 8 pixels — and 10 more
    let mut reach = (w * 0.125).trunc().max(0.0) as usize + 10;
    while start < text.len() {
        let guess = start + reach;
        let mut cut = if guess >= text.len() {
            text.len()
        } else {
            // back to the last space, then over the run of spaces before it
            let mut i = guess;
            while i > start && text[i] != ' ' {
                i -= 1;
            }
            while i > start && text[i] == ' ' {
                i -= 1;
            }
            if i == start { text.len() } else { i + 1 }
        };
        // and back another word while the line is not strictly narrower than the box
        while cut > start && width(&text[start..cut]) >= w {
            let mut i = cut - 1;
            while i > start && text[i] != ' ' {
                i -= 1;
            }
            while i > start && text[i] == ' ' {
                i -= 1;
            }
            if i == start {
                break;
            }
            cut = i + 1;
        }
        let line: String = text[start..cut].iter().collect();
        let line = line.trim_end_matches(' ').to_string();
        reach = cut - start + 10;
        out.push(line);
        start = cut;
        while start < text.len() && text[start] == ' ' {
            start += 1;
        }
    }
    out
}

/// One piece of `NFS5.GlintyRectangle`'s flourish: a rectangle relative to the widget's origin
/// and the two colours it ramps between, the first at its left or top edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlintPiece {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub c0: [u8; 4],
    pub c1: [u8; 4],
    /// the ramp runs along x; otherwise along y
    pub along_x: bool,
}

/// The flourish of **0x51FEE8…0x52061C** — the bright segment that runs round a glinty
/// rectangle's outline, as at most four pieces. `RE/notes/controls.md` §4.27.
///
/// Two segments travel the border in mirror image, each drawn as one piece on a vertical edge and
/// one on a horizontal edge as it turns the corner, all of them **one pixel** thick, through the
/// gouraud rect 0x4D96B0. `p` is the leading edge and `band` the segment's length; `variant`,
/// 1…4, is `animate` — which corner the pair starts from.
///
/// Every rule here is one of the sixteen arms behind the four jump tables at 0x520628, 0x520638,
/// 0x520648 and 0x520658, with the three-way split before each table at 0x51FF5E / 0x520036 /
/// 0x520063 and 0x5202AC / 0x520399 / 0x5203D4, and the gates at 0x51FFF2, 0x5201AB, 0x520338 and
/// 0x520525.
pub fn glint_pieces(w: i32, h: i32, p: i32, band: i32, variant: i32) -> Vec<GlintPiece> {
    let mut out = Vec::new();
    if band <= 0 || w < 2 || h < 2 {
        return out;
    }
    // 0x4D8850: `(c0·(255−t) + c1·t) / 255`, channel by channel
    let lerp = |c0: [u8; 4], c1: [u8; 4], t: i32| -> [u8; 4] {
        let t = t.clamp(0, 255) as u32;
        std::array::from_fn(|k| ((c0[k] as u32 * (255 - t) + c1[k] as u32 * t) / 255) as u8)
    };
    // 0xDDFFFFFF and 0x00FFFFFF as this port spells colours. `count` is the loop index handed to
    // 0x4D8850 as a colour at 0x51FFA9 — a near-transparent black, which is why a segment's far
    // end disappears; it is kept rather than tidied because it is what the original mixes.
    let glint: [u8; 4] = [0xff, 0xff, 0xff, 0xdd];
    let clear: [u8; 4] = [0xff, 0xff, 0xff, 0x00];
    let count = |i: i32| -> [u8; 4] { [0, 0, i.clamp(0, 255) as u8, 0] };
    let (wf, hf) = (w as f32, h as f32);
    let mut push = |x: f32, y: f32, pw: f32, ph: f32, c0, c1, along_x| {
        // the original passes a signed extent, so a negative one runs back from `x`/`y`
        let (x, pw) = if pw < 0.0 { (x + pw, -pw) } else { (x, pw) };
        let (y, ph) = if ph < 0.0 { (y + ph, -ph) } else { (y, ph) };
        if pw >= 0.5 && ph >= 0.5 {
            out.push(GlintPiece { x, y, w: pw, h: ph, c0, c1, along_x });
        }
    };
    // ---- the segment that rides a vertical edge first, then a horizontal one (0x51FF5E…)
    let (i0, i1, a, ca0, ca1, cb0, cb1);
    if p > h || band + p < h {
        if p > h {
            i0 = 0;
            i1 = (w + h - p).min(band);
            a = p;
        } else if p < 0 {
            i0 = band + p;
            i1 = 0;
            a = 0;
        } else {
            i0 = band;
            i1 = 0;
            a = p;
        }
        ca0 = lerp(count(i0), glint, 255 - i0 * 255 / band);
        ca1 = glint;
        cb0 = clear;
        cb1 = lerp(clear, glint, i1 * 255 / band);
    } else {
        // 0x520063: the segment straddles the corner, and both pieces take the one blend
        i0 = h - p;
        i1 = band - i0;
        a = p;
        let t = lerp(clear, glint, i0 * 255 / band);
        ca0 = clear;
        ca1 = t;
        cb0 = t;
        cb1 = glint;
    }
    if a < h {
        // table 0x520628: left edge for 1 and 3, right for 2 and 4; down for 1 and 2, up for 3, 4
        let x = if variant == 1 || variant == 3 { 0.0 } else { wf - 1.0 };
        let (y, hh) = if variant <= 2 { (a as f32, i0 as f32) } else { (hf - a as f32, -i0 as f32) };
        push(x, y, 1.0, hh, ca0, ca1, false);
    }
    if band + a >= h {
        // table 0x520638: the bottom edge for 1 and 2, the top for 3 and 4
        let b = (a - h).max(0) as f32;
        let (x, ww) = if variant == 1 || variant == 3 { (b, i1 as f32) } else { (wf - b, -(i1 as f32)) };
        let y = if variant <= 2 { hf - 1.0 } else { 0.0 };
        push(x, y, ww, 1.0, cb0, cb1, true);
    }
    // ---- and its mirror, riding a horizontal edge first (0x5202AC…)
    let (j0, j1, c, cc0, cc1, cd0, cd1);
    if p > w || band + p < w {
        if p > w {
            j0 = 0;
            j1 = (w + h - p).min(band);
            c = p;
        } else if p < 0 {
            j0 = band + p;
            j1 = 0;
            c = 0;
        } else {
            j0 = band;
            j1 = 0;
            c = p;
        }
        cc0 = lerp(count(j0), glint, 255 - j0 * 255 / band);
        cc1 = glint;
        cd0 = clear;
        cd1 = lerp(clear, glint, j1 * 255 / band);
    } else {
        // 0x5203D4
        j0 = w - p;
        j1 = band - j0;
        c = p;
        let t = lerp(clear, glint, j0 * 255 / band);
        cc0 = clear;
        cc1 = t;
        cd0 = t;
        cd1 = glint;
    }
    if c < w {
        // table 0x520648: the top edge for 1 and 2, the bottom for 3 and 4
        let (x, ww) = if variant == 1 || variant == 3 { (c as f32, j0 as f32) } else { (wf - c as f32, -(j0 as f32)) };
        let y = if variant <= 2 { 0.0 } else { hf - 1.0 };
        push(x, y, ww, 1.0, cc0, cc1, true);
    }
    if band + c >= w {
        // table 0x520658: the right edge for 1 and 3, the left for 2 and 4
        let d = (c - w).max(0) as f32;
        let x = if variant == 1 || variant == 3 { wf - 1.0 } else { 0.0 };
        let (y, hh) = if variant <= 2 { (d, j1 as f32) } else { (hf - d, -(j1 as f32)) };
        push(x, y, 1.0, hh, cd0, cd1, false);
    }
    out
}

/// `NFS5.GLINTYLINE`'s sweep — the bright segment that slides along the line, on top of the bar
/// the class draws for its body (§4.28). `RE/notes/controls.md` §4.29.
///
/// The object's setup (0x51F9A0) puts the sweep's range in `[+0x40]`/`[+0x44]`: **the long axis**,
/// left…right when `data.horizontal` is set and top…bottom when it is not. Its width is a fifth of
/// that range (0x51FB38, the same 0x66666667 as the rectangle's band), it starts one width outside
/// one end and runs to one width past the other at `rand() % 3 + 3` pixels a tick (0x51FB4D,
/// 0x51FBC5), and between runs it waits `rand() % 128` (0x51FC10).
///
/// The rectangle it draws is one pixel thick across the line and `width` long along it (0x51FCCA
/// for the horizontal case, 0x51FD71 for the vertical), in a ramp between **0xDDFFFFFF** through
/// the fade and **0x00FFFFFF** (0x51FBA1), and it goes through **0x4D94A0** rather than 0x4D96B0 —
/// the same gouraud rect but **clipped to a box**, the widget's own, with the colours interpolated
/// at the cut (0x4D94C8…0x4D9577). Returns the clipped rectangle and the two colours, or `None`
/// when it is entirely outside.
///
/// `along` is the position of the leading edge on the long axis, relative to the widget's origin.
pub fn glint_sweep(len: f32, along: f32, width: f32, forward: bool) -> Option<(f32, f32, [u8; 4], [u8; 4])> {
    let glint: [u8; 4] = [0xff, 0xff, 0xff, 0xdd];
    let clear: [u8; 4] = [0xff, 0xff, 0xff, 0x00];
    let (c0, c1) = if forward { (clear, glint) } else { (glint, clear) };
    let lerp = |a: [u8; 4], b: [u8; 4], t: f32| -> [u8; 4] {
        let t = t.clamp(0.0, 1.0);
        std::array::from_fn(|k| (a[k] as f32 + (b[k] as f32 - a[k] as f32) * t).round() as u8)
    };
    let (mut s, mut e) = (along, along + width);
    if e <= 0.0 || s >= len || width <= 0.0 {
        return None;
    }
    let (mut ca, mut cb) = (c0, c1);
    if s < 0.0 {
        ca = lerp(c0, c1, -s / width);
        s = 0.0;
    }
    if e > len {
        cb = lerp(c0, c1, (len - along) / width);
        e = len;
    }
    (e - s >= 0.5).then_some((s, e - s, ca, cb))
}

/// `band = min((w+h)/5, w−1, h−1)` (0x51FEE8…0x51FF2E) and the leading edge
/// `p = (band + w + h)·elapsed/duration − band` (0x51FF32…0x51FF4D).
pub fn glint_band(w: i32, h: i32) -> i32 {
    ((w + h) / 5).min(w - 1).min(h - 1)
}
