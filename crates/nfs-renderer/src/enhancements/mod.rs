//! The enhanced game's switches (`RE/notes/ENHANCEMENTS.md`, `REMASTER-PLAN.md`).
//!
//! **Not the original.** Everything an enhancement does is marked `ENHANCEMENT E<n>` in code and
//! is gated on one switch of [`Enhancements`]. The set of switches is [`REGISTRY`]; adding an
//! enhancement is one [`Entry`] there plus one [`Switch`] constant beside it.
//!
//! **The guard.** [`Enhancements::ORIGINAL`] (every level 0) is the original's code path: a GPU-side
//! branch that tests a switch is not entered, so the targets, formats, sample counts and passes
//! are `master`'s. The set reaches the GPU side through the renderer's command list
//! ([`crate::Renderer::set_enhancements`], the same path as `set_gamma`), which is also what the
//! render thread (D5) executes, in call order, at the start of the frame it belongs to. The core
//! starts with [`Enhancements::ORIGINAL`] until the game sets it.
//!
//! **Levels.** A switch is not a bool but a small *level*: index 0 is always "off" (= the original),
//! the others are the entry's choices ([`Entry::levels`], with captions for the menu's combo); the
//! shipped default of an entry is its own [`Entry::default_level`] (MSAA: 4x, not 8x). A plain
//! on/off switch is just the levels `[off, on]`. [`Enhancements::is_on`] is `level > 0`.
//!
//! The struct is a fixed array of levels rather than one field per switch because it is `Copy`
//! (the scene hand-off `..*self`) and a new switch must be a registry entry, not a struct edit.

/// One switch's registry row.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// `E1` — the token of `NFS_ENH=E1,E3`, the key in `enhancements.json` and the `ENHANCEMENT E1`
    /// marker in code
    pub token: &'static str,
    /// the menu row's caption, `[English, Russian]`
    pub name: [&'static str; 2],
    /// the one-line help of the row, `[English, Russian]`
    pub help: [&'static str; 2],
    /// the choices, `[English, Russian]` captions in the combo's order; index 0 is always "off"
    /// (the original), so there are at least two and at most [`MAX_LEVELS`]
    pub levels: &'static [[&'static str; 2]],
    /// the level the shipped game starts on (and `NFS_ENH=all` / a bare `E1` give);
    /// it is below `levels.len()`; 0 = off by default (E3, E4, E6: the original look)
    pub default_level: u8,
    /// the platforms that have the entry's backend ([`Entry::available`])
    pub platforms: Platforms,
}

/// Which platforms an entry's backend exists on. Entries stay in the registry on every platform (a `cfg` on an
/// entry would shift the later switch indices per platform) but where the rule says no they are
/// [`Entry::available`] = false: no menu row, `NFS_ENH` / the stored file cannot set them, `all` leaves them 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platforms {
    /// every platform
    All,
    /// macOS only (E33 EDR, E34 MetalFX)
    MacOnly,
}

impl Entry {
    /// Whether this platform has the entry's backend ([`Entry::platforms`]; E37 (FSR 3.1) is on every platform since the vendored
    /// shaders run on Metal too, ENH-UPSCALER: the SPD pyramid's read-write `Rg16Float` images became `Rgba16Float`).
    pub fn available(&self) -> bool {
        match self.platforms {
            Platforms::All => true,
            Platforms::MacOnly => cfg!(target_os = "macos"),
        }
    }
}

/// How many entries the menu shows (the available ones).
pub fn visible_count() -> usize {
    REGISTRY.iter().filter(|e| e.available()).count()
}

/// The registry index of the `k`-th entry the menu shows.
pub fn visible_index(k: usize) -> usize {
    REGISTRY.iter().enumerate().filter(|(_, e)| e.available()).nth(k).map_or(k, |(i, _)| i)
}

/// Index of an [`Entry`] in [`REGISTRY`]; the constants beside the registry are the code's way to
/// ask for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Switch(pub u8);

impl Switch {
    /// The entry's token (`E1`, ...): the `NFS_ENH` token and the `enhancements.json` key, from [`REGISTRY`].
    pub const fn token(self) -> &'static str {
        REGISTRY[self.0 as usize].token
    }
}

/// The captions of a plain on/off switch: levels `[off, on]`.
pub const ON_OFF: &[[&str; 2]] = &[["off", "выкл"], ["on", "вкл"]];

mod levels;
mod registry;
pub use levels::*;
pub use registry::REGISTRY;

/// The [`Switch`] constants. `E7,` declares `pub const E7: Switch = Switch(index_of("E7"))`: the index is the position of the
/// entry with that token in [`REGISTRY`], found AT COMPILE TIME (a token with no entry does not compile). With the tokens unique
/// and one constant per registry row (both checked at compile time too) a constant cannot point at another entry. REGISTRY's order
/// never changes (the indices are the menu order and the `Enhancements` slots): a new entry is appended there, its constant here.
macro_rules! switches {
    ($($(#[$doc:meta])* $name:ident,)*) => {
        $($(#[$doc])* pub const $name: Switch = Switch(index_of(stringify!($name)));)*
        const _: () = assert!([$(stringify!($name)),*].len() == REGISTRY.len(), "a REGISTRY entry has no switch constant");
    };
}

/// The index of the [`REGISTRY`] entry whose token is `token`, in a const context; a compile error when there is none.
const fn index_of(token: &str) -> u8 {
    let mut i = 0;
    while i < REGISTRY.len() {
        if str_eq(REGISTRY[i].token, token) {
            return i as u8;
        }
        i += 1;
    }
    panic!("no REGISTRY entry has this token")
}

/// `a == b` in a const context.
const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// No two entries of `reg` share a token.
const fn tokens_unique(reg: &[Entry]) -> bool {
    let mut i = 0;
    while i < reg.len() {
        let mut j = i + 1;
        while j < reg.len() {
            if str_eq(reg[i].token, reg[j].token) {
                return false;
            }
            j += 1;
        }
        i += 1;
    }
    true
}

const _: () = assert!(tokens_unique(REGISTRY), "two REGISTRY entries share a token");
const _: () = assert!(REGISTRY.len() <= MAX_SWITCHES, "REGISTRY has more entries than an Enhancements holds (MAX_SWITCHES)");

switches! {
    E1,
    E2,
    E3,
    E4,
    E5,
    E6,
    E7,
    E8,
    E9,
    /// ENHANCEMENT E12 soft particles
    E12,
    E10,
    E11,
    /// ENHANCEMENT E16 car light from the surroundings (SH probe of E8's faces)
    E16,
    /// ENHANCEMENT E15 headlight spots
    E15,
    E13,
    /// ENHANCEMENT E17 water
    E17,
    /// ENHANCEMENT E14 corner shading (AO)
    E14,
    /// ENHANCEMENT E18 particles in HDR
    E18,
    /// ENHANCEMENT E19 fog sun in-scatter + output dither
    E19,
    // index 19 was E20 (lightning lights the scene), removed 2026-11-13; the later switches moved down by one,
    // as when E30 left (an old file's "E20" key is ignored, an `NFS_ENH` "E20" is reported unknown)
    /// ENHANCEMENT E21 eye adaptation
    E21,
    /// ENHANCEMENT E23 street lamps as lights
    E23,
    /// ENHANCEMENT E24 headlights light up in tunnels (render side only)
    E24,
    // index 22 was E25 (soft shadows when overcast), removed 2026-11-16 (SHADOW-OVERCAST); the later switches moved down by one,
    // as when E20 / E30 left (an old file's "E25" key is ignored, an `NFS_ENH` "E25" is reported unknown)
    /// ENHANCEMENT E22 frame rate limit
    E22,
    /// ENHANCEMENT E27 motion blur
    E27,
    /// ENHANCEMENT E28 lens dirt
    E28,
    /// ENHANCEMENT E29 camera shake
    E29,
    /// ENHANCEMENT E26 wet road and rain
    E26,
    // index 29 was E30 (photo mode), removed 2026-10-27 (photo mode is always on now, PORT-DEVIATION D46 of master); the
    // later switches moved down by one. Nothing stores an index: `enhancements.json` and `NFS_ENH` name the tokens, which are
    // unchanged (an old file's "E30" key is ignored, an `NFS_ENH` "E30" is reported unknown)
    /// ENHANCEMENT E31 depth of field in replays
    E31,
    /// ENHANCEMENT E32 baked bounce light (global illumination)
    E32,
    /// ENHANCEMENT E35 SMAA 1x over the tone-mapped image
    E35,
    /// ENHANCEMENT E36 tunnel haze and visible headlight beams in fog / rain / tunnels
    E36,
    /// ENHANCEMENT E34 MetalFX upscaling (registered on every platform, after E36, so the other indices do not move; only macOS has the backend)
    E34,
    /// ENHANCEMENT E33 HDR display / EDR output (registered on every platform, after E34; the backend is macOS only)
    E33,
    /// ENHANCEMENT E37 FSR 3.1 temporal upscaling (registered last, on every platform; the backend is Windows / Linux, macOS only with `NFS_FSR_FORCE`)
    E37,
    /// ENHANCEMENT ENH-UPSCALER: the upscaler choice (0 off, 1 MetalFX, 2 FSR 3.1; registered after E37, on every platform, level 1 is macOS only)
    E38,
    /// ENHANCEMENT ENH-UPSCALER: the upscaler's RCAS sharpness (0 off, then 0.2 / 0.3 / 0.5 / 0.8; FSR's own, MetalFX's through `sharpen.rs`)
    E39,
    /// ENHANCEMENT E40 raindrops on the windscreen of the driver view (registered last, after E39)
    E40,
    /// ENHANCEMENT E41 the HD texture pack: 0 off, 1 on (registered last, after E40; read by the asset loaders, no GPU pass)
    E41,
    /// ENHANCEMENT E42 rounded details of the cars: 0 off, 1 soft, 2 strong (registered last, after E41; read by the car loader, no GPU pass)
    E42,
    /// ENHANCEMENT E43 the soft contact shadow under the cars: 0 off, 1 on (registered last, after E42)
    E43,
    /// ENHANCEMENT E44 the lighthouse beam as soft volumetric light: 0 off, 1 on (registered last, after E43; needs E7)
    E44,
    /// ENHANCEMENT E47 the cars' shadows from the street / tunnel lamps (E23): 0 off, 1 on (registered last, after E44; needs E23)
    E47,
    /// ENHANCEMENT E48 the chase / heli cameras' steady slope lift: 0 off (the original), 1 on (registered last, after E47)
    E48,
    /// ENHANCEMENT E49 reflections of nearby objects: 0 off, 1 on (registered after E48; needs E8)
    E49,
    /// ENHANCEMENT E50 moonlight on the night tracks: 0 off, 1 weak, 2 normal (registered after E49; needs E9)
    E50,
    /// ENHANCEMENT E51 the cars' own sun shadow on dimly baked roads: 0 off, 1 where the sun reaches the car, 2 always (registered after E50; needs E9)
    E51,
}

/// ENH-LITE: the «лёгкий» (light) preset — the levels a weak (integrated) GPU runs the enhanced game on.
/// A token listed gets that level, **every other entry is 0 (off)** (so an enhancement registered later is
/// off in the light preset until someone measures it and lists it here). Chosen from the ablation on a
/// real weak GPU (Intel UHD Ice Lake GT1, Vulkan, 1920x1080, alps with 3 opponents): the entries that keep
/// the most visible look for the least frame time; the table and the measurements are in
/// `RE/notes/ENHANCEMENTS.md` "Light preset (ENH-LITE)". Render-only like every enhancement.
pub const LITE_PRESET: &[(Switch, u8)] = &[
    (E1, 1),  // MSAA 2x: +3 ms — 2026-10-04: smooth edges AND the reflections, both
    (E2, 3),  // anisotropic 16x: +0.4 ms, the sharpest road at no real cost
    (E5, 2),  // draw distance +50 %: +0.1 ms
    (E7, 1),  // HDR scene "as the original": +1.5 ms, the base the others need
    (E8, 2),  // body clear coat, normal: a few tenths of a ms with E16 (the 256 px cube)
    (E12, 1), // soft particles: ~0.2 ms
    (E16, 1), // car ambient probe (needs E8)
    (E17, 1), // water: 0 where there is none, ~0 where there is
    (E18, 1), // particles in HDR
    (E19, 1), // fog sun glow + dither: +0.3 ms
    (E21, 1), // eye adaptation: +0.3 ms
    (E23, 1), // street lamps, soft: 0 by day, +5.6 ms on a night street (the one costly entry kept: the night look)
    (E24, 1), // headlights in tunnels
    (E26, 1), // wet road and rain
    (E32, 2), // baked bounce light, medium as the full defaults (baked once per track: no frame cost)
    (E42, 1), // rounded details, soft: load-time CPU (~10 ms a car) and a few thousand triangles, no render pass
    (E43, 1), // contact shadow: stands in for the blob (the light preset has no E9), analytic, a few tenths of a ms
    (E44, 1), // lighthouse beam: one analytic full-screen pass on coastal only, a few tenths of a ms there
    // no SMAA (E35, +5.6 ms on this GPU)
];

/// ENHANCEMENT ENH-UPSCALER: when a setting cannot act. Every row of [`DEPENDS`] says "entry `entry` has no effect while `cond`
/// holds"; the panel shows such a row inactive with the reason as its hint (its stored level is kept: switching the cause back
/// restores it), and [`Enhancements::resolved`] applies the `Needs` rows to a set (data only; the unit test
/// `resolved_zeroes_the_dependents` checks that it and [`Enhancements::blocked`] agree). Derived from the code's `is_on` / `level`
/// gates in `gpu.rs` and `gpu/csm/`, 2026-10-04.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    /// the entry with this token is off (level 0)
    Needs(&'static str),
    /// an upscaler is chosen (E38): it sets the internal size itself, so E3's render scale is not used
    UpscalerOn,
    /// a TEMPORAL upscaler is active (FSR 3.1, or MetalFX's temporal mode): the main view is drawn at 1 sample with no
    /// post-process anti-aliasing (E1 MSAA and E35 SMAA are replaced by the upscaler's own temporal filter)
    TemporalUpscaler,
    /// UPSCALE-SOFT: the chosen upscaler is a TEMPORAL one (FSR 3.1, MetalFX's temporal mode) and E7 is off: the temporal backends take
    /// the float scene (`temporal_frame` / `enh_fsr` need `hdr`), so nothing is upscaled (MetalFX's spatial mode works without E7)
    TemporalNeedsHdr,
    /// no HD texture pack was found beside the game ([`set_hd_textures_available`])
    NoHdPack,
}

/// Whether this run has an HD texture pack, set once at start-up: E41's row is grey without one.
static HD_TEXTURES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_hd_textures_available(yes: bool) {
    HD_TEXTURES.store(yes, std::sync::atomic::Ordering::Relaxed);
}

pub fn hd_textures_available() -> bool {
    HD_TEXTURES.load(std::sync::atomic::Ordering::Relaxed)
}

/// The row "`entry` has no effect while `on` is off" (a [`Cond::Needs`] row).
const fn needs(entry: Switch, on: Switch) -> Dependency {
    Dependency { entry: entry.token(), cond: Cond::Needs(on.token()) }
}

/// One "has no effect while" row, see [`Cond`].
#[derive(Debug, Clone, Copy)]
pub struct Dependency {
    pub entry: &'static str,
    pub cond: Cond,
}

/// Every dependency / override of the enhancements (the panel's inactive rows; the renderer documents them here too).
/// Only RULES THE CODE ENFORCES are listed: a "needs" that the help texts mention but no gate checks is not here (E36's beams and
/// E15 spotlights, E24 and E15, E19's fog glow which does not need HDR, E22's levels
/// 5 / 6 with the frame output (a fallback, not a block), E17 / E8 (only at night, partial)).
/// The tokens are the [`Switch`] constants' own (`E7.token()`), so a row cannot name an entry that does not exist.
pub const DEPENDS: &[Dependency] = &[
    Dependency { entry: E41.token(), cond: Cond::NoHdPack },
    // the float scene (E7) is what the post effects work on: with E7 off `enh_hdr` gives None and every `hdr` / `cur_hdr` gate fails
    needs(E10, E7),
    needs(E11, E7),
    needs(E13, E7),
    needs(E18, E7),
    needs(E21, E7),
    needs(E27, E7),
    needs(E31, E7),
    needs(E33, E7),
    needs(E40, E7),
    needs(E36, E7),
    needs(E44, E7),
    // the lens dirt is added by the bloom pass (inside `if bloom_level > 0`), so it needs E10 and, through it, E7
    needs(E28, E7),
    needs(E28, E10),
    // the car-ambient probe samples E8's environment faces
    needs(E16, E8),
    // the lamp shadows shadow E23's lamps: without them there is no lamp light to shadow
    needs(E47, E23),
    // the moonlight is a third light of E9's cascades, and the cars' dim-road shadow a cars-only map of them
    needs(E50, E9),
    needs(E51, E9),
    // the reflections of nearby objects are drawn into E8's cube and read by its clear coat
    needs(E49, E8),
    // an upscaler draws the scene at its own internal size: E3's scale is not used; a temporal one also draws at 1 sample with
    // no SMAA pass (the spatial MetalFX mode keeps both)
    Dependency { entry: E3.token(), cond: Cond::UpscalerOn },
    Dependency { entry: E1.token(), cond: Cond::TemporalUpscaler },
    Dependency { entry: E35.token(), cond: Cond::TemporalUpscaler },
    // UPSCALE-SOFT: a temporal upscaler runs only on the float scene: with E7 off the choice and its sharpness do nothing (the
    // spatial MetalFX mode is not blocked; the mode / scale rows stay editable, so the player can pick it)
    Dependency { entry: E38.token(), cond: Cond::TemporalNeedsHdr },
    Dependency { entry: E39.token(), cond: Cond::TemporalNeedsHdr },
];

/// Why a row is inactive: the [`DEPENDS`] row, and for the upscaler conditions which backend (the E38 level, to name it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blocked {
    pub dep: usize,
    pub by: u8,
}

impl Enhancements {
    /// ENHANCEMENT ENH-UPSCALER: the set the renderer works with. E34 (MetalFX's mode + scale) counts only while E38 = MetalFX (macOS),
    /// E37 (FSR's scale) only while E38 = FSR 3.1; the other is 0. A chosen backend whose level is still 0 (`NFS_ENH=E38=2`, a first
    /// choice) takes the default: MetalFX temporal 67 % (E34 = 2), FSR 1.5x (E37 = 2). The stored set keeps every value (a hidden row's
    /// level survives); only this view zeroes them. With `E38` = 0 and E34 / E37 > 0 in a hand-made set nothing is on.
    pub fn effective(&self) -> Self {
        let mut e = *self;
        let up = match e.level(E38) {
            UP_METALFX if !cfg!(target_os = "macos") => UP_OFF,
            u => u,
        };
        if up == UP_METALFX {
            if e.level(E34) == 0 {
                e.set_level(E34, 2);
            }
        } else {
            e.set_level(E34, 0);
        }
        if up == UP_FSR {
            if e.level(E37) == 0 {
                e.set_level(E37, 2);
            }
        } else {
            e.set_level(E37, 0);
        }
        e
    }

    /// The old files / `NFS_ENH` lists have no E38: a set with E34 > 0 (macOS) means MetalFX, else with E37 > 0 means FSR 3.1 (both: the
    /// platform's own, MetalFX on macOS). The sharpness (E39), new, then takes its default (E37 always had 0.3; UPSCALE-SOFT: MetalFX's too) unless `e39_named`.
    pub fn infer_upscaler(&mut self, e39_named: bool) {
        let up = if self.level(E34) > 0 && cfg!(target_os = "macos") {
            UP_METALFX
        } else if self.level(E37) > 0 {
            UP_FSR
        } else {
            return;
        };
        self.set_level(E38, up);
        if !e39_named && self.level(E39) == 0 {
            self.set_level(E39, REGISTRY[E39.0 as usize].default_level);
        }
    }

    /// Whether a TEMPORAL upscaler is active (the effective set: FSR 3.1, or MetalFX's temporal mode, and E7 on: UPSCALE-SOFT, the
    /// renderer's `temporal_frame` / `enh_fsr` need the float scene).
    pub fn temporal_upscaler(&self) -> bool {
        self.temporal_chosen() && self.level(E7) > 0
    }

    /// A TEMPORAL upscaler is chosen (FSR 3.1, or MetalFX's temporal mode), whether or not E7 lets it run.
    fn temporal_chosen(&self) -> bool {
        let e = self.effective();
        e.level(E37) > 0 || (1..=E34_TEMPORAL_LEVELS).contains(&e.level(E34))
    }

    /// Whether an upscaler sets the internal size this frame: MetalFX's spatial mode (no E7 needed) or a running temporal one.
    pub fn upscaler_running(&self) -> bool {
        self.effective().level(E34) > E34_TEMPORAL_LEVELS || self.temporal_upscaler()
    }

    /// The level a row shows: its own, but "off" for the textures with no pack (the stored level is kept for when one appears).
    pub fn shown_level(&self, sw: Switch) -> u8 {
        if sw == E41 && !hd_textures_available() { 0 } else { self.level(sw) }
    }

    /// Why entry `token` has no effect now, `None` when it acts (or nothing is known to block it). The first unmet row of [`DEPENDS`].
    pub fn blocked(&self, token: &str) -> Option<Blocked> {
        let eff = self.effective();
        DEPENDS.iter().enumerate().filter(|(_, d)| d.entry == token).find_map(|(i, d)| {
            let hit = match d.cond {
                Cond::Needs(t) => REGISTRY.iter().position(|e| e.token == t).is_some_and(|k| self.levels[k] == 0),
                // UPSCALE-SOFT: a temporal one with E7 off does not run, so it blocks nothing
                Cond::UpscalerOn => self.upscaler_running(),
                Cond::TemporalUpscaler => self.temporal_upscaler(),
                Cond::TemporalNeedsHdr => self.temporal_chosen() && self.level(E7) == 0,
                Cond::NoHdPack => !hd_textures_available(),
            };
            hit.then(|| Blocked { dep: i, by: if matches!(d.cond, Cond::Needs(_)) { 0 } else { eff.level(E38).min(UP_FSR) } })
        })
    }

    /// The [`effective`](Self::effective) set with every `Needs` row of [`DEPENDS`] applied: an entry whose prerequisite reads 0
    /// reads 0 too, through chains (E28 -> E10 -> E7). Pure data, the registry's own statement of what can act; the renderer keeps
    /// its own gates (this does not replace them). The upscaler rows ([`Cond::UpscalerOn`] / [`Cond::TemporalUpscaler`]) are not
    /// levels and are not applied here.
    pub fn resolved(&self) -> Self {
        let mut e = self.effective();
        let index = |t: &str| REGISTRY.iter().position(|r| r.token == t);
        loop {
            let mut changed = false;
            for d in DEPENDS {
                let Cond::Needs(t) = d.cond else { continue };
                if let (Some(k), Some(i)) = (index(t), index(d.entry))
                    && e.levels[k] == 0
                    && e.levels[i] != 0
                {
                    e.levels[i] = 0;
                    changed = true;
                }
            }
            if !changed {
                return e;
            }
        }
    }
}

/// How strong the GPU is, for the first-run choice of the default set (ENH-LITE).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuClass {
    /// a discrete GPU, Apple Silicon, anything not known to be weak: the full defaults
    Strong,
    /// an integrated GPU (not Apple Silicon) or a software rasteriser: the light preset
    Weak,
}

/// The class of an adapter: a CPU / software adapter or an integrated GPU is [`GpuClass::Weak`] —
/// except Apple Silicon (a Metal adapter named "Apple ...": integrated, but as strong as a mid-range
/// discrete card, the full defaults run it at 60 fps), and an Intel Mac's integrated GPU is weak.
pub fn classify_gpu(device_type: wgpu::DeviceType, backend: wgpu::Backend, name: &str) -> GpuClass {
    let apple_silicon = backend == wgpu::Backend::Metal && name.trim_start().to_ascii_lowercase().starts_with("apple");
    match device_type {
        wgpu::DeviceType::Cpu => GpuClass::Weak,
        wgpu::DeviceType::IntegratedGpu if !apple_silicon => GpuClass::Weak,
        _ => GpuClass::Strong,
    }
}

/// The most switches an [`Enhancements`] holds.
pub const MAX_SWITCHES: usize = 64;
/// The most levels an [`Entry`] has (the menu's string ids reserve this many per row).
pub const MAX_LEVELS: usize = 8;

/// One stored value of a switch: a level, or the old boolean form of the first file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stored {
    Level(u8),
    /// `true` = the entry's default level, `false` = off
    Bool(bool),
}

/// The level of every switch (0 = off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enhancements {
    levels: [u8; MAX_SWITCHES],
}

impl Default for Enhancements {
    fn default() -> Self {
        Self::ORIGINAL
    }
}

/// The levels whose meaning code tests by number (the captions of their [`REGISTRY`] rows), one name per meaning: what a gate
/// compares a [`Enhancements::level`] with. Associated constants, so the crate's users reach them through the exported type.
impl Enhancements {
    /// E1, the MSAA: «2x»
    pub const E1_2X: u8 = 1;
    /// E6, the present mode: «mailbox (low lag)»
    pub const E6_MAILBOX: u8 = 1;
    /// E6: «immediate (no vsync)» (the front end is held to 120 frames a second then)
    pub const E6_IMMEDIATE: u8 = 2;
    /// E16, the car light from the surroundings: «stronger»
    pub const E16_STRONGER: u8 = 2;
    /// E17, the water: «stronger»
    pub const E17_STRONGER: u8 = 2;
    /// E22, the frame rate limit: levels 1..=4 cap at these frames a second
    pub const E22_FPS: [f64; 4] = [30.0, 60.0, 120.0, 240.0];
    /// E22: «display» (the display's refresh rate)
    pub const E22_DISPLAY: u8 = 5;
    /// E22: «auto (60 on battery)»
    pub const E22_AUTO: u8 = 6;
    /// E51, the cars' own sun shadow: «where the sun reaches» (the renderer measures each car's share of the sun)
    pub const E51_WHERE_SUN: u8 = 1;
    /// E51: «always»
    pub const E51_ALWAYS: u8 = 2;
}

impl Enhancements {
    /// Every switch off: the original, `master` byte for byte.
    pub const ORIGINAL: Self = Self { levels: [0; MAX_SWITCHES] };

    /// Every switch at its default level (the shipped default).
    pub fn all() -> Self {
        Self::all_in(REGISTRY)
    }

    /// Whether the switch is on (its level is above 0).
    pub fn is_on(&self, s: Switch) -> bool {
        self.level(s) > 0
    }

    /// The switch's level; 0 = off, and always 0 for an index out of range.
    pub fn level(&self, s: Switch) -> u8 {
        self.levels.get(s.0 as usize).copied().unwrap_or(0)
    }

    /// Set a level (an index out of range is ignored; the caller keeps it below its entry's count).
    pub fn set_level(&mut self, s: Switch, level: u8) {
        if let Some(l) = self.levels.get_mut(s.0 as usize) {
            *l = level;
        }
    }

    /// Whether any switch is on — the guard the GPU side may test first.
    pub fn any_on(&self) -> bool {
        self.levels.iter().any(|&l| l > 0)
    }

    /// The raw levels (tests).
    #[cfg(test)]
    pub fn levels(&self) -> &[u8; MAX_SWITCHES] {
        &self.levels
    }

    /// `NFS_ENH`: `off` (the original), `all` (every default), or a list `E1,E3=0,E4=2` — a bare
    /// token is that switch at its default level, `token=n` an explicit level, the others off.
    /// `None` for an empty value. Case-insensitive; an unknown token or a level out of range in a
    /// list is skipped and pushed to `unknown`.
    pub fn parse_override(value: &str, unknown: &mut Vec<String>) -> Option<Self> {
        Self::parse_override_in(REGISTRY, value, unknown)
    }

    /// The switches that are on, `E1,E3=1`: a bare token at its default level; `all` / `off` when
    /// every one is at its default / none is on. Parses back to the same set.
    pub fn describe(&self) -> String {
        self.describe_in(REGISTRY)
    }

    /// The light preset ([`LITE_PRESET`]): the listed entries at their level, the rest off.
    pub fn lite() -> Self {
        Self::lite_in(REGISTRY, LITE_PRESET.iter().map(|(s, l)| (s.token(), *l)))
    }

    /// A preset table of `(token, level)` over a registry: the listed tokens at their level (clamped), the rest off, an unknown or
    /// unavailable token skipped.
    pub fn lite_in<'a>(reg: &[Entry], table: impl IntoIterator<Item = (&'a str, u8)>) -> Self {
        let mut out = Self::ORIGINAL;
        for (tok, lvl) in table {
            if let Some(i) = reg.iter().position(|e| e.token == tok).filter(|i| *i < MAX_SWITCHES && reg[*i].available()) {
                out.levels[i] = lvl.min(reg[i].levels.len().saturating_sub(1) as u8);
            }
        }
        out
    }

    // The same operations over any registry — what the unit tests drive.

    pub fn all_in(reg: &[Entry]) -> Self {
        let mut out = Self::ORIGINAL;
        for (l, e) in out.levels.iter_mut().zip(reg) {
            *l = if e.available() { e.default_level } else { 0 };
        }
        out
    }

    pub fn parse_override_in(reg: &[Entry], value: &str, unknown: &mut Vec<String>) -> Option<Self> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("off") || v.eq_ignore_ascii_case("none") {
            return Some(Self::ORIGINAL);
        }
        if v.eq_ignore_ascii_case("all") || v.eq_ignore_ascii_case("on") {
            return Some(Self::all_in(reg));
        }
        if v.eq_ignore_ascii_case("lite") || v.eq_ignore_ascii_case("light") {
            return Some(Self::lite_for(reg));
        }
        if v.is_empty() {
            return None;
        }
        let mut out = Self::ORIGINAL;
        // ENHANCEMENT ENH-UPSCALER: which of E38 / E39 the list names itself (the others are inferred from E34 / E37, see below)
        let (mut named_e38, mut named_e39) = (false, false);
        for tok in v.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let key = tok.split_once('=').map_or(tok, |(n, _)| n.trim());
            named_e38 |= key.eq_ignore_ascii_case("E38");
            named_e39 |= key.eq_ignore_ascii_case("E39");
            // `all,E16=2`: every default, then the listed overrides (a tooling convenience; `all` alone is as before)
            if tok.eq_ignore_ascii_case("all") {
                out = Self::all_in(reg);
                continue;
            }
            // `lite,E14=1`: the light preset, then the overrides
            if tok.eq_ignore_ascii_case("lite") || tok.eq_ignore_ascii_case("light") {
                out = Self::lite_for(reg);
                continue;
            }
            let (name, lvl) = match tok.split_once('=') {
                Some((n, l)) => (n.trim(), Some(l.trim())),
                None => (tok, None),
            };
            let Some(i) = reg.iter().position(|e| e.token.eq_ignore_ascii_case(name)).filter(|i| *i < MAX_SWITCHES && reg[*i].available()) else {
                unknown.push(tok.to_string());
                continue;
            };
            let level = match lvl {
                None => Some(reg[i].default_level),
                Some(l) => l.parse::<u8>().ok().filter(|l| (*l as usize) < reg[i].levels.len()),
            };
            match level {
                Some(l) => out.levels[i] = l,
                None => unknown.push(tok.to_string()),
            }
        }
        // ENHANCEMENT ENH-UPSCALER: the tokens of the old two-row menu keep working: `E34=2` / `E37=3` without an `E38` choose that backend
        if !named_e38 && std::ptr::eq(reg, REGISTRY) {
            out.infer_upscaler(named_e39);
        }
        Some(out)
    }

    /// The light preset of a registry: [`LITE_PRESET`] for the real one, an empty table for any other (the tests').
    fn lite_for(reg: &[Entry]) -> Self {
        if std::ptr::eq(reg, REGISTRY) { Self::lite() } else { Self::lite_in(reg, []) }
    }

    pub fn describe_in(&self, reg: &[Entry]) -> String {
        if !reg.is_empty() && *self == Self::all_in(reg) {
            return "all".into();
        }
        if std::ptr::eq(reg, REGISTRY) && *self == Self::lite() {
            return "lite".into();
        }
        let on: Vec<String> = reg
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < MAX_SWITCHES && self.levels[*i] > 0)
            .map(|(i, e)| if self.levels[i] == e.default_level { e.token.to_string() } else { format!("{}={}", e.token, self.levels[i]) })
            .collect();
        if on.is_empty() { "off".into() } else { on.join(",") }
    }

    /// `(token, level)` pairs for a registry — the storage form.
    pub fn pairs_in(&self, reg: &[Entry]) -> Vec<(&'static str, u8)> {
        reg.iter().enumerate().filter(|(i, _)| *i < MAX_SWITCHES).map(|(i, e)| (e.token, self.levels[i])).collect()
    }

    /// The set a stored list gives; a switch the list does not name keeps its default level, so a
    /// newly registered enhancement is on (at its default) for a player with an older file. A stored
    /// level beyond the entry's choices is clamped to the highest one; an old boolean is `true` =
    /// the default level, `false` = off.
    pub fn from_stored_in<'a>(reg: &[Entry], pairs: impl IntoIterator<Item = (&'a str, Stored)>) -> Self {
        let mut out = Self::all_in(reg);
        for (tok, v) in pairs {
            if let Some(i) = reg.iter().position(|e| e.token == tok).filter(|i| *i < MAX_SWITCHES && reg[*i].available()) {
                out.levels[i] = match v {
                    Stored::Bool(true) => reg[i].default_level,
                    Stored::Bool(false) => 0,
                    Stored::Level(l) => l.min(reg[i].levels.len().saturating_sub(1) as u8),
                };
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
