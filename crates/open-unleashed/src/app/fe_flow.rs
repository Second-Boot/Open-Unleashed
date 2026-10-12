//! The front end's flow: dialogs, the bottom bar, opening and leaving screens.

use crate::*;

impl App {
    /// Raise a dialog (slot 13, 0x5128A0) with its text from `FEStrings` — unless the one for the
    /// same purpose is still up, which is 0x50CC1B's "gone" test.
    pub(crate) fn fe_raise_dialog(&mut self, kind: nfs_game::dialog::DialogKind, text_id: u32, labels: [u32; 2], purpose: DialogFor) {
        if self.fe.dialog.as_ref().is_some_and(|(d, p)| *p == purpose && !d.gone()) {
            return;
        }
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let text = self.fe.strings.get(text_id).unwrap_or_default().to_string();
        let mut d = nfs_game::dialog::Dialog::new(kind, text, labels);
        d.raise(now);
        self.fe.dialog = Some((d, purpose));
    }

    /// Act on a dialog's answer (the callers' polls of slots 18/19). Returns whether the front
    /// end is leaving.
    pub(crate) fn fe_dialog_answer(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.fe.dialog_answer.take() {
            // 0x50CC4F: "yes" to 811 is 0x4D56E0(0) — the front end is over
            Some((true, DialogFor::Quit)) => {
                event_loop.exit();
                true
            }
            Some((yes, DialogFor::Offer(i))) => {
                // the answered offer's tick (0x505F66…0x506076) enables the buttons again and
                // lifts the lock (0x4D5BC0)
                if self.fe.nav.back_lock {
                    self.fe_back_lock(false);
                }
                let mut sold = false;
                if let Some(c) = self.career.as_mut() {
                    if yes {
                        sold = c.career.accept_offer(&c.tables, i).is_ok();
                        c.car_sel = 0;
                    } else {
                        c.career.decline_offer(i);
                    }
                }
                // 0x502190 plays front-end sound 10 once the money is in (0x502224 `push 0xA /
                // call 0x4AADB0`)
                if sold {
                    self.fe_sound(10);
                }
                false
            }
            Some((true, DialogFor::Forfeit)) => {
                if let Some(c) = self.career.as_mut()
                    && let Some(t) = c.current().cloned()
                {
                    c.career.forfeit(t.id);
                }
                self.fe_back(0, event_loop);
                false
            }
            Some((true, DialogFor::KillReplay)) => {
                self.kill_selected_replay();
                false
            }
            // 0x5103DA…0x51042F: delete the selected stem; the list is rebuilt (0x4DEF60) when it
            // went, 804 says it did not
            // 0x50FB6B…0x50FC9F: an empty text does nothing; else the parsed price (0x524220) lists
            // the car (0x4F91C0) and the market ticks (0x4F7DB0 with the year, 0x5032C0)
            Some((true, DialogFor::SellPrice)) => {
                let typed = self.fe.dialog_input.take().unwrap_or_default();
                if !typed.is_empty()
                    && let Some(c) = self.career.as_mut()
                {
                    let ask: u32 = typed.parse().unwrap_or(0);
                    let sel = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                    if c.career.list_for_sale(sel, ask).is_ok() {
                        // 0x4F7DB0 seeds the CRT generator from the clock (0x4F7DB5)
                        let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u32).unwrap_or(1) | 1;
                        let mut rand = move || {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            seed >> 8
                        };
                        c.career.generate_offers(&c.tables, &mut rand);
                        c.career.reprice_market(&c.tables);
                        c.career.refresh_market(&c.tables, &mut rand, &mut || nfs_game::layout::glints::crt_rand() as u32);
                    }
                }
                self.bind_lists();
                false
            }
            // 0x5097C2's input dialog: another name, and the join goes again under it
            Some((true, DialogFor::NetName)) => {
                let typed = self.fe.dialog_input.take().unwrap_or_default();
                let host = self.net.session.as_ref().and_then(|n| n.host_addr());
                if let (Some(host), false) = (host, typed.trim().is_empty()) {
                    self.fe_session_leave();
                    let r = netplay::NetPlay::start(&format!("join:{host}"), typed.trim());
                    self.open_network_session("CONNECT", r);
                } else {
                    self.fe_back_callback(event_loop);
                }
                false
            }
            Some((true, DialogFor::NetGameName)) => {
                let typed = self.fe.dialog_input.take().unwrap_or_default();
                let name: String = typed.chars().take(netplay::GAME_NAME_MAX).collect();
                if let Err(e) = std::fs::write(netplay::game_name_path(), &name) {
                    tracing::debug!(error = %e, "Comms.GameName not kept");
                }
                let opened = self.net.lobby.as_ref().map(|l| l.create(&name));
                if let Some(r) = opened {
                    self.open_network_session("CREATE", r);
                }
                false
            }
            Some((false, DialogFor::NetGameName)) => false,
            Some((_, DialogFor::NetName | DialogFor::NetFail | DialogFor::NetConnecting | DialogFor::UpdateCheck)) => {
                self.fe_back_callback(event_loop);
                false
            }
            // 0x5112ED…0x51146F: the typed text, parsed (0x524220), goes to `OwnedCar.DecalNum`
            // through the accessor's slot 0x70; nothing typed is −1
            Some((true, DialogFor::Decal)) => {
                let typed = self.fe.dialog_input.take().unwrap_or_default();
                let n: i32 = if typed.is_empty() { -1 } else { typed.parse().unwrap_or(0) };
                if let Some(c) = self.career.as_mut() {
                    let sel = c.car_sel.min(c.career.garage.len().saturating_sub(1));
                    if let Some(o) = c.career.garage.get_mut(sel) {
                        o.trim.decal = n.clamp(-1, 99) as i8;
                    }
                }
                false
            }
            Some((true, DialogFor::RemovePlayer)) => {
                let profiles = nfs_game::save::list_profiles(&nfs_game::save::save_dir());
                if let Some(p) = self.user_row(&profiles).and_then(|i| profiles.get(i)) {
                    match nfs_game::save::delete_profile(&nfs_game::save::save_dir(), p) {
                        Ok(()) => {
                            self.fe.user_sel = None;
                            self.bind_lists();
                        }
                        Err(e) => {
                            tracing::info!(error = %e, "profile not deleted");
                            self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 804, [0, 0], DialogFor::Notice);
                        }
                    }
                }
                self.bind_player_buttons();
                false
            }
            Some((true, DialogFor::TournamentReset)) => {
                if let Some(c) = self.career.as_mut() {
                    c.confirm_reset();
                }
                // 0x4D2C94: the screen's tick runs the start check again — on the layout front
                // end that is `PLAYGAME`'s own (`start_tournament_from_layout`)
                if self.fe.layout_preview.is_some() {
                    return self.start_tournament_from_layout(event_loop);
                }
                self.menu_key(MenuKey::Enter, event_loop);
                self.fe.menu.is_none()
            }
            _ => false,
        }
    }

    /// A dialog the career screen asked for (`CareerUi::dialog`, §4.147): 858 two-button with
    /// its reset answer, the rest one-button notices — 802, 809, 810, 813, 857.
    pub(crate) fn fe_career_dialog(&mut self) {
        if self.fe.dialog.is_some() {
            return;
        }
        if let Some((id, two)) = self.career.as_mut().and_then(|c| c.dialog.take()) {
            use nfs_game::dialog::DialogKind;
            let (kind, purpose) = if two { (DialogKind::Two, DialogFor::TournamentReset) } else { (DialogKind::One, DialogFor::Notice) };
            self.fe_raise_dialog(kind, id, [0, 0], purpose);
        }
    }

    /// The pre-race screen's tick (0x505E58…0x505ED3): with no offer dialog up and a garage car
    /// holding an offer, ask about it — `FEStrings` 807 over the offer, the car's name and its
    /// asking price — with yes / no. The tick is slot 1 (0x505D50) of `FETournamentCallBackList`
    /// (0x5B8798), the class `callbacklist=ID_TOURNYSCREEN` builds — `prerace.lay` **and**
    /// `tournament.lay` (until 2026-09-24 the port asked for `prerace` alone).
    pub(crate) fn fe_offer_check(&mut self) {
        // the tick of `[fe+0xE0]`, the bottom layer's list
        let tourny_screen = self.fe_list_now() == "ID_TOURNYSCREEN";
        let on_career = tourny_screen || self.fe.menu.as_ref().is_some_and(|m| (1..=3).contains(&m.mode));
        if !on_career || self.fe.dialog.is_some() {
            return;
        }
        let Some(ui) = self.career.as_ref() else { return };
        let Some(i) = ui.career.pending_offer() else { return };
        let car = &ui.career.garage[i];
        let name = ui.tables.car(car.car_id).map(|c| c.display_name.clone()).unwrap_or_default();
        let fmt = self.fe.strings.get(807).unwrap_or_default();
        // "%d … %s … %d": the offer, the name, the asking price (0x505E95…0x505EB1)
        let text = fmt
            .replacen("%d", &car.offer.unwrap_or(0).to_string(), 1)
            .replacen("%s", &name, 1)
            .replacen("%d", &car.asking_price.unwrap_or(0).to_string(), 1);
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let mut d = nfs_game::dialog::Dialog::new(nfs_game::dialog::DialogKind::Two, text, [700, 701]);
        d.raise(now);
        self.fe.dialog = Some((d, DialogFor::Offer(i)));
        // 0x505F56: with the dialog up the tick disables its eight buttons and locks BACK
        // (0x4D5BA0 at 0x505F5C) — on the tournament screen, the only one whose tick this is
        if tourny_screen {
            self.fe_back_lock(true);
        }
    }

    /// One frame of the dialog: the tick, the answer kept for `fe_dialog_answer`, the screens
    /// covered while it is up.
    pub(crate) fn fe_dialog_step(&mut self) {
        // `NFS_FE_DIALOG=<FEStrings id>` raises a two-button dialog with that text at tick 0, for
        // a still — the way `NFS_FE_PARTS` opens the car's parts
        if self.fe.dialog.is_none()
            && !self.fe.dialog_forced
            && let Some(id) = nfs_game::config::cfg().nfs_fe_dialog.as_str().and_then(|v| v.parse::<u32>().ok())
        {
            self.fe.dialog_forced = true;
            let text = self.fe.strings.get(id).unwrap_or_default().to_string();
            let mut d = nfs_game::dialog::Dialog::new(nfs_game::dialog::DialogKind::Two, text, [700, 701]);
            d.raise(0);
            self.fe.dialog = Some((d, DialogFor::Quit));
        }
        // `NFS_FE_DIALOG=input:<FEStrings id>:<typed>` — the input dialog with that text typed
        if self.fe.dialog.is_none()
            && !self.fe.dialog_forced
            && let Some(spec) = nfs_game::config::cfg().nfs_fe_dialog.as_str().and_then(|v| v.strip_prefix("input:").map(str::to_string))
        {
            self.fe.dialog_forced = true;
            let (id, typed) = spec.split_once(':').unwrap_or((&spec, ""));
            let text = self.fe.strings.get(id.parse().unwrap_or(0)).unwrap_or_default().to_string();
            let mut d = nfs_game::dialog::Dialog::input(text, true, 2);
            d.raise(0);
            typed.chars().for_each(|c| {
                d.type_char(c);
            });
            self.fe.dialog = Some((d, DialogFor::Notice));
        }
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let mut answer = None;
        let mut up = false;
        if let Some((d, purpose)) = self.fe.dialog.as_mut() {
            d.tick(now);
            if let Some(a) = d.take_answer() {
                answer = Some((a, *purpose));
                self.fe.dialog_input = d.field.as_ref().map(|f| f.text.clone());
            }
            up = d.up;
            if d.gone() {
                self.fe.dialog = None;
            }
        }
        // a 3D car zoomed into a door covers every screen each tick too (0x4C052F → 0x4D2000),
        // and lets them go through 0x4D20F0 (= 0x4D1E70) when it zooms back out
        let zoomed = self.fe.layout_preview.as_ref().is_some_and(|l| l.car_3d().iter().any(|w| l.car_zoom(w.index).1));
        let up = up || zoomed;
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_covered(up);
        }
        // 0x4D1E70 also covers the bottom bar while more than one layer is open (0x4D1F4C)
        let layered = self.fe.nav.bottom_covered();
        if let Some(b) = self.fe.layout_bottom.as_mut() {
            b.set_covered(up || layered);
        }
        if answer.is_some() {
            self.fe.dialog_answer = answer;
        }
    }

    /// `careerfirst`'s way on — `NFS5.MOVIECIRCLE`'s tick 0x4CD410 with `othersource=careerfirst`
    /// (0x4CD5A6…0x4CD62F): while its clip is open (0x4E2370), when the clip has ended (0x4E2960)
    /// or the keyboard's scan code 1 — ESC — is down (0x55FEB0(6), byte `+5` of the key table
    /// at 0x6A5A10), it stops the clip (0x4E2360) and runs `0x4D2100("tournament", 0, 1)`. The
    /// ESC half arrives through the key handler (the ESC arm beside `fe_manager_key`).
    pub(crate) fn fe_career_first_tick(&mut self) {
        if self.fe.layout_name.as_deref() == Some("careerfirst") && self.fe.layout_preview.as_ref().is_some_and(|l| l.movie_finished()) {
            self.fe_open("tournament", 0, true);
        }
    }

    /// The bottom bar's button states (`controls.md` §4.165.2): its refresh **0x4D81C0**, which the
    /// front end runs after every open (0x4D25DC, 0x4D26E2, 0x4D2C19) and at the end of 0x4D3420
    /// (0x4D5210 through 0x4D6130), and its tick **0x4D8250** (slot 1 of vtable 0x5B6888).
    ///
    /// * `ID_BACKBUTTON` `[+0x264]` enabled iff **0x4D6320**: not locked (`[+0x17C]`), and the one
    ///   layer's history holds more than the screen itself — or the front end was entered from
    ///   the race's options (`[+0x178]` = 1), which no build of the race can ask for — no HUD
    ///   page answers the pump's event 1 that leads to 0x4155B4 (`controls.md` §4.169.1);
    /// * `ID_OPTIONSBUTTON` `[+0x268]` enabled iff the callback list's slot 0xA8 is false (every
    ///   list but `ID_OPTIONSSCREEN`'s, 0x5B9F58) and there is one layer (0x4D63A0);
    /// * `ID_PLAYGAMEBUTTON` `[+0x26C]` by the top screen's `canplaygame` (slot 0x90, 0x4E0BF0 —
    ///   the reader §4.85 could not find): 0 disabled, 1 enabled, anything else the callback list's
    ///   slot 0xA4 — false for `ID_FDVICTORYSCREEN`, `ID_TOURNYSCREEN`'s 0x505D30 "the garage
    ///   (0x501AC0, `OwnedCars`) is not empty", `ID_COMMSCREATESCREEN`'s 0x5091B0 (below), true
    ///   for the rest;
    /// * `ID_CASHBKGD` / `ID_ACCOUNTTHINGY` `[+0x270]` / `[+0x274]` shown iff `Game.GameType` is 4
    ///   and the list's slot 0xAC is false — true for `ID_VICTORYSCREEN`, `ID_CHRONICLE`,
    ///   `ID_CREDITS`, `ID_MAINSCREEN`, `ID_OPTIONSSCREEN` and `ID_FDVICTORYSCREEN`.
    ///
    /// BACK and OPTIONS read the manager's layers (`nfs_game::layout::nav`, §4.166): 0x4D6320
    /// and 0x4D63A0 exactly.
    /// Inputs as the port has them: `[+0x178]` (the race's options entry) is never set — nor in
    /// the original (§4.169.1); the lock `[+0x17C]` is `fe_back_lock`'s (§4.169.2). 0x5091B0 needs the session linked, not ejected,
    /// `+0xBE` set and an id (as §4.163's four tests), every player's `[0x658604 + id·0x580]` set
    /// and `net+0xE5` clear — `App::net_can_start` (`network.md` §11.9).
    pub(crate) fn bind_bottom_bar(&mut self) {
        const NAMES: [&str; 5] = ["ID_BACKBUTTON", "ID_OPTIONSBUTTON", "ID_PLAYGAMEBUTTON", "ID_CASHBKGD", "ID_ACCOUNTTHINGY"];
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        // `data.callbacklist`, "" read as `ID_NONE` (0x4E0B30)
        // — slots 0xA8 / 0xA4 / 0xAC of `[fe+0xE0]` (0x4D81F3, 0x4D82BB, 0x4D8324), the object the
        // **bottom** layer's screen names (0x4D1BD0 / 0x4D2570), not the top's
        let list = self.fe_list_now();
        // 0x4D6320 and 0x4D63A0 over the manager's layers (`nfs_game::layout::nav`)
        let back = self.fe.nav.back_enabled();
        let options = list != "ID_OPTIONSSCREEN" && self.fe.nav.one_layer();
        let play = match l.layout.screen().and_then(|s| s.int("canplaygame")).unwrap_or(0) {
            0 => false,
            1 => true,
            _ => match list.as_str() {
                "ID_FDVICTORYSCREEN" => false,
                "ID_TOURNYSCREEN" => !self.owned_list().is_empty(),
                "ID_COMMSCREATESCREEN" => self.net_can_start(),
                _ => true,
            },
        };
        let account = self.fe.game_type == game_type::TOURNAMENT && !slot_ac(&list);
        let layered = self.fe.nav.bottom_covered();
        let Some(b) = self.fe.layout_bottom.as_mut() else { return };
        // 0x4D1E70 after the open: the bar is covered while more than one layer is open
        // (0x4D1F4C), which disables every button whose template has the state (0x4C7E62);
        // uncovering is left to the dialog step, which also covers it for a dialog
        if layered {
            b.set_covered(true);
        }
        for (index, which) in b.widgets_named(&NAMES) {
            match which {
                0 => b.set_disabled_flag(index, !back),
                1 => b.set_disabled_flag(index, !options),
                2 => b.set_disabled_flag(index, !play),
                _ => b.set_hidden_flag(index, !account),
            }
        }
    }

    /// A release on the bottom bar — `NFS5.BOTTOMBARBUTTON`'s slot 9 is `NFS5.GOTOBUTTON`'s
    /// **0x4C9470**: unless disabled, sound 5, then the callback (0x50D6D0) and the destination.
    /// `QUIT` raises the exit dialog; `GOBACK` and `PLAYGAME` are the port's own back and race
    /// paths; `OPTIONS` is `fe_options`.
    pub(crate) fn fe_bottom_click(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(b) = self.fe.layout_bottom.as_ref() else { return false };
        let Some(hit) = b.hovered() else { return false };
        if b.widget_disabled(hit) {
            return true;
        }
        let Some(c) = b.controls().into_iter().find(|c| c.index == hit) else { return false };
        self.fe_sound(5);
        match c.callback.as_deref() {
            // slot 0xBC — `carsetup`'s 0x4F1D00 asks 0x502420 first
            Some("QUIT") => {
                if !self.fe_carsetup_blocked() {
                    self.fe_raise_dialog(nfs_game::dialog::DialogKind::Two, 811, [0, 0], DialogFor::Quit);
                }
            }
            // 0x50E5A0: the callback list's slot 0xB4
            Some("GOBACK") => self.fe_back_callback(event_loop),
            Some("PLAYGAME") => {
                if self.fe.layout_preview.as_ref().is_some_and(|l| l.can_play()) {
                    self.fe_start_game(event_loop);
                }
            }
            Some("OPTIONS") => self.fe_options(),
            other => tracing::info!(callback = ?other, "bottom bar callback not ported"),
        }
        true
    }

    /// `OPTIONS` — `bottombar.lay`'s `ID_OPTIONSBUTTON`, which has no `destination`. The name is
    /// registered at 0x50C275 in `FECallBackList`'s constructor with the thunk **0x50E5C0**
    /// (`jmp [vtbl+0xD8]`, slot 54), whose base is **0x50D310**: `0x4D2100("options", 0, 1)` —
    /// offset 0, so `options` **replaces** the top screen and the stack depth stays (the
    /// `GOTOBUTTON` offset-0 arm of `layout_key`).
    ///
    /// Of the 31 vtables that carry the table, one overrides slot 54: `carsetup`'s (0x5B7558)
    /// **0x4F1D20** runs 0x50D310 only when 0x502420 lets it — the parts-missing check behind
    /// dialog 650 (`fe_carsetup_blocked`), which the screen's back, `QUIT` and `TESTDRIVECHECK`
    /// share.
    pub(crate) fn fe_options(&mut self) {
        if self.fe_carsetup_blocked() {
            return;
        }
        self.fe_open("options", 0, true);
    }

    /// **0x502420** — `carsetup`'s gate (§4.169.6), true when the car may not leave: on
    /// `ID_CARSETUP` its back 0x4F1CE0, `QUIT` 0x4F1D00, `OPTIONS` 0x4F1D20 and `TESTDRIVECHECK`
    /// 0x4F1A90 all run it first. `Game.GameType` 0…3 takes `zMergedList.Car`'s car while it is
    /// in the list's owned half (0x4F4DF0 — `car_garage`, `bind_mergedlist_panel`'s own test),
    /// 4 `OwnedCars.Car`, anything else passes. The car's stock parts without an installed part
    /// in their slot (0x4F0220, `parts_shop::missing_parts`) raise the one-button **650** with
    /// their names (0x5135B0, 0x4F03CC).
    pub(crate) fn fe_carsetup_blocked(&mut self) -> bool {
        // the callback list `[fe+0xE0]` is the bottom layer's
        let on_carsetup = self.fe_list_now() == "ID_CARSETUP";
        if !on_carsetup {
            return false;
        }
        let Some(c) = self.career.as_ref() else { return false };
        let car = match self.fe.game_type {
            0..=3 => self.fe.menu.as_ref().filter(|m| m.car_garage).map(|_| c.car_sel.min(c.career.garage.len().saturating_sub(1))),
            game_type::TOURNAMENT => (!c.career.garage.is_empty()).then(|| c.car_sel.min(c.career.garage.len() - 1)),
            _ => None,
        };
        let Some(car) = car else { return false };
        let (stock, installed) = parts_shop::stock_and_installed(c, car);
        let missing = parts_shop::missing_parts(&stock, &installed);
        if missing.is_empty() {
            return false;
        }
        let names: Vec<String> = missing.iter().map(|p| self.fe.strings.get(p.name_string_id).unwrap_or_default().to_string()).collect();
        let head = self.fe.strings.get(650).unwrap_or_default().to_string();
        let text = parts_shop::missing_parts_text(&head, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let mut d = nfs_game::dialog::Dialog::new(nfs_game::dialog::DialogKind::One, text, [0, 0]);
        d.raise(now);
        self.fe.dialog = Some((d, DialogFor::Notice));
        true
    }

    /// **0x4D2100(`name`, `offset`, `record`)** — the manager's open (`nfs_game::layout::nav`):
    /// `offset > 0` a new layer, `< 0` (two layers or more) the top one closed and the one under
    /// it takes the screen, else the top layer's screen replaced; `record` puts the name on that
    /// layer's history. Then sound 0 (0x4D23D0) and the screen itself (`open_layout`, which runs
    /// 0x4D1BD0 and the bottom bar).
    pub(crate) fn fe_open(&mut self, name: &str, offset: i32, record: bool) {
        let layers = self.fe.nav.layers.len();
        let shown = self.fe.nav.open(name, offset, record);
        tracing::debug!(name, offset, record, nav = self.fe.nav.describe(), "front end: 0x4D2100");
        self.fe_sound(0);
        match self.fe.nav.layers.len().cmp(&layers) {
            // the screen on show stays, under the new layer
            std::cmp::Ordering::Greater => self.fe_push_under(),
            // the top layer closed; the one under it takes the destination, so its screen goes
            std::cmp::Ordering::Less => {
                if let Some((_, old)) = self.fe.layout_under.pop() {
                    self.fe_screen_gone(&old);
                }
            }
            std::cmp::Ordering::Equal => {}
        }
        if let nfs_game::layout::nav::Shown::Screen(n) = shown {
            self.open_layout(&n);
        }
    }

    /// **0x4D23F0(`arg`)** — the manager's back: −1 empties the top layer's history and closes
    /// it (sound 4); anything else is 0x4D5FB0 on the top layer — drop the last name and show the
    /// one before (sound 1), or, with none before it, close the layer (sound 4); an empty history
    /// does nothing. A closed layer uncovers the one under it, whose screen the original never
    /// freed — the port keeps it loaded too (`fe_push_under` / `fe_pop_under`). The last layer
    /// closing ends the front end; the port has no main loop around
    /// it to go back to, so it leaves the program as its old back did at the bottom of its stack.
    pub(crate) fn fe_back(&mut self, arg: i32, event_loop: &ActiveEventLoop) {
        use nfs_game::layout::nav::Shown;
        let layers = self.fe.nav.layers.len();
        let shown = self.fe.nav.back(arg);
        tracing::debug!(arg, nav = self.fe.nav.describe(), "front end: 0x4D23F0");
        match shown {
            Shown::Nothing => {}
            Shown::Ended => {
                // the last layer closes: its screen gets `NOTINUSE` (0x4D5610), which stores the
                // Display options when that screen is the Options one (N02)
                if self.fe.layout_preview.as_ref().is_some_and(|l| Self::fe_list_of(l) == "ID_OPTIONSSCREEN") {
                    self.hud_cfg_apply();
                }
                self.fe_sound(4);
                event_loop.exit();
            }
            Shown::Screen(n) => {
                let closed = layers - self.fe.nav.layers.len();
                self.fe_sound(if arg == -1 || closed > 0 { 4 } else { 1 });
                if closed == 0 {
                    self.open_layout(&n);
                } else {
                    // the closed layers' screens go; the one under them is shown again
                    for _ in 1..closed {
                        if let Some((_, old)) = self.fe.layout_under.pop() {
                            self.fe_screen_gone(&old);
                        }
                    }
                    self.fe_pop_under();
                }
            }
        }
    }

    /// The screen's callback list's **slot 0xB4** — what `GOBACK` (0x50E5A0),
    /// the `b` key (0x4D2A6B) and the player screens' backs call. The base is 0x50CBF0 =
    /// 0x4D23F0(0); of the callback-list vtables (those with 0x4E0B30 at +0x88) six override it:
    ///
    /// * `ID_VICTORYSCREEN` (0x5B89D4) and `ID_FDVICTORYSCREEN` (0x5BAABC): **0x516950, a bare
    ///   `ret`** — no back at all;
    /// * `ID_CARSETUP` (0x5B7558) 0x4F1CE0: the back only when 0x502420 is false —
    ///   `fe_carsetup_blocked`;
    /// * `ID_OPTIONSSCREEN` (0x5B9F58) 0x5118D0: `[fe+0x178]` == 1 → 0x4D56E0(1) (back into the
    ///   race), dead in this build — no HUD page answers the event that opens the options from
    ///   a race (§4.169.1) — else the back;
    /// * `ID_COMMSCREATESCREEN` 0x509170 / `ID_COMMSJOINSCREEN` 0x50A4B0 (0x48E6F0 on a linked
    ///   session) and `ID_PEERTOPEERIP` / `ID_COMMSSERVERSCREEN` 0x50B4C0 (0x48E9C0): the session
    ///   is left (`fe_session_leave`, §4.169.4), then the back;
    /// * the LAN / modem / web lists' 0x506CB0 is `jmp 0x50CBF0`.
    pub(crate) fn fe_back_callback(&mut self, event_loop: &ActiveEventLoop) {
        // slot 0xB4 of `[fe+0xE0]` (the `b` key 0x4D2A6B, `GOBACK` 0x50E5A0): the bottom layer's
        // list, so a back pressed on an overlay runs the covered screen's list's
        // override — `settings` over `carsetup` asks 0x502420 first, over a comms screen leaves the
        // session
        let list = self.fe_list_now();
        match list.as_str() {
            "ID_VICTORYSCREEN" | "ID_FDVICTORYSCREEN" => return,
            // 0x4F1CE0
            "ID_CARSETUP" if self.fe_carsetup_blocked() => return,
            // 0x509170 (`[+0x286]` = 1 first) / 0x50A4B0 (once, `[+0x291]`): 0x48E6F0 when the
            // session `[0x628C70]` is linked (`+8`) and not ejected (`+0xBF`)
            "ID_COMMSCREATESCREEN" | "ID_COMMSJOINSCREEN" => self.fe_session_leave(),
            // 0x50B4C0: 0x48E9C0 whenever the session object exists
            "ID_PEERTOPEERIP" | "ID_COMMSSERVERSCREEN" => self.fe_session_leave(),
            _ => {}
        }
        self.fe_back(0, event_loop);
    }

    /// The comms screens' way out of a session — **0x48E6F0** (create / join) and **0x48E9C0**
    /// (peer-to-peer, server list), `controls.md` §4.169.4. 0x48E9C0 is the teardown: with the
    /// link up (`+0xC2`) and a kind other than 2 (modem) it sends **`0xB1`** (our id `+0xDC`, or
    /// −2 unlinked) on the host link `[+0xC]` — "a player goes", `network.md` §10.1 — then
    /// 0x492260, frees `[+0x14]`, the links `[+0xC]` and `[+8]` (0x48AED0) and, linked, the room
    /// `[0x628C74]` (0x491FC0), and clears the state (`+0x24`, `+0xBF`, `+0xC0`, `+0xC2`,
    /// `+0xC6`, `+0xE0`; 0x48EAE8…0x48EB06). 0x48E6F0 is that teardown for the transport kinds
    /// `[+0x28]` 0 and 2 (then 0x48E1A0(kind, 0, 0) makes a fresh comms object), and for the
    /// others, when `[+0xC6]` is set, frees `[+0x14]` and sends `0xBA` instead (ignored by every
    /// receiver).
    ///
    /// **Every arm the port can reach is the teardown** (`network.md` §11.27, settled): `[+0x28]`
    /// is the transport kind, not `Comms.GameMode` — the LAN `local` screen's constructor 0x50A950
    /// tears the object down and makes a kind-0 one (0x50A98F / 0x50A9DC), the modem's is 2, and
    /// the IP screen's host is kind 1 with `+0xC6` = 0. `+0xC6` is written 1 at two places only:
    /// 0x48DF5A (0x48DCB0's re-`CREATE` when the link is in the room but has no id) and 0x48E3AC
    /// (0x48E1A0's kind-1 `0xB6` request, the web lobby's `JOIN` 0x50B73D); the room's `+0xBE`
    /// makes 0x48DCB0's first test (0x48DE0B) send the description `0xB4` instead. The `+0xC6`
    /// arm is the internet lobby's (a member that goes back to the list stays linked), which the
    /// port does not have.
    ///
    /// The port's session is `NetPlay`: leaving is its `'K'` (0x589152 clears our bit on the host,
    /// whose room then sends `0xB1` for us) and dropping it closes the socket; the lobby's
    /// browsing socket is reopened by the screen the back shows (`fe_bind_screen`), which is the
    /// kind-0 object 0x48E1A0(0, 0, 0) makes. The host's room going is the joiners' `0xC6 (0xC)`.
    /// APPROXIMATION: our own `0xB1` to the host is the host's to send.
    pub(crate) fn fe_session_leave(&mut self) {
        if let Some(mut n) = self.net.session.take() {
            tracing::info!("front end: session left (0x48E6F0 / 0x48E9C0)");
            self.net.connect_pending = false;
            n.exchange_leave();
            n.leave();
            // 0x48E9C0 (0x48E9E2…0x48EA2B): `+0xC2` — written 1 only by the host's create, 0x48E143 — and a kind
            // other than the modem's: the leave `0xB1` goes out and the teardown waits `[0x5DEB48] / 2` ticks
            // (0x53C380 pumping the timer callbacks 0x5366E0: the queue's pump 0x48EE90, the room's tick) before
            // it frees the links. The original's wait blocks the whole process; the port keeps the session alive
            // and drains it for the same time while the window goes on, so the leave and what the room answers
            // it with (the joiners' `0xC6 (0xC)`) are on the wire before the socket closes.
            if n.is_host() {
                let wait = Duration::from_millis(1000 * (nfs_net::reliable::TICK_HZ / 2) as u64 / nfs_net::reliable::TICK_HZ as u64);
                self.net.leaving.push((n, Instant::now() + wait));
            }
        }
    }

    /// `carinfo+0xC` for the player's car: `Car.Transmission` at index 1, 561 "automatic"
    /// (`fe.txt`'s `car[0].transmission = 1`); 0, 560 "manual", otherwise.
    ///
    /// A Factory Driver mission overrides it: 0x4EB9B7…0x4EB9D7 set the per-car routine's seventh
    /// argument when the current mission (0x51AEC0, the first not yet passed) is index **0x1D**,
    /// and 0x4EEF8D then sends `.TRANSMISSION` 0 — a manual gearbox — whatever the screen says.
    ///
    /// **A playback** reads the word from the recording: `Replay_begin` 0x413CF0 copies the whole recorded block
    /// over `0x6573E8` (0xF89 dwords, `rep movsd`), the car-info records with it, and the machine-local words it
    /// puts back (`0x657C84…`, `0x657A48…`, `0x657D5C…`) do not reach `+0xCA4…`; the race's own 0x411130 hands
    /// 0x49CB50 / 0x49D290 / the shift logic that record's `+0xC`, not `Car.Transmission` (RP, replay.md §18.18).
    /// The player's slot only: the opponents', cops' and traffic cars' words are [`Self::option_automatic`] (the
    /// recording's slot-1 word in a playback), so mission 0x1D's manual gearbox is the player's alone (GB).
    pub(crate) fn player_automatic(&self) -> bool {
        if self.playback_running()
            && let Some(a) = self.replay.setup.transmission
        {
            return a;
        }
        if self.mission.as_ref().is_some_and(|m| m.index == 0x1D) {
            return false;
        }
        self.option_automatic()
    }

    /// `carinfo+0xC` of every car but the player's: 0x4EEDA0's `.TRANSMISSION` is `Car.Transmission` whenever its
    /// seventh argument is 0 (0x4EEF8D / 0x4EEF95 → 0x4EEF9F), and only the player's call 0x4EBA4D passes mission
    /// 0x1D's flag (0x4EB9C6…0x4EB9CB → `[esp+0xC]`, pushed as the seventh argument); the opponents' callers (0x4EBCBF /
    /// 0x4EBF06 / 0x4EC3FF / 0x4DE213), the cops' and the traffic's pass 0. A playback answers the recording's
    /// opponent word (slot 1 `+0xC`), else the player's.
    pub(crate) fn option_automatic(&self) -> bool {
        if self.playback_running()
            && let Some(a) = self.replay.setup.opponent_transmission.or(self.replay.setup.transmission)
        {
            return a;
        }
        self.choice_index(choice::CAR_TRANSMISSION).is_none_or(|(i, _)| i == 1)
    }

    /// `TIMER_getTicks()` (0x53C2E0), the 128 Hz clock `TIMER_init` (0x565030, called from main
    /// 0x4B6A9B) starts at the process start: the port's is the App's construction.
    pub(crate) fn fe_timer_ticks(&self) -> i64 {
        (self.fe.timer_origin.elapsed().as_millis() as i64 * 128) / 1000
    }

    /// The tail of 0x510210's re-roll once 0x4DC140 returns: `MusicPlay("Menu*", 0)` (0x510246…0x510251)
    /// and the stamp `[+0x290] = TIMER_getTicks()` (0x510254…0x510259) — read after the movie.
    pub(crate) fn finish_idle_movie(&mut self) {
        self.fe_music(|m, a, draw| m.play_pattern(a, "Menu*", 0, draw));
        self.fe.main_music_stamp = self.fe_timer_ticks();
    }

    /// 0x4DC140 on screen: the movie and the wait for the buttons take the window and the renderer
    /// the way the start-up movies do, and hand them back (`intro_event`).
    pub(crate) fn start_idle_movie(&mut self) {
        self.idle_movie_due = false;
        let (Some(renderer), Some(window)) = (self.renderer.take(), self.window.take()) else { return };
        if self.snd.audio.is_none() {
            self.snd.audio = nfs_audio::AudioOutput::new().map_err(|e| tracing::warn!(error = %e, "audio disabled")).ok();
        }
        tracing::info!("front end: the main screen's minute is up — inPC-av.mad (0x4DC140 → 0x4DD4A0)");
        self.intro = Some(intro_run::IntroRun::new_idle(window, renderer, &self.paths));
        self.idle_movie_running = true;
    }
}

/// A screen's `data.callbacklist`, upper-cased, "" read as `ID_NONE` (0x4E0B30) — the id that
/// picks the class of the current screen `[fe+0xE0]`.
pub(crate) fn callback_list_of(l: &nfs_game::layout::LayoutScreen) -> String {
    l.layout.elements.first().and_then(|e| e.get("callbacklist")).filter(|c| !c.is_empty()).unwrap_or("ID_NONE").to_ascii_uppercase()
}

/// The screen class's slot 0xAC: **0x510E90** (`mov al, 1`) for `ID_VICTORYSCREEN` (0x5B89D4),
/// `ID_CHRONICLE` (0x5B8B24), `ID_CREDITS` (0x5B98DC), `ID_MAINSCREEN` (0x5B9D64),
/// `ID_OPTIONSSCREEN` (0x5B9F58) and `ID_FDVICTORYSCREEN` (0x5BAABC); 0x50DD60 (`xor al, al`) for
/// every other screen class. It hides the bar's `ID_CASHBKGD` / `ID_ACCOUNTTHINGY` (0x4D8324) and
/// the career era tabs (0x4D8461).
pub(crate) fn slot_ac(list: &str) -> bool {
    matches!(list, "ID_VICTORYSCREEN" | "ID_CHRONICLE" | "ID_CREDITS" | "ID_MAINSCREEN" | "ID_OPTIONSSCREEN" | "ID_FDVICTORYSCREEN")
}
