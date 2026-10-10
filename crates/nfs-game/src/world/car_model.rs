//! The GPU-resident car model: its parts, wheels, driver and light blocks, LOD passes and key animations (a move out of the parent module, no logic changed).

use super::*;

/// GPU-resident car model at one LOD, split into body parts (with their pivots applied) and
/// wheels (kept separate so they can spin/steer). Model space (RE/notes/car-model.md §3) is
/// +Y up, −Z front, +X left; `MODEL_TO_BODY` rotates it into the physics body frame (+Z forward).
///
/// `Clone` is cheap: every field but the GPU ids (`MeshId`/`MaterialId`/`TextureId`, plain indices)
/// is small metadata or a `Vec` of them — no GPU memory is duplicated by cloning alone. Used by
/// [`Self::instantiate`].
#[derive(Clone)]
pub struct CarModel {
    /// The model's scale in its render matrix: 1, or **0.4** for a `.TINYCAR` (`[0x5B37C8]`, the
    /// scale 0x4433D2 builds into the car's matrix through 0x539B30)
    pub scale: f32,
    /// A mirrored race (`[0x65742C]`, the race option `MIRROR`): the car draw 0x4433A1…0x4433F8
    /// builds the model's x scale as `−m·s` with `m = −1` there (0x44339B) — the model flipped
    /// back across the body's x, so under the mirrored view a car still reads the right way round
    /// (plates, the driver's side) — and slot 9 negates the steer channels 1 / 2 `.y` (0x438471).
    pub mirror: bool,
    pub body: Vec<(MeshId, MaterialId, Mat4)>,
    /// Each material's `R3DLightLayer` parameters (`RE/notes/lighting.md` §6) — every shipped car
    /// material has them.
    material_light: MaterialMap<nfs_formats::crp::LayerLight>,
    /// The materials whose render method takes the shadow projector (layer class 0x8102,
    /// `R3DShadowMapTextureLayer`).
    shadowed: MaterialSet,
    /// The meshes each **model animation** morphs (`Base+77`, index 1…7), with every frame's
    /// positions: 1 the convertible's roof, 2 the `SpoilerW` wing, 3 the wipers, 4 / 5 / 6 the
    /// driver's right arm, left arm and body, 7 the aerial (`anims` example over every car).
    morph: [Vec<MorphPart>; ANIMS],
    /// `0x438190(n)`: animation `n`'s length in frames, 0 for a car without it
    anim_length: [u32; ANIMS],
    /// the time each animation was last set to (0x4380D0), so an unchanged one is not re-uploaded
    anim_time: [f32; ANIMS],
    roof_state: i32,
    driver: DriverAnim,
    /// the `{frame, next, frac}` each animation's meshes were last blended to
    blended: [Option<(usize, usize, f32)>; ANIMS],
    /// The **rotation channel** of each `body` entry — the article's `Base+75`, 0 for a part that
    /// never moves (`car-model.md`). `R3DCarLOD::draw` picks it at 0x43C98A and turns
    /// `channel_turns[n]` into `Rx·Ry·Rz` about the part's own origin.
    body_channel: Vec<u8>,
    /// The **draw order** of each `body` entry — its article's place in its LOD's part list after
    /// 0x43C1B0's sort. 0x43C1B0 lists a LOD's parts as `{article, lod, Base+70}` in file order and
    /// `std::sort`s them by the byte at `+8`, signed, `a < b` (0x43C35B → 0x43E710 / 0x43E910 /
    /// 0x43EA20, `cmp bl, cl` / `jge`), and `R3DCarLOD::draw` 0x43C830 walks them in that order
    /// (the `lod_list` of the car's load, sorted by [`nfs_formats::stl_sort::vc6_sort`]). On the
    /// 993 the wheels are 0x0A and the wheel shadows, the `Shadow.fsh` quads over the wheels'
    /// outer faces, 0x0B — drawn after the wheel they darken.
    body_order: Vec<u32>,
    /// Each `body` entry's LOD and pass bytes ([`PartInfo`]): the model holds **every** LOD, and
    /// the car draw 0x444E00 picks per pass which one a part is drawn from ([`CarPass`]).
    body_info: Vec<PartInfo>,
    /// `R3DCarLOD+0x10` of each LOD 1…7 — how many parts 0x43C1B0 listed for it (the admitted
    /// articles with a `Base` LOD record for that LOD); the LOD pick 0x43A0DC…0x43A117 stops at
    /// the first empty one
    lod_parts: [u32; 7],
    /// `R3DCarLOD+0x14 + 4m` of each LOD: whether a part of it rides translation channel `m`
    /// (`Base+72`, 0x43C66D…0x43C67C) — the gate 0x43E670 writes channel `m` behind
    lod_moves: [[bool; 4]; 7],
    /// `.tpg` `movespoilerextent` → `R3DCar+0x74` (0x441D08), in **turns**: −0.04 on the 996 and
    /// the GT3, −0.08 on the two Boxsters, and absent everywhere else.
    move_spoiler_extent: f32,
    /// `.tpg` `headlightextent` → `R3DCar+0x70` (0x441CF7), in **turns**: 0.19 on the 914, −0.18
    /// on the 928, 0.14 on the 944 — the three Porsches with pop-up headlights, and no others.
    headlight_extent: f32,
    spoiler: ChannelEase,
    headlight: ChannelEase,
    /// `.tpg` `dooraxis` → `R3DCar+0x2B0` (0x441C92), default (0, 1, 0): the axis channels 6 and 7
    /// turn about, x negated for 7 (0x43CA5A…0x43CA7D)
    door_axis: Vec3,
    /// `.tpg` `hoodextent` → `+0x68` (0x441CCC), default **0.15** turn; `trunkextent` → `+0x6C`
    /// (0x441CE6), default **−0.15** — the targets of the two lid easers
    hood_extent: f32,
    trunk_extent: f32,
    /// the lid easers at `car+0x64 + 0x80` (hood) and `+ 0x90` (engine lid) that slot 9 writes
    /// into channels 8 and 9 (0x4387CE…0x43891C)
    hood: ChannelEase,
    trunk: ChannelEase,
    /// `object + 0x2DC + 12n` — three floats a channel, **rotations in turns** about x, y and z
    /// (0x43C9C0 / 0x43C9D7 / 0x43C9F8 each multiply by `[0x5B25CC]` = 360.0 before the
    /// axis-angle build 0x539BD0). Only the x column is used so far: channel **16** is
    /// `MoveSpoiler` and **17** the pop-up `HeadLight`.
    pub channel_turns: [f32; 32],
    /// the same channels' `y` and `z` turns — the wheels' steer and camber, the steering wheel's
    /// and the dial needles' `z`, the siren's `y` (slot 9, `car-model.md` "Slot 9")
    channel_yz: [[f32; 2]; 32],
    /// `nfs5.car +0x2F0…+0x2FC` (the options 0x251…0x254 `.TACH0RPM`, `.TACH1000RPM`,
    /// `.SPEEDO0KM`, `.SPEEDO100KM`, sent at 0x4EF331…0x4EF37B into `carinfo+0x524…+0x530`): the
    /// dash needles' rest angle and scale, in turns
    dash: [f32; 4],
    /// the frame clock the race channels run on, ms (`TIMER_getTicks` is 128 Hz)
    clock_ms: u64,
    /// (pivot in body space, is_front, is_left, parts)
    pub wheels: Vec<CarWheelModel>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub wheel_radius: f32,
    pub half_track: f32,
    /// The car's lamps: every glare article's `ef` record and its mirrored copy
    /// (`RE/notes/particles.md` §4).
    pub lights: Vec<CarLight>,
    /// The car's texture pages before any damage, the decals of `<model>D.fsh` / `<model>L.fsh` and
    /// the page textures to rewrite when they change (`RE/notes/car-model.md` §11).
    damage_pages: Vec<nfs_assets::car::TexturePage>,
    damage_patches: Vec<nfs_assets::car::DamagePatch>,
    page_textures: Vec<nfs_renderer::TextureId>,
    /// `R3DCar+0x6A8`: the ten zone values ([`zone_damage`]) the car was last brought up to date
    /// with — 0x438230 compares them exactly every frame (0x438240…0x438259)
    damage_shown: [f32; 10],
    /// the meshes 0x43E390 crushes: every part of an article with `Base+78 & 8` and a `dz` part
    deform: Vec<DeformPart>,
}

/// One mesh of a crushable part (0x43E390): its undamaged vertices (`vt#lod`), each vertex's crush
/// vector (`vt#(0x8000|lod)`, `w` 0) and its damage zone (the `dz#lod` list it is in; 255 none).
#[derive(Clone)]
struct DeformPart {
    mesh: MeshId,
    base: Vec<Vertex>,
    /// its mesh's triangle list — kept so [`CarModel::instantiate`] can re-upload a pristine copy
    /// of this part for a traffic/cop car instance without reading the (possibly already crushed)
    /// GPU buffer back
    indices: Vec<u32>,
    delta: Vec<[f32; 3]>,
    zone: Vec<u8>,
}

/// `0x5B35AC` / `0x5B35D4`: the caps 0x4382E0 puts on each zone's damage, × 100 — every zone 1 on
/// a car the camera does not follow, and on the one it does (`carInfo+0x520 == 0x465D80(0)`, with
/// slot 0's view non-zero or `[0x627F43] & 8` — any view) **0** on zones 3, 7 and 8, the sides and
/// the roof.
const ZONE_CAP: [f32; 10] = [1.0; 10];
const OWN_ZONE_CAP: [f32; 10] = [1.0, 1.0, 1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0];

/// **0x4382E0(zone)**: the zone's damage `car+0x4E4 + 4·zone` capped at `100 · cap` — `fst` of the
/// cap to a float, then the smaller (0x43831F…0x43837A, `test ah, 1`).
pub fn zone_damage(damage: &[f32; 10], own_car: bool) -> [f32; 10] {
    let caps = if own_car { OWN_ZONE_CAP } else { ZONE_CAP };
    std::array::from_fn(|z| {
        let cap = caps[z] * 100.0;
        if cap < damage[z] { cap } else { damage[z] }
    })
}

/// A lamp's `ef` record (`RE/notes/particles.md` §4), in the body frame.
#[derive(Debug, Clone, Copy)]
pub struct CarLight {
    /// `L+0x00`, the lamp's centre
    pub position: Vec3,
    /// `L+0x10` / `L+0x20`, its two half-axes
    pub axis1: Vec3,
    pub axis2: Vec3,
    /// `L+0x30`, the way it shines
    pub facing: Vec3,
    /// `L+0x40` / `L+0x44`, the two sprites' ARGB colours
    pub colour: [u32; 2],
    /// `L+0x4C`: 0 headlight, 1 brake, 2 reverse, 3 indicator, 4 glint (the 0x5CD73C row)
    pub kind: u8,
    /// the lamp slot `car+0x2D4[slot]` that switches it (0x443660)
    pub slot: u8,
    /// `L+0x4F`: the headlight that also throws the night beam 0x43D4F0
    pub night: bool,
    /// The article's own frame, model space (column form): its `tr` part for the LOD being drawn
    /// (0x59F220 looks up `"tr"` + `[this+8]`, 0x43D8FB…0x43D907), identity when it has none. The
    /// mirrored copy's is that matrix with its rotation transposed and its translation's x
    /// negated (0x43DB5B…0x43DC0D).
    pub frame: Mat4,
    /// The rotation channel turning the lamp inside that frame (0 = none): the article's
    /// `Base+0x4B` for the primary (0x43D943), `L+0x4D` for the mirrored copy (0x43DC36) — both
    /// only when the article has a `tr` and `Base+0x4B` is non-zero (0x43D94A / 0x43DC30).
    pub channel: u8,
}

/// Model animations are numbered 1…7 in the shipped cars; slot 0 is unused.
const ANIMS: usize = 8;

/// A multiplicative hash for the per-item material lookups ([`CarModel::draw_items_block`],
/// [`CarModel::apply_projector`]): a `MaterialId` is a small `u32`, and SipHash on it for every item of
/// every car every frame was the cost. Lookups only — nothing iterates these tables, so the order
/// a hasher gives is never seen.
#[derive(Default, Clone, Copy)]
struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type IdBuild = std::hash::BuildHasherDefault<IdHasher>;
type MaterialMap<V> = HashMap<MaterialId, V, IdBuild>;
type MaterialSet = std::collections::HashSet<MaterialId, IdBuild>;

/// The two work buffers of one [`CarModel::draw_items_into`]: a pass's items with their
/// `body_order` keys, and the `(key, index)` pairs sorted to put them in the original's order.
#[derive(Default)]
pub struct DrawScratch {
    items: Vec<(u32, DrawItem)>,
    order: Vec<(u32, u32)>,
}

thread_local! {
    /// [`CarModel::draw_items`]' buffers: the draw runs on one thread and does not nest.
    static DRAW_SCRATCH: std::cell::RefCell<DrawScratch> = std::cell::RefCell::new(DrawScratch::default());
}

/// The steering wheel's travel, `±4.5 / 23` of a turn (`.z = ch1.y + ch2.y` within it,
/// 0x43858F…0x4385FB; the frame index is `23·ch5.z + 4.5`, 0x438C4F…0x438C6B).
const STEERING_WHEEL_LIMIT: f32 = 4.5 / 23.0;

/// One of slot 9's eight keyframe players, `R3DCar+0x484 + 0x1C·n` (0x438B66…0x438C3D) — player
/// `n` drives animation `n`: `{count, table, start, duration, frame, next, frac}`.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct KeyPlayer {
    table: &'static [u32],
    start_ms: u64,
    duration_ms: u64,
    pub(super) frame: u32,
    pub(super) next: u32,
    pub(super) frac: f32,
}

impl KeyPlayer {
    pub(super) fn active(&self) -> bool {
        !self.table.is_empty()
    }

    /// A gesture starting now: frame and next are the table's first two when it has two
    /// (0x438DF1…0x438E18), the fraction 0.
    pub(super) fn start(&mut self, table: &'static [u32], now: u64, duration_ms: u64) {
        self.table = table;
        self.start_ms = now;
        self.duration_ms = duration_ms;
        if table.len() > 1 {
            self.frame = table[0];
            self.next = table[1];
        }
        self.frac = 0.0;
    }

    /// 0x438B73…0x438C30: while the elapsed time is under the duration, `p = elapsed / duration ·
    /// (count − 1)`, the frame `table[fistp(p − 0.5)]`, the next `table[fistp(p + 0.5)]` and the
    /// fraction `p − fistp(p − 0.5)`; past it the player stops, its last frame kept.
    pub(super) fn step(&mut self, now: u64) {
        if !self.active() {
            return;
        }
        let elapsed = now.saturating_sub(self.start_ms);
        if elapsed >= self.duration_ms {
            self.table = &[];
            return;
        }
        let p = elapsed as f32 / self.duration_ms as f32 * (self.table.len() as f32 - 1.0);
        let i = (p - 0.5).round_ties_even().max(0.0) as usize;
        let j = (p + 0.5).round_ties_even().max(0.0) as usize;
        self.frame = self.table[i.min(self.table.len() - 1)];
        self.next = self.table[j.min(self.table.len() - 1)];
        self.frac = p - i as f32;
    }
}

/// The right arm's gear-change gestures, by the steering frame 0…9 (`0x5CD3AC`…`0x5CD4C0`, the
/// ten `{count, table}` pairs 0x438CD1…0x438D42 builds): from the wheel out to the lever's frames
/// 10…15 and back.
pub(super) const GEAR_GESTURES: [&[u32]; 10] = [
    &[0, 10, 11, 12, 14, 15, 14, 12, 11, 10, 0],
    &[1, 10, 11, 12, 14, 15, 14, 12, 11, 10, 1],
    &[2, 11, 12, 14, 15, 14, 12, 11, 2],
    &[3, 11, 12, 14, 15, 14, 12, 11, 3],
    &[4, 12, 14, 15, 14, 12, 4],
    &[5, 12, 14, 15, 14, 12, 5],
    &[6, 13, 15, 13, 6],
    &[7, 13, 15, 13, 7],
    &[8, 13, 15, 13, 8],
    &[9, 13, 15, 13, 9],
];

/// The handbrake's, three frames each (`0x5CD2BC` pulling, `0x5CD334` letting go, 12 bytes apart,
/// 0x438EB0…0x438F85): to the lever at 16…18 and the held frame 19.
pub(super) const HANDBRAKE_ON: [[u32; 3]; 10] =
    [[0, 16, 19], [1, 16, 19], [2, 16, 19], [3, 16, 19], [4, 16, 19], [5, 17, 19], [6, 17, 19], [7, 18, 19], [8, 18, 19], [9, 18, 19]];
pub(super) const HANDBRAKE_OFF: [[u32; 3]; 10] =
    [[19, 16, 0], [19, 16, 1], [19, 16, 2], [19, 16, 3], [19, 16, 4], [19, 17, 5], [19, 17, 6], [19, 18, 7], [19, 18, 8], [19, 18, 9]];

/// The body's turn to look back, by the body frame 0…4 (`0x5CD568` into reverse, `0x5CD590` out
/// of it, `{count, table}` pairs, 0x43914F / 0x4391A3): toward frame 7 and back.
pub(super) const REVERSE_ON: [&[u32]; 5] = [&[0, 1, 2, 6, 7], &[1, 2, 6, 7], &[2, 6, 7], &[3, 6, 7], &[4, 6, 7]];
pub(super) const REVERSE_OFF: [&[u32]; 5] = [&[7, 6, 2, 1, 0], &[7, 6, 2, 1], &[7, 6, 2], &[7, 6, 3], &[7, 6, 4]];

/// What slot 9's driver half reads off the car each frame (0x438B2C…0x439352).
#[derive(Debug, Clone, Copy, Default)]
pub struct DriverInputs {
    /// `[0x6197C4]`, the car detail (`CARDETAIL`, 0x44EF80…0x44EFA7): under 1 the driver is posed,
    /// not animated
    pub detail: u8,
    /// channel 5's `.z`: `ch1.y + ch2.y` — the two front wheels' `+0xB4` steering values over 216,
    /// negated in a mirrored race (0x438461…0x4384EB) — clamped to ±4.5/23 (0x4385AF…0x4385EB)
    pub wheel_turns: f32,
    /// `car+0xD84`, the shift timer (ticks)
    pub shift_timer: u32,
    /// `car+0xB50 == 0` and neither pause flag (`[0x606A90]`, `[0x606A98]`) set (0x438CA0…0x438CC2)
    pub racing: bool,
    /// `car+0xD86`
    pub handbrake: bool,
    /// `car+0xD82 == 0`
    pub reverse: bool,
    /// `car+0xE04` / `+0xE08`, the filtered longitudinal and lateral accelerations
    pub long_acc: f32,
    pub lat_acc: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct DriverAnim {
    clock_ms: u64,
    players: [KeyPlayer; ANIMS],
    /// `R3DCar+0x564` / `+0x566`: the handbrake and reverse states the gestures were last started for
    handbrake: bool,
    reverse: bool,
}

/// One `{start, duration, from, to}` easer — the 16-byte record the original keeps per channel at
/// `object+0x64 + 16n` (`car-model.md`), with the clock kept as elapsed rather than a start tick.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelEase {
    from: f32,
    to: f32,
    elapsed: u32,
    duration: u32,
}

/// One float key out of a `.tpg` `[header]`, `default` when absent.
fn tpg_f32_or(tpg: &nfs_formats::ini::Ini, key: &str, default: f32) -> f32 {
    tpg.section("header").and_then(|h| h.get_f32_list(key)).and_then(|v| v.first().copied()).unwrap_or(default)
}

/// One float key out of a `.tpg` `[header]`, 0.0 when absent.
fn tpg_f32(tpg: &nfs_formats::ini::Ini, key: &str) -> f32 {
    tpg.section("header").and_then(|h| h.get_f32_list(key)).and_then(|v| v.first().copied()).unwrap_or(0.0)
}

#[derive(Clone)]
pub struct CarWheelModel {
    pub pivot: Vec3,
    pub is_front: bool,
    pub is_left: bool,
    /// The article's `Base+75` rotation channel. Every wheel-typed article of every shipped car is
    /// bound to **1…4** — the wheels that spin — or to **18…21**, the tread that takes the steer
    /// but not the spin, because 0x438669…0x438726 copy channels 1…4 into 18…21 with `.x` cleared
    /// (`cargo run -p nfs-formats --example rotchan`).
    pub channel: u8,
    /// the article's place in its LOD's sorted part list (see `CarModel::body_order`)
    pub order: u32,
    /// the LOD these parts are and the article's pass bytes
    pub info: PartInfo,
    pub parts: Vec<(MeshId, MaterialId, Mat4)>,
}

/// What the car draw needs of one article's `Base` record to place a part in a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartInfo {
    /// the LOD (1…7) the part's primitives belong to
    pub lod: u8,
    /// `Base+78`, the pass bits: `R3DCarLOD::draw` 0x43C830 draws the part in a call whose mask
    /// meets it, `Base+78 & mask & 0xF0` (0x43C8DF…0x43C8E9)
    pub mask: u8,
    /// `Base+81`: the part takes part in a call with the flag argument set (0x43C8C7)
    pub inside: bool,
    /// `Base+82`: the part stays out of a call without it (0x43C8D4) — the driver's hands
    pub inside_only: bool,
    /// `Base+72`, the translation channel, −1 for none (0x43C945)
    pub move_channel: i8,
}

impl PartInfo {
    /// Whether 0x43C830 draws this part in `pass` (0x43C8AC…0x43C8E9).
    pub fn in_pass(&self, pass: &CarPass) -> bool {
        self.lod == pass.lod && self.mask & pass.mask & 0xF0 != 0 && if pass.flag { self.inside } else { !self.inside_only }
    }
}

/// One `R3DCarLOD::draw` 0x43C830 call of the car draw **0x444E00**: the LOD object (`[R3DCar+0xB0]
/// [lod − 1]`), the mask its parts' `Base+78` must meet and the flag argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarPass {
    pub lod: u8,
    pub mask: u8,
    pub flag: bool,
}

/// The front end's two calls (0x444E86…0x444EBD): mask **0x50** from the LOD of the class
/// `R3DCar+0x2C0`, which the front end's branch of 0x439F90 zeroes every frame (0x439FF1), and mask
/// **0x20** from LOD 1 — no 0x80 call, so no driver.
pub const FRONT_END_PASSES: [CarPass; 2] = [CarPass { lod: 1, mask: 0x50, flag: false }, CarPass { lod: 1, mask: 0x20, flag: false }];

// **The car draw is `nfs5.dll`'s, not the file's.** The Verok patch's runtime patcher jumps the first
// five bytes of 0x444E00 into its own `R3DCar` draw (dll VA 0x100012C0 → 0x10001090, `RE/notes/nfs5-dll.md`
// §2.1) and rewrites the car detail to five tiers (0 … 4, the `Graphics.CarDetail` list 0x5B6D5C), so
// every table below is the dll's own (`.rdata` 0x10005094 … 0x100051A0 and `.data` 0x1000600C), not the
// exe's 0x5CDB1C … 0x5CDBA8 / 0x5CD5B8, which the shipped game never reads for the car.
/// dll `0x10005094[eff]`: the driver call's LOD object, effective class 0…3 only (0x100011E7)
const DRIVER_LOD: [u8; 4] = [0, 0, 1, 2];
/// dll `0x100050E0[7·CARDETAIL + class]`: the effective class the body and the driver are drawn from
/// (0x100011C7…0x100011D9); tiers 0 … 2 are the class itself, 3 and 4 draw finer LODs
const EFFECTIVE_CLASS: [[u8; 7]; 5] = [
    [0, 1, 2, 3, 4, 5, 6],
    [0, 1, 2, 3, 4, 5, 6],
    [0, 1, 2, 3, 4, 5, 6],
    [0, 1, 1, 2, 3, 4, 5],
    [0, 0, 0, 1, 1, 2, 3],
];
/// dll `0x1000516C[class]`: the wheels' LOD object (0x1000128D) — by the raw class, not the tier
const WHEEL_LOD: [u8; 7] = [0, 0, 0, 1, 1, 2, 4];
/// dll `0x10005188[class]`: a traffic car's wheels (0x10001268)
const TRAFFIC_WHEEL_LOD: [u8; 7] = [0, 0, 0, 0, 1, 2, 3];
/// dll `0x1000600C[CARDETAIL]` (written into the push operand 0x43A086, dll 0x100013C8): the class a
/// racer or cop starts from
const CLASS_BIAS: [i32; 5] = [5, 4, 3, 2, 1];
/// dll `0x100050AC[CARDETAIL]`: the LOD object of the driver view's shell and cabin pass (0x100010EE)
const CABIN_LOD: [u8; 5] = [1, 1, 1, 0, 0];
/// dll `0x100050C0[CARDETAIL]`: the hands' LOD object in the driver view (0x10001161), tiers 2 and up
const HANDS_LOD: [u8; 5] = [1, 1, 1, 0, 0];

/// The race's calls of the car draw (dll 0x10001090), for LOD class `class`: mask **0x80** (the
/// driver) from `DRIVER_LOD[eff]` while `eff < 4`, **0x50** (the body, its interior, the steering
/// wheel) from LOD `eff`, **0x20** (the wheels) from `WHEEL_LOD[class]`, or for traffic (`car+0x52C & 0x10`)
/// from `TRAFFIC_WHEEL_LOD[class]` — all without the flag, so a part with `Base+82` (the hands) is
/// left out. `eff` is `EFFECTIVE_CLASS[CARDETAIL][class]`.
pub fn race_passes(class: u8, detail: u8, traffic: bool) -> Vec<CarPass> {
    let c = (class as usize).min(6);
    let eff = EFFECTIVE_CLASS[(detail as usize).min(4)][c] as usize;
    let mut v = Vec::with_capacity(3);
    if eff < 4 {
        v.push(CarPass { lod: DRIVER_LOD[eff] + 1, mask: 0x80, flag: false });
    }
    v.push(CarPass { lod: eff as u8 + 1, mask: 0x50, flag: false });
    let w = if traffic { TRAFFIC_WHEEL_LOD[c] } else { WHEEL_LOD[c] };
    v.push(CarPass { lod: w + 1, mask: 0x20, flag: false });
    v
}

/// dll 0x100010DB…0x10001186: the camera's own car in views 0 and 1 (`[0x627F43] & 8`) — mask **0x10**
/// from LOD `CABIN_LOD[CARDETAIL]` with the flag `class > 1` (the stale class the pick left, 0x100010DB…0x100010E6; the file's 0x444F5A pushed 1 and LOD 2), **0x40** with the flag from LOD 1 (the
/// steering wheel), and, at `CARDETAIL` above 1 and with none of the look inputs 0x13 / 0x14 / 0x15
/// held (`[0x5E99B0]`, `[0x5E99B4]`, `[0x5E99AC]`), **0x80** with the flag from `HANDS_LOD[CARDETAIL]`
/// (the hands and a passenger; the file's 0x444F92 test was `CARDETAIL > 0`).
pub fn in_car_passes(class: u8, detail: u8, looking: bool) -> Vec<CarPass> {
    let d = (detail as usize).min(4);
    let mut v = vec![CarPass { lod: CABIN_LOD[d] + 1, mask: 0x10, flag: class > 1 }, CarPass { lod: 1, mask: 0x40, flag: true }];
    if detail > 1 && !looking {
        v.push(CarPass { lod: HANDS_LOD[d] + 1, mask: 0x80, flag: true });
    }
    v
}

/// What the LOD pick needs of a car beside its model: `car+0x52C`'s traffic (0x10) and opponent
/// (8) bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CarKind {
    /// the player, a cop
    Racer,
    Opponent,
    Traffic,
}

/// A car's own LOD state, which the original keeps on its `R3DCar`: the class `+0x2C0` and the
/// translation channels' `.y` (`+0x6D0 + 12m`, 0x438210). The traffic and the cops share one
/// model per type, so this lives with each car.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CarLodState {
    pub class: u8,
    pub travel: [f32; 4],
}

/// The screen the LOD pick projects on (0x539700 through the view `[[0x628130]+0x10]+0xC`) and its
/// width in pixels `[0x657A48]`.
#[derive(Debug, Clone, Copy)]
pub struct LodView {
    pub view_projection: Mat4,
    pub width: f32,
    pub height: f32,
    /// the render width `[0x657A48]` the reference 640 is taken over (0x43A2D7): the window's
    /// width, whichever view's viewport `width` / `height` are
    pub ref_width: f32,
    /// the 2-D screen's virtual width `nfs5.dll` keeps for the mode (dll 0x10006134, computed at
    /// 0x10003840): 640 up to 4:3, `480 · W / H` wider — what replaces the literal 640 at 0x43A2DD
    /// (the operand is rewritten at dll 0x10003D94)
    pub virtual_width: f32,
}

/// `nfs5.dll`'s virtual 2-D screen `(width, height)` for a mode `w × h` pixels (dll 0x10003840…
/// 0x100038BE, run on every SetMode): with `r = w / h`, `r` above 4/3 (the double at dll 0x10005600)
/// → `(480 · r, 480)`; `r` 4/3 or less → `(640, 640 / r)`. Cells dll 0x10006134 (width) /
/// 0x10006130 (height), read by the LOD pick (0x43A2DF) and the quad draw 0x451710
/// (0x451824 / 0x451834).
pub fn virtual_screen(w: f32, h: f32) -> (f32, f32) {
    let ratio = w / h;
    if 4.0_f32 / 3.0 < ratio { (480.0 * ratio, 480.0) } else { (640.0, 640.0 / ratio) }
}

/// The virtual screen's width (dll 0x10006134): 640 when `w / h` is 4/3 or less, `480 · w / h`
/// above it.
pub fn virtual_width(w: f32, h: f32) -> f32 {
    virtual_screen(w, h).0
}

/// The car's collision body points `car+0x8C` (`BndP`, `RE/notes/car-model.md` §12): the records
/// the body style admits, their points **as the loader 0x442080 stores them** — the model's own
/// axes, not turned into the body frame the model is drawn in (0x49AE30 uses them unturned with
/// the physics rows) — then 0x49ABC0's sort by height and quadrant permutation
/// ([`nfs_physics::vehicle::body_points_setup`]), so the first four are the ground probes FL, FR,
/// RL, RR. (history: `game-history.md` §10.1)
///
/// `y` here keeps the model's `y = 0` on the contact plane; the loader then subtracts `car+0x3AC`
/// — half the model box's height, set by 0x445090 before it (0x4420A3 → 0x442775) — which
/// [`car_body`] does, so a racer built from that carries the original's origin, half the model's
/// height above the ground (`Vehicle::set_car_body`).
pub fn car_body_points(car: &CarAssets) -> Vec<Vec3> {
    let Ok(recs) = car.crp.bound_points(&car.crp_data) else { return Vec::new() };
    // The loader's filter is `carinfo[0x56C + 4·slot] == value` (0x4420B9), i.e. the body style's
    // own array — not "slot 0 only", which is what this used to be and which left every
    // style-selected record out (the spoilers, the GT1 wing).
    let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
    let mut pts: Vec<Vec3> = recs
        .iter()
        .filter(|r| style.article_enabled(r.style_slot as u8, r.style_value as u8))
        .flat_map(|r| r.points.iter().map(|p| Vec3::from(*p)))
        .collect();
    nfs_physics::vehicle::body_points_setup(&mut pts);
    pts
}

/// The render model's local box that **0x445090** (`RCarPages::finishLoad`, tail of 0x4425B0,
/// called from `RCar::load` 0x442775) halves into `car+0x3A8 / +0x3AC / +0x3B0` — before the `BndP`
/// loader 0x442080 runs (0x4420A3 → 0x4425B0), so its `y` is what the loader subtracts.
///
/// 0x445090 asks the LOD object `[this+0xB0][4]` (`[this+0xB0][0]` for traffic, `car+0x52C &
/// 0x10`) for its box through vtable `+0x10`, **0x43DE90**; `d = max − min` (0x532330),
/// `car+0x3A8…+0x3B0 = 0.5·d` (0x532360), `× 0.4` for `.TINYCAR` (0x445140…0x44517C, `[0x5B37C8]`).
/// The seven LOD objects are built by 0x4425B0 through slot 17 (0x43ADB0: `+8 = i`, `i = 1…7`,
/// the count from slot 16 0x43ADA0), so `[4]` is **LOD 5** and `[0]` LOD 1. 0x43DE90 starts `min`
/// and `max` at **zero** (0x43DE99…0x43DEDA — the box always holds the model origin) and folds in,
/// for every article the LOD lists — 0x43C1B0: the style admits it (`carinfo[0x56C + 4·Base+68] ==
/// Base+69`, 0x43C2A1) and one of its `Base` LOD records carries this LOD (`word Base+0x6C + 12k`,
/// 0x43C2BC) — every vertex of its `vt#lod` part (count = the part entry's `b`, 16 bytes each),
/// through its `tr#lod` matrix when it has one (0x59F220 `'tr' | lod`, 0x53A330 `v·M`).
///
/// CONFIRMED for the code path; the port's reading of the `vt` part is the one its mesh loader
/// uses. A racer's box is LOD 5 ([`car_body`]); traffic's LOD 1 (`crates/porsche`'s traffic set-up,
/// `nfs_game::traffic::TrafficCar::draw_origin`).
pub fn car_model_box(car: &CarAssets, lod: u16) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::ZERO, Vec3::ZERO);
    let Ok(arts) = car.crp.articles(&car.crp_data) else { return (lo, hi) };
    let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
    for a in &arts {
        let Some(base) = a
            .level(b"Base")
            .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok().map(|d| (e, d)))
            .and_then(|(e, d)| nfs_formats::crp::Base::parse(d, e.data_offset()).ok())
        else {
            continue;
        };
        if !style.article_enabled(base.tag[0], base.tag[1]) || !base.lods.iter().any(|r| r.lod == lod) {
            continue;
        }
        let Some(vt) = a.find(b"vt", lod) else { continue };
        let Ok(bytes) = car.crp.entry_data(&car.crp_data, vt) else { continue };
        let count = match vt {
            nfs_formats::crp::CrpEntry::Part { b, .. } => (*b as usize).min(bytes.len() / 16),
            _ => 0,
        };
        let tr = a.find(b"tr", lod).and_then(|e| car.crp.entry_data(&car.crp_data, e).ok()).filter(|b| b.len() >= 64).map(|b| {
            let f = |i: usize| bytes::read_f32(b, i * 4);
            // row-major, translation in row 3, `v' = v·M` — glam's column-major reading of the
            // same sixteen floats is that matrix transposed, which is `M` applied to a column
            Mat4::from_cols_array(&std::array::from_fn(f))
        });
        for k in 0..count {
            let f = |o: usize| nfs_formats::bytes::read_f32(bytes, k * 16 + o);
            let mut p = Vec3::new(f(0), f(4), f(8));
            if let Some(m) = tr {
                p = m.transform_point3(p);
            }
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    (lo, hi)
}

/// A racer's collision body as `RCar::load` leaves it, in the original's own frame: the half
/// extents `car+0x3A8…+0x3B0` of 0x445090 ([`car_model_box`], LOD 5, `× 0.4` for `.TINYCAR`) and
/// the `BndP` points of 0x442080 — the style's records, `× 0.4` for `.TINYCAR` (0x442140…), then
/// **`y −= car+0x3AC`** (0x4421BE, `fsub [ecx+0x3AC]`) — through 0x49ABC0's sort and quadrant deal
/// ([`nfs_physics::vehicle::body_points_setup`]; a constant shift leaves both unchanged).
///
/// So the body origin `car+0x330` sits `car+0x3AC` — half the model box's height, the box holding
/// the model's `y = 0` ground plane — above the plane the wheel quad stands on, and the renderer
/// draws the model that far below it (`R3DCar` slot 5 0x43A410, `Vehicle::draw_origin`). Racers
/// only: nothing re-cuts a racer's box afterwards (0x4A1280's `(max − min)/2` of the body points,
/// 0x4A1472…0x4A14A6, runs from 0x4107B6 only for `car+0x52C & 0x80`, a smackable).
pub fn car_body(car: &CarAssets, tiny: bool) -> (Vec<Vec3>, Vec3) {
    let (lo, hi) = car_model_box(car, 5);
    let scale = if tiny { 0.4 } else { 1.0 };
    let half = (hi - lo) * 0.5 * scale;
    let mut pts = car_body_points(car);
    for p in &mut pts {
        *p *= scale;
        p.y -= half.y;
    }
    (pts, half)
}

/// A **traffic** car's collision body as `RCar::load` leaves it: the same `BndP` points as
/// [`car_body`], but lowered by `car+0x3AC` of **its own** box — 0x445090 takes a traffic car's box
/// (`car+0x52C & 0x10`, 0x4450CC) from `[this+0xB0][0]`, LOD **1** ([`car_model_box`]`(.., 1)`), and
/// the loader's `y −= car+0x3AC` (0x4421BE) reads that, not LOD 5's (the traffic models have no LOD 5
/// at all, so [`car_body`] lowered them by 0). `half` is LOD 1's half box, never `× 0.4`
/// (`.TINYCAR` is off for traffic).
pub fn traffic_body(car: &CarAssets) -> (Vec<Vec3>, Vec3) {
    let (lo, hi) = car_model_box(car, 1);
    let half = (hi - lo) * 0.5;
    let mut pts = car_body_points(car);
    for p in &mut pts {
        p.y -= half.y;
    }
    (pts, half)
}

/// What a car's reflection passes draw with, set per race.
#[derive(Debug, Clone, Copy, Default)]
pub struct CarEnv {
    /// `R3DSpecularTextureLayer`'s three textures, `[0x6115CC]+0xDC…+0xE4` — [`car_specular_maps`]:
    /// `SPC1…3` in a race, `FEDU` / `FE  ` / `FESH` in the front end (0x441AF1). (history: `game-history.md` §10.2)
    pub specular: [Option<nfs_renderer::TextureId>; 3],
    /// `[0x6197F8]` has bit 2 — `Graphics.CarChrome` ≥ Medium (0x44EF3A / 0x44EF5D) — so the
    /// `CarExtEnv` / `CarExtEnvNS` passes of mask 2 are built (`lighting.md` §5.3)
    pub chrome_passes: bool,
    /// Give each material its `Rs` block's depth bias (Thrash 0x18, `mt+276`) — set for the race,
    /// whose view's depth scale is read (the view's near and the camera object's far 700,
    /// `thrash.md` §6); the front end's 3D view's planes are not read, so its cars keep 0 there.
    pub rs_bias: bool,
}

/// The load options of [`CarModel::upload_with_env`]; [`Default`] is the plain front-end upload
/// of [`CarModel::upload`].
#[derive(Debug, Clone, Copy, Default)]
pub struct CarUpload {
    /// The specular layer's three textures and whether the option word builds the `CarExtEnv`
    /// passes ([`CarEnv`]).
    pub env: CarEnv,
    /// The track is a night one (the model's style reads it).
    pub night: bool,
    /// In a race, the car's `.CABRIODOWN` option for slot 19's roof line
    /// ([`nfs_assets::car::CarStyle::apply_race_roof`]); `None` in the front end, whose branch
    /// (0x43B003) does not take it.
    pub race_roof: Option<u32>,
    /// The `.tpg` entry test's `testdrive` context — `Some(Factory Driver mission)` on `skidpad`,
    /// `None` elsewhere ([`nfs_assets::car::CarStyle::skidpad`]).
    pub skidpad: Option<bool>,
}

/// 0x43C1B0's part list of each LOD (`list`: the 12-byte records `{article, lod record, Base+70}`
/// in article order, 0x43C2D5…0x43C2F3 — one per admitted article, the first `Base` LOD record for
/// the LOD), with each LOD's part count and its translation-channel gate.
#[derive(Default)]
struct LodLists {
    list: [Vec<(usize, i8)>; 7],
    parts: [u32; 7],
    moves: [[bool; 4]; 7],
}

impl LodLists {
    /// Article `ai`'s `Base` LOD records into the lists.
    fn admit(&mut self, car: &CarAssets, style: &nfs_assets::car::CarStyle, ai: usize, a: &nfs_formats::crp::Article) {
        // 0x43C1B0's part list of each LOD: the articles the style admits with a `Base` LOD
        // record for it (0x43C2A1…0x43C2C9) — glares included — and, per part, its
        // translation channel `Base+72` into the LOD's gate `+0x14 + 4m` (0x43C66D…0x43C67C)
        if let Some(d) = a.level(b"Base").and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
            && d.len() > 0x48
            && style.article_enabled(d[0x44], d[0x45])
        {
            let u32_at = |o: usize| bytes::get_u32(d, o).map_or(0, |v| v as usize);
            for k in u32_at(8)..u32_at(12) {
                let o = 0x64 + 12 * k;
                let Some(l) = bytes::get_u16(d, o + 8) else { break };
                let l = l as usize;
                if (1..=7).contains(&l) {
                    if !self.list[l - 1].iter().any(|&(i, _)| i == ai) {
                        self.list[l - 1].push((ai, d[0x46] as i8));
                    }
                    self.parts[l - 1] += 1;
                    if let Ok(m) = usize::try_from(d[0x48] as i8)
                        && m < 4
                    {
                        self.moves[l - 1][m] = true;
                    }
                }
            }
        }
    }
}

/// The parts [`CarModel::upload_with_env`] collects, in article order: the body's (with each
/// one's rotation channel, info and article) and the wheels' (with each one's article).
#[derive(Default)]
struct CarParts {
    body: Vec<(MeshId, MaterialId, Mat4)>,
    body_channel: Vec<u8>,
    body_info: Vec<PartInfo>,
    /// each `body` entry's article index, for its place in the LOD list after the loop
    body_art: Vec<usize>,
    wheels: Vec<CarWheelModel>,
    wheel_art: Vec<usize>,
}

impl CarParts {
    /// The parts' draw order from 0x43C1B0's sorted lists, the unlisted parts dropped: returns the
    /// body's order and how many were dropped.
    fn order(&mut self, lod_list: &mut [Vec<(usize, i8)>; 7]) -> (Vec<u32>, usize) {
        // 0x43C1B0's sort of each LOD's list (0x43C31B…0x43C38A): the VC6 `std::sort` — introsort
        // 0x43E710, then 0x43E910 / 0x43EA20 — over the records, `less` = the signed byte at `+8`
        // (`cmp al, cl; jge` in the median 0x43E77D, `cmp [esi+8], cl; jge` in the partition
        // 0x43E7B1, `cmp bl, cl; jge` 0x43E950 / `cmp dl, bl; jge` 0x43E9D3 / 0x43EA49 in the
        // inserts, `cmp dl, [edi-4]; jge` / `cmp dl, bl; jge` in the heap 0x43EA90 / 0x43EBE0).
        // Above 16 parts it is not stable, so equal keys draw in the order it leaves them.
        let mut order_of: std::collections::HashMap<(u8, usize), u32> = std::collections::HashMap::new();
        for (l, list) in lod_list.iter_mut().enumerate() {
            nfs_formats::stl_sort::vc6_sort(list, |a, b| a.1 < b.1);
            for (rank, &(ai, _)) in list.iter().enumerate() {
                order_of.insert((l as u8 + 1, ai), rank as u32);
            }
        }
        // A part with primitives at a LOD but not in that LOD's list (no `Base` record for it, or
        // an article the style does not admit) is not drawn by the original at all: the draw
        // 0x43C830 walks the LOD's list 0x43C1B0 built, and nothing else. Dropped here, as the
        // original does. Census 2026-09-30 (`unlisted` in the log, every one of the 45 shipped
        // `CarModel/*.crp` models loaded): 0 parts — the case never arises on shipped data.
        let listed = |lod: u8, ai: usize| order_of.contains_key(&(lod, ai));
        let keep_body: Vec<bool> = self.body_info.iter().zip(&self.body_art).map(|(info, &ai)| listed(info.lod, ai)).collect();
        let keep_wheel: Vec<bool> = self.wheels.iter().zip(&self.wheel_art).map(|(w, &ai)| listed(w.info.lod, ai)).collect();
        let unlisted = keep_body.iter().chain(&keep_wheel).filter(|k| !**k).count();
        fn keep_only<T>(v: &mut Vec<T>, keep: &[bool]) {
            let mut k = keep.iter();
            v.retain(|_| *k.next().unwrap_or(&true));
        }
        keep_only(&mut self.body, &keep_body);
        keep_only(&mut self.body_channel, &keep_body);
        keep_only(&mut self.body_info, &keep_body);
        keep_only(&mut self.body_art, &keep_body);
        keep_only(&mut self.wheels, &keep_wheel);
        keep_only(&mut self.wheel_art, &keep_wheel);
        let order = |lod: u8, ai: usize| order_of.get(&(lod, ai)).copied().unwrap_or(u32::MAX);
        let body_order: Vec<u32> = self.body_info.iter().zip(&self.body_art).map(|(info, &ai)| order(info.lod, ai)).collect();
        for (w, &ai) in self.wheels.iter_mut().zip(&self.wheel_art) {
            w.order = order(w.info.lod, ai);
        }
        (body_order, unlisted)
    }
}

/// Whether 0x43D800's effect pass keeps article `a` at `lod`: the style admits it and its `Base`
/// LOD record for the LOD has the "has effects" byte `+1` set (0x43D863…0x43D872).
fn effect_part(car: &CarAssets, style: &nfs_assets::car::CarStyle, a: &nfs_formats::crp::Article, lod: u8) -> bool {
    a
        .level(b"Base")
        .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
        .is_some_and(|d| {
            let u32_at = |o: usize| bytes::get_u32(d, o).map_or(0, |v| v as usize);
            d.len() > 0x45
                && style.article_enabled(d[0x44], d[0x45])
                && (u32_at(8)..u32_at(12))
                    .map(|k| 0x64 + 12 * k)
                    .find(|&o| bytes::get_u16(d, o + 8) == Some(u16::from(lod)))
                    .is_some_and(|o| d.get(o + 1).is_some_and(|&b| b != 0))
        })
}

/// A glare article's lamps (`particles.md` §4): its `ef` record of type 5 as a [`CarLight`] and,
/// when `L+0x48 & 8`, the mirrored copy (0x43DAC5…0x43DB32); `None` for an article without one.
fn glare_lamps(car: &CarAssets, a: &nfs_formats::crp::Article, lod: u8) -> Option<Vec<CarLight>> {
    if let Some(ef) = a.parts.iter().find(|p| p.is_part(b"ef"))
        && let Ok(b) = car.crp.entry_data(&car.crp_data, ef)
        && b.len() >= 88
        && bytes::read_u32(b, 0) == 5
    {
        let f = |o: usize| bytes::read_f32(b, 8 + o);
        let v = |o: usize| Vec3::new(f(o), f(o + 4), f(o + 8));
        let u = |o: usize| bytes::read_u32(b, 8 + o);
        let slot = a
            .level(b"Base")
            .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
            .and_then(|d| d.get(0x50).copied())
            .unwrap_or(9);
        let to_body = |p: Vec3| MODEL_TO_BODY.transform_point3(p);
        let dir_body = |d: Vec3| MODEL_TO_BODY.transform_vector3(d);
        // the article's frame for this LOD: 0x59F220 finds the part `"tr"` + `[this+8]` — the
        // LOD itself, no fallback (0x59F238, `or eax, 0x74720000`) — and the pass multiplies
        // it in front of the car's matrix (0x43D915…0x43D929, `0x539B00`: `arg · this`),
        // then, while `Base+0x4B` names a channel, the channel's `Rx·Ry·Rz` in front of
        // that (0x43D943…0x43D9FE). Shipped: the pop-up headlights (channel 17), the cops'
        // sirens (22), a wheel glint (1), the lids' and doors' glints (6, 8, 9) — every
        // glare `tr` has an identity rotation, only a translation.
        let tr = a
            .parts
            .iter()
            .find(|t| t.is_part(b"tr") && t.part_index() == u16::from(lod))
            .and_then(|t| car.crp.entry_data(&car.crp_data, t).ok())
            .filter(|m| m.len() >= 64)
            .map(|m| Mat4::from_cols_array(&std::array::from_fn(|i| bytes::read_f32(m, 4 * i))));
        let base_channel = a
            .level(b"Base")
            .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
            .and_then(|d| d.get(0x4B).copied())
            .unwrap_or(0);
        let channel = if tr.is_some() { base_channel } else { 0 };
        let lamp = CarLight {
            position: to_body(v(0)),
            axis1: dir_body(v(0x10)),
            axis2: dir_body(v(0x20)),
            facing: dir_body(v(0x30)),
            colour: [u(0x40), u(0x44)],
            kind: b[8 + 0x4C],
            slot,
            night: b[8 + 0x4F] != 0,
            frame: tr.unwrap_or(Mat4::IDENTITY),
            channel,
        };
        // the mirrored copy only when `L+0x48 & 8` (0x43DAB6…0x43DABA): 804 of the shipped
        // records carry 8 there and 126 carry 0, and those have no copy across x (history:
        // `game-history.md` §10.10).
        let mut lamps = vec![lamp];
        if b[8 + 0x48] & 8 != 0 {
            let flip = |d: Vec3| Vec3::new(-d.x, d.y, d.z);
            lamps.push(CarLight {
                position: flip(lamp.position),
                axis1: -lamp.axis1,
                axis2: flip(lamp.axis2),
                facing: flip(lamp.facing),
                slot: b[8 + 0x4E],
                // 0x43DB5B…0x43DC0D: the `tr` copied with its translation zeroed, transposed
                // (0x53A650), and `T(−t.x, t.y, t.z) · that` (0x539B80, 0x539B00) — in
                // column form `Rᵀ · T`; the channel's value is `L+0x4D`'s (0x43DC36), the
                // gate still the primary's `Base+0x4B` (0x43DC29)
                frame: tr.map_or(Mat4::IDENTITY, |m| {
                    let t = m.w_axis.truncate();
                    Mat4::from_mat3(glam::Mat3::from_mat4(m).transpose()) * Mat4::from_translation(Vec3::new(-t.x, t.y, t.z))
                }),
                channel: if tr.is_some() && base_channel != 0 { b[8 + 0x4D] } else { 0 },
                ..lamp
            });
        }
        return Some(lamps);
    }
    None
}

/// The crush vectors and zones laid out as the meshes are, at LOD `l`: the damage geometry, and the
/// geometry with each vertex's position replaced by its zone (the `dz` lists index the whole
/// `vt#lod`, a primitive's window starting at `vt_first`).
#[allow(clippy::type_complexity)]
fn crush_meshes(
    car: &CarAssets,
    a: &nfs_formats::crp::Article,
    g: &nfs_formats::crp::ArticleGeometry,
    dg: &nfs_formats::crp::ArticleGeometry,
    l: u8,
) -> Option<(Vec<nfs_assets::mesh::MaterialMesh>, Vec<nfs_assets::mesh::MaterialMesh>)> {
        let dz = car.crp.damage_zones(&car.crp_data, a, l).ok()??;
        let n = dz.zones.iter().flat_map(|z| z.1.iter()).map(|&i| i as usize + 1).max().unwrap_or(0);
        let mut zone_of = vec![255u8; n];
        for (z, list) in &dz.zones {
            for &i in list {
                zone_of[i as usize] = u8::try_from(*z).unwrap_or(255);
            }
        }
        let mut gz = g.clone();
        for p in gz.primitives.iter_mut().filter(|p| p.lod == l) {
            let first = p.vt_first as usize;
            for (i, q) in p.positions.iter_mut().enumerate() {
                *q = [zone_of.get(first + i).copied().unwrap_or(255) as f32, 0.0, 0.0];
            }
        }
        let mut sk = 0;
        Some((article_meshes(dg, l, &mut sk), article_meshes(&gz, l, &mut sk)))
}

/// A race car's pixel pages are read only for the pages a damage patch lands on
/// ([`CarModel::update_damage`], [`CarModel::instantiate`]); `blit_page` is the front-end rooms' —
/// so in a race the other pages' pixels (on the GPU already) are not kept, in the model or in every
/// instance cloned from it.
fn race_damage_pages(pages: Vec<nfs_assets::car::TexturePage>, damage_patches: &[nfs_assets::car::DamagePatch], race: bool) -> Vec<nfs_assets::car::TexturePage> {
    if race {
        let touched: std::collections::HashSet<usize> = damage_patches.iter().map(|p| p.page).collect();
        pages.into_iter().enumerate().map(|(i, mut p)| {
            if !touched.contains(&i) {
                p.rgba = Vec::new();
            }
            p
        }).collect()
    } else {
        pages
    }
}

pub const MODEL_TO_BODY: Mat4 = Mat4::from_cols(
    glam::Vec4::new(-1.0, 0.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 0.0, -1.0, 0.0),
    glam::Vec4::new(0.0, 0.0, 0.0, 1.0),
);

impl CarModel {
    pub fn upload(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: impl Into<nfs_assets::car::Paint>) -> anyhow::Result<Self> {
        Self::upload_with_env(r, car, paths, lod, paint, CarUpload::default())
    }

    /// The model's `[styleN]` with what `R3DCar`'s load writes over it (0x437C09, slot 19's race /
    /// front-end branches 0x43AEA5 / 0x43B003, the overlay 0x43B0B7, `.TINYCAR` 0x43B0E5) and the
    /// `.tpg` entry test's context.
    fn model_style(car: &CarAssets, night: bool, race_roof: Option<u32>, skidpad: Option<bool>) -> nfs_assets::car::CarStyle {
        // `[styleN]` is **1-based** and `CarAssets::style_index` is the raw 0-based `nfs5.car
        // +0x1C0`, which is what 0x437B33's `inc esi` turns into the section name.
        let mut style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
        // …and after the section `applyStyle` writes slot 10 (`car+0x594`) from the race number:
        // 1 when car-info `+0x510` is 0 or more, else 0 (0x437C09…0x437C2B; the front end's
        // 0x4D1800 does the same from the car's `DecalNum`). A car without page text is the
        // front end's or one whose number was not read, and −1 is what both hold.
        style.geometry.insert(10, u32::from(car.page_text.as_ref().is_some_and(|t| t.race_number >= 0)));
        // Slot 19 overlays the driver's variants onto the same array at model load, in its race
        // branch only (0x43AEA5…0x43AF6B; the front end's, 0x43B003, jumps past them), keyed on
        // the car's `.DRIVER` pick (`CarAssets::driver`).
        //
        // The other things that block writes (`car-model.md`): slot 41 is recomputed from the roof option
        // in a race (0x43AE7A), and **slot 42 (`SpoilerDown` / `SpoilerUp`) is zeroed by 0x43AEB2
        // (`mov [esi+0x614], ebx`, ebx = 0), so neither half of the pop-up spoiler is drawn in a race**
        // (the wing itself is the `SpoilerW` morph, animation 2, which the port runs: `update_animation`).
        // (history: `game-history.md` §10.3)
        match race_roof {
            Some(v) => {
                style.apply_race_roof(v);
                style.geometry.insert(42, 0); // 0x43AEB2
                style.apply_driver_detail(car.driver.driver, car.driver.no_driver, car.driver.factory_driver);
            }
            // slot 19's front-end branch, 0x43B003: slot 41 and slot 6 to 0
            None => {
                style.geometry.insert(41, 0);
                style.geometry.insert(6, 0);
            }
        }
        // and both branches end in the overlay, 0x43B0B7…0x43B0DC, and in `slot58 =
        // carinfo+0x518`, the `.TINYCAR` flag (0x43B0E5)
        style.apply_overlay(&car.style_overlay);
        style.geometry.insert(58, u32::from(car.tiny));
        // the entry test's context (0x439C60): the front-end branch is the one without a race roof
        style.front_end = race_roof.is_none();
        style.night = night;
        style.skidpad = skidpad;
        tracing::info!(style = style.name, "car style");
        style
    }

    /// `opts`: the environment, the night flag, the race roof and the skid-pad context
    /// ([`CarUpload`]).
    pub fn upload_with_env(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: impl Into<nfs_assets::car::Paint>, opts: CarUpload) -> anyhow::Result<Self> {
        let CarUpload { env, night, race_roof, skidpad } = opts;
        let style = Self::model_style(car, night, race_roof, skidpad);
        let paint: nfs_assets::car::Paint = paint.into();
        // `R3DCar+0xAA`, the exterior's HSV value `max(r, g, b)` (0x443CA0 from 0x443E18), which
        // picks the damage pack below (0x441FFD, 0x442080)
        let light_paint = paint.exterior.iter().copied().max().unwrap_or(0) > 0x50;
        let pages = car.build_pages(paths, paint, &style)?;
        let page_tex: Vec<(nfs_renderer::TextureId, bool)> = pages
            .iter()
            .map(|p| (r.upload_texture_mips(p.width, p.height, std::slice::from_ref(&p.rgba)), p.is_window || p.is_alpha))
            .collect();
        // the damage decals, `L` for a paint brighter than 0x50, `D` otherwise
        let damage_patches = nfs_assets::car::damage_patches(paths, &car.model, light_paint).unwrap_or_default();
        let page_textures: Vec<nfs_renderer::TextureId> = page_tex.iter().map(|p| p.0).collect();
        let materials = car.crp.materials(&car.crp_data)?;
        let mut mat_ids: HashMap<u16, MaterialId> = HashMap::new();
        let mut material_light: MaterialMap<nfs_formats::crp::LayerLight> = MaterialMap::default();
        let mut shadowed = MaterialSet::default();
        let mut material_for = |r: &mut Renderer, id: u16| {
            if let Some(m) = mat_ids.get(&id) {
                return *m;
            }
            let m = materials.get(id as usize);
            let page = m.map(|m| m.page_index() as usize).and_then(|p| page_tex.get(p));
            let blend = m.map(|m| matches!(m.render_method.as_str(), "CarWindow" | "CarWheel")).unwrap_or(false) || page.map(|p| p.1).unwrap_or(false);
            // the specular and sky-map passes belong to `CarExtEnv` and `CarExtEnvNS` (passes of
            // option mask 2, built at CarChrome ≥ Medium) and to `CarWindow` (mask 0, always built)
            // — `rmthpasses`, `lighting.md` §5.3. (history: `game-history.md` §10.4)
            let reflective = m.is_some_and(|m| match m.render_method.as_str() {
                "CarExtEnv" | "CarExtEnvNS" => env.chrome_passes,
                "CarWindow" => true,
                _ => false,
            });
            // `R3DSpecularTextureLayer` and `R3DSkyMapTextureLayer` bucket the material's own
            // reflection strength to pick which of their three textures they take (0x44553F,
            // 0x445B33; `lighting.md` §3.1, §3.2 — the four strength dwords at `mt+284…+296` are equal
            // in every shipped material, so the one `reflection` stands for each layer's own). The
            // passes' combine is their `Rs` block `{1, 2, 0, 1, 0, −1}` (`rmthpasses`): alpha-blended
            // over the pixel by the texel's own alpha — `env_mix` is only the switch
            let variant = nfs_formats::crp::env_variant(m.map(|m| m.reflection).unwrap_or(0.0));
            // the specular texture is the car's own `SPC{bucket+1}` (0x441AB2), not a section map
            let spec = if reflective { env.specular[variant] } else { None };
            // the cull mode is the material's own `Rs` block, Thrash state 2 at `mt+272`: 1 the
            // view's own (the back faces, `thrash.md` §6d.2), 0 none — the port had culled every opaque one and
            // none of the blended, which with depth writes on let a wheel well's outer faces cover
            // the wheel
            let cull = m.and_then(|m| m.rs).map_or(!blend, |rs| rs[3] == 1);
            // A front-end room is drawn by the car's own class (`nfs_assets::car::CarAssets::
            // front_end_room`), so its `Rs` block is what `LayerState` hands the driver: Thrash
            // 0x0A = 2 blends every one of its materials, and 0x38 picks the pair — 0 the plain
            // alpha pair, **1 `SRCALPHA` / `ONE`** for the light pools and light cones on the 4444
            // pages (`Lightcast*`, `SpotLight`, `GarageSpot`'s `ligh` / `spot`), which add their light
            // to what is under them (`RE/notes/frontend-3d.md` §9). The cars keep the port's rule by
            // method and page: every shipped car block is `{1, 2, 0, …}`, the plain pair.
            let (blend, blend_pair) = match m.and_then(|m| m.rs) {
                Some(rs) if car.front_end_room => (rs[1] == 2, rs[2].clamp(0, 3) as u8),
                _ => (blend, 0),
            };
            // Thrash 0x18 from the same block, `rs[4]` (`LayerState` 0x53F780 → 0x540660 hands the
            // driver states 6, 0x0A, 0x38, 2 and 0x18, the last through 0x540500): over the shipped
            // cars −10, −3, −2, −1, 0, 1, 3 and 5 (`nfs-formats/examples/rsblock.rs`) — `b · 2^-15`
            // on the game's screen z `far/(far − near) · (1 − near / Z)` (0x540CE0, `RE/notes/
            // thrash.md` §6d.3): ≈ `b · 2^-15 · Z² / near` of view depth, 0.05 mm a unit at 0.5 m in
            // the driver view (near 0.15), 0.5 m at 50 m. (history: `game-history.md` §10.5)
            let depth_bias = if env.rs_bias { m.and_then(|m| m.rs).map_or(0, |rs| rs[4]) } else { 0 };
            let mid = r.create_material(MaterialDesc { texture: page.map(|p| p.0), alpha_blend: blend, cull_back: cull, depth_test: true, env_texture: spec, env_mix: 1.0, env_variant: variant, sky_map: reflective, blend_pair, clamp: m.map_or([false; 2], |m| m.address.map(|a| a == 0)), lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: true, view_cull: false, depth_bias, false_sort: false });
            if let Some(l) = m.and_then(|m| m.light) {
                material_light.insert(mid, l);
            }
            // the methods with a pass of layer class **0x8102**, `R3DShadowMapTextureLayer` (the
            // factory's 0x449FB4 arm → vtable 0x5B3964) that the option word builds (`rmthpasses`
            // over every shipped car and room `.crp`, `lighting.md` §5.3): `CarExt` and `CarIntShd`
            // (pass 1, mask 0), `CarWindow` (pass 3, mask 0) and `CarExtEnv` (pass 3, mask 2 — built
            // at CarChrome ≥ Medium). `CarExtEnvNS` ("no shadow") lists it only in its dead mask-4
            // passes, `CarInt` and `CarWheel` not at all. (history: `game-history.md` §10.6)
            if m.is_some_and(|m| match m.render_method.as_str() {
                "CarExt" | "CarIntShd" | "CarWindow" => true,
                "CarExtEnv" => env.chrome_passes,
                _ => false,
            }) {
                shadowed.insert(mid);
            }
            mat_ids.insert(id, mid);
            mid
        };
        let arts = car.crp.articles(&car.crp_data)?;
        let mut lists = CarParts::default();
        let mut skipped = 0;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut lights: Vec<CarLight> = Vec::new();
        let mut morph: [Vec<MorphPart>; ANIMS] = Default::default();
        let mut anim_length = [0u32; ANIMS];
        let mut wheel_radius: f32 = 0.3;
        let mut half_track: f32 = 0.7;
        let mut deform: Vec<DeformPart> = Vec::new();
        let mut lod_lists = LodLists::default();
        for (ai, a) in arts.iter().enumerate() {
            lod_lists.admit(car, &style, ai, a);
            // the glare articles hold no geometry, only an `ef` record (`particles.md` §4): its
            // lamp, the slot `Base[0x50]` that switches it (0x43C427 / 0x43D8C0), and a mirrored copy
            // across x with the slot `L+0x4E` (0x43DAC5…0x43DB32) when `L+0x48 & 8`. One `ef` per article in
            // every shipped model (0x43DDA5…0x43DE48 would walk on through consecutive ones).
            //
            // The effect pass 0x43D800 walks the LOD's part list, which 0x43C1B0 builds from the
            // articles the style admits (`carinfo[0x56C + 4·Base+68] == Base+69`, 0x43C2A1) that have
            // a `Base` LOD record for this LOD — the first `k` in `[Base+8, Base+0xC)` whose `word
            // Base+0x6C + 12k` is it (0x43C2AA…0x43C2C9); the part's `[article+0x48]` is that record
            // (0x43C42A…0x43C431), and the pass skips the part when the record's **byte +1** is 0
            // (0x43D863…0x43D872) before it looks for the `ef` (0x43D88B). Across every shipped
            // model the byte is 1 on each LOD record of an article with an `ef` and 0 on every
            // other, so it is the "has effects" flag; the style test is what drops lamps: glare
            // articles carry types 0, 1, 4, 5, 7, 18, 19, 20 and 27 with their variants, and one
            // of another variant was lit alongside the style's own.
            let effect_part = effect_part(car, &style, a, lod);
            if !effect_part && a.parts.iter().any(|p| p.is_part(b"ef")) {
                continue;
            }
            if let Some(lamps) = glare_lamps(car, a, lod) {
                lights.extend(lamps);
                continue;
            }
            let g = car.crp.article_geometry(&car.crp_data, a)?;
            let (ty, variant) = g.base.as_ref().map(|b| (b.tag[0], b.tag[1])).unwrap_or((0, 0));
            if !style.article_enabled(ty, variant) {
                continue;
            }
            let is_wheel = matches!(ty, 0x0D | 0x0E);
            // an animated article's morph frames: each frame's geometry — 0x43C4FB probes `vt`
            // part `frame·0x10 + index` until one is missing, and the count is the animation's
            // length when it is the largest (0x43C54B…0x43C56F)
            let anim = g.base.as_ref().map(|b| b.anim as usize).filter(|&n| n > 0 && n < ANIMS).unwrap_or(0);
            let frame_geometry: Vec<nfs_formats::crp::ArticleGeometry> = if !is_wheel && anim != 0 {
                (1u16..).map_while(|f| car.crp.article_geometry_frame(&car.crp_data, a, f).ok()).collect()
            } else {
                Vec::new()
            };
            // 0x43E390 crushes the parts whose `Base+78` has bit 3 (0x43E3D8) from their
            // `vt#(0x8000|lod)` vectors by their `dz#lod` zones, into the buffer `*(part+0x40)`
            // points at. *(Settled 2026-09-27: a part that also morphs (the convertibles' roof,
            // `Base+77`) is not excluded there — 0x43E390 only gates on `Base+78 & 8` and a
            // non-null `part+0x40` (0x43E3D8, 0x43E3EF) — so it IS crushed. The two writers of
            // `*(part+0x40)`: the crush, 0x43E390, called from `R3DCar` slot 7's `0x438230(0)`
            // (0x4383CC); the morph blend, 0x43E120 (its own destination load at 0x43E225,
            // `ebp = *(part+0x40)`), called from the SAME slot's `0x438030` (0x4383D3) — right
            // after 0x438230, in the same per-frame update. Both are edge-triggered (0x438230 on a
            // changed zone value, 0x438030 on a queued, changed animation), so on a frame where
            // both trigger the morph runs last and wins; the crush persists on a stationary part
            // between hits and is erased the next time that part's animation changes. Ported: the
            // article now goes in both `deform` and `morph`; `App::update_damage_decals` already
            // calls `update_damage` before `update_animation` / `update_driver`
            // (`CarModel::blend`'s own dedup on `(frame, next, frac)` gives the edge-triggering).)*
            let damage_geometry = if g.base.as_ref().is_some_and(|b| b.draw_bytes[0] & 8 != 0) { car.crp.article_geometry_damage(&car.crp_data, a).ok() } else { None };
            // Every LOD the article has primitives in: the car draw 0x444E00 takes each pass from
            // its own LOD object (`CarPass`). (history: `game-history.md` §10.7)
            let lods: std::collections::BTreeSet<u8> = g.primitives.iter().map(|p| p.lod).filter(|l| (1..=7).contains(l)).collect();
            for l in lods {
                // 0x43C8F8: the part's `"tr"` + the LOD, **no fallback** — without one the part
                // is drawn in the car's own frame and the whole matrix block, the translation and
                // rotation channels with it, is skipped (0x43C902 → 0x43CAFA). Such parts are
                // modelled in car space: the 356's `DoorWindow` at LOD 4 sits where LOD 1's does
                // through its `tr` (`lodtris` example; 314 articles of the shipped cars, none at
                // LOD 1).
                let tr = g.transforms.iter().find(|(i, _)| *i == l).map(|t| Mat4::from_cols_array(&t.1));
                let local = tr.unwrap_or(Mat4::IDENTITY);
                let info = PartInfo {
                    lod: l,
                    mask: g.base.as_ref().map_or(0, |b| b.draw_bytes[0]),
                    inside: g.base.as_ref().is_some_and(|b| b.draw_bytes[3] != 0),
                    inside_only: g.base.as_ref().is_some_and(|b| b.draw_bytes[4] != 0),
                    move_channel: if tr.is_some() { g.base.as_ref().map_or(-1, |b| b.move_channel) } else { -1 },
                };
                let rot_channel = if tr.is_some() { g.base.as_ref().map_or(0, |b| b.rot_channel) } else { 0 };
                let roof_frames: Vec<Vec<MeshData>> = frame_geometry
                    .iter()
                    .map(|gf| {
                        let mut sk = 0;
                        article_meshes(gf, l, &mut sk).into_iter().map(|m| m.mesh).collect()
                    })
                    .collect();
                if !roof_frames.is_empty() {
                    anim_length[anim] = anim_length[anim].max(roof_frames.len() as u32 + 1);
                }
                // the crush vectors and zones laid out as the meshes are: the damage geometry, and
                // the geometry with each vertex's position replaced by its zone (the `dz` lists
                // index the whole `vt#lod`, a primitive's window starting at `vt_first`)
                let crush = damage_geometry.as_ref().and_then(|dg| crush_meshes(car, a, &g, dg, l));
                let mut parts = Vec::new();
                let mut part_channels: Vec<u8> = Vec::new();
                for (k, mm) in article_meshes(&g, l, &mut skipped).into_iter().enumerate() {
                    let mut mesh = mm.mesh;
                    // the port's own measures (the model box, the tyre radius, the track) come
                    // from the reference LOD alone
                    if l == lod {
                        for v in &mut mesh.vertices {
                            let p = (MODEL_TO_BODY * local).transform_point3(Vec3::from(v.position));
                            if !is_wheel {
                                min = min.min(p);
                                max = max.max(p);
                            } else {
                                // the tyre's rolling radius is its extent about the hub, not the distance
                                // to a corner of the wheel's quad: the 993's wheel mesh spans ±0.3148 in
                                // both y and z, and the `.sim` tyre (16\" rim, 205/55) has a radius of
                                // 16·12.7 + 205·0.55 = 315.95 mm
                                let lp = Vec3::from(v.position);
                                wheel_radius = wheel_radius.max(lp.y.abs()).max(lp.z.abs());
                            }
                        }
                    }
                    let material = material_for(r, mm.material);
                    let id = r.upload_mesh(&mesh);
                    if !roof_frames.is_empty() {
                        let mut positions = vec![mm_positions(&mesh)];
                        for f in &roof_frames {
                            match f.get(k) {
                                Some(m) if m.vertices.len() == mesh.vertices.len() => positions.push(mm_positions(m)),
                                _ => break,
                            }
                        }
                        morph[anim].push(MorphPart { mesh: id, scratch: mesh.vertices.clone(), seen: 0, frames: positions });
                    }
                    if let Some((dm, zm)) = &crush
                        && let (Some(d), Some(z)) = (dm.get(k), zm.get(k))
                        && d.mesh.vertices.len() == mesh.vertices.len()
                        && z.mesh.vertices.len() == mesh.vertices.len()
                    {
                        deform.push(DeformPart {
                            mesh: id,
                            base: mesh.vertices.clone(),
                            indices: mesh.indices.clone(),
                            delta: d.mesh.vertices.iter().map(|v| v.position).collect(),
                            zone: z.mesh.vertices.iter().map(|v| v.position[0] as u8).collect(),
                        });
                    }
                    parts.push((id, material, local));
                    part_channels.push(rot_channel);
                }
                if parts.is_empty() {
                    continue;
                }
                if is_wheel && tr.is_some() {
                    let pivot = MODEL_TO_BODY.transform_point3(local.w_axis.truncate());
                    if l == lod {
                        half_track = half_track.max(pivot.x.abs());
                    }
                    lists.wheels.push(CarWheelModel { pivot, is_front: ty == 0x0D, is_left: local.w_axis.x > 0.0, channel: rot_channel, order: u32::MAX, info, parts });
                    lists.wheel_art.push(ai);
                } else {
                    lists.body_art.extend(std::iter::repeat_n(ai, parts.len()));
                    lists.body_info.extend(std::iter::repeat_n(info, parts.len()));
                    lists.body.extend(parts);
                    lists.body_channel.extend(part_channels);
                }
            }
        }
        let (body_order, unlisted) = lists.order(&mut lod_lists.list);
        let CarParts { body, body_channel, body_info, wheels, .. } = lists;
        tracing::info!(unlisted, "car part order");
        tracing::info!(body_parts = body.len(), wheels = wheels.len(), lights = lights.len(), roof_parts = morph[1].len(), roof_length = anim_length[1], anims = ?anim_length, skipped, pages = pages.len(), wheel_radius, half_track, "car uploaded");
        tracing::info!(patches = damage_patches.len(), night, "damage decals");
        Ok(Self {
            scale: 1.0,
            mirror: false,
            body,
            body_channel,
            body_order,
            body_info,
            lod_parts: lod_lists.parts,
            lod_moves: lod_lists.moves,
            material_light,
            shadowed,
            morph,
            anim_length,
            anim_time: [-1.0; ANIMS],
            roof_state: 0,
            driver: DriverAnim::default(),
            blended: [None; ANIMS],
            channel_turns: [0.0; 32],
            channel_yz: [[0.0; 2]; 32],
            dash: [0.0; 4],
            clock_ms: 0,
            move_spoiler_extent: tpg_f32(&car.tpg, "movespoilerextent"),
            headlight_extent: tpg_f32(&car.tpg, "headlightextent"),
            spoiler: ChannelEase::default(),
            headlight: ChannelEase::default(),
            door_axis: car
                .tpg
                .section("header")
                .and_then(|h| h.get_f32_list("dooraxis"))
                .filter(|v| v.len() >= 3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .unwrap_or(Vec3::Y),
            hood_extent: tpg_f32_or(&car.tpg, "hoodextent", 0.15),
            trunk_extent: tpg_f32_or(&car.tpg, "trunkextent", -0.15),
            hood: ChannelEase::default(),
            trunk: ChannelEase::default(),
            wheels,
            bbox_min: min,
            bbox_max: max,
            wheel_radius,
            half_track,
            lights,
            // a race car's pixel pages are read only for the pages a damage patch lands on
            // ([`Self::update_damage`], [`Self::instantiate`]); `blit_page` is the front-end rooms'
            // — so the other pages' pixels (on the GPU already) are not kept, in the model or in
            // every instance cloned from it
            damage_pages: race_damage_pages(pages, &damage_patches, race_roof.is_some()),
            damage_patches,
            page_textures,
            damage_shown: [0.0; 10],
            deform,
        })
    }

    /// A private copy of this model's damage-mutable GPU resources, for one traffic/cop car
    /// instance of a shared catalogue model (`RE/notes/car-model.md` §13.1's residual gap).
    ///
    /// The original shares one upload per catalogue type too (`R3DCar` slot 7, 0x4383C0), but
    /// recomputes a drawn car's decals/crush from *its own* raw damage and draws it — synchronously,
    /// through the software blitter 0x572B40 — before the next car (0x4383CC/0x4383D3, no
    /// car-kind gate), so two cars of one model always show correctly despite sharing the upload.
    /// This port's renderer instead records a frame's `DrawItem`s and reads each `MeshId`/
    /// `TextureId`/`MaterialId` once at `render()`; without this, two *simultaneously visible*,
    /// differently damaged instances of one shared model would alias to whichever instance called
    /// [`Self::update_damage`] last that frame.
    ///
    /// Cost: a fresh vertex+index buffer per crushable part ([`DeformPart`], `self.deform.len()`
    /// of them — a handful of body panels, not the whole model) and a fresh texture plus a fresh
    /// material per damage-decal page actually referenced by [`nfs_assets::car::DamagePatch`]
    /// (typically one or two pages) — once per car at grid setup, not per frame. Everything else
    /// (LOD/wheel/light geometry, the environment/specular textures, every non-decal material) is
    /// `self.clone()`d as plain ids — cheap, no GPU allocation — and stays shared with the
    /// template this was instantiated from.
    pub fn instantiate(&self, r: &mut Renderer) -> Self {
        let mut c = self.clone();
        // the crushable parts: a fresh buffer holding the pristine (undamaged) positions —
        // `update_damage` overwrites it once this instance takes zone damage. Re-uploaded from
        // `base`/`indices`, not copied GPU-side, so a template already crushed by another
        // instance is not what this instance starts from.
        for p in &mut c.deform {
            p.mesh = r.upload_mesh(&MeshData { vertices: p.base.clone(), indices: p.indices.clone() });
        }
        // the decal pages any zone can touch: a fresh texture (pristine pixels) plus a fresh
        // material for every existing material that samples it, so its bind group is not shared
        // with the template's or another instance's
        let mut touched: Vec<usize> = c.damage_patches.iter().map(|p| p.page).collect();
        touched.sort_unstable();
        touched.dedup();
        if !touched.is_empty() {
            let mut tex_remap: HashMap<nfs_renderer::TextureId, nfs_renderer::TextureId> = HashMap::new();
            for page_index in touched {
                let (Some(base), Some(&old_tex)) = (c.damage_pages.get(page_index), c.page_textures.get(page_index)) else { continue };
                let new_tex = r.upload_texture_mips(base.width, base.height, std::slice::from_ref(&base.rgba));
                c.page_textures[page_index] = new_tex;
                tex_remap.insert(old_tex, new_tex);
            }
            let mut mat_remap: HashMap<MaterialId, MaterialId> = HashMap::new();
            let mut remap = |mid: &mut MaterialId, material_light: &mut MaterialMap<nfs_formats::crp::LayerLight>, shadowed: &mut MaterialSet| {
                if let Some(&nid) = mat_remap.get(mid) {
                    *mid = nid;
                    return;
                }
                let Some(&new_tex) = r.material_texture(*mid).as_ref().and_then(|t| tex_remap.get(t)) else { return };
                let nid = r.duplicate_material_texture(*mid, new_tex);
                if let Some(l) = material_light.get(mid).copied() {
                    material_light.insert(nid, l);
                }
                if shadowed.contains(mid) {
                    shadowed.insert(nid);
                }
                mat_remap.insert(*mid, nid);
                *mid = nid;
            };
            for (_, mid, _) in &mut c.body {
                remap(mid, &mut c.material_light, &mut c.shadowed);
            }
            for w in &mut c.wheels {
                for (_, mid, _) in &mut w.parts {
                    remap(mid, &mut c.material_light, &mut c.shadowed);
                }
            }
        }
        // this instance starts with no damage shown yet, matching a just-uploaded model — the
        // next `update_damage` call (before this instance's own `draw_items`) fills it in
        c.damage_shown = [0.0; 10];
        c
    }

    /// **0x438230(0)**, which `R3DCar` slot 7 runs every frame (0x4383CC): when any of the ten
    /// [`zone_damage`] values differs from the last ones kept at `+0x6A8` (an exact `fcomp`,
    /// 0x438240…0x438259), keep the new ones (0x4382A4…0x4382B7) and recompose the texture pages
    /// the damage decals touch from the car's **raw** zone damage `car+0x4E4` (0x4451E0 → 0x448010,
    /// `RE/notes/car-model.md` §11). `own_car`: the car the camera follows, whose side and roof
    /// zones are capped at 0 here — their changes alone do not trigger it. (history: `game-history.md` §10.8)
    ///
    /// Before the pages, **0x43E390** crushes the body for every LOD (0x438273…0x4382A2): at
    /// `CARDETAIL` (`detail`) above 0 (0x43E3E2), each part with `Base+78 & 8`, per `dz` zone whose
    /// value changed (0x438390): `f = value · 0.02`, 1 above 1 (0x43E54D…0x43E571), and each of the
    /// zone's vertices `vt + f · vt#(0x8000|lod)` (0x53C5D0, 0x43E5D4…0x43E617). (history: `game-history.md` §10.9)
    /// The byte `part+0x50` it sets
    /// above `f = 0.5` (0x43E584) is not read here — its reader is not found.
    pub fn update_damage(&mut self, r: &mut Renderer, damage: &[f32; 10], own_car: bool, detail: u8) {
        let now = zone_damage(damage, own_car);
        if now == self.damage_shown {
            return;
        }
        let changed: [bool; 10] = std::array::from_fn(|z| now[z] != self.damage_shown[z]);
        self.damage_shown = now;
        if detail > 0 {
            let f: [f32; 10] = std::array::from_fn(|z| {
                let f = now[z] * 0.02;
                if f <= 1.0 { f } else { 1.0 }
            });
            for p in &self.deform {
                if !p.zone.iter().any(|&z| changed.get(z as usize).copied().unwrap_or(false)) {
                    continue;
                }
                let mut v = p.base.clone();
                for (k, out) in v.iter_mut().enumerate() {
                    let Some(&k_f) = f.get(p.zone[k] as usize) else { continue };
                    let d = p.delta[k];
                    out.position = [out.position[0] + k_f * d[0], out.position[1] + k_f * d[1], out.position[2] + k_f * d[2]];
                }
                r.update_mesh_vertices(p.mesh, &v);
            }
        }
        if self.damage_patches.is_empty() {
            return;
        }
        let touched: Vec<usize> = {
            let mut v: Vec<usize> = self.damage_patches.iter().map(|p| p.page).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        for page_index in touched {
            let Some(base) = self.damage_pages.get(page_index) else { continue };
            let Some(&tex) = self.page_textures.get(page_index) else { continue };
            let mut page = base.clone();
            for patch in self.damage_patches.iter().filter(|p| p.page == page_index) {
                nfs_assets::car::blend_patch(&mut page, patch, damage[patch.zone as usize]);
            }
            r.update_texture(tex, &TextureData::single(page.width, page.height, page.rgba));
        }
    }

    /// Slot 9's race branch for the parts this port animates (0x43891E…0x438B27, the race only —
    /// `[0x65B928]` set skips it) and slot 6's headlight easer.
    ///
    /// **The pop-up wing is animation 2**, the `SpoilerW` morph — not channel 16, which this said
    /// until 2026-09-22 (channel 16 is written only in the front end, from the roof: see
    /// [`Self::follow_roof`]). Above `[0x5B3698]` = 22.222 m/s (**80 km/h**) of `car+0xEC4`, and
    /// only once the easer at `car+0x64 + 0x170` has run its course (0x438950), it restarts from
    /// where it is toward animation 2's length (`[car+0x59C]`, 0x43895E) over **2000 ms**; below
    /// `[0x5B3694]` = 13.889 (**50**) the same toward 0 (0x4389D1…0x438A57). It re-arms whenever
    /// the gate holds and the last travel is over, even toward the value it already has — so a
    /// retract can wait out an idle travel. The value (0x438A5E…0x438AD1: `from + (to − from)·t`,
    /// `to` once `t` passes 1) goes to 0x4380D0(2, ·).
    ///
    /// **The aerial is animation 7**, and only a `.TINYCAR` moves it (0x438ADE…0x438B27):
    /// `min(0.067 · car+0xEC4, 1)` (`[0x5B3690]`), 0 in reverse (`car+0xD82 == 0`).
    ///
    /// **The headlights**, channel 17, are slot 6's easer at 0x43950E…0x439620: a **1000 ms**
    /// travel from the car's headlight bit — which `Car_init` freezes at 0x410C74 as
    /// `word[0x628A28] ? 3 : 0`, so it is the track's night flag and never changes during a race.
    pub fn update_animation(&mut self, r: &mut Renderer, dt_ms: u32, speed_mps: f32, headlights_on: bool, tiny: bool, reverse: bool) {
        // the wing's easer: advance, then (0x43892B) re-arm when the gate holds and it is done
        let e = &mut self.spoiler;
        if e.elapsed < e.duration {
            e.elapsed = e.elapsed.saturating_add(dt_ms);
        }
        let len = self.anim_length[2] as f32;
        let want = if speed_mps > 22.222 {
            Some(len)
        } else if speed_mps < 13.889 {
            Some(0.0)
        } else {
            None
        };
        if let Some(want) = want
            && e.elapsed >= e.duration
        {
            // 0x438970…0x4389CC: the travel's current value becomes the new start
            e.from = e.to;
            e.to = want;
            e.elapsed = 0;
            e.duration = 2000;
        }
        let v = if e.duration == 0 || e.elapsed >= e.duration {
            e.to
        } else {
            e.from + (e.to - e.from) * (e.elapsed as f32 / e.duration as f32)
        };
        self.set_anim(r, 2, v);
        if tiny {
            let t = if reverse { 0.0 } else { (speed_mps * 0.067).min(1.0) };
            self.set_anim(r, 7, t);
        }
        if self.headlight_extent != 0.0 {
            let e = &mut self.headlight;
            let want = if headlights_on { 1.0 } else { 0.0 };
            if e.elapsed < e.duration {
                e.elapsed = e.elapsed.saturating_add(dt_ms);
            } else if e.to != want {
                e.from = e.to;
                e.to = want;
                e.elapsed = 0;
                e.duration = 1000;
            }
            let t = if e.duration == 0 { 1.0 } else { (e.elapsed as f32 / e.duration as f32).min(1.0) };
            let v = e.from + (e.to - e.from) * t;
            let x = self.headlight_extent;
            self.set_channel(17, v * x);
        }
    }

    /// Slot 9's driver half (0x438B2C…0x439352, the race only): the right arm (animation 4), the
    /// left arm (5) and the body (6), `CarModel`'s eight keyframe players, and `R3DCar+0x564` /
    /// `+0x566`. `RE/notes/car-model.md` "Slot 9" has the whole of it.
    pub fn update_driver(&mut self, r: &mut Renderer, dt_ms: u32, d: &DriverInputs) {
        // under detail 1 the driver is posed and nothing else runs (0x438B2C…0x438B5A)
        if d.detail < 1 {
            self.set_anim(r, 4, 4.5);
            self.set_anim(r, 5, 4.5);
            self.set_anim(r, 6, 5.0);
            return;
        }
        self.driver.clock_ms += dt_ms as u64;
        let now = self.driver.clock_ms;
        for p in &mut self.driver.players {
            p.step(now);
        }
        // the steering frame: 23·ch5.z + 4.5 (0x438C4F…0x438C6B), 0…9 as a table index
        // (0x438CC8…0x438D7E: under 0 → 0, over 9 → 9, else fistp(f − 0.5))
        let f = d.wheel_turns.clamp(-STEERING_WHEEL_LIMIT, STEERING_WHEEL_LIMIT) * 23.0 + 4.5;
        let index = |f: f32| -> usize {
            if f < 0.0 {
                0
            } else if f > 9.0 {
                9
            } else {
                (f - 0.5).round_ties_even().max(0.0) as usize
            }
        };
        self.set_anim(r, 5, f);
        // a gesture already running is copied into the arm's record (0x438C7A → 0x439035);
        // otherwise the gear change may start one (0x438C92…0x438E18), then the handbrake
        // (0x438E3A…0x438FEA), and on the frame one starts the arm is not written (0x438FF4)
        let arm = self.driver.players[4];
        if arm.active() {
            self.set_anim_keys(r, 4, arm.frame, arm.next, arm.frac);
        } else {
            if d.shift_timer > 0 && d.racing {
                let ms = (d.shift_timer as i64 * 2000 / 32) as u64;
                self.driver.players[4].start(GEAR_GESTURES[index(f)], now, ms);
            }
            if !self.driver.players[4].active() && d.handbrake != self.driver.handbrake {
                self.driver.handbrake = d.handbrake;
                let i = index(f);
                let t: &'static [u32] = if d.handbrake { &HANDBRAKE_ON[i] } else { &HANDBRAKE_OFF[i] };
                self.driver.players[4].start(t, now, 125);
            }
            if !self.driver.players[4].active() {
                self.set_anim(r, 4, if self.driver.handbrake { 19.0 } else { f });
            }
        }
        // the body: 2 − 0.1·car+0xE08, the lateral acceleration's dead band 0.1 (0x439091…0x4390D5)
        let lat = if d.lat_acc.abs() < 0.1 { 0.0 } else { -d.lat_acc };
        let b = 0.1 * lat + 2.0;
        let body = self.driver.players[6];
        if body.active() {
            // 0x4390DA → 0x4392F1
            self.set_anim_keys(r, 6, body.frame, body.next, body.frac);
            return;
        }
        if d.reverse != self.driver.reverse {
            self.driver.reverse = d.reverse;
            let i = if b < 0.0 {
                0
            } else if b > 4.0 {
                4
            } else {
                (b + 0.5) as usize
            };
            let t = if d.reverse { REVERSE_ON[i] } else { REVERSE_OFF[i] };
            self.driver.players[6].start(t, now, 250);
        }
        if self.driver.players[6].active() {
            // 0x439216: started this frame, not written
        } else if self.driver.reverse {
            // looking back over the shoulder (0x43922E…0x439241)
            self.set_anim(r, 6, 7.0);
        } else if d.long_acc <= -0.1 {
            // braking: the body frame toward frame 5 by 0.1·|car+0xE04| (0x43924D…0x4392D9)
            let frame = (b - 0.5).round_ties_even().max(0.0) as u32;
            self.set_anim_keys(r, 6, frame, 5, -0.1 * d.long_acc);
        } else {
            self.set_anim(r, 6, b);
        }
    }

    /// Slot 9's front-end branch for channel **16**, `MoveSpoiler` (0x438751…0x4387CC): it
    /// follows the **roof**, animation 1 (`car+0x57C` frame + `+0x584` fraction = its time `v`,
    /// `+0x588` its length `L`): `v · extent` over the first frame, the full `movespoilerextent`
    /// until the last, `(L − v) · extent` over it. So the 996's and the Boxster's rear lid turns
    /// out of the way while the roof folds, and back.
    pub fn follow_roof(&mut self) {
        if self.move_spoiler_extent == 0.0 || self.anim_length[1] == 0 {
            return;
        }
        let v = self.anim_time[1].max(0.0);
        let l = self.anim_length[1] as f32;
        let e = self.move_spoiler_extent;
        let t = if (0.0..1.0).contains(&v) {
            v * e
        } else if v < l - 1.0 {
            e
        } else {
            (l - v) * e
        };
        self.set_channel(16, t);
    }

    /// The rotation a channel's three floats compose to: `Rx·Ry·Rz`, each angle in **turns**
    /// (0x43C9C0 / 0x43C9D7 / 0x43C9F8, all `× 360` into the axis-angle build 0x539BD0, and
    /// accumulated in that order by the two 0x539AD0 multiplies at 0x43CA26 / 0x43CA38).
    ///
    /// The port only ever fills the x term, so the y and z rotations are identity here; they are
    /// written this way round so a later channel that uses them lands in the right order.
    fn channel_matrix(&self, n: u8) -> Mat4 {
        let t = self.channel_turns.get(n as usize).copied().unwrap_or(0.0);
        let [y, z] = self.channel_yz.get(n as usize).copied().unwrap_or([0.0; 2]);
        if t == 0.0 && y == 0.0 && z == 0.0 {
            return Mat4::IDENTITY;
        }
        // **Channels 6 and 7 are the doors** (0x43CA5A): they turn about the `.tpg` `dooraxis`,
        // normalised (0x5328D0), with its x negated for 7, by the channel's *y* in turns — through
        // 0x5326C0, the same axis-angle build 0x539BD0 uses for the others, so the same
        // convention. The door value is kept in `channel_turns` like the rest.
        if n == 6 || n == 7 {
            let mut a = self.door_axis;
            if n == 7 {
                a.x = -a.x;
            }
            return Mat4::from_axis_angle(a.normalize_or(Vec3::Y), t * std::f32::consts::TAU);
        }
        // Rx·Ry·Rz, in that order (0x43CA26 / 0x43CA38); the y and z senses follow the x one,
        // MEDIUM until the needles and the steering wheel have been seen turning the right way
        use std::f32::consts::TAU;
        Mat4::from_rotation_x(t * TAU) * Mat4::from_rotation_y(y * TAU) * Mat4::from_rotation_z(z * TAU)
    }

    /// A lamp's two frames in the body frame (lamp's body-frame data → car body), as the effect
    /// pass 0x43D800 builds them per `ef` record: the **glare's** `Rch · tr` in front of the car's
    /// matrix (0x43D8D1…0x43DA46; `Rch` = `Rx·Ry·Rz` of the channel's three turns, 0x43D952…
    /// 0x43D9FE) — the sprite, the reflection and the beam's rings go through it — and the
    /// **beam's** own, the same with the x term left out (0x43DA03 resets to identity and
    /// multiplies only `Ry·Rz`, 0x43DA0F…0x43DA46), through which 0x43D4F0 carries only the
    /// forward `(0, 0, −1)` (0x43D5A3…0x43D5F4): a pop-up headlight's glare tips with the lamp,
    /// its beam still points down the road. The three floats compose straight, with no door
    /// case (unlike the body's 0x43CA5A); the order and senses are `channel_matrix`'s.
    pub fn lamp_frames(&self, l: &CarLight) -> (Mat4, Mat4) {
        use std::f32::consts::TAU;
        let n = l.channel as usize;
        let (x, [y, z]) = if n == 0 {
            (0.0, [0.0; 2])
        } else {
            (self.channel_turns.get(n).copied().unwrap_or(0.0), self.channel_yz.get(n).copied().unwrap_or([0.0; 2]))
        };
        let yz = Mat4::from_rotation_y(y * TAU) * Mat4::from_rotation_z(z * TAU);
        let body = |m: Mat4| MODEL_TO_BODY * l.frame * m * MODEL_TO_BODY;
        (body(Mat4::from_rotation_x(x * TAU) * yz), body(yz))
    }

    /// Whether the car has pop-up headlights: `headlightextent`, `R3DCar+0x70`, non-zero only on
    /// the 914, 928 and 944. 0x439627 tests it: only then do the headlight slots take the ramp's
    /// value over it (0x43963B…0x43969E); with 0 they are 1.0 or 0 at once (0x43972D…0x43973E).
    pub fn has_headlight_extent(&self) -> bool {
        self.headlight_extent != 0.0
    }

    /// The dash needles' calibration from the car's `nfs5.car` record (see `dash`).
    pub fn set_dash(&mut self, tach0: f32, tach1000: f32, speedo0: f32, speedo100: f32) {
        self.dash = [tach0, tach1000, speedo0, speedo100];
    }

    /// Slot 9's race channels (0x438461…0x43874C) other than the wheels' own draw: the steering
    /// wheel, the two needles, the non-spinning wheel parts and the siren.
    ///
    /// `steer` is each front wheel's `+0xB4` over 216 (turns, negated in a mirrored race); `rpm`
    /// is `car+0xDB4`, `speed` `car+0xEC4`; `dt_ms` advances the clock 0x53C2E0 stands for.
    pub fn update_race_channels(&mut self, dt_ms: u32, steer: [f32; 2], spin: [f32; 4], rpm: f32, speed: f32) {
        self.clock_ms += dt_ms as u64;
        // `TIMER_getTicks` (0x53C2E0 → `[0x6B7C7C]`) counts at 128 Hz (`timing.md`)
        let clock = (self.clock_ms * 128 / 1000) as u32;
        let yz = &mut self.channel_yz;
        // 1, 2 `.y` = the fronts' steer; 1…4 `.x` = −the wheel's turn (`+0x88`); `.z` is the
        // wheel's `+0x70`, which this port's suspension does not produce (left at 0)
        for (k, s) in steer.iter().enumerate() {
            yz[1 + k][0] = *s;
        }
        for k in 0..4 {
            self.channel_turns[1 + k] = -spin[k];
        }
        // 11 `.z`: `.TACH0RPM + .TACH1000RPM · rpm · 0.001` (0x43853C…0x438561); 12 `.z`:
        // `.SPEEDO0KM + .SPEEDO100KM · car+0xEC4 · 0.144` (0x43856C…0x438593)
        let [t0, t1, s0, s1] = self.dash;
        yz[11][1] = t0 + t1 * rpm * 0.001;
        yz[12][1] = s0 + s1 * speed * 0.144;
        // 5 `.z`: the steering wheel, `ch1.y + ch2.y` within ±4.5/23 (0x43858F…0x4385FB)
        yz[5][1] = (steer[0] + steer[1]).clamp(-STEERING_WHEEL_LIMIT, STEERING_WHEEL_LIMIT);
        // 18…21: channels 1…4 with `.x` = 0 — the tread and the shadow turn but do not spin
        // (0x438669…0x438726)
        for k in 0..4 {
            yz[18 + k] = yz[1 + k];
            self.channel_turns[18 + k] = 0.0;
        }
        // 22 `.y`: 0x53C2E0() · 1/128 — the cop's siren glare, a turn a second (0x43872C…0x43874C)
        yz[22][0] = (clock as f32 * 0.007_812_5).fract();
    }

    /// Channel 22 alone — the one slot 9 channel that reads no car state, only the clock — for the
    /// models every car of a type shares (traffic, the cops): their sirens turn as one, which is
    /// what the original's per-car copies do too.
    pub fn update_clock_channels(&mut self, dt_ms: u32) {
        self.clock_ms += dt_ms as u64;
        let clock = (self.clock_ms * 128 / 1000) as u32;
        self.channel_yz[22][0] = (clock as f32 * 0.007_812_5).fract();
    }

    /// The lids' two easers, 0x439A00 / 0x439AA0 (hood) and 0x439B30 / 0x439BD0 (engine lid):
    /// a travel already running is left alone (0x439A1E); otherwise it restarts from where it
    /// ended toward the extent (`open`) or 0, over **1000 ms** (0x439A81).
    pub fn set_lid(&mut self, hood: bool, open: bool) {
        let (e, target) = if hood {
            (&mut self.hood, if open { self.hood_extent } else { 0.0 })
        } else {
            (&mut self.trunk, if open { self.trunk_extent } else { 0.0 })
        };
        if e.elapsed < e.duration {
            return;
        }
        e.from = e.to;
        e.to = target;
        e.elapsed = 0;
        e.duration = 1000;
    }

    /// Advance the lid easers and write channels 8 and 9 (slot 9, 0x4387CE…0x43891C): the easer's
    /// `from + (to − from)·t`, and `to` once it has run its course. Front-end cars only; the race
    /// never opens a lid.
    pub fn update_lids(&mut self, dt_ms: u32) {
        for (n, e) in [(8u8, self.hood), (9u8, self.trunk)] {
            let v = if e.duration == 0 || e.elapsed >= e.duration {
                e.to
            } else {
                e.from + (e.to - e.from) * (e.elapsed as f32 / e.duration as f32)
            };
            self.set_channel(n, v);
        }
        for e in [&mut self.hood, &mut self.trunk] {
            if e.elapsed < e.duration {
                e.elapsed = e.elapsed.saturating_add(dt_ms);
            }
        }
    }

    /// Set one animation channel, in turns. Channel **16** is `MoveSpoiler`, **17** the pop-up
    /// `HeadLight`; both are `Base+75` bindings confirmed against every shipped model.
    pub fn set_channel(&mut self, n: u8, turns: f32) {
        if let Some(slot) = self.channel_turns.get_mut(n as usize) {
            *slot = turns;
        }
    }

    /// Set all three turns of one channel — `R3DCar` 0x443F30(n, &v), which copies the vector
    /// into `object + 0x2DC + 12n` whole.
    pub fn set_channel_vec(&mut self, n: u8, v: [f32; 3]) {
        if let (Some(x), Some(yz)) = (self.channel_turns.get_mut(n as usize), self.channel_yz.get_mut(n as usize)) {
            *x = v[0];
            *yz = [v[1], v[2]];
        }
    }

    /// Write `src` (RGBA, `w` × `h`) into texture page `page` (0-based, `tpage<page+1>` — the
    /// order of the model's page array `[R3DCar+0x78]`, 0x34 bytes an entry, 0x442F1B) at `(x, y)`
    /// the way the software blitter stores a texel in the page's format
    /// ([`nfs_assets::car::blitter_texel`]), clipped to the page, and upload it — what 0x534480
    /// (the page as the destination), a blit and 0x443C80 (the page's refresh, 0x447AF0) do. The
    /// page stays written: a later blit lands over it, as on the original's surface.
    #[allow(clippy::too_many_arguments)]
    pub fn blit_page(&mut self, r: &mut Renderer, page: usize, src: &[u8], w: u32, h: u32, x: i64, y: i64) {
        let (Some(p), Some(&tex)) = (self.damage_pages.get_mut(page), self.page_textures.get(page)) else { return };
        for row in 0..h as i64 {
            let py = y + row;
            if py < 0 || py >= p.height as i64 {
                continue;
            }
            for col in 0..w as i64 {
                let px = x + col;
                if px < 0 || px >= p.width as i64 {
                    continue;
                }
                let Some(s) = src.get(((row as u32 * w + col as u32) * 4) as usize..).and_then(|s| s.get(..4)) else { continue };
                let t = nfs_assets::car::blitter_texel(p.bpp, [s[0], s[1], s[2], s[3]]);
                let k = ((py as u32 * p.width + px as u32) * 4) as usize;
                p.rgba[k..k + 4].copy_from_slice(&t);
            }
        }
        r.update_texture(tex, &TextureData::single(p.width, p.height, p.rgba.clone()));
    }

    /// A channel's three turns — 0x443F10(n).
    pub fn channel_vec(&self, n: u8) -> [f32; 3] {
        let x = self.channel_turns.get(n as usize).copied().unwrap_or(0.0);
        let [y, z] = self.channel_yz.get(n as usize).copied().unwrap_or([0.0; 2]);
        [x, y, z]
    }

    /// Draw items for a car at `car_to_world` (body frame → world). `wheel_spin` in turns,
    /// `steer` in turns for the front wheels, positive to the right — take it from
    /// `Vehicle::wheel_steer_turns`, not from the raw `Wheel::steer`. `travel` is the car's four
    /// translation channels' `.y` ([`CarLodState::travel`]) and `passes` the car draw's calls
    /// ([`race_passes`], [`in_car_passes`], [`FRONT_END_PASSES`]).
    ///
    /// Each pass is one `R3DCarLOD::draw` 0x43C830: the parts of its LOD whose `Base+78` meets its
    /// mask ([`PartInfo::in_pass`]) in `Base+70` order (0x43C1B0's sort of that LOD's list), and
    /// the passes one after the other as 0x444E00 makes them — the driver, then the body, then the
    /// wheels.
    pub fn draw_items(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2], travel: [f32; 4], passes: &[CarPass]) -> Vec<DrawItem> {
        let mut out = Vec::with_capacity(self.body.len() / 4 + 4 * 8);
        DRAW_SCRATCH.with(|s| self.draw_items_into(&mut out, &mut s.borrow_mut(), car_to_world, wheel_spin, steer, travel, passes));
        out
    }

    /// [`Self::draw_items`] appending to `out` with the caller's scratch buffers, so a frame's cars
    /// allocate nothing per car beyond `out` itself.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_items_into(&self, out: &mut Vec<DrawItem>, scratch: &mut DrawScratch, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2], travel: [f32; 4], passes: &[CarPass]) {
        // 0x4433D2 / 0x4433BC: `(−m·s, s, −s)` — `MODEL_TO_BODY` scaled, its x turned back in a
        // mirrored race; and the steer channels negated there (0x438471)
        let car_to_world = car_to_world * Mat4::from_scale(Vec3::new(if self.mirror { -self.scale } else { self.scale }, self.scale, self.scale));
        let steer = if self.mirror { [-steer[0], -steer[1]] } else { steer };
        // the translation channel `Base+72` (0x43C945…0x43C982): `T(object + 0x6D0 + 12m)` into the
        // part's matrix after its `tr` and before its rotation — the only float of it written is
        // `.y` (0x43E6E1…0x43E6F1), so a lift along the model's (and the body's) up
        let lift = |m: i8| usize::try_from(m).ok().and_then(|m| travel.get(m).copied()).map_or(Mat4::IDENTITY, |y| Mat4::from_translation(Vec3::new(0.0, y, 0.0)));
        // (the same products every part makes, taken once: `(car_to_world · MODEL_TO_BODY) · m` is how
        // the expression below associates, and the inverse is of a constant)
        let to_body = car_to_world * MODEL_TO_BODY;
        let body_to_model = MODEL_TO_BODY.inverse();
        let DrawScratch { items, order } = scratch;
        for pass in passes {
            items.clear();
            for (i, (mesh, material, local)) in self.body.iter().enumerate() {
                let Some(info) = self.body_info.get(i).filter(|info| info.in_pass(pass)) else { continue };
                // A part bound to a rotation channel turns about **its own origin**: 0x43CA19…
                // 0x43CA38 compose Rx·Ry·Rz and 0x43CA43 multiplies that into the part's transform,
                // so the rotation is on the right of the part's own matrix.
                let local = &(*local * lift(info.move_channel));
                let m = match self.body_channel.get(i).copied().unwrap_or(0) {
                    0 => *local,
                    // 18…21 are channels 1…4 with `.x` cleared (0x438669…0x438726) — the wheel shadow
                    // and the parts that turn with the steer but do not spin. Taking them from the
                    // caller's `steer` instead of this model's own channels is what lets the traffic
                    // and the cops have them: those share one upload per car type.
                    n @ 18..=21 if n <= 19 => *local * Mat4::from_rotation_y(steer[(n - 18) as usize] * std::f32::consts::TAU),
                    20..=21 => *local,
                    // 5, the steering wheel: `.z = ch1.y + ch2.y` within ±4.5/23 (0x43858F…0x4385FB).
                    // Off the caller's `steer` for the same reason as 18…21 — the traffic and the cops
                    // share one model per car type and would otherwise hold the wheel straight.
                    5 => *local * Mat4::from_rotation_z((steer[0] + steer[1]).clamp(-STEERING_WHEEL_LIMIT, STEERING_WHEEL_LIMIT) * std::f32::consts::TAU),
                    n => *local * self.channel_matrix(n),
                };
                items.push((self.body_order.get(i).copied().unwrap_or(u32::MAX), DrawItem::new(*mesh, *material, to_body * m)));
            }
            for w in self.wheels.iter().filter(|w| w.info.in_pass(pass)) {
                let idx = match (w.is_front, w.is_left) {
                    (true, true) => 0,
                    (true, false) => 1,
                    (false, true) => 2,
                    (false, false) => 3,
                };
                // The original draws its wheels through channels 1…4 like every other moving part:
                // `.x = −(wheel+0x88)` (0x438600…0x438660), in **model** space, which is the body
                // frame turned half a turn about y — so `+turn` about the body's x. This read
                // `−turn` until 2026-09-22 and rolled the wheels backwards.
                // a wheel article on channels 18…21 takes the steer but **not** the spin: `.x` of those
                // channels is cleared at 0x438669…0x438726
                let spin = if (18..=21).contains(&w.channel) {
                    Mat4::IDENTITY
                } else {
                    Mat4::from_rotation_x(wheel_spin[idx] * std::f32::consts::TAU)
                };
                // **Positive steer is a right turn** (the right arrow gives +1, and the car yaws right
                // on it), and in the body frame +X is right and +Z forward — so the wheel turns by
                // `+steer`: `from_rotation_y(θ)` carries forward (0,0,1) to (sin θ, 0, cos θ), toward
                // +X. This said `-steer` until 2026-09-21 and drew the front wheels turned the wrong
                // way while the car steered correctly — found in play.
                // `.y` is the wheel's own `+0xB4` over 216 (channels 1 and 2, 0x438461…0x4384EB); a
                // turn about y is the same in both frames
                let st = if w.is_front { Mat4::from_rotation_y(steer[if w.is_left { 0 } else { 1 }] * std::f32::consts::TAU) } else { Mat4::IDENTITY };
                for (mesh, material, local) in &w.parts {
                    // local has translation = pivot (every wheel `tr` is a pure translation); rotate
                    // about the pivot in model space, the lift before it: `tr · T · R`
                    let pivot = local.w_axis.truncate();
                    let m = lift(w.info.move_channel) * Mat4::from_translation(pivot) * MODEL_TO_BODY * st * spin * body_to_model * Mat4::from_translation(-pivot) * *local;
                    items.push((w.order, DrawItem::new(*mesh, *material, to_body * m)));
                }
            }
            // the parts in the order 0x43C1B0's sort left the LOD's list (`body_order`, read
            // 2026-09-27: the VC6 `std::sort` of the whole list at load, reproduced by `vc6_sort`),
            // which 0x43C830 walks skipping the parts not in the pass (0x43C8AC…0x43C8E9) — so
            // the pass's parts keep their places in the sorted whole list. One article's meshes
            // share a place and keep their own order (the stable sort).
            // (sorted as `(key, index)` pairs, the index breaking ties: a stable order, without moving the
            // fat items)
            order.clear();
            order.extend(items.iter().enumerate().map(|(i, (k, _))| (*k, i as u32)));
            order.sort_unstable();
            out.extend(order.iter().map(|&(_, i)| items[i as usize].1));
        }
    }

    /// The class count the LOD pick scales by (0x43A0DC…0x43A117): the 1-based number of the first
    /// LOD whose part list is empty (`R3DCarLOD` slot 6, 0x43ADF0, `+0x10`), 7 when none is.
    fn lod_span(&self) -> i32 {
        self.lod_parts.iter().position(|&n| n == 0).map_or(7, |k| k as i32 + 1)
    }

    /// `R3DCar` slot 13, **0x439F90**, in a race past its camera test (0x43A060…0x43A3B0): the LOD
    /// class `+0x2C0`, or `None` when the car covers 3 pixels or less and is not drawn (0x43A2E8,
    /// the caller 0x443192).
    ///
    /// The class starts from `CLASS_BIAS[CARDETAIL]` = 5, 4, 3, 2, 1 (the dll's table, written into
    /// the push operand 0x43A086; the file's 0x5CD5B8 was 3, 3, 2, 2) and 1 more on a machine under
    /// 40 MB (`GlobalMemoryStatus` 0x53C3D0 — never here). **The dll cuts the replay test out** (its jump at 0x43A08A
    /// goes to 0x43A099, past the file's `cmp [0x657424], 2` / `jl`): `CARDETAIL` above 0 takes 1 off in
    /// a race and a replay alike, an opponent (`car+0x52C & 8`) at `CARDETAIL` 0 adds 1, and a traffic car
    /// (`& 0x10`) starts from 0. The car's box — `car+0x3A8` as `(x, y, z)`, `(−2x, y, z)` and
    /// `(x, y, −z)` about the draw origin, through the car's rows (0x43A18D…0x43A206) — is projected
    /// (0x539700), and `size` = the bit-trick roots of the two edges' squared screen lengths from
    /// the first point, × the virtual width (640 at 4:3, `virtual_width`) / screen width (0x43A26A…0x43A2E5). Then `t = max(1 − rsqrt₁₂₀ · √size,
    /// 0) · (N − bias − 1)` with the bit-trick `rsqrt(120)` (0x5F400000 − bits/2) and `N`
    /// [`Self::lod_span`]; `t · (t + 1) / 2` for all but traffic; the class is `bias + fistp(t −
    /// 0.5)`, within `0 … 7 − (CARDETAIL > 1 ? 2 : 1)` (0x43A305…0x43A3B0).
    pub fn lod_class(&self, view: &LodView, origin: Vec3, rotation: glam::Quat, half: Vec3, detail: u8, kind: CarKind) -> Option<u8> {
        let cd = detail as i32;
        let mut bias = CLASS_BIAS[(cd as usize).min(4)];
        if cd > 0 {
            bias -= 1;
        } else if kind == CarKind::Opponent {
            bias += 1;
        }
        if kind == CarKind::Traffic {
            bias = 0;
        }
        let span = self.lod_span() - bias;
        let screen = |p: Vec3| {
            let c = view.view_projection * (origin + rotation * p).extend(1.0);
            glam::Vec2::new(c.x / c.w * view.width * 0.5, c.y / c.w * view.height * 0.5)
        };
        let s0 = screen(Vec3::new(half.x, half.y, half.z));
        let s1 = screen(Vec3::new(-2.0 * half.x, half.y, half.z));
        let s2 = screen(Vec3::new(half.x, half.y, -half.z));
        let d1 = (s1 - s0).length_squared();
        let d2 = (s2 - s0).length_squared();
        // x87: the two roots summed and scaled on the stack, `fst` to a float for the root below
        let size = (bit_root(d1) as f64 + bit_root(d2) as f64) * (view.virtual_width as f64 / view.ref_width as f64);
        if size <= 3.0 {
            return None;
        }
        let rsqrt_120 = f32::from_bits(0x5F40_0000 - (120f32.to_bits() >> 1));
        let mut t = 1.0 - rsqrt_120 as f64 * bit_root(size as f32) as f64;
        if t < 0.0 {
            t = 0.0;
        }
        t *= (span - 1) as f64;
        if kind != CarKind::Traffic {
            t *= (t + 1.0) * 0.5;
        }
        let k = ((t - 0.5) as f32).round_ties_even() as i32;
        let max = 7 - if cd > 1 { 2 } else { 1 };
        let class = bias + k;
        Some(if class >= max { max } else { class.max(0) } as u8)
    }

    /// The translation channels after a class: 0x43E670 over the LOD objects `0 … class − 1`
    /// (0x43A3B6…0x43A3F8; the driver view jumps there with the class it had), each writing channel
    /// `m`'s `.y` from wheel `[0x5B36F0 + 4m]`'s `+0x68` — `(0, 1, 2, 3)`, or `(1, 0, 3, 2)` in a
    /// mirrored race (`[0x65742C]`, 0x43E6B1…0x43E6C2) — while its gate says one of the LOD's parts
    /// rides it. **The dll walks at least LOD 0**: its jump at 0x43A3B6 (dll 0x10001070) loads the class
    /// as 1 when it is 0 before the loop's entry test (0x43A3C4) while the loop's own bound re-reads
    /// the real class (0x43A3EC), so a car at class 0 walks LOD 0 once — the file's loop walked none.
    pub fn update_travel(&self, state: &mut CarLodState, compression: [f32; 4], mirror: bool) {
        let map = if mirror { [1, 0, 3, 2] } else { [0, 1, 2, 3] };
        for moves in self.lod_moves.iter().take((state.class as usize).max(1)) {
            for m in 0..4 {
                if moves[m] {
                    state.travel[m] = compression[map[m]];
                }
            }
        }
    }
}

/// One mesh of a morphing article: its frame-0 vertices and every frame's positions.
#[derive(Clone)]
struct MorphPart {
    mesh: MeshId,
    /// The vertices last blended and queued for the mesh (the frame-0 vertices at first, which is
    /// what the upload holds): the next blend rewrites only the positions, in place.
    scratch: Vec<Vertex>,
    /// `Renderer::mesh_writes` of the mesh right after this part's last write (0 at the upload):
    /// the mesh still holds `scratch` while the count is the same.
    seen: u32,
    frames: Vec<Vec<[f32; 3]>>,
}

fn mm_positions(m: &MeshData) -> Vec<[f32; 3]> {
    m.vertices.iter().map(|v| v.position).collect()
}

impl CarModel {
    /// **0x4D1990(1, state)**: the roof animation to `length · state / 255` frames (0x4D199F…
    /// 0x4D19BA), which 0x4380D0 splits as `frame = fistp(t − 0.5)`, the fraction the rest, and
    /// the draw (0x43E2FD…0x43E35B) blends each vertex `A + (B − A)·frac` between that frame and
    /// the next — the frame alone where there is no next. Past the last frame the part keeps it.
    pub fn set_roof(&mut self, r: &mut Renderer, state: i32) {
        if self.anim_length[1] == 0 || state == self.roof_state {
            return;
        }
        self.roof_state = state;
        let t = self.anim_length[1] as f32 * state as f32 * (1.0 / 255.0);
        self.set_anim(r, 1, t);
    }

    /// The roof state [`set_roof`](Self::set_roof) last took — what the front end's car object
    /// keeps at `+4` and 0x4D19E0(1) answers.
    pub fn roof_state(&self) -> i32 {
        self.roof_state
    }

    /// `0x438190(n)`: animation `n`'s length in frames, 0 when the car has none.
    pub fn anim_length(&self, n: usize) -> u32 {
        self.anim_length.get(n).copied().unwrap_or(0)
    }

    /// **0x4380D0(n, t)**: animation `n` to time `t` frames, clamped to `[0, length]`.
    pub fn set_anim(&mut self, r: &mut Renderer, n: usize, t: f32) {
        let len = self.anim_length(n);
        if len == 0 {
            return;
        }
        let t = t.clamp(0.0, len as f32);
        if t == self.anim_time[n] {
            return;
        }
        self.anim_time[n] = t;
        let frame = (t - 0.5).round_ties_even().max(0.0) as usize;
        let frac = t - frame as f32;
        self.blend(r, n, frame, frame + 1, frac);
    }

    /// Write animation `n`'s `{frame, next, frac}` straight into its record, as slot 9's keyframe
    /// copies do (0x439035…0x43908D for the right arm, 0x4392F1…0x43933C for the body): the draw
    /// (0x43E2FD…0x43E35B) blends each vertex from `frame` toward `next` by `frac`.
    fn set_anim_keys(&mut self, r: &mut Renderer, n: usize, frame: u32, next: u32, frac: f32) {
        if self.anim_length(n) == 0 {
            return;
        }
        // a key write is not a time: forget the last one so a later 0x4380D0 re-uploads
        self.anim_time[n] = -1.0;
        self.blend(r, n, frame as usize, next as usize, frac);
    }

    /// The draw's blend of animation `n`'s parts (0x43E2FD…0x43E35B). Each part keeps the vertices it
    /// last queued: the positions are blended in place (the same `A + (B − A)·frac` per vertex, the
    /// other fields are the frame-0 ones, which nothing rewrites) and the mesh is rewritten only when
    /// a position changed bit for bit or someone else (the crush, another instance sharing the mesh)
    /// has written it since — a write of the bytes the mesh already holds is not made.
    fn blend(&mut self, r: &mut Renderer, n: usize, frame: usize, next: usize, frac: f32) {
        if self.blended[n] == Some((frame, next, frac)) {
            return;
        }
        self.blended[n] = Some((frame, next, frac));
        for p in &mut self.morph[n] {
            let Some(a) = p.frames.get(frame.min(p.frames.len() - 1)) else { continue };
            let b = p.frames.get(next);
            let frac = if frame >= p.frames.len() { 0.0 } else { frac };
            let mut changed = p.seen != r.mesh_writes(p.mesh);
            for (i, out) in p.scratch.iter_mut().enumerate() {
                let pa = Vec3::from(a[i]);
                let position = match b {
                    Some(b) => (pa + (Vec3::from(b[i]) - pa) * frac).to_array(),
                    None => a[i],
                };
                if position.map(f32::to_bits) != out.position.map(f32::to_bits) {
                    out.position = position;
                    changed = true;
                }
            }
            if changed {
                r.update_mesh_vertices(p.mesh, &p.scratch);
                p.seen = r.mesh_writes(p.mesh);
            }
        }
    }
}

/// A render object's light block, `obj+0x2F4` (`RE/notes/lighting.md` §6): seven colour slots —
/// slot 3 is the direction — and the material colour at `+0x74` that scales the directional half.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightBlock {
    /// slots 0, 1, 2, (3 unused here), 4, 5, 6 — rgb, the alpha the original carries is 1
    pub colours: [[f32; 3]; 7],
    /// slot 3, the way the light travels, in world space
    pub direction: Vec3,
    /// `+0x74`, rgb
    pub material: [f32; 3],
}

impl LightBlock {
    /// The 3D room's block, 0x4E84C0: with `flag` clear (the room's own models) slots 0–2 are the
    /// rig's ambient and 4–6 its sun colour, `/ 255` and faded, and the direction the rig's sun
    /// direction normalised (0x4E876D…0x4E87D7); with `flag` set (the car, 0x4EA08A / 0x4E8CB2)
    /// they are 0.3 and 0.7 grey, faded, and the direction (0, −0.707, 0.707) (0x4E85FE…). `+0x74`
    /// is white either way (0x4E8513). `fade` is `1 − progress / 512` (0x4E84DE).
    pub fn room(ambient: [f32; 3], sun: [f32; 3], sun_dir: Vec3, fade: f32, flag: bool) -> Self {
        let (a, s, dir) = if flag {
            ([0.3 * fade; 3], [0.7 * fade; 3], Vec3::new(0.0, f32::from_bits(0xbf34fdf4), f32::from_bits(0x3f34fdf4)))
        } else {
            (ambient.map(|c| c * fade / 255.0), sun.map(|c| c * fade / 255.0), sun_dir.normalize_or_zero())
        };
        Self { colours: [a, a, a, [0.0; 3], s, s, s], direction: dir, material: [1.0; 3] }
    }

    /// What `R3DLightLayer` 0x446530 makes of a material's `{p0, p1, slot}`: the slot's colour
    /// × p0, and its `slot + 4` partner × `+0x74` × p1 — nothing directional when `p1 <= 0`
    /// (0x446724). A slot past 2 would read the direction as a colour; none ships.
    pub fn layer_light(&self, l: nfs_formats::crp::LayerLight) -> Option<LayerLight> {
        let s = l.slot as usize;
        if s > 2 {
            return None;
        }
        let a = self.colours[s].map(|c| c * l.p0);
        let d = if l.p1 > 0.0 { [0, 1, 2].map(|k| self.colours[s + 4][k] * self.material[k] * l.p1) } else { [0.0; 3] };
        Some(LayerLight { ambient: a, diffuse: d, direction: self.direction.to_array() })
    }
}

impl CarModel {
    /// `draw_items` lit through each material's `R3DLightLayer` from `block`.
    /// `R3DShadowMapTextureLayer` for this car's items: the projector's planes on every item whose
    /// material carries the layer (0x440920 / 0x440910 set it per car at 0x43A70E).
    pub fn apply_projector(&self, items: &mut [DrawItem], planes: Option<([f32; 4], [f32; 4])>) {
        for it in items {
            it.shadow = if self.shadowed.contains(&it.material) { planes } else { None };
        }
    }

    /// `R3DSkyMapTextureLayer` for this car's items (`TrackWorld::sky_map`); the renderer draws
    /// the pass only on the items whose material has it (`MaterialDesc::sky_map`).
    pub fn apply_sky(&self, items: &mut [DrawItem], sky: Option<nfs_renderer::SkyMap>) {
        for it in items {
            it.sky = sky;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw_items_block(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2], travel: [f32; 4], passes: &[CarPass], block: &LightBlock) -> Vec<DrawItem> {
        let mut out = self.draw_items(car_to_world, wheel_spin, steer, travel, passes);
        for it in &mut out {
            it.layer_light = self.material_light.get(&it.material).and_then(|l| block.layer_light(*l));
        }
        out
    }
}
