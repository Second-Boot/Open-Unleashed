//! Joystick input via SDL2 (`sdl2` crate, joystick / game-controller / haptic subsystems only; no
//! window, events polled from the game thread). Mapped onto the same logical actions as the keyboard.
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
//! The triggers and buttons below are still this port's own fixed map, used for an action with no
//! pad binding. *(Corrected 2026-09-25: this said the original's pad bindings were `fe.txt`'s
//! commented examples only. `DEFAULTCONTROLS`' 0x51D1D0 binds a pad's X and Y axes and six of its
//! buttons, and the front end's factory runs it on the first pad at start-up —
//! `RE/notes/controls.md` §4.103a, `bindings::default_bindings`.)*

use crate::bindings::PadRecord;
use crate::{Action, InputState};
use sdl2::controller::{Axis as GcAxis, Button as GcButton, GameController};
use sdl2::event::Event;
use sdl2::joystick::{HatState, Joystick};
use sdl2::{EventPump, GameControllerSubsystem, JoystickSubsystem, Sdl};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PadAxes {
    pub steer: f32,
    pub throttle: f32,
    pub brake: f32,
}

/// The port's own names for a standard pad's buttons (the layout `gilrs` used to call
/// `South` … `RightThumb`; SDL's `A B Y X ...` map onto them in [`pad_button`]).
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
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}

/// The port's own `Gamepads` state (the SDL context lives in [`Devices`]).
pub struct Gamepads {
    devices: Option<Devices>,
    axes: PadAxes,
    /// `XAxis.Min` / `XAxis.Max`, the accessors' own 0…0x80
    x_band: (i32, i32),
    rumbler: crate::ff::Rumbler,
    rumble_test: Option<RumbleTest>,
    /// the pads' records of the frame, reused (`poll` runs every frame)
    records_buf: Vec<PadRecord>,
}

/// One opened SDL joystick.
struct Device {
    joy: Joystick,
    /// the Game Controller view of the same device, opened only for a **standard pad** (see
    /// [`Devices::open`]); `None` for wheels, sticks, pedals and anything SDL has no gamepad
    /// mapping for, which are read raw
    pad: Option<GameController>,
    name: String,
}

/// The SDL context with only the joystick / game-controller subsystems (no video, so no window
/// and no focus change) and the devices it has opened, in connection order. Everything runs on the
/// thread that created it (the game's main thread).
pub struct Devices {
    _sdl: Sdl,
    joystick: JoystickSubsystem,
    controller: GameControllerSubsystem,
    pump: EventPump,
    list: Vec<Device>,
}

impl Devices {
    fn new() -> Result<Self, String> {
        // a game in the background still reads its pad; no signal handlers of SDL's own
        sdl2::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
        sdl2::hint::set("SDL_NO_SIGNAL_HANDLERS", "1");
        // macOS: leave the Xbox / PlayStation pads to the system's GameController framework (the
        // rumble backend `crate::ff_macos` drives them through it); SDL's own HID driver would
        // open the device itself. Wheels and pedals come through the IOKit joystick driver.
        #[cfg(target_os = "macos")]
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI", "0");
        let sdl = sdl2::init()?;
        let joystick = sdl.joystick()?;
        let controller = sdl.game_controller()?;
        let pump = sdl.event_pump()?;
        let mut d = Self { _sdl: sdl, joystick, controller, pump, list: Vec::new() };
        let n = d.joystick.num_joysticks().unwrap_or(0);
        for i in 0..n {
            d.open(i);
        }
        Ok(d)
    }

    /// Open the joystick at SDL index `index`. A device is read through the Game Controller
    /// mapping only when SDL classes it as a **gamepad** (`SDL_JOYSTICK_TYPE_GAMECONTROLLER`) and
    /// has a mapping; a wheel, flight stick, throttle or pedal set — even one the controller
    /// database knows — is read raw, so every axis and slider stays visible
    /// (`RE/notes/controls.md`, PA-SDL).
    fn open(&mut self, index: u32) {
        let Ok(joy) = self.joystick.open(index) else { return };
        if self.list.iter().any(|d| d.joy.instance_id() == joy.instance_id()) {
            return;
        }
        let ty = unsafe { sdl2::sys::SDL_JoystickGetDeviceType(index as i32) };
        let standard = ty == sdl2::sys::SDL_JoystickType::SDL_JOYSTICK_TYPE_GAMECONTROLLER;
        let pad = if standard && self.controller.is_game_controller(index) { self.controller.open(index).ok() } else { None };
        let name = joy.name();
        tracing::info!(name = name.as_str(), axes = joy.num_axes(), hats = joy.num_hats(), buttons = joy.num_buttons(), mapped_pad = pad.is_some(), "joystick connected");
        self.list.push(Device { joy, pad, name });
    }

    /// Whether any open device can rumble through SDL.
    #[cfg(not(target_os = "macos"))]
    pub(crate) fn rumble_device_name(&self) -> Option<&str> {
        self.list.iter().find(|d| d.can_rumble()).map(|d| d.name.as_str())
    }

    #[cfg(not(target_os = "macos"))]
    /// `SDL_JoystickRumble` on the first device that can: `low` / `high` are the strong / weak
    /// motors, 0…65535, for `ms`.
    pub(crate) fn rumble(&mut self, low: u16, high: u16, ms: u32) -> bool {
        match self.list.iter_mut().find(|d| d.can_rumble()) {
            Some(d) => d.joy.set_rumble(low, high, ms).is_ok(),
            None => false,
        }
    }
}

impl Device {
    #[cfg(not(target_os = "macos"))]
    fn can_rumble(&self) -> bool {
        self.joy.has_rumble()
    }
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
        // `XAxis.Min` / `.Max` default to 0 and 0x80 (0x51E546, 0x51E550) — no dead zone, and
        // full lock at the stick's end
        Self { devices, axes: PadAxes::default(), x_band: (0, 0x80), rumbler: Default::default(), rumble_test: None, records_buf: Vec::new() }
    }

    /// The steering band, `XAxis.Min` and `XAxis.Max` of the accessors' own 0…0x80. The original
    /// bakes the same numbers into its binding descriptor as `0x80 ± min`, `0x80 ± max` (see the
    /// module comment).
    pub fn set_axis_band(&mut self, x_axis_min: i32, x_axis_max: i32) {
        self.x_band = (x_axis_min.clamp(0, 0x80), x_axis_max.clamp(0, 0x80));
    }

    /// Whether a pad that can rumble is connected (on macOS through GameController, `crate::ff_macos`).
    pub fn rumble_supported(&self) -> bool {
        self.devices.as_ref().is_some_and(|d| self.rumbler.supported(d))
    }

    /// A motor's sustained level, 0…1; 0 stops it.
    pub fn rumble_sustain(&mut self, motor: crate::ff::Motor, level: f32) {
        if let Some(d) = self.devices.as_mut() {
            self.rumbler.sustain(d, motor, level);
        }
    }

    /// A one-shot strong pulse.
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

    /// Drain SDL's events into the logical input state. Call once per frame (on the game thread:
    /// the pump belongs to the thread that initialised SDL).
    pub fn poll(&mut self, input: &mut InputState) {
        let Some(d) = self.devices.as_mut() else { return };
        let mut touched = false;
        // collected first: opening a device while `poll_iter` borrows the pump would alias it
        let mut events = Vec::new();
        events.extend(d.pump.poll_iter());
        for ev in events {
            match ev {
                Event::JoyDeviceAdded { which, .. } => d.open(which),
                Event::JoyDeviceRemoved { which, .. } => {
                    if let Some(i) = d.list.iter().position(|x| x.joy.instance_id() == which) {
                        tracing::info!(name = d.list[i].name.as_str(), "joystick disconnected");
                        d.list.remove(i);
                    }
                }
                // the legacy pad actions apply to a **standard pad** only (the old `gilrs` path); a
                // wheel reaches the game through its bindings alone
                Event::ControllerAxisMotion { which, axis, value, .. } if d.list.iter().any(|x| x.pad.as_ref().is_some_and(|p| p.instance_id() == which)) => {
                    touched = true;
                    match axis {
                        GcAxis::LeftX => {
                            // the packet byte, doubled on the way into the car (0x4992CF) and handed on in
                            // the −1…1 the keyboard's steering uses, where 126 is full lock
                            self.axes.steer = (stick_steer(unit(value), self.x_band.0, self.x_band.1) * 2) as f32 / 126.0;
                        }
                        GcAxis::TriggerRight => self.axes.throttle = trigger(value),
                        GcAxis::TriggerLeft => self.axes.brake = trigger(value),
                        _ => {}
                    }
                }
                // an action bound to a pad control is pressed by that control and no other, as a
                // key-bound one is by its key (`InputState::key_event`) — so `DEFAULTCONTROLS`
                // putting the handbrake on button 0 does not leave `South` also accelerating
                Event::ControllerButtonDown { which, button, .. } if d.list.iter().any(|x| x.pad.as_ref().is_some_and(|p| p.instance_id() == which)) => {
                    touched = true;
                    if let Some(a) = pad_button(button).and_then(map_button).filter(|&a| !input.pad_bound(a)) {
                        input.press(a);
                    }
                }
                Event::ControllerButtonUp { which, button, .. } if d.list.iter().any(|x| x.pad.as_ref().is_some_and(|p| p.instance_id() == which)) => {
                    touched = true;
                    if let Some(a) = pad_button(button).and_then(map_button) {
                        input.release(a);
                    }
                }
                _ => {}
            }
        }
        if touched || input.pad.is_some() {
            input.pad = Some(self.axes);
        }
        let mut records = std::mem::take(&mut self.records_buf);
        self.records_into(&mut records);
        input.set_pads(&records);
        self.records_buf = records;
    }
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

/// An SDL trigger (0…32767), 0…1.
fn trigger(v: i16) -> f32 {
    (f32::from(v.max(0)) / 32767.0).clamp(0.0, 1.0)
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

/// SDL's gamepad button in the port's names (A = South, B = East, X = West, Y = North, as the
/// layout `gilrs` named them).
fn pad_button(b: GcButton) -> Option<Button> {
    Some(match b {
        GcButton::A => Button::South,
        GcButton::B => Button::East,
        GcButton::X => Button::West,
        GcButton::Y => Button::North,
        GcButton::LeftShoulder => Button::LeftTrigger,
        GcButton::RightShoulder => Button::RightTrigger,
        GcButton::Back => Button::Select,
        GcButton::Start => Button::Start,
        GcButton::Guide => Button::Mode,
        GcButton::LeftStick => Button::LeftThumb,
        GcButton::RightStick => Button::RightThumb,
        GcButton::DPadUp => Button::DPadUp,
        GcButton::DPadDown => Button::DPadDown,
        GcButton::DPadLeft => Button::DPadLeft,
        GcButton::DPadRight => Button::DPadRight,
        _ => return None,
    })
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

/// What a **standard pad** (an SDL game controller with a mapping) reports, in SDL's units.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PadSnapshot {
    /// left stick X / Y, right stick X / Y (SDL: Y grows downward, as DirectInput's)
    pub sticks: [i16; 4],
    /// left / right trigger, 0…32767
    pub triggers: [i16; 2],
    /// d-pad up / right / down / left
    pub dpad: [bool; 4],
    /// the 15 buttons in [`BUTTON_ORDER`] (the triggers' entries are filled by [`pad_record`])
    pub buttons: [bool; 15],
}

/// A raw SDL joystick's state: `SDL_JoystickGetAxis` per axis, `SDL_JoystickGetHat` per hat (SDL's
/// bits `up 1 | right 2 | down 4 | left 8`), `SDL_JoystickGetButton` per button.
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

/// A trigger pressed past this is a pressed `LeftTrigger2` / `RightTrigger2`. **APPROXIMATION**:
/// the threshold the old `gilrs` path used is not documented; three quarters of the travel.
const TRIGGER_PRESSED: i16 = 24576;

/// A **standard pad** as the original's device record. The axes are the sticks on DirectInput's
/// `lX lY` and `lRx lRy` (bits 0, 1, 3, 4; mask 0x1B) — the triggers are buttons here, as this
/// port has always read them, not `lZ`. Buttons are the port's numbering ([`BUTTON_ORDER`]; PORT-DEVIATION
/// D25) and `button_mask` is every one the standard mapping has (all but `C` and `Z`).
pub fn pad_record(p: &PadSnapshot) -> PadRecord {
    let mut r = PadRecord { mask: 0x1B, ..Default::default() };
    for (slot, v) in [0usize, 1, 3, 4].into_iter().zip(p.sticks) {
        r.axes[slot] = di_value(v);
    }
    for (n, b) in BUTTON_ORDER.iter().enumerate() {
        r.buttons[n] = match b {
            Button::LeftTrigger2 => p.triggers[0] > TRIGGER_PRESSED,
            Button::RightTrigger2 => p.triggers[1] > TRIGGER_PRESSED,
            _ => p.buttons[n],
        };
        if !matches!(b, Button::C | Button::Z) {
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

impl Device {
    fn record(&self) -> PadRecord {
        if let Some(c) = &self.pad {
            let mut p = PadSnapshot {
                sticks: [c.axis(GcAxis::LeftX), c.axis(GcAxis::LeftY), c.axis(GcAxis::RightX), c.axis(GcAxis::RightY)],
                triggers: [c.axis(GcAxis::TriggerLeft), c.axis(GcAxis::TriggerRight)],
                dpad: [c.button(GcButton::DPadUp), c.button(GcButton::DPadRight), c.button(GcButton::DPadDown), c.button(GcButton::DPadLeft)],
                buttons: [false; 15],
            };
            for (n, b) in BUTTON_ORDER.iter().enumerate() {
                let sdl = match b {
                    Button::South => GcButton::A,
                    Button::East => GcButton::B,
                    Button::North => GcButton::Y,
                    Button::West => GcButton::X,
                    Button::LeftTrigger => GcButton::LeftShoulder,
                    Button::RightTrigger => GcButton::RightShoulder,
                    Button::Select => GcButton::Back,
                    Button::Start => GcButton::Start,
                    Button::Mode => GcButton::Guide,
                    Button::LeftThumb => GcButton::LeftStick,
                    Button::RightThumb => GcButton::RightStick,
                    _ => continue,
                };
                p.buttons[n] = c.button(sdl);
            }
            return pad_record(&p);
        }
        let j = &self.joy;
        let snap = RawSnapshot {
            axes: (0..j.num_axes()).map(|i| j.axis(i).unwrap_or(0)).collect(),
            hats: (0..j.num_hats()).map(|i| j.hat(i).map(hat_bits).unwrap_or(0)).collect(),
            buttons: (0..j.num_buttons()).map(|i| j.button(i).unwrap_or(false)).collect(),
        };
        raw_record(&snap)
    }
}

/// SDL's hat bits (`up 1 | right 2 | down 4 | left 8`) of a hat position.
fn hat_bits(h: HatState) -> u8 {
    h.to_raw()
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

    /// A standard pad reads as it did through `gilrs`: the sticks on bits 0, 1, 3, 4, Y growing
    /// downward, every button but `C` / `Z` in the mask, triggers as buttons 8 / 9, the d-pad as the
    /// first hat.
    #[test]
    fn a_standard_pad_reads_as_before() {
        let mut p = PadSnapshot::default();
        let rest = pad_record(&p);
        assert_eq!(rest.mask, 0x1B);
        assert_eq!(rest.button_mask, 0x7FFF & !0x30);
        assert_eq!(rest.pov[0], -1);
        p.sticks = [32767, -32768, 0, 32767];
        p.triggers = [32767, 0];
        p.dpad = [true, true, false, false];
        p.buttons[0] = true;
        let r = pad_record(&p);
        assert_eq!((r.axes[0], r.axes[1], r.axes[4]), (65535, 0, 65535));
        assert!(r.buttons[0] && r.buttons[8] && !r.buttons[9]);
        assert_eq!(r.pov[0], 4500);
    }
}
