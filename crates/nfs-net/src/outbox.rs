//! The send queue in front of the reliable pool — the object `0x48AA10` builds (vtable **0x5B4968**,
//! `net+8`), stop-and-wait (`RE/notes/network.md` §11.32).
//!
//! Every game message a machine sends through the message builder `0x48C870` ends in slot 0 of this
//! object, **0x48AD70**; the builder object's first dword (`[builder]`, `net+0xC`, made at 0x48E2BF /
//! 0x48E45A / 0x48E62B: eight bytes, `[+0] = [net+8]`) is the queue (`0x48C938 mov ecx, [ebx]` /
//! `0x48C945 mov eax, [ecx]` / `call [eax]`; the pseudo-C's `(****this)` has one dereference too many).
//! The only other callers of the layer's send, 0x55D4A0 (whose callers are 0x48AD70 and 0x48AE30
//! alone), are these two: a client's reliable messages all go through the queue. The unreliable sends
//! (the input sample, the `0xBE` state: 0x55D4D0 from 0x48ED57 / 0x48EE7A) and the host's per-link
//! sends (0x55C270 from 0x48A45D / 0x48A519 / 0x48B959, 0x55C2B0) do not.
//!
//! ```text
//! +0x04 the link (0x55C370's result)     +0x08 the lock         +0x0C the list (0x48A920, 0x3D dwords a node)
//! +0x10 "idle": the next registration may send at once (starts 1, 0x48AA10)
//! +0x11 "enabled": the pump may send (starts 0; set by the link's event 4, 0x48FBB0, and by 0x48B210)
//! ```
//!
//! * **0x48AD70 (register)**: copy the message into a node, append it at the tail (0x48A920); then,
//!   only while `+0x10` is set, send the **head** through 0x55D4A0 (nothing unless the link is
//!   connected, `conn+0x2C8 & 1`) and, when that returned nonzero, erase the head (0x4C3150 the first
//!   node, 0x48A8D0 the erase); `+0x10 = 0` **either way**.
//! * **0x48AE30 (pump, once per 1000 units of the `GetTickCount() << 5` clock — 32 a second — from
//!   0x48EE90 at 0x48EFD3, and from the room's own tick 0x48B4F0 at 0x48B52B)**: with the list
//!   non-empty *and* `+0x11` set, send the head again (erased only on a nonzero return), `+0x10 = 0`,
//!   answer **1**; otherwise `+0x10 = 1` and answer **0**. 0x48EE90 acts on the 0: while the answer is
//!   0 and the connection is up it sends one file chunk (0x48CCC0).
//!
//! So a burst leaves one message per pump after the first, and a refused send (an empty pool) keeps
//! the message at the head. Teardown 0x48E9C0 → 0x48AED0 frees the list: what is still queued is lost.

use std::collections::VecDeque;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct OutboxStats {
    /// messages registered (0x48AD70 calls)
    pub queued: u64,
    /// heads the layer took (0x55D4A0 answered nonzero) — from the registration
    pub sent_at_once: u64,
    /// heads the layer took — from the pump
    pub sent_by_pump: u64,
    /// sends the layer refused (0x55D4A0 answered 0: no free packet, or not connected); the message stays
    pub refused: u64,
    /// the most messages waiting at any time
    pub deepest: usize,
}

#[derive(Debug, Clone)]
pub struct Outbox {
    list: VecDeque<Vec<u8>>,
    /// `+0x10`
    idle: bool,
    /// `+0x11`
    enabled: bool,
    pub stats: OutboxStats,
}

impl Default for Outbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Outbox {
    /// 0x48AA10: `+0x10 = 1`, `+0x11 = 0`, an empty list.
    pub fn new() -> Self {
        Self { list: VecDeque::new(), idle: true, enabled: false, stats: OutboxStats::default() }
    }

    /// The link's event 4 (0x48FBB0: `[[net+8] + 0x11] = 1`).
    pub fn enable(&mut self) {
        self.enabled = true;
    }

    /// Messages waiting.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Teardown 0x48AED0: the list is freed with the object.
    pub fn clear(&mut self) {
        self.list.clear();
    }

    /// **0x48AD70.** `send` is 0x55D4A0: whether the layer took the message.
    pub fn register(&mut self, body: &[u8], send: &mut dyn FnMut(&[u8]) -> bool) {
        self.list.push_back(body.to_vec());
        self.stats.queued += 1;
        self.stats.deepest = self.stats.deepest.max(self.list.len());
        if self.idle {
            if self.send_head(send) {
                self.stats.sent_at_once += 1;
            }
            self.idle = false;
        }
    }

    /// **0x48AE30.** Answers what the original does: `true` (1) when it sent (or tried to send) the
    /// head, `false` (0) when the list was empty or the queue is not enabled yet.
    pub fn pump(&mut self, send: &mut dyn FnMut(&[u8]) -> bool) -> bool {
        if !self.list.is_empty() && self.enabled {
            if self.send_head(send) {
                self.stats.sent_by_pump += 1;
            }
            self.idle = false;
            true
        } else {
            self.idle = true;
            false
        }
    }

    fn send_head(&mut self, send: &mut dyn FnMut(&[u8]) -> bool) -> bool {
        let Some(head) = self.list.front() else { return false };
        if send(head) {
            self.list.pop_front();
            true
        } else {
            self.stats.refused += 1;
            false
        }
    }
}

/// The **host's** send queue of one peer record (the 0xFC-byte record 0x48A570 builds, `+0xB4` the count, `+0xBA`
/// "in", `+0xBE` "idle", `+0xC4` the list) — 0x48A3D0 registers, 0x48A4E0 pumps; the room's send slot 0
/// **0x48B340** is what the message builder reaches for a peer's link, and the room's 32 Hz tick **0x48B4F0**
/// (0x48B54D) pumps every record whose registration answered 1 (a record enters the room's list `+0x18` when it
/// stops being idle, and 0x48B660 removes it again once its pump answers an empty queue). Stop-and-wait like
/// [`Outbox`], with two differences read off the disassembly (`RE/notes/network.md` §11.33):
///
/// ```text
/// 0x48A3D0 register(buf, len, link)            idle (+0xBE) starts 1
///   node appended at the tail, count++ (0x48A927)
///   if idle: if link != 0: r = 0x55C270(link, peer, head)  ; head erased, count-- when r != 0
///            +0xBE = 0                                      ; either way
///   (not idle: nothing sent)
/// 0x48A4E0 pump(link)                          once per 32 Hz room tick, for the listed records
///   if list non-empty and +0xBA (in) and peer != 0:
///        r = 0x55C270(link, peer, head)         ; head erased, count-- when r != 0
///        +0xBE = 0                              ; either way
///   if count == 0: +0xBE = 1                    ; (unlike the client's, an unsent head leaves +0xBE alone)
/// ```
///
/// A refused send (an empty pool) keeps the head and is retried at the next tick — the original's room
/// senders never drop a message, and never send a second before the first left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerQueue {
    list: VecDeque<Vec<u8>>,
    /// `+0xBE`
    idle: bool,
    pub stats: OutboxStats,
}

impl Default for PeerQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerQueue {
    /// 0x48A570: `+0xB4 = 0`, `+0xBE = 1`, an empty list.
    pub fn new() -> Self {
        Self { list: VecDeque::new(), idle: true, stats: OutboxStats::default() }
    }

    /// Messages waiting (`+0xB4`).
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// **0x48A3D0.** `send` is 0x55C270 on the peer's link: whether the layer took the message.
    pub fn register(&mut self, body: &[u8], send: &mut dyn FnMut(&[u8]) -> bool) {
        self.list.push_back(body.to_vec());
        self.stats.queued += 1;
        self.stats.deepest = self.stats.deepest.max(self.list.len());
        if self.idle {
            if self.send_head(send) {
                self.stats.sent_at_once += 1;
            }
            self.idle = false;
        }
    }

    /// **0x48A4E0**, `in_room` = `+0xBA`.
    pub fn pump(&mut self, in_room: bool, send: &mut dyn FnMut(&[u8]) -> bool) {
        if !self.list.is_empty() && in_room {
            if self.send_head(send) {
                self.stats.sent_by_pump += 1;
            }
            self.idle = false;
        }
        if self.list.is_empty() {
            self.idle = true;
        }
    }

    fn send_head(&mut self, send: &mut dyn FnMut(&[u8]) -> bool) -> bool {
        let Some(head) = self.list.front() else { return false };
        if send(head) {
            self.list.pop_front();
            true
        } else {
            self.stats.refused += 1;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_leaves_one_message_per_pump_after_the_first() {
        let mut q = Outbox::new();
        q.enable();
        let mut wire = Vec::new();
        for i in 0..4u8 {
            q.register(&[i], &mut |b| {
                wire.push(b[0]);
                true
            });
        }
        // the first went at once (`+0x10` was set), the other three wait
        assert_eq!(wire, vec![0]);
        assert_eq!(q.len(), 3);
        for want in 1..=3u8 {
            assert!(q.pump(&mut |b| {
                wire.push(b[0]);
                true
            }));
            assert_eq!(*wire.last().unwrap(), want);
        }
        // the list is empty: the pump answers 0 and sets `+0x10`, so the next message goes at once
        assert!(!q.pump(&mut |_| unreachable!()));
        q.register(&[9], &mut |b| {
            wire.push(b[0]);
            true
        });
        assert_eq!(wire, vec![0, 1, 2, 3, 9]);
    }

    #[test]
    fn a_refused_send_keeps_the_head_and_clears_the_idle_flag() {
        let mut q = Outbox::new();
        q.enable();
        q.register(&[1], &mut |_| false);
        assert_eq!(q.len(), 1);
        q.register(&[2], &mut |_| unreachable!("`+0x10` is clear"));
        assert_eq!(q.len(), 2);
        assert!(q.pump(&mut |_| false), "a refusal still answers 1");
        let mut got = Vec::new();
        assert!(q.pump(&mut |b| {
            got.push(b[0]);
            true
        }));
        assert_eq!(got, vec![1], "the head is sent again, in order");
        assert_eq!(q.stats.refused, 2);
    }

    #[test]
    fn nothing_is_pumped_until_the_link_says_connected() {
        let mut q = Outbox::new();
        q.register(&[1], &mut |_| false);
        q.register(&[2], &mut |_| unreachable!());
        assert!(!q.pump(&mut |_| unreachable!()), "`+0x11` is clear: answer 0, `+0x10 = 1`");
        // `+0x10` is set again, so a registration tries the head (the older message)
        let mut got = Vec::new();
        q.register(&[3], &mut |b| {
            got.push(b[0]);
            true
        });
        assert_eq!(got, vec![1]);
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn a_record_that_is_not_in_sends_nothing_from_the_pump() {
        // 0x48A4E0 needs `+0xBA`: the first message of a burst goes at once (0x48A3D0 does not look at it),
        // the rest wait for the record to be in
        let mut q = PeerQueue::new();
        let mut wire = Vec::new();
        for i in 0..3u8 {
            q.register(&[i], &mut |b| {
                wire.push(b[0]);
                true
            });
        }
        q.pump(false, &mut |_| unreachable!("not in"));
        q.pump(false, &mut |_| unreachable!("not in"));
        assert_eq!(wire, vec![0]);
        assert_eq!(q.len(), 2);
        q.pump(true, &mut |b| {
            wire.push(b[0]);
            true
        });
        assert_eq!(wire, vec![0, 1]);
    }

    #[test]
    fn a_hosts_peer_queue_sends_one_message_per_tick_and_retries_a_refusal() {
        let mut q = PeerQueue::new();
        let mut wire = Vec::new();
        // idle at the start: the first goes at once, the burst waits
        for i in 0..3u8 {
            q.register(&[i], &mut |b| {
                wire.push(b[0]);
                true
            });
        }
        assert_eq!(wire, vec![0]);
        assert_eq!(q.len(), 2);
        // a refusal keeps the head and does not make the queue idle
        q.pump(true, &mut |_| false);
        assert_eq!((q.len(), q.stats.refused), (2, 1));
        // not "in" (+0xBA clear): the pump sends nothing, and the record stays busy
        q.pump(false, &mut |_| unreachable!());
        for want in 1..=2u8 {
            q.pump(true, &mut |b| {
                wire.push(b[0]);
                true
            });
            assert_eq!(*wire.last().unwrap(), want);
        }
        assert!(q.is_empty());
        // the pump that finds it empty makes it idle again: the next registration goes at once
        q.pump(true, &mut |_| unreachable!());
        q.register(&[9], &mut |b| {
            wire.push(b[0]);
            true
        });
        assert_eq!(wire, vec![0, 1, 2, 9]);
    }
}
