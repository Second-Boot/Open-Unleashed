//! Leaving a race: results settlement, the knockout series, the front end's after-race hand-off and the sim shutdown.

use crate::*;

impl App {
    /// `OpponentCar.Car`'s cars as the accessor holds them — the record by id, not by the port's
    /// catalogue index.
    pub(crate) fn opp_car_saves(catalog: &[nfs_formats::fedata::CarCatalogEntry], picks: &[nfs_game::opponents::OpponentPick]) -> Vec<nfs_game::save::OpponentCarSave> {
        picks
            .iter()
            .filter_map(|p| Some(nfs_game::save::OpponentCarSave { car_id: catalog.get(p.car)?.id, paint_set: p.paint_set, interior: p.interior, package: p.package, decal: p.decal }))
            .collect()
    }

    /// One of those back as a draw over `catalog`; `None` for a record the catalogue lacks.
    fn pick_of(catalog: &[nfs_formats::fedata::CarCatalogEntry], s: &nfs_game::save::OpponentCarSave) -> Option<nfs_game::opponents::OpponentPick> {
        let car = catalog.iter().position(|c| c.id == s.car_id)?;
        Some(nfs_game::opponents::OpponentPick { car, paint_set: s.paint_set, interior: s.interior, package: s.package, decal: s.decal })
    }

    /// `nfs5.car` as the front end's catalogue (`FEData/Data`).
    pub(crate) fn fe_car_catalog(&self) -> Vec<nfs_formats::fedata::CarCatalogEntry> {
        nfs_formats::fedata::parse_car_catalog(&std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car")).unwrap_or_default()).unwrap_or_default()
    }

    /// `nfs5.prt` (`FEData/Data`) — the table [`nfs_physics::specs::stock_parts`] resolves a car's
    /// stock ids against outside the career, where [`CareerTables`](nfs_game::career::CareerTables)
    /// (and its own already-parsed copy) is not loaded. `specs.rs:49`.
    pub(crate) fn fe_parts_table(&self) -> Vec<nfs_formats::career::Part> {
        nfs_formats::career::parse_parts(&std::fs::read(self.paths.fe_data().join("Data").join("nfs5.prt")).unwrap_or_default()).unwrap_or_default()
    }

    /// The block `[0x690410]` as it stands: the series', else the one the profile's `Knockout`
    /// module loaded (0x4DDD60), else the image's zeros.
    fn knockout_block(&self) -> nfs_formats::sav::KnockoutState {
        self.knockout
            .as_ref()
            .map(|k| k.state.clone())
            .or_else(|| self.career.as_ref().and_then(|c| c.side.knockout.as_ref()).map(|k| k.to_state()))
            .unwrap_or_default()
    }

    /// **`Knockout_start` 0x4DDF30** — run by the launch at 0x4D52D9 when `Game.GameType` is 2 and
    /// no series is running (0x4DDF00): `OpponentCar.Car` refills its seven cars (0x4F4FA0 at
    /// 0x4DDF58, the player's `zMergedList.Car` and the menu's choice), the block is set up for
    /// `Game.NumKOOpponents` races and `0x4E3160(n, Game.Difficutly)` draws the series' drivers
    /// (0x4DE011) — the draw that `[0x6907D8]`, the `Personalities` module, now holds
    /// (`nfs_game::career::knockout_start`).
    pub(crate) fn knockout_start(&mut self) {
        let Some(m) = self.fe.menu.as_ref() else { return };
        // `zMergedList.Car`'s own car — the garage half's own record while `car_garage` holds
        let Some(car_id) = self.logo_car(Some("zMergedList.Car"), None).map(|c| c.id) else { return };
        let (opp_choice, n) = (m.opp_car as u32, m.ko_opponents as i32);
        let catalog = self.fe_car_catalog();
        let selected = catalog.iter().position(|c| c.id == car_id).unwrap_or(0);
        let mut crand = nfs_game::opponents::CRand::default();
        let picks = Self::draw_opp_cars(&self.paths, opp_choice, &catalog, selected, &mut crand);
        self.opp_cars = Self::opp_car_saves(&catalog, &picks);
        let drawn = Self::draw_drivers(&self.paths, self.race_skill, n.max(0) as usize, &mut crand);
        let mut state = self.knockout_block();
        nfs_game::career::knockout_start(&mut state, n, &drawn);
        if let Some(c) = self.career.as_mut() {
            c.side.personalities = Some(drawn.iter().map(|p| *p as u16).collect());
        }
        tracing::info!(n, drivers = ?drawn, "knockout series started (0x4DDF30)");
        self.knockout = Some(KnockoutSeries { state, message: String::new() });
    }

    /// **A knockout round's set-up** — 0x4ED5D0 for `Game.GameType` 2: `RACE_TYPE` 2 with the
    /// single race's `SKILL` / `DAMAGE` (0x4EB470), the round's row `Track<raceIndex+1>` for the
    /// track, laps (circuits only), `REVERSE` / `MIRROR` and traffic (0x4FC0B0 → 0x4FC20E), no cops
    /// (0x4ECF00's cop arms are modes 9 and 0), `NUMOPPONENTRACECARS = n − raceIndex` and, from
    /// 0x4DE150, the player's `.STARTINGPOS = start_pos[0]` and for each opponent `i` in 1…n whose
    /// `alive[i]` is 1 (`cmp byte, 1`, 0x4DE1C2) the next slot with car `OpponentCar.Car[i − 1]`
    /// (0x4DE1E7), the driver `nfs5.per[personalities[i − 1]]` (0x4DE1D4) and
    /// `.STARTINGPOS = start_pos[i]`. Returns the track.
    pub(crate) fn knockout_round(&mut self) -> Option<String> {
        let k = self.knockout.as_ref().filter(|k| k.state.active)?.state.clone();
        let m = self.fe.menu.as_ref()?;
        let row = m.ko_row(k.race_index.max(0) as usize);
        // `FETrackAccessor`'s out-of-range rule: row 0 of the list (`Menu::track`)
        let track = m.tracks.get(row.track_idx).or(m.tracks.first())?.clone();
        let (laps, damage) = (fe_laps(&m.circuits, &track, row.laps), m.damage);
        let catalog = self.fe_car_catalog();
        let per = Self::per_table(&self.paths);
        let mut opponents = Vec::new();
        for car in 1..=k.opponents.clamp(0, 8) as usize {
            if k.alive[car] != 1 {
                continue;
            }
            let Some(p) = self.opp_cars.get(car - 1).and_then(|s| Self::pick_of(&catalog, s)) else { continue };
            let mut o = Self::opponent_of(&self.fe.strings, &per, &catalog, &p, Some(k.personalities[car - 1] as i16));
            o.grid = Some((k.start_pos[car] - 1).max(0) as u32);
            opponents.push(o);
        }
        self.player_grid_override = Some((k.start_pos[0] - 1).max(0) as u32);
        self.num_opponents = opponents.len() as u32;
        tracing::info!(track, race = k.race_index + 1, of = k.opponents, grid = ?k.start_pos, drivers = ?opponents.iter().map(|o| (o.name.as_str(), o.car.display_name.as_str())).collect::<Vec<_>>(), "knockout round (0x4DE150)");
        self.career_opponents = Some(opponents);
        self.road.traffic_count = TRAFFIC_COUNT[row.traffic.min(3) as usize];
        self.road.traffic_ids = None;
        self.road.damage_on = damage;
        self.road.cops_enabled = false;
        self.road.cops_race = false;
        self.road.cop_count = 4;
        self.quick_knockout = false;
        self.num_laps = laps;
        self.backwards = row.backwards;
        self.mirror = row.mirror;
        self.knockout_round = true;
        Some(track)
    }

    /// **`Knockout_afterRace` 0x4DE060**, from the race end 0x4EDF00 for a type-2 race run to its
    /// end: `pos[k]` is racer `k`'s place — slot order, the player first and the opponents in the
    /// order 0x4DE150 gave them slots — padded with 99 (`0x690AB0[9]`), and the loops count to
    /// `Game.NumKOOpponents`' value now (`nfs_game::career::knockout_after_race`).
    /// `FEGAME_TYPE [0x6573E8]` of the race running (option 0x30, written by 0x4EB470 from the
    /// launch's `Game.GameType`): the layout front end's type when it started the race; for the
    /// text menu and the command line, 1 for a Quick Knockout (`RACE_TYPE` 3), 2 for a knockout
    /// round, else 0 — APPROXIMATION for those port-only launches: a text-menu career event (the
    /// original's 4) answers 0, which picks the same summary title.
    pub(crate) fn race_fegame_type(&self) -> u8 {
        self.fe.race_game_type.unwrap_or(if self.quick_knockout {
            1
        } else if self.knockout_round {
            2
        } else {
            0
        })
    }

    /// `HRaceSummary`'s title by `FEGAME_TYPE`, chosen by its creator **0x424BD2…0x424BEF**: 1 →
    /// 18175 "Quick Knockout", 2 → 18169 "Knockout", anything else 18129 "Single Race" — handed
    /// to the constructor 0x42BCD0 as the billboard's first title, beside its own 18130 "Race
    /// Summary" (0x42BCDF). *(Until 2026-09-27 the port keyed this on its text menu's `mode`,
    /// whose 1 and 2 are career modes and whose knockouts are 4 and 5, so no knockout race —
    /// layout or text menu — showed its title, and a text-menu career race showed "Quick
    /// Knockout".)*
    pub(crate) fn summary_title(fegame_type: u8) -> u32 {
        match fegame_type {
            1 => 18175,
            2 => 18169,
            _ => 18129,
        }
    }

    pub(crate) fn knockout_race_end(&mut self, menu: &mut Menu) {
        if !std::mem::take(&mut self.knockout_round) {
            return;
        }
        let (mut order, _) = self.finishing_order();
        // diagnostic: `NFS_KO_PLACE=<p>` files the player at place `p` (1-based) of the order —
        // a scripted series (`NFS_FE_KO_SERIES`) cannot drive past anyone, so this is how a still
        // reaches the rounds after the first and `knockoutraceinfo`
        if let Some(p) = std::env::var("NFS_KO_PLACE").ok().and_then(|v| v.trim().parse::<usize>().ok())
            && let Some(at) = order.iter().position(|d| *d == 0)
        {
            let d = order.remove(at);
            order.insert(p.clamp(1, order.len() + 1) - 1, d);
        }
        let pos: Vec<i32> = (0..9).map(|d| order.iter().position(|x| *x == d).map_or(99, |p| p as i32 + 1)).collect();
        let Some(k) = self.knockout.as_mut().filter(|k| k.state.active) else { return };
        let before = k.state.race_index;
        nfs_game::career::knockout_after_race(&mut k.state, &pos, menu.ko_opponents as i32);
        let s = &k.state;
        tracing::info!(race = before + 1, ?pos, active = s.active, alive = ?&s.alive[..9], start_pos = ?s.start_pos, last_pos = ?s.last_pos, "knockout race filed (0x4DE060)");
        // the text menu's status line (port-only)
        k.message = if !s.active && s.race_index == before {
            format!("KNOCKED OUT IN RACE {} OF {}", before + 1, s.opponents)
        } else if !s.active {
            format!("KNOCKOUT WON  ({} RACES)", s.opponents)
        } else {
            format!("RACE {} OF {}   (P{} LAST RACE)   ENTER", s.race_index + 1, s.opponents, pos[0])
        };
        menu.knockout_line = k.message.clone();
    }

    /// **The race run to its end is left** (the last page's Space, or the automatic replay running
    /// out, 0x415CB3 → `REPLAY = 3` → the session loop's exit 0x4B6F2F): the mission and the career
    /// event are filed from the result records, the replay is saved as the de-init chain does
    /// (`Replay_saveDefault` 0x413FD0 at 0x4B69F1) and the front end comes back with `[0x657E34]`
    /// clear. `false` when there is no menu to go back to (the app was started straight into a
    /// race).
    pub(crate) fn end_finished_race(&mut self) -> bool {
        self.file_track_record();
        let Some(menu) = self.fe.menu_template.take() else {
            return false;
        };
        self.leave_race_mode();
        self.save_replay();
        self.replay_session_end();
        // 0x501340 + `LoadInfo` (0x4D3B52), before this race's own filing — same rule as
        // `file_track_record`'s own reload, applied to the career
        self.reload_career_at_fe_entry();
        self.settle_mission();
        if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
            let (order, times) = self.finishing_order();
            let clean = self.clean_racers();
            // the damage is the record's `+0xC98`, frozen at race over — not the replay's under the
            // pages (0x4276A4; the park is 0x493CF0's fold, run by 0x416AC0 there)
            let zones = self.result_damage();
            if let Some(c) = self.career.as_mut() {
                // the car is parked with the damage it took, which sticks to its parts
                // until the shop repairs them (`career.md` §11)
                if let Some(z) = zones {
                    c.park(z);
                }
                c.finish(&order, &times, &clean);
            }
        }
        self.skill = None;
        self.mission = None;
        self.career_opponents = None;
        self.stop_race_audio();
        let mut menu = menu;
        self.knockout_race_end(&mut menu);
        self.player_grid_override = None;
        self.fe.menu = Some(menu);
        self.sim_shutdown();
        // the race was run to its end: `[0x657E34]` is clear
        self.fe_after_race(false);
        self.results = None;
        true
    }

    /// **The track record** — `0x4EDF00`, the front end's race end (0x4EDD00 → 0x4EDF00), after the
    /// session loop has left: not when the race was aborted (`[0x657E34]`, 0x4EDF00's first test —
    /// every quit through the pause menu, before or after the finish, 0x426182), not in a network
    /// race (3) or a mission (9) (0x4EDF31 / 0x4EDF36); the test drives are not excluded.
    /// `Records_insert` 0x4FAFC0 (or its thunk 0x4FB100) takes the player's result record: the
    /// total `+0xCD8` on a point-to-point track, the best lap `+0xCFC` on a circuit (`[0x5E77A4]` /
    /// `[0x5E77C8]`, 0x4EDF67…0x4EDF9C), under the strict `<` with no zero check — a player knocked
    /// out before a lap does enter 0:00.00, as the original does. The table the race showed
    /// (`HRecordTime`, the records page) is the one from before it. *(Until 2026-09-26 the port
    /// inserted at the player's live finish, so a race quit from the pause menu afterwards was
    /// entered too.)* The port files it on its way back to the menu, or at the end of a race run
    /// with no front end.
    ///
    /// **The reload.** `FEApplication::run` 0x4D3420's entry preamble calls `LoadTrackRecords`
    /// 0x4FAE20 unconditionally at 0x4D36BE, well before this race's own filing (0x4D3EB2's
    /// dispatch table, entered later in the same call) — so every return to the front end
    /// re-reads `SaveData\nfs5.trk` from disk (re-applying the empty-slot `"n/a"`/359999 fixup)
    /// *before* this race's result goes in, and the insert then lands directly on that fresh,
    /// live buffer with no further reload (`RE/notes/savegame.md` §7a). The port used to load
    /// records once at start-up (`main.rs`'s `Records::load`) and mutate `self.records` in place
    /// forever after — matched here by redoing the same load at the top of every filing, so a
    /// 0-tick row this session's *previous* race entered now reads back correctly as `"n/a"` /
    /// `59:59.99` on this entry too, exactly as 0x4FAE20 would leave it. *(Corrected
    /// 2026-09-27: `Records::load` at start-up only, §7a's recorded gap.)*
    /// The player's short car label for a result record — `nfs5.car +0x34`'s **family** string
    /// (e.g. `911 Turbo`, `911 S`, `944`), not `CarSpecs::name` (the `.sim` file's own descriptive
    /// name, e.g. `1994 993 cabrio 3`). Traced 2026-09-29 (`modes.md` §6.3/§7.1, dated paragraph):
    /// at sim shutdown (0x4B68F0) `car+0xBB8` is `strcpy`'d from the FE car/race-slot record's
    /// `spec+0x534` (`[car+0x558]+0x534`, `spec+0x17C` next to it being the *driver* name), then
    /// bulk-copied (0x4B69BC…0x4B69E6, 0x96 dwords from `car+0xAD4`) into the per-racer result
    /// record `0x5E75A0 + i·0x258`, whose own `+0xE4` is that same string (modes.md §7.1's record
    /// layout). Both consumers read that `+0xE4` slot, not `spec+0x17C`/`CarSpecs::name`:
    /// `HTrackRecords::insertPlayer` 0x433A20 (the results board's `Рекорд круга`/Lap Record АВТО
    /// cell) and the persistent `nfs5.trk` "car" field (`Race_finalize`'s 0x4EDF00 →
    /// `Records_insert` 0x4FAFC0/0x4FB100, pushing the fixed global `0x5E7684` = `0x5E75A0+0xE4`,
    /// slot 0 = player — confirmed algebraically and by `bscan.py 0x5E7644 0x5E7684` finding both
    /// only at that one call site). `HRaceSummary`'s CAR column (0x42C368…0x42C3BA) reads the same
    /// `spec+0x534` directly, capped to 15 chars — independent corroboration that it is a short
    /// field, not the full name. The shipped ghost rows in `nfs5.trk`/`records.json` are already
    /// this short form (byte-identical to the original, pass 9); only the player's own row used
    /// `CarSpecs::name` until this fix.
    pub(crate) fn player_car_family(&self) -> String {
        let family = |t: &nfs_game::career::CareerTables| t.car(self.car_id).map(|c| c.family.clone());
        match self.career.as_ref() {
            Some(c) => family(&c.tables),
            None => nfs_game::career::CareerTables::load(&self.paths).ok().and_then(|t| family(&t)),
        }
        .unwrap_or_else(|| self.specs.name.clone())
    }

    pub(crate) fn file_track_record(&mut self) {
        self.records = Records::load(&self.paths, &self.records_path);
        tracing::debug!(path = %self.records_path.display(), "track records reloaded (0x4FAE20, before this race's own filing)");
        if self.net.session.is_some() || self.mission.is_some() {
            return;
        }
        let Some(me) = self.results.as_ref().and_then(|r| r.racers.first()) else { return };
        let circuit = self.records.track(&self.track.name).map(|t| t.circuit).unwrap_or(false);
        let time = if circuit { me.best_lap } else { me.time } as u32;
        let (driver, car_name) = (self.player_name.clone(), self.player_car_family());
        if let Some(rank) = self.records.insert(&self.track.name, &driver, &car_name, time) {
            tracing::info!(rank, time = format_ticks(time as u64), circuit, "track record entered (0x4EDF00)");
            if let Err(e) = self.records.save(&self.records_path) {
                tracing::warn!(error = %e, "records save failed");
            }
        } else {
            tracing::info!(time = format_ticks(time as u64), circuit, "no track record (0x4EDF00)");
        }
    }

    /// Abandon the running race and go back to the menu; `false` when there is no menu to go back
    /// to (the app was started straight into a race).
    pub(crate) fn leave_race(&mut self) -> bool {
        self.leave_race_as(true)
    }

    /// [`Self::leave_race`] with the front end's `[0x657E34]` (aborted) chosen by the caller: the pause
    /// menu's quit sets it (0x426182), the network race's Esc leave (0x41523A) does not.
    pub(crate) fn leave_race_as(&mut self, aborted: bool) -> bool {
        let Some(menu) = self.fe.menu_template.take() else {
            return false;
        };
        self.leave_race_mode();
        self.save_replay();
        self.replay_session_end();
        // a `STARTREPLAY` playback (`[0x699FC9]`, 0x50C120) is rebuilt at 0x4D3EB2 and jumps to
        // 0x4D5207: neither the race end 0x4EDD00 nor the series test 0x4D3F0A runs for it
        let playback = self.fe.after_replay;
        let zones = self.result_damage();
        // 0x501340 + `LoadInfo` (0x4D3B52): every return to the front end reloads the profile,
        // aborted or a replay alike, before the abort arm of 0x4EDD00 runs
        self.reload_career_at_fe_entry();
        if let Some(c) = self.career.as_mut().filter(|_| !playback) {
            // an aborted tournament race still runs the race end 0x4EDD00: the damage goes back
            // on the car, the market ticks, the current event is reset and nothing is filed
            // (`CareerUi::abort`); any other race just drops its pending entry
            if !c.abort(zones) {
                c.pending = None;
            }
        }
        self.sync_save_side_state();
        // 0x4D3F0A: an aborted race whose type is not 5…9 cancels the series (0x4DE040 clears
        // `active` only) — a quit test drive or mission leaves it running
        self.knockout_round = false;
        let race_type = self.fe.race_game_type.unwrap_or(self.fe.game_type);
        if !playback
            && !(5..=9).contains(&race_type)
            && let Some(k) = self.knockout.as_mut()
            && k.state.active
        {
            k.state.active = false;
            k.message = "SERIES CANCELLED".into();
        }
        self.player_grid_override = None;
        self.career_opponents = None;
        self.skill = None;
        self.mission = None;
        self.stop_race_audio();
        self.fe.menu = Some(menu);
        self.sim_shutdown();
        // left from the pause menu: `[0x657E34]` is set
        self.fe_after_race(aborted);
        true
    }

    /// **The replay's mode at the next session** — `Replay_begin` 0x413CF0, the start of every
    /// session's race load (0x4B67CC), sets `[0x5E99F4] = REPLAY [0x657424]` (0x413D04), and
    /// `REPLAY` is `fe.txt`'s `replay = 0` again by then: the session loop re-reads `fe.txt`
    /// (0x4B6660 at 0x4B6C1C) and applies it (0x4B4B80 at 0x4B6C83) before each race. So the
    /// race after a playback **or after a recording that filled the buffer** (mode 1, which
    /// `Replay_startRace` 0x413EA0 never clears) records afresh. A replay the front end starts
    /// (`STARTREPLAY`) sets its own stream. *(Until 2026-09-26 only a playback was reset here, so
    /// the race after an overflowed recording recorded nothing; and the rule was put down to the
    /// front-end controller's constructor zeroing `[0x699FC9]` at 0x50C16F — that byte is the
    /// front end's own "start a replay" flag, `STARTREPLAY` 0x50D390's, not the replay mode.)*
    pub(crate) fn replay_session_end(&mut self) {
        self.replay.stream = Some(nfs_game::replay::Replay::new());
    }

    /// **The sim shutdown 0x4B68F0** as far as the port keeps its objects: the race's car, world,
    /// models and sky dropped (0x4690F0 undoes the track load — the weather 0x458BB0, the sky
    /// 0x45DD10 among it —, 0x414F80 the race's init, 0x471C40 deletes the camera manager) and the
    /// HUD torn down (0x425330 at 0x4B6A23).
    pub(crate) fn sim_shutdown(&mut self) {
        // an input-exchange race's player reports `0xB7 (−2, −1)` to the room as it leaves (0x48DA60)
        if let Some(n) = self.net.session.as_mut() {
            n.exchange_leave();
        }
        self.net_dump = None;
        self.net_notice = None;
        self.net_notice_space = false;
        self.net_lag_box = false;
        self.vehicle = None;
        self.race = None;
        self.world = None;
        self.car_model = None;
        self.sky = None;
        self.scene = Scene::default();
        self.hud_teardown();
    }

    /// **The HUD torn down — 0x425330**, from the sim shutdown 0x4B68F0 (0x4B6A23), which runs only
    /// where the session loop is left for the front end (0x4B6F2F) — not on a restart or the
    /// automatic replay. It is memory work alone: the ~30 HUD elements `[0x60C9AC]…[0x60CA7C]`
    /// deleted through their virtual destructor (`push 1; call [vtbl]`) and their pointers zeroed,
    /// 0x419DC0 (the object `[0x60C7E8]` and the buffers `[0x60AB10]` / `[0x60AB18]` / `[0x60AB1C]`
    /// freed), the two dialog vectors `[0x60C9F4]` / `[0x60C9C4]` emptied and freed, the shared HUD
    /// resources released by their reference count `[0x60C978]` (0x41E2C0) and the skill-test HUD's
    /// vector `[0x610A80]` freed (0x434F00). Nothing it zeroes is read before the next race's HUD
    /// init (the elements built afresh from 0x4244A4 on, the shared resources re-acquired by 0x41E170 at 0x42449A), so what it means for the port is that
    /// the next race starts with the elements' own state at its constructor values; the port keeps
    /// that state on `App` and resets it in [`App::start_race`] (0x426B20…), save for the two
    /// fields only the elements' construction clears: `HButtonVisual +0x100`, the button under the
    /// pointer ([`App::hud_hover`] — a race left by the keyboard kept it), and the page billboard's
    /// shrink (`HBillBoard` state 2, [`App::page_closing`]).
    fn hud_teardown(&mut self) {
        self.hud_hover = None;
        self.page_closing = None;
    }

    /// **The front end coming back from a race — 0x4D3420 at 0x4D3EB2…0x4D5202**, the screen
    /// stack rebuilt by [`after_race_screens`] over the entry screen `gamesetup`. Only a race the
    /// layout front end started has a stack to rebuild (`race_game_type`); one from the text
    /// menu goes back to it. `aborted` is `[0x657E34]`, a race left from the pause menu.
    ///
    /// Inputs as this port has them: the network test (0x4D4072…0x4D40A6, the session object
    /// `[0x628C70]` linked, not ejected and holding a player id) is "a session is open", and host
    /// (0x48CAA0: our id is the first player's) is `NetPlay::is_host` — APPROXIMATION of both.
    /// `TOURNAMENTLASTRACE` / `TOURNAMENTENDOFERA` (options 0xB9 / 0xBA) are what the event's
    /// launch set and the era-changed byte (0x505800) what filing it left
    /// (`CareerUi::race_flags`). The mission pair is the career's (0x51AF90 `promotion_due`,
    /// 0x51B020 `current_mission` none left).
    pub(crate) fn fe_after_race(&mut self, aborted: bool) {
        let replay = std::mem::take(&mut self.fe.after_replay);
        // 0x4B6F34…0x4B6F60: the session loop's exit after the race — `0x48DAD0`, `0xB7 (−1, −1)` when
        // no time was sent, then `0xB7 (−2, −1)`; every machine's `net+0xE5` bit for us goes. (Glue:
        // only after a race a launch numbered, `in_race != 0`.)
        if let Some(n) = self.net.session.as_mut().filter(|n| n.in_race != 0) {
            n.leave_race();
        }
        self.fe.test_drive = None;
        let Some(game_type) = self.fe.race_game_type.take() else { return };
        // 0x4D36BE: `LoadTrackRecords` runs on every entry, aborted or not. A non-aborted race
        // already redid it at the top of `file_track_record` (before *this* race's own filing,
        // per that fn's doc) — an aborted race never calls `file_track_record` at all, so redo
        // it here for that path alone (avoids reloading a second time right after this same
        // race's own insert+save, which would fixup its own 0-tick row one entry early).
        if aborted {
            self.records = Records::load(&self.paths, &self.records_path);
            tracing::debug!(path = %self.records_path.display(), "track records reloaded (0x4FAE20, aborted-race entry)");
        }
        // every entry after the first puts up the loading screen: 0x4DDA00(0) at 0x4D345C …
        // 0x4DDA00(9) at 0x4D521D (`fe_loading_frame`)
        self.fe_loading_frame(0);
        // 0x4D3B52: an entry after the first reloads the profile `player.lst` names
        // (`LoadInfo(0x4DF3C0(0), 0)`); when that fails, the stack is not rebuilt at all — the
        // entry screen with the `selectname` layer over it, as at start-up (0x4D3E1D)
        if !self.fe_profile_loadable() {
            tracing::info!(game_type, "front end: back from the race, no profile to load (0x4D3B63) — selectname");
            self.fe.layout_under.clear();
            self.fe.layout_preview = None;
            self.fe.nav = nfs_game::layout::nav::Nav::with_screen(FE_ENTRY);
            // 0x4D5267 runs on this path too
            self.fe_tournament_refresh();
            self.open_layout(FE_ENTRY);
            self.fe_select_name();
            self.fe_loading_frame(nfs_game::layout::loading::LAST_STEP);
            return;
        }
        // the two rebuilds of 0x4FBC10 on the way back: `LoadInfo`'s (0x4DED10, from the reload at
        // 0x4D3B52) and 0x4F3E40's at the end of the race end 0x4EDD00 (0x4EDD18, after 0x4EDF00
        // and the tournament filing 0x503AD0 with its own 0x4F3E40 runs have stepped the year;
        // 0x4D3EEC, skipped for a replay) — both for the year the race left and the same
        // `Game.GameType`, so one here carries them
        self.car_lists_head();
        self.rebuild_track_lists();
        let flags = self.career.as_ref().map(|c| c.race_flags).unwrap_or_default();
        // 0x4EDD00 on a type-4 race, run or aborted, ends in 0x503A50 and 0x503AD0: the next event
        // `[0x691E7C]` is the first undone one again — `Tournament.RemainingTracks`' first row
        if game_type == 4 {
            self.fe.remaining_sel = 0;
        }
        let (promotion, missions_done) = self
            .career
            .as_ref()
            .map_or((false, false), |c| (c.career.promotion_due(&c.tables).is_some(), c.career.current_mission(&c.tables).is_none()));
        let input = AfterRace {
            replay,
            aborted,
            game_type,
            knockout_active: self.knockout.as_ref().is_some_and(|k| k.state.active),
            net: self.net.session.as_ref().map(|n| (self.fe.comms_game_mode, n.is_host())),
            last_race: flags.last_race,
            end_of_era: flags.end_of_era,
            era_changed: flags.era_changed,
            promotion,
            missions_done,
        };
        let (stack, screen, layer) = after_race_screens(&input);
        // the manager is new on every entry (one empty layer); 0x4D5BE0 per name, then
        // 0x4D5DF0(`vec[0]`, screen, 1) — or 0x4D2100(name, 1, 1) for the second layer
        let mut nav = nfs_game::layout::nav::Nav::new();
        for s in &stack {
            nav.push_history(s);
        }
        nav.show_first(screen);
        if let Some(top) = layer {
            nav.open(top, 1, true);
        }
        tracing::info!(game_type, aborted, replay, ?flags, nav = nav.describe(), "front end: back from the race (0x4D3420)");
        // the screens of the last visit were destroyed when the front end handed over to the
        // race (their destructors ran then, the lock with them); the manager is new
        self.fe.layout_under.clear();
        self.fe.layout_preview = None;
        self.fe.nav = nav;
        // 0x505570 at 0x4D5267, before the event loop: the screens opened below read it
        self.fe_tournament_refresh();
        self.open_layout(screen);
        if let Some(top) = layer {
            self.fe_push_under();
            self.open_layout(top);
        }
        self.fe_loading_frame(nfs_game::layout::loading::LAST_STEP);
    }

    /// **0x505570** — the `Tournament.*` accessors put back, run on every entry to the front end
    /// (0x4D5267, after its start-up and before its event loop: at start-up and after every
    /// race) and at the end of the profile switches (`SETNAMETHENBACK` 0x510715 / 0x51074D /
    /// 0x510762, `CREATEPLAYERDONE` 0x51081B…0x510976 — every path but the empty name).
    /// Read to the instruction 2026-09-27:
    ///
    /// ```text
    /// 0x505570  if ([0x691E54]) 0x4FD1A0()              ; EventType rebuilt: the three club lists
    /// 0x50558C  0x5045C0(&id, &row)                      ; the row to show (Career::default_tournament_row)
    /// 0x5055BE  Tournament.Tournament: slot 2 (refresh), slot 11 (row)
    /// 0x5055CD  [0x691E6C] = the table index of id
    /// 0x505622  0x506AA0(Tournament.EventType)           ; its count by the era: 521→1, 522→2, 523→3
    /// 0x505651  if (Game.GameType ∈ {5, 10, 11}) Game.GameType = 4
    /// 0x505696  Tournament.RemainingTracks = row 0
    /// ```
    ///
    /// So the tournament screen always opens on the first tournament under way, else the first
    /// never completed — not where the player left it — and a test drive (5 `prerace`, 11
    /// `VictoryRoom`) or the finale (10) is a career again. *(Until 2026-09-27 the port kept the
    /// row the player had and `Game.GameType` 5 / 11 after a test drive, and `EventType` had its
    /// four rows in every era, the Evolution's end included.)*
    pub(crate) fn fe_tournament_refresh(&mut self) {
        const EV: &str = "Tournament.EventType";
        // the index as the accessor holds it; 0x4FD1A0 carries it over to the new accessor (slot
        // 10 read, then slot 11 — `SetSelection` 0x4BC240, clamped into the three rows)
        let mut ev_type = self.choices.get(EV).copied().unwrap_or(0);
        if let Some(c) = self.career.as_mut() {
            if c.career.evolution_complete {
                ev_type = ev_type.min(2);
            }
            c.refresh_tournament(ev_type);
            // 0x506AA0 → `setCount` 0x4BC310: an index not below the era's count goes to 0
            let kept = if ev_type < c.event_type_rows().len() { ev_type } else { 0 };
            self.choices.insert(EV.to_string(), kept);
        }
        if matches!(self.fe.game_type, 5 | 10 | 11) {
            self.fe.game_type = 4;
        }
        self.fe.remaining_sel = 0;
    }

    /// **0x501340 + 0x4D3B52's `LoadInfo`** — the FE-entry reset and reload, `controls.md` §4.172:
    /// on *every* front-end entry (start-up; the return from a race, a replay or a Factory Driver
    /// skill test — all of them re-enter `FEApplication::run` 0x4D3420), the preamble zeroes the
    /// live `PlayerInfo` block (0x501340: money `[0x691D90]` = 11000, the garage
    /// `[0x691DF8]`/owned-list `[0x691DFC]` vectors, the flags `0x691D90…0x691DFC`, picture
    /// `[0x5DB748]` = 8) and then unconditionally reloads the current profile from disk
    /// (`LoadInfo` 0x4DF3C0 + 0x4DE9B0), which overwrites every one of those fields with the
    /// `.sav`'s own values. Net effect, since nothing else in the original writes a field the
    /// reload does not also set: **discard whatever the live career holds and read the profile
    /// back off disk** — the same operation `CareerUi::switch_profile` already does for an
    /// explicit profile switch, just aimed at the *current* profile rather than a different one.
    /// Called before this race's own result is filed (`park`/`finish`/`finish_mission`), exactly
    /// where 0x4D3B52 sits relative to the race-end dispatch table 0x4D3EB2's own 0x4EDD00 — the
    /// same ordering rule already applied to `LoadTrackRecords` 0x4FAE20 in
    /// [`Self::file_track_record`]'s own doc. Decided 2026-09-28: port this
    /// as the original — an unsaved purchase, sale, repair or flag change vanishes at the next FE
    /// entry unless a race was launched, `LOADGAMEBUTTON` was pressed or the profile was created
    /// in between (`RE/notes/savegame.md`, "Save timing, ported 2026-09-28"). No-op when there is
    /// no career (a non-career session) or the profile file does not parse yet (first-ever entry,
    /// before any of the three save sites has run) — `fe_profile_loadable`'s own check below
    /// still decides whether the screen stack can be rebuilt at all.
    pub(crate) fn reload_career_at_fe_entry(&mut self) {
        if let Some(c) = self.career.as_mut() {
            let p = c.profile.clone();
            let money_before = c.career.money;
            if c.switch_profile(&p) {
                tracing::debug!(profile = %p.name, money = c.career.money, money_before, "front end: profile reloaded from disk (0x501340 + 0x4D3B52)");
            } else {
                tracing::debug!("front end: profile reload at FE entry found nothing on disk yet (0x4D3B52) — keeping the live career");
            }
        }
    }

    /// **0x4DF3C0(0)** then **0x4DE9B0**(name, 0) as 0x4D3B52 calls them: `player.lst` in the
    /// save directory names the profile (at most 100 bytes, 0x4DF3F6) and `LoadInfo` must read
    /// its `.sav`. The port's test is that the file parses (`CareerUi::switch_profile`'s); the
    /// actual reload is [`Self::reload_career_at_fe_entry`], called by every caller of this check
    /// before the race's own result is filed. *(Until 2026-09-28 the reload was not repeated —
    /// `savegame.md`'s "when the profile is saved" question, answered "as the
    /// original".)*
    fn fe_profile_loadable(&self) -> bool {
        let dir = nfs_game::save::save_dir();
        nfs_game::save::read_player_lst(&dir)
            .filter(|stem| stem.len() <= 100)
            .and_then(|stem| std::fs::read(dir.join(format!("{stem}.sav"))).ok())
            .is_some_and(|d| nfs_formats::sav::SaveFile::parse(&d).is_ok())
    }

    /// **0x4D3E7C…0x4D3EA4** — the profile chooser on a layer of its own over the entry screen:
    /// `0x4D2100("selectname", 1, 1)`, then the pointer to (320, 394) — 0x5370C0(3, 0x140) and
    /// 0x5370C0(4, 0x18A), the x and y of `[0x69E07C]` / `[0x69E080]` handed to 0x5727C0.
    pub(crate) fn fe_select_name(&mut self) {
        self.fe_open("selectname", 1, true);
        self.fe.cursor = Some((320.0, 394.0));
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_cursor(self.fe.cursor);
        }
    }

    /// `Race_finalize(2)` 0x416AC0 over the live race: the ranking table and every racer's record.
    fn finalize_race(&self) -> (Vec<usize>, Vec<nfs_game::race::RacerResult>) {
        use nfs_game::race::{FinalizeIn, STATE_FINISHED, STATE_OUT, STATE_RACING};
        // **The order is the ranking table 0x608B38** (`race::ranking`): progress `car+0xAF8`
        // descending, ties by the earlier `car+0xB14`, whatever the car's state — `Race_finalize`
        // 0x416AC0 reads each racer's place `car+0xCD4` straight from it. *(Until the audit of
        // 2026-09-26 the port put every car with its finished flag first, ordered by its lap
        // times: a car knocked out of a knockout — flagged finished with fewer laps — was ranked
        // ahead of the cars still racing, and a finisher was never ranked on its frozen progress.)*
        let mut drivers: Vec<usize> = Vec::new();
        let mut keys: Vec<(f32, u64)> = Vec::new();
        let mut cars: Vec<FinalizeIn> = Vec::new();
        let state = |finished: bool, out: bool| if out { STATE_OUT } else if finished { STATE_FINISHED } else { STATE_RACING };
        if let Some(r) = &self.race {
            drivers.push(0);
            keys.push((r.progress, r.tick));
            // a knocked-out car's `car+0xB24` is zeroed (0x416649…0x4166FD); the port also flags
            // it finished, so the knock-out is tested first
            let damage = self.vehicle.as_ref().map_or([0.0; 10], |v| v.damage);
            cars.push(FinalizeIn::from_progress(r, state(r.finished, self.player_eliminated), 0, damage));
        }
        for (i, o) in self.road.opponents.iter().enumerate() {
            drivers.push(i + 1);
            keys.push((o.progress.progress, o.progress.tick));
            cars.push(FinalizeIn::from_progress(&o.progress, state(o.progress.finished, o.eliminated), 0, o.vehicle.damage));
        }
        // the racers' insertion order into 0x608B38 is the Sc64 list's at priority 0x19: every
        // car's `+0xE18` = 0x4169B0 → `Race_updateProgress` 0x416310 (0x416A14), inserted car by
        // car in `0x5E8B34` order by `Cars_create` (0x411277…0x411349), each after every entry of
        // equal priority (0x414C30, `jl` at 0x414C50; `traffic.md` §17.5e) — the human, then the
        // opponents by slot. So a full tie (same progress, same tick) keeps the player ahead, and
        // `ranking`'s stable sort over this order is the table's
        let rank = nfs_game::race::ranking(&keys);
        // `car+0xB48`, the live position the lap knockout tests (0x416DE8)
        for (row, &k) in rank.iter().enumerate() {
            cars[k].position = row as u32 + 1;
        }
        // **Every racer gets a time.** `Race_finalize` 0x416AC0 projects one for every opponent
        // still in state 0 (0x416E16…) — the time `0x503960` records for the tournament and
        // 0x502880 sorts on — except a car a lap knockout puts out, which has none (`RACE_TYPE 3`)
        let humans = usize::from(self.race.is_some());
        let tick = self.tick;
        let mut records = nfs_game::race::race_finalize(&mut cars, humans, &rank, self.num_laps, tick, self.linear_track(), self.quick_knockout);
        // by driver index
        let mut by_driver = vec![nfs_game::race::RacerResult::default(); records.len()];
        for (k, r) in records.drain(..).enumerate() {
            by_driver[drivers[k]] = r;
        }
        let mut order: Vec<usize> = rank.into_iter().map(|k| drivers[k]).collect();
        if let Some(p) = race_place_diagnostic() {
            place_player(&mut order, &mut by_driver, p);
        }
        (order, by_driver)
    }

    /// **Race over** — `0x417010`, first in `GameTick`'s odd-tick group (0x4150D9): once every
    /// human car is in state ≥ 2 and the tick is past its `+0xB14` by more than **0x140** (0x4170DC…
    /// 0x4170F3, `jle` — strictly more), `[0x606A8C] = 1` and `Race_finalize(2)` 0x416AC0 fill the
    /// result records (0x417136…0x417142). Never in a network race (`FEGAME_TYPE 3`, 0x417128) and
    /// never twice (`[0x606A8C] == 0`, 0x4170BC). **It has no replay test** (0x417010…0x41714F):
    /// a playback whose humans get there before the stream runs out ends its race loop here too
    /// (`Playback::race_over`, left at the frame's end by `replay_exit_check`) — and the session
    /// loop then leaves at `REPLAY == 2` (0x4B6ED5…0x4B6EE6), no second replay. *(Until
    /// 2026-09-27 this said "a playback cannot reach it" and the port skipped it in a playback.)*
    /// There the playback runs `Race_finalize(2)` a second time, and an automatic replay's race
    /// end files those records; after a `--replay` or a `STARTREPLAY` nothing reads them
    /// (0x4D3EB2 skips the race end 0x4EDD00).
    ///
    /// While the announcer is still talking — the speech ring's read index `[0x656AE4]` short of
    /// its write index `[0x657300]`, or a phrase's voices `[0x6573A8]` / `[0x6573AC]` not −1
    /// (0x4B08F2…0x4B09A3) — the original waits **0x280** ticks instead (0x4170F5…0x417116);
    /// `nfs_audio::speech::Speech::busy` answers that (a phrase queued or one playing).
    pub(crate) fn race_over_check(&mut self) {
        // a network race (`FEGAME_TYPE 3`): 0x417010 never runs (0x417128 tests the game type, not the link — a
        // race whose session was torn down is still no single-player race); the pages come from the room's times
        if self.fe.race_game_type == Some(3) {
            self.net_results_check();
            return;
        }
        let playback = self.replay.stream.as_ref().is_some_and(|s| matches!(s.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        if self.net.session.is_some() || self.replay.race_over || (!playback && (self.results.is_some() || self.auto_replay_due)) {
            return;
        }
        let Some(r) = self.race.as_ref() else { return };
        // every human (the port has one): state ≥ 2 — finished, or knocked out — and more than
        // 0x140 ticks past its finish; while the announcer is busy (the speech ring not drained or
        // a phrase's voices still held, 0x4170F5…0x41710D) more than 0x280 (`jle`, strictly after)
        let wait = if self.snd.speech.as_ref().is_some_and(|sp| sp.busy()) { 0x280 } else { 0x140 };
        if !(r.finished && self.tick > r.tick + wait) {
            return;
        }
        if playback {
            tracing::info!(tick = self.tick, "race over (0x417010) in a playback: the race loop ends");
            self.replay.race_over = true;
            // **The playback's own `Race_finalize(2)`** (0x417136…0x417142, no replay test in
            // 0x416AC0 either): the records `+0xCD4` / `+0xCD8` / `+0xCD0` / `+0xCDC…` / `+0xCFC` /
            // `+0xC98` are written again from the replayed cars, and the sim shutdown 0x4B68F0
            // copies those to `0x5E75A0` for the race end 0x4EDF00 — an automatic replay files
            // them. Only the records: the clean flag `+0xCC0` is 0x427660's (the pages' tally at
            // the replay's start, 0x426C36), the mission verdict `[0x657834]` 0x4293F0's, and
            // neither runs again here. A `--replay` / `STARTREPLAY` playback has no race end
            // (0x4D3EB2 → 0x4D5207), so nothing reads its records and the port keeps none.
            if self.results.is_some() {
                let (order, racers) = self.finalize_race();
                tracing::info!(tick = self.tick, order = ?order, places = ?racers.iter().map(|r| r.place).collect::<Vec<_>>(), times = ?racers.iter().map(|r| r.time).collect::<Vec<_>>(), "race over (0x417010) in the automatic replay: Race_finalize(2) again");
                if let Some(res) = self.results.as_mut() {
                    res.racers = racers;
                    res.order = order;
                }
            }
            return;
        }
        let (order, racers) = self.finalize_race();
        // the HUD tally's clean test, 0x427660: `Σ car+0xC98 <= [0x60C9C8 + i·4]` (`jle`)
        let mut clean = [true; 8];
        for (i, rec) in racers.iter().take(8).enumerate() {
            clean[i] = nfs_game::career::clean_race(self.start_damage[i], &rec.damage);
        }
        let mission = self.mission_outcome();
        tracing::info!(tick = self.tick, order = ?order, places = ?racers.iter().map(|r| r.place).collect::<Vec<_>>(), times = ?racers.iter().map(|r| r.time).collect::<Vec<_>>(), "race over (0x417010): result records");
        self.results = Some(RaceResults { racers, order, clean, mission, opened: 0, congrats: true });
        self.auto_replay_due = true;
    }

    /// Finishing order as driver indices (0 = player, 1.. = opponents in grid order), and each
    /// driver's time `car+0xCD8` in ticks — the result records once the race is over, else what
    /// `Race_finalize` would make of the race as it stands.
    pub(crate) fn finishing_order(&self) -> (Vec<usize>, [i32; 8]) {
        if let Some(r) = &self.results {
            return (r.order.clone(), r.times());
        }
        let (order, racers) = self.finalize_race();
        let mut times = [0i32; 8];
        for (i, r) in racers.iter().take(8).enumerate() {
            times[i] = r.time;
        }
        (order, times)
    }
}

/// The row index of a `Knockout.TrackNumber(<n><FIELD>)` reference, if that is what this is.
///
/// `locationsetup.lay` names the one accessor seven times with the row written into the argument
/// (`controls.md` §4.x), and `FEKnockoutTrackNumberAccessor`'s getter compares that row against
/// `Game.NumKOOpponents` at 0x4BA7CF before it looks anything up.
pub(crate) fn ko_row_of(src: &str) -> Option<u32> {
    let arg = src.split_once("Knockout.TrackNumber(")?.1;
    let digits: String = arg.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// HPosition's right column under `DERBY [0x657400] == 1` (0x42B2D5…0x42B325): the ten zones
/// `car+0x4E4` added into an int one at a time, each partial sum `_ftol`'d (truncated, 0x42B2F2…
/// 0x42B30B), then divided by 10 (the `0x66666667` multiply and `sar 2`, 0x42B30D…0x42B321).
pub(crate) fn derby_damage_total(zones: &[f32; 10]) -> i32 {
    zones.iter().fold(0i32, |acc, z| (acc as f64 + *z as f64) as i32) / 10
}

#[cfg(test)]
mod derby_tests {
    /// HPosition under `DERBY` (0x42B2D5): every partial sum truncated, then `/ 10`
    #[test]
    fn the_derby_column_truncates_each_step() {
        let mut z = [0.0f32; 10];
        assert_eq!(super::derby_damage_total(&z), 0);
        // ten zones at 0.5: the float sum would be 5, but each step truncates back to 0
        z = [0.5; 10];
        assert_eq!(super::derby_damage_total(&z), 0);
        z = [15.75; 10];
        assert_eq!(super::derby_damage_total(&z), 15);
        z = [100.0; 10];
        assert_eq!(super::derby_damage_total(&z), 100);
    }
}
