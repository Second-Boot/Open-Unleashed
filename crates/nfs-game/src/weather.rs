//! The weather layer `[0x627E08]` — the snow and rain around the camera (`RE/notes/particles.md` §7).
//!
//! Two emitters built by 0x458C90 (constructor 0x456ED0), primed at the track load by 0x459260 and run
//! by **0x459020** at the end of every scene draw: each frame both are stepped by the ticks since the
//! last frame (0x4567D0) with a strength out of the sky `.bin`'s track B at the camera's slice — the
//! snow one field 7 `+0x20` (0x45CFD0 → 0x45A940), the rain one field 5 `+0x18` (0x45D000 →
//! 0x45A8E0) — and drawn (0x4564E0) in the main race view. Nothing else gates it: not `WEATHER`, not
//! the particle budget; `SPECIALFX` only sets the density `[0x6197F0]` and the texture index
//! `[0x6197F4]`. Shipped, only **Alps** has snow and only **Autobahn** rain.
//!
//! The particles live in **camera space** (x right, y up, z forward) inside a box ahead of the eye:
//! each step carries them along with the camera's own motion (`R · p + rotate(eyePrev − eyeCur +
//! drift)`), so they stand still in the world but for the drift, and wraps those that leave the box
//! to its other side. Each is one textured triangle, half of the unit square in UV
//! ([`Emitter::triangles`], 0x4565F0 / 0x456790), drawn additive (pair 1), unlit, without depth
//! writes.
//!
//! The third member is the windscreen drops ([`Splat`], 0x4577A0 / 0x4582C0 / 0x457E60): drawn from
//! the player's car in the driver view only, on the plane of the car's `Wndo` corners.
//!
//! The draws from 0x5561D0 / 0x538040 — the emitters' flutter timers and gusts, the drops' points
//! and alphas — come from the front end's generator itself (`opponents::FeRng`), the game's **one**
//! state, which the caller hands in: the race's, seeded from `time(0)` by the HUD's init and shared
//! with the sky's jitter and the announcer (`App::fe_rng` in `porsche`), drawn in the original's call
//! order (the prime, then per frame snow, rain, splat). The constructor's draws (0x456F5F, at the
//! track load 0x469092) come from the state the track load's 0x44F240 left, which that reseed
//! replaces and the prime's reset draws again: they leave no trace, and are made from a state of
//! their own here. *(Until 2026-09-26 the emitters drew from a stand-in, the race's 16-bit LCG
//! form, and the splat from a state of its own; then all three from one state of the layer's own
//! starting at the image's `.data` values.)*

use glam::{Mat3, Vec3};

use crate::track_particles::ParticleRng;

/// The particles an emitter can hold: 1203 colour / position slots are made for 401 alive bytes
/// (0x456F97, 0x456F43), 400 triangles.
pub const CAPACITY: usize = 400;

/// The 0x54-byte block 0x458C90 hands the constructor (copied to `+0x8604`).
#[derive(Debug, Clone, PartialEq)]
pub struct Params {
    /// `+0x00 / +0x0C / +0x18`, `+0x04 / +0x10 / +0x1C`, `+0x08 / +0x14 / +0x24` — the box in camera
    /// space: min, max and the range the births draw over (`+0x20` has no reader)
    pub min: Vec3,
    pub max: Vec3,
    pub range: Vec3,
    /// `+0x28` — `(float)[0x6197F0]`, `SPECIALFX`'s `F0`
    pub density: f32,
    /// `+0x2C`, `+0x30` — the triangle's width across and length along the world's down
    pub width: f32,
    pub length: f32,
    /// `+0x34…+0x3C`, `+0x40…+0x48` — the drift per tick, calm and in a gust (world)
    pub drift: Vec3,
    pub gust: Vec3,
    /// `+0x4C`: 0x44D320 by `<name><F4>` (0x5CEEB8 / 0x5CEEC8), else by `<fallback>`
    pub texture: &'static str,
    pub fallback: &'static str,
    /// `+0x50` — the flutter of 0x457062
    pub flutter: bool,
}

impl Params {
    /// The first emitter, `[+4]` (0x458CA1…0x458DEE).
    pub fn snow(density: f32) -> Self {
        Self {
            min: Vec3::new(-15.0, -3.0, 1.0),
            max: Vec3::new(15.0, 10.0, 30.0),
            range: Vec3::new(30.0, 13.0, 29.0),
            density,
            width: 0.2,
            length: 0.2,
            drift: Vec3::new(0.01, -0.04, 0.0),
            gust: Vec3::new(0.02, -0.02, 0.15),
            texture: "sno",
            fallback: "snow",
            flutter: true,
        }
    }
    /// The second, `[+8]` (0x458DF1…0x458F45).
    pub fn rain(density: f32) -> Self {
        Self {
            min: Vec3::new(-15.0, -3.0, 2.5),
            max: Vec3::new(15.0, 10.0, 15.0),
            range: Vec3::new(30.0, 13.0, 12.5),
            density,
            width: 0.2,
            length: 0.5,
            drift: Vec3::new(0.0, -0.8, 0.0),
            gust: Vec3::new(0.0, -0.32, 0.0),
            texture: "ran",
            fallback: "rain",
            flutter: false,
        }
    }
    /// The texture names 0x458C90 asks for, in order, at `SPECIALFX` level `level` (`F4` = the
    /// level): `sno<F4>` / `ran<F4>`, then `snow` / `rain`. No shipped pack has the first.
    pub fn texture_names(&self, level: usize) -> [String; 2] {
        [format!("{}{}", self.texture, level.min(3)), self.fallback.to_string()]
    }
}

/// `[0x6197F0]` by `SPECIALFX` level (0x44ED10's table, `F0`).
pub fn density(level: usize) -> f32 {
    [60.0, 140.0, 220.0, 300.0][level.min(3)]
}

/// The camera as 0x4567D0 takes it from `[[0x628130]+0x10]+0xB8`: its rotation (world to camera,
/// rows right, up, forward) and its eye.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub to_camera: Mat3,
    pub eye: Vec3,
}

impl View {
    /// From the eye and the camera's forward and up (the world is left-handed: right = up × forward).
    pub fn new(eye: Vec3, forward: Vec3, up: Vec3) -> Self {
        let f = forward.normalize_or_zero();
        let r = up.cross(f).normalize_or_zero();
        let u = f.cross(r);
        Self { to_camera: Mat3::from_cols(r, u, f).transpose(), eye }
    }
    /// A camera-space point in the world.
    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.eye + self.to_camera.transpose() * p
    }
}

/// The front end's generator 0x538040 (`career.md`), which 0x5561D0 wraps.
pub use crate::opponents::FeRng;

/// 0x5561D0: `(0x538040() & 0xFFFF) / 65536`.
fn unit(fe: &mut FeRng) -> f32 {
    (fe.next_u32() & 0xFFFF) as f32 * (1.0 / 65536.0)
}

/// One camera-space vertex of a frame: position and UV.
pub type Vertex = (Vec3, [f32; 2]);

#[derive(Debug, Clone)]
pub struct Emitter {
    pub params: Params,
    /// `+0`, `+4`, `+8`
    strength: f32,
    count: usize,
    target: usize,
    /// `+0xC + 0x30·i` (the first of each triangle's three vertices), `+0x83A0`
    pos: Vec<Vec3>,
    alive: Vec<bool>,
    /// `+0x8534`, `+0x8554`, `+0x85B4`
    timers: [i32; 8],
    wind: [Vec3; 8],
    last_sin: f32,
    /// `+0x85C4`, `+0x85C8`
    gust: f32,
    gust_target: f32,
    /// `+0x85FC` with `+0x85CC` / `+0x85F0`
    prev: Option<View>,
    /// `+0x8600`
    speed2: f32,
    /// `+0x8658`: `sin(2π·i/128)` (0x5327E0)
    sin: [f32; 128],
    /// the triangles 0x4565F0 left, `count · 3` vertices
    verts: Vec<Vertex>,
}

impl Emitter {
    /// 0x456ED0: nothing alive, the flutter timers drawn (0x456F5F…).
    pub fn new(params: Params, fe: &mut FeRng) -> Self {
        let mut sin = [0.0; 128];
        for (i, s) in sin.iter_mut().enumerate() {
            *s = (std::f32::consts::TAU * (i as f32 * 0.0078125)).sin();
        }
        let mut me = Self {
            params,
            strength: 0.0,
            count: 0,
            target: 0,
            pos: vec![Vec3::ZERO; CAPACITY],
            alive: vec![false; CAPACITY + 1],
            timers: [0; 8],
            wind: [Vec3::ZERO; 8],
            last_sin: 0.0,
            gust: 0.0,
            gust_target: 0.0,
            prev: None,
            speed2: 0.0,
            sin,
            verts: Vec::new(),
        };
        me.draw_timers(fe);
        me
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// The camera-space triangles of the last step (0x4565F0), three vertices a particle.
    pub fn triangles(&self) -> &[Vertex] {
        &self.verts
    }

    fn draw_timers(&mut self, fe: &mut FeRng) {
        for k in 0..8 {
            // `ftol(0x5561D0() · 1000)` — truncation
            self.timers[k] = (unit(fe) * 1000.0) as i32;
            self.wind[k] = Vec3::ZERO;
        }
    }

    /// `rand · range / 65536 + min` (0x459510 / 0x459560 / 0x4595B0 and their inline copies).
    fn uniform(rng: &mut ParticleRng, min: f32, range: f32) -> f32 {
        rng.next() as f32 * range * (1.0 / 65536.0) + min
    }

    /// `+0 = s`, `+8 = fistp(s · density − 0.5)`, below 3 → 0 (0x4568F2…0x45691A) — only when `s`
    /// differs from the strength held.
    fn set_strength(&mut self, s: f32) {
        if s == self.strength {
            return;
        }
        self.strength = s;
        let t = (s * self.params.density - 0.5).round_ties_even() as i32;
        // the shipped densities keep it under 300; the clamp only guards the arrays
        self.target = if t < 3 { 0 } else { (t as usize).min(CAPACITY) };
    }

    /// The reset (0x456B37…0x456CB2, 0x4575FF…, 0x459296…): nothing alive, the timers drawn again,
    /// the flutter and the speed zeroed, `count = target` and every one alive and uniform in the box.
    fn reset(&mut self, rng: &mut ParticleRng, fe: &mut FeRng) {
        self.alive.iter_mut().for_each(|a| *a = false);
        self.draw_timers(fe);
        self.last_sin = 0.0;
        self.speed2 = 0.0;
        self.count = self.target;
        let p = self.params.clone();
        for i in 0..self.count {
            self.alive[i] = true;
            let x = Self::uniform(rng, p.min.x, p.range.x);
            let y = Self::uniform(rng, p.min.y, p.range.y);
            let z = Self::uniform(rng, p.min.z, p.range.z);
            self.pos[i] = Vec3::new(x, y, z);
        }
    }

    /// 0x459260 for this emitter: the strength at load, then the reset.
    pub fn prime(&mut self, s: f32, rng: &mut ParticleRng, fe: &mut FeRng) {
        self.set_strength(s);
        self.reset(rng, fe);
    }

    /// Where a dead particle is parked: one range below the box (0x456D11…, 0x4575A0…).
    fn park(&mut self, i: usize) {
        let p = &self.params;
        self.alive[i] = false;
        self.pos[i] = p.min - p.range;
    }

    /// 0x456D70: particle `i` born — at the far end when the camera runs fast, else on top.
    fn birth(&mut self, i: usize, rng: &mut ParticleRng) {
        let p = self.params.clone();
        self.alive[i] = true;
        let x = Self::uniform(rng, p.min.x, p.range.x);
        self.pos[i] = if self.speed2 > 2025.0 {
            let y = Self::uniform(rng, p.min.y, p.range.y);
            Vec3::new(x, y, p.max.z)
        } else {
            let z = Self::uniform(rng, p.min.z, p.range.z);
            Vec3::new(x, p.max.y, z)
        };
        self.count += 1;
    }

    /// 0x456B00: the count toward the target.
    fn grow(&mut self, rng: &mut ParticleRng, fe: &mut FeRng) {
        let (count, target) = (self.count, self.target);
        if target > 4 * count && self.speed2 > 10000.0 {
            self.reset(rng, fe);
        } else if target > count {
            for i in count..target.min(count + 20) {
                self.birth(i, rng);
            }
        } else if target < count {
            if target == 0 && self.speed2 > 2025.0 {
                for i in 0..count {
                    self.park(i);
                }
            }
            while self.count > 0 && self.count - 1 >= target && !self.alive[self.count - 1] {
                self.count -= 1;
            }
        }
    }

    /// 0x4567D0(n, s): one frame of `n` ticks with strength `s`, under the camera `view`; `frozen`
    /// is 0x459020's first branch (the HUD's dialog queue up outside a replay), which also takes the
    /// drift and the flutter away.
    pub fn step(&mut self, n: u32, s: f32, view: View, frozen: bool, rng: &mut ParticleRng, fe: &mut FeRng) {
        let nf = n as f32;
        // 1. the camera's turn and move since the last frame
        let (r, d) = match self.prev {
            None => (Mat3::IDENTITY, Vec3::ZERO),
            Some(p) => (view.to_camera * p.to_camera.transpose(), p.eye - view.eye),
        };
        self.prev = Some(view);
        // 2. its horizontal speed², `× 4096 / k²` (0x5CEE14), k = n up to 10
        let k = n.clamp(1, 10) as f32;
        self.speed2 = (d.x * d.x + d.z * d.z) * (4096.0 / (k * k));
        // 3., 4.
        self.set_strength(s);
        self.grow(rng, fe);
        // 5. the gust and the drift
        let mut disp = Vec3::ZERO;
        if self.count > 0 {
            if self.gust < self.gust_target {
                self.gust += nf * 0.004;
                if self.gust > self.gust_target {
                    self.gust_target = 0.0;
                }
            } else if self.gust > self.gust_target {
                self.gust -= nf * 0.0015;
                if self.gust < self.gust_target {
                    self.gust = 0.0;
                    self.gust_target = 0.0;
                }
            } else if nf * 0.0015 > unit(fe) {
                self.gust_target = 1.0;
            }
            let v = if frozen {
                Vec3::ZERO
            } else if self.gust > 0.0 {
                self.params.drift * ((1.0 - self.gust) * nf) + self.params.gust * (nf * self.gust)
            } else {
                self.params.drift * ((1.0 - self.gust) * nf)
            };
            disp = view.to_camera * (d + v);
        }
        // 6. 0x457040
        self.carry(r, disp, n, frozen, rng, fe);
        // 7. 0x4565F0
        self.build(view);
    }

    /// 0x457040(R, disp, n).
    fn carry(&mut self, r: Mat3, disp: Vec3, n: u32, frozen: bool, rng: &mut ParticleRng, fe: &mut FeRng) {
        if self.count < 1 {
            return;
        }
        let p = self.params.clone();
        if p.flutter {
            for k in 0..8 {
                self.timers[k] = self.timers[k].wrapping_add((n << 7) as i32);
                // `((t << 7) / 65536) % 128`, C's signed division and remainder; past 2^24 (34
                // minutes of ticks) the original's index goes negative and reads before its table —
                // the port wraps it into the table
                let idx = (self.timers[k].wrapping_shl(7) / 65536) % 128;
                let s = self.sin[idx.rem_euclid(128) as usize];
                let y = s - self.last_sin;
                self.wind[k] = Vec3::new(y * p.drift.x * self.gust * 9.0, y, y * p.drift.z * self.gust * 9.0);
                self.last_sin = s;
            }
        }
        let count = self.count;
        for i in 0..count {
            if !self.alive[i] && self.target > count {
                self.alive[i] = true;
            }
            if self.alive[i] {
                self.pos[i] = r * self.pos[i] + disp;
                if p.flutter && !frozen {
                    self.pos[i] += self.wind[i & 7];
                }
            }
        }
        // the wrap
        let mut wrapped_n = 0.0f32;
        for i in 0..count {
            if !self.alive[i] {
                continue;
            }
            let mut q = self.pos[i];
            let mut wrapped = false;
            if q.z > p.max.z {
                q.z = p.min.z;
                q.x = Self::uniform(rng, p.min.x, p.range.x);
                q.y = Self::uniform(rng, p.min.y, p.range.y);
                wrapped = true;
            } else if q.z < p.min.z {
                q.z = p.max.z;
                q.x = Self::uniform(rng, p.min.x, p.range.x);
                q.y = Self::uniform(rng, p.min.y, p.range.y);
                wrapped = true;
            }
            if q.x > p.max.x {
                q.x = p.min.x;
                q.y = Self::uniform(rng, p.min.y, p.range.y);
                q.z = Self::uniform(rng, p.min.z, p.range.z);
                wrapped = true;
            } else if q.x < p.min.x {
                q.x = p.max.x;
                q.y = Self::uniform(rng, p.min.y, p.range.y);
                q.z = Self::uniform(rng, p.min.z, p.range.z);
                wrapped = true;
            }
            if q.y > p.max.y {
                q.y = (p.min.y as f64 + 0.01) as f32;
                q.x = Self::uniform(rng, p.min.x, p.range.x);
                q.z = Self::uniform(rng, p.min.z, p.range.z);
                wrapped = true;
            } else if q.y < p.min.y {
                q.y = (p.max.y as f64 - 0.01) as f32;
                q.x = Self::uniform(rng, p.min.x, p.range.x);
                q.z = Self::uniform(rng, p.min.z, p.range.z);
                wrapped = true;
            }
            self.pos[i] = q;
            if wrapped {
                wrapped_n += 1.0;
                if i >= self.target {
                    self.park(i);
                }
            }
        }
        if self.count > 20 && wrapped_n > count as f32 * 0.5 {
            self.reset(rng, fe);
        }
    }

    /// 0x4565F0: world down in camera space `a`, `b = cross((0, 0, −1), a)`, `P = (b.x·w, b.y·w,
    /// b.z)`, `Q = (a.x·l, a.y·l, a.z)` — z not scaled, as read — and per pair of particles `c, c + P
    /// + Q, c + Q` / `c, c + P, c + P + Q`, with the UVs 0x456790 set: `(0,0) (1,1) (0,1)` / `(0,0)
    /// (1,0) (1,1)`.
    fn build(&mut self, view: View) {
        self.verts.clear();
        if self.count < 1 {
            return;
        }
        let a = view.to_camera * Vec3::new(0.0, -1.0, 0.0);
        let b = Vec3::new(0.0, 0.0, -1.0).cross(a);
        let pv = Vec3::new(b.x * self.params.width, b.y * self.params.width, b.z);
        let qv = Vec3::new(a.x * self.params.length, a.y * self.params.length, a.z);
        for i in 0..self.count {
            let c = self.pos[i];
            if i % 2 == 0 {
                self.verts.extend([(c, [0.0, 0.0]), (c + pv + qv, [1.0, 1.0]), (c + qv, [0.0, 1.0])]);
            } else {
                self.verts.extend([(c, [0.0, 0.0]), (c + pv, [1.0, 0.0]), (c + pv + qv, [1.0, 1.0])]);
            }
        }
    }
}

/// The splat object's slots: 201 triangles of three vertices (0x4580E0 / 0x458150 / 0x457869 all
/// run to 0xC9), one drop a triangle.
pub const SPLAT_SLOTS: usize = 201;

/// 0x458C90's quad for the splat plane until the first draw hands it the car's `Wndo` corners
/// (0x458F55…0x458FCD, `{x, y, z, 1}` each): the model frame, front at −z.
pub const SPLAT_DEFAULT_QUAD: [Vec3; 4] = [
    Vec3::new(1.0, 1.4, -0.55),
    Vec3::new(-1.0, 1.4, -0.55),
    Vec3::new(-1.0, 0.8, -0.95),
    Vec3::new(1.0, 0.8, -0.95),
];

/// One stage of a drop's life, the table at **0x5CEE40** (six dwords a row, 0x458750): from `age`
/// ticks on, the drop's length and half-width run toward the *next* row's halves at its `rate`,
/// and its alpha toward the next row's `alpha` at `alpha_rate` a tick.
#[derive(Debug, Clone, Copy)]
struct SplatStage {
    age: u32,
    len: f32,
    width: f32,
    rate: f32,
    alpha: u32,
    alpha_rate: u32,
}

const SPLAT_STAGES: [SplatStage; 5] = [
    SplatStage { age: 20, len: 0.05, width: 0.05, rate: 0.2, alpha: 0xFF, alpha_rate: 1 },
    SplatStage { age: 40, len: 0.025, width: 0.025, rate: 0.2, alpha: 0xFF, alpha_rate: 1 },
    SplatStage { age: 60, len: 0.025, width: 0.025, rate: 0.03, alpha: 0xFF, alpha_rate: 1 },
    SplatStage { age: 400, len: 0.025, width: 0.025, rate: 0.03, alpha: 0, alpha_rate: 1 },
    SplatStage { age: 100_000, len: 0.0, width: 0.0, rate: 0.0, alpha: 0, alpha_rate: 1 },
];

/// The plane 0x457920 builds out of four corners — the glass the drops sit on, in the car model's
/// frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct SplatPlane {
    /// `+0x6750` `normalize((c1 − c0) × (c1 − c2))`
    pub normal: Vec3,
    /// `+0x675C` the corners' mean
    pub centre: Vec3,
    /// `+0x6768` `c0.x − c1.x` (the top edge), `+0x676C` `|(c1 − c2).yz|` (the side)
    pub width: f32,
    pub height: f32,
    /// `+0x6770` `normalize(normal × u)` — up the glass; `+0x677C` `u = (−1, 0, 0)`
    pub v: Vec3,
    pub u: Vec3,
    /// `+0x6788` `centre − ½·height·v`, `+0x6794` `centre − ¼·height·v`
    pub origin: Vec3,
    pub origin_quarter: Vec3,
    /// `+0x67A0` / `+0x67A4` `centre.y ± ½·v.y·height`, `+0x67A8` / `+0x67AC` `centre.x ∓ ½·u.x·width`
    pub y_hi: f32,
    pub y_lo: f32,
    pub x_hi: f32,
    pub x_lo: f32,
    /// `+0x67B0` `min(width · height / 1.3, 1)` — scales the drop count
    pub area: f32,
}

impl SplatPlane {
    /// 0x457920: the corners sorted (0x4579D2…0x457BEC — two on top by y with an epsilon of 0.08
    /// `[0x5B3E44]`, then each pair by x), their mean, the normal, the two edge lengths and the
    /// basis, the bounds and the two origins.
    pub fn new(quad: [Vec3; 4]) -> Self {
        let mut c = quad;
        let eps = 0.08f32;
        if c[0].y + eps < c[2].y {
            c.swap(0, 2);
        } else if c[0].y + eps < c[3].y {
            c.swap(0, 3);
        }
        if c[1].y + eps < c[3].y {
            c.swap(1, 3);
        }
        if c[0].x < c[1].x {
            c.swap(0, 1);
        }
        if c[3].x < c[2].x {
            c.swap(2, 3);
        }
        let centre = (c[0] + c[1] + c[2] + c[3]) * 0.25;
        let normal = (c[1] - c[0]).cross(c[1] - c[2]);
        let normal = normal * (1.0 / normal.length());
        let width = c[0].x - c[1].x;
        let (dy, dz) = (c[1].y - c[2].y, c[1].z - c[2].z);
        let height = (dy * dy + dz * dz).sqrt();
        // `fst +0x67B0; fcomp 1.0` — above one it is one
        let area = height * width * 0.769_230_8;
        let area = if area > 1.0 { 1.0 } else { area };
        let u = Vec3::new(-1.0, 0.0, 0.0);
        let v = normal.cross(u);
        let v = v * (1.0 / v.length());
        let h = v.y * height;
        let w = u.x * width;
        Self {
            normal,
            centre,
            width,
            height,
            v,
            u,
            origin: Vec3::new(centre.x - v.x * height * 0.5, centre.y - h * 0.5, centre.z - v.z * height * 0.5),
            origin_quarter: Vec3::new(centre.x - v.x * height * 0.25, centre.y - h * 0.25, centre.z - v.z * height * 0.25),
            y_hi: centre.y + h * 0.5,
            y_lo: centre.y - h * 0.5,
            x_hi: centre.x - w * 0.5,
            x_lo: centre.x + w * 0.5,
            area,
        }
    }
}

/// The windscreen drops, the splat object `[0x627E08]+0xC` (constructor **0x4577A0**, 0x67E4
/// bytes; `RE/notes/particles.md` §7.5). Stepped by 0x459020 with both weather strengths
/// (**0x4582C0**), primed by 0x459260 (**0x4581A0**) and drawn from the player's car in the driver
/// view (0x43A490 → 0x459200 → **0x457E60**) as `count` triangles in the car model's frame, on the
/// plane 0x457920 builds from the car's `Wndo` corners.
///
/// A drop is born at a uniform point of the plane with a random alpha, points down the glass (up
/// it when the camera is upside down, away from the plane's quarter point above 80 km/h), grows,
/// shrinks and fades by the table 0x5CEE40 and is born again once its alpha is below 15 or it has
/// left the plane. It does not slide.
#[derive(Debug, Clone)]
pub struct Splat {
    /// `+0`, `+4`, `+8`
    strength: f32,
    count: usize,
    target: usize,
    /// `+0xC + 0x30·i`: each triangle's first vertex, the drop's point
    pos: Vec<Vec3>,
    /// `+0x3894 + 12·i`: its ARGB (the three vertices share it)
    colour: Vec<u32>,
    /// `+0x4200`, `+0x42CA` (a signed word)
    alive: Vec<bool>,
    age: Vec<i16>,
    /// `+0x445C` the direction the drop points, `+0x4DC8` the one it turns toward, `+0x5734`
    /// `dir × normal`
    dir: Vec<Vec3>,
    goal: Vec<Vec3>,
    side: Vec<Vec3>,
    /// `+0x60A0` the length along `dir`, `+0x63C4` the half-width along `side`
    len: Vec<f32>,
    half: Vec<f32>,
    /// `+0x66E8` the camera's eye at the last step (zero from the constructor)
    prev_eye: Vec3,
    /// `+0x66F8` above 80 km/h, `+0x66F9` the camera upside down, `+0x66FA` snow rather than rain
    fast: bool,
    inverted: bool,
    snowy: bool,
    /// `+0x6700`: the next draw rebuilds the plane from the corners it is handed
    dirty: bool,
    pub plane: SplatPlane,
}

impl Splat {
    /// 0x4577A0 with 0x458C90's quad: nothing alive, the plane from [`SPLAT_DEFAULT_QUAD`] and
    /// marked dirty, the per-drop arrays cleared (0x4580E0), every colour `0xFFFFFFFF` (0x457869).
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            strength: 0.0,
            count: 0,
            target: 0,
            pos: vec![Vec3::ZERO; SPLAT_SLOTS],
            colour: vec![0xFFFF_FFFF; SPLAT_SLOTS],
            alive: vec![false; SPLAT_SLOTS],
            age: vec![0; SPLAT_SLOTS],
            dir: vec![Vec3::ZERO; SPLAT_SLOTS],
            goal: vec![Vec3::ZERO; SPLAT_SLOTS],
            side: vec![Vec3::ZERO; SPLAT_SLOTS],
            len: vec![0.0; SPLAT_SLOTS],
            half: vec![0.0; SPLAT_SLOTS],
            prev_eye: Vec3::ZERO,
            fast: false,
            inverted: false,
            snowy: false,
            dirty: true,
            plane: SplatPlane::new(SPLAT_DEFAULT_QUAD),
        }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// 0x4580E0: every slot dead, its age, directions and sizes zero (not its point or colour).
    fn clear(&mut self) {
        for i in 0..SPLAT_SLOTS {
            self.alive[i] = false;
            self.dir[i] = Vec3::ZERO;
            self.goal[i] = Vec3::ZERO;
            self.side[i] = Vec3::ZERO;
            self.len[i] = 0.0;
            self.half[i] = 0.0;
            self.age[i] = 0;
        }
    }

    /// The common head of 0x4581A0 and 0x4582C0: `s = min(rain + snow, 1)`, the snow flag (rain at
    /// most ½ and snow at least ½), and when `s` differs from the strength held, the target
    /// `ftol(fistp(s · 150) · area)` (0x45826B / 0x458345).
    fn set_strength(&mut self, rain: f32, snow: f32) {
        let s = rain + snow;
        let s = if s > 1.0 { 1.0 } else { s };
        self.snowy = !(rain > 0.5) && !(snow < 0.5);
        if s != self.strength {
            self.strength = s;
            let whole = ((s as f64 * 150.0) as f32).round_ties_even() as i32;
            // 0x5A0F98, truncation; the 201 slots bound it (150 at most as shipped)
            let t = (whole as f64 * self.plane.area as f64) as i32;
            self.target = t.clamp(0, SPLAT_SLOTS as i32) as usize;
        }
    }

    /// 0x4581A0(rain, snow), the last thing 0x459260 does: the arrays cleared, the strength, and
    /// `count = target` drops alive at uniform points (0x4583F0 only — no direction, size or
    /// colour yet).
    pub fn prime(&mut self, rain: f32, snow: f32, fe: &mut FeRng) {
        self.clear();
        self.set_strength(rain, snow);
        self.count = self.target;
        for i in 0..self.count {
            self.alive[i] = true;
            self.place(i, fe);
        }
    }

    /// 0x4583F0: `origin + (rand · height)·v + (rand · width − ½·width)·u`, the first draw the
    /// across one.
    fn place(&mut self, i: usize, fe: &mut FeRng) {
        let p = self.plane;
        let across = unit(fe) * p.width - (p.width as f64 * 0.5) as f32;
        let up = unit(fe) * p.height;
        self.pos[i] = p.origin + p.v * up + p.u * across;
    }

    /// 0x4584F0: where the drop turns — away from the quarter point when fast (0x5328D0's
    /// normalise), up the glass when the camera is upside down, else down it.
    fn aim(&mut self, i: usize) {
        let p = self.plane;
        self.goal[i] = if self.fast {
            let d = self.pos[i] - p.origin_quarter;
            d * (1.0 / d.length())
        } else if self.inverted {
            p.v
        } else {
            -p.v
        };
    }

    /// 0x458490: placed, aimed, age 0, the colour `0x538040() | 0xFFFFFF` (a random alpha, white),
    /// and turned at once (0x4585D0 with 1).
    fn birth(&mut self, i: usize, fe: &mut FeRng) {
        self.place(i, fe);
        self.aim(i);
        self.age[i] = 0;
        self.colour[i] = fe.next_u32() | 0x00FF_FFFF;
        self.turn(i, true);
    }

    /// 0x4585D0(i, init): nothing while the direction already is the goal; else the goal itself
    /// on `init` or within 0.01 on every axis, a sixtieth of the way to it otherwise (per call,
    /// not per tick); then `side = dir × normal`.
    fn turn(&mut self, i: usize, init: bool) {
        let d = self.goal[i] - self.dir[i];
        if d.x == 0.0 && d.y == 0.0 && d.z == 0.0 {
            return;
        }
        if init || (d.x.abs() < 0.01 && d.y.abs() < 0.01 && d.z.abs() < 0.01) {
            self.dir[i] = self.goal[i];
        } else {
            let k = 1.0f32 / 60.0;
            let d = self.goal[i] - self.dir[i];
            self.dir[i] += d * k;
        }
        self.side[i] = self.dir[i].cross(self.plane.normal);
    }

    /// 0x458750(i, n, nf): the stage by age (the signed word read unsigned against 0x5CEE40's
    /// ages); the length and half-width run toward half the next row's at `rate · nf · ½`, the
    /// point stepping back half the length's change along the direction; the alpha toward the
    /// next row's by `alpha_rate · n`, clamped there — in unsigned 32-bit arithmetic whose wrap
    /// (a fade past zero) lands in the colour's top byte as the original's `shl 24` does.
    fn ramp(&mut self, i: usize, n: u32) {
        let age = self.age[i] as i32 as u32;
        let stage = (0..SPLAT_STAGES.len()).rev().find(|&k| age >= SPLAT_STAGES[k].age).map_or(0, |k| k + 1);
        // stage 4 (age 400…) is left alone; stage 5 would read past the table (0x5CEEB8, the snow
        // texture names) — a word age past 32767, which the fade (dead by about 315) never reaches
        if stage >= 4 {
            return;
        }
        let e = SPLAT_STAGES[stage];
        let nf = n as f32;
        let t = (e.len * 0.5 - self.len[i]) * e.rate * nf * 0.5;
        self.pos[i] -= self.dir[i] * (t as f64 * 0.5) as f32;
        self.len[i] += t;
        self.half[i] += (e.width * 0.5 - self.half[i]) * e.rate * nf * 0.5;
        let a = self.colour[i] >> 24;
        let step = e.alpha_rate.wrapping_mul(n);
        let a = if a > e.alpha {
            let a = a.wrapping_sub(step);
            if a < e.alpha { e.alpha } else { a }
        } else if a < e.alpha {
            let a = a.wrapping_add(step);
            if a > e.alpha { e.alpha } else { a }
        } else {
            return;
        };
        self.colour[i] = (self.colour[i] & 0x00FF_FFFF) | a.wrapping_shl(24);
    }

    /// 0x458390: toward the target — up to five births a call, or the dead tail trimmed.
    fn grow(&mut self, fe: &mut FeRng) {
        let (count, target) = (self.count, self.target);
        if target > count {
            for i in count..target.min(count + 5) {
                self.alive[i] = true;
                self.birth(i, fe);
                self.count += 1;
            }
        } else if target < count {
            let mut i = count as isize - 1;
            while i >= target as isize && !self.alive[i as usize] {
                self.count -= 1;
                i -= 1;
            }
        }
    }

    /// 0x4588D0(n): the camera's horizontal speed from its eye (`[cam+0x10C]`, `(bits >> 1) +
    /// 0x1FC00000` for the root, × `64 / min(n, 10)` 0x5CEDEC), capped at 160 km/h; fast above 80
    /// km/h, never while it snows; upside down when the camera's `m[1][1]` (`[cam+0xF8]`, its up
    /// axis' y) is not positive. A change of either re-aims every live drop. Then per live drop:
    /// age, ramp, turn, and born again (or, past the target, parked `5 · height` below the plane)
    /// once its point is off the plane or its alpha below 15.
    fn update(&mut self, n: u32, eye: Vec3, up_y: f32, fe: &mut FeRng) {
        if self.count < 1 {
            return;
        }
        let d = eye - self.prev_eye;
        self.prev_eye = eye;
        let s2 = d.x * d.x + d.z * d.z;
        let root = f32::from_bits(((s2.to_bits() as i32 >> 1) as u32).wrapping_add(0x1FC0_0000));
        let per = [64.0, 32.0, 21.333_334, 16.0, 12.8, 10.666_667, 9.142_858, 8.0, 7.111_111, 6.4];
        let speed = root * per[if n > 10 { 9 } else { n as usize - 1 }];
        let speed = if speed > 44.444_443 { 44.444_443 } else { speed };
        let fast = speed > 22.222_221 && !self.snowy;
        let inverted = up_y <= 0.0;
        for i in 0..self.count {
            if !self.alive[i] && self.target > self.count {
                self.alive[i] = true;
            }
        }
        if fast != self.fast || inverted != self.inverted {
            self.fast = fast;
            self.inverted = inverted;
            for i in 0..self.count {
                if self.alive[i] {
                    self.aim(i);
                }
            }
        }
        for i in 0..self.count {
            if !self.alive[i] {
                continue;
            }
            self.age[i] = self.age[i].wrapping_add(n as i16);
            self.ramp(i, n);
            self.turn(i, false);
            let (q, p) = (self.pos[i], self.plane);
            let on = q.y <= p.y_hi && q.y >= p.y_lo && q.x <= p.x_hi && q.x >= p.x_lo;
            if on && self.colour[i] >= 0x0F00_0000 {
                continue;
            }
            if i >= self.target {
                self.alive[i] = false;
                self.pos[i] = p.origin;
                self.pos[i].y = p.origin.y - p.height * 5.0;
            } else {
                self.birth(i, fe);
            }
        }
    }

    /// 0x4582C0(n, rain, snow), 0x459020's third step. `eye` and `up_y` are the camera's.
    /// (Its `+0x66FC` flip-flop and 0x516950 — an empty function — do nothing the drops see.)
    pub fn step(&mut self, n: u32, rain: f32, snow: f32, eye: Vec3, up_y: f32, fe: &mut FeRng) {
        self.set_strength(rain, snow);
        self.grow(fe);
        self.update(n, eye, up_y, fe);
    }

    /// `+0x6700`: the plane still waits for the car's corners.
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    /// 0x457E60's head: the plane rebuilt from `quad` (the car's first four `Wndo` points, model
    /// frame) the first time after the constructor, then kept.
    pub fn attach(&mut self, quad: [Vec3; 4]) {
        if self.dirty {
            self.dirty = false;
            self.plane = SplatPlane::new(quad);
        }
    }

    /// 0x457FD0 with 0x458150's UVs: per slot `i < count`, dead or alive, the triangle `P`, `P +
    /// len·dir − half·side`, `P + len·dir + half·side` in the model frame, UVs `(1,0) (1,1) (0,0)`
    /// on an even slot and `(0,1) (0,0) (1,1)` on an odd one, and the slot's ARGB.
    pub fn triangles(&self) -> Vec<(Vec3, [f32; 2], u32)> {
        let mut out = Vec::with_capacity(self.count * 3);
        for i in 0..self.count {
            let p = self.pos[i];
            let tip = p + self.dir[i] * self.len[i];
            let s = self.side[i] * self.half[i];
            let uv = if i % 2 == 0 { [[1.0, 0.0], [1.0, 1.0], [0.0, 0.0]] } else { [[0.0, 1.0], [0.0, 0.0], [1.0, 1.0]] };
            let c = self.colour[i];
            out.extend([(p, uv[0], c), (tip - s, uv[1], c), (tip + s, uv[2], c)]);
        }
        out
    }
}

/// `[0x627E08]`: `{lastTick, snow, rain, splat}`.
#[derive(Debug, Clone)]
pub struct Weather {
    last_tick: u32,
    pub snow: Emitter,
    pub rain: Emitter,
    pub splat: Splat,
}

impl Weather {
    /// 0x458C90 at `SPECIALFX` level `level`, then 0x459260 with the strengths at load, its draws
    /// from `fe` (see the module's note for the constructor's).
    pub fn new(level: usize, snow: f32, rain: f32, rng: &mut ParticleRng, fe: &mut FeRng) -> Self {
        let d = density(level);
        let mut load = FeRng::new(0);
        let (snow_e, rain_e) = (Emitter::new(Params::snow(d), &mut load), Emitter::new(Params::rain(d), &mut load));
        let mut me = Self { last_tick: 0, snow: snow_e, rain: rain_e, splat: Splat::new() };
        me.prime(snow, rain, rng, fe);
        me
    }

    /// **0x459260**, the prime — at the track load (0x4690AF from 0x469040's caller) and again at
    /// every pass of the session loop on the same object (0x4690A0 from 0x414EB0: a restart, the
    /// automatic replay): `+0` (the last tick) = 0 (0x459272), then each emitter's reset with its
    /// strength (the snow's from 0x45CFD0, the rain's from 0x45D000; `+8` recomputed only when the
    /// strength differs, `test ah, 0x40` at 0x4592F6 / 0x459478) and the drops' 0x4581A0. The
    /// emitters' gust and last view and the drops' plane, colours and last eye are not touched.
    pub fn prime(&mut self, snow: f32, rain: f32, rng: &mut ParticleRng, fe: &mut FeRng) {
        self.last_tick = 0;
        self.snow.prime(snow, rng, fe);
        self.rain.prime(rain, rng, fe);
        self.splat.prime(rain, snow, fe);
    }

    /// 0x459020's step: `frozen` (the dialog queue up, no replay) steps by one on every call;
    /// otherwise by the ticks since the last call, nothing when none passed.
    pub fn frame(&mut self, tick: u32, snow: f32, rain: f32, view: View, frozen: bool, rng: &mut ParticleRng, fe: &mut FeRng) {
        let n = if frozen {
            1
        } else {
            let n = tick.wrapping_sub(self.last_tick);
            self.last_tick = tick;
            if n < 1 {
                return;
            }
            n
        };
        self.snow.step(n, snow, view, frozen, rng, fe);
        self.rain.step(n, rain, view, frozen, rng, fe);
        // the splat only in the second branch (0x459112); `[cam+0xF8]` is the rotation's m[1][1]
        if !frozen {
            self.splat.step(n, rain, snow, view.eye, view.to_camera.col(1).y, fe);
        }
    }

    /// 0x459020's draw gate: either emitter holds particles.
    pub fn visible(&self) -> bool {
        self.snow.count() > 0 || self.rain.count() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View::new(Vec3::new(0.0, 1.0, 0.0), Vec3::Z, Vec3::Y)
    }

    #[test]
    fn target_from_strength_and_density() {
        let mut rng = ParticleRng::new(0);
        let w = Weather::new(3, 1.0, 0.0, &mut rng, &mut FeRng::new(0));
        // fistp(300 − 0.5) = 300 (ties to even) — the prime fills the box
        assert_eq!(w.snow.count(), 300);
        assert_eq!(w.rain.count(), 0);
        // below three is none (0x456912)
        let mut fe = FeRng::new(0);
        let mut e = Emitter::new(Params::rain(60.0), &mut fe);
        e.prime(0.04, &mut rng, &mut fe);
        assert_eq!(e.count(), 0);
    }

    #[test]
    fn primed_particles_are_in_the_box() {
        let mut rng = ParticleRng::new(7);
        let w = Weather::new(3, 0.0, 1.0, &mut rng, &mut FeRng::new(0));
        let p = &w.rain.params;
        for t in w.rain.pos.iter().take(w.rain.count()) {
            assert!(t.x >= p.min.x && t.x < p.max.x && t.y >= p.min.y && t.y < p.max.y && t.z >= p.min.z && t.z < p.max.z);
        }
    }

    #[test]
    fn a_still_camera_lets_the_rain_fall_and_wrap() {
        let mut rng = ParticleRng::new(3);
        let mut fe = FeRng::new(0);
        let mut w = Weather::new(3, 0.0, 1.0, &mut rng, &mut fe);
        w.frame(1, 0.0, 1.0, view(), false, &mut rng, &mut fe); // first frame: no camera delta
        let before: Vec<Vec3> = w.rain.pos[..w.rain.count()].to_vec();
        w.frame(2, 0.0, 1.0, view(), false, &mut rng, &mut fe);
        // −0.8 a tick down (0x458EE7), or wrapped to the top
        for (a, b) in before.iter().zip(&w.rain.pos) {
            let fell = (b.y - (a.y - 0.8)).abs() < 1e-4 && (b.x - a.x).abs() < 1e-5;
            let wrapped = (b.y - (10.0 - 0.01)).abs() < 1e-4;
            assert!(fell || wrapped, "{a} → {b}");
        }
        assert_eq!(w.rain.triangles().len(), w.rain.count() * 3);
    }

    #[test]
    fn a_moving_camera_leaves_the_flakes_in_the_world() {
        let mut rng = ParticleRng::new(5);
        let mut fe = FeRng::new(9);
        let mut e = Emitter::new(Params::rain(300.0), &mut fe);
        e.params.drift = Vec3::ZERO;
        e.params.gust = Vec3::ZERO;
        e.prime(1.0, &mut rng, &mut fe);
        let v0 = View::new(Vec3::ZERO, Vec3::Z, Vec3::Y);
        e.step(1, 1.0, v0, false, &mut rng, &mut fe);
        let i = (0..e.count()).find(|&i| e.pos[i].z > 5.0 && e.pos[i].z < 14.0 && e.pos[i].x.abs() < 10.0).unwrap();
        let world = v0.to_world(e.pos[i]);
        // half a metre forward and a small turn to the right
        let v1 = View::new(Vec3::new(0.0, 0.0, 0.5), Vec3::new(0.1, 0.0, 1.0), Vec3::Y);
        e.step(1, 1.0, v1, false, &mut rng, &mut fe);
        assert!((v1.to_world(e.pos[i]) - world).length() < 1e-3);
    }

    #[test]
    fn the_default_splat_plane() {
        let p = SplatPlane::new(SPLAT_DEFAULT_QUAD);
        assert!((p.centre - Vec3::new(0.0, 1.1, -0.75)).length() < 1e-6);
        assert_eq!(p.width, 2.0);
        assert!((p.height - 0.52f32.sqrt()).abs() < 1e-6);
        // 2 · 0.721 / 1.3 is past one
        assert_eq!(p.area, 1.0);
        // up the glass, toward the back
        assert!(p.v.y > 0.8 && p.v.z > 0.5 && p.v.x == 0.0);
        assert!((p.y_hi - 1.4).abs() < 1e-5 && (p.y_lo - 0.8).abs() < 1e-5);
        assert!((p.x_hi - 1.0).abs() < 1e-6 && (p.x_lo + 1.0).abs() < 1e-6);
        assert!((p.origin - Vec3::new(0.0, 0.8, -0.95)).length() < 1e-5);
    }

    #[test]
    fn the_993_windscreen_holds_72_drops() {
        // `Wndo` of the 993 in record order, front edge low (car-model.md)
        let quad = [Vec3::new(0.63, 0.90, -0.69), Vec3::new(-0.63, 0.90, -0.69), Vec3::new(-0.50, 1.26, -0.17), Vec3::new(0.50, 1.26, -0.17)];
        let p = SplatPlane::new(quad);
        // sorted: the top edge first, +x before −x
        assert_eq!(p.width, 1.0);
        let mut s = Splat::new();
        s.attach(quad);
        s.prime(1.0, 0.0, &mut FeRng::new(0));
        // fistp(150) · (1.0 · 0.632 / 1.3), truncated
        assert_eq!(s.count(), (150.0 * p.area as f64) as usize);
        assert_eq!(s.count(), 72);
    }

    #[test]
    fn drops_live_on_the_plane() {
        let mut fe = FeRng::new(7);
        let mut s = Splat::new();
        s.prime(0.0, 0.0, &mut fe);
        assert_eq!(s.count(), 0);
        let eye = Vec3::new(3.0, 1.0, 5.0);
        for t in 0..400u32 {
            // standing still after the first step, upright, in the rain
            s.step(if t == 0 { 1 } else { 2 }, 1.0, 0.0, eye, 1.0, &mut fe);
            assert!(s.count() <= 150);
        }
        assert_eq!(s.count(), 150);
        let p = s.plane;
        for i in 0..s.count() {
            if s.alive[i] {
                let q = s.pos[i];
                assert!(q.y <= p.y_hi + 1e-3 && q.y >= p.y_lo - 1e-3 && q.x <= p.x_hi && q.x >= p.x_lo, "{q}");
                // slow and upright: pointing down the glass
                assert!((s.goal[i] + p.v).length() < 1e-6);
                assert!(s.colour[i] >> 24 >= 15);
            }
        }
        assert_eq!(s.triangles().len(), s.count() * 3);
    }

    #[test]
    fn the_first_step_is_fast_but_not_in_snow() {
        // the eye held from the constructor is zero, so the first step reads a long way moved
        let mut fe = FeRng::new(1);
        let mut s = Splat::new();
        s.prime(1.0, 0.0, &mut fe);
        s.step(1, 1.0, 0.0, Vec3::new(100.0, 0.0, 0.0), 1.0, &mut fe);
        assert!(s.fast);
        let mut s = Splat::new();
        s.prime(0.0, 1.0, &mut fe);
        s.step(1, 0.0, 1.0, Vec3::new(100.0, 0.0, 0.0), 1.0, &mut fe);
        assert!(!s.fast && s.snowy);
    }

    #[test]
    fn the_gate_is_the_count() {
        let mut rng = ParticleRng::new(0);
        let mut fe = FeRng::new(0);
        assert!(!Weather::new(3, 0.0, 0.0, &mut rng, &mut fe).visible());
        assert!(Weather::new(0, 0.5, 0.0, &mut rng, &mut fe).visible());
        assert_eq!(Params::snow(0.0).texture_names(3), ["sno3".to_string(), "snow".to_string()]);
    }
}
