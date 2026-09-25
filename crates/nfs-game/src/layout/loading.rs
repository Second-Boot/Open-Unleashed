//! The front end's loading screen — **0x4DDA00(step)** (`RE/notes/controls.md` §4.165.3).
//!
//! 0x4D3420 calls it on every entry but the first (`[esp+0x23]` = `[0x5D9158] == 0`, the byte
//! 0x4D3E12 / 0x4D3EA7 clear after start-up), with steps 0…8 through its load (0x4D345C …
//! 0x4D3B42) and 9 at the end (0x4D521D). Each call is one whole frame:
//!
//! ```text
//! 0x4DDA0F  MusicStop(1), then wait while the stream is still going (0x4AD670 / 0x4AD6C0)
//! 0x4DDA48  loadtext.fsh, once, when LANGUAGE [0x657C84] != 0 and the file exists
//! 0x4DDA95  "%sera%d.fsh" with 0 — era0.fsh, once
//! 0x4DDADC  era0.fsh's 45 entries as tiles: 20 of 32×32 on y 0, 10 of 64×64 on y 32, then
//!           three rows of 5 of 128×128 on y 96, 224, 352 (0x563440)
//! 0x4DDC3B  LANGUAGE != 0: loadtext entry LANGUAGE−1 at its own (−misc0, −misc1) (0x562810)
//! 0x4DDC8C  step ≥ 0: the bar (0x562680) at 302, 467, ftol(step · 13.777778) × 6, 0xFFFFBB00
//! 0x4DDCC3  end the frame and flip (0x534550, 0x534580)
//! 0x4DDCCD  step ≥ 9: both packs freed
//! ```
//!
//! `[0x5B6B28]` = 13.777778 = 124 / 9, so step 9 is a 124-pixel bar. English (`LANGUAGE` 0) has
//! no caption drawn: the loadtext pack's six entries are `Fren`, `Germ`, `Ital`, `Span`, `Swed`,
//! `Port` — the English word is on the era background itself.

use super::DESIGN;
use nfs_assets::{texture::decode_pack, GamePaths};
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};

/// One uploaded pack entry: texture, width, height, and `-place` (where the entry says it goes).
type Art = (TextureId, f32, f32, f32, f32);

/// `[0x5B6B28]`, the bar's pixels per step.
pub const BAR_PER_STEP: f32 = 13.777_778;
/// The last step; its call frees the packs.
pub const LAST_STEP: u32 = 9;

pub struct LoadingScreen {
    tiles: Vec<Art>,
    caption: Option<Art>,
    white: TextureId,
}

fn load_pack(r: &mut Renderer, paths: &GamePaths, name: &str) -> Vec<Art> {
    let dir = paths.fe_data().join("Art");
    GamePaths::find_file(&dir, name)
        .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
        .and_then(|(p, d)| decode_pack(&p, &d).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|img| {
            let level0 = img.mips.first()?.clone();
            let id = r.upload_texture(&TextureData::single(img.width, img.height, level0));
            Some((id, img.width as f32, img.height as f32, -img.place[0] as f32, -img.place[1] as f32))
        })
        .collect()
}

/// The five rows of the background, `(first entry, count, tile size, y)` — 0x4DDADC…0x4DDC39.
const ROWS: [(usize, usize, f32, f32); 5] = [(0, 20, 32.0, 0.0), (20, 10, 64.0, 32.0), (30, 5, 128.0, 96.0), (35, 5, 128.0, 224.0), (40, 5, 128.0, 352.0)];

impl LoadingScreen {
    /// The two packs: `era0.fsh` and, for a `LANGUAGE` other than 0, `loadtext.fsh`'s entry
    /// `language − 1`.
    pub fn load(r: &mut Renderer, paths: &GamePaths, language: u32) -> Self {
        let tiles = load_pack(r, paths, "era0.fsh");
        let caption = if language != 0 { load_pack(r, paths, "loadtext.fsh").get(language as usize - 1).copied() } else { None };
        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Self { tiles, caption, white }
    }

    /// **0x4DD600**'s picture — the start-up loading screen (`RE/notes/controls.md` §4.168):
    /// `"%sloadscrn.fsh"` over the art directory `[0x65B334]` (0x4DD731), its 45 entries as the
    /// same mosaic (0x4DD786…0x4DD9C5, `0x563440(entry, x, y, w, h)` with the sizes and rows of
    /// 0x4DDA00), no caption and no bar.
    pub fn startup(r: &mut Renderer, paths: &GamePaths) -> Self {
        let tiles = load_pack(r, paths, "loadscrn.fsh");
        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Self { tiles, caption: None, white }
    }

    /// The mosaic alone — 0x4DD600's frame, which it draws and flips six times
    /// (`[ebp−8]` = 6, 0x4DD77F…0x4DD9D9) so every buffer of the flip chain holds it.
    pub fn mosaic_sprites(&self, aspect: f32) -> Vec<Sprite> {
        let mut out = Vec::new();
        for (first, count, size, y) in ROWS {
            for k in 0..count {
                if let Some(a) = self.tiles.get(first + k) {
                    out.push(design_sprite(aspect, a.0, k as f32 * size, y, size, size, [1.0; 4], false));
                }
            }
        }
        out
    }

    /// The frame 0x4DDA00(`step`) draws, in the 640×480 design letterboxed into `aspect`.
    pub fn sprites(&self, aspect: f32, step: u32) -> Vec<Sprite> {
        let place = |x: f32, y: f32, w: f32, h: f32| {
            let design = DESIGN.0 / DESIGN.1;
            let (fw, x0) = if aspect > design { (design / aspect, (1.0 - design / aspect) * 0.5) } else { (1.0, 0.0) };
            ([x0 + x / DESIGN.0 * fw, y / DESIGN.1], [w / DESIGN.1, h / DESIGN.1])
        };
        let sprite = |tex: TextureId, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]| {
            let (pos, size) = place(x, y, w, h);
            Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None }
        };
        let mut out = Vec::new();
        for (first, count, size, y) in ROWS {
            for k in 0..count {
                if let Some(a) = self.tiles.get(first + k) {
                    out.push(sprite(a.0, k as f32 * size, y, size, size, [1.0; 4]));
                }
            }
        }
        if let Some(a) = self.caption {
            out.push(sprite(a.0, a.3, a.4, a.1, a.2, [1.0; 4]));
        }
        // `_ftol` truncates
        let w = (step as f32 * BAR_PER_STEP) as i32 as f32;
        out.push(sprite(self.white, 302.0, 467.0, w, 6.0, [1.0, 0xBB as f32 / 255.0, 0.0, 1.0]));
        out
    }
}

/// A 640×480 design rectangle as a sprite letterboxed into `aspect` — the same placement
/// [`LoadingScreen::sprites`] uses. `flip` mirrors the art left to right (0x563320).
#[allow(clippy::too_many_arguments)]
fn design_sprite(aspect: f32, tex: TextureId, x: f32, y: f32, w: f32, h: f32, color: [f32; 4], flip: bool) -> Sprite {
    let design = DESIGN.0 / DESIGN.1;
    let (fw, x0) = if aspect > design { (design / aspect, (1.0 - design / aspect) * 0.5) } else { (1.0, 0.0) };
    let (u0, u1) = if flip { (1.0, 0.0) } else { (0.0, 1.0) };
    Sprite {
        texture: tex,
        pos: [x0 + x / DESIGN.0 * fw, y / DESIGN.1],
        size: [w / DESIGN.1, h / DESIGN.1],
        uv_min: [u0, 0.0],
        uv_max: [u1, 1.0],
        color,
        rotation: 0.0,
        pivot: [0.5, 0.5],
        tri: None,
    }
}

/// One pack entry with its header's signed `misc[0]`, `misc[1]` as they are (`TextureImage::place`).
type Entry = (TextureId, f32, f32, i32, i32);

fn load_entries(r: &mut Renderer, dir: &std::path::Path, name: &str) -> Vec<Entry> {
    GamePaths::find_file(dir, name)
        .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
        .and_then(|(p, d)| decode_pack(&p, &d).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|img| {
            let level0 = img.mips.first()?.clone();
            let id = r.upload_texture(&TextureData::single(img.width, img.height, level0));
            Some((id, img.width as f32, img.height as f32, img.place[0], img.place[1]))
        })
        .collect()
}

/// What 0x4A4A70 reads besides its step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RaceLoadingInputs {
    /// `Game.GameType` `[0x6573E8]`
    pub game_type: u8,
    /// `0x5032D0()`: 1 Classic (0x209), 2 Golden (0x20A), 3 Modern (0x20B), else 0 — used only
    /// for a type-4 race (0x4A4C0F…0x4A4C36)
    pub era: u32,
    /// the track's name `[0x657444]`, for `FEData/TrackArt/<name>.fsh`
    pub track: String,
    /// `MIRROR` `[0x65742C]` == 1
    pub mirror: bool,
    /// `REVERSE` `[0x657430]` == 1
    pub reverse: bool,
    /// the four session tests of 0x4A4F5F (linked, not ejected, `+0xBE`, an id)
    pub network: bool,
    /// `LANGUAGE` `[0x657C84]`
    pub language: u32,
}

/// One blit into the loading surface: texture, x, y, w, h, mirrored.
type Blit = (TextureId, f32, f32, f32, f32, bool);

/// **0x4A4A70(`step`)** — the loading screen on the way **into** a race (`RE/notes/controls.md`
/// §4.167). It draws into a 640×480 surface `[0x655A24]` that lives across the calls, so the
/// port keeps what was drawn (`surface`) and the bar on top of it.
///
/// ```text
/// step 0             [0x655A28] = 1 (the screen is up); step 10: = 0 and return; any other
///                    step while it is 0 returns (0x4A4A7E…0x4A4AA6)
/// NOLOADINGSCREEN    [0x657440] == 1 → nothing (0x4A4B57)
/// step 0, or 5 when GameType != 9 (0x4A4BA6…0x4A4BBC):
///   step 5: FEData/TrackArt/<track>.fsh (0x4A4BD0, "%s%s.fsh" over [0x65B350], [0x657444])
///   era = type 4 ? 1/2/3 by 0x5032D0 : 0; "%sera%d.fsh" (step 0) / "%sera%dtr.fsh" (step 5)
///   the pack's 45 entries as 0x4DDA00's mosaic (0x4A4C7C…0x4A4DC6)
///   step 5 with the track pack: entry 0 at (|misc0|, |misc1|), mirrored under MIRROR
///   (0x563320 before and after); entry 1 at its own; entry 3 (REVERSE) or 2 at its own —
///   under MIRROR mirrored and at x = 2·x1 + w1 − x (0x4A4EDC…0x4A4F17), about entry 1's middle
/// network, step 0 or 5: MLoading.fsh entries k, k+1, k+2 (k = step ≥ 5 ? 3 : 0) at
///   (0, 322) 256×158, (256, 322) 256×158, (512, 322) 128×158 (0x4A4FB5…0x4A5041)
/// step 0 or 5, LANGUAGE != 0: loadtext.fsh entry LANGUAGE−1 at (−misc0, −misc1) (0x4A5071)
/// step ≥ 0: the bar 0x562680(302, 467, ftol(step · 12.4), 6, 0xFFFFBB00) (0x4A5118)
/// then the surface is copied to the middle of the screen and flipped (0x4A5371…0x4A53AF)
/// ```
///
/// Steps: 0 at 0x4B6CCA (main, as the front end hands over), 1 and 2 at 0x4B67D8 / 0x4B67DF, 3
/// inside the track load (0x469065), 4 and 5 after it (0x4B6827 / 0x4B682E), 6 at 0x4B683F, 7
/// at 0x4B6855, 8 and 9 in 0x414DB0 (0x414E63 / 0x414E74), 10 at 0x4B68CB. The network's
/// waits call it with −1 (0x48CE27 …): no bar, the surface as it was.
pub struct RaceLoading {
    white: TextureId,
    packs: std::collections::HashMap<String, Vec<Entry>>,
    /// `[0x655A28]`
    pub active: bool,
    surface: Vec<Blit>,
    /// the bar's width, drawn last
    bar: Option<f32>,
}

/// `[0x5B4AC0]`, 0x4A4A70's pixels per step.
pub const RACE_BAR_PER_STEP: f32 = 12.4;

impl RaceLoading {
    pub fn new(r: &mut Renderer) -> Self {
        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Self { white, packs: Default::default(), active: false, surface: Vec::new(), bar: None }
    }

    fn pack(&mut self, r: &mut Renderer, dir: &std::path::Path, name: &str) -> Vec<Entry> {
        let key = format!("{}/{}", dir.display(), name.to_ascii_lowercase());
        self.packs.entry(key).or_insert_with(|| load_entries(r, dir, name)).clone()
    }

    /// One call of 0x4A4A70; returns whether a frame is to be shown.
    pub fn step(&mut self, r: &mut Renderer, paths: &GamePaths, step: i32, input: &RaceLoadingInputs) -> bool {
        if step == 0 {
            self.active = true;
        } else if step == 10 {
            self.active = false;
            return false;
        }
        if !self.active {
            return false;
        }
        let art = paths.fe_data().join("Art");
        if step == 0 || (step == 5 && input.game_type != 9) {
            self.surface.clear();
            let track = if step == 5 { self.pack(r, &paths.fe_data().join("TrackArt"), &format!("{}.fsh", input.track)) } else { Vec::new() };
            let era = if input.game_type == 4 { input.era } else { 0 };
            let name = if step == 0 { format!("era{era}.fsh") } else { format!("era{era}tr.fsh") };
            let tiles = self.pack(r, &art, &name);
            for (first, count, size, y) in ROWS {
                for k in 0..count {
                    if let Some(e) = tiles.get(first + k) {
                        self.surface.push((e.0, k as f32 * size, y, size, size, false));
                    }
                }
            }
            if step == 5 && track.len() >= 2 {
                let at = |e: &Entry| (e.3.abs() as f32, e.4.abs() as f32);
                let (p, l) = (track[0], track[1]);
                let (px, py) = at(&p);
                self.surface.push((p.0, px, py, p.1, p.2, input.mirror));
                let (lx, ly) = at(&l);
                self.surface.push((l.0, lx, ly, l.1, l.2, false));
                if let Some(f) = track.get(if input.reverse { 3 } else { 2 }) {
                    let (fx, fy) = at(f);
                    let x = if input.mirror { 2.0 * lx + l.1 - fx } else { fx };
                    self.surface.push((f.0, x, fy, f.1, f.2, input.mirror));
                }
            }
        }
        if input.network && (step == 0 || step == 5) {
            let m = self.pack(r, &art, "MLoading.fsh");
            let k = if step >= 5 { 3 } else { 0 };
            for (i, (x, w)) in [(0.0, 256.0), (256.0, 256.0), (512.0, 128.0)].into_iter().enumerate() {
                if let Some(e) = m.get(k + i) {
                    self.surface.push((e.0, x, 322.0, w, 158.0, false));
                }
            }
        }
        if (step == 0 || step == 5) && input.language != 0 {
            let t = self.pack(r, &art, "loadtext.fsh");
            if let Some(e) = t.get(input.language as usize - 1) {
                self.surface.push((e.0, -e.3 as f32, -e.4 as f32, e.1, e.2, false));
            }
        }
        if step >= 0 {
            // `_ftol` truncates
            self.bar = Some((step as f32 * RACE_BAR_PER_STEP) as i32 as f32);
        }
        true
    }

    /// The surface and the bar as sprites, in the 640×480 design letterboxed into `aspect` —
    /// APPROXIMATION: the original copies the surface unscaled to the middle of the screen
    /// (0x53D570 at ((W − 640) / 2, (H − 480) / 2)); the port scales it as it does the front end.
    pub fn sprites(&self, aspect: f32) -> Vec<Sprite> {
        let mut out: Vec<Sprite> = self.surface.iter().map(|&(t, x, y, w, h, f)| design_sprite(aspect, t, x, y, w, h, [1.0; 4], f)).collect();
        if let Some(w) = self.bar {
            out.push(design_sprite(aspect, self.white, 302.0, 467.0, w, 6.0, [1.0, 0xBB as f32 / 255.0, 0.0, 1.0], false));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_tile_the_design_exactly() {
        let n: usize = ROWS.iter().map(|r| r.1).sum();
        assert_eq!(n, 45, "era0.fsh has 45 entries");
        for (_, count, size, _) in ROWS {
            assert_eq!(count as f32 * size, 640.0);
        }
        let bottom = ROWS.last().map(|r| r.3 + r.2).unwrap();
        assert_eq!(bottom, 480.0);
        assert_eq!((LAST_STEP as f32 * BAR_PER_STEP) as i32, 124);
    }
}
