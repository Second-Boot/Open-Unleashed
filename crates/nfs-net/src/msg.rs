//! Control messages `0xB0 … 0xC6` (`network.md` §5.4).
//!
//! Every one has the same head: the type byte, then a **24-bit big-endian value, sign-extended**
//! (0x48F318…0x48F338 assembles it as `(b1 << 16) | (b2 << 8) | b3` and then `shl 8; sar 8`), and
//! then a body that is big-endian throughout. On most messages the 24-bit value is a player id.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MsgError {
    #[error("message is {0} bytes, the head alone needs 4")]
    TooShort(usize),
    #[error("type {0:#04x} is outside the 0xB0..=0xC6 the dispatch table covers")]
    UnknownType(u8),
    #[error("body is {have} bytes, {want} wanted")]
    ShortBody { have: usize, want: usize },
}

/// The first and last ids the jump table at 0x48FC1C covers.
pub const FIRST: u8 = 0xB0;
pub const LAST: u8 = 0xC6;

/// The ids this port understands by name. The rest travel as [`Kind::Other`], which keeps a
/// conversation going without pretending to know what a message means.
pub mod kind {
    /// 0x48F3D2 — a player is in the room; the body is its name, which `0x48C840` copies into a
    /// new record (`crate::room`).
    pub const JOIN: u8 = 0xB0;
    /// 0x48BC1B (host) / 0x48F799 (player) — "your id is" and the room's name, to a newcomer.
    pub const YOUR_ID: u8 = 0xB2;
    /// 0x48BAA0 / 0x48BD10 — the room's 0x50-byte description, sent to a newcomer (0x48F9D2 reads it).
    pub const DESCRIPTION: u8 = 0xB4;
    /// 0x48F46F — a player has gone; the handler removes it from the list at `net+0x260`.
    pub const LEAVE: u8 = 0xB1;
    /// 0x48F38B — a chat line (`crate::room::ChatLine`). *(This was named `JOIN_ALT`, "the second
    /// player-building message", until 2026-09-25; 0x48F38B builds nothing — it hands the text to
    /// the callback at `[net+0x20]`.)*
    pub const CHAT: u8 = 0xBB;
    /// 0x48F6AD — the first chunk of a player's file (`crate::room::Chunk`).
    pub const DATA_FIRST: u8 = 0xBF;
    /// 0x48F6AD — every later chunk of it.
    pub const DATA_MORE: u8 = 0xC0;
    /// 0x48EDF0 / 0x48F5E8 — the per-tick car state of [`crate::state`].
    pub const CAR_STATE: u8 = 0xBE;
    /// 0x48F82F — the race is starting. Sent by the host's room object when it starts (0x48BF10:
    /// `0x48C870(0xB3, 0, clock)` to every link, after it has set the room's "started" byte
    /// `+0xCC3`); the word is the host's game clock, which the receiver turns into its own offset
    /// (0x48F8C8 → 0x48A9B0: `[clock] = word − GetTickCount()`).
    pub const START: u8 = 0xB3;
    /// 0x48F998 — a block of option records for the option table (`crate::options`); sent by
    /// 0x48D440 / 0x48D550 from the host's front end and launch (`network.md` §11.6).
    pub const OPTIONS: u8 = 0xBC;
    /// 0x48F579 (player) / the room's 0x4918BD → 0x4929B0 → 0x48C220 — a player's finish time, or
    /// −1 (leaving the game) / −2 (back in the front end) in the head (`crate::room::race_time`).
    pub const RACE_TIME: u8 = 0xB7;
    /// 0x48F8D2 (player) / the room's 0x49185B → 0x48BFB0 — the room's "go", answering the first
    /// `0xC5` request of a race (`crate::room::go`).
    pub const GO: u8 = 0xC5;
    /// 0x48F527 (player) / 0x491661 (host) — the lobby's ping exchange (`crate::room::Ping`).
    pub const PING: u8 = 0xC3;
    /// 0x48F34F — eject / shut down (`push 0xC6` at 0x491748, under "Ejected by host").
    pub const EJECT: u8 = 0xC6;
}

/// One decoded message head plus its body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message<'a> {
    pub kind: u8,
    /// bytes 1..3, big-endian and sign-extended — the player id on most messages
    pub value: i32,
    /// everything from +4
    pub body: &'a [u8],
}

/// Sign-extend a 24-bit value, the way 0x48F32F..0x48F338 does with `shl 8` / `sar 8`.
pub fn sign_extend_24(v: u32) -> i32 {
    ((v << 8) as i32) >> 8
}

impl<'a> Message<'a> {
    pub fn parse(buf: &'a [u8]) -> Result<Self, MsgError> {
        if buf.len() < 4 {
            return Err(MsgError::TooShort(buf.len()));
        }
        let kind = buf[0];
        if !(FIRST..=LAST).contains(&kind) {
            return Err(MsgError::UnknownType(kind));
        }
        let v = ((buf[1] as u32) << 16) | ((buf[2] as u32) << 8) | buf[3] as u32;
        Ok(Self { kind, value: sign_extend_24(v), body: &buf[4..] })
    }

    /// The head, written back the way 0x48C870's callers build it.
    pub fn write_head(kind: u8, value: i32, out: &mut Vec<u8>) {
        let v = (value as u32) & 0x00FF_FFFF;
        out.push(kind);
        out.push((v >> 16) as u8);
        out.push((v >> 8) as u8);
        out.push(v as u8);
    }

    /// The body's first four bytes as the big-endian u32 the handlers read (0x48F543…, 0x48F595…).
    pub fn body_u32(&self) -> Result<u32, MsgError> {
        let b = self.body.get(..4).ok_or(MsgError::ShortBody { have: self.body.len(), want: 4 })?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_is_a_type_and_a_signed_24_bit_value() {
        let mut buf = Vec::new();
        Message::write_head(kind::CAR_STATE, 5, &mut buf);
        assert_eq!(buf, vec![0xBE, 0x00, 0x00, 0x05]);
        let m = Message::parse(&buf).unwrap();
        assert_eq!((m.kind, m.value), (0xBE, 5));

        // the sign extension is the original's `shl 8; sar 8`
        assert_eq!(sign_extend_24(0x00_0001), 1);
        assert_eq!(sign_extend_24(0xFF_FFFF), -1);
        assert_eq!(sign_extend_24(0x80_0000), -8_388_608);
        assert_eq!(sign_extend_24(0x7F_FFFF), 8_388_607);
        let mut neg = Vec::new();
        Message::write_head(kind::EJECT, -1, &mut neg);
        assert_eq!(neg, vec![0xC6, 0xFF, 0xFF, 0xFF]);
        assert_eq!(Message::parse(&neg).unwrap().value, -1);
    }

    #[test]
    fn only_the_dispatch_tables_range_is_accepted() {
        assert_eq!(Message::parse(&[0xAF, 0, 0, 0]), Err(MsgError::UnknownType(0xAF)));
        assert_eq!(Message::parse(&[0xC7, 0, 0, 0]), Err(MsgError::UnknownType(0xC7)));
        assert!(Message::parse(&[0xB0, 0, 0, 0]).is_ok());
        assert!(Message::parse(&[0xC6, 0, 0, 0]).is_ok());
        assert_eq!(Message::parse(&[0xBE, 0, 0]), Err(MsgError::TooShort(3)));
    }

    #[test]
    fn a_body_word_is_big_endian() {
        let m = Message::parse(&[0xB7, 0, 0, 7, 0x12, 0x34, 0x56, 0x78]).unwrap();
        assert_eq!(m.value, 7);
        assert_eq!(m.body_u32().unwrap(), 0x1234_5678);
    }
}
