//! The lobby screens' per-player logic — `creategame` / `joingame` (`RE/notes/network.md` §11.9).
//!
//! The original's `FECommsCreateScreen` tick 0x509290 and `FECommsJoinScreen` tick 0x50A4F0 send the
//! player's own slot records once the connection is up, `zMergedList.Car` (mode 1, built with
//! `push 1` at 0x4F2274) re-sends the car's three records on every selection (0x4BC240 →
//! 0x4B7F70 → slot 1 0x4F4890 → 0x48D550), `READY` (0x50A300) toggles `.READY`, and the create
//! screen's PLAYGAME is live only while 0x5091B0 holds. The rows of `Comms.PlayerList` are drawn
//! by 0x4BBEB0.

use super::*;
use nfs_net::slots;

/// The `Comms.PlayerList` row's tags: 0x5D7524, 0x5D7518, 0x5D7508.
const NAMEPOS: &str = "ID_NAMEPOS";
/// APPROXIMATION (instant): how many ticks the update thread 0x4A5750 is still seen running (state 2) after
/// `SETMODENET`; the original's duration is the DLL's connect to `trans.ea.com:80`, which the port never makes.
pub(crate) const UPDATE_THREAD_TICKS: u32 = 1;
const CARPOS: &str = "ID_CARPOS";
const STATUSPOS: &str = "ID_STATUSPOS";

impl App {
    /// The player's own car as `zMergedList.Car`'s slot 1 (0x4F4890) puts it on the wire: the
    /// catalogue id (slot 36's word), the name the sub-list's slot 4 answers — `%s *` (0x5DA728)
    /// for a garage car — and the colour (slot 29). The colour is the paint the launch would use
    /// (`start_from_layout`): the menu's paint row or its custom colour, as `0xAARRGGBB`.
    pub(crate) fn net_own_car(&self) -> Option<(i32, Vec<u8>, u32)> {
        let m = self.fe.menu.as_ref()?;
        let car = self.logo_car(Some("zMergedList.Car"), None)?;
        let name = if m.car_garage { format!("{} *", car.display_name) } else { car.display_name.clone() };
        let rgb = Self::menu_custom_paint(m, car)
            .map(|p| p.exterior)
            .or_else(|| car.paint_sets.get(m.paint_idx % car.paint_sets.len().max(1)).map(|p| p.colours[0]))
            .unwrap_or([0xD4, 0x10, 0x10]);
        let colour = 0xFF00_0000 | (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
        let bytes = name.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
        Some((i32::from(car.id), bytes, colour))
    }

    /// **The player's own record as 0x4EEDA0 (kind 1) reads it** — the inputs of
    /// [`nfs_net::launchcar::OwnCar`], from the car `zMergedList.Car` stands on (the catalogue
    /// record, or the garage car's record and set-up while `car_garage`), the paint the launch
    /// would use, the accessors the dump reads (`Car.ABS`, `Car.Traction`, `Car.Roof`,
    /// `Car.Transmission`) and the cheat bits (`network.md` §11.12).
    ///
    /// **APPROXIMATIONS** (direction: each value is the accessor's own where the port has one):
    /// `Camera.1 … Camera.4` are sent at the constructor's values 0…3 (0x4E4DB4…0x4E4E68) — the
    /// port keeps no camera-list choice; `Car.ABS` and `Car.Traction` are read without their
    /// predicates 0x4F1DA0 / 0x4F1DF0 (a car without the capability would read 0), which the
    /// walk's later `.ABS` / `.TRACTION` overwrite anyway.
    pub(crate) fn net_own_launch_car(&self) -> Option<nfs_net::launchcar::OwnCar> {
        let m = self.fe.menu.as_ref()?;
        let entry = self.logo_car(Some("zMergedList.Car"), None)?;
        let (_, car_name, colour) = self.net_own_car()?;
        let bytes = |s: &str| -> Vec<u8> { s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect() };
        // 0x4EEEB5 / 0x4EEEC6: the `evenstvn` code (cheat bit 3) replaces the spec names
        let cheat = self.cheat_bits & (1 << 3) != 0;
        // the paint the race puts on it (`start_race`): the custom colour, else the set whose exterior it is
        let rgb = entry.paint_sets.get(m.paint_idx % entry.paint_sets.len().max(1)).map(|p| p.colours[0]).unwrap_or([0xD4, 0x10, 0x10]);
        let paint = Self::menu_custom_paint(m, entry).unwrap_or_else(|| Paint::matching(entry, rgb));
        let dword = |c: [u8; 3]| 0xFF00_0000 | (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]);
        let garage = if m.car_garage { self.garage_car().map(|(_, o)| o) } else { None };
        // 0x4A5400: `fld; fmul 65536; _ftol` — a truncating float → 16.16
        let fixed = |v: f32| (v * 65536.0) as i32;
        let setup = garage.map_or(nfs_game::career::CAR_SETUP_DEFAULT, |o| o.setup).map(fixed);
        let parts: Vec<u32> = match garage {
            Some(o) => o.parts.iter().map(|p| *p as u32).collect(),
            None => entry.stock_parts.iter().map(|p| *p as u32).collect(),
        };
        let index = |name: &str| self.choice_index(name).map_or(0, |(i, _)| i as u32);
        Some(nfs_net::launchcar::OwnCar {
            driver: u32::from(self.career.as_ref().map_or(nfs_game::save::DEFAULT_PICTURE, |c| c.profile.picture)),
            driver_name: bytes(&self.player_name),
            car_name,
            filename: bytes(&entry.model),
            specname: bytes(if cheat { "993coupe36" } else { &entry.sim }),
            aispecname: bytes(if cheat { "993" } else { &entry.sim_family }),
            soundname: bytes(&entry.sound_set),
            bodystyle: entry.variant,
            transmission: u32::from(self.player_automatic()),
            colour,
            traction: self.mission.as_ref().is_some_and(|m| m.index == 0xE),
            // 0x4EF02A…0x4EF0F5: `Camera.1 … Camera.4` through slot 10 (`[vtable + 0x28]`)
            camera: self.fe.camera_list.map(|v| v as u32),
            colorconfig: [dword(paint.exterior), dword(paint.stripe1), dword(paint.stripe2), dword(paint.interior)],
            package: paint.package,
            racenumber: self.fe_car_decal(Some("zMergedList.Car")).map_or(-1, i32::from),
            setup,
            dash: entry.dash.map(f32::to_bits),
            parts,
            tiny: self.cheat_bits & (1 << 1) != 0,
            glass: self.cheat_bits & (1 << 2) != 0,
            abs_accessor: index("Car.ABS"),
            traction_accessor: index("Car.Traction"),
            // 0x4F1E40 forces `Car.Roof` to 0 on a car without the convertible flag
            roof_accessor: if entry.cabrio { index("Car.Roof") } else { 0 },
        })
    }

    /// `festrings` 814 — "%s picked %s. You need to download this car" — for the first player
    /// whose car is not here (0x50A433…0x50A48F; the car name is 0x4F4360(id)+2, the first record
    /// when the id is unknown).
    fn net_missing_car_notice(&mut self) -> bool {
        let Some(n) = self.net.session.as_ref() else { return false };
        let catalog = self.fe_car_catalog();
        let missing = n.room.players.iter().find_map(|p| {
            let c = usize::try_from(p.id).ok().and_then(|i| n.slots.slot.get(i))?.car_id;
            (c > 0 && !catalog.iter().any(|e| i32::from(e.id) == c)).then(|| (p.name.clone(), c))
        });
        let Some((name, _)) = missing else { return false };
        let who: String = name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let car = catalog.first().map(|e| e.display_name.clone()).unwrap_or_default();
        let fmt = self.fe.strings.get(814).unwrap_or_default().to_string();
        let text = nfs_net::room::format(&fmt, &[&who, &car]);
        self.fe_raise_dialog_text(nfs_game::dialog::DialogKind::One, text, [0, 0], DialogFor::Notice);
        true
    }

    fn on_lobby_screen(&self) -> bool {
        self.vehicle.is_none()
            && self.fe.layout_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("creategame") || n.eq_ignore_ascii_case("joingame"))
    }

    /// One tick of the lobby screen's own logic, after the session has been drained.
    ///
    /// * **First tick with the connection up** — 0x509290 (host) / 0x50A4F0 (joiner), each guarded by
    ///   `[this+0x285]`: the host sets its own `.READY` to 1 (0x509342…) and sends the car's records
    ///   and `(0x255, id, 1)`; the joiner sends `(0x255, id, 0)` and then the car's records.
    /// * **Afterwards** — a new selection of `zMergedList.Car` sends its records again (0x4BC240 while
    ///   0x4B7F70 holds: mode 1 and a live connection). The original sends on every call; the port
    ///   compares the car with what it last sent (**APPROXIMATION**: the repeats of an unchanged
    ///   value are not sent).
    /// * The joiner's tick 0x50A6A8…0x50A7A3: a player whose car this install lacks, while we are
    ///   ready — notice 814, `.READY` back to 0, `(0x255, id, 0)`.
    pub(crate) fn net_lobby_step(&mut self) {
        if !self.on_lobby_screen() {
            return;
        }
        let Some((me, host)) = self.net.session.as_ref().filter(|n| n.established()).and_then(|n| Some((n.my_slot()?, n.is_host()))) else { return };
        let car = self.net_own_car();
        let first = !self.net.lobby_sent;
        let changed = !first && car.as_ref().map(|c| (c.0, c.1.clone())) != self.net.lobby_car;
        if first || changed {
            let mut recs: Vec<Vec<u32>> = Vec::new();
            if first && !host {
                recs.push(slots::ready_record(me as u32, false));
            }
            if let Some((id, name, colour)) = &car {
                recs.extend(slots::car_records(me as u32, *id, name, *colour));
            }
            if first && host {
                recs.push(slots::ready_record(me as u32, true));
            }
            if let Some(n) = self.net.session.as_mut() {
                n.send_slot_records(&recs);
            }
            self.net.lobby_sent = true;
            self.net.lobby_car = car.map(|c| (c.0, c.1));
            self.bind_lists();
        }
        // the rows' draw asks 0x4905A0 for every player, every frame (0x4BC04D)
        if let Some(n) = self.net.session.as_mut() {
            n.ping_step();
        }
        if !host {
            let ready = self.net.session.as_ref().is_some_and(|n| n.slots.slot[me].ready);
            if ready && self.fe.dialog.is_none() && self.net_missing_car_notice() {
                if let Some(n) = self.net.session.as_mut() {
                    n.send_slot_records(&[slots::ready_record(me as u32, false)]);
                }
                self.bind_lists();
            }
        }
    }

    /// **The join screen's destructor 0x50A260** (vtable 0x5B90A0): with the link up (`[0x628C70]`,
    /// `net+8`, `+0xBF == 0`, `+0xBE != 0`) and our player id ≥ 0 (0x4EDCD0) it sends the record
    /// `(0x255, id, 0)` — `.READY` cleared — through 0x48D550, whichever way the screen goes (the race
    /// starting, or the screen replaced); a session already torn down by the BACK (0x48E6F0 runs
    /// first) sends nothing. So a joiner who was READY is not READY when the lobby comes back after
    /// the race, and the host's PLAYGAME gate (`everyone_ready`) closes with it.
    pub(crate) fn net_join_screen_gone(&mut self) {
        let Some((me, host)) = self.net.session.as_ref().filter(|n| n.established()).and_then(|n| Some((n.my_slot()?, n.is_host()))) else { return };
        if host {
            return;
        }
        tracing::info!(target: "netplay", me, "join screen's destructor 0x50A260: (0x255, id, 0)");
        if let Some(n) = self.net.session.as_mut() {
            n.send_slot_records(&[slots::ready_record(me as u32, false)]);
        }
    }

    /// **PLAYGAME's slot 53, 0x50CE90**, on a network screen — the create screen's press (once
    /// 0x5091B0 holds) and the join screen's tick once the room's `0xB3` has cleared `[net+0xBC]`
    /// (0x50A8C3…0x50A8E1). Read to the instruction:
    ///
    /// 1. the re-entry latch `[this+0x278]` (0x50CEA0, 0x50CEAE) — kept by the call not being
    ///    re-entered;
    /// 2. with the connection up (`net+8`, `+0xBF == 0`, `+0xBE`, `+0xDC ≥ 0`, 0x50CEB5…0x50CEF0) the
    ///    own `.READY` is saved (0x50CF0C `setne bl`); a machine whose id is not 0 clears it and sends
    ///    `(0x255, id, 0)` (0x50CF20…0x50CF6B); then **0x48D370**, the `0xB3` request (0x50CF73);
    /// 3. the wait for `+0xC4` — the room's `0xB3` (0x50CF78…0x50CFF1); a lost link, or ESC, aborts:
    ///    `+0xC5 = 1`, `+0xBC = 1`, `+0xC4 = 0`, `0xB7 (−2, −1)` (0x50D01E…0x50D04D);
    /// 4. the barrier 0x48CDF0 (0x50D07A);
    /// 5. a machine that was ready checks every room player's car, `.CAR > 0` and 0x4F4450 (0x50D0C5…
    ///    0x50D0E9): the first one this install lacks raises 814 (0x50D107) and clears `bl`;
    /// 6. the host with `bl` clear (0x50D169…0x50D17B) sets `bl = 1` **and clears the latch**;
    /// 7. `.READY = bl`, sent as `(0x255, id, bl != 0)` (0x50D194…0x50D1DB);
    /// 8. the barrier again (0x50D1FE);
    /// 9. any room player whose `.READY` is 0 clears the latch (0x50D230…0x50D251);
    /// 10. a cleared latch is the abort (0x50D28E…0x50D2EE: the same `0xB7 (−2, −1)`); otherwise the
    ///     launch 0x4D5730 runs (0x50D274).
    ///
    /// Without a live connection step 2 to 9 are skipped and the launch runs (0x50CEBF `je 0x50D25E`).
    /// Answers whether the launch runs. Step 5's dialog is **not modal**: 0x50D14C calls slot 13 of
    /// `FEDialogOneItemSelfDeleting` (vtable 0x5BA13C), **0x5128A0 = raise** (out of the global dialog
    /// stack `[0x69A05C]`, on its end, `[+0xF0] = TIMER_getTicks()`, `[+0xD8] = 1`, the manager told, 0x4D2000),
    /// which returns at once — the call goes on, as the port's does (read NM, 2026-09-30; until then this said
    /// "the original's is modal"). ESC in the waits: [`crate::netplay::KeyRing`].
    pub(crate) fn net_playgame(&mut self) -> bool {
        let Some((me, host)) = self.net.session.as_ref().filter(|n| n.established()).and_then(|n| Some((n.my_slot()?, n.is_host()))) else {
            return true;
        };
        let mut ready = self.net.session.as_ref().is_some_and(|n| n.slots.slot[me].ready);
        if !host {
            if let Some(n) = self.net.session.as_mut() {
                n.send_slot_records(&[slots::ready_record(me as u32, false)]);
            }
        }
        let Some(n) = self.net.session.as_mut() else { return true };
        n.request_start();
        if !n.wait_released() {
            n.abort_race();
            return false;
        }
        if !n.launch_barrier() {
            n.abort_race();
            return false;
        }
        if ready && self.net_missing_car_notice() {
            ready = false;
        }
        let mut latch = true;
        if host && !ready {
            ready = true;
            latch = false;
        }
        let Some(n) = self.net.session.as_mut() else { return true };
        n.send_slot_records(&[slots::ready_record(me as u32, ready)]);
        if !n.launch_barrier() {
            n.abort_race();
            return false;
        }
        if n.room.players.iter().any(|p| !usize::try_from(p.id).ok().and_then(|i| n.slots.slot.get(i)).is_some_and(|s| s.ready)) {
            latch = false;
        }
        if !latch {
            tracing::info!(target: "netplay", "PLAYGAME: not every player is ready (or a car is missing) — back to the lobby (0xB7 −2)");
            n.abort_race();
            self.bind_lists();
        }
        latch
    }

    /// **The network race's start, after the load** — the session loop's per-pass 0x4B6DAA…0x4B6E05
    /// runs the barrier twice (0x4B6DE4, 0x4B6E00) and the race frame's set-up 0x4152E0 (game type 3,
    /// 0x415300…0x41532F) a third time, then sends the `0xC5` request while `net+0xC5` is clear
    /// (0x4153AE, 0x48D3A0) and pumps until the room's first `0xC5` has cleared `+0xC4` (0x4153BF…
    /// 0x4153F0: `while (up && +0xC4) 0x4B0FA0`). Only for a race a launch numbered (`net+0xE5` set,
    /// glue: a `--net` run that never launched has none of it).
    pub(crate) fn net_race_go(&mut self) {
        let Some(n) = self.net.session.as_mut().filter(|n| n.established() && n.in_race != 0) else { return };
        for _ in 0..3 {
            if !n.launch_barrier() {
                return;
            }
        }
        // NG: the exchange's ring and timer restart here (0x415E00, 0x41537B), before the 0xC5 request
        n.exchange_race_frame();
        n.request_go();
        n.wait_go();
    }

    /// **The barrier at the end of the race load, 0x4B68C4** — after step 9 (0x414E74) and before step
    /// 10 (0x4B68CB): 0x48CDF0, whose waits call 0x4A4A70(−1) each turn (0x48CE27, 0x48CF5F,
    /// 0x48D079, 0x48D200) — `[0x655A28]` is still set, so the loading screen redraws the other
    /// players' rows while it waits. The redraw is paced (glue: the original redraws on every turn of
    /// its own pump, which also waits for the vertical retrace).
    pub(crate) fn net_load_barrier(&mut self, renderer: &mut Renderer) {
        let Some(mut run) = self.net.session.as_ref().filter(|n| n.established() && n.in_race != 0).and_then(|n| n.barrier_begin()) else { return };
        let mut last = Instant::now() - Duration::from_millis(100);
        loop {
            let Some(mut n) = self.net.session.take() else { return };
            let done = n.barrier_poll(&mut run);
            let _ = n.poll_lobby();
            self.net.session = Some(n);
            if let Some(passed) = done {
                if passed {
                    tracing::info!(target: "netplay", "launch barrier passed (0xBD ×4)");
                }
                return;
            }
            if last.elapsed() >= Duration::from_millis(50) {
                self.race_loading_frame(renderer, -1);
                last = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// **`READY`** — 0x50A300 (registered at 0x50A1A5 through 0x50D760). With a live connection and
    /// the lobby still open (`[net+0xBC] != 0`, which the host's `0xB3` clears, 0x48F853): a player
    /// whose car is not here raises 814 and nothing else happens (0x50A389…0x50A48F); else our
    /// `.READY` flips (`esi = 1`, or 0 when it was set, 0x50A3A2…0x50A3BF) and `(0x255, id, esi)`
    /// goes out (0x50A3F6…0x50A41E).
    pub(crate) fn net_ready_callback(&mut self) {
        let Some((me, open)) = self.net.session.as_ref().filter(|n| n.established()).and_then(|n| Some((n.my_slot()?, n.in_race == 0))) else { return };
        if !open {
            return;
        }
        if self.net_missing_car_notice() {
            return;
        }
        let Some(n) = self.net.session.as_mut() else { return };
        let now = !n.slots.slot[me].ready;
        n.send_slot_records(&[slots::ready_record(me as u32, now)]);
        tracing::info!(ready = now, "READY (0x50A300)");
        self.bind_lists();
    }

    /// **0x5091B0** — the create screen's PLAYGAME gate: every player ready, nobody in the race.
    pub(crate) fn net_can_start(&self) -> bool {
        self.net.session.as_ref().is_some_and(|n| n.everyone_ready())
    }

    /// `Comms.PlayerList`'s rows as **0x4BBEB0** draws them: with the connection up, the row number
    /// (`%d`, centred at `ID_NAMEPOS − 4`), the name (left, `+4`) and — for a row that has a player
    /// — the ping when known, the car's colour as a 15 × 10 swatch at `ID_CARPOS + 4`, the car's
    /// name at `ID_CARPOS + 0x17`, and the status at `ID_STATUSPOS + 4`: FE 283 for a player in the
    /// race (`net+0xE5`), else 280 when `.READY`, else 284.
    ///
    /// The ping is `min(player+0x54, 9999)` right-aligned at `ID_CARPOS − 4` when above −1
    /// (0x4BC04D…0x4BC098: the `%d` cell is drawn with alignment 2), `player+0x54` being what the
    /// `0xC3` exchange filled ([`netplay::NetPlay::ping_step`], `network.md` §11.11).
    pub(crate) fn net_player_rows(&self) -> Option<(Vec<ListRow>, Option<usize>)> {
        let empty = self.fe.strings.get(287).unwrap_or_default().to_string();
        let Some(n) = self.net.session.as_ref() else {
            return Some((vec![ListRow::Text(empty); nfs_net::room::MAX_PLAYERS], None));
        };
        let cell = |anchor: &str, dx: f32, text: String, align: Option<u8>| ListCell { anchor: anchor.into(), dx, text, fade: 0xFF, align };
        let rows = n
            .players(&empty)
            .into_iter()
            .enumerate()
            .map(|(i, (name, id))| {
                if !n.established() {
                    return ListRow::Columns(Vec::new());
                }
                let mut cells = vec![cell(NAMEPOS, -4.0, format!("{}", i + 1), Some(1)), cell(NAMEPOS, 4.0, name, Some(0))];
                let mut swatch = None;
                if let Some(p) = n.ping_shown(id) {
                    cells.push(cell(CARPOS, -4.0, format!("{p}"), Some(2)));
                }
                if let Some(s) = usize::try_from(id).ok().and_then(|k| n.slots.slot.get(k)) {
                    swatch = Some((CARPOS.to_string(), 4.0, colour_rgba(s.colour)));
                    let car: String = s.car_name.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
                    cells.push(cell(CARPOS, 23.0, car, Some(0)));
                    let status = if n.in_race & (1u8 << id) != 0 {
                        283
                    } else if s.ready {
                        280
                    } else {
                        284
                    };
                    cells.push(cell(STATUSPOS, 4.0, self.fe.strings.get(status).unwrap_or_default().to_string(), Some(0)));
                }
                ListRow::Racer(cells, swatch)
            })
            .collect();
        Some((rows, n.player_sel))
    }

    /// `fe_raise_dialog` with a text already built (the computed strings, 814).
    pub(crate) fn fe_raise_dialog_text(&mut self, kind: nfs_game::dialog::DialogKind, text: String, labels: [u32; 2], purpose: DialogFor) {
        if self.fe.dialog.as_ref().is_some_and(|(d, p)| *p == purpose && !d.gone()) {
            return;
        }
        let now = self.fe.layout_preview.as_ref().or(self.fe.layout_bottom.as_ref()).map(|l| l.ticks()).unwrap_or(0);
        let mut d = nfs_game::dialog::Dialog::new(kind, text, labels);
        d.raise(now);
        self.fe.dialog = Some((d, purpose));
    }

    /// **The comms screens' ticks** — what each one does after the base tick 0x509550 / 0x509FA0 (the comm-state
    /// notices, [`Self::comm_tick`]). Every rule is `0x50D880(name, enable)`: the widget by its `.lay` name, enabled
    /// or not (`set_disabled_flag`).
    ///
    /// * **`local`** 0x50AB40 (vtable 0x5B9184): `ID_COMMSJOINGAME` ⇔ `Comms.GamesList`'s count (slot 9) > 0;
    /// * **`udpip`** 0x50B760 (0x5B9508): `ID_COMMSCREATEGAME` ⇔ the link is up (`net+8`, `+0xBF == 0`, `+0xBE`),
    ///   `ID_COMMSJOINGAME` ⇔ the link is up and the count > 0;
    /// * **`serverlist`** 0x50B4E0 (0x5B9428): `ID_COMMSENTERSERVER` ⇔ the link is up and the count > 0 (and the
    ///   lobby-service reconnect when `+0xC0 == 0` and `[net+0xE0] > [0x5DEB48]·5`, which has no LAN equivalent);
    /// * **`PeerToPeerIP`** 0x50B270 (0x5B9348): no button rule — with the link up (`established()`), `[this+0x288]`
    ///   (CONNECT's latch) set and `[this+0x289]` clear, `0x4D2100("joingame", 0, 1)` and `[+0x289] = 1`;
    /// * **`modem`** 0x50AEF0 (0x5B9268): `ID_MODEMCREATEGAME` and `ID_DIALBUTTON` ⇔ `Comms.ModemList` is **not empty**
    ///   (slot 0x6C = 0x4B8B50, `begin == end`, tested `== 0`); the screen opens `joingame` when linked and DIAL's
    ///   latch `[+0x294]` is set (`[+0x295]`).
    ///
    /// **Platform glue:** the port's LAN browse has no connection object to wait for, so "the link is up" is the
    /// browsing socket being open (`Lobby::open` succeeded) or a session being established; and `Comms.ModemList`
    /// is empty — there is no modem device (the connection layer's enumeration 0x48E1A0 kind 2 lists TAPI / serial
    /// devices), so both modem buttons are disabled, exactly what the original's tick gives a machine without one.
    pub(crate) fn net_browse_step(&mut self) {
        if self.vehicle.is_some() || self.fe.layout_preview.is_none() {
            return;
        }
        let Some(name) = self.fe.layout_name.clone() else { return };
        let count = self.net.lobby.as_ref().map_or(0, |l| l.rows().len());
        let link = self.net.lobby.is_some() || self.net.session.as_ref().is_some_and(|n| n.established());
        // the buttons, by name: (widget name, enabled)
        let rules: Vec<(&str, bool)> = match name.to_ascii_lowercase().as_str() {
            "local" => vec![("ID_COMMSJOINGAME", count > 0)],
            "udpip" => vec![("ID_COMMSCREATEGAME", link), ("ID_COMMSJOINGAME", link && count > 0)],
            "serverlist" => vec![("ID_COMMSENTERSERVER", link && count > 0)],
            // `Comms.ModemList` — no device on this machine
            "modem" => vec![("ID_MODEMCREATEGAME", false), ("ID_DIALBUTTON", false)],
            _ => Vec::new(),
        };
        if !rules.is_empty()
            && let Some(l) = self.fe.layout_preview.as_mut()
        {
            let names: Vec<&str> = rules.iter().map(|r| r.0).collect();
            for (index, k) in l.widgets_named(&names) {
                l.set_disabled_flag(index, !rules[k].1);
            }
        }
        // 0x50B270
        if name.eq_ignore_ascii_case("peertopeerip")
            && self.net.connect_pending
            && !self.net.connect_opened
            && self.net.session.as_ref().is_some_and(|n| n.established())
        {
            self.net.connect_opened = true;
            self.fe_open("joingame", 0, true);
        }
    }

    /// **`accountlogin.lay`'s three buttons** (`FEMainScreen`; the lobby service they speak to no longer exists:
    /// `porsche-entry.earacing.com` and `login.earacing.com` answer NXDOMAIN, `network.md` §11.15).
    ///
    /// * `CLEARACCOUNTINFO` 0x50DD00: slot 0x44 of the `COMMS.ACCOUNTID` and `COMMS.PASSWORD` accessors — their text
    ///   is emptied;
    /// * `SIGNON` 0x5109B0: `net+0x58 = 99`, the two texts into `net+0x59` / `+0x98`, the profile name into `+0xA8`
    ///   and **`0x48E1A0(net, 1, 0, host)`** with the registry's `Primary Address` (0x5099A0, default
    ///   `porsche-entry.earacing.com`); `[this+0x294] = 1`. The connect goes into comm state 3 — the base tick shows
    ///   817 «Connecting…» — and, the host being unreachable, ends in state 4 → 830 (`comm_tick`). **APPROXIMATION:**
    ///   the original's answer for an *unresolvable* host (whether the enumerate 0x5803E0 fails at once or runs into
    ///   the connect's 16 s deadline) is not traced; the port waits the deadline, like an unanswered address;
    /// * `REGISTERUSER` 0x510990: `[0x655A4C] = 1` and `0x4D56E0(front end, 0)` — `[fe+0xF4] = 1`, `MusicStop(1500)`
    ///   (0x4AE3C0): the same exit the update question's yes (0x4A5960) and `QUIT` (0x50CC30) take — and the loop's
    ///   end, 0x4D2F1A → **0x4A5EA0**, `ShellExecute("open", RegUrl)` (`http://login.earacing.com` when the registry
    ///   has none, as this install's has not). **Platform glue:** `open` on macOS, and not in a scripted
    ///   (`NFS_NO_FOCUS`) run.
    pub(crate) fn account_callback(&mut self, cb: &str, event_loop: &ActiveEventLoop) {
        match cb {
            "CLEARACCOUNTINFO" => {
                if let Some(l) = self.fe.layout_preview.as_mut() {
                    l.clear_input("COMMS.ACCOUNTID");
                    l.clear_input("COMMS.PASSWORD");
                }
            }
            "SIGNON" => {
                let who = self.player_name.to_string();
                match netplay::NetPlay::start("signon:", &who) {
                    Ok(n) => {
                        tracing::info!("SIGNON (0x5109B0): 0x48E1A0(net, 1, 0, porsche-entry.earacing.com) — the lobby service is gone");
                        self.net.session = Some(n);
                        self.net.join_notified = false;
                    }
                    Err(e) => tracing::warn!(error = %e, "SIGNON: no socket"),
                }
            }
            _ => {
                let url = "http://login.earacing.com";
                tracing::info!(url, "REGISTERUSER (0x510990): the front end exits and the registration page is opened (0x4A5EA0)");
                #[cfg(target_os = "macos")]
                if !NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = std::process::Command::new("open").arg(url).spawn();
                }
                event_loop.exit();
            }
        }
    }

    /// **0x509550**, the comm state machine's notices, run from every comms screen's tick. The state
    /// is `[net+0x24]` ([`netplay::NetPlay::comm_state`]):
    ///
    /// | state | 0x509550 | notice |
    /// |---|---|---|
    /// | 3 | 0x5095A5: `0x512F70(0x331)` kept up while the state holds; its button ends the connect (teardown 0x48E9C0) | 817 |
    /// | 4, 12 | `0x513440(0x33E)`, raised, teardown | 830 |
    /// | 6 | 0x33F | 831 |
    /// | 7 | 0x333 | 819 |
    /// | 8 | the input dialog `0x514240(0x335)` prefilled with the name (0x514960); an answer with text sets the name and reconnects `0x48E1A0(1, 0, name)` (0x509905…0x509978); an empty answer re-arms it (0x50) | 821 |
    /// | 9 | 0x332 | 818 |
    /// | 10 | 0x336 | 822 |
    /// | 11 | 0x334 | 820 |
    ///
    /// Each error state then tears the session down (0x48E9C0) and the screen goes back (the
    /// screen's slot 0xB4, or 0x4D23F0(0)) — here the dialog's answer does both (`DialogFor::NetFail`).
    pub(crate) fn comm_tick(&mut self) {
        use nfs_net::room::comm_state as cs;
        if self.vehicle.is_some() || self.fe.layout_preview.is_none() {
            return;
        }
        let Some(st) = self.net.session.as_ref().filter(|n| !n.is_host()).map(|n| n.comm_state()) else { return };
        let kept = matches!(self.fe.dialog, Some((_, DialogFor::NetConnecting)));
        if st == cs::CONNECTING {
            if self.fe.dialog.is_none() {
                self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, 817, [0, 0], DialogFor::NetConnecting);
            }
            return;
        } else if kept {
            // `else if [0x699DDC] != NULL` (0x50960B): the dialog goes when the state leaves 3
            self.fe.dialog = None;
        }
        if self.net.join_notified || self.fe.dialog.is_some() {
            return;
        }
        let notice = match st {
            cs::LOST | cs::LOST_2 => 830,
            cs::COULD_NOT_CONNECT => 831,
            cs::BAD_LOGIN => 819,
            cs::EJECTED => 818,
            cs::WRONG_VERSION => 822,
            cs::FULL => 820,
            cs::NAME_TAKEN => {
                tracing::info!("comm state 8 (name taken): the input dialog 821");
                self.net.join_notified = true;
                let now = self.fe.layout_preview.as_ref().map(|l| l.ticks()).unwrap_or(0);
                let text = self.fe.strings.get(821).unwrap_or_default().to_string();
                let mut d = nfs_game::dialog::Dialog::input(text, false, 15);
                d.raise(now);
                self.fe.dialog = Some((d, DialogFor::NetName));
                return;
            }
            _ => return,
        };
        tracing::info!(state = st, notice, "comm state notice (0x509550)");
        self.net.join_notified = true;
        self.fe_raise_dialog(nfs_game::dialog::DialogKind::One, notice, [0, 0], DialogFor::NetFail);
    }

    /// **`SETMODENET`'s update check** (0x510D80 → 0x4A56E0 on the object at 0x655A30; `network.md` §11.22).
    ///
    /// The handler builds the one-button dialog `0x512F70(0x4E, 0)` = **78** «Checking for updates…» into
    /// `[+0x29C]`, raises it (slot 0x34), sets `[+0x296] = 1`, and 0x4A56E0 — `fe.txt`'s `castanet=1` — sets the
    /// thread state `[obj+0x2F4] = 2` and starts the thread 0x4A5750 (`mrbupd.dll`: `mrbInitUpdate`, prepare /
    /// repair / update-available checks, then state 0 / 1 / 3). The button destination (`accountlogin`) opens as
    /// usual, the dialog over it.
    ///
    /// The thread's answer on this install (`.castanet\properties.txt` + `index.mrb`, no `undo.mrb` /
    /// `holding.mrb`) is **state 0** whatever the transmitter does (§11.21 item 6); the port never contacts
    /// `trans.ea.com`. **APPROXIMATION (instant):** the thread's duration — in the original the DLL's connect to
    /// `trans.ea.com:80` (a black hole, OS-dependent: seconds) — is here the case of a machine whose connect is
    /// refused at once: the thread is done before the second tick after the click (`UPDATE_THREAD_TICKS` = 1 tick
    /// still seen as running: the thread starts in the click's frame, and its file checks and the refused connect
    /// take more than the rest of that frame). `NFS_UPDATE_CHECK_TICKS=<n>` (test glue) runs it `n` ticks longer.
    /// **`SETMODENET`'s update check** (`network.md` §11.22). The handler 0x510D80 (inline in `layout_key`'s
    /// `SETMODENET` arm) builds the one-button dialog `0x512F70(0x4E, 0)` = **78** «Checking for updates…» into
    /// `[+0x29C]`, raises it (slot 0x34), sets `[+0x296] = 1`, and 0x4A56E0 — `fe.txt`'s `castanet=1` — sets the
    /// thread state `[obj+0x2F4] = 2` and starts the thread 0x4A5750 (`mrbupd.dll`); the button's destination
    /// (`accountlogin`) opens as usual, the dialog over it. The thread's answer on this install
    /// (`.castanet\properties.txt` + `index.mrb`, no `undo.mrb` / `holding.mrb`) is **state 0** whatever the
    /// transmitter does (§11.21 item 6); the port never contacts `trans.ea.com`.
    ///
    /// **APPROXIMATION (instant):** the thread's duration — in the original the DLL's connect to `trans.ea.com:80`
    /// (a black hole, OS-dependent: seconds) — is here the case of a machine whose connect is refused at once: the
    /// thread is still running at the click frame's own tick and done at the next ([`UPDATE_THREAD_TICKS`] = 1).
    /// `NFS_UPDATE_CHECK_TICKS=<n>` (test glue) runs it `n` ticks instead.
    ///
    /// **`FEMainScreen`'s tick 0x5103F0, the update half** (0x510461…0x510493): `0x4A5930` reads the thread's
    /// state; while it is **2** the dialog stays (its button: [`DialogFor::UpdateCheck`]); any other state deletes
    /// it at once (`[+0x29C] = 0`) — and state 0, this install's answer, does nothing more (state 3 would raise
    /// the 0x2BE dialog, state 1 the question 0x32C: neither is reachable here, `network.md` §11.22).
    pub(crate) fn update_check_tick(&mut self) {
        let Some((left, restamp)) = self.fe.update_check else { return };
        if self.vehicle.is_some() || self.fe.layout_preview.is_none() {
            return;
        }
        if restamp {
            // the screen's clock restarted when `accountlogin` opened; the dialog's fade counts from now
            let now = self.fe.layout_preview.as_ref().map(|l| l.ticks()).unwrap_or(0);
            if let Some((d, DialogFor::UpdateCheck)) = self.fe.dialog.as_mut() {
                d.raise(now);
            }
        }
        if left > 0 {
            self.fe.update_check = Some((left - 1, false));
            return;
        }
        // state 0
        self.fe.update_check = None;
        if matches!(self.fe.dialog, Some((_, DialogFor::UpdateCheck))) {
            self.fe.dialog = None;
        }
    }
}
