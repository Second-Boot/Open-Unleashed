//! Factory Driver skill tests (`RACE_TYPE 4`), from `RE/notes/skill-tests.md` (CONFIRMED unless noted).
//!
//! * The `_st<N>.scn` scene holds the start pose (type-2 trigger), checkpoints (type 0, taken
//!   strictly in sequence-id order), the finish (type 1), cones (`CONE` smackables) and arrow
//!   markers (`ARW*`, shown one at a time through the trigger link).
//! * Per tick (§7): the next checkpoint fires when one of the car's side segments crosses its
//!   line within ±3.5 m height (or the car centre is inside its sphere); the finish stores the
//!   race time; a wreck about to auto-reset counts as DNF except on the skidpad.
//! * Evaluation (§10): min rank ⇒ position; else time + cone penalty < limit; damage cap; cone cap.
//! * Cone hits: the original simulates cones as extra cars and flags one on an impact > 80; here a
//!   cone is knocked when the car footprint overlaps it (MEDIUM), and then tumbles as a simple
//!   body (`TrackWorld::knock`).

use glam::{Mat3, Vec3};
use nfs_formats::career::{ConeRule, Mission};
use nfs_formats::scn::{ElementKind, Scene, SceneElement};

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
    /// Required heading (start pose forward) when set.
    pub heading: Option<Vec3>,
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
    /// `away` = the direction from the car to the cone, `launch` = the velocity the cone is given
    /// (`TrackWorld::knock`)
    ConeHit { element: usize, away: Vec3, launch: Vec3 },
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
}

const RACE_START_TICK: u64 = 448;

fn v3(t: &[f32], i: usize) -> Option<Vec3> {
    Some(Vec3::new(*t.get(i)?, *t.get(i + 1)?, *t.get(i + 2)?))
}

fn is_arrow(tag: u32) -> bool {
    let b = tag.to_le_bytes();
    &b[..3] == b"ARW"
}

/// The prop radius `0x498280`'s caller reads from the prop's own `vtable[+0x14]`. A cone's value
/// has not been read; this is an **APPROXIMATION** of it.
const CONE_RADIUS: f32 = 0.25;

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
        Self { checkpoints, cones, arrows, next_sequence: 1, finished: false, dnf: false, time_ticks: None, shown_arrow: None, skidpad: track_name.eq_ignore_ascii_case("skidpad") }
    }

    fn checkpoint(index: usize, e: &SceneElement) -> Option<Checkpoint> {
        let p = e.params.first()?;
        let kind = *p.first()? as u8;
        let shape = *p.get(1)? as u8;
        let radius = *p.get(2)?;
        let sequence = *p.get(7)? as i32;
        let link = p.get(12).copied().filter(|l| *l >= 0.0).map(|l| l as usize);
        let chime = p.get(14).map(|c| *c == 1.0).unwrap_or(false);
        let ends = e.params.get(1)?;
        let p1 = v3(ends, 0)?;
        let p2 = v3(ends, 3)?;
        // line 6: `+0xE4 flag, +0xBC velocity dir (3), +0xE8 flag, +0xC8 heading dir (3)`
        let heading = e.params.get(2).and_then(|l| if l.get(4).copied().unwrap_or(0.0) != 0.0 { v3(l, 5) } else { None }).filter(|h| h.length_squared() > 0.5);
        Some(Checkpoint { element: index, kind, shape, radius, position: Vec3::from(e.position), p1, p2, sequence, link, chime, heading })
    }

    /// Start pose from the type-2 trigger: position and heading (`+0xC8`).
    pub fn start_pose(&self) -> Option<(Vec3, Vec3)> {
        let s = self.checkpoints.iter().find(|c| c.kind == 2)?;
        let fwd = s.heading.unwrap_or_else(|| (s.p2 - s.p1).cross(Vec3::Y).normalize_or_zero());
        Some((s.position, fwd.normalize_or_zero()))
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

    /// Knock every cone inside the car's footprint, which is the original's own test.
    ///
    /// `0x498280(car, point, radius)` — CONFIRMED: it builds the delta from the prop to the car
    /// with its **y component zeroed**, dots it against the car's own x axis (`car+0x364`) and
    /// forward axis (`car+0x37C`), and accepts when both `|dot|` are within `radius + the matching
    /// half extent` (`car+0x3A8` and `car+0x3B0`). Nothing else: no y test, no sphere test. Its
    /// caller walks the props the zone query `0x484630(pos, 3.0f, …)` returns — so **3 m**, and the
    /// radius is the prop's own, from its `vtable[+0x14]`.
    ///
    /// **APPROXIMATION** in what is left: that radius. A cone's own value has not been read, so
    /// 0.25 m stands in for it. The 3 m pre-filter below is the query's radius, and the horizontal
    /// delta is now the original's.
    pub fn knock_cones(&mut self, pos: Vec3, m: &Mat3, he: Vec3, car_velocity: Vec3) -> Vec<SkillEvent> {
        let mut ev = Vec::new();
        for c in self.cones.iter_mut().filter(|c| !c.knocked) {
            let d = c.position - pos;
            if d.length_squared() > 9.0 {
                continue;
            }
            // 0x498293 / 0x4982AB take only x and z, and 0x49829B leaves the third component 0
            let local = *m * Vec3::new(d.x, 0.0, d.z);
            if local.x.abs() < he.x + CONE_RADIUS && local.z.abs() < he.z + CONE_RADIUS {
                c.knocked = true;
                let away = Vec3::new(d.x, 0.0, d.z).normalize_or_zero();
                let away = if away.length_squared() > 0.0 { away } else { m.row(2) };
                // the original gives the smackable to the car physics (§5.3) — a 20 kg body struck
                // by a car leaves with more than the car's own speed; the port launches it along
                // the car's velocity, pushed out to the side it was hit on. APPROXIMATION.
                let v = car_velocity + away * car_velocity.length() * 0.5;
                let launch = if v.length() > 18.0 { v.normalize() * 18.0 } else { v } + Vec3::Y * 2.0;
                ev.push(SkillEvent::ConeHit { element: c.element, away, launch });
            }
        }
        ev
    }

    pub fn cones_knocked(&self) -> usize {
        self.cones.iter().filter(|c| c.knocked).count()
    }

    /// `HMissionSummary::evaluate` (§10). `position` = finishing position for race missions,
    /// `damage_pct` = average body damage. Returns (pass, effective time in ticks incl. penalty).
    pub fn evaluate(&self, mission: &Mission, position: usize, damage_pct: f32) -> (bool, u64) {
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
        if let Some(d) = mission.damage_limit_pct && damage_pct > d as f32 {
            pass = false;
        }
        if let Some(ConeRule::Limit(n)) = mission.cone_rule && cones > n as u64 {
            pass = false;
        }
        (pass, t)
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
        // cone at the origin is knocked when driven over
        let ev = st.knock_cones(Vec3::new(0.5, 0.0, 0.3), &Mat3::IDENTITY, Vec3::new(0.9, 0.6, 2.0), Vec3::new(0.0, 0.0, 20.0));
        let Some(SkillEvent::ConeHit { element: 0, launch, .. }) = ev.first() else { panic!("{ev:?}") };
        // it leaves along the car's velocity, capped, with a little lift
        assert!(launch.z > 10.0 && launch.z <= 18.0, "{launch:?}");
        assert!(launch.y > 0.0);
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
        assert_eq!(st.evaluate(&m, 1, 0.0), (true, 26 * 64));
        m.cone_rule = Some(ConeRule::PenaltySeconds(3));
        assert_eq!(st.evaluate(&m, 1, 0.0).0, false, "25 + 3 s ≥ 27 s");
        m.cone_rule = None;
        m.damage_limit_pct = Some(5);
        assert!(!st.evaluate(&m, 1, 7.0).0);
        m.damage_limit_pct = None;
        m.min_rank = Some(1);
        assert!(st.evaluate(&m, 1, 0.0).0 && !st.evaluate(&m, 2, 0.0).0);
        // DNF makes any time limit fail
        m.min_rank = None;
        m.time_limit_s = Some(385);
        st.dnf = true;
        st.time_ticks = Some(99_999);
        assert!(!st.evaluate(&m, 1, 0.0).0);
        // never reached the finish: also a fail
        st.dnf = false;
        st.time_ticks = None;
        assert!(!st.evaluate(&m, 1, 0.0).0);
    }
}
