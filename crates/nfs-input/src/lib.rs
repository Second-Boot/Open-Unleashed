//! Input: physical device → native macOS events → logical game actions.
//!
//! The action set mirrors the original's logical controls (see `RE/INPUT.md`); the default key
//! bindings below are the original PC defaults *as far as they are known* and are flagged where
//! unverified. Steering-ramp and dead-zone behaviour of the original are not yet recovered, so
//! digital keys map to full-scale axis values here.

pub mod bindings;
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
}

const ALL_ACTIONS: [Action; 11] = [
    Action::CameraNext,
    Action::RearView,
    Action::SteerLeft,
    Action::SteerRight,
    Action::Throttle,
    Action::Brake,
    Action::Handbrake,
    Action::Horn,
    Action::ShiftUp,
    Action::ShiftDown,
    Action::Reset,
];

/// The port's fixed keyboard map, used for every action without an `fe.txt` binding
/// ([`bindings`]). Its race keys agree with the shipped `fe.txt` (`C1_LEFT = key,SC_LEFT`, …,
/// `I_CYCLECAMERA = key,SC_C`, `I_LOOKBEHIND = key,SC_B`); the free-camera keys, `T` and `P`/`Esc`
/// are this port's own.
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

/// The key bound to an action in the original's own key codes, the ones its key formatter
/// 0x51C050 names (`nfs_game::keyname`): a printable character is itself and an arrow is an
/// extended code in the high byte — `0x4800` up, `0x4B00` left, `0x4D00` right, `0x5000` down.
/// The reverse of [`default_binding`] for the rows the controller-setup screens show; actions
/// with no row of their own return `None`. Letters are given upper case; 0x51C050 prints both
/// cases the same (0x51C1EE), so which one the original stores does not show on screen.
pub fn key_code_for(action: Action) -> Option<u32> {
    Some(match action {
        Action::SteerLeft => 0x4B00,
        Action::SteerRight => 0x4D00,
        Action::Throttle => 0x4800,
        Action::Brake => 0x5000,
        Action::Handbrake => 0x20,
        Action::ShiftUp => b'A' as u32,
        Action::ShiftDown => b'Z' as u32,
        Action::CameraNext => b'C' as u32,
        Action::Reset => b'R' as u32,
        Action::RearView => b'B' as u32,
        Action::Horn => b'H' as u32,
        _ => return None,
    })
}

/// The DirectInput scancode of a physical key — the set `fe.txt`'s `SC_*` names and the key-down
/// table `0x5DE028` index (`bindings::SC_NAMES`). Only the keys that table names.
pub fn scancode(k: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match k {
        Escape => 0x01, Digit1 => 0x02, Digit2 => 0x03, Digit3 => 0x04, Digit4 => 0x05, Digit5 => 0x06,
        Digit6 => 0x07, Digit7 => 0x08, Digit8 => 0x09, Digit9 => 0x0A, Digit0 => 0x0B, Minus => 0x0C,
        Equal => 0x0D, Backspace => 0x0E, Tab => 0x0F, KeyQ => 0x10, KeyW => 0x11, KeyE => 0x12,
        KeyR => 0x13, KeyT => 0x14, KeyY => 0x15, KeyU => 0x16, KeyI => 0x17, KeyO => 0x18, KeyP => 0x19,
        BracketLeft => 0x1A, BracketRight => 0x1B, Enter => 0x1C, ControlLeft | ControlRight => 0x1D,
        KeyA => 0x1E, KeyS => 0x1F, KeyD => 0x20, KeyF => 0x21, KeyG => 0x22, KeyH => 0x23, KeyJ => 0x24,
        KeyK => 0x25, KeyL => 0x26, Semicolon => 0x27, Quote => 0x28, Backquote => 0x29, ShiftLeft => 0x2A,
        KeyZ => 0x2C, KeyX => 0x2D, KeyC => 0x2E, KeyV => 0x2F, KeyB => 0x30, KeyN => 0x31, KeyM => 0x32,
        Comma => 0x33, Period => 0x34, Slash => 0x35, ShiftRight => 0x36, PrintScreen => 0x37,
        AltLeft | AltRight => 0x38, Space => 0x39, CapsLock => 0x3A, F1 => 0x3B, F2 => 0x3C, F3 => 0x3D,
        F4 => 0x3E, F5 => 0x3F, F6 => 0x40, F7 => 0x41, F8 => 0x42, F9 => 0x43, F10 => 0x44,
        NumLock => 0x45, ScrollLock => 0x46, Home => 0x47, ArrowUp => 0x48, PageUp => 0x49,
        NumpadSubtract => 0x4A, ArrowLeft => 0x4B, Numpad5 => 0x4C, ArrowRight => 0x4D, NumpadAdd => 0x4E,
        End => 0x4F, ArrowDown => 0x50, PageDown => 0x51, Insert => 0x52, Delete => 0x53,
        _ => return None,
    })
}

/// The binding id an action reads through, where the original has one (`bindings::id`).
pub fn binding_id(a: Action) -> Option<u16> {
    use bindings::id::*;
    Some(match a {
        Action::SteerLeft => C1_LEFT,
        Action::SteerRight => C1_RIGHT,
        Action::Throttle => C1_GAS,
        Action::Brake => C1_BRAKE,
        Action::Handbrake => C1_HANDBRAKE,
        Action::Horn => C1_HORN,
        Action::ShiftUp => C1_SHIFTUP,
        Action::ShiftDown => C1_SHIFTDOWN,
        Action::Reset => C1_RESET,
        Action::CameraNext => I_CYCLECAMERA,
        Action::RearView => I_LOOKBEHIND,
        _ => return None,
    })
}

/// Current logical input state, updated from window events and sampled once per tick.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    held: HashSet<Action>,
    pressed: HashSet<Action>,
    /// `fe.txt`'s `C1_*` key bindings; an action with one reads through it, the rest through
    /// [`default_binding`]
    pub bindings: bindings::Bindings,
    /// scancodes down, the port's copy of the key-down table `0x5DE028`
    keys_down: HashSet<u8>,
    /// the pads' readings in the original's record shape, refreshed by `Gamepads::poll`
    pub pads: Vec<bindings::PadRecord>,
    /// each axis's `<Axis>.Min` / `.Max` (0…0x80), what an axis binding's band is baked from
    pub axis_bands: [[i32; 2]; 8],
    pub mouse_delta: (f32, f32),
    /// Analogue axes from a gamepad (−1..1 steer, 0..1 throttle/brake); `None` when no pad is used.
    pub pad: Option<gamepad::PadAxes>,
}

impl InputState {
    pub fn key_event(&mut self, ev: &KeyEvent) {
        let PhysicalKey::Code(code) = ev.physical_key else { return };
        if let Some(sc) = scancode(code) {
            match ev.state {
                ElementState::Pressed => self.keys_down.insert(sc),
                ElementState::Released => self.keys_down.remove(&sc),
            };
            // an action bound in `fe.txt` is pressed by its binding's key and no other
            let bound = self.bindings.by_id.iter().filter(|(_, b)| b.kind == 3 && b.index == sc as u32).map(|(&i, _)| i).collect::<Vec<_>>();
            for i in bound {
                if let Some(a) = ALL_ACTIONS.iter().copied().find(|&a| binding_id(a) == Some(i)) {
                    match ev.state {
                        ElementState::Pressed if self.read(a) > 0x40 => {
                            if self.held.insert(a) {
                                self.pressed.insert(a);
                            }
                        }
                        ElementState::Pressed => {}
                        ElementState::Released => {
                            self.held.remove(&a);
                        }
                    }
                }
            }
        }
        let Some(action) = default_binding(code) else { return };
        if binding_id(action).is_some_and(|i| self.bindings.get(i).is_some()) {
            return;
        }
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
        // a bound action's key reads 0 while Shift, Ctrl or Alt is down (0x411C5F); what else
        // holds it — a pad button, the autotest's own presses — goes through `press` as before
        if self.pad_bound(a) && self.read(a) > 0x40 {
            return true;
        }
        let gated = binding_id(a).is_some_and(|i| self.bindings.get(i).is_some_and(|b| b.kind == 3)) && self.modifier_down();
        self.held.contains(&a) && !gated
    }
    /// New pad readings: an action bound to a pad control is pressed on the tick its read goes
    /// above 0x40 (the packer's threshold, 0x412FE4), which is what `take_pressed` hands out.
    pub fn set_pads(&mut self, pads: Vec<bindings::PadRecord>) {
        let before: Vec<Action> = ALL_ACTIONS.iter().copied().filter(|&a| self.pad_bound(a) && self.read(a) > 0x40).collect();
        self.pads = pads;
        for a in ALL_ACTIONS {
            if self.pad_bound(a) && self.read(a) > 0x40 && !before.contains(&a) {
                self.pressed.insert(a);
            }
        }
    }
    /// **0x51D1D0** for pad `pad` of [`pads`](Self::pads), committed over the bindings
    /// ([`bindings::Bindings::set_defaults`]). No pad, or one that is not there, is the device
    /// 0x51F230 answers for a row it does not find — **0**, which is not analogue, so every row
    /// gets its key. The language is **0**, English: the one this port reads everything in.
    /// `id_of` names the option id behind a `Controller.*` row.
    pub fn default_controls(&mut self, pad: Option<usize>, id_of: impl Fn(&str) -> Option<u16>) {
        let (device, caps) = pad
            .and_then(|p| self.pads.get(p).map(|r| (p as u32, r.caps())))
            .unwrap_or((0, bindings::DeviceCaps::default()));
        self.bindings.set_defaults(&bindings::default_bindings(device, caps, 0), id_of);
    }
    /// How many keys are down — 0x51C610 takes a key for a binding row only when exactly one is.
    pub fn keys_down_count(&self) -> usize {
        self.keys_down.len()
    }
    fn modifier_down(&self) -> bool {
        [0x2A, 0x36, 0x1D, 0x38].iter().any(|s| self.keys_down.contains(s))
    }
    /// An action's 0…255 read through its binding by the kind's reader (the records at
    /// `0x5CC5C8 + 12·kind`): `Analog` 0x411B80, `Button` 0x411A50, `Key` 0x411C20, `Pov`
    /// 0x411AC0. 0 for an action with no binding, or a binding of kind 0.
    pub fn read(&self, a: Action) -> u8 {
        let Some(b) = binding_id(a).and_then(|i| self.bindings.get(i)) else { return 0 };
        match b.kind {
            1 => bindings::axis_read(b, &self.pads, &self.axis_bands),
            2 => bindings::button_read(b, &self.pads),
            3 => {
                let down = |s: u8| self.keys_down.contains(&s);
                let m = bindings::Modifiers {
                    shift: down(0x2A) || down(0x36),
                    ctrl: down(0x1D),
                    alt: down(0x38),
                };
                bindings::key_read(b, down, m)
            }
            5 => bindings::pov_read(b, &self.pads),
            _ => 0,
        }
    }
    /// Whether an action is bound to a pad control (kinds 1, 2, 5), which the packer then reads
    /// through its reader rather than through a held key.
    pub fn pad_bound(&self, a: Action) -> bool {
        binding_id(a).and_then(|i| self.bindings.get(i)).is_some_and(|b| matches!(b.kind, 1 | 2 | 5))
    }
    /// `.RAMPS` / `.RAMPG` / `.RAMPB` — 0x51CC00 writes `kind != 1` for the `C1_RIGHT`, `C1_GAS`
    /// and `C1_BRAKE` bindings into option ids 0x222 / 0x223 / 0x224 (0x51CF54…0x51CFFC): every
    /// control but an axis is ramped. Without a binding the port's keyboard is digital.
    pub fn ramps(&self) -> (bool, bool, bool) {
        let digital = |a: Action| binding_id(a).and_then(|i| self.bindings.get(i)).is_none_or(|b| b.kind != 1);
        (digital(Action::SteerRight), digital(Action::Throttle), digital(Action::Brake))
    }
    /// Edge-triggered: true once per key press.
    pub fn take_pressed(&mut self, a: Action) -> bool {
        self.pressed.remove(&a)
    }
    /// −1..1 steering: gamepad axis if present, else digital keys (no ramp yet — see module docs).
    pub fn steer(&self) -> f32 {
        // the packer's steering, `(C1_RIGHT − C1_LEFT) / 4` (0x412F39), when a pad control is bound
        // to either side; handed on doubled, in the −1…1 where 126 is full lock
        if self.pad_bound(Action::SteerLeft) || self.pad_bound(Action::SteerRight) {
            let r = self.read(Action::SteerRight).max(if self.held(Action::SteerRight) { 255 } else { 0 }) as i32;
            let l = self.read(Action::SteerLeft).max(if self.held(Action::SteerLeft) { 255 } else { 0 }) as i32;
            return ((r - l) / 4 * 2) as f32 / 126.0;
        }
        let keys = (self.held(Action::SteerRight) as i32 - self.held(Action::SteerLeft) as i32) as f32;
        match self.pad {
            Some(p) if keys == 0.0 => p.steer,
            _ => keys,
        }
    }
    pub fn throttle(&self) -> f32 {
        // `C1_GAS / 2`, capped at 0x7E (0x412F66, 0x412F9A)
        if self.pad_bound(Action::Throttle) {
            return (self.read(Action::Throttle) / 2).min(0x7E) as f32 / 126.0;
        }
        let k = self.held(Action::Throttle) as i32 as f32;
        self.pad.map(|p| p.throttle.max(k)).unwrap_or(k)
    }
    pub fn brake(&self) -> f32 {
        // `C1_BRAKE / 2`, capped at 0x7E (0x412F89, 0x412FB0)
        if self.pad_bound(Action::Brake) {
            return (self.read(Action::Brake) / 2).min(0x7E) as f32 / 126.0;
        }
        let k = self.held(Action::Brake) as i32 as f32;
        self.pad.map(|p| p.brake.max(k)).unwrap_or(k)
    }
    /// Call at the end of each tick.
    pub fn end_tick(&mut self) {
        self.pressed.clear();
        self.mouse_delta = (0.0, 0.0);
    }
}
