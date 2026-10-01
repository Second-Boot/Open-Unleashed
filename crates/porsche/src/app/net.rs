//! Network play on the application side: the lobby callbacks, the lock-step exchange, the race-start sync and the net state types.

use crate::*;

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
        let page_up = self.results.is_some() && self.result_pages().get(self.results_page as usize).is_some();
        // the first arm: `[0x5E99F4] < 2`, `[0x606A88] == 0`, `[0x606AC4] > 0x20` and no live link
        let first_arm = !live && self.tick > 0x20;
        let queue = self.paused || self.net_notice.is_some() || page_up;
        if !queue {
            // 0x426800, type 3
            self.net_notice = Some(18190);
            self.net_notice_space = true;
            self.orbit = Orbit::default();
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
    pub(crate) fn net_pause_action(&mut self, event_loop: &ActiveEventLoop) {
        if self.net_pause_press() && !self.leave_race_as(false) {
            event_loop.exit();
        }
    }

    /// **The input exchange's network seats' `Car_simulate`** (0x49FD30, Sc32 priority 0x1E): each seat
    /// in `seats` (race-index order) is a human car — the same step the player's car takes, from the
    /// controller sample the room's frame gave it. `local` is the player's car, which the draft sees
    /// with the other seats (live); `wet` is `LoadTransfer`'s wet test (0x4A191D…0x4A194D).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn exchange_seats_step(
        opponents: &mut [RaceCar],
        seats: &[(usize, u32, Controls)],
        local: Vec3,
        tick: u64,
        flags: (bool, bool),
        wet: &dyn Fn(Vec3) -> bool,
        grid: &nfs_physics::collision::TriangleGrid,
    ) {
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
                3
            } else if o.progress.finished {
                2
            } else {
                0
            };
            o.vehicle.draft = nfs_physics::vehicle::draft_factor(o.vehicle.position, o.vehicle.forward(), o.vehicle.v_long(), &others);
            o.vehicle.misfire_roll = nfs_game::rng::rand01();
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
            if let Ok(path) = nfs_game::config::cfg().nfs_net_dump.var()
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
        for _ in 0..samples {
            if let Some(s) = self.exchange_sample_bytes()
                && let Some(n) = self.net.session.as_mut()
            {
                n.exchange_sample(s);
            }
        }
        let next = self.tick + 1;
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
            if !n.established() || n.exch.stopped || Instant::now() > end {
                return false;
            }
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    /// **A network race played back**: the same samples the live race latched, read from each human's lane of
    /// the stream (0x4144B0 for every human car, in race-index order), at the same even ticks
    /// (`replay.md` §18.15). No session runs, so this is what stands in for the exchange's latch.
    fn replay_net_step(&mut self) {
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
            if let Some(r) = self.replay.stream.as_mut() {
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
                self.player_eliminated = true;
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
                self.player_eliminated = true;
                r.finished = true;
                r.tick = tick;
                tracing::info!(target: "netplay", race_index = i, "own link gone: state 3 (0x416310)");
            }
            // 0x416310 (0x4166F0…0x4168A9): the local car reaches `NUM_LAPS` → state 2, and with the
            // connection up (`net+8`, `+0xBF == 0`) `0x48DA60(tick − 0x1C0, best of its lap times)` sends
            // `0xB7`: the finish time (`car+0xB24` after its `−= 0x1C0`, 0x416840) and the smallest of
            // `car+0xB28 …` over the race's laps (0x416883…0x41689F). Not for a car retired to state 3.
            if !self.net_time_sent
                && !self.net_derby
                && !self.player_eliminated
                && let Some(r) = self.race.as_ref()
                && r.finished
            {
                self.net_time_sent = true;
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
            n.send(tick, v, self.net_pad_buttons);
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
        let seats = (0..(self.num_opponents as usize).max(self.net_seat_cars.len()))
            .map(|k| nfs_game::replay::NetSeat {
                race_index: n.seat_race_index(k).unwrap_or(u32::MAX),
                name: names.get(k).cloned().unwrap_or_default(),
                grid: self.net_seat_grids.get(k).copied().flatten(),
                words: self.net_seat_cars.get(k).map(|c| c.words.iter().map(|(i, w)| (*i, w.clone())).collect()).unwrap_or_default(),
            })
            .collect();
        Some(nfs_game::replay::NetSetup {
            local_index,
            grid: self.player_grid_override,
            seats,
            traffic: self.net_traffic.iter().map(|t| nfs_game::replay::NetTraffic { model: t.model.clone(), name: t.name.clone(), colour: t.colour }).collect(),
            camera0: (!self.camera_cycle.is_empty()).then(|| self.camera_cycle.iter().map(|&c| u32::from(c)).collect()),
            own_words: self.net_own_words.clone(),
            fe_txt: ["weather", "time", "fog"].into_iter().filter_map(|k| self.net_fe_txt.get(k).map(|v| (k.to_string(), *v))).collect(),
        })
    }

    /// A replay file's network section put back where a launch would have put it (`net_race_apply`): the
    /// seats' car records and grid places, the traffic, the camera list, and the run's lanes. Nothing when
    /// the setup carries none — and then nothing of an earlier network race stays.
    pub(crate) fn apply_replay_net(&mut self) {
        self.replay_net = None;
        self.net_seat_cars.clear();
        self.net_own_words.clear();
        self.net_fe_txt.clear();
        self.net_seat_grids.clear();
        self.net_traffic.clear();
        self.player_grid_override = None;
        let Some(net) = self.replay.setup.net.clone() else { return };
        self.net_own_words = net.own_words.clone();
        // the host's `TIME` / `WEATHER` / `FOG` the recorded race ran on (the snapshot's own words)
        self.net_fe_txt = net.fe_txt.iter().filter_map(|(k, v)| Some((["weather", "time", "fog"].into_iter().find(|n| *n == k)?, *v))).collect();
        self.net_seat_cars = net
            .seats
            .iter()
            .map(|s| nfs_net::launchcar::CarSlot { words: s.words.iter().cloned().collect() })
            .collect();
        self.net_seat_grids = net.seats.iter().map(|s| s.grid).collect();
        self.player_grid_override = net.grid;
        self.num_opponents = net.seats.len() as u32;
        self.net_traffic = net
            .traffic
            .iter()
            .map(|t| nfs_net::options::TrafficCar { model: t.model.clone(), name: t.name.clone(), colour: t.colour })
            .collect();
        // the traffic cars' models and colours, as `net_race_apply` resolves the records
        self.road.traffic_ids = None;
        self.road.traffic_colours.clear();
        if !self.net_traffic.is_empty() {
            let catalog = self.fe_car_catalog();
            let (mut ids, mut colours) = (Vec::new(), Vec::new());
            for c in &self.net_traffic {
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
        self.road.traffic_count = self.net_traffic.len() as u32;
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
    /// port (a replay is a file), so the `Some` arm runs only under the test glue `NFS_FOLLOW_VIEWED`.
    pub(crate) fn followed_seat(&self) -> Option<usize> {
        let viewed = Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view)?;
        let live = self.net.session.as_ref().is_some_and(|n| n.link_up() && n.finish_sent()) || nfs_game::config::cfg().nfs_follow_viewed.var_os().is_some();
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

    /// **Test glue** (`NFS_NET_DUMP`): every human car of an input-exchange race after the tick, one line
    /// each in race-index order — `tick,index,` then the position, velocity, orientation, spin, rpm and
    /// steer as `f32` bit patterns, and the gear.
    pub(crate) fn net_dump_tick(out: &mut std::io::BufWriter<std::fs::File>, local: Option<u32>, seat_index: &[Option<u32>], v: &Vehicle, opponents: &[RaceCar], tick: u64) {
        use std::io::Write;
        let mut cars: Vec<(u32, &Vehicle)> = Vec::new();
        if let Some(i) = local {
            cars.push((i, v));
        }
        for (k, o) in opponents.iter().enumerate() {
            if let Some(i) = seat_index.get(k).copied().flatten() {
                cars.push((i, &o.vehicle));
            }
        }
        cars.sort_by_key(|c| c.0);
        for (i, c) in cars {
            let mut f: Vec<f32> = Vec::new();
            f.extend(c.position.to_array());
            f.extend(c.velocity.to_array());
            f.extend(c.orientation.to_cols_array());
            f.extend(c.angular_velocity.to_array());
            f.push(c.rpm);
            f.push(c.steer as f32);
            let bits: Vec<String> = f.iter().map(|x| format!("{:08x}", x.to_bits())).collect();
            let _ = writeln!(out, "{},{},{},{}", tick, i, bits.join(","), c.gear);
        }
    }

    /// One 32 Hz sample of this machine's controller for the input exchange (0x412E70 →
    /// 0x413800): the [`App::controller_sample`] bytes as the ring holds them — the steering **raw**
    /// (the car negates it under `MIRROR` when it reads the packet, 0x4992D6, so the sample undoes
    /// the mirroring [`App::controller_sample`] applied on the way in).
    fn exchange_sample_bytes(&mut self) -> Option<nfs_net::exchange::Sample> {
        let v = self.vehicle.as_ref()?;
        let mut c = Self::controller_sample(&mut self.input, self.mirror, self.follow_road, self.slices.as_ref(), self.race.as_ref(), v, true);
        if self.mirror {
            c.steer = -c.steer;
        }
        let p = nfs_game::replay::Packet::from_controls(&c);
        // bit 1 is the horn, the second held button (`car+0xD86`, `controls.md`)
        let horn = u8::from(self.input.held(Action::Horn)) << 1;
        Some([p.buttons | horn, p.steer as u8, p.throttle, p.brake])
    }

    /// The four `Track1.*` accessors as the option table names them (`network.md` §11.6):
    /// `TRACKLOCALEID0` 0xC (the track record's `+4` name id), `TRACKDIRECTION0` 0xD (the direction
    /// row, `REVERSE + 2·MIRROR`), `TRACKTRAFFIC0` 0xE (the traffic row) and `TRACKLAPS0` 0xF (the
    /// laps row, laps − 1), with whether the track is a circuit (`trk+0x6E`, which 0x509B00 asks
    /// before it reads the laps row).
    fn net_track_options(&self) -> Option<([(u32, u32); 4], bool)> {
        let m = self.fe.menu.as_ref()?;
        let t = self.fe.track_tables.tracks.iter().find(|r| r.file.eq_ignore_ascii_case(m.track()))?;
        let dir = usize::from(m.backwards) | (usize::from(m.mirror) << 1);
        Some((
            [
                (0xC, t.name_string_id),
                (0xD, dir as u32),
                (0xE, m.traffic.min(3)),
                (0xF, m.laps.clamp(1, LAPS_MAX) - 1),
            ],
            t.circuit,
        ))
    }

    /// The front end's half of the option records, once per tick on a lobby screen
    /// (`network.md` §11.6).
    ///
    /// **Host.** `Track1.Track`, `.Direction` and `.Laps` are mode-2 accessors: a selection change
    /// sends `(id, value)` as a `0xBC` (0x4BC240 → 0x4B7F70 → slot 1 → 0x48D550); the
    /// port sends when the value differs from what was last sent (the first look only records it,
    /// the original's accessors are set up before any session exists). `Track1.Traffic` is mode 0
    /// and goes with the launch only. The session's own descriptor (what a browser's `'d'` shows,
    /// 0x509B00) follows the same accessors.
    ///
    /// **Player.** The records that came (0x48F998 → 0x4B4B80 writes the option table) are what
    /// the launch reads: `TRACKLOCALEID0` selects the track record whose `+4` is that id — the
    /// first when none is (0x4FBF70's rule for a machine that does not host, `hud.md`) —, the rest
    /// are the direction / traffic / laps rows.
    pub(crate) fn net_sync_options(&mut self) {
        let Some(host) = self.net.session.as_ref().map(|n| n.is_host()) else { return };
        if host {
            let Some((opts, circuit)) = self.net_track_options() else { return };
            let positional = self.net.session.as_ref().is_some_and(|n| n.positional);
            let Some(n) = self.net.session.as_mut() else { return };
            for (id, v) in [opts[0], opts[1], opts[3]] {
                n.sync_option(id, v);
            }
            // 0x509B00: laps' row only on a circuit, the direction's slot 27 (`0x5B963C[row]`), the
            // track's name id
            n.set_descriptor(
                u8::from(positional),
                if circuit { opts[3].1 as u8 } else { 0 },
                DIRECTION_OPTIONS[opts[1].1 as usize & 3] as u16,
                opts[0].1 as u16,
            );
            return;
        }
        let Some(got) = self.net.session.as_mut().map(|n| n.take_options()).filter(|g| !g.is_empty()) else { return };
        let Some(m) = self.fe.menu.as_mut() else { return };
        for (id, v) in got {
            match id {
                0xC => {
                    let at = self.fe.track_tables.tracks.iter().position(|r| r.name_string_id == v);
                    let file = at.map(|i| self.fe.track_tables.tracks[i].file.clone());
                    m.track_idx = file
                        .and_then(|f| m.tracks.iter().position(|t| t.eq_ignore_ascii_case(&f)))
                        .unwrap_or(0);
                }
                0xD => {
                    m.backwards = v & 1 != 0;
                    m.mirror = v & 2 != 0;
                }
                0xE => m.traffic = v.min(3),
                0xF => m.laps = (v + 1).min(LAPS_MAX),
                _ => {}
            }
        }
        self.bind_lists();
        self.bind_checks();
    }

    /// The host's launch settings as 0x4ED5D0's chain would build them for a network game.
    fn net_launch_settings(&mut self) -> Option<nfs_net::options::LaunchSettings> {
        let m = self.fe.menu.as_ref()?;
        let t = self.fe.track_tables.tracks.iter().find(|r| r.file.eq_ignore_ascii_case(m.track()))?;
        let positional = self.net.session.as_ref().is_some_and(|n| n.positional);
        // `Comms.Collision`'s row 0 is "off" (FEStrings 1500)
        let collision = self.choices.get("Comms.Collision").copied().unwrap_or(0);
        let players = self.net.session.as_ref().map_or(1, |n| n.room_len().max(1)) as u32;
        let mut s = nfs_net::options::LaunchSettings {
            players,
            traffic: Vec::new(),
            positional: u32::from(positional),
            direction_row: (usize::from(m.backwards) | (usize::from(m.mirror) << 1)) as u32,
            laps_row: m.laps.clamp(1, LAPS_MAX) - 1,
            track_name_id: t.name_string_id,
            traffic_row: m.traffic.min(3),
            playlist: self.current_playlist(),
            skill: u32::from(m.difficulty.min(2)),
            damage: u32::from(m.damage),
            track: m.track().to_string(),
            linear: !t.circuit,
            laps: fe_laps(&m.circuits, m.track(), m.laps),
            traffic_cars: TRAFFIC_COUNT[m.traffic.min(3) as usize],
            nocollision: positional && collision == 0,
            derby: false,
            // 0x4ED5D0: `0x4D8790(0)` → `RALLY` 0x19, `0x4D8790(8)` → `HEAVY` 0x1A, each `1` and only when set
            rally: self.cheat_bits & 1 != 0,
            heavy: self.cheat_bits & (1 << 8) != 0,
            own_car: Vec::new(),
        };
        // the "Smash Up" cheat (cheat bit 7): 0x4ED7D9…0x4ED841 sets `DERBY`, the skid pad, damage on,
        // `NOCOLLISION` off and `LINEAR_TRACK` 0 after the mode's own options — so the stream the host's
        // launch sends says so (`derby_track`, `network.md` §11.18)
        if self.cheat_bits & (1 << 7) != 0 {
            s.derby = true;
            s.track = "skidpad".to_string();
            s.linear = false;
            s.damage = 1;
            s.nocollision = false;
        }
        let mut fe_rng = self.fe_rng.clone();
        s.traffic = self.net_traffic_cars(&s.track, s.traffic_kept(), &mut || fe_rng.next_u32());
        self.fe_rng = fe_rng;
        Some(s)
    }

    /// The traffic cars 0x4ECF00 builds for a network game, as their per-slot records
    /// (0x4ED4E0…0x4ED56E → 0x4EEDA0, kind 4): car `k` is the track record's `trk+0x7C[k % 7]`
    /// (`.FILENAME`, `.FEMULTIPLAYERCARNAME`), painted with the colour the loop draws from the front
    /// end's 0x538040 — four draws per car, in order after the record 0x4F4400 (0x4ED4E4…0x4ED51A):
    /// `a << 24 | (b & 0xFF) << 16 | (c & 0xFF) << 8 | (d & 0xFF)`, alpha included, handed to the
    /// car's constructor as the exterior and all three other colours (0x4ED53B…0x4ED542) and to
    /// 0x4EEDA0 as `.COLOUR`. `draw` is that generator; a caller that only wants the model
    /// list passes one it does not mind spending.
    fn net_traffic_cars(&self, track: &str, count: u32, draw: &mut impl FnMut() -> u32) -> Vec<nfs_net::options::TrafficCar> {
        let trk = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.trk"))
            .ok()
            .and_then(|d| nfs_formats::career::parse_track_table(&d).ok())
            .unwrap_or_default();
        let ids: Vec<u16> = trk
            .iter()
            .find(|r| r.file.eq_ignore_ascii_case(track))
            .map(|r| r.traffic_cars.iter().copied().filter(|i| *i != 0).collect())
            .unwrap_or_default();
        let catalog = self.fe_car_catalog();
        (0..count as usize)
            .filter_map(|k| {
                // 0x4ED4E4…0x4ED509: the four draws come before the record is even looked at
                let (a, b, c, d) = (draw(), draw(), draw(), draw());
                let colour = a << 24 | (b & 0xFF) << 16 | (c & 0xFF) << 8 | (d & 0xFF);
                let e = catalog.iter().find(|c| Some(&c.id) == ids.get(k % ids.len().max(1)))?;
                Some(nfs_net::options::TrafficCar {
                    model: e.model.clone(),
                    name: e.display_name.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect(),
                    colour,
                })
            })
            .collect()
    }

    /// **0x4B4DA0's network half**, run by every machine once the front end has handed over: the
    /// host sends the launch stream (0x48D490 / 0x48D440) and draws the grid (0x4B4FD0), then all
    /// run the barrier 0x48D6A0, after which a player's table holds the host's records. What the
    /// launch settled goes to `net.race`, which `start_from_layout` reads.
    pub(crate) fn net_launch_sync(&mut self) {
        let host = self.net.session.as_ref().is_some_and(|n| n.is_host());
        let mut settings = if host { self.net_launch_settings() } else { None };
        // 0x4EEDA0's record for the player (kind 1) — every machine builds it and the walk sends it
        let own_car = self.net_own_launch_car();
        let Some(n) = self.net.session.as_mut() else { return };
        // 0x4B4E28 — 0x48D3D0: the barrier, then every player's finish time and race index back to −1
        n.launch_begin();
        // `n` (`ebx` of 0x4B4DA0, the players loop 0x4B4EA7…0x4B4ED9): the players `net+0xE5` marks,
        // not the room's count — 0x4B5079 stores it as NUMPLAYERRACECARS
        let players = n.race_indices().len().max(1) as u32;
        // the own-slot arm of the walk (0x4B4F25…0x4B4F66): the slot rewritten to the race index
        let own_recs = own_car.as_ref().map(|c| n.own_launch_records(c)).unwrap_or_default();
        if let Some(s) = settings.as_mut() {
            s.own_car = own_recs.clone();
        } else {
            // a player's walk sends only its own slot's records (the host's broadcast bands are not its)
            n.send_launch(&own_recs);
        }
        let settings = settings;
        if let Some(s) = settings.as_ref() {
            // 0x4B6660 builds the list from `fe.txt` and the launch chain appends after it; 0x4B6560
            // keeps the last record of an id (`nfs_net::options::broadcast_stream`)
            let fe_text = std::fs::read_to_string(self.paths.root.join("fe.txt")).unwrap_or_default();
            let fe_recs = nfs_net::options::fe_txt_records(&fe_text);
            let mut stream = nfs_net::options::broadcast_stream(&fe_recs, &s.stream());
            // the launch chain's own `rand()` draws come first: 0x4EEDA0 starts every car's record
            // with one (0x4EEDAC) — the player's (0x4EBA4D) and each kept traffic car's
            // (0x4ED56E), none for opponents or cops in a network game
            for _ in 0..1 + s.traffic_kept() {
                self.crt_rand.next();
            }
            // 0x4B4FD0…0x4B5043: `rand() % left` out of what is left of `0…n−1`, on the C runtime's
            // own generator (`App::crt_rand`)
            let crt = &mut self.crt_rand;
            let grid = nfs_net::options::grid_draw(players, || crt.next() as u32);
            n.note_grid(&grid);
            stream.extend(grid);
            n.send_launch(&stream);
        }
        use nfs_net::options::id as opt;
        let passed = n.launch_barrier();
        // the records' slot is the **race index** — `player+0x4C`, the player's place in the room's
        // list among those in the race (0x4B4EBC, 0x4B4ED2) — not the player id
        let index = n.race_indices();
        // `player+0x4C` / `[0x65741C]`, which 0x490140 (the link classification) and the seats read
        n.note_race_indices(&index);
        let me = index.iter().find(|(id, _)| *id == n.my_player_id()).map_or(0, |(_, i)| *i);
        let got = n.launch.clone();
        let own = settings.as_ref();
        // the host's values are its own settings; a player's are the records it heard (the
        // barrier has run, so the host's stream is in), falling back on its own menu
        let m = self.fe.menu.as_ref();
        let race = NetRace {
            track: got.track.clone().or_else(|| own.map(|s| s.track.clone())),
            laps: got.laps.or(own.map(|s| s.laps)).unwrap_or_else(|| m.map_or(1, |m| m.laps)),
            backwards: got.reverse.or(own.map(|s| s.direction_row & 1)).map_or(m.is_some_and(|m| m.backwards), |v| v != 0),
            mirror: got.mirror.or(own.map(|s| (s.direction_row >> 1) & 1)).map_or(m.is_some_and(|m| m.mirror), |v| v != 0),
            damage: got.damage.or(own.map(|s| s.damage)).map_or(m.is_none_or(|m| m.damage), |v| v != 0),
            skill: got.skill.or(own.map(|s| s.skill)).unwrap_or(1).min(2) as u8,
            traffic_cars: got.num_cars.map_or_else(|| own.map_or(0, |s| s.traffic_kept()), |c| c.saturating_sub(nfs_net::options::PLAYER_SLOTS)),
            positional: got.positional.or(own.map(|s| s.positional)).is_none_or(|v| v != 0),
            nocollision: got.nocollision.or(own.map(|s| u32::from(s.nocollision))).is_some_and(|v| v != 0),
            derby: got.derby.or(own.map(|s| u32::from(s.derby))).is_some_and(|v| v != 0),
            rally: got.rally.or(own.map(|s| u32::from(s.rally))).is_some_and(|v| v != 0),
            heavy: got.heavy.or(own.map(|s| u32::from(s.heavy))).is_some_and(|v| v != 0),
            players,
            opponents: got.num_opponent_cars.or(own.map(|_| 0)).unwrap_or(0),
            // `Grid_slot` 0x410580 takes the drawn value as `g` itself (0-based here, so the value 0
            // sits one row ahead of the pole `g = 1`); `grid_pos` is `g − 1`, the 0 as `u32::MAX`
            grid: got.grid.get(&me).map(|g| g.wrapping_sub(1)),
            seat_grids: index
                .iter()
                .filter(|(id, _)| *id != n.my_player_id())
                .map(|(_, i)| got.grid.get(i).map(|g| g.wrapping_sub(1)))
                .collect(),
            playlist: got.playlist.clone(),
            seat_cars: n.remote_cars().into_iter().map(|(_, _, _, c)| c).collect(),
            // this machine's own record is in `launch.cars[me]` too (`own_launch_records`), so index
            // 0 is whoever holds it — the record the walk's relay delivered, or our own
            camera0: got.cars.get(&0).and_then(|c| c.array(nfs_net::launchcar::id::CAMERA)),
            own_words: got.cars.get(&me).map(|c| c.words.iter().map(|(i, w)| (*i, w.clone())).collect()).unwrap_or_default(),
            traffic: if host { own.map(|s| s.traffic.clone()).unwrap_or_default() } else { got.traffic.values().cloned().collect() },
            fe_txt: [("rand_seed", opt::RAND_SEED), ("time", opt::TIME), ("weather", opt::WEATHER), ("fog", opt::FOG)]
                .into_iter()
                .filter_map(|(k, id)| got.number(id).map(|v| (k, v)))
                .collect(),
        };
        tracing::info!(target: "netplay", host, passed, ?race, "network launch settled (0x4B4DA0)");
        self.net.race = Some(race);
    }

    /// The settled network race onto the launch's own set-up (`start_from_layout`'s arms have just
    /// read the menu): the same fields the original's `0x4B4B80` writes from the stream. Returns
    /// the track to load.
    pub(crate) fn net_race_apply(&mut self, track: String) -> String {
        // a live race is not a replay of one
        self.replay_net = None;
        let Some(r) = self.net.race.take() else {
            // every session re-reads `fe.txt` (0x4B6C1C): nothing a network launch wrote stays
            self.net_traffic.clear();
            self.net_derby = false;
            self.net_flags = None;
            self.net_fe_txt.clear();
            self.net_nocollision = false;
            self.net_seat_grids.clear();
            self.net_seat_cars.clear();
            self.net_own_words.clear();
            self.derby_hud_laps = None;
            return track;
        };
        self.backwards = r.backwards;
        self.mirror = r.mirror;
        self.num_laps = r.laps.max(1);
        // a Smash Up has no laps: the knockout arm ends every path of 0x416310 before the lap code
        // (0x4164C0), so no car ever counts one (`car+0xB20` stays 0) and none ends the race. The
        // port's lap tracking is kept from ending it by a count no race reaches (99), and the
        // HUD's lap counter — which the original still draws, `1/NUM_LAPS`, 0x432F92 with
        // `[0x657494]` clear — reads the launch's real count from `derby_hud_laps`.
        self.derby_hud_laps = r.derby.then_some(r.laps.max(1));
        if r.derby {
            self.num_laps = 99;
        }
        self.road.damage_on = r.damage;
        self.race_skill = r.skill;
        // 0x4ECF00: no cops, `nOpp` 0 in a network game; the humans are the room's players
        // (0x4B5079's `NUMPLAYERRACECARS = ebx`), one of which is this machine
        self.road.cops_race = false;
        self.road.cops_enabled = false;
        self.road.traffic_count = r.traffic_cars;
        self.road.traffic_ids = None;
        self.road.traffic_colours.clear();
        // **the traffic cars' records** (0x4B4B80 wrote the host's `.FILENAME` / `.COLOUR` per
        // slot): the model each is, by file name, and the colour where it is not the model's own
        if !r.traffic.is_empty() {
            let catalog = self.fe_car_catalog();
            let (mut ids, mut colours) = (Vec::new(), Vec::new());
            for c in &r.traffic {
                let Some(e) = catalog.iter().find(|e| e.model.eq_ignore_ascii_case(&c.model)) else {
                    tracing::warn!(model = c.model, "network traffic car: no such model in this install");
                    continue;
                };
                let own = e.paint_sets.first().map(|p| p.colours[0]).unwrap_or([0x8C, 0x8C, 0x90]);
                let got = [(c.colour >> 16) as u8, (c.colour >> 8) as u8, c.colour as u8];
                ids.push(e.id);
                colours.push((got != own).then_some(got));
            }
            let own: Vec<String> = self.net_traffic_cars(r.track.as_deref().unwrap_or(&track), r.traffic.len() as u32, &mut || 0).into_iter().map(|c| c.model).collect();
            tracing::info!(target: "netplay", traffic = ?r.traffic.iter().map(|c| (c.model.as_str(), format!("{:08X}", c.colour))).collect::<Vec<_>>(), own_table_says = ?own, "network race: the traffic cars of the host's records");
            if !ids.is_empty() {
                self.road.traffic_ids = Some(ids);
                self.road.traffic_colours = colours;
            }
        }
        if r.opponents != 0 {
            tracing::warn!(target: "netplay", opponents = r.opponents, "the stream carries NUMOPPONENTRACECARS != 0: the original numbers opponent slots after the players (0x4B5081); this port has no AI opponent in a network race");
        }
        self.num_opponents = r.players.saturating_sub(1);
        // the camera reads `car[0].camera` (`[0x658294]`, slot 0 — 0x465A60 is called with 0 by
        // 0x415AF2, 0x462F30 / 0x465790 with the camera slot): in a network race the table is by
        // **race index**, so that is the list of whoever holds index 0 — the host, in list order —
        // on every machine, and the other players' lists are stored and never read. Nothing else
        // reads `.CAMERA` (a scan of `.text` for 0x658294: 0x463226, 0x46582A, 0x465B72, 0x465B93).
        if let Some(list) = r.camera0.as_ref().filter(|l| !l.is_empty()) {
            self.camera_cycle = list.iter().map(|&v| v as u16).collect();
        }
        // the drawn `.STARTINGPOS` is `Grid_slot`'s `g` as it stands (the values are 0…n−1, so the
        // 0 sits one row ahead of the pole `g = 1`, the original's own arithmetic); `grid_pos` is
        // `g − 1` (`u32::MAX` for 0, which `grid_slot` reads back as `g = 0`)
        self.player_grid_override = r.grid;
        self.net_seat_grids = r.seat_grids.clone();
        self.net_seat_cars = r.seat_cars.clone();
        self.net_own_words = r.own_words.clone();
        self.net_traffic = r.traffic.clone();
        self.net_derby = r.derby;
        self.net_flags = Some((r.rally, r.heavy, r.derby));
        if let Some(n) = self.net.session.as_mut() {
            n.positional = r.positional;
        }
        if let Some(list) = r.playlist.as_ref()
            && !self.net.session.as_ref().is_some_and(|n| n.is_host())
        {
            self.set_cd_playlist(list.iter().map(|i| *i as usize).collect());
        }
        // 0x4B4B80 wrote the host's `fe.txt` records into the option table: what this machine's own
        // file said is replaced for the ids the stream carried (a host's own values are its file's)
        self.net_fe_txt = r.fe_txt.iter().copied().collect();
        self.net_nocollision = r.positional && r.nocollision;
        // NOCOLLISION (0x65740C) and COMMSPOSITIONAL (0x657408): 0x498FF0's car-car half is skipped
        // while both are set (`net_nocollision`, read by the collision pass)
        tracing::info!(
            track = r.track.as_deref().unwrap_or(&track), laps = self.num_laps, reverse = self.backwards, mirror = self.mirror,
            damage = self.road.damage_on, skill = self.race_skill, traffic = self.road.traffic_count, opponents = self.num_opponents,
            startingpos = g_log(self.player_grid_override), positional = r.positional, nocollision = r.nocollision,
            "network race start: settings applied"
        );
        r.track.filter(|t| !t.is_empty()).unwrap_or(track)
    }

    /// `CREATE`, `JOIN`, `CONNECT`, `DIAL`, `ENTER`, `EJECTPLAYER` and `READY` — the callbacks the
    /// network layouts name.
    ///
    /// **`CREATE` and `JOIN` open a lobby screen, they do not start a race.** `0x509FA0`, the
    /// handler behind `ID_COMMSCREATESCREEN` / `ID_COMMSJOINSCREEN`, calls `0x4D2100(this,
    /// "creategame")` at 0x50A0DB and `0x4D2100(this, "joingame")` at 0x50AB31 — that helper opens
    /// the named layout. The data agrees: `creategame` carries `canplaygame=2`, so the host starts
    /// the race from it, and `joingame` carries `canplaygame=0`, so the joiner cannot — it presses
    /// `READY` (0x50A877 looks the control up by name and 0x48CAA0 tests it) and waits.
    ///
    /// `DIAL` has no modem behind it here, and `ENTER` on the server list only moves the cursor,
    /// since what the original's server list connects *to* is the lobby service of §1.
    pub(crate) fn network_callback(&mut self, cb: &str, _event_loop: &ActiveEventLoop) {
        let opened = match cb {
            // `CREATE` on the games screens (0x509E40, registered at 0x509D53 for `local` and
            // `udpip`) raises the game-name dialog; on the peer-to-peer screen (0x50B140) it goes
            // straight to 0x509A00 with `Comms.GameName`'s text as it stands
            "CREATE" if self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("modem")) => {
                // 0x50AFA0: `0x48E1A0(net, 2, 0, 0)`, `[this+0x294] = 0`, then 0x509A00(ModemList's index) — a game
                // over the modem; unreachable without a modem (`ID_MODEMCREATEGAME` is disabled, `net_browse_step`)
                tracing::info!("CREATE on the modem screen (0x50AFA0): no modem transport in this port");
                return;
            }
            "CREATE" => {
                let p2p = self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("peertopeerip"));
                let stored = std::fs::read_to_string(netplay::game_name_path()).unwrap_or_default().trim().to_string();
                if p2p {
                    self.net.lobby.as_ref().map(|l| l.create(&stored))
                } else {
                    // 0x509E80…0x509F4F: an empty accessor text becomes `sprintf(FEStrings[602], name)`;
                    // 0x514AB0 copies it into the field (at most 0x19 bytes) and the dialog goes up
                    // **without** emptying the field (`raise` with 0, 0x514880)
                    let who = self.player_name.to_string();
                    let text = if stored.is_empty() {
                        self.fe.strings.get(602).unwrap_or_default().replacen("%s", &who, 1)
                    } else {
                        stored
                    };
                    let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
                    let prompt = self.fe.strings.get(601).unwrap_or_default().to_string();
                    let mut d = nfs_game::dialog::Dialog::input(prompt, false, netplay::GAME_NAME_DIALOG_MAX);
                    d.raise(now);
                    if let Some(f) = d.field.as_mut() {
                        f.text = text.chars().take(netplay::GAME_NAME_DIALOG_MAX).collect();
                    }
                    self.fe.dialog = Some((d, DialogFor::NetGameName));
                    return;
                }
            }
            // `JOIN` (`udpip` / `local`): the row of `Comms.GamesList` under the cursor
            // 0x50B6A0 / 0x50AA80: the row's version (`[entry+0x8C]`) against ours (`net+0x2C`)
            // first — 822 `IDS_WRONGVERSION` (0x4B9E80) — then its player count against 8 — 286
            // "the game is full" (0x4B9DA0); only then the connect and the `joingame` screen
            "JOIN" => {
                let row = self.net.lobby.as_ref().and_then(|l| l.rows().into_iter().nth(l.sel));
                if let Some(r) = row.as_ref().filter(|r| r.version != netplay::game_version()) {
                    tracing::info!(version = %r.version, "JOIN refused: wrong version (0x4B9E80)");
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 822, [0, 0], DialogFor::Notice);
                    return;
                }
                if row.as_ref().is_some_and(|r| r.players >= 8) {
                    tracing::info!("JOIN refused: the game is full (0x4B9DA0)");
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 286, [0, 0], DialogFor::Notice);
                    return;
                }
                self.net.lobby.as_ref().and_then(|l| l.join_target().map(|a| l.connect(a)))
            }
            // `CONNECT` (0x50B150): the current row of `Comms.IPList`; with the list empty 0x50B1D9
            // raises `IDS_MUSTHAVEPEERTOPEERADDRESS` (816) and goes nowhere
            "CONNECT" => {
                // **OBSERVED on the original (SC 2026-10-01, Wine):** after a
                // click into the `Comms.IP` box, the typed `127.0.0.1` and a click on CONNECT, the
                // drop-down of `Comms.IPList` held `127.0.0.1` (empty before; the box kept the text).
                // So the typed text IS committed to the front of the list on that path, as here —
                // and before CONNECT read it: 0x50B150 only reads the list's current string
                // (`[this+0x284]` slot 4), and the original tried it (the `Соединение потеряно.`
                // notice, 127.0.0.1 being local with no session).
                // Which instruction does it is not pinned: the box's release 0x4C2220 commits a
                // *focused* box's text only while its latch `[+0x148]` is 0 (written 0 only by the
                // constructors 0x4CC39A / 0x4CC41C, 1 by the first activate 0x4CCB85 and 0x4CCBE0,
                // never cleared — every `mov [reg+0x148]` read 2026-09-30), which the click path
                // above does not obviously leave at 0 — the commit evidently comes from a path that
                // does not test it (0x4C2120's Return commit, or CONNECT's own 0x50B150 reading the
                // box); the outcome is what is ported, the trigger stays unread.
                let typed = self.fe.layout_preview.as_ref().map(|l| l.input_text("Comms.IP")).unwrap_or_default();
                if let Some(l) = self.net.lobby.as_mut()
                    && !typed.trim().is_empty()
                    && l.history.get(l.ip_sel).map(|t| t.as_str()) != Some(typed.trim())
                {
                    l.commit_ip(&typed);
                    self.bind_lists();
                }
                match self.net.lobby.as_ref().map(|l| l.connect_target()) {
                    Some(Some(a)) => self.net.lobby.as_ref().map(|l| l.connect(a)),
                    Some(None) => {
                        self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 816, [0, 0], DialogFor::Notice);
                        return;
                    }
                    None => None,
                }
            }
            // `DIAL` (0x50ADC0): `[this+0x294] = 1`; the number `Comms.PhoneNumber`'s text (slot 4) equal to the
            // shared empty string 0x5E8E50 → `0x513440(0x337)` = 823 and stop. Otherwise the name and the
            // modem go to `0x48E1A0(net, 2, ModemList index, number)`: **no transport in this port** (the button
            // is disabled while `Comms.ModemList` is empty, `net_browse_step`, which it always is here)
            "DIAL" => {
                let number = self.fe.layout_preview.as_ref().map(|l| l.input_text("Comms.PhoneNumber")).unwrap_or_default();
                if number.is_empty() {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 823, [0, 0], DialogFor::Notice);
                } else {
                    tracing::info!(number, "DIAL (0x50ADC0): the modem transport (0x48E1A0 kind 2) has no equivalent in this port");
                }
                return;
            }
            // `EJECTPLAYER` is message 0xC6, which only a host can mean — and only once a session
            // is actually running, since the lobby's socket has no peers of its own
            "EJECTPLAYER" => {
                // 0x509110: the `Comms.PlayerList` selection's row id (0x4BBB40) goes to 0x48D8A0,
                // and the selection is cleared (slot 16)
                let id = self.net.session.as_ref().and_then(|n| {
                    let empty = String::new();
                    n.player_sel.and_then(|r| n.players(&empty).get(r).map(|p| p.1))
                });
                if let (Some(id), Some(n)) = (id, self.net.session.as_mut()) {
                    let sent = n.eject(id);
                    n.player_sel = None;
                    tracing::info!(id, sent, "eject (0x509110)");
                    self.bind_lists();
                }
                return;
            }
            // 0x50A300 (`netlobby`)
            "READY" => {
                self.net_ready_callback();
                return;
            }
            _ => {
                // ENTER on the server list (`serverlist.lay`): the original connects to the lobby
                // service (§1), which is not reproduced. **APPROXIMATION** (direction: what the
                // list is *for*): the port's server list shows the LAN sessions, so ENTER joins
                // the row under the cursor, as `JOIN` does on the games screen.
                let joined = self.net.lobby.as_ref().and_then(|l| l.join_target().map(|a| l.connect(a)));
                if let Some(r) = joined {
                    self.open_network_session("ENTER", r);
                }
                return;
            }
        };
        if let Some(r) = opened {
            self.open_network_session(cb, r);
        }
    }

    /// A session opened by `CREATE` / `JOIN` / `CONNECT`: keep it and open the lobby screen.
    pub(crate) fn open_network_session(&mut self, cb: &str, r: anyhow::Result<netplay::NetPlay>) {
        match r {
            Ok(mut n) => {
                let host = n.is_host();
                tracing::info!(cb, host, "network session open");
                n.positional = self.net.lobby.as_ref().is_none_or(|l| l.positional);
                self.net.session = Some(n);
                self.net.join_notified = false;
                self.net.lobby_sent = false;
                self.net.lobby_car = None;
                // one seat per player the session can hold, minus ourselves
                if self.num_opponents == 0 {
                    self.num_opponents = 1;
                }
                // **CONNECT** (0x50B150) does not leave the screen: it sets `[this+0x288]` and the peer-to-peer
                // screen's tick 0x50B270 opens `joingame` once the link is up (`net_browse_step`); 817 «Connecting…»
                // (0x509550, state 3) stays up meanwhile
                if cb == "CONNECT" {
                    self.net.connect_pending = true;
                    self.net.connect_opened = false;
                    return;
                }
                // 0x50A0DB / 0x50AB31: the lobby screen, not the race — `0x4D2100(name, 0, 1)`
                self.fe_open(if host { "creategame" } else { "joingame" }, 0, true);
            }
            Err(e) => tracing::warn!(cb, error = %e, "cannot open the network session"),
        }
    }

    /// `Comms.IPList`'s selection moves to `row`; the string box beside it takes the row's text
    /// (0x4C2310: while the box is not focused and its string differs from the list's, it is
    /// cleared and set to the list's).
    pub(crate) fn pick_ip_row(&mut self, row: usize) {
        let Some(text) = self.net.lobby.as_mut().and_then(|l| l.pick_ip(row)) else { return };
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.clear_input("Comms.IP");
            l.type_into("Comms.IP", &text);
        }
        self.bind_lists();
    }

    /// The wait of 0x48E9C0 (see [`Self::fe_session_leave`]): what the timer callbacks would do meanwhile —
    /// [`netplay::NetPlay::poll`] is the port's drain of the socket, the room's 32 Hz loop and the queue's
    /// pump — until the deadline, then the session is freed (its socket closes).
    pub(crate) fn net_leaving_step(&mut self) {
        if self.net.leaving.is_empty() {
            return;
        }
        let now = Instant::now();
        for (n, _) in self.net.leaving.iter_mut() {
            let _ = n.poll(0);
        }
        let before = self.net.leaving.len();
        self.net.leaving.retain(|(_, until)| *until > now);
        if self.net.leaving.len() != before {
            tracing::info!(target: "netplay", "the leave's wait is over (0x48EA2B): the links are freed");
        }
    }

    /// **The network race's end pages** — what `RaceFrame` 0x4152E0 does at its loop's tail for `FEGAME_TYPE 3`
    /// (0x415CE8…0x415D17) and the summary's own update:
    ///
    /// * once the local car's rank `0x48D7F0([0x65741C])` is above 0 — its finish time has come back through
    ///   the room (`0xB7`) — and the pages are not built (`arg_13h`), **0x426A80(1)** builds them (0x415D08):
    ///   `[0x60CA6C] = 0x48D7F0(local)` (0x426B50: the place is the room's, not `car+0xCD4`), the pages
    ///   ([`Self::open_result_pages`]: `HCongrats`' speech and jingle, the records copy) — the summary with
    ///   no timeout and no records page for type 3 ([`Self::result_pages`], [`Self::page_timeout`]);
    /// * **`HRaceSummary`'s update 0x42C080** (and its set-up 0x42C110): for every racer `k` whose rank
    ///   `0x48D7F0(k)` is above 0 and whose row (`[this+0x128 + 4k]`) is not yet filled, its row goes in at row
    ///   `rank − 1` with the time `0x48D770(k)` and **best lap 0** (`0x42C210(k, time, 0, rank − 1)`) — the best
    ///   lap `0xB7` carries goes no further than the room's log; the local player's row is the highlighted one
    ///   (`0x48D7F0(local)`, 0x42C1B5).
    ///
    /// A racer is a *race index* (`player+0x4C`): the player's own, or a seat's (`NetPlay::seat_race_index`).
    /// `RacerResult::state` (`car+0xCD0`) is 2 for a ranked racer and 0 for the others and **nothing reads it**
    /// (settled, AC 2026-09-30): 0x42C110's type-3 arm takes the rank from 0x48D7F0, the time from 0x48D770 and
    /// a best lap of 0 (0x42C153…0x42C15C, `xor` at 0x42C15E), the row filler 0x42C210 takes them as its
    /// arguments, and the state byte is not among the type-3 reads (the single-player arm reads `+0xCD4`,
    /// `+0xCD8`, `+0xCFC`).
    pub(crate) fn net_results_check(&mut self) {
        let Some(n) = self.net.session.as_mut().filter(|n| n.in_race != 0) else { return };
        let Some(local) = n.local_race_index() else { return };
        let drivers = 1 + self.road.opponents.len();
        let mut racers = vec![nfs_game::race::RacerResult::default(); drivers];
        for (d, r) in racers.iter_mut().enumerate() {
            let index = if d == 0 { Some(local) } else { n.seat_race_index(d - 1) };
            if let Some(i) = index {
                let rank = n.race_rank(i);
                if rank > 0 {
                    r.place = rank as u32;
                    r.time = n.race_time(i).max(0) as i32;
                    r.state = 2;
                }
            }
        }
        let order_of = |racers: &[nfs_game::race::RacerResult]| -> Vec<usize> {
            let mut o: Vec<usize> = (0..racers.len()).collect();
            o.sort_by_key(|&d| (racers[d].place == 0, racers[d].place, d));
            o
        };
        match self.results.as_mut() {
            None => {
                // 0x415D00: `jle` — no rank for the local car yet
                if racers[0].place == 0 {
                    return;
                }
                let order = order_of(&racers);
                tracing::info!(target: "netplay", places = ?racers.iter().map(|r| r.place).collect::<Vec<_>>(), times = ?racers.iter().map(|r| r.time).collect::<Vec<_>>(), "network race: the local car has a rank — 0x426A80(1) builds the pages (0x415D08)");
                self.results = Some(RaceResults { racers, order, clean: [true; 8], mission: None, opened: self.tick, congrats: true });
                self.results_page = 0;
                self.page_seen = None;
                self.page_open_tick = None;
                self.page_closing = None;
                self.open_result_pages();
            }
            Some(res) => {
                // 0x42C080: rows fill as the ranks come in
                let mut changed = false;
                for (row, new) in res.racers.iter_mut().zip(racers) {
                    if row.place == 0 && new.place > 0 {
                        *row = new;
                        changed = true;
                    }
                }
                if changed {
                    res.order = order_of(&res.racers);
                }
            }
        }
    }
}

/// Network play, split out of [`App`] on 2026-09-21: the running session and the front end's
/// **`HInfo`'s lag arm** (0x427C7E…0x427EDF, `net_race_notice`): what one look at `lag = 2·net+0xF0 −
/// [0x6559F8] + 1` does. `Lost`: under −0x100 with the link up (0x427CD3: `cmp ecx, 0xffffff00; jge`, then
/// the connection test again) — 18182 and the teardown; else under −0x20 (0x427E1A): with no box up
/// (`[0x60CA08]`) `Show` — 18193 and the tick ends; with it up `Hold` (0x427EC2 → 0x427DAA: nothing else
/// runs); at or above −0x20 with the box up (0x427EBA → 0x427EC2): above −0x10 `Close` (vtable `+0x14`, 2,
/// the flag cleared) and on to the states, else `Hold`; with no box up `States` (0x427DB7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LagStep {
    Lost,
    Show,
    Hold,
    Close,
    States,
}

pub(crate) fn lag_step(lag: i32, connected: bool, box_up: bool) -> LagStep {
    if lag < -0x100 && connected {
        return LagStep::Lost;
    }
    if lag < -0x20 {
        return if box_up { LagStep::Hold } else { LagStep::Show };
    }
    match (box_up, lag > -0x10) {
        (false, _) => LagStep::States,
        (true, true) => LagStep::Close,
        (true, false) => LagStep::Hold,
    }
}

/// The controller samples an input-exchange race's frame driver latched for one tick (0x415EF0 →
/// 0x413870, then `Car_applyInput` 0x4991E0 reads them per car): as the cars read them, the steering
/// negated under `MIRROR` (0x4992D6).
pub(crate) struct ExchangeIn {
    /// this machine's own car (`[0x65741C]`)
    pub(crate) local: Controls,
    /// its race index
    pub(crate) local_index: u32,
    /// every network seat: `(seat, race index, sample)`
    pub(crate) seats: Vec<(usize, u32, Controls)>,
    /// the latched sample's horn bit (`b0` bit 1 = `car+0xD86`, 0x4991E0) of this machine's car and of
    /// each entry of `seats`, in the same order
    pub(crate) local_horn: bool,
    seat_horns: Vec<bool>,
    /// the samples as the ring holds them (`[b0, b1, b2, b3]`): what the replay records per human
    pub(crate) local_raw: nfs_net::exchange::Sample,
    pub(crate) seat_raw: Vec<nfs_net::exchange::Sample>,
}

/// What the simulation reads of a human car in an input-exchange race, taken the same way for the local car
/// and for a seat (`network.md` §11.18).
pub(crate) struct ExchHuman {
    #[allow(dead_code)]
    pub(crate) car: HumanCar,
    pub(crate) position: Vec3,
    pub(crate) slice: usize,
    pub(crate) laps: u32,
    #[allow(dead_code)]
    pub(crate) progress: f32,
    /// `car+0xEC0`: the speed along the car's own heading
    pub(crate) vl: f32,
    /// `car+0xEB0`
    pub(crate) dir: f32,
    pub(crate) speed: f32,
    pub(crate) hw: f32,
    pub(crate) hl: f32,
    /// `car+0xD86`
    pub(crate) horn: bool,
}

/// 0x417800's `car+0xEB0`: −1 below −3 m/s, +1 from 0 up, otherwise the heading's sign against the slice.
pub(crate) fn exch_dir(t: &SliceTable, vl: f32, fwd: Vec3, s: usize) -> f32 {
    if vl <= -3.0 {
        -1.0
    } else if vl >= 0.0 {
        1.0
    } else {
        fwd.dot(t.slices[s].forward).signum()
    }
}

/// A network race played back (`replay.md` §18.15): the input-exchange race without the network. The
/// stream carries one lane per human car (the original's `0x606880 + 0x20·p`, `p` the race index); the
/// simulation reads each car's packet from its lane where a live race takes it from the room's frames.
pub(crate) struct ReplayNetRun {
    /// the local car's race index (`[0x65741C]`, the car the setup's car / paint / name are)
    pub(crate) local_index: u32,
    /// each seat's race index, in seat order (`u32::MAX`: a seat the launch gave no index)
    pub(crate) seats: Vec<u32>,
    /// the room's names of the seats
    pub(crate) names: Vec<String>,
}

/// A human car of an input-exchange race, by where it lives on this machine: the local player's
/// car (`self.vehicle`) or a network seat (`self.road.opponents[k]`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum HumanCar {
    Local,
    Seat(usize),
}

impl ExchangeIn {
    /// The human cars in **registration order** (`Car_register` 0x40F189 fills `0x5E8B34[]` /
    /// `0x5E8E2C[]` in the order `Race_setCars` 0x4ECF00 creates the cars, i.e. by race index, the
    /// same on every machine — `network.md` §11.18). The port keeps the local car first and the
    /// seats after it, which is a different order on each machine; every list the simulation walks
    /// is built from this instead. A seat the launch left without a race index (parked) follows the
    /// humans.
    pub(crate) fn humans(&self, n_seats: usize) -> Vec<HumanCar> {
        let mut order: Vec<(u32, HumanCar)> = vec![(self.local_index, HumanCar::Local)];
        order.extend(self.seats.iter().map(|e| (e.1, HumanCar::Seat(e.0))));
        order.sort_by_key(|e| e.0);
        let mut order: Vec<HumanCar> = order.into_iter().map(|e| e.1).collect();
        for k in 0..n_seats {
            if !order.contains(&HumanCar::Seat(k)) {
                order.push(HumanCar::Seat(k));
            }
        }
        order
    }

    /// The horn of a human car (`car+0xD86`).
    pub(crate) fn horn(&self, h: HumanCar) -> bool {
        match h {
            HumanCar::Local => self.local_horn,
            HumanCar::Seat(k) => self.seats.iter().position(|e| e.0 == k).and_then(|i| self.seat_horns.get(i).copied()).unwrap_or(false),
        }
    }
}

/// lobby. `self.net` became `self.net.session`; `self.lobby` became `self.net.lobby`.
pub(crate) struct Network {
    /// `--net host[:PORT]` / `--net join:HOST[:PORT]` — network play (`netplay`, `network.md` §7).
    pub(crate) session: Option<netplay::NetPlay>,
    /// Sessions in the wait of 0x48E9C0 (0x48EA2B, `0x53C380([0x5DEB48] / 2)`): a host's teardown has sent
    /// its leave and pumps for half a second before the links are freed — the port lets the front end go on
    /// and keeps draining them until the deadline ([`App::net_leaving_step`]).
    pub(crate) leaving: Vec<(netplay::NetPlay, Instant)>,
    /// The front end's side of it, alive while one of the four network layouts is open.
    pub(crate) lobby: Option<netplay::Lobby>,
    /// the host's `0xB3` came in on a lobby screen: launch the race once the frame is drawn
    pub(crate) start_due: bool,
    /// the join outcome has been put in front of the player already (one notice per session)
    pub(crate) join_notified: bool,
    /// the lobby screen has sent our slot records (`[this+0x285]`, 0x509290 / 0x50A4F0)
    pub(crate) lobby_sent: bool,
    /// the car (`id`, name) those records last carried, to see a new selection
    pub(crate) lobby_car: Option<(i32, Vec<u8>)>,
    /// the car list's setter 0x4BC240 ran since the last tick (a step of `zMergedList.Car`, changed
    /// or not): the accessor sends its records on every call, 0x4B7F70 holding
    pub(crate) lobby_setsel: bool,
    /// what the launch stream settled for the race about to start (`network.md` §11.7)
    pub(crate) race: Option<NetRace>,
    /// `PeerToPeerIP`'s `[this+0x288]` — set by CONNECT (0x50B150, its first instruction): the screen's tick
    /// opens `joingame` once the link is up (0x50B270)
    pub(crate) connect_pending: bool,
    /// `[this+0x289]`: that `joingame` has been opened (once)
    pub(crate) connect_opened: bool,
}

/// The race a network launch settled - the host's own choices, or what a player heard in the
/// host's `0xBC` stream - resolved the way the launch reads the option table (`network.md` §11.7).
#[derive(Debug, Clone)]
pub(crate) struct NetRace {
    pub(crate) track: Option<String>,
    pub(crate) laps: u32,
    pub(crate) backwards: bool,
    pub(crate) mirror: bool,
    pub(crate) damage: bool,
    pub(crate) skill: u8,
    /// traffic cars after 0x4ECF00's clamp to nine cars (`NUMCARS - 8`)
    pub(crate) traffic_cars: u32,
    pub(crate) positional: bool,
    nocollision: bool,
    /// `DERBY`: the stream said the race is a Smash Up (0x416407's knockout arm runs)
    pub(crate) derby: bool,
    /// `RALLY` `[0x6573F8]` / `HEAVY` `[0x6573FC]`: the host's cheats 0 and 8 as the stream carried them
    /// (`network.md` §11.20) — every machine's cars run on these, whatever cheat bits it has of its own
    pub(crate) rally: bool,
    pub(crate) heavy: bool,
    /// the players in the race (`ebx` of 0x4B4DA0: the players `net+0xE5` marks)
    pub(crate) players: u32,
    /// `NUMOPPONENTRACECARS` `[0x658088]` as the stream settled it: 0 for a network launch (0x4ECF00;
    /// the stream's `(0x28, 0)`, `options.rs`); the fix-up 0x4B5081 numbers that many opponent slots
    /// after the players, and the port has no AI opponent in a network race
    pub(crate) opponents: u32,
    /// `.STARTINGPOS` of this machine's own race index, as the host drew it, as the port's
    /// `grid_pos` (`g − 1`)
    pub(crate) grid: Option<u32>,
    /// the same for each other player, in room order — the seat's parking place until its
    /// packets arrive
    seat_grids: Vec<Option<u32>>,
    pub(crate) playlist: Option<Vec<u32>>,
    /// `fe.txt`'s `RAND_SEED` / `TIME` / `WEATHER` / `FOG` as the host's stream carried them
    fe_txt: Vec<(&'static str, u32)>,
    /// the traffic cars' per-slot records, in slot order (the host's own for the host)
    pub(crate) traffic: Vec<nfs_net::options::TrafficCar>,
    /// each other player's per-slot car record at its race index, in room order (the seat order)
    /// — what 0x4EEDA0 built on the owner's machine (`network.md` §11.12)
    seat_cars: Vec<nfs_net::launchcar::CarSlot>,
    /// `.CAMERA` 0x148 of **race index 0** — `car[0].camera`, the only list this machine's camera
    /// reads (`network.md` §11.12c): the first player of the race's, whoever that is
    camera0: Option<Vec<u32>>,
    /// this machine's own car record at its race index (`launch.cars[me]`), for the replay's `.rpl`
    own_words: Vec<(u32, Vec<u32>)>,
}

/// The layouts that speak to the network. Their `callbacklist=` ids are `ID_COMMSSERVERSCREEN`,
/// `ID_COMMSWEBGAMESSCREEN`, `ID_PEERTOPEERIP` and the modem one — see `netplay::Lobby`.
/// Matched case-insensitively, because the screens name each other in lower case
/// (`destination=peertopeerip`) while the files are mixed case.
const NETWORK_LAYOUTS: [&str; 7] = [
    "serverlist",
    "udpip",
    "local",
    "peertopeerip",
    "modem",
    "creategame",
    "joingame",
];

pub(crate) fn is_network_layout(name: &str) -> bool {
    NETWORK_LAYOUTS.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// **TEST GLUE** (`NFS_REMOTE_DUMP=<file>`): one CSV line per tick for every remote seat after the dead
/// reckoning — tick, seat, position, speed estimate, the last block's tick, the lag book's `+0x68` /
/// `+0x5C` and the stale flag — what the smooth-motion proof reads (`network.md` 8.4).
pub(crate) fn remote_dump(tick: u64, seat: usize, v: &Vehicle, n: &netplay::NetPlay) {
    use std::io::Write;
    static OUT: std::sync::OnceLock<Option<std::sync::Mutex<std::io::BufWriter<std::fs::File>>>> = std::sync::OnceLock::new();
    let out = OUT.get_or_init(|| {
        let path = nfs_game::config::cfg().nfs_remote_dump.var_os()?;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).ok()?);
        let _ = writeln!(f, "tick,seat,x,y,z,speed_est,block_tick,lag,lag_avg,stale");
        Some(std::sync::Mutex::new(f))
    });
    let (Some(out), Some(r)) = (out.as_ref(), n.remotes.values().find(|r| r.seat == seat)) else { return };
    if let Ok(mut f) = out.lock() {
        let _ = writeln!(f, "{tick},{seat},{:.4},{:.4},{:.4},{:.3},{},{},{},{}", v.position.x, v.position.y, v.position.z, v.speed_est, r.state.tick, r.lag, r.lag_avg, r.stale as u8);
        let _ = f.flush();
    }
}

#[cfg(test)]
mod net_page_tests {
    use super::*;

    /// 0x4272BF / 0x4272C9: game type 3 clears the summary's timeout (`0x41B130(0)`) — it stays until Space;
    /// every other type passes 1 and the page closes after 0x280 ticks. The other pages are unchanged.
    #[test]
    fn a_network_summary_has_no_timeout() {
        assert_eq!(App::page_timeout(ResultPage::Summary, true), None);
        assert_eq!(App::page_timeout(ResultPage::Summary, false), Some(0x280));
        assert_eq!(App::page_timeout(ResultPage::Records, true), Some(0x280));
        assert_eq!(App::page_timeout(ResultPage::TournamentPlace(1), true), Some(0xC0));
    }
}
