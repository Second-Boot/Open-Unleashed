//! PORT-DEVIATION D46 (photo mode): the first pad read straight from the device.

use super::*;

use super::axis::{trigger, unit};

/// PORT-DEVIATION D46 (photo mode): the first pad's sticks, triggers and the buttons the free camera uses, read
/// straight from the device (not through the actions: the camera takes them while the game is paused).
/// Sticks are -1..1 with up positive and a dead zone of 0.15 taken off; triggers 0..1.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhotoPad {
    pub left: (f32, f32),
    pub right: (f32, f32),
    pub trigger_left: f32,
    pub trigger_right: f32,
    /// A, B, X, Y (south, east, west, north), the shoulders, the d-pad, the left stick's click and Start
    pub south: bool,
    pub east: bool,
    pub west: bool,
    pub north: bool,
    pub bumper_left: bool,
    pub bumper_right: bool,
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub left_thumb: bool,
    pub start: bool,
}

/// A stick axis with the dead zone taken off and the rest rescaled to -1..1.
pub fn photo_deadzone(v: f32) -> f32 {
    const DZ: f32 = 0.15;
    if v.abs() <= DZ { 0.0 } else { (v - DZ * v.signum()) / (1.0 - DZ) }
}

impl Gamepads {
    /// PORT-DEVIATION D46: the first connected pad's state for the photo mode's free camera, `None` without a pad.
    pub fn photo_pad(&self) -> Option<PhotoPad> {
        let p = self.pad_snapshot()?;
        // SDL's stick Y grows downwards; the photo camera takes up as positive
        let ax = |v: i16| photo_deadzone(unit(v));
        let ay = |v: i16| photo_deadzone(-unit(v));
        Some(PhotoPad {
            left: (ax(p.sticks[0]), ay(p.sticks[1])),
            right: (ax(p.sticks[2]), ay(p.sticks[3])),
            trigger_left: trigger(p.triggers[0]),
            trigger_right: trigger(p.triggers[1]),
            south: p.button(Button::South),
            east: p.button(Button::East),
            west: p.button(Button::West),
            north: p.button(Button::North),
            bumper_left: p.button(Button::LeftTrigger),
            bumper_right: p.button(Button::RightTrigger),
            dpad_up: p.dpad[0],
            dpad_down: p.dpad[2],
            dpad_left: p.dpad[3],
            dpad_right: p.dpad[1],
            left_thumb: p.button(Button::LeftThumb),
            start: p.button(Button::Start),
        })
    }
}
