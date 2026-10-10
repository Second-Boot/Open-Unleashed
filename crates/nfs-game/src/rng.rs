//! The original's global random number generator (`Rand_seed` 0x4A4780, drawn inline at every call
//! site, `RE/notes/traffic.md` §14.7).
//!
//! A 16-bit multiplicative LCG: `state = (state · 123456789) mod 2^16`, with the draw taken from bits
//! 8..23 of the full 32-bit product. It is **one shared stream** — the AI's branch choice, the traffic
//! spawner and everything else pull from the same state, which is why reproducing a race means seeding
//! it once and then not touching it out of order.
//!
//! The state is one process-wide word ([`AtomicU32`], the original's global): every thread draws from
//! the same stream, as with the `Mutex` it replaced, but a panic can no longer poison it. Tests that
//! seed it run in parallel threads and share it; a test that needs a fixed sequence reads it right
//! after its own `seed`.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

pub const MUL: u32 = 123_456_789;

/// `2^-16` — the `[0, 1)` scale of a 16-bit draw (`[0x5B253C]`, `RE/notes/traffic.md` §4826,
/// `race-logic.md`): [`rand01`] and the AI's skill draw ([`crate::ai`]) multiply a `rand16` by it.
pub const RAND16_TO_UNIT: f32 = 1.525_878_9e-5;

static STATE: AtomicU32 = AtomicU32::new(0x3ADE_68B1);

/// `Rand_seed(seed)` — `seed mod 500` warm-up steps from 0x3ADE68B1.
pub fn seed(seed: u32) {
    let mut s = 0x3ADE_68B1u32;
    for _ in 0..(seed % 500) {
        s = s.wrapping_mul(MUL) & 0xFFFF;
    }
    STATE.store(s, Relaxed);
}

/// The stream's current state (the LCG word) — **test glue**: a dump's view of where the shared
/// stream stands; reading it draws nothing.
pub fn state() -> u32 {
    STATE.load(Relaxed)
}

/// One 16-bit draw.
pub fn rand16() -> u32 {
    // one read-modify-write, so concurrent draws still each take a distinct step as under the lock
    // (a compare-exchange loop rather than `fetch_update`, which newer toolchains deprecate)
    let mut g = STATE.load(Relaxed);
    while let Err(now) = STATE.compare_exchange_weak(g, g.wrapping_mul(MUL) & 0xFFFF, Relaxed, Relaxed) {
        g = now;
    }
    (g.wrapping_mul(MUL) >> 8) & 0xFFFF
}

/// `rand16 · 1/65536` — the `[0, 1)` the branch choice compares its thresholds against.
pub fn rand01() -> f32 {
    rand16() as f32 * RAND16_TO_UNIT
}

/// `(rand16 · n) >> 16` — the original's "random below n".
pub fn below(n: u32) -> u32 {
    (rand16() * n) >> 16
}

/// The C runtime's `rand`, 0x5A17C9: `s = s·0x343FD + 0x269EC3`, `(s >> 16) & 0x7FFF`. One step of
/// the state word — shared by [`CRand`] and the front end's thread-local `holdrand`
/// (`layout::glints::crt_rand`).
pub const fn crt_step(s: u32) -> u32 {
    s.wrapping_mul(0x343FD).wrapping_add(0x269EC3)
}

/// The draw the C runtime's `rand` takes from a stepped state word (`(s >> 16) & 0x7FFF`).
pub const fn crt_out(s: u32) -> i32 {
    ((s >> 16) & 0x7FFF) as i32
}

/// The C runtime's `rand`, 0x5A17C9, as a generator of its own. The default seed of a C program is 1.
#[derive(Debug, Clone)]
pub struct CRand(u32);

impl Default for CRand {
    fn default() -> Self {
        Self(1)
    }
}

impl CRand {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }
    pub fn next(&mut self) -> i32 {
        self.0 = crt_step(self.0);
        crt_out(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_is_the_originals_and_is_shared() {
        seed(0);
        let a: Vec<u32> = (0..4).map(|_| rand16()).collect();
        // the first draws from the shipped initial state
        assert_eq!(a[0], 65363);
        assert_eq!(a[1], 12122);
        // seeding again rewinds it
        seed(0);
        assert_eq!(rand16(), 65363);
        // and a warm-up moves it on
        seed(3);
        assert_ne!(rand16(), 65363);
        // rand01 stays inside [0, 1)
        for _ in 0..100 {
            let r = rand01();
            assert!((0.0..1.0).contains(&r), "{r}");
        }
    }
}
