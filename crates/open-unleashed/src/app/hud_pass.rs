//! The race HUD's per-frame draw (0x425950, `RE/notes/hud*.md`): one method per element, in the
//! original's order, and the port's dialogs drawn over the race after it (`rebuild_scene`).

use crate::*;

/// `HGear`'s forward gears 1…7 (gear bytes 2…8), as `_itoa` prints them.
const GEAR_DIGITS: [&str; 7] = ["1", "2", "3", "4", "5", "6", "7"];

/// `HPosition`'s rows `(car, colour, name, value)`, the player's place and the racers' link badges
/// ([`App::opponent_rows`], drawn by `Hud::opponent_list_sprites`).
type OpponentRows = (Vec<(usize, [f32; 4], String, String)>, usize, Vec<Option<LinkBadge>>);

impl App {
    /// The HUD's per-frame draw 0x425950 (`RE/notes/hud*.md`): the sprites of every tray, billboard,
    /// dialog and counter laid into `scene.sprites` from `hud_sprites_from` on. Returns where the HUD's
    /// sprites start and the dialog button under the pointer, which `rebuild_scene` settles after the
    /// unanchored text is resolved.
    pub(super) fn hud_pass(&mut self) -> (usize, Option<Option<(u8, usize)>>) {
        // the HUD is a 640×480 layout; on a wider window its elements are anchored to the window's
        // edges as 0x41E480 does (`Hud::anchor`), not stretched
        let hud_aspect = self.hud_aspect();
        // the dialog button under the pointer, when a dialog with buttons is drawn this frame
        let mut hud_hover: Option<Option<(u8, usize)>> = None;
        // everything from here to the pause menu is the HUD's per-frame draw 0x425950, which
        // `[0x60CA00]` skips whole (0x425950…0x42595D; `hud_hidden`)
        let hud_sprites_from = self.scene.sprites.len();
        PENDING_TEXT.with(|p| p.borrow_mut().clear());
        let wrong_way = self.wrong_way_step();
        let leader = self.leader_picture();
        let finish_close = self.hud.is_some() && self.vehicle.is_some() && self.hud_elements(hud_aspect, wrong_way, leader, &mut hud_hover);
        self.hud_cone_count();
        // the page that finished shrinking this frame makes way for the next (state 0)
        if finish_close {
            self.advance_results_page();
        }
        (hud_sprites_from, hud_hover)
    }

    /// **`HWrongWay`**'s gate and blink, one step per draw: whether the sign shows this frame.
    fn wrong_way_step(&mut self) -> bool {
        // `HWrongWay` is drawn only while `car+0xD30` is set past tick 448, outside a mission and
        // a replay (0x436F70's callers); each draw steps its blink. The replay test is `REPLAY
        // == 2` (0x425AFB), which a playback keeps through its exhausted frames — `REPLAY` turns
        // 3 only as the race loop is left (0x415CE2). *(Until 2026-09-27 the port tested
        // `Playing` only, so a stream run out showed the sign.)*
        let playing = self.playback_running();
        let up = self.race.as_ref().is_some_and(|r| !r.finished && r.wrong_way_ticks > 0)
            && self.tick > 448
            && self.mission.is_none()
            && !playing
            // `HWrongWay`'s draw returns at once under `DERBY [0x657400]` (0x436F70…0x436F80,
            // before the blink counters 0x436F86 — they hold)
            && self.cheat_bits & CHEAT_DERBY == 0;
        if up {
            let (count, last, visible) = &mut self.wrong_way_blink;
            *count += self.tick.saturating_sub(*last);
            if (*visible && *count > 0x40) || (!*visible && *count > 0x20) {
                *visible = !*visible;
                *count = 0;
            }
            *last = self.tick;
        }
        up && self.wrong_way_blink.2
    }

    /// **`HLeader`**'s picture, only in a network race.
    fn leader_picture(&mut self) -> Option<TextureId> {
        // `HLeader`'s picture: the car in first place (`car+0xB48 == 1`, 0x428717) and its
        // player's file (`netplay::NetPlay::leader_picture`) — only in a network race
        if self.net.session.is_some() && self.race.is_some() {
            let first = self.live_order().first().copied();
            match (self.net.session.as_mut(), self.renderer.as_mut(), first) {
                (Some(n), Some(r), Some(d)) => n.leader_picture(r, d.checked_sub(1)),
                _ => None,
            }
        } else {
            None
        }
    }

    /// The HUD's elements over a race with a car, in 0x425950's order (the trays, the post-race
    /// billboards, the counters); `true` when a results page finished shrinking this frame.
    fn hud_elements(&mut self, hud_aspect: f32, wrong_way: bool, leader: Option<TextureId>, hud_hover: &mut Option<Option<(u8, usize)>>) -> bool {
        if let Some(h) = &self.hud {
            h.display.set(Hud::display_colour(self.choice_index(choice::DISPLAY_DISP_COLS).map_or(0, |(i, _)| i)));
            // the trays' box: `HudKeys::boxes`, TRANSLUCENCY's at the session's start, F8 flips it
            h.boxes.set(self.hud_keys.boxes);
        }
        let opp_list = self.opponent_rows();
        // `REPLAY != 0` (0x418540…0x418547): 2 through a playback's exhausted frames too
        let in_replay_now = self.playback_running();
        let (time_penalty, time_alert) = self.time_penalty_step();
        // `SPEEDUNITS [0x657DC0]` (`HSpeed`, `hud_text`): 0 km/h, otherwise mph
        let mph = self.choice_index(choice::DISPLAY_SPEED_UNITS).is_some_and(|(i, _)| i != 0);
        // "wide screen" (0x4259B2 → 0x427B00): two opaque black bars over the full width,
        // `ftol(H·0.15 + 2)` high at the top and from `H − that` at the bottom, drawn under the
        // HUD elements (0x41A330 follows)
        if self.widescreen()
            && let Some(h) = &self.hud
        {
            // in the render target's pixels (design coordinates are pixels, shifted by the origin)
            let (el, et, er, eb) = Hud::edges();
            let b = ((eb - et) * 0.15 + 2.0).trunc();
            let black = [0.0, 0.0, 0.0, 1.0];
            self.scene.sprites.push(h.quad(hud_aspect, el, et, er - el, b, black));
            self.scene.sprites.push(h.quad(hud_aspect, el, eb - b, er - el, b, black));
        }
        let (live, clock, tray4, time_slide) = self.tray_slides();
        // (pure: built here, after the slides, so its borrow does not span `tray_slides`)
        let text = self.hud_text(mph);
        let (Some(h), Some(v)) = (&self.hud, &self.vehicle) else { return false };
        // `Display.ScreenLayout`'s current "Config N" preset (`SCREEN_LAYOUTS`, `0x41FB40`) —
        // the shipped file only has four at 640×480, so a fifth option in `CHOICE_ACCESSORS`
        // would have nothing to select; there is none (`main.rs`'s `("Display.ScreenLayout",
        // "screenlayout", &[1824, 1825, 1826, 1827])` is exactly four rows)
        let layout = &SCREEN_LAYOUTS[self.choice_index(choice::DISPLAY_SCREEN_LAYOUT).map_or(0, |(i, _)| i.min(3))];
        self.scene.sprites.extend(h.sprites(
            hud_aspect,
            layout,
            // `HSpeedometer`: `speed · 3.6` or `· 2.237` by `SPEEDUNITS` (`hud.md` §3.3)
            v.speed() * if mph { 2.237_413_9 } else { 3.6 },
            v.rpm,
            self.tick,
            self.race.as_ref(),
            // no countdown in a replay or in one of the three test drives (0x4184FC…0x41850E),
            // nor while a billboard is queued: 0x418514 calls 0x426A60 (`[0x60C9C4]` non-empty)
            // and returns before drawing anything — the pause menu hides it (audit 2026-09-27)
            !in_replay_now && self.fe.test_drive.is_none() && self.billboard_key == 0,
            time_penalty,
            time_alert,
            &text,
            wrong_way,
            tray4,
            time_slide,
            // `car+0xB24` once the player's state is 2: the finish sets it to `tick − 448` —
            // a race at 0x416643 / 0x416840 (the finish tick is `car+0xB14`, `RaceProgress::
            // tick`), a skill test at 0x416170 (`SkillTest::time_ticks`). A knocked-out player
            // (0x416649…) and a skill test's DNF are state 3 and keep the running clock
            match &self.skill {
                Some(sk) => sk.time_ticks.filter(|_| !sk.dnf),
                None => self
                    .race
                    .as_ref()
                    .filter(|r| r.finished && !self.player_eliminated)
                    .map(|r| r.tick.saturating_sub(RACE_START_TICK)),
            },
        ));
        self.hud_damage(hud_aspect, tray4, live);
        self.hud_mirror(hud_aspect, clock, live, layout);
        if let Some(h) = &self.hud {
            if let Some(tex) = leader {
                let m = &mut self.scene.sprites;
                let n0 = m.len();
                h.leader_sprites(m, hud_aspect, layout.leader, tex, self.fe.strings.hud(hud_str::LEADER).unwrap_or_default());
                let (lx, ly) = layout.leader;
                Hud::slide(m, n0, hud_aspect, (lx, ly, 59.0, 85.0), tray4);
            }
            if let Some((rows, place, links)) = &opp_list {
                let (px, py) = layout.position;
                let list = &mut self.scene.sprites;
                let n0 = list.len();
                let label = self.fe.strings.hud(17).unwrap_or_default();
                h.opponent_list_sprites(list, hud_aspect, (px, py), rows, links, *place, label, self.cheat_bits & CHEAT_DERBY != 0);
                Hud::slide(list, n0, hud_aspect, (px, py, 138.0, (rows.len() as f32 + 1.0) * 15.0 + 4.0), tray4);
            }
        }
        self.hud_replay_bar(hud_aspect, clock);
        self.hud_track_map(hud_aspect, clock, live, layout);
        self.hud_diagnostic_dialogs(hud_aspect);
        let finish_close = self.hud_result_pages(hud_aspect, hud_hover);
        self.hud_multi_time(hud_aspect, layout, time_slide);
        finish_close
    }

    /// **`HPosition`**'s rows (0x42B000): the listed cars by live position with their colours, names
    /// and value columns, the player's place, and the racers' link badges in a network race.
    fn opponent_rows(&mut self) -> Option<OpponentRows> {
        let v = self.vehicle.as_ref()?;
        // `HPosition` 0x42B000 — only with `OPPONENTLIST` on and `[0x60C970]` (0x41E880), never
        // in a replay (`REPLAY` 2 or 3, 0x42B010 / 0x42B01C; until 2026-09-27 the port tested
        // `Playing` only). It stays up past the player's finish: neither it nor 0x425950 tests
        // `car+0xB50` for the whole element (audit 2026-09-27; the port had dropped it at the
        // finish). The port used to print "POS p/N" (or "DNF") in the top tray instead.
        let opp_mode = self.choice_index(choice::DISPLAY_OPP_LIST).map_or(0, |(i, _)| i);
        let playing_back = self.playback_running();
        if opp_mode != 0
            && self.hud_keys.live
            && !playing_back
            && !self.road.opponents.is_empty()
            && let Some(r) = self.race.as_ref()
        {
            let order = self.live_order();
            let derby = self.cheat_bits & CHEAT_DERBY != 0;
            let metres = |d: usize| self.hud_metres.get(d).copied().unwrap_or(0);
            let p_speed = v.horizontal_speed();
            let mut rows = Vec::new();
            let mut links: Vec<(usize, Option<LinkBadge>)> = Vec::new();
            let game_tick = self.tick as u32;
            for &d in &order {
                let (progress, eliminated, name, paint, speed) = if d == 0 {
                    (r, self.player_eliminated, self.player_name.clone(), self.paint, p_speed)
                } else {
                    let o = &self.road.opponents[d - 1];
                    let paint = self.road.opponent_paints.get(d - 1).copied().unwrap_or([0x20, 0x40, 0xC0]);
                    (&o.progress, o.eliminated, o.name.clone(), paint, o.vehicle.horizontal_speed())
                };
                // Quick Knockout (game type 1) drops the knocked-out cars from the list and
                // closes it up (0x42B04E…0x42B06C, 0x42B1C9…0x42B1E0)
                if self.quick_knockout && eliminated {
                    continue;
                }
                let gap = metres(d) - metres(0);
                // FEGAME_TYPE 3 (a network race): the racer's race index, which 0x48D7F0 / 0x48D770
                // (0x42B366…0x42B38E) and 0x490140 (0x42B4DE) take as `i`
                let net_index = self.net.session.as_ref().and_then(|n| if d == 0 { n.local_race_index() } else { n.seat_race_index(d - 1) });
                // the other columns, 0x42B393…0x42B4AE: "position" (the gap in metres, "-----"
                // when level) and "time" (the gap over the faster-ish of the two speeds)
                let mode_column = || -> String {
                    if opp_mode == 1 {
                        // "position": the gap in metres, "-----" when level (0x42B39D…0x42B3E4)
                        if gap != 0 { gap.to_string() } else { "-----".to_string() }
                    } else {
                        // "time": the gap over the faster-ish of the two speeds — the leader's own,
                        // or half the other's (0x42B420…0x42B458), signed; "-----" when level or
                        // while the player is at 13.39 m/s or less (0x42B45B)
                        let vv = if gap > 0 { p_speed.max(0.5 * speed) } else { speed.max(0.5 * p_speed) };
                        if p_speed > 13.39 && gap != 0 {
                            let t = ((gap * 64) as f32 / vv).round_ties_even() as i32;
                            // 0x42B488: the `fistp` dword with the sign flag 1 (0x42B48A)
                            format_time_dword(t, true)
                        } else {
                            "-----".to_string()
                        }
                    }
                };
                let value = if derby {
                    // `DERBY [0x657400] == 1` (0x42B2D5, ahead of every other column; the
                    // game type 3 "DNF" before it is the network race's): the ten zones
                    // `car+0x4E4` added one at a time into an int, each step `_ftol`'d
                    // (0x42B2F2…0x42B30B), then `/ 10` (the 0x66666667 multiply, 0x42B30D…
                    // 0x42B321) and `_itoa(…, 10)` (0x5A19AC) — no "%"
                    let zones = if d == 0 { &v.damage } else { &self.road.opponents[d - 1].vehicle.damage };
                    derby_damage_total(zones).to_string()
                } else if let Some(i) = net_index {
                    // game type 3 (0x42B335 `cmp edx, 3`, `je 0x42B366`) — **ahead of the state-2
                    // arm, which it skips**: `0x48D7F0(i) > 0` (0x42B36A…0x42B374) prints the time
                    // 0x48D770(i) (0x42B37A) through `Time_format` 0x4356E0 with its sign flag 0
                    // (0x42B386): the finish time the racer's own `0xB7` carried. A racer with no
                    // time yet (`jle 0x42B393`) takes the "position" / "time" columns.
                    match self.net.session.as_ref().filter(|n| n.race_rank(i) > 0).map(|n| n.race_time(i)) {
                        Some(t) => format_ticks(t.max(0) as u64),
                        None => mode_column(),
                    }
                } else if progress.finished && !eliminated {
                    // state 2: the car's race time `car+0xB24` (0x42B33A…0x42B34C) — the finish tick
                    // less 448 (0x416840…0x41684C, `FinalizeIn::from_progress`), which the port had
                    // printed as the lap times' sum: 448 ticks (7 s) longer, the countdown included
                    format_ticks(progress.tick.saturating_sub(0x1C0))
                } else {
                    mode_column()
                };
                let colour = [paint[0] as f32 / 255.0, paint[1] as f32 / 255.0, paint[2] as f32 / 255.0, 1.0];
                // FEGAME_TYPE 3 (a network race): 0x490140 of the racer's race index → the badge,
                // and FE 18191 in place of the gap for a racer in state 3 (0x42B25E…0x42B27C)
                let link = self.net.session.as_mut().and_then(|n| {
                    let i = if d == 0 { n.local_race_index() } else { n.seat_race_index(d - 1) }?;
                    let c = n.link_quality(i, game_tick).colour();
                    let f = |sh: u32| ((c >> sh) & 0xFF) as f32 / 255.0;
                    Some(LinkBadge {
                        colour: [f(16), f(8), f(0), f(24)],
                        dnf: eliminated.then(|| self.fe.strings.hud(hud_str::DNF).unwrap_or_default().to_string()),
                    })
                });
                links.push((d, link));
                rows.push((d, colour, name, value));
            }
            // rows are laid out by live position among the cars still listed
            let listed: Vec<_> = rows
                .into_iter()
                .enumerate()
                .map(|(k, (d, c, n, val))| (d, (k, c, n, val)))
                .collect();
            let place = listed.iter().position(|(d, _)| *d == 0).map_or(0, |k| k + 1);
            Some((listed.into_iter().map(|(_, r)| r).collect::<Vec<_>>(), place, links.into_iter().map(|(_, l)| l).collect::<Vec<_>>()))
        } else {
            None
        }
    }

    /// `HTime`'s cone penalty and alert, then the cone flash's countdown (once per draw).
    fn time_penalty_step(&mut self) -> (i64, bool) {
        let Some(v) = self.vehicle.as_ref() else { return (0, false) };
        let penalty = match (&self.mission, &self.skill) {
            (Some(ms), Some(_)) => {
                let per = match ms.mission.cone_rule {
                    Some(nfs_formats::career::ConeRule::PenaltySeconds(s)) => s as i64,
                    _ => -1,
                };
                let over = nfs_game::skill::damage_over_limit(&ms.mission, &v.damage);
                // `HTime+0xE4` (the count's moves) × `CONEPENALTY` × 64 (0x43176A…0x4317A1)
                (self.hud_cones.changes as i64 * per * 64, over || self.cone_flash > 0)
            }
            (Some(ms), None) => (0, nfs_game::skill::damage_over_limit(&ms.mission, &v.damage)),
            _ => (0, false),
        };
        // 0x4317C9…0x4317E0: the flash counts down once per draw
        self.cone_flash = self.cone_flash.saturating_sub(1);
        penalty
    }

    /// The racing trays' texts (`HSpeed`, `HGear`, `HTime`, `HWrongWay`, `HTach`) and their Display choices.
    fn hud_text(&self, mph: bool) -> HudText<'_> {
        let Some(v) = self.vehicle.as_ref() else { unreachable!("the HUD's texts are built with a car") };
        let strings = &self.fe.strings;
        let fe = move |id: u32| strings.hud(id).unwrap_or_default();
        // `HSpeed` 0x42E593: `SPEEDUNITS [0x657DC0]` 0 → `_ftol(car+0xEC4 · 3.6)` under FE 11,
        // otherwise `· 2.2374139` under FE 20; "00" below 10, "0" below 100 (0x42E5E9…0x42E656),
        // and "000" outright below 1 m/s (0x42E66D). This port always showed km/h, and "001"…"003"
        // where the original shows "000".
        let ms = v.speed();
        HudText {
            speed: if ms < 1.0 {
                "000".to_string()
            } else {
                format!("{:03}", (ms * if mph { 2.237_413_9 } else { 3.6 }) as i32)
            },
            speed_label: fe(if mph { 20 } else { 11 }),
            gear: match v.gear {
                0 => fe(hud_str::GEAR_REVERSE),
                1 => fe(hud_str::GEAR_NEUTRAL),
                g @ 2..=8 => GEAR_DIGITS[g - 2],
                _ => fe(hud_str::GEAR_NEUTRAL),
            },
            gear_label: fe(10),
            time_label: fe(18),
            wrong_way: fe(hud_str::WRONG_WAY),
            speedometer: self.choice_index(choice::DISPLAY_SPEEDOMETER).map_or(2, |(i, _)| i),
            tachometer: self.choice_index(choice::DISPLAY_TACH).map_or(2, |(i, _)| i),
            // `fe.txt` has no `gear` line, so `GEAR [0x657DF0]` keeps the `nfs5-2.hud` Config 1
            // default, 1 (hud.md §2) — `choice_index` answers that default too
            gear_on: self.choice_index(choice::DISPLAY_GEAR).is_some_and(|(i, _)| i == 1),
            rpm_label: fe(hud_str::TACH_RPM),
            tach_scale: {
                let r = self.specs.redline as i32;
                if r != 0 { (r - 750) as f32 / r as f32 } else { 1.0 }
            },
        }
    }

    /// The trays' slide states for this draw: `[0x60C970]`, the HUD clock, the `+4` trays' slide and
    /// `HTime` / `HMultiTime`'s.
    fn tray_slides(&mut self) -> (bool, u64, TraySlide, TraySlide) {
        let Some(h) = &self.hud else { unreachable!("the trays slide with a HUD") };
        // **The trays' slide** (0x425950): the `+4` trays are drawn only past clock 0x1C0 (448,
        // 0x425A29 / 0x425BAB); with the billboard queue `[0x60C9C4]` empty and the clock past 448,
        // 0x425E20(0) puts every tray still in state 0 into state 1 (0x425CCA…0x425CE2, 0x425ED8),
        // and while a billboard is queued every tray in state 3 gets state 2 (0x425C86…0x425CC0).
        // A transition stamps `[0x606AC8]` (0x434490) and runs 11 ticks (slot 14, 0x4349F0).
        // `[0x60C970]` clear keeps 0x425E20 from opening a tray (0x425E55) and draws only one
        // sliding out (0x425A35…0x425A50: `live || state == 2`) — F5's hide (`hud_key`)
        let live = self.hud_keys.live;
        let clock = self.hud_clock();
        let slide = {
            let (st, stamp) = step_slide(self.tray_slide, clock, self.tick <= RACE_START_TICK, self.billboard_key != 0, live);
            self.tray_slide = (st, stamp);
            h.trays_in.set(st == 3);
            let st = if live || st == 2 { st } else { 0 };
            TraySlide { state: st, t: clock.saturating_sub(stamp).min(1 << 20) as i32 }
        };
        // **Playback** (`REPLAY [0x657424]` 2 or 3 — every playback, `replay.md`): the `+4` trays
        // are drawn only while `REPLAY` is 0 or 1 (0x425A52…0x425A5E), so none of them shows in
        // a replay — the port had drawn the dials, gear, time, record and damage over it (audit
        // 2026-09-27). In a mission (`[0x6573E8] == 9`, 0x425A87) `HTime` and `HMultiTime` are
        // the exception: each frame one still in state 0 is put in state 1 (0x425A8F…0x425AC4,
        // whatever the clock) and both are drawn (0x425AC7…0x425ADA); the pages do not slide
        // them out (0x425CA1…0x425CB0). Their state carries over from the live race, so an
        // automatic replay finds them in place and a `--replay` slides them in.
        let hidden = TraySlide { state: 0, t: 0 };
        let playback = self.playback_running();
        let (tray4, time_slide) = if !playback {
            self.time_trays = self.tray_slide;
            (slide, slide)
        } else if self.mission.is_some() {
            let (st, stamp) = step_slide(self.time_trays, clock, false, false, true);
            let t = clock.saturating_sub(stamp).min(1 << 20) as i32;
            self.time_trays = (st, stamp);
            // the rule is each tray's own state-3 test (0x43428D); only these two draw one here
            h.trays_in.set(st == 3);
            (hidden, TraySlide { state: st, t })
        } else {
            (hidden, hidden)
        };
        // `HTime`'s draw returns at once while `[0x60C970]` is clear (0x4316F4), sliding or not —
        // and `HMultiTime`'s gate 0x41E850 is `SPLITTIMES != 0 && [0x60C970]`: both vanish on
        // F5's hide where the other trays slide out
        let time_slide = if live { time_slide } else { hidden };
        (live, clock, tray4, time_slide)
    }

    /// `HGraphicDamage` / `HTextDamage` by `Display.Damage`.
    fn hud_damage(&mut self, hud_aspect: f32, tray4: TraySlide, live: bool) {
        let (Some(h), Some(v)) = (&self.hud, &self.vehicle) else { return };
        // `HGraphicDamage` with `Display.Damage` on its first choice (0x41E8B0 `== 1`), while the
        // racing trays are up — its gates are 0x41E8B0 and the state (0x42A3B0) only, so past
        // the player's finish too (audit 2026-09-27)
        if self.choice_index(choice::DISPLAY_DAMAGE).is_some_and(|(i, _)| i == 1)
            && self.race.is_some()
        {
            let over = self.mission.as_ref().is_some_and(|ms| nfs_game::skill::damage_over_limit(&ms.mission, &v.damage));
            let out = &mut self.scene.sprites;
            let n0 = out.len();
            h.damage_sprites(out, hud_aspect, &v.damage, ["0%", "100%", self.fe.strings.hud(12).unwrap_or_default()], over);
            Hud::slide(out, n0, hud_aspect, (0.0, 0.0, 127.0, 105.0), tray4);
        }
        // `HTextDamage` with `Display.Damage` on "bar graph" or "percentages" (0x41E8D0 `== 2 ||
        // (== 3 && [0x60C970])`), finished or not (0x430150 has no finish test)
        if let Some((mode @ (2 | 3), _)) = self.choice_index(choice::DISPLAY_DAMAGE)
            && (mode == 2 || live)
            && self.race.is_some()
        {
            let over = self.mission.as_ref().is_some_and(|ms| nfs_game::skill::damage_over_limit(&ms.mission, &v.damage));
            let strings = &self.fe.strings;
            let fe = move |id: u32| strings.hud(id).unwrap_or_default();
            let values = [
                v.engine_wear(&self.specs) as i32,
                v.steering_damage() as i32,
                v.body_damage() as i32,
                v.suspension_wear(&self.specs) as i32,
            ];
            let out = &mut self.scene.sprites;
            let n0 = out.len();
            h.text_damage_sprites(out, hud_aspect, values, [fe(13), fe(14), fe(15), fe(16), fe(12)], mode == 3, over);
            Hud::slide(out, n0, hud_aspect, (0.0, 0.0, 124.0, 100.0), tray4);
        }
    }

    /// **`HRearview`**: the mirror's tray, its slide and its view.
    fn hud_mirror(&mut self, hud_aspect: f32, clock: u64, live: bool, layout: &HudLayout) {
        let playback = self.playback_running();
        let Some(h) = &self.hud else { return };
        // `HRearview` (draw 0x42C740): the 240 × 84 tray at (185, 0) with the mirror's view filling
        // it (0x42C510 / 0x42C680 hand 0x4691D0 the tray's own rectangle). Audit 2026-09-26: the
        // tray is built with `+0xA0 = 1`, `+0xC0 = 1` (0x42C526…0x42C536), so its box shows only
        // while it slides (0x434182…0x4341B0); and after it the draw puts the **sunken 4-px
        // bevel** 0x435A20(x, y, w, h, `[0x60CB34]`, `[0x5CCE30]` = 4) round the element's
        // configured rectangle (0x420C10 / 0x420A70, 0x42C77A…0x42C7AC) — `[0x60CB34]` is in
        // `.bss` and has no writer, so the style is 0. It is not moved by the slide (the
        // configured position, not the tray's). The draw runs in any state but 0 (0x42C768).
        // This port drew the box always and no bevel.
        // It is a `+5` element (0x425B91…0x425BDD: past clock 448, `[0x60C970]` or state 2) with
        // its own state (`mirror_slide`), opened by 0x425E20 only while its slot 10 0x42C800 is
        // clear — `REARVIEWMIRRROR && [0x60C970]` — and only outside a playback (0x425CD2…
        // 0x425CDE); its draw needs `REARVIEWMIRRROR` or the state 1 / 2 (0x42C752…0x42C764,
        // so it still slides out after F6) and the state non-zero. The view is in the tray
        // only in state 3: the pass 0x468CA0 renders it on 0x4275A0 (`[0x60C970]` and state
        // 3), so a sliding mirror is its box alone.
        // **In a playback** (read 2026-09-27) the draw returns first thing when `REPLAY` is 2
        // **or 3** (0x42C740…0x42C750) — the mirror is not in any replay, whatever the `+5`
        // loop's gates (which do not test `REPLAY`). *(The port tested `Playing` only, so a
        // playback run off its stream's end, `Exhausted` = 3, drew the mirror again.)*
        let mirror_slide = {
            let (st, stamp) = step_slide(self.mirror_slide, clock, self.tick <= RACE_START_TICK, self.billboard_key != 0, self.hud_keys.rearview && live && !playback);
            self.mirror_slide = (st, stamp);
            let drawn = (live || st == 2) && (self.hud_keys.rearview || matches!(st, 1 | 2));
            TraySlide { state: if drawn { st } else { 0 }, t: clock.saturating_sub(stamp).min(1 << 20) as i32 }
        };
        if !playback
            && self.vehicle.is_some()
            && mirror_slide.state != 0
        {
            let (rx, ry) = layout.rearview;
            let slide = mirror_slide;
            let m = &mut self.scene.sprites;
            let n0 = m.len();
            if matches!(slide.state, 1 | 2) {
                h.tray(m, hud_aspect, rx, ry, 240.0, 84.0);
            }
            if slide.state == 3
                && self.rear_mirror.is_some()
                && let Some(tex) = self.mirror_tex
            {
                m.push(h.image(hud_aspect, tex, rx, ry, 240.0, 84.0));
            }
            Hud::slide(m, n0, hud_aspect, (rx, ry, 240.0, 84.0), slide);
            if slide.state != 0 {
                let (bx, by) = Hud::anchor(hud_aspect, rx, ry, 240.0, 84.0);
                h.bevel(m, hud_aspect, (bx, by, 240.0, 84.0), 4.0, 0);
            }
        }
    }

    /// **`HReplay`**, the playback's bar.
    fn hud_replay_bar(&mut self, hud_aspect: f32, clock: u64) {
        let Some(h) = &self.hud else { return };
        // `HReplay` is up for as long as the stream is playing (`hud.md` §5.2): its draw tests
        // `REPLAY == 2` (0x42D1E1…0x42D1EC), which holds until the race loop is left — the
        // exhausted frames too (`REPLAY = 3` at 0x415CE2 only). *(Until 2026-09-27 the port
        // tested `Playing` only.)*
        if self.playback_running() {
            // not drawn in state 0 (0x42D28E), faded by the state's per cent otherwise
            if let Some(pct) = self.replay_bar.pct(clock) {
                let tips: [&str; 7] = std::array::from_fn(|i| self.fe.strings.hud(nfs_game::replay::bar_tooltip(i, self.replay.frozen, self.replay_net.is_some())).unwrap_or_default());
                h.replay_bar(&mut self.scene.sprites, hud_aspect, self.replay.frozen, self.replay_net.is_some(), &self.replay_bar, pct, self.fe.cursor, &tips);
            }
        }
    }

    /// **`HTrackMap`** and its blips.
    fn hud_track_map(&mut self, hud_aspect: f32, clock: u64, live: bool, layout: &HudLayout) {
        let playback = self.playback_running();
        let (Some(h), Some(v)) = (&self.hud, &self.vehicle) else { return };
        // the blip loop 0x433110 walks every car (`[0x5E8B58]` of them) but skips one with neither
        // `car+0x52C` bit 0 (a racing car — the player and the opponents) nor bit 0x20 (a cop), so
        // **no traffic** is on the map (0x43312B…0x433142); this port drew the traffic too.
        // It runs **from the last car down** (`dec ebp; jns`, 0x4333EA), and a cop is appended to
        // the car list after the racers (`traffic.md`, the racer slots), so the cops go under the
        // opponents, the opponents under each other from the last one up, and the player on top.
        // A cop's blip is the blue / red pair whatever its lights: the loop tests only the class
        // bit 0x20 (0x43332E…0x43333C), not `car+0xECC & 2`, where this port had drawn a cop
        // without its lights in grey (audit 2026-09-26). Two more gates per car (0x433148…
        // 0x43316E): a racer outside a mission (`[0x6573E8] != 9`) needs its position
        // `[0x5E8E2C + 4i] + 0xB48 >= 1` — 0 only before the ranking pass 0x415FC0's first
        // run, the first odd tick (its only store 0x41601A writes 1 + the table row; *until
        // 2026-09-27 this said "0x417710's first sort", the all-car list's sort*), and the port's
        // `racecar::positions` ranks from the first tick, so nothing to carry beyond frame 0 —
        // and every car must be in the world (`car+0x7F`), which the port's racers always are
        let tick = self.tick;
        let mut others: Vec<(Vec3, Vec3, [f32; 2], [[f32; 4]; 4])> = self
            .road
            .police
            .iter()
            .flat_map(|p| p.cars.iter().filter(|c| c.active))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|c| (c.position, c.orientation.row(2), [c.half_extents.x, c.half_extents.z], [[0.0; 4]; 4]))
            .collect();
        // the cops' registration indices, in the order of `others`' first entries
        let n_opp = self.road.opponents.len();
        let cop_slots: Vec<usize> = self.road.police.iter().flat_map(|p| p.cars.iter().enumerate().filter(|(_, c)| c.active).map(|(i, _)| 1 + n_opp + i)).collect::<Vec<_>>().into_iter().rev().collect();
        others.extend(self.road.opponents.iter().enumerate().rev().map(|(i, o)| {
            (
                o.vehicle.position,
                o.vehicle.forward(),
                [o.vehicle.half_extents.x, o.vehicle.half_extents.z],
                Hud::blip_color(self.road.opponent_paints.get(i).copied().unwrap_or([0x20, 0x40, 0xC0]), None),
            )
        }));
        // 0x432A61: the track map is **not drawn in a replay** — the gate returns at once when
        // `REPLAY [0x657424]` is 2 or 3 (`modes.md` §`0x657424`, `hud.md` §5). The two debug
        // overlays that would force it are set by nothing in the image.
        // (`REPLAY` 2 **or 3**, 0x432A67 / 0x432A6E; until 2026-09-27 the port tested `Playing`
        // only)
        let in_replay = playback;
        // HTrackMap draws while `MAP != 0` or in state 1 / 2 (0x432A8B…0x432A9D, so it slides out
        // after F7), and in any state but 0 (0x432A9F). It is a `+5` element (0x425B91…0x425BDD:
        // past clock 448, `[0x60C970]` or state 2) with its own state (`map_slide`), opened by
        // 0x425E20 only while its slot 10 0x433400 is clear — `MAP != 0 && [0x60C970]`. MAP is
        // the runtime copy F7 / "2" / F5 change (`HudKeys::map`).
        let map_slide = {
            let (st, stamp) = step_slide(self.map_slide, clock, self.tick <= RACE_START_TICK, self.billboard_key != 0, self.hud_keys.map != 0 && live && !playback);
            self.map_slide = (st, stamp);
            let drawn = (live || st == 2) && (self.hud_keys.map != 0 || matches!(st, 1 | 2));
            TraySlide { state: if drawn { st } else { 0 }, t: clock.saturating_sub(stamp).min(1 << 20) as i32 }
        };
        if !in_replay && map_slide.state != 0 {
            let slide = map_slide;
            // 0x432F92: the current lap is `min(car+0xB20 + 1, NUM_LAPS)` — the laps already
            // timed, plus one — and 0x432EA3 suppresses the whole counter on a linear track.
            // This port printed it as "LAP n/N" in the top-right tray.
            let linear = !self.records.track(&self.track.name).map(|t| t.circuit).unwrap_or(true);
            let lap_label = self.fe.strings.hud(19).unwrap_or_default();
            let lap = self
                .race
                .as_ref()
                // no finish test: past the finish it reads `min(laps + 1, NUM_LAPS)` = N/N
                // (0x432F8D…0x432FAA); a Smash Up counts no laps, so it stays `1/N`
                .filter(|_| (self.num_laps < 99 || self.derby_hud_laps.is_some()) && !linear)
                .map(|r| {
                    let (done, of) = match self.derby_hud_laps {
                        Some(n) => (0, n),
                        None => (r.laps, self.num_laps),
                    };
                    (lap_label, format!("{}/", (done + 1).min(of)), of.to_string())
                });
            // MAP 1 "rotating" and 3 "radar" turn the view (0x4336BF / 0x4336B9 → 0x433440): by
            // the car's heading, or in radar mode by its slice's direction; 2 "static" does not
            // (MAP 0 while it slides out after F7 answers neither test: the static view)
            let map_mode = self.hud_keys.map;
            let heading = match map_mode {
                1 => Some((v.forward().x, v.forward().z)),
                3 => self
                    .slices
                    .as_ref()
                    .zip(self.race.as_ref())
                    .and_then(|(t, r)| t.slices.get(r.slice))
                    .map(|sl| (sl.forward.x, sl.forward.z)),
                _ => None,
            };
            // the zoom 0x41E9C0: base 0 / 0 / 33 / 66 / 100 by MAPSCALE, then `· 4.15 + 185`, or
            // `· 0.15 + 5` in radar mode; 600 (= 0x41EA40) is the whole-track view
            let base = [0.0f32, 0.0, 33.0, 66.0, 100.0][self.choice_index(choice::DISPLAY_MAP_SCALE).map_or(4, |(i, _)| i.min(4))];
            let zoom = if map_mode == 3 { base * 0.15 + 5.0 } else { base * 4.15 + 185.0 };
            let follow = (zoom != 600.0).then_some((v.position.x, v.position.z, zoom));
            if let Some(r) = self.renderer.as_mut() {
                h.map_update(r, heading, map_mode != 3, follow);
            }
            // the cops' colours step only here, in the drawn map's loop
            for (j, &slot) in cop_slots.iter().enumerate() {
                if self.cop_blips.len() <= slot {
                    self.cop_blips.resize(slot + 1, (0xC000_00FF, 0));
                }
                others[j].3 = Hud::blip_color([0; 3], Some(Hud::cop_blip_step(&mut self.cop_blips[slot], tick)));
            }
            // the player is one of the racers the loop walks: its blip is its own paint too
            others.push((v.position, v.forward(), [v.half_extents.x, v.half_extents.z], Hud::blip_color(self.paint, None)));
            let k = if map_mode == 3 { 1.0 } else if zoom == 600.0 { 60.0 } else { 20.0 };
            let map = &mut self.scene.sprites;
            let n0 = map.len();
            h.map_sprites(map, hud_aspect, layout.track_map, &others, k, lap, matches!(slide.state, 1 | 2));
            let (mpx, mpy) = layout.track_map;
            Hud::slide(map, n0, hud_aspect, (mpx, mpy, MAP_PX as f32, MAP_PX as f32), slide);
        }
    }

    /// The `NFS_CONGRATS` / `NFS_BILLBOARD` diagnostics' dialogs over the race.
    fn hud_diagnostic_dialogs(&mut self, hud_aspect: f32) {
        let Some(h) = &self.hud else { return };
        // diagnostic: `NFS_CONGRATS=<place>,<tick>,<0|1 tournament>` draws the `HCongrats`
        // picture at that point of its timeline, for a `--screenshot` without finishing a race
        if let Some(spec) = nfs_game::config::cfg().nfs_congrats.as_str() {
            let v: Vec<u64> = spec.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let [place, t, tour] = v[..] {
                let mut c = Vec::new();
                h.congrats_sprites(&mut c, hud_aspect, place as usize, t, tour != 0);
                self.scene.sprites.extend(c);
            }
        }
        // diagnostic: `NFS_BILLBOARD=pause|mission|yesno|summary|standings` draws that dialog over the race, for a
        // `--screenshot` of its layout
        if let Some(which) = nfs_game::config::cfg().nfs_billboard.as_str() {
            let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
            let mut c = Vec::new();
            match which {
                "summary" => {
                    let rows = vec![
                        SummaryRow { name: self.player_name.clone(), car: self.specs.name.clone(), colour: [0.83, 0.06, 0.06, 1.0], best: Some(3999), total: Some(8100) },
                        SummaryRow { name: "Stroller".into(), car: "'94 911 Carrera".into(), colour: [0.2, 0.4, 0.8, 1.0], best: None, total: None },
                    ];
                    let head = [fe(hud_str::COL_NAME), fe(hud_str::COL_CAR), fe(hud_str::COL_BEST_LAP), fe(hud_str::COL_TIME), fe(hud_str::COL_POINTS)];
                    let bb = Billboard { left: &fe(Self::summary_title(self.race_fegame_type())), right: &fe(hud_str::TITLE_RACE_SUMMARY), hints: Some((&fe(hud_str::HINT_ESC), Some(&fe(hud_str::HINT_SPACE)))) };
                    let ord: Vec<String> = (18116..18124).map(fe).collect();
                    h.results_sprites(&mut c, hud_aspect, &rows, 0, &bb, &head, &[30.0, 90.0, 140.0, 100.0, 80.0, 60.0], true, true, &ord);
                }
                // the standings with the pointer on the player's WINNINGS cell, so `HWinnings` shows
                "standings" => {
                    let rows = vec![
                        (self.player_name.clone(), "15".to_string(), "7200".to_string()),
                        ("Stroller".to_string(), "14".to_string(), "5800".to_string()),
                    ];
                    let w = nfs_game::career::Winnings { events: vec![(1, 1000, 400), (2, 800, 0)], tournament_bonus: Some(5000), total: 7200, completed: 2 };
                    let (x0, x1, y0, y1) = Hud::table_cell(&[40.0, 90.0, 100.0, 100.0], 0, 3);
                    let bb = Billboard { left: "", right: &fe(hud_str::TITLE_TOURNAMENT_STANDINGS), hints: Some((&fe(hud_str::HINT_ESC), Some(&fe(hud_str::HINT_SPACE)))) };
                    let ord: Vec<String> = (18116..18124).map(fe).collect();
                    let panel = (&w, [fe(hud_str::PRIZE_LABEL), fe(hud_str::PRIZE_SAFE_BONUS), fe(hud_str::PRIZE_TOURNAMENT_BONUS), fe(hud_str::PRIZE_TOTAL)], Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0)));
                    h.standings_sprites(&mut c, hud_aspect, &rows, Some(0), &bb, &[fe(hud_str::COL_NAME), fe(hud_str::COL_TOTAL_POINTS), fe(hud_str::COL_WINNINGS)], false, &ord, Some(panel));
                }
                "yesno" => h.yes_no_sprites(&mut c, hud_aspect, &Billboard { left: "", right: &fe(hud_str::TITLE_YES_NO), hints: None }, [&fe(hud_str::QUIT_CONFIRM_LINE), ""], [&fe(hud_str::YES_NO_YES), &fe(hud_str::YES_NO_NO)], 1, false),
                "pause" => h.pause_sprites(&mut c, hud_aspect, &Billboard { left: "", right: &fe(hud_str::TITLE_PAUSE), hints: None }, &[&fe(hud_str::PAUSE_BUTTON_1), &fe(hud_str::PAUSE_BUTTON_2), &fe(hud_str::PAUSE_BUTTON_3)], 0, false),
                "mission" => h.mission_summary_sprites(&mut c, hud_aspect, &Billboard { left: &fe(hud_str::TITLE_MISSION_LEFT), right: &fe(hud_str::TITLE_MISSION_RIGHT), hints: Some((&fe(hud_str::HINT_ESC), Some(&fe(hud_str::HINT_SPACE)))) }, Some(0), &fe(18176), &[fe(18156)], &fe(hud_str::PAUSE_BUTTON_2), None, (false, false)),
                _ => {}
            }
            self.scene.sprites.extend(c);
        }
    }

    /// The post-race queue's pages (`HRaceSummary`, `HCongrats`, `HTournamentStandings`, `HInfo`,
    /// `HTrackRecords`, the mission summary); `true` when the page finished shrinking this frame.
    fn hud_result_pages(&mut self, hud_aspect: f32, hud_hover: &mut Option<Option<(u8, usize)>>) -> bool {
        let Some(h) = &self.hud else { return false };
        let mut finish_close = false;
        // the post-race queue is built from the result records, and only once they exist — at
        // race over, over the automatic replay (`0x426A80(1)`, 0x41533A); the live race runs on
        // past the player's finish with no page up
        if let Some(res) = self.results.as_ref()
            && !self.result_pages().is_empty()
        {
            let order = res.order.clone();
            let circuit = !self.linear_track();
            // the NAME cell is every car's `.DRIVERNAME` (car-info `+0x17C`, 0x42C327…0x42C34E) —
            // the player's own name too, where this port printed "YOU" — and the CAR cell is
            // drawn in the car's paint `[[car+0x558]+0x218]` (0x42C3BE…0x42C3DD)
            let paint = |c: [u8; 3]| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0];
            // `HRaceSummary` fills row `car+0xCD4 − 1` with the record's time `+0xCD8` and best
            // lap `+0xCFC` (0x42C163…0x42C18A); the TIME cell prints dashes for a time of 0
            // (0x42C41C), the BEST LAP cell prints whatever `+0xCFC` holds (0x42C410) —
            // `Race_finalize` 0x416AC0 made it 0 for a player with no lap, 99999 for an
            // opponent with none
            // the CAR cell is `spec+0x534` — the car's `nfs5.car +0x34` family — capped to
            // 15 characters (0x42C368…0x42C3BA: `lea edx,[eax+0x534]`, `cmp ecx,0xf`),
            // `modes.md` §7.1; not `CarSpecs::name`
            let family_cell = |s: &str| s.chars().take(15).collect::<String>();
            let row = |d: usize| -> SummaryRow {
                let (name, car, colour) = if d == 0 {
                    (self.player_name.clone(), family_cell(&self.player_car_family()), paint(self.paint))
                } else {
                    let o = &self.road.opponents[d - 1];
                    let fam = self.road.opponent_families.get(d - 1).cloned().unwrap_or_else(|| o.specs.name.clone());
                    (o.name.clone(), family_cell(&fam), paint(self.road.opponent_paints.get(d - 1).copied().unwrap_or([0x20, 0x40, 0xC0])))
                };
                let rec = res.racers.get(d).cloned().unwrap_or_default();
                SummaryRow { name, car, colour, best: Some(rec.best_lap), total: (rec.time != 0).then_some(rec.time) }
            };
            let rows: Vec<_> = order.iter().map(|&d| row(d)).collect();
            let player = order.iter().position(|d| *d == 0).unwrap_or(0);
            let mut table = Vec::new();
            // the headers, the ordinals and the billboard's title come from `FEStrings`
            let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
            // `HRaceSummary`'s four column sets (0x42BF43…0x42BFED): a tournament event ranked by
            // points adds POINTS; a point-to-point track (or a network race) drops BEST LAP
            let points = self.tournament_event().is_some_and(|(t, _)| !t.time_ranked());
            let (head, widths): (Vec<String>, &[f32]) = match (points, circuit) {
                (true, false) => (vec![fe(hud_str::COL_NAME), fe(hud_str::COL_CAR), fe(hud_str::COL_TIME), fe(hud_str::COL_POINTS)], &[30.0, 90.0, 140.0, 80.0, 60.0]),
                (true, true) => (vec![fe(hud_str::COL_NAME), fe(hud_str::COL_CAR), fe(hud_str::COL_BEST_LAP), fe(hud_str::COL_TIME), fe(hud_str::COL_POINTS)], &[30.0, 90.0, 140.0, 100.0, 80.0, 60.0]),
                (false, true) => (vec![fe(hud_str::COL_NAME), fe(hud_str::COL_CAR), fe(hud_str::COL_BEST_LAP), fe(hud_str::COL_TIME)], &[30.0, 90.0, 140.0, 100.0, 80.0]),
                (false, false) => (vec![fe(hud_str::COL_NAME), fe(hud_str::COL_CAR), fe(hud_str::COL_TIME)], &[30.0, 90.0, 140.0, 80.0]),
            };
            let ordinals: Vec<String> = (18116..18124).map(fe).collect();
            let (esc, space) = (fe(hud_str::HINT_ESC), fe(hud_str::HINT_SPACE));
            let title = fe(Self::summary_title(self.race_fegame_type()));
            let page = self.result_pages().get(self.results_page as usize).copied().unwrap_or(ResultPage::Closed);
            // the race's own `HCongrats` is the first thing the queue shows (0x426D54), for
            // game types 0…4 (0x426CCD…0x426CEC, and 0x41D080 skips type 5), places 1–3 and in a
            // Quick Knockout only the winner (show 0x41DD40); it closes itself past 0xC0 ticks
            // the place it shows is the player's `car+0xCD4` (0x426D41…0x426D53), its index in
            // the ranking table — not the count of cars ahead the port took when it crossed
            // the line, which missed a car level on progress that got there first
            let place = res.racers.first().map_or(8, |r| r.place as usize);
            let race_congrats = {
                // it opens with the queue, at the replay's start (0x426A80(1) → 0x426C90)
                let t = self.tick.saturating_sub(res.opened);
                let allowed = self.mission.is_none()
                    && self.career.as_ref().and_then(|c| c.pending.as_ref()).is_none_or(|p| !p.finale)
                    && if self.quick_knockout { place == 1 } else { (1..=3).contains(&place) };
                (page == ResultPage::Summary && allowed && res.congrats && t <= 0xC0).then_some((place, t))
            };
            // the billboard opening (`Hud::billboard`): frames since this page first showed,
                // counted only once the race's own `HCongrats` has made way for it
            let mut closed_now = false;
            // the page's box fully open this frame (state 3)
            let mut open_now = false;
            self.results_billboard = None;
            let billboard_up = race_congrats.is_none() && !matches!(page, ResultPage::Closed | ResultPage::TournamentPlace(_));
            if billboard_up {
                if self.page_seen != Some(self.results_page) {
                    self.page_seen = Some(self.results_page);
                    self.page_frames = 0;
                    self.page_open_tick = Some(self.tick);
                } else {
                    self.page_frames += 1;
                }
                let mut height = (40.0 + 65.0 * (self.page_frames as f32 + 1.0)).min(BILLBOARD.3);
                if let Some(f) = self.page_closing.as_mut() {
                    *f += 1;
                    height = (BILLBOARD.3 - 65.0 * *f as f32).max(40.0);
                    if height <= 40.0 {
                        closed_now = true;
                    }
                }
                h.billboard_h.set(height);
                open_now = self.page_closing.is_none() && height >= BILLBOARD.3;
                h.billboard_clock.set((self.hud_clock(), self.hud_clock().saturating_sub(self.tick.saturating_sub(self.page_open_tick.unwrap_or(0)))));
                self.results_billboard = self.page_closing.is_none().then_some(self.results_page as u32);
            } else if let ResultPage::TournamentPlace(_) = page
                && self.page_seen != Some(self.results_page)
            {
                self.page_seen = Some(self.results_page);
                self.page_open_tick = Some(self.tick);
            }
            if page == ResultPage::Closed {
            } else if page == ResultPage::MissionSummary {
                let narrator = self.mission.as_ref().map(|m| m.mission.narrator.clamp(0, 5) as usize);
                let name = narrator.map(|n| fe(18176 + n as u32)).unwrap_or_default();
                let lines = self.mission_text_lines();
                // 0x428947…0x428986: 18134 left, 18135 right, both hints (`push 1, push 1`)
                let bb = Billboard { left: &fe(hud_str::TITLE_MISSION_LEFT), right: &fe(hud_str::TITLE_MISSION_RIGHT), hints: Some((&esc, Some(&space))) };
                // its "race again" button's hover (0x41B340), once the box is open
                let over = h.billboard_h.get() >= BILLBOARD.3 && Hud::over_button(Hud::mission_button(), self.fe.cursor);
                *hud_hover = Some(over.then_some((2, 0)));
                h.mission_summary_sprites(&mut table, hud_aspect, &bb, narrator, &name, &lines, &fe(hud_str::PAUSE_BUTTON_2), Some(self.page_frames as usize), (over, over && self.fe.mouse_down));
            } else if let Some((place, t)) = race_congrats {
                h.congrats_sprites(&mut table, hud_aspect, place, t, false);
            } else if let ResultPage::TournamentPlace(rank) = page {
                h.congrats_sprites(&mut table, hud_aspect, rank, self.tick.saturating_sub(self.results_page_tick), true);
            } else if page == ResultPage::Standings
                && let Some((t, event)) = self.tournament_event()
                && let Some(c) = self.career.as_ref()
            {
                // `HTournamentStandings` 0x431860: title 18132, NAME 18111 / TOTAL POINTS 18127
                // / WINNINGS 18161, or NAME / RALLY TIME 18162 / WINNINGS for a club event
                // ranked by time (0x4318A2…0x431957; `[0x6577E4]` picks at 0x431DAA)
                let (order, times) = self.finishing_order();
                let clean = self.clean_racers();
                // on the last race each place's WINNINGS carries its tournament prize (0x431B80)
                let racers = self.career_opponents.as_ref().map_or(0, |l| l.len()) + 1;
                let last = c.career.tournament_flags(&c.tables, &t).0;
                let st = c.career.standings_page(&t, event, &order, &times, &clean, racers, last);
                let name = |d: usize| {
                    if d == 0 {
                        self.player_name.clone()
                    } else {
                        self.career_opponents.as_ref().and_then(|l| l.get(d - 1)).map(|o| o.name.clone()).unwrap_or_default()
                    }
                };
                let timed = t.time_ranked();
                let rows: Vec<(String, String, String)> = st
                    .iter()
                    .map(|(d, pts, money, tm)| {
                        let mid = if timed { format_time_dword(*tm, false) } else { pts.to_string() };
                        (name(*d).chars().take(31).collect(), mid, money.to_string())
                    })
                    .collect();
                let player = st.iter().position(|(d, ..)| *d == 0);
                let head = [fe(hud_str::COL_NAME), fe(if timed { hud_str::COL_RALLY_TIME } else { hud_str::COL_TOTAL_POINTS }), fe(hud_str::COL_WINNINGS)];
                // 0x431897: "" left, 18132 right, both hints
                let bb = Billboard { left: "", right: &fe(hud_str::TITLE_TOURNAMENT_STANDINGS), hints: Some((&esc, Some(&space))) };
                // `HWinnings`, the player's money, under the pointer (0x431AF6…0x431B70)
                let winnings = c.career.winnings_after(&t, event, &order, &times, &clean, racers, last);
                // diagnostic (port-only): `NFS_RESULTS_CURSOR=winnings` parks the pointer on the
                // middle of the player's WINNINGS cell, `=x,y` at that point of the 640 × 480
                // HUD space — a scripted run has no pointer, and `NFS_FE_CURSOR` parks the front
                // end's, not the race's. Unset, the live pointer.
                let parked = nfs_game::config::cfg().nfs_results_cursor.as_str().and_then(|v| {
                    if v.trim().eq_ignore_ascii_case("winnings") {
                        let widths: &[f32] = if !timed { &[40.0, 90.0, 100.0, 100.0] } else { &[40.0, 90.0, 120.0, 100.0] };
                        let (x0, x1, y0, y1) = Hud::table_cell(widths, player?, 3);
                        Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0))
                    } else {
                        let (x, y) = v.split_once(',')?;
                        Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
                    }
                });
                let panel = (&winnings, [fe(hud_str::PRIZE_LABEL), fe(hud_str::PRIZE_SAFE_BONUS), fe(hud_str::PRIZE_TOURNAMENT_BONUS), fe(hud_str::PRIZE_TOTAL)], parked.or(self.fe.cursor));
                h.standings_sprites(&mut table, hud_aspect, &rows, player, &bb, &head, timed, &ordinals, Some(panel));
            } else if let ResultPage::Notice(id) = page {
                // `HInfo` 0x4282F0: the text centred on the 500 × 325 box (`[0x5B2BF0]`/`[0x5B2BF4]`)
                // `HInfo` 0x428328: "" left, 18157 "Race Info" right, both hints — its Spacebar
                // byte is its own constructor argument, and the post-race one `[0x60C9FC]` is
                // built with 1 (`push 1` before 0x4250BF)
                let bb = Billboard { left: "", right: &fe(hud_str::TITLE_RACE_INFO), hints: Some((&esc, Some(&space))) };
                h.notice_sprites(&mut table, hud_aspect, &bb, &fe(id));
            } else if page == ResultPage::Records
                && let Some(rec) = &self.hud_records
            {
                // `HTrackRecords` 0x433830: 18133 "Track Records" / 18158 "Lap Records", columns
                // NAME 18111, CAR 18112, TIME 18114, rows 1st…8th
                let title = fe(if rec.linear { 18133 } else { 18158 });
                let head = [fe(hud_str::COL_NAME), fe(hud_str::COL_CAR), fe(hud_str::COL_TIME)];
                // 0x433874: "" left, 18133 / 18158 right, both hints
                let bb = Billboard { left: "", right: &title, hints: Some((&esc, Some(&space))) };
                h.records_sprites(&mut table, hud_aspect, rec, &bb, &head, &ordinals);
                // 0x4339C0: once open (state 3), the first time (`+0x124`): kLapRecord when
                // `+0x126`, else kTrackRecord when `+0x125` — one of the two by the track
                if open_now
                    && !self.records_spoken
                {
                    self.records_spoken = true;
                    if rec.first
                        && let (Some(_), Some(sp)) = (&self.snd.audio, &self.snd.speech)
                    {
                        // 0x4A6600: kLapRecord `125 + rand % 5`; 0x4A6540: kTrackRecord
                        // `113 + rand % 12` (`div ecx` of 0x538040's draw; until 2026-09-26
                        // the port took the tick)
                        let rng = &mut self.fe_rng;
                        let mut draw = || rng.next_u32();
                        sp.say_id(if !rec.linear {
                            nfs_audio::speech::phrase::lap_record(&mut draw)
                        } else {
                            nfs_audio::speech::phrase::track_record(&mut draw)
                        });
                    }
                }
            } else {
                // 0x42BD08: the game type's title left, 18130 "Race Summary" right, both hints
                let bb = Billboard { left: &title, right: &fe(hud_str::TITLE_RACE_SUMMARY), hints: Some((&esc, Some(&space))) };
                h.results_sprites(&mut table, hud_aspect, &rows, player, &bb, &head, widths, circuit, points, &ordinals);
            }
            h.billboard_h.set(BILLBOARD.3);
            self.scene.sprites.extend(table);
            if closed_now {
                finish_close = true;
            }
        }
        finish_close
    }

    /// **`HMultiTime`** 0x42A200.
    fn hud_multi_time(&mut self, hud_aspect: f32, layout: &HudLayout, time_slide: TraySlide) {
        let Some(h) = &self.hud else { return };
        // `HMultiTime` 0x42A200 — a `HComboTextVisual` like `HTime`: the tray 73×34 at (427,0)
        // (`nfs5-2.hud`), the value top-right and a `hud.loc` label bottom-left (0x41C8C0).
        // It shows only while the racing trays do (they slide out for the post-race billboards).
        // This port printed an upper-case "RECORD LAP  m:ss.hh" line with no tray, and a
        // "KNOCKED OUT" banner for an eliminated player that has no string in the game —
        // HPosition's FE 18191 "DNF" is the original's only sign of that (0x42B2B6).
        // (gates 0x41E850 and the state 0x42A3B0 only — it stays past the finish, 2026-09-27)
        let racing = self.race.is_some();
        // the gate 0x41E850: `SPLITTIMES != 0 && [0x60C970]`
        let split_times = self.choice_index(choice::DISPLAY_SPLIT_TIMES).map_or(0, |(i, _)| i);
        let multi = if !racing || split_times == 0 || !self.hud_keys.live {
            None
        } else if let Some(ms) = &self.mission {
            // game type 9: FE 18172 "goal time" (0x42A22B) over
            // `_ftol(FACTORYDRIVERTIMELIMIT · 64.0)` (0x42A24E…0x42A25A); nothing when the
            // limit is −1 (0x42A245)
            ms.mission.time_limit_s.map(|limit| (18172, limit as u64 * 64))
        } else if let Some(t) = self.records.track(&self.track.name) {
            // point-to-point (`[0x657494]`) → FE 21 "record" and the track record; circuits →
            // the SPLITTIMES selection: 1 FE 22 "record lap", 2 FE 23 "best lap", 3 FE 24
            // "last lap" (0x42A279…0x42A2F2)
            let record = t.entries.first().map(|e| e.time as u64);
            let laps: &[u64] = self.race.as_ref().map_or(&[], |r| &r.lap_times);
            if !t.circuit {
                record.map(|v| (21, v))
            } else {
                // before the first lap is in, both of these read **zero** and are still
                // drawn: `car+0xB20` (the lap count) being 0 takes the `xor esi, esi` arms
                // at 0x42A2E6 and 0x42A2A8, and the element formats and draws `esi`
                // whatever it is. The min at 0x42A2D4 has no non-zero filter either, and
                // the car it reads is `[0x65741C]` — the player's own index, not the
                // camera's (`replay.md` §2).
                // `SPLITTIMES [0x657DD8]` is the `Display.SplitTimes` choice (0 off, which
                // the gate below already returned on); this port had it start at 1 and cycled
                // it on its own T key, which the original has no binding for
                //
                // **Reference comparison, not a bug (2026-09-29, `hud.md`'s
                // HMultiTime entry):** the reference capture showed `59:59.99` here before the
                // first lap, against this port's `00:00.00` / "лучший". `fe.txt` ships
                // `splittimes=2` (the default, matching this port and this arm), and the
                // zero-lap short-circuit above is real (0x42A299…0x42A2A8) — `0x42A2B4`'s
                // `esi = 0x57E3F` (= 359999, the `NO_RECORD_TIME` "n/a" sentinel) is only the
                // min-loop's seed and is never reached before the zero test. `monaco3`'s own
                // `nfs5.trk` row is not empty (a real 5870-tick record), so the sentinel cannot
                // legitimately show here for this track under the shipped default; the capture
                // most likely reflects a different `SplitTimes` choice or stale session state in
                // that one wine run, not a difference in this port's zero/sentinel handling.
                match split_times {
                    2 => Some((23, laps.iter().copied().min().unwrap_or(0))),
                    3 => Some((24, laps.last().copied().unwrap_or(0))),
                    _ => record.map(|v| (22, v)),
                }
            }
        } else {
            None
        };
        if let Some((label, value)) = multi {
            // from `hud.loc` (0x42A230 / 0x42A2F4 call 0x41E160, the HUD's table) — like every
            // HUD element's text; the front end's table has no ids 18172, 21…24 (audit 2026-09-27)
            let label = self.fe.strings.hud(label).unwrap_or_default();
            // `Display.ScreenLayout`'s own position for this tray (cfg2 +0xC/+0x10, `layout`
            // above, `HudLayout::multi_time`) — was hardcoded to Config 1's `(427.0, 0.0)`
            let (mx, my) = layout.multi_time;
            let tray = &mut self.scene.sprites;
            let n0 = tray.len();
            h.combo_sprites(tray, hud_aspect, (mx, my, 73.0, 34.0), &format_ticks(value), label, h.display());
            Hud::slide(tray, n0, hud_aspect, (mx, my, 73.0, 34.0), time_slide);
        }
    }

    /// The HUD frame's cone count.
    fn hud_cone_count(&mut self) {
        // the HUD frame's cone count (0x425D7F…0x425DFD): not in a replay (`[0x657424]` 0 or 1), in
        // a mission (`[0x6573E8] == 9`) with a cone rule (`CONEIMIT` or `CONEPENALTY` not −1), over
        // every smackable; a move plays 0x4AAE50, flashes `HTime` and bumps the summary's `+0x140`.
        // It runs after the billboards' and the trays' draws (0x425A10…0x425A72), so the frame
        // the summary first evaluates in (`fix_mission_result`) has not counted yet, and the
        // flash shows from the next draw on.
        if self.hud.is_some()
            && self.vehicle.is_some()
            && self.mission.as_ref().is_some_and(|ms| ms.mission.cone_rule.is_some())
            && !self.playback_running()
            && self.hud_cones.frame(self.smackables.knocked_count())
        {
            tracing::info!(count = self.hud_cones.last, moves = self.hud_cones.changes, "cone hit");
            self.cone_flash = 16;
            if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
                s.pylon_hit(a);
            }
        }
    }

    /// **`HReplaySave`** over the race (0x42E040 / 0x42E2A0) and its overwrite question
    /// `HYesNoOverlay` 0x437440, while the save box is up and the game is not paused. Sets
    /// `hud_hover` to the button under the pointer.
    pub(super) fn replay_save_overlay(&mut self, hud_hover: &mut Option<Option<(u8, usize)>>) {
        if self.paused || !self.replay_save.up {
            return;
        }
        let Some(h) = self.hud.as_ref().filter(|h| h.font.is_some()) else { return };
        let strings = &self.fe.strings;
        let fe = move |id: u32| strings.hud(id).unwrap_or_default();
        let aspect = self.hud_aspect();
        let out = &mut self.scene.sprites;
        let pressed = self.fe.mouse_down;
        h.billboard_h.set(BILLBOARD.3);
        if self.replay_save.overwrite {
            // `HYesNoOverlay` 0x437440 ("" | 18174) with 0x4376B0(18139, 18140)
            let hover = Hud::yes_no_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor));
            *hud_hover = Some(hover.map(|k| (4, k)));
            let bb = Billboard { left: "", right: fe(hud_str::TITLE_YES_NO), hints: None };
            h.yes_no_sprites(out, aspect, &bb, [fe(hud_str::OVERWRITE_LINE_1), fe(hud_str::OVERWRITE_LINE_2)], [fe(hud_str::YES_NO_YES), fe(hud_str::YES_NO_NO)], hover.unwrap_or(usize::MAX), pressed && hover.is_some());
        } else {
            let hover = Hud::replay_save_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor));
            *hud_hover = Some(hover.map(|k| (3, k)));
            let bb = Billboard { left: "", right: fe(hud_str::TITLE_REPLAY_SAVE), hints: None };
            // the caret flips each time `[0x606AC8]` is 16 past the last flip — every 17 ticks
            let t = self.replay_save.opened.map_or(0, |o| (o.elapsed().as_secs_f64() * TICKS_PER_SEC_F64) as u64);
            let caret = (t / 17) % 2 == 0;
            h.replay_save_sprites(out, aspect, &bb, fe(hud_str::REPLAY_SAVE_LABEL), &self.replay_save.name, caret, [fe(hud_str::REPLAY_SAVE_BUTTON_1), fe(hud_str::REPLAY_SAVE_BUTTON_2)], hover, pressed && hover.is_some());
        }
    }

    /// **`HPauseMenu`** 0x42A440 while paused: title 18128 "Pause Menu" (0x46D0) over three buttons,
    /// 18108 continue / 18109 race again / 18110 quit; the quit question is `HYesNoOverlay`
    /// 0x437440, 18174 "Confirmation" with 18171 and 18141 yes / 18142 no. The strings and their
    /// places are the original's (`pause_sprites`, `yes_no_sprites`; hud.md). Sets `hud_hover`.
    pub(super) fn pause_overlay(&mut self, hud_hover: &mut Option<Option<(u8, usize)>>) {
        if !self.paused {
            return;
        }
        let Some(h) = self.hud.as_ref().filter(|h| h.font.is_some()) else { return };
        let strings = &self.fe.strings;
        let fe = move |id: u32| strings.hud(id).unwrap_or_default();
        // the pointer over a button: its hover look, and the port's keyboard selection moves
        // there so only one button is lit (the original has no keyboard selection)
        let hover = match self.pause_confirm {
            None => Hud::pause_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor)).map(|k| (0, k)),
            Some(_) => Hud::yes_no_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor)).map(|k| (1, k)),
        };
        *hud_hover = Some(hover);
        match hover {
            Some((0, k)) => self.pause_sel = k as u8,
            Some((_, k)) => self.pause_confirm = Some(k as u8),
            None => {}
        }
        let pressed = hover.is_some() && self.fe.mouse_down;
        let aspect = self.hud_aspect();
        // `[0x606AC8]` keeps running through the pause (see `pause_ticks`)
        let clock = self.hud_clock();
        let out = &mut self.scene.sprites;
        match self.pause_confirm {
            None => {
                // 0x42A477: "" left, 18128 right, no hints
                let bb = Billboard { left: "", right: fe(hud_str::TITLE_PAUSE), hints: None };
                h.billboard_clock.set((clock, 0));
                h.pause_sprites(out, aspect, &bb, &[fe(hud_str::PAUSE_BUTTON_1), fe(hud_str::PAUSE_BUTTON_2), fe(hud_str::PAUSE_BUTTON_3)], self.pause_sel as usize, pressed);
            }
            Some(sel) => {
                let bb = Billboard { left: "", right: fe(hud_str::TITLE_YES_NO), hints: None };
                h.billboard_clock.set((clock, 0));
                h.yes_no_sprites(out, aspect, &bb, [fe(hud_str::QUIT_CONFIRM_LINE), ""], [fe(hud_str::YES_NO_YES), fe(hud_str::YES_NO_NO)], sel as usize, pressed);
            }
        }
    }

    /// `HInfo`'s in-race box (0x4282F0; `net_race_notice`), while not paused.
    pub(super) fn net_notice_overlay(&mut self) {
        if self.paused {
            return;
        }
        let Some(id) = self.net_notice else { return };
        let Some(h) = self.hud.as_ref().filter(|h| h.font.is_some()) else { return };
        let strings = &self.fe.strings;
        let fe = move |id: u32| strings.hud(id).unwrap_or_default();
        let aspect = self.hud_aspect();
        h.billboard_h.set(BILLBOARD.3);
        // 0x428328: "" left, 18157 right, the Esc hint alone (Spacebar byte 0)
        let space = self.net_notice_space.then(|| fe(hud_str::HINT_SPACE));
        let bb = Billboard { left: "", right: fe(hud_str::TITLE_RACE_INFO), hints: Some((fe(hud_str::HINT_ESC), space)) };
        h.notice_sprites(&mut self.scene.sprites, aspect, &bb, fe(id));
    }
}
