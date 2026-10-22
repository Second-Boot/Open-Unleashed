//! `COMM_MODE` (`[0x657404]`, option 0x13): the race's communication mode, a global the vehicle reads.
//! `Race_setTypeSkillDamage` 0x4EB470 sets it to 4 for a network race (game type 3) and to 0 otherwise
//! (`RE/notes/modes.md`); a split-screen race sets 1 (not in the port). The vehicle model reads it in
//! `Car_processInput` 0x49DD20 (0x49E0CA): any value above 0 puts the pedal and steering divisor at
//! 240 instead of 252 (see `Vehicle::process_input`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

static COMM_MODE: AtomicU32 = AtomicU32::new(0);
static POSITIONAL_NET: AtomicBool = AtomicBool::new(false);

/// Set whether this is a positional network race: game type 3 (`[0x6573E8] == 3`) with `COMMSPOSITIONAL`
/// (`[0x657408]`) non-zero. The stability assist 0x49F0C0 is skipped then (0x49F0D1…0x49F0E1).
pub fn set_positional_network(on: bool) {
    POSITIONAL_NET.store(on, Relaxed);
}

/// The gate of 0x49F0D1…0x49F0E1: `[0x6573E8] == 3 && [0x657408] != 0`.
pub fn positional_network() -> bool {
    POSITIONAL_NET.load(Relaxed)
}

/// Set the mode (0 single player, 4 network).
pub fn set(mode: u32) {
    COMM_MODE.store(mode, Relaxed);
}

/// The mode as `[0x657404]` holds it.
pub fn get() -> u32 {
    COMM_MODE.load(Relaxed)
}

/// The pedal divisor of 0x49E0CF…0x49E0DB: `240.0` when `COMM_MODE > 0` (signed compare, 0x49E0D9 `jle`), else `252.0`.
pub fn pedal_divisor() -> f32 {
    divisor_for(get())
}

/// [`pedal_divisor`] for a given mode word.
pub fn divisor_for(mode: u32) -> f32 {
    if (mode as i32) > 0 { 240.0 } else { 252.0 }
}

#[cfg(test)]
mod tests {
    use super::divisor_for;

    /// 0x49E0CF…0x49E0DB: 252 at `COMM_MODE` 0, 240 at the network race's 4 (and at split screen's 1); a full
    /// packet (0x7E · 2 = 252) is 1.0 either way, the exchange wire's top throttle 124 · 2 = 248 only under 240
    /// 0x49F0D1…0x49F0E1: the assist's network gate is on only for a positional race (game type 3 with `COMMSPOSITIONAL`)
    #[test]
    fn the_assist_gate_is_the_positional_network_race() {
        assert!(!super::positional_network());
        super::set_positional_network(true);
        assert!(super::positional_network());
        super::set_positional_network(false);
        assert!(!super::positional_network());
    }

    #[test]
    fn the_pedal_divisor_follows_comm_mode() {
        assert_eq!((divisor_for(0), divisor_for(1), divisor_for(4)), (252.0, 240.0, 240.0));
        assert_eq!((252.0f32 / divisor_for(4)).min(1.0), 1.0);
        assert!((248.0f32 / divisor_for(0)).min(1.0) < 0.985);
        assert_eq!((248.0f32 / divisor_for(4)).min(1.0), 1.0);
    }
}
