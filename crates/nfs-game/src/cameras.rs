//! Trackside cameras and the triggers that switch them — `RE/notes/race-logic.md` §3.3.
//!
//! A `<track>_cameras.scn` is pairs of `CAMERA_ELEMENT` and `TRIGGER_ELEMENT`. Every tick
//! `0x413B80` tests every trigger against every car; when a car enters one whose `+0xDC` camera
//! link differs from the one it already has, that becomes the car's camera (`car+0x10E4`), and for
//! the car the player is watching (`[0x606858]`) `0x413960(link)` switches the view:
//!
//! ```text
//! cam = 0x46D980(link)                    ; the camera element with that index
//! if !cam->+0x7C: keep the eye, reset the zoom (0x413A86)   ; "disabled"
//! else setView(0, cam->+0x78)             ; the camera's own view id (12 in the shipped files)
//!      if cam->+0x8C: camera position = cam->vt[0x38]()   ; 0x627EA4…
//! ```
//!
//! The port keeps the same table and the same crossing test, and the app uses the current camera's
//! position and field of view for its trackside view.

use glam::Vec3;
use nfs_formats::scn::{ElementKind, Scene, SceneElement};

/// A `CAMERA_ELEMENT`: line 5 is `+0x8C, +0x7C, +0x78, +0x90, +0xC0, +0x94, +0x98, +0x9C, +0xA0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackCamera {
    pub position: Vec3,
    /// `+0x78` — the view id the switch selects (12 in every shipped file)
    pub view: i32,
    /// `+0x7C` — 0 leaves the view alone
    pub enabled: bool,
    /// `+0x8C` — whether the element carries a position to move the camera to
    pub has_position: bool,
    /// The slot half-angle, in degrees, this camera leaves: 0x413960 sets the view (SetSlotView,
    /// which resets the half-angle to 37.5) and then, only when `+0x98` is set (0x469AF0 at
    /// 0x413A2A), copies `+0x9C` into the slot's `+0x38` / `+0x78` (0x413A3F), which the per-frame
    /// 0x462D4C… carries into `+0x58`. Without the flag `+0x9C` is ignored. It is a **horizontal**
    /// half-angle (`camera::fov_y`); this port used to take `+0x9C` as the vertical angle.
    pub half_angle: f32,
    /// `+0x84` (line 4's float): the switch 0x413960 stores it in the slot's `+0x30` (0x413B1B,
    /// over SetSlotView's 3.0), and the TV update 0x462E30 lifts the eye above where the switch put
    /// it by `clamp(0.25·lift·d − 0.25, 0, lift)`, `d` the car's distance from the eye
    /// ([`TrackCamera::eye_lift`]). 0 on 668 of the 718 shipped cameras.
    pub lift: f32,
    /// `+0x98`: whether `half_angle` came from `+0x9C`
    pub fov_flag: bool,
    /// `+0x88` (line 4's last integer) 0: the switch copies the element's matrix `+0x14` (vt+0x40,
    /// 0x46BEA0) into the slot and clears the aim bit, so the camera keeps that orientation — the
    /// forward and up rows. 60 of the shipped cameras. `None` when it tracks the car.
    pub fixed_view: Option<(Vec3, Vec3)>,
    /// `+0x90` set: the dolly law of 0x462B80 with `+0x94` as its near distance (the switch,
    /// 0x413AF1…0x413B06). 382 of the shipped cameras.
    pub dolly: Option<f32>,
}

/// The TV view's slot state — what the switch 0x413960 leaves in slot 0 and what 0x462E30 /
/// 0x462B80 make of it each update (`race-logic.md` §11.1, §11.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TvSlot {
    /// the eye as the last enabled switch placed it (`+0x24`, 0x413A00…0x413A16)
    pub eye: Option<Vec3>,
    /// the element's own matrix (forward, up) when its `+0x88` is 0 (0x413A5A…0x413A7F)
    pub fixed_view: Option<(Vec3, Vec3)>,
    /// a **disabled** camera with `+0x88` 0 clears the aim bit without giving a matrix
    /// (0x413AE8): the view keeps the direction it last had
    pub frozen: bool,
    /// `+0x30`, the eye's rise (0x462E30)
    pub lift: f32,
    /// `+0x58`, the half-angle the build takes (degrees)
    pub half_angle: f32,
    flag34: bool,
    a38: f32,
    a78: f32,
    t44: f32,
    step48: f32,
    dolly: Option<f32>,
}

impl Default for TvSlot {
    fn default() -> Self {
        let h = crate::camera::DEFAULT_HALF_ANGLE;
        // SetSlotView 0x465790: +0x34 0, +0x38 = +0x78 = +0x58 = 37.5, +0x44 1.0, +0x48 0.02,
        // +0x3C 0, +0x30 3.0 (0x465854…0x465896)
        Self { eye: None, fixed_view: None, frozen: false, lift: 3.0, half_angle: h, flag34: false, a38: h, a78: h, t44: 1.0, step48: 0.02, dolly: None }
    }
}

impl TvSlot {
    /// 0x462E30's rise (0x462EC6…0x462F19): `d = |car − eye|` (0x5323A0), the eye's y the height the
    /// switch left (`[0x6280A4 + 4·slot]`, re-captured when the eye's x changes) plus this.
    pub fn eye_lift(&self, d: f32) -> f32 {
        (self.lift * 0.25 * d - 0.25).min(self.lift).max(0.0)
    }

    /// The switch 0x413960 for a camera element. An enabled one runs SetSlotView(0, view) first,
    /// moves the eye, and takes `+0x98` / `+0x9C` into `+0x34` / `+0x38` / `+0x78`; a disabled one
    /// (0x413A86) only zeroes `+0x44` and takes `+0x9C` into the target `+0x78`. Both then take the
    /// dolly (`+0x3C` = `+0x90`, `+0x40` = `+0x94` when set) and the rise `+0x30` = `+0x84`.
    pub fn switch(&mut self, c: &TrackCamera) {
        let fov = c.fov_flag.then_some(c.half_angle);
        if c.enabled {
            *self = Self { eye: self.eye, ..Self::default() };
            if c.has_position {
                self.eye = Some(c.position);
            }
            self.fixed_view = c.fixed_view;
            if let Some(a) = fov {
                self.flag34 = true;
                self.a38 = a;
                self.a78 = a;
            }
        } else {
            self.t44 = 0.0;
            self.flag34 = fov.is_some();
            if let Some(a) = fov {
                self.a78 = a;
            }
            if c.fixed_view.is_some() {
                self.frozen = true;
            }
        }
        // `+0x3C` takes `+0x90` either way; `+0x40` only when it is set
        self.dolly = c.dolly;
        self.lift = c.lift;
    }

    /// 0x462B80(slot, d), `d` the car's distance from the eye. With the dolly: nothing while
    /// `d < +0x40` (0x462BD5); else `D' = d·A/37.5` (A = `+0x38`, `[0x5B3F70]`), `V` = `D'` moved
    /// toward `+0x40` by `t·|D' − +0x40|` and the half-angle `A·V/D'` — at `t` = 1 that is
    /// `37.5·+0x40/d`, a zoom that holds the car's size (0x462BE6…0x462C9F); `t` then grows by
    /// `+0x48·(1 − t)` within [0.01, 1] (0x462CE3…0x462D41). Without it, when `+0x34` is set: a
    /// non-zero `t` first copies `+0x38` into `+0x58` and zeroes `t`, then `+0x58` steps toward
    /// `+0x78` by `+0x48` (0x462D4C…0x462DC5); when clear, 37.5.
    pub fn update(&mut self, d: f32) {
        if let Some(d0) = self.dolly {
            if d < d0 {
                return;
            }
            let a = self.a38;
            let dp = d * a * 0.026_666_667;
            let e = (dp - d0).abs() * self.t44;
            // equal distances load the slot index's bits (`[esp+8]`, 0x462C93) — 0 for slot 0
            let v = if d0 < dp { dp - e } else if d0 > dp { dp + e } else { 0.0 };
            self.half_angle = v * a / dp;
            let t = self.t44 + self.step48 * (1.0 - self.t44);
            self.t44 = if t >= 1.0 { 1.0 } else if t <= 0.01 { 0.01 } else { t };
        } else if self.flag34 {
            if self.t44 != 0.0 {
                self.half_angle = self.a38;
                self.t44 = 0.0;
            }
            let cur = self.half_angle;
            self.a38 = if cur < self.a78 {
                cur + self.step48
            } else if cur > self.a78 {
                cur - self.step48
            } else {
                cur
            };
            self.half_angle = self.a38;
        } else {
            self.half_angle = crate::camera::DEFAULT_HALF_ANGLE;
        }
    }
}

/// A `TRIGGER_ELEMENT` of a cameras scene: the shape from `race-logic.md` §3.2 plus its camera link.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraTrigger {
    pub position: Vec3,
    /// 1 = line segment between `p1` and `p2`, 0 = sphere of `radius`
    pub shape: u8,
    pub radius: f32,
    pub p1: Vec3,
    pub p2: Vec3,
    /// `+0xDC`, the camera element this trigger switches to
    pub link: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TrackCameras {
    pub cameras: Vec<TrackCamera>,
    pub triggers: Vec<CameraTrigger>,
    /// the camera the watched car is on (`car+0x10E4`)
    current: Option<usize>,
    /// slot 0 as the switches leave it
    pub tv: TvSlot,
}

fn v3(p: &[f32], i: usize) -> Option<Vec3> {
    Some(Vec3::new(*p.get(i)?, *p.get(i + 1)?, *p.get(i + 2)?))
}

impl TrackCameras {
    pub fn from_scene(scene: &Scene) -> Self {
        let mut out = Self::default();
        for e in &scene.elements {
            match e.kind {
                ElementKind::Camera => out.cameras.push(Self::camera(e)),
                ElementKind::Trigger => {
                    if let Some(t) = Self::trigger(e) {
                        out.triggers.push(t);
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn camera(e: &SceneElement) -> TrackCamera {
        let p = e.params.get(1).cloned().unwrap_or_default();
        TrackCamera {
            position: Vec3::from(e.position),
            view: p.get(2).copied().unwrap_or(12.0) as i32,
            enabled: p.get(1).copied().unwrap_or(1.0) != 0.0,
            has_position: p.first().copied().unwrap_or(1.0) != 0.0,
            half_angle: match (p.get(6), p.get(7)) {
                (Some(&flag), Some(&a)) if flag != 0.0 => a,
                _ => crate::camera::DEFAULT_HALF_ANGLE,
            },
            lift: e.params.first().and_then(|l| l.get(1)).copied().unwrap_or(0.0),
            fixed_view: match (e.params.first().and_then(|l| l.get(2)), e.orientation) {
                (Some(&t), Some(m)) if t == 0.0 => Some((Vec3::from(m[2]), Vec3::from(m[1]))),
                _ => None,
            },
            fov_flag: p.get(6).is_some_and(|&f| f != 0.0),
            dolly: match (p.get(3), p.get(5)) {
                (Some(&f), Some(&d0)) if f != 0.0 => Some(d0),
                _ => None,
            },
        }
    }

    fn trigger(e: &SceneElement) -> Option<CameraTrigger> {
        let p = e.params.first()?;
        // Only **type 6**. The scene manager keeps five separate element vectors and
        // 0x46CB00 fills `[0x628174]` with the triggers whose type accessor 0x46AF50
        // (`return [this+0x7C]`) answers 6 (0x46CC56 `cmp eax, 6`); 0x413B80, the
        // per-frame trackside-camera sweep, reads exactly that vector through 0x46D880.
        // Checkpoints, the start and finish lines (types 0/1/2) go into `[0x628170]`
        // instead and can never reach the camera switch. All 1297 shipped type-6
        // triggers live in `<track>_cameras.scn` and they are the only ones carrying a
        // camera link. — race-logic.md §3.3.
        if p.first().copied().map(|t| t as i32) != Some(6) {
            return None;
        }
        let link = p.get(11).copied().filter(|l| *l >= 0.0)? as usize;
        let ends = e.params.get(1);
        Some(CameraTrigger {
            position: Vec3::from(e.position),
            shape: p.get(1).copied().unwrap_or(1.0) as u8,
            radius: p.get(2).copied().unwrap_or(1.0),
            p1: ends.and_then(|l| v3(l, 0)).unwrap_or(Vec3::from(e.position)),
            p2: ends.and_then(|l| v3(l, 3)).unwrap_or(Vec3::from(e.position)),
            link,
        })
    }

    /// The camera the watched car is on, if any has been reached.
    pub fn current(&self) -> Option<&TrackCamera> {
        self.current.and_then(|i| self.cameras.get(i))
    }

    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// One tick of 0x413B80 for the watched car: returns the camera it switched to, if it changed.
    ///
    /// The crossing test is `CTriggerElem::hit` (`race-logic.md` §3.2) with the **motion** segment
    /// of the car this tick. The original tests two of those, for the car's front corners
    /// (`car+0x7F8→+0xA44` and `+0x8BC→+0x980`); the port tests the centre's, which differs only
    /// for a trigger line shorter than the car is wide — and a camera trigger in the shipped scenes
    /// is 25–40 m long.
    pub fn update(&mut self, pos: Vec3, prev: Vec3) -> Option<usize> {
        for t in &self.triggers {
            let hit = if t.shape == 0 {
                (pos - t.position).length() < t.radius
            } else {
                (pos.y - t.position.y).abs() <= 3.5
                    && (prev.y - t.position.y).abs() <= 3.5
                    && crate::skill::segments_intersect_xz(prev, pos, t.p1, t.p2)
            };
            if !hit || self.current == Some(t.link) {
                continue;
            }
            // a disabled camera still becomes the car's camera and still reaches 0x413960, whose
            // 0x413A86 path keeps the eye but changes the zoom (`TvSlot::switch`)
            self.current = Some(t.link);
            if let Some(c) = self.cameras.get(t.link) {
                self.tv.switch(c);
                if c.enabled {
                    return Some(t.link);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tv_eye_lift_ramps_to_the_element_height() {
        let c = TvSlot { lift: 3.0, ..TvSlot::default() };
        assert_eq!(c.eye_lift(0.0), 0.0);
        assert_eq!(c.eye_lift(2.0), 1.25);
        assert_eq!(c.eye_lift(10.0), 3.0);
        assert_eq!(TvSlot { lift: 0.0, ..c }.eye_lift(50.0), 0.0);
    }

    #[test]
    fn tv_dolly_holds_the_car_size() {
        let c = TrackCamera { position: Vec3::ZERO, view: 12, enabled: true, has_position: true, half_angle: 37.5, lift: 0.0, fov_flag: false, fixed_view: None, dolly: Some(5.0) };
        let mut tv = TvSlot::default();
        tv.switch(&c);
        tv.update(3.0); // nearer than +0x40: unchanged
        assert_eq!(tv.half_angle, 37.5);
        tv.update(50.0);
        assert!((tv.half_angle - 3.75).abs() < 1e-4, "{}", tv.half_angle);
        // a disabled camera starts the blend again from t = 0: the angle stays A = 37.5 at first
        tv.switch(&TrackCamera { enabled: false, ..c });
        tv.update(50.0);
        assert!((tv.half_angle - 37.5).abs() < 1e-3, "{}", tv.half_angle);
    }

    const SAMPLE: &str = "CAMERA_ELEMENT 4\n\
        -183.945236 36.522900 325.160583\n\
        1.0 0.0 0.0\t0.0 1.0 0.0\t0.0 0.0 1.0\n\
        0 0.000000 1\n\
        1 1 12 0 2 3.000000 1 37.500000 0.600000\n\
        0.000000 0.000000 0.000000\n\
        c1\n\
        TRIGGER_ELEMENT 5\n\
        -273.218323 32.763489 397.518097\n\
        0.857167 0.0 -0.515038\t0.0 1.0 0.0\t0.515038 0.0 0.857167\n\
        6 1 1.000000 25.000000 0.250000 0.000000 200.000000 -1 -1 -1 -1 0 -1 0 0 0 0\n\
        -266.780334 32.763489 408.232697\t-279.656311 32.763489 386.803497\n\
        1 0.857163 0.0 -0.515045\t0 0.0 0.0 0.0\n\
        Replay Camera Trigger\n";

    #[test]
    fn reads_a_camera_and_its_trigger() {
        let s = Scene::parse(SAMPLE).unwrap();
        let c = TrackCameras::from_scene(&s);
        assert_eq!(c.cameras.len(), 1);
        assert_eq!(c.triggers.len(), 1);
        assert_eq!(c.cameras[0].view, 12, "the shipped cameras are all view 12");
        assert!(c.cameras[0].enabled);
        assert!((c.cameras[0].half_angle - 37.5).abs() < 1e-3);
        assert_eq!(c.triggers[0].link, 0);
        assert_eq!(c.triggers[0].shape, 1);
    }

    #[test]
    fn crossing_the_line_switches_the_camera() {
        let s = Scene::parse(SAMPLE).unwrap();
        let mut c = TrackCameras::from_scene(&s);
        let t = c.triggers[0];
        let mid = (t.p1 + t.p2) * 0.5;
        // the crossing direction is the trigger's row 0; step across the line through its middle
        let across = (t.p2 - t.p1).cross(Vec3::Y).normalize() * 3.0;
        let (prev, pos) = (mid - across, mid + across);
        assert_eq!(c.update(pos, prev), Some(0));
        assert_eq!(c.current().map(|c| c.view), Some(12));
        // it does not fire again for the camera it is already on
        assert_eq!(c.update(pos, prev), None);
        // and a car standing on the line does not re-fire it
        assert_eq!(c.update(mid, mid), None);
    }
}
