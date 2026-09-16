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

    /// Sprites for `text` with its top-left at `pos` (screen units 0..1); `height` = line height in screen units.
    pub fn sprites(&self, text: &str, pos: [f32; 2], height: f32, color: [f32; 4]) -> Vec<Sprite> {
        // Metrics (UniversC12b): caps/ascender glyphs are 13 px with ink rows 0..11 (baseline at row 12),
        // x-height glyphs ('m') 10 px, descender glyphs ('g','y') 13 px; `advance` equals the glyph
        // width and `y_offset` is 1 for every glyph, so placement follows `flags`:
        // 1 = ascender (top-aligned), 4 = lowercase (baseline-aligned unless it has a descender),
        // 10 = punctuation (baseline-aligned). HYPOTHESIS — consistent with the rendered menu/HUD.
        let scale = height / self.font.line_height.max(1) as f32; // screen units per texel (vertical)
        // cap height = tallest flags==1 glyph; round capitals overshoot by a pixel and are bottom-aligned
        let cap_h = self.font.glyphs.iter().filter(|g| g.flags == 1).map(|g| g.height).max().unwrap_or(self.font.line_height) as f32;
        let baseline = cap_h;
        let descender = (cap_h * 0.25).round();
        let (aw, ah) = (self.font.atlas_width as f32, self.font.atlas_height as f32);
        let mut x = pos[0];
        let mut out = Vec::new();
        for ch in text.chars() {
            // the FNTF fonts carry ASCII only: fold Latin-1 accents to their base letters
            let ch = match ch {
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
            };
            let Some(g) = self.font.glyph(ch) else {
                x += height * 0.4;
                continue;
            };
            if g.width > 0 {
                out.push(Sprite {
                    texture: self.texture,
                    pos: [x, pos[1] + glyph_top(g, baseline, descender) * scale],
                    size: [g.width as f32 * scale, g.height as f32 * scale],
                    uv_min: [g.x as f32 / aw, g.y as f32 / ah],
                    uv_max: [(g.x + g.width as u16) as f32 / aw, (g.y + g.height as u16) as f32 / ah],
                    color,
                    rotation: 0.0,
                    pivot: [0.0, 0.0],
                });
            }
            x += (g.advance as f32 + 1.0) * scale * 0.85; // 1 px spacing; 0.85 = width/height texel aspect of the atlas as displayed
        }
        out
    }
}

fn glyph_top(g: &nfs_formats::fnt::Glyph, baseline: f32, descender: f32) -> f32 {
    let h = g.height as f32;
    // bottom-align on the baseline; lowercase glyphs as tall as capitals carry a descender
    let has_descender = g.flags & 4 != 0 && h >= baseline - 1.0;
    (baseline - h + if has_descender { descender } else { 0.0 }).max(-1.0)
}
