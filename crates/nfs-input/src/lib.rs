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

/// What the port keeps per action, one row each, **in the order of [`Action`]'s variants** (so
/// `ACTIONS[a as usize]` is `a`'s row — a test checks it): the registry name and option id its
/// binding reads through (`bindings::id`), the key it reads when it has no binding at all
/// ([`default_binding`]) and, for the nineteen rows the controller-setup screens show, the
/// original's own code for that key ([`key_code_for`]).
struct ActionDef {
    action: Action,
    /// `fe.txt`'s name of the binding
    fe_name: &'static str,
    /// the option id (`[0x5E9130 + id·4]`)
    id: u16,
    default_key: KeyCode,
    key_code: Option<u32>,
}

const fn def(action: Action, fe_name: &'static str, id: u16, default_key: KeyCode, key_code: Option<u32>) -> ActionDef {
    ActionDef { action, fe_name, id, default_key, key_code }
}

const ACTIONS: [ActionDef; 33] = {
    use bindings::id::*;
    [
        def(Action::SteerLeft, "C1_LEFT", C1_LEFT, KeyCode::ArrowLeft, Some(0x4B00)),
        def(Action::SteerRight, "C1_RIGHT", C1_RIGHT, KeyCode::ArrowRight, Some(0x4D00)),
        def(Action::Throttle, "C1_GAS", C1_GAS, KeyCode::ArrowUp, Some(0x4800)),
        def(Action::Brake, "C1_BRAKE", C1_BRAKE, KeyCode::ArrowDown, Some(0x5000)),
        def(Action::Handbrake, "C1_HANDBRAKE", C1_HANDBRAKE, KeyCode::Space, Some(0x20)),
        def(Action::ShiftUp, "C1_SHIFTUP", C1_SHIFTUP, KeyCode::KeyA, Some(b'a' as u32)),
        def(Action::ShiftDown, "C1_SHIFTDOWN", C1_SHIFTDOWN, KeyCode::KeyZ, Some(b'z' as u32)),
        def(Action::CameraNext, "I_CYCLECAMERA", I_CYCLECAMERA, KeyCode::KeyC, Some(b'c' as u32)),
        def(Action::RearView, "I_LOOKBEHIND", I_LOOKBEHIND, KeyCode::KeyB, Some(b'b' as u32)),
        def(Action::Pause, "I_PAUSE", I_PAUSE, KeyCode::KeyP, Some(b'p' as u32)),
        def(Action::Reset, "C1_RESET", C1_RESET, KeyCode::KeyR, Some(b'r' as u32)),
        def(Action::Horn, "C1_HORN", C1_HORN, KeyCode::KeyH, Some(b'h' as u32)),
        def(Action::Abort, "C1_ABORT", C1_ABORT, KeyCode::KeyQ, None),
        def(Action::AiDrive, "C1_AIDRIVE", C1_AIDRIVE, KeyCode::KeyD, None),
        def(Action::SignalLeft, "C1_SIGNALLEFT", C1_SIGNALLEFT, KeyCode::KeyW, Some(b'w' as u32)),
        def(Action::SignalRight, "C1_SIGNALRIGHT", C1_SIGNALRIGHT, KeyCode::KeyE, Some(b'e' as u32)),
        def(Action::Hazards, "C1_SIREN", C1_SIREN, KeyCode::KeyT, Some(b't' as u32)),
        def(Action::Lights, "C1_LIGHTSON", C1_LIGHTSON, KeyCode::KeyL, Some(b'l' as u32)),
        def(Action::LookLeft, "I_LOOKLEFT", I_LOOKLEFT, KeyCode::KeyY, Some(b'y' as u32)),
        def(Action::LookRight, "I_LOOKRIGHT", I_LOOKRIGHT, KeyCode::KeyI, Some(b'i' as u32)),
        def(Action::LookForward, "I_LOOKFORWARD", I_LOOKFORWARD, KeyCode::KeyU, Some(b'u' as u32)),
        def(Action::TogglePauseMenu, "I_TOGGLEPAUSEMENU", I_TOGGLEPAUSEMENU, KeyCode::F1, None),
        def(Action::RotCamCounter, "I_ROTCAMCCLOCK", I_ROTCAMCCLOCK, KeyCode::F4, None),
        def(Action::RotCamStop, "I_ROTCAMSTOP", I_ROTCAMSTOP, KeyCode::F3, None),
        def(Action::RotCamClock, "I_ROTCAMCLOCK", I_ROTCAMCLOCK, KeyCode::F2, None),
        def(Action::ToggleHud, "I_TOGGLEHUD", I_TOGGLEHUD, KeyCode::F5, None),
        def(Action::ToggleRearView, "I_TOGGLEREARVIEW", I_TOGGLEREARVIEW, KeyCode::F6, None),
        def(Action::ToggleMap, "I_TOGGLEMAP", I_TOGGLEMAP, KeyCode::F7, None),
        def(Action::HudTransparency, "I_HUDTRANSPARENCY", I_HUDTRANSPARENCY, KeyCode::F8, None),
        def(Action::Music, "I_MUSIC", I_MUSIC, KeyCode::F9, None),
        def(Action::Sfx, "I_SFX", I_SFX, KeyCode::F10, None),
        def(Action::CycleHud, "I_CYCLEHUD", I_CYCLEHUD, KeyCode::Digit1, None),
        def(Action::CycleMap, "I_CYCLEMAP", I_CYCLEMAP, KeyCode::Digit2, None),
    ]
};

const _: () = assert!(ACTIONS.len() <= 64, "InputState keeps held / pressed in one u64 each");

/// Every action, in the order of [`ACTIONS`].
const ALL_ACTIONS: [Action; 33] = {
    let mut out = [Action::SteerLeft; 33];
    let mut i = 0;
    while i < out.len() {
        out[i] = ACTIONS[i].action;
        i += 1;
    }
    out
};

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
    ACTIONS.iter().find(|d| d.default_key == key).map(|d| d.action)
}

/// The key bound to an action in the original's own key codes, the ones its key formatter
/// 0x51C050 names (`nfs_game::keyname`): a printable character is itself and an arrow is an
/// extended code in the high byte — `0x4800` up, `0x4B00` left, `0x4D00` right, `0x5000` down.
/// The reverse of [`default_binding`] for the nineteen rows the controller-setup screens show,
/// in the codes 0x51D1D0 binds them with (lower-case letters, which 0x51C050 prints as the
/// capitals, 0x51C1EE); other actions return `None`.
pub fn key_code_for(action: Action) -> Option<u32> {
    ACTIONS[action as usize].key_code
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
        BracketLeft => 0x1A, BracketRight => 0x1B, Enter => 0x1C, ControlLeft | ControlRight => bindings::SC_CTRL,
        KeyA => 0x1E, KeyS => 0x1F, KeyD => 0x20, KeyF => 0x21, KeyG => 0x22, KeyH => 0x23, KeyJ => 0x24,
        KeyK => 0x25, KeyL => 0x26, Semicolon => 0x27, Quote => 0x28, Backquote => 0x29, ShiftLeft => bindings::SC_LSHIFT,
        KeyZ => 0x2C, KeyX => 0x2D, KeyC => 0x2E, KeyV => 0x2F, KeyB => 0x30, KeyN => 0x31, KeyM => 0x32,
        Comma => 0x33, Period => 0x34, Slash => 0x35, ShiftRight => bindings::SC_RSHIFT, PrintScreen => 0x37,
        AltLeft | AltRight => bindings::SC_ALT, Space => 0x39, CapsLock => 0x3A, F1 => 0x3B, F2 => 0x3C, F3 => 0x3D,
        F4 => 0x3E, F5 => 0x3F, F6 => 0x40, F7 => 0x41, F8 => 0x42, F9 => 0x43, F10 => 0x44,
        NumLock => 0x45, ScrollLock => 0x46, Home => 0x47, ArrowUp => 0x48, PageUp => 0x49,
        NumpadSubtract => 0x4A, ArrowLeft => 0x4B, Numpad5 => 0x4C, ArrowRight => 0x4D, NumpadAdd => 0x4E,
        End => 0x4F, ArrowDown => 0x50, PageDown => 0x51, Insert => 0x52, Delete => 0x53,
        _ => return None,
    })
}

/// The binding id each action reads through (`bindings::id`).
pub fn binding_id(a: Action) -> Option<u16> {
    Some(ACTIONS[a as usize].id)
}

/// The action whose binding has option id `id`.
fn action_of_id(id: u16) -> Option<Action> {
    ACTIONS.iter().find(|d| d.id == id).map(|d| d.action)
}

/// Current logical input state, updated from window events and sampled once per tick.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    /// the actions held / pressed since the last tick, one bit each by `Action as u8`
    held: u64,
    pressed: u64,
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

/// An action's bit in [`InputState`]'s held / pressed masks.
const fn bit(a: Action) -> u64 {
    1 << (a as u8)
}

impl InputState {
    /// The action goes down: held, and pressed when it was not held already.
    fn hold(&mut self, a: Action) {
        if self.held & bit(a) == 0 {
            self.held |= bit(a);
            self.pressed |= bit(a);
        }
    }

    fn unhold(&mut self, a: Action) {
        self.held &= !bit(a);
    }

    pub fn key_event(&mut self, ev: &KeyEvent) {
        let PhysicalKey::Code(code) = ev.physical_key else { return };
        self.key_changed(code, ev.state);
    }
    /// A physical key going down or up (`key_event` minus the winit event, which tests cannot build).
    fn key_changed(&mut self, code: KeyCode, state: ElementState) {
        if let Some(sc) = scancode(code) {
            match state {
                ElementState::Pressed => self.keys_down.insert(sc),
                ElementState::Released => self.keys_down.remove(&sc),
            };
            // an action bound in `fe.txt` is pressed by its binding's key and no other
            let bound = self.bindings.by_id.iter().filter(|(_, b)| b.kind_class() == bindings::BindingKind::Key && b.index == sc as u32).map(|(&i, _)| i).collect::<Vec<_>>();
            for i in bound {
                if let Some(a) = action_of_id(i) {
                    match state {
                        ElementState::Pressed if self.read(a) > 0x40 => self.hold(a),
                        ElementState::Pressed => {}
                        ElementState::Released => self.unhold(a),
                    }
                }
            }
        }
        let Some(action) = default_binding(code) else { return };
        if binding_id(action).is_some_and(|i| self.bindings.get(i).is_some()) {
            return;
        }
        match state {
            ElementState::Pressed => self.hold(action),
            ElementState::Released => self.unhold(action),
        }
    }
    pub fn press(&mut self, action: Action) {
        self.hold(action);
    }
    pub fn release(&mut self, action: Action) {
        self.unhold(action);
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
        let gated = binding_id(a).is_some_and(|i| self.bindings.get(i).is_some_and(|b| b.kind_class() == bindings::BindingKind::Key)) && self.modifier_down();
        self.held & bit(a) != 0 && !gated
    }
    /// New pad readings: an action bound to a pad control is pressed on the tick its read goes
    /// above 0x40 (the packer's threshold, 0x412FE4), which is what `take_pressed` hands out.
    pub fn set_pads(&mut self, pads: &[bindings::PadRecord]) {
        let before = self.pad_pressed_mask();
        self.pads.clear();
        self.pads.extend_from_slice(pads);
        self.pressed |= self.pad_pressed_mask() & !before;
    }
    /// The actions bound to a pad control whose read is above the packer's 0x40 threshold now.
    fn pad_pressed_mask(&self) -> u64 {
        ALL_ACTIONS.iter().filter(|&&a| self.pad_bound(a) && self.read(a) > 0x40).fold(0, |m, &a| m | bit(a))
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
        [bindings::SC_LSHIFT, bindings::SC_RSHIFT, bindings::SC_CTRL, bindings::SC_ALT].iter().any(|s| self.keys_down.contains(s))
    }
    /// An action's 0…255 read through its binding by the kind's reader (the records at
    /// `0x5CC5C8 + 12·kind`): `Analog` 0x411B80, `Button` 0x411A50, `Key` 0x411C20, `Pov`
    /// 0x411AC0. 0 for an action with no binding, or a binding of kind 0.
    pub fn read(&self, a: Action) -> u8 {
        let Some(b) = binding_id(a).and_then(|i| self.bindings.get(i)) else { return 0 };
        match b.kind_class() {
            bindings::BindingKind::Axis => bindings::axis_read(b, &self.pads, &self.axis_bands),
            bindings::BindingKind::Button => bindings::button_read(b, &self.pads),
            bindings::BindingKind::Key => {
                let down = |s: u8| self.keys_down.contains(&s);
                let m = bindings::Modifiers {
                    shift: down(bindings::SC_LSHIFT) || down(bindings::SC_RSHIFT),
                    ctrl: down(bindings::SC_CTRL),
                    alt: down(bindings::SC_ALT),
                };
                bindings::key_read(b, down, m)
            }
            bindings::BindingKind::Hat => bindings::pov_read(b, &self.pads),
            _ => 0,
        }
    }
    /// Whether an action is bound to a pad control (kinds 1, 2, 5), which the packer then reads
    /// through its reader rather than through a held key.
    pub fn pad_bound(&self, a: Action) -> bool {
        binding_id(a).and_then(|i| self.bindings.get(i)).is_some_and(|b| matches!(b.kind_class(), bindings::BindingKind::Axis | bindings::BindingKind::Button | bindings::BindingKind::Hat))
    }
    /// `.RAMPS` / `.RAMPG` / `.RAMPB` — 0x51CC00 writes `kind != 1` for the `C1_RIGHT`, `C1_GAS`
    /// and `C1_BRAKE` bindings into option ids 0x222 / 0x223 / 0x224 (0x51CF54…0x51CFFC): every
    /// control but an axis is ramped. Without a binding the port's keyboard is digital.
    pub fn ramps(&self) -> (bool, bool, bool) {
        let digital = |a: Action| binding_id(a).and_then(|i| self.bindings.get(i)).is_none_or(|b| b.kind_class() != bindings::BindingKind::Axis);
        (digital(Action::SteerRight), digital(Action::Throttle), digital(Action::Brake))
    }
    /// Edge-triggered: true once per key press.
    pub fn take_pressed(&mut self, a: Action) -> bool {
        let was = self.pressed & bit(a) != 0;
        self.pressed &= !bit(a);
        was
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
    /// The window lost or regained the keyboard focus (FOCUS-INPUT): forget every held key, action and
    /// pending press. PORT-DEVIATION D17 (PORT-DEVIATIONS.md): the release events of keys / modifiers
    /// held at that moment never arrive (macOS: Cmd+Shift+5 leaves Cmd / Shift "down").
    pub fn release_all(&mut self) {
        self.held = 0;
        self.pressed = 0;
        self.keys_down.clear();
        self.mouse_delta = (0.0, 0.0);
    }
    /// Call at the end of each tick.
    pub fn end_tick(&mut self) {
        self.pressed = 0;
        self.mouse_delta = (0.0, 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FOCUS-INPUT: a focus change forgets the held keys, actions and pending presses.
    #[test]
    fn focus_change_releases_everything() {
        let mut st = InputState::default();
        st.press(Action::Throttle);
        st.keys_down.insert(0x2A);
        st.keys_down.insert(0x1D);
        st.mouse_delta = (3.0, 4.0);
        assert!(st.held(Action::Throttle) && !st.keys_down.is_empty());
        st.release_all();
        assert!(!st.held(Action::Throttle));
        assert!(!st.take_pressed(Action::Throttle));
        assert!(st.keys_down.is_empty());
        assert_eq!(st.mouse_delta, (0.0, 0.0));
    }

    fn keyed(lines: &'static [(&'static str, &'static str)]) -> InputState {
        InputState { bindings: bindings::Bindings::from_registry(|k| lines.iter().find(|(n, _)| *n == k).map(|(_, v)| *v)), ..Default::default() }
    }

    /// an action with no binding reads its default key: pressed once on the
    /// down edge, held until the up, the edge is cleared by `end_tick`.
    #[test]
    fn a_default_key_presses_once_and_holds() {
        let mut st = InputState::default();
        st.key_changed(KeyCode::KeyW, ElementState::Pressed);
        assert!(st.held(Action::SignalLeft));
        assert!(st.take_pressed(Action::SignalLeft));
        assert!(!st.take_pressed(Action::SignalLeft), "one edge per press");
        st.key_changed(KeyCode::KeyW, ElementState::Pressed); // auto-repeat: no second edge
        assert!(!st.take_pressed(Action::SignalLeft));
        st.key_changed(KeyCode::KeyW, ElementState::Released);
        assert!(!st.held(Action::SignalLeft));
        st.key_changed(KeyCode::KeyW, ElementState::Pressed);
        st.end_tick();
        assert!(!st.take_pressed(Action::SignalLeft), "end_tick drops the edge");
        assert!(st.held(Action::SignalLeft));
        st.key_changed(KeyCode::Escape, ElementState::Pressed);
        assert_eq!(st.keys_down_count(), 2, "Escape has a scancode but no action");
    }

    /// A bound action is pressed by its binding's key and no other: `C1_LEFT = key,SC_H` moves
    /// the steering to H and leaves the arrow (its default) and `Horn`'s own H dead.
    #[test]
    fn a_bound_action_follows_its_binding() {
        let mut st = keyed(&[("C1_LEFT", "key,SC_H"), ("C1_HORN", "key,SC_X")]);
        st.key_changed(KeyCode::ArrowLeft, ElementState::Pressed);
        assert!(!st.held(Action::SteerLeft), "the default key is dead once the action is bound");
        st.key_changed(KeyCode::KeyH, ElementState::Pressed);
        assert!(st.held(Action::SteerLeft) && st.take_pressed(Action::SteerLeft));
        assert!(!st.held(Action::Horn), "Horn is bound to X, not H");
        assert_eq!(st.read(Action::SteerLeft), 255);
        st.key_changed(KeyCode::KeyH, ElementState::Released);
        assert!(!st.held(Action::SteerLeft));
        assert_eq!(st.read(Action::SteerLeft), 0);
    }

    /// 0x411C5F: Shift, Ctrl or Alt down zeroes a bound key's read and gates its held state; what
    /// is held through `press` is gated only for a key binding.
    #[test]
    fn a_modifier_gates_a_bound_key() {
        let mut st = keyed(&[("C1_GAS", "key,SC_UP")]);
        st.key_changed(KeyCode::ArrowUp, ElementState::Pressed);
        assert!(st.held(Action::Throttle));
        for m in [KeyCode::ShiftLeft, KeyCode::ShiftRight, KeyCode::ControlLeft, KeyCode::AltRight] {
            st.key_changed(m, ElementState::Pressed);
            assert!(!st.held(Action::Throttle), "{m:?}");
            assert_eq!(st.read(Action::Throttle), 0);
            st.key_changed(m, ElementState::Released);
            assert!(st.held(Action::Throttle), "{m:?} released");
        }
        // a modifier pressed first: the key goes down under it and is not pressed at all
        let mut st = keyed(&[("C1_GAS", "key,SC_UP")]);
        st.key_changed(KeyCode::ShiftLeft, ElementState::Pressed);
        st.key_changed(KeyCode::ArrowUp, ElementState::Pressed);
        assert!(!st.take_pressed(Action::Throttle) && !st.held(Action::Throttle));
        // an unbound action's default key is not gated
        let mut st = InputState::default();
        st.key_changed(KeyCode::ShiftLeft, ElementState::Pressed);
        st.key_changed(KeyCode::ArrowUp, ElementState::Pressed);
        assert!(st.held(Action::Throttle));
    }

    /// 0x412FE4: an action bound to a pad control is pressed on the tick its read goes above 0x40,
    /// held while it stays there; the digital/analogue split of the ramps follows the kind.
    #[test]
    fn a_pad_control_presses_on_the_edge() {
        let mut st = InputState::default();
        st.bindings.by_id.insert(bindings::id::C1_HANDBRAKE, bindings::Binding { kind: 2, device: 0, index: 4, dir: 0, pov: 0, key: 0 });
        st.bindings.by_id.insert(bindings::id::C1_GAS, bindings::Binding { kind: 1, device: 0, index: 1, dir: 1, pov: 0, key: 0 });
        assert!(st.pad_bound(Action::Handbrake) && st.pad_bound(Action::Throttle) && !st.pad_bound(Action::Brake));
        assert_eq!(st.ramps(), (true, false, true), "an axis is not ramped, the rest are");
        let mut rec = bindings::PadRecord::default();
        st.set_pads(&[rec]);
        assert!(!st.held(Action::Handbrake) && !st.take_pressed(Action::Handbrake));
        rec.buttons[4] = true;
        st.set_pads(&[rec]);
        assert!(st.held(Action::Handbrake) && st.take_pressed(Action::Handbrake));
        st.set_pads(&[rec]); // still down: no new edge
        assert!(st.held(Action::Handbrake) && !st.take_pressed(Action::Handbrake));
        rec.buttons[4] = false;
        st.set_pads(&[rec]);
        assert!(!st.held(Action::Handbrake));
        // `press` / `release` work for any action, key or not
        st.press(Action::Horn);
        assert!(st.held(Action::Horn) && st.take_pressed(Action::Horn));
        st.release(Action::Horn);
        assert!(!st.held(Action::Horn));
    }

    /// The table's row `i` is the action whose discriminant is `i`, and every row is the action's own.
    #[test]
    fn the_action_table_is_indexed_by_the_variant() {
        for (i, d) in ACTIONS.iter().enumerate() {
            assert_eq!(d.action as usize, i, "{:?}", d.action);
            assert_eq!(binding_id(d.action), Some(d.id));
            assert_eq!(action_of_id(d.id), Some(d.action));
            assert_eq!(default_binding(d.default_key), Some(d.action));
            assert!(d.fe_name.starts_with("C1_") || d.fe_name.starts_with("I_"), "{}", d.fe_name);
        }
        let names: std::collections::HashSet<_> = ACTIONS.iter().map(|d| d.fe_name).collect();
        assert_eq!(names.len(), 33, "names are unique");
        // the declaration order is the table's, so the last variant is the last row
        assert_eq!(Action::CycleMap as usize, ACTIONS.len() - 1);
    }

    /// the per-action tables as one text, pinned by hash (ids, default keys,
    /// display codes, `ALL_ACTIONS` order, scancodes) — the guard for folding them into one table.
    #[test]
    fn the_action_tables_are_pinned() {
        let mut s = String::new();
        let mut actions = ALL_ACTIONS;
        actions.sort_by_key(|a| format!("{a:?}"));
        for a in actions.iter() {
            let key = KEY_PROBE.iter().find(|k| default_binding(**k) == Some(*a));
            s += &format!("{a:?}:{:?}:{:?}:{:?}\n", binding_id(*a), key_code_for(*a), key.map(|k| scancode(*k)));
        }
        let ids: std::collections::HashSet<_> = ALL_ACTIONS.iter().map(|a| binding_id(*a)).collect();
        assert_eq!(ids.len(), 33, "ids are unique");
        let h = s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
        eprintln!("TABLES {h:016x}\n{s}");
        assert_eq!(h, PINNED_TABLES);
    }
    const PINNED_TABLES: u64 = 0x9e95fc809b4b2db8;
    const KEY_PROBE: [KeyCode; 41] = [
        KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::Space,
        KeyCode::KeyA, KeyCode::KeyZ, KeyCode::KeyC, KeyCode::KeyR, KeyCode::KeyB, KeyCode::KeyH,
        KeyCode::KeyP, KeyCode::KeyL, KeyCode::KeyW, KeyCode::KeyE, KeyCode::KeyT, KeyCode::KeyY,
        KeyCode::KeyI, KeyCode::KeyU, KeyCode::KeyQ, KeyCode::KeyD, KeyCode::F1, KeyCode::F2,
        KeyCode::F3, KeyCode::F4, KeyCode::F5, KeyCode::F6, KeyCode::F7, KeyCode::F8, KeyCode::F9,
        KeyCode::F10, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Escape, KeyCode::KeyX, KeyCode::F11,
        KeyCode::F12, KeyCode::NumpadAdd, KeyCode::Tab, KeyCode::Digit3, KeyCode::KeyS,
    ];

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
