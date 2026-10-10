//! The session itself: start-up, the dispatch of the room's answers, the receive side (`hear`, `poll`) and the admission of peers.

use super::*;

impl NetPlay {
    /// `host`, `host:PORT`, `join:HOST[:PORT]`. Anything else is an error rather than a guess.
    pub fn start(spec: &str, who: &str) -> anyhow::Result<Self> {
        Self::start_named(spec, who, &format!("{who}'s race"))
    }

    /// [`NetPlay::start`] with the session's name — `Comms.GameName`'s text, which 0x509B00 puts
    /// into the descriptor (0x509B41) whatever it is, empty included.
    pub fn start_named(spec: &str, who: &str, game_name: &str) -> anyhow::Result<Self> {
        let (mode, rest) = spec.split_once(':').unwrap_or((spec, ""));
        let mut join_target = None;
        let net = match mode {
            "host" => {
                let port: u16 = if rest.is_empty() {
                    port()
                } else {
                    rest.parse()?
                };
                let sock = CommSocket::bind(port)?;
                // 0x48DD97…0x48DDF8: the session's name is `<Comms.GameName>~<version>`
                let name = nfs_net::comm::desc::name_field(game_name, &game_version());
                // **TEST GLUE** (not in the original): `NFS_NET_LIMIT=n` sets the player limit
                // (`session+0x84`, 8 in the game: 0x509B49), so a "game full" refusal (`-2`, 0x588F3D)
                // can be reached with two instances instead of eight
                let limit = std::env::var("NFS_NET_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(nfs_net::session::DEFAULT_LIMIT);
                let mut n = with_probe(Net::host(sock, name, key(), limit));
                n.advertise = advertised_record();
                n.my_name = name16(who);
                // 0x588EBF…0x588ED3 → 0x48A700: the version (the game-name test 0x49151E… is the
                // lobby's `0xB6` room request only, never a LAN / direct join — `network.md` §11.28)
                n.identity_gate = Some(game_version());
                tracing::info!(addr = %n.local_addr()?, "hosting");
                n
            }
            "join" => {
                let target = if rest.contains(':') {
                    rest.to_string()
                } else {
                    format!("{rest}:{}", port())
                };
                let host: SocketAddr = target
                    .to_socket_addrs()?
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("cannot resolve {target}"))?;
                let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(
                    Ipv4Addr::UNSPECIFIED,
                    0,
                )))?;
                let mut n = with_probe(Net::client(sock, key(), name16(who)));
                // The original's connect to a typed address (0x48AC90 → 0x55D370, the CONMAN's connect
                // 0x55C6E8 → the enumerate slot 0x588030) sends **one** datagram, the `'j'` (0x588030
                // builds it: the host's address at +0x04, the player record from +0x28), and nothing
                // before it — a real two-instance capture shows no `'w'`, `'r'` or `'d'` on this path
                // (`REFERENCE-2026-10-02-SRVE.md` §1). 0x588130 repeats the same buffer until the `'a'`.
                n.join(host, &join_identity(who))?;
                join_target = Some(host);
                tracing::info!(%host, addr = %n.local_addr()?, "joining");
                n
            }
            // `accountlogin`'s SIGNON (0x5109B0 → 0x48E1A0(net, 1, 0, "porsche-entry.earacing.com")): a connect to the
            // EA lobby service, which no longer exists — the session is a client that never hears an answer, so the
            // connect's 16 s deadline ends it in state 4 (830)
            "signon" => {
                let sock = CommSocket::bind_on(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))?;
                with_probe(Net::client(sock, key(), name16(who)))
            }
            other => anyhow::bail!(
                "--net wants `host`, `host:PORT` or `join:HOST[:PORT]`, not {other:?}"
            ),
        };
        let id = if net.role == Role::Host { 0 } else { u8::MAX }; // a client's id arrives with its 'a'
        let mut n = Self {
            net,
            id,
            remotes: HashMap::new(),
            driven: Vec::new(),
            seats: Vec::new(),
            positional: true,
            room: Room::new(if id == u8::MAX { -1 } else { id as i32 }),
            chat: Vec::new(),
            chat_open: false,
            player_sel: None,
            dirty: true,
            started: Instant::now(),
            pictures: HashMap::new(),
            next_seat: 0,
            lagged: VecDeque::new(),
            start_due: false,
            join: if mode == "host" { JoinState::Hosting } else { JoinState::Pending(Instant::now()) },
            join_wait: join_target.map(|h| (h, who.to_string(), Instant::now())),
            hello_wait: Default::default(),
            options: HashMap::new(),
            options_in: Vec::new(),
            sent: HashMap::new(),
            launch: Default::default(),
            barrier: 0,
            last_poll: Instant::now(),
            slots: Default::default(),
            state_in: None,
            lost: false,
            in_race: 0,
            pings: HashMap::new(),
            net148: tick_ms(),
            reported: HashMap::new(),
            launch_index: Vec::new(),
            exch: Default::default(),
            server: Default::default(),
            exchange: false,
            timer_anchor: None,
            timer_done: 0,
            commands: [0; 8],
            go_wait: false,
            go_clock: None,
            aborted: false,
            sent_time: false,
            tick_override: None,
        };
        // the host's own player is the room's first, id 0: in the original the host's game
        // joins the room object like any other (0x48BB50 hands out the lowest clear bit of
        // `room+0xCC0`, and the room is empty)
        if n.is_host() {
            let out = n.room.admit(0, &who.as_bytes()[..who.len().min(room::NAME_MAX)]);
            n.dispatch(out);
        }
        Ok(n)
    }

    /// Deliver what the room object sends — the host's half (0x48C870 on the room's links). Every
    /// link means every peer and the host's own player; one player's link is that peer's address,
    /// or the host itself for id 0.
    pub(super) fn dispatch(&mut self, out: Vec<(To, Vec<u8>)>) {
        for (to, m) in out {
            match to {
                To::All => {
                    // (two originals joining one port host at the same moment, live): the room's message goes to the
                    // room's players. A peer whose `'j'` is answered but whose CONMAN hello has not come yet (`hello_wait`) has
                    // no player object in the original (it is made when the link reports "connected", 0x48F0C4 event 1, after
                    // 0x55CED0's `0xFE` answer), so 0x48C870's walk of the players list never reaches it. Sending it the first
                    // joiner's `0xB0` ahead of its own `0xFE 1` made the second original wait 5 s in vain and drop («Соединение
                    // потеряно», its `'b'`).
                    let sent = if self.is_host() && !self.hello_wait.is_empty() {
                        let to: Vec<_> = (0..self.net.peers.len() as u32)
                            .filter(|s| !self.hello_wait.contains(s))
                            .filter_map(|s| self.net.peer_addr(s))
                            .collect();
                        self.net.send_game_to(&to, &m)
                    } else {
                        self.net.send_game(&m)
                    };
                    if let Err(e) = sent {
                        tracing::debug!(error = %e, "room message not sent");
                    }
                    // a joiner's `0xBD` comes back from the host's relay (0x48C0F0 does not skip the sender for it) and is
                    // heard then; the host's own is heard at once
                    if !(m.first() == Some(&0xBD) && !self.is_host()) {
                        self.hear(&m);
                    }
                }
                To::Player(id) if id == self.id as i32 => self.hear(&m),
                To::Player(id) => {
                    if let Some(a) = u32::try_from(id - 1).ok().and_then(|s| self.net.peer_addr(s))
                        && let Err(e) = self.net.send_game_to(&[a], &m)
                    {
                        tracing::debug!(error = %e, "room message not sent");
                    }
                }
            }
        }
    }

    /// One room message for our own player (0x48F0A0's cases).
    pub(super) fn hear(&mut self, m: &[u8]) {
        // 0x48F784 (`0xBD`): `[net+0xE6] ^= 1 << value` (a word: bits 8…15 are `+0xE7`)
        if m.first() == Some(&0xBD) {
            if let Ok(h) = nfs_net::msg::Message::parse(m) {
                self.barrier ^= 1u16.wrapping_shl(h.value as u32);
            }
            return;
        }
        // 0x48F527…0x48F574 (`0xC3`): the answer to a ping request
        if let Some(p) = room::Ping::parse(m) {
            self.ping_answer(p, self.now());
            return;
        }
        // 0x48F92A (`0xC4`): the host's clock, and ours when it came (0 without a connection) — the input
        // exchange's heartbeat (network.md §11.17)
        if m.first() == Some(&0xC4) {
            if let Ok(h) = nfs_net::msg::Message::parse(m)
                && let Ok(w) = h.body_u32()
            {
                let local = if self.established() { self.now() } else { 0 };
                self.exch.on_beat(w, local);
            }
            return;
        }
        match self.room.hear(m) {
            // the chat box's callback 0x4C2CE0 queues `(text, from, to)`; with no box open the
            // callback slot is clear and the line goes nowhere (0x48F3B1)
            Some(Heard::Chat(line)) if self.chat_open => {
                self.chat.push(line);
                // 0x4C2711: the box keeps a hundred lines, dropping the oldest
                let over = self.chat.len().saturating_sub(100);
                self.chat.drain(..over);
                self.dirty = true;
            }
            Some(Heard::Chat(_)) | None => {}
            // the host's own room object sends this and does not act on it (0x48BF10 sets the
            // started byte itself); a player takes it as the race starting
            Some(Heard::Start { clock }) => {
                // 0x48F837: `[net+0xE5] = [net+0xE4]` — everyone present is now in the race
                self.in_race = self.room.players.iter().fold(0u8, |m, p| m | 1u8.checked_shl(p.id as u32).unwrap_or(0));
                // 0x48F82F on every machine, the host's own included (its room sends to its own link
                // too): `+0xC5 = 0` (0x48F85A), `+0xC4 = 1` (0x48F861), `+0xBD = 0` (0x48F868), the
                // barrier's word `+0xE6 = 0` (0x48F896)
                self.aborted = false;
                self.go_wait = true;
                self.go_clock = None;
                self.sent_time = false;
                self.barrier = 0;
                // the exchange's counters, the sender's reset and the clock set to the room's word (0x48F82F…0x48F8C8)
                self.exch.on_start(clock, self.now());
                // (0x48F87F…0x48F88B also clears the input-exchange timers `[+0x108 + 4i]` /
                // `[+0x128 + 4i]`, which the port has no counterpart of; the players' `+0x48` /
                // `+0x50` are reset by the launch, 0x48D3D0 — [`NetPlay::launch_begin`])
                if !self.is_host() {
                    tracing::info!(target: "netplay", clock, "the host started the race (0xB3)");
                    self.start_due = true;
                }
            }
            // 0x48F579: `+0x50` / `+0x48` of the player; the finish time HPosition shows (0x42B37A)
            Some(Heard::RaceTime { id, value }) => {
                tracing::info!(target: "netplay", player = id, value, "0xB7: a player's finish time");
                self.dirty = true;
            }
            // 0x48F5B9…0x48F5CF: the player's bit leaves `net+0xE5`
            Some(Heard::Returned(id)) => {
                if (0..8).contains(&id) {
                    self.in_race &= !(1u8 << id);
                }
                tracing::info!(target: "netplay", player = id, "0xB7 (-2): a player is back in the front end");
                self.dirty = true;
            }
            // 0x48F8D2: the first `0xC5` after `0xB3` is the room's go; a later one puts the sender's
            // bit into `net+0xE5`
            Some(Heard::Go { id, clock }) => {
                if self.go_wait {
                    self.go_wait = false;
                    self.go_clock = Some(clock);
                    // 0x48F906: the word is the room's clock at the start — `net+0x15C`, what the exchange's
                    // tick clock counts from
                    self.exch.on_go(clock);
                    tracing::info!(target: "netplay", clock, "the room's go (0xC5)");
                } else if (0..8).contains(&id) {
                    self.in_race |= 1u8 << id;
                }
            }
            // 0x48F998: every record goes to 0x4B4B80 (the option table at 0x5D1E40); the plain
            // band (id < 0x10E) is `*ptr = value`. The port keeps those, and says what else came.
            Some(Heard::Options(recs)) => {
                for r in recs {
                    // the launch's per-slot records for a slot past the players are the host's
                    // traffic cars (0x4B4F25…0x4B4F75: slot `8 + k` rebased to `n + k`); before the
                    // launch (`net+0xE5 == 0`) a slot is a lobby player's
                    if self.in_race != 0 && self.launch.take_traffic(&r, self.room.players.len() as u32) {
                        tracing::info!(target: "netplay", record = ?r, "traffic car record (0xBC)");
                        continue;
                    }
                    // …and a slot below it is a player's **race index** (0x4B4F25…0x4B4F66): the
                    // car the owner races, its name, set-up and options (`network.md` §11.12)
                    if self.in_race != 0 && self.launch.take_car(&r, self.room.players.len() as u32) {
                        tracing::info!(target: "netplay", record = ?r, "player car record (0xBC, race index)");
                        continue;
                    }
                    if self.slots.apply(&r) {
                        tracing::info!(target: "netplay", record = ?r, "player slot record (0xBC)");
                        continue;
                    }
                    // the typed table the launch reads (every band, as 0x4B4B80 writes them)
                    let read = self.launch.take(&r);
                    match r.as_slice() {
                        [id, v] if *id < 0x10E => {
                            tracing::info!(target: "netplay", id = format_args!("{id:#x}"), value = *v, "option record (0xBC)");
                            self.options.insert(*id, *v);
                            self.options_in.push((*id, *v));
                        }
                        other => tracing::info!(target: "netplay", record = ?other, read, "option record (0xBC), string / array / per-slot band"),
                    }
                }
                self.dirty = true;
            }
            // 0x48F34F: the value is the comm state; 0xE is "in" (`+0xBE = 1`), anything else ends
            // the connection (`+0xBF = 1`, teardown 0x48E790)
            Some(Heard::State(v)) => {
                if !self.is_host() {
                    tracing::info!(target: "netplay", state = v, "the host set the comm state (0xC6)");
                    self.state_in = Some(v as u32);
                    self.dirty = true;
                }
            }
            // 0x48F46F / 0x48F4FD…0x48F51B: the player's bits leave `+0xE4` and `+0xE5`
            Some(Heard::Left(id)) => {
                self.pings.remove(&id);
                self.reported.remove(&id);
                if (0..8).contains(&id) {
                    self.in_race &= !(1u8 << id);
                    self.slots.clear(id as usize);
                }
                self.dirty = true;
            }
            Some(h) => {
                // 0x48F3F9…0x48F403: a new player record starts with no ping and no request out
                if let Heard::Joined(id) = h {
                    self.pings.insert(id, PlayerPing::default());
                }
                tracing::info!(target: "netplay", "room: {h:?}");
                self.dirty = true;
            }
        }
    }

    /// Whether a player id is on the room's list (the host's copy).
    pub(super) fn in_room(&self, id: i32) -> bool {
        self.room.players.iter().any(|p| p.id == id)
    }

    /// 0x48BDA0's tail (0x48BEB7…0x48BEC8): a player out of the room, `room+0xAC` − 1, then 0x490B70 — the room's description
    /// (0x48BAA0) to every player record left on the manager's list `mgr+0x164` with the new count. The `0xB1` before it has
    /// already taken the player off our own list, so the count is the room's after the leave. Only for a player that was in the
    /// room: 0x492770 calls 0x48BDA0 only when the record has a room (`+0xA4 != 0`); a peer still in `hello_wait` has no record.
    pub(super) fn describe_room_to_all(&mut self) {
        if !self.is_host() {
            return;
        }
        let count = self.room.players.len() as u32;
        let (rname, dbytes, dwords) = (self.net.name, self.net.desc_bytes, self.net.desc_words);
        self.dispatch(vec![(To::All, room::description(ROOM_ID, &rname, dbytes, dwords, count))]);
    }

    /// 0x48BB50 — the room admits a player (the old body of the `PeerJoined` arm).
    fn admit_peer(&mut self, slot: u32, _log: &mut Vec<String>) {
        // 0x48BB50: the room tells the newcomer who is there, and everyone of it
        let id = slot as i32 + 1;
        let name = self.net.peers.get(slot as usize).and_then(|p| p.as_ref()).map(|p| p.name).unwrap_or_default();
        let name = name.split(|&c| c == 0).next().unwrap_or_default().to_vec();
        // captured order (REFERENCE-2026-10-02-SRVX, the original host): the description 0xB4 (0x48BBC7), the host's option records,
        // 0xB2 (0x48BC1B), the 0xB0s, the second description (count + 1), `0xC6 (0xE)`
        let count = self.room.players.len() as u32;
        let (rname, dbytes, dwords) = (self.net.name, self.net.desc_bytes, self.net.desc_words);
        let to_new = |m: Vec<u8>| (To::Player(id), m);
        self.dispatch(vec![to_new(room::description(ROOM_ID, &rname, dbytes, dwords, count))]);
        // 0x48BBEE…0x48BBFC: for every player already in the room, 0x48A390 replays that player's own message list (`player+0xC8`) to the
        // newcomer, host first, before `0xB2` (0x48BC1B). The list is the log `Room::file_options` keeps (0x48A2F0: the `0xBC` messages a
        // player sent in the lobby group, one per key). The capture (two originals) shows the host's list: the create screen's
        // options `0x14` (COMMSPOSITIONAL), `0xD`, `0xF`, `0xC`, then the lobby's rows `0x226`, `0x141`, `0x225`, `0x255` (slot 0). Only the
        // newcomer hears them (To::Player), not every link.
        let recs: Vec<(To, Vec<u8>)> = self.room.replay_all().into_iter().map(to_new).collect();
        if !recs.is_empty() {
            self.dispatch(recs);
        }
        let end = rname.iter().position(|&c| c == 0 || c == b'~').unwrap_or(rname.len());
        let mut out = vec![to_new(room::your_id(id, &rname[..end]))];
        let mut adm = self.room.admit(id, &name);
        let c6 = if adm.last().map(|m| m.1.first()) == Some(Some(&0xC6)) { adm.pop() } else { None };
        out.extend(adm);
        // 0x48BCE3…0x48BCEE: `room+0xAC` + 1, then 0x490B70 sends the description (0x48BAA0) to every player record of the manager's
        // list `mgr+0x164` — every connected player, not the newcomer alone (live: the original host's runB / run3,
        // an earlier joiner gets the newcomer's `0xB0` and then `0xB4` with the new count)
        out.push((To::All, room::description(ROOM_ID, &rname, dbytes, dwords, count + 1)));
        out.extend(c6);
        self.dispatch(out);
        // 0x491AB4 / 0x491C25: the handler's last store, `+0xBA = 1`, after the `0xC6`
        self.net.peer_in(slot);
        // 0x48BB66: the newcomer's bit in the exchange room's `+0xCC0` and `+0xCC2`
        self.server.join(slot as usize + 1);
        // A newcomer hears **no** earlier record: 0x48BB50 / 0x4924D0 send the `0xB0`s and the
        // room description, no state (`network.md` §11.9). What makes every machine know
        // every car is the launch's per-slot resend with race indices (`send_launch`,
        // §11.12) — the port used to hand the host's table to a newcomer here.
    }

    /// Drain the socket. Returns the events worth logging; the car states are kept internally.
    pub fn poll(&mut self, tick: u64) -> Vec<String> {
        let mut log = Vec::new();
        // 0x48FB9F…0x48FBA5 (link event 1): `net+0x148 = GetTickCount()` while the connection is not
        // yet complete (`+0xBE == 0`), so the first request carries a large number
        if !self.established() {
            self.net148 = tick_ms();
        }
        // the connect command's attempts repeat (0x55C7D2: every `[0x5DEB48] * 5`); a probe lost on the
        // way is sent again once a second
        if let Some((host, who, last)) = self.join_wait.as_mut()
            && last.elapsed() >= Duration::from_secs(1)
        {
            let (h, w) = (*host, who.clone());
            let _ = self.net.join(h, &join_identity(&w));
            *last = Instant::now();
        }
        for e in self.net.poll() {
            if let Event::Data { slot: Some(s), bytes, .. } = &e
                && self.is_host()
                && self.hello_wait.remove(s)
            {
                if bytes.first() == Some(&0xFF) {
                    // 0x55CED0's wait: the link message `0xFE`, byte 1 = connected (§7.15)
                    self.dispatch(vec![(To::Player(*s as i32 + 1), vec![0xFE, 1])]);
                }
                self.admit_peer(*s, &mut log);
            }
            match e {
                Event::Joined { slot } => {
                    let join_who = self.join_wait.take().map(|(_, w, _)| w);
                    self.join = JoinState::Joined;
                    self.id = slot as u8 + 1;
                    self.room.me = self.id as i32;
                    log.push(format!("joined at slot {slot}, player id {}", self.id));
                    // the CONMAN's hello, 0x55CED0 via 0x560420 (captured, REFERENCE-2026-10-02-SRVE.md / SRVX): the first frame
                    // of the data layer is `0xFF, 0x00, "Player\0", <identity>\0`; the original host answers it with the
                    // link message `0xFE, 1` and an original joiner waits 5 s for that answer (0x55CED0)
                    if let Some(who) = join_who {
                        let mut hello = vec![0xFF, 0x00];
                        hello.extend_from_slice(b"Player\0");
                        hello.extend_from_slice(&join_identity(&who));
                        let _ = self.net.send_game(&hello);
                    }
                }
                Event::Refused { status } => {
                    self.join_wait = None;
                    self.join = JoinState::Refused(status);
                    log.push(format!("the host refused: {status:?}"))
                }
                Event::PeerJoined { slot, from } => {
                    log.push(format!("slot {slot} taken by {from}"));
                    // the original host talks to a joiner only after its link layer has answered the CONMAN hello
                    // (SRV-E capture: `'q'`, joiner `0xFF`, host `0xFE 1`, then `0xB4` / `0xBC` / `0xB2` / `0xB0`); a peer whose
                    // first data frame is not a hello (an older port) is admitted by that frame
                    self.hello_wait.insert(slot);
                }
                Event::PeerLeft { slot } => {
                    self.hello_wait.remove(&slot);
                    self.slots.clear(slot as usize + 1);
                    self.in_race &= !(1u8 << (slot + 1));
                    // 0x48BDA0: a player in the race is reported out (0x48C220), then its bit leaves `+0xCC0`
                    self.server.out_of_race(slot as usize + 1);
                    self.server.leave(slot as usize + 1);
                    log.push(format!("slot {slot} left"));
                    // 0x48BE05: the room tells everyone left that the player has gone
                    let was_in = self.in_room(slot as i32 + 1);
                    self.dispatch(vec![(To::All, room::player_out(slot as i32 + 1))]);
                    if was_in {
                        self.describe_room_to_all();
                    }
                }
                Event::Quit { from } => log.push(format!("{from} quit")),
                Event::Found { from, describe } => {
                    let n = String::from_utf8_lossy(&describe.name)
                        .trim_end_matches('\0')
                        .to_string();
                    log.push(format!("found \"{n}\" at {from}"));
                    // (a browse's answer: the join itself is the typed-address connect's `'j'`, sent at
                    // `start_named`, and repeated until the `'a'`)
                }
                // a player's chat line reaches the host as a request, and the room object
                // passes it on rewritten (0x4912D4…0x4913AC)
                // the CONMAN hello of a joiner: the host's link layer answers `0xFE, 1` ("connected", 0x55CED0's wait,
                // §7.15's link message) — an original joiner drops the connection after 5 s without it
                Event::Data { bytes, .. } if self.is_host() && bytes.first() == Some(&0xFF) => {}
                Event::Data { slot: Some(s), bytes, .. }
                    if self.is_host() && bytes.first() == Some(&kind::CHAT) =>
                {
                    if let Some(r) = ChatRequest::parse(&bytes) {
                        let out = self.room.route_chat(s as i32 + 1, r.to, &r.text);
                        self.dispatch(out);
                    }
                }
                // a player's `0xBC` record: the room's relay 0x48C050 sends it to the players in the
                // sender's lobby group (`+0xCC2`) and no others — the host's own copy included
                Event::Data { slot: Some(s), bytes, .. } if self.is_host() && bytes.first() == Some(&kind::OPTIONS) => {
                    self.room.file_options(s as i32 + 1, &bytes);
                    let mut out = self.room.relay_options(s as i32 + 1, &bytes);
                    // 0x48C0A4: the original's list includes the sender, which hears its own record back — an original joiner keeps its
                    // table (its own `.READY`, which PLAYGAME's 0x50CF0C reads) from that echo alone. Live: without it the original
                    // joiner pressed READY, the host saw it, but at PLAYGAME the joiner's own `.READY` read 0 and it aborted (0xB7 −2, −1)
                    out.push((To::Player(s as i32 + 1), bytes.to_vec()));
                    self.dispatch(out);
                }
                // the room's `0xB3` / `0xC5` / `0xB7` cases (0x49120C's jump table, 0x4917C0 / 0x49185B /
                // 0x4918BD): a player's request, and the room's answer to every link
                Event::Data { slot: Some(s), bytes, .. }
                    if self.is_host() && matches!(bytes.first(), Some(&kind::START) | Some(&kind::GO) | Some(&kind::RACE_TIME)) =>
                {
                    self.serve_race_request(s as i32 + 1, &bytes);
                }
                // a player's ping request reaches the host's room alone (0x491661)
                Event::Data { slot: Some(s), bytes, .. } if self.is_host() && bytes.first() == Some(&kind::PING) => {
                    if let Some(p) = room::Ping::parse(&bytes) {
                        self.ping_request(s as i32 + 1, p);
                    }
                }
                // **the input exchange** (network.md §11.17). The server dispatch 0x49120C hands a
                // message below `0xB0` to the player record's 0x48A2C0 → the room's 0x48C2B0: the
                // three bytes go into the block of the frame being filled
                Event::Data { slot: Some(s), bytes, .. } if self.is_host() && bytes.len() >= 3 && bytes[0] < 0xB0 => {
                    self.server.input(s as usize + 1, [bytes[0], bytes[1], bytes[2]]);
                }
                // and a frame from the host reaches a player (0x48F0DC)
                Event::Data { bytes, .. } if !self.is_host() && bytes.len() >= nfs_net::exchange::FRAME && bytes[0] < 0xB0 => {
                    self.exch_receive(&bytes);
                }
                Event::Data { bytes, .. } if bytes.first() != Some(&kind::CAR_STATE) => self.hear(&bytes),
                Event::Data { bytes, .. } => {
                    // **TEST GLUE** (not in the original): `NFS_NET_LAG_MS=<ms>` holds every car-state
                    // packet that long before the receiver hears it, to show the dead reckoning across
                    // a slow link (`network.md` 8.4)
                    match lag_glue() {
                        Some(ms) => self.lagged.push_back((Instant::now() + Duration::from_millis(ms), bytes)),
                        None => self.car_state_in(&bytes, tick, &mut log),
                    }
                }
            }
        }
        while self.lagged.front().is_some_and(|(due, _)| *due <= Instant::now()) {
            if let Some((_, bytes)) = self.lagged.pop_front() {
                self.car_state_in(&bytes, tick, &mut log);
            }
        }
        // 0x587CE0 can drop a peer whose countdown ran out, and that has to be reported too
        let mut due = self.last_poll.elapsed();
        let mut events = Vec::new();
        // at most a second's worth, so a stall does not drop everyone at once
        let mut budget = 64;
        while due >= POLL_PERIOD && budget > 0 {
            events.extend(self.net.tick());
            self.last_poll += POLL_PERIOD;
            due -= POLL_PERIOD;
            budget -= 1;
        }
        if budget == 0 {
            self.last_poll = Instant::now();
        }
        for e in events {
            match e {
                Event::PeerLeft { slot } => {
                    log.push(format!("slot {slot} timed out"));
                    self.server.out_of_race(slot as usize + 1);
                    self.server.leave(slot as usize + 1);
                    self.remotes
                        .retain(|_, r| Some(r.seat) != self.seats.get(slot as usize).copied());
                    self.dispatch(vec![(To::All, room::player_out(slot as i32 + 1))]);
                }
                other => log.push(format!("{other:?}")),
            }
        }
        // 0x48F0C4 / 0x48FBB6: the link to the host went away after we were in
        if !self.is_host() && self.join == JoinState::Joined && self.net.host_silent() {
            self.lost = true;
            self.dirty = true;
        }
        // the room's files go out a chunk at a time on their own clock (0x48EFB5…0x48F02E), on
        // the broadcast channel `[net+8]` — which is `send_game` here, relayed by the host
        let ms = self.started.elapsed().as_millis() as u32;
        struct Link<'a>(&'a mut nfs_net::session::Net);
        impl room::ChunkLink for Link<'_> {
            fn pump(&mut self) -> bool {
                // 0x48AE30 — the client's send queue (a host has none: its room sends on each link itself)
                self.0.pump_outbox()
            }
            fn send(&mut self, msg: &[u8]) {
                if let Err(e) = self.0.send_game(msg) {
                    tracing::debug!(error = %e, "file chunk not sent");
                }
            }
        }
        self.room.tick_with(ms, &mut Link(&mut self.net));
        // the exchange's room tick 0x48B6B0 (the original runs it from the 128 Hz timer)
        if self.is_host() {
            log.extend(self.server_tick());
        }
        log
    }

    /// **0x48F5E8** for one `0xBE` datagram: our own id comes back on a relayed packet and is dropped;
    /// the sender's car is found (or given the next reserved seat) and 0x4A0DF0 stores the block.
    fn car_state_in(&mut self, bytes: &[u8], tick: u64, log: &mut Vec<String>) {
        // 0x48F5E8: our own id comes back on a relayed packet, and it is dropped
        let Some((who, st)) = CarState::from_packet(bytes, self.id) else {
            return;
        };
        let seat = match self.remotes.get(&who) {
            Some(r) => r.seat,
            None => {
                // the seat is the player's index among the others (`remote_players`);
                // a player the room has not announced takes the next free one
                let idx = self.remote_players().iter().position(|p| p.0 == who as i32);
                let Some(s) = idx.and_then(|k| self.seats.get(k)).or_else(|| self.seats.get(self.next_seat)).copied() else {
                    return;
                };
                self.next_seat += 1;
                log.push(format!("player {who} is driving seat {s}"));
                s
            }
        };
        // the packet carries the *sender's* tick (+0x04), so this line can be matched
        // against the sender's own log without the two clocks having to agree
        tracing::debug!(target: "netplay", "rx player={who} sender_tick={} pos={:?} steer={} brake={}", st.tick, st.position, st.steer, st.brake);
        if !self.driven.contains(&seat) {
            self.driven.push(seat);
        }
        // 0x48F671 → 0x4A0DF0: the store, with `gGameTick` as the receiver's clock
        self.remotes.entry(who).or_insert_with(|| Remote::new(seat)).store(st, tick as i32);
    }

    /// The front end's poll: the same drain as the race's, with no seats to drive. Answers whether
    /// the room or the chat changed, i.e. whether the lists want binding again.
    pub fn poll_lobby(&mut self) -> bool {
        for line in self.poll(0) {
            tracing::info!(target: "netplay", "{line}");
        }
        std::mem::take(&mut self.dirty)
    }
}
