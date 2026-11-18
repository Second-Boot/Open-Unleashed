//! ENHANCEMENT E9: cascaded shadow maps from the sun.
//!
//! What it does. The sun the cars are lit with (`-SkyBin::sun_position`, the `R3DLightLayer`
//! direction of 0x43AC8D, passed in [`crate::SunShadow`]) casts shadows from the cars (player,
//! opponents, traffic, police: every opaque item with a car light) and from roadside scenery
//! ([`crate::CSM_CAST`] items: non-drivable track article parts, instance props, smackables,
//! breakables) into a depth map, cascaded over the camera's view depth. The main pass reads it
//! (`shader.wgsl`, `csm_visibility`) and darkens only the **direct** share of the light:
//!
//! * a car: the `R3DLightLayer`'s directional term, `min(ambient + diffuse · n·l · visibility, 1)`;
//!   the ambient slot (the ground patch colour × the `.map` ambient) is untouched;
//! * the track (no direct term of its own — its colour is baked vertex light × texture, no
//!   `R3DLightLayer`): the product is scaled by `1 − s · (1 − visibility)`, `s` = [`GROUND_STRENGTH`]
//!   × how much the surface faces the sun (a face turned away is already dark in the bake) × how bright
//!   the baked colour is (an already baked shadow is not darkened twice). The track itself casts nothing.
//!
//! The original's `sh80` blob under the car is left as it is, drawn over the result as before. Not
//! used at night (a night track has no sun to cast with), in the rear-view mirror, in the front end.
//!
//! Stable cascades: each slice of the view frustum is fitted with a sphere whose radius depends on the
//! field of view, the aspect and the split only (not on the camera's heading), and the box's centre
//! is snapped to whole shadow texels in light space, so the map does not swim or shimmer as the
//! camera turns or moves at 64 Hz or between ticks.
//!
//! SHADOW-REFACTOR: one frame of cascades has ONE [`ShadowMode`] (the sky's sun or a sunless track's full data sun, E51's
//! cars-only data sun, E50's moon, a front-end car's focus) picked from [`crate::ShadowLights`] by
//! [`ShadowMode::pick`], and ONE [`CarLayers`] (E51's cars-only layers); the shader reads the mode as a class constant
//! (`misc.y`, `classes::SHADOW_*`) instead of decoding it from the kernel spacing and the strengths.

use super::shadowmap::{Caster, DepthArray, LightMats, UniformSlot};
use super::*;
use crate::{classes, gfx};
use bytemuck::Zeroable;
use glam::{Mat4, Vec3, Vec4};

/// How many cascades the maps hold at most (the shader's arrays).
pub(super) const MAX_CASCADES: usize = 4;
/// How strongly a shadow darkens the track's baked colour at full visibility loss (0..1): the track has
/// no separate direct light, so this stands for the sun's share of the baked brightness.
pub(super) const GROUND_STRENGTH: f32 = 0.6;
/// The car's floor of its direct term in the sun's shadow: never fully black, a quarter of the direct term (the sky's own
/// scatter). (Was the shader's default for a `params.w` of 0.)
pub(super) const SUN_CAR_FLOOR: f32 = 0.25;
/// ENHANCEMENT E50: the moonlight per level (index 1 weak, 2 normal): the share of the baked night colour a moon shadow takes at
/// most (the track), and the cold term added to the cars' light (times `MOON_COLOUR`, `n.l` and the map's visibility).
/// APPROXIMATION (enhancement content, by eye on monaco1 / industrial stills): no original counterpart.
pub(super) const MOON_STRENGTH: [f32; 3] = [0.0, 0.22, 0.35];
pub(super) const MOON_CAR: [f32; 3] = [0.0, 0.08, 0.15];
/// ENHANCEMENT E50: the moon's cold colour (a pale blue of luminance about 0.8), the soft edge's radius in texels (a 16-tap disc),
/// and the cars' floor of the direct term (`params.w`; not reached: a moon frame leaves the cars' own direct term unshadowed).
pub(super) const MOON_COLOUR: [f32; 3] = [0.66, 0.8, 1.0];
pub(super) const MOON_SOFT: f32 = 2.5;
pub(super) const MOON_FLOOR: f32 = 0.4;
/// ENHANCEMENT E50: the strongest moon level (`MOON_STRENGTH` / `MOON_CAR`'s last index).
const MOON_MAX_LEVEL: u8 = 2;
/// ENHANCEMENT E51: how many of the near cascades get a cars-only layer (level 1: 0-40 and 40-140 m; a car's shadow beyond is a
/// few pixels).
pub(super) const CAR_CASCADES: usize = 2;
/// How far toward the sun a caster may stand and still shade a cascade (m), beyond the cascade's sphere.
const CASTER_REACH: f32 = 120.0;
/// SHADOW-UNIFY (casters): a scenery part whose bounding sphere is larger than this (m) is BROAD: it casts only when it is not
/// terrain-like ([`terrain_like`]). Until SHADOW-UNIFY this was a plain limit (`MAX_CASTER_RADIUS`: every part over 60 m left out,
/// so a hillside or a cliff face would not fill the map with a shade the bake never had); but a track's parts are one mesh per
/// material per article, and the limit also left out whole objects whose one material spans an article: castle's courtyard ring
/// wall (`ct42`, 77.8 m) and its roof slopes, the terraced walls, rows of tree sheets (`fhtm`, `fht1`, `lin1`, `ftl5`, `trl1`),
/// farmyard walls and fence rows, the coastal freighter's hull, Monaco's facades (the caster audit of 2026-11-18).
const BROAD_CASTER_RADIUS: f32 = 60.0;
/// SHADOW-UNIFY (APPROXIMATION, by the caster audit's scan of every track's parts): a broad part is upright, so an object and a
/// caster, when the mean `|n.y|` of its vertex normals is under [`UPRIGHT_UP`] (a wall, a facade, a fence, a hull, a sheet of
/// trees: on the shipped tracks every broad part under 0.1 is one, but canyon's `mn02` rock face), or when it is built: low
/// (under [`BUILT_HEIGHT`] m tall) and steep (mean `|n.y|` under [`BUILT_UP`]: castle's roof slopes `ct59`, a fence row on a
/// slope, a ship's superstructure). Every other broad part is terrain-like (hills, cliffs, slopes, fields, the sea bed: on the
/// shipped tracks they are either over 30 m tall or, lower, mostly facing up, mean `|n.y|` 0.65 and over) and casts nothing: its
/// shade along the sun would contradict the bake, which lights the road beside it evenly.
const UPRIGHT_UP: f32 = 0.1;
const BUILT_UP: f32 = 0.65;
const BUILT_HEIGHT: f32 = 30.0;
/// FOREST-TREESH: `Csm::params.z` of a frame whose track takes the shadow without E9's baked-dark gate (the shader tests it against one half).
const GATE_OFF: f32 = 1.0;
/// GATE-REL: `Csm::params.z` of a frame whose track's gate is relative to its bake light (`crate::BakedGate::Relative`; the shader
/// tests it against one and a half).
const GATE_REL: f32 = 2.0;
/// GATE-REL (APPROXIMATION, an enhancement's look; the ramp variant, [`GATE_REL_CAP`] off): the ramp over `baked luma / the bake
/// light's lit level`: no extra darkening at or under `lo` (the bake has the shade), the whole shadow from `hi` (lit in the bake).
const GATE_REL_RAMP: [f32; 2] = [0.5, 0.85];
/// GATE-REL (APPROXIMATION; the ramp variant): the least share of the shadow a receiver takes however deep in the bake's shade it is.
const GATE_REL_FLOOR: f32 = 0.0;
/// GATE-REL (APPROXIMATION): the drivable ground's lit level, as a share of `BakeLight::ground` (its 90th percentile). At 1.0 a dim
/// forest road under the trees counts as baked shade and takes nearly no shadow (the trees before the covered bridge cast none, as
/// with the absolute gate; by comparing stills of the forest); 0.65 put it near the
/// ground's median (forest 1.92 x 0.65 = 1.25 against a median of 1.24). SHADOW-UNIFY (2026-11-18, by sheet): 0.80 keeps the
/// forest canopy's dappled stripes and still shades the road under the trees before the covered bridge and at the rock / bush seam.
const GATE_REL_GROUND_SCALE: f32 = 0.80;
/// GATE-REL (APPROXIMATION): the cap mode instead of the ramp: the bake's own shade counts toward the shadow, `keep' = min(1, keep /
/// min(1, baked / lit))`: a point is darkened only down to the level a face lit in the bake takes in the same shadow, never twice;
/// a baked-dark stretch still shows the shadow's shapes, weaker (the dappled canopy), a lit one takes it whole. The ramp and floor
/// are then unused (kept for the comparison, `NFS_GATE_REL`).
const GATE_REL_CAP: bool = true;
/// A cascade's sphere radius is rounded up to whole steps of this many per metre, so the box keeps its size as the camera turns.
const RADIUS_STEPS_PER_M: f32 = 8.0;
/// The depth margin (m) added to a cascade's range beyond its sphere and the casters' reach.
const RANGE_MARGIN: f32 = 2.0;
/// A slice of the view depth thinner than this (m) gets no cascade (the camera's far plane cut it off).
const MIN_SLICE: f32 = 0.25;
/// A light direction shorter than this (squared) is none: no frame.
const MIN_DIRECTION_SQ: f32 = 0.5;
/// The light's travel direction counts as vertical (the map's axes then start from X instead of Y) above this `|y|`.
const VERTICAL_Y: f32 = 0.99;

/// ENHANCEMENT ENH-SHPOP: where the last cascade starts to fade out, as a share of its end (was 0.85: a 33 m ramp at 220 m)
const FADE_START: f32 = 0.6;

/// ENHANCEMENT E9 (SHADOWFIX): how fast the cover gate eases toward its target, per second (fully in or out in 0.5 s). SHADOW-REAL:
/// only a fallback now: the race holds the gate open while it hands the covered boxes (`SunShadow::lit`), and the shader decides
/// per surface (`cover_at`), so a tunnel exit has no wall-clock ramp.
const GATE_RATE: f32 = 2.0;
/// ENHANCEMENT E9 (SHADOWFIX): the longest step (s) the gate takes in one frame (a stalled frame does not jump it), and the gate
/// below which the shadows are out (no pass at all).
const GATE_MAX_STEP: f32 = 0.25;
const GATE_SHUT: f32 = 0.002;
/// ENHANCEMENT ENH-FE3D: how far toward the sun a caster may stand and still shade the front-end car's one cascade (m,
/// [`fit_focus`]'s `ext`), beyond its focus sphere.
const FOCUS_CASTER_REACH: f32 = 12.0;
/// ENHANCEMENT ENH-FE3D: the front-end cascade's fade start, as a share of its split (`params.y`).
const FOCUS_FADE_START: f32 = 0.85;
/// ENHANCEMENT ENH-FE3D: the front-end cascade's split: beyond any view depth, so the map is read for every pixel.
const FOCUS_SPLIT: f32 = 1.0e6;

/// The levels' map sizes and cascade ends (view depth, m): level 1, then level 2 and up. ENHANCEMENT ENH-SHPOP: the last cascade
/// reaches far (the old 140 / 220 m ends made shadows pop in as they came in range).
const LEVEL_1: (u32, &[f32]) = (1024, &[40.0, 140.0, 400.0]);
const LEVEL_2: (u32, &[f32]) = (2048, &[25.0, 80.0, 220.0, 520.0]);

/// The level's map size, cascade ends (view depth, m).
pub(super) fn level_config(level: u8) -> (u32, &'static [f32]) {
    if level == 1 { LEVEL_1 } else { LEVEL_2 }
}

/// ENHANCEMENT E9: which light a frame's cascades are drawn for, and how (see the module doc).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ShadowMode {
    /// ENHANCEMENT ENH-FE3D: a front-end car's one soft cascade ([`crate::SunShadow::focus`])
    Focus(crate::ShadowFocus),
    /// the race's sun: the sky's (E9), or a sunless track's data sun with its scenery (SUN-FULL); `casters`: which scenery parts
    /// cast (SHADOW-UNIFY: the shape rule, [`terrain_like`]); `baked_gate` (FOREST-TREESH, GATE-REL): E9's "already baked dark"
    /// gate on the track (`Relative` only with the track's bake light)
    Sun { casters: crate::SceneryCasters, baked_gate: crate::BakedGate },
    /// ENHANCEMENT E51 (CORSICA-SUN): a sunless track's data sun with the cars alone: the near cascades only, the scenery casts
    /// nothing (its shade is baked / in the decals)
    /// `small` (CANYON-SMALLSH, 2026-11-18): the small upright scenery parts (signs, posts, rails, walls, bushes: `SceneryCasters::Small`)
    /// that also cast into the same near cascades; everything else of the frame (cascades, strength, gate, floor, the cars' shadow) is
    /// the cars-only frame's own
    CarsOnly { small: Option<crate::SceneryCasters> },
    /// ENHANCEMENT E50: the moon at its level (1 weak, 2 normal: faint, its own soft edge)
    Moon { level: u8 },
}

/// SHADOW-UNIFY (2026-11-18: one shadow mode for every sun track): the sky sun's baked-dark gate is GATE-REL's relative one,
/// as the data sun's ([`ShadowMode::pick`] falls back to the absolute gate when the track has no bake light, a data rule).
/// TEST GLUE (comparison stills): `NFS_SKY_SUN_GATE=on` = the absolute gate (the default before SHADOW-UNIFY), `off` = no gate
/// (the FULL-SHADOWS trial), `rel` / unset = the relative gate.
fn sky_sun_gate() -> crate::BakedGate {
    static GATE: std::sync::OnceLock<crate::BakedGate> = std::sync::OnceLock::new();
    *GATE.get_or_init(|| match std::env::var("NFS_SKY_SUN_GATE").as_deref().map(str::trim) {
        Ok("off") => crate::BakedGate::Off,
        Ok("on") => crate::BakedGate::Absolute,
        _ => crate::BakedGate::Relative,
    })
}

/// TEST GLUE (GATE-REL, comparison stills): `NFS_GATE_REL=<lo>,<hi>[,<floor>[,<ground scale>[,<cap 0|1>]]]` replaces
/// [`GATE_REL_RAMP`], [`GATE_REL_FLOOR`], [`GATE_REL_GROUND_SCALE`] and [`GATE_REL_CAP`].
fn gate_rel_tune() -> [f32; 5] {
    static TUNE: std::sync::OnceLock<[f32; 5]> = std::sync::OnceLock::new();
    *TUNE.get_or_init(|| {
        let mut t = [GATE_REL_RAMP[0], GATE_REL_RAMP[1], GATE_REL_FLOOR, GATE_REL_GROUND_SCALE, f32::from(u8::from(GATE_REL_CAP))];
        let v = std::env::var("NFS_GATE_REL").ok();
        let p: Vec<f32> = v.as_deref().map_or(Vec::new(), |v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect());
        if p.len() >= 2 && p[0] < p[1] {
            t[..p.len().min(5)].copy_from_slice(&p[..p.len().min(5)]);
        }
        t
    })
}

/// ENHANCEMENT E9 (SHADOW-FLOOR, 2026-11-18; APPROXIMATION, an enhancement's look): a sun frame's shadow may not darken a
/// receiver below the bake's own ambient share. The bake light's fit (`luma = a + b max(0, n.L)`, [`crate::BakeLight`] `base` /
/// `gain`) says a face turned away from the bake's light keeps `a` of a lit face's `a + b`: that is the light a shadow leaves. So
/// the frame's darkness (`params.x`, [`GROUND_STRENGTH`]: the track receivers and the cars' shadows on them, E51's cars-only frame
/// too) is `min(strength, 1 - a / (a + b))`; without a bake light (no fit, night) the strength stays. One data rule for every sun
/// track instead of one darkness for all: a bake with much ambient light (a dusk, an overcast-like bake) takes lighter shadows.
fn floored_strength(strength: f32, bake: Option<&crate::BakeLight>) -> f32 {
    bake.filter(|b| b.base + b.gain > 1e-4).map_or(strength, |b| strength.min(1.0 - (b.base / (b.base + b.gain)).clamp(0.0, 1.0)))
}

/// TEST GLUE (SHADOW-FLOOR, comparison stills): `NFS_SHADOW_FLOOR=off` = the frame's darkness without the bake's ambient floor.
fn shadow_floor_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("NFS_SHADOW_FLOOR").map_or(true, |v| v.trim() != "off"))
}

/// GATE-REL: `Csm::bake` / `bake_k` / `bake_f` of a frame: (toward the bake light, its base), (its gain, the ground's factor, the
/// ramp), (the floor).
fn bake_uniform(bake: Option<&crate::BakeLight>) -> [[f32; 4]; 3] {
    bake.map_or([[0.0; 4]; 3], |b| {
        let [lo, hi, floor, gs, cap] = gate_rel_tune();
        [[b.to_light.x, b.to_light.y, b.to_light.z, b.base], [b.gain, b.ground * gs, lo, hi], [floor, cap, 0.0, 0.0]]
    })
}

impl ShadowMode {
    /// The light and the mode of a frame: the first light of `lights` that is set and whose switch is on, in
    /// [`crate::ShadowLights`]' order (the sun, the data sun, E50's moon).
    fn pick(lights: &crate::ShadowLights, e: &crate::Enhancements) -> Option<(crate::SunShadow, Self)> {
        // GATE-REL: the relative gate needs the track's bake light
        let gate = |g: crate::BakedGate| if g == crate::BakedGate::Relative && lights.bake.is_none() { crate::BakedGate::Absolute } else { g };
        if let Some(s) = lights.sun {
            return Some((s, s.focus.map_or(Self::Sun { casters: crate::scenery_casters(), baked_gate: gate(sky_sun_gate()) }, Self::Focus)));
        }
        // ENHANCEMENT E51 (CORSICA-SUN): the cars-only frame with E51 on; E9 / E51 (SUN-FULL): the full frame with E9 alone, the
        // scenery casters by the frame's rule (SHADOW-UNIFY: the shape rule, as on a sky-sun track)
        if let Some(d) = lights.data_sun {
            match d.scenery {
                // CANYON-SMALLSH: the decals' sun's frame: the cars-only frame with the small parts added (with E51 on, as it)
                Some(small @ crate::SceneryCasters::Small { .. }) if e.is_on(crate::E51) => return Some((d.light, Self::CarsOnly { small: Some(small) })),
                Some(crate::SceneryCasters::Small { .. }) => {}
                Some(casters) => return Some((d.light, Self::Sun { casters, baked_gate: gate(d.baked_gate) })),
                None if e.is_on(crate::E51) => return Some((d.light, Self::CarsOnly { small: None })),
                None => {}
            }
        }
        // (SHADOW-OVERCAST: at most the track's cap, `ShadowLights::moon_level_cap`)
        let level = e.level(crate::E50).min(lights.moon_level_cap.unwrap_or(MOON_MAX_LEVEL));
        lights.moon.filter(|_| level > 0).map(|s| (s, Self::Moon { level: level.min(MOON_MAX_LEVEL) }))
    }

    /// The class the shader tests (`csm.misc.y`, `classes::SHADOW_*`).
    fn shader_class(&self) -> f32 {
        match self {
            Self::Sun { .. } | Self::CarsOnly { .. } => classes::SHADOW_SUN,
            Self::Moon { .. } => classes::SHADOW_MOON,
            Self::Focus(_) => classes::SHADOW_FOCUS,
        }
    }

    /// `params`: x the track's strength, y where the last cascade (ending at `last`) starts to fade out, z [`GATE_OFF`] when the
    /// track's baked-dark gate is off (FOREST-TREESH), [`GATE_REL`] when it is relative (GATE-REL), else 0, w the cars' floor of the
    /// direct term.
    fn params(&self, last: f32) -> [f32; 4] {
        match *self {
            Self::Sun { baked_gate: crate::BakedGate::Off, .. } => [GROUND_STRENGTH, last * FADE_START, GATE_OFF, SUN_CAR_FLOOR],
            Self::Sun { baked_gate: crate::BakedGate::Relative, .. } => [GROUND_STRENGTH, last * FADE_START, GATE_REL, SUN_CAR_FLOOR],
            Self::Sun { .. } | Self::CarsOnly { .. } => [GROUND_STRENGTH, last * FADE_START, 0.0, SUN_CAR_FLOOR],
            Self::Moon { level } => [MOON_STRENGTH[usize::from(level)], last * FADE_START, 0.0, MOON_FLOOR],
            // ENHANCEMENT ENH-FE3D: the front-end car's own strength
            Self::Focus(f) => [f.strength, last * FOCUS_FADE_START, 0.0, SUN_CAR_FLOOR],
        }
    }

    /// Which scenery parts cast in this mode (the moon's and the front end's frames: the default rule, [`crate::scenery_casters`]).
    fn casters(&self) -> crate::SceneryCasters {
        match *self {
            Self::Sun { casters, .. } => casters,
            Self::CarsOnly { small: Some(small) } => small,
            _ => crate::scenery_casters(),
        }
    }

    /// ENHANCEMENT E9 / E51 (SHADOW-OVERCAST): whether the sky data's precipitation weakens this mode's shadows
    /// ([`crate::ShadowLights::precipitation`]): the race's sun and data sun, not the front end's focus or the moon
    fn fades_with_precipitation(&self) -> bool {
        matches!(self, Self::Sun { .. } | Self::CarsOnly { .. })
    }

    /// ENHANCEMENT ENH-DBLSH: whether a track's `sh80` decals give way to this frame's shadows: the sun's (not the front end's focus,
    /// not E50's moon, whose shadows are too faint to stand in for them, not the cars-only frame: no
    /// scenery shadow stands in for the decals)
    fn decal_fades(&self) -> bool {
        matches!(self, Self::Sun { .. })
    }

    /// ENHANCEMENT E9: whether the car's `SHAD` patch gives way to the sun's car shadow in the open (the race's sun and the cars-only
    /// frame; not the front end's focus or the moon, whose car shadow is too faint to stand in for it)
    fn takes_car_patch(&self) -> bool {
        matches!(self, Self::Sun { .. } | Self::CarsOnly { .. })
    }
}

/// ENHANCEMENT E51: the cars-only layers of a frame (the near cascades again, with the cars alone as casters).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CarLayers {
    /// none: E51 off, or not the race's sun
    None,
    /// CORSICA-SUN: the cars-only frame's cascades are themselves the cars-only layers (no extra layers)
    Own,
    /// after the cascades, one layer per near cascade; `sun_share` (level 1, CARSH-FADE): each car's shadow scaled by its measured
    /// share of the sun (`carsun.rs`; without compute shaders level 1 draws as level 2)
    Extra { sun_share: bool },
}

/// One fitted cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cascade {
    /// world → the map's clip space (x, y in -1..1, z in 0..1 along the light)
    pub mat: Mat4,
    /// a texel's size in the world (m)
    pub texel: f32,
    /// the depth range the 0..1 of z covers (m)
    pub range: f32,
    /// the box's half width (m) and its centre in light space (x, y, and the near depth)
    pub half: f32,
    pub centre: [f32; 2],
    pub zmin: f32,
    /// the view depth where this cascade ends
    pub split: f32,
}

impl Cascade {
    /// Whether a caster's sphere can shade the cascade: against the box in light space (toward the sun it is open).
    fn reaches(&self, axes: (Vec3, Vec3, Vec3), c: &Caster) -> bool {
        let (ax, ay, d) = axes;
        let (lx, ly, lz) = (ax.dot(c.centre), ay.dot(c.centre), d.dot(c.centre));
        let lim = self.half + c.radius;
        (lx - self.centre[0]).abs() <= lim && (ly - self.centre[1]).abs() <= lim && lz - c.radius <= self.zmin + self.range
    }
}

/// The light's axes for a travel direction `d` (unit): the map's x and y, deterministic in `d`.
pub(super) fn light_axes(d: Vec3) -> (Vec3, Vec3) {
    let r = if d.y.abs() > VERTICAL_Y { Vec3::X } else { Vec3::Y };
    let x = r.cross(d).normalize();
    let y = d.cross(x).normalize();
    (x, y)
}

/// The cascade of a sphere (`centre`, radius `r` rounded up) along the light `d` (unit): the box snapped to whole texels of a
/// `size` map, `ext` metres of depth reserved toward the sun for casters, ending at view depth `split`.
fn snapped(d: Vec3, centre: Vec3, r: f32, size: u32, ext: f32, split: f32) -> Cascade {
    let (ax, ay) = light_axes(d);
    let r = (r * RADIUS_STEPS_PER_M).ceil() / RADIUS_STEPS_PER_M;
    let texel = 2.0 * r / size as f32;
    let (lx, ly, lz) = (ax.dot(centre), ay.dot(centre), d.dot(centre));
    let (cx, cy) = ((lx / texel).round() * texel, (ly / texel).round() * texel);
    let range = (2.0 * r + ext).ceil() + RANGE_MARGIN;
    let zmin = (lz - r - ext).floor();
    let mat = Mat4::from_cols(
        Vec4::new(ax.x / r, ay.x / r, d.x / range, 0.0),
        Vec4::new(ax.y / r, ay.y / r, d.y / range, 0.0),
        Vec4::new(ax.z / r, ay.z / r, d.z / range, 0.0),
        Vec4::new(-cx / r, -cy / r, -zmin / range, 1.0),
    );
    Cascade { mat, texel, range, half: r, centre: [cx, cy], zmin, split }
}

/// ENHANCEMENT ENH-FE3D: the one cascade of a front-end car's shadow — a sphere round the car, the box snapped to whole
/// texels like [`fit`]'s; its split is far beyond any view depth, so the map is read for every pixel (the cascade's own
/// box says where a shadow can fall, outside it the point is lit).
pub(super) fn fit_focus(centre: Vec3, radius: f32, dir: Vec3, size: u32, ext: f32) -> Cascade {
    snapped(dir.normalize(), centre, radius, size, ext, FOCUS_SPLIT)
}

/// ENHANCEMENT E9 (SHADOWFIX): the lowest elevation the shadows' light has over the horizon (degrees).
const MIN_ELEVATION_DEG: f32 = 30.0;
/// A direction's least horizontal length squared to have an azimuth to keep.
const MIN_HORIZONTAL_SQ: f32 = 1e-6;

/// `d` (a travel direction) with its elevation raised to at least [`MIN_ELEVATION_DEG`], azimuth kept.
pub(super) fn lift_elevation(d: Vec3) -> Vec3 {
    let (s, c) = MIN_ELEVATION_DEG.to_radians().sin_cos();
    let h = Vec3::new(d.x, 0.0, d.z);
    if -d.y >= s || h.length_squared() < MIN_HORIZONTAL_SQ {
        return d;
    }
    let h = h.normalize() * c;
    Vec3::new(h.x, -s, h.z)
}

/// The cascades for the camera: slice `[prev end, end]` of the view depth each, a sphere around the
/// slice, the box snapped to texels. `ext` is the depth reserved toward the sun for casters.
pub(super) fn fit(camera: &Camera, aspect: f32, dir: Vec3, splits: &[f32], size: u32, ext: f32) -> Vec<Cascade> {
    let d = dir.normalize();
    let fwd = (camera.target - camera.position).normalize();
    let ty = (camera.fov_y_radians * 0.5).tan();
    let tx = ty * aspect;
    let k2 = tx * tx + ty * ty;
    let mut out = Vec::new();
    let mut near = camera.near;
    for &end in splits {
        let far = end.min(camera.far);
        if far <= near + MIN_SLICE {
            break;
        }
        // the smallest sphere on the view axis holding the slice's eight corners (centre at depth
        // `(n + f)(1 + k²)/2`, held at the far plane's centre when that lies beyond it); its radius depends on nothing the
        // camera's heading changes
        let c = (0.5 * (near + far) * (1.0 + k2)).min(far);
        let r = (far * far * k2 + (far - c) * (far - c)).sqrt();
        out.push(snapped(d, camera.position + fwd * c, r, size, ext, far));
        near = far;
    }
    out
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CsmUniform {
    mats: [[[f32; 4]; 4]; MAX_CASCADES],
    /// per cascade: x a texel (m), y the depth range (m), z the view depth it ends at
    info: [[f32; 4]; MAX_CASCADES],
    /// xyz the light's travel direction, w the cascade count (0 = off)
    dir: [f32; 4],
    /// [`ShadowMode::params`]
    params: [f32; 4],
    /// ENHANCEMENT E9 (SHADOWFIX): xyz the light the cars are really lit with (the shadow's own is elevated), w the
    /// gate 0..1 (shadows faded out under cover)
    orig: [f32; 4],
    /// ENHANCEMENT ENH-FE3D: the front-end car's soft edge's radius (x) and contact core's radius (y) in texels, the penumbra's
    /// share (z), the `FESH` blob's fade (w)
    extra: [f32; 4],
    /// x (ENHANCEMENT E9, ENH-TUNPOOL) the count of covered boxes below (the nearest to the camera; 0 = none), y the frame's
    /// [`ShadowMode`] class (`classes::SHADOW_*`)
    misc: [f32; 4],
    /// GATE-REL ([`bake_uniform`]): xyz toward the track's bake light, w its base; then x its gain, y the drivable ground's factor,
    /// zw the relative gate's ramp. Zero unless the frame's gate is relative.
    bake: [f32; 4],
    bake_k: [f32; 4],
    bake_f: [f32; 4],
    /// ENHANCEMENT E9 (FOREST-E9PERF): the world box round every covered box below (xyz least, xyz most; empty with no boxes): a
    /// point outside it is in none of them (`cover_at` skips its loop)
    cover_lo: [f32; 4],
    cover_hi: [f32; 4],
    /// per box, 5 vec4: (centre, half length), (forward, half width), (lateral, weight at the back face),
    /// (up, weight at the front face), (ceiling height, nominal half length, 0, 0)
    cover: [[f32; 4]; MAX_COVER * COVER_VEC4S],
    /// ENHANCEMENT E50: x the moon's track strength (0 = not a moon frame), y the cars' cold term, z the soft edge's radius in texels
    moon: [f32; 4],
    /// ENHANCEMENT E50: rgb the moon's colour
    moon_col: [f32; 4],
    /// ENHANCEMENT E51: x how many near cascades have a cars-only layer (0 = none), y the first of those layers in the map,
    /// z 1 when each car's shadow is scaled by its share of the sun (level 1, CARSH-FADE), w how many cars `carsh_cars` lists
    carsh: [f32; 4],
    /// ENHANCEMENT E51 (CARSH-FADE): per measured car its body's centre (xyz) and the radius (w) within which a receiver's ray
    /// toward the sun passes the car
    carsh_cars: [[f32; 4]; crate::MAX_CONTACTS],
    /// ENHANCEMENT E51 (CARSH-FADE): the cars' eased shares of their own shadow (1 all .. 0 none), four to a vec4; written on the
    /// GPU by `carsun.rs` (copied in after the shadow passes)
    carsh_k: [[f32; 4]; crate::MAX_CONTACTS / 4],
}

/// The depth-only shadow passes' shader (E9's cascades, E15's headlight map, E47's lamp maps): the items' `struct Object`
/// (`classes.rs` `OBJECT_WGSL`), then `shadow.wgsl`.
pub(crate) fn shadow_source() -> String {
    crate::wgsl::compose(&[crate::classes::OBJECT_WGSL, include_str!("shadow.wgsl")])
}

/// The most covered boxes the shader knows (the nearest to the camera go in).
pub(super) const MAX_COVER: usize = 64;
/// A covered box's vec4s in the uniform, and its bytes.
const COVER_VEC4S: usize = 5;
const COVER_BYTES: usize = COVER_VEC4S * std::mem::size_of::<[f32; 4]>();
/// Covered boxes farther than this from the camera are left out (m); past it a tunnel's interior is shaded as open ground, which
/// E9's own far fade has reached (the last cascade ends at 400 m / 520 m).
const COVER_REACH: f32 = 420.0;
/// PERF (FOREST-E9PERF): how far outside the view volume (m) a covered box may stand and still be handed to the shader: a
/// particle's light reads E9 at the particle's centre (`particle_fx`), which may be off the screen while its sprite is on it.
const COVER_VIEW_SLACK: f32 = 30.0;
/// PERF (FOREST-E9PERF): how much wider than its boxes' corners (m) the world box round the covered boxes is: room for the
/// rounding of the corners and of the shader's own tests.
const COVER_BOUND_PAD: f32 = 0.05;

/// Everything E9 keeps: the pass's pipelines, the uniform the main shader reads, the map.
pub(super) struct CsmState {
    pub(super) pipe_opaque: wgpu::RenderPipeline,
    pub(super) pipe_cut: wgpu::RenderPipeline,
    /// the one-matrix light group the depth-only pipelines read (E15's and E47's maps use it too)
    pub(super) light_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub uniform: UniformSlot<CsmUniform>,
    lights: LightMats,
    dummy_view: wgpu::TextureView,
    pub map: Option<DepthArray>,
    /// ENHANCEMENT E9 (SHADOWFIX): the eased cover gate (1 = in the open) and when it was last stepped
    gate: f32,
    gate_at: Option<std::time::Instant>,
    /// ENHANCEMENT ENH-FE3D: this frame's shadow is a front-end car's (`SunShadow::focus`)
    focus: bool,
    /// a cascade's caster list, kept between frames (no allocation per pass)
    list: Vec<u32>,
    /// ENHANCEMENT E9 (ENH-TUNPOOL): the covered boxes in reach this frame, (distance squared, index), kept between frames
    cover_near: Vec<(f32, usize)>,
}

/// What a frame with shadows needs, from [`Core::csm_frame`].
pub(super) struct CsmFrame {
    cascades: Vec<Cascade>,
    dir: Vec3,
    /// ENHANCEMENT E9 (SHADOWFIX): the light the cars are lit with, and the eased cover gate
    orig: Vec3,
    gate: f32,
    mode: ShadowMode,
    /// ENHANCEMENT E51: the cars-only layers and how many near cascades have one
    car_layers: CarLayers,
    car_cascades: usize,
}

impl CsmFrame {
    /// ENHANCEMENT ENH-DBLSH: whether a track's `sh80` decals give way to this frame's shadows ([`ShadowMode::decal_fades`])
    pub(super) fn decal_fades(&self) -> bool {
        self.mode.decal_fades()
    }

    /// ENHANCEMENT E50: whether this is the moon's frame (the mesh shader's MOON part runs)
    pub(super) fn is_moon(&self) -> bool {
        matches!(self.mode, ShadowMode::Moon { .. })
    }

    /// ENHANCEMENT ENH-FE3D: whether this is a front-end car's focus frame (the mesh shader's FOCUS part runs)
    pub(super) fn is_focus(&self) -> bool {
        matches!(self.mode, ShadowMode::Focus(_))
    }

    /// ENHANCEMENT E51: whether this frame has the cars-only layers (the mesh shader's CARSH part runs)
    pub(super) fn has_car_layers(&self) -> bool {
        self.car_layers != CarLayers::None
    }

    /// ENHANCEMENT E9: how much of the car's `SHAD` patch the sun's shadow takes in the open (the cover gate; 0 where the mode's car
    /// shadow does not stand in for it, [`ShadowMode::takes_car_patch`]). SHADOW-REAL: the fill scales it by the patch's own
    /// openness (`DrawItem::cover`), so a car in a tunnel keeps its patch whatever the camera does.
    pub(super) fn car_patch_gate(&self) -> f32 {
        if self.mode.takes_car_patch() { self.gate } else { 0.0 }
    }
}

impl CsmState {
    pub(super) fn new(device: &wgpu::Device, object_layout: &wgpu::BindGroupLayout, material_layout: &wgpu::BindGroupLayout) -> Self {
        let light_layout = gfx::bgl(device, "shadow light", &[gfx::bgl_uniform(0, wgpu::ShaderStages::VERTEX)]);
        let layout = gfx::pipeline_layout(device, "shadow pipeline layout", &[&light_layout, object_layout, material_layout]);
        let module = gfx::wgsl_module(device, "shadow shader", &shadow_source());
        let make = |label: &str, fragment: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
                    })],
                },
                fragment: fragment.then_some(wgpu::FragmentState { module: &module, entry_point: Some("fs_cut"), compilation_options: Default::default(), targets: &[] }),
                // both faces cast (thin foliage, open meshes); the receiver's normal offset and this slope-scaled bias keep the surfaces clean
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let (pipe_opaque, pipe_cut) = (make("shadow opaque", false), make("shadow cut-out", true));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let lights = LightMats::new(device, &light_layout, "shadow light vp", MAX_CASCADES);
        let dummy_view = DepthArray::placeholder(device, "shadow map placeholder");
        Self { pipe_opaque, pipe_cut, light_layout, sampler, uniform: UniformSlot::new(device, "csm uniform"), lights, dummy_view, map: None, gate: 1.0, gate_at: None, focus: false, list: Vec::new(), cover_near: Vec::new() }
    }

    /// The view the main shader binds (the placeholder until a map exists).
    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.array_view)
    }
}

/// The three bindings E9 adds to the globals group (the map, the comparison sampler, the uniform).
pub(super) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    use crate::classes::binding as b;
    [super::shadowmap::array_layout_entry(b::CSM_MAP), gfx::bgl_sampler(b::CSM_SAMPLER, wgpu::SamplerBindingType::Comparison), gfx::bgl_uniform(b::CSM, wgpu::ShaderStages::FRAGMENT)]
}

/// The most vertices of a flat sheet's thickness (m) and the least mean `|n.y|` of its normals (see [`mesh_bounds`]).
const FLAT_THICKNESS: f32 = 0.5;
const FLAT_UP: f32 = 0.85;

/// ENHANCEMENT E9: a mesh's shape as the casters' rule reads it ([`mesh_bounds`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct MeshShape {
    /// a level flat sheet (a road layer, a marking, a ground decal): never a caster
    pub flat: bool,
    /// SHADOW-UNIFY: upright or built (see [`UPRIGHT_UP`]): a broad part of this shape is an object, not terrain
    pub upright: bool,
    /// the part's height in mesh space (m; the track's articles and props are not scaled): TEST GLUE's small-parts rule
    pub height: f32,
}

/// SHADOW-UNIFY: whether a scenery part of bounding radius `radius` (m, world) and `shape` is terrain-like, so no caster under
/// the shape rule: broad ([`BROAD_CASTER_RADIUS`]) and neither upright nor built ([`UPRIGHT_UP`]).
pub(crate) fn terrain_like(radius: f32, shape: MeshShape) -> bool {
    radius > BROAD_CASTER_RADIUS && !shape.upright
}

/// SHADOW-UNIFY: whether a scenery part of bounding radius `radius` (m, world) and `shape` casts under `rule` (never a flat sheet).
pub(crate) fn part_casts(rule: crate::SceneryCasters, radius: f32, shape: MeshShape) -> bool {
    !shape.flat
        && match rule {
            crate::SceneryCasters::Shape => !terrain_like(radius, shape),
            crate::SceneryCasters::Radius(r) => radius <= r,
            crate::SceneryCasters::Small { radius: r, height: h } => radius <= r && shape.height <= h,
        }
}

/// SHADOW-UNIFY: [`part_casts`] of a mesh's vertices placed at `scale` (`crate::scenery_part_casts`).
pub(crate) fn part_casts_vertices(vertices: &[Vertex], scale: f32, rule: crate::SceneryCasters) -> bool {
    let (_, r, shape) = mesh_bounds(vertices);
    !vertices.is_empty() && part_casts(rule, r * scale, shape)
}

/// A mesh's bounding sphere (the box's centre, the farthest vertex from it) and its shape: whether it is a flat sheet lying level
/// (thinner than half a metre with mostly upward normals: a road layer, a marking, a ground decal), which casts nothing worth the
/// coplanar acne it would give the road under it, and (SHADOW-UNIFY) whether it is upright or built ([`UPRIGHT_UP`]; the height in
/// mesh space: the track's articles and props are not scaled).
pub(super) fn mesh_bounds(vertices: &[Vertex]) -> ([f32; 3], f32, MeshShape) {
    if vertices.is_empty() {
        return ([0.0; 3], 0.0, MeshShape::default());
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(v.position[i]);
            hi[i] = hi[i].max(v.position[i]);
        }
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let r2 = vertices.iter().map(|v| (0..3).map(|i| (v.position[i] - c[i]) * (v.position[i] - c[i])).sum::<f32>()).fold(0.0, f32::max);
    let up = vertices.iter().map(|v| v.normal[1].abs()).sum::<f32>() / vertices.len() as f32;
    let height = hi[1] - lo[1];
    let shape = MeshShape { flat: height < FLAT_THICKNESS && up > FLAT_UP, upright: up < UPRIGHT_UP || (up < BUILT_UP && height < BUILT_HEIGHT), height };
    (c, r2.sqrt(), shape)
}

impl Core {
    /// The cascades of this frame, or `None` when no shadows are drawn: E9 off, no light the switches allow ([`ShadowMode::pick`]),
    /// the cover gate shut, or the rear-view mirror's pass. Makes the map when the level's size or layers changed.
    pub(super) fn csm_frame(&mut self, scene: &Scene, camera: &Camera, aspect: f32) -> Option<CsmFrame> {
        self.csm.focus = scene.shadows.sun.is_some_and(|s| s.focus.is_some());
        if !self.enhancements.is_on(crate::E9) || self.no_reflections {
            return None;
        }
        let (sun, mode) = ShadowMode::pick(&scene.shadows, &self.enhancements)?;
        if sun.direction.length_squared() < MIN_DIRECTION_SQ {
            return None;
        }
        // ENHANCEMENT E9 (SHADOWFIX): the cover gate eases (0.5 s) toward its target; fully out = no pass at all
        let now = std::time::Instant::now();
        let dt = self.csm.gate_at.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(GATE_MAX_STEP));
        self.csm.gate_at = Some(now);
        let target = if sun.lit { 1.0 } else { 0.0 };
        self.csm.gate += (target - self.csm.gate).clamp(-dt * GATE_RATE, dt * GATE_RATE);
        if self.csm.gate <= GATE_SHUT {
            return None;
        }
        // ENHANCEMENT E9 / E51 (SHADOW-OVERCAST, "overcast during precipitation: no direct sun"): the sun's shadows are drawn at
        // `1 - precipitation` of their share, through the gate every part of them reads (`csm_share`, the car's `SHAD` patch); fully
        // overcast = no pass. The eased gate itself is not touched (the precipitation is smooth along the road already).
        let gate = if mode.fades_with_precipitation() { self.csm.gate * (1.0 - scene.shadows.precipitation.clamp(0.0, 1.0)) } else { self.csm.gate };
        if gate <= GATE_SHUT {
            return None;
        }
        // ENHANCEMENT E9 (SHADOWFIX): a low sun (coastal's is 3 degrees up) throws shadows hundreds of metres long: hills and
        // the tunnel's mass would blacken whole stretches with hard edges where the map ends. The shadow's own light is
        // lifted to at least `MIN_ELEVATION` over the horizon, keeping its azimuth; the cars' shading keeps the original's.
        let orig = sun.direction.normalize();
        let shadow_dir = lift_elevation(orig);
        let (size, splits) = level_config(self.enhancements.level(crate::E9));
        // ENHANCEMENT E51: a sun frame of the race with E51 on has a cars-only layer per near cascade after the cascades' own; the
        // cars-only frame (CORSICA-SUN) keeps the near cascades only (E51's own reach: a car's shadow farther away is a few pixels),
        // and they are the cars-only layers themselves
        let e51 = self.enhancements.is_on(crate::E51);
        let car_layers = match mode {
            ShadowMode::CarsOnly { .. } => CarLayers::Own,
            // (CARSH-FADE: level 1 asks per car how much of the sun reaches it past the scenery; without compute shaders it draws
            // as level 2)
            ShadowMode::Sun { .. } if e51 => CarLayers::Extra { sun_share: self.compute_supported && self.enhancements.level(crate::E51) == crate::Enhancements::E51_WHERE_SUN },
            _ => CarLayers::None,
        };
        let splits = if car_layers == CarLayers::Own { &splits[..CAR_CASCADES.min(splits.len())] } else { splits };
        // ENHANCEMENT ENH-FE3D: a front-end car's volume: one cascade round the car
        let cascades = match mode {
            ShadowMode::Focus(f) => vec![fit_focus(f.centre, f.radius, shadow_dir, size, FOCUS_CASTER_REACH)],
            _ => fit(camera, aspect, shadow_dir, splits, size, CASTER_REACH),
        };
        if cascades.is_empty() {
            return None;
        }
        let car_cascades = if car_layers == CarLayers::None { 0 } else { CAR_CASCADES.min(splits.len()) };
        let layers = match (mode, car_layers) {
            (ShadowMode::Focus(_), _) => 1,
            (_, CarLayers::Extra { .. }) => (splits.len() + car_cascades) as u32,
            _ => splits.len() as u32,
        };
        if self.csm.map.as_ref().is_none_or(|m| m.size != size || m.layers != layers) {
            self.csm.map = Some(DepthArray::new(&self.device, "sun shadow map (ENHANCEMENT E9)", size, layers));
            self.rebuild_globals_bg();
        }
        Some(CsmFrame { cascades, dir: shadow_dir, orig, gate, mode, car_layers, car_cascades })
    }

    /// An item the shadow passes draw: a car's opaque part (its own light's direction is the sun's),
    /// or scenery flagged [`crate::CSM_CAST`]; never a sky / overlay / additive / ground-decal item.
    pub(super) fn csm_caster(&self, item: &crate::DrawItem) -> bool {
        let Some(mat) = self.materials.get(item.material.0 as usize) else { return false };
        if item.overlay || item.sky_pass || !mat.depth_test || item.alpha < 0.5 || mat.blend_pair != 0 {
            return false;
        }
        // ENHANCEMENT ENH-FE3D: a front-end room's own pieces never cast
        if item.csm & crate::CSM_ROOM != 0 {
            return false;
        }
        let car = item.car_lit();
        if !car && item.csm & crate::CSM_CAST == 0 {
            return false;
        }
        // a blended ground decal (the `S_Normal` shadow, depth-biased) is not an object; `early` marks one only in a race: the front end
        // submits every item early (its materials all blend), its car's body included, and its blob quad is a flat mesh
        !(mat.alpha_blend && (mat.depth_bias != 0 || item.depth_bias != 0 || item.early && car && !self.csm.focus))
    }

    /// What an item does with the shadow map in the main pass, for `ObjectUniform::lit_d.w`: 1 a car
    /// (the direct term is scaled), 2 a track item (the baked colour is scaled), 0 neither.
    pub(super) fn csm_receive(&self, item: &crate::DrawItem) -> f32 {
        use crate::classes::{RECV_CAR, RECV_NONE, RECV_TRACK};
        let Some(mat) = self.materials.get(item.material.0 as usize) else { return RECV_NONE };
        let plain = mat.depth_test && !item.sky_pass && !item.overlay;
        // ENHANCEMENT ENH-FE3D: a front-end room's piece takes the shadow like the track (its baked / lit colour scaled)
        if item.csm & crate::CSM_ROOM != 0 {
            // ENHANCEMENT ENH-GARAGE: the garage's lift platform (`CarIntShd`, the material that carries the `FESH` projector)
            // is a blend-pair-1 material: it takes the shadow too, or the car's contact on it is the blob alone
            return if plain && (mat.blend_pair == 0 || item.shadow.is_some()) { RECV_TRACK } else { RECV_NONE };
        }
        if item.car_lit() {
            return if item.overlay || item.sky_pass || mat.blend_pair != 0 { RECV_NONE } else { RECV_CAR };
        }
        if item.csm & crate::CSM_RECEIVE != 0 && plain && mat.blend_pair == 0 {
            return RECV_TRACK;
        }
        RECV_NONE
    }

    /// The covered boxes the shader knows this frame (ENHANCEMENT E9, ENH-TUNPOOL): the nearest to the camera within
    /// [`COVER_REACH`], at most [`MAX_COVER`]; a fragment inside one takes no sun-shadow darkening (the shader's `cover_at`),
    /// whatever the camera is doing: a tunnel's pools look the same from outside and inside.
    /// `near`: the pass's kept list (refilled).
    fn write_cover(u: &mut CsmUniform, cover: &[crate::CoverBox], eye: Vec3, vp: Mat4, near: &mut Vec<(f32, usize)>) {
        near.clear();
        near.extend(cover.iter().enumerate().map(|(i, b)| (b.centre.distance_squared(eye), i)).filter(|(d, _)| *d < COVER_REACH * COVER_REACH));
        if near.len() > MAX_COVER {
            near.select_nth_unstable_by(MAX_COVER - 1, |a, b| a.0.total_cmp(&b.0));
            near.truncate(MAX_COVER);
        }
        // PERF (FOREST-E9PERF, picture identical): of those, only the boxes a fragment of this view can fall in (`cover_at` loops
        // over every box for every fragment). Culled after the nearest 64 are picked, so the boxes that remain are exactly the ones
        // that could change a pixel before.
        near.retain(|&(_, i)| cover[i].may_cover_view(vp, COVER_VIEW_SLACK));
        u.misc[0] = near.len() as f32;
        // PERF (FOREST-E9PERF, picture identical): the world box round them all (a degenerate box: round everything), widened a
        // little against rounding; a point outside it skips the loop
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &(_, i) in near.iter() {
            match cover[i].corners() {
                Some(cs) => cs.iter().for_each(|&c| (lo, hi) = (lo.min(c), hi.max(c))),
                None => (lo, hi) = (Vec3::splat(f32::MIN), Vec3::splat(f32::MAX)),
            }
        }
        let pad = Vec3::splat(COVER_BOUND_PAD);
        u.cover_lo = (lo - pad).extend(0.0).to_array();
        u.cover_hi = (hi + pad).extend(0.0).to_array();
        for (box_, b) in u.cover.chunks_exact_mut(COVER_VEC4S).zip(near.iter().map(|&(_, i)| &cover[i])) {
            box_.copy_from_slice(&[
                [b.centre.x, b.centre.y, b.centre.z, b.half_len],
                [b.forward.x, b.forward.y, b.forward.z, b.half_wid],
                [b.lateral.x, b.lateral.y, b.lateral.z, b.w_back],
                [b.up.x, b.up.y, b.up.z, b.w_front],
                [b.ceil, b.half_nom, 0.0, 0.0],
            ]);
        }
    }

    /// Write the frame's uniform and draw the cascades' casters (`casters`: the frame's, [`Core::frame_casters`]). Without a frame
    /// the uniform is set to "off" once if it was on.
    pub(super) fn csm_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, casters: &[Caster], camera: &Camera, vp: Mat4, frame: Option<&CsmFrame>) {
        let Some(frame) = frame else {
            self.csm.uniform.clear(&mut self.belt, encoder);
            return;
        };
        let cascades = &frame.cascades;
        let mut u = CsmUniform::zeroed();
        for (i, c) in cascades.iter().enumerate() {
            u.mats[i] = c.mat.to_cols_array_2d();
            u.info[i] = [c.texel, c.range, c.split, 0.0];
        }
        u.dir = [frame.dir.x, frame.dir.y, frame.dir.z, cascades.len() as f32];
        u.orig = [frame.orig.x, frame.orig.y, frame.orig.z, frame.gate];
        u.params = frame.mode.params(cascades.last().map_or(0.0, |c| c.split));
        // ENHANCEMENT E9 (SHADOW-FLOOR): a sun frame's darkness held to the bake's own ambient share
        if matches!(frame.mode, ShadowMode::Sun { .. } | ShadowMode::CarsOnly { .. }) && shadow_floor_on() {
            u.params[0] = floored_strength(u.params[0], scene.shadows.bake.as_ref());
        }
        u.misc[1] = frame.mode.shader_class();
        if let ShadowMode::Sun { baked_gate: crate::BakedGate::Relative, .. } = frame.mode {
            [u.bake, u.bake_k, u.bake_f] = bake_uniform(scene.shadows.bake.as_ref());
        }
        match frame.mode {
            // ENHANCEMENT ENH-FE3D: the front-end car's shadow: a soft jittered disc and (ENH-GARAGE) a contact core
            ShadowMode::Focus(f) => u.extra = [f.blur / cascades[0].texel, f.core / cascades[0].texel, f.soft, f.blob_fade],
            // ENHANCEMENT E50: the moon: its own strength and soft disc, a cold term on the cars
            ShadowMode::Moon { level } => {
                let l = usize::from(level);
                u.moon = [MOON_STRENGTH[l], MOON_CAR[l], MOON_SOFT, 0.0];
                u.moon_col = [MOON_COLOUR[0], MOON_COLOUR[1], MOON_COLOUR[2], 0.0];
            }
            _ => {}
        }
        // ENHANCEMENT E51: the cars-only layers follow the cascades' own in the map (CORSICA-SUN: the cars-only frame's cascades are
        // the cars-only layers: the shader's `car_visibility` reads them from 0)
        let car_layers = frame.car_cascades.min(cascades.len());
        let car_base = match frame.car_layers {
            CarLayers::Extra { .. } => self.csm.map.as_ref().map_or(0, |m| m.layers as usize - frame.car_cascades),
            _ => 0,
        };
        // ENHANCEMENT E51 (CARSH-FADE): level 1: the measured cars (their shares come from the compute after the passes)
        let mut sun_cars = 0;
        if car_layers > 0 {
            let sun_share = frame.car_layers == CarLayers::Extra { sun_share: true };
            if sun_share {
                let cs = self.car_sun.get_or_insert_with(|| super::carsun::CarSun::new(&self.device));
                sun_cars = cs.prepare((&mut self.belt, &mut *encoder), cascades, camera.view(), &scene.sun_cars, (car_layers, car_base), &mut u.carsh_cars);
                u.carsh_k = [[1.0; 4]; crate::MAX_CONTACTS / 4];
            }
            u.carsh = [car_layers as f32, car_base as f32, f32::from(u8::from(sun_share)), sun_cars as f32];
        }
        if !scene.cover.is_empty() && !matches!(frame.mode, ShadowMode::Focus(_)) {
            Self::write_cover(&mut u, &scene.cover, camera.position, vp, &mut self.csm.cover_near);
        }
        // PERF (picture identical): the covered boxes past the count are not written (`cover_at` reads `misc.x` of them; the
        // rest of the 5 KB keep an earlier frame's)
        let used = std::mem::offset_of!(CsmUniform, cover) + u.misc[0] as usize * COVER_BYTES;
        self.csm.uniform.write_except(&mut self.belt, encoder, &u, used..std::mem::offset_of!(CsmUniform, moon));
        for (i, c) in cascades.iter().enumerate() {
            self.csm.lights.write(&mut self.belt, encoder, i, &c.mat.to_cols_array_2d());
        }

        // the cascades, then (ENHANCEMENT E51) the near ones again with the cars alone into the cars-only layers (CORSICA-SUN: the
        // cars-only frame draws its cascades with the cars alone and has no extra layers)
        let own_cars = frame.car_layers == CarLayers::Own;
        let extra = if matches!(frame.car_layers, CarLayers::Extra { .. }) { car_layers } else { 0 };
        let passes = (0..cascades.len()).map(|ci| (ci, ci, own_cars)).chain((0..extra).map(|ci| (ci, car_base + ci, true)));
        let axes = { let (ax, ay) = light_axes(frame.dir); (ax, ay, frame.dir) };
        // (the mode's casters: E9 leaves the flat sheets out and, SHADOW-UNIFY, the terrain-like broad parts, `part_casts`)
        let rule = frame.mode.casters();
        // (CANYON-SMALLSH: the cars-only frame's scenery is the small parts alone: a car casts as before, a scenery part by `Small`)
        let small = matches!(frame.mode, ShadowMode::CarsOnly { small: Some(_) });
        let ours = || casters.iter().filter(|k| part_casts(rule, k.radius, k.shape));
        let mut list = std::mem::take(&mut self.csm.list);
        for (ci, layer, cars_only) in passes {
            let c = &cascades[ci];
            list.clear();
            list.extend(ours().filter(|k| (k.car || !cars_only || small) && c.reaches(axes, k)).map(|k| k.idx));
            let map = self.csm.map.as_mut().expect("shadow map made");
            // PERF (SHADOW-REFACTOR): a layer the last frame left cleared, with no caster now, holds what this pass would write
            // (E51's far cars-only layer with no car in it, most frames)
            if !map.needs_pass(layer, list.is_empty()) {
                continue;
            }
            let map = &*map;
            let (label, prof) = if cars_only { ("sun shadow cars-only pass", ["csm cars 0", "csm cars 1+"][ci.min(1)]) } else { ("sun shadow pass", ["csm cascade 0", "csm cascade 1", "csm cascade 2", "csm cascade 3+"][ci.min(3)]) };
            let mut pass = super::shadowmap::depth_pass(encoder, label, prof, &map.layer_views[layer]);
            pass.set_bind_group(0, self.csm.lights.bind_group(ci), &[]);
            self.draw_shadow_casters(&mut pass, scene, &list);
        }
        self.csm.list = list;
        // ENHANCEMENT E51 (CARSH-FADE): the cars' shares of the sun, from the map just drawn, into the uniform the main pass reads
        if sun_cars > 0
            && let (Some(cs), Some(map)) = (self.car_sun.as_mut(), self.csm.map.as_ref())
        {
            let offset = std::mem::offset_of!(CsmUniform, carsh_k) as u64;
            cs.record(&self.device, encoder, sun_cars, (&map.array_view, &self.csm.sampler), (&self.csm.uniform.buf, offset));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mesh shader's `Csm` is this uniform, byte for byte in size.
    #[test]
    fn the_shader_reads_the_uniform() {
        assert_eq!(crate::classes::tests::mesh_struct_size("Csm") as usize, std::mem::size_of::<CsmUniform>());
    }

    fn cam(pos: Vec3, target: Vec3) -> Camera {
        Camera { position: pos, target, up: Vec3::Y, fov_y_radians: 45f32.to_radians(), near: 0.3, far: 700.0 }
    }

    #[test]
    fn cascade_size_does_not_depend_on_the_heading() {
        let d = Vec3::new(0.3, -0.8, 0.2);
        let a = fit(&cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(10.0, 2.0, 105.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
        let b = fit(&cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(110.0, 2.0, 5.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
        assert_eq!(a.len(), 2);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((x.half, x.texel, x.range), (y.half, y.texel, y.range));
        }
    }

    #[test]
    fn box_centre_is_snapped_to_whole_texels() {
        let d = Vec3::new(0.3, -0.8, 0.2);
        for k in 0..20 {
            let p = Vec3::new(3.0 + k as f32 * 0.0137, 2.0, 5.0 + k as f32 * 0.0291);
            let cs = fit(&cam(p, p + Vec3::new(0.2, 0.0, 1.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
            for c in &cs {
                for v in c.centre {
                    let t = v / c.texel;
                    assert!((t - t.round()).abs() < 1e-3, "{t}");
                }
            }
        }
    }

    #[test]
    fn a_point_in_the_slice_lands_inside_the_map() {
        let d = Vec3::new(0.3, -0.8, 0.2).normalize();
        let c = cam(Vec3::new(100.0, 5.0, -40.0), Vec3::new(130.0, 4.0, 10.0));
        let cs = fit(&c, 1.6, d, &[40.0, 140.0], 1024, 120.0);
        let fwd = (c.target - c.position).normalize();
        let p = c.position + fwd * 20.0;
        let q = cs[0].mat * p.extend(1.0);
        assert!(q.x.abs() < 1.0 && q.y.abs() < 1.0 && (0.0..1.0).contains(&q.z), "{q:?}");
    }

    #[test]
    fn bounds_cover_the_vertices() {
        let v = |x: f32, y: f32, z: f32| Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] };
        let (c, r, shape) = mesh_bounds(&[v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), v(1.0, 2.0, 0.0)]);
        assert_eq!(c, [1.0, 1.0, 0.0]);
        assert!((r - 2f32.sqrt()).abs() < 1e-6);
        assert!(!shape.flat);
    }

    /// SHADOW-FLOOR: the darkness held to the bake's ambient share; no bake light, no floor
    #[test]
    fn the_shadow_floor_follows_the_bake() {
        let b = |base: f32, gain: f32| crate::BakeLight { to_light: Vec3::Y, base, gain, ground: 1.5 };
        let forest = floored_strength(GROUND_STRENGTH, Some(&b(0.384, 0.335)));
        assert!((forest - (1.0 - 0.384 / 0.719)).abs() < 1e-6, "{forest}");
        assert_eq!(floored_strength(GROUND_STRENGTH, Some(&b(0.1, 0.9))), GROUND_STRENGTH, "a strong bake keeps the strength");
        assert_eq!(floored_strength(GROUND_STRENGTH, None), GROUND_STRENGTH);
        assert_eq!(floored_strength(GROUND_STRENGTH, Some(&b(0.0, 0.0))), GROUND_STRENGTH);
    }

    /// SHADOW-UNIFY: the casters' shape rule: a broad part casts when upright or built, not when terrain-like; a flat sheet never
    #[test]
    fn the_shape_rule_keeps_terrain_out() {
        let v = |x: f32, y: f32, z: f32, ny: f32| Vertex { position: [x, y, z], normal: [(1.0 - ny * ny).sqrt(), ny, 0.0], uv: [0.0; 2], color: [1.0; 4] };
        let part = |w: f32, h: f32, ny: f32| [v(-w, 0.0, 0.0, ny), v(w, 0.0, 0.0, ny), v(0.0, h, 1.0, ny)];
        let casts = |p: &[Vertex]| part_casts_vertices(p, 1.0, crate::SceneryCasters::Shape);
        assert!(casts(&part(80.0, 34.0, 0.02)), "a ring wall 160 m round (castle ct42)");
        assert!(casts(&part(80.0, 120.0, 0.0)), "a sheet of trees 120 m tall (forest ftl6)");
        assert!(casts(&part(75.0, 15.0, 0.58)), "roof slopes (castle ct59)");
        assert!(!casts(&part(70.0, 120.0, 0.4)), "a cliff (canyon clfa)");
        assert!(!casts(&part(70.0, 60.0, 0.8)), "a hillside (farmland grs2)");
        assert!(!casts(&part(70.0, 12.0, 0.98)), "a field (farmland fm03)");
        assert!(casts(&part(20.0, 40.0, 0.8)), "anything under 60 m casts as before");
        assert!(!casts(&part(20.0, 0.2, 1.0)), "a flat sheet never");
        assert!(!part_casts_vertices(&part(80.0, 34.0, 0.02), 1.0, crate::SceneryCasters::Radius(60.0)), "the radius rule of before");
        assert!(!part_casts_vertices(&[], 1.0, crate::SceneryCasters::Shape));
        let small = crate::SceneryCasters::Small { radius: 15.0, height: 8.0 };
        assert!(part_casts_vertices(&part(1.0, 2.0, 0.0), 1.0, small), "a sign");
        assert!(part_casts_vertices(&part(12.0, 3.0, 0.0), 1.0, small), "a roadside wall");
        assert!(!part_casts_vertices(&part(12.0, 20.0, 0.4), 1.0, small), "a cliff chunk (too tall)");
        assert!(!part_casts_vertices(&part(40.0, 3.0, 0.0), 1.0, small), "a long gorge wall (too broad)");
    }

    /// SHADOW-REFACTOR: the mode a set of lights and switches gives, in the lights' order (the sun, the data sun, E50), as the
    /// flags of before decided it.
    #[test]
    fn the_shadow_mode_follows_the_lights_and_switches() {
        use crate::{DataSun, Enhancements, SceneryCasters, ShadowLights, SunShadow, E50, E51};
        let s = SunShadow::race(Vec3::new(0.3, -0.8, 0.2));
        let mut e = Enhancements::ORIGINAL;
        let pick = |l: &ShadowLights, e: &Enhancements| ShadowMode::pick(l, e).map(|(_, m)| m);
        let sun = ShadowLights { sun: Some(s), data_sun: Some(DataSun { light: s, scenery: None, baked_gate: crate::BakedGate::Absolute }), ..ShadowLights::NONE };
        assert_eq!(pick(&sun, &e), Some(ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute }));
        let cars = DataSun { light: s, scenery: None, baked_gate: crate::BakedGate::Absolute };
        let sunless = ShadowLights { data_sun: Some(cars), ..ShadowLights::NONE };
        // E51 off: nothing; E51: the cars-only frame
        assert_eq!(pick(&sunless, &e), None);
        e.set_level(E51, 1);
        assert_eq!(pick(&sunless, &e), Some(ShadowMode::CarsOnly { small: None }));
        // CANYON-SMALLSH: the small parts ride on the cars-only frame (E51 on); with E51 off the frame is not drawn
        let small = SceneryCasters::Small { radius: 15.0, height: 8.0 };
        let with_small = ShadowLights { data_sun: Some(DataSun { light: s, scenery: Some(small), baked_gate: crate::BakedGate::Relative }), ..ShadowLights::NONE };
        assert_eq!(pick(&with_small, &e), Some(ShadowMode::CarsOnly { small: Some(small) }));
        e.set_level(E51, 0);
        assert_eq!(pick(&with_small, &e), None);
        e.set_level(E51, 1);
        assert_eq!(ShadowMode::CarsOnly { small: Some(small) }.params(100.0), ShadowMode::CarsOnly { small: None }.params(100.0), "the cars' shadow's strength and gate are the cars-only frame's");
        assert!(!ShadowMode::CarsOnly { small: Some(small) }.decal_fades());
        // SUN-FULL does not need E51 on; its casters' rule rides along (SHADOW-UNIFY: the shape rule; a radius only by test glue)
        e.set_level(E51, 0);
        let full = ShadowLights { data_sun: Some(DataSun { light: s, scenery: Some(SceneryCasters::Shape), baked_gate: crate::BakedGate::Absolute }), ..ShadowLights::NONE };
        assert_eq!(pick(&full, &e), Some(ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute }));
        let full = ShadowLights { data_sun: Some(DataSun { light: s, scenery: Some(SceneryCasters::Radius(15.0)), baked_gate: crate::BakedGate::Absolute }), ..ShadowLights::NONE };
        assert_eq!(pick(&full, &e), Some(ShadowMode::Sun { casters: SceneryCasters::Radius(15.0), baked_gate: crate::BakedGate::Absolute }));
        // FOREST-TREESH: the gate's switch rides along and reaches the shader's params
        let ungated = ShadowLights { data_sun: Some(DataSun { light: s, scenery: Some(SceneryCasters::Shape), baked_gate: crate::BakedGate::Off }), ..ShadowLights::NONE };
        let m = pick(&ungated, &e).unwrap();
        assert_eq!(m, ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Off });
        assert_eq!(m.params(100.0)[2], GATE_OFF);
        assert_eq!(ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute }.params(100.0)[2], 0.0);
        // GATE-REL: the relative gate with the track's bake light; without one it is the absolute gate
        let rel = DataSun { light: s, scenery: Some(SceneryCasters::Shape), baked_gate: crate::BakedGate::Relative };
        let bake = crate::BakeLight { to_light: Vec3::Y, base: 0.3, gain: 0.3, ground: 1.5 };
        let m = pick(&ShadowLights { data_sun: Some(rel), bake: Some(bake), ..ShadowLights::NONE }, &e).unwrap();
        assert_eq!(m, ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Relative });
        assert_eq!(m.params(100.0)[2], GATE_REL);
        let m = pick(&ShadowLights { data_sun: Some(rel), ..ShadowLights::NONE }, &e).unwrap();
        assert_eq!(m, ShadowMode::Sun { casters: SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute });
        let u = bake_uniform(Some(&bake));
        assert_eq!(u[1][..2], [0.3, 1.5 * GATE_REL_GROUND_SCALE]);
        assert_eq!(u[2][1], 1.0, "the cap mode by default");
        // the moon at E50's level
        let night = ShadowLights { moon: Some(s), ..ShadowLights::NONE };
        assert_eq!(pick(&night, &e), None);
        e.set_level(E50, 2);
        assert_eq!(pick(&night, &e), Some(ShadowMode::Moon { level: 2 }));
        // SHADOW-OVERCAST: a track's cap holds the level down, never up, and a cap of 0 is no moon
        let capped = ShadowLights { moon_level_cap: Some(1), ..night };
        assert_eq!(pick(&capped, &e), Some(ShadowMode::Moon { level: 1 }));
        e.set_level(E50, 1);
        assert_eq!(pick(&capped, &e), Some(ShadowMode::Moon { level: 1 }));
        e.set_level(E50, 0);
        assert_eq!(pick(&capped, &e), None);
        e.set_level(E50, 2);
        assert_eq!(pick(&ShadowLights { moon_level_cap: Some(0), ..night }, &e), None);
        // which modes fade the decals and take the car's patch
        let modes = [ShadowMode::Sun { casters: SceneryCasters::Radius(1.0), baked_gate: crate::BakedGate::Absolute }, ShadowMode::CarsOnly { small: None }, ShadowMode::Moon { level: 1 }];
        assert_eq!(modes.map(|m| m.decal_fades()), [true, false, false]);
        assert_eq!(modes.map(|m| m.takes_car_patch()), [true, true, false]);
        // SHADOW-OVERCAST: the precipitation weakens the sun's and the data sun's shadows, not the moon's
        assert_eq!(modes.map(|m| m.fades_with_precipitation()), [true, true, false]);
    }
}
