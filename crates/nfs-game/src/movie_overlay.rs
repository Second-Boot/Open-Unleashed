//! `NFS5.MOVIECIRCLE`'s procedural overlay, **0x4CDF40** (`RE/notes/controls.md` §4.33 and the
//! credits paragraph after the cheat codes): six glinty lines, three `circles.fsh` flashes and four
//! "NEED FOR SPEED" runs over the widget's box, built once by 0x4CDC80 and run every draw off the
//! millisecond clock and the C runtime's `rand`.
//!
//! APPROXIMATION: the original seeds `rand` from the time (`srand(time)` at 0x418100) and shares it
//! with the whole program, so no two runs agree; this one has a generator of its own with a fixed
//! seed, which makes a still reproducible.

/// The C runtime's `rand`, 0x5A17C9: `s = s·0x343FD + 0x269EC3`, `(s >> 16) & 0x7FFF`.
#[derive(Debug, Clone)]
pub struct CRand(u32);

impl CRand {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }
    pub fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(0x343FD).wrapping_add(0x269EC3);
        ((self.0 >> 16) & 0x7FFF) as i32
    }
}

/// The overlay's box: `{left, top, right, bottom}` (0x4CDC9A…0x4CDCE8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box4 {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// One glinty line, 0x51F9A0 / 0x51FAA0 (§4.28, §4.29).
#[derive(Debug, Clone)]
pub struct Line {
    pub pos: i32,
    lo: i32,
    hi: i32,
    fixed: i32,
    len: i32,
    step_div: i32,
    last: i64,
    forward: bool,
    /// `[+0x19]`: travels top…bottom (a horizontal bar) when set
    pub horizontal: bool,
    sweeping: bool,
    start: i32,
    width: i32,
    speed: i32,
    dir: i32,
    sweep_t: i64,
    wait: i32,
    s_lo: i32,
    s_hi: i32,
}

impl Line {
    fn new(b: Box4, horizontal: bool, now: i64, r: &mut CRand) -> Self {
        let (lo, hi, fixed, len, s_lo, s_hi) = if horizontal {
            (b.top, b.bottom, b.left, b.right - b.left, b.left, b.right)
        } else {
            (b.left, b.right, b.top, b.bottom - b.top, b.top, b.bottom)
        };
        let pos = r.next() % (hi - lo).max(1) + lo;
        let step_div = r.next() % 5 + 5;
        let forward = r.next() & 1 == 1;
        let wait = r.next() & 127;
        Self { pos, lo, hi, fixed, len, step_div, last: now, forward, horizontal, sweeping: false, start: 0, width: 0, speed: 0, dir: 1, sweep_t: now, wait, s_lo, s_hi }
    }

    /// 0x51FAA0's update half: the bar steps by the elapsed time over its divisor and bounces, and
    /// the sweep waits, starts, runs and stops.
    fn update(&mut self, now: i64, r: &mut CRand) {
        let mut n = ((now - self.last) / self.step_div as i64) as i32;
        if n == 0 {
            n = 1;
        }
        self.last = now;
        if !self.forward {
            n = -n;
        }
        self.pos += n;
        if self.pos < self.lo {
            self.pos = self.lo;
            self.forward = true;
        }
        if self.pos > self.hi - 1 {
            self.pos = self.hi - 1;
            self.forward = false;
        }
        if !self.sweeping && now > self.sweep_t + self.wait as i64 {
            self.sweeping = true;
            self.sweep_t = now;
            self.width = (self.s_hi - self.s_lo) / 5;
            self.speed = r.next() % 3 + 3;
            if r.next() & 1 == 1 {
                self.dir = 1;
                self.start = self.s_lo - self.width;
            } else {
                self.dir = -1;
                self.start = self.s_hi + self.width;
            }
        }
        if self.sweeping {
            self.start += self.speed * (now - self.sweep_t) as i32 * self.dir;
            self.sweep_t = now;
            let past = if self.dir == 1 { self.start > self.s_hi + self.width } else { self.start < self.s_lo - self.width };
            if past {
                self.sweeping = false;
                self.sweep_t = now;
                self.wait = r.next() & 127;
            }
        }
    }
}

/// One flash, 0x520670 / 0x520740.
#[derive(Debug, Clone)]
pub struct Flash {
    active: bool,
    pub frame: i32,
    x_line: usize,
    y_line: usize,
    start: i64,
    wait: i32,
}

impl Flash {
    fn new(now: i64, r: &mut CRand) -> Self {
        Self { active: false, frame: 0, x_line: 0, y_line: 0, start: now, wait: r.next() & 0x1FF }
    }
}

/// One text run, 0x520890 / 0x5208E0.
#[derive(Debug, Clone)]
pub struct Run {
    active: bool,
    start: i64,
    wait: i32,
    duration: i32,
    pub font: usize,
    pub align: i32,
    pub vertical: bool,
    pub x: i32,
    pub y: i32,
    b: Box4,
}

impl Run {
    fn new(b: Box4, now: i64, r: &mut CRand) -> Self {
        Self { active: false, start: now, wait: r.next() % 0x180, duration: 0, font: 0, align: 0, vertical: false, x: 0, y: 0, b }
    }
}

/// What one frame draws, in the order 0x4CDF40 draws it.
#[derive(Debug, Clone, PartialEq)]
pub enum Draw {
    /// 0x4D9280: a bar, in the lines' tint
    Bar { x: f32, y: f32, w: f32, h: f32 },
    /// 0x4D94A0: a line's sweep — its leading edge `along` the long axis from `s_lo`, clipped to
    /// `s_lo…s_hi`; `forward` puts the bright end first
    Sweep { horizontal: bool, cross: f32, s_lo: f32, len: f32, along: f32, width: f32, forward: bool },
    /// 0x4D9AF0: `circles.fsh` entry `frame`, centred on `(cx, cy)`, in the flashes' tint
    Flash { frame: usize, cx: f32, cy: f32 },
    /// 0x522050: "NEED FOR SPEED", a letter at a time, in the runs' tint scaled by `alpha`
    Run { font: usize, x: f32, y: f32, alpha: u8, gap: i32, align: i32, vertical: bool },
}

pub const RUN_TEXT: &str = "NEED FOR SPEED";

#[derive(Debug, Clone)]
pub struct MovieOverlay {
    rand: CRand,
    pub lines: Vec<Line>,
    flashes: Vec<Flash>,
    runs: Vec<Run>,
    /// the clock the objects were last run at
    pub at: i64,
}

/// The step the port runs the overlay at. APPROXIMATION: the original runs it once a drawn frame
/// off the real clock; a fixed 16 ms step keeps a still reproducible.
pub const STEP_MS: i64 = 16;

impl MovieOverlay {
    /// 0x4CDC80: three lines with `horizontal` set, three with it clear, three flashes, four runs.
    pub fn new(b: Box4, now: i64, seed: u32) -> Self {
        let mut rand = CRand::new(seed);
        let mut lines = Vec::new();
        for _ in 0..3 {
            lines.push(Line::new(b, true, now, &mut rand));
        }
        for _ in 0..3 {
            lines.push(Line::new(b, false, now, &mut rand));
        }
        let flashes = (0..3).map(|_| Flash::new(now, &mut rand)).collect();
        let runs = (0..4).map(|_| Run::new(b, now, &mut rand)).collect();
        Self { rand, lines, flashes, runs, at: now }
    }

    /// Text runs alone: `NFS5.ANIMSHOWCASEPICTURE`'s bind 0x4BDB90 hands 0x520860 the box
    /// `{200, 0, 400, 200}` (0x4BDCE2…0x4BDCFD) and builds **five** runs (0x118 bytes of 0x38,
    /// 0x4BDD05…0x4BDD2B) with the same 0x520890, and no lines or flashes.
    pub fn runs_only(b: Box4, n: usize, now: i64, seed: u32) -> Self {
        let mut rand = CRand::new(seed);
        let runs = (0..n).map(|_| Run::new(b, now, &mut rand)).collect();
        Self { rand, lines: Vec::new(), flashes: Vec::new(), runs, at: now }
    }

    /// Run the objects forward to `now` in `STEP_MS` steps.
    pub fn advance(&mut self, now: i64) {
        while self.at + STEP_MS <= now {
            self.at += STEP_MS;
            self.step(self.at);
        }
    }

    fn step(&mut self, now: i64) {
        for l in &mut self.lines {
            l.update(now, &mut self.rand);
        }
        // 0x520740: x off a line of the first list (the three with `horizontal` set), y off one
        // of the second — as written
        for f in &mut self.flashes {
            if !f.active && now - f.start > f.wait as i64 {
                f.active = true;
                f.frame = 0;
                f.x_line = (self.rand.next() % 3) as usize;
                f.y_line = 3 + (self.rand.next() % 3) as usize;
                f.start = now;
            }
            if f.active {
                f.frame = ((now - f.start) / 16) as i32;
                if f.frame >= 5 {
                    f.active = false;
                    f.start = now;
                    f.wait = self.rand.next() & 0x1FF;
                }
            }
        }
        for run in &mut self.runs {
            if !run.active && now > run.start + run.wait as i64 {
                run.font = (self.rand.next() % 2) as usize;
                run.active = true;
                run.start = now;
                run.duration = (self.rand.next() & 0xFF) + 0x100;
                run.wait = self.rand.next() % 0x180;
                run.align = self.rand.next() % 3;
                run.vertical = self.rand.next() & 1 == 1;
                let b = run.b;
                run.x = self.rand.next() % (b.right - b.left).max(1) + b.left;
                run.y = (self.rand.next() & (b.bottom - b.top)) + b.top;
            }
            if run.active && now > run.duration as i64 + run.start {
                run.active = false;
                run.start = now;
                run.wait = self.rand.next() % 0x180;
            }
        }
    }

    /// The frame as the objects stand.
    pub fn draws(&self) -> Vec<Draw> {
        let mut out = Vec::new();
        for l in &self.lines {
            if l.horizontal {
                out.push(Draw::Bar { x: l.fixed as f32, y: l.pos as f32, w: l.len as f32, h: 1.0 });
            } else {
                out.push(Draw::Bar { x: l.pos as f32, y: l.fixed as f32, w: 1.0, h: l.len as f32 });
            }
            if l.sweeping {
                out.push(Draw::Sweep {
                    horizontal: l.horizontal,
                    cross: l.pos as f32,
                    s_lo: l.s_lo as f32,
                    len: (l.s_hi - l.s_lo) as f32,
                    along: (l.start - l.s_lo) as f32,
                    width: l.width as f32,
                    forward: l.dir == 1,
                });
            }
        }
        for f in &self.flashes {
            if f.active && f.frame < 5 {
                out.push(Draw::Flash { frame: f.frame as usize, cx: self.lines[f.x_line].pos as f32, cy: self.lines[f.y_line].pos as f32 });
            }
        }
        for run in &self.runs {
            if !run.active {
                continue;
            }
            // 0x520A07…0x520A9A
            let t = (self.at - run.start) as f32 / run.duration as f32;
            let (a, gap) = if (t as f64) < 0.2 {
                (t * 5.0, 1)
            } else {
                let u = (t - 0.2) * 1.25;
                (1.0 - (u + 0.2).min(1.0), 1 - (u * -49.0) as i32)
            };
            out.push(Draw::Run { font: run.font, x: run.x as f32, y: run.y as f32, alpha: (a * 255.0) as u8, gap, align: run.align, vertical: run.vertical });
        }
        out
    }
}

/// The credits roll's string range: `FEStrings` 6000 … 6467 (0x1770…0x1943), wrapping by 0x1D3
/// (0x4CE257…0x4CE277, 0x4CE28E…0x4CE2A8).
pub const CREDITS_FIRST: i32 = 0x1770;
pub fn credits_wrap(id: i32) -> i32 {
    if id < 0x1770 {
        id + 0x1D3
    } else if id > 0x1943 {
        id - 0x1D3
    } else {
        id
    }
}

/// The roll's state, `[+0x23C]` the first string on screen and `[+0x244]` how far it has scrolled
/// past the top; the bind sets 6000 and 0 (0x4CDBAA, 0x4CDBB4).
#[derive(Debug, Clone)]
pub struct CreditsRoll {
    pub id: i32,
    pub offset: i32,
    at: i64,
    last: i64,
}

impl CreditsRoll {
    pub fn new(now: i64) -> Self {
        Self { id: CREDITS_FIRST, offset: 0, at: now, last: now }
    }

    /// Run the roll to `now` in `STEP_MS` steps: past 4 ms since the last move the offset moves by
    /// `speed` (0x4CE118…0x4CE139), and once it passes the first line's height `lh(id)` that line
    /// is taken off and the next one leads — either way (0x4CE21D…0x4CE251).
    pub fn advance(&mut self, now: i64, speed: i32, lh: impl Fn(i32) -> i32) {
        while self.at + STEP_MS <= now {
            self.at += STEP_MS;
            if self.at - self.last > 4 {
                self.last = self.at;
                self.offset += speed;
            }
            let h = lh(self.id);
            if self.offset > h {
                self.offset -= h;
                self.id += 1;
            } else if -self.offset > h {
                self.offset += h;
                self.id -= 1;
            }
            self.id = credits_wrap(self.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_roll_steps_a_line_at_a_time_and_wraps() {
        let mut r = CreditsRoll::new(0);
        // every line 12 high, one pixel a step: 13 steps pass the first line
        r.advance(13 * STEP_MS, 1, |_| 12);
        assert_eq!((r.id, r.offset), (6001, 1));
        let mut back = CreditsRoll::new(0);
        back.advance(5 * STEP_MS, -3, |_| 12);
        // −15 passes the line above 6000, which wraps to 6467
        assert_eq!((back.id, back.offset), (6466, -3));
    }

    #[test]
    fn the_c_rand_sequence() {
        // MSVC's `srand(1)` stream
        let mut r = CRand::new(1);
        assert_eq!([r.next(), r.next(), r.next()], [41, 18467, 6334]);
    }

    #[test]
    fn the_overlay_runs_inside_its_box() {
        let b = Box4 { left: 112, top: 100, right: 528, bottom: 448 };
        let mut o = MovieOverlay::new(b, 0, 1);
        for t in (0..20_000).step_by(16) {
            o.advance(t);
            for l in &o.lines {
                let (lo, hi) = if l.horizontal { (b.top, b.bottom) } else { (b.left, b.right) };
                assert!(l.pos >= lo && l.pos < hi, "line at {} outside {lo}..{hi}", l.pos);
            }
            for d in o.draws() {
                if let Draw::Run { x, y, .. } = d {
                    assert!(x >= b.left as f32 && x < b.right as f32 && y >= b.top as f32 && y <= b.bottom as f32);
                }
            }
        }
    }
}
