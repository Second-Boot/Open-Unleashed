//! Rumble output — the port's force-feedback mechanism, and a narrow one (`RE/notes/force-feedback.md`
//! §7). Two sustained motor levels (strong / weak) and one-shot strong pulses.
//!
//! * macOS: `crate::ff_macos` (GameController + CoreHaptics) — unchanged by the move to SDL2
//!   (PA-SDL): SDL's own rumble on macOS goes through the same framework, but the existing path is
//!   the one that has been tested, so it stays.
//! * Linux / Windows: SDL2's `SDL_JoystickRumble` (evdev `FF_RUMBLE` / XInput / DirectInput rumble /
//!   HIDAPI), the two motors being the low- and high-frequency ones. It replaces `gilrs`' effect
//!   scheduler (50 ms ticks) one for one: a constant strong or weak magnitude and a timed pulse.
//!
//! No periodic waveform of the original's 6…35 Hz can be shaped, and a DirectInput constant force,
//! spring or condition has no counterpart: SDL's haptic subsystem could drive them on a wheel, but
//! that mapping (the original's `CreateEffect` parameters, `ff_drive`) is **not done yet** — see the
//! PA-SDL note in `RE/notes/controls.md`. The mapping onto motors is therefore a **PORT-DEVIATION
//! D24** and lives with the caller (`open_unleashed::app::force_feedback`); this module only
//! provides the mechanism.

#[cfg(target_os = "macos")]
pub use crate::ff_macos::Rumbler;

/// A sustained rumble: one effect per motor, played until it is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motor {
    /// the heavy, low-frequency motor
    Strong,
    /// the light, high-frequency motor
    Weak,
}

/// The rumble state: both sustained levels and the pulse in flight. SDL's rumble call sets both
/// motors at once for a duration, so every change re-sends the combined state.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct Rumbler {
    strong: f32,
    weak: f32,
    /// (end, level) of a pulse in flight
    pulse: Option<(std::time::Instant, f32)>,
    /// (low, high, when) of the last call sent
    sent: Option<(u16, u16, std::time::Instant)>,
}

/// How long one sustained call runs before SDL stops the motors by itself; renewed whenever the
/// caller touches the rumbler after half of it.
#[cfg(not(target_os = "macos"))]
const SUSTAIN_MS: u32 = 60_000;

#[cfg(not(target_os = "macos"))]
impl Rumbler {
    fn apply(&mut self, pads: &mut crate::gamepad::Devices) {
        let now = std::time::Instant::now();
        let pulse = match self.pulse {
            Some((end, level)) if end > now => level,
            _ => {
                self.pulse = None;
                0.0
            }
        };
        let to_u16 = |l: f32| (l.clamp(0.0, 1.0) * f32::from(u16::MAX)) as u16;
        let (low, high) = (to_u16(self.strong.max(pulse)), to_u16(self.weak));
        let fresh = matches!(self.sent, Some((l, h, t)) if l == low && h == high && t.elapsed().as_millis() < u128::from(SUSTAIN_MS / 2));
        if fresh || (self.sent.is_none() && low == 0 && high == 0) {
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
        } else if pads.rumble(low, high, ms) {
            self.sent = Some((low, high, now));
        }
    }

    /// Set a motor's sustained level, 0…1 (0 stops it).
    pub fn sustain(&mut self, pads: &mut crate::gamepad::Devices, motor: Motor, level: f32) {
        let level = level.clamp(0.0, 1.0);
        match motor {
            Motor::Strong => self.strong = level,
            Motor::Weak => self.weak = level,
        }
        self.apply(pads);
    }

    /// A one-shot strong pulse of `level` (0…1) for `ms` milliseconds.
    pub fn pulse(&mut self, pads: &mut crate::gamepad::Devices, level: f32, ms: u32) {
        let level = level.clamp(0.0, 1.0);
        if level <= 0.0 {
            return;
        }
        self.pulse = Some((std::time::Instant::now() + std::time::Duration::from_millis(u64::from(ms.max(1))), level));
        self.sent = None;
        self.apply(pads);
    }

    /// Stop both sustained motors (a pause, a menu, the end of the race).
    pub fn stop_all(&mut self, pads: &mut crate::gamepad::Devices) {
        self.strong = 0.0;
        self.weak = 0.0;
        self.pulse = None;
        self.apply(pads);
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
