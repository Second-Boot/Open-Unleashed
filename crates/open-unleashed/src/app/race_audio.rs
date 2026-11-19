//! The race's audio update of the odd 32 Hz tick (`0x4A8120`), moved out of the simulation file unchanged. It is a **sim file** for the
//! build fingerprint: the AI cars' loop producers draw the game's shared random stream here whether or not a sound device is up.

use crate::*;
use nfs_game::track_id::TrackId;

impl App {
    /// The point the player's audio measures from: `car+0x330` of the car the camera follows (a
    /// network replay's viewed car, as [`Self::car_pos`] takes it, else the player's own) — the
    /// physics position, not the draw origin.
    pub(super) fn audio_car_position(&self, v: &Vehicle) -> Vec3 {
        Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view)
            .and_then(|k| self.road.opponents.get(k))
            .map_or(v.position, |o| o.vehicle.position)
    }

    /// **The odd tick's audio update** (`0x4A8120`, see the comment above its call in [`Self::simulate`]):
    /// the speech service, the thunder, the audio slots and the cars in them, the loop producers, the
    /// ambience, the car effects, the music, the tunnel reverb, the player's engine and SFX. Moved out
    /// of `simulate` unchanged; `view` is the camera view id and `exch_humans_now` the exchange
    /// session's human cars in registration order, both taken by `simulate` before the car was borrowed.
    pub(super) fn simulate_audio(&mut self, view: u16, exch_humans_now: Vec<HumanCar>) {
        let Some(v) = self.vehicle.as_ref() else { return };
        let car_330 = self.audio_car_position(v);
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
        // the speech service 0x4B0A20 is also the pump of the streamed-patch loads: it installs one finished bank and
        // issues the queued reads (`Ambience::bank_pump`), so a bank the ambience requested at update `u` can play from
        // update `u + 2` on
        if let Some(amb) = self.snd.ambience.as_mut() {
            amb.bank_pump();
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
                    s.siren(a, *slot, false, 0.0, nfs_audio::ai_engine::Heard { pan: 0, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL }, 0);
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
                            nfs_audio::ai_engine::AiCarInput { rpm_frac: frac, distance: d, cos, speed16: nfs_audio::ai_engine::ai_speed_16(o.vehicle.v_long()) },
                            nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL },
                            nfs_audio::ai_engine::AiScene { detail: self.snd.audio_settings.audio_quality, crowding, spline_cam: view == 0x0B, levels: ai_levels },
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
                                n.owned_seats().contains(&i) && n.seat_arm(i).horn(self.net_launch.pad_buttons & 2 != 0)
                            };
                            if horn && self.tick % 32 == 1 {
                                tracing::debug!(target: "netplay", seat = i, slot, tick = self.tick, distance = d, "a remote human's horn is voiced (0x4A7080's human arm)");
                            }
                            ai.honk(a, slot, horn, d, nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL }, self.snd.audio_settings.ambient_level);
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
                            nfs_audio::ai_engine::AiCarInput { rpm_frac: frac, distance: d, cos, speed16: nfs_audio::ai_engine::ai_speed_16(c.velocity.dot(c.orientation.row(2))) },
                            nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL },
                            nfs_audio::ai_engine::AiScene { detail: self.snd.audio_settings.audio_quality, crowding, spline_cam: view == 0x0B, levels: ai_levels },
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
                        ai.honk(a, slot, step == nfs_audio::ai_engine::HonkStep::Play, d, nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL }, self.snd.audio_settings.ambient_level);
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
                            nfs_audio::ai_engine::AiCarInput { rpm_frac: frac, distance: d, cos, speed16: nfs_audio::ai_engine::ai_speed_16(c.velocity.dot(c.orientation.row(2))) },
                            nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL },
                            nfs_audio::ai_engine::AiScene { detail: self.snd.audio_settings.audio_quality, crowding, spline_cam: view == 0x0B, levels: ai_levels },
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
                s.siren(a, slot, c.active && c.lights, d, nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL }, self.snd.audio_settings.ambient_level);
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
            let autobahn = self.track.name.eq_ignore_ascii_case(TrackId::Autobahn.name());
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
        let wet = self.track_wet_at(self.cam_slice.unwrap_or(0) as i32).unwrap_or(0.0) > 0.1;
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
            // the car's `+0x330` (0x4A7753…0x4A7790 → 0x4AA050's distance, 0x4A7862 its angle, 0x4A7918
            // its `cos`), not the draw origin `car_pos`, which sits the body lift below it. *(Until
            // 2026-10-24 the port measured from the draw origin: in the cockpit 1.11 m for the
            // original's 0.57 m, the engine 1.6 dB quieter and its pair pan narrower —
            // `REFERENCE-2026-10-24-PA-AUDIOMIX.md`.)*
            let to_car = car_330 - self.camera.position;
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
                    // upshift: the gear over the one before it, `car+0xD81` = `prev_gear`, kept from
                    // the change to the next, 0x4AA62C…0x4AA63A) **and `car+0xD81 != 1`** — out of
                    // neutral it does not dip at all (0x4AA640). *(Until 2026-10-24 the port compared
                    // the gear with the one its last audio update saw, true on the change's update
                    // alone: the dip ran for one update instead of the whole shift.)*
                    shift: (v.shift_timer > 0 && v.gear > v.prev_gear && v.prev_gear != 1)
                        .then_some((v.shift_timer, self.specs.shift_ticks)),
                },
            );
            // The click fires on the last tick of the shift (`car+0xD84 == 1`), not at the gear
            // change, with `[0x627F43] & 8` (viewId <= 1) **and** `viewId ∉ {0, 0xF}`, which together
            // leave exactly **view 1** (0x4AA6C5…0x4AA6FE) — on **every** gear change: the test is
            // after §4.3's dip, which every one of its exits reaches (0x4AA626 / 0x4AA63A / 0x4AA642
            // → 0x4AA6C2 → 0x4AA6C5), so an upshift, a downshift and the pull away out of neutral all
            // click. Measured on the running original (`REFERENCE-2026-10-24-PA-AUDIOMIX.md`): the click
            // a shift's length after the launch, each upshift and each downshift. *(Until 2026-10-24
            // the port took it as part of the dip — upshifts out of a forward gear only — and compared
            // the gear with its last audio update's, so it never fired.)*
            if v.shift_timer == 1 && view == 1 {
                e.play_shift(a, self.snd.audio_settings.sfx_level);
            }
        }
        // **0x4A7660's head** (0x4A76B1…0x4A7744), the player's audio update: the wrong-way counter
        // `car+0xD30` steps here, on the odd tick, and its 64th, 96th and 128th step sound the cue
        // 0x4AAE70 (outside a mission, which gates the counter to 0, and a skill test)
        let forward = v.forward();
        if let (Some(t), Some(r)) = (&self.slices, self.race.as_mut())
            && r.wrong_way_audio_step(t, forward)
            && self.mission.is_none()
            && self.skill.is_none()
            && let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut())
        {
            s.wrong_way_cue(a);
        }
        if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
            s.update();
            // the queue's distance is the car's own (`car+0x330`, 0x4A78E8) and so is the loops'
            // pan (0x4A7862…0x4A78E5, `MIRROR` negating it at 0x4A78C1 as for the engine above)
            let to_car = car_330 - self.camera.position;
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
                wet: self.track.name.eq_ignore_ascii_case(TrackId::Autobahn.name()) && w.overhead_flags(v.position) == 0,
                human: true,
            };
            // (an exchange race, or its `--resim`, produced it above, in registration order)
            if !self.net.session.as_ref().is_some_and(|n| n.exchanging()) && self.resim.is_none() {
                self.snd.player_loops.produce_loops(&input, nfs_game::rng::rand16);
            }
            // the queue's one-shots — pushed during the tick's physics, above — take the angle to
            // `car+0x440` as it stands now, read ONCE at this drain (0x4A7BB1…0x4A7C3B), after the
            // whole tick's collisions: every one-shot of the tick takes the same, final contact,
            // not each one's own moment (`CollisionSfx::drain_player`; approximation withdrawn
            // 2026-09-27, `sfx.md` "The drain's distance and pan")
            let one_shot_pan = listener_pan(self.camera.position, self.camera.target, self.mirror, v.last_contact);
            // TEST GLUE: `NFS_PA_LOOPS`, the drain's loop entries (the original's 0x4A93C0 calls)
            super::glue::pa_trace::loops(self.tick, self.snd.player_loops.entries(), self.camera.position, v.position, (self.camera.target - self.camera.position).normalize_or_zero(), v.orientation.row(2));
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
                s.road_roll(a, v.v_long(), v.surface_nibble(), road_a, mode >= 2, nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL });
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
                    nfs_audio::sfx::TurboState { boost: v.boost, throttle: v.throttle_raw, rpm: v.rpm, redline: self.specs.redline },
                    e.aux_volume(mode),
                    e.turbo_gain(),
                    nfs_audio::ai_engine::Heard { pan, pitch_base: nfs_audio::ai_engine::PITCH_BASE_STILL },
                );
            }
        }
    }
}
