//! Factory Driver skill tests (`RACE_TYPE 4`), from `RE/notes/skill-tests.md` (CONFIRMED unless noted).
//!
//! * The `_st<N>.scn` scene holds the start pose (type-2 trigger), checkpoints (type 0, taken
//!   strictly in sequence-id order), the finish (type 1), cones (`CONE` smackables) and arrow
//!   markers (`ARW*`, shown one at a time through the trigger link).
//! * Per tick (§7): the next checkpoint fires when one of the car's diagonals crosses its line
//!   (the first within ±3.5 m height) or the car centre is inside its sphere, and the car's
//!   heading, direction of travel and speed pass the trigger's gates (0x4704E0); the finish
//!   stores the race time; a wreck about to auto-reset counts as DNF except on the skidpad.
//! * Evaluation (§10, [`evaluate_mission`]): min rank ⇒ position and nothing else; else time +
//!   cone penalty ≤ limit, damage cap, cone cap.
//! * Cones (§5.3, §8): each is a **smackable**, a car record of its own (`car+0x52C & 0x80`)
//!   simulated by the car physics — `nfs_physics::smackable`, spawned from the scene's
//!   `kSmackable` elements (`TrackWorld::smackables`) and stepped in the handlers' order by
//!   `crate::smackables::RaceSmackables`. A cone counts as hit when the car–car solver flags it
//!   (`car+0x42C`, 0x496A10: any contact that is not separating, on a class-0x10 prop); the
//!   HUD's frame counts the flags ([`HudCones`], 0x425950).

use glam::Vec3;
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
    /// `+0xBC`, the required direction of travel: 0x4704E0 wants
    /// `dot(+0xBC, normalize(car+0x33C)) > 0.5` unless all three components are exactly 0 (0x470575…0x470662) — the `+0xE4` flag
    /// beside it is never read
    pub velocity_dir: Vec3,
    /// `+0xC8` as stored, whatever the `+0xE8` flag — what 0x46F0F0 hands the start placement
    /// 0x410430, and 0x4704E0's heading gate `dot(+0xC8, car+0x37C) > 0.5` unless it is exactly
    /// zero (0x4704FA…0x47056A; the flag is never read)
    pub heading_c8: Vec3,
    /// `+0x90` / `+0x94`, the speed window 0x4704E0 compares `car+0x35C` with
    pub speed_min: f32,
    pub speed_max: f32,
}

/// What `CTriggerElem::test` 0x4704E0 / `hit` 0x4706C0 read of the car.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerCar {
    /// `car+0x330`
    pub position: Vec3,
    /// The four wheels' world points `wheel+0x40` = `car+0x7F8 + 0xC4·i` — the ground contacts
    pub wheels: [Vec3; 4],
    /// `car+0x37C`, the body's forward row
    pub forward: Vec3,
    /// `car+0x33C`
    pub velocity: Vec3,
    /// `car+0x35C`, `max(|vx|, |vz|) + ¼·min(|vx|, |vz|)`
    pub speed: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cone {
    pub element: usize,
    pub position: Vec3,
}

/// The mission HUD's cone count, the tail of the HUD frame 0x425950 (0x425D7F…0x425DFD, read to
/// the instruction 2026-09-26). Every frame, while not in a replay (`[0x657424]` 0 or 1), in a
/// Factory Driver mission (`[0x6573E8] == 9`) with a cone rule (`CONEIMIT` or `CONEPENALTY` not
/// −1), it counts the flagged smackables — **all** of `0x5E8C24[]`, the track's own included —
/// and when the count differs from the last one `[0x60CA04]` it plays 0x4AAE50, stores the count,
/// and bumps **by one** `HMissionSummary+0x140` (0x429820 `inc`) and `HTime+0xE4` (0x431840)
/// besides the flash `HTime+0xE0 = 16` (0x431830). So those two count the frames the count
/// moved on, not the cones: two cones flagged between two frames cost one penalty.
/// 0x426B20…0x426B3B zeroes all three when the race (re)starts.
///
/// (history: `game-history.md` §19.1)
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HudCones {
    /// `[0x60CA04]`, the last count
    pub last: usize,
    /// `HMissionSummary+0x140` = `HTime+0xE4`: how many times the count moved
    pub changes: u64,
}

impl HudCones {
    /// One HUD frame with `count` flagged smackables; `true` when the count moved (the sound,
    /// the flash).
    pub fn frame(&mut self, count: usize) -> bool {
        if count == self.last {
            return false;
        }
        self.last = count;
        self.changes += 1;
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkillEvent {
    Checkpoint { sequence: i32, chime: bool },
    Finish { time_ticks: u64 },
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
                        cones.push(Cone { element: i, position: Vec3::from(e.position) });
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
        if !matches!(kind, 0..=2) {
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
        let velocity_dir = e.params.get(2).and_then(|l| v3(l, 1)).unwrap_or(Vec3::ZERO);
        let heading_c8 = e.params.get(2).and_then(|l| v3(l, 5)).unwrap_or(Vec3::ZERO);
        let speed_min = *p.get(5)?;
        let speed_max = *p.get(6)?;
        Some(Checkpoint { element: index, kind, shape, radius, position: Vec3::from(e.position), p1, p2, sequence, link, chime, velocity_dir, heading_c8, speed_min, speed_max })
    }

    /// The start trigger's pose for 0x410430: its position (vtable `+0x38`) and `+0xC8` as
    /// stored (0x46F0F0, no flag test, not normalised — the placement re-orthonormalises the
    /// frame, 0x4A4810). Every shipped start trigger carries a unit `+0xC8` with its `+0xE8` flag
    /// set. (history: `game-history.md` §19.2)
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

    /// `CTriggerElem::test(car)` 0x4704E0: `hit` 0x4706C0, then the direction of travel, the
    /// heading and the speed window.
    ///
    /// `hit`, read to the instruction 2026-09-26: a sphere (`+0x80 == 0`) fires while
    /// `|car+0x330 − position| < +0x84` (0x470928…0x4709A6); a line (`+0x80 == 1`) takes the car's
    /// **diagonals** — wheel 0 → wheel 3 (`car+0x7F8`, `+0xA44`) and wheel 1 → wheel 2 (`+0x8BC`,
    /// `+0x980`) — and fires when the first crosses the line in x–z (0x489CC0) or, failing that,
    /// the second does; the first is refused outright unless both its ends are within 3.5 m in
    /// height of endpoint `+0xA4`, inclusive (0x4707B2…0x470810), the second has no height test.
    /// Any other shape never fires. Not carried: the `carinfo+0x518` (`.TINYCAR`) re-basing of the
    /// four points (0x470757…0x4707AF), which reads an unwritten local.
    /// (history: `game-history.md` §19.3)
    fn hit(c: &Checkpoint, car: &TriggerCar) -> bool {
        let fired = match c.shape {
            0 => (car.position - c.position).length() < c.radius,
            1 => {
                let near = |y: f32| y + 3.5 >= c.p1.y && y - 3.5 <= c.p1.y;
                let (w0, w3) = (car.wheels[0], car.wheels[3]);
                if !near(w0.y) || !near(w3.y) {
                    return false;
                }
                segments_intersect_xz(w0, w3, c.p1, c.p2) || segments_intersect_xz(car.wheels[1], car.wheels[2], c.p1, c.p2)
            }
            _ => false,
        };
        if !fired {
            return false;
        }
        // 0x4704FA…0x47056A: the heading, unless `+0xC8` is exactly zero
        if c.heading_c8 != Vec3::ZERO && c.heading_c8.dot(car.forward) <= 0.5 {
            return false;
        }
        // 0x470575…0x470662: the direction of travel, `normalize(car+0x33C)` (0x5328D0) or
        // `(0, 1, 0)` when the velocity is exactly zero, unless `+0xBC` is exactly zero
        if c.velocity_dir != Vec3::ZERO {
            let dir = if car.velocity == Vec3::ZERO { Vec3::Y } else { car.velocity.normalize() };
            if c.velocity_dir.dot(dir) <= 0.5 {
                return false;
            }
        }
        // 0x47066B…0x47069D: `car+0x35C + 0.1 > +0x90` and `+0x94 + 0.1 > car+0x35C`
        car.speed + 0.1 > c.speed_min && c.speed_max + 0.1 > car.speed
    }

    /// Per-tick update (even ticks). `about_to_reset` = the car is wrecked / upside down and will
    /// be auto-reset (`car+0xDB0 == 0 && car+0x1100 < 4`).
    pub fn tick(&mut self, tick: u64, car: &TriggerCar, about_to_reset: bool) -> Vec<SkillEvent> {
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
        if !Self::hit(&c, car) {
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

    /// `HMissionSummary::evaluate` (§10) for a skill test: [`evaluate_mission`] with the race time
    /// `car+0xCD8 = car+0xB24` (99 999 after a DNF or with no finish taken). Returns (pass, the
    /// time the summary compared, penalty included).
    /// `cones` is the summary's `+0x140` ([`HudCones::changes`]).
    pub fn evaluate(&self, mission: &Mission, position: usize, cones: u64, zones: &[f32; 10]) -> (bool, i64) {
        evaluate_mission(mission, position, self.time_ticks.unwrap_or(99_999) as i64, cones, zones)
    }
}

/// `HMissionSummary::evaluate` 0x4293F0, read to the instruction 2026-09-26. `position` is the
/// player's `car+0xCD4`, `time` its `car+0xCD8` in ticks, `cones` the summary's `+0x140`,
/// `zones` the ten damage zones. Returns (pass, the time compared with the limit). `time` is the
/// signed dword `+0xCD8` (0x4294DE `mov eax,[car+0xCD8]`, compared as `fild` floats at 0x42951A: a
/// negative projected time never fails the limit).
///
/// ```text
/// pass = 1
/// if MINRANK:              pass = car+0xCD4 <= MINRANK, and nothing else    (0x4294A2…0x4294C4)
/// else:
///   if TIMELIMIT != −1 and car+0xCD8 != 0:                                   (0x4294C9…0x4294E6)
///       t = car+0xCD8, + _ftol(CONEPENALTY·cones·64.0) when CONEPENALTY != −1
///       fail when TIMELIMIT·64 < t                                           (0x42952A)
///   fail when 0x434FA0(0)                                                    (0x42953F)
///   fail when CONEIMIT != −1 and cones > CONEIMIT                            (0x429552)
/// ```
///
/// A minimum rank therefore skips the damage and cone-limit tests: `jle 0x429569` (0x4294BE)
/// lands past both, and the failing `jmp 0x429564` only clears the flag. (history: `game-history.md` §19.4)
pub fn evaluate_mission(mission: &Mission, position: usize, time: i64, cones: u64, zones: &[f32; 10]) -> (bool, i64) {
    if let Some(rank) = mission.min_rank {
        return (position <= rank as usize, time);
    }
    let mut pass = true;
    let mut t = time;
    if let Some(limit) = mission.time_limit_s
        && t != 0
    {
        if let Some(ConeRule::PenaltySeconds(s)) = mission.cone_rule {
            t += s as i64 * cones as i64 * 64;
        }
        // `fild t; fild limit; fmul 64; fcompp; test ah, 1` (0x42951A…0x429531): fails only
        // when `limit·64 < t` — a time exactly on the limit passes
        if t > limit as i64 * 64 {
            pass = false;
        }
    }
    if damage_over_limit(mission, zones) {
        pass = false;
    }
    if let Some(ConeRule::Limit(n)) = mission.cone_rule
        && cones > n as u64
    {
        pass = false;
    }
    (pass, t)
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

/// 0x489CC0 with no output point: the segment `a → b` crosses `c → d` in x–z. Both parameters
/// must lie in [0, 1], ends included, and parallel segments never cross (`den == 0`,
/// 0x489CFC…0x489D0B). `t` is stored as a float before its compares (0x489D3D), `u` is compared
/// straight off the FPU stack.
pub fn segments_intersect_xz(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> bool {
    let (sx, sz) = (b.x - a.x, b.z - a.z);
    let (lx, lz) = (d.x - c.x, d.z - c.z);
    let den = sx as f64 * lz as f64 - lx as f64 * sz as f64;
    if den == 0.0 {
        return false;
    }
    let den32 = den as f32 as f64;
    let (dz, dx) = ((a.z - c.z) as f64, (a.x - c.x) as f64);
    let t = ((dz * lx as f64 - dx * lz as f64) / den32) as f32;
    if !(0.0..=1.0).contains(&t) {
        return false;
    }
    let u = (dz * sx as f64 - dx * sz as f64) / den32;
    (0.0..=1.0).contains(&u)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "GEOM_ELEMENT 4\r\n0 0 0\r\n1 0 0\t0 1 0\t0 0 1\r\n0 1162760003 0 0 -1 1 1 100\r\nGeometry Element\r\nTRIGGER_ELEMENT 5\r\n0 0 10\r\n1 0 0\t0 1 0\t0 0 1\r\n2 0 3.0 3.0 0.25 0.0 200.0 0 -1 -1 0 -1 3 0 0 0 0\r\n0 0 11.5\t0 0 8.5\r\n0 0 0 0\t2 0 0 -1\r\nStart\r\nTRIGGER_ELEMENT 5\r\n0 0 -10\r\n0 0 1\t0 1 0\t-1 0 0\r\n0 1 1.0 20.0 0.25 0.0 200.0 1 -1 -1 1 -1 3 0 1 0 0\r\n-10 0 -10\t10 0 -10\r\n0 0 0 0\t0 0 0 0\r\nWaypoint\r\nGEOM_ELEMENT 4\r\n0 0 -20\r\n1 0 0\t0 1 0\t0 0 1\r\n1 827806273 0 0 -1 0 1 41\r\nArrow 1\r\nTRIGGER_ELEMENT 5\r\n0 0 -30\r\n0 0 1\t0 1 0\t-1 0 0\r\n1 1 1.0 20.0 0.25 0.0 200.0 2 -1 -1 2 -1 -1 0 1 0 0\r\n-10 0 -30\t10 0 -30\r\n0 0 0 0\t0 0 0 0\r\nEnd\r\n";

    /// A car at `pos` heading −z at 10 m/s, wheels FL, FR, RL, RR.
    fn car(pos: Vec3) -> TriggerCar {
        let w = |x: f32, z: f32| pos + Vec3::new(x, 0.0, z);
        TriggerCar { position: pos, wheels: [w(-0.8, -1.2), w(0.8, -1.2), w(-0.8, 1.2), w(0.8, 1.2)], forward: Vec3::NEG_Z, velocity: Vec3::new(0.0, 0.0, -10.0), speed: 10.0 }
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
        let e = st.tick(500, &car(Vec3::new(0.0, 0.0, -31.0)), false);
        assert!(e.is_empty());
        let e = st.tick(600, &car(Vec3::new(0.0, 0.0, -11.0)), false);
        assert_eq!(e, vec![SkillEvent::Checkpoint { sequence: 1, chime: true }]);
        assert_eq!(st.shown_arrow, Some(3));
        let e = st.tick(700, &car(Vec3::new(0.0, 0.0, -31.0)), false);
        assert_eq!(e, vec![SkillEvent::Finish { time_ticks: 700 - 448 }]);
        assert!(st.finished);
    }

    #[test]
    fn the_hud_counts_the_moves_not_the_cones() {
        let mut h = HudCones::default();
        assert!(!h.frame(0));
        // two cones flagged between two frames: one move, one penalty (0x429820 `inc`)
        assert!(h.frame(2));
        assert!(!h.frame(2));
        assert!(h.frame(3));
        assert_eq!(h, HudCones { last: 3, changes: 2 });
    }

    #[test]
    fn the_trigger_gates_heading_travel_and_height() {
        // the waypoint with `+0xC8` = +x: a car heading −z crosses it without firing (the 360-spin
        // checkpoints of skidpad_st13 and the rest), one turned to +x fires it
        let scene = Scene::parse(&SCENE.replacen("0 0 0 0\t0 0 0 0\r\nWaypoint", "0 0 0 0\t2 1 0 0\r\nWaypoint", 1)).unwrap();
        let mut st = SkillTest::from_scene(&scene, "skidpad");
        let at = Vec3::new(0.0, 0.0, -10.0);
        assert!(st.tick(600, &car(at), false).is_empty());
        let turned = TriggerCar { forward: Vec3::X, ..car(at) };
        assert_eq!(st.tick(602, &turned, false), vec![SkillEvent::Checkpoint { sequence: 1, chime: true }]);
        // `+0xBC` = −z: travelling +z does not fire the finish, travelling −z does
        let scene = Scene::parse(&SCENE.replacen("0 0 0 0\t0 0 0 0\r\nEnd", "2 0 0 -1\t0 0 0 0\r\nEnd", 1)).unwrap();
        let mut st = SkillTest::from_scene(&scene, "skidpad");
        st.next_sequence = 2;
        let at = Vec3::new(0.0, 0.0, -30.0);
        assert!(st.tick(700, &TriggerCar { velocity: Vec3::new(0.0, 0.0, 10.0), ..car(at) }, false).is_empty());
        assert!(st.tick(702, &car(at), false).contains(&SkillEvent::Finish { time_ticks: 702 - 448 }));
        // the first diagonal's ends must be within 3.5 m of the line's height, inclusive
        let mut st = SkillTest::from_scene(&Scene::parse(SCENE).unwrap(), "skidpad");
        assert!(st.tick(600, &car(Vec3::new(0.0, 3.6, -10.0)), false).is_empty());
        assert_eq!(st.tick(602, &car(Vec3::new(0.0, 3.5, -10.0)), false).len(), 1);
        // 0x489CC0 takes the ends: a diagonal ending exactly on the line crosses it
        assert!(segments_intersect_xz(Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)));
        assert!(!segments_intersect_xz(Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 0.0, -0.01), Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0)));
    }

    #[test]
    fn evaluation_rules() {
        let scene = Scene::parse(SCENE).unwrap();
        let mut st = SkillTest::from_scene(&scene, "skidpad");
        st.time_ticks = Some(25 * 64);
                let mut m = nfs_formats::career::parse_missions(&[0u8; 0xF0]).unwrap().remove(0);
        m.time_limit_s = Some(27);
        m.cone_rule = Some(ConeRule::PenaltySeconds(1));
        let clean = [0.0f32; 10];
        assert_eq!(st.evaluate(&m, 1, 1, &clean), (true, 26 * 64));
        m.cone_rule = Some(ConeRule::PenaltySeconds(3));
        assert!(!st.evaluate(&m, 1, 1, &clean).0, "25 + 3 s > 27 s");
        // 0x42952E: a time exactly on the limit passes
        m.cone_rule = Some(ConeRule::PenaltySeconds(2));
        assert_eq!(st.evaluate(&m, 1, 1, &clean), (true, 27 * 64));
        m.cone_rule = None;
        m.damage_limit_pct = Some(5);
        assert!(!st.evaluate(&m, 1, 1, &[7.0; 10]).0);
        // 0x434FA0 truncates after every add: 5.99 × 10 sums to 50, and 50 / 10 = 5 is not over 5
        assert!(st.evaluate(&m, 1, 1, &[5.99; 10]).0);
        assert!(!st.evaluate(&m, 1, 1, &[6.0; 10]).0);
        m.damage_limit_pct = None;
        m.min_rank = Some(1);
        assert!(st.evaluate(&m, 1, 1, &clean).0 && !st.evaluate(&m, 2, 1, &clean).0);
        // 0x4294BE: a rank rule skips the damage and cone-limit tests
        m.damage_limit_pct = Some(5);
        m.cone_rule = Some(ConeRule::Limit(0));
        assert!(st.evaluate(&m, 1, 1, &[50.0; 10]).0);
        m.damage_limit_pct = None;
        m.cone_rule = None;
        // DNF makes any time limit fail
        m.min_rank = None;
        m.time_limit_s = Some(385);
        st.dnf = true;
        st.time_ticks = Some(99_999);
        assert!(!st.evaluate(&m, 1, 1, &clean).0);
        // never reached the finish: also a fail
        st.dnf = false;
        st.time_ticks = None;
        assert!(!st.evaluate(&m, 1, 1, &clean).0);
    }
}
