//! The sound library's software-voice kernel (`RE/notes/sfx.md`, "The resampling kernel"): the
//! 16.16 step a voice walks its sample with, the two interpolators, the per-channel gain and its
//! ramp, and the float → 16-bit store of the mix.
//!
//! The library mixes in **floats**: each software voice channel is a chain of nodes pulled once a
//! service block (≤ 256 frames, 0x598940) — the decoder (PCM 0x597B60 / looped 0x59CBA0 / EA-XA
//! 0x597C40, samples as floats of their 16-bit values), the **resampler** 0x59BE00 (order word 2,
//! made by 0x599830 on the first `setPitch`), and the voice mixer 0x598760 adds the result into
//! one float buffer per output channel at the channel's gain; 0x59A350 stores the sum as 16 bits.
//!
//! The CPU paths: `[0x6B49A3]` is 0x597070's cpuid mask (bit 1 SSE, bit 2 3DNow!), and 0x58A871
//! clears the SSE bit unless config `+0x22` (`[0x6B48E2]`) is set — 0x4A66B0 writes it 0 (`bl`,
//! 0x4A67E5). So the game never takes the SSE kernels (0x59D350, 0x59D140, 0x59BAF0); on a CPU
//! without 3DNow! (every x86 made in the last decade and more) it runs the x87 ones ported here: 0x59CC60 (cubic),
//! 0x59CF70 (linear) and 0x59BA70 (the mix). The 3DNow! ones (0x59CD70, 0x59D040, 0x59BC40) are
//! not read. *(The SSE cubic 0x59D350 squares its `t²` again and weights the cubic term by `t⁴`;
//! the game's settings keep it off.)*
//!
//! APPROXIMATION left: the x87 intermediates are carried in f64; every f32 store of the original is
//! an f32 rounding here. Narrowed 2026-09-28 (`career.md` §11.4a, `RE/tools/fpucw.py`): every real
//! `fldcw`/`fnstcw` in the whole `.exe` was surveyed (59 confirmed sites out of ~680 raw opcode
//! hits, almost all of which turned out to be IAT/global address bytes mis-decoded as instructions,
//! not real ones) — none of them is in this library's own code, 0x596B60…0x59D350; the resampler,
//! decoder and mixer never touch the control word themselves. `_ftol` (716 call sites across
//! `.text`, this library's `fistp`s included) only ever changes rounding control and restores
//! exactly what it found, so it cannot be the source of a precision difference either. What is
//! still not read: the *ambient* control word — nothing in `nfs5.exe` sets precision to begin with
//! (no init-time `fldcw`/`_control87` call exists in the shipped files at all), so whatever the
//! OS/CRT thread default is stays in effect through the whole audio path, and what DirectX 7's own
//! runtime (`ddraw.dll`, not part of the shipped files) does to it around a draw call is likewise
//! not something a read of `nfs5.exe`/`dx7z.dll` can settle.
//!
//! **Measured 2026-09-28** (`career.md` §11.4c): nine `gdb -p <pid>; p/x $fctrl` samples of the
//! running process on a Debian x86_64 test machine, under both the shipped `dx7z.dll` driver
//! and the repack's default `opengl3` one, at idle and mid-movie — all nine read `0x027F`: PC =
//! 53-bit (double) precision, RC = round-to-nearest, all exceptions masked. This is the first
//! direct evidence for the `f64`-intermediate reading below (matches it) but does not retire the
//! APPROXIMATION: it is wine 5.12's own `ddraw`/`d3d` on Linux, not Microsoft's DX7 runtime on
//! Windows, and no sample was taken inside a live audio/physics tick (career-mode profile
//! creation could not be driven to completion through this build's front end in the time
//! available — `RE/notes/REFERENCE-2026-09-28.md`).

/// 0x599830: the step is capped at 4.0 (`cmp ebx, 0x40000 ; jle`, signed).
pub const STEP_MAX: u32 = 0x40000;

/// The step that leaves a sample alone: 0x59BE00 then only copies (0x59BE1A `cmp ebp, 0x10000`),
/// which gives what the kernels give at `t = 0`.
pub const STEP_UNITY: u32 = 0x10000;

/// **0x596B60 → 0x596A50 → 0x599830**: a voice channel's 16.16 step. At the start 0x596B60 keeps
/// `rate · 16.0 / [0x6B48DA]` as an f32 (`fild` the request's rate word, `fmul [0x5B26A8]` = 16.0,
/// `fidiv` the mix rate, `fstp [+0x1C]`); the driver's `setPitch` 0x596A50 multiplies the voice's
/// pitch word `voice+0x6E` (4096 = unchanged, 0x589FC0) by it and truncates (`_ftol` 0x5A0F98);
/// 0x599830 caps it at [`STEP_MAX`] and 0x59BDF0 stores it in the resampler (`+0x18`).
pub fn step_16_16(pitch_word: u16, rate: u32, mix_rate: u32) -> u32 {
    let per = (rate as f64 * 16.0 / mix_rate.max(1) as f64) as f32;
    let step = (pitch_word as f64 * per as f64) as i64 as i32;
    step.min(STEP_MAX as i32).max(0) as u32
}

/// The caller's pitch word `voice+0x6C`: 4096 is 1.0, unchanged — every start/update call in the
/// original hands this word directly (`Voice_play`'s `pitch16_16 >> 4`, `Voice_setPitch`'s low 16
/// bits, 0x4A9AC5 / 0x565FEB), never a float. *(Until 2026-09-27 (audio port 8, this pass) every
/// `mixer`/`AudioOutput` caller gave a float factor here, rounded to this word by a port-only
/// `resample::pitch_word(f32) -> u16` with no counterpart in the original — every caller now hands
/// the word itself, [`crate::mixer::AudioOutput::play_word`] / `set_word`, so that bridge function
/// is gone.)* The library's word `voice+0x6E = voice+0x6C · voice+0x6A >> 12` (0x589FC0),
/// `voice+0x6A` the voice's cents through 0x589E80, is `mixer`'s (`Voice::pitch_word`,
/// `mixer::cents_word`).
pub const PITCH_UNIT: u16 = 4096;

/// Which interpolator 0x599830 gives the resampler: `[0x6B48E5]` (config `+0x25`) `>= 50` → the
/// 4-tap cubic, else the 2-tap linear one (`cmp dl, 0x32 ; sbb eax, eax ; inc eax` into 0x59C080's
/// third argument). 0x4A66B0 sets it from `0x5B4B20[q]` = 100 / 60 / 30 (0x4A67A7 → `[esp+0x31]`),
/// so qualities 0 and 1 are cubic and 2 linear; the library's own default is 0 (0x58A5D2, `cl`),
/// kept at quality 3 — linear.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Interp {
    #[default]
    Linear,
    Cubic,
}

impl Interp {
    pub fn for_quality(audio_quality: u8) -> Self {
        let level: u8 = match audio_quality {
            0 => 100,
            1 => 60,
            2 => 30,
            _ => 0,
        };
        if level >= 0x32 { Interp::Cubic } else { Interp::Linear }
    }

    /// How many samples before the current one the kernel reads — 0x59C080's `+0x20` (1 cubic,
    /// 0 linear); the resampler's history `+0x22` holds 4 / 2. A voice starts with these zeroed
    /// (0x59BF59 `rep stosd`), so the cubic's first `s[−1]` is 0.
    pub fn lookbehind(self) -> i64 {
        match self {
            Interp::Cubic => 1,
            Interp::Linear => 0,
        }
    }
}

/// 0x59CC60's `t` unit, `[0x5C1818]` = 1.5258e-5 (0x377FFC9C) — a hair under 2⁻¹⁶.
const CUBIC_T: f32 = f32::from_bits(0x377F_FC9C);

/// **0x59CC60**, the x87 cubic: `t = frac16 · [0x5C1818]`, then with the four samples
/// `s[−1], s0, s1, s2` around the position
/// `a = (3(s0 − s1) − s[−1] + s2) / 2`, `b = (2 s1 + s[−1]) − (5 s0 + s2) / 2`, `c = (s1 − s[−1]) / 2`
/// and `out = ((a·t³ + b·t²) + c·t) + s0` in that order, stored as an f32 — a Catmull-Rom spline.
pub fn cubic(sm1: f32, s0: f32, s1: f32, s2: f32, frac16: u32) -> f32 {
    let (sm1, s0, s1, s2) = (sm1 as f64, s0 as f64, s1 as f64, s2 as f64);
    let t = (frac16 & 0xFFFF) as f64 * CUBIC_T as f64;
    let t2 = t * t;
    let a = ((s0 - s1) * 3.0 - sm1 + s2) * 0.5;
    let at3 = a * (t2 * t);
    let b = (s1 + s1 + sm1) - (s0 * 5.0 + s2) * 0.5;
    let sum = b * t2 + at3;
    let c = (s1 - sm1) * 0.5 * t;
    ((c + sum) + s0) as f32
}

/// **0x59CF70**, the x87 linear: `out = (s1 − s0) · frac32 · 2⁻³²  + s0` (`fild qword` of the
/// 32-bit fraction with a zero high word, `[0x5E3B88]` = 2⁻³²), an f32. The fraction is the 16.16
/// step's low word shifted up (0x59C003), so `frac32 · 2⁻³² = frac16 / 65536` exactly.
pub fn linear(s0: f32, s1: f32, frac16: u32) -> f32 {
    let f = ((frac16 & 0xFFFF) as u64) << 16;
    ((s1 as f64 - s0 as f64) * f as f64 * (1.0 / 4294967296.0) + s0 as f64) as f32
}

/// One output sample of the interpolator at stream position `pos` + `frac16 / 65536`, with
/// `sample(k)` the voice's input stream (0 before its start).
pub fn interpolate(interp: Interp, frac16: u32, pos: i64, sample: impl Fn(i64) -> f32) -> f32 {
    match interp {
        Interp::Cubic => cubic(sample(pos - 1), sample(pos), sample(pos + 1), sample(pos + 2), frac16),
        Interp::Linear => linear(sample(pos), sample(pos + 1), frac16),
    }
}

/// Where a voice is in its input stream: the sample index and the 16-bit fraction the resampler
/// keeps (`+0x10` the index within a block, `+0x14` the fraction; `idx += (frac + step) >> 16`,
/// `frac = (frac + step) & 0xFFFF`, 0x59CCBC…0x59CCD4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub pos: u64,
    pub frac: u32,
}

impl Cursor {
    pub fn advance(&mut self, step: u32) {
        let f = self.frac + step;
        self.pos += (f >> 16) as u64;
        self.frac = f & 0xFFFF;
    }
}

/// **0x598700**: a channel's target gain is `volume · pan` (both f32: the level byte `voice+0x3D`
/// and the pan table's byte, each `· [0x5C1788]` = 1/127, 0x596983 / 0x59694C) as an f32, and
/// below 0.005 (`[0x5B2738]`, `fcomp ; test ah, 1`) it is 0.
pub fn channel_gain(volume: f32, pan: f32) -> f32 {
    // `fst` stores the f32, `fcomp` tests the unrounded product
    let g = volume as f64 * pan as f64;
    if g < f32::from_bits(0x3BA3_D70A) as f64 { 0.0 } else { g as f32 }
}

/// **0x59BA70**, the x87 mix of one sample: `dst = src · gain + dst`, an f32 store.
pub fn mix_into(dst: f32, src: f32, gain: f32) -> f32 {
    (src as f64 * gain as f64 + dst as f64) as f32
}

/// How many samples a gain change ramps over: 0x598760 hands a voice whose gains were set since
/// the last block (`+1`, 0x598721) to 0x5980C0 for the block's first `min(n, 32)` samples
/// (0x5980C9).
pub const RAMP_SAMPLES: usize = 32;

/// A voice channel's two gains: `+0x14…` the current, `+0x24…` the target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gains {
    pub cur: [f32; 2],
    pub target: [f32; 2],
    /// set by a new target, cleared when a block has ramped to it
    pub pending: bool,
    started: bool,
}

impl Gains {
    /// 0x598700 with the new target. The voice's first one is taken at once — the start 0x598630
    /// copies the targets to the current gains with the flag clear (0x598652…0x598661).
    pub fn set(&mut self, target: [f32; 2]) {
        if !self.started {
            self.started = true;
            self.cur = target;
            self.target = target;
            return;
        }
        if target != self.target {
            self.target = target;
            self.pending = true;
        }
    }

    /// The block's ramp, **0x5980C0**: over `len = min(n, 32)` samples each channel steps by
    /// `(target − cur) / len` (an f32, `fdiv` by `len` as an f32), `cur += step` before each
    /// sample, which is mixed at that gain (0x598177…0x5981A5); then `cur = target`
    /// (0x5981F8). `None` when no ramp is due.
    pub fn ramp(&mut self, n: usize) -> Option<Ramp> {
        if !self.pending {
            return None;
        }
        self.pending = false;
        let len = n.min(RAMP_SAMPLES);
        let step = [0, 1].map(|c| ((self.target[c] as f64 - self.cur[c] as f64) / len as f32 as f64) as f32);
        Some(Ramp { len, step })
    }

    /// The gain of channel `c` for the ramp's next sample.
    pub fn ramp_next(&mut self, r: &Ramp, c: usize) -> f32 {
        self.cur[c] = (r.step[c] as f64 + self.cur[c] as f64) as f32;
        self.cur[c]
    }

    /// The ramp's end: the current gains are the targets.
    pub fn ramp_done(&mut self) {
        self.cur = self.target;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ramp {
    pub len: usize,
    pub step: [f32; 2],
}

/// **0x59A350**, the store of the float mix as 16 bits: `x + 12582912.0` (`[0x5C1800]`, 1.5·2²³)
/// as an f32 — the round to nearest — and the low 20 bits of its mantissa `m`: `m ≤ 0x7FFF` or
/// `m ≥ 0xF8000` is the sample (the word), else `m < 0x80000` → 0x7FFF and the rest → −0x8000
/// (`cmp ; sbb ; and 0xFFFF ; add 0xFFFF8000`). So a sum past ±2¹⁹ wraps before it saturates.
pub fn to_i16(x: f32) -> i16 {
    let m = (x + 12_582_912.0f32).to_bits() & 0xF_FFFF;
    if m <= 0x7FFF || m >= 0xF_8000 {
        m as u16 as i16
    } else if m < 0x8_0000 {
        0x7FFF
    } else {
        -0x8000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_step_is_rate_times_sixteen_over_the_mix_rate_times_the_pitch_word() {
        // 22050 Hz at the 22050 mix rate, pitch 4096: exactly 1.0
        assert_eq!(step_16_16(4096, 22050, 22050), 0x10000);
        // 22050 at 16000: 22.05 in f32 = 22.049999237…, · 4096 = 90316.797 → 90316 (truncated)
        assert_eq!(step_16_16(4096, 22050, 16000), 90316);
        // 11025 at 32000: 5.5125 · 4096 = 22579.2 → 22579
        assert_eq!(step_16_16(4096, 11025, 32000), 22579);
        // capped at 4.0
        assert_eq!(step_16_16(0xFFFF, 44100, 11025), STEP_MAX);
        assert_eq!(PITCH_UNIT, 4096);
    }

    #[test]
    fn the_interpolator_follows_the_quality_byte() {
        assert_eq!([0, 1, 2, 3].map(Interp::for_quality), [Interp::Cubic, Interp::Cubic, Interp::Linear, Interp::Linear]);
    }

    #[test]
    fn the_cubic_is_catmull_rom_with_the_short_t_unit() {
        // at t = 0 it is the sample itself
        assert_eq!(cubic(100.0, 200.0, -300.0, 50.0, 0), 200.0);
        // a straight line stays straight (a = b = 0 for evenly spaced points … up to the t unit)
        let t = 0x8000 as f64 * CUBIC_T as f64;
        let got = cubic(0.0, 1000.0, 2000.0, 3000.0, 0x8000);
        assert_eq!(got, (1000.0 + 1000.0 * t) as f32);
        // a known sample, by hand: s = [-1000, 0, 1000, 0], frac 0x4000
        // a = (3·(0 − 1000) + 1000 + 0)/2 = −1000, b = (2000 − 1000) − 0 = 1000, c = 2000/2 = 1000
        let t = 0x4000 as f64 * CUBIC_T as f64;
        let t2 = t * t;
        let want = (1000.0 * t + (1000.0 * t2 + -1000.0 * (t2 * t))) as f32;
        assert_eq!(cubic(-1000.0, 0.0, 1000.0, 0.0, 0x4000), want);
        assert!((want - 296.875).abs() < 0.1, "{want}");
        // the t unit is a hair short of 1/65536: t(0x8000) is just under a half
        let half = 0x8000 as f64 * CUBIC_T as f64;
        assert!(half < 0.5 && half > 0.4999);
    }

    #[test]
    fn the_linear_is_exact_in_sixteenths_of_the_step() {
        assert_eq!(linear(100.0, 200.0, 0), 100.0);
        assert_eq!(linear(100.0, 200.0, 0x8000), 150.0);
        assert_eq!(linear(-32768.0, 32767.0, 0xC000), (-32768.0 + 65535.0 * 0.75) as f32);
    }

    #[test]
    fn the_cursor_carries_the_fraction() {
        let mut c = Cursor::default();
        c.advance(0x18000);
        assert_eq!(c, Cursor { pos: 1, frac: 0x8000 });
        c.advance(0x18000);
        assert_eq!(c, Cursor { pos: 3, frac: 0 });
    }

    #[test]
    fn a_gain_below_five_thousandths_is_zero() {
        assert_eq!(channel_gain(0.1, 0.04), 0.0);
        assert_eq!(channel_gain(0.1, 0.06), (0.1f64 * 0.06f32 as f64) as f32);
    }

    #[test]
    fn a_gain_change_ramps_over_the_first_32_samples() {
        let mut g = Gains::default();
        g.set([1.0, 0.5]);
        assert_eq!((g.cur, g.pending), ([1.0, 0.5], false));
        assert!(g.ramp(256).is_none());
        g.set([0.0, 0.5]);
        let r = g.ramp(256).unwrap();
        assert_eq!((r.len, r.step), (32, [-1.0 / 32.0, 0.0]));
        let first = g.ramp_next(&r, 0);
        assert_eq!(first, 1.0 - 1.0 / 32.0);
        g.ramp_done();
        assert_eq!(g.cur, [0.0, 0.5]);
        // a short block ramps over the block
        g.set([1.0, 0.5]);
        assert_eq!(g.ramp(10).unwrap().len, 10);
    }

    #[test]
    fn the_store_rounds_to_nearest_and_saturates_then_wraps() {
        assert_eq!(to_i16(0.0), 0);
        assert_eq!(to_i16(1.5), 2);
        assert_eq!(to_i16(2.5), 2);
        assert_eq!(to_i16(-1.5), -2);
        assert_eq!(to_i16(32767.4), 32767);
        assert_eq!(to_i16(40000.0), 32767);
        assert_eq!(to_i16(-32768.0), -32768);
        assert_eq!(to_i16(-40000.0), -32768);
        // past 2^19 the 20-bit field wraps: 600000 reads as a negative overflow
        assert_eq!(to_i16(600_000.0), -32768);
        assert_eq!(to_i16(-600_000.0), 32767);
    }
}
