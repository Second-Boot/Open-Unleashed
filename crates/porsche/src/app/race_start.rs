//! Starting a race: the front end's launch paths, the test drive, the career race and `start_race`.

use crate::*;

impl App {
    /// Start the race the front end's own settings describe (the same path the text menu takes).
    /// The "evenstvn" name-box code, cheat bit 3: the launch's per-car routine 0x4EEDA0 sends every
    /// car but a traffic one (kind 4) `.SIM` 0x13E = `"993coupe36"` and the model 0x13F = `"993"`
    /// in place of the record's own `+0xB5` / `+0x119` (0x4EEE9B…0x4EEEC6) — the player's car, the
    /// opponents and the cops all become 993 coupés; everything else of the record stays.
    pub(crate) fn cheat_993(&self, mut car: nfs_formats::fedata::CarCatalogEntry) -> nfs_formats::fedata::CarCatalogEntry {
        if self.cheat_bits & (1 << 3) != 0 {
            car.model = "993".to_string();
            car.sim = "993coupe36".to_string();
        }
        car
    }

    /// The "Smash Up" name-box code, cheat bit 7 (`nfs_game::cheats`): after every mode's own
    /// options the launch appends `DERBY` 0x1B = 1, `LINEAR_TRACK` 0x1D = 0, `RACE_TYPE` 0x10 = **3**
    /// (the lap knockout), `TRACK_NAME` 0x10F = `"Skidpad"`, `DAMAGE` 0x20 = 1 and `NOCOLLISION`
    /// 0x29 = 0 (0x4ED7D9…0x4ED841), so whatever was chosen runs as a lap knockout on the skid pad
    /// with the mode's own lap count. `DERBY` itself is read by the network grid spacing
    /// (`modes.md` §1), by `Damage` 0x494500 (×1.5 instead of ×0.5, 0x49452F — `Vehicle::derby`),
    /// by the lap-knockout pass 0x416407 (a car with all eight ring zones over 30 is out — not
    /// carried) and by `HPosition` 0x42B2D5 (not carried); this port's races always take damage.
    /// *(Corrected 2026-09-26: this said the grid was its only reader.)*
    pub(crate) fn derby_track(&mut self, track: &str) -> String {
        if self.cheat_bits & (1 << 7) == 0 {
            return track.to_string();
        }
        self.quick_knockout = true;
        "skidpad".to_string()
    }

    /// A test drive, `Game.GameType` 6 / 7 / 8. The car is 0x4EB710's per type — 6 a new
    /// `DealerCar.Car`, built fresh (0x4EB7C3…0x4EB859), 7 `UsedCar.Car` and 8 `OwnedCars.Car`,
    /// each the record as it stands (0x4EB7A6, 0x4EB85E → 0x4EB727); the race is 0x4FC0B0's for
    /// 6…8 — `Track1.Track`, one lap, forward, **five** traffic cars — with **no** opponents
    /// (0x4ECF00) and no cops.
    pub(crate) fn start_test_drive(&mut self, game_type: u8, event_loop: &ActiveEventLoop) {
        let Some(m) = self.fe.menu.as_ref() else { return };
        // 0x4FC0B0's arms for the two tournament drives (`modes.md` §2): 5 is `prerace.lay`'s
        // `TESTDRIVE` (0x5060D0) on the event `Tournament.RemainingTracks` has selected, and 11 is
        // `VictoryRoom.lay`'s (0x506C70) on the finale's event (0x504990) — the track, its laps
        // (0 → 1), `evt+0x3A4` / `+0x3A5` reverse / mirror; traffic none for 5 and for 11 the
        // tournament's, as a mode-4 race takes it (count of `trn+0x382` if `trn+0x380`, else
        // `{0, 2, 4, 8}[trn+0xE0]`); no opponents for either (0x4ECF00)
        let event: Option<(String, u32, bool, bool, u32, Option<Vec<u16>>)> = match game_type {
            5 | 11 => self.career.as_ref().and_then(|c| {
                let t = if game_type == 11 { c.career.finale_tournament(&c.tables)? } else { c.current()? };
                let done = c.career.tournaments.get(&t.id).map_or(0, |s| s.events.len());
                let ev = if game_type == 11 {
                    t.events.get(done.min(t.events.len().saturating_sub(1)))?
                } else {
                    // `[0x691E7C]`, which 0x504F00 set to the row's undone event
                    t.events.get(c.career.remaining_event(t, self.fe.remaining_sel)?)?
                };
                let file = c.tables.track(ev.track_id)?.file.clone();
                let (traffic, ids) = if game_type == 5 {
                    (0, None)
                } else {
                    match &t.custom_traffic {
                        Some(ids) => (ids.len() as u32, Some(ids.iter().map(|id| *id as u16).collect())),
                        None => (career_ui::CAREER_TRAFFIC[(t.traffic_level as usize).min(3)], None),
                    }
                };
                Some((file, ev.laps.max(1) as u32, ev.backwards, ev.direction_flag != 0, traffic, ids))
            }),
            _ => None,
        };
        if matches!(game_type, 5 | 11) && event.is_none() {
            return;
        }
        let track_name = event.as_ref().map_or_else(|| m.track().to_string(), |e| e.0.clone());
        let (car, owned) = match game_type {
            // the finale is driven in its loaned prize car (0x4EB710 modes 10 / 11)
            11 => {
                let Some(car) = self.career.as_ref().and_then(|c| {
                    let t = c.career.finale_tournament(&c.tables)?;
                    c.tables.car(t.prize_car as u16).cloned()
                }) else { return };
                (car, None)
            }
            // a new car's test drive is the dealership's car in a career, in the rows chosen on it
            6 => {
                let Some(car) = self.dealer_car().or(m.car()).cloned() else { return };
                (car, None)
            }
            7 => {
                let Some(o) = self.used_car().cloned() else { return };
                let Some(car) = self.career.as_ref().and_then(|c| c.tables.car(o.car_id)).cloned() else { return };
                (car, Some(o))
            }
            _ => {
                let Some(c) = self.career.as_ref() else { return };
                let Some(o) = c.career.garage.get(c.car_sel).cloned() else { return };
                let Some(car) = c.tables.car(o.car_id).cloned() else { return };
                (car, Some(o))
            }
        };
        let dealer = (game_type == 6 && self.dealer_cars().is_some()).then_some(self.fe.dealer);
        let paint_set = owned.as_ref().map_or(dealer.map_or(m.paint_idx, |d| d.paint), |o| o.paint_set as usize);
        self.paint = car.paint_sets.get(paint_set % car.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
        self.owned_decal = owned.as_ref().map(|o| o.trim.decal);
        self.owned_paint = match (owned.as_ref(), dealer) {
            (Some(o), _) if !car.paint_sets.is_empty() => Some(Self::paint_of(&car, o)),
            (None, Some(d)) if !car.paint_sets.is_empty() => Some(Paint::owned(&car, d.paint, d.interior, 0, 0, 0)),
            (None, None) => Self::menu_custom_paint(m, &car),
            _ => None,
        };
        // a colour that is not a row (`OwnedCar::custom`) is the paint the race compares against
        if let Some(p) = self.owned_paint {
            self.paint = p.exterior;
        }
        self.num_opponents = 0;
        self.num_laps = 1;
        self.backwards = false;
        self.mirror = false;
        self.road.traffic_count = 5;
        self.road.traffic_ids = None;
        if let Some((_, laps, rev, mir, traffic, ids)) = event {
            self.num_laps = laps;
            self.backwards = rev;
            self.mirror = mir;
            self.road.traffic_count = traffic;
            self.road.traffic_ids = ids;
        }
        self.road.cops_enabled = false;
        self.road.cop_count = 4;
        self.road.cops_race = false;
        self.career_opponents = None;
        self.quick_knockout = false;
        // the player's `nfs5.car` record is the car 0x4EB710 took for this type — what the dash
        // calibration and the replay's car read (`player_car_id`); until 2026-10-01 a test drive
        // kept the last race's record here (EV1 audit)
        self.player_car_id = Some(car.id);
        // `RACE_TYPE` 0 for 5…8 and 11: `Game.Difficutly` / `Game.Damage` (0x4EB56C), the camera list
        self.launch_skill_damage(game_type, self.race_skill, false);
        let car = self.cheat_993(car);
        // a record car's parts write the style overlay (0x49C5F7), as in a career race
        let swaps: Vec<[i16; 3]> = match (owned.as_ref(), self.career.as_ref()) {
            (Some(o), Some(cu)) => o.parts.iter().filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id)).flat_map(|p| p.geometry_swaps.iter().copied()).collect(),
            _ => Vec::new(),
        };
        // 0x4D52EA: the launch saves the profile once the start check said yes
        self.launch_save();
        tracing::info!(track = track_name, car = car.model, game_type, "front end: test drive");
        match (
            { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
            CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio).with_style_overlay(swaps.clone())),
            CarAssets::load_sim(&self.paths, if car.sim.is_empty() { "default" } else { &car.sim }),
        ) {
            (Ok(t), Ok(c), Ok(sim)) => {
                self.track = t;
                self.car = c;
                let race_class = car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                self.specs = CarSpecs::from_sim(&sim, race_class);
                // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                if self.cheat_bits & (1 << 1) != 0 {
                    self.specs.apply_tinycar();
                }
                // a record car brings its parts, their wear and its setup, as a career race does
                if let (Some(o), Some(cu)) = (owned.as_ref(), self.career.as_ref()) {
                    let parts: Vec<&nfs_formats::career::Part> = o.parts.iter().filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id)).collect();
                    self.specs.apply_parts(&parts, &car.sim_family);
                    self.specs.parts = nfs_game::career::installed_parts(&parts, Some(o));
                    self.specs.apply_setup(&o.setup, self.player_automatic());
                } else {
                    // no owned/career state: a freshly built catalogue car — its stock parts
                    // (0x4EE9A0) and the constructor's neutral sliders (0x4EED2D) through 0x49CB50
                    let table = self.fe_parts_table();
                    let automatic = self.player_automatic();
                    nfs_game::career::fold_fresh_car(&mut self.specs, &car.stock_parts, &table, &car.sim_family, automatic);
                }
                self.snd.sound_set = car.sound_set.clone();
                self.sim_name = car.sim.clone();
                self.dials = Some((car.dial_kmh, car.dial_mph, car.dial_tach));
                self.fe.test_drive = Some((game_type, owned));
                // `prerace`'s 0x5060D0 and `VictoryRoom`'s 0x506C70 write `Game.GameType` (5, 11);
                // 6…8 are the override `[0x5DC1C0]` the launch reads (0x50C110) — whether the
                // front end's own value keeps them afterwards is not read, so it is left alone
                if matches!(game_type, 5 | 11) {
                    self.fe.game_type = game_type;
                }
                self.fe.race_game_type = Some(game_type);
                self.fe.menu_template = self.fe.menu.take();
                self.fe.layout_preview = None;
                self.fe.layout_name = None;
                let (renderer, window) = (self.renderer.take().unwrap(), self.window.take().unwrap());
                self.tick = 0;
                self.start_race(renderer, window, event_loop);
            }
            (t, c, s) => tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "test drive load failed"),
        }
    }

    /// The player's car-info record as the launch left it (`.TYPE`, the paint, the installed parts and their wear, the
    /// seventeen sliders): what [`CarSpecs::apply_parts`] / [`CarSpecs::apply_setup`] were last given for this
    /// race's car and `Car_raceInit`'s copy of `.PARTDAMAGE` (0x41085F). `None` for a network race (its own
    /// launch words ride in [`nfs_game::replay::NetSetup`]) and for a car no launch path folded.
    pub(crate) fn launch_player_record(&self) -> Option<nfs_game::replay::CarRecord> {
        if self.net.session.is_some() || self.replay_net.is_some() || (self.specs.applied_setup.is_none() && self.specs.applied_part_ids.is_empty()) {
            return None;
        }
        Some(nfs_game::replay::CarRecord {
            car_id: self.player_car_id.unwrap_or(self.car_id),
            paint: self.paint,
            name: self.player_name.clone(),
            decal: -1,
            parts: Some(self.specs.applied_part_ids.clone()),
            part_damage: self.specs.parts.iter().map(|p| p.carried).collect(),
            setup: self.specs.applied_setup,
            ..Default::default()
        })
    }

    /// A career race's setup from its [`career_ui::StartRequest`] — a tournament event, a
    /// finale or a Factory Driver mission — up to the point the race starts: the options 0x4ED5D0
    /// sends (laps, direction, traffic, grid, skill, the opponents), then the track, the car with
    /// its parts' style overlay, the `.sim` with the parts, their wear and the setup. `false` when
    /// the assets do not load; the event is no longer pending then. The callers start the race.
    pub(crate) fn load_career_race(&mut self, req: career_ui::StartRequest) -> bool {
        self.num_opponents = req.opponents.len() as u32;
        self.num_laps = req.laps;
        self.owned_paint = None;
        self.owned_decal = None;
        self.paint = req.paint;
        self.career_opponents = Some(req.opponents.clone());
        self.road.traffic_count = req.traffic;
        self.road.traffic_ids = req.traffic_ids.clone();
        self.player_grid_override = req.player_grid;
        // **Career police.** 0x4ED0A5 gives cops only to game type 9 — a factory-driver
        // mission — and only when its record's `+0x26` is set, and then two of them
        // (0x4ED0BC…0x4ED0E7); a tournament has none. This path used to leave
        // `cops_enabled` at whatever the last menu race had.
        let mission_cops = req.mission.as_ref().is_some_and(|m| m.mission.police);
        self.road.cops_enabled = mission_cops;
        self.road.cop_count = 2;
        self.race_skill = req.skill;
        self.backwards = req.backwards;
        self.mirror = req.mirror;
        // the dials' calibration is the player's own `nfs5.car` record (a quick race's choice
        // used to linger here)
        self.player_car_id = Some(req.car.id);
        self.quick_knockout = false;
        self.road.cops_race = false;
        let car = self.cheat_993(req.car.clone());
        self.mission = req.mission.clone().map(cone_limit_diagnostic);
        self.skill = None;
        match (
            { let derby = self.derby_track(&req.track_file); TrackAssets::load(&self.paths, &derby) },
            CarAssets::load(&self.paths, &car.model).map(|c| {
                c.with_style(car.variant)
                    .with_cabrio(car.cabrio)
                    .with_style_overlay(req.parts.iter().flat_map(|p| p.geometry_swaps.iter().copied()).collect())
            }),
            CarAssets::load_sim(&self.paths, &car.sim),
        ) {
            (Ok(mut t), Ok(c), Ok(sim)) => {
                if let Some(ms) = &req.mission
                    && let Some(suffix) = &ms.scene_suffix
                {
                    match t.load_scene(suffix) {
                        Ok(Some(scene)) => {
                            self.skill = Some(SkillTest::from_scene(&scene, &t.name));
                            t.extra_scenes.push(scene);
                        }
                        other => {
                            tracing::error!(suffix, result = ?other.err(), "skill-test scene missing")
                        }
                    }
                }
                self.track = t;
                self.car = c;
                let race_class =
                    car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                self.specs = CarSpecs::from_sim(&sim, race_class);
                // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                if self.cheat_bits & (1 << 1) != 0 {
                    self.specs.apply_tinycar();
                }
                // the purchased parts, then the setup sliders — the order 0x49CB50 runs
                // them in, and both of them before anything reads the block
                // A car with no garage record behind it and no parts of its own (the era finale's
                // loaned prize car) is a freshly built `FECar`: its stock parts (0x4EE9A0) and the
                // neutral sliders (0x4EED2D) go through 0x49CB50 — `fold_fresh_car`.
                let fresh_stock = req.parts.is_empty()
                    && self.career.as_ref().is_some_and(|cu| cu.pending.as_ref().is_none_or(|p| p.garage_idx.is_none()));
                let grip_ratio = if fresh_stock {
                    0.0
                } else {
                    self.specs.apply_parts(&req.parts.iter().collect::<Vec<_>>(), &car.sim_family)
                };
                tracing::debug!(parts = req.parts.len(), grip_ratio, "parts applied");
                if fresh_stock {
                    let table = self.fe_parts_table();
                    let automatic = self.player_automatic();
                    nfs_game::career::fold_fresh_car(&mut self.specs, &car.stock_parts, &table, &car.sim_family, automatic);
                } else if let Some(cu) = &self.career {
                    // the installed-part list `car+0x768` the damage getters walk, each part
                    // with its own zone mask and the damage it carries from earlier races
                    let owned = cu
                        .pending
                        .as_ref()
                        .and_then(|p| p.garage_idx)
                        .and_then(|i| cu.career.garage.get(i));
                    self.specs.parts = nfs_game::career::installed_parts(
                        &req.parts.iter().collect::<Vec<_>>(),
                        owned,
                    );
                    // …and the car-setup screen's seventeen sliders reach the block
                    // here, as 0x49D290 folds them in out of the per-player record — the owned
                    // car's own, else the constructor's neutral ones (a mission's car is a fresh
                    // `FECar`, 0x4EED2D). `automatic` is `carinfo+0xC`, the manual ×1.05 of 0x49D733
                    self.specs.apply_setup(owned.map_or(&nfs_game::career::CAR_SETUP_DEFAULT, |o| &o.setup), self.player_automatic());
                }
                self.snd.sound_set = car.sound_set.clone();
                self.sim_name = car.sim.clone();
                self.dials = Some((car.dial_kmh, car.dial_mph, car.dial_tach));
                true
            }
            (t, c, s) => {
                tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "career event load failed");
                if let Some(cu) = self.career.as_mut() {
                    cu.pending = None;
                }
                self.career_opponents = None;
                false
            }
        }
    }

    /// **`PLAYGAME` for `Game.GameType` 4 on the layout front end** (`prerace.lay`,
    /// `tournament.lay`): the start check 0x4D5730 and the fee (`CareerUi::start_selected_event`,
    /// its dialogs raised by `fe_career_dialog`), then the race 0x4ED5D0 builds for the selected
    /// tournament's event in the garage car `OwnedCars.Car` — the car the race end credits
    /// (0x4EDE7A…0x4EDEA3) and whose own paint, trim and race number it wears, as a type-8 test
    /// drive's does. The front end is kept for the way back (`fe_after_race`), whose type-4
    /// branch reads the flags the launch and the filing leave (`CareerUi::race_flags`).
    /// Returns whether a race started.
    /// The era finale's launch from `VictoryRoom` (game type 10): the loaned prize car in its
    /// first paint set (0x4EB710), no garage car (`[0x690ADC] = 0`, 0x4EDEAA).
    fn start_finale_from_layout(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // the side state for the launch's save, which `start_finale` makes (0x4D52EA)
        self.sync_save_side_state();
        let Some(cu) = self.career.as_mut() else { return false };
        let Some(req) = cu.start_finale() else { return false };
        let skill = req.skill;
        if !self.load_career_race(req) {
            return false;
        }
        self.launch_skill_damage(10, skill, false);
        self.fe.menu_template = self.fe.menu.take();
        self.fe.race_game_type = Some(10);
        self.fe.layout_preview = None;
        self.fe.layout_name = None;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return false };
        self.tick = 0;
        self.start_race(renderer, window, event_loop);
        true
    }

    pub(crate) fn start_tournament_from_layout(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // the side state for the launch's save, which `start_selected_event` makes (0x4D52EA)
        self.sync_save_side_state();
        let Some(cu) = self.career.as_mut() else { return false };
        if cu.career.garage.is_empty() {
            return false;
        }
        let g = cu.car_sel.min(cu.career.garage.len() - 1);
        let row = self.fe.remaining_sel;
        let Some(req) = cu.start_selected_event(g, row) else { return false };
        let owned = cu.career.garage.get(g).cloned();
        let entry = owned.as_ref().and_then(|o| cu.tables.car(o.car_id)).cloned();
        let skill = req.skill;
        if !self.load_career_race(req) {
            return false;
        }
        self.launch_skill_damage(4, skill, false);
        if let (Some(o), Some(e)) = (owned.as_ref(), entry.as_ref())
            && !e.paint_sets.is_empty()
        {
            self.owned_paint = Some(Self::paint_of(e, o));
            self.owned_decal = Some(o.trim.decal);
        }
        self.fe.menu_template = self.fe.menu.take();
        self.fe.race_game_type = Some(4);
        self.fe.layout_preview = None;
        self.fe.layout_name = None;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return false };
        self.tick = 0;
        self.start_race(renderer, window, event_loop);
        true
    }

    /// **`PLAYGAME` for `Game.GameType` 9** (`factorydriver.lay`, `factdone.lay`, both
    /// `canplaygame=1`): 0x4D5730 lets type 9 through without a check (0x4D57AA → 0x4D59EA →
    /// 0x4D5AA3, whose test is types 4 / 5 only) and the launch builds the mission 0x51AE80 picks —
    /// the first record whose result `+0xE8` is 0 (0x51AE99), **record 0 when every one is
    /// passed** (0x51AEAA) — in `FactoryDriver.Car` (0x4EB710), with 0x4EC040's mission options
    /// (`CareerUi::mission_request`, `load_career_race`). *(Until 2026-10-01 the layout front end
    /// had no type-9 launch: `PLAYGAME` on these screens raced the Quick Race's settings.)*
    fn start_mission_from_layout(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.career.is_none() {
            return false;
        }
        self.launch_save();
        let Some(cu) = self.career.as_mut() else { return false };
        let idx = cu.career.current_mission(&cu.tables).unwrap_or(0);
        let Some(req) = cu.mission_request(idx) else { return false };
        let skill = req.skill;
        // 0x4EB51C…0x4EB544: `RACE_TYPE` 0 when any of the five `+0xD4` words is set, else 4
        let opponents = req.mission.as_ref().is_some_and(|m| m.mission.opponents.iter().any(|(per, _)| *per != 0));
        if !self.load_career_race(req) {
            return false;
        }
        self.launch_skill_damage(9, skill, opponents);
        self.fe.menu_template = self.fe.menu.take();
        self.fe.race_game_type = Some(9);
        self.fe.layout_preview = None;
        self.fe.layout_name = None;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return false };
        self.tick = 0;
        self.start_race(renderer, window, event_loop);
        true
    }

    /// **0x4D52DE…0x4D52EA: the launch saves the profile** — `SaveInfo(getPlayerName(), picture)`
    /// on the way out of the front end after 0x4D5730 said yes, for **every** game type (the only
    /// branch before it is the knockout start, 0x4D52CB). The side state (series, playlist,
    /// settings) goes in as it stands. *(Until 2026-10-01 a test drive and a mission saved nothing,
    /// and a tournament or finale launch saved the career without the side state.)*
    pub(crate) fn launch_save(&mut self) {
        self.sync_save_side_state();
        if let Some(c) = self.career.as_mut() {
            c.save();
        }
    }

    /// **0x4EB470's `SKILL` (0x12) and `DAMAGE` (0x20)**, and the player car's camera list — what
    /// every launch writes afresh. The launch fills a new option buffer for every race (0x4ED5D0 →
    /// 0x4EB470 → 0x4ECDF0 → 0x4ECF00), so nothing of the last race's options survives in the
    /// original; the port's launch paths each write the fields themselves, and these three were
    /// written by the single-race path alone (EV1, 2026-10-01: an Evolution race after a Quick Race
    /// with damage off ran without damage, a test drive kept the last race's skill).
    ///
    /// `RACE_TYPE` by `Game.GameType` (jump table 0x4EB6C0: 0 → 0, 1 with cheat 5 at 0x4EB4FD;
    /// 1 → 3; 2, 4 → 2; 9 → 0 with opponents else 4, 0x4EB51C; 3, 5…8, 10, 11 → 0), then by
    /// `RACE_TYPE` (bytes 0x4EB704 → table 0x4EB6F0):
    /// * 0, 1, 3 — **0x4EB56C**: `SKILL` = type 9 ? mission `+0x56` (0x4EB57A) : `Game.Difficutly`
    ///   (0x4EB5A8); `DAMAGE` = `Game.Damage` (0x4EB5D5) — the finale (10) and every test drive too;
    /// * 2 — **0x4EB5DF**: type 2 `Game.Damage` / `Game.Difficutly`, else (type 4) the tournament's
    ///   `+0xE6` (0x4EB64F) and `DAMAGE` 1 (0x4EB659);
    /// * 4 — **0x4EB66B**: mission `+0x56`, `DAMAGE` 1.
    ///
    /// The camera list: 0x4EB710 → 0x4EEDA0 appends `.CAMERA` from `Camera.1…4` for slot 0 at every
    /// launch (0x4EF02A…0x4EF0F5), whatever the type (`start_from_layout`'s own comment).
    pub(crate) fn launch_skill_damage(&mut self, game_type: u8, own_skill: u8, opponents: bool) {
        let menu = self.fe.menu.as_ref().or(self.fe.menu_template.as_ref());
        let (difficulty, damage) = menu.map_or((self.race_skill, self.road.damage_on), |m| (m.difficulty.min(2), m.damage));
        (self.race_skill, self.road.damage_on) = match game_type {
            4 => (own_skill, true),
            9 if opponents => (own_skill, damage),
            9 => (own_skill, true),
            _ => (difficulty, damage),
        };
        self.camera_cycle = self.fe.camera_list.iter().map(|&v| v as u16).collect();
    }

    /// `PLAYGAME` from the front end: in a hosted network game the host's room announces the start
    /// to the players first (0x48BF10, `0xB3`), then the launch goes on as any other, with 0x4B4DA0's
    /// network half (`net_launch_sync`) in front of it. **The order is the original's**: `0xB3`, then
    /// the stream (`0xBC`), then the barrier (`0xBD`).
    pub(crate) fn fe_start_game(&mut self, event_loop: &ActiveEventLoop) {
        // the original's order (`network.md` §11.7): the room's `0xB3` (0x48BF10) first, then each
        // machine's launch - 0x4B4DA0 sends the stream and runs the barrier 0x48D6A0
        if !self.net_playgame() {
            return;
        }
        self.net_launch_sync();
        self.start_from_layout(event_loop);
    }

    pub(crate) fn start_from_layout(&mut self, event_loop: &ActiveEventLoop) {
        // `Game.GameType` 10 — `VictoryRoom`'s `PLAYGAME` — is the era finale (0x4D5730's type-10
        // arm to the launch, `CareerUi::start_finale`). *(Until 2026-09-27 the port ran the quick
        // race's single race here, so the finale could not be raced from the layout front end.)*
        if self.fe.game_type == 10 && self.career.is_some() {
            self.start_finale_from_layout(event_loop);
            return;
        }
        // **Every `PLAYGAME` runs one start check, 0x4D5730, keyed on `Game.GameType`**
        // (`[0x690AAC]` through the accessor, 0x4D5750…0x4D577B): the handler 0x50CE90 calls it at
        // 0x50D274 whichever button was pressed — and the only `PLAYGAME` button the shipped
        // layouts carry is the bottom bar's (`bottombar.lay` `ID_PLAYGAMEBUTTON`). Type 4 (0x4D578C)
        // is the selected tournament's event in `OwnedCars.Car`, type 9 (0x4D57A7) the current
        // Factory Driver mission. *(Until 2026-10-01 only a press on a layout control took type 4
        // here, and nothing took type 9: the bottom bar's button on `tournament` / `prerace` raced
        // the Quick Race's track, car and opponents — user report EV1.)*
        if self.career.is_some() {
            match self.fe.game_type {
                4 => {
                    self.start_tournament_from_layout(event_loop);
                    return;
                }
                9 => {
                    self.start_mission_from_layout(event_loop);
                    return;
                }
                _ => {}
            }
        }
        let Some(m) = self.fe.menu.as_ref() else { return };
        // `zMergedList.Car`'s own car — the garage half's own record while `car_garage` holds
        let Some(car) = self.logo_car(Some("zMergedList.Car"), None).cloned() else { return };
        // **the car's camera list** — the launch chain's first per-car routine (0x4EB710 → 0x4EEDA0,
        // 0x4EBA4D) appends `.CAMERA` 0x148 for slot 0 from `Camera.1 … Camera.4` (0x4EF02A…0x4EF0F5),
        // after `fe.txt`'s `car[0].camera`, and 0x4B6560 keeps the last record of an id, so the list
        // the race's camera reads — `car[slot].camera[0…3]` at `[0x658294 + slot·0x580]`, slot 0:
        // 0x462F30 (0x46321B), 0x465790 (0x465827), 0x465A60 (0x465B72, `0x415AF2` passes 0) —
        // is the four accessors, not the file's four names. `network.md` §11.12c.
        self.camera_cycle = self.fe.camera_list.iter().map(|&v| v as u16).collect();
        // **0x4ED5D0 by `Game.GameType`** (`modes.md` §1, §2): 2 the knockout series' round, 1 the
        // Quick Knockout, anything else the single race. *(Until 2026-09-26 every layout launch
        // was a single race, so `knockout.lay` and `quickknockout.lay` raced as `QuickRace.lay`.)*
        let game_type = self.fe.game_type;
        // 0x4EBBA0's arm for 0 and 1: `OpponentCar.Car`'s choice and the player's
        // `zMergedList.Car` record feed 0x4F4FA0 (`quick_opponents` in the race set-up)
        self.opp_choice = m.opp_car as u32;
        self.player_car_id = Some(car.id);
        // 0x4EB470: `SKILL = Game.Difficutly` for `RACE_TYPE` 0, 1, 3 outside mode 9 and for the
        // knockout series (mode 2) — every game type this launch takes. Before the series start,
        // whose driver draw 0x4E3160 reads it.
        self.race_skill = m.difficulty.min(2);
        let track_name = match game_type {
            2 => {
                // 0x4D52CB: a series is started unless one is running (0x4DDF00 / 0x4DDF30)
                if !self.knockout.as_ref().is_some_and(|k| k.state.active) {
                    self.knockout_start();
                }
                let Some(t) = self.knockout_round() else { return };
                t
            }
            _ => {
                let m = self.fe.menu.as_ref().expect("menu");
                self.player_grid_override = None;
                self.career_opponents = None;
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                self.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
                self.road.traffic_ids = None;
                self.road.damage_on = m.damage;
                self.road.cop_count = 4;
                if game_type == 1 {
                    // 1: `LoopedTrack.Track`, `RACE_TYPE` 3, `NUM_LAPS` = `NUMOPPONENTRACECARS` =
                    // `Game.NumKOOpponents` (0x4ECDF0 / 0x4ECF00, §4); no cops
                    let t = m.looped_track().to_string();
                    self.num_opponents = m.ko_opponents;
                    self.num_laps = m.ko_opponents.max(1);
                    self.road.cops_race = false;
                    self.road.cops_enabled = false;
                    self.quick_knockout = true;
                    t
                } else {
                    let t = m.track().to_string();
                    self.num_opponents = m.opponents;
                    self.num_laps = fe_laps(&m.circuits, &t, m.laps);
                    // the single race, game type 0: the Fuzzyfuz code (bit 5) makes it the cops
                    // race — `RACE_TYPE` 1 (0x4EB4FD) and four cop cars (0x4ED0BC…0x4ED282)
                    self.road.cops_race = self.cheat_bits & (1 << 5) != 0;
                    self.road.cops_enabled = m.cops || self.road.cops_race;
                    self.quick_knockout = false;
                    t
                }
            }
        };
        let track_name = self.net_race_apply(track_name);
        // **0x4D52DE…0x4D52EA: the launch saves the profile** — `SaveInfo(getPlayerName(),
        // picture)` after the knockout start and before the front end hands over, so the file
        // carries the running series (and a tournament's running event, 0x503940)
        self.launch_save();
        let Some(m) = self.fe.menu.as_ref() else { return };
        self.owned_paint = None;
        self.owned_decal = None;
        self.paint = car
            .paint_sets
            .get(m.paint_idx % car.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
        if let Some(p) = Self::menu_custom_paint(m, &car) {
            (self.paint, self.owned_paint) = (p.exterior, Some(p));
        }
        // (`quick_knockout` is the arm's own above — 1 true, 0 and 2 false —, and `derby_track`'s
        // Smash Up code sets it at the track load below; a `false` here, as it stood until
        // 2026-10-01, turned the Quick Knockout's lap cut off in every layout launch of type 1:
        // `racecfg` said `game_type=1 quick_knockout=false`)
        let car = self.cheat_993(car);
        // **A garage car in a layout race** (`Game.GameType` 0, 1, 2, 3): 0x4EB710 takes the owned
        // `FECar` itself for `zMergedList.Car`'s garage half (0x4F4DF0 true → slot 0x8C), not a
        // fresh one built from the catalogue record (0x4EE3B0), and 0x4EEDA0 kind 1 streams its
        // set-up `+0x6D0…` (0x4EF179…0x4EF32F) and its installed parts `+0x758` (`.PARTINDEX`,
        // 0x4EF3F3) exactly as for a career race — `.PARTDAMAGE` / `.DAMAGEZONES` are 0 for these
        // types (0x4EF3E6…0x4EF445). A catalogue car keeps what it had.
        let garage = if self.fe.menu.as_ref().is_some_and(|m| m.car_garage) {
            self.garage_car().zip(self.career.as_ref()).map(|((_, o), cu)| {
                let parts: Vec<nfs_formats::career::Part> = o.parts.iter().take(20).filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id)).cloned().collect();
                (parts, o.setup)
            })
        } else {
            None
        }
        .filter(|(parts, _)| !parts.is_empty());
        let swaps: Vec<[i16; 3]> = garage.iter().flat_map(|(parts, _)| parts.iter().flat_map(|p| p.geometry_swaps.iter().copied())).collect();
        tracing::info!(track = track_name, car = car.model, "front end: start race");
        match (
            { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
            CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio).with_style_overlay(swaps.clone())),
            CarAssets::load_sim(
                &self.paths,
                if car.sim.is_empty() {
                    "default"
                } else {
                    &car.sim
                },
            ),
        ) {
            (Ok(t), Ok(c), Ok(sim)) => {
                self.track = t;
                self.car = c;
                let race_class =
                    car.sim.to_lowercase().starts_with("gt") || car.sim.starts_with("935");
                self.specs = CarSpecs::from_sim(&sim, race_class);
                // `.TINYCAR` (cheat bit 1) at the `.sim` load, 0x49C9B2
                if self.cheat_bits & (1 << 1) != 0 {
                    self.specs.apply_tinycar();
                }
                // the garage car's parts, then its sliders — 0x49CB50's order (`loaders.md` §1.3,
                // `physics.md` §3a), both before anything reads the block
                if let Some((parts, setup)) = &garage {
                    let refs: Vec<&nfs_formats::career::Part> = parts.iter().collect();
                    let before = (self.specs.top_speed, self.specs.final_drive, self.specs.grip, self.specs.gear_ratio[2]);
                    let grip_ratio = self.specs.apply_parts(&refs, &car.sim_family);
                    self.specs.parts = nfs_game::career::layout_race_parts(&refs);
                    self.specs.apply_setup(setup, self.player_automatic());
                    tracing::info!(
                        parts = ?parts.iter().map(|p| p.id).collect::<Vec<_>>(), grip_ratio,
                        setup = ?setup.map(|v| (v * 100.0).round() / 100.0),
                        top_speed_before = before.0, final_drive_before = before.1, grip_before = before.2, first_gear_before = before.3,
                        top_speed = self.specs.top_speed, final_drive = self.specs.final_drive, grip = self.specs.grip,
                        first_gear = self.specs.gear_ratio[2],
                        "layout race: the garage car's installed parts and set-up applied (0x4EEDA0 kind 1, 0x49CB50)"
                    );
                }
                // **A network race's own catalogue car goes through the fold every other machine puts its
                // copy through.** 0x49CB50 folds the car's parts and its `carinfo` set-up into the block for
                // every car of the race, the player's included; the seat build in `start_race` does it from the
                // record the owner sent, and this is that build for the owner's own copy — from the record it
                // sent (`launch.cars` at its own index). Two machines then race one set of numbers per car,
                // which the input exchange needs (`network.md` §11.17). A single-player race's catalogue car takes
                // the same fold from the neutral record (the `else if` below, §11.12c); a garage car took its own above.
                if garage.is_none()
                    && let Some(n) = self.net.session.as_ref()
                    && n.in_race != 0
                    && let Some(idx) = n.local_race_index()
                    && let Some(rec) = n.launch.cars.get(&idx).cloned()
                {
                    use nfs_net::launchcar::id as cid;
                    let parts_table = self.fe_parts_table();
                    let before = (self.specs.grip, self.specs.transfer_div, self.specs.roll_gain);
                    // the same single fold the seat build makes of a record (`start_race`'s `fold_list`): the
                    // record's `.PARTINDEX` when it is not the car's stock list, else the stock list itself
                    let ids: Vec<i16> = rec.parts().into_iter().map(|p| p as i16).collect();
                    if !ids.is_empty() && ids != car.stock_parts {
                        let owned: Vec<&nfs_formats::career::Part> = ids.iter().filter_map(|id| parts_table.iter().find(|p| p.id == *id)).collect();
                        self.specs.apply_parts(&owned, &car.sim_family);
                        self.specs.parts = nfs_game::career::installed_parts(&owned, None);
                    } else {
                        let stock: Vec<&nfs_formats::career::Part> = car.stock_parts.iter().filter_map(|id| parts_table.iter().find(|p| p.id == *id)).collect();
                        self.specs.apply_parts(&stock, &car.sim_family);
                        self.specs.parts = nfs_physics::specs::stock_parts(&car.stock_parts, &parts_table);
                    }
                    let automatic = rec.num(cid::TRANSMISSION).is_none_or(|v| v == 1);
                    if let Some(setup) = rec.setup() {
                        self.specs.apply_setup(&setup, automatic);
                    }
                    tracing::info!(
                        target: "netplay", grip_before = before.0, grip = self.specs.grip, transfer_div_before = before.1, transfer_div = self.specs.transfer_div,
                        roll_gain_before = before.2, roll_gain = self.specs.roll_gain,
                        "network race: the player's own car folded from its own record, as its copies on the other machines are (0x49CB50)"
                    );
                } else if garage.is_none() {
                    // **A single-player layout race's catalogue car** (QuickRace, Knockout, a series'
                    // round): 0x4EB710's default arm builds a fresh `FECar` from the record (0x4EE3B0),
                    // so it carries the stock parts (0x4EE9A0) and the neutral sliders (0x4EED2D),
                    // and 0x49CB50 folds both — `network.md` §11.12b.
                    let table = self.fe_parts_table();
                    let before = (self.specs.grip, self.specs.transfer_div, self.specs.roll_gain);
                    let automatic = self.player_automatic();
                    nfs_game::career::fold_fresh_car(&mut self.specs, &car.stock_parts, &table, &car.sim_family, automatic);
                    tracing::info!(
                        grip_before = before.0, grip = self.specs.grip, transfer_div_before = before.1, transfer_div = self.specs.transfer_div,
                        roll_gain_before = before.2, roll_gain = self.specs.roll_gain,
                        "layout race: the catalogue car's stock parts and neutral set-up folded (0x4EE9A0, 0x4EED2D, 0x49CB50)"
                    );
                }
                self.snd.sound_set = car.sound_set.clone();
                self.sim_name = car.sim.clone();
                self.dials = Some((car.dial_kmh, car.dial_mph, car.dial_tach));
                // the join screen's destructor 0x50A260 runs as the race takes over
                if self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("joingame")) {
                    self.net_join_screen_gone();
                }
                self.fe.layout_preview = None;
                self.fe.layout_name = None;
                // kept for the way back, as the other launches keep it: without it neither the
                // pause menu's quit (`leave_race`) nor the last results page had a front end to
                // return to, and the quit closed the program
                self.fe.menu_template = self.fe.menu.take();
                // the race's `FEGAME_TYPE [0x6573E8]` is `Game.GameType`, which the `SETTYPE*` button
                // wrote on the way in (0 quick race, 1 quick knockout, 2 knockout, 3 multiplayer …);
                // a direct `--fe=peertopeerip` / `--fe=udpip` entry never passed that button (test glue),
                // so a race with an established link says 3 itself. *(The 2026-09-30 change wrote
                // 3 for every layout race — a quick race's P then took the network rule 0x415110 and left.)*
                let net_link = self.net.session.as_ref().is_some_and(|n| n.established());
                self.fe.race_game_type = Some(if net_link { 3 } else { self.fe.game_type });
                let (renderer, window) =
                    (self.renderer.take().unwrap(), self.window.take().unwrap());
                self.tick = 0;
                self.start_race(renderer, window, event_loop);
            }
            (t, c, s) => {
                tracing::error!(track = ?t.err(), car = ?c.err(), sim = ?s.err(), "load failed")
            }
        }
    }

    pub(crate) fn start_race(
        &mut self,
        mut renderer: Renderer,
        window: Arc<Window>,
        event_loop: &ActiveEventLoop,
    ) {
        self.enter_race_mode();
        // `Replay_startRace` 0x413EA0 + `0x414EB0`: rewind the stream and seed the shared RNG from
        // the race's seed, which is what makes a replay re-simulate the same race (replay.md §11).
        let playing = self
            .replay.stream
            .as_ref()
            .is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing);
        // the player's typed name is the car's `carinfo+0x17C`, which the specs' tail compares
        // with the two cheat names (0x49D6E7…0x49D875), in the gear loop beside the manual ×1.05
        let name = if playing && !self.replay.setup.driver_name.is_empty() {
            // `Replay_begin` 0x413CF0's bulk `memcpy(0x6573E8, 0x5E9A20, …)` restores the whole
            // recorded car-slot block on playback, `.DRIVERNAME` (+0x17C, `replay.md` §16.2)
            // among it, before the race is built — so a playback of an original `.rpl` (or the
            // port's own `.prpl`, once it carries the field) shows the recording's driver, not
            // the profile up now. *(Until 2026-09-28 the port's reader dropped the field, so
            // playback kept whatever `self.player_name` already was — the live profile's, or the
            // start-up one for a CLI `--replay` with no career.)*
            Some(self.replay.setup.driver_name.clone())
        } else {
            self.career.as_ref().map(|c| c.profile.name.clone())
        };
        if let Some(name) = name {
            let automatic = self.player_automatic();
            self.specs.apply_driver_name(&name, automatic);
            // …and the same field is `.DRIVERNAME` 0x13B, which every name the race shows reads:
            // the standings / summary / position rows (`[car+0x558]+0x17C`, 0x431D6E…0x431D90,
            // `hud.md` §5) and the plate (`.LICENSE`, the same argument). The launch's per-car
            // set-up 0x4EEDA0 writes it (0x4EEDF1) from its fourth argument, the profile name
            // 0x501270 `[0x691DC8]` for the player (0x4EBA2E) — the profile up **now**, not the one
            // `player.lst` named when the program started (or, on a playback, the snapshot's,
            // above). *(Until 2026-09-27 `player_name` was read once at start-up, so a career
            // created at `CREATEPLAYERDONE` (or a profile loaded) raced as the start-up profile
            // — "DRIVER" on a fresh install.)*
            self.player_name = name;
        }
        if let Some(r) = self.replay.stream.as_mut() {
            r.start_race();
        }
        if playing {
            tracing::info!(driver = %self.player_name, from_snapshot = !self.replay.setup.driver_name.is_empty(), "playback: .DRIVERNAME resolved");
            // `Replay_startRace` 0x413EA0 opens playback on the **first** entry of the cycle
            // table, `DirectorsCam` (0x14) — "whichever track-side camera the viewed car is
            // currently passing", the classic NFS director's cut. Playback does not cycle the
            // car's own camera list at all: it has its own nine ids (`replay.md` §8).
            self.replay.camera = 0;
            // …and at normal speed, `[0x606864] = 2` (0x413ECF), whatever the last playback was
            // left at. *(Until the audit of 2026-09-26 the speed carried over.)*
            self.replay.speed = nfs_game::replay::Speed::Normal;
        }
        // not paused: `[0x606860] = 0` (0x413ECB) and the bar's `+0x199` too (0x42E030, 0x426A89)
        self.replay.frozen = false;
        self.replay.race_over = false;
        // the camera slots (0x4640B0 at 0x4B6DB9)
        self.camera_slot_reset();
        let race_pass = std::mem::take(&mut self.race_pass);
        // a new session's timer starts `[0x5E99B8]`, and so `[0x606AC8]`, at 0 (0x4A4752)
        if !race_pass {
            // the session's HUD (0x4B6878 → 0x424460 → `HUD_init` 0x41E170): live, the options'
            // MAP / REARVIEWMIRRROR, TRANSLUCENCY's box (0x425298), the map's suspend cleared;
            // a pass keeps them (`HudKeys`)
            self.hud_keys = HudKeys::new(
                self.choice_index("Display.Map").map_or(1, |(i, _)| i),
                self.choice_index("Display.RearView").is_some_and(|(i, _)| i != 0),
                self.choice_index("Display.Translucency").is_none_or(|(i, _)| i == 0),
            );
            self.map_slide = (0, 0);
            self.mirror_slide = (0, 0);
            // `HReplay` 0x42C910, built by the same factory (0x424F90 / 0x424FA6)
            self.replay_bar = nfs_game::replay::BarState::new();
            self.hud_clock_base = -(self.pause_offset + self.pause_ticks);
            // `car+0xB54`, the metres: a session's race init allocates every car afresh and
            // zero-fills it (`Cars_create` 0x411130: `rep stosd` over 0x495 dwords, 0x4111B0…
            // 0x4111B9), and nothing but 0x4170A2 writes it — so they are 0 until the first
            // `tick % 64 == 1` of a new session, and a pass (restart, replay) keeps the last ones
            // (neither `Car_reset` 0x410730 nor the HUD's init 0x424460 touches them). *(Until
            // 2026-09-26 the port showed the last session's until that tick.)*
            self.hud_metres.clear();
        }
        // `Cars_resetAll` 0x4110A0 (0x414F12, every pass of the session loop): both race-order
        // arrays `0x5E8B88[]` / `0x5E8BAC[]` back to the registration order `0x5E8B34[]` over
        // `NUMCARS` (0x4110AC…0x4110C4) before any car is reset. *(Until 2026-09-26 the port
        // started afresh only on a different car count, so a race with as many cars as the last
        // began from the order the last one ended in — ties and all.)*
        self.race_order = Default::default();
        if playing {
        }
        if !playing {
            // **`RAND_SEED` `[0x657A34]`** is what 0x414EB0 hands `Rand_seed` 0x4A4780 (0x414EE9) at
            // every race start, restart and replay. Nothing in the program writes it but the
            // registry: no store names the global (only the reads 0x414EE9 / 0x414EF4) and no
            // front-end `0x4EDB20` call passes its id, so it is `fe.txt`'s `rand_seed` (8165
            // shipped; `fe_out.txt` keeps 8165), which the session loop applies afresh before
            // every race (0x4B6660 reads `fe.txt` at 0x4B6C1C, 0x4B4B80 applies each line at
            // 0x4B6C83) — or a loaded replay's block (0x413CF0). Every live race of every session
            // draws the same stream. *(Until 2026-09-26 the port seeded each race from the clock.)*
            // `NFS_TRAFFIC_SEED` pins another value (diagnostic).
            self.traffic_seed = nfs_game::config::cfg().nfs_traffic_seed.var()
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| self.net_fe_txt.get("rand_seed").copied().unwrap_or_else(|| self.registry.u32("rand_seed", 0)));
            self.replay.setup = nfs_game::replay::ReplaySetup {
                track: self.track.name.clone(),
                car: self.car.model.clone(),
                sim: self.sim_name.clone(),
                // the catalogue record the race's car is (`start_from_layout`'s `player_car_id`), which a
                // replay resolves the model's body style and roof from; the start-up default otherwise
                car_id: self.player_car_id.unwrap_or(self.car_id),
                paint: self.paint,
                laps: self.num_laps,
                opponents: self.num_opponents,
                traffic: TRAFFIC_COUNT
                    .iter()
                    .position(|c| *c == self.road.traffic_count)
                    .unwrap_or(0) as u32,
                cops: self.road.cops_enabled,
                damage: self.road.damage_on,
                backwards: self.backwards,
                mirror: self.mirror,
                seed: self.traffic_seed,
                skill: self.race_skill,
                // the name this race is recorded under (§16.2 / §18.11): a later playback of this
                // same setup (the automatic replay-in-place, or a saved `.prpl`) shows it back
                driver_name: self.player_name.clone(),
                net: self.replay_net_snapshot(),
                // the launch block's words a playback gets back (`Replay_begin` 0x413CF0 restores the whole
                // block): `RACE_TYPE` 0x10, `FEGAME_TYPE` 0x30 (0x4EB470) and the car's `.CAMERA` list (0x4EF02A...)
                race_type: Some(self.launch_race_type()),
                fegame_type: Some(self.race_fegame_type()),
                camera: Some(self.camera_cycle.iter().take(4).map(|&c| u32::from(c)).collect()),
                // the car-info records (`Replay_begin` restores slot 0 ... 8): the player's here (its start damage is
                // filled in where the car is built), the opponents' where the plan is made (`race_stage_grid`)
                player: self.launch_player_record(),
                opponent_cars: None,
                // `SCENENUMBER` 0x1C (`fac+0x24`, 0 outside a skill test), `DERBY` 0x1B, `NOCOLLISION` 0x29
                scene: Some(self.mission.as_ref().map_or(0, |m| m.mission.scene)),
                derby: Some(self.cheat_bits & (1 << 7) != 0 || self.net_derby),
                nocollision: Some(self.net_nocollision),
                // `.TRANSMISSION` 0x221 (`carinfo+0xC`), the word the car is built with: a playback restores it
                transmission: Some(self.player_automatic()),
                opponent_transmission: Some(self.option_automatic()),
                // the mission's launch words (0x4EC040), the block's copy a playback runs the HUD from
                mission: self.mission.as_ref().map(|m| nfs_game::replay::mission_words(&m.mission)),
            };
        }
        self.replay.setup.seed = self.traffic_seed;
        // **TEST GLUE** (`RUST_LOG=racecfg=info`): what this race was set up with, so a race of a
        // scripted session can be compared with the same race in a fresh process
        tracing::info!(
            target: "racecfg",
            game_type = ?self.fe.race_game_type,
            fe_game_type = self.fe.game_type,
            track = self.track.name,
            car = self.car.model,
            car_id = ?self.player_car_id,
            sound_set = self.snd.sound_set,
            dials = ?self.dials,
            laps = self.num_laps,
            opponents = self.num_opponents,
            career_opponents = ?self.career_opponents.as_ref().map(|o| o.len()),
            career_opponent_cars = ?self.career_opponents.as_ref().map(|o| o.iter().map(|o| (o.name.as_str(), o.car.display_name.as_str(), o.grid)).collect::<Vec<_>>()),
            net_seat_grids = ?self.net_seat_grids,
            net_traffic = self.net_traffic.len(),
            grid = ?self.player_grid_override,
            backwards = self.backwards,
            mirror = self.mirror,
            traffic = self.road.traffic_count,
            traffic_ids = ?self.road.traffic_ids.as_ref().map(|t| t.len()),
            cops = self.road.cops_enabled,
            cop_count = self.road.cop_count,
            cops_race = self.road.cops_race,
            damage = self.road.damage_on,
            skill = self.race_skill,
            quick_knockout = self.quick_knockout,
            mission = ?self.mission.as_ref().map(|m| m.index),
            skill_test = self.skill.is_some(),
            camera_cycle = ?self.camera_cycle,
            block_game_type = ?self.block_game_type(),
            setup_race_type = ?self.replay.setup.race_type,
            paint = ?self.paint,
            seed = self.traffic_seed,
            "race settings"
        );
        // **`srand(time(0))`** (0x418100, `HMissionSummary`'s constructor — a race load): the C
        // runtime's state for everything up to the next race load
        self.crt_rand = nfs_game::movie_overlay::CRand::new(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_secs() as u32));
        if self.net.session.is_some() {
            let time = self.net_fe_txt.get("time").copied().unwrap_or_else(|| self.registry.u32("time", 0));
            tracing::info!(target: "netplay", rand_seed = self.traffic_seed, time, weather = 0, car_car_pass_skipped = self.net_nocollision, "network race: the option values in force (0x414EE9 RAND_SEED, TIME; WEATHER is stored 0 by 0x414DF9; 0x498FF0's car-car half skipped when COMMSPOSITIONAL && NOCOLLISION)");
        }
        self.replay.saved = false;
        self.replay.loop_count = 0;
        nfs_game::rng::seed(self.traffic_seed);
        // 0x4A4A70(0) as the front end hands over (main 0x4B6CCA)
        self.race_loading_frame(&mut renderer, 0);
        // 0x427A60, the race load 0x4B67B0's first call (before its steps 1 and 2): `pic16.fsh` from
        // the HUD folder posted as file type 0, for the others' `HLeader` and their loading screen's
        // rows (the port posted it with the seats, after step 5)
        if let Some(n) = self.net.session.as_mut()
            && let Some(b) = GamePaths::find_file(&self.paths.game_data().join("DashHud"), "pic16.fsh").and_then(|p| std::fs::read(p).ok())
        {
            n.post_picture(&b);
        }
        // 0x4B67D8 / 0x4B67DF: steps 1 and 2 straight after 0x427A60, with nothing between them
        self.race_loading_frame(&mut renderer, 1);
        self.race_loading_frame(&mut renderer, 2);
        // the start scene the race loads (0x46D600) decides whose GEOM elements stand
        self.track.reverse = self.backwards;
        // the texture pixels are read again for a restart / replay pass (released after the last upload)
        self.track.ensure_textures().expect("track textures");
        let mut world = TrackWorld::upload(&mut renderer, &self.track).expect("track");
        // …and are on the GPU now: the CPU copies (a track's whole mip chains) go
        self.track.release_textures();
        // 0x469065: step 3 inside the track load 0x468FB0, right after `RdrKeys` (0x44FAE0) — the track's
        // data and keys are in; the sky and the weather (0x45DBD0, 0x458B80) still follow there
        self.race_loading_frame(&mut renderer, 3);
        // 0x4A17F0 / `Cars_resetAll` at every race's start: the smackables' car records afresh
        self.smackables = Default::default();
        // Each `RMth` pass carries an option mask (`+8` of its sixteen-byte record from `+0x1C`),
        // and 0x44BF70 builds a pass only when that mask is 0 or meets the render flags
        // `[0x6197F8]` (0x44C185). `Graphics.Headlights` = Projected (1) is what sets bit 8 of
        // those flags (0x44EDB7); the night `Opaque` method's second pass, the `HEAD` decal's
        // `LayerUVLight` ops, has the mask 8. So the decal is drawn only for Projected, and not at
        // the shipped Vertex (0) (`RE/notes/lighting.md` §5.3).
        if self.choice_index("Graphics.Headlights").is_none_or(|(i, _)| i != 1) {
            world.clear_uv_lit();
        }
        // `Graphics.CarChrome` below Medium leaves bit 2 out of `[0x6197F8]` (0x44EF7E), and the
        // `CarExtEnv` / `CarExtEnvNS` passes that carry the reflection layers have the mask 2
        // (`lighting.md` §5.3): at the shipped Low (0) the paint has no reflection pass
        // and the specular layer draws with `SPC1…SPC3` of `carcmn.fsh`, whatever the option (0x441AB2)
        let car_env = nfs_game::world::CarEnv {
            specular: nfs_game::world::car_specular_maps(&mut renderer, &self.paths, false),
            chrome_passes: self.choice_index("Graphics.CarChrome").is_some_and(|(i, _)| i >= 1),
            rs_bias: true,
        };
        // the car shadow projectors' texture, `[0x6115CC]+0x18C` (`crp-geometry.md` §8)
        renderer.set_projector_texture(world.projector_texture);
        self.car.tiny = self.cheat_bits & (1 << 1) != 0;
        // the driven car's catalogue record gives the stripe and interior colours that go with
        // the paint (0x443DB0, `car-model.md` §5.0a)
        let player_paint = nfs_formats::fedata::parse_car_catalog(&std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car")).unwrap_or_default())
            .unwrap_or_default()
            .iter()
            .find(|c| c.sim.eq_ignore_ascii_case(&self.sim_name) && c.model.eq_ignore_ascii_case(&self.car.model))
            .map(|c| Paint::matching(c, self.paint))
            .unwrap_or(self.paint.into());
        let player_paint = self.owned_paint.filter(|p| p.exterior == self.paint).unwrap_or(player_paint);
        // A launch with no front end (`fe = 0`) never runs 0x4EEDA0, so the car's four colours are
        // the registry's own `car[0].colorconfig` (`.COLORCONFIG` 0x149 → `carinfo+0x218`, the
        // registry triple 0x4B2FD7…0x4B3013): exterior, stripe 1, stripe 2, **interior** (`+0x224`),
        // each `0xAARRGGBB` as a signed decimal, and `.STRIPEPACKAGE` (absent: 0). The shipped
        // fe.txt's `-65536 ×3, -3757195` is a red paint with the interior 0xC6AB75 — the grey-beige
        // cabin of the original's own frames (`REFERENCE-2026-10-02-SRVF.md`), not the catalogue's
        // first set's interior the port took (brown).
        let player_paint = match self.registry.get("car[0].colorconfig").map(|v| v.split(',').filter_map(|x| x.trim().parse::<i64>().ok()).collect::<Vec<_>>()) {
            Some(cc) if self.fe0 && cc.len() >= 4 => {
                let rgb = |c: i64| [(c >> 16) as u8, (c >> 8) as u8, c as u8];
                Paint { exterior: rgb(cc[0]), stripe1: rgb(cc[1]), stripe2: rgb(cc[2]), interior: rgb(cc[3]), package: self.registry.u32("car[0].stripepackage", 0) }
            }
            _ => player_paint,
        };
        let mut text = self.page_text(0, &self.player_name, None);
        // a garage car brings its own race number (the front end's 0x4D1800 from `DecalNum`)
        if let (Some(n), true) = (self.owned_decal, self.owned_paint.is_some_and(|p| p.exterior == self.paint)) {
            text.race_number = n as i32;
        }
        self.car.page_text = Some(text);
        // `.DRIVER` 0x23D: 0x4EEDA0 draws `rand() & 7` for every car it launches (0x4EEDAC…
        // 0x4EEDE2), in 0x4ECF00's order — the player (0x4EBA4D), the opponents (0x4EBCBF /
        // 0x4EBF06 / 0x4EC3FF / 0x4DE213), the cops one per car (0x4ED174 / 0x4ED25E), then the
        // traffic (0x4ED460 / 0x4ED56E, unused: slot 9 = 0 at 0x43AED4). The player's draw is
        // made and thrown away: kind 1 (0x4EEDBD) sends `[0x5DB748]`, the profile's picture
        // (0x5024E0; 8 until a profile is loaded or created, `savegame.md` §2), instead.
        // The original draws from the program's C `rand`, seeded from the clock (0x418100) and
        // shared with everything else, so no two launches agree (unobservable):
        // this is a generator of the race's own on its seed. The C `rand` draws 0x4F4FA0 makes
        // between the player's and the opponents' (package / decal, 0x4F5153…) come from
        // `quick_opponents`' own generator, so this sequence is not advanced by them — only the
        // count of draws between them differs, on a clock-seeded stream (`car-model.md`, `.DRIVER`)
        let mut driver_rand = nfs_game::movie_overlay::CRand::new(self.traffic_seed);
        let factory_driver = self.mission.is_some();
        let _player_draw = driver_rand.next();
        // A launch with no front end (`fe = 0`) never reaches 0x4EEDA0 either: `carinfo+0x150` keeps
        // what the registry's `car[0].driver` says, and the shipped fe.txt has no such key, so the
        // `.data` zero of 0x6581DC stays — `.DRIVER` 0, the arm and hand variants 2 / 1 of slot 19's
        // `.DRIVER < 8` arm (0x43AF51), which the original's own cockpit frames show (hands on the wheel).
        let picture = if self.fe0 {
            self.registry.u32("car[0].driver", 0) as u8
        } else {
            self.career.as_ref().map_or(nfs_game::save::DEFAULT_PICTURE, |c| c.profile.picture)
        };
        self.car.driver = nfs_assets::car::DriverSeat { driver: picture as u32, no_driver: false, factory_driver };
        let car = CarModel::upload_with_env(
            &mut renderer,
            &self.car,
            &self.paths,
            1,
            player_paint,
            car_env,
            self.track.is_night(),
            // `.CABRIODOWN` 0x24B: the player's car (kind 1) keeps `Car.Roof`, which 0x4F1E40
            // forces to 0 on a car without the convertible flag; every other car is sent 0
            // (0x4EEF0C…0x4EEF1A)
            Some(if self.car.cabrio { self.choice_index("Car.Roof").map_or(0, |(i, _)| i as u32) } else { 0 }),
            self.skidpad_rule(),
        )
        .expect("car");
        let sky_level = nfs_game::sky::forced_level().unwrap_or_else(|| self.choice_index("Graphics.SkyRendering").map_or(0, |(i, _)| i));
        // a new session's race load reseeds 0x538040 at the HUD's init (`HMissionSummary`'s
        // constructor, 0x428C2D; see `fe_rng`), which comes before the loop's pass remakes the sky
        // (0x4B6878 before 0x4B6DB4 → 0x4690A0 → 0x45DD00): the pass's sky and weather draw from
        // it. `NFS_FE_TIME=<seconds>` pins `time(0)` (diagnostic).
        if !race_pass {
            let t = nfs_game::config::cfg().nfs_fe_time.var().ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or_else(|| {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32)
            });
            self.fe_rng = nfs_game::opponents::FeRng::new(t);
        }
        self.sky = Sky::upload(&mut renderer, &self.track, sky_level, &mut self.fe_rng);
        // 0x4B6827 / 0x4B682E: steps 4 and 5 straight after the whole track load 0x468FB0 (the sky and
        // the weather were its last parts), the track picture drawn at 5
        self.race_loading_frame(&mut renderer, 4);
        self.race_loading_frame(&mut renderer, 5);
        // a night track switches the dials to their `*N` faces and needles (`hud.md` §3.2)
        let night = self.track.is_night();
        if night {
            tracing::info!(track = self.track.name, "night track");
        }
        let mph = self.choice_index("Display.SpeedUnits").is_some_and(|(i, _)| i != 0);
        self.hud = Hud::load(&mut renderer, &self.paths, self.dials, night, mph);
        self.wrong_way_blink = (0, 0, true);
        if let (Some(h), Some(t)) = (
            self.hud.as_mut(),
            SliceTable::from_crp(&self.track.crp, &self.track.crp_data).ok(),
        ) {
            h.set_map(
                &mut renderer,
                &if self.backwards { t.reversed() } else { t },
            );
        }
        // **The camera manager 0x471B70** (`WTrack::load`): moved ahead of the audio load so that its step 6
        // (0x4B683F) comes before step 7 (0x4B6855) as in 0x4B67B0 — the data depends on the track alone.
        // the trackside cameras and their triggers travel with the track (`race-logic.md` §3.3)
        self.track_cameras = self
            .track
            .cameras
            .as_ref()
            .map(nfs_game::cameras::TrackCameras::from_scene);
        // the `CmAn` camera paths (0x471850 from `WTrack::load`, 0x48779C)
        self.cam_anims = nfs_game::camera::anim_cam::load(&self.track.crp, &self.track.crp_data);
        self.race_loading_frame(&mut renderer, 6);
        self.race_stage_audio(playing, &mut world);
        // 0x4B6855: step 7 after `SND_coll` 0x4A8CD0 (the collision sounds, `CollisionSfx` above)
        self.race_loading_frame(&mut renderer, 7);
        // 0x414E63: step 8 in the race init 0x414DB0, before `Cars_create` 0x411130 (0x414E68) makes the cars
        self.race_loading_frame(&mut renderer, 8);
        let (table, rot, grid_place) = self.race_stage_grid(&mut renderer, &mut world, car_env, &mut driver_rand, factory_driver);
        if let Some(h) =
            world
                .collision
                .ground(self.car_pos.x, self.car_pos.y + 5.0, self.car_pos.z, 50.0)
        {
            self.car_pos.y = h.point.y + car.wheel_radius;
        }
        self.car_rot = Quat::from_mat3(&rot.transpose()).normalize();
        let mut rot = rot;
        // the skill test's start (0x410430), placed on the vehicle below
        // (the same pick as the grid stage's, which holds it across its own `self` writes)
        let start_scene = if self.backwards { self.track.start_backward.as_ref().or(self.track.start_forward.as_ref()) } else { self.track.start_forward.as_ref() };
        let mut skill_start: Option<(Vec3, Vec3)> = None;
        if let Some(sk) = &self.skill {
            // skill test: start pose from the type-2 trigger (0x410430), first arrow shown
            if let Some((pos, fwd)) = sk.start_pose() {
                skill_start = Some((pos, fwd));
                // a provisional frame for `Vehicle::new`; `place_at_trigger` sets the real one
                let right = Vec3::Y.cross(fwd).normalize_or_zero();
                let up = fwd.cross(right).normalize_or_zero();
                rot = glam::Mat3::from_cols(right, up, fwd.normalize_or_zero()).transpose();
                self.car_pos = pos;
                self.car_rot = Quat::from_mat3(&rot.transpose()).normalize();
                if let Some(r) = self.race.as_mut()
                    && let Some(t) = &table
                {
                    let linear = r.linear.is_some();
                    *r = RaceProgress::new(t.find_slice(pos, 0), t);
                    if linear {
                        r.set_linear(t, nfs_game::race::finish_slice(t, start_scene));
                    }
                }
            }
            let shown: Vec<usize> = sk.initial_arrow().into_iter().collect();
            world.set_shown(&sk.arrows, &shown);
            // laps never end a skill test: the finish trigger does
            self.num_laps = 99;
            tracing::info!(
                checkpoints = sk.checkpoints.len(),
                cones = sk.cones.len(),
                arrows = sk.arrows.len(),
                "skill test loaded"
            );
        }
        self.prev_car_pos = self.car_pos;
        self.slices = table;
        self.car_half_width = (car.bbox_max.x - car.bbox_min.x).abs() * 0.5;
        let mut vehicle = Vehicle::new(
            self.car_pos,
            rot,
            &self.specs,
            car.half_track,
            car.wheel_radius,
        );
        // `.TRANSMISSION` 0x221 → `carinfo+0xC` (0x4EEF9F): `Car.Transmission`'s index, 1 being
        // 561 "automatic" — the value 0x49DDD5 and 0x49DEDC compare with 1
        vehicle.automatic = self.player_automatic();
        // `.TRACTION` for the player's car: set only in Factory Driver mission 0xE
        // (0x4EB9D2…0x4EB9D7 → 0x4EEFDB…0x4EEFF1); `Car.Traction` itself is overwritten by the
        // per-car routine's later entry (`physics.md` §5.4)
        vehicle.tcs = self.mission.as_ref().is_some_and(|m| m.index == 0xE);
        // **The damage the car brings with it.** `Car_raceInit` 0x41083A copies ten dwords from
        // `carInfo+0xC8 … +0xEC` into `car+0x4E4`, the per-zone body damage, and 0x41085F copies
        // twenty more from `carInfo+0x78 … +0xC4` into each installed part's `+0x164`. The port had
        // the second half — `installed_parts` puts those states into `CarSpecs::parts` — and started
        // every race with an undented body whatever the garage said.
        // a playback starts from the recording's own `.DAMAGEZONES` (`Replay_begin` 0x413CF0 restores slot 0's record)
        let recorded_damage = if self.playback_running() { self.replay.setup.player.as_ref().map(|c| c.damage) } else { None };
        if let Some(z) = recorded_damage {
            vehicle.damage = z;
        } else if let Some(owned) = self
            .career
            .as_ref()
            .and_then(|cu| cu.pending.as_ref().and_then(|p| p.garage_idx).and_then(|i| cu.career.garage.get(i)))
            .or(self.fe.test_drive.as_ref().and_then(|(_, o)| o.as_ref()))
        {
            vehicle.damage = owned.damage;
            tracing::info!(damage = ?owned.damage, "body damage carried into the race");
        }
        // …and the launch record keeps what the car came in with (`.DAMAGEZONES` 0x14D)
        if !self.playback_running()
            && let Some(c) = self.replay.setup.player.as_mut()
        {
            c.damage = vehicle.damage;
        }
        // …and then **0x493CF0(car, 1)** (0x41088B): each part of type 1…10 takes back the fold
        // of the zones it came in with — the saved `+0x164` already holds it — so the getters
        // during the race and the race-over fold count those zones once. Every pass runs it
        // (`Car_raceInit` from `Cars_resetAll` 0x4110D7) from the unchanged `carInfo` record, so a
        // restart and the automatic replay start from the same states (`InstalledPart::carried`).
        for p in self.specs.parts.iter_mut() {
            p.unfold(vehicle.part_damage(&p.mask));
        }
        if self.specs.parts.iter().any(|p| p.state != p.carried) {
            tracing::info!(states = ?self.specs.parts.iter().map(|p| (p.kind, p.carried, p.state)).collect::<Vec<_>>(), "0x493CF0(car, 1): the carried zones unfolded from the parts");
        }
        // the body the car actually collides with (`BndP`, `RE/notes/car-model.md` §12)
        let (pts, half) = nfs_game::world::car_body(&self.car, self.cheat_bits & (1 << 1) != 0);
        vehicle.set_car_body(pts, half);
        // `Car_raceInit` 0x410730 → `Car_placeAtSlice` 0x49B090 (0x41095C) with `Grid_slot`'s
        // slice and offset; the human is not an opponent (`car+0x52C & 8` clear), dir +1 (the
        // reversed table turns the slices). A skill test takes 0x410430's trigger pose instead
        // (0x4108B2, which jumps past 0x49B090): the trigger's position and `+0xC8`, the frame
        // on the patch normal and one `Susp_update` drop (`Vehicle::place_at_trigger`).
        if let Some((pos, fwd)) = skill_start {
            vehicle.game_tick = self.tick;
            vehicle.place_at_trigger(&world.collision, &self.specs, pos, fwd);
            self.car_pos = vehicle.draw_origin();
            self.car_rot = Quat::from_mat3(&vehicle.orientation.transpose()).normalize();
            self.prev_car_pos = self.car_pos;
        } else if self.skill.is_none()
            && let (Some((slice, pos)), Some(t)) = (grid_place, &self.slices)
        {
            vehicle.game_tick = self.tick;
            let s = &t.slices[slice];
            vehicle.place_at_slice(&world.collision, &self.specs, s.centre, s.forward, grid_offset(s, pos), false, false);
            self.car_pos = vehicle.draw_origin();
            self.prev_car_pos = self.car_pos;
        }
        self.vehicle = Some(vehicle);
        self.ff_arm();
        // `Car_init` 0x410C74: the lamp byte `car+0xD87` starts at 3 (the headlights) on a night
        // track, else 0; the headlights' ramp starts clear (`HeadRamp`)
        self.lamp_bits = if self.track.is_night() { 3 } else { 0 };
        self.head_ramp = HeadRamp::default();
        self.hud_hidden = false;
        tracing::info!(pos = ?self.car_pos, "car placed");
        // 0x414E74: step 9 right after `Cars_create`; the HUD factory 0x424460 and the rest of the race
        // set-up (0x4690D0, 0x434A90, 0x415DC0, 0x4121B0) follow it in 0x4B67B0, as here
        self.race_loading_frame(&mut renderer, 9);
        // 0x426A80(0): every racer's damage total as it lines up, garage damage included
        self.start_damage = [0; 8];
        if let Some(v) = &self.vehicle {
            self.start_damage[0] = nfs_game::career::damage_sum(&v.damage);
        }
        for (i, o) in self.road.opponents.iter().take(7).enumerate() {
            self.start_damage[i + 1] = nfs_game::career::damage_sum(&o.vehicle.damage);
        }
        self.race_stage_pools(&mut renderer);
        // **A pass keeps the weather layer.** The session's track load makes it (0x469092 →
        // 0x458B80 → 0x458C90) and the session's end deletes it (0x4690F0 → 0x458BB0); every pass
        // in between — the race, a restart, the automatic replay — only runs 0x4690A0 from 0x414EB0,
        // which primes the same object again (0x4690AF → 0x459260). This port reloads the track
        // for a pass, so the layer is carried over and primed on the first update below; what the
        // prime does not touch runs on from where the last pass left it — the emitters' gust
        // `+0x85C4` / `+0x85C8` and last view `+0x85CC…+0x85FC`, the drops' plane (the car's `Wndo`
        // once attached), their colours, last eye and flags. *(Until 2026-09-26 every pass made
        // the layer afresh.)*
        self.weather_kept = if race_pass { self.weather.take() } else { None };
        self.weather = None;
        // the skid marks' cells (0x479900) are cut from the world's fitted tree (`WTrack+0x10`,
        // `nfs_game::fitted_tree`): its root is the union of the eight box corners every placed item
        // answers (`getBounds`, 0x47B6C6…0x47B89C — the corners moved through the item's matrix
        // where its bundle has one, so a rotated article widens the box, 0x4893A0's `this[1]` branch
        // read 2026-09-30), and it is split by the vertices of the items' first-LOD `vt` parts
        // (0x47B762…0x47B88B → 0x47BE30) — `nfs_game::world::tree_population`. The same tree
        // feeds the track walk's node pass ([`nfs_game::world::TrackWorld::article_draws`]).
        self.skid = nfs_game::skidmarks::SkidMarks::for_tree(world.fitted.clone());
        // a replay about to play opens on the track's flagged camera: `Replay_startRace` 0x413EA0
        // (`[0x5E99F4] >= 2`, camera elements present) sets every car's `+0x10E4` to 0x46D890()
        // and switches to it (0x413F08…0x413F37) — except in a factory-driver replay
        // (`[0x6573E8] == 9`, 0x413EF4), which takes view 0x0A instead (not carried here)
        let playing = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        if playing
            && self.skill.is_none()
            && let Some(t) = self.track_cameras.as_mut()
            && !t.cameras.is_empty()
        {
            t.open_replay(self.backwards);
        }
        if let Some(t) = &self.track_cameras {
            tracing::info!(
                cameras = t.cameras.len(),
                triggers = t.triggers.len(),
                "trackside cameras"
            );
        }
        // the headlight projector's bitmap, loaded on a night track as `Headlights_init` 0x452590
        // does (`RE/notes/lighting.md` §5)
        if self.track.is_night()
            && let Some(m) = nfs_assets::track::night_map(&self.paths)
        {
            renderer.set_headlight_map(&m);
        }
        self.world = Some(world);
        // diagnostic: `NFS_PROJECTOR_VIEW=k` sets the car down on the track's k-th shadow projector
        if let Some(k) = nfs_game::config::cfg().nfs_projector_view.var().ok().and_then(|v| v.parse::<usize>().ok())
            && let Some(q) = self.world.as_ref().and_then(|w| w.projectors.get(k)).cloned()
        {
            let ground = self.world.as_ref().and_then(|w| w.collision.ground(q.point.x, q.point.y + 50.0, q.point.z, 200.0)).map(|h| h.point.y).unwrap_or(q.point.y);
            if let Some(v) = self.vehicle.as_mut() {
                // diagnostic pose, no original counterpart: the origin half the model box up
                v.position = Vec3::new(q.point.x, ground + v.body_lift, q.point.z);
                self.car_pos = v.draw_origin();
            }
        }
        let mut car = car;
        // `.TINYCAR`: the model at 0.4 (0x4433D2)
        if self.cheat_bits & (1 << 1) != 0 {
            car.scale = 0.4;
        }
        // the dash needles' calibration, off the player's `nfs5.car` record (the same pick as the
        // grid's: the chosen car id, else the model's first record with a sim)
        let catalog = nfs_formats::fedata::parse_car_catalog(
            &std::fs::read(self.paths.fe_data().join("Data").join("nfs5.car")).unwrap_or_default(),
        )
        .unwrap_or_default();
        if let Some(e) = self
            .player_car_id
            .and_then(|id| catalog.iter().find(|c| c.id == id))
            .or_else(|| catalog.iter().find(|c| c.model.eq_ignore_ascii_case(&self.car.model) && !c.sim.is_empty()))
        {
            car.set_dash(e.dash[0], e.dash[1], e.dash[2], e.dash[3]);
        }
        self.car_model = Some(car);
        // the cockpit view's eye point, for the car's own body style
        // `drivercamposstyle<N>` is keyed on the **raw 0-based** style number — `550.tpg` has a
        // `drivercamposstyle0` — where `[styleN]` is 1-based. This passed a flat 1.
        self.driver_cam = nfs_assets::car::DriverCam::from_tpg(&self.car.tpg, self.car.style_index)
            .map(|dc| if self.mirror { dc.mirrored() } else { dc });
        // `Game.Damage` `[0x657A2C]` over every car record of the race — `Damage` 0x494500 reads
        // the global (0x49451B), whichever car it is called for. *(Until 2026-09-26 the port's
        // bodies were built with the flag on and nothing read the option.)*
        let damage_on = self.road.damage_on;
        if let Some(v) = self.vehicle.as_mut() {
            v.damage_enabled = damage_on;
        }
        for o in &mut self.road.opponents {
            o.vehicle.damage_enabled = damage_on;
        }
        if let Some(p) = self.road.police.as_mut() {
            for c in &mut p.cars {
                c.body.damage_enabled = damage_on;
            }
        }
        if let Some(tr) = self.road.traffic.as_mut() {
            for b in &mut tr.bodies {
                b.damage_enabled = damage_on;
            }
        }
        for sm in &mut self.smackables.props {
            sm.body.damage_enabled = damage_on;
        }
        // 0x4A4A70(10), the screen down (0x4B68CB), after the barrier
        // 0x4B68C4: the barrier between them, the loading screen redrawn while it waits
        self.net_load_barrier(&mut renderer);
        self.race_loading_frame(&mut renderer, 10);
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.rebuild_scene();
        // the session loop's barriers and the race frame's `0xC5` round (`net_race_go`)
        self.net_race_go();
        // …and the exchange's own race-frame step (`net_exchange_begin`: the seats are human cars, the state dump)
        self.net_exchange_begin();
        self.last_frame = Instant::now();
        self.fps_timer = Instant::now();
        if let Some(secs) = self.autotest.take() {
            let ticks = (secs * 64.0) as u32;
            // diagnostic: with `NFS_SMACK_DRIVE` the scripted run draws a frame every other tick,
            // so the draw's side of a struck smackable (its player's sample, its record's `+8`,
            // the burst) runs as in a windowed race
            // — and with `NFS_FX_FRAMES`, so the cars' effects (driven by each drawn frame,
            // `nfs_game::car_effects`) run as in a windowed race
            let smack_frames = nfs_game::config::cfg().nfs_smack_drive.var_os().is_some() || nfs_game::config::cfg().nfs_cone_drive.var_os().is_some() || nfs_game::config::cfg().nfs_fx_frames.var_os().is_some();
            let mut pause_hold = 0;
            // **TEST GLUE** (not in the original): `NFS_NET_PACE=1` holds a scripted network race to
            // the game's own 64 Hz, so the two machines' tick counters stay comparable — the link
            // classification (0x490140) measures a packet's age against the game tick, and an unpaced
            // headless run lets one machine's tick run far ahead of the other's
            let exchange_run = self.net.session.as_ref().is_some_and(|n| n.exchanging());
            let pace = (nfs_game::config::cfg().nfs_net_pace.var_os().is_some() && self.net.session.is_some() && !exchange_run).then(Instant::now);
            for t in 0..ticks {
                self.net_race_notice();
                // an input-exchange race runs a tick when the frames and the host's clock allow it
                if exchange_run && !self.net_exchange_wait() {
                    tracing::warn!(tick = self.tick, "the input exchange stopped: the scripted run ends");
                    break;
                }
                if let Some(t0) = pace
                    && let Some(wait) = (t0 + Duration::from_micros(u64::from(t) * 15_625)).checked_duration_since(Instant::now())
                {
                    std::thread::sleep(wait);
                }
                // diagnostic `NFS_HUD_KEYS=tick:pause` in a network race: the press has asked to leave
                if std::mem::take(&mut self.scripted_net_leave) {
                    if !self.leave_race_as(false) {
                        tracing::info!("network race left by a scripted pause press: no front end to return to");
                    } else if self.fe_sequence_in("NFS_NET_LEAVE_SEQ", event_loop) {
                        // **TEST GLUE** `NFS_NET_LEAVE_SEQ=<dir>`: the lobby after the return stays up
                        // and is drawn (`NFS_FE_SEQ_SCRIPT` tokens, e.g. `wait:` to tick the room);
                        // the sequence ends the run
                        return;
                    }
                    event_loop.exit();
                    return;
                }
                // diagnostic `NFS_HUD_KEYS=tick:pause`: the headless run holds the pause for 64 of
                // its iterations (no tick runs), the audio paused and resumed as a frame would
                if self.scripted_pause {
                    self.sync_audio_pause();
                    pause_hold += 1;
                    if pause_hold >= 64 {
                        pause_hold = 0;
                        self.scripted_pause = false;
                        self.paused = false;
                        if std::mem::take(&mut self.scripted_restart) {
                            // `NFS_HUD_KEYS=tick:pauserestart` — the pause menu's restart
                            // (command 3, 0x415721) instead of its continue, for the restart's
                            // audio-pick count (`replay.md` §18.12): this recurses into a fresh
                            // `start_race` (`self.autotest` is already taken, so it runs the
                            // ordinary — not the scripted — set-up), so the outer loop's own
                            // `ticks`/`t` no longer describe a live race. The extra `sync_audio_pause`
                            // right after stands for the windowed loop's next `RedrawRequested` —
                            // the frame where the divergence's stray resume pick used to fire once
                            // `restart_race` had already made its own race-start pick — then the
                            // scripted run ends (scripted runs never stay open, as the ordinary
                            // race-end path does at its own `event_loop.exit()`).
                            self.restart_race(event_loop);
                            self.sync_audio_pause();
                            event_loop.exit();
                            return;
                        }
                        // **TEST GLUE** `NFS_HUD_KEYS=tick:pausequit` — the pause menu's quit (its question
                        // answered yes): the race left for the front end (`leave_race`), then, in a
                        // scripted session (`NFS_AUTOTEST_RACES`), `PLAYGAME` again for the next race
                        if std::mem::take(&mut self.scripted_quit) {
                            let game_type = self.fe.race_game_type.unwrap_or(self.fe.game_type);
                            if self.leave_race() && self.session_races > 1 {
                                self.session_races -= 1;
                                self.session_replayed = false;
                                tracing::info!(left = self.session_races, "session: quit from the pause menu, next race");
                                // the script's own keys are let go, as `session_step` does
                                self.input.release(Action::Throttle);
                                self.input.release(Action::SteerRight);
                                self.rearm_hud_keys();
                                self.autotest = Some(secs);
                                self.session_next_from_fe(game_type, event_loop);
                                return;
                            }
                            event_loop.exit();
                            return;
                        }
                        self.sync_audio_pause();
                    }
                    continue;
                }
                if smack_frames && t % 2 == 1 {
                    // the windowed frame's billboard key first, as `RedrawRequested` takes it
                    self.billboard_key_frame();
                    self.rebuild_scene();
                    self.shot_at_tick();
                }
                let phase = t as f32 / ticks as f32;
                if t % 2 == 0 {
                    self.input.press(Action::Throttle);
                    // **TEST GLUE** (not in the original): `NFS_NO_STEER=1` keeps the scripted run's
                    // last third straight, for a still of the wheel and the hands at rest
                    if !self.follow_road && phase > 0.66 && nfs_game::config::cfg().nfs_no_steer.var_os().is_none() {
                        self.input.press(Action::SteerRight);
                    }
                }
                self.simulate();
                if self.auto_replay_due {
                    self.start_auto_replay(event_loop);
                }
                // diagnostic: `NFS_REPLAY_STOP_EXHAUSTED=1` stops a scripted playback on the first
                // tick its stream has run out (`[0x5E99F4] == 3`), before the frame's end test, so
                // `--screenshot` shows the frame the original draws last before it leaves
                if nfs_game::config::cfg().nfs_replay_stop_exhausted.var_os().is_some()
                    && self.replay.stream.as_ref().is_some_and(|r| r.mode == nfs_game::replay::Mode::Exhausted)
                {
                    tracing::info!(tick = self.tick, "diagnostic: the scripted run stops at the stream's end");
                    break;
                }
                // diagnostic: `NFS_RESULTS_STOP=<n>[:<ticks>]` stops a scripted run `ticks` (96)
                // after the n-th result page of the queue (0-based) was first drawn, its slide-in
                // over, so `--screenshot` shows it (with `NFS_FX_FRAMES`, which draws the frames
                // that open and time the pages)
                if let Some((n, wait)) = nfs_game::config::cfg().nfs_results_stop.var().ok().and_then(|v| {
                    let (n, w) = v.split_once(':').map_or((v.as_str(), "96"), |(n, w)| (n, w));
                    Some((n.trim().parse::<u8>().ok()?, w.trim().parse::<u64>().unwrap_or(96)))
                }) && self.results.is_some()
                    && self.results_page == n
                    && self.page_seen == Some(n)
                    && self.page_open_tick.is_some_and(|t0| self.tick >= t0 + wait)
                {
                    tracing::info!(tick = self.tick, page = ?self.result_pages().get(n as usize), "diagnostic: the scripted run stops on the result page");
                    break;
                }
                // a scripted run's iteration is a frame of one tick
                if self.replay_exit_check(event_loop) {
                    return;
                }
                let bar_save_was_up = self.replay_save.up;
                self.run_bar_script();
                self.replay_bar_frame(event_loop);
                // diagnostic: `NFS_REPLAY_TYPE` typed into `HReplaySave`'s name once the bar's
                // scripted press (or a real one) has just opened it (0x430E10, `replay_save_click`
                // reads it from the field the same way)
                if !bar_save_was_up
                    && self.replay_save.up
                    && let Ok(name) = nfs_game::config::cfg().nfs_replay_type.var()
                {
                    for c in name.chars() {
                        self.replay_save.type_char(c);
                    }
                }
                // NFS_AI_TRACE=<opponent index>: per-quarter-second trace of one AI car (target vs speed, pedals, slip)
                if t % 16 == 0
                    && let Some(idx) = nfs_game::config::cfg().nfs_ai_trace.var()
                        .ok()
                        .and_then(|v| v.parse::<usize>().ok())
                    && let Some(o) = self.road.opponents.get(idx)
                {
                    let target = match &o.controller {
                        Controller::Ai(a) => a.target_speed * 3.6,
                        _ => -1.0,
                    };
                    let (n_ahead, t_off, t_dot) = match &o.controller {
                        Controller::Ai(a) => (a.dbg_n, a.dbg_offset, a.dbg_dot),
                        _ => (0, 0.0, 0.0),
                    };
                    let car_off = self
                        .slices
                        .as_ref()
                        .map(|t| {
                            let sl = &t.slices[o.progress.slice];
                            (o.vehicle.position - sl.centre).dot(sl.lateral)
                        })
                        .unwrap_or(0.0);
                    let lat = o.vehicle.v_local.x;
                    tracing::info!(t = t as f32 / 64.0, slice = o.progress.slice, speed = (o.vehicle.speed() * 3.6) as i32, target = target as i32, thr = (o.vehicle.throttle * 100.0) as i32, brk = (o.vehicle.brake * 100.0) as i32, steer = o.vehicle.steer, n_ahead, t_off = (t_off * 10.0).round() / 10.0, t_dot = (t_dot * 100.0).round() / 100.0, car_off = (car_off * 10.0).round() / 10.0, v_lat = lat, slip = ?o.vehicle.wheels.iter().map(|w| (w.slip * 10.0) as i32).collect::<Vec<_>>(), locked = ?o.vehicle.wheels.iter().map(|w| w.locked as u8).collect::<Vec<_>>(), walls = o.wall_hits, "ai trace");
                }
                if t % 64 == 0
                    && let Some(v) = &self.vehicle
                {
                    tracing::info!(t = t / 64, speed_kmh = v.speed() * 3.6, gear = v.gear, rpm = v.rpm, slice = self.race.as_ref().map(|r| r.slice), walls = self.wall_hits, resets = self.resets, damage = ?v.damage.map(|d| d as i32), opp = ?self.road.opponents.iter().map(|o| (o.progress.slice, (o.vehicle.speed() * 3.6) as i32, match &o.controller { Controller::Ai(a) => (a.target_speed * 3.6) as i32, _ => -1 }, o.vehicle.gear, o.wall_hits, o.resets, o.vehicle.wheels[0].steer as i32, (o.vehicle.throttle * 100.0) as i32, (o.vehicle.brake * 100.0) as i32, o.vehicle.position.x as i32, o.vehicle.position.z as i32)).collect::<Vec<_>>(), wrong_way = self.race.as_ref().map(|r| r.wrong_way_ticks > 0), police = ?self.road.police.as_ref().map(|p| p.cars.iter().filter(|c| c.active).map(|c| (c.slice, (c.speed * 3.6) as i32, (c.position - v.position).length() as i32, c.mode)).collect::<Vec<_>>()), violation = ?self.road.police.as_ref().map(|p| p.violation), traffic = ?self.road.traffic.as_ref().map(|tr| tr.cars.iter().filter(|c| c.active).map(|c| (c.slice, c.dir as i32, (c.speed * 3.6) as i32, (c.lateral * 10.0) as i32, (c.position - v.position).length() as i32, (c.position.y - v.position.y) as i32)).collect::<Vec<_>>()), pos = ?v.position, "autotest");
                }
            }
            if self.session_step(event_loop, secs) {
                return;
            }
            // 0x4EDF00's record arm runs on every race end, career or not, type 10 (the finale)
            // included ([`App::file_track_record`]) — `end_finished_race` calls it first for the
            // interactive path; this scripted fallthrough (the last race of an `--autotest`
            // session, or a single one) skipped it entirely until now, so a scripted finale (or
            // any other scripted race that runs off the end of its ticks here) filed the career
            // event but never the track record. *(Until 2026-09-27 this diagnostic path called
            // neither `file_track_record` nor `end_finished_race`.)*
            self.file_track_record();
            self.rebuild_scene();
            self.update_damage_decals();
            // 0x501340 + `LoadInfo` (0x4D3B52), same as the interactive `end_finished_race` path
            self.reload_career_at_fe_entry();
            self.settle_mission();
            if self.career.as_ref().is_some_and(|c| c.pending.is_some()) {
                let (order, times) = self.finishing_order();
                let clean = self.clean_racers();
                let zones = self.result_damage();
                if let Some(c) = self.career.as_mut() {
                    if let Some(z) = zones {
                        c.park(z);
                    }
                    c.finish(&order, &times, &clean);
                    tracing::info!(order = ?order, message = c.message, money = c.career.money, "career event recorded");
                }
            }
            // a race the layout front end started (`NFS_FE_PLAYGAME`) goes back to it as the last
            // results page does, so the log names the screen 0x4D3420 opens
            if self.fe.race_game_type.is_some()
                && let Some(mut menu) = self.fe.menu_template.take()
            {
                self.knockout_race_end(&mut menu);
                self.career_opponents = None;
                self.player_grid_override = None;
                self.stop_race_audio();
                self.fe.menu = Some(menu);
                self.fe_after_race(false);
                // diagnostic: `NFS_FE_KO_SERIES=1` keeps pressing `PLAYGAME` while the knockout
                // series runs, each round another `--autotest` race of the same length, so a
                // scripted run drives a whole series and the log carries every 0x4DE060
                if nfs_game::config::cfg().nfs_fe_ko_series.var_os().is_some() && self.knockout.as_ref().is_some_and(|k| k.state.active) {
                    self.autotest = Some(secs);
                    self.fe.game_type = 2;
                    self.start_from_layout(event_loop);
                    return;
                }
            }
            if self.screenshot.is_none() {
                // The original saves the replay from the race de-init chain after *every* race,
                // finished or not (replay.md §6.2) — so a scripted run saves on its way out too.
                self.save_replay();
                // scripted runs never stay open
                event_loop.exit();
            }
        }
        // with `NFS_RESULTS_STOP` the still waits for the scripted run to reach its page: the
        // automatic replay's pass (started inside that run) does not take it at its first frame
        let results_stop_pass = race_pass && nfs_game::config::cfg().nfs_results_stop.var_os().is_some();
        if !results_stop_pass
            && let Some(path) = self.screenshot.take()
        {
            // a scripted run seldom redraws, so the mirror may not have a texture yet: render it,
            // and rebuild the scene once more so the HUD carries it
            if self.rear_mirror.is_some() {
                let first = self.mirror_tex.is_none();
                self.render_mirror();
                if first {
                    self.rebuild_scene();
                }
            }
            // The scripted loop never calls `draw_frame`, which is otherwise the only place that
            // sets `set_trilinear` / `set_fakesort_mode` (both `SPECIALFX`-driven) each frame —
            // so a `--screenshot` run kept the renderer's construction-time defaults whatever
            // `Graphics.SpecialFX` said. Set them here too, or `NFS_CHOICE=Graphics.
            // SpecialFX=…` cannot show either one in a still.
            let fx_level = self.race.is_some().then(|| self.special_fx_level());
            let fakesort_mode = self.fakesort_mode(fx_level);
            if let Some(r) = self.renderer.as_mut() {
                r.set_trilinear(fx_level.is_some_and(|l| l >= 3));
                r.set_fakesort_mode(fakesort_mode);
            }
            // **TEST GLUE** (not in the original): `NFS_LENS_WARMUP=<n>` — the sun's lens flare runs over
            // frames and wall-clock (its stored block is taken on the first frame the sun is on
            // screen, its test every 200 ms, and the read-backs come back a frame later), and a
            // scripted run draws no frame; so the still is preceded by n frames drawn 210 ms apart
            // (`render_to_image`, discarded), the scene rebuilt for each as a frame does
            if let Some(n) = nfs_game::config::cfg().nfs_lens_warmup.var().ok().and_then(|v| v.parse::<u32>().ok()) {
                for _ in 0..n {
                    self.rebuild_scene();
                    let _ = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1);
                    std::thread::sleep(Duration::from_millis(210));
                }
                self.rebuild_scene();
            }
            self.sync_gamma();
            let rgba = self
                .renderer
                .as_mut()
                .unwrap()
                .render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1)
                .expect("offscreen render");
            image::save_buffer(&path, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8)
                .expect("write png");
            tracing::info!(path, "screenshot written");
            // **TEST GLUE** (not in the original): `NFS_SHOT_SEQ=<n>` also writes the frames a windowed
            // run draws between this tick and the next — `<shot>.seq<k>.png` at the interpolation
            // fraction k / n (the mirror rebuilt with each, as a frame does), k = 0…n − 1
            if let Some(n) = nfs_game::config::cfg().nfs_shot_seq.var().ok().and_then(|v| v.parse::<u32>().ok()) {
                for k in 0..n {
                    self.test_alpha = Some(k as f32 / n as f32);
                    self.rebuild_scene();
                    self.render_mirror();
                    let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1).expect("offscreen render");
                    let sp = format!("{}.seq{k}.png", path.trim_end_matches(".png"));
                    image::save_buffer(&sp, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8).expect("write png");
                    if nfs_game::config::cfg().nfs_mirror_dump.var().is_ok()
                        && let (Some(tex), Some(r)) = (self.mirror_tex, self.renderer.as_mut())
                        && let Ok((w, h, px)) = r.read_texture(tex)
                    {
                        image::save_buffer(format!("{}.mirror.png", sp.trim_end_matches(".png")), &px, w, h, image::ColorType::Rgba8).expect("write mirror png");
                    }
                }
                self.test_alpha = None;
            }
            // diagnostic: `NFS_MIRROR_DUMP=1` writes the mirror's own texture next to the screenshot
            if nfs_game::config::cfg().nfs_mirror_dump.var().is_ok()
                && let (Some(tex), Some(r)) = (self.mirror_tex, self.renderer.as_mut())
                && let Ok((w, h, px)) = r.read_texture(tex)
            {
                let mp = format!("{}.mirror.png", path.trim_end_matches(".png"));
                image::save_buffer(&mp, &px, w, h, image::ColorType::Rgba8).expect("write mirror png");
                tracing::info!(path = mp, w, h, "mirror texture written");
            }
            event_loop.exit();
        }
    }

    /// **TEST GLUE** (not in the original): the `NFS_HUD_KEYS` script armed afresh for the next race
    /// of a scripted session (`NFS_AUTOTEST_RACES`) — from `NFS_HUD_KEYS_<k>` for race `k`
    /// (1-based) when set, else `NFS_HUD_KEYS` again — so every race of the session is driven alike.
    pub(crate) fn rearm_hud_keys(&mut self) {
        self.scripted_race_no += 1;
        let k = self.scripted_race_no + 1;
        self.hud_key_script = std::env::var(format!("NFS_HUD_KEYS_{k}"))
            .or_else(|_| std::env::var("NFS_HUD_KEYS"))
            .map(|s| parse_hud_keys(&s))
            .unwrap_or_default();
    }

    pub(crate) fn restart_race(&mut self, event_loop: &ActiveEventLoop) {
        // The pause menu's restart (command 3, 0x415721) resets the race audio through 0x4AB360
        // (a hard stop), never through the resume 0x4AB200 — there is no shared "paused" flag on
        // the original's side to desync. The port tracks pause/resume with `audio_paused`
        // (checked once a frame by `sync_audio_pause` against `self.paused`); clearing it here
        // keeps the next frame's check a no-op, so only `start_race`'s own race-start pick
        // (`AudioLoadTrack`'s `MusicPlay(NULL, 0)`, 0x4A8A4A) runs. *(Until this port pass, a
        // restart from the pause menu left `audio_paused` set, so the next frame's
        // `sync_audio_pause` also ran the resume's pick, `play_next` — an extra draw from
        // `App::fe_rng` after the race-start pick already made one — recorded 2026-09-27 as a
        // divergence, now matched: one pick, the race-start one.)*
        self.audio_paused = false;
        self.stop_race_audio();
        // 0x4AB360's own `MusicStop(0)` (0x4AB3EF, after its voice sweep): the song stops at once —
        // already the case after the pause menu's 0x4AB18E, so it does nothing there, but a
        // restart that finds a song chosen cuts it instead of fading it over the race start's 1500
        // 
        if let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut()) {
            m.stop(a);
        }
        self.replay.stream = Some(nfs_game::replay::Replay::new());
        // **The pump's answer 3** (0x425EF0, switch case 3): outside game types 2, 4 and 10
        // (0x426011…0x42602B) the top dialog's slot 5(0) is called (0x42604B) and every queued
        // dialog popped — `[0x60C9C4]`'s end stepped back 8 a dialog (0x426053…0x426061) — before
        // 0x426A80(0). A restart from `HMissionSummary`'s "race again" (type 9) thus leaves no result
        // page queued: the page's stand-in [`App::results_billboard`] goes, so the restarted race
        // has no dialog up ([`App::hud_dialog_up`]); the frame's `billboard_key` follows it as for
        // the pause menu's restart. (Types 2 / 4 / 10 reach a restart only from the pause menu,
        // which keeps no result page.)
        self.results_billboard = None;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return };
        // `[0x606AC8]` runs on through the pass (`hud_clock_base`)
        self.hud_clock_base += self.tick as f64;
        self.tick = 0;
        self.race_pass = true;
        self.start_race(renderer, window, event_loop);
    }
}

/// A colour accessor's dword — the record's bytes byte-swapped, `b0<<24 | b1<<16 | b2<<8 | b3` —
/// as `[r, g, b, a]`: `b1 b2 b3` the colour, `b0` the alpha (0xFF in every shipped entry).
/// The `.STARTINGPOS` a `grid_pos` (`g − 1`, `u32::MAX` for the network draw's 0) stands for, for logs.
pub(crate) fn g_log(grid_pos: Option<u32>) -> String {
    grid_pos.map_or_else(|| "-".to_string(), |g| (g as i32 + 1).to_string())
}

/// `NUM_LAPS` for a track the front end chose. 0x4FC0B0 reads the row's `Laps` accessor **only when
/// the track is a circuit** (`nfs5.trk +0x6E == 1`, tested at 0x4FC17F for the single race and at
/// 0x4FC2E7 for a knockout round) and otherwise writes a flat **1** (0x4FC1C8 / 0x4FC32C). So a
/// point-to-point track always runs a single lap, whatever the lap combo box was left on.
/// CONFIRMED.
pub(crate) fn fe_laps(circuits: &[String], track: &str, laps: u32) -> u32 {
    if circuits.iter().any(|c| c.eq_ignore_ascii_case(track)) { laps.max(1) } else { 1 }
}

/// `TRAFFIC_DENSITY` 0..3 → number of traffic cars for a single race, quick knockout and knockout:
/// `_ftol(index · 2.5)` (0x4FC13B, `traffic.md` §1). Career events use `{0, 2, 4, 8}` instead.
pub(crate) const TRAFFIC_COUNT: [u32; 4] = [0, 2, 5, 7];
