//! Force feedback: the original's producers ([`nfs_physics::force_feedback`], read to the
//! instruction, `RE/notes/force-feedback.md`) wired to the race loop, and the one place where their
//! DirectInput effect parameters are **approximated** onto what gilrs can drive (a pad's two rumble
//! motors, [`nfs_input::ff`]). The producers' output never reaches the simulation.
//!
//! What is the original's: when the driver runs (every Sc32 tick, before the player's physics
//! step, 0x4121B0 / 0x4123E0), which effect gets which parameters (`FfCommand`), the five `FF.*`
//! levels as the per-effect gains and the total, the jolt's direction / duration / magnitude and its
//! re-arm window, the pause stop (0x412030…).
//!
//! What is **APPROXIMATION** (each labelled where it is made):
//! * **A1, the device.** The original arms force feedback only when the player has a
//!   force-feedback-capable DirectInput device bound in the control config (0x4116D0, 0x411D20) and
//!   picks the *last* such device of 32. The port arms it when gilrs reports a connected pad that
//!   supports rumble (on macOS through GameController / CoreHaptics, `nfs_input::ff_macos`) and treats it as a pad, not a wheel (0x412D5A's class test
//!   `devtype & 0xFF00 == 0x600` — `dwDevType` of the legacy `DIDEVICEINSTANCE` (0x56FDB0, filled by `GetDeviceInfo`, returned by 0x532E10 kind 2): subtype 6 = `DIDEVTYPEJOYSTICK_WHEEL`; an Xbox pad is GAMEPAD, subtype 4, so not a wheel — `force-feedback.md` §2).
//! * **A2, the effect types.** (the same on macOS, where the motors are a pad's left / right handle
//!   haptic engines.) gilrs has constant strong / weak motor magnitudes only (50 ms ticks).
//!   Road (a square wave of 6…17 Hz, magnitude ≤ 5000 of 10000) → the strong motor at a constant
//!   level `magnitude · gain · total`; engine (a sine, 10…35 Hz) → the weak motor, the same level
//!   law; jolt (a constant force with a polar direction and an envelope) → a strong pulse of its
//!   magnitude for its duration (rounded up to 50 ms), the direction dropped. The spring that
//!   carries the grip / self-aligning feel (`FfCommand::Spring`) has **no counterpart on a pad** and
//!   is not output.
//! * **A3, the clock.** `[0x6559E0]`, the 128 Hz counter the jolt's re-arm window and left/right
//!   parity read, is the simulation's own: `2 · tick`, plus 1 for the collision pass (the effect
//!   only needs a monotonic counter).

use super::*;
use nfs_input::ff::Motor;
use nfs_physics::force_feedback::{FfCar, FfCommand, FfEffect, FfLevels, ForceFeedback};

/// `DICONDITION` / `dwGain` scale: a level is a percentage, the effect's magnitude a figure of 10000.
fn level(magnitude: i32, gain: i32, total: i32) -> f32 {
    (magnitude as f32 / 10000.0) * (gain as f32 / 100.0) * (total as f32 / 100.0)
}

impl App {
    /// The five `FF.*` accessors, the first dword of the original's five option globals
    /// `[0x657BC8…0x657C48]` (`forcefeedback.lay`, 0…100, default 50).
    pub(crate) fn ff_levels(&self) -> FfLevels {
        let v = |n: &str| self.range_value(n).map_or(50, |r| r.0);
        FfLevels {
            total: v("FF.TotalVolume"),
            road: v("FF.RoadVolume"),
            engine: v("FF.EngineVolume"),
            collision: v("FF.CollisionVolume"),
            grip: v("FF.RoadGripVolume"),
        }
    }

    /// 0x4121B0 / 0x412150, at a race's start: the FF state is made afresh (A1).
    pub(crate) fn ff_arm(&mut self) {
        self.gamepads.rumble_stop_all();
        let enabled = self.gamepads.rumble_supported();
        tracing::info!("force feedback: {} — {}", if enabled { "armed" } else { "off" }, self.gamepads.rumble_describe());
        self.ff = Some(ForceFeedback::new(enabled, false));
    }

    /// 0x412030 / 0x412090 / 0x4120F0, the pause's stop-all.
    pub(crate) fn ff_stop_all(&mut self) {
        if let Some(ff) = self.ff.as_mut() {
            let cmds = ff.stop_all();
            ff_output(&mut self.gamepads, &cmds, self.ff_levels.total);
        }
    }
}

/// The 128 Hz counter the port feeds the effects (A3).
pub(crate) fn ff_clock(tick: u64) -> i32 {
    (tick as i32).wrapping_mul(2)
}

/// Map the platform calls onto the pad's rumble (A2).
pub(crate) fn ff_output(pads: &mut nfs_input::gamepad::Gamepads, cmds: &[FfCommand], total: i32) {
    for c in cmds {
        match *c {
            FfCommand::Stop(FfEffect::Road) => pads.rumble_sustain(Motor::Strong, 0.0),
            FfCommand::Stop(FfEffect::Engine) => pads.rumble_sustain(Motor::Weak, 0.0),
            FfCommand::Stop(FfEffect::Collision | FfEffect::Grip) => {}
            FfCommand::Periodic { effect: FfEffect::Road, magnitude, gain, .. } => pads.rumble_sustain(Motor::Strong, level(magnitude, gain, total)),
            FfCommand::Periodic { effect: FfEffect::Engine, magnitude, gain, .. } => pads.rumble_sustain(Motor::Weak, level(magnitude, gain, total)),
            FfCommand::Periodic { .. } => {}
            FfCommand::Spring { .. } => {}
            FfCommand::Jolt { magnitude, duration_us, gain, .. } => pads.rumble_pulse(level(magnitude, gain, total), (duration_us.max(0) / 1000) as u32),
        }
    }
}

/// The driver 0x4123E0, once per Sc32 tick, before the player's physics step (priority 10 against
/// the car's 0x1E): reads the car as the previous tick left it.
pub(crate) fn ff_drive(ff: &mut Option<ForceFeedback>, pads: &mut nfs_input::gamepad::Gamepads, levels: FfLevels, replay: bool, tick: u64, car: &FfCar) {
    let Some(ff) = ff.as_mut() else { return };
    if !ff.enabled {
        return;
    }
    let cmds = ff.tick(car, levels, replay, ff_clock(tick));
    ff_output(pads, &cmds, levels.total);
}

/// 0x40F740 → 0x412230, for the player's car, after the tick's contact words are final.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ff_collision(
    ff: &mut Option<ForceFeedback>,
    pads: &mut nfs_input::gamepad::Gamepads,
    levels: FfLevels,
    replay: bool,
    tick: u64,
    car: &FfCar,
    kind: u32,
    body_point: Vec3,
    magnitude: f32,
) {
    let Some(ff) = ff.as_mut() else { return };
    if !ff.enabled {
        return;
    }
    let cmds = ff.collision_jolt(car, kind, body_point, magnitude, levels, replay, ff_clock(tick).wrapping_add(1));
    ff_output(pads, &cmds, levels.total);
}
