//! Input: physical device → native macOS events → logical game actions.
//!
//! The action set is the original's in-race controls ([`Action`]) and each reads through its
//! binding as the input packer 0x412E70 does (`RE/notes/controls.md`, audit 2026-09-26); the
//! defaults are 0x51D1D0's nineteen `Controller.*` rows over the shipped `fe.txt`. A key reads
//! 255, so a key's steering is ±63 in the packet (±126 on the car) and its throttle and brake
//! 0x7E (252 on the car) — full scale; the ±12-a-tick steering ramp a key gets (`.RAMPS`) is the
//! physics'.

pub mod bindings;
pub mod ff;
#[cfg(target_os = "macos")]
mod ff_macos;
pub mod gamepad;

use std::collections::HashSet;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// The original's in-race controls, each read through its binding (option ids 0x14E…0x21D,
/// `[0x5E9130 + id·4]`) by the input packer 0x412E70 (`RE/notes/controls.md`, audit 2026-09-26).
/// Three kinds of reading:
///
/// * **the packet's axes and buttons** (0x412F04…0x41300B): steering, throttle, brake, and bits
///   0 / 1 of byte 0 for the handbrake and the horn — levels;
/// * **the commands** (0x41312F…0x41326B), a code in bits 3..7 of byte 0 on the press: 2 shift up,
///   3 shift down, 4 abort, 5 reset, 6 AI drive, 10 / 11 the turn signals, 12 the siren (the
///   hazards), 13 the lights — edges;
/// * **the interface bits** (0x413744…0x4137A0), bit `id − 0x1EE` of the word the packer files in
///   the ring `0x5E954C`, read by `RaceFrame` through 0x4138D0 — on the press (edge) for 8 pause,
///   0x0A…0x0C the pause orbit's step, 0x0E the HUD hide, 0x11 / 0x12 the sound and music, 0x17
///   the camera, 0x18…0x1F the HUD toggles (0x415866…0x415ABE); held (level) for 0x13…0x16, the
///   looks (0x415AFA…0x415B2C).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    SteerLeft,
    SteerRight,
    Throttle,
    Brake,
    Handbrake,
    ShiftUp,
    ShiftDown,
    /// `I_CYCLECAMERA`, bit 0x17
    CameraNext,
    /// `I_LOOKBEHIND`, bit 0x13
    RearView,
    /// `I_PAUSE`, bit 8
    Pause,
    /// Reset the car onto the track (`C1_RESET`, command 5).
    Reset,
    /// `C1_HORN`, packet bit 1
    Horn,
    /// `C1_ABORT`, command 4 — nothing in this build reads the code (`car+0xD8B`)
    Abort,
    /// `C1_AIDRIVE`, command 6 — nothing in this build reads the code
    AiDrive,
    /// `C1_SIGNALLEFT`, command 10 (`Controller.LeftTurnSignal`)
    SignalLeft,
    /// `C1_SIGNALRIGHT`, command 11 (`Controller.RightTurnSignal`)
    SignalRight,
    /// `C1_SIREN`, command 12 (`Controller.Hazards`)
    Hazards,
    /// `C1_LIGHTSON`, command 13 (`Controller.Lights`)
    Lights,
    /// `I_LOOKLEFT`, bit 0x14
    LookLeft,
    /// `I_LOOKRIGHT`, bit 0x15
    LookRight,
    /// `I_LOOKFORWARD`, bit 0x16
    LookForward,
    /// `I_TOGGLEPAUSEMENU`, bit 0x0E — 0x425750, the HUD hide `[0x60CA00]`
    TogglePauseMenu,
    /// `I_ROTCAMCCLOCK`, bit 0x0A — 0x461790, the pause orbit's step +3
    RotCamCounter,
    /// `I_ROTCAMSTOP`, bit 0x0B — 0x4617B0, step 0
    RotCamStop,
    /// `I_ROTCAMCLOCK`, bit 0x0C — 0x4617A0, step −3
    RotCamClock,
    /// `I_TOGGLEHUD`, bit 0x18 — 0x4273B0
    ToggleHud,
    /// `I_TOGGLEREARVIEW`, bit 0x1A — 0x427550
    ToggleRearView,
    /// `I_TOGGLEMAP`, bit 0x1B — 0x427360
    ToggleMap,
    /// `I_HUDTRANSPARENCY`, bit 0x1F — 0x4275D0
    HudTransparency,
    /// `I_MUSIC`, bit 0x12 — 0x4AD790
    Music,
    /// `I_SFX`, bit 0x11 — 0x4AB4E0
    Sfx,
    /// `I_CYCLEHUD`, bit 0x19 — 0x427AC0, which answers 0 whatever the HUD: nothing happens
    CycleHud,
    /// `I_CYCLEMAP`, bit 0x1C — 0x426740
    CycleMap,
}

const ALL_ACTIONS: [Action; 33] = [
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
    Action::Pause,
    Action::Abort,
    Action::AiDrive,
    Action::SignalLeft,
    Action::SignalRight,
    Action::Hazards,
    Action::Lights,
    Action::LookLeft,
    Action::LookRight,
    Action::LookForward,
    Action::TogglePauseMenu,
    Action::RotCamCounter,
    Action::RotCamStop,
    Action::RotCamClock,
    Action::ToggleHud,
    Action::ToggleRearView,
    Action::ToggleMap,
    Action::HudTransparency,
    Action::Music,
    Action::Sfx,
    Action::CycleHud,
    Action::CycleMap,
];

/// The original's default key of each action, used only for an action with no binding at all
/// ([`bindings`]; with the shipped `fe.txt` and 0x51D1D0's rows every action has one). The keys
/// are 0x51D1D0's keyboard half (English, `lang` 0 — `bindings::default_key`) for its nineteen
/// rows and the shipped `fe.txt` for the rest: `C1_ABORT = key,SC_Q`, `C1_AIDRIVE = key,SC_D`,
/// `I_TOGGLEPAUSEMENU` … `I_SFX` = `SC_F1` … `SC_F10`, `I_CYCLEHUD = key,SC_1`, `I_CYCLEMAP =
/// key,SC_2`. `I_VIEWDISTANCE`, `I_HORIZON` (`SC_F11`, `SC_F12`) and the map zoom (`SC_SUBTRACT`,
/// `SC_ADD`) name keys the `SC_*` table 0x5D6588 does not have, so they bind nothing. Escape is
/// no in-race key: no binding names `SC_ESC`, and nothing else reads its entry `[0x5DE029]` of
/// the key-down table. *(Corrected 2026-09-26: this map also had Escape for pause and the
/// port's own free-camera keys W / S / Q / E / PageUp / PageDown / Shift, which nothing read.)*
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
        KeyCode::KeyP => Action::Pause,
        KeyCode::KeyL => Action::Lights,
        KeyCode::KeyW => Action::SignalLeft,
        KeyCode::KeyE => Action::SignalRight,
        KeyCode::KeyT => Action::Hazards,
        KeyCode::KeyY => Action::LookLeft,
        KeyCode::KeyI => Action::LookRight,
        KeyCode::KeyU => Action::LookForward,
        KeyCode::KeyQ => Action::Abort,
        KeyCode::KeyD => Action::AiDrive,
        KeyCode::F1 => Action::TogglePauseMenu,
        KeyCode::F2 => Action::RotCamClock,
        KeyCode::F3 => Action::RotCamStop,
        KeyCode::F4 => Action::RotCamCounter,
        KeyCode::F5 => Action::ToggleHud,
        KeyCode::F6 => Action::ToggleRearView,
        KeyCode::F7 => Action::ToggleMap,
        KeyCode::F8 => Action::HudTransparency,
        KeyCode::F9 => Action::Music,
        KeyCode::F10 => Action::Sfx,
        KeyCode::Digit1 => Action::CycleHud,
        KeyCode::Digit2 => Action::CycleMap,
        _ => return None,
    })
}

/// The key bound to an action in the original's own key codes, the ones its key formatter
/// 0x51C050 names (`nfs_game::keyname`): a printable character is itself and an arrow is an
/// extended code in the high byte — `0x4800` up, `0x4B00` left, `0x4D00` right, `0x5000` down.
/// The reverse of [`default_binding`] for the nineteen rows the controller-setup screens show,
/// in the codes 0x51D1D0 binds them with (lower-case letters, which 0x51C050 prints as the
/// capitals, 0x51C1EE); other actions return `None`.
pub fn key_code_for(action: Action) -> Option<u32> {
    let c = |ch: u8| ch as u32;
    Some(match action {
        Action::SteerLeft => 0x4B00,
        Action::SteerRight => 0x4D00,
        Action::Throttle => 0x4800,
        Action::Brake => 0x5000,
        Action::Handbrake => 0x20,
        Action::ShiftUp => c(b'a'),
        Action::ShiftDown => c(b'z'),
        Action::CameraNext => c(b'c'),
        Action::Reset => c(b'r'),
        Action::RearView => c(b'b'),
        Action::Horn => c(b'h'),
        Action::Pause => c(b'p'),
        Action::Lights => c(b'l'),
        Action::SignalLeft => c(b'w'),
        Action::SignalRight => c(b'e'),
        Action::Hazards => c(b't'),
        Action::LookLeft => c(b'y'),
        Action::LookRight => c(b'i'),
        Action::LookForward => c(b'u'),
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

/// The binding id each action reads through (`bindings::id`).
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
        Action::Abort => C1_ABORT,
        Action::Reset => C1_RESET,
        Action::AiDrive => C1_AIDRIVE,
        Action::SignalLeft => C1_SIGNALLEFT,
        Action::SignalRight => C1_SIGNALRIGHT,
        Action::Hazards => C1_SIREN,
        Action::Lights => C1_LIGHTSON,
        Action::Pause => I_PAUSE,
        Action::RotCamCounter => I_ROTCAMCCLOCK,
        Action::RotCamStop => I_ROTCAMSTOP,
        Action::RotCamClock => I_ROTCAMCLOCK,
        Action::TogglePauseMenu => I_TOGGLEPAUSEMENU,
        Action::Sfx => I_SFX,
        Action::Music => I_MUSIC,
        Action::RearView => I_LOOKBEHIND,
        Action::LookLeft => I_LOOKLEFT,
        Action::LookRight => I_LOOKRIGHT,
        Action::LookForward => I_LOOKFORWARD,
        Action::CameraNext => I_CYCLECAMERA,
        Action::ToggleHud => I_TOGGLEHUD,
        Action::CycleHud => I_CYCLEHUD,
        Action::ToggleRearView => I_TOGGLEREARVIEW,
        Action::ToggleMap => I_TOGGLEMAP,
        Action::CycleMap => I_CYCLEMAP,
        Action::HudTransparency => I_HUDTRANSPARENCY,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `fe.txt`'s binding lines (lines 397…436) and 0x51D1D0's keyboard rows give
    /// every action a key, and it is the one [`default_binding`] names.
    #[test]
    fn every_action_has_its_default_key() {
        let fe = [
            ("I_PAUSE", "key,SC_P"),
            ("C1_RIGHT", "key,SC_RIGHT"),
            ("C1_LEFT", "key,SC_LEFT"),
            ("C1_GAS", "key,SC_UP"),
            ("C1_BRAKE", "key,SC_DOWN"),
            ("C1_HANDBRAKE", "key,SC_SPACE"),
            ("C1_SHIFTUP", "key,SC_A"),
            ("C1_SHIFTDOWN", "key,SC_Z"),
            ("C1_HORN", "key,SC_H"),
            ("C1_ABORT", "key,SC_Q"),
            ("C1_RESET", "key,SC_R"),
            ("C1_AIDRIVE", "key,SC_D"),
            ("I_CYCLECAMERA", "key,SC_C"),
            ("I_LOOKBEHIND", "key,SC_B"),
            ("I_TOGGLEPAUSEMENU", "key,SC_F1"),
            ("I_ROTCAMCLOCK", "key,SC_F2"),
            ("I_ROTCAMSTOP", "key,SC_F3"),
            ("I_ROTCAMCCLOCK", "key,SC_F4"),
            ("I_TOGGLEHUD", "key,SC_F5"),
            ("I_TOGGLEREARVIEW", "key,SC_F6"),
            ("I_TOGGLEMAP", "key,SC_F7"),
            ("I_HUDTRANSPARENCY", "key,SC_F8"),
            ("I_MUSIC", "key,SC_F9"),
            ("I_SFX", "key,SC_F10"),
            ("I_CYCLEHUD", "key,SC_1"),
            ("I_CYCLEMAP", "key,SC_2"),
        ];
        let mut s = InputState {
            bindings: bindings::Bindings::from_registry(|k| fe.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)),
            ..Default::default()
        };
        let row_action = |n: &str| {
            Some(match n.strip_prefix("Controller.")? {
                "Accel" => Action::Throttle,
                "Brake" => Action::Brake,
                "HandBrake" => Action::Handbrake,
                "SteerLeft" => Action::SteerLeft,
                "SteerRight" => Action::SteerRight,
                "Horn" => Action::Horn,
                "ShiftUp" => Action::ShiftUp,
                "ShiftDown" => Action::ShiftDown,
                "LookBehind" => Action::RearView,
                "Lights" => Action::Lights,
                "ResetCar" => Action::Reset,
                "CameraView" => Action::CameraNext,
                "Pause" => Action::Pause,
                "LeftTurnSignal" => Action::SignalLeft,
                "RightTurnSignal" => Action::SignalRight,
                "Hazards" => Action::Hazards,
                "LookLeft" => Action::LookLeft,
                "LookRight" => Action::LookRight,
                _ => Action::LookForward,
            })
        };
        s.default_controls(None, |n| row_action(n).and_then(binding_id));
        assert_eq!(s.bindings.rows.len(), 19);
        let codes = [
            KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::Space,
            KeyCode::KeyA, KeyCode::KeyZ, KeyCode::KeyC, KeyCode::KeyR, KeyCode::KeyB, KeyCode::KeyH,
            KeyCode::KeyP, KeyCode::KeyL, KeyCode::KeyW, KeyCode::KeyE, KeyCode::KeyT, KeyCode::KeyY,
            KeyCode::KeyI, KeyCode::KeyU, KeyCode::KeyQ, KeyCode::KeyD, KeyCode::F1, KeyCode::F2,
            KeyCode::F3, KeyCode::F4, KeyCode::F5, KeyCode::F6, KeyCode::F7, KeyCode::F8, KeyCode::F9,
            KeyCode::F10, KeyCode::Digit1, KeyCode::Digit2,
        ];
        assert_eq!(codes.len(), ALL_ACTIONS.len());
        for k in codes {
            let a = default_binding(k).unwrap();
            let b = s.bindings.get(binding_id(a).unwrap()).copied().unwrap_or_else(|| panic!("{a:?} unbound"));
            assert_eq!((b.kind, b.index), (3, scancode(k).unwrap() as u32), "{a:?}");
        }
        assert_eq!(default_binding(KeyCode::Escape), None);
    }
}
