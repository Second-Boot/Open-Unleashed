//! The per-player slot table the lobby fills over `0xBC` — `RE/notes/network.md` §11.9.
//!
//! The original keeps one 0x580-byte record per player slot at `0x65808C + slot·0x580`
//! (`replay.md` §16.2: the option table's per-slot band, ids `≥ 0x21E` and the strings
//! `0x13A … 0x141`, which `0x4B4B80` writes as `*(row.ptr + slot·0x580) = value` /
//! a string copy). The lobby sends four of its fields, each as one `0xBC` record whose second
//! dword is the *player id* used as the slot:
//!
//! | id | field | address | sent by |
//! |---|---|---|---|
//! | `0x226` | `.FEMULTIPLAYERCARID` — the catalogue id of the car | `0x658600` | `zMergedList.Car`'s slot 1 (0x4F4890, 0x4F4930) |
//! | `0x141` | `.FEMULTIPLAYERCARNAME` — its name, `%s *` for a garage car | `0x6585C0` | the same, through 0x4EDB90 (0x4F49B5), only for `Game.GameType == 3` |
//! | `0x225` | `.COLOUR` — the paint | `0x65809C` | the same (0x4F49CF) |
//! | `0x255` | `.READY` | `0x658604` | the screens' ticks and `READY` (0x50A300, 0x50A4F0, 0x509290, 0x50CF38) |
//!
//! Every send is `slot = 0x4EDCD0()` — our own id — and goes through 0x48D550, one message per record.

/// `.FEMULTIPLAYERCARID` (`zMergedList.Car`'s 0x4F4930 pushes `0x226`).
pub const CAR_ID: u32 = 0x226;
/// `.FEMULTIPLAYERCARNAME` — a string record `(id, slot, text)` (0x4EDB90).
pub const CAR_NAME: u32 = 0x141;
/// `.COLOUR` (0x4F49CF pushes `0x225`).
pub const COLOUR: u32 = 0x225;
/// `.READY` (0x50A402, 0x50A5EA, 0x50CF53 push `0x255`).
pub const READY: u32 = 0x255;

/// A player slot holds eight players' records here (`0x48BB59` refuses a ninth).
pub const SLOTS: usize = 8;
/// The name field is 0x40 bytes (`+0x534 … +0x574`): 0x4B4B80 copies the string unbounded, the
/// port stops at the field's end.
pub const NAME_MAX: usize = 0x3F;

/// One player's record, as far as the lobby fills it. A record nothing has written reads zero:
/// no car (`0x226` = 0 fails 0x50A387's `> 0` test), no name, colour 0, not ready.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerSlot {
    pub car_id: i32,
    pub car_name: Vec<u8>,
    pub colour: u32,
    pub ready: bool,
}

/// The eight records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slots {
    pub slot: [PlayerSlot; SLOTS],
}

impl Slots {
    /// **0x4B4B80** for the four ids above: a three-dword record `(id, slot, value)` writes
    /// `value` into the field of slot `slot`; `(0x141, slot, string)` copies the string. Answers
    /// whether the record was one of these (and in range) and so has been applied.
    pub fn apply(&mut self, rec: &[u32]) -> bool {
        let (Some(&id), Some(&slot)) = (rec.first(), rec.get(1)) else { return false };
        let Some(s) = self.slot.get_mut(slot as usize) else { return false };
        match id {
            CAR_ID => s.car_id = rec.get(2).copied().unwrap_or(0) as i32,
            COLOUR => s.colour = rec.get(2).copied().unwrap_or(0),
            READY => s.ready = rec.get(2).copied().unwrap_or(0) != 0,
            CAR_NAME => {
                let bytes: Vec<u8> = rec.get(2..).unwrap_or(&[]).iter().flat_map(|w| w.to_le_bytes()).collect();
                s.car_name = bytes.into_iter().take_while(|&c| c != 0).take(NAME_MAX).collect();
            }
            _ => return false,
        }
        true
    }

    /// Clear a slot — the player has gone.
    pub fn clear(&mut self, slot: usize) {
        if let Some(s) = self.slot.get_mut(slot) {
            *s = PlayerSlot::default();
        }
    }
}

/// **0x4EDB90** `(id, cursor, slot, text)`: `id`, `slot`, then the text's bytes and its NUL packed
/// four to a dword (little-endian, zero padded) — `⌈(len + 1) / 4⌉` dwords, the size 0x4B4CD0
/// computes for the `0x13A … 0x141` band.
pub fn string_record(id: u32, slot: u32, text: &[u8]) -> Vec<u32> {
    let text: Vec<u8> = text.iter().copied().take_while(|&c| c != 0).take(NAME_MAX).collect();
    let mut v = vec![id, slot];
    let mut bytes = text.clone();
    bytes.push(0);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
    v.extend(bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
    v
}

/// The records `zMergedList.Car`'s slot 1 (0x4F4890) encodes for the selected car, in its order:
/// `(0x226, slot, car id)` (0x4F492B), `(0x141, slot, name)` (0x4F49B5 — only when
/// `Game.GameType == 3`, which the lobby always is), `(0x225, slot, colour)` (0x4F49CF).
pub fn car_records(slot: u32, car_id: i32, name: &[u8], colour: u32) -> Vec<Vec<u32>> {
    vec![vec![CAR_ID, slot, car_id as u32], string_record(CAR_NAME, slot, name), vec![COLOUR, slot, colour]]
}

/// `(0x255, slot, 0 | 1)` — 0x4EDC00 with the ready state (0x50A402, 0x50A5EA, 0x50CF53).
pub fn ready_record(slot: u32, ready: bool) -> Vec<u32> {
    vec![READY, slot, u32::from(ready)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{decode, message, record_words};

    #[test]
    fn a_car_choice_is_three_records_that_fill_the_slot() {
        let mut t = Slots::default();
        for r in car_records(3, 154, b"'94 993 Coupe *", 0x00_44_55_66) {
            assert!(t.apply(&r));
        }
        assert!(t.apply(&ready_record(3, true)));
        let s = &t.slot[3];
        assert_eq!((s.car_id, s.colour, s.ready), (154, 0x0044_5566, true));
        assert_eq!(s.car_name, b"'94 993 Coupe *".to_vec());
        // the other slots are untouched
        assert_eq!(t.slot[0], PlayerSlot::default());
    }

    #[test]
    fn the_string_record_has_the_size_the_dispatch_expects_and_survives_the_wire() {
        for text in [&b""[..], b"a", b"abc", b"abcd", b"Porsche 911 Carrera"] {
            let rec = string_record(CAR_NAME, 2, text);
            assert_eq!(record_words(&rec), Some(rec.len()), "{text:?}");
            let got = decode(&message(&rec));
            assert_eq!(got, vec![rec.clone()]);
            let mut t = Slots::default();
            assert!(t.apply(&got[0]));
            assert_eq!(t.slot[2].car_name, text.to_vec());
        }
    }

    #[test]
    fn a_slot_out_of_range_or_an_unknown_id_changes_nothing() {
        let mut t = Slots::default();
        assert!(!t.apply(&[READY, 8, 1]));
        assert!(!t.apply(&[0x21E, 0, 1]));
        assert_eq!(t, Slots::default());
    }
}
