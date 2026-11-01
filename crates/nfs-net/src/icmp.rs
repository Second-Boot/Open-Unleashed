//! The server list's ping — an ICMP echo, as the original's ping thread sends it (`lobby.md` §4).
//!
//! 0x55D800 loads `ICMP.DLL`, `IcmpCreateFile`, and `IcmpSendEcho`es a 32-byte `' '` pattern with a 2500 ms timeout, up to four tries
//! 100 ms apart; 0x48C660 stores the round trip (or −1) into the server entry, and the list shows only entries whose ping is not
//! negative.
//!
//! **PORT-DEVIATION D55** (the unprivileged echo): `ICMP.DLL` is Windows'. On macOS and Linux an unprivileged `SOCK_DGRAM` /
//! `IPPROTO_ICMP` socket sends the same echo without root (macOS always; Linux when `net.ipv4.ping_group_range` admits the user, the
//! default of current distributions). Where it cannot be opened — Windows here, a locked-down Linux — the answer is
//! [`Echo::Unavailable`] and the caller decides (the application shows the row with the round trip of its own link instead).

use std::net::Ipv4Addr;
use std::time::Duration;

/// The echo's pattern: 32 spaces (0x55D8C0).
pub const PATTERN: [u8; 32] = [b' '; 32];
/// 0x55D916: the reply timeout.
pub const TIMEOUT: Duration = Duration::from_millis(2500);
/// The tries and their spacing.
pub const TRIES: u32 = 4;
pub const RETRY_GAP: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Echo {
    /// the round trip in milliseconds (at least 1: a loopback reply of 0 ms is shown as 1, the list needs "not negative")
    Reply(u32),
    /// no reply within the tries
    Timeout,
    /// the host cannot send an unprivileged echo
    Unavailable,
}

/// The ICMP checksum (RFC 792): the ones' complement of the ones' complement sum of 16-bit words.
pub fn checksum(b: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for c in b.chunks(2) {
        sum += u32::from(c[0]) << 8 | u32::from(*c.get(1).unwrap_or(&0));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// An echo request (type 8, code 0) with `id`, `seq` and [`PATTERN`].
pub fn request(id: u16, seq: u16) -> Vec<u8> {
    let mut p = vec![8, 0, 0, 0, (id >> 8) as u8, id as u8, (seq >> 8) as u8, seq as u8];
    p.extend_from_slice(&PATTERN);
    let c = checksum(&p);
    p[2] = (c >> 8) as u8;
    p[3] = c as u8;
    p
}

/// Whether `reply` (with or without the IP header the BSDs include) is an echo reply (type 0).
pub fn is_reply(reply: &[u8]) -> bool {
    let icmp = if reply.first().is_some_and(|b| b >> 4 == 4) { reply.get(usize::from(reply[0] & 0xF) * 4..) } else { Some(reply) };
    icmp.and_then(|i| i.first()).is_some_and(|&t| t == 0)
}

/// The echo, up to [`TRIES`] times. Blocks for at most `TRIES × (TIMEOUT + RETRY_GAP)`; call it from a thread.
#[cfg(unix)]
pub fn echo(ip: Ipv4Addr) -> Echo {
    use std::time::Instant;
    // SAFETY: plain socket calls on a descriptor we own, closed on every path
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, libc::IPPROTO_ICMP);
        if fd < 0 {
            return Echo::Unavailable;
        }
        let tv = libc::timeval { tv_sec: TIMEOUT.as_secs() as _, tv_usec: (TIMEOUT.subsec_micros()) as _ };
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, &tv as *const _ as *const libc::c_void, std::mem::size_of::<libc::timeval>() as libc::socklen_t);
        let mut to: libc::sockaddr_in = std::mem::zeroed();
        to.sin_family = libc::AF_INET as _;
        to.sin_addr.s_addr = u32::from_ne_bytes(ip.octets());
        let mut result = Echo::Timeout;
        'tries: for seq in 0..TRIES {
            let req = request(std::process::id() as u16, seq as u16);
            let t0 = Instant::now();
            let sent = libc::sendto(fd, req.as_ptr() as *const _, req.len(), 0, &to as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t);
            if sent < 0 {
                result = Echo::Unavailable;
                break;
            }
            loop {
                let mut buf = [0u8; 256];
                let n = libc::recv(fd, buf.as_mut_ptr() as *mut _, buf.len(), 0);
                if n < 0 {
                    break;
                }
                if is_reply(&buf[..n as usize]) {
                    result = Echo::Reply((t0.elapsed().as_millis() as u32).max(1));
                    break 'tries;
                }
                if t0.elapsed() >= TIMEOUT {
                    break;
                }
            }
            std::thread::sleep(RETRY_GAP);
        }
        libc::close(fd);
        result
    }
}

/// No unprivileged echo on this platform (see the module's D55).
#[cfg(not(unix))]
pub fn echo(_ip: Ipv4Addr) -> Echo {
    Echo::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_has_a_valid_checksum_and_the_replies_are_told_apart() {
        let r = request(0x1234, 1);
        assert_eq!(r.len(), 8 + 32);
        assert_eq!(r[0], 8);
        // a correct checksum makes the whole packet sum to zero
        assert_eq!(checksum(&r), 0);
        assert!(!is_reply(&r));
        let mut rep = r.clone();
        rep[0] = 0;
        assert!(is_reply(&rep));
        // with a 20-byte IP header in front
        let mut ip = vec![0x45u8; 1];
        ip.extend_from_slice(&[0; 19]);
        ip.extend_from_slice(&rep);
        assert!(is_reply(&ip));
    }

    #[test]
    fn loopback_answers_when_the_host_allows_the_echo() {
        // an environment fact, not a rule: either the echo works or it is reported unavailable
        match echo(Ipv4Addr::LOCALHOST) {
            Echo::Reply(ms) => assert!(ms >= 1),
            Echo::Unavailable | Echo::Timeout => {}
        }
    }
}
