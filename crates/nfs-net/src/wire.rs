//! The bytes on the wire: the datagram the original's transport actually sends
//! (`RE/notes/network.md` §7).
//!
//! The EA Comm middleware adds **one byte** in front of whatever the layer above gives it — the
//! message-type character — and nothing else. `0x587950` writes it at `pkt+0x28`, copies the
//! payload after it, and stores `len + 1` as the packet length; the sender thread 0x5874E0 then
//! `sendto`s `pkt+0x28` for `pkt+0x24` bytes. The receive side (0x5879E0) hands a caller only the
//! datagrams whose first byte equals the tag it asked for.
//!
//! Everything here is transcribed from the disassembly. It has **not** been checked against a
//! running original — see §7.8.

use thiserror::Error;

/// `recvfrom(sock, pkt+0x28, 0x11A, …)` at 0x587451: the largest datagram the original will read.
pub const MAX_DATAGRAM: usize = 0x11A;

/// One tag byte comes off the front, so this is the largest payload that fits.
pub const MAX_PAYLOAD: usize = MAX_DATAGRAM - 1;

/// The slot a packet's buffer occupies inside the socket object (`+0x28` of a 0x144 stride).
pub const PACKET_BUFFER: usize = 0x144 - 0x28;

/// `select` timeout of the receiver thread, `tv_usec = 0x61A8` at 0x587412.
pub const RECV_POLL: std::time::Duration = std::time::Duration::from_micros(0x61A8);

/// Packets per pool, and there are two pools — 0x587780 and 0x5877AC.
pub const POOL_PACKETS: usize = 8;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum WireError {
    #[error("a datagram cannot be empty — the tag byte is always there")]
    Empty,
    #[error("payload is {0} bytes, the original's buffer takes {MAX_PAYLOAD}")]
    TooLong(usize),
}

/// A whole datagram: the tag byte 0x587950 writes, and the payload after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    pub tag: u8,
    pub payload: Vec<u8>,
}

impl Datagram {
    /// Lay a datagram out exactly as `0x587950` does: `tag` then `payload`, total `len + 1`.
    pub fn encode(tag: u8, payload: &[u8]) -> Result<Vec<u8>, WireError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(WireError::TooLong(payload.len()));
        }
        let mut out = Vec::with_capacity(payload.len() + 1);
        out.push(tag);
        out.extend_from_slice(payload);
        Ok(out)
    }

    /// Split a received datagram the way `0x5879E0` does. Anything past [`MAX_DATAGRAM`] could not
    /// have come from the original's `recvfrom`, so it is refused rather than silently truncated.
    pub fn decode(bytes: &[u8]) -> Result<Self, WireError> {
        match bytes.split_first() {
            None => Err(WireError::Empty),
            Some(_) if bytes.len() > MAX_DATAGRAM => Err(WireError::TooLong(bytes.len() - 1)),
            Some((tag, rest)) => Ok(Self { tag: *tag, payload: rest.to_vec() }),
        }
    }
}

/// The 20-byte address record the transport carries in every packet (`pkt+0x04`, five dwords).
///
/// 0x587250 turns one into a `sockaddr` for `sendto` and 0x5871D0 turns a `sockaddr` back into one,
/// branching on the socket's family word at `sock+0x30`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Addr(pub [u8; 20]);

impl Addr {
    /// IP: the record **is** the `sockaddr_in`, with `sin_family = 2` written by 0x587272 and the
    /// remaining four bytes of the 20 left over from the 16 the socket's `+0x34` asks for.
    pub fn ip(addr: [u8; 4], port: u16) -> Self {
        let mut r = [0u8; 20];
        r[0..2].copy_from_slice(&2u16.to_le_bytes()); // AF_INET, host order in the sockaddr
        r[2..4].copy_from_slice(&port.to_be_bytes()); // sin_port is network order
        r[4..8].copy_from_slice(&addr);
        Self(r)
    }

    /// IPX (0x587279): family 6, then network (4), node (6) and socket (2) — the record keeps the
    /// same field order as the `sockaddr_ipx` the transport builds from it.
    pub fn ipx(network: [u8; 4], node: [u8; 6], socket: u16) -> Self {
        let mut r = [0u8; 20];
        r[0..2].copy_from_slice(&6u16.to_le_bytes());
        r[2..6].copy_from_slice(&network);
        r[6..12].copy_from_slice(&node);
        r[12..14].copy_from_slice(&socket.to_be_bytes());
        Self(r)
    }

    /// The family word the socket compares at `sock+0x30`: 2 = `AF_INET`, 6 = `AF_IPX`.
    pub fn family(&self) -> u16 {
        u16::from_le_bytes([self.0[0], self.0[1]])
    }

    /// `ntohs` of the port, which is what 0x5871D0 returns to its caller.
    pub fn port(&self) -> u16 {
        match self.family() {
            6 => u16::from_be_bytes([self.0[12], self.0[13]]),
            _ => u16::from_be_bytes([self.0[2], self.0[3]]),
        }
    }

    pub fn ipv4(&self) -> Option<[u8; 4]> {
        (self.family() == 2).then(|| [self.0[4], self.0[5], self.0[6], self.0[7]])
    }
}

/// Where a queued packet is going. `0x587950` stores "broadcast" as `pkt+0x18 = (to == NULL)`, and
/// the sender thread then builds the address from the socket's own port (0x5872C0) instead of the
/// packet's record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    To(Addr),
    /// `sendto` to the broadcast address on the socket's bound port (`sock+0x38`).
    Broadcast,
}

impl Target {
    pub fn from_option(addr: Option<Addr>) -> Self {
        match addr {
            Some(a) => Self::To(a),
            None => Self::Broadcast,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_datagram_is_the_tag_byte_then_the_payload() {
        let d = Datagram::encode(b'p', &[1, 2, 3]).unwrap();
        assert_eq!(d, vec![b'p', 1, 2, 3]);
        assert_eq!(Datagram::decode(&d).unwrap(), Datagram { tag: b'p', payload: vec![1, 2, 3] });
    }

    #[test]
    fn the_payload_stops_where_the_originals_buffer_does() {
        assert_eq!(MAX_PAYLOAD, 281);
        assert!(Datagram::encode(b'=', &vec![0u8; MAX_PAYLOAD]).is_ok());
        assert_eq!(Datagram::encode(b'=', &vec![0u8; MAX_PAYLOAD + 1]), Err(WireError::TooLong(282)));
        assert_eq!(Datagram::decode(&[]), Err(WireError::Empty));
    }

    #[test]
    fn the_packet_buffer_holds_a_whole_datagram() {
        // pkt+0x28 to the next packet at +0x144 leaves 0x11C, and recvfrom asks for 0x11A
        assert_eq!(PACKET_BUFFER, 0x11C);
        assert_eq!(MAX_DATAGRAM, 0x11A);
    }

    #[test]
    fn an_ip_record_keeps_the_sockaddr_layout() {
        let a = Addr::ip([192, 168, 0, 7], crate::PORT);
        assert_eq!(a.family(), 2);
        assert_eq!(a.port(), crate::PORT);
        assert_eq!(a.ipv4(), Some([192, 168, 0, 7]));
        assert_eq!(&a.0[0..8], &[2, 0, 0x24, 0xE2, 192, 168, 0, 7]);
    }

    #[test]
    fn an_ipx_record_is_network_node_socket() {
        let a = Addr::ipx([1, 2, 3, 4], [0xff; 6], 0x452);
        assert_eq!(a.family(), 6);
        assert_eq!(a.port(), 0x452);
        assert_eq!(a.ipv4(), None);
    }
}
