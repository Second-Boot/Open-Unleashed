//! HUD text from the original `FNTF` bitmap fonts, emitted as one sprite per glyph.

use nfs_formats::fnt::Font;
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};

pub struct BitmapFont {
    font: Font,
    texture: TextureId,
    /// Window aspect the screen-space `sprites` lays a line out for (4:3 until told otherwise).
    aspect: f32,
}

impl BitmapFont {
    pub fn upload(r: &mut Renderer, font: Font) -> Self {
        let rgba = font.atlas_rgba();
        let texture = r.upload_texture(&TextureData::single(font.atlas_width as u32, font.atlas_height as u32, rgba));
        Self { font, texture, aspect: 4.0 / 3.0 }
    }

    /// Tell the font what shape the window is, so a screen-space line keeps the original's spacing.
    pub fn set_aspect(&mut self, aspect: f32) {
        self.aspect = aspect.max(0.1);
    }

    pub fn texture(&self) -> TextureId {
        self.texture
    }

    /// Whether the text draw finds a glyph for `ch` — its Windows-1252 byte, else that byte in the
    /// other case ([`Font::lookup`], 0x53DBCC…0x53DBFD).
    pub fn has_glyph(&self, ch: char) -> bool {
        self.font.lookup(ch).is_some()
    }

    /// The font's line height in texels.
    pub fn line_px(&self) -> f32 {
        self.font.line_height.max(1) as f32
    }

    /// Lay `text` out in font texels with the line's top-left at the origin; also returns the
    /// total advance, which is what a right- or centre-aligned draw subtracts.
    ///
    /// Placement is the original's (`FONT_buildmesh` 0x53DC53…0x53DCD4, CONFIRMED): the glyph quad
    /// sits at the pen plus the record's signed `(offset_x, offset_y)` and the pen then moves on by
    /// `advance`. Nothing is inferred from the glyph heights.
    pub fn place(&self, text: &str) -> (Vec<PlacedGlyph>, f32) {
        let (glyphs, pen, _) = self.layout(text);
        (glyphs, pen)
    }

    /// The loop of 0x53DAB0 (CONFIRMED), in font texels from a pen at 0: returns the quads, the
    /// pen's end and the low pen mark.
    ///
    /// * Each char is read as its Windows-1252 byte (0x573AB0, [`Font::lookup`]); a NUL ends the
    ///   string (0x53DB47).
    /// * A byte ≤ 0x20 makes no quad (0x53DB2B…0x53DBC7): the space takes the glyph path only
    ///   while the text's background colour, property 9 (`+0x60`), has a non-zero alpha
    ///   (0x53DB36), and it is `0x00FFFFFF` from creation (0x53C7D1) — the setter 0x53E5C0 is
    ///   called with 9 only by the car-texture text 0x4449xx — so for the front end and the HUD
    ///   a space only moves the pen. Such a byte first pulls the low pen mark down to the pen
    ///   (0x53DB4D…0x53DB5D); then `'\n'` puts the pen back at the line's start and down by
    ///   [`Font::line_step`] (0x53DB60…0x53DB77); anything else (the tab too: the tab stops
    ///   `+0x40` stay null, 0x53C78D is 0x53EEA0's one caller) moves it by the space record's
    ///   `advance`, 0 without one (0x53DB09…0x53DB19, 0x53DBB8).
    /// * Any other byte is the glyph, else the glyph of its other case (0x53E270), else nothing:
    ///   no quad and **no advance** (0x53DBE9 / 0x53DBFD). A found glyph always makes a quad,
    ///   zero-sized or not (0x53DC53…0x53DCBC), then advances the pen (0x53DCD4). Kerning
    ///   (0x53DC28, property 7) is off: its table's installer 0x53EF20 has no caller.
    ///
    /// The low mark starts 1000 right of the pen (0x53DAD6).
    fn layout(&self, text: &str) -> (Vec<PlacedGlyph>, f32, f32) {
        let (aw, ah) = (self.font.atlas_width as f32, self.font.atlas_height as f32);
        let space = self.font.glyph_code(0x20).map_or(0.0, |g| g.advance as f32);
        let (mut x, mut y, mut mark) = (0.0f32, 0.0f32, 1000.0f32);
        let mut out = Vec::new();
        for ch in text.chars() {
            let code = nfs_formats::locale::to_cp1252(ch);
            match code {
                Some(0) => break,
                Some(c) if c <= 0x20 => {
                    mark = mark.min(x);
                    if c == b'\n' {
                        x = 0.0;
                        y += self.font.line_step as f32;
                    } else {
                        x += space;
                    }
                    continue;
                }
                _ => {}
            }
            let Some(g) = self.font.lookup(ch) else { continue };
            out.push(PlacedGlyph {
                x: x + g.offset_x as f32,
                y: y + g.offset_y as f32,
                w: g.width as f32,
                h: g.height as f32,
                uv_min: [g.x as f32 / aw, g.y as f32 / ah],
                uv_max: [(g.x + g.width as u16) as f32 / aw, (g.y + g.height as u16) as f32 / ah],
            });
            x += g.advance as f32;
        }
        (out, x, mark)
    }

    /// The string's measure as 0x536970 → 0x53ECE0 gives it: the box of its glyph quads, the
    /// width reaching to the pen's end when that is further right (0x53EDB3) and to the low pen
    /// mark when that is further left (0x53EDC8 — a leading space or line break), the height the
    /// quads' own span — not the font's line height. `(0, 0)` for a string with no glyph
    /// (0x53ED4C).
    pub fn measure(&self, text: &str) -> (f32, f32) {
        let (_, _, w, h) = self.measure_box(text);
        (w, h)
    }

    /// All four of 0x536970's outputs (0x53EE4B…0x53EE89): `(minX, minY, maxX − minX, maxY −
    /// minY)`, zeros for a string with no glyph.
    pub fn measure_box(&self, text: &str) -> (f32, f32, f32, f32) {
        let (glyphs, pen, mark) = self.layout(text);
        if glyphs.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }
        let min_x = glyphs.iter().map(|g| g.x).fold(f32::MAX, f32::min).min(mark);
        let max_x = glyphs.iter().map(|g| g.x + g.w).fold(f32::MIN, f32::max).max(pen);
        let min_y = glyphs.iter().map(|g| g.y).fold(f32::MAX, f32::min);
        let max_y = glyphs.iter().map(|g| g.y + g.h).fold(f32::MIN, f32::max);
        (min_x, min_y, max_x - min_x, max_y - min_y)
    }

    /// Sprites for `text` with its top-left at `pos` (screen units 0..1); `height` = line height in
    /// screen units. Used by the HUD, whose sprites are square in screen units.
    /// Lay a line out at `pos` in **screen** units with glyphs `height` tall (also in screen units,
    /// i.e. a fraction of the window's height).
    ///
    /// A glyph's advance is in the same units as its height, so turning it into a horizontal screen
    /// fraction divides by the window's aspect — `set_aspect` is what tells the font which window it
    /// is drawing into. Without it a wide window spreads a line out (the port used to carry a fixed
    /// 0.85 here, right only at 4:3). The front end's screens and the HUD do not come through this
    /// path: they place each glyph in the original's 640×480 space themselves.
    pub fn sprites(&self, text: &str, pos: [f32; 2], height: f32, color: [f32; 4]) -> Vec<Sprite> {
        let scale = height / self.line_px();
        let advance = scale / self.aspect;
        let (glyphs, _) = self.place(text);
        glyphs
            .iter()
            .map(|g| Sprite {
                texture: self.texture,
                pos: [pos[0] + g.x * advance, pos[1] + g.y * scale],
                size: [g.w * scale, g.h * scale],
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                color,
                rotation: 0.0,
                pivot: [0.0, 0.0],
                tri: None, corner_colors: None,
                depth: None,
            })
            .collect()
    }
}

/// One glyph box in font texels, relative to the text's top-left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedGlyph {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}
