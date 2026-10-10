//! Named thresholds of the force step. Each keeps the **literal text** the code carried (same type,
//! same bits — `bits_match_the_literals_the_code_carried` pins them); the address of the compare
//! that reads it is in the doc comment.

/// `game_tick` below which an AI car's misfire window is the short one (`ignition_cut`,
/// `engine_update`): `game_tick < 0x1A0`.
pub(super) const TICK_AI_MISFIRE: u64 = 0x1A0;
/// The race-start tick of the pedal routines: an AI car leaves neutral past it (0x40C051…0x40C0AB),
/// a human's manual request is taken past it (0x49DDAE), an opponent is placed absolutely past it.
pub(super) const TICK_RACE_START: u64 = 0x1C0;
/// The automatic gearbox keeps 2nd requested below this tick (`automatic_request`).
pub(super) const TICK_AUTOMATIC: u64 = 0x1C4;
/// The reset request goes through only past this game tick (0x40F7DB…0x40F865).
pub(super) const TICK_RESET_ALLOWED: u64 = 0x300;
/// … and only while `car+0x35C` is under this (`[0x5B2734]`).
pub(super) const RESET_SPEED_LIMIT: f32 = 27.6;
/// The reset hold a granted request loads (0x40F7DB…0x40F865).
pub(super) const RESET_HOLD: u32 = 0xA0;
/// The reset countdown `car+0x1100`: loaded by the crash response (0x4948E6) and by
/// `reset_countdown_start` (0x40FB6D…0x40FBA2).
pub(super) const RESET_COUNTDOWN: i32 = 0xC0;
/// `car+0x3A0` (cos of the tilt) above which a car counts as upright (0x4A1C35; the body-ground
/// pass reads the same figure).
pub(super) const UPRIGHT_COS_TILT: f32 = 0.939;
/// The body's up row `y` under which the slope push applies to `x` as well (0x49F0C4…).
pub(super) const SLOPE_UP_Y: f32 = 0.833;
/// The slope push on `x` and `z` (`accel_local`), times the body rows' `y`.
pub(super) const SLOPE_PUSH_X: f32 = -19.6;
pub(super) const SLOPE_PUSH_Z: f32 = -4.9;
/// The speed above which surface 3 damps the velocity by 0.995 (the arm next to 0x49FCED).
pub(super) const SURFACE3_DAMP_SPEED: f32 = 44.64;
/// The gravity scale a car starts with and `load_transfer` resets to.
pub(super) const GRAVITY_SCALE: f32 = 1.16;
/// `ClampAngVel`'s limit in turns/s (0x4964D0, 0x49BBA0): the `.TINYCAR` figure, the smackable
/// figure, the ordinary one, and the collision mass under which a smackable gets the high one.
pub(super) const ANG_LIMIT_TINY: f32 = 2.25;
pub(super) const ANG_LIMIT_SMACKABLE: f32 = 6.75;
pub(super) const ANG_LIMIT_DEFAULT: f32 = 0.75;
pub(super) const SMACKABLE_LIGHT_MASS: f32 = 0.75;

#[cfg(test)]
mod tests {
    use super::*;

    /// The bit patterns of the literals the code carried before they were named (captured from the
    /// old text, not recomputed).
    #[test]
    fn bits_match_the_literals_the_code_carried() {
        let f = [
            (RESET_SPEED_LIMIT, 0x41dc_cccd_u32),
            (UPRIGHT_COS_TILT, 0x3f70_624e),
            (SLOPE_UP_Y, 0x3f55_3f7d),
            (SLOPE_PUSH_X, 0xc19c_cccd),
            (SLOPE_PUSH_Z, 0xc09c_cccd),
            (SURFACE3_DAMP_SPEED, 0x4232_8f5c),
            (GRAVITY_SCALE, 0x3f94_7ae1),
            (ANG_LIMIT_TINY, 0x4010_0000),
            (ANG_LIMIT_SMACKABLE, 0x40d8_0000),
            (ANG_LIMIT_DEFAULT, 0x3f40_0000),
            (SMACKABLE_LIGHT_MASS, 0x3f40_0000),
        ];
        for (v, bits) in f {
            assert_eq!(v.to_bits(), bits, "{v}");
        }
        assert_eq!((TICK_AI_MISFIRE, TICK_RACE_START, TICK_AUTOMATIC, TICK_RESET_ALLOWED), (416, 448, 452, 768));
        assert_eq!((RESET_HOLD, RESET_COUNTDOWN), (160, 192));
    }
}
