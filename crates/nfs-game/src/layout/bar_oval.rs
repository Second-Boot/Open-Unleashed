//! The oval flash `FEBottomBarMenu` paints under its own parts — the object at `[bar+0x278]`,
//! built by the bar's constructor 0x4D7E70 (0x4D7EC7 → **0x520AD0**) and drawn by the bar's
//! slot 15 **0x4D83B0** through **0x520B80** before its base draw 0x528760 (`RE/notes/controls.md`,
//! "The bottom bar's oval flash").
//!
//! * **The art** (0x520AD0): `[0x65B334]` + `%sovalhilt.fsh` (0x5DD7E8, 0x4D6D20) and its entry
//!   `Oval` (0x5DD7E0, 0x4D7B20) — one quarter of an oval, 231×195 with its place at (89, 79), so
//!   its bottom-right corner is the screen's (320, 274).
//! * **The timer** (`+4` on, `+8` stamp, `+0xC` duration, in `TIMER_getTicks` 0x53C2E0 ticks):
//!   built off with the stamp now and a wait of `rand % 256 + 256` (0x520ADF…0x520B06). Off and
//!   `now > stamp + wait` (0x520BB5): on, stamp now, duration `rand % 32 + 96`, and **nothing drawn
//!   that call** (0x520BDD `ret`). On and `now > stamp + duration` (0x520BF5): off, stamp now, wait
//!   `rand % 256 + 256`.
//! * **The draw** while on, `t = now − stamp`, `d` the duration: the quarter at
//!   `W = w + 3·w·t / d`, `H = h + 3·h·t / d` (`imul`, `idiv` — truncating), four times, mirrored
//!   about (320, 273): flip 0 with its bottom-right at the art's own `(x0 + w, y0 + h)`, flip 1
//!   from x 320.0 (0x520CFD), flip 2 from y 273.0 (0x520D24), flip 3 from (320.0, 273.0) — in
//!   white at alpha `level · 0x37 / 255` (0x4D87D0(level) through 0x4D87F0(c, 0x37)), `level`
//!   the bar's `255 − 0x4E0D90(1)·255 / 512` (0x4D83B2…0x4D83D0).
//!
//! Both functions return at once on a low-end card (0x4E35C0), which this port never is, and the
//! draw also behind the glint gate 0x4E35E0 (`[0x6197AA]`), which the port draws past as it does
//! for every glint (`controls.md` §4.60).

use super::career_tabs::fade;
use super::{glints::crt_rand, tint, LayoutScreen, TRANS_FULL};
use nfs_assets::{texture::decode_pack, GamePaths};
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};
use std::cell::Cell;

const PACK: &str = "ovalhilt.fsh";
const TAG: &str = "Oval";
/// the mirror lines, 320.0 (0x43A00000, 0x520CFD / 0x520D4F) and 273.0 (0x43888000, 0x520D24 /
/// 0x520D4A)
const MID_X: i32 = 320;
const MID_Y: i32 = 273;
/// 0x520C3B: the fade the flash is drawn at, out of 255
const ALPHA: u32 = 0x37;

pub(super) struct BarOval {
    /// texture, width, height, `+0x1C`, `+0x20` (the pack's `misc` pair negated)
    art: Option<(TextureId, i32, i32, i32, i32)>,
    on: Cell<bool>,
    stamp: Cell<i64>,
    dur: Cell<i64>,
    /// whether the preloaded ticks (`NFS_FE_CLOCK`) have been run
    seen: Cell<bool>,
}

impl BarOval {
    /// `rand % 256 + 256` (0x520AEA…0x520AFD, 0x520C05…0x520C18): `rand` is never negative, so the
    /// signed remainder idiom is the plain one.
    fn wait() -> i64 {
        (crt_rand() % 256 + 256) as i64
    }

    /// One call of 0x520B80's timer at tick `now`: whether (and how far into its run) to draw.
    fn step(&self, now: i64) -> Option<i64> {
        if !self.on.get() {
            if now > self.stamp.get() + self.dur.get() {
                self.on.set(true);
                self.stamp.set(now);
                self.dur.set((crt_rand() % 32 + 96) as i64);
            }
            return None;
        }
        if now > self.stamp.get() + self.dur.get() {
            self.on.set(false);
            self.stamp.set(now);
            self.dur.set(Self::wait());
            return None;
        }
        Some(now - self.stamp.get())
    }
}

impl LayoutScreen {
    /// The bar's constructor's half, 0x520AD0: the art, and the timer off with its first wait.
    pub fn load_bar_oval(&mut self, r: &mut Renderer, paths: &GamePaths) {
        let dir = paths.fe_data().join("Art");
        let art = GamePaths::find_file(&dir, PACK)
            .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
            .and_then(|(p, d)| decode_pack(&p, &d).ok())
            .and_then(|images| {
                let img = images.into_iter().find(|i| i.name == TAG)?;
                let level0 = img.mips.first()?.clone();
                let id = r.upload_texture(&TextureData::single(img.width, img.height, level0));
                Some((id, img.width as i32, img.height as i32, -img.place[0], -img.place[1]))
            });
        // built at the front end's start (0x4D39D3): a preloaded clock (`NFS_FE_CLOCK`) stands
        // for that many ticks already run, so the stamp is the clock's start. *(Until this pass
        // the port loaded the bar after the first screen, whose glint activates
        // (`activate_glints`) had already drawn theirs, putting this `rand` — and the bottom
        // bar's own four button sweeps — one screen's worth of draws late against the original's
        // 0x4D3420, which builds the bar (0x4D7E70 at 0x4D39C4) before the first layout
        // (0x4D67D0…, `layouts.md`'s "The loading steps"). Fixed in `main.rs`'s `open_layout` and
        // `--layout` start-up path, which now build the bar first.)*
        let now = self.ticks() as i64 - self.preload_ticks;
        self.bar_oval = Some(BarOval { art, on: Cell::new(false), stamp: Cell::new(now), dur: Cell::new(BarOval::wait()), seen: Cell::new(false) });
    }

    /// The bar's slot 15 first half, **0x520B80** — called before the bar's own parts are drawn.
    pub fn bar_oval_sprites(&self, aspect: f32) -> Vec<Sprite> {
        let mut out = Vec::new();
        let Some(o) = self.bar_oval.as_ref() else { return out };
        let now = self.ticks() as i64;
        // a preloaded clock has no frames behind it: run the ticks it skipped, as the glints do
        if !o.seen.get() {
            o.seen.set(true);
            for t in 1..self.preload_ticks.min(now) {
                o.step(t);
            }
        }
        let Some(t) = o.step(now) else { return out };
        let Some((tex, w, h, x0, y0)) = o.art else { return out };
        let d = o.dur.get().max(1);
        let big_w = (w as i64 * t * 3 / d) as i32 + w;
        let big_h = (h as i64 * t * 3 / d) as i32 + h;
        let level = (255 - self.widget_progress() * 255 / TRANS_FULL).clamp(0, 255) as u32;
        let c = fade(level << 24 | 0xFF_FFFF, ALPHA);
        let color = tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]);
        let (left, top) = (x0 + w - big_w, y0 + h - big_h);
        for (x, y, flip) in [(left, top, 0), (MID_X, top, 1), (left, MID_Y, 2), (MID_X, MID_Y, 3)] {
            let (pos, size) = self.place(aspect, x as f32, y as f32, big_w as f32, big_h as f32);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::glints::crt_srand;

    #[test]
    fn the_timer_is_the_originals() {
        // seed 1: the CRT's first draws are 41, 18467, 6334 — wait 41 % 256 + 256 = 297,
        // duration 18467 % 32 + 96 = 99, then the next wait 6334 % 256 + 256 = 446
        crt_srand(1);
        let o = BarOval { art: None, on: Cell::new(false), stamp: Cell::new(0), dur: Cell::new(BarOval::wait()), seen: Cell::new(true) };
        assert_eq!(o.dur.get(), 297);
        assert_eq!(o.step(297), None);
        // switched on at 298 but not drawn that call
        assert_eq!(o.step(298), None);
        assert_eq!(o.dur.get(), 99);
        assert_eq!(o.step(300), Some(2));
        assert_eq!(o.step(397), Some(99));
        assert_eq!(o.step(398), None);
        assert_eq!(o.dur.get(), 446);
        assert!(!o.on.get());
    }
}
