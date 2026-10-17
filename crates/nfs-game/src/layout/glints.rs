//! The front end's glints, each on **its own timer and its own random waits** — the state the
//! original keeps per widget (`RE/notes/controls.md` §4.25–§4.29, §4.53), drawn from the C
//! runtime's `rand` (0x5A17C9) through [`crt_rand`].
//!
//! | kind | fields | activate | a run starts | a run ends |
//! |---|---|---|---|---|
//! | `NFS5.HEADER` sweep | `+0x1DE/+0x1E0/+0x1E4` | 0x4CADFB: stamp = now, wait = `rand % 128 + 128` | `now > stamp + wait` (0x4CB048) | `now − stamp >= 640` (0x4CB084): wait = `rand % 128 + 128` (0x4CB098) |
//! | `NFS5.CarLogo` sweep | `+0x212/+0x214/+0x218` | 0x4C13F7: wait = `rand % 128` (0x4C1409) | the same (0x4C1D04) | the same (0x4C1D40), wait = `rand % 128` (0x4C1D54) — **no** `+ 128` |
//! | the glinty box / `NFS5.GlintyRectangle` flourish | `+0x10…+0x1C` of 0x51FDA0's object | 0x51FDA0: stamp = now, duration = 0, no draw | `now − stamp > duration` and the caller's *may start* (0x51FE53, 0x51FE5D): duration = `rand % 32 + 64`, variant = `animate` or, for 5, `rand % 4 + 1` (0x51FE6C, 0x51FE8E) | `now − stamp > duration` (0x51FEB9): wait = `rand % 256 + 256` (0x51FEC2) |
//! | `NFS5.GLINTYLINE` sweep | `+0x24…+0x3C` of 0x51F9A0's object | 0x51F9A0: the bar's position `rand % (hi − lo) + lo`, its step `rand % 5 + 5`, its direction a coin, then stamp = now, wait = `rand % 128` (0x51FA2D…0x51FA7E) | `now > stamp + wait` (0x51FB24): speed = `rand % 3 + 3`, direction a coin (0x51FB4D, 0x51FB5F) | one width past the far end (0x51FBE1…0x51FC02): wait = `rand % 128` (0x51FC10) |
//! | `NFS5.AnimatingPicture` | `+0x14C…+0x160` | 0x4BD8BE: stamp = now, wait = `min + rand % (max − min)` only when `max > min` | `now > stamp + wait` (0x4BD917) | `now > stamp + numticks` (0x4BD960): with `max > 0` a new wait (0x4BD981), else it plays again |
//!
//! The header's timer only moves while the screen's fade is 0 (0x4CB01C tests 0x4C8820's answer);
//! the car logo's, the line's and the picture's always do. (history: `game-history.md` §29.1)

use std::cell::Cell;

thread_local! {
    /// The C runtime's `holdrand` — per thread in the original's CRT (0x5A545C's block, `+0x14`),
    /// and **1** until the first `srand` (`_initptd`). The one `srand` in the image is
    /// `srand(time(0))` at 0x418100, in `H321Go`'s constructor (0x417A60), which the HUD builds in
    /// every race — so on the first entry to the front end the sequence starts at 1.
    static HOLDRAND: Cell<u32> = const { Cell::new(1) };
}

/// `rand()`, 0x5A17C9: `s = s·0x343FD + 0x269EC3`, `(s >> 16) & 0x7FFF`, on the shared state.
pub fn crt_rand() -> i32 {
    HOLDRAND.with(|h| {
        let s = crate::rng::crt_step(h.get());
        h.set(s);
        crate::rng::crt_out(s)
    })
}

/// `srand(seed)`, 0x5A17BC.
pub fn crt_srand(seed: u32) {
    HOLDRAND.with(|h| h.set(seed));
}

/// Hands the shared state to a caller that draws through a [`crate::rng::CRand`] (0x4F4FA0's
/// opponent rolls, 0x4E3160's driver draw) and keeps what it left: one stream, as in the original.
pub fn with_crt<R>(f: impl FnOnce(&mut crate::rng::CRand) -> R) -> R {
    HOLDRAND.with(|h| {
        let mut c = crate::rng::CRand::new(h.get());
        let r = f(&mut c);
        h.set(c.state());
        r
    })
}

/// The header's and the car logo's sweep (0x4CB024…0x4CB0F6, 0x4C1CE0…0x4C1DAF).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sweep {
    pub running: bool,
    pub stamp: i64,
    pub wait: i64,
    /// the car logo draws its wait without the header's `+ 128`
    pub logo: bool,
}

/// `ftol([0x6907E0])` — 640.0f, set by 0x4D35B4 (0x4E3550).
pub const SWEEP_TRAVEL: i64 = 640;

impl Sweep {
    fn draw_wait(logo: bool) -> i64 {
        (crt_rand() % 128) as i64 + if logo { 0 } else { 128 }
    }

    /// The activate: 0x4CADFB…0x4CAE24 for the header, 0x4C13F7…0x4C141A for the logo.
    pub fn new(now: i64, logo: bool) -> Self {
        Sweep { running: false, stamp: now, wait: Self::draw_wait(logo), logo }
    }

    /// One draw's timing: the elapsed tick count to draw the sprite at, or `None`.
    pub fn step(&mut self, now: i64) -> Option<i64> {
        if !self.running && now > self.stamp + self.wait {
            self.running = true;
            self.stamp = now;
        }
        if !self.running {
            return None;
        }
        let e = now - self.stamp;
        if e >= SWEEP_TRAVEL {
            self.running = false;
            self.stamp = now;
            self.wait = Self::draw_wait(self.logo);
            return None;
        }
        Some(e)
    }
}

/// The flourish object 0x51FDA0 builds and 0x51FDE0 draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flourish {
    pub running: bool,
    pub stamp: i64,
    /// `[+0x18]`: the run's length while running, the wait while not
    pub duration: i64,
    pub variant: i32,
    /// the buttons' own latch — `[+0x160]` on `NFS5.GOTOBUTTON`, `[+0x158]` on the tab buttons,
    /// `[+0x214]` on the combo family — 0 when the object is built (0x4C9082, 0x4D01A7, 0x4C5989)
    pub latch: bool,
}

impl Flourish {
    /// 0x51FDA0: not running, stamped now, no wait.
    pub fn new(now: i64) -> Self {
        Flourish { running: false, stamp: now, duration: 0, variant: 0, latch: false }
    }

    /// The buttons' *may start* (0x4C93DC…0x4C9450 for `NFS5.GOTOBUTTON`; the tab buttons'
    /// 0x4D04C0…0x4D0534 and the combo family's 0x4C6415…0x4C648A are the same code on their own
    /// latch): a disabled widget latches; until it has latched it may start once the fade is 0,
    /// which latches it; after that only while it is hovered, selected or focused — and never
    /// while disabled.
    pub fn button_may_start(&mut self, disabled: bool, hovered: bool, selected: bool, focused: bool, fade_zero: bool) -> bool {
        if disabled {
            self.latch = true;
        }
        if self.latch {
            if !(hovered || selected || focused) {
                return false;
            }
        } else if fade_zero {
            self.latch = true;
        } else {
            return false;
        }
        !disabled
    }

    /// One draw's timing (0x51FE39…0x51FEE2): `(elapsed, duration, variant)` while a run is on.
    pub fn step(&mut self, now: i64, may_start: bool, animate: i32) -> Option<(i64, i64, i32)> {
        if animate == 0 {
            return None;
        }
        if !self.running {
            if now - self.stamp > self.duration && may_start {
                self.running = true;
                self.stamp = now;
                self.duration = (crt_rand() % 32) as i64 + 64;
                self.variant = if animate == 5 { crt_rand() % 4 + 1 } else { animate };
            }
            // the frame a run starts on draws nothing (0x51FEA3 returns)
            return None;
        }
        if now - self.stamp > self.duration {
            self.running = false;
            self.stamp = now;
            self.duration = (crt_rand() % 256) as i64 + 256;
            return None;
        }
        Some((now - self.stamp, self.duration, self.variant))
    }
}

/// `NFS5.GLINTYLINE`'s object, 0x51F9A0 — the sweep half; the bar half's draws are made and kept
/// (`bar_pos`, `bar_step`, `bar_up`) but the bar is drawn as the whole strip (§4.28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub bar_pos: i32,
    pub bar_step: i32,
    pub bar_up: bool,
    pub running: bool,
    pub stamp: i64,
    pub wait: i64,
    /// `[+0x28]`, relative to the start of the long axis
    pub pos: i64,
    pub width: i64,
    pub speed: i64,
    pub dir: i64,
    /// `[+0x44] − [+0x40]`, the long axis
    pub len: i64,
}

impl Line {
    /// 0x51F9A0: the bar's range `short` (one pixel on every shipped line) and the sweep's `len`.
    pub fn new(now: i64, short: i32, len: i64) -> Self {
        // `idiv` by the range (0x51FA38); a zero range would fault in the original and ships nowhere
        let r = crt_rand();
        let bar_pos = if short != 0 { r % short } else { 0 };
        let bar_step = crt_rand() % 5 + 5;
        let bar_up = crt_rand() % 2 == 1;
        let wait = (crt_rand() % 128) as i64;
        Line { bar_pos, bar_step, bar_up, running: false, stamp: now, wait, pos: 0, width: 0, speed: 0, dir: 1, len }
    }

    /// One draw's timing (0x51FB0A…0x51FC24): the sweep's leading position while it runs.
    pub fn step(&mut self, now: i64) -> Option<i64> {
        if !self.running && now > self.stamp + self.wait {
            self.running = true;
            self.stamp = now;
            // 0x66666667 with the sign fix-up: a C `/ 5`
            self.width = self.len / 5;
            self.speed = (crt_rand() % 3) as i64 + 3;
            if crt_rand() % 2 == 1 {
                self.dir = 1;
                self.pos = -self.width;
            } else {
                self.dir = -1;
                self.pos = self.len + self.width;
            }
        }
        if !self.running {
            return None;
        }
        self.pos += self.speed * (now - self.stamp) * self.dir;
        self.stamp = now;
        if (self.dir == 1 && self.pos > self.len + self.width) || (self.dir == -1 && self.pos < -self.width) {
            self.running = false;
            self.stamp = now;
            self.wait = (crt_rand() % 128) as i64;
            return None;
        }
        Some(self.pos)
    }
}

/// `NFS5.AnimatingPicture`'s clock (bind 0x4BD8BE, tick 0x4BD8F0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anim {
    pub playing: bool,
    pub stamp: i64,
    pub wait: i64,
}

impl Anim {
    fn draw_wait(min: i64, max: i64) -> i64 {
        min + if max > min { (crt_rand() as i64) % (max - min) } else { 0 }
    }

    pub fn new(now: i64, min: i64, max: i64) -> Self {
        Anim { playing: false, stamp: now, wait: Self::draw_wait(min, max) }
    }

    /// One tick: the frame to show.
    pub fn step(&mut self, now: i64, start: i64, end: i64, numticks: i64, min: i64, max: i64) -> i64 {
        if !self.playing {
            if now > self.stamp + self.wait {
                self.playing = true;
                self.stamp = now;
            }
            if !self.playing {
                return start;
            }
        }
        if now > self.stamp + numticks {
            if max > 0 {
                self.playing = false;
                self.wait = Self::draw_wait(min, max);
                self.stamp = now;
                return start;
            }
            self.stamp = now;
        }
        let v = start + (now - self.stamp) * (end - start) / numticks;
        v.max(start).min(end)
    }
}

/// One widget's glint state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Glint {
    Sweep(Sweep),
    Flourish(Flourish),
    Line(Line),
    Anim(Anim),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crt_rand_starts_from_one() {
        crt_srand(1);
        // MSVC's first three from the default seed
        assert_eq!([crt_rand(), crt_rand(), crt_rand()], [41, 18467, 6334]);
    }

    #[test]
    fn a_crand_borrowed_through_with_crt_continues_and_leaves_the_shared_state() {
        crt_srand(1);
        let first = with_crt(|c| [c.next(), c.next()]);
        assert_eq!(first, [41, 18467]);
        assert_eq!(crt_rand(), 6334, "the next draw continues where the borrowed stream stopped");
    }

    #[test]
    fn a_sweep_waits_then_crosses_the_screen_and_draws_a_new_wait() {
        crt_srand(1);
        let mut s = Sweep::new(0, false);
        assert_eq!(s.wait, 41 + 128);
        assert_eq!(s.step(169), None, "not more than stamp + wait");
        assert_eq!(s.step(172), Some(0), "starts on the draw that finds it passed");
        assert_eq!(s.step(172 + 639), Some(639));
        assert_eq!(s.step(172 + 640), None);
        assert_eq!((s.running, s.stamp, s.wait), (false, 812, (18467 % 128) as i64 + 128));
        crt_srand(1);
        assert_eq!(Sweep::new(0, true).wait, 41, "the logo's has no + 128");
    }

    #[test]
    fn a_flourish_starts_the_draw_after_its_build_and_redraws_its_variant() {
        crt_srand(1);
        let mut f = Flourish::new(10);
        assert_eq!(f.step(10, true, 5), None, "0 is not more than 0");
        assert_eq!(f.step(11, false, 5), None, "not allowed to start");
        assert_eq!(f.step(11, true, 5), None, "the start draws nothing");
        assert_eq!((f.duration, f.variant), ((41 % 32) as i64 + 64, 18467 % 4 + 1));
        assert_eq!(f.step(20, true, 5), Some((9, 73, 4)));
        assert_eq!(f.step(11 + 73, true, 5), Some((73, 73, 4)), "the end is strict");
        assert_eq!(f.step(11 + 74, true, 5), None);
        assert_eq!(f.duration, (6334 % 256) as i64 + 256);
    }

    #[test]
    fn the_buttons_latch() {
        let mut f = Flourish::new(0);
        assert!(!f.button_may_start(false, false, false, false, false), "in a transition");
        assert!(f.button_may_start(false, false, false, false, true), "the first frame at rest");
        assert!(!f.button_may_start(false, false, false, false, true), "then only when hot");
        assert!(f.button_may_start(false, true, false, false, true));
        assert!(!f.button_may_start(true, true, false, false, true), "never disabled");
    }

    #[test]
    fn a_line_sweeps_one_width_past_the_far_end() {
        crt_srand(1);
        let mut l = Line::new(0, 1, 100);
        assert_eq!((l.bar_pos, l.wait), (0, (26500 % 128) as i64));
        let mut t = l.wait + 1;
        let first = l.step(t);
        assert!(first == Some(-20) || first == Some(120));
        while l.step(t).is_some() {
            t += 1;
        }
        assert!(!l.running);
    }

    #[test]
    fn a_picture_with_no_delay_plays_again() {
        let mut a = Anim::new(0, 0, 0);
        assert_eq!(a.step(0, 0, 29, 256, 0, 0), 0);
        assert_eq!(a.step(1, 0, 29, 256, 0, 0), 0, "starts, at elapsed 0");
        assert_eq!(a.step(257, 0, 29, 256, 0, 0), 29);
        assert_eq!(a.step(258, 0, 29, 256, 0, 0), 0, "past numticks it restarts");
    }
}
