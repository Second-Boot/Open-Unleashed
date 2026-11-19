//! The scene's lighting types: the street lamps (E23), contact shadows (E43), car suns (E51), spots (E15) and the sun / moon
//! shadow lights (E9, E50). Re-exported from the crate root.

use crate::Vertex;
use glam::Vec3;

/// ENHANCEMENT E23: one street lamp as a point light.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct StreetLamp {
    pub pos: Vec3,
    /// the lamp's colour (the flare's), brightness folded in; may be 0 (faded out)
    pub color: [f32; 3],
    /// the reach of this lamp, metres
    pub radius: f32,
    /// MONACO-TUNNEL: the track's baked colour already holds this lamp's light (a tunnel lamp: a luminaire painted into a lit
    /// tunnel's texture, or a flare lamp in a covered stretch, round 2): where E47 shadows it, the shadow also takes
    /// [`BAKED_SHADOW`] of the baked colour, not only the lamp's added term
    pub baked: bool,
}

/// ENHANCEMENT E47 (MONACO-TUNNEL): how much of a track point's baked colour a [`StreetLamp::baked`] lamp's shadow takes where the lamp
/// lights it fully (scaled by the lamp's falloff and cone there; the losses of several lamps add, held to [`BAKED_SHADOW_MAX`]). A
/// tunnel lit by its bake gets E23's lamps at a small share (the bake already is their light), so a shadow that only removed that
/// share was invisible; the bake's own share of the lamp is what a car standing in its light really blocks. APPROXIMATION (by the
/// stills, `RE/notes/ENHANCEMENTS.md` E47 MONACO-TUNNEL).
pub const BAKED_SHADOW: f32 = 0.65;
/// ENHANCEMENT E47 (MONACO-TUNNEL): the most a point's baked colour loses to the baked lamps' shadows together (`shader.wgsl`
/// `lamp_light`, emitted by `classes.rs`). APPROXIMATION (as [`BAKED_SHADOW`]).
pub const BAKED_SHADOW_MAX: f32 = 0.6;

/// The most street lamps a frame lights.
pub const MAX_LAMPS: usize = 32;

/// ENHANCEMENT E23: the street lamps of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamps {
    pub n: u8,
    /// the strength of the added light
    pub gain: f32,
    pub lamps: [StreetLamp; MAX_LAMPS],
}

impl Default for Lamps {
    fn default() -> Self {
        Self { n: 0, gain: 1.0, lamps: [StreetLamp::default(); MAX_LAMPS] }
    }
}

/// ENHANCEMENT E43: the most cars a frame's contact shadow knows.
pub const MAX_CONTACTS: usize = 16;

/// ENHANCEMENT E43: one car for the contact shadow. The pose is the one the original's blob quad (`shadow_quad`, 0x43B820) is
/// laid from: the body box `half` (LOD 5's, `car+0x3A8` / `+0x3B0`) round the model origin, which stands on the ground
/// (`origin`), in the body's axes (right, up, forward).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ContactCar {
    /// the model origin on the road under the car (the box's bottom centre), world
    pub origin: Vec3,
    /// the body's right, up and forward axes in the world (unit)
    pub axes: [Vec3; 3],
    /// the box's half width (x) and half length (z), metres
    pub half: [f32; 2],
    /// the tyres' contact patch centres in the body's (x, z), metres: FL, FR, RL, RR
    pub wheels: [[f32; 2]; 4],
    /// the tyre radius, metres
    pub wheel_r: f32,
}

/// ENHANCEMENT E43: the cars of a frame (the nearest to the camera first, at most [`MAX_CONTACTS`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contacts {
    pub n: u8,
    pub cars: [ContactCar; MAX_CONTACTS],
}

impl Default for Contacts {
    fn default() -> Self {
        Self { n: 0, cars: [ContactCar::default(); MAX_CONTACTS] }
    }
}

impl Contacts {
    /// Append a car (ignored past [`MAX_CONTACTS`]).
    pub fn push(&mut self, c: ContactCar) {
        if (self.n as usize) < MAX_CONTACTS {
            self.cars[self.n as usize] = c;
            self.n += 1;
        }
    }
}

/// ENHANCEMENT E51 (CARSH-FADE): one car of [`Scene::sun_cars`]: its pose (the contact shadow's, [`ContactCar`]) and a key that
/// stays the car's from frame to frame (the renderer eases each car's share of the sun over time under it, `gpu/carsun.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SunCar {
    /// below [`SUN_CAR_KEYS`]: the player 0, the opponents 1 + seat, the police 64 + index, the traffic 128 + index (mod 128)
    pub key: u16,
    pub pose: ContactCar,
}

/// ENHANCEMENT E51 (CARSH-FADE): how many keys a [`SunCar`] may have.
pub const SUN_CAR_KEYS: usize = 256;

/// ENHANCEMENT E51 (CARSH-FADE): the key ranges of [`SunCar::key`] (first key, count): the player, the opponents, the police, the
/// traffic.
const SUN_KEYS_PLAYER: u16 = 0;
const SUN_KEYS_OPPONENTS: (u16, usize) = (1, 63);
const SUN_KEYS_POLICE: (u16, usize) = (64, 64);
const SUN_KEYS_TRAFFIC: (u16, usize) = (128, 128);

impl SunCar {
    fn keyed((first, count): (u16, usize), i: usize, pose: ContactCar) -> Self {
        Self { key: first + (i % count) as u16, pose }
    }

    /// The player's car.
    pub fn player(pose: ContactCar) -> Self {
        Self { key: SUN_KEYS_PLAYER, pose }
    }

    /// Opponent `seat` (its index in the race's opponents).
    pub fn opponent(seat: usize, pose: ContactCar) -> Self {
        Self::keyed(SUN_KEYS_OPPONENTS, seat, pose)
    }

    /// Police car `i`.
    pub fn police(i: usize, pose: ContactCar) -> Self {
        Self::keyed(SUN_KEYS_POLICE, i, pose)
    }

    /// Traffic car `i`.
    pub fn traffic(i: usize, pose: ContactCar) -> Self {
        Self::keyed(SUN_KEYS_TRAFFIC, i, pose)
    }
}

/// ENHANCEMENT E51 (CARSH-FADE): the cars of a frame whose sun share is measured (the nearest to the camera first, at most
/// [`MAX_CONTACTS`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunCars {
    pub n: u8,
    pub cars: [SunCar; MAX_CONTACTS],
    /// the draw clock (s: the game ticks and the frame's fraction of the next over 64 Hz): the ease runs on it, so a paused race
    /// holds and a scripted still (one frame per tick or fewer) eases by the race time between its frames, not the wall clock's
    pub clock: f64,
}

impl Default for SunCars {
    fn default() -> Self {
        Self { n: 0, cars: [SunCar::default(); MAX_CONTACTS], clock: 0.0 }
    }
}

impl SunCars {
    /// Append a car (ignored past [`MAX_CONTACTS`]).
    pub fn push(&mut self, c: SunCar) {
        if (self.n as usize) < MAX_CONTACTS {
            self.cars[self.n as usize] = c;
            self.n += 1;
        }
    }
}

/// ENHANCEMENT E15: one lit headlight as a spotlight.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SpotLamp {
    /// the lamp's centre in the world
    pub pos: Vec3,
    /// the way it shines (unit)
    pub dir: Vec3,
    /// 0..1: the lamp slot's ramp (`car+0x2D8`) times how dark the place is
    pub intensity: f32,
}

/// The most spotlights a frame lights.
pub const MAX_SPOTS: usize = 8;

/// ENHANCEMENT E15: the spotlights of a frame. The first `shadowed` lamps are the camera's own car's
/// pair, which share one shadow map (the rest are lights without shadows).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Spots {
    pub n: u8,
    pub shadowed: u8,
    pub lamps: [SpotLamp; MAX_SPOTS],
}

impl Spots {
    pub fn push(&mut self, l: SpotLamp) -> bool {
        if (self.n as usize) < MAX_SPOTS {
            self.lamps[self.n as usize] = l;
            self.n += 1;
            true
        } else {
            false
        }
    }
}

/// ENHANCEMENT E9: the sun of a race scene's shadows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunShadow {
    /// The direction the light travels (from the sun to the ground) — the one the cars' `R3DLightLayer`
    /// is given (`-SkyBin::sun_position`, 0x43AC8D), so the shadows agree with the car shading.
    pub direction: Vec3,
    /// ENHANCEMENT E9 (SHADOWFIX): false = the frame-wide cover gate shuts (the renderer fades the shadows out over half a
    /// second and back in). SHADOW-REAL: the race keeps it true whenever `Scene::cover` holds the track's covered boxes, so the
    /// shadow ends at a tunnel's mouth per surface (the shader's `cover_at`); the gate is the fallback of a track without them
    /// (false while the camera or the player's car is under cover).
    pub lit: bool,
    /// ENHANCEMENT ENH-FE3D: the front end's 3-D views fit ONE cascade to a sphere (centre, radius) round the car
    /// instead of the race's view-depth cascades, and soften the edge (a jittered disc of that many texels); `None` =
    /// the race's.
    pub focus: Option<ShadowFocus>,
}

/// ENHANCEMENT E9: the lights a view's sun-shadow cascades can be drawn for, one field per kind. The renderer draws ONE frame of
/// cascades a frame, for the first kind in this order that is set and whose switch is on (`gpu/csm/mode.rs`, `ShadowMode::pick`): the
/// sky's sun (E9; or a front-end car's [`SunShadow::focus`]), the data sun (E51 / SUN-FULL), the moon
/// (E50). Besides the cascades only [`Self::sun`] is read (the fog glow, the particles' and the water's sun).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ShadowLights {
    /// ENHANCEMENT E9: the sun the race's main view (or a front-end car) is lit with; `None` on a night or a sunless track. Read
    /// only with E9 on.
    pub sun: Option<SunShadow>,
    /// ENHANCEMENT E51 (CORSICA-SUN, SUN-ALL) / E9 (SUN-FULL): a sunless day track's sun from its data ([`DataSun`]). Read only
    /// with E9 on, and with E51 on unless it drives E9's full frame.
    pub data_sun: Option<DataSun>,
    /// ENHANCEMENT E50: on a night track the moon's light through the same cascades (a faint cold light, soft shadows, a weak cold
    /// term on the cars). Read only with E9 and E50 on.
    pub moon: Option<SunShadow>,
    /// ENHANCEMENT E50 (SHADOW-OVERCAST): the highest E50 level this track's moon is drawn at (`None` = E50's own level): a level
    /// above it draws at it (Monaco: «средне» draws as «слабо»).
    pub moon_level_cap: Option<u8>,
    /// ENHANCEMENT E9 / E51 (SHADOW-OVERCAST, "overcast during precipitation: no direct sun"): how much snow / rain the sky data
    /// has round the camera, 0 (none) .. 1 (full). The sun's and the data sun's shadows are drawn at `1 - precipitation` of their
    /// strength (the cover gate scaled by it: the track's darkening, the cars' direct-term shadow, the car's `SHAD` patch and the
    /// `sh80` decals' give-way all follow); at 1 no frame is drawn. 0 (the `Default`) changes nothing.
    pub precipitation: f32,
    /// ENHANCEMENT E9 (GATE-REL): the track's bake light, for [`BakedGate::Relative`] (`None`: that gate is `Absolute`).
    pub bake: Option<BakeLight>,
}

impl ShadowLights {
    /// No shadow light (the `Default`, usable in a `const`).
    pub const NONE: Self = Self { sun: None, data_sun: None, moon: None, moon_level_cap: None, precipitation: 0.0, bake: None };

    /// Whether a frame of these lights can draw the scenery's shadows (any light but the cars-only data sun): the race then adds
    /// the casters round the camera (`TrackWorld::shadow_casters`). By the lights set, not by the switches.
    pub fn casts_scenery(&self) -> bool {
        self.sun.is_some() || self.moon.is_some() || self.data_sun.is_some_and(|d| d.scenery.is_some())
    }
}

/// ENHANCEMENT E9 (SHADOW-UNIFY): which non-ground scenery parts cast the sun's / the moon's shadow (never a level flat sheet).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneryCasters {
    /// every part whatever its size, but the terrain-like ones: broad (bounding radius over 60 m) and neither upright nor built
    /// (`gpu/csm/shape.rs` `terrain_like`; hills, cliffs, slopes, fields). APPROXIMATION: an enhancement's rule, by the caster audit.
    Shape,
    /// TEST GLUE (CANYON-SMALLSH, comparison): only the small upright parts: bounding radius up to `radius` m and (mesh space) height up
    /// to `height` m
    Small { radius: f32, height: f32 },
}

/// ENHANCEMENT E9 (SHADOW-UNIFY): the casters' rule of a frame that sets none of its own (the sky sun, E50's moon, the front end,
/// the data sun's full frame): [`SceneryCasters::Shape`]. (The comparison switch `NFS_CASTER_RULE` and the radius rule of before
/// were removed 2026-11-19, REFACTOR-0119.)
pub fn scenery_casters() -> SceneryCasters {
    SceneryCasters::Shape
}

/// ENHANCEMENT E9 (SHADOW-UNIFY): whether a non-ground scenery part with these vertices (mesh space) placed at `scale` casts under
/// `rule`, as the renderer's shadow pass decides it (its bounding sphere and shape at upload, `gpu/csm/shape.rs` `mesh_bounds` /
/// `part_casts`). For ENH-DBLSH's decal tagging: a decal under a caster gives way to its shadow.
pub fn scenery_part_casts(vertices: &[Vertex], scale: f32, rule: SceneryCasters) -> bool {
    crate::gpu::part_casts_vertices(vertices, scale, rule)
}

/// ENHANCEMENT E51 (CORSICA-SUN, SUN-ALL) / E9 (SUN-FULL): the sun of a day track whose sky has none (the sky's sun object overhead
/// and off; the cars' own light straight down) but whose data gives one: its `sh80` tree-shadow decals were drawn along it
/// (Corsica, `canyon`) or its bake was lit by it (Schwarzwald, `nfs_game::bake_light`). The cars' shading is left as it is (their
/// light is the original's straight-down one), and nothing but the cascades reads it (not the fog glow, the cars' glint, water).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DataSun {
    pub light: SunShadow,
    /// `None`: E51's cars-only frame (only the cars cast, onto the track; the scenery's shade is already baked / in the decals).
    /// `Some(rule)` (SUN-FULL): E9's FULL frame (scenery and cars cast, the scenery by `rule`: SHADOW-UNIFY's shape rule as on a
    /// sky-sun track, which keeps the terrain out: a hillside's shade along a sun the bake never had would cover whole stretches),
    /// E9's baked-dark gate on the track, E51's cars-only layers on top when E51 is on, as on a sky-sun track.
    pub scenery: Option<SceneryCasters>,
    /// FOREST-TREESH (with `scenery`): E9's baked-dark gate on the track for this frame ([`BakedGate`]; SHADOW-UNIFY: `Relative`; the gate keeps a shadow baked into the colour from being darkened twice).
    pub baked_gate: BakedGate,
}

/// ENHANCEMENT E9: how a track receiver's baked colour scales the sun's shadow on it (the "already baked dark" gate: a shade the
/// bake already has is not darkened twice). APPROXIMATION: an enhancement's look, nothing of the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakedGate {
    /// by the baked colour's absolute luma (`smoothstep(0.3, 0.7)`, E9's gate since it came)
    Absolute,
    /// GATE-REL: by the baked luma against the lit level the track's own bake light gives that orientation ([`BakeLight`]): a
    /// receiver counts as baked in shade when it is well below its own lit level, not when it is merely dark. Without a
    /// [`BakeLight`] it is `Absolute`.
    Relative,
}

/// ENHANCEMENT E9 (GATE-REL): the light a track's vertex colours were baked with, as `nfs_game::bake_light` fits it over the
/// scenery, `luma = base + gain max(0, n . to_light)`; the drivable ground's lit level is `ground` times that (its bake is lighter
/// than the scenery's). Read only by [`BakedGate::Relative`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakeLight {
    /// toward the bake's light (unit, upward)
    pub to_light: Vec3,
    pub base: f32,
    pub gain: f32,
    pub ground: f32,
}

/// ENHANCEMENT ENH-FE3D: a front-end car's shadow volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowFocus {
    pub centre: Vec3,
    pub radius: f32,
    /// the soft edge's radius in metres
    pub blur: f32,
    /// how strongly the shadow darkens a receiver at full shadow, 0..1
    pub strength: f32,
    /// ENHANCEMENT ENH-GARAGE: the contact core's radius in metres (a second, sharper disc; 0 = none): the shadow is
    /// the darker of the soft penumbra (weighted by `soft`) and the core, so the car sits in a tight dark footprint
    pub core: f32,
    /// the penumbra's share of the full strength, 0..1 (the core is always whole)
    pub soft: f32,
    /// how much of the original's `FESH` blob under the garage car gives way to this shadow, 0..1
    pub blob_fade: f32,
}

impl SunShadow {
    /// The race's sun (the view-depth cascades).
    pub const fn race(direction: Vec3) -> Self {
        Self { direction, lit: true, focus: None }
    }
}
