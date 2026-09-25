//! The original's control bindings — what `fe.txt`'s `C1_*` lines say and what the controller-setup
//! screen rebinds (`RE/notes/controls.md` §4.117).
//!
//! A binding is the six dwords at `[binding+0x14]`: **kind, device, index, direction, pov, key**.
//! The race side keeps one packed descriptor per option id in `[0x5E9130 + id·4]` (0x4B4BE0) and
//! reads it through the kind's reader in the records at `0x5CC5C8 + 12·kind`, each answering
//! 0…255. This module has the keyboard half: kind 3, `Key`, whose reader **0x411C20** looks the
//! **scancode** up in the key-down table `0x5DE028`.
//!
//! `fe.txt` writes a key binding as `key,SC_<NAME>`; the names are the 82-entry table at
//! **0x5D6588** (`{char* name, u32 scancode}`), which is DirectInput's own scancode set.

/// The `SC_*` table at 0x5D6588, in its own order.
pub const SC_NAMES: [(&str, u8); 82] = [
    ("SC_ESC", 0x01), ("SC_1", 0x02), ("SC_2", 0x03), ("SC_3", 0x04), ("SC_4", 0x05), ("SC_5", 0x06),
    ("SC_6", 0x07), ("SC_7", 0x08), ("SC_8", 0x09), ("SC_9", 0x0A), ("SC_0", 0x0B), ("SC_MINUS", 0x0C),
    ("SC_EQUAL", 0x0D), ("SC_BS", 0x0E), ("SC_TAB", 0x0F), ("SC_Q", 0x10), ("SC_W", 0x11), ("SC_E", 0x12),
    ("SC_R", 0x13), ("SC_T", 0x14), ("SC_Y", 0x15), ("SC_U", 0x16), ("SC_I", 0x17), ("SC_O", 0x18),
    ("SC_P", 0x19), ("SC_LBRACKET", 0x1A), ("SC_RBRACKET", 0x1B), ("SC_ENTER", 0x1C), ("SC_CTRL", 0x1D),
    ("SC_A", 0x1E), ("SC_S", 0x1F), ("SC_D", 0x20), ("SC_F", 0x21), ("SC_G", 0x22), ("SC_H", 0x23),
    ("SC_J", 0x24), ("SC_K", 0x25), ("SC_L", 0x26), ("SC_COLON", 0x27), ("SC_QUOTE", 0x28),
    ("SC_BQUOTE", 0x29), ("SC_LSHIFT", 0x2A), ("SC_Z", 0x2C), ("SC_X", 0x2D), ("SC_C", 0x2E),
    ("SC_V", 0x2F), ("SC_B", 0x30), ("SC_N", 0x31), ("SC_M", 0x32), ("SC_COMMA", 0x33),
    ("SC_PERIOD", 0x34), ("SC_QUESTION", 0x35), ("SC_RSHIFT", 0x36), ("SC_PRTSRC", 0x37), ("SC_ALT", 0x38),
    ("SC_SPACE", 0x39), ("SC_CAPLOCK", 0x3A), ("SC_F1", 0x3B), ("SC_F2", 0x3C), ("SC_F3", 0x3D),
    ("SC_F4", 0x3E), ("SC_F5", 0x3F), ("SC_F6", 0x40), ("SC_F7", 0x41), ("SC_F8", 0x42), ("SC_F9", 0x43),
    ("SC_F10", 0x44), ("SC_NUMLOCK", 0x45), ("SC_SCROLLLOCK", 0x46), ("SC_HOME", 0x47), ("SC_UP", 0x48),
    ("SC_PGUP", 0x49), ("SC_NPMINUS", 0x4A), ("SC_CENTER", 0x4C), ("SC_PLUS", 0x4E), ("SC_END", 0x4F),
    ("SC_DOWN", 0x50), ("SC_PGDN", 0x51), ("SC_INS", 0x52), ("SC_DEL", 0x53), ("SC_LEFT", 0x4B),
    ("SC_RIGHT", 0x4D),
];

/// The option ids of the player-one bindings, from the name table the registry init fills at
/// 0x4B3080… (`{id, ptr, name}` records from 0x5D1E40).
pub mod id {
    pub const C1_LEFT: u16 = 0x150;
    pub const C1_RIGHT: u16 = 0x151;
    pub const C1_GAS: u16 = 0x152;
    pub const C1_BRAKE: u16 = 0x153;
    pub const C1_HANDBRAKE: u16 = 0x154;
    pub const C1_HORN: u16 = 0x155;
    pub const C1_SHIFTUP: u16 = 0x169;
    pub const C1_SHIFTDOWN: u16 = 0x16A;
    pub const C1_ABORT: u16 = 0x16B;
    pub const C1_RESET: u16 = 0x16C;
    pub const C1_AIDRIVE: u16 = 0x16D;
    /// `I_LOOKBEHIND` and `I_CYCLECAMERA` (the option table's entries at 0x4B41F7 / 0x4B425B) —
    /// the ids `Controller.LookBehind` and `Controller.CameraView` are built with (0x51E1A9,
    /// 0x51E22D)
    pub const I_LOOKBEHIND: u16 = 0x201;
    pub const I_CYCLECAMERA: u16 = 0x205;
}

/// One binding's six fields, `[binding+0x14…+0x28]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Binding {
    /// 0 none, 1 axis, 2 button, 3 key, 4 none, 5 hat (0x51D010 / 0x51CB7C)
    pub kind: u32,
    pub device: u32,
    /// the axis, button or hat — for a key, the **scancode** (0x51D0A0)
    pub index: u32,
    /// an axis binding's sign, +1 or −1
    pub dir: i32,
    /// a hat binding's direction, 0…7
    pub pov: u32,
    /// a key binding's display code, the one 0x51C050 names
    pub key: u32,
}

impl Binding {
    /// The six dwords `FEControllerInputAccessor` saves from `this+0x14` (0x51D060) and loads
    /// back (0x51D080), in their order.
    pub fn to_fields(&self) -> [u32; 6] {
        [self.kind, self.device, self.index, self.dir as u32, self.pov, self.key]
    }

    pub fn from_fields(f: [u32; 6]) -> Self {
        Self { kind: f[0], device: f[1], index: f[2], dir: f[3] as i32, pov: f[4], key: f[5] }
    }
}

pub fn scancode_by_name(name: &str) -> Option<u8> {
    SC_NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|&(_, s)| s)
}

/// The code 0x51C050 prints for a scancode: a key that types a character is that character, and
/// every other key is the scancode in the high byte, which is how its own extended codes read —
/// `0x4800` up, `0x4B00` left, `0x4D00` right, `0x5000` down, `0x4700` home and so on
/// (`nfs_game::keyname`). The characters are the named ones of the `SC_*` table.
///
/// A letter is the **lower-case** character: that is what the image itself pairs with a letter's
/// scancode when it builds a key binding — `DEFAULTCONTROLS`' 0x51D1D0 calls 0x51D0A0 with
/// `'h'` for 0x23, `'a'` for 0x1E, `'z'` for 0x2C and so on (0x51DAC6…0x51DC0C), never the
/// capital — and 0x51C050 takes both cases and prints the capital (0x51C1EE). *(Corrected
/// 2026-09-25: this used to give the capital, so a letter captured on the setup screen never
/// equalled — under 0x51DC20, which compares this code — the same letter as the defaults bind it,
/// and was not taken from the row that had it.)*
pub fn display_code(scancode: u8) -> u32 {
    let name = SC_NAMES.iter().find(|&&(_, s)| s == scancode).map(|&(n, _)| &n[3..]).unwrap_or("");
    match name {
        "SPACE" => 0x20,
        "COMMA" => b',' as u32,
        "PERIOD" => b'.' as u32,
        "QUESTION" => b'/' as u32,
        "COLON" => b';' as u32,
        "EQUAL" => b'=' as u32,
        "LBRACKET" => b'[' as u32,
        "RBRACKET" => b']' as u32,
        "BQUOTE" => b'`' as u32,
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_alphanumeric() => n.as_bytes()[0].to_ascii_lowercase() as u32,
        _ => (scancode as u32) << 8,
    }
}

/// A binding as `fe.txt` writes it. Only the keyboard form, `key,SC_<NAME>`, is read here; the
/// pad forms (`Pad, dev, button`, `Analog, dev, axis, lo, hi`) appear in the shipped file only as
/// comments.
pub fn parse(value: &str) -> Option<Binding> {
    let mut parts = value.split(',').map(str::trim);
    let kind = parts.next()?;
    if !kind.eq_ignore_ascii_case("key") {
        return None;
    }
    let sc = scancode_by_name(parts.next()?)?;
    Some(Binding { kind: 3, device: 0, index: sc as u32, dir: 0, pov: 0, key: display_code(sc) })
}

/// The modifier state 0x411C20 folds into a key read: Shift (0x2A or 0x36, `[0x5DE052]` /
/// `[0x5DE05E]`), Ctrl (0x1D, `[0x5DE045]`), Alt (0x38, `[0x5DE060]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// `Key`'s reader **0x411C20**: 255 while the key is down, else 0 — and 0 whenever Shift, Ctrl or
/// Alt is held, since the reader XORs their bits into a binding that carries none and answers 0
/// on any bit left set (0x411C5F).
pub fn key_read(b: &Binding, down: impl Fn(u8) -> bool, m: Modifiers) -> u8 {
    if b.kind != 3 || m.shift || m.ctrl || m.alt {
        return 0;
    }
    if down((b.index & 0xFF) as u8) { 255 } else { 0 }
}

/// One pad's reading in the shape of the original's 0x104-byte device record (0x532E10(i, 6)):
/// eight axes at `+4 + 4n` (DirectInput's default 0…65535), four hats at `+0x24` (hundredths of
/// a degree, negative when centred), 32 buttons at `+0x34`; `mask` is the info block's axis
/// bitmask `[info+8]` and `button_mask` its button mask `[info+0xC]` (0x51F140).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadRecord {
    pub mask: u8,
    pub button_mask: u32,
    pub axes: [i32; 8],
    pub pov: [i32; 4],
    pub buttons: [bool; 32],
}

impl Default for PadRecord {
    fn default() -> Self {
        Self { mask: 0, button_mask: 0, axes: [0x7FFF; 8], pov: [-1; 4], buttons: [false; 32] }
    }
}

impl PadRecord {
    /// This record's device as 0x51D1D0 asks about it. Every pad this port enumerates is an
    /// enabled analogue device (`Gamepads::devices`), so 0x51F080 answers true for each.
    pub fn caps(&self) -> DeviceCaps {
        DeviceCaps { analogue: true, axes: self.mask, buttons: self.button_mask }
    }
}

/// **0x51F3B0** — what moved on the pads since the row took focus (`base` is the snapshot
/// 0x51F310 took then). Per device, in this order, each overwriting the last: a button that
/// changed (kind 2), a hat that changed (kind 5, index `0x20 + n`, pov `value / 4500`), an axis
/// past ±5000 of its snapshot (kind 1, direction −1 below `base + 5000`, else +1) — and for the
/// sliders 6 and 7 the direction is taken from **axis 5** (ZRot), as the operands at
/// 0x51F67A/0x51F67D are. The first device that moved wins.
pub fn capture(base: &[PadRecord], cur: &[PadRecord]) -> Option<Binding> {
    for (d, (b, c)) in base.iter().zip(cur).enumerate() {
        let mut out: Option<Binding> = None;
        for n in 0..32 {
            if b.buttons[n] != c.buttons[n] {
                out = Some(Binding { kind: 2, device: d as u32, index: n as u32, dir: 0, pov: 0, key: 0 });
            }
        }
        for n in 0..4 {
            if b.pov[n] != c.pov[n] {
                out = Some(Binding { kind: 5, device: d as u32, index: 0x20 + n as u32, dir: 0, pov: (c.pov[n] / 4500) as u32, key: 0 });
            }
        }
        let moved = |n: usize| c.axes[n] >= b.axes[n] + 5000 || c.axes[n] <= b.axes[n] - 5000;
        for n in 0..8 {
            if c.mask & (1 << n) == 0 || !moved(n) {
                continue;
            }
            let from = if n < 6 { n } else { 5 };
            let dir = if c.axes[from] < b.axes[from] + 5000 { -1 } else { 1 };
            out = Some(Binding { kind: 1, device: d as u32, index: n as u32, dir, pov: 0, key: 0 });
        }
        if out.is_some() {
            return out;
        }
    }
    None
}

/// `Button`'s reader **0x411A50**: 255 while the button is down.
pub fn button_read(b: &Binding, pads: &[PadRecord]) -> u8 {
    match pads.get(b.device as usize) {
        Some(p) if p.buttons.get((b.index & 31) as usize).copied().unwrap_or(false) => 255,
        _ => 0,
    }
}

/// `Pov`'s reader **0x411AC0**: 255 while the device's first hat points within a neighbour of
/// the bound direction — `[(d−1)·4500, (d+1)·4500]`, wrapped about north (0x411B0F…0x411B1D) —
/// 0 when centred.
pub fn pov_read(b: &Binding, pads: &[PadRecord]) -> u8 {
    let Some(p) = pads.get(b.device as usize) else { return 0 };
    let v = p.pov[0];
    if v < 0 {
        return 0;
    }
    let d = b.pov as i32;
    let mut hi = ((d + 1) & 7) * 4500;
    let mut lo = ((d - 1) & 7) * 4500;
    if lo > hi {
        if v <= hi {
            lo = -hi;
        } else {
            hi = lo + 9000;
        }
    }
    if v <= hi && v >= lo { 255 } else { 0 }
}

/// `Analog`'s reader **0x411B80** for an axis binding, with the band 0x51CEA1…0x51CEDE bakes from
/// the pad's `<Axis>.Min` / `.Max` (`bands[axis]`).
pub fn axis_read(b: &Binding, pads: &[PadRecord], bands: &[[i32; 2]; 8]) -> u8 {
    let Some(p) = pads.get(b.device as usize) else { return 0 };
    let n = (b.index & 7) as usize;
    let (lo, hi) = crate::gamepad::axis_band(bands[n][0], bands[n][1], b.dir);
    crate::gamepad::analog_read(p.axes[n], lo, hi) as u8
}

/// The player's key bindings by option id, from `fe.txt`'s `C1_*` lines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bindings {
    pub by_id: std::collections::HashMap<u16, Binding>,
}

impl Bindings {
    pub fn from_registry<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Self {
        let names = [
            ("C1_LEFT", id::C1_LEFT),
            ("C1_RIGHT", id::C1_RIGHT),
            ("C1_GAS", id::C1_GAS),
            ("C1_BRAKE", id::C1_BRAKE),
            ("C1_HANDBRAKE", id::C1_HANDBRAKE),
            ("C1_HORN", id::C1_HORN),
            ("C1_SHIFTUP", id::C1_SHIFTUP),
            ("C1_SHIFTDOWN", id::C1_SHIFTDOWN),
            ("C1_ABORT", id::C1_ABORT),
            ("C1_RESET", id::C1_RESET),
            ("C1_AIDRIVE", id::C1_AIDRIVE),
            ("I_LOOKBEHIND", id::I_LOOKBEHIND),
            ("I_CYCLECAMERA", id::I_CYCLECAMERA),
        ];
        let by_id = names.iter().filter_map(|&(n, i)| Some((i, parse(get(n)?)?))).collect();
        Self { by_id }
    }
    pub fn get(&self, id: u16) -> Option<&Binding> {
        self.by_id.get(&id)
    }

    /// A row's new binding, as both capture paths commit it (0x51C541…0x51C5DC for a pad,
    /// 0x51D0DA…0x51D1B8 for a key): nothing happens when it equals the row's own under
    /// [`same`]; otherwise **0x51DCC0** clears the kind of every binding equal to it — a
    /// duplicate is taken from the row that had it, which then shows nothing — and the new one
    /// is stored. 0x51DCC0 always answers false (0x51E015), so the "Game control already in
    /// use." box its callers would open (`FEStrings[832]`) never does.
    pub fn assign(&mut self, id: u16, new: Binding) {
        if self.by_id.get(&id).is_some_and(|b| same(b, &new)) {
            return;
        }
        for b in self.by_id.values_mut() {
            if same(b, &new) {
                b.kind = 0;
            }
        }
        self.by_id.insert(id, new);
    }
}

/// **0x51DC20**, two bindings the same: the same kind and device, and then for a key the same
/// display code and index, for a hat the same index, direction and pov, and otherwise the same
/// index and direction.
pub fn same(a: &Binding, b: &Binding) -> bool {
    if a.kind != b.kind || a.device != b.device {
        return false;
    }
    match a.kind {
        3 => a.key == b.key && a.index == b.index,
        5 => a.index == b.index && a.dir == b.dir && a.pov == b.pov,
        _ => a.index == b.index && a.dir == b.dir,
    }
}

/// The nineteen `Controller.*` accessors, in the order **0x51D1D0** gathers them (0x51D1E7…
/// 0x51D4AD, into `esp+0x3C + 4·row`) — the same order 0x51DCC0 walks (0x51DCD5…0x51DFA5).
pub const CONTROLLER_ROWS: [&str; 19] = [
    "Controller.Accel",
    "Controller.Brake",
    "Controller.HandBrake",
    "Controller.SteerLeft",
    "Controller.SteerRight",
    "Controller.Horn",
    "Controller.ShiftUp",
    "Controller.ShiftDown",
    "Controller.LookBehind",
    "Controller.Lights",
    "Controller.ResetCar",
    "Controller.CameraView",
    "Controller.Pause",
    "Controller.LeftTurnSignal",
    "Controller.RightTurnSignal",
    "Controller.Hazards",
    "Controller.LookLeft",
    "Controller.LookRight",
    "Controller.LookForward",
];

/// What 0x51D1D0 asks of the device it is handed, all out of `0x532E10(device, 2)`'s info block
/// and the class byte of the cached record `[[0x69C9EC] + 0x104·device]`:
///
/// ```text
/// 0x51F080(dev)     class == 4 && [info+4] & 1      an enabled analogue device
/// 0x51F100(dev)     [info+8] & 1                    it has an X axis
/// 0x51F120(dev)     [info+8] >> 1 & 1               it has a Y axis
/// 0x51F140(dev, n)  [info+0xC] & (1 << n) != 0      it has button n
/// ```
///
/// `[info+8]` is the axis mask `NFS5.DEADZONE` reads (`RE/notes/controls.md` §4.17); `[info+0xC]`
/// is read here only as a mask, bit `n` for button `n` — where the enumeration fills it is not
/// traced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceCaps {
    /// 0x51F080
    pub analogue: bool,
    /// `[info+8]`
    pub axes: u8,
    /// `[info+0xC]`
    pub buttons: u32,
}

/// The pad half of 0x51D1D0, in its order, as `(row, kind, index, dir)`. A kind-1 row is gated
/// on the device having axis `index`, a kind-2 row on its having button `index`; the row's
/// "already bound" byte each gate also tests (`esp+0x10 + row`, zeroed at 0x51D1EC…0x51D1FC) is
/// still clear when the pad half reaches it, so the capability is the whole test.
///
/// ```text
/// 0x51D4F3  X axis   SteerLeft  {1, dev, 0, -1}   SteerRight {1, dev, 0, +1}
/// 0x51D5F9  Y axis   Accel      {1, dev, 1, -1}   Brake      {1, dev, 1, +1}
/// 0x51D6FA  button 0 HandBrake  {2, dev, 0, 0}
/// 0x51D785  button 3 ResetCar   {2, dev, 3, 0}
/// 0x51D815  button 5 CameraView {2, dev, 5, 0}
/// 0x51D8A8  button 2 LookBehind {2, dev, 2, 0}
/// 0x51D934  button 7 ShiftUp    {2, dev, 7, 0}
/// 0x51D9C4  button 8 ShiftDown  {2, dev, 8, 0}
/// ```
const PAD_DEFAULTS: [(usize, u32, u32, i32); 10] = [
    (3, 1, 0, -1),
    (4, 1, 0, 1),
    (0, 1, 1, -1),
    (1, 1, 1, 1),
    (2, 2, 0, 0),
    (10, 2, 3, 0),
    (11, 2, 5, 0),
    (8, 2, 2, 0),
    (6, 2, 7, 0),
    (7, 2, 8, 0),
];

/// The keyboard half of 0x51D1D0 (0x51DA47…0x51DC0C): `(code, scancode)` for a row the pad half
/// left alone, which 0x51D0A0(code, scancode, 0) turns into `{3, 0, scancode, 0, 0, code}`.
///
/// Four rows' letters follow the language `[0x690710]` (0x4E0750 reads it; 0x4E0598 stores the
/// registry's `Language` there, folded into 0…6), so that the key sits where the English one does
/// on a French (1, AZERTY) or a German (2, QWERTZ) keyboard: ShiftUp, ShiftDown, LeftTurnSignal
/// and LookLeft. `LookForward`'s own test is of a constant 0 (`xor eax, eax` at 0x51DBFA), so it
/// always gets its key.
pub fn default_key(row: usize, lang: u32) -> (u32, u8) {
    let c = |ch: u8| ch as u32;
    match row {
        0 => (0x4800, 0x48),                                            // 0x51DA5D, up
        1 => (0x5000, 0x50),                                            // 0x51DA74, down
        2 => (0x20, 0x39),                                              // 0x51DA8B, space
        3 => (0x4B00, 0x4B),                                            // 0x51DA9F, left
        4 => (0x4D00, 0x4D),                                            // 0x51DAB6, right
        5 => (c(b'h'), 0x23),                                           // 0x51DACD
        6 => (c(if lang == 1 { b'q' } else { b'a' }), 0x1E),            // 0x51DADA
        7 => (c(match lang { 2 => b'y', 1 => b'w', _ => b'z' }), 0x2C), // 0x51DAF7
        8 => (c(b'b'), 0x30),                                           // 0x51DB27
        9 => (c(b'l'), 0x26),                                           // 0x51DB3B
        10 => (c(b'r'), 0x13),                                          // 0x51DB4F
        11 => (c(b'c'), 0x2E),                                          // 0x51DB63
        12 => (c(b'p'), 0x19),                                          // 0x51DB77
        13 => (c(if lang == 1 { b'z' } else { b'w' }), 0x11),           // 0x51DB84
        14 => (c(b'e'), 0x12),                                          // 0x51DBA8
        15 => (c(b't'), 0x14),                                          // 0x51DBBC
        16 => (c(if lang == 2 { b'z' } else { b'y' }), 0x15),           // 0x51DBC9
        17 => (c(b'i'), 0x17),                                          // 0x51DBF3
        _ => (c(b'u'), 0x16),                                           // 0x51DC0A
    }
}

/// **0x51D1D0**, `DEFAULTCONTROLS`' work: the binding each of the nineteen rows
/// ([`CONTROLLER_ROWS`]) ends up with for `device`. When the device is analogue (0x51F080,
/// tested at 0x51D4CF) the pad half binds what it has ([`PAD_DEFAULTS`]); every row it did not
/// bind gets its key ([`default_key`]), whatever the device.
pub fn default_bindings(device: u32, caps: DeviceCaps, lang: u32) -> [Binding; 19] {
    let mut out = [Binding::default(); 19];
    let mut done = [false; 19];
    if caps.analogue {
        for &(row, kind, index, dir) in &PAD_DEFAULTS {
            let has = match kind {
                1 => caps.axes & (1 << index) != 0,
                _ => caps.buttons & (1 << index) != 0,
            };
            if has {
                out[row] = Binding { kind, device, index, dir, pov: 0, key: 0 };
                done[row] = true;
            }
        }
    }
    for row in 0..19 {
        if !done[row] {
            let (key, sc) = default_key(row, lang);
            out[row] = Binding { kind: 3, device: 0, index: sc as u32, dir: 0, pov: 0, key };
        }
    }
    out
}

impl Bindings {
    /// Store what 0x51D1D0 built, each the way it commits one: **0x51DCC0** clears every binding
    /// equal to it, then the six dwords go in. The 0x51DC20 compare it makes first is not looked
    /// at in the pad half, and in 0x51D0A0 only when its third argument is set, which here it is
    /// not (0x51DA5A pushes `ebx` = 0) — so unlike [`assign`](Self::assign) there is no early
    /// out. The nineteen are pairwise different, so their order does not change the outcome.
    /// `id_of` names the option id behind a row where this port has one; the rest (lights,
    /// pause, the turn signals, the hazards and the three look rows) have no binding here.
    pub fn set_defaults(&mut self, defaults: &[Binding; 19], id_of: impl Fn(&str) -> Option<u16>) {
        for (row, b) in defaults.iter().enumerate() {
            let Some(id) = id_of(CONTROLLER_ROWS[row]) else { continue };
            for other in self.by_id.values_mut() {
                if same(other, b) {
                    other.kind = 0;
                }
            }
            self.by_id.insert(id, *b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fe_txt_keys_parse_to_scancodes() {
        let b = parse("key,SC_RIGHT").unwrap();
        assert_eq!((b.kind, b.index, b.key), (3, 0x4D, 0x4D00));
        assert_eq!(parse("key, SC_SPACE").unwrap().key, 0x20);
        assert_eq!(parse("key,SC_A").unwrap().key, b'a' as u32);
        assert_eq!(parse("key,SC_ADD"), None); // not in the table (fe.txt's zoom key)
        assert_eq!(parse("Pad, 0, 22"), None);
        let m = Modifiers::default();
        assert_eq!(key_read(&b, |s| s == 0x4D, m), 255);
        assert_eq!(key_read(&b, |s| s == 0x4D, Modifiers { shift: true, ..m }), 0);
    }

    /// 0x51F3B0: an axis beats a hat beats a button on one device, the ±5000 window, and a hat's
    /// neighbourhood in 0x411AC0 wrapping about north.
    #[test]
    fn pad_capture_and_reads() {
        let base = PadRecord { mask: 0x3F, ..Default::default() };
        let mut cur = base;
        cur.buttons[3] = true;
        assert_eq!(capture(&[base], &[cur]).map(|b| (b.kind, b.index)), Some((2, 3)));
        cur.pov[0] = 27000;
        assert_eq!(capture(&[base], &[cur]).map(|b| (b.kind, b.index, b.pov)), Some((5, 0x20, 6)));
        cur.axes[0] = 0x7FFF + 4999;
        assert_eq!(capture(&[base], &[cur]).map(|b| b.kind), Some(5));
        cur.axes[0] = 0x7FFF - 5000;
        assert_eq!(capture(&[base], &[cur]).map(|b| (b.kind, b.index, b.dir)), Some((1, 0, -1)));
        let north = Binding { kind: 5, pov: 0, ..Default::default() };
        let mut p = PadRecord::default();
        for (v, want) in [(0, 255), (4500, 255), (31500, 255), (9000, 0), (-1, 0)] {
            p.pov[0] = v;
            assert_eq!(pov_read(&north, &[p]), want, "hat at {v}");
        }
    }

    /// Rebinding a key another row holds takes it from that row (0x51DCC0), and binding a row to
    /// what it already has changes nothing.
    #[test]
    fn a_duplicate_is_taken_not_refused() {
        let mut bs = Bindings::from_registry(|k| match k {
            "C1_LEFT" => Some("key,SC_LEFT"),
            "C1_HORN" => Some("key,SC_H"),
            _ => None,
        });
        let h = parse("key,SC_H").unwrap();
        bs.assign(id::C1_LEFT, h);
        assert_eq!(bs.get(id::C1_LEFT), Some(&h));
        assert_eq!(bs.get(id::C1_HORN).unwrap().kind, 0);
        let before = bs.clone();
        bs.assign(id::C1_LEFT, h);
        assert_eq!(bs, before);
    }

    fn row(name: &str) -> usize {
        CONTROLLER_ROWS.iter().position(|r| *r == name).unwrap()
    }

    /// 0x51D1D0 on a device that is not analogue (the keyboard, or none): every row a key, and
    /// the English layout's are `fe.txt`'s own scancodes.
    #[test]
    fn keyboard_defaults() {
        let d = default_bindings(0, DeviceCaps::default(), 0);
        assert!(d.iter().all(|b| b.kind == 3 && b.device == 0));
        let key = |n: &str| (d[row(n)].index as u8, d[row(n)].key);
        assert_eq!(key("Controller.Accel"), (0x48, 0x4800));
        assert_eq!(key("Controller.SteerLeft"), (0x4B, 0x4B00));
        assert_eq!(key("Controller.HandBrake"), (0x39, 0x20));
        assert_eq!(key("Controller.ShiftUp"), (0x1E, b'a' as u32));
        assert_eq!(key("Controller.ShiftDown"), (0x2C, b'z' as u32));
        assert_eq!(key("Controller.LookForward"), (0x16, b'u' as u32));
        // the letters are the ones `display_code` gives a captured key, so they compare equal
        for b in &d {
            assert_eq!(b.key, display_code(b.index as u8), "scancode {:#x}", b.index);
        }
        // and the nineteen differ pairwise, so the commit order does not matter
        for (i, a) in d.iter().enumerate() {
            assert!(d[i + 1..].iter().all(|b| !same(a, b)));
        }
        // the same scancodes, French and German letters (0x51DADA, 0x51DAF7, 0x51DB84, 0x51DBC9)
        let fr = default_bindings(0, DeviceCaps::default(), 1);
        let de = default_bindings(0, DeviceCaps::default(), 2);
        let letters = |d: &[Binding; 19]| {
            ["Controller.ShiftUp", "Controller.ShiftDown", "Controller.LeftTurnSignal", "Controller.LookLeft"]
                .map(|n| d[row(n)].key as u8 as char)
        };
        assert_eq!(letters(&d), ['a', 'z', 'w', 'y']);
        assert_eq!(letters(&fr), ['q', 'w', 'z', 'y']);
        assert_eq!(letters(&de), ['a', 'y', 'w', 'z']);
        assert!(fr.iter().zip(&d).all(|(a, b)| a.index == b.index));
    }

    /// A pad with both stick axes and twelve buttons: steering on X, the pedals on Y, six rows on
    /// buttons 0, 2, 3, 5, 7 and 8, and the other nine rows still keys.
    #[test]
    fn full_pad_defaults() {
        let caps = DeviceCaps { analogue: true, axes: 0x3F, buttons: 0xFFF };
        let d = default_bindings(2, caps, 0);
        let b = |n: &str| d[row(n)];
        let axis = |n: &str| (b(n).kind, b(n).device, b(n).index, b(n).dir);
        assert_eq!(axis("Controller.SteerLeft"), (1, 2, 0, -1));
        assert_eq!(axis("Controller.SteerRight"), (1, 2, 0, 1));
        assert_eq!(axis("Controller.Accel"), (1, 2, 1, -1));
        assert_eq!(axis("Controller.Brake"), (1, 2, 1, 1));
        for (n, i) in [
            ("Controller.HandBrake", 0),
            ("Controller.LookBehind", 2),
            ("Controller.ResetCar", 3),
            ("Controller.CameraView", 5),
            ("Controller.ShiftUp", 7),
            ("Controller.ShiftDown", 8),
        ] {
            assert_eq!((b(n).kind, b(n).device, b(n).index), (2, 2, i), "{n}");
        }
        assert_eq!(d.iter().filter(|b| b.kind == 3).count(), 9);
        assert_eq!(b("Controller.Horn").index, 0x23);
    }

    /// Each gate on its own: an X-only wheel keeps the pedals on the arrows, a pad with four
    /// buttons binds only the two it has (0 and 3 of 0, 2, 3, 5, 7, 8 — and 2), and a device
    /// with axes and buttons but not analogue (0x51F080 false) is the keyboard's.
    #[test]
    fn partial_devices() {
        let x_only = default_bindings(1, DeviceCaps { analogue: true, axes: 0x01, buttons: 0 }, 0);
        assert_eq!(x_only[row("Controller.SteerLeft")].kind, 1);
        assert_eq!(x_only[row("Controller.Accel")].kind, 3);
        assert_eq!(x_only[row("Controller.Accel")].index, 0x48);
        let four = default_bindings(1, DeviceCaps { analogue: true, axes: 0x02, buttons: 0xF }, 0);
        assert_eq!(four[row("Controller.SteerLeft")].kind, 3);
        assert_eq!(four[row("Controller.Brake")].kind, 1);
        let kinds: Vec<u32> = ["Controller.HandBrake", "Controller.LookBehind", "Controller.ResetCar", "Controller.CameraView", "Controller.ShiftUp", "Controller.ShiftDown"]
            .iter()
            .map(|n| four[row(n)].kind)
            .collect();
        assert_eq!(kinds, [2, 2, 2, 3, 3, 3]);
        let not_analogue = default_bindings(1, DeviceCaps { analogue: false, axes: 0xFF, buttons: !0 }, 0);
        assert_eq!(not_analogue, default_bindings(0, DeviceCaps::default(), 0));
    }

    /// Storing the defaults over rebound rows: a row whose control another row held is simply
    /// overwritten, a row with no id here is skipped, and nothing else is touched.
    #[test]
    fn set_defaults_overwrites_the_rows() {
        let mut bs = Bindings::from_registry(|k| match k {
            "C1_LEFT" => Some("key,SC_H"),
            "C1_HORN" => Some("key,SC_LEFT"),
            "C1_ABORT" => Some("key,SC_Q"),
            _ => None,
        });
        let id_of = |n: &str| match n {
            "Controller.SteerLeft" => Some(id::C1_LEFT),
            "Controller.Horn" => Some(id::C1_HORN),
            _ => None,
        };
        let pad = DeviceCaps { analogue: true, axes: 0x03, buttons: 0x1FF };
        bs.set_defaults(&default_bindings(0, pad, 0), id_of);
        assert_eq!(bs.get(id::C1_LEFT).map(|b| (b.kind, b.index, b.dir)), Some((1, 0, -1)));
        assert_eq!(bs.get(id::C1_HORN).map(|b| (b.kind, b.index, b.key)), Some((3, 0x23, b'h' as u32)));
        assert_eq!(bs.get(id::C1_ABORT).map(|b| b.index), Some(0x10));
        assert_eq!(bs.by_id.len(), 3);
    }
}
