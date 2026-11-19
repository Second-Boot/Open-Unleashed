//! The lobby screens' network callbacks, the session open, the leaving drain and the results check.

use crate::*;

impl App {
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
    pub(crate) fn network_callback(&mut self, cb: &str, _event_loop: &LoopCtl) {
        // the internet lobby (`lobby.md`): the link is the session, the lists its own
        if self.internet_session() {
            match cb {
                "ENTER" => {
                    self.lobby_enter_server();
                    return;
                }
                "JOIN" => {
                    self.lobby_join();
                    return;
                }
                _ => {}
            }
        }
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
                // service (§1), which is not reproduced. **PORT-DEVIATION D34** (direction: what the
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
                self.hud_state.pages.page = 0;
                self.hud_state.pages.seen = None;
                self.hud_state.pages.open_tick = None;
                self.hud_state.pages.closing = None;
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
