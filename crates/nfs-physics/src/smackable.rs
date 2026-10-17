//! Smackables — the world props the race gives a car record of their own (`car+0x52C & 0x80`),
//! the Factory Driver cones among them (`RE/notes/skill-tests.md` §5.3, `traffic.md` §8b / §15).
//! Read 2026-09-25; CONFIRMED unless a line says otherwise.
//!
//! **Creation.** `WAnimatableBundle` 0x4772F0 builds a 56-byte record for every article whose
//! `animdefs.txt` definition says `kSmackable` (`SIGN` through **0x476B30** —
//! [`sign_point_cloud`] — every other one through **0x476ED0** — [`point_cloud`]); 0x483360 keeps its point cloud (`rec+0xC`, count `+0x10`), its
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
//! **Handlers** (`Car_installHandlers` 0x410D60, 0x410E43): the tracker slot 0x49AF70
//! ([`Smackable::track`], priority 6), `+0xE1C` = **0x4A1030** ([`Smackable::damp`]),
//! `+0xE20` = `Car_ground` 0x49BD40 whose `& 0x80` fork takes `Susp_AI` ([`Smackable::ground`]),
//! `+0xE2C` = the collision pass 0x498FF0 ([`Smackable::collide`]), `+0xE30` = 0x410280
//! ([`Smackable::post_impact`]), and the 64 Hz integrators 0x49B7A0 / 0x49B8E0 ([`Smackable::integrate`]).
//! `+0xE10` / `+0xE14` are cleared: there is no drive and no input.
//!
//! **The LOD gate** (2026-09-26): `Car_ground` asks `Lod_shouldUpdate(1, 3, car+0x78, car+0, …,
//! car+0x10B8)` (0x49BD6B…0x49BD8C, [`crate::vehicle::ground_due`]) for a prop too. `car+0x78` is
//! 0x417190's octagonal x–z distance to the nearest racer of `0x5E8E2C[]`, from 1000, and only
//! 0x49AF70 calls it (0x49AF99), behind the same three gates `Car_ground` has (`+0x7F`, `!+0x7D`,
//! `+0x7C`), so the value is always fresh when `Car_ground` reads it: a prop can only be disturbed
//! while awake, and the next tick's tracker (priority 6) runs before its damper can put it to sleep.
//! `car+0x10B8` is 0: `Car_reset` falls through from the smackable branch (0x4107F6 → 0x410A37) to
//! `AI_resetState` 0x406D50 (0x410CC8), which stores `flags & 1` there (0x406E16…0x406E7F) and a
//! smackable has no racer bit. `car+0` is `(slot | 0x100)` with the slot `[0x658080] + k` for the
//! k-th smackable (the race init's second loop, 0x4110ED…0x41111E, 0x4107AE → `RigidBody_init`
//! 0x49B51D) — [`Smackable::slot`]. So a prop within 96 m of a racer settles at 32 Hz, farther ones
//! at 16 / 8 Hz with the gravity of 2 / 4 ticks.
//!
//! `0x49AF70`'s slice half lives with the slice table, in `nfs_game::smackables::RaceSmackables`.
//!
//! **A heavy, long prop rocks itself off the ground — as the original does** (read 2026-09-26,
//! `RE/notes/suspension-collision.md` §9 "The heavy smackables", HIGH): the mass and the inertia
//! unit `6m` come from the definition (0x4A12B5 → 0x49B510), not the points' box, and
//! `GroundImpulse` 0x495F30 changes a contact's normal speed by `0.6·(1.5 + ρ²)/(1.5 + ρ)` times
//! its approach, `ρ` its distance from the origin across the normal — past 2.14 m the contact is
//! thrown back, past 4.17 m faster than it came. A 7000 kg tanker's ends are 4.4 m out; the port
//! keeps the original's arithmetic (the test `a_long_heavy_smackable_rocks_itself_up`).
//!
//! **Every race has them** (2026-09-26): the bundles are the mission scene's and the track's
//! instance bundle, collected into `[WTrack+0x34]` up to 125 (0x4888B0) — the track's placed
//! `SIGN`s, hydrants, sawhorses … (`RE/notes/suspension-collision.md` §9 "The track's
//! smackables"); `nfs_game::world::TrackWorld::smackables` builds both.
//!
//! **0x410280's sound event is never heard — CONFIRMED (2026-09-26), so nothing is played for it.**
//! A smackable pushes `{1, −1, idA, idB, min(0x430, 50), 0}` onto its own queue `car+0x1110`
//! (count `car+0x110C`, at most 7) when `0x430 > 0.15` (`[0x5B2638]`, 0x4102E1…0x4103F8): `idA =
//! 0x5CC518[kind]`, replaced by **0 (Tires) on a pylon** (`car+0x44C == 0x10`, 0x41032E) for the
//! ground / car kinds, and `idB` the event's low byte (a car contact, a wall), `0x5CC4D8[·]` for the
//! ground or `0x5CC534[·]` for a prop without a car. The only readers of `car+0x110C` in the audio
//! module are 0x4A73DE (0x4A7080, whose car comes from the slot table 0x655D60 that 0x4A7DC0 fills
//! from `0x5E8B34[]` — a smackable is not in it) and 0x4A7AF8 (0x4A7660, the human); 0x498220
//! (← 0x4150A4) zeroes the count of every car of `0x5E8B34` and **every** entry of `0x5E8C24`.
//! What a cone hit sounds like is the racer's own event — CarBody × the prop's material, since the
//! solver writes the racer's `car+0x438 = 0x50000 | smackable+0x44C` (0x496CA0…0x496CBD,
//! [`crate::vehicle::Vehicle::pair_event`]): **Pylon**, group 0x56, for a cone; *corrected
//! 2026-09-26 from "CarBody × CarBody"* — and the HUD count's 0x4AAE50 (`Sfx::pylon_hit`). *(2026-09-25:)* the tracker and its plane (0x49AF70 without
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

/// `0x483360`'s centring of a smackable's record (called by 0x4772F0 after 0x476B30 / 0x476ED0,
/// 0x477515): the bounding box of the points is taken per axis (min / max seeded `±1e8`,
/// `0x4833F9…0x483419`), its centre `c = (min + max) / 2` (`[0x5B23F0]` = 0.5, 0x4834BD…0x4834DF)
/// becomes the body's origin — every point loses `c` (`rec+0..8 = −c`, added to each point at
/// 0x48352B…0x48356F) and the record's position `rec+0x28` is the item's position **plus `c`**
/// (0x483503…0x483520: `item − (−c)`, `c` added as it is, not rotated by the item's matrix).
/// So a sign (points 0 … 3.2 m up from its foot) stands on a body whose origin is its middle,
/// 1.59 m up, and a cone (tip 0.74, base 0) on one 0.37 m up; the body then rotates about that
/// centre, which is what the original's live records hold (the server's alps `SIGN` 0: position
/// = the `tr` position + (0.04, 1.588, 0.01), points y ∈ ±1.588). Returns the record's position;
/// `points` are centred in place.
pub fn centre_record(points: &mut [Vec3], position: Vec3) -> Vec3 {
    if points.is_empty() {
        return position;
    }
    let (lo, hi) = points.iter().fold((Vec3::splat(1e8), Vec3::splat(-1e8)), |(a, b), p| (a.min(*p), b.max(*p)));
    let c = (lo + hi) * 0.5;
    for p in points.iter_mut() {
        *p -= c;
    }
    position + c
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
    hull_xz_points(points).into_iter().map(|v| Vec3::new(v.x, 0.0, v.z)).collect()
}

/// [`hull_xz`] keeping each hull point whole: 0x5A0480 copies the 16-byte points it keeps
/// (0x5A04CA…0x5A04E0, 0x5A0525…0x5A0538), so a hull point carries its own y. Of points closer
/// than `√1e-8` in x–z the first in input order stays (a stable sort).
fn hull_xz_points(points: &[Vec3]) -> Vec<Vec3> {
    let mut p: Vec<Vec3> = points.to_vec();
    p.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.z.total_cmp(&b.z)));
    p.dedup_by(|a, b| (a.x - b.x).powi(2) + (a.z - b.z).powi(2) <= 1e-8);
    if p.len() < 3 {
        return p;
    }
    let cross = |o: Vec3, a: Vec3, b: Vec3| (a.x - o.x) * (b.z - o.z) - (a.z - o.z) * (b.x - o.x);
    let mut h: Vec<Vec3> = Vec::with_capacity(2 * p.len());
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
    h
}

/// `[0x5B2638]` = 0.15: half a sign's thickness (0x476D0C / 0x476D4D).
const SIGN_HALF_DEPTH: f32 = 0.15;

/// **0x476B30** — a `SIGN` smackable's collision points (the one definition 0x4772F0 sends here,
/// 0x477441), from its article's vertices (article frame) and the axis lengths of its instance's
/// 3×3 (0x476A80 splits the record's matrix into the unscaled rows and these; 0x539B30 /
/// 0x539F50 scale the vertices by them — the other smackables' 0x476ED0 takes no scale):
///
/// 1. the scaled vertices turned **90° about x** (0x539BD0(90, 1, 0, 0), 0x476C9B…0x476CC3), so
///    the sign's face lies in x–z;
/// 2. their x–z hull (0x5A0480), each hull point keeping its turned y — the face's depth;
/// 3. every hull point twice, at `y − 0.15` and then all of them at `y + 0.15` (0x476CF6…0x476D64),
///    turned back (0x539DB0, the rigid inverse, 0x476D8E…0x476DA3);
/// 4. the lowest `y` of them all (from 1e9, 0x476DAA…0x476DCA); of the points at or under that
///    `+ 0.1` (`[0x5B2604]`) only the **first** with `z > 0.001` (`[0x5B27B4]`) and the first with
///    `z ≤ 0.001` stay, every higher point stays (0x476DED…0x476E72). No cap.
///
/// So a sign is a 0.3 m slab of its face's outline standing on two points. MEDIUM: the turn's
/// sense (glam's right-handed `x` rotation; the ±0.15 pair is symmetric, only which of the two
/// sets comes first changes) and the hull's order, which picks the two kept bottom points; the
/// vertices are the port's article meshes (the original reads the `vt` of the LOD record
/// `Base+0x64 + 12·Base[+4]`, 0x476BA6…0x476C1E).
pub fn sign_point_cloud(vertices: &[Vec3], axis_lengths: Vec3) -> Vec<Vec3> {
    // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable rotation, not glam/std; original smackable set-up 0x476BA6…0x476E72.
    let turn = nfs_math::portable::quat_rotation_x(90f32.to_radians());
    let turned: Vec<Vec3> = vertices.iter().map(|v| turn * (*v * axis_lengths)).collect();
    let hull = hull_xz_points(&turned);
    let back = turn.inverse();
    let pts: Vec<Vec3> = hull
        .iter()
        .map(|h| Vec3::new(h.x, h.y - SIGN_HALF_DEPTH, h.z))
        .chain(hull.iter().map(|h| Vec3::new(h.x, h.y + SIGN_HALF_DEPTH, h.z)))
        .map(|p| back * p)
        .collect();
    let floor = pts.iter().map(|p| p.y).fold(1.0e9f32, f32::min) + COLLAPSE;
    let (mut front, mut rear) = (false, false);
    pts.into_iter()
        .filter(|p| {
            if floor < p.y {
                return true;
            }
            let side = if p.z > 0.001 { &mut front } else { &mut rear };
            !std::mem::replace(side, true)
        })
        .collect()
}

/// `max(|dx|, |dz|) + 0.25 · min(|dx|, |dz|)` — the octagonal distance 0x4A0F80 measures.
fn oct_distance(a: Vec3, b: Vec3) -> f32 {
    let (dx, dz) = ((a.x - b.x).abs(), (a.z - b.z).abs());
    crate::vehicle::norm_est(dx, dz)
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
    /// `car+0x78` — the octagonal x–z distance to the nearest racer, which only [`Self::track`]
    /// (0x49AF70 → 0x417190) writes. Its value before the first write is never read (module doc).
    pub racer_distance: f32,
    /// `car+0`'s slot, `[0x658080] + k` — the number of cars in `0x5E8B34[]` plus the prop's index
    /// in `0x5E8C24[]` (0x4110FF…0x41110B) — which staggers the LOD gate ([`crate::vehicle::ground_due`]).
    pub slot: usize,
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
        let mut s = Self { body, asleep: true, ground_h: position.y, racer_distance: 0.0, slot: 0 };
        // 0x4A1510: 0x4994F0 (`updateZone(pos, 1)`), 0x49AF70 — which returns at once, the body
        // being asleep — then 0x499520 on its own (0x4A167A…0x4A1686), then `Susp_AI`
        s.body.tracker.update_zone(grid, position);
        s.body.tracker.refresh_plane();
        s.ground_h = s.susp_ai(grid);
        s
    }

    /// **TEST GLUE** (`NFS_SMACK_RAM`): sets the awake body down at `position` with `orientation`
    /// (rows = the body axes), at rest, its tracker and ground read afresh — the in-game memory
    /// writes of the reference run on the original (`REFERENCE-2026-10-17-SIGN.md`).
    pub fn relocate(&mut self, grid: &TriangleGrid, position: Vec3, orientation: Mat3) {
        self.body.position = position;
        self.body.orientation = orientation;
        self.body.velocity = Vec3::ZERO;
        self.body.angular_velocity = Vec3::ZERO;
        self.body.tracker = crate::collision::PatchTracker::new(position);
        self.body.tracker.update_zone(grid, position);
        self.body.tracker.refresh_plane();
        self.ground_h = self.susp_ai(grid);
        self.asleep = false;
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
    /// points. Answers the plane's height at the position, or **−2 097 152 000** (`0xCEFA0000`,
    /// 0x49993E) when the plane is as steep as `n.y ≤ 0.1` (`test ah, 0x41`, 0x4998FD…0x499908) —
    /// and then `BodyHeight` is **not** run: `car+0x424` / `+0x428` keep the last tick's.
    /// *(Corrected 2026-09-26: the port answered −2³¹, took `n.y = 0.1` as flat enough and
    /// measured the body on the steep plane too.)*
    ///
    /// The plane is the body's [`crate::collision::PatchTracker`]: the patch normal and the centre
    /// `car+0x400` that 0x499520 kept (MEDIUM only in that a patch is one of the port's triangles).
    fn susp_ai(&mut self, grid: &TriangleGrid) -> f32 {
        let p = self.body.position;
        // `carTracker.updateZone(pos, 1)` (0x4998D8) — no plane refresh here — and its normal
        // (0x4998DF) through `car+0x400`
        self.body.tracker.update_zone(grid, p);
        let (n, q) = (self.body.tracker.normal, self.body.tracker.plane_point);
        if n.y <= 0.1 {
            return -2_097_152_000.0;
        }
        let h = q.y - ((p.x - q.x) * n.x + (p.z - q.z) * n.z) / n.y;
        // BodyHeight 0x49A900 (0x499934), whose smackable arm is the all-points one (0x49A942)
        self.body.body_height_on(n, q);
        // `Damage`'s crash response tests `car+0x424` through this field
        self.body.height_marker = self.body.body_height;
        h
    }

    /// The tracker slot `0x49AF70` (32 Hz, priority 6 — ahead of every car's drive and ground
    /// entries): nothing while asleep or undisturbed (0x49AF75…0x49AF90); else 0x417190 —
    /// `car+0x78` = the least octagonal x–z distance ([`oct_distance`]) from the prop to a racer
    /// of `0x5E8E2C[]` (`racers`: the humans and the opponents), starting from 1000 (0x4171AA) —
    /// then the tracker (`Tracker::update` / `updateZone` and, on a new patch, 0x499520; the slice
    /// half is the caller's). Answers whether it ran.
    pub fn track(&mut self, grid: &TriangleGrid, racers: &[Vec3]) -> bool {
        if self.asleep || !self.body.disturbed {
            return false;
        }
        self.racer_distance = racers.iter().map(|r| oct_distance(self.body.position, *r)).fold(1000.0, f32::min);
        self.body.tracker.track(grid, self.body.position);
        true
    }

    /// `Car_ground` 0x49BD40 (`+0xE20`, 32 Hz, priority 0x1E) for `flags & 0x80`, read again
    /// 2026-09-25 (0x49BD40…0x49C344): nothing while asleep or undisturbed (`!car+0x7C`,
    /// 0x49BD55), nor when the LOD gate says the prop is not due (`tick` is `gGameTick`, module
    /// doc); else `Susp_AI`, `dh = (h − car+0x41C)·rateHz`, and from the gravity (× `nTicks`) on the body
    /// a car runs too, [`Vehicle::car_ground_body`] — where a smackable always takes the tilted
    /// test (0x49BF55) and keeps the surface's friction in it (0x49BFDF), its hard landing cannot
    /// happen (`car+0x424 < −0.05` after failing `< 0.001`), and `Susp_applyLift` returns at once
    /// for it (0x49AEC5). The plane point `car+0x404` of the 8 m ceiling rule is the tracker's
    /// patch centre (0x499520, [`crate::collision::PatchTracker`]).
    pub fn ground(&mut self, grid: &TriangleGrid, tick: u64) {
        if self.asleep || !self.body.disturbed {
            return;
        }
        // 0x49BD6B…0x49BD96: `Lod_shouldUpdate(1, 3, car+0x78, car+0, …, car+0x10B8 = 0)`
        let Some((rate_hz, n_ticks)) = crate::vehicle::ground_due(self.racer_distance, false, tick, self.slot) else { return };
        let h = self.susp_ai(grid);
        // 0x49BDC5…0x49BDCD: `fimul rateHz`
        let dh = (h - self.ground_h) * rate_hz;
        self.ground_h = h;
        let b = &mut self.body;
        let (n, q) = (b.tracker.normal, b.tracker.plane_point);
        b.ground_normal = n;
        b.cos_tilt = n.dot(b.orientation.row(1));
        let mass = b.sim_mass;
        let surf = (b.tracker.word & 0xF) as usize;
        b.car_ground_body(dh, n, q, surf, mass, None, grid, n_ticks, false); // a smackable is no racer (`car+0x52C` 0x80)
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
    /// 0x438` cleared (0x410402…0x41040E). The sound event it pushes first is dropped unheard in
    /// the original (module doc), so the port does not make one.
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

    /// A sign: a 1 × 0.6 face on a 1.6 m post, 2 cm thick, in the article's x–y plane.
    #[test]
    fn a_sign_is_a_slab_of_its_outline_on_two_points() {
        let mut v = Vec::new();
        for z in [-0.01f32, 0.01] {
            for (x, y) in [(-0.5, 1.6), (0.5, 1.6), (0.5, 2.2), (-0.5, 2.2), (-0.03, 0.0), (0.03, 0.0), (0.03, 1.6), (-0.03, 1.6)] {
                v.push(Vec3::new(x, y, z));
            }
        }
        let pts = sign_point_cloud(&v, Vec3::ONE);
        // the face's corners and the post's foot, each on both sides of the slab ...
        assert!(pts.iter().all(|p| p.z.abs() > 0.1 && p.z.abs() < 0.2), "{pts:?}");
        assert!(pts.iter().any(|p| (p.y - 2.2).abs() < 1e-5));
        // ... of which only two stand on the ground, one each side
        let feet: Vec<&Vec3> = pts.iter().filter(|p| p.y < 0.1).collect();
        assert_eq!(feet.len(), 2, "{pts:?}");
        assert!(feet[0].z * feet[1].z < 0.0);
        // scaled by the instance's axis lengths
        let big = sign_point_cloud(&v, Vec3::new(2.0, 1.0, 1.0));
        assert!(big.iter().any(|p| (p.x.abs() - 1.0).abs() < 1e-5));
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
    fn a_far_prop_settles_at_the_lod_rate() {
        // 110 m from the racer: row 3 index 6 → 16 Hz, `nTicks` 2, due when
        // `(slot | 0x100) + tick/2` is odd (mask 1); the gravity covers the skipped tick
        let grid = flat();
        let mut c = cone();
        c.asleep = false;
        c.body.disturbed = true;
        c.body.position.y = 1.0;
        c.slot = 1;
        let racer = [Vec3::new(110.0, 0.0, 0.0)];
        assert!(c.track(&grid, &racer));
        assert_eq!(c.racer_distance, 110.0);
        // past the first 15 game ticks, where every car is due
        let (vy0, y0) = (c.body.velocity.y, c.body.position.y);
        c.ground(&grid, 34);
        assert_eq!((c.body.velocity.y, c.body.position.y), (vy0, y0), "(0x101 + 17) & 1 == 0: not due");
        c.ground(&grid, 36);
        let g = c.body.velocity.y - vy0;
        assert!((g + 2.0 * 9.8 / 32.0).abs() < 0.05, "two ticks of gravity: {g}");
    }

    /// A box of the given half length (x), 2.6 m wide and 3 m tall, as 0x476ED0 would lay it: the
    /// roof's four corners and the floor's four.
    fn crate_points(half_len: f32) -> Vec<Vec3> {
        let mut v = Vec::new();
        for y in [3.0, 0.0] {
            for (x, z) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                v.push(Vec3::new(x * half_len, y, z * 1.3));
            }
        }
        v
    }

    /// `GroundImpulse` 0x495F30's arithmetic throws a contact farther than 2.14 m from the origin
    /// back and one past 4.17 m back faster than it came (`suspension-collision.md` §6.3, §9 "The
    /// heavy smackables"): a 7000 kg body 8.8 m long, given a 0.05 turns/s pitch on a flat plane,
    /// rocks itself off the ground; the same body 1.8 m long settles.
    #[test]
    fn a_long_heavy_smackable_rocks_itself_up() {
        let grid = flat();
        let run = |half_len: f32| {
            let mut s = Smackable::new(Vec3::new(0.0, 0.01, 0.0), Mat3::IDENTITY, crate_points(half_len), 7000.0, 7, &grid);
            s.asleep = false;
            s.body.disturbed = true;
            s.body.angular_velocity = Vec3::new(0.0, 0.0, 0.05);
            let racer = [Vec3::new(5.0, 0.0, 0.0)];
            let mut top = 0.0f32;
            for i in 0..320u64 {
                s.track(&grid, &racer);
                s.damp(&racer);
                s.ground(&grid, 2 * i + 100);
                s.integrate();
                s.integrate();
                top = top.max(s.body.position.y);
            }
            top
        };
        let (long, short) = (run(4.4), run(0.9));
        assert!(long > 1.0, "the long body climbs: {long}");
        assert!(short < 0.1, "the short one settles: {short}");
    }

    /// 0x483360 centres a record on its points' bounding box: a sign's 3.2 m of points sit
    /// ±1.6 m about the body origin, which is the item's position plus the centre (the server's
    /// alps `SIGN` 0: position = the `tr` position + (0.04, 1.588, 0.01), points y ∈ ±1.588).
    #[test]
    fn a_record_is_centred_on_its_points() {
        let mut pts = vec![Vec3::new(-0.72, 1.69, 0.15), Vec3::new(-0.02, 0.0, 0.17), Vec3::new(0.79, 3.18, -0.15)];
        let at = centre_record(&mut pts, Vec3::new(88.5, 15.85, -5212.87));
        assert!((at - Vec3::new(88.535, 17.44, -5212.86)).abs().max_element() < 1e-3, "{at:?}");
        let (lo, hi) = pts.iter().fold((Vec3::splat(1e8), Vec3::splat(-1e8)), |(a, b), p| (a.min(*p), b.max(*p)));
        assert!((lo + hi).abs().max_element() < 1e-5, "centred: {lo:?} {hi:?}");
        assert!(centre_record(&mut [], Vec3::ONE) == Vec3::ONE);
    }

    /// A 13 m/s car into an 80 kg sign (collision mass 0.625) at its middle: the impact is over 80,
    /// so `Damage`'s heavy arm runs **inside** the solver, on the spin in rad/s (the original's own
    /// call sees (2.5, 1.4, 5.1) where the tick ends with 2π times less — a live hit,
    /// `RE/notes/REFERENCE-2026-10-17-SIGN.md`). The sign leaves with the car's speed and a spin
    /// under a turn a second; run on turns/s the same hit drew the random ±10 turns/s spin
    /// (past 5 turns/s after the clamp) and made a 6-turn tumble.
    #[test]
    fn a_car_hits_a_sign_in_the_solvers_units() {
        let grid = flat();
        let mut pts = vec![];
        for z in [-0.16f32, 0.16] {
            for (x, y) in [(-0.76, 1.7), (0.76, 1.7), (0.76, 3.2), (-0.76, 3.2), (0.0, 0.0)] {
                pts.push(Vec3::new(x, y, z));
            }
        }
        let at = centre_record(&mut pts, Vec3::new(0.0, 0.1, 0.0));
        let mut s = Smackable::new(at, Mat3::IDENTITY, pts, 80.0, 1, &grid);
        s.asleep = false;
        let mut car = Vehicle::bare(Vec3::new(0.0, 1.4, -2.5), Mat3::IDENTITY, 2.5, 1300.0, 0.8, 0.3);
        car.half_extents = Vec3::new(0.9, 0.5, 2.2);
        car.velocity = Vec3::new(0.0, 0.0, 13.0);
        let mut others = [(&mut car, 1300.0f32 / 128.0)];
        let mut hit = false;
        for _ in 0..3 {
            if !s.collide(&mut others, &mut |_| {}).is_empty() {
                hit = true;
                break;
            }
            others[0].0.position += others[0].0.velocity / 32.0;
        }
        assert!(hit, "the car reaches the sign");
        let w = s.body.angular_velocity.abs().max_element();
        assert!(w < 1.0, "{w} turns/s: {:?}", s.body.angular_velocity);
        assert!(s.body.velocity.z > 13.0 && s.body.velocity.y > 3.0, "{:?}", s.body.velocity);
    }

    #[test]
    fn a_disturbed_cone_falls_and_settles() {
        let mut c = cone();
        c.asleep = false;
        c.body.disturbed = true;
        c.body.position.y = 1.0;
        let grid = flat();
        let racer = [Vec3::new(5.0, 0.0, 0.0)];
        for i in 0..200u64 {
            assert!(c.track(&grid, &racer) || c.asleep);
            c.damp(&racer);
            c.ground(&grid, 2 * i);
            c.post_impact();
            c.integrate();
            c.integrate();
        }
        assert!(c.body.position.y.abs() < 0.05, "{:?}", c.body.position);
        assert!(c.body.velocity.length() < 0.5, "{:?}", c.body.velocity);
    }
}
