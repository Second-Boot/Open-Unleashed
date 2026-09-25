//! Smackables — the world props the race gives a car record of their own (`car+0x52C & 0x80`),
//! the Factory Driver cones among them (`RE/notes/skill-tests.md` §5.3, `traffic.md` §8b / §15).
//! Read 2026-09-25; CONFIRMED unless a line says otherwise.
//!
//! **Creation.** `WAnimatableBundle` 0x4772F0 builds a 56-byte record for every article whose
//! `animdefs.txt` definition says `kSmackable` (`SIGN` through 0x476B30, every other one through
//! **0x476ED0** — [`point_cloud`]); 0x483360 keeps its point cloud (`rec+0xC`, count `+0x10`), its
//! mass (`+0x14`, the definition's, 10.0 without one) and its audio type (`+0x18`). The race init
//! 0x414DB0 then makes one car record per smackable (0x4A17F0: class 0x80, model "356B") and
//! `Car_reset` 0x410730 forks on the class (0x4107A1):
//!
//! * 0x4A1280 — `car+0x354 = rec+0x14 / 128` (0x4A12B5, `[0x5B273C]`), the material class
//!   `car+0x44C` from the audio type ([`class_for_audio`]), the point cloud copied to the body
//!   points `car+0x8C`, and the half extents `(max − min) / 2` over them **with the running min /
//!   max starting at 0** (0x4A137C…0x4A13A4) — the body's own origin is always inside its box;
//! * `RigidBody_init` 0x49B510 with that mass (`car+0x52C & 0x10` is clear, so no volume rule);
//! * 0x4A1510 — the pose from the record (position `rec+0x28`, rows `rec+0x34` each normalised),
//!   `0x483590(rec, &car+0x364, &car+0x330)` **points the prop record's pose at the car's**, so the
//!   article is drawn wherever the body is; `car+0x7F = 1`, **`+0x7D = 1` (asleep)**, `+0x7C = 0`
//!   (undisturbed), and one `Susp_AI` 0x4998C0 / `BodyHeight` 0x49A900 to seat it.
//!
//! **Handlers** (`Car_installHandlers` 0x410D60, 0x410E43): `+0xE1C` = **0x4A1030** ([`Smackable::damp`]),
//! `+0xE20` = `Car_ground` 0x49BD40 whose `& 0x80` fork takes `Susp_AI` ([`Smackable::ground`]),
//! `+0xE2C` = the collision pass 0x498FF0 ([`Smackable::collide`]), `+0xE30` = 0x410280
//! ([`Smackable::post_impact`]), and the 64 Hz integrators 0x49B7A0 / 0x49B8E0 ([`Smackable::integrate`]).
//! `+0xE10` / `+0xE14` are cleared: there is no drive and no input.
//!
//! **Not carried** (each recorded in the note): the `Lod_shouldUpdate` gate of `Car_ground` —
//! its distance `car+0x78` is 0x417190's octagonal distance to the nearest racer, and row 3 of
//! 0x5CC770 runs every tick under 96 m, which is every cone of a skill course; the port runs every
//! tick — `0x49AF70`'s slice tracking, and 0x410280's sound event (`{1, −1, idA, idB, min(0x430,
//! 50)}` pushed when `0x430 > 0.15`) — a smackable holds no audio car slot (0x4A7DC0 takes cars),
//! so nothing drains it (MEDIUM). *(2026-09-25:)* the tracker and its plane (0x49AF70 without
//! the slice part, 0x499520) are carried, [`crate::collision::PatchTracker`]. The collision hold
//! `car+0x43C` needs nothing: every writer in the image stores 0 (0x40DFC7, 0x40E14E, 0x40F45B, 0x410D0A, 0x49B6F2) and 0x499019 only counts it
//! down while positive, so the 0x498DE9 skip never fires. *(Corrected 2026-09-25: this listed
//! the 8 m ceiling rule as not carried; it is, in the shared `Vehicle::car_ground_body`.)*

use crate::collision::TriangleGrid;
use crate::vehicle::{collide_pair, Vehicle};
use glam::{Mat3, Vec3};

/// 0x4772D1: the record keeps at most **20** points (upper hull first, so a long lower hull is the
/// one cut).
pub const MAX_POINTS: usize = 20;

/// `[0x5B2604]` = 0.1: an upper hull narrower than this in both x and z collapses to its centre
/// (0x4771BC / 0x4771D1).
const COLLAPSE: f32 = 0.1;

/// 0x4A12D0's switch on the record's audio type (`kAud…` order, `animdefs.rs`): the material id
/// of `sfx.md` §2 — `kAudSign` 0 → 0x13 WoodSign, `kAudMetalSign` → 0x12, `kAudGlass` → 0x11,
/// `kAudWoodbox` → 0x14, **`kAudPylon` 4 → 0x10 Pylon**, `kAudTree` → 0x0E, `kAudBush` → 0x0F,
/// `kAudCarBody` → 3, `kAudWood` → 9, `kAudSnow` → 0x0A, `kAudIce` → 0x0B, `kAudEarth` → 0x0C,
/// `kAudRock` → 0x0D; anything past 12 → 0x13 (the jump table at 0x4A14D4).
pub fn class_for_audio(audio_type: u32) -> u32 {
    const T: [u32; 13] = [0x13, 0x12, 0x11, 0x14, 0x10, 0x0E, 0x0F, 3, 9, 0x0A, 0x0B, 0x0C, 0x0D];
    T.get(audio_type as usize).copied().unwrap_or(0x13)
}

/// `0x476ED0` — a smackable's collision points from its article's vertices (article frame):
/// the vertices are split at the middle of the box's height (`y > mid` above, the rest below,
/// 0x4770FC…0x477156), each half is reduced to its **convex hull in x–z** (0x5A0480), the upper
/// hull collapses to its centre when it is under 0.1 wide both ways, and the points are laid at
/// the box's **top** (upper hull) and **bottom** (lower hull) height (0x477234…0x4772A0), upper
/// first, at most [`MAX_POINTS`].
///
/// MEDIUM on two inputs: the box is the vertices' own (the original reads the `Base` part's
/// `+0x14` / `+0x20`), and the hull is a monotone chain that drops collinear points — 0x5A0480 is
/// an incremental hull with the same 1e-8 duplicate test; the order it leaves the points in only
/// matters past the cap. An empty upper half (a flat article) gives no upper point here; what
/// 0x489BF0's untouched ±1e6 box does then was not followed.
pub fn point_cloud(vertices: &[Vec3]) -> Vec<Vec3> {
    let Some(first) = vertices.first() else { return Vec::new() };
    let (lo, hi) = vertices.iter().fold((*first, *first), |(a, b), v| (a.min(*v), b.max(*v)));
    let mid = (lo.y + hi.y) * 0.5;
    let upper: Vec<Vec3> = vertices.iter().filter(|v| v.y > mid).copied().collect();
    let lower: Vec<Vec3> = vertices.iter().filter(|v| v.y <= mid).copied().collect();
    let mut up = hull_xz(&upper);
    if let Some(f) = up.first().copied() {
        let (a, b) = up.iter().fold((f, f), |(a, b), p| (a.min(*p), b.max(*p)));
        if b.x - a.x < COLLAPSE && b.z - a.z < COLLAPSE {
            up = vec![Vec3::new((a.x + b.x) * 0.5, 0.0, (a.z + b.z) * 0.5)];
        }
    }
    let down = hull_xz(&lower);
    let mut out: Vec<Vec3> = up.iter().map(|p| Vec3::new(p.x, hi.y, p.z)).chain(down.iter().map(|p| Vec3::new(p.x, lo.y, p.z))).collect();
    out.truncate(MAX_POINTS);
    out
}

/// The convex hull of the points in x–z (y is dropped), counter-clockwise, without duplicates
/// closer than `√1e-8` (`[0x5C18E4]`) and without collinear points.
fn hull_xz(points: &[Vec3]) -> Vec<Vec3> {
    let mut p: Vec<(f32, f32)> = points.iter().map(|v| (v.x, v.z)).collect();
    p.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    p.dedup_by(|a, b| (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) <= 1e-8);
    if p.len() < 3 {
        return p.into_iter().map(|(x, z)| Vec3::new(x, 0.0, z)).collect();
    }
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut h: Vec<(f32, f32)> = Vec::with_capacity(2 * p.len());
    for &q in &p {
        while h.len() >= 2 && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    let lower_len = h.len() + 1;
    for &q in p.iter().rev().skip(1) {
        while h.len() >= lower_len && cross(h[h.len() - 2], h[h.len() - 1], q) <= 0.0 {
            h.pop();
        }
        h.push(q);
    }
    h.pop();
    h.into_iter().map(|(x, z)| Vec3::new(x, 0.0, z)).collect()
}

/// `max(|dx|, |dz|) + 0.25 · min(|dx|, |dz|)` — the octagonal distance 0x4A0F80 measures.
fn oct_distance(a: Vec3, b: Vec3) -> f32 {
    let (dx, dz) = ((a.x - b.x).abs(), (a.z - b.z).abs());
    dx.max(dz) + 0.25 * dx.min(dz)
}

/// One smackable: its car record and the fields of it the four handlers use.
#[derive(Debug, Clone, PartialEq)]
pub struct Smackable {
    /// The car record (`car+0x52C & 0x80`): pose, velocities, mass, body points, the hit flag.
    pub body: Vehicle,
    /// `car+0x7D` — asleep: every handler but 0x4A1030 returns at once.
    pub asleep: bool,
    /// `car+0x41C`, the ground height under it the last `Car_ground` saw.
    ground_h: f32,
}

impl Smackable {
    /// `Car_reset` 0x410730's smackable branch: 0x4A1280, `RigidBody_init` 0x49B510 and 0x4A1510.
    ///
    /// `orientation` is the authored one in the port's convention (rows = the body axes, the
    /// record's `rec+0x34`; each row is normalised here as 0x4A1561…0x4A157B does), `points` the
    /// collision points in that frame ([`point_cloud`]), `mass` the definition's (kg),
    /// `audio_type` its `kAud…` index.
    pub fn new(position: Vec3, orientation: Mat3, points: Vec<Vec3>, mass: f32, audio_type: u32, grid: &TriangleGrid) -> Self {
        let r = |i: usize| orientation.row(i).normalize_or_zero();
        let orientation = Mat3::from_cols(r(0), r(1), r(2)).transpose();
        let mut body = Vehicle::bare(position, orientation, 0.0, mass, 0.0, 0.0);
        // 0x4A1280: the half extents over the points, min and max starting at 0
        let (lo, hi) = points.iter().fold((Vec3::ZERO, Vec3::ZERO), |(a, b), p| (a.min(*p), b.max(*p)));
        body.half_extents = (hi - lo) * 0.5;
        body.body_points = points;
        body.smackable = true;
        body.material_class = class_for_audio(audio_type);
        body.collision_mass = mass / 128.0;
        body.sim_mass = mass;
        // 0x49B510 then 0x4A1510: at rest, asleep, undisturbed
        body.disturbed = false;
        body.knocked_over = false;
        body.gravity_scale = 1.0;
        body.tracker = crate::collision::PatchTracker::new(position);
        let mut s = Self { body, asleep: true, ground_h: position.y };
        // 0x4A1510: 0x4994F0 (`updateZone(pos, 1)`), 0x49AF70 — which returns at once, the body
        // being asleep — then 0x499520 on its own (0x4A167A…0x4A1686), then `Susp_AI`
        s.body.tracker.update_zone(grid, position);
        s.body.tracker.refresh_plane();
        s.ground_h = s.susp_ai(grid);
        s
    }

    /// The collision mass `car+0x354` in the original's units.
    pub fn mass(&self) -> f32 {
        self.body.collision_mass
    }

    /// `0x4A1030` (`+0xE1C`, 32 Hz, priority 0x1E): the damper and sleep controller. `cars` are
    /// the live cars of `0x5E8B34` — 0x4A0F80 wakes the prop when one is within 10 m by
    /// [`oct_distance`].
    pub fn damp(&mut self, cars: &[Vec3]) {
        let near = cars.iter().any(|c| oct_distance(self.body.position, *c) < 10.0);
        let b = &mut self.body;
        if self.asleep {
            // 0x4A103D: a disturbed prop that has gone to sleep is held still
            if b.disturbed {
                b.velocity = Vec3::ZERO;
                b.angular_velocity = Vec3::ZERO;
            }
            if near {
                self.asleep = false;
            }
            return;
        }
        if b.body_height < 0.005 {
            // on the ground (`car+0x424 < [0x5B2738]`): 0.925 on the velocity, 0.995 / 0.98 /
            // 0.995 on the spin (0x4A1090…0x4A1178)
            b.velocity *= 0.925;
            b.angular_velocity.x *= 0.995;
            b.angular_velocity.y *= 0.98;
            b.angular_velocity.z *= 0.995;
        } else {
            // in the air only a spin past 4 turns/s is bled off (0x4A10D0…0x4A1178)
            for w in [&mut b.angular_velocity.x, &mut b.angular_velocity.y, &mut b.angular_velocity.z] {
                if w.abs() > 4.0 {
                    *w *= 0.995;
                }
            }
        }
        let v = b.velocity.abs();
        let w = b.angular_velocity.abs();
        if v.x + v.y + v.z < 0.1 && w.x + w.y + w.z < 0.05 && b.min_height < 0.01 && !near {
            self.asleep = true;
        }
    }

    /// `Susp_AI` 0x4998C0 and `BodyHeight` 0x49A900's smackable arm: the tracker's plane under the
    /// position, and the body's height over it — `car+0x424` the lowest point's, `car+0x428` the
    /// mean of the three lowest (0x49AA57…0x49AB52, `[0x5B2680]` = 1/3) — over **all** the body
    /// points. Answers the plane's height at the position, or −2³¹ when the plane is steeper than
    /// `n.y < 0.1`.
    ///
    /// The plane is the body's [`crate::collision::PatchTracker`]: the patch normal and the centre
    /// `car+0x400` that 0x499520 kept (MEDIUM only in that a patch is one of the port's triangles).
    fn susp_ai(&mut self, grid: &TriangleGrid) -> f32 {
        let p = self.body.position;
        // `carTracker.updateZone(pos, 1)` (0x4998D8) — no plane refresh here — and its normal
        // (0x4998DF) through `car+0x400`
        self.body.tracker.update_zone(grid, p);
        let (n, q) = (self.body.tracker.normal, self.body.tracker.plane_point);
        let h = if n.y >= 0.1 { q.y - ((p.x - q.x) * n.x + (p.z - q.z) * n.z) / n.y } else { -2_147_483_648.0 };
        // BodyHeight 0x49A900, whose smackable arm is the all-points one (0x49A942)
        self.body.body_height_on(n, q);
        // `Damage`'s crash response tests `car+0x424` through this field
        self.body.height_marker = self.body.body_height;
        h
    }

    /// `Car_ground` 0x49BD40 (`+0xE20`, 32 Hz, priority 0x1E) for `flags & 0x80`, read again
    /// 2026-09-25 (0x49BD40…0x49C344): nothing while asleep or undisturbed (`!car+0x7C`,
    /// 0x49BD55); else `Susp_AI`, `dh = (h − car+0x41C)·rateHz`, and from the gravity on the body
    /// a car runs too, [`Vehicle::car_ground_body`] — where a smackable always takes the tilted
    /// test (0x49BF55) and keeps the surface's friction in it (0x49BFDF), its hard landing cannot
    /// happen (`car+0x424 < −0.05` after failing `< 0.001`), and `Susp_applyLift` returns at once
    /// for it (0x49AEC5). The plane point `car+0x404` of the 8 m ceiling rule is the tracker's
    /// patch centre (0x499520, [`crate::collision::PatchTracker`]).
    pub fn ground(&mut self, grid: &TriangleGrid) {
        if self.asleep || !self.body.disturbed {
            return;
        }
        // 0x49AF70 (priority 6, the same gates): the tracker and, on a new patch, 0x499520
        self.body.tracker.track(grid, self.body.position);
        let h = self.susp_ai(grid);
        // `rateHz` = 32 for a prop within 96 m of a racer (the LOD gate, not carried)
        let dh = (h - self.ground_h) * 32.0;
        self.ground_h = h;
        let b = &mut self.body;
        let (n, q) = (b.tracker.normal, b.tracker.plane_point);
        b.ground_normal = n;
        b.cos_tilt = n.dot(b.orientation.row(1));
        let mass = b.sim_mass;
        let surf = (b.tracker.word & 0xF) as usize;
        b.car_ground_body(dh, n, q, surf, mass, None, grid);
    }

    /// `0x498FF0` (`+0xE2C`, 32 Hz, priority 0x28) for this prop: nothing while asleep; once
    /// disturbed its walls (0x495020) and the track's props (0x498DE0); then the pair solver
    /// 0x4980D0 against every body already in this tick's list — **the prop is A**, the car B:
    /// the racers register first (0x41127C before 0x41137C) and 0x414C30 puts an equal priority
    /// after the ones already there, so every car's pass has run before any prop's. `others` are
    /// those bodies with their `car+0x354` in the original's units. Answers each other body's own
    /// impact `car+0x430` from the pair, when there was one.
    pub fn collide(&mut self, others: &mut [(&mut Vehicle, f32)], walls: &mut dyn FnMut(&mut Vehicle)) -> Vec<(usize, f32)> {
        let mut hits = Vec::new();
        if self.asleep {
            return hits;
        }
        if self.body.disturbed {
            walls(&mut self.body);
        }
        let mass = self.mass();
        for (i, (other, m)) in others.iter_mut().enumerate() {
            if let Some((_, e)) = collide_pair(&mut self.body, mass, other, *m, walls) {
                hits.push((i, e));
            }
        }
        hits
    }

    /// `0x410280` (`+0xE30`, priority 0x32): a prop's impact is spent — `car+0x430 / 0x434 /
    /// 0x438` cleared (0x410402…0x41040E). The sound event it would push first is not carried
    /// (module doc).
    pub fn post_impact(&mut self) {
        if !self.asleep {
            self.body.impact = 0.0;
        }
    }

    /// The 64 Hz integrators 0x49B7A0 / 0x49B8E0, which bail on `car+0x7D` (asleep).
    pub fn integrate(&mut self) {
        if !self.asleep {
            self.body.integrate64();
        }
    }
}

/// The pair pass among the props themselves: prop `k`'s 0x498FF0 meets the props before it in
/// the list that are awake (an asleep one never appends itself, 0x499000). Both are smackables,
/// so both masses are the original's units.
pub fn collide_among(props: &mut [Smackable], k: usize, walls: &mut dyn FnMut(&mut Vehicle)) {
    if props[k].asleep {
        return;
    }
    let (before, rest) = props.split_at_mut(k);
    let me = &mut rest[0];
    for o in before.iter_mut().filter(|o| !o.asleep) {
        let (ma, mb) = (me.mass(), o.mass());
        collide_pair(&mut me.body, ma, &mut o.body, mb, walls);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Triangle;

    fn flat() -> TriangleGrid {
        let s = 50.0;
        TriangleGrid::build(
            vec![
                Triangle::new(Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), 1),
                Triangle::new(Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s), 1),
            ],
            10.0,
        )
    }

    /// A cone: an octagonal base ring at y = 0 and a small top ring near the tip.
    fn cone_vertices() -> Vec<Vec3> {
        let mut v = Vec::new();
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::TAU / 8.0;
            v.push(Vec3::new(0.2 * a.cos(), 0.0, 0.2 * a.sin()));
            v.push(Vec3::new(0.02 * a.cos(), 0.5, 0.02 * a.sin()));
        }
        v
    }

    #[test]
    fn a_cone_is_its_base_ring_and_one_top_point() {
        let pts = point_cloud(&cone_vertices());
        // the top ring is 0.04 across: it collapses to its centre, laid at the top
        assert_eq!(pts[0], Vec3::new(0.0, 0.5, 0.0));
        assert_eq!(pts.len(), 1 + 8);
        assert!(pts[1..].iter().all(|p| p.y == 0.0 && (p.x.hypot(p.z) - 0.2).abs() < 1e-5));
    }

    #[test]
    fn the_cloud_is_capped_at_twenty() {
        let mut v = Vec::new();
        for i in 0..30 {
            let a = i as f32 * std::f32::consts::TAU / 30.0;
            v.push(Vec3::new(a.cos(), 0.0, a.sin()));
            v.push(Vec3::new(a.cos(), 1.0, a.sin()));
        }
        let pts = point_cloud(&v);
        assert_eq!(pts.len(), MAX_POINTS);
        // the upper hull comes first, so the cap cuts the lower one away entirely here
        assert!(pts.iter().all(|p| p.y == 1.0));
    }

    #[test]
    fn a_pylon_is_class_0x10() {
        assert_eq!(class_for_audio(4), 0x10);
        assert_eq!(class_for_audio(0), 0x13);
        assert_eq!(class_for_audio(99), 0x13);
    }

    fn cone() -> Smackable {
        Smackable::new(Vec3::ZERO, Mat3::IDENTITY, point_cloud(&cone_vertices()), 20.0, 4, &flat())
    }

    #[test]
    fn a_cone_is_built_asleep_undisturbed_and_light() {
        let c = cone();
        assert!(c.asleep && !c.body.disturbed && !c.body.knocked_over);
        assert!((c.mass() - 20.0 / 128.0).abs() < 1e-6);
        assert_eq!(c.body.material_class, 0x10);
        // the box's y runs from 0 (the origin) to the tip
        assert!((c.body.half_extents.y - 0.25).abs() < 1e-6);
        assert!(c.body.body_height.abs() < 1e-6, "{}", c.body.body_height);
    }

    #[test]
    fn a_car_nearby_wakes_it_and_a_touch_knocks_it_over() {
        let mut c = cone();
        c.damp(&[Vec3::new(30.0, 0.0, 0.0)]);
        assert!(c.asleep, "10 m by the octagonal metric");
        c.damp(&[Vec3::new(9.0, 0.0, 0.0)]);
        assert!(!c.asleep);
        // a car-sized box sliding into it at 15 m/s
        let mut car = Vehicle::bare(Vec3::new(-2.1, 0.3, 0.0), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        car.half_extents = Vec3::new(2.0, 0.6, 0.9);
        car.velocity = Vec3::new(15.0, 0.0, 0.0);
        let mut others = [(&mut car, 1300.0f32 / 128.0)];
        let hits = c.collide(&mut others, &mut |_| {});
        assert_eq!(hits.len(), 1);
        assert!(c.body.disturbed && c.body.knocked_over);
        // the car's own impact is the 9.0 of a body lighter than 0.65 (0x496B91)
        assert_eq!(hits[0].1, 9.0);
        assert!(c.body.velocity.x > 15.0, "the cone leaves faster than the car: {:?}", c.body.velocity);
        assert!(car.velocity.x < 15.0 && car.velocity.x > 14.5, "{:?}", car.velocity);
        // and it spins no faster than the light smackable's 6.75 turns/s
        assert!(c.body.angular_velocity.abs().max_element() <= 6.75 + 1e-4);
    }

    #[test]
    fn a_disturbed_cone_falls_and_settles() {
        let mut c = cone();
        c.asleep = false;
        c.body.disturbed = true;
        c.body.position.y = 1.0;
        let grid = flat();
        for _ in 0..200 {
            c.damp(&[Vec3::new(5.0, 0.0, 0.0)]);
            c.ground(&grid);
            c.post_impact();
            c.integrate();
            c.integrate();
        }
        assert!(c.body.position.y.abs() < 0.05, "{:?}", c.body.position);
        assert!(c.body.velocity.length() < 0.5, "{:?}", c.body.velocity);
    }
}
