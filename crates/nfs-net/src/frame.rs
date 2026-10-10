//! The six-byte frame with the CRC-16 — data layer **0x5E1C50** (`RE/notes/network.md` §7.12a).
//!
//! `0x588910`, slot `+0x34` of the UDP connection class, installs it when the connection's flag
//! word asks for it; `0x55EAE0` uses the same descriptor for the modem and serial links. It is one
//! of the five data layers of §7.11 — [`crate::tcp`] is the one that needs no framing of its own
//! because TCP already sequences and checks.
//!
//! The flag `0x55D4D0` ORs into the length — `0x80000000` — is read here as
//! `reliable = (lenflags & 0x80000000) == 0`: the car-state broadcast is sent once, everything else
//! is queued on `conn[0x14]+0x54` until acknowledged.

use thiserror::Error;

/// `mov byte ptr [esi + 0xc], 0xfe` (0x586246) — the first byte of every frame. The 4-byte
/// header of the other serial layer (0x586A40) opens with the same value.
pub const MARKER: u8 = 0xFE;

/// `mov byte ptr [esi + 0xe], 0xfc` (0x586237) — what stands in for the sequence when the message
/// is unreliable.
pub const UNSEQUENCED: u8 = 0xFC;

/// The sequence and the ack are both masked to seven bits (0x586227, 0x585D52).
pub const SEQ_MASK: u8 = 0x7F;

/// `0x55D4D0` sets this on the length word, and 0x586120 reads it back.
pub const UNRELIABLE: u32 = 0x8000_0000;

/// `pkt[0x0D] + 6` is the datagram length (0x58627A), and the length is one byte.
pub const HEADER: usize = 6;

/// The length field is a single byte, so this is all a frame can carry.
pub const MAX_PAYLOAD: usize = 255;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame is {0} bytes, the header alone needs {HEADER}")]
    TooShort(usize),
    #[error("first byte is {0:#04x}, not the marker {MARKER:#04x}")]
    NotAFrame(u8),
    #[error("the length byte says {want} payload bytes, {have} arrived")]
    LengthMismatch { want: usize, have: usize },
    #[error("CRC is {got:#06x}, {want:#06x} computed")]
    BadCrc { got: u16, want: u16 },
    #[error("payload is {0} bytes, the length byte only holds {MAX_PAYLOAD}")]
    TooLong(usize),
}

/// What goes in the sequence byte: a real sequence number, or the unreliable stand-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sequence {
    /// `sequence & 0x7F` of a packet that will be retransmitted until acknowledged.
    Reliable(u8),
    /// 0xFC — sent once and forgotten, which is what the car state uses.
    Unreliable,
}

impl Sequence {
    fn byte(self) -> u8 {
        match self {
            Self::Reliable(n) => n & SEQ_MASK,
            Self::Unreliable => UNSEQUENCED,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub sequence: Sequence,
    /// `+0x03`: `(the last sequence received - 1) & 0x7F`, which 0x585D30 reads off the peer.
    pub ack: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(FrameError::TooLong(self.payload.len()));
        }
        let mut out = Vec::with_capacity(HEADER + self.payload.len());
        out.push(MARKER);
        out.push(self.payload.len() as u8);
        out.push(self.sequence.byte());
        out.push(self.ack & SEQ_MASK);
        out.extend_from_slice(&[0, 0]); // the CRC is computed with its own two bytes zeroed
        out.extend_from_slice(&self.payload);
        let crc = crc16(&out);
        out[4] = (crc >> 8) as u8;
        out[5] = crc as u8;
        Ok(out)
    }

    pub fn decode(b: &[u8]) -> Result<Self, FrameError> {
        if b.len() < HEADER {
            return Err(FrameError::TooShort(b.len()));
        }
        if b[0] != MARKER {
            return Err(FrameError::NotAFrame(b[0]));
        }
        let want = b[1] as usize;
        if b.len() != HEADER + want {
            return Err(FrameError::LengthMismatch { want, have: b.len().saturating_sub(HEADER) });
        }
        let got = u16::from_be_bytes([b[4], b[5]]);
        // the checksum is taken with its own two bytes zeroed — streamed over the three pieces, no copy
        let want_crc = [&b[..4], &[0, 0][..], &b[HEADER..]].into_iter().fold(CRC_INIT, crc16_update);
        if got != want_crc {
            return Err(FrameError::BadCrc { got, want: want_crc });
        }
        Ok(Self {
            sequence: if b[2] == UNSEQUENCED { Sequence::Unreliable } else { Sequence::Reliable(b[2]) },
            ack: b[3],
            payload: b[HEADER..].to_vec(),
        })
    }
}

/// `0x5B1AC0` — a reflected CRC-16 over the table at 0x5E2A50, which is the ordinary CRC-16/ARC
/// table (polynomial 0xA001). The only thing the original changes is the initial value.
pub const CRC_INIT: u16 = 0xFBEA;
const POLY: u16 = 0xA001;

pub fn crc16(bytes: &[u8]) -> u16 {
    crc16_update(CRC_INIT, bytes)
}

/// Continue a running checksum over more bytes (`crc16(a ++ b) == crc16_update(crc16(a), b)`).
fn crc16_update(mut crc: u16, bytes: &[u8]) -> u16 {
    for b in bytes {
        let idx = ((crc ^ *b as u16) & 0xFF) as usize;
        crc = (crc >> 8) ^ table()[idx];
    }
    crc
}

fn table() -> &'static [u16; 256] {
    static T: std::sync::OnceLock<[u16; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u16; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u16;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            }
            *e = c;
        }
        t
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decode's streamed checksum against the copy-and-zero form it replaced, on frames with
    /// random contents and on corrupted ones.
    #[test]
    fn the_streamed_checksum_equals_the_copy_and_zero_form() {
        let mut x = 0x1234_5678u32;
        let mut next = || {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 16) as u8
        };
        for n in 0..=MAX_PAYLOAD.min(40) {
            let payload: Vec<u8> = (0..n).map(|_| next()).collect();
            let f = Frame { sequence: Sequence::Reliable(next() & SEQ_MASK), ack: next() & SEQ_MASK, payload };
            let mut bytes = f.encode().unwrap();
            assert_eq!(Frame::decode(&bytes).unwrap(), f);
            let mut zeroed = bytes.clone();
            zeroed[4] = 0;
            zeroed[5] = 0;
            assert_eq!(u16::from_be_bytes([bytes[4], bytes[5]]), crc16(&zeroed));
            // corrupt one byte at a time: the old and the new form reject the same ones
            for k in 0..bytes.len() {
                bytes[k] ^= 0x5A;
                let mut z = bytes.clone();
                z[4] = 0;
                z[5] = 0;
                let old_ok = bytes[0] == MARKER && bytes[1] as usize == n && u16::from_be_bytes([bytes[4], bytes[5]]) == crc16(&z);
                assert_eq!(Frame::decode(&bytes).is_ok(), old_ok, "n {n} byte {k}");
                bytes[k] ^= 0x5A;
            }
        }
    }


    #[test]
    fn the_table_matches_the_originals() {
        // spot values read straight out of 0x5E2A50
        let t = table();
        assert_eq!(&t[..8], &[0x0000, 0xC0C1, 0xC181, 0x0140, 0xC301, 0x03C0, 0x0280, 0xC241]);
        assert_eq!(*t.iter().max().unwrap(), 0xFF01);
    }

    #[test]
    fn an_empty_frame_is_the_init_value_over_the_header() {
        let f = Frame { sequence: Sequence::Unreliable, ack: 0, payload: vec![] };
        let b = f.encode().unwrap();
        assert_eq!(b.len(), HEADER);
        assert_eq!(&b[..4], &[0xFE, 0, 0xFC, 0]);
        assert_eq!(Frame::decode(&b).unwrap(), f);
    }

    #[test]
    fn a_frame_round_trips_and_the_crc_covers_everything() {
        let f = Frame { sequence: Sequence::Reliable(0x33), ack: 0x12, payload: (0..40).collect() };
        let b = f.encode().unwrap();
        assert_eq!(b.len(), HEADER + 40);
        assert_eq!(b[1], 40);
        assert_eq!(Frame::decode(&b).unwrap(), f);

        let mut bad = b.clone();
        bad[HEADER + 5] ^= 1;
        assert!(matches!(Frame::decode(&bad), Err(FrameError::BadCrc { .. })));
    }

    #[test]
    fn the_sequence_is_seven_bits_and_0xfc_means_unreliable() {
        let f = Frame { sequence: Sequence::Reliable(0xFF), ack: 0xFF, payload: vec![1] };
        let b = f.encode().unwrap();
        assert_eq!(b[2], 0x7F);
        assert_eq!(b[3], 0x7F);
        assert_eq!(Frame::decode(&b).unwrap().sequence, Sequence::Reliable(0x7F));
        // 0xFC is above the mask, so it can never collide with a real sequence
        const { assert!(UNSEQUENCED > SEQ_MASK) };
    }

    #[test]
    fn the_length_byte_bounds_the_payload() {
        let f = Frame { sequence: Sequence::Unreliable, ack: 0, payload: vec![0; 256] };
        assert_eq!(f.encode(), Err(FrameError::TooLong(256)));
        // and the car-state packet of §5.2 fits with room to spare
        const { assert!(crate::state::PACKET_SIZE <= MAX_PAYLOAD) };
    }

    #[test]
    fn a_truncated_or_padded_frame_is_refused() {
        let f = Frame { sequence: Sequence::Reliable(1), ack: 0, payload: vec![9; 4] };
        let b = f.encode().unwrap();
        assert!(matches!(Frame::decode(&b[..5]), Err(FrameError::TooShort(5))));
        assert!(matches!(Frame::decode(&b[..b.len() - 1]), Err(FrameError::LengthMismatch { want: 4, have: 3 })));
        assert_eq!(Frame::decode(&[0x11, 0, 0, 0, 0, 0]), Err(FrameError::NotAFrame(0x11)));
    }

    #[test]
    fn the_unreliable_flag_is_the_one_nfsnet_sets() {
        // 0x55D4E7 ORs it in, 0x586134 reads it back out
        assert_eq!(UNRELIABLE, 0x8000_0000);
        let lenflags = 0x28u32 | UNRELIABLE;
        assert_eq!(lenflags & 0x00FF_FFFF, 0x28);
        assert!(lenflags & UNRELIABLE != 0);
    }
}
