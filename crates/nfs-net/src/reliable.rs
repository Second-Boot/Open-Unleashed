//! The reliable half of the data layer 0x5E2E58 — sequencing, acknowledgement, retransmission
//! (`RE/notes/network.md` §11.18; the frame's own layout is [`crate::data`], §7.12c).
//!
//! One [`Conn`] is the original's connection object `conn` (`link+0x14`, built by 0x5951A0), one
//! per peer. Every offset below is that object's. Times are ticks of the 128 Hz timer
//! (`TIMER_FREQ = [0x5DEB48]`, `timing.md` §2) — the unit 0x53C2E0 (`[0x6B7C7C]`) counts in.
//!
//! **Send, reliable — 0x595C40 → 0x595D10 / 0x595EB0.** A packet comes off the free list `conn+0x60`
//! (seven of them: `(1 << 4) / 2 − 1`, the low byte of the layer flag `0x8004`, 0x5951A0); an empty
//! list makes the send **return 0** (0x595C58). The header takes the sequence `conn+0x5A` (starts 1,
//! wraps 0xFFFB → 0), the ack `conn+0x5E`, and up to two of the **newest still-unacknowledged**
//! messages ride along (0x595D10's loop: `count − 1` downwards from the list `conn+0x7C`, delta =
//! low byte of the sequences' difference). The frame is stored on `conn+0x7C` and sent
//! (0x595EB0: sent-time `pkt+4 = now`, `conn+0x28 = 0`, `conn+0x2C = TIMER_FREQ`).
//!
//! **Send, unreliable** (`len | 0x80000000`, 0x55D4E7): an `alloca` frame with sequence `0xFFFE`, no
//! extras, not stored — but through the same 0x595EB0, so it carries the ack and resets the ack
//! timer as well.
//!
//! **Receive — 0x595740.** The ack (`+0`) first (0x595950). A frame of length 0 is an ack alone
//! (`0xFFFD`; `0xFFFF` closes). Sequence `< 0xFFFC`: the message must be one of the seven the
//! receive list `conn+0x9C` holds (the window `conn+0x5E + 1 … + 7`); otherwise it is **dropped**
//! (`conn+0x44++`, 0x595800 — a duplicate or too far ahead). 0x595B50: `seq == conn+0x5E + 1` is
//! delivered (`conn+0x5E = seq`, `conn+0x4C++`, and `conn+0x28++` with `conn+0x2C = TIMER_FREQ / 2`
//! while `conn+0x28 < 2`, else `−1`: the ack goes out at once), then every buffered successor
//! (`conn+0xB8`, ordered by sequence); any other in-window sequence is buffered. `0xFFFE` is
//! handed up at once, in whatever order it came.
//!
//! **Ack — 0x595950.** `0 < ack − conn+0x5C < conn+0x5A − conn+0x5C` (16-bit, signed after the
//! promotion — the original loses the layer 32 768 messages into a connection; the port does the
//! arithmetic modulo 0xFFFC): `conn+0x5C = ack`, the sent list is popped from the front up to and
//! including the sequence acked, and the round trip is measured on the **fastest** of them that was
//! not retransmitted (`pkt+0xC == 0`): the first sample `srtt = 8·m`, `rttvar = 2·m`; then
//! `m −= srtt >> 3; srtt += m; rttvar = rttvar − rttvar >> 2 + |m|`; `rto = (rttvar + srtt >> 3) ·
//! 1.25 · 1.5`, at least `(TIMER_FREQ + 19) / 20` and at most `60 · TIMER_FREQ`; `conn+0x3C = rto`
//! (initially `3 · TIMER_FREQ`, 0x5951A0).
//!
//! **The poll — 0x595F70.** When the oldest unacknowledged message has waited `rto`
//! (0x595AA0: `conn+0x30 = rto + pkt+4 − now`): the rto doubles (`≤ 60 · TIMER_FREQ`) and **that one
//! message** is sent again (0x5960E0: `pkt+0xC = 1`, `conn+0x48++`, 0x595EB0 stamps the current
//! ack over the stored one). Then the ack timer `conn+0x2C` (initially −1): an ack-only frame
//! (0x596080, sequence `0xFFFD`, the ack `conn+0x5E`) when it has run out, and it is rearmed to
//! `TIMER_FREQ` — a bare ack every second for the life of the connection.
//!
//! What the port adds, and says so, is in [`Conn::send_queued`] (the backlog).

use crate::data::SEQ_WRAP;
use std::collections::{BTreeMap, VecDeque};

/// `TIMER_FREQ`, `[0x5DEB48]` — `main` calls `TIMER_init(0x80)` (`timing.md`).
pub const TICK_HZ: i64 = 128;
/// The pool of packets (0x5951A0's `iVar6`): `(1 << 4) / 2 − 1` for the flag byte `0x04` of `0x8004`.
pub const POOL: usize = 7;
/// The receive window is the same seven.
pub const WINDOW: u16 = 7;
/// `0xFFFD`: the sequence of an ack-only frame (0x596080's call from 0x595F70 passes it).
pub const ACK_ONLY: u16 = 0xFFFD;
/// The counter runs 0 … 0xFFFB.
const MOD: u32 = SEQ_WRAP as u32 + 1;
/// `conn+0x0C`, the frame size the appends are checked against (0x595E1D: `conn+0xC <
/// frame_len + len + 2` stops the loop). Where it comes from (the `param_3` of 0x5951A0, raised to
/// `[link+0x1E]`) was not traced: **APPROXIMATION** — the largest payload a datagram carries after
/// its tag byte, 281 (`0x11A − 1`, `crate::wire::MAX_PAYLOAD`), since a frame longer than that
/// could not be sent at all. (A first version took 0x200: a frame with two long option records
/// appended was refused by the socket and never left — found by the two-instance run.)
pub const MTU: usize = crate::wire::MAX_PAYLOAD;

/// `(to − from) mod 0xFFFC`.
fn dist(from: u16, to: u16) -> u32 {
    (to as u32 + MOD - from as u32) % MOD
}

fn advance(seq: u16) -> u16 {
    crate::data::Frame::next_seq(seq)
}

#[derive(Debug, Clone)]
struct Sent {
    seq: u16,
    /// the whole frame, as 0x595D10 built it (`pkt+0xE`)
    frame: Vec<u8>,
    body: Vec<u8>,
    /// `pkt+4`
    at: i64,
    /// `pkt+0xC`
    retransmitted: bool,
}

/// Counters the original keeps on the connection, for the proofs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// `conn+0x48` — messages sent again on the timer
    pub retransmitted: u32,
    /// `conn+0x44` — reliable messages dropped for lying outside the window (duplicates included)
    pub dropped: u32,
    /// `conn+0x4C` — reliable messages delivered
    pub delivered: u32,
    /// sends refused for an empty pool (0x595C58 returning 0)
    pub refused: u32,
}

#[derive(Debug, Clone)]
pub struct Conn {
    /// `conn+0x5A`
    next_seq: u16,
    /// `conn+0x5C`
    acked: u16,
    /// `conn+0x5E`
    delivered: u16,
    /// `conn+0x7C`
    sent: VecDeque<Sent>,
    /// the free list `conn+0x60`
    free: usize,
    /// `conn+0x34`
    srtt: i32,
    /// `conn+0x38`
    rttvar: i32,
    /// `conn+0x3C`
    rto: i32,
    /// `conn+0x28`
    pending: u32,
    /// `conn+0x2C` as a deadline
    ack_due: i64,
    /// `conn+0xB8`
    buffered: BTreeMap<u16, Vec<u8>>,
    /// Port glue, see [`Conn::send_queued`].
    backlog: VecDeque<Vec<u8>>,
    pub stats: Stats,
}

impl Conn {
    /// 0x5951A0.
    pub fn new(now: i64) -> Self {
        Self {
            next_seq: 1,
            acked: 0,
            delivered: 0,
            sent: VecDeque::new(),
            free: POOL,
            srtt: 0,
            rttvar: 0,
            rto: (3 * TICK_HZ) as i32,
            pending: 0,
            // `conn+0x2C` starts at −1: the first poll sends an ack
            ack_due: now,
            buffered: BTreeMap::new(),
            backlog: VecDeque::new(),
            stats: Stats::default(),
        }
    }

    /// Would 0x595C40 take a reliable message now — is a packet free (0x595C58)?
    pub fn can_send(&self) -> bool {
        self.free > 0 && self.backlog.is_empty()
    }

    /// Packets free on the pool (`conn+0x60`), less the messages held back.
    pub fn room(&self) -> usize {
        if self.backlog.is_empty() { self.free } else { 0 }
    }

    /// Reliable messages sent and not acknowledged yet.
    pub fn in_flight(&self) -> usize {
        self.sent.len()
    }

    /// Messages the port is holding back (glue).
    pub fn queued(&self) -> usize {
        self.backlog.len()
    }

    /// `conn+0x5E` — the last sequence delivered in order, which every frame carries.
    pub fn ack(&self) -> u16 {
        self.delivered
    }

    /// 0x595EB0's bookkeeping, common to every frame that leaves.
    fn stamp(&mut self, now: i64) {
        self.pending = 0;
        self.ack_due = now + TICK_HZ;
    }

    /// **0x595C40, reliable.** `None` when the pool is empty (the original returns 0 and the message
    /// is gone); otherwise the frame to put on the wire.
    pub fn send(&mut self, body: &[u8], now: i64) -> Option<Vec<u8>> {
        if self.free == 0 {
            self.stats.refused += 1;
            return None;
        }
        self.free -= 1;
        let seq = self.next_seq;
        self.next_seq = advance(seq);
        let mut frame = Vec::with_capacity(crate::data::HEADER + body.len() + 16);
        frame.extend_from_slice(&self.delivered.to_be_bytes());
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.push(0); // `conn+0x58`
        frame.push(body.len() as u8);
        frame.extend_from_slice(body);
        // 0x595D10: the newest two unacknowledged, newest first
        for old in self.sent.iter().rev().take(2) {
            if MTU < frame.len() + old.body.len() + 2 {
                break;
            }
            frame.push(old.body.len() as u8);
            frame.push(dist(old.seq, seq) as u8);
            frame.extend_from_slice(&old.body);
        }
        self.stamp(now);
        self.sent.push_back(Sent { seq, frame: frame.clone(), body: body.to_vec(), at: now, retransmitted: false });
        Some(frame)
    }

    /// **0x595C40, unreliable:** sequence `0xFFFE`, nothing stored.
    pub fn send_unreliable(&mut self, body: &[u8], now: i64) -> Vec<u8> {
        let mut frame = Vec::with_capacity(crate::data::HEADER + body.len());
        frame.extend_from_slice(&self.delivered.to_be_bytes());
        frame.extend_from_slice(&crate::data::UNRELIABLE_SEQ.to_be_bytes());
        frame.push(0);
        frame.push(body.len() as u8);
        frame.extend_from_slice(body);
        self.stamp(now);
        frame
    }

    /// **PORT GLUE (APPROXIMATION).** A reliable send for a caller that has no way to react to the
    /// original's "returned 0": with the pool empty, or messages already waiting, the message is
    /// held (in order) and goes out from [`Conn::poll`] as the pool frees. The original's callers
    /// either check the return (the room's frames, 0x48B961 — those use [`Conn::can_send`] and
    /// never come here) or were not traced (the bursts of the launch stream); a queue keeps their
    /// messages where the original's pool would have dropped some. Direction: nothing is lost that
    /// the original delivers; a burst of more than seven is delivered late, not lost.
    pub fn send_queued(&mut self, body: &[u8], now: i64) -> Option<Vec<u8>> {
        if self.free == 0 || !self.backlog.is_empty() {
            self.backlog.push_back(body.to_vec());
            return None;
        }
        self.send(body, now)
    }

    /// **0x595740 / 0x595950 / 0x595B50.** One frame in; the messages it delivers, in order.
    pub fn receive(&mut self, b: &[u8], now: i64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if b.len() < crate::data::HEADER {
            return out;
        }
        let n = b[5] as usize;
        let end = crate::data::HEADER + n;
        if b.len() < end {
            return out;
        }
        self.on_ack(u16::from_be_bytes([b[0], b[1]]), now);
        if n == 0 {
            return out; // an ack alone (0xFFFD), or the close (0xFFFF), not read
        }
        let seq = u16::from_be_bytes([b[2], b[3]]);
        if seq == crate::data::UNRELIABLE_SEQ {
            out.push(b[crate::data::HEADER..end].to_vec());
            return out;
        }
        if seq > SEQ_WRAP {
            return out;
        }
        self.on_message(seq, &b[crate::data::HEADER..end], now, &mut out);
        // 0x595820: while `n + 8 < len` — at least three bytes more
        let mut at = end;
        while at + 3 <= b.len() {
            let n = b[at] as usize;
            let delta = b[at + 1] as u32;
            let from = at + 2;
            let to = from + n;
            if b.len() < to {
                break;
            }
            // 0x59586B: the frame's sequence less the delta, modulo 0xFFFC
            let s = (seq as u32 + MOD - delta % MOD) % MOD;
            self.on_message(s as u16, &b[from..to], now, &mut out);
            at = to;
        }
        out
    }

    fn on_message(&mut self, seq: u16, body: &[u8], now: i64, out: &mut Vec<Vec<u8>>) {
        let d = dist(self.delivered, seq);
        if d == 0 || d > WINDOW as u32 || self.buffered.contains_key(&seq) {
            self.stats.dropped += 1;
            return;
        }
        if d != 1 {
            self.buffered.insert(seq, body.to_vec());
            return;
        }
        // 0x595B50
        if self.pending < 2 {
            self.ack_due = now + TICK_HZ / 2;
        } else {
            self.ack_due = now;
        }
        self.pending += 1;
        self.delivered = seq;
        self.stats.delivered += 1;
        out.push(body.to_vec());
        loop {
            let next = advance(self.delivered);
            let Some(b) = self.buffered.remove(&next) else { break };
            self.delivered = next;
            self.stats.delivered += 1;
            out.push(b);
        }
    }

    /// 0x595950.
    fn on_ack(&mut self, ack: u16, now: i64) {
        if ack > SEQ_WRAP {
            return;
        }
        let d = dist(self.acked, ack);
        if d == 0 || d >= dist(self.acked, self.next_seq) {
            return;
        }
        self.acked = ack;
        let mut best: Option<i64> = None;
        while let Some(p) = self.sent.pop_front() {
            let m = now - p.at;
            if !p.retransmitted && best.is_none_or(|b| m < b) {
                best = Some(m);
            }
            self.free += 1;
            if p.seq == ack {
                break;
            }
        }
        if let Some(m) = best {
            let mut m = m as i32;
            let rttvar;
            if self.srtt == 0 {
                rttvar = m * 2;
                self.srtt = m * 8;
            } else {
                if m < 1 {
                    m = 1;
                }
                m -= self.srtt >> 3;
                self.srtt += m;
                rttvar = (self.rttvar - (self.rttvar >> 2)) + m.abs();
            }
            self.rttvar = rttvar;
            let mut rto = rttvar + (self.srtt >> 3);
            rto += rto >> 2;
            rto += rto >> 1;
            self.rto = rto.max(((TICK_HZ + 19) / 20) as i32).min((60 * TICK_HZ) as i32);
        }
    }

    /// **0x595F70**, one look: the frames due — the oldest unacknowledged message again, an
    /// ack-only frame — and (glue) the held-back messages the pool now has room for.
    pub fn poll(&mut self, now: i64) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if let Some(front) = self.sent.front()
            && now >= front.at + self.rto as i64
        {
            self.rto = (self.rto * 2).min((60 * TICK_HZ) as i32);
            let ack = self.delivered.to_be_bytes();
            let front = self.sent.front_mut().expect("checked");
            front.retransmitted = true;
            front.at = now;
            front.frame[0] = ack[0];
            front.frame[1] = ack[1];
            let frame = front.frame.clone();
            self.stats.retransmitted += 1;
            self.stamp(now);
            out.push(frame);
        }
        if now >= self.ack_due {
            // 0x596080
            let mut f = Vec::with_capacity(crate::data::HEADER);
            f.extend_from_slice(&self.delivered.to_be_bytes());
            f.extend_from_slice(&ACK_ONLY.to_be_bytes());
            f.push(0);
            f.push(0);
            self.ack_due = now + TICK_HZ;
            out.push(f);
        }
        while self.free > 0 {
            let Some(b) = self.backlog.pop_front() else { break };
            if let Some(f) = self.send(&b, now) {
                out.push(f);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two ends and a wire that loses what a rule says.
    struct Pair {
        a: Conn,
        b: Conn,
        now: i64,
        got_a: Vec<Vec<u8>>,
        got_b: Vec<Vec<u8>>,
        rng: u32,
    }

    impl Pair {
        fn new() -> Self {
            Self { a: Conn::new(0), b: Conn::new(0), now: 0, got_a: vec![], got_b: vec![], rng: 12345 }
        }
        fn lose(&mut self, percent: u32) -> bool {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 17;
            self.rng ^= self.rng << 5;
            self.rng % 100 < percent
        }
        fn to_b(&mut self, f: Vec<u8>, loss: u32) {
            if !self.lose(loss) {
                let d = self.b.receive(&f, self.now);
                self.got_b.extend(d);
            }
        }
        fn to_a(&mut self, f: Vec<u8>, loss: u32) {
            if !self.lose(loss) {
                let d = self.a.receive(&f, self.now);
                self.got_a.extend(d);
            }
        }
        fn step(&mut self, loss: u32) {
            self.now += 1;
            for f in self.a.poll(self.now) {
                self.to_b(f, loss);
            }
            for f in self.b.poll(self.now) {
                self.to_a(f, loss);
            }
        }
    }

    #[test]
    fn every_message_arrives_once_and_in_order_over_a_lossy_wire() {
        let mut p = Pair::new();
        let mut sent = 0u32;
        for tick in 0..20_000 {
            // 16 messages a second one way, an unreliable sample the other (which carries the acks)
            if tick % 8 == 0 && sent < 400 {
                if let Some(f) = p.a.send_queued(&sent.to_be_bytes(), p.now) {
                    p.to_b(f, 10);
                }
                sent += 1;
            }
            if tick % 4 == 0 {
                let f = p.b.send_unreliable(&[1, 2, 3], p.now);
                p.to_a(f, 10);
            }
            p.step(10);
        }
        let got: Vec<u32> = p.got_b.iter().map(|m| u32::from_be_bytes(m[..4].try_into().unwrap())).collect();
        assert_eq!(got, (0..400).collect::<Vec<u32>>(), "lost or reordered");
        assert!(p.a.stats.retransmitted > 0 || p.b.stats.dropped > 0);
    }

    #[test]
    fn the_eighth_unacknowledged_message_is_refused_and_the_queue_holds_it() {
        let mut c = Conn::new(0);
        for i in 0..POOL as u8 {
            assert!(c.send(&[i], 0).is_some());
        }
        assert!(!c.can_send());
        assert!(c.send(&[9], 0).is_none());
        assert_eq!(c.stats.refused, 1);
        assert!(c.send_queued(&[9], 0).is_none());
        assert_eq!(c.queued(), 1);
    }

    #[test]
    fn two_newest_unacknowledged_ride_along_and_repair_a_lost_frame() {
        let mut a = Conn::new(0);
        let mut b = Conn::new(0);
        let f1 = a.send(b"one", 0).unwrap();
        let _lost = a.send(b"two", 1).unwrap();
        let f3 = a.send(b"three", 2).unwrap();
        // f3 carries "two" and "one" (newest first)
        assert_eq!(f3.len(), 6 + 5 + 2 + 3 + 2 + 3);
        assert_eq!(b.receive(&f1, 3), vec![b"one".to_vec()]);
        // "three" is in the window but not next: buffered; "two" rides in the extras and releases both
        let got = b.receive(&f3, 4);
        assert_eq!(got, vec![b"two".to_vec(), b"three".to_vec()]);
        assert_eq!(b.ack(), 3);
    }

    #[test]
    fn an_append_that_would_pass_the_frame_size_is_left_off() {
        // 0x595E1D: `conn+0xC < frame_len + len + 2` ends the loop — a datagram holds 281 bytes
        let mut a = Conn::new(0);
        let big = vec![7u8; 200];
        let _ = a.send(&big, 0).unwrap();
        let f = a.send(&big, 1).unwrap();
        assert_eq!(f.len(), 6 + 200, "the first message does not fit twice, so nothing rides along");
        let small = a.send(b"x", 2).unwrap();
        assert!(small.len() <= MTU);
        assert_eq!(small.len(), 6 + 1 + 2 + 200, "one 200-byte message fits, the second does not");
    }

    #[test]
    fn a_duplicate_is_dropped_and_an_ack_frees_the_pool() {
        let mut a = Conn::new(0);
        let mut b = Conn::new(0);
        let f = a.send(b"x", 0).unwrap();
        assert_eq!(b.receive(&f, 1).len(), 1);
        assert!(b.receive(&f, 2).is_empty());
        assert_eq!(b.stats.dropped, 1);
        // b's next ack-only frame acknowledges it
        let acks = b.poll(1 + 64);
        assert_eq!(acks.len(), 1);
        assert_eq!(&acks[0][2..4], &ACK_ONLY.to_be_bytes());
        a.receive(&acks[0], 70);
        assert_eq!(a.in_flight(), 0);
        assert!(a.can_send());
    }

    #[test]
    fn the_oldest_is_sent_again_after_the_rto_and_the_rto_doubles() {
        let mut a = Conn::new(0);
        let _ = a.send(b"x", 0).unwrap();
        // the ack timer fires first at tick 0; use a busy poll from tick 1
        assert!(a.poll(1).iter().all(|f| f.len() == 6));
        let rto = 3 * TICK_HZ;
        assert!(a.poll(rto - 1).iter().all(|f| f.len() == 6), "not yet");
        let out = a.poll(rto);
        assert!(out.iter().any(|f| f.len() == 7 && f[5] == 1), "the stored frame again");
        assert_eq!(a.stats.retransmitted, 1);
        // now the timeout is twice as long
        assert!(a.poll(rto + 2 * rto - 1).iter().all(|f| f.len() == 6));
        assert_eq!(a.stats.retransmitted, 1);
        assert!(a.poll(rto + 2 * rto).iter().any(|f| f.len() == 7));
    }

    #[test]
    fn the_first_round_trip_sets_the_rto_from_the_original_formula() {
        let mut a = Conn::new(0);
        let f = a.send(b"x", 0).unwrap();
        let mut b = Conn::new(0);
        b.receive(&f, 0);
        let ack = b.poll(64).pop().unwrap();
        a.receive(&ack, 64);
        // m = 64: srtt = 512, rttvar = 128, rto = (128 + 64) * 1.25 * 1.5 -> 240 + ... (integer shifts)
        let mut r = 128 + 64;
        r += r >> 2;
        r += r >> 1;
        assert_eq!(a.rto, r);
    }
}
