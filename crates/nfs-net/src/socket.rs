//! A UDP socket that speaks the original's datagram framing (`RE/notes/network.md` §7.1, §7.2).
//!
//! The original runs two threads around one socket — 0x587340 `select`s with a 25 ms timeout and
//! `recvfrom`s into a free packet, 0x5874E0 drains the send queue — and both pools hold eight
//! packets of 0x11C bytes. A port does not need the threads: the game already has a frame loop, so
//! this is the same socket set up the same way (`SO_BROADCAST`, 0x560570) polled without blocking.
//! What crosses the wire is byte for byte what [`crate::wire::Datagram`] describes.

use crate::wire::{Addr, Datagram, MAX_DATAGRAM, MAX_PAYLOAD};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};

/// One socket, set up the way `0x560550` sets its own up.
pub struct CommSocket {
    sock: UdpSocket,
    buf: Vec<u8>,
}

impl CommSocket {
    /// Bind every interface on `port`, as `0x560550` does with `sin_addr = INADDR_ANY`. Port 0 asks
    /// the OS for a free one, which is what the tests use.
    pub fn bind(port: u16) -> io::Result<Self> {
        Self::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)))
    }

    pub fn bind_on(addr: SocketAddr) -> io::Result<Self> {
        let sock = UdpSocket::bind(addr)?;
        // setsockopt(SOL_SOCKET, SO_BROADCAST, 1) at 0x560580 — the IPX path sets it too
        sock.set_broadcast(true)?;
        sock.set_nonblocking(true)?;
        Ok(Self { sock, buf: vec![0u8; MAX_DATAGRAM] })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// The 20-byte record the protocol carries for this socket — what `[desc+0x44]` copies out of
    /// `sock+0x08` (0x5878F0). `Ipv4Addr::UNSPECIFIED` stays as it is; a caller that needs a
    /// routable address puts one in itself, exactly as the original relies on `getsockname`.
    pub fn local_record(&self) -> io::Result<Addr> {
        match self.local_addr()? {
            SocketAddr::V4(v4) => Ok(Addr::ip(v4.ip().octets(), v4.port())),
            SocketAddr::V6(_) => Err(io::Error::new(io::ErrorKind::Unsupported, "the protocol has no IPv6 address record")),
        }
    }

    /// `0x587950` with a non-null address: one tag byte, then the payload.
    pub fn send_to(&self, to: SocketAddr, tag: u8, payload: &[u8]) -> io::Result<usize> {
        let d = Datagram::encode(tag, payload).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        self.sock.send_to(&d, to)
    }

    /// `0x587950` with a null address: the sender thread builds the broadcast address from the
    /// socket's own port (0x5872C0), so the caller says which port to shout at.
    pub fn broadcast(&self, port: u16, tag: u8, payload: &[u8]) -> io::Result<usize> {
        self.send_to(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, port)), tag, payload)
    }

    /// One datagram, or `None` when nothing is waiting. A datagram that does not fit the original's
    /// 282-byte buffer could not have come from it and is dropped rather than truncated.
    pub fn recv(&mut self) -> io::Result<Option<(SocketAddr, Datagram)>> {
        loop {
            match self.sock.recv_from(&mut self.buf) {
                Ok((n, from)) => match Datagram::decode(&self.buf[..n]) {
                    Ok(d) => return Ok(Some((from, d))),
                    Err(_) => continue, // empty or oversized: not ours
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                // an ICMP port-unreachable comes back as an error on the *next* call on some
                // platforms; it says nothing about this socket, so keep draining
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// The largest payload a caller may hand [`Self::send_to`].
    pub const fn max_payload() -> usize {
        MAX_PAYLOAD
    }
}

/// The wire record for a `SocketAddr`, or `None` for IPv6 — the protocol has no room for one.
pub fn record_of(addr: SocketAddr) -> Option<Addr> {
    match addr {
        SocketAddr::V4(v4) => Some(Addr::ip(v4.ip().octets(), v4.port())),
        SocketAddr::V6(_) => None,
    }
}

/// The inverse: where to send a datagram that carries this record.
pub fn addr_of(record: &Addr) -> Option<SocketAddr> {
    let ip = record.ipv4()?;
    Some(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), record.port())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(port: u16) -> CommSocket {
        CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))).unwrap()
    }

    #[test]
    fn a_datagram_crosses_the_loopback_with_its_tag() {
        let a = local(0);
        let mut b = local(0);
        let to = b.local_addr().unwrap();
        a.send_to(to, b'=', &[]).unwrap();
        a.send_to(to, b'p', &[1, 2, 3]).unwrap();

        let (from, d) = wait(&mut b);
        assert_eq!(d.tag, b'=');
        assert!(d.payload.is_empty());
        assert_eq!(from, a.local_addr().unwrap());

        let (_, d) = wait(&mut b);
        assert_eq!((d.tag, d.payload), (b'p', vec![1, 2, 3]));
        assert!(b.recv().unwrap().is_none());
    }

    #[test]
    fn the_biggest_datagram_the_original_reads_still_fits() {
        let a = local(0);
        let mut b = local(0);
        let to = b.local_addr().unwrap();
        a.send_to(to, b'd', &vec![0x5A; CommSocket::max_payload()]).unwrap();
        let (_, d) = wait(&mut b);
        assert_eq!(d.payload.len(), 281);
        assert!(a.send_to(to, b'd', &vec![0; CommSocket::max_payload() + 1]).is_err());
    }

    #[test]
    fn a_record_round_trips_through_a_socket_address() {
        let s = local(0);
        let a = s.local_addr().unwrap();
        let r = record_of(a).unwrap();
        assert_eq!(r.ipv4(), Some([127, 0, 0, 1]));
        assert_eq!(addr_of(&r), Some(a));
    }

    fn wait(s: &mut CommSocket) -> (SocketAddr, Datagram) {
        for _ in 0..500 {
            if let Some(x) = s.recv().unwrap() {
                return x;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("nothing arrived");
    }
}
