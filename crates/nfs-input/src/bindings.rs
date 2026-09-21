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
        n if n.len() == 1 && n.as_bytes()[0].is_ascii_alphanumeric() => n.as_bytes()[0] as u32,
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
/// bitmask `[info+8]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PadRecord {
    pub mask: u8,
    pub axes: [i32; 8],
    pub pov: [i32; 4],
    pub buttons: [bool; 32],
}

impl Default for PadRecord {
    fn default() -> Self {
        Self { mask: 0, axes: [0x7FFF; 8], pov: [-1; 4], buttons: [false; 32] }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fe_txt_keys_parse_to_scancodes() {
        let b = parse("key,SC_RIGHT").unwrap();
        assert_eq!((b.kind, b.index, b.key), (3, 0x4D, 0x4D00));
        assert_eq!(parse("key, SC_SPACE").unwrap().key, 0x20);
        assert_eq!(parse("key,SC_A").unwrap().key, b'A' as u32);
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
}
