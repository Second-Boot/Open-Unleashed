//! Factory Driver skill tests (`RACE_TYPE 4`), from `RE/notes/skill-tests.md` (CONFIRMED unless noted).
//!
//! * The `_st<N>.scn` scene holds the start pose (type-2 trigger), checkpoints (type 0, taken
//!   strictly in sequence-id order), the finish (type 1), cones (`CONE` smackables) and arrow
//!   markers (`ARW*`, shown one at a time through the trigger link).
//! * Per tick (§7): the next checkpoint fires when one of the car's side segments crosses its
//!   line within ±3.5 m height (or the car centre is inside its sphere); the finish stores the
//!   race time; a wreck about to auto-reset counts as DNF except on the skidpad.
//! * Evaluation (§10): min rank ⇒ position; else time + cone penalty < limit; damage cap; cone cap.
//! * Cones (§5.3, §8): each is a **smackable**, a car record of its own (`car+0x52C & 0x80`)
//!   simulated by the car physics — `nfs_physics::smackable`, spawned from the scene's
//!   `kSmackable` elements (`TrackWorld::smackables`) and stepped in the handlers' order by
//!   [`SkillTest::step_props`] / [`SkillTest::integrate_props`]. A cone counts as hit when the
//!   car–car solver flags it (`car+0x42C`, 0x496A10: any contact that is not separating, on a
//!   class-0x10 prop), and the HUD's count (0x425950) raises [`SkillEvent::ConeHit`] when it moves.

use glam::{Mat4, Vec3};
use nfs_formats::career::{ConeRule, Mission};
use nfs_formats::scn::{ElementKind, Scene, SceneElement};
use nfs_physics::smackable::{collide_among, Smackable};
use nfs_physics::vehicle::Vehicle;

#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    pub element: usize,
    /// 0 checkpoint, 1 finish, 2 start.
    pub kind: u8,
    /// 1 line, 0 sphere.
    pub shape: u8,
    pub radius: f32,
    pub position: Vec3,
    pub p1: Vec3,
    pub p2: Vec3,
    pub sequence: i32,
    /// Element index of the arrow marker to show when this trigger fires.
    pub link: Option<usize>,
    pub chime: bool,
    /// Required heading when set (`+0xE8` flag, 0x4704E0's `dot(car.fwd, +0xC8) > 0.5`).
    pub heading: Option<Vec3>,
    /// `+0xC8` as stored, whatever the flag — what 0x46F0F0 hands the start placement 0x410430
    pub heading_c8: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cone {
    pub element: usize,
    pub position: Vec3,
    pub knocked: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkillEvent {
    Checkpoint { sequence: i32, chime: bool },
    Finish { time_ticks: u64 },
    /// The HUD's cone count moved (0x425950: the flagged smackables counted every frame against
    /// the last count `[0x60CA04]`): the sound 0x4AAE50, `HTime+0xE0 = 16` (the flash) and
    /// `HMissionSummary+0x140 = count`. `count` is the new count.
    ConeHit { count: usize },
    Dnf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillTest {
    pub checkpoints: Vec<Checkpoint>,
    pub cones: Vec<Cone>,
    /// Elements of every arrow marker (hidden unless linked from the last trigger).
    pub arrows: Vec<usize>,
    /// Next sequence id to take (`car+0xB58`), starts at 1.
    pub next_sequence: i32,
    pub finished: bool,
    pub dnf: bool,
    /// Race time in ticks (`car+0xB24 = tick − 448`).
    pub time_ticks: Option<u64>,
    pub shown_arrow: Option<usize>,
    pub skidpad: bool,
    /// The mission's smackables, built on the first [`Self::step_props`] from
    /// `TrackWorld::smackables` (0x4A17F0 at race init), and their scene elements.
    pub props: Vec<Smackable>,
    pub prop_elements: Vec<usize>,
    props_spawned: bool,
    /// `[0x60CA04]`, the HUD's last cone count.
    last_cone_count: usize,
}

const RACE_START_TICK: u64 = 448;

fn v3(t: &[f32], i: usize) -> Option<Vec3> {
    Some(Vec3::new(*t.get(i)?, *t.get(i + 1)?, *t.get(i + 2)?))
}

fn is_arrow(tag: u32) -> bool {
    let b = tag.to_le_bytes();
    &b[..3] == b"ARW"
}

impl SkillTest {
    pub fn from_scene(scene: &Scene, track_name: &str) -> Self {
        let mut checkpoints = Vec::new();
        let mut cones = Vec::new();
        let mut arrows = Vec::new();
        for (i, e) in scene.elements.iter().enumerate() {
            match e.kind {
                ElementKind::Trigger => {
                    if let Some(c) = Self::checkpoint(i, e) {
                        checkpoints.push(c);
                    }
                }
                ElementKind::Geom => {
                    let Some(tag) = Scene::param_u32(e, 0, 1) else { continue };
                    if tag == u32::from_le_bytes(*b"CONE") {
                        cones.push(Cone { element: i, position: Vec3::from(e.position), knocked: false });
                    } else if is_arrow(tag) {
                        arrows.push(i);
                    }
                }
                _ => {}
            }
        }
        checkpoints.sort_by_key(|c| c.sequence);
        Self {
            checkpoints,
            cones,
            arrows,
            next_sequence: 1,
            finished: false,
            dnf: false,
            time_ticks: None,
            shown_arrow: None,
            skidpad: track_name.eq_ignore_ascii_case("skidpad"),
            props: Vec::new(),
            prop_elements: Vec::new(),
            props_spawned: false,
            last_cone_count: 0,
        }
    }

    fn checkpoint(index: usize, e: &SceneElement) -> Option<Checkpoint> {
        let p = e.params.first()?;
        let kind = *p.first()? as u8;
        // Types 0, 1 and 2 only — checkpoint, finish line, start line. 0x46C8D0 builds
        // `[0x628170]` with just those three (the compares at 0x46C9AD…0x46C9D0), and
        // the two lookups the skill tests use, 0x46D290 (by type) and 0x46D2E0 (by
        // sequence id), read that vector. Type 6 is a trackside camera and type 4 an
        // audio trigger; neither is ever a checkpoint. — race-logic.md §3.2.
        if !matches!(kind, 0 | 1 | 2) {
            return None;
        }
        let shape = *p.get(1)? as u8;
        let radius = *p.get(2)?;
        let sequence = *p.get(7)? as i32;
        let link = p.get(12).copied().filter(|l| *l >= 0.0).map(|l| l as usize);
        let chime = p.get(14).map(|c| *c == 1.0).unwrap_or(false);
        let ends = e.params.get(1)?;
        let p1 = v3(ends, 0)?;
        let p2 = v3(ends, 3)?;
        // line 6: `+0xE4 flag, +0xBC velocity dir (3), +0xE8 flag, +0xC8 heading dir (3)`
        let heading_c8 = e.params.get(2).and_then(|l| v3(l, 5)).unwrap_or(Vec3::ZERO);
        let heading = e.params.get(2).and_then(|l| if l.get(4).copied().unwrap_or(0.0) != 0.0 { v3(l, 5) } else { None }).filter(|h| h.length_squared() > 0.5);
        Some(Checkpoint { element: index, kind, shape, radius, position: Vec3::from(e.position), p1, p2, sequence, link, chime, heading, heading_c8 })
    }

    /// The start trigger's pose for 0x410430: its position (vtable `+0x38`) and `+0xC8` as
    /// stored (0x46F0F0, no flag test, not normalised — the placement re-orthonormalises the
    /// frame, 0x4A4810). Every shipped start trigger carries a unit `+0xC8` with its `+0xE8` flag
    /// set. *(Until 2026-09-25 this fell back to the line's normal when the flag was clear — no
    /// original counterpart, and never taken with the shipped scenes.)*
    pub fn start_pose(&self) -> Option<(Vec3, Vec3)> {
        let s = self.checkpoints.iter().find(|c| c.kind == 2)?;
        Some((s.position, s.heading_c8))
    }

    /// Arrow to show at the start (link of the start trigger).
    pub fn initial_arrow(&self) -> Option<usize> {
        self.checkpoints.iter().find(|c| c.kind == 2).and_then(|c| c.link)
    }

    fn next(&self) -> Option<&Checkpoint> {
        self.checkpoints.iter().find(|c| c.sequence == self.next_sequence && c.kind != 2)
    }

    /// Does the trigger fire for a car whose side segments are `sides` (FL→RL, FR→RR) and whose
    /// centre moved `prev → pos` this tick?
    fn hit(c: &Checkpoint, pos: Vec3, prev: Vec3, sides: &[(Vec3, Vec3); 2]) -> bool {
        if c.shape == 0 {
            return (pos - c.position).length() < c.radius;
        }
        let crosses = |a: Vec3, b: Vec3| -> bool {
            if (a.y - c.position.y).abs() > 3.5 || (b.y - c.position.y).abs() > 3.5 {
                return false;
            }
            segments_intersect_xz(a, b, c.p1, c.p2)
        };
        crosses(sides[0].0, sides[0].1) || crosses(sides[1].0, sides[1].1) || crosses(prev, pos)
    }

    /// Per-tick update (even ticks). `about_to_reset` = the car is wrecked / upside down and will
    /// be auto-reset (`car+0xDB0 == 0 && car+0x1100 < 4`).
    pub fn tick(&mut self, tick: u64, pos: Vec3, prev: Vec3, sides: &[(Vec3, Vec3); 2], about_to_reset: bool) -> Vec<SkillEvent> {
        let mut ev = Vec::new();
        // the HUD's cone count (0x425950), whatever the car's state. APPROXIMATION: the HUD reads
        // it every frame and only for `FEGAME_TYPE == 9` with a cone rule on; here it is read on
        // the skill tick, always
        let count = self.cones_knocked();
        if count != self.last_cone_count {
            self.last_cone_count = count;
            ev.push(SkillEvent::ConeHit { count });
        }
        if self.finished || self.dnf {
            return ev;
        }
        if about_to_reset && !self.skidpad {
            self.dnf = true;
            self.time_ticks = Some(99_999);
            ev.push(SkillEvent::Dnf);
            return ev;
        }
        let Some(c) = self.next().cloned() else { return ev };
        if !Self::hit(&c, pos, prev, sides) {
            return ev;
        }
        match c.kind {
            1 => {
                self.finished = true;
                let t = tick.saturating_sub(RACE_START_TICK);
                self.time_ticks = Some(t);
                ev.push(SkillEvent::Finish { time_ticks: t });
            }
            _ => {
                self.next_sequence += 1;
                // a checkpoint without a link keeps the current arrow (skidpad_st1 seq 4; MEDIUM —
                // 0x482F70 with −1 was not traced)
                if c.link.is_some() {
                    self.shown_arrow = c.link;
                }
                ev.push(SkillEvent::Checkpoint { sequence: c.sequence, chime: c.chime });
            }
        }
        ev
    }

    /// One 32 Hz tick of the mission's smackables, in the order the priority list runs their
    /// handlers (`traffic.md` §14.2 / §21.5): per prop `0x4A1030` then `Car_ground` 0x49BD40 (both
    /// priority 0x1E), then per prop the collision pass 0x498FF0 (0x28) — against `cars`, the
    /// racers with their `car+0x354` in the original's units (kg / 128), which were all entered in
    /// this tick's list before any prop, then against the props before it that are awake — and
    /// last per prop 0x410280 (0x32). The caller runs this after the racers' own collision pass.
    ///
    /// `walls` is what 0x4980D0 runs for each body between its rounds and 0x498FF0 for a
    /// disturbed prop: `Car_wallCollide` 0x495020 and the props' pass 0x498DE0. Answers the pairs
    /// that touched as (index into `cars`, that car's own `car+0x430`).
    ///
    /// The racers' own pass stays the port's (`main.rs`, labelled there); the props' pairs follow
    /// all of it, which is the original's order. Not carried (`skill-tests.md` §5.4): the
    /// `Car_ground` LOD gate and the props' sound event. (The 8 m ceiling rule is in
    /// `Vehicle::car_ground_body`; `car+0x43C` is only ever written 0, so its skip never runs.)
    pub fn step_props(&mut self, world: &crate::world::TrackWorld, cars: &mut [(&mut Vehicle, f32)], walls: &mut dyn FnMut(&mut Vehicle)) -> Vec<(usize, f32)> {
        if !self.props_spawned {
            self.props_spawned = true;
            self.props = world
                .smackables
                .iter()
                .map(|s| Smackable::new(s.position, s.orientation, s.points.clone(), s.mass, s.audio_type, &world.collision))
                .collect();
            self.prop_elements = world.smackables.iter().map(|s| s.element).collect();
            tracing::info!(props = self.props.len(), "smackables spawned");
        }
        let alive: Vec<Vec3> = cars.iter().map(|(c, _)| c.position).collect();
        for p in self.props.iter_mut() {
            p.damp(&alive);
            p.ground(&world.collision);
        }
        let mut hits = Vec::new();
        for k in 0..self.props.len() {
            hits.extend(self.props[k].collide(cars, walls));
            collide_among(&mut self.props, k, walls);
        }
        for p in self.props.iter_mut() {
            p.post_impact();
        }
        // the cone count reads the flags (`car+0x42C`)
        for c in self.cones.iter_mut() {
            if let Some(k) = self.prop_elements.iter().position(|e| *e == c.element) {
                c.knocked = self.props[k].body.knocked_over;
            }
        }
        hits
    }

    /// The props' 64 Hz integrators (0x49B7A0 / 0x49B8E0), after the racers' — and every 32 ticks
    /// the re-orthonormalisation the port gives every body (0x4A4810).
    pub fn integrate_props(&mut self, tick: u64) {
        for p in self.props.iter_mut() {
            p.integrate();
            if tick % 32 == 0 {
                p.body.renormalise();
            }
        }
    }

    /// Each prop's pose, body to world, for `TrackWorld::place_smackable` — the record's pose
    /// pointers are the car's (0x483590).
    pub fn prop_poses(&self) -> impl Iterator<Item = (usize, Mat4)> + '_ {
        self.prop_elements.iter().zip(&self.props).map(|(e, p)| (*e, crate::world::body_pose(p.body.position, p.body.orientation)))
    }

    pub fn cones_knocked(&self) -> usize {
        self.cones.iter().filter(|c| c.knocked).count()
    }

    /// `HMissionSummary::evaluate` (§10). `position` = finishing position for race missions,
    /// `zones` = the player's ten body-damage zones. Returns (pass, effective time in ticks incl.
    /// penalty).
    pub fn evaluate(&self, mission: &Mission, position: usize, zones: &[f32; 10]) -> (bool, u64) {
        let cones = self.cones_knocked() as u64;
        // no finish trigger taken = DNF (`car+0xB24 = 99999`)
        let mut t = self.time_ticks.unwrap_or(99_999);
        let mut pass = true;
        if let Some(rank) = mission.min_rank {
            pass = position <= rank as usize;
        } else if let Some(limit) = mission.time_limit_s {
            if let Some(ConeRule::PenaltySeconds(s)) = mission.cone_rule {
                t += s as u64 * cones * 64;
            }
            if t >= limit as u64 * 64 {
                pass = false;
            }
        }
        if damage_over_limit(mission, zones) {
            pass = false;
        }
        if let Some(ConeRule::Limit(n)) = mission.cone_rule && cones > n as u64 {
            pass = false;
        }
        (pass, t)
    }
}

/// `0x434FA0` (§9) — the mission's damage test, CONFIRMED (read 2026-09-23): the ten zones are
/// summed into an integer **truncating after every add** (`fild acc / fadd zone / _ftol`,
/// 0x434FD6…0x43500C, the same arithmetic as `career::damage_sum`), the sum is divided by ten as a
/// C integer (the `0x66666667 / sar 2` pair at 0x435019) and the mission fails when that is
/// **greater** than `FACTORYDRIVERDAMAGELIMIT [0x657828]`. The float mean the port used to compare
/// failed a 5 % mission on a car whose zones held 5.99 each; this passes it, as the original does.
pub fn damage_over_limit(mission: &Mission, zones: &[f32; 10]) -> bool {
    match mission.damage_limit_pct {
        Some(limit) => (crate::career::damage_sum(zones) / 10) as i64 > limit as i64,
        None => false,
    }
}

/// 2-D segment intersection in the xz plane.
pub fn segments_intersect_xz(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> bool {
    let (ax, az, bx, bz, cx, cz, dx, dz) = (a.x, a.z, b.x, b.z, c.x, c.z, d.x, d.z);
    let orient = |px: f32, pz: f32, qx: f32, qz: f32, rx: f32, rz: f32| (qx - px) * (rz - pz) - (qz - pz) * (rx - px);
    let o1 = orient(ax, az, bx, bz, cx, cz);
    let o2 = orient(ax, az, bx, bz, dx, dz);
    let o3 = orient(cx, cz, dx, dz, ax, az);
    let o4 = orient(cx, cz, dx, dz, bx, bz);
    (o1 * o2 < 0.0) && (o3 * o4 < 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "GEOM_ELEMENT 4\r\n0 0 0\r\n1 0 0\t0 1 0\t0 0 1\r\n0 1162760003 0 0 -1 1 1 100\r\nGeometry Element\r\nTRIGGER_ELEMENT 5\r\n0 0 10\r\n1 0 0\t0 1 0\t0 0 1\r\n2 0 3.0 3.0 0.25 0.0 200.0 0 -1 -1 0 -1 3 0 0 0 0\r\n0 0 11.5\t0 0 8.5\r\n0 0 0 0\t2 0 0 -1\r\nStart\r\nTRIGGER_ELEMENT 5\r\n0 0 -10\r\n0 0 1\t0 1 0\t-1 0 0\r\n0 1 1.0 20.0 0.25 0.0 200.0 1 -1 -1 1 -1 3 0 1 0 0\r\n-10 0 -10\t10 0 -10\r\n0 0 0 0\t0 0 0 0\r\nWaypoint\r\nGEOM_ELEMENT 4\r\n0 0 -20\r\n1 0 0\t0 1 0\t0 0 1\r\n1 827806273 0 0 -1 0 1 41\r\nArrow 1\r\nTRIGGER_ELEMENT 5\r\n0 0 -30\r\n0 0 1\t0 1 0\t-1 0 0\r\n1 1 1.0 20.0 0.25 0.0 200.0 2 -1 -1 2 -1 -1 0 1 0 0\r\n-10 0 -30\t10 0 -30\r\n0 0 0 0\t0 0 0 0\r\nEnd\r\n";

    fn sides(pos: Vec3) -> [(Vec3, Vec3); 2] {
        [(pos + Vec3::new(-0.8, 0.0, 1.2), pos + Vec3::new(-0.8, 0.0, -1.2)), (pos + Vec3::new(0.8, 0.0, 1.2), pos + Vec3::new(0.8, 0.0, -1.2))]
    }

    #[test]
    fn checkpoints_in_order_then_finish() {
        let scene = Scene::parse(SCENE).unwrap();
        let mut st = SkillTest::from_scene(&scene, "canyon");
        assert_eq!(st.checkpoints.len(), 3);
        assert_eq!(st.start_pose().unwrap(), (Vec3::new(0.0, 0.0, 10.0), Vec3::new(0.0, 0.0, -1.0)));
        assert_eq!(st.initial_arrow(), Some(3));
        assert_eq!(st.cones.len(), 1);
        // crossing the finish line first does nothing (strict order)
        let e = st.tick(500, Vec3::new(0.0, 0.0, -31.0), Vec3::new(0.0, 0.0, -29.0), &sides(Vec3::new(0.0, 0.0, -31.0)), false);
        assert!(e.is_empty());
        let e = st.tick(600, Vec3::new(0.0, 0.0, -11.0), Vec3::new(0.0, 0.0, -9.0), &sides(Vec3::new(0.0, 0.0, -11.0)), false);
        assert_eq!(e, vec![SkillEvent::Checkpoint { sequence: 1, chime: true }]);
        assert_eq!(st.shown_arrow, Some(3));
        let e = st.tick(700, Vec3::new(0.0, 0.0, -31.0), Vec3::new(0.0, 0.0, -29.0), &sides(Vec3::new(0.0, 0.0, -31.0)), false);
        assert_eq!(e, vec![SkillEvent::Finish { time_ticks: 700 - 448 }]);
        assert!(st.finished);
        // the HUD's count: a flagged cone raises one event, and only when the count moves
        st.cones[0].knocked = true;
        let e = st.tick(800, Vec3::ZERO, Vec3::ZERO, &sides(Vec3::ZERO), false);
        assert_eq!(e, vec![SkillEvent::ConeHit { count: 1 }]);
        assert!(st.tick(802, Vec3::ZERO, Vec3::ZERO, &sides(Vec3::ZERO), false).is_empty());
        assert_eq!(st.cones_knocked(), 1);
    }

    #[test]
    fn evaluation_rules() {
        let scene = Scene::parse(SCENE).unwrap();
        let mut st = SkillTest::from_scene(&scene, "skidpad");
        st.time_ticks = Some(25 * 64);
        st.cones[0].knocked = true;
        let mut m = nfs_formats::career::parse_missions(&[0u8; 0xF0]).unwrap().remove(0);
        m.time_limit_s = Some(27);
        m.cone_rule = Some(ConeRule::PenaltySeconds(1));
        let clean = [0.0f32; 10];
        assert_eq!(st.evaluate(&m, 1, &clean), (true, 26 * 64));
        m.cone_rule = Some(ConeRule::PenaltySeconds(3));
        assert_eq!(st.evaluate(&m, 1, &clean).0, false, "25 + 3 s ≥ 27 s");
        m.cone_rule = None;
        m.damage_limit_pct = Some(5);
        assert!(!st.evaluate(&m, 1, &[7.0; 10]).0);
        // 0x434FA0 truncates after every add: 5.99 × 10 sums to 50, and 50 / 10 = 5 is not over 5
        assert!(st.evaluate(&m, 1, &[5.99; 10]).0);
        assert!(!st.evaluate(&m, 1, &[6.0; 10]).0);
        m.damage_limit_pct = None;
        m.min_rank = Some(1);
        assert!(st.evaluate(&m, 1, &clean).0 && !st.evaluate(&m, 2, &clean).0);
        // DNF makes any time limit fail
        m.min_rank = None;
        m.time_limit_s = Some(385);
        st.dnf = true;
        st.time_ticks = Some(99_999);
        assert!(!st.evaluate(&m, 1, &clean).0);
        // never reached the finish: also a fail
        st.dnf = false;
        st.time_ticks = None;
        assert!(!st.evaluate(&m, 1, &clean).0);
    }
}
