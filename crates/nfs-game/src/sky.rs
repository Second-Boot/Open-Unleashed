//! Sky: the layers `Track/Sky/<Track>.txt` asks for, on a dome (`SkyLevel_*`, RE/RENDERING.md).
//!
//! The layer list, the textures, their UV repeats, the wind and the warp all come from the track's
//! own sky file now (`nfs_formats::sky`): the block of the `SKYRENDERING` level is taken, weather
//! region A is used, and each layer that names a sky map is drawn as one dome, the first opaque and
//! the rest alpha-blended (the shipped tracks are all `MULTINORMAL` or `basic`). The `horz` horizon
//! strips keep their ring.
//!
//! The **horizon ring is the original's own** now (`RE/RENDERING.md`, sky `.bin`): sixteen quads on
//! a circle of the radius the `.bin`'s weather-region-A record gives, spanning `+0x14` to
//! `+0x14 + +0x18` in y, each showing the tile of the `horz` grid the `.bin` names for it (mirrored
//! where it says so) and tinted per corner from the `hzcl` 16 × 2 strip, rotated by `+0x08`.
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
//! **APPROXIMATION** in what is left: the jitter's random numbers come from the port's own
//! generator, not the game's shared 0x538040 sequence; and `basic` skies' cloud / alpha maps, which
//! are drawn as a plain layer. *(Region B's own maps are blended in by `t` now, `MapBlend`.)*

use glam::Vec3;
use nfs_assets::track::TrackAssets;
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};

pub struct Sky {
    /// mesh, material and how the layer's UVs move
    parts: Vec<(MeshId, MaterialId, Motion)>,
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
    rng: u32,
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
            rng: 0x1234_5678,
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

    fn refill(&mut self) {
        for k in 0..13 {
            self.rng = self.rng.wrapping_mul(0x0001_9660).wrapping_add(0x3C6E_F35F);
            self.table[k] = ((self.rng >> 8) & 0xFFFF) as f32 * (1.0 / 65536.0);
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
    /// A MultiSky stage whose region-B map differs from region A's: its texture is 0x45DD90's
    /// blend of the two, redone by 0x45E160 at each `t` — `(A·wa + B·wb) >> 16` per byte with
    /// `wa = fistp((1 − t)·65536)`, `wb = fistp(t·65536)` (0x45E188…0x45E25D). Only `Alps.txt`
    /// names such a pair (`st1a`/`st1b`, `sc1a`/`sc1b`, all 16 × 16).
    blend: Option<MapBlend>,
}

struct MapBlend {
    texture: nfs_renderer::TextureId,
    width: u32,
    height: u32,
    a: Vec<u8>,
    b: Vec<u8>,
    /// the `wb` last drawn, so an unchanged `t` costs nothing
    wb: i32,
}

/// 0x45E160's blend of two maps' bytes at `t`: returns `wb` and the pixels.
fn blend_maps(a: &[u8], b: &[u8], t: f32) -> (i32, Vec<u8>) {
    let wa = ((1.0 - t) * 65536.0).round_ties_even() as i32;
    let wb = (t * 65536.0).round_ties_even() as i32;
    let px = a.iter().zip(b).map(|(&a, &b)| ((a as i32 * wa + b as i32 * wb) >> 16) as u8).collect();
    (wb, px)
}

/// The 16 × 16 layer grid of 0x460EC0(15, …) / 0x460F40, normalised by `norm` like the horizon
/// ring, with the base UVs of 0x45EC40 (`j/a`, `i/b`), as the 15 × 15 quads' triangles.
fn layer_grid(ring: &nfs_formats::sky::SkyRing, scale: f32, a: f32, b: f32, norm: f32) -> MeshData {
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
            let x = (c * v + sn * u) * scale * norm;
            let y = (ring.height - ring.small * (u * u + v * v)) * scale * norm;
            let z = (c * u - sn * v) * scale * norm;
            m.vertices.push(Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [j as f32 / a, i as f32 / b], color: [1.0; 4] });
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

const RADIUS: f32 = 3000.0;

/// The horizon ring of `Sky/<Track>.bin`: sixteen quads, each one tile of the `horz` grid, with the
/// four `hzcl` colours of its two edges. `colour(i)` is the strip's pixel `i` on row 0 (the top) and
/// row 1 (the bottom); the original reads them as two runs of sixteen dwords 0x40 bytes apart
/// (0x459D8F / 0x459DE5), which is exactly the two rows of the 16 × 2 image.
fn horizon_ring(bin: &nfs_formats::sky::SkyBin, y0: f32, y1: f32, colour: impl Fn(usize, usize) -> [f32; 4]) -> MeshData {
    let mut m = MeshData::default();
    // the file's radius is in metres (500 … 1500) but the whole sky is rebuilt at `RADIUS` and then
    // scaled into the camera's far plane, so the ring is normalised here and its heights with it —
    // the shape the data asks for is the ratio, not the absolute size
    let k = RADIUS * 0.9 / bin.rings[0].radius.max(1.0);
    let (r, y0, y1) = (RADIUS * 0.9, y0 * k, y1 * k);
    for k in 0..16usize {
        let uv = bin.segment_uv(k);
        let c = bin.segment_colour(k);
        let p = |j: usize| {
            // the original steps 1/16 of a turn per vertex (0x3D800000 = 0.0625, 0x45A03D)
            let az = (k + j) as f32 / 16.0 * std::f32::consts::TAU;
            (az.cos() * r, az.sin() * r)
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
        let sky_mat = |r: &mut Renderer, id, blend| r.create_material(MaterialDesc { texture: Some(id), alpha_blend: blend, cull_back: false, depth_test: false, env_texture: None, env_mix: 0.0, env_variant: 0, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false });

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
            let bin = track.sky_bin.as_ref().filter(|b| b.rings[0].radius > 0.0);
            let multi = matches!(b.kind, nfs_formats::sky::SkyKind::Multi { .. });
            for (i, layer) in region.layers.iter().enumerate() {
                let Some(map) = layer.map.as_deref().and_then(find) else { continue };
                // region B's own map, blended in by `t` (MultiSky only — BasicSky keeps region A's)
                let map_b = region_b.layers.get(i).and_then(|l| l.map.as_deref()).filter(|n| multi && Some(*n) != layer.map.as_deref()).and_then(find);
                let blend = map_b.filter(|mb| mb.width == map.width && mb.height == map.height && bin.is_some()).and_then(|mb| {
                    let (a, b) = (map.mips.first()?.clone(), mb.mips.first()?.clone());
                    let texture = r.upload_texture(&TextureData { width: map.width, height: map.height, mips: vec![a.clone()] });
                    Some(MapBlend { texture, width: map.width, height: map.height, a, b, wb: 0 })
                });
                let id = match &blend {
                    Some(bl) => bl.texture,
                    None => tex(r, map),
                };
                let m = sky_mat(r, id, i > 0);
                let scale = [layer.scale[0].max(0.01), layer.scale[1].max(0.01)];
                // with no usable `.bin` the layer is not drawn: the loader falls back to
                // `default.bin` (0x459680 / 0x4596A1, as `TrackAssets` does), so only a broken
                // install gets here, and what the original then does was not read. *(Until
                // 2026-09-25 the port drew an invented dome scrolled by an untraced wind rule.)*
                if let Some(bin) = bin {
                    {
                        let ring = &bin.rings[0];
                        // normalised as the horizon ring is, the higher layers a little closer
                        let norm = RADIUS * 0.9 / ring.radius * (1.0 - 0.02 * i as f32);
                        let mesh = layer_grid(ring, bin.scalar_ce4, scale[0], scale[1], norm);
                        // 0x45F1B0: both regions' numbers for this stage; the wind flag when either
                        // region's speed is over 0.25, the parallax when either move ratio is over 1
                        let lb = region_b.layers.get(i);
                        let a = StageParams::of(Some(layer), region.move_ratio);
                        let bp = StageParams::of(lb, region_b.move_ratio);
                        let wind_on = layer.wind_speed > 0.25 || lb.is_some_and(|l| l.wind_speed > 0.25);
                        let parallax_on = region.move_ratio > 1.0 || region_b.move_ratio > 1.0;
                        let handle = r.upload_dynamic_mesh(&mesh);
                        grids.push(GridLayer { mesh: handle, vertices: mesh.vertices, a, b: bp, wind_on, parallax_on, angle: ring.angle_turns, step: a.step, offset: [0.0, 0.0], blend });
                        parts.push((handle, m, Motion::Grid(grids.len() - 1)));
                    }
                }
            }
        }
        // the horizon's texture `+0x44` is `hor<level>` when the pack has one and `horz` otherwise
        // (0x459C1F…0x459C4D) — the existence test before it (0x459B8D…0x459BB7) asks the other
        // way round, but either name keeps the ring. At the shipped level 0 `Industrial` and
        // `foothills` carry a `hor0`, and `Canyon` a `hor1` / `hor2`; the port had always taken `horz`.
        if let Some(hz) = find(&format!("hor{level}")).or_else(|| find("horz")) {
            let id = tex(r, hz);
            let m = sky_mat(r, id, true);
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
                    parts.push((r.upload_mesh(&horizon_ring(bin, y0, y1, colour)), m, Motion::Still));
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
            if let Some(bl) = g.blend.as_mut() {
                let (wb, px) = blend_maps(&bl.a, &bl.b, t);
                if wb != bl.wb {
                    bl.wb = wb;
                    r.update_texture(bl.texture, &TextureData { width: bl.width, height: bl.height, mips: vec![px] });
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

    /// Draw items centred on the camera (sky follows the viewer). `far` is the camera's far plane —
    /// the sky is scaled to sit just inside it, since the draw distance the track's `.key` file
    /// asks for is far shorter than the sky's nominal radius.
    pub fn draw_items(&self, camera_pos: Vec3, far: f32) -> impl Iterator<Item = DrawItem> + '_ {
        let k = (far * 0.9 / RADIUS).clamp(0.02, 4.0);
        let t = glam::Mat4::from_translation(camera_pos) * glam::Mat4::from_scale(Vec3::splat(k));
        self.parts.iter().map(move |(mesh, material, motion)| {
            let mut item = DrawItem::new(*mesh, *material, t);
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
        assert_eq!(blend_maps(&a, &b, 0.5), (32768, vec![100, 100, 127, 255]));
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
