//! Force-feedback output — the port's mechanism (`RE/notes/force-feedback.md` §10–§13). Two backends,
//! both SDL3:
//!
//! * **Rumble** ([`Rumbler`], a standard pad on every OS): two sustained motor levels (strong /
//!   weak) and one-shot strong pulses through `SDL_RumbleGamepad` — the low- and high-frequency
//!   motors (XInput / GameInput / Windows.Gaming.Input on Windows, evdev `FF_RUMBLE` or HIDAPI on
//!   Linux, GameController + CoreHaptics on macOS, which is what the port's own `ff_macos.rs`
//!   did by hand until SDL3). The mapping of the original's effects onto two motors is
//!   **PORT-DEVIATION D24** and lives with the caller (`open_unleashed::app::force_feedback`).
//! * **Haptic** ([`HapticWheel`], a wheel or any other non-pad joystick SDL can drive through
//!   `SDL_OpenHapticFromJoystick`): the original's four DirectInput effects one for one —
//!   constant force, square, sine, spring — with the arguments of 0x533910 / 0x5338D0
//!   ([`DiCall`]) converted to SDL's units ([`sdl_effect`]); DirectInput on Windows, evdev `FF_*` on
//!   Linux, IOKit `ForceFeedback.framework` on macOS. **Not tested on a device** (no wheel was
//!   available).
//!
//! Trigger rumble (`SDL_RumbleGamepadTriggers`) is not driven: none of the original's four effects
//! is a trigger effect, and no producer has anything that would map onto one.

use sdl3_sys::haptic::*;

/// A sustained rumble: one level per motor, played until it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motor {
    /// the heavy, low-frequency motor
    Strong,
    /// the light, high-frequency motor
    Weak,
}

/// Where the rumbler's calls go: `SDL_RumbleGamepad` on the first device that can (the
/// [`crate::gamepad::Devices`]), a recorder in the tests.
pub(crate) trait RumbleSink {
    /// `low` / `high` = the strong / weak motors, 0…65535, for `ms`; false when SDL refused it.
    fn rumble(&mut self, low: u16, high: u16, ms: u32) -> bool;
}

impl RumbleSink for crate::gamepad::Devices {
    fn rumble(&mut self, low: u16, high: u16, ms: u32) -> bool {
        crate::gamepad::Devices::rumble(self, low, high, ms)
    }
}

/// The rumble state: both sustained levels and the pulse in flight. SDL's rumble call sets both
/// motors at once for a duration, so every change re-sends the combined state.
///
/// **Rate limit (PAD-HANG, PORT-DEVIATION D24).** The producers hand a new level every Sc32 tick
/// (the engine rumble follows the revs, the road rumble the speed), and every `SDL_RumbleGamepad`
/// whose pair differs from the last reaches the driver (`SDL_joystick.c` `SDL_RumbleJoystick`:
/// only an identical pair is "just update the expiration"). On macOS the driver is the
/// GameController one (`SDL_mfijoystick.m` `IOS_JoystickRumble` → `SDL3_RumbleContext
/// rumbleWithLowFrequency:andHighFrequency:`), which sends **both** motors every time — a
/// `CHHapticDynamicParameter` and `-[CHHapticPatternPlayer sendParameters:atTime:]` per motor
/// (`SDL3_RumbleMotor setIntensity:`), plus `stopAtTime:` / `startAtTime:` when a motor goes to /
/// from 0 — i.e. ~128 CoreHaptics messages a second to a Bluetooth pad. So the levels are
/// quantised to [`LEVEL_STEPS`] and a sustained change goes out at most every [`MIN_INTERVAL`];
/// a motor starting or stopping, a pulse and a stop go out at once.
#[derive(Default)]
pub struct Rumbler {
    strong: f32,
    weak: f32,
    /// (end, level) of a pulse in flight
    pulse: Option<(std::time::Instant, f32)>,
    /// (low, high, when) of the last call sent
    sent: Option<(u16, u16, std::time::Instant)>,
    /// when a refused call may be tried again
    retry_at: Option<std::time::Instant>,
}

/// How long one sustained call runs before SDL stops the motors by itself (SDL caps a call at
/// 0xFFFF ms); renewed whenever the caller touches the rumbler after half of it.
const SUSTAIN_MS: u32 = 60_000;
/// The shortest time between two calls that only change a sustained level (PAD-HANG).
pub const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
/// How many levels a motor has (PAD-HANG): a change smaller than 1/32 of the range is not sent.
pub const LEVEL_STEPS: u16 = 32;
/// How long a refused call (an engine CoreHaptics stopped, a pad that went away) waits before the
/// next try.
const RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// A 0…1 level as SDL's 0…65535 in [`LEVEL_STEPS`] steps, rounded to the nearest; a level above 0
/// is at least one step (a quiet rumble does not become a stop).
pub fn quantise(level: f32) -> u16 {
    let l = level.clamp(0.0, 1.0);
    if l <= 0.0 {
        return 0;
    }
    let steps = ((l * f32::from(LEVEL_STEPS)).round() as u16).clamp(1, LEVEL_STEPS);
    (u32::from(steps) * u32::from(u16::MAX) / u32::from(LEVEL_STEPS)) as u16
}

impl Rumbler {
    fn apply(&mut self, pads: &mut impl RumbleSink, urgent: bool) {
        self.apply_at(pads, std::time::Instant::now(), urgent);
    }

    /// Send the combined state if it is due. `urgent`: a pulse or a stop the caller asked for.
    fn apply_at(&mut self, pads: &mut impl RumbleSink, now: std::time::Instant, urgent: bool) {
        let mut pulse_ended = false;
        let pulse = match self.pulse {
            Some((end, level)) if end > now => level,
            Some(_) => {
                self.pulse = None;
                pulse_ended = true;
                0.0
            }
            None => 0.0,
        };
        let (low, high) = (quantise(self.strong.max(pulse)), quantise(self.weak));
        let renew = |t: std::time::Instant| now.saturating_duration_since(t).as_millis() >= u128::from(SUSTAIN_MS / 2);
        let due = match self.sent {
            None => low != 0 || high != 0,
            Some((l, h, t)) if l == low && h == high => renew(t),
            Some((l, h, t)) => {
                // a motor starting or stopping, a pulse and its end, a stop: at once; a level change
                // within the rate limit
                let on_off = (l == 0) != (low == 0) || (h == 0) != (high == 0);
                urgent || pulse_ended || on_off || now.saturating_duration_since(t) >= MIN_INTERVAL
            }
        };
        if !due || self.retry_at.is_some_and(|r| now < r && !urgent) {
            return;
        }
        // a lone pulse stops by itself when it ends; with a motor sustained the call runs long
        let ms = match self.pulse {
            Some((end, _)) if self.weak <= 0.0 && self.strong <= 0.0 => end.saturating_duration_since(now).as_millis().max(1) as u32,
            _ => SUSTAIN_MS,
        };
        if low == 0 && high == 0 {
            pads.rumble(0, 0, 0);
            self.sent = None;
            self.retry_at = None;
        } else if pads.rumble(low, high, ms) {
            self.sent = Some((low, high, now));
            self.retry_at = None;
        } else {
            // refused: not again every tick (SDL's GameController driver answers "Haptics engine
            // was stopped" for good once CoreHaptics stopped the engine)
            self.retry_at = Some(now + RETRY);
        }
    }

    /// Send a sustained change the rate limit held back, once its interval is over, and end a pulse
    /// on time; called once a frame (`Gamepads::poll`).
    pub(crate) fn flush(&mut self, pads: &mut impl RumbleSink) {
        self.apply(pads, false);
    }

    /// Set a motor's sustained level, 0…1 (0 stops it).
    pub fn sustain(&mut self, pads: &mut crate::gamepad::Devices, motor: Motor, level: f32) {
        self.set_level(motor, level);
        self.apply(pads, false);
    }

    fn set_level(&mut self, motor: Motor, level: f32) {
        let level = level.clamp(0.0, 1.0);
        match motor {
            Motor::Strong => self.strong = level,
            Motor::Weak => self.weak = level,
        }
    }

    /// A one-shot strong pulse of `level` (0…1) for `ms` milliseconds.
    pub fn pulse(&mut self, pads: &mut crate::gamepad::Devices, level: f32, ms: u32) {
        if self.start_pulse(std::time::Instant::now(), level, ms) {
            self.apply(pads, true);
        }
    }

    fn start_pulse(&mut self, now: std::time::Instant, level: f32, ms: u32) -> bool {
        let level = level.clamp(0.0, 1.0);
        if level <= 0.0 {
            return false;
        }
        self.pulse = Some((now + std::time::Duration::from_millis(u64::from(ms.max(1))), level));
        // re-sent even when the pair is the same: the pulse's own duration starts now
        self.sent = None;
        true
    }

    /// Stop both sustained motors (a pause, a menu, the end of the race).
    pub fn stop_all(&mut self, pads: &mut crate::gamepad::Devices) {
        self.strong = 0.0;
        self.weak = 0.0;
        self.pulse = None;
        self.apply(pads, true);
    }

    /// Whether a device that can rumble is connected.
    pub fn supported(&self, pads: &crate::gamepad::Devices) -> bool {
        pads.rumble_device_name().is_some()
    }

    /// One line for the log: which backend this is and which device it drives.
    pub fn describe(&self, pads: &crate::gamepad::Devices) -> String {
        match pads.rumble_device_name() {
            Some(n) => format!("SDL rumble (strong / weak motor), device \"{n}\""),
            None => "SDL rumble (strong / weak motor), no device that supports it".to_string(),
        }
    }
}

/// The original's four effects, by their handle slot `0x5E9420 + 8·dev + {0,2,4,6}` (0x411D20,
/// `force-feedback.md` §3) and their GUID (the 0x5BB3B0 table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiSlot {
    /// slot 0, `GUID_ConstantForce` — the collision jolt
    Constant,
    /// slot 2, `GUID_Square` — the road rumble
    Square,
    /// slot 4, `GUID_Sine` — the engine rumble
    Sine,
    /// slot 6, `GUID_Spring` (two conditions) — the grip / self-aligning spring
    Spring,
}

impl DiSlot {
    fn index(self) -> usize {
        self as usize
    }
}

/// One call of the original's platform library, in DirectInput's units (`force-feedback.md` §3):
/// `0x5338D0(handle)` = `Stop`, `0x533910(handle, 1, params, dur, gain, 0, dir, env)` = set the
/// parameters that changed and `Start` unless running. `gain` is the effect's `FF.*` level 0…100
/// (`dwGain = gain·100`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiCall {
    Stop(DiSlot),
    /// `DIPERIODIC {magnitude, 0, 0, period_us}` on the square or the sine, infinite duration
    Periodic { slot: DiSlot, magnitude: i32, period_us: i32, gain: i32 },
    /// two `DICONDITION`s `{offset, coef, coef, coef, coef, 0}` (axes X, Y), infinite duration
    Spring { x: (i32, i32), y: (i32, i32), gain: i32 },
    /// `DICONSTANTFORCE {magnitude}`, `duration_us`, polar `direction` in degrees, `DIENVELOPE`
    Constant { magnitude: i32, duration_us: i32, direction: i32, attack_level: i32, attack_time: i32, fade_level: i32, fade_time: i32, gain: i32 },
}

/// An SDL effect description (the arm of `SDL_HapticEffect` in use, comparable).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SdlEffect {
    Constant(SDL_HapticConstant),
    Periodic(SDL_HapticPeriodic),
    Condition(SDL_HapticCondition),
}

impl SdlEffect {
    fn raw(&self) -> SDL_HapticEffect {
        let mut e = SDL_HapticEffect::default();
        match *self {
            SdlEffect::Constant(c) => e.constant = c,
            SdlEffect::Periodic(p) => e.periodic = p,
            SdlEffect::Condition(c) => e.condition = c,
        }
        e
    }
}

/// A DirectInput level (±10000) as SDL's ±0x7FFF, rounded away from zero so that SDL's own
/// DirectInput backend (`SDL_dinputhaptic.c` `CONVERT(x) = x·10000 / 0x7FFF`, truncating) hands the
/// driver back exactly the original's figure.
pub fn sdl_level(di: i32) -> i16 {
    let d = di.clamp(-10000, 10000);
    let m = (d.unsigned_abs() * 0x7FFF).div_ceil(10000) as i32;
    (m * d.signum()) as i16
}

/// A DirectInput saturation (0…10000) as SDL's 0…0xFFFF (SDL's backend halves it before
/// `CCONVERT`, so twice the level figure round-trips exactly).
pub fn sdl_saturation(di: i32) -> u16 {
    (sdl_level(di.clamp(0, 10000)) as u16) * 2
}

/// Microseconds as SDL's milliseconds, rounded to the nearest (**APPROXIMATION**: SDL's effect times
/// are whole milliseconds, DirectInput's microseconds — a period of 1e6/35 µs becomes 29 ms).
fn ms(us: i32) -> u32 {
    (us.max(0) as u32).saturating_add(500) / 1000
}

fn ms16(us: i32) -> u16 {
    ms(us).min(u32::from(u16::MAX)) as u16
}

/// A level scaled by the effect's gain block, 0…100 (`dwGain = gain·100`, 0x5339A7…). SDL has no
/// per-effect gain, so the scale is folded into the magnitudes, as DirectInput applies it.
fn gained(v: i32, gain: i32) -> i32 {
    v * gain.clamp(0, 100) / 100
}

/// The SDL effect for one [`DiCall`] (`None` for a stop). `wheel` is 0x412D5A's class (a
/// `DIDEVTYPEJOYSTICK_WHEEL`, here `SDL_JOYSTICK_TYPE_WHEEL`): the effects are created with the polar
/// direction **90°** on a wheel and 0° otherwise (0x411E06…0x411E24); the jolt carries its own.
pub fn sdl_effect(call: &DiCall, wheel: bool) -> Option<SdlEffect> {
    let polar = |deg100: i32| SDL_HapticDirection { r#type: SDL_HAPTIC_POLAR, dir: [deg100, 0, 0] };
    let created = polar(if wheel { 9000 } else { 0 });
    Some(match *call {
        DiCall::Stop(_) => return None,
        DiCall::Periodic { slot, magnitude, period_us, gain } => SdlEffect::Periodic(SDL_HapticPeriodic {
            r#type: if slot == DiSlot::Square { SDL_HAPTIC_SQUARE } else { SDL_HAPTIC_SINE },
            direction: created,
            length: SDL_HAPTIC_INFINITY,
            period: ms16(period_us).max(1),
            magnitude: sdl_level(gained(magnitude, gain)),
            ..Default::default()
        }),
        DiCall::Spring { x, y, gain } => {
            let mut c = SDL_HapticCondition { r#type: SDL_HAPTIC_SPRING, direction: created, length: SDL_HAPTIC_INFINITY, ..Default::default() };
            for (i, (offset, coef)) in [x, y].into_iter().enumerate() {
                // DICONDITION {lOffset, lPositiveCoefficient = lNegativeCoefficient = dwPositiveSaturation =
                // dwNegativeSaturation = coef, lDeadBand 0} (0x4125F5…0x4126F4); the gain scales the
                // coefficients and saturations (APPROXIMATION: DirectInput's own application of dwGain
                // to a condition is the driver's)
                let k = gained(coef, gain);
                c.center[i] = sdl_level(offset);
                c.right_coeff[i] = sdl_level(k);
                c.left_coeff[i] = sdl_level(k);
                c.right_sat[i] = sdl_saturation(k.abs());
                c.left_sat[i] = sdl_saturation(k.abs());
            }
            SdlEffect::Condition(c)
        }
        DiCall::Constant { magnitude, duration_us, direction, attack_level, attack_time, fade_level, fade_time, gain } => {
            // 0x533910 clamps the duration to 1 500 000 µs; the fade time `dur − mag` (0x412DE2) can be
            // negative, a huge DWORD to DirectInput — 0 here (APPROXIMATION: SDL's field is unsigned)
            let dur = duration_us.clamp(0, 1_500_000);
            SdlEffect::Constant(SDL_HapticConstant {
                r#type: SDL_HAPTIC_CONSTANT,
                direction: polar(direction.rem_euclid(360) * 100),
                length: ms(dur).max(1),
                level: sdl_level(gained(magnitude, gain)),
                attack_length: ms16(attack_time),
                attack_level: sdl_level(gained(attack_level, gain).clamp(0, 10000)) as u16,
                fade_length: ms16(fade_time),
                fade_level: sdl_level(gained(fade_level, gain).clamp(0, 10000)) as u16,
                ..Default::default()
            })
        }
    })
}

/// `SDL_GetHapticFeatures` bits (`SDL_haptic.h`; written out because sdl3-sys 0.7's
/// `SDL_HapticEffectType` is 16 bits wide and truncates `SDL_HAPTIC_GAIN` = 1 << 16 to 0).
const FEAT_CONSTANT: u32 = 1 << 0;
const FEAT_SINE: u32 = 1 << 1;
const FEAT_SQUARE: u32 = 1 << 2;
const FEAT_SPRING: u32 = 1 << 7;
const FEAT_GAIN: u32 = 1 << 16;
/// Any of the four effects the original drives.
pub(crate) const FEAT_ANY: u32 = FEAT_CONSTANT | FEAT_SINE | FEAT_SQUARE | FEAT_SPRING;

fn slot_feature(slot: DiSlot) -> u32 {
    match slot {
        DiSlot::Constant => FEAT_CONSTANT,
        DiSlot::Square => FEAT_SQUARE,
        DiSlot::Sine => FEAT_SINE,
        DiSlot::Spring => FEAT_SPRING,
    }
}

/// An open SDL haptic device and its four effects (created on first use, as 0x411D20 creates them
/// at session start), **untested on hardware**. Every failure is logged once and otherwise ignored.
pub struct HapticWheel {
    haptic: *mut SDL_Haptic,
    pub(crate) features: u32,
    /// `SDL_JOYSTICK_TYPE_WHEEL` (0x412D5A's class)
    pub(crate) wheel: bool,
    effects: [Option<(SDL_HapticEffectID, SdlEffect)>; 4],
    running: [bool; 4],
    failed: [bool; 4],
}

impl HapticWheel {
    /// # Safety
    /// `haptic` is a device `SDL_OpenHapticFromJoystick` returned; it is closed on drop.
    pub(crate) unsafe fn new(haptic: *mut SDL_Haptic, wheel: bool) -> Self {
        let features = unsafe { SDL_GetHapticFeatures(haptic) };
        Self { haptic, features, wheel, effects: [None; 4], running: [false; 4], failed: [false; 4] }
    }

    /// 0x5333A0 at session start: `DIPROP_FFGAIN` = `FF.TotalVolume`·100 — SDL's device gain 0…100
    /// (its DirectInput backend sets exactly `gain·100`). Without `SDL_HAPTIC_GAIN` nothing is set.
    pub(crate) fn set_gain(&mut self, total: i32) {
        if self.features & FEAT_GAIN != 0 && !unsafe { SDL_SetHapticGain(self.haptic, total.clamp(0, 100)) } {
            tracing::warn!(error = %crate::gamepad::sdl_error(), "haptic: gain not set");
        }
    }

    /// One 0x533910 / 0x5338D0 call.
    pub(crate) fn call(&mut self, call: &DiCall) {
        let slot = match *call {
            DiCall::Stop(s) => s,
            DiCall::Periodic { slot, .. } => slot,
            DiCall::Spring { .. } => DiSlot::Spring,
            DiCall::Constant { .. } => DiSlot::Constant,
        };
        let i = slot.index();
        if self.features & slot_feature(slot) == 0 || self.failed[i] {
            return;
        }
        let Some(eff) = sdl_effect(call, self.wheel) else {
            if let (Some((id, _)), true) = (self.effects[i], self.running[i]) {
                unsafe { SDL_StopHapticEffect(self.haptic, id) };
            }
            self.running[i] = false;
            return;
        };
        let raw = eff.raw();
        let id = match self.effects[i] {
            Some((id, last)) if last == eff => id,
            Some((id, _)) => {
                if !unsafe { SDL_UpdateHapticEffect(self.haptic, id, &raw) } {
                    return self.fail(i, "update");
                }
                self.effects[i] = Some((id, eff));
                id
            }
            None => {
                let id = unsafe { SDL_CreateHapticEffect(self.haptic, &raw) };
                if id.0 < 0 {
                    return self.fail(i, "create");
                }
                self.effects[i] = Some((id, eff));
                id
            }
        };
        // `Start` unless running (0x533B40); a jolt is finite, and its re-arm window (1.11 × the
        // duration, 0x412CF0) has let the last one end, so it is started every time
        if !self.running[i] || slot == DiSlot::Constant {
            if !unsafe { SDL_RunHapticEffect(self.haptic, id, 1) } {
                return self.fail(i, "run");
            }
            self.running[i] = slot != DiSlot::Constant;
        }
    }

    fn fail(&mut self, i: usize, what: &str) {
        tracing::warn!(slot = i, error = %crate::gamepad::sdl_error(), "haptic: effect {what} failed; the effect is left off");
        self.failed[i] = true;
    }

    /// Stop every effect (the pause / race-exit paths, 0x412030…).
    pub(crate) fn stop_all(&mut self) {
        unsafe { SDL_StopHapticEffects(self.haptic) };
        self.running = [false; 4];
    }
}

impl Drop for HapticWheel {
    fn drop(&mut self) {
        unsafe { SDL_CloseHaptic(self.haptic) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Records every rumble call with the time the rumbler was told.
    #[derive(Default)]
    struct Rec {
        calls: Vec<(u16, u16, u32)>,
        refuse: bool,
    }

    impl RumbleSink for Rec {
        fn rumble(&mut self, low: u16, high: u16, ms: u32) -> bool {
            self.calls.push((low, high, ms));
            !self.refuse
        }
    }

    fn sustain_at(r: &mut Rumbler, s: &mut Rec, now: Instant, motor: Motor, level: f32) {
        r.set_level(motor, level);
        r.apply_at(s, now, false);
    }

    /// PAD-HANG: the engine and road rumble change every Sc32 tick, two producer calls a tick — the
    /// pad gets at most one call per [`MIN_INTERVAL`], not 64 a second.
    #[test]
    fn a_sustained_rumble_is_rate_limited() {
        let (mut r, mut s) = (Rumbler::default(), Rec::default());
        let t0 = Instant::now();
        let ticks = 64; // two seconds of Sc32 ticks
        for i in 0..ticks {
            let now = t0 + Duration::from_micros(31_250 * i);
            sustain_at(&mut r, &mut s, now, Motor::Strong, 0.2 + 0.6 * (i as f32 / ticks as f32));
            sustain_at(&mut r, &mut s, now, Motor::Weak, 0.3 + 0.01 * (i % 7) as f32);
        }
        assert!(s.calls.len() <= 2_000 / MIN_INTERVAL.as_millis() as usize + 1, "{} calls in two seconds", s.calls.len());
        assert!(s.calls.len() >= 5, "the level still follows the producers: {} calls", s.calls.len());
        // the first call went out at once (the motors starting)
        assert_eq!(s.calls[0], (quantise(0.2), 0, SUSTAIN_MS));
    }

    /// A change under one step of [`LEVEL_STEPS`] is never sent; a held-back change goes out with the
    /// first flush after the interval.
    #[test]
    fn small_changes_wait_and_held_back_ones_are_flushed() {
        let (mut r, mut s) = (Rumbler::default(), Rec::default());
        let t0 = Instant::now();
        sustain_at(&mut r, &mut s, t0, Motor::Weak, 0.5);
        assert_eq!(s.calls.len(), 1);
        sustain_at(&mut r, &mut s, t0 + Duration::from_millis(500), Motor::Weak, 0.505);
        assert_eq!(s.calls.len(), 1, "under one step");
        sustain_at(&mut r, &mut s, t0 + Duration::from_millis(510), Motor::Weak, 0.8);
        assert_eq!(s.calls.len(), 2, "a visible change after the interval goes out");
        sustain_at(&mut r, &mut s, t0 + Duration::from_millis(520), Motor::Weak, 0.6);
        assert_eq!(s.calls.len(), 2, "within the interval: held back");
        r.apply_at(&mut s, t0 + Duration::from_millis(560), false);
        assert_eq!(s.calls.len(), 2);
        r.apply_at(&mut s, t0 + Duration::from_millis(615), false);
        assert_eq!(s.calls.last(), Some(&(0, quantise(0.6), SUSTAIN_MS)), "flushed once the interval is over");
    }

    /// A stop, a motor starting and a pulse go out at once, whatever the rate limit.
    #[test]
    fn stops_starts_and_pulses_are_immediate() {
        let (mut r, mut s) = (Rumbler::default(), Rec::default());
        let t0 = Instant::now();
        sustain_at(&mut r, &mut s, t0, Motor::Weak, 0.5);
        sustain_at(&mut r, &mut s, t0 + Duration::from_millis(10), Motor::Strong, 0.4);
        assert_eq!(s.calls.last(), Some(&(quantise(0.4), quantise(0.5), SUSTAIN_MS)), "a motor starting");
        assert!(r.start_pulse(t0 + Duration::from_millis(20), 1.0, 300));
        r.apply_at(&mut s, t0 + Duration::from_millis(20), true);
        assert_eq!(s.calls.last(), Some(&(u16::MAX, quantise(0.5), SUSTAIN_MS)), "the pulse");
        // its end drops the strong motor back at once
        r.apply_at(&mut s, t0 + Duration::from_millis(330), false);
        assert_eq!(s.calls.last(), Some(&(quantise(0.4), quantise(0.5), SUSTAIN_MS)), "the pulse's end");
        sustain_at(&mut r, &mut s, t0 + Duration::from_millis(340), Motor::Strong, 0.0);
        assert_eq!(s.calls.last(), Some(&(0, quantise(0.5), SUSTAIN_MS)), "a motor stopping");
        r.strong = 0.0;
        r.weak = 0.0;
        r.apply_at(&mut s, t0 + Duration::from_millis(345), true);
        assert_eq!(s.calls.last(), Some(&(0, 0, 0)), "the stop");
        let n = s.calls.len();
        r.apply_at(&mut s, t0 + Duration::from_millis(400), false);
        assert_eq!(s.calls.len(), n, "nothing more to send");
        // a lone pulse carries its own duration
        assert!(r.start_pulse(t0 + Duration::from_millis(500), 0.5, 200));
        r.apply_at(&mut s, t0 + Duration::from_millis(500), true);
        assert_eq!(s.calls.last(), Some(&(quantise(0.5), 0, 200)));
    }

    /// A refused call is not retried every tick.
    #[test]
    fn a_refused_call_backs_off() {
        let (mut r, mut s) = (Rumbler::default(), Rec { refuse: true, ..Default::default() });
        let t0 = Instant::now();
        for i in 0..64 {
            sustain_at(&mut r, &mut s, t0 + Duration::from_millis(31 * i), Motor::Weak, 0.5);
        }
        assert_eq!(s.calls.len(), 1);
        s.refuse = false;
        sustain_at(&mut r, &mut s, t0 + Duration::from_millis(2_100), Motor::Weak, 0.5);
        assert_eq!(s.calls.len(), 2, "tried again after the back-off");
    }

    #[test]
    fn the_levels_are_quantised() {
        assert_eq!(quantise(0.0), 0);
        assert_eq!(quantise(-1.0), 0);
        assert_eq!(quantise(1.0), u16::MAX);
        assert_eq!(quantise(0.001), quantise(1.0 / 32.0), "a quiet rumble stays on");
        assert_eq!(quantise(0.5), quantise(0.51));
    }

    /// SDL's DirectInput backend (`CONVERT`, `CCONVERT` of half the saturation) gives the original's
    /// DirectInput figures back exactly.
    #[test]
    fn the_levels_round_trip_through_sdls_directinput_backend() {
        let convert = |x: i32| x * 10000 / 0x7FFF;
        let cconvert = |x: i32| if x > 0x7FFF { 10000 } else { x * 10000 / 0x7FFF };
        for d in -10000..=10000 {
            assert_eq!(convert(i32::from(sdl_level(d))), d, "level {d}");
        }
        for d in 0..=10000 {
            assert_eq!(cconvert(i32::from(sdl_saturation(d)) / 2), d, "saturation {d}");
        }
    }

    /// The jolt (0x412CF0's call) and the road square as SDL effects.
    #[test]
    fn the_effects_carry_the_originals_arguments() {
        let jolt_with = |fade_time| DiCall::Constant { magnitude: 8000, duration_us: 300_000, direction: 270, attack_level: 8000, attack_time: 150_000, fade_level: 8000, fade_time, gain: 50 };
        let jolt = jolt_with(292_000);
        let Some(SdlEffect::Constant(c)) = sdl_effect(&jolt, true) else { panic!() };
        assert_eq!((c.direction.r#type, c.direction.dir[0]), (SDL_HAPTIC_POLAR, 27000));
        assert_eq!((c.length, c.attack_length, c.fade_length), (300, 150, 292));
        assert_eq!(c.level, sdl_level(4000));
        // the landing jolt's negative fade time
        let land = jolt_with(-9500);
        let Some(SdlEffect::Constant(c)) = sdl_effect(&land, false) else { panic!() };
        assert_eq!(c.fade_length, 0);
        let road = DiCall::Periodic { slot: DiSlot::Square, magnitude: 5000, period_us: 166_667, gain: 100 };
        let Some(SdlEffect::Periodic(p)) = sdl_effect(&road, true) else { panic!() };
        assert_eq!((p.r#type, p.period, p.magnitude, p.length), (SDL_HAPTIC_SQUARE, 167, sdl_level(5000), SDL_HAPTIC_INFINITY));
        assert_eq!(p.direction.dir[0], 9000, "a wheel's effects are created at 90°");
        let spring = DiCall::Spring { x: (300, 6000), y: (0, 0), gain: 100 };
        let Some(SdlEffect::Condition(s)) = sdl_effect(&spring, true) else { panic!() };
        assert_eq!((s.center[0], s.right_coeff[0], s.left_coeff[0], s.right_sat[0]), (sdl_level(300), sdl_level(6000), sdl_level(6000), sdl_saturation(6000)));
        assert_eq!((s.center[1], s.right_coeff[1]), (0, 0));
        assert!(sdl_effect(&DiCall::Stop(DiSlot::Sine), true).is_none());
    }
}
