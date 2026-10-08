//! The result pages, the mission outcome and the in-race HUD's keys and clicks.

use crate::*;

impl App {
    /// Mission over: evaluate the skill test (or the race position) and store the result.
    /// The mission's `(pass, time in ticks, cones)` as things stand — `HMissionSummary::evaluate`
    /// 0x4293F0 — with the time and cones 0x4EDF00 hands 0x51AEF0: the race time `result+0x204`
    /// (`car+0xCD8`, no cone penalty, 0x4EE15C) and `result+0x8C` (`car+0xB60`, every flagged
    /// smackable, 0x4EE14E).
    pub(crate) fn mission_outcome(&self) -> Option<(bool, u64, usize)> {
        let ms = self.mission.as_ref()?;
        if let Some(r) = &self.results {
            return r.mission;
        }
        let (order, times) = self.finishing_order();
        let position = order
            .iter()
            .position(|d| *d == 0)
            .map(|p| p + 1)
            .unwrap_or(8);
        let zones = self.vehicle.as_ref().map(|v| v.damage).unwrap_or([0.0; 10]);
        Some(match &self.skill {
            Some(sk) => {
                let (pass, _) = sk.evaluate(&ms.mission, position, self.hud_cones.changes, &zones);
                (pass, sk.time_ticks.unwrap_or(99_999), self.smackables.knocked_count())
            }
            None => {
                // race mission / solo lap race: position and total time
                // the same `HMissionSummary::evaluate` 0x4293F0 (`nfs_game::skill::evaluate_mission`)
                // on the race's `car+0xCD4` / `car+0xCD8`; the summary's cone count `+0x140` stays 0
                // without a skill test's cones
                let finished = self.race.as_ref().map(|r| r.finished).unwrap_or(false);
                let t = if finished { times[0] } else { 99_999 };
                let (pass, _) = nfs_game::skill::evaluate_mission(&ms.mission, position, t as i64, 0, &zones);
                // (`finished`: the port settles a race left unfinished as a fail — the original only
                // evaluates at the race end, when every racer has a result)
                (pass && finished, t.max(0) as u64, 0)
            }
        })
    }

    /// Fix the mission's verdict once, at the race end (`HMissionSummary::evaluate` 0x4293F0):
    /// `[0x657834] = pass`; one of the up-to-three pass or fail text ids — the non-zero entries of
    /// `[0x65780C…]` or `[0x657818…]` — chosen by `rand() % count` (0x429588…0x4295ED, after
    /// `srand(time(0))` in the constructor, 0x428C25); and the `win*` / `lose*` music cue
    /// (0x4AE4D0). The clock stands in for the CRT `rand()`.
    pub(crate) fn fix_mission_result(&mut self) {
        if self.mission_result.is_some() || !self.race.as_ref().is_some_and(|r| r.finished) {
            return;
        }
        let Some((pass, _, _)) = self.mission_outcome() else { return };
        let Some(ms) = self.mission.as_ref() else { return };
        let ids = if pass { ms.mission.success_string_ids } else { ms.mission.failure_string_ids };
        let ids: Vec<u32> = ids.iter().copied().filter(|t| *t != 0).collect();
        let roll = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos() as usize);
        let text = ids.get(roll % ids.len().max(1)).copied();
        if let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut()) {
            let rng = &mut self.fe_rng;
            m.play_pattern(a, if pass { "win*" } else { "lose*" }, 1, &mut || rng.next_u32().wrapping_add(crate::app::audio::tick_count_ms()));
        }
        self.mission_result = Some((pass, text));
    }

    /// The mission's result line as `HMissionSummary` prints it: the text id `sprintf`ed with the
    /// driver's name (`car+0x558 → +0x17C`, 0x429608…0x42962B) and broken at spaces into lines
    /// shorter than 50 characters (0x42965A…).
    pub(crate) fn mission_text_lines(&self) -> Vec<String> {
        let Some((_, Some(id))) = self.mission_result else { return Vec::new() };
        let text = self.fe.strings.hud(id).unwrap_or_default().replacen("%s", &self.player_name, 1);
        let mut lines: Vec<String> = Vec::new();
        for w in text.split(' ').filter(|w| !w.is_empty()) {
            match lines.last_mut() {
                Some(l) if l.len() + 1 + w.len() < 50 => {
                    l.push(' ');
                    l.push_str(w);
                }
                _ => lines.push(w.to_string()),
            }
        }
        lines
    }

    pub(crate) fn settle_mission(&mut self) {
        let Some(ms) = self.mission.clone() else {
            return;
        };
        if !self
            .career
            .as_ref()
            .is_some_and(|c| c.pending.as_ref().is_some_and(|p| p.mission.is_some()))
        {
            return;
        }
        self.fix_mission_result();
        let Some((pass, t, cones)) = self.mission_outcome() else { return };
        // 0x51AF45: the record takes `[0x657834]`, the verdict the summary fixed, not a new one
        let pass = self.mission_result.map_or(pass, |r| r.0);
        let text = self.mission_result.and_then(|r| r.1);
        let driver = self.player_name.clone();
        // the record's `+0xC98` ten zones, which 0x4EE12A…0x4EE14E sum for `+0xEC`
        let zones = self.result_damage().unwrap_or([0.0; 10]);
        if let Some(c) = self.career.as_mut() {
            c.finish_mission(ms.index, pass, t, cones, &zones, text, &driver);
            tracing::info!(
                mission = ms.mission.code,
                pass,
                time_s = t as f32 / 64.0,
                cones,
                message = c.message,
                "mission recorded"
            );
        }
    }

    /// The HUD toggles' bodies. `RaceFrame` reads the keys with the pause flag `[0x606A90]` clear
    /// (0x415982, the call site's `!self.paused`), and then **each toggle function tests the dialog
    /// list itself** — 0x4273B0 (F5), 0x426740 ("2"), 0x427550 (F6), 0x427360 (F7), 0x427600 /
    /// 0x427630, 0x4275D0 (F8) all begin `if [0x60C9C4] is not empty { return }` (read to the
    /// instruction 2026-09-30, batch's lead) — so returning early while a dialog is up
    /// (`hud_dialog_up`: the pause menu, a notice, the save question, a result page) is the same
    /// behaviour, not a divergence from 0x415982.
    pub(crate) fn hud_key(&mut self, a: Action) {
        if self.hud_dialog_up() {
            return;
        }
        let clock = self.hud_clock();
        let k = &mut self.hud_keys;
        match a {
            Action::ToggleHud => {
                if k.live {
                    k.live = false;
                    k.map_at_hide = k.map != 0;
                    if k.map_at_hide {
                        k.suspend_map();
                        self.map_slide = (2, clock);
                    }
                    k.mirror_at_hide = k.rearview;
                    if k.mirror_at_hide {
                        k.rearview = false;
                        self.mirror_slide = (2, clock);
                    }
                    for s in [&mut self.tray_slide, &mut self.time_trays] {
                        if s.0 != 2 {
                            *s = (2, clock);
                        }
                    }
                } else {
                    k.live = true;
                    if k.map_at_hide {
                        k.restore_map();
                        self.map_slide = (1, clock);
                    }
                    if k.mirror_at_hide {
                        k.rearview = true;
                        self.mirror_slide = (1, clock);
                    }
                    self.tray_slide = (1, clock);
                    self.time_trays = (1, clock);
                }
            }
            Action::ToggleRearView => {
                if k.rearview {
                    k.rearview = false;
                    self.mirror_slide = (2, clock);
                } else if k.live {
                    k.rearview = true;
                    self.mirror_slide = (1, clock);
                }
            }
            Action::ToggleMap => {
                if k.map != 0 {
                    k.suspend_map();
                    self.map_slide = (2, clock);
                } else if k.live {
                    k.restore_map();
                    self.map_slide = (1, clock);
                }
            }
            Action::CycleMap => {
                k.map = match k.map {
                    3 => 2,
                    1 => 3,
                    _ => 1,
                };
            }
            Action::HudTransparency => k.boxes = !k.boxes,
            _ => {}
        }
        tracing::info!(?a, keys = ?self.hud_keys, "HUD key");
    }

    /// Which billboard is up this race frame (`billboard_key`): the pause menu (1), its quit
    /// question (2), the results page open last frame (10 + page) or none; a billboard opening or
    /// closing plays sound 4 (0x41ADD0).
    pub(crate) fn billboard_key_frame(&mut self) {
        let key = if self.paused {
            if self.pause_confirm.is_some() { 2 } else { 1 }
        } else {
            self.results_billboard.map_or(0, |p| 10 + p)
        };
        if key != self.billboard_key {
            self.billboard_key = key;
            self.fe_sound(4);
        }
    }

    pub(crate) fn hud_dialog_up(&self) -> bool {
        self.paused
            || self.net_notice.is_some()
            || self.replay_save.up
            || self.billboard_key != 0
            || (self.results.is_some() && self.result_pages().get(self.results_page as usize).is_some())
    }

    /// The post-race pages this race queues, in order.
    pub(crate) fn result_pages(&self) -> Vec<ResultPage> {
        // `DERBY` (the Smash Up cheat): the queue is `HCongrats` alone — 0x426DB7 jumps to 0x427345 with
        // nothing else queued: no summary, no records, no tournament chain (`hud.md`, the DERBY readers 0x4266A0 / 0x426D07 / 0x426DB7)
        if self.cheat_bits & (1 << 7) != 0 {
            return Vec::new();
        }
        if self.mission.is_some() {
            return vec![ResultPage::MissionSummary];
        }
        // the three test drives (game types 6, 7, 8) queue nothing at all (0x426C98…0x426CB2)
        if self.fe.test_drive.is_some() {
            return Vec::new();
        }
        if let Some(id) = self.finale_notice() {
            return vec![ResultPage::Notice(id)];
        }
        let mut pages = vec![ResultPage::Summary];
        if let Some((rank, last, _)) = self.tournament_rank() {
            if last && rank <= 3 {
                pages.push(ResultPage::TournamentPlace(rank));
            }
            pages.push(ResultPage::Standings);
            if last && rank > 3 {
                pages.push(ResultPage::Notice(18160));
            }
        }
        // game type 3 skips the records page (0x4272F7 `cmp [0x6573E8], ebp; je 0x427345`, `ebp = 3` from
        // 0x426CCF): the queue goes on to the summary's timeout and nothing else
        if self.hud_records.is_some() && self.block_game_type() != Some(3) {
            pages.push(ResultPage::Records);
        }
        pages
    }

    /// The player's rank in the standings with this race in them (1-based), and the front end's
    /// `(TOURNAMENTLASTRACE, TOURNAMENTENDOFERA)` for this event — `None` outside a tournament.
    fn tournament_rank(&self) -> Option<(usize, bool, bool)> {
        let (t, event) = self.tournament_event()?;
        let c = self.career.as_ref()?;
        let (order, times) = self.finishing_order();
        let clean = self.clean_racers();
        let st = c.career.standings_after(&t, event, &order, &times, &clean);
        let rank = st.iter().position(|(d, ..)| *d == 0)? + 1;
        let (last, end_of_era) = c.career.tournament_flags(&c.tables, &t);
        Some((rank, last, end_of_era))
    }

    /// Speech as the page after the summary opens in a tournament. On the last race the queue first
    /// shows `HCongrats` with the tournament place when it is 1–3 (0x426E5D), whose update says
    /// **kTourPlaced** `0x4A6500(place − 1)` — `Tour-Cwon`/`Cplaced2nd`/`Cplaced3rd` or
    /// `Tour-won`/`placed2nd`/`placed3rd` by `rand & 1` — and then **kNextTournament**
    /// (`Bon-moveon` / `Bon-readytochalnexttour`) unless `TOURNAMENTENDOFERA` (0x41D10D…0x41D140).
    /// On any other race the standings say **kTournamentLeader** `Bon-youretournleader` when the
    /// player's row is the first (0x4A64D0, `sfx.md` §7.2, MEDIUM). The `HCongrats` picture itself
    /// is not drawn by this port.
    /// An era finale (game type 10) queues **only** an `HInfo` (0x4270EF…0x42726E): no
    /// `HCongrats`, no summary, no records. The text is FE 18153 / 18154 / 18155 "you have won the
    /// Classic / Golden / Modern Era bonus car" by `TOURNAMENTERA [0x6577E0]` when the player's
    /// place `car+0xCD4` is 1, otherwise FE 18156 "Sorry, you did not win the bonus car". This port
    /// showed the ordinary summary and records pages.
    fn finale_notice(&self) -> Option<u32> {
        let c = self.career.as_ref()?;
        c.pending.as_ref().filter(|p| p.finale)?;
        // `TOURNAMENTERA` 0xBB is the launch's `0x504800()` (`modes.md` §1)
        let era = match c.career.completed_era(&c.tables)? {
            nfs_formats::career::Era::Classic => 0,
            nfs_formats::career::Era::Golden => 1,
            nfs_formats::career::Era::Modern => 2,
        };
        let won = self.finishing_order().0.first() == Some(&0);
        Some(if won { 18153 + era } else { 18156 })
    }

    /// The page on top closes and the queue shows the next one (0x4261ED, or the page's own
    /// timeout), with the speech the next one opens with. Past the last page nothing is shown.
    /// Close the page on top: a billboard first **shrinks** (state 2, 0x41A814…0x41A839: `h/5` a
    /// frame down to the 40-px floor, then state 0), and only then does the queue show the next
    /// page ([`App::advance_results_page`]). The tournament `HCongrats` just goes.
    pub(crate) fn close_results_page(&mut self) {
        let page = self.result_pages().get(self.results_page as usize).copied();
        if matches!(page, Some(ResultPage::TournamentPlace(_)) | None) {
            self.advance_results_page();
        } else if self.page_closing.is_none() {
            self.page_closing = Some(0);
        }
    }

    pub(crate) fn advance_results_page(&mut self) {
        self.page_closing = None;
        self.results_page += 1;
        self.results_page_tick = self.tick;
        match self.result_pages().get(self.results_page as usize) {
            // the tournament `HCongrats` speaks as it opens; with no top-three page the
            // standings may say "tournament leader"
            Some(ResultPage::TournamentPlace(_)) => self.tournament_speech(),
            Some(ResultPage::Standings)
                if !matches!(
                    self.result_pages().get(self.results_page as usize - 1),
                    Some(ResultPage::TournamentPlace(_))
                ) =>
            {
                self.tournament_speech()
            }
            _ => {}
        }
    }

    /// Whether the page on top closes by itself, and after how many ticks: an `HBillBoard` built
    /// with its timeout byte `+0x118` closes once `[0x606AC4]` is more than **0x280** past the tick
    /// it opened at (0x41A7B6…0x41A7E3) — the summary, the standings, the records and `HInfo`
    /// pass 1 as the constructor's seventh argument (calls 0x42BD08, 0x431897, 0x433874, 0x428328); the
    /// mission summary and the pause menu pass 0. The tournament `HCongrats` closes itself past
    /// 0xC0 ticks (0x41DD40).
    pub(crate) fn page_timeout(page: ResultPage, net: bool) -> Option<u64> {
        match page {
            // game type 3 clears the summary's timeout with 0x41B130(0) (0x4272BF `cmp [0x6573E8], ebp`,
            // 0x4272C9): it stays until Space; every other type passes 1 (0x4272E5)
            ResultPage::Summary if net => None,
            ResultPage::Summary | ResultPage::Standings | ResultPage::Records | ResultPage::Notice(_) => Some(0x280),
            ResultPage::TournamentPlace(_) => Some(0xC0),
            ResultPage::MissionSummary | ResultPage::Closed => None,
        }
    }

    /// kTourPlaced 0x4A6500(rank − 1), then kNextTournament 0x4A6490 unless the era ends;
    /// kTournamentLeader 0x4A64D0 — the variants drawn from 0x538040 (until 2026-09-26 from the
    /// tick).
    fn tournament_speech(&mut self) {
        let Some((rank, last, end_of_era)) = self.tournament_rank() else { return };
        let (Some(_), Some(sp)) = (&self.snd.audio, &self.snd.speech) else { return };
        use nfs_audio::speech::phrase;
        let rng = &mut self.fe_rng;
        let mut draw = || rng.next_u32();
        if last && rank <= 3 {
            sp.say_id(phrase::tour_placed(rank as u32 - 1, &mut draw));
            if !end_of_era {
                sp.say_id(phrase::next_tournament(&mut draw));
            }
        } else if !last && rank == 1 {
            sp.say_id(phrase::TOURNAMENT_LEADER);
        }
    }

    /// A left-button release over an in-race dialog's button. The HUD takes the window's
    /// `WM_LBUTTONUP` as event **1** and `WM_LBUTTONDOWN` as 2 (0x434EB5…0x434EEC → 0x435200 /
    /// 0x435180, while `[0x5CD1E4]`, set for the race, 0x427C50); the pump 0x425EF0 hands it to the top
    /// dialog's slot 9, which gives it to each of its buttons, 0x41B300: event 1 with `+0x100` (the
    /// hover the last draw found) sets `+0x101` and plays sound 9 (0x41B30B…0x41B31F), and the slot
    /// answers by the first button so marked — `HPauseMenu` 0x42A6A0: 2 continue, 3 race again, 0x1A
    /// the quit question; `HYesNoOverlay` 0x437770: 0x1C yes, 0x1D no; `HMissionSummary` 0x429390:
    /// event 3, the restart, only for a failed mission (0x4293BB). The answers do what Enter on the
    /// port's keyboard selection does.
    pub(crate) fn hud_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some((dialog, k)) = self.hud_hover else { return false };
        match dialog {
            0 if self.paused && self.pause_confirm.is_none() => {
                self.fe_sound(9);
                match k {
                    0 => self.paused = false,
                    1 => {
                        self.paused = false;
                        self.restart_race(event_loop);
                    }
                    _ => self.pause_confirm = Some(1),
                }
            }
            1 if self.paused && self.pause_confirm.is_some() => {
                self.fe_sound(9);
                if k == 0 {
                    self.paused = false;
                    self.pause_confirm = None;
                    if !self.leave_race() {
                        event_loop.exit();
                    }
                } else {
                    self.pause_confirm = None;
                }
            }
            3 if !self.paused && self.replay_save.up && !self.replay_save.overwrite => {
                self.fe_sound(9);
                self.replay_save_click(k);
            }
            4 if !self.paused && self.replay_save.up && self.replay_save.overwrite => {
                self.fe_sound(9);
                self.replay_overwrite_click(k);
            }
            2 if !self.paused
                && self.results.is_some()
                && self.result_pages().get(self.results_page as usize) == Some(&ResultPage::MissionSummary) =>
            {
                self.fe_sound(9);
                if self.mission_result.is_some_and(|(passed, _)| !passed) {
                    self.restart_race(event_loop);
                }
            }
            _ => return false,
        }
        self.hud_hover = None;
        true
    }

    /// The tournament and event this race is, when it is one (game type 4 — not a mission, not a
    /// finale).
    pub(crate) fn tournament_event(&self) -> Option<(nfs_formats::career::Tournament, usize)> {
        let c = self.career.as_ref()?;
        let p = c.pending.as_ref()?;
        if p.mission.is_some() || p.finale {
            return None;
        }
        Some((c.tables.tournament(p.tournament)?.clone(), p.event))
    }

    /// Which racers took no new body damage (`career::clean_race`), by driver index.
    /// The player's damage zones the race end parks the car with: the record's `+0xC98` once the
    /// race is over — the car the pages sit on is the replay's — else the live car's (a race left
    /// before race over, whose `0x416AC0(1)` at 0x415784 copies the live zones).
    pub(crate) fn result_damage(&self) -> Option<[f32; 10]> {
        self.results.as_ref().and_then(|r| r.racers.first()).map(|r| r.damage).or_else(|| self.vehicle.as_ref().map(|v| v.damage))
    }

    /// **The pages are built** — `0x426A80(1)`'s `true` arm at the replay's first `RaceFrame`
    /// (0x41533A), once (`[0x60CA70]`), when the player has a place `+0xCD4` or it is a mission
    /// (0x426C25…0x426C34):
    ///
    /// * `HTrackRecords` slot 2, **0x433A20** (0x426C4F): the player's `+0xCD8` / `+0xCFC` into the
    ///   race's copy of the track's records (`TRACKRECORDS` / `LAPRECORDS`, which 0x4ECB70 filled
    ///   before the race), and the record-speech flags;
    /// * `0x426C90(0)` (0x426C5B), whose queue opens with the race's **`HCongrats`** for game types
    ///   0…5 (0x426CCD…0x426CEC); its first draw (0x41D050, `+0xB1` once) says the placing and then,
    ///   for the race's own one (`+0xB0`) with more than one car `[0x5E8AE8]`, plays
    ///   **`MusicPlay(place == 1 ? "win*" : "lose*", 1)`** (0x41D1CB…0x41D1FB) — the finish jingle —
    ///   whatever `REPLAY` is; type 5 draws nothing (0x41D080). The queue's other speech, the era
    ///   finale's kWon996Turbo (0x42722F…0x427245), is skipped while `REPLAY` is 2 — i.e. here,
    ///   the only place a single-player queue is built — so it is not said.
    ///
    /// *(Until 2026-09-26 the port played the jingle and the finale's speech at the player's live
    /// finish, and built the records copy there; until 2026-09-27 it said `HCongrats`' placing
    /// speech at the live finish, in a playback too.)*
    pub(crate) fn open_result_pages(&mut self) {
        let Some(res) = self.results.as_ref() else { return };
        let Some(me) = res.racers.first() else { return };
        let (place, total, best) = (me.place, me.time as u32, me.best_lap as u32);
        // 0x426D07: under `DERBY` `HCongrats` takes the place `car+0xB50 == 3 ? 8 : 1` of `[0x65741C]`'s car —
        // knocked out, or the winner — and not the ranking's
        let place = if self.cheat_bits & (1 << 7) != 0 { if self.player_eliminated { 8 } else { 1 } } else { place };
        if place == 0 && self.mission.is_none() {
            return;
        }
        // 0x433A20 — not shown for a mission (its queue is the mission summary alone)
        if self.hud_records.is_none()
            && self.mission.is_none()
            && let Some(t) = self.records.track(&self.track.name)
        {
            let circuit = t.circuit;
            self.hud_records = Some(nfs_game::records::hud_records(&t.entries, !circuit, &self.player_name, &self.player_car_family(), total, best));
            self.records_spoken = false;
        }
        // `HCongrats` heads the queue for game types 0…4 that draw it; the finales, the mission and
        // the test drives have none
        let game_type = self.block_game_type().unwrap_or(0);
        let finale = self.career.as_ref().and_then(|c| c.pending.as_ref()).is_some_and(|p| p.finale);
        let n_cars = self.road.opponents.len() + 1;
        let congrats = game_type <= 4 && self.mission.is_none() && !finale && self.fe.test_drive.is_none();
        // **The placing speech**, the first draw's `+0xB0` arm (0x41D14A…0x41D1C6): by
        // `FEGAME_TYPE` through the byte table 0x41D3A4 (0 and 3…9 → 0x41D163, 1 → 0x41D1B6, 2 →
        // 0x41D18E; above 9 nothing), with `+0xAC` the place `[0x60CA6C]` (the player's `+0xCD4`)
        // and `[0x5E8AE8]` the ranking table's count. No `REPLAY` test on the way — it is said over
        // the automatic replay, at the page's first draw, which the port takes as the queue's build.
        if congrats
            && place >= 1
            && let (Some(_), Some(sp)) = (&self.snd.audio, &self.snd.speech)
        {
            use nfs_audio::speech::phrase;
            let fegame_type = self.race_fegame_type();
            let rng = &mut self.fe_rng;
            let mut draw = || rng.next_u32();
            let count = n_cars as u32;
            match fegame_type {
                // 0x41D1B6: place 1 → kKnockoutWon 0x4A62E0, else kKnockoutEliminated 0x4A6350
                1 => sp.say_id(if place == 1 { phrase::knockout_won(&mut draw) } else { phrase::knockout_eliminated(&mut draw) }),
                // 0x41D18E: place 1 of two → won; place < count → kKnockoutQualify 0x4A6320; else
                // eliminated (`jge`)
                2 => sp.say_id(if place == 1 && count == 2 {
                    phrase::knockout_won(&mut draw)
                } else if place < count {
                    phrase::KNOCKOUT_QUALIFY
                } else {
                    phrase::knockout_eliminated(&mut draw)
                }),
                // 0x41D163: kRacePlacing 0x4A6430(place − 1), then kLostRace 0x4A62A0 when the
                // place is the count and the count above 1
                0 | 3..=9 => {
                    sp.say_id(phrase::race_placing(place - 1, &mut draw));
                    if place == count && count > 1 {
                        sp.say_id(phrase::lost_race(&mut draw));
                    }
                }
                _ => {}
            }
            tracing::info!(place, "HCongrats' placing speech (0x41D16B)");
        }
        if congrats
            && n_cars > 1
            && let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut())
        {
            tracing::info!(place, "HCongrats' jingle (0x41D1EF)");
            let rng = &mut self.fe_rng;
            m.play_pattern(a, if place == 1 { "win*" } else { "lose*" }, 1, &mut || rng.next_u32().wrapping_add(crate::app::audio::tick_count_ms()));
        }
    }
}

pub(crate) fn format_lap_time(ticks: u32) -> String {
    let secs = ticks / 64;
    format!("{:02}:{:02}.{:02}", secs / 60, secs % 60, (ticks & 63) * 100 / 64)
}

/// A number with thousands separators, the way 0x4DBF80 prints one: it splits off `n % 1000` and
/// `n / 1000 % 1000` (the 0x3E8 and 0xF4240 divisors at 0x4DBFA3 and 0x4DBFAD) and joins the groups
/// with commas.
pub(crate) fn with_thousands(n: i64) -> String {
    let neg = n < 0;
    let mut d = n.unsigned_abs().to_string();
    let mut out = String::new();
    while d.len() > 3 {
        let at = d.len() - 3;
        out.insert_str(0, &format!(",{}", &d[at..]));
        d.truncate(at);
    }
    out.insert_str(0, &d);
    if neg {
        out.insert(0, '-');
    }
    out
}

#[cfg(test)]
mod race_number_tests {
    use super::App;
    use nfs_assets::GamePaths;

    /// `.RACENUMBER` 0x24D (`car-model.md` §5.3): `nfs_game::opponents::fill` has drawn the decal
    /// since 1149de15 (0x4F517B `rand()%100 > 80` → 0x4F518D `rand()%100 − 1`, on the same `CRand`
    /// as the package roll, package test first). What was missing was carrying that draw to the
    /// plate — `App::opponent_of` is the hand-off from `OpponentPick::decal` to
    /// `CareerOpponent::decal`, which `page_text` now reads for an opponent slot instead of the
    /// player-only `fe.txt` registry lookup. This locks the hand-off for whatever the draw gives,
    /// including the −1 default and a two-digit `0…98` value.
    #[test]
    fn opponent_of_carries_the_decal_draw_to_the_plate() {
        let Ok(paths) = GamePaths::locate() else {
            eprintln!("skipping: original game data not found");
            return;
        };
        let catalog = nfs_formats::fedata::parse_car_catalog(&std::fs::read(paths.fe_data().join("Data").join("nfs5.car")).unwrap()).unwrap();
        let strings = nfs_formats::locale::Strings::default();
        let per_table: Vec<nfs_formats::career::Personality> = Vec::new();
        for decal in [-1, 0, 37, 98] {
            let pick = nfs_game::opponents::OpponentPick { car: 0, paint_set: 0, interior: 0, package: 0, decal };
            let o = App::opponent_of(&strings, &per_table, &catalog, &pick, None);
            assert_eq!(o.decal, decal);
        }
    }
}
