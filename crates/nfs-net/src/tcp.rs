//! `TCPDATA` — the data layer that puts a message on a TCP stream
//! (`RE/notes/network.md` §7.11, §7.12).
//!
//! Descriptor **0x5E1A10**, and its send `0x581460` names its own source: the error string at
//! 0x5C10C0 / 0x5C10D8 is `\real\patch3\pc\tcp.c` and *"CONN_send - PACKET TOO LONG, MAXIMUM SIZE =
//! %d (TCP)."*. The framing is the whole of it — a two-byte big-endian length, then the payload,
//! written straight to the socket:
//!
//! ```text
//! +0x00 u16be  len
//! +0x02        the payload
//! ```
//!
//! There is no marker, no sequence and no checksum: TCP carries those. The layer refuses a second
//! message while the first is still going out (`obj[+0x06] != 0`), retries a short write from
//! `obj[+0x08]`, and drops the frame on `WSAEWOULDBLOCK`.

use thiserror::Error;

/// `0x581460` writes the length with `0x56E2C0(.., 2)`, so this is what fits.
pub const MAX_PAYLOAD: usize = u16::MAX as usize;

/// The two bytes in front of every message.
pub const HEADER: usize = 2;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TcpError {
    #[error("payload is {0} bytes, the length field holds {MAX_PAYLOAD}")]
    TooLong(usize),
    #[error("need {HEADER} bytes for the length, {0} arrived")]
    Short(usize),
}

/// Frame one message: `u16be len` then `payload`.
pub fn encode(payload: &[u8]) -> Result<Vec<u8>, TcpError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(TcpError::TooLong(payload.len()));
    }
    let mut out = Vec::with_capacity(HEADER + payload.len());
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

/// Take one whole message off the front of a stream buffer, if there is one.
///
/// Returns the message and how many bytes to drop. `None` means "not yet" — the caller keeps
/// reading, which is what a stream demands and a datagram never does.
pub fn decode(buf: &[u8]) -> Option<(&[u8], usize)> {
    let n = u16::from_be_bytes([*buf.first()?, *buf.get(1)?]) as usize;
    let end = HEADER + n;
    (buf.len() >= end).then(|| (&buf[HEADER..end], end))
}

/// A stream reassembler: push whatever `recv` gave you, pull whole messages out.
#[derive(Debug, Default)]
pub struct Stream {
    buf: Vec<u8>,
}

impl Stream {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, or `None` while one is still arriving.
    pub fn take(&mut self) -> Option<Vec<u8>> {
        let (msg, used) = decode(&self.buf).map(|(m, u)| (m.to_vec(), u))?;
        self.buf.drain(..used);
        Some(msg)
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_two_bytes_of_length_then_the_payload() {
        let f = encode(&[1, 2, 3]).unwrap();
        assert_eq!(f, vec![0, 3, 1, 2, 3]);
        assert_eq!(decode(&f), Some((&[1u8, 2, 3][..], 5)));
        assert_eq!(encode(&vec![0u8; MAX_PAYLOAD + 1]), Err(TcpError::TooLong(65536)));
    }

    #[test]
    fn a_stream_gives_up_whole_messages_only() {
        let mut s = Stream::default();
        let a = encode(&crate::state::CarState::default().packet(1)).unwrap();
        let b = encode(b"hello").unwrap();
        // arrive in three pieces that cut both messages
        s.push(&a[..3]);
        assert!(s.take().is_none());
        s.push(&a[3..]);
        s.push(&b[..4]);
        let got = s.take().unwrap();
        assert_eq!(got.len(), crate::state::PACKET_SIZE);
        assert!(s.take().is_none());
        s.push(&b[4..]);
        assert_eq!(s.take().unwrap(), b"hello");
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn an_empty_message_is_legal_and_two_bytes_long() {
        let f = encode(&[]).unwrap();
        assert_eq!(f, vec![0, 0]);
        assert_eq!(decode(&f), Some((&[][..], 2)));
        assert_eq!(decode(&[0]), None);
    }
}
