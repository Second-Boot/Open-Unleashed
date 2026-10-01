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
        let car = self.cheat_993(car);
        // a record car's parts write the style overlay (0x49C5F7), as in a career race
        let swaps: Vec<[i16; 3]> = match (owned.as_ref(), self.career.as_ref()) {
            (Some(o), Some(cu)) => o.parts.iter().filter_map(|id| cu.tables.parts.iter().find(|p| p.id == *id)).flat_map(|p| p.geometry_swaps.iter().copied()).collect(),
            _ => Vec::new(),
        };
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
        let Some(cu) = self.career.as_mut() else { return false };
        let Some(req) = cu.start_finale() else { return false };
        if !self.load_career_race(req) {
            return false;
        }
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
        let Some(cu) = self.career.as_mut() else { return false };
        if cu.career.garage.is_empty() {
            return false;
        }
        let g = cu.car_sel.min(cu.career.garage.len() - 1);
        let row = self.fe.remaining_sel;
        let Some(req) = cu.start_selected_event(g, row) else { return false };
        let owned = cu.career.garage.get(g).cloned();
        let entry = owned.as_ref().and_then(|o| cu.tables.car(o.car_id)).cloned();
        if !self.load_career_race(req) {
            return false;
        }
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
        self.sync_save_side_state();
        if let Some(c) = self.career.as_mut() {
            c.save();
        }
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
        self.quick_knockout = false;
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
            self.traffic_seed = std::env::var("NFS_TRAFFIC_SEED")
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
            };
        }
        self.replay.setup.seed = self.traffic_seed;
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
        let mut world = TrackWorld::upload(&mut renderer, &self.track).expect("track");
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
        let picture = self.career.as_ref().map_or(nfs_game::save::DEFAULT_PICTURE, |c| c.profile.picture);
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
            let t = std::env::var("NFS_FE_TIME").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or_else(|| {
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
                if let Some(m) = self.snd.music.as_mut() {
                    m.set_front_end(false);
                    m.stop_fade(a, 1500);
                    if !playing {
                        m.start_race(a);
                    }
                }
            }
        }
        // 0x4B6855: step 7 after `SND_coll` 0x4A8CD0 (the collision sounds, `CollisionSfx` above)
        self.race_loading_frame(&mut renderer, 7);
        // 0x414E63: step 8 in the race init 0x414DB0, before `Cars_create` 0x411130 (0x414E68) makes the cars
        self.race_loading_frame(&mut renderer, 8);
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
                let plan: Vec<(
                    nfs_formats::fedata::CarCatalogEntry,
                    [u8; 3],
                    String,
                    Option<(f32, [[f32; 2]; 4])>,
                    Option<u32>,
                    i32,
                )> = match &self.career_opponents {
                    Some(list) => list
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
                    None => {
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
                for (g, (entry, paint, name, skill, grid, decal)) in plan.iter().enumerate() {
                    // the opponent record's parts (0x4ED880), for a career opponent
                    let parts: Vec<nfs_formats::career::Part> = self.career_opponents.as_ref().and_then(|l| l.get(g)).map(|o| o.parts.clone()).unwrap_or_default();
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
                    let car_tiny = seat_rec.map_or(self.cheat_bits & (1 << 1) != 0, |r| r.num(nfs_net::launchcar::id::TINYCAR).is_some_and(|v| v != 0));
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
                        &mut renderer,
                        &assets,
                        &self.paths,
                        1,
                        paint,
                        car_env,
                        self.track.is_night(),
                        // `.CABRIODOWN` 0x24B: 0 for every car but a player's (0x4EEF0C), whose own
                        // machine sent `Car.Roof` through 0x4F1E40's predicate
                        Some(seat_rec.and_then(|r| r.num(nfs_net::launchcar::id::CABRIODOWN)).unwrap_or(0)),
                        self.skidpad_rule(),
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
                        .map_or_else(|| self.player_automatic(), |v| v == 1);
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
                            let mut d = nfs_game::ai::AiDriver::new(ais, &specs, &sim.unknown_tail);
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
                    let traffic_automatic = self.player_automatic();
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
                                let inst = self.road.traffic_models[t].instantiate(&mut renderer);
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
                                    &mut renderer,
                                    &assets,
                                    &self.paths,
                                    1,
                                    paint,
                                    car_env,
                                    self.track.is_night(),
                                    Some(0),
                                    self.skidpad_rule(),
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
                                let (lo, hi) = nfs_game::world::car_model_box(&assets, 1);
                                boxes.push((hi - lo) * 0.5);
                                points.push(nfs_game::world::car_body(&assets, self.cheat_bits & (1 << 1) != 0).0);
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
                                    let inst = self.road.police_models[t].instantiate(&mut renderer);
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
                                        &mut renderer,
                                        &assets,
                                        &self.paths,
                                        1,
                                        paint,
                                        car_env,
                                        self.track.is_night(),
                                        Some(0),
                                        self.skidpad_rule(),
                                    ) else {
                                        continue;
                                    };
                                    let mut model = model;
                                    if self.cheat_bits & (1 << 1) != 0 {
                                        model.scale = 0.4;
                                    }
                                    self.road.police_models.push(model);
                                    // 0x445090: a cop (`car+0x52C` 0x22 / 0x62, bit 0x10 clear at
                                    // 0x4450D2) takes `[this+0xB0][4]`, LOD 5, as a racer does;
                                    // `× 0.4` for `.TINYCAR` (0x445140)
                                    let (points, half) = nfs_game::world::car_body(&assets, self.cheat_bits & (1 << 1) != 0);
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
                            let cop_automatic = self.player_automatic();
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
                            cop.body.tiny = self.cheat_bits & (1 << 1) != 0;
                            // `DERBY` `[0x657400]`, which only the Smash Up code sets (cheat bit 7): `Damage` takes ×1.5, not ×0.5 (0x49452F)
                            cop.body.derby = self.cheat_bits & (1 << 7) != 0;
                            cop.body.rally = self.cheat_bits & 1 != 0;
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
        if let Some(owned) = self
            .career
            .as_ref()
            .and_then(|cu| cu.pending.as_ref().and_then(|p| p.garage_idx).and_then(|i| cu.career.garage.get(i)))
            .or(self.fe.test_drive.as_ref().and_then(|(_, o)| o.as_ref()))
        {
            vehicle.damage = owned.damage;
            tracing::info!(damage = ?owned.damage, "body damage carried into the race");
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
        // `GameData/Render/common.fsh`: `SKDP` is the skid marks' texture (the one the original's
        // per-surface table points at throughout, 0x5D00E8) and `SHAD` the shadow every car casts
        // (0x43B960 looks it up by that FourCC)
        // `pair` is the draw's Thrash 0x38 blend (`thrash.md` §6): the skid marks and the shadow
        // keep the state object's default 0, the lamp glare's object gets 1 (0x43CF5B, additive)
        let sprite = |renderer: &mut Renderer,
                      quad: &nfs_renderer::MeshData,
                      name: &str,
                      pair: u8|
         -> Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)> {
            let path = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")?;
            let data = std::fs::read(&path).ok()?;
            let images = nfs_assets::texture::decode_pack(&path, &data).ok()?;
            // an exact name first — `common.fsh` has both `spik` and `SPIK` (0x44D320 asks by FourCC)
            let img = images
                .iter()
                .find(|t| t.name.trim() == name)
                .or_else(|| images.iter().find(|t| t.name.trim().eq_ignore_ascii_case(name)))?;
            let tex = renderer.upload_texture(&nfs_renderer::TextureData {
                width: img.width,
                height: img.height,
                mips: img.mips.clone(),
            });
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
        self.particle_draw = sprite(&mut renderer, &quad, "SKDP", 0);
        self.shadow_draw = sprite(&mut renderer, &quad, "SHAD", 0);
        self.glare_draw = sprite(&mut renderer, &quad, "CARG", 1);
        self.glare_star = [sprite(&mut renderer, &quad, "SPIK", 1), sprite(&mut renderer, &quad, "GLNT", 1)];
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
        self.lamp_pools.reflection_material = sprite(&mut renderer, &quad, "BLUR", 1).map(|(_, m)| m);
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
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
        {
            let mut names: Vec<String> = self.flare_defs.iter().flat_map(|d| d.textures.iter().flatten().cloned()).collect();
            names.sort();
            names.dedup();
            for name in names {
                let Some(img) = images.iter().find(|t| t.name.trim() == name) else {
                    tracing::debug!(name, "flare texture not in common.fsh");
                    continue;
                };
                let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
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
                // defaults with the pair set to 1, additive (0x46088E / 0x46089D)
                if self.flare_defs.iter().any(|d| d.id == SUN_FLARE && d.textures.iter().flatten().any(|t| *t == name)) {
                    let sun = renderer.create_material(nfs_renderer::MaterialDesc {
                        alpha_blend: true,
                        cull_back: false,
                        depth_test: false,
                        blend_pair: 1,
                        ..nfs_renderer::MaterialDesc::textured(tex)
                    });
                    self.sun_sprites.insert(name.clone(), (mesh, sun));
                }
                self.flare_sprites.insert(name, (mesh, material));
            }
        }
        // `HEAD` — what `Headlights::LayerUVLight` projects (0x452645 fetches it by that
        // four-character code into `[0x61E1D4]`, whatever the night flag says)
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
            && let Some(img) = images.iter().find(|t| t.name.trim() == "HEAD")
        {
            let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
            renderer.set_headlight_texture(Some(tex));
        }
        // the windscreen drops (0x4577A0 from 0x458FF5): `SPLT` (0x544C5053) of the shared bundle
        // `[0x613E04]` = `common.fsh` (0x448B10's first of two, 0x5CDEAC); the state object
        // 0x5379F0 with the pair set to 1, additive (0x4578B4); the draw object 0x468240(9, 3, 1,
        // 0, 0) over 201 triangles; no depth writes (0x459200 turns them off around the draw)
        self.splat_draw = None;
        if let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh")
            && let Ok(data) = std::fs::read(&path)
            && let Ok(images) = nfs_assets::texture::decode_pack(&path, &data)
            && let Some(img) = images.iter().find(|t| t.name.trim() == "SPLT")
        {
            let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
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
                let tex = *textures.entry(name.clone()).or_insert_with(|| renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() }));
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
                    let tex = renderer.upload_texture(&nfs_renderer::TextureData { width: img.width, height: img.height, mips: img.mips.clone() });
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
        if let Some(k) = std::env::var("NFS_PROJECTOR_VIEW").ok().and_then(|v| v.parse::<usize>().ok())
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
            let smack_frames = std::env::var_os("NFS_SMACK_DRIVE").is_some() || std::env::var_os("NFS_CONE_DRIVE").is_some() || std::env::var_os("NFS_FX_FRAMES").is_some();
            let mut pause_hold = 0;
            // **TEST GLUE** (not in the original): `NFS_NET_PACE=1` holds a scripted network race to
            // the game's own 64 Hz, so the two machines' tick counters stay comparable — the link
            // classification (0x490140) measures a packet's age against the game tick, and an unpaced
            // headless run lets one machine's tick run far ahead of the other's
            let exchange_run = self.net.session.as_ref().is_some_and(|n| n.exchanging());
            let pace = (std::env::var_os("NFS_NET_PACE").is_some() && self.net.session.is_some() && !exchange_run).then(Instant::now);
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
                        self.sync_audio_pause();
                    }
                    continue;
                }
                if smack_frames && t % 2 == 1 {
                    self.rebuild_scene();
                }
                let phase = t as f32 / ticks as f32;
                if t % 2 == 0 {
                    self.input.press(Action::Throttle);
                    if !self.follow_road && phase > 0.66 {
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
                if std::env::var_os("NFS_REPLAY_STOP_EXHAUSTED").is_some()
                    && self.replay.stream.as_ref().is_some_and(|r| r.mode == nfs_game::replay::Mode::Exhausted)
                {
                    tracing::info!(tick = self.tick, "diagnostic: the scripted run stops at the stream's end");
                    break;
                }
                // diagnostic: `NFS_RESULTS_STOP=<n>[:<ticks>]` stops a scripted run `ticks` (96)
                // after the n-th result page of the queue (0-based) was first drawn, its slide-in
                // over, so `--screenshot` shows it (with `NFS_FX_FRAMES`, which draws the frames
                // that open and time the pages)
                if let Some((n, wait)) = std::env::var("NFS_RESULTS_STOP").ok().and_then(|v| {
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
                    && let Ok(name) = std::env::var("NFS_REPLAY_TYPE")
                {
                    for c in name.chars() {
                        self.replay_save.type_char(c);
                    }
                }
                // NFS_AI_TRACE=<opponent index>: per-quarter-second trace of one AI car (target vs speed, pedals, slip)
                if t % 16 == 0
                    && let Some(idx) = std::env::var("NFS_AI_TRACE")
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
                if std::env::var_os("NFS_FE_KO_SERIES").is_some() && self.knockout.as_ref().is_some_and(|k| k.state.active) {
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
        let results_stop_pass = race_pass && std::env::var_os("NFS_RESULTS_STOP").is_some();
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
            if let Some(n) = std::env::var("NFS_SHOT_SEQ").ok().and_then(|v| v.parse::<u32>().ok()) {
                for k in 0..n {
                    self.test_alpha = Some(k as f32 / n as f32);
                    self.rebuild_scene();
                    self.render_mirror();
                    let rgba = self.renderer.as_mut().unwrap().render_to_image(&self.scene, &self.camera, shot_geometry().0, shot_geometry().1).expect("offscreen render");
                    let sp = format!("{}.seq{k}.png", path.trim_end_matches(".png"));
                    image::save_buffer(&sp, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8).expect("write png");
                    if std::env::var("NFS_MIRROR_DUMP").is_ok()
                        && let (Some(tex), Some(r)) = (self.mirror_tex, self.renderer.as_ref())
                        && let Ok((w, h, px)) = r.read_texture(tex)
                    {
                        image::save_buffer(format!("{}.mirror.png", sp.trim_end_matches(".png")), &px, w, h, image::ColorType::Rgba8).expect("write mirror png");
                    }
                }
                self.test_alpha = None;
            }
            // diagnostic: `NFS_MIRROR_DUMP=1` writes the mirror's own texture next to the screenshot
            if std::env::var("NFS_MIRROR_DUMP").is_ok()
                && let (Some(tex), Some(r)) = (self.mirror_tex, self.renderer.as_ref())
                && let Ok((w, h, px)) = r.read_texture(tex)
            {
                let mp = format!("{}.mirror.png", path.trim_end_matches(".png"));
                image::save_buffer(&mp, &px, w, h, image::ColorType::Rgba8).expect("write mirror png");
                tracing::info!(path = mp, w, h, "mirror texture written");
            }
            event_loop.exit();
        }
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
