//! The **room host's protocol side** — what the original's room object (`NfsRoom`, 0x48B9B0…0x48C2B0) and the server dispatch
//! (0x491010 and its jump tables) do for the players of one game (`network.md` §10, §11.6-§11.18; `lobby.md` §7).
//!
//! It is pure protocol: plain data in, `(To, message)` lists out, no socket, no clock of its own (the caller passes the room's
//! `GetTickCount()` word) and nothing of the game's physics, front end or audio. One [`HostSide`] belongs to one room, so a process
//! can hold any number of them, and none of its methods assumes a local player: ids are the room's player ids (the host's own
//! player, when there is one, is id 0 and the application delivers `To::Player(0)` to itself; a dedicated server has none and
//! maps every id to a link). The messages of a player that are *not* the room's business (link events, the CONMAN hello) stay in
//! [`crate::session`].
//!
//! [`HostedRoom`] bundles a [`Room`] with a [`HostSide`] for a host that has no local player: it applies the room's own broadcasts
//! to its copy of the room the way a player's `hear` does, which is the part a game window does through `NetPlay::hear`.

use crate::exchange::{self, Out};
use crate::msg::kind;
use crate::room::{self, ChatRequest, Heard, Ping, Room, To};
use std::collections::HashMap;

/// `room+0x70`, the 24-bit value of the room's `0xB4` (0x48BAA0 pushes it, 0x48BBBE): the room's session number. It is the manager's
/// counter `[mgr+0x150]` (0x491CF5 sets it to −1) incremented by 0x490AF0 (`& 0x7FFFFF`) until no room (`+0x70`, lists `+0x174` /
/// `+0x178`) and no player (`+0xAC`, list `+0x164`) already has it, and stored by the room's constructor 0x48B9B0 (0x48B9D7, the
/// argument 0x491402 / 0x491A07 computes). The first room of a session skips 0 because the host's own player (id 0) is in the
/// list, so its value is **1** — the value of every capture (one room per run). HIGH: the player list's identity at 0x490B0E was
/// read, not run. A server with many rooms numbers them with [`next_id`].
pub const FIRST_ROOM_ID: i32 = 1;

/// 0x490AF0: the next number of the manager's 23-bit counter (`& 0x7FFFFF`) that no room or player already uses.
pub fn next_id(counter: &mut i32, used: impl Fn(i32) -> bool) -> i32 {
    loop {
        *counter = (*counter + 1) & 0x7F_FFFF;
        if !used(*counter) {
            return *counter;
        }
    }
}

/// What a room announces about itself: the `0xB4` record's inputs (the room's number, its name field `Name~version`, the four
/// descriptor bytes and the two descriptor words, `room+0x70` / `+0x74…`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomDesc {
    pub id: i32,
    pub name: [u8; 32],
    pub bytes: [u8; 4],
    pub words: [u16; 2],
}

impl RoomDesc {
    /// The description a session's `'d'` already carries.
    pub fn of(net: &crate::session::Net, id: i32) -> Self {
        Self { id, name: net.name, bytes: net.desc_bytes, words: net.desc_words }
    }

    /// The `0xB4` for `count` players (0x48BAA0).
    pub fn message(&self, count: u32) -> Vec<u8> {
        room::description(self.id, &self.name, self.bytes, self.words, count)
    }

    /// The room's name without the `~version` (what the `0xB2` carries, 0x48BC1B).
    pub fn short_name(&self) -> &[u8] {
        let end = self.name.iter().position(|&c| c == 0 || c == b'~').unwrap_or(self.name.len());
        &self.name[..end]
    }
}

/// The host-only state of one room that is not the [`Room`] list itself: the input exchange's room half and the pings the players
/// last reported. Every method returns what the host sends; the caller delivers it (and, if it plays, hears its own copy).
#[derive(Debug, Default)]
pub struct HostSide {
    /// the room object's input-exchange half (0x48B6B0 …, `+0xCC0…`)
    pub server: exchange::Server,
    /// the host's `peer+0xB0` per player id: what each player last reported (−1 until it has)
    pub reported: HashMap<i32, i32>,
}

impl HostSide {
    pub fn new() -> Self {
        Self::default()
    }

    /// **0x48BB50** — the room admits player `id`. The order is the captured one (REFERENCE-2026-10-02-SRVX, the original host): the
    /// description `0xB4` (0x48BBC7), the room's recorded option messages of every player already in (0x48BBEE…0x48BBFC, only the
    /// newcomer hears them), `0xB2` (0x48BC1B), the `0xB0`s (and `0xC5`s) of the room, the second description with the new count
    /// to **everyone** (0x48BCE3…0x48BCEE: 0x490B70 walks `mgr+0x164`), then `0xC6 (0xE)`.
    /// Also sets the newcomer's bits in the exchange room (0x48BB66). The caller's link layer has answered the CONMAN hello first.
    pub fn admit(&mut self, room: &Room, desc: &RoomDesc, id: i32, name: &[u8]) -> Vec<(To, Vec<u8>)> {
        let count = room.players.len() as u32;
        let to_new = |m: Vec<u8>| (To::Player(id), m);
        let mut out = vec![to_new(desc.message(count))];
        out.extend(room.replay_all().into_iter().map(to_new));
        out.push(to_new(room::your_id(id, desc.short_name())));
        let mut adm = room.admit(id, name);
        let c6 = if adm.last().map(|m| m.1.first()) == Some(Some(&0xC6)) { adm.pop() } else { None };
        out.extend(adm);
        out.push((To::All, desc.message(count + 1)));
        out.extend(c6);
        // 0x48BB66: the newcomer's bit in the exchange room's `+0xCC0` and `+0xCC2`
        if let Ok(slot) = usize::try_from(id) {
            self.server.join(slot);
        }
        out
    }

    /// 0x48BDA0's tail (0x48BEB7…0x48BEC8): the room's description to every player with the count the room has **now** (so call it
    /// after the room has heard the `0xB1`).
    pub fn describe(&self, room: &Room, desc: &RoomDesc) -> (To, Vec<u8>) {
        (To::All, desc.message(room.players.len() as u32))
    }

    /// 0x48BDA0: a player in the race is reported out (0x48C220), then its bit leaves `+0xCC0`.
    pub fn exchange_leave(&mut self, id: i32) {
        if let Ok(slot) = usize::try_from(id) {
            self.server.out_of_race(slot);
            self.server.leave(slot);
        }
    }

    /// 0x48BE05: the room tells everyone left that the player has gone.
    pub fn player_out(&self, id: i32) -> (To, Vec<u8>) {
        (To::All, room::player_out(id))
    }

    /// **The host's chat case, 0x4912D4…0x4913AC**: a player's `0xBB` request becomes the room's lines ([`Room::route_chat`]).
    pub fn chat(&self, room: &Room, from: i32, bytes: &[u8]) -> Vec<(To, Vec<u8>)> {
        ChatRequest::parse(bytes).map_or_else(Vec::new, |r| room.route_chat(from, r.to, &r.text))
    }

    /// **0x48C050** — a player's `0xBC` record: filed on the sender (0x48A2F0), relayed to the players in its lobby group, and
    /// 0x48C0A4's echo to the sender itself (the list includes the sender: an original joiner keeps its own `.READY` from it).
    pub fn options(&self, room: &mut Room, from: i32, bytes: &[u8]) -> Vec<(To, Vec<u8>)> {
        room.file_options(from, bytes);
        let mut out = room.relay_options(from, bytes);
        out.push((To::Player(from), bytes.to_vec()));
        out
    }

    /// **The host's case 195, 0x491661…0x4916CE**: the sender's word is kept (`peer+0xB0`); then for the room's player with the
    /// asked id the sender is answered `0xC3(id, that player's peer+0xB0)` (0x4916B7…0x4916BC).
    pub fn ping(&mut self, room: &Room, from: i32, p: Ping) -> Vec<(To, Vec<u8>)> {
        self.reported.insert(from, p.word);
        if room.player(p.id).is_some() {
            let word = self.reported.get(&p.id).copied().unwrap_or(-1);
            return vec![(To::Player(from), Ping { id: p.id, word }.encode())];
        }
        Vec::new()
    }

    /// **The host's room, one player's request** (`0xB3` 0x4917C0, `0xC5` 0x49185B, `0xB7` 0x4918BD): the room object answers every
    /// link ([`Room::start_request`], `go_request`, `race_time_request`) and the exchange half follows (0x48BF10 / 0x48BFB0 /
    /// 0x48C220). `clock` is the room's `GetTickCount()` word. `None` for any other message.
    pub fn race_request(&mut self, room: &mut Room, from: i32, m: &[u8], clock: u32) -> Option<Vec<(To, Vec<u8>)>> {
        Some(match m.first().copied() {
            Some(kind::START) => {
                tracing::info!(target: "netplay", player = from, "room: Start Race (0xB3 request)");
                let out = room.start_request(from, clock);
                // 0x48BF10's other half (network.md §11.17): a race the room has just started has fresh exchange buffers (the ring
                // zeroed, the head and `+0xCC1` cleared, `+0xCC4` clear — 0x48BF51…0x48BF84), and the player's frame pointer starts at 0
                if !out.is_empty() {
                    self.server.started = false;
                    self.server.running = false;
                }
                if let Ok(slot) = usize::try_from(from) {
                    let _ = self.server.ready(slot, clock);
                }
                out
            }
            Some(kind::GO) => {
                let out = room.go_request(clock);
                // 0x48BFB0's other half: the frame and send clocks start at the go
                if !out.is_empty() {
                    let _ = self.server.go(clock);
                }
                out
            }
            Some(kind::RACE_TIME) => {
                let head = m.get(1..).and_then(crate::msg::read_u24).map_or(0, crate::msg::sign_extend_24);
                let best = m.get(4..8).map_or(0, |w| i32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                match head {
                    -2 => {
                        tracing::info!(target: "netplay", player = from, "room: Returning to frontend (0xB7)");
                        // 0x48C220's exchange half: the player is out of the race again; the last one out ends the started room
                        if let Ok(slot) = usize::try_from(from) {
                            self.server.out_of_race(slot);
                        }
                    }
                    -1 => tracing::info!(target: "netplay", player = from, "room: Leaving game (0xB7)"),
                    t => tracing::info!(
                        target: "netplay",
                        player = from,
                        "room: Finish {:02}:{:02}.{:02}, Best Lap {:02}:{:02}.{:02}",
                        t / 0xF00,
                        (t / 64) % 60,
                        (t & 63) * 100 / 64,
                        best / 0xF00,
                        (best / 64) % 60,
                        (best & 63) * 100 / 64
                    ),
                }
                room.race_time_request(from, head)
            }
            _ => return None,
        })
    }

    /// **0x48C2B0** — an input-exchange sample (a message below `0xB0`, three bytes) from player `from`.
    pub fn input(&mut self, from: i32, bytes: &[u8]) {
        if let (Ok(slot), true) = (usize::try_from(from), bytes.len() >= 3 && bytes[0] < 0xB0) {
            self.server.input(slot, [bytes[0], bytes[1], bytes[2]]);
        }
    }

    /// **0x48B6B0**, for as long as the room is started (see [`exchange::Server::tick`]): the `0xC4` heartbeat to everyone, each
    /// player's next frame and the drop of a player 128 frames behind. `can_send(id)` says whether player `id`'s link takes
    /// another frame now. Empty while the room is not started.
    pub fn tick(&mut self, room: &Room, now: u32, can_send: &mut dyn FnMut(usize) -> bool) -> Vec<Out> {
        if !self.server.started {
            return Vec::new();
        }
        let players: Vec<usize> = room.players.iter().filter_map(|p| usize::try_from(p.id).ok()).filter(|i| *i < 8).collect();
        self.server.tick(now, &players, can_send)
    }
}

/// A room with its host side, for a host that has **no local player** (a dedicated server): the application-free equivalent of what
/// `NetPlay` is for the room's creator. [`HostedRoom::emit`] applies the room's own broadcasts to this copy of the room, as the
/// original's host does when its room object's links include its own game.
#[derive(Debug)]
pub struct HostedRoom {
    pub room: Room,
    pub side: HostSide,
    pub desc: RoomDesc,
}

impl HostedRoom {
    /// A room with no player yet (`me = -1`: nobody here is a player).
    pub fn new(desc: RoomDesc) -> Self {
        Self { room: Room::new(-1), side: HostSide::new(), desc }
    }

    /// The room's own copy hears what is sent to everyone (the host's room object delivers to its list as well). The messages go on
    /// unchanged for the caller to put on the links.
    pub fn emit(&mut self, out: Vec<(To, Vec<u8>)>) -> Vec<(To, Vec<u8>)> {
        for (to, m) in &out {
            if *to == To::All {
                self.room.hear(m);
            }
        }
        out
    }

    /// Player `id` (named `name`) comes in; the link layer has answered its hello.
    pub fn admit(&mut self, id: i32, name: &[u8]) -> Vec<(To, Vec<u8>)> {
        let name = &name[..name.len().min(room::NAME_MAX)];
        let out = self.side.admit(&self.room, &self.desc, id, name);
        self.emit(out)
    }

    /// Player `id` left or was lost (link events 8/9/10, 0x48BDA0): out of the race, off the exchange room, `0xB1` to the others and
    /// the room's description with the new count when it was in.
    pub fn remove(&mut self, id: i32) -> Vec<(To, Vec<u8>)> {
        self.side.exchange_leave(id);
        let was_in = self.room.player(id).is_some();
        let mut out = self.emit(vec![self.side.player_out(id)]);
        if was_in {
            let d = self.side.describe(&self.room, &self.desc);
            out.extend(self.emit(vec![d]));
        }
        out
    }

    /// One message from player `id`'s link, after the admission: chat, options, race requests, pings and input samples. Returns what the
    /// host sends.
    pub fn handle(&mut self, id: i32, bytes: &[u8], clock: u32) -> Vec<(To, Vec<u8>)> {
        let out = match bytes.first().copied() {
            Some(kind::CHAT) => self.side.chat(&self.room, id, bytes),
            Some(kind::OPTIONS) => self.side.options(&mut self.room, id, bytes),
            Some(kind::START | kind::GO | kind::RACE_TIME) => self.side.race_request(&mut self.room, id, bytes, clock).unwrap_or_default(),
            Some(kind::PING) => Ping::parse(bytes).map_or_else(Vec::new, |p| self.side.ping(&self.room, id, p)),
            Some(b) if b < 0xB0 => {
                self.side.input(id, bytes);
                Vec::new()
            }
            _ => Vec::new(),
        };
        self.emit(out)
    }

    /// What a player's room message means to the room's own copy (for a server that wants to know, e.g. a `Start`).
    pub fn heard(&mut self, m: &[u8]) -> Option<Heard> {
        self.room.hear(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc() -> RoomDesc {
        RoomDesc { id: FIRST_ROOM_ID, name: crate::session::name32("Game~3.5"), bytes: [0, 1, 3, 0], words: [410, 7] }
    }

    #[test]
    fn a_dedicated_room_admits_players_in_the_captured_order() {
        let mut r = HostedRoom::new(desc());
        let first = r.admit(0, b"ann");
        let kinds: Vec<u8> = first.iter().map(|(_, m)| m[0]).collect();
        // description, your id, the room's list (empty, then the newcomer), the description again, the state
        assert_eq!(kinds, [0xB4, 0xB2, 0xB0, 0xB4, 0xC6]);
        assert_eq!(first[0].0, To::Player(0));
        assert_eq!(first[3].0, To::All);
        assert_eq!(r.room.players.len(), 1);
        let second = r.admit(1, b"bob");
        let kinds: Vec<u8> = second.iter().map(|(_, m)| m[0]).collect();
        assert_eq!(kinds, [0xB4, 0xB2, 0xB0, 0xB0, 0xB4, 0xC6]);
        // the count in the second description is the room's after the join
        assert_eq!(second[4].1[0x4C], 2);
        assert_eq!(r.room.players.len(), 2);
        assert!(r.side.server.joined & 0b11 == 0b11);
    }

    #[test]
    fn a_race_request_starts_the_room_and_the_last_one_out_ends_it() {
        let mut r = HostedRoom::new(desc());
        r.admit(0, b"ann");
        r.admit(1, b"bob");
        let out = r.handle(0, &room::start_request(), 1234);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (To::All, room::race_start(1234)));
        assert!(r.room.race_running());
        // a second request while running only moves the bit
        assert!(r.handle(1, &room::start_request(), 1300).is_empty());
        let out = r.handle(0, &room::go_request(), 1400);
        assert_eq!(out, vec![(To::All, room::go(1400))]);
        r.handle(0, &room::race_time_request(-2, -1), 1500);
        assert!(r.room.race_running());
        r.handle(1, &room::race_time_request(-2, -1), 1500);
        assert!(!r.room.race_running());
    }

    #[test]
    fn chat_and_pings_are_routed_and_a_leaver_is_announced() {
        let mut r = HostedRoom::new(desc());
        r.admit(0, b"ann");
        r.admit(1, b"bob");
        let out = r.handle(1, &ChatRequest { to: -1, text: b"hi".to_vec() }.encode(), 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, To::All);
        let out = r.handle(1, &Ping { id: 0, word: 42 }.encode(), 0);
        assert_eq!(out, vec![(To::Player(1), Ping { id: 0, word: -1 }.encode())]);
        let out = r.remove(1);
        assert_eq!(out[0], (To::All, room::player_out(1)));
        assert_eq!(out[1].0, To::All);
        assert_eq!(out[1].1[0], 0xB4);
        assert_eq!(out[1].1[0x4C], 1);
        assert_eq!(r.room.players.len(), 1);
    }

    #[test]
    fn room_numbers_skip_the_used() {
        let mut c = 0;
        assert_eq!(next_id(&mut c, |i| i == 1), 2);
        let mut c = 0x7F_FFFF;
        assert_eq!(next_id(&mut c, |_| false), 0);
    }
}
