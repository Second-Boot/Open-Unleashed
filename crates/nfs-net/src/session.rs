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
use std::time::{Duration, Instant};

/// `add ebx, 0x64` at 0x588D8 and 0x587DBD — one player record is 100 bytes.
pub const PLAYER_STRIDE: usize = 0x64;
/// `lea ebx, [edi + 0x88]` (0x5888A9) — where the records start inside the session.
pub const PLAYERS_AT: usize = 0x88;
/// `[record + 0x5C]` — the per-player countdown `0x588890` refreshes and 0x587D82 spends.
pub const COUNTDOWN_AT: usize = 0x5C;
/// What a `'p'` carries and what a record's head is worth.
pub const RECORD_SIZE: usize = comm::RECORD_SIZE;

/// `0x589440` walks bits `0 … session[0x84] - 2`. The original's host answers `'d'` with `+0x58 = 9`
/// (captured, `REFERENCE-2026-10-02-SRVE.md` §4, both sessions) — eight joiner slots. **The `+1` (a gdb
/// watch on the original host's descriptor copy):** the CONMAN create 0x55AAA0 copies the game's 0x34-byte descriptor
/// (`desc+0x30` = 8, 0x509B49) to `conman+0x1D4` (`rep movsd`, 0x55AB34), reads that word (`conman+0x204`, 0x55AB73),
/// `inc ebx` (0x55AB86) and stores it back (0x55AB8D) — the host's own player makes nine; the provider's create 0x5882A0
/// copies the 0x34 bytes on to `session+0x54` (`rep movsd`, 0x58830B): that is `session+0x84` = 9.
pub const DEFAULT_LIMIT: u32 = 9;

/// **Port-only guard (read, network.md §7.22):** the room's masks are *bytes* in the original too —
/// `room+0xCC0 … +0xCC4` (0x48C2B0's `1 << slot`, the lobby / started / running groups) and the input
/// frame ring's 24-byte frames hold `3 · slot` for **eight** players (0x48C2B0: `room + 0xB0 + 24·head +
/// 3·slot`) — so the original's session admits up to eight joiners (`'d'` limit 9) but a ninth id (slot 8)
/// has no place in the room's masks or frames: what the original does with it was not run (it needs nine
/// connections). The port keeps the host's slots to ids 1…7, i.e. seven joiners, so the host + 7 = 8 cars
/// of a race (`car[0..7]`); an eighth joiner is refused (`Full`) where the original would take it.
pub const MAX_JOINER_SLOTS: u32 = 7;

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
    /// the player's name: the first field of the identity at `rec+0x48` (0x48A700's `%[^~]`), cut to 15 bytes
    pub name: [u8; 16],
    /// `record+0x5C`
    pub countdown: u32,
    /// the rest of the 0x5C-byte record a `'p'` sends back
    pub record: [u8; RECORD_SIZE],
    /// the host's send queue of this peer's record (0x48A3D0 / 0x48A4E0, [`crate::outbox::PeerQueue`])
    pub queue: crate::outbox::PeerQueue,
    /// `+0xBA`, "in" (0x48A570 starts it 0): set by the host's arrival handler once it has answered the
    /// peer's identity message — 0x491991 / 0x491A9E / 0x491C25 inside `case 7` of 0x491010, each right after
    /// the `0xB8` / `0xC6` it sends (the room's `admit`: the `0xB0`s, then `0xC6 (0xE)`), and 0x490913 /
    /// 0x490927 in the 0x4907C0 pass for a record whose version is decided. Never cleared (no instruction
    /// writes 0 to it: a scan of nfs5.exe's `c6 8x ba 00 00 00 imm` finds those five stores only). The pump
    /// 0x48A4E0 sends nothing for a record that is not in. [`Net::peer_in`].
    pub in_room: bool,
    /// **The advertised `session+0x50` bit's lifetime**: when this slot's bit leaves the mask a `'d'` carries.
    /// 0x589440 sets the bit and `record+0x5C = 450` at the admission; 0x587CE0 (the provider's periodic callback)
    /// spends the count and, at its end, clears the bit (0x587D8B...0x587D92) - no leave, no `'K'`: the player stays in
    /// the room. [`ADVERTISED_BIT_LIFE`].
    pub adv_until: Instant,
}

/// How long a slot's bit stays in the mask a `'d'` advertises (`session+0x50`), with nothing refreshing it. **APPROXIMATION
/// of the period only**: the count is 450 spends of 0x587CE0 (0x5E1A04 -> 0x587D82), which the original ran live on its
/// `nfs5-third` host as 3.52 s (admission 0x589486 -> clearing 0x587D92, gdb, runs of 3.52 / 3.54 / 3.54 s; the callback
/// was entered 900 times, two registrations per round); the timer that calls it (0x580DC0's registry, driven from
/// 0x53C0F0) was not read, so the cadence is the measured one.
pub const ADVERTISED_BIT_LIFE: Duration = Duration::from_millis(3530);

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
    /// What a `'w'` probe carries: the key text and its NUL (`strlen + 1`, 0x560620), 43 bytes for the
    /// application's own key; defaults to `key` for a session that made its own.
    pub probe_payload: Vec<u8>,
    /// The request number an outstanding `'j'` keeps while it is repeated (0x588130 sends one buffer
    /// again and again until the reply or the deadline).
    join_req: Option<u32>,
    /// the peers the data layer has already sent its connect `'q'` to ([`Net::send_frame`])
    q_sent: std::collections::HashSet<SocketAddr>,
    /// `session+0x84`.
    pub limit: u32,
    /// `session+0x50`.
    pub mask: u32,
    /// `session+0x74 … +0x77`, what a `'d'` puts at `+0x48` — the game's descriptor bytes
    /// `desc+0x20 … +0x23` ([`crate::comm::desc`]).
    pub desc_bytes: [u8; 4],
    /// `session+0x78` / `+0x7A`, `'d'` `+0x4C` / `+0x4E` — `desc+0x24` / `+0x26`.
    pub desc_words: [u16; 2],
    /// The address record a `'d'` carries at `+0x04` (`conn+0x24`). `None` sends the socket's own
    /// (0.0.0.0 while bound to every interface). **Platform glue:** a host sets its machine's
    /// address here so that the answers to one probe that reach a browser over several of its
    /// interfaces name one session (the browser merges on it), as one broadcast would have.
    pub advertise: Option<Addr>,
    pub peers: Vec<Option<Peer>>,
    /// our own 16-byte name, which the host refuses if it equals its own (0x588E7E)
    pub my_name: [u8; 16],
    /// where we joined, client side
    pub host: Option<SocketAddr>,
    /// the slot an `'a'` gave us
    pub slot: Option<u32>,
    /// Host only: the **room's relay** — the server dispatch 0x49120C hands a player's `0xBC` to
    /// 0x48C050 (lobby-group gated), `0xBD` / `0xBF` … `0xC2` to 0x48C0F0 and `0xBE` to 0x48C170, each of which
    /// sends the message on to the room's other players (`network.md` §11.18; earlier notes took the
    /// original for a broadcast, `0x5894C0`, which nothing calls). Clear it to make a bare session
    /// that only receives.
    pub relay: bool,
    /// The data layer's connection object per peer (`conn` of 0x5951A0: its own sequence
    /// `+0x5A`, ack `+0x5E`, sent list, timers — [`crate::reliable`], `network.md` §11.18), keyed by
    /// address. Made when the first game frame goes to or comes from a peer.
    pub conns: std::collections::HashMap<SocketAddr, crate::reliable::Conn>,
    /// The origin of the 128 Hz tick clock the connections' timers run on (glue: the original's
    /// `[0x6B7C7C]` is the multimedia timer's counter).
    epoch: std::time::Instant,
    /// Host only: the game's gate on a joiner, which the original runs as the callback `conn[0x1C]+0x4C`
    /// (0x588EBF…0x588ED3, refusing with `-3`) — 0x48A700's version check (the identity's second
    /// field against `"3.5"`, 0x5D06FC): the version this host answers to. A joiner whose identity
    /// string ([`comm::identity`], the `'j'` record's tail) names another version is refused. The
    /// room's game-name test (0x49151E…0x491569) belongs to the `0xB6` room request of the lobby
    /// service (case 182 of 0x491010) and is not run for a LAN / direct join (`network.md` §11.28).
    /// `None` (a test's bare session) lets everyone in.
    pub identity_gate: Option<String>,
    /// The send queue in front of the pool, a client's only road for reliable game messages
    /// ([`crate::outbox`], 0x48AD70 / 0x48AE30). A host's room sends on each link itself.
    pub outbox: crate::outbox::Outbox,
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
            probe_payload: key.to_vec(),
            join_req: None,
            q_sent: std::collections::HashSet::new(),
            limit,
            mask: 0,
            desc_bytes: [0; 4],
            desc_words: [0; 2],
            advertise: None,
            peers: vec![None; limit.max(1) as usize],
            my_name: [0; 16],
            host: None,
            slot: None,
            relay: true,
            identity_gate: None,
            conns: std::collections::HashMap::new(),
            outbox: crate::outbox::Outbox::new(),
            // det-allow: transport timeouts, not simulation state
            epoch: std::time::Instant::now(),
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
            probe_payload: key.to_vec(),
            join_req: None,
            q_sent: std::collections::HashSet::new(),
            limit: DEFAULT_LIMIT,
            mask: 0,
            desc_bytes: [0; 4],
            desc_words: [0; 2],
            advertise: None,
            peers: vec![None; DEFAULT_LIMIT as usize],
            my_name,
            host: None,
            slot: None,
            relay: false,
            identity_gate: None,
            conns: std::collections::HashMap::new(),
            outbox: crate::outbox::Outbox::new(),
            // det-allow: transport timeouts, not simulation state
            epoch: std::time::Instant::now(),
            alive: comm::ALIVE_TICKS,
            ping: comm::PING_TICKS,
        }
    }

    /// **0x582700**: the data layer's send. When the link is not connected yet (`link+0x4C < 0`) an empty
    /// `'q'` goes first, then the frame under `'='` (0x582711 / 0x58273D). Captured: one `'q'` per side, 1 ms
    /// before the first `'='` (the ack-only frame), none after (`REFERENCE-2026-10-02-SRVE.md` §2).
    /// **APPROXIMATION:** where the original's `link+0x4C` turns non-negative was not traced; this sends the
    /// `'q'` once per peer, before the first frame to it, which is what both captured sessions show.
    fn send_frame(&mut self, to: SocketAddr, frame: &[u8]) -> io::Result<usize> {
        if self.q_sent.insert(to) {
            let _ = self.sock.send_to(to, crate::data::CONNECT_TAG, &[]);
        }
        self.sock.send_to(to, crate::data::TAG, frame)
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
        self.sock.broadcast(port, tag::W, &self.probe_payload).map(|_| ())
    }

    /// The `'w'` to every place [`crate::socket::probe_targets`] names — the original's own
    /// broadcast, and the platform glue around it — with what each send answered, so a caller can
    /// say when the OS refuses one (macOS answers `EHOSTUNREACH`/`EPERM` to a send while the Local
    /// Network permission is not granted).
    pub fn probe_all(&self, port: u16) -> Vec<(crate::socket::ProbeKind, SocketAddr, io::Result<usize>)> {
        crate::socket::probe_targets(port)
            .into_iter()
            .map(|(k, a)| (k, a, self.sock.send_to(a, tag::W, &self.probe_payload)))
            .collect()
    }

    /// The same probe at one address, for a host the user typed in.
    pub fn probe_at(&self, to: SocketAddr) -> io::Result<()> {
        self.sock.send_to(to, tag::W, &self.probe_payload).map(|_| ())
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

    /// `'j'` — ask for a slot, as the original's 0x588030 builds it ([`Join::original`]): the host's
    /// address at `+0x04`, 16 zeros at `+0x18`, our own record from `+0x28` with `identity` (the
    /// `name~version~game` text) at `rec+0x48`. Repeating the call while no `'a'` has come repeats the
    /// same request number, as 0x588130's loop does.
    pub fn join(&mut self, host: SocketAddr, identity: &[u8]) -> io::Result<()> {
        self.host = Some(host);
        let request = match self.join_req {
            Some(r) => r,
            None => {
                let r = self.open_request();
                self.join_req = Some(r);
                r
            }
        };
        let to = record_of(host).unwrap_or_default();
        let j = Join::original(request, to, self.record()?, identity);
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

    /// The data layer's clock: 128 Hz ticks since the session was made (glue for `[0x6B7C7C]`).
    fn ticks(&self) -> i64 {
        (self.epoch.elapsed().as_micros() as i64 * crate::reliable::TICK_HZ) / 1_000_000
    }

    /// Which of the game's messages ride the reliable half of the data layer. `network.md` §11.17:
    /// the input sample (a player's three bytes, 0x55D4D0), the `0xBE` state and the room's `0xC4`
    /// heartbeat (0x55C2B0) go **unreliably**; the room's frames, `0xB3`, `0xC5`, the reports and
    /// every other message of the room's links go through 0x55C270 -> 0x595C40 reliably.
    ///
    /// A message a client sends the host that the host does not act on itself
    /// ([`msg_for_the_host`]) is relayed by the **room**: `0xBC` by 0x48C050 (only to the players in
    /// the sender's lobby group, see [`crate::room::Room::relay_options`]), `0xBD` / `0xBF` … `0xC2` by
    /// 0x48C0F0 (to every player, the sender excluded for the file chunks), `0xBE` by 0x48C170 — to
    /// every player but the sender through **0x55C2B0, the unreliable send**, where the others go
    /// through the slot-0 sender of the room's channel like every other room message
    /// (`network.md` §11.18). [`Net::relay`] does the relaying (the group gate for `0xBC` is the
    /// caller's); the client's own send and the relay both go reliably except the `0xBE` state.
    fn is_reliable(&self, body: &[u8]) -> bool {
        use crate::msg::kind;
        match body.first() {
            None => false,
            Some(&kind::CAR_STATE) | Some(&0xC4) => false,
            // below 0xB0: a frame from the room (reliable), a sample from a player (not)
            Some(&t) if t < 0xB0 => self.role == Role::Host,
            Some(_) => true,
        }
    }

    /// [`Net::send_game`] to the addresses named — what the host's room object does when 0x48C980
    /// finds one player's link for it (`crate::room::To::Player`). Reliable or not by
    /// [`Net::is_reliable`], through the peer's own connection ([`crate::reliable::Conn`]).
    pub fn send_game_to(&mut self, addrs: &[SocketAddr], bytes: &[u8]) -> io::Result<()> {
        let reliable = self.is_reliable(bytes);
        if reliable && self.role == Role::Client {
            // the builder 0x48C870 → slot 0 of the queue, 0x48AD70 (`crate::outbox`): registered, and sent
            // only while the queue is idle; the pump sends the rest
            let mut ob = std::mem::take(&mut self.outbox);
            ob.register(bytes, &mut |b| self.link_send(b));
            self.outbox = ob;
            return Ok(());
        }
        let now = self.ticks();
        for &to in addrs {
            if reliable && self.host_register(to, bytes) {
                continue;
            }
            if reliable {
                // the original's room sends reliably only through a peer record's queue
                // (0x48B340 → 0x48A3D0); an address without a record has no sender, nothing goes
                continue;
            }
            let conn = self.conns.entry(to).or_insert_with(|| crate::reliable::Conn::new(now));
            let f = conn.send_unreliable(bytes, now);
            self.send_frame(to, &f)?;
        }
        Ok(())
    }

    /// The host's room sender for one peer — the room's send slot 0 (0x48B340) into the peer record's queue,
    /// **0x48A3D0** ([`crate::outbox::PeerQueue::register`]): `true` when `to` is a peer of this host's slot
    /// table (the message is registered and sent only while the record is idle), `false` for any other address
    /// (an address the table does not hold: no record, no reliable sender — [`Net::send_game_to`] sends nothing).
    fn host_register(&mut self, to: SocketAddr, body: &[u8]) -> bool {
        if self.role != Role::Host {
            return false;
        }
        let Some(i) = self.slot_of(to) else { return false };
        let mut q = std::mem::take(&mut self.peers[i as usize].as_mut().expect("slot_of found it").queue);
        q.register(body, &mut |b| self.peer_send(to, b));
        if let Some(p) = self.peers[i as usize].as_mut() {
            p.queue = q;
        }
        true
    }

    /// 0x55C270 -> 0x595C40 on the link to the peer at `to`: `false` when no packet is free (the original's
    /// answer 0).
    fn peer_send(&mut self, to: SocketAddr, body: &[u8]) -> bool {
        let now = self.ticks();
        let conn = self.conns.entry(to).or_insert_with(|| crate::reliable::Conn::new(now));
        match conn.send(body, now) {
            Some(f) => {
                let _ = self.send_frame(to, &f);
                true
            }
            None => false,
        }
    }

    /// The host's arrival handler is through with the peer in `slot`: `+0xBA = 1` (0x491AB4 / 0x491C25, after the
    /// `0xC6` the handler sent — see [`Peer::in_room`]). The caller runs it right after dispatching the room's
    /// `admit` output (the `0xB0`s and `0xC6 (0xE)`), which were registered while the record was not in: the
    /// first of them went at once, the rest wait for the pump.
    pub fn peer_in(&mut self, slot: u32) {
        if let Some(Some(p)) = self.peers.get_mut(slot as usize) {
            p.in_room = true;
        }
    }

    /// **0x48A4E0** for every peer record, once per turn of the room's 32 Hz loop (0x48B4F0 at 0x48B54D walks
    /// the records the registrations listed; one that is idle and empty does nothing, so walking all of them is
    /// the same). `+0xBA` ("in") is [`Peer::in_room`]: false until the arrival handler has answered
    /// ([`Net::peer_in`]), and a record that is not in sends nothing.
    pub fn pump_peers(&mut self) {
        for i in 0..self.peers.len() {
            let Some(p) = self.peers[i].as_mut() else { continue };
            let (to, in_room) = (p.sock, p.in_room);
            let mut q = std::mem::take(&mut p.queue);
            q.pump(in_room, &mut |b| self.peer_send(to, b));
            if let Some(p) = self.peers[i].as_mut() {
                p.queue = q;
            }
        }
    }

    /// **0x55D4A0** for the queue: the layer's reliable send (0x560420 → 0x595C40) on the link to the host —
    /// nothing unless the link is connected (`conn+0x2C8 & 1`: the host has taken us, [`Net::slot`]); `false`
    /// when there is no packet free.
    fn link_send(&mut self, body: &[u8]) -> bool {
        let (Some(to), Some(_)) = (self.host, self.slot) else { return false };
        let now = self.ticks();
        let conn = self.conns.entry(to).or_insert_with(|| crate::reliable::Conn::new(now));
        match conn.send(body, now) {
            Some(f) => {
                let _ = self.send_frame(to, &f);
                true
            }
            None => false,
        }
    }

    /// **0x48AE30**, the queue's pump — the caller runs it 32 times a second (0x48EE90). `true`: it sent (or tried
    /// to send) the head; `false`: the queue is empty or not enabled (0x48EE90 then sends a file chunk).
    pub fn pump_outbox(&mut self) -> bool {
        if self.role == Role::Host {
            // the room's tick 0x48B4F0: its own queue (none here) and every listed peer record
            // (0x48A4E0); a host has no client queue, so the answer is 0 and a file chunk may go
            self.pump_peers();
            return false;
        }
        let mut ob = std::mem::take(&mut self.outbox);
        let r = ob.pump(&mut |b| self.link_send(b));
        self.outbox = ob;
        r
    }

    /// 0x55C270 -> 0x595C40 as the room's frame sender sees it: would the link take a reliable
    /// message now (a packet free, 0x595C58)? A peer with no connection yet always can.
    pub fn can_send_reliable(&self, to: SocketAddr) -> bool {
        self.reliable_room(to) > 0
    }

    /// How many reliable messages the link to `to` would take right now (the pool, 0x595C58).
    pub fn reliable_room(&self, to: SocketAddr) -> usize {
        self.conns.get(&to).map_or(crate::reliable::POOL, |c| c.room())
    }

    /// A line per connection — in flight, held back, acks, counters — for a log.
    pub fn report(&self) -> String {
        self.conns
            .iter()
            .map(|(a, c)| format!("{a}: flight {} ack {} {:?}", c.in_flight(), c.ack(), c.stats))
            .collect::<Vec<_>>()
            .join("; ")
            + &format!("; outbox {:?}, {} waiting", self.outbox.stats, self.outbox.len())
            + &self
                .peers
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.as_ref().map(|p| format!("; peer {i} queue {:?}, {} waiting", p.queue.stats, p.queue.len())))
                .collect::<String>()
            + &format!("; the test glue dropped {} data frames", self.sock.dropped())
    }

    /// The data layer's poll (0x595F70) for every connection: what is due goes out.
    fn pump(&mut self) {
        let now = self.ticks();
        let mut out = Vec::new();
        for (&to, c) in self.conns.iter_mut() {
            for f in c.poll(now) {
                out.push((to, f));
            }
        }
        for (to, f) in out {
            let _ = self.send_frame(to, &f);
        }
    }

    /// `0x5894C0` literally: tag `'<'`, no header, and a null address, which 0x5872C0 turns into
    /// the subnet broadcast on the socket's own port. Every peer must be bound to `port` for this
    /// to reach them, which is why the original uses a fixed port.
    pub fn broadcast_game(&self, port: u16, bytes: &[u8]) -> io::Result<()> {
        self.sock.broadcast(port, tag::USER, bytes).map(|_| ())
    }

    /// One pass over whatever has arrived. Never blocks.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(Some((from, d))) = self.sock.recv() {
            self.handle(from, d.tag, &d.payload, &mut out);
        }
        self.pump();
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
                        self.join_req = None;
                        self.ping = comm::PING_TICKS;
                        self.alive = comm::ALIVE_TICKS;
                        match a.status {
                            JoinStatus::Joined(slot) => {
                                self.slot = Some(slot);
                                self.host = Some(from);
                                // the transport's "connected" event (4, 0x48FBB0) enables the queue's pump
                                self.outbox.enable();
                                out.push(Event::Joined { slot });
                            }
                            other => out.push(Event::Refused { status: other }),
                        }
                    }
            }
            // 0x5890ED's keep-alive carries nothing; 0x582700's data frame carries a frame.
            tag::ALIVE if p.is_empty() => {
                if self.role == Role::Host {
                    self.refresh_advertised(from);
                    self.refresh(from);
                } else if self.host == Some(from) {
                    self.alive = comm::ALIVE_TICKS;
                }
            }
            tag::ALIVE => {
                let slot = if self.role == Role::Host { self.refresh(from) } else { None };
                // (live, the original hosting): the original host's `'*'` ping rounds stop ~3 s after admission (two originals' capture:
                // `'*'` at 0.78 / 1.56 / 2.34 / 3.12 s and none after), what keeps the link alive afterwards is its data layer's frames.
                // read the original's own test (network.md 7.21): **there is none** — `conn+0xAC` is decremented by 0x587D10 and read
                // only by 0x588220 (the connect's "already connected" compare) and 0x58908A (the `'*'` rearm); events 9 / 10 of 0x48F0C4 come
                // only from the CONMAN's command 7 (0x55CB75), which the two disconnect members (`[link+0x84]` = 0x55B480, `[conman+0x270]` =
                // 0x55CDE0) queue, and nothing in the image calls either member. So the original never declares a silent host lost; this is a
                // **PORT-ONLY GUARD** (PORT-DEVIATIONS.md), not an approximation of a test: any datagram from the host counts as life.
                if self.role == Role::Client && self.host == Some(from) {
                    self.ping = comm::PING_TICKS;
                }
                let now = self.ticks();
                let conn = self.conns.entry(from).or_insert_with(|| crate::reliable::Conn::new(now));
                for body in conn.receive(p, now) {
                    // the room's relay (0x48C0F0 / 0x48C170): what the host does not act on itself
                    // goes on to the others, as a fresh frame on each link
                    // (`0xBC` is the room's own relay with the lobby-group gate, 0x48C050: the caller's)
                    if self.relay && !msg_for_the_host(&body) && body.first() != Some(&crate::msg::kind::OPTIONS) {
                        let reliable = self.is_reliable(&body);
                        // 0x48C0F0 skips the sender for `0xBF` … `0xC2` and **not for `0xBD`**: the barrier's bit comes back to the joiner that
                        // sent it (captured, REFERENCE-2026-10-02-SRVX: the original host answers a joiner's `0xBD 1` with `0xBD 1`)
                        let echo = body.first() == Some(&0xBD);
                        let others: Vec<SocketAddr> = self.everyone().into_iter().filter(|&a| echo || a != from).collect();
                        for to in others {
                            if reliable && self.host_register(to, &body) {
                                continue;
                            }
                            if reliable {
                                continue; // no peer record, no reliable sender
                            }
                            let c = self.conns.entry(to).or_insert_with(|| crate::reliable::Conn::new(now));
                            let f = c.send_unreliable(&body, now);
                            let _ = self.send_frame(to, &f);
                        }
                    }
                    out.push(Event::Data { from, slot, bytes: body });
                }
            }
            tag::STAR => {
                if self.role == Role::Host {
                    self.refresh_advertised(from);
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
                    // the link is gone: the queue is freed with it (0x48AED0)
                    self.outbox.clear();
                }
            }
            // 0x589029: `if (conn+0x14 != NULL && conn+0x14(...) == 0) send '!'`. The game's session has no such
            // callback — captured: the host never answered the data layer's connect `'q'` (0x582711) with a
            // `'!'`, and a `'q'` takes nothing away (the room's players leave by `'K'`/`'b'` or the countdown)
            tag::Q => {}
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

    /// `session+0x50` as a `'d'` carries it: the admitted slots whose bit has not yet run out ([`Peer::adv_until`]).
    pub fn advertised_mask(&self) -> u32 {
        let now = Instant::now();
        self.peers.iter().enumerate().filter(|(_, p)| p.as_ref().is_some_and(|p| p.adv_until > now)).fold(0, |m, (i, _)| m | 1 << i)
    }

    /// 0x588890 for the two keep-alive tags (`'='` with no body, `'*'`): it walks only the slots whose bit is still set,
    /// so a bit that has run out is not brought back; a refresh gives the slot 450 spends again.
    fn refresh_advertised(&mut self, from: SocketAddr) {
        let now = Instant::now();
        if let Some(i) = self.slot_of(from)
            && let Some(p) = self.peers[i as usize].as_mut()
            && p.adv_until > now
        {
            p.adv_until = now + ADVERTISED_BIT_LIFE;
        }
    }

    /// 0x5892A0, with the fields this port knows about; the rest are zero and §7.10 says so.
    fn describe(&self, to: SocketAddr, request: u32) -> io::Result<()> {
        let g = &comm::APP_GUID;
        let d = Describe {
            request,
            from: match self.advertise {
                Some(a) => a,
                None => self.record()?,
            },
            // +0x18…+0x27 = the application's GUID in the text order of 0x587F90 (captured:
            // `5cd46b81 ece4 11d3 | 9b6a0090 2724ab1e`): u32be, u16be, u16be, two plain dwords
            conn_dword: u32::from_le_bytes([g[0], g[1], g[2], g[3]]),
            conn_words: [u16::from_le_bytes([g[4], g[5]]), u16::from_le_bytes([g[6], g[7]])],
            conn_raw: [u32::from_le_bytes([g[8], g[9], g[10], g[11]]), u32::from_le_bytes([g[12], g[13], g[14], g[15]])],
            name: self.name,
            bytes: self.desc_bytes,
            words: self.desc_words,
            dwords: [0, 0],
            player_limit: self.limit,
            // `session+0x50` (0x589386 -> payload +0x6C): the bit 0x589440 sets at an admission, cleared by 0x587D92 once
            // the slot's 450 spends are gone (gdb on the original host: set 0x589486, cleared 3.52 s later; the
            // steady state is 0 with a player in the room, `REFERENCE-2026-10-02-SRVE.md` §4)
            slot_mask: self.advertised_mask(),
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
        // 0x588E4F…0x588E67: the full test comes first (`popcount(mask) >= limit − 1`, −2), then the
        // name (0x588E7E, −1), then the slot (0x589440) and the game's callback (−3)
        let Some(slot) = comm::free_slot(self.mask, self.limit.min(MAX_JOINER_SLOTS + 1)) else {
            return JoinReply { status: JoinStatus::Full, ..echo };
        };
        if j.name == self.my_name && self.my_name != [0; 16] {
            return JoinReply { status: JoinStatus::NameTaken, ..echo };
        }
        if let Some(version) = &self.identity_gate {
            let (_, v, _) = comm::identity::parse(j.identity());
            if &v != version {
                return JoinReply { status: JoinStatus::Refused, ..echo };
            }
        }
        // 0x589440 keeps the 0x5C bytes at `+0x28`; 0x588E11 then writes the datagram's source address
        // over the first five dwords
        let mut record = [0u8; RECORD_SIZE];
        let have = j.record.len().min(RECORD_SIZE);
        record[..have].copy_from_slice(&j.record[..have]);
        let addr = record_of(from).unwrap_or(j.from);
        record[..20].copy_from_slice(&addr.0);
        // the game reads the name out of the identity (0x48A700's first `%[^~]`)
        let (id_name, _, _) = comm::identity::parse(j.identity());
        let mut name = [0u8; 16];
        for (o, c) in name.iter_mut().take(15).zip(id_name.chars()) {
            *o = u8::try_from(u32::from(c)).unwrap_or(b'?');
        }
        self.peers[slot as usize] = Some(Peer {
            addr,
            sock: from,
            name,
            countdown: comm::PING_TICKS,
            record,
            queue: crate::outbox::PeerQueue::new(),
            in_room: false,
            adv_until: Instant::now() + ADVERTISED_BIT_LIFE,
        });
        self.mask |= 1 << slot;
        JoinReply { status: JoinStatus::Joined(slot), ..echo }
    }

    /// Host: free a player's slot (0x4925A0, which the eject 0x491755 and the leave paths call).
    /// Answers whether the slot was in use.
    pub fn remove_peer(&mut self, i: u32) -> bool {
        let mut out = Vec::new();
        self.drop_slot(i, &mut out);
        !out.is_empty()
    }

    fn drop_slot(&mut self, i: u32, out: &mut Vec<Event>) {
        if let Some(p) = self.peers[i as usize].take() {
            self.conns.remove(&p.sock);
            self.q_sent.remove(&p.sock);
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
            // PORT-DEVIATION D16: the slot is held while the joiner talks (`refresh` on its datagrams) and dropped after
            // PING_TICKS of silence at the port's 64 Hz (~7 s). The original's refresh never matches on UDP/IP (0x560790
            // compares the record's self-reported 0.0.0.0 with the source), so its slot frees itself ~3.5 s after admission.
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

/// Whether the room acts on a player's message itself, where the others' messages it only passes on
/// (0x49120C's jump table, `network.md` §11.18): the chat request 0x48D9B0 is rewritten before anyone
/// else sees it (0x491324); the ping request `0xC3` (0x48D9F0) is answered to its sender alone
/// (0x4916B7); the exchange's samples (a type below `0xB0`, 0x49120C → 0x48A2C0), `0xB3`, `0xC5` and
/// `0xB7` are the room's own cases. What is left — `0xBC`, `0xBD`, `0xBE`, `0xBF` … `0xC2` — the room
/// relays (0x48C050, 0x48C0F0, 0x48C170).
fn msg_for_the_host(body: &[u8]) -> bool {
    match body.first() {
        // `0xFF` is the CONMAN's hello (0x55CED0 / 0x560420): the host's link layer answers it, nobody else gets it
        Some(&crate::msg::kind::CHAT | &crate::msg::kind::PING | &0xFF) => true,
        // the input exchange's messages to the room: a player's three input bytes (any type below
        // `0xB0`, 0x49120C -> 0x48A2C0), its `0xB3` request, `0xC5` (start) and `0xB7` (report)
        Some(&t) => t < 0xB0 || matches!(t, crate::msg::kind::START | 0xB7 | 0xC5),
        None => false,
    }
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
