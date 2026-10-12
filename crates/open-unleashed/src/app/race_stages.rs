//! The stages of `start_race`, split out as plain moves (no change of order or content): the race audio load, the grid and the cars, and the sprite / particle pools. The `race_loading_frame` steps stay in `start_race`, between them.

use crate::*;
use nfs_game::world::CarUpload;

impl App {
    /// Stage "audio" — steps 6 → 7 of the load: the output, the engine, the ambience, the banks, the speech and the race music.
    pub(super) fn race_stage_audio(&mut self, playing: bool, world: &mut TrackWorld) {
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new()
                .map_err(|e| tracing::warn!(error = %e, "audio disabled"))
                .ok();
        }
        // the voice budget of the sound's open 0x4A66B0 for the current `AUDIOQUALITY` — re-given on
        // every use, as 0x4A6A00 reopens the sound when the setting changed
        if let Some(a) = &self.snd.audio {
            a.set_audio_quality(self.snd.audio_settings.audio_quality);
        }
        // the race audio init 0x4A8660 loads `replay.spl` over the mix block for a playback and
        // `game.spl` otherwise (0x4A86A1); the players the port keeps across races took the old
        // block's trims, so they are rebuilt (the engine and the ambience are, every race)
        if nfs_formats::spl::select_mix(playing) {
            self.snd.sfx = None;
            self.snd.ai_engines = None;
            if let Some(m) = self.snd.music.as_mut() {
                m.set_trim(nfs_formats::spl::GameSpl::load(&self.paths.game_data().join("Sounds")).music());
            }
        }
        // `<track>_audio.scn`: the ambience elements and the type-4 triggers that flag them.
        // `[0x628168]` would hold every `*.scn` of the track (0x46CCB0) in the order they load;
        // the port loads this one scene, so its indices are the combined list's and a trigger's
        // file-local `+0xD8` (re-based by the parser 0x46C417…0x46C46A) indexes it directly.
        let audio_scene = match self.track.load_scene("_audio.scn") {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "audio scene unreadable");
                None
            }
        };
        self.snd.audio_triggers = audio_scene
            .as_ref()
            .map(|s| s.elements.iter().filter_map(|e| nfs_game::cameras::CameraTrigger::from_element(e, 4, 9)).collect())
            .unwrap_or_default();
        if !self.snd.audio_triggers.is_empty() {
            tracing::info!(triggers = self.snd.audio_triggers.len(), "type-4 audio triggers loaded");
        }
        if let Some(a) = &self.snd.audio {
            self.snd.engine_sound = nfs_audio::engine::EngineSound::load(
                a,
                &self.paths.game_data().join("Sounds"),
                &self.snd.sound_set,
                &self.sim_name,
            )
            .map_err(|e| tracing::warn!(error = %e, "engine sound unavailable"))
            .ok();
            // track ambience (`engine-audio.md` §7): the `<track>_audio.scn` emitters over `track.viv`
            // — loaded without a scene too (no elements), since the sky's thunder plays its
            // `track.viv` patches through it whatever the track's scene (Industrial has none)
            let scenes: Vec<_> = audio_scene.iter().collect();
            self.snd.ambience = nfs_audio::ambience::Ambience::load(
                &self.paths.game_data().join("Sounds"),
                &scenes,
                self.snd.audio_settings.ambient_level,
            )
            .map_err(|e| tracing::warn!(error = %e, "ambience unavailable"))
            .ok();
            // The autobahn train's ambient sound (`animdefs.txt SECTION:Audio{tag=TRAN}`,
            // `engine-audio.md` §7.4, `COVERAGE-2026-09-26.md` §14.38): `world.animated_sound` is
            // `Some` only on the one track that carries it. Registered once here; the per-tick
            // position/velocity sync is at the ambience update call site below.
            if let (Some(amb), Some(sound)) = (self.snd.ambience.as_mut(), world.animated_sound.as_mut()) {
                sound.ambience_index = Some(amb.add_animated(&sound.def, sound.label.clone()));
                tracing::info!(label = sound.label, patch = sound.def.patch_id, "autobahn train ambience registered");
            }
            if self.snd.ai_engines.is_none() {
                self.snd.ai_engines =
                    nfs_audio::ai_engine::AiEngines::load(&self.paths.game_data().join("Sounds"))
                        .map_err(|e| tracing::warn!(error = %e, "AI engine banks unavailable"))
                        .ok();
            }
            if self.snd.sfx.is_none() {
                self.snd.sfx =
                    nfs_audio::sfx::CollisionSfx::load(&self.paths.game_data().join("Sounds"))
                        .map_err(|e| tracing::warn!(error = %e, "collision sfx unavailable"))
                        .ok();
            }
            if self.snd.speech.is_none() {
                self.snd.speech =
                    nfs_audio::speech::Speech::load(&self.paths.game_data().join("Speech"))
                        .map_err(|e| tracing::warn!(error = %e, "speech unavailable"))
                        .ok();
            }
            // `AudioLoadTrack` 0x4A88A0 flushes the speech on every pass (0x4A88D0, 0x4AFD30)
            if let Some(sp) = self.snd.speech.as_ref() {
                sp.flush(a);
            }
            self.snd.countdown_said = 0;
            // the front end is over: `[this+0x10C]` emptied (0x4D5273), `MusicStop(1500)`
            // (0x4D5295) and `[0x65B928]` cleared (0x4D559A) — the next entry starts afresh
            self.fe.music_current.clear();
            self.fe.list_current.clear();
            if self.autotest.is_none() && self.screenshot.is_none() {
                // `AudioLoadTrack` 0x4A88A0: `MusicStop(1500)`, the saved playlist, then
                // `MusicPlay(NULL, 0)` — any entry of it but the current one. In a playback
                // (`[0x5E99F4] >= 2`, 0x4A8A4A…0x4A8A56 `jge 0x4A8A75`) the playlist and the play
                // are skipped: the music stops and stays stopped (`replay.md` §9). *(Until the
                // audit of 2026-09-26 a playback started the race music.)*
                if self.snd.music.is_none() {
                    self.snd.music = nfs_audio::music::MusicPlayer::new(
                        &self.paths.game_data().join("Music"),
                        &self.paths.game_data().join("Sounds"),
                        &self.snd.audio_settings,
                    )
                    .map_err(|e| tracing::warn!(error = %e, "music unavailable"))
                    .ok();
                }
                // the shuffle flag of a race is `RANDOMPLAYLIST` `[0x657A5C]` (0x4AE639),
                // which the launch's accessor dump sets from `CDPlayer.Options`' selection (id 0x43;
                // rows 1856 "continuous" = 0, 1855 "random order" = 1), not the file's value
                let shuffle = self.choice_index(choice::CDPLAYER_OPTIONS).is_some_and(|(i, _)| i != 0);
                if let Some(m) = self.snd.music.as_mut() {
                    m.set_shuffle(shuffle);
                    m.set_front_end(false);
                    m.stop_fade(a, 1500);
                    if !playing {
                        // the pick's `0x538040() + GetTickCount()` (0x4AE739…0x4AE746): the shared
                        // generator `fe_rng`, which the session's load reseeded from `time(0)`
                        // (0x428C2D), and the clock (`tick_count_ms`, APPROXIMATION). APPROXIMATION
                        // (order): the original's pick (the pass, `AudioLoadTrack` 0x4A88A0) draws
                        // before the pass remakes the sky (0x4690A0 → 0x45DBD0, the jitter
                        // 0x45F3C0); the port's one sky was drawn at the load (`Sky::upload`), so
                        // here the pick comes after the jitter's draws — same count, swapped order
                        // on a clock-seeded state
                        let rng = &mut self.fe_rng;
                        m.start_race(a, &mut || rng.next_u32().wrapping_add(crate::app::audio::tick_count_ms()));
                    }
                }
            }
        }
    }

    /// Stage "cars" — steps 8 → 9: the grid pose, the player's seat, the opponents, the police and the traffic; hands back the slice table, the start frame and the player's grid slot.
    pub(super) fn race_stage_grid(&mut self, renderer: &mut Renderer, world: &mut TrackWorld, car_env: nfs_game::world::CarEnv, driver_rand: &mut nfs_game::movie_overlay::CRand, factory_driver: bool) -> (Option<SliceTable>, glam::Mat3, Option<(usize, Vec3)>) {
        // start pose from the slice-based grid rule (pole position, single racer)
        // `<track>.jnc` carries the branch weights the AI's junction choice reads (`race-logic.md` §10)
        let table = SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok().map(|mut t| {
            if let Some(p) = GamePaths::find_file(&self.track.dir, &format!("{}.jnc", self.track.name))
                && let Ok(d) = std::fs::read(&p)
                && !t.apply_jnc(&d)
            {
                tracing::warn!(path = %p.display(), "junction weights rejected: the file does not match the node count");
            }
            // `SliceTable::frac`'s backwards arm reads LINEAR_TRACK (0x4162E4)
            t.linear = self.linear_track();
            if self.backwards { t.reversed() } else { t }
        });
        let start_scene = if self.backwards {
            self.track
                .start_backward
                .as_ref()
                .or(self.track.start_forward.as_ref())
        } else {
            self.track.start_forward.as_ref()
        };
        self.player_eliminated = false;
        self.net_time_sent = false;
        self.net_pad_buttons = 0;
        // the port's own counters for the autotest log (the opponents' start at 0 with their cars)
        // and the autotest bot's stuck count
        self.wall_hits = 0;
        self.resets = 0;
        self.stuck_ticks = 0;
        // the HUD's init builds `HReplaySave` afresh (0x424EA4)
        self.replay_save = ReplaySave::default();
        self.hud_records = None;
        self.results_page = 0;
        self.page_seen = None;
        self.page_open_tick = None;
        self.results = None;
        self.mission_result = None;
        // 0x426B20…0x426B3B: `[0x60CA04] = 0`, `HTime` reset (0x431850), `HMissionSummary+0x140 = 0`
        self.hud_cones = Default::default();
        self.cone_flash = 0;
        // network play: the others are the seats (0x4B4DA0 gives every player of the race an index,
        // `player+0x4C`, and the launch's `NUMOPPONENTRACECARS` is 0 — 0x4ECF00 for `FEGAME_TYPE` 3, the
        // stream's `(0x28, 0)` — so the fix-up loop 0x4B5081…0x4B50AB that numbers opponent slots after
        // the players never runs). `net_race_apply` set `num_opponents = players − 1` from the launch.
        // **Glue:** a `--net` run that never launched (`net+0xE5 == 0`) has no launch to say so, and
        // gets one seat per other player of the room
        if let Some(n) = self.net.session.as_ref().filter(|n| n.in_race == 0) {
            self.num_opponents = self.num_opponents.max(n.remote_players().len() as u32);
        }
        let n_cars = self.num_opponents + 1;
        // grid order: `.STARTINGPOS` (`[0x658200 + slot·0x580]`, 1 = pole) — 0x4ECF00 gives slot
        // `i` `(nPlayers + nOpp) − i` outside the knockout and the tournament (`modes.md` §1), so
        // the player (slot 0) starts last; the port keeps `STARTINGPOS − 1`
        self.player_grid = self.player_grid_override.unwrap_or(n_cars - 1);
        // `Grid_slot` 0x410580's globals: `LINEAR_TRACK`, and the network smash-up's spacing
        // (`FEGAME_TYPE == 3 && DERBY`, the stream's `DERBY`)
        // (`NUMCARS` at the grid is the players and the kept traffic, 0x4B5079's fix-up on the launch's 8 + traffic)
        let grid_rule = nfs_game::race::GridRule { linear: self.linear_track(), derby_cars: self.net_derby.then(|| n_cars + self.road.traffic_count) };
        // the player's `Grid_slot` slice and pose, for `Car_placeAtSlice` below
        let mut grid_place: Option<(usize, Vec3)> = None;
        let rot = match &table {
            Some(t) => {
                let (pos, rot, slice) = grid_pose(t, start_scene, self.player_grid, n_cars, &grid_rule);
                self.car_pos = pos;
                grid_place = Some((slice, pos));
                let mut progress = RaceProgress::new(slice, t);
                if self.linear_track() {
                    progress.set_linear(t, nfs_game::race::finish_slice(t, start_scene));
                }
                self.race = Some(progress);
                tracing::info!(
                    slice,
                    lap_slices = t.lap_len,
                    startingpos = g_log(Some(self.player_grid)),
                    cars = n_cars,
                    "grid pose"
                );
                // opponents: distinct catalogue models, parked on their grid slots
                self.road.opponents.clear();
                self.road.opponent_models.clear();
                self.road.opponent_sets.clear();
                self.road.opponent_paints.clear();
                self.road.opponent_families.clear();
                // `specs.rs:49`: an opponent's installed-part list is its stock parts (0x4EE9A0,
                // `FECar::init`, unconditional) unless a career opponent brings its own
                let parts_table = self.fe_parts_table();
                let catalog = nfs_formats::fedata::parse_car_catalog(
                    &std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car"))
                        .unwrap_or_default(),
                )
                .unwrap_or_default();
                // (catalogue entry, paint, name, bend-speed factor, grid slot, `.RACENUMBER` decal)
                // per grid slot — the decal is 0x4F4FA0's (`car-model.md` §5.3, `CareerOpponent::decal`)
                // **A playback's opponents** (`Replay_begin` 0x413CF0 copies the recorded block, the car-info records
                // of slots 1...8 with it): the recorded plan and parts, each car resolved by its `.TYPE`; a record
                // that does not resolve (another install) sends the whole race back to the draw
                let replaying = self.playback_running();
                let rec_resolved: Option<Vec<(nfs_formats::fedata::CarCatalogEntry, &nfs_game::replay::CarRecord)>> =
                    if replaying && self.replay_net.is_none() {
                        self.replay.setup.opponent_cars.as_ref().filter(|l| l.len() == self.num_opponents as usize).and_then(|l| {
                            l.iter().map(|r| catalog.iter().find(|c| c.id == r.car_id).map(|c| (c.clone(), r))).collect()
                        })
                    } else {
                        None
                    };
                if replaying && rec_resolved.is_none() && self.replay_net.is_none() && self.num_opponents > 0 {
                    tracing::info!("replay: no opponent records in the setup, the opponents are drawn again (APPROXIMATION)");
                }
                let rec_plan: Option<Vec<(
                    nfs_formats::fedata::CarCatalogEntry,
                    [u8; 3],
                    String,
                    Option<(f32, [[f32; 2]; 4])>,
                    Option<u32>,
                    i32,
                )>> = rec_resolved.as_ref().map(|l| l.iter().map(|(e, r)| (e.clone(), r.paint, r.name.clone(), r.skill.map(|k| (r.difficulty, k)), r.grid, r.decal)).collect());
                let rec_parts: Option<Vec<Vec<nfs_formats::career::Part>>> = rec_resolved.as_ref().map(|l| {
                    l.iter().map(|(_, r)| r.parts.as_ref().map(|ids| ids.iter().filter_map(|id| parts_table.iter().find(|p| p.id == *id).cloned()).collect()).unwrap_or_default()).collect()
                });
                let mut plan_pers: Vec<i16> = self.career_opponents.as_ref().map(|l| l.iter().map(|o| o.personality).collect()).unwrap_or_default();
                let plan: Vec<(
                    nfs_formats::fedata::CarCatalogEntry,
                    [u8; 3],
                    String,
                    Option<(f32, [[f32; 2]; 4])>,
                    Option<u32>,
                    i32,
                )> = match (&rec_plan, &self.career_opponents) {
                    // a playback of a recording that carries the opponents' records: the cars the race ran
                    // with (`Replay_begin` 0x413CF0 restores the nine car-info records), no new draw
                    (Some(p), _) => p.clone(),
                    (None, Some(list)) => list
                        .iter()
                        .map(|o| {
                            (
                                o.car.clone(),
                                o.paint,
                                o.name.clone(),
                                Some((o.difficulty, o.skill)),
                                o.grid,
                                o.decal,
                            )
                        })
                        .collect(),
                    // quick race: `FEOpponentCarAccessor` 0x4F4FA0 (`modes.md` §1.1a). The candidate
                    // list is every racing-car record with a model on disk (0x4F3E40), the choice is
                    // the menu's `OpponentCar.Car`, and each opponent draws its own paint set.
                    (None, None) => {
                        let selected = self
                            .player_car_id
                            .and_then(|id| catalog.iter().position(|c| c.id == id))
                            .or_else(|| {
                                catalog.iter().position(|c| {
                                    c.model.eq_ignore_ascii_case(&self.car.model)
                                        && !c.sim.is_empty()
                                })
                            })
                            .unwrap_or(0);
                        let (list, picks, drawn) = Self::quick_opponents(&self.paths, &self.fe.strings, self.opp_choice, self.race_skill, &catalog, selected, self.num_opponents as usize);
                        // the accessor keeps the seven cars (a knockout round in between takes
                        // them, 0x4DE1E7), and `[0x6907D8]` is this draw now (0x4EBC63 → 0x4E3160
                        // clears it at 0x4E3176 and pushes each id at 0x4E32B5) — what the next
                        // save's `Personalities` module writes
                        self.opp_cars = Self::opp_car_saves(&catalog, &picks);
                        if let Some(c) = self.career.as_mut() {
                            c.side.personalities = Some(drawn.iter().map(|p| *p as u16).collect());
                        }
                        plan_pers = list.iter().map(|o| o.personality).collect();
                        list.into_iter()
                            .map(|o| (o.car, o.paint, o.name, (o.personality != 0).then_some((o.difficulty, o.skill)), None, o.decal))
                            .collect()
                    }
                };
                // **Each remote player races the car its own machine built** (0x4EEDA0, kind 1): the
                // launch's per-slot records at the players' race indices (`network.md` §11.12) fill
                // the opponent seats in the room's order — the seat of a player is its index among
                // the others, which is how 0x4B4DA0 numbers `player+0x4C`. `.FILENAME` / `.SPECNAME` /
                // `.BODYSTYLE` name the car, `.COLORCONFIG` / `.STRIPEPACKAGE` its paint, `.LICENSE`
                // the plate, `.RACENUMBER` the number, `.DRIVER` the driver; a seat whose owner sent
                // no car (or one this install lacks) keeps the seat's own.
                let mut plan = plan;
                let mut seat_recs: Vec<Option<nfs_net::launchcar::CarSlot>> = vec![None; plan.len()];
                // (a network race played back from a file has no session: the names come from the file)
                let net_names: Option<Vec<String>> = match (self.net.session.as_ref(), self.replay_net.as_ref()) {
                    (Some(n), _) => Some(n.remote_players().into_iter().map(|p| p.1).collect()),
                    (None, Some(r)) => Some(r.names.clone()),
                    (None, None) => None,
                };
                if let Some(room_names) = net_names {
                    use nfs_net::launchcar::id as cid;
                    for (k, rec) in self.net_seat_cars.iter().enumerate() {
                        let Some(p) = plan.get_mut(k) else { break };
                        let text = |i: u32| rec.text(i).map(|b| b.iter().map(|&c| nfs_formats::locale::cp1252(c)).collect::<String>());
                        let Some(file) = text(cid::FILENAME).filter(|f| !f.is_empty()) else {
                            tracing::warn!(seat = k, "network opponent sent no car record: the seat keeps its own car");
                            continue;
                        };
                        let (spec, body) = (text(cid::SPECNAME).unwrap_or_default(), rec.num(cid::BODYSTYLE).map(|v| v as i16));
                        let same_file = |c: &&nfs_formats::fedata::CarCatalogEntry| c.model.eq_ignore_ascii_case(&file);
                        let Some(mut e) = catalog
                            .iter()
                            .filter(same_file)
                            .find(|c| c.sim.eq_ignore_ascii_case(&spec) && Some(c.variant) == body)
                            .or_else(|| catalog.iter().filter(same_file).find(|c| c.sim.eq_ignore_ascii_case(&spec)))
                            .or_else(|| catalog.iter().find(same_file))
                            .cloned()
                        else {
                            tracing::warn!(seat = k, file, "network opponent's car is not in this install: the seat keeps its own");
                            continue;
                        };
                        // the record's own spec names (the `evenstvn` code sends "993coupe36" / "993")
                        if !spec.is_empty() {
                            e.sim = spec.clone();
                        }
                        if let Some(a) = text(cid::AISPECNAME).filter(|a| !a.is_empty()) {
                            e.sim_family = a;
                        }
                        // `.COLORCONFIG` = exterior, stripe 1, stripe 2, interior (0x4EF100…0x4EF13C)
                        let rgb = |c: u32| [(c >> 16) as u8, (c >> 8) as u8, c as u8];
                        let colour = rec.num(cid::COLOUR).map_or(p.1, rgb);
                        p.0 = e.clone();
                        p.1 = colour;
                        p.2 = text(cid::LICENSE).filter(|l| !l.is_empty()).or_else(|| room_names.get(k).cloned()).unwrap_or_default();
                        if let Some(g) = self.net_seat_grids.get(k).copied().flatten() {
                            p.4 = Some(g);
                        }
                        if let Some(r) = rec.num(cid::RACENUMBER) {
                            p.5 = r as i32;
                        }
                        tracing::info!(
                            seat = k, car = %e.display_name, model = %e.model, sim = %e.sim, colour = format_args!("{:02X}{:02X}{:02X}", colour[0], colour[1], colour[2]),
                            plate = %p.2, grid = g_log(self.net_seat_grids.get(k).copied().flatten()),
                            "network opponent's car (0x4EEDA0 record: .FILENAME / .SPECNAME / .COLOUR / .LICENSE)"
                        );
                        seat_recs[k] = Some(rec.clone());
                    }
                }
                // the opponents' car-info records, kept in the setup for a playback (`Replay_begin` restores them)
                if !replaying && self.net.session.is_none() && self.replay_net.is_none() && self.replay.setup.net.is_none() {
                    self.replay.setup.opponent_cars = Some(
                        plan.iter()
                            .enumerate()
                            .map(|(g, (entry, paint, name, skill, grid, decal))| nfs_game::replay::CarRecord {
                                car_id: entry.id,
                                paint: *paint,
                                name: name.clone(),
                                grid: *grid,
                                decal: *decal,
                                personality: plan_pers.get(g).copied().unwrap_or(0),
                                difficulty: skill.map_or(0.0, |k| k.0),
                                skill: skill.map(|k| k.1),
                                parts: self.career_opponents.as_ref().and_then(|l| l.get(g)).map(|o| o.parts.iter().map(|p| p.id).collect()),
                                ..Default::default()
                            })
                            .collect(),
                    );
                }
                for (g, (entry, paint, name, skill, grid, decal)) in plan.iter().enumerate() {
                    // the opponent record's parts (0x4ED880), for a career opponent
                    let parts: Vec<nfs_formats::career::Part> = match &rec_parts {
                        Some(l) => l.get(g).cloned().unwrap_or_default(),
                        None => self.career_opponents.as_ref().and_then(|l| l.get(g)).map(|o| o.parts.clone()).unwrap_or_default(),
                    };
                    let swaps: Vec<[i16; 3]> = parts.iter().flat_map(|p| p.geometry_swaps.iter().copied()).collect();
                    // opponent slot `g + 1` → `STARTINGPOS = n − (g + 1)` (0x4ECF00) unless the
                    // career set one (knockout 0x4DE150, tournament ranks 0x503660)
                    let slot = g;
                    let g = grid.unwrap_or((n_cars - 1).saturating_sub(g as u32 + 1));
                    // a remote player's car is what its own machine sent (`.SPECNAME` already holds the
                    // sender's `evenstvn` code): this machine's cheat bits are not applied to it
                    let seat_rec = seat_recs.get(slot).and_then(|r| r.as_ref());
                    let entry_993 = if seat_rec.is_some() { entry.clone() } else { self.cheat_993(entry.clone()) };
                    let entry = &entry_993;
                    // `.TINYCAR` 0x24F is per car: a remote player's car is tiny when *its* record says so (its own
                    // machine's Gulliver code, sent only when set), every other car by this machine's own bit
                    let car_tiny = seat_rec.map_or(self.cheat_bits & CHEAT_TINY != 0, |r| r.num(nfs_net::launchcar::id::TINYCAR).is_some_and(|v| v != 0));
                    let (Ok(assets), Ok(sim)) = (
                        CarAssets::load(&self.paths, &entry.model).map(|c| {
                            let mut c = c.with_style(entry.variant).with_cabrio(entry.cabrio).with_style_overlay(swaps);
                            c.tiny = car_tiny;
                            c
                        }),
                        CarAssets::load_sim(&self.paths, &entry.sim),
                    ) else {
                        continue;
                    };
                    let mut paint = Paint::matching(entry, *paint);
                    // `.COLORCONFIG` (exterior, stripe 1, stripe 2, interior) and `.STRIPEPACKAGE`
                    if let Some(rec) = seat_rec
                        && let Some(cc) = rec.array(nfs_net::launchcar::id::COLORCONFIG).filter(|a| a.len() == 4)
                    {
                        let rgb = |c: u32| [(c >> 16) as u8, (c >> 8) as u8, c as u8];
                        paint = Paint { exterior: rgb(cc[0]), stripe1: rgb(cc[1]), stripe2: rgb(cc[2]), interior: rgb(cc[3]), package: rec.num(nfs_net::launchcar::id::STRIPEPACKAGE).unwrap_or(0) };
                    }
                    let mut assets = assets;
                    assets.page_text = Some(self.page_text(slot + 1, name, Some(*decal)));
                    // the draw stays (the seat order of the shared generator is the same), a remote
                    // player's own `.DRIVER` (its profile picture, 0x4EEDBD) replaces the value
                    let drawn = (driver_rand.next() & 7) as u32;
                    let driver = seat_rec.and_then(|r| r.num(nfs_net::launchcar::id::DRIVER)).unwrap_or(drawn);
                    assets.driver = nfs_assets::car::DriverSeat { driver, no_driver: false, factory_driver };
                    let Ok(model) = CarModel::upload_with_env(
                        renderer,
                        &assets,
                        &self.paths,
                        1,
                        paint,
                        CarUpload {
                            env: car_env,
                            night: self.track.is_night(),
                            // `.CABRIODOWN` 0x24B: 0 for every car but a player's (0x4EEF0C), whose own
                            // machine sent `Car.Roof` through 0x4F1E40's predicate
                            race_roof: Some(seat_rec.and_then(|r| r.num(nfs_net::launchcar::id::CABRIODOWN)).unwrap_or(0)),
                            skidpad: self.skidpad_rule(),
                        },
                    ) else {
                        continue;
                    };
                    let mut specs = CarSpecs::from_sim(
                        &sim,
                        entry.sim.to_lowercase().starts_with("gt") || entry.sim.starts_with("935"),
                    );
                    if car_tiny {
                        specs.apply_tinycar();
                    }
                    // `CarSim_applyParts` 0x49CB50 on the opponent's installed list, whose grip ratio the
                    // `.ais` loader then reads (0x49CAD7, `ai.md` §2.1). Which list: a remote player's
                    // record (0x4EEDA0, kind 1) sends `.PARTINDEX` — its installed parts, used when they
                    // are not the car's stock ones; a career opponent brings its own parts (`opp+0x04`);
                    // every other FE car is `FECar::init`'s unconditional 0x4EE9A0 stock fit
                    // (0x4EED28) — the record's stock parts, **folded** here since 0x49CB50 walks
                    // `car+0x768` for every car of the race, whoever it belongs to.
                    let seat_parts: Option<Vec<&nfs_formats::career::Part>> = seat_rec.and_then(|rec| {
                        let ids: Vec<i16> = rec.parts().into_iter().map(|p| p as i16).collect();
                        (!ids.is_empty() && ids != entry.stock_parts).then(|| ids.iter().filter_map(|id| parts_table.iter().find(|p| p.id == *id)).collect())
                    });
                    let fold_list: Vec<&nfs_formats::career::Part> = match (&seat_parts, parts.is_empty()) {
                        (Some(o), _) => o.clone(),
                        (None, false) => parts.iter().collect(),
                        (None, true) => entry.stock_parts.iter().filter_map(|id| parts_table.iter().find(|p| p.id == *id)).collect(),
                    };
                    let block_before = (specs.grip, specs.transfer_div, specs.roll_gain, specs.top_speed);
                    let grip_ratio = specs.apply_parts(&fold_list, &entry.sim_family);
                    // the installed-part list `car+0x768`: the list just folded, at its carried state
                    // (a career opponent's ids, else the record's stock ones — `specs.rs:49`, `stock_parts`)
                    specs.parts = match (&seat_parts, parts.is_empty()) {
                        (Some(o), _) => nfs_game::career::installed_parts(o, None),
                        (None, false) => nfs_game::career::installed_parts(&parts.iter().collect::<Vec<_>>(), None),
                        (None, true) => nfs_physics::specs::stock_parts(&entry.stock_parts, &parts_table),
                    };
                    // then the seventeen car-setup sliders (0x49CB50 folds them in after the parts,
                    // 0x49D290): a remote player's own (`.CARSETUP` records), else the constructor's
                    // neutral ones (0x4EED2D — every FECar carries them and 0x4EEDA0 streams them for
                    // every kind, 0x4EF179…0x4EF32F). `carinfo+0xC` is `.TRANSMISSION` 0x221: the
                    // record's, else the gearbox option (0x4EEDA0 writes it for every kind whose
                    // `param_7` is 0 — the opponents' callers 0x4EBCBF / 0x4EBF06 / 0x4EC3FF and 0x4DE213 pass 0)
                    let mut seat_setup = None;
                    let automatic = seat_rec
                        .and_then(|rec| rec.num(nfs_net::launchcar::id::TRANSMISSION))
                        .map_or_else(|| self.option_automatic(), |v| v == 1);
                    let setup = seat_rec.and_then(|rec| rec.setup());
                    if let Some(setup) = setup {
                        seat_setup = Some(setup);
                    }
                    specs.apply_setup(&setup.unwrap_or(nfs_game::career::CAR_SETUP_DEFAULT), automatic);
                    tracing::info!(
                        seat = slot, model = entry.model, parts = fold_list.len(), grip_ratio,
                        grip_before = block_before.0, grip = specs.grip, transfer_div_before = block_before.1, transfer_div = specs.transfer_div,
                        roll_gain_before = block_before.2, roll_gain = specs.roll_gain, top_speed_before = block_before.3, top_speed = specs.top_speed,
                        "opponent's stock parts / own parts and set-up folded (0x49CB50)"
                    );
                    let (p, r, sl) = grid_pose(t, start_scene, g, n_cars, &grid_rule);
                    let (linear, finish) = (self.linear_track(), nfs_game::race::finish_slice(t, start_scene));
                    let mut vehicle = Vehicle::new(p, r, &specs, model.half_track, model.wheel_radius);
                    vehicle.race_started = false;
                    vehicle.traction_control = true; // AI cars never spin their wheels (Tyre 0x4A20D0)
                    vehicle.ai_driven = true;
                    // its own `carinfo+0xC` (0x4EEDA0's `.TRANSMISSION`: `Car.Transmission` — the opponents' callers pass 0
                    // for the mission flag): the reset gear 0x49E228; AutoShift runs for an AI car either way (0x49E885)
                    vehicle.automatic = automatic;
                    // the remote car's `carinfo` options: `.TRACTION` 0x23B → `carinfo+0x20` (`Vehicle::tcs`),
                    // `.TRANSMISSION` 0x221 → `+0xC`, `.ABS` 0x23A → `+0x24` (no field of the port's
                    // vehicle: the ABS arm 0x4A2308 / 0x4A23CC is not ported, `physics.md` §5.4)
                    if let Some(rec) = seat_rec {
                        use nfs_net::launchcar::id as cid;
                        vehicle.tcs = rec.num(cid::TRACTION).is_some_and(|v| v != 0);
                        vehicle.automatic = rec.num(cid::TRANSMISSION).is_none_or(|v| v == 1);
                        tracing::info!(
                            seat = slot, abs = rec.num(cid::ABS), traction = rec.num(cid::TRACTION), roof = rec.num(cid::CABRIODOWN),
                            transmission = rec.num(cid::TRANSMISSION), driver = rec.num(cid::DRIVER), body_style = rec.num(cid::BODYSTYLE),
                            package = rec.num(cid::STRIPEPACKAGE), race_number = rec.num(cid::RACENUMBER).map(|v| v as i32),
                            parts = ?rec.parts(), setup = ?seat_setup.map(|s| s.map(|v| (v * 100.0).round() / 100.0)),
                            "network race setup: the remote car carries its owner's carinfo (0x4EEDA0 kind 1)"
                        );
                    }
                    // the body `RCar::load` builds: 0x445090's model box, `BndP` less `car+0x3AC`
                    let (pts, half) = nfs_game::world::car_body(&assets, car_tiny);
                    vehicle.set_car_body(pts, half);
                    // `Car_raceInit` 0x410730: `Grid_slot` 0x410580's slice and offset into
                    // `Car_placeAtSlice` 0x49B090 (0x41095C), dir +1 — the port's reversed table
                    // already turns the slices round. An opponent (`car+0x52C & 8`) on the race's
                    // first tick, so the offset is slice-local (0x49B0FB)
                    vehicle.game_tick = self.tick;
                    let s = &t.slices[sl];
                    vehicle.place_at_slice(&world.collision, &specs, s.centre, s.forward, grid_offset(s, p), true, false);
                    let mut model = model;
                    if car_tiny {
                        model.scale = 0.4;
                    }
                    // its dash needles' calibration (0x4EF331…0x4EF37B, per car)
                    let dash = seat_rec
                        .and_then(|r| {
                            use nfs_net::launchcar::id as cid;
                            Some([r.num(cid::TACH0RPM)?, r.num(cid::TACH1000RPM)?, r.num(cid::SPEEDO0KM)?, r.num(cid::SPEEDO100KM)?].map(f32::from_bits))
                        })
                        .unwrap_or(entry.dash);
                    model.set_dash(dash[0], dash[1], dash[2], dash[3]);
                    self.road.opponent_models.push(model);
                    let controller = match CarAssets::load_ais(&self.paths, &entry.model) {
                        Ok(ais) => {
                            let mut d = nfs_game::ai::AiDriver::new(ais, &sim.unknown_tail);
                            d.grip_ratio = grip_ratio;
                            // the family key `nfs5.car` +0x115 the steering table is keyed on
                            d.set_track(&self.track.name, self.backwards, &entry.sim_family);
                            if let Some((difficulty, pairs)) = skill {
                                d.set_skill(*difficulty, *pairs);
                            }
                            Controller::Ai(Box::new(d))
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, model = entry.model, "no .ais: opponent parked");
                            Controller::Parked
                        }
                    };
                    self.road.opponent_sets.push(entry.sound_set.clone());
                    self.road.opponent_paints.push(paint.exterior);
                    self.road.opponent_families.push(entry.family.clone());
                    self.road.opponents.push(RaceCar {
                        name: name.clone(),
                        vehicle,
                        specs,
                        progress: {
                            let mut p = RaceProgress::new(sl, t);
                            if linear {
                                p.set_linear(t, finish);
                            }
                            p
                        },
                        controller,
                        grid_pos: g,
                        model: self.road.opponent_models.len() - 1,
                        wall_hits: 0,
                        resets: 0,
                        eliminated: false,
                        off_surface: false,
                        // `Car_init` 0x410C74: the night flag
                        lamp_bits: if self.track.is_night() { 3 } else { 0 },
                        tiny: car_tiny,
                    });
                }
                // network play: every opponent seat belongs to a remote player. Their cars sit on
                // the grid until the first 0xBE arrives and are driven by it afterwards, so their
                // controllers never run.
                if let Some(n) = self.net.session.as_mut() {
                    n.reserve((0..self.road.opponents.len()).collect());
                    // the remote cars are human cars (`car+0xDA8 == 0`, no AI flag, no AI traction control) that
                    // keep their `.ais` driver for the AI arm of a stale car (0x4991E0); one whose driver did
                    // not load stays parked, which is what the port's stale seat did before
                    for o in self.road.opponents.iter_mut() {
                        o.vehicle.ai_driven = false;
                        o.vehicle.traction_control = false;
                    }
                    tracing::info!(
                        seats = self.road.opponents.len(),
                        host = n.is_host(),
                        "network seats reserved"
                    );
                } else if self.replay_net.is_some() {
                    // a network race played back: every human car is fed from its lane of the stream, and is
                    // a human car as `net_exchange_begin` made it — no AI flag, no AI traction control
                    let seat_index: Vec<u32> = self.replay_net.as_ref().map(|r| r.seats.clone()).unwrap_or_default();
                    for (k, o) in self.road.opponents.iter_mut().enumerate() {
                        o.controller = Controller::Parked;
                        if seat_index.get(k).is_some_and(|i| *i != u32::MAX) {
                            o.vehicle.ai_driven = false;
                            o.vehicle.traction_control = false;
                        }
                    }
                    tracing::info!(seats = self.road.opponents.len(), "network replay: the seats' cars are fed from the stream");
                }
                // traffic (`traffic.md` §1, §8, §9): the track's seven traffic car ids are
                // `nfs5.trk +0x7C`; their `nfs5.car` records carry only a display name, a model and a
                // sound set (`Sbus`/`Tbus` mark the buses, which respawn five times less often).
                // Which of the race's car slots take which id is not in the notes (§8, UNKNOWN), so
                // the ids are handed out in table order and cycled when more cars than ids are asked
                // for (the heavy career setting wants eight).
                // the host's per-slot colours are for this launch only (set by `net_race_apply`)
                let net_colours = std::mem::take(&mut self.road.traffic_colours);
                self.road.traffic = None;
                self.road.traffic_models.clear();
                self.road.traffic_sets.clear();
                self.road.traffic_paints.clear();
                self.road.traffic_prev.clear();
                // **Nine cars at most.** 0x4ED382…0x4ED399 add cops, players, opponents and traffic
                // and, when that exceeds 9, cut the traffic to `9 − players − opponents − cops` —
                // so a full grid of eight leaves room for one traffic car, and with the four cops
                // of a cops race none; a police mission's two count likewise.
                let cops: u32 = if self.road.cops_enabled { self.road.cop_count } else { 0 };
                let room = 9u32.saturating_sub(1 + self.num_opponents + cops);
                let traffic_count = self.road.traffic_count.min(room);
                if traffic_count > 0 {
                    let trk = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.trk"))
                        .ok()
                        .and_then(|d| nfs_formats::career::parse_track_table(&d).ok())
                        .unwrap_or_default();
                    let ids: Vec<u16> = match &self.road.traffic_ids {
                        Some(v) => v.clone(),
                        None => trk
                            .iter()
                            .find(|r| r.file.eq_ignore_ascii_case(&self.track.name))
                            .map(|r| r.traffic_cars.iter().copied().filter(|i| *i != 0).collect())
                            .unwrap_or_default(),
                    };
                    let mut plan: Vec<TrafficSpec> = Vec::new();
                    // id -> the *first* car's slot of that catalogue id (the template every repeat
                    // instantiates its own damage resources from, `car-model.md` §13.1)
                    let mut template_slot: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
                    // per model slot: half the LOD-1 box 0x445090 takes for traffic, and the
                    // `BndP` body points `RCar::load` builds (`car_body`) for its `RigidBody`
                    let mut boxes: Vec<Vec3> = Vec::new();
                    let mut points: Vec<Vec<Vec3>> = Vec::new();
                    // each traffic car's own block: `default.sim` (its sim name is empty, §21.1) with
                    // its record's stock parts (0x4EE9A0) and the neutral sliders (0x4EED2D) folded in
                    // by 0x49CB50 — a traffic car is a racer slot below `NUMCARS`, so 0x411130 builds
                    // its parts (0x49C4C0) and 0x410D60 its block (0x49C6C0 → 0x49CB50) like any car's
                    let traffic_parts_table = self.fe_parts_table();
                    let traffic_automatic = self.option_automatic();
                    let traffic_sim = CarAssets::load_sim(&self.paths, "default").ok();
                    let mut traffic_car_specs: Vec<CarSpecs> = Vec::new();
                    for k in 0..traffic_count as usize {
                        let Some(id) = ids.get(k % ids.len().max(1)).copied() else {
                            break;
                        };
                        let Some(entry) = catalog.iter().find(|c| c.id == id) else {
                            continue;
                        };
                        let bus = entry.sound_set.eq_ignore_ascii_case("sbus")
                            || entry.sound_set.eq_ignore_ascii_case("tbus");
                        let slot: usize = match template_slot.get(&id) {
                            // a repeat of a shared catalogue model: this car's own crush meshes
                            // and decal-page materials, a cheap clone of the template's GPU ids —
                            // everything else (body/wheel/light geometry) stays shared with it
                            Some(&t) => {
                                let inst = self.road.traffic_models[t].instantiate(renderer);
                                self.road.traffic_models.push(inst);
                                boxes.push(boxes[t]);
                                points.push(points[t].clone());
                                self.road.traffic_models.len() - 1
                            }
                            None => {
                                let assets = match CarAssets::load(&self.paths, &entry.model) {
                                    Ok(a) => a,
                                    Err(e) => {
                                        tracing::warn!(id, model = entry.model, error = %e, "traffic car assets missing");
                                        continue;
                                    }
                                };
                                let paint = match net_colours.get(k).copied().flatten() {
                                    // the host's `.COLOUR` for this slot, where it differs (network race)
                                    Some(c) => Paint::from(c),
                                    None if entry.paint_sets.is_empty() => Paint::from([0x8C, 0x8C, 0x90]),
                                    None => Paint::from_catalogue(entry, 0),
                                };
                                // a traffic car (`car+0x52C & 0x10`) has no driver: slot 9 = 0 (0x43AED4)
                                let mut assets = assets;
                                assets.driver = nfs_assets::car::DriverSeat { driver: 0, no_driver: true, factory_driver };
                                let model = match CarModel::upload_with_env(
                                    renderer,
                                    &assets,
                                    &self.paths,
                                    1,
                                    paint,
                                    CarUpload { env: car_env, night: self.track.is_night(), race_roof: Some(0), skidpad: self.skidpad_rule() },
                                ) {
                                    Ok(m) => m,
                                    Err(e) => {
                                        tracing::warn!(id, model = entry.model, error = %e, "traffic car upload failed");
                                        continue;
                                    }
                                };
                                self.road.traffic_models.push(model);
                                // 0x445090: a traffic car (`car+0x52C & 0x10`, 0x4450CC) takes the
                                // box of `[this+0xB0][0]`, LOD 1, origin included (0x43DE90)
                                // the `BndP` points are lowered by that same `car+0x3AC` (0x4421BE,
                                // `traffic_body`) — not by LOD 5's, which a traffic model lacks: they
                                // sat at the origin's own height and the car stood `half.y` too low
                                let (pts, half) = nfs_game::world::traffic_body(&assets);
                                boxes.push(half);
                                points.push(pts);
                                let slot = self.road.traffic_models.len() - 1;
                                template_slot.insert(id, slot);
                                slot
                            }
                        };
                        // the collision box is half the model's LOD-1 box (`traffic.md` §21.4,
                        // `car-model.md` §12) — `car+0x3AC` is also how far the body origin sits
                        // above the model's ground (`TrafficCar::draw_origin`)
                        self.road.traffic_sets.push(entry.sound_set.clone());
                        self.road.traffic_paints.push(net_colours.get(k).copied().flatten().unwrap_or_else(|| {
                            entry.paint_sets.first().map(|p| p.colours[0]).unwrap_or([0x8C, 0x8C, 0x90])
                        }));
                        plan.push(TrafficSpec {
                            model: slot,
                            bus,
                            half_extents: boxes[slot],
                        });
                        if let Some(sim) = &traffic_sim {
                            // (`.TINYCAR` is on every car but traffic — `CarSpecs::apply_tinycar`)
                            let mut cs = CarSpecs::from_sim(sim, false);
                            nfs_game::career::fold_fresh_car(&mut cs, &entry.stock_parts, &traffic_parts_table, &entry.sim_family, traffic_automatic);
                            traffic_car_specs.push(cs);
                        }
                    }
                    if self.road.traffic_specs.is_none() {
                        self.road.traffic_specs = traffic_sim.as_ref().map(|sim| {
                            let mut cs = CarSpecs::from_sim(sim, false);
                            nfs_game::career::fold_fresh_car(&mut cs, &[], &traffic_parts_table, "", traffic_automatic);
                            cs
                        });
                    }
                    if !plan.is_empty() {
                        // the table is already reversed for a backwards race, so the route direction
                        // is always +1 here; `linear` = a point-to-point track (`LINEAR_TRACK`)
                        let linear = !self
                            .records
                            .track(&self.track.name)
                            .map(|r| r.circuit)
                            .unwrap_or(false);
                        // `NFS_TRAFFIC_SEED` makes a run repeatable (the original seeds from the clock)
                        let seed = self.traffic_seed;
                        let mut set = TrafficSet::new(&plan, t, 1.0, linear, seed);
                        // `RigidBody_init` 0x49B510 for each car with `default.sim` (§21.1)
                        let specs = self.road.traffic_specs.clone().unwrap_or_else(|| self.specs.clone());
                        set.set_bodies_each(&traffic_car_specs, &specs, &points);
                        for b in &mut set.bodies {
                            b.automatic = traffic_automatic; // its own `carinfo+0xC` (0x4ED460 / 0x4ED56E pass 0)
                        }
                        set.cops_race = self.road.cops_race;
                        // `TRACK_NAME` for the spawn gate's canyon / industrial windows (0x40E0AD)
                        set.set_track(&self.track.name);
                        // `[0x5E8BF8]` for the §5.2 rule 3 early-race boost
                        set.opponents = self.num_opponents;
                        // The ×0.9 is gated on `[0x657A24]`, which the option registry names
                        // **`WEATHER`** (id 0x1E at 0x5D1F90) — not the track's night flag (the
                        // port used to pass the night flag here; corrected 2026-09-21). It is not
                        // `registry.u32("weather")` either: the race load's 0x414DB0 stores 0 into
                        // it (0x414DF9) after the track load's spec-index read and before any
                        // in-race reader, so a race always sees 0 (`traffic.md` §33.4).
                        set.weather = false;
                        // `[0x657A30]`, the registry's `TIME` (the time-of-day slot: `traffic.md`
                        // §33.3) gates the human-honk trigger (0x40172E). Only `fe.txt` sets it —
                        // no race-setup message carries id 0x21 and the track spec `Tr00.bin` that
                        // 0x475C80 would overwrite it from is not shipped
                        set.time_mode = self.net_fe_txt.get("time").copied().unwrap_or_else(|| self.registry.u32("time", 0)) != 0;
                        // `scripts.bin` drives the reactions of `traffic.md` §12.3 / §12.9 / §30
                        if let Some(p) = GamePaths::find_file(
                            &self.paths.game_data().join("Simulation").join("AI"),
                            "scripts.bin",
                        ) && let Ok(d) = std::fs::read(&p)
                        {
                            match nfs_formats::scripts::Scripts::parse(&d) {
                                Ok(sc) => set.scripts = sc,
                                Err(e) => tracing::warn!(error = %e, "scripts.bin unreadable"),
                            }
                        }
                        self.road.traffic = Some(set);
                        for sp in &plan {
                            tracing::debug!(model = sp.model, bus = sp.bus, half = ?sp.half_extents, mass = sp.mass(), "traffic body");
                        }
                        tracing::info!(
                            cars = plan.len(),
                            models = self.road.traffic_models.len(),
                            linear,
                            "traffic placed"
                        );
                    }
                }
                // police (`traffic.md` §17.1, §25): the track's five cop car ids are `nfs5.trk +0x8A`
                // and the spawn points `GameData/Track/Cops/<track>.cop`; a single race gets four cars.
                self.road.police = None;
                self.road.police_models.clear();
                self.road.police_sets.clear();
                self.road.police_specs.clear();
                self.road.police_prev.clear();
                if self.road.cops_enabled {
                    let trk = std::fs::read(self.paths.fe_data().join("Data").join("nfs5.trk"))
                        .ok()
                        .and_then(|d| nfs_formats::career::parse_track_table(&d).ok())
                        .unwrap_or_default();
                    let ids: Vec<u16> = trk
                        .iter()
                        .find(|r| r.file.eq_ignore_ascii_case(&self.track.name))
                        .map(|r| r.police_cars.iter().copied().filter(|i| *i != 0).collect())
                        .unwrap_or_default();
                    let cop_file = GamePaths::find_file(
                        &self.paths.game_data().join("Track").join("Cops"),
                        &format!("{}.cop", self.track.name),
                    )
                    .and_then(|p| std::fs::read(p).ok())
                    .and_then(|d| {
                        nfs_formats::cop::CopFile::parse(&d)
                            .map_err(|e| tracing::warn!(error = %e, "cop file unreadable"))
                            .ok()
                    });
                    if let Some(file) = cop_file
                        && !ids.is_empty()
                    {
                        let mut cars = Vec::new();
                        // (id, driver) -> the *first* cop's slot of that combination (the
                        // template every repeat instantiates its own damage resources from,
                        // `car-model.md` §13.1)
                        let mut template_slot: std::collections::HashMap<(u16, u32), usize> = std::collections::HashMap::new();
                        // per model slot: half the LOD-5 box 0x445090 takes for a cop
                        let mut boxes: Vec<Vec3> = Vec::new();
                        let mut probe_sets: Vec<Vec<Vec3>> = Vec::new();
                        for k in 0..self.road.cop_count as usize {
                            let id = ids[k % ids.len()];
                            // a cop's `.DRIVER`: 0x4ED0BC / 0x4ED1A5 launch each cop car through
                            // 0x4EEDA0 (kind 8, 0x4ED174 / 0x4ED25E), so every cop record draws
                            // its own `rand() & 7` (0x4EEDAC), cop 0 first, right after the
                            // opponents' — made before the model is looked up, as the original
                            // calls 0x4EEDA0 whatever 0x4F43A0 returned. Cops that share a model
                            // share it only when they drew the same driver
                            let driver = (driver_rand.next() & 7) as u32;
                            let Some(entry) = catalog.iter().find(|c| c.id == id) else {
                                continue;
                            };
                            let entry_993 = self.cheat_993(entry.clone());
                            let entry = &entry_993;
                            let slot = match template_slot.get(&(id, driver)) {
                                // a repeat (id, driver): this cop's own crush meshes and
                                // decal-page materials, a cheap clone of the template's GPU ids
                                Some(&t) => {
                                    let inst = self.road.police_models[t].instantiate(renderer);
                                    self.road.police_models.push(inst);
                                    boxes.push(boxes[t]);
                                    probe_sets.push(probe_sets[t].clone());
                                    self.road.police_models.len() - 1
                                }
                                None => {
                                    let Ok(assets) = CarAssets::load(&self.paths, &entry.model)
                                    else {
                                        continue;
                                    };
                                    let paint = if entry.paint_sets.is_empty() { Paint::from([0xE0, 0xE0, 0xE0]) } else { Paint::from_catalogue(entry, 0) };
                                    let mut assets = assets;
                                    assets.driver = nfs_assets::car::DriverSeat { driver, no_driver: false, factory_driver };
                                    let Ok(model) = CarModel::upload_with_env(
                                        renderer,
                                        &assets,
                                        &self.paths,
                                        1,
                                        paint,
                                        CarUpload { env: car_env, night: self.track.is_night(), race_roof: Some(0), skidpad: self.skidpad_rule() },
                                    ) else {
                                        continue;
                                    };
                                    let mut model = model;
                                    if self.cheat_bits & CHEAT_TINY != 0 {
                                        model.scale = 0.4;
                                    }
                                    self.road.police_models.push(model);
                                    // 0x445090: a cop (`car+0x52C` 0x22 / 0x62, bit 0x10 clear at
                                    // 0x4450D2) takes `[this+0xB0][4]`, LOD 5, as a racer does;
                                    // `× 0.4` for `.TINYCAR` (0x445140)
                                    let (points, half) = nfs_game::world::car_body(&assets, self.cheat_bits & CHEAT_TINY != 0);
                                    boxes.push(half);
                                    // the body points, the first four being `Susp_update`'s
                                    // wheel probes (0x499AD4, `CopCar::probes`)
                                    probe_sets.push(points);
                                    let slot = self.road.police_models.len() - 1;
                                    template_slot.insert((id, driver), slot);
                                    slot
                                }
                            };
                            // `car+0x3A8…`: also how far the body origin sits above the model's
                            // ground (`CopCar::draw_origin`)
                            let half = boxes[slot];
                            // a cop car has a real `.sim` (§17.1), so its mass is the sim's
                            // …folded by 0x49CB50 like every racer's: the record's stock parts (0x4EE9A0) and
                            // the neutral sliders (0x4EED2D), `.TRANSMISSION` the gearbox option (0x4ED174's
                            // `param_7` is 0)
                            let cop_automatic = self.option_automatic();
                            let cop_parts_table = self.fe_parts_table();
                            let specs = CarAssets::load_sim(&self.paths, &entry.sim)
                                .ok()
                                .map(|s| {
                                    let mut cs = CarSpecs::from_sim(&s, false);
                                    nfs_game::career::fold_fresh_car(&mut cs, &entry.stock_parts, &cop_parts_table, &entry.sim_family, cop_automatic);
                                    cs
                                })
                                .unwrap_or_else(|| self.specs.clone());
                            self.road.police_sets.push(entry.sound_set.clone());
                            self.road.police_specs.push(specs.clone());
                            // §17.2: the cop's chase level is the heat record's `level[role]`, not
                            // the race skill — role 0 is always level 2
                            let mut cop = nfs_game::police::CopCar::new(
                                slot,
                                half,
                                &specs,
                                nfs_game::police::COP_LEVEL[0],
                            );
                            // the body `RCar::load` builds: the `BndP` points less `car+0x3AC`, the
                            // first four being `Susp_update`'s probes (0x499AD4)
                            if let Some(p) = probe_sets.get(slot).filter(|p| p.len() >= 4) {
                                cop.set_body(p.clone(), half);
                            }
                            cop.body.automatic = cop_automatic; // its own `carinfo+0xC` (0x4ED174 / 0x4ED25E pass 0)
                            cop.body.tiny = self.cheat_bits & CHEAT_TINY != 0;
                            // `DERBY` `[0x657400]`, which only the Smash Up code sets (cheat bit 7): `Damage` takes ×1.5, not ×0.5 (0x49452F)
                            cop.body.derby = self.cheat_bits & CHEAT_DERBY != 0;
                            cop.body.rally = self.cheat_bits & CHEAT_RALLY != 0;
                            cars.push(cop);
                        }
                        if !cars.is_empty() {
                            let one_way = nfs_game::police::one_way_road(t);
                            tracing::info!(
                                cars = cars.len(),
                                models = self.road.police_models.len(),
                                points = file.records.len(),
                                "police on duty"
                            );
                            let mut pol =
                                nfs_game::police::Police::new(file, cars, self.race_skill, one_way);
                            // the same `WEATHER` the traffic set gets — `[0x657A24]`, which the
                            // race load zeroes (0x414DF9, `traffic.md` §33.4), and 0x4081B0 scales
                            // every AI car's rate by it
                            pol.weather = false;
                            pol.budget = nfs_game::police::duty_budget(
                                1 + self.num_opponents as usize,
                                false,
                            );
                            self.road.police = Some(pol);
                        }
                    }
                }
                self.racing_line = self.track.racing_line(&self.paths, self.backwards);
                tracing::info!(
                    opponents = self.road.opponents.len(),
                    racing_line = self.racing_line.as_ref().map(|l| l.points.len()),
                    "opponents placed"
                );
                rot
            }
            None => {
                self.car_pos = world.start;
                glam::Mat3::IDENTITY
            }
        };
        (table, rot, grid_place)
    }

    /// Stage "pools" — after step 9: the sprite, beam, skid, flare, particle and weather draws out of `common.fsh` / `particle.fsh`.
    pub(super) fn race_stage_pools(&mut self, renderer: &mut Renderer) {
        // `GameData/Render/common.fsh`: `SKDP` is the skid marks' texture (the one the original's
        // per-surface table points at throughout, 0x5D00E8) and `SHAD` the shadow every car casts
        // (0x43B960 looks it up by that FourCC)
        // `pair` is the draw's Thrash 0x38 blend (`thrash.md` §6): the skid marks and the shadow
        // keep the state object's default 0, the lamp glare's object gets 1 (0x43CF5B, additive)
        // `common.fsh` is read and decoded once for all the draws below (each used to decode the whole pack)
        let common = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
            .and_then(|path| std::fs::read(&path).ok().and_then(|data| nfs_assets::texture::decode_pack(&path, &data).ok()));
        let sprite = |renderer: &mut Renderer,
                      quad: &nfs_renderer::MeshData,
                      name: &str,
                      pair: u8|
         -> Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)> {
            let images = common.as_ref()?;
            // an exact name first — `common.fsh` has both `spik` and `SPIK` (0x44D320 asks by FourCC)
            let img = images
                .iter()
                .find(|t| t.name.trim() == name)
                .or_else(|| images.iter().find(|t| t.name.trim().eq_ignore_ascii_case(name)))?;
            let tex = renderer.upload_texture_mips(img.width, img.height, &img.mips);
            let mesh = renderer.upload_mesh(quad);
            let material = renderer.create_material(nfs_renderer::MaterialDesc {
                alpha_blend: true,
                cull_back: false,
                depth_test: true,
                blend_pair: pair,
                ..nfs_renderer::MaterialDesc::textured(tex)
            });
            Some((mesh, material))
        };
        let mut quad = nfs_renderer::MeshData::default();
        for (x, y, u, v) in [
            (-0.5f32, -0.5f32, 0.0f32, 1.0f32),
            (0.5, -0.5, 1.0, 1.0),
            (0.5, 0.5, 1.0, 0.0),
            (-0.5, 0.5, 0.0, 0.0),
        ] {
            quad.vertices.push(nfs_renderer::Vertex {
                position: [x, y, 0.0],
                normal: [0.0, 1.0, 0.0],
                uv: [u, v],
                color: [1.0, 1.0, 1.0, 1.0],
            });
        }
        quad.indices = vec![0, 1, 2, 0, 2, 3];
        self.particle_draw = sprite(renderer, &quad, "SKDP", 0);
        self.shadow_draw = sprite(renderer, &quad, "SHAD", 0);
        self.glare_draw = sprite(renderer, &quad, "CARG", 1);
        self.glare_star = [sprite(renderer, &quad, "SPIK", 1), sprite(renderer, &quad, "GLNT", 1)];
        // the beams: 16 meshes of the strip's 18 vertices as a triangle list
        self.lamp_pools.beam_material = Some(renderer.create_material(nfs_renderer::MaterialDesc {
            alpha_blend: true,
            cull_back: false,
            depth_test: true,
            ..nfs_renderer::MaterialDesc::flat()
        }));
        let mut strip = nfs_renderer::MeshData::default();
        strip.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] }; 18];
        for k in 0..8u32 {
            let (n0, f0, n1, f1) = (2 * k, 2 * k + 1, 2 * k + 2, 2 * k + 3);
            strip.indices.extend([n0, f0, n1, f0, f1, n1]);
        }
        self.lamp_pools.beam = (0..32).map(|_| renderer.upload_dynamic_mesh(&strip)).collect();
        // the glare's road reflection 0x451D60: the track flares' four-vertex object `[0x61BCB0]`
        // (0x468240(0xE, 4, 1, 0, 4) at 0x4514AD), their state object `[0x61BCB4]` — 0x5379F0's
        // gouraud, blending on, with the pair set to 1, additive (0x4514E7) — and the texture
        // `[0x61BCB8]`, FourCC `BLUR` (0x52554C42, 0x45157D → 0x44D320), UVs `[0x61BC78]` = (0, 0)
        // (1, 0) (1, 1) (0, 1) (0x4514F5…0x451594); a pool of 32 quads rewritten in place
        let mut refl = nfs_renderer::MeshData::default();
        refl.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] }; 4];
        refl.indices = vec![0, 1, 2, 0, 2, 3];
        self.lamp_pools.reflection_material = sprite(renderer, &quad, "BLUR", 1).map(|(_, m)| m);
        self.lamp_pools.reflection = (0..64).map(|_| renderer.upload_dynamic_mesh(&refl)).collect();
        // the skid marks' strips: MAX_CAPACITY vertices each, a triangle strip as a list
        let cap = nfs_game::skidmarks::MAX_CAPACITY as u32;
        let mut skid = nfs_renderer::MeshData::default();
        skid.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] }; cap as usize];
        skid.indices = (0..cap - 2).flat_map(|k| [k, k + 1, k + 2]).collect();
        // a first pool; the draw adds meshes when more strips are in range
        self.skid_pool = (0..160).map(|_| renderer.upload_dynamic_mesh(&skid)).collect();
        // the track flares' definitions and their textures, out of `common.fsh` by the exact
        // name the loader swapped into place (the pack has both `spik` and `SPIK`)
        self.flare_defs = std::fs::read(self.paths.render_dir().join("flare.dat")).map(|d| nfs_formats::flare::parse(&d)).unwrap_or_default();
        self.flare_sprites.clear();
        self.sun_sprites.clear();
        self.lens_sprites.clear();
        // the lens flare's per-race object (0x4601F0 → 0x460780): its element positions
        // `flarepos.dat`, the `LENSFLARE` option, and the surface copy its read-back needs
        self.lens_reset();
        // the launch's accessor dump (0x4ED653 → 0x4B7F10 → 0x4BC3D0) leaves
        // `ADJUSTABLEVIEWDISTANCE` `[0x657D70]` = `Graphics.ViewDistance`'s selection, and 0x44F1A0
        // scales the `.key` draw distance by `{0.5, 0.67, 0.83, 1.0}[0x657D70]` (0x5CE90C). The
        // port took 1.0 (`NFS_DETAIL` stays the diagnostic override).
        if nfs_game::config::cfg().nfs_detail.as_str().is_none()
            && let Some((i, _)) = self.choice_index(choice::GRAPHICS_VIEW_DISTANCE)
        {
            self.detail = [0.5f32, 0.67, 0.83, 1.0][i.min(3)];
        }
        renderer.set_lens_probe(self.lens.enabled);
        if let Some(images) = common.as_ref() {
            let mut names: Vec<String> = self.flare_defs.iter().flat_map(|d| d.textures.iter().flatten().cloned()).collect();
            names.sort();
            names.dedup();
            for name in names {
                let Some(img) = images.iter().find(|t| t.name.trim() == name) else {
                    tracing::debug!(name, "flare texture not in common.fsh");
                    continue;
                };
                let tex = renderer.upload_texture_mips(img.width, img.height, &img.mips);
                let mesh = renderer.upload_mesh(&quad);
                // the flares' state object `[0x61BCB4]` is set to blend 1, additive, at 0x4514E7
                let material = renderer.create_material(nfs_renderer::MaterialDesc {
                    alpha_blend: true,
                    cull_back: false,
                    depth_test: true,
                    blend_pair: 1,
                    ..nfs_renderer::MaterialDesc::textured(tex)
                });
                // the sun draws its layers inside the sky pass (0x460290), under that pass's
                // `ZFUNC` `ALWAYS` and no depth writes, with its own state object — 0x5379F0's
                // defaults with the pair set to 1, additive (0x46088E / 0x46089D). The lens flare's
                // thirteen elements (0x460460) are drawn by the same 0x451710 through state objects
                // made the same way (the loop 0x46087E…0x4608ED builds fourteen pairs alike), so
                // every flare texture gets the material, whichever record uses it.
                let sun = renderer.create_material(nfs_renderer::MaterialDesc {
                    alpha_blend: true,
                    cull_back: false,
                    depth_test: false,
                    blend_pair: 1,
                    ..nfs_renderer::MaterialDesc::textured(tex)
                });
                if self.flare_defs.iter().any(|d| d.id == SUN_FLARE && d.textures.iter().flatten().any(|t| *t == name)) {
                    self.sun_sprites.insert(name.clone(), (mesh, sun));
                }
                self.lens_sprites.insert(name.clone(), (mesh, sun));
                self.flare_sprites.insert(name, (mesh, material));
            }
        }
        // `HEAD` — what `Headlights::LayerUVLight` projects (0x452645 fetches it by that
        // four-character code into `[0x61E1D4]`, whatever the night flag says)
        if let Some(images) = common.as_ref()
            && let Some(img) = images.iter().find(|t| t.name.trim() == "HEAD")
        {
            let tex = renderer.upload_texture_mips(img.width, img.height, &img.mips);
            renderer.set_headlight_texture(Some(tex));
        }
        // the windscreen drops (0x4577A0 from 0x458FF5): `SPLT` (0x544C5053) of the shared bundle
        // `[0x613E04]` = `common.fsh` (0x448B10's first of two, 0x5CDEAC); the state object
        // 0x5379F0 with the pair set to 1, additive (0x4578B4); the draw object 0x468240(9, 3, 1,
        // 0, 0) over 201 triangles; no depth writes (0x459200 turns them off around the draw)
        self.splat_draw = None;
        if let Some(images) = common.as_ref()
            && let Some(img) = images.iter().find(|t| t.name.trim() == "SPLT")
        {
            let tex = renderer.upload_texture_mips(img.width, img.height, &img.mips);
            let material = renderer.create_material(nfs_renderer::MaterialDesc {
                alpha_blend: true,
                cull_back: false,
                depth_test: true,
                blend_pair: 1,
                ..nfs_renderer::MaterialDesc::textured(tex)
            });
            let n = nfs_game::weather::SPLAT_SLOTS * 3;
            let mut mesh = nfs_renderer::MeshData::default();
            mesh.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] }; n];
            mesh.indices = (0..n as u32).collect();
            self.splat_draw = Some((renderer.upload_dynamic_mesh(&mesh), material));
        }
        // the particle streaks (0x454EC0): untextured (`[0x6221D8]+0x40 = 0`, 0x454ECC), a triangle
        // list (0x59F1D0 type 3) in the particle's colour, under the definition's blend
        {
            let n = STREAK_CAP * 3;
            let mut mesh = nfs_renderer::MeshData::default();
            mesh.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] }; n];
            mesh.indices = (0..n as u32).collect();
            let pool = (0..8).map(|_| renderer.upload_dynamic_mesh(&mesh)).collect();
            let mut material = |pair: u8| {
                renderer.create_material(nfs_renderer::MaterialDesc { alpha_blend: true, cull_back: false, depth_test: true, blend_pair: pair, ..nfs_renderer::MaterialDesc::flat() })
            };
            let materials = [material(0), material(1), material(2)];
            self.streak_draw = Some((pool, materials));
        }
        // the track's particle systems, started by its kind-3 `ef` parts (`particles.md` §6); their
        // budget `[0x6197E0]` is set at 0x44ED10 by `SPECIALFX` `[0x657D6C]` (the registry record
        // 0x4B1284: slot 0x47): 0 → 0, 1 → 100, 2 → 200, else 256 (0x44EDEA…0x44EECC)
        let defs = std::fs::read(self.paths.render_dir().join("particle.dat")).map(|d| nfs_formats::particle_dat::parse(&d)).unwrap_or_default();
        // 0x455570 then loads `carpart.dat` the same way into the same map — the cars' own nine
        // systems (`particles.md` §11), with their textures in the same `particle.fsh`
        let car_defs = std::fs::read(self.paths.render_dir().join("carpart.dat")).map(|d| nfs_formats::particle_dat::parse(&d)).unwrap_or_default();
        self.track_particle_sprites.clear();
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "particle.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
        {
            // one material per texture and blend: the definition's `+0x28` is the draw's Thrash
            // state 0x38 (0x454756…0x454766 → 0x537A50; `thrash.md` §6) — 0 the plain alpha blend,
            // 1 additive, 2 darkening by alpha, as shipped (45 / 16 / 1 of the 62)
            let mut keys: Vec<(String, u32)> = defs.iter().chain(car_defs.iter()).map(|d| (d.material.clone(), d.blend)).collect();
            keys.sort();
            keys.dedup();
            let mut textures = std::collections::HashMap::new();
            for (name, blend) in keys {
                let Some(img) = images.iter().find(|t| t.name.trim() == name) else {
                    tracing::debug!(name, "particle texture not in particle.fsh");
                    continue;
                };
                let tex = *textures.entry(name.clone()).or_insert_with(|| renderer.upload_texture_mips(img.width, img.height, &img.mips));
                let mesh = renderer.upload_mesh(&quad);
                let material = renderer.create_material(nfs_renderer::MaterialDesc {
                    alpha_blend: true,
                    cull_back: false,
                    depth_test: true,
                    blend_pair: blend.min(3) as u8,
                    ..nfs_renderer::MaterialDesc::textured(tex)
                });
                self.track_particle_sprites.insert((name, blend), (mesh, material));
            }
            // the weather's two draws (0x458C90): the texture `sno<F4>` / `ran<F4>` of the particle
            // bundle `[0x613E08]`, else `snow` / `rain` (no shipped pack has the first); the state
            // object 0x5379F0 with the pair set to 1, additive (0x456FDB) — gouraud, cull 0, bias 0
            // — and no depth writes (0x459020 turns them off around the draw); white vertices
            // (0x456F9C) and 1203 slots, a triangle list of `CAPACITY` triangles
            let level = self.special_fx_level();
            for (k, params) in [nfs_game::weather::Params::snow(0.0), nfs_game::weather::Params::rain(0.0)].iter().enumerate() {
                let img = params.texture_names(level).iter().find_map(|n| images.iter().find(|t| t.name.trim() == n.as_str()));
                self.weather_draws[k] = img.map(|img| {
                    let tex = renderer.upload_texture_mips(img.width, img.height, &img.mips);
                    let material = renderer.create_material(nfs_renderer::MaterialDesc {
                        alpha_blend: true,
                        cull_back: false,
                        depth_test: true,
                        blend_pair: 1,
                        ..nfs_renderer::MaterialDesc::textured(tex)
                    });
                    let n = nfs_game::weather::CAPACITY * 3;
                    let mut mesh = nfs_renderer::MeshData::default();
                    mesh.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] }; n];
                    mesh.indices = (0..n as u32).collect();
                    (renderer.upload_dynamic_mesh(&mesh), material)
                });
            }
        }
        // made on the first update, once the track is up
        self.track_particle_defs = defs;
        self.track_particles = None;
        self.car_effect_defs = car_defs;
        self.car_effects = None;
    }
}
