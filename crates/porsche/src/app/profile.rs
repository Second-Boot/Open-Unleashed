//! The saved profile and the track / car lists it selects: what the front end and the menu start from.

use crate::*;

impl App {
    /// Keep the `.sav`'s knockout and CD-player modules in step with the live state, so a career
    /// save carries them (`RE/notes/savegame.md` §4.6/§4.8). Run every frame: the original's
    /// modules save the globals as they are, so whatever the front end has is what a save writes.
    pub(crate) fn sync_save_side_state(&mut self) {
        // the block `[0x690410]` as it stands, verbatim
        let ko = self.knockout.as_ref().map(|k| nfs_game::save::KnockoutProgress::from_state(&k.state));
        let playlist = self.current_playlist();
        let settings = self.current_settings();
        if let Some(c) = self.career.as_mut() {
            // `[0x690410]` changes only when a series is started and run (0x4DDF30, 0x4DE060) or
            // cancelled (0x4DE040); with none here the loaded block stays as it was. `[0x6907D8]`
            // is set by the two draws themselves (`knockout_start`, the quick race's set-up).
            if let Some(k) = ko {
                c.side.knockout = Some(k);
            }
            // `OpponentCar.Car`: the menu's choice and the seven cars of the last draw (0x4F56C0);
            // with none drawn yet, what the profile loaded stays
            let sel = self.fe.menu.as_ref().or(self.fe.menu_template.as_ref()).map(|m| m.opp_car as u32);
            if let Some(sel) = sel
                && !self.opp_cars.is_empty()
            {
                c.side.opponent_cars = Some((sel, self.opp_cars.iter().map(|o| nfs_game::save::opponent_car_image(&c.tables, o)).collect()));
            }
            c.side.playlist = playlist;
            c.side.settings = settings;
            // `OwnedCars.Car`'s index as its Save 0x4F5B30 reads it (`vtbl[+0x28]`, always
            // within the garage: 0x4BC240 keeps it so)
            c.side.owned_car_selection = nfs_game::save::clamp_index(c.car_sel as u32, c.career.garage.len() as u32);
        }
        // `[0x691E7C]`, the event `Tournament.RemainingTracks` stands on (0x504F00)
        let next = self.career.as_ref().and_then(|c| {
            let t = c.current()?;
            c.career.remaining_event(t, self.fe.remaining_sel)
        });
        if let Some(c) = self.career.as_mut() {
            c.side.tournament.next_event = next.unwrap_or(0) as u32;
        }
    }

    /// **`XXDefXX`** — what 0x4DE880 (`SaveInfo("XXDefXX", −1)`, 0x4D3B76) writes at every
    /// start-up before a profile is loaded (`[0x5D9158]` is set, so 0x4D3B50 skips the load): the
    /// pristine career and the front end's start-up state. `CREATEPLAYERDONE`'s 0x4DE5B0 loads it
    /// to make the new profile (`savegame.md` §2.2, §2.3). The block `[0x690410]` and `[0x6907D8]`
    /// are the image's zeros then; `OpponentCar.Car` is left out (its constructor's draw 0x4F4EA8
    /// is not made — `App::opp_cars`), so a new profile keeps the seven cars of the last draw.
    pub(crate) fn startup_side(&self) -> nfs_game::save::SaveSide {
        nfs_game::save::SaveSide {
            knockout: Some(nfs_game::save::KnockoutProgress::from_state(&nfs_formats::sav::KnockoutState::default())),
            personalities: Some(Vec::new()),
            playlist: self.current_playlist(),
            settings: self.current_settings(),
            tournament: nfs_game::save::TournamentFe::default(),
            opponent_cars: None,
            owned_car_selection: 0,
        }
    }

    /// The CD player's selected tracks `[0x65B62C]`, as `CDPlayer` saves them (0x4BE340).
    pub(crate) fn current_playlist(&self) -> Vec<u32> {
        self.snd.audio_settings.playlist.iter().map(|i| *i as u32).collect()
    }

    /// What the settings screens hold, for the save's accessor block.
    fn current_settings(&self) -> nfs_game::save::SavedSettings {
        let a = &self.snd.audio_settings;
        nfs_game::save::SavedSettings {
            music: a.music_level,
            sfx: a.sfx_level,
            engine: a.engine_level,
            speech: a.speech_level,
            ambient: a.ambient_level,
            audio_quality: a.audio_quality,
            // the accessor's own index `[this+0x18]` (0x4BC460), which is the row the graphics
            // screen shows — *(until 2026-09-26 the port wrote the index of its own draw-distance
            // factor `detail`, which no screen sets)*
            view_distance: self.choice_index("Graphics.ViewDistance").map_or(0, |(i, _)| i as u8),
            laps: self.fe.menu.as_ref().map(|m| m.laps).unwrap_or(self.num_laps),
            opponents: self
                .fe.menu
                .as_ref()
                .map(|m| m.opponents)
                .unwrap_or(self.num_opponents),
            traffic: self.fe.menu.as_ref().map(|m| m.traffic).unwrap_or(0),
            ko_opponents: self.fe.menu.as_ref().map(|m| m.ko_opponents).unwrap_or(4),
            // the sixteen dead-zone values as the screen has them, for pad 0
            deadzone: self
                .fe.layout_preview
                .as_ref()
                .map(|l| l.deadzone_values().map(|p| [p[0] as u32, p[1] as u32]))
                .unwrap_or([[0, 0x80]; 8]),
            controls: self.saved_control_fields(),
            options: self.saved_option_indices(),
            // the peer-to-peer screen's list, in the profile's option block (`Comms.IPList`, 0x4B8D70)
            ip_list: Some(netplay::comms_ip_list()),
        }
    }

    /// Every multiple-choice and range accessor this port keeps, as the index its generic Save
    /// 0x4BC460 writes (`[this+0x18]`): a choice's row, a range's value less its minimum (0x4BA342).
    /// `OwnedCar.PackageNum` is left out: this port answers it from the garage car's own `+0x684`,
    /// which `PlayerInfo`'s car image carries.
    fn saved_option_indices(&self) -> Vec<(String, u32)> {
        let mut out: Vec<(String, u32)> = CHOICE_ACCESSORS
            .iter()
            .filter(|(n, ..)| *n != "OwnedCar.PackageNum")
            .filter_map(|(n, ..)| self.choice_index(n).map(|(i, _)| (n.to_string(), i as u32)))
            .collect();
        // `Camera.1 … Camera.4` and `Camera.Current` (`FEPositionableAccessor`, 0x4BC460): the car's
        // camera list and the slot the Options screen has selected — `savegame.md` §9 had them
        // "deliberately not ported" until the list took effect in a race (§4.89a)
        for (i, v) in self.fe.camera_list.iter().enumerate() {
            out.push((format!("Camera.{}", i + 1), *v as u32));
        }
        out.push(("Camera.Current".into(), self.fe.camera_current as u32));
        out.extend(RANGE_ACCESSORS.iter().filter_map(|(n, ..)| self.range_value(n).map(|(v, lo, _)| (n.to_string(), (v - lo) as u32))));
        // the knockout table `Track1..7.*` (0x4F9790), each its list index: `Laps` 0…7 (the count
        // less one, 0x4FC1BC), `Direction` 0…3, `Traffic` 0…3 — `Track1.Laps` / `.Traffic` are
        // `SavedSettings`' own fields — and the selected row `Knockout.TrackNumber` and
        // `LoopedTrack.Track`. What a series loaded half-way races its next rounds on.
        // A `Track` index is into `[0x691D04]` and `LoopedTrack.Track`'s into `[0x691D08]`, the
        // two lists 0x4FBC10 builds (`nfs_game::track_list`) as they stand when the profile is
        // saved: `nfs5.trk` order, the era unlocks by the chronicle year, no Skidpad. *(Until
        // 2026-09-26 it was the port's own list, the track directory's `.crp` names sorted.)*
        if let Some(m) = self.fe.menu.as_ref().or(self.fe.menu_template.as_ref()) {
            for i in 0..7 {
                let r = m.ko_row(i);
                out.push((format!("Track{}.Track", i + 1), r.track_idx as u32));
                out.push((format!("Track{}.Direction", i + 1), r.direction() as u32));
                if i > 0 {
                    out.push((format!("Track{}.Laps", i + 1), nfs_game::save::laps_to_index(r.laps)));
                    out.push((format!("Track{}.Traffic", i + 1), r.traffic));
                }
            }
            out.push(("Knockout.TrackNumber".into(), m.ko_sel as u32));
            // `Game.Difficutly`'s list index (0x4BC460, `[this+0x18]`)
            out.push(("Game.Difficutly".into(), u32::from(m.difficulty)));
            out.push(("LoopedTrack.Track".into(), m.looped_idx as u32));
        }
        out
    }

    /// **`LoadInfo`'s accessor pass and modules, onto the live state** (0x4DE9B0 at start-up
    /// 0x4D3B35 and on every profile load): the `Controller.*` bindings (0x51D080), every option
    /// accessor through 0x4BC480 → `SetSelection` 0x4BC240 (the index clamped into the list), the
    /// race-screen counts, the CD player's list (0x4BE170) and the knockout block (0x4DDD60), which
    /// replaces whatever series was running. A profile without a `.sav` keeps everything.
    pub(crate) fn apply_saved_profile(&mut self) {
        self.apply_saved_controls();
        let Some(c) = self.career.as_ref() else { return };
        let Some(s) = std::fs::read(c.profile.path(&c.save_dir)).ok().and_then(|d| nfs_formats::sav::SaveFile::parse(&d).ok()) else { return };
        let side = nfs_game::save::side_from_sav(&s);
        use nfs_game::save::clamp_index;
        // `Comms.IPList`'s Load 0x4B8E00: the strings, then the selection through `SetSelection` 0x4BC240
        if let Some((items, sel)) = side.settings.ip_list.clone() {
            let sel = clamp_index(sel, items.len() as u32);
            netplay::set_comms_ip_list(items, sel);
        }
        let mut display_seed: Vec<(usize, usize)> = Vec::new();
        for (name, d) in &side.settings.options {
            let name = name.as_str();
            if let Some((_, key, options)) = CHOICE_ACCESSORS.iter().find(|(n, ..)| *n == name).filter(|(n, ..)| *n != "OwnedCar.PackageNum") {
                let v = clamp_index(*d, options.len() as u32) as usize;
                // the Display options belong to the profile's HUD record, not to the accessor
                if let Some(slot) = Self::hud_cfg2_slot(key) {
                    display_seed.push((slot, v));
                } else {
                    self.choices.insert(Self::choice_slot(name, key).to_string(), v);
                }
            } else if let Some(slot) = name.strip_prefix("Camera.").and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=4).contains(n)) {
                // `Camera.1 … Camera.4` (`FEPositionableAccessor`, Save / Load 0x4BC460 / 0x4BC480): the
                // car's camera list as the Options screen left it (`controls.md` §4.89a). The original
                // loads the plain dword through `SetSelection`; nothing clamps a positionable accessor.
                self.fe.camera_list[slot - 1] = *d as i32;
            } else if name == "Camera.Current" {
                // the slot `NFS5.CAMERASELECT` last chose; the port indexes its four slots with it
                self.fe.camera_current = (*d as usize).min(3);
            } else if let Some((_, key, _, lo, hi)) = RANGE_ACCESSORS.iter().find(|(n, ..)| *n == name) {
                let v = clamp_index(*d, (hi - lo + 1) as u32) as i32 + lo;
                let a = &mut self.snd.audio_settings;
                match name {
                    "Audio.MusicLevel" => {
                        a.music_level = v as u8;
                        // `LoadAccessors` 0x4B7FD0 → the accessor's Load 0x4BC480 → `SetSelection`
                        // 0x4BC240, which stores the value and calls the commit callback `+0x10`
                        // (0x4BC2A5…0x4BC2B4) — for `Audio.MusicLevel` 0x4E47B0 (the ctor's
                        // seventh argument at 0x4E4928 → 0x4BC1F0 → 0x4B7B30's `+0x10`):
                        // `MusicSetVolume(MusicLevelToVolume(v))`
                        self.music_level_changed();
                    }
                    "Audio.SfxLevel" => a.sfx_level = v as u8,
                    "Audio.EngineLevel" => a.engine_level = v as u8,
                    "Audio.SpeechLevel" => a.speech_level = v as u8,
                    "Audio.AmbientLevel" => a.ambient_level = v as u8,
                    _ => {
                        self.choices.insert(key.to_string(), v as usize);
                    }
                }
            }
            // the four the race screens keep in the menu, each only when the file carries it
            let st = &side.settings;
            match name {
                "Audio.Quality" => self.snd.audio_settings.audio_quality = st.audio_quality,
                "Track1.Laps" | "Game.NumOpponents" | "Track1.Traffic" | "Game.NumKOOpponents" => {
                    if let Some(m) = self.fe.menu.as_mut() {
                        match name {
                            "Track1.Laps" => m.laps = st.laps,
                            "Game.NumOpponents" => m.opponents = st.opponents,
                            "Track1.Traffic" => m.traffic = st.traffic,
                            _ => m.ko_opponents = st.ko_opponents,
                        }
                        m.fit_ko();
                    }
                }
                // the rest of the knockout table `Track1..7.*`, its selected row and
                // `LoopedTrack.Track`, each clamped into its list by 0x4BC240
                _ => {
                    let Some(m) = self.fe.menu.as_mut() else { continue };
                    if let Some((row, field)) = name.strip_prefix("Track").and_then(|r| r.split_once('.')).and_then(|(n, f)| Some((n.parse::<usize>().ok()?.checked_sub(1).filter(|i| *i < 7)?, f))) {
                        let mut k = m.ko_row(row);
                        match field {
                            "Track" => k.track_idx = clamp_index(*d, m.tracks.len() as u32) as usize,
                            "Direction" => k.set_direction(clamp_index(*d, 4) as usize),
                            "Laps" if row > 0 => k.laps = nfs_game::save::laps_from_index(clamp_index(*d, 8)),
                            "Traffic" if row > 0 => k.traffic = clamp_index(*d, 4),
                            _ => continue,
                        }
                        m.set_ko_row(row, k);
                    } else if name == "Game.Difficutly" {
                        m.difficulty = clamp_index(*d, 3) as u8;
                    } else if name == "Knockout.TrackNumber" {
                        m.ko_sel = clamp_index(*d, 7) as usize;
                        m.fit_ko();
                    } else if name == "LoopedTrack.Track" {
                        m.looped_idx = clamp_index(*d, m.looped_tracks().len() as u32) as usize;
                    }
                }
            }
        }
        // the Display options of a profile with no `<Name>2.hud` of its own (saved before the HUD
        // option files were written) — port-only migration, [`App::hud_cfg_seed`]; with the file the
        // record is the master and the `.sav`'s values are the echo the original also keeps
        self.hud_cfg_ensure();
        if !self.hud_cfg.borrow().from_profile {
            for (slot, v) in display_seed {
                self.hud_cfg_seed(slot, v);
            }
        }
        // `OpponentCar.Car` (0x4F56F0): the choice, clamped into its five rows (0x4F4F90 →
        // 0x4BC240), and the seven cars as they were saved
        if let Some((sel, cars)) = side.opponent_cars.as_ref() {
            if let Some(m) = self.fe.menu.as_mut() {
                m.opp_car = clamp_index(*sel, 5) as u8;
            }
            self.opp_cars = cars.iter().map(nfs_game::save::opponent_car_of).collect();
        }
        // the CD player's list. *Port-only:* every `.sav` this port wrote before 2026-09-26 has
        // an empty `CDPlayer` module whatever the list was (the module was fed from a copy only a
        // race abort refreshed), so an empty list keeps `fe.txt`'s — the original would take it.
        if !side.playlist.is_empty() {
            self.set_cd_playlist(side.playlist.iter().map(|i| *i as usize).collect());
        }
        // the tournament the screen is on and its remaining-events row (0x5029F0)
        if let Some(row) = self.career.as_mut().and_then(|c| c.apply_tournament_fe(side.tournament)) {
            self.fe.remaining_sel = row;
        }
        // **the loaded block is the series now** (0x4DDD60 copies it over `[0x690410]`): a
        // series saved running goes on at its `race_index` with the rows and cars loaded above,
        // as the knockout screen's next launch finds it active (0x4D52CB). *(Until 2026-09-26
        // the port dropped it: it saved neither `OpponentCar.Car` nor `Track1..7.*`.)*
        self.knockout = side.knockout.as_ref().map(|k| KnockoutSeries { state: k.to_state(), message: String::new() });
        if let Some(c) = self.career.as_mut() {
            // `OwnedCars.Car` (0x4F5B70 → 0x4F0A30 → 0x4BC240): the saved index, clamped
            // against the garage's count
            c.car_sel = clamp_index(side.owned_car_selection, c.career.garage.len() as u32) as usize;
            c.side = side;
        }
        // 0x4DED10, the loader's last step but the gamma: 0x4FBC10 for the loaded profile's year —
        // **after** the accessor pass, so the `Track<n>.Track` / `LoopedTrack.Track` indices above
        // were clamped (0x4BC240) against the lists as they stood before the load
        self.rebuild_track_lists();
    }

    /// The chronicle year 0x4FBC10 gates the tracks on, `[0x5DB764]` (0x5032C0): the profile's
    /// career's, else the global's initial 0x79E.
    fn career_year(&self) -> i32 {
        self.career.as_ref().map_or(nfs_game::track_list::START_YEAR, |c| c.career.year as i32)
    }

    /// A track list's row text, 0x4FA560: `FEStrings[trk+4]` of the record the row holds (the
    /// port's rows are file names, so the record is found by its `trk+8`) — **as the table has
    /// it**, the 300 block's mixed case ("Côte d'Azur"): the row draw 0x4BC350 hands the pointer
    /// straight to 0x4DA350, and nothing on the way changes case (the only case function the font
    /// code calls, 0x53E270 from 0x53DBDD, swaps it for a glyph the font lacks, and every
    /// `FEData/Art/*.ffn` has both cases and the accented letters). *(Until 2026-09-26 the port
    /// upper-cased it.)* A file no record carries (never, in the original) shows as it is.
    pub(crate) fn track_full_name(&self, file: &str) -> String {
        self.fe
            .track_tables
            .tracks
            .iter()
            .find(|r| r.file.eq_ignore_ascii_case(file))
            .and_then(|r| self.fe.strings.get(r.name_string_id))
            .map_or_else(|| file.to_string(), str::to_string)
    }

    /// The track `Record.Track` stands for — its row text **0x4FB580** (slot 0x30), which ignores
    /// the row it is asked for and answers the target's own current row: `Game.GameType` 1 →
    /// `LoopedTrack.Track`, 2 → the `Track<n>.Track` `Knockout.TrackNumber`'s selection links to
    /// (0x4BAAF0: `[this+0x38][index]`), anything else → `Track1.Track`.
    pub(crate) fn record_track(&self) -> Option<String> {
        let m = self.fe.menu.as_ref()?;
        Some(
            match self.fe.game_type {
                1 => m.looped_track(),
                2 => m.ko_track(m.ko_sel),
                _ => m.track(),
            }
            .to_string(),
        )
    }

    /// `Record.Track` as a list: 0x4FB510 builds it through 0x4BC1F0 with **one** row
    /// (`[this+0x14] = 1`) and selects it (0x4BC240(0)); the row's text is 0x4FB580's, the
    /// target's current track. No shipped layout lists it (`trackrecords.lay` only watches it).
    pub(crate) fn record_track_rows(&self) -> Option<(Vec<ListRow>, Option<usize>)> {
        Some((vec![ListRow::Text(self.track_full_name(&self.record_track()?))], Some(0)))
    }

    /// **0x4FBC10**: rebuild `[0x691D04]` / `[0x691D08]` for the chronicle year and
    /// `Game.GameType` ([`nfs_game::track_list`]). The selections stay the indices they were; a
    /// read past a shorter list's end takes row 0 ([`Menu::track`], 0x4FA690).
    ///
    /// **Also rebuilds `cars` and `cars2`** (`GenericCar.Car`'s `[0x6919EC]`/`Menu::set_cars` and
    /// `GenericCar2.Car`'s `[0x6919F4]`/`Menu::set_cars2`) — the original's 0x4F3E40 is one
    /// function that rebuilds both the track lists (via its own 0x4FBC10 call, 0x4F3EA8) and all
    /// six car lists for the same chronicle year and `Game.GameType` in one pass, so this port
    /// hook, already every one of 0x4F3E40's own triggers by proxy (`modes.md`'s "0x4F3E40's
    /// seventeen call sites": start-up, `apply_saved_profile`, every `SETTYPE*`, race end), is
    /// where both are kept in step too.
    pub(crate) fn rebuild_track_lists(&mut self) {
        let year = self.career_year();
        let catalog = self.career.as_ref().map(|c| c.tables.catalog.clone()).unwrap_or_else(|| self.fe_car_catalog());
        if let Some(m) = self.fe.menu.as_mut().or(self.fe.menu_template.as_mut()) {
            m.set_track_lists(&self.fe.track_tables, year, self.fe.game_type);
            m.set_cars(&catalog, self.fe.game_type);
            m.set_cars2(&catalog, year);
        }
    }

    /// **0x4F3E40's head** (0x4F3E54…0x4F3EA8, before its 0x4FBC10 rebuild): `DealerCar.Car.slot11(0)`
    /// and `UsedCar.Car.slot11(0)` — the dealership and the used-car lot go back to their first car on
    /// every run (the race end 0x4EDD18, every `SETTYPE*`, the profile list's 0x5106D7, the tournament's
    /// filing 0x503AD0, …), and `DealerCar.Car`'s slot 11 sends both its colour accessors to row 0 when
    /// the car moved ([`DealerSel::slot11`]). `UsedCar.Car`'s is `FEOwnedCarAccessor`'s 0x4F5BF0: row 0
    /// of a non-empty list (0x4F5C0D → 0x4F0A30), else nothing. (the port kept both selections
    /// across a race and a mode change.)
    pub(crate) fn car_lists_head(&mut self) {
        Self::car_lists_head_of(&mut self.fe.dealer, &mut self.fe.usedcar_sel);
    }

    /// [`Self::car_lists_head`] on the two selections, for a caller that holds another borrow of `self`.
    pub(crate) fn car_lists_head_of(dealer: &mut DealerSel, used: &mut usize) {
        let before = (*dealer, *used);
        dealer.slot11(0, 1);
        *used = 0;
        if (dealer.car, dealer.paint, dealer.interior, *used) != (before.0.car, before.0.paint, before.0.interior, before.1) {
            tracing::info!(dealer_car = before.0.car, paint = before.0.paint, interior = before.0.interior, used = before.1, "DealerCar.Car / UsedCar.Car slot 11(0) (0x4F3E40's head)");
        }
    }

    /// The port's own catch for a chronicle year that moved without a rebuild: the layout front
    /// end rebuilds on its way back from a race (`fe_after_race`, 0x4D3B52 / 0x4EDD18), so this
    /// only finds work after a race the text menu started. Not in the original.
    pub(crate) fn sync_track_lists(&mut self) {
        let year = self.career_year();
        if self.fe.menu.as_ref().or(self.fe.menu_template.as_ref()).is_some_and(|m| m.tracks_year != year) {
            self.rebuild_track_lists();
        }
    }

    /// The nineteen `Controller.*` accessors, in the order the factory 0x51E030 builds them
    /// (the names it pushes at 0x51E04E…0x51E36D) — every row has a binding behind it here since
    /// `controls.md` §4.170, so every one is saved.
    const CONTROL_ACCESSORS: [&'static str; nfs_game::save::CONTROL_ROWS] = [
        "Controller.Accel",
        "Controller.Brake",
        "Controller.HandBrake",
        "Controller.SteerLeft",
        "Controller.SteerRight",
        "Controller.ShiftUp",
        "Controller.ShiftDown",
        "Controller.Horn",
        "Controller.LookBehind",
        "Controller.Lights",
        "Controller.ResetCar",
        "Controller.CameraView",
        "Controller.Pause",
        "Controller.LeftTurnSignal",
        "Controller.RightTurnSignal",
        "Controller.Hazards",
        "Controller.LookLeft",
        "Controller.LookRight",
        "Controller.LookForward",
    ];

    /// The player's bindings as the profile's `FEControllerInputAccessor`s save them (0x51D060).
    fn saved_control_fields(&self) -> [Option<(&'static str, [u32; 6])>; nfs_game::save::CONTROL_ROWS] {
        let mut out = [None; nfs_game::save::CONTROL_ROWS];
        for (k, name) in Self::CONTROL_ACCESSORS.iter().enumerate() {
            out[k] = accessors::controller_action(name)
                .and_then(nfs_input::binding_id)
                .and_then(|id| self.input.bindings.get(id))
                .map(|b| (*name, b.to_fields()));
        }
        out
    }

    /// Load the profile's saved `Controller.*` bindings over `fe.txt`'s defaults — the accessors'
    /// Load 0x51D080 restores the six fields, which the race's option stream then carries into
    /// `[0x5E9130 + id·4]` (`RE/notes/controls.md` §4.117). A profile with none keeps the defaults.
    fn apply_saved_controls(&mut self) {
        let Some(c) = self.career.as_ref() else { return };
        let Some(s) = std::fs::read(c.profile.path(&c.save_dir)).ok().and_then(|d| nfs_formats::sav::SaveFile::parse(&d).ok()) else { return };
        for (name, fields) in nfs_game::save::saved_controls(&s) {
            if let Some(id) = accessors::controller_action(&name).and_then(nfs_input::binding_id) {
                self.input.bindings.by_id.insert(id, nfs_input::bindings::Binding::from_fields(fields));
            }
        }
    }

    /// Which accessor `CARSETTINGS.CAR` forwards to — 0x4B88F0's switch on `Game.GameType`. The
    /// port has no numeric game type of its own, so it derives the three cases 0x4B88F0
    /// distinguishes and nothing finer: a factory-driver mission is **9**, a career event pending or
    /// a career loaded at all is the Evolution **4**, and everything else is **0**.
    pub(crate) fn car_settings_delegate(&self) -> &'static str {
        if self.mission.is_some() {
            "FactoryDriver.Car"
        } else if self.career.is_some() {
            "OwnedCars.Car"
        } else {
            "zMergedList.Car"
        }
    }
}
