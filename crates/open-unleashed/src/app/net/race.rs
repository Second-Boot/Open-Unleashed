//! A network race on the application side: `HInfo`'s lag arm, the pause, the lock-step input exchange and the replay of a network race.

use crate::*;
use nfs_physics::vehicle::RaceState;

impl App {
    /// **`HInfo`'s tick 0x427C70** for a network race (`[0x6573E8] == 3`; the HUD tick 0x425950 calls it at
    /// 0x425E0B, once per **rendered frame** — the port calls it from each frame of the windowed loop and from
    /// each turn of the scripted one and its waits, since it is exactly while ticks stall that it matters).
    /// Two things, in this order:
    ///
    /// 1. **the input exchange's lag** (`[0x657408] == 0`, 0x427C7E…0x427EDF): `lag = 2·net+0xF0 −
    ///    [0x6559F8] + 1` (0 for `net+0xF0` without a full connection) — how far the frames received are
    ///    behind the tick clock, in ticks. Under **−0x100** (4 s) with the link up: **18182 «Связь
    ///    утеряна.»** and the session is torn down (0x48E9C0). Under **−0x20** (0.5 s) and no box up:
    ///    **18193 «Связь...»**, `[0x60CA08] = 1`, and the tick ends. With the box up, above **−0x10** it
    ///    closes (vtable `+0x14`, 2) and the flag clears. While the flag is set nothing below runs (0x427DAA);
    /// 2. **the comm state** `[net+0x24]` (0x48CA40): 4 and 12 → 18182, 6 → 18183, 7 → 18186, 8 → 18188,
    ///    9 → 18185, 10 → 18189, 11 → 18187, 5 nothing (jump table 0x428274, states 4…12); the session is
    ///    torn down for 4, 6, 7, 8, 12 always and for 9, 10, 11 only while `net+0xC6` is clear (0x427FDD…0x428267;
    ///    the host's `+0xC6` is set, 0x48DF5A).
    ///
    /// **The box and its keys (settled, AC 2026-09-30):** the box is `HInfo` `[0x60C9C0]` on the HUD queue
    /// `[0x60C9C4]`; the port draws it (`Hud::notice_sprites`) and counts it in [`Self::hud_dialog_up`]. The
    /// class's shared slots (vtable 0x5B2BFC: 0x42A3B0 / 0x42A3C0 / 0x42A3D0 / 0x42A400 / 0x42A410 / 0x42A430)
    /// answer one key: slot 9, 0x42A3D0, returns 0x19 on Space only while the Spacebar byte `+8` is set, which
    /// 0x4251D5 builds as 0 for this box (18190's, 0x4250BF, as 1 — `net_notice_space`); nothing else closes
    /// it, so it stays up while the race runs, and the pause action 0x415110 with the queue non-empty goes to
    /// 0x41523A, the leave ([`Self::net_pause_press`]); game type 3 never opens the pause menu (0x426800). The
    /// teardown is 0x48E9C0 at once (0x427DA5 for the lag arm, 0x427FDD…0x428267 for the states): the
    /// port's [`Self::fe_session_leave`] — `exchange_leave`, the host's ejection and the socket.
    /// **The teardown's wait (ported, AK 2026-09-30):** 0x48E9C0 with `+0xC2` set (written 1 only by the host's create,
    /// 0x48E143) and a kind other than the modem's sends its leave and waits `[0x5DEB48] / 2` (0.5 s) in 0x53C380, a
    /// loop on the timer callbacks 0x5366E0, before it frees the links: [`Self::fe_session_leave`] keeps the host's
    /// session alive and drains it for that time ([`Self::net_leaving_step`]) instead of freezing the window.
    /// **The box is not pushed every frame (AC's claim corrected, AK):** each case of the state switch pushes the
    /// box once per call (0x427F44, 0x427F91, 0x428048, 0x4280EF …), but the tail of the 9 / 10 / 11 cases (0x427FCF…
    /// 0x427FEE) **consumes the state**: a session with `+0xC6` set (the host's, 0x48DF5A) gets `[net+0x24] = 14`
    /// (0xE), so the next frame's switch sees nothing and the box was pushed once per event; every other session is
    /// torn down at once (0x428267). The port's host has no such state to consume — [`netplay::NetPlay::comm_state`]
    /// answers `HOSTING` for it, and 9 / 10 / 11 are the room's `0xC6` to a *joiner* (the eject 0x491746, the version
    /// gate, a full room), never to the host's own player — so the arm is not reachable here.
    pub(crate) fn net_race_notice(&mut self) {
        let Some(n) = self.net.session.as_ref() else { return };
        if self.vehicle.is_none() || n.in_race == 0 {
            return;
        }
        let state = n.comm_state();
        let host = n.is_host();
        let mut teardown = false;
        // 0x427C7E: only an input-exchange race has the lag arm
        let mut to_states = true;
        if !n.positional {
            let connected = n.established();
            let f0 = if connected { n.exch.received as i32 } else { 0 };
            let lag = 2 * f0 - n.exch.timer.wall as i32 + 1;
            match lag_step(lag, connected, self.net_lag_box) {
                LagStep::Lost => {
                    self.net_notice = Some(18182);
                    self.net_notice_space = false;
                    teardown = true;
                    // (0x427DAA, after the teardown 0x427DA5: with the lag box up nothing else runs; the
                    // state read at the top, 0x427C77, is what the switch below still sees)
                    to_states = !self.net_lag_box;
                }
                LagStep::Show => {
                    tracing::info!(target: "netplay", lag, "HInfo (0x427C70): the frames are late — «Связь...» (18193)");
                    self.net_notice = Some(18193);
                    self.net_notice_space = false;
                    self.net_lag_box = true;
                    return;
                }
                LagStep::Hold => to_states = false,
                LagStep::Close => {
                    tracing::info!(target: "netplay", lag, "HInfo (0x427C70): the frames are back — the notice closes");
                    self.net_notice = None;
                    self.net_notice_space = false;
                    self.net_lag_box = false;
                }
                LagStep::States => {}
            }
        }
        if to_states && !self.net_lag_box {
            let text = match state {
                4 | 12 => Some(18182),
                6 => Some(18183),
                7 => Some(18186),
                8 => Some(18188),
                9 => Some(18185),
                10 => Some(18189),
                11 => Some(18187),
                _ => None,
            };
            if let Some(t) = text {
                self.net_notice = Some(t);
                    self.net_notice_space = false;
                // 0x427FDD…0x428267: 9, 10, 11 keep the session while `net+0xC6` is set (a host's)
                teardown |= !(matches!(state, 9..=11) && host);
            }
        }
        if teardown {
            tracing::info!(target: "netplay", state, notice = ?self.net_notice, "HInfo (0x427C70): the network is gone — the session is torn down (0x48E9C0)");
            self.fe_session_leave();
        }
    }

    /// **0x415110, the pause action (Esc / P / Start), in a network race** (`[0x6573E8] == 3`; read to
    /// the instruction, `hud.md` "Keys"). `live` is the link test `[0x628C70]`, `+8`, `+0xBF == 0`,
    /// `+0xBE != 0`, `+0xDC >= 0` ([`netplay::NetPlay::established`]); `queue` is the HUD dialog queue
    /// `[0x60C9C4]` being non-empty (0x426A60): the pause menu, a page, an `HInfo` notice.
    ///
    /// * **with a live link** (0x415110's second arm) and the queue empty, 0x426800's game-type-3 arm
    ///   (0x42695A…0x4269FF) queues `HInfo` `[0x60C9FC]` with hud.loc **18190**, opens it and clears its
    ///   timeout (0x41B130(0)): nothing pauses, the race keeps running, Space closes the notice;
    /// * with a notice (or any page) up, the press falls to 0x41523A: 0x4AB270, 0x4120F0, `[0x606A90] = 0`,
    ///   and — `[0x5E99F4]` below 2 — `[0x657424] = 3`, **0x416AC0(1) = leave the race**;
    /// * **with no live link** the first arm (0x41513E…) pauses (`[0x606A90] = 1`, 0x4AB150 …), runs
    ///   0x461740 and 0x426800 — the same 18190 notice, still no pause menu — and then, in type 3,
    ///   falls into the same leave: the press queues the notice and leaves in one call.
    ///
    /// Answers whether the press leaves the race. The leave is the front end coming back **not aborted** (`[0x657E34]` stays clear once the tick is
    /// past 0x20, 0x415120's other arm), i.e. to the lobby screen (`after_race_screens`' network arm).
    /// 0x416AC0(1)'s finalize is **not run, and that is not a difference** (settled, AC 2026-09-30): its outputs
    /// (`car+0xCD0…`, the record blocks `0x5E75A0[]` copied by 0x4B69D8) have three readers — 0x4EDD40 (game
    /// type 4 only), 0x4EDF00 (its 0x4FB100 / 0x4FAFC0 filing is skipped for type 3 and 9 at 0x4EDF34…0x4EDF3B;
    /// the rest of it is type 2 / 4 / 9 / 10) and the loop bound 0x4068C4 — and the type-3 race end 0x4EDD00
    /// files nothing (`network.md` §11.21 item 8).
    pub(crate) fn net_pause_press(&mut self) -> bool {
        let live = self.net.session.as_ref().is_some_and(|n| n.established());
        let page_up = self.results.is_some() && self.result_pages().get(self.hud_state.pages.page as usize).is_some();
        // the first arm: `[0x5E99F4] < 2`, `[0x606A88] == 0`, `[0x606AC4] > 0x20` and no live link
        let first_arm = !live && self.tick > 0x20;
        let queue = self.paused || self.net_notice.is_some() || page_up;
        if !queue {
            // 0x426800, type 3
            self.net_notice = Some(18190);
            self.net_notice_space = true;
            self.cam.orbit = Orbit::default();
            tracing::info!(target: "netplay", live, "pause action (0x415110): HInfo 18190 queued (0x426800, game type 3)");
            if !first_arm {
                return false;
            }
        }
        tracing::info!(target: "netplay", live, "pause action (0x415110): 0x41523A - leave the race (0x416AC0(1))");
        self.paused = false;
        self.net_notice = None;
        self.net_notice_space = false;
        true
    }

    /// The press, then the leave it asks for (0x41523A → the session loop's front-end return).
    pub(crate) fn net_pause_action(&mut self, event_loop: &LoopCtl) {
        if self.net_pause_press() && !self.leave_race_as(false) {
            event_loop.exit();
        }
    }

    /// **The input exchange's network seats' `Car_simulate`** (0x49FD30, Sc32 priority 0x1E): each seat
    /// in `seats` (race-index order) is a human car — the same step the player's car takes, from the
    /// controller sample the room's frame gave it. `local` is the player's car, which the draft sees
    /// with the other seats (live); `wet` is `LoadTransfer`'s wet test (0x4A191D…0x4A194D).
    pub(crate) fn exchange_seats_step(opponents: &mut [RaceCar], step: SeatStep) {
        let SeatStep { seats, local, tick, flags, wet, grid } = step;
        for &(oi, _, c) in seats {
            let others: Vec<Vec3> = std::iter::once(local)
                .chain(opponents.iter().enumerate().filter(|(k, _)| *k != oi).map(|(_, q)| q.vehicle.position))
                .collect();
            let Some(o) = opponents.get_mut(oi) else { continue };
            o.vehicle.race_started = tick >= RACE_START_TICK;
            o.vehicle.game_tick = tick;
            // the launch's `RALLY` / `DERBY` (`launch_flags`); `.TINYCAR` is the seat's own record's (`RaceCar::tiny`)
            o.vehicle.rally = flags.0;
            o.vehicle.tiny = o.tiny;
            o.vehicle.derby = flags.1;
            o.vehicle.race_state = if o.eliminated {
                RaceState::Out
            } else if o.progress.finished {
                RaceState::Finished
            } else {
                RaceState::Racing
            };
            o.vehicle.draft = nfs_physics::vehicle::draft_factor(o.vehicle.position, o.vehicle.forward(), o.vehicle.v_long(), &others);
            o.vehicle.wet_track = wet(o.vehicle.position);
            o.vehicle.step32(&c, &o.specs, grid);
        }
    }

    /// **The input-exchange race after the race frame's `0xC5` round** (`net_race_go`, NetPlay::exchange_race_frame
    /// set the ring up before it): the seats become human cars — no AI flag, no AI traction control
    /// (`car+0x52C & 4`, `car+0xDA8` clear) — the in-race notice is cleared and the run's state dump
    /// (`NFS_NET_DUMP`) opens.
    pub(crate) fn net_exchange_begin(&mut self) {
        self.net_notice = None;
        self.net_lag_box = false;
        let Some(n) = self.net.session.as_mut() else { return };
        if n.exchanging() {
            for (k, o) in self.road.opponents.iter_mut().enumerate() {
                if n.seat_race_index(k).is_some() {
                    o.vehicle.ai_driven = false;
                    o.vehicle.traction_control = false;
                }
            }
            if let Some(path) = nfs_game::config::cfg().nfs_net_dump.as_str()
                && let Ok(f) = std::fs::File::create(&path)
            {
                self.net_dump = Some(std::io::BufWriter::new(f));
                // the cars as the race starts, by race index: the specs and the vehicle, in `Debug`
                // text, so two machines' copies of a car can be told apart at the first field that differs
                use std::io::Write;
                if let Ok(mut sf) = std::fs::File::create(format!("{path}.cars")) {
                    let mut cars: Vec<(u32, String)> = Vec::new();
                    if let (Some(i), Some(v)) = (n.local_race_index(), self.vehicle.as_ref()) {
                        cars.push((i, format!("{:#?}\n{:#?}", self.specs, v)));
                    }
                    for (k, o) in self.road.opponents.iter().enumerate() {
                        if let Some(i) = n.seat_race_index(k) {
                            cars.push((i, format!("{:#?}\n{:#?}", o.specs, o.vehicle)));
                        }
                    }
                    cars.sort_by_key(|c| c.0);
                    for (i, t) in cars {
                        let _ = writeln!(sf, "=== car {i}\n{t}");
                    }
                }
                // the specs alone, for a diff against a replay's (`replay.md` §18.15)
                if let Ok(mut sf) = std::fs::File::create(format!("{path}.specs")) {
                    let mut cars: Vec<(u32, String)> = Vec::new();
                    if let Some(i) = n.local_race_index() {
                        cars.push((i, format!("{:#?}", self.specs)));
                    }
                    for (k, o) in self.road.opponents.iter().enumerate() {
                        if let Some(i) = n.seat_race_index(k) {
                            cars.push((i, format!("{:#?}", o.specs)));
                        }
                    }
                    cars.sort_by_key(|c| c.0);
                    for (i, t) in cars {
                        let _ = writeln!(sf, "=== car {i}\n{t}");
                    }
                }
            }
            tracing::info!(local = ?n.local_race_index(), seats = self.road.opponents.len(), "input exchange: the race runs on the room's frames (0x4152E0)");
        }
    }

    /// One turn of the network machinery the original runs beside its frame loop — the events, the
    /// room's tick, the 128 Hz timer's callbacks — and the 32 Hz controller samples they call for
    /// (0x4A46CC: `0x412E70`, `0x413800`). Answers whether a tick may run now: the **frame driver's
    /// gate** (0x415455…0x415495), `ticks done <= min(0x6559F8, 2·F0 + 1) − 1`.
    pub(crate) fn net_exchange_gate(&mut self) -> bool {
        let paused = self.paused;
        let samples = self.net.session.as_mut().map_or(0, |n| n.exchange_pump(paused));
        // (the 32 Hz samples are the input exchange's; a positional race's callbacks only count the clock)
        let samples = if self.net.session.as_ref().is_some_and(|n| n.exchanging()) { samples } else { 0 };
        for _ in 0..samples {
            if let Some(s) = self.exchange_sample_bytes()
                && let Some(n) = self.net.session.as_mut()
            {
                n.exchange_sample(s);
            }
        }
        // the tick about to run is `self.tick` itself (`[0x606AA0]`, the ticks done, which 0x414EB0 zeroes
        // with `[0x606AC4]`): 0x415B96…0x415BC4 runs it while it is at most `[0x606A9C]` = allowed − 1, and
        // latches ring entry `t / 2` for it (0x415BD4…0x415BDC). *(Until the port ran tick 0, 2026-10-22, the
        // tick about to run was `self.tick + 1`.)*
        let next = self.tick;
        self.net.session.as_ref().is_some_and(|n| next < n.exchange_ticks_allowed())
    }

    /// The scripted run's blocking form of the gate: waits for a tick to be allowed, and gives up when
    /// the link is lost, the ring overflowed or (**glue**) [`netplay::BARRIER_LIMIT`] has passed.
    pub(crate) fn net_exchange_wait(&mut self) -> bool {
        let end = Instant::now() + netplay::BARRIER_LIMIT;
        loop {
            if self.net_exchange_gate() {
                return true;
            }
            // (the frames drawn while the game waits: `HInfo`'s tick, 0x427C70)
            self.net_race_notice();
            let Some(n) = self.net.session.as_ref() else { return true };
            // (a positional race goes on without its peers: only the clock gates it, as in the original, whose positional
            // target `[0x6559F8] − 1` has no frames to wait for)
            if (n.exchanging() && (!n.established() || n.exch.stopped)) || Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    /// **A network race played back**: the same samples the live race latched, read from each human's lane of
    /// the stream (0x4144B0 for every human car, in race-index order), at the same even ticks
    /// (`replay.md` §18.15). No session runs, so this is what stands in for the exchange's latch.
    pub(super) fn replay_net_step(&mut self) {
        let tick = self.tick;
        self.exchange_in = None;
        if self.exchange_in.is_none()
            && tick % 2 == 0
            && let Some(rn) = self.replay_net.as_ref()
            && self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
        {
            let local_index = rn.local_index;
            let seat_idx: Vec<(usize, u32)> = rn.seats.iter().enumerate().filter(|(_, i)| **i != u32::MAX).map(|(k, i)| (k, *i)).collect();
            let mut order: Vec<u32> = seat_idx.iter().map(|e| e.1).chain(std::iter::once(local_index)).collect();
            order.sort_unstable();
            let mirror = self.mirror;
            let controls = |s: nfs_net::exchange::Sample| {
                let mut c = nfs_game::replay::Packet { buttons: s[0], steer: s[1] as i8, throttle: s[2], brake: s[3] }.to_controls();
                if mirror {
                    c.steer = -c.steer;
                }
                c
            };
            let mut samples: std::collections::HashMap<u32, nfs_net::exchange::Sample> = std::collections::HashMap::new();
            if let Some(rs) = self.resim.as_ref() {
                // `--resim`: the samples the room's frames hold (`nfs_game::resim`), where a replay reads its lanes
                for &i in &order {
                    samples.insert(i, rs.feed.sample(i, tick).unwrap_or([0; 4]));
                }
            } else if let Some(r) = self.replay.stream.as_mut() {
                for &i in &order {
                    let p = r.playback_player(i as usize);
                    samples.insert(i, [p.buttons, p.steer as u8, p.throttle, p.brake]);
                }
            }
            let sample = |i: u32| samples.get(&i).copied().unwrap_or([0; 4]);
            if tick % 64 == 0 {
                tracing::debug!(target: "netplay", tick, samples = ?order.iter().map(|i| (*i, sample(*i))).collect::<Vec<_>>(), "network replay: this tick's samples");
            }
            self.exchange_in = Some(ExchangeIn {
                local: controls(sample(local_index)),
                local_index,
                seats: seat_idx.iter().map(|&(k, i)| (k, i, controls(sample(i)))).collect(),
                local_horn: sample(local_index)[0] & 2 != 0,
                seat_horns: seat_idx.iter().map(|&(_, i)| sample(i)[0] & 2 != 0).collect(),
                local_raw: sample(local_index),
                seat_raw: seat_idx.iter().map(|&(_, i)| sample(i)).collect(),
            });
            // 0x4163B0: a racer whose sample carries command 1 has left — state 3
            for &(k, i) in &seat_idx {
                if sample(i)[0] >> 3 == nfs_net::exchange::CMD_LEFT
                    && let Some(o) = self.road.opponents.get_mut(k)
                    && !o.eliminated
                    && !o.progress.finished
                {
                    o.eliminated = true;
                    o.progress.finished = true;
                    o.progress.tick = tick;
                }
            }
            if sample(local_index)[0] >> 3 == nfs_net::exchange::CMD_LEFT
                && let Some(r) = self.race.as_mut()
                && !r.finished
            {
                self.ko.player_eliminated = true;
                r.finished = true;
                r.tick = tick;
            }
        }
    }

    /// Drain the network and put every remote player on its seat. Returns the seats the network
    /// owns this tick, so the AI/physics below leaves them alone — a remote car is driven by what
    /// arrived, exactly as the original's receiver drives it (`network.md` §5.2).
    pub(crate) fn net_step(&mut self) -> Vec<usize> {
        if self.net.session.is_none() {
            // (a network race played back has no session: its lanes stand in for the exchange)
            if self.replay_net.is_some() {
                self.replay_net_step();
                // every seat is a human car fed from its lane: the AI pass leaves them alone, as it leaves the
                // seats the network owns
                return (0..self.road.opponents.len()).collect();
            }
            return Vec::new();
        }
        let tick = self.tick;
        // taken out for the duration so the seats and the player's car can be borrowed too
        let mut net = self.net.session.take();
        let seats = {
            let n = net.as_mut().expect("checked");
            for line in n.poll(tick) {
                tracing::info!(target: "netplay", "{line}");
            }
            n.owned_seats()
        };
        // **The input exchange**: at an even tick the frame driver latches ring entry `t / 2` for every
        // car (0x415BD4 → 0x415EF0) and `Car_applyInput` reads each car's packet from it; the tick
        // clock is this port's `self.tick` (the value the tick runs under, `t`)
        self.exchange_in = None;
        if let Some(n) = net.as_mut()
            && n.exchanging()
            && tick % 2 == 0
            && let Some(local_index) = n.local_race_index()
        {
            n.exchange_latch((tick / 2) as u32);
            if tick % 64 == 0 {
                tracing::debug!(target: "netplay", tick, received = n.exch.received, wall = n.exch.timer.wall, pending = n.exch.ring.pending(0), "input exchange: the tick clock");
            }
            let mirror = self.mirror;
            let controls = |s: nfs_net::exchange::Sample| {
                let mut c = nfs_game::replay::Packet { buttons: s[0], steer: s[1] as i8, throttle: s[2], brake: s[3] }.to_controls();
                // 0x4992D6: `car+0xD90 = ±(steer << 1)`, negated when `MIRROR`
                if mirror {
                    c.steer = -c.steer;
                }
                c
            };
            let seat_idx: Vec<(usize, u32)> = (0..self.road.opponents.len()).filter_map(|k| n.seat_race_index(k).map(|i| (k, i))).collect();
            self.exchange_in = Some(ExchangeIn {
                local: controls(n.latched(local_index as usize)),
                local_index,
                seats: seat_idx.iter().map(|&(k, i)| (k, i, controls(n.latched(i as usize)))).collect(),
                // `b0` bit 1 is the horn, which the sample carries whole (0x48EC1C, 0x48F20A)
                local_horn: n.latched(local_index as usize)[0] & 2 != 0,
                seat_horns: seat_idx.iter().map(|&(_, i)| n.latched(i as usize)[0] & 2 != 0).collect(),
                local_raw: n.latched(local_index as usize),
                seat_raw: seat_idx.iter().map(|&(_, i)| n.latched(i as usize)).collect(),
            });
        }
        if let Some(n) = net.as_mut() {
            // **The remote cars' dead reckoning** (`NetPlay::apply`, `Vehicle::net_dead_reckon`) runs after
            // the tick's 64 Hz integration, where 0x49B8E0 calls it; what the drain leaves here is the
            // store's own snap 0x4A03D0 (`snap_due`), which the original runs inside the receiver
            if !n.exchanging()
                && let Some(w) = self.world.as_ref()
            {
                n.store_snaps(|seat, b| {
                    if let Some(o) = self.road.opponents.get_mut(seat) {
                        o.vehicle.net_store_snap(b.velocity, &b.matrix, &w.collision);
                    }
                });
            }
            // 0x416310 (0x41637D…0x4163F5, game type 3, state 0): a racer whose link is gone goes to
            // state 3 — `+0xB14 = tick`, `+0xB24 = 0`, no other change to the progress. It is the
            // Quick Knockout's "out" state (`eliminated` + `finished` here), which HPosition shows as
            // FE 18191 "DNF" (0x42B279…0x42B2C1) and `Race_finalize` ranks last with no time.
            for s in 0..self.road.opponents.len() {
                let Some(i) = n.seat_race_index(s) else { continue };
                if let Some(o) = self.road.opponents.get_mut(s)
                    && !o.eliminated
                    && !o.progress.finished
                    && n.retired(i, tick as u32)
                {
                    o.eliminated = true;
                    o.progress.finished = true;
                    o.progress.tick = tick;
                    tracing::info!(target: "netplay", seat = s, race_index = i, "network racer's link gone: state 3 (0x416310)");
                }
            }
            if let Some(i) = n.local_race_index()
                && let Some(r) = self.race.as_mut()
                && !r.finished
                && n.retired(i, tick as u32)
            {
                self.ko.player_eliminated = true;
                r.finished = true;
                r.tick = tick;
                tracing::info!(target: "netplay", race_index = i, "own link gone: state 3 (0x416310)");
            }
            // 0x416310 (0x4166F0…0x4168A9): the local car reaches `NUM_LAPS` → state 2, and with the
            // connection up (`net+8`, `+0xBF == 0`) `0x48DA60(tick − 0x1C0, best of its lap times)` sends
            // `0xB7`: the finish time (`car+0xB24` after its `−= 0x1C0`, 0x416840) and the smallest of
            // `car+0xB28 …` over the race's laps (0x416883…0x41689F). Not for a car retired to state 3.
            if !self.net_launch.time_sent
                && !self.net_launch.derby
                && !self.ko.player_eliminated
                && let Some(r) = self.race.as_ref()
                && r.finished
            {
                self.net_launch.time_sent = true;
                // `[0x606AC4] − 0x1C0` (0x4168A1 `add ebx, -0x1C0`: the finish tick less the 448-tick
                // countdown — `car+0xB24`, which 0x416840 has just taken the same amount off) and the
                // smallest of the first `NUM_LAPS` lap times (`FinalizeIn::from_progress` keeps the same
                // `tick − 448`)
                let finish = r.tick.saturating_sub(0x1C0);
                let best = r.lap_times.iter().take(self.num_laps as usize).copied().min().unwrap_or(0);
                tracing::info!(target: "netplay", tick = r.tick, finish, best, "own finish: 0xB7 (0x4168A9)");
                n.send_race_time(finish as i32, best as i32);
            }
        }
        // `NfsNet_tick` 0x48EDF0 hangs off the **64 Hz** world tick and its gate is
        // `(gGameTick & 7) == 7`, which is always an odd tick — so this cannot sit inside the
        // port's nested 32 Hz force step, or it would never fire.
        if let (Some(n), Some(v)) = (net.as_mut(), self.vehicle.as_ref()) {
            n.send(tick, v, self.net_launch.pad_buttons);
        }
        self.net.session = net;
        seats
    }

    /// **The network race's part of the replay file** (`replay.md` §18.15): what the launch settled that
    /// the single-car setup does not say — each other human's race index, name, car record and grid
    /// place, the traffic cars' records, the camera list. `None` unless this is an input-exchange race
    /// (the others' cars are not simulated in a positional one, so nothing could play them back).
    pub(crate) fn replay_net_snapshot(&self) -> Option<nfs_game::replay::NetSetup> {
        let n = self.net.session.as_ref().filter(|n| !n.positional && n.in_race != 0)?;
        let local_index = n.local_race_index()?;
        let names: Vec<String> = n.remote_players().into_iter().map(|p| p.1).collect();
        let seats = (0..(self.num_opponents as usize).max(self.net_launch.seat_cars.len()))
            .map(|k| nfs_game::replay::NetSeat {
                race_index: n.seat_race_index(k).unwrap_or(u32::MAX),
                name: names.get(k).cloned().unwrap_or_default(),
                grid: self.net_launch.seat_grids.get(k).copied().flatten(),
                words: self.net_launch.seat_cars.get(k).map(|c| c.words.iter().map(|(i, w)| (*i, w.clone())).collect()).unwrap_or_default(),
            })
            .collect();
        Some(nfs_game::replay::NetSetup {
            local_index,
            grid: self.ko.player_grid_override,
            seats,
            traffic: self.net_launch.traffic.iter().map(|t| nfs_game::replay::NetTraffic { model: t.model.clone(), name: t.name.clone(), colour: t.colour }).collect(),
            camera0: (!self.camera_cycle.is_empty()).then(|| self.camera_cycle.iter().map(|&c| u32::from(c)).collect()),
            own_words: self.net_launch.own_words.clone(),
            fe_txt: ["weather", "time", "fog"].into_iter().filter_map(|k| self.net_launch.fe_txt.get(k).map(|v| (k.to_string(), *v))).collect(),
        })
    }

    /// A replay file's network section put back where a launch would have put it (`net_race_apply`): the
    /// seats' car records and grid places, the traffic, the camera list, and the run's lanes. Nothing when
    /// the setup carries none — and then nothing of an earlier network race stays.
    pub(crate) fn apply_replay_net(&mut self) {
        self.replay_net = None;
        self.net_launch.seat_cars.clear();
        self.net_launch.own_words.clear();
        self.net_launch.fe_txt.clear();
        self.net_launch.seat_grids.clear();
        self.net_launch.traffic.clear();
        self.ko.player_grid_override = None;
        let Some(net) = self.replay.setup.net.clone() else { return };
        self.net_launch.own_words = net.own_words.clone();
        // the host's `TIME` / `WEATHER` / `FOG` the recorded race ran on (the snapshot's own words)
        self.net_launch.fe_txt = net.fe_txt.iter().filter_map(|(k, v)| Some((["weather", "time", "fog"].into_iter().find(|n| *n == k)?, *v))).collect();
        self.net_launch.seat_cars = net
            .seats
            .iter()
            .map(|s| nfs_net::launchcar::CarSlot { words: s.words.iter().cloned().collect() })
            .collect();
        self.net_launch.seat_grids = net.seats.iter().map(|s| s.grid).collect();
        self.ko.player_grid_override = net.grid;
        self.num_opponents = net.seats.len() as u32;
        self.net_launch.traffic = net
            .traffic
            .iter()
            .map(|t| nfs_net::options::TrafficCar { model: t.model.clone(), name: t.name.clone(), colour: t.colour })
            .collect();
        // the traffic cars' models and colours, as `net_race_apply` resolves the records
        self.road.traffic_ids = None;
        self.road.traffic_colours.clear();
        if !self.net_launch.traffic.is_empty() {
            let catalog = self.fe_car_catalog();
            let (mut ids, mut colours) = (Vec::new(), Vec::new());
            for c in &self.net_launch.traffic {
                let Some(e) = catalog.iter().find(|e| e.model.eq_ignore_ascii_case(&c.model)) else { continue };
                let own = e.paint_sets.first().map(|p| p.colours[0]).unwrap_or([0x8C, 0x8C, 0x90]);
                let got = [(c.colour >> 16) as u8, (c.colour >> 8) as u8, c.colour as u8];
                ids.push(e.id);
                colours.push((got != own).then_some(got));
            }
            if !ids.is_empty() {
                self.road.traffic_ids = Some(ids);
                self.road.traffic_colours = colours;
            }
        }
        self.road.traffic_count = self.net_launch.traffic.len() as u32;
        if let Some(list) = net.camera0.as_ref().filter(|l| !l.is_empty()) {
            self.camera_cycle = list.iter().map(|&v| v as u16).collect();
        }
        self.replay_net = Some(ReplayNetRun {
            local_index: net.local_index,
            seats: net.seats.iter().map(|s| s.race_index).collect(),
            names: net.seats.iter().map(|s| s.name.clone()).collect(),
        });
        // `[0x606858] = [0x65741C]` (0x413CF0): the camera starts on the recorded machine's own car
        let mut humans: Vec<u32> = net.seats.iter().map(|s| s.race_index).filter(|i| *i != u32::MAX).chain(std::iter::once(net.local_index)).collect();
        humans.sort_unstable();
        self.replay_view = humans.iter().position(|i| *i == net.local_index).unwrap_or(0);
        tracing::info!(local = net.local_index, seats = net.seats.len(), traffic = net.traffic.len(), "network replay: the race's humans and traffic from the file");
    }

    /// The seat whose car the camera views in a network replay (`[0x606858]`, an index into the humans by race
    /// index, `replay.md` 18.15); `None` = the recorded machine's own car (the viewed index is its own).
    pub(crate) fn viewed_seat_of(rn: Option<&ReplayNetRun>, view: usize) -> Option<usize> {
        let rn = rn?;
        let mut order: Vec<u32> = rn.seats.iter().copied().filter(|i| *i != u32::MAX).chain(std::iter::once(rn.local_index)).collect();
        order.sort_unstable();
        let target = *order.get(view % order.len().max(1))?;
        if target == rn.local_index { None } else { rn.seats.iter().position(|i| *i == target) }
    }

    /// **0x465D80(0)**, the car the car model's "followed car" arms follow (`carInfo+0x520 == 0x465D80(0)`:
    /// 0x437ABD the shadow, 0x4382EC the zone caps, 0x4383EA the effects, 0x43A020 the LOD pick, 0x43A4B3 the
    /// windscreen drops, 0x4430D3 the draw's gate, 0x444E18 the cabin passes): `[0x606858]` (the viewed car)
    /// while the net object has its session (`net+8`), the link is up (`+0xBF == 0`) and `+0xBD` is set (a
    /// `0xB7` has gone out: this machine's car has finished), else **`[0x65741C]`, the recorded machine's own
    /// car** - so a network replay played from a file (no session) views a remote car with the arms still on
    /// the local one. `None` = the local car, `Some(k)` = seat `k`. A session and a replay never meet in the
    /// port (a replay is a file), so the `Some` arm is not reached today.
    pub(crate) fn followed_seat(&self) -> Option<usize> {
        let viewed = Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view)?;
        let live = self.net.session.as_ref().is_some_and(|n| n.link_up() && n.finish_sent());
        live.then_some(viewed)
    }

    /// The human cars of a **positional** network race in registration order (race index, the same on every
    /// machine): the local car and every seat the network owns — a network race has no AI opponents
    /// (`NUMOPPONENTRACECARS` is 0, 0x4ECF00 for `FEGAME_TYPE` 3), so every seat is a human car whose
    /// `0x5E8B34[]` / `0x5E8E2C[]` / `0x5E8B10[]` entries the traffic reads (`traffic.md` §11).
    /// Empty outside a positional session race.
    pub(crate) fn positional_human_order(&self) -> Vec<HumanCar> {
        let Some(n) = self.net.session.as_ref().filter(|n| !n.exchanging() && n.positional) else { return Vec::new() };
        if self.road.opponents.is_empty() {
            return Vec::new();
        }
        let mut order: Vec<(u32, HumanCar)> = vec![(n.local_race_index().unwrap_or(0), HumanCar::Local)];
        order.extend((0..self.road.opponents.len()).map(|k| (n.seat_race_index(k).unwrap_or(u32::MAX), HumanCar::Seat(k))));
        // (a stable sort: a seat without an index follows the others in seat order)
        order.sort_by_key(|e| e.0);
        order.into_iter().map(|e| e.1).collect()
    }

    /// The human cars of an input-exchange race in registration order (race index), from the session —
    /// empty outside one. [`ExchangeIn::humans`] is the same list at an even tick; the audio update
    /// (odd ticks) has no latched samples, only the session.
    pub(crate) fn exch_human_order(&self) -> Vec<HumanCar> {
        // `--resim` re-runs such a race with no session and no sound: the same order from the record, so that the local car's draw on the
        // shared stream (`simulate_audio`) is made as in the race it re-runs, not in the producer gated on the sound
        if self.resim.is_some()
            && let Some(rn) = self.replay_net.as_ref()
        {
            let mut order: Vec<(u32, HumanCar)> = vec![(rn.local_index, HumanCar::Local)];
            order.extend((0..self.road.opponents.len()).map(|k| (rn.seats.get(k).copied().unwrap_or(u32::MAX), HumanCar::Seat(k))));
            // (a stable sort: a seat without an index follows the others in seat order)
            order.sort_by_key(|e| e.0);
            return order.into_iter().map(|e| e.1).collect();
        }
        let Some(n) = self.net.session.as_ref().filter(|n| n.exchanging()) else { return Vec::new() };
        let Some(local_index) = n.local_race_index() else { return Vec::new() };
        let mut order: Vec<(u32, HumanCar)> = vec![(local_index, HumanCar::Local)];
        order.extend((0..self.road.opponents.len()).filter_map(|k| n.seat_race_index(k).map(|i| (i, HumanCar::Seat(k)))));
        order.sort_by_key(|e| e.0);
        let mut order: Vec<HumanCar> = order.into_iter().map(|e| e.1).collect();
        for k in 0..self.road.opponents.len() {
            if !order.contains(&HumanCar::Seat(k)) {
                order.push(HumanCar::Seat(k));
            }
        }
        order
    }

    /// One 32 Hz sample of this machine's controller for the input exchange (0x412E70 →
    /// 0x413800): the [`App::controller_sample`] bytes as the ring holds them — the steering **raw**
    /// (the car negates it under `MIRROR` when it reads the packet, 0x4992D6, so the sample undoes
    /// the mirroring [`App::controller_sample`] applied on the way in).
    pub(super) fn exchange_sample_bytes(&mut self) -> Option<nfs_net::exchange::Sample> {
        let v = self.vehicle.as_ref()?;
        let c = Self::controller_sample(&mut self.input, self.mirror, self.follow_road, self.slices.as_ref(), self.race.as_ref(), v);
        // TEST GLUE `NFS_PA_INPUT`: the scripted pad is what the exchange sends too (not the original's)
        let c = crate::app::glue::pa_trace::input(self.tick, c);
        let mut c = c;
        if self.mirror {
            c.steer = -c.steer;
        }
        let p = nfs_game::replay::Packet::from_controls(&c);
        // bit 1 is the horn, the second held button (`car+0xD86`, `controls.md`)
        let horn = u8::from(self.live_horn()) << 1;
        Some([p.buttons | horn, p.steer as u8, p.throttle, p.brake])
    }
}
