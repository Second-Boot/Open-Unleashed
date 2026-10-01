//! Cameras: the chase / cockpit views, the animated cameras and the head look.

use crate::*;

impl App {
    /// The shadow projector a car at `pos` takes, 0x43A6F1 → 0x456160 (`crp-geometry.md` §8),
    /// only while `[0x6197CA]` is set — `Graphics.CarChrome` = High (0x44EF19…0x44EF33; Low and
    /// Medium clear it, `lighting.md` §5.3). The candidates are the projectors in the map
    /// `[0x627DFC]`, which register as their articles draw (0x450DBD → 0x456090,
    /// `TrackWorld::register_projectors` from this frame's walk), and that map is made with the
    /// track (0x455F60 ← 0x46907E, beside `RdrKeys`) and deleted with it (0x455FF0 ← 0x46912C),
    /// never cleared between frames — so a projector stays a candidate from its article's first
    /// draw on. The port registers before the frame's cars draw; the original's queue runs the hook
    /// with the track's other queued parts, at most a frame apart (the queue's flush order is not read).
    pub(crate) fn projector_planes(&self, pos: Vec3) -> Option<([f32; 4], [f32; 4])> {
        if self.choice_index("Graphics.CarChrome").is_none_or(|(i, _)| i < 2) {
            return None;
        }
        self.world.as_ref().and_then(|w| w.projector_for(pos)).map(|q| (q.u, q.v))
    }

    /// The `.tpg` entry test's `testdrive` context (0x439E75…0x439EC1): on `skidpad` an entry's
    /// `testdrive` must equal `[0x6573E8] == 9` — `Game.GameType` 9 is a Factory Driver mission —
    /// and elsewhere `testdrive = 1` rules it out. `Some(mission)` on `skidpad`, `None` elsewhere.
    pub(crate) fn skidpad_rule(&self) -> Option<bool> {
        self.track.name.eq_ignore_ascii_case("skidpad").then_some(self.mission.is_some())
    }

    /// The view the camera is on, by the original's ids (`nfs_game::camera`).
    /// `Graphics.WideScreen` on (`[0x657D5C]`, `camera::WIDESCREEN_BAND`).
    pub(crate) fn widescreen(&self) -> bool {
        self.choice_index("Graphics.WideScreen").is_some_and(|(i, _)| i == 1)
    }

    pub(crate) fn camera_view(&self) -> u16 {
        // diagnostic: `NFS_VIEW=<id>` (decimal: 10 CircleCam, 11 SplineCam, 20 DirectorsCam …) holds
        // the slot on that view, for screenshots of views outside the car's cycle
        if let Some(v) = nfs_game::config::cfg().nfs_view.var().ok().and_then(|v| v.parse::<u16>().ok()) {
            return v;
        }
        if let Some(v) = self.replay_view() {
            return v;
        }
        if self.anim_cam != AnimCamSlot::Off {
            return 0x0E;
        }
        self.camera_cycle
            .get(self.camera_mode as usize)
            .copied()
            .unwrap_or(0x02)
    }

    /// The view a **playing** replay is on — its own nine-entry cycle (`replay::CAMERA_IDS`, the
    /// table 0x5CC654), not the car's camera list. 0x414953 applies the entry: `0x14` is the
    /// track-side "TV" camera only while the track has camera nodes and **id 3** (`TailCam1`) when
    /// it has none, and every other id goes through as it is.
    ///
    /// `0x0A CircleCam` circles the car (0x462980) and `0x0B SplineCam` follows the slices
    /// (0x463410); both are ported.
    pub(crate) fn replay_view(&self) -> Option<u16> {
        let playing = self.replay.stream.as_ref().is_some_and(|r| {
            matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)
        });
        if !playing {
            return None;
        }
        let id = *nfs_game::replay::CAMERA_IDS.get(self.replay.camera as usize)?;
        Some(nfs_game::replay::apply_camera(id as u8, self.track_has_camera_nodes()) as u16)
    }

    /// `LINEAR_TRACK [0x657494]` = `trk+0x6E != 1` (`modes.md` §2): the track is point-to-point.
    pub(crate) fn linear_track(&self) -> bool {
        !self.records.track(&self.track.name).map(|t| t.circuit).unwrap_or(true)
    }

    /// `0x46D960()` — whether the track carries any camera node at all, which is what decides
    /// between the "TV" camera and its fallback both at 0x413F3A and at 0x414960.
    fn track_has_camera_nodes(&self) -> bool {
        self.track_cameras.as_ref().is_some_and(|t| !t.cameras.is_empty())
    }

    /// `SKYRENDERING` `[0x657D9C]` (the `Graphics.SkyRendering` choice, or `NFS_SKY_LEVEL`): where
    /// 0x459950 starts its search for the colour record and 0x45FB80's block level.
    pub(crate) fn sky_level(&self) -> usize {
        nfs_game::sky::forced_level().unwrap_or_else(|| self.choice_index("Graphics.SkyRendering").map_or(0, |(i, _)| i))
    }

    /// A car's `R3DSkyMapTextureLayer` (`RE/notes/lighting.md` §3.2): the maps of its own slice's
    /// section, only while `[0x6197CB]` is set — `Graphics.CarChrome` = High (0x43A730; below it
    /// 0x43A780 binds none and the pass draws nothing). `origin` is the car's `car+0x330`.
    pub(crate) fn sky_map_for(&self, origin: Vec3, slice: usize) -> Option<nfs_renderer::SkyMap> {
        if self.choice_index("Graphics.CarChrome").is_none_or(|(i, _)| i < 2) {
            return None;
        }
        let record = self.slices.as_ref()?.slices.get(slice)?;
        self.world.as_ref()?.sky_map(origin, slice, record)
    }

    /// **The camera slots set up again — 0x4640B0**, which the session loop runs on every pass
    /// (0x4B6DB9: a new race, a restart, the automatic replay). For each slot it takes the viewed
    /// car's position and matrix afresh, puts the view `+0xA8` to 0x0E (`AnimCam`, the start
    /// animation) and the cycle index `+0xAA` to 0 (0x4641BC / 0x4641C5), the flag bytes `+0xAC…
    /// +0xAF`, `+0xB0…+0xB4` to 0, `+0xC0` to 0xFF and `+0xC4` to 0x40, and the offset `+0x08` to
    /// −4 × the car's `+0x37C` row (0x465CF0). When the start animation is over, 0x462F30 gives a
    /// live race's slot its car's first camera, `car[n].camera[0]` (`[0x658294 + n·0x580]`,
    /// 0x46321B…0x46322C) — **every race opens on the list's first view**, whichever the player
    /// had cycled to (`I_CYCLECAMERA` 0x465A60 steps `+0xAA`) — and that SetSlotView 0x465790
    /// starts the followers again: the settle count `+0xC1` = 0x32 (0x465848), the yaw `+0xC6`
    /// and its step `+0xC8` 0 (0x4658B2 / 0x4658B9), the offset (0x465805). The port keeps the
    /// slot's followers on `App`, so they go back to what they were before the first race.
    /// *(Until 2026-09-26 the view the last race ended on carried into the next, the follower
    /// state with it; and the front end's `Camera.Current` click set the race's first view —
    /// the launch 0x4EF010 never reads that accessor.)* The slot starts on `AnimCam`
    /// (`AnimCamSlot::Start`, `camera_anim_step`). `NFS_CAMERA` (diagnostic) still names another
    /// index, and then skips the opening animation.
    /// **Slot 0's `AnimCam` update, 0x462F30** (dispatch 0x463FFE), for the frame's latest game
    /// tick. With no player in `+0x0C` it asks 0x471A50 for the next (`anim_cam::pick`); the start
    /// is `gGameTick` then — **0** for the pick of 0x4640B0's own dispatch, 0x414EB0 having
    /// zeroed the clock just before. Each update samples the player at `gGameTick − start`
    /// (`anim_cam::sample`) and builds the slot from it (`anim_cam::pose`). When there is none, or
    /// it has run out (status 2, 0x462F89), the slot leaves the view (0x463084): in a live race
    /// SetSlotView gives it `car[n].camera[0]` (0x46321B…0x46322C; `+0xAC` bit 0 is clear, 0x4640B0
    /// wrote `0x20`), the player is deleted and the dispatcher runs again (0x46324A) — the new
    /// view's update runs in the same tick. So every live race opens on 119 keys at 1/28 s, 272
    /// game ticks (4.25 s of the 7 s countdown), then the list's first camera.
    ///
    /// In a replay the pass's dispatch still picks (the counter steps) and samples once, but
    /// `Replay_startRace` 0x413EA0, run right after 0x4640B0, puts the slot on the track's
    /// opening camera through 0x413960 → SetSlotView (or 0x4659E0 with view 0x0A without camera
    /// nodes), so a replay never shows the animation (`replay_view` rules there).
    ///
    /// Diagnostic: `NFS_VIEW=14` holds the view, and the update then picks the next path each time
    /// one runs out (0x462F58 with `+0x0C` cleared at 0x463244), as the original's slot would if
    /// nothing moved it off 0x0E.
    pub(crate) fn camera_anim_step(&mut self, tick: u64, car_pos: Vec3, car_rot: Quat) -> Option<(Vec3, glam::Mat3)> {
        let held = nfs_game::config::cfg().nfs_view.var().ok().and_then(|v| v.parse::<u16>().ok()) == Some(0x0E);
        if held && self.anim_cam == AnimCamSlot::Off {
            self.anim_cam = AnimCamSlot::Start;
        }
        if self.anim_cam == AnimCamSlot::Start {
            self.anim_cam_pick(0);
        }
        if !held && self.replay_view().is_some() {
            self.anim_cam = AnimCamSlot::Off;
        }
        let AnimCamSlot::Playing { index, start } = self.anim_cam else { return None };
        let ticks = tick as i64 - start as i64;
        match self.cam_anims.get(index).and_then(|t| nfs_game::camera::anim_cam::sample(t, ticks)) {
            Some((a, t)) => {
                let car = glam::Mat3::from_quat(car_rot).transpose();
                let (eye, m) = nfs_game::camera::anim_cam::pose(a, t, car, car_pos);
                tracing::debug!(ticks, index, local = ?t, fwd = ?(car * m.row(2)), "AnimCam sample");
                Some((eye, m))
            }
            None => {
                tracing::info!(tick, index, "AnimCam over (0x463084): the slot takes the cycle's first view");
                if held {
                    self.anim_cam_pick(tick);
                    return self.camera_anim_step(tick, car_pos, car_rot);
                }
                self.anim_cam = AnimCamSlot::Off;
                None
            }
        }
    }

    /// 0x462F58…0x462F64: the next `CmAn` player into the slot, started at game tick `start`.
    fn anim_cam_pick(&mut self, start: u64) {
        self.anim_cam = match nfs_game::camera::anim_cam::pick(&mut self.anim_cam_counter, self.cam_anims.len()) {
            Some(index) => AnimCamSlot::Playing { index, start },
            None => AnimCamSlot::Off,
        };
        tracing::debug!(state = ?self.anim_cam, "AnimCam pick (0x471A50)");
    }

    pub(crate) fn camera_slot_reset(&mut self) {
        let forced = nfs_game::config::cfg().nfs_camera.var().ok().and_then(|v| v.parse::<u8>().ok());
        self.camera_mode = forced.unwrap_or(0);
        self.anim_cam = if forced.is_some() { AnimCamSlot::Off } else { AnimCamSlot::Start };
        self.chase_offset = None;
        self.chase_settle = 0;
        self.look_yaw = None;
        self.orbit = Orbit::default();
        self.camera_tick = 0;
        self.cam_hist.clear();
        self.flip_side = None;
        self.slot_eye = None;
        // the pass's SetSlotView zeroes the wander too (0x4657A0…0x4657F3)
        self.replay_wander = Default::default();
        self.circle = (nfs_game::camera::CircleCam::default(), 0xFFFF, 0);
        self.cam_slice = None;
    }
}

/// Slot 0's `AnimCam` state: the view `+0xA8` on 0x0E and the player in `+0x0C`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AnimCamSlot {
    /// on another view
    #[default]
    Off,
    /// 0x4640B0 has put the slot on 0x0E with no player (`+0x0C` = 0, 0x464129): the dispatch at
    /// its end (0x4642D1) picks one at the pass's `gGameTick` 0
    Start,
    /// playing animation `index`, started at game tick `start`
    Playing { index: usize, start: u64 },
}

/// The pause orbit's state, slot 0's words `+0xC6` (the yaw, 1/1024 turns — the same word the
/// look-around uses in degrees for views 0 and 1) and `+0xC8` (its step a frame, `[0x627F48]`),
/// both zeroed as the pause opens (0x461740); `base` is the offset the orbit turns. The camera
/// is built once a frame, and so the yaw steps once a frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Orbit {
    pub(crate) yaw: i16,
    pub(crate) step: i16,
    pub(crate) base: Option<Vec3>,
}

/// A car's headlight ramp, 0x43950E…0x4396A1: `{start, duration, from, to}` at `[car+0x64]+0x110`
/// and the flag `+0x567` it compares the lamp bit with, all in units of the full level `+0x70`.
/// On a change of the bit the flag takes it at once; a ramp still running (`now − start <
/// duration`, `jb`) then keeps going and the change starts nothing. Otherwise the value the old
/// ramp had reached becomes `from` (its `to` when it had run out or there was none), the bit's
/// level `to`, and a new ramp of 0x3E8 ms starts. The value is `from + (to − from)·t` while
/// `t = elapsed / duration` is at most 1 (`test ah, 0x41`), else `to` (and the duration is cleared).
/// The clock is `GetTickCount` (0x53BFD0) — here the port's lamp clock in ms.
/// What the camera slot's per-tick update reads of one game tick (0x463C30 on `Sc64`): the
/// viewed car's pose at the end of that tick (`car+0x330` with the body lift, the matrix), the
/// car's local velocity (the follow rate, the pull-back), its half length, and the race slice
/// with the wrong-way flag (the slope lift).
#[derive(Debug, Clone, Copy)]
pub(crate) struct CamTick {
    pub(crate) tick: u64,
    pub(crate) pos: Vec3,
    pub(crate) rot: Quat,
    pub(crate) v_local: Vec3,
    pub(crate) half_z: f32,
    pub(crate) slice: usize,
    pub(crate) wrong_way: bool,
    /// the world velocity (the SplineCam's pace, 0x46398F) and `car+0x3A0` (the roll-over test, 0x463CA6)
    pub(crate) velocity: Vec3,
    pub(crate) cos_tilt: f32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct HeadRamp {
    pub(crate) flag: bool,
    pub(crate) start: f64,
    duration: f64,
    pub(crate) from: f32,
    pub(crate) to: f32,
}

impl HeadRamp {
    pub(crate) fn value(&mut self, bit: bool, now: f64) -> f32 {
        if self.flag != bit {
            self.flag = bit;
            if now - self.start >= self.duration {
                let cur = self.current(now);
                self.from = cur;
                self.to = if bit { 1.0 } else { 0.0 };
                self.start = now;
                self.duration = 1000.0;
            }
        }
        self.current(now)
    }

    pub(crate) fn current(&mut self, now: f64) -> f32 {
        if self.duration > 0.0 {
            let t = ((now - self.start) / self.duration) as f32;
            if t <= 1.0 {
                return self.from + (self.to - self.from) * t;
            }
            self.duration = 0.0;
        }
        self.to
    }
}

/// The look-around target of 0x4611C0 (0x4611C4…0x461220), in degrees as the word it keeps:
/// 0, then −90 with look-left (bit 0x14) and +90 with look-right (0x15, which wins); with
/// look-back (0x13) `180 − t/2`; with look-forward (0x16) `t/2` (both `cdq; sub; sar` —
/// truncated toward 0). Look-forward alone is 0, as nothing held.
pub(crate) fn look_target(left: bool, right: bool, back: bool, forward: bool) -> i16 {
    let mut t: i16 = 0;
    if left {
        t = -90;
    }
    if right {
        t = 90;
    }
    if back {
        t = 180 - t / 2;
    }
    if forward {
        t /= 2;
    }
    t
}

#[cfg(test)]
mod look_tests {
    use super::look_target;

    /// 0x4611C4…0x461220: left −90, right +90 (right wins), back `180 − t/2`, forward `t/2`.
    #[test]
    fn the_four_look_bits() {
        let t = |l, r, b, f| look_target(l, r, b, f);
        assert_eq!(t(false, false, false, false), 0);
        assert_eq!(t(true, false, false, false), -90);
        assert_eq!(t(false, true, false, false), 90);
        assert_eq!(t(true, true, false, false), 90);
        assert_eq!(t(false, false, true, false), 180);
        assert_eq!(t(true, false, true, false), 225);
        assert_eq!(t(false, true, true, false), 135);
        assert_eq!(t(true, false, false, true), -45);
        assert_eq!(t(false, false, true, true), 90);
        assert_eq!(t(false, false, false, true), 0);
    }
}
