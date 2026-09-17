//! Trackside cameras and the triggers that switch them — `RE/notes/race-logic.md` §3.3.
//!
//! A `<track>_cameras.scn` is pairs of `CAMERA_ELEMENT` and `TRIGGER_ELEMENT`. Every tick
//! `0x413B80` tests every trigger against every car; when a car enters one whose `+0xDC` camera
//! link differs from the one it already has, that becomes the car's camera (`car+0x10E4`), and for
//! the car the player is watching (`[0x606858]`) `0x413960(link)` switches the view:
//!
//! ```text
//! cam = 0x46D980(link)                    ; the camera element with that index
//! if !cam->+0x7C: return                  ; disabled
//! setView(0, cam->+0x78)                  ; the camera's own view id (12 in the shipped files)
//! if cam->+0x8C: camera position = cam->vt[0x38]()   ; 0x627EA4…
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
    /// `+0x9C` — field of view in degrees (37.5 / 45 in the shipped files)
    pub fov: f32,
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
            fov: p.get(7).copied().filter(|f| *f > 1.0).unwrap_or(45.0),
        }
    }

    fn trigger(e: &SceneElement) -> Option<CameraTrigger> {
        let p = e.params.first()?;
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
            // a disabled camera still becomes the car's camera; it just does not switch the view
            self.current = Some(t.link);
            if self.cameras.get(t.link).is_some_and(|c| c.enabled) {
                return Some(t.link);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!((c.cameras[0].fov - 37.5).abs() < 1e-3);
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
