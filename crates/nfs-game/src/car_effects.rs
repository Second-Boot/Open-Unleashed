//! Each car's own particle effects — the eleven `carpart.dat` systems of `RE/notes/particles.md`
//! §11: the set **0x43EE10** builds per `R3DCar` (0x437920 → 0x437A1E, `[car+0x704]`), the
//! per-frame drive **0x43EF80** the car's draw runs (`R3DCar` slot 7 0x4383C0 → 0x438416, from
//! the draw 0x4430A0 at 0x443239 — a car that is drawn — and, ungated, `R3DCar` slot 8 0x438420
//! from the walk of every car **0x443640** on `GameTick`'s odd tick, 0x4150ED), the update
//! **0x43F660** on the same odd tick right after it (0x4150F2) and the draw **0x43F6E0** after the
//! track's systems (0x468B6F).
//!
//! The systems are the track's `RGenericParticleSystem` (`crate::track_particles`), each on a
//! 0x30-byte slot record that stands for the track part: the point `+0`, the two directions
//! `+0x10` = (1, 0, 0) and `+0x1C` = (0, 1, 0) — the second the drive overwrites with the
//! particles' velocity — and the tag `+0x28` from 0x5B3724.

use glam::Vec3;
use nfs_formats::particle_dat::ParticleDef;

use crate::track_particles::{Emitter, ParticleRng, Sprite, Streak, System};

/// `SMRT` "Skid smoke (rear tires)" — the tyre smoke of all four wheels.
pub const SMRT: u32 = 0x5452_4D53;
/// `CSPK` "Sparks".
pub const CSPK: u32 = 0x4B50_5343;
/// `WTSP` — the rear wheels' water spray (named "Skid smoke (rear tires)" in `carpart.ini`).
pub const WTSP: u32 = 0x5053_5457;
/// `RDDT` "Road dirt".
pub const RDDT: u32 = 0x5444_4452;
/// `RDGR` "Road grass".
pub const RDGR: u32 = 0x5247_4452;
/// `RDSN` "Road snow".
pub const RDSN: u32 = 0x4E53_4452;

/// `0x5B3724`, the eleven slots' tags: the tyre smoke of wheels 0…3, the sparks, the rear wheels'
/// spray, the surface effect of wheels 0…3.
pub const SLOT_TAGS: [u32; 11] = [SMRT, SMRT, SMRT, SMRT, CSPK, WTSP, WTSP, RDDT, RDDT, RDDT, RDDT];

/// `0x5CD964[surface]`, the class the drive gates the smoke and the spray on (0x43F02E): 1 for the
/// surfaces that smoke.
pub const SURFACE_CLASS: [u32; 16] = [0, 1, 2, 2, 1, 2, 0, 1, 0, 2, 1, 1, 1, 2, 0, 2];

/// `0x5B3750[surface]`, the surface effect a wheel's slot 7 + w is retagged to (0x43F0F1), 0 for
/// none: dirt on 2, 5 and 13, grass on 3, the water spray on 6, snow on 9 and 15.
pub const SURFACE_TAGS: [u32; 16] = [0, 0, RDDT, RDGR, 0, RDDT, WTSP, 0, 0, RDSN, 0, 0, 0, RDDT, 0, RDSN];

/// `[0x5CD9B0]`, the forward speed the rear wheels' spray starts above (0x43F244).
pub const SPRAY_SPEED: f32 = 7.5;

/// One wheel as 0x43EF80 reads it (`car + 0xC4·w`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelInput {
    /// `wheel+0x8C & 0xF` (`[car+0x844] & 0xF`, 0x43F024…0x43F02B)
    pub surface: u32,
    /// `wheel+0xB8` (`[car+0x870]`)
    pub slip: f32,
    /// `wheel+0x40` (`[car+0x7F8]`), the ground contact
    pub contact: Vec3,
}

/// The car as 0x43EF80 reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarInput {
    /// `car+0x330`
    pub position: Vec3,
    /// `car+0x33C`, m/s
    pub velocity: Vec3,
    /// the orientation's forward row `car+0x37C`; the drive takes its `x` and `z`
    pub forward: Vec3,
    /// `car+0xD60`, the physics-frame forward speed
    pub speed: f32,
    pub wheels: [WheelInput; 4],
}

impl CarInput {
    /// The fields of a body: the wheels' zone nibble, slip and ground contact `(probe.x, h_i,
    /// probe.z)` — the port's hub less its ride height, as the skid marks take it.
    pub fn from_vehicle(v: &nfs_physics::vehicle::Vehicle) -> Self {
        CarInput {
            position: v.position,
            velocity: v.velocity,
            forward: v.forward(),
            speed: v.v_long(),
            wheels: std::array::from_fn(|i| {
                let w = &v.wheels[i];
                WheelInput { surface: w.zone & 0xF, slip: w.slip, contact: w.world_pos - Vec3::Y * v.ride_height }
            }),
        }
    }
}

struct EffectSet {
    /// `+8…+0x30`; `None` where the tag names no definition
    systems: [Option<System>; 11],
    /// entered in the draw set `[0x6115C0]` since the last update (0x43F38C…, emptied by
    /// 0x43F660 after its walk)
    drawn: bool,
}

/// Every car's effect set, by the app's car index.
pub struct CarEffects {
    defs: Vec<ParticleDef>,
    /// `[0x6197E0]`
    budget: u32,
    sets: Vec<EffectSet>,
}

impl CarEffects {
    /// `defs` from `carpart.dat` (0x455570 loads it after `particle.dat`, into the same map),
    /// `budget` the detail level's `[0x6197E0]`.
    pub fn new(defs: Vec<ParticleDef>, budget: u32) -> Self {
        Self { defs, budget, sets: Vec::new() }
    }

    /// **0x43EE10** for `car` (and any before it) the first time it is asked: per slot the record
    /// `+0x10` = (1, 0, 0), `+0x1C` = (0, 1, 0), the tag (0x43EE70…0x43EEAF), the system through
    /// the constructor 0x454250 `(0, slot, 1)` — no prime (0 in every `carpart.dat` definition) —
    /// and `+0x2C = 0` (0x43EEE4): nothing emits until the drive says so.
    fn ensure(&mut self, car: usize) {
        while self.sets.len() <= car {
            let systems = std::array::from_fn(|k| {
                let tag = SLOT_TAGS[k];
                let di = self.defs.iter().position(|d| d.tag == tag)?;
                let e = Emitter { article: 0, part: k as u16, position: Vec3::ZERO, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag };
                let mut s = System::new(&self.defs, di, e, self.budget);
                s.emitting = false;
                // the sparks: `+0x2E |= 1 | 4` and `+0x4C = car + 0x330` (0x43EEEA…0x43EF03) —
                // born at the slot's point, drawn about the car's position; bit 4 has no reader
                // in the particle code (0x454000…0x456000)
                s.relative = tag == CSPK;
                Some(s)
            });
            self.sets.push(EffectSet { systems, drawn: false });
        }
    }

    /// **0x43EF80(1)**, the per-frame drive of a drawn car: nothing without a budget
    /// (`[0x6197E0]`, 0x43EF88); `d² = |car+0x330 − eye|²` (0x43EF96…0x43EFCD); `wet` the sky's
    /// field 5 above 0.1 (0x43EFD7…0x43EFF8); `fancy` the byte `[0x6197AA]` (the spray's gate). Per
    /// wheel `w` with `class = 0x5CD964[surface]`:
    ///
    /// * **tyre smoke** `SMRT` slot `w`: on when `!wet && class == 1 && slip ≥ 0.5` (0x43F04A…
    ///   0x43F070) at the contact, `+0x44` the slip, the velocity `(v.x, v.y + 0.8, v.z)` (the
    ///   `double` 0.8 at `[0x5B37B8]`); else off.
    /// * **the surface** slot `7 + w`: on when `tag = 0x5B3750[surface]` is not 0, `slip ≥ 0.3`
    ///   and `car+0xD60 > 0` (0x43F0F1…0x43F130): `+0x44` the slip, the retag 0x454AC0(tag) —
    ///   dirt, grass, snow or the water spray by the ground — the contact, less the forward row's
    ///   `x` and `z` unless dirt, and the velocity `(v.x − 3.66·f.x, v.y + k, v.z − 3.66·f.z)`
    ///   with `k` 0.8 for dirt, else 6.32; else off.
    /// * **the water spray** `WTSP`, slot 5 / 6 of the rear wheels 2 / 3: on when `wet && class
    ///   == 1 && car+0xD60 > 7.5 && [0x6197AA]` (0x43F21E…0x43F25C): `+0x44 = min(speed − 7.5,
    ///   15) / 15`, the contact less `1.14·f` across, the velocity `(v.x − 2·f.x, v.y + 4.35 +
    ///   0.05·speed, v.z − 2·f.z)`; else off. So in the wet the tyres throw water instead of
    ///   smoke.
    ///
    /// A lit slot also gets `+8 = 100` and `+0x10 = d²`. Then the set enters the draw set.
    pub fn drive(&mut self, car: usize, c: &CarInput, eye: Vec3, wet: bool, fancy: bool) {
        if self.budget == 0 {
            return;
        }
        let d = c.position - eye;
        let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
        self.ensure(car);
        let (defs, budget, set) = (&self.defs, self.budget, &mut self.sets[car]);
        tracing::trace!(car, wet, surf = ?c.wheels.map(|w| w.surface), slip = ?c.wheels.map(|w| w.slip), speed = c.speed, "car effects drive (0x43EF80)");
        for (w, wheel) in c.wheels.iter().enumerate() {
            let class = SURFACE_CLASS[(wheel.surface & 0xF) as usize];
            let slip = wheel.slip;
            if let Some(s) = set.systems[w].as_mut() {
                if !wet && class == 1 && slip >= 0.5 {
                    s.emitter.position = wheel.contact;
                    s.strength = slip;
                    s.emitting = true;
                    s.sqd = d2;
                    s.emitter.dir_b = Vec3::new(c.velocity.x, (c.velocity.y as f64 + 0.8) as f32, c.velocity.z);
                } else {
                    s.emitting = false;
                }
            }
            let tag = SURFACE_TAGS[(wheel.surface & 0xF) as usize];
            if let Some(s) = set.systems[7 + w].as_mut() {
                if tag != 0 && slip >= 0.3 && c.speed > 0.0 {
                    s.emitter.position = wheel.contact;
                    s.strength = slip;
                    s.retag(defs, tag, budget);
                    s.emitting = true;
                    s.sqd = d2;
                    // 0x43F173…0x43F1BE: `k` 0.8 (`[0x5B24E8]`) for dirt at the contact, else
                    // 6.32 (`[0x5B37B0]`) and the point a metre back along the car's axis
                    let k = if tag == RDDT {
                        0.8f32
                    } else {
                        s.emitter.position.x -= c.forward.x;
                        s.emitter.position.z -= c.forward.z;
                        6.32f32
                    };
                    // 0x43F1C0…0x43F204: `v − 3.66·f` (`[0x5B37AC]`) across, `v.y + k` up
                    let across = |v: f32, f: f32| (v as f64 - f as f64 * 3.66f32 as f64) as f32;
                    s.emitter.dir_b = Vec3::new(across(c.velocity.x, c.forward.x), k + c.velocity.y, across(c.velocity.z, c.forward.z));
                } else {
                    s.emitting = false;
                }
            }
            // the rear wheels' spray, slots 5 and 6 (0x43F212…0x43F34E)
            if w < 2 {
                continue;
            }
            if let Some(s) = set.systems[3 + w].as_mut() {
                if wet && class == 1 && c.speed > SPRAY_SPEED && fancy {
                    // 0x43F262…0x43F293: `min(speed − 7.5, 15) / 15` — `15` is `7.5 + 7.5`
                    let over = c.speed - SPRAY_SPEED;
                    let top = SPRAY_SPEED + SPRAY_SPEED;
                    s.strength = if over > top { top / top } else { over / top };
                    // 0x43F2B1…0x43F2E1: the contact less `1.14·f` (`[0x5B37A8]`) across
                    let back = |p: f32, f: f32| (p as f64 - f as f64 * 1.14f32 as f64) as f32;
                    s.emitter.position = Vec3::new(back(wheel.contact.x, c.forward.x), wheel.contact.y, back(wheel.contact.z, c.forward.z));
                    s.emitting = true;
                    s.sqd = d2;
                    // 0x43F2F8…0x43F348: `v − 2·f` across, `v.y + 4.35 + 0.05·speed` up
                    // (`[0x5B3798]`, the `double` 0.05 at `[0x5B37A0]`)
                    let up = (c.velocity.y as f64 + 4.35f32 as f64) + c.speed as f64 * 0.05;
                    s.emitter.dir_b = Vec3::new(c.velocity.x - (c.forward.x + c.forward.x), up as f32, c.velocity.z - (c.forward.z + c.forward.z));
                } else {
                    s.emitting = false;
                }
            }
        }
        set.drawn = true;
    }

    /// **0x43F4C0**, the sparks, fired by 0x40F740 for a wall event (kind 4, 0x40F97E) or a
    /// body–ground one (kind 3, 0x40F9D7) whose `car+0x430` is over 1.5, with `point` the contact
    /// `car+0x440` and `v` the velocity `car+0x33C`: nothing without a budget; `d² = |point −
    /// eye|²`; the slot's point is `point − car+0x330` (its `+0xC` the floor `car+0x404 −
    /// point.y`, which only a flag-1 definition reads — `CSPK` has flags 6); `dir_b = −0.1·v`
    /// (`[0x5B2444]`); with `a = (0, 1, 0)` — `(0.015, 1, 0)` when both `|v.x|` and `|v.z|` are
    /// under 0.01 (`[0x5B2BC0]`) — `c = a × v` (0x532880) and `dir_a` = the system's axis `+0x30`
    /// = `normalize(c × v)` (0x5328A0 twice); then slot 3 (0x454420: emitting, the tick 0), `+8
    /// = 100`, `+0x10 = d²`. `CSPK` is once-only (flag 2) with a period of 1 on, 2 off: two
    /// updates of emission at 4.29 an update (40 / ((28 / 3) · 1)), then it stops itself.
    /// APPROXIMATION: a velocity of exactly zero gives the original a NaN axis (1 / 0 in
    /// 0x5328D0); here a zero one.
    pub fn sparks(&mut self, car: usize, point: Vec3, v: Vec3, car_pos: Vec3, eye: Vec3) {
        if self.budget == 0 {
            return;
        }
        let d = point - eye;
        let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
        self.ensure(car);
        let Some(s) = self.sets[car].systems[4].as_mut() else { return };
        s.emitter.position = point - car_pos;
        s.emitter.dir_b = v * -0.1;
        let a = if (v.x.abs() as f64) < 0.01 && (v.z.abs() as f64) < 0.01 { Vec3::new(0.015, 1.0, 0.0) } else { Vec3::Y };
        let c = a.cross(v);
        let n = c.cross(v).normalize_or_zero();
        s.emitter.dir_a = n;
        s.axis = n;
        s.start();
        s.sqd = d2;
        tracing::debug!(car, ?point, ?v, "car sparks (0x43F4C0)");
    }

    /// **0x43F660**, from `GameTick`'s odd tick (0x4150F2, 32 Hz): every set in the draw set runs
    /// its eleven systems' update (slot 2, 0x454C30) in slot order, and the draw set is emptied —
    /// a car that is not drawn is not updated. `draw` is the draw distance `[0x6197CC]` the
    /// emission fades by (the systems' `+0x10` is the car's `d²`).
    pub fn update(&mut self, rng: &mut ParticleRng, draw: f32) {
        let defs = &self.defs;
        for set in self.sets.iter_mut().filter(|s| s.drawn) {
            for s in set.systems.iter_mut().flatten() {
                s.update(defs, rng, draw);
            }
            set.drawn = false;
        }
    }

    /// **0x43F6E0**, in the render frame after the track's systems (0x468B64 → 0x455260, then
    /// 0x468B6F): every set of the draw set, its eleven systems' draw (slot 1, 0x454430) in slot
    /// order. `car_position` answers a car's `car+0x330`, which a relative system's draw adds.
    /// APPROXIMATION: the draw set is a `std::set` of the sets' addresses, so the cars' order is
    /// their heap order; here it is the car index.
    pub fn draw(&self, car_position: impl Fn(usize) -> Vec3) -> (Vec<Sprite>, Vec<Streak>) {
        let (mut sprites, mut streaks) = (Vec::new(), Vec::new());
        for (car, set) in self.sets.iter().enumerate().filter(|(_, s)| s.drawn) {
            let offset = car_position(car);
            for s in set.systems.iter().flatten() {
                s.draw_into(&self.defs, offset, &mut sprites, &mut streaks);
            }
        }
        tracing::trace!(sprites = sprites.len(), streaks = streaks.len(), first = ?sprites.first(), "car effects draw (0x43F6E0)");
        (sprites, streaks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> Vec<ParticleDef> {
        // `SMRT` as `carpart.dat` ships it (particles.md §11)
        vec![ParticleDef {
            tag: SMRT,
            flags: 0,
            count: 42,
            emit: [0.0, 0.0025, 0.0],
            rand_emit: [0.0075, 0.0075, 0.005],
            air_friction: 0.8,
            material: "sstk".into(),
            blend: 0,
            zbias: -20000,
            frames: 1,
            frame_speed: 10,
            states: 3,
            overall_size: 0,
            start_size: 0.097,
            end_size: 0.231,
            lifetime: 26,
            period_on: 100,
            period_off: 0,
            prime: 0,
            weight: -0.15,
            rand_weight: 0.0,
            rand_rotation: 486.0,
            volume: 1.0,
            colours: [0x4079_7C8A, 0x2884_8793, 0x0078_7878, 0],
        }]
    }

    fn car(surface: u32, slip: f32) -> CarInput {
        let w = WheelInput { surface, slip, contact: Vec3::new(1.0, 0.0, 2.0) };
        CarInput { position: Vec3::ZERO, velocity: Vec3::new(0.0, 0.0, 10.0), forward: Vec3::Z, speed: 10.0, wheels: [w; 4] }
    }

    #[test]
    fn tyre_smoke_needs_half_a_slip_on_a_smoking_surface_in_the_dry() {
        let mut fx = CarEffects::new(defs(), 256);
        let mut rng = ParticleRng::new(1);
        // tarmac (class 1), slip 0.6, dry: smoke
        for _ in 0..20 {
            fx.drive(0, &car(1, 0.6), Vec3::ZERO, false, true);
            fx.update(&mut rng, 1000.0);
        }
        fx.drive(0, &car(1, 0.6), Vec3::ZERO, false, true);
        let (sp, _) = fx.draw(|_| Vec3::ZERO);
        assert!(!sp.is_empty());
        assert!(sp.iter().all(|s| s.material == "sstk" && s.zbias == -20000));
        // 0.4 of slip, or wet, or class 2 (surface 2): none
        for (surf, slip, wet) in [(1, 0.4, false), (1, 0.9, true), (2, 0.9, false)] {
            let mut fx = CarEffects::new(defs(), 256);
            for _ in 0..20 {
                fx.drive(0, &car(surf, slip), Vec3::ZERO, wet, true);
                fx.update(&mut rng, 1000.0);
            }
            fx.drive(0, &car(surf, slip), Vec3::ZERO, wet, true);
            assert!(fx.draw(|_| Vec3::ZERO).0.is_empty(), "{surf} {slip} {wet}");
        }
    }

    /// `SMRT` plus the surface definitions with their shipped counts and textures.
    fn surface_defs() -> Vec<ParticleDef> {
        let base = defs().remove(0);
        let mk = |tag: u32, count: u32, material: &str| ParticleDef { tag, count, material: material.into(), ..base.clone() };
        vec![base.clone(), mk(RDDT, 35, "CDRT"), mk(RDGR, 43, "GRSS"), mk(RDSN, 16, "WFAL"), mk(WTSP, 35, "sstk")]
    }

    #[test]
    fn the_surface_slot_takes_the_grounds_effect() {
        let mut rng = ParticleRng::new(1);
        let run = |surf: u32, slip: f32, speed: f32, rng: &mut ParticleRng| {
            let mut fx = CarEffects::new(surface_defs(), 256);
            let mut c = car(surf, slip);
            c.speed = speed;
            for _ in 0..10 {
                fx.drive(0, &c, Vec3::ZERO, false, true);
                fx.update(rng, 1000.0);
            }
            fx.drive(0, &c, Vec3::ZERO, false, true);
            fx.draw(|_| Vec3::ZERO).0.iter().map(|s| s.material.clone()).collect::<Vec<_>>()
        };
        // dirt (2) at 0.3 of slip: `CDRT` only (class 2 does not smoke)
        let m = run(2, 0.35, 5.0, &mut rng);
        assert!(!m.is_empty() && m.iter().all(|m| m == "CDRT"), "{m:?}");
        // grass (3) retags to `GRSS`, snow (9) to `WFAL`
        assert!(run(3, 0.35, 5.0, &mut rng).iter().all(|m| m == "GRSS"));
        assert!(!run(9, 0.35, 5.0, &mut rng).is_empty());
        // below 0.3, standing or backing (`car+0xD60 ≤ 0`), or tarmac (no tag): nothing of it
        assert!(run(2, 0.25, 5.0, &mut rng).is_empty());
        assert!(run(2, 0.9, 0.0, &mut rng).is_empty());
        assert!(run(1, 0.35, 5.0, &mut rng).is_empty());
    }

    #[test]
    fn the_retag_keeps_the_particles_of_an_equal_count() {
        // 0x454B62: `RDDT` → `WTSP` (35 each) keeps them, → `RDGR` (43) frees them
        let defs = surface_defs();
        let e = Emitter { article: 0, part: 7, position: Vec3::ZERO, local: Vec3::ZERO, dir_a: Vec3::X, dir_b: Vec3::Y, tag: RDDT };
        let mut s = System::new(&defs, 1, e, 200);
        let mut rng = ParticleRng::new(3);
        for _ in 0..10 {
            s.update(&defs, &mut rng, 1000.0);
        }
        let count = |s: &System| {
            let (mut sp, mut st) = (Vec::new(), Vec::new());
            s.draw_into(&defs, Vec3::ZERO, &mut sp, &mut st);
            sp.len()
        };
        let n = count(&s);
        assert!(n > 0);
        s.retag(&defs, WTSP, 200);
        assert_eq!(count(&s), n);
        // the rate from the budget's maximum: 35 · 200 >> 8 = 27 over the lifetime
        assert_eq!(s.max, 27);
        assert_eq!(s.rate, 27.0 / 26.0);
        s.retag(&defs, RDGR, 200);
        assert_eq!(count(&s), 0);
    }

    #[test]
    fn the_rear_wheels_throw_water_in_the_wet_above_7_5() {
        let mut rng = ParticleRng::new(5);
        let run = |speed: f32, wet: bool, fancy: bool, rng: &mut ParticleRng| {
            let mut fx = CarEffects::new(surface_defs(), 256);
            let mut c = car(1, 0.0);
            c.speed = speed;
            for _ in 0..10 {
                fx.drive(0, &c, Vec3::ZERO, wet, fancy);
                fx.update(rng, 1000.0);
            }
            fx.drive(0, &c, Vec3::ZERO, wet, fancy);
            let sets = &fx.sets[0].systems;
            (sets[5].as_ref().unwrap().strength, fx.draw(|_| Vec3::ZERO).0.len(), sets[2].as_ref().unwrap().emitting)
        };
        let (k, n, front_rear_smoke) = run(15.0, true, true, &mut rng);
        assert_eq!(k, 0.5);
        assert!(n > 0 && !front_rear_smoke);
        assert_eq!(run(30.0, true, true, &mut rng).0, 1.0);
        assert_eq!(run(7.5, true, true, &mut rng).1, 0);
        assert_eq!(run(15.0, false, true, &mut rng).1, 0);
        assert_eq!(run(15.0, true, false, &mut rng).1, 0);
    }

    #[test]
    fn a_hit_throws_a_burst_of_sparks_that_follow_the_car() {
        // `CSPK` as shipped: flags 6 (once-only, streaks), sizes 0, 40 over a period of 1 on, 2 off
        let base = defs().remove(0);
        let cspk = ParticleDef {
            tag: CSPK,
            flags: 6,
            count: 40,
            emit: [0.2, 0.1925, 0.2],
            rand_emit: [0.0625, 0.0625, 0.0625],
            air_friction: 1.0,
            material: "SPRK".into(),
            blend: 1,
            zbias: -40000,
            start_size: 0.0,
            end_size: 0.0,
            lifetime: 28,
            period_on: 1,
            period_off: 2,
            weight: 2.75,
            colours: [0x83FF_F140, 0xC3FF_8A20, 0x00D6_0101, 0x0300_0000],
            ..base
        };
        let mut fx = CarEffects::new(vec![cspk], 256);
        let mut rng = ParticleRng::new(7);
        let car_pos = Vec3::new(100.0, 5.0, 100.0);
        fx.sparks(0, car_pos + Vec3::new(1.0, 0.2, 2.0), Vec3::new(0.0, 0.0, 20.0), car_pos, Vec3::ZERO);
        for _ in 0..6 {
            fx.drive(0, &car(1, 0.0), Vec3::ZERO, false, true);
            fx.update(&mut rng, 1000.0);
        }
        fx.drive(0, &car(1, 0.0), Vec3::ZERO, false, true);
        let s = fx.sets[0].systems[4].as_ref().unwrap();
        assert!(!s.emitting, "the period stops a once-only system");
        // 4.29 an update for two updates: 4 then 4 more
        let (sp, st) = fx.draw(|_| car_pos);
        assert!(sp.is_empty(), "sizes 0: no billboards");
        assert_eq!(st.len(), 8);
        // born at the contact relative to the car and drawn about its position
        let (_, moved) = fx.draw(|_| car_pos + Vec3::X * 10.0);
        assert!((moved[0].head - st[0].head - Vec3::X * 10.0).length() < 1e-4);
        assert!(st.iter().all(|s| s.blend == 1 && s.zbias == -40000));
    }

    #[test]
    fn a_car_not_drawn_is_neither_updated_nor_drawn() {
        let mut fx = CarEffects::new(defs(), 256);
        let mut rng = ParticleRng::new(1);
        for _ in 0..10 {
            fx.drive(0, &car(1, 0.8), Vec3::ZERO, false, true);
            fx.update(&mut rng, 1000.0);
        }
        // the update emptied the draw set: nothing to draw until the next drive
        assert!(fx.draw(|_| Vec3::ZERO).0.is_empty());
        fx.drive(0, &car(1, 0.8), Vec3::ZERO, false, true);
        let n = fx.draw(|_| Vec3::ZERO).0.len();
        assert!(n > 0);
        // no budget: nothing at all
        let mut none = CarEffects::new(defs(), 0);
        none.drive(0, &car(1, 0.8), Vec3::ZERO, false, true);
        none.update(&mut rng, 1000.0);
        assert!(none.draw(|_| Vec3::ZERO).0.is_empty());
    }
}
