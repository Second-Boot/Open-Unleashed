//! The start-up movies' player, **0x4DC850** (`FEIntro.cpp`; `RE/notes/mad.md` §11 and §12).
//!
//! 0x4DD600 (the boot screen, main 0x4B6C42 unless `NOINTROMOVIES`) plays `earts-av.mad` through
//! it and then 0x4DD4A0 plays `inPC-av.mad` the same way, each after three blank flips. The player
//! is one blocking loop over the file; here it is the same loop cut at the places where the
//! original waits — the full-ring wait (0x4DCEC9…0x4DCF92), the tail (0x4DD2E9…0x4DD399) and the
//! audio drain (0x4DD3B2…0x4DD402) — so that one [`IntroPlayer::step`] is one pass of whichever
//! wait the original is in. Everything else runs straight through, as it does there.
//!
//! The frame type is generic so the rules can be tested without a decoder; the port's is the
//! RGBA of `MadDecoder`.

use std::collections::VecDeque;

/// The ring 0x4DC180 is asked for: `count` 6 (0x4DC9D3), `[0x6903F0]`.
pub const RING: usize = 6;
/// `|drift| > 0x108` snaps the clock (0x4DCCDD, and the same test at the four other sync sites).
pub const DRIFT_SNAP: i32 = 0x108;
/// A frame due in less than this many ms is dropped, with every inter frame after it up to the
/// next intra one (0x4DCDDF).
pub const LATE_DROP: i32 = 0x43;
/// A `MADe` frame is skipped when due in less than this (0x4DCD2B) — or always while the display
/// converts in software (0x4DCD20; the port always does, `mad.md` §11).
pub const LATE_SKIP_E: i32 = 0x85;

/// The frame clock and its audio sync: base `[0x65B9C8]`, sync on `[0x65B9CC]` (the sound
/// player's id ≥ 0, 0x4DCC0B), the last audio position `[0x65E3D8]` and the drift accumulator
/// `[0x65E3EC]` (0x4DCC6C…0x4DCD0B, repeated at 0x4DCEC9, 0x4DD01A, 0x4DD23B and 0x4DD2F7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clock {
    pub base: i32,
    pub audio: bool,
    pub last_pos: i32,
    pub acc: i32,
}

impl Clock {
    /// 0x4DCC19: the base is the millisecond clock (0x53BFD0) once the sound has been started;
    /// 0x4DCC25…0x4DCC31 clear the rest.
    pub fn new(now: i32, audio: bool) -> Self {
        Self { base: now, audio, last_pos: 0, acc: 0 }
    }

    /// The movie time now. `pos` is the sound's position in ms less the output latency — 0x566910's
    /// `+4` (`played · 1000 / rate`, 0x5669B9…0x5669D5) minus 0x56AFA0. Only a position past the last
    /// one moves the accumulator, `acc += pos − acc/8 − elapsed` (an eight-tap running sum of the
    /// drift); beyond ±0x108 the base moves by `acc/8` and the sum restarts. The divisions round
    /// toward zero (`cdq ; and edx, 7 ; add ; sar 3`).
    pub fn elapsed(&mut self, now: i32, pos: i32) -> i32 {
        let mut e = now.wrapping_sub(self.base);
        if self.audio && pos > self.last_pos {
            self.last_pos = pos;
            self.acc += pos - self.acc / 8 - e;
            if self.acc.abs() > DRIFT_SNAP {
                self.base -= self.acc / 8;
                self.acc = 0;
                e = now.wrapping_sub(self.base);
            }
        }
        e
    }
}

/// A video chunk's kind, by its tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `MADk`, intra
    K,
    /// `MADm`, inter
    M,
    /// `MADe` — accepted (0x4DC8E1) but in none of the shipped files
    E,
}

impl Kind {
    pub fn of(tag: &[u8; 4]) -> Option<Self> {
        match tag {
            b"MADk" => Some(Self::K),
            b"MADm" => Some(Self::M),
            b"MADe" => Some(Self::E),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// the top of the main loop, 0x4DCC43
    Main,
    /// 0x4DCEA7: a frame to decode, waiting for a free slot
    WaitSlot,
    /// 0x4DD23B: the file is done (or the player stopped); showing what is left in the ring
    Tail,
    /// 0x4DD3A4: waiting for the sound to end
    AudioWait,
    Done,
}

/// What one step is given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tick {
    /// the millisecond clock, 0x53BFD0
    pub now: i32,
    /// the sound's position in ms (0 when there is none — 0x566910 clears its answer first)
    pub audio_pos: i32,
    /// 0x566910's state is 3: the sound has finished
    pub audio_done: bool,
}

pub struct IntroPlayer<F> {
    kinds: Vec<Kind>,
    /// `[0x65E268]`, 16.16 ms
    period: u32,
    /// the current video chunk (`ebx` / `[esp+0x14]`)
    cur: Option<usize>,
    /// the next frame's time, whole ms (`[esp+0x10]`) and its 16-bit fraction (`[esp+0x18]`)
    t: i32,
    frac: u32,
    /// `{time, surface}` at `[0x65B9D0 + i·8]`, `[0x65BA64]` of them, kept in time order
    ring: VecDeque<(i32, F)>,
    pub clock: Clock,
    /// `[0x65E3D4]`
    stop: bool,
    /// `*abort` — set when 0x569420 answers 2, the software YUV conversion (0x4DCC5A); neither
    /// caller reads it
    software: bool,
    phase: Phase,
    /// `[0x65E3E4]`, frames dropped late
    pub dropped: u32,
    /// `[0x65BA5C]`, `MADe` frames skipped
    pub skipped: u32,
}

impl<F> IntroPlayer<F> {
    /// The first video chunk's period (0x4DC947…0x4DC952) and the chunk list.
    pub fn new(kinds: Vec<Kind>, period: u32) -> Self {
        let cur = if kinds.is_empty() { None } else { Some(0) };
        Self {
            kinds,
            period,
            cur,
            t: 0,
            frac: 0,
            ring: VecDeque::with_capacity(RING),
            clock: Clock::new(0, false),
            stop: false,
            software: true,
            phase: if cur.is_some() { Phase::Main } else { Phase::Done },
            dropped: 0,
            skipped: 0,
        }
    }

    /// The period added to the frame time: 16.16, whole part to the time, fraction carried
    /// (0x4DCAF6…0x4DCB1C).
    fn advance(&mut self) {
        let s = self.frac.wrapping_add(self.period);
        self.frac = s & 0xFFFF;
        self.t += (s as i32) >> 16;
    }

    fn next_chunk(&mut self) {
        self.cur = self.cur.map(|c| c + 1).filter(|&c| c < self.kinds.len());
    }

    /// 0x4DCAB1…0x4DCAEF: the frame goes in at its time — a sorted insert from the end.
    fn insert(&mut self, frame: F) {
        let at = self.ring.iter().rposition(|(t, _)| self.t > *t).map_or(0, |i| i + 1);
        self.ring.insert(at, (self.t, frame));
    }

    /// Before the clock starts, 0x4DCA05…0x4DCB84: decode until the ring is full or the file ends.
    pub fn preload(&mut self, decode: &mut dyn FnMut(usize) -> F) {
        while let Some(c) = self.cur {
            if self.ring.len() >= RING {
                break;
            }
            let f = decode(c);
            self.insert(f);
            self.advance();
            self.next_chunk();
        }
    }

    /// 0x4DCC0B…0x4DCC31: the clock starts after the sound has.
    pub fn start(&mut self, now: i32, audio: bool) {
        self.clock = Clock::new(now, audio);
        self.stop = false;
    }

    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }

    pub fn ring_len(&self) -> usize {
        self.ring.len()
    }

    /// **0x4DC610(elapsed)**, the display: nothing while the ring is empty or its first frame is
    /// not yet due; otherwise the **last** due frame is shown and every due one leaves the ring
    /// (0x4DC64F…0x4DC66D picks `k`, 0x4DC7ED…0x4DC831 rotates them out). 0x565560 — the display
    /// ready — is taken as always true.
    pub fn display(&mut self, elapsed: i32) -> Option<F> {
        let first = self.ring.front()?.0;
        if elapsed < first {
            return None;
        }
        let mut k = 1;
        while k < self.ring.len() && elapsed >= self.ring[k].0 {
            k += 1;
        }
        let mut shown = None;
        for _ in 0..k {
            shown = self.ring.pop_front().map(|(_, f)| f);
        }
        shown
    }

    /// One pass. `skip` is the original's key read — the next key from the keyboard ring is Esc
    /// (0x55FF60 == 0x1B) or a mouse button is down (0x5372A0(6) `+0x10…+0x13`) — called exactly
    /// where it reads them: after each decoded frame (0x4DD1FE) and in the audio drain (0x4DD3C5).
    /// Returns the frame to show, if one was shown.
    pub fn step(&mut self, tick: Tick, decode: &mut dyn FnMut(usize) -> F, skip: &mut dyn FnMut() -> bool) -> Option<F> {
        let mut shown = None;
        loop {
            match self.phase {
                Phase::Main => {
                    let Some(mut c) = self.cur.filter(|_| !self.stop) else {
                        self.phase = Phase::Tail;
                        continue;
                    };
                    let e = self.clock.elapsed(tick.now, tick.audio_pos);
                    // 0x4DCD0D: a `MADe` is skipped when late or while converting in software
                    if self.kinds[c] == Kind::E && (self.software || self.t - e < LATE_SKIP_E) {
                        self.skipped += 1;
                        self.advance();
                        self.next_chunk();
                        match self.cur {
                            Some(n) => c = n,
                            None => {
                                self.phase = Phase::Tail;
                                continue;
                            }
                        }
                    }
                    // 0x4DCDD9: late — drop it and every frame up to the next intra one; the time
                    // is not tested again inside that run (0x4DCE99 → 0x4DCDE8)
                    if self.t - e < LATE_DROP {
                        loop {
                            if self.kinds[c] != Kind::E {
                                self.dropped += 1;
                            } else {
                                self.skipped += 1;
                            }
                            self.advance();
                            self.next_chunk();
                            match self.cur {
                                Some(n) if self.kinds[n] == Kind::K => break,
                                Some(n) => c = n,
                                None => break,
                            }
                        }
                        if self.cur.is_none() {
                            self.phase = Phase::Tail;
                            continue;
                        }
                    }
                    self.phase = Phase::WaitSlot;
                }
                Phase::WaitSlot => {
                    if self.ring.len() >= RING {
                        let e = self.clock.elapsed(tick.now, tick.audio_pos);
                        if let Some(f) = self.display(e) {
                            shown = Some(f);
                        }
                        if self.ring.len() >= RING {
                            return shown;
                        }
                    }
                    let c = self.cur.expect("a chunk to decode");
                    let f = decode(c);
                    self.insert(f);
                    self.advance();
                    self.next_chunk();
                    if skip() {
                        self.stop = true;
                    }
                    self.phase = Phase::Main;
                }
                Phase::Tail => {
                    let e = self.clock.elapsed(tick.now, tick.audio_pos);
                    if e >= self.t {
                        self.phase = Phase::AudioWait;
                        continue;
                    }
                    if let Some(f) = self.display(e) {
                        shown = Some(f);
                    }
                    return shown;
                }
                Phase::AudioWait => {
                    if !self.clock.audio {
                        self.phase = Phase::Done;
                        continue;
                    }
                    if skip() {
                        self.stop = true;
                    }
                    if tick.audio_done || self.stop {
                        self.phase = Phase::Done;
                        continue;
                    }
                    return shown;
                }
                Phase::Done => return shown,
            }
        }
    }
}

/// Where 0x4DC850 puts the picture on the 640×480 mode 0x4DD61F sets: pixel-doubled (the
/// `scale` 3 it passes 0x4DC180) at `(640/2 − w, 480/2 − h)` (0x4DC9AD…0x4DC9C0) — `{x, y, w, h}`.
pub fn placement(w: i32, h: i32) -> (i32, i32, i32, i32) {
    (640 / 2 - w, 480 / 2 - h, 2 * w, 2 * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERIOD: u32 = 4_369_067; // every shipped movie: 66.67 ms

    fn player(kinds: &[Kind]) -> IntroPlayer<usize> {
        IntroPlayer::new(kinds.to_vec(), PERIOD)
    }

    #[test]
    fn the_clock_accumulates_eight_times_the_drift_and_snaps_past_0x108() {
        let mut c = Clock::new(1000, true);
        // 20 ms ahead: acc = 20, no snap
        assert_eq!(c.elapsed(1100, 120), 100);
        assert_eq!(c.acc, 20);
        // the same position again moves nothing
        assert_eq!(c.elapsed(1110, 120), 110);
        assert_eq!(c.acc, 20);
        // 40 ms ahead: acc = 20 − 2 + 40 = 58
        assert_eq!(c.elapsed(1200, 240), 200);
        assert_eq!(c.acc, 58);
        // a big lead: 58 − 7 + 300 = 351 > 0x108 → base −= 351/8 = 43
        assert_eq!(c.elapsed(1300, 600), 343);
        assert_eq!((c.acc, c.base), (0, 957));
        // the sound behind: −300 truncates toward zero, base −= −37 and the movie waits
        let mut c = Clock::new(0, true);
        assert_eq!(c.elapsed(400, 100), 363);
        assert_eq!(c.base, 37);
        // no sync without sound
        let mut c = Clock::new(0, false);
        assert_eq!(c.elapsed(400, 100), 400);
    }

    #[test]
    fn frame_times_carry_the_16_16_fraction() {
        let mut p = player(&[Kind::K; 8]);
        let mut n = 0;
        p.preload(&mut |c| {
            n += 1;
            c
        });
        assert_eq!(n, RING);
        let times: Vec<i32> = p.ring.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, vec![0, 66, 133, 200, 266, 333]);
        assert_eq!(p.t, 400);
    }

    #[test]
    fn the_display_shows_the_last_due_frame_and_drops_the_rest() {
        let mut p = player(&[Kind::K; 8]);
        p.preload(&mut |c| c);
        assert_eq!(p.display(-1), None);
        assert_eq!(p.display(0), Some(0));
        assert_eq!(p.display(140), Some(2));
        assert_eq!(p.ring_len(), 3);
    }

    #[test]
    fn a_late_frame_drops_everything_up_to_the_next_intra_one() {
        use Kind::*;
        let kinds = [K, M, M, M, M, M, M, M, M, K, M, M];
        let mut p = player(&kinds);
        p.preload(&mut |c| c);
        p.start(0, false);
        // 6 frames in the ring, the next (index 6, due at 400) is late at 350: 6, 7, 8 dropped, the
        // intra frame 9 decoded
        let mut decoded = Vec::new();
        let shown = p.step(Tick { now: 350, audio_pos: 0, audio_done: false }, &mut |c| {
            decoded.push(c);
            c
        }, &mut || false);
        assert_eq!(p.dropped, 3);
        assert_eq!(decoded.first(), Some(&9));
        // the ring was full, so the display ran first: frames 0…5 are all due at 350
        assert_eq!(shown, Some(5));
    }

    #[test]
    fn the_skip_is_read_after_a_decode_and_the_ring_still_drains() {
        let mut p = player(&[Kind::K; 20]);
        p.preload(&mut |c| c);
        p.start(0, true);
        let mut reads = 0;
        let tick = |now| Tick { now, audio_pos: 0, audio_done: false };
        // the ring is full and nothing is due: no decode, no key read
        assert_eq!(p.step(tick(-10), &mut |c| c, &mut || {
            reads += 1;
            true
        }), None);
        assert_eq!(reads, 0);
        // frame 0 due: shown, a slot frees, frame 6 decoded, the key read says stop
        assert_eq!(p.step(tick(0), &mut |c| c, &mut || {
            reads += 1;
            true
        }), Some(0));
        assert_eq!(reads, 1);
        // stopped: the tail shows what is left until the next frame's time (466)…
        assert_eq!(p.step(tick(300), &mut |c| c, &mut || false), Some(4));
        assert!(!p.done());
        // …then the audio wait reads the key once more and ends on the stop flag
        assert_eq!(p.step(tick(470), &mut |c| c, &mut || false), None);
        assert!(p.done());
    }

    #[test]
    fn the_player_waits_for_the_sound_to_end() {
        let mut p = player(&[Kind::K; 2]);
        p.preload(&mut |c| c);
        p.start(0, true);
        let t = |now, done| Tick { now, audio_pos: 0, audio_done: done };
        assert_eq!(p.step(t(0, false), &mut |c| c, &mut || false), Some(0));
        assert_eq!(p.step(t(200, false), &mut |c| c, &mut || false), None);
        assert!(!p.done());
        assert_eq!(p.step(t(300, true), &mut |c| c, &mut || false), None);
        assert!(p.done());
    }

    #[test]
    fn the_picture_is_doubled_and_centred() {
        assert_eq!(placement(320, 240), (0, 0, 640, 480));
        assert_eq!(placement(320, 176), (0, 64, 640, 352));
    }
}
