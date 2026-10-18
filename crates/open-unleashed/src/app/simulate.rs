//! The race simulation tick (`simulate`): controller sampling, the vehicle, the opponents, the police and the traffic.

use crate::*;
use nfs_physics::vehicle::{ContactKind, RaceState};

impl App {
    /// **One 32 Hz sample of the controller**, the packer 0x412E70's output as the car reads it
    /// (`replay.md` §2, `Car_applyInput` 0x4991E0): the steering (negated under `MIRROR`, 0x4992D6), the
    /// throttle and brake bytes doubled, the handbrake, and the command code the packer fires on a
    /// press (0x41312F…0x4131E9). `exchange` adds the reset key as command **5** (`C1_RESET`), which in
    /// an input-exchange race travels in the sample like every other command — 0x40F7EB reads it back
    /// from `car+0xD8B` for every human car — where a live race takes the key straight from the input.
    /// The scripted drive (`follow_road`, autotest only, not the original's) steers the car itself.
    pub(crate) fn controller_sample(
        input: &mut InputState,
        mirror: bool,
        follow_road: bool,
        slices: Option<&SliceTable>,
        race: Option<&RaceProgress>,
        v: &Vehicle,
        exchange: bool,
    ) -> Controls {
        // right = +x in the body frame; rounded, so a pad's packet byte (a multiple of 2/126)
        // comes back whole
        let mut steer = (input.steer() * 126.0).round() as i32;
        if mirror {
            steer = -steer;
        }
        // **TEST GLUE** (not in the original): `NFS_AUTOPILOT=1` with `--follow` drives a windowed race too — the
        // throttle held, the steering below — so a live race (HUD and all) can be filmed without a driver
        if follow_road && nfs_game::config::cfg().nfs_autopilot.is_set() {
            input.press(Action::Throttle);
        }
        if follow_road && let (Some(t), Some(r)) = (slices, race) {
            // pure pursuit on the slice ahead (autotest only; not the original AI)
            let ahead = t.add(r.slice, 4);
            let target = t.slices[ahead].centre;
            let to = v.orientation * (target - v.position); // body-frame vector (rows = axes)
            // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable atan2 for the scripted drive (test glue, no original counterpart).
            let angle = nfs_math::portable::atan2f(to.x, to.z.max(0.1));
            steer = ((angle / 0.4).clamp(-1.0, 1.0) * 126.0) as i32;
            // crude speed limit for the scripted drive: ease off above 110 km/h or in sharp bends
            let limit = if angle.abs() > 0.15 { 60.0 } else { 110.0 };
            if v.speed() * 3.6 > limit {
                input.release(Action::Throttle);
                input.press(Action::Brake);
            } else {
                input.release(Action::Brake);
            }
        }
        let mut throttle = input.throttle();
        if follow_road {
            // autotest: ease off above 130 km/h and when steering hard
            if v.speed() > 36.0 || steer.abs() > 80 {
                throttle = throttle.min(0.35);
            }
        }
        // The shape the original's controller packet has (`replay.md` §2): every axis is at
        // most 0x7E and the car doubles it (0x4992CF), so the values the physics sees are even
        // and never exceed 252 / ±126.
        Controls {
            throttle: ((throttle * 126.0) as u8).min(0x7E) * 2,
            brake: ((input.brake() * 126.0) as u8).min(0x7E) * 2,
            steer: (steer / 2).clamp(-63, 63) * 2,
            handbrake: input.held(Action::Handbrake),
            // the packer's plain group fires a command on its press (0x41312F…0x4131E9): code 2
            // `C1_SHIFTUP`, 3 `C1_SHIFTDOWN`, 4 `C1_ABORT`, 6 `C1_AIDRIVE`, 10 / 11 the turn
            // signals, 12 `C1_SIREN` (the hazards), 13 `C1_LIGHTSON`; it walks them in code
            // order and each one pressed overwrites the code (0x413178), so the highest wins.
            // Code 5 `C1_RESET` is still taken straight from the input (the reset below) — except
            // in an input-exchange race, where it is one of the codes.
            command: [
                (Action::ShiftUp, 2u8),
                (Action::ShiftDown, 3),
                (Action::Abort, 4),
                (Action::Reset, 5),
                (Action::AiDrive, 6),
                (Action::SignalLeft, 10),
                (Action::SignalRight, 11),
                (Action::Hazards, 12),
                (Action::Lights, 13),
            ]
            .into_iter()
            .filter(|(a, _)| exchange || *a != Action::Reset)
            .fold(0, |code, (a, c)| if input.take_pressed(a) { c } else { code }),
        }
    }

    /// **The cars' cheat flags** `(RALLY [0x6573F8], HEAVY [0x6573FC], DERBY [0x657400])`: in a network race
    /// the host's, as its stream carried them (`net_flags`; 0x4B4B80 writes the three globals on every
    /// machine and nothing else does), otherwise this machine's own cheat bits 0, 8 and 7.
    fn launch_flags(&self) -> (bool, bool, bool) {
        self.net_flags.unwrap_or((self.cheat_bits & CHEAT_RALLY != 0, self.cheat_bits & CHEAT_HEAVY != 0, self.cheat_bits & CHEAT_DERBY != 0))
    }

    pub(crate) fn simulate(&mut self) {
        self.tick += 1;
        let (rally_on, heavy_on, derby_on) = self.launch_flags();
        // diagnostic: `NFS_HUD_KEYS` — a scripted HUD toggle press on its tick
        while let Some(&(t, a)) = self.hud_key_script.front()
            && t <= self.tick
        {
            self.hud_key_script.pop_front();
            if self.vehicle.is_some() && !self.paused {
                match a {
                    // a network race (`[0x6573E8] == 3`): 0x415110's rules — the notice, the second press leaves
                    ScriptedHudKey::Pause if self.fe.race_game_type == Some(game_type::NETWORK) => {
                        if self.net_pause_press() {
                            self.scripted_net_leave = true;
                        }
                    }
                    ScriptedHudKey::Pause | ScriptedHudKey::PauseRestart | ScriptedHudKey::PauseQuit => {
                        // the pause key's own arm (the keyboard handler's), with no dialog up
                        if self.results.is_none() {
                            self.paused = true;
                            self.orbit = Orbit::default();
                            self.pause_sel = 0;
                            self.pause_confirm = None;
                            self.scripted_pause = true;
                            self.scripted_restart = a == ScriptedHudKey::PauseRestart;
                            self.scripted_quit = a == ScriptedHudKey::PauseQuit;
                        }
                    }
                    ScriptedHudKey::Toggle(k) => self.hud_key(k),
                    ScriptedHudKey::Tap(k) => {
                        self.input.press(k);
                        self.input.release(k);
                    }
                    ScriptedHudKey::Pad(k, true) => self.input.press(k),
                    ScriptedHudKey::Pad(k, false) => self.input.release(k),
                }
            }
        }
        // 0x417010, the first call of `GameTick`'s odd-tick group (0x4150D5 `test al, 1` → 0x4150D9).
        // What it reads — the finish state and `+0xB14` — only the even tick's progress update
        // changes, so testing it here, before this odd tick's work, is the same test
        // diagnostic `NFS_RACE_PLACE=<p>:<tick>`: the player's car flagged finished at that tick,
        // as its line would (state 2, `+0xB14 = tick`), so a scripted race reaches race over and
        // its result pages
        if let Some(at) = race_finish_diagnostic()
            && self.tick >= at
            && self.replay.stream.as_ref().is_none_or(|s| !matches!(s.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
            && let Some(r) = self.race.as_mut().filter(|r| !r.finished)
        {
            r.finished = true;
            r.tick = self.tick;
            tracing::info!(tick = self.tick, "NFS_RACE_PLACE: the player flagged finished");
        }
        if self.tick % 2 == 1 {
            self.race_over_check();
        }
        if self.results.is_some()
            && let Some(page) = self.result_pages().get(self.results_page as usize).copied()
            && let (Some(limit), Some(t0)) = (Self::page_timeout(page, self.block_game_type() == Some(game_type::NETWORK)), self.page_open_tick)
            && self.page_seen == Some(self.results_page)
            && self.tick.saturating_sub(t0) > limit
        {
            self.page_open_tick = None;
            self.close_results_page();
        }
        if self.tick % 64 == 1
            && !self.road.opponents.is_empty()
            && let Some(t) = &self.slices
        {
            let laps = self.num_laps as f32;
            let m = |p: f32| (p.min(laps) * t.lap_len as f32 * 6.0).round_ties_even() as i32;
            self.hud_metres = std::iter::once(self.race.as_ref().map_or(0, |r| m(r.progress)))
                .chain(self.road.opponents.iter().map(|o| m(o.progress.progress)))
                .collect();
        }
        // the track's particle systems run on the 64 Hz clock (0x4689C0 → 0x4551E0)
        if self.track_particles.is_none()
            && !self.track_particle_defs.is_empty()
            && let Some(w) = self.world.as_ref()
        {
            let level = self.special_fx_level();
            let budget = [0, 100, 200, 256][level];
            let mut p = nfs_game::track_particles::TrackParticles::new(std::mem::take(&mut self.track_particle_defs), &w.emitters, self.traffic_seed, budget);
            // every car's effect set (0x43EE10 from each `R3DCar`'s constructor, 0x437A1E), made on
            // a car's first drive — the same empty, silent state the constructor leaves
            self.car_effects = Some(nfs_game::car_effects::CarEffects::new(std::mem::take(&mut self.car_effect_defs), budget));
            tracing::info!(emitters = w.emitters.len(), systems = p.systems(), flares = w.flares.len(), "track effects");
            // the weather layer, made by the track load (0x469092 → 0x458C90) and primed by the
            // pass (0x4690AF → 0x459260) with the new sky's values at slice 0 (`weather_strengths_at`)
            let (snow, rain) = self.weather_strengths_at(0);
            // (a restart or the replay primes the layer it kept: `weather_kept`)
            let wx = match self.weather_kept.take() {
                Some(mut wx) => {
                    wx.prime(snow, rain, p.rng_mut(), &mut self.fe_rng);
                    wx
                }
                None => nfs_game::weather::Weather::new(level, snow, rain, p.rng_mut(), &mut self.fe_rng),
            };
            tracing::info!(snow, rain, snow_n = wx.snow.count(), rain_n = wx.rain.count(), "weather");
            self.weather = Some(wx);
            self.track_particles = Some(p);
            // diagnostic: `NFS_EMITTER_VIEW=k` runs the systems 300 updates ahead so a still of
            // emitter k (see `rebuild_scene`) shows them going
            if nfs_game::config::cfg().nfs_emitter_view.as_str().is_some()
                && let Some(p) = self.track_particles.as_mut()
            {
                for _ in 0..300 {
                    p.draw(self.draw_distance, |_| Some((0.0, false)));
                    p.step(self.draw_distance);
                }
                // (the view's draw refreshes them: 0x455260 draws only a system at `+8 == 100`)
                p.draw(self.draw_distance, |_| Some((0.0, false)));
                let sp = p.sprites();
                tracing::info!(sprites = sp.len(), first = ?sp.first(), "emitter view");
            }
        }
        if let Some(p) = self.track_particles.as_mut() {
            p.step(self.draw_distance);
        }
        self.render_prev = Some((self.car_pos, self.car_rot));
        self.local_prev = Some(self.local_pose);
        // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the smackables' previous pose, render state only
        self.smackables.snapshot_prev();
        let steer4 = |v: &nfs_physics::vehicle::Vehicle| [v.wheels[0].steer, v.wheels[1].steer, v.steer_smoothed[0], v.steer_smoothed[1]];
        self.road.player_steer_prev = self.vehicle.as_ref().map(steer4);
        // (the previous-pose lists keep their buffers: cleared and refilled in the same order)
        self.road.opp_steer_prev.clear();
        self.road.opp_steer_prev.extend(self.road.opponents.iter().map(|o| steer4(&o.vehicle)));
        self.road.opp_prev.clear();
        self.road.opp_prev.extend(self.road.opponents.iter().map(|o| {
            (
                o.vehicle.draw_origin(),
                Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize(),
            )
        }));
        let net_seats = self.net_step();
        // the input exchange's samples for this tick (`net_step` latched them at an even tick)
        let exchange_in = self.exchange_in.take();
        // **Input exchange: registration order** (`network.md` §11.18). The port keeps the local car
        // first and the seats after it; the original's arrays (`0x5E8B34[]`, `0x5E8E2C[]`, `0x5E8B88[]`
        // …) are in the order the cars were created, by race index, the same on every machine. Empty
        // outside an exchange race, where `reg_of` is the identity.
        let exch_order: Vec<HumanCar> = exchange_in.as_ref().map(|x| x.humans(self.road.opponents.len())).unwrap_or_default();
        let n_seat_cars = self.road.opponents.len();
        // the same list from the session, for the odd tick's audio update (no latched samples then)
        let exch_humans_now: Vec<HumanCar> = self.exch_human_order();
        // a **positional** race's human cars in registration order, for the traffic's lists only
        let pos_order: Vec<HumanCar> = if exchange_in.is_none() { self.positional_human_order() } else { Vec::new() };
        // the seats' horns (`car+0xD86`) of a positional race: the block's bit 1, the local pad's before the
        // first block, 0 for a stale car (`SeatArm::horn`)
        let pos_seat_horns: Vec<bool> = match self.net.session.as_ref() {
            Some(n) if !pos_order.is_empty() => {
                let local_horn = self.input.held(Action::Horn);
                (0..self.road.opponents.len()).map(|k| n.seat_arm(k).horn(local_horn)).collect()
            }
            _ => Vec::new(),
        };
        let view = self.camera_view();
        // whether the camera key steps the replay's own nine ids (`replay.md` §8) — taken before
        // the car is borrowed
        let replay_camera = self.replay_view().is_some();
        let fx_marks = self.choice_index(choice::GRAPHICS_SPECIAL_FX).is_none_or(|(i, _)| i >= 2);
        // force feedback (output only, `RE/notes/force-feedback.md`): the five levels, read before the car is borrowed
        self.ff_levels = self.ff_levels();
        let ff_replay = self.replay.stream.as_ref().is_some_and(|s| matches!(s.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        // what the sections of the 32 Hz block read of the above (`TickCtx`)
        let ctx = TickCtx { rally_on, heavy_on, derby_on, ff_replay, view, replay_camera, net_seats, exchange_in, exch_order, n_seat_cars, pos_order, pos_seat_horns };
        let (net_seats, exchange_in) = (&ctx.net_seats, &ctx.exchange_in);
        // …and what one section hands on to a later one (`TickState`)
        let mut st = TickState::default();
        // the test glue: scripted placements of the cars (it borrows the car and the world itself)
        self.apply_tick_glue(exchange_in.as_ref());
        if self.tick % 2 == 0 {
            // the non-player cars' collision events of this tick, in the order they happen, for
            // their sound queues (`car+0x1110`, 0x40F740) — committed after the collision pass
            let mut ai_hits: Vec<(AudioCar, nfs_audio::sfx::QueuedSfx)> = Vec::new();
            self.tick_controls(&ctx, &mut st);
            // **The even tick, in the original's order** (`GameTick` 0x414FF0, `traffic.md` §17.5d):
            // `0x416140`, `Collide_beginTick` 0x498220, then **the AI pass `0x406720`** (0x4150A9),
            // then **the Sc32 list** (0x4150B4), then `0x417800`; the Sc64 list runs every tick.
            //
            // The AI pass walks `0x5E6774[]` / `0x5E8B34[]` in racer-slot order (0x40672C…0x406785)
            // — the humans, the opponents (slot `nPlayers + i`), the cops (from `nPlayers + nOpp`,
            // 0x4ED103 / 0x4ED1ED), the traffic — and calls each `AIHigh`'s update (`call [vt+0]`,
            // 0x40677B). An `AIHigh_Opponent` runs its physics inside it: `AIState_Racing::update`
            // 0x40CA20 writes the input bytes and calls `Car_simulate` 0x49FD30 (0x40CA6F, its only
            // call site), and its Sc32 slots `+0xE0C` / `+0xE1C` hold 0x408360, which returns at once
            // for a racer (0x408366). So an opponent's decision and step stay together, in slot
            // order. Everything else only decides here: the human's perp update (0x405690 →
            // `tickHeat` 0x405D70), a cop's `AIHigh_Cop::update` 0x403E40 and a traffic car's
            // `AIHigh_Traffic::update` 0x406020 write targets, and their motion is on the Sc32 list.
            //
            // The Sc32 list (`Cars_create` 0x411277…0x411349 inserts each car's entries in slot order,
            // `0x414C30` puts an entry after every one of equal priority): priority 6 `+0xE0C`
            // (0x49AF70, the tracker), 0x15 `+0xE14` (the human's controller copy 0x4991E0), 0x1E
            // `+0xE1C` then `+0xE20` per car (the human's `Car_simulate` and `Car_ground`; the
            // cops' and the traffic's drive 0x408360), 0x28 `+0xE2C` (the collision pass 0x498FF0),
            // 0x32 `+0xE30` (0x410280, the post-impact pass). So the player steps after every AI
            // decision of the tick, which therefore sees the player as the tick began.
            //
            self.tick_ai_opponents(&ctx, &mut st);
            self.tick_ai_police(&ctx);
            self.tick_ai_traffic(&ctx, &mut st);
            self.tick_player_step(&ctx, &st);
            let mut ground_event = None;
            self.tick_step_sounds(&mut ground_event);
            self.tick_trackside_and_skill(&ctx);
            self.tick_cop_and_traffic_drive(&st);
            // The collision pass 0x498FF0 (priority 0x28 on the 32 Hz list, behind the drive (6) and
            // the ground contact (0x1E) of all cars, `traffic.md` §21.5): car by car in `0x5E8B34`
            // order — the human, the opponents, the cops, the traffic (0x4ECF00) — each car's own
            // walls (0x495020) and props (0x498DE0) behind `car+0x7C` (every body's, 0x49B525), then
            // its pairs with every car already done this tick (`0x628CDC[]`, 0x4980D0's first round
            // per pair — `nfs_physics::vehicle::collide_pair`), then the car joins the list.
            // *(Until 2026-09-26 the port collided the human's and the opponents' walls in their own
            // steps, ran the opponent pairs after the cops' and the traffic's, gave the traffic no
            // walls and no pairs among themselves — the "approximation of the pass itself".)*
            let ci = self.collision_inputs(&ctx);
            self.tick_collide_racers(&ctx, &ci, &mut ai_hits, &mut ground_event);
            self.tick_collide_police(&ctx, &ci, &mut ai_hits, &mut ground_event);
            self.tick_collide_traffic(&ctx, &ci, &mut ai_hits, &mut ground_event);
            self.tick_collide_smackables(&ctx, &ci, &mut ai_hits, &mut ground_event);
            self.tick_player_contact(&ctx, ground_event);
            self.tick_ai_sound_queues(&mut ai_hits);
            self.tick_ai_sparks();
            self.tick_player_reset(&ctx, &mut st);
            self.tick_opponent_resets(&ctx, &st);
            self.tick_traffic_post_impact(&ctx);
            self.tick_race_order(&ctx);
            self.tick_quick_knockout(&st);
            self.tick_derby_outcomes(&ctx);
            self.tick_race_progress(&ctx);
            self.tick_camera_key(&ctx);
        }
        if self.world.is_none() {
            return;
        }
        let Some(v) = self.vehicle.as_mut() else {
            return;
        };
        // police move on the same 64 Hz integrator as traffic
        if let (Some(pol), Some(t)) = (self.road.police.as_mut(), self.slices.as_ref()) {
            self.road.police_prev.clear();
            self.road.police_prev.extend(pol.cars.iter().map(|c| {
                (
                    c.draw_origin(),
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                )
            }));
            // the ground contact is the 32 Hz `Police::ground`; without a world the slice's height
            pol.integrate_tick(t, 1.0 / 64.0, self.world.is_some(), Some(self.tick));
        }
        // traffic is integrated at 64 Hz from the velocity the AI wrote on the last 32 Hz tick
        if let (Some(tr), Some(t)) = (self.road.traffic.as_mut(), self.slices.as_ref()) {
            self.road.traffic_prev.clear();
            self.road.traffic_prev.extend(tr.cars.iter().map(|c| {
                (
                    c.draw_origin(),
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                )
            }));
            // `[0x5B2828]`, the integrator's fixed dt; the collision grid gives the cars the ground
            // contact `Car_installHandlers` leaves them with (`car+0xE20`, §14.1)
            let grid = self.world.as_ref().map(|w| &w.collision);
            tr.integrate_tick(t, 1.0 / 64.0, grid, Some(self.tick));
            self.road.traffic_spin.resize(tr.cars.len(), 0.0);
            for (sp, c) in self.road.traffic_spin.iter_mut().zip(tr.cars.iter()) {
                *sp = if c.active {
                    *sp + c.speed / TICKS_PER_SEC_F32 / 0.32
                } else {
                    0.0
                };
            }
        }
        v.step64(&self.specs);
        let exchanging = self.net.session.as_ref().is_some_and(|n| n.exchanging());
        for (oi, o) in self.road.opponents.iter_mut().enumerate() {
            o.vehicle.step64(&o.specs);
            // **0x49B8E0's network hook** (0x49B9AB…0x49B9EC), after the orientation step: a remote
            // car of a positional race, whose class 0x490140 is at most 2 (a player who has left keeps
            // its car where it stopped) — `NetPlay::apply` holds the other gates (`car+0xDAC`, `+0x70`)
            if net_seats.contains(&oi)
                && !exchanging
                && let (Some(n), Some(w)) = (self.net.session.as_mut(), self.world.as_ref())
                && !n.seat_race_index(oi).is_some_and(|i| n.link_quality(i, self.tick as u32).gone())
            {
                // the retrack's walk goes toward the car's slice centre (the tracker's slice, `slice + 4`)
                let centre = self.slices.as_ref().and_then(|t| t.slices.get(o.progress.slice)).map(|sl| sl.centre);
                n.apply(oi, &mut o.vehicle, &o.specs, &w.collision, centre, self.tick);
                remote_dump(self.tick, oi, &o.vehicle, n);
            }
            if self.tick % 32 == 0 {
                o.vehicle.renormalise();
            }
        }
        // the skid marks, per wheel (0x4A2880 from the 64 Hz position step 0x49B84F, which
        // skips only the smackables, `car+0x52C & 0x80` — the traffic and the cops run it too,
        // `ai.md` §6 corrected 2026-09-25). **Every tick, after the car's move**: 0x49B7A0 is the
        // Sc64 integrator (`car+0xE24`), and it adds `vel/64` to the body and the four contacts
        // `wheel+0x40` (0x49B7F5…0x49B84C) before it calls 0x4A2880 — so the emitters' counter, the
        // commit every eighth call and the head pair all run at 64 Hz. *(Until 2026-09-26 the port
        // fed them on the even tick only, before the move: half the calls, a pair every 1/4 s.)*
        // The cars' order is the Sc64 list's at priority 0x1E — `+0xE24` inserted car by car in
        // `0x5E8B34` order (`Cars_create` 0x411277…0x411349, 0x414C30 after every equal
        // priority, `traffic.md` §17.5e): the human, the opponents, the cops, the traffic, as
        // here; the traffic's and the cops' contacts are moved by `vel/64` in their own
        // integrators (`TrafficSet::integrate_tick`, `Police::integrate_tick`). The emitter 0x478F30 is gated by
        // `byte [0x6197E7]`, which 0x44ED10 sets only when `SPECIALFX` is 2 or 3 (0x44EE93,
        // 0x44EEDC; `fe.txt` ships 3) — `fx_marks`, above. The point is `wheel+0x40`, the ground
        // contact `(probe.x, h_i, probe.z)` (`suspension-collision.md` §2) — the port's hub
        // position less its ride height. The traffic's and the cops' wheels are their bodies'
        // (`Car_ground` on `TrafficSet::bodies` / `CopCar::body`, 2026-09-26): their zone word
        // and contact come from `Susp_update`, their slip `wheel+0xB8` stays 0 (only
        // `Car_simulate` 0x49FD30 writes it, which they do not run — `sfx.md` §4.2), so they
        // mark only the kinds 0x4A2880 emits regardless of slip (surface 9, water / wet 1)
        if fx_marks {
            let feed = |skid: &mut nfs_game::skidmarks::SkidMarks, base: usize, car: &nfs_physics::vehicle::Vehicle, tyre_mm: f32| {
                // `car+0x348`, the travel direction `Car_ground` keeps (the forward row at rest)
                let dir = car.travel_dir;
                for (i, w) in car.wheels.iter().enumerate() {
                    skid.set_width(base + i, nfs_game::skidmarks::half_width(tyre_mm, true));
                    // 0x4A2880 reads the wheel's whole zone word (`wheel+0x8C`) — `== 9`, `== 6`, `== 1`,
                    // and hands it on as the kind, which 0x478F30 masks `& 0xF` only to index its
                    // colours (0x478F99)
                    let (strength, kind, a, b) = nfs_game::particles::wheel_input(i < 2, w.zone as usize, w.wet, w.slip);
                    let ground = w.world_pos - Vec3::Y * car.ride_height;
                    skid.emit(base + i, ground, strength, kind, dir, a, b);
                }
            };
            feed(&mut self.skid, 0, v, self.specs.tyre_width_mm);
            for (k, o) in self.road.opponents.iter().enumerate() {
                feed(&mut self.skid, 4 + 4 * k, &o.vehicle, o.specs.tyre_width_mm);
            }
            let mut base = 4 + 4 * self.road.opponents.len();
            if let Some(pol) = &self.road.police {
                for c in pol.cars.iter() {
                    if c.active {
                        feed(&mut self.skid, base, &c.body, c.specs.tyre_width_mm);
                    }
                    base += 4;
                }
            }
            if let Some(tr) = &self.road.traffic {
                let tyre = tr.specs.as_ref().map_or(self.specs.tyre_width_mm, |s| s.tyre_width_mm);
                for (c, b) in tr.cars.iter().zip(tr.bodies.iter()) {
                    if c.active {
                        feed(&mut self.skid, base, b, tyre);
                    }
                    base += 4;
                }
            }
        }
        self.smackables.integrate(self.tick);
        if self.tick % 32 == 0 {
            v.renormalise();
        }
        self.car_pos = v.draw_origin();
        self.car_rot = Quat::from_mat3(&v.orientation.transpose()).normalize();
        // a network replay's viewed car (`[0x606858]`): the camera follows that human's car, not the recorded
        // machine's own. The recorded machine's own car keeps its pose for the draw (`local_pose`): whether
        // the "followed car" arms of the car model (0x465D80(0)) follow the viewed car is [`Self::followed_seat`].
        // The HUD's gauges keep reading the recorded machine's car, as the original's do: the readers of the
        // viewed car `[0x606858]` are the trackside camera sweep 0x413BFE, the commands 9 / 10 (0x414953,
        // 0x4149CF), their writers 0x413D09 / 0x413E6D / 0x413F86 / 0x413FB3 and the followed-car test 0x465DA0;
        // every gauge reads `[0x65741C]` (`replay.md` section 18.15) — settled, AC 2026-09-30.
        self.local_pose = (self.car_pos, self.car_rot);
        if let Some(k) = Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view)
            && let Some(o) = self.road.opponents.get(k)
        {
            self.car_pos = o.vehicle.draw_origin();
            self.car_rot = Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize();
        }
        // the pose this tick's camera update would read (`Sc64`, 0x463C30), with the viewed
        // car's body lift the draw adds (`viewed_lift`)
        {
            let lift = match Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view) {
                Some(k) => self.road.opponents.get(k).map_or(0.0, |o| o.vehicle.body_lift),
                None => v.body_lift,
            };
            let (slice, wrong_way) = self.race.as_ref().map_or((0, false), |r| (r.slice, r.wrong_way_ticks > 0));
            self.cam_hist.push_back(CamTick {
                tick: self.tick,
                pos: self.car_pos + self.car_rot * Vec3::Y * lift,
                rot: self.car_rot,
                v_local: v.v_local,
                half_z: v.half_extents.z,
                slice,
                wrong_way,
                velocity: v.velocity,
                cos_tilt: v.cos_tilt,
            });
        }
        // (a network race played back from a file dumps the same rows, its lanes' race indices standing for
        // the session's: `NFS_NET_DUMP` proves the playback against the live race)
        if self.net_dump.is_none()
            && self.replay_net.is_some()
            && let Some(path) = nfs_game::config::cfg().nfs_net_dump.as_str()
            && let Ok(f) = std::fs::File::create(&path)
        {
            self.net_dump = Some(std::io::BufWriter::new(f));
            use std::io::Write;
            if let Ok(mut sf) = std::fs::File::create(format!("{path}.specs"))
                && let Some(rn) = self.replay_net.as_ref()
            {
                let mut cars: Vec<(u32, String)> = vec![(rn.local_index, format!("{:#?}", self.specs))];
                for (k, o) in self.road.opponents.iter().enumerate() {
                    if let Some(&i) = rn.seats.get(k).filter(|i| **i != u32::MAX) {
                        cars.push((i, format!("{:#?}", o.specs)));
                    }
                }
                cars.sort_by_key(|c| c.0);
                for (i, t) in cars {
                    let _ = writeln!(sf, "=== car {i}\n{t}");
                }
            }
        }
        let dump_index: Option<(Option<u32>, Vec<Option<u32>>)> = match (self.net.session.as_ref(), self.replay_net.as_ref()) {
            (Some(n), _) => Some((n.local_race_index(), (0..self.road.opponents.len()).map(|k| n.seat_race_index(k)).collect())),
            (None, Some(r)) => Some((Some(r.local_index), r.seats.iter().map(|&i| (i != u32::MAX).then_some(i)).collect())),
            (None, None) => None,
        };
        // (test glue: every car's whole `Vehicle` after the first tick, to diff two machines' or a replay's)
        if self.tick == 1
            && self.net_dump.is_some()
            && let Some((local, seat_index)) = dump_index.as_ref()
            && let Some(path) = nfs_game::config::cfg().nfs_net_dump.as_str()
            && let Ok(mut f) = std::fs::File::create(format!("{path}.t1"))
        {
            use std::io::Write;
            let mut cars: Vec<(u32, String)> = Vec::new();
            if let Some(i) = local {
                cars.push((*i, format!("{v:#?}")));
            }
            for (k, o) in self.road.opponents.iter().enumerate() {
                if let Some(i) = seat_index.get(k).copied().flatten() {
                    cars.push((i, format!("{:#?}", o.vehicle)));
                }
            }
            cars.sort_by_key(|c| c.0);
            for (i, t) in cars {
                let _ = writeln!(f, "=== car {i}\n{t}");
            }
        }
        if let Some(out) = self.net_dump.as_mut()
            && let Some((local, seat_index)) = dump_index
        {
            Self::net_dump_tick(out, local, &seat_index, v, &self.road.opponents, self.tick);
            // **Test glue:** the traffic cars (rows `100 + i`, the body's state) and the shared LCG's
            // state (row 900), so two machines' agreement covers what the traffic AI drew
            if let Some(tr) = self.road.traffic.as_ref() {
                use std::io::Write;
                for (i, (c, b)) in tr.cars.iter().zip(tr.bodies.iter()).enumerate() {
                    let mut f: Vec<f32> = Vec::new();
                    f.extend(b.position.to_array());
                    f.extend(b.velocity.to_array());
                    f.extend(b.orientation.to_cols_array());
                    f.extend(b.angular_velocity.to_array());
                    f.push(b.rpm);
                    f.push(if c.active { 1.0 } else { 0.0 });
                    let bits: Vec<String> = f.iter().map(|x| format!("{:08x}", x.to_bits())).collect();
                    let _ = writeln!(out, "{},{},{},{}", self.tick, 100 + i, bits.join(","), c.slice);
                }
            }
            {
                use std::io::Write;
                let _ = writeln!(out, "{},900,{:08x},0", self.tick, nfs_game::rng::state());
            }
        }
        // **The audio update `0x4A8120` runs on the odd tick only** — `GameTick` 0x414FF0 calls it
        // at 0x4150E3 behind `test byte [gGameTick], 1 ; je` (0x4150D5), after the even tick's
        // physics and AI (`timing.md` §7): 32 Hz. Everything below is that update — the slot table
        // 0x4A7DC0, the slot cars 0x4A7080 (engines, honk timers, sirens, queue drains), the
        // player 0x4A7660 (engine, horn, loops, the one-shot gate's decay 0x4AA056, the ambience
        // 0x4AEF30 its `(tick >> 1) & 3` quarter assumes) — so its per-update counters (the ±8
        // layer slew, the gate, the reverb's 100 → 68 → 36 → 8 fade, the honk timers) run at the
        // original's rate. *(Until 2026-09-26 the port ran it on every 64 Hz tick: twice as fast.)*
        if self.tick % 2 == 0 {
            self.replay_tick();
            return;
        }
        self.simulate_audio(view, exch_humans_now);
        self.replay_tick();
    }

    /// **TEST GLUE** `NFS_AUTOPILOT=ai`: an opponent's driver for the player's car — its `.ais`, its `.sim` tail and
    /// the steering family of its `nfs5.car` record, as `race_stages` builds one for an opponent (default skill)
    fn make_player_ai(&self) -> Option<Box<nfs_game::ai::AiDriver>> {
        let ais = CarAssets::load_ais(&self.paths, &self.car.model).ok()?;
        let sim = CarAssets::load_sim(&self.paths, if self.sim_name.is_empty() { "default" } else { &self.sim_name }).ok()?;
        let id = self.player_car_id.unwrap_or(self.car_id);
        let family = self.fe_car_catalog().into_iter().find(|e| e.id == id).map(|e| e.sim_family).unwrap_or_default();
        let mut d = nfs_game::ai::AiDriver::new(ais, &sim.unknown_tail);
        d.set_track(&self.track.name, self.backwards, &family);
        tracing::info!(model = self.car.model, family, "NFS_AUTOPILOT=ai: the opponents' driver drives the player's car");
        Some(Box::new(d))
    }

    /// **The 32 Hz block's controller latch** (see [`Self::simulate`]): the car's flags for the tick, the countdown speech, the controller sample the car reads (this machine's own, the exchange's, the replay's), and the lamp bytes. Moved out of `simulate` unchanged; the sample goes to `st.controls`.
    fn tick_controls(&mut self, ctx: &TickCtx, st: &mut TickState) {
        if self.player_ai.is_none() && self.follow_road && nfs_game::config::cfg().nfs_autopilot.as_str() == Some("ai") {
            self.player_ai = self.make_player_ai();
        }
        let Some(v) = self.vehicle.as_mut() else { return };
        let (rally_on, derby_on) = (ctx.rally_on, ctx.derby_on);
        let (exchange_in, exch_order, net_seats) = (&ctx.exchange_in, &ctx.exch_order, &ctx.net_seats);
        // 32 Hz: latch controller, run the force step (Sc32)
        // before the start tick the original forces full brake and no throttle (0x49DD20)
        v.race_started = self.tick >= RACE_START_TICK;
        v.game_tick = self.tick;
        v.rally = rally_on;
        v.tiny = self.cheat_bits & CHEAT_TINY != 0;
        // `DERBY` `[0x657400]`, which only the Smash Up code sets (cheat bit 7): `Damage` takes ×1.5, not ×0.5 (0x49452F)
        // (a network race's `DERBY` is the launch's, the same on every machine: a joiner has no cheat of its own)
        v.derby = derby_on;
        v.race_state = if exchange_in.is_some() && self.player_eliminated {
            // `car+0xB50 = 3` (0x416310, `retired`), which every machine's copy of this car carries
            RaceState::Out
        } else if self.race.as_ref().is_some_and(|r| r.finished) {
            RaceState::Finished
        } else {
            RaceState::Racing
        };
        // countdown speech (Phrase_kCountdown: SOR-a-3/2/1/Go at ticks 256/320/384/448)
        let phase = COUNTDOWN_TICKS
            .iter()
            .chain(std::iter::once(&RACE_START_TICK))
            .filter(|&&t| self.tick >= t)
            .count() as u8;
        if phase > self.snd.countdown_said
            && let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech)
        {
            self.snd.countdown_said = phase;
            sp.say_countdown(
                a,
                ["SOR-a-3.bnk", "SOR-a-2.bnk", "SOR-a-1.bnk", "SOR-a-Go.bnk"]
                    [(phase - 1) as usize],
            );
        }
        // The controller as the car reads it: this machine's own sample (0x412E70's packer, the
        // helper below) — or, in an input-exchange race, what the frame driver latched for this car
        // from the room's frame (0x415EF0 → 0x413870 → 0x4992CF), which is the same sample sixteen
        // times a second, averaged in pairs and quantised, `network.md` §11.17
        let controls = match (exchange_in.as_ref(), self.player_ai.as_mut(), self.slices.as_ref(), self.race.as_ref()) {
            (Some(x), ..) => x.local,
            // **TEST GLUE** `NFS_AUTOPILOT=ai`: the opponents' driver steers the player's car (see `player_ai`), with
            // the assists an AI car runs with (`ai_driven`, traction control) and its gear-shift draw
            (None, Some(d), Some(t), Some(r)) => {
                let c = d.controls(t, self.racing_line.as_ref(), r, v, &self.specs, self.tick, &[], v.half_extents.x);
                v.ai_driven = true;
                v.traction_control = true;
                v.shift_roll = d.shift_roll.take();
                c
            }
            (None, ..) => Self::controller_sample(&mut self.input, self.mirror, self.follow_road, self.slices.as_ref(), self.race.as_ref(), v, false),
        };
        // **TEST GLUE** (not in the original): `NFS_SKID_SCRIPT=<from_s>:<for_s>:<steer −1…1>` holds the
        // handbrake with full throttle and that steering from `from_s` for `for_s` seconds of the
        // race clock (`tick / 64`), so a scripted drive leaves real skid marks for stills
        let controls = match nfs_game::config::cfg().nfs_skid_script.as_str().and_then(|v| {
            let f: Vec<f32> = v.split(':').filter_map(|x| x.parse().ok()).collect();
            (f.len() == 3).then(|| (f[0], f[1], f[2]))
        }) {
            Some((from, len, steer)) if (from..from + len).contains(&(self.tick as f32 / TICKS_PER_SEC_F32)) => Controls { throttle: 252, handbrake: true, steer: ((steer.clamp(-1.0, 1.0) * 126.0) as i32 / 2).clamp(-63, 63) * 2, ..controls },
            _ => controls,
        };
        // replay.md §4: the hook sits inside the 32 Hz input step, before the car reads the
        // packet, so recording and playback are in phase with the simulation by construction
        let controls = match self.replay.stream.as_mut() {
            // **An input-exchange race records every human car** (`0x4143F0` per car, `car+0x524` =
            // the race index): the raw sample each car reads, into its own lane, in race-index
            // order — the local car is one of them, not lane 0 (`replay.md` §18.15). A network
            // replay's samples came from the lanes already (`net_step`), nothing to hook.
            Some(r) if exchange_in.is_some() => {
                if let Some(x) = exchange_in.as_ref()
                    && matches!(r.mode, nfs_game::replay::Mode::Recording | nfs_game::replay::Mode::Full)
                {
                    let (steer, gas, brake) = self.input.ramps();
                    for h in exch_order {
                        let (idx, raw, ramp) = match *h {
                            HumanCar::Local => (x.local_index, x.local_raw, nfs_game::replay::Ramp { steer, gas, brake }),
                            HumanCar::Seat(k) => match x.seats.iter().position(|e| e.0 == k) {
                                Some(j) => (x.seats[j].1, x.seat_raw[j], nfs_game::replay::Ramp::default()),
                                None => continue,
                            },
                        };
                        r.record_player(idx as usize, nfs_game::replay::Packet { buttons: raw[0], steer: raw[1] as i8, throttle: raw[2], brake: raw[3] }, ramp, self.tick);
                    }
                }
                controls
            }
            // `.RAMPS` / `.RAMPG` / `.RAMPB` are `kind != 1` of the `C1_RIGHT`, `C1_GAS` and
            // `C1_BRAKE` bindings (0x51CF54…0x51CFFC) — set for keys, clear for an axis
            Some(r) => r.step(
                controls,
                {
                    let (steer, gas, brake) = self.input.ramps();
                    nfs_game::replay::Ramp { steer, gas, brake }
                },
                self.tick,
            ),
            None => controls,
        };
        // `Car_applyInput` switches on the packet's code for the lamps (0x49933E, jump table
        // 0x499480) — the recorded code too, so a replay blinks as the race did
        self.lamp_bits = lamp_command(self.lamp_bits, controls.command);
        // `car+0xD8A`, the packed pad byte the block's `+0x03` carries (0x499216 → 0x48EE20): the handbrake
        // bit, the horn bit and the command code of this even tick's sample
        self.net_pad_buttons = nfs_game::replay::Packet::from_controls(&controls).buttons | (u8::from(self.input.held(Action::Horn)) << 1);
        // **The other human cars' lamp bytes.** An input-exchange seat runs `Car_applyInput` on its own
        // latched sample, so its own command switches its lamps; a remote car of a positional race
        // takes the switch too, but 0x4991E0 leaves `car+0xD8B` at `[0x5E9664] >> 3` (0x499336, the
        // **local** pad's command) in all three of its arms — so the local player's lamp keys switch the
        // remote cars' lamps as well, whatever they are doing (the arms differ only above 0x499336)
        if let Some(x) = exchange_in.as_ref() {
            for &(oi, _, c) in &x.seats {
                if let Some(o) = self.road.opponents.get_mut(oi) {
                    o.lamp_bits = lamp_command(o.lamp_bits, c.command);
                }
            }
        } else if self.net.session.is_some() {
            for &oi in net_seats {
                if let Some(o) = self.road.opponents.get_mut(oi) {
                    o.lamp_bits = lamp_command(o.lamp_bits, controls.command);
                }
            }
        }
        st.controls = controls;
    }

    /// **The AI pass for the opponents** (see [`Self::simulate`]): each opponent's controller, `Car_simulate` and lap tracker, in slot order; the remote seats of a positional race are collected for their step later in the tick. Moved out of `simulate` unchanged; `st.ko_laps` and `st.remote_steps` are its outputs.
    fn tick_ai_opponents(&mut self, ctx: &TickCtx, st: &mut TickState) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let (rally_on, derby_on) = (ctx.rally_on, ctx.derby_on);
        let (exchange_in, net_seats) = (&ctx.exchange_in, &ctx.net_seats);
        let reg_of = |s: usize| ctx.reg_of(s);
        let controls = st.controls;
        // AI pass: opponents (parked controller until the AI rules land)
        let mut ko_laps: Vec<(usize, u32)> = Vec::new();
        // the remote cars of a positional race whose `Car_simulate` runs this tick on the wire's controls
        let mut remote_steps: Vec<(usize, u32, Controls)> = Vec::new();
        if let Some(t) = &self.slices {
            // race order for the avoidance rule: `0x5E8B88`, sorted ascending by
            // `car+0xAF8 − car+0xB20` (0x417710) — the progress **into the lap**, laps timed
            // taken off, so a lapping leader sits among the cars it is physically near. For
            // traffic `+0xB20` stays 0 and `+0xAF8` is its slice's lap fraction
            // (`traffic.md` §17.5c). *(Until the audit of 2026-09-26 the port sorted the racers
            // on their whole progress, laps included, next to the traffic's lap fraction.)*
            // Entries: (registration index, opponent index, the `AheadCar` the AI reads). The human
            // is registered first (`0x5E8B34[0]`), then the opponents, the cops, the traffic;
            // the order is the places `nfs_game::race_order::RaceOrder` gave at the end of the
            // previous even tick (0x417800 → 0x417710's stable bubble sort), ties included.
            type AheadEntry = nfs_game::ai::AheadCar;
            // `0x5E8B88[]` as the previous even tick's `0x417710` left it (`nfs_game::race_order`):
            // every registered car — the human, the opponents, the cops, the traffic — in the
            // array's order, each with what the avoidance reads off it. *(Until 2026-09-26 this
            // was a fresh sort on this tick's progress, without the cops.)*
            let n_opp = self.road.opponents.len();
            let n_cops = self.road.police.as_ref().map_or(0, |p| p.cars.len());
            let mut order: Vec<(usize, Option<usize>, AheadEntry)> = Vec::new();
            if let Some(r) = &self.race {
                order.push((0, None, AheadEntry { position: v.position, slice: r.slice, v_long: v.v_long(), half_width: v.half_extents.x, human: true }));
            }
            order.extend(self.road.opponents.iter().enumerate().map(|(k, o)| {
                (
                    1 + k,
                    Some(k),
                    AheadEntry {
                        position: o.vehicle.position,
                        slice: o.progress.slice,
                        v_long: o.vehicle.v_long(),
                        // `o.hw` = `car+0x3A8` (0x40C571), the model box's half width
                        half_width: o.vehicle.half_extents.x,
                        human: false,
                    },
                )
            }));
            // traffic is in the race-order array too (`traffic.md` §8), so the opponents' "two cars
            // directly ahead" rule sees it and steers around it
            if let Some(pol) = &self.road.police {
                order.extend(
                    pol.cars
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.active)
                        .map(|(i, c)| (1 + n_opp + i, None, AheadEntry { position: c.position, slice: c.slice, v_long: c.body.v_long(), half_width: c.body.half_extents.x, human: false })),
                );
            }
            if let Some(tr) = &self.road.traffic {
                order.extend(
                    tr.cars
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.active)
                        .map(|(j, c)| (1 + n_opp + n_cops + j, None, AheadEntry { position: c.position, slice: c.slice, v_long: c.speed * c.dir, half_width: c.half_extents.x, human: false })),
                );
            }
            order.sort_by_key(|e| self.race_order.place(reg_of(e.0)));
            // catch-up band (`ai.md` §5.2 rule 1): the human's progress and average pace
            let (player_progress, player_pace) = match &self.race {
                Some(r) => (
                    r.progress,
                    if self.tick > 0 {
                        (r.progress - r.start_progress) * 640_000.0 / self.tick as f32
                    } else {
                        0.0
                    },
                ),
                None => (0.0, 0.0),
            };
            // every racer's position: index 0 is the human, then the opponents in order
            let racer_positions: Vec<Vec3> = std::iter::once(self.car_pos)
                .chain(self.road.opponents.iter().map(|o| o.vehicle.position))
                .collect();
            // the avoidance reads the other cars **live** — `o+0x330`, `o+0xD60` at 0x40C31A /
            // 0x40C4E0 — and every opponent's `AIState_Racing::update` runs its own
            // `Car_simulate` (0x40CA6F), so the ones updated earlier this tick are seen where
            // they have just moved to. Kept here per opponent: (position, v_long).
            let mut live: Vec<(Vec3, f32)> = self.road.opponents.iter().map(|o| (o.vehicle.position, o.vehicle.v_long())).collect();
            for (oi, o) in self.road.opponents.iter_mut().enumerate() {
                // the stale remote car's block, for 0x4A0680 after the AI arm (`car+0x70 == 1`)
                let mut stale_arm: Option<nfs_physics::vehicle::NetBlock> = None;
                o.vehicle.race_started = self.tick >= RACE_START_TICK;
                // `car+0xB50`: 2 once the opponent has finished, 3 when it is knocked out —
                // which the AI's stuck counter (0x40C92B), the reset gate (0x40F7EB) and
                // `Damage` (0x494521) read. The port never set it for an opponent until the
                // audit of 2026-09-26, so a finished opponent kept taking zone damage.
                o.vehicle.race_state = if o.eliminated {
                    RaceState::Out
                } else if o.progress.finished {
                    RaceState::Finished
                } else {
                    RaceState::Racing
                };
                o.vehicle.game_tick = self.tick;
                o.vehicle.rally = rally_on;
                o.vehicle.tiny = o.tiny;
                // `DERBY` `[0x657400]`, which only the Smash Up code sets (cheat bit 7): `Damage` takes ×1.5, not ×0.5 (0x49452F)
                o.vehicle.derby = derby_on;
                if net_seats.contains(&oi) {
                    // an input-exchange seat is a human car: it steps beside the player's own
                    // (below) and its tracker runs with the player's, after the tick's physics
                    if exchange_in.is_some() {
                        continue;
                    }
                    // **A remote car of a positional race** — a human car (`car+0xDA8 == 0`) whose
                    // controller copy 0x4991E0 takes one of three arms (`NetPlay::seat_arm`):
                    // the **local pad** until the first block has come (`car+0xDAC == 0`,
                    // 0x4992CF…0x499330), the last block's steering / brake / handbrake (0x499285…
                    // 0x4992CD; the throttle byte is not written and keeps the last one), or — once the
                    // 64 Hz gate 0x49B9F1 has set `car+0xDA8 = 1` for a stale car (`car+0x70`) — the AI
                    // arm: 0x40B650 / 0x40BFE0 (the ordinary opponent code below) and then 0x4A0680.
                    // `car+0xD8B` is the local pad's command in all three (0x499336); the tick's 64 Hz
                    // apply (`NetPlay::apply`) puts the car back on the packet's track.
                    let arm = self.net.session.as_ref().map_or(netplay::SeatArm::Pad, |n| n.seat_arm(oi));
                    if !matches!(arm, netplay::SeatArm::Ai { .. }) {
                        let c = match arm {
                            netplay::SeatArm::Block { brake, steer, handbrake, .. } => {
                                Controls { throttle: o.vehicle.throttle_raw, brake, steer, handbrake, command: controls.command }
                            }
                            _ => controls,
                        };
                        o.vehicle.ai_driven = false;
                        o.vehicle.traction_control = false;
                        if matches!(arm, netplay::SeatArm::Pad) && self.tick % 64 == 0 {
                            tracing::debug!(target: "netplay", seat = oi, tick = self.tick, pos = ?o.vehicle.position, throttle = c.throttle, steer = c.steer, "remote seat before its first block: driven by the local pad (0x4992CF)");
                        }
                        // the race index the slot list (`Cars_create`) orders the cars by
                        let idx = self.net.session.as_ref().and_then(|n| n.seat_race_index(oi)).unwrap_or(u32::MAX);
                        remote_steps.push((oi, idx, c));
                        o.progress.update_tracked(
                            t,
                            o.vehicle.position,
                            o.vehicle.forward(),
                            self.tick,
                            self.num_laps,
                            None,
                        );
                        continue;
                    }
                    o.vehicle.ai_driven = true;
                    o.vehicle.traction_control = true;
                    stale_arm = match arm {
                        netplay::SeatArm::Ai { block, stale: true } => Some(block),
                        _ => None,
                    };
                }
                if let Controller::Ai(d) = &mut o.controller {
                    d.set_catchup(self.race_skill, player_progress, player_pace);
                    // 0x40BAF0…0x40BB13: `[0x606AE0]` (autobahn) and nothing overhead
                    d.wet = self.track.name.eq_ignore_ascii_case("autobahn") && w.overhead_flags(o.vehicle.position) == 0;
                }
                // the two entries after the car's own index `car+0x528` (0x40C2CC…0x40C2E7:
                // `j = min(idx + 2, n − 1)` down to `idx + 1`), whatever their keys
                let ahead: Vec<AheadEntry> = match order.iter().position(|e| e.1 == Some(oi)) {
                    Some(i) => order[i + 1..(i + 3).min(order.len())]
                        .iter()
                        .map(|e| match e.1 {
                            Some(k) => AheadEntry { position: live[k].0, slice: e.2.slice, v_long: live[k].1, half_width: e.2.half_width, human: e.2.human },
                            None => e.2,
                        })
                        .collect(),
                    None => Vec::new(),
                };
                let mut c =
                    o.controls_for(&controls, t, self.racing_line.as_ref(), self.tick, &ahead);
                // **0x4A0680** after the AI arm of a stale remote car (0x499232…0x499252): the progress
                // error against the block either puts the car back on the road or brakes it
                if let Some(block) = stale_arm {
                    let preferred = match &o.controller {
                        Controller::Ai(d) => d.preferred_segment,
                        _ => None,
                    };
                    match nfs_game::netcar::stale_place(t, &block, self.tick as i32, o.progress.slice, preferred, o.vehicle.velocity, o.progress.progress) {
                        nfs_game::netcar::StalePlace::Left => {}
                        nfs_game::netcar::StalePlace::PutOnRoad { forward, along } => {
                            o.vehicle.net_stale_place(block.position, forward, along, &w.collision, &o.specs);
                            // 0x4A02A0's `car+0xE94 = 0`, `car+0xC = −1`
                            if let Controller::Ai(d) = &mut o.controller {
                                d.clear_branch();
                            }
                            tracing::info!(target: "netplay", seat = oi, tick = self.tick, slice = o.progress.slice, "stale remote car put on the road (0x4A0680)");
                        }
                        nfs_game::netcar::StalePlace::Brake(b) => {
                            c.throttle = 0;
                            c.brake = b;
                            tracing::debug!(target: "netplay", seat = oi, tick = self.tick, brake = b, "stale remote car: 0x4A0680's brake rule");
                        }
                    }
                }
                if let Controller::Ai(d) = &mut o.controller {
                    o.vehicle.shift_roll = d.shift_roll.take();
                }
                o.vehicle.misfire_roll = nfs_game::rng::rand01();
                o.vehicle.draft = nfs_physics::vehicle::draft_factor(
                    o.vehicle.position,
                    o.vehicle.forward(),
                    o.vehicle.v_long(),
                    &racer_positions[..]
                        .iter()
                        .enumerate()
                        .filter(|(k, _)| *k != oi + 1)
                        .map(|(_, p)| *p)
                        .collect::<Vec<_>>(),
                );
                // diagnostic: `NFS_OFFROAD_LOG=1` prints an opponent the tick it crosses from
                // drivable ground to none and back, which is how a car that gets past the
                // `.edg` line is caught (the wall chain is one-sided, so "behind it" is off
                // every patch)
                let off_before = o.off_surface;
                // `LoadTransfer`'s wet test, 0x4A191D…0x4A194D: `[0x606AE0]` and nothing overhead
                o.vehicle.wet_track = self.track.name.eq_ignore_ascii_case("autobahn") && w.overhead_flags(o.vehicle.position) == 0;
                o.vehicle.step32(&c, &o.specs, &w.collision);
                live[oi] = (o.vehicle.position, o.vehicle.v_long());
                o.off_surface = w.collision.find_patch(o.vehicle.position) & 0xF == 0;
                if o.off_surface != off_before && nfs_game::config::cfg().nfs_offroad_log.is_set() {
                    tracing::info!(
                        t = self.tick,
                        opponent = oi,
                        off = o.off_surface,
                        pos = ?o.vehicle.position,
                        kmh = o.vehicle.speed() * 3.6,
                        wall = ?w.walls.nearest(o.vehicle.position, 8.0).map(|(n, d)| (n.x, n.y, d)),
                        "offroad watch"
                    );
                }
                // its walls and props run in the collision pass 0x498FF0 below (priority 0x28),
                // after every car's step, not here (priority 0x1E). *(Until 2026-09-26 the port
                // collided them in the step.)*
                // the tracker's `preferredSection`: the branch this opponent chose (§1.4)
                let pref = match &o.controller {
                    Controller::Ai(d) => d.preferred_segment,
                    _ => None,
                };
                let o_lap_done = o.progress.update_tracked(
                    t,
                    o.vehicle.position,
                    o.vehicle.forward(),
                    self.tick,
                    self.num_laps,
                    pref,
                );
                if o_lap_done {
                    tracing::info!(opponent = oi, lap = o.progress.laps, tick = self.tick, finished = o.progress.finished, "opponent lap");
                }
                if o_lap_done && !o.eliminated && o.progress.laps <= self.num_laps {
                    ko_laps.push((oi, o.progress.laps));
                }
            }
        }
        st.ko_laps = ko_laps;
        st.remote_steps = remote_steps;
    }

    /// **The AI pass for the police** (see [`Self::simulate`]): the human's perp update and the cops' decisions in index order (`traffic.md` §17.3, §25, §27). Moved out of `simulate` unchanged.
    fn tick_ai_police(&mut self, ctx: &TickCtx) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let reg_of = |s: usize| ctx.reg_of(s);
        // AI pass: the human's perp update (slot 0; the port runs it here, next to the cops that
        // read it — nothing an opponent does reads the violation) and the cops in index order
        // (`traffic.md` §17.3 violation, §25 spawn, §27 chase). `Police::ai` is the decision
        // half only; each cop's drive 0x408360 is a Sc32 entry and runs below, after the
        // player's step and before the traffic's (§17.5e).
        if let (Some(pol), Some(t)) = (self.road.police.as_mut(), self.slices.as_ref()) {
            let slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
            let sl0 = &t.slices[slice];
            // `car+0xEC0` is `velocity · slice forward` (0x4178BE…0x4178E6), not the body-frame
            // speed, and `car+0x1020` is the lane `0x40EBB0` puts the car in — both are what
            // 0x403990 reads. The table is already reversed for a backwards race, so the sign
            // and the lane side flip together, as they do in the original (cb1ad65).
            let vl = v.velocity.dot(sl0.forward.normalize_or_zero());
            let lane = lane_of(sl0, (v.position - sl0.centre).dot(sl0.lateral.normalize_or_zero()));
            // `[0x5E7544]`: slice 1's lanesA in the track's own frame (0x40E670), which the
            // reversed table of a backwards race keeps in `lanes_b`
            let one_way = nfs_game::police::one_way_road(t);
            // the racers in registration order (`0x5E8E2C`: the human, then the opponents) —
            // the perps the spawn search and `0x405220` walk, and the cars a cop may take as
            // its reference car `car+0xED4` — and `0x417710`'s two race-order sorts
            // `car+0x528` of every cop and of the cars its lane pass is handed (`others` below:
            // the human, the opponents, the traffic in the world) — the previous even tick's
            // `0x5E8B88[]` places (`nfs_game::race_order`)
            {
                let n_opp = self.road.opponents.len();
                let n_cops = pol.cars.len();
                for (i, c) in pol.cars.iter_mut().enumerate() {
                    c.place = self.race_order.place(1 + n_opp + i);
                }
                pol.other_places = (0..=n_opp)
                    .map(|k| self.race_order.place(reg_of(k)))
                    .chain(
                        self.road
                            .traffic
                            .iter()
                            .flat_map(|tr| tr.cars.iter().enumerate().filter(|(_, c)| c.active).map(move |(j, _)| 1 + n_opp + n_cops + j))
                            .map(|k| self.race_order.place(k)),
                    )
                    .collect();
            }
            pol.set_racers(
                std::iter::once(nfs_game::police::RacerRef {
                    position: v.position,
                    slice,
                    progress: self.race.as_ref().map(|r| r.progress).unwrap_or(0.0),
                    laps_timed: self.race.as_ref().map(|r| r.lap_times.len() as u32).unwrap_or(0),
                    speed: vl,
                })
                .chain(self.road.opponents.iter().map(|o| nfs_game::police::RacerRef {
                    position: o.vehicle.position,
                    slice: o.progress.slice,
                    progress: o.progress.progress,
                    laps_timed: o.progress.lap_times.len() as u32,
                    speed: o.vehicle.velocity.dot(t.slices[o.progress.slice].forward.normalize_or_zero()),
                }))
                .collect(),
            );
            // the speed limit is a flat 22 m/s; wrong way is the side of the centreline not
            // matching the direction of travel, or, on a one-way road, progress against the race
            // — and once the player has finished, 0x4059C0 clears it instead (every tick)
            let finished = self.race.as_ref().map(|r| r.finished).unwrap_or(false);
            // 0x403B49…0x403B8C: the new violation is committed only once `gGameTick >= 0x200`
            // and a cop-class car is in the world (`c+0x52C & 0x20`, `c+0x7F` — a cop out of
            // purgatory); otherwise `+0x78` keeps what it had
            let tick = self.tick;
            let cop_in_world = pol.active_count() > 0;
            // `AIHigh_Human` keeps `+0x10 = 0` (base ctor 0x406790), so the AI pass runs its
            // update — `tickHeat` 0x405D70 and the check — only when 0x414B10 says it is due:
            // slot 0, forced to lod 0 by `car+0x10B8`, i.e. every fourth 32 Hz tick (§17.5e)
            if nfs_game::traffic::ai_pass_due(0.0, true, tick, 0).is_some() {
                pol.violation = nfs_game::police::perp_update(pol.violation, finished, |cur| {
                    let v = nfs_game::police::check_violation(cur, vl, 1.0, one_way, lane, false, 0.0);
                    if v != nfs_game::police::Violation::None && tick >= nfs_game::police::PURSUIT_START_TICK && cop_in_world { v } else { cur }
                });
            }
            let clean = pol.violation.is_clean();
            // §25.5 runs for every on-duty cop whatever the perp is doing: while the perp is
            // clean the probability doubles and the cop comes out on *patrol* (§17.4 states
            // 2/3); a wanted perp gets chasers. `set_wanted` is the order that turns the patrol
            // cars into chasers when the perp offends.
            pol.set_wanted(!clean);
            // state 1 of every idle cop: `0x405440` over every racer — the human and the
            // opponents (§17.5d) — each with its own cursor, claims and 500 m clearance; the
            // 172 m veto is the human's. State 1 (0x40403F…0x40405E) has no tick gate — only
            // the duty bit and the order — and the records are live from tick 1 (§25.5).
            // *(Until 2026-09-25 the port held the search back to tick 0x200, the violation
            // commit's gate, which belongs above.)* Every cop runs in index order, the idle
            // ones' searches between the others' updates (0x406720, `Police::tick`).
            {
                let sl = &t.slices[slice];
                let lat = (v.position - sl.centre).dot(sl.lateral.normalize_or_zero());
                let target = nfs_game::police::ChaseTarget {
                    position: v.position,
                    slice,
                    lateral: lat,
                    lane: lane_of(sl, lat),
                    speed: vl,
                    // `car+0x3A8` / `car+0x3B0`, the windows the §27.8 sector tests use
                    half_width: self.car_half_width,
                    half_length: v.half_extents.z,
                    finished: self.race.as_ref().map(|r| r.finished).unwrap_or(false),
                };
                let mut others = vec![LaneCar {
                    slice,
                    lane: target.lane,
                    lane_mask: lane_span(sl, lat, self.car_half_width),
                    speed: vl,
                    speed_est: v.speed(),
                    half_len: v.half_extents.z,
                    lateral: lat,
                    half_span: self.car_half_width,
                    is_human: true,
                    is_racer: true,
                }];
                // A cop's lane pass runs the same scorers as a traffic car's (`AIState_Normal`
                // 0x40B5A0 / `AIState_Chase`, §27), and they walk every car in the world with
                // `car+0x7F` set (0x401FD3 over `0x5E8B88[]`, 0x4026E4 over `0x5E8B34[]`) —
                // the opponents and the traffic too, not only the player. `Police::lane_pass`
                // adds the other cops itself.
                others.extend(self.road.opponents.iter().map(|o| {
                    let s = o.progress.slice;
                    let osl = &t.slices[s];
                    let olat = (o.vehicle.position - osl.centre).dot(osl.lateral.normalize_or_zero());
                    LaneCar {
                        slice: s,
                        lane: lane_of(osl, olat),
                        lane_mask: lane_span(osl, olat, o.vehicle.half_extents.x),
                        speed: o.vehicle.v_long(),
                        speed_est: o.vehicle.speed(),
                        half_len: o.vehicle.half_extents.z,
                        lateral: olat,
                        half_span: o.vehicle.half_extents.x,
                        is_human: false,
                        is_racer: true,
                    }
                }));
                if let Some(tr) = &self.road.traffic {
                    others.extend(tr.cars.iter().filter(|c| c.active).map(|c| c.as_lane_car(t)));
                }
                // world positions in `others`' own order — the player, the opponents, the
                // traffic — for `Police::lane_pass`'s `0x40E860` fine step (`traffic.md` §34.6);
                // `TrafficSet::ai`'s `other_positions` is the same idea for a traffic car's pass.
                pol.other_positions = std::iter::once(v.position)
                    .chain(self.road.opponents.iter().map(|o| o.vehicle.position))
                    .chain(self.road.traffic.iter().flat_map(|tr| tr.cars.iter().filter(|c| c.active).map(|c| c.position)))
                    .collect();
                // the AI pass's gate phase: the cops' slots follow the human's and the
                // opponents' (0x4ECF00, `edi = nP + nOpp`)
                pol.slot_base = 1 + self.road.opponents.len();
                pol.ai(t, self.tick, &target, &others, &[slice], &mut nfs_game::rng::rand16, Some(&w.collision));
                // Sc32 priority 6, the trackers 0x49AF70 → 0x417190: each cop's `car+0x78`,
                // its distance to the nearest racer, which the next AI pass's gate reads
                let racers: Vec<Vec3> = std::iter::once(v.position).chain(self.road.opponents.iter().map(|o| o.vehicle.position)).collect();
                pol.track_racers(&racers);
            }
        }
    }

    /// **The AI pass for the traffic** (see [`Self::simulate`]): `AIHigh_Traffic::update` over the lists built from the human cars, the opponents and the cops. Moved out of `simulate` unchanged; the player's pose it took goes to `st.traffic_reference`.
    fn tick_ai_traffic(&mut self, ctx: &TickCtx, st: &mut TickState) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let (exchange_in, exch_order, pos_order, pos_seat_horns) = (&ctx.exchange_in, &ctx.exch_order, &ctx.pos_order, &ctx.pos_seat_horns);
        let reg_of = |s: usize| ctx.reg_of(s);
        // traffic: `AIHigh_Traffic::update` 0x406020, last in 0x406720's index order. Its drive
        // 0x408360 is a Sc32 entry and runs below, after the player's step.
        let mut traffic_reference = None;
        if let (Some(tr), Some(t)) = (self.road.traffic.as_mut(), self.slices.as_ref()) {
            let slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
            // the player's own `car+0xEB0` (0x417800): −1 below −3 m/s, +1 from 0 up, otherwise
            // the sign of the heading against the slice
            let vl = v.v_long();
            let dir = if vl <= -3.0 {
                -1.0
            } else if vl >= 0.0 {
                1.0
            } else {
                v.forward().dot(t.slices[slice].forward).signum()
            };
            let laps = self.race.as_ref().map(|r| r.laps).unwrap_or(0);
            let mut reference = Reference {
                position: v.position,
                slice,
                laps,
                speed: vl,
                dir,
                // `human+0xD86`: bit 1 of the buttons byte `[0x5E9664]`, copied by 0x4991E0
                // (`physics.md` §8) — the same button the horn sound reads
                honking: self.input.held(Action::Horn),
            };
            // **Input exchange** (`network.md` §11.18): every list the traffic reads is built from
            // the human cars in registration order (race index — the same on every machine), each
            // human's numbers taken the same way whichever machine's car it is; the local car first
            // and the seats after it is a per-machine order. Empty outside an exchange race, where
            // the single-player lists below stand unchanged.
            // **A positional network race** builds the same lists: `0x5E8B34[]` holds every player's car,
            // in race-index order (0x40F189, `Car_register`), so the reference `car+0xED4`, the lane
            // scorers, `HumanView` and 0x4013D0's `Honk` see the remote humans too.
            let hum_order: &[HumanCar] = if exchange_in.is_some() { exch_order } else { pos_order };
            let horn_of = |h: HumanCar| match (exchange_in.as_ref(), h) {
                (Some(x), _) => x.horn(h),
                (None, HumanCar::Local) => reference.honking,
                (None, HumanCar::Seat(k)) => pos_seat_horns.get(k).copied().unwrap_or(false),
            };
            let hum: Vec<ExchHuman> = match !hum_order.is_empty() {
                true => hum_order
                    .iter()
                    .copied()
                    .map(|h| match h {
                        HumanCar::Local => {
                            let r = self.race.as_ref();
                            let s = r.map_or(slice, |r| r.slice);
                            let vl = v.v_long();
                            ExchHuman {
                                car: h,
                                position: v.position,
                                slice: s,
                                laps: r.map_or(0, |r| r.laps),
                                progress: r.map_or(0.0, |r| r.progress),
                                vl,
                                dir: exch_dir(t, vl, v.forward(), s),
                                speed: v.speed(),
                                hw: v.half_extents.x,
                                hl: v.half_extents.z,
                                horn: horn_of(h),
                            }
                        }
                        HumanCar::Seat(k) => {
                            let o = &self.road.opponents[k];
                            let s = o.progress.slice;
                            let vl = o.vehicle.v_long();
                            ExchHuman {
                                car: h,
                                position: o.vehicle.position,
                                slice: s,
                                laps: o.progress.laps,
                                progress: o.progress.progress,
                                vl,
                                dir: exch_dir(t, vl, o.vehicle.forward(), s),
                                speed: o.vehicle.speed(),
                                hw: o.vehicle.half_extents.x,
                                hl: o.vehicle.half_extents.z,
                                horn: horn_of(h),
                            }
                        }
                    })
                    .collect(),
                false => Vec::new(),
            };
            // the storage index of each human (`race_order`'s place table is indexed by registration
            // in an exchange race, by the port's storage index — 0 local, `1 + k` seat — otherwise)
            let hum_place = |r: usize| match (exchange_in.is_some(), hum_order.get(r)) {
                (false, Some(HumanCar::Seat(k))) => self.race_order.place(1 + k),
                (false, Some(HumanCar::Local)) => self.race_order.place(0),
                _ => self.race_order.place(r),
            };
            if let Some(h0) = hum.first() {
                // `0x5E8B34[0]`, the first human registered: what the spawn gate's tunnel window,
                // the early-race boost and the purgatory constructor read (`traffic.md` §34.5)
                reference = Reference { position: h0.position, slice: h0.slice, laps: h0.laps, speed: h0.vl, dir: h0.dir, honking: h0.horn };
            }
            // the cars the lane scorers must see besides the traffic set itself
            let lane_car = |pos_slice: usize,
                            lateral_of: f32,
                            hw: f32,
                            hl: f32,
                            speed: f32,
                            est: f32,
                            human: bool| {
                let sl = &t.slices[pos_slice];
                LaneCar {
                    slice: pos_slice,
                    lane: lane_of(sl, lateral_of),
                    lane_mask: lane_span(sl, lateral_of, hw),
                    speed,
                    speed_est: est,
                    half_len: hl,
                    lateral: lateral_of,
                    half_span: hw,
                    is_human: human,
                    // every car this closure builds is in the race — the player or an
                    // opponent — and 0x401C8D's shoulder bonus asks for `52C & 1`, which is
                    // exactly that (`traffic.md`)
                    is_racer: true,
                }
            };
            let lateral_on = |pos: Vec3, s: usize| {
                (pos - t.slices[s].centre).dot(t.slices[s].lateral.normalize_or_zero())
            };
            let mut others = vec![lane_car(
                slice,
                lateral_on(v.position, slice),
                self.car_half_width,
                v.half_extents.z,
                vl,
                v.speed(),
                true,
            )];
            others.extend(self.road.opponents.iter().map(|o| {
                let s = o.progress.slice;
                lane_car(
                    s,
                    lateral_on(o.vehicle.position, s),
                    o.vehicle.half_extents.x,
                    o.vehicle.half_extents.z,
                    o.vehicle.v_long(),
                    o.vehicle.speed(),
                    false,
                )
            }));
            if !hum.is_empty() {
                // every human in registration order, each a human (`car+0x52C & 4`) of the lane pass
                others = hum.iter().map(|h| lane_car(h.slice, lateral_on(h.position, h.slice), h.hw, h.hl, h.vl, h.speed, true)).collect();
            }
            // …and the cops. The lane scorers walk every car in the world: `0x401F20` (the
            // blockers) `0x5E8B88[0 … [0x5E8B58])` from 0x401FD3, `0x4026B0` `0x5E8B34[]` from
            // 0x4026E4 — the lists `Car_register` 0x40F189 fills with every car that is not a
            // smackable, cops included — skipping only itself and a car with `car+0x7F == 0`
            // (0x401FE2), which a cop in purgatory is (0x40DE60). The port left the cops out.
            if let Some(pol) = &self.road.police {
                others.extend(pol.cars.iter().filter(|c| c.active).map(|c| c.as_lane_car(t)));
            }
            // **Once**, not twice. `traffic.md` §21.5 used to say 0x408360 was inserted into
            // two scheduler slots for a traffic car and therefore ran twice per due tick; it
            // is not. `Car_installHandlers` seeds `car+0xE0C = 0x49AF70` at 0x410DDD and only
            // overwrites it with 0x408360 at 0x410E6D **when `car+0x52C & 1`** — the racer
            // bit, which traffic does not have (`test bl, al; je` at 0x410E64/0x410E6B skips
            // the store). `+0xE1C` gets 0x408360 unconditionally at 0x410E83, so traffic has
            // it in one slot. A racer has it in both and 0x408366 makes it return at once for
            // exactly that case, so it never does work twice for anybody.
            // the gate's phase is the slot, and the traffic's slots follow the cops' (0x4ECF00)
            tr.slot_base = 1 + self.road.opponents.len() + self.road.police.as_ref().map_or(0, |p| p.cars.len());
            // `0x405ED0`'s view of the cops (`0x5E8AC4[]`, registration order), after their own
            // AI this tick: the traffic's reaction to a cop within 75 m (`traffic.md` §11, §34.4)
            // `car+0x330` of each of `others`, in its order, for 0x40E860's fine step
            tr.other_positions = std::iter::once(v.position)
                .chain(self.road.opponents.iter().map(|o| o.vehicle.position))
                .chain(self.road.police.iter().flat_map(|p| p.cars.iter().filter(|c| c.active).map(|c| c.position)))
                .collect();
            tr.human_views = hum.iter().map(|h| nfs_game::traffic::HumanView { laps: h.laps, honking: h.horn }).collect();
            if !hum.is_empty() {
                tr.other_positions = hum.iter().map(|h| h.position).collect();
            }
            debug_assert_eq!(tr.other_positions.len(), others.len());
            // `car+0x528` of each of `others` and of every traffic car — the previous even tick's
            // `0x5E8B88[]` places, which 0x401FA2's walk starts from (`nfs_game::race_order`)
            {
                let n_opp = self.road.opponents.len();
                let n_cops = self.road.police.as_ref().map_or(0, |p| p.cars.len());
                // (in an exchange race `race_order` is indexed by registration index, `reg_of`,
                // and `others` is the humans in that order: place `r` is the `r`-th human's)
                tr.other_places = (0..=n_opp)
                    .map(|k| self.race_order.place(reg_of(k)))
                    .chain(
                        self.road
                            .police
                            .iter()
                            .flat_map(|p| p.cars.iter().enumerate().filter(|(_, c)| c.active).map(move |(i, _)| 1 + n_opp + i))
                            .map(|k| self.race_order.place(k)),
                    )
                    .collect();
                if !hum.is_empty() {
                    tr.other_places = (0..hum.len()).map(&hum_place).collect();
                }
                debug_assert_eq!(tr.other_places.len(), others.len());
                for (j, c) in tr.cars.iter_mut().enumerate() {
                    c.place = self.race_order.place(1 + n_opp + n_cops + j);
                }
            }
            tr.cops = self
                .road
                .police
                .as_ref()
                .map(|p| p.cars.iter().map(|c| nfs_game::traffic::CopView { slice: c.slice, speed: c.speed, in_world: c.active, has_target: c.target.is_some() }).collect())
                .unwrap_or_default();
            // `0x5E8B10[]`, the chase targets a traffic car takes its reference `car+0xED4` from:
            // the human, and the opponents only under `COPS [0x657414]` (`Car_register`
            // 0x40F336) — the port's `COPS` is a race with police (`traffic.md` §34.5)
            let dir_of = |vl: f32, fwd: Vec3, s: usize| {
                // 0x417800's `car+0xEB0`: −1 below −3 m/s, +1 from 0 up, else the heading's sign
                if vl <= -3.0 {
                    -1.0
                } else if vl >= 0.0 {
                    1.0
                } else {
                    fwd.dot(t.slices[s].forward).signum()
                }
            };
            tr.chase_targets = std::iter::once(nfs_game::traffic::ChaseTarget {
                position: v.position,
                slice,
                speed: vl,
                dir,
                place: self.race_order.place(reg_of(0)),
            })
            .chain(self.road.opponents.iter().enumerate().filter(|_| self.road.police.is_some() && hum.is_empty()).map(|(k, o)| {
                let s = o.progress.slice;
                let ovl = o.vehicle.velocity.dot(t.slices[s].forward.normalize_or_zero());
                nfs_game::traffic::ChaseTarget {
                    position: o.vehicle.position,
                    slice: s,
                    speed: ovl,
                    dir: dir_of(ovl, o.vehicle.forward(), s),
                    place: self.race_order.place(1 + k),
                }
            }))
            .collect();
            if !hum.is_empty() {
                // every human is appended to `0x5E8B10[]` unconditionally (0x40F299), in
                // registration order
                tr.chase_targets = hum
                    .iter()
                    .enumerate()
                    .map(|(r, h)| nfs_game::traffic::ChaseTarget { position: h.position, slice: h.slice, speed: h.vl, dir: h.dir, place: hum_place(r) })
                    .collect();
            }
            tr.ai_on(t, self.tick, reference, &others, self.world.as_ref().map(|w| &w.collision));
            traffic_reference = Some(reference);
            // Sc32 priority 6, the trackers 0x49AF70 → 0x417190: each traffic car's `car+0x78`,
            // its distance to the nearest racer (the human, then the opponents, `0x5E8E2C[]`),
            // which the next AI pass's gate reads
            let racers: Vec<Vec3> = if hum.is_empty() {
                std::iter::once(v.position).chain(self.road.opponents.iter().map(|o| o.vehicle.position)).collect()
            } else {
                hum.iter().map(|h| h.position).collect()
            };
            tr.track_racers(&racers);
        }
        st.traffic_reference = traffic_reference;
    }

    /// **The player's step and the human cars around it** (see [`Self::simulate`]): the seats of an exchange race (or of a positional one) with a lower race index, the player's `Car_simulate` (the force-feedback driver before it, the `--record` row after it), then the seats after it. Moved out of `simulate` unchanged.
    fn tick_player_step(&mut self, ctx: &TickCtx, st: &TickState) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let (rally_on, derby_on, ff_replay) = (ctx.rally_on, ctx.derby_on, ctx.ff_replay);
        let exchange_in = &ctx.exchange_in;
        let controls = st.controls;
        // Sc32, priority 0x1E, slot 0: the player's `Car_simulate` 0x49FD30 and `Car_ground`
        // 0x49BD40, after the whole AI pass; its walls and props run in the collision pass
        // below (0x495020 / 0x498DE0, priority 0x28), as of 2026-09-26.
        // **Input exchange: the human cars step in race-index order** (Sc32 priority 0x1E, the slot
        // order of `Cars_create`), whichever of them is this machine's — so a car sees the ones
        // before it where they have just moved to and the ones after it where the tick began
        // (`DraftFactor` reads them live, the misfire draw is the shared stream's next), and
        // every machine gets the same numbers. The seats with a lower index first…
        let autobahn = self.track.name.eq_ignore_ascii_case("autobahn");
        if let Some(x) = exchange_in.as_ref() {
            let mut lower: Vec<(usize, u32, Controls)> = x.seats.iter().filter(|e| e.1 < x.local_index).copied().collect();
            lower.sort_by_key(|e| e.1);
            Self::exchange_seats_step(&mut self.road.opponents, &lower, v.position, self.tick, (rally_on, derby_on), &|p| autobahn && w.overhead_flags(p) == 0, &w.collision);
        }
        // **A positional race: the same slot order** — the remote cars' `Car_simulate` is the
        // human's 0x49FD30 on the Sc32 list, inserted car by car in `Cars_create`'s order
        // (0x411277…0x411349, race index), so the seats with a lower index than this
        // machine's own car step before it and the rest after (until then
        // all of them stepped after the player's). Without a local index (no session) they
        // keep the old place, all after.
        let local_idx = self.net.session.as_ref().and_then(|n| n.local_race_index());
        let (mut early, mut late): (Vec<(usize, u32, Controls)>, Vec<(usize, u32, Controls)>) =
            st.remote_steps.iter().copied().partition(|e| local_idx.is_some_and(|l| e.1 < l));
        early.sort_by_key(|e| e.1);
        late.sort_by_key(|e| e.1);
        if !early.is_empty() {
            Self::exchange_seats_step(&mut self.road.opponents, &early, v.position, self.tick, (rally_on, derby_on), &|p| autobahn && w.overhead_flags(p) == 0, &w.collision);
        }
        // `DraftFactor` 0x49D9B0: the racer array only — traffic and police do not draft
        let racers: Vec<Vec3> = self.road.opponents.iter().map(|o| o.vehicle.position).collect();
        v.draft =
            nfs_physics::vehicle::draft_factor(v.position, v.forward(), v.v_long(), &racers);
        // the misfire draw comes from the shared LCG, like every other draw in the game
        v.misfire_roll = nfs_game::rng::rand01();
        // `LoadTransfer`'s wet test, 0x4A191D…0x4A194D: `[0x606AE0]` and nothing overhead
        v.wet_track = self.track.name.eq_ignore_ascii_case("autobahn") && w.overhead_flags(v.position) == 0;
        // 0x4123E0, the force-feedback driver: Sc32 priority 10, before the car's step (priority
        // 0x1E) — it reads what the last tick left
        super::force_feedback::ff_drive(&mut self.ff, &mut self.gamepads, self.ff_levels, ff_replay, self.tick, &nfs_physics::force_feedback::FfCar::from_vehicle(v, &self.specs));
        v.step32(&controls, &self.specs, &w.collision);
        // the remote cars' `Car_simulate` (a positional race) with an index above this machine's
        // own, after the player's, in race-index order (above)
        if !late.is_empty() {
            Self::exchange_seats_step(&mut self.road.opponents, &late, v.position, self.tick, (rally_on, derby_on), &|p| autobahn && w.overhead_flags(p) == 0, &w.collision);
        }
        // …and the seats after it
        if let Some(x) = exchange_in.as_ref() {
            let mut higher: Vec<(usize, u32, Controls)> = x.seats.iter().filter(|e| e.1 > x.local_index).copied().collect();
            higher.sort_by_key(|e| e.1);
            Self::exchange_seats_step(&mut self.road.opponents, &higher, v.position, self.tick, (rally_on, derby_on), &|p| autobahn && w.overhead_flags(p) == 0, &w.collision);
        }
        if let Some(rec) = self.record.as_mut() {
            use std::io::Write;
            let _ = writeln!(
                rec,
                "{},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.1},{},{}",
                self.tick,
                controls.throttle,
                controls.brake,
                controls.steer,
                controls.handbrake as u8,
                v.position.x,
                v.position.y,
                v.position.z,
                v.velocity.x,
                v.velocity.y,
                v.velocity.z,
                v.rpm,
                v.gear,
                self.race.as_ref().map(|r| r.slice).unwrap_or(0)
            );
        }
    }

    /// **The sounds the player's step raised** (see [`Self::simulate`]): the hard landing and the roll-over / crash one-shots, queued for the drain at the next odd tick. Moved out of `simulate` unchanged; the body-ground event word that a later contact of the tick may replace goes out through `ground_event_out`.
    fn tick_step_sounds(&mut self, ground_event_out: &mut Option<(f32, usize)>) {
        let Some(v) = self.vehicle.as_mut() else { return };
        // the player's one-shots are panned by the angle to its last contact point `car+0x440`
        // (0x4A7BB1…0x4A7C3B), measured from the listener the loops' pan uses — but, as the
        // original's drain does (0x4A7AF8…0x4A7C97), only ONCE, after this whole tick's physics,
        // from whatever `car+0x440` this tick's contacts leave standing: every one-shot below is
        // *queued* (`self.snd.player_loops`, drained with the loops next tick, `CollisionSfx::drain_player`)
        // rather than played straight against its own moment's contact, as this port did until
        // 2026-09-27 (`sfx.md`'s "the drain's distance and pan"). The queue's cap is 7
        // (`SfxQueue::CAP`), the original's `car+0x1110`'s.
        // the hard-landing event `Car_ground` pushed during step32 (the closing speed): its
        // own queue entry (0x49C1E6, priority 0x1E), played whether or not the collision pass
        // below queues a wall hit too *(until 2026-09-26 a wall hit in the same tick silenced it)*
        let landing = v.landing_sound.take().unwrap_or(0.0);
        // `Car_ground`'s body–ground event word (`0x30000 | car+0x464`, 0x496104) — 0x40F740
        // plays it at the tick's end unless a later contact wrote its own word over it: a wall
        // hit (0x494000), a prop (0x498A44 / 0x498DB6 / 0x498F00 / 0x498F39) or a car
        // (0x496CB5 / 0x496CBD), the byte scan's other `car+0x438` writers in the pass below
        let ground_event = v.ground_sound.take();
        if landing > 6.0 {
            // Tires × Asphalt whatever the ground (0x49C1DD `push 5`) — the port had the
            // surface under the front-left wheel (corrected 2026-09-26)
            self.snd.player_loops.push(nfs_audio::sfx::QueuedSfx::Landing { closing: landing });
        }
        // the walls and the props run in the collision pass 0x498FF0 below
        // crash / roll-over one-shot (Damage 0x494500 zone rule; roll-over 0x49BC80 at level 127)
        // the roll-over event `Car_ground` raises (0x49C095…0x49C0E6: damage on, cos tilt
        // under −0.86, roof below 40, not finished)
        let rolled = std::mem::take(&mut v.rollover_sound);
        if v.crash_sound || rolled {
            if !self.snd.crash_played {
                self.snd.player_loops.push(nfs_audio::sfx::QueuedSfx::Crash);
            }
            self.snd.crash_played = rolled;
        } else {
            self.snd.crash_played = false;
        }
        v.crash_sound = false;
        *ground_event_out = ground_event;
    }

    /// **The trigger sweeps on the player's car** (see [`Self::simulate`]): the trackside cameras' sweep and the Factory Driver skill test's checkpoints, finish and cones, then the position the next tick's sweeps compare with. Moved out of `simulate` unchanged.
    fn tick_trackside_and_skill(&mut self, ctx: &TickCtx) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let (view, replay_camera) = (ctx.view, ctx.replay_camera);
        // trackside cameras: the trigger sweep 0x413B80 for the watched car. The dispatcher
        // calls it only while a replay plays (`[0x5E99F4] >= 2`, 0x463E14…0x463E28) — or in a
        // network session with `+0xBD` set (0x463E36…0x463E60, not carried) — never in a live
        // single-player race; its flag, `[0x606850] == 0x14`, lets the crossing switch the
        // view (0x413BFA). The `NFS_VIEW` diagnostic runs it live, so a TV view held on a
        // live race still follows the triggers. 0x0C counts as the TV choice, as it does for
        // the camera update's `tv_mode`.
        if replay_camera || nfs_game::config::cfg().nfs_view.is_set() {
            let probe = nfs_game::skill::TriggerCar {
                position: v.position,
                wheels: std::array::from_fn(|i| v.wheels[i].world_pos - Vec3::Y * v.ride_height),
                forward: v.forward(),
                velocity: v.velocity,
                speed: v.horizontal_speed(),
            };
            if let Some(tc) = self.track_cameras.as_mut()
                && let Some(i) = tc.update(&probe, matches!(view, 0x0C | 0x14))
            {
                tracing::debug!(camera = i, "trackside camera");
            }
        }
        // Factory Driver skill test: checkpoints / finish / cones (even ticks, 0x416170)
        if let Some(sk) = self.skill.as_mut() {
            // what 0x4704E0 / 0x4706C0 read: the wheels' world points `wheel+0x40` (the ground
            // contacts — the port's `world_pos` sits `ride_height` above them), `car+0x37C`,
            // `car+0x33C` and `car+0x35C`
            let probe = nfs_game::skill::TriggerCar {
                position: v.position,
                wheels: std::array::from_fn(|i| v.wheels[i].world_pos - Vec3::Y * v.ride_height),
                forward: v.forward(),
                velocity: v.velocity,
                speed: v.horizontal_speed(),
            };
            // 0x416170: `car+0xDB0 == 0 && car+0x1100 < 4`
            let about_to_reset = v.engine_alive == 0 && v.reset_countdown < 4;
            let events = sk.tick(self.tick, &probe, about_to_reset);
            for e in events {
                match e {
                    SkillEvent::Checkpoint { sequence, chime } => {
                        // 0x470460: a trigger whose field 15 is 1 chimes (0x4AAE30)
                        if chime
                            && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
                        {
                            s.trigger_chime(a);
                        }
                        let shown: Vec<usize> = sk.shown_arrow.into_iter().collect();
                        self.world_actions
                            .push(WorldAction::Show(sk.arrows.clone(), shown));
                        tracing::info!(sequence, "checkpoint");
                    }
                    SkillEvent::Finish { time_ticks } => {
                        // 0x4161FB…0x416226: state 2, `car+0xB14 = tick`, `car+0xB24 =
                        // tick − 448` — and no lap slot `+0xB28[]` (the port had pushed the
                        // time as a lap, which `Race_finalize` then took as the best lap)
                        if let Some(r) = self.race.as_mut() {
                            r.finished = true;
                            r.tick = self.tick;
                        }
                        tracing::info!(
                            time_s = time_ticks as f32 / TICKS_PER_SEC_F32,
                            cones = self.smackables.knocked_count(),
                            "skill test finished"
                        );
                    }
                    SkillEvent::Dnf => {
                        if let Some(r) = self.race.as_mut() {
                            r.finished = true;
                        }
                        tracing::info!("skill test DNF");
                    }
                }
            }
        }
        // both trigger sweeps above compare the position with the previous tick's
        self.prev_car_pos = v.position;
    }

    /// **The cops' and the traffic's drive** (see [`Self::simulate`]): `0x408360` for each, then its `Car_ground`, after the player's step and before the collision pass. Moved out of `simulate` unchanged; the traffic reads the reference its AI pass took (`st.traffic_reference`).
    fn tick_cop_and_traffic_drive(&mut self, st: &TickState) {
        let Some(w) = self.world.as_ref() else { return };
        // the traffic's drive `0x408360` (`car+0xE1C`, Sc32 priority 0x1E): after the player's
        // step (slot 0, the same priority, inserted first) and before the collision pass at
        // 0x28, from the targets this tick's AI pass wrote. Its reference is the one the AI
        // pass took, the player's pose as the tick began: positions move only on the Sc64 list.
        // The cops' drives come first: `Cars_create` inserts the Sc32 entries car by car in
        // `0x5E8B34` order (0x411277…0x411349) and the cops' slots precede the traffic's.
        if let (Some(pol), Some(t)) = (self.road.police.as_mut(), self.slices.as_ref()) {
            // 0x408750's net → `0x40F540(car, 1, 0)` → 0x40F480 walks every live car of
            // `0x5E8B34[]` for a taken slice: the human, the opponents and the traffic (the
            // other cops `Police` adds itself), then the smackables
            let occupied: Vec<usize> = self
                .race
                .as_ref()
                .map(|r| r.slice)
                .into_iter()
                .chain(self.road.opponents.iter().filter(|o| !o.eliminated).map(|o| o.progress.slice))
                .chain(reset_blockers(None, self.road.traffic.as_ref(), &self.smackables))
                .collect();
            pol.drive(t, self.tick, Some(nfs_game::police::CopWorld { grid: &w.collision, occupied: &occupied }));
            // …and each cop's `Car_ground` 0x49BD40 (`car+0xE20`, the entry after its drive)
            pol.ground(t, &w.collision, self.tick);
        }
        // the slices 0x408750's reset skips (0x40F480 walks every live car of `0x5E8B34` — the
        // racers and the cops; the traffic adds its own — and every smackable of `0x5E8C24`)
        let racer_slices_now: Vec<usize> = self
            .race
            .as_ref()
            .map(|r| r.slice)
            .into_iter()
            .chain(self.road.opponents.iter().filter(|o| !o.eliminated).map(|o| o.progress.slice))
            .chain(reset_blockers(self.road.police.as_ref(), None, &self.smackables))
            .collect();
        if let (Some(tr), Some(t), Some(reference)) = (self.road.traffic.as_mut(), self.slices.as_ref(), st.traffic_reference) {
            let grid = self.world.as_ref().map(|w| &w.collision);
            tr.drive(t, self.tick, reference, grid, &racer_slices_now);
            // …and each car's `Car_ground` 0x49BD40 (`car+0xE20`, the entry after its drive)
            if let Some(g) = grid {
                tr.ground(g, self.tick);
            }
        }
    }

    /// **The collision pass's switches and masses** (see [`Self::simulate`]): whether cars meet at all, and the collision mass of the player and of each opponent under `HEAVY` and `.TINYCAR`. Moved out of `simulate` unchanged.
    fn collision_inputs(&self, ctx: &TickCtx) -> CollisionInputs {
        let (heavy_on, net_seats) = (ctx.heavy_on, &ctx.net_seats);
        // **`if ([0x657408] == 0 || [0x65740C] == 0)` (0x498FF0)** guards the pair loop and the
        // car's entry into the list `0x628CDC[]`: with `COMMSPOSITIONAL && NOCOLLISION` no car
        // meets another (each car's walls 0x495020 and props 0x498DE0 run before the gate);
        // `NFS_NO_CARCOL` is the port's own diagnostic switch for the same skip
        let no_carcol = self.net_nocollision || nfs_game::config::cfg().nfs_no_carcol.is_set();
        // `car+0x354`, the collision mass: `HEAVY` `[0x6573FC]` (the fetherw8 code, cheat bit 8)
        // multiplies a human car's by 12 (0x49B5FB…0x49B613, `[0x5B240C]`)
        // …and otherwise `.TINYCAR` divides any car's by 12 (0x49B615…0x49B62F, `[0x5B49B0]`) —
        // every car but traffic carries it under the Gulliver code
        let tiny_k = if self.cheat_bits & CHEAT_TINY != 0 { 1.0 / 12.0 } else { 1.0 };
        let player_mass = self.specs.mass * if heavy_on { 12.0 } else { tiny_k };
        // every opponent's collision-mass factor (0x49B5FB…0x49B62F): a human car — a network seat —
        // takes `HEAVY`'s ×12 when the launch has it, else the car's own `.TINYCAR` /12
        // (`RaceCar::tiny`: this machine's bit for an AI opponent, the record's for a seat)
        let opp_k: Vec<f32> = self
            .road
            .opponents
            .iter()
            .enumerate()
            .map(|(oi, o)| if heavy_on && net_seats.contains(&oi) { 12.0 } else if o.tiny { 1.0 / 12.0 } else { 1.0 })
            .collect();
        CollisionInputs { no_carcol, tiny_k, player_mass, opp_k }
    }

    /// **The collision pass, the race's smackables** (see [`Self::simulate`]): the signs, hydrants, cones … after every car's own pass, each meeting the cars as B. Moved out of `simulate` unchanged; `ai_hits` and the player's body-ground event word are the pass's shared state.
    fn tick_collide_smackables(&mut self, ctx: &TickCtx, ci: &CollisionInputs, ai_hits: &mut Vec<(AudioCar, nfs_audio::sfx::QueuedSfx)>, ground_event_out: &mut Option<(f32, usize)>) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let exch_order = &ctx.exch_order;
        let reg_of = |s: usize| ctx.reg_of(s);
        let (tiny_k, player_mass, opp_k) = (ci.tiny_k, ci.player_mass, &ci.opp_k);
        // `Car_wallCollide` 0x495020's corner skip, as in the rest of the pass
        let patch_height = |p: Vec3| w.collision.find_patch_height(p).and_then(|(word, h)| (word & 0xF != 0).then_some(h));
        let tick = self.tick as u32;
        let mut ground_event = *ground_event_out;
        // the race's smackables — a mission's cones and the track's own signs, hydrants, pipes,
        // sawhorses … (0x4A17F0 in every race) — after every car's collision pass: each is a
        // car record of its own, and its pairs meet the cars of `0x5E8B34` as B (the human,
        // the opponents, the cops, the traffic; `nfs_game::smackables`,
        // `nfs_physics::smackable`); a car's own impact from one plays as its contact event
        // (`car+0x438 = 0x50000 | smackable+0x44C` with `+0x434` = the prop, a car record, so
        // 0x40F740 queues CarBody × the prop's material, `idB = word & 0xFF` at 0x40FA38 — a
        // cone's 0x10 is **Pylon**, group 0x56; corrected 2026-09-26 from "CarBody × CarBody",
        // `Vehicle::pair_event`). The prop's own 0x410280 event is never played: only
        // 0x4A7080 (slot cars, from `0x5E8B34`) and 0x4A7660 (the human) drain `car+0x110C`,
        // and 0x498220 zeroes every smackable's count unheard (`skill-tests.md` §5.4)
        {
            // `[0x658080]`: the cars of `0x5E8B34[]` — the player, the opponents, the cops, the
            // traffic — ahead of the props' slots
            let n_cars = 1
                + self.road.opponents.len()
                + self.road.police.as_ref().map_or(0, |p| p.cars.len())
                + self.road.traffic.as_ref().map_or(0, |tr| tr.cars.len());
            let racers = 1 + self.road.opponents.len();
            let mut keys: Vec<Option<AudioCar>> = vec![None];
            keys.extend((0..self.road.opponents.len()).map(|i| Some(AudioCar::Opponent(i))));
            let mut cars: Vec<(&mut nfs_physics::vehicle::Vehicle, f32)> = std::iter::once((&mut *v, player_mass / 128.0))
                .chain(self.road.opponents.iter_mut().enumerate().map(|(oi, o)| (&mut o.vehicle, o.specs.mass * opp_k[oi] / 128.0)))
                .collect();
            // an exchange race: the human cars in registration order, so that a prop two of them hit on
            // one tick goes to the same one on every machine (`0x5E8B34[]`, 0x4A17F0's walk)
            if !exch_order.is_empty() {
                let mut by_storage: Vec<Option<(&mut nfs_physics::vehicle::Vehicle, f32)>> = cars.drain(..).map(Some).collect();
                let old_keys = std::mem::take(&mut keys);
                let mut order: Vec<usize> = (0..by_storage.len()).collect();
                order.sort_by_key(|&s| reg_of(s));
                for s in order {
                    cars.push(by_storage[s].take().expect("each car once"));
                    keys.push(old_keys[s]);
                }
            }
            if let Some(pol) = self.road.police.as_mut() {
                for (i, c) in pol.cars.iter_mut().enumerate().filter(|(_, c)| c.active) {
                    keys.push(Some(AudioCar::Police(i)));
                    cars.push((&mut c.body, c.mass * tiny_k / 128.0));
                }
            }
            if let Some(tr) = self.road.traffic.as_mut() {
                let masses: Vec<(usize, f32)> = tr.cars.iter().enumerate().filter(|(i, c)| c.active && *i < tr.bodies.len()).map(|(i, c)| (i, c.mass)).collect();
                let mut bodies: Vec<Option<&mut nfs_physics::vehicle::Vehicle>> = tr.bodies.iter_mut().map(Some).collect();
                for (i, m) in masses {
                    if let Some(b) = bodies[i].take() {
                        keys.push(Some(AudioCar::Traffic(i)));
                        cars.push((b, m / 128.0));
                    }
                }
            }
            let mut walls = |c: &mut nfs_physics::vehicle::Vehicle| {
                c.collide_walls(&w.walls, &w.tree, &patch_height);
                c.collide_props(&w.props, tick);
            };
            let hits = self.smackables.step(w, self.slices.as_ref(), self.tick, n_cars, racers, &mut cars, &mut walls);
            let mut touched_cops = Vec::new();
            let mut touched_traffic = Vec::new();
            for (i, effect) in hits {
                let Some((car, _)) = cars.get(i) else { continue };
                let material = car.pair_event & 0xFF;
                match keys.get(i).copied().flatten() {
                    Some(key) => {
                        match key {
                            AudioCar::Police(k) => touched_cops.push(k),
                            AudioCar::Traffic(k) => touched_traffic.push(k),
                            AudioCar::Opponent(_) => {}
                        }
                        ai_hits.push((key, nfs_audio::sfx::QueuedSfx::Smackable { effect, material }));
                    }
                    None => {
                        ground_event = None;
                        tracing::debug!(effect, material, "smackable contact: the player's CarBody x smackable one-shot");
                        self.snd.player_loops.push_impact(nfs_audio::sfx::QueuedSfx::Smackable { effect, material });
                    }
                }
            }
            drop(cars);
            // the bodies the props moved hold the kinematic state the cops' and the
            // traffic's own records read back
            if let Some(pol) = self.road.police.as_mut() {
                for k in touched_cops {
                    pol.cars[k].sync_body_out();
                }
            }
            if let Some(tr) = self.road.traffic.as_mut() {
                for k in touched_traffic {
                    tr.sync_out(k);
                }
            }
        }
        *ground_event_out = ground_event;
    }

    /// **The collision pass, the traffic** (see [`Self::simulate`]): each traffic car as its own rigid body against the walls, the props, the human cars, the cops and the traffic cars before it. Moved out of `simulate` unchanged.
    fn tick_collide_traffic(&mut self, ctx: &TickCtx, ci: &CollisionInputs, ai_hits: &mut Vec<(AudioCar, nfs_audio::sfx::QueuedSfx)>, ground_event_out: &mut Option<(f32, usize)>) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let exch_order = &ctx.exch_order;
        let (no_carcol, tiny_k, player_mass, opp_k) = (ci.no_carcol, ci.tiny_k, ci.player_mass, &ci.opp_k);
        // `Car_wallCollide` 0x495020's corner skip, as in the rest of the pass
        let patch_height = |p: Vec3| w.collision.find_patch_height(p).and_then(|(word, h)| (word & 0xF != 0).then_some(h));
        let tick = self.tick as u32;
        // 0x4980D0's rounds collide each car's walls again between them (0x495020)
        let mut racer_walls = |c: &mut nfs_physics::vehicle::Vehicle| {
            c.collide_walls(&w.walls, &w.tree, &patch_height);
            c.collide_props(&w.props, tick);
        };
        let mut ground_event = *ground_event_out;
        // traffic vs everyone (`traffic.md` §21.5): the broad phase walks every car pair, so a
        // traffic car is its own rigid body (`TrafficSet::bodies`). The impulse it takes lives
        // until the next AI tick overwrites its velocity, exactly as in the original.
        // `car+0x1100` / `car+0xDB0` go through the body: the solver's `Damage` (0x496F8E →
        // 0x4948E6 / 0x4948C1, `Vehicle::crash_response`) arms the crash countdown on the
        // traffic car's own impact past 80 and kills it past 120, and the walls' call
        // (0x495563) does the same (`TrafficSet::sync_in` / `sync_out`). *(Until 2026-09-26
        // the body was a stand-in synced with its angular velocity zeroed.)*
        if !no_carcol && let Some(tr) = self.road.traffic.as_mut() {
            for i in 0..tr.cars.len() {
                if !tr.cars[i].active || i >= tr.bodies.len() {
                    continue;
                }
                tr.sync_in(i);
                let (cars, bodies) = (&mut tr.cars, &mut tr.bodies);
                let (done_cars, rest_cars) = cars.split_at_mut(i);
                let (done_bodies, rest_bodies) = bodies.split_at_mut(i);
                let (c, b) = (&mut rest_cars[0], &mut rest_bodies[0]);
                // its own walls and props first (0x495020 / 0x498DE0 behind `car+0x7C`)
                b.impact = 0.0;
                if b.collide_walls(&w.walls, &w.tree, &patch_height) {
                    ai_hits.push((AudioCar::Traffic(i), wall_sfx(b)));
                }
                if let Some((impact, event)) = b.collide_props(&w.props, tick) {
                    ai_hits.push((AudioCar::Traffic(i), prop_sfx(impact, event)));
                }
                // the human cars meet it in registration order (an exchange race: by race index, the
                // same on every machine; otherwise the local car, then the opponents)
                let human_pass: Vec<HumanCar> = if exch_order.is_empty() {
                    std::iter::once(HumanCar::Local).chain((0..self.road.opponents.len()).map(HumanCar::Seat)).collect()
                } else {
                    exch_order.clone()
                };
                for who in human_pass {
                    match who {
                        HumanCar::Local => {
                            if let Some((ea, eb)) =
                                nfs_physics::vehicle::collide_pair(v, player_mass, b, c.mass, &mut racer_walls)
                            {
                                ai_hits.push((AudioCar::Traffic(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                                ground_event = None;
                                // the collision script trigger (§12.9 trigger 0)
                                c.hit_by_human_tick = self.tick;
                                // `car+0x418`, the counter that disables the "on its roof" exit (§21.7)
                                c.collisions += 1;
                                tracing::debug!(
                                    car = i,
                                    ea,
                                    eb,
                                    mass = c.mass,
                                    speed = v.speed(),
                                    "player hit traffic"
                                );
                                if !exch_order.is_empty() {
                                    tracing::info!(target: "netplay", tick = self.tick, traffic = i, ea, eb, "an input-exchange race: the local car hit the traffic car");
                                }
                                // the player's own `car+0x430` (A)
                                self.snd.player_loops.push_impact(nfs_audio::sfx::QueuedSfx::CarCar { effect: ea });
                            }
                        }
                        HumanCar::Seat(oi) => {
                            let o = &mut self.road.opponents[oi];
                            if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(
                                &mut o.vehicle,
                                o.specs.mass * opp_k[oi],
                                b,
                                c.mass,
                                &mut racer_walls,
                            ) {
                                let effect = ea.max(eb);
                                ai_hits.push((AudioCar::Opponent(oi), nfs_audio::sfx::QueuedSfx::CarCar { effect: ea }));
                                ai_hits.push((AudioCar::Traffic(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                                c.collisions += 1;
                                // a seat is a human in an exchange race (`car+0x52C & 4`): the script
                                // trigger 0 sees its hit as it sees the local car's
                                if !exch_order.is_empty() {
                                    c.hit_by_human_tick = self.tick;
                                }
                                tracing::debug!(
                                    car = i,
                                    opponent = o.name,
                                    effect,
                                    "opponent hit traffic"
                                );
                                if !exch_order.is_empty() {
                                    tracing::info!(target: "netplay", tick = self.tick, traffic = i, seat = oi, ea, eb, "an input-exchange race: a seat's car hit the traffic car");
                                }
                            }
                        }
                    }
                }
                // …and the cops, whose passes ran before this traffic car's (their slots
                // precede the traffic's in `0x5E8B34`)
                if let Some(pol) = self.road.police.as_mut() {
                    for (pi, p) in pol.cars.iter_mut().enumerate().filter(|(_, p)| p.active) {
                        let p_mass = p.mass * tiny_k;
                        if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(&mut p.body, p_mass, b, c.mass, &mut racer_walls) {
                            c.collisions += 1;
                            p.sync_body_out();
                            ai_hits.push((AudioCar::Police(pi), nfs_audio::sfx::QueuedSfx::CarCar { effect: ea }));
                            ai_hits.push((AudioCar::Traffic(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                        }
                    }
                }
                // …and the traffic cars before it, whose bodies hold their own pass's state
                let mut touched = Vec::new();
                for (j, (dc, db)) in done_cars.iter_mut().zip(done_bodies.iter_mut()).enumerate().filter(|(_, (dc, _))| dc.active) {
                    if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(db, dc.mass, b, c.mass, &mut racer_walls) {
                        dc.collisions += 1;
                        c.collisions += 1;
                        touched.push(j);
                        ai_hits.push((AudioCar::Traffic(j), nfs_audio::sfx::QueuedSfx::CarCar { effect: ea }));
                        ai_hits.push((AudioCar::Traffic(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                    }
                }
                tr.sync_out(i);
                for j in touched {
                    tr.sync_out(j);
                }
            }
        }
        *ground_event_out = ground_event;
    }

    /// **The collision pass, the cops** (see [`Self::simulate`]): each active cop against the walls, the props, the player, the opponents and the cops before it. Moved out of `simulate` unchanged.
    fn tick_collide_police(&mut self, ctx: &TickCtx, ci: &CollisionInputs, ai_hits: &mut Vec<(AudioCar, nfs_audio::sfx::QueuedSfx)>, ground_event_out: &mut Option<(f32, usize)>) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let (rally_on, derby_on) = (ctx.rally_on, ctx.derby_on);
        let (no_carcol, tiny_k, player_mass, opp_k) = (ci.no_carcol, ci.tiny_k, ci.player_mass, &ci.opp_k);
        // `Car_wallCollide` 0x495020's corner skip, as in the rest of the pass
        let patch_height = |p: Vec3| w.collision.find_patch_height(p).and_then(|(word, h)| (word & 0xF != 0).then_some(h));
        let tick = self.tick as u32;
        // 0x4980D0's rounds collide each car's walls again between them (0x495020)
        let mut racer_walls = |c: &mut nfs_physics::vehicle::Vehicle| {
            c.collide_walls(&w.walls, &w.tree, &patch_height);
            c.collide_props(&w.props, tick);
        };
        let mut ground_event = *ground_event_out;
        // The cops' passes, in `0x5E8B34` order — after the humans and the opponents, before
        // the traffic (0x4ED103 / 0x4ED1ED). 0x498FF0 heads each car's pass with its own walls
        // (0x495020) and props (0x498DE0) behind `car+0x7C`, which `RigidBody_init` sets to 1
        // for every body (0x49B525) — a cop included (`traffic.md` §22) — and then pairs it
        // with every car already processed this tick (`0x628CDC[]`): the human, the opponents
        // and the cops before it. *(Until 2026-09-25 a cop met the walls only inside a car–car
        // round, so one that ran wide drove through the edge line and off the road; until
        // 2026-09-26 the cops came after the traffic and never met each other.)* The body is the
        // cop's own car record (`CopCar::body`), so the impulse's `car+0x418` step
        // (0x496F1E / 0x496F25) and `Damage` land where the drive and the ground read them.
        if let Some(pol) = self.road.police.as_mut() {
            for i in 0..pol.cars.len() {
                if !pol.cars[i].active {
                    continue;
                }
                let (done, rest) = pol.cars.split_at_mut(i);
                let c = &mut rest[0];
                c.body.tiny = self.cheat_bits & CHEAT_TINY != 0;
                // `DERBY` `[0x657400]`, which only the Smash Up code sets (cheat bit 7): `Damage` takes ×1.5, not ×0.5 (0x49452F)
                c.body.derby = derby_on;
                c.body.rally = rally_on;
                c.sync_body_in();
                c.body.impact = 0.0;
                if c.body.collide_walls(&w.walls, &w.tree, &patch_height) {
                    ai_hits.push((AudioCar::Police(i), wall_sfx(&c.body)));
                }
                if let Some((impact, event)) = c.body.collide_props(&w.props, tick) {
                    ai_hits.push((AudioCar::Police(i), prop_sfx(impact, event)));
                }
                if !no_carcol {
                    let cop_mass = c.mass * tiny_k;
                    if let Some((ea, eb)) =
                        nfs_physics::vehicle::collide_pair(v, player_mass, &mut c.body, cop_mass, &mut racer_walls)
                    {
                        tracing::debug!(cop = i, ea, eb, "player hit a cop");
                        ground_event = None;
                        ai_hits.push((AudioCar::Police(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                        // the player's own `car+0x430` (A)
                        self.snd.player_loops.push_impact(nfs_audio::sfx::QueuedSfx::CarCar { effect: ea });
                    }
                    for (oi, o) in self.road.opponents.iter_mut().enumerate() {
                        if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(&mut o.vehicle, o.specs.mass * opp_k[oi], &mut c.body, cop_mass, &mut racer_walls) {
                            ai_hits.push((AudioCar::Opponent(oi), nfs_audio::sfx::QueuedSfx::CarCar { effect: ea }));
                            ai_hits.push((AudioCar::Police(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                        }
                    }
                    for (di, d) in done.iter_mut().enumerate().filter(|(_, d)| d.active) {
                        // the earlier cop is A (it was processed first); its body holds its
                        // kinematic state from its own pass
                        let d_mass = d.mass * tiny_k;
                        if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(&mut d.body, d_mass, &mut c.body, cop_mass, &mut racer_walls) {
                            d.sync_body_out();
                            ai_hits.push((AudioCar::Police(di), nfs_audio::sfx::QueuedSfx::CarCar { effect: ea }));
                            ai_hits.push((AudioCar::Police(i), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                        }
                    }
                }
                c.sync_body_out();
            }
        }
        *ground_event_out = ground_event;
    }

    /// **The collision pass, the human cars and the opponents** (see [`Self::simulate`]): the player's and the opponents' own walls and props, then their pairs, car by car (in race-index order in an exchange race). Moved out of `simulate` unchanged.
    fn tick_collide_racers(&mut self, ctx: &TickCtx, ci: &CollisionInputs, ai_hits: &mut Vec<(AudioCar, nfs_audio::sfx::QueuedSfx)>, ground_event_out: &mut Option<(f32, usize)>) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let exchange_in = &ctx.exchange_in;
        let (no_carcol, player_mass, opp_k) = (ci.no_carcol, ci.player_mass, &ci.opp_k);
        // `Car_wallCollide` 0x495020's corner skip: the drivable patch that contains the
        // corner in plan and its height, which the collider compares with the car's own y
        // (§6.1). It used to be a downward ray with a 6 m window, which is not the rule.
        let patch_height = |p: Vec3| w.collision.find_patch_height(p).and_then(|(word, h)| (word & 0xF != 0).then_some(h));
        let mut ground_event = *ground_event_out;
        // 0x4980D0's rounds collide each car's walls again between them (0x495020)
        let tick = self.tick as u32;
        let mut racer_walls = |c: &mut nfs_physics::vehicle::Vehicle| {
            c.collide_walls(&w.walls, &w.tree, &patch_height);
            c.collide_props(&w.props, tick);
        };
        // **Input exchange: the cars in race-index order.** Every machine runs the same pass over the
        // same cars — each car's walls and props, then its pairs with the cars already done, the
        // earlier of a pair being A — whichever of them is this machine's own (`v`), so the two
        // sides of a contact are the same numbers everywhere (`network.md` §11.17). The seats a
        // player's launch left without a race index (parked) follow the humans.
        if let Some(x) = exchange_in.as_ref() {
            #[derive(Clone, Copy, PartialEq)]
            enum H {
                Local,
                Seat(usize),
            }
            let mut order: Vec<(u32, H)> = vec![(x.local_index, H::Local)];
            order.extend(x.seats.iter().map(|e| (e.1, H::Seat(e.0))));
            order.sort_by_key(|e| e.0);
            let mut order: Vec<H> = order.into_iter().map(|e| e.1).collect();
            for oi in 0..self.road.opponents.len() {
                if !order.contains(&H::Seat(oi)) {
                    order.push(H::Seat(oi));
                }
            }
            let mut done: Vec<H> = Vec::new();
            for who in order.iter().copied() {
                // this car's own walls and props (0x495020 / 0x498DE0)
                match who {
                    H::Local => {
                        v.impact = 0.0;
                        if v.collide_walls(&w.walls, &w.tree, &patch_height) {
                            ground_event = None;
                            self.wall_hits += 1;
                            self.snd.player_loops.push_impact(wall_sfx(v));
                            v.impact = 0.0;
                        }
                        let prop_hit = v.collide_props(&w.props, self.tick as u32);
                        if prop_hit.is_some() {
                            ground_event = None;
                        }
                        if let Some((impact, event)) = prop_hit {
                            self.snd.player_loops.push_impact(prop_sfx(impact, event));
                        }
                    }
                    H::Seat(oi) => {
                        let o = &mut self.road.opponents[oi];
                        o.vehicle.impact = 0.0;
                        if o.vehicle.collide_walls(&w.walls, &w.tree, &patch_height) {
                            o.wall_hits += 1;
                            ai_hits.push((AudioCar::Opponent(oi), wall_sfx(&o.vehicle)));
                        }
                        if let Some((impact, event)) = o.vehicle.collide_props(&w.props, tick) {
                            ai_hits.push((AudioCar::Opponent(oi), prop_sfx(impact, event)));
                        }
                    }
                }
                if !no_carcol {
                    for d in done.iter().copied() {
                        // A = the car done earlier (`d`), B = this one (`who`)
                        let mass_of = |h: H, opp: &[RaceCar]| match h {
                            H::Local => player_mass,
                            H::Seat(k) => opp[k].specs.mass * opp_k[k],
                        };
                        let (ma, mb) = (mass_of(d, &self.road.opponents), mass_of(who, &self.road.opponents));
                        let hit = match (d, who) {
                            (H::Local, H::Seat(b)) => nfs_physics::vehicle::collide_pair(v, ma, &mut self.road.opponents[b].vehicle, mb, &mut racer_walls),
                            (H::Seat(a), H::Local) => nfs_physics::vehicle::collide_pair(&mut self.road.opponents[a].vehicle, ma, v, mb, &mut racer_walls),
                            (H::Seat(a), H::Seat(b)) => {
                                let (lo, hi) = (a.min(b), a.max(b));
                                let (head, tail) = self.road.opponents.split_at_mut(hi);
                                let (ca, cb) = if a < b { (&mut head[lo].vehicle, &mut tail[0].vehicle) } else { (&mut tail[0].vehicle, &mut head[lo].vehicle) };
                                nfs_physics::vehicle::collide_pair(ca, ma, cb, mb, &mut racer_walls)
                            }
                            (H::Local, H::Local) => None,
                        };
                        if let Some((ea, eb)) = hit {
                            for (h, effect) in [(d, ea), (who, eb)] {
                                match h {
                                    H::Local => {
                                        ground_event = None;
                                        self.snd.player_loops.push_impact(nfs_audio::sfx::QueuedSfx::CarCar { effect });
                                    }
                                    H::Seat(k) => ai_hits.push((AudioCar::Opponent(k), nfs_audio::sfx::QueuedSfx::CarCar { effect })),
                                }
                            }
                        }
                    }
                }
                done.push(who);
            }
        } else {
            // the human's pass: its walls and props (no earlier car to pair with)
            v.impact = 0.0;
            if v.collide_walls(&w.walls, &w.tree, &patch_height) {
                ground_event = None;
                self.wall_hits += 1;
                self.snd.player_loops.push_impact(wall_sfx(v));
                v.impact = 0.0;
            }
            let prop_hit = v.collide_props(&w.props, self.tick as u32);
            if prop_hit.is_some() {
                ground_event = None;
            }
            if let Some((impact, event)) = prop_hit {
                self.snd.player_loops.push_impact(prop_sfx(impact, event));
            }
            // each opponent's pass: its walls and props, then the human and the opponents before it.
            // Each body's own impact (`collide_pair`, the one `collide_cars_with` takes the larger
            // of) goes to its own queue: the player's plays, the other car's is queued. 0x40F740
            // reads each car's own `car+0x430`, which 0x496860 stores per body (A 0x496B42…0x496B91,
            // B 0x496BEF…0x496C3E) — the player is always A here, so its one-shot takes `ea`
            // (until 2026-09-26 it took the larger of the two)
            for oi in 0..self.road.opponents.len() {
                let (done, rest) = self.road.opponents.split_at_mut(oi);
                let o = &mut rest[0];
                // the impact `car+0x430` 0x40F740 turns into the opponent's queued one-shot
                o.vehicle.impact = 0.0;
                if o.vehicle.collide_walls(&w.walls, &w.tree, &patch_height) {
                    o.wall_hits += 1;
                    ai_hits.push((AudioCar::Opponent(oi), wall_sfx(&o.vehicle)));
                }
                if let Some((impact, event)) = o.vehicle.collide_props(&w.props, tick) {
                    ai_hits.push((AudioCar::Opponent(oi), prop_sfx(impact, event)));
                }
                if no_carcol {
                    continue;
                }
                let o_mass = o.specs.mass * opp_k[oi];
                if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(v, player_mass, &mut o.vehicle, o_mass, &mut racer_walls) {
                    ai_hits.push((AudioCar::Opponent(oi), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                    ground_event = None;
                    // the player's own `car+0x430` (A)
                    self.snd.player_loops.push_impact(nfs_audio::sfx::QueuedSfx::CarCar { effect: ea });
                }
                for (di, d) in done.iter_mut().enumerate() {
                    if let Some((ea, eb)) = nfs_physics::vehicle::collide_pair(&mut d.vehicle, d.specs.mass * opp_k[di], &mut o.vehicle, o_mass, &mut racer_walls) {
                        ai_hits.push((AudioCar::Opponent(di), nfs_audio::sfx::QueuedSfx::CarCar { effect: ea }));
                        ai_hits.push((AudioCar::Opponent(oi), nfs_audio::sfx::QueuedSfx::CarCar { effect: eb }));
                    }
                }
            }
        }
        *ground_event_out = ground_event;
    }

    /// **The player's contact word after the collision pass** (see [`Self::simulate`]): the force-feedback jolt, the sparks and the body-ground one-shot, on what the tick's contacts left in the car (`0x40F740`). Moved out of `simulate` unchanged.
    fn tick_player_contact(&mut self, ctx: &TickCtx, ground_event: Option<(f32, usize)>) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let ff_replay = ctx.ff_replay;
        // **the sparks** 0x43F4C0, from the same 0x40F740 (0x40F97E / 0x40F9D7): the tick's
        // word `car+0x438` of kind 4 (a wall) or 3 (the body on the ground) over the 1.5 gate
        // (0x40F944) on the tick's magnitude `car+0x430` fires them at the contact `car+0x440`
        // with the velocity `car+0x33C` (`nfs_game::car_effects`). Word and magnitude are what
        // the tick's writers left in `Vehicle::contact_kind` / `contact_mag` — the last writer's
        // kind and the largest-or-assigned magnitude, the pair rounds' wall hits (0x495020)
        // included; 0x40F740 clears them (`take_contact`). *(Until batch A6 the port took the
        // last event's own magnitude and let the pair rounds' wall hits raise none.)*
        let (contact_mag, contact_kind) = v.take_contact();
        // 0x40F740 → 0x412230: the force-feedback jolt, on the same words (the car's body frame is
        // `R·(point − position)`, 0x4122A8…0x4122CB)
        if contact_mag > 0.0 {
            let body_point = v.orientation * (v.last_contact - v.position);
            super::force_feedback::ff_collision(&mut self.ff, &mut self.gamepads, self.ff_levels, ff_replay, self.tick, &nfs_physics::force_feedback::FfCar::from_vehicle(v, &self.specs), contact_kind.word(), body_point, contact_mag);
        }
        if matches!(contact_kind, ContactKind::Ground | ContactKind::Wall)
            && contact_mag > 1.5
            && let Some(fx) = self.car_effects.as_mut()
        {
            tracing::debug!(contact_mag, contact_kind = contact_kind.word(), "sparks: the player's contact word (0x40F740)");
            fx.sparks(0, v.last_contact, v.velocity, v.position, self.camera.position);
        }
        // the player's body–ground one-shot (0x40F740 kind 3: CarBody × `0x5CC4D8[car+0x464]`
        // at `car+0x430 / 8` over 1.5), when no later contact of the tick replaced its word
        if let Some((effect, nibble)) = ground_event {
            tracing::debug!(effect, nibble, "player body-ground one-shot (0x40F740 kind 3)");
            self.snd.player_loops.push_impact(nfs_audio::sfx::QueuedSfx::Ground { effect, nibble });
        }
    }

    /// **The non-player cars' sound queues** (see [`Self::simulate`]): each body's landing, roll-over and crash flags, the body-ground events and the tick's collision events (`ai_hits`), queued in the order the original pushes them. Moved out of `simulate` unchanged.
    fn tick_ai_sound_queues(&mut self, ai_hits: &mut Vec<(AudioCar, nfs_audio::sfx::QueuedSfx)>) {
        // The non-player cars' sound queues, in the order the original pushes (§4 of sfx.md):
        // `Car_ground`'s landing / roll-over (0x49C1E6 / 0x49C0DE, priority 0x1E), `Damage`'s
        // crash (0x494500, inside the collision pass), then 0x40F740's collision one-shot in
        // `Car_postImpact` 0x410280 (priority 0x32). The loops (0x40FFF0) are produced in the
        // audio pass. The cars' own flags are outputs only; taking them changes no physics.
        {
            let q = &mut self.snd.ai_sfx;
            // `Car_ground`'s body–ground event words, held until the tick's contacts are known:
            // a wall, prop or car contact of the same car writes `car+0x438` after it, and
            // 0x40F740 pushes one event per tick from the last word
            let mut grounds: Vec<(AudioCar, f32, usize)> = Vec::new();
            let mut take = |key: AudioCar, b: &mut nfs_physics::vehicle::Vehicle| {
                if let Some((effect, nibble)) = b.ground_sound.take() {
                    grounds.push((key, effect, nibble));
                }
                let e = q.entry(key).or_default();
                if let Some(closing) = b.landing_sound.take() {
                    e.push(nfs_audio::sfx::QueuedSfx::Landing { closing });
                }
                if std::mem::take(&mut b.rollover_sound) {
                    e.push(nfs_audio::sfx::QueuedSfx::Crash);
                }
                if std::mem::take(&mut b.crash_sound) {
                    e.push(nfs_audio::sfx::QueuedSfx::Crash);
                }
            };
            for (oi, o) in self.road.opponents.iter_mut().enumerate() {
                take(AudioCar::Opponent(oi), &mut o.vehicle);
            }
            if let Some(pol) = self.road.police.as_mut() {
                for (i, c) in pol.cars.iter_mut().enumerate() {
                    take(AudioCar::Police(i), &mut c.body);
                }
            }
            if let Some(tr) = self.road.traffic.as_mut() {
                for (i, b) in tr.bodies.iter_mut().enumerate() {
                    take(AudioCar::Traffic(i), b);
                }
            }
            for (key, effect, nibble) in grounds {
                let replaced = ai_hits.iter().any(|(k, _)| *k == key);
                tracing::debug!(car = ?key, effect, nibble, replaced, "ai body-ground event (0x40F740 kind 3)");
                if !replaced {
                    q.entry(key).or_default().push_impact(nfs_audio::sfx::QueuedSfx::Ground { effect, nibble });
                }
            }
            for (key, e) in ai_hits.drain(..) {
                q.entry(key).or_default().push_impact(e);
            }
        }
    }

    /// **The other cars' sparks** (see [`Self::simulate`]): each opponent's, cop's and traffic car's contact word and magnitude, taken and cleared, fire the sparks (`0x40F740` → `0x43F4C0`). Moved out of `simulate` unchanged.
    fn tick_ai_sparks(&mut self) {
        // the other cars' sparks (0x40F740 → 0x43F4C0, as the player's above): each body's tick
        // word and magnitude, taken and cleared
        {
            let mut fired: Vec<(usize, Vec3, Vec3, Vec3)> = Vec::new();
            let n_opp = self.road.opponents.len();
            let n_pol = self.road.police.as_ref().map_or(0, |p| p.cars.len());
            let mut check = |k: usize, b: &mut nfs_physics::vehicle::Vehicle| {
                let (mag, kind) = b.take_contact();
                if matches!(kind, ContactKind::Ground | ContactKind::Wall) && mag > 1.5 {
                    fired.push((k, b.last_contact, b.velocity, b.position));
                }
            };
            for (i, o) in self.road.opponents.iter_mut().enumerate() {
                check(1 + i, &mut o.vehicle);
            }
            if let Some(pol) = self.road.police.as_mut() {
                for (i, c) in pol.cars.iter_mut().enumerate() {
                    check(1 + n_opp + i, &mut c.body);
                }
            }
            if let Some(tr) = self.road.traffic.as_mut() {
                for (i, b) in tr.bodies.iter_mut().enumerate() {
                    check(1 + n_opp + n_pol + i, b);
                }
            }
            if let Some(fx) = self.car_effects.as_mut() {
                for (k, contact, velocity, position) in fired {
                    tracing::debug!(car = k, "sparks: a car's contact word (0x40F740)");
                    fx.sparks(k, contact, velocity, position, self.camera.position);
                }
            }
        }
    }

    /// **The player's post-impact pass** (see [`Self::simulate`]): the crash countdown, the reset request (the key, the exchange's command 5, the scripted bot's stand-in) and the reset onto the slice. Moved out of `simulate` unchanged; the request goes to `st.asked` for the remote cars of a positional race.
    fn tick_player_reset(&mut self, ctx: &TickCtx, st: &mut TickState) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let Some(w) = self.world.as_ref() else { return };
        let exchange_in = &ctx.exchange_in;
        // the post-impact pass (`Car_postImpact` 0x410280, priority 0x32, after the collisions):
        // reset to the current slice when 0x40F740's crash countdown runs out
        // (`Vehicle::reset_countdown_tick`), which runs eight times faster off the road
        // surface — `car+0x464 == 0`, the body tracker's word (0x49AFA1…0x49AFDB), which
        // `find_patch` answers. The placement is 0x40F540's: `Car_placeAtSlice` 0x49B090.
        // diagnostic: `NFS_ESCAPE_LOG=x0,x1` prints the player's state while its x is in that
        // band, for chasing a car that gets past the wall line
        if let Some(band) = nfs_game::config::cfg().nfs_escape_log.as_str() {
            let mut it = band.split(',').filter_map(|v| v.parse::<f32>().ok());
            if let (Some(a), Some(b)) = (it.next(), it.next())
                && v.position.x >= a.min(b)
                && v.position.x <= a.max(b)
            {
                tracing::info!(t = self.tick, pos = ?v.position, kmh = v.speed() * 3.6, patch = w.collision.find_patch(v.position) & 0xF, wall = ?w.walls.nearest(v.position, 20.0).map(|(n, d)| (n.x, n.y, d)), "escape watch");
            }
        }
        let off_surface = w.collision.find_patch(v.position) & 0xF == 0;
        let crashed = v.reset_countdown_tick(off_surface);
        // headless runs stand in for a human pressing reset: a scripted bot that is stuck below
        // 2 m/s for 5 s resets itself (the original's AI rule `car+0xD8B = 5`, ai.md §3.2) so a
        // wedged car cannot block a regression run. Interactive play uses the key only.
        if self.scripted {
            self.stuck_ticks = if v.speed() < 2.0 && v.race_started {
                self.stuck_ticks + 1
            } else {
                0
            };
        }
        let bot_stuck = self.stuck_ticks >= 160;
        if bot_stuck {
            self.stuck_ticks = 0;
        }
        // 0x40F7EB: a reset request (the player's key, or the bot's stand-in for it) goes through
        // `Vehicle::reset_request`'s gates; 0x40F540 itself does nothing in a skill test on the
        // skidpad (`[0x6573EC] == 4` and the track "skidpad", 0x40F540…0x40F565)
        // in an input-exchange race the request is the latched sample's command 5 (`car+0xD8B`, 0x40F7EB)
        // — every machine's copy of the car reads the same one — and the scripted bot's own reset
        // (a stand-in for a key) is off, since it would reset this machine's copy alone
        let asked = match exchange_in.as_ref() {
            Some(x) => x.local.command == nfs_net::exchange::CMD_RESET,
            None => self.input.take_pressed(Action::Reset) || bot_stuck,
        };
        let requested = v.reset_request(asked, self.tick);
        let exempt = self.skill.is_some() && self.track.name.eq_ignore_ascii_case("skidpad");
        // a car behind its start on a point-to-point track goes back to its start slice
        // (0x4174A2 → 0x40F540(car, 3, 0), which starts from `car+0xAFC`, 0x40F640…0x40F658)
        let behind = self.race.as_mut().is_some_and(|r| std::mem::take(&mut r.behind_start));
        if (crashed || requested || behind)
            && !exempt
            && let (Some(t), Some(r)) = (&self.slices, &self.race)
        {
            self.resets += 1;
            let occupied: Vec<usize> = self
                .road
                .opponents
                .iter()
                .filter(|o| !o.eliminated)
                .map(|o| o.progress.slice)
                .chain(reset_blockers(self.road.police.as_ref(), self.road.traffic.as_ref(), &self.smackables))
                .collect();
            let from = if behind { r.start_slice } else { r.slice };
            let slice = free_reset_slice(t, &w.collision, from, &occupied);
            let sl = &t.slices[slice];
            // 0x40F6DE: the human's offset is zero — `Car_placeAtSlice` 0x49B090 puts the
            // origin on the slice centre and drops the body onto the road (`place_at_slice`),
            // dir `car+0xEAC` (+1 along this port's lap)
            v.place_at_slice(&w.collision, &self.specs, sl.centre, sl.forward, Vec3::ZERO, false, false);
            v.velocity = Vec3::ZERO;
            // 0x40F400: the spin and `+0x418` the placement's `Susp_update` left
            v.angular_velocity = Vec3::ZERO;
            v.airborne_count = 0;
            v.rpm = 0.0;
            v.gear = 2;
            v.rearm_after_reset();
            // 0x40F59D: every reset clears the countdown
            v.reset_countdown = 0;
            tracing::info!(slice, "car reset to track");
        }
        if let (Some(t), Some(r)) = (&self.slices, &self.race) {
            let off_surface = w.collision.find_patch(v.position) & 0xF == 0;
            v.reset_countdown_start(t.slices[r.slice].up.normalize(), off_surface);
        }
        st.asked = asked;
    }

    /// **The opponents' and the cops' post-impact pass** (see [`Self::simulate`]): the AI reset request and the crash countdown of every opponent (a human seat's request is its sample's command 5), then the cops' pass. Moved out of `simulate` unchanged; `st.asked` is the player's reset request.
    fn tick_opponent_resets(&mut self, ctx: &TickCtx, st: &TickState) {
        let Some(w) = self.world.as_ref() else { return };
        let (exchange_in, net_seats) = (&ctx.exchange_in, &ctx.net_seats);
        let asked = st.asked;
        // the opponents' post-impact pass, after the player's (each car's 0x410280 slot, in the
        // cars' order): the AI reset request and 0x40F740's crash countdown
        if let Some(t) = &self.slices {
            let racer_slices: Vec<Option<usize>> = std::iter::once(self.race.as_ref().map(|r| r.slice))
                .chain(self.road.opponents.iter().map(|o| (!o.eliminated).then_some(o.progress.slice)))
                .collect();
            // …and the cops', the traffic's and the smackables' (0x40F480)
            let blockers = reset_blockers(self.road.police.as_ref(), self.road.traffic.as_ref(), &self.smackables);
            for (oi, o) in self.road.opponents.iter_mut().enumerate() {
                // stuck: reset onto the slice (AI reset request car+0xD8B = 5)
                // the three arms are kept apart for the log (`ai.md` §3.2): facing back under
                // 10 m/s, five seconds under 2 m/s, and the dead car's countdown
                let (why_ai, facing) = match &o.controller {
                    Controller::Ai(ai) => (ai.reset_why, ai.dbg_facing),
                    _ => (None, 0.0),
                };
                // an input-exchange seat is a human car: its request is its sample's command 5
                let exchange_seat = exchange_in.as_ref().and_then(|x| x.seats.iter().find(|e| e.0 == oi));
                // a remote car of a positional race is a human car whose `car+0xD8B` is the **local** pad's
                // command in every arm (0x4991E0 at 0x499336), the AI arm's own reset request overwritten —
                // so the local reset key resets it (0x40F7EB), through the same gates as any car's
                let net_human = exchange_seat.is_none() && exchange_in.is_none() && net_seats.contains(&oi);
                let stuck = match (&mut o.controller, exchange_seat) {
                    (_, Some(e)) => e.2.command == nfs_net::exchange::CMD_RESET,
                    _ if net_human => asked,
                    (Controller::Ai(ai), None) => ai.wants_reset(),
                    _ => false,
                };
                // diagnostic: `NFS_SLOW_LOG=1` prints every opponent that is barely moving,
                // which is how a car that cannot get going is told from one that is merely slow
                if self.tick % 32 == 0
                    && o.vehicle.speed() < 3.0
                    && nfs_game::config::cfg().nfs_slow_log.is_set()
                {
                    let surf: Vec<u32> = o.vehicle.wheels.iter().map(|wh| w.collision.find_patch(wh.world_pos) & 0xF).collect();
                    tracing::info!(
                        opponent = oi,
                        t = self.tick / 64,
                        kmh = o.vehicle.speed() * 3.6,
                        rpm = o.vehicle.rpm,
                        gear = o.vehicle.gear,
                        engine = o.vehicle.engine_alive,
                        thr = o.vehicle.throttle,
                        arm = o.vehicle.engine_arm as u8,
                        alive_ticks = o.vehicle.ticks_alive,
                        brk = o.vehicle.brake,
                        contact = o.vehicle.wheels.iter().filter(|wh| wh.contact).count(),
                        surf = ?surf,
                        slip = ?o.vehicle.wheels.iter().map(|wh| wh.slip as i32).collect::<Vec<_>>(),
                        pos = ?o.vehicle.position,
                        "slow opponent"
                    );
                }
                // 0x40F740's crash countdown runs for the AI cars too, with the same
                // off-surface rule (`car+0x464 == 0`)
                let off_surface = w.collision.find_patch(o.vehicle.position) & 0xF == 0;
                let crashed = o.vehicle.reset_countdown_tick(off_surface);
                let stuck = o.vehicle.reset_request(stuck, self.tick);
                let behind = std::mem::take(&mut o.progress.behind_start);
                if stuck || crashed || behind {
                    let occupied: Vec<usize> = racer_slices
                        .iter()
                        .enumerate()
                        .filter(|&(k, _)| k != oi + 1)
                        .filter_map(|(_, s)| *s)
                        .chain(blockers.iter().copied())
                        .collect();
                    let from = if behind { o.progress.start_slice } else { o.progress.slice };
                    let slice = free_reset_slice(t, &w.collision, from, &occupied);
                    let sl = &t.slices[slice];
                    tracing::debug!(
                        opponent = oi,
                        why = if !stuck { "crash countdown" } else { why_ai.unwrap_or("reset request") },
                        from_slice = o.progress.slice,
                        to_slice = slice,
                        pos = ?o.vehicle.position,
                        kmh = o.vehicle.speed() * 3.6,
                        engine = o.vehicle.engine_alive,
                        killed_by = o.vehicle.engine_kill,
                        wheels_down = o.vehicle.wheels.iter().filter(|w| w.contact).count(),
                        gear = o.vehicle.gear,
                        facing,
                        "opponent reset"
                    );
                    // 0x40F6A2…0x40F6DA: an opponent (`car+0x52C & 8`) is handed its
                    // racing-line point on the slice, `{line.x, centre.y, line.z}` (0x403450,
                    // `.lsp`), which 0x49B090 takes as a world position once the race runs
                    // (tick > 0x1C0); then the drop onto the road (`place_at_slice`)
                    let line = self
                        .racing_line
                        .as_ref()
                        .and_then(|l| l.points.get(slice))
                        .map_or(Vec3::ZERO, |p| Vec3::new(p[0], sl.centre.y, p[1]));
                    if exchange_seat.is_some() || net_human {
                        // a human's reset (0x40F6DE: offset zero, `car+0x52C & 8` clear), the player's own block
                        o.vehicle.place_at_slice(&w.collision, &o.specs, sl.centre, sl.forward, Vec3::ZERO, false, false);
                        o.vehicle.velocity = Vec3::ZERO;
                        o.vehicle.angular_velocity = Vec3::ZERO;
                        o.vehicle.airborne_count = 0;
                        o.vehicle.rpm = 0.0;
                        o.vehicle.gear = 2;
                        o.vehicle.rearm_after_reset();
                        o.vehicle.reset_countdown = 0;
                    } else {
                    o.vehicle.reset_to(o.vehicle.position, o.vehicle.orientation);
                    o.vehicle.place_at_slice(&w.collision, &o.specs, sl.centre, sl.forward, line, true, false);
                    // 0x40F711…0x40F724: the velocity, then 0x40F400 the spin and `+0x418`
                    o.vehicle.velocity = Vec3::ZERO;
                    o.vehicle.angular_velocity = Vec3::ZERO;
                    o.vehicle.airborne_count = 0;
                    }
                    // 0x49E421 / 0x4A03A3 / 0x4A047F: a reset clears the branch latch
                    // (`car+0xE94 = 0`) and the tracker's preferred section (`car+0xC = -1`)
                    if let Controller::Ai(d) = &mut o.controller {
                        d.clear_branch();
                    }
                    o.resets += 1;
                }
                let off_surface = w.collision.find_patch(o.vehicle.position) & 0xF == 0;
                o.vehicle.reset_countdown_start(t.slices[o.progress.slice].up.normalize(), off_surface);
            }
            // …and the cops' (their `car+0xE30` slots follow the opponents'): 0x40F740's
            // countdown, loaded by `Damage` on a hard hit, and its reset 0x40F540
            if let Some(pol) = self.road.police.as_mut() {
                let occupied: Vec<usize> = racer_slices
                    .iter()
                    .filter_map(|s| *s)
                    .chain(reset_blockers(None, self.road.traffic.as_ref(), &self.smackables))
                    .collect();
                pol.post_impact(t, &w.collision, &occupied);
            }
        }
    }

    /// **The traffic's post-impact pass** (see [`Self::simulate`]): the crash countdown and the in-place reset of each traffic car, then the second race-order sort whose swaps are the overtake triggers (`0x417800`'s end). Moved out of `simulate` unchanged.
    fn tick_traffic_post_impact(&mut self, ctx: &TickCtx) {
        let Some(w) = self.world.as_ref() else { return };
        let exch_order = &ctx.exch_order;
        let n_seat_cars = ctx.n_seat_cars;
        let reg_of = |s: usize| ctx.reg_of(s);
        // the traffic's post-impact pass (each car's own 0x410280 slot, after the racers' in
        // `0x5E8B34` order): 0x40F740's crash countdown and the in-place reset 0x40F540
        // (`nfs_game::traffic` module docs); the racers', the cops' and the smackables' slices are
        // what 0x40F480 skips
        if let (Some(tr), Some(t)) = (self.road.traffic.as_mut(), self.slices.as_ref()) {
            let occupied: Vec<usize> = self
                .race
                .as_ref()
                .map(|r| r.slice)
                .into_iter()
                .chain(self.road.opponents.iter().filter(|o| !o.eliminated).map(|o| o.progress.slice))
                .chain(reset_blockers(self.road.police.as_ref(), None, &self.smackables))
                .collect();
            tr.post_impact(t, Some(&w.collision), &occupied);
            // `0x417800`'s end (0x4179BF → 0x417710): the second race-order sort on `car+0xAF8`,
            // whose swaps are 0x401580's triggers 4 / 5 (`traffic.md` §34.8). The keys as they
            // stand before this tick's Sc64: the human's and the opponents' `lap + frac −
            // unlaps`, a cop's and a traffic car's bare lap fraction (0 out of the world), as
            // 0x4169B0 wrote it on the even tick before the move (`CopCar::progress`,
            // `TrafficCar::progress`).
            let cops: Vec<f32> = self
                .road
                .police
                .as_ref()
                .map(|p| p.cars.iter().map(|c| if c.active { c.progress } else { 0.0 }).collect())
                .unwrap_or_default();
            let keys: Vec<f32> = std::iter::once(self.race.as_ref().map_or(0.0, |r| r.progress))
                .chain(self.road.opponents.iter().map(|o| o.progress.progress))
                .chain(cops.iter().copied())
                .chain(tr.cars.iter().map(|c| c.progress))
                .collect();
            let keys = if exch_order.is_empty() {
                keys
            } else {
                let mut k2 = keys.clone();
                for s in 0..=n_seat_cars {
                    k2[reg_of(s)] = keys[s];
                }
                k2
            };
            tr.overtakes(&keys, self.tick, 1 + self.road.opponents.len() + cops.len());
        }
    }

    /// **The race order's first sort** (see [`Self::simulate`]): `0x417710`'s sort of `0x5E8B88[]` by progress into the lap over every registered car, which the next tick's AI, lane passes and cops read. Moved out of `simulate` unchanged.
    fn tick_race_order(&mut self, ctx: &TickCtx) {
        let exch_order = &ctx.exch_order;
        let n_seat_cars = ctx.n_seat_cars;
        let reg_of = |s: usize| ctx.reg_of(s);
        // `0x417710`'s first sort, the same call (0x4179BF): `0x5E8B88[]` ascending by
        // `car+0xAF8 − car+0xB20` — a racer's progress less the laps it has timed, a cop's or a
        // traffic car's bare lap fraction (`+0xB20` = 0), 0 for a car out of the world — over
        // every registered car (the human, the opponents, the cops, the traffic). What the AI,
        // the lane passes and the cops read during the next tick pair.
        if self.slices.is_some() {
            let keys: Vec<f32> = std::iter::once(self.race.as_ref().map_or(0.0, |r| r.progress - r.lap_times.len() as f32))
                .chain(self.road.opponents.iter().map(|o| o.progress.progress - o.progress.lap_times.len() as f32))
                .chain(self.road.police.iter().flat_map(|p| p.cars.iter().map(|c| if c.active { c.progress } else { 0.0 })))
                .chain(self.road.traffic.iter().flat_map(|tr| tr.cars.iter().map(|c| if c.active { c.progress } else { 0.0 })))
                .collect();
            // in an exchange race the array is indexed by registration index (`reg_of`)
            let keys = if exch_order.is_empty() {
                keys
            } else {
                let mut k2 = keys.clone();
                for s in 0..=n_seat_cars {
                    k2[reg_of(s)] = keys[s];
                }
                k2
            };
            self.race_order.resort(&keys);
        }
    }

    /// **The Quick Knockout cut for the opponents** (see [`Self::simulate`], `modes.md` §4): the opponents that completed a lap this tick (`st.ko_laps`) and sit at the cut put themselves and every car behind out. Moved out of `simulate` unchanged.
    fn tick_quick_knockout(&mut self, st: &TickState) {
        // Quick Knockout cut for opponents that completed a lap this tick (modes.md §4)
        if self.quick_knockout && !st.ko_laps.is_empty() {
            let mut all: Vec<(usize, f32)> = self
                .road.opponents
                .iter()
                .enumerate()
                .map(|(i, o)| (i + 1, o.progress.progress))
                .collect();
            if let Some(r) = &self.race {
                all.push((0, r.progress));
            }
            all.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            let pos_of = |d: usize| {
                all.iter()
                    .position(|(i, _)| *i == d)
                    .map(|p| p as i64 + 1)
                    .unwrap_or(99)
            };
            for &(oi, lap) in &st.ko_laps {
                let cut = self.num_laps as i64 - lap as i64 + 2;
                if pos_of(oi + 1) == cut {
                    for &(d, _) in all.iter().filter(|(d, _)| pos_of(*d) >= cut) {
                        if d == 0 {
                            if let Some(r) = self.race.as_mut()
                                && !r.finished
                            {
                                self.player_eliminated = true;
                                r.finished = true;
                                r.progress -= 0.01;
                                r.tick = self.tick;
                                if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                                    sp.say(a, "KO-eliminated.bnk");
                                }
                            }
                        } else if let Some(o) = self.road.opponents.get_mut(d - 1)
                            && !o.eliminated
                        {
                            o.eliminated = true;
                            o.progress.finished = true;
                            o.progress.progress -= 0.01;
                            o.progress.tick = self.tick;
                            // it keeps its AI: `car+0xB50 = 3` pulls it over (0x40C60C)
                            tracing::info!(opponent = o.name, lap, "knocked out");
                        }
                    }
                }
            }
        }
    }

    /// **`DERBY`'s knockout** (see [`Self::simulate`]): for each racer still racing, in registration order, the damage-zone outcome (out / last one standing) and the room's race-time message. Moved out of `simulate` unchanged.
    fn tick_derby_outcomes(&mut self, ctx: &TickCtx) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let (exchange_in, exch_order) = (&ctx.exchange_in, &ctx.exch_order);
        // **`DERBY`'s knockout** (`Race_updateProgress` 0x416310's game-type-3 arm, 0x416407…0x41646A —
        // `nfs_game::race::derby_outcome`): for each racer still racing, in registration order (the racer
        // array `0x5E8C00[]`), state 3 when all eight ring zones are past 30, else state 2 when every
        // other racer is out; and each machine's `0x48DA60(tick − 0x1C0, tick)` says so to the room —
        // for every racer that ends, whichever machine's car it is (`network.md` §11.18). A race by
        // laps is not run: this arm ends every path of the function before the lap code (0x4164C0).
        //
        // **A positional race** (`Comms.Positional` 1) runs the same loop over the same racers, on this
        // machine's own copy of each: the local car's ten zones are real, a remote car's are whatever
        // this machine's own contacts with it did (its `0xBE` state carries no damage — 0x4A0DF0's block
        // has no zone), so each machine finds *its own* car out and the rest by the "last one wins" count
        // — `network.md` §11.20. The order is by race index (`0x5E8C00[]`, the same on every machine).
        let derby_order: Vec<HumanCar> = if !self.net_derby {
            Vec::new()
        } else if !exch_order.is_empty() {
            exch_order.clone()
        } else if let Some(n) = self.net.session.as_ref().filter(|n| n.in_race != 0) {
            let mut o: Vec<(u32, HumanCar)> = n.local_race_index().map(|i| (i, HumanCar::Local)).into_iter().collect();
            o.extend((0..self.road.opponents.len()).filter_map(|k| n.seat_race_index(k).map(|i| (i, HumanCar::Seat(k)))));
            o.sort_by_key(|e| e.0);
            o.into_iter().map(|e| e.1).collect()
        } else {
            Vec::new()
        };
        if !derby_order.is_empty() {
            let tick = self.tick;
            let racers = derby_order.len();
            // **Test glue** (`NFS_DERBY_SMASH=<race index>:<tick>`, set on every machine): that car's ten
            // damage zones are set to 31 at that tick — a smash-up the two-instance runs would not
            // otherwise reach in a few seconds
            if let Some((idx, at)) = nfs_game::config::cfg().nfs_derby_smash.as_str()
                .and_then(|v| v.split_once(':').and_then(|(i, t)| Some((i.parse::<u32>().ok()?, t.parse::<u64>().ok()?))))
                && tick == at
            {
                // (in a positional race the car's own machine alone has its zones)
                let local_index = exchange_in.as_ref().map(|x| x.local_index).or_else(|| self.net.session.as_ref().and_then(|n| n.local_race_index()));
                if Some(idx) == local_index {
                    v.damage = [31.0; 10];
                } else if let Some(x) = exchange_in.as_ref()
                    && let Some(e) = x.seats.iter().find(|e| e.1 == idx)
                    && let Some(o) = self.road.opponents.get_mut(e.0)
                {
                    o.vehicle.damage = [31.0; 10];
                }
            }
            let mut out = derby_order
                .iter()
                .filter(|h| match **h {
                    HumanCar::Local => self.player_eliminated,
                    HumanCar::Seat(k) => self.road.opponents.get(k).is_some_and(|o| o.eliminated),
                })
                .count();
            for h in derby_order.iter().copied() {
                let (racing, zones): (bool, [f32; 10]) = match h {
                    HumanCar::Local => (self.race.as_ref().is_some_and(|r| !r.finished), v.damage),
                    HumanCar::Seat(k) => match self.road.opponents.get(k) {
                        Some(o) => (!o.progress.finished, o.vehicle.damage),
                        None => continue,
                    },
                };
                if !racing {
                    continue;
                }
                let outcome = nfs_game::race::derby_outcome(&zones, out, racers);
                if outcome == nfs_game::race::DerbyOutcome::Racing {
                    continue;
                }
                let out_now = outcome == nfs_game::race::DerbyOutcome::Out;
                match h {
                    HumanCar::Local => {
                        if let Some(r) = self.race.as_mut() {
                            r.finished = true;
                            r.tick = tick;
                        }
                        self.player_eliminated = out_now;
                    }
                    HumanCar::Seat(k) => {
                        if let Some(o) = self.road.opponents.get_mut(k) {
                            o.progress.finished = true;
                            o.progress.tick = tick;
                            o.eliminated = out_now;
                        }
                    }
                }
                if out_now {
                    out += 1;
                }
                tracing::info!(target: "netplay", ?h, tick, out = out_now, "DERBY: a racer ends its race (0x416407…0x41646A)");
                if let Some(n) = self.net.session.as_mut() {
                    n.send_race_time((tick as i32).wrapping_sub(0x1C0), tick as i32);
                }
            }
        }
    }

    /// **The lap trackers and the player's race progress** (see [`Self::simulate`]): the exchange seats' trackers, then the player's `Race::update` — wrong-way gate, lap speech, the finish and the Quick Knockout cut for the player. Moved out of `simulate` unchanged.
    fn tick_race_progress(&mut self, ctx: &TickCtx) {
        let Some(v) = self.vehicle.as_mut() else { return };
        let exchange_in = &ctx.exchange_in;
        // the input-exchange seats' lap trackers, beside the player's (which follows): every machine's
        // copy of a car is tracked at the same point of the tick
        if let (Some(x), Some(t)) = (exchange_in.as_ref(), &self.slices) {
            for e in &x.seats {
                let o = &mut self.road.opponents[e.0];
                o.progress.update_tracked(t, o.vehicle.position, o.vehicle.forward(), self.tick, self.num_laps, None);
            }
        }
        if let (Some(t), Some(r)) = (&self.slices, self.race.as_mut()) {
            let was_finished = r.finished;
            let steps_before = r.wrong_way_steps;
            // 0x4A7660's gates: finished or in a mission clears the counter; `word car+0x418`
            // non-zero holds it — `Vehicle::airborne_count`, which the suspension, the landing,
            // a car–car hit and a heavy `Damage` write. `+0x1104` (0x4948F0, cleared by
            // 0x40F865…0x40F876 once `+0x418` is 0) is not carried. `+0xDA8` (AI-driven) and
            // `+0x544` (post-finish) are 0 for the player in a race
            let gate = if r.finished || self.mission.is_some() {
                nfs_game::race::WrongWayGate::Reset
            } else if v.airborne_count != 0 {
                nfs_game::race::WrongWayGate::Hold
            } else {
                nfs_game::race::WrongWayGate::Count
            };
            r.wrong_way_gate = gate;
            // a skill test runs 0x416170 in place of 0x416310 (0x4169FE): no lap logic, so the
            // cone drive's teleport (or any jump across the line) counts no lap and finishes
            // nothing. *(Until 2026-09-27 the port ran the race's lap logic too: on canyon
            // mission 2 with `NFS_CONE_DRIVE=0` the teleport past the finish fraction counted a
            // 600-tick "lap" and then one every step, twelve by the finish — all in
            // `lap_times`.)* The gate is `Reset` in a mission, so the wrong-way counters
            // `update` would clear stay 0.
            let lap_done = if self.skill.is_some() {
                r.track_only(t, v.position, None);
                false
            } else {
                r.update(t, v.position, v.forward(), self.tick, self.num_laps)
            };
            // the "wrong way" cue at 64, 96 and 128 steps (0x4A7734…0x4A7744)
            if r.wrong_way_steps != steps_before
                && nfs_game::race::wrong_way_cue(r.wrong_way_steps)
                && self.mission.is_none()
                && self.skill.is_none()
                && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
            {
                s.wrong_way_cue(a);
            }
            let n_cars = self.road.opponents.len() + 1;
            let place = 1 + self
                .road.opponents
                .iter()
                .filter(|o| o.progress.progress > r.progress)
                .count();
            if lap_done {
                tracing::info!(
                    lap = r.laps,
                    tick = self.tick,
                    time_s = r.lap_times.last().map(|t| *t as f32 / TICKS_PER_SEC_F32),
                    "lap completed"
                );
                if let (Some(_), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                    // every variant is a draw of 0x538040 (`nfs_audio::speech::phrase`), in
                    // the order the wrappers are called; until 2026-09-26 the port took the
                    // tick (`tick ^ laps·7919`) and named the banks itself
                    use nfs_audio::speech::phrase;
                    let rng = &mut self.fe_rng;
                    let mut draw = || rng.next_u32();
                    if r.finished && !was_finished {
                        // nothing at the line: the placing speech is `HCongrats`' first draw
                        // (0x41D16B / 0x41D187), over the automatic replay
                        // (`App::open_result_pages`). *(Until 2026-09-27 the port said it here,
                        // in a playback too.)*
                    } else if self.num_laps < 99
                        // the whole bundle is skipped while `REPLAY` is 2 or 3 (0x427874…
                        // 0x427886) — a playback, run out or not. *(Until 2026-09-27 the port
                        // said it in a playback too.)*
                        && !self.replay.stream.as_ref().is_some_and(|p| matches!(p.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
                    {
                        // Race_lapSpeech 0x4277D0 bundle (RE/notes/sfx.md §7.2): kLap 0x4A6640(n)
                        let n = r.laps.saturating_sub(1); // lapsDone − 1
                        sp.say_id(phrase::lap(n, &mut draw));
                        let lap_m = t.lap_len as f32 * nfs_game::race::SLICE_SPACING;
                        let leader = self
                            .road.opponents
                            .iter()
                            .map(|o| o.progress.progress)
                            .fold(r.progress, f32::max);
                        let speed = v.speed();
                        if n_cars == 2 {
                            // kPositionRel 0x4A61A0(g): seconds ahead (+) / behind (−) of the
                            // rival, `38 + bucket` — no draw
                            if let Some(o) = self.road.opponents.first()
                                && speed > 1.0
                            {
                                let g =
                                    ((r.progress - o.progress.progress) * lap_m / speed) as i32;
                                let bucket = if g > 30 {
                                    0
                                } else if g >= 13 {
                                    1
                                } else if g >= 7 {
                                    2
                                } else if g == 6 {
                                    3
                                } else if g == 5 {
                                    4
                                } else if g == 4 {
                                    5
                                } else if g == 3 {
                                    6
                                } else if g == 2 {
                                    7
                                } else if g == 1 {
                                    8
                                } else if g >= -4 {
                                    9
                                } else {
                                    10
                                };
                                sp.say_id(38 + bucket);
                            }
                        } else if n_cars >= 3 {
                            // kLapPosition 0x4A6390(place − 1) + kLapTimeBehind 0x4A63E0(s)
                            // (whole seconds to the leader, speed > 13.4 m/s)
                            sp.say_id(phrase::lap_position((place - 1).min(7) as u32, &mut draw));
                            if speed > 13.4 {
                                let sec =
                                    ((leader - r.progress) * lap_m / speed).floor() as i32;
                                if let Some(id) = phrase::lap_time_behind(sec, &mut draw) {
                                    sp.say_id(id);
                                }
                            }
                        }
                        // kBestLap 0x4A6580: this lap beat the session best (not on the first lap)
                        if r.lap_times.len() >= 2
                            && let Some(last) = r.lap_times.last()
                            && r.lap_times[..r.lap_times.len() - 1]
                                .iter()
                                .all(|t| last < t)
                        {
                            sp.say_id(phrase::best_lap(&mut draw));
                        }
                        // kFinalLap 0x4A65C0
                        if r.laps + 1 == self.num_laps {
                            sp.say_id(phrase::final_lap(&mut draw));
                        }
                    }
                }
            }
            // the automatic replay re-runs the finish: what the live race did once at the line
            // (the jingle, the finale's speech, the track record) is not done again — the
            // playback skips the music altogether (0x4A8A4A) and the record went in once
            let playback = self.replay.stream.as_ref().is_some_and(|s| matches!(s.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
            // the jingle is `HCongrats`'s, at the replay's start (`App::open_result_pages`); the
            // track record goes in as the front end files the race (`App::end_finished_race`)
            if r.finished && !was_finished {
                tracing::info!(place, n_cars, playback, tick = self.tick, "race finished");
            }
            // Quick Knockout (RACE_TYPE 3, modes.md §4): when the car that just completed lap L sits in
            // position cut = NUM_LAPS − L + 2 it and every car behind it are out
            if self.quick_knockout && lap_done && !r.finished {
                let lap = r.laps;
                let cut = self.num_laps as i64 - lap as i64 + 2;
                if place as i64 == cut {
                    self.player_eliminated = true;
                    r.finished = true;
                    r.progress -= 0.01;
                    r.tick = self.tick;
                    for o in self
                        .road.opponents
                        .iter_mut()
                        .filter(|o| o.progress.progress < r.progress + 0.01)
                    {
                        if !o.progress.finished {
                            o.eliminated = true;
                            o.progress.finished = true;
                            o.progress.progress -= 0.01;
                            o.progress.tick = self.tick;
                            // it keeps its AI: `car+0xB50 = 3` pulls it over (0x40C60C)
                        }
                    }
                    if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
                        sp.say(a, "KO-eliminated.bnk");
                    }
                    tracing::info!(lap, place, "player knocked out");
                }
            }
        }
    }

    /// **The camera key and the end of the 32 Hz input step** (see [`Self::simulate`]): `I_CYCLECAMERA` steps the car's own camera list (not in a replay, not while the opening animation runs), then the input's tick ends. Moved out of `simulate` unchanged.
    fn tick_camera_key(&mut self, ctx: &TickCtx) {
        let replay_camera = ctx.replay_camera;
        // taken once: `take_pressed(..) && replay_camera` consumed the press before testing the
        // replay, so outside a replay the `else if` below never saw it (since f5d06f8c)
        let camera_next = self.input.take_pressed(Action::CameraNext);
        if camera_next && replay_camera {
            // In a replay `RaceFrame` skips bit 0x17 (`[0x5E99F4] >= 2`, 0x415ACA): the key does
            // nothing, and the replay's nine ids step only by command 8, the bar's cam cell
            // (`replay_bar_apply_press`). *(Corrected 2026-09-26: the port stepped them with this key,
            // "which key is UNKNOWN (§15)"; no key reaches the replay's commands at all — they
            // are the HUD dialogs' answers, 0x425EF0.)*
        } else if camera_next && self.anim_cam != AnimCamSlot::Off {
            // 0x465A60 returns at once while the slot is on `AnimCam` (`cmp ax, 0xE; je
            // 0x465CE2`, 0x465ABF): the opening animation cannot be cycled away
        } else if camera_next {
            // `I_CYCLECAMERA`: step through the car's own camera list (`car[0].camera`)
            let n = self.camera_cycle.len().max(1) as u8;
            let driver = nfs_game::camera::VIEW_DRIVER as u16;
            let has_driver = self.driver_cam.is_some();
            // 0x465A60 re-runs the slot's reset (settle count 0x32, 0x465BB1; the offset seed 0x465B1A when the old
            // view is outside 2…7) whatever view the list lands on
            self.slot_key_enters = self.slot_key_enters.wrapping_add(1);
            self.camera_mode = (self.camera_mode + 1) % n;
            if !has_driver && self.camera_cycle.get(self.camera_mode as usize) == Some(&driver)
            {
                self.camera_mode = (self.camera_mode + 1) % n;
            }
        }
        self.input.end_tick();
    }

    /// **The odd tick's audio update** (`0x4A8120`, see the comment above its call in [`Self::simulate`]):
    /// the speech service, the thunder, the audio slots and the cars in them, the loop producers, the
    /// ambience, the car effects, the music, the tunnel reverb, the player's engine and SFX. Moved out
    /// of `simulate` unchanged; `view` is the camera view id and `exch_humans_now` the exchange
    /// session's human cars in registration order, both taken by `simulate` before the car was borrowed.
    fn simulate_audio(&mut self, view: u16, exch_humans_now: Vec<HumanCar>) {
        let Some(v) = self.vehicle.as_ref() else { return };
        let Some(w) = self.world.as_ref() else { return };
        // the speech service 0x4B0A20 is the audio update's first call (0x4A813A) — on the odd
        // tick, after race over (0x4150D9) has asked whether the announcer is busy. *(Until
        // 2026-09-26 the port ran it on every tick.)* A playback at double speed says nothing
        // (0x4B0915: `[0x5E99F4] >= 2` and `[0x606864] == 3`).
        let double = self.replay.speed == nfs_game::replay::Speed::Double
            && self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        if let (Some(a), Some(sp)) = (&self.snd.audio, &self.snd.speech) {
            sp.set_level(self.snd.audio_settings.speech_level);
            sp.update(a, double);
        }
        // the thunder's countdown 0x4A8380 is the update's second call (0x4A813F): a strike's call
        // 0x4A8340 (the sky's `ThunderCall`, drawn in the strike) sets it going, and at the end of
        // it the patch plays on the ambience's `track.viv` banks
        if let Some(call) = self.sky.as_mut().and_then(|s| s.take_thunder())
            && let Some(amb) = self.snd.ambience.as_mut()
        {
            amb.thunder.call(call.lead, call.pan, call.draw);
        }
        if let (Some(a), Some(amb)) = (&self.snd.audio, self.snd.ambience.as_mut()) {
            amb.thunder_update(a);
        }
        // `Voice_play`'s strip filter (0x4A9FA8…0x4AA03B, `nfs_audio::mixer::voice_play_filter`):
        // 3000 Hz on every updated voice of the SFX / AI / ambience banks in the cockpit —
        // `[0x627F43] & 8` (view ≤ 1), `0x443EE0` not exterior (this port's `interior`, the driver
        // view, MEDIUM as in `nfs_game::camera::audio_mode`) and the view word not 0 / 0xF —
        // open (0xFFFF) elsewhere, nothing at `AUDIOQUALITY` ≥ 2
        {
            let cockpit = view <= 1 && view == nfs_game::camera::VIEW_DRIVER as u16 && !matches!(view, 0 | 0xF);
            let filter = nfs_audio::mixer::voice_play_filter(false, self.snd.audio_settings.audio_quality, cockpit);
            if let Some(s) = self.snd.sfx.as_mut() {
                s.filter = filter;
            }
            if let Some(ai) = self.snd.ai_engines.as_mut() {
                ai.filter = filter;
            }
            if let Some(amb) = self.snd.ambience.as_mut() {
                amb.filter = filter;
            }
        }
        let mirror = self.mirror;
        // **The audio car slots** (0x4A7DC0, `nfs_audio::slots`): only the nearest
        // `{4, 2, 1}[AUDIOQUALITY]` cars inside 300 m of the listener are heard — their engines,
        // horns and sirens all run on the slot's voices (engine `slot + 6`, 0x4A7080) — and a car
        // keeps its slot while it stays among them. The port had given every opponent and every
        // traffic car its own engine voices, and each cop a siren by its index in the police list.
        // The walk is the car list's own order (`0x5E8B34`: the human, the opponents, the cops,
        // then the traffic — `Race_setCars` 0x4ECF00, `modes.md` §1), which only breaks exact ties
        // *(until 2026-09-26 the port walked the traffic before the cops)*
        let cam = self.camera.position;
        let count = nfs_audio::slots::slot_count(self.snd.audio_settings.audio_quality);
        let mut cars: Vec<(AudioCar, f32)> = Vec::new();
        for (i, o) in self.road.opponents.iter().enumerate() {
            cars.push((AudioCar::Opponent(i), (o.vehicle.position - cam).length()));
        }
        if let Some(pol) = &self.road.police {
            for (i, c) in pol.cars.iter().enumerate().filter(|(_, c)| c.active) {
                cars.push((AudioCar::Police(i), (c.position - cam).length()));
            }
        }
        if let Some(tr) = &self.road.traffic {
            for (i, c) in tr.cars.iter().enumerate().filter(|(_, c)| c.active) {
                cars.push((AudioCar::Traffic(i), (c.position - cam).length()));
            }
        }
        // who held which slot through the last update: the even tick's `Honk` / `HonkPattern` calls
        // (0x4013D0) found their cars in that table, before 0x4A7DC0 re-deals it below
        let held_before: Vec<(usize, AudioCar)> = self.snd.slots.held().collect();
        let released = self.snd.slots.update(count, cars.iter().copied());
        if let Some(a) = &self.snd.audio {
            for (slot, _) in &released {
                if let Some(ai) = self.snd.ai_engines.as_mut() {
                    // voices stopped and the honk timer zeroed (0x4A801B / 0x4A80E7)
                    ai.release(a, *slot);
                }
                if let Some(s) = self.snd.sfx.as_mut() {
                    s.siren(a, *slot, false, 0.0, 0, nfs_audio::ai_engine::PITCH_BASE_STILL, 0);
                    // 0x4A7FF8 / 0x4A80BD: the slot's loop voice `slot + 0x22` too
                    s.stop_ai_loop(a, *slot);
                }
            }
        }
        // the crowding factor over the slot table's cars (0x4A8199)
        let held: Vec<(usize, AudioCar)> = self.snd.slots.held().collect();
        let crowding = crowding_factor(held.iter().filter_map(|(_, k)| cars.iter().find(|c| c.0 == *k).map(|c| c.1)));
        self.snd.crowding = crowding;
        // 0x4A8236…0x4A823D: the slot cars' pass 0x4A7080 — engines, horns, sirens — runs only
        // while AmbientLevel `[0x5D1750] > 0`; at 0 none of them is started or updated (the slot
        // table itself, 0x4A7DC0 above, is kept regardless)
        let slot_cars_on = self.snd.audio_settings.ambient_level > 0;
        if slot_cars_on && let (Some(a), Some(ai)) = (&self.snd.audio, self.snd.ai_engines.as_mut()) {
            // `MIRROR` negates the pan angle before the atan2 that produces it (0x4A704D, 0x4A78C1
            // and the other four sites of `modes.md` §2.1), so flip the right vector with the world
            let pan_of = listener_pan_of(cam, self.camera.target, mirror);
            let st = &self.snd.audio_settings;
            let ai_levels = nfs_audio::ai_engine::Levels { sfx: st.sfx_level, engine: st.engine_level, ambient: st.ambient_level };
            for &(slot, key) in &held {
                match key {
                    // AI engine loops (`engine-audio.md` §5): two crossfaded samples plus a road loop
                    AudioCar::Opponent(i) => {
                        let Some(o) = self.road.opponents.get(i) else { continue };
                        let set = self.road.opponent_sets.get(i).cloned().unwrap_or_default();
                        if set.is_empty() {
                            continue;
                        }
                        let rel = o.vehicle.position - cam;
                        let d = rel.length();
                        let (cos, pan) = if d > 0.1 {
                            ((rel / d).dot(o.vehicle.forward()), pan_of(rel))
                        } else {
                            (0.0, pan_of(rel))
                        };
                        let frac = nfs_audio::ai_engine::ai_rpm_frac(o.vehicle.rpm, o.specs.redline);
                        ai.update(
                            a,
                            slot,
                            &set,
                            frac,
                            d,
                            cos,
                            pan,
                            nfs_audio::ai_engine::ai_speed_16(o.vehicle.v_long()),
                            nfs_audio::ai_engine::PITCH_BASE_STILL,
                            self.snd.audio_settings.audio_quality,
                            crowding,
                            view == 0x0B,
                            ai_levels,
                        );
                        // **A remote human's horn**. The cars in the audio slots are every car but the
                        // listener's own (0x4A7DC0, `0x5E8B34[]` less `[0x655F50]`), so another player's car is
                        // one of them and is voiced by 0x4A7080 like the rest; its `car+0x52C & 4` clear-arm
                        // (the honk timers of 0x4A750A…0x4A75BF, "every car but a human") is skipped for a human
                        // car, whose horn byte `car+0xD86` is read straight (0x4A7560…0x4A75AB): set → the same
                        // `Voice_play(slot + 0x27, 0xA + …)` as the AI honk, at the siren's distance law, every
                        // update; clear with the slot's flag `+0x655D80` up → the voice stops. `car+0xD86` is the
                        // block's `+0x3B` bit 1 (0x4992BF), the local pad's horn before the first block
                        // (0x499330), 0 for the AI arm (0x499279) — or, in an input-exchange race, the latched
                        // sample's bit 1 (`b0`, 0x48EC1C).
                        if let Some(n) = self.net.session.as_ref() {
                            let horn = if n.exchanging() {
                                n.seat_race_index(i).is_some_and(|r| n.latched(r as usize)[0] & 2 != 0)
                            } else {
                                n.owned_seats().contains(&i) && n.seat_arm(i).horn(self.net_pad_buttons & 2 != 0)
                            };
                            if horn && self.tick % 32 == 1 {
                                tracing::debug!(target: "netplay", seat = i, slot, tick = self.tick, distance = d, "a remote human's horn is voiced (0x4A7080's human arm)");
                            }
                            ai.honk(a, slot, horn, d, pan, nfs_audio::ai_engine::PITCH_BASE_STILL, self.snd.audio_settings.ambient_level);
                        }
                    }
                    // traffic uses the same cheap path. Its engine speed is real: `0x408360`
                    // computes `car+0xDB4 = carSpecs[0x60 + 4·gear] · car+0x35C` at 0x4084AD with
                    // the gear from `Traffic_pickGear` 0x408130 (`nfs_audio::ai_engine::ai_gear`):
                    // 1 under 2 m/s, 0 while reversing out of trouble (`car+0x104C == −1`), else
                    // the gear the speed calls for. *(Until 2026-09-26 this took `car+0x104C == −1`
                    // for the idle state — it is +1 from every spawn — so a moving traffic car or
                    // cop sat in the reverse slot, whose negative ratio pinned its loops at the
                    // bottom of their pitch; and it multiplied `car+0xEC0` for `car+0x35C`.)*
                    // The road loop's speed is `|car+0xD60|` = `|v · car+0x37C|` (0x4A7392…0x4A73B3,
                    // 0x408F20), not `car+0xEC0`.
                    AudioCar::Traffic(i) => {
                        let (Some(tr), Some(specs)) = (&self.road.traffic, &self.road.traffic_specs) else { continue };
                        let specs = tr.car_specs.get(i).unwrap_or(specs);
                        let Some(c) = tr.cars.get(i) else { continue };
                        let set = self.road.traffic_sets.get(i).cloned().unwrap_or_default();
                        if set.is_empty() {
                            continue;
                        }
                        let rel = c.position - cam;
                        let d = rel.length();
                        let (cos, pan) = if d > 0.1 {
                            ((rel / d).dot(c.orientation.row(2) * c.dir), pan_of(rel))
                        } else {
                            (0.0, pan_of(rel))
                        };
                        let v = c.speed_estimate();
                        let gear = nfs_audio::ai_engine::ai_gear(v, c.gear.sense < 0.0, &specs.mps_per_rpm, specs.gear_slots, specs.redline);
                        let rpm = specs.ratio_slot(gear) * v;
                        let frac = nfs_audio::ai_engine::ai_rpm_frac(rpm, specs.redline);
                        ai.update(
                            a,
                            slot,
                            &set,
                            frac,
                            d,
                            cos,
                            pan,
                            nfs_audio::ai_engine::ai_speed_16(c.velocity.dot(c.orientation.row(2))),
                            nfs_audio::ai_engine::PITCH_BASE_STILL,
                            self.snd.audio_settings.audio_quality,
                            crowding,
                            view == 0x0B,
                            ai_levels,
                        );
                        // The horn (`traffic.md` §12.3, `nfs_audio::ai_engine::HonkTimer`): the even
                        // tick's `0x4013D0` programs the slot's honk timers — HONK_HORN (mask 8) ⇒
                        // `Honk(car, 1)` 0x4A6D90, else with the horn byte `car+0xD86` set ⇒
                        // `Honk(car, 0)`; CRAZY_HORN (mask 0x1000) ⇒ `HonkPattern` 0x4A6DE0 with the
                        // pattern of `(gGameTick >> 3) & 3` — and 0x4A750A…0x4A75BF steps them here.
                        // Both actions are muted in the cops race: `[0x6573EC] != 1` gates HONK_HORN
                        // (0x4013EC) and CRAZY_HORN (0x401421). Traffic runs personality row 0,
                        // whose collision script is `HONK_HORN(2) END HONK_HORN(2) END` — so a car
                        // the player runs into leans on the horn. A car that got its slot in this
                        // update had none when the calls were made (they find no slot and do
                        // nothing). *(Until 2026-09-26 CRAZY_HORN sounded as a plain held honk.)*
                        // After the triggers, the oncoming car's random `HonkPattern(car, 2, 0x20, 8)`
                        // (0x401237; `TrafficCar::honk_tick` = the even tick it was drawn on).
                        let t = ai.honk_timer(slot);
                        if held_before.contains(&(slot, key)) {
                            let cops = self.road.cops_race;
                            if !cops && c.script.query(nfs_formats::scripts::mask::HONK_HORN).is_some() {
                                t.honk(true);
                            } else if t.horn {
                                t.honk(false);
                            }
                            if !cops && c.script.query(nfs_formats::scripts::mask::CRAZY_HORN).is_some() {
                                // the even tick's gGameTick, the one before this odd tick
                                t.pattern(nfs_audio::ai_engine::CRAZY_HORN_PATTERNS[((self.tick - 1) >> 3) as usize & 3]);
                            }
                            if c.honk_tick == Some(self.tick - 1) {
                                t.pattern(nfs_audio::ai_engine::ONCOMING_HONK_PATTERN);
                            }
                        }
                        let step = t.step();
                        if step != nfs_audio::ai_engine::HonkStep::Idle {
                            tracing::debug!(slot, car = i, ?step, timer = t.timer, "traffic honk timer");
                        }
                        ai.honk(a, slot, step == nfs_audio::ai_engine::HonkStep::Play, d, pan, nfs_audio::ai_engine::PITCH_BASE_STILL, self.snd.audio_settings.ambient_level);
                    }
                    // a cop is an AI car like the traffic (`AIHigh_Cop`): 0x4A7080 treats every slot
                    // car alike (bank = its catalogue sound set, voice `slot + 6`), and its engine
                    // speed is 0x408360's `carSpecs[0x60 + 4·gear] · car+0x35C` with 0x408130's gear
                    // (`ai_gear`, as the traffic above), off its own `.sim`
                    AudioCar::Police(i) => {
                        let Some(c) = self.road.police.as_ref().and_then(|p| p.cars.get(i)) else { continue };
                        let (Some(set), Some(specs)) = (self.road.police_sets.get(i), self.road.police_specs.get(i)) else { continue };
                        if set.is_empty() {
                            continue;
                        }
                        let rel = c.position - cam;
                        let d = rel.length();
                        let (cos, pan) = if d > 0.1 {
                            ((rel / d).dot(c.orientation.row(2) * c.dir), pan_of(rel))
                        } else {
                            (0.0, pan_of(rel))
                        };
                        let v = c.speed_estimate();
                        let gear = nfs_audio::ai_engine::ai_gear(v, c.gear.sense < 0.0, &specs.mps_per_rpm, specs.gear_slots, specs.redline);
                        let rpm = specs.ratio_slot(gear) * v;
                        let frac = nfs_audio::ai_engine::ai_rpm_frac(rpm, specs.redline);
                        tracing::debug!(slot, cop = i, gear, rpm, frac, "cop engine");
                        ai.update(
                            a,
                            slot,
                            set,
                            frac,
                            d,
                            cos,
                            pan,
                            nfs_audio::ai_engine::ai_speed_16(c.velocity.dot(c.orientation.row(2))),
                            nfs_audio::ai_engine::PITCH_BASE_STILL,
                            self.snd.audio_settings.audio_quality,
                            crowding,
                            view == 0x0B,
                            ai_levels,
                        );
                    }
                }
            }
        }
        // the police sirens (`sfx.md` §6): a cop with its lights on sounds SND_coll 6 / 8 by its
        // audio slot's parity, full on top of the listener and silent from 70 m
        if slot_cars_on && let (Some(a), Some(s), Some(pol)) = (&self.snd.audio, self.snd.sfx.as_mut(), self.road.police.as_ref()) {
            let pan_of = listener_pan_of(cam, self.camera.target, mirror);
            for &(slot, key) in &held {
                let AudioCar::Police(i) = key else { continue };
                let Some(c) = pol.cars.get(i) else { continue };
                let rel = c.position - cam;
                let d = rel.length();
                let pan = pan_of(rel);
                s.siren(a, slot, c.active && c.lights, d, pan, nfs_audio::ai_engine::PITCH_BASE_STILL, self.snd.audio_settings.ambient_level);
            }
        }
        // Every non-player car's loop producer `Car_bodyScrapeSfx` 0x40FFF0 → `Car_squealSfx`
        // 0x40FBC0 (from `Car_postImpact` 0x410280, every full car), then the drain 0x4A73DE of
        // each slot car's queue inside 0x4A7080 (`CollisionSfx::drain_ai`), then 0x498220's clear of
        // every car's count. The squeal needs the wheel slips only `Car_simulate` 0x49FD30 writes —
        // an opponent's; a traffic car or a cop runs 0x408360 instead (`traffic.md` §21.5), so its
        // are 0 and it only ever produces the roof scrape, whose draw is the game's LCG
        // `[0x5D1028] · [0x5D102C]` (0x4100F8…0x41011E) — `nfs_game::rng::rand16`, the stream the
        // AI draws from, as the player's. *(Until 2026-09-26 both took the collision bank's own
        // generator, an approximation.)*
        // not gated on the sound device: the draws belong to the game stream
        {
            // 0x40FBC8…0x40FC0D: `[0x606AE0]` (the track is `autobahn`, 0x414DFF…0x414E11) and
            // nothing overhead (0x473330 at `car+0x330`, the lowest patch more than 3.5 m up)
            let autobahn = self.track.name.eq_ignore_ascii_case("autobahn");
            let wet = |p: Vec3| autobahn && w.overhead_flags(p) == 0;
            let q = &mut self.snd.ai_sfx;
            // **Input exchange:** every human car's producer runs in registration order — the local car
            // among the seats, where the port runs it after them (below) — so that the draws on the shared
            // stream fall to the same car on every machine (`Car_postImpact` 0x410280 walks the cars in
            // slot order, `network.md` §11.18)
            let exch_humans = exch_humans_now.clone();
            let racer_order: Vec<HumanCar> = if exch_humans.is_empty() {
                (0..self.road.opponents.len()).map(HumanCar::Seat).collect()
            } else {
                exch_humans
            };
            for h in racer_order {
                let HumanCar::Seat(i) = h else {
                    let input = nfs_audio::sfx::LoopInput {
                        cos_tilt: v.cos_tilt,
                        body_height: v.body_height,
                        speed_est: v.horizontal_speed(),
                        front_slip: v.wheels[0].slip + v.wheels[1].slip,
                        rear_slip: v.wheels[2].slip + v.wheels[3].slip,
                        racer_distance: 0.0,
                        tyre_class: self.specs.class.min(2),
                        surface_nibble: v.surface_nibble(),
                        wet: wet(v.position),
                        human: true,
                    };
                    self.snd.player_loops.produce_loops(&input, nfs_game::rng::rand16);
                    continue;
                };
                let o = &self.road.opponents[i];
                let b = &o.vehicle;
                let input = nfs_audio::sfx::LoopInput {
                    cos_tilt: b.cos_tilt,
                    body_height: b.body_height,
                    speed_est: b.horizontal_speed(),
                    front_slip: b.wheels[0].slip + b.wheels[1].slip,
                    rear_slip: b.wheels[2].slip + b.wheels[3].slip,
                    // 0x417190: `car+0x78 = 0` for a racer (`car+0x52C & 1`, 0x4171A5)
                    racer_distance: 0.0,
                    tyre_class: o.specs.class.min(2),
                    surface_nibble: b.surface_nibble(),
                    wet: wet(b.position),
                    human: false,
                };
                q.entry(AudioCar::Opponent(i)).or_default().produce_loops(&input, nfs_game::rng::rand16);
            }
            if let Some(pol) = &self.road.police {
                for (i, c) in pol.cars.iter().enumerate().filter(|(_, c)| c.active) {
                    let b = &c.body;
                    let input = nfs_audio::sfx::LoopInput {
                        cos_tilt: b.cos_tilt,
                        body_height: b.body_height,
                        speed_est: c.speed_estimate(),
                        front_slip: 0.0,
                        rear_slip: 0.0,
                        racer_distance: c.racer_distance,
                        tyre_class: 0,
                        surface_nibble: b.surface_nibble(),
                        wet: wet(b.position),
                        human: false,
                    };
                    q.entry(AudioCar::Police(i)).or_default().produce_loops(&input, nfs_game::rng::rand16);
                }
            }
            if let Some(tr) = &self.road.traffic {
                for (i, (c, b)) in tr.cars.iter().zip(tr.bodies.iter()).enumerate().filter(|(_, (c, _))| c.active) {
                    let input = nfs_audio::sfx::LoopInput {
                        cos_tilt: b.cos_tilt,
                        body_height: b.body_height,
                        speed_est: c.speed_estimate(),
                        front_slip: 0.0,
                        rear_slip: 0.0,
                        racer_distance: c.racer_distance,
                        tyre_class: 0,
                        surface_nibble: b.surface_nibble(),
                        wet: wet(b.position),
                        human: false,
                    };
                    q.entry(AudioCar::Traffic(i)).or_default().produce_loops(&input, nfs_game::rng::rand16);
                }
            }
        }
        if slot_cars_on && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
            let (target, view_0c) = (self.camera.target, view == 0x0C);
            for &(slot, key) in &held {
                let pos = match key {
                    AudioCar::Opponent(i) => self.road.opponents.get(i).map(|o| o.vehicle.position),
                    AudioCar::Traffic(i) => self.road.traffic.as_ref().and_then(|tr| tr.cars.get(i)).map(|c| c.position),
                    AudioCar::Police(i) => self.road.police.as_ref().and_then(|p| p.cars.get(i)).map(|c| c.position),
                };
                let (Some(pos), Some(q)) = (pos, self.snd.ai_sfx.get(&key)) else { continue };
                if !q.entries().is_empty() {
                    tracing::debug!(slot, car = ?key, entries = q.entries().len(), "ai sfx queue");
                }
                s.drain_ai(a, slot, q, (pos - cam).length(), view_0c, listener_pan(cam, target, mirror, pos));
            }
        }
        for q in self.snd.ai_sfx.values_mut() {
            q.clear();
        }
        // track ambience: a quarter of the elements per update (0x4AEF30)
        if let (Some(a), Some(amb)) = (&self.snd.audio, self.snd.ambience.as_mut()) {
            let cam = self.camera.position;
            let fwd = (self.camera.target - cam).normalize_or_zero();
            let side = Vec3::Y.cross(fwd).normalize_or_zero() * if mirror { -1.0 } else { 1.0 };
            // panBase: the angle of the player car's forward `car+0x3D0` on the same axes
            // (0x4A7A4E…0x4A7AAC)
            let pan_base = nfs_audio::mixer::pan_angle(v.forward().dot(side), v.forward().dot(fwd));
            amb.update(a, self.tick, cam.into(), side.into(), fwd.into(), pan_base);
            // The autobahn train's ambient emitter (`engine-audio.md` §7.4's second, unconditional
            // loop — not phased like the `.scn` elements above): `sound.ambience_index` is only
            // `Some` on the one track that has it. `p1`/`p2` come from the object's own path
            // (`AnimatedSound::eval_pair`, `0x4AEF30`'s `EvalPos(tick)`/`EvalPos(tick+0x20)`).
            if let Some(sound) = self.world.as_ref().and_then(|w| w.animated_sound.as_ref())
                && let Some(idx) = sound.ambience_index
            {
                let (p1, p2) = sound.eval_pair(self.tick);
                amb.sync_animated(idx, p1, p2);
            }
            amb.update_animated(a, self.tick, cam.into(), side.into(), fwd.into(), pan_base);
        }
        // **0x443640** (`GameTick` 0x4150ED, the odd tick, right before the update below): the
        // walk of every `R3DObject` on `[0x6115F0]` calling its **slot 8** — for an `R3DCar`,
        // 0x438420: `0x43EF80([this+0x704], 1)` unless the front end is up (`[0x65B928]`), with
        // neither the draw nor slot 7's camera gate (0x4383D8…0x43840C) in front of it. So every
        // car's effects are driven at 32 Hz whether it is drawn or not, the camera's own car in
        // the bumper and driver views too; the draw's drive (slot 7, `draw_cars`) comes on top.
        // *(Read 2026-09-30 (DDD); until then the port drove a car's effects only when it drew
        // it.)* The list's order (traffic, cops, opponents, each last slot first, the player
        // last — `particles.md` "The order between cars") is kept, though the drive consumes no
        // random numbers and so no order shows. MEDIUM: the traffic the port simulates are its
        // active cars; an inactive slot's `R3DCar` is driven too in the original, from a sim car
        // the port does not keep (0x43EF80 lights nothing at zero slip and zero speed).
        // the drive's `wet`, the sky's field 5 at the eye's slice (`weather_strengths`, spelled
        // out: `v` holds `self` here)
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        let wet = self.track.sky_bin.as_ref().map_or(0.0, |b| b.sample_track_b(5, self.cam_slice.unwrap_or(0) as i32, lap)) > 0.1;
        let eye = self.camera.position;
        if let Some(fx) = self.car_effects.as_mut() {
            let n_opp = self.road.opponents.len();
            let n_pol = self.road.police.as_ref().map_or(0, |p| p.cars.len());
            use nfs_game::car_effects::CarInput;
            if let Some(tr) = &self.road.traffic {
                for (i, b) in tr.cars.iter().zip(tr.bodies.iter()).enumerate().rev().filter(|(_, (c, _))| c.active).map(|(i, (_, b))| (i, b)) {
                    fx.drive(1 + n_opp + n_pol + i, &CarInput::from_vehicle(b), eye, wet, true);
                }
            }
            if let Some(pol) = &self.road.police {
                for (i, c) in pol.cars.iter().enumerate().rev() {
                    fx.drive(1 + n_opp + i, &CarInput::from_vehicle(&c.body), eye, wet, true);
                }
            }
            for (i, o) in self.road.opponents.iter().enumerate().rev() {
                fx.drive(1 + i, &CarInput::from_vehicle(&o.vehicle), eye, wet, true);
            }
            fx.drive(0, &CarInput::from_vehicle(v), eye, wet, true);
        }
        // the cars' effects, 0x43F660 (`GameTick` 0x4150F2, the odd tick, before the trigger
        // sweep): each car driven since the last one runs its eleven systems once, and the draw
        // set is emptied (`nfs_game::car_effects`)
        if let (Some(fx), Some(p)) = (self.car_effects.as_mut(), self.track_particles.as_mut()) {
            fx.update(p.rng_mut(), self.draw_distance);
        }
        // the trigger sweep 0x470BB0 (`GameTick` 0x4150F7, the odd tick, after the audio update):
        // each type-4 trigger is tested against the first car of `[0x5E8B34]`, the human — `hit`
        // 0x4706C0, the heading 0x470AD0, the direction of travel 0x4709C0 and the speed window
        // 0x470B60, `CTriggerElem::test`'s parts — and on a pass (every tick: `+0x9C`, the
        // fire-once flag, is 0 in every shipped trigger, 0x470D2E…0x470D3D; `+0xE0`'s chime is
        // for types 0/1 only) its `+0xD8` element gets `0x4AC0E0(1)` when it is a Type-1
        // `CAudioElem` with a PatchID (0x470D5F…0x470DB3) — the ambience plays it on its next visit
        if !self.snd.audio_triggers.is_empty() {
            let probe = nfs_game::skill::TriggerCar {
                position: v.position,
                wheels: std::array::from_fn(|i| v.wheels[i].world_pos - Vec3::Y * v.ride_height),
                forward: v.forward(),
                velocity: v.velocity,
                speed: v.horizontal_speed(),
            };
            for t in self.snd.audio_triggers.iter().filter(|t| t.test(&probe)) {
                let flagged = self.snd.ambience.as_mut().map(|amb| amb.fire(t.link));
                tracing::debug!(element = t.link, ?flagged, "type-4 audio trigger fired (0x470BB0 -> 0x4AC0E0)");
            }
        }
        // end-of-stream advance through the playlist (0x4ADD6D)
        if let (Some(a), Some(m)) = (&self.snd.audio, self.snd.music.as_mut()) {
            // the shuffled advance's `0x538040() + GetTickCount()` (0x4ADDA0…0x4ADDA7): the shared
            // generator `fe_rng` and the wall clock's milliseconds — PORT-DEVIATION D20 for the time
            // since Windows started (as `I_MUSIC`'s pick); drawn only when a song ended with the
            // shuffle on
            let rng = &mut self.fe_rng;
            let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u32);
            m.update(a, &mut || rng.next_u32().wrapping_add(ms));
        }
        // tunnel reverb (`engine-audio.md` §4.7): the enclosure test every audio update, then the
        // level that snaps to 100 under a road deck and decays 100 → 68 → 36 → 8 on the way out
        if let Some(a) = &self.snd.audio {
            let in_tunnel = w.in_tunnel(v.position);
            // the two parameters are the buses `[0x6B4A0C]` / `[0x6B4A0D]` every voice's send is
            // taken from (0x565CD0 → 0x589D70)
            if let Some(p) = self.snd.reverb.update(in_tunnel, false, false) {
                a.set_env(p.p20, p.p80);
            }
        }
        if let (Some(a), Some(e)) = (&self.snd.audio, self.snd.engine_sound.as_mut()) {
            // view mode: the audio mode follows the view id the camera stands for
            // (`nfs_game::camera::audio_mode`, `engine-audio.md` §4.2) — 1 for the bumper view,
            // 2 for the chase views, 0 (the `.cct`/`.clt` cockpit layers) for the driver view and
            // 3 for the detached trackside camera.
            // `[0x627F43] & 8` is **`viewId <= 1`** (0x46596B…0x465986), not this port's earlier
            // "any view but 0x0C / 0x14": views 5…14 and above 15 are detached and take mode 3.
            let attached = view <= 1;
            let interior = view == nfs_game::camera::VIEW_DRIVER as u16;
            let mode = nfs_game::camera::audio_mode(view, attached, interior);
            let to_car = self.car_pos - self.camera.position;
            let dist = to_car.length();
            let cos = if dist > 0.1 {
                (to_car / dist).dot(v.forward())
            } else {
                0.0
            };
            // mirrored races negate the pan (see the AI engines above)
            let pan_of = listener_pan_of(self.camera.position, self.camera.target, self.mirror);
            let pan = pan_of(to_car);
            e.update(
                a,
                nfs_audio::engine::EngineInput {
                    // §4.3's last line: `[0x627F43] & 8` and `0x443EE0()` halve the engine gain
                    // `[ebp+0x13]`: stored only on the jump table's default arm (views 1, 5…14,
                    // > 15) when attached — view 1 — never on views 0 / 15 (0x4AA279). The port
                    // had it on view 0 until 2026-09-26.
                    attached_exterior: !matches!(view, 0 | 2..=4 | 15) && attached && !interior,
                    // `[0x5E99F4] >= 2`: a replay playing back (2) or run out (3)
                    view_0c_replay: view == 0x0C
                        && self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)),
                    // `car+0xDB4` as it is: 0x4A79F9 (the rpm level) and 0x4AA8A5 (rpm/redline) read
                    // it raw — the port's `max(600)` floor had no source and is gone (2026-09-26)
                    rpm: v.rpm,
                    automatic: v.automatic,
                    airborne_count: v.airborne_count,
                    redline: self.specs.redline,
                    throttle: v.throttle_raw,
                    brake: v.brake_raw,
                    reversing: v.gear == 0,
                    running: v.engine_alive != 0, // `car+0xDB0`: a heavy crash or a roll kills it until the reset
                    // `car+0x548`, the rev limiter's ignition-cut counter (`physics.md` §4.1) — the
                    // engine carries it, so the loud-load branch of `engine-audio.md` §3 gets the real
                    // thing rather than "at or past the redline"
                    rev_cut: v.ignition_cut > 0,
                    mode,
                    cos,
                    pan,
                    distance: dist,
                    // `.TINYCAR` doubles the pitch base it hands the engine queue (0x4AA8F9…0x4AA906),
                    // after the road and turbo layers have taken theirs
                    doppler: if self.cheat_bits & CHEAT_TINY != 0 { 2.0 } else { 1.0 },
                    detail: self.snd.audio_settings.audio_quality,
                    crowding: self.snd.crowding,
                    engine_level: self.snd.audio_settings.engine_level,
                    // §4.3's dip runs while the shift timer is up, `car+0xD82 > car+0xD81` (an
                    // upshift) **and `car+0xD81 != 1`** — out of neutral it does not dip at all
                    shift: (v.shift_timer > 0 && v.gear > self.snd.last_gear && v.prev_gear != 1)
                        .then_some((v.shift_timer, self.specs.shift_ticks)),
                },
            );
            // The click fires on the last tick of the shift, not at the gear change, and it lives
            // inside §4.3's gear-shift dip: the shift timer is running, `car+0xD82 > car+0xD81`
            // (an upshift) and **`car+0xD81 != 1`** — the previous gear was not neutral, so the
            // pull away off the grid is silent. Then at `car+0xD84 == 1` it needs
            // `[0x627F43] & 8` (viewId <= 1) **and** `viewId ∉ {0, 0xF}`, which together leave
            // exactly **view 1**. This port had neither gate.
            if v.shift_timer == 1 && v.gear > self.snd.last_gear && v.prev_gear != 1 && view == 1 {
                e.play_shift(a, self.snd.audio_settings.sfx_level);
            }
        }
        self.snd.last_gear = v.gear;
        if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
            s.update();
            // the queue's distance is the car's own (`car+0x330`, 0x4A78E8) and so is the loops'
            // pan (0x4A7862…0x4A78E5, `MIRROR` negating it at 0x4A78C1 as for the engine above)
            let to_car = self.car_pos - self.camera.position;
            let dist = to_car.length();
            let pan_of = listener_pan_of(self.camera.position, self.camera.target, self.mirror);
            // 0x4A7855: on view 0 the angle is not computed — `ebp` stays 0 (centre) for the engine,
            // the road layer and the loops (the one-shots' contact-point angle is separate)
            let pan = if view == 0 { 0 } else { pan_of(to_car) };
            s.set_levels(
                self.snd.audio_settings.sfx_level,
                self.snd.audio_settings.engine_level,
                self.snd.reverb.level(),
                dist,
                view == 0x0C,
                pan,
            );
            // the player's loop producers **0x40FFF0 → 0x40FBC0** (`Car_postImpact` 0x410280), the
            // opponents' own ([`SfxQueue::produce_loops`]): upright (`car+0x3A0 > 0.2`) the squeal
            // pair on the front / rear channels, gated on `car+0x424 < 0.1` and `car+0x3A0 > 0.5`;
            // on its side or roof the CarBody × surface scrape on the **rear** channel only, or its
            // stop — the front channel gets no entry, so a front squeal left playing is not
            // stopped there. The entries are walked last-first (0x4A7AF8…0x4A7C97) into 0x4A93C0
            // on voices 0x12 / 0x14. *(Until 2026-09-27 the port hard-stopped the front loop at
            // every scrape tick and released both channels whenever the car was not upright.)*
            // The scrape's draw `[0x5D1028]` is still made inside the audio update.
            let input = nfs_audio::sfx::LoopInput {
                cos_tilt: v.cos_tilt,
                body_height: v.body_height,
                speed_est: v.horizontal_speed(),
                front_slip: v.wheels[0].slip + v.wheels[1].slip,
                rear_slip: v.wheels[2].slip + v.wheels[3].slip,
                // 0x417190: `car+0x78 = 0` for a racer
                racer_distance: 0.0,
                tyre_class: self.specs.class.min(2),
                // `car+0x464` (0x40FC1B, 0x4100BB), the body's surface nibble
                surface_nibble: v.surface_nibble(),
                // `[0x606AE0]` && 0x473330 == 0 (0x40FBC8…0x40FC0D)
                wet: self.track.name.eq_ignore_ascii_case("autobahn") && w.overhead_flags(v.position) == 0,
                human: true,
            };
            // (an exchange race produced it above, in registration order)
            if !self.net.session.as_ref().is_some_and(|n| n.exchanging()) {
                self.snd.player_loops.produce_loops(&input, nfs_game::rng::rand16);
            }
            // the queue's one-shots — pushed during the tick's physics, above — take the angle to
            // `car+0x440` as it stands now, read ONCE at this drain (0x4A7BB1…0x4A7C3B), after the
            // whole tick's collisions: every one-shot of the tick takes the same, final contact,
            // not each one's own moment (`CollisionSfx::drain_player`; approximation withdrawn
            // 2026-09-27, `sfx.md` "The drain's distance and pan")
            let one_shot_pan = listener_pan(self.camera.position, self.camera.target, self.mirror, v.last_contact);
            s.drain_player(a, &self.snd.player_loops, one_shot_pan);
            self.snd.player_loops.clear();
            // road roll (§4.4), wind (§4.5) and the turbo layer (§4.6) of 0x4AA050, with the view
            // mode the engine took (`audio_mode`: `[0x627F43] & 8` is `viewId <= 1`) and the loops'
            // pan above (0 on view 0, negated under `MIRROR`) — the port had its own un-mirrored
            // pan here and "not 0x0C / 0x14" for the turbo's attached test until 2026-09-26
            let attached = view <= 1;
            let interior = view == nfs_game::camera::VIEW_DRIVER as u16;
            let mode = nfs_game::camera::audio_mode(view, attached, interior);
            let exterior = !matches!(view, 0 | 2..=4 | 15) && attached && !interior;
            if let Some(e) = &self.snd.engine_sound {
                // `A`, or `12800/(d+1)` on view 0x0C (0x4AA3B2…0x4AA3E1)
                let road_a = if view == 0x0C { nfs_audio::fix_atten(0x64_0000, 0x1_0000, 7, dist) } else { e.road_gain() };
                // `car+0x464`, the body's surface nibble (0x4AA0CE, 0x4AA246…0x4AA253), not a
                // wheel's; `V >> 1` in modes 2 and 3 (0x4AA2C8)
                s.road_roll(a, v.v_long(), v.surface_nibble(), road_a, mode >= 2, nfs_audio::ai_engine::PITCH_BASE_STILL, pan);
            }
            // §4.5: `[ebp+0x13]` and `(int)(|car+0x35C| · 5) != 0` (0x4AA4D8…0x4AA4E5); the
            // walk's draws are 0x538040's (the shared `fe_rng`), at this point of the audio update
            let rng = &mut self.fe_rng;
            s.wind(a, exterior, v.horizontal_speed(), v.v_long(), v.surface_nibble(), &mut || rng.next_u32());
            // §4.6: the boost state the physics now keeps (`car+0x54C`), at the aux base level of
            // `Engine_auxVolume(mode)` scaled by the engine gain before the exterior halving
            if let Some(e) = &self.snd.engine_sound {
                s.turbo(
                    a,
                    v.boost,
                    v.throttle_raw,
                    v.rpm,
                    self.specs.redline,
                    e.aux_volume(mode),
                    e.turbo_gain(),
                    nfs_audio::ai_engine::PITCH_BASE_STILL,
                    pan,
                );
            }
        }
    }
}

/// What the sections of one tick's 32 Hz block (`App::simulate`) read and do not change: the cheat flags of
/// the launch, the input exchange's samples and the registration order of the human cars, all taken before
/// the car is borrowed.
struct TickCtx {
    rally_on: bool,
    heavy_on: bool,
    derby_on: bool,
    /// a replay is playing back or has run out (`[0x5E99F4] >= 2`)
    ff_replay: bool,
    /// the camera view id, and whether the camera key steps the replay's own nine ids
    view: u16,
    replay_camera: bool,
    /// the seats `net_step` found driven by a remote machine (a positional race)
    net_seats: Vec<usize>,
    exchange_in: Option<ExchangeIn>,
    exch_order: Vec<HumanCar>,
    n_seat_cars: usize,
    pos_order: Vec<HumanCar>,
    pos_seat_horns: Vec<bool>,
}

impl TickCtx {
    /// the registration index of a car by the port's storage index (0 = the local car, `1 + k` = seat
    /// `k`, the rest — cops, traffic — after the racers, unchanged)
    fn reg_of(&self, s: usize) -> usize {
        if self.exch_order.is_empty() || s > self.n_seat_cars {
            return s;
        }
        let want = if s == 0 { HumanCar::Local } else { HumanCar::Seat(s - 1) };
        self.exch_order.iter().position(|h| *h == want).unwrap_or(s)
    }
}

/// The collision pass's per-tick inputs (`App::collision_inputs`).
struct CollisionInputs {
    /// `if ([0x657408] == 0 || [0x65740C] == 0)` fails: no car meets another
    no_carcol: bool,
    /// `.TINYCAR`'s divisor of a car's collision mass
    tiny_k: f32,
    player_mass: f32,
    /// every opponent's collision-mass factor
    opp_k: Vec<f32>,
}

/// What a section of the 32 Hz block hands on to the later ones (`App::simulate`).
#[derive(Default)]
struct TickState {
    /// the controller sample the car reads this tick (`tick_controls`)
    controls: Controls,
    /// the opponents that completed a lap this tick, for the Quick Knockout cut (`tick_ai_opponents`)
    ko_laps: Vec<(usize, u32)>,
    /// the remote cars of a positional race whose `Car_simulate` runs this tick on the wire's controls
    remote_steps: Vec<(usize, u32, Controls)>,
    /// the player's pose as the traffic's AI pass took it, which the traffic's drive reads too (`tick_ai_traffic`)
    traffic_reference: Option<Reference>,
    /// the player asked for a reset (the key, the exchange's command 5 or the scripted bot), which a remote
    /// car of a positional race shares (`tick_player_reset`)
    asked: bool,
}

/// Everyone else on the road, split out of [`App`] on 2026-09-21: the opponents, the traffic and
/// the police, each with its models, bodies, previous poses and per-car sound set and paint.
/// The fields kept their names, so `self.x` became `self.road.x`.
pub(crate) struct RoadCars {
    /// opponents (parked until the AI rules are implemented) with their own models
    pub(crate) opponents: Vec<RaceCar>,
    pub(crate) opponent_models: Vec<CarModel>,
    /// opponent poses before the last physics tick, as `App::render_prev` is for the player
    pub(crate) opp_prev: Vec<(Vec3, Quat)>,
    /// PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x438478…0x4384B5]:
    /// each opponent's front-wheel angles before the last tick — `wheels[0..2].steer` and, for a
    /// replay, `steer_smoothed` — as `opp_prev` is for the pose; render state only
    pub(crate) opp_steer_prev: Vec<[f32; 4]>,
    /// the same for the player's car
    pub(crate) player_steer_prev: Option<[f32; 4]>,
    /// kinematic traffic (`nfs_game::traffic`), rebuilt with the scene
    pub(crate) traffic: Option<TrafficSet>,
    pub(crate) traffic_models: Vec<CarModel>,
    /// engine sound set per opponent (`nfs5.car +0x179`), for the AI engine loops
    pub(crate) opponent_sets: Vec<String>,
    /// engine sound set per traffic car, same source
    pub(crate) traffic_sets: Vec<String>,
    /// paint colour per opponent, for the HUD map blips
    pub(crate) opponent_paints: Vec<[u8; 3]>,
    /// each opponent's `nfs5.car +0x34` family (`spec+0x534`, the short car label the results
    /// boards print — `modes.md` §7.1, 0x42C368…0x42C3BA), pushed beside `opponent_paints`
    pub(crate) opponent_families: Vec<String>,
    /// paint colour per traffic car, same use
    pub(crate) traffic_paints: Vec<[u8; 3]>,
    /// police: the spawn manager, the cop cars and their models (`traffic.md` §17, §25, §27)
    pub(crate) police: Option<nfs_game::police::Police>,
    pub(crate) police_models: Vec<CarModel>,
    /// per cop car: its catalogue engine sound set and its own `.sim`'s specs (§17.1), for the
    /// engine it sounds from an audio slot (0x4A7080, as any car)
    pub(crate) police_sets: Vec<String>,
    pub(crate) police_specs: Vec<CarSpecs>,
    pub(crate) police_prev: Vec<(Vec3, Quat)>,
    /// whether the race was set up with police at all (`COPS`)
    pub(crate) cops_enabled: bool,
    /// how many cop cars go on the road when `cops_enabled`: **4** for the cops race
    /// (0x4ED0BC…0x4ED282), **2** for a factory-driver mission with `fac+0x26` (0x4ED0E7)
    pub(crate) cop_count: u32,
    /// `RACE_TYPE 1` (0x4EB4FD): a single race launched with the Fuzzyfuz code, cheat bit 5
    pub(crate) cops_race: bool,
    pub(crate) traffic_prev: Vec<(Vec3, Quat)>,
    /// `default.sim` — what `Car_loadSim` 0x49C6C0 falls back to for a car with no sim name (§21.1)
    pub(crate) traffic_specs: Option<CarSpecs>,
    /// wheel roll angle per traffic car (radians), integrated from the distance driven
    pub(crate) traffic_spin: Vec<f32>,
    /// each car's LOD class and translation channels (`R3DCar+0x2C0`, `+0x6D0`), which the
    /// original keeps on the car's own `R3DCar` — here beside the models the cars of a type share
    pub(crate) opponent_lods: Vec<nfs_game::world::CarLodState>,
    pub(crate) police_lods: Vec<nfs_game::world::CarLodState>,
    pub(crate) traffic_lods: Vec<nfs_game::world::CarLodState>,
    /// number of traffic cars this race was set up with
    pub(crate) traffic_count: u32,
    /// `Game.Damage` `[0x657A2C]`: `Damage` 0x494500 adds to any car's zones only while it is
    /// set (0x49451B), and `Car_ground`'s roll-over sound needs it too (0x49C095); the crash
    /// response runs regardless. Every body of the race takes it (`Vehicle::damage_enabled`)
    pub(crate) damage_on: bool,
    /// car ids to build the traffic from instead of the track's list (a tournament's own
    /// `trn+0x382`, `career_ui::StartRequest::traffic_ids`)
    pub(crate) traffic_ids: Option<Vec<u16>>,
    /// per traffic car, the colour the host's per-slot record (`.COLOUR` 0x225, 0x4EEDA0) named
    /// when it differs from the model's first paint set (a network race only)
    pub(crate) traffic_colours: Vec<Option<[u8; 3]>>,
}

/// A lap or record time in 64 Hz ticks, the way 0x4356E0 prints it: seconds are `ticks/64`, the
/// minutes `secs/60` and the seconds `secs % 60`, and the hundredths `(ticks & 63)·100/64`
/// (0x43573C…0x43579D). Minutes below ten get a leading zero (0x4357AF).
/// 0x40F654…0x40F691: the slice a reset puts a car on — its own, or the first one after it along
/// the race (0x4817D0 with `[0x657430] ? −1 : 1`, which is +1 along this port's lap) that
/// 0x40F480 calls free: the slice centre is over a drivable patch (`Tracker::updateZone` then
/// `car+0x2C & 0xF`, 0x40F4BF…0x40F4C8) and no other live racer stands on it (0x40F4D3…0x40F500).
/// The other cars and the smackables are the `occupied` list's too ([`reset_blockers`]).
/// The original searches without end; the port stops after one lap (a guard, not a rule).
fn free_reset_slice(t: &SliceTable, grid: &nfs_physics::collision::TriangleGrid, start: usize, occupied: &[usize]) -> usize {
    let mut s = start;
    for _ in 0..t.lap_len.max(1) {
        if grid.find_patch(t.slices[s].centre) & 0xF != 0 && !occupied.contains(&s) {
            return s;
        }
        s = t.add(s, 1);
    }
    start
}

/// The slices 0x40F480 finds taken besides a racer's: every live cop and traffic car of
/// `0x5E8B34[]` (`car+0x7F`, 0x40F4EC — all classes are in that list) and every smackable of
/// `0x5E8C24[]` whether asleep or not (0x40F502…0x40F523, `RaceSmackables::slices` — the track's
/// signs and hydrants as much as a mission's cones). The caller leaves out the set whose own reset
/// adds itself (`Police::reset_at`, `TrafficSet::reset_to_slice`).
fn reset_blockers(police: Option<&nfs_game::police::Police>, traffic: Option<&TrafficSet>, props: &nfs_game::smackables::RaceSmackables) -> Vec<usize> {
    police
        .into_iter()
        .flat_map(|p| p.cars.iter().filter(|c| c.active).map(|c| c.slice))
        .chain(traffic.into_iter().flat_map(|tr| tr.cars.iter().filter(|c| c.active).map(|c| c.slice)))
        .chain(props.slices().iter().copied())
        .collect()
}

/// `Grid_slot` 0x410580's slice-local offset `{lateral, 0.5, 0}` (0x41070F…0x410718) for
/// `Vehicle::place_at_slice` (0x49B090), read back from `grid_pose`'s `centre + lateral·x +
/// up·0.5` so the lateral rule stays in one place (`nfs_game::race::grid_pose`).
pub(crate) fn grid_offset(s: &nfs_game::race::Slice, pos: Vec3) -> Vec3 {
    let d = pos - s.centre - s.up * 0.5;
    let x = d.dot(s.lateral) / s.lateral.length_squared().max(1e-6);
    Vec3::new(x, 0.5, 0.0)
}
