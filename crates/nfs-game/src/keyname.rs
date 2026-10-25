//! What a keyboard binding looks like in the controller-setup rows — **0x51C050**, the key
//! formatter `NFS5.CONTROLLERSETUP`'s draw calls for a binding of kind 3 (0x51C73B).
//!
//! The codes are the original's own: a printable character is itself, and the six keys of the
//! navigation block and the four arrows are an extended code in the high byte (`0x4800` up,
//! `0x4B00` left, `0x4D00` right, `0x5000` down, `0x4700` home, `0x4900` page up, `0x4F00` end,
//! `0x5100` page down, `0x5200` insert, `0x5300` delete).
//!
//! ```text
//! 0x51C053  buffer = ""                     ; the empty string 0x5E8E50
//! 0x51C086  arc = -1
//! 0x51C0A9  0x20…0xD6: a byte table at 0x51C2BC over a jump table at 0x51C284 —
//!           ' ' -> FEStrings[0xCF]; , . / ; = [ \ ] ` -> themselves; 0xC4 0xC5 0xD6 -> themselves
//! 0x51C092  0xE4 -> 0xC4, 0x51C142 0xE5 -> 0xC5, 0x51C147 0xF6 -> 0xD6   ; ä å ö shown upper case
//! 0x51C149  0x4700 -> FEStrings[0xC9]   0x4900 -> [0xCA]   0x4F00 -> [0xCC]   0x5100 -> [0xCB]
//!           0x5200 -> FEStrings[0xCD]   0x5300 -> [0xCE]
//! 0x51C173  0x4800 -> arc 0     0x51C1A3  0x4D00 -> arc 2
//! 0x51C1C6  0x5000 -> arc 4     0x51C18D  0x4B00 -> arc 6      ; the buffer stays empty
//! 0x51C1EE  'a'…'z' -> upper case; 'A'…'Z' -> "%c"; anything else -> "" and a false return
//! ```
//!
//! The row then prints `FEStrings[0xD0]`, `"keyboard %s"`, over the buffer (0x51C75C), and an
//! arc other than −1 puts `weeicons.fsh`'s `arc<n>` at the row's right edge (0x51C9DA).
use nfs_formats::locale::Strings;

/// `(name, arc)` for key `code`: the text 0x51C050 leaves in its buffer and the arc it picks,
/// `None` for the −1 it starts from. A code it does not know is an empty name, as in the
/// original (0x51C21A returns false with the buffer still empty; the caller ignores the flag).
pub fn key_name(code: u32, strings: &Strings) -> (String, Option<u8>) {
    let fe = |n: u32| strings.get(n).unwrap_or_default().to_string();
    let lit = |c: u8| (c as char).to_string();
    let name = match code {
        0x20 => fe(0xCF),
        0x2C | 0x2E | 0x2F | 0x3B | 0x3D | 0x5B | 0x5C | 0x5D | 0x60 | 0xC4 | 0xC5 | 0xD6 => lit(code as u8),
        0xE4 => lit(0xC4),
        0xE5 => lit(0xC5),
        0xF6 => lit(0xD6),
        0x4700 => fe(0xC9),
        0x4900 => fe(0xCA),
        0x4F00 => fe(0xCC),
        0x5100 => fe(0xCB),
        0x5200 => fe(0xCD),
        0x5300 => fe(0xCE),
        0x4800 => return (String::new(), Some(0)),
        0x4D00 => return (String::new(), Some(2)),
        0x5000 => return (String::new(), Some(4)),
        0x4B00 => return (String::new(), Some(6)),
        0x61..=0x7A => lit(code as u8 - 0x20),
        0x41..=0x5A => lit(code as u8),
        _ => String::new(),
    };
    (name, None)
}

/// The row's text for a keyboard binding: `FEStrings[0xD0]` (`"keyboard %s"`) over the key's name
/// (0x51C754…0x51C76F).
pub fn keyboard_row(code: u32, strings: &Strings) -> (String, Option<u8>) {
    let (name, arc) = key_name(code, strings);
    let fmt = strings.get(0xD0).unwrap_or_default();
    (fmt.replacen("%s", &name, 1), arc)
}

/// A pad binding's row (kinds 1, 2 and 5 of 0x51CB7C). Each starts from `FEStrings[0xDB]`,
/// `"joystick %d"`, over `0x51F170(device) + 1` — the device's place among the analogue, enabled
/// ones, which on this port is every pad, so its index — and then:
///
/// ```text
/// kind 1 axis   0x51C7D5  "%s %s" over that and FEStrings[0xD1 + axis]  ("x axis" … "slider b")
///                         arc by axis and direction [+0x20]: X, XRot, SliderA, SliderB  +1 -> 2, -1 -> 6
///                                                             Y, YRot                   +1 -> 4, -1 -> 0
///                                                             Z, ZRot                   +1 -> 0, -1 -> 4
/// kind 2 button 0x51C77C  FEStrings[0xDA] "%s Button %d" over that and index + 1
/// kind 5 hat    0x51C932  FEStrings[0xD9] "%s pov %d" over that and index − 0x1F; arc = pov [+0x24]
/// ```
///
/// `standard` (PORT-DEVIATION D44, PAD-MODERN): the device is an SDL standard pad, whose controls
/// read as the pad's own labels after the same `"joystick %d"` — `A`, `RB`, `Start`, `L3`, `LT`,
/// `LS X`, `D-pad` ([`standard_pad_label`]) — where the original's names would be DirectInput's
/// `"z axis"` for LT, `"z rotation axis"` for RT and `"Button 12"` for Start. The arcs stay the
/// original's. A raw device, or a binding of a pad not connected, keeps the original's text.
pub fn pad_row(kind: u32, device: u32, index: u32, dir: i32, pov: u32, standard: bool, strings: &Strings) -> (String, Option<u8>) {
    let fe = |n: u32| strings.get(n).unwrap_or_default().to_string();
    let joy = fe(0xDB).replacen("%d", &(device + 1).to_string(), 1);
    if standard && let Some(label) = standard_pad_label(kind, index) {
        let arc = match kind {
            1 => original_arc(index & 7, dir),
            5 => Some((pov & 7) as u8),
            _ => None,
        };
        return (format!("{joy} {label}"), arc);
    }
    match kind {
        1 => {
            let axis = index & 7;
            let text = format!("{joy} {}", fe(0xD1 + axis));
            (text, original_arc(axis, dir))
        }
        2 => (fe(0xDA).replacen("%s", &joy, 1).replacen("%d", &(index + 1).to_string(), 1), None),
        5 => (fe(0xD9).replacen("%s", &joy, 1).replacen("%d", &(index as i32 - 0x1F).to_string(), 1), Some((pov & 7) as u8)),
        _ => (String::new(), None),
    }
}

/// 0x51C7D5's arc of an axis binding by axis and direction (see [`pad_row`]).
fn original_arc(axis: u32, dir: i32) -> Option<u8> {
    let plus = dir == 1;
    Some(match axis {
        1 | 4 => if plus { 4 } else { 0 },
        2 | 5 => if plus { 0 } else { 4 },
        _ => if plus { 2 } else { 6 },
    })
}

/// **PORT-DEVIATION D44** (PAD-MODERN): a standard pad's control by the pad's own label, the
/// numbering of `nfs_input::gamepad::pad_record` — axes `LS X, LS Y, LT, RS X, RS Y, RT`, buttons
/// in `BUTTON_ORDER` (PORT-DEVIATION D25), the d-pad as the first hat. No `FEStrings` entry names
/// them (0xD1…0xDB are DirectInput's axis, button and hat words), so they are the labels printed on
/// the pads themselves, the same in every language. `None` for a slot the standard pad has not.
pub fn standard_pad_label(kind: u32, index: u32) -> Option<&'static str> {
    const AXES: [&str; 6] = ["LS X", "LS Y", "LT", "RS X", "RS Y", "RT"];
    const BUTTONS: [&str; 15] = ["A", "B", "Y", "X", "", "", "LB", "RB", "", "", "Back", "Start", "Guide", "L3", "R3"];
    let s = match kind {
        1 => AXES.get(index as usize)?,
        2 => BUTTONS.get(index as usize)?,
        5 if index == 0x20 => "D-pad",
        _ => return None,
    };
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings() -> Strings {
        let mut s = Strings::default();
        for (id, t) in [(0xC9, "home"), (0xCA, "page up"), (0xCB, "page down"), (0xCC, "end"), (0xCD, "insert"), (0xCE, "delete"), (0xCF, "space"), (0xD0, "keyboard %s")] {
            s.by_id.insert(id, t.to_string());
        }
        s
    }

    /// The four arrows are arcs with no name, a quarter-circle apart (0x51C173 / 0x51C1A3 /
    /// 0x51C1C6 / 0x51C18D); the navigation keys and space are `FEStrings`; letters print upper case.
    #[test]
    fn keys_name_as_0x51c050_does() {
        let s = strings();
        assert_eq!(keyboard_row(0x4800, &s), ("keyboard ".to_string(), Some(0)));
        assert_eq!(keyboard_row(0x4D00, &s), ("keyboard ".to_string(), Some(2)));
        assert_eq!(keyboard_row(0x5000, &s), ("keyboard ".to_string(), Some(4)));
        assert_eq!(keyboard_row(0x4B00, &s), ("keyboard ".to_string(), Some(6)));
        assert_eq!(keyboard_row(0x20, &s).0, "keyboard space");
        assert_eq!(keyboard_row(0x4F00, &s).0, "keyboard end");
        assert_eq!(keyboard_row(0x5100, &s).0, "keyboard page down");
        assert_eq!(keyboard_row(b'a' as u32, &s).0, "keyboard A");
        assert_eq!(keyboard_row(b'Q' as u32, &s).0, "keyboard Q");
        assert_eq!(keyboard_row(0x3B, &s).0, "keyboard ;");
        assert_eq!(keyboard_row(0x1B, &s), ("keyboard ".to_string(), None));
    }

    /// 0x51C7D5 / 0x51C77C / 0x51C932: the pad rows' text and arc.
    #[test]
    fn pad_rows_as_the_draw_builds_them() {
        let mut s = strings();
        for (id, t) in [(0xD1, "x axis"), (0xD2, "y axis"), (0xD9, "%s pov %d"), (0xDA, "%s Button %d"), (0xDB, "joystick %d")] {
            s.by_id.insert(id, t.to_string());
        }
        assert_eq!(pad_row(1, 0, 0, -1, 0, false, &s), ("joystick 1 x axis".to_string(), Some(6)));
        assert_eq!(pad_row(1, 0, 1, 1, 0, false, &s), ("joystick 1 y axis".to_string(), Some(4)));
        assert_eq!(pad_row(2, 1, 2, 0, 0, false, &s), ("joystick 2 Button 3".to_string(), None));
        assert_eq!(pad_row(5, 0, 0x20, 0, 6, false, &s), ("joystick 1 pov 1".to_string(), Some(6)));
        assert_eq!(pad_row(0, 0, 0, 0, 0, false, &s), (String::new(), None));
    }

    /// PAD-MODERN (D44): a standard pad's rows by the pad's labels, the arcs the original's; a
    /// slot the pad has not (button 8, the old LT button) falls back to the original's text.
    #[test]
    fn standard_pad_rows_read_as_the_pad() {
        let mut s = strings();
        for (id, t) in [(0xD3, "z axis"), (0xD6, "z rotation axis"), (0xD9, "%s pov %d"), (0xDA, "%s Button %d"), (0xDB, "joystick %d")] {
            s.by_id.insert(id, t.to_string());
        }
        assert_eq!(pad_row(1, 0, 5, 1, 0, true, &s), ("joystick 1 RT".to_string(), Some(0)));
        assert_eq!(pad_row(1, 0, 2, 1, 0, true, &s), ("joystick 1 LT".to_string(), Some(0)));
        assert_eq!(pad_row(1, 0, 0, -1, 0, true, &s), ("joystick 1 LS X".to_string(), Some(6)));
        assert_eq!(pad_row(1, 0, 3, 1, 0, true, &s), ("joystick 1 RS X".to_string(), Some(2)));
        assert_eq!(pad_row(2, 0, 11, 0, 0, true, &s), ("joystick 1 Start".to_string(), None));
        assert_eq!(pad_row(2, 0, 13, 0, 0, true, &s), ("joystick 1 L3".to_string(), None));
        assert_eq!(pad_row(5, 0, 0x20, 0, 4, true, &s), ("joystick 1 D-pad".to_string(), Some(4)));
        assert_eq!(pad_row(2, 0, 8, 0, 0, true, &s), ("joystick 1 Button 9".to_string(), None));
        // the original's words for the same axes
        assert_eq!(pad_row(1, 0, 5, 1, 0, false, &s).0, "joystick 1 z rotation axis");
        assert_eq!(pad_row(1, 0, 2, 1, 0, false, &s).0, "joystick 1 z axis");
    }
}
