//! The ping exchange, the race indices and the link classification.

use super::*;

impl NetPlay {
    /// **0x4905A0** for the room's player `id` (`ecx` = its record): the ping to draw, `player+0x54`
    /// (−1 = none yet). While the connection is up (`net+8`, `+0xBF == 0`, `+0xBE != 0`) and no
    /// request is out (`+0x58 == 0`), a request goes out when more than a second has passed since
    /// the last (`1000 < GetTickCount() − +0x44`): `+0x58 = 1`, `+0x44 = now`, then 0x48D9F0 sends
    /// `0xC3(id, net+0x148)` to the host. The lobby's row draws (0x4BC04D, 0x4BADF9) call it every
    /// frame for each row that has a player.
    pub fn ping(&mut self, id: i32) -> i32 {
        self.ping_at(id, self.now())
    }

    pub(super) fn now(&self) -> u32 {
        self.tick_override.unwrap_or_else(tick_ms)
    }

    pub(super) fn ping_at(&mut self, id: i32, now: u32) -> i32 {
        let up = self.established();
        let Some(p) = self.pings.get_mut(&id) else { return -1 };
        if up && !p.pending && (now.wrapping_sub(p.asked) as i32) > 1000 {
            p.pending = true;
            p.asked = now;
            let req = room::Ping { id, word: self.net148 as i32 };
            if self.is_host() {
                // the host's own game is a player of its own room (0x48BB50): the request is
                // answered in process
                self.ping_request(0, req);
            } else if let Err(e) = self.net.send_game(&req.encode()) {
                tracing::debug!(error = %e, "ping request not sent");
            }
        }
        self.pings.get(&id).map_or(-1, |p| p.ping)
    }

    /// **The host's case 195, 0x491661…0x4916CE**: the sender's word is kept (`peer+0xB0`); then
    /// for the room's player with the asked id the sender is answered `0xC3(id, that player's
    /// peer+0xB0)` (0x4916B7…0x4916BC).
    pub(super) fn ping_request(&mut self, from: i32, p: room::Ping) {
        self.reported.insert(from, p.word);
        if self.room.player(p.id).is_some() {
            let word = self.reported.get(&p.id).copied().unwrap_or(-1);
            self.dispatch(vec![(To::Player(from), room::Ping { id: p.id, word }.encode())]);
        }
    }

    /// **0x48F527…0x48F574 → 0x490600**: the player with the asked id (`+0x40`) takes the answer:
    /// `net+0x148 = now − player+0x44` (0x48DA30 → `[net+0x148]`, needing the connection up),
    /// `player+0x54 = word`, `player+0x58 = 0`.
    pub(super) fn ping_answer(&mut self, p: room::Ping, now: u32) {
        let up = self.established();
        if let Some(e) = self.pings.get_mut(&p.id) {
            if up {
                self.net148 = now.wrapping_sub(e.asked);
            }
            let changed = e.ping != p.word;
            e.ping = p.word;
            e.pending = false;
            self.dirty = true;
            if changed {
                tracing::info!(target: "netplay", player = p.id, ping = p.word, own_round_trip = self.net148, "ping answer (0xC3)");
            }
        }
    }

    /// The lobby row draw's calls of 0x4905A0, one per room player that has a row: what a frame of
    /// `Comms.PlayerList` (0x4BBEB0) does. The rows are drawn only with the connection up.
    pub fn ping_step(&mut self) {
        let ids: Vec<i32> = self.room.players.iter().map(|p| p.id).collect();
        for id in ids {
            self.ping(id);
        }
    }

    /// The ping the lobby draws for player `id`: `min(player+0x54, 9999)` when it is above −1
    /// (0x4BC04D…0x4BC06A, signed compares), `None` otherwise.
    pub fn ping_shown(&self, id: i32) -> Option<u32> {
        let v = self.pings.get(&id)?.ping;
        (v > -1).then_some(v.min(9999) as u32)
    }

    /// **0x4B4DA0's numbering** (`player+0x4C`, and our own index into `[0x65741C]`): kept for the race,
    /// where 0x490140 and the seats read it. Players who are not in the race have none (−1).
    pub fn note_race_indices(&mut self, index: &[(i32, u32)]) {
        self.launch_index = index.to_vec();
    }

    /// The numbering the race reads. **Glue:** a `--net` CLI run (test scaffolding) never runs the
    /// launch 0x4B4DA0, so with none noted the room's list order stands in for it.
    pub(super) fn index_table(&self) -> Vec<(i32, u32)> {
        if self.launch_index.is_empty() { self.race_indices() } else { self.launch_index.clone() }
    }

    /// `[0x65741C]` (`LOCAL_CAR`): our own race index.
    pub fn local_race_index(&self) -> Option<u32> {
        self.index_table().iter().find(|(id, _)| *id == self.id as i32).map(|(_, i)| *i)
    }

    /// The race index of the `k`-th **other** player in room order — the opponent seat `k` of the
    /// port (`remote_players`, 0x4B4DA0 numbering the list in order).
    pub fn seat_race_index(&self, k: usize) -> Option<u32> {
        self.index_table().iter().filter(|(id, _)| *id != self.id as i32).nth(k).map(|(_, i)| *i)
    }

    /// `net+0xBD`: a `0xB7` has gone out since the race started (this machine's car has finished or left).
    pub fn finish_sent(&self) -> bool {
        self.sent_time
    }

    /// `net+8 != 0 && +0xBF == 0`: the session exists and its link has not dropped (0x48F0C4 sets
    /// `+0xBF` for the events 9 / 10 and for any `0xC6` other than 0xE, 0x48F34F).
    pub fn link_up(&self) -> bool {
        self.is_host() || (self.join == JoinState::Joined && !self.lost && self.state_in.is_none_or(|s| s == nfs_net::room::comm_state::CONNECTED))
    }

    /// **0x490140(race index)**, read to the instruction (`network.md` §11.13):
    ///
    /// * 3 when the session is gone (`net+8`, `+0xBF`), the list `net+0x260` is empty or holds no
    ///   player whose `+0x4C` is `race_index` (0x49016B…0x490179, 0x490187);
    /// * 4 when 0x490390 says the player is not racing: needs the connection up (`+0xBE`, `+0xDC ≥ 0`)
    ///   and its bit in `net+0xE5` (0x490199);
    /// * with `COMMSPOSITIONAL` (`[0x657408]`) set (0x4901A0): the local car's ping through 0x4905A0
    ///   × 0.001 (`[0x5B49A8]`), any other car's packet age `(game tick − car+0x3C) ×` 1/64
    ///   (`[0x5B2828]`), against `[0x5B2750]` = 1.0 (→ 2) and `[0x5B49A0]` = 0.3 (→ 1, else 0);
    /// * without it (0x490243): `net+0x128 + 4·index` × 0.001, against `[0x5B2778]` = 0.5 (→ 2) and
    ///   `[0x5B4998]` = 0.2 (→ 1, else 0) — 0 when the connection is not fully up.
    ///
    /// `game_tick` is `[0x606AC4]`. Like 0x4905A0 (which it calls for the local car) it may send a
    /// ping request.
    pub fn link_quality(&mut self, race_index: u32, game_tick: u32) -> LinkQuality {
        if !self.link_up() {
            return LinkQuality::NotListed;
        }
        // the player of `net+0x260` whose `+0x4C` is the index
        let Some(id) = self
            .index_table()
            .iter()
            .find(|(id, i)| *i == race_index && self.room.player(*id).is_some())
            .map(|(id, _)| *id)
        else {
            return LinkQuality::NotListed;
        };
        // 0x490390: connection up (`+0xBE`, `+0xDC >= 0`) and the player's bit of `net+0xE5`
        // (glue: with no launch ever run there is no `net+0xE5` to test)
        let racing = self.launch_index.is_empty() && self.in_race == 0 || (0..8).contains(&id) && self.in_race & (1u8 << id) != 0;
        if !(self.established() && racing) {
            return LinkQuality::NotRacing;
        }
        let (bad, fair) = if self.positional {
            let secs = if Some(race_index) == self.local_race_index() {
                f64::from(self.ping(id)) * 0.001
            } else {
                let last = self.remotes.get(&(id as u8)).map_or(0, |r| r.state.tick);
                f64::from(game_tick.wrapping_sub(last) as i32) * 0.015625
            };
            (secs > 1.0, secs > 0.3)
        } else {
            let ms = if self.established() { self.exch.sender.latency.get(race_index as usize).copied().unwrap_or(0) as i32 } else { 0 };
            let secs = f64::from(ms) * 0.001;
            (secs > 0.5, secs > 0.2)
        };
        match (bad, fair) {
            (true, _) => LinkQuality::Poor,
            (false, true) => LinkQuality::Fair,
            _ => LinkQuality::Good,
        }
    }

    /// **0x416310's network test** (FEGAME_TYPE 3, a racer still in state 0; 0x41637D…0x4163F5): with
    /// `COMMSPOSITIONAL` set the racer goes to state 3 when 0x490140 answers above 2; without it when
    /// the connection is not fully up (`net+8`, `+0xBF`, `+0xBE`, `+0xDC ≥ 0`, 0x4163B9…0x4163E1) —
    /// or `car+0xD8B == 1`, a flag the input-exchange mode would set and this port never does.
    pub fn retired(&mut self, race_index: u32, game_tick: u32) -> bool {
        if self.positional {
            self.link_quality(race_index, game_tick).gone()
        } else {
            // 0x4163B0: `car+0xD8B == 1` — the command code of the racer's last sample, which the
            // room writes (`ABSENT`) for a player that is not in the race — or no full connection
            !self.established() || self.commands.get(race_index as usize) == Some(&nfs_net::exchange::CMD_LEFT)
        }
    }

    // ------------------------------------------------------------------------------------------
    // the input exchange (`Comms.Positional` off) — network.md §11.17
    // ------------------------------------------------------------------------------------------
}
