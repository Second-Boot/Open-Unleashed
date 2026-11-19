//! **TEST GLUE** — a scripted stand-in for the internet lobby, built on [`crate::session::Net`], [`crate::host::HostedRoom`] and
//! [`crate::lobby`]. It plays the entry server and the lobby server of `lobby.md` §11 in one process (§11.12): one socket, the
//! same admission for the login link and for the link ENTER opens, the games and their rooms. It is not the lobby server — it
//! has no accounts (the account and persona names `bad`, `taken`, `old`, `full` script the refusals), no persistence, no
//! idle timeouts and no limits beyond what a test needs — and it is not part of the game: the application never starts it. The
//! example `lobby_stub` runs it from the command line for scripted front-end runs.
//!
//! Scripted answers (the account is the `c<account>~<password>` field of the `'j'` record, the persona the identity's first field):
//!
//! | input | answer |
//! |---|---|
//! | account `bad` | `0xC6 7` (invalid user name / password) |
//! | persona `taken` (or one already in the lobby) | `0xC6 8` |
//! | identity version other than `3.5` | `0xC6 0xA` |
//! | account `full` | `0xC6 0xB` |
//! | anything else | the admission of `lobby.md` §5.3 and `0xC6 0xE` |

use crate::host::{HostedRoom, RoomDesc};
use crate::lobby::{self, Game, GameDesc};
use crate::msg::kind;
use crate::room::{self, ChatLine, To};
use crate::session::{Event, Net};
use crate::socket::CommSocket;
use std::time::{Duration, Instant};

/// The room's 32 Hz loop: 15 625 µs, half the 64 Hz game tick.
const ROOM_TICK: Duration = Duration::from_micros(15_625);

/// How many players one game holds (`room::MAX_PLAYERS`).
const LIMIT: usize = room::MAX_PLAYERS;

struct Member {
    slot: u32,
    /// the lobby id (a 23-bit number shared with the games' ids, 0x490AF0)
    id: i32,
    persona: String,
    /// the identity's third field, the install's version
    version: String,
    /// admitted (the `0xC6 0xE` went out)
    in_lobby: bool,
    /// the game it is in and its id there
    game: Option<(usize, i32)>,
}

struct StubGame {
    id: u32,
    desc: GameDesc,
    room: HostedRoom,
    /// room player id -> member slot
    seats: Vec<(i32, u32)>,
    /// players of a seeded game (no link behind them), listed in its count
    phantom: u32,
}

impl StubGame {
    fn count(&self) -> u32 {
        self.seats.len() as u32 + self.phantom
    }
}

/// The stub lobby. Drive it with [`LobbyStub::step`] from a thread (or a test's loop).
pub struct LobbyStub {
    pub net: Net,
    /// the text of the one server row it lists (itself): `a.b.c.d.port.Name`
    pub row: String,
    members: Vec<Member>,
    /// the account of each link, from its `'j'` record
    accounts: Vec<(u32, String)>,
    games: Vec<Option<StubGame>>,
    /// `(lobby id, greeting)` of the seeded bots
    bots: Vec<(i32, String)>,
    counter: i32,
    last_tick: Instant,
    epoch: Instant,
    /// every login the stub saw: `(account, password, persona, version)` — for tests
    pub logins: Vec<(String, String, String, String)>,
    /// every message a member sent after its admission: `(persona, bytes)` — for tests
    pub heard: Vec<(String, Vec<u8>)>,
}

impl LobbyStub {
    /// Bind `port` (0 = any) on loopback-capable `0.0.0.0` and list itself as `127.0.0.1.<port>.<name>`.
    pub fn bind(port: u16, name: &str) -> std::io::Result<Self> {
        let sock = CommSocket::bind(port)?;
        let mut net = Net::host(sock, crate::session::name32("Lobby~3.5"), crate::comm::key32(&crate::comm::APP_GUID), 33);
        net.slot_cap = 32;
        // the server's room routes every message itself (`lobby.md` §8); the LAN host's automatic relay of a peer's datagrams is off
        net.relay = false;
        let real = net.local_addr()?.port();
        Ok(Self {
            net,
            row: format!("127.0.0.1.{real}.{name}"),
            members: Vec::new(),
            accounts: Vec::new(),
            games: Vec::new(),
            bots: Vec::new(),
            counter: 0,
            last_tick: Instant::now(),
            epoch: Instant::now(),
            logins: Vec::new(),
            heard: Vec::new(),
        })
    }

    /// A game that is already listed, with `players` players nobody drives (scripted front-end runs).
    pub fn seed_game(&mut self, desc: GameDesc, players: u32) {
        let id = self.next_id();
        let name = crate::comm::desc::name_field(&desc.name, &desc.version);
        let rd = RoomDesc { id, name, bytes: [0, desc.positional, desc.laps, 0], words: [desc.direction, desc.track] };
        self.games.push(Some(StubGame { id: id as u32, desc, room: HostedRoom::new(rd), seats: Vec::new(), phantom: players }));
    }

    /// A lobby member with no link, who greets every newcomer with `greeting` (a chat line to everybody).
    pub fn seed_bot(&mut self, persona: &str, greeting: &str) {
        let id = self.next_id();
        self.members.push(Member { slot: u32::MAX - self.bots.len() as u32, id, persona: persona.into(), version: "3.5".into(), in_lobby: true, game: None });
        self.bots.push((id, greeting.to_string()));
    }

    pub fn port(&self) -> u16 {
        self.net.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    fn clock(&self) -> u32 {
        3_600_000 + self.epoch.elapsed().as_millis() as u32
    }

    /// 0x490AF0: the next 23-bit number no player and no game uses.
    fn next_id(&mut self) -> i32 {
        let (members, games) = (&self.members, &self.games);
        let used = |i: i32| members.iter().any(|m| m.id == i) || games.iter().flatten().any(|g| g.id as i32 == i);
        let mut c = self.counter;
        let id = crate::host::next_id(&mut c, used);
        self.counter = c;
        id
    }

    fn send(&mut self, slot: u32, m: &[u8]) {
        if let Some(a) = self.net.peer_addr(slot) {
            let _ = self.net.send_game_to(&[a], m);
        }
    }

    fn lobby_slots(&self) -> Vec<u32> {
        self.members.iter().filter(|m| m.in_lobby && m.game.is_none()).map(|m| m.slot).collect()
    }

    fn counts(&self) -> (u16, u16) {
        let games = self.games.iter().flatten().count() as u16;
        let players = self.members.iter().filter(|m| m.in_lobby).count() as u16;
        (games, players)
    }

    /// One pass: the socket, the events, the periodic tick.
    pub fn step(&mut self) {
        for e in self.net.poll() {
            match e {
                Event::PeerJoined { slot, .. } => {
                    let rec = self.net.peers.get(slot as usize).and_then(|p| p.as_ref()).map(|p| p.record).unwrap_or([0; crate::comm::record::SIZE]);
                    let end = |b: &[u8]| b.iter().position(|&c| c == 0).unwrap_or(b.len());
                    let acct = &rec[crate::comm::record::PLAYER..crate::comm::record::IDENTITY];
                    let ident = &rec[crate::comm::record::IDENTITY..crate::comm::record::IDENTITY + crate::comm::record::IDENTITY_LEN];
                    let (persona, ver2, version) = crate::comm::identity::parse(&ident[..end(ident)]);
                    let (_, account, password) = lobby::parse_account_field(&acct[..end(acct)]);
                    self.logins.push((account.clone(), password, persona.clone(), version.clone()));
                    // the version gate rides in the second field (0x48A700)
                    let version = if ver2 == lobby::PROTOCOL_VERSION { version } else { format!("!{ver2}") };
                    self.members.push(Member { slot, id: -1, persona, version, in_lobby: false, game: None });
                    // the account name rides along for the decision
                    self.pending_account(slot, &account);
                }
                Event::PeerLeft { slot } => self.drop_member(slot),
                Event::Data { slot: Some(s), bytes, .. } => self.data(s, &bytes),
                _ => {}
            }
        }
        // the room's 32 Hz loop: the peer records' queues (0x48A4E0)
        self.net.pump_outbox();
        if self.last_tick.elapsed() >= ROOM_TICK {
            for e in self.net.tick() {
                if let Event::PeerLeft { slot } = e {
                    self.drop_member(slot);
                }
            }
            self.last_tick = Instant::now();
        }
    }

    // the account of a link is kept beside the member so the admission after the hello can decide
    fn pending_account(&mut self, slot: u32, account: &str) {
        self.accounts.retain(|(s, _)| *s != slot);
        self.accounts.push((slot, account.to_string()));
    }

    fn member(&self, slot: u32) -> Option<usize> {
        self.members.iter().position(|m| m.slot == slot)
    }

    fn data(&mut self, slot: u32, bytes: &[u8]) {
        let Some(mi) = self.member(slot) else { return };
        if !self.members[mi].in_lobby {
            // the CONMAN hello: `0xFE 01` first, then the decision (§2.2)
            if bytes.first() == Some(&0xFF) {
                self.send(slot, &[0xFE, 1]);
                self.decide(slot);
            }
            return;
        }
        self.heard.push((self.members[mi].persona.clone(), bytes.to_vec()));
        match self.members[mi].game {
            None => self.lobby_message(mi, bytes),
            Some((gi, pid)) => self.game_message(mi, gi, pid, bytes),
        }
    }

    fn decide(&mut self, slot: u32) {
        let Some(mi) = self.member(slot) else { return };
        let account = self.accounts.iter().find(|(s, _)| *s == slot).map(|(_, a)| a.clone()).unwrap_or_default();
        let m = &self.members[mi];
        let persona_used = self.members.iter().any(|o| o.in_lobby && o.persona == m.persona);
        let verdict = if account == "bad" {
            Some(room::comm_state::BAD_LOGIN)
        } else if m.persona == "taken" || persona_used {
            Some(room::comm_state::NAME_TAKEN)
        } else if m.version.starts_with('!') {
            Some(room::comm_state::WRONG_VERSION)
        } else if account == "full" {
            Some(room::comm_state::FULL)
        } else {
            None
        };
        if let Some(v) = verdict {
            self.net.peer_in(slot);
            self.send(slot, &room::state(v));
            // the client tears the link down; the record goes with its leave
            self.members.remove(mi);
            return;
        }
        self.admit(mi);
    }

    /// 0x4907C0, `lobby.md` §5.3: the newcomer is announced, joins the lobby, hears every game, every lobby member, the server rows and
    /// `0xC6 0xE`.
    fn admit(&mut self, mi: usize) {
        let id = self.next_id();
        self.members[mi].id = id;
        self.members[mi].in_lobby = true;
        let (slot, persona) = (self.members[mi].slot, self.members[mi].persona.clone());
        for s in self.lobby_slots() {
            if s != slot {
                self.send(s, &room::player_in(id, persona.as_bytes()));
            }
        }
        let games: Vec<Vec<u8>> = self.games.iter().flatten().map(|g| g.desc.message(g.id, g.count())).collect();
        for g in games {
            self.send(slot, &g);
        }
        let list: Vec<(i32, String)> = self.members.iter().filter(|m| m.in_lobby && m.game.is_none()).map(|m| (m.id, m.persona.clone())).collect();
        for (i, p) in list {
            self.send(slot, &room::player_in(i, p.as_bytes()));
        }
        let (g, p) = self.counts();
        let row = lobby::server_row(1, g, p, &self.row);
        self.send(slot, &row);
        self.send(slot, &room::state(room::comm_state::CONNECTED));
        self.net.peer_in(slot);
        for (bid, text) in self.bots.clone() {
            self.send(slot, &ChatLine { from: bid, to: room::TO_ALL, text: text.into_bytes() }.encode());
        }
    }

    fn lobby_message(&mut self, mi: usize, m: &[u8]) {
        let (slot, id) = (self.members[mi].slot, self.members[mi].id);
        match m.first().copied() {
            Some(kind::CHAT) => {
                if let Some(r) = room::ChatRequest::parse(m) {
                    let line = ChatLine { from: id, to: r.to, text: r.text }.encode();
                    let to_one = r.to > room::TO_ALL && self.members.iter().any(|o| o.in_lobby && o.id == r.to);
                    if to_one {
                        if let Some(o) = self.members.iter().find(|o| o.id == r.to).map(|o| o.slot) {
                            self.send(o, &line);
                        }
                        if r.to != id {
                            self.send(slot, &line);
                        }
                    } else {
                        for s in self.lobby_slots() {
                            self.send(s, &line);
                        }
                    }
                }
            }
            Some(kind::PING) => {
                if let Some(p) = room::Ping::parse(m) {
                    self.send(slot, &room::Ping { id: p.id, word: -1 }.encode());
                }
            }
            Some(kind::DESCRIPTION) => {
                // CREATE: id 0 (0x4913B1) — a new room from the descriptor, the creator moved in
                let Some(g) = Game::parse(m) else { return };
                if g.id != 0 {
                    return;
                }
                let gid = self.next_id();
                let name = crate::comm::desc::name_field(&g.desc.name, &g.desc.version);
                let rd = RoomDesc { id: gid, name, bytes: [0, g.desc.positional, g.desc.laps, 0], words: [g.desc.direction, g.desc.track] };
                let game = StubGame { id: gid as u32, desc: g.desc, room: HostedRoom::new(rd), seats: Vec::new(), phantom: 0 };
                let gi = match self.games.iter().position(|x| x.is_none()) {
                    Some(i) => {
                        self.games[i] = Some(game);
                        i
                    }
                    None => {
                        self.games.push(Some(game));
                        self.games.len() - 1
                    }
                };
                self.announce(gi);
                self.move_in(mi, gi);
            }
            Some(lobby::kind::JOIN_GAME) => {
                let gid = m.get(1..).and_then(crate::msg::read_u24).unwrap_or(0);
                let Some(gi) = self.games.iter().position(|x| x.as_ref().is_some_and(|g| g.id == gid)) else { return };
                let g = self.games[gi].as_ref().unwrap();
                if self.members[mi].version != g.desc.version {
                    self.send(slot, &room::state(room::comm_state::WRONG_VERSION));
                } else if g.seats.len() >= LIMIT {
                    self.send(slot, &room::state(room::comm_state::FULL));
                } else {
                    self.move_in(mi, gi);
                }
            }
            _ => {}
        }
    }

    fn announce(&mut self, gi: usize) {
        let Some(g) = self.games[gi].as_ref() else { return };
        let m = g.desc.message(g.id, g.count());
        for s in self.members.iter().filter(|m| m.in_lobby).map(|m| m.slot).collect::<Vec<_>>() {
            self.send(s, &m);
        }
    }

    /// 0x4924D0 + 0x48BB50 (`lobby.md` §7.4): out of the lobby, into the room.
    fn move_in(&mut self, mi: usize, gi: usize) {
        let (slot, id, persona) = (self.members[mi].slot, self.members[mi].id, self.members[mi].persona.clone());
        for s in self.lobby_slots() {
            if s != slot {
                self.send(s, &room::player_out(id));
            }
        }
        // the player's own lobby record goes too (`lobby.md` §7.4 reads "every remaining lobby member"; INFERRED to include it, or the
        // creator would not be the first record of its list)
        let rest: Vec<i32> = self.members.iter().filter(|m| m.in_lobby && m.game.is_none()).map(|m| m.id).collect();
        for i in rest {
            self.send(slot, &room::player_out(i));
        }
        self.members[mi].game = None;
        let g = self.games[gi].as_mut().unwrap();
        let pid = (0..LIMIT as i32).find(|p| !g.seats.iter().any(|(q, _)| q == p)).unwrap_or(0);
        g.seats.push((pid, slot));
        let out = g.room.admit(pid, persona.as_bytes());
        self.members[mi].game = Some((gi, pid));
        self.deliver(gi, out);
        // the room's count changed: every peer's games list
        self.announce(gi);
    }

    /// The room's output to its links (`To::All` = the room's players; a `0xB4` description goes to everyone, 0x490B70).
    fn deliver(&mut self, gi: usize, out: Vec<(To, Vec<u8>)>) {
        let Some(g) = self.games[gi].as_ref() else { return };
        let seats = g.seats.clone();
        for (to, m) in out {
            match to {
                To::All => {
                    if m.first() == Some(&kind::DESCRIPTION) {
                        for s in self.members.iter().filter(|m| m.in_lobby).map(|m| m.slot).collect::<Vec<_>>() {
                            self.send(s, &m);
                        }
                    } else {
                        for (_, s) in &seats {
                            self.send(*s, &m);
                        }
                    }
                }
                To::Player(p) => {
                    if let Some((_, s)) = seats.iter().find(|(q, _)| *q == p) {
                        self.send(*s, &m);
                    }
                }
            }
        }
    }

    fn game_message(&mut self, mi: usize, gi: usize, pid: i32, m: &[u8]) {
        match m.first().copied() {
            Some(lobby::kind::BACK) => self.leave_game(mi, gi, pid),
            Some(kind::DESCRIPTION) => {
                // the host re-sends the descriptor (0x4913B1, in-game arm)
                if let Some(g) = Game::parse(m)
                    && pid == 0
                    && let Some(game) = self.games[gi].as_mut()
                {
                    game.room.desc.name = crate::comm::desc::name_field(&g.desc.name, &g.desc.version);
                    game.room.desc.bytes = [0, g.desc.positional, g.desc.laps, 0];
                    game.room.desc.words = [g.desc.direction, g.desc.track];
                    game.desc = g.desc;
                    self.announce(gi);
                }
            }
            // relayed to the room: the barrier (the sender included), car state (the others), picture chunks, voice
            Some(b @ (kind::BARRIER | kind::DATA_FIRST | kind::DATA_MORE | 0xC1 | 0xC2)) => {
                let all = b == kind::BARRIER;
                let seats = self.games[gi].as_ref().map(|g| g.seats.clone()).unwrap_or_default();
                for (q, s) in seats {
                    if all || q != pid {
                        self.send(s, m);
                    }
                }
            }
            Some(kind::CAR_STATE) => {
                let seats = self.games[gi].as_ref().map(|g| g.seats.clone()).unwrap_or_default();
                for (q, s) in seats {
                    if q != pid {
                        self.send(s, m);
                    }
                }
            }
            Some(kind::EJECT) => {}
            // `0xB1 (id)` from the room's first player naming another player (0x4916E1…0x491765): the target hears `0xC6 9` and goes
            // back to the lobby (0x4925A0); from anybody else, or naming the sender, it is ignored here (the host's own is not modelled)
            Some(kind::LEAVE) => {
                let target = crate::msg::Message::parse(m).ok().map(|h| h.value);
                let first = self.games[gi].as_ref().and_then(|g| g.room.room.players.first().map(|p| p.id));
                let seat = self.games[gi].as_ref().and_then(|g| g.seats.iter().find(|(q, _)| Some(*q) == target).copied());
                if first == Some(pid)
                    && let Some((t, s)) = seat
                    && t != pid
                {
                    self.send(s, &room::state(crate::room::comm_state::EJECTED));
                    if let Some(omi) = self.member(s) {
                        self.leave_game(omi, gi, t);
                    }
                }
            }
            Some(_) => {
                let clock = self.clock();
                if let Some(g) = self.games[gi].as_mut() {
                    let out = g.room.handle(pid, m, clock);
                    self.deliver(gi, out);
                }
            }
            None => {}
        }
    }

    /// 0x4925A0 (`lobby.md` §7.6): out of the room, a new lobby id, `0xB0`s and every game for the returner.
    fn leave_game(&mut self, mi: usize, gi: usize, pid: i32) {
        let slot = self.members[mi].slot;
        let g = self.games[gi].as_mut().unwrap();
        let out = g.room.remove(pid);
        g.seats.retain(|(q, _)| *q != pid);
        let remaining: Vec<(i32, Vec<u8>)> = g.room.room.players.iter().map(|p| (p.id, p.name.clone())).collect();
        // what the returner hears: the leave of everybody left, then `0xB2 (-2, "")`
        for (i, _) in remaining {
            self.send(slot, &room::player_out(i));
        }
        self.send(slot, &room::your_id(-2, b""));
        self.deliver_after_leave(gi, out);
        let empty = self.games[gi].as_ref().is_some_and(|g| g.seats.is_empty() && g.phantom == 0);
        if empty {
            let id = self.games[gi].as_ref().unwrap().id;
            self.games[gi] = None;
            for s in self.members.iter().filter(|m| m.in_lobby).map(|m| m.slot).collect::<Vec<_>>() {
                self.send(s, &lobby::game_gone(id));
            }
        } else {
            self.announce(gi);
        }
        self.members[mi].game = None;
        let id = self.next_id();
        self.members[mi].id = id;
        let persona = self.members[mi].persona.clone();
        for s in self.lobby_slots() {
            if s != slot {
                self.send(s, &room::player_in(id, persona.as_bytes()));
            }
        }
        let list: Vec<(i32, String)> = self.members.iter().filter(|m| m.in_lobby && m.game.is_none()).map(|m| (m.id, m.persona.clone())).collect();
        for (i, p) in list {
            self.send(slot, &room::player_in(i, p.as_bytes()));
        }
        let games: Vec<Vec<u8>> = self.games.iter().flatten().map(|g| g.desc.message(g.id, g.count())).collect();
        for g in games {
            self.send(slot, &g);
        }
    }

    fn deliver_after_leave(&mut self, gi: usize, out: Vec<(To, Vec<u8>)>) {
        // the leaver is already off the seats: its own `0xB1` has no addressee
        self.deliver(gi, out.into_iter().filter(|(_, m)| m.first() != Some(&kind::DESCRIPTION)).collect());
    }

    fn drop_member(&mut self, slot: u32) {
        let Some(mi) = self.member(slot) else { return };
        if let Some((gi, pid)) = self.members[mi].game {
            let g = self.games[gi].as_mut().unwrap();
            let out = g.room.remove(pid);
            g.seats.retain(|(q, _)| *q != pid);
            self.deliver_after_leave(gi, out);
            if self.games[gi].as_ref().is_some_and(|g| g.seats.is_empty() && g.phantom == 0) {
                let id = self.games[gi].as_ref().unwrap().id;
                self.games[gi] = None;
                for s in self.members.iter().filter(|m| m.in_lobby && m.slot != slot).map(|m| m.slot).collect::<Vec<_>>() {
                    self.send(s, &lobby::game_gone(id));
                }
            } else {
                self.announce(gi);
            }
        } else if self.members[mi].in_lobby {
            let id = self.members[mi].id;
            for s in self.lobby_slots() {
                if s != slot {
                    self.send(s, &room::player_out(id));
                }
            }
        }
        self.members.remove(mi);
        self.accounts.retain(|(s, _)| *s != slot);
    }
}
