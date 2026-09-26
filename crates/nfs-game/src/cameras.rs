//! Trackside cameras and the triggers that switch them — `RE/notes/race-logic.md` §3.3.
//!
//! A `<track>_cameras.scn` is pairs of `CAMERA_ELEMENT` and `TRIGGER_ELEMENT`. The sweep
//! `0x413B80(flag)` runs `CTriggerElem::test` 0x4704E0 for every type-6 trigger against every car;
//! when a car passes one whose `+0xDC` camera link differs from the one it already has, that
//! becomes the car's camera (`car+0x10E4`), and — only with `flag` set — for the car the player is
//! watching (`[0x606858]`) `0x413960(link)` switches the view:
//!
//! ```text
//! cam = 0x46D980(link)                    ; the camera element at that element index
//! if !cam->+0x7C: keep the eye, reset the zoom (0x413A86)   ; "disabled"
//! else setView(0, cam->+0x78)             ; the camera's own view id (12 in the shipped files)
//!      if cam->+0x8C: camera position = cam->vt[0x38]()   ; 0x627EA4…
//! ```
//!
//! The sweep has three callers, all with `flag = ([0x606850] == 0x14)`, the TV camera chosen:
//! the camera dispatcher 0x463C30 while a replay plays (`[0x5E99F4] >= 2`, 0x463E14…0x463E28) or,
//! in a linked network session, when its `+0xBD` is set (0x463E36…0x463E60), and the replay's
//! input playback (0x414620…0x41463D). A live single-player race never runs it.

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
    /// `+0x80` (line 4's first integer): with view 11 the switch raises the eye by `+0x84` at once
    /// (0x413B23…0x413B3A)
    pub raise: bool,
    /// `+0xA0` (line 5's last float, 0.6 in the sample): the switch stores it in the slot's `+0xCC`
    /// (0x413B42…0x413B47) — SplineCam's pace
    pub pace: f32,
    /// `+0xC0` (line 5's fifth integer, accessor 0x469B50): the replay's opening camera — 1 on the
    /// one camera a forward race opens on, 2 on the one a backwards race opens on
    /// (0x46D890); 0 on the other 688 shipped cameras, and every track has one of each but monaco2
    /// (no 1) and skidpad (no 2)
    pub opening: i32,
    /// `+0xB4` (line 6), copied by vt 0x469B20 into the slot's `+0x60…+0x68` (0x413B4D…0x413B6D) —
    /// SplineCam's offset along the slice's lateral / up / forward axes
    pub spline_offset: Vec3,
}

/// The TV view's slot state — what the switch 0x413960 leaves in slot 0 and what 0x462E30 /
/// 0x462B80 make of it each update (`race-logic.md` §11.1, §11.5).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TvSlot {
    /// the eye as the last enabled switch placed it (`+0x24`, 0x413A00…0x413A16)
    pub eye: Option<Vec3>,
    /// the view the last enabled switch set: 0x413960 runs SetSlotView(0, `+0x78`) (0x4139D3…
    /// 0x4139DD), so a camera element whose id is not 12 puts the TV slot into that view — 11
    /// SplineCam on 52 shipped cameras, 8 on 24, 10 CircleCam on 13, 6 / 9 / 3 on four
    pub view: Option<u16>,
    /// the element's own matrix (forward, up) when its `+0x88` is 0 (0x413A5A…0x413A7F)
    pub fixed_view: Option<(Vec3, Vec3)>,
    /// a **disabled** camera with `+0x88` 0 clears the aim bit without giving a matrix
    /// (0x413AE8): the view keeps the direction it last had
    pub frozen: bool,
    /// `+0x30`, the eye's rise (0x462E30)
    pub lift: f32,
    /// `+0x58`, the half-angle the build takes (degrees)
    pub half_angle: f32,
    /// `+0x5C`, the near plane the build gives the main view (`camera::near_plane`), written by
    /// 0x462B80 beside the half-angle (`camera::zoom_near`)
    pub near: f32,
    /// SplineCam's eye (`+0x24`), its tracker slice (`+0xDC`), pace `+0xCC` and offset `+0x60`
    pub spline_eye: Option<Vec3>,
    pub spline_slice: Option<usize>,
    pub pace: f32,
    pub spline_offset: Vec3,
    /// how many enabled switches have run SetSlotView(0, `+0x78`) — each one resets the slot's
    /// per-view words, such as CircleCam's count `+0xC6` (0x4658B2), even when the view stays
    pub switches: u32,
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
        // +0x3C 0, +0x30 3.0, +0x5C 0.5 (0x465854…0x465896)
        Self {
            eye: None,
            view: None,
            fixed_view: None,
            frozen: false,
            lift: 3.0,
            half_angle: h,
            near: crate::camera::DEFAULT_NEAR,
            spline_eye: None,
            spline_slice: None,
            pace: 0.0,
            spline_offset: Vec3::ZERO,
            switches: 0,
            flag34: false,
            a38: h,
            a78: h,
            t44: 1.0,
            step48: 0.02,
            dolly: None,
        }
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
            *self = Self { eye: self.eye, pace: self.pace, switches: self.switches.wrapping_add(1), ..Self::default() };
            self.view = u16::try_from(c.view).ok();
            if c.has_position {
                self.eye = Some(c.position);
                // SplineCam starts from the element's place, raised by `+0x84` when `+0x80` is set
                let raise = if c.view == 11 && c.raise { c.lift } else { 0.0 };
                self.spline_eye = Some(c.position + Vec3::Y * raise);
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
        self.pace = c.pace;
        self.spline_offset = c.spline_offset;
    }

    /// 0x462B80(slot, d), `d` the car's distance from the eye. With the dolly: nothing while
    /// `d < +0x40` (0x462BD5); else `D' = d·A/37.5` (A = `+0x38`, `[0x5B3F70]`), `V` = `D'` moved
    /// toward `+0x40` by `t·|D' − +0x40|` and the half-angle `A·V/D'` — at `t` = 1 that is
    /// `37.5·+0x40/d`, a zoom that holds the car's size (0x462BE6…0x462C9F); `t` then grows by
    /// `+0x48·(1 − t)` within [0.01, 1] (0x462CE3…0x462D41). Without it, when `+0x34` is set: a
    /// non-zero `t` first copies `+0x38` into `+0x58` and zeroes `t`, then `+0x58` steps toward
    /// `+0x78` by `+0x48` (0x462D4C…0x462DC5); when clear, 37.5. Each branch that writes `+0x58`
    /// also writes the near plane `+0x5C` from the same angle (`camera::zoom_near`, 0x462CA5…
    /// 0x462CE3 and 0x462DC7…0x462E19); the early return leaves both.
    ///
    /// Re-read 2026-09-25: the stepping branch stores `+0x58` into `+0x38` first (`fst`, 0x462D89)
    /// and then adds or takes `+0x48` by its compare with `+0x78` (equal leaves it), with no clamp —
    /// so the half-angle hunts ±0.02° about a target it rarely lands on exactly.
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
            self.near = crate::camera::zoom_near(self.half_angle);
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
            self.near = crate::camera::zoom_near(self.half_angle);
        } else {
            self.half_angle = crate::camera::DEFAULT_HALF_ANGLE;
            self.near = crate::camera::DEFAULT_NEAR;
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
    /// `+0xBC`, the direction of travel 0x4704E0 wants (zero = any)
    pub velocity_dir: Vec3,
    /// `+0xC8`, the heading 0x4704E0 wants (zero = any)
    pub heading: Vec3,
    /// `+0x90` / `+0x94`, the speed window
    pub speed_min: f32,
    pub speed_max: f32,
    /// `+0xDC`, the **element index** (in the scene file, re-based by the parser 0x46C417…
    /// 0x46C46A onto the combined list `[0x628168]`) of the camera this trigger switches to
    pub link: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TrackCameras {
    pub cameras: Vec<TrackCamera>,
    pub triggers: Vec<CameraTrigger>,
    /// element index → index into `cameras`: 0x46D980(link) is `[0x628168][link]` through
    /// `__RTDynamicCast` to `CCameraElem` (0x46D987…0x46D99E), so a link to any other class
    /// answers null
    by_element: Vec<Option<usize>>,
    /// the watched car's `car+0x10E4`, the element index of the last link it crossed
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
            out.by_element.push(matches!(e.kind, ElementKind::Camera).then_some(out.cameras.len()));
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
            raise: e.params.first().and_then(|l| l.first()).is_some_and(|&f| f != 0.0),
            pace: p.get(8).copied().unwrap_or(0.0),
            opening: p.get(4).copied().unwrap_or(0.0) as i32,
            spline_offset: e.params.get(2).and_then(|l| Some(Vec3::new(*l.first()?, *l.get(1)?, *l.get(2)?))).unwrap_or(Vec3::ZERO),
            dolly: match (p.get(3), p.get(5)) {
                (Some(&f), Some(&d0)) if f != 0.0 => Some(d0),
                _ => None,
            },
        }
    }

    fn trigger(e: &SceneElement) -> Option<CameraTrigger> {
        // Only **type 6**. The scene manager keeps five separate element vectors and
        // 0x46CB00 fills `[0x628174]` with the triggers whose type accessor 0x46AF50
        // (`return [this+0x7C]`) answers 6 (0x46CC56 `cmp eax, 6`); 0x413B80, the
        // per-frame trackside-camera sweep, reads exactly that vector through 0x46D880.
        // Checkpoints, the start and finish lines (types 0/1/2) go into `[0x628170]`
        // instead and can never reach the camera switch. All 1297 shipped type-6
        // triggers live in `<track>_cameras.scn` and they are the only ones carrying a
        // camera link. — race-logic.md §3.3.
        CameraTrigger::from_element(e, 6, 11)
    }
}

impl CameraTrigger {
    /// A `TRIGGER_ELEMENT` of type `kind` with its shape and gates, `link` taken from line 4's
    /// token `link_token` (11 = `+0xDC`, the camera; 9 = `+0xD8`, the element a type-4 audio
    /// trigger flags — `race-logic.md` §3.2) as a **file-local** element index; `None` for another
    /// type or a negative link.
    pub fn from_element(e: &SceneElement, kind: i32, link_token: usize) -> Option<CameraTrigger> {
        let p = e.params.first()?;
        if p.first().copied().map(|t| t as i32) != Some(kind) {
            return None;
        }
        let link = p.get(link_token).copied().filter(|l| *l >= 0.0)? as usize;
        let ends = e.params.get(1);
        // line 6: `+0xE4 flag, +0xBC (3), +0xE8 flag, +0xC8 (3)` — the flags are never read
        let dirs = e.params.get(2);
        Some(CameraTrigger {
            position: Vec3::from(e.position),
            shape: p.get(1).copied().unwrap_or(1.0) as u8,
            radius: p.get(2).copied().unwrap_or(1.0),
            p1: ends.and_then(|l| v3(l, 0)).unwrap_or(Vec3::from(e.position)),
            p2: ends.and_then(|l| v3(l, 3)).unwrap_or(Vec3::from(e.position)),
            velocity_dir: dirs.and_then(|l| v3(l, 1)).unwrap_or(Vec3::ZERO),
            heading: dirs.and_then(|l| v3(l, 5)).unwrap_or(Vec3::ZERO),
            speed_min: p.get(5).copied().unwrap_or(0.0),
            speed_max: p.get(6).copied().unwrap_or(200.0),
            link,
        })
    }
}

impl TrackCameras {
    /// 0x46D980(link): the camera element at that element index, `None` for any other class.
    pub fn camera_at(&self, element: usize) -> Option<&TrackCamera> {
        self.by_element.get(element).copied().flatten().and_then(|i| self.cameras.get(i))
    }

    /// 0x46D890: the element index of the first camera (in element order) whose `+0xC0` is 1 in a
    /// forward race or 2 in a backwards one (`REVERSE [0x657430]`, 0x46D8C7…0x46D8EF); failing
    /// that the first camera element at all (0x46D900…0x46D947); failing that 0.
    pub fn opening(&self, reverse: bool) -> usize {
        let want = if reverse { 2 } else { 1 };
        let cams = || self.by_element.iter().enumerate().filter_map(|(e, c)| Some((e, (*c)?)));
        cams()
            .find(|&(_, c)| self.cameras[c].opening == want)
            .or_else(|| cams().next())
            .map_or(0, |(e, _)| e)
    }

    /// The camera part of `Replay_startRace` 0x413EA0 when a replay starts to play and the track
    /// has camera elements (0x46D960): every car's `car+0x10E4 = 0x46D890()` and 0x413960 on it
    /// (0x413F08…0x413F37), then `[0x606850] = 0x14`. Not in a factory-driver replay
    /// (`FEGAME_TYPE [0x6573E8] == 9`), which sets view 0x0A instead (0x413F46) — the caller's test.
    pub fn open_replay(&mut self, reverse: bool) {
        let e = self.opening(reverse);
        self.current = Some(e);
        if let Some(c) = self.camera_at(e).copied() {
            self.tv.switch(&c);
        }
    }

    /// The camera the watched car is on, if any has been reached.
    pub fn current(&self) -> Option<&TrackCamera> {
        self.current.and_then(|i| self.camera_at(i))
    }

    /// The watched car's `car+0x10E4`: the element index of the last link it crossed.
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// One tick of 0x413B80 for the watched car: returns the camera it switched to, if it changed.
    /// `switch` is the sweep's flag, `[0x606850] == 0x14`: without it the car's `+0x10E4` still
    /// follows the links (0x413BF4) but 0x413960 is not called (0x413BFA).
    pub fn update(&mut self, car: &crate::skill::TriggerCar, switch: bool) -> Option<usize> {
        for t in &self.triggers {
            // 0x413BBF: `CTriggerElem::test`, then 0x46EE30 (`+0xDC`) against `car+0x10E4`
            if !t.test(car) || self.current == Some(t.link) {
                continue;
            }
            // `car+0x10E4` takes the link whatever it names; 0x413960 returns at once when
            // 0x46D980 finds no camera there (0x413975) — two monaco3 triggers link to a trigger
            self.current = Some(t.link);
            if !switch {
                continue;
            }
            // a disabled camera still reaches 0x413960, whose 0x413A86 path keeps the eye but
            // changes the zoom (`TvSlot::switch`)
            if let Some(c) = self.camera_at(t.link).copied() {
                self.tv.switch(&c);
                if c.enabled {
                    return Some(t.link);
                }
            }
        }
        None
    }
}

impl CameraTrigger {
    /// `CTriggerElem::test(car)` 0x4704E0, the same routine the skill-test checkpoints run
    /// (`skill-tests.md` §7): `hit` 0x4706C0 — a sphere (`+0x80 == 0`) while `|car+0x330 −
    /// position| < +0x84` (0x470928…0x4709A6); a line (`+0x80 == 1`) when the car's diagonal wheel 0
    /// → wheel 3 (`car+0x7F8`, `+0xA44`) crosses it in x–z (0x489CC0), or failing that wheel 1 →
    /// wheel 2 (`+0x8BC`, `+0x980`), the first refused outright unless both its ends are within
    /// 3.5 m (inclusive) of endpoint `+0xA4`'s height (0x4707B2…0x470810) — then the heading
    /// `dot(+0xC8, car+0x37C) > 0.5` (0x4704FA…0x47056A), the direction of travel
    /// `dot(+0xBC, normalize(car+0x33C)) > 0.5` (0x470575…0x470662; `(0, 1, 0)` for a car at
    /// rest), each skipped when its vector is exactly zero, and the speed window
    /// `car+0x35C + 0.1 > +0x90 && +0x94 + 0.1 > car+0x35C` (0x47066B…0x47069D).
    ///
    /// *(Until 2026-09-26 the port tested the car centre's path over the tick against the trigger
    /// position's height and ran no gate: every shipped camera trigger carries a `+0xBC`, so a car
    /// driving the wrong way — a backwards race on the forward cameras — switched them too.)*
    pub fn test(&self, car: &crate::skill::TriggerCar) -> bool {
        let fired = match self.shape {
            0 => (car.position - self.position).length() < self.radius,
            1 => {
                let near = |y: f32| y + 3.5 >= self.p1.y && y - 3.5 <= self.p1.y;
                let (w0, w3) = (car.wheels[0], car.wheels[3]);
                if !near(w0.y) || !near(w3.y) {
                    return false;
                }
                crate::skill::segments_intersect_xz(w0, w3, self.p1, self.p2)
                    || crate::skill::segments_intersect_xz(car.wheels[1], car.wheels[2], self.p1, self.p2)
            }
            _ => false,
        };
        if !fired {
            return false;
        }
        if self.heading != Vec3::ZERO && self.heading.dot(car.forward) <= 0.5 {
            return false;
        }
        if self.velocity_dir != Vec3::ZERO {
            let dir = if car.velocity == Vec3::ZERO { Vec3::Y } else { car.velocity.normalize() };
            if self.velocity_dir.dot(dir) <= 0.5 {
                return false;
            }
        }
        car.speed + 0.1 > self.speed_min && self.speed_max + 0.1 > car.speed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_type_4_trigger_links_its_audio_element() {
        // castle_audio.scn's first type-4 trigger: a line across the road at slice 1591, its
        // `+0xD8` element 296 ("Bad Dog 1592a"), no gates (line 6 all zero), speeds 0…200
        let Ok(paths) = nfs_assets::GamePaths::locate() else { return };
        let Ok(text) = std::fs::read_to_string(paths.game_data().join("Track").join("castle_audio.scn")) else { return };
        let scene = Scene::parse(&text).unwrap();
        let t: Vec<CameraTrigger> = scene.elements.iter().filter_map(|e| CameraTrigger::from_element(e, 4, 9)).collect();
        assert_eq!(t.iter().map(|t| t.link).collect::<Vec<_>>(), [296, 294, 297, 295]);
        assert!(scene.elements.iter().all(|e| CameraTrigger::from_element(e, 6, 11).is_none()), "no camera links here");
        // a car at the trigger (on slice 1591, 0.7 m off its centre) heading along the slice, its
        // diagonals straddling the line
        let (c, f) = (Vec3::new(-522.2, 39.4, -1526.5), Vec3::new(-0.31, 0.0, -0.95).normalize());
        let l = Vec3::new(f.z, 0.0, -f.x);
        let w = |a: f32, b: f32| c + f * a + l * b;
        let car = crate::skill::TriggerCar { position: c, wheels: [w(1.3, -0.8), w(1.3, 0.8), w(-1.3, -0.8), w(-1.3, 0.8)], forward: f, velocity: f * 20.0, speed: 20.0 };
        assert!(t[0].test(&car));
        // 10 m further on it has passed it
        let car2 = crate::skill::TriggerCar { position: c + f * 10.0, wheels: car.wheels.map(|p| p + f * 10.0), ..car };
        assert!(!t[0].test(&car2));
    }

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
        let c = TrackCamera { position: Vec3::ZERO, view: 12, enabled: true, has_position: true, half_angle: 37.5, lift: 0.0, fov_flag: false, fixed_view: None, dolly: Some(5.0), raise: false, pace: 0.0, opening: 0, spline_offset: Vec3::ZERO };
        let mut tv = TvSlot::default();
        tv.switch(&c);
        tv.update(3.0); // nearer than +0x40: unchanged
        assert_eq!(tv.half_angle, 37.5);
        assert_eq!(tv.near, 0.5);
        tv.update(50.0);
        assert!((tv.half_angle - 3.75).abs() < 1e-4, "{}", tv.half_angle);
        // the zoom pushes the near plane out with it: (37.5 − 3.75) / 3.75
        assert!((tv.near - 9.0).abs() < 1e-3, "{}", tv.near);
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
    fn the_replay_opens_on_the_flagged_camera() {
        // c1 with `+0xC0` 0, then the sample's own camera (`+0xC0` 2) again as c2 at element 2
        let two = SAMPLE.replacen("1 1 12 0 2 ", "1 1 12 0 0 ", 1) + &SAMPLE.replace("c1\n", "c2\n");
        let c = TrackCameras::from_scene(&Scene::parse(&two).unwrap());
        assert_eq!((c.cameras[0].opening, c.cameras[1].opening), (0, 2));
        assert_eq!(c.opening(true), 2);
        // no camera flagged 1: the first camera element
        assert_eq!(c.opening(false), 0);
        let mut c = c;
        c.open_replay(true);
        assert_eq!(c.current_index(), Some(2));
        assert_eq!(c.tv.eye, Some(c.cameras[1].position));
    }

    #[test]
    fn the_link_is_an_element_index() {
        // alps_cameras.scn: c1, its trigger (link 0), c2, its trigger (link 2) — 0x46D980 indexes
        // the element list, so link 2 is c2, the second camera, not the third
        let two = SAMPLE.replace("Replay Camera Trigger\n", "Replay Camera Trigger\n")
            + &SAMPLE.replace("c1\n", "c2\n").replace("-1 -1 -1 -1 0 -1", "-1 -1 -1 -1 2 -1").replace("1 1 12 0 2", "1 1 11 0 2");
        let c = TrackCameras::from_scene(&Scene::parse(&two).unwrap());
        assert_eq!(c.triggers[1].link, 2);
        assert_eq!(c.camera_at(2).map(|c| c.view), Some(11));
        // a link to a trigger answers no camera (monaco3's two)
        assert_eq!(c.camera_at(1), None);
    }

    #[test]
    fn crossing_the_line_switches_the_camera() {
        let s = Scene::parse(SAMPLE).unwrap();
        let mut c = TrackCameras::from_scene(&s);
        let t = c.triggers[0];
        let mid = (t.p1 + t.p2) * 0.5;
        // a car on the line, its diagonals across it, driving along the trigger's `+0xBC`
        let fwd = t.velocity_dir;
        let side = (t.p2 - t.p1).normalize();
        let car = |dir: Vec3| crate::skill::TriggerCar {
            position: mid,
            wheels: [mid + fwd * 1.2 - side * 0.8, mid + fwd * 1.2 + side * 0.8, mid - fwd * 1.2 - side * 0.8, mid - fwd * 1.2 + side * 0.8],
            forward: dir,
            velocity: dir * 30.0,
            speed: 30.0,
        };
        // the wrong way: the `+0xBC` gate refuses it
        assert_eq!(c.update(&car(-fwd), true), None);
        assert_eq!(c.current_index(), None);
        // without the flag the car takes the link but the view is not switched
        assert_eq!(c.update(&car(fwd), false), None);
        assert_eq!(c.current_index(), Some(0));
        assert_eq!(c.tv.switches, 0);
        let mut c = TrackCameras::from_scene(&s);
        assert_eq!(c.update(&car(fwd), true), Some(0));
        assert_eq!(c.current().map(|c| c.view), Some(12));
        // it does not fire again for the camera it is already on
        assert_eq!(c.update(&car(fwd), true), None);
    }
}
