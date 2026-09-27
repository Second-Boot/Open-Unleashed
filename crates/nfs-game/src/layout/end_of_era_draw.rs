//! The end-of-era screen's painting — `FEEndOfEraCallBackList`'s slot 15 **0x4FF1F0** over this
//! screen's painters (`RE/notes/controls.md` §4.150a). The show itself is
//! `crate::end_of_era::EndOfEra`; this loads its art and turns its draws into sprites.

use super::{tint, LayoutScreen, MoviePlayer};
use crate::end_of_era::{self as eoe, Draw, EndOfEra};
use crate::text::BitmapFont;
use nfs_assets::{texture::decode_pack, GamePaths};
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};
use std::collections::HashMap;

/// One uploaded pack entry: texture, width, height and the image's `+0x1C / +0x20` — the pack's
/// `misc` pair negated (`TextureImage::place`), which the tiles are drawn at.
type Art = (TextureId, f32, f32, f32, f32);

pub(super) struct EndOfEraScreen {
    pub(super) show: std::cell::RefCell<EndOfEra>,
    art: HashMap<&'static str, Vec<Art>>,
    font: Option<BitmapFont>,
    pub(super) movie: Option<MoviePlayer>,
}

fn argb(c: u32) -> [f32; 4] {
    tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8])
}

impl LayoutScreen {
    /// Make this the end-of-era screen: the show `show` (built by `EndOfEra::build`), the packs
    /// the constructor loads (0x4FE4D8…0x4FECDD), `Univers20b.ffn` (0x4FD5B7) and the era's movie
    /// (0x4FEC50), which runs once the show starts it.
    pub fn set_end_of_era(&mut self, r: &mut Renderer, paths: &GamePaths, mut show: EndOfEra) {
        // `NFS_EOE_STEPS=n` runs the show n steps ahead before the first frame — a still of the
        // middle of it, the way `NFS_FE_CLOCK` parks the other clocks
        if let Some(n) = std::env::var("NFS_EOE_STEPS").ok().and_then(|v| v.parse::<i64>().ok()) {
            let base = self.end_of_era_now() - n * eoe::STEP_TICKS - 1;
            for k in 0..=n {
                show.frame(base + k * eoe::STEP_TICKS);
            }
        }
        let dir = paths.fe_data().join("Art");
        let mut art = HashMap::new();
        let mut packs: Vec<&'static str> = vec![eoe::OVAL, eoe::NUMBERS, eoe::CURVES, eoe::CURVES2, eoe::LINE];
        packs.extend([eoe::ERA_ART_1[show.previous], eoe::ERA_ART_2[show.previous], eoe::ERA_ART_1[show.current], eoe::ERA_ART_2[show.current]]);
        for name in packs {
            if art.contains_key(name) {
                continue;
            }
            let entries: Vec<Art> = GamePaths::find_file(&dir, name)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|img| {
                    let level0 = img.mips.first()?.clone();
                    let id = r.upload_texture(&TextureData::single(img.width, img.height, level0));
                    Some((id, img.width as f32, img.height as f32, -img.place[0] as f32, -img.place[1] as f32))
                })
                .collect();
            art.insert(name, entries);
        }
        let font = GamePaths::find_file(&dir, eoe::FONT)
            .and_then(|p| std::fs::read(&p).ok())
            .and_then(|d| nfs_formats::fnt::Font::parse(&d).ok())
            .map(|f| BitmapFont::upload(r, f));
        let movie = MoviePlayer::open(r, paths, "Movies", eoe::MOVIES[show.movie.min(2)]);
        self.end_of_era = Some(EndOfEraScreen { show: std::cell::RefCell::new(show), art, font, movie });
    }

    /// True when this is the end-of-era screen and its show has run to the end (`+0x2E5`).
    pub fn end_of_era_finished(&self) -> bool {
        self.end_of_era.as_ref().is_some_and(|e| e.show.borrow().finished)
    }

    /// Slot 1's decision (0x4FF180): true once, when the screen should go back one entry.
    pub fn end_of_era_leave(&self) -> bool {
        let Some(e) = self.end_of_era.as_ref() else { return false };
        let now = self.end_of_era_now();
        e.show.borrow_mut().tick(now)
    }

    /// Slot 4 (0x500770): true when the show takes the key.
    pub fn end_of_era_key(&self, code: u32) -> bool {
        self.end_of_era.as_ref().is_some_and(|e| e.show.borrow_mut().key(code))
    }

    /// The era-complete sting's argument when it is due (0x4FFA7C), once.
    pub fn take_end_of_era_sting(&self) -> Option<i32> {
        let e = self.end_of_era.as_ref()?;
        let now = self.end_of_era_now();
        e.show.borrow_mut().take_sting(now)
    }

    /// `TIMER_getTicks()` — this screen's clock at 128 Hz, from 1 (the stamps take 0 as unset).
    fn end_of_era_now(&self) -> i64 {
        self.clock_ms as i64 * 128 / 1000 + 1
    }

    pub(super) fn end_of_era_sprites(&self, e: &EndOfEraScreen, aspect: f32, out: &mut Vec<Sprite>) {
        let now = self.end_of_era_now();
        let draws = e.show.borrow_mut().frame(now);
        let entry = |pack: &str, k: usize| e.art.get(pack).and_then(|v| v.get(k)).copied();
        let sprite = |out: &mut Vec<Sprite>, a: Art, x: f32, y: f32, flip: u8, c: [f32; 4]| {
            let (pos, size) = self.place(aspect, x, y, a.1, a.2);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: a.0, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: c, rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        };
        let amber = argb(eoe::AMBER);
        // the movie's depth mask, stepped at the first oval piece (0x4FF785). The show's single
        // steps (0x4DA950 before its ramped tiles, 0x4FF3FE / 0x4FF4F0 / 0x4FF606) move only where
        // this one steps from, and nothing masks before it in this draw, so they are not carried.
        let mut mask: Option<super::fe_depth::Mask> = None;
        for d in draws {
            match d {
                Draw::Tiles { pack, alpha } => {
                    let c = argb((alpha as u32) << 24 | 0xFF_FFFF);
                    for &a in e.art.get(pack).into_iter().flatten() {
                        sprite(out, a, a.3, a.4, 0, c);
                    }
                }
                Draw::Corner { sprite: k, x, y, flip } => {
                    let a = match k {
                        0 => entry(eoe::CURVES, 0),
                        1 => entry(eoe::CURVES2, 0),
                        _ => entry(eoe::CURVES2, 1),
                    };
                    if let Some(a) = a {
                        sprite(out, a, x as f32, y as f32, flip, amber);
                    }
                }
                Draw::Rule { x, y, w, h } => out.push(self.quad(aspect, x as f32, y as f32, w as f32, h as f32, amber)),
                // **The oval is a depth mask** (read 2026-09-27): 0x4DAA10 (0x4FF785), then 0x4FFAA0
                // paints the twelve `MovieOval.fsh` pieces — entry 4 (`[+0x2F4]`), 0 (`[+0x2EC]`)
                // and 1 (`[+0x2F0]`), loaded at 0x4FE59C…0x4FE63C — opaque white (`push -1`,
                // 0x4FFB66 / 0x4FFB8D / 0x4FFBB7), so they are drawn and write depth where their
                // art's alpha passes the test; 0x4DAAC0 (0x4FF791); the movie through the player's
                // slot 1 (0x4FF84F) lands only on them; 0x4DAB00 (0x4FF852), and the four lines after
                // it (0x4FFBF0) are outside. *(Until 2026-09-27 the pieces were not painted and the
                // movie was its whole 416 × 348 rectangle, an APPROXIMATION.)*
                Draw::Oval { entry: k, x, y, flip } => {
                    let m = *mask.get_or_insert_with(super::fe_depth::mask_begin);
                    if let Some(a) = entry(eoe::OVAL, k) {
                        let start = out.len();
                        sprite(out, a, x as f32, y as f32, flip, [1.0; 4]);
                        super::fe_depth::apply(&mut out[start..], m.stencil);
                    }
                }
                Draw::Movie { x, y, w, h, alpha } => {
                    if let Some(m) = e.movie.as_ref() {
                        let (pos, size) = self.place(aspect, x as f32, y as f32, w as f32, h as f32);
                        out.push(Sprite { texture: m.tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: argb((alpha as u32) << 24 | 0xFF_FFFF), rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: mask.map(|m| m.inside) });
                    }
                }
                Draw::Line { x, y, flip } => {
                    if let Some(a) = entry(eoe::LINE, 0) {
                        sprite(out, a, x as f32, y as f32, flip, [1.0; 4]);
                    }
                }
                Draw::Digits { digits, x, y, alpha } => {
                    let arts: Vec<Art> = digits.iter().filter_map(|&k| entry(eoe::NUMBERS, k)).collect();
                    // 0x4FFD0B…0x4FFD39: centred by half the sum of the widths, integer
                    let total: i32 = arts.iter().map(|a| a.1 as i32).sum();
                    let mut px = x - total / 2;
                    let c = argb((alpha as u32) << 24 | 0xFF_FFFF);
                    for a in arts {
                        sprite(out, a, px as f32, y as f32, 0, c);
                        px += a.1 as i32;
                    }
                }
                Draw::Text { text, x, y, argb: c, scale } => {
                    if let Some(f) = e.font.as_ref() {
                        self.scaled_text(out, aspect, f, &text, x as f32, y as f32, scale, argb(c));
                    }
                }
            }
        }
    }

    /// 0x4DA350 with alignment 1 and the font's properties 2 and 3 (`+0xB4`, `+0xB8`) at
    /// `(sx, 1.0)` (0x536CE0): the pen's advances, the glyph offsets and widths are scaled by `sx`
    /// (0x53DAEE…0x53DCE3), the heights by 1; the line is centred on `x` by its scaled width.
    #[allow(clippy::too_many_arguments)]
    fn scaled_text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, sx: f32, color: [f32; 4]) {
        let (glyphs, width) = f.place(s);
        let left = (x - width * sx * 0.5).round();
        let top = y.round();
        for g in &glyphs {
            let (pos, size) = self.place(aspect, left + g.x * sx, top + g.y, g.w * sx, g.h);
            out.push(Sprite { texture: f.texture(), pos, size, uv_min: g.uv_min, uv_max: g.uv_max, color, rotation: 0.0, pivot: [0.5, 0.5], tri: None, depth: None });
        }
    }
}
