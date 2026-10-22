//! The voice chat's message (`RE/notes/network.md` §11.40, `RE/notes/voice-codec.md`): **72 bytes**,
//! `0xC1`, `0x00`, then five 14-byte packets of the sound library's 8 kHz codec (100 ms of speech).
//!
//! The sender 0x493350 keeps the message in the voice object (`[net+0x14]`): `+0x29 = 0xC1`,
//! `+0x2A = 0` (the ctor 0x4931C0, 0x49321D / 0x493221), the packets copied to `+0x2B + 14·i`
//! (0x493421) and, at the fifth, the 72 bytes from `+0x29` handed to slot 0 of the message
//! queue `[[voice+0x18]]` = `[[net+0xC]]` = the queue `[net+8]` (0x48AD70, 0x49343A…0x493445) —
//! the reliable road every room message takes; the host's room relays it to everyone but the
//! sender (0x48C0F0). The receiver 0x48F76C (the cases `0xC1` and `0xC2` of 0x48F060) hands the
//! message to 0x493270, which copies the five packets from `+2` (0x493291) whatever the length.
//! Nothing sends `0xC2`.

/// The type byte the voice object sends (0x49321D).
pub const VOICE: u8 = 0xC1;
/// The other type 0x48F76C takes the same way (no sender).
pub const VOICE_ALT: u8 = 0xC2;
/// Packets per message (0x493432 `cmp eax, 5`).
pub const PACKETS: usize = 5;
/// One codec packet (`0x58AD52`).
pub const PACKET: usize = 14;
/// The message (0x49343A `push 0x48`).
pub const LEN: usize = 2 + PACKETS * PACKET;

/// The message for five packets, as 0x493350 sends it.
pub fn message(packets: &[[u8; PACKET]; PACKETS]) -> [u8; LEN] {
    let mut m = [0u8; LEN];
    m[0] = VOICE;
    m[1] = 0;
    for (i, p) in packets.iter().enumerate() {
        m[2 + i * PACKET..2 + (i + 1) * PACKET].copy_from_slice(p);
    }
    m
}

/// The five packets of a received `0xC1` / `0xC2` (0x493270). The original copies 70 bytes from
/// `+2` of whatever arrived; the port takes only a message that has them (a shorter one would be
/// read past its end there).
pub fn packets(m: &[u8]) -> Option<[[u8; PACKET]; PACKETS]> {
    if !matches!(m.first(), Some(&VOICE) | Some(&VOICE_ALT)) || m.len() < LEN {
        return None;
    }
    let mut out = [[0u8; PACKET]; PACKETS];
    for (i, p) in out.iter_mut().enumerate() {
        p.copy_from_slice(&m[2 + i * PACKET..2 + (i + 1) * PACKET]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_is_c1_00_and_five_packets() {
        let mut ps = [[0u8; PACKET]; PACKETS];
        for (i, p) in ps.iter_mut().enumerate() {
            for (j, b) in p.iter_mut().enumerate() {
                *b = (i * 16 + j) as u8;
            }
        }
        let m = message(&ps);
        assert_eq!(m.len(), 72);
        assert_eq!(&m[..4], &[0xC1, 0x00, 0x00, 0x01]);
        assert_eq!(m[2 + 14], 0x10);
        assert_eq!(m[71], 0x4D);
        assert_eq!(packets(&m), Some(ps));
    }

    #[test]
    fn c2_reads_the_same_and_a_short_or_other_message_does_not() {
        let mut m = message(&[[7u8; PACKET]; PACKETS]).to_vec();
        m[0] = VOICE_ALT;
        assert_eq!(packets(&m), Some([[7u8; PACKET]; PACKETS]));
        assert_eq!(packets(&m[..71]), None);
        m[0] = 0xC0;
        assert_eq!(packets(&m), None);
        // a longer datagram: the first 70 bytes after the head
        let mut long = message(&[[9u8; PACKET]; PACKETS]).to_vec();
        long.push(0xAA);
        assert_eq!(packets(&long), Some([[9u8; PACKET]; PACKETS]));
    }
}
