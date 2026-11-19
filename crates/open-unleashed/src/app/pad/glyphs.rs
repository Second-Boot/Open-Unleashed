//! **Pad button icons** (PORT-DEVIATION D64): small icons beside the
//! actions a standard pad can trigger, shown only while such a pad is connected. The original has no pad
//! menus at all (D10 / D54), so this is the port's own UI and changes no key.
//!
//! The art is drawn here, procedurally, from signed distance shapes (discs, rounded boxes, round-capped
//! strokes) into one RGBA atlas, so nothing is vendored and no licence is involved: a dark disc with a
//! light rim and a coloured letter or symbol, three sets by [`PadKind`] — Xbox letters, PlayStation
//! shapes, Nintendo letters. 128 px tiles keep them sharp at 4K (the icons are 16–24 px of the 480-row
//! design) and the rim keeps them readable on the dark HUD boxes and on the front end's light art alike.

use nfs_input::gamepad::PadKind;
use nfs_renderer::{Sprite, TextureId};

/// One icon of a set. `Face` is the slot of `nfs_input::gamepad::face_buttons`: 0 = A (the front end's
/// select), 1 = B (back), 2 = Y (options / "race again"), 3 = X — the buttons by their labels on a Nintendo
/// pad, by position-independent name everywhere, so a PlayStation pad shows cross / circle / triangle / square.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Glyph {
    Face(usize),
    /// Start (Xbox), Options (PlayStation), + (Nintendo)
    Start,
    /// View (Xbox), Share / Create (PlayStation), − (Nintendo)
    View,
    /// LB / L1 / L
    Prev,
    /// RB / R1 / R
    Next,
}

impl Glyph {
    /// The glyph's column in an atlas row.
    fn column(self) -> usize {
        match self {
            Glyph::Face(s) => s.min(3),
            Glyph::Start => 4,
            Glyph::View => 5,
            Glyph::Prev => 6,
            Glyph::Next => 7,
        }
    }
}

/// The uploaded atlas and the kind it is drawn for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PadIcons {
    pub tex: TextureId,
    pub kind: PadKind,
}

const TILE: usize = 128;
const COLS: usize = 8;
const ROWS: usize = 3;

fn row(kind: PadKind) -> usize {
    match kind {
        PadKind::Xbox => 0,
        PadKind::PlayStation => 1,
        PadKind::Nintendo => 2,
    }
}

/// The atlas cell of a glyph: `(uv_min, uv_max)`.
pub(crate) fn tile_uv(kind: PadKind, g: Glyph) -> ([f32; 2], [f32; 2]) {
    let (c, r) = (g.column() as f32, row(kind) as f32);
    ([c / COLS as f32, r / ROWS as f32], [(c + 1.0) / COLS as f32, (r + 1.0) / ROWS as f32])
}

impl PadIcons {
    /// A sprite of `g` at `pos` / `size` (screen units, as the caller's own `place` gives them).
    pub fn sprite(&self, g: Glyph, pos: [f32; 2], size: [f32; 2]) -> Sprite {
        let (uv_min, uv_max) = tile_uv(self.kind, g);
        Sprite { texture: self.tex, pos, size, uv_min, uv_max, color: [1.0; 4], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None }
    }
}

// ---- the drawing -------------------------------------------------------------------------------------

type Rgb = [f32; 3];

fn hex(c: u32) -> Rgb {
    [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0]
}

/// One tile's canvas: premultiplied RGBA, `TILE` x `TILE`.
struct Canvas {
    px: Vec<[f32; 4]>,
}

/// A shape as a signed distance in tile pixels (negative inside).
type Sdf<'a> = &'a dyn Fn(f32, f32) -> f32;

impl Canvas {
    fn new() -> Self {
        Self { px: vec![[0.0; 4]; TILE * TILE] }
    }

    /// Paint `colour` over the canvas where the shape is (anti-aliased by the distance).
    fn paint(&mut self, sdf: Sdf<'_>, colour: Rgb, alpha: f32) {
        for y in 0..TILE {
            for x in 0..TILE {
                let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
                let cov = (0.5 - d).clamp(0.0, 1.0) * alpha;
                if cov <= 0.0 {
                    continue;
                }
                let p = &mut self.px[y * TILE + x];
                for k in 0..3 {
                    p[k] = colour[k] * cov + p[k] * (1.0 - cov);
                }
                p[3] = cov + p[3] * (1.0 - cov);
            }
        }
    }

    /// Straight-alpha RGBA8; texels with no coverage take `edge` as their colour, so the bilinear and
    /// mip filtering never pulls black into the rim.
    fn rgba(&self, edge: Rgb) -> Vec<u8> {
        let mut out = Vec::with_capacity(TILE * TILE * 4);
        for p in &self.px {
            let a = p[3];
            let c = if a > 1e-4 { [p[0] / a, p[1] / a, p[2] / a] } else { edge };
            out.extend(c.iter().map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
            out.push((a.clamp(0.0, 1.0) * 255.0 + 0.5) as u8);
        }
        out
    }
}

fn disc(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r
}

fn rbox(cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| {
        let (qx, qy) = ((x - cx).abs() - (hw - r), (y - cy).abs() - (hh - r));
        (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r
    }
}

fn ring<F: Fn(f32, f32) -> f32>(inner: F, w: f32) -> impl Fn(f32, f32) -> f32 {
    move |x, y| inner(x, y).abs() - w * 0.5
}

/// A round-capped stroke of width `w` along `a`-`b`: the distance to the segment less half the width.
fn seg_d(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py, ax, ay, bx, by) = (p.0, p.1, a.0, a.1, b.0, b.1);
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy).max(1e-6)).clamp(0.0, 1.0);
    ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt()
}

/// One stroke of a glyph, in the unit square of its box.
#[derive(Clone, Copy)]
enum Stroke {
    Line((f32, f32), (f32, f32)),
    /// centre, radius, from, to (degrees)
    Arc((f32, f32), f32, f32, f32),
}

use Stroke::{Arc, Line};

/// The strokes of the letters the icons use (A B X Y L R 1), in a unit box, y down.
fn letter(c: char) -> Vec<Stroke> {
    match c {
        'A' => vec![Line((0.0, 1.0), (0.5, 0.0)), Line((0.5, 0.0), (1.0, 1.0)), Line((0.2, 0.68), (0.8, 0.68))],
        'B' => vec![
            Line((0.1, 0.0), (0.1, 1.0)),
            Line((0.1, 0.0), (0.55, 0.0)),
            Arc((0.55, 0.25), 0.25, -90.0, 90.0),
            Line((0.55, 0.5), (0.1, 0.5)),
            Line((0.1, 0.5), (0.62, 0.5)),
            Arc((0.62, 0.75), 0.25, -90.0, 90.0),
            Line((0.62, 1.0), (0.1, 1.0)),
        ],
        'X' => vec![Line((0.0, 0.0), (1.0, 1.0)), Line((1.0, 0.0), (0.0, 1.0))],
        'Y' => vec![Line((0.0, 0.0), (0.5, 0.5)), Line((1.0, 0.0), (0.5, 0.5)), Line((0.5, 0.5), (0.5, 1.0))],
        'L' => vec![Line((0.15, 0.0), (0.15, 1.0)), Line((0.15, 1.0), (0.9, 1.0))],
        'R' => vec![
            Line((0.1, 0.0), (0.1, 1.0)),
            Line((0.1, 0.0), (0.55, 0.0)),
            Arc((0.55, 0.25), 0.25, -90.0, 90.0),
            Line((0.55, 0.5), (0.1, 0.5)),
            Line((0.42, 0.5), (0.9, 1.0)),
        ],
        '1' => vec![Line((0.55, 0.0), (0.55, 1.0)), Line((0.55, 0.0), (0.15, 0.22))],
        _ => Vec::new(),
    }
}

/// Paint `strokes` mapped from the unit square onto the box centred at (`cx`, `cy`), `w` x `h`, with a
/// stroke width of `width`.
fn strokes(cv: &mut Canvas, list: &[Stroke], (cx, cy, w, h): (f32, f32, f32, f32), width: f32, colour: Rgb) {
    let (x0, y0) = (cx - w * 0.5, cy - h * 0.5);
    let map = |(u, v): (f32, f32)| (x0 + u * w, y0 + v * h);
    for s in list {
        match *s {
            Line(a, b) => {
                let (a, b) = (map(a), map(b));
                cv.paint(&move |x, y| seg_d((x, y), a, b) - width * 0.5, colour, 1.0);
            }
            Arc(c, r, a0, a1) => {
                // a bowl of the unit box is an ellipse of the real one: a short run of strokes
                const N: usize = 10;
                let at = |k: usize| {
                    let t = (a0 + (a1 - a0) * k as f32 / N as f32).to_radians();
                    map((c.0 + r * t.cos(), c.1 + r * t.sin()))
                };
                for k in 0..N {
                    let (a, b) = (at(k), at(k + 1));
                    cv.paint(&move |x, y| seg_d((x, y), a, b) - width * 0.5, colour, 1.0);
                }
            }
        }
    }
}

fn text(cv: &mut Canvas, s: &str, cx: f32, cy: f32, lw: f32, lh: f32, gap: f32, width: f32, colour: Rgb) {
    let n = s.chars().count() as f32;
    let total = n * lw + (n - 1.0) * gap;
    for (i, c) in s.chars().enumerate() {
        let x = cx - total * 0.5 + lw * 0.5 + i as f32 * (lw + gap);
        strokes(cv, &letter(c), (x, cy, lw, lh), width, colour);
    }
}

const DARK: Rgb = [0.09, 0.09, 0.10];
const RIM: Rgb = [0.93, 0.93, 0.93];
const WHITE: Rgb = [1.0, 1.0, 1.0];

/// The plate: a dark disc (or a rounded box) with a light rim.
fn plate(cv: &mut Canvas, boxed: Option<(f32, f32)>) {
    let c = 64.0;
    match boxed {
        None => {
            cv.paint(&disc(c, c, 61.0), RIM, 1.0);
            cv.paint(&disc(c, c, 56.0), DARK, 1.0);
        }
        Some((hw, hh)) => {
            cv.paint(&rbox(c, c, hw + 5.0, hh + 5.0, hh * 0.5 + 5.0), RIM, 1.0);
            cv.paint(&rbox(c, c, hw, hh, hh * 0.5), DARK, 1.0);
        }
    }
}

fn bars(cv: &mut Canvas, colour: Rgb) {
    for dy in [-17.0, 0.0, 17.0] {
        strokes(cv, &[Line((0.0, 0.5), (1.0, 0.5))], (64.0, 64.0 + dy, 50.0, 0.0), 8.0, colour);
    }
}

/// One tile.
fn draw_tile(kind: PadKind, g: Glyph) -> Vec<u8> {
    let mut cv = Canvas::new();
    let c = 64.0;
    match g {
        Glyph::Face(slot) => {
            plate(&mut cv, None);
            match kind {
                PadKind::PlayStation => {
                    // cross, circle, triangle, square (the slots A, B, Y, X)
                    let (shape, colour) = [("cross", 0x6EA8F0), ("circle", 0xF0624F), ("triangle", 0x4FD3A6), ("square", 0xE592D0)][slot.min(3)];
                    let colour = hex(colour);
                    match shape {
                        "cross" => strokes(&mut cv, &[Line((0.0, 0.0), (1.0, 1.0)), Line((1.0, 0.0), (0.0, 1.0))], (c, c, 44.0, 44.0), 10.0, colour),
                        "circle" => cv.paint(&ring(disc(c, c, 23.0), 10.0), colour, 1.0),
                        "triangle" => {
                            let (a, b, d) = ((c, c - 27.0), (c - 26.0, c + 20.0), (c + 26.0, c + 20.0));
                            cv.paint(&move |x, y| seg_d((x, y), a, b).min(seg_d((x, y), b, d)).min(seg_d((x, y), d, a)) - 5.0, colour, 1.0);
                        }
                        _ => cv.paint(&ring(rbox(c, c, 22.0, 22.0, 3.0), 10.0), colour, 1.0),
                    }
                }
                _ => {
                    let ch = ['A', 'B', 'Y', 'X'][slot.min(3)];
                    let colour = if kind == PadKind::Xbox { hex([0x7CC243, 0xE8453C, 0xFFC20E, 0x2E9BE8][slot.min(3)]) } else { WHITE };
                    strokes(&mut cv, &letter(ch), (c, c, 42.0, 56.0), 10.0, colour);
                }
            }
        }
        Glyph::Start => match kind {
            PadKind::Xbox => {
                plate(&mut cv, None);
                bars(&mut cv, WHITE);
            }
            PadKind::PlayStation => {
                // Options: a disc like every other bar glyph, three bars
                plate(&mut cv, None);
                bars(&mut cv, WHITE);
            }
            PadKind::Nintendo => {
                plate(&mut cv, None);
                strokes(&mut cv, &[Line((0.0, 0.5), (1.0, 0.5)), Line((0.5, 0.0), (0.5, 1.0))], (c, c, 48.0, 48.0), 10.0, WHITE);
            }
        },
        Glyph::View => match kind {
            PadKind::Xbox => {
                plate(&mut cv, None);
                // two overlapping boxes, the "view" mark
                cv.paint(&ring(rbox(c - 7.0, c - 6.0, 18.0, 15.0, 3.0), 7.0), WHITE, 1.0);
                cv.paint(&ring(rbox(c + 7.0, c + 6.0, 18.0, 15.0, 3.0), 7.0), WHITE, 1.0);
            }
            PadKind::PlayStation => {
                plate(&mut cv, None);
                // a box with an arrow leaving it: the Share / Create mark
                cv.paint(&ring(rbox(c, c + 12.0, 24.0, 18.0, 4.0), 7.0), WHITE, 1.0);
                strokes(&mut cv, &[Line((0.5, 1.0), (0.5, 0.0)), Line((0.5, 0.0), (0.15, 0.4)), Line((0.5, 0.0), (0.85, 0.4))], (c, c - 14.0, 32.0, 34.0), 7.0, WHITE);
            }
            PadKind::Nintendo => {
                plate(&mut cv, None);
                strokes(&mut cv, &[Line((0.0, 0.5), (1.0, 0.5))], (c, c, 48.0, 0.0), 10.0, WHITE);
            }
        },
        Glyph::Prev | Glyph::Next => {
            plate(&mut cv, Some((44.0, 30.0)));
            let left = g == Glyph::Prev;
            let (s, lw) = match kind {
                PadKind::Xbox => (if left { "LB" } else { "RB" }, 28.0),
                PadKind::PlayStation => (if left { "L1" } else { "R1" }, 28.0),
                PadKind::Nintendo => (if left { "L" } else { "R" }, 34.0),
            };
            text(&mut cv, s, c, c, lw, 38.0, 8.0, 8.0, WHITE);
        }
    }
    cv.rgba(DARK)
}

/// The whole atlas, `COLS x TILE` wide and `ROWS x TILE` high, RGBA8, plus a premade mip chain
/// (box-filtered, down to one tile pixel per 16): `(width, height, levels)`.
pub(crate) fn atlas() -> (u32, u32, Vec<Vec<u8>>) {
    let (w, h) = (COLS * TILE, ROWS * TILE);
    let mut base = vec![0u8; w * h * 4];
    for kind in [PadKind::Xbox, PadKind::PlayStation, PadKind::Nintendo] {
        for col in 0..COLS {
            let g = match col {
                0..=3 => Glyph::Face(col),
                4 => Glyph::Start,
                5 => Glyph::View,
                6 => Glyph::Prev,
                _ => Glyph::Next,
            };
            let t = draw_tile(kind, g);
            for y in 0..TILE {
                let dst = ((row(kind) * TILE + y) * w + col * TILE) * 4;
                base[dst..dst + TILE * 4].copy_from_slice(&t[y * TILE * 4..(y + 1) * TILE * 4]);
            }
        }
    }
    let mut levels = vec![base];
    let (mut lw, mut lh) = (w, h);
    // the tiles are 128 px, so four halvings keep a tile's pixels apart from its neighbour's
    for _ in 0..4 {
        let prev = levels.last().expect("a level");
        let (nw, nh) = (lw / 2, lh / 2);
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let i = ((2 * y + dy) * lw + 2 * x + dx) * 4;
                    let a = f32::from(prev[i + 3]);
                    for k in 0..3 {
                        acc[k] += f32::from(prev[i + k]) * a;
                    }
                    acc[3] += a;
                }
                let o = (y * nw + x) * 4;
                for k in 0..3 {
                    next[o + k] = if acc[3] > 0.0 { (acc[k] / acc[3] + 0.5) as u8 } else { prev[(2 * y * lw + 2 * x) * 4 + k] };
                }
                next[o + 3] = (acc[3] / 4.0 + 0.5) as u8;
            }
        }
        levels.push(next);
        (lw, lh) = (nw, nh);
    }
    (w as u32, h as u32, levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three sets are all drawn: every tile has a rim (opaque pixels) and the Xbox face letters
    /// carry their colours, the PlayStation symbols theirs.
    #[test]
    fn every_tile_is_drawn() {
        let (w, h, levels) = atlas();
        assert_eq!((w as usize, h as usize), (COLS * TILE, ROWS * TILE));
        assert_eq!(levels[0].len(), w as usize * h as usize * 4);
        for kind in [PadKind::Xbox, PadKind::PlayStation, PadKind::Nintendo] {
            for col in 0..COLS {
                let (x0, y0) = (col * TILE, row(kind) * TILE);
                let alpha = |x: usize, y: usize| levels[0][((y0 + y) * w as usize + x0 + x) * 4 + 3];
                assert_eq!(alpha(64, 64) > 0 || alpha(64, 20) > 0, true, "{kind:?} {col} has a plate");
                assert_eq!(alpha(0, 0), 0, "{kind:?} {col} corner is clear");
            }
        }
        // the Xbox A's letter is green somewhere on its tile
        let green = (0..TILE * TILE).any(|i| {
            let (x, y) = (i % TILE, i / TILE);
            let p = &levels[0][(y * w as usize + x) * 4..][..4];
            p[3] == 255 && p[1] > 150 && p[0] < 150 && p[2] < 120
        });
        assert!(green);
    }

    /// The icon of an action follows the set, and its cell is where `tile_uv` says.
    #[test]
    fn the_cells_follow_the_kind() {
        let (a0, _) = tile_uv(PadKind::Xbox, Glyph::Face(0));
        let (b0, b1) = tile_uv(PadKind::PlayStation, Glyph::Face(1));
        assert_eq!(a0, [0.0, 0.0]);
        assert!(b0[1] > 0.3 && b0[1] < 0.34 && b0[0] == 1.0 / COLS as f32);
        assert!((b1[0] - 2.0 / COLS as f32).abs() < 1e-6);
        assert_ne!(tile_uv(PadKind::Nintendo, Glyph::Start), tile_uv(PadKind::Xbox, Glyph::Start));
        assert_eq!(tile_uv(PadKind::Xbox, Glyph::Face(9)), tile_uv(PadKind::Xbox, Glyph::Face(3)));
    }

    /// The front end's placement: pillar-boxed past 4:3, whole at 4:3.
    #[test]
    fn the_front_end_placement_matches_the_design() {
        let (p, s) = nfs_game::layout::design_place(4.0 / 3.0, 320.0, 240.0, 24.0, 24.0);
        assert!((p[0] - 0.5).abs() < 1e-6 && (p[1] - 0.5).abs() < 1e-6 && (s[1] - 0.05).abs() < 1e-6);
        let (p, _) = nfs_game::layout::design_place(16.0 / 9.0, 0.0, 0.0, 1.0, 1.0);
        assert!((p[0] - 0.125).abs() < 1e-6);
    }
}
