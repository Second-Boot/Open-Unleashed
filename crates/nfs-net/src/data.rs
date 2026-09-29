//! The data layer the game actually installs — descriptor **0x5E2E58**
//! (`RE/notes/network.md` §7.12c).
//!
//! `0x588910`, slot `+0x38` of the connection class, chooses between three layers from a flag word,
//! and the word the CONMAN hands it is the constant `[0x5DF404] = 0x8004` — read at five sites and
//! **written nowhere**. Its low byte is non-zero and bit `0x40` of its high byte is clear, so the
//! branch always falls to `0x5E2E58`: not the six-byte CRC frame of [`crate::frame`], not the
//! four-byte one, and not TCPDATA.
//!
//! Its send `0x595C40` has the same shape as the others — `reliable = (lenflags & 0x80000000) == 0`,
//! a packet from the pool when reliable and an `alloca` when not — and `0x595D10` builds the header
//! while `0x595EB0` stamps the id and hands the frame down.
//!
//! ```text
//! +0x00 u16be  ack               htons([conn+0x5E])            0x595EEC
//! +0x02 u16be  sequence          htons([conn+0x5A])            0x595D52
//! +0x04 u8     flags             [conn+0x58]                   0x595DAD
//! +0x05 u8     length of the first message                     0x595D9A
//! +0x06        the first message
//! ```
//!
//! `conn+0x5E` is **not** an id: the receive path advances it as each frame is delivered in order
//! (0x595BAE and 0x595BEB, with the same 0xFFFB wrap), so it is the **last** sequence this side
//! delivered from the peer (0 before any: 0x5951A0 stores 0 there and starts the counter at 1) — a
//! cumulative acknowledgement riding on every frame. (An earlier reading here called it the sequence
//! next expected; the receive path 0x595B50 compares `seq == conn+0x5E + 1` and stores `seq`.) `conn+0x58` is written **once**, to 0
//! when the layer opens (0x5952B7), and read only to be copied here and into an ack-only frame, so
//! in this build the flags byte is always zero.
//!
//! and then up to **two** more messages may be appended (the loop at 0x595DF0 stops at `eax == 2`),
//! each as **two bytes and its payload** — its own length and the difference between its sequence
//! and the frame's (0x595E3C), which is how a receiver puts them back in order. Each extra message
//! costs `len + 2`, and the total is checked against `[conn+0x0C]` before it is added (0x595E1D).
//!
//! The sequence counter at `conn+0x5A` is post-incremented and **wraps to 0 at 0xFFFB** (0x595D4D);
//! an unreliable frame uses `0xFFFE` instead of a counter value (0x595D7C).
//!
//! **The finished frame goes out under the tag `'='` (0x3D)**, to the peer's own address:
//! `0x582700(peer, buf, len)` calls `transport_send(conn[0x10], peer+0x38, 0x3D, buf, len)`, after
//! a `'q'` with an empty payload when the peer is not connected yet (`peer+0x4C < 0`). The same tag
//! is what the peer layer asks for on the way back in (0x582695), so a whole in-race datagram is
//!
//! ```text
//! +0x00 u8     0x3D  '='
//! +0x01 u16be  connection id
//! +0x03 u16be  sequence
//! +0x05 u8     flags
//! +0x06 u8     the first message's length
//! +0x07        the first message  — one of the 0xB0 … 0xC6 of §5.4
//! ```
//!
//! which for the 40-byte `0xBE` car state is **47 bytes on the wire**. The session's own `'='` is
//! the keep-alive of §7.4 and carries **no** payload, so the two are told apart by length.
//!
//! The order of those two bytes is the **length first, then the delta**: the builder writes the
//! length at `esp+0x28` (0x595E21) and the delta at `esp+0x3d` (0x595E4A), and the two-byte `memcpy`
//! at 0x595E4E starts at the first of them. Both `0x595560` and `0x595930` are plain byte swaps, so
//! the delta is `frame.seq - its.seq` in host order.
//!
//! `0x596080` builds the degenerate case: a six-byte frame with **length 0** and no payload, which
//! is how an acknowledgement travels on its own.

use thiserror::Error;

/// `0x58273D` — the transport tag a finished frame goes out under. The session's keep-alive uses
/// the same character with an **empty** payload (§7.4), which is how a receiver tells them apart.
pub const TAG: u8 = b'=';

/// `0x582711` — a `'q'` with an empty payload goes first when the peer is not connected yet.
pub const CONNECT_TAG: u8 = b'q';

/// `0x595D7C` — the sequence an unreliable frame carries instead of a counter value.
pub const UNRELIABLE_SEQ: u16 = 0xFFFE;

/// `cmp di, 0xfffb` at 0x595D4D — the counter wraps to 0 here rather than at 0xFFFF.
pub const SEQ_WRAP: u16 = 0xFFFB;

/// The header in front of the first message.
pub const HEADER: usize = 6;

/// Each appended message carries its own length and a sequence delta.
pub const EXTRA_HEADER: usize = 2;

/// `inc eax ; cmp eax, 2 ; jl` — two appends at most, so three messages in a frame.
pub const MAX_MESSAGES: usize = 3;

/// A message's length is one byte.
pub const MAX_MESSAGE: usize = 255;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DataError {
    #[error("a frame carries {MAX_MESSAGES} messages at most, {0} given")]
    TooMany(usize),
    #[error("a message is {0} bytes, the length field holds {MAX_MESSAGE}")]
    TooLong(usize),
    #[error("frame is {0} bytes, the header alone needs {HEADER}")]
    TooShort(usize),
    #[error("the frame says {want} bytes of message, {have} are there")]
    Truncated { want: usize, have: usize },
}

/// One message inside a frame, with the sequence it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The frame's own sequence for the first, and the frame's sequence minus the delta byte for
    /// the rest (0x595E3C subtracts in the other direction, so this undoes it).
    pub seq: u16,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// `+0x00` — the last sequence this side delivered from the peer (`conn+0x5E`).
    pub ack: u16,
    /// `+0x02` — the frame's sequence.
    pub seq: u16,
    /// `+0x04` — `[conn+0x58]`, which nothing ever sets to anything but 0.
    pub flags: u8,
    pub pieces: Vec<Piece>,
}

impl Frame {
    /// `0x596080` — six bytes, length 0, no payload: an acknowledgement on its own.
    pub fn ack_only(ack: u16, seq: u16, flags: u8) -> Self {
        Self { ack, seq, flags, pieces: Vec::new() }
    }

    /// The next value of the counter at `conn+0x5A`.
    pub fn next_seq(seq: u16) -> u16 {
        if seq == SEQ_WRAP { 0 } else { seq.wrapping_add(1) }
    }

    pub fn encode(&self) -> Result<Vec<u8>, DataError> {
        // an empty frame is legal and is how a bare acknowledgement travels (0x596080)
        if self.pieces.len() > MAX_MESSAGES {
            return Err(DataError::TooMany(self.pieces.len()));
        }
        if let Some(p) = self.pieces.iter().find(|p| p.body.len() > MAX_MESSAGE) {
            return Err(DataError::TooLong(p.body.len()));
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.ack.to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.push(self.flags);
        let Some(first) = self.pieces.first() else {
            out.push(0);
            return Ok(out);
        };
        out.push(first.body.len() as u8);
        out.extend_from_slice(&first.body);
        for p in &self.pieces[1..] {
            out.push(p.body.len() as u8);
            out.push(((self.seq as u32 + SEQ_WRAP as u32 + 1 - p.seq as u32) % (SEQ_WRAP as u32 + 1)) as u8);
            out.extend_from_slice(&p.body);
        }
        Ok(out)
    }

    pub fn decode(b: &[u8]) -> Result<Self, DataError> {
        if b.len() < HEADER {
            return Err(DataError::TooShort(b.len()));
        }
        let seq = u16::from_be_bytes([b[2], b[3]]);
        let mut pieces = Vec::new();
        let n = b[5] as usize;
        let end = HEADER + n;
        if b.len() < end {
            return Err(DataError::Truncated { want: n, have: b.len() - HEADER });
        }
        if n > 0 {
            pieces.push(Piece { seq, body: b[HEADER..end].to_vec() });
        }
        let mut at = end;
        while at + EXTRA_HEADER <= b.len() && pieces.len() < MAX_MESSAGES {
            let n = b[at] as usize;
            let delta = b[at + 1] as u16;
            let from = at + EXTRA_HEADER;
            let to = from + n;
            if b.len() < to {
                return Err(DataError::Truncated { want: n, have: b.len() - from });
            }
            // 0x59586B: `seq − delta`, and a result past 0xFFFB has 0xFFFC added (modulo 0xFFFC)
            let m = SEQ_WRAP as u32 + 1;
            let s = (seq as u32 + m - (delta as u32 % m)) % m;
            pieces.push(Piece { seq: s as u16, body: b[from..to].to_vec() });
            at = to;
        }
        Ok(Self { ack: u16::from_be_bytes([b[0], b[1]]), seq, flags: b[4], pieces })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(seq: u16, n: usize) -> Piece {
        Piece { seq, body: (0..n).map(|i| i as u8).collect() }
    }

    #[test]
    fn one_message_is_six_bytes_of_header_and_the_body() {
        let f = Frame { ack: 0x1234, seq: 7, flags: 0x20, pieces: vec![piece(7, 3)] };
        let b = f.encode().unwrap();
        assert_eq!(b, vec![0x12, 0x34, 0, 7, 0x20, 3, 0, 1, 2]);
        assert_eq!(Frame::decode(&b).unwrap(), f);
    }

    #[test]
    fn two_more_messages_cost_two_bytes_each() {
        let f = Frame {
            ack: 1,
            seq: 100,
            flags: 0,
            pieces: vec![piece(100, 2), piece(98, 4), piece(95, 1)],
        };
        let b = f.encode().unwrap();
        // 6 + 2 + (2 + 4) + (2 + 1)
        assert_eq!(b.len(), HEADER + 2 + 6 + 3);
        assert_eq!(b[8], 4); // the second message's length
        assert_eq!(b[9], 2); // 100 - 98
        assert_eq!(Frame::decode(&b).unwrap(), f);
    }

    #[test]
    fn the_frame_holds_three_messages_at_most() {
        let f = Frame { ack: 0, seq: 0, flags: 0, pieces: vec![piece(0, 1); 4] };
        assert_eq!(f.encode(), Err(DataError::TooMany(4)));
        // and the empty one is the bare acknowledgement of 0x596080
        let a = Frame::ack_only(5, 9, 0);
        assert_eq!(a.encode().unwrap(), vec![0, 5, 0, 9, 0, 0]);
        assert_eq!(Frame::decode(&a.encode().unwrap()).unwrap(), a);
    }

    #[test]
    fn the_counter_wraps_at_fffb_not_ffff() {
        assert_eq!(Frame::next_seq(0), 1);
        assert_eq!(Frame::next_seq(SEQ_WRAP - 1), SEQ_WRAP);
        assert_eq!(Frame::next_seq(SEQ_WRAP), 0);
        // and an unreliable frame uses a value the counter can never reach
        const { assert!(UNRELIABLE_SEQ > SEQ_WRAP) };
    }

    #[test]
    fn a_car_state_fits_with_room_for_two_more() {
        let s = crate::state::CarState::default();
        let f = Frame {
            ack: 9,
            seq: UNRELIABLE_SEQ,
            flags: 0,
            pieces: vec![Piece { seq: UNRELIABLE_SEQ, body: s.packet(1).to_vec() }],
        };
        let b = f.encode().unwrap();
        assert_eq!(b.len(), HEADER + crate::state::PACKET_SIZE);
        let back = Frame::decode(&b).unwrap();
        assert_eq!(back.pieces[0].body.len(), crate::state::PACKET_SIZE);
        assert_eq!(back.seq, UNRELIABLE_SEQ);
    }

    #[test]
    fn a_short_or_lying_frame_is_refused() {
        assert_eq!(Frame::decode(&[0; 5]), Err(DataError::TooShort(5)));
        assert_eq!(Frame::decode(&[0, 0, 0, 0, 0, 9]), Err(DataError::Truncated { want: 9, have: 0 }));
    }
}
