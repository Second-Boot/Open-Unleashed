//! The pad's state as the original's device record (PORT-DEVIATION D44 / D25): the snapshots and the record builders.

use super::*;

use super::axis::{di_value, trigger_value};

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

/// A **standard pad** as the original's device record (PORT-DEVIATION D44, PAD-MODERN: a
/// modern-pad layout). The axes are DirectInput's six: the sticks on `lX lY` and `lRx lRy`
/// (Y growing downward, as DirectInput's), **LT on `lZ`** and **RT on `lRz`** ([`trigger_value`]),
/// mask 0x3F — two separate analogue triggers, both pressable at once, where DirectInput's own
/// Xbox driver folds both into one `lZ`. *(Until PAD-MODERN the triggers were buttons 8 / 9 past
/// three quarters of their travel, an approximation; a profile that bound those buttons must be
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
/// §4.17; **APPROXIMATION** (device behaviour: the HID report of a foreign device is not readable from any binary of ours; PORT-DEVIATION D25): the index equivalence is by the HID usage order, not read from the
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

/// DirectInput's hat for the d-pad buttons (up, right, down, left): hundredths of a degree
/// clockwise from north, −1 centred (and for the combinations no hat reports). Indexed by the
/// four buttons as bits `up | right << 1 | down << 2 | left << 3`.
const HAT: [i32; 16] = [-1, 0, 9000, 4500, 18000, -1, 13500, -1, 27000, 31500, -1, -1, 22500, -1, -1, -1];

pub(super) fn hat_of(up: bool, right: bool, down: bool, left: bool) -> i32 {
    HAT[up as usize | (right as usize) << 1 | (down as usize) << 2 | (left as usize) << 3]
}
