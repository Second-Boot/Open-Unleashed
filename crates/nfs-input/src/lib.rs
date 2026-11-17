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
pub mod gamepad;
#[cfg(target_os = "macos")]
pub mod runloop_pump;

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
///   hazards), 13 the lights, 16 the music (stripped into bit 0x12 by 0x4136D9) — edges;
/// * **the interface bits** (0x413744…0x4137A0), bit `id − 0x1EE` of the word the packer files in
///   the ring `0x5E954C`, read by `RaceFrame` through 0x4138D0 — on the press (edge) for 8 pause,
///   0x0A…0x0C the pause orbit's step, 0x0D the screenshot, 0x0E the HUD hide, 0x0F the view
///   distance, 0x10 the horizon, 0x11 / 0x12 the sound and music, 0x17 the camera, 0x18…0x1F the
///   HUD toggles and the map's zoom (0x415866…0x415ABE); held (level) for 0x13…0x16, the looks
///   (0x415AFA…0x415B2C).
///
/// A race reads 0x411760's bindings for the 25 ids it stores at every race start and the player's
/// for the rest ([`InputState::binding_of`], `bindings::RACE_FIXED`).
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
    /// `I_CYCLEMAP`, bit 0x1C — 0x426740 (Tab, 0x411760)
    CycleMap,
    /// `I_VIEWDISTANCE`, bit 0x0F — 0x44F220, `ADJUSTABLEVIEWDISTANCE` `[0x657D70]` stepped 0 → 1 → 2 → 3 → 0
    /// (F11, 0x411760)
    ViewDistance,
    /// `I_HORIZON`, bit 0x10 — 0x45DBB0 / 0x45DB90, the sky's `[+0x60]` flipped (F12, 0x411760)
    Horizon,
    /// `I_ZOOMINMAP`, bit 0x1D — 0x427600 → 0x41E620, MAPSCALE up (keypad −, 0x411760)
    ZoomInMap,
    /// `I_ZOOMOUTMAP`, bit 0x1E — 0x427630 → 0x41E5E0, MAPSCALE down (keypad +, 0x411760)
    ZoomOutMap,
    /// `I_SCREENSHOT`, bit 0x0D — 0x4158A2…0x415969, `Unleashed<nn>.tga` (Alt+P, 0x411760)
    Screenshot,
    /// `C1_SWITCHMUSIC`, command 16 — 0x4136D9 strips it from the packet and sets the interface bit
    /// 0x12, `I_MUSIC`'s (F9, 0x411760)
    SwitchMusic,
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

const ACTIONS: [ActionDef; 39] = {
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
        // the row's code stays 0x51D1D0's `p` (the setup screen shows it); the key a race reads is
        // 0x411760's Escape (`bindings::RACE_FIXED`)
        def(Action::Pause, "I_PAUSE", I_PAUSE, KeyCode::Escape, Some(b'p' as u32)),
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
        def(Action::CycleMap, "I_CYCLEMAP", I_CYCLEMAP, KeyCode::Tab, None),
        def(Action::ViewDistance, "I_VIEWDISTANCE", I_VIEWDISTANCE, KeyCode::F11, None),
        def(Action::Horizon, "I_HORIZON", I_HORIZON, KeyCode::F12, None),
        def(Action::ZoomInMap, "I_ZOOMINMAP", I_ZOOMINMAP, KeyCode::NumpadSubtract, None),
        def(Action::ZoomOutMap, "I_ZOOMOUTMAP", I_ZOOMOUTMAP, KeyCode::NumpadAdd, None),
        def(Action::Screenshot, "I_SCREENSHOT", I_SCREENSHOT, KeyCode::KeyP, None),
        def(Action::SwitchMusic, "C1_SWITCHMUSIC", C1_SWITCHMUSIC, KeyCode::F9, None),
    ]
};

const _: () = assert!(ACTIONS.len() <= 64, "InputState keeps held / pressed in one u64 each");

/// Every action, in the order of [`ACTIONS`].
const ALL_ACTIONS: [Action; 39] = {
    let mut out = [Action::SteerLeft; 39];
    let mut i = 0;
    while i < out.len() {
        out[i] = ACTIONS[i].action;
        i += 1;
    }
    out
};

/// The original's default key of each action, used only for an action with no binding at all
/// ([`bindings`]; with the shipped `fe.txt`, 0x51D1D0's rows and 0x411760's table every action has
/// one). The keys are 0x51D1D0's keyboard half (English, `lang` 0 — `bindings::default_key`) for
/// its nineteen rows, the shipped `fe.txt` for `C1_ABORT = key,SC_Q`, `C1_AIDRIVE = key,SC_D`,
/// `I_MUSIC = key,SC_F9` and the look / camera ids, and **0x411760** (`bindings::RACE_FIXED`, stored
/// over both at every race start) for the pause (Escape), F1 … F8, F10, `I_CYCLEHUD` `1`,
/// `I_CYCLEMAP` Tab, `I_VIEWDISTANCE` F11, `I_HORIZON` F12, the map zoom keypad − / +,
/// `I_SCREENSHOT` Alt+P and `C1_SWITCHMUSIC` F9. (`fe.txt`'s own `SC_F11`, `SC_F12`,
/// `SC_SUBTRACT`, `SC_ADD` are names the `SC_*` table 0x5D6588 does not have and bind nothing; it
/// is 0x411760 that binds those keys, by scan code.)
/// *(Corrected 2026-09-26: this map also had the port's own free-camera keys W / S / Q / E /
/// PageUp / PageDown / Shift, which nothing read. Corrected 2026-11-17: the 2026-09-26 line
/// "Escape is no in-race key — no binding names `SC_ESC`, nothing reads `[0x5DE029]`" was
/// wrong; 0x411760 names it and 0x411C20 reads it by index. Corrected 2026-11-17, ISS4-KEYS: P
/// paused and `2` cycled the map here, and F11 / F12 / keypad − / + / Alt+P bound nothing.)*
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
/// table `0x5DE028` index (`bindings::SC_NAMES`). Only the keys that table names, and F11 / F12 (0x57 / 0x58),
/// which 0x411760 binds by scan code (`I_VIEWDISTANCE`, `I_HORIZON`).
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
        End => 0x4F, ArrowDown => 0x50, PageDown => 0x51, Insert => 0x52, Delete => 0x53, F11 => 0x57, F12 => 0x58,
        _ => return None,
    })
}

/// The binding id each action reads through (`bindings::id`).
pub fn binding_id(a: Action) -> Option<u16> {
    Some(ACTIONS[a as usize].id)
}

/// A front-end key from a standard pad (PORT-DEVIATION D10: the original's front end reads no
/// pad), queued by `Gamepads::poll` whatever the bindings say — the d-pad, the left stick, A and B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuNav {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    /// the standard pad's Y (the port's third face button, `face[2]`): the in-race dialogs' second answer
    /// (PAD-END2, D10) — the failed mission summary's "race again"
    Alt,
    /// the standard pad's X (`face[3]`): the replay bar's "save" (PAD-END2, D10)
    Aux,
    /// Start: the front end's bottom bar "race" button (PORT-DEVIATION D54); a race ignores it
    Start,
    /// View / Back / Select (SDL's `BACK`): the front end's "exit" question, the Escape path (D54)
    View,
    /// the left shoulder: the previous tab of a screen with tab buttons (D54)
    Prev,
    /// the right shoulder: the next tab (D54)
    Next,
}

/// The hold-repeat of a pad's direction and shoulder keys in the front end (PAD-SLIDERS, PORT-DEVIATION D54): the key held
/// since `press` repeats after [`MenuHold::DELAY`], then every [`MenuHold::EVERY`]. Only the keys that can *adjust* a
/// value repeat (the four directions and the shoulders); one key is held at a time (the last pressed).
#[derive(Debug, Default, Clone)]
pub struct MenuHold {
    held: Option<(MenuNav, std::time::Instant)>,
}

impl MenuHold {
    /// the wait before the first repeat
    pub const DELAY: std::time::Duration = std::time::Duration::from_millis(400);
    /// the interval between repeats
    pub const EVERY: std::time::Duration = std::time::Duration::from_millis(80);

    /// `key` went down at `now`.
    pub fn press(&mut self, key: MenuNav, now: std::time::Instant) {
        self.held = matches!(key, MenuNav::Up | MenuNav::Down | MenuNav::Left | MenuNav::Right | MenuNav::Prev | MenuNav::Next).then_some((key, now + Self::DELAY));
    }

    /// `key` came up.
    pub fn release(&mut self, key: MenuNav) {
        if self.held.is_some_and(|(k, _)| k == key) {
            self.held = None;
        }
    }

    /// The repeats due at `now`: the held key once per interval since the last call, none before the delay.
    pub fn due(&mut self, now: std::time::Instant) -> Vec<MenuNav> {
        let mut out = Vec::new();
        if let Some((k, next)) = self.held.as_mut() {
            while *next <= now && out.len() < 8 {
                out.push(*k);
                *next += Self::EVERY;
            }
            if *next <= now {
                *next = now + Self::EVERY;
            }
        }
        out
    }
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
    /// the front end's pad keys since the last tick ([`MenuNav`]); a race drops them
    pub pad_menu: Vec<MenuNav>,
    /// the held-key repeats of the same keys ([`MenuHold`]), for the widgets a direction key *adjusts* (PAD-SLIDERS,
    /// D54): kept apart from `pad_menu` so a held d-pad still moves between targets one step per push
    pub pad_menu_repeat: Vec<MenuNav>,
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
    pub fn key_changed(&mut self, code: KeyCode, state: ElementState) {
        if let Some(sc) = scancode(code) {
            match state {
                ElementState::Pressed => self.keys_down.insert(sc),
                ElementState::Released => self.keys_down.remove(&sc),
            };
            // an action bound to a key is pressed by its binding's key and no other — the race's
            // binding ([`Self::binding_of`]: 0x411760's table over the rows and `fe.txt`)
            for &a in ALL_ACTIONS.iter() {
                let Some(b) = self.binding_of(a) else { continue };
                if b.kind_class() != bindings::BindingKind::Key {
                    continue;
                }
                if b.index & 0xFF == u32::from(sc) {
                    match state {
                        ElementState::Pressed if self.read(a) > 0x40 => self.hold(a),
                        ElementState::Pressed => {}
                        ElementState::Released => self.unhold(a),
                    }
                } else if b.index & 0x70000 != 0 && bindings::is_modifier(sc) {
                    // a binding that asks for a modifier (0x411760's Alt+P) reads its key again when a
                    // modifier goes down or up, as the packer's level read each tick does (0x411C20)
                    if self.read(a) > 0x40 {
                        self.hold(a);
                    } else {
                        self.unhold(a);
                    }
                }
            }
        }
        let Some(action) = default_binding(code) else { return };
        if let Some(b) = self.binding_of(action) {
            // PORT-DEVIATION D44 (PAD-HANG): a row bound to a pad control still answers its
            // default key (0x51D1D0's keyboard half, `bindings::default_key`) unless a row is
            // bound to that key — the keyboard keeps working beside the pad. In the original a
            // row has one binding (`[0x5E9130 + id·4]`) and a pad-bound row reads no key.
            if b.kind_class() == bindings::BindingKind::Key || self.key_bound(code) {
                return;
            }
        }
        match state {
            ElementState::Pressed => self.hold(action),
            ElementState::Released => self.unhold(action),
        }
    }
    /// The binding a race reads for action `a`: **0x411760**'s for the ids it stores
    /// (`bindings::race_fixed`, written over the option stream at every race start, 0x4155EF), the
    /// player's (`fe.txt`, then the `Controller.*` rows) for every other. The rows keep their own
    /// bindings ([`bindings`](Self::bindings)) for the setup screens and the profile: in the
    /// original too the `Controller.Pause` row still shows (and saves) P, which no race reads.
    pub fn binding_of(&self, a: Action) -> Option<bindings::Binding> {
        let id = ACTIONS[a as usize].id;
        bindings::race_fixed(id).or_else(|| self.bindings.get(id).copied())
    }
    /// PORT-DEVIATION D44 (the pad's pause): the `Controller.Pause` row's own binding when it is a pad
    /// control. The original has **no** pad pause: 0x411760 overwrites `I_PAUSE` with Escape after
    /// the rows are applied, clears `I_PAUSEJOY` (`[0x5E990C]` = 0) whose bit 9 nothing reads (every
    /// `0x4138D0` call site pushes 8 or 0x0A…0x1F), and the `PSX_START` word `[0x5E9548]` (0x4137ED)
    /// has no reader. The port keeps the row's pad control pausing beside Escape, the only way to
    /// pause with a pad.
    fn pause_pad_row(&self) -> Option<&bindings::Binding> {
        self.bindings.get(bindings::id::I_PAUSE).filter(|b| matches!(b.kind_class(), bindings::BindingKind::Axis | bindings::BindingKind::Button | bindings::BindingKind::Hat))
    }
    /// Whether a binding the race reads is a key of physical key `code` (its scancode).
    fn key_bound(&self, code: KeyCode) -> bool {
        scancode(code).is_some_and(|sc| ALL_ACTIONS.iter().any(|&a| self.binding_of(a).is_some_and(|b| b.kind_class() == bindings::BindingKind::Key && b.index & 0xFF == u32::from(sc))))
    }
    pub fn press(&mut self, action: Action) {
        self.hold(action);
    }
    /// One press edge of `action` without holding it — an interface bit set for one sample, as
    /// 0x4136D9 sets `I_MUSIC`'s bit 0x12 from command 16.
    pub fn pulse(&mut self, action: Action) {
        self.pressed |= bit(action);
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
        self.held_by_press(a)
    }
    /// [`held`](Self::held) without the pad read: a key or a `press` holding the action, gated
    /// by a modifier for a key binding (0x411C5F).
    fn held_by_press(&self, a: Action) -> bool {
        let gated = self.binding_of(a).is_some_and(|b| b.kind_class() == bindings::BindingKind::Key && bindings::modifier_mismatch(&b, self.modifiers()));
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
    ///
    /// PORT-DEVIATION D44 (PAD-MODERN): an SDL **standard pad** gets the modern layout
    /// ([`bindings::modern_pad_bindings`]) instead of 0x51D1D0's pad half; a wheel or any other raw
    /// device the original's.
    pub fn default_controls(&mut self, pad: Option<usize>, id_of: impl Fn(&str) -> Option<u16>) {
        let rows = match pad.and_then(|p| self.pads.get(p).map(|r| (p as u32, r))) {
            Some((device, r)) if r.standard => bindings::modern_pad_bindings(device, 0),
            Some((device, r)) => bindings::default_bindings(device, r.caps(), 0),
            None => bindings::default_bindings(0, bindings::DeviceCaps::default(), 0),
        };
        self.bindings.set_defaults(&rows, id_of);
    }
    /// Whether the nineteen `Controller.*` rows are the keyboard's factory rows (0x51D1D0 on device
    /// 0) — the player never changed them (PORT-DEVIATION D44 item 9).
    pub fn rows_are_keyboard_defaults(&self, id_of: impl Fn(&str) -> Option<u16>) -> bool {
        let rows = bindings::default_bindings(0, bindings::DeviceCaps::default(), 0);
        bindings::CONTROLLER_ROWS.iter().zip(rows.iter()).all(|(r, b)| id_of(r).is_none_or(|id| self.bindings.get(id) == Some(b)))
    }
    /// How many keys are down — 0x51C610 takes a key for a binding row only when exactly one is.
    pub fn keys_down_count(&self) -> usize {
        self.keys_down.len()
    }
    /// The modifiers down, as 0x411C20 reads them.
    fn modifiers(&self) -> bindings::Modifiers {
        let down = |s: u8| self.keys_down.contains(&s);
        bindings::Modifiers { shift: down(bindings::SC_LSHIFT) || down(bindings::SC_RSHIFT), ctrl: down(bindings::SC_CTRL), alt: down(bindings::SC_ALT) }
    }
    /// An action's 0…255 read through its binding by the kind's reader (the records at
    /// `0x5CC5C8 + 12·kind`): `Analog` 0x411B80, `Button` 0x411A50, `Key` 0x411C20, `Pov`
    /// 0x411AC0. 0 for an action with no binding, or a binding of kind 0.
    pub fn read(&self, a: Action) -> u8 {
        let v = self.binding_of(a).map_or(0, |b| self.read_binding(&b));
        // PORT-DEVIATION D44: the pause row's pad control beside 0x411760's Escape
        if a == Action::Pause
            && let Some(b) = self.pause_pad_row()
        {
            return v.max(self.read_binding(b));
        }
        v
    }
    /// One binding read by its kind's reader; 0 for kind 0 and for the interface reader 0x411C80
    /// (kind 4), which only 0x411760's `I_UP` … `I_SELECT` use and no `RaceFrame` read reaches.
    fn read_binding(&self, b: &bindings::Binding) -> u8 {
        match b.kind_class() {
            bindings::BindingKind::Axis => bindings::axis_read(b, &self.pads, &self.axis_bands),
            bindings::BindingKind::Button => bindings::button_read(b, &self.pads),
            bindings::BindingKind::Key => bindings::key_read(b, |s: u8| self.keys_down.contains(&s), self.modifiers()),
            bindings::BindingKind::Hat => bindings::pov_read(b, &self.pads),
            _ => 0,
        }
    }
    /// Whether an action is bound to a pad control (kinds 1, 2, 5), which the packer then reads
    /// through its reader rather than through a held key.
    pub fn pad_bound(&self, a: Action) -> bool {
        if a == Action::Pause {
            return self.pause_pad_row().is_some();
        }
        self.binding_of(a).is_some_and(|b| matches!(b.kind_class(), bindings::BindingKind::Axis | bindings::BindingKind::Button | bindings::BindingKind::Hat))
    }
    /// `.RAMPS` / `.RAMPG` / `.RAMPB` — 0x51CC00 writes `kind != 1` for the `C1_RIGHT`, `C1_GAS`
    /// and `C1_BRAKE` bindings into option ids 0x222 / 0x223 / 0x224 (0x51CF54…0x51CFFC): every
    /// control but an axis is ramped. Without a binding the port's keyboard is digital.
    ///
    /// PORT-DEVIATION D44 (PAD-HANG): while a key holds a side of an axis-bound control (its default
    /// key beside the pad, [`key_changed`](Self::key_changed)) that control is ramped as a key's is.
    pub fn ramps(&self) -> (bool, bool, bool) {
        let digital = |a: Action| self.binding_of(a).is_none_or(|b| b.kind_class() != bindings::BindingKind::Axis);
        let key = |a: Action| self.held_by_press(a);
        (
            digital(Action::SteerRight) || key(Action::SteerLeft) || key(Action::SteerRight),
            digital(Action::Throttle) || key(Action::Throttle),
            digital(Action::Brake) || key(Action::Brake),
        )
    }
    /// Edge-triggered: true once per key press.
    pub fn take_pressed(&mut self, a: Action) -> bool {
        let was = self.pressed & bit(a) != 0;
        self.pressed &= !bit(a);
        was
    }
    /// −1..1 steering: through the bindings — a pad-bound side in proportion, a key full lock.
    /// *(PAD-MODERN, D44: a pad no binding reads no longer steers through the port's fixed map.)*
    pub fn steer(&self) -> f32 {
        // the packer's steering, `(C1_RIGHT − C1_LEFT) / 4` (0x412F39), when a pad control is bound
        // to either side; handed on doubled, in the −1…1 where 126 is full lock
        //
        // *(Corrected 2026-10-25, PAD-XBOX: each side was `read.max(held ? 255 : 0)`, and `held`
        // of a pad-bound side is `read > 0x40` — so a stick past a quarter of its half-travel
        // jumped to full lock. Only a key or a `press` holding the side counts as 255 now.)*
        if self.pad_bound(Action::SteerLeft) || self.pad_bound(Action::SteerRight) {
            let r = self.read(Action::SteerRight).max(if self.held_by_press(Action::SteerRight) { 255 } else { 0 }) as i32;
            let l = self.read(Action::SteerLeft).max(if self.held_by_press(Action::SteerLeft) { 255 } else { 0 }) as i32;
            return ((r - l) / 4 * 2) as f32 / 126.0;
        }
        (self.held(Action::SteerRight) as i32 - self.held(Action::SteerLeft) as i32) as f32
    }
    pub fn throttle(&self) -> f32 {
        // `C1_GAS / 2`, capped at 0x7E (0x412F66, 0x412F9A)
        // a pad-bound pedal and its default key beside it (D44, PAD-HANG): the larger of the two
        if self.pad_bound(Action::Throttle) {
            return (self.pedal(Action::Throttle) / 2).min(0x7E) as f32 / 126.0;
        }
        self.held(Action::Throttle) as i32 as f32
    }
    pub fn brake(&self) -> f32 {
        // `C1_BRAKE / 2`, capped at 0x7E (0x412F89, 0x412FB0)
        if self.pad_bound(Action::Brake) {
            return (self.pedal(Action::Brake) / 2).min(0x7E) as f32 / 126.0;
        }
        self.held(Action::Brake) as i32 as f32
    }
    /// A pad-bound pedal's read, or 255 while a key or a `press` holds it (as [`steer`](Self::steer)
    /// takes each side).
    fn pedal(&self, a: Action) -> u8 {
        self.read(a).max(if self.held_by_press(a) { 255 } else { 0 })
    }
    /// The window lost or regained the keyboard focus (FOCUS-INPUT): forget every held key, action and
    /// pending press. PORT-DEVIATION D17 (PORT-DEVIATIONS.md): the release events of keys / modifiers
    /// held at that moment never arrive (macOS: Cmd+Shift+5 leaves Cmd / Shift "down").
    pub fn release_all(&mut self) {
        self.held = 0;
        self.pressed = 0;
        self.keys_down.clear();
        self.mouse_delta = (0.0, 0.0);
        self.pad_menu.clear();
        self.pad_menu_repeat.clear();
    }
    /// Call at the end of each tick.
    pub fn end_tick(&mut self) {
        self.pressed = 0;
        self.mouse_delta = (0.0, 0.0);
        self.pad_menu.clear();
        self.pad_menu_repeat.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PAD-SLIDERS: a held direction repeats after the delay, once per interval; a release stops it; a face button never repeats.
    #[test]
    fn a_held_direction_repeats_after_the_delay() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut h = MenuHold::default();
        h.press(MenuNav::Right, t0);
        assert!(h.due(t0 + Duration::from_millis(399)).is_empty());
        assert_eq!(h.due(t0 + Duration::from_millis(400)), vec![MenuNav::Right]);
        assert!(h.due(t0 + Duration::from_millis(450)).is_empty());
        assert_eq!(h.due(t0 + Duration::from_millis(560)), vec![MenuNav::Right, MenuNav::Right]);
        h.release(MenuNav::Left);
        assert_eq!(h.due(t0 + Duration::from_millis(640)), vec![MenuNav::Right]);
        h.release(MenuNav::Right);
        assert!(h.due(t0 + Duration::from_secs(5)).is_empty());
        h.press(MenuNav::Select, t0);
        assert!(h.due(t0 + Duration::from_secs(5)).is_empty());
    }

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
        st.key_changed(KeyCode::Digit0, ElementState::Pressed);
        assert_eq!(st.keys_down_count(), 2, "0 has a scancode but no action");
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

    /// PAD-XBOX: a stick bound to both steering rows (0x51D1D0's `{1, dev, 0, ∓1}`) steers in
    /// proportion through 0x411B80 and `(right − left) / 4` (0x412F39) — before, a side past a read
    /// of 0x40 counted as held and jumped to full lock. A key or a `press` still gives 255.
    #[test]
    fn a_bound_stick_steers_in_proportion() {
        let mut st = InputState::default();
        st.axis_bands = [[0, 0x80]; 8];
        st.bindings.by_id.insert(bindings::id::C1_LEFT, bindings::Binding { kind: 1, device: 0, index: 0, dir: -1, pov: 0, key: 0 });
        st.bindings.by_id.insert(bindings::id::C1_RIGHT, bindings::Binding { kind: 1, device: 0, index: 0, dir: 1, pov: 0, key: 0 });
        let mut rec = bindings::PadRecord { mask: 0x1B, ..Default::default() };
        for (x, want) in [(0x8000, 0), (0x8000 + 0x4000, 32), (0x8000 + 0x2000, 16), (0xFFFF, 63), (0x8000 - 0x4000, -31), (0, -63)] {
            rec.axes[0] = x;
            st.set_pads(&[rec]);
            let packet = (st.steer() * 126.0).round() as i32 / 2;
            assert_eq!(packet, want, "axis at {x:#x}");
        }
        // half-way right is half lock, not full
        rec.axes[0] = 0x8000 + 0x4000;
        st.set_pads(&[rec]);
        assert!(st.steer() < 0.6 && st.held(Action::SteerRight));
        // the autotest's press of a pad-bound side is full lock, as a key is
        rec.axes[0] = 0x8000;
        st.set_pads(&[rec]);
        st.press(Action::SteerLeft);
        assert_eq!((st.steer() * 126.0).round() as i32, -126);
    }

    /// PAD-XBOX: a player's rebound rows (Accel / Brake on buttons 9 / 8, both
    /// steering rows on axis 3, …) and `DEFAULTCONTROLS` (0x511F50 → 0x51D1D0): on the keyboard
    /// row every row is its key again; on an SDL standard pad's record the pad half binds the
    /// sticks and buttons 0, 2, 3, 7, 8 (not 5, `Z`, which such a pad has not), the rest keys.
    #[test]
    fn default_controls_reset_rebound_rows() {
        let rows = |n: &str| {
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
            .and_then(binding_id)
        };
        let rebound = |st: &mut InputState| {
            st.default_controls(None, rows);
            let b = |kind, index, dir| bindings::Binding { kind, device: 0, index, dir, pov: 0, key: 0 };
            for (a, nb) in [
                (Action::Throttle, b(2, 9, 0)),
                (Action::Brake, b(2, 8, 0)),
                (Action::Handbrake, b(2, 3, 0)),
                (Action::SteerLeft, b(1, 3, -1)),
                (Action::SteerRight, b(1, 3, 1)),
                (Action::Reset, b(2, 6, 0)),
                (Action::CameraNext, b(2, 2, 0)),
                (Action::RearView, b(2, 1, 0)),
            ] {
                st.bindings.assign(binding_id(a).unwrap(), nb);
            }
        };
        let pad = gamepad::pad_record(&gamepad::PadSnapshot::default());
        let mut st = InputState { pads: vec![pad], ..Default::default() };
        rebound(&mut st);
        // both directions of axis 3 survive 0x51DCC0: equality includes `dir` (0x51DC20)
        let get = |st: &InputState, a: Action| *st.bindings.get(binding_id(a).unwrap()).unwrap();
        assert_eq!((get(&st, Action::SteerLeft).dir, get(&st, Action::SteerRight).dir), (-1, 1));
        assert_eq!(get(&st, Action::SteerLeft).kind, 1);
        // the keyboard row: every row its key
        st.default_controls(None, rows);
        for a in [Action::Throttle, Action::Brake, Action::Handbrake, Action::SteerLeft, Action::SteerRight, Action::Reset, Action::CameraNext, Action::RearView] {
            let b = get(&st, a);
            assert_eq!((b.kind, b.key), (3, key_code_for(a).unwrap()), "{a:?}");
        }
        // the pad's row: a standard pad gets the modern layout (PAD-MODERN, D44)
        rebound(&mut st);
        st.default_controls(Some(0), rows);
        let k = |a| {
            let b = get(&st, a);
            (b.kind, b.index, b.dir, b.pov)
        };
        assert_eq!(k(Action::SteerLeft), (1, 0, -1, 0));
        assert_eq!(k(Action::SteerRight), (1, 0, 1, 0));
        assert_eq!(k(Action::Throttle), (1, 5, 1, 0)); // RT
        assert_eq!(k(Action::Brake), (1, 2, 1, 0)); // LT
        assert_eq!(k(Action::Handbrake), (2, 0, 0, 0)); // A
        assert_eq!(k(Action::RearView), (2, 1, 0, 0)); // B
        assert_eq!(k(Action::Reset), (2, 3, 0, 0)); // X
        assert_eq!(k(Action::CameraNext), (2, 2, 0, 0)); // Y
        assert_eq!(k(Action::ShiftUp), (2, 7, 0, 0)); // RB
        assert_eq!(k(Action::ShiftDown), (2, 6, 0, 0)); // LB
        assert_eq!(k(Action::Lights), (5, 0x20, 0, 0)); // d-pad up
        assert_eq!(k(Action::LookForward).0, 3, "LookForward keeps its key");
        // a raw device with the same axes and buttons (a wheel, a stick): the original's pad half
        let mut raw = pad;
        raw.standard = false;
        raw.button_mask = 0xFFF;
        st.pads = vec![raw];
        st.default_controls(Some(0), rows);
        let k = |a| {
            let b = get(&st, a);
            (b.kind, b.index, b.dir, b.pov)
        };
        assert_eq!(k(Action::Throttle), (1, 1, -1, 0));
        assert_eq!(k(Action::Brake), (1, 1, 1, 0));
        assert_eq!(k(Action::ShiftDown), (2, 8, 0, 0));
        assert_eq!(k(Action::CameraNext), (2, 5, 0, 0));
    }

    /// PAD-MODERN (D44): the port's fixed in-race pad map is gone — a pad no binding reads moves
    /// nothing (stick right, both triggers and every button down, all rows on keys), and the same
    /// pad drives the car once the rows are its (`DEFAULTCONTROLS`).
    #[test]
    fn a_pad_drives_only_through_its_bindings() {
        let rows = |n: &str| match n {
            "Controller.Accel" => binding_id(Action::Throttle),
            "Controller.Brake" => binding_id(Action::Brake),
            "Controller.SteerLeft" => binding_id(Action::SteerLeft),
            "Controller.SteerRight" => binding_id(Action::SteerRight),
            "Controller.HandBrake" => binding_id(Action::Handbrake),
            _ => None,
        };
        let mut p = gamepad::PadSnapshot { sticks: [32767, 0, 0, 0], triggers: [32767, 32767], ..Default::default() };
        p.buttons = [true; 15];
        let rec = gamepad::pad_record(&p);
        let mut st = InputState { axis_bands: [[0, 127]; 8], ..Default::default() };
        st.default_controls(None, rows);
        st.set_pads(&[rec]);
        assert_eq!((st.steer(), st.throttle(), st.brake()), (0.0, 0.0, 0.0));
        assert!(!st.held(Action::Handbrake) && !st.take_pressed(Action::Throttle));
        st.default_controls(Some(0), rows);
        st.set_pads(&[rec]);
        assert_eq!((st.steer(), st.throttle(), st.brake()), (1.0, 1.0, 1.0));
        assert!(st.held(Action::Handbrake));
    }

    /// The table's row `i` is the action whose discriminant is `i`, and every row is the action's own.
    #[test]
    fn the_action_table_is_indexed_by_the_variant() {
        for (i, d) in ACTIONS.iter().enumerate() {
            assert_eq!(d.action as usize, i, "{:?}", d.action);
            assert_eq!(binding_id(d.action), Some(d.id));
            // `C1_SWITCHMUSIC` shares F9 with `I_MUSIC` (0x411760 and `fe.txt`), which comes first
            if d.action != Action::SwitchMusic {
                assert_eq!(default_binding(d.default_key), Some(d.action));
            }
            assert!(d.fe_name.starts_with("C1_") || d.fe_name.starts_with("I_"), "{}", d.fe_name);
        }
        let names: std::collections::HashSet<_> = ACTIONS.iter().map(|d| d.fe_name).collect();
        assert_eq!(names.len(), ACTIONS.len(), "names are unique");
        // the declaration order is the table's, so the last variant is the last row
        assert_eq!(Action::SwitchMusic as usize, ACTIONS.len() - 1);
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
        assert_eq!(ids.len(), ALL_ACTIONS.len(), "ids are unique");
        let h = s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
        eprintln!("TABLES {h:016x}\n{s}");
        assert_eq!(h, PINNED_TABLES);
    }
    // ISS4-KEYS: re-pinned for 0x411760 (the pause on Escape, the map on Tab, the six new rows)
    const PINNED_TABLES: u64 = 0x0692c33f1f68d352;
    const KEY_PROBE: [KeyCode; 43] = [
        KeyCode::ArrowLeft, KeyCode::ArrowRight, KeyCode::ArrowUp, KeyCode::ArrowDown, KeyCode::Space,
        KeyCode::KeyA, KeyCode::KeyZ, KeyCode::KeyC, KeyCode::KeyR, KeyCode::KeyB, KeyCode::KeyH,
        KeyCode::KeyP, KeyCode::KeyL, KeyCode::KeyW, KeyCode::KeyE, KeyCode::KeyT, KeyCode::KeyY,
        KeyCode::KeyI, KeyCode::KeyU, KeyCode::KeyQ, KeyCode::KeyD, KeyCode::F1, KeyCode::F2,
        KeyCode::F3, KeyCode::F4, KeyCode::F5, KeyCode::F6, KeyCode::F7, KeyCode::F8, KeyCode::F9,
        KeyCode::F10, KeyCode::Digit1, KeyCode::Digit2, KeyCode::Escape, KeyCode::KeyX, KeyCode::F11,
        KeyCode::F12, KeyCode::NumpadAdd, KeyCode::Tab, KeyCode::Digit3, KeyCode::KeyS, KeyCode::NumpadSubtract,
        KeyCode::Digit0,
    ];

    /// The shipped `fe.txt`'s binding lines (lines 397…436), 0x51D1D0's keyboard rows and 0x411760's
    /// table give every action the key a race reads ([`InputState::binding_of`]), and it is the one
    /// [`default_binding`] names (`C1_SWITCHMUSIC`'s F9 is `I_MUSIC`'s too, so it names that one).
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
            KeyCode::Escape, KeyCode::KeyL, KeyCode::KeyW, KeyCode::KeyE, KeyCode::KeyT, KeyCode::KeyY,
            KeyCode::KeyI, KeyCode::KeyU, KeyCode::KeyQ, KeyCode::KeyD, KeyCode::F1, KeyCode::F2,
            KeyCode::F3, KeyCode::F4, KeyCode::F5, KeyCode::F6, KeyCode::F7, KeyCode::F8, KeyCode::F9,
            KeyCode::F10, KeyCode::Digit1, KeyCode::Tab, KeyCode::F11, KeyCode::F12, KeyCode::NumpadSubtract,
            KeyCode::NumpadAdd, KeyCode::KeyP,
        ];
        assert_eq!(codes.len() + 1, ALL_ACTIONS.len(), "every action but C1_SWITCHMUSIC (F9, as I_MUSIC)");
        for k in codes {
            let a = default_binding(k).unwrap();
            let b = s.binding_of(a).unwrap_or_else(|| panic!("{a:?} unbound"));
            // `I_SCREENSHOT` is P with 0x411760's Alt bit (0x40000)
            let want = if a == Action::Screenshot { 0x40000 | scancode(k).unwrap() as u32 } else { scancode(k).unwrap() as u32 };
            assert_eq!((b.kind, b.index), (3, want), "{a:?}");
        }
        assert_eq!(s.binding_of(Action::SwitchMusic).map(|b| b.index), Some(u32::from(scancode(KeyCode::F9).unwrap())));
        // the rows keep 0x51D1D0's P for the pause, which the setup screen shows and no race reads
        assert_eq!(s.bindings.get(bindings::id::I_PAUSE).map(|b| b.index), Some(u32::from(scancode(KeyCode::KeyP).unwrap())));
    }

    /// ISS4-KEYS: Escape presses the pause and P does not — the original's `I_PAUSE` is 0x411760's
    /// `SC_ESC` (`[0x5E9908]` = 0x103), stored over the `Controller.Pause` row (P) at every race start;
    /// a row bound to Escape fires beside it, as both read the same key; the pause row on a pad still
    /// pauses (PORT-DEVIATION D44), and Escape with it.
    #[test]
    fn escape_pauses_and_p_does_not() {
        let mut st = InputState::default();
        st.key_changed(KeyCode::Escape, ElementState::Pressed);
        assert!(st.take_pressed(Action::Pause));
        st.key_changed(KeyCode::Escape, ElementState::Released);
        assert!(!st.held(Action::Pause));
        st.key_changed(KeyCode::KeyP, ElementState::Pressed);
        assert!(!st.take_pressed(Action::Pause), "P does not pause");
        st.key_changed(KeyCode::KeyP, ElementState::Released);

        let mut pad = modern_pad_state(&gamepad::PadSnapshot::default());
        pad.key_changed(KeyCode::Escape, ElementState::Pressed);
        assert!(pad.take_pressed(Action::Pause), "pause row on Start: Escape still pauses");

        let mut bound = InputState::default();
        let horn = binding_id(Action::Horn).unwrap();
        bound.bindings.by_id.insert(horn, bindings::Binding { kind: 3, index: u32::from(scancode(KeyCode::Escape).unwrap()), ..Default::default() });
        bound.key_changed(KeyCode::Escape, ElementState::Pressed);
        assert!(bound.take_pressed(Action::Pause), "0x411760's Escape whatever the rows");
        assert!(bound.take_pressed(Action::Horn), "and the row bound to Escape beside it");

        // the pause row remapped to K: K does not pause either (0x411760 overwrites the id)
        let mut k = InputState::default();
        k.bindings.by_id.insert(bindings::id::I_PAUSE, bindings::Binding { kind: 3, index: u32::from(scancode(KeyCode::KeyK).unwrap()), key: u32::from(b'k'), ..Default::default() });
        k.key_changed(KeyCode::KeyK, ElementState::Pressed);
        assert!(!k.take_pressed(Action::Pause));
    }

    /// ISS4-KEYS: 0x411760's other keys — Tab cycles the map (`fe.txt`'s `2` is overwritten), F11 the
    /// view distance, F12 the horizon, keypad − / + the map zoom, Alt+P (not P, not Shift+Alt+P) the
    /// screenshot, F9 both `I_MUSIC` (`fe.txt`) and `C1_SWITCHMUSIC`; a row 0x411760 does not store
    /// (`LookBehind`) follows the player's remapping.
    #[test]
    fn the_race_keys_are_0x411760s() {
        const FE: [(&str, &str); 3] = [("I_CYCLEMAP", "key,SC_2"), ("I_MUSIC", "key,SC_F9"), ("I_LOOKBEHIND", "key,SC_K")];
        let mut st = keyed(&FE);
        let tap = |st: &mut InputState, k: KeyCode| {
            st.key_changed(k, ElementState::Pressed);
            st.key_changed(k, ElementState::Released);
        };
        tap(&mut st, KeyCode::Digit2);
        assert!(!st.take_pressed(Action::CycleMap), "2 is no longer the map's");
        tap(&mut st, KeyCode::Tab);
        assert!(st.take_pressed(Action::CycleMap));
        for (k, a) in [(KeyCode::F11, Action::ViewDistance), (KeyCode::F12, Action::Horizon), (KeyCode::NumpadSubtract, Action::ZoomInMap), (KeyCode::NumpadAdd, Action::ZoomOutMap)] {
            tap(&mut st, k);
            assert!(st.take_pressed(a), "{a:?}");
        }
        tap(&mut st, KeyCode::KeyP);
        assert!(!st.take_pressed(Action::Screenshot), "P alone");
        st.key_changed(KeyCode::AltLeft, ElementState::Pressed);
        tap(&mut st, KeyCode::KeyP);
        assert!(st.take_pressed(Action::Screenshot), "Alt+P");
        st.key_changed(KeyCode::ShiftLeft, ElementState::Pressed);
        tap(&mut st, KeyCode::KeyP);
        assert!(!st.take_pressed(Action::Screenshot), "Shift+Alt+P: a bit left over (0x411C5F)");
        st.key_changed(KeyCode::ShiftLeft, ElementState::Released);
        st.key_changed(KeyCode::AltLeft, ElementState::Released);
        // P held, then Alt: the level read sees Alt+P (0x411C20 each sample)
        st.key_changed(KeyCode::KeyP, ElementState::Pressed);
        st.key_changed(KeyCode::AltLeft, ElementState::Pressed);
        assert!(st.take_pressed(Action::Screenshot));
        st.key_changed(KeyCode::AltLeft, ElementState::Released);
        st.key_changed(KeyCode::KeyP, ElementState::Released);
        tap(&mut st, KeyCode::F9);
        assert!(st.take_pressed(Action::Music) && st.take_pressed(Action::SwitchMusic));
        tap(&mut st, KeyCode::KeyB);
        assert!(!st.take_pressed(Action::RearView), "LookBehind is on K now");
        tap(&mut st, KeyCode::KeyK);
        assert!(st.take_pressed(Action::RearView));
        tap(&mut st, KeyCode::Digit0);
        assert_eq!(st.pressed, 0, "0 is no race key");
    }

    /// 0x411760 read back out of `nfs5.exe` (skipped without the game): its 25 stores, `mov dword
    /// [0x5E9130 + id·4], imm32` (C7 05) or `mov [abs], eax` with `eax` = 0 (A3, after `xor eax, eax`),
    /// then `ret`, are [`bindings::RACE_FIXED`] in order.
    #[test]
    fn the_race_bindings_match_the_image() {
        let Some(home) = std::env::var_os("HOME") else { return };
        let Ok(img) = std::fs::read(std::path::PathBuf::from(home).join("Games/NFS-Porsche-Unleashed/game_info/data/nfs5.exe")) else { return };
        let u16at = |o: usize| u16::from_le_bytes([img[o], img[o + 1]]) as usize;
        let u32at = |o: usize| u32::from_le_bytes([img[o], img[o + 1], img[o + 2], img[o + 3]]);
        let pe = u32at(0x3C) as usize;
        let table = pe + 24 + u16at(pe + 20);
        let at = |va: u32| {
            let rva = va - 0x40_0000;
            (0..u16at(pe + 6))
                .map(|i| table + i * 40)
                .find(|&s| rva >= u32at(s + 12) && rva < u32at(s + 12) + u32at(s + 8))
                .map(|s| (u32at(s + 20) + rva - u32at(s + 12)) as usize)
                .expect("0x411760 is in a section")
        };
        let mut o = at(0x41_1760);
        assert_eq!(&img[o..o + 2], &[0x33, 0xC0], "xor eax, eax");
        o += 2;
        let mut got = Vec::new();
        loop {
            match img[o] {
                0xC7 if img[o + 1] == 0x05 => {
                    got.push((u32at(o + 2), u32at(o + 6)));
                    o += 10;
                }
                0xA3 => {
                    got.push((u32at(o + 1), 0));
                    o += 5;
                }
                0xC3 => break,
                b => panic!("unexpected opcode {b:#x} at file offset {o:#x}"),
            }
        }
        let want: Vec<(u32, u32)> = bindings::RACE_FIXED.iter().map(|&(id, v)| (0x5E_9130 + 4 * u32::from(id), v)).collect();
        assert_eq!(got, want);
    }

    /// changed row, is not.
    #[test]
    fn untouched_keyboard_rows_are_recognised() {
        let row = |n: &str| bindings::CONTROLLER_ROWS.iter().position(|r| *r == n).map(|i| 100 + i as u16);
        let mut st = InputState::default();
        st.set_pads(&[gamepad::pad_record(&gamepad::PadSnapshot::default())]);
        st.default_controls(None, row);
        assert!(st.rows_are_keyboard_defaults(row));
        let mut changed = st.clone();
        changed.bindings.by_id.get_mut(&100).unwrap().index ^= 1;
        assert!(!changed.rows_are_keyboard_defaults(row));
        st.default_controls(Some(0), row);
        assert!(!st.rows_are_keyboard_defaults(row));
    }

    /// The modern pad layout (D44) on a standard pad at rest: every `Controller.*` row's binding.
    fn modern_pad_state(p: &gamepad::PadSnapshot) -> InputState {
        let row = |n: &str| {
            let i = bindings::CONTROLLER_ROWS.iter().position(|r| *r == n)?;
            const ROW_ACTIONS: [Action; 19] = [
                Action::Throttle, Action::Brake, Action::Handbrake, Action::SteerLeft, Action::SteerRight, Action::Horn,
                Action::ShiftUp, Action::ShiftDown, Action::RearView, Action::Lights, Action::Reset, Action::CameraNext,
                Action::Pause, Action::SignalLeft, Action::SignalRight, Action::Hazards, Action::LookLeft, Action::LookRight,
                Action::LookForward,
            ];
            binding_id(ROW_ACTIONS[i])
        };
        let mut st = InputState { axis_bands: [[0, 127]; 8], ..Default::default() };
        st.set_pads(&[gamepad::pad_record(p)]);
        st.default_controls(Some(0), row);
        let _ = st.take_pressed(Action::Pause);
        st
    }

    /// PAD-HANG (D44): the pad's Start on the pause row presses `I_PAUSE` once per push, through
    /// `set_pads` (the race frame reads it after `Gamepads::poll`).
    #[test]
    fn the_pads_start_presses_pause() {
        let rest = gamepad::PadSnapshot::default();
        let mut st = modern_pad_state(&rest);
        assert!(st.pad_bound(Action::Pause));
        let mut start = rest;
        start.buttons[gamepad::BUTTON_ORDER.iter().position(|b| *b == gamepad::Button::Start).unwrap()] = true;
        st.set_pads(&[gamepad::pad_record(&start)]);
        assert!(st.take_pressed(Action::Pause));
        st.set_pads(&[gamepad::pad_record(&start)]);
        assert!(!st.take_pressed(Action::Pause), "held: no second press");
        st.set_pads(&[gamepad::pad_record(&rest)]);
        st.set_pads(&[gamepad::pad_record(&start)]);
        assert!(st.take_pressed(Action::Pause), "the next push");
    }

    /// PAD-HANG (D44): a row bound to a pad control still answers its default key — Escape pauses, the
    /// arrows steer, accelerate and brake beside the pad, the larger of key and pad wins, and a
    /// pedal a key holds is ramped as a key's is.
    #[test]
    fn a_pad_bound_row_still_answers_its_default_key() {
        let rest = gamepad::PadSnapshot::default();
        let mut st = modern_pad_state(&rest);
        assert!(st.pad_bound(Action::Pause) && st.pad_bound(Action::Throttle) && st.pad_bound(Action::SteerLeft));
        st.key_changed(KeyCode::Escape, ElementState::Pressed);
        assert!(st.take_pressed(Action::Pause), "Escape pauses beside the pad's Start");
        st.key_changed(KeyCode::Escape, ElementState::Released);
        assert_eq!((st.throttle(), st.brake(), st.steer()), (0.0, 0.0, 0.0));
        assert_eq!(st.ramps(), (false, false, false), "axes are not ramped");
        st.key_changed(KeyCode::ArrowUp, ElementState::Pressed);
        st.key_changed(KeyCode::ArrowLeft, ElementState::Pressed);
        assert_eq!((st.throttle(), st.steer()), (1.0, -1.0));
        assert_eq!(st.ramps(), (true, true, false), "a key holding the control ramps it");
        st.key_changed(KeyCode::ArrowUp, ElementState::Released);
        st.key_changed(KeyCode::ArrowLeft, ElementState::Released);
        // the pad alone, then both: the larger read
        let half = gamepad::PadSnapshot { triggers: [0, 16384], sticks: [16384, 0, 0, 0], ..Default::default() };
        st.set_pads(&[gamepad::pad_record(&half)]);
        let pad_gas = st.throttle();
        assert!((0.49..0.52).contains(&pad_gas), "{pad_gas}");
        assert!(st.steer() > 0.4 && st.steer() < 0.6);
        st.key_changed(KeyCode::ArrowUp, ElementState::Pressed);
        assert_eq!(st.throttle(), 1.0);
        st.key_changed(KeyCode::ArrowLeft, ElementState::Pressed);
        assert!(st.steer() < 0.0, "the key's full left against the stick's half right: {}", st.steer());
        st.key_changed(KeyCode::ArrowUp, ElementState::Released);
        assert_eq!(st.throttle(), pad_gas);
    }

    /// PAD-HANG (D44): a default key another row is bound to belongs to that row.
    #[test]
    fn a_default_key_bound_to_another_row_is_that_rows() {
        let mut st = modern_pad_state(&gamepad::PadSnapshot::default());
        let p = bindings::Binding { kind: bindings::BindingKind::Key.as_u32(), device: 0, index: u32::from(scancode(KeyCode::KeyP).unwrap()), dir: 0, pov: 0, key: u32::from(b'p') };
        st.bindings.by_id.insert(binding_id(Action::Horn).unwrap(), p);
        st.key_changed(KeyCode::KeyP, ElementState::Pressed);
        assert!(st.held(Action::Horn));
        assert!(!st.take_pressed(Action::Pause), "P is the horn's now");
    }
}
