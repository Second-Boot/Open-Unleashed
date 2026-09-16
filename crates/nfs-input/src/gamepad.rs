//! Gamepad input via `gilrs` (macOS Game Controller/IOKit). Mapped onto the same logical
//! actions as the keyboard. Dead-zone value is a native default; the original's controller
//! dead-zone setting (`FEControllerDeadZoneControl`) is applied once recovered.

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
        Self { gilrs, axes: PadAxes::default(), dead_zone: 0.12 }
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
        Button::DPadLeft => Action::SteerLeft,
        Button::DPadRight => Action::SteerRight,
        Button::DPadUp => Action::Throttle,
        Button::DPadDown => Action::Brake,
        _ => return None,
    })
}
