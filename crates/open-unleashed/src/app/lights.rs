//! The lights the scene feed hands the renderer: the shadow cascades' lights (ENHANCEMENT E9 / E50 / E51: sun, data sun, moon,
//! precipitation), the spot-lamp gain (E15), the headlight projector and the light shafts (E13). Moved out of `render_scene.rs`.

use crate::*;

/// ENHANCEMENT E50: the moon's travel direction on a night track whose sky has no moon of its own (the Monaco circuits: their sky
/// `.bin` puts the sun object overhead and switches it off, `SkyBin::sun_active`), about 48 degrees over the horizon.
/// APPROXIMATION (enhancement content, chosen by eye on the monaco1 stills: no original counterpart; Industrial, whose sky's sun
/// object is on at night, has no moon at all since SHADOW-OVERCAST: its sky layers cover it, `TrackWorld::sky_blocks_sun`).
pub(crate) const MOON_LIGHT: Vec3 = Vec3::new(-0.55, -0.75, 0.35);
/// ENHANCEMENT E50 (SHADOW-OVERCAST, 2026-11-16): E50's «слабо» level, the highest a capped track's moon is drawn at.
const MOON_WEAK: u8 = 1;
/// ENHANCEMENT E9 / E51 (SHADOW-OVERCAST): how many slices either side of the camera's the sky data's precipitation is averaged
/// over (about 120 m each way at the 6 m slices): the data's own snow steps (alps: 0 -> 1 over the four slices 644..648, the 0.3
/// pulses that drop to 0 in one slice at 474 / 539 / 579 / 620) become a ramp of the shadows over ~240 m instead of a pop.
/// APPROXIMATION (enhancement content, chosen for a smooth fade on the alps stills).
const PRECIP_REACH: i32 = 20;
/// ENHANCEMENT E9 (SHADOWFIX): the radius (m) round the camera inside which scenery casts a sun shadow whether or not the walk drew it.
pub(crate) const SHADOW_CASTER_REACH: f32 = 600.0;
/// ENHANCEMENT E15: a lamp's spot on an open day road counts this much ([`App::spot_gain`]), the covered ramp the rest.
const SPOT_OPEN_DAY: f32 = 0.3;
const SPOT_COVERED: f32 = 0.7;
/// The headlight projector 0x4526C0: the car's frame lowered by `[0x5CEC54]` = −4.8 (0x452749), then turned by `[0x5CEC5C]` −17°
/// about x (0x452769).
const HEADLIGHT_DROP: f32 = -4.8;
const HEADLIGHT_PITCH_DEG: f32 = -17.0;
/// A sun direction with a horizontal part at most this long is the port's straight-down fallback (a sky with no sun of its own).
const SUN_FLAT_EPS: f32 = 1e-3;
/// ENHANCEMENT E13 [`App::light_shafts`]: the shafts fade out by this share of the screen beyond its edge, and take this warm
/// sunlight tint, this much of it against the sky fog's colour. APPROXIMATION (enhancement content, by eye).
const SHAFT_EDGE_FADE: f32 = 0.6;
const SHAFT_WARM: [f32; 3] = [1.0, 0.86, 0.62];
const SHAFT_WARM_SHARE: f32 = 0.7;
const SHAFT_SKY_SHARE: f32 = 0.3;

impl App {
    /// ENHANCEMENT E15: how much a lamp's spot counts at `pos`: 1 on a night track and in a tunnel or under a
    /// deck (`TrackWorld::in_tunnel`, the original's covered-stretch bit), a third of it on an open day road
    /// (lit by the key there, the original's lamps do nothing at day).
    /// the gain is no longer a jump of the covered bit: a one-tick gap in the road's overhead flags inside a
    /// tunnel (castle tick 3007) dropped the spots to 0.3 for a frame and the road's light pool blinked out. The covered
    /// state is held over short gaps and the gain follows the same ramp as E24's lamps (up over 350 ms, down over
    /// 700 ms): `0.3 + 0.7 * v` for the car `key`, `now_ms` the lamp clock of the draw.
    pub(crate) fn spot_gain(&self, key: u32, pos: Vec3, now_ms: f64) -> f32 {
        if self.track.is_night() {
            return 1.0;
        }
        SPOT_OPEN_DAY + SPOT_COVERED * self.ambient.spot_cover.borrow_mut().value_held(key, self.covered_at(pos), now_ms)
    }

    /// ENHANCEMENT E9 / E50 / E51: the lights the race's main view can draw its shadow cascades for (the renderer picks one by
    /// its switches, `nfs_renderer::ShadowLights`). `lit`: the cover gate's fallback ([`nfs_renderer::SunShadow::lit`]); `moon_on`:
    /// a night track with a moon ([`Self::has_moon`]) and E50 on. None in the front end (no car).
    pub(super) fn shadow_lights(&self, lit: bool, moon_on: bool) -> nfs_renderer::ShadowLights {
        let (Some(w), Some(_)) = (self.world.as_ref(), self.vehicle.as_ref()) else { return nfs_renderer::ShadowLights::NONE };
        let light = |direction: Vec3| nfs_renderer::SunShadow { direction, lit, focus: None };
        // the sky's sun object has a direction of its own (the port's straight-down fallback is a sky without one)
        let sky_sun = w.sun_direction.x.abs() + w.sun_direction.z.abs() > SUN_FLAT_EPS;
        if self.track.is_night() {
            // ENHANCEMENT E50: a night track's moon. Where the sky's sun object is on at night (`SkyBin::sun_active`) and the sky's
            // layers do not cover it it stands for the moon: the cars are already lit from it (`TrackWorld::sun_direction`,
            // 0x43AC8D); a sky without one (the Monaco circuits) takes `MOON_LIGHT`. SHADOW-OVERCAST: a sky that covers it
            // (Industrial) has no moon at all (`has_moon`, `moon_on` false), and a track whose moon is the port's own (`!sky_moon`: the Monaco circuits) is held to the faint level.
            let sky_moon = self.track.sky_bin.as_ref().is_some_and(|b| b.sun_active()) && sky_sun;
            let cap = (!sky_moon).then_some(MOON_WEAK);
            return nfs_renderer::ShadowLights {
                moon: moon_on.then(|| light(if sky_moon { w.sun_direction } else { MOON_LIGHT.normalize() })),
                moon_level_cap: cap,
                ..nfs_renderer::ShadowLights::NONE
            };
        }
        // ENHANCEMENT E9 / E51 (SHADOW-OVERCAST): the sun's shadows fade with the sky data's snow / rain
        let precipitation = self.shadow_precipitation();
        if sky_sun {
            // ENHANCEMENT E9: the sun the cars are lit with (`-SkyBin::sun_position`, 0x43AC8D; `TrackWorld::sun_direction`)
            return nfs_renderer::ShadowLights { sun: Some(light(w.sun_direction)), precipitation, bake: w.bake_light, ..nfs_renderer::ShadowLights::NONE };
        }
        nfs_renderer::ShadowLights {
            // ENHANCEMENT E51 (CORSICA-SUN, SUN-ALL) / E9 (SUN-FULL): its data's sun (`nfs_game::data_sun`): the cars' own shadow
            // along it, or on the tracks so chosen E9's full frame (the scenery casts too: a vertex bake has no shade of the signs,
            // posts, trees); the cars' shading stays the original's. A track whose data gives no sun (foothills, autobahn since
            // SHADOW-OVERCAST) has none: nothing else stands in for it
            data_sun: w.data_sun.map(|d| nfs_renderer::DataSun { light: light(d.direction), scenery: d.scenery, baked_gate: d.baked_gate }),
            // ENHANCEMENT E9 (GATE-REL): the bake's light for the relative gate
            bake: w.bake_light,
            precipitation,
            ..nfs_renderer::ShadowLights::NONE
        }
    }

    /// ENHANCEMENT E50 (SHADOW-OVERCAST): whether this night track has a moon: not when its sky's layers cover the sun
    /// (`TrackWorld::sky_blocks_sun`: Industrial's "Does this layer block the sun" YES). A data rule; of the six night tracks it
    /// takes the moon from Industrial only (Monaco 1-5 say NO).
    pub(crate) fn has_moon(&self) -> bool {
        self.track.is_night() && !self.world.as_ref().is_some_and(|w| w.sky_blocks_sun)
    }

    /// ENHANCEMENT E9 / E51 (SHADOW-OVERCAST, "overcast during precipitation: no direct sun"; an enhancement judgement, nothing
    /// of the original's): the sky data's precipitation round the camera, 0..1: the larger of the snow (track B field 7 `+0x20`,
    /// 0x45CFD0 -> 0x45A940) and the rain (field 5 `+0x18`, 0x45D000 -> 0x45A8E0) - the two strengths the original's weather layer
    /// 0x459020 hands its emitters at the camera's slice (`weather_strengths_at`) - averaged over `PRECIP_REACH` slices either side
    /// (the data's own snow steps would pop the shadows). Shipped: alps' snowfall (slices 644..1343), autobahn's rain (no sun there).
    pub(super) fn shadow_precipitation(&self) -> f32 {
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        let (Some(_), Some(cam), true) = (self.track.sky_bin.as_ref(), self.cam_slice, lap > 0) else { return 0.0 };
        let sum: f32 = (-PRECIP_REACH..=PRECIP_REACH)
            .map(|d| {
                let (snow, rain) = self.weather_strengths_at((cam as i32 + d).rem_euclid(lap));
                snow.max(rain).clamp(0.0, 1.0)
            })
            .sum();
        sum / (2 * PRECIP_REACH + 1) as f32
    }

    /// **The headlight projector** 0x4526C0, rebuilt each frame from the player car: world into
    /// the car's frame (0x466280 / 0x466220 from `car+0x364` / `+0x330`), its y shifted by
    /// `[0x5CEC54]` = −4.8 (0x452749), then turned −17° about x (`[0x5CEC5C]` through 0x539BD0
    /// at 0x452769, the row-vector product 0x539AD0). The car's lamp bit `car+0xD87 & 1` starts
    /// as the night flag (`Car_init` 0x410C74) and the lights key toggles it (`lamp_command`).
    /// The vertex light runs the projection only while lamp 0 **or** 1 of the camera's car
    /// (`0x5E8B34[[0x65741C]]+0x74C`) is lit (0x453177…0x4531BC, else the colours pass
    /// through, 0x453516), and the `HEAD` decal only while lamp **0** is (0x453CA3) — so damage
    /// over 30 in zones 0 and 2 (0x4398C6…0x439924) puts the beam out. *(Until 2026-09-26 the
    /// port lit it on the night flag alone.)*
    /// ENHANCEMENT E24: with the lamps out and the car in a covered stretch the projector and the decal ramp in
    /// with the tunnel's lit value (the car's lamp byte is not touched)
    pub(super) fn headlight_projector(&mut self, car_pos: Vec3, car_rot: Quat, local_draw: Vec3) {
        let tunnel_ms = (self.tick as f64 + self.pause_offset + self.pause_ticks) * 1000.0 / TICKS_PER_SEC_F64;
        let tunnel = self.tunnel_lit(0, local_draw, tunnel_ms);
        let lamps = Self::lamp_slots_tunnel(tunnel, self.lamp_bits & lamp_bit::HEADLIGHTS != 0, 1.0, false, false, (false, false, false), self.vehicle.as_ref().map(|v| &v.damage));
        self.scene.headlight_gain = if self.lamp_bits & lamp_bit::HEADLIGHTS != 0 { 1.0 } else { lamps[0].1.max(lamps[1].1).min(1.0) };
        self.scene.headlight_decal = lamps[0].0;
        self.scene.headlight = (self.vehicle.is_some() && (lamps[0].0 || lamps[1].0)).then(|| {
            Mat4::from_rotation_x(HEADLIGHT_PITCH_DEG.to_radians())
                * Mat4::from_translation(Vec3::new(0.0, HEADLIGHT_DROP, 0.0))
                * Mat4::from_rotation_translation(car_rot, car_pos).inverse()
        });
    }

    /// ENHANCEMENT E13: the sun's screen point for the light shafts — the same projection the sky pass
    /// and the lens flare use (`sun_point_any`, 0x460306) — for the main race view of a track with a
    /// sun by day. `None` at night, on a sunless track (the sky configuration without a sun: autobahn,
    /// canyon, foothills, forest) and with the sun behind the eye. Strength fades with how far the
    /// sun is beyond the screen edge (full inside, none 0.6 of the screen away). The renderer reads
    /// it only with E13 and E7 on; where the sun is hidden the depth mask lets no ray through, so
    /// the original's occlusion test (the lens probe) is not needed.
    pub(super) fn light_shafts(&self, aspect: f32) -> Option<nfs_renderer::Shafts> {
        if self.vehicle.is_none() || self.track.is_night() {
            return None;
        }
        self.world.as_ref().filter(|w| w.sun_direction.x.abs() + w.sun_direction.z.abs() > SUN_FLAT_EPS)?;
        let pt = self.sun_point_any(&self.render_camera(), aspect, SUN_VIEW_MAIN)?; // ENHANCEMENT E29: the drawn camera
        if pt.depth <= 0.0 || pt.depth >= nfs_renderer::VIEW_FAR {
            return None;
        }
        let uv = [pt.px / HUD_REF_W, pt.py / HUD_REF_H];
        let over = (-uv[0]).max(uv[0] - 1.0).max(-uv[1]).max(uv[1] - 1.0).max(0.0);
        let t = (1.0 - over / SHAFT_EDGE_FADE).clamp(0.0, 1.0);
        let strength = nfs_math::smooth01(t);
        if strength <= 0.0 {
            return None;
        }
        // warm sunlight leaning toward the sky's fog colour
        let sky = self.scene.sky_fog.map_or([1.0; 3], |f| f.colour);
        let tint = [0, 1, 2].map(|i| SHAFT_WARM[i] * SHAFT_WARM_SHARE + sky[i] * SHAFT_SKY_SHARE);
        Some(nfs_renderer::Shafts { uv, strength, tint })
    }
}
