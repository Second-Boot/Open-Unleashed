//! The room as the front end sees it: players, chat, pictures, loading rows, slots, eject and leave.

use super::*;

impl NetPlay {
    /// `Comms.PlayerList` — **0x4BBBD0**, the accessor's refill: one row per player on
    /// `net+0x260` in the order the room announced them, the name as it came (`player+0`) and the
    /// id `player+0x40` kept with the row (`+0xC`, which 0x4BBB40 and 0x4BBB60 read), then rows of
    /// FE 287 "empty" with id −1 until there are eight (0x4BBD07…0x4BBDA9). *(Until 2026-09-25 this
    /// answered "HOST (YOU)" and "PLAYER n", upper-cased — not the original's.)*
    ///
    /// Not carried: the row draw 0x4BBEB0, which numbers the rows and marks a player whose ready
    /// bit (`net+0xE5`, 0x490390) is set — `READY` has no port.
    pub fn players(&self, empty: &str) -> Vec<(String, i32)> {
        let mut v: Vec<(String, i32)> = self
            .room
            .players
            .iter()
            .map(|p| (p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect(), p.id))
            .collect();
        while v.len() < room::MAX_PLAYERS {
            v.push((empty.to_string(), -1));
        }
        v
    }

    /// Step `Comms.PlayerList`'s selection. With none yet the step lands on row 0.
    pub fn step_player(&mut self, delta: i32) {
        let n = room::MAX_PLAYERS.max(self.room.players.len()) as i32;
        let cur = self.player_sel.map_or(0, |s| s as i32 + delta);
        self.player_sel = Some(cur.rem_euclid(n) as usize);
    }

    /// Open or close the chat box's callback — the box's slots 21 and 22 (0x4C2940, 0x4C2A80).
    /// Opening drains what the box held (0x4C2A10…0x4C2A6A), so a box always opens empty.
    pub fn chat_box(&mut self, open: bool) {
        if open && !self.chat_open {
            self.chat.clear();
        }
        self.chat_open = open;
    }

    /// The chat input box's Return — **0x4C3310**: the addressee is the selected player row's id
    /// when `Comms.PlayerList` has a selection (0x4BC630, 0x4BBB40), everyone otherwise, and the
    /// line goes out through **0x48D9B0**, on the link to the host. The host's own line is the
    /// room object's to pass on straight away. Nothing is sent before the session has given us an
    /// id (0x48D9B0's gate).
    pub fn say(&mut self, text: &str, empty: &str) {
        if self.id == u8::MAX {
            return;
        }
        let to = self
            .player_sel
            .and_then(|r| self.players(empty).get(r).map(|p| p.1))
            .unwrap_or(room::TO_ALL);
        // the box took only 0x20…0xFE (0x4CCD95…0x4CCD9D), one byte each
        let text: Vec<u8> = text.chars().map(|c| c as u32).filter(|&c| c < 0x100).map(|c| c as u8).collect();
        if self.is_host() {
            let out = self.room.route_chat(0, to, &text);
            self.dispatch(out);
        } else if let Err(e) = self.net.send_game(&ChatRequest { to, text }.encode()) {
            tracing::warn!(error = %e, "chat line not sent");
        }
    }

    /// The chat box's rows, formatted the way **0x4C24F0** formats each line: `"%s: %s"`
    /// (0x5D7E24) for a line to everyone, FE 580 `IDS_WHISPER` for one to us, FE 581
    /// `IDS_WHISPERTO` otherwise. The names come from `Comms.PlayerList` (the box's
    /// `data.accessor`, `[this+0x154]`): 0x4BBB60 finds the row with that id and answers **row 0**
    /// when there is none. "Us" is `net+0xDC`, or −2 with no session (0x4C2559…0x4C257B).
    ///
    /// The original formats a line once, when its box's tick 0x4C2DB0 drains it; this formats the
    /// kept lines each time the list is bound, so a name that changes in between would read the
    /// new way — which no shipped path does.
    pub fn chat_rows(&self, empty: &str, whisper: &str, whisper_to: &str) -> Vec<String> {
        let rows = self.players(empty);
        let name = |id: i32| rows.iter().find(|r| r.1 == id).or(rows.first()).map_or(String::new(), |r| r.0.clone());
        let me = if self.id == u8::MAX { -2 } else { self.id as i32 };
        self.chat
            .iter()
            .map(|l| {
                let text: String = l.text.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                if l.to == room::TO_ALL {
                    room::format("%s: %s", &[&name(l.from), &text])
                } else if l.to == me {
                    room::format(whisper, &[&name(l.from), &text])
                } else {
                    room::format(whisper_to, &[&name(l.from), &name(l.to), &text])
                }
            })
            .collect()
    }

    /// **0x427A60**, from the race load 0x4B67B0 (which zeroes the game clock at 0x4B67F3): read
    /// `pic16.fsh` from the HUD folder and post it as file type 0 (0x48CB50), so every other
    /// player's `HLeader` can show it.
    pub fn post_picture(&mut self, bytes: &[u8]) {
        self.room.post(room::PICTURE, bytes);
    }

    /// **`HLeader`'s** picture (draw 0x4286F0): the car in first place is `seat` (`None` for our
    /// own), its player is the one whose car it is (0x48D720 matches `player+0x4C` against
    /// `car+0x524`; `player+0x4C` is written by the launch's players loop 0x4B4EA7…0x4B4ED9, `mov [edx+0x4C], ebx`
    /// at 0x4B4EBC: the running number over the players whose bit is set in `net+0xE5`, in list order, −1 for
    /// the others — the race index of [`NetPlay::seat_race_index`] — so a seat's car is the remote id's whose
    /// race index it holds, [`Remote::seat`]; settled, AC 2026-09-30), and its picture is file type 0 once whole
    /// (0x4903F0, 0x428772). The upload takes the file's first image, the record `[file+0x14]`
    /// points at (0x42878E) — an FSH's first directory entry.
    pub fn leader_picture(&mut self, r: &mut Renderer, seat: Option<usize>) -> Option<TextureId> {
        let id = match seat {
            None => self.id as i32,
            Some(s) => *self.remotes.iter().find(|(_, x)| x.seat == s)?.0 as i32,
        };
        self.picture_of(r, id).map(|(t, _, _)| t)
    }

    /// The picture of player `id` once its file is whole, uploaded once: texture, width, height
    /// (0x4903F0 finds the file of type 0, 0x428772 / 0x4A51F5 test `+0xC >= +8`).
    pub fn picture_of(&mut self, r: &mut Renderer, id: i32) -> Option<(TextureId, u32, u32)> {
        if let Some(t) = self.pictures.get(&id) {
            return *t;
        }
        let f = self.room.player(id)?.files.iter().find(|f| f.kind == room::PICTURE && f.complete())?;
        let tex = nfs_formats::fsh::Fsh::parse(&f.bytes)
            .ok()
            .and_then(|fsh| fsh.decode(&f.bytes, 0).ok())
            .map(|img| (r.upload_texture(&TextureData::single(img.width, img.height, img.rgba)), img.width as u32, img.height as u32));
        self.pictures.insert(id, tex);
        tex
    }

    /// **0x4A4A70(step)'s send, 0x4A4AD4…0x4A4B4F**: with the connection up (`net+8`, `+0xBF == 0`,
    /// `+0xBE`, `+0xDC ≥ 0`), the screen up (`[0x655A28]`, the caller's) and `step > 0`, the record
    /// `(0x256, [0x65741C], step)` goes out through 0x48D550 — every player's bar. The sender hears
    /// it back from the host's relay in the original; here — **glue**, as the other 0x48D550
    /// senders — it is applied to our own table at once.
    pub fn send_loading_step(&mut self, step: i32) {
        if step <= 0 || !self.established() {
            return;
        }
        let rec = vec![nfs_net::options::id::LOADING, self.local_race_index().unwrap_or(0), step as u32];
        for m in nfs_net::options::messages_each(std::slice::from_ref(&rec)) {
            if self.is_host() {
                self.room.file_options(self.id as i32, &m);
            }
            if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "loading step not sent");
            }
        }
        self.launch.take(&rec);
        tracing::info!(target: "netplay", record = ?rec, "loading step sent (0xBC 0x256)");
    }

    /// **0x4A4A70's per-player block, 0x4A5149…0x4A5353**, while the screen is up and the
    /// connection is: for each of the race's `NUMPLAYERRACECARS` slots — the player whose `+0x4C` is
    /// the slot (0x48D720) that is racing (0x490390: its bit of `net+0xE5`) with a whole picture
    /// gets its picture at `(74·grid + 32, 364)` and the two bars `(x, 436, 6)` by the progress its
    /// `0x256` reported; a slot without a racing player a 54 × 64 black box (0x4A52FC); a player
    /// whose picture has not come whole nothing (0x4A5208 `je 0x4A5327`). `grid` is the slot's
    /// `.STARTINGPOS` (`[0x658200 + slot·0x580]`).
    pub fn loading_rows(&mut self, r: &mut Renderer) -> Vec<LoadingRow> {
        if !self.established() {
            return Vec::new();
        }
        let table = self.index_table();
        let mut out = Vec::new();
        for slot in 0..table.len() as u32 {
            let x = 74.0 * self.launch.grid.get(&slot).copied().unwrap_or(0) as f32 + 32.0;
            let who = table.iter().find(|(_, i)| *i == slot).map(|(id, _)| *id).filter(|id| self.room.player(*id).is_some());
            let racing = who.is_some_and(|id| (0..8).contains(&id) && self.in_race & (1u8 << id) != 0);
            match who.filter(|_| racing) {
                None => out.push(LoadingRow::Absent { x }),
                Some(id) => {
                    if let Some((picture, w, h)) = self.picture_of(r, id) {
                        out.push(LoadingRow::Player { x, picture, w: w as f32, h: h as f32, progress: self.launch.progress.get(&slot).copied().unwrap_or(0) });
                    }
                }
            }
        }
        out
    }

    /// `Comms.Data(GAMENAME)` — the session's own name.    /// `Comms.Data(GAMENAME)` — the session's own name.
    pub fn game_name(&self) -> String {
        nfs_net::comm::desc::split_name(&self.net.name).0.trim().to_string()
    }

    /// `Comms.Data(MYIPADDRESS)` — the machine's address as the room shows it (0x50BF09), which
    /// is [`nfs_net::socket::my_ip_string`].
    pub fn my_address(&self) -> String {
        nfs_net::socket::my_ip_string()
    }

    /// **`[net+0x24]`** as 0x509550 reads it (0x48CA40), `nfs_net::room::comm_state`; the writers are
    /// `network.md` §11.10's table. A host is `HOSTING` (0x48DF69). A joiner is `CONNECTING` (0x48E4B4,
    /// 0x48E68E) until the host's `0xC6 (0xE)` arrives; `0xC6` with another value is that state (9
    /// ejected by the host 0x491746, 10 the room's version / game gate 0x490AB0 / 0x49157A, 11 full
    /// 0x49159E, 12 the host closing the room 0x48C035).
    ///
    /// The comm layer's own events give **4** (0x48F0C4, 0x48FBB6): a connect that runs out
    /// ([`JOIN_TIMEOUT`], conman event 5) and a host that goes silent after we were in (events 9 / 10).
    /// **PORT-DEVIATION D35** of the two joins' refusals, which the original never had in this form (its
    /// version / game / full checks are the room's `0xC6` after a link that is up): a refused `'a'`
    /// maps to 11 (`Full`, −2), 10 (`Refused`, −3: the identity gate of `nfs_net::session`) and — with
    /// no original writer on a LAN, state 8 is the lobby service's `0xC6` value — 8 (`NameTaken`, −1,
    /// the comm layer's own name test 0x588E7E, whose game-level consequence was not traced). States 6
    /// and 7 have no writer in the game's code at all (no `0xC6` the room server sends carries them), so
    /// nothing here produces them.
    pub fn comm_state(&self) -> u32 {
        use nfs_net::comm::JoinStatus;
        use nfs_net::room::comm_state as cs;
        if self.is_host() {
            return cs::HOSTING;
        }
        if self.lost {
            return cs::LOST;
        }
        match self.join {
            JoinState::Hosting => cs::HOSTING,
            JoinState::Pending(t) if t.elapsed() > JOIN_TIMEOUT => cs::LOST,
            JoinState::Pending(_) => cs::CONNECTING,
            JoinState::Joined => self.state_in.unwrap_or(cs::CONNECTING),
            JoinState::Refused(JoinStatus::NameTaken) => cs::NAME_TAKEN,
            JoinState::Refused(JoinStatus::Full) => cs::FULL,
            JoinState::Refused(_) => cs::WRONG_VERSION,
        }
    }

    /// `net+0xBE` with an id (0x4EDCD0 ≥ 0): the connection is established — every test the lobby
    /// screens' ticks make first.
    pub fn established(&self) -> bool {
        self.is_host() || (self.join == JoinState::Joined && self.state_in == Some(nfs_net::room::comm_state::CONNECTED) && !self.lost)
    }

    /// The other players in the room's order — their race index among the others is their seat
    /// (0x4B4DA0 numbers `player+0x4C` in list order over the players `net+0xE5` marks, and the
    /// opponents are all but our own): `(id, name, slot record)`.
    pub fn remote_players(&self) -> Vec<(i32, String, nfs_net::slots::PlayerSlot)> {
        self.room
            .players
            .iter()
            .filter(|p| p.id != self.id as i32)
            .map(|p| {
                let name = p.name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                let slot = usize::try_from(p.id).ok().and_then(|i| self.slots.slot.get(i)).cloned().unwrap_or_default();
                (p.id, name, slot)
            })
            .collect()
    }

    /// Our own slot in the table: the player id (0x4EDCD0).
    pub fn my_slot(&self) -> Option<usize> {
        (self.id != u8::MAX).then_some(self.id as usize).filter(|i| *i < nfs_net::slots::SLOTS)
    }

    /// **0x48D550 with our own slot records**: sent to the room, and — **glue** — applied to our own
    /// table at once, where the original hears them back from the host's relay (0x48C050 sends to
    /// every player with the sender's `+0xCC2` bit, the sender included; this port's relay leaves
    /// the sender out).
    pub fn send_slot_records(&mut self, recs: &[Vec<u32>]) {
        for m in nfs_net::options::messages_each(recs) {
            if self.is_host() {
                // the host's own record goes to its room like anyone's, and the room's relay
                // (0x48C050) sends it to the players in the host's lobby group only
                self.room.file_options(self.id as i32, &m);
                let out = self.room.relay_options(self.id as i32, &m);
                self.dispatch(out);
            } else if let Err(e) = self.net.send_game(&m) {
                tracing::debug!(error = %e, "slot record not sent");
            }
        }
        for r in recs {
            self.slots.apply(r);
            tracing::info!(target: "netplay", record = ?r, "player slot record sent (0xBC)");
        }
        self.dirty = true;
    }

    /// **0x5091B0** over the table: every player in the room has `.READY` set and nobody is in the
    /// race yet (`net+0xE5 == 0`) — the create screen's PLAYGAME is live then.
    pub fn everyone_ready(&self) -> bool {
        self.established()
            && self.in_race == 0
            && self.room.players.iter().all(|p| usize::try_from(p.id).ok().and_then(|i| self.slots.slot.get(i)).is_some_and(|s| s.ready))
    }

    /// `EJECTPLAYER` — the host's request `0xB1 (id)` reaches its own room, whose server dispatch
    /// (0x4916E1…0x491765) refuses the host itself (`+0xB9`, 0x49171D) and otherwise sends the
    /// target `0xC6 (9)` "Ejected by host" (0x491746…0x49174D), frees it (0x4925A0) and tells
    /// everyone `0xB1`. Answers whether a player was ejected.
    pub fn eject(&mut self, player: i32) -> bool {
        if !self.is_host() || player <= 0 {
            return false;
        }
        let slot = (player - 1) as u32;
        let Some(a) = self.net.peer_addr(slot) else { return false };
        let _ = self.net.send_game_to(&[a], &room::state(nfs_net::room::comm_state::EJECTED));
        self.net.remove_peer(slot);
        self.slots.clear(player as usize);
        self.in_race &= !(1u8 << player);
        let was_in = self.in_room(player);
        self.dispatch(vec![(To::All, room::player_out(player))]);
        if was_in {
            self.describe_room_to_all();
        }
        true
    }

    /// `'K'` — tell the others we are going (0x589152 clears our bit on the host). The comms
    /// screens' back sends it (`App::fe_session_leave`, 0x48E6F0 / 0x48E9C0).
    pub fn leave(&mut self) {
        // 0x48E9C0 (0x48EA0D…0x48EA38): a host's teardown asks its room to eject its own player
        // (`0xB1` with its own id), and the room's answer to that (0x491717…0x491723 → 0x48C000) frees
        // every other player and sends each `0xC6 (0xC)` — their state 12, the notice 830 and back
        if self.is_host() {
            for a in self.net.everyone() {
                let _ = self.net.send_game_to(&[a], &room::state(nfs_net::room::comm_state::LOST_2));
            }
        }
        let _ = self.net.leave();
    }
}
