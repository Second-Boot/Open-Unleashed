//! The cars of one scene pass: `CarSlot`, the per-car `CarDraw` and the pass's shared `CarPassCtx`, the player's, opponents', police
//! and traffic cars' draws (`draw_car`), the body items and the pose blend. Moved out of `render_scene.rs` unchanged apart from item
//! visibility (`pub(super)`: the pass drivers `dyn_items` / `track_and_car_items` stay in `render_scene.rs`).

use crate::*;
use nfs_physics::vehicle::RaceState;

/// ENHANCEMENT E43 [`contact_car`]: a body's own wheel offsets are used while its wheel radius (their `y`) lies in this range (m).
pub(super) const CONTACT_WHEEL_R_VALID: std::ops::Range<f32> = 0.15..0.6;
/// ENHANCEMENT E43, a car with no body: APPROXIMATION (see [`contact_car`]): the wheels this far (m) inside the box's side, at
/// least this far from the centre line, at this share of the half length, of this radius (m).
pub(super) const CONTACT_WHEEL_INSET: f32 = 0.12;
pub(super) const CONTACT_WHEEL_MIN_X: f32 = 0.2;
pub(super) const CONTACT_WHEEL_Z_SHARE: f32 = 0.62;
pub(super) const CONTACT_WHEEL_R: f32 = 0.3;
/// PORT-DEVIATION D8: a car that moved this far (m) or more since the previous tick is drawn at this tick's pose (a reset), not
/// blended ([`App::blend_pose`]).
pub(super) const POSE_RESET_DIST: f32 = 5.0;
/// `rain` (the sky's field 5 at the camera's slice, `weather_strengths`) above which the cars' effect drive takes the road as wet.
pub(super) const FX_WET_RAIN: f32 = 0.1;
/// 0x408495…0x4084A1: the AI's speed (m/s) from which its backing up lights the reverse lamp.
pub(super) const AI_REVERSE_LAMP_SPEED: f32 = 2.0;
/// A cop's wheels are rolled from its speed over this radius (m): a cop has no wheel state of its own.
/// APPROXIMATION (no original counterpart read: the cop's draw pose is the port's).
pub(super) const POLICE_WHEEL_RADIUS: f32 = 0.32;
/// Slot 9's steer channels: a front wheel's `+0xB4` over 216 (channels 1 and 2, 0x438478…0x4384B5).
pub(crate) const STEER_UNITS: f32 = 216.0;
/// The indicators' half period in 64 Hz ticks: `TIMER_getTicks() / (ticksPerSecond/2)` (0x4393C8…0x4393D8), half a second.
pub(super) const BLINK_TICKS: u64 = 32;
/// ENHANCEMENT E15: the camera's own car's spotlights (its headlight pair, sharing the shadow map) before the others'.
pub(super) const OWN_SPOTS: usize = 2;
/// ENHANCEMENT E15: a spot lamp's reach (m), the renderer's `gpu/spot.rs` RANGE: a lamp farther off the view lights nothing on screen.
pub(super) const SPOT_REACH: f32 = 70.0;

/// ENHANCEMENT E43: one car for the contact shadow. `centre` / `rot` / `half` are what the blob quad (`shadow_quad`) is laid from
/// (the body box's centre, its orientation, the box's half sizes: the model origin stands on the ground `half.y` under the centre);
/// the four tyre patches come from the vehicle's own wheel offsets (the model's, set at load) when it has a body.
/// APPROXIMATION (a traffic car with no body, no original counterpart): the wheels at 0.62 of the half length and 0.12 m inside the box's
/// side, tyre radius 0.3 m. Render only, reads the pose.
pub(super) fn contact_car(centre: Vec3, rot: Quat, half: Vec3, body: Option<&nfs_physics::vehicle::Vehicle>) -> nfs_renderer::ContactCar {
    let axes = [rot * Vec3::X, rot * Vec3::Y, rot * Vec3::Z];
    let (wheels, wheel_r) = match body {
        Some(v) if CONTACT_WHEEL_R_VALID.contains(&v.wheel_offsets[0].y) => (std::array::from_fn(|k| [v.wheel_offsets[k].x, v.wheel_offsets[k].z]), v.wheel_offsets[0].y),
        _ => {
            let (x, z) = ((half.x - CONTACT_WHEEL_INSET).max(CONTACT_WHEEL_MIN_X), half.z * CONTACT_WHEEL_Z_SHARE);
            ([[-x, z], [x, z], [-x, -z], [x, -z]], CONTACT_WHEEL_R)
        }
    };
    nfs_renderer::ContactCar { origin: centre - axes[1] * half.y, axes, half: [half.x, half.z], wheels, wheel_r }
}

/// [`slot_model`] with the car's LOD state.
pub(super) fn slot_model_lod<'a>(own: &'a Option<CarModel>, own_lod: &'a mut nfs_game::world::CarLodState, road: &'a mut RoadCars, slot: CarSlot, model: usize) -> (Option<&'a CarModel>, &'a mut nfs_game::world::CarLodState) {
    match slot {
        CarSlot::Player => (own.as_ref(), own_lod),
        CarSlot::Opponent(i) => (road.opponent_models.get(model), &mut road.opponent_lods[i]),
        CarSlot::Police(i) => (road.police_models.get(model), &mut road.police_lods[i]),
        CarSlot::Traffic(i) => (road.traffic_models.get(model), &mut road.traffic_lods[i]),
    }
}

/// The model a slot draws: the player's own, an opponent's, the traffic / police type's shared one (`model` its index).
pub(super) fn slot_model<'a>(own: &'a Option<CarModel>, road: &'a RoadCars, slot: CarSlot, model: usize) -> Option<&'a CarModel> {
    match slot {
        CarSlot::Player => own.as_ref(),
        CarSlot::Opponent(_) => road.opponent_models.get(model),
        CarSlot::Police(_) => road.police_models.get(model),
        CarSlot::Traffic(_) => road.traffic_models.get(model),
    }
}

/// Each wheel's suspension compression (the LOD pick's wheel lift, `update_travel`).
pub(super) fn wheel_compression(v: &nfs_physics::vehicle::Vehicle) -> [f32; 4] {
    std::array::from_fn(|k| v.wheels[k].compression)
}

/// Each wheel's `+0x70`, channels 1...4's and 18...21's `.z` (0x4384F3...0x43852F).
pub(super) fn wheel_roll(v: &nfs_physics::vehicle::Vehicle) -> [f32; 4] {
    std::array::from_fn(|k| v.wheels[k].render_angle)
}

/// How a car's passes are found.
#[derive(Clone, Copy)]
pub(super) enum Pick {
    /// the LOD pick 0x439F90 for a car of this kind, then `race_passes`
    Lod(nfs_game::world::CarKind),
    /// the followed car in the driver view: the pick skipped, the cabin passes (0x43A020, 0x444E18)
    InCar,
    /// nothing of the body is drawn (the followed car in the bumper view, 0x4430D3; a car the pass does not queue)
    Hidden,
}

/// One car's lamp state for [`App::lamp_slots_tunnel`].
#[derive(Clone, Copy)]
pub(super) struct LampInput {
    pub(super) headlights: bool,
    /// the headlights take the player's ramp (`head_ramp`) instead of the race's shared one
    pub(super) ramp: bool,
    pub(super) brake: bool,
    pub(super) reverse: bool,
    pub(super) signals: (bool, bool, bool),
    pub(super) damage: Option<[f32; 10]>,
}

/// One car of a pass, as [`App::draw_car`] draws it (plain data: read off the car before the draw).
pub(super) struct CarDraw {
    pub(super) slot: CarSlot,
    /// the model's index in the slot's model list (unused for the player)
    pub(super) model: usize,
    /// the draw pose (`car+0x330 − M[1]·car+0x3AC`, 0x43A410), blended (PORT-DEVIATION D8)
    pub(super) pose: (Vec3, Quat),
    /// `car+0x330`: the blob shadow's and the contact shadow's place
    pub(super) centre: Vec3,
    pub(super) half: Vec3,
    /// ENHANCEMENT E43 / E51: the car's contact-shadow pose (made only while either is on)
    pub(super) contact: Option<nfs_renderer::ContactCar>,
    /// no shadow and no contact shadow (0x437ABD: the followed car in the bumper and driver views)
    pub(super) no_shadow: bool,
    pub(super) pick: Pick,
    pub(super) compression: [f32; 4],
    /// the wheels' spin, the front wheels' steer (in turns) and each wheel's roll
    pub(super) wheels: ([f32; 4], [f32; 2], [f32; 4]),
    /// where the light block and the headlight projector are read (`car_body_items`)
    pub(super) at: Vec3,
    /// the sky map's origin and slice (`apply_sky`)
    pub(super) sky: Option<(Vec3, usize)>,
    /// a traffic / police car's own damage, baked into the shared model before its draw
    pub(super) shared_damage: Option<[f32; 10]>,
    /// the effect drive (its set, the car as 0x43EF80 reads it), when this pass runs it
    pub(super) fx: Option<(usize, nfs_game::car_effects::CarInput)>,
    pub(super) lamps: LampInput,
    /// `car+0x41C`, the ground the lamp reflections lie on
    pub(super) ground: f32,
    /// the player's windscreen (the splats, ENHANCEMENT E40's drops)
    pub(super) splat: bool,
}

/// The parts of a kinematic AI car's draw ([`App::ai_car`]) that differ between the traffic and the cops.
pub(super) struct AiCarDraw<'a> {
    pub(super) slot: CarSlot,
    pub(super) kind: nfs_game::world::CarKind,
    pub(super) pose: (Vec3, Quat),
    pub(super) centre: Vec3,
    pub(super) half: Vec3,
    pub(super) model: usize,
    pub(super) slice: usize,
    pub(super) brake: bool,
    pub(super) reverse: bool,
    pub(super) spin: f32,
    pub(super) body: Option<&'a nfs_physics::vehicle::Vehicle>,
}

/// What every car of one pass shares (`dyn_items`).
pub(super) struct CarPassCtx {
    pub(super) main: bool,
    /// the pass's frustum (0x4430A0's test 0x539480 on `car+0x330`, radius `car+0x3B4`)
    pub(super) cull: Option<nfs_game::world::CullView>,
    /// the player's draw pose, its `car+0x330` and orientation (`DynPose`)
    pub(super) local: (Vec3, Vec3, Quat),
    /// (traffic, police, opponents)
    pub(super) counts: (usize, usize, usize),
    pub(super) followed: Option<usize>,
    pub(super) own_view: u16,
    pub(super) replaying: bool,
    pub(super) blink: bool,
    pub(super) spots_on: bool,
    pub(super) contact_on: bool,
    pub(super) sun_cars_on: bool,
    pub(super) eye: Vec3,
    pub(super) fx_wet: bool,
    pub(super) alpha: f32,
    pub(super) lamp_ms: f64,
    /// the race's headlight ramp (every car's first lamp update is the same frame)
    pub(super) fade: f32,
    pub(super) detail: u8,
    pub(super) lod_view: Option<nfs_game::world::LodView>,
    pub(super) dusk_lamps: bool,
    pub(super) mirror: bool,
    pub(super) looking: bool,
    pub(super) shadow_material: Option<nfs_renderer::MaterialId>,
}

/// The lists one pass fills car by car (the scratch buffers are `App::scratch`'s, taken for the pass).
pub(super) struct PassLists {
    pub(super) car_items: Vec<nfs_renderer::DrawItem>,
    pub(super) shadow_verts: Vec<nfs_renderer::Vertex>,
    pub(super) lamp_draws: Vec<LampDraw>,
    pub(super) contact_cars: Vec<(f32, nfs_renderer::ContactCar)>,
    pub(super) sun_cars: Vec<(f32, nfs_renderer::SunCar)>,
    pub(super) spot_others: Vec<nfs_renderer::SpotLamp>,
    pub(super) cone_log: Vec<crate::app::car_lights::BeamLog>,
    pub(super) lamp_used: [usize; 2],
    pub(super) shadow_used: [usize; 7],
}

impl CarSlot {
    pub(crate) fn id(self) -> u32 {
        match self {
            CarSlot::Player => 0,
            CarSlot::Opponent(i) => 1 + i as u32,
            CarSlot::Traffic(i) => 0x100 + i as u32,
            CarSlot::Police(i) => 0x200 + i as u32,
        }
    }

    /// The car's effect set (`car_effects`): the player 0, the opponents, the cops, the traffic; `counts` = (traffic, police,
    /// opponents).
    pub(super) fn effect_index(self, counts: (usize, usize, usize)) -> usize {
        match self {
            CarSlot::Player => 0,
            CarSlot::Opponent(i) => 1 + i,
            CarSlot::Police(i) => 1 + counts.2 + i,
            CarSlot::Traffic(i) => 1 + counts.2 + counts.1 + i,
        }
    }

    /// ENHANCEMENT E51: the car's entry of the sun-share list.
    pub(super) fn sun_car(self, pose: nfs_renderer::ContactCar) -> nfs_renderer::SunCar {
        match self {
            CarSlot::Player => nfs_renderer::SunCar::player(pose),
            CarSlot::Opponent(i) => nfs_renderer::SunCar::opponent(i, pose),
            CarSlot::Police(i) => nfs_renderer::SunCar::police(i, pose),
            CarSlot::Traffic(i) => nfs_renderer::SunCar::traffic(i, pose),
        }
    }
}

impl CarPassCtx {
    /// The pass's frustum test of a car at `pos` (its bounding sphere from the half extents); every car passes without a frustum.
    pub(super) fn seen(&self, pos: Vec3, half: Vec3) -> bool {
        self.cull.as_ref().is_none_or(|cv| cv.sphere_visible(pos, half.length()))
    }

    /// ENHANCEMENT E43 / E51: whether the cars' contact-shadow poses are made.
    pub(super) fn pose_on(&self) -> bool {
        self.contact_on || self.sun_cars_on
    }

    /// PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: a car's draw pose from its
    /// previous tick's and its orientation matrix ([`App::blend_pose`]).
    pub(super) fn blend(&self, prev: Option<&(Vec3, Quat)>, origin: Vec3, orientation: &glam::Mat3) -> (Vec3, Quat) {
        App::blend_pose(prev, origin, Quat::from_mat3(&orientation.transpose()).normalize(), self.alpha)
    }

    /// The car's passes, `None` = nothing of its body: 0x43A3B0 stores the class, then 0x43E670 lifts the wheels from the LODs
    /// below it.
    pub(super) fn passes(&self, model: &CarModel, state: &mut nfs_game::world::CarLodState, pick: Pick, (p, r): (Vec3, Quat), half: Vec3, compression: [f32; 4]) -> Option<Vec<nfs_game::world::CarPass>> {
        match pick {
            Pick::Hidden => None,
            Pick::InCar => {
                model.update_travel(state, compression, self.mirror);
                Some(nfs_game::world::in_car_passes(state.class, self.detail, self.looking))
            }
            Pick::Lod(kind) => {
                let class = model.lod_class(self.lod_view.as_ref()?, p, r, half, self.detail, kind)?;
                state.class = class;
                model.update_travel(state, compression, self.mirror);
                Some(nfs_game::world::race_passes(class, self.detail, kind == nfs_game::world::CarKind::Traffic))
            }
        }
    }
}

/// Which car of the draw list one [`App::draw_car`] call draws. Its [`id`](Self::id) is the car's stable key: the tunnel-lamp and
/// spot-cover state (E24 / E15), the beam strips' log (ENH-CONES) and the motion-id group (E37).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CarSlot {
    Player,
    Opponent(usize),
    Police(usize),
    Traffic(usize),
}

impl App {
    pub(crate) fn update_damage_decals(&mut self, anim_ms: u32) {
        let Some(damage) = self.vehicle.as_ref().map(|v| v.damage) else {
            return;
        };
        // `CARDETAIL` `[0x6197C4]`: the crush 0x43E390 needs it above 0
        let detail = self.car_detail();
        // 0x4382EC: the zone caps of the car the model's arms follow (0x465D80(0))
        let followed = self.followed_seat();
        if let (Some(c), Some(r)) = (self.car_model.as_mut(), self.renderer.as_mut()) {
            c.update_damage(r, &damage, followed.is_none(), detail);
        }
        if let Some(r) = self.renderer.as_mut() {
            for (i, o) in self.road.opponents.iter().enumerate() {
                if let Some(m) = self.road.opponent_models.get_mut(o.model) {
                    m.update_damage(r, &o.vehicle.damage, followed == Some(i), detail);
                }
            }
        }
        // slot 9's race animations (the wing off the speed gate, a tiny car's aerial) and slot 6's
        // pop-up headlights off each car's lamp bit (`car-model.md`)
        let speed = self.vehicle.as_ref().map(|v| v.speed()).unwrap_or(0.0);
        let reverse = self.vehicle.as_ref().is_some_and(|v| v.gear == 0);
        // slot 6's headlight bit is the car's own lamp byte, `car+0xD87 & 1` (0x4394CC…0x4394D2,
        // compared with `R3DCar+0x567` at 0x43950E), not the track's night flag: `Car_init` 0x410C74
        // starts it from the flag and the lights key (0x49941D) toggles it
        let night = self.lamp_bits & lamp_bit::HEADLIGHTS != 0;
        let tiny = self.cheat_bits & CHEAT_TINY != 0;
        // slot 9's driver: `[0x6197C4]` is `CARDETAIL`, the `Graphics.CarDetail` choice (0x44EF80), `detail` above
        let driver = self.vehicle.as_ref().map(|v| nfs_game::world::DriverInputs {
            detail,
            // channels 1 and 2's `.y`: each front wheel's `+0xB4` over 216, negated when mirrored
            wheel_turns: (v.wheels[0].steer + v.wheels[1].steer) / STEER_UNITS * if self.mirror { -1.0 } else { 1.0 },
            shift_timer: v.shift_timer,
            racing: v.race_state == RaceState::Racing && !self.paused,
            handbrake: v.handbrake,
            reverse: v.gear == 0,
            long_acc: v.long_filtered,
            lat_acc: v.lateral_filtered,
        });
        if let (Some(c), Some(r)) = (self.car_model.as_mut(), self.renderer.as_mut()) {
            c.update_animation(r, anim_ms, speed, night, tiny, reverse);
            if let Some(d) = driver {
                c.update_driver(r, anim_ms, &d);
            }
            // the shared traffic and police models take slot 9's clock channel (the siren)
            for m in self.road.police_models.iter_mut().chain(self.road.traffic_models.iter_mut()) {
                m.update_clock_channels(anim_ms);
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
                m.update_animation(r, anim_ms, v.speed(), o.lamp_bits & lamp_bit::HEADLIGHTS != 0, o.tiny, v.gear == 0);
                m.update_driver(
                    r,
                    anim_ms,
                    &nfs_game::world::DriverInputs {
                        detail,
                        wheel_turns: (v.wheels[0].steer + v.wheels[1].steer) / STEER_UNITS * k,
                        shift_timer: v.shift_timer,
                        racing: v.race_state == RaceState::Racing && !self.paused,
                        handbrake: v.handbrake,
                        reverse: v.gear == 0,
                        long_acc: v.long_filtered,
                        lat_acc: v.lateral_filtered,
                    },
                );
                let ws = wheel_steer(v);
                m.update_race_channels(
                    anim_ms,
                    [ws[0] / STEER_UNITS * k, ws[1] / STEER_UNITS * k],
                    [v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation],
                    [v.wheels[0].render_angle, v.wheels[1].render_angle, v.wheels[2].render_angle, v.wheels[3].render_angle],
                    v.rpm,
                    v.speed(),
                );
            }
            if let Some(v) = self.vehicle.as_ref() {
                let ws = wheel_steer(v);
                c.update_race_channels(
                    anim_ms,
                    [ws[0] / STEER_UNITS * k, ws[1] / STEER_UNITS * k],
                    [v.wheels[0].rotation, v.wheels[1].rotation, v.wheels[2].rotation, v.wheels[3].rotation],
                    [v.wheels[0].render_angle, v.wheels[1].render_angle, v.wheels[2].render_angle, v.wheels[3].render_angle],
                    v.rpm,
                    v.speed(),
                );
            }
        }
    }

    /// The draw of one kinematic AI car (traffic or police) of a pass: the two kinds differ only in the pick (`kind`), the wheels' spin
    /// source and whether a body (the traffic's may be missing before the grid) backs the car.
    pub(super) fn ai_car(&self, ctx: &CarPassCtx, a: AiCarDraw) -> CarDraw {
        let AiCarDraw { slot, kind, pose: (p, r), centre, half, model, slice, brake, reverse, spin, body } = a;
        CarDraw {
            slot,
            model,
            pose: (p, r),
            centre,
            half,
            // ENHANCEMENT E43: the soft contact shadow stands in for the blob patch
            contact: ctx.pose_on().then(|| contact_car(centre, r, half, body)),
            no_shadow: false,
            // the model at `car+0x330 - M[1]*car+0x3AC` (0x43A410), LOD 1's box (traffic) / LOD 5's (a cop)
            pick: Pick::Lod(kind),
            compression: body.map_or([0.0; 4], wheel_compression),
            wheels: ([spin; 4], [0.0; 2], body.map_or([0.0; 4], wheel_roll)),
            at: p,
            sky: Some((centre, slice)),
            // `R3DCar` slot 7 (0x4383CC/0x438273) recomputes a car's decals and crush from
            // *its own* raw zone damage every frame it is drawn, with no car-kind gate - the
            // traffic and the cops share one upload per type, so this instance's own damage
            // (`Vehicle::damage`, `TrafficSet::post_impact`) is baked into that shared model
            // right before this car's own `draw_items`, the same way `update_travel`
            // already is: two cars of one type drawn in the same frame each get their own
            // decals/crush because the model is mutated-then-drawn once per car, in order -
            // and, when they are simultaneously visible, each into its own instantiated copy
            // of the shared model's mesh/texture/material ids (`car-model.md` 13.1's residual
            // gap), so this port's batched `render()` does not alias one onto the other.
            shared_damage: Some(body.map_or([0.0; 10], |b| b.damage)),
            fx: body.filter(|_| ctx.main).map(|b| (slot.effect_index(ctx.counts), nfs_game::car_effects::CarInput::from_vehicle(b))),
            lamps: LampInput { headlights: self.track.is_night(), ramp: false, brake, reverse, signals: (false, false, false), damage: None },
            // `car+0x41C`, the ground height the reflection 0x451D60 lays its quad on, is the
            // body's (`Vehicle::ground_h`); a car without a body (no grid) takes its origin's height
            ground: body.map_or(p.y, |b| b.ground_h),
            splat: false,
        }
    }

    /// The traffic of a pass ([`Self::dyn_items`]), from its last slot.
    pub(super) fn traffic_cars(&mut self, ctx: &CarPassCtx, acc: &mut PassLists, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        for i in (0..ctx.counts.0).rev() {
            let Some(tr) = self.road.traffic.as_ref() else { break };
            let c = &tr.cars[i];
            if !c.active || !ctx.seen(c.position, c.half_extents) {
                continue;
            }
            let body = tr.bodies.get(i);
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the draw pose is the previous tick's blended toward this one by `alpha`; the shadow follows it
            let (p, r) = ctx.blend(self.road.traffic_prev.get(i), c.draw_origin(), &c.orientation);
            let centre = p + (c.position - c.draw_origin());
            // a kinematic car has no wheel state: the wheels are rolled from the distance driven
            let spin = self.road.traffic_spin.get(i).copied().unwrap_or(0.0);
            // the brake lamps follow the AI's own counter `car+0xD80` (0x439360, `aidrive::brake_light`);
            // the reverse lamp its gear byte `car+0xD82`, 0 once the race is on while the AI
            // backs up at 2 m/s or more (0x408495…0x4084A1 → 0x408130), refreshed every
            // frame: its timer `car+0xEDC` is only ever cleared (0x406D22, 0x406E04), so
            // the decrement 0x40847E, taken while it is positive, never runs.
            // `car+0x41C`, the ground height the reflection 0x451D60 lays its quad on, is the
            // body's (`Vehicle::ground_h`, `Car_ground` on `TrafficSet::bodies` since
            // 2026-09-26); a car without a body (no grid) takes its origin's height
            let reverse = self.tick >= RACE_START_TICK && c.gear.sense < 0.0 && c.speed_estimate() >= AI_REVERSE_LAMP_SPEED;
            let car = self.ai_car(ctx, AiCarDraw {
                slot: CarSlot::Traffic(i),
                kind: nfs_game::world::CarKind::Traffic,
                pose: (p, r),
                centre,
                half: c.half_extents,
                model: c.model,
                slice: c.slice,
                brake: c.brake_light > 0,
                reverse,
                spin,
                body,
            });
            self.draw_car(ctx, acc, queue, car);
        }
    }

    /// The cops of a pass, from the last.
    pub(super) fn police_cars(&mut self, ctx: &CarPassCtx, acc: &mut PassLists, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        for i in (0..ctx.counts.1).rev() {
            let Some(pol) = self.road.police.as_ref() else { break };
            let c = &pol.cars[i];
            if !c.active || !ctx.seen(c.position, c.half_extents) {
                continue;
            }
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the draw pose is the previous tick's blended toward this one by `alpha`; the shadow follows it
            let (p, r) = ctx.blend(self.road.police_prev.get(i), c.draw_origin(), &c.orientation);
            let centre = p + (c.position - c.draw_origin());
            let spin = c.speed.abs() * self.tick as f32 / TICKS_PER_SEC_F32 / POLICE_WHEEL_RADIUS;
            // the lamps as the traffic's (above): the AI's brake counter and gear byte, the
            // reflection on the body's `car+0x41C`
            let reverse = self.tick >= RACE_START_TICK && c.gear.sense < 0.0 && c.speed_estimate() >= AI_REVERSE_LAMP_SPEED;
            let car = self.ai_car(ctx, AiCarDraw {
                slot: CarSlot::Police(i),
                // a cop is `car+0x52C & 0x20`, neither the opponent nor the traffic bit: LOD 5's box like a racer's
                kind: nfs_game::world::CarKind::Racer,
                pose: (p, r),
                centre,
                half: c.half_extents,
                model: c.model,
                slice: c.slice,
                brake: c.brake_light > 0,
                reverse,
                spin,
                body: Some(&c.body),
            });
            self.draw_car(ctx, acc, queue, car);
        }
    }

    /// The opponents of a pass, from the last.
    pub(super) fn opponent_cars(&mut self, ctx: &CarPassCtx, acc: &mut PassLists, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        for i in (0..ctx.counts.2).rev() {
            let o = &self.road.opponents[i];
            let v = &o.vehicle;
            // 0x465D80(0) may be this seat's car (`followed`): then the arms of 0x437ABD / 0x43A020 / 0x4430D3 /
            // 0x444E18 / 0x4383EA apply to it, as they do to the local car in `followed`'s `None` arm
            let followed_here = ctx.followed == Some(i);
            // the mirror's pass: the followed car returns at 0x4430EC, the rest take the frustum test
            // (the main pass: the followed car skips the test in views 0 / 1, `[0x627F43] & 8`, 0x4430DD)
            let in_car = followed_here && ctx.own_view <= 1;
            if !ctx.main && followed_here || !(in_car || ctx.seen(v.position, v.half_extents)) {
                continue;
            }
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the draw pose is the previous tick's blended toward this one by `alpha`; the shadow follows it
            let (p, r) = ctx.blend(self.road.opp_prev.get(i), v.draw_origin(), &v.orientation);
            let centre = p + (v.position - v.draw_origin());
            let w = &v.wheels;
            // the front wheels' `+0xB4` over 216 (slot 9's channels 1 and 2), in a replay the
            // smoothed `car+0xAC8` / `+0xACC` (0x438478…0x4384B5)
            let st = if ctx.replaying { v.steer_smoothed } else { [w[0].steer, w[1].steer] };
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x438478…0x4384B5]: the angles lerped from the previous tick's, as the body is
            let st = Self::blend_steer(self.road.opp_steer_prev.get(i), st, ctx.replaying, ctx.alpha);
            let out = v.race_state == RaceState::Out;
            // the car's own lamp byte (`car+0xD87`, the night flag at `Car_init`): the indicators too
            let bits = o.lamp_bits;
            let car = CarDraw {
                slot: CarSlot::Opponent(i),
                model: o.model,
                pose: (p, r),
                centre,
                half: v.half_extents,
                // ENHANCEMENT E43: the soft contact shadow stands in for the blob patch
                contact: ctx.pose_on().then(|| contact_car(centre, r, v.half_extents, Some(v))),
                // 0x437ABD: no shadow under the followed car in the bumper and driver views (`[0x627F43] & 8`)
                no_shadow: in_car,
                pick: match (followed_here, ctx.own_view) {
                    // 0x4430D3: the followed car in the bumper view is not drawn at all
                    (true, 0) => Pick::Hidden,
                    // the driver view: the pick is skipped and the draw takes its cabin passes (0x43A020, 0x444E18)
                    (true, 1) => Pick::InCar,
                    _ => Pick::Lod(nfs_game::world::CarKind::Opponent),
                },
                compression: wheel_compression(v),
                wheels: (std::array::from_fn(|k| w[k].rotation), st.map(|s| s / STEER_UNITS), wheel_roll(v)),
                at: p,
                sky: Some((centre, o.progress.slice)),
                shared_damage: None,
                fx: (ctx.main && (!followed_here || nfs_game::camera::draws_own_car(ctx.own_view))).then(|| (CarSlot::Opponent(i).effect_index(ctx.counts), nfs_game::car_effects::CarInput::from_vehicle(v))),
                lamps: LampInput {
                    headlights: bits & lamp_bit::HEADLIGHTS != 0,
                    ramp: false,
                    brake: v.brake_lamp(),
                    reverse: v.gear == 0,
                    signals: (out || bits & lamp_bit::SIGNAL_LEFT != 0, out || bits & lamp_bit::SIGNAL_RIGHT != 0, ctx.blink),
                    damage: Some(v.damage),
                },
                ground: v.ground_h,
                splat: false,
            };
            self.draw_car(ctx, acc, queue, car);
        }
    }

    /// The player's car (slot 0), the list's tail.
    pub(super) fn player_car(&mut self, ctx: &CarPassCtx, acc: &mut PassLists, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        let (local_draw, local_pos, local_rot) = ctx.local;
        // the player's car, slot 0, the list's tail
        // the mirror's pass: the camera's own car (`followed` none) is not queued at all
        // (0x4430EC), any other goes through the frustum test
        // (the main pass: with the arms following another seat's car the local car is an ordinary
        // one and takes the frustum test; the camera's own car takes it only outside views 0 / 1 —
        // there it is in view by construction)
        let own_unseen = || !ctx.seen(local_pos, self.vehicle.as_ref().map_or(Vec3::ZERO, |v| v.half_extents));
        let own_skipped = if ctx.main { ctx.followed.is_some() && own_unseen() } else { ctx.followed.is_none() || own_unseen() };
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
        let gate_view = if ctx.followed.is_none() { ctx.own_view } else { u16::MAX };
        let v = self.vehicle.as_ref();
        let out = v.is_some_and(|v| v.race_state == RaceState::Out);
        // the player's own lamp byte (`lamp_command`): the headlights and their ramp (a pop-up
        // car's only, 0x439627), and the indicators — both of them while eliminated (0x439408)
        let bits = self.lamp_bits;
        let car = CarDraw {
            slot: CarSlot::Player,
            model: 0,
            pose: (local_draw, local_rot),
            centre: local_pos,
            half: v.map_or(Vec3::ZERO, |v| v.half_extents),
            // ENHANCEMENT E43: the soft contact shadow stands in for the blob patch
            contact: v.filter(|_| ctx.pose_on() && !own_skipped).map(|v| contact_car(local_pos, local_rot, v.half_extents, Some(v))),
            // 0x437ABD: no shadow under the followed car in the bumper and driver views (`[0x627F43] & 8`)
            no_shadow: v.is_none() || ctx.followed.is_none() && ctx.own_view <= 1 || own_skipped,
            pick: match gate_view {
                _ if own_skipped => Pick::Hidden,
                0 => Pick::Hidden,
                1 => Pick::InCar,
                _ => Pick::Lod(nfs_game::world::CarKind::Racer),
            },
            compression: v.map_or([0.0; 4], wheel_compression),
            wheels: match v {
                Some(v) => {
                    // in a replay the smoothed `car+0xAC8` / `+0xACC` (0x438478…0x4384B5)
                    let st = if ctx.replaying { v.steer_smoothed } else { [v.wheels[0].steer, v.wheels[1].steer] };
                    // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x438478…0x4384B5]: lerped from the previous tick's angles, as the body is
                    let st = Self::blend_steer(self.road.player_steer_prev.as_ref(), st, ctx.replaying, ctx.alpha);
                    (std::array::from_fn(|k| v.wheels[k].rotation), st.map(|s| s / STEER_UNITS), wheel_roll(v))
                }
                None => ([0.0; 4], [0.0; 2], [0.0; 4]),
            },
            // the car's light block — the ground under it times the `.map` colours, and the sun of
            // the track's sky (`RE/notes/lighting.md` §6)
            at: local_pos,
            sky: self.race.as_ref().map(|r| (local_pos, r.slice)),
            shared_damage: None,
            // the effect drive, but not for the camera's own car in the bumper and driver views
            // (0x4383D8…0x43840C: `carInfo+0x520 == 0x465D80(0)` and the view id 0 or 1)
            fx: v.filter(|_| ctx.main && nfs_game::camera::draws_own_car(gate_view)).map(|v| (CarSlot::Player.effect_index(ctx.counts), nfs_game::car_effects::CarInput::from_vehicle(v))),
            lamps: LampInput {
                headlights: bits & lamp_bit::HEADLIGHTS != 0,
                ramp: true,
                brake: v.is_some_and(|v| v.brake_lamp()),
                reverse: v.is_some_and(|v| v.gear == 0),
                signals: (out || bits & lamp_bit::SIGNAL_LEFT != 0, out || bits & lamp_bit::SIGNAL_RIGHT != 0, ctx.blink),
                damage: v.map(|v| v.damage),
            },
            ground: v.map_or(local_draw.y, |v| v.ground_h),
            // R3DCar slot 14, 0x43A490, run by the car's draw 0x443490 inside its model transform: in
            // the driver view the `Wndo` corners go to the windscreen drops (0x459200 → 0x457E60).
            // *(Corrected 2026-09-26: this drew the `Wndo` quad itself with `INTGLASS`'s `glai` in views
            // 0 and 1. The quad is never drawn — 0x457E60 only builds the drops' plane from it — and
            // 0x43A490 also needs the slot's view id non-zero (0x465770(0)), so view 1 alone.)*
            splat: ctx.followed.is_none() && ctx.main,
        };
        self.draw_car(ctx, acc, queue, car);
    }

    /// One car's whole draw 0x4430A0 — its shadow (the blob patch, or ENHANCEMENT E43's contact shadow), its body, its lamps —
    /// as one `+0xC` entry of the pass (0x4430A0 → 0x443229, key `round(d²)` 0x4431F8, from the eye to the car's draw position):
    /// queued with that key and merged with the track's far list and the skid marks farthest-first (`blended_dyn_items`).
    pub(super) fn draw_car(&mut self, ctx: &CarPassCtx, acc: &mut PassLists, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>, car: CarDraw) {
        let (p, r) = car.pose;
        let id = car.slot.id();
        acc.car_items.clear();
        let contact = ctx.contact_on && !car.no_shadow && car.contact.is_some_and(|ca| {
            Self::push_near(&mut acc.contact_cars, ctx.eye, ca.origin, ca);
            true
        });
        if let Some(ca) = car.contact.filter(|_| ctx.sun_cars_on) {
            Self::push_near(&mut acc.sun_cars, ctx.eye, ca.origin, car.slot.sun_car(ca));
        }
        if !contact
            && !car.no_shadow
            && let Some(mat) = ctx.shadow_material
            && let Some(q) = self.shadow_quad(car.centre, r, car.half, &mut acc.shadow_verts)
        {
            Self::push_shadow(&mut acc.car_items, self.renderer.as_mut(), &mut self.draws.shadow_meshes, &mut acc.shadow_used, mat, (q, acc.shadow_verts.as_slice()), &self.scene.cover);
        }
        let key = (p - ctx.eye).length_squared().round_ties_even() as u32;
        if car.splat {
            self.splat_items(r, p);
            // ENHANCEMENT E40: the refractive drops on the same glass
            self.drops_items(r, p, ctx.alpha);
        }
        if let (Some(dmg), Some(rr)) = (car.shared_damage, self.renderer.as_mut()) {
            let shared = match car.slot {
                CarSlot::Traffic(_) => self.road.traffic_models.get_mut(car.model),
                CarSlot::Police(_) => self.road.police_models.get_mut(car.model),
                _ => None,
            };
            if let Some(m) = shared {
                m.update_damage(rr, &dmg, false, ctx.detail);
            }
        }
        let picked = match slot_model_lod(&self.car_model, &mut self.car_lod, &mut self.road, car.slot, car.model) {
            (Some(model), state) => ctx.passes(model, state, car.pick, (p, r), car.half, car.compression).map(|passes| (passes, state.travel)),
            (None, _) => None,
        };
        if let Some((passes, travel)) = picked {
            if let (Some((index, input)), Some(fx)) = (car.fx, self.car_effects.as_mut()) {
                fx.drive(index, &input, ctx.eye, ctx.fx_wet, true);
            }
            let LampInput { headlights, ramp, brake, reverse, signals, damage } = car.lamps;
            let extent = slot_model(&self.car_model, &self.road, car.slot, car.model).is_some_and(|m| m.has_headlight_extent());
            let fade = match (extent, ramp) {
                (false, _) => 1.0,
                (true, true) => self.head_ramp.value(headlights, ctx.lamp_ms),
                (true, false) => ctx.fade,
            };
            let tl = self.tunnel_lit(id, p, ctx.lamp_ms); // ENHANCEMENT E24
            let slots = Self::lamp_slots_tunnel(tl, headlights, fade, brake, reverse, signals, damage.as_ref());
            let block = self.world.as_ref().map(|w| w.car_light(car.at));
            let own = car.slot == CarSlot::Player && ctx.main;
            if let Some(model) = slot_model(&self.car_model, &self.road, car.slot, car.model) {
                let (spin, steer, roll) = car.wheels;
                acc.car_items.extend(self.car_body_items(model, block.as_ref(), (Mat4::from_rotation_translation(r, p), spin, steer, roll, travel), &passes, car.at, car.sky, own));
                self.lamp_draws(model, p, r, car.ground, slots, &mut acc.lamp_draws);
                if ctx.spots_on && car.slot != CarSlot::Player {
                    Self::spot_lamps(model, p, r, &slots, self.spot_gain(id, p, ctx.lamp_ms), &mut acc.spot_others);
                }
            }
            Self::push_lamp_draws(&mut acc.car_items, self.renderer.as_mut(), &self.draws.lamp_pools, &mut acc.lamp_used, &mut acc.lamp_draws, ctx.spots_on.then_some((&mut acc.cone_log, id))); // ENHANCEMENT E15 (ENH-CONES)
        }
        // ENHANCEMENT E37: a stable id per car part for the temporal upscaler's motion vectors
        nfs_renderer::assign_motion_ids(&mut acc.car_items, id);
        // ENHANCEMENT E49: the player's own car reads the environment faces with the near cars (read only with E49 on)
        if car.slot == CarSlot::Player && ctx.main {
            for it in acc.car_items.iter_mut() {
                it.near_env = true;
            }
        }
        queue.extend(acc.car_items.drain(..).map(|it| (key, it)));
    }

    /// The main view's lights and shadows of its cars, once every car is drawn: ENHANCEMENT E15's spotlights (the camera's own
    /// car's pair first — they share the shadow map; made whether or not the car's body is drawn, so the cockpit views have them —
    /// then the nearest of the other cars' lamps, eight in all) and their beam strips faded, E43's nearest contact shadows, E51's
    /// nearest sun cars, E23's street lamps.
    pub(super) fn finish_main_pass(&mut self, ctx: &CarPassCtx, acc: &mut PassLists, local_draw: Vec3, local_rot: Quat) {
        let lamp_ms = ctx.lamp_ms;
        let (mut lit_all, mut over_limit): (Vec<(Vec3, f32)>, Vec<Vec3>) = (Vec::new(), Vec::new());
        let mut spots = nfs_renderer::Spots::default();
        if ctx.spots_on && let (Some(c), Some(v)) = (self.car_model.as_ref(), self.vehicle.as_ref()) {
            let bits = self.lamp_bits;
            let head_fade = if c.has_headlight_extent() { self.head_ramp.value(bits & lamp_bit::HEADLIGHTS != 0, lamp_ms) } else { 1.0 };
            let tl = self.tunnel_lit(0, local_draw, lamp_ms); // ENHANCEMENT E24
            let slots = Self::lamp_slots_tunnel(tl, bits & lamp_bit::HEADLIGHTS != 0, head_fade, false, false, (false, false, false), Some(&v.damage));
            let mut own = Vec::new();
            Self::spot_lamps(c, local_draw, local_rot, &slots, self.spot_gain(0, local_draw, lamp_ms), &mut own);
            for l in own.into_iter().take(OWN_SPOTS) {
                spots.push(l);
            }
            spots.shadowed = spots.n;
            let eye = self.camera.position;
            // ENH-CONES: every lit lamp's intensity, the pairs' positions as `spot_lamps` made them
            lit_all.extend(spots.lamps[..spots.n as usize].iter().map(|l| (l.pos, l.intensity)));
            lit_all.extend(acc.spot_others.iter().map(|l| (l.pos, l.intensity)));
            // PERF: a lamp whose reach (`gpu/spot.rs` RANGE) does not touch the view lights nothing on screen
            let cone = self.view_cone();
            acc.spot_others.retain(|l| l.intensity > 0.0 && cone.is_none_or(|c| c.sees(l.pos, SPOT_REACH)));
            acc.spot_others.sort_by(|a, b| a.pos.distance_squared(eye).total_cmp(&b.pos.distance_squared(eye)));
            let mut left_out = false;
            for l in acc.spot_others.drain(..) {
                if left_out || !spots.push(l) {
                    left_out = true;
                    over_limit.push(l.pos);
                }
            }
        }
        self.scene.spots = spots;
        if ctx.spots_on {
            self.fade_cones(&mut acc.cone_log, &lit_all, &over_limit, lamp_ms);
        }
        // ENHANCEMENT E43: the nearest cars' contact shadows (none: E43 off, no car in view)
        acc.contact_cars.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut contacts = nfs_renderer::Contacts::default();
        for (_, c) in acc.contact_cars.drain(..) {
            contacts.push(c);
        }
        self.scene.contacts = contacts;
        // ENHANCEMENT E51 (CARSH-FADE): the nearest cars whose sun share is measured
        acc.sun_cars.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut list = nfs_renderer::SunCars { clock: (self.tick as f64 + f64::from(ctx.alpha)) / TICKS_PER_SEC_F64, ..Default::default() };
        for (_, c) in acc.sun_cars.drain(..) {
            list.push(c);
        }
        self.scene.sun_cars = list;
        // ENHANCEMENT E23: the street lamps of the main view
        self.scene.lamps = self.frame_lamps(ctx.dusk_lamps);
    }

    /// A car model's body draw (0x444E00) with its light block, then the headlight projector at
    /// `projector_at` and, with `sky = Some((origin, slice))`, the sky map (`CarModel::apply_projector`,
    /// `CarModel::apply_sky`). `own`: the player's car (ENHANCEMENT E49 moves its sky map by its real lateral offset).
    pub(super) fn car_body_items(&self, model: &CarModel, block: Option<&nfs_game::world::LightBlock>, (m, spin, steer, roll, travel): (Mat4, [f32; 4], [f32; 2], [f32; 4], [f32; 4]), passes: &[nfs_game::world::CarPass], projector_at: Vec3, sky: Option<(Vec3, usize)>, own: bool) -> Vec<nfs_renderer::DrawItem> {
        let pose = nfs_game::world::CarPose { car_to_world: m, wheel_spin: spin, steer, roll, travel };
        let mut items = match block {
            Some(b) => model.draw_items_block(pose, passes, b),
            None => model.draw_items(pose, passes),
        };
        model.apply_projector(&mut items, self.projector_planes(projector_at));
        if let Some((origin, slice)) = sky {
            let mut map = self.sky_map_for(origin, slice);
            if own && self.enh.is_on(nfs_renderer::E49) {
                map = map.map(|m| self.sky_map_lateral(m, origin, slice));
            }
            model.apply_sky(&mut items, map);
        }
        items
    }

    /// PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: a car's draw
    /// pose, the previous tick's `prev` blended toward this tick's `(p1, r1)` by `alpha` — or this tick's when
    /// there is none or the car moved 5 or more since (a reset).
    pub(crate) fn blend_pose(prev: Option<&(Vec3, Quat)>, p1: Vec3, r1: Quat, alpha: f32) -> (Vec3, Quat) {
        match prev {
            Some((p0, r0)) if p0.distance(p1) < POSE_RESET_DIST => (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize()),
            _ => (p1, r1),
        }
    }

    /// PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x438478…0x4384B5]: the front
    /// wheels' steer angles `st` lerped from the previous tick's `prev` (the wheels' `[0, 1]`, or in a replay
    /// the smoothed `[2, 3]`) by `alpha`.
    pub(crate) fn blend_steer(prev: Option<&[f32; 4]>, st: [f32; 2], replaying: bool, alpha: f32) -> [f32; 2] {
        match prev {
            Some(q) => {
                let q = if replaying { [q[2], q[3]] } else { [q[0], q[1]] };
                [q[0] + (st[0] - q[0]) * alpha, q[1] + (st[1] - q[1]) * alpha]
            }
            None => st,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `blend_pose` / `blend_steer` against the expressions they replaced in `dyn_items` and
    /// `rebuild_scene` (bit for bit: the same calls in the same order).
    #[test]
    fn blend_pose_and_steer_match_the_inline_forms() {
        let p1 = Vec3::new(10.25, -3.5, 7.125);
        let r1 = Quat::from_rotation_y(0.7).normalize();
        for (p0, alpha) in [(Vec3::new(9.0, -3.0, 6.0), 0.37f32), (Vec3::new(9.0, -3.0, 6.0), 1.0), (Vec3::new(30.0, 0.0, 0.0), 0.5)] {
            let r0 = Quat::from_rotation_y(0.2).normalize();
            let inline = match Some((p0, r0)) {
                Some((p0, r0)) if p0.distance(p1) < 5.0 => (p0.lerp(p1, alpha), r0.slerp(r1, alpha).normalize()),
                _ => (p1, r1),
            };
            let (p, r) = App::blend_pose(Some(&(p0, r0)), p1, r1, alpha);
            assert_eq!((p.to_array().map(f32::to_bits), r.to_array().map(f32::to_bits)), (inline.0.to_array().map(f32::to_bits), inline.1.to_array().map(f32::to_bits)));
        }
        assert_eq!(App::blend_pose(None, p1, r1, 0.5), (p1, r1));
        let prev = [0.25f32, -0.5, 1.75, -2.0];
        let st = [3.0f32, -4.5];
        for replaying in [false, true] {
            let q = if replaying { [prev[2], prev[3]] } else { [prev[0], prev[1]] };
            let inline = [q[0] + (st[0] - q[0]) * 0.3, q[1] + (st[1] - q[1]) * 0.3];
            assert_eq!(App::blend_steer(Some(&prev), st, replaying, 0.3).map(f32::to_bits), inline.map(f32::to_bits));
        }
        assert_eq!(App::blend_steer(None, st, true, 0.3), st);
    }
}
