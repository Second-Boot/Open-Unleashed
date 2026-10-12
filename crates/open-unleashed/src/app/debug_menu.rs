//! The port's own developer text menu (`--debug-menu`): its keys and its sprites. **TEST GLUE /
//! a developer tool**, no counterpart in the original — the layout front end does not use it. It
//! draws and edits the same [`Menu`] the layout screens' accessors read (the catalogue lists and
//! the race set-up); the fields it drives are `row`, `mode` (also `NFS_MENU_MODE`) and `knockout_line`. Moved out of
//! `input.rs` and `menu.rs` unchanged.

use crate::*;

impl App {
    /// The text menu's keys.
    ///
    /// **The port-only developer text menu (`--debug-menu`) is TEST GLUE / a developer tool: the
    /// original has no counterpart, so nothing here is RE.** What it launches (modes 0, 4 and 5 —
    /// the career modes go through `load_career_race`, which hands back to the layout front end)
    /// must not depend on a race before it in the same process (menu → race → menu → race; the
    /// text menu cannot reach the layout front end). Each launch arm therefore writes every field
    /// the race reads that an earlier arm of this menu writes: track, laps, direction, traffic,
    /// damage, cops, opponents, the car's id / paint / sound set / dials, `quick_knockout`, the grid
    /// and the career opponents. `race_skill` and `camera_cycle` have no writer in this menu (no
    /// row edits them), so they keep the start-up values (`fe.txt` / the command line) in every
    /// launch. *(2026-10-01: the knockout arm did not write `player_car_id`, so a
    /// Knockout round after a Single Race recorded the Single Race's car id in its replay setup.)*
    pub(crate) fn text_menu_key(&mut self, key: MenuKey, event_loop: &ActiveEventLoop) {
        let Some(m) = self.fe.menu.as_mut() else { return };
        let rows = m.rows();
        match key {
            MenuKey::Up => {
                m.row = (m.row + rows - 1) % rows;
                if (1..=3).contains(&m.mode)
                    && m.row > 0
                    && let Some(c) = self.career.as_mut()
                {
                    c.row = m.row - 1;
                }
            }
            MenuKey::Down => {
                m.row = (m.row + 1) % rows;
                if (1..=3).contains(&m.mode)
                    && m.row > 0
                    && let Some(c) = self.career.as_mut()
                {
                    c.row = m.row - 1;
                }
            }
            MenuKey::Left | MenuKey::Right => {
                let delta: i32 = if key == MenuKey::Left { -1 } else { 1 };
                if m.row == 0 {
                    m.mode = if self.career.is_some() {
                        ((m.mode as i32 + delta).rem_euclid(6)) as u8
                    } else {
                        [0u8, 4, 5][((match m.mode {
                            0 => 0,
                            4 => 1,
                            _ => 2,
                        } as i32
                            + delta)
                            .rem_euclid(3)) as usize]
                    };
                    if m.mode == 4
                        && !m
                            .circuits
                            .iter()
                            .any(|c| c.eq_ignore_ascii_case(m.track()))
                        && let Some(i) = m
                            .tracks
                            .iter()
                            .position(|t| m.circuits.iter().any(|c| c.eq_ignore_ascii_case(t)))
                    {
                        m.track_idx = i;
                    }
                    if let Some(c) = self.career.as_mut() {
                        c.set_mode(m.mode);
                    }
                } else if (1..=3).contains(&m.mode) {
                    if let Some(c) = self.career.as_mut() {
                        c.row = m.row - 1;
                        c.move_col(delta);
                    }
                } else {
                    let step = |i: usize, n: usize| {
                        ((i as i32 + delta).rem_euclid(n.max(1) as i32)) as usize
                    };
                    match m.row {
                        1 => {
                            m.track_idx = step(m.track_idx, m.tracks.len());
                            if m.mode == 4 {
                                // Quick Knockout: circuits only
                                for _ in 0..m.tracks.len() {
                                    if m.circuits
                                        .iter()
                                        .any(|c| c.eq_ignore_ascii_case(m.track()))
                                    {
                                        break;
                                    }
                                    m.track_idx = step(m.track_idx, m.tracks.len());
                                }
                            }
                        }
                        2 => {
                            // `GenericCar.Car`'s slot 11 (0x4F3060) sends `GenericCar.Colour` to
                            // `slot 11(0)` on every changed row but the very first call (the
                            // `[+0x38]` latch, 0x4F30A4…0x4F3100): the new car shows its own
                            // colour row 0 (SC 2026-10-01, original QuickRace under Wine)
                            let before = m.car_idx;
                            m.car_idx = step(m.car_idx, m.cars.len());
                            if m.car_idx != before {
                                m.paint_idx = 0;
                                m.paint_custom = None;
                            }
                        }
                        3 => {
                            m.paint_idx = if delta < 0 {
                                m.paint_idx.saturating_sub(1)
                            } else {
                                m.paint_idx + 1
                            }
                        }
                        // the knockout modes edit `Game.NumKOOpponents`, whose minimum is 1
                        // (0x4E5570); everything else edits `Game.NumOpponents`, minimum 0
                        4 => {
                            if m.mode >= 4 {
                                m.ko_opponents = (m.ko_opponents as i32 + delta).clamp(1, 7) as u32;
                                m.fit_ko();
                            } else {
                                m.opponents = (m.opponents as i32 + delta).clamp(0, 7) as u32;
                            }
                        }
                        9 => m.opp_car = ((m.opp_car as i32 + delta).rem_euclid(5)) as u8,
                        5 => m.laps = (m.laps as i32 + delta).clamp(1, 10) as u32,
                        7 => m.traffic = (m.traffic as i32 + delta).clamp(0, 3) as u32,
                        8 => m.cops = !m.cops,
                        _ => {
                            let cur = (m.backwards as i32) | ((m.mirror as i32) << 1);
                            let next = (cur + delta).rem_euclid(4);
                            m.backwards = next & 1 != 0;
                            m.mirror = next & 2 != 0;
                        }
                    }
                }
            }
            MenuKey::Back => {
                if (1..=3).contains(&m.mode)
                    && let Some(c) = self.career.as_mut()
                {
                    c.back();
                }
            }
            MenuKey::Enter if (1..=3).contains(&m.mode) => {
                let Some(req) = self.career.as_mut().and_then(|c| c.activate()) else {
                    // a dialog the career screen asked for is raised on the next frame
                    // (`fe_career_dialog`)
                    return;
                };
                let template = m.race_template();
                self.fe.menu_template = Some(template);
                if !self.load_career_race(req) {
                    if let Some(cu) = self.career.as_mut() {
                        cu.message = "EVENT LOAD FAILED".into();
                    }
                    if let Some(mut mt) = self.fe.menu_template.take() {
                        mt.font = mt.font.take();
                        self.fe.menu = Some(mt);
                    }
                    return;
                }
                self.fe.menu = None;
                let (renderer, window) =
                    (self.renderer.take().unwrap(), self.window.take().unwrap());
                self.tick = 0;
                self.start_race(renderer, window, event_loop);
            }
            MenuKey::Enter if m.mode == 5 => {
                let Some(car) = m.car().cloned() else { return };
                // 0x4D52CB: a new series (`Knockout_start` 0x4DDF30) unless one is running
                // (0x4DDF00), then the round's set-up from the block and the live rows / cars
                if !self.knockout.as_ref().is_some_and(|k| k.state.active) {
                    self.knockout_start();
                }
                let Some(track_name) = self.knockout_round() else { return };
                let Some(m) = self.fe.menu.as_mut() else { return };
                let template = m.race_template();
                let look = PlayerLook::from_menu(m, &car);
                self.player_car_id = Some(car.id);
                self.apply_player_look(look);
                self.fe.menu_template = Some(template);
                let car = self.cheat_993(car);
                tracing::info!(track = track_name, "knockout race");
                match (
                    { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
                    CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio)),
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
                        if self.cheat_bits & CHEAT_TINY != 0 {
                            self.specs.apply_tinycar();
                        }
                        // a freshly built catalogue car: its stock parts (0x4EE9A0) and the constructor's
                        // neutral sliders (0x4EED2D) go through 0x49CB50 like any car's
                        let table = self.fe_parts_table();
                        let automatic = self.player_automatic();
                    nfs_game::career::fold_fresh_car(&mut self.specs, &car.stock_parts, &table, &car.sim_family, automatic);
                        self.snd.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_mph, car.dial_tach));
                        self.fe.menu = None;
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
            MenuKey::Enter => {
                let Some(car) = m.car().cloned() else { return };
                let track_name = m.track().to_string();
                self.player_grid_override = None;
                let template = m.race_template();
                self.opp_choice = m.opp_car as u32;
                self.player_car_id = Some(car.id);
                // the single race (game type 0) with the Fuzzyfuz code is the cops race (0x4EB4FD)
                // (read off `template`, the menu's field-for-field copy, while `m` holds the menu mutably)
                let m = &template;
                let cops_race = m.mode == 0 && self.cheat_bits & CHEAT_COPS != 0;
                let rs = RaceSetup {
                    opponents: if m.mode >= 4 { m.ko_opponents } else { m.opponents },
                    laps: if m.mode == 4 {
                        m.ko_opponents.max(1)
                    } else {
                        fe_laps(&m.circuits, m.track(), m.laps)
                    },
                    cops_race,
                    cops_enabled: m.cops || cops_race,
                    quick_knockout: m.mode == 4,
                    look: PlayerLook::from_menu(m, &car),
                    // `player_grid_override` was cleared at the top of this arm
                    ..RaceSetup::menu(m, self.race_setup())
                };
                self.apply_race_setup(rs);
                self.fe.menu_template = Some(template);
                let car = self.cheat_993(car);
                tracing::info!(
                    track = track_name,
                    car = car.model,
                    sim = car.sim,
                    "menu: start race"
                );
                match (
                    { let derby = self.derby_track(&track_name); TrackAssets::load(&self.paths, &derby) },
                    CarAssets::load(&self.paths, &car.model).map(|c| c.with_style(car.variant).with_cabrio(car.cabrio)),
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
                        if self.cheat_bits & CHEAT_TINY != 0 {
                            self.specs.apply_tinycar();
                        }
                        // a freshly built catalogue car: its stock parts (0x4EE9A0) and the constructor's
                        // neutral sliders (0x4EED2D) go through 0x49CB50 like any car's
                        let table = self.fe_parts_table();
                        let automatic = self.player_automatic();
                    nfs_game::career::fold_fresh_car(&mut self.specs, &car.stock_parts, &table, &car.sim_family, automatic);
                        self.snd.sound_set = car.sound_set.clone();
                        self.sim_name = car.sim.clone();
                        self.dials = Some((car.dial_kmh, car.dial_mph, car.dial_tach));
                        self.fe.menu = None;
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
        }
    }
}

impl Menu {
    pub(crate) fn rows(&self) -> u8 {
        match self.mode {
            0 | 4 | 5 => 10,
            3 => 2,
            _ => 4,
        }
    }

    pub(crate) fn sprites(&self, career: Option<&CareerUi>) -> Vec<Sprite> {
        let Some(f) = &self.font else {
            return Vec::new();
        };
        let mut out = f.sprites(
            &crate::brand::TITLE.to_uppercase(),
            [0.05, 0.08],
            0.05,
            [1.0, 1.0, 1.0, 1.0],
        );
        let sel = |r: u8| {
            if self.row == r {
                [1.0, 0.75, 0.1, 1.0]
            } else {
                [0.85, 0.85, 0.85, 1.0]
            }
        };
        out.extend(f.sprites(
            &format!(
                "MODE    < {} >",
                [
                    "QUICK RACE",
                    "EVOLUTION",
                    "TOURNAMENT",
                    "FACTORY DRIVER",
                    "QUICK KNOCKOUT",
                    "KNOCKOUT"
                ][self.mode.min(5) as usize]
            ),
            [0.08, 0.15],
            0.04,
            sel(0),
        ));
        if (1..=3).contains(&self.mode) {
            match career {
                // career rows share the cursor: menu row 1 = tournament, row 2 = car
                Some(c) => out.extend(c.sprites(f, (self.row > 0).then(|| self.row - 1))),
                None => out.extend(f.sprites(
                    "CAREER TABLES UNAVAILABLE",
                    [0.08, 0.30],
                    0.045,
                    [0.85, 0.85, 0.85, 1.0],
                )),
            }
            out.extend(f.sprites(
                "UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER BUY / RACE   ESC QUIT",
                [0.08, 0.85],
                0.028,
                [0.6, 0.6, 0.6, 1.0],
            ));
            return out;
        }
        if self.mode == 5 {
            out.extend(f.sprites(&self.knockout_line, [0.08, 0.30], 0.045, sel(1)));
        } else {
            out.extend(f.sprites(
                &format!("TRACK   < {} >", self.track().to_string()),
                [0.08, 0.30],
                0.045,
                sel(1),
            ));
        }
        let Some(c) = self.car() else { return Vec::new() };
        out.extend(f.sprites(
            &format!("CAR     < {} >", c.display_name.to_string()),
            [0.08, 0.40],
            0.045,
            sel(2),
        ));
        let paint = c
            .paint_sets
            .get(self.paint_idx % c.paint_sets.len().max(1))
            .map(|p| p.colours[0])
            .unwrap_or([0xD4, 0x10, 0x10]);
        out.extend(f.sprites(
            &format!(
                "PAINT   < {} / {} >",
                self.paint_idx % c.paint_sets.len().max(1) + 1,
                c.paint_sets.len()
            ),
            [0.08, 0.48],
            0.045,
            sel(3),
        ));
        out.extend(f.sprites(
            "####",
            [0.42, 0.48],
            0.045,
            [
                paint[0] as f32 / 255.0,
                paint[1] as f32 / 255.0,
                paint[2] as f32 / 255.0,
                1.0,
            ],
        ));
        out.extend(f.sprites(
            &format!("OPPONENTS  < {} >", if self.mode >= 4 { self.ko_opponents } else { self.opponents }),
            [0.08, 0.545],
            0.045,
            sel(4),
        ));
        // `OpponentCar.Car`: FE strings 640..644, default "same car" (`modes.md` §1.1a)
        out.extend(f.sprites(
            &format!(
                "THEY DRIVE < {} >",
                nfs_game::opponents::OpponentChoice::from_index(self.opp_car as u32).label()
            ),
            [0.08, 0.602],
            0.04,
            sel(9),
        ));
        if self.mode == 4 {
            out.extend(f.sprites(
                &format!("LAPS    {}  (ONE CAR OUT PER LAP)", self.ko_opponents.max(1)),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        } else if self.mode == 5 {
            out.extend(f.sprites(
                &format!(
                    "RACES   {}   LAPS < {} >   (LAST CAR OUT EACH RACE)",
                    self.ko_opponents.max(1),
                    self.laps
                ),
                [0.08, 0.66],
                0.04,
                sel(5),
            ));
        } else {
            out.extend(f.sprites(
                &format!("LAPS    < {} >", self.laps),
                [0.08, 0.66],
                0.045,
                sel(5),
            ));
        }
        // Track1.Direction ids 0x19A..0x19D: forward / backward / forward mirrored / backward mirrored
        let dir = match (self.backwards, self.mirror) {
            (false, false) => "FORWARD",
            (true, false) => "BACKWARD",
            (false, true) => "FORWARD MIRRORED",
            (true, true) => "BACKWARD MIRRORED",
        };
        out.extend(f.sprites(
            &format!("DIRECTION  < {dir} >"),
            [0.08, 0.74],
            0.045,
            sel(6),
        ));
        // FE strings 403/402/401/400; the count is `_ftol(index · 2.5)` = 0/2/5/7 (`traffic.md` §1)
        let density = ["NONE", "LIGHT", "MEDIUM", "HEAVY"][self.traffic.min(3) as usize];
        out.extend(f.sprites(
            &format!(
                "TRAFFIC    < {density} >   ({} CARS)",
                TRAFFIC_COUNT[self.traffic.min(3) as usize]
            ),
            [0.08, 0.80],
            0.04,
            sel(7),
        ));
        out.extend(f.sprites(
            &format!(
                "POLICE     < {} >",
                if self.cops { "ON  (4 CARS)" } else { "OFF" }
            ),
            [0.08, 0.845],
            0.04,
            sel(8),
        ));
        out.extend(f.sprites(
            "UP/DOWN SELECT ROW   LEFT/RIGHT CHANGE   ENTER START   ESC QUIT",
            [0.08, 0.91],
            0.026,
            [0.6, 0.6, 0.6, 1.0],
        ));
        out
    }
}
