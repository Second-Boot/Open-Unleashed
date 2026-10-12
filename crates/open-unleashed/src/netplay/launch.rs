//! The race launch: the launch records, the race options, the room's race requests and the race times.

use super::*;

impl NetPlay {
    /// **0x4B4DA0's walk** (0x4B4EF5…0x4B4FC7): the records this machine's walk sends are appended
    /// with 0x48D490 (the buffer is sent first when a record would take it past 0xF0) and the rest
    /// goes out with 0x48D440 — several records to a message, not one each as the selection path
    /// (0x48D550) sends them. On the host the list holds the broadcast bands, its own slot's records
    /// and the traffic cars'; on a player only its own slot's ([`Self::own_launch_records`]) — every
    /// machine runs this walk. What a machine sends it holds already (the host's own table is the
    /// source; a player's own records are applied by `own_launch_records`), so nothing is heard back.
    pub fn send_launch(&mut self, stream: &[Vec<u32>]) {
        let mut out = nfs_net::options::Outgoing::new();
        let mut msgs = Vec::new();
        for r in stream {
            msgs.extend(out.push(r));
            tracing::info!(target: "netplay", record = ?r, "launch record sent (0xBC)");
        }
        msgs.extend(out.flush());
        for m in msgs {
            if self.is_host() {
                self.room.file_options(self.id as i32, &m);
            }
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "launch record not sent");
            }
        }
    }

    /// **The own-slot arm of 0x4B4DA0's walk** (0x4B4F25…0x4B4F66): the records the launch chain left
    /// for this player's slot (`OwnCar::launch_list`, at the player id `[net+0xDC]`) go out with the
    /// slot rewritten to this machine's **race index** — `player+0x4C`, its place in the room's list
    /// among the players `net+0xE5` marks ([`Self::race_indices`]). Returns them for the caller to
    /// send, and applies them to this machine's own table at that index — where the original hears
    /// them back from the host's relay (0x48C050 sends to the sender too; this port's relay leaves
    /// the sender out, `network.md` §8.3).
    pub fn own_launch_records(&mut self, own: &nfs_net::launchcar::OwnCar) -> Vec<Vec<u32>> {
        let me = self.my_player_id();
        let index = self.race_indices().into_iter().find(|(id, _)| *id == me).map_or(0, |(_, i)| i);
        let players = self.room.players.len() as u32;
        let recs: Vec<Vec<u32>> = own
            .launch_list(me.max(0) as u32)
            .into_iter()
            .map(|mut r| {
                r[1] = index;
                r
            })
            .collect();
        for r in &recs {
            self.launch.take_car(r, players);
        }
        recs
    }

    /// The car record every **other** player of the race holds at its race index, in the room's
    /// order — the seat order ([`Self::remote_players`]): `(player id, race index, name, record)`.
    pub fn remote_cars(&self) -> Vec<(i32, u32, String, nfs_net::launchcar::CarSlot)> {
        let me = self.my_player_id();
        self.race_indices()
            .into_iter()
            .filter(|(id, _)| *id != me)
            .map(|(id, idx)| {
                let name = self.room.players.iter().find(|p| p.id == id).map(|p| p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect()).unwrap_or_default();
                (id, idx, name, self.launch.cars.get(&idx).cloned().unwrap_or_default())
            })
            .collect()
    }

    /// The race indices 0x4B4DA0 hands out (0x4B4EBC, 0x4B4ED2): the players `net+0xE5` marks, in the
    /// room's list order, numbered from 0 (`player+0x4C`); `(player id, index)`. With no start
    /// heard yet (`+0xE5 == 0`) every player of the room counts.
    pub fn race_indices(&self) -> Vec<(i32, u32)> {
        let all = self.in_race == 0;
        self.room
            .players
            .iter()
            .filter(|p| all || self.in_race.checked_shr(p.id as u32).is_some_and(|b| b & 1 != 0))
            .enumerate()
            .map(|(i, p)| (p.id, i as u32))
            .collect()
    }

    /// The option records a `0xBC` brought since the last call, in order.
    pub fn take_options(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.options_in)
    }

    /// **0x48D550** on the host: the stream goes to every player, one `0xBC` per record (the
    /// port's records are all two dwords, `(id, value)`). The host's own option table is written by
    /// the caller — the original's launch applies the whole stream locally first (0x4B4DFD).
    pub fn send_options(&mut self, stream: &[(u32, u32)]) {
        if !self.is_host() {
            return;
        }
        let recs: Vec<Vec<u32>> = stream.iter().map(|&(id, v)| vec![id, v]).collect();
        for m in nfs_net::options::messages_each(&recs) {
            self.room.file_options(self.id as i32, &m);
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "option record not sent");
            }
        }
        for &(id, v) in stream {
            self.sent.insert(id, v);
            tracing::info!(target: "netplay", id = format_args!("{id:#x}"), value = v, "option record sent (0xBC)");
        }
    }

    /// **0x4BC240 on a mode-2 accessor** (`Track1.Track` / `.Direction` / `.Laps` are built with
    /// mode 2 — 0x4FA4F0 pushes 2 into 0x4BC1F0, 0x4BB7A0 / 0x4BB680 take it as their 4th argument
    /// — and 0x4B7F70 lets a mode-2 accessor send when this machine hosts the session): its slot 1
    /// encodes `(id, value)` and 0x48D550 sends it. The original sends on every selection call;
    /// the port calls this each front-end tick with the accessor's current value and sends when it
    /// differs from what was last sent — the same records at the moments a user changes them,
    /// minus the repeats of an unchanged value. The first value seen for an id is the accessor's
    /// own starting state (they are set up before any session exists, so nothing is sent for it).
    /// Answers whether a record went out.
    pub fn sync_option(&mut self, id: u32, value: u32) -> bool {
        if !self.is_host() {
            return false;
        }
        match self.sent.get(&id) {
            Some(v) if *v == value => return false,
            None => {
                self.sent.insert(id, value);
                // the original's accessor sends on every selection, and the host's own loopback files the record on its player (0x48A2F0) whether or
                // not anybody is in the room yet — the later newcomer is replayed it (0x48A390). The wire has nobody to tell yet.
                for m in nfs_net::options::messages_each(&[vec![id, value]]) {
                    self.room.file_options(self.id as i32, &m);
                }
                return false;
            }
            Some(_) => {}
        }
        self.send_options(&[(id, value)]);
        true
    }

    /// The game's descriptor as the `'d'` carries it (`nfs_net::comm::desc`), which 0x509B00 puts
    /// together for 0x48DCB0 when the session is created.
    pub fn set_descriptor(&mut self, positional: u8, laps: u8, direction: u16, track: u16) {
        self.net.desc_bytes = [0, positional, laps, 0];
        self.net.desc_words = [direction, track];
    }

    /// The room's word for "now": `0x48A9A0()` is a bare `jmp [GetTickCount]` (no offset), so the host's word is
    /// the machine's tick counter; a player sets its clock offset from the word (`[clock] = word −
    /// GetTickCount()`, 0x48A9B0 at 0x48F8C8, [`nfs_net::exchange::Clock::set`]) and reads its own word
    /// through 0x48AA00 (`GetTickCount() + offset`). Only the difference of two clocks is ever used, so the
    /// counter's origin (the port's is the session's start, [`NetPlay::now`]) cannot be seen — settled, AC 2026-09-30.
    fn room_clock(&self) -> u32 {
        // the same scale as the exchange room's heartbeat and frame clocks ([`NetPlay::now`]), which the
        // players' synchronised clock is set from (0x48F8C8) and the go's word is read against
        self.now()
    }

    /// **The host's room, one player's request** (`0xB3` 0x4917C0, `0xC5` 0x49185B, `0xB7` 0x4918BD): the
    /// room object answers every link ([`nfs_net::room::Room::start_request`], `go_request`,
    /// `race_time_request`). A request is a one-byte `0xB3` / `0xC5` and an 8-byte `0xB7` (head =
    /// the sender's time or −1 / −2, word = its best lap). 0x4929B0 logs the `0xB7` as
    /// `"Finish %02d:%02d.%02d, Best Lap %02d:%02d.%02d"`, `"Leaving game"` or `"Returning to
    /// frontend"` (0x492CF0).
    pub(super) fn serve_race_request(&mut self, from: i32, m: &[u8]) {
        let clock = self.room_clock();
        let out = match m.first().copied() {
            Some(kind::START) => {
                tracing::info!(target: "netplay", player = from, "room: Start Race (0xB3 request)");
                let out = self.room.start_request(from, clock);
                // 0x48BF10's other half (network.md §11.17): a race the room has just started has fresh
                // exchange buffers (the ring zeroed, the head and `+0xCC1` cleared, `+0xCC4` clear —
                // 0x48BF51…0x48BF84), and the player's frame pointer starts at 0
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
                let out = self.room.go_request(clock);
                // 0x48BFB0's other half: the frame and send clocks start at the go
                if !out.is_empty() {
                    let _ = self.server.go(clock);
                }
                out
            }
            Some(kind::RACE_TIME) => {
                let head = m.get(1..4).map_or(0, |b| nfs_net::msg::sign_extend_24(u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2])));
                let best = m.get(4..8).map_or(0, |w| i32::from_be_bytes([w[0], w[1], w[2], w[3]]));
                match head {
                    -2 => {
                        tracing::info!(target: "netplay", player = from, "room: Returning to frontend (0xB7)");
                        // 0x48C220's exchange half: the player is out of the race again; the last one out
                        // ends the started room
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
                self.room.race_time_request(from, head)
            }
            _ => return,
        };
        self.dispatch(out);
    }

    /// One request of ours to the room: the host's own room object is a player's link away in the
    /// original (loopback) and here a call; a player sends it to the host.
    fn request(&mut self, m: Vec<u8>) {
        if self.is_host() {
            let me = self.id as i32;
            self.serve_race_request(me, &m);
        } else if let Err(e) = self.net.send_game(&m) {
            tracing::warn!(error = %e, "room request not sent");
        }
    }

    /// **0x48D370** — PLAYGAME's `0xB3` request (0x50CF73), on every machine: the room takes this
    /// player out of the lobby group and, when no race is running, starts one for everybody.
    pub fn request_start(&mut self) {
        self.request(room::start_request());
    }

    /// **0x48D3A0** — the race frame's `0xC5` request (0x4153AE), sent while `net+0xC5` is clear and
    /// the connection is up.
    pub fn request_go(&mut self) {
        if !self.aborted && self.established() {
            self.request(room::go_request());
        }
    }

    /// **0x48DA60**`(a, b)` — `0xB7` to the room with the head `a` and the word `b`, and `+0xBD = 1`;
    /// needs the session and its link (0x48DA67…0x48DA78).
    pub fn send_race_time(&mut self, a: i32, b: i32) {
        if !self.link_up() {
            return;
        }
        self.sent_time = true;
        self.request(room::race_time_request(a, b));
    }

    /// The abort arm PLAYGAME's wait, the barrier and the session loop share (0x50D028…0x50D04D,
    /// 0x48CF0E…0x48CF1D, 0x4B6D67…): with the connection up and `net+0xC5` clear, `+0xC5 = 1`,
    /// `+0xBC = 1`, `+0xC4 = 0`, and `0xB7 (−2, −1)` — "returning to the front end".
    pub fn abort_race(&mut self) {
        if self.established() && !self.aborted {
            self.aborted = true;
            self.go_wait = false;
            self.send_race_time(-2, -1);
        }
    }

    /// **0x48DAD0** — the session loop's normal exit (0x4B6F5B, after the race is shut down): a player
    /// that has sent no `0xB7` since the race started says `(−1, −1)` ("leaving game" — its time
    /// stays −1), then `(−2, −1)` for everyone — its bit leaves `net+0xE5` on every machine.
    pub fn leave_race(&mut self) {
        if !self.established() {
            return;
        }
        // (test glue's view of the data layer: the reliable layer's counters, `network.md` §11.18)
        tracing::info!(target: "netplay", link = %self.net.report(), "data layer at the race's end");
        if !self.sent_time {
            self.send_race_time(-1, -1);
        }
        self.send_race_time(-2, -1);
    }

    /// **0x48D770(race index)** — the finish time of the player whose `+0x4C` is `index` (`+0x50`), −1
    /// when there is none, the connection is down, or (with `COMMSPOSITIONAL` set) another player
    /// that has not finished reports a smaller `+0x48` than the time (0x48D7A5…0x48D7E5).
    pub fn race_time(&self, index: u32) -> i32 {
        if !self.link_up() {
            return -1;
        }
        let table = self.index_table();
        let (mut mine, mut floor) = (-1, -1);
        for p in &self.room.players {
            let idx = table.iter().find(|(id, _)| *id == p.id).map(|(_, i)| *i);
            if idx == Some(index) {
                mine = p.time;
            } else if p.time == -1 && (floor == -1 || p.progress < floor) {
                floor = p.progress;
            }
        }
        if !self.positional || floor == -1 || mine < floor {
            return mine;
        }
        -1
    }

    /// **0x48D7F0(race index)** — the finish place: 1 + the players whose `+0x50` is above −1 and
    /// below this one's time; −1 while 0x48D770 has no time for it.
    pub fn race_rank(&self, index: u32) -> i32 {
        let t = self.race_time(index);
        if t < 0 {
            return -1;
        }
        1 + self.room.players.iter().filter(|p| p.time > -1 && p.time < t).count() as i32
    }

    /// Whether the host has announced the race since the last call.
    pub fn take_start(&mut self) -> bool {
        std::mem::take(&mut self.start_due)
    }

    /// Where a joiner joined (or is joining).
    pub fn host_addr(&self) -> Option<SocketAddr> {
        self.net.host
    }

    /// How many players the room holds (`net+0x260`'s length).
    /// `[net+0xDC]`, this machine's player id (-1 before it has one).
    pub fn my_player_id(&self) -> i32 {
        self.room.me
    }

    /// The `.STARTINGPOS` records the host drew, kept as if heard (the host's own table takes them).
    pub fn note_grid(&mut self, recs: &[Vec<u32>]) {
        for r in recs {
            self.launch.take(r);
        }
    }

    pub fn room_len(&self) -> usize {
        self.room.players.len()
    }

    pub fn is_host(&self) -> bool {
        self.net.role == Role::Host
    }

    /// The opponent seats the network owns. Called once the race has built its opponents.
    pub fn reserve(&mut self, seats: Vec<usize>) {
        self.seats = seats;
        self.next_seat = 0;
        self.remotes.clear();
        self.driven.clear();
        self.exchange = false;
    }

    /// The seats the network owns this tick. In an input-exchange race that is every seat with a player
    /// behind it (the launch's numbering, `player+0x4C`): each is a car the same simulation runs on the
    /// frames' inputs, from the first tick. In a positional race **every reserved seat**: the cars of the
    /// other players are human cars from the start (`car+0xDA8 == 0`), and one whose player has not sent a
    /// block yet (`car+0xDAC == 0`) is driven by the local pad (0x4991E0, [`SeatArm::Pad`]) — a seat is not
    /// the network's only once a block has come, as it was until that change.
    pub fn owned_seats(&self) -> Vec<usize> {
        if self.exchanging() {
            return (0..self.seats.len()).filter(|k| self.seat_race_index(*k).is_some()).collect();
        }
        let mut v = self.seats.clone();
        for s in self.driven.iter().copied().chain(self.remotes.values().map(|r| r.seat)) {
            if !v.contains(&s) {
                v.push(s);
            }
        }
        v
    }
}
