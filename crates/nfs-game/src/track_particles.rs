//! The track's particle systems — `RGenericParticleSystem` (vtable 0x5B3DF8) started by the kind-3
//! `ef` parts of the track's articles through 0x455150 (`RE/notes/particles.md` §6,
//! `crp-geometry.md` §8): the constructor 0x454250, the 64 Hz update 0x454C30 with the birth
//! 0x454860, and the draw 0x454430's billboards.
//!
//! A system's emission fades past `volume ×` the draw distance ([`emission_scale`],
//! 0x454C7B…0x454CD2) by the value its article was last drawn with, and a system lives only while
//! its article is drawn: made on the first draw, dropped 100 updates after the last
//! ([`TrackParticles::draw`] 0x455150, [`TrackParticles::step`] 0x4551E0) — "drawn" is the track
//! walk's (`crate::world::TrackWorld::article_draws`).

use glam::Vec3;
use nfs_formats::particle_dat::ParticleDef;

/// One kind-3 `ef` part: its data from `+8`.
#[derive(Debug, Clone, PartialEq)]
pub struct Emitter {
    /// the `.crp` index of the article carrying it, and the part's own index — 0x455150 keys the
    /// system by the part's address
    pub article: usize,
    pub part: u16,
    /// `+0…+8` in the world: the point the system emits from once it is running — the part's own
    /// `+0…+8` through its article's `tr` (for a `tr`-less article, the same numbers)
    pub position: Vec3,
    /// `+0…+8` as the part stores it, untransformed: where the constructor's prime updates birth
    /// (see [`TrackParticles::draw`], 0x455030)
    pub local: Vec3,
    /// `+0x10…+0x18`, `+0x1C…+0x24` as the part stores them — neither creator transforms them
    /// (0x455000 hands the constructor the part itself, 0x455030 copies it, 0x455069)
    pub dir_a: Vec3,
    pub dir_b: Vec3,
    /// `+0x28`
    pub tag: u32,
}

/// The generator the systems draw from, `[0x5CEAE0]` × `[0x5CEAE4]`: the same 16-bit LCG as the
/// race's (`crate::rng`) on a state of its own, seeded by 0x44FD00 with the race's `RAND_SEED`
/// (0x414EFB).
#[derive(Debug, Clone)]
pub struct ParticleRng(u32);

impl ParticleRng {
    pub fn new(seed: u32) -> Self {
        let mut s = 0x3ADE_68B1u32;
        for _ in 0..(seed % 500) {
            s = s.wrapping_mul(crate::rng::MUL) & 0xFFFF;
        }
        Self(s)
    }
    pub(crate) fn next(&mut self) -> u32 {
        let p = self.0.wrapping_mul(crate::rng::MUL);
        self.0 = p & 0xFFFF;
        (p >> 8) & 0xFFFF
    }
    /// `rand · r / 32768 − r` (0x4548AD…0x4548BA): uniform over `[−r, r)`.
    fn spread(&mut self, r: f32) -> f32 {
        self.next() as f32 * r * 3.051_757_8e-5 - r
    }
}

#[derive(Debug, Clone)]
struct Particle {
    /// `+0x04…`, `+0x14…`
    pos: Vec3,
    vel: Vec3,
    /// `+0x20`
    age: f32,
    /// `+0x24`, taken off `vel.y` each update
    gravity: f32,
    /// `+0x28`, the turn the draw scales by the particle's age
    turn: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct System {
    pub(crate) def: usize,
    /// the part the system reads through `+0x28`: its point `+0`, its two directions `+0x10` /
    /// `+0x1C` and its tag `+0x28` — a car's effect slot rewrites them every frame
    /// (`crate::car_effects`), so the births take the values of the update they happen in
    pub(crate) emitter: Emitter,
    /// `+0x30` — `cross(dir_a, dir_b)` (0x532880), made once by the constructor (0x454395)
    pub(crate) axis: Vec3,
    /// `+0x54` — `count × budget >> 8`
    pub(crate) max: usize,
    /// `+0x40`
    pub(crate) rate: f32,
    /// `+0x0C`, `+0x14`, `+0x18`, `+0x2C`
    pub(crate) tick: u32,
    pub(crate) spawned: f32,
    pub(crate) accumulated: f32,
    pub(crate) emitting: bool,
    /// `+0x2E & 2`: both sizes 0, no billboards
    pub(crate) no_sprites: bool,
    /// `+0x2E & 1` with `+0x4C` pointing at a car's `+0x330` (the sparks, 0x43EEF2…0x43EF03): the
    /// particles live in the car's frame of translation and the draw adds the car's position
    /// (0x4546A3…0x4546CA)
    pub(crate) relative: bool,
    /// `+0x44`, the emission's own factor: 1.0 from the constructor (0x454305), a car effect's
    /// strength (0x43F09C, 0x43F163, 0x43F2EB)
    pub(crate) strength: f32,
    particles: Vec<Particle>,
    /// whether the system exists — made on its article's first draw (0x455150)
    live: bool,
    /// `+8`: set to 100 by each draw, counted down by each update, dropped at 0 (0x4551E0)
    countdown: u32,
    /// `+0x10`: the value its article was last drawn with (0x4551BF) — the walk's square distance
    /// (`s²` of the sphere's near edge, or `d²` for a mid-ground article), rounded
    pub(crate) sqd: f32,
}

/// The constructor's and the retag's rate `+0x40` (0x454322…0x45437C, 0x454BC2…0x454C25): `n /
/// lifetime`, or with an off time and `lifetime ≥ on`: `n / on` when `lifetime ≤ on + off`, else
/// `n / ((lifetime / (on + off)) · on)` — `n` the definition's count in the constructor, the
/// budget's maximum in the retag.
pub(crate) fn rate_of(n: u32, d: &ParticleDef) -> f32 {
    let (on, off, life) = (d.period_on, d.period_off, d.lifetime);
    if off != 0 && life >= on {
        if life <= on + off { n as f32 / on as f32 } else { n as f32 / ((life as f32 / (on + off) as f32) * on as f32) }
    } else {
        n as f32 / life.max(1) as f32
    }
}

impl System {
    /// 0x454250 `(value, part, 1)` with no prime run: the definition `di`, `max = count × budget
    /// >> 8`, the rate, the axis, emitting (`+0x2C = 1`) unless flag 2.
    pub(crate) fn new(defs: &[ParticleDef], di: usize, emitter: Emitter, budget: u32) -> Self {
        let d = &defs[di];
        System {
            def: di,
            axis: emitter.dir_a.cross(emitter.dir_b),
            emitter,
            max: ((d.count as u64 * budget as u64) >> 8) as usize,
            rate: rate_of(d.count, d),
            tick: 0,
            spawned: 0.0,
            accumulated: 0.0,
            emitting: d.flags & 2 == 0,
            no_sprites: d.start_size == 0.0 && d.end_size == 0.0,
            relative: false,
            strength: 1.0,
            particles: Vec::new(),
            live: false,
            countdown: 0,
            sqd: 0.0,
        }
    }

    /// **0x454AC0**, the retag: nothing when the part already carries `tag`; else the part takes
    /// it (0x454AD6), the system the definition it names (0x454BEF…), its texture, `max = count ×
    /// budget >> 8` and the rate **from that maximum**, not from the count as the constructor
    /// (0x454BDD `fild`s the stack slot 0x454B64 stored the maximum in); every particle is freed
    /// only when the new definition's count differs from the old one's (0x454B62…0x454BB8) —
    /// `RDDT` ↔ `WTSP` (35 each) keep theirs. The tick, the accumulator and the emission are left
    /// alone.
    pub(crate) fn retag(&mut self, defs: &[ParticleDef], tag: u32, budget: u32) {
        if self.emitter.tag == tag {
            return;
        }
        self.emitter.tag = tag;
        // 0x455D20 finds the tag in the map; a missing one would be default-inserted (0x50E2C0) —
        // every tag the car effects hand it is in `carpart.dat`
        let Some(di) = defs.iter().position(|d| d.tag == tag) else { return };
        let d = &defs[di];
        let max = (d.count as u64 * budget as u64) >> 8;
        if d.count != defs[self.def].count {
            self.particles.clear();
        }
        self.max = max as usize;
        self.rate = rate_of(max as u32, d);
        self.def = di;
    }

    /// Slot 3, **0x454420**: `+0x2C = 1` and the period's tick `+0xC = 0`.
    pub(crate) fn start(&mut self) {
        self.emitting = true;
        self.tick = 0;
    }

    /// **0x454C30**, one update. `draw` is the draw distance for the emission fade, which goes by
    /// the system's `+0x10`.
    pub(crate) fn update(&mut self, defs: &[ParticleDef], rng: &mut ParticleRng, draw: f32) {
        let d = &defs[self.def];
        let s = self;
        if s.particles.is_empty() && !s.emitting {
            return;
        }
        let period = d.period_on + d.period_off;
        let off_part = d.period_off != 0 && s.tick > d.period_on;
        if s.emitting && !off_part {
            // 0x454C7B…0x454CD4: `acc += rate · (k · +0x44)`
            s.accumulated += s.rate * (emission_scale(s.sqd, draw, d.volume) * s.strength);
            while s.accumulated > s.spawned + 1.0 {
                if s.particles.len() < s.max {
                    let p = birth(rng, d, &s.emitter, s.axis);
                    s.particles.insert(0, p);
                }
                s.spawned += 1.0;
            }
            s.tick += 1;
        } else if s.tick <= period {
            s.tick += 1;
        } else {
            // 0x454DEA…0x454DF7: the period starts again, and a once-only system stops
            s.tick -= period;
            if d.flags & 2 != 0 {
                s.emitting = false;
            }
        }
        // flag 1's floor is the part's `+0xC` (0x454DB6…0x454DD2 `fcomp [part+0xC]`), which the
        // port does not carry: it keeps the emitter's height. No shipped definition has flag 1
        // (`particles.md` §11).
        let floor_y = s.emitter.position.y;
        let life = d.lifetime as f32;
        s.particles.retain_mut(|p| {
            p.pos += p.vel;
            if d.flags & 1 != 0 && p.pos.y < floor_y {
                p.pos.y = floor_y;
                p.vel.y *= -0.5;
            }
            p.vel.y -= p.gravity;
            if d.air_friction < 1.0 {
                p.vel.x *= d.air_friction;
            }
            // 0x454E24…0x454E35: `z` is taken by the friction whatever its value
            p.vel.z *= d.air_friction;
            p.age += 1.0;
            p.age <= life
        });
    }

    /// **0x454430**'s per-particle records: a billboard each unless `+0x2E & 2` (0x454781), and a
    /// streak each when the definition has flag 4 (0x45482F → 0x454EC0). `offset` is what a
    /// relative system's draw adds, its `+0x4C`'s point (0x4546A3…0x4546CA).
    pub(crate) fn draw_into(&self, defs: &[ParticleDef], offset: Vec3, sprites: &mut Vec<Sprite>, streaks: &mut Vec<Streak>) {
        let d = &defs[self.def];
        let offset = if self.relative { offset } else { Vec3::ZERO };
        for p in &self.particles {
            let t = p.age / d.lifetime.max(1) as f32;
            let argb = ramp(d, t);
            let position = p.pos + offset;
            if !self.no_sprites {
                sprites.push(Sprite {
                    material: d.material.clone(),
                    blend: d.blend,
                    zbias: d.zbias,
                    sqd: self.sqd,
                    position,
                    size: (t * (d.end_size - d.start_size) + d.start_size) * 10.0,
                    argb,
                    degrees: t * p.turn,
                });
            }
            if d.flags & 4 != 0 {
                // 0x4546F8…0x454739: the tail `pos − 3·vel` (`[0x5B2600]`)
                streaks.push(Streak { head: position, tail: position - p.vel * 3.0, argb, blend: d.blend, zbias: d.zbias, sqd: self.sqd });
            }
        }
    }
}

/// One streak of 0x454EC0: a triangle `(head + (0, 0.025, 0), tail, head)` (`[0x5B26DC]` on the
/// first vertex's `y`, 0x454F72…0x454F80) in the particle's ramp colour on all three vertices,
/// drawn untextured (`[0x6221D8]+0x40 = 0`, 0x454ECC) as a triangle list (0x59F1D0 type 3, 80 a
/// batch) under the definition's blend and depth bias.
#[derive(Debug, Clone, PartialEq)]
pub struct Streak {
    pub head: Vec3,
    pub tail: Vec3,
    pub argb: u32,
    pub blend: u32,
    pub zbias: i32,
    pub sqd: f32,
}

/// One billboard of a frame: the definition's texture, the particle's point, its side, its ARGB
/// colour and its turn in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    pub material: String,
    /// the definition's `+0x28`, handed to the state object's `+8` (0x454756…0x454766 → 0x537A50),
    /// the Thrash 0x38 value of the draw (`RE/notes/thrash.md` §6)
    pub blend: u32,
    /// the definition's `+0x2C`, the depth bias's numerator (0x45445C…0x454491)
    pub zbias: i32,
    /// the system's `+0x10`, the value its article was last drawn with (0x455150) — the bias's
    /// denominator
    pub sqd: f32,
    pub position: Vec3,
    pub size: f32,
    pub argb: u32,
    pub degrees: f32,
}

pub struct TrackParticles {
    defs: Vec<ParticleDef>,
    systems: Vec<System>,
    rng: ParticleRng,
}

impl TrackParticles {
    /// 0x454250 for every emitter whose tag names a definition, with `budget` the detail level's
    /// `[0x6197E0]` (0…256).
    pub fn new(defs: Vec<ParticleDef>, emitters: &[Emitter], seed: u32, budget: u32) -> Self {
        let mut me = Self { defs, systems: Vec::new(), rng: ParticleRng::new(seed) };
        if budget == 0 {
            return me;
        }
        for e in emitters {
            let Some(di) = me.defs.iter().position(|d| d.tag == e.tag) else { continue };
            let sys = System::new(&me.defs, di, e.clone(), budget);
            me.systems.push(sys);
        }
        me
    }

    /// The constructor 0x454250 for system `k`, as 0x455150's creator 0x455000 calls it — `(value,
    /// part, 1)`: a fresh state with `+0x10` the article's value (0x454274) and `+0x2C = 1`
    /// (0x4542C2), then, the third argument being 1, its `prime` updates at once (0x4543A7…) — so
    /// they already fade by the value against the draw distance `draw` — and only **after** them
    /// flag 2 clears `+0x2C` (0x4543DF…0x4543E4): a once-only system primes emitting.
    /// *(Until 2026-09-26 the port primed at the full rate, and a flag-2 system not emitting.)*
    ///
    /// **Where the prime births (0x455030, read 2026-09-30).** Both creators prime at the part's
    /// point as the part stores it, [`Emitter::local`]: the static one hands the constructor the
    /// part itself (`+0x28 = part`, 0x4542BC); the movable one 0x455030 — the creator 0x450980
    /// uses for a `tr` article's kind-3 parts (`[esp+0x70] = 0x5B3CD8` at 0x450AE0, whose slot 0
    /// is 0x455030) — calls the constructor with prime **0** (0x45504E), copies the part's 48
    /// bytes to its own `+0x58` and points `+0x28` / `+0x4C` there (0x455059…0x455071), and runs
    /// the prime updates itself (0x45508A…0x45509F) before 0x450980 re-seats the point through
    /// slot 4 (0x4550B0, see [`Self::draw`]). So a `tr` article's prime is born at its
    /// untransformed point, near the world's origin, not at the prop — of the shipped kind-3
    /// definitions only `BPST` (the breakaway pipes, prime 10) and `HEXH` (prime 14, on `tr`-less
    /// articles, where the two points are one) prime at all. The point is restored to
    /// [`Emitter::position`] after the prime.
    fn make(&mut self, k: usize, value: f32, draw: f32) {
        let d = &self.defs[self.systems[k].def];
        let (emitting, prime) = (d.flags & 2 == 0, d.prime);
        let s = &mut self.systems[k];
        (s.tick, s.spawned, s.accumulated, s.emitting, s.live, s.sqd) = (0, 0.0, 0.0, true, true, value);
        s.particles.clear();
        let world = s.emitter.position;
        s.emitter.position = s.emitter.local;
        for _ in 0..prime {
            self.systems[k].update(&self.defs, &mut self.rng, draw);
        }
        let s = &mut self.systems[k];
        s.emitter.position = world;
        s.emitting = emitting;
    }

    pub fn systems(&self) -> usize {
        self.systems.len()
    }

    /// The generator `[0x5CEAE0]`, which the weather layer (`crate::weather`, 0x459260 / 0x456D70 /
    /// 0x457040) draws from too.
    pub fn rng_mut(&mut self) -> &mut ParticleRng {
        &mut self.rng
    }

    /// The draw's side, 0x450D10's kind 3 → 0x455150 (or the instance drawer's 0x450980), once per
    /// frame: `drawn` answers, per emitter, the value its article is drawn with this frame and
    /// whether the drawer triggers it, or `None` when the walk does not draw it
    /// (`TrackWorld::effect_run`). A drawn system is made if it does not exist (prime and all),
    /// its `+8` set to 100 and its `+0x10` to the value; a triggered one then takes slot 3,
    /// **0x454420**: `+0x2C = 1` (emitting) and `+0xC = 0` (the period's tick) — what 0x450980 does
    /// every drawn frame of a struck item (its record's `+4` is 1, 0x450B25…0x450B2F), so a
    /// once-only system (flag 2: `BEXP`, `BEXO`, `TRSH`) keeps going. The matrix 0x450980 hands
    /// slot 4 is the item's placement: 0x450010 asks 0x4769E0 for the lead's sample with its
    /// last argument 1, which swaps the sample for the identity (0x476A24…0x476A3E, `[0x5BC660]`)
    /// before the product with the instance's matrix (0x539CD0 / 0x559230) — so the point
    /// 0x53A330 makes of the part's (0x450B0D…0x450B14) is the part through the placement, every
    /// drawn frame the same. *(Corrected 2026-09-30: the system 0x450980 makes is the movable
    /// one, vtable 0x5B3E24, whose slot 4 is 0x4550B0 — it copies that point, 16 bytes, to the
    /// system's `+0x58` — not the static one's `ret 4` 0x50BBE0. The outcome is the same place:
    /// [`Emitter::position`], which is `tr` × the part's point — MEDIUM that the anim-world
    /// record's instance matrix `world+0x40 + 56·id` is the article's `tr`, as the port's own
    /// draw of the article at rest takes it. What does differ is the prime, [`Self::make`], and
    /// the directions, which stay the part's own: [`Emitter::dir_a`].)*
    /// `draw` is the scaled draw distance `[0x6197CC]` the constructor's prime updates fade by.
    pub fn draw(&mut self, draw: f32, drawn: impl Fn(&Emitter) -> Option<(f32, bool)>) {
        for k in 0..self.systems.len() {
            let Some((v, trigger)) = drawn(&self.systems[k].emitter) else { continue };
            if !self.systems[k].live {
                self.make(k, v, draw);
            }
            let s = &mut self.systems[k];
            s.countdown = 100;
            s.sqd = v;
            if trigger {
                s.emitting = true;
                s.tick = 0;
            }
        }
    }

    /// One 64 Hz update (0x4551E0) with the scaled draw distance `[0x6197CC]` (0x44F1A0): every
    /// live system counts `+8` down and runs its update (slot 2), or is dropped when it reaches 0 —
    /// so one out of sight stops 100 updates after its last draw ([`Self::draw`]), and starts
    /// afresh when it is drawn again.
    pub fn step(&mut self, draw: f32) {
        for k in 0..self.systems.len() {
            if !self.systems[k].live {
                continue;
            }
            self.systems[k].countdown = self.systems[k].countdown.saturating_sub(1);
            if self.systems[k].countdown == 0 {
                self.systems[k].live = false;
                self.systems[k].particles.clear();
                continue;
            }
            self.systems[k].update(&self.defs, &mut self.rng, draw);
        }
    }

    /// 0x454430's billboards for every particle.
    pub fn sprites(&self) -> Vec<Sprite> {
        let (mut out, mut streaks) = (Vec::new(), Vec::new());
        for s in self.systems.iter().filter(|s| s.live) {
            s.draw_into(&self.defs, Vec3::ZERO, &mut out, &mut streaks);
        }
        out
    }
}

/// The emission's distance factor (0x454C7B…0x454CD2): 1 while the square distance `sqd` is within
/// `(volume · draw)²`, else `(draw − √sqd) / ((1 − volume) · draw)` — falling to 0 at the draw
/// distance and below it past that — with the square root the image's bit trick,
/// `bits(sqd) >> 1 + 0x1FC00000` (0x454CA3…0x454CB4). The system's own factor `+0x44` is the 1.0
/// the constructor sets (0x454305). `sqd` is the system's `+0x10`, its article's value from the
/// last draw (0x455150).
pub fn emission_scale(sqd: f32, draw: f32, volume: f32) -> f32 {
    let r = volume * draw;
    if r * r >= sqd {
        return 1.0;
    }
    let s = f32::from_bits(((sqd.to_bits() as i32 >> 1) + 0x1FC0_0000) as u32);
    (draw - s) / ((1.0 - volume) * draw)
}

/// 0x454860: the point with `x` / `z` spread by `emit`, the speed `emit.y + rand` along the second
/// direction plus random parts along the first and the axis, the gravity `(weight + rand) × 0.005`
/// and a random turn — in that order off the generator.
fn birth(rng: &mut ParticleRng, d: &ParticleDef, e: &Emitter, axis: Vec3) -> Particle {
    let mut pos = e.position;
    pos.x += rng.spread(d.emit[0]);
    pos.z += rng.spread(d.emit[2]);
    let speed = d.emit[1] + rng.spread(d.rand_emit[1]);
    let mut vel = e.dir_b * speed;
    vel += e.dir_a * rng.spread(d.rand_emit[0]);
    vel += axis * rng.spread(d.rand_emit[2]);
    let gravity = (d.weight + rng.spread(d.rand_weight)) * 0.005;
    let turn = rng.spread(d.rand_rotation);
    Particle { pos, vel, age: 0.0, gravity, turn }
}

/// The colour at `t` (0x45454B…0x454685): one key for fewer than two or more than four, a lerp
/// over two, and for three or four `t` held below 1 (0.9999), `f = t·(n − 1)`, the key `floor(f)`
/// and its interval's delta by the fraction — per byte, `key + trunc(delta × frac)` wrapping.
pub fn ramp(d: &ParticleDef, t: f32) -> u32 {
    let n = d.states as i32;
    let lerp = |a: u32, b: u32, f: f32| -> u32 {
        let mut out = 0u32;
        for j in 0..4 {
            let (x, y) = ((a >> (8 * j)) & 0xFF, (b >> (8 * j)) & 0xFF);
            let delta = y as i32 - x as i32;
            // 0x45463C / 0x4545DF: `fld delta ; fmul t` on the x87 stack, then `_ftol` 0x5A0F98
            // — the product is exact in extended precision (and in f64), not rounded to f32
            // first, so a product just under an integer truncates down
            let v = (x as u8).wrapping_add((delta as f64 * f as f64) as i32 as u8);
            out |= (v as u32) << (8 * j);
        }
        out
    };
    match n {
        2 => lerp(d.colours[0], d.colours[1], t),
        3 | 4 => {
            let t = if t < 1.0 { t } else { 0.9999 };
            let f = t * (n - 1) as f32;
            let k = (f.floor() as usize).min(2);
            lerp(d.colours[k], d.colours[k + 1], f - k as f32)
        }
        _ => d.colours[0],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_system_lives_while_its_article_is_drawn() {
        // 0x455150 makes it on the first draw; 0x4551E0 drops it 100 updates after the last
        let e = Emitter { article: 0, part: 0, position: Vec3::ZERO, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let mut p = TrackParticles::new(vec![def()], &[e], 0, 256);
        p.step(1000.0);
        assert!(!p.systems[0].live);
        p.draw(1000.0, |_| Some((25.0, false)));
        assert!(p.systems[0].live);
        assert_eq!(p.systems[0].sqd, 25.0);
        for _ in 0..99 {
            p.step(1000.0);
        }
        assert!(p.systems[0].live);
        p.step(1000.0);
        assert!(!p.systems[0].live && p.sprites().is_empty());
    }

    #[test]
    fn the_prime_births_at_the_parts_own_point_and_the_run_at_the_placed_one() {
        // 0x455030: prime 0 to the constructor (0x45504E), the prime updates on the part's
        // copy (0x45508A…), then 0x450980's slot 4 (0x4550B0) re-seats it at the placed point
        let placed = Vec3::new(1000.0, 0.0, 0.0);
        let e = Emitter { article: 0, part: 0, position: placed, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let mut d = def();
        d.prime = 20;
        let mut p = TrackParticles::new(vec![d], &[e], 0, 256);
        p.draw(1000.0, |_| Some((0.0, true)));
        let primed = p.sprites();
        assert!(!primed.is_empty());
        assert!(primed.iter().all(|s| s.position.length() < 10.0));
        assert_eq!(p.systems[0].emitter.position, placed);
        for _ in 0..20 {
            p.draw(1000.0, |_| Some((0.0, true)));
            p.step(1000.0);
        }
        assert!(p.sprites().iter().any(|s| (s.position - placed).length() < 10.0));
    }

    #[test]
    fn emission_fades_from_the_volume_to_the_draw_distance() {
        // full within volume × draw; past it the bit-trick root (√90000 → 303.78, √160000 → 412.25)
        // makes it 0.481 half-way and slightly negative at the draw distance itself
        assert_eq!(super::emission_scale(100.0 * 100.0, 400.0, 0.5), 1.0);
        assert!((super::emission_scale(300.0 * 300.0, 400.0, 0.5) - 0.481_093_75).abs() < 1e-5);
        assert!((super::emission_scale(400.0 * 400.0, 400.0, 0.5) + 0.061_25).abs() < 1e-5);
    }

    use super::*;

    fn def() -> ParticleDef {
        ParticleDef {
            tag: 1,
            flags: 0,
            count: 20,
            emit: [0.0, 0.06, 0.0],
            rand_emit: [0.02, 0.02, 0.01],
            air_friction: 1.0,
            material: "SPRK".into(),
            blend: 1,
            zbias: 0,
            frames: 1,
            frame_speed: 10,
            states: 2,
            overall_size: 0,
            start_size: 0.03,
            end_size: 0.1,
            lifetime: 100,
            period_on: 100,
            period_off: 0,
            prime: 0,
            weight: 0.1,
            rand_weight: 0.0,
            rand_rotation: 0.0,
            volume: 1.0,
            colours: [0xFFFF_FFFF, 0xFF00_0000, 0, 0],
        }
    }

    #[test]
    fn rate_emission_and_death() {
        let e = Emitter { article: 0, part: 0, position: Vec3::ZERO, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let mut p = TrackParticles::new(vec![def()], &[e], 0, 256);
        assert_eq!(p.systems(), 1);
        // 20 over a 100-tick life: 0.2 a tick, the first one born once 1.2 has accumulated
        let tick = |p: &mut TrackParticles| {
            p.draw(1000.0, |_| Some((0.0, false)));
            p.step(1000.0);
        };
        for _ in 0..5 {
            tick(&mut p);
        }
        assert_eq!(p.sprites().len(), 0);
        tick(&mut p);
        assert_eq!(p.sprites().len(), 1);
        for _ in 0..200 {
            tick(&mut p);
        }
        // the population settles at what 100 ticks of life hold
        let n = p.sprites().len();
        assert!((19..=21).contains(&n), "{n}");
        // sizes are the file's tenths times ten
        assert!(p.sprites().iter().all(|s| s.size >= 0.3 - 1e-4 && s.size <= 1.0 + 1e-4));
    }

    #[test]
    fn colour_ramp() {
        let mut d = def();
        assert_eq!(ramp(&d, 0.0), 0xFFFF_FFFF);
        assert_eq!(ramp(&d, 0.5), 0xFF80_8080);
        d.states = 1;
        assert_eq!(ramp(&d, 0.7), 0xFFFF_FFFF);
        d.states = 3;
        d.colours = [0xFF00_0000, 0xFF00_00FF, 0xFF00_FF00, 0];
        assert_eq!(ramp(&d, 0.5), 0xFF00_00FF);
        assert_eq!(ramp(&d, 1.0) & 0xFF00, 0xFE00);
        // `_ftol` of the extended product: 10 × 0.7f32 is 6.99999988, so 6 — an f32 product rounds
        // to 7.0
        d.states = 2;
        d.colours = [0, 10, 0, 0];
        assert_eq!(ramp(&d, 0.7), 6);
    }

    #[test]
    fn the_prime_emits_and_fades_by_the_value() {
        // 0x454250: `+0x2C = 1` before the prime loop, flag 2 clears it only after; `+0x10` is
        // the value 0x455000 hands it, so the prime's emission fades past `volume · D`
        let e = Emitter { article: 0, part: 0, position: Vec3::ZERO, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let mut d = def();
        (d.flags, d.prime, d.volume) = (2, 50, 0.5);
        let mut p = TrackParticles::new(vec![d.clone()], &[e.clone()], 0, 256);
        p.draw(1000.0, |_| Some((0.0, false)));
        let near = p.sprites().len();
        assert!(near > 0, "a once-only system primes emitting");
        assert!(!p.systems[0].emitting, "and stops after the prime");
        // at 750 of a 1000 draw distance with volume 0.5 the rate is about half
        let mut p = TrackParticles::new(vec![d], &[e], 0, 256);
        p.draw(1000.0, |_| Some((750.0 * 750.0, false)));
        let far = p.sprites().len();
        assert!(far < near && far > 0, "{far} {near}");
    }

    #[test]
    fn budget_and_once_only() {
        let e = Emitter { article: 0, part: 0, position: Vec3::ZERO, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let none = TrackParticles::new(vec![def()], &[e.clone()], 0, 0);
        assert_eq!(none.systems(), 0);
        let mut d = def();
        d.flags = 2;
        let mut p = TrackParticles::new(vec![d], &[e], 0, 256);
        for _ in 0..50 {
            p.draw(1000.0, |_| Some((0.0, false)));
            p.step(1000.0);
        }
        assert!(p.sprites().is_empty(), "flag 2 waits for a trigger");
        // 0x450980's slot 3 (0x454420) on every drawn frame of a struck item starts it and keeps it
        for _ in 0..50 {
            p.draw(1000.0, |_| Some((0.0, true)));
            p.step(1000.0);
        }
        assert!(!p.sprites().is_empty(), "triggered");
    }
}
