//! Input: physical device → native macOS events → logical game actions.
//!
//! The action set mirrors the original's logical controls (see `RE/INPUT.md`); the default key
//! bindings below are the original PC defaults *as far as they are known* and are flagged where
//! unverified. Steering-ramp and dead-zone behaviour of the original are not yet recovered, so
//! digital keys map to full-scale axis values here.

pub mod gamepad;

use std::collections::HashSet;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    SteerLeft,
    SteerRight,
    Throttle,
    Brake,
    Handbrake,
    ShiftUp,
    ShiftDown,
    CameraNext,
    RearView,
    Pause,
    // free-camera / debug
    FreeForward,
    FreeBack,
    FreeLeft,
    FreeRight,
    FreeUp,
    FreeDown,
    FreeFast,
    /// Reset the car onto the track (original "reset car" mode 3).
    Reset,
    Horn,
    /// Cycle the `SPLITTIMES` display (record lap / best lap / last lap).
    SplitTimes,
}

/// Default keyboard bindings. Original defaults (arrow keys steer/throttle/brake, A/Z shift,
/// space handbrake, C camera) are MEDIUM CONFIDENCE from the original readme "Controls" section
/// and will be checked against the `.ctr`/registry data.
pub fn default_binding(key: KeyCode) -> Option<Action> {
    Some(match key {
        KeyCode::ArrowLeft => Action::SteerLeft,
        KeyCode::ArrowRight => Action::SteerRight,
        KeyCode::ArrowUp => Action::Throttle,
        KeyCode::ArrowDown => Action::Brake,
        KeyCode::Space => Action::Handbrake,
        KeyCode::KeyA => Action::ShiftUp,
        KeyCode::KeyZ => Action::ShiftDown,
        KeyCode::KeyC => Action::CameraNext,
        KeyCode::KeyR => Action::Reset,
        KeyCode::KeyB => Action::RearView,
        KeyCode::KeyH => Action::Horn,
        KeyCode::KeyT => Action::SplitTimes,
        KeyCode::KeyP | KeyCode::Escape => Action::Pause,
        KeyCode::KeyW => Action::FreeForward,
        KeyCode::KeyS => Action::FreeBack,
        KeyCode::KeyQ => Action::FreeLeft,
        KeyCode::KeyE => Action::FreeRight,
        KeyCode::PageUp => Action::FreeUp,
        KeyCode::PageDown => Action::FreeDown,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Action::FreeFast,
        _ => return None,
    })
}

/// The key bound to an action, as the key-binding screen would print it. The reverse of
/// [`default_binding`]; actions with no key of their own return `None`.
pub fn key_for(action: Action) -> Option<&'static str> {
    Some(match action {
        Action::SteerLeft => "LEFT ARROW",
        Action::SteerRight => "RIGHT ARROW",
        Action::Throttle => "UP ARROW",
        Action::Brake => "DOWN ARROW",
        Action::Handbrake => "SPACE",
        Action::ShiftUp => "A",
        Action::ShiftDown => "Z",
        Action::CameraNext => "C",
        Action::Reset => "R",
        Action::RearView => "B",
        Action::Horn => "H",
        Action::SplitTimes => "T",
        Action::Pause => "ESC",
        _ => return None,
    })
}

/// Current logical input state, updated from window events and sampled once per tick.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    held: HashSet<Action>,
    pressed: HashSet<Action>,
    pub mouse_delta: (f32, f32),
    /// Analogue axes from a gamepad (−1..1 steer, 0..1 throttle/brake); `None` when no pad is used.
    pub pad: Option<gamepad::PadAxes>,
}

impl InputState {
    pub fn key_event(&mut self, ev: &KeyEvent) {
        let PhysicalKey::Code(code) = ev.physical_key else { return };
        let Some(action) = default_binding(code) else { return };
        match ev.state {
            ElementState::Pressed => {
                if self.held.insert(action) {
                    self.pressed.insert(action);
                }
            }
            ElementState::Released => {
                self.held.remove(&action);
            }
        }
    }
    pub fn press(&mut self, action: Action) {
        if self.held.insert(action) {
            self.pressed.insert(action);
        }
    }
    pub fn release(&mut self, action: Action) {
        self.held.remove(&action);
    }
    pub fn mouse_motion(&mut self, dx: f64, dy: f64) {
        self.mouse_delta.0 += dx as f32;
        self.mouse_delta.1 += dy as f32;
    }
    pub fn held(&self, a: Action) -> bool {
        self.held.contains(&a)
    }
    /// Edge-triggered: true once per key press.
    pub fn take_pressed(&mut self, a: Action) -> bool {
        self.pressed.remove(&a)
    }
    /// −1..1 steering: gamepad axis if present, else digital keys (no ramp yet — see module docs).
    pub fn steer(&self) -> f32 {
        let keys = (self.held(Action::SteerRight) as i32 - self.held(Action::SteerLeft) as i32) as f32;
        match self.pad {
            Some(p) if keys == 0.0 => p.steer,
            _ => keys,
        }
    }
    pub fn throttle(&self) -> f32 {
        let k = self.held(Action::Throttle) as i32 as f32;
        self.pad.map(|p| p.throttle.max(k)).unwrap_or(k)
    }
    pub fn brake(&self) -> f32 {
        let k = self.held(Action::Brake) as i32 as f32;
        self.pad.map(|p| p.brake.max(k)).unwrap_or(k)
    }
    /// Call at the end of each tick.
    pub fn end_tick(&mut self) {
        self.pressed.clear();
        self.mouse_delta = (0.0, 0.0);
    }
}
