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
//! **APPROXIMATION** in what is left: the jitter draws from the front end's generator 0x538040
//! (`opponents::FeRng`, through 0x5561D0 at 0x45EEA1 / 0x45F3D3) on a state of its own that starts
//! at the image's `.data` values, where the game's one global state is shared with the weather and
//! the front end. *(Until 2026-09-26 it drew from an LCG of the port's own.)* Not ported: `basic`'s `noise` alpha maps
//! (0x45E680, folded into the colours' alpha by 0x45E27D…; no shipped block names one), and what
//! 0x45DF50 does instead of the blend while `[0x627E38]` is set (0x45CEF0, a flash — not read).

use glam::Vec3;
use nfs_assets::track::TrackAssets;
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};

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
}

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
    /// 0x538040's state (see the module's note)
    fe: crate::opponents::FeRng,
}

impl Jitter {
    const VERTICES: usize = 16 * 16;

    fn new(speed: [f32; 2], amount: [f32; 2]) -> Self {
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
            fe: crate::opponents::FeRng::new(0),
        };
        // 0x45F3C0, with the numbers 0x45F1B0 copied from region A (0x45F366…0x45F375)
        let (rate, amp) = (j.rate[0], j.amp[0]);
        j.refill();
        for i in 0..Self::VERTICES {
            let u = j.draw(amp);
            let v = j.draw(amp);
            j.pos[i] = [u, v];
        }
        j.step(1, rate, amp);
        j
    }

    /// The thirteen randoms, each 0x5561D0 = `(0x538040() & 0xFFFF) / 65536` (0x45EEA1 / 0x45F3D3).
    fn refill(&mut self) {
        for k in 0..13 {
            self.table[k] = (self.fe.next_u32() & 0xFFFF) as f32 * (1.0 / 65536.0);
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
    fn step(&mut self, ticks: u32, rate: f32, amp: f32) {
        if !self.on {
            return;
        }
        self.refill();
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
    Still,
    /// a grid layer: index into `Sky::grids`
    Grid(usize),
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
    std::env::var("NFS_SKY_LEVEL").ok()?.parse().ok()
}

/// The 16 × 16 layer grid of 0x460EC0(15, …) / 0x460F40 in metres around the eye, with the base UVs of 0x45EC40 (`j/a`, `i/b`) and the colour stream `colours` (vertex
/// `16·i + j` takes texel `16·i + j`), as the 15 × 15 quads' triangles.
fn layer_grid(ring: &nfs_formats::sky::SkyRing, scale: f32, a: f32, b: f32, colours: &[u8]) -> MeshData {
    const N: usize = 15;
    let mut m = MeshData::default();
    let side = ring.radius;
    let cell = side / N as f32;
    let t = (ring.angle_turns as f64 * 6.28318452835083) as f32;
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

/// The horizon ring of `Sky/<Track>.bin` as 0x459D30 builds it: sixteen quads, each one tile of the
/// `horz` grid, with the four `hzcl` colours of its two edges. The points are `(0, 0, 400)` turned
/// about y by the `.bin`'s `+0x04` (in turns) and then by 1/16 of a turn per point (`[0x3D800000]`,
/// 0x45A014…0x45A08A: `x' = x·cos δ + z·sin δ`, `z' = z·cos δ − x·sin δ`) — point `k` at
/// `(400·sin φ, 400·cos φ)`, `φ = +0x04 + k/16` turns — and quad `k` spans points `k` and `k + 1`,
/// its top row at `y = +0x14 + +0x18` and its bottom at `+0x14` (0x45A0B4…0x45A181), in metres
/// around the eye. `colour(i)` is the strip's pixel `i` on row 0 (the top) and row 1 (the bottom);
/// the original reads them as two runs of sixteen dwords 0x40 bytes apart (0x459D8F / 0x459DE5),
/// which is exactly the two rows of the 16 × 2 image. *(Until 2026-09-26 the port put point `k` at
/// `(cos, sin)` of `k/16` without the start turn — the ring mirrored and turned — on a circle of the
/// grid's side, 500…1500 m, rescaled with its heights: every ring sat 1.25…3.75 times flatter.)*
fn horizon_ring(bin: &nfs_formats::sky::SkyBin, y0: f32, y1: f32, colour: impl Fn(usize, usize) -> [f32; 4]) -> MeshData {
    let mut m = MeshData::default();
    for k in 0..16usize {
        let uv = bin.segment_uv(k);
        let c = bin.segment_colour(k);
        let p = |j: usize| {
            let az = ((bin.ring_turn + (k + j) as f32 * 0.0625) as f64 * 6.28318452835083) as f32;
            (az.sin() * RING_RADIUS, az.cos() * RING_RADIUS)
        };
        let (x0, z0) = p(0);
        let (x1, z1) = p(1);
        let base = m.vertices.len() as u32;
        let v = |x: f32, z: f32, y: f32, u: f32, vv: f32, col: [f32; 4]| Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [u, vv], color: col };
        m.vertices.push(v(x0, z0, y1, uv[0], uv[2], colour(c[0], 0)));
        m.vertices.push(v(x1, z1, y1, uv[1], uv[2], colour(c[1], 0)));
        m.vertices.push(v(x0, z0, y0, uv[0], uv[3], colour(c[0], 1)));
        m.vertices.push(v(x1, z1, y0, uv[1], uv[3], colour(c[1], 1)));
        m.indices.extend_from_slice(&[base, base + 1, base + 2, base + 1, base + 3, base + 2]);
    }
    m
}

impl Sky {
    /// `level` is `SKYRENDERING` (`[0x657D9C]`, `Graphics.SkyRendering`).
    pub fn upload(r: &mut Renderer, track: &TrackAssets, level: usize) -> Option<Self> {
        let find = |n: &str| track.sky_textures.iter().find(|t| t.name.trim() == n);
        fn tex(r: &mut Renderer, t: &nfs_assets::texture::TextureImage) -> nfs_renderer::TextureId {
            r.upload_texture(&TextureData { width: t.width, height: t.height, mips: t.mips.clone() })
        }
        let mut parts = Vec::new();
        let sky_mat = |r: &mut Renderer, texture, blend, blend_pair| r.create_material(MaterialDesc { texture, alpha_blend: blend, cull_back: false, depth_test: false, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false });

        let file = nfs_formats::sky::SkyFile::parse(&track.sky_params.join("\n")).ok();
        // the blocks of the `SKYRENDERING` level (0x45D8F0 → 0x45FB80, `SkyFile::for_level`) — none
        // at level 0 on any shipped track. The port read the `HIGH` block whatever the option said.
        // APPROXIMATION of an unreachable case: the original builds a level for **every** matching
        // block; only `default.txt`'s two `LOW` blocks are ever more than one, and every shipped
        // track has its own file, so the port builds the first
        let block = file.as_ref().and_then(|f| f.for_level(level).first().copied());
        let mut grids = Vec::new();
        let mut jitter = Jitter::new([0.0; 2], [0.0; 2]);
        if let Some(b) = block {
            jitter = Jitter::new([b.regions[0].warp_speed, b.regions[1].warp_speed], [b.regions[0].warp_amount, b.regions[1].warp_amount]);
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
            // a 16 × 16 `0x7D`. APPROXIMATION of an unreachable case: any other entry is dropped.
            let map = |n: Option<&str>| n.and_then(find).filter(|t| t.width == 16 && t.height == 16 && t.code == 0x7D).and_then(|t| t.mips.first().cloned());
            for (i, (layer, cloud)) in region.layers.iter().zip(&clouds).take(stages).enumerate() {
                // unreachable with the shipped files, which list their stages' maps and clouds
                // first: a stage whose cloud is `none` or whose region-A map is missing
                let (Some(bin), Some(cloud), Some(a_px)) = (bin, cloud.as_deref(), map(layer.map.as_deref())) else { continue };
                let lb = region_b.layers.get(i);
                // region B's map when it is another entry; `none` there (a null the original's
                // 0x45E160 would read through) keeps A's — unreachable, no shipped block has it
                let b_px = lb.and_then(|l| l.map.as_deref()).filter(|n| Some(*n) != layer.map.as_deref()).and_then(|n| map(Some(n)));
                let colours = MapBlend { a: a_px, b: b_px, w: (65536, 0) };
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
                    parts.push((r.upload_mesh(&horizon_ring(bin, y0, y1, colour)), m, Motion::Still, true));
                }
                // no usable `.bin` (a broken install, see the layers above): no ring. *(The port
                // drew an invented two-strip ring here until 2026-09-25.)*
                None => {}
            }
        }
        let blend = track.sky_bin.as_ref().map(|b| b.track_a.clone()).unwrap_or_default();
        if parts.is_empty() { None } else { Some(Self { parts, grids, blend, last_tick: None, jitter }) }
    }

    /// The layers' update (0x45D030 → each layer's slot 0 → 0x45F0A0): `t` from the `.bin`'s track
    /// at the camera's slice, `ticks = (gGameTick − last) / 2` (1 for a single tick, at most 64),
    /// every stage's numbers lerped between the regions by `t`, the UV step rewritten into the grid
    /// when it moved (0x45EC40), and the offset moved by the wind or set by the parallax (0x45ED40).
    pub fn update(&mut self, r: &mut Renderer, camera_pos: Vec3, slice: i32, lap_slices: i32, game_tick: u64) {
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
        let jit = &mut self.jitter;
        let lerp = |p: [f32; 2]| p[0] + (p[1] - p[0]) * t;
        let (rate, amp) = (lerp(jit.rate), lerp(jit.amp));
        jit.step(ticks_i, rate, amp);
        for g in &mut self.grids {
            let mut dirty = false;
            // the stage's colour stream at this `t` (slot 0 → 0x45DF50 → 0x45E160, 0x45BCFE / 0x45B352)
            if let Some(b) = g.colours.b.as_deref() {
                let (w, px) = blend_maps(&g.colours.a, b, t);
                if w != g.colours.w {
                    g.colours.w = w;
                    for (k, v) in g.vertices.iter_mut().enumerate() {
                        v.color = MapBlend::colour(&px, k);
                    }
                    dirty = true;
                }
            }
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
                let (c, s) = (a.cos(), a.sin());
                let x = c * camera_pos.x + s * camera_pos.z;
                let z = c * camera_pos.z - s * camera_pos.x;
                let fu = p.step[0] * x * p.inv_ratio;
                let fv = p.step[1] * z * p.inv_ratio;
                g.offset = [-(fu - fu.floor()), fv - fv.floor()];
            }
        }
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
    pub fn draw_items(&self, camera_pos: Vec3, after_sun: bool) -> impl Iterator<Item = DrawItem> + '_ {
        let t = glam::Mat4::from_translation(camera_pos);
        const UNLIT: nfs_renderer::LayerLight = nfs_renderer::LayerLight { ambient: [1.0; 3], diffuse: [0.0; 3], direction: [0.0; 3] };
        self.parts.iter().filter(move |p| p.3 == after_sun).map(move |(mesh, material, motion, _)| {
            // the pass sets Thrash 0x24 ← 10 before its levels and the ring (0x45D689…0x45D68D): the
            // alpha test `GREATER 10` (`RE/notes/thrash.md` §6d.4)
            let mut item = DrawItem { layer_light: Some(UNLIT), alpha_test: true, sky_pass: true, ..DrawItem::new(*mesh, *material, t) };
            item.uv_offset = match *motion {
                Motion::Still => [0.0, 0.0],
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
        let mut m = MapBlend { a: px.clone(), b: None, w: (65536, 0) };
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

    #[test]
    fn jitter_wanders_within_its_bounds() {
        // off at or under 0.01 on both regions (0x45F342…0x45F362)
        let still = Jitter::new([0.01, 0.0], [0.5, 0.5]);
        assert!(!still.on);
        let mut j = Jitter::new([0.3, 0.3], [0.2, 0.2]);
        let (rate, amp) = (0.3 / 180.0, 0.2);
        assert!(j.pos.iter().all(|p| p[0].abs() <= amp && p[1].abs() <= amp));
        for _ in 0..500 {
            j.step(1, rate, amp);
            // the velocity is held within ±rate (0x45EFB4…0x45F03F)
            assert!(j.vel.iter().all(|v| v[0].abs() <= rate + 1e-9 && v[1].abs() <= rate + 1e-9));
        }
        // the targets are 2·amp·r − amp and the index cycles through twelve of the thirteen randoms
        assert!(j.target.iter().all(|p| p[0].abs() <= amp && p[1].abs() <= amp));
        assert!(j.next < 12);
    }
}
