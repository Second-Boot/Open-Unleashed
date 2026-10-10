//! Sky: the layers `Track/Sky/<Track>.txt` asks for, on a dome (`SkyLevel_*`, RE/RENDERING.md).
//!
//! The layer list, the textures, their UV repeats, the wind and the warp all come from the track's
//! own sky file now (`nfs_formats::sky`): the block of the `SKYRENDERING` level is taken, and each
//! stage is the grid below with its **cloud** as the texture (`flat`: none) and its **sky map** —
//! region A's, blended towards region B's by `t` — as the 16 × 16 vertices' colours (`MapBlend`),
//! blended into the frame unless 0x45BBA5's test calls it opaque (the shipped tracks are all
//! `MULTINORMAL` or `basic`). The `horz` horizon strips keep their ring.
//!
//! The **horizon ring is the original's own** now (`RE/RENDERING.md`, sky `.bin`): sixteen quads on
//! a circle of 400 m (0x459F92) from the `.bin`'s `+0x04` turn, spanning `+0x14` to
//! `+0x14 + +0x18` in y, each showing the tile of the `horz` grid the `.bin` names for it (mirrored
//! where it says so) and tinted per corner from the `hzcl` 16 × 2 strip, rotated by `+0x08`. The whole sky is drawn in metres around the eye and clipped at the far plane
//! 700 (0x45D4F0).
//!
//! **The layers are the original's grid** now (`RE/RENDERING.md`, "The sky layers are a curved
//! grid"): a 16 × 16 vertex cap built by 0x460F40 from region A's `.bin` record, each vertex
//! `x = (c·v + s·u)·S, y = (h − k(u² + v²))·S, z = (c·u − s·v)·S`, its UV `(j/a, i/b)` from the
//! layer's two numbers (0x45EC40), and the offset moved either by the layer's wind —
//! `speed · 0.00025 · (sin θ, cos θ)` per 32 Hz step, `θ = −direction/360` turns, when the speed
//! is over 0.25 — or, when the region's move ratio is over 1, by the camera turned into the
//! grid's frame, `(−frac(x/(a·ratio)), frac(z/(b·ratio)))` (0x45ED40), all wrapped into [0, 1).
//! A track without a `.bin` keeps the old dome.
//!
//! Every grid vertex's UV also **wanders** by the region's warp (0x45EE80, [`Jitter`]): one
//! position and velocity per vertex, shared by all the layers, accelerating towards a random target
//! within `±warpAmount` that is redrawn every 32 steps.
//!
//! The jitter draws from the front end's generator 0x538040 (`opponents::FeRng`, through 0x5561D0
//! at 0x45EEA1 / 0x45F3D3) — the game's **one** state, which the caller hands in: the race's, seeded
//! from `time(0)` by the HUD's init and shared with the weather and the announcer (`App::fe_rng` in
//! `porsche`). *(Until 2026-09-26 it drew from an LCG of the port's own, then from a state of its
//! own starting at the image's `.data` values.)* Not ported: `basic`'s `noise` alpha maps
//! (0x45E680, folded into the colours' alpha by 0x45E27D…; no shipped block names one).
//!
//! **Lightning** (2026-09-27, [`Lightning`], [`Flash`]): track B's field 6 strikes a cell of the
//! 16 × 16 grid now and then (Autobahn, Industrial), and while it lasts every colour stream runs a
//! flash — bolts that brighten the vertices around the cell up to the `ltmp` map's colours — in
//! place of the region blend (0x45CCF0, 0x45CD90, 0x45DF50, 0x45E3D0, 0x45E550). One strike in
//! ten calls the thunder 0x4A8340 ([`ThunderCall`], taken by [`Sky::take_thunder`]; the sound is
//! `nfs_audio::ambience::Thunder`).

use glam::Vec3;
use nfs_assets::track::TrackAssets;
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, Vertex};

pub struct Sky {
    /// mesh, material, how the layer's UVs move, and whether the part draws after the sun
    parts: Vec<(MeshId, MaterialId, Motion, bool)>,
    grids: Vec<GridLayer>,
    /// the `.bin`'s per-slice blend between the regions (track A, 0x45A350 at 0x45D077)
    blend: Vec<(i32, f32)>,
    /// `gGameTick` of the last update (`[this+0x5C]`, 0x45D038)
    last_tick: Option<u64>,
    /// the per-vertex UV wander of the layers' UV object (0x45EE80)
    jitter: Jitter,
    /// `[0x627E38]` (none without a `.bin`)
    lightning: Option<Lightning>,
    /// the `.bin`'s `+0x04`, the turn the horizon ring starts at (0x45A1BD)
    ring_turn: f32,
    /// the `.bin`'s `+0x655C` byte track (0x45A970, `[this+0x4C]`)
    track_c: Vec<(i32, u8)>,
    /// `[this+0x50]`: the slice of the last pass's eye (0x45D598)
    slice: i32,
    /// `[this+0x38…+0x40]`: the last pass's eye (0x45D562…0x45D579)
    eye: Vec3,
}

/// 0x45A9E0(slice) over the `+0x655C` track (0x45A970): the byte of the key pair `b`, `b + 1` with
/// `key[b] ≤ slice < key[b + 1]`, searched from 0 when the pair it kept does not hold, and the last
/// key when none does. With fewer than two keys 0x45A970 writes two zero ones (slices 0 and 4,
/// 0x45A9A9…0x45A9BE), so the answer is 0. On the shipped files only **Forest** sets any byte
/// (1 at 697, 753, 754, 927, 928, 1811, 1946, …).
fn track_c_flag(keys: &[(i32, u8)], slice: i32) -> bool {
    let n = keys.len();
    if n < 2 {
        return false;
    }
    let b = (0..n - 1).find(|&k| keys[k].0 <= slice && keys[k + 1].0 > slice).unwrap_or(n - 1);
    keys[b].1 != 0
}

/// The view the sky pass 0x45D4F0 draws for: the eye, the camera's forward row (`view + 0xB8 +
/// 0x30`'s third row, `[esp+0x74…0x7C]`) and up — the pass's copy of the view's 3 × 3 rows — and
/// whether it is the main race view (`+0x12C` 2, 0x468BEF) rather than the mirror's (1, 0x468D2F).
#[derive(Debug, Clone, Copy)]
pub struct SkyView {
    pub eye: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    pub main: bool,
}

impl SkyView {
    /// 0x45D59D…0x45D63D: the heading in turns the ring's draw culls by. The forward row with its
    /// y cleared — kept only when `|z| ≤ 0.001` and `x ≤ 0.001` — against `(0, 0, 1)` through
    /// 0x532AE0 (both normalised by 0x5328D0, the dot 0x532910, `acos` 0x5A1060, × `1/2π`
    /// `[0x5BB2F4]`), taken as `1 − a` when x is negative: 0 looking down +z, ¼ down +x.
    pub fn heading(&self) -> f32 {
        let f = self.forward;
        let keep_y = !(f.z > 0.001) && !(f.z < -0.001) && !(f.x > 0.001);
        let v = Vec3::new(f.x, if keep_y { f.y } else { 0.0 }, f.z);
        let len = (v.x as f64 * v.x as f64 + v.y as f64 * v.y as f64 + v.z as f64 * v.z as f64).sqrt();
        let nz = (v.z as f64 * (1.0 / len)) as f32;
        // det-allow: sky drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
        let a = ((nz as f64).acos() * f32::from_bits(0x3E22_F984) as f64) as f32;
        if f.x < 0.0 { 1.0 - a } else { a }
    }

    /// `m[1][1]`, the y of the view's up row (`[esp+0x6C]` of the pass's copy): the camera's up made
    /// square to its forward (`right = up × forward`, `up' = forward × right`, as the weather's view).
    pub fn up_row_y(&self) -> f32 {
        let f = self.forward.normalize_or_zero();
        let r = self.up.cross(f).normalize_or_zero();
        f.cross(r).y
    }
}

/// 0x45A1A0, the ring's draw: the quads `lo .. hi` of its 32, `h` the heading past the ring's
/// start turn (a negative one taken a turn on), `lo = fistp((h − 0.125)·16) − 1`, `hi =
/// fistp((h + 0.125)·16) + 1`, both moved on by 16 when `lo` is negative — six or seven quads
/// around the heading, the 0.25 turn the view can see widened by a quad each side. The product is
/// stored as a float before the `fistp` (0x45A1EF / 0x45A20D), which rounds to nearest even.
fn ring_window(heading: f32, ring_turn: f32) -> (i32, i32) {
    let mut h = heading - ring_turn;
    if h < 0.0 {
        h += 1.0;
    }
    let lo = (((h as f64 - 0.125) * 16.0) as f32).round_ties_even() as i32 - 1;
    let hi = (((h as f64 + 0.125) * 16.0) as f32).round_ties_even() as i32 + 1;
    if lo < 0 { (lo + 16, hi + 16) } else { (lo, hi) }
}

/// A strike's call of the thunder **0x4A8340**(`0x5CF104[mode]`, `fistp(turn · 65535)`) at 0x45CE5C,
/// with the `0x538040` random that function draws first (its `% 9` picks the patch). The audio side
/// (`nfs_audio::ambience::Thunder`) does the rest; only the arguments and the draw are made here, in
/// the strike's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThunderCall {
    /// `0x5CF104[mode]` = 0x28 / 0x50 / 0x7E
    pub lead: i32,
    /// the pan angle, `fistp` of the f32 `turn · 65535.0` (`[0x5B3ED0]`), 0…65535
    pub pan: i32,
    /// 0x538040's answer
    pub draw: u32,
}

/// `[0x5CF104]`: the thunder's lead per lightning mode (0x45CE54).
const THUNDER_LEAD: [i32; 3] = [0x28, 0x50, 0x7E];

/// The UV object's jitter (0x45E8E0 `+0x5C…+0x6C`, `+0x130…+0x16C`; 0x45EE80, 0x45F3C0): one
/// `(u, v)` position, target and velocity per grid vertex, the same for every stage.
///
/// ```text
/// every update (ticks = 32 Hz steps), when either region's warpSpeed > 0.01 (+0x74):
///     refill the 13 randoms (0x5561D0: rand & 0xFFFF over 65536)
///     acc = (4·ticks) · rate · ticks · 1/30            ; rate = warpSpeed/180 lerped by t (+0x84)
///     every 32 ticks: each target.{u,v} = 2·amp·r − amp, r the next of randoms 0…11 (+0x16C)
///     every 4 ticks:  each vel.{u,v} ±= acc towards the target, held within ±rate
///     each pos += ticks · vel
/// ```
/// `amp` is warpAmount lerped (`+0x90`). At creation the positions take `2·amp·r − amp` with
/// region A's numbers, the velocities and targets 0, the counters 32 and 4, then one step.
#[derive(Debug, Clone)]
struct Jitter {
    on: bool,
    /// region A's and B's `warpSpeed / 180` (`+0x7C` / `+0x80`) and `warpAmount` (`+0x88` / `+0x8C`)
    rate: [f32; 2],
    amp: [f32; 2],
    table: [f32; 13],
    next: usize,
    retarget: u32,
    accelerate: u32,
    pos: Vec<[f32; 2]>,
    target: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
}

impl Jitter {
    const VERTICES: usize = 16 * 16;

    /// A sky with no UV object: still, and nothing drawn.
    fn off() -> Self {
        Self {
            on: false,
            rate: [0.0; 2],
            amp: [0.0; 2],
            table: [0.0; 13],
            next: 0,
            retarget: 0x20,
            accelerate: 4,
            pos: vec![[0.0; 2]; Self::VERTICES],
            target: vec![[0.0; 2]; Self::VERTICES],
            vel: vec![[0.0; 2]; Self::VERTICES],
        }
    }

    fn new(speed: [f32; 2], amount: [f32; 2], fe: &mut crate::opponents::FeRng) -> Self {
        let mut j = Self {
            on: speed[0] > 0.01 || speed[1] > 0.01,
            rate: [speed[0] / 180.0, speed[1] / 180.0],
            amp: amount,
            table: [0.0; 13],
            next: 0,
            retarget: 0x20,
            accelerate: 4,
            pos: vec![[0.0; 2]; Self::VERTICES],
            target: vec![[0.0; 2]; Self::VERTICES],
            vel: vec![[0.0; 2]; Self::VERTICES],
        };
        // 0x45F3C0, with the numbers 0x45F1B0 copied from region A (0x45F366…0x45F375)
        let (rate, amp) = (j.rate[0], j.amp[0]);
        j.refill(fe);
        for i in 0..Self::VERTICES {
            let u = j.draw(amp);
            let v = j.draw(amp);
            j.pos[i] = [u, v];
        }
        j.step(1, rate, amp, fe);
        j
    }

    /// The thirteen randoms, each 0x5561D0 = `(0x538040() & 0xFFFF) / 65536` (0x45EEA1 / 0x45F3D3).
    fn refill(&mut self, fe: &mut crate::opponents::FeRng) {
        for k in 0..13 {
            self.table[k] = (fe.next_u32() & 0xFFFF) as f32 * (1.0 / 65536.0);
        }
    }

    /// The next random of the twelve the index cycles through, as `2·amp·r − amp`.
    fn draw(&mut self, amp: f32) -> f32 {
        self.next += 1;
        if self.next == 12 {
            self.next = 0;
        }
        self.table[self.next] * amp * 2.0 - amp
    }

    /// 0x45EE80(ticks) with the lerped rate and amplitude.
    fn step(&mut self, ticks: u32, rate: f32, amp: f32, fe: &mut crate::opponents::FeRng) {
        if !self.on {
            return;
        }
        self.refill(fe);
        let t = ticks as f32;
        let acc = (4 * ticks) as f32 * rate * t * (1.0 / 30.0);
        self.retarget += ticks;
        self.accelerate += ticks;
        if self.retarget >= 0x20 {
            self.retarget -= 0x20;
            for i in 0..Self::VERTICES {
                let u = self.draw(amp);
                let v = self.draw(amp);
                self.target[i] = [u, v];
            }
        }
        if self.accelerate >= 4 {
            self.accelerate -= 4;
            for i in 0..Self::VERTICES {
                for c in 0..2 {
                    let vel = &mut self.vel[i][c];
                    if self.target[i][c] < self.pos[i][c] {
                        *vel = (*vel - acc).max(-rate);
                    } else {
                        *vel = (*vel + acc).min(rate);
                    }
                }
            }
        }
        for (p, v) in self.pos.iter_mut().zip(&self.vel) {
            p[0] += t * v[0];
            p[1] += t * v[1];
        }
    }
}

/// How a layer's UV offset moves (0x45ED40).
#[derive(Debug, Clone, Copy)]
enum Motion {
    /// a grid layer: index into `Sky::grids`
    Grid(usize),
    /// quad `q` (0…31) of the horizon ring
    Ring(i32),
}

/// One region's numbers for a grid stage (0x45F1B0): the UV step `(1/a, 1/b)`, the wind per 32 Hz
/// step and `1/moveRatio`.
#[derive(Debug, Clone, Copy, Default)]
struct StageParams {
    step: [f32; 2],
    wind: [f32; 2],
    inv_ratio: f32,
}

impl StageParams {
    fn of(layer: Option<&nfs_formats::sky::SkyLayer>, move_ratio: f32) -> Self {
        let Some(l) = layer else { return Self::default() };
        let th = (-l.wind_direction / 360.0) as f64 * 6.28318452835083;
        let k = l.wind_speed * 0.00025;
        let inv = |x: f32| if x != 0.0 { 1.0 / x } else { 0.0 };
        // det-allow: sky drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
        Self { step: [inv(l.scale[0]), inv(l.scale[1])], wind: [th.sin() as f32 * k, th.cos() as f32 * k], inv_ratio: inv(move_ratio) }
    }

    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * t;
        Self {
            step: [l(a.step[0], b.step[0]), l(a.step[1], b.step[1])],
            wind: [l(a.wind[0], b.wind[0]), l(a.wind[1], b.wind[1])],
            inv_ratio: l(a.inv_ratio, b.inv_ratio),
        }
    }
}

/// A layer drawn as the grid: its dynamic mesh, both regions' numbers and the running offset.
struct GridLayer {
    mesh: MeshId,
    vertices: Vec<Vertex>,
    a: StageParams,
    b: StageParams,
    /// `+0x70+k`: either region's wind is over 0.25
    wind_on: bool,
    /// `+0x73`: either region's move ratio is over 1
    parallax_on: bool,
    angle: f32,
    step: [f32; 2],
    offset: [f32; 2],
    /// The layer's colour stream: its sky maps as one colour per grid vertex.
    colours: MapBlend,
}

/// The 0x45DD90 object of a stage (MultiSky 0x45B7FA, BasicSky 0x45AFCB): region A's and B's
/// sky maps, each a 16 × 16 ARGB8888 entry read in place — 0x45E640(16, entry) keeps `entry + 0x10`,
/// the pixels right after the SHPI entry header, and resamples nothing — and a 16 × 16 array of
/// dwords at `+0x14` which is the layer's **vertex colour** stream (0x45BB66 / 0x45B21B → the
/// helper's `LayerColours`, 0x4685A8), one texel per vertex of the 16 × 16 grid. It starts as A's
/// copy (0x45DDEB) and 0x45E160 rewrites it at every `t` as `(A·wa + B·wb) >> 16` per byte with
/// `wa = fistp((1 − t)·65536)`, `wb = fistp(t·65536)` (0x45E188…0x45E25D) — skipped when A and B
/// are one entry (0x45E1B2…0x45E1C8). The file's bytes are `B, G, R, A`, the dword a D3DCOLOR,
/// so the vertex colour is the texel's own RGBA; the blend is per byte, so its order is moot.
struct MapBlend {
    a: Vec<u8>,
    /// region B's map when it is another entry
    b: Option<Vec<u8>>,
    /// the `(wa, wb)` last written, so an unchanged `t` costs nothing
    w: (i32, i32),
    /// `+0x14`: the stream itself — what the vertices show (A's copy at first, 0x45DDEB)
    stream: Vec<u8>,
    /// `[+0]`: the lightning flash over this stream (0x45DDFE…0x45DF0A)
    flash: Flash,
}

impl MapBlend {
    /// The colour of vertex `k` (row `k / 16`, column `k % 16`) from the stream's bytes.
    fn colour(px: &[u8], k: usize) -> [f32; 4] {
        let o = k * 4;
        [px[o] as f32 / 255.0, px[o + 1] as f32 / 255.0, px[o + 2] as f32 / 255.0, px[o + 3] as f32 / 255.0]
    }

    /// 0x45E5E0: every one of the 16 × 16 alpha bytes is 0xFF.
    fn opaque(px: &[u8]) -> bool {
        px.chunks_exact(4).all(|p| p[3] == 0xFF)
    }

    /// 0x45E110 for a stage without noise objects: A's test when A and B are one entry, else
    /// both maps'.
    fn is_opaque(&self) -> bool {
        Self::opaque(&self.a) && self.b.as_deref().is_none_or(Self::opaque)
    }
}

/// 0x45E160's blend of two maps' bytes at `t`: returns `(wa, wb)` and the pixels.
fn blend_maps(a: &[u8], b: &[u8], t: f32) -> ((i32, i32), Vec<u8>) {
    let wa = ((1.0 - t) * 65536.0).round_ties_even() as i32;
    let wb = (t * 65536.0).round_ties_even() as i32;
    let px = a.iter().zip(b).map(|(&a, &b)| ((a as i32 * wa + b as i32 * wb) >> 16) as u8).collect();
    ((wa, wb), px)
}

/// 0x5561D0: `(0x538040() & 0xFFFF) / 65536`.
fn rand01(fe: &mut crate::opponents::FeRng) -> f32 {
    (fe.next_u32() & 0xFFFF) as f32 * (1.0 / 65536.0)
}

/// The flash kinds at 0x5CF018, twenty bytes each: the countdown `+0x1910`, the bolt count
/// `+0x191C`, the spread `+0x1920` (a float, its bits here), the peak `+0x1924` and the bolts
/// started per tick `+0x1928`.
const FLASH_KINDS: [(i32, i32, u32, i32, i32); 5] = [
    (90, 35, 0x3F51_EB85, 255, 6),
    (90, 8, 0x3F2E_147B, 255, 5),
    (90, 80, 0x3E99_999A, 255, 5),
    (90, 8, 0x3E85_1EB8, 255, 5),
    (90, 33, 0x3DE1_47AE, 255, 18),
];
/// Per lightning mode, the three kinds 0x45CEF0 picks from (the lists at 0x5CF07C / 0x5CF088 /
/// 0x5CF094, through `[0x5CF0A0 + 4·mode]`).
const FLASH_LISTS: [[usize; 3]; 3] = [[0, 0, 1], [1, 1, 2], [2, 3, 4]];
/// Per mode, the 32 cell offsets a bolt lands at around the strike (0x5CF144 + 0x80·mode).
const FLASH_OFFSETS: [[i32; 32]; 3] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 0, 0, 0, 0, 0, 0, 0, 0, -1, -1, -1, -1, -1, -1, -2, -2],
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, -1, -1, -1, -1, 0, 0, 0],
    [0; 32],
];

/// One of a flash's 200 bolts (0x20 bytes from the flash object's `+0`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bolt {
    /// `+0x00`: the peak it climbs to
    peak: i32,
    /// `+0x04`: how far it has climbed; below 0 idle
    level: i32,
    /// `+0x08`: the stream entry it brightens (`None`: outside the 16 × 16 stream, see [`Flash::step`])
    at: Option<usize>,
    /// `+0x0C`: the climb per step, negated at the peak
    step: i32,
    /// `+0x10…+0x1C`: the `ltmp` record `{A, R, G, B}` capping the entry's channels
    cap: [i32; 4],
}

/// The lightning flash on one colour stream (0x45DD90's `[+0]`, 0x1930 bytes; 0x45DF50, 0x45E3D0,
/// 0x45E550). While it runs the stream is its saved copy (`+0x18`) with the bolts added on top,
/// every update; the region blend (0x45E160) waits until it ends.
#[derive(Debug, Clone)]
struct Flash {
    /// `+0x1918`
    active: bool,
    /// `+0x1908`, `+0x190C`: the strike's cell (column, row)
    x: i32,
    z: i32,
    /// `+0x1910`, `+0x191C`, `+0x1920`, `+0x1924`, `+0x1928` — one of [`FLASH_KINDS`]
    countdown: i32,
    count: i32,
    spread: f32,
    peak: i32,
    rate: i32,
    /// `+0x192C`: the lightning's mode at the strike
    mode: usize,
    bolts: Vec<Bolt>,
    /// `+0x18`: the stream as it was when the flash began
    saved: Vec<u8>,
}

impl Flash {
    const BOLTS: usize = 200;

    /// 0x45DDFE…0x45DF0A: countdown 0, 80 bolts, spread 0.3, peak 255, rate 5, and every bolt set
    /// up as a strike would (two draws each).
    fn new(fe: &mut crate::opponents::FeRng) -> Self {
        let mut f = Self { active: false, x: 0, z: 0, countdown: 0, count: 0x50, spread: f32::from_bits(0x3E99_999A), peak: 0xFF, rate: 5, mode: 0, bolts: Vec::new(), saved: Vec::new() };
        f.arm(fe);
        f
    }

    /// The 200 bolts' set-up (0x45DE68…0x45DF04, again at 0x45E047…0x45E0E3): `a = r·peak`,
    /// `b = max(r·countdown·spread, 2)`, the peak `fistp(a)` and the step `fistp(max(2a/b, 1))` —
    /// a bolt climbs to its peak and back in about `b` steps. Idle, uncapped, nowhere.
    fn arm(&mut self, fe: &mut crate::opponents::FeRng) {
        self.bolts.clear();
        for _ in 0..Self::BOLTS {
            let a = (rand01(fe) as f64 * self.peak as f64) as f32;
            let mut b = rand01(fe) as f64 * self.countdown as f64 * self.spread as f64;
            if b < 2.0 {
                b = 2.0;
            }
            let mut c = (2.0 * a as f64 / b) as f32;
            if c < 1.0 {
                c = 1.0;
            }
            self.bolts.push(Bolt { peak: a.round_ties_even() as i32, level: -1, at: None, step: c.round_ties_even() as i32, cap: [0xFF; 4] });
        }
    }

    /// 0x45E3D0(ticks) on `stream` (RGBA bytes, 16 × 16): one countdown tick, then each of the
    /// kind's bolts — every bolt counted done while the lightning is off (its level set to 0), a
    /// running one climbing (0x45E550), an idle one started while fewer than `rate · ticks` have
    /// started this update and the countdown lasts: at the strike's cell moved by two of the
    /// mode's offsets (`0x538040 & 31`, redrawn while the column is negative), capped by the
    /// `ltmp` record `7·dx + dz + 24`, from level 0, its step made positive. When every bolt
    /// counted done the flash ends.
    fn step(&mut self, ticks: u32, lightning: &Lightning, stream: &mut [u8], fe: &mut crate::opponents::FeRng) {
        if !self.active {
            return;
        }
        self.countdown -= 1;
        let (mut done, mut started) = (0, 0i32);
        let side = 16;
        for k in 0..self.count.max(0) as usize {
            let Some(bolt) = self.bolts.get_mut(k) else { break };
            if !lightning.on {
                bolt.level = 0;
                done += 1;
                continue;
            }
            if bolt.level >= 0 {
                bolt.climb(ticks, stream);
                continue;
            }
            if started >= self.rate.wrapping_mul(ticks as i32) || self.countdown <= 0 {
                done += 1;
                continue;
            }
            started += 1;
            let offsets = &FLASH_OFFSETS[self.mode.min(2)];
            // 0x45E4A5…0x45E4F2: only a negative column is drawn again — the row and a column
            // past the side are taken as they are
            let (dx, dz, x, z) = loop {
                let dx = offsets[(fe.next_u32() & 0x1F) as usize];
                let dz = offsets[(fe.next_u32() & 0x1F) as usize];
                let (x, z) = (self.x + dx, self.z + dz);
                if x >= 0 {
                    break (dx, dz, x, z);
                }
            };
            // `stream + 4·(side·z + x)`. No bolt reaches past the side or out of the 256 entries:
            // the strike's cell is `(½ ∓ d·sin|cos θ / side)·15` truncated, `d/side` below
            // `½·(0.2·65535/65536 + 0.4)` < 0.3 (0x45CDE6…0x45CE84), so each coordinate lies in
            // 3…11, and mode 0's offsets (the widest, ±2, 0x5CF144) keep a bolt in 1…13 — entries
            // 17…221. (The original would write into the heap past the stream's `malloc(0x400)`,
            // 0x45DDC4; the guard below only keeps the port safe.)
            let i = side * z + x;
            bolt.at = (0..256).contains(&i).then_some(i as usize);
            bolt.cap = lightning.ltmp[(7 * dx + dz + 24) as usize];
            bolt.level = 0;
            if bolt.step < 0 {
                bolt.step = -bolt.step;
            }
        }
        if done >= self.count {
            self.active = false;
        }
    }
}

impl Bolt {
    /// 0x45E550(ticks): turn at the peak, climb, and while the level is not negative add
    /// `level · ticks` to the entry's R, G and B, each held to the `ltmp` cap (which may lower a
    /// channel brighter than it); the alpha stays.
    fn climb(&mut self, ticks: u32, stream: &mut [u8]) {
        if self.level < 0 {
            return;
        }
        if self.level >= self.peak && self.step > 0 {
            self.step = -self.step;
        }
        self.level += self.step;
        if self.level < 0 {
            return;
        }
        let v = self.level.wrapping_mul(ticks as i32);
        let Some(i) = self.at else { return };
        let px = &mut stream[4 * i..4 * i + 4];
        for (c, cap) in [(0, self.cap[1]), (1, self.cap[2]), (2, self.cap[3])] {
            px[c] = (px[c] as i32 + v).min(cap) as u8;
        }
    }
}

/// The sky's lightning (0x45CC40, `[0x627E38]` with the `ltmp` table `[0x627E3C]`), made with the
/// sky (0x45D856) and stepped after its levels every update (0x45D15C…0x45D189). `RE/RENDERING.md`,
/// "Lightning".
#[derive(Debug, Clone)]
struct Lightning {
    /// `+0x00`
    on: bool,
    /// `+0x04`: the ticks summed
    time: u32,
    /// `+0x0C`: when the strike ends
    end: u32,
    /// `+0x10`
    mode: usize,
    /// `+0x1C`, `+0x20`: the strike's cell on the 16 × 16 stream, 0…15
    cell: [f32; 2],
    /// `[[+0x18]+0x104]+0x40`: region A's grid side (`SkyRing::radius`)
    side: f32,
    /// 0x45CB80: the sky pack's `ltmp`, 49 `{A, R, G, B}` from its first 49 texels; all 255
    /// without one (only `Autobahn.fsh` and `Industrial.fsh` carry it, 7 × 7 `0x7D`)
    ltmp: [[i32; 4]; 49],
    /// track B, whose field 6 drives it (0x45A910)
    bin: nfs_formats::sky::SkyBin,
    /// the last strike's thunder call, until the audio update takes it
    thunder: Option<ThunderCall>,
}

impl Lightning {
    fn new(bin: &nfs_formats::sky::SkyBin, ltmp: Option<&nfs_assets::texture::TextureImage>) -> Self {
        let mut t = [[0xFF; 4]; 49];
        // 0x460B50("ltmp") → entry + 0x10, read as 49 dwords whatever the entry's size
        if let Some(px) = ltmp.and_then(|t| t.mips.first()).filter(|p| p.len() >= 49 * 4) {
            for (k, e) in t.iter_mut().enumerate() {
                let p = &px[4 * k..4 * k + 4];
                *e = [p[3] as i32, p[0] as i32, p[1] as i32, p[2] as i32];
            }
        }
        Self { on: false, time: 0, end: u32::MAX, mode: 0, cell: [0.0; 2], side: bin.rings[0].radius, ltmp: t, bin: bin.clone(), thunder: None }
    }

    /// 0x45CCF0(value, ticks): the clock moves on; a strike ends once it passes its end; with none
    /// on, a value of 0.04 or more picks the mode (under 0.33 → 0, under 0.66 → 1, else 2) and
    /// strikes when `ticks · {1/30, 1/15, 1/10}[mode]` beats a random.
    fn update(&mut self, value: f32, ticks: u32, fe: &mut crate::opponents::FeRng) {
        self.time = self.time.wrapping_add(ticks);
        if self.on {
            if self.time > self.end {
                self.on = false;
            }
            return;
        }
        if value < 0.04 {
            return;
        }
        self.mode = if value < 0.33 {
            0
        } else if value < 0.66 {
            1
        } else {
            2
        };
        let r = rand01(fe);
        let p = [1.0f32 / 30.0, 1.0 / 15.0, 0.1][self.mode];
        if (ticks as f64 * p as f64) > r as f64 {
            self.strike(fe);
        }
    }

    /// 0x45CD90: lasts `fistp((r + 1)·60)` ticks; lands `(r·{0.2, 0.1, 0.1} + {0.4, 0.3, 0.2})·side/2`
    /// from the grid's centre at a random turn `θ` — the cell `((side/2 − sin θ·d)/side·15,
    /// (cos θ·d + side/2)/side·15)` held to 0…15 — and one time in ten (`r < 0.1`, `fcomp ; test
    /// ah, 1`) calls the thunder 0x4A8340 ([`ThunderCall`]), whose `0x538040` is drawn here.
    fn strike(&mut self, fe: &mut crate::opponents::FeRng) {
        let half = self.side * 0.5;
        let r = rand01(fe);
        self.end = self.time.wrapping_add(((r + 1.0) * 60.0).round_ties_even() as i32 as u32);
        let d = ((rand01(fe) * [0.2f32, 0.1, 0.1][self.mode] + [0.4f32, 0.3, 0.2][self.mode]) * half) as f64;
        let turn = rand01(fe);
        let a = turn as f64 * 6.28318452835083;
        // det-allow: sky drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
        let (s, c) = ((a.sin() * d) as f32, (a.cos() * d) as f32);
        // 0x45CE29…0x45CE38: `fstp` the f32 `turn · 65535`, then `fistp` (to nearest)
        let pan = (turn * 65535.0f32).round_ties_even() as i32;
        if rand01(fe) < 0.1 {
            // 0x4A8340(0x5CF104[mode], pan): it draws 0x538040 first (the patch `0x3DF + r % 9`)
            let draw = fe.next_u32();
            self.thunder = Some(ThunderCall { lead: THUNDER_LEAD[self.mode.min(2)], pan, draw });
            if forced_lightning() {
                tracing::info!(lead = THUNDER_LEAD[self.mode.min(2)], pan, draw, "lightning: thunder called");
            }
        }
        let clamp = |v: f32| if v >= 15.0 { 15.0 } else if v < 0.0 { 0.0 } else { v };
        self.cell = [clamp((half - s) / self.side * 15.0), clamp((c + half) / self.side * 15.0)];
        self.on = true;
    }

    /// 0x45CEF0: the strike's cell, and a flash kind from the mode's list at
    /// `fistp(trunc(3r) − 0.5)` — 0, 0 or 2, the `fistp` rounding 0.5 to 0.
    fn pick(&self, fe: &mut crate::opponents::FeRng) -> usize {
        let k = (rand01(fe) as f64 * 3.0) as i32;
        let i = ((k as f32) - 0.5).round_ties_even() as i32;
        FLASH_LISTS[self.mode.min(2)][i.clamp(0, 2) as usize]
    }
}

impl MapBlend {
    /// 0x45DF50(t, ticks): a running flash puts the saved stream back and steps; a lightning with
    /// no flash running starts one — the stream saved, the strike's cell truncated, the kind's
    /// numbers taken and every bolt set up again — and the stream stays as it is; otherwise the
    /// region blend (0x45E160). Answers whether the stream changed.
    fn update(&mut self, t: f32, ticks: u32, lightning: Option<&Lightning>, fe: &mut crate::opponents::FeRng) -> bool {
        if self.flash.active {
            self.stream.copy_from_slice(&self.flash.saved);
            if let Some(l) = lightning {
                self.flash.step(ticks, l, &mut self.stream, fe);
            }
            if !self.flash.active {
                // the blend below writes again once the flash is over
                self.w = (-1, -1);
            }
            return true;
        }
        if let Some(l) = lightning.filter(|l| l.on) {
            let kind = l.pick(fe);
            self.flash.saved.clone_from(&self.stream);
            let f = &mut self.flash;
            f.x = l.cell[0] as i32;
            f.z = l.cell[1] as i32;
            let (countdown, count, spread, peak, rate) = FLASH_KINDS[kind];
            (f.countdown, f.count, f.spread, f.peak, f.rate) = (countdown, count, f32::from_bits(spread), peak, rate);
            f.mode = l.mode;
            f.arm(fe);
            f.active = true;
            return false;
        }
        match self.b.as_deref() {
            Some(b) => {
                let (w, px) = blend_maps(&self.a, b, t);
                if w == self.w {
                    return false;
                }
                self.w = w;
                self.stream = px;
                true
            }
            None if self.w == (-1, -1) => {
                self.w = (65536, 0);
                true
            }
            None => false,
        }
    }
}

/// Whether 0x460D40 calls a texture alpha-free: 0x556690 puts its format code in class 3, 4 or 5 —
/// `0x03 / 0x42 / 0x7E` (1555), `0x21 / 0x2A / 0x78` (565), `0x04 / 0x43 / 0x7F` (888), read off
/// the tables at 0x556748 / 0x556774.
fn alpha_free(code: u8) -> bool {
    matches!(code, 0x03 | 0x42 | 0x7E | 0x21 | 0x2A | 0x78 | 0x04 | 0x43 | 0x7F)
}

/// A cloud name `flat`: 0x45B967 / 0x45B1BC compare four bytes, a prefix, on the lower-cased file.
fn is_flat(name: &str) -> bool {
    name.len() >= 4 && name[..4].eq_ignore_ascii_case("flat")
}

/// `NFS_SKY_LEVEL=<n>`: a diagnostic that draws the sky at `SKYRENDERING` level `n` (0 off / low,
/// 1 medium, 2 high) whatever the option says — the shipped option is 0, where no track has a
/// block, so the layers are otherwise never seen.
pub fn forced_level() -> Option<usize> {
    crate::config::cfg().nfs_sky_level.as_str()?.parse().ok()
}

/// `NFS_LIGHTNING=1`: a port diagnostic that drives the lightning's field value to 1 (mode 2).
pub fn forced_lightning() -> bool {
    crate::config::cfg().nfs_lightning.is_set()
}

/// The 16 × 16 layer grid of 0x460EC0(15, …) / 0x460F40 in metres around the eye, with the base UVs of 0x45EC40 (`j/a`, `i/b`) and the colour stream `colours` (vertex
/// `16·i + j` takes texel `16·i + j`), as the 15 × 15 quads' triangles.
fn layer_grid(ring: &nfs_formats::sky::SkyRing, scale: f32, a: f32, b: f32, colours: &[u8]) -> MeshData {
    const N: usize = 15;
    let mut m = MeshData::default();
    let side = ring.radius;
    let cell = side / N as f32;
    let t = (ring.angle_turns as f64 * 6.28318452835083) as f32;
    // det-allow: sky drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
    let (c, sn) = (t.cos(), t.sin());
    for i in 0..=N {
        let u = -0.5 * N as f32 * cell + i as f32 * cell;
        for j in 0..=N {
            let v = 0.5 * N as f32 * cell - j as f32 * cell;
            let x = (c * v + sn * u) * scale;
            let y = (ring.height - ring.small * (u * u + v * v)) * scale;
            let z = (c * u - sn * v) * scale;
            let color = MapBlend::colour(colours, i * (N + 1) + j);
            m.vertices.push(Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [j as f32 / a, i as f32 / b], color });
        }
    }
    for i in 0..N as u32 {
        for j in 0..N as u32 {
            let k = |r: u32, q: u32| r * (N as u32 + 1) + q;
            // the quad (r, c), (r, c+1), (r+1, c+1), (r+1, c) of 0x460D70
            m.indices.extend_from_slice(&[k(i, j), k(i, j + 1), k(i + 1, j + 1), k(i, j), k(i + 1, j + 1), k(i + 1, j)]);
        }
    }
    m
}

/// The horizon ring's radius in metres: 0x459D30's first point is `(0, 0, 400.0)` (`[0x5B25C4]`,
/// 0x459F92), whatever the `.bin`'s grid side says.
const RING_RADIUS: f32 = 400.0;

/// Quad `q` (0…31) of the horizon ring of `Sky/<Track>.bin` as 0x459D30 builds it: 32 quads, quad
/// `q` the ring's segment `q & 15` (0x45A0CD…0x45A18F, the positions; 0x459E71…0x459F6B, the UVs),
/// one tile of the `horz` grid with the four `hzcl` colours of its two edges. The points are
/// `(0, 0, 400)` turned about y by the `.bin`'s `+0x04` (in turns) and then by 1/16 of a turn per
/// point (`[0x3D800000]`, 0x45A014…0x45A08A: `x' = x·cos δ + z·sin δ`, `z' = z·cos δ − x·sin δ`) —
/// point `k` at `(400·sin φ, 400·cos φ)`, `φ = +0x04 + k/16` turns — and segment `k` spans points
/// `k` and `k + 1`, its top row at `y = +0x14 + +0x18` and its bottom at `+0x14` (0x45A0B4…0x45A181),
/// in metres around the eye. `colour(i, row)` is the strip's pixel `i` on row 0 (the top) and row 1
/// (the bottom); the original reads them as two runs of sixteen dwords 0x40 bytes apart (0x459D8F /
/// 0x459DE5), which is exactly the two rows of the 16 × 2 image.
///
/// **The colours of quads 16…31 are a second set** (`[this+0x3C] + 0x100`, 0x459D7D…0x459E25): the
/// same as the first — `k + rot` and `k + rot + 1` on each row, `rot` the `.bin`'s `+0x08` — but
/// for the top-right corner, whose index is `(1 − rot) + (rot + k)` = `k + 1` (0x459DBE…0x459DDB):
/// **not rotated**. On the tracks whose `+0x08` is 5 (Canyon, Forest, Industrial, Monaco1…5,
/// foothills) the draw's window reaches those quads whenever it wraps past segment 15 (the draw
/// 0x45A1A0, [`ring_window`]), and their top-right corner takes the colour five places back.
/// *(Until 2026-09-27 the port drew the sixteen once, with the first set; until 2026-09-26 it put
/// point `k` at `(cos, sin)` of `k/16` without the start turn — the ring mirrored and turned — on a
/// circle of the grid's side, 500…1500 m, rescaled with its heights: every ring sat 1.25…3.75 times
/// flatter.)*
fn horizon_ring_quad(bin: &nfs_formats::sky::SkyBin, q: usize, y0: f32, y1: f32, colour: impl Fn(usize, usize) -> [f32; 4]) -> MeshData {
    let mut m = MeshData::default();
    let k = q % 16;
    let uv = bin.segment_uv(k);
    let c = bin.segment_colour(k);
    // the second set's top-right corner (0x459DDB)
    let top_right = if q >= 16 { (k + 1) % 16 } else { c[1] };
    let p = |j: usize| {
        let az = ((bin.ring_turn + (k + j) as f32 * 0.0625) as f64 * 6.28318452835083) as f32;
        // det-allow: sky drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
        (az.sin() * RING_RADIUS, az.cos() * RING_RADIUS)
    };
    let (x0, z0) = p(0);
    let (x1, z1) = p(1);
    let v = |x: f32, z: f32, y: f32, u: f32, vv: f32, col: [f32; 4]| Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [u, vv], color: col };
    m.vertices.push(v(x0, z0, y1, uv[0], uv[2], colour(c[0], 0)));
    m.vertices.push(v(x1, z1, y1, uv[1], uv[2], colour(top_right, 0)));
    m.vertices.push(v(x0, z0, y0, uv[0], uv[3], colour(c[0], 1)));
    m.vertices.push(v(x1, z1, y0, uv[1], uv[3], colour(c[1], 1)));
    m.indices.extend_from_slice(&[0, 1, 2, 1, 3, 2]);
    m
}

impl Sky {
    /// `level` is `SKYRENDERING` (`[0x657D9C]`, `Graphics.SkyRendering`).
    /// `fe` is 0x538040's state: the UV object's jitter draws from it as it is made (0x45F3C0).
    pub fn upload(r: &mut Renderer, track: &TrackAssets, level: usize, fe: &mut crate::opponents::FeRng) -> Option<Self> {
        let find = |n: &str| track.sky_textures.iter().find(|t| t.name.trim() == n);
        fn tex(r: &mut Renderer, t: &nfs_assets::texture::TextureImage) -> nfs_renderer::TextureId {
            r.upload_texture_mips(t.width, t.height, &t.mips)
        }
        let mut parts = Vec::new();
        let sky_mat = |r: &mut Renderer, texture, blend, blend_pair| r.create_material(MaterialDesc { texture, alpha_blend: blend, cull_back: false, depth_test: false, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false });

        let file = nfs_formats::sky::SkyFile::parse(&track.sky_params.join("\n")).ok();
        // the blocks of the `SKYRENDERING` level (0x45D8F0 → 0x45FB80, `SkyFile::for_level`) — none
        // at level 0 on any shipped track. The port read the `HIGH` block whatever the option said.
        // NOT PORTED, unreachable on shipped data (`nfs-formats/examples/sky_blocks.rs`, 2026-09-30):
        // the original builds a level for **every** matching block (0x45D8F0 loops 0x45FB80's
        // count) and falls back to `Sky/default.txt` when `<track>.txt` is missing (0x45FBAF…
        // 0x45FBFA); all 15 shipped tracks have their own file, and each has at most one matching
        // block at every level (`[0, 1, 1, 0]` at LOW / MEDIUM / HIGH / AMAZING — the only file with
        // two matching blocks is `default.txt`'s two `LOW`, which no track reaches). The port builds
        // the first block.
        let block = file.as_ref().and_then(|f| f.for_level(level).first().copied());
        let mut grids = Vec::new();
        // no UV object without a block: nothing drawn from the generator (0x45F3C0 runs as one is
        // made, 0x45EADF)
        let mut jitter = Jitter::off();
        if let Some(b) = block {
            let region = &b.regions[0];
            let region_b = &b.regions[1];
            // with no usable `.bin` no layer is drawn: the loader falls back to `default.bin`
            // (0x459680 / 0x4596A1, as `TrackAssets` does), so only a broken install gets here, and
            // what the original then does was not read. *(Until 2026-09-25 the port drew an
            // invented dome scrolled by an untraced wind rule.)*
            let bin = track.sky_bin.as_ref().filter(|b| b.rings[0].radius > 0.0);
            // A stage `k` draws the grid with **cloud `k` as its texture** and **sky map `k` as its
            // vertex colours** (MultiSky 0x45BB56…0x45BB82, BasicSky 0x45B20A…0x45B236 → 0x468630):
            // the helper's `LayerTexture` takes `[this+0x64+4k]` (MultiSky; BasicSky `+0x50`) and its
            // `LayerColours` the 0x45DD90 object's `+0x14` over the region-A / region-B sky maps.
            // MultiSky has as many stages as its three cloud names have entries other than `none`
            // (`+0x80`, 0x45B97B / 0x45B9AD) and takes them in order from 0; BasicSky one, with region
            // A's cloud only (0x45B1AB). *(Corrected 2026-09-25: the port drew the sky map as the
            // texture and never read the clouds.)*
            let (clouds, pair): (Vec<Option<String>>, u8) = match b.kind {
                // the factory's blend argument (0x45D997 / 0x45D9C7 / 0x45DA13 / 0x45DA6C: 0, 1, 2, 3)
                // is the stage state block's `+8` (0x537A50 at 0x45B827), Thrash 0x38
                nfs_formats::sky::SkyKind::Multi { blend, .. } => (
                    b.clouds.clone(),
                    match blend {
                        nfs_formats::sky::SkyBlend::Normal => 0,
                        nfs_formats::sky::SkyBlend::Additive => 1,
                        nfs_formats::sky::SkyBlend::Attenuate => 2,
                        nfs_formats::sky::SkyBlend::Modulate => 3,
                    },
                ),
                // BasicSky leaves the block at 0x5379F0's pair 0
                _ => (vec![region.cloud.clone()], 0),
            };
            let stages = clouds.iter().filter(|c| c.is_some()).count();
            // a sky map is read in place as 16 × 16 dwords (0x45E640(16, …)); every shipped one is
            // a 16 × 16 `0x7D` (`nfs-game/examples/skymaps.rs`, 2026-09-30: 82 map names over the 15
            // tracks' blocks of every level, none other). Any other entry would be read in place by
            // the original whatever its size; the port drops it — not ported, unreachable.
            let map = |n: Option<&str>| n.and_then(find).filter(|t| t.width == 16 && t.height == 16 && t.code == 0x7D).and_then(|t| t.mips.first().cloned());
            for (i, (layer, cloud)) in region.layers.iter().zip(&clouds).take(stages).enumerate() {
                // unreachable with the shipped files, which list their stages' maps and clouds
                // first: a stage whose cloud is `none` or whose region-A map is missing
                let (Some(bin), Some(cloud), Some(a_px)) = (bin, cloud.as_deref(), map(layer.map.as_deref())) else { continue };
                let lb = region_b.layers.get(i);
                // region B's map when it is another entry; `none` there (a null the original's
                // 0x45E160 would read through) keeps A's — unreachable, no shipped block has it
                let b_px = lb.and_then(|l| l.map.as_deref()).filter(|n| Some(*n) != layer.map.as_deref()).and_then(|n| map(Some(n)));
                // 0x45DD90: the stream starts as A's copy, and its flash object sets up its 200
                // bolts from the generator (400 draws, 0x45DE72 / 0x45DE80)
                let colours = MapBlend { stream: a_px.clone(), a: a_px, b: b_px, w: (65536, 0), flash: Flash::new(fe) };
                // `flat` is the pack's image-less texture `[pack+0]` (0x460A99 → 0x535ED0(dev, 0, 0)),
                // bound as driver handle 0 — `dx7z` `SetTexture(stage, NULL)` (0x60007F61…0x60007F7B):
                // the vertex colours alone — and its stage never blends (0x537A40(0) at 0x45B998 /
                // 0x45B1D9). A named cloud blends unless both sky maps are opaque and the cloud's
                // format is alpha-free (0x45BBA5…0x45BBB9 / 0x45B23B…0x45B250, 0x460D40).
                let (texture, blend) = if is_flat(cloud) {
                    (None, false)
                } else {
                    let Some(t) = find(cloud) else { continue };
                    (Some(tex(r, t)), !(colours.is_opaque() && alpha_free(t.code)))
                };
                let m = sky_mat(r, texture, blend, pair);
                let scale = [layer.scale[0].max(0.01), layer.scale[1].max(0.01)];
                let ring = &bin.rings[0];
                // every stage shares the one grid (`[this+0x30]`, bound once per block at 0x45BED5),
                // so no stage sits nearer than another. *(Until 2026-09-25 each higher layer was drawn
                // 2 % closer, the port's own.)*
                let mesh = layer_grid(ring, bin.scalar_ce4, scale[0], scale[1], &colours.a);
                // 0x45F1B0: both regions' numbers for this stage; the wind flag when either
                // region's speed is over 0.25, the parallax when either move ratio is over 1
                let a = StageParams::of(Some(layer), region.move_ratio);
                let bp = StageParams::of(lb, region_b.move_ratio);
                let wind_on = layer.wind_speed > 0.25 || lb.is_some_and(|l| l.wind_speed > 0.25);
                let parallax_on = region.move_ratio > 1.0 || region_b.move_ratio > 1.0;
                let handle = r.upload_dynamic_mesh(&mesh);
                grids.push(GridLayer { mesh: handle, vertices: mesh.vertices, a, b: bp, wind_on, parallax_on, angle: ring.angle_turns, step: a.step, offset: [0.0, 0.0], colours });
                // a level whose sun token starts with `no` carries flag 0x400 (0x45B6FE…0x45B723,
                // BasicSky 0x45AEBC…0x45AEDD) and draws before the sun, any other after it
                parts.push((handle, m, Motion::Grid(grids.len() - 1), b.blocks_sun));
            }
            // the level's UV object comes after its colour streams (MultiSky 0x45B7FA before
            // 0x45BAF8, BasicSky 0x45AFCB before 0x45B14F), so its jitter draws after their bolts
            jitter = Jitter::new([b.regions[0].warp_speed, b.regions[1].warp_speed], [b.regions[0].warp_amount, b.regions[1].warp_amount], fe);
        }
        // the horizon's texture `+0x44` is `hor<level>` when the pack has one and `horz` otherwise
        // (0x459C1F…0x459C4D) — the existence test before it (0x459B8D…0x459BB7) asks the other
        // way round, but either name keeps the ring. At the shipped level 0 `Industrial` and
        // `foothills` carry a `hor0`, and `Canyon` a `hor1` / `hor2`; the port had always taken `horz`.
        if let Some(hz) = find(&format!("hor{level}")).or_else(|| find("horz")) {
            let id = tex(r, hz);
            let m = sky_mat(r, Some(id), true, 0);
            match track.sky_bin.as_ref().filter(|b| b.tile_cols > 0) {
                // the ring the track's own `.bin` describes
                Some(bin) => {
                    let hzcl = find("hzcl");
                    let colour = |i: usize, row: usize| -> [f32; 4] {
                        let Some(t) = hzcl else { return [1.0; 4] };
                        let (w, h) = (t.width as usize, t.height as usize);
                        let Some(px) = t.mips.first() else { return [1.0; 4] };
                        if w == 0 || h == 0 || px.len() < w * h * 4 {
                            return [1.0; 4];
                        }
                        let o = ((row.min(h - 1)) * w + i.min(w - 1)) * 4;
                        [px[o] as f32 / 255.0, px[o + 1] as f32 / 255.0, px[o + 2] as f32 / 255.0, 1.0]
                    };
                    let (y0, y1) = (bin.height_pair[0], bin.height_pair[0] + bin.height_pair[1]);
                    // the ring is the list's last level, flags 0x80 (0x459B53 → 0x45AAA0, 0x45DADF):
                    // no 0x400, so it draws after the sun
                    for q in 0..32 {
                        parts.push((r.upload_mesh(&horizon_ring_quad(bin, q, y0, y1, colour)), m, Motion::Ring(q as i32), true));
                    }
                }
                // no usable `.bin` (a broken install, see the layers above): no ring. *(The port
                // drew an invented two-strip ring here until 2026-09-25.)*
                None => {}
            }
        }
        let blend = track.sky_bin.as_ref().map(|b| b.track_a.clone()).unwrap_or_default();
        // 0x45D856: the lightning, with the sky pack's `ltmp` (0x45CB80) and region A's grid side
        let lightning = track.sky_bin.as_ref().map(|b| Lightning::new(b, find("ltmp")));
        let ring_turn = track.sky_bin.as_ref().map_or(0.0, |b| b.ring_turn);
        let track_c = track.sky_bin.as_ref().map(|b| b.track_c.clone()).unwrap_or_default();
        if parts.is_empty() { None } else { Some(Self { parts, grids, blend, last_tick: None, jitter, lightning, ring_turn, track_c, slice: 0, eye: Vec3::ZERO }) }
    }

    /// The layers' update (0x45D030 → each layer's slot 0 → 0x45F0A0): `t` from the `.bin`'s track
    /// at the camera's slice, `ticks = (gGameTick − last) / 2` (1 for a single tick, at most 64),
    /// every stage's numbers lerped between the regions by `t`, the UV step rewritten into the grid
    /// when it moved (0x45EC40), and the offset moved by the wind or set by the parallax (0x45ED40).
    ///
    /// **The eye and the slice it works from are the last pass's**: 0x45D7D0 runs the update and
    /// then the pass, and only the pass saves the view's eye into `+0x38…+0x40` (0x45D562…0x45D579)
    /// and its slice into `+0x50` (0x481400 at 0x45D587 → 0x45D598), which the update reads
    /// (0x45D073, 0x45D086…0x45D12A, 0x45D15C) — both 0 from the constructor (0x45D81D,
    /// 0x45D82B…0x45D831). `eye` / `slice` are what this frame's pass saves after it. With the
    /// mirror up, 0x45D7D0 runs again in the mirror's pass (0x468AA1 after the main one at
    /// 0x468A74): a second update in the same game tick — 0 ticks (`(0)/2`) — from the main pass's
    /// eye and slice, then the mirror's saved for the next frame's main update.
    pub fn update(&mut self, r: &mut Renderer, eye: Vec3, slice_now: i32, lap_slices: i32, game_tick: u64, fe: &mut crate::opponents::FeRng) {
        let (camera_pos, slice) = (self.eye, self.slice);
        let ticks = match self.last_tick {
            None => 1,
            Some(last) => {
                let d = game_tick.saturating_sub(last);
                (if d == 1 { 1 } else { d / 2 }).min(64)
            }
        };
        let ticks_i = ticks as u32;
        let ticks = ticks as f32;
        self.last_tick = Some(game_tick);
        let t = nfs_formats::sky::sample_track(&self.blend, slice, lap_slices).unwrap_or(0.0);
        // each level's slot 0 runs its colour streams first (0x45BCFE / 0x45B352 → 0x45DF50: the
        // flash or the region blend 0x45E160), then its UV object (0x45BD2D / 0x45B36E → 0x45F0A0)
        let mut recolour = vec![false; self.grids.len()];
        for (g, dirty) in self.grids.iter_mut().zip(&mut recolour) {
            if g.colours.update(t, ticks_i, self.lightning.as_ref(), fe) {
                for (k, v) in g.vertices.iter_mut().enumerate() {
                    v.color = MapBlend::colour(&g.colours.stream, k);
                }
                *dirty = true;
            }
        }
        let jit = &mut self.jitter;
        let lerp = |p: [f32; 2]| p[0] + (p[1] - p[0]) * t;
        let (rate, amp) = (lerp(jit.rate), lerp(jit.amp));
        jit.step(ticks_i, rate, amp, fe);
        for (g, &recoloured) in self.grids.iter_mut().zip(&recolour) {
            let mut dirty = recoloured;
            let p = StageParams::lerp(&g.a, &g.b, t);
            if p.step != g.step || jit.on {
                g.step = p.step;
                let a = g.step;
                // 0x45EC40: `u = j·A + jitter.u`, `v = i·B + jitter.v` (the offset goes in the draw)
                for (k, v) in g.vertices.iter_mut().enumerate() {
                    let (i, j) = (k / 16, k % 16);
                    let d = jit.pos[k];
                    v.uv = [j as f32 * a[0] + d[0], i as f32 * a[1] + d[1]];
                }
                dirty = true;
            }
            if dirty {
                r.write_mesh_vertices(g.mesh, &g.vertices);
            }
            if g.wind_on {
                g.offset = [(g.offset[0] + p.wind[0] * ticks).rem_euclid(1.0), (g.offset[1] + p.wind[1] * ticks).rem_euclid(1.0)];
            } else if g.parallax_on {
                // the camera turned by minus the grid's angle (0x45D086…0x45D12A)
                let a = -(g.angle as f64 * 6.28318452835083) as f32;
                // det-allow: sky drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
                let (c, s) = (a.cos(), a.sin());
                let x = c * camera_pos.x + s * camera_pos.z;
                let z = c * camera_pos.z - s * camera_pos.x;
                let fu = p.step[0] * x * p.inv_ratio;
                let fv = p.step[1] * z * p.inv_ratio;
                g.offset = [-(fu - fu.floor()), fv - fv.floor()];
            }
        }
        // after the levels, the lightning by track B's field 6 at the camera's slice (0x45D15C…
        // 0x45D189: 0x45A560, 0x45A910 `+0x1C`, 0x45CCF0 with the same ticks) — Autobahn and
        // Industrial are the tracks whose field reaches 0.04
        if let Some(l) = self.lightning.as_mut() {
            let value = l.bin.sample_track_b(6, slice, lap_slices);
            // port diagnostic: `NFS_LIGHTNING=1` feeds the field's top value on every tick, so a
            // strike lands within a few ticks for a still (the original has no such switch)
            let value = if forced_lightning() { 1.0 } else { value };
            l.update(value, ticks_i, fe);
            if forced_lightning() {
                tracing::info!("lightning: on={} mode={} cell={:?} time={} end={} grids={} flashes={}", l.on, l.mode, l.cell, l.time, l.end, self.grids.len(), self.grids.iter().filter(|g| g.colours.flash.active).count());
            }
        }
        // the pass that follows saves its own eye and slice (0x45D562…0x45D598)
        self.eye = eye;
        self.slice = slice_now;
    }

    /// `[this+0x50]`, the slice the last pass saved: where 0x481400 starts its search for the next
    /// pass's eye (0x45D57C…0x45D587).
    pub fn pass_slice(&self) -> i32 {
        self.slice
    }

    /// The thunder the last strike called (0x4A8340), taken once: the audio update hands it to the
    /// thunder's countdown 0x4A8380. A second strike before the take replaces it, as the original's
    /// globals `[0x656494]`, `[0x656834]`, `[0x656498]` are overwritten.
    pub fn take_thunder(&mut self) -> Option<ThunderCall> {
        self.lightning.as_mut().and_then(|l| l.thunder.take())
    }

    /// Draw items centred on the camera, in metres: the sky pass 0x45D4F0 moves the camera's eye to
    /// the origin (0x45D63D…0x45D679, `0x466220` with `(0, 0, 0)`; the eye back at 0x45D794) and sets
    /// the view's planes to 0.15 / 700 (0x45D501…0x45D51D → 0x467120), so the grid and the ring are
    /// drawn unscaled around the eye and whatever lies beyond 700 m of view depth is clipped — the
    /// grid's far corners on `alps`, `Canyon`, `Castle`, `Farmland` and `foothills` (whose side ×
    /// `+0xCE4` puts them past it). *(Until 2026-09-26 the port scaled the whole sky to 0.9 of the
    /// draw distance, its own rule: the same picture, but nothing ever clipped.)* The port keeps the
    /// view's own near plane (0.5, the driver view's 0.15) — no sky point comes within 10 m.
    ///
    /// Unlit: the layers and the horizon ring are drawn through the primitive helper 0x468240 /
    /// 0x468310, whose chain (0x468440…0x4685CE, 0x4685E0, 0x468630) is `LayerUV`, `LayerTexture`,
    /// `LayerState`, `LayerColours` and the draw — no light layer — so a sky pixel is the texture ×
    /// its own colour stream (`RE/notes/thrash.md` §6d.5), not the scene's ambient-and-sun term.
    ///
    /// `after_sun` picks the levels the pass draws after the sun (0x45D6F0 → 0x460290): the pass
    /// runs its list twice, first the levels with flag 0x400 (0x45D6A5…0x45D6EE), then the sun, then
    /// the rest (0x45D704…0x45D748).
    ///
    /// Each level goes through the gate **0x45D470**: the ring (flag 0x80) draws whenever the view's
    /// `+0x11C` is 1 — both race views set it, 0x468BE6 / 0x468D24 — and shows only the quads of
    /// [`ring_window`] around the view's heading ([`SkyView::heading`]), each view its own. Any other
    /// level draws only in the main view (`+0x12C` 2) with the sky's `[this+0x60]` set, and there
    /// unless the view's up row is near vertical (`m[1][1]` ≥ 0.95, the double `[0x5B3ED8]`) **and**
    /// the `+0x655C` track's byte at the camera's slice is set (0x45A9E0, [`track_c_flag`]) — on
    /// Forest's flagged stretches the layers are left out while the camera looks level, the ring
    /// alone behind the trees. So the mirror (`+0x12C` 1) shows the ring and the sun, no layer.
    /// `[this+0x60]` is 1 from the constructor (0x45D827); only the `I_HORIZON` key (input bit 0x10,
    /// 0x4159A3…0x4159C4 → 0x45DBB0 / 0x45DB90) flips it, and `fe.txt` leaves that key unbound
    /// (`controls.md` §4.170), so it stays 1.
    pub fn draw_items(&self, view: &SkyView, after_sun: bool) -> impl Iterator<Item = DrawItem> + '_ {
        let t = glam::Mat4::from_translation(view.eye);
        const UNLIT: nfs_renderer::LayerLight = nfs_renderer::LayerLight { ambient: [1.0; 3], diffuse: [0.0; 3], direction: [0.0; 3] };
        let (lo, hi) = ring_window(view.heading(), self.ring_turn);
        let layers = view.main && ((view.up_row_y() as f64) < 0.95 || !track_c_flag(&self.track_c, self.slice));
        self.parts
            .iter()
            .filter(move |p| p.3 == after_sun)
            .filter(move |p| match p.2 {
                Motion::Ring(q) => (lo..hi).contains(&q),
                Motion::Grid(_) => layers,
            })
            .map(move |(mesh, material, motion, _)| {
                // the pass sets Thrash 0x24 ← 10 before its levels and the ring (0x45D689…0x45D68D): the
                // alpha test `GREATER 10` (`RE/notes/thrash.md` §6d.4)
                let mut item = DrawItem { layer_light: Some(UNLIT), alpha_test: true, sky_pass: true, ..DrawItem::new(*mesh, *material, t) };
                item.uv_offset = match *motion {
                    Motion::Ring(_) => [0.0, 0.0],
                    Motion::Grid(i) => self.grids[i].offset,
                };
                item
            })
    }
}

#[cfg(test)]
mod stage_tests {

    #[test]
    fn map_blend_is_0x45e160s_fixed_point_lerp() {
        let (a, b) = ([200u8, 100, 0, 255], [0u8, 100, 255, 255]);
        assert_eq!(blend_maps(&a, &b, 0.0).1, [200, 100, 0, 255]);
        // t = 1: wa = 0, wb = 65536
        assert_eq!(blend_maps(&a, &b, 1.0).1, [0, 100, 255, 255]);
        // t = 0.5: (a + b)·32768 >> 16
        assert_eq!(blend_maps(&a, &b, 0.5), ((32768, 32768), vec![100, 100, 127, 255]));
    }

    #[test]
    fn stage_colours_and_blend_state() {
        // vertex k takes texel k as its RGBA
        let px: Vec<u8> = (0..256u32).flat_map(|k| [k as u8, 0, 255, 255]).collect();
        assert_eq!(MapBlend::colour(&px, 17), [17.0 / 255.0, 0.0, 1.0, 1.0]);
        let mut m = MapBlend { stream: px.clone(), a: px.clone(), b: None, w: (65536, 0), flash: Flash::new(&mut crate::opponents::FeRng::new(0)) };
        assert!(m.is_opaque());
        // one texel under 0xFF in either map makes the stage blend (0x45E5E0, 0x45E110)
        let mut b = px;
        b[3] = 0xFE;
        m.b = Some(b);
        assert!(!m.is_opaque());
        // 0x556690 classes 3, 4, 5 are alpha-free; 8888 and 4444 are not
        assert!(alpha_free(0x7E) && alpha_free(0x78) && alpha_free(0x7F));
        assert!(!alpha_free(0x7D) && !alpha_free(0x6D) && !alpha_free(0x7B));
        assert!(is_flat("flat") && is_flat("FLAT") && !is_flat("cc1a") && !is_flat("fl"));
    }
    use super::*;

    #[test]
    fn stage_numbers_and_their_blend() {
        let l = |a: f32, speed: f32, dir: f32| nfs_formats::sky::SkyLayer { map: None, alpha_map: None, scale: [a, a], wind_speed: speed, wind_direction: dir };
        // 1/a, the wind speed · 0.00025 along (sin θ, cos θ) with θ = −dir/360 turns, 1/moveRatio
        let a = StageParams::of(Some(&l(2.0, 4.0, 90.0)), 400.0);
        assert!((a.step[0] - 0.5).abs() < 1e-6 && (a.inv_ratio - 0.0025).abs() < 1e-9);
        assert!((a.wind[0] + 0.001).abs() < 1e-6 && a.wind[1].abs() < 1e-6, "{:?}", a.wind);
        let b = StageParams::of(Some(&l(4.0, 0.0, 0.0)), 0.0);
        let m = StageParams::lerp(&a, &b, 0.5);
        assert!((m.step[0] - 0.375).abs() < 1e-6 && (m.inv_ratio - 0.00125).abs() < 1e-9);
    }

    fn test_bin() -> nfs_formats::sky::SkyBin {
        nfs_formats::sky::SkyBin {
            version: 9,
            colours: Vec::new(),
            ring_turn: 0.0,
            colour_rotation: 0,
            tile_cols: 4,
            tile_rows: 2,
            height_pair: [0.0; 2],
            segments: Default::default(),
            rings: [nfs_formats::sky::SkyRing { angle_turns: 0.0, radius: 1000.0, small: 0.0, height: 100.0 }; 2],
            scalar_ce4: 1.0,
            sun_offset: [0.0; 2],
            sun_readback: false,
            track_a: Vec::new(),
            track_b: Vec::new(),
            track_c: Vec::new(),
        }
    }

    #[test]
    fn lightning_waits_for_its_value_and_its_draw() {
        let mut fe = crate::opponents::FeRng::new(7);
        let mut l = Lightning::new(&test_bin(), None);
        // under 0.04 nothing is drawn (0x45CD17)
        let before = fe.clone().next_u32();
        l.update(0.03, 1, &mut fe);
        assert_eq!(fe.clone().next_u32(), before);
        assert!(!l.on && l.time == 1);
        // mode by the value; a strike eventually, its cell held to 0…15 and its end 60…120 ticks on
        for _ in 0..10_000 {
            l.update(0.7, 1, &mut fe);
            if l.on {
                break;
            }
        }
        assert!(l.on && l.mode == 2);
        assert!(l.cell.iter().all(|c| (0.0..=15.0).contains(c)));
        let span = l.end.wrapping_sub(l.time);
        assert!((60..=120).contains(&span), "{span}");
        // mode 2 lands within 0.2…0.3 of the half side from the centre: cells 4.5…10.5
        assert!(l.cell.iter().all(|c| (4.0..=11.0).contains(c)), "{:?}", l.cell);
        for _ in 0..span {
            l.update(0.7, 1, &mut fe);
        }
        assert!(l.on);
        l.update(0.7, 1, &mut fe);
        assert!(!l.on);
    }

    #[test]
    fn a_strike_in_ten_calls_the_thunder_with_the_turn_as_its_pan() {
        let mut fe = crate::opponents::FeRng::new(11);
        let mut l = Lightning::new(&test_bin(), None);
        let (mut strikes, mut calls) = (0, 0);
        for _ in 0..2_000 {
            // replay the strike's draws on a copy: r, d, turn, then the thunder test
            let mut peek = fe.clone();
            let _ = rand01(&mut peek);
            l.mode = 2;
            l.on = false;
            l.strike(&mut fe);
            let (_, _, turn, test) = (rand01(&mut peek), 0, rand01(&mut peek), rand01(&mut peek));
            strikes += 1;
            match l.thunder.take() {
                Some(c) => {
                    calls += 1;
                    assert!(test < 0.1);
                    assert_eq!(c.lead, 0x7E);
                    assert_eq!(c.pan, (turn * 65535.0f32).round_ties_even() as i32);
                    // the draw 0x4A8340 makes is the strike's last
                    assert_eq!(c.draw, peek.next_u32());
                    assert_eq!(fe.clone().next_u32(), peek.next_u32());
                }
                None => assert!(test >= 0.1),
            }
        }
        assert!(calls > strikes / 20 && calls < strikes / 5, "{calls} of {strikes}");
    }

    #[test]
    fn flash_kind_pick_rounds_half_to_even() {
        // fistp(trunc(3r) − 0.5): 0 → 0, 1 → 0, 2 → 2
        for (k, want) in [(0, 0), (1, 0), (2, 2)] {
            assert_eq!(((k as f32) - 0.5).round_ties_even() as i32, want);
        }
    }

    #[test]
    fn bolts_brighten_to_their_caps_and_the_stream_comes_back() {
        let mut fe = crate::opponents::FeRng::new(3);
        let mut l = Lightning::new(&test_bin(), None);
        l.ltmp[24] = [255, 200, 150, 100];
        l.on = true;
        l.mode = 2; // every bolt on the strike's own cell
        l.cell = [5.7, 9.2];
        let a: Vec<u8> = (0..256).flat_map(|_| [10u8, 20, 30, 255]).collect();
        let mut m = MapBlend { stream: a.clone(), a, b: None, w: (65536, 0), flash: Flash::new(&mut fe) };
        // the strike starts the flash and changes nothing yet
        assert!(!m.update(0.0, 1, Some(&l), &mut fe));
        assert!(m.flash.active && (m.flash.x, m.flash.z) == (5, 9));
        let at = 4 * (16 * 9 + 5);
        let mut peak = 0;
        for _ in 0..40 {
            assert!(m.update(0.0, 1, Some(&l), &mut fe));
            let px = &m.stream[at..at + 4];
            assert!(px[0] <= 200 && px[1] <= 150 && px[2] <= 100 && px[3] == 255);
            peak = peak.max(px[0]);
            // nothing but the strike's cell moves in mode 2
            assert!(m.stream.chunks(4).enumerate().all(|(k, p)| 4 * k == at || p == [10, 20, 30, 255]));
        }
        assert!(peak > 10);
        // the lightning over: every bolt done, the flash ends and the saved stream is back
        l.on = false;
        m.update(0.0, 1, Some(&l), &mut fe);
        assert!(!m.flash.active);
        assert!(m.stream.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn jitter_wanders_within_its_bounds() {
        // off at or under 0.01 on both regions (0x45F342…0x45F362)
        let mut fe = crate::opponents::FeRng::new(0);
        let still = Jitter::new([0.01, 0.0], [0.5, 0.5], &mut fe);
        assert!(!still.on);
        let mut j = Jitter::new([0.3, 0.3], [0.2, 0.2], &mut fe);
        let (rate, amp) = (0.3 / 180.0, 0.2);
        assert!(j.pos.iter().all(|p| p[0].abs() <= amp && p[1].abs() <= amp));
        for _ in 0..500 {
            j.step(1, rate, amp, &mut fe);
            // the velocity is held within ±rate (0x45EFB4…0x45F03F)
            assert!(j.vel.iter().all(|v| v[0].abs() <= rate + 1e-9 && v[1].abs() <= rate + 1e-9));
        }
        // the targets are 2·amp·r − amp and the index cycles through twelve of the thirteen randoms
        assert!(j.target.iter().all(|p| p[0].abs() <= amp && p[1].abs() <= amp));
        assert!(j.next < 12);
    }

    #[test]
    fn ring_window_is_six_quads_about_the_heading() {
        // straight down +z with the ring starting at 0: quads −3…2, moved on a lap → 13…18
        assert_eq!(ring_window(0.0, 0.0), (13, 19));
        // a quarter turn: 16·0.125 = 2 → quads 1…6
        assert_eq!(ring_window(0.25, 0.0), (1, 7));
        // the start turn is taken off, a negative difference a lap on
        assert_eq!(ring_window(0.25, 0.25), ring_window(0.0, 0.0));
        assert_eq!(ring_window(0.1, 0.9), ring_window(0.2, 0.0));
        // near a whole turn the window runs past 15 without wrapping
        let (lo, hi) = ring_window(0.97, 0.0);
        assert!(lo >= 0 && hi > 16 && hi - lo == 6);
    }

    #[test]
    fn heading_is_the_turn_from_plus_z_towards_plus_x() {
        let v = |f: Vec3| SkyView { eye: Vec3::ZERO, forward: f, up: Vec3::Y, main: true };
        assert!(v(Vec3::Z).heading().abs() < 1e-6);
        assert!((v(Vec3::X).heading() - 0.25).abs() < 1e-6);
        assert!((v(-Vec3::Z).heading() - 0.5).abs() < 1e-6);
        assert!((v(-Vec3::X).heading() - 0.75).abs() < 1e-6);
        // the y is dropped: pitch does not move the heading
        assert!((v(Vec3::new(1.0, -0.5, 0.0)).heading() - 0.25).abs() < 1e-6);
        assert!((v(Vec3::Y).up_row_y()).abs() < 1e-6);
        assert!((v(Vec3::Z).up_row_y() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn track_c_is_the_last_key_at_or_before_the_slice() {
        let keys = [(0, 0u8), (4, 0), (697, 1), (753, 1), (754, 0)];
        assert!(!track_c_flag(&keys, 696));
        assert!(track_c_flag(&keys, 697));
        assert!(track_c_flag(&keys, 753));
        assert!(!track_c_flag(&keys, 754));
        assert!(!track_c_flag(&keys, 5000));
        // fewer than two keys: 0x45A970's two zero keys
        assert!(!track_c_flag(&[(0, 1)], 10));
    }

    #[test]
    fn ring_quads_past_sixteen_keep_an_unrotated_top_right() {
        let mut bin = test_bin();
        bin.colour_rotation = 5;
        let colour = |i: usize, row: usize| [i as f32, row as f32, 0.0, 1.0];
        for k in 0..16 {
            let a = horizon_ring_quad(&bin, k, 0.0, 10.0, colour);
            let b = horizon_ring_quad(&bin, k + 16, 0.0, 10.0, colour);
            assert_eq!(a.vertices[0].position, b.vertices[0].position);
            // top left, bottom left, bottom right: the same rotated colours
            for v in [0, 2, 3] {
                assert_eq!(a.vertices[v].color, b.vertices[v].color);
            }
            assert_eq!(a.vertices[1].color[0], ((k + 6) % 16) as f32);
            assert_eq!(b.vertices[1].color[0], ((k + 1) % 16) as f32);
        }
    }
}
