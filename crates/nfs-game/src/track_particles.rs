//! The track's particle systems — `RGenericParticleSystem` (vtable 0x5B3DF8) started by the kind-3
//! `ef` parts of the track's articles through 0x455150 (`RE/notes/particles.md` §6,
//! `crp-geometry.md` §8): the constructor 0x454250, the 64 Hz update 0x454C30 with the birth
//! 0x454860, and the draw 0x454430's billboards.
//!
//! APPROXIMATION: every system is made when the track loads and lives for the race — the
//! original makes one the first time its article is drawn and drops it 100 updates after it was
//! last drawn (0x4551E0), and a system's emission fades past `volume ×` the draw distance
//! (0x454C7E…0x454CD2), which this port has no counterpart for.

use glam::Vec3;
use nfs_formats::particle_dat::ParticleDef;

/// One kind-3 `ef` part: its data from `+8`.
#[derive(Debug, Clone, PartialEq)]
pub struct Emitter {
    /// `+0…+8`
    pub position: Vec3,
    /// `+0x10…+0x18`, `+0x1C…+0x24`
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
    fn next(&mut self) -> u32 {
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
struct System {
    def: usize,
    emitter: Emitter,
    /// `+0x30` — `cross(dir_a, dir_b)` (0x532880)
    axis: Vec3,
    /// `+0x54` — `count × budget >> 8`
    max: usize,
    /// `+0x40`
    rate: f32,
    /// `+0x0C`, `+0x14`, `+0x18`, `+0x2C`
    tick: u32,
    spawned: f32,
    accumulated: f32,
    emitting: bool,
    /// `+0x2E & 2`: both sizes 0, no billboards
    no_sprites: bool,
    particles: Vec<Particle>,
}

/// One billboard of a frame: the definition's texture, the particle's point, its side, its ARGB
/// colour and its turn in degrees.
#[derive(Debug, Clone, PartialEq)]
pub struct Sprite {
    pub material: String,
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
            let d = &me.defs[di];
            // 0x454322…0x45437C
            let (count, on, off, life) = (d.count, d.period_on, d.period_off, d.lifetime);
            let rate = if off != 0 && life >= on {
                if life <= on + off { count as f32 / on as f32 } else { count as f32 / ((life as f32 / (on + off) as f32) * on as f32) }
            } else {
                count as f32 / life.max(1) as f32
            };
            let sys = System {
                def: di,
                emitter: e.clone(),
                axis: e.dir_a.cross(e.dir_b),
                max: ((count as u64 * budget as u64) >> 8) as usize,
                rate,
                tick: 0,
                spawned: 0.0,
                accumulated: 0.0,
                emitting: d.flags & 2 == 0,
                no_sprites: d.start_size == 0.0 && d.end_size == 0.0,
                particles: Vec::new(),
            };
            let prime = d.prime;
            me.systems.push(sys);
            let k = me.systems.len() - 1;
            for _ in 0..prime {
                me.step_one(k);
            }
        }
        me
    }

    pub fn systems(&self) -> usize {
        self.systems.len()
    }

    /// One 64 Hz update of every system (0x4551E0 → slot 2).
    pub fn step(&mut self) {
        for k in 0..self.systems.len() {
            self.step_one(k);
        }
    }

    /// 0x454C30.
    fn step_one(&mut self, k: usize) {
        let d = &self.defs[self.systems[k].def];
        let s = &mut self.systems[k];
        if s.particles.is_empty() && !s.emitting {
            return;
        }
        let period = d.period_on + d.period_off;
        let off_part = d.period_off != 0 && s.tick > d.period_on;
        if s.emitting && !off_part {
            s.accumulated += s.rate;
            while s.accumulated > s.spawned + 1.0 {
                if s.particles.len() < s.max {
                    let p = birth(&mut self.rng, d, &s.emitter, s.axis);
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

    /// 0x454430's billboards for every particle.
    pub fn sprites(&self) -> Vec<Sprite> {
        let mut out = Vec::new();
        for s in &self.systems {
            if s.no_sprites {
                continue;
            }
            let d = &self.defs[s.def];
            for p in &s.particles {
                let t = p.age / d.lifetime.max(1) as f32;
                out.push(Sprite {
                    material: d.material.clone(),
                    position: p.pos,
                    size: (t * (d.end_size - d.start_size) + d.start_size) * 10.0,
                    argb: ramp(d, t),
                    degrees: t * p.turn,
                });
            }
        }
        out
    }
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
            let v = (x as u8).wrapping_add((delta as f32 * f) as i32 as u8);
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
        let e = Emitter { position: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let mut p = TrackParticles::new(vec![def()], &[e], 0, 256);
        assert_eq!(p.systems(), 1);
        // 20 over a 100-tick life: 0.2 a tick, the first one born once 1.2 has accumulated
        for _ in 0..5 {
            p.step();
        }
        assert_eq!(p.sprites().len(), 0);
        p.step();
        assert_eq!(p.sprites().len(), 1);
        for _ in 0..200 {
            p.step();
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
    }

    #[test]
    fn budget_and_once_only() {
        let e = Emitter { position: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: 1 };
        let none = TrackParticles::new(vec![def()], &[e.clone()], 0, 0);
        assert_eq!(none.systems(), 0);
        let mut d = def();
        d.flags = 2;
        let mut p = TrackParticles::new(vec![d], &[e], 0, 256);
        for _ in 0..50 {
            p.step();
        }
        assert!(p.sprites().is_empty(), "flag 2 waits for a trigger");
    }
}
