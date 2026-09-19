//! Gamepad input via `gilrs` (macOS Game Controller/IOKit). Mapped onto the same logical
//! actions as the keyboard.
//!
//! The dead zone comes from the front end's own `XAxis.Min`, as the original's does. The binding
//! builder around 0x51CC70 switches on the axis index through the jump table at 0x51D028, looks
//! `<Axis>.Min` and `<Axis>.Max` up by name (0x4B7AB0 plus a `dynamic_cast` to 0x5DD3D8) and bakes
//! them into the binding descriptor as two bytes about the centre **0x80**:
//!
//! ```text
//! 0x51CEA4  lo = max(min * dir + 0x80, 0)
//! 0x51CED1  hi = min(max * dir + 0x80, 0xFF)
//! 0x51CEFD  descriptor = ((((dev << 4 | axis) << 8 | lo) << 8 | hi) << 8) | flags
//! ```
//!
//! so on a 0…0xFF axis centred at 0x80, `min` is the **dead-zone half-width** and `max` the
//! **saturation** one — which is what the mirrored bar on `deadzone.lay` draws. The factory value
//! is **0** (0x51E546), so out of the box the original has no dead zone at all and the player sets
//! one on that screen; this port used to invent 0.12 instead.

use crate::{Action, InputState};
use gilrs::{Axis, Button, EventType, Gilrs};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadAxes {
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
}

pub struct Gamepads {
    gilrs: Option<Gilrs>,
    axes: PadAxes,
    dead_zone: f32,
}

impl Gamepads {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                for (_, gp) in g.gamepads() {
                    tracing::info!(name = gp.name(), "gamepad connected");
                }
                Some(g)
            }
            Err(e) => {
                tracing::warn!(error = %e, "gamepad support unavailable");
                None
            }
        };
        // `XAxis.Min`'s own default is 0 (0x51E546) — no dead zone until the player sets one
        Self { gilrs, axes: PadAxes::default(), dead_zone: 0.0 }
    }

    /// The steering dead zone, as `XAxis.Min` of the accessors' own 0…0x80 scaled to the −1…1 an
    /// axis reads here. The original bakes the same number into its binding descriptor as
    /// `0x80 ± min` (see the module comment).
    pub fn set_dead_zone(&mut self, x_axis_min: i32) {
        self.dead_zone = (x_axis_min.clamp(0, 0x80) as f32) / 128.0;
    }

    /// Drain gamepad events into the logical input state. Call once per frame.
    pub fn poll(&mut self, input: &mut InputState) {
        let Some(g) = self.gilrs.as_mut() else { return };
        let mut touched = false;
        while let Some(ev) = g.next_event() {
            touched = true;
            match ev.event {
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    self.axes.steer = if v.abs() < self.dead_zone { 0.0 } else { v };
                }
                EventType::ButtonChanged(Button::RightTrigger2, v, _) => self.axes.throttle = v,
                EventType::ButtonChanged(Button::LeftTrigger2, v, _) => self.axes.brake = v,
                EventType::ButtonPressed(b, _) => {
                    if let Some(a) = map_button(b) {
                        input.press(a);
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if let Some(a) = map_button(b) {
                        input.release(a);
                    }
                }
                _ => {}
            }
        }
        if touched || input.pad.is_some() {
            input.pad = Some(self.axes);
        }
    }
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

fn map_button(b: Button) -> Option<Action> {
    Some(match b {
        Button::South => Action::Throttle,
        Button::East => Action::Brake,
        Button::West => Action::Handbrake,
        Button::RightTrigger => Action::ShiftUp,
        Button::LeftTrigger => Action::ShiftDown,
        Button::North => Action::CameraNext,
        Button::Start => Action::Pause,
        Button::Select => Action::RearView,
        Button::RightThumb => Action::Horn,
        Button::DPadLeft => Action::SteerLeft,
        Button::DPadRight => Action::SteerRight,
        Button::DPadUp => Action::Throttle,
        Button::DPadDown => Action::Brake,
        _ => return None,
    })
}

/// One connected pad's axis set, as the front end's dead-zone screen needs it.
///
/// The original reads this out of DirectInput: `0x532E10(i, 2)->[+8]` is an **axis bitmask** whose
/// bit `n` is axis `n` of `DIJOYSTATE` — `lX, lY, lZ, lRx, lRy, lRz, rglSlider[0], rglSlider[1]` —
/// and `[rec+4 + 4n]` is that axis's current value (`RE/notes/controls.md` §4.17). `gilrs` reports
/// the same HID axis usages under its own names, so the eight map across one for one.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadAxisSet {
    /// bit `n` set when the pad has axis `n`, in `DIJOYSTATE` order
    pub mask: u8,
    /// each axis's current value, scaled to the 0…0x80 the dead-zone accessors use
    pub values: [i32; 8],
}

impl Gamepads {
    /// The connected pads' axis sets, in connection order — what `NFS5.DEADZONE` draws from.
    pub fn axis_sets(&self) -> Vec<PadAxisSet> {
        let Some(g) = self.gilrs.as_ref() else { return Vec::new() };
        // `DIJOYSTATE`'s eight axes against the HID usages `gilrs` names them by
        const ORDER: [Axis; 6] =
            [Axis::LeftStickX, Axis::LeftStickY, Axis::LeftZ, Axis::RightStickX, Axis::RightStickY, Axis::RightZ];
        g.gamepads()
            .map(|(_, gp)| {
                let mut set = PadAxisSet::default();
                for (n, a) in ORDER.iter().enumerate() {
                    if let Some(d) = gp.axis_data(*a) {
                        set.mask |= 1 << n;
                        // `[rec+4+4n]` is the **device's** own reading, not the accessor's: the
                        // image sets no `DIPROP_RANGE` (0x56FD70's only `SetProperty` is
                        // `DIPROP_BUFFERSIZE`), so an absolute axis keeps DirectInput's default
                        // 0…65535 with the centre at 32767, which is the range `NFS5.DEADZONE`'s
                        // marker divides by 65536 (0x51B819). `gilrs` reads -1…1.
                        set.values[n] = (((d.value() + 1.0) * 0.5 * 65535.0) as i32).clamp(0, 0xFFFF);
                    }
                }
                set
            })
            .collect()
    }
}
