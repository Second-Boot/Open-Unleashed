//! The session state machine — hosting, joining and keeping peers alive
//! (`RE/notes/network.md` §7.4 … §7.10).
//!
//! This is the original's own logic, not a design of mine: the slot mask at `session+0x50`, the
//! 100-byte player records from `session+0x88`, the two countdowns and the ping round are all
//! transcribed from 0x588A80's handlers, `0x588890`, `0x589440` and the poll `0x587CE0`.
//!
//! The game's own `0xB0 … 0xC6` messages ride the connection's data layer [`crate::data`] and go
//! out under the tag **`'='`** (§7.12c) — not under `'<'`, whose sender `0x5894C0` is dead code in
//! the original (H36, refuted). [`Net::send_game`] frames them the original's way and sends them
//! that way; a `'='` with an empty payload is still the keep-alive, exactly as the original tells
//! the two apart.

use crate::comm::{self, tag, CommError, Describe, Join, JoinReply, JoinStatus};
use crate::socket::{addr_of, record_of, CommSocket};
use crate::wire::Addr;
use std::io;
use std::net::SocketAddr;

/// `add ebx, 0x64` at 0x588D8 and 0x587DBD — one player record is 100 bytes.
pub const PLAYER_STRIDE: usize = 0x64;
/// `lea ebx, [edi + 0x88]` (0x5888A9) — where the records start inside the session.
pub const PLAYERS_AT: usize = 0x88;
/// `[record + 0x5C]` — the per-player countdown `0x588890` refreshes and 0x587D82 spends.
pub const COUNTDOWN_AT: usize = 0x5C;
/// What a `'p'` carries and what a record's head is worth.
pub const RECORD_SIZE: usize = comm::RECORD_SIZE;

/// `0x589440` walks bits `0 … session[0x84] - 2`, so a limit of 8 hands out seven slots and the
/// host keeps the eighth.
pub const DEFAULT_LIMIT: u32 = 8;

/// What a poll produced. The names are the original's behaviour, not new concepts.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A `'d'` answered our `'w'` or `'r'`.
    Found { from: SocketAddr, describe: Box<Describe> },
    /// An `'a'` with status 1 — we are in, at this slot.
    Joined { slot: u32 },
    /// An `'a'` with anything else.
    Refused { status: JoinStatus },
    /// A `'j'` took a slot (host side).
    PeerJoined { slot: u32, from: SocketAddr },
    /// A `'K'`/`'b'`, or a countdown that ran out (0x587D8B clears the bit).
    PeerLeft { slot: u32 },
    /// A `'<'` — the game's own message, handed on untouched.
    Data { from: SocketAddr, slot: Option<u32>, bytes: Vec<u8> },
    /// A `'q'`: the peer is going away and has been answered with an empty `'!'`.
    Quit { from: SocketAddr },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
    /// `record+0x00` — the 20 bytes 0x587DA9 hands to the transport as the `'*'` destination.
    pub addr: Addr,
    pub sock: SocketAddr,
    /// the 16 bytes a `'j'` carries at `+0x18`
    pub name: [u8; 16],
    /// `record+0x5C`
    pub countdown: u32,
    /// the rest of the 0x5C-byte record a `'p'` sends back
    pub record: [u8; RECORD_SIZE],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Client,
}

pub struct Net {
    pub sock: CommSocket,
    pub role: Role,
    /// The next request number to stamp on a `'g'`, `'r'` or `'j'` — `[0x5E1CE8]`, which starts at
    /// 1 and is post-incremented by 0x588130.
    pub seq: u32,
    /// The request the reply handlers match against, which the original keeps in `conn+0x20`. One
    /// at a time, as the original does.
    pub pending: u32,
    /// `session+0x54` — what a `'d'` puts at `+0x28`.
    pub name: [u8; 32],
    /// `session+0x0C` — what a `'w'` probe's first 32 bytes are compared against (0x588AD9).
    /// Both ends must carry the same one, so it is the *application's* identity rather than a
    /// per-session secret: a probe with the wrong key gets no answer at all. Where the original
    /// fills `session+0x0C` from has not been read (§7.13), so a port picks its own and says so.
    pub key: [u8; 32],
    /// `session+0x84`.
    pub limit: u32,
    /// `session+0x50`.
    pub mask: u32,
    pub peers: Vec<Option<Peer>>,
    /// our own 16-byte name, which the host refuses if it equals its own (0x588E7E)
    pub my_name: [u8; 16],
    /// where we joined, client side
    pub host: Option<SocketAddr>,
    /// the slot an `'a'` gave us
    pub slot: Option<u32>,
    /// Host only. The original does not relay: `0x5894C0` hands the transport a **null** address,
    /// which the sender thread turns into a subnet broadcast (0x5872C0), so on a LAN every peer
    /// hears every other peer directly. That only works when all of them sit on the one well-known
    /// port 1106, which two copies on one machine cannot do. With this set the host passes each
    /// peer's `'<'` on to the others, which puts the same bytes in front of the same players — it
    /// is a **substitute for the broadcast**, not something the original does. Clear it and use
    /// [`Net::broadcast_game`] for the literal behaviour.
    pub relay: bool,
    /// `conn+0x5A` — the data layer's own sequence, which `0x5952EF` starts at 1.
    pub data_seq: u16,
    /// `conn+0x5E` per peer — the sequence we next expect from that peer, which every frame we
    /// send it carries as its acknowledgement. The original keeps one connection per peer with its
    /// own counter, so this is the same thing keyed by address.
    pub data_ack: std::collections::HashMap<SocketAddr, u16>,
    /// `conn+0xA8`
    alive: u32,
    /// `conn+0xAC`
    ping: u32,
}

impl Net {
    pub fn host(sock: CommSocket, name: [u8; 32], key: [u8; 32], limit: u32) -> Self {
        Self {
            sock,
            role: Role::Host,
            seq: 1,
            pending: 0,
            name,
            key,
            limit,
            mask: 0,
            peers: vec![None; limit.max(1) as usize],
            my_name: [0; 16],
            host: None,
            slot: None,
            relay: true,
            data_seq: 1,
            data_ack: std::collections::HashMap::new(),
            alive: comm::ALIVE_TICKS,
            ping: comm::PING_TICKS,
        }
    }

    pub fn client(sock: CommSocket, key: [u8; 32], my_name: [u8; 16]) -> Self {
        Self {
            sock,
            role: Role::Client,
            seq: 1,
            pending: 0,
            name: [0; 32],
            key,
            limit: DEFAULT_LIMIT,
            mask: 0,
            peers: vec![None; DEFAULT_LIMIT as usize],
            my_name,
            host: None,
            slot: None,
            relay: false,
            data_seq: 1,
            data_ack: std::collections::HashMap::new(),
            alive: comm::ALIVE_TICKS,
            ping: comm::PING_TICKS,
        }
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    fn record(&self) -> io::Result<Addr> {
        self.sock.local_record()
    }

    /// 0x588130: take the next request number and remember it as the one a reply must carry.
    fn open_request(&mut self) -> u32 {
        let s = self.seq;
        self.seq = self.seq.wrapping_add(1).max(1);
        self.pending = s;
        s
    }

    /// `'w'` — shout the session key and see who answers with a `'d'` (0x588AB8).
    pub fn probe(&self, port: u16) -> io::Result<()> {
        self.sock.broadcast(port, tag::W, &self.key).map(|_| ())
    }

    /// The same probe at one address, for a host the user typed in.
    pub fn probe_at(&self, to: SocketAddr) -> io::Result<()> {
        self.sock.send_to(to, tag::W, &self.key).map(|_| ())
    }

    /// `'r'` — ask a host to describe itself (0x588B06). The four bytes are the request number.
    pub fn query(&mut self, to: SocketAddr) -> io::Result<()> {
        let q = self.open_request();
        self.sock.send_to(to, tag::R, &q.to_be_bytes()).map(|_| ())
    }

    /// `'g'` — "describe slot `index`", answered with `'p'` or `'?'` (0x588CA2).
    pub fn query_slot(&mut self, to: SocketAddr, index: i32) -> io::Result<()> {
        let head = comm::Header { request: self.open_request(), from: self.record()?, index };
        let mut p = Vec::new();
        head.encode(&mut p);
        self.sock.send_to(to, tag::QUERY, &p).map(|_| ())
    }

    /// `'j'` — ask for a slot. `record` is the tail from `+0x28` that 0x589440 keeps.
    pub fn join(&mut self, host: SocketAddr, record: &[u8]) -> io::Result<()> {
        self.host = Some(host);
        let j = Join { request: self.open_request(), from: self.record()?, name: self.my_name, record: record.to_vec() };
        self.sock.send_to(host, tag::JOIN, &j.encode()).map(|_| ())
    }

    /// `'K'` — leave (0x589152 clears our bit on the host).
    pub fn leave(&self) -> io::Result<()> {
        let mut p = Vec::new();
        comm::Header { request: self.pending, from: self.record()?, index: self.slot.map_or(-1, |s| s as i32) }.encode(&mut p);
        for to in self.everyone() {
            self.sock.send_to(to, tag::BYE_K, &p)?;
        }
        Ok(())
    }

    /// Every address this side talks to: the host's peers, or the client's host.
    pub fn everyone(&self) -> Vec<SocketAddr> {
        match self.role {
            Role::Host => self.peers.iter().flatten().map(|p| p.sock).collect(),
            Role::Client => self.host.into_iter().collect(),
        }
    }

    /// The game's own message, framed by the data layer of §7.12c and sent under `'='`, which is
    /// what `0x582700` does. A host sends to every peer and a client to the host; see
    /// [`Net::relay`] for why this is unicast rather than the original's broadcast.
    ///
    /// The connection id and the flags byte are **zero**: `0x5951A0` starts both at 0 and what
    /// fills them later has not been read (§7.17). The sequence is this side's own counter.
    pub fn send_game(&mut self, bytes: &[u8]) -> io::Result<()> {
        let to = self.everyone();
        self.send_game_to(&to, bytes)
    }

    /// [`Net::send_game`] to the addresses named — what the host's room object does when 0x48C980
    /// finds one player's link for it (`crate::room::To::Player`).
    pub fn send_game_to(&mut self, addrs: &[SocketAddr], bytes: &[u8]) -> io::Result<()> {
        let seq = self.data_seq;
        self.data_seq = crate::data::Frame::next_seq(seq);
        for &to in addrs {
            let f = crate::data::Frame {
                // what we next expect from *this* peer, which is what the original's per-peer
                // connection carries at `conn+0x5E`
                ack: self.data_ack.get(&to).copied().unwrap_or(1),
                seq,
                // `conn+0x58` is set to 0 when the layer opens and never to anything else
                flags: 0,
                pieces: vec![crate::data::Piece { seq, body: bytes.to_vec() }],
            };
            let framed = f.encode().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
            self.sock.send_to(to, crate::data::TAG, &framed)?;
        }
        Ok(())
    }

    /// `0x5894C0` literally: tag `'<'`, no header, and a null address, which 0x5872C0 turns into
    /// the subnet broadcast on the socket's own port. Every peer must be bound to `port` for this
    /// to reach them, which is why the original uses the fixed 1106.
    pub fn broadcast_game(&self, port: u16, bytes: &[u8]) -> io::Result<()> {
        self.sock.broadcast(port, tag::USER, bytes).map(|_| ())
    }

    /// One pass over whatever has arrived. Never blocks.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(Some((from, d))) = self.sock.recv() {
            self.handle(from, d.tag, &d.payload, &mut out);
        }
        out
    }

    fn slot_of(&self, from: SocketAddr) -> Option<u32> {
        self.peers.iter().position(|p| p.as_ref().is_some_and(|p| p.sock == from)).map(|i| i as u32)
    }

    /// `0x588890`: find the peer this datagram came from and refresh its countdown to 450.
    fn refresh(&mut self, from: SocketAddr) -> Option<u32> {
        let i = self.slot_of(from)?;
        if let Some(p) = self.peers[i as usize].as_mut() {
            p.countdown = comm::PING_TICKS;
        }
        Some(i)
    }

    fn handle(&mut self, from: SocketAddr, t: u8, p: &[u8], out: &mut Vec<Event>) {
        // every reply handler checks the request number first and drops the datagram when it does
        // not match the one outstanding (0x588B7C, 0x588DAB, 0x588FD5)
        let replies_to_us = |p: &[u8]| comm::read_be(p, 0, 4) == Some(self.pending);
        match t {
            tag::W if self.role == Role::Host => {
                if p.len() >= 32 && p[..32] == self.key[..] {
                    let _ = self.describe(from, 0);
                }
            }
            tag::R if self.role == Role::Host => {
                if let Some(id) = comm::read_be(p, 0, 4) {
                    let _ = self.describe(from, id);
                }
            }
            tag::DESCRIBE => {
                // a `'w'` carries no request, so its `'d'` echoes 0 (0x588AF0)
                if (d_ok(p, self.pending)) && let Ok(d) = Describe::decode(p) {
                    self.alive = comm::ALIVE_TICKS;
                    out.push(Event::Found { from, describe: Box::new(d) });
                }
            }
            tag::QUERY if self.role == Role::Host => {
                if let Ok(head) = comm::Header::decode(p) {
                    let rec = self.slot_record(head.index);

                    let (t, body) = comm::answer_query(head, rec.as_ref());
                    let _ = self.sock.send_to(from, t, &body);
                }
            }
            tag::REPLY | tag::NO_SUCH => {
                if replies_to_us(p) {
                    self.alive = comm::ALIVE_TICKS;
                    if t == tag::REPLY && p.len() >= comm::REPLY_SIZE {
                        // the original hands payload+0x1C and index+1 to the connection's callback
                        out.push(Event::Data {
                            from,
                            slot: comm::read_be(p, 0x18, 4).map(|i| i.wrapping_add(1)),
                            bytes: p[comm::Header::SIZE..].to_vec(),
                        });
                    }
                }
            }
            tag::JOIN if self.role == Role::Host => {
                if let Ok(j) = Join::decode(p) {
                    let reply = self.admit(&j, from);
                    if let JoinStatus::Joined(slot) = reply.status {
                        out.push(Event::PeerJoined { slot, from });
                    }
                    let _ = self.sock.send_to(from, tag::A, &reply.encode());
                }
            }
            tag::A if self.role == Role::Client => {
                if let Ok(a) = JoinReply::decode(p)
                    && a.request == self.pending
                    {
                        self.ping = comm::PING_TICKS;
                        self.alive = comm::ALIVE_TICKS;
                        match a.status {
                            JoinStatus::Joined(slot) => {
                                self.slot = Some(slot);
                                self.host = Some(from);
                                out.push(Event::Joined { slot });
                            }
                            other => out.push(Event::Refused { status: other }),
                        }
                    }
            }
            // 0x5890ED's keep-alive carries nothing; 0x582700's data frame carries a frame.
            tag::ALIVE if p.is_empty() => {
                if self.role == Role::Host {
                    self.refresh(from);
                } else if self.host == Some(from) {
                    self.alive = comm::ALIVE_TICKS;
                }
            }
            tag::ALIVE => {
                let slot = if self.role == Role::Host { self.refresh(from) } else { None };
                if self.relay && !for_the_host(p) {
                    for to in self.everyone() {
                        if to != from {
                            let _ = self.sock.send_to(to, crate::data::TAG, p);
                        }
                    }
                }
                if let Ok(f) = crate::data::Frame::decode(p) {
                    // the peer's sequence is now delivered, so the next one is what we expect
                    self.data_ack.insert(from, crate::data::Frame::next_seq(f.seq));
                    for piece in f.pieces {
                        out.push(Event::Data { from, slot, bytes: piece.body });
                    }
                }
            }
            tag::STAR => {
                if self.role == Role::Host {
                    self.refresh(from);
                } else if self.host == Some(from) {
                    // 0x5890C3: a client answers the ping with another '*' and rearms 0xAC
                    self.ping = comm::PING_TICKS;
                    let _ = self.sock.send_to(from, tag::STAR, &[]);
                }
            }
            tag::BYE_K | tag::BYE_B => {
                if self.role == Role::Host {
                    if let Some(i) = self.slot_of(from) {
                        self.drop_slot(i, out);
                    }
                } else if self.host == Some(from) {
                    self.host = None;
                    self.slot = None;
                }
            }
            tag::Q => {
                let _ = self.sock.send_to(from, tag::QUIT_ACK, &[]);
                out.push(Event::Quit { from });
                if self.role == Role::Host
                    && let Some(i) = self.slot_of(from)
                {
                    self.drop_slot(i, out);
                }
            }
            tag::USER => {
                let slot = if self.role == Role::Host { self.refresh(from) } else { None };
                if self.relay {
                    for to in self.everyone() {
                        if to != from {
                            let _ = self.sock.send_to(to, tag::USER, p);
                        }
                    }
                }
                out.push(Event::Data { from, slot, bytes: p.to_vec() });
            }
            _ => {} // 0x5891E2: anything else is dropped
        }
    }

    /// The 0x5C bytes a `'p'` carries: the session's own head for `-1`, else the player's record.
    fn slot_record(&self, index: i32) -> Option<[u8; RECORD_SIZE]> {
        if index == comm::Header::SESSION_SLOT {
            let mut r = [0u8; RECORD_SIZE];
            if let Ok(a) = self.record() {
                r[..20].copy_from_slice(&a.0);
            }
            return Some(r);
        }
        let i = usize::try_from(index).ok()?;
        // 0x588D08 answers '?' unless the slot's bit is set
        if self.mask & (1 << i) == 0 {
            return None;
        }
        self.peers.get(i)?.as_ref().map(|p| p.record)
    }

    /// 0x5892A0, with the fields this port knows about; the rest are zero and §7.10 says so.
    fn describe(&self, to: SocketAddr, request: u32) -> io::Result<()> {
        let d = Describe {
            request,
            from: self.record()?,
            conn_dword: 0,
            conn_words: [0, 0],
            conn_raw: [0, 0],
            name: self.name,
            bytes: [0; 4],
            words: [0, 0],
            dwords: [0, 0],
            player_limit: self.limit,
            slot_mask: self.mask,
        };
        self.sock.send_to(to, tag::DESCRIBE, &d.encode()).map(|_| ())
    }

    /// 0x588E11's rules, in order.
    fn admit(&mut self, j: &Join, from: SocketAddr) -> JoinReply {
        let echo = JoinReply { request: j.request, from: j.from, status: JoinStatus::Refused };
        // already in? the original answers with the slot it already has (0x588F41)
        if let Some(i) = self.slot_of(from) {
            return JoinReply { status: JoinStatus::Joined(i), ..echo };
        }
        if j.name == self.my_name && self.my_name != [0; 16] {
            return JoinReply { status: JoinStatus::NameTaken, ..echo };
        }
        let Some(slot) = comm::free_slot(self.mask, self.limit) else {
            return JoinReply { status: JoinStatus::Full, ..echo };
        };
        let mut record = [0u8; RECORD_SIZE];
        let addr = record_of(from).unwrap_or(j.from);
        record[..20].copy_from_slice(&addr.0);
        let tail = j.record.len().min(RECORD_SIZE - 20);
        record[20..20 + tail].copy_from_slice(&j.record[..tail]);
        self.peers[slot as usize] = Some(Peer {
            addr,
            sock: from,
            name: j.name,
            countdown: comm::PING_TICKS,
            record,
        });
        self.mask |= 1 << slot;
        JoinReply { status: JoinStatus::Joined(slot), ..echo }
    }

    fn drop_slot(&mut self, i: u32, out: &mut Vec<Event>) {
        if self.peers[i as usize].take().is_some() {
            self.mask &= !(1 << i);
            out.push(Event::PeerLeft { slot: i });
        }
    }

    /// One turn of `0x587CE0`. The original registers it as a periodic task, so a port calls it
    /// once per frame: the counts are in polls, not seconds.
    pub fn tick(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if self.role == Role::Client {
            // 0x587D10: only a non-host spends 0xAC
            self.ping = self.ping.saturating_sub(1);
            return out;
        }
        if self.mask == 0 {
            return out;
        }
        // 0x587D3E: below 2 it rearms and this pass is a ping round
        let ping_round = self.alive < 2;
        self.alive = if ping_round { comm::ALIVE_TICKS } else { self.alive - 1 };

        for i in 0..self.limit.saturating_sub(1) {
            if self.mask & (1 << i) == 0 {
                continue;
            }
            let Some(p) = self.peers[i as usize].as_mut() else { continue };
            if p.countdown == 0 {
                self.drop_slot(i, &mut out);
                continue;
            }
            p.countdown -= 1;
            if ping_round {
                let to = p.sock;
                let _ = self.sock.send_to(to, tag::STAR, &[]);
            }
        }
        out
    }

    /// Has the client heard from the host lately? `conn+0xAC` reaching zero is what the original
    /// leaves to its caller to notice.
    pub fn host_silent(&self) -> bool {
        self.role == Role::Client && self.ping == 0
    }

    /// The address record of a slot, for a caller that wants to talk to one peer.
    pub fn peer_addr(&self, slot: u32) -> Option<SocketAddr> {
        self.peers.get(slot as usize)?.as_ref().map(|p| p.sock).or_else(|| {
            self.peers.get(slot as usize)?.as_ref().and_then(|p| addr_of(&p.addr))
        })
    }
}

/// Whether a data frame carries a message a player sends on its link to the host (`[net+0xC]`)
/// rather than on the broadcast channel (`[net+8]`) — which the relay, a stand-in for the
/// broadcast, must then leave alone. The chat request 0x48D9B0 is one (`crate::room`): the host
/// rewrites it before anyone else sees it (0x491324). The other `[net+0xC]` senders — 0x48D38B,
/// 0x48D3BB, 0x48D97B, 0x48DA22, 0x48DAA4 … — are not ported, so they are not listed.
fn for_the_host(p: &[u8]) -> bool {
    crate::data::Frame::decode(p).is_ok_and(|f| f.pieces.iter().any(|x| x.body.first() == Some(&crate::msg::kind::CHAT)))
}

/// A `'d'` answers either an `'r'` (echoing its request number) or a `'w'` (echoing 0, because a
/// probe has no request behind it).
fn d_ok(p: &[u8], pending: u32) -> bool {
    matches!(comm::read_be(p, 0, 4), Some(v) if v == 0 || v == pending)
}

/// Pad or clip a name into the 16 bytes a `'j'` carries.
pub fn name16(s: &str) -> [u8; 16] {
    let mut n = [0u8; 16];
    let b = s.as_bytes();
    let k = b.len().min(16);
    n[..k].copy_from_slice(&b[..k]);
    n
}

/// The same for the 32 bytes a `'d'` carries.
pub fn name32(s: &str) -> [u8; 32] {
    let mut n = [0u8; 32];
    let b = s.as_bytes();
    let k = b.len().min(32);
    n[..k].copy_from_slice(&b[..k]);
    n
}

/// So a caller can tell a decode failure from a drop.
pub type Result<T> = std::result::Result<T, CommError>;
