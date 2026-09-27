//! The career era tabs `FEBottomBarMenu` paints under every career screen — its slot 41,
//! **0x4D83F0** (vtable 0x5B6888 `+0xA4`), which the front end's draw 0x4D2F30 calls at 0x4D30F2,
//! after the background screen `[fe+0xE4]` and before the bottom layer's screen
//! (`RE/notes/career.md` §12). What it draws with is loaded by the bar's slot 21, **0x4D7FD0**.

use super::{tint, LayoutScreen, TRANS_FULL};
use crate::text::BitmapFont;
use nfs_assets::{texture::decode_pack, GamePaths};
use nfs_formats::locale::Strings;
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};

/// `%scareertabs.fsh` (0x5D9598) and its three entries `Tab1` / `Tab2` / `Tab3` (0x5D9604,
/// 0x5D95FC, 0x5D95F4 through 0x4D7B20), kept at `+0x280/+0x284/+0x288`
const PACK: &str = "careertabs.fsh";
const TAGS: [&str; 3] = ["Tab1", "Tab2", "Tab3"];
/// `%sUnivers7b.ffn` (0x5D7F90) → `+0x28C`, the three era names; `%sUnivers11b.ffn` (0x5D8CEC)
/// → `+0x290`, the year
const NAME_FONT: &str = "univers7b.ffn";
const YEAR_FONT: &str = "univers11b.ffn";
/// The three columns, `212.0`, `320.0`, `426.0` (0x4D84E2, 0x4D8526, 0x4D856A; the year's
/// 0x4D865A / 0x4D8615 / 0x4D85CD repeat them)
const COLUMN_X: [f32; 3] = [212.0, 320.0, 426.0];
/// the names at y `4.0` (0x4D84DD…), the year at `16.0` (0x4D8655…)
const NAME_Y: f32 = 4.0;
const YEAR_Y: f32 = 16.0;
/// `FEString` 0x1E0…0x1E2 (0x4D84C6, 0x4D850A, 0x4D854E), one per era
const NAME_STRING: u32 = 0x1E0;
/// 0x4D847F / 0x4D848C / 0x4D849D: the year's gold, the current era's name, the other two
const GOLD: u32 = 0xCCBB_A25A;
const LIT: u32 = 0xCCC3_B898;
const DIM: u32 = 0xCC5A_5A5A;

/// One uploaded entry: texture, width, height and the image's `+0x1C / +0x20` — the pack's `misc`
/// pair negated (`TextureImage::place`), where 0x4D9AF0 is told to put it.
type Art = (TextureId, f32, f32, f32, f32);

pub(super) struct CareerTabs {
    tabs: [Option<Art>; 3],
    name_font: Option<BitmapFont>,
    year_font: Option<BitmapFont>,
}

/// 0x4D87F0: `c`'s alpha scaled by `level / 255` (`mul 0x80808081`, `shr 7` — the exact floor of
/// the quotient), the colour itself untouched; `level` 255 answers `c` as it is.
pub fn fade(c: u32, level: u32) -> u32 {
    if level == 0xFF {
        return c;
    }
    ((c >> 24) * level / 255) << 24 | (c & 0xFF_FFFF)
}

/// `255 − (p·255) / 512` (0x4D8400…0x4D8414, `idiv`): `p` the bar's 0x4E0D90(1) — the larger of
/// its own transition progress and the front screen's (0x4D6300).
pub fn level(progress: i32) -> u32 {
    (255 - progress * 255 / TRANS_FULL).clamp(0, 255) as u32
}

fn argb(c: u32) -> [f32; 4] {
    tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8])
}

impl LayoutScreen {
    /// The bottom bar's slot 21, **0x4D7FD0**: after the layout's own load (0x4E0840) the career
    /// tab pack and the two fonts (0x4D7FEA…0x4D80BA).
    pub fn load_career_tabs(&mut self, r: &mut Renderer, paths: &GamePaths) {
        let dir = paths.fe_data().join("Art");
        let images = GamePaths::find_file(&dir, PACK)
            .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
            .and_then(|(p, d)| decode_pack(&p, &d).ok())
            .unwrap_or_default();
        let tabs = TAGS.map(|tag| {
            let img = images.iter().find(|i| i.name == tag)?;
            let level0 = img.mips.first()?.clone();
            let id = r.upload_texture(&TextureData::single(img.width, img.height, level0));
            Some((id, img.width as f32, img.height as f32, -img.place[0] as f32, -img.place[1] as f32))
        });
        let mut font = |name: &str| {
            GamePaths::find_file(&dir, name)
                .and_then(|p| std::fs::read(&p).ok())
                .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
                .map(|f| BitmapFont::upload(r, f))
        };
        let name_font = font(NAME_FONT);
        let year_font = font(YEAR_FONT);
        self.career_tabs = Some(CareerTabs { tabs, name_font, year_font });
    }

    /// Slot 41, **0x4D83F0**, once the caller has made its three tests — a current screen
    /// (`[[0x65B928]+0xE0]`), `Game.GameType` 4 and the screen's slot `0xAC` false, the same gate
    /// as the bar's `ID_CASHBKGD` (0x4D82E2…0x4D832C). `era` is `[0x691E50] − 0x209` (0x5032D0),
    /// `year` `[0x5DB764]` (0x5032C0), `front_progress` the front screen's transition progress
    /// (0x4D6300).
    ///
    /// The three names first, at their columns, centred (alignment 1), the current era's in the
    /// lit colour and the others dim; then — only for an era of the three (0x4D8578…0x4D8589) —
    /// its tab at the art's own place (0x4D9AF0 with `+0x1C`, `+0x20`) in white at the fade's
    /// alpha (0x4D87D0), and the year (`"%d"`, 0x5CD1B0) under its name in gold.
    pub fn career_tabs_sprites(&self, aspect: f32, strings: &Strings, front_progress: i32, era: Option<usize>, year: i32) -> Vec<Sprite> {
        let mut out = Vec::new();
        let Some(t) = self.career_tabs.as_ref() else { return out };
        let level = level(self.transition.progress().max(front_progress));
        let (gold, lit, dim) = (fade(GOLD, level), fade(LIT, level), fade(DIM, level));
        if let Some(f) = t.name_font.as_ref() {
            for (i, &x) in COLUMN_X.iter().enumerate() {
                let c = if era == Some(i) { lit } else { dim };
                self.text(&mut out, aspect, f, &strings.text(NAME_STRING + i as u32), x, NAME_Y, 0.0, 1, argb(c));
            }
        }
        let Some(k) = era.filter(|&k| k < 3) else { return out };
        if let Some(a) = t.tabs[k] {
            let (pos, size) = self.place(aspect, a.3, a.4, a.1, a.2);
            out.push(Sprite { texture: a.0, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: argb(level << 24 | 0xFF_FFFF), rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
        if let Some(f) = t.year_font.as_ref() {
            self.text(&mut out, aspect, f, &year.to_string(), COLUMN_X[k], YEAR_Y, 0.0, 1, argb(gold));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_and_level_are_the_originals() {
        // 0x4D87F0: level 255 passes the colour through; otherwise alpha · level / 255, floored
        assert_eq!(fade(GOLD, 255), GOLD);
        assert_eq!(fade(GOLD, 0), 0x00BB_A25A);
        assert_eq!(fade(0xCC00_0000, 128), (0xCC * 128 / 255) << 24);
        // 255 − p·255/512, truncating
        assert_eq!(level(0), 255);
        assert_eq!(level(512), 0);
        assert_eq!(level(1), 255);
        assert_eq!(level(3), 254);
        assert_eq!(level(256), 128);
    }
}
