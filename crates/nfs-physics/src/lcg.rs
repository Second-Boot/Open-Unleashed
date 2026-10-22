//! The word of the original's one shared random stream (`[0x5D1028]`, `Rand_seed` 0x4A4780), kept
//! here, below `nfs-game`, so the vehicle model can draw from it too; `nfs_game::rng` is its API.
//! The arithmetic: `prod = state · 123456789`, `state = prod & 0xFFFF`, draw `(prod >> 8) & 0xFFFF`.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// The multiplier.
pub const MUL: u32 = 123_456_789;

/// The stream's state word.
pub static STATE: AtomicU32 = AtomicU32::new(0x3ADE_68B1);

/// One 16-bit draw of the shared stream.
#[track_caller]
pub fn rand16() -> u32 {
    // one read-modify-write, so concurrent draws still each take a distinct step
    let mut g = STATE.load(Relaxed);
    while let Err(now) = STATE.compare_exchange_weak(g, g.wrapping_mul(MUL) & 0xFFFF, Relaxed, Relaxed) {
        g = now;
    }
    // TEST GLUE `NFS_PA_RNGLOG`: every draw with its call site and the state it leaves, for the
    // trace against the original's watch on `[0x5D1028]`
    if *LOG.get_or_init(|| std::env::var_os("NFS_PA_RNGLOG").is_some()) {
        eprintln!("rng {} {}", std::panic::Location::caller(), g.wrapping_mul(MUL) & 0xFFFF);
    }
    (g.wrapping_mul(MUL) >> 8) & 0xFFFF
}

static LOG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
