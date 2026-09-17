//! HUD text from the original `FNTF` bitmap fonts, emitted as one sprite per glyph.

use nfs_formats::fnt::Font;
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};

pub struct BitmapFont {
    font: Font,
    texture: TextureId,
}

impl BitmapFont {
    pub fn upload(r: &mut Renderer, font: Font) -> Self {
        let rgba = font.atlas_rgba();
        let texture = r.upload_texture(&TextureData::single(font.atlas_width as u32, font.atlas_height as u32, rgba));
        Self { font, texture }
    }

    pub fn texture(&self) -> TextureId {
        self.texture
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
        let (aw, ah) = (self.font.atlas_width as f32, self.font.atlas_height as f32);
        let mut x = 0.0f32;
        let mut out = Vec::new();
        for ch in text.chars() {
            let ch = fold(ch);
            let Some(g) = self.font.glyph(ch) else {
                x += self.line_px() * 0.4;
                continue;
            };
            if g.width > 0 {
                out.push(PlacedGlyph {
                    x: x + g.offset_x as f32,
                    y: g.offset_y as f32,
                    w: g.width as f32,
                    h: g.height as f32,
                    uv_min: [g.x as f32 / aw, g.y as f32 / ah],
                    uv_max: [(g.x + g.width as u16) as f32 / aw, (g.y + g.height as u16) as f32 / ah],
                });
            }
            x += g.advance as f32;
        }
        (out, x)
    }

    /// Sprites for `text` with its top-left at `pos` (screen units 0..1); `height` = line height in
    /// screen units. Used by the HUD, whose sprites are square in screen units.
    pub fn sprites(&self, text: &str, pos: [f32; 2], height: f32, color: [f32; 4]) -> Vec<Sprite> {
        let scale = height / self.line_px();
        let (glyphs, _) = self.place(text);
        glyphs
            .iter()
            .map(|g| Sprite {
                texture: self.texture,
                // 0.85 = the width/height texel aspect of the atlas as the HUD displays it
                pos: [pos[0] + g.x * scale * 0.85, pos[1] + g.y * scale],
                size: [g.w * scale, g.h * scale],
                uv_min: g.uv_min,
                uv_max: g.uv_max,
                color,
                rotation: 0.0,
                pivot: [0.0, 0.0],
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

/// The FNTF fonts carry ASCII only: fold Latin-1 accents to their base letters.
fn fold(ch: char) -> char {
    match ch {
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'ä' | 'à' | 'á' | 'â' | 'ã' | 'å' => 'a',
        'Ä' | 'À' | 'Á' | 'Â' | 'Ã' | 'Å' => 'A',
        'ö' | 'ó' | 'ò' | 'ô' | 'õ' => 'o',
        'Ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' => 'O',
        'ü' | 'ú' | 'ù' | 'û' => 'u',
        'Ü' | 'Ú' | 'Ù' | 'Û' => 'U',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        '\u{2019}' | '`' => '\'',
        c => c,
    }
}

