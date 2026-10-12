//! The input exchange of a non-positional race.

use super::*;

impl NetPlay {
    /// Whether the race in play runs by input exchange (`[0x657408] == 0`, entered through
    /// [`NetPlay::exchange_race_frame`]).
    pub fn exchanging(&self) -> bool {
        self.exchange && !self.positional
    }

    /// **0x48B6B0**, for as long as the room is started (see [`nfs_net::exchange::Server::tick`]):
    /// the `0xC4` heartbeat to everyone, each player's next frame (reliably in the original), and the
    /// `0xC6 (4)` that drops a player 128 frames behind.
    pub(super) fn server_tick(&mut self) -> Vec<String> {
        use nfs_net::exchange::Out;
        let mut log = Vec::new();
        if !self.server.started {
            return log;
        }
        let players: Vec<usize> = self.room.players.iter().filter_map(|p| usize::try_from(p.id).ok()).filter(|i| *i < 8).collect();
        let now = self.now();
        // 0x48B959…0x48B963: a frame the link does not take (no packet free, 0x595C58) stays and ends
        // the list loop; `spent` is what this call has already promised each link
        let mut spent = [0usize; 8];
        let addrs: Vec<Option<std::net::SocketAddr>> = (0..8).map(|k| if k == 0 { None } else { self.net.peer_addr(k as u32 - 1) }).collect();
        let net = &self.net;
        let mut can = |k: usize| {
            let ok = match addrs.get(k).copied().flatten() {
                Some(a) => net.reliable_room(a) > spent[k],
                None => true,
            };
            if ok {
                spent[k] += 1;
            }
            ok
        };
        let outs = self.server.tick(now, &players, &mut can);
        for o in outs {
            match o {
                Out::Beat { now } => self.dispatch(vec![(To::All, nfs_net::exchange::beat(now))]),
                Out::Frame { slot: 0, bytes } => self.exch_receive(&bytes),
                Out::Frame { slot, bytes } => {
                    if let Some(a) = self.net.peer_addr(slot as u32 - 1)
                        && let Err(e) = self.net.send_game_to(&[a], &bytes)
                    {
                        tracing::debug!(error = %e, "frame not sent");
                    }
                }
                Out::Drop { slot } => {
                    // 0x48B872…0x48B893: `0xC6 (4)` to it and its slot freed (0x4925A0)
                    log.push(format!("player {slot} is 128 frames behind: dropped (0xC6 4)"));
                    self.dispatch(vec![(To::Player(slot as i32), room::state(4))]);
                    self.net.remove_peer(slot as u32 - 1);
                    self.slots.clear(slot);
                    self.in_race &= !(1u8 << slot);
                    self.server.out_of_race(slot);
                    self.server.leave(slot);
                    self.dispatch(vec![(To::All, room::player_out(slot as i32))]);
                }
            }
        }
        log
    }

    /// **0x48F0DC…0x48F30F** — a frame from the room: [`nfs_net::exchange::Client::receive`] with the
    /// launch's numbering (`player+0x4C`) of the players on the list. A full ring is the original's
    /// error path (0x48F29B…0x48F2C0): `+0xC5`, `+0xBC` set, `+0xC4` cleared, and `0x48DA60(−2, −1)` —
    /// a `0xB7` to the room, which counts this machine out of the race.
    pub(super) fn exch_receive(&mut self, pkt: &[u8]) {
        if self.id == u8::MAX || !self.established() {
            return;
        }
        let idx = self.index_table();
        let room = &self.room;
        let car = |k: usize| idx.iter().find(|(id, _)| *id == k as i32 && room.player(*id).is_some()).map(|(_, i)| *i as usize);
        let now = self.now();
        let me = self.id as usize;
        let taken = self.exch.receive(pkt, me, car, now);
        if let nfs_net::exchange::Received::Taken { overflow: true } = taken {
            tracing::warn!(target: "netplay", "the input ring is full: this machine is 128 entries behind the frames (0x48F29B) — it leaves the race");
            // 0x48F29B…0x48F2C0: `+0xC5 = 1`, `+0xBC = 1`, `+0xC4 = 0` and 0x48DA60(−2, −1) — the abort arm
            self.abort_race();
            self.lost = true;
            self.dirty = true;
        }
    }

    /// **The race frame's ring** (0x4152E0, `[0x6573E8] == 3`): after its barriers (0x415376) the ring is
    /// cleared (0x415E00, 0x41537B) and the timer's counters restart, **before** the `0xC5` request
    /// (0x4153AE) whose answer starts the room's frames; with `Comms.Positional` off the race is an
    /// input-exchange one from here (`network.md` §11.17). Called by `App::net_race_go`.
    pub fn exchange_race_frame(&mut self) {
        if !self.established() || self.in_race == 0 || self.id == u8::MAX {
            return;
        }
        self.exch.enter_race();
        self.timer_anchor = None;
        self.timer_done = 0;
        self.commands = [0; 8];
        self.exchange = !self.positional;
        // The original's first tick, `t = 0`, latches entry 0 — the zero sample 0x415E00 preloaded —
        // and frees it (0x415EF0). The port's ticks count from 1 (its `simulate` increments first), so
        // that latch is made here: without it entry 0 would stay full and the 128th write would find it so.
        if self.exchange {
            let n = self.index_table().len();
            self.exch.ring.latch(0, n);
        }
    }

    /// **0x413800 → 0x48CA00 → 0x48EB20**: one 32 Hz sample of this machine's controller — the sender
    /// packs every second one into three bytes (16 Hz) and they go to the host's room, unreliably
    /// (0x55D4D0's `0x80000000`); the host's own goes into its room directly (its link is to itself).
    pub fn exchange_sample(&mut self, s: nfs_net::exchange::Sample) {
        // 0x48EB28…0x48EB4A: an id, `[0x657408] == 0`, not stopped
        if !self.exchanging() || self.exch.stopped || self.id == u8::MAX || !self.established() {
            return;
        }
        let me = self.id as usize;
        let now = self.now();
        if let Some(b) = self.exch.sender.sample(s, me, now) {
            if self.is_host() {
                self.server.input(0, b);
            } else if let Err(e) = self.net.send_game(&b) {
                tracing::debug!(error = %e, "input packet not sent");
            }
        }
    }

    /// One turn of what runs beside the game loop in the original: the events, the room's tick and the
    /// 128 Hz timer's callbacks ([`NetPlay::exchange_callbacks`]). Answers how many 32 Hz samples are due.
    pub fn exchange_pump(&mut self, paused: bool) -> u32 {
        let _ = self.poll_lobby();
        if !self.exchanging() {
            return 0;
        }
        self.exchange_callbacks(Instant::now(), paused)
    }

    /// **0x4A45D0** — the timer callback, emulated (**glue:** the original's is a thread every
    /// 7.8125 ms), with **0x48EE90**'s clock pull beside it: while `net+0xC4` is set nothing counts
    /// (0x4A462C); while the game is paused (`[0x5CC81C]`) the callback returns (0x4A45F1); otherwise
    /// [`nfs_net::exchange::Timer::callback`] runs with 0x48D840's milliseconds since the start.
    /// Answers the number of 32 Hz samples now due (0x4A46CC: `0x412E70` then `0x413800`).
    pub fn exchange_callbacks(&mut self, now: Instant, paused: bool) -> u32 {
        if self.exch.waiting {
            return 0;
        }
        let anchor = *self.timer_anchor.get_or_insert(now);
        let period = Duration::from_nanos(7_812_500);
        let due = now.saturating_duration_since(anchor).as_nanos() / period.as_nanos();
        let mut samples = 0;
        while u128::from(self.timer_done) < due {
            self.timer_done += 1;
            if paused {
                continue;
            }
            let local = tick_ms_at(anchor + period * (self.timer_done as u32));
            self.exch.update_clock(local);
            let ms = self.exch.since_start(local);
            samples += self.exch.timer.callback(ms);
        }
        samples
    }

    /// **0x415EF0(frame)** — at an even game tick `t` the frame driver latches ring entry `t / 2` for
    /// every player of the race (`[0x658084]`), and each car's command code `car+0xD8B` is the sample's
    /// `[0x5E9664] >> 3` (0x4992F6).
    pub fn exchange_latch(&mut self, frame: u32) {
        let n = self.index_table().len();
        self.exch.ring.latch(frame, n);
        for p in 0..8 {
            self.commands[p] = self.exch.ring.latched[p][0] >> 3;
        }
    }

    /// The sample of the race index `p` the last latch took (`0x608B14 + 4·p`).
    pub fn latched(&self, p: usize) -> nfs_net::exchange::Sample {
        self.exch.ring.latched.get(p).copied().unwrap_or([0; 4])
    }

    /// **The frame driver's gate** (0x415455…0x415495, 0x415BD4): a tick may run while the ticks done
    /// stay at or below `min(0x6559F8, 2·F0 + 1) − 1`; without a connection (0x4154A4) the target is
    /// the input-frame counter's, `2·[0x5CC818]`. Answers how many ticks in all may have run.
    pub fn exchange_ticks_allowed(&self) -> u64 {
        if self.established() && !self.exch.stopped {
            u64::from(self.exch.allowed_ticks())
        } else {
            2 * u64::from(self.exch.input_frames) + 1
        }
    }

    /// The race is over for the exchange: nothing more is sampled or gated. (What the room is told is
    /// 0x48DAD0's `0xB7`s, [`NetPlay::leave_race`], sent by the front end's return.)
    pub fn exchange_leave(&mut self) {
        self.exchange = false;
    }
}
