//! Joystick input via SDL3 (`sdl3-sys`, SDL built from source and linked statically; joystick /
//! gamepad / haptic subsystems only — no window, events polled from the game thread). Mapped onto the
//! same logical actions as the keyboard. `RE/notes/controls.md` §SDL3 has the move from SDL2.
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
//!
//! **And the band is a rescale, not a cut.** At race time the descriptor's `Analog` reader
//! **0x411B80** answers 0…255 from the axis's raw DirectInput value `v` (0…65535 — the image sets
//! no `DIPROP_RANGE`) and the two bytes as `L = lo·256`, `H = hi·256`:
//!
//! ```text
//! L < H:  v < L -> 0;  v > H -> 255;  else (v − L)·255 / (H − L)
//! L > H:  v > L -> 0;  v < H -> 255;  else (L − v)·255 / (L − H)
//! L = H:  0
//! ```
//!
//! and the packer 0x412E70 makes steering `(C1_RIGHT − C1_LEFT) / 4` (0x412F39…0x412F4B, `cdq /
//! and 3 / sar 2`, truncating). Steering is those two directional bindings on one axis, `dir` +1
//! and −1, so with the factory 0 / 0x80 the right half runs 0x80…0xFF and the left 0x80…0x00.
//! A pad drives the car only through its bindings (PORT-DEVIATION D44, PAD-MODERN: the port's own
//! fixed in-race pad map is gone). *(Corrected 2026-09-25: this said the original's pad bindings were `fe.txt`'s
//! commented examples only. `DEFAULTCONTROLS`' 0x51D1D0 binds a pad's X and Y axes and six of its
//! buttons, and the front end's factory runs it on the first pad at start-up —
//! `RE/notes/controls.md` §4.103a, `bindings::default_bindings`.)*

use crate::InputState;
use crate::bindings::PadRecord;
use sdl3_sys::events::*;
use sdl3_sys::gamepad::*;
use sdl3_sys::haptic::{SDL_IsJoystickHaptic, SDL_OpenHapticFromJoystick};
use sdl3_sys::hints::*;
use sdl3_sys::init::*;
use sdl3_sys::joystick::*;
use std::ffi::CStr;

/// The port's own names for a standard pad's buttons (the layout `gilrs` used to call
/// `South` … `RightThumb`; SDL's gamepad buttons map onto them in `Device::open` / [`face_buttons`]).
/// `C`, `Z` and, since PAD-MODERN, `LeftTrigger2` / `RightTrigger2` are never pressed and not in the
/// record's button mask: they only keep the numbers of the buttons after them (PORT-DEVIATION D25)
/// so a profile's `Select` … `RightThumb` bindings still read the same button. `LeftTrigger` /
/// `RightTrigger` are the **shoulder** buttons LB / RB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    South,
    East,
    North,
    West,
    C,
    Z,
    LeftTrigger,
    RightTrigger,
    LeftTrigger2,
    RightTrigger2,
    Select,
    Start,
    Mode,
    LeftThumb,
    RightThumb,
}

/// The port's own `Gamepads` state (the SDL context lives in [`Devices`]).
pub struct Gamepads {
    devices: Option<Devices>,
    /// the left stick's X / Y past [`MENU_STICK`] at the last motion event (the front end's keys)
    stick_latch: [bool; 2],
    rumbler: crate::ff::Rumbler,
    rumble_test: Option<RumbleTest>,
    /// the pads' records of the frame, reused (`poll` runs every frame)
    records_buf: Vec<PadRecord>,
    /// set by [`Gamepads::haptic_arm`]: the race's force feedback goes to the haptic device
    haptic_armed: bool,
    /// set by [`Gamepads::poll`] when a device that can rumble or has SDL haptic effects was
    /// connected; taken by [`Gamepads::take_ff_hotplug`]
    ff_hotplug: bool,
    /// set by [`Gamepads::poll`] when any device was connected; taken by [`Gamepads::take_pad_hotplug`]
    pad_hotplug: bool,
}

/// The last SDL error, for the log.
pub(crate) fn sdl_error() -> String {
    // SAFETY: SDL_GetError returns a valid, NUL-terminated string (possibly empty)
    unsafe { CStr::from_ptr(sdl3_sys::error::SDL_GetError()) }.to_string_lossy().into_owned()
}

/// One opened SDL joystick. The handles are SDL's own and are closed on drop.
struct Device {
    /// the joystick instance id (SDL3: stable for the device's connection)
    id: SDL_JoystickID,
    joy: *mut SDL_Joystick,
    /// the gamepad view of the same device, opened only for a **standard pad** (see
    /// [`Devices::open`]); null for wheels, sticks, pedals and anything SDL has no gamepad
    /// mapping for, which are read raw
    pad: *mut SDL_Gamepad,
    /// SDL's gamepad buttons for the port's `South`, `East`, `North`, `West` ([`face_buttons`])
    face: [SDL_GamepadButton; 4],
    /// the force-feedback device of a non-pad joystick SDL can drive ([`crate::ff::HapticWheel`])
    haptic: Option<crate::ff::HapticWheel>,
    name: String,
}

impl Drop for Device {
    fn drop(&mut self) {
        self.haptic = None;
        // SAFETY: both handles were opened by `Devices::open` and are closed once, here
        unsafe {
            if !self.pad.is_null() {
                SDL_CloseGamepad(self.pad);
            }
            SDL_CloseJoystick(self.joy);
        }
    }
}

/// The SDL context with only the joystick / gamepad (and, if it starts, haptic) subsystems — no video,
/// so no window and no focus change — and the devices it has opened, in connection order. Everything
/// runs on the thread that created it (the game's main thread).
pub struct Devices {
    list: Vec<Device>,
    haptic: bool,
}

const SUBSYSTEMS: SDL_InitFlags = SDL_InitFlags(SDL_INIT_JOYSTICK.0 | SDL_INIT_GAMEPAD.0);

impl Devices {
    fn new() -> Result<Self, String> {
        // SAFETY: plain SDL calls with valid NUL-terminated strings, on the thread that owns SDL
        unsafe {
            // a game in the background still reads its pad; no signal handlers of SDL's own
            SDL_SetHint(SDL_HINT_JOYSTICK_ALLOW_BACKGROUND_EVENTS, c"1".as_ptr());
            SDL_SetHint(SDL_HINT_NO_SIGNAL_HANDLERS, c"1".as_ptr());
            // macOS: SDL's own HID drivers (HIDAPI) take the Xbox / PlayStation / Switch pads they
            // know, the GameController framework (SDL's MFi driver) the rest, the IOKit joystick
            // driver wheels and pedals. PAD-FF (2026-10-27): with HIDAPI off every pad went through
            // the MFi driver, whose rumble is CoreHaptics (four `CHHapticEngine`s, two
            // `sendParameters` a call) — late and smeared on an Xbox pad, where HIDAPI
            // writes the pad's own rumble report at once.
            if !SDL_Init(SUBSYSTEMS) {
                return Err(sdl_error());
            }
            let haptic = SDL_InitSubSystem(SDL_INIT_HAPTIC);
            if !haptic {
                tracing::warn!(error = %sdl_error(), "SDL haptic subsystem unavailable (no wheel force feedback)");
            }
            let mut d = Self { list: Vec::new(), haptic };
            let mut n = 0;
            let ids = SDL_GetJoysticks(&mut n);
            if !ids.is_null() {
                for i in 0..n.max(0) as usize {
                    d.open(*ids.add(i));
                }
                sdl3_sys::stdinc::SDL_free(ids.cast());
            }
            let v = sdl3_sys::version::SDL_GetVersion();
            tracing::info!(sdl = format!("{}.{}.{}", v / 1_000_000, v / 1000 % 1000, v % 1000).as_str(), joysticks = d.list.len(), haptic, "SDL joystick layer started");
            Ok(d)
        }
    }

    /// Open the joystick with SDL instance id `id`. A device is read through the gamepad mapping
    /// only when SDL classes it as a **gamepad** (`SDL_JOYSTICK_TYPE_GAMEPAD`, SDL2's
    /// `GAMECONTROLLER`) and has a mapping (`SDL_IsGamepad`); a wheel, flight stick, throttle or
    /// pedal set — even one the controller database knows — is read raw, so every axis and slider
    /// stays visible (`RE/notes/controls.md`, PA-SDL). A non-pad device SDL can drive force feedback
    /// on gets its haptic device opened too.
    fn open(&mut self, id: SDL_JoystickID) {
        if self.list.iter().any(|d| d.id == id) {
            return;
        }
        // SAFETY: `id` comes from SDL (the device list or an added event); every handle is checked
        unsafe {
            let joy = SDL_OpenJoystick(id);
            if joy.is_null() {
                tracing::warn!(error = %sdl_error(), "joystick not opened");
                return;
            }
            let ty = SDL_GetJoystickType(joy);
            let standard = ty == SDL_JOYSTICK_TYPE_GAMEPAD && SDL_IsGamepad(id);
            let pad = if standard { SDL_OpenGamepad(id) } else { std::ptr::null_mut() };
            let face = if pad.is_null() {
                DEFAULT_FACE
            } else {
                face_buttons([SDL_GAMEPAD_BUTTON_SOUTH, SDL_GAMEPAD_BUTTON_EAST, SDL_GAMEPAD_BUTTON_WEST, SDL_GAMEPAD_BUTTON_NORTH].map(|b| SDL_GetGamepadButtonLabel(pad, b)))
            };
            let name = {
                let p = SDL_GetJoystickName(joy);
                if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
            };
            let haptic = if pad.is_null() && self.haptic && SDL_IsJoystickHaptic(joy) {
                let h = SDL_OpenHapticFromJoystick(joy);
                if h.is_null() {
                    tracing::warn!(name = name.as_str(), error = %sdl_error(), "haptic device not opened");
                    None
                } else {
                    let w = crate::ff::HapticWheel::new(h, ty == SDL_JOYSTICK_TYPE_WHEEL);
                    (w.features & crate::ff::FEAT_ANY != 0).then_some(w)
                }
            } else {
                None
            };
            let count = |n: i32| n.max(0);
            let path = SDL_GetJoystickPath(joy);
            let path = if path.is_null() { String::new() } else { std::ffi::CStr::from_ptr(path).to_string_lossy().into_owned() };
            tracing::info!(
                name = name.as_str(),
                path = path.as_str(),
                axes = count(SDL_GetNumJoystickAxes(joy)),
                hats = count(SDL_GetNumJoystickHats(joy)),
                buttons = count(SDL_GetNumJoystickButtons(joy)),
                mapped_pad = !pad.is_null(),
                haptic = haptic.as_ref().map_or(0, |h| h.features),
                wheel = ty == SDL_JOYSTICK_TYPE_WHEEL,
                "joystick connected"
            );
            self.list.push(Device { id, joy, pad, face, haptic, name });
        }
    }

    /// The record index (`records` order) of the standard pad with SDL instance id `which`.
    fn pad_index(&self, which: SDL_JoystickID) -> Option<usize> {
        self.list.iter().position(|x| x.id == which && !x.pad.is_null())
    }

    /// The name of the first open device that can rumble.
    pub(crate) fn rumble_device_name(&self) -> Option<&str> {
        self.list.iter().find(|d| d.can_rumble()).map(|d| d.name.as_str())
    }

    /// `SDL_RumbleGamepad` (`SDL_RumbleJoystick` on a raw device) on the first device that can:
    /// `low` / `high` are the strong / weak motors, 0…65535, for `ms`.
    pub(crate) fn rumble(&mut self, low: u16, high: u16, ms: u32) -> bool {
        match self.list.iter().find(|d| d.can_rumble()) {
            // SAFETY: the handles are open for as long as the device is in the list
            Some(d) if !d.pad.is_null() => unsafe { SDL_RumbleGamepad(d.pad, low, high, ms) },
            Some(d) => unsafe { SDL_RumbleJoystick(d.joy, low, high, ms) },
            None => false,
        }
    }

    /// The first device with an SDL haptic device (a wheel, …).
    fn haptic_device(&mut self) -> Option<(&str, &mut crate::ff::HapticWheel)> {
        self.list.iter_mut().find_map(|d| d.haptic.as_mut().map(|h| (d.name.as_str(), h)))
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.list.clear();
        // SAFETY: balances the `SDL_Init` / `SDL_InitSubSystem` of `Devices::new`
        unsafe {
            if self.haptic {
                SDL_QuitSubSystem(SDL_INIT_HAPTIC);
            }
            SDL_QuitSubSystem(SUBSYSTEMS);
        }
    }
}

impl Device {
    /// `SDL_PROP_JOYSTICK_CAP_RUMBLE_BOOLEAN` (SDL2's `SDL_JoystickHasRumble`).
    fn can_rumble(&self) -> bool {
        // SAFETY: the joystick is open; the property name is SDL's own constant
        unsafe { sdl3_sys::properties::SDL_GetBooleanProperty(SDL_GetJoystickProperties(self.joy), SDL_PROP_JOYSTICK_CAP_RUMBLE_BOOLEAN, false) }
    }
}

/// SDL3's buttons for the port's `South`, `East`, `North`, `West` on a pad that reports no labels
/// (or one whose labels match the positions — Xbox, PlayStation): the positions.
const DEFAULT_FACE: [SDL_GamepadButton; 4] = [SDL_GAMEPAD_BUTTON_SOUTH, SDL_GAMEPAD_BUTTON_EAST, SDL_GAMEPAD_BUTTON_NORTH, SDL_GAMEPAD_BUTTON_WEST];

/// The SDL3 face buttons that are the port's `South`, `East`, `North`, `West` (`BUTTON_ORDER` 0…3),
/// from the labels SDL reports for the south, east, west and north positions.
///
/// The port's numbering came from SDL2's game-controller names `A B Y X`, and SDL2 (2.26, the
/// bundled one until SDL3) named a Nintendo layout's buttons **by their labels**
/// (`SDL_GAMECONTROLLER_USE_BUTTON_LABELS`, on by default: `A` was the right-hand button labelled
/// A). SDL3's buttons are positional (`SOUTH` is the bottom one whatever it reads), so each of `A B
/// Y X` is looked up by its label to keep a Switch-style pad's bindings and the front end's A / B
/// where they were; PlayStation's ✕ ○ △ □ are `A B Y X`; a pad without labels keeps the positions.
pub fn face_buttons(labels: [SDL_GamepadButtonLabel; 4]) -> [SDL_GamepadButton; 4] {
    use SDL_GamepadButtonLabel as L;
    let positions = [SDL_GAMEPAD_BUTTON_SOUTH, SDL_GAMEPAD_BUTTON_EAST, SDL_GAMEPAD_BUTTON_WEST, SDL_GAMEPAD_BUTTON_NORTH];
    let wanted = [[L::A, L::CROSS], [L::B, L::CIRCLE], [L::Y, L::TRIANGLE], [L::X, L::SQUARE]];
    let mut out = DEFAULT_FACE;
    for (slot, w) in wanted.iter().enumerate() {
        if let Some(n) = labels.iter().position(|l| w.contains(l)) {
            out[slot] = positions[n];
        }
    }
    // a partial or odd label set that would bind one SDL button twice keeps the positions
    if (0..4).any(|i| (i + 1..4).any(|j| out[i] == out[j])) { DEFAULT_FACE } else { out }
}
/// `Analog`'s reader **0x411B80**: raw axis `v` (0…65535) against the descriptor's `lo`/`hi`
/// bytes, 0…255. Integer arithmetic as the original's (`idiv`, truncating).
pub fn analog_read(v: i32, lo: i32, hi: i32) -> i32 {
    let (l, h) = (lo << 8, hi << 8);
    if l < h {
        if v < l {
            0
        } else if v > h {
            255
        } else {
            (v - l) * 255 / (h - l)
        }
    } else if l > h {
        if v > l {
            0
        } else if v < h {
            255
        } else {
            (l - v) * 255 / (l - h)
        }
    } else {
        0
    }
}

/// One directional binding's two bytes, as 0x51CEA1…0x51CEDE bake them: `min·dir + 0x80` floored
/// at 0, `max·dir + 0x80` capped at 0xFF.
pub fn axis_band(min: i32, max: i32, dir: i32) -> (i32, i32) {
    ((min * dir + 0x80).max(0), (max * dir + 0x80).min(0xFF))
}

/// The packet's steering from a stick at `x` (−1…1): the raw value DirectInput would report,
/// read once for `C1_RIGHT` (`dir` +1) and once for `C1_LEFT` (−1), then `(right − left) / 4`
/// (0x412F39). −63…63, the packet byte.
pub fn stick_steer(x: f32, min: i32, max: i32) -> i32 {
    let v = (((x.clamp(-1.0, 1.0) + 1.0) * 0.5) * 65535.0).round() as i32;
    let (rl, rh) = axis_band(min, max, 1);
    let (ll, lh) = axis_band(min, max, -1);
    (analog_read(v, rl, rh) - analog_read(v, ll, lh)) / 4
}

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
struct RumbleTest {
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
    pub fn new() -> Self {
        let devices = match Devices::new() {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!(error = %e, "joystick support unavailable");
                None
            }
        };
        Self { devices, stick_latch: [false; 2], rumbler: Default::default(), rumble_test: None, records_buf: Vec::new(), haptic_armed: false, ff_hotplug: false, pad_hotplug: false }
    }

    /// Whether a device that can rumble or has SDL haptic effects was connected since the last call
    /// (PAD-HANG): the race arms force feedback once, at its start (0x4121B0), so a pad that
    /// arrives later — a Bluetooth pad whose GameController discovery ends after the arm, one the
    /// player switches on mid-race — would otherwise never rumble.
    pub fn take_ff_hotplug(&mut self) -> bool {
        std::mem::take(&mut self.ff_hotplug)
    }

    /// Whether a device was connected since the last call (PORT-DEVIATION D44 item 9: the caller
    /// gives untouched keyboard rows the pad's layout).
    pub fn take_pad_hotplug(&mut self) -> bool {
        std::mem::take(&mut self.pad_hotplug)
    }

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
        match self.devices.as_mut() {
            Some(d) => self.rumbler.stop_all(d),
            None => {}
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

    /// The first standard pad's state in SDL's units (sticks, triggers, d-pad, the port's 15
    /// buttons), `None` without one — for code that reads a pad directly rather than through the
    /// bindings.
    pub fn pad_snapshot(&self) -> Option<PadSnapshot> {
        self.devices.as_ref()?.list.iter().find_map(Device::snapshot)
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

    /// Drain SDL's events, open / drop hot-plugged devices, queue the front end's pad keys
    /// ([`crate::MenuNav`]) and hand every device's record to the bindings (`InputState::set_pads`).
    /// Call once per frame (on the game thread: the pump belongs to the thread that initialised SDL).
    ///
    /// PORT-DEVIATION D44 (PAD-MODERN): in a race a pad works **only through its bindings**, as the
    /// original's packer 0x412E70 reads a pad (`Analog` 0x411B80, `Button` 0x411A50, `Pov`
    /// 0x411AC0). The port's own fixed pad map (A throttle, B brake, the triggers as gas / brake,
    /// the left stick steering for a control no binding read) is gone; only the front end's
    /// navigation keys stay the port's own (PORT-DEVIATION D10).
    pub fn poll(&mut self, input: &mut InputState) {
        let Some(d) = self.devices.as_mut() else { return };
        let mut ev = SDL_Event::default();
        // SAFETY: SDL_PollEvent fills `ev`; each arm reads the union member its type names
        while unsafe { SDL_PollEvent(&mut ev) } {
            let ty = SDL_EventType(unsafe { ev.r#type });
            if ty == SDL_EVENT_JOYSTICK_ADDED {
                let which = unsafe { ev.jdevice.which };
                d.open(which);
                self.pad_hotplug = true;
                // a force-feedback device arriving (a Bluetooth pad's MFi discovery finishes after
                // the race has armed, or the player connects one mid-race): the caller re-arms
                if d.list.iter().any(|x| x.id == which && (x.haptic.is_some() || x.can_rumble())) {
                    self.ff_hotplug = true;
                }
            } else if ty == SDL_EVENT_JOYSTICK_REMOVED {
                let which = unsafe { ev.jdevice.which };
                if let Some(i) = d.list.iter().position(|x| x.id == which) {
                    tracing::info!(name = d.list[i].name.as_str(), "joystick disconnected");
                    d.list.remove(i);
                }
            } else if ty == SDL_EVENT_GAMEPAD_BUTTON_DOWN {
                // the front end's pad keys (D10): a standard pad's d-pad, A and B, and the left
                // stick past 0.6 of its travel (latched, one key per push)
                let b = unsafe { ev.gbutton };
                if let Some(i) = d.pad_index(b.which)
                    && let Some(k) = menu_button(SDL_GamepadButton(i32::from(b.button)), &d.list[i].face)
                {
                    input.pad_menu.push(k);
                }
            } else if ty == SDL_EVENT_GAMEPAD_AXIS_MOTION {
                let a = unsafe { ev.gaxis };
                if d.pad_index(a.which).is_none() {
                    continue;
                }
                let axis = SDL_GamepadAxis(i32::from(a.axis));
                let (i, neg, pos) = if axis == SDL_GAMEPAD_AXIS_LEFTX {
                    (0, crate::MenuNav::Left, crate::MenuNav::Right)
                } else if axis == SDL_GAMEPAD_AXIS_LEFTY {
                    (1, crate::MenuNav::Up, crate::MenuNav::Down)
                } else {
                    continue;
                };
                let v = unit(a.value);
                let now = v.abs() > MENU_STICK;
                if now && !self.stick_latch[i] {
                    input.pad_menu.push(if v < 0.0 { neg } else { pos });
                }
                self.stick_latch[i] = now;
            }
        }
        // a sustained rumble change held back by the rate limit goes out once its interval is over
        self.rumbler.flush(d);
        let mut records = std::mem::take(&mut self.records_buf);
        self.records_into(&mut records);
        input.set_pads(&records);
        self.records_buf = records;
    }
}

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

/// PORT-DEVIATION D46: an SDL trigger (0…32767), 0…1.
fn trigger(v: i16) -> f32 {
    (f32::from(v.max(0)) / 32767.0).clamp(0.0, 1.0)
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

/// The left stick's travel past which the front end takes a direction key (D10).
const MENU_STICK: f32 = 0.6;

/// A standard pad's button as a front-end key (PORT-DEVIATION D10; the original's front end reads
/// no pad, `RE/notes/controls.md` §4.169.5): the d-pad moves, A (the port's `South`) selects, B
/// (`East`) goes back. `face` is the pad's [`face_buttons`].
pub fn menu_button(b: SDL_GamepadButton, face: &[SDL_GamepadButton; 4]) -> Option<crate::MenuNav> {
    use crate::MenuNav::*;
    Some(if b == SDL_GAMEPAD_BUTTON_DPAD_UP {
        Up
    } else if b == SDL_GAMEPAD_BUTTON_DPAD_DOWN {
        Down
    } else if b == SDL_GAMEPAD_BUTTON_DPAD_LEFT {
        Left
    } else if b == SDL_GAMEPAD_BUTTON_DPAD_RIGHT {
        Right
    } else if b == face[0] {
        Select
    } else if b == face[1] {
        Back
    } else if b == face[2] {
        Alt
    } else if b == face[3] {
        Aux
    } else {
        return None;
    })
}

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

/// An SDL axis, −1…1.
fn unit(v: i16) -> f32 {
    (f32::from(v) / 32767.0).clamp(-1.0, 1.0)
}

/// The port's button numbering for [`Gamepads::records`] — see there.
pub const BUTTON_ORDER: [Button; 15] = [
    Button::South,
    Button::East,
    Button::North,
    Button::West,
    Button::C,
    Button::Z,
    Button::LeftTrigger,
    Button::RightTrigger,
    Button::LeftTrigger2,
    Button::RightTrigger2,
    Button::Select,
    Button::Start,
    Button::Mode,
    Button::LeftThumb,
    Button::RightThumb,
];

/// One connected device's axis set, as the front end's dead-zone screen needs it.
///
/// The original reads this out of DirectInput: `0x532E10(i, 2)->[+8]` is an **axis bitmask** whose
/// bit `n` is axis `n` of `DIJOYSTATE` — `lX, lY, lZ, lRx, lRy, lRz, rglSlider[0], rglSlider[1]` —
/// and `[rec+4 + 4n]` is that axis's current value (`RE/notes/controls.md` §4.17).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadAxisSet {
    /// bit `n` set when the pad has axis `n`, in `DIJOYSTATE` order
    pub mask: u8,
    /// each axis's current value, scaled to the 0…0x80 the dead-zone accessors use
    pub values: [i32; 8],
}

/// What a **standard pad** (an SDL gamepad with a mapping) reports, in SDL's units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadSnapshot {
    /// left stick X / Y, right stick X / Y (SDL: Y grows downward, as DirectInput's)
    pub sticks: [i16; 4],
    /// left / right trigger, 0…32767
    pub triggers: [i16; 2],
    /// d-pad up / right / down / left
    pub dpad: [bool; 4],
    /// the 15 buttons in [`BUTTON_ORDER`] (the entries of `C`, `Z` and the two trigger slots are
    /// ignored by [`pad_record`])
    pub buttons: [bool; 15],
}

/// A raw SDL joystick's state: `SDL_GetJoystickAxis` per axis, `SDL_GetJoystickHat` per hat (SDL's
/// bits `up 1 | right 2 | down 4 | left 8`), `SDL_GetJoystickButton` per button.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawSnapshot {
    pub axes: Vec<i16>,
    pub hats: Vec<u8>,
    pub buttons: Vec<bool>,
}

/// An SDL axis value (−32768…32767) as DirectInput's default 0…65535 range, the one the image
/// keeps (it sets no `DIPROP_RANGE`, 0x56FD70).
fn di_value(v: i16) -> i32 {
    i32::from(v) + 32768
}

/// A trigger (SDL 0…32767) as a DirectInput axis value resting at the **centre**: 0x8000 released,
/// 0xFFFF fully pressed. PORT-DEVIATION D44 (PAD-MODERN): the trigger's travel is the upper half
/// of the axis, so `Analog`'s reader 0x411B80 under a `dir` +1 binding — whose band 0x51CEA4 /
/// 0x51CED1 bake as `0x80 + <Axis>.Min` … `0x80 + <Axis>.Max` — reads **0 at rest and 255 fully
/// pressed** over the whole travel, the factory `Min` 0 being no dead zone and a `Min` set on the
/// dead-zone screen one at the released end. Had it rested at 0 (the axis's own end), a `+1`
/// binding would read nothing over the first half of the travel and a `−1` one 255 at rest.
fn trigger_value(v: i16) -> i32 {
    0x8000 + i32::from(v.max(0))
}

/// A **standard pad** as the original's device record (PORT-DEVIATION D44, PAD-MODERN: a
/// modern-pad layout). The axes are DirectInput's six: the sticks on `lX lY` and `lRx lRy`
/// (Y growing downward, as DirectInput's), **LT on `lZ`** and **RT on `lRz`** ([`trigger_value`]),
/// mask 0x3F — two separate analogue triggers, both pressable at once, where DirectInput's own
/// Xbox driver folds both into one `lZ`. *(Until PAD-MODERN the triggers were buttons 8 / 9 past
/// three quarters of their travel, an APPROXIMATION; a profile that bound those buttons must be
/// rebound.)* Buttons are the port's numbering ([`BUTTON_ORDER`]; PORT-DEVIATION D25) and
/// `button_mask` is every one the standard mapping has (all but `C`, `Z` and the two trigger
/// slots). The d-pad is the first hat.
pub fn pad_record(p: &PadSnapshot) -> PadRecord {
    let mut r = PadRecord { mask: 0x3F, standard: true, ..Default::default() };
    for (slot, v) in [0usize, 1, 3, 4].into_iter().zip(p.sticks) {
        r.axes[slot] = di_value(v);
    }
    r.axes[2] = trigger_value(p.triggers[0]);
    r.axes[5] = trigger_value(p.triggers[1]);
    for (n, b) in BUTTON_ORDER.iter().enumerate() {
        if !matches!(b, Button::C | Button::Z | Button::LeftTrigger2 | Button::RightTrigger2) {
            r.buttons[n] = p.buttons[n];
            r.button_mask |= 1 << n;
        }
    }
    r.pov[0] = hat_of(p.dpad[0], p.dpad[1], p.dpad[2], p.dpad[3]);
    r
}

/// Any other joystick — a wheel, pedals, a stick, a throttle quadrant — as the original's device
/// record, **every axis, hat and button it has**. SDL lists a joystick's axes in the order the
/// device's HID report declares them (X, Y, Z, Rx, Ry, Rz, then sliders / dials), which is the
/// order of `DIJOYSTATE`'s `lX lY lZ lRx lRy lRz rglSlider[0] rglSlider[1]`, so SDL axis `n` is
/// DirectInput axis `n` and the axes past the sixth are the two sliders (`RE/notes/controls.md`
/// §4.17; **APPROXIMATION**: the index equivalence is by the HID usage order, not read from the
/// device's usages; a device that declares its axes in another order shows them in that order).
/// Axes past the eighth, hats past the fourth and buttons past the 32nd do not fit the
/// record and are dropped. SDL's raw axis values are used unscaled (centre 32768 of 0…65535).
pub fn raw_record(s: &RawSnapshot) -> PadRecord {
    let mut r = PadRecord::default();
    for (n, &v) in s.axes.iter().take(8).enumerate() {
        r.mask |= 1 << n;
        r.axes[n] = di_value(v);
    }
    for (n, &h) in s.hats.iter().take(4).enumerate() {
        r.pov[n] = hat_of(h & 1 != 0, h & 2 != 0, h & 4 != 0, h & 8 != 0);
    }
    for (n, &b) in s.buttons.iter().take(32).enumerate() {
        r.button_mask |= 1 << n;
        r.buttons[n] = b;
    }
    r
}

impl PadSnapshot {
    /// The port's button `b` ([`BUTTON_ORDER`]).
    pub fn button(&self, b: Button) -> bool {
        BUTTON_ORDER.iter().position(|x| *x == b).is_some_and(|n| self.buttons[n])
    }
}

impl Device {
    /// A standard pad's state, `None` for a raw device.
    fn snapshot(&self) -> Option<PadSnapshot> {
        if self.pad.is_null() {
            return None;
        }
        let c = self.pad;
        // SAFETY: the gamepad is open for as long as the device is in the list
        let axis = |a: SDL_GamepadAxis| unsafe { SDL_GetGamepadAxis(c, a) };
        let button = |b: SDL_GamepadButton| unsafe { SDL_GetGamepadButton(c, b) };
        let mut p = PadSnapshot {
            sticks: [SDL_GAMEPAD_AXIS_LEFTX, SDL_GAMEPAD_AXIS_LEFTY, SDL_GAMEPAD_AXIS_RIGHTX, SDL_GAMEPAD_AXIS_RIGHTY].map(axis),
            // SDL3: 0…32767, as SDL2's
            triggers: [SDL_GAMEPAD_AXIS_LEFT_TRIGGER, SDL_GAMEPAD_AXIS_RIGHT_TRIGGER].map(axis),
            dpad: [SDL_GAMEPAD_BUTTON_DPAD_UP, SDL_GAMEPAD_BUTTON_DPAD_RIGHT, SDL_GAMEPAD_BUTTON_DPAD_DOWN, SDL_GAMEPAD_BUTTON_DPAD_LEFT].map(button),
            buttons: [false; 15],
        };
        for (n, b) in BUTTON_ORDER.iter().enumerate() {
            if let Some(sdl) = sdl_button(*b, &self.face) {
                p.buttons[n] = button(sdl);
            }
        }
        Some(p)
    }

    fn record(&self) -> PadRecord {
        if let Some(p) = self.snapshot() {
            return pad_record(&p);
        }
        let j = self.joy;
        // SAFETY: the joystick is open for as long as the device is in the list
        let snap = unsafe {
            RawSnapshot {
                axes: (0..SDL_GetNumJoystickAxes(j).max(0)).map(|i| SDL_GetJoystickAxis(j, i)).collect(),
                // SDL's hat bits are SDL2's: up 1 | right 2 | down 4 | left 8
                hats: (0..SDL_GetNumJoystickHats(j).max(0)).map(|i| SDL_GetJoystickHat(j, i)).collect(),
                buttons: (0..SDL_GetNumJoystickButtons(j).max(0)).map(|i| SDL_GetJoystickButton(j, i)).collect(),
            }
        };
        raw_record(&snap)
    }
}

/// The SDL3 gamepad button that is the port's button `b` on a pad whose face buttons are `face`
/// ([`face_buttons`]); `None` for the slots a standard pad has not (`C`, `Z`, the trigger slots).
/// The rest are SDL2's game-controller names, one for one: LB / RB `LEFT_SHOULDER` /
/// `RIGHT_SHOULDER`, `BACK`, `START`, `GUIDE`, `LEFT_STICK`, `RIGHT_STICK`.
pub fn sdl_button(b: Button, face: &[SDL_GamepadButton; 4]) -> Option<SDL_GamepadButton> {
    Some(match b {
        Button::South => face[0],
        Button::East => face[1],
        Button::North => face[2],
        Button::West => face[3],
        Button::LeftTrigger => SDL_GAMEPAD_BUTTON_LEFT_SHOULDER,
        Button::RightTrigger => SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER,
        Button::Select => SDL_GAMEPAD_BUTTON_BACK,
        Button::Start => SDL_GAMEPAD_BUTTON_START,
        Button::Mode => SDL_GAMEPAD_BUTTON_GUIDE,
        Button::LeftThumb => SDL_GAMEPAD_BUTTON_LEFT_STICK,
        Button::RightThumb => SDL_GAMEPAD_BUTTON_RIGHT_STICK,
        Button::C | Button::Z | Button::LeftTrigger2 | Button::RightTrigger2 => return None,
    })
}

impl Gamepads {
    /// Every input device the layer enumerates, with the flag that says whether it is a **pad**.
    ///
    /// The original builds `Controller.AllControllers` and `Controller.AllPads` at 0x511809 from
    /// exactly this: `0x51F050()` — `0x532E10(0, 1)`, the count at `[0x69CB0C]` — then for each
    /// index `0x51F0C0(i)` to skip a device, `0x51F060(i)` for its name (the descriptor
    /// `0x532E10(i, 2)` returns, name at **+0x14**) and `0x51F080(i)` to decide whether the name
    /// also goes on the pads' list. `RE/notes/controls.md` §4.103.
    ///
    /// That enumeration is DirectInput's, and it carries the keyboard too; SDL's joystick list has
    /// only game devices (pads, wheels, sticks, pedals), so here every device is a pad and the two
    /// lists come out the same.
    pub fn devices(&self) -> Vec<(String, bool)> {
        let Some(d) = self.devices.as_ref() else { return Vec::new() };
        d.list.iter().map(|x| (x.name.clone(), true)).collect()
    }

    /// Every device's reading as the original's device record (`bindings::PadRecord`): see
    /// [`pad_record`] (standard pads) and [`raw_record`] (everything else). DirectInput numbers a
    /// device's buttons in its own HID order; a standard pad's numbering is `BUTTON_ORDER`, a raw
    /// device's is SDL's button index (PORT-DEVIATION D25). The capture and the reader use the same
    /// order, so what is bound is what is read.
    pub fn records(&self) -> Vec<PadRecord> {
        let mut out = Vec::new();
        self.records_into(&mut out);
        out
    }

    /// [`records`](Self::records) into `out`, which is cleared first (the per-frame path reuses one
    /// buffer).
    pub fn records_into(&self, out: &mut Vec<PadRecord>) {
        out.clear();
        let Some(d) = self.devices.as_ref() else { return };
        out.extend(d.list.iter().map(Device::record));
    }

    /// The connected devices' axis sets, in connection order — what `NFS5.DEADZONE` draws from.
    /// `[rec+4+4n]` is the **device's** own reading, not the accessor's: the image sets no
    /// `DIPROP_RANGE` (0x56FD70's only `SetProperty` is `DIPROP_BUFFERSIZE`), so an absolute axis
    /// keeps DirectInput's default 0…65535 with the centre at 32767, the range `NFS5.DEADZONE`'s
    /// marker divides by 65536 (0x51B819). `DEFAULTCONTROLS` puts `Accel` on Y **−1** and `Brake` on
    /// Y **+1** (0x51D60A…0x51D6EB), which is stick forward to accelerate only with a Y that grows
    /// downward — SDL's does, as DirectInput's.
    pub fn axis_sets(&self) -> Vec<PadAxisSet> {
        let Some(d) = self.devices.as_ref() else { return Vec::new() };
        d.list
            .iter()
            .map(|x| {
                let r = x.record();
                PadAxisSet { mask: r.mask, values: r.axes.map(|v| v.clamp(0, 0xFFFF)) }
            })
            .collect()
    }
}

/// DirectInput's hat for the d-pad buttons (up, right, down, left): hundredths of a degree
/// clockwise from north, −1 centred (and for the combinations no hat reports). Indexed by the
/// four buttons as bits `up | right << 1 | down << 2 | left << 3`.
const HAT: [i32; 16] = [-1, 0, 9000, 4500, 18000, -1, 13500, -1, 27000, 31500, -1, -1, 22500, -1, -1, -1];

fn hat_of(up: bool, right: bool, down: bool, left: bool) -> i32 {
    HAT[up as usize | (right as usize) << 1 | (down as usize) << 2 | (left as usize) << 3]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0x411B80 over the factory band (0 / 0x80): centred reads nothing, either end reads full
    /// lock, and the two halves are the original's slightly different spans (0x80…0xFF right,
    /// 0x80…0x00 left).
    #[test]
    fn the_hat_table_is_the_original_match() {
        for bits in 0..16usize {
            let (u, rt, d, l) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);
            let old = match (u, rt, d, l) {
                (true, false, false, false) => 0,
                (true, true, false, false) => 4500,
                (false, true, false, false) => 9000,
                (false, true, true, false) => 13500,
                (false, false, true, false) => 18000,
                (false, false, true, true) => 22500,
                (false, false, false, true) => 27000,
                (true, false, false, true) => 31500,
                _ => -1,
            };
            assert_eq!(hat_of(u, rt, d, l), old, "buttons {bits:04b}");
        }
    }

    #[test]
    fn the_stick_reads_through_0x411b80() {
        assert_eq!(axis_band(0, 0x80, 1), (0x80, 0xFF));
        assert_eq!(axis_band(0, 0x80, -1), (0x80, 0x00));
        assert_eq!(stick_steer(0.0, 0, 0x80), 0);
        assert_eq!(stick_steer(1.0, 0, 0x80), 63);
        assert_eq!(stick_steer(-1.0, 0, 0x80), -63);
        // a rescale, not a cut: half-way right is about half lock
        assert_eq!(stick_steer(0.5, 0, 0x80), 32);
        // a dead zone of 0x20 reads nothing inside it and still reaches full lock at the end
        assert_eq!(stick_steer(0.2, 0x20, 0x80), 0);
        assert_eq!(stick_steer(1.0, 0x20, 0x80), 63);
        // L = H answers 0
        assert_eq!(analog_read(40000, 0x90, 0x90), 0);
    }

    /// A steering wheel with its pedals on separate axes and a clutch / dial on two sliders:
    /// SDL axis `n` is DirectInput axis `n`, the 7th and 8th are the sliders (bits 6, 7).
    #[test]
    fn a_wheel_with_pedals_and_sliders_keeps_every_axis() {
        let s = RawSnapshot {
            // wheel, gas, brake, clutch (Rx), Ry, Rz, slider0, slider1, and one too many
            axes: vec![-32768, 0, 32767, 100, -100, 0, 20000, -20000, 5],
            hats: vec![0, 2, 4, 8, 1],
            buttons: (0..40).map(|n| n == 3 || n == 33).collect(),
        };
        let r = raw_record(&s);
        assert_eq!(r.mask, 0xFF);
        assert_eq!(r.axes[0], 0);
        assert_eq!(r.axes[1], 32768);
        assert_eq!(r.axes[2], 65535);
        assert_eq!(r.axes[6], 52768);
        assert_eq!(r.axes[7], 12768);
        // centred, right, down, left; the fifth hat does not fit
        assert_eq!(r.pov, [-1, 9000, 18000, 27000]);
        assert_eq!(r.button_mask, u32::MAX);
        assert!(r.buttons[3] && !r.buttons[2]);
        // a combined-axis wheel (steering + one pedal axis) has three bits only
        let two = raw_record(&RawSnapshot { axes: vec![0, 0, 0], ..Default::default() });
        assert_eq!(two.mask, 0b111);
        assert_eq!(two.button_mask, 0);
    }

    /// Moving a pedal axis of such a wheel is captured as an axis binding, in the direction it
    /// moved (0x51F3B0), and the binding then reads it back.
    #[test]
    fn a_pedal_axis_can_be_captured_and_read() {
        use crate::bindings::{BindingKind, axis_read, capture};
        let mut s = RawSnapshot { axes: vec![0; 4], ..Default::default() };
        s.axes[0] = 0;
        let base = raw_record(&s);
        s.axes[2] = 30000;
        let cur = raw_record(&s);
        let b = capture(&[base], &[cur]).expect("the moved axis is captured");
        assert_eq!(b.kind_class(), BindingKind::Axis);
        let bands = [[0, 0x80]; 8];
        assert!(axis_read(&b, &[cur], &bands) > 0x80);
        assert_eq!(axis_read(&b, &[base], &bands), 0);
    }

    /// PAD-MODERN (D44): a standard pad's record has the six axes, LT on `lZ` and RT on `lRz`
    /// resting at the centre, the trigger slots 8 / 9 never pressed nor in the mask, the d-pad as
    /// the first hat.
    #[test]
    fn a_standard_pad_has_two_trigger_axes() {
        let mut p = PadSnapshot::default();
        let rest = pad_record(&p);
        assert!(rest.standard);
        assert_eq!(rest.mask, 0x3F);
        assert_eq!(rest.button_mask, 0x7FFF & !0x30 & !0x300);
        assert_eq!((rest.axes[2], rest.axes[5]), (0x8000, 0x8000));
        assert_eq!(rest.pov[0], -1);
        p.sticks = [32767, -32768, 0, 32767];
        p.triggers = [32767, 16384];
        p.dpad = [true, true, false, false];
        p.buttons[0] = true;
        p.buttons[8] = true; // whatever the snapshot says, slot 8 is no button any more
        let r = pad_record(&p);
        assert_eq!((r.axes[0], r.axes[1], r.axes[3], r.axes[4]), (65535, 0, 32768, 65535));
        assert_eq!((r.axes[2], r.axes[5]), (0xFFFF, 0x8000 + 16384));
        assert!(r.buttons[0] && !r.buttons[8] && !r.buttons[9]);
        assert_eq!(r.pov[0], 4500);
        // a raw device is not a standard pad
        assert!(!raw_record(&RawSnapshot::default()).standard);
    }

    /// PAD-MODERN: a trigger under a `dir` +1 binding reads 0 released and 255 fully pressed through
    /// 0x411B80, in proportion between, with the factory band (0 / 127) and with a dead zone; and
    /// gas / brake on the two triggers are analogue, unramped (0x51CC00: kind 1) and both readable
    /// at once.
    #[test]
    fn a_trigger_axis_reads_0_at_rest_and_255_pressed() {
        use crate::bindings::{Binding, axis_read, id};
        let gas = Binding { kind: 1, device: 0, index: 5, dir: 1, pov: 0, key: 0 };
        let brake = Binding { kind: 1, device: 0, index: 2, dir: 1, pov: 0, key: 0 };
        let factory = [[0, 127]; 8];
        let rec = |lt: i16, rt: i16| pad_record(&PadSnapshot { triggers: [lt, rt], ..Default::default() });
        assert_eq!(axis_read(&gas, &[rec(0, 0)], &factory), 0);
        assert_eq!(axis_read(&gas, &[rec(0, 32767)], &factory), 255);
        assert_eq!(axis_read(&brake, &[rec(32767, 0)], &factory), 255);
        assert_eq!(axis_read(&brake, &[rec(0, 32767)], &factory), 0);
        let half = axis_read(&gas, &[rec(0, 16384)], &factory);
        assert!((126..=130).contains(&half), "half travel reads {half}");
        // a dead zone of 0x10 on the released end: a light touch reads nothing, full still 255
        let dz = [[0x10, 127]; 8];
        assert_eq!(axis_read(&gas, &[rec(0, 0x0F00)], &dz), 0);
        assert_eq!(axis_read(&gas, &[rec(0, 32767)], &dz), 255);
        let mut st = InputState { axis_bands: factory, ..Default::default() };
        st.bindings.by_id.insert(id::C1_GAS, gas);
        st.bindings.by_id.insert(id::C1_BRAKE, brake);
        assert_eq!(st.ramps(), (true, false, false), "gas and brake on axes are not ramped");
        st.set_pads(&[rec(32767, 16384)]);
        assert_eq!(st.brake(), 1.0);
        assert!((0.49..0.52).contains(&st.throttle()), "throttle {}", st.throttle());
        // released: nothing, so a held brake at a standstill (`brake_raw > 0x80`, throttle 0,
        // drivetrain 0x49DD92…) still reaches the reverse
        st.set_pads(&[rec(32767, 0)]);
        assert_eq!(st.throttle(), 0.0);
        assert!(((st.brake() * 126.0) as u8).min(0x7E) * 2 > 0x80);
    }

    /// PAD-MODERN: the front end's pad keys (D10) are the d-pad, A and B (Y and X are the race's dialog keys, PAD-END2).
    #[test]
    fn the_front_end_pad_keys() {
        use crate::MenuNav;
        let f = DEFAULT_FACE;
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_DPAD_UP, &f), Some(MenuNav::Up));
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_DPAD_LEFT, &f), Some(MenuNav::Left));
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_SOUTH, &f), Some(MenuNav::Select));
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_EAST, &f), Some(MenuNav::Back));
        // PAD-END2: Y and X are the in-race dialogs' second answer and the replay bar's save
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_NORTH, &f), Some(MenuNav::Alt));
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_WEST, &f), Some(MenuNav::Aux));
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER, &f), None);
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_START, &f), None);
    }

    /// SDL3: the port's `A B Y X` (South, East, North, West) are found by the labels SDL reports, as
    /// SDL2's label-named buttons were: an Xbox or PlayStation pad keeps the positions, a Nintendo
    /// layout (B at the bottom, A on the right) has the port's `South` on SDL3's `EAST`.
    #[test]
    fn the_face_buttons_follow_sdl2s_names() {
        use SDL_GamepadButtonLabel as L;
        // labels of the south, east, west, north positions
        assert_eq!(face_buttons([L::A, L::B, L::X, L::Y]), DEFAULT_FACE);
        assert_eq!(face_buttons([L::CROSS, L::CIRCLE, L::SQUARE, L::TRIANGLE]), DEFAULT_FACE);
        assert_eq!(face_buttons([L::UNKNOWN; 4]), DEFAULT_FACE);
        let nintendo = face_buttons([L::B, L::A, L::Y, L::X]);
        assert_eq!(nintendo, [SDL_GAMEPAD_BUTTON_EAST, SDL_GAMEPAD_BUTTON_SOUTH, SDL_GAMEPAD_BUTTON_WEST, SDL_GAMEPAD_BUTTON_NORTH]);
        assert_eq!(menu_button(SDL_GAMEPAD_BUTTON_EAST, &nintendo), Some(crate::MenuNav::Select));
        // a label set that would read one button twice keeps the positions
        assert_eq!(face_buttons([L::A, L::A, L::X, L::Y]), DEFAULT_FACE);
    }

    /// The port's 15 buttons on SDL3's gamepad buttons (SDL2's `A B Y X LB RB Back Start Guide LS RS`),
    /// the four placeholder slots on none.
    #[test]
    fn the_port_buttons_on_sdl3() {
        let f = DEFAULT_FACE;
        let got: Vec<_> = BUTTON_ORDER.iter().map(|b| sdl_button(*b, &f)).collect();
        assert_eq!(
            got,
            [
                Some(SDL_GAMEPAD_BUTTON_SOUTH),
                Some(SDL_GAMEPAD_BUTTON_EAST),
                Some(SDL_GAMEPAD_BUTTON_NORTH),
                Some(SDL_GAMEPAD_BUTTON_WEST),
                None,
                None,
                Some(SDL_GAMEPAD_BUTTON_LEFT_SHOULDER),
                Some(SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER),
                None,
                None,
                Some(SDL_GAMEPAD_BUTTON_BACK),
                Some(SDL_GAMEPAD_BUTTON_START),
                Some(SDL_GAMEPAD_BUTTON_GUIDE),
                Some(SDL_GAMEPAD_BUTTON_LEFT_STICK),
                Some(SDL_GAMEPAD_BUTTON_RIGHT_STICK),
            ]
        );
        let p = PadSnapshot { buttons: std::array::from_fn(|n| n == 7), ..Default::default() };
        assert!(p.button(Button::RightTrigger) && !p.button(Button::South));
    }
}
