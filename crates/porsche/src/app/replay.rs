//! Replays: recording, saving, playback and the replay bar.

use crate::*;

impl App {
    /// The replay's per-tick housekeeping: save the recording when the race ends the way
    /// `Replay_saveDefault` 0x413FD0 does from the de-init chain.
    pub(crate) fn replay_tick(&mut self) {
        use nfs_game::replay::Mode;
        let Some(r) = self.replay.stream.as_ref() else {
            return;
        };
        if matches!(r.mode, Mode::Recording | Mode::Full) {
            let done = self.race.as_ref().map(|r| r.finished).unwrap_or(false) || self.player_eliminated;
            if done && !self.replay.saved {
                self.save_replay();
            }
        }
    }

    /// **The end of a playback** — `RaceFrame` 0x4152E0 tests it **once per frame, after the
    /// catch-up loop** (0x415CB3), so the frame's remaining ticks still run (and draw); the callers
    /// make it the frame's last step. Two ways end a playback's race loop (`[0x606A8C] = 1`,
    /// tested at 0x415D17):
    ///
    /// * the stream has run out (`[0x5E99F4] == 3`, 0x4144E7) and `gGameTick [0x606AC4] > 0x40`
    ///   (0x415CBB `jle 0x415CE8`): `[0x606A8C] = 1`, `REPLAY [0x657424] = 3` (0x415CD8…0x415CE2);
    /// * race over, 0x417010 — it has **no** replay test (0x417010…0x41714F): a playback whose
    ///   humans are all past `+0xB14 + 0x140` (0x280 while the announcer talks) before the
    ///   stream runs out ends there, `REPLAY` still 2 (`Playback::race_over`).
    ///
    /// Either way the session loop takes its exit: `REPLAY > 1` and `[0x657428] == 0`
    /// (0x4B6ED5…0x4B6EE6) → 0x4B6F1D, and with `[0x606A88]` clear (0x4B6F29) the sim shutdown
    /// 0x4B68F0 (0x4B6F2F), which saves the replay (0x413FD0 at 0x4B69F1). Then the front end
    /// comes back while `FE [0x65B298]` is on (0x4B6FAF → 0x4B6C0C, 0x4D3420): a `STARTREPLAY`
    /// playback (`[0x699FC9]`, 0x50C120 at 0x4D3EB2) to `gamesetup` + `loadreplay` with no race
    /// end 0x4EDD00, an automatic replay through the race end. **With the front end off** (the
    /// port's `--replay` and every race started from the command line) main leaves its loop at
    /// 0x4B6FB5 and the program ends (0x4B6FC0…0x4B6FE5, `return 0`). *(Until 2026-09-27 the port
    /// dropped the stream there and drove on as a live race.)* `true` when the program is ending.
    pub(crate) fn replay_exit_check(&mut self, event_loop: &ActiveEventLoop) -> bool {
        use nfs_game::replay::Mode;
        let Some(r) = self.replay.stream.as_ref() else {
            return false;
        };
        let playback = matches!(r.mode, Mode::Playing | Mode::Exhausted);
        let exhausted = nfs_game::replay::playback_over(r.mode, self.tick);
        if !(playback && (exhausted || self.replay.race_over)) {
            return false;
        }
        tracing::info!(tick = self.tick, exhausted, race_over = self.replay.race_over, automatic = self.results.is_some(), "replay finished (0x415CB3 / 0x417010): the race loop is left");
        // after a race's automatic replay that is the end of a race run to its end, filed from
        // the records; a replay started on its own has none and goes back as `STARTREPLAY`'s
        let left = if self.results.is_some() { self.end_finished_race() } else { self.leave_race() };
        if left {
            return false;
        }
        // no front end: 0x4B68F0 (the replay saved at 0x4B69F1), then out of main
        self.save_replay();
        self.stop_race_audio();
        self.sim_shutdown();
        tracing::info!("no front end (FE [0x65B298] = 0): the program ends (0x4B6FAF → 0x4B6FC0)");
        event_loop.exit();
        true
    }

    /// Write the port's replay file (`replay.md` §6.2 saves `replay.rpl` after every race; §14.5
    /// explains why this is the port's own format and not the original's 118 312-byte dump).
    /// `Replay_saveDefault` 0x413FD0 has no condition at all: an empty recording and a playback
    /// are written too, the latter as the stream it played. *(Until the audit of 2026-09-26 the
    /// port skipped the save when nothing had been written, which also skipped every playback.)*
    pub(crate) fn save_replay(&mut self) {
        self.replay.saved = true;
        let Some(r) = self.replay.stream.as_mut() else {
            return;
        };
        if r.mode == nfs_game::replay::Mode::Recording {
            r.final_tick = self.tick;
        }
        let path = self.replay.path.clone();
        self.write_replay(&path);
    }

    /// The stream as the port's own file at `path` and the original's `.rpl` beside it.
    fn write_replay(&self, path: &std::path::Path) {
        let Some(r) = self.replay.stream.as_ref() else {
            return;
        };
        let file = nfs_game::replay::ReplayFile {
            setup: self.replay.setup.clone(),
            final_tick: r.final_tick,
            stream: r.stream().to_vec(),
        };
        let bytes = file.write();
        let res = std::fs::create_dir_all(path.parent().unwrap_or(path))
            .and_then(|_| std::fs::write(path, bytes));
        // …and the original's own format beside it (`replay.md` §6.1 + §16): the registry snapshot
        // with the fields the port has values for, the final tick and the same stream
        // `NUMCARS` = cops + players + opponents + traffic (0x4ECF00), the traffic cut to fit nine
        // as the race set it up (0x4ED382…0x4ED399)
        let cops = if self.road.cops_enabled { self.road.cop_count } else { 0 };
        let traffic = self.road.traffic_count.min(9u32.saturating_sub(1 + self.num_opponents + cops));
        let num_cars = cops + 1 + self.num_opponents + traffic;
        // a network race's (`NetSetup` in the setup): the humans and the host's traffic cars kept
        let num_cars = if let Some(n) = file.setup.net.as_ref() { 1 + n.seats.len() as u32 + n.traffic.len() as u32 } else { num_cars };
        let rpl = nfs_game::replay::original_rpl(&file.setup, file.final_tick, &file.stream, r.ramp, num_cars);
        if let Err(e) = std::fs::write(path.with_extension("rpl"), rpl) {
            tracing::debug!(error = %e, "writing the original-format .rpl failed");
        }
        match res {
            Ok(()) => {
                tracing::info!(path = %path.display(), ticks = file.final_tick, bytes = r.written(), "replay saved")
            }
            Err(e) => tracing::warn!(error = %e, "replay save failed"),
        }
    }

    /// `RebuildReplayList` — `%s*.rpl` from the replay directory (0x4E5E1B, and again at
    /// 0x50D45A after a delete), by name.
    pub(crate) fn replay_names(&self) -> Vec<String> {
        let Some(dir) = self.replay.path.parent() else { return Vec::new() };
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("rpl")))
            .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
            .collect();
        names.sort();
        names
    }

    /// **`STARTREPLAY`** (slot 51, 0x50D390): `[0x699FC9] = 1` — "this is a replay playback" —
    /// then `PLAYGAME`. The launch (0x4D5372…0x4D5425) sends option 0x97 = 2 and 0x110 =
    /// `"%s.rpl"` over `Replay.List`'s current name, and the start check 0x4D5730 skips the
    /// game-type switch and the car checks for it (0x4D5786). Here: that file read, the stream set
    /// playing, the race set up from its snapshot, and the race started.
    pub(crate) fn start_replay(&mut self, event_loop: &ActiveEventLoop) {
        let names = self.replay_names();
        let Some(name) = names.get(self.fe.replay_sel.min(names.len().saturating_sub(1))).cloned() else { return };
        let Some(dir) = self.replay.path.parent().map(|d| d.to_path_buf()) else { return };
        let path = dir.join(format!("{name}.rpl"));
        // the port's own file beside the listed `.rpl` is read first (`replay.md` §17): it names the car's
        // body style and carries a network race's humans (§18.15), which the single-car `.rpl` cannot
        let Some(f) = std::fs::read(path.with_extension("prpl"))
            .ok()
            .and_then(|d| nfs_game::replay::ReplayFile::parse(&d))
            .or_else(|| std::fs::read(&path).ok().and_then(|d| read_replay_file(&d)))
        else {
            tracing::warn!(path = %path.display(), "not a replay file");
            return;
        };
        let mut setup = f.setup.clone();
        resolve_replay_car(&self.paths, &mut setup);
        let (track, car) = match (TrackAssets::load(&self.paths, &setup.track), CarAssets::load(&self.paths, &setup.car), CarAssets::load_sim(&self.paths, if setup.sim.is_empty() { "default" } else { &setup.sim })) {
            (Ok(t), Ok(c), Ok(sim)) => (t, (c, sim)),
            (t, c, s) => {
                tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "replay load failed");
                return;
            }
        };
        tracing::info!(path = %path.display(), track = setup.track, car = setup.car, "front end: start replay");
        let race_class = setup.sim.to_lowercase().starts_with("gt") || setup.sim.starts_with("935");
        self.specs = CarSpecs::from_sim(&car.1, race_class);
        // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
        if self.cheat_bits & (1 << 1) != 0 {
            self.specs.apply_tinycar();
        }
        // 0x49CB50 folds the car's stock parts (0x4EE9A0) and the neutral sliders (0x4EED2D) — the
        // replay file carries neither a part list nor sliders of its own here, so the car is a fresh
        // catalogue car (`fold_fresh_car`)
        let entry = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car"))
            .ok()
            .and_then(|d| nfs_formats::fedata::parse_car_catalog(&d).ok())
            .and_then(|cat| cat.into_iter().find(|c| (setup.car_id != 0 && c.id == setup.car_id) || (!setup.sim.is_empty() && c.sim.eq_ignore_ascii_case(&setup.sim))));
        if let Some(e) = &entry {
            let table = self.fe_parts_table();
            let automatic = self.player_automatic();
            nfs_game::career::fold_fresh_car(&mut self.specs, &e.stock_parts, &table, &e.sim_family, automatic);
        }
        self.track = track;
        // the catalogue record's body style and roof, as the live race's car had them
        self.car = match &entry {
            Some(e) => car.0.with_style(e.variant).with_cabrio(e.cabrio),
            None => car.0,
        };
        self.sim_name = setup.sim.clone();
        self.num_opponents = setup.opponents;
        self.num_laps = setup.laps.max(1);
        self.backwards = setup.backwards;
        self.mirror = setup.mirror;
        self.owned_paint = None;
        self.owned_decal = None;
        self.paint = setup.paint;
        self.road.traffic_count = TRAFFIC_COUNT[(setup.traffic as usize).min(3)];
        self.road.traffic_ids = None;
        self.road.cops_enabled = setup.cops;
        self.road.damage_on = setup.damage;
        self.road.cop_count = 4;
        self.road.cops_race = false;
        // `SKILL` travels in the block as well (0x413DCF)
        self.race_skill = setup.skill;
        self.traffic_seed = setup.seed;
        self.career_opponents = None;
        self.replay.stream = Some(nfs_game::replay::Replay::playing(f.stream, f.final_tick));
        self.replay.setup = setup;
        self.apply_replay_net();
        // the menu comes back when the playback ends (`leave_race`)
        self.fe.menu_template = self.fe.menu.take();
        self.fe.after_replay = true;
        self.fe.race_game_type = Some(self.fe.game_type);
        self.fe.layout_preview = None;
        self.fe.layout_name = None;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return };
        self.tick = 0;
        self.start_race(renderer, window, event_loop);
    }

    /// **0x50D3C0**, the "yes" to 808: `"%s%s.rpl"` over the replay directory and `Replay.List`'s
    /// current name (0x50D402), removed when it exists (0x50D412…0x50D425), then the list rebuilt.
    /// The directory is this port's own save directory, never the game's.
    pub(crate) fn kill_selected_replay(&mut self) {
        let names = self.replay_names();
        let Some(name) = names.get(self.fe.replay_sel.min(names.len().saturating_sub(1))) else { return };
        if let Some(dir) = self.replay.path.parent() {
            let path = dir.join(format!("{name}.rpl"));
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!(path = %path.display(), "replay deleted"),
                Err(e) => tracing::warn!(path = %path.display(), error = %e, "replay delete failed"),
            }
        }
        let n = self.replay_names().len();
        self.fe.replay_sel = self.fe.replay_sel.min(n.saturating_sub(1));
        self.bind_lists();
    }

    /// **The button going down over the replay bar** — `WM_LBUTTONDOWN`, 0x42D8EC's event 2. The
    /// original never answers it here: the message pump only drops it into the single mailbox
    /// `[0x610A6C]` (0x435180); `RaceFrame`'s own pump (0x425EF0, called once per frame at
    /// 0x415520) is its only reader, at 0x4264B5…0x4264CC — so a press taken between two pump
    /// calls waits for the next one. *(Until 2026-09-27 the port answered the press right here, on
    /// the button's down event, up to one frame before the original would — `replay.md` §18.11's
    /// gap.)* This only latches the position (a hit test against the seven cells, no state
    /// change — `bar_event` steps `BarState` and is left for [`App::replay_bar_apply_press`], the
    /// pump's own call); a second press before the first is consumed overwrites it, as the one
    /// mailbox slot does.
    pub(crate) fn replay_bar_latch(&mut self) -> bool {
        use nfs_game::replay::bar_cell_at;
        // `REPLAY == 2` (0x426463): through the exhausted frames too
        if !self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)) || self.hud.is_none() {
            return false;
        }
        let Some((cx, cy)) = self.fe.cursor else { return false };
        let (cells, _) = Hud::bar_geometry();
        if bar_cell_at(&cells, cx.floor() as i32, cy.floor() as i32).is_none() {
            return false;
        }
        self.bar_press = Some((cx, cy));
        true
    }

    /// **A press on the replay bar, answered** — `HReplay`'s slot 9 0x42D880, called from
    /// [`App::replay_bar_frame`] with the mailbox's latched position (or, for the scripted
    /// diagnostics that call it directly, the pointer put there for one script step). While
    /// `REPLAY == 2` (0x426463…0x4264CC) whenever the dialog on top answered nothing, which a press
    /// always is (the dialogs act on the release). The answer ([`nfs_game::replay::BarEvent`]) goes
    /// through the pump (7, 0xE, 0xF, 0x4264D1…0x4264E8) and then `RaceFrame`'s action switch
    /// (0x415520 → 0x414850):
    ///
    /// * 6 → command 8, the next camera (0x414901…): stepped here at once, as `RaceFrame` does it
    ///   within the frame, frozen or not;
    /// * 7 → the pump sets `[0x60CA10] = 1` and `MusicPlay(0, 0)` (0x4266BD; a playback has no race
    ///   music, not carried), then command 1: `[0x606868] = 1`, `[0x657428] = 1`, and the session
    ///   loop plays the stream again from the start (0x4B6EF4…) — whose `0x426A80(1)` finds
    ///   `[0x60CA10]` and queues **no** pages, clearing it (0x426C1C, 0x426C6C);
    /// * 8 → command 3, the pause toggle `[0x606860] ^= 1`;
    /// * 9 → command 2, quit: `[0x606868] = 1`, the session loop's exit with `REPLAY` 2 (as the
    ///   playback running out, `App::replay_tick`);
    /// * 0xA…0xD → commands 4…7, the speed;
    /// * 0xE → every dialog closed and popped, then — unless aborted `[0x657E34]`, `[0x60C9B4]` or
    ///   `DERBY` — `0x426C90(1)`: the result pages again, **without** the race's `HCongrats` (its
    ///   argument skips it, 0x426CF2) (0x42664B…0x4266AB);
    /// * 0xF → `HReplaySave` (0x4264EE…0x42663C).
    ///
    /// Not carried: the network variant (`+0x198`). *(The tooltips 0x42DD10 and the auto-hide
    /// `+0x124` are ported since 2026-09-27: `Hud::replay_bar`, `App::replay_bar_frame`.)*
    fn replay_bar_apply_press(&mut self, cx: f32, cy: f32, event_loop: &ActiveEventLoop) -> bool {
        use nfs_game::replay::{bar_cell_at, bar_event, BarEvent};
        if !self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)) || self.hud.is_none() {
            return false;
        }
        let (cells, _) = Hud::bar_geometry();
        let Some(cell) = bar_cell_at(&cells, cx.floor() as i32, cy.floor() as i32) else { return false };
        // `+0x198`, built for `FEGAME_TYPE 3` only (0x424F7B): a network race's replay has the network bar
        let Some(ev) = bar_event(cell, &mut self.replay_bar, self.replay_net.is_some()) else { return false };
        tracing::info!(tick = self.tick, cell, event = ev.code(), "replay bar (0x42D880)");
        match ev {
            BarEvent::Camera => {
                let skidpad = self.track.name.eq_ignore_ascii_case("skidpad");
                self.replay.camera = nfs_game::replay::next_camera(self.replay.camera as usize, skidpad) as u8;
            }
            BarEvent::Restart => {
                // command 1 is the only writer of `[0x657428]` (0x4148A0): the same frame's pause plumbing
                // (0x4157DE → 0x4157F2) runs 0x4AB360, whose `MusicStop(0)` (0x4AB3EF → 0x4AE3C0) stops the
                // song at once — the race's jingle, say — where the replay pass's own `AudioLoadTrack`
                // (`MusicStop(1500)`) would fade it (the network bar keeps this cell, 0x42D3CD)
                tracing::info!(tick = self.tick, playing = self.snd.music.is_some(), "MusicStop(0) (0x4AB3EF, [0x657428] set by the bar's restart)");
                if let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut()) {
                    m.stop(a);
                }
                self.replay_again(event_loop, false)
            }
            // `+0x199` flips with `[0x606860]` unconditionally (0x42DA24…0x42DA34, no `[0x606A90]`
            // test); the actual freeze/resume — 0x4157CE's plumbing, gated on the pause menu being
            // down — is `App::sync_audio_pause`'s edge on `self.paused || self.replay.frozen`
            // (`replay.md` §18.11's second gap: re-checked 2026-09-27, already matched — flipping
            // `frozen` here has no visible effect while `self.paused` already holds the race
            // frozen, and the deferred toggle takes hold exactly when the pause menu closes, since
            // `frozen` itself carries it through)
            BarEvent::Pause => self.replay.frozen = !self.replay.frozen,
            BarEvent::Quit => {
                let left = if self.results.is_some() { self.end_finished_race() } else { self.leave_race() };
                if !left {
                    event_loop.exit();
                }
            }
            BarEvent::Speed(sp) => self.replay.speed = sp,
            BarEvent::Results => {
                self.paused = false;
                self.pause_confirm = None;
                self.replay_save.up = false;
                self.replay_save.overwrite = false;
                // the gates `[0x657E34]` (aborted — such a race is left, never replayed here),
                // `[0x60C9B4]` (not in this port) and `DERBY` (0x4266A0: `[0x657400] == 0`) — the Smash Up
                // cheat closes the dialogs and queues nothing
                if self.cheat_bits & (1 << 7) == 0
                    && let Some(r) = self.results.as_mut()
                {
                    // `0x426C90(1)`: the queue from its start, no `HCongrats`
                    r.congrats = false;
                    self.results_page = 0;
                    self.results_page_tick = self.tick;
                    self.page_seen = None;
                    self.page_open_tick = None;
                    self.page_closing = None;
                }
            }
            BarEvent::Save => self.open_replay_save(),
            // 0x42D8FC…: `[0x606858]` steps (wrapping in `[0, [0x5E8BFC])`) and the camera is applied to
            // the new viewed car — a network replay's humans, by race index
            BarEvent::NextCar | BarEvent::PrevCar => {
                if let Some(rn) = self.replay_net.as_ref() {
                    let n = rn.seats.iter().filter(|i| **i != u32::MAX).count() + 1;
                    self.replay_view = if ev == BarEvent::NextCar { (self.replay_view + 1) % n } else { (self.replay_view + n - 1) % n };
                    tracing::info!(view = self.replay_view, "network replay bar: the viewed car (0x42D8FC)");
                }
            }
        }
        true
    }

    /// **The bar's frame** (`nfs_game::replay::BarState`), once per `RaceFrame` of a playback — the
    /// original's order: the pump's slot-9 call first (0x415520 → 0x4264CC: the mailbox's press,
    /// if any, answered — 0x4264B5…0x4264CC — then `away` and the cells' hover counts,
    /// 0x42D892…0x42DCE2; the pump calls it whenever the top dialog answered 0, which the port's
    /// dialogs always do between presses), then the draw's open / close (0x42D22F…0x42D282, at
    /// `[0x606AC8]`), then the HUD frame's pin (0x425CF5…0x425D22: any dialog queued — the pause
    /// menu or its question, a result page, `HReplaySave` or its overwrite question).
    pub(crate) fn replay_bar_frame(&mut self, event_loop: &ActiveEventLoop) {
        // the mailbox is taken and cleared here whatever the guard below finds — 0x435410 runs
        // unconditionally at the pump's head, before any `REPLAY` test
        if let Some((cx, cy)) = self.bar_press.take() {
            self.replay_bar_apply_press(cx, cy, event_loop);
        }
        if !self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)) || self.hud.is_none() {
            return;
        }
        let (cells, frame) = Hud::bar_geometry();
        let pointer = self.fe.cursor.map(|(x, y)| (x.floor() as i32, y.floor() as i32));
        self.replay_bar.count(&cells, frame, pointer, self.replay_net.is_some());
        let clock = self.hud_clock();
        self.replay_bar.transition(clock);
        let page_up = self.results.is_some() && usize::from(self.results_page) < self.result_pages().len();
        let queued = self.paused || self.replay_save.up || self.replay_save.overwrite || page_up;
        self.replay_bar.pin(queued);
    }

    /// The bar's 0xF, in the pump (0x4264EE…0x42663C): the dialog on top closed (slot 5(0)) and
    /// **every** queued dialog popped — the result pages and the pause menu go — then
    /// `HReplaySave` queued and opened (slot 5(1)), the typed-key queue emptied (0x435060), and,
    /// unless the bar's `+0x199` is already set (0x42E010), `+0x199 = 1` (0x42E020) and event 8
    /// returned — command 3, the replay paused.
    fn open_replay_save(&mut self) {
        self.paused = false;
        self.pause_confirm = None;
        self.results_page = self.result_pages().len() as u8;
        self.replay_save.up = true;
        self.replay_save.overwrite = false;
        self.replay_save.opened = Some(Instant::now());
        if !self.replay.frozen {
            self.replay.frozen = true;
        }
    }

    /// `HReplaySave`'s answer to a click (slot 9, 0x42E310, only while fully open, 0x42E325): the
    /// first button with a name typed (0x430E20) — `"%s%s.rpl"` (0x5CCEC4) over the save
    /// directory `[0x65B360]`, and 0x59DC30: when that file exists, **0x11**; otherwise
    /// `Replay_saveAs("%s.rpl")` 0x414660 and **0x10**; the second button **0x12**. The pump
    /// (0x425FA6, table 0x426718): 0x10 and 0x12 close it (slot 5(2)) and clear the bar's `+0x199`
    /// (0x42E030) with event 8 — the replay goes on (0x4263BB / 0x426446…0x42645E); 0x11 closes it
    /// and raises `HYesNoOverlay` with 18139 / 18140, `[0x60CA9D] = 1` (0x4263D1…0x42643D).
    pub(crate) fn replay_save_click(&mut self, button: usize) {
        if !self.replay_save.up || self.replay_save.overwrite {
            return;
        }
        match button {
            0 if !self.replay_save.name.is_empty() => {
                if self.replay_file(&self.replay_save.name).with_extension("rpl").exists() {
                    tracing::info!(name = %self.replay_save.name, "replay save: the file exists (0x11)");
                    self.replay_save.overwrite = true;
                } else {
                    self.replay_save_as(&self.replay_save.name.clone());
                    self.close_replay_save();
                }
            }
            1 => self.close_replay_save(),
            _ => {}
        }
    }

    /// The overwrite question's answer (`HYesNoOverlay` 0x437770): yes 0x1C with `[0x60CA9D]` —
    /// the flag cleared, `0x42E480(1)` (the save, forced), 0x42E030, the question closed, event 8
    /// (0x4261BF…0x4261FE); no 0x1D — the question closed and `HReplaySave` queued again with its
    /// name (0x426272…0x4262D8).
    pub(crate) fn replay_overwrite_click(&mut self, button: usize) {
        if !self.replay_save.up || !self.replay_save.overwrite {
            return;
        }
        self.replay_save.overwrite = false;
        if button == 0 {
            self.replay_save_as(&self.replay_save.name.clone());
            self.close_replay_save();
        } else {
            self.replay_save.opened = Some(Instant::now());
        }
    }

    /// 0x10 / 0x12 / the yes: the dialog closed and the bar's `+0x199` cleared (0x42E030), and event
    /// 8 — command 3, which **toggles** `[0x606860]` (0x4148BB).
    fn close_replay_save(&mut self) {
        self.replay_save.up = false;
        self.replay_save.overwrite = false;
        self.replay.frozen = !self.replay.frozen;
    }

    /// `<save dir>/<name>` — `[0x65B360]` is `SaveData\` (`career.md` §1); this port's own save
    /// directory, never the game's.
    pub(crate) fn replay_file(&self, name: &str) -> std::path::PathBuf {
        self.replay.path.parent().map(|d| d.join(format!("{name}.prpl"))).unwrap_or_else(|| format!("{name}.prpl").into())
    }

    /// **`Replay_saveAs`** 0x414660 — 0x413FD0 with the name: the final tick taken from the clock
    /// only while recording (`[0x5E99F4] == 0`), the snapshot with `REPLAY = 2`, the tick and the
    /// stream, written to `"%s%s"` over `[0x65B360]` and `"%s.rpl"` of the name (0x42E410…0x42E430).
    /// The port writes its own file and the original's `.rpl` beside it, as its default save does.
    fn replay_save_as(&mut self, name: &str) {
        if let Some(r) = self.replay.stream.as_mut()
            && r.mode == nfs_game::replay::Mode::Recording
        {
            r.final_tick = self.tick;
        }
        let path = self.replay_file(name);
        self.write_replay(&path);
    }

    /// **The automatic replay** — the session loop after a race that reached race over
    /// (0x4B6ECD…0x4B6F18, `replay.md` §10): not network (`FEGAME_TYPE 3`), `REPLAY <= 1`, and
    /// `[0x606A88]` clear (no "race again" pending) → `REPLAY = 2`, 0x412030 (stops the force
    /// feedback effects, which this port does not have), `[0x5E99F4] = 2`, `[0x657428] = 0`, and
    /// back to 0x4B6D9F: the same race set up again (0x4ACB80, 0x4A88A0, 0x471B60, 0x414EB0,
    /// 0x4640B0, 0x413EA0) with no reload, played from the buffer still in memory. Its `RaceFrame`
    /// opens with `0x426A80(REPLAY == 2)` (0x41533A), whose `true` arm reopens the result pages
    /// from the records (0x426B50…0x426C63). The port's race start reloads the track, as its
    /// "race again" does. What the restart does not touch survives it: the records, the track
    /// record already entered, the mission's verdict and the records page.
    pub(crate) fn start_auto_replay(&mut self, event_loop: &ActiveEventLoop) {
        self.auto_replay_due = false;
        if !self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Recording | nfs_game::replay::Mode::Full)) {
            return;
        }
        self.replay_again(event_loop, true);
    }

    /// The session loop's "play it (again)" arm (0x4B6EF4…0x4B6F18): `REPLAY = 2`, `[0x5E99F4] = 2`,
    /// `[0x657428] = 0`, back to 0x4B6D9F. `pages` is whether the replay's `0x426A80(1)` queues the
    /// result pages — not after the bar's restart, which set `[0x60CA10]` (0x426C1C).
    pub(crate) fn replay_again(&mut self, event_loop: &ActiveEventLoop, pages: bool) {
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return };
        if let Some(r) = self.replay.stream.as_mut() {
            r.replay_in_place();
        }
        tracing::info!(tick = self.tick, "automatic replay (0x4B6EFB): the race again from the recorded stream");
        let results = self.results.take();
        let hud_records = self.hud_records.take();
        let mission_result = self.mission_result.take();
        let replay_saved = self.replay.saved;
        self.stop_race_audio();
        self.paused = false;
        // `[0x606AC8]` runs on through the pass (`hud_clock_base`)
        self.hud_clock_base += self.tick as f64;
        self.tick = 0;
        self.race_pass = true;
        self.start_race(renderer, window, event_loop);
        self.hud_records = hud_records;
        self.mission_result = mission_result;
        self.replay.saved = replay_saved;
        self.results = results.map(|r| RaceResults { opened: self.tick, congrats: pages, ..r });
        if pages {
            self.open_result_pages();
        } else {
            // `[0x60CA10]`: nothing queued (0x426C1C), the flag cleared (0x426C6C)
            self.results_page = self.result_pages().len() as u8;
        }
        tracing::info!(pages = ?self.result_pages(), queued = pages, "result pages over the replay (0x426A80(1))");
    }
}

/// **`HReplaySave`** — built once per race by the HUD's init (0x424EA4 → `[0x60C9B0]`), so its
/// name field keeps what was typed until the race is set up again; on the queue while `up`.
#[derive(Default)]
pub(crate) struct ReplaySave {
    pub(crate) up: bool,
    /// the field's buffer `+0x108`, at most `+0x14C` = 11 characters (0x42E0FA, 0x430D30)
    pub(crate) name: String,
    /// the overwrite question: `HYesNoOverlay` `[0x60C9AC]` with 18139 / 18140 (0x4263D1…0x42643D)
    /// and `[0x60CA9D] = 1`, raised in the dialog's place when the name exists
    pub(crate) overwrite: bool,
    /// when the dialog opened, for the caret's blink (`[0x606AC8]` runs on through the pause)
    pub(crate) opened: Option<Instant>,
}

impl ReplaySave {
    /// `0x435630`'s filter on a `WM_CHAR` (0x43567B…0x4356B7): below 0x20 only backspace, and none
    /// of `\ / : * ? " < > | %`; then 0x42E397: backspace drops the last character (0x430D60), any
    /// other goes on the end while the length is short of 11 (0x430D30).
    pub(crate) fn type_char(&mut self, c: char) -> bool {
        let u = c as u32;
        if c == '\u{8}' {
            self.name.pop();
            return true;
        }
        if u < 0x20 || u > 0xFF || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '%') {
            return false;
        }
        if self.name.chars().count() < 11 {
            self.name.push(c);
        }
        true
    }
}

/// Replay, split out of [`App`] on 2026-09-21. The fields dropped their redundant `replay_`
/// prefix: `self.replay` is `self.replay.stream`, `self.replay_speed` is `self.replay.speed`,
/// and `replay_loop` became `loop_count`.
pub(crate) struct Playback {
    /// Input recording / playback (`RE/notes/replay.md`): the original records the four-byte
    /// controller packet per 32 Hz frame and re-simulates everything else.
    pub(crate) stream: Option<nfs_game::replay::Replay>,
    pub(crate) speed: nfs_game::replay::Speed,
    pub(crate) setup: nfs_game::replay::ReplaySetup,
    pub(crate) path: std::path::PathBuf,
    pub(crate) saved: bool,
    /// Loop-iteration counter the playback-speed decimation of `replay.md` §5.1 indexes.
    pub(crate) loop_count: u64,
    /// `[0x60686C]` — the index into `replay::CAMERA_IDS` playback cycles through (`replay.md` §8).
    pub(crate) camera: u8,
    /// The replay pause `[0x606860]` (command 3 toggles it, 0x4148BB) and with it the frozen flag
    /// `[0x606A98]` the frame driver follows (0x415819…0x415824): `RaceFrame` then only renders
    /// (0x415C69) and the audio is paused (0x4AB150). The bar's `+0x199` flips with it. Cleared as a
    /// playback starts (0x413ECB, and 0x42E030 from 0x426A89).
    pub(crate) frozen: bool,
    /// Race over (0x417010) reached in a playback: `[0x606A8C] = 1` with `REPLAY` still 2, so the
    /// frame's end leaves the race as the stream's end does (`App::replay_exit_check`). Cleared
    /// with the rest of the race state as a race starts (0x414EB0 zeroes `[0x606A8C]`).
    pub(crate) race_over: bool,
}

/// A replay file in either format: the port's own, or an original `.rpl` (§6.1 + §16) — the setup
/// snapshot, the final tick and the same stream, played back as far as the port's simulation
/// matches.
pub(crate) fn read_replay_file(d: &[u8]) -> Option<nfs_game::replay::ReplayFile> {
    nfs_game::replay::ReplayFile::parse(d).or_else(|| {
        let (setup, final_tick, stream) = nfs_game::replay::parse_original_rpl(d)?;
        tracing::info!(track = setup.track, "original-format .rpl");
        Some(nfs_game::replay::ReplayFile { setup, final_tick, stream })
    })
}

/// An original `.rpl` names the car by its `nfs5.car` id only (§16.2): the model, the simulation
/// and the first paint from the catalogue.
pub(crate) fn resolve_replay_car(paths: &GamePaths, rs: &mut nfs_game::replay::ReplaySetup) {
    if rs.car.is_empty()
        && rs.car_id != 0
        && let Ok(cat) = std::fs::read(paths.fe_data().join("Data").join("nfs5.car")).map_err(|e| tracing::warn!(error = %e, "nfs5.car"))
        && let Ok(entries) = nfs_formats::fedata::parse_car_catalog(&cat)
        && let Some(e) = entries.iter().find(|c| c.id == rs.car_id)
    {
        rs.car = e.model.clone();
        rs.sim = e.sim.clone();
        rs.paint = e.paint_sets.first().map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
    }
}
