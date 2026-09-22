//! `FEEndOfEraCallBackList`, the end-of-era screen (`RE/notes/controls.md` §4.150 / §4.150a): a
//! timed show the controller paints itself — built once by its constructor **0x4FD4F0** and run
//! by its slot 15, **0x4FF1F0**, off the 128 Hz `TIMER_getTicks()`.
//!
//! The show is a list of entries, one on screen at a time, a step every 12 ticks; behind it the
//! old era's art gives way to the new era's, twelve corner sprites and eight rules blink, and
//! towards the end the era's movie fades in inside its oval.

use crate::opponents::FeRng;

/// `[0x691D84]`: `_ftol(0.1 × 128.0)` (0x4FD3B0) — the ticks one step lasts.
pub const STEP_TICKS: i64 = 12;
/// The blink's period (0x4FF308).
pub const BLINK_TICKS: i64 = 12;
/// The era-complete sting plays once, this long after the first frame (0x4FFA45).
pub const STING_TICKS: i64 = 0x80;

/// The years of the eras, 0x5049E0 / 0x504A10, by era index (Classic, Golden, Modern).
pub const ERA_FIRST: [i32; 3] = [1950, 1970, 1989];
pub const ERA_LAST: [i32; 3] = [1970, 1989, 2000];
/// `0x5B8670`: the year each shown era tournament gets on the evolution-complete show.
pub const EVOLUTION_YEARS: [i32; 17] = [1950, 1956, 1960, 1965, 1969, 1970, 1973, 1975, 1978, 1982, 1988, 1989, 1993, 1996, 1997, 1999, 2000];
/// The title's text id (0x4FE053).
pub const TITLE_TEXT: u32 = 0x46F4;

/// The art the constructor loads (`%s%s.fsh` from the front end's art directory).
pub const ERA_ART_1: [&str; 3] = ["C1.fsh", "G1.fsh", "M1.fsh"];
pub const ERA_ART_2: [&str; 3] = ["C2.fsh", "G2.fsh", "M2.fsh"];
pub const OVAL: &str = "MovieOval.fsh";
pub const NUMBERS: &str = "numbers.fsh";
pub const CURVES: &str = "curves.fsh";
pub const CURVES2: &str = "curves2.fsh";
pub const LINE: &str = "movieline.fsh";
pub const FONT: &str = "Univers20b.ffn";
/// `0x5DB62C`, by `0x504800()`: the era's movie.
pub const MOVIES: [&str; 3] = ["er2pc-av.mad", "er3pc-av.mad", "er4pc-av.mad"];
/// `0x5B8228`: how long each movie is shown, in ticks.
pub const MOVIE_TICKS: [i64; 3] = [2560, 2560, 3840];
/// `[0x5B8238]`, `[0x5B823C]`: the movie's size; it is drawn centred on (320, 252).
pub const MOVIE_SIZE: (i32, i32) = (0x1A0, 0x15C);
/// The fade at either end of the movie (0x4FF7CB).
const MOVIE_FADE: i64 = 0x280;
/// Every tint of the corner sprites and the rules (0x4FF689, 0x4FF70B).
pub const AMBER: u32 = 0xFFFF_B400;

/// `0x5B8268`: `{x, y, sprite, flip}` of the twelve corner pieces — sprite 0 `curves[0]`, 1
/// `curves2[0]`, 2 `curves2[1]` (`+0x2D4…+0x2DC`).
pub const CORNERS: [(i32, i32, usize, u8); 12] = [
    (113, 75, 0, 0),
    (168, 120, 1, 0),
    (213, 158, 2, 0),
    (320, 75, 0, 1),
    (320, 120, 1, 1),
    (320, 158, 2, 1),
    (113, 250, 0, 2),
    (168, 250, 1, 2),
    (213, 250, 2, 2),
    (320, 250, 0, 3),
    (320, 250, 1, 3),
    (320, 250, 2, 3),
];
/// `0x5B8388`: which corner pieces each blink phase draws.
pub const CORNER_ON: [[bool; 12]; 2] = [
    [false, true, false, true, false, true, true, false, true, false, true, false],
    [true, false, true, false, true, false, false, true, false, true, false, true],
];
/// `0x5B8418`: the rules `{x0, y0, x1, y1}`, drawn `max(x1 − x0, 1)` × `max(y1 − y0, 1)`.
pub const RULES: [(i32, i32, i32, i32); 8] = [
    (112, 250, 168, 250),
    (472, 250, 528, 250),
    (320, 120, 320, 382),
    (213, 250, 427, 250),
    (320, 75, 320, 120),
    (320, 380, 320, 427),
    (320, 158, 320, 343),
    (168, 250, 464, 250),
];
/// `0x5B8498`: which rules each blink phase draws.
pub const RULE_ON: [[bool; 8]; 2] = [[true, true, true, true, false, false, false, false], [false, false, false, false, true, true, true, true]];

/// One 0x2C-byte entry of `[+0x2A4]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// `+0` — 1 for a year (0x4FFDB0), 0 for text
    pub year_kind: bool,
    /// `+4` — the year a year entry draws with `numbers.fsh`, 0 otherwise
    pub year: i32,
    /// `+8`
    pub text: String,
    /// `+0x14`, zeroed once the step is past the entry (0x4FF29E)
    pub alpha: u8,
    /// `+0x18`, `+0x1C`
    pub x: i32,
    pub y: i32,
    /// `+0x20` — the font's horizontal scale (property 2, `+0xB4`, through 0x536CE0; the
    /// vertical one is the 1.0 the draw passes)
    pub scale: f32,
    /// `+0x24`
    pub colour: u32,
    /// `+0x28` — a black copy 4 px down and right first
    pub shadow: bool,
}

impl Entry {
    fn text(text: &str, x: i32, y: i32, alpha: u8, scale: f32, shadow: bool) -> Self {
        Self { year_kind: false, year: 0, text: text.to_string(), alpha, x, y, scale, colour: 0xFF_FFFF, shadow }
    }
}

/// What the constructor reads from the career.
#[derive(Debug, Clone)]
pub struct EraInput {
    /// `+0x300` — `0x5032E0()`, the era before the current one (Classic for Classic), 0…2
    pub previous: usize,
    /// `+0x304` — `0x5032D0()`, the current era `[0x691E50]`, 0…2
    pub current: usize,
    /// `0x504800()`, the highest completed era, 0…2 — the movie
    pub completed: usize,
    /// `0x503ED0(0x5032D0())`: every visible tournament of the current era completed — the
    /// evolution is over
    pub current_done: bool,
    /// the names of the three eras, `FEStrings[0x209…0x20B]`
    pub era_names: [String; 3],
    /// the catalogue `[0x6919E8]` in its order: each car's `+0x1F8` year and `+2` name
    pub cars: Vec<(i32, String)>,
    /// per era, the titles `FEStrings[rec+0x24]` of the era's tournaments with `+0xE4 == 0`, in
    /// the order of its list (0x503340)
    pub tournaments: [Vec<String>; 3],
    /// `FEStrings[0x46F4]`
    pub title: String,
    /// `(+0x29C, +0x2A0)`, half the screen's size (0x4FD5EF…0x4FD60D)
    pub centre: (i32, i32),
}

/// The built show.
#[derive(Debug, Clone)]
pub struct EndOfEra {
    pub entries: Vec<Entry>,
    /// `+0x2AC` — the entry count when the new era's first year came up (0 on the evolution show)
    pub mark: i32,
    pub previous: usize,
    pub current: usize,
    /// `+0x30C`, the movie's era
    pub movie: usize,
    /// `+0x288`, `+0x28C`, `+0x290` — seeded on the first frame
    step_at: i64,
    blink_at: i64,
    first: i64,
    /// `+0x294`, `+0x298`, `+0x2E5`, `+0x2E4`, `+0x2F8`
    pub step: usize,
    pub blink: usize,
    pub finished: bool,
    sting_done: bool,
    movie_at: Option<i64>,
    /// `+0x314` (ESC seen by slot 4) and `+0x308` (the screen left)
    escaped: bool,
    left: bool,
}

/// One primitive of a frame, in the order 0x4FF1F0 draws them.
#[derive(Debug, Clone, PartialEq)]
pub enum Draw {
    /// every entry of one era art pack at its own place, `alpha << 24 | 0xFFFFFF` (255 opaque)
    Tiles { pack: &'static str, alpha: u8 },
    /// a corner piece (0x4D9AF0), `sprite` 0…2 as [`CORNERS`], in [`AMBER`]
    Corner { sprite: usize, x: i32, y: i32, flip: u8 },
    /// a rule (0x4D9280) in [`AMBER`]
    Rule { x: i32, y: i32, w: i32, h: i32 },
    /// a `MovieOval.fsh` entry (0x4FFAA0), opaque white
    Oval { entry: usize, x: i32, y: i32, flip: u8 },
    /// the movie's current frame at `(x, y)`, `alpha << 24 | 0xFFFFFF`
    Movie { x: i32, y: i32, w: i32, h: i32, alpha: u8 },
    /// `movieline.fsh[0]` (0x4FFBF0)
    Line { x: i32, y: i32, flip: u8 },
    /// four `numbers.fsh` digits centred on `x` by their widths (0x4FFC90)
    Digits { digits: [usize; 4], x: i32, y: i32, alpha: u8 },
    /// text through 0x4DA350, centred (alignment 1), `argb` its colour, `scale` the horizontal
    Text { text: String, x: i32, y: i32, argb: u32, scale: f32 },
}

/// 0x4FFF80: a text pulses — scale 1, 1.5, 2, 2.5, 2, 1.5, 1 — and fades — 0xCD, 0xAF, 0x64,
/// 0x19 — one entry a step, at (320, 240).
fn pulse(out: &mut Vec<Entry>, text: &str) {
    for s in [1.0, 1.5, 2.0, 2.5, 2.0, 1.5, 1.0] {
        out.push(Entry::text(text, 320, 240, 0xFF, s, false));
    }
    for a in [0xCD, 0xAF, 0x64, 0x19] {
        out.push(Entry::text(text, 320, 240, a, 1.0, false));
    }
}

/// 0x4FFDB0: a year at a place the front end's generator picks — `x = rand % 0x11C + 0xB2`,
/// `y = rand % 0xE0 + 0x80`.
fn year(out: &mut Vec<Entry>, y: i32, rng: &mut FeRng) {
    let x = (rng.next_u32() % 0x11C + 0xB2) as i32;
    let yy = (rng.next_u32() % 0xE0 + 0x80) as i32;
    out.push(Entry { year_kind: true, year: y, text: y.to_string(), alpha: 0xFF, x, y: yy, scale: 1.0, colour: 0xFF_FFFF, shadow: false });
}

impl EndOfEra {
    /// The constructor's entries (0x4FD721…0x4FE4FC).
    pub fn build(input: &EraInput, rng: &mut FeRng) -> Self {
        let mut e = Vec::new();
        let (cx, cy) = input.centre;
        let mut mark = 0;
        if !input.current_done {
            let (from, to, new_first) = (ERA_FIRST[input.previous], ERA_LAST[input.current], ERA_FIRST[input.current]);
            for y in from..=to {
                if y == from {
                    pulse(&mut e, &input.era_names[input.previous]);
                }
                if y == new_first {
                    mark = e.len() as i32;
                    pulse(&mut e, &input.era_names[input.current]);
                }
                year(&mut e, y, rng);
                for (cy_, name) in &input.cars {
                    if *cy_ == y {
                        e.push(Entry::text(name, cx, cy, 0xFF, 1.0, false));
                    }
                }
            }
        } else {
            let mut k = 0;
            for era in 0..3 {
                pulse(&mut e, &input.era_names[era]);
                for t in &input.tournaments[era] {
                    e.push(Entry::text(t, cx, cy, 0xFF, 1.0, false));
                    let y = EVOLUTION_YEARS.get(k).copied().unwrap_or(0);
                    year(&mut e, y, rng);
                    k += 1;
                }
            }
        }
        // the title typed out, then pulsed, faded and blinked (0x4FE053…0x4FE4D0)
        let chars: Vec<char> = input.title.chars().collect();
        for n in 1..=chars.len() {
            let s: String = chars[..n].iter().collect();
            e.push(Entry::text(&s, 320, 240, 0xFF, 2.0, true));
        }
        for s in [1.8, 1.6, 1.4, 1.2, 1.0, 1.2, 1.4, 1.6, 1.8, 2.0] {
            e.push(Entry::text(&input.title, 320, 240, 0xFF, s, true));
        }
        for a in [200, 150, 100, 50, 25, 10] {
            e.push(Entry::text(&input.title, 320, 240, a, 2.0, true));
        }
        for _ in 0..10 {
            e.push(Entry::text(&input.title, 320, 240, 0xFF - 0x17, 2.0, true));
            e.push(Entry::text(&input.title, 320, 240, 0, 2.0, true));
        }
        Self {
            entries: e,
            mark,
            previous: input.previous,
            current: input.current,
            movie: input.completed,
            step_at: 0,
            blink_at: 0,
            first: 0,
            step: 0,
            blink: 0,
            finished: false,
            sting_done: false,
            movie_at: None,
            escaped: false,
            left: false,
        }
    }

    /// `+0x2A8`
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// When the movie started (0x4FF76F: once `step ≥ count − 10`), in ticks.
    pub fn movie_started(&self) -> Option<i64> {
        self.movie_at
    }

    /// The era-complete sting's argument, once (0x4FFA2F…0x4FFA84): 0 when the current era is
    /// Classic or Golden, else 1 when `0x5032E0()` equals `0x5032D0()` and 2 when it does not —
    /// as written: `neg; sbb; add 2` answers 1 for a non-zero difference.
    pub fn take_sting(&mut self, now: i64) -> Option<i32> {
        if self.sting_done || self.first == 0 || now - self.first <= STING_TICKS {
            return None;
        }
        self.sting_done = true;
        Some(if self.current == 2 { if self.previous != self.current { 1 } else { 2 } } else { 0 })
    }

    /// Slot 4, 0x500770: a key whose code is **0x1B** (ESC) is taken — `+0x314` set, answer 1 —
    /// and every other key is left to the screen (answer 0).
    pub fn key(&mut self, code: u32) -> bool {
        if code == 0x1B {
            self.escaped = true;
            return true;
        }
        false
    }

    /// Slot 1, 0x4FF180, after the base tick: once the show is over (`+0x2E5`) or ESC was
    /// pressed, and not yet done (`+0x308`), the screen goes back — 0x4D23F0(0), one history
    /// entry — at once for ESC, else when the movie has run its `0x5B8228` ticks since it started.
    pub fn tick(&mut self, now: i64) -> bool {
        if !(self.finished || self.escaped) || self.left {
            return false;
        }
        let e = now - self.movie_at.unwrap_or(0);
        if !self.escaped && e <= MOVIE_TICKS[self.movie.min(2)] {
            return false;
        }
        self.left = true;
        true
    }

    /// One call of slot 15 at `now` (128 Hz ticks, never 0 — the stamps treat 0 as unset).
    pub fn frame(&mut self, now: i64) -> Vec<Draw> {
        let mut out = Vec::new();
        let count = self.count();
        if self.finished {
            out.push(Draw::Tiles { pack: ERA_ART_2[self.current], alpha: 0xFF });
        } else {
            if self.first == 0 {
                self.first = now;
            }
            if self.step_at == 0 {
                self.step_at = now;
            }
            if self.blink_at == 0 {
                self.blink_at = now;
            }
            for k in 0..self.step.min(count) {
                self.entries[k].alpha = 0;
            }
            if now - self.step_at >= STEP_TICKS {
                self.step += 1;
                if self.step >= count {
                    self.step = 0;
                    self.finished = true;
                }
                self.step_at = now;
            }
            if now - self.blink_at >= BLINK_TICKS {
                self.blink = (self.blink + 1) % 2;
                self.blink_at = now;
            }
            self.background(&mut out);
            for (k, &(x, y, sprite, flip)) in CORNERS.iter().enumerate() {
                if CORNER_ON[self.blink][k] {
                    out.push(Draw::Corner { sprite, x, y, flip });
                }
            }
            for (k, &(x0, y0, x1, y1)) in RULES.iter().enumerate() {
                if RULE_ON[self.blink][k] {
                    out.push(Draw::Rule { x: x0, y: y0, w: (x1 - x0).max(1), h: (y1 - y0).max(1) });
                }
            }
        }
        // the movie, from ten entries before the end (0x4FF744…0x4FF859)
        if self.step as i64 >= count as i64 - 10 || self.finished {
            let start = *self.movie_at.get_or_insert(now);
            for (entry, pts, flips) in [
                (4usize, [(192, 136), (320, 136), (192, 252), (320, 252)], [0u8; 4]),
                (0, [(160, 72), (320, 72), (160, 368), (320, 368)], [0, 1, 2, 3]),
                (1, [(113, 136), (448, 136), (113, 252), (448, 252)], [0, 1, 2, 3]),
            ] {
                for k in 0..4 {
                    out.push(Draw::Oval { entry, x: pts[k].0, y: pts[k].1, flip: flips[k] });
                }
            }
            let e = now - start;
            let len = MOVIE_TICKS[self.movie.min(2)];
            let alpha = if e <= MOVIE_FADE {
                e * 255 / MOVIE_FADE
            } else if e <= len - MOVIE_FADE {
                255
            } else if e > len {
                0
            } else {
                (len - e - MOVIE_FADE) * 255 / MOVIE_FADE + 255
            };
            let (w, h) = MOVIE_SIZE;
            out.push(Draw::Movie { x: 320 - w / 2, y: 252 - h / 2, w, h, alpha: alpha.clamp(0, 255) as u8 });
            for (k, (x, y)) in [(112, 76), (320, 76), (112, 252), (320, 252)].into_iter().enumerate() {
                out.push(Draw::Line { x, y, flip: k as u8 });
            }
        }
        // the entry the step is on (0x4FF85E…0x4FFA2A)
        if let Some(en) = self.entries.get(self.step)
            && en.alpha > 0
        {
            let a = en.alpha as u32;
            if en.year_kind && en.year != 0 {
                let d = if en.year == 2000 { [2, 0, 0, 0] } else { let r = en.year - 1900; [1, 9, (r / 10) as usize, (r % 10) as usize] };
                out.push(Draw::Digits { digits: d, x: en.x, y: en.y, alpha: en.alpha });
            } else {
                if !en.year_kind && en.shadow {
                    out.push(Draw::Text { text: en.text.clone(), x: en.x + 4, y: en.y + 4, argb: a << 24, scale: en.scale });
                }
                out.push(Draw::Text { text: en.text.clone(), x: en.x, y: en.y, argb: a << 24 | en.colour, scale: en.scale });
            }
        }
        out
    }

    /// 0x4FF32D…0x4FF660: the era art behind the entries.
    fn background(&self, out: &mut Vec<Draw>) {
        let t = self.step as i32;
        let big_t = self.mark as f32;
        let tf = t as f32;
        let ramp = |lo: i32, hi: i32| -> u8 {
            if t < lo {
                0
            } else if t > hi {
                255
            } else {
                ((t - lo) * 255 / (hi - lo).max(1)) as u8
            }
        };
        if tf < big_t * 0.9 {
            let a = ramp((big_t * 0.1) as i32, (big_t * 0.3) as i32);
            out.push(Draw::Tiles { pack: ERA_ART_1[self.previous], alpha: 0xFF });
            out.push(Draw::Tiles { pack: ERA_ART_2[self.previous], alpha: a });
        } else if tf > big_t * 0.9 && tf < big_t * 1.1 {
            let (lo, hi) = ((big_t * 0.9) as i32, (big_t * 1.1) as i32);
            let a = 255 - (t - lo) * 255 / (hi - lo).max(1);
            out.push(Draw::Tiles { pack: ERA_ART_1[self.current], alpha: 0xFF });
            out.push(Draw::Tiles { pack: ERA_ART_2[self.previous], alpha: a.clamp(0, 255) as u8 });
        } else {
            let lo = (big_t * 1.1) as i32;
            let hi = ((self.count() as i32 - self.mark) as f32 * 0.4 + big_t) as i32;
            out.push(Draw::Tiles { pack: ERA_ART_1[self.current], alpha: 0xFF });
            out.push(Draw::Tiles { pack: ERA_ART_2[self.current], alpha: ramp(lo, hi) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(current_done: bool) -> EraInput {
        EraInput {
            previous: 0,
            current: 1,
            completed: 0,
            current_done,
            era_names: ["CLASSIC".into(), "GOLDEN".into(), "MODERN".into()],
            cars: vec![(1950, "356".into()), (1970, "914".into()), (1975, "930".into())],
            tournaments: [vec!["A".into()], vec!["B".into(), "C".into()], vec![]],
            title: "PORSCHE".into(),
            centre: (320, 240),
        }
    }

    /// 0x4FD76D…0x4FDA10: the old era's pulse, 1950 and its car, …, at 1970 the mark and the
    /// new era's pulse before the year, …, then the title's 7 + 10 + 6 + 20 entries.
    #[test]
    fn classic_to_golden_order() {
        let s = EndOfEra::build(&input(false), &mut FeRng::new(0));
        let e = &s.entries;
        assert_eq!(e[0].text, "CLASSIC");
        assert_eq!(e[..11].iter().map(|x| x.scale).collect::<Vec<_>>(), vec![1.0, 1.5, 2.0, 2.5, 2.0, 1.5, 1.0, 1.0, 1.0, 1.0, 1.0]);
        assert!(e[11].year_kind && e[11].year == 1950);
        assert_eq!(e[12].text, "356");
        // 1951…1969: 19 years without cars
        let at = 13 + 19;
        assert_eq!(s.mark, at as i32);
        assert_eq!(e[at].text, "GOLDEN");
        assert_eq!(e[at + 11].year, 1970);
        assert_eq!(e[at + 12].text, "914");
        let title = 7 + 10 + 6 + 20;
        // 1970…1989: 20 years, two cars in them
        assert_eq!(s.count(), at + 11 + 20 + 2 + title);
        assert_eq!(e[s.count() - 1].alpha, 0);
        assert_eq!(e[s.count() - 2].alpha, 0xE8);
    }

    #[test]
    fn evolution_lists_tournaments_with_the_years() {
        let s = EndOfEra::build(&input(true), &mut FeRng::new(0));
        let e = &s.entries;
        assert_eq!(s.mark, 0);
        assert_eq!(e[11].text, "A");
        assert_eq!(e[12].year, 1950);
        assert_eq!(e[13].text, "GOLDEN");
        assert_eq!((e[24].text.as_str(), e[25].year, e[26].text.as_str(), e[27].year), ("B", 1956, "C", 1960));
    }

    /// 0x4FF2C8: a step every 12 ticks; the entries behind the step are zeroed; the last step
    /// finishes the show.
    #[test]
    fn steps_and_finish() {
        let mut s = EndOfEra::build(&input(false), &mut FeRng::new(0));
        let n = s.count();
        let mut now = 1;
        s.frame(now);
        assert_eq!(s.step, 0);
        for _ in 0..n {
            now += STEP_TICKS;
            s.frame(now);
        }
        assert!(s.finished);
        assert!(s.movie_started().is_some());
        assert_eq!(s.frame(now + 1)[0], Draw::Tiles { pack: "G2.fsh", alpha: 0xFF });
    }

    /// 0x4FF180: after the show the screen waits for the movie's length; ESC leaves at once and
    /// only once.
    #[test]
    fn leaving() {
        let mut s = EndOfEra::build(&input(false), &mut FeRng::new(0));
        assert!(!s.tick(10));
        s.finished = true;
        s.movie_at = Some(100);
        assert!(!s.tick(100 + 2560));
        assert!(s.tick(100 + 2561));
        assert!(!s.tick(100 + 9999));
        let mut s = EndOfEra::build(&input(false), &mut FeRng::new(0));
        assert!(!s.key(0x20));
        assert!(s.key(0x1B));
        assert!(s.tick(5));
    }

    #[test]
    fn background_ramps() {
        let mut s = EndOfEra::build(&input(false), &mut FeRng::new(0));
        s.mark = 100;
        let bg = |s: &EndOfEra| {
            let mut v = Vec::new();
            s.background(&mut v);
            v
        };
        s.step = 5;
        assert_eq!(bg(&s), vec![Draw::Tiles { pack: "C1.fsh", alpha: 255 }, Draw::Tiles { pack: "C2.fsh", alpha: 0 }]);
        s.step = 20;
        assert_eq!(bg(&s)[1], Draw::Tiles { pack: "C2.fsh", alpha: 127 });
        s.step = 100;
        assert_eq!(bg(&s)[0], Draw::Tiles { pack: "G1.fsh", alpha: 255 });
        assert_eq!(bg(&s)[1], Draw::Tiles { pack: "C2.fsh", alpha: 255 - 127 });
    }

    #[test]
    fn digits_of_a_year() {
        let mut s = EndOfEra::build(&input(false), &mut FeRng::new(0));
        s.step = 11;
        s.first = 1;
        s.step_at = 1;
        s.blink_at = 1;
        let d = s.frame(2);
        assert!(d.iter().any(|x| matches!(x, Draw::Digits { digits: [1, 9, 5, 0], .. })));
    }
}
