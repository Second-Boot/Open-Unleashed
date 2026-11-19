//! The rumble / haptic face of [`Gamepads`] and the by-hand rumble self-test (`NFS_FF_TEST`, TEST GLUE).

use super::*;

#[derive(Clone, Copy)]
enum TestAction {
    Sustain(crate::ff::Motor, f32),
    Pulse,
    Silence,
}

struct TestStep {
    index: usize,
    label: &'static str,
    action: TestAction,
}

/// The `NFS_FF_TEST` sequence's clock (see [`Gamepads::rumble_test_tick`]).
pub(super) struct RumbleTest {
    start: std::time::Instant,
    current: Option<usize>,
    /// the step just entered this call (log it, fire its pulse)
    entered: bool,
}

impl Default for RumbleTest {
    fn default() -> Self {
        Self { start: std::time::Instant::now(), current: None, entered: false }
    }
}

impl RumbleTest {
    /// (label, action, seconds)
    const STEPS: [(&'static str, TestAction, f32); 9] = [
        ("Strong 0.3", TestAction::Sustain(crate::ff::Motor::Strong, 0.3), 1.0),
        ("Strong 1.0", TestAction::Sustain(crate::ff::Motor::Strong, 1.0), 1.0),
        ("Weak 0.3", TestAction::Sustain(crate::ff::Motor::Weak, 0.3), 1.0),
        ("Weak 1.0", TestAction::Sustain(crate::ff::Motor::Weak, 1.0), 1.0),
        ("pause", TestAction::Silence, 0.5),
        ("Strong pulse 1.0 for 300 ms (1 of 3)", TestAction::Pulse, 1.0),
        ("Strong pulse 1.0 for 300 ms (2 of 3)", TestAction::Pulse, 1.0),
        ("Strong pulse 1.0 for 300 ms (3 of 3)", TestAction::Pulse, 1.0),
        ("end of the test", TestAction::Silence, f32::INFINITY),
    ];

    fn advance(&mut self) -> Option<TestStep> {
        let t = self.start.elapsed().as_secs_f32();
        let mut at = 0.0;
        let mut idx = Self::STEPS.len() - 1;
        for (i, (_, _, d)) in Self::STEPS.iter().enumerate() {
            if t < at + d {
                idx = i;
                break;
            }
            at += d;
        }
        if self.current == Some(idx) && idx == Self::STEPS.len() - 1 {
            return None;
        }
        self.entered = self.current != Some(idx);
        self.current = Some(idx);
        let (label, action, _) = Self::STEPS[idx];
        Some(TestStep { index: idx, label, action })
    }
}


impl Gamepads {
    /// Whether a pad that can rumble is connected (`SDL_PROP_JOYSTICK_CAP_RUMBLE_BOOLEAN`).
    pub fn rumble_supported(&self) -> bool {
        self.devices.as_ref().is_some_and(|d| self.rumbler.supported(d))
    }

    /// A motor's sustained level, 0…1; 0 stops it.
    pub fn rumble_sustain(&mut self, motor: crate::ff::Motor, level: f32) {
        if let Some(d) = self.devices.as_mut() {
            self.rumbler.sustain(d, motor, level);
        }
    }

    /// A one-shot pulse of both motors.
    pub fn rumble_pulse(&mut self, level: f32, ms: u32) {
        if let Some(d) = self.devices.as_mut() {
            self.rumbler.pulse(d, level, ms);
        }
    }

    /// Stop the sustained motors.
    pub fn rumble_stop_all(&mut self) {
        if let Some(d) = self.devices.as_mut() {
            self.rumbler.stop_all(d);
        }
    }

    /// One line for the log: the rumble backend in use and the pad it drives.
    pub fn rumble_describe(&self) -> String {
        match self.devices.as_ref() {
            Some(d) => self.rumbler.describe(d),
            None => "no joystick support".to_string(),
        }
    }

    /// The first connected device SDL can drive force-feedback **effects** on (a wheel or another
    /// non-pad joystick with an SDL haptic device), with 0x412D5A's class: `Some(true)` for a wheel
    /// (`SDL_JOYSTICK_TYPE_WHEEL`, DirectInput's `DIDEVTYPEJOYSTICK_WHEEL`). `None` without one.
    pub fn haptic_device(&mut self) -> Option<bool> {
        self.devices.as_mut()?.haptic_device().map(|(_, h)| h.wheel)
    }

    /// Arm the haptic device for a race (0x411D20 / 0x5333A0: the device gain from
    /// `FF.TotalVolume`, every effect stopped); from now on [`haptic_call`](Self::haptic_call)
    /// drives it. Returns its class as [`haptic_device`](Self::haptic_device) does; `None` (nothing
    /// armed, the pad's rumble in use) without one.
    pub fn haptic_arm(&mut self, total: i32) -> Option<bool> {
        self.haptic_armed = false;
        let (_, h) = self.devices.as_mut().and_then(Devices::haptic_device)?;
        h.stop_all();
        h.set_gain(total);
        self.haptic_armed = true;
        Some(h.wheel)
    }

    /// Whether the race's force feedback goes to the haptic device ([`haptic_arm`](Self::haptic_arm)).
    pub fn haptic_armed(&self) -> bool {
        self.haptic_armed
    }

    /// One of the original's effect calls (0x533910 / 0x5338D0) on the armed haptic device.
    pub fn haptic_call(&mut self, call: &crate::ff::DiCall) {
        if let Some((_, h)) = self.devices.as_mut().and_then(Devices::haptic_device) {
            h.call(call);
        }
    }

    /// Stop every effect of the haptic device.
    pub fn haptic_stop_all(&mut self) {
        if let Some((_, h)) = self.devices.as_mut().and_then(Devices::haptic_device) {
            h.stop_all();
        }
    }

    /// One line for the log: the haptic device and the effects it supports.
    pub fn haptic_describe(&mut self) -> String {
        match self.devices.as_mut().and_then(Devices::haptic_device) {
            Some((n, h)) => format!("SDL haptic effects (constant / square / sine / spring, features {:#x}), device \"{n}\"{}", h.features, if h.wheel { ", a wheel" } else { "" }),
            None => "SDL haptic effects, no device".to_string(),
        }
    }

    /// **TEST GLUE** (`NFS_FF_TEST`, not in the original): a by-hand check of the rumble
    /// backend. Call once per frame; the first call starts a fixed sequence — Strong 0.3 for 1 s,
    /// Strong 1.0 for 1 s, Weak 0.3 for 1 s, Weak 1.0 for 1 s, then three Strong pulses (1.0, 300 ms,
    /// 0.5 s apart) — and every step is logged with the backend's description. It runs once.
    pub fn rumble_test_tick(&mut self) {
        let test = self.rumble_test.get_or_insert_with(RumbleTest::default);
        let Some(step) = test.advance() else { return };
        let log = test.entered;
        if log {
            tracing::info!("NFS_FF_TEST step {}: {} — backend: {}", step.index, step.label, self.rumble_describe());
        }
        match step.action {
            TestAction::Sustain(motor, level) => {
                self.rumble_sustain(motor, level);
                self.rumble_sustain(if motor == crate::ff::Motor::Strong { crate::ff::Motor::Weak } else { crate::ff::Motor::Strong }, 0.0);
            }
            TestAction::Pulse => {
                if log {
                    self.rumble_pulse(1.0, 300);
                }
            }
            TestAction::Silence => self.rumble_stop_all(),
        }
    }
}
