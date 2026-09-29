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
    /// **TEST GLUE, not in the original.** `NFS_NET_DROP=<percent>` makes `send_to` drop that share
    /// of the game's data frames (tag `'='` with a payload) — a lossy link for the reliable layer's
    /// proofs (`network.md` §11.18). `NFS_NET_DROP_SEED` seeds the generator (default: the port).
    drop_percent: u32,
    drop_state: std::cell::Cell<u32>,
    dropped: std::cell::Cell<u32>,
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
        let drop_percent = std::env::var("NFS_NET_DROP").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0).min(100);
        let seed = std::env::var("NFS_NET_DROP_SEED").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or_else(|| sock.local_addr().map(|a| a.port() as u32).unwrap_or(1));
        Ok(Self {
            sock,
            buf: vec![0u8; MAX_DATAGRAM],
            drop_percent,
            drop_state: std::cell::Cell::new(seed.wrapping_mul(2654435761).max(1)),
            dropped: std::cell::Cell::new(0),
        })
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
        if self.drop_percent > 0 && tag == b'=' && payload.len() >= 6 {
            let mut x = self.drop_state.get();
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            self.drop_state.set(x);
            if x % 100 < self.drop_percent {
                self.dropped.set(self.dropped.get() + 1);
                return Ok(payload.len() + 1);
            }
        }
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

    /// **TEST GLUE.** The same as `NFS_NET_DROP` / `NFS_NET_DROP_SEED`, set from a test.
    pub fn set_drop(&mut self, percent: u32, seed: u32) {
        self.drop_percent = percent.min(100);
        self.drop_state.set(seed.wrapping_mul(2654435761).max(1));
    }

    /// How many data frames the test glue has dropped so far.
    pub fn dropped(&self) -> u32 {
        self.dropped.get()
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

/// One place a `'w'` probe is sent, and why it is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// What the original does: `0x587950` with a null address, which the sender thread turns into
    /// `sin_addr = 0xFFFFFFFF` on the socket's own port (0x5872C0: `mov [esi+4], 0xffffffff`).
    Original,
    /// **Platform glue, not in the original.** The directed broadcast of one network interface
    /// (`address | !netmask`). The all-ones address leaves through the *default route's* interface
    /// only, so with a VPN or a second adapter holding the default route the probe never reaches
    /// the LAN; Windows 98/2000's WinSock (the original's) spread it over the interfaces itself.
    Interface,
    /// **Test/platform glue, not in the original.** Two copies of the port on one machine: the
    /// original never has to find a host on its own machine through the network.
    Loopback,
}

/// The addresses a probe goes to, in the order they are sent.
///
/// `NFS_NET_PROBE` (test glue) narrows the list: `original` keeps only the 255.255.255.255 the
/// original sends, so a test can show the literal path works; `interfaces` keeps only the
/// directed broadcasts; unset keeps everything.
pub fn probe_targets(port: u16) -> Vec<(ProbeKind, SocketAddr)> {
    let only = std::env::var("NFS_NET_PROBE").unwrap_or_default();
    let mut v = Vec::new();
    let mut push = |k: ProbeKind, ip: Ipv4Addr| {
        let a = SocketAddr::V4(SocketAddrV4::new(ip, port));
        if !v.iter().any(|(_, x)| *x == a) {
            v.push((k, a));
        }
    };
    if only.is_empty() || only == "original" {
        push(ProbeKind::Original, Ipv4Addr::BROADCAST);
    }
    if only.is_empty() || only == "interfaces" {
        for ip in lan_broadcasts() {
            push(ProbeKind::Interface, ip);
        }
    }
    if only.is_empty() || only == "loopback" {
        push(ProbeKind::Loopback, Ipv4Addr::LOCALHOST);
    }
    v
}

/// `address | !netmask`, the broadcast address of a subnet.
pub fn directed_broadcast(addr: Ipv4Addr, mask: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(u32::from(addr) | !u32::from(mask))
}

/// Every up, broadcast-capable, non-loopback, non-link-local IPv4 interface as `(address, mask)`.
/// Platform glue (see [`ProbeKind::Interface`]); empty where `getifaddrs` is not available.
#[cfg(unix)]
pub fn interfaces() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    let mut out = Vec::new();
    // SAFETY: `getifaddrs` hands back a linked list that `freeifaddrs` releases; every pointer
    // read below is checked for null first and only read while the list is alive.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            let flags = ifa.ifa_flags as i32;
            let usable = flags & libc::IFF_UP != 0 && flags & libc::IFF_BROADCAST != 0 && flags & libc::IFF_LOOPBACK == 0;
            if usable && !ifa.ifa_addr.is_null() && !ifa.ifa_netmask.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let ip = Ipv4Addr::from(u32::from_be((*(ifa.ifa_addr as *const libc::sockaddr_in)).sin_addr.s_addr));
                let mask = Ipv4Addr::from(u32::from_be((*(ifa.ifa_netmask as *const libc::sockaddr_in)).sin_addr.s_addr));
                if !ip.is_link_local() && !out.contains(&(ip, mask)) {
                    out.push((ip, mask));
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(head);
    }
    out
}

#[cfg(not(unix))]
pub fn interfaces() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    Vec::new()
}

/// The directed broadcast address of every [`interfaces`] entry.
pub fn lan_broadcasts() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for (ip, mask) in interfaces() {
        let b = directed_broadcast(ip, mask);
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// This machine's own address as the game's lobby shows it (`Comms.Data(MYIPADDRESS)`, 0x50BF09:
/// the room's string at `+0x50`). The original builds that string at 0x583899…0x5838C6:
/// `gethostname` (0x59518C), `gethostbyname` (0x587BDE), and `inet_ntoa` (0x587BEA) of the first
/// address of an `AF_INET` answer — dotted decimal, no port.
///
/// The name lookup is the original's. **Platform glue** on top of it: a modern OS often resolves its
/// own host name to the loopback (or not at all), which tells the person who has to type this on
/// the other machine nothing; in that case the first LAN interface's address is shown instead.
pub fn my_ip_string() -> String {
    let named = hostname().and_then(|h| {
        use std::net::ToSocketAddrs;
        (h.as_str(), 0u16).to_socket_addrs().ok()?.find_map(|a| match a {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            SocketAddr::V6(_) => None,
        })
    });
    match named {
        Some(ip) if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() => ip.to_string(),
        _ => interfaces().first().map(|(ip, _)| ip.to_string()).or_else(|| named.map(|ip| ip.to_string())).unwrap_or_default(),
    }
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is 256 bytes and `gethostname` writes at most that many, NUL-terminated
    // unless it truncates, which the last byte below covers.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    (r == 0).then(|| String::from_utf8_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len() - 1)]).into_owned())
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    None
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

    #[test]
    fn a_directed_broadcast_is_the_address_with_the_host_bits_set() {
        assert_eq!(directed_broadcast(Ipv4Addr::new(192, 168, 0, 82), Ipv4Addr::new(255, 255, 255, 0)), Ipv4Addr::new(192, 168, 0, 255));
        assert_eq!(directed_broadcast(Ipv4Addr::new(10, 1, 2, 3), Ipv4Addr::new(255, 255, 0, 0)), Ipv4Addr::new(10, 1, 255, 255));
    }

    #[test]
    fn the_probe_always_includes_the_originals_own_broadcast_first() {
        let t = probe_targets(1106);
        assert_eq!(t[0], (ProbeKind::Original, SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, 1106))));
        assert!(t.iter().all(|(_, a)| a.port() == 1106));
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
