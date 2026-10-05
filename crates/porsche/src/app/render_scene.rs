//! Scene assembly: the per-frame draw lists (`rebuild_scene`), the dynamic items and the HUD geometry.

use crate::*;

impl App {
    /// Re-bind the cars' environment map when the `.map` section under the player changes
    /// (`RE/notes/lighting.md` §3): inside a section the car reflects that section's map, in its
    /// covered variant while something drivable hangs over the car, and the track's default map
    /// everywhere else.
    /// Repaint the car's texture pages when its body damage changes (`RE/notes/car-model.md` §11):
    /// the `<model>D.fsh` / `<model>L.fsh` decals are blended over the pages with an alpha that
    /// follows each zone's damage. `CarModel::update_damage` is 0x438230, which does nothing until
    /// a zone's value changes, so this is cheap to call every frame. Every car with its own model
    /// takes it — the player (the car the camera follows) and each opponent; the traffic and the
    /// cops share one model per type and keep their pages clean (gap: the original's are per car).
    pub(crate) fn update_damage_decals(&mut self) {
        let Some(mut damage) = self.vehicle.as_ref().map(|v| v.damage) else {
            return;
        };
        // diagnostic: `NFS_DAMAGE=<per cent>` puts that much damage on every zone
        let forced = nfs_game::config::cfg().nfs_damage.var().ok().map(|v| v.parse::<f32>().unwrap_or(0.0));
        if let Some(d) = forced {
            damage = [d; 10];
        }
        // `CARDETAIL` `[0x6197C4]`: the crush 0x43E390 needs it above 0
        let detail = self.choice_index("Graphics.CarDetail").map_or(0, |(i, _)| i as u8);
        // 0x4382EC: the zone caps of the car the model's arms follow (0x465D80(0))
        let followed = self.followed_seat();
        if let (Some(c), Some(r)) = (self.car_model.as_mut(), self.renderer.as_mut()) {
            c.update_damage(r, &damage, followed.is_none(), detail);
        }
        if let Some(r) = self.renderer.as_mut() {
            for (i, o) in self.road.opponents.iter().enumerate() {
                if let Some(m) = self.road.opponent_models.get_mut(o.model) {
                    m.update_damage(r, &forced.map_or(o.vehicle.damage, |d| [d; 10]), followed == Some(i), detail);
                }
            }
        }
        // slot 9's race animations (the wing off the speed gate, a tiny car's aerial) and slot 6's
        // pop-up headlights off the track's night flag (`car-model.md`)
        let speed = self.vehicle.as_ref().map(|v| v.speed()).unwrap_or(0.0);
        let reverse = self.vehicle.as_ref().is_some_and(|v| v.gear == 0);
        let night = self.track.is_night();
        let tiny = self.cheat_bits & (1 << 1) != 0;
        // slot 9's driver: `[0x6197C4]` is `CARDETAIL`, the `Graphics.CarDetail` choice (0x44EF80)
        let detail = self.choice_index("Graphics.CarDetail").map_or(0, |(i, _)| i as u8);
        let driver = self.vehicle.as_ref().map(|v| nfs_game::world::DriverInputs {
            detail,
            // channels 1 and 2's `.y`: each front wheel's `+0xB4` over 216, negated when mirrored
            wheel_turns: (v.wheels[0].steer + v.wheels[1].steer) / 216.0 * if self.mirror { -1.0 } else { 1.0 },
            shift_timer: v.shift_timer,
            racing: v.race_state == 0 && !self.paused,
            handbrake: v.handbrake,
            reverse: v.gear == 0,
            long_acc: v.long_filtered,
            lat_acc: v.lateral_filtered,
        });
        if let (Some(c), Some(r)) = (self.car_model.as_mut(), self.renderer.as_mut()) {
            c.update_animation(r, 16, speed, night, tiny, reverse);
            if let Some(d) = driver {
                c.update_driver(r, 16, &d);
            }
            // the shared traffic and police models take slot 9's clock channel (the siren)
            for m in self.road.police_models.iter_mut().chain(self.road.traffic_models.iter_mut()) {
                m.update_clock_channels(16);
            }
            // slot 9 runs for every car's model: the opponents' own too
            let k = if self.mirror { -1.0 } else { 1.0 };
            // 0x43846F: with `[0x657424] >= 2` (a replay) the steer channels take the smoothed
            // angle `car+0xAC8` / `+0xACC` instead of the wheels' own
            let replaying = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
            let wheel_steer = |v: &nfs_physics::vehicle::Vehicle| if replaying { v.steer_smoothed } else { [v.wheels[0].steer, v.wheels[1].steer] };
            for o in &self.road.opponents {
                let Some(m) = self.road.opponent_models.get_mut(o.model) else { continue };
                let v = &o.vehicle;
                m.update_animation(r, 16, v.speed(), night, o.tiny, v.gear == 0);
                m.update_driver(
                    r,
                    16,
                    &nfs_game::world::DriverInputs {
                        detail,
                        wheel_turns: (v.wheels[0].steer + v.wheels[1].steer) / 216.0 * k,
                        shift_timer: v.shift_timer,
                        racing: v.race_state == 0 && !self.paused,
                        handbrake: v.handbrake,
                        reverse: v.gear == 0,
                        long_acc: v.long_filtered,
                        lat_acc: v.lateral_filtered,
                    },
                );
                let ws = wheel_steer(v);
                m.update_race_channels(
                    16,
                    [ws[0] / 216.0 * k, ws[1] / 216.0 * k],
                    [v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation],
                    v.rpm,
                    v.speed(),
                );
            }
            if let Some(v) = self.vehicle.as_ref() {
                let ws = wheel_steer(v);
                c.update_race_channels(
                    16,
                    [ws[0] / 216.0 * k, ws[1] / 216.0 * k],
                    [v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation],
                    v.rpm,
                    v.speed(),
                );
            }
        }
    }

    /// The cars' and the skid marks' own `+0xC` entries for one view's pass: the main pass's
    /// (`Self::rebuild_scene`) or the mirror's (`Self::render_mirror`, `p.mirror_pass`), each from
    /// the pass's own eye — `self.camera` is the view's camera while this runs (the mirror swaps its
    /// own in), its far plane `self.view_far`. The draw 0x4430A0 runs in both passes over the same
    /// car list (0x443610): in the mirror's, slot 0's flag bit 4 is set (0x468DA8…0x468E1F) and the
    /// camera's own car (`carInfo+0x520 == 0x465D80(0)`) returns at 0x4430EC — nothing of it is queued,
    /// no shadow, body or lamps; the others go through the frustum test 0x539480 (`car+0x330`, radius
    /// `car+0x3B4`) and the LOD pick of the mirror's view. Difference (not an approximation, see below): the draw's last act, the
    /// effect drive (slot 7, 0x443239), runs in the mirror's pass too in the original, with that
    /// pass's eye; the port drives the effects once a frame, from the main pass. **Settled
    /// 2026-09-30: equivalent.** 0x43EF80 measures `d²` from the slot's eye
    /// `[0x627EA4…0x627EAC]`, which the mirror pass does not touch (its own eye is the view's
    /// camera object), and every write it makes — `+0x2C = 1`, `+8 = 100`, `+0x10 = d²`, the
    /// emitter points and directions, `+0x28` behind an `!=` guard (0x454AC0), the draw set's
    /// `std::set` insert — is a function of the car's state and that eye, so a second call in the
    /// frame writes what the first did. No count or timer moves in the drive. The own car returns
    /// before it in the mirror (0x4430E5) and the main pass has no frustum test, so the main drive
    /// already reaches every car the mirror's would. The mirror's meshes come after the main pass's in
    /// each dynamic pool (`PoolUse`), so nothing the main view queued is rewritten.
    pub(crate) fn dyn_items(&mut self, p: &DynPose, pools: &mut PoolUse, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        let DynPose { alpha, local_draw, local_pos, local_rot, mirror_pass, .. } = *p;
        let seen = |pos: Vec3, half: Vec3| p.cull.as_ref().is_none_or(|cv| cv.sphere_visible(pos, half.length()));
        queue.clear();
        // the per-car buffers live in `self.scratch` between frames: taken here, put back at the end
        let mut car_items = std::mem::take(&mut self.scratch.car_items);
        let mut shadow_verts = std::mem::take(&mut self.scratch.shadow_verts);
        let mut lamp_scratch = std::mem::take(&mut self.scratch.lamp_draws);
        let lamp_ms = (self.tick as f64 + self.pause_offset + self.pause_ticks) * 1000.0 / 64.0;
        let fade = (lamp_ms / 1000.0).min(1.0) as f32;
        // The LOD pick, `R3DCar` slot 13 0x439F90, which the car's draw 0x4430A0 asks first
        // (0x443192): each car's class from its box on the screen `[0x657A48]` wide and `CARDETAIL`
        // (`[0x6197C4]`, the `Graphics.CarDetail` choice, five tiers under `nfs5.dll`, which also
        // cuts the replay test out of the pick, 0x43A08A); `None` — 3 pixels or less — draws nothing of it, lamps included
        // (`CarModel::lod_class`). Then the car draw (0x444E00, the dll's 0x10001090) makes its passes from the class
        // (`race_passes`). *(Until 2026-09-26 every car was LOD 1 at every distance, every part
        // of it drawn.)* The mirror's pass asks again for its own view (`Self::render_mirror`).
        let detail = self.choice_index("Graphics.CarDetail").map_or(0, |(i, _)| i as u8);
        // diagnostic: `NFS_DAMAGE=<per cent>` (`update_damage_decals`'s own forcing of the
        // player's and the opponents' zones) also puts that much damage on every traffic and cop
        // car drawn below, for a screenshot check of their per-car decals/crush. Port-only
        // extension, `car-model.md` §13.1: a comma list, `NFS_DAMAGE=<a>,<b>,…`, is cycled by each
        // traffic/cop car's own draw position, so two cars sharing one catalogue model can be
        // told apart — a screenshot check that the shared-model aliasing fix actually holds.
        let forced_damage: Vec<f32> = nfs_game::config::cfg().nfs_damage.var()
            .ok()
            .map(|v| v.split(',').filter_map(|s| s.trim().parse::<f32>().ok()).collect())
            .unwrap_or_default();
        let forced_damage_at = |i: usize| -> Option<f32> { (!forced_damage.is_empty()).then(|| forced_damage[i % forced_damage.len()]) };
        let replaying = self.replay_view().is_some();
        // the pick's screen: the view's own viewport (the mirror's tray, 0x539700 through its
        // camera), the size scaled to the virtual reference width (640 at 4:3) by the render width `[0x657A48]`
        let lod_view = self.renderer.as_ref().map(|r| {
            let (w, h) = p.lod;
            // `nfs5.dll` replaces the 640 of 0x43A2DD with its virtual 2-D width for the mode (window)
            let (win_w, win_h) = r.size();
            let virtual_width = nfs_game::world::virtual_width(win_w.max(1) as f32, win_h.max(1) as f32);
            nfs_game::world::LodView { view_projection: self.camera.view_projection(w / h), width: w, height: h, ref_width: p.ref_width, virtual_width }
        });
        let mirror = self.mirror;
        // **0x465D80(0)**: the car the model's "followed car" arms follow (`None` = the local car)
        let followed = self.followed_seat();
        let own_view = self.camera_view();
        let looking = (self.input.held(Action::RearView) || nfs_game::config::cfg().nfs_lookback.var().is_ok()) && self.vehicle.is_some();
        // a mirrored race turns every car model back across its x (0x4433A1…0x4433BC)
        for m in self.car_model.iter_mut().chain(self.road.opponent_models.iter_mut()).chain(self.road.traffic_models.iter_mut()).chain(self.road.police_models.iter_mut()) {
            m.mirror = mirror;
        }
        // 0x43A3B0 stores the class, then 0x43E670 lifts the wheels from the LODs below it
        let pick = |model: &CarModel, state: &mut nfs_game::world::CarLodState, p: Vec3, r: Quat, half: Vec3, kind: nfs_game::world::CarKind, compression: [f32; 4]| -> Option<Vec<nfs_game::world::CarPass>> {
            let class = model.lod_class(lod_view.as_ref()?, p, r, half, detail, kind)?;
            state.class = class;
            model.update_travel(state, compression, mirror);
            Some(nfs_game::world::race_passes(class, detail, kind == nfs_game::world::CarKind::Traffic))
        };
        let (mut lamp_used, mut shadow_used) = (pools.lamp, pools.shadow);
        // every car drops a blob shadow on the ground under it (0x43B820), off `car+0x330` and cut
        // to `car+0x3A8…` like every car's (0x43B220)
        // **TEST GLUE** (not in the original): `NFS_NO_SHADOW=1` leaves the blob shadows out, for a with / without difference
        let shadow_material = self.shadow_draw.map(|(_, m)| m).filter(|_| nfs_game::config::cfg().nfs_no_shadow.var_os().is_none());
        let counts = (self.road.traffic.as_ref().map_or(0, |t| t.cars.len()), self.road.police.as_ref().map_or(0, |p| p.cars.len()), self.road.opponents.len());
        self.road.traffic_lods.resize(counts.0, Default::default());
        self.road.police_lods.resize(counts.1, Default::default());
        self.road.opponent_lods.resize(counts.2, Default::default());
        let compression = |v: &nfs_physics::vehicle::Vehicle| -> [f32; 4] { std::array::from_fn(|k| v.wheels[k].compression) };
        // the cars' effect drive 0x43EF80(1), which the car's draw 0x4430A0 runs last (slot 7
        // 0x4383C0 at 0x443239) — so for every car drawn: the eye `0x627EA4`, the sky's field 5 at
        // the camera's slice (0x45DD80 → 0x45D000, the rain's value, `weather_strengths`) and
        // `[0x6197AA]` taken as set (`thrash.md` §4). The effect sets are keyed player 0, the
        // opponents, the cops, the traffic (`Self::effect_key`)
        let fx_eye = self.camera.position;
        let fx_wet = self.weather_strengths().1 > 0.1;
        if let Some(tr) = &self.road.traffic {
            for (i, c) in tr.cars.iter().enumerate().rev().filter(|(_, c)| c.active) {
                if !seen(c.position, c.half_extents) {
                    continue;
                }
                // the car's whole draw (shadow, body, lamps) is one `+0xC` entry in the original
                // (0x4430A0 → 0x443229, key `round(d²)` 0x4431F8, from the eye to the car's centre —
                // its draw position `p` below): collected here and merged with the track's far list
                // and the skid marks farthest-first (`Self::blended_dyn_items`)
                car_items.clear();
                // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the draw pose is the previous tick's blended toward this one by `alpha`; the shadow follows it
                let (p1, r1) = (
                    c.draw_origin(),
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                );
                let (p, r) = match self.road.traffic_prev.get(i) {
                    Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                        (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                    }
                    _ => (p1, r1),
                };
                if let Some(mat) = shadow_material
                    && let Some(q) = self.shadow_quad(p + (c.position - c.draw_origin()), r, c.half_extents, &mut shadow_verts)
                {
                    Self::push_shadow(&mut car_items, self.renderer.as_mut(), &mut self.shadow_meshes, &mut shadow_used, mat, (q, shadow_verts.as_slice()));
                }
                // the model at `car+0x330 − M[1]·car+0x3AC` (0x43A410), LOD 1's box
                let key = (p - fx_eye).length_squared().round_ties_even() as u32;
                let m = Mat4::from_rotation_translation(r, p);
                // a kinematic car has no wheel state: the wheels are rolled from the distance driven
                let spin = self.road.traffic_spin.get(i).copied().unwrap_or(0.0);
                let block = self.world.as_ref().map(|w| w.car_light(p));
                // `R3DCar` slot 7 (0x4383CC/0x438273) recomputes a car's decals and crush from
                // *its own* raw zone damage every frame it is drawn, with no car-kind gate — the
                // traffic and the cops share one upload per type, so this instance's own damage
                // (`Vehicle::damage`, `TrafficSet::post_impact`) is baked into that shared model
                // right before this car's own `draw_items` below, the same way `update_travel`
                // already is: two cars of one type drawn in the same frame each get their own
                // decals/crush because the model is mutated-then-drawn once per car, in order —
                // and, when they are simultaneously visible, each into its own instantiated copy
                // of the shared model's mesh/texture/material ids (`car-model.md` §13.1's residual
                // gap), so this port's batched `render()` does not alias one onto the other.
                let dmg = forced_damage_at(i).map_or_else(|| tr.bodies.get(i).map_or([0.0; 10], |b| b.damage), |d| [d; 10]);
                if let (Some(m), Some(rr)) = (self.road.traffic_models.get_mut(c.model), self.renderer.as_mut()) {
                    m.update_damage(rr, &dmg, false, detail);
                }
                let model = &self.road.traffic_models[c.model];
                let state = &mut self.road.traffic_lods[i];
                let Some(passes) = pick(model, state, p, r, c.half_extents, nfs_game::world::CarKind::Traffic, tr.bodies.get(i).map_or([0.0; 4], compression)) else {
                    queue.extend(car_items.drain(..).map(|it| (key, it)));
                    continue;
                };
                if let (Some(fx), Some(b)) = (self.car_effects.as_mut().filter(|_| !mirror_pass), tr.bodies.get(i)) {
                    fx.drive(1 + counts.2 + counts.1 + i, &nfs_game::car_effects::CarInput::from_vehicle(b), fx_eye, fx_wet, true);
                }
                let travel = state.travel;
                let mut items = match &block { Some(b) => model.draw_items_block(m, [spin; 4], [0.0; 2], travel, &passes, b), None => model.draw_items(m, [spin; 4], [0.0; 2], travel, &passes) };
                model.apply_projector(&mut items, self.projector_planes(p));
                model.apply_sky(&mut items, self.sky_map_for(p + (c.position - c.draw_origin()), c.slice));
                car_items.extend(items);
                // the brake lamps follow the AI's own counter `car+0xD80` (0x439360, `aidrive::brake_light`);
                // the reverse lamp its gear byte `car+0xD82`, 0 once the race is on while the AI
                // backs up at 2 m/s or more (0x408495…0x4084A1 → 0x408130), refreshed every
                // frame: its timer `car+0xEDC` is only ever cleared (0x406D22, 0x406E04), so
                // the decrement 0x40847E, taken while it is positive, never runs.
                // `car+0x41C`, the ground height the reflection 0x451D60 lays its quad on, is the
                // body's (`Vehicle::ground_h`, `Car_ground` on `TrafficSet::bodies` since
                // 2026-09-26); a car without a body (no grid) takes its origin's height
                let reverse = self.tick >= RACE_START_TICK && c.gear.sense < 0.0 && c.speed_estimate() >= 2.0;
                let slots = Self::lamp_slots(self.track.is_night(), if model.has_headlight_extent() { fade } else { 1.0 }, c.brake_light > 0, reverse, (false, false, false), None);
                let ground = tr.bodies.get(i).map_or(p.y, |b| b.ground_h);
                self.lamp_draws(model, p, r, ground, slots, &mut lamp_scratch);
                Self::push_lamp_draws(&mut car_items, self.renderer.as_mut(), &self.lamp_pools, &mut lamp_used, &mut lamp_scratch);
                queue.extend(car_items.drain(..).map(|it| (key, it)));
            }
        }
        if let Some(pol) = &self.road.police {
            for (i, c) in pol.cars.iter().enumerate().rev().filter(|(_, c)| c.active) {
                if !seen(c.position, c.half_extents) {
                    continue;
                }
                car_items.clear();
                // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the draw pose is the previous tick's blended toward this one by `alpha`; the shadow follows it
                let (p1, r1) = (
                    c.draw_origin(),
                    Quat::from_mat3(&c.orientation.transpose()).normalize(),
                );
                let (p, r) = match self.road.police_prev.get(i) {
                    Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                        (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                    }
                    _ => (p1, r1),
                };
                if let Some(mat) = shadow_material
                    && let Some(q) = self.shadow_quad(p + (c.position - c.draw_origin()), r, c.half_extents, &mut shadow_verts)
                {
                    Self::push_shadow(&mut car_items, self.renderer.as_mut(), &mut self.shadow_meshes, &mut shadow_used, mat, (q, shadow_verts.as_slice()));
                }
                // the model at `car+0x330 − M[1]·car+0x3AC` (0x43A410), LOD 5's box like a racer's
                let key = (p - fx_eye).length_squared().round_ties_even() as u32;
                let m = Mat4::from_rotation_translation(r, p);
                let spin = c.speed.abs() * self.tick as f32 / 64.0 / 0.32;
                let block = self.world.as_ref().map(|w| w.car_light(p));
                // as the traffic's above: this cop's own damage baked into its own instantiated
                // copy of the shared per-type model right before its own draw
                if let (Some(m), Some(rr)) = (self.road.police_models.get_mut(c.model), self.renderer.as_mut()) {
                    m.update_damage(rr, &forced_damage_at(i).map_or(c.body.damage, |d| [d; 10]), false, detail);
                }
                let model = &self.road.police_models[c.model];
                // a cop is `car+0x52C & 0x20`, neither the opponent nor the traffic bit
                let state = &mut self.road.police_lods[i];
                let Some(passes) = pick(model, state, p, r, c.half_extents, nfs_game::world::CarKind::Racer, compression(&c.body)) else {
                    queue.extend(car_items.drain(..).map(|it| (key, it)));
                    continue;
                };
                if let Some(fx) = self.car_effects.as_mut().filter(|_| !mirror_pass) {
                    fx.drive(1 + counts.2 + i, &nfs_game::car_effects::CarInput::from_vehicle(&c.body), fx_eye, fx_wet, true);
                }
                let travel = state.travel;
                let mut items = match &block { Some(b) => model.draw_items_block(m, [spin; 4], [0.0; 2], travel, &passes, b), None => model.draw_items(m, [spin; 4], [0.0; 2], travel, &passes) };
                model.apply_projector(&mut items, self.projector_planes(p));
                model.apply_sky(&mut items, self.sky_map_for(p + (c.position - c.draw_origin()), c.slice));
                car_items.extend(items);
                // the lamps as the traffic's (above): the AI's brake counter and gear byte, the
                // reflection on the body's `car+0x41C`
                let reverse = self.tick >= RACE_START_TICK && c.gear.sense < 0.0 && c.speed_estimate() >= 2.0;
                let slots = Self::lamp_slots(self.track.is_night(), if model.has_headlight_extent() { fade } else { 1.0 }, c.brake_light > 0, reverse, (false, false, false), None);
                self.lamp_draws(model, p, r, c.body.ground_h, slots, &mut lamp_scratch);
                Self::push_lamp_draws(&mut car_items, self.renderer.as_mut(), &self.lamp_pools, &mut lamp_used, &mut lamp_scratch);
                queue.extend(car_items.drain(..).map(|it| (key, it)));
            }
        }
        for (i, o) in self.road.opponents.iter().enumerate().rev() {
            car_items.clear();
            // 0x465D80(0) may be this seat's car (`followed`): then the arms of 0x437ABD / 0x43A020 / 0x4430D3 /
            // 0x444E18 / 0x4383EA apply to it, as they do to the local car in `followed`'s `None` arm
            let followed_here = followed == Some(i);
            // the mirror's pass: the followed car returns at 0x4430EC, the rest take the frustum test
            // (the main pass: the followed car skips the test in views 0 / 1, `[0x627F43] & 8`, 0x4430DD)
            if mirror_pass && followed_here || !(followed_here && own_view <= 1 || seen(o.vehicle.position, o.vehicle.half_extents)) {
                continue;
            }
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the draw pose is the previous tick's blended toward this one by `alpha`; the shadow follows it
            let (p1, r1) = (
                o.vehicle.draw_origin(),
                Quat::from_mat3(&o.vehicle.orientation.transpose()).normalize(),
            );
            let (p, r) = match self.road.opp_prev.get(i) {
                Some((p0, r0)) if p0.distance(p1) < 5.0 => {
                    (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize())
                }
                _ => (p1, r1),
            };
            if let Some(mat) = shadow_material
                && !(followed_here && own_view <= 1)
                && let Some(q) = self.shadow_quad(p + (o.vehicle.position - o.vehicle.draw_origin()), r, o.vehicle.half_extents, &mut shadow_verts)
            {
                Self::push_shadow(&mut car_items, self.renderer.as_mut(), &mut self.shadow_meshes, &mut shadow_used, mat, (q, shadow_verts.as_slice()));
            }
            let key = (p - fx_eye).length_squared().round_ties_even() as u32;
            let m = Mat4::from_rotation_translation(r, p);
            let w = &o.vehicle.wheels;
            let block = self.world.as_ref().map(|w| w.car_light(p));
            // the front wheels' `+0xB4` over 216 (slot 9's channels 1 and 2), in a replay the
            // smoothed `car+0xAC8` / `+0xACC` (0x438478…0x4384B5)
            let st = if replaying { o.vehicle.steer_smoothed } else { [w[0].steer, w[1].steer] };
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x438478…0x4384B5]: the angles lerped from the previous tick's, as the body is
            let st = match self.road.opp_steer_prev.get(i) {
                Some(q) => {
                    let q = if replaying { [q[2], q[3]] } else { [q[0], q[1]] };
                    [q[0] + (st[0] - q[0]) * alpha, q[1] + (st[1] - q[1]) * alpha]
                }
                None => st,
            };
            let (spin, steer) = ([w[0].rotation, w[1].rotation, w[2].rotation, w[3].rotation], [st[0] / 216.0, st[1] / 216.0]);
            let model = &self.road.opponent_models[o.model];
            let state = &mut self.road.opponent_lods[i];
            let passes = match (followed_here, own_view) {
                // 0x4430D3: the followed car in the bumper view is not drawn at all
                (true, 0) => None,
                // the driver view: the pick is skipped and the draw takes its cabin passes (0x43A020, 0x444E18)
                (true, 1) => {
                    model.update_travel(state, compression(&o.vehicle), mirror);
                    Some(nfs_game::world::in_car_passes(state.class, detail, looking))
                }
                _ => pick(model, state, p, r, o.vehicle.half_extents, nfs_game::world::CarKind::Opponent, compression(&o.vehicle)),
            };
            let Some(passes) = passes else {
                queue.extend(car_items.drain(..).map(|it| (key, it)));
                continue;
            };
            if (!followed_here || nfs_game::camera::draws_own_car(own_view))
                && !mirror_pass
                && let Some(fx) = self.car_effects.as_mut()
            {
                fx.drive(1 + i, &nfs_game::car_effects::CarInput::from_vehicle(&o.vehicle), fx_eye, fx_wet, true);
            }
            let travel = state.travel;
            let mut items = match &block {
                Some(b) => model.draw_items_block(m, spin, steer, travel, &passes, b),
                None => model.draw_items(m, spin, steer, travel, &passes),
            };
            model.apply_projector(&mut items, self.projector_planes(p));
            model.apply_sky(&mut items, self.sky_map_for(p + (o.vehicle.position - o.vehicle.draw_origin()), o.progress.slice));
            car_items.extend(items);
            let out = o.vehicle.race_state == 3;
            // the car's own lamp byte (`car+0xD87`, the night flag at `Car_init`): the indicators too
            let bits = o.lamp_bits;
            let slots = Self::lamp_slots(bits & 1 != 0, if model.has_headlight_extent() { fade } else { 1.0 }, o.vehicle.brake_lamp(), o.vehicle.gear == 0, (out || bits & 8 != 0, out || bits & 0x10 != 0, (self.tick / 32) % 2 == 1), Some(&o.vehicle.damage));
            self.lamp_draws(model, p, r, o.vehicle.ground_h, slots, &mut lamp_scratch);
            Self::push_lamp_draws(&mut car_items, self.renderer.as_mut(), &self.lamp_pools, &mut lamp_used, &mut lamp_scratch);
            queue.extend(car_items.drain(..).map(|it| (key, it)));
        }
        // the player's car, slot 0, the list's tail
        car_items.clear();
        let car_key = (local_draw - fx_eye).length_squared().round_ties_even() as u32;
        // the mirror's pass: the camera's own car (`followed` none) is not queued at all
        // (0x4430EC), any other goes through the frustum test
        // (the main pass: with the arms following another seat's car the local car is an ordinary
        // one and takes the frustum test; the camera's own car takes it only outside views 0 / 1 —
        // there it is in view by construction)
        let own_unseen = || !seen(local_pos, self.vehicle.as_ref().map_or(Vec3::ZERO, |v| v.half_extents));
        let own_skipped = if mirror_pass { followed.is_none() || own_unseen() } else { followed.is_some() && own_unseen() };
        // 0x437ABD: no shadow under the followed car in the bumper and driver views (`[0x627F43] & 8`)
        if let (Some(mat), Some(v)) = (shadow_material, self.vehicle.as_ref())
            && !(followed.is_none() && own_view <= 1)
            && !own_skipped
            && let Some(q) = self.shadow_quad(local_pos, local_rot, v.half_extents, &mut shadow_verts)
        {
            Self::push_shadow(&mut car_items, self.renderer.as_mut(), &mut self.shadow_meshes, &mut shadow_used, mat, (q, shadow_verts.as_slice()));
        }
        // R3DCar slot 14, 0x43A490, run by the car's draw 0x443490 inside its model transform: in
        // the driver view the `Wndo` corners go to the windscreen drops (0x459200 → 0x457E60).
        // *(Corrected 2026-09-26: this drew the `Wndo` quad itself with `INTGLASS`'s `glai` in views
        // 0 and 1. The quad is never drawn — 0x457E60 only builds the drops' plane from it — and
        // 0x43A490 also needs the slot's view id non-zero (0x465770(0)), so view 1 alone.)*
        if followed.is_none() && !mirror_pass {
            self.splat_items(local_rot, local_draw);
        }
        // The camera's own car (0x43A01E…0x43A05A, `carInfo+0x520 == 0x465D80(0)`): in the bumper
        // view (slot 0's view id 0, 0x465770) the LOD pick answers 0 and nothing of it is drawn;
        // in the driver view (`[0x627F43] & 8`, view 1 here) the pick is skipped — the class stays
        // what it was and the wheels are lifted from the LODs below it (0x43A3B6) — and the draw
        // 0x444E00 takes its cabin passes (`in_car_passes`: the shell and the cabin of LOD 2, the
        // steering wheel, the hands unless a look input is held — `I_LOOKBEHIND` here, the port
        // has no look-left / right keys). *(Until 2026-09-26 the port drew nothing of the car in
        // view 1: `camera::draws_own_car` read 0x4383FD as the body's gate — it gates the wheel
        // spray 0x43EF80.)*
        // (a car the camera does not follow is an ordinary one whatever the view: the view id the arms read is
        // slot 0's, and they follow `followed`)
        let gate_view = if followed.is_none() { own_view } else { u16::MAX };
        let own_passes = match (&self.car_model, gate_view) {
            _ if own_skipped => None,
            (_, 0) => None,
            (Some(c), 1) => {
                c.update_travel(&mut self.car_lod, self.vehicle.as_ref().map_or([0.0; 4], compression), mirror);
                Some(nfs_game::world::in_car_passes(self.car_lod.class, detail, looking))
            }
            (Some(c), _) => {
                let half = self.vehicle.as_ref().map_or(Vec3::ZERO, |v| v.half_extents);
                pick(c, &mut self.car_lod, local_draw, local_rot, half, nfs_game::world::CarKind::Racer, self.vehicle.as_ref().map_or([0.0; 4], compression))
            }
            (None, _) => None,
        };
        if let Some(c) = &self.car_model
            && let Some(passes) = own_passes
        {
            let travel = self.car_lod.travel;
            let m = Mat4::from_rotation_translation(local_rot, local_draw);
            let (spin, steer) = match &self.vehicle {
                Some(v) => (
                    [
                        v.wheels[0].rotation,
                        v.wheels[1].rotation,
                        v.wheels[2].rotation,
                        v.wheels[3].rotation,
                    ],
                    // in a replay the smoothed `car+0xAC8` / `+0xACC` (0x438478…0x4384B5)
                    {
                        let st = if replaying { v.steer_smoothed } else { [v.wheels[0].steer, v.wheels[1].steer] };
                        // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x438478…0x4384B5]: lerped from the previous tick's angles, as the body is
                        let st = match self.road.player_steer_prev {
                            Some(q) => {
                                let q = if replaying { [q[2], q[3]] } else { [q[0], q[1]] };
                                [q[0] + (st[0] - q[0]) * alpha, q[1] + (st[1] - q[1]) * alpha]
                            }
                            None => st,
                        };
                        [st[0] / 216.0, st[1] / 216.0]
                    },
                ),
                None => ([0.0; 4], [0.0; 2]),
            };
            // the car's light block — the ground under it times the `.map` colours, and the sun of
            // the track's sky (`RE/notes/lighting.md` §6)
            let block = self.world.as_ref().map(|w| w.car_light(local_pos));
            let mut items = match &block { Some(b) => c.draw_items_block(m, spin, steer, travel, &passes, b), None => c.draw_items(m, spin, steer, travel, &passes) };
            c.apply_projector(&mut items, self.projector_planes(local_pos));
            if let Some(slice) = self.race.as_ref().map(|r| r.slice) {
                c.apply_sky(&mut items, self.sky_map_for(local_pos, slice));
            }
            car_items.extend(items);
            // the effect drive, but not for the camera's own car in the bumper and driver views
            // (0x4383D8…0x43840C: `carInfo+0x520 == 0x465D80(0)` and the view id 0 or 1)
            if nfs_game::camera::draws_own_car(gate_view)
                && !mirror_pass
                && let (Some(fx), Some(v)) = (self.car_effects.as_mut(), self.vehicle.as_ref())
            {
                fx.drive(0, &nfs_game::car_effects::CarInput::from_vehicle(v), fx_eye, fx_wet, true);
            }
            let v = self.vehicle.as_ref();
            // the player's own lamp byte (`lamp_command`): the headlights and their ramp (a pop-up
            // car's only, 0x439627), and the indicators — both of them while eliminated (0x439408)
            let out = v.is_some_and(|v| v.race_state == 3);
            let bits = self.lamp_bits;
            let head_fade = if c.has_headlight_extent() { self.head_ramp.value(bits & 1 != 0, lamp_ms) } else { 1.0 };
            let signals = (out || bits & 8 != 0, out || bits & 0x10 != 0, (self.tick / 32) % 2 == 1);
            let slots = Self::lamp_slots(bits & 1 != 0, head_fade, v.is_some_and(|v| v.brake_lamp()), v.is_some_and(|v| v.gear == 0), signals, v.map(|v| &v.damage));
            self.lamp_draws(c, local_draw, local_rot, v.map_or(local_draw.y, |v| v.ground_h), slots, &mut lamp_scratch);
            Self::push_lamp_draws(&mut car_items, self.renderer.as_mut(), &self.lamp_pools, &mut lamp_used, &mut lamp_scratch);
        }
        queue.extend(car_items.drain(..).map(|it| (car_key, it)));
        (pools.lamp, pools.shadow) = (lamp_used, shadow_used);
        (self.scratch.car_items, self.scratch.shadow_verts, self.scratch.lamp_draws) = (car_items, shadow_verts, lamp_scratch);
            // the skid marks: 0x478AA0 draws a strip of more than two points when both of its ends
            // lie within 100 of the camera, as a strip with the UV table `[0x628B64]` — `(0,0)
            // (1,0) (0,1) (1,1)` over and over (0x478930) — its colours and `SKDP`, unlit.
            // The depth bias (0x478B51…0x478BA2): `−80000 · cam[+0xC] / d²` with `d²` the nearer
            // end's horizontal square distance, rounded by `fistp` and clamped to −10…−1, handed to
            // Thrash state 0x18 (0x537A60), `b · 2^-15` on the game's screen z `far/(far − near) ·
            // (1 − near / Z)` (0x540CE0, `RE/notes/thrash.md` §6d.3) — `DrawItem::depth_bias`, which
            // the shader adds to the clip z. `cam[+0xC]` is `far · near / (far − near)`
            // (0x467247…0x467261) of the view's planes — near the slot's `+0x5C`
            // (`camera::near_plane`: 0.5, 0.15 in the driver view), far 700 — the slope of that z,
            // so the bias is a near-constant `80000 / 32768` ≈ 2.4 m of view depth toward the eye
            // until the clamp (−10…−1) holds it. The render states of 0x468240 are not read
            // (alpha blended)
            if let (Some((_, material)), Some(r)) = (self.particle_draw, self.renderer.as_mut()) {
                let cam = self.camera.position;
                let near = |p: Vec3| (p.x - cam.x).powi(2) + (p.z - cam.z).powi(2) <= nfs_game::skidmarks::DRAW_DISTANCE.powi(2);
                const UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
                let mut used = pools.skid;
                let mut skid_verts = std::mem::take(&mut self.scratch.skid_verts);
                // one queue entry per cell in the original (0x44FFC0 with the walk's value for the
                // cell), whose draw 0x47A060 then lays the cell's strips in array order; every strip
                // of a cell carries the cell's key here, in that order
                // the walk's gates for a cell (node pass, frustum, `Vis `, `s² < far²`) with
                // the pass's own view — the main one's, or the mirror's
                // (**TEST GLUE**: `NFS_SKID_NOWALK=1` queues every cell as before, for a with / without still)
                let cells: Vec<(u32, Vec<&nfs_game::skidmarks::Strip>)> = match (self.world.as_ref(), p.cull.as_ref().filter(|_| nfs_game::config::cfg().nfs_skid_nowalk.var_os().is_none())) {
                    (Some(w), Some(cv)) => {
                        let walk = w.cell_walk(cv);
                        self.skid.cells_walked(|cx, cy, cz, r, nodes| walk.visit(cx, cy, cz, r, nodes))
                    }
                    _ => self.skid.cells(cam).collect(),
                };
                // **TEST GLUE**: `NFS_SKID_STATS=1` logs, per pass, the strips within 100 m of the eye
                // before and after the walk's gates
                if nfs_game::config::cfg().nfs_skid_stats.var_os().is_some() {
                    let within = |cs: &[(u32, Vec<&nfs_game::skidmarks::Strip>)]| cs.iter().flat_map(|(_, v)| v.iter()).filter(|s| s.points.first().zip(s.points.last()).is_some_and(|(&a, &b)| near(a) && near(b))).count();
                    let all: Vec<_> = self.skid.cells(cam).collect();
                    tracing::info!(mirror = p.mirror_pass, all = within(&all), walked = within(&cells), "skid strips in range");
                }
                for (key, strip) in cells.into_iter().flat_map(|(k, strips)| strips.into_iter().map(move |s| (k, s))) {
                    let (Some(&first), Some(&last)) = (strip.points.first(), strip.points.last()) else { continue };
                    if !near(first) || !near(last) {
                        continue;
                    }
                    // the pool grows as more strips come into range, so none is left out
                    if self.skid_pool.len() <= used {
                        let cap = nfs_game::skidmarks::MAX_CAPACITY as u32;
                        let mut skid = nfs_renderer::MeshData::default();
                        skid.vertices = vec![nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] }; cap as usize];
                        skid.indices = (0..cap - 2).flat_map(|k| [k, k + 1, k + 2]).collect();
                        self.skid_pool.push(r.upload_dynamic_mesh(&skid));
                    }
                    let mesh = self.skid_pool[used];
                    used += 1;
                    let z_near = self.camera.near;
                    let far = self.view_far;
                    let k = far * z_near / (far - z_near);
                    let d2 = |p: Vec3| ((p.x - cam.x).powi(2) + (p.z - cam.z).powi(2)).max(1e-6);
                    // 0x478B57…0x478B84: the quotient on the x87 stack, `fstp` to a float, then
                    // `fistp` — round to nearest even (f64 stands for the extended precision)
                    let bias = (((-80000.0 * k as f64 / d2(first).min(d2(last)) as f64) as f32).round_ties_even() as i32).clamp(-10, -1);
                    skid_verts.clear();
                    skid_verts.resize(nfs_game::skidmarks::MAX_CAPACITY, nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] });
                    for (k, v) in skid_verts.iter_mut().enumerate() {
                        // past the strip's end every vertex sits on its last point: no area
                        let i = k.min(strip.points.len() - 1);
                        let p = strip.points[i];
                        let c = strip.colours[i];
                        v.position = p.into();
                        v.uv = UV[k % 4];
                        v.color = [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0, (c >> 24) as f32 / 255.0];
                    }
                    r.write_mesh_vertices(mesh, &skid_verts);
                    // the alpha test `GREATER 10`: the skid object's drawer (`RDrawLineBundle`, vtable
                    // 0x5B3D28, type 0xE) queues each cell through slot 1 0x44FFC0 → 0x466A90 (list
                    // `+0xC`) — from the track walk 0x47F890's step 6 (`drawer slot 1(index, value)`,
                    // the cell being an item of `WLineBundle`, `RE/RENDERING.md` "The walk") with the
                    // value of a non-`bl` item, `s²` from the cell's sphere ([`SkidMarks::cell_key`],
                    // 0x479FC0) — and the flush 0x466660 draws that list (slot 2 0x451420 → 0x47A060)
                    // after its 0x24 ← 10 (0x466703), not under the particles' 1, which come later in
                    // the pass. Merged with the track's far list and the cars' own `+0xC` entries,
                    // farthest first (`Self::blended_dyn_items`). *(Until 2026-09-30 the key was the
                    // strip's own midpoint square distance to the eye.)* **Closed 2026-10-02, the chain read
                    // to the instruction:** 0x44FFC0 `fistp`s the walk's float (round to nearest even, the x87 default,
                    // 0x44FFC9), packs the item (0x44FE70: `[+0]` the bundle, `[+4]` 1, `[+6]` the ref's index),
                    // copies it through 0x443250 / 0x443270 with the key as the entry's last dword `+0xC`, takes the
                    // view `[0x628130]` (0x44FE90) and its list pointer `+0x54` (0x4432A0) and calls 0x466A90, whose tree
                    // insert compares that dword against each node's `+0x1C` **unsigned** (`jbe` / `ja`, 0x466AD3) and
                    // goes right for greater, left for equal-or-less — the key is exactly `round(s²)`. The walk's gates
                    // are the port's (`TrackWorld::cell_walk`). Residual, class B (no longer an unread call):
                    // a tie between two cells' keys (every cell whose sphere holds the eye has `s² = 0`) is ordered in
                    // the original by the node pass and the registration order of the cell records (the table sorted by
                    // the node addresses, `RE/notes/particles.md` §9 — the pool's runtime order); the port
                    // queues the cells in its table order, after the track's far list.
                    queue.push((key, nfs_renderer::DrawItem { layer_light: Some(unlit([1.0; 3])), alpha_test: true, alpha_ref: 10, depth_bias: bias, ..nfs_renderer::DrawItem::new(mesh, material, Mat4::IDENTITY) }));
                }
                pools.skid = used;
                self.scratch.skid_verts = skid_verts;
            }
    }

    /// **`[0x606AC8]`**, the HUD clock (64 Hz): what the billboards' strips (0x41A8A1), the trays'
    /// slide stamps and elapsed (0x434490, 0x434A0F) and the save dialog's caret (0x430D8C) read.
    /// The frame driver's loop steps it by one a pass until it reaches the timer's clock
    /// `[0x5E99B8]` (0x415B3F…0x415B52), which a pause does not stop — the port's game tick plus
    /// the pauses — on from where the session's earlier passes left it (`hud_clock_base`).
    pub(crate) fn hud_clock(&self) -> u64 {
        (self.hud_clock_base + self.tick as f64 + self.pause_offset + self.pause_ticks).max(0.0) as u64
    }

    /// **The port's HiDPI mapping** (a platform stand-in, not a game rule; 2026-10-01):
    /// the render target `(physical W, physical H, scale)` — the window's framebuffer and winit's
    /// scale factor, or a still's `NFS_SHOT_SIZE` / `NFS_SHOT_SCALE` ([`shot_geometry`]). The
    /// original's rules (0x41E480 anchoring, 0x41FB40, native-pixel sizes and fonts, the centred
    /// loading picture) run on the **logical** resolution `physical / scale` ([`App::hud_logical`]);
    /// the sprites are fractions of the target, so they land at `× scale` physical pixels. At scale 1
    /// that is the original to the pixel. A font is the native bitmap font drawn `scale` times
    /// as large (its glyph quads are simply scaled; for an integer scale that is a nearest-neighbour
    /// enlargement if the renderer samples nearest, otherwise the sampler's filter — APPROXIMATION
    /// for fractional scales, which the original has no analogue for).
    pub(crate) fn hud_geometry(&self) -> (u32, u32, f32) {
        if let Some(t) = self.shot_target {
            return t;
        }
        let (w, h) = self.renderer.as_ref().map_or((640, 480), |r| r.size());
        (w, h, self.window_scale())
    }

    /// The window's scale factor; while `start_race` has the window taken out of the `App` the last one seen
    /// (the loading screen of a race start, LOADSEAM: it fell back to 1 and 640x480 and drew the picture
    /// stretched over the real target with black gaps between the tiles).
    fn window_scale(&self) -> f32 {
        if let Some(w) = self.window.as_ref() {
            self.hud_scale.set(w.scale_factor() as f32);
        }
        self.hud_scale.get().max(1.0)
    }

    /// [`App::hud_geometry`] for a renderer that is not in `self.renderer` (the one `start_race` holds locally
    /// through the whole load): that renderer's surface, the window's scale.
    pub(crate) fn hud_logical_on(&self, r: &Renderer) -> (u32, u32) {
        let (w, h, s) = self.shot_target.unwrap_or_else(|| {
            let (w, h) = r.size();
            (w, h, self.window_scale())
        });
        (((w as f32 / s).round() as u32).max(1), ((h as f32 / s).round() as u32).max(1))
    }

    /// The HUD's logical resolution in pixels, `W = physical W / scale`, `H = physical H / scale`
    /// (see [`App::hud_geometry`]).
    pub(crate) fn hud_logical(&self) -> (u32, u32) {
        let (w, h, s) = self.hud_geometry();
        (((w as f32 / s).round() as u32).max(1), ((h as f32 / s).round() as u32).max(1))
    }

    pub(crate) fn rebuild_scene(&mut self) {
        let (lw, lh) = self.hud_logical();
        Hud::set_screen(lw, lh);
        self.hud_stored_sync(lw, lh); // N01a: an existing HUD record is used as stored
        self.fix_mission_result();
        // the port's own overlays lay their text out in screen units, so the fonts need to know
        // what shape the window is (the HUD and the front end place glyphs in 640×480 themselves)
        let a = self
            .renderer
            .as_ref()
            .map(|r| r.aspect())
            .unwrap_or(HUD_REF_W / HUD_REF_H);
        if let Some(h) = self.hud.as_mut() {
            for f in [&mut h.font, &mut h.font_small, &mut h.font_italic].into_iter().flatten() {
                f.set_aspect(a);
            }
        }
        if let Some(f) = self.fe.menu.as_mut().and_then(|m| m.font.as_mut()) {
            f.set_aspect(a);
        }
        if let Some(w) = self.world.as_mut() {
            for a in self.world_actions.drain(..) {
                match a {
                    WorldAction::Show(all, shown) => w.set_shown(&all, &shown),
                }
            }
        }
        self.scene.items.clear();
        self.scene.lens_probe = None;
        self.scene.sprites.clear();
        self.scene.mirror = self.mirror && self.vehicle.is_some();
        // the sky's own colour, from the track's `Sky/<Track>.bin` (`RE/RENDERING.md`): black on the
        // Monaco circuits, sunset on coastal, plain blue on forest. It shows wherever the sky layers
        // do not cover, and the layers are drawn over it.
        // The record is the `SKYRENDERING` level's (`[0x657D9C]`, the `Graphics.SkyRendering`
        // choice): 0x459970 addresses `bin + 0xCE8 + 60·level` and steps down past unused records
        // (0x459983…0x45998E). The port read level 0 whatever the option said.
        let sky_level = self.sky_level();
        // 0x459A70 at the race pass's start (0x468C63): the record's gradient flag sends the
        // colour to the sky manager's track B at the camera's slice, else track A blends the
        // record's top and bottom (`SkyBin::background_at`), group A — the main view
        let cam_slice = self.cam_slice.or(self.race.as_ref().map(|r| r.slice)).unwrap_or(0) as i32;
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        if self.vehicle.is_some()
            && let Some(c) = self.track.sky_bin.as_ref().and_then(|b| b.background_at(sky_level, 0, cam_slice, lap))
        {
            self.scene.clear_color = [c[0] as f64, c[1] as f64, c[2] as f64, 1.0];
        }
        // the weather fog the sky manager sets at the camera's slice: the sky pass's own (0x45D2E0)
        // and, as that pass ends, the scene's (0x45D3B0) over everything drawn after it
        // (`SkyBin::fog_at`); the front end has none
        let fog = |sky: bool| {
            if self.vehicle.is_none() {
                return None;
            }
            let (colour, density) = self.track.sky_bin.as_ref()?.fog_at(sky, cam_slice, lap)?;
            Some(nfs_renderer::Fog { colour, density })
        };
        (self.scene.sky_fog, self.scene.fog) = (fog(true), fog(false));
        if self.vehicle.is_none() && self.fe.menu.is_some() {
            self.scene.clear_color = [0.0, 0.0, 0.0, 1.0];
            self.load_fe_room();
            self.fe_room_items();
            // a screen with a room behind it must not paint its own black fill over it
            let room = !self.fe.room.is_empty() || !self.fe.cars.is_empty();
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_overlay(room);
            }
            // the background drawn is the bottom layer's (`fe_sprites`), so its fill
            // gives way to the room the same way
            if let Some((_, u)) = self.fe.layout_under.first_mut() {
                u.set_overlay(room);
            }
            self.scene.sprites = self.fe_sprites();
            if let Some(step) = self.fe.loading_still {
                self.scene.items.clear();
                self.scene.sprites = self.fe_loading_sprites(step);
            }
            if let Some(l) = self.fe.startup_still.as_ref() {
                let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
                self.scene.items.clear();
                self.scene.sprites = l.mosaic_sprites(aspect);
            }
            if let Some(step) = self.fe.race_loading_still
                && let Some(mut r) = self.renderer.take()
            {
                for k in 0..=step.clamp(0, 9) {
                    self.race_loading_step(&mut r, k);
                }
                self.scene.items.clear();
                let (lw, lh) = self.hud_logical();
                self.scene.sprites = self.fe.race_loading.as_ref().map(|l| l.sprites(lw as f32, lh as f32)).unwrap_or_default();
                self.renderer = Some(r);
            }
            return;
        }
        let (hud_sprites_from, mut hud_hover) = self.hud_pass();
        // render pose = previous tick → current tick, by the accumulator fraction
        let mut alpha = (self.accumulator.as_secs_f32() / GAME_TICK.as_secs_f32()).clamp(0.0, 1.0);
        // **TEST GLUE** (not in the original): `NFS_ALPHA=<0…1>` fixes the interpolation fraction, so
        // a scripted run (one frame per tick, where the accumulator reads 0) can draw the frames a
        // windowed run draws between two ticks — `--autotest 20` with `NFS_ALPHA` 0, 0.25, 0.5, …
        // are the sub-tick frames of one tick
        if let Some(a) = self.test_alpha.or_else(|| nfs_game::config::cfg().nfs_alpha.var().ok().and_then(|v| v.parse::<f32>().ok())) {
            alpha = a.clamp(0.0, 1.0);
        }
        // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: every moving thing is drawn at `lerp(previous
        // tick, this tick, alpha)` (cars, camera, shadows, smackables, `WAnimBundle` articles); the
        // original draws this tick's pose (alpha 1) — see `RE/notes/PORT-DEVIATIONS.md` D8
        let (car_pos, car_rot) = match self.render_prev {
            Some((p0, r0)) if p0.distance(self.car_pos) < 5.0 => (
                p0.lerp(self.car_pos, alpha),
                r0.slerp(self.car_rot, alpha).normalize(),
            ),
            _ => (self.car_pos, self.car_rot),
        };
        // `self.car_pos` is where the body is drawn, `car+0x330 − M[1]·car+0x3AC` (0x43A410 at
        // 0x4434D7); the cameras (0x461903), the headlight projector (0x452749), the beacon and
        // the shadow (0x43B220) hang off `car+0x330` itself, half the model box higher
        let car_draw = car_pos;
        // the viewed car's own lift (a network replay may view another human's car)
        let viewed_seat = Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view);
        let viewed_lift = match viewed_seat {
            Some(k) => self.road.opponents.get(k).map_or(0.0, |o| o.vehicle.body_lift),
            None => self.vehicle.as_ref().map_or(0.0, |v| v.body_lift),
        };
        let car_pos = car_pos + car_rot * Vec3::Y * viewed_lift;
        // the recorded machine's own car, slot 0 of the draw: where it is, whichever car the camera views
        // (`car_pos` / `car_rot` are the camera's car's)
        let (local_draw, local_rot) = if viewed_seat.is_some() {
            match self.local_prev {
                Some((p0, r0)) if p0.distance(self.local_pose.0) < 5.0 => (p0.lerp(self.local_pose.0, alpha), r0.slerp(self.local_pose.1, alpha).normalize()),
                _ => self.local_pose,
            }
        } else {
            (car_draw, car_rot)
        };
        let local_pos = local_draw + local_rot * Vec3::Y * self.vehicle.as_ref().map_or(0.0, |v| v.body_lift);
        // **The headlight projector** 0x4526C0, rebuilt each frame from the player car: world into
        // the car's frame (0x466280 / 0x466220 from `car+0x364` / `+0x330`), its y shifted by
        // `[0x5CEC54]` = −4.8 (0x452749), then turned −17° about x (`[0x5CEC5C]` through 0x539BD0
        // at 0x452769, the row-vector product 0x539AD0). The car's lamp bit `car+0xD87 & 1` starts
        // as the night flag (`Car_init` 0x410C74) and the lights key toggles it (`lamp_command`).
        // The vertex light runs the projection only while lamp 0 **or** 1 of the camera's car
        // (`0x5E8B34[[0x65741C]]+0x74C`) is lit (0x453177…0x4531BC, else the colours pass
        // through, 0x453516), and the `HEAD` decal only while lamp **0** is (0x453CA3) — so damage
        // over 30 in zones 0 and 2 (0x4398C6…0x439924) puts the beam out. *(Until 2026-09-26 the
        // port lit it on the night flag alone.)*
        let lamps = Self::lamp_slots(self.lamp_bits & 1 != 0, 1.0, false, false, (false, false, false), self.vehicle.as_ref().map(|v| &v.damage));
        self.scene.headlight_decal = lamps[0].0;
        self.scene.headlight = (self.vehicle.is_some() && (lamps[0].0 || lamps[1].0)).then(|| {
            Mat4::from_rotation_x((-17.0f32).to_radians())
                * Mat4::from_translation(Vec3::new(0.0, -4.8, 0.0))
                * Mat4::from_rotation_translation(car_rot, car_pos).inverse()
        });
        let fwd = car_rot * Vec3::Z;
        // diagnostic: `NFS_LOOKBACK=1` holds look-back (`I_LOOKBEHIND`) for a `--screenshot`
        let rear_view = (self.input.held(Action::RearView) || nfs_game::config::cfg().nfs_lookback.var().is_ok())
            && self.vehicle.is_some();
        // **The rear-view mirror** (AUDIT §O): the pass 0x468CA0 renders the view 0x4691D0 made — near
        // 1, far 65000 (the projection; the scene stops at the cull distance below), cleared 0xFF77BBFF — with camera slot 0 in its type-1 form (0x464F80's branch at
        // 0x46503B): the eye at the car's position raised by the current view's height (`[0x5CF614 +
        // 16·view]`, y only), the car's matrix with its right and forward rows negated — a half turn
        // about the car's up axis, not a left-right mirror. Skipped in a replay (`[0x657424]` 2/3) and
        // unless the tray is in (0x4275A0); `REARVIEWMIRRROR` gates the tray (0x41E9A0). The field of
        // view is the mirror view's own `+0x130`, 45.0 from its constructor (0x4673D5,
        // `camera::VIEW_HALF_ANGLE`): the build 0x464F80 writes slot 0's `+0x58` / `+0x5C` into the
        // scene manager's first view `[[0x628140] + 4]` (0x464F91), the main one, even for this type-1
        // build — so no zoom and no wide-screen ×1.2 here. The projection 0x5391B0 takes `2·+0x130`
        // as the **horizontal** angle with the viewport's aspect in 640 × 480 units (0x466FD7…
        // 0x46700C): 90° across the 240 × 84 tray, `2·atan(84/240)` high. The pass's view distances (read
        // 2026-09-24, 0x468D3B…0x468D82): `+0x120` = 200 (the main view's 0x468BF8 has 400), `+0x124` =
        // `max([0x6197CC]/3, 50)` (`[0x5B2680]`, `[0x5B251C]`; the main view the whole draw distance)
        // and `+0x128` its square. The one reader of the view's `+0x124` is 0x47FB57 → 0x47F550, which builds
        // the view's footprint on the ground out to that distance (`+0xA0` = its square) to pick the track
        // sections to draw — the draw distance, which the mirror's own walk takes (`render_mirror`);
        // its far plane is the view's own 65000 (0x4691D0). (0x499D9C / 0x49AEE0 / 0x49C96A read
        // `+0x124` off `car+0x760`, another struct.) The 0x537890 state is not carried.
        // The pass's gates, read 2026-09-27 (0x468CA0…0x468CD1): `REPLAY` neither 2 nor 3, 0x4275A0
        // (`[0x60C970]` and the mirror element in state 3 — REARVIEWMIRRROR opens it,
        // `mirror_slide`) and the view `[0x628144]` made (its one writer, 0x469225, is the view's
        // creation). *(Until 2026-09-27 the port tested `Playing` only and stopped the view at the
        // player's finish, which nothing in the pass or its gates does.)*
        let playback_now = self
            .replay
            .stream
            .as_ref()
            .is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        self.rear_mirror = (self.hud_keys.live
            && self.mirror_slide.0 == 3
            && self.vehicle.is_some()
            && !playback_now
            && self.race.is_some())
        .then(|| {
            let h = nfs_game::camera::VIEW_OFFSETS.get(self.camera_view() as usize).map_or(0.0, |o| o.y);
            let position = car_pos + Vec3::new(0.0, h, 0.0);
            Camera { position, target: position - fwd * 20.0, up: car_rot * Vec3::Y, fov_y_radians: nfs_game::camera::fov_y_viewport(nfs_game::camera::VIEW_HALF_ANGLE, 240.0, 84.0), near: 1.0, far: 65000.0 }
        });
        // the horn is a loop on the car's own engine bank (`engine-audio.md` §6)
        let horn_view = self.camera_view();
        let to_car = self.car_pos - self.camera.position;
        let horn_dist = to_car.length();
        // the pan is 0x4A7660's `ebp` (0x4A7CAB → `Horn_start`'s fourth argument → `Voice_play`'s
        // pan, 0x4AACD8): 0 on view 0 (0x4A7855) and negated under `MIRROR` (0x4A78C1) — the port
        // had neither until 2026-09-26
        let horn_pan = if horn_view == 0 { 0 } else { listener_pan_of(self.camera.position, self.camera.target, self.mirror)(to_car) };
        let sfx_level = self.snd.audio_settings.sfx_level;
        // the horn is 0x4A7660's (0x4A7CBF / 0x4A7CE6), part of the audio update that does not run
        // while paused: a held horn is not released by the pause but silenced with the voice table
        // (0x4AB150), and plays on at volume 0 after the resume until it is let go
        if let (Some(a), Some(e)) = (&self.snd.audio, self.snd.engine_sound.as_mut())
            && !(self.paused || self.replay.frozen)
        {
            e.horn(
                a,
                self.input.held(Action::Horn) && self.vehicle.is_some(),
                horn_view,
                horn_dist,
                horn_pan,
                sfx_level,
            );
        }
        if !self.paused
            && self.replay_save.up
            && let Some(h) = &self.hud
            && h.font.is_some()
        {
            let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
            let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(HUD_REF_W / HUD_REF_H);
            let mut dlg = Vec::new();
            let pressed = self.fe.mouse_down;
            h.billboard_h.set(BILLBOARD.3);
            if self.replay_save.overwrite {
                // `HYesNoOverlay` 0x437440 ("" | 18174) with 0x4376B0(18139, 18140)
                let hover = Hud::yes_no_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor));
                hud_hover = Some(hover.map(|k| (4, k)));
                let bb = Billboard { left: "", right: &fe(18174), hints: None };
                h.yes_no_sprites(&mut dlg, aspect, &bb, [&fe(18139), &fe(18140)], [&fe(18141), &fe(18142)], hover.unwrap_or(usize::MAX), pressed && hover.is_some());
            } else {
                let hover = Hud::replay_save_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor));
                hud_hover = Some(hover.map(|k| (3, k)));
                let bb = Billboard { left: "", right: &fe(18136), hints: None };
                // the caret flips each time `[0x606AC8]` is 16 past the last flip — every 17 ticks
                let t = self.replay_save.opened.map_or(0, |o| (o.elapsed().as_secs_f64() * 64.0) as u64);
                let caret = (t / 17) % 2 == 0;
                h.replay_save_sprites(&mut dlg, aspect, &bb, &fe(18170), &self.replay_save.name, caret, [&fe(18137), &fe(18138)], hover, pressed && hover.is_some());
            }
            self.scene.sprites.extend(dlg);
        }
        if self.paused
            && let Some(h) = &self.hud
            && h.font.is_some()
        {
            // `HPauseMenu` 0x42A440: title 18128 "Pause Menu" (0x46D0) over three buttons, 18108
            // continue / 18109 race again / 18110 quit; the quit question is `HYesNoOverlay`
            // 0x437440, 18174 "Confirmation" with 18171 and 18141 yes / 18142 no. The strings and their
            // places are the original's (`pause_sprites`, `yes_no_sprites`; hud.md).
            let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
            // the pointer over a button: its hover look, and the port's keyboard selection moves
            // there so only one button is lit (the original has no keyboard selection)
            let hover = match self.pause_confirm {
                None => Hud::pause_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor)).map(|k| (0, k)),
                Some(_) => Hud::yes_no_buttons().iter().position(|b| Hud::over_button(*b, self.fe.cursor)).map(|k| (1, k)),
            };
            hud_hover = Some(hover);
            match hover {
                Some((0, k)) => self.pause_sel = k as u8,
                Some((_, k)) => self.pause_confirm = Some(k as u8),
                None => {}
            }
            let pressed = hover.is_some() && self.fe.mouse_down;
            match self.pause_confirm {
                None => {
                    let mut menu = Vec::new();
                    let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(HUD_REF_W / HUD_REF_H);
                    // 0x42A477: "" left, 18128 right, no hints
                    let bb = Billboard { left: "", right: &fe(18128), hints: None };
                    // `[0x606AC8]` keeps running through the pause (see `pause_ticks`)
                    h.billboard_clock.set((self.hud_clock(), 0));
                    h.pause_sprites(&mut menu, aspect, &bb, &[fe(18108), fe(18109), fe(18110)], self.pause_sel as usize, pressed);
                    self.scene.sprites.extend(menu);
                }
                Some(sel) => {
                    let mut dlg = Vec::new();
                    let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(HUD_REF_W / HUD_REF_H);
                    let bb = Billboard { left: "", right: &fe(18174), hints: None };
                    // `[0x606AC8]` keeps running through the pause (see `pause_ticks`)
                    h.billboard_clock.set((self.hud_clock(), 0));
                    h.yes_no_sprites(&mut dlg, aspect, &bb, [&fe(18171), ""], [&fe(18141), &fe(18142)], sel as usize, pressed);
                    self.scene.sprites.extend(dlg);
                }
            }
        }
        // `HInfo`'s in-race box (0x4282F0; `net_race_notice`)
        if !self.paused
            && let Some(id) = self.net_notice
            && let Some(h) = &self.hud
            && h.font.is_some()
        {
            let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
            let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(HUD_REF_W / HUD_REF_H);
            let mut dlg = Vec::new();
            h.billboard_h.set(BILLBOARD.3);
            // 0x428328: "" left, 18157 right, the Esc hint alone (Spacebar byte 0)
            let space = self.net_notice_space.then(|| fe(18101));
            let bb = Billboard { left: "", right: &fe(18157), hints: Some((&fe(18100), space.as_deref())) };
            h.notice_sprites(&mut dlg, aspect, &bb, &fe(id));
            self.scene.sprites.extend(dlg);
        }
        // every string still pending is an unanchored element's: its pen is its own pixel
        Hud::resolve_text(&mut self.scene.sprites[hud_sprites_from..], (0.0, 0.0));
        PENDING_TEXT.with(|p| p.borrow_mut().clear());
        if self.hud_hidden {
            self.scene.sprites.truncate(hud_sprites_from);
        }
        // `+0x100` for the next release, and sound 9 when the pointer crosses into a button
        // (0x41B3BA…0x41B3C6)
        let hover = hud_hover.flatten();
        if hover.is_some() && hover != self.hud_hover {
            self.fe_sound(9);
        }
        self.hud_hover = hover;
        let (view, slot_near) = self.camera_pass(car_pos, car_rot, rear_view);
        // draw distance from `<track>_{forward,backwards}.key` for the camera's slice, scaled by the
        // detail setting (0x44F1A0: `value × {0.5, 0.67, 0.83, 1.0}`, at least 50 m). The sky is drawn
        // around the camera, so distant terrain simply stops at this range as in the original.
        let keys = if self.backwards {
            &self.track.draw_keys_backward
        } else {
            &self.track.draw_keys_forward
        };
        // 0x4689C0 keys on the camera slot's own tracker slice `[0x627F5C]`, which the dispatcher moves
        // with the eye every update (0x474440 at 0x463C61, except in SplineCam, which runs it itself)
        // diagnostic, no original counterpart: `NFS_WEATHER_VIEW=<slice>` puts the camera 1.5 m over
        // that slice's centre looking down the road, and `weather_items` then runs the weather 40
        // frames of a tick each, so a still shows it built up
        if let Some(k) = nfs_game::config::cfg().nfs_weather_view.var().ok().and_then(|v| v.parse::<usize>().ok())
            && let Some(sl) = self.slices.as_ref().and_then(|t| t.slices.get(k))
        {
            self.camera.position = sl.centre + sl.up * 1.5;
            self.camera.target = self.camera.position + sl.forward;
            self.cam_slice = Some(k);
        }
        let car_slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
        let cam_slice = match self.slices.as_ref() {
            Some(t) if !t.slices.is_empty() => t.find_slice(self.camera.position, self.cam_slice.unwrap_or(car_slice)),
            _ => car_slice,
        };
        self.cam_slice = Some(cam_slice);
        // the near plane: the camera object's 0.15 (0x4677F5) is only the main view's creation value
        // (0x468E6A) — the build 0x464F80 overwrites the view's `+0x18` every frame with the slot's
        // `+0x5C`, less 0.35 in view 1 (0x465413…0x465439): 0.5, the driver view 0.15, a TV zoom
        // further out (`camera::near_plane`). The far plane is the camera object's 700 (0x4677FC,
        // `RE/notes/thrash.md` §6); the draw distance `[0x6197CC]` is the walk's (the view's
        // `+0x124`: the footprint and the non-mid-ground articles' distance rule, `RE/RENDERING.md`
        // "The track draw's culling"), not a clip plane. *(Until 2026-09-26 the port clipped at the
        // draw distance, cutting the mid-ground's 600 / 700 m short.)*
        self.camera.near = nfs_game::camera::near_plane(slot_near, view);
        self.draw_distance = keys.far_distance(cam_slice, self.detail);
        self.camera.far = if self.vehicle.is_some() { nfs_renderer::VIEW_FAR } else { 20000.0 };
        self.sun_view_diagnostic();
        self.main_sky_items(cam_slice);
        self.camera_diagnostics();
        self.track_walk(alpha);
        self.track_and_car_items(alpha, local_draw, local_pos, local_rot);
    }

    /// The sky manager's update 0x45D030 for the main view and its draw items (the sky, the sun,
    /// the ring): the first range of `scene.items` (`sky_item_range`).
    fn main_sky_items(&mut self, cam_slice: usize) {
        // `[0x657DB4]` SKYOFF (the config row `SKYOFF`, id 0x5A, 0x4B14C4 — `fe.txt`'s `skyoff`, absent
        // in the shipped one): 0x468AF9 skips the whole sky pass 0x45D7D0, update and draw, for every
        // view; `[0x619799]`, the third gate there, is only ever written 0 (0x44E89D)
        if self.sky_off() {
            let n = self.scene.items.len();
            self.sky_item_range = (n, n);
            return;
        }
        // the sky manager's update (0x45D030): the camera's eye and slice drive the layers
        if let (Some(s), Some(r)) = (self.sky.as_mut(), self.renderer.as_mut()) {
            let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
            s.update(r, self.camera.position, cam_slice as i32, lap, self.tick, &mut self.fe_rng);
        }
        if let Some(s) = &self.sky {
            // in metres around the eye, clipped by the far plane 700 as the sky pass 0x45D4F0 does
            // (`Sky::draw_items`); it draws first without a depth test, so the mid-ground shows over
            // it — the levels that let the sun through, the sun, then the rest and the ring
            let aspect = self.renderer.as_ref().map_or(1.6, |r| r.aspect());
            let sun = self.sun_items(&self.camera, aspect, SUN_VIEW_MAIN);
            let view = nfs_game::sky::SkyView { eye: self.camera.position, forward: (self.camera.target - self.camera.position).normalize_or_zero(), up: self.camera.up, main: true };
            let start = self.scene.items.len();
            self.scene.items.extend(s.draw_items(&view, false));
            self.scene.items.extend(sun);
            self.scene.items.extend(s.draw_items(&view, true));
            self.sky_item_range = (start, self.scene.items.len());
        }
    }

    /// `[0x657DB4]`, `SKYOFF`: set from the config (`fe.txt`), 0 as shipped.
    pub(crate) fn sky_off(&self) -> bool {
        self.registry.u32("skyoff", 0) != 0
    }

    /// The port's `NFS_SUN_VIEW` camera diagnostic (no original counterpart), run before the sky's
    /// items are laid out so that the sun is placed for the turned camera.
    fn sun_view_diagnostic(&mut self) {
        // diagnostic: `NFS_SUN_VIEW=down,yaw` (degrees, the yaw optional) turns the camera, where it
        // stands, to the sun (`SkyBin::sun_position`, around the eye) lowered by `down` and turned
        // about the vertical by `yaw`, so a still has the sun in front of it — and the lens flare,
        // whose elements lie on the line from the screen's centre through the sun, off the centre
        if let Some((deg, yaw)) = nfs_game::config::cfg().nfs_sun_view.var().ok().and_then(|v| {
            let mut it = v.split(',').map(|x| x.trim().parse::<f32>().ok());
            Some((it.next()??, it.next().flatten().unwrap_or(0.0)))
        }) && let Some(bin) = self.track.sky_bin.as_ref()
        {
            let d = Vec3::from(bin.sun_position()).normalize_or_zero();
            let aim = Quat::from_rotation_y(yaw.to_radians()) * (d - Vec3::Y * deg.to_radians().tan()).normalize_or_zero();
            if aim != Vec3::ZERO {
                self.camera.target = self.camera.position + aim * 10.0;
            }
        }
    }

    /// The port's `NFS_*` camera diagnostics (no original counterpart): each moves the camera before
    /// the track walk culls from it.
    fn camera_diagnostics(&mut self) {
        // diagnostic: `NFS_EMITTER_VIEW=k` puts the camera 6 m off the track's k-th particle
        // emitter, looking at it (before the track walk below, which culls from it)
        if let Some(k) = nfs_game::config::cfg().nfs_emitter_view.var().ok().and_then(|v| v.parse::<usize>().ok())
            && let Some(e) = self.world.as_ref().and_then(|w| w.emitters.get(k))
        {
            self.camera.target = e.position;
            self.camera.position = e.position + e.dir_a * 6.0 + Vec3::Y * 1.5;
        }
        // diagnostic: `NFS_ANIM_VIEW=<metres>` puts the camera that far off the track's
        // SoundDef-animated object (autobahn's Locomotive, `world.animated_sound`), 8 m up,
        // looking at where its own path puts it this tick
        if let Some(d) = nfs_game::config::cfg().nfs_anim_view.var().ok().and_then(|v| v.parse::<f32>().ok())
            && nfs_game::config::cfg().nfs_anim_article.var_os().is_none()
            && let Some(s) = self.world.as_ref().and_then(|w| w.animated_sound.as_ref())
        {
            let (p1, _) = s.eval_pair(self.tick);
            let p = Vec3::from(p1);
            self.camera.target = p;
            self.camera.position = p + Vec3::new(d, 8.0, d * 0.5);
        }
        // diagnostic: `NFS_CAM_AT=px,py,pz,tx,ty,tz` puts the camera at a world point looking at
        // another (before / after comparisons at a fixed place)
        if let Ok(v) = nfs_game::config::cfg().nfs_cam_at.var() {
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if n.len() == 6 {
                self.camera.position = Vec3::new(n[0], n[1], n[2]);
                self.camera.target = Vec3::new(n[3], n[4], n[5]);
            }
        }
        // diagnostic: `NFS_ANIM_ARTICLE=<name>` (with `NFS_ANIM_VIEW=<metres>`, default 2.5 radii)
        // puts the camera off the named `WAnimBundle` article's current sphere (0x477DB0), looking
        // at it — any article of any track, `world.anim_article_sphere`
        if let Ok(name) = nfs_game::config::cfg().nfs_anim_article.var()
            && let Some((_, c, r)) = self.world.as_ref().and_then(|w| w.anim_article_sphere(&name))
        {
            let d = nfs_game::config::cfg().nfs_anim_view.var().ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.5 * r);
            self.camera.target = c;
            self.camera.position = c + Vec3::new(d, d * 0.4, d * 0.5);
        }
        // diagnostic: `NFS_SMACK_VIEW=k` puts the camera 8 m off the track's k-th smackable's
        // place, looking at it (its burst stays there: 0x450980 re-seats its system at the
        // placement every drawn frame, slot 4 0x4550B0)
        if let Some(k) = nfs_game::config::cfg().nfs_smack_view.var().ok().and_then(|v| v.parse::<usize>().ok())
            && let Some((_, home, _)) = self.world.as_ref().and_then(|w| w.track_smackable(k))
        {
            self.camera.target = home + Vec3::Y * 1.0;
            self.camera.position = home + Vec3::new(6.0, 2.5, 5.0);
        }
        // **TEST GLUE** (not in the original) `NFS_TRAFFIC_VIEW=k[,d]`: the camera `d` m (default 7) off the k-th active
        // traffic car's body, looking at it — close stills of the traffic's ride height
        if let Ok(v) = std::env::var("NFS_TRAFFIC_VIEW") {
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            let k = n.first().copied().unwrap_or(0.0) as usize;
            let d = n.get(1).copied().unwrap_or(7.0);
            if let Some(c) = self.road.traffic.as_ref().and_then(|t| t.cars.iter().filter(|c| c.active).nth(k)) {
                let o = c.draw_origin();
                self.camera.target = o + Vec3::Y * 0.5;
                self.camera.position = o + c.orientation.row(0) * d + Vec3::Y * 1.0;
            }
        }
        // diagnostic: `NFS_FLARE_VIEW=k` puts the camera 8 m off the k-th `.scn` instance flare,
        // looking at it
        if let Some(k) = nfs_game::config::cfg().nfs_flare_view.var().ok().and_then(|v| v.parse::<usize>().ok())
            && let Some((p, d)) = self.world.as_ref().and_then(|w| w.instance_flare(k))
        {
            let back = if d.length_squared() > 1e-6 { d.normalize() } else { Vec3::X };
            self.camera.target = p;
            self.camera.position = p + back * 8.0 + Vec3::Y * 1.5;
        }
    }

    /// The main view's track walk 0x47F890 and what the walk's draws feed (smackables, projectors,
    /// the kind-3 emitters): `article_draws` for this frame.
    fn track_walk(&mut self, alpha: f32) {
        if let Some(w) = self.world.as_mut() {
            // each smackable's article is drawn at its body's pose (0x483590, `skill-tests.md` §5.3)
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: at the pose blended by `alpha` between the
            // previous tick and this one, like every car (the original draws this tick's, alpha 1)
            for (owner, pose) in self.smackables.poses_blended(alpha) {
                w.place_smackable(owner, pose);
            }
            // the track draw's prep 0x488CC0 → 0x476810: a smackable a draw found pushed gets its
            // record's `+8` (`suspension-collision.md` §9 "The track's smackables")
            w.smackables_prep();
            // the looping props (`RE/notes/crp-geometry.md` §7) walk on the race clock
            // 0x488CC0 heads every walk (0x47FB49) and counts them in `[0x628C4C]`, never reset: the
            // eighth call re-registers the moving refs; the main view's walk is one
            // count, the mirror's (`render_mirror`) the next
            self.walk_calls = self.walk_calls.wrapping_add(1);
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the race time of the drawn pose — this tick's
            // `tick / 64` at `alpha` 1 (all the original draws), a fraction of a tick earlier
            // for a frame between two ticks, as the cars are blended; this tick's time at tick 0
            w.animate((self.tick as f32 - 1.0 + alpha).max(0.0).min(self.tick as f32) / 64.0, self.walk_calls & 7 == 0);
            // water/flag UV scroll (`RCustomLayers::LayerUVAnimate`, the EAGL
            // layers): `TIMER_getTicks()` is two of this 64 Hz race tick, as the beacon uses below
            w.animate_uv((self.tick * 2) as u32);
            // the breakable props' pieces (`RE/notes/suspension-collision.md` §9)
            w.break_props(self.tick as u32);
            // the track walk 0x47F890 over the articles that carry `ef` parts (`RE/RENDERING.md`
            // "The track draw's culling"), with the eye, the view's axes, the half-angles' tangents
            // of the port's own frustum, its near plane and the draw distance; the projectors of
            // the drawn articles go into the map `[0x627DFC]` for good (0x450DBD → 0x456090)
            let forward = (self.camera.target - self.camera.position).normalize_or_zero();
            let right = forward.cross(self.camera.up).normalize_or_zero();
            let tan_v = (self.camera.fov_y_radians * 0.5).tan();
            let view = nfs_game::world::CullView {
                eye: self.camera.position,
                forward,
                right,
                up: right.cross(forward).normalize_or_zero(),
                tan_h: tan_v * self.renderer.as_ref().map_or(4.0 / 3.0, |r| r.aspect()),
                tan_v,
                near: self.camera.near,
                draw: self.draw_distance,
            };
            w.article_draws_into(&view, &mut self.article_draws);
            w.register_projectors(&self.article_draws);
            // 0x450010's entry loop samples each drawn smackable's player (0x4711E0)
            w.smackables_sample(&self.article_draws);
            // and kind 3: each drawn emitter's system made or kept alive with its article's value,
            // and triggered where 0x450980 runs it (a struck item's)
            if let Some(p) = self.track_particles.as_mut() {
                p.draw(self.draw_distance, |e| w.effect_run(&self.article_draws, e.article, e.part, 3));
            }
        }
    }

    /// The track's near list, the cars and the skid marks with the track's far list in one
    /// farthest-first run, then the effects and the weather: the rest of `scene.items`.
    fn track_and_car_items(&mut self, alpha: f32, local_draw: Vec3, local_pos: Vec3, local_rot: Quat) {
        // The track's near list (`+4`, flag-2 geometry, nearest first) and its own `+0xC` entries
        // (flag-1 / flag-4 parts, instance items, a breakable's pieces) are kept apart here now: the
        // original's `+0xC` also holds the cars (0x4430A0 → 0x443229) and the skid marks (0x44FFC0),
        // one distance-sorted list flushed farthest first (0x466660, `RE/RENDERING.md` "The view's
        // lists"), where the port drew three separate blocks until this reading
        // (`RE/notes/thrash.md` §6d.6). `track_far` and `track_rest` are folded into `blended`
        // below, once the cars' and the skid marks' own keyed entries have joined it, and pushed as
        // one farthest-first run after `track_near`. The mirror's pass builds its own cars' and skid
        // marks' entries for its eye (`Self::dyn_items`, called from `Self::render_mirror`).
        self.blended_dyn_items.clear();
        // (the lists, the sort's pairs and the merge's buffer are `self.scratch`'s, kept between frames; the
        // near list is sorted as `(key, index)` pairs — a stable ascending order, the index breaking ties —
        // and only then are the items copied out, instead of moving the whole items in a stable sort)
        let mut sc = std::mem::take(&mut self.scratch);
        let track_range_start = self.scene.items.len();
        if let Some(w) = &self.world {
            w.track_draw_lists_into(&self.article_draws, self.camera.position, &mut sc.lists);
            nfs_renderer::sort_order(&sc.lists.near, false, &mut sc.order);
            let near = &sc.lists.near;
            self.scene.items.extend(sc.order.iter().map(|&(_, i)| nfs_renderer::DrawItem { early: true, layer_light: Some(nfs_game::world::TRACK_LIGHT), ..near[i as usize].1 }));
            tracing::trace!(articles = self.article_draws.iter().flatten().count(), items = self.scene.items.len() - track_range_start, draw = self.draw_distance, "track walk");
            self.scene.ambient = w.ambient;
        } else {
            (sc.lists.near.clear(), sc.lists.far.clear(), sc.lists.rest.clear());
        }
        // The cars, in the order of the original's car list `[0x6115F0]`, which 0x443610 walks at
        // 0x468B2F (through `+0x474`) calling each `R3DCar`'s draw 0x4430A0: its shadow (0x437AE2 →
        // 0x43B820), its body and then its effect pass 0x43D800 — the lamps' glares and beams
        // (`lamp_draws`). The list is built by `R3DObject`'s constructor 0x4414C0, which puts every
        // new car at its **head** (0x4416E0…0x4416EB, the old head its `+0x474`), and the cars are
        // made in slot order: `Car_installHandlers` 0x410D60 builds the `R3DCar` (0x410F08) and is
        // called for `0x5E8B34[i]`, `i < [0x658080]` (0x41127C…0x41135C), whose order is
        // `Cars_create`'s over the racer slots (0x411130, `Car_register` 0x40F160 appending each),
        // and `Race_setCars` 0x4ECF00 lays the slots out as the player, the opponents, the cops,
        // then the traffic (`modes.md` §1). So the list — and the draw — runs the other way: the
        // traffic from its last slot, the cops, the opponents from the last, the player last. (The
        // world props that follow, 0x411370…, carry `+4 == 1` and return at 0x410EB6 before any
        // `R3DCar` is made.) MEDIUM: each of the port's traffic and police vectors is taken to be in
        // its slots' order. *(Until 2026-09-26 the port drew police, traffic, opponents, player,
        // and laid every shadow after the AI cars' lamps.)*
        //
        // The headlights' ramp runs from the race's first tick: the flag `+0x567` is taken as clear
        // when the car is built (its constructor is not read), so the first lamp update at night
        // starts it from 0. It runs on `GetTickCount` (0x53BFD0), which a pause does not stop —
        // the port's wall clock `tick + pause_offset + pause_ticks` (64 Hz units) does the same
        // *(until 2026-09-26 the world tick, which a pause halts)*; every car's first lamp update
        // is the same frame, so the traffic and the police share the race's ramp. Only a car with
        // pop-up headlights takes it (`CarModel::has_headlight_extent`, 0x439627): with
        // `R3DCar+0x70` = 0 the slots are 1.0 from the first frame (0x43972D) *(until 2026-09-26
        // every car faded in)*
        // the main pass takes 0x4430A0's frustum test 0x539480 on every car but the camera's own
        // (0x44311F): a car outside the view queues nothing — no shadow, no lamps, no effect drive
        let main_cull = {
            let forward = (self.camera.target - self.camera.position).normalize_or_zero();
            let right = forward.cross(self.camera.up).normalize_or_zero();
            let tan_v = (self.camera.fov_y_radians * 0.5).tan();
            nfs_game::world::CullView { eye: self.camera.position, forward, right, up: right.cross(forward).normalize_or_zero(), tan_h: tan_v * self.renderer.as_ref().map_or(4.0 / 3.0, |r| r.aspect()), tan_v, near: self.camera.near, draw: self.draw_distance }
        };
        let dyn_pose = DynPose { alpha, local_draw, local_pos, local_rot, mirror_pass: false, cull: Some(main_cull), lod: self.renderer.as_ref().map(|r| { let (w, h) = r.size(); (w.max(1) as f32, h.max(1) as f32) }).unwrap_or((640.0, 480.0)), ref_width: self.renderer.as_ref().map_or(640.0, |r| r.size().0.max(1) as f32) };
        let mut pools = PoolUse::default();
        let mut dyn_items = std::mem::take(&mut self.blended_dyn_items);
        self.scratch = sc;
        self.dyn_items(&dyn_pose, &mut pools, &mut dyn_items);
        self.blended_dyn_items = dyn_items;
        let mut sc = std::mem::take(&mut self.scratch);
        self.pool_used = pools;
        self.dyn_pose = Some(dyn_pose);
        {
            let fwd = (self.camera.target - self.camera.position).normalize_or_zero();
            let right = fwd.cross(self.camera.up).normalize_or_zero();
            let up = right.cross(fwd).normalize_or_zero();
            // the merge: the track's far list (flag-1 / flag-4 parts, instance items, a
            // breakable's pieces) with the cars' and the skid marks' own `+0xC` entries, one
            // farthest-first run (`nfs_renderer::farthest_first`, stable on equal keys, as the
            // original's tree insert is — 0x466933 / 0x466AD3). Then the port's no-`Base`
            // fallback, as before. `RE/RENDERING.md` "The view's lists"; `RE/notes/thrash.md`
            // §6d.6 — until now the port drew these three kinds in separate blocks instead of
            // this one list.
            sc.blended.clear();
            sc.blended.extend(sc.lists.far.iter().map(|&(k, i)| (k, nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i })));
            sc.blended.extend(self.blended_dyn_items.iter().copied());
            nfs_renderer::farthest_first_into(&sc.blended, &mut sc.order, &mut self.scene.items);
            self.scene.items.extend(sc.lists.rest.iter().map(|&i| nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i }));
            self.track_item_range = (track_range_start, self.scene.items.len());
            // the flares, the particles and the streaks: this view's own entries (the mirror's pass
            // builds its own, `Self::effect_items` from `Self::render_mirror`)
            let effects_start = self.scene.items.len();
            let draws = std::mem::take(&mut self.article_draws);
            self.effect_items(fwd, right, up, &draws, true, &mut sc.effects);
            self.article_draws = draws;
            self.scene.items.extend(sc.effects.drain(..));
            self.effect_item_range = (effects_start, self.scene.items.len());
            self.weather_items();
            // the sun's lens flare (0x45D25C): last, after the world and the cars, as 0x468B89 runs it;
            // SKYOFF skips that pass too (0x468B80)
            if !self.sky_off() {
                self.lens_flare_items();
            }
        }
        self.scratch = sc;
    }

    /// The HUD's per-frame draw 0x425950 (`RE/notes/hud*.md`): the sprites of every tray, billboard,
    /// dialog and counter laid into `scene.sprites` from `hud_sprites_from` on. Returns where the HUD's
    /// sprites start and the dialog button under the pointer, which `rebuild_scene` settles after the
    /// unanchored text is resolved.
    fn hud_pass(&mut self) -> (usize, Option<Option<(u8, usize)>>) {
        // the HUD is a 640×480 layout; on a wider window its elements are anchored to the window's
        // edges as 0x41E480 does (`Hud::anchor`), not stretched
        let hud_aspect = self
            .renderer
            .as_ref()
            .map(|r| r.aspect())
            .unwrap_or(HUD_REF_W / HUD_REF_H);
        // the dialog button under the pointer, when a dialog with buttons is drawn this frame
        let mut hud_hover: Option<Option<(u8, usize)>> = None;
        // everything from here to the pause menu is the HUD's per-frame draw 0x425950, which
        // `[0x60CA00]` skips whole (0x425950…0x42595D; `hud_hidden`)
        let hud_sprites_from = self.scene.sprites.len();
        PENDING_TEXT.with(|p| p.borrow_mut().clear());
        // `HWrongWay` is drawn only while `car+0xD30` is set past tick 448, outside a mission and
        // a replay (0x436F70's callers); each draw steps its blink. The replay test is `REPLAY
        // == 2` (0x425AFB), which a playback keeps through its exhausted frames — `REPLAY` turns
        // 3 only as the race loop is left (0x415CE2). *(Until 2026-09-27 the port tested
        // `Playing` only, so a stream run out showed the sign.)*
        let wrong_way = {
            let playing = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
            let up = self.race.as_ref().is_some_and(|r| !r.finished && r.wrong_way_ticks > 0)
                && self.tick > 448
                && self.mission.is_none()
                && !playing
                // `HWrongWay`'s draw returns at once under `DERBY [0x657400]` (0x436F70…0x436F80,
                // before the blink counters 0x436F86 — they hold)
                && self.cheat_bits & (1 << 7) == 0;
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
        };
        let mut finish_close = false;
        // `HLeader`'s picture: the car in first place (`car+0xB48 == 1`, 0x428717) and its
        // player's file (`netplay::NetPlay::leader_picture`) — only in a network race
        let leader = if self.net.session.is_some() && self.race.is_some() {
            let first = self.live_order().first().copied();
            match (self.net.session.as_mut(), self.renderer.as_mut(), first) {
                (Some(n), Some(r), Some(d)) => n.leader_picture(r, d.checked_sub(1)),
                _ => None,
            }
        } else {
            None
        };
        if let (Some(h), Some(v)) = (&self.hud, &self.vehicle) {
            h.display.set(Hud::display_colour(self.choice_index("Display.DispCols").map_or(0, |(i, _)| i)));
            // the trays' box: `HudKeys::boxes`, TRANSLUCENCY's at the session's start, F8 flips it
            h.boxes.set(self.hud_keys.boxes);
            // `HPosition` 0x42B000 — only with `OPPONENTLIST` on and `[0x60C970]` (0x41E880), never
            // in a replay (`REPLAY` 2 or 3, 0x42B010 / 0x42B01C; until 2026-09-27 the port tested
            // `Playing` only). It stays up past the player's finish: neither it nor 0x425950 tests
            // `car+0xB50` for the whole element (audit 2026-09-27; the port had dropped it at the
            // finish). The port used to print "POS p/N" (or "DNF") in the top tray instead.
            let opp_mode = self.choice_index("Display.OppList").map_or(0, |(i, _)| i);
            let playing_back = self
                .replay
                .stream
                .as_ref()
                .is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
            let opp_list = if opp_mode != 0
                && self.hud_keys.live
                && !playing_back
                && !self.road.opponents.is_empty()
                && let Some(r) = self.race.as_ref()
            {
                let order = self.live_order();
                let derby = self.cheat_bits & (1 << 7) != 0;
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
                            dnf: eliminated.then(|| self.fe.strings.hud(18191).unwrap_or_default().to_string()),
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
            };
            // `REPLAY != 0` (0x418540…0x418547): 2 through a playback's exhausted frames too
            let in_replay_now = self
                .replay
                .stream
                .as_ref()
                .is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
            let (time_penalty, time_alert) = match (&self.mission, &self.skill) {
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
            let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
            // `HSpeed` 0x42E593: `SPEEDUNITS [0x657DC0]` 0 → `_ftol(car+0xEC4 · 3.6)` under FE 11,
            // otherwise `· 2.2374139` under FE 20; "00" below 10, "0" below 100 (0x42E5E9…0x42E656),
            // and "000" outright below 1 m/s (0x42E66D). This port always showed km/h, and "001"…"003"
            // where the original shows "000".
            let mph = self.choice_index("Display.SpeedUnits").is_some_and(|(i, _)| i != 0);
            let ms = v.speed();
            let text = HudText {
                speed: if ms < 1.0 {
                    "000".to_string()
                } else {
                    format!("{:03}", (ms * if mph { 2.237_413_9 } else { 3.6 }) as i32)
                },
                speed_label: fe(if mph { 20 } else { 11 }),
                gear: match v.gear {
                    0 => fe(18145),
                    1 => fe(18146),
                    g @ 2..=8 => (g - 1).to_string(),
                    _ => fe(18146),
                },
                gear_label: fe(10),
                time_label: fe(18),
                wrong_way: fe(18159),
                speedometer: self.choice_index("Display.Speedometer").map_or(2, |(i, _)| i),
                tachometer: self.choice_index("Display.Tach").map_or(2, |(i, _)| i),
                // `fe.txt` has no `gear` line, so `GEAR [0x657DF0]` keeps the `nfs5-2.hud` Config 1
                // default, 1 (hud.md §2) — `choice_index` answers that default too
                gear_on: self.choice_index("Display.Gear").is_some_and(|(i, _)| i == 1),
                rpm_label: fe(18144),
                tach_scale: {
                    let r = self.specs.redline as i32;
                    if r != 0 { (r - 750) as f32 / r as f32 } else { 1.0 }
                },
            };
            // "wide screen" (0x4259B2 → 0x427B00): two opaque black bars over the full width,
            // `ftol(H·0.15 + 2)` high at the top and from `H − that` at the bottom, drawn under the
            // HUD elements (0x41A330 follows)
            if self.widescreen() {
                // in the render target's pixels (design coordinates are pixels, shifted by the origin)
                let (el, et, er, eb) = Hud::edges();
                let b = ((eb - et) * 0.15 + 2.0).trunc();
                let black = [0.0, 0.0, 0.0, 1.0];
                let mut bars = vec![h.quad(hud_aspect, el, et, er - el, b, black)];
                bars.push(h.quad(hud_aspect, el, eb - b, er - el, b, black));
                self.scene.sprites.extend(bars);
            }
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
            let playback = self
                .replay
                .stream
                .as_ref()
                .is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
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
            // `Display.ScreenLayout`'s current "Config N" preset (`SCREEN_LAYOUTS`, `0x41FB40`) —
            // the shipped file only has four at 640×480, so a fifth option in `CHOICE_ACCESSORS`
            // would have nothing to select; there is none (`main.rs`'s `("Display.ScreenLayout",
            // "screenlayout", &[1824, 1825, 1826, 1827])` is exactly four rows)
            let layout = &SCREEN_LAYOUTS[self.choice_index("Display.ScreenLayout").map_or(0, |(i, _)| i.min(3))];
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
            // `HGraphicDamage` with `Display.Damage` on its first choice (0x41E8B0 `== 1`), while the
            // racing trays are up — its gates are 0x41E8B0 and the state (0x42A3B0) only, so past
            // the player's finish too (audit 2026-09-27)
            if self.choice_index("Display.Damage").is_some_and(|(i, _)| i == 1)
                && self.race.is_some()
            {
                let over = self.mission.as_ref().is_some_and(|ms| nfs_game::skill::damage_over_limit(&ms.mission, &v.damage));
                let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
                let mut dmg = Vec::new();
                h.damage_sprites(&mut dmg, hud_aspect, &v.damage, ["0%", "100%", &fe(12)], over);
                Hud::slide(&mut dmg, 0, hud_aspect, (0.0, 0.0, 127.0, 105.0), tray4);
                self.scene.sprites.extend(dmg);
            }
            // `HTextDamage` with `Display.Damage` on "bar graph" or "percentages" (0x41E8D0 `== 2 ||
            // (== 3 && [0x60C970])`), finished or not (0x430150 has no finish test)
            if let Some((mode @ (2 | 3), _)) = self.choice_index("Display.Damage")
                && (mode == 2 || live)
                && self.race.is_some()
            {
                let over = self.mission.as_ref().is_some_and(|ms| nfs_game::skill::damage_over_limit(&ms.mission, &v.damage));
                let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
                let values = [
                    v.engine_wear(&self.specs) as i32,
                    v.steering_damage() as i32,
                    v.body_damage() as i32,
                    v.suspension_wear(&self.specs) as i32,
                ];
                let mut dmg = Vec::new();
                h.text_damage_sprites(&mut dmg, hud_aspect, values, [&fe(13), &fe(14), &fe(15), &fe(16), &fe(12)], mode == 3, over);
                Hud::slide(&mut dmg, 0, hud_aspect, (0.0, 0.0, 124.0, 100.0), tray4);
                self.scene.sprites.extend(dmg);
            }
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
                let mut m = Vec::new();
                if matches!(slide.state, 1 | 2) {
                    h.tray(&mut m, hud_aspect, rx, ry, 240.0, 84.0);
                }
                if slide.state == 3
                    && self.rear_mirror.is_some()
                    && let Some(tex) = self.mirror_tex
                {
                    m.push(h.image(hud_aspect, tex, rx, ry, 240.0, 84.0));
                }
                Hud::slide(&mut m, 0, hud_aspect, (rx, ry, 240.0, 84.0), slide);
                if slide.state != 0 {
                    let (bx, by) = Hud::anchor(hud_aspect, rx, ry, 240.0, 84.0);
                    h.bevel(&mut m, hud_aspect, (bx, by, 240.0, 84.0), 4.0, 0);
                }
                self.scene.sprites.extend(m);
            }
            if let Some(tex) = leader {
                let mut m = Vec::new();
                h.leader_sprites(&mut m, hud_aspect, layout.leader, tex, self.fe.strings.hud(18192).unwrap_or_default());
                let (lx, ly) = layout.leader;
                Hud::slide(&mut m, 0, hud_aspect, (lx, ly, 59.0, 85.0), tray4);
                self.scene.sprites.extend(m);
            }
            if let Some((rows, place, links)) = &opp_list {
                let (px, py) = layout.position;
                let mut list = Vec::new();
                let label = self.fe.strings.hud(17).unwrap_or_default().to_string();
                h.opponent_list_sprites(&mut list, hud_aspect, (px, py), rows, links, *place, &label, self.cheat_bits & (1 << 7) != 0);
                Hud::slide(&mut list, 0, hud_aspect, (px, py, 138.0, (rows.len() as f32 + 1.0) * 15.0 + 4.0), tray4);
                self.scene.sprites.extend(list);
            }
            // `HReplay` is up for as long as the stream is playing (`hud.md` §5.2): its draw tests
            // `REPLAY == 2` (0x42D1E1…0x42D1EC), which holds until the race loop is left — the
            // exhausted frames too (`REPLAY = 3` at 0x415CE2 only). *(Until 2026-09-27 the port
            // tested `Playing` only.)*
            if let Some(r) = &self.replay.stream
                && matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)
            {
                // not drawn in state 0 (0x42D28E), faded by the state's per cent otherwise
                if let Some(pct) = self.replay_bar.pct(clock) {
                    let mut bar = Vec::new();
                    let tips: [String; 7] = std::array::from_fn(|i| self.fe.strings.hud(nfs_game::replay::bar_tooltip(i, self.replay.frozen, self.replay_net.is_some())).unwrap_or_default().to_string());
                    h.replay_bar(&mut bar, hud_aspect, self.replay.frozen, self.replay_net.is_some(), &self.replay_bar, pct, self.fe.cursor, &tips);
                    self.scene.sprites.extend(bar);
                }
            }
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
                let mut map = Vec::new();
                // 0x432F92: the current lap is `min(car+0xB20 + 1, NUM_LAPS)` — the laps already
                // timed, plus one — and 0x432EA3 suppresses the whole counter on a linear track.
                // This port printed it as "LAP n/N" in the top-right tray.
                let linear = !self.records.track(&self.track.name).map(|t| t.circuit).unwrap_or(true);
                let lap_label = self.fe.strings.hud(19).unwrap_or_default().to_string();
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
                        (lap_label.as_str(), format!("{}/", (done + 1).min(of)), of.to_string())
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
                let base = [0.0f32, 0.0, 33.0, 66.0, 100.0][self.choice_index("Display.MapScale").map_or(4, |(i, _)| i.min(4))];
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
                h.map_sprites(&mut map, hud_aspect, layout.track_map, &others, k, lap, matches!(slide.state, 1 | 2));
                let (mpx, mpy) = layout.track_map;
                Hud::slide(&mut map, 0, hud_aspect, (mpx, mpy, MAP_PX as f32, MAP_PX as f32), slide);
                self.scene.sprites.extend(map);
            }
            // diagnostic: `NFS_CONGRATS=<place>,<tick>,<0|1 tournament>` draws the `HCongrats`
            // picture at that point of its timeline, for a `--screenshot` without finishing a race
            if let Ok(spec) = nfs_game::config::cfg().nfs_congrats.var() {
                let v: Vec<u64> = spec.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if let [place, t, tour] = v[..] {
                    let mut c = Vec::new();
                    h.congrats_sprites(&mut c, hud_aspect, place as usize, t, tour != 0);
                    self.scene.sprites.extend(c);
                }
            }
            // diagnostic: `NFS_BILLBOARD=pause|mission|yesno|summary|standings` draws that dialog over the race, for a
            // `--screenshot` of its layout
            if let Ok(which) = nfs_game::config::cfg().nfs_billboard.var() {
                let fe = |id: u32| self.fe.strings.hud(id).unwrap_or_default().to_string();
                let mut c = Vec::new();
                match which.as_str() {
                    "summary" => {
                        let rows = vec![
                            SummaryRow { name: self.player_name.clone(), car: self.specs.name.clone(), colour: [0.83, 0.06, 0.06, 1.0], best: Some(3999), total: Some(8100) },
                            SummaryRow { name: "Stroller".into(), car: "'94 911 Carrera".into(), colour: [0.2, 0.4, 0.8, 1.0], best: None, total: None },
                        ];
                        let head = [fe(18111), fe(18112), fe(18113), fe(18114), fe(18115)];
                        let bb = Billboard { left: &fe(Self::summary_title(self.race_fegame_type())), right: &fe(18130), hints: Some((&fe(18100), Some(&fe(18101)))) };
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
                        let bb = Billboard { left: "", right: &fe(18132), hints: Some((&fe(18100), Some(&fe(18101)))) };
                        let ord: Vec<String> = (18116..18124).map(fe).collect();
                        let panel = (&w, [fe(18165), fe(18166), fe(18167), fe(18168)], Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0)));
                        h.standings_sprites(&mut c, hud_aspect, &rows, Some(0), &bb, &[fe(18111), fe(18127), fe(18161)], false, &ord, Some(panel));
                    }
                    "yesno" => h.yes_no_sprites(&mut c, hud_aspect, &Billboard { left: "", right: &fe(18174), hints: None }, [&fe(18171), ""], [&fe(18141), &fe(18142)], 1, false),
                    "pause" => h.pause_sprites(&mut c, hud_aspect, &Billboard { left: "", right: &fe(18128), hints: None }, &[fe(18108), fe(18109), fe(18110)], 0, false),
                    "mission" => h.mission_summary_sprites(&mut c, hud_aspect, &Billboard { left: &fe(18134), right: &fe(18135), hints: Some((&fe(18100), Some(&fe(18101)))) }, Some(0), &fe(18176), &[fe(18156)], &fe(18109), None, (false, false)),
                    _ => {}
                }
                self.scene.sprites.extend(c);
            }
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
                    (true, false) => (vec![fe(18111), fe(18112), fe(18114), fe(18115)], &[30.0, 90.0, 140.0, 80.0, 60.0]),
                    (true, true) => (vec![fe(18111), fe(18112), fe(18113), fe(18114), fe(18115)], &[30.0, 90.0, 140.0, 100.0, 80.0, 60.0]),
                    (false, true) => (vec![fe(18111), fe(18112), fe(18113), fe(18114)], &[30.0, 90.0, 140.0, 100.0, 80.0]),
                    (false, false) => (vec![fe(18111), fe(18112), fe(18114)], &[30.0, 90.0, 140.0, 80.0]),
                };
                let ordinals: Vec<String> = (18116..18124).map(fe).collect();
                let (esc, space) = (fe(18100), fe(18101));
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
                    let bb = Billboard { left: &fe(18134), right: &fe(18135), hints: Some((&esc, Some(&space))) };
                    // its "race again" button's hover (0x41B340), once the box is open
                    let over = h.billboard_h.get() >= BILLBOARD.3 && Hud::over_button(Hud::mission_button(), self.fe.cursor);
                    hud_hover = Some(over.then_some((2, 0)));
                    h.mission_summary_sprites(&mut table, hud_aspect, &bb, narrator, &name, &lines, &fe(18109), Some(self.page_frames as usize), (over, over && self.fe.mouse_down));
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
                    let head = [fe(18111), fe(if timed { 18162 } else { 18127 }), fe(18161)];
                    // 0x431897: "" left, 18132 right, both hints
                    let bb = Billboard { left: "", right: &fe(18132), hints: Some((&esc, Some(&space))) };
                    // `HWinnings`, the player's money, under the pointer (0x431AF6…0x431B70)
                    let winnings = c.career.winnings_after(&t, event, &order, &times, &clean, racers, last);
                    // diagnostic (port-only): `NFS_RESULTS_CURSOR=winnings` parks the pointer on the
                    // middle of the player's WINNINGS cell, `=x,y` at that point of the 640 × 480
                    // HUD space — a scripted run has no pointer, and `NFS_FE_CURSOR` parks the front
                    // end's, not the race's. Unset, the live pointer.
                    let parked = nfs_game::config::cfg().nfs_results_cursor.var().ok().and_then(|v| {
                        if v.trim().eq_ignore_ascii_case("winnings") {
                            let widths: &[f32] = if !timed { &[40.0, 90.0, 100.0, 100.0] } else { &[40.0, 90.0, 120.0, 100.0] };
                            let (x0, x1, y0, y1) = Hud::table_cell(widths, player?, 3);
                            Some(((x0 + x1) / 2.0, (y0 + y1) / 2.0))
                        } else {
                            let (x, y) = v.split_once(',')?;
                            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
                        }
                    });
                    let panel = (&winnings, [fe(18165), fe(18166), fe(18167), fe(18168)], parked.or(self.fe.cursor));
                    h.standings_sprites(&mut table, hud_aspect, &rows, player, &bb, &head, timed, &ordinals, Some(panel));
                } else if let ResultPage::Notice(id) = page {
                    // `HInfo` 0x4282F0: the text centred on the 500 × 325 box (`[0x5B2BF0]`/`[0x5B2BF4]`)
                    // `HInfo` 0x428328: "" left, 18157 "Race Info" right, both hints — its Spacebar
                    // byte is its own constructor argument, and the post-race one `[0x60C9FC]` is
                    // built with 1 (`push 1` before 0x4250BF)
                    let bb = Billboard { left: "", right: &fe(18157), hints: Some((&esc, Some(&space))) };
                    h.notice_sprites(&mut table, hud_aspect, &bb, &fe(id));
                } else if page == ResultPage::Records
                    && let Some(rec) = &self.hud_records
                {
                    // `HTrackRecords` 0x433830: 18133 "Track Records" / 18158 "Lap Records", columns
                    // NAME 18111, CAR 18112, TIME 18114, rows 1st…8th
                    let title = fe(if rec.linear { 18133 } else { 18158 });
                    let head = [fe(18111), fe(18112), fe(18114)];
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
                    let bb = Billboard { left: &title, right: &fe(18130), hints: Some((&esc, Some(&space))) };
                    h.results_sprites(&mut table, hud_aspect, &rows, player, &bb, &head, widths, circuit, points, &ordinals);
                }
                h.billboard_h.set(BILLBOARD.3);
                self.scene.sprites.extend(table);
                if closed_now {
                    finish_close = true;
                }
            }
            // `HMultiTime` 0x42A200 — a `HComboTextVisual` like `HTime`: the tray 73×34 at (427,0)
            // (`nfs5-2.hud`), the value top-right and a `hud.loc` label bottom-left (0x41C8C0).
            // It shows only while the racing trays do (they slide out for the post-race billboards).
            // This port printed an upper-case "RECORD LAP  m:ss.hh" line with no tray, and a
            // "KNOCKED OUT" banner for an eliminated player that has no string in the game —
            // HPosition's FE 18191 "DNF" is the original's only sign of that (0x42B2B6).
            // (gates 0x41E850 and the state 0x42A3B0 only — it stays past the finish, 2026-09-27)
            let racing = self.race.is_some();
            // the gate 0x41E850: `SPLITTIMES != 0 && [0x60C970]`
            let split_times = self.choice_index("Display.SplitTimes").map_or(0, |(i, _)| i);
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
                let laps = self
                    .race
                    .as_ref()
                    .map(|r| r.lap_times.clone())
                    .unwrap_or_default();
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
                let label = self.fe.strings.hud(label).unwrap_or_default().to_string();
                // `Display.ScreenLayout`'s own position for this tray (cfg2 +0xC/+0x10, `layout`
                // above, `HudLayout::multi_time`) — was hardcoded to Config 1's `(427.0, 0.0)`
                let (mx, my) = layout.multi_time;
                let mut tray = Vec::new();
                h.combo_sprites(&mut tray, hud_aspect, (mx, my, 73.0, 34.0), &format_ticks(value), &label, h.display());
                Hud::slide(&mut tray, 0, hud_aspect, (mx, my, 73.0, 34.0), time_slide);
                self.scene.sprites.extend(tray);
            }
        }
        // the HUD frame's cone count (0x425D7F…0x425DFD): not in a replay (`[0x657424]` 0 or 1), in
        // a mission (`[0x6573E8] == 9`) with a cone rule (`CONEIMIT` or `CONEPENALTY` not −1), over
        // every smackable; a move plays 0x4AAE50, flashes `HTime` and bumps the summary's `+0x140`.
        // It runs after the billboards' and the trays' draws (0x425A10…0x425A72), so the frame
        // the summary first evaluates in (`fix_mission_result`) has not counted yet, and the
        // flash shows from the next draw on.
        if self.hud.is_some()
            && self.vehicle.is_some()
            && self.mission.as_ref().is_some_and(|ms| ms.mission.cone_rule.is_some())
            && !self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted))
            && self.hud_cones.frame(self.smackables.knocked_count())
        {
            tracing::info!(count = self.hud_cones.last, moves = self.hud_cones.changes, "cone hit");
            self.cone_flash = 16;
            if let (Some(a), Some(s)) = (&self.snd.audio, self.snd.sfx.as_mut()) {
                s.pylon_hit(a);
            }
        }
        // the page that finished shrinking this frame makes way for the next (state 0)
        if finish_close {
            self.advance_results_page();
        }
        (hud_sprites_from, hud_hover)
    }

    /// The camera slot's per-frame work (`RE/notes/camera*.md`): the view's update(s) since the last
    /// game tick, the car-relative eye, the mirror's eye, the beacon and the headlight frames.
    /// Returns the view id and the slot's near plane.
    fn camera_pass(&mut self, car_pos: Vec3, car_rot: Quat, rear_view: bool) -> (u16, f32) {
        // every view is slot 0's half-angle, 37.5° by default, across (`camera::fov_y`); only a
        // trackside camera element brings its own (`+0x9C` when `+0x98` is set)
        // the TV modes take the view the last trackside camera set (`TvSlot::view`): the sweep
        // 0x413B80 switches only while the chosen camera `[0x606850]` is 0x14 (0x463E1D…0x463E28),
        // and then 0x413960's SetSlotView makes the element's own view the slot's.
        // `[0x606850] == 0x14`: the chosen camera is the TV one
        // the slot's `AnimCam` update 0x462F30 runs once per game tick too (dispatch 0x463FFE of the
        // same `Sc64`): it samples the player at `gGameTick − start`, so a frame of several ticks asks it for
        // each tick in turn, and when the player runs out at tick `m` of the frame (status 2, 0x462F89) the
        // slot's SetSlotView takes the cycle's first view and the dispatcher runs again in the same tick
        // (0x46324A) — the new view's updates are the frame's remaining ticks, from `m` on. (`AnimCam` keeps
        // no state but the clock, so the ticks before the end leave nothing else behind.) A frame between two
        // ticks (`steps == 0`) asks once, at the frame's own tick.
        let cam_tick0 = self.camera_tick;
        let mut steps = self.tick.saturating_sub(self.camera_tick).min(u32::MAX as u64) as u32;
        self.camera_tick = self.tick;
        let mut anim_pose = None;
        let mut anim_done = 0;
        for u in 0..steps.max(1) {
            let was_anim = self.anim_cam != AnimCamSlot::Off;
            let at = if steps == 0 { self.tick } else { cam_tick0 + 1 + u64::from(u) };
            anim_pose = self.camera_anim_step(at, car_pos, car_rot);
            if was_anim && anim_pose.is_none() && self.anim_cam == AnimCamSlot::Off {
                anim_done = u;
                break;
            }
        }
        steps -= anim_done;
        let tv_mode = matches!(self.camera_view(), 0x0C | 0x14);
        let view = match (self.camera_view(), self.track_cameras.as_ref().and_then(|t| t.tv.view)) {
            (0x0C | 0x14, Some(v)) if v != 0x0C && v <= 0x13 => v,
            (v, _) => v,
        };
        // **The slot's offset `+0x08` seed 0x465CF0**: `-4.0 · car+0x37C` (the matrix's third row — the
        // car's forward axis), called with the slot's mask at the race's init 0x4640B0 (0x464247), by SetSlotView
        // 0x465790 (0x465805) and by the cycle key 0x465A60 (0x465B1A) — the last two only when the slot's OLD
        // view is outside 2…7 (`cmp ax, 2 ; jl` / `cmp ax, 7 ; jle`, 0x4657EF…0x4657FF, 0x465B0A…0x465B14),
        // before the new view is stored. The views 2…7 keep their own offset across a switch; the others
        // (0, 1, 8…0x0F, 0x10…) leave it at the seed, which a view whose update never writes it (CircleCam,
        // SplineCam, TVCam, BlimpCam, 0x10 / 0x11 / 0x12 / 0x13) then keeps for the pause orbit (0x461850) to turn.
        // The switch is seen here as a change of the view since the last pass.
        // SetSlotView re-entered on the SAME view (the cycle key landing on it, 0x465A60; an enabled
        // trackside switch to an element of the same view, 0x4139DD) runs the same reset — the settle count back
        // to 0x32 (0x465848) and, when the old view is outside 2…7, the offset seed — which a view-tag compare
        // cannot see
        let entered_now = (self.slot_key_enters, self.track_cameras.as_ref().map_or(0, |t| t.tv.switches));
        let slot_entered = self.slot_view.is_some() && entered_now != self.slot_seen;
        self.slot_seen = entered_now;
        if self.slot_view.is_none_or(|old| (old != view || slot_entered) && !(0x02..=0x07).contains(&old)) {
            self.chase_offset = Some((car_rot * Vec3::Z * -4.0, u16::MAX));
        }
        self.slot_view = Some(view);
        let ws = self.widescreen();
        let fov_of = |h: f32| nfs_game::camera::fov_y_window(nfs_game::camera::half_angle(h, ws, view), ws);
        self.camera.fov_y_radians = fov_of(nfs_game::camera::DEFAULT_HALF_ANGLE);
        // the slot's near plane `+0x5C` (`camera::near_plane`, applied with the far plane below):
        // SetSlotView's 0.5 unless 0x462B80 (TVCam, SplineCam) has zoomed
        let mut slot_near = nfs_game::camera::DEFAULT_NEAR;
        // the aimed views' up comes out of 0x464BF0 below, which may keep last build's up row
        let prev_up = self.camera.up;
        // and its forward row `+0x94`, which the roll-over camera reads on its first update
        let prev_fwd = (self.camera.target - self.camera.position).normalize_or_zero();
        // the slot updates since the last frame: one per game tick (`camera_tick`); a frame drawn
        // between two ticks moves nothing but the interpolated car the offsets hang off.
        // every view's update (chase / heli 0x461E80 / 0x4623D0, the roll-over 0x461590,
        // 0x4618B0, the driver's head lag 0x461A30, BlimpCam, the circle / spline / trackside cameras) runs once
        // per game tick against its own tick's car (`Sc64`, 0x463C30) — the [`CamTick`] history; the last update
        // of the frame takes this frame's (interpolated) pose. The look-around (0x4611C0) reads no car state
        // (held bits only); `AnimCam` 0x462F30 is stepped per tick above.
        // `+0xC0 & 0x08`, the aim bit — the table's 0x04 shifted up by SetSlotView: views 2…8,
        // 0x0A, 0x0B…0x0D (a trackside camera without its own matrix, below), 0x12 / 0x13 (the cop
        // cameras, not carried)
        let mut aimed = matches!(view, 0x02..=0x08 | 0x0A | 0x0B | 0x0D);
        self.camera.up = Vec3::Y;
        let off = nfs_game::camera::VIEW_OFFSETS
            .get(view as usize)
            .copied()
            .unwrap_or(nfs_game::camera::VIEW_OFFSETS[2]);
        // a view that rides the car and looks forward from it: the bumper and shotgun cameras
        let rides_car = view == 0x00;
        // the chase family — behind the car, heading and height low-passed
        let chases = matches!(view, 0x02..=0x07 | 0x0D);
        // **0x4611C0**: the bumper and driver views turn about the slot's up axis toward the
        // target the four look bits make (`look_target`) and back to 0 when they are released
        // (`camera::look_step`). The bits are levels, `RaceFrame` reading them held
        // (0x415AFA…0x415B2C: 0x16 `[0x5E99A8]`, 0x15 `[0x5E99AC]`, 0x13 `[0x5E99B0]`, 0x14
        // `[0x5E99B4]`), and only while the pause is off (`[0x606A90]`, 0x415982).
        if !self.paused {
            self.look_held = [Action::LookLeft, Action::LookRight, Action::LookForward].map(|a| self.vehicle.is_some() && self.input.held(a));
        }
        let [look_left, look_right, look_fwd] = self.look_held;
        let look = if matches!(view, 0x00 | 0x01) && self.vehicle.is_some() {
            let (mut y, mut turn) = match self.look_yaw {
                Some((y, v, t)) if v == view => (y, t),
                _ => (0, false),
            };
            let target = look_target(look_left, look_right, rear_view, look_fwd);
            for _ in 0..steps {
                (y, turn) = nfs_game::camera::look_step(y, target);
            }
            self.look_yaw = Some((y, view, turn));
            Quat::from_rotation_y(if turn { (y as f32).to_radians() } else { 0.0 })
        } else {
            self.look_yaw = None;
            Quat::IDENTITY
        };
        // **The ceiling test 0x461370(slot, p, &y)**: `y` is an eye height over the car's y. Over the
        // ceiling patches at `p` in plan (0x473420 from `(p.x, car.y, p.z)`), skipping one whose corners
        // are all within 1.0 of both the eye `car.y + y` and `p.y` or below (0x461408…0x461450) and one
        // whose centre (vt+4, the corners' mean) is within 2.0 of the car's y (0x461462…0x461494): when
        // the plane's height `h` at `p` less 1.0 is below the eye, `y = max(h − car.y − 1, 1.0)`
        // (0x4614B6…0x4614F9) — the last such patch wins; the eye it compares with stays the first.
        let ceiling_at = |w: &nfs_game::world::TrackWorld, p: Vec3, car_pos: Vec3, y: f32| -> f32 {
            let eye = car_pos.y + y;
            let mut out = y;
            for (t, h) in w.collision.patches_where(p, |word| word & 0x8000 != 0 && word & 0xF == 0) {
                if [t.a, t.b, t.c].iter().all(|v| v.y - 1.0 <= eye && v.y - 1.0 <= p.y) {
                    continue;
                }
                let c = (t.a + t.b + t.c) * (1.0 / 3.0);
                if c.y >= car_pos.y - 2.0 && c.y <= car_pos.y + 2.0 {
                    continue;
                }
                if h - 1.0 < eye {
                    out = (h - car_pos.y - 1.0).max(1.0);
                }
            }
            out
        };
        // **one slot update per game tick** (`Sc64`, 0x463C30 — every view's update, not the chase /
        // heli pair alone): update `upd` of the frame reads its own tick's car (`car+0x330`, `+0x364`,
        // `+0x3A0`, the velocity, the slice), the last the frame's interpolated pose (glue); the slot's eye
        // `+0x24` the update before left seeds the next one's ceiling tests. A frame between two ticks
        // (`steps == 0`) runs the body once with no step. The build 0x464F80 (pushes, clamp, aim) is not in
        // the dispatcher: it runs once per frame on the last eye, as below. (The slot matrix's forward row,
        // which the roll-over latch reads, is the build's, so `prev_fwd` is the frame's for every update.)
        let aimed0 = aimed;
        let frame_steps = steps;
        let n_upd = (frame_steps + anim_done).max(1);
        let (frame_car_pos, frame_car_rot) = (car_pos, car_rot);
        for upd in anim_done..n_upd {
            let last_upd = upd + 1 == n_upd;
            let steps = frame_steps.min(1);
            let hist = if !last_upd {
                // the ticks are consecutive: the entry is at its offset from the oldest
                let want = cam_tick0 + 1 + u64::from(upd);
                let first = self.cam_hist.front().map_or(want, |h| h.tick);
                self.cam_hist.get(want.wrapping_sub(first) as usize).filter(|h| h.tick == want).copied()
            } else {
                None
            };
            let (car_pos, car_rot) = hist.map_or((frame_car_pos, frame_car_rot), |h| (h.pos, h.rot));
            let fwd = car_rot * Vec3::Z;
            aimed = aimed0;
            // what the update reads of its tick's car: the frame's own for the last
            let cam_car = hist.or_else(|| {
                let v = self.vehicle.as_ref()?;
                let (slice, wrong_way) = self.race.as_ref().map_or((0, false), |r| (r.slice, r.wrong_way_ticks > 0));
                Some(CamTick { tick: 0, pos: car_pos, rot: car_rot, v_local: v.v_local, half_z: v.half_extents.z, slice, wrong_way, velocity: v.velocity, cos_tilt: v.cos_tilt })
            });
            let ceiling = |w: &nfs_game::world::TrackWorld, p: Vec3, y: f32| -> f32 { ceiling_at(w, p, car_pos, y) };
            // look-back (bit 0x13) is read only by views 0…7 (0x4611C0, 0x461B96, 0x461E80, 0x4623D0; the
            // one other reader, 0x444F89, gates a draw): elsewhere it changes nothing
            // **The roll-over camera** (dispatcher 0x463C9E…0x463D2A): for a slot whose flags carry
            // `+0xC0 & 0x40` — the table's 0x20, views 2…0x0A — the view's own update is replaced by
            // **0x461590** while the car's `car+0x3A0` (the ground normal · its up) is below
            // `[0x5B23F0]` 0.5 (`test ah, 1`: C0, strictly below). On the first such update (`+0xC2`
            // 0) the side is latched in `+0xC3` bit 0: the slot matrix's forward row `+0x94` · the
            // slice forward `car+0x3D0` < 0 (0x463CC6…0x463CFB); `+0xC2` is set to 100 on every
            // tilted update, so it is only cleared — and the side read again — once the car is
            // upright (0x463D03). 0x461590: the offset (0, 3, −10), or (0, 3, +10) with the bit, is
            // turned by the **slice's** axes `car+0x3B8` (lateral, up, forward rows, 0x46160C); its y
            // the lower of the two ceiling tests at the car and the old eye (0x461632…0x461669); the
            // slot offset `+0x08` moves toward it by `[0x5B3F50]` 0.02 on every axis (0x46166D…
            // 0x4616C7), eye = car + offset, and the slot flags become `+0xC0 & ~4 | 0x7A`
            // (0x46171F…0x46172C): aimed with the lift (0.325 for views 2…4, else 0.4, 0x464EAF…
            // 0x464ED0), pushed and clamped. The dispatcher sets `+0xC0` bit 0 so the next normal
            // update reloads the view's flags (0x463D22…0x463D59); the offset carries over.
            let flip = if matches!(view, 0x02..=0x0A)
                && let (Some(v), Some(t)) = (cam_car, self.slices.as_ref())
                && let Some(sl) = t.slices.get(v.slice)
            {
                // diagnostic, no original counterpart: `NFS_FLIP_VIEW=1` takes the roll-over camera on
                // an upright car, for a `--screenshot` without a crash
                if v.cos_tilt < 0.5 || nfs_game::config::cfg().nfs_flip_view.var_os().is_some() {
                    let side = *self.flip_side.get_or_insert(prev_fwd.dot(sl.forward) < 0.0);
                    Some(sl.up * 3.0 + sl.forward * if side { 10.0 } else { -10.0 })
                } else {
                    self.flip_side = None;
                    None
                }
            } else {
                None
            };
            if let Some(mut a) = flip {
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.slot_eye.unwrap_or(car_pos + a);
                    a.y = ceiling(w, car_pos, a.y).min(ceiling(w, old_eye, a.y));
                }
                let mut cur = self.chase_offset.map_or(a, |(o, _)| o);
                for _ in 0..steps {
                    cur += (a - cur) * 0.02;
                }
                self.chase_offset = Some((cur, view));
                self.camera.position = car_pos + cur;
                self.camera.target = car_pos + Vec3::Y * 4.0 * if (0x02..=0x04).contains(&view) { 0.325 } else { 0.4 };
                aimed = true;
            } else if matches!(view, 0x08 | 0x09 | 0x0F) {
                // **0x4618B0** (`ShotgunCam2` / `ShotgunCam3` / `ReverseCam1`, dispatch 0x464005): the eye is
                // the car plus its matrix times the table offset, whose y is the lower of the two ceiling
                // tests at the car and the old eye (0x461903…0x461964); the slot's offset is that; the
                // slot matrix is the car's with its right and forward rows negated — a half turn about
                // its up (0x4619BD…0x461A1E). SetSlotView puts the table's flag bits one place up in
                // `+0xC0` (0x4658C0…0x465965), so the aim bit 0x08 is the table's 0x04: view 8 (0x3C) is
                // aimed at the car's origin with the world's up by 0x464BF0; 9 (0x38) and 0x0F (0x12)
                // keep the half-turned matrix and look back down the car.
                let mut a = car_rot * off;
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.slot_eye.unwrap_or(car_pos + a);
                    a.y = ceiling(w, car_pos, a.y).min(ceiling(w, old_eye, a.y));
                }
                self.camera.position = car_pos + a;
                // the slot's offset `+0x08` is that same `A` (0x4619B1…0x4619BA)
                self.chase_offset = Some((a, view));
                if view != 0x08 {
                    self.camera.target = self.camera.position - fwd * 20.0;
                    self.camera.up = car_rot * Vec3::Y;
                } else {
                    self.camera.target = car_pos;
                }
            } else if matches!(view, 0x02..=0x07)
                && let Some(v) = cam_car
            {
                // **The chase cameras 0x461E80** (views 2 / 3 / 4 with argument 0 / 1 / 2, race-logic §11.2):
                // the view's offset less the car's half length, lifted on a slope by the height difference
                // of the slices three behind and three ahead, turned by the car's matrix; the slot's offset
                // follows it by `k` in x and z and 0.05 in y, is kept at least 4.94 long, and the eye is the
                // car plus it, aimed above the car's centre (`aim_lift` below). The build's pushes, which
                // `+0xC0` bit 0x10 (the table's 0x08, set in 0x2D) turns on at 0x4651E4, follow the
                // view branches below.
                // the heli cameras 5 / 6 / 7 run **0x4623D0** with arguments 0 / 1 / 2, the same routine
                // but for: no half-length, the offset pulled back by `min(speed·[0x5B3F6C] 0.05, 2.0)`
                // (0x46256F…0x4625F6), the slope term halved (`[0x5B23F0]`) and clamped [0.25, 1.25] /
                // [−0.75, 3.0], and a 6.32 minimum length in place of 4.94
                let heli = view >= 5;
                let arg = if heli { view - 5 } else { view - 2 };
                // the heli alone: while a replay plays (`[0x5E99F4] >= 2`, playing or run out) the rate
                // is 0.01 whatever the argument (0x462538…0x462541); the chase has no such line
                let replay_heli = heli
                    && self.replay.stream.as_ref().is_some_and(|r| {
                        matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)
                    });
                let table_off = off;
                let (slices, race_backwards) = (self.slices.as_ref(), self.backwards);
                // what one update reads of its tick's car: the offset before the slope lift, the
                // lift, the follow rate `k`
                let per_tick = |v_local: Vec3, half_z: f32, slice: usize, wrong: bool| -> (Vec3, f32, f32) {
                    let (a, b) = (v_local.z.abs(), v_local.x.abs());
                    let speed = a.max(b) + 0.25 * a.min(b);
                    let mut off = off;
                    if heli {
                        off.z -= (speed * 0.05).min(2.0);
                    } else {
                        off.z -= half_z;
                    }
                    let lift = match (arg, slices) {
                        (1 | 2, Some(t)) if !t.slices.is_empty() => {
                            // the step is +3 when wrong-way XOR REVERSE, else −3 (0x461EE2…0x461EFC)
                            let step: i32 = if wrong != race_backwards { 3 } else { -3 };
                            let y = |d: i32| t.slices[t.add_graph(slice, d, None)].centre.y;
                            let dy = y(step) - y(-step);
                            match (heli, arg) {
                                (false, 1) => (dy * (1.0f32 / 3.0)).clamp(0.25, 1.0),
                                (false, _) => (dy * (1.0f32 / 3.0)).clamp(-0.25, 2.0),
                                (true, 1) => (dy * 0.5).clamp(0.25, 1.25),
                                (true, _) => (dy * 0.5).clamp(-0.75, 3.0),
                            }
                        }
                        _ => 0.0,
                    };
                    // the follow rate: 0.1 for views 2 / 5; else `clamp((max + ¼ min of |v_long|, |v_lat|) / 900,
                    // 0.02, 0.1 or 0.05)` (0x461F7D…0x462008)
                    let k = if replay_heli {
                        0.01
                    } else {
                        match arg {
                            0 => 0.1,
                            1 => (speed * (1.0 / 900.0)).clamp(0.02, 0.1),
                            _ => (speed * (1.0 / 900.0)).clamp(0.02, 0.05),
                        }
                    };
                    (off, lift, k)
                };
                let (mut off, lift, k) = per_tick(v.v_local, v.half_z, v.slice, v.wrong_way);
                // look-back (bit 0x13) first snaps the slot's offset `+0x08` to the car-turned offset —
                // the chase one with the half length taken off (0x46200C…0x46202A), the heli one the
                // table's (0x462549…0x462567) — so the follow restarts from there on release
                let snap = car_rot * if heli { table_off } else { off };
                // the aim 0x464BF0: forward `(car − eye)·0.25` plus, with `+0xC0` bit 0x02 (the table's
                // 0x01, set in 0x2D), 0.325 (views 2…4, `[0x5B3F84]`) or 0.4 (`[0x5B261C]`) on y before
                // normalising (0x464E6B…0x464ED3): aimed 1.3 / 1.6 above the car's origin
                let aim_lift = Vec3::Y * 4.0 * if heli { 0.4 } else { 0.325 };
                off.y += lift;
                let target = car_rot * off;
                // both branches seed 0x461370 at the car and at the slot's old eye with the offset's y
                // and keep the lower answer (0x462280 / 0x462292 then 0x462297…0x4622B7; the rigid
                // branch 0x4621BB / 0x4621CD)
                let old_eye = self.slot_eye.unwrap_or(car_pos + target);
                let world = self.world.as_ref();
                let lowest = |y: f32| world.map_or(y, |w| ceiling(w, car_pos, y).min(ceiling(w, old_eye, y)));
                let ky = 0.05;
                // SetSlotView (0x465848…0x46588C) resets the settle count but not the offset `+0x08`, so a
                // new view starts from where the last one left the eye and glides in at 0.1 for 0x32 updates
                let mut cur = match self.chase_offset {
                    Some((o, v0)) if v0 == view && !slot_entered => o,
                    Some((o, _)) => {
                        self.chase_settle = 0x32;
                        o
                    }
                    None => {
                        self.chase_settle = 0x32;
                        target
                    }
                };
                if rear_view {
                    // **the rigid branch** 0x462136…0x462235 (heli 0x4626F1…0x4627F0): the eye is the car
                    // plus the car-turned offset with its z negated — ahead of the car — aimed back at its
                    // centre, and the update returns before the follow; the eye's y is the lower of the
                    // two ceiling tests
                    self.chase_offset = Some((snap, view));
                    let mut a = car_rot * Vec3::new(off.x, off.y, -off.z);
                    a.y = lowest(a.y);
                    self.camera.position = car_pos + a;
                    self.camera.target = car_pos + aim_lift;
                    self.camera.up = Vec3::Y;
                } else {
                    // this update's own tick (the body runs once per tick, `steps` is 0 or 1
                    // here); the ceiling test's old eye is the eye the update before left (0x461370 seeds).
                    let oe = self.slot_eye.unwrap_or(car_pos + target);
                    let mut tgt = target;
                    if let Some(w) = world {
                        tgt.y = ceiling_at(w, car_pos, car_pos, tgt.y).min(ceiling_at(w, oe, car_pos, tgt.y));
                    }
                    for _ in 0..steps {
                        let (k, ky) = if self.chase_settle > 0 {
                            self.chase_settle -= 1;
                            (0.1, 0.1)
                        } else {
                            (k, ky)
                        };
                        cur.x += (tgt.x - cur.x) * k;
                        cur.y += (tgt.y - cur.y) * ky;
                        cur.z += (tgt.z - cur.z) * k;
                        // unless the settle count is still running, an offset shorter than the minimum
                        // is stretched to it (0x532B20 / 0x532360): `[0x5B3F58]` 4.94 for the chase
                        // (0x462345…0x46236F), `[0x5B37B0]` 6.32 for the heli (0x462900…0x46292A) —
                        // the length of the HeliCam0 / 1 offset (0, 2, −6)
                        if self.chase_settle == 0 {
                            let min = if heli { 6.32 } else { 4.94 };
                            let len = cur.length();
                            if len > 0.0 && len < min {
                                cur *= min / len;
                            }
                        }
                    }
                    self.chase_offset = Some((cur, view));
                    self.camera.position = car_pos + cur;
                    self.camera.target = car_pos + aim_lift;
                    self.camera.up = Vec3::Y;
                }
            } else if chases {
                // **BlimpCam 0x0D**, inline in the dispatcher (0x463F11…0x463FF8): the table offset in world
                // axes — not turned by the car — its y the lower of the two ceiling tests 0x461370 at the
                // car and at the old eye; the eye's offset from the car `D = eye − car` (0x532330) moves
                // toward it by `[0x5B2604]` 0.1 per axis each update and the eye is `car + D` (0x532300).
                // Its flags 0x14 give `+0xC0` 0x28 — the aim bit without the lift — so it looks at the
                // car's origin with the world's up.
                let mut target = off;
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.slot_eye.unwrap_or(car_pos + off);
                    target.y = ceiling(w, car_pos, off.y).min(ceiling(w, old_eye, off.y));
                }
                let mut d = self.slot_eye.map_or(target, |e| e - car_pos);
                for _ in 0..steps {
                    d += (target - d) * 0.1;
                }
                self.camera.position = car_pos + d;
                self.camera.target = car_pos;
                self.camera.up = Vec3::Y;
            } else if rides_car {
                // bumper: the table's offset in the car's own frame, looking along it
                self.camera.position = car_pos + car_rot * off;
                self.camera.target = self.camera.position + car_rot * (look * Vec3::Z) * 20.0;
                // `BumperCam` (view 0, 0x461D50): the slot's matrix is the car's own `car+0x364` copied
                // whole (0x461DAF…0x461DC7), so the view rolls and pitches with the car; this port kept
                // the world's up; 0x4611C0 then turns it by the look-around yaw (`look` above).
                self.camera.up = car_rot * Vec3::Y;
            } else if view == 0x0B
                && let (Some(t), Some(v), Some(tc)) = (self.slices.as_ref(), cam_car, self.track_cameras.as_mut())
                && !t.slices.is_empty()
            {
                // **SplineCam 0x463410** (race-logic §11.5), slot state in `TvSlot`: `+0xCC` the pace,
                // `+0x60` the offset, `+0x30` the lift (3.0 when 0, `[0x5B2600]`).
                let n = t.slices.len();
                let car_slice = v.slice.min(n - 1);
                let lift = if tc.tv.lift != 0.0 { tc.tv.lift } else { 3.0 };
                let mut eye = tc.tv.spline_eye.or(self.slot_eye).unwrap_or(car_pos + Vec3::Y * lift);
                // the watched car's distance (0x5323A0) and its way along the slices: the sign of its
                // forward · the slice's forward, turned round when it rolls backwards (0x463488…0x463501)
                let d = car_pos.distance(eye);
                let mut dir: i32 = if fwd.dot(t.slices[car_slice].forward) < 0.0 { -1 } else { 1 };
                if v.v_local.z < 0.0 {
                    dir = -dir;
                }
                // the eye's tracker (0x474440) and the circular slice gap to the car (0x463537…0x463584)
                let mut eye_slice = t.find_slice(eye, tc.tv.spline_slice.unwrap_or(car_slice));
                let raw = car_slice as i32 - eye_slice as i32;
                let gap = if raw.unsigned_abs() as usize > n / 2 { n as i32 - raw.abs() } else { raw.abs() };
                let pace = tc.tv.pace;
                let k = ((1.0 - pace) * 15.0) as i32;
                if gap > k + 3 && pace < 1.0 && d > 25.0 && !tv_mode {
                    // the jump (0x4635E3…0x4636CE): `min(k + 1, 8)` slices ahead in the car's way, cut at
                    // the first junction 0x4816F0 finds; the eye goes to the centre of the slice that far
                    // less one step, raised by the lift, and the offset `+0x60` is cleared
                    let m = (k + 1).min(8) * dir;
                    let mut e = m;
                    let mut j = if m > 0 { 0 } else { m };
                    while if m > 0 { j < m } else { j <= 0 } {
                        if t.junction_within(car_slice, j).is_some() {
                            e = j;
                            break;
                        }
                        j += 1;
                    }
                    eye_slice = t.add_graph(car_slice, e - dir, None);
                    eye = t.slices[eye_slice].centre + Vec3::Y * lift;
                    tc.tv.spline_offset = Vec3::ZERO;
                }
                // the point it heads for (0x4636EF…0x463941): the next slice in its way — through a
                // junction the last link going that way — clamped to the table, its centre raised by
                // the lift plus `+0x60` along its lateral (× the way) / up / forward; from the eye, or in
                // the TV mode from its own slice's raised centre
                let base = if tv_mode { t.slices[eye_slice].centre + Vec3::Y * lift } else { eye };
                let next = if t.junction_within(eye_slice, dir).is_some() {
                    t.add_graph(eye_slice, dir, None)
                } else {
                    (eye_slice as i32 + dir).clamp(0, n as i32 - 1) as usize
                };
                let sl = &t.slices[next];
                let o = tc.tv.spline_offset;
                let q = sl.centre + Vec3::Y * lift + sl.lateral * (dir as f32 * o.x) + sl.up * o.y + sl.forward * o.z;
                let mut to = q - base;
                if to != Vec3::ZERO {
                    to = to.normalize();
                }
                // the pace (0x46398F…0x463A39): the car's velocity along its slice, × the way, × `+0xCC`,
                // a sixty-fourth a step
                let speed = v.velocity.dot(t.slices[car_slice].forward) * dir as f32 * pace;
                // (per tick: this frame's `steps` updates move it along the one heading)
                eye += to * speed * (1.0 / 64.0) * steps as f32;
                tc.tv.spline_eye = Some(eye);
                tc.tv.spline_slice = Some(eye_slice);
                for _ in 0..steps {
                    tc.tv.update(d);
                }
                self.camera.fov_y_radians = fov_of(tc.tv.half_angle);
                slot_near = tc.tv.near;
                self.camera.position = eye;
                self.camera.target = car_pos;
                self.camera.up = Vec3::Y;
            } else if view == 0x0A {
                // **CircleCam 0x462980** (`camera::CircleCam`): the eye circles the car on a 4.5 × 6.0
                // ellipse in its own frame, a lap per 1024 updates, at the heights 1 / 2 / 3 in turn
                // (0x4629E9…0x462ABC). Turned by the car's matrix, its y bounded by the two ceiling
                // tests, the eye is the car plus it (0x462AC0…0x462B72); flags 0x3C give the aim bit, so
                // it looks at the car's origin. It does not call 0x462B80: the half-angle stays
                // SetSlotView's 37.5. SetSlotView zeroes the count — a new view, or in the TV modes
                // every enabled trackside switch (0x4139DD)
                let switches = if tv_mode { self.track_cameras.as_ref().map_or(0, |t| t.tv.switches) } else { 0 };
                let (mut circle, v0, s0) = self.circle;
                if v0 != view || s0 != switches {
                    circle.reset();
                }
                let local = circle.step_count(steps);
                self.circle = (circle, view, switches);
                let mut off = car_rot * local;
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.slot_eye.unwrap_or(car_pos + off);
                    off.y = ceiling(w, car_pos, off.y).min(ceiling(w, old_eye, off.y));
                }
                self.camera.position = car_pos + off;
                self.camera.target = car_pos;
                self.camera.up = Vec3::Y;
            } else if view == 0x0E
                && let Some((eye, m)) = anim_pose
            {
                // **AnimCam 0x462F30** (`camera_anim_step`): the eye and the whole matrix from the
                // animation; the table's flags for 0x0E (0x10, reloaded at 0x463D3A…0x463DC4 since
                // 0x4640B0 left `+0xC0` = 0xFF) carry neither the aim nor the pushes, so only the
                // ground clamp below acts on it
                self.camera.position = eye;
                self.camera.target = eye + m.row(2) * 20.0;
                self.camera.up = m.row(1);
            } else if view == nfs_game::camera::VIEW_DRIVER as u16
                && let Some(dc) = self.driver_cam
            {
                // driver (`BumperCam2`): the car's own `drivercampos` / `drivercamrot`, in the same body
                // frame as the table, turned by the file's x / y / z in degrees: 0x5563D0 / 0x5564A0 /
                // 0x556560 each pre-multiply the slot matrix by 0x532600's family `M_x` / `M_y` / `M_z`
                // (0x461B5A…0x461B8E), so the car's matrix ends up `M_z·M_y·M_x·M` — in this port's
                // column form `car · Rx · Ry · Rz`, the roll innermost
                let aim = Quat::from_rotation_x(dc.rotation.x.to_radians())
                    * Quat::from_rotation_y(dc.rotation.y.to_radians())
                    * Quat::from_rotation_z(dc.rotation.z.to_radians());
                // 0x461A30 does not put the eye at the offset outright: the slot's world offset `+0x08`
                // moves toward `car matrix · offset` by `[0x5B23F4]` 0.2 in x and y and `[0x5B23F0]` 0.5 in
                // z each update (0x461BC5…0x461C25), so the head lags the body a little.
                // with look-back held the table offset's x is 0 (0x461B96…0x461B9F)
                let mut pos = dc.position;
                if rear_view {
                    pos.x = 0.0;
                }
                let target = car_rot * pos;
                // **Looking left or right** (bit 0x14 `[0x5E99B4]` or 0x15 `[0x5E99AC]`, tested at
                // 0x461A90 / 0x461A9D) takes the rigid path 0x461C74: the car's matrix, the look-around
                // turn 0x4611C0, the look-back's x = 0 (0x461CD0), and the eye at `car + M·offset`
                // outright (0x461CE4…0x461D40) — none of the file's `drivercamrot` turns, and the
                // slot's lagged offset `+0x08` neither followed nor moved
                let rigid = look_left || look_right;
                let frame = if rigid {
                    self.camera.position = car_pos + target;
                    car_rot * look
                } else {
                    let mut cur = self.chase_offset.map_or(target, |(o, _)| o);
                    for _ in 0..steps {
                        cur.x += (target.x - cur.x) * 0.2;
                        cur.y += (target.y - cur.y) * 0.2;
                        cur.z += (target.z - cur.z) * 0.5;
                    }
                    self.chase_offset = Some((cur, view));
                    self.camera.position = car_pos + cur;
                    // the look-around yaw turns the car's matrix before the slot's own turns (0x461B07
                    // is ahead of 0x5563D0…0x556560)
                    car_rot * look * aim
                };
                // the view is the slot matrix whole, so its up row carries the turns too (the roll)
                self.camera.target = self.camera.position + frame * Vec3::Z * 20.0;
                self.camera.up = frame * Vec3::Y;
            } else {
                // `TVCam` / `DirectorsCam`: the camera the last camera trigger switched to
                // (0x413B80 → 0x413960), with its own field of view; before any trigger has fired, the
                // nearest element
                let slot_eye = self.slot_eye;
                let cam = self
                    .track_cameras
                    .as_mut()
                    .and_then(|t| {
                        // 0x462E30: the eye rises above where the switch put it by the element's `+0x84`
                        // as the car moves off, `d` measured from last update's eye; then 0x462B80(d)
                        // steps the half-angle (`TvSlot`)
                        let base = t.tv.eye?;
                        let last = slot_eye.filter(|e| e.x == base.x && e.z == base.z).unwrap_or(base);
                        let d = car_pos.distance(last);
                        let lift = t.tv.eye_lift(d);
                        for _ in 0..steps {
                            t.tv.update(d);
                        }
                        Some((base + Vec3::Y * lift, t.tv.half_angle, t.tv.fixed_view, t.tv.frozen, t.tv.near))
                    })
                    .or_else(|| {
                        self.track.cameras.as_ref().and_then(|c| {
                            c.of_kind(&nfs_formats::scn::ElementKind::Camera)
                                .map(|e| Vec3::from(e.position))
                                .min_by(|a, b| {
                                    a.distance_squared(self.car_pos)
                                        .partial_cmp(&b.distance_squared(self.car_pos))
                                        .unwrap()
                                })
                                .map(|p| (p, nfs_game::camera::DEFAULT_HALF_ANGLE, None, false, nfs_game::camera::DEFAULT_NEAR))
                        })
                    });
                match cam {
                    Some((p, fov, fixed, frozen, near)) if p.distance(car_pos) < 400.0 => {
                        // view 12's flags 0x14 carry the aim bit, so 0x464BF0 points it at the car's
                        // origin; a camera whose `+0x88` is 0 instead gets the element's own matrix and
                        // loses the aim bit (0x413A4B…0x413A7F)
                        self.camera.position = p;
                        match fixed {
                            Some((f, u)) => {
                                self.camera.target = p + f * 20.0;
                                self.camera.up = u;
                            }
                            // a disabled `+0x88` 0 camera leaves the last matrix in place
                            None if frozen => {
                                let dir = self.camera.target - self.camera.position;
                                self.camera.target = p + dir;
                            }
                            None => {
                                self.camera.target = car_pos;
                                aimed = true;
                            }
                        }
                        self.camera.fov_y_radians = fov_of(fov);
                        slot_near = near;
                    }
                    _ => {
                        self.camera.position = car_pos - fwd * 7.5 + Vec3::Y * 2.3;
                        self.camera.target = car_pos + Vec3::Y * 0.9;
                    }
                }
            }
            if !last_upd {
                self.slot_eye = Some(self.camera.position);
            }
        }
        // diagnostic (`RUST_LOG=camslot=debug`): the slot's eye `+0x24` after this frame's updates
        tracing::debug!(target: "camslot", tick = self.tick, view, updates = frame_steps, eye = ?self.camera.position, at = ?self.camera.target, up = ?self.camera.up, car = ?car_pos, "camera slot");
        // the ticks this frame's updates consumed are done with
        while self.cam_hist.front().is_some_and(|h| h.tick <= self.tick) {
            self.cam_hist.pop_front();
        }
        // the build's ceiling test runs at the frame's pose
        let ceiling = |w: &nfs_game::world::TrackWorld, p: Vec3, y: f32| -> f32 { ceiling_at(w, p, car_pos, y) };
        // **The ground clamp 0x464330**, which the build 0x464F80 runs on the eye for views above 1
        // other than 0x0C (0x46524C…0x465261), before it aims: of the drivable patches under the eye
        // (0x473650), one whose three corners are all more than `[0x5B24A8]` 1.0 below the eye is
        // passed over (0x464397…0x4643C2); otherwise when its height `h` at the eye (0x48A060) plus
        // 1.0 is above the eye, the eye is lifted to `h + 1` — but only when `h` is within
        // `[0x5B38EC]` 3.5 of the car's ground height `car+0x41C`, unless `[0x5CF584]` is 0, which
        // 0x4642CC sets from `strcmp(track, "castle")`: on Castle any patch lifts it.
        // **The pause orbit** (the dispatcher 0x463C66…0x463C99): while the HUD's dialog queue is
        // up (0x426A60 — the pause menu, a page) outside a replay (`[0x5E99F4] < 2`), the slot runs
        // 0x4617C0 in place of its view's update — except in views 0, 1 and 0x0E, which keep their
        // eye: the yaw `+0xC6` (a word) steps by `+0xC8` (0x46181E) on every update, and the eye
        // is the car plus `M_y(yaw / 1024 turn) · +0x08` (0x461838…0x46189E), the slot's offset
        // from the car as its view left it. The steps are the keys' (`orbit`: +3, −3 or 0) and
        // the update runs once a frame while paused (0x415C69). The aim that follows (0x464BF0)
        // looks at the car again, so an aimed view's target turns with the eye; the other views'
        // matrix is left as it was. `+0x08` (slot `+0x08` = 0x627E88) is the slot's world offset car →
        // eye that the view updates keep — 0x461590 (chase) writes `[+0x24] = car+0x330 + [+0x08]`
        // (0x4616D3…0x461708); the sibling view updates 0x461A30 / 0x461E80 / 0x4623D0 take its address
        // too, and SetSlotView's 0x465CF0 seeds it — so the eye minus the car before the build's pushes
        // is that offset (CONFIRMED 2026-09-30, for the chase-type views). A view whose update does not
        // write it (CircleCam, SplineCam, TVCam, BlimpCam, 0x10…0x13) leaves in the slot what the last writer or
        // the seed put there: `chase_offset`, which the seed 0x465CF0 (see the SetSlotView note above) fills with
        // `-4 · car forward` on leaving a view outside 2…7 (CLOSED 2026-10-02, WP-R) — the orbit turns that, not the
        // eye's own offset.
        // (`cmp [0x5E99F4], 2 ; jge`, 0x463C75: a stream run out is a replay too)
        let replaying = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        if self.vehicle.is_some() && self.hud_dialog_up() && !replaying && !matches!(view, 0x00 | 0x01 | 0x0E) {
            let base = *self.orbit.base.get_or_insert(self.chase_offset.map_or(self.camera.position - car_pos, |(o, _)| o));
            self.orbit.yaw = self.orbit.yaw.wrapping_add(self.orbit.step);
            let turn = Quat::from_rotation_y(self.orbit.yaw as f32 / 1024.0 * std::f32::consts::TAU);
            let eye = car_pos + turn * base;
            if aimed {
                self.camera.target = car_pos + turn * (self.camera.target - car_pos);
            } else {
                self.camera.target += eye - self.camera.position;
            }
            self.camera.position = eye;
        } else {
            self.orbit.base = None;
        }
        // the slot keeps its own eye `+0x24`; the pushes and the clamp below act on the copy the build
        // hands out (0x4651BF…0x4651DB)
        self.slot_eye = Some(self.camera.position);
        // **The build's pushes** (0x4651E4…0x465233), for views whose slot flags carry 0x10 — the
        // table's 0x08: views 2…0x0A, 0x10 and 0x11 (`race-logic.md` §11): the prop push 0x4649E0,
        // then the wall push 0x464500
        // (`WallSet::camera_push`), and when that moved the eye, the ceiling test 0x461370 at the eye
        // on its height over the car
        if matches!(view, 0x02..=0x0A | 0x10 | 0x11)
            && let (Some(w), Some(v)) = (self.world.as_ref(), self.vehicle.as_ref())
        {
            let mut eye = self.camera.position;
            // **0x4649E0, the props**: each tree object near the segment car → eye (0x4739E0) whose
            // radius (slot 5) is 2.0 or more and whose slot-8 answer to the car's velocity is not
            // 3 is cut by the segment (slot 7 — only boxes pass the radius here, the cylinders'
            // being 0.25: 0x486FB0, `box_edge_test`); a cut puts the eye's x and z at the cut point
            // plus the horizontal part of the unit `car − eye` (0x464AD4…0x464B58). The answer is
            // the collision's own, so a crate stack in the speed band starts here as it would there.
            // The tree query 0x4739E0 walks the quadtree cells that overlap the segment's x-z box
            // (0x4848B0 on the min / max of its ends) and keeps each object whose own slot 7 cuts
            // the segment (0x473AB6) — the same cut test as below, so an x-z box test over every prop
            // gives the same set: the tree's insert **0x484AE0** descends into every child that
            // **0x484320** accepts — the object's centre (vtable +4) inside the cell, or else its
            // box (vtable +8) overlapping it — so a prop sits in every cell its box covers
            // (read 2026-09-25; it had been left open whether only the centre's cell held it).
            // Every object is cut by the segment from the car to the eye **as the push found it**:
            // 0x4649E0 copies the eye into a local on entry (0x4649EA…0x4649FC) and hands that copy
            // to the query and to each slot-7 test (0x464AB3…0x464AC9), and takes the direction from
            // it (0x532330 at 0x464AE5: car − entry eye); only the output is written per cut. So a
            // prop listed twice (the tree keeps it in every cell it covers) pushes to the same place
            // twice, and of several props that cut, the last one listed wins — the tree's cell
            // order there, this port's now-real tree query order here (wired 2026-09-28,
            // `RE/RENDERING.md` "The tree, built"). The query also keeps only cell entries whose
            // object byte `+4` is 0 (0x473A95; the constructor 0x486340 writes 0 at 0x486389, no
            // other writer traced) — every `Prop` this port builds qualifies (nothing else writes
            // that byte here either), so no extra filter is needed.
            let e0 = eye;
            for item in w.tree.query_segment((e0.x, e0.z), (v.position.x, v.position.z)) {
                let nfs_physics::track_tree::TreeItem::Prop(pi) = item else { continue };
                let p = &w.props[pi];
                let nfs_physics::props::PropShape::Box { corners, radius } = &p.shape else { continue };
                if *radius < 2.0 || p.answer(self.tick as u32, v.velocity) == 3 {
                    continue;
                }
                let (count, point, _) = nfs_physics::props::box_edge_test(corners, v.position, e0);
                if count == 0 {
                    continue;
                }
                let d = (v.position - e0).normalize_or_zero();
                eye.x = point.x + d.x;
                eye.z = point.z + d.z;
            }
            let castle = self.track.name.eq_ignore_ascii_case("castle");
            if w.walls.camera_push(&mut eye, v.position, v.ground_h, castle, &w.tree) {
                eye.y = v.position.y + ceiling(w, eye, eye.y - v.position.y);
                // the target stays where the view put it: the aim 0x464BF0 looks from the moved eye
            }
            self.camera.position = eye;
        }
        if view > 1
            && view != 0x0C
            && let (Some(w), Some(v)) = (self.world.as_ref(), self.vehicle.as_ref())
        {
            let any_height = self.track.name.eq_ignore_ascii_case("castle");
            let mut eye = self.camera.position;
            for (t, h) in w.collision.patches_at(eye) {
                if t.a.y.max(t.b.y).max(t.c.y) + 1.0 < eye.y {
                    continue;
                }
                if h + 1.0 > eye.y && (any_height || (h - v.ground_h).abs() <= 3.5) {
                    eye.y = h + 1.0;
                }
            }
            self.camera.position = eye;
        }
        // **The playback's wander** (0x464C2B…0x464E42, `camera::ReplayWander`): while a replay
        // plays (`[0x5E99F4] == 2`, not run out) an aimed view whose word `+0xBC` is not 2…7,
        // 0x0A or 0x0B looks at the car's origin plus the wandering offset — the trackside
        // cameras, 8 and 0x0D. The word is what SetSlotView last set: the trackside element's
        // view, or 3 for `DirectorsCam` (0x14) before any trigger (0x414953 → 0x4659E0(0, 3)).
        {
            let word = match (self.camera_view(), self.track_cameras.as_ref().and_then(|t| t.tv.view)) {
                (0x0C | 0x14, Some(v)) => v,
                (0x14, None) => 3,
                (v, _) => v,
            };
            let key = (self.camera_view(), word, self.track_cameras.as_ref().map_or(0, |t| t.tv.switches).wrapping_add(self.slot_key_enters));
            if self.replay_wander.1 != key {
                self.replay_wander = (nfs_game::camera::ReplayWander::default(), key);
            }
            let plays = self.replay.stream.as_ref().is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing);
            if aimed
                && plays
                && nfs_game::camera::ReplayWander::applies(word)
                && let (Some(p), Some(v)) = (self.track_particles.as_mut(), self.vehicle.as_ref())
            {
                // the pause flags `[0x606860]` / `[0x606A98]` (the replay's) and `[0x606A90]`
                let paused = self.paused || self.replay.frozen;
                let off = self.replay_wander.0.step(self.tick as i32, v.horizontal_speed(), self.replay.speed.camera_lag(), paused, p.rng_mut());
                self.camera.target += off;
                tracing::debug!(tick = self.tick, word, ?off, goal = ?self.replay_wander.0.goal, "replay aim wander (0x464C2B)");
            }
        }
        // **The aim 0x464BF0** (`+0xC0 & 0x08`, 0x465269…0x46527F), last in the build: the view
        // looks from the eye at its aim point with the world's up, or — within 0.99 of straight
        // down or up — with last build's up row (`camera::aim_up`)
        if aimed {
            let f = (self.camera.target - self.camera.position).normalize_or_zero();
            if f != Vec3::ZERO {
                self.camera.up = nfs_game::camera::aim_up(f, prev_up);
            }
        }
        // diagnostic (`RUST_LOG=camslot=debug`): the view the build hands out — the orbit, the pushes,
        // the clamp and the aim applied
        tracing::debug!(target: "camslot", tick = self.tick, view, eye = ?self.camera.position, at = ?self.camera.target, up = ?self.camera.up, dialog = self.hud_dialog_up(), "camera built");
        // diagnostic, no original counterpart: `NFS_COP_VIEW=1` looks at the active cop nearest
        // the player from 7 m off its front quarter, 2 m up — the only way to see where a cop
        // sits on the road without a pursuit in front of the chase view
        if nfs_game::config::cfg().nfs_cop_view.var().is_ok()
            && let Some(c) = self.road.police.as_ref().and_then(|p| {
                p.cars.iter().filter(|c| c.active).min_by(|a, b| {
                    a.position.distance_squared(car_pos).partial_cmp(&b.position.distance_squared(car_pos)).unwrap()
                })
            })
        {
            let (right, fwd) = (c.orientation.row(0), c.orientation.row(2));
            self.camera.position = c.position + (fwd + right).normalize_or_zero() * 7.0 + Vec3::Y * 2.0;
            self.camera.target = c.position;
            self.camera.up = Vec3::Y;
        }
        // **The police beacon's projector** 0x4526C0's second half: on a night track
        // (`word [0x628A28] != 0`) it walks the car list and keeps the live car whose `car+0xECC & 2`
        // (lights on) is set and whose octagonal distance to the camera in the xz plane,
        // `max(|dx|,|dz|) + 0.25·min(|dx|,|dz|)` (0x4527DB…0x45284D), is smallest — starting from
        // **180** (`0x43340000`, 0x4527AF), so a farther one lights nothing. Its frame is the car's
        // own (`car+0x330` / `car+0x364`) turned about the car's **y** by `TIMER_getTicks() × 0.02`
        // **turns** (`[0x5B3714]`, through 0x5564A0 whose sine takes turns, `[0x5BB2C0]` = 2π), and
        // 0x539CD0 stores world → that frame in `[0x61E184]`. The 128 Hz TIMER is two of this
        // port's 64 Hz game ticks.
        self.scene.beacon = self.track.is_night().then(|| {
            let cam = self.camera.position;
            // diagnostic: `NFS_BEACON_VIEW=1` spins the beacon on the player's own car, the only
            // way to watch the sweep without a pursuit
            if nfs_game::config::cfg().nfs_beacon_view.var().is_ok() {
                let turns = (self.tick * 2) as f32 * 0.02;
                let rot = car_rot * Quat::from_rotation_y(turns * std::f32::consts::TAU);
                return Some(Mat4::from_rotation_translation(rot, car_pos).inverse());
            }
            let pol = self.road.police.as_ref()?;
            let (car, _) = pol
                .cars
                .iter()
                .filter(|c| c.active && c.lights)
                .filter_map(|c| {
                    let (dx, dz) = ((c.position.x - cam.x).abs(), (c.position.z - cam.z).abs());
                    let d = dx.max(dz) + 0.25 * dx.min(dz);
                    (d < 180.0).then_some((c, d))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
            let turns = (self.tick * 2) as f32 * 0.02;
            let rot = Quat::from_mat3(&car.orientation) * Quat::from_rotation_y(turns * std::f32::consts::TAU);
            Some(Mat4::from_rotation_translation(rot, car.position).inverse())
        }).flatten();
        (view, slot_near)
    }
}
