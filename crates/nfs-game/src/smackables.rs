//! The race's smackables — the car records 0x4A17F0 makes at every race's init (0x414E17, no
//! condition) for the props `WTrack::load` collected into `[WTrack+0x34]`: the mission scene's
//! `kSmackable` GEOM elements (the skill tests' cones) and then the track's own placed
//! `kSmackable` articles — signs, hydrants, breakaway pipes, sawhorses … — at most 125
//! (0x4888B0, `RE/notes/suspension-collision.md` §9 "The track's smackables",
//! `skill-tests.md` §5.3 / §5.4). The bodies are `nfs_physics::smackable`; this module keeps them
//! for the race and runs their handlers in the priority list's order.
//!
//! *(Until 2026-09-26 only a skill test had them, `SkillTest::step_props`; a race's hydrants and
//! signs were drawn but took no hit — the port's 0x498DE0 has no object for them, rightly: the
//! loader 0x4750B0 builds none for `collideType` 3.)*

use crate::world::{SmackableOwner, TrackWorld};
use glam::{Mat3, Mat4, Quat, Vec3};
use nfs_physics::smackable::{collide_among, Smackable};
use nfs_physics::vehicle::Vehicle;

#[derive(Debug, Clone, Default)]
pub struct RaceSmackables {
    /// The bodies, built on the first [`Self::step`] from `TrackWorld::smackables` (0x4A17F0 at
    /// race init), in `[WTrack+0x34]` order — the k-th is `0x5E8C24[k]`.
    pub props: Vec<Smackable>,
    /// What each body draws: a mission element or a track article.
    pub owners: Vec<SmackableOwner>,
    /// Each smackable's `car+8`, the slice its tracker holds — what 0x40F480's second loop
    /// (0x40F502…0x40F523) compares a reset's candidate slice with, for every smackable of
    /// `0x5E8C24[]`, asleep or not ([`Self::slices`]).
    slices: Vec<usize>,
    spawned: bool,
    /// Each body's position and orientation as the tick began ([`Self::snapshot_prev`]), for the
    /// render-side blend [`Self::poses_blended`]. PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: render
    /// state only, never read by the simulation.
    prev: Vec<(Vec3, Mat3)>,
}

impl RaceSmackables {
    /// One 32 Hz tick of the smackables, in the order the priority list runs their handlers
    /// (`traffic.md` §14.2 / §21.5): per prop the tracker 0x49AF70 (priority 6: the racer distance
    /// `car+0x78`, the patch and the slice), then per prop `0x4A1030` and `Car_ground` 0x49BD40
    /// (both priority 0x1E, the latter behind its LOD gate), then per prop the collision pass
    /// 0x498FF0 (0x28) — against `cars`, the cars of `0x5E8B34[]` in their order (the human, the
    /// opponents, the cops, the traffic) with their `car+0x354` in the original's units (kg / 128),
    /// which were all entered in this tick's list before any prop, then against the props before
    /// it that are awake — and last per prop 0x410280 (0x32). The caller runs this after the cars'
    /// own collision pass.
    ///
    /// `walls` is what 0x4980D0 runs for each body between its rounds and 0x498FF0 for a
    /// disturbed prop: `Car_wallCollide` 0x495020 and the props' pass 0x498DE0. Answers the pairs
    /// that touched as (index into `cars`, that car's own `car+0x430`).
    ///
    /// `racers` is how many of `cars` lead the list as `0x5E8E2C[]`, the racers 0x417190 measures
    /// the props' distance to. `table` is the track's slice table for the props' own slices
    /// ([`Self::slices`]), `tick` the game tick (`gGameTick`, 64 Hz) the LOD gate staggers on and
    /// `n_cars` the number of cars in `0x5E8B34[]` (`[0x658080]`), which the k-th prop's slot
    /// `car+0` follows. Not carried (`skill-tests.md` §5.4): the props' own sound event (never
    /// heard in the original). (The 8 m ceiling rule is in `Vehicle::car_ground_body`;
    /// `car+0x43C` is only ever written 0, so its skip never runs.)
    #[allow(clippy::too_many_arguments)]
    pub fn step(&mut self, world: &TrackWorld, table: Option<&crate::race::SliceTable>, tick: u64, n_cars: usize, racers: usize, cars: &mut [(&mut Vehicle, f32)], walls: &mut dyn FnMut(&mut Vehicle)) -> Vec<(usize, f32)> {
        if !self.spawned {
            self.spawned = true;
            self.props = world
                .smackables
                .iter()
                .enumerate()
                .map(|(k, s)| {
                    let mut p = Smackable::new(s.position, s.orientation, s.points.clone(), s.mass, s.audio_type, &world.collision);
                    // `Car_reset(prop, [0x658080] + k)` (0x4110FF…0x41110B)
                    p.slot = n_cars + k;
                    p
                })
                .collect();
            self.owners = world.smackables.iter().map(|s| s.owner).collect();
            tracing::info!(props = self.props.len(), track = self.owners.iter().filter(|o| matches!(o, SmackableOwner::Article(_))).count(), "smackables spawned");
        }
        if self.props.is_empty() {
            return Vec::new();
        }
        // `car+8`: `Car_reset` 0x410730 seats the tracker on slice 0 (`setSlice(0)`, 0x41074A) and
        // 0x4A1510's 0x4994F0 walks it to the prop (`Tracker::update` 0x474440 — the port's
        // `find_slice`); from then on only the tracker slot 0x49AF70 (priority 6, ahead of this
        // tick's damper) moves it, and it bails while the prop is asleep or undisturbed
        // (0x49AF80…0x49AF90)
        // 0x417190 measures to the racers `0x5E8E2C[]`; 0x4A0F80 wakes on any live car of `0x5E8B34`
        let alive: Vec<Vec3> = cars.iter().map(|(c, _)| c.position).collect();
        let racing = &alive[..racers.min(alive.len())];
        if let Some(t) = table
            && self.slices.len() != self.props.len()
        {
            self.slices = self.props.iter().map(|p| t.find_slice(p.body.position, 0)).collect();
        }
        for (k, p) in self.props.iter_mut().enumerate() {
            // 0x49AF70 → 0x417190: the distance to the racers (`0x5E8E2C[]`)
            if p.track(&world.collision, racing)
                && let (Some(t), Some(s)) = (table, self.slices.get_mut(k))
            {
                *s = t.find_slice(p.body.position, *s);
            }
        }
        for p in self.props.iter_mut() {
            p.damp(&alive);
            p.ground(&world.collision, tick);
        }
        let mut hits = Vec::new();
        for k in 0..self.props.len() {
            let h = self.props[k].collide(cars, walls);
            for (i, effect) in &h {
                tracing::debug!(prop = k, owner = ?self.owners.get(k), car = i, effect, "car hit a smackable (0x498FF0: the prop is A)");
            }
            hits.extend(h);
            collide_among(&mut self.props, k, walls);
        }
        for p in self.props.iter_mut() {
            p.post_impact();
        }
        if tick.is_multiple_of(32) {
            for (k, p) in self.props.iter().enumerate().filter(|(_, p)| !p.asleep) {
                tracing::trace!(prop = k, owner = ?self.owners.get(k), pos = ?p.body.position, vel = ?p.body.velocity, spin = ?p.body.angular_velocity, h = p.body.body_height, "smackable awake");
            }
        }
        hits
    }

    /// The props' 64 Hz integrators (0x49B7A0 / 0x49B8E0), after the cars' — and every 32 ticks
    /// the re-orthonormalisation the port gives every body (0x4A4810).
    pub fn integrate(&mut self, tick: u64) {
        for p in self.props.iter_mut() {
            p.integrate();
            if tick.is_multiple_of(32) {
                p.body.renormalise();
            }
        }
    }

    /// Each prop's pose, body to world, for `TrackWorld::place_smackable` — the record's pose
    /// pointers are the car's (0x483590).
    pub fn poses(&self) -> impl Iterator<Item = (SmackableOwner, Mat4)> + '_ {
        self.owners.iter().zip(&self.props).map(|(o, p)| (*o, crate::world::body_pose(p.body.position, p.body.orientation)))
    }

    /// Each body's pose as the tick begins, the "previous tick" the render blend starts from
    /// (the cars' `render_prev` counterpart). Called once per simulated tick, before the step.
    pub fn snapshot_prev(&mut self) {
        self.prev.clear();
        self.prev.extend(self.props.iter().map(|p| (p.body.position, p.body.orientation)));
    }

    /// [`Self::poses`] drawn between the previous tick and this one by `alpha` (0 = the previous
    /// tick's pose, 1 = this tick's, which is all the original ever draws, 0x483590). Lerp of the
    /// position, slerp of the orientation, the cars' own `< 5 m` teleport guard; a body that did
    /// not move since the snapshot (every resting prop) returns exactly [`Self::poses`]' matrix.
    /// PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: drawing only.
    pub fn poses_blended(&self, alpha: f32) -> impl Iterator<Item = (SmackableOwner, Mat4)> + '_ {
        self.owners.iter().zip(&self.props).enumerate().map(move |(k, (o, p))| {
            let (pos, ori) = (p.body.position, p.body.orientation);
            let pose = match self.prev.get(k) {
                Some(&(p0, o0)) if (p0 != pos || o0 != ori) && p0.distance(pos) < 5.0 => {
                    let q0 = Quat::from_mat3(&o0.transpose()).normalize();
                    let q1 = Quat::from_mat3(&ori.transpose()).normalize();
                    Mat4::from_rotation_translation(q0.slerp(q1, alpha).normalize(), p0.lerp(pos, alpha))
                }
                _ => crate::world::body_pose(pos, ori),
            };
            (*o, pose)
        })
    }

    /// The smackables' slices (`car+8`) for `Car_resetToSlice` 0x40F540's free-slice test
    /// 0x40F480: a candidate slice any smackable holds is taken, whether the prop is asleep or
    /// not (the loop 0x40F502…0x40F523 reads every `0x5E8C24[]` entry with no gate). Empty before
    /// the props are built or without a slice table.
    pub fn slices(&self) -> &[usize] {
        &self.slices
    }

    /// Whether the mission element `element`'s body has been flagged by the solver
    /// (`car+0x42C`, 0x496A10).
    pub fn knocked(&self, element: usize) -> Option<bool> {
        let k = self.owners.iter().position(|o| *o == SmackableOwner::Element(element))?;
        Some(self.props[k].body.knocked_over)
    }

    /// How many smackables the solver has flagged (`car+0x42C != 0`), over **every** entry of
    /// `0x5E8C24[0 .. NUMSMACKABLES]` — the mission scene's and the track's own alike: what the
    /// HUD counts (0x425DAA…0x425DCC) and what the race end stores as `player+0xB60`
    /// (0x416ACF…0x416B09). Only a class-0x10 prop (`kAudPylon`: `CONE`, `PLON`) is ever flagged.
    pub fn knocked_count(&self) -> usize {
        self.props.iter().filter(|p| p.body.knocked_over).count()
    }
}

#[cfg(test)]
impl RaceSmackables {
    /// Test-only: a spawned set of resting props at the given poses (orientation in the body
    /// convention `Smackable::new` takes), owned by `SmackableOwner::Article(k)`, on an empty grid.
    fn for_test(poses: &[(Vec3, Mat3)]) -> Self {
        let grid = nfs_physics::collision::TriangleGrid::build(Vec::new(), 8.0);
        let props = poses
            .iter()
            .map(|&(p, o)| Smackable::new(p, o, vec![Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5)], 20.0, 4, &grid))
            .collect();
        Self { props, owners: (0..poses.len()).map(SmackableOwner::Article).collect(), spawned: true, ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Mat4, b: Mat4) -> bool {
        a.to_cols_array().iter().zip(b.to_cols_array()).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    fn blended(s: &RaceSmackables, alpha: f32) -> Vec<Mat4> {
        s.poses_blended(alpha).map(|(_, m)| m).collect()
    }

    /// D8: a body that did not move since the snapshot (every resting prop) returns exactly
    /// [`RaceSmackables::poses`]' matrix, whatever `alpha`.
    #[test]
    fn a_resting_prop_returns_its_old_matrix() {
        let mut s = RaceSmackables::for_test(&[(Vec3::new(3.0, 0.0, 4.0), Mat3::from_rotation_y(0.7))]);
        s.snapshot_prev();
        let at_rest: Vec<Mat4> = s.poses().map(|(_, m)| m).collect();
        for alpha in [0.0, 0.3, 1.0] {
            assert_eq!(blended(&s, alpha), at_rest, "alpha {alpha}");
        }
        // no snapshot yet: the tick's pose
        let fresh = RaceSmackables::for_test(&[(Vec3::X, Mat3::IDENTITY)]);
        assert_eq!(blended(&fresh, 0.5), fresh.poses().map(|(_, m)| m).collect::<Vec<_>>());
    }

    /// A body that moved 2 m (and turned a quarter turn) since the snapshot is drawn between the two
    /// poses: lerp of the position, slerp of the orientation.
    #[test]
    fn a_moving_prop_blends() {
        let mut s = RaceSmackables::for_test(&[(Vec3::new(1.0, 0.0, 1.0), Mat3::IDENTITY)]);
        s.snapshot_prev();
        s.props[0].body.position = Vec3::new(3.0, 0.0, 1.0);
        let turn = std::f32::consts::FRAC_PI_2;
        s.props[0].body.orientation = Mat3::from_rotation_y(turn);
        let pose = |x: f32, a: f32| crate::world::body_pose(Vec3::new(x, 0.0, 1.0), Mat3::from_rotation_y(a));
        assert!(close(blended(&s, 0.0)[0], pose(1.0, 0.0)), "alpha 0 is the previous tick");
        assert!(close(blended(&s, 0.5)[0], pose(2.0, turn / 2.0)), "halfway");
        assert!(close(blended(&s, 1.0)[0], pose(3.0, turn)), "alpha 1 is this tick");
        assert_ne!(blended(&s, 0.5)[0], s.poses().next().unwrap().1);
    }

    /// The `< 5 m` teleport guard: a jump of more than 5 m is drawn at the new pose, not blended.
    #[test]
    fn a_jump_over_five_metres_is_not_blended() {
        let mut s = RaceSmackables::for_test(&[(Vec3::ZERO, Mat3::IDENTITY), (Vec3::ZERO, Mat3::IDENTITY)]);
        s.snapshot_prev();
        s.props[0].body.position = Vec3::new(5.5, 0.0, 0.0);
        s.props[1].body.position = Vec3::new(4.5, 0.0, 0.0);
        let now: Vec<Mat4> = s.poses().map(|(_, m)| m).collect();
        let b = blended(&s, 0.5);
        assert_eq!(b[0], now[0], "5.5 m: the new pose");
        assert_ne!(b[1], now[1], "4.5 m: blended");
        assert!(close(b[1], crate::world::body_pose(Vec3::new(2.25, 0.0, 0.0), Mat3::IDENTITY)));
    }
}
