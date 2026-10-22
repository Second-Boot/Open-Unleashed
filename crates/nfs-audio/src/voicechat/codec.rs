//! The sound library's **8 kHz voice codec** (format code `0x40`, `RE/notes/voice-codec.md`): an
//! LPC-10-style vocoder, 160 samples (20 ms) in, one 14-byte packet out, and back.
//!
//! * **Encoder** `0x597660` (state `[0x6B17DC]`, 0x3EA8 bytes, built by `0x597560`): the 160 new
//!   samples go through a one-pole + two-pole pre-filter for the pitch path; the 240-sample frame
//!   (80 old + 160 new) is Hamming-windowed (`0x597560`'s table), autocorrelated to lag 10
//!   (`0x597120`) and Levinson-Durbin solved (`0x597180`) into 10 reflection coefficients and an
//!   RMS gain; the pitch (`0x597360`) is a 4th-order whitened, 5:1 decimated autocorrelation peak
//!   with parabolic interpolation and a two-frame voicing hysteresis (`[0x6AF164]`).
//! * **Packet** (`0x597770…0x5977B6`): `u16` pitch period · 256 (0 = unvoiced), `u8` gain · 256,
//!   ten `i8` reflection coefficients · 128, every value through `_ftol` 0x5A0F98 (truncation,
//!   low bits kept). Byte 13 is never written: the capture ring's slot 0x5619D1 keeps whatever the
//!   allocator left there (`VoiceRecBuff`, 0x4933AF), and the decoder never reads it.
//! * **Decoder** `0x597810` (state = the stream codec object `+0x10`, 0xC8 bytes, zeroed by the
//!   object's init 0x59B830): pulse train (voiced) or `rand()` noise (unvoiced) through a 10-stage
//!   lattice, the pitch / amplitude / coefficients interpolated linearly over the frame from the
//!   previous packet's values, clipped to ±1 and scaled by 32767 into float samples.
//!
//! Arithmetic: x87 with the control word `0x027F` (53-bit precision, measured on the running
//! original, STATUS "x87 precision control"); every `fstp dword` is an `f32` store, every value
//! kept on the FPU stack an `f64`, so the code below carries `f32` storage and `f64` expressions
//! exactly where the original does. Verified bit for bit against the original's machine code run
//! under an x87 emulator (tests `the_encoder_matches_the_original` / `the_decoder_matches_the_original`).

/// Samples per packet (`0x597675` `mov esi, 0xA0`; `0x59799A`).
pub const FRAME: usize = 160;
/// Bytes per packet (`0x58AD52` returns 0xE).
pub const PACKET: usize = 14;
/// The frame the encoder analyses: 80 samples kept from the previous call + the 160 new ones
/// (`0x5977BD…0x5977DD` keep the last 80).
const WIN: usize = 240;
const KEEP: usize = 80;
/// LPC order (`0x59774A` / `0x597765` `push 0xA`).
pub const ORDER: usize = 10;

/// `_ftol` 0x5A0F98: `fistp qword` under round-toward-zero; out of range or NaN is the integer
/// indefinite `0x8000_0000_0000_0000`. Callers keep the low bits (`mov word`, `mov byte`).
fn ftol(x: f64) -> i64 {
    if x.is_nan() || x >= 9.223_372_036_854_775_808e18 || x < -9.223_372_036_854_775_808e18 {
        i64::MIN
    } else {
        x.trunc() as i64
    }
}

/// `f32(acc + a·b)` the way `fld a ; fmul b ; fadd acc ; fstp acc` rounds it at 53 bits: the
/// product of two `f32` is exact in `f64`, the sum rounds once to `f64`, the store to `f32`.
#[inline]
fn mac(acc: f32, a: f32, b: f32) -> f32 {
    (a as f64 * b as f64 + acc as f64) as f32
}

/// `0x597120(x, n, p, r)`: `r[k] = Σ_{i < n−k} x[i]·x[i+k]` for `k = 0..=p`, accumulated in the
/// `f32` output word (`fld ; fmul ; fadd [r] ; fstp [r]`, 0x597152…0x59715F).
fn autocorr(x: &[f32], p: usize, r: &mut [f32]) {
    let n = x.len();
    for k in 0..=p {
        let mut acc = 0.0f32;
        for i in 0..n.saturating_sub(k) {
            acc = mac(acc, x[i], x[i + k]);
        }
        r[k] = acc;
    }
}

/// `0x597180(r, p, k, gain)`: Levinson-Durbin. `k[1..=p]` gets the reflection coefficients
/// (`f32`), the return value is `sqrt(E)` with the prediction error `E` kept on the FPU stack
/// (`f64`) throughout. When `E` is 0 (or NaN, `fcom` 0x597218 `test ah, 0x40`) after an order's
/// sum, the function stops there: that `k[i]` keeps the undivided sum, the higher ones are not
/// written (the caller's stack then holds whatever was there), the gain is 0 (0x597289).
fn levinson(r: &[f32], p: usize, k: &mut [f32]) -> f32 {
    let mut a = [0.0f32; ORDER + 1]; // [esp+0x40]
    let mut b = [0.0f32; ORDER + 1]; // [esp+0x14]
    let mut e = r[0] as f64;
    for i in 1..=p {
        let mut acc = -r[i];
        for j in 1..i {
            acc = (acc as f64 - r[i - j] as f64 * a[j] as f64) as f32;
            b[j] = a[j];
        }
        k[i] = acc;
        if e == 0.0 || e.is_nan() {
            return 0.0;
        }
        let kk = acc as f64 / e;
        let kf = kk as f32;
        k[i] = kf;
        a[i] = kf;
        for j in 1..i {
            a[j] = (kf as f64 * b[i - j] as f64 + b[j] as f64) as f32;
        }
        e *= 1.0 - kk * kk;
    }
    // 0x59727C: `fcom 0.0`, C0 (below or unordered) → 0, then `fsqrt`.
    if !(e >= 0.0) {
        e = 0.0;
    }
    e.sqrt() as f32
}

/// `0x5972B0(d, k)`: the 4-stage lattice analysis (whitening) filter over the 48 decimated
/// samples, in place. `f[m] = f[m−1] + k[m]·g'[m−1]`, `g[m] = g'[m−1] + k[m]·f[m−1]` with `g'` the
/// previous sample's `g` (`[esp+0x28…]`, zeroed on entry).
fn lattice4(d: &mut [f32; 48], k: &[f32; 5]) {
    let mut gold = [0.0f32; 4];
    let mut f = [0.0f32; 5];
    let mut g = [0.0f32; 5];
    for x in d.iter_mut() {
        f[0] = *x;
        g[0] = *x;
        for m in 1..=4 {
            f[m] = mac(f[m - 1], k[m], gold[m - 1]);
            g[m] = mac(gold[m - 1], k[m], f[m - 1]);
            gold[m - 1] = g[m - 1];
        }
        *x = f[4];
    }
}

/// `[0x5B2DA8]` 0.4 and `[0x5B49A0]` 0.3: the voicing thresholds of the normalised peak.
const VOICED: f64 = 0.4;
const VOICED_HELD: f64 = 0.3;

/// The encoder's state (`[0x6B17DC]`) plus the voicing history `[0x6AF164]` (a global of the
/// original, one per process; one encoder per process uses it).
#[derive(Clone)]
pub struct Encoder {
    /// `+0x0000`: the input as `x / 32768`, the 80 kept + 160 new.
    raw: [f32; WIN],
    /// `+0x0FA0`: the pre-filtered input, same layout (the pitch path).
    filt: [f32; WIN],
    /// `+0x1F40`: the Hamming window, `0x597560`.
    window: [f32; WIN],
    /// `+0x3E84…0x3E94`: the pre-filter's coefficients.
    c: [f32; 5],
    /// `+0x3E9C`, `+0x3EA0`, `+0x3EA4`: the pre-filter's memory.
    s: [f32; 3],
    /// `[0x6AF164]`: bit 0 = this frame voiced, bit 1 = the previous one.
    voicing: u32,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// `[0x5C17B4]` 2π/239, `[0x5C17B0]` 0.46, `[0x5C17AC]` 0.54, `[0x5C17A8]` 1.5863 (f32).
const W_STEP: f32 = f32::from_bits(0x3CD7_5D09);
const W_046: f32 = f32::from_bits(0x3EEB_851F);
const W_054: f32 = f32::from_bits(0x3F0A_3D71);
const W_SCALE: f32 = f32::from_bits(0x3FCB_0BE1);
/// `[0x5C17A0]`, `[0x5C1790]`, `[0x5C1798]` (f64): the pre-filter's two pole radii (as natural
/// logarithms) and the second pole's angle.
const P1_LN: f64 = f64::from_bits(0xBFC6_408E_8DCD_9800);
const P2_LN: f64 = f64::from_bits(0xBFB6_408E_8DCD_9800);
const P2_ANGLE: f64 = f64::from_bits(0x3FDB_D6DF_AB91_2A00);

impl Encoder {
    /// `0x597560` (on the first `0x58ACA0` with code 0x40): the window table and the pre-filter
    /// coefficients; the filter memory (`+0x3E9C…`) is zeroed, the sample history is not touched by
    /// the init (a fresh allocation, `0x58AAE0`; zero here).
    pub fn new() -> Self {
        let mut window = [0.0f32; WIN];
        for (i, w) in window.iter_mut().enumerate() {
            // fild i ; fmul [2π/239] ; fcos ; fmul [0.46] ; fsubr [0.54] ; fmul [1.5863] ; fstp
            let x = i as f64 * W_STEP as f64;
            let v = (W_054 as f64 - libm::cos(x) * W_046 as f64) * W_SCALE as f64;
            *w = v as f32;
        }
        // 0x5975BE…0x597636 (exp by f2xm1 / fscale).
        let e1 = libm::exp(P1_LN);
        let e2 = libm::exp(P2_LN);
        let c8c = (e2 * libm::cos(P2_ANGLE) * -2.0) as f32;
        let c = [
            (-e1) as f32,                            // +0x3E84
            (-e1 + 1.0) as f32,                      // +0x3E88
            c8c,                                     // +0x3E8C
            e1 as f32,                               // +0x3E90
            ((e1 + c8c as f64) + 1.0) as f32,        // +0x3E94
        ];
        Encoder { raw: [0.0; WIN], filt: [0.0; WIN], window, c, s: [0.0; 3], voicing: 0 }
    }

    /// The window table (`+0x1F40`), for the tests.
    pub fn window(&self) -> &[f32; WIN] {
        &self.window
    }

    /// The pre-filter coefficients (`+0x3E84…+0x3E94`), for the tests.
    pub fn coefficients(&self) -> [f32; 5] {
        self.c
    }

    /// `0x597660`: one packet from 160 samples. Byte 13 is left as `out` had it.
    pub fn encode_into(&mut self, pcm: &[i16; FRAME], out: &mut [u8; PACKET]) {
        let [c84, c88, c8c, c90, c94] = self.c.map(|v| v as f64);
        for (n, &x) in pcm.iter().enumerate() {
            // movsx ; fild ; fmul [2^-15] ; fstp → exact
            let x = (x as f64 * f32::from_bits(0x3800_0000) as f64) as f32;
            self.raw[KEEP + n] = x;
            let t = (x as f64 * c88 - c84 * self.s[0] as f64) as f32;
            let y = ((c94 * self.s[0] as f64 - c8c * self.s[1] as f64) - c90 * self.s[2] as f64) as f32;
            self.filt[KEEP + n] = y;
            self.s[0] = t;
            self.s[2] = self.s[1];
            self.s[1] = y;
        }
        let pitch = self.pitch();
        let mut windowed = [0.0f32; WIN]; // +0x2EE0
        for i in 0..WIN {
            windowed[i] = self.raw[i] * self.window[i];
        }
        let mut r = [0.0f32; ORDER + 1];
        autocorr(&windowed, ORDER, &mut r);
        let mut k = [0.0f32; ORDER + 1];
        let gain = levinson(&r, ORDER, &mut k);
        let p = ftol(pitch as f64 * 256.0) as u16;
        out[0..2].copy_from_slice(&p.to_le_bytes());
        out[2] = ftol(gain as f64 * 256.0) as u8;
        for i in 0..ORDER {
            out[3 + i] = ftol(k[1 + i] as f64 * 128.0) as u8;
        }
        self.raw.copy_within(WIN - KEEP.., 0);
        self.filt.copy_within(WIN - KEEP.., 0);
    }

    /// [`Encoder::encode_into`] into a packet whose byte 13 is 0 (the port's choice for the
    /// allocator's leftover byte; no reader looks at it).
    pub fn encode(&mut self, pcm: &[i16; FRAME]) -> [u8; PACKET] {
        let mut out = [0u8; PACKET];
        self.encode_into(pcm, &mut out);
        out
    }

    /// `0x597360(filt)`: the pitch period in samples, 0 for an unvoiced frame.
    fn pitch(&mut self) -> f32 {
        let mut d = [0.0f32; 48];
        for (i, v) in d.iter_mut().enumerate() {
            *v = self.filt[5 * i];
        }
        // R4 and Rp share the stack slots S+0x20…; K4 sits just below them (S+0x10…S+0x1C).
        let mut r4 = [0.0f32; 5];
        autocorr(&d, 4, &mut r4);
        let mut k4 = [0.0f32; 5];
        levinson(&r4, 4, &mut k4);
        lattice4(&mut d, &k4);
        // 0x5973CE `push 0x21`: lags 0..=33 into the 33 slots S+0x20…S+0xA0 **and** S+0xA4, which
        // is D[0] itself: 0x597143 zeroes it before the lag-33 sum, so that sum's `i = 0` term is
        // `0 · D[33]` (the accumulator read as the sample). Rp[33] is only read as Rp[L+1] for L = 32.
        let mut rp = [0.0f32; 34];
        autocorr(&d, 32, &mut rp);
        let mut r33 = 0.0f32;
        for i in 1..48 - 33 {
            r33 = mac(r33, d[i], d[i + 33]);
        }
        rp[33] = r33;
        // 0x5973D8…0x597406: the largest positive Rp[5..=32] (strictly greater wins).
        let mut max = 0.0f64;
        let mut l = 0usize;
        for (c, &v) in rp.iter().enumerate().skip(5) {
            if !(max >= v as f64) {
                max = v as f64;
                l = c;
            }
        }
        // `[esp+L·4+0x20]` / `[esp+L·4+0x28]` (esp = S−4): Rp[L−1] and Rp[L+1]; for L = 0 (no
        // positive peak) the slot below Rp[0] is the last K4 (S+0x1C).
        let a = if l == 0 { k4[4] } else { rp[l - 1] } as f64;
        let b = rp[l + 1] as f64;
        let c = max;
        let lf = l as f64;
        // 0x597408…0x5974D4: the parabola through (L−1, a), (L, c), (L+1, b).
        let a2 = (a * 0.5 - c) + b * 0.5;
        let x = (1.0 - (lf + lf)) * b * 0.5 - ((lf + lf) + 1.0) * a * 0.5;
        let lc = lf * c;
        let bf = (x + (lc + lc)) as f32;
        let lp = (-(bf as f64 / (a2 + a2))) as f32;
        let y = ((l as i32 - 1) * l as i32) as f64 * b + ((l as i32 + 1) * l as i32) as f64 * a;
        let c0 = y * 0.5 + (1.0 - (l * l) as f64) * c;
        let peak = c0 + (lp as f64 * a2 + bf as f64) * lp as f64;
        let period = (lp as f64 * 5.0) as f32;
        let ratio = peak / rp[0] as f64;
        // 0x5974DC…0x597551 (C0 = below or unordered).
        let s = self.voicing;
        let voiced = !(ratio < VOICED) || (s == 3 && !(ratio < VOICED_HELD));
        if voiced {
            self.voicing = (s & 1) * 2 + 1;
            period
        } else {
            self.voicing = (s & 1) << 1;
            0.0
        }
    }
}

/// `[0x5C17B8]` 1/32767: the noise scale.
const NOISE_SCALE: f64 = f64::from_bits(0x3F00_0020_0040_0080);
/// `[0x5C17C0]` 1/160 (as the nearest double): the per-sample interpolation step.
const STEP: f64 = f64::from_bits(0x3F79_9999_9999_999A);

/// MSVC `rand()` 0x5A17C9 (`holdrand · 0x343FD + 0x269EC3`, bits 16…30). The original's word is
/// the mixer thread's own CRT `holdrand` (`_getptd()+0x14`), shared with whatever else draws on
/// that thread. APPROXIMATION: the port gives every decoder its own word starting at the CRT's
/// per-thread start value 1; only the phase of the unvoiced noise depends on it.
#[derive(Clone, Debug)]
pub struct CrtRand(pub u32);

impl CrtRand {
    pub fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(0x343FD).wrapping_add(0x269EC3);
        (self.0 >> 16) & 0x7FFF
    }
}

/// The decoder's state: the stream codec object's `+0x10…+0xD8` (0xC8 bytes, zeroed by its init
/// 0x59B830 when the stream is opened).
#[derive(Clone, Debug)]
pub struct Decoder {
    /// `+0x10`: the previous packet's pitch period (samples), 0 = unvoiced.
    pitch: f64,
    /// `+0x18`: the previous packet's excitation amplitude.
    amp: f64,
    /// `+0x28`: the previous packet's reflection coefficients.
    k: [f64; ORDER],
    /// `+0x78`: the last output sample before the clip.
    y: f64,
    /// `+0x80`: the lattice's backward memory.
    b: [f64; ORDER],
    /// `+0xD0`: samples left to the next pitch pulse.
    count: i32,
    pub rand: CrtRand,
}

impl Default for Decoder {
    fn default() -> Self {
        Decoder { pitch: 0.0, amp: 0.0, k: [0.0; ORDER], y: 0.0, b: [0.0; ORDER], count: 0, rand: CrtRand(1) }
    }
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// `0x597810(packet, out, state)`: 160 float samples in ±32767 from one packet.
    pub fn decode(&mut self, pkt: &[u8], out: &mut [f32; FRAME]) {
        let p = u16::from_le_bytes([pkt[0], pkt[1]]) as f64 * (1.0 / 256.0);
        let g = pkt[2] as f64 * (1.0 / 256.0);
        let mut knew = [0.0f64; ORDER];
        for (i, k) in knew.iter_mut().enumerate() {
            *k = pkt[3 + i] as i8 as f64 * (1.0 / 128.0);
        }
        // 0x597881…0x5978DF: a = g / sqrt(240 / p), p = 3 for an unvoiced packet.
        let pc = if p == 0.0 { 3.0 } else { p };
        let amp = g / (240.0 / pc).sqrt();
        let mut cur_p = self.pitch;
        let mut cur_a = self.amp;
        let mut k = self.k;
        let (dp, da, dk) = if self.pitch == 0.0 || p == 0.0 {
            (0.0, 0.0, [0.0; ORDER])
        } else {
            let mut dk = [0.0; ORDER];
            for i in 0..ORDER {
                dk[i] = (knew[i] - self.k[i]) * STEP;
            }
            ((p - self.pitch) * STEP, (amp - self.amp) * STEP, dk)
        };
        if cur_p == 0.0 {
            self.count = 0;
        }
        for o in out.iter_mut() {
            let mut e = if cur_p == 0.0 {
                self.rand.next() as f64 * NOISE_SCALE * cur_a
            } else if self.count == 0 {
                self.count = ftol(cur_p) as i32;
                cur_a
            } else {
                self.count -= 1;
                0.0
            };
            // 0x5979EF…0x597A29: the lattice, stage 9 down to 0; stage 0's backward input is
            // the previous output sample.
            for i in (0..ORDER).rev() {
                let bprev = if i == 0 { self.y } else { self.b[i - 1] };
                let ki = k[i];
                k[i] = ki + dk[i];
                e -= ki * bprev;
                self.b[i] = ki * e + bprev;
            }
            self.y = e;
            let yc = if e > 1.0 {
                1.0
            } else if !(e >= -1.0) {
                -1.0
            } else {
                e
            };
            *o = (yc * f32::from_bits(0x46FF_FE00) as f64) as f32;
            cur_p += dp;
            cur_a += da;
        }
        self.pitch = p;
        self.amp = amp;
        self.k = knew;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FNV-1a 64.
    fn fnv(b: &[u8]) -> u64 {
        b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x0100_0000_01b3))
    }

    /// TEST GLUE: a deterministic test signal — ten 40-frame segments of noise, resonator-filtered
    /// pulse trains of several periods, loud noise and a ±2 dither (no window is all zeros).
    fn signal() -> Vec<i16> {
        let mut s: u32 = 0x0123_4567;
        let mut rnd = move || {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((s >> 16) & 0x7FFF) as i32
        };
        let segs: [(u8, i32); 10] =
            [(0, 2000), (1, 57), (1, 40), (1, 90), (1, 23), (0, 30000), (1, 130), (2, 0), (1, 61), (1, 33)];
        let (mut y1, mut y2, mut ph) = (0i32, 0i32, 0i32);
        let mut out = Vec::new();
        for (kind, arg) in segs {
            for _ in 0..40 * FRAME {
                let d = rnd() % 5 - 2;
                let x = match kind {
                    0 => ((rnd() - 16384) * arg).div_euclid(16384), // Python floors
                    1 => {
                        ph += 1;
                        if ph >= arg {
                            ph = 0;
                            12000
                        } else {
                            0
                        }
                    }
                    _ => 0,
                };
                // Python's `>>` floors like Rust's on i32.
                let y = (x + ((3 * y1) >> 1) - ((3 * y2) >> 2)).clamp(-32768, 32767);
                y2 = y1;
                y1 = y;
                let v = if kind == 1 { y + d } else { x + d };
                out.push(v.clamp(-32768, 32767) as i16);
            }
        }
        out
    }

    fn bits(v: &[f32]) -> Vec<u8> {
        // The one difference to the original: inside the lattice the x87 registers keep values
        // below the smallest double (a long decay), where `f64` underflows — the output then
        // differs in the sign of a zero only. Both are compared as +0.
        v.iter().flat_map(|x| (if *x == 0.0 { 0 } else { x.to_bits() }).to_le_bytes()).collect()
    }

    #[test]
    fn the_window_and_the_prefilter_match_the_original() {
        let e = Encoder::new();
        let h = e.window().iter().fold(0u64, |h, v| (h ^ v.to_bits() as u64).wrapping_mul(0x0100_0000_01b3));
        assert_eq!(h, 0x13ec_9317_14a3_589c);
        assert_eq!(e.window()[0].to_bits(), 0x3E01_F321);
        assert_eq!(e.coefficients().map(f32::to_bits), [0xBF57_263A, 0x3E23_6717, 0xBFD4_D503, 0x3F57_263A, 0x3E35_F0D1]);
    }

    /// The digests are of the original's own `0x597660` / `0x597810` run on the same input under an
    /// x87 emulator (unicorn, control word 0x027F; `RE/notes/voice-codec.md` §5).
    #[test]
    fn the_encoder_matches_the_original() {
        let pcm = signal();
        let mut enc = Encoder::new();
        let mut pk = Vec::new();
        for f in pcm.chunks_exact(FRAME) {
            pk.extend_from_slice(&enc.encode(f.try_into().unwrap()));
        }
        assert_eq!(pk.len(), 400 * PACKET);
        assert_eq!(hex(&pk[40 * PACKET..41 * PACKET]), "2c38b7954f1506f9000103fa0700");
        assert_eq!(hex(&pk[240 * PACKET..241 * PACKET]), "000015ccf40a4800fef4fe05fc00");
        assert_eq!(fnv(&pk), 0xb5a9_2990_a848_be4b);
        let mut dec = Decoder::new();
        let mut out = Vec::new();
        for p in pk.chunks_exact(PACKET) {
            let mut o = [0.0f32; FRAME];
            dec.decode(p, &mut o);
            out.extend_from_slice(&o);
        }
        assert_eq!(fnv(&bits(&out)), 0x976a_ef04_d200_f03f);
    }

    #[test]
    fn the_decoder_matches_the_original() {
        let mut s: u32 = 0x2545_F491;
        let mut rnd = move || {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            ((s >> 16) & 0xFF) as u8
        };
        let mut dec = Decoder::new();
        let (mut pk, mut out) = (Vec::new(), Vec::new());
        for i in 0..600 {
            let mut p = [0u8; PACKET];
            p.iter_mut().for_each(|b| *b = rnd());
            match i % 3 {
                0 => (p[0], p[1]) = (0, 0),
                1 => p[1] &= 1,
                _ => {}
            }
            pk.extend_from_slice(&p);
            let mut o = [0.0f32; FRAME];
            dec.decode(&p, &mut o);
            out.extend_from_slice(&o);
        }
        assert_eq!(fnv(&pk), 0xa2f7_da60_2915_6b1c);
        assert_eq!(fnv(&bits(&out)), 0x80cf_4806_81a4_53a6);
    }

    #[test]
    fn silence_is_an_unvoiced_packet_of_gain_zero() {
        let mut enc = Encoder::new();
        let p = enc.encode(&[0; FRAME]);
        assert_eq!(p, [0; PACKET]);
        let mut o = [1.0f32; FRAME];
        Decoder::new().decode(&p, &mut o);
        assert!(o.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn ftol_truncates_and_gives_the_indefinite_integer_out_of_range() {
        assert_eq!(ftol(2.9), 2);
        assert_eq!(ftol(-2.9), -2);
        assert_eq!(ftol(f64::NAN), i64::MIN);
        assert_eq!(ftol(1e30) as u16, 0);
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }
}
