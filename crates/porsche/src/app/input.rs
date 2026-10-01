//! Keyboard, gamepad and mouse input: the menu and layout keys and the front end's mouse handling.

use crate::*;

impl App {
    /// Menu navigation shared by the keyboard and the gamepad.
    pub(crate) fn menu_key(&mut self, key: MenuKey, event_loop: &ActiveEventLoop) {
        // `--layout` browse mode: the screens navigate each other through their own `destination` fields
        if self.fe.layout_preview.is_some() {
            self.layout_key(key, event_loop);
            return;
        }
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
                let template = Menu {
                    mode: m.mode,
                    tracks: m.tracks.clone(),
                    looped: m.looped.clone(),
                    tracks_year: m.tracks_year,
                    cars: m.cars.clone(),
                    cars2: m.cars2.clone(),
                    track_idx: m.track_idx,
                    car_idx: m.car_idx,
                    car_garage: m.car_garage,
                    car2_idx: m.car2_idx,
                    ko: m.ko.clone(),
                    ko_sel: m.ko_sel,
                    opponents: m.opponents,
                    ko_opponents: m.ko_opponents,
                    damage: m.damage,
                    difficulty: m.difficulty,
                    opp_car: m.opp_car,
                    paint_idx: m.paint_idx,
                    paint_custom: m.paint_custom,
                    laps: m.laps,
                    traffic: m.traffic,
                    cops: m.cops,
                    backwards: m.backwards,
                    mirror: m.mirror,
                    circuits: m.circuits.clone(),
                    looped_idx: m.looped_idx,
                    knockout_line: m.knockout_line.clone(),
                    row: m.row,
                    font: m.font.take(),
                };
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
                let template = Menu {
                    mode: m.mode,
                    tracks: m.tracks.clone(),
                    looped: m.looped.clone(),
                    tracks_year: m.tracks_year,
                    cars: m.cars.clone(),
                    cars2: m.cars2.clone(),
                    track_idx: m.track_idx,
                    car_idx: m.car_idx,
                    car_garage: m.car_garage,
                    car2_idx: m.car2_idx,
                    ko: m.ko.clone(),
                    ko_sel: m.ko_sel,
                    opponents: m.opponents,
                    ko_opponents: m.ko_opponents,
                    damage: m.damage,
                    difficulty: m.difficulty,
                    opp_car: m.opp_car,
                    paint_idx: m.paint_idx,
                    paint_custom: m.paint_custom,
                    laps: m.laps,
                    traffic: m.traffic,
                    cops: m.cops,
                    backwards: m.backwards,
                    mirror: m.mirror,
                    circuits: m.circuits.clone(),
                    looped_idx: m.looped_idx,
                    knockout_line: m.knockout_line.clone(),
                    row: m.row,
                    font: m.font.take(),
                };
                self.owned_paint = None;
                self.owned_decal = None;
                self.paint = car
                    .paint_sets
                    .get(m.paint_idx % car.paint_sets.len().max(1))
                    .map(|p| p.colours[0])
                    .unwrap_or([0xD4, 0x10, 0x10]);
                if let Some(p) = Self::menu_custom_paint(&m, &car) {
                    (self.paint, self.owned_paint) = (p.exterior, Some(p));
                }
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
                        if self.cheat_bits & (1 << 1) != 0 {
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
                let template = Menu {
                    mode: m.mode,
                    tracks: m.tracks.clone(),
                    looped: m.looped.clone(),
                    tracks_year: m.tracks_year,
                    cars: m.cars.clone(),
                    cars2: m.cars2.clone(),
                    track_idx: m.track_idx,
                    car_idx: m.car_idx,
                    car_garage: m.car_garage,
                    car2_idx: m.car2_idx,
                    ko: m.ko.clone(),
                    ko_sel: m.ko_sel,
                    opponents: m.opponents,
                    ko_opponents: m.ko_opponents,
                    damage: m.damage,
                    difficulty: m.difficulty,
                    opp_car: m.opp_car,
                    paint_idx: m.paint_idx,
                    paint_custom: m.paint_custom,
                    laps: m.laps,
                    traffic: m.traffic,
                    cops: m.cops,
                    backwards: m.backwards,
                    mirror: m.mirror,
                    circuits: m.circuits.clone(),
                    looped_idx: m.looped_idx,
                    knockout_line: m.knockout_line.clone(),
                    row: m.row,
                    font: m.font.take(),
                };
                self.num_opponents = if m.mode >= 4 { m.ko_opponents } else { m.opponents };
                self.opp_choice = m.opp_car as u32;
                self.player_car_id = Some(car.id);
                self.quick_knockout = m.mode == 4;
                self.num_laps = if m.mode == 4 {
                    m.ko_opponents.max(1)
                } else {
                    fe_laps(&m.circuits, m.track(), m.laps)
                };
                self.backwards = m.backwards;
                self.mirror = m.mirror;
                self.career_opponents = None;
                self.road.traffic_count = TRAFFIC_COUNT[m.traffic.min(3) as usize];
                self.road.traffic_ids = None;
                self.road.damage_on = m.damage;
                // the single race (game type 0) with the Fuzzyfuz code is the cops race (0x4EB4FD)
                self.road.cops_race = m.mode == 0 && self.cheat_bits & (1 << 5) != 0;
                self.road.cops_enabled = m.cops || self.road.cops_race;
                self.road.cop_count = 4;
                self.owned_paint = None;
                self.owned_decal = None;
                self.paint = car
                    .paint_sets
                    .get(m.paint_idx % car.paint_sets.len().max(1))
                    .map(|p| p.colours[0])
                    .unwrap_or([0xD4, 0x10, 0x10]);
                if let Some(p) = Self::menu_custom_paint(&m, &car) {
                    (self.paint, self.owned_paint) = (p.exterior, Some(p));
                }
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
                        if self.cheat_bits & (1 << 1) != 0 {
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

    /// Navigate the previewed front-end screen: the buttons carry the name of the screen they lead to.
    pub(crate) fn layout_key(&mut self, key: MenuKey, event_loop: &ActiveEventLoop) {
        let Some(l) = self.fe.layout_preview.as_ref() else {
            return;
        };
        let controls = l.controls();
        if controls.is_empty() {
            if key == MenuKey::Back {
                self.fe_back(0, event_loop);
            }
            return;
        }
        let sel = self.fe.layout_sel.min(controls.len() - 1);
        match key {
            MenuKey::Up => {
                self.fe.layout_sel = (sel + controls.len() - 1) % controls.len();
                self.fe.layout_nav_used = true;
            }
            MenuKey::Down => {
                self.fe.layout_sel = (sel + 1) % controls.len();
                self.fe.layout_nav_used = true;
            }
            // A key reaches **no combo** (`controls.md` §4.171.8, CONFIRMED). A key is a type-2
            // event; 0x5289B0 hands it to the capture or the widget under the pointer, whose
            // slot 3 0x525F80 sends type 2 to slot 4, and `FeComboBox`'s (0x5B5C24 + 0x10) and
            // `NFS5.ColorBox`'s (0x5B5BA8 + 0x10) slot 4 is 0x4D1B40 (`xor al, al; ret 4`); the
            // front end's own slot 4 0x4D2970 knows ESC, `b` and `=` alone, and no pad reaches the
            // front end (§4.169.5). The wrapping pair 0x4BC660 / 0x4BC690 is reached by the
            // caption-strip release (slot 21 only, `combo_strip_release`) and by
            // `NFS5.AccessorIterator`'s two buttons (0x4BCE00, no shipped widget). The port's
            // earlier Left / Right stepping of a combo through `set_accessor(∓1)` was its own
            // invention and is **removed** (NH, 2026-10-02): a combo ignores both keys.
            //
            // **And no other class steps its accessor by a key either** (NM, read 2026-09-30,
            // `controls.md` §4.171.8): the slot 4 (vtable `+0x10`) of every accessor-bearing class in the
            // shipped layouts is 0x4D1B40, the same `xor al, al; ret 4` — `NFS5.AccessorWatch`,
            // `WORDWRAPACCESSORWATCH`, `CARSETTINGBUTTON`, `CarSettingSlider`, `GouraudIndicatorBar`,
            // `ListBox`, `CarLogo`, `3DCAR`, `CHATDISPLAYBOX`, `CHRONICLESLIDER` — except the four that
            // take **characters**: `NFS5.INPUTBOX` / `CHARVECTACCESSORINPUTBOX` (0x4CCC20, 0x4C2120,
            // 0x525A90), `CHATINPUTBOX` (0x4C3310: Enter, else 0x4CCC20) and `CONTROLLERSETUP`
            // (0x51C610: the pressed key becomes the binding). None of them reads a Left / Right as a
            // step. So the two arrows do nothing on a layout screen; the accessors move through the
            // widgets' own mouse slots (`combo_strip_release`, the drop-down, the list's rows, the sliders'
            // drags) and the front end's key slot 4 0x4D2970 knows ESC, `b` and `=` alone.
            MenuKey::Left | MenuKey::Right => {}
            MenuKey::Enter => {
                let c = controls[sel].clone();
                // the network screens' own callbacks. What the original's handlers behind
                // `ID_COMMSSERVERSCREEN` and friends do has not been read; these do what the
                // protocol of `network.md` §7 allows and nothing more.
                // (`READY` and `EJECTPLAYER` belong to the lobby screens, where the browsing lobby
                // is gone and the session is what is open)
                // `accountlogin.lay`'s three (`FEMainScreen`, registered 0x510034 / 0x50C5B1 / 0x510045)
                if let Some(cb) = c.callback.as_deref()
                    && matches!(cb, "SIGNON" | "CLEARACCOUNTINFO" | "REGISTERUSER")
                    && self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("accountlogin"))
                {
                    self.account_callback(cb, event_loop);
                    return;
                }
                if (self.net.lobby.is_some() || self.net.session.is_some())
                    && let Some(cb) = c.callback.as_deref()
                    && matches!(
                        cb,
                        "CREATE" | "JOIN" | "CONNECT" | "DIAL" | "ENTER" | "EJECTPLAYER" | "READY"
                    )
                {
                    // a button the screen's tick has disabled (0x50D880) takes no press
                    if !l.widget_disabled(c.index) {
                        self.network_callback(cb, event_loop);
                    }
                    return;
                }
                // `parts.lay`'s and `advancedrepair.lay`'s five each, unless the button is off
                if let Some(cb) = c.callback.as_deref()
                    && parts_shop::CALLBACKS.contains(&cb)
                    && self.on_parts_shop()
                {
                    if !l.widget_disabled(c.index) {
                        self.parts_shop_callback(cb);
                    }
                    return;
                }
                // `dealership.lay`'s three (`ID_DEALERCBL`, 0x50EC10): `DEALERCARDEC` / `INC` run
                // `DealerCar.Car`'s slots 0x50 / 0x54 (0x50ED20 / 0x50ED50) — one row back or on,
                // wrapping — and `BUYCAR` is **0x501CF0**: dialog 802 when the price (0x4EE220) is
                // more than the money, 61 when the garage holds 80, else a new FECar from the
                // accessor's slots +0x74…+0x88 (exterior, interior, stripes, package 0, decal −1)
                // with the price at `+0x6A4`, the money going with it
                // `repair.lay`: `GOTOQUICKFIX` (0x50F080) clears the ten `QUICKFIX.*` ticks and goes on
                // to its destination; `QUICKFIX` (0x50F110) sums the ticked categories' costs
                // (0x4EFBC0…0x4EFF80) and raises one-button 809 (0x329) when that is more than the
                // money (0x5012D0), else repairs each ticked category (0x4F0490…0x4F0830) and clears
                // the ticks (0x50F359…0x50F3D8)
                if c.callback.as_deref() == Some("GOTOQUICKFIX") {
                    self.fe.repair_ticks.clear();
                }
                if c.callback.as_deref() == Some("QUICKFIX")
                    && self.career.is_some()
                {
                    // the slots' order, +0x284…+0x2A8 (0x50EEC0…0x50F03E)
                    let ticked: Vec<(&str, &'static [u16])> = QUICKFIX_NAMES
                        .iter()
                        .filter(|n| self.fe.repair_ticks.contains(**n))
                        .filter_map(|n| Some((*n, quickfix_kinds(n)?)))
                        .collect();
                    let short = self.career.as_ref().is_some_and(|cu| {
                        let sel = cu.car_sel.min(cu.career.garage.len().saturating_sub(1));
                        let total: i64 = ticked.iter().map(|(_, k)| cu.career.repair_cost(&cu.tables, sel, k)).sum();
                        cu.career.money < total
                    });
                    if short {
                        self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 809, [0, 0], DialogFor::Notice);
                    } else if let Some(cu) = self.career.as_mut() {
                        let sel = cu.car_sel.min(cu.career.garage.len().saturating_sub(1));
                        // each box's fix function (0x50F27D…0x50F354): COSMETIC is 0x4F0750 (the ten
                        // zones and the body part), OVERALL 0x4F0830, the rest their categories
                        for (name, kinds) in &ticked {
                            let fix = match *name {
                                "COSMETIC" => nfs_game::career::QuickFix::Body,
                                "OVERALL" => nfs_game::career::QuickFix::Overall,
                                _ => nfs_game::career::QuickFix::Kinds(kinds),
                            };
                            let _ = cu.career.repair(&cu.tables, sel, fix);
                        }
                        self.fe.repair_ticks.clear();
                    }
                    self.bind_checks();
                    self.bind_indicator_bars();
                    return;
                }
                // `carsetup.lay`'s three (0x4F1A30 / 0x4F1A50 / 0x4F1A70) on the garage car:
                // `SAVESETTINGS` copies the setup over the backup (0x4EF4E0, after 0x4F14B0 has
                // taken the sliders, which the port writes through as they move), `LOADSETTINGS`
                // the backup over the setup (0x4EF4C0) and `DEFAULTSETTINGS` puts the
                // constructor's defaults back (0x4EF500); the last two re-read the sliders
                // (0x4F1080)
                if let Some(cb @ ("SAVESETTINGS" | "LOADSETTINGS" | "DEFAULTSETTINGS")) = c.callback.as_deref()
                    && let Some(cu) = self.career.as_mut()
                {
                    let sel = cu.car_sel.min(cu.career.garage.len().saturating_sub(1));
                    if let Some(o) = cu.career.garage.get_mut(sel) {
                        match cb {
                            "SAVESETTINGS" => o.setup_backup = o.setup,
                            "LOADSETTINGS" => o.setup = o.setup_backup,
                            _ => o.setup = nfs_game::career::CAR_SETUP_DEFAULT,
                        }
                    }
                    self.bind_indicator_bars();
                    return;
                }
                // `locationsetup.lay`'s `MAKERANDOMKNOCKOUT` (0x50D570)
                if c.callback.as_deref() == Some("MAKERANDOMKNOCKOUT")
                    && let Some(m) = self.fe.menu.as_mut()
                {
                    m.fit_ko();
                    // CRT `rand`'s state is the process's (seed 1 until the first race's `srand(time)`,
                    // 0x418100); the port seeds from the clock — the same unobservability, the draws are the original's
                    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u32).unwrap_or(1);
                    m.randomize_knockout(&mut nfs_game::movie_overlay::CRand::new(seed));
                    self.bind_lists();
                    return;
                }
                // `singleplayer.lay`'s three game types reset the track rows and the car row
                // (0x510C30 / 0x510CA0 / 0x510D10) and then go on to their `destination`
                if matches!(c.callback.as_deref(), Some("SETTYPEQUICKRACE" | "SETTYPEKNOCKOUT" | "SETTYPEQUICKKNOCKOUT")) {
                    let default_colour = default_paint_colour(&self.fe_car_catalog());
                    if let Some(m) = self.fe.menu.as_mut() {
                        m.reset_track_rows(default_colour);
                    }
                }
                // each `SETTYPE*` first writes `Game.GameType` through the accessor's slot 0x2C —
                // `FEMainScreen`'s slots 0xEC / 0xF0 / 0xF4 / 0xF8 / 0xFC / 0x100 (vtable 0x5B9D64)
                // are 0x510C30 / 0x510D10 / 0x510CA0 / 0x510B10 / 0x510B50 / 0x510BC0; what the race
                // leaves the front end on keys on it (`after_race_screens`). The career and factory
                // pair then navigate themselves (their `GameSetup.lay` buttons have no
                // `destination`) — below.
                if let Some(t) = match c.callback.as_deref() {
                    Some("SETTYPEQUICKRACE") => Some(0),
                    Some("SETTYPEQUICKKNOCKOUT") => Some(1),
                    Some("SETTYPEKNOCKOUT") => Some(2),
                    Some("SETTYPEMULTIPLAYER") => Some(3),
                    Some("SETTYPECAREERMODE") => Some(4),
                    Some("SETTYPEFACTORYDRIVER") => Some(9),
                    _ => None,
                } {
                    self.fe.game_type = t;
                    // 0x4F3E40's head (`car_lists_head`, spelled out for the borrow): both lots back to row 0
                    App::car_lists_head_of(&mut self.fe.dealer, &mut self.fe.usedcar_sel);
                    // every one of the six then runs 0x4F3E40, whose 0x4FBC10 (0x4F3EA8) rebuilds
                    // the track lists for the new type (0x510B3D, 0x510B7D, 0x510BEE, 0x510C5D,
                    // 0x510CCD, 0x510D3D) — `rebuild_track_lists`, spelled out for the borrow. Same
                    // 0x4F3E40 call also rebuilds `GenericCar.Car`'s own `[0x6919EC]` list on its
                    // new `+0x204` dealer-availability admit rule ([`dealer_flag_admitted`]) — this
                    // spelled-out block had not been carrying that along (`cars2` past it too, a
                    // pre-existing, separate gap left as found: only `set_track_lists` was called
                    // here, not `set_cars2`, so `GenericCar2.Car`'s own list still lags a `SETTYPE*`
                    // that does not also change the chronicle year)
                    let year = self.career.as_ref().map_or(nfs_game::track_list::START_YEAR, |cu| cu.career.year as i32);
                    let catalog = self.career.as_ref().map(|cu| cu.tables.catalog.clone()).unwrap_or_else(|| self.fe_car_catalog());
                    if let Some(m) = self.fe.menu.as_mut().or(self.fe.menu_template.as_mut()) {
                        m.set_track_lists(&self.fe.track_tables, year, t);
                        m.set_cars(&catalog, t);
                    }
                }
                // `SETTYPECAREERMODE` 0x510B50, after the type and 0x4F3E40: `0x5015B0` — the
                // profile's `[0x691DD5]`, the Evolution intro seen — opens `tournament` through
                // `0x4D2100("tournament", 0, 1)` (0x510B8B), else `0x4D2100("careerfirst", 0, 0)`
                // (0x510BA0) and sets the byte (`jmp 0x5015C0`). `SETTYPEFACTORYDRIVER` 0x510BC0:
                // `0x51B020` — every mission done (the career's `current_mission` none) — pushes
                // `factdone` (`0x4D2100("factdone", 1, 1)`, 0x510C04), else replaces with
                // `factorydriver` (`("factorydriver", 0, 1)`, 0x510C14). The third argument is
                // 0x4D5DF0's "add to the history" (0x4D5E48), so `careerfirst` is not recorded.
                // (0x4F3E40's own lists — the dealer's, the used lot's — are read from the career per use, not stored)
                if c.callback.as_deref() == Some("SETTYPECAREERMODE") {
                    let seen = self.career.as_ref().is_some_and(|cu| cu.career.seen_evolution_intro);
                    if seen {
                        self.fe_open("tournament", 0, true);
                    } else {
                        self.fe_open("careerfirst", 0, false);
                        if let Some(cu) = self.career.as_mut() {
                            cu.career.seen_evolution_intro = true;
                        }
                    }
                    return;
                }
                if c.callback.as_deref() == Some("SETTYPEFACTORYDRIVER") {
                    let done = self.career.as_ref().is_some_and(|cu| cu.career.current_mission(&cu.tables).is_none());
                    if done {
                        self.fe_open("factdone", 1, true);
                    } else {
                        self.fe_open("factorydriver", 0, true);
                    }
                    return;
                }
                // `protocol.lay`'s four (`FEMainScreen`, registered 0x50FFDF…0x510012): each writes
                // `Comms.GameMode` through slot 0x2C — `SETMODENET` 0 (0x510D80), `SETMODELAN` 1
                // (0x510E00), `SETMODEMODEM` 2 (0x510E30), `SETMODEIP` 3 (0x510E60) — and the
                // button's destination follows. The rebuild after a network race reads it
                // (`after_race_screens`). `SETMODENET` also raises its one-button dialog 78
                // "Checking for updates..." (0x512F70 into `[+0x29C]`, `[+0x296]` = 1) while EA's
                // update thread 0x4A56E0 → 0x4A5750 runs (`update_check_tick`); not carried: the
                // profile save's `Comms.GameMode` dword, which the port's save does not write.
                if let Some(mode) = match c.callback.as_deref() {
                    Some("SETMODENET") => Some(0),
                    Some("SETMODELAN") => Some(1),
                    Some("SETMODEMODEM") => Some(2),
                    Some("SETMODEIP") => Some(3),
                    _ => None,
                } {
                    self.fe.comms_game_mode = mode;
                    if mode == 0 {
                        // (`update_check_tick`; set by fields: `l` borrows the screen)
                        let ticks = std::env::var("NFS_UPDATE_CHECK_TICKS").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(netlobby::UPDATE_THREAD_TICKS);
                        if self.fe.update_check.is_none() {
                            self.fe.update_check = Some((ticks, true));
                        }
                        if !self.fe.dialog.as_ref().is_some_and(|(d, p)| *p == DialogFor::UpdateCheck && !d.gone()) {
                            let text = self.fe.strings.get(78).unwrap_or_default().to_string();
                            let mut d = nfs_game::dialog::Dialog::new(nfs_game::dialog::DialogKind::One, text, [0, 0]);
                            d.raise(l.ticks());
                            self.fe.dialog = Some((d, DialogFor::UpdateCheck));
                        }
                    }
                }
                // the location screens' arrows (registered at 0x50C5C7…0x50C63E): each "…INC" is its
                // accessor's slot 0x54, one row on, and each "…DEC" slot 0x50, one row back, both
                // wrapping (0x4BC690 / 0x4BC660); they also set `[0x690A84]` to 0 / 1, the turn
                // direction of the track room's wall (0x4EA9B0, `frontend-3d.md`), which the port's
                // static `trackwall` room does not turn
                // `trackrecords.lay`'s `RESETRECORDS` (0x4FB500): the shipped table over the save,
                // read back
                if c.callback.as_deref() == Some("RESETRECORDS") {
                    self.records = Records::reset(&self.paths, &self.records_path);
                    self.bind_lists();
                    return;
                }
                // `FactoryVictory.lay`'s `ACCEPTPRIZECAR` (0x51A6D0): the pending prize car
                // (0x51B050) joins the garage through AddOwnedCar 0x502270 — no sort — the pending
                // one is cleared (0x51AF80: `+0xC = −1`), and 0x4D23F0(0) goes back
                if c.callback.as_deref() == Some("ACCEPTPRIZECAR") {
                    if let Some(cu) = self.career.as_mut() {
                        if let Some(car) = cu.career.accept_promotion(&cu.tables) {
                            tracing::info!(car, "factory prize car accepted");
                        }
                    }
                    self.fe_back(0, event_loop);
                    return;
                }
                // The plain "back" callbacks. `DECLINE` (0x506CB0), `GOBACKNOVALIDATION` (0x506CB0)
                // and `FACTORYDRIVERDONECONFIRM` (0x50CBF0) are 0x4D23F0(0) — the top layer's back,
                // 0x4D5FB0. `GOBACKMINUSONE` (name pushed at 0x50C23E, handler 0x50E5D0 stored at
                // 0x50C260) is `jmp [vt+0xB8]`, and slot 0xB8 is **0x50CC00** in all 31 screen
                // callback-list vtables — 0x4D23F0(−1), as `FDMSGGOBACK`: the top layer's history
                // emptied and the layer closed (0x4D24B0…0x4D2539, sound 4). *(Corrected
                // 2026-09-27: this said slot 0xB4 — 0x50E5A0 is `GOBACK`'s handler, stored at
                // 0x50C229 — so `missionbrief` reached from `factwelcome`, and `missionmap` from
                // `missionbrief`, went back one screen instead of closing the layer.)*
                // These buttons have no `destination`, and on a screen that may start a race they
                // used to fall through to it.
                match c.callback.as_deref() {
                    Some("DECLINE" | "GOBACKNOVALIDATION" | "FACTORYDRIVERDONECONFIRM") => return self.fe_back(0, event_loop),
                    Some("GOBACKMINUSONE" | "FDMSGGOBACK") => return self.fe_back(-1, event_loop),
                    _ => {}
                }
                // `OPTIONS` (0x50E5C0 → slot 54, 0x50D310) is the bottom bar's; no shipped screen
                // carries it, but it resolves on every controller, so a button that did would
                // open `options` too rather than fall into the race start below
                if c.callback.as_deref() == Some("OPTIONS") {
                    self.fe_options();
                    return;
                }
                // the main menu's (`GameSetup.lay`, `ID_MAINSCREEN`) `LOADGAMEBUTTON`, registered
                // at 0x510023 with the direct handler 0x510AF0: `SaveInfo(name [0x691DC8], picture
                // [0x5DB748])` (0x4DEDA0), which writes nothing for the empty name (0x4DEDB9…
                // 0x4DEDF4). Its `destination=selectname` (offset 1) then goes on below. §4.161.
                if c.callback.as_deref() == Some("LOADGAMEBUTTON")
                    && let Some(cu) = self.career.as_mut()
                    && !cu.profile.name.is_empty()
                {
                    cu.save();
                }
                // `teamprofile.lay`'s arrows (0x519A10 / 0x519A40): `Factory.ProfileNum` one row on
                // or back of its six, wrapping
                if let Some(d) = match c.callback.as_deref() {
                    Some("PROFILEINC") => Some(1),
                    Some("PROFILEDEC") => Some(-1),
                    _ => None,
                } {
                    self.fe.factory_profile = ((self.fe.factory_profile as i32 + d).rem_euclid(6)) as usize;
                    self.bind_player_pictures();
                    return;
                }
                let arrow = match c.callback.as_deref() {
                    Some("TRACK1INC") => Some(("Track1.Track", 1)),
                    Some("TRACK1DEC") => Some(("Track1.Track", -1)),
                    Some("KNOCKOUTTRACKINC") => Some(("Knockout.Track", 1)),
                    Some("KNOCKOUTTRACKDEC") => Some(("Knockout.Track", -1)),
                    Some("TOURNREMAININC") => Some(("Tournament.RemainingTracks", 1)),
                    Some("TOURNREMAINDEC") => Some(("Tournament.RemainingTracks", -1)),
                    Some("LOOPEDINC") => Some(("LoopedTrack.Track", 1)),
                    Some("LOOPEDDEC") => Some(("LoopedTrack.Track", -1)),
                    _ => None,
                };
                if let Some((acc, delta)) = arrow {
                    self.fe.track_turn_back = delta < 0;
                    self.set_accessor(acc, delta);
                    // was a hand-picked `bind_lists`+`bind_car_logos` pair; every other
                    // `set_accessor` site runs the full `bind_after_accessor()` (the
                    // audit of the `NFS5.CARSETTINGBUTTON` gap)
                    self.bind_after_accessor();
                    return;
                }
                // the garage's `SELLCAR` (0x50FCB0): fewer than two cars left once the listed ones
                // are taken off (0x501AC0 less 0x4F93C0) raises one-button 60; else the price
                // dialog 800. `DONTSELLCAR` (0x50FD30) takes the selected car off the market
                // (0x4F9340: ask, listed, sold, offer and its amount all cleared)
                if self.career.is_some() && matches!(c.callback.as_deref(), Some("SELLCAR" | "DONTSELLCAR")) {
                    if c.callback.as_deref() == Some("DONTSELLCAR") {
                        if let Some(cu) = self.career.as_mut() {
                            let sel = cu.car_sel.min(cu.career.garage.len().saturating_sub(1));
                            cu.career.withdraw_from_sale(sel);
                        }
                        self.bind_lists();
                        return;
                    }
                    // 0x50FCB5: the count is `OwnedCars.Car`'s list 0x501AC0 (`owned_list`), the
                    // listings 0x4F93C0 all of them, compared signed (`sub esi, ecx / cmp esi, 2`)
                    let owned = self.owned_list().len() as i64;
                    let can = self.career.as_ref().is_some_and(|cu| {
                        let listed = cu.career.garage.iter().filter(|o| o.asking_price.is_some()).count() as i64;
                        owned - listed >= 2
                    });
                    if can {
                        let now = self.fe.layout_preview.as_ref().map(|l| l.ticks()).unwrap_or(0);
                        let text = self.fe.strings.get(800).unwrap_or_default().to_string();
                        let mut d = nfs_game::dialog::Dialog::input(text, true, 8);
                        d.raise(now);
                        self.fe.dialog = Some((d, DialogFor::SellPrice));
                    } else {
                        self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 60, [702, 0], DialogFor::Notice);
                    }
                    return;
                }
                // the paint shop's `CHANGEDECAL` (0x5111B0): the input dialog 251, digits only, two
                // of them (0x514240 with 1, 2), raised over the screen
                if c.callback.as_deref() == Some("CHANGEDECAL") && self.career.is_some() {
                    let now = self.fe.layout_preview.as_ref().map(|l| l.ticks()).unwrap_or(0);
                    let text = self.fe.strings.get(251).unwrap_or_default().to_string();
                    let mut d = nfs_game::dialog::Dialog::input(text, true, 2);
                    d.raise(now);
                    self.fe.dialog = Some((d, DialogFor::Decal));
                    return;
                }
                // the two showcase callbacks set the accessor the next screen's left car reads
                // (0x50E6A0) and let the button's `destination` go on
                match c.callback.as_deref() {
                    Some("SHOWCASE.DEALERCAR") => self.fe.compare_dealer = true,
                    Some("CARCOMPARE.MERGEDCAR") => self.fe.compare_dealer = false,
                    _ => {}
                }
                let on_dealer = self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("dealership")) && self.career.is_some();
                if on_dealer
                    && let Some(cb @ ("DEALERCARDEC" | "DEALERCARINC")) = c.callback.as_deref()
                {
                    self.set_accessor("DealerCar.Car", if cb == "DEALERCARDEC" { -1 } else { 1 });
                    // was a hand-picked `bind_car_logos`+`bind_indicator_bars` pair; same fix as
                    // the track arrows above and `NFS5.CARSETTINGBUTTON`
                    self.bind_after_accessor();
                    return;
                }
                if on_dealer && c.callback.as_deref() == Some("BUYCAR") {
                    let d = self.fe.dealer;
                    let pick = self.dealer_car().map(|e| e.id);
                    if let (Some(id), Some(cu)) = (pick, self.career.as_mut()) {
                        let trim = nfs_game::career::Trim { interior: d.interior as u8, ..Default::default() };
                        match cu.career.buy_car(&cu.tables, id, d.paint as u8, trim) {
                            Ok(()) => {
                                // `buy_car` sorted the garage (0x4F0EE0); sound 0xB (0x501ED8)
                                self.fe_sound(0xB);
                                self.bind_lists();
                            }
                            Err(nfs_game::career::CareerError::NoMoney { .. }) => {
                                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 802, [0, 0], DialogFor::Notice)
                            }
                            Err(nfs_game::career::CareerError::GarageFull) => {
                                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 61, [0, 0], DialogFor::Notice)
                            }
                            Err(e) => tracing::info!("dealer: {e}"),
                        }
                    }
                    return;
                }
                // `usedcars.lay`'s `ID_BUYCARBUTTON` — `callback=BUYCAR`, which for the used-car
                // lot is 0x501F40: the money goes and the car moves across as it stands, mileage,
                // parts and all. `UsedCar.Car` lists only the generated cars (0x4F9010), so the
                // selection is an index into `Career::market`.
                if c.callback.as_deref() == Some("BUYCAR")
                    && self.fe.layout_name.as_deref().map(|n| n.eq_ignore_ascii_case("usedcars")).unwrap_or(false)
                {
                    let n = self.career.as_ref().map_or(0, |c| c.career.market.len());
                    let idx = self.fe.usedcar_sel;
                    if idx < n
                        && let Some(cu) = self.career.as_mut()
                    {
                        match cu.career.buy_used_car(&cu.tables, idx) {
                            Ok(()) => {
                                // 0x502161 sorts the garage (in `buy_used_car`), then sound 0xB (0x502166)
                                self.fe_sound(0xB);
                                self.fe.usedcar_sel = 0;
                                self.bind_lists();
                                self.bind_indicator_bars();
                                self.bind_usedcars_panels();
                            }
                            // 0x501F86 / 0x501FD8: one-button 802 (money) or 61 (eighty cars)
                            Err(nfs_game::career::CareerError::NoMoney { .. }) => {
                                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 802, [0, 0], DialogFor::Notice)
                            }
                            Err(nfs_game::career::CareerError::GarageFull) => {
                                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 61, [0, 0], DialogFor::Notice)
                            }
                            Err(e) => tracing::info!("used car: {e}"),
                        }
                    }
                    return;
                }
                // The bottom bar's PLAYGAME button, or any screen that says it can start a game.
                //
                // **The gate is this port's, and it is an approximation — §4.85.** `PLAYGAME`
                // resolves through the controller's callback table to **slot 53**, 0x50CE90
                // (0x50C1F2 stores 0x50E5B0, which is `jmp [vtbl+0xD4]`), and that handler asks
                // nothing: it opens with its own re-entry latch `[this+0x278]` and goes straight
                // into the network-session checks. `canplaygame`'s getter, slot 36 at 0x4E0BF0,
                // has no direct callers at all — in the original the **bottom bar** shows and
                // hides the button per screen, so on a screen that cannot race there is no
                // button to press. This port keeps the button and declines instead, which is the
                // same outcome by a different route, and it is what stops the RACE button
                // starting a race from `credits.lay` and `createplayer.lay`. The thirteen screens
                // with a non-zero `canplaygame` are exactly the race-launching ones.
                //
                // (An earlier reading paired `PLAYGAME` with 0x50E540 and hung this on slot 47's
                // "ask then act". That registration is **`QUIT`**, and its `[this+0x270]` is the
                // exit-confirm dialog, `FEStrings` 811.)
                // `FORFEITTOURNY` (0x506130) raises 824; it has no destination, so without this
                // it fell into the start below and raced instead of forfeiting
                if c.callback.as_deref() == Some("FORFEITTOURNY") {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 824, [700, 701], DialogFor::Forfeit);
                    return;
                }
                // `KILLREPLAY` (0x50D3A0) raises the prebuilt 808 with the default ok / cancel
                // (0x50C690 passes 0, 0); `STARTREPLAY` is not ported — both used to fall into the
                // race start below
                if c.callback.as_deref() == Some("KILLREPLAY") {
                    if !self.replay_names().is_empty() {
                        self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 808, [0, 0], DialogFor::KillReplay);
                    }
                    return;
                }
                let can_play = l.can_play();
                if let Some(cb) = c.callback.clone()
                    && (self.player_callback(&cb, event_loop) || self.cd_callback(&cb))
                {
                    return;
                }
                if c.callback.as_deref() == Some("STARTREPLAY") {
                    self.start_replay(event_loop);
                    return;
                }
                // `DEFAULTCONTROLS` 0x511F50: 0x51D1D0 on the device `Controller.AllControllers`
                // is on (its slot 0x28, mapped to a device by 0x51F230) — `nfs_input::bindings`
                if c.callback.as_deref() == Some("DEFAULTCONTROLS") {
                    let pad = (!self.gamepads.devices().is_empty()).then_some(self.fe.controllers_sel);
                    self.input.default_controls(pad, |n| accessors::controller_action(n).and_then(nfs_input::binding_id));
                    return;
                }
                // 0x50D330 / 0x50D350 / 0x50D370 arm the `Game.GameType` override `[0x5DC1C0]`
                // with 7 / 6 / 8 and run PLAYGAME
                if let Some(t) = match c.callback.as_deref() {
                    Some("TESTDRIVEUSED") => Some(7),
                    Some("TESTDRIVENEW") => Some(6),
                    Some("TESTDRIVECARSETUP") => Some(8),
                    // `TESTDRIVE` by screen: `VictoryRoom.lay`'s 0x506C70 sets `Game.GameType` 11
                    // and starts; `prerace.lay`'s 0x5060D0 sets 5 when the garage holds a car
                    // (0x501AC0) and starts, putting 4 back when the start fails (0x506122). The
                    // dealership's goes through its `destination` (vtable +0xE0 = 0x516950).
                    Some("TESTDRIVE") => match self.fe.layout_name.as_deref() {
                        Some(n) if n.eq_ignore_ascii_case("VictoryRoom") => Some(11),
                        Some(n) if n.eq_ignore_ascii_case("prerace") => self.career.as_ref().filter(|c| !c.career.garage.is_empty()).map(|_| 5),
                        _ => None,
                    },
                    _ => None,
                } {
                    self.start_test_drive(t, event_loop);
                    return;
                }
                // `carsetup.lay`'s `TESTDRIVECHECK` (0x4F1A90): unless 0x502420 says no
                // (`fe_carsetup_blocked`), 0x4F14B0, then `0x4D2100("testdrivecarsetup", 1, 1)` —
                // pushed over the setup screen. 0x4F14B0 is not read.
                if c.callback.as_deref() == Some("TESTDRIVECHECK") {
                    if !self.fe_carsetup_blocked() {
                        self.fe_open("testdrivecarsetup", 1, true);
                    }
                    return;
                }
                // `GOBACK` — the callback-list's slot 0xB4, the same handler `fe_bottom_click`
                // runs for the bottom bar's own `GOBACK` (`fe_back_callback`, with its per-screen
                // overrides for `ID_VICTORYSCREEN`/`ID_FDVICTORYSCREEN` (no back at all),
                // `ID_CARSETUP` (gated) and the comms screens (session torn down first)). Read
                // fresh (`dasm.py 0x4c9470 +0x70`): `NFS5.GOTOBUTTON`'s release (0x4C9470) runs
                // `data.callback` through 0x50D6D0 **whenever the callback string is non-empty**,
                // and *separately* — not an `else` — 0x4D2100(`data.destination`, offset, 1)
                // **whenever the destination string is non-empty**; each is skipped independently
                // when its own string is empty. Fourteen shipped `NFS5.GOTOBUTTON`s carry
                // `callback=GOBACK` with an **empty** `destination` — `accountlogin.lay`,
                // `bottombar.lay` (already the bar's own path), `levelvictory.lay` (paired with
                // `destination=levelbadge` — see below), `loadreplay.lay`, `missionmap.lay`,
                // `partsdesc.lay`, `protocol.lay`, `testdriveoval.lay`, `repair.lay`,
                // `teamprofile.lay`, `settings.lay`, `singleplayer.lay`, `testdrivecarsetup.lay`,
                // `testdrivesquare.lay` — and the port's click handler used to fall straight
                // through to `let Some(dest) = c.destination else { return }` and do nothing:
                // every one of these screens' own "готово"/"назад" button was a dead click
                // (USER REPORT 2026-09-30(e) — QuickRace's `settings`, `protocol`'s network mode
                // picker, and the rest of this list). This does not `return`: `levelvictory.lay`'s
                // one `GOBACK` button also carries `destination=levelbadge`, and the original runs
                // both — here the override makes `GOBACK` itself a no-op, so only the destination
                // open below actually shows anything, matching the original exactly.
                if c.callback.as_deref() == Some("GOBACK") {
                    self.fe_back_callback(event_loop);
                }
                if (c.callback.as_deref() == Some("PLAYGAME") || c.destination.is_none()) && can_play {
                    // `Game.GameType` 4 races the selected tournament's event (0x4D5730, 0x4ED5D0)
                    if self.fe.game_type == 4 && self.career.is_some() {
                        self.start_tournament_from_layout(event_loop);
                        return;
                    }
                    self.fe_start_game(event_loop);
                    return;
                }
                let Some(dest) = c.destination else { return };
                // **`data.offset` is what the button does to the screen stack.** Its slot 9 hands
                // the attribute's int to `0x4D2100(destination, offset, 1)` (0x4C94BD…0x4C94CF),
                // and that function branches on it three ways over the vector at `[mgr+0xD8]`:
                //
                // * **> 0** — a new **layer** is made and appended (0x4D21A5…0x4D2229), so the
                //   screen it was clicked on stays underneath, covered;
                // * **0** — the top layer's screen becomes `destination`
                //   (0x4D235F → `0x4D5DF0(end[-1], …)`), and the depth does not change;
                // * **< 0** — only when the vector holds **at least two** layers (0x4D22DD), the
                //   top one is closed through its slot 31 (0x4D22FC) and the one under it takes
                //   `destination` (`0x4D5DF0(end[-2], …)`); with one layer it is the 0 arm.
                //
                // Every arm passes the button's 1, so 0x4D5DF0 records `destination` in that
                // layer's history (0x4D5E48), which the back 0x4D5FB0 walks (§4.166,
                // `nfs_game::layout::nav`). Of the 153 shipped `NFS5.GOTOBUTTON`s, 109 are the
                // 0 arm (the key absent or 0), 25 push a layer and 19 close one.
                self.fe_open(&dest, c.offset, true);
                // `selectname.lay`'s `CREATEPLAYERENTER` (0x510780, `FEMainScreen` slot 0xE4) runs
                // before this `destination=createplayer`: the global `UserList.NameInput`'s clear
                // (`vt[0x44]`, 0x4BC9C0) and `[0x5D8F2C] = 8` (0x4CF790), so the new screen opens
                // with none of its eight pictures (`ournumber` 0…7) chosen. This port's name buffer
                // belongs to the box on the opened screen, so the clear lands there. §4.161, §4.162.
                if c.callback.as_deref() == Some("CREATEPLAYERENTER") {
                    self.fe.driver_picture = nfs_game::save::DEFAULT_PICTURE as i32;
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.clear_input("UserList.NameInput");
                        l.driver_picture = self.fe.driver_picture;
                    }
                }
            }
            // 0x4D23F0(0)
            MenuKey::Back => self.fe_back(0, event_loop),
        }
    }

    /// **0x4D2970**, the front end's own keys once nothing on the screen took them (the key code
    /// is the character, `controls.md` §4.166): `0x1B` (ESC) presses `ID_EXITBUTTON`'s callback,
    /// slot 0xBC — `QUIT`, 0x50CC10: the exit dialog — unless that button is disabled; `0x62`
    /// (`b`) goes back through slot 0xB4 when `ID_BACKBUTTON` is not disabled, the back lock
    /// `[+0x17C]` is clear and 0x4D6320's layer test passes (0x4D29FD…0x4D2A55). Returns whether
    /// the key was taken.
    /// A key the screen's widgets did not take: the callback list's slot 4 first (`careerfirst`'s
    /// 0x506170 and the end-of-era show's 0x500770 take 0x1B — the movie circle's tick 0x4CD410
    /// then sees ESC down and opens `tournament`; the show's tick goes back), then 0x4D2970.
    pub(crate) fn fe_key_code(&mut self, code: u8, event_loop: &ActiveEventLoop) {
        if !self.fe_callback_key(code) {
            self.fe_manager_key(code, event_loop);
        }
    }

    /// The callback list's slot 4 — `[fe+0xE0]`, which 0x4D2850 hands every event **first**
    /// (0x4D285B…0x4D286B), before any transition test. Whether it took the key.
    pub(crate) fn fe_callback_key(&mut self, code: u8) -> bool {
        if code == 0x1B && self.fe.layout_name.as_deref() == Some("careerfirst") {
            self.fe_open("tournament", 0, true);
            true
        } else {
            // taken; the show's tick leaves (`end_of_era_leave`)
            code == 0x1B && self.fe.layout_preview.as_ref().is_some_and(|l| l.end_of_era_key(0x1B))
        }
    }

    /// **0x4D2850**, `FEApplication`'s slot 3 — the front end's input dispatch, which every event
    /// the pump drains reaches (key 2, button 4 / 8, move 7; §4.40). After the callback list
    /// `[fe+0xE0]` has seen it, it answers 0 — the event is **dropped** — while
    ///
    /// * the background screen `[fe+0xE4]` is in transition (0x4D2877: 0x4E0DE0, state 1 or 3),
    /// * the room `[fe+0xE8]` is busy (0x4D2892: 0x4E7850, `[room+0x264]` — the dealership's
    ///   car slide, 0x4E961F / 0x4E93FF, and the track room's wall turn, 0x4EAB0E / 0x4EAC66),
    /// * or the top layer's screen `[[fe+0xD8].back()]` is in transition (0x4D28CA, 0x4E0DE0).
    ///
    /// Only then does a dialog take it (0x512170 → 0x512120), then the bottom bar `[fe+0xDC]`
    /// (unless the callback list took it), the top layer's screen, the background and the
    /// manager's own keys 0x525F80 → slot 4 0x4D2970. So keys, clicks and the widgets' hover all
    /// stop while a screen slides; the pointer itself does not — the cursor draw reads the
    /// device (0x4D32DB). `RE/notes/controls.md` §4.171.
    pub(crate) fn fe_input_swallowed(&self) -> bool {
        let Some(l) = self.fe.layout_preview.as_ref() else { return false };
        let room_busy = self.fe.room_obj.is_some()
            && (self.fe.dealer_row.is_some_and(|r| r.busy) || self.fe.track_room.as_ref().is_some_and(|t| t.busy));
        l.in_transition() || room_busy
    }

    fn fe_manager_key(&mut self, code: u8, event_loop: &ActiveEventLoop) -> bool {
        let button_enabled = |b: &Option<nfs_game::layout::LayoutScreen>, name: &str| {
            b.as_ref().and_then(|b| b.widgets_named(&[name]).first().map(|(i, _)| !b.widget_disabled(*i))).unwrap_or(false)
        };
        match code {
            0x1B => {
                if !button_enabled(&self.fe.layout_bottom, "ID_EXITBUTTON") {
                    return false;
                }
                // `carsetup`'s slot 0xBC 0x4F1D00 is gated on 0x502420 like its back
                if self.fe.dialog.is_none() && !self.fe_carsetup_blocked() {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 811, [0, 0], DialogFor::Quit);
                }
                true
            }
            0x62 => {
                if !button_enabled(&self.fe.layout_bottom, "ID_BACKBUTTON") || !self.fe.nav.back_enabled() {
                    return false;
                }
                self.fe_back_callback(event_loop);
                true
            }
            _ => false,
        }
    }

    /// The player screens' enables from the tick 0x510210, through the by-name 0x50D880:
    /// `ID_CREATEPLAYEROKBUTTON` while the name box holds something (0x51034D) and
    /// `ID_REMOVEBUTTON` while the list is not empty (0x51036E: the accessor's slot 0x6C,
    /// 0x4B8B50, is `begin == end`). `ID_CONTINUEBUTTON`'s is gated on a local the slot-0x6C call
    /// never writes — it returns with a plain `ret` over the argument pushed for it — so what it
    /// sees is left to a running original (AUDIT.md §E) and the button is left as it stands.
    /// One printable key into input box `i`: the key as the byte `WM_CHAR` would have delivered
    /// (`nfs_formats::locale::typed_char`), then the box's own filter (0x4CCC20 / 0x4BCA90,
    /// `LayoutScreen::type_char`). Shared by the real keyboard path and `NFS_FE_KEYS=keys:…`.
    pub(crate) fn fe_input_key(&mut self, i: usize, c: char) -> bool {
        let Some(c) = nfs_formats::locale::typed_char(c) else { return false };
        self.fe.layout_preview.as_mut().is_some_and(|l| l.type_char(i, c))
    }

    /// The open combo's own scrollbar drag, one step — `FeListScrollBar`'s mouse-move slot 8,
    /// **0x4DB9B0** (vtable 0x5B6A64, §4.101), not `NFS5.VerticalSlider`'s 0x4D0F20:
    ///
    /// ```text
    /// 0x4DB9C2  0x4DB2F0(ev.x, ev.y)                       ; hit test -> [+0xF0]
    /// 0x4DB9C7  [+0xEC] == 3 (the press latched the knob)  ; else return 0
    /// 0x4DB9D6  [+0xF0] == 3 || [+0xF0] == 4               ; the pointer on the knob or trough
    /// 0x4DBA24  pos = [+0xE8] + (ev.y − [+0xE4]) · count / travel
    /// 0x4DBA29  model->slot11(pos); 0x4DBA30 this->slot11()   ; set, then re-place the knob
    /// ```
    ///
    /// so the step divides by the count (`combo_drag_pos`, the inverse of 0x4DB220's placement)
    /// and **does nothing while the pointer is off the bar or over an arrow** — the drag stays
    /// latched (`[+0xEC]` is cleared only by the release, 0x4DB1B8) and resumes when the pointer
    /// comes back. Called from the real `CursorMoved` handler, from the press on the trough (arm
    /// 4's reposted move, 0x4DB119) and from the `NFS_FE_CLICKS=drag:…` diagnostic below, so a
    /// scripted drag steps the model exactly the way a real one does.
    pub(crate) fn fe_apply_combo_drag(&mut self) {
        // `NFS5.CHRONICLESLIDER`'s knob (0x4D0F20) rides the same move events
        self.chronicle_slider_move();
        if let (Some((index, gy, at)), Some((cx, cy))) = (self.fe.combo_drag, self.fe.cursor) {
            let Some(l) = self.fe.layout_preview.as_ref() else { return };
            if !matches!(l.combo_scroll_at(cx, cy), Some((i, 3 | 4)) if i == index) {
                return;
            }
            if let Some((travel, positions)) = l.combo_scroll_track(index)
                && let Some(pos) = nfs_game::layout::combo_drag_pos(cy, gy, at, positions, travel)
                && let Some(l) = self.fe.layout_preview.as_mut()
            {
                l.set_combo_scroll_pos(index, pos);
            }
        }
    }

    /// The front end's left button, press or release — `window_event`'s own arm, and the
    /// `NFS_FE_CLICKS` screenshot diagnostic (tooling: a scripted click through the same path).
    pub(crate) fn fe_left_button(&mut self, event_loop: &ActiveEventLoop, down: bool) {
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_pressed(down);
        }
        self.fe.mouse_down = down;
        if !down && self.hud_click(event_loop) {
            return;
        }
        // `HReplay` latches the press, event 2 (0x42D8EC), where the dialogs answer the
        // release; the pump answers it once per `RaceFrame` (`App::replay_bar_frame`), not
        // here — moving the down event straight into an answer used to act up to one frame
        // before the original's own pump would (`replay.md` §18.11's gap)
        if down && self.replay_bar_latch() {
            return;
        }
        // 0x4D2850 drops the button, down and up alike, while a screen slides or the room
        // is busy — before the dialog and the bar see it (`fe_input_swallowed`)
        if self.fe_input_swallowed() {
            return;
        }
        // a dialog's input slot 3 (0x512F10) hands the event to its buttons first, and
        // stops at the one that takes it (slot 9, 0x515130)
        if !down && let Some((d, _)) = self.fe.dialog.as_mut() && let Some(s) = d.release() {
            self.fe_sound(s as usize);
            return;
        }
        // the bottom bar is a screen of its own, and its buttons navigate on the release
        if !down && self.fe_bottom_click(event_loop) {
            return;
        }
        if down {
            self.chronicle_slider_press();
        } else {
            self.fe.chronicle_slider_drag = None;
        }
        // `NFS5.ChroniclePicture` drags its picture about: slot 5 takes hold, slot 8
        // pans, slot 9 lets go (§4.119)
        if let Some((cx, cy)) = self.fe.cursor {
            let hit = self
                .fe.layout_preview
                .as_ref()
                .and_then(|l| l.hovered())
                .filter(|i| self.fe.layout_preview.as_ref().and_then(|l| l.class_of(*i)).as_deref() == Some("NFS5.ChroniclePicture"));
            if let Some(l) = self.fe.layout_preview.as_mut() {
                match (down, hit) {
                    (true, Some(i)) => l.picture_press(i, cx, cy),
                    (false, _) => l.picture_release(),
                    _ => {}
                }
            }
        }
        if !down {
            // 0x4C1FB0: the release hands `[this+0x14C]` to 0x5292D0 and zeroes it
            self.fe.setting_repeat = None;
            // 0x4DB180: same shape, for the combo drop-down's own scrollbar arrow — unconditional
            // like `list_repeat`/`setting_repeat` above, so a release with the pointer carried
            // off the arrow entirely (where `combo_scroll_at` below finds nothing) still stops it.
            self.fe.combo_arrow_repeat = None;
        }
        // A scrollbar drag holds the combo's own focus and, with it, the window's capture
        // (§4.57) — every event until the release is the scrollbar's, not whatever the pointer
        // now sits over, so the release ends the drag right here even when it has carried the
        // pointer off the bar entirely (where `combo_scroll_at` below would find nothing).
        if !down && self.fe.combo_drag.take().is_some() {
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_combo_scroll(None);
            }
            return;
        }
        // `FeListScrollBar::onPress` 0x4DB040: the hit test first, then the jump table at
        // 0x4DB164. Arms 1 and 2 step the list and swallow the press — the drop-down must
        // not close under the pointer (§4.101). Arms 3 and 4 (the knob and the track) start a
        // drag instead of stepping once — the same grab/move/release shape §4.46 already ports
        // for the list box's own scrollbar (`list_drag` below), against this bar's own numbers
        // (§4.90's `FeListScrollBar`, not `NFS5.VerticalSlider`). *(User report, 2026-09-30 (b):
        // until this fix the open combo's scrollbar had no drag at all — only the arrows moved
        // it.)*
        if let Some((cx, cy)) = self.fe.cursor
            && let Some((i, region)) = self.fe.layout_preview.as_ref().and_then(|l| l.combo_scroll_at(cx, cy))
        {
            // arms 1 and 2 play click 8 through 0x4AADB0 whether or not the list moves
            // (0x4DB08B, 0x4DB0A1)
            if down && matches!(region, 1 | 2) {
                self.fe_sound(8);
            }
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_combo_scroll(Some((i, region, down)));
                if down {
                    match region {
                        1 | 2 => {
                            l.combo_scroll_press(i, region);
                        }
                        3 | 4 => {
                            self.fe.combo_drag = l.combo_scroll_grab(i, region, cy);
                        }
                        _ => {}
                    }
                }
            }
            // an arrow (1 or 2) auto-repeats every 150 ms while held — `FeListScrollBar::onPress`
            // 0x4DB040 arms `0x529220(this, 0x96, 1)` right after the immediate step above; the
            // unconditional release clear above (`!down`) kills it the way 0x4DB180 does.
            if down && matches!(region, 1 | 2) {
                self.fe.combo_arrow_repeat = Some((i, region, 0));
            }
            // arm 4 reposts a move (0x4DB101 `bl = 1` → 0x4D6140 at 0x4DB119), which the drag
            // slot 0x4DB9B0 takes at once: the list jumps by the pointer's distance from the knob
            if down && region == 4 {
                self.fe_apply_combo_drag();
            }
            return;
        }
        // `NFS5.DEADZONE` takes the button before anything else on its screen: its slot 5
        // (0x51BB60) hit-tests its four handles, takes focus and sets the value, its slot
        // 8 (0x51BCD0) drags while one is held and its slot 9 (0x51BCB0) lets go.
        if let Some((cx, _)) = self.fe.cursor
            && !down
            && let Some(l) = self.fe.layout_preview.as_mut()
        {
            let _ = cx;
            l.deadzone_release();
        }
        if let (Some((cx, cy)), true) = (self.fe.cursor, down)
            && let Some(l) = self.fe.layout_preview.as_mut()
            && l.deadzone_press(cx, cy)
        {
            return;
        }
        // `NFS5.GouraudIndicatorBar`'s slot 5 (0x4CA470) takes the button when the bar is
        // `data.changeable` and the pointer is on it, and sets the accessor straight from
        // where the pointer landed. Its slot 8 (0x4CA590) then drags, gated on the focus
        // the press granted (0x4C7DD0) and on the pointer still being over the part.
        // §4.81.
        if !down {
            self.fe.bar_grab = None;
        }
        if let (Some((cx, cy)), true) = (self.fe.cursor, down)
            && let Some((_, name, f)) = self.fe.layout_preview.as_ref().and_then(|l| l.bar_at(cx, cy))
        {
            self.fe.bar_grab = Some(name.clone());
            self.set_accessor_fraction(&name, f);
            return;
        }
        // The colour editor eats the button while it is up: its slots 5 and 9 return
        // without ever reaching the combo's own (0x4C4730, 0x4C4980 fall through to the
        // base only when the widget has **lost** focus or the latch is down) — §4.66.
        if let (Some(open), Some((cx, cy))) = (self.fe.layout_preview.as_ref().and_then(|l| l.open_picker()), self.fe.cursor) {
            // `[this+0x218]`, the region the release lands in: 3 is ok, 4 cancel (0x4C4993…0x4C49D3)
            let region = self.fe.layout_preview.as_ref().map_or(5, |l| l.picker_region());
            let base = self.picker_accessor();
            let closed = if let Some(l) = self.fe.layout_preview.as_mut() {
                if down {
                    l.picker_press(cx, cy);
                    false
                } else {
                    l.picker_release(cx, cy)
                }
            } else {
                false
            };
            // a press writes the selected well, cancel all eight — both the global's
            self.store_picker_wells();
            if down {
                // …and tells the accessor which well (slot 27, 0x4C480E)
                self.picker_tell_accessor();
            } else if closed && let Some(base) = base {
                // ok hands the accessor back its own row (slot 0x28 into slot 11, 0x4C49AD…0x4C49BF);
                // cancel the row it had when the editor came up, `[this+0x29C]`, after the wells
                // are back (0x4C49E6…0x4C4A11)
                if region == 4
                    && let Some(prev) = self.fe.picker_prev.take()
                {
                    match prev {
                        Ok(row) if base.starts_with("OwnedCar.") && self.garage_car().is_some() => self.colour_select(&base, row),
                        _ => self.colour_assign(&base, prev),
                    }
                } else {
                    let row = self.colour_row_of(&base).unwrap_or(0);
                    self.colour_select(&base, row);
                }
                self.bind_after_accessor();
            }
            if closed {
                // "ok" and "cancel" both play sound 5 (0x4C49C4, 0x4C4A16)
                self.fe_sound(5);
                self.fe.layout_open_combo = None;
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.set_open_combo(None);
                }
            }
            let _ = open;
            return;
        }
        // A release on a row of an open drop-down is the combo commit, `FeComboBox`'s
        // slot 9 0x4C6CDD…0x4C6E2B, for every combo alike (§4.171.7):
        //
        // * inside the selection box (0x4E2970 on `[+0x178]`) the row `[+0x1A8]` — the
        //   last one the move handler 0x4C6F60 found under the pointer — goes to the
        //   accessor's **slot 11** `SetSelection` (0x4C6D82, `combo_select`); the
        //   `data.selectmode == 1` re-select arm (0x4C6D12, slot 16) is a list box's —
        //   `ComboBox.ctr` / `ColorBox.ctr` declare no `selectmode`;
        // * a colour box first asks 0x4B94B0 whether the row is **at** the colour count:
        //   then the editor's latch 0x4C39F0 goes up instead, and slot 28 (0x4C46F0)
        //   keeps the focus (§4.171.1); a saved colour's row carries its RGBA (§4.171.6);
        // * then sound 5, gated at 0x4C6D85…0x4C6DAA on an `IntAttribute`
        //   `data.disablesound` (0x5258C0 → 0x5A134B): `ListBox.ctr` declares it,
        //   `ComboBox.ctr` and `ColorBox.ctr` do not, so every drop-down clicks;
        // * then, not a list box and focused, the unfocus 0x4C7DA0 — the list closes —,
        //   0x4D61A0's synthetic move (type 7) so the hover is found again, the refresh
        //   0x4C53A0 and the stamp `[+0x1D4]`.
        //
        // *(Until 2026-09-27 only a colour list committed; any other drop-down's row
        // closed the list and changed nothing.)*
        if !down
            && let Some((cx, cy)) = self.fe.cursor
            && let Some((i, row)) = self.fe.layout_preview.as_ref().and_then(|l| l.combo_row_at(cx, cy))
            && self.combo_commit(i, row)
        {
            return;
        }
        // the press moves the screen's current widget onto whatever is under the pointer,
        // and the release runs it
        // a list's scrollbar arrows, hit-tested in design coordinates: the original moves
        // the position in the scrollbar object the row loop reads (§4.45)
        if !down {
            if let (Some((i, _, _)), Some(l)) = (self.fe.list_drag, self.fe.layout_preview.as_mut()) {
                l.park_knob(i, None);
            }
            self.fe.list_drag = None;
            self.fe.list_repeat = None;
        }
        // The open combo takes the release before anything else once its rows (above)
        // did not: 0x4C6E2E plays sound **3** (not a list box) and lets go (0x4C7DA0);
        // a release outside the widget's own box (0x4C8670) is then handed on to the
        // root's slot 3 (0x4C6E93…0x4C6EB2) — so it still reaches the widget under the
        // pointer, and a release on another combo opens that one (below).
        let was_open = self.fe.layout_open_combo;
        if !down && was_open.is_some() {
            self.fe_sound(3);
            self.fe.layout_open_combo = None;
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_open_combo(None);
            }
        }
        if down && let (Some(l), Some((cx, cy))) = (self.fe.layout_preview.as_ref(), self.fe.cursor) {
            let mut moved = None;
            let mut grabbed = None;
            let mut parked = None;
            // A click on one of `Parts.AllParts`'s two section headers opens or closes it.
            // `FePartsAllPartsAccessor`'s slot 11, 0x515440, decodes the row (0x5152E0) and
            // for a **header** flips `[this+0x2C]` or `[this+0x2D]` with a `sete`
            // (0x51548B, 0x5154B2) before refreshing. The second header's row is 1 while
            // the first section is closed and `1 + <its rows>` while it is open.
            let mut picked_part = None;
            let section = l
                .list_boxes()
                .into_iter()
                .find(|w| w.accessor.as_deref() == Some("Parts.AllParts"))
                .and_then(|w| l.list_row_at(w.index, cx, cy))
                .and_then(|row| match row {
                    0 => Some(0usize),
                    r if r == self.parts_header_b() => Some(1),
                    // any other row is a part, and clicking it moves the accessor's own
                    // index — which is what 0x4F1ADE reads back (§4.114)
                    r => {
                        picked_part = Some(r);
                        None
                    }
                });
            for w in l.list_boxes() {
                let Some(b) = l.list_scrollbar(w.index) else { continue };
                let inside = |r: (f32, f32, f32, f32)| cx >= r.0 && cx < r.0 + r.2 && cy >= r.1 && cy < r.1 + r.3;
                if inside(b.up) {
                    moved = Some((w.index, -1));
                } else if inside(b.down) {
                    moved = Some((w.index, 1));
                } else if inside(b.knob) {
                    // 0x4D0973: the press remembers where it grabbed and the position it
                    // grabbed at, and the move (0x4D0F20) works from those two
                    grabbed = Some((w.index, cy, b.at));
                } else if cx >= b.up.0 && cx < b.up.0 + b.up.2 && cy >= b.track_top && cy < b.down.1 {
                    // regions 4 and 5, the track either side of the knob (0x4D0995): the
                    // press does **not** move the model — it remembers the knob's own
                    // top-left as the grab (0x4D09D6, 0x4D09E3) and the position it is on
                    // (0x4D09F6), so the bar only moves once the pointer does. It does
                    // move the knob's **drawn** rect to the pointer, though (0x4D0A0A /
                    // 0x4D0A18 → 0x5276E0), and re-runs the hit test so the region becomes
                    // the knob — a look that lasts until the first move.
                    grabbed = Some((w.index, b.knob.1, b.at));
                    parked = Some((w.index, cy));
                }
            }
            if let Some(g) = grabbed {
                self.fe.list_drag = Some(g);
                if let (Some((i, ky)), Some(l)) = (parked, self.fe.layout_preview.as_mut()) {
                    l.park_knob(i, Some(ky));
                }
                return;
            }
            if let Some((index, delta)) = moved
                && let Some(l) = self.fe.layout_preview.as_mut()
            {
                l.scroll_list(index, delta);
                self.fe.list_repeat = Some((index, delta, 0));
                return;
            }
            if let Some(sec) = section {
                self.fe.parts_open[sec] = !self.fe.parts_open[sec];
                // the two headers move the second one's row, so the picked part goes with
                // the list rather than staying on a row that is no longer a part
                self.fe.parts_sel = None;
                self.bind_lists();
                self.bind_checks();
                self.bind_parts_buttons();
                return;
            }
            if let Some(r) = picked_part {
                self.fe.parts_sel = Some(r);
                self.bind_parts_buttons();
                return;
            }
            // a click on `Replay.List` puts its accessor on that row (FeListBox slot 9 →
            // the accessor's slot 11)
            if let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| {
                l.list_boxes().into_iter().find(|w| w.accessor.as_deref() == Some("Replay.List")).and_then(|w| l.list_row_at(w.index, cx, cy))
            }) {
                self.fe.replay_sel = row;
                self.bind_lists();
                return;
            }
            // the CD player's two lists keep their own selected rows. Both carry
            // `data.selectmode = 1`, so a click on the row already selected clears the
            // selection (0x4C6D12…0x4C6D2E: accessor slot 16, 0x4BC610) instead of setting
            // it again
            for (acc, avail) in [("CDPlayer.AvailableList", true), ("CDPlayer.SelectedList", false)] {
                if let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| {
                    l.list_boxes().into_iter().find(|w| w.accessor.as_deref() == Some(acc)).and_then(|w| l.list_row_at(w.index, cx, cy))
                }) {
                    let sel = if avail { &mut self.fe.cd_available } else { &mut self.fe.cd_selected };
                    *sel = if *sel == Some(row) { None } else { Some(row) };
                    self.bind_lists();
                    return;
                }
            }
            // the shop's basket and the repair screen's two lists keep their own current
            // rows (§4.160)
            if let Some((acc, row)) = self.fe.layout_preview.as_ref().and_then(|l| {
                l.list_boxes().into_iter().find_map(|w| Some((w.accessor.clone()?, l.list_row_at(w.index, cx, cy)?)))
            }) && self.fe.shop.click(self.career.as_ref(), &self.fe.strings, &acc, row)
            {
                self.bind_lists();
                self.bind_parts_buttons();
                return;
            }
            // and one on `UserList.UserList` selects that profile's row
            if let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| {
                l.list_boxes().into_iter().find(|w| w.accessor.as_deref() == Some("UserList.UserList")).and_then(|w| l.list_row_at(w.index, cx, cy))
            }) {
                self.fe.user_sel = Some(row);
                self.bind_lists();
                return;
            }

        }
        // a click on a timeline cell (the main box takes neither hover nor press itself — slots 5 / 6
        // return 0, 0x508000 — so this is reached with no widget under the pointer) — `FEChronicleMainBoxControl`'s slot 9,
        // 0x5080C0: row 0 opens `chroniclepicture` and row 1 `chroniclemedia`, each
        // for the column's own year and variant, and only when 0x5074A0 finds files
        // there (§4.120)
        if !down
            && let Some((cx, cy)) = self.fe.cursor
            && let Some((col, row)) = self.fe.layout_preview.as_ref().and_then(|l| l.chronicle_cell_at(cx, cy))
        {
            self.fe.chronicle_col = Some(col);
            // 0x5074A0 frees and rebuilds `[0x699C84]` and zeroes `[0x699CFC]` (0x50749B…)
            self.fe.chronicle_sel = 0;
            self.bind_chronicle_icons();
            self.bind_lists();
            if row < 2 && !self.chronicle_media_for(row).is_empty() {
                // `0x4D2100(name, 0, 1)` (0x50810F / 0x50814E)
                self.fe_open(if row == 0 { "chroniclepicture" } else { "chroniclemedia" }, 0, true);
            }
            return;
        }
        if let Some(hit) = self.fe.layout_preview.as_ref().and_then(|l| l.hovered()) {
            // An input box takes focus when it is activated — `FeInputBox`'s slot 9,
            // 0x4CCB70, which the base's release path reaches through the `"CLICKED"`
            // message (§4.40). Three classes share that slot. §4.109.
            if !down
                && let Some(l) = self.fe.layout_preview.as_mut()
            {
                let class = l.class_of(hit).unwrap_or_default();
                // `NFS5.CONTROLLERSETUP`'s slot 9 is **0x51C5F0**, three calls: take
                // focus (0x4C7D70), then 0x51F000 and 0x51F310 — the input layer's
                // "start listening for the next control". §4.117.
                if matches!(class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP") {
                    l.focus_input(hit);
                }
                if class == "NFS5.CONTROLLERSETUP" {
                    self.fe.binding_base = Some(self.input.pads.clone());
                }
                // `NFS5.GOTOBUTTON`'s slot 9, 0x4C9470: unless disabled (0x4C7E40), sound 5
                // before the transition
                if class == "NFS5.GOTOBUTTON" && !l.widget_disabled(hit) {
                    self.fe_sound(5);
                }
                // `NFS5.CAMERASELECT`'s slot 9, **0x4BF5F0**: `Camera.Current` takes the
                // widget's `accessornum` — this port's view index is that selection (§4.89)
                if let Some(n) = self.fe.layout_preview.as_ref().and_then(|l| l.camera_select_of(hit)) {
                    self.fe.camera_current = n as usize;
                }
                // `NFS5.CAMERAPICTURE`'s slot 9, **0x4BF920**: not focused (0x4BF923 `0x4C7DD0`,
                // `[+0x10C] & 0x40` — set only by 0x4C7D70 for a widget whose `[+0x110]` has 0x40,
                // which the base ctor 0x4C7480 clears and the base bind 0x4C7530 never sets), and
                // `[0x5DEB48] / 2` — half a second — since the widget's last release (0x4BF969…
                // 0x4BF984: the game timer 0x53C2E0 less `[this+0x140]`, `jge 0x4BF9CA`): the time
                // is taken (0x4BF9E3…0x4BF9EE) and `Camera.<Camera.Current + 1>` (the accessor at
                // `[this + 0x154 + 4·Camera.Current]`, 0x4BF9E8…0x4BF9F9) gets
                // `SetSelection(data.picturenum)` (slot 0x2C, 0x4BFA0B). A quicker second release
                // only clears the time (0x4BF99B `[this+0x140] = 0`), redraws (0x4C8AC0), plays 0x528850
                // (a sound call, not read) and puts the number in `[0x5D7A20]`, a cell only this
                // widget's draw and release read; no accessor is written.
                if let Some(pic) = self.fe.layout_preview.as_ref().and_then(|l| l.camera_picture_of(hit)) {
                    let now = Instant::now();
                    if self.fe.camera_pic_click.get(&hit).is_none_or(|t| now.duration_since(*t) >= Duration::from_millis(500)) {
                        self.fe.camera_pic_click.insert(hit, now);
                        if let Some(slot) = self.fe.camera_list.get_mut(self.fe.camera_current) {
                            *slot = pic;
                        }
                    } else {
                        self.fe.camera_pic_click.remove(&hit);
                    }
                }
                // the accessors' new values reach the screen's draw at once, as the next frame's
                // read of them would (the frame hook does the same on every tick)
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.set_camera_current(self.fe.camera_current);
                    l.set_camera_list(self.fe.camera_list);
                }
                // `NFS5.KNOCKOUTTRACKLIST`'s slot 9, **0x4FA470**: the row under the
                // pointer becomes `Knockout.TrackNumber`'s selection when it is below the
                // opponent count
                if let Some((_, cy)) = self.fe.cursor
                    && let Some(row) = self.fe.layout_preview.as_ref().and_then(|l| l.ko_row_at(hit, 0.0, cy))
                    && let Some(m) = self.fe.menu.as_mut()
                {
                    m.ko_sel = row;
                    self.bind_indicator_bars();
                    self.bind_lists();
                }
                // the tab buttons' slot 9, **0x4D0290**: the strip's group selects this
                // one (0x4CFF30), and sound 5 either way (0x4D02C2)
                if matches!(class.as_str(), "NFS5.TABBUTTON" | "NFS5.GRAPHICTABBUTTON") {
                    if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
                        l.tab_click(r, &self.paths, hit);
                    }
                    // `set_panel` (0x4CEA90, called from `tab_click`) loads the new panel's
                    // widgets — `display.lay`'s `NFS5.HUD` among them, one of `Options`'s
                    // five tab panels — after the screen's own open-time bind already ran
                    // once, so a widget in a panel reached only by a tab switch never got
                    // its content. Re-bind now that the panel exists, the same way
                    // `SHOWPLAYLISTPANEL`/`HIDEPLAYLISTPANEL` re-bind their lists after
                    // `set_panel` above.
                    //
                    // Audited every shipped RADIOGROUP+PANEL screen for the same shape of
                    // gap (2026-09-29): `showcase.lay`'s four
                    // panels (`ID_general_panel`/`ID_glancestats_panel`/`ID_chassis_panel`/
                    // `ID_performance_panel`) were the first found, fixed with a
                    // `bind_player_pictures`+`bind_showcase_pictures` pair; that pair missed
                    // `Controllers.lay`'s `forcefeedback`/`gamectrl` panels and
                    // `graphics.lay`'s `advgraphics` panel, whose `NFS5.GouraudIndicatorBar`
                    // meters and `NFS5.CarSettingSlider` knobs (`bind_indicator_bars`, set
                    // by `set_bar`/`set_knob` from an accessor — never re-run by either tab
                    // re-bind site) stayed blank on first click. A hand-picked pair only
                    // ever catches the one widget class someone happened to look for, so
                    // this now runs `bind_screen_widgets()` — the same full set
                    // `fe_bind_screen` runs on screen open — instead: every bind in it fires
                    // the same way regardless of which panel a widget lives in, so the
                    // original's own re-evaluation is never gated on which tab was already
                    // open, and there is no cheaper subset that is provably still complete
                    // the next time a `.lay` grows a new tab panel.
                    self.bind_screen_widgets();
                    self.fe_sound(5);
                }
                // `FEChroniclePictureIconControl`'s slot 9, **0x5089C0**: `if ([this+0x144] == 0)
                // return 0; [0x699CFC] = [this+0x140]` — the release of a thumbnail that has
                // art (0x5087EB, so `data.number` is inside the media list) makes its record the
                // one the big picture and the badge draw (0x508310, 0x4C16AE), at once
                if class == "NFS5.ChroniclePictureIcon"
                    && let Some(n) = self.fe.layout_preview.as_ref().and_then(|l| l.numbered_widgets("NFS5.ChroniclePictureIcon").into_iter().find(|(i, _)| *i == hit).map(|(_, n)| n))
                    && n >= 0
                    && (n as usize) < self.chronicle_media().len()
                {
                    self.fe.chronicle_sel = n as usize;
                    self.bind_car_logos();
                }
                // `NFS5.3DCAR`'s slot 9, 0x4C0EB0: a click opens or closes the part under
                // the pointer (§4.153)
                let l = match self.fe.layout_preview.as_mut() {
                    Some(l) => l,
                    None => return,
                };
                if class == "NFS5.3DCAR" {
                    let events = l.car_click(hit);
                    self.fe_car_part_events(hit, &events);
                }
                // `NFS5.PLAYERDRIVERPICTURE`'s slot 9, 0x4CF940: `[0x5D8F2C]` takes the
                // released picture's `data.ournumber`; nothing else, no sound
                if let Some(n) = self.fe.layout_preview.as_ref().and_then(|l| l.driver_picture_of(hit)) {
                    self.fe.driver_picture = n;
                    if let Some(l) = self.fe.layout_preview.as_mut() {
                        l.driver_picture = n;
                    }
                }
            }
            // `usedcars.lay`'s two panel buttons flip `[FEUsedCarsScreen + 0x290]`
            // through the `SHOWPARTS` and `SHOWDAMAGE` callbacks the constructor
            // registers at 0x4F71C0 and 0x4F71D1 (§4.113)
            if !down
                && let Some(which) = self
                    .fe.layout_preview
                    .as_ref()
                    .and_then(|l| l.widgets_named(&["ID_SHOWPARTSBUTTON", "ID_SHOWDAMAGEBUTTON"]).into_iter().find(|(i, _)| *i == hit).map(|(_, k)| k))
            {
                self.fe.usedcars_showparts = which == 0;
                self.bind_usedcars_panels();
                return;
            }
            // and a click on one of `chronicle.lay`'s five model-group buttons moves
            // `[0x5DBA44]` — 0x506E68, 0x506EA6 and the three arms after them each write
            // the group their button stands for and then reload the screen (§4.111)
            if !down
                && let Some(which) = self
                    .fe.layout_preview
                    .as_ref()
                    .and_then(|l| l.widgets_named(&CHRONICLE_GROUP_BUTTONS).into_iter().find(|(i, _)| *i == hit).map(|(_, k)| k))
            {
                self.fe.chronicle_group = which;
                // 0x507690 and its siblings: `Chronicle.Year` back to the group's earliest year
                self.fe.chronicle_year_idx = None;
                self.bind_chronicle_group();
                self.bind_chronicle_icons();
                self.bind_lists();
                return;
            }
            let controls = self.fe.layout_preview.as_ref().map(|l| l.controls()).unwrap_or_default();
            // a combo takes focus on the **release** (0x4C6CC1, after sound 2 at
            // 0x4C6C97), and focus is what opens its list (0x4C53A0) — clicking the open
            // one again gives the focus back
            //
            // An open one was closed above (`was_open`); a release on it again only
            // closes it. On a closed combo, 0x4C6B7E…0x4C6C3D comes first: a release in
            // the **caption strip** (`in_combo_strip`, `x < backrect.left + [+0x1F0]`)
            // runs `data.callback` (0x50D6D0) or goes to `data.destination` (0x4D2100
            // with `data.offset`) — no shipped combo carries either — and with both
            // empty (0x4C6C07…0x4C6C29) steps the accessor **forward** through slot 21
            // 0x4BC690 (0x4C6C33) and plays sound **5** (0x4C6C38); it does not open.
            // Anywhere else in `backrect` it opens on sound **2** (0x4C6C97), only with
            // rows to show (slot 9 ≠ 0, 0x4C6C4F).
            let combos = self.fe.layout_preview.as_ref().map(|l| l.combo_sliders()).unwrap_or_default();
            let strip = !down
                && was_open != Some(hit)
                && self.fe.cursor.is_some_and(|(cx, cy)| self.fe.layout_preview.as_ref().is_some_and(|l| l.in_combo_strip(hit, cx, cy)));
            if strip && combos.iter().any(|c| c.index == hit) {
                self.combo_strip_release(hit);
            } else if !down
                && was_open != Some(hit)
                && let Some(c) = combos.iter().find(|c| c.index == hit)
                && c.accessor.as_deref().and_then(|a| self.combo_rows(a)).is_none_or(|(rows, _)| !rows.is_empty())
            {
                self.fe_sound(2);
                self.fe.layout_open_combo = Some(hit);
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.set_open_combo(Some(hit));
                }
            }
            // `NFS5.CheckBox` is no button and carries no `accessor`, so it is not in
            // `controls()` at all — its tick comes off the hit itself. `repair.lay`'s ten
            // are the only ones in the front end, each naming a `QUICKFIX.*` bool (§4.95).
            // It ticks on the **release**: its handler is slot 9, **0x4C3770** — the
            // `checkBoxPic` part's rect at the widget box's corner (0x4C8670, 0x4E2970:
            // `left ≤ x < right`, `top ≤ y < bottom`) against the event's point, then the
            // accessor's slot 0x74 (the toggle), slot 0x6C read back into `[+0x141]` and the
            // update (slot 14). The part sits at `0,0` and the box autosizes round it, so
            // the rect is the widget's own. *(Until 2026-09-26 this ticked on the press.)*
            if !down
                && let Some(name) = self
                    .fe.layout_preview
                    .as_ref()
                    .and_then(|l| l.check_boxes().into_iter().find(|(i, _)| *i == hit).map(|(_, a)| a))
                && let Some(n) = name.strip_prefix("QUICKFIX.")
                && let Some(k) = QUICKFIX_NAMES.iter().find(|k| **k == n)
            {
                if !self.fe.repair_ticks.remove(*k) {
                    self.fe.repair_ticks.insert(k);
                }
                self.bind_checks();
                return;
            }
            if let Some(n) = controls.iter().position(|c| c.index == hit) {
                self.fe.layout_sel = n;
                // `NFS5.CARSETTINGBUTTON` steps its accessor on the **press**, not the
                // release: slot 5 (0x4C1F6C) takes focus and slot 27 (0x4C1F20) hands
                // `data.step` — `[this+0x144]`, bound at 0x4C1ED7 — to the accessor.
                // Seventeen of the 34 carry 1 and seventeen −1, so the arrow that points
                // left really does subtract. §4.84.
                if down
                    && controls[n].step != 0
                    && let Some(a) = controls[n].accessor.clone()
                {
                    let step = controls[n].step;
                    self.fe.setting_repeat = Some((a.clone(), step, 0));
                    self.set_accessor(&a, step);
                    // Every other `set_accessor` call runs the full `bind_after_accessor()`
                    // (`combo_commit`, `combo_strip_release`, `layout_key`'s Left/Right) —
                    // this arm used to re-bind a hand-picked six of its seventeen instead,
                    // missing `bind_carsetup_gearratio` among others (this button's own
                    // screen, `carsetup.lay`, is the *only* shipped `.lay` carrying
                    // `NFS5.CARSETTINGBUTTON`). The 34 shipped buttons'
                    // screen has no `NFS5.CarLogo`/chronicle/showcase/track-map/player-
                    // picture widgets at all, so those binds in the full set are a cheap
                    // widget-list scan away from a no-op here; every texture-backed one
                    // (`set_car_logo`/`set_pack_picture`) is already gated on the resolved
                    // file+entry actually changing, so the 150 ms repeat below never
                    // re-decodes or re-uploads anything already current.
                    self.bind_after_accessor();
                    return;
                }
                if !down && !combos.iter().any(|c| c.index == hit) {
                    self.layout_key(MenuKey::Enter, event_loop);
                }
            }
        }
    }
}
