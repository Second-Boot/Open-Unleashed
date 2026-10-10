//! The mixer's reverb tail: four all-passes over the summed sends (0x598A70…0x598BC8).

use super::*;

/// The reverb entries of the driver's default table (0x5E25B0, the only one the game selects:
/// 0x4AB470 → 0x565C70 with mode 0x28 → 0x5901B0 takes `0x5E25B0` for any mode but 0 and 1, and
/// mode 1's table `[0x6B4A10]` is never given): the three group counts `+4…+6` are 4 / 0 / 0, so
/// the network is four all-pass filters in series on one buffer (0x598AE0's group 0; the groups 1
/// and 2 with their extra buffers are empty), the gain byte `+7` is 0xFA, and the entries from `+8`
/// are `(delay word BE, coefficient c, damping d)`, delay in ms.
pub(super) const REVERB_ENTRIES: [(i32, i32, i32); 4] = [(100, 0x40, 0), (70, 0x40, 95), (49, 0x4C, 0), (34, 0x4C, 95)];

pub(super) const REVERB_GAIN_BYTE: i32 = 0xFA;

/// The smallest prime not below `n` — 0x598CD0's search (the smallest divisor ≥ 2 of `n` equals
/// `n`, else `n + 1`, and again).
pub(super) fn next_prime(mut n: i64) -> i64 {
    loop {
        let mut d = 2;
        if n >= 2 {
            while d <= n && n % d != 0 {
                d += 1;
            }
        }
        if d == n {
            return n;
        }
        n += 1;
    }
}

/// One all-pass with a damped feedback, 0x598D10's state (0x24 bytes) and 0x59BD20's loop.
pub(super) struct AllPass {
    pub(super) buf: Vec<f32>,
    /// `+8`, the delay in samples
    pub(super) len: usize,
    /// `+0` and `+4`, the write and the read counter, both counting down through the buffer: they
    /// start at 0 and `len` and the first pass resets the write one to `+8` (0x598B1E), so they
    /// stay equal — one counter
    pub(super) left: usize,
    /// `+0x10` / `+0x14`, `c` and `−c`
    pub(super) c: f32,
    pub(super) nc: f32,
    /// `+0x1C` / `+0x20`, the feedback's damping and its complement
    pub(super) g1: f32,
    pub(super) g2: f32,
    /// `+0x18`, the damping's state
    pub(super) state: f32,
}

impl AllPass {
    /// 0x598D10 for one entry. The x87 intermediates are f64 (`career.md` §11.4c: the control word
    /// read 0x027F, 53-bit precision).
    pub(super) fn new(rate: u32, (ms, c, d): (i32, i32, i32)) -> Self {
        // 0x598D62…0x598DBB: `d · [0x5B4A78]` (an f32 1/127) stored, `1 − ` it (the unrounded
        // product), each times `(byte+7 + 100)` and `[0x5B73F4]` (an f32 1/350), stored
        let x = d as f64 * (1.0f32 / 127.0) as f64;
        let g1t = x as f32;
        let g2t = (1.0 - x) as f32;
        let k = f32::from_bits(0x3B3B_3EE7) as f64;
        let g1 = ((REVERB_GAIN_BYTE + 100) as f64 * g1t as f64 * k) as f32;
        let g2 = ((REVERB_GAIN_BYTE + 100) as f64 * g2t as f64 * k) as f32;
        let cf = (c as f64 * (1.0f32 / 127.0) as f64) as f32;
        // 0x598DD3…0x598DED: the delay word `· [0x5B3060]` (an f32 0.001) stored, `rate ·` it
        // truncated (0x5A0F98), then the next prime (0x598CD0)
        let secs = (ms as f64 * f32::from_bits(0x3A83_126F) as f64) as f32;
        let len = next_prime((rate as f64 * secs as f64).trunc() as i64) as usize;
        Self { buf: vec![0.0; len], len, left: 0, c: cf, nc: -cf, g1, g2, state: 0.0 }
    }

    /// 0x598AE0's chunk loop over `x`, in place, and 0x59BD20's per-sample body.
    pub(super) fn run(&mut self, x: &mut [f32]) {
        let mut i = 0;
        while i < x.len() {
            if self.left < 1 {
                self.left = self.len;
            }
            let m = (x.len() - i).min(self.left);
            for j in 0..m {
                let p = self.left - 1 - j;
                let xin = x[i + j] as f64;
                let a = xin * self.nc as f64;
                let b = self.state as f64 * self.g1 as f64;
                let y = a + self.buf[p] as f64;
                x[i + j] = y as f32;
                let z = y * self.c as f64 + xin;
                let w = (z * self.g2 as f64 + b) as f32;
                self.buf[p] = w;
                self.state = w;
            }
            self.left -= m;
            i += m;
        }
    }
}

/// The library's software reverb — the DSP the driver runs itself (0x5901B0 → 0x598D10 sets it up,
/// 0x598A90 takes each voice's send, 0x598AE0 runs it after the voices of a block). Every voice's
/// pulled block times its wet gain is summed into one input buffer (`0x6B0A80`, 0x598A90); once a
/// block, the four all-passes run over it in place, and the result is added — gain 1.0, the same
/// mono signal — to both output channels (0x598B9B…0x598BC8). It runs for the 2000 blocks after
/// the last send (`[0x6B1680]`, set to 2000 at the setup 0x598A70, reset to 0 by a send, +1 per
/// block before the test, 0x598AE5 / 0x598AF2), so the tail rings out however the buses move.
///
/// *(Named `ReverbTail` so as not to be taken for [`crate::reverb::Reverb`], the tunnel-level state.)*
pub(super) struct ReverbTail {
    pub(super) aps: [AllPass; 4],
    pub(super) input: [f32; BLOCK],
    pub(super) quiet: u32,
}

impl ReverbTail {
    pub(super) fn new(rate: u32) -> Self {
        Self { aps: REVERB_ENTRIES.map(|e| AllPass::new(rate, e)), input: [0.0; BLOCK], quiet: 2000 }
    }

    /// 0x598A90 for one frame of a voice: `input[at] += x · wet`.
    pub(super) fn send(&mut self, at: usize, x: f32, wet: f32) {
        self.input[at] = resample::mix_into(self.input[at], x, wet);
        self.quiet = 0;
    }

    /// 0x598AE0 for a block of `out.len()` frames.
    pub(super) fn process(&mut self, out: &mut [[f32; 2]]) {
        let n = out.len();
        self.quiet += 1;
        if self.quiet >= 2000 {
            return;
        }
        for ap in self.aps.iter_mut() {
            ap.run(&mut self.input[..n]);
        }
        for (frame, r) in out.iter_mut().zip(self.input.iter()) {
            for o in frame.iter_mut() {
                *o = resample::mix_into(*o, *r, 1.0);
            }
        }
        self.input[..n].fill(0.0);
    }
}

#[cfg(test)]
mod reverb_tests {
    use super::{next_prime, AllPass, ReverbTail, BLOCK, REVERB_ENTRIES};

    #[test]
    fn the_delay_is_the_next_prime_of_the_truncated_frame_count() {
        assert_eq!(next_prime(0), 2);
        assert_eq!(next_prime(1), 2);
        assert_eq!(next_prime(13), 13);
        assert_eq!(next_prime(14), 17);
        // 100 / 70 / 49 / 34 ms at 22050 Hz: 2205 → 2207, 1543.5 → 1543, 1080.45 → 1087, 749.7 → 751
        let l: Vec<usize> = REVERB_ENTRIES.iter().map(|&e| AllPass::new(22050, e).len).collect();
        assert_eq!(l, [2207, 1543, 1087, 751]);
    }

    #[test]
    fn an_all_pass_returns_the_input_at_once_and_its_feedback_one_delay_later() {
        // entry 0: c = 64/127, no damping (g1 = 0, g2 = 1)
        let mut ap = AllPass::new(1000, REVERB_ENTRIES[0]);
        let len = ap.len;
        assert_eq!(len, 101);
        let c = ap.c;
        let mut x = vec![0.0f32; len + 1];
        x[0] = 1.0;
        ap.run(&mut x);
        // y = −c·x + (the empty line) on the way in, then the stored `x + c·y` comes back
        assert!((x[0] + c).abs() < 1e-6);
        assert!((x[len] - (1.0 - c * c)).abs() < 1e-5);
    }

    #[test]
    fn the_reverb_rings_for_two_thousand_blocks_after_the_last_send() {
        let mut r = ReverbTail::new(22050);
        let mut out = vec![[0.0f32; 2]; BLOCK];
        // nothing sent: silent, and the input is never touched
        r.process(&mut out);
        assert!(out.iter().all(|f| *f == [0.0, 0.0]));
        r.send(0, 1000.0, 1.0);
        r.process(&mut out);
        assert!(out[0][0] != 0.0 && out[0][0] == out[0][1]);
        for _ in 0..1997 {
            r.process(&mut out);
        }
        assert_eq!(r.quiet, 1998);
        r.process(&mut out);
        r.process(&mut out);
        assert_eq!(r.quiet, 2000);
        let mut quiet = vec![[0.0f32; 2]; BLOCK];
        r.process(&mut quiet);
        assert!(quiet.iter().all(|f| *f == [0.0, 0.0]));
    }
}
