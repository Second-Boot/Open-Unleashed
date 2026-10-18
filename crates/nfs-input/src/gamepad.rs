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
    /// `XAxis.Min` / `XAxis.Max`, the accessors' own 0…0x80
    x_band: (i32, i32),
    rumbler: crate::ff::Rumbler,
    rumble_test: Option<RumbleTest>,
    /// the pads' records of the frame, reused (`poll` runs every frame)
    records_buf: Vec<crate::bindings::PadRecord>,
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
        // `XAxis.Min` / `.Max` default to 0 and 0x80 (0x51E546, 0x51E550) — no dead zone, and
        // full lock at the stick's end
        Self { gilrs, axes: PadAxes::default(), x_band: (0, 0x80), rumbler: Default::default(), rumble_test: None, records_buf: Vec::new() }
    }

    /// The steering band, `XAxis.Min` and `XAxis.Max` of the accessors' own 0…0x80. The original
    /// bakes the same numbers into its binding descriptor as `0x80 ± min`, `0x80 ± max` (see the
    /// module comment).
    pub fn set_axis_band(&mut self, x_axis_min: i32, x_axis_max: i32) {
        self.x_band = (x_axis_min.clamp(0, 0x80), x_axis_max.clamp(0, 0x80));
    }

    /// Whether a pad that can rumble is connected (on macOS through GameController, `crate::ff_macos`).
    pub fn rumble_supported(&self) -> bool {
        self.gilrs.as_ref().is_some_and(|g| self.rumbler.supported(g))
    }

    /// A motor's sustained level, 0…1; 0 stops it.
    pub fn rumble_sustain(&mut self, motor: crate::ff::Motor, level: f32) {
        if let Some(g) = self.gilrs.as_mut() {
            self.rumbler.sustain(g, motor, level);
        }
    }

    /// A one-shot strong pulse.
    pub fn rumble_pulse(&mut self, level: f32, ms: u32) {
        if let Some(g) = self.gilrs.as_mut() {
            self.rumbler.pulse(g, level, ms);
        }
    }

    /// Stop the sustained motors.
    pub fn rumble_stop_all(&mut self) {
        self.rumbler.stop_all();
    }

    /// One line for the log: the rumble backend in use and the pad it drives.
    pub fn rumble_describe(&self) -> String {
        match self.gilrs.as_ref() {
            Some(g) => self.rumbler.describe(g),
            None => "no gamepad support".to_string(),
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

    /// Drain gamepad events into the logical input state. Call once per frame.
    pub fn poll(&mut self, input: &mut InputState) {
        let Some(g) = self.gilrs.as_mut() else { return };
        let mut touched = false;
        while let Some(ev) = g.next_event() {
            touched = true;
            match ev.event {
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    // the packet byte, doubled on the way into the car (0x4992CF) and handed on in
                    // the −1…1 the keyboard's steering uses, where 126 is full lock
                    self.axes.steer = (stick_steer(v, self.x_band.0, self.x_band.1) * 2) as f32 / 126.0;
                }
                EventType::ButtonChanged(Button::RightTrigger2, v, _) => self.axes.throttle = v,
                EventType::ButtonChanged(Button::LeftTrigger2, v, _) => self.axes.brake = v,
                // an action bound to a pad control is pressed by that control and no other, as a
                // key-bound one is by its key (`InputState::key_event`) — so `DEFAULTCONTROLS`
                // putting the handbrake on button 0 does not leave `South` also accelerating
                EventType::ButtonPressed(b, _) => {
                    if let Some(a) = map_button(b).filter(|&a| !input.pad_bound(a)) {
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
    /// Every input device the layer enumerates, with the flag that says whether it is a **pad**.
    ///
    /// The original builds `Controller.AllControllers` and `Controller.AllPads` at 0x511809 from
    /// exactly this: `0x51F050()` — `0x532E10(0, 1)`, the count at `[0x69CB0C]` — then for each
    /// index `0x51F0C0(i)` to skip a device, `0x51F060(i)` for its name (the descriptor
    /// `0x532E10(i, 2)` returns, name at **+0x14**) and `0x51F080(i)` to decide whether the name
    /// also goes on the pads' list. `RE/notes/controls.md` §4.103.
    ///
    /// That enumeration is DirectInput's, and it carries the keyboard too; `gilrs` enumerates
    /// only game controllers, so here every device is a pad and the two lists come out the same.
    pub fn devices(&self) -> Vec<(String, bool)> {
        let Some(g) = self.gilrs.as_ref() else { return Vec::new() };
        g.gamepads().map(|(_, gp)| (gp.name().to_string(), true)).collect()
    }

    /// The connected pads' axis sets, in connection order — what `NFS5.DEADZONE` draws from.
    /// Every pad's reading as the original's device record (`bindings::PadRecord`): the axes as
    /// [`axis_sets`](Self::axis_sets) orders them, the d-pad as the first hat and the buttons in
    /// [`BUTTON_ORDER`]. DirectInput numbers a pad's buttons in the device's own HID order, which
    /// `gilrs` does not expose; this port's numbering is `BUTTON_ORDER`, so a button binding's
    /// number is this port's own — PORT-DEVIATION D25. The capture and the reader use the same order,
    /// so what is bound is what is read.
    pub fn records(&self) -> Vec<crate::bindings::PadRecord> {
        let mut out = Vec::new();
        self.records_into(&mut out);
        out
    }

    /// [`records`](Self::records) into `out`, which is cleared first (the per-frame path reuses one
    /// buffer).
    pub fn records_into(&self, out: &mut Vec<crate::bindings::PadRecord>) {
        out.clear();
        let Some(g) = self.gilrs.as_ref() else { return };
        out.extend(g.gamepads().map(|(_, gp)| {
            let set = axis_set_of(&gp);
            let mut r = crate::bindings::PadRecord { mask: set.mask, axes: set.values, ..Default::default() };
            for (n, b) in BUTTON_ORDER.iter().enumerate() {
                r.buttons[n] = gp.is_pressed(*b);
                // `[info+0xC]`, the buttons 0x51F140 asks about: the ones the pad's mapping
                // has, in this port's numbering
                if gp.button_code(*b).is_some() {
                    r.button_mask |= 1 << n;
                }
            }
            r.pov[0] = hat_of(
                gp.is_pressed(Button::DPadUp),
                gp.is_pressed(Button::DPadRight),
                gp.is_pressed(Button::DPadDown),
                gp.is_pressed(Button::DPadLeft),
            );
            r
        }));
    }

    /// The connected pads' axis sets, in connection order — what `NFS5.DEADZONE` draws from.
    pub fn axis_sets(&self) -> Vec<PadAxisSet> {
        let Some(g) = self.gilrs.as_ref() else { return Vec::new() };
        g.gamepads().map(|(_, gp)| axis_set_of(&gp)).collect()
    }
}

/// One pad's axes as `DIJOYSTATE`'s eight, against the HID usages `gilrs` names them by.
fn axis_set_of(gp: &gilrs::Gamepad<'_>) -> PadAxisSet {
    const ORDER: [Axis; 6] = [Axis::LeftStickX, Axis::LeftStickY, Axis::LeftZ, Axis::RightStickX, Axis::RightStickY, Axis::RightZ];
    let mut set = PadAxisSet::default();
    for (n, a) in ORDER.iter().enumerate() {
        if let Some(d) = gp.axis_data(*a) {
            set.mask |= 1 << n;
            // `[rec+4+4n]` is the **device's** own reading, not the accessor's: the
            // image sets no `DIPROP_RANGE` (0x56FD70's only `SetProperty` is
            // `DIPROP_BUFFERSIZE`), so an absolute axis keeps DirectInput's default
            // 0…65535 with the centre at 32767, which is the range `NFS5.DEADZONE`'s
            // marker divides by 65536 (0x51B819). `gilrs` reads -1…1.
            //
            // A stick's Y runs **down**, as DirectInput's `lY` / `lRy` do: `gilrs`
            // turns the platform's downward Y round to point up (its
            // `IS_Y_AXIS_REVERSED`), and this turns it back. The image depends on
            // it — `DEFAULTCONTROLS` puts `Accel` on Y **−1** and `Brake` on Y **+1**
            // (0x51D60A…0x51D6EB), which is stick forward to accelerate only with
            // a Y that grows downward.
            let v = if n == 1 || n == 4 { -d.value() } else { d.value() };
            set.values[n] = (((v + 1.0) * 0.5 * 65535.0) as i32).clamp(0, 0xFFFF);
        }
    }
    set
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
}
