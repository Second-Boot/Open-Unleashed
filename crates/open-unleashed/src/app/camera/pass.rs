//! The camera slot's per-frame pass (`App::camera_pass`: the view's updates since the last tick, the slot's view switch, the
//! `AnimCam` steps) and the audio listener's second copy of the slot. Moved out of `render_scene.rs` unchanged.

use crate::*;

use super::build::ceiling_at;

/// No trackside camera within this distance (m) of the car (TVCam / DirectorsCam before any trigger): the fallback view behind the
/// car, this far back, this high, aimed this high above it. APPROXIMATION (the port's stand-in: no original counterpart read).
const TV_REACH: f32 = 400.0;
const TV_FALLBACK_BACK: f32 = 7.5;
const TV_FALLBACK_UP: f32 = 2.3;
const TV_FALLBACK_AIM: f32 = 0.9;

impl App {
    /// The camera slot's per-frame work (`RE/notes/camera*.md`): the view's update(s) since the last
    /// game tick, the car-relative eye, the mirror's eye, the beacon and the headlight frames.
    /// Returns the view id and the slot's near plane.
    pub(crate) fn camera_pass(&mut self, car_pos: Vec3, car_rot: Quat, rear_view: bool, audio: bool, alpha: f32) -> (u16, f32) {
        // every view is slot 0's half-angle, 37.5° by default, across (`camera::fov_y`); only a
        // trackside camera element brings its own (`+0x9C` when `+0x98` is set)
        // the TV modes take the view the last trackside camera set (`TvSlot::view`): the sweep
        // 0x413B80 switches only while the chosen camera `[0x606850]` is 0x14 (0x463E1D…0x463E28),
        // and then 0x413960's SetSlotView makes the element's own view the slot's.
        // `[0x606850] == 0x14`: the chosen camera is the TV one
        // the slot's `AnimCam` update 0x462F30 runs once per game tick too (dispatch 0x463FFE of the
        // same `Sc64`): it samples the player at `gGameTick − start`, so a frame of several ticks asks it for
        // each tick in turn, and when the player runs out at tick `m` of the frame (status 2, 0x462F89) the
        // slot's SetSlotView takes the cycle's first view and the dispatcher runs again in the same tick
        // (0x46324A) — the new view's updates are the frame's remaining ticks, from `m` on. (`AnimCam` keeps
        // no state but the clock, so the ticks before the end leave nothing else behind.) A frame between two
        // ticks (`steps == 0`) asks once, at the frame's own tick.
        let cam_tick0 = self.cam.camera_tick;
        let mut steps = self.tick.saturating_sub(self.cam.camera_tick).min(u32::MAX as u64) as u32;
        self.cam.camera_tick = self.tick;
        let mut anim_pose = None;
        let mut anim_done = 0;
        for u in 0..steps.max(1) {
            let was_anim = self.cam.anim_cam != AnimCamSlot::Off;
            let at = if steps == 0 { self.tick } else { cam_tick0 + 1 + u64::from(u) };
            anim_pose = self.camera_anim_step(at, car_pos, car_rot);
            if was_anim && anim_pose.is_none() && self.cam.anim_cam == AnimCamSlot::Off {
                anim_done = u;
                break;
            }
        }
        steps -= anim_done;
        let tv_mode = matches!(self.camera_view(), 0x0C | 0x14);
        let view = match (self.camera_view(), self.track_cameras.as_ref().and_then(|t| t.tv.view)) {
            (0x0C | 0x14, Some(v)) if v != 0x0C && v <= 0x13 => v,
            (v, _) => v,
        };
        // **The slot's offset `+0x08` seed 0x465CF0**: `-4.0 · car+0x37C` (the matrix's third row — the
        // car's forward axis), called with the slot's mask at the race's init 0x4640B0 (0x464247), by SetSlotView
        // 0x465790 (0x465805) and by the cycle key 0x465A60 (0x465B1A) — the last two only when the slot's OLD
        // view is outside 2…7 (`cmp ax, 2 ; jl` / `cmp ax, 7 ; jle`, 0x4657EF…0x4657FF, 0x465B0A…0x465B14),
        // before the new view is stored. The views 2…7 keep their own offset across a switch; the others
        // (0, 1, 8…0x0F, 0x10…) leave it at the seed, which a view whose update never writes it (CircleCam,
        // SplineCam, TVCam, BlimpCam, 0x10 / 0x11 / 0x12 / 0x13) then keeps for the pause orbit (0x461850) to turn.
        // The switch is seen here as a change of the view since the last pass.
        // SetSlotView re-entered on the SAME view (the cycle key landing on it, 0x465A60; an enabled
        // trackside switch to an element of the same view, 0x4139DD) runs the same reset — the settle count back
        // to 0x32 (0x465848) and, when the old view is outside 2…7, the offset seed — which a view-tag compare
        // cannot see
        let entered_now = (self.slot_key_enters, self.track_cameras.as_ref().map_or(0, |t| t.tv.switches));
        let slot_entered = self.cam.slot_view.is_some() && entered_now != self.cam.slot_seen;
        self.cam.slot_seen = entered_now;
        if self.cam.slot_view.is_none_or(|old| (old != view || slot_entered) && !(0x02..=0x07).contains(&old)) {
            self.cam.chase_offset = Some((car_rot * Vec3::Z * -4.0, u16::MAX));
        }
        // SetSlotView ran this pass (a new view, or the same one entered again)
        let view_entered = self.cam.slot_view.is_none_or(|old| old != view) || slot_entered;
        self.cam.slot_view = Some(view);
        let ws = self.widescreen();
        // the dll's SetMode cells (`camera::set_mode`) for the window as it is now — the
        // default half-angle `[0x10006080]` SetSlotView stores (0x46584F) and the aspect 0x466F30 takes
        if !audio && let Some(r) = self.renderer.as_ref() {
            let (w, h) = r.size();
            nfs_game::camera::set_mode(w, h);
        }
        let fov_of = |h: f32| nfs_game::camera::fov_y_window(nfs_game::camera::half_angle(h, ws, view));
        // ENHANCEMENT E4: the race views' default angle widened by `bonus` degrees on the original's horizontal
        // angle (the half-angle after the wide-screen factor, so +N° across); a trackside / TV / spline camera
        // that brings its own half-angle keeps the authored one, the mirror and the front end's rooms are other builds
        let bonus_half = self.enh_fov_bonus() * 0.5;
        self.camera.fov_y_radians = if bonus_half > 0.0 {
            nfs_game::camera::fov_y_window(nfs_game::camera::half_angle(nfs_game::camera::mode_half_angle(), ws, view) + bonus_half)
        } else {
            fov_of(nfs_game::camera::mode_half_angle())
        };
        // the slot's near plane `+0x5C` (`camera::near_plane`, applied with the far plane below):
        // SetSlotView's 0.5 unless 0x462B80 (TVCam, SplineCam) has zoomed
        let mut slot_near = nfs_game::camera::DEFAULT_NEAR;
        // the aimed views' up comes out of 0x464BF0 below, which may keep last build's up row
        let prev_up = self.camera.up;
        // and its forward row `+0x94`, which the roll-over camera reads on its first update
        let prev_fwd = (self.camera.target - self.camera.position).normalize_or_zero();
        // the slot updates since the last frame: one per game tick (`camera_tick`); a frame drawn
        // between two ticks moves nothing but the interpolated car the offsets hang off.
        // every view's update (chase / heli 0x461E80 / 0x4623D0, the roll-over 0x461590,
        // 0x4618B0, the driver's head lag 0x461A30, BlimpCam, the circle / spline / trackside cameras) runs once
        // per game tick against its own tick's car (`Sc64`, 0x463C30) — the [`CamTick`] history; the last update
        // of the frame takes this frame's (interpolated) pose. The look-around (0x4611C0) reads no car state
        // (held bits only); `AnimCam` 0x462F30 is stepped per tick above.
        // `+0xC0 & 0x08`, the aim bit — the table's 0x04 shifted up by SetSlotView: views 2…8,
        // 0x0A, 0x0B…0x0D (a trackside camera without its own matrix, below), 0x12 / 0x13 (the cop
        // cameras, not carried)
        let mut aimed = matches!(view, 0x02..=0x08 | 0x0A | 0x0B | 0x0D);
        self.camera.up = Vec3::Y;
        let off = nfs_game::camera::VIEW_OFFSETS
            .get(view as usize)
            .copied()
            .unwrap_or(nfs_game::camera::VIEW_OFFSETS[2]);
        // a view that rides the car and looks forward from it: the bumper and shotgun cameras
        let rides_car = view == 0x00;
        // the chase family — behind the car, heading and height low-passed
        let chases = matches!(view, 0x02..=0x07 | 0x0D);
        // **0x4611C0**: the bumper and driver views turn about the slot's up axis toward the
        // target the four look bits make (`look_target`) and back to 0 when they are released
        // (`camera::look_step`). The bits are levels, `RaceFrame` reading them held
        // (0x415AFA…0x415B2C: 0x16 `[0x5E99A8]`, 0x15 `[0x5E99AC]`, 0x13 `[0x5E99B0]`, 0x14
        // `[0x5E99B4]`), and only while the pause is off (`[0x606A90]`, 0x415982).
        if !self.paused {
            self.cam.look_held = [Action::LookLeft, Action::LookRight, Action::LookForward].map(|a| self.vehicle.is_some() && self.input.held(a));
        }
        let [look_left, look_right, look_fwd] = self.cam.look_held;
        let look = if matches!(view, 0x00 | 0x01) && self.vehicle.is_some() {
            let (mut y, mut turn) = match self.cam.look_yaw {
                Some((y, v, t)) if v == view => (y, t),
                _ => (0, false),
            };
            let target = look_target(look_left, look_right, rear_view, look_fwd);
            for _ in 0..steps {
                (y, turn) = nfs_game::camera::look_step(y, target);
            }
            self.cam.look_yaw = Some((y, view, turn));
            Quat::from_rotation_y(if turn { (y as f32).to_radians() } else { 0.0 })
        } else {
            self.cam.look_yaw = None;
            Quat::IDENTITY
        };
        // **one slot update per game tick** (`Sc64`, 0x463C30 — every view's update, not the chase /
        // heli pair alone): update `upd` of the frame reads its own tick's car (`car+0x330`, `+0x364`,
        // `+0x3A0`, the velocity, the slice), the last the frame's interpolated pose (glue); the slot's eye
        // `+0x24` the update before left seeds the next one's ceiling tests. A frame between two ticks
        // (`steps == 0`) runs the body once with no step. The build 0x464F80 (pushes, clamp, aim) is not in
        // the dispatcher: it runs once per frame on the last eye, as below. (The slot matrix's forward row,
        // which the roll-over latch reads, is the build's, so `prev_fwd` is the frame's for every update.)
        let aimed0 = aimed;
        let frame_steps = steps;
        let n_upd = (frame_steps + anim_done).max(1);
        let (frame_car_pos, frame_car_rot) = (car_pos, car_rot);
        // PORT-DEVIATION D8 (camera followers, `CamFollow`): the views whose offset from the car is a state stepped
        // per tick (chase / heli, the roll-over of those, BlimpCam) take the tick's own pose in the frame's last
        // update too, not the interpolated one, so the state is a function of the tick sequence alone; the eye is
        // drawn at `lerp(offset before, offset after, alpha)` from the interpolated car below the loop. The
        // other views (rigid on the car, aimed at it, the world's cameras) keep the frame's pose in that update.
        let follower_view = matches!(view, 0x02..=0x07 | 0x0D) && !rear_view;
        let mut followed = false;
        for upd in anim_done..n_upd {
            let last_upd = upd + 1 == n_upd;
            let steps = frame_steps.min(1);
            let hist = if !last_upd || (follower_view && frame_steps > 0) {
                // the ticks are consecutive: the entry is at its offset from the oldest
                let want = cam_tick0 + 1 + u64::from(upd);
                let first = self.cam.cam_hist.front().map_or(want, |h| h.tick);
                self.cam.cam_hist.get(want.wrapping_sub(first) as usize).filter(|h| h.tick == want).copied()
            } else {
                None
            };
            // a frame between two ticks: the follower's latest tick pose
            let held = (last_upd && follower_view && frame_steps == 0).then(|| self.cam.cam_follow.filter(|f| f.view == view).map(|f| f.pose)).flatten();
            let (car_pos, car_rot) = hist.map(|h| (h.pos, h.rot)).or(held).unwrap_or((frame_car_pos, frame_car_rot));
            // the follower's offset from the car this update leaves (exact state, not eye - car)
            let mut follow_off: Option<Vec3> = None;
            let fwd = car_rot * Vec3::Z;
            aimed = aimed0;
            // what the update reads of its tick's car: the frame's own for the last
            let cam_car = hist.or_else(|| {
                let v = self.vehicle.as_ref()?;
                let (slice, wrong_way) = self.race.as_ref().map_or((0, false), |r| (r.slice, r.wrong_way_ticks > 0));
                Some(CamTick { tick: 0, pos: car_pos, rot: car_rot, v_local: v.v_local, half_z: v.half_extents.z, slice, wrong_way, velocity: v.velocity, cos_tilt: v.cos_tilt })
            });
            let ceiling = |w: &nfs_game::world::TrackWorld, p: Vec3, y: f32| -> f32 { ceiling_at(w, p, car_pos, y) };
            // look-back (bit 0x13) is read only by views 0…7 (0x4611C0, 0x461B96, 0x461E80, 0x4623D0; the
            // one other reader, 0x444F89, gates a draw): elsewhere it changes nothing
            // **The roll-over camera** (dispatcher 0x463C9E…0x463D2A): for a slot whose flags carry
            // `+0xC0 & 0x40` — the table's 0x20, views 2…0x0A — the view's own update is replaced by
            // **0x461590** while the car's `car+0x3A0` (the ground normal · its up) is below
            // `[0x5B23F0]` 0.5 (`test ah, 1`: C0, strictly below). On the first such update (`+0xC2`
            // 0) the side is latched in `+0xC3` bit 0: the slot matrix's forward row `+0x94` · the
            // slice forward `car+0x3D0` < 0 (0x463CC6…0x463CFB); `+0xC2` is set to 100 on every
            // tilted update, so it is only cleared — and the side read again — once the car is
            // upright (0x463D03). 0x461590: the offset (0, 3, −10), or (0, 3, +10) with the bit, is
            // turned by the **slice's** axes `car+0x3B8` (lateral, up, forward rows, 0x46160C); its y
            // the lower of the two ceiling tests at the car and the old eye (0x461632…0x461669); the
            // slot offset `+0x08` moves toward it by `[0x5B3F50]` 0.02 on every axis (0x46166D…
            // 0x4616C7), eye = car + offset, and the slot flags become `+0xC0 & ~4 | 0x7A`
            // (0x46171F…0x46172C): aimed with the lift (0.325 for views 2…4, else 0.4, 0x464EAF…
            // 0x464ED0), pushed and clamped. The dispatcher sets `+0xC0` bit 0 so the next normal
            // update reloads the view's flags (0x463D22…0x463D59); the offset carries over.
            let flip = if matches!(view, 0x02..=0x0A)
                && let (Some(v), Some(t)) = (cam_car, self.slices.as_ref())
                && let Some(sl) = t.slices.get(v.slice)
            {
                if v.cos_tilt < 0.5 {
                    let side = *self.cam.flip_side.get_or_insert(prev_fwd.dot(sl.forward) < 0.0);
                    Some(sl.up * 3.0 + sl.forward * if side { 10.0 } else { -10.0 })
                } else {
                    self.cam.flip_side = None;
                    None
                }
            } else {
                None
            };
            if let Some(mut a) = flip {
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.cam.slot_eye.unwrap_or(car_pos + a);
                    a.y = ceiling(w, car_pos, a.y).min(ceiling(w, old_eye, a.y));
                }
                let mut cur = self.cam.chase_offset.map_or(a, |(o, _)| o);
                for _ in 0..steps {
                    cur += (a - cur) * 0.02;
                }
                self.cam.chase_offset = Some((cur, view));
                follow_off = Some(cur);
                self.camera.position = car_pos + cur;
                self.camera.target = car_pos + Vec3::Y * 4.0 * if (0x02..=0x04).contains(&view) { 0.325 } else { 0.4 };
                aimed = true;
            } else if matches!(view, 0x08 | 0x09 | 0x0F) {
                // **0x4618B0** (`ShotgunCam2` / `ShotgunCam3` / `ReverseCam1`, dispatch 0x464005): the eye is
                // the car plus its matrix times the table offset, whose y is the lower of the two ceiling
                // tests at the car and the old eye (0x461903…0x461964); the slot's offset is that; the
                // slot matrix is the car's with its right and forward rows negated — a half turn about
                // its up (0x4619BD…0x461A1E). SetSlotView puts the table's flag bits one place up in
                // `+0xC0` (0x4658C0…0x465965), so the aim bit 0x08 is the table's 0x04: view 8 (0x3C) is
                // aimed at the car's origin with the world's up by 0x464BF0; 9 (0x38) and 0x0F (0x12)
                // keep the half-turned matrix and look back down the car.
                let mut a = car_rot * off;
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.cam.slot_eye.unwrap_or(car_pos + a);
                    a.y = ceiling(w, car_pos, a.y).min(ceiling(w, old_eye, a.y));
                }
                self.camera.position = car_pos + a;
                // the slot's offset `+0x08` is that same `A` (0x4619B1…0x4619BA)
                self.cam.chase_offset = Some((a, view));
                if view != 0x08 {
                    self.camera.target = self.camera.position - fwd * 20.0;
                    self.camera.up = car_rot * Vec3::Y;
                } else {
                    self.camera.target = car_pos;
                }
            } else if matches!(view, 0x02..=0x07)
                && let Some(v) = cam_car
            {
                // **The chase cameras 0x461E80** (views 2 / 3 / 4 with argument 0 / 1 / 2, race-logic §11.2):
                // the view's offset less the car's half length, lifted on a slope by the height difference
                // of the slices three behind and three ahead, turned by the car's matrix; the slot's offset
                // follows it by `k` in x and z and 0.05 in y, is kept at least 4.94 long, and the eye is the
                // car plus it, aimed above the car's centre (`aim_lift` below). The build's pushes, which
                // `+0xC0` bit 0x10 (the table's 0x08, set in 0x2D) turns on at 0x4651E4, follow the
                // view branches below.
                // the heli cameras 5 / 6 / 7 run **0x4623D0** with arguments 0 / 1 / 2, the same routine
                // but for: no half-length, the offset pulled back by `min(speed·[0x5B3F6C] 0.05, 2.0)`
                // (0x46256F…0x4625F6), the slope term halved (`[0x5B23F0]`) and clamped [0.25, 1.25] /
                // [−0.75, 3.0], and a 6.32 minimum length in place of 4.94
                let heli = view >= 5;
                let arg = if heli { view - 5 } else { view - 2 };
                // the heli alone: while a replay plays (`[0x5E99F4] >= 2`, playing or run out) the rate
                // is 0.01 whatever the argument (0x462538…0x462541); the chase has no such line
                let replay_heli = heli
                    && self.playback_running();
                let table_off = off;
                let (slices, race_backwards) = (self.slices.as_ref(), self.backwards);
                // what one update reads of its tick's car: the offset before the slope lift, the
                // lift, the follow rate `k`
                let per_tick = |v_local: Vec3, half_z: f32, slice: usize, wrong: bool| -> (Vec3, f32, f32) {
                    let (a, b) = (v_local.z.abs(), v_local.x.abs());
                    let speed = a.max(b) + 0.25 * a.min(b);
                    let mut off = off;
                    if heli {
                        off.z -= (speed * 0.05).min(2.0);
                    } else {
                        off.z -= half_z;
                    }
                    let lift = match (arg, slices) {
                        (1 | 2, Some(t)) if !t.slices.is_empty() => {
                            // the step is +3 when wrong-way XOR REVERSE, else −3 (0x461EE2…0x461EFC)
                            let step: i32 = if wrong != race_backwards { 3 } else { -3 };
                            let y = |d: i32| t.slices[t.add_graph(slice, d, None)].centre.y;
                            let dy = y(step) - y(-step);
                            match (heli, arg) {
                                (false, 1) => (dy * (1.0f32 / 3.0)).clamp(0.25, 1.0),
                                (false, _) => (dy * (1.0f32 / 3.0)).clamp(-0.25, 2.0),
                                (true, 1) => (dy * 0.5).clamp(0.25, 1.25),
                                (true, _) => (dy * 0.5).clamp(-0.75, 3.0),
                            }
                        }
                        _ => 0.0,
                    };
                    // the follow rate: 0.1 for views 2 / 5; else `clamp((max + ¼ min of |v_long|, |v_lat|) / 900,
                    // 0.02, 0.1 or 0.05)` (0x461F7D…0x462008)
                    let k = if replay_heli {
                        0.01
                    } else {
                        match arg {
                            0 => 0.1,
                            1 => (speed * (1.0 / 900.0)).clamp(0.02, 0.1),
                            _ => (speed * (1.0 / 900.0)).clamp(0.02, 0.05),
                        }
                    };
                    (off, lift, k)
                };
                // ENHANCEMENT E48: the lift's side kept while the car's heading is near its slice's perpendicular
                // (`camera::steady_slope_wrong`); camera state only. Off: the original's bit, and no state is kept.
                let wrong = match slices.and_then(|t| t.slices.get(v.slice)) {
                    Some(sl) if arg != 0 && self.enh.is_on(nfs_renderer::E48) => {
                        let along = (v.rot * Vec3::Z).dot(sl.forward);
                        let w = steady_slope_wrong(self.cam.slope_wrong, v.wrong_way, along);
                        if w != v.wrong_way {
                            tracing::trace!(tick = v.tick, along, wrong = v.wrong_way, "slope side held (ENHANCEMENT E48)");
                        }
                        self.cam.slope_wrong = Some(w);
                        w
                    }
                    _ => {
                        self.cam.slope_wrong = None;
                        v.wrong_way
                    }
                };
                let (mut off, lift, k) = per_tick(v.v_local, v.half_z, v.slice, wrong);
                // look-back (bit 0x13) first snaps the slot's offset `+0x08` to the car-turned offset —
                // the chase one with the half length taken off (0x46200C…0x46202A), the heli one the
                // table's (0x462549…0x462567) — so the follow restarts from there on release
                let snap = car_rot * if heli { table_off } else { off };
                // the aim 0x464BF0: forward `(car − eye)·0.25` plus, with `+0xC0` bit 0x02 (the table's
                // 0x01, set in 0x2D), 0.325 (views 2…4, `[0x5B3F84]`) or 0.4 (`[0x5B261C]`) on y before
                // normalising (0x464E6B…0x464ED3): aimed 1.3 / 1.6 above the car's origin
                let aim_lift = Vec3::Y * 4.0 * if heli { 0.4 } else { 0.325 };
                off.y += lift;
                let target = car_rot * off;
                // both branches seed 0x461370 at the car and at the slot's old eye with the offset's y
                // and keep the lower answer (0x462280 / 0x462292 then 0x462297…0x4622B7; the rigid
                // branch 0x4621BB / 0x4621CD)
                let old_eye = self.cam.slot_eye.unwrap_or(car_pos + target);
                let world = self.world.as_ref();
                let lowest = |y: f32| world.map_or(y, |w| ceiling(w, car_pos, y).min(ceiling(w, old_eye, y)));
                let ky = 0.05;
                // SetSlotView (0x465848…0x46588C) resets the settle count but not the offset `+0x08`, so a
                // new view starts from where the last one left the eye and glides in at 0.1 for 0x32 updates
                let mut cur = match self.cam.chase_offset {
                    Some((o, v0)) if v0 == view && !slot_entered => o,
                    Some((o, _)) => {
                        self.cam.chase_settle = 0x32;
                        o
                    }
                    None => {
                        self.cam.chase_settle = 0x32;
                        target
                    }
                };
                if rear_view {
                    // **the rigid branch** 0x462136…0x462235 (heli 0x4626F1…0x4627F0): the eye is the car
                    // plus the car-turned offset with its z negated — ahead of the car — aimed back at its
                    // centre, and the update returns before the follow; the eye's y is the lower of the
                    // two ceiling tests
                    self.cam.chase_offset = Some((snap, view));
                    let mut a = car_rot * Vec3::new(off.x, off.y, -off.z);
                    a.y = lowest(a.y);
                    self.camera.position = car_pos + a;
                    self.camera.target = car_pos + aim_lift;
                    self.camera.up = Vec3::Y;
                } else {
                    // this update's own tick (the body runs once per tick, `steps` is 0 or 1
                    // here); the ceiling test's old eye is the eye the update before left (0x461370 seeds).
                    let oe = self.cam.slot_eye.unwrap_or(car_pos + target);
                    let mut tgt = target;
                    if let Some(w) = world {
                        tgt.y = ceiling_at(w, car_pos, car_pos, tgt.y).min(ceiling_at(w, oe, car_pos, tgt.y));
                    }
                    for _ in 0..steps {
                        let (k, ky) = if self.cam.chase_settle > 0 {
                            self.cam.chase_settle -= 1;
                            (0.1, 0.1)
                        } else {
                            (k, ky)
                        };
                        cur.x += (tgt.x - cur.x) * k;
                        cur.y += (tgt.y - cur.y) * ky;
                        cur.z += (tgt.z - cur.z) * k;
                        // unless the settle count is still running, an offset shorter than the minimum
                        // is stretched to it (0x532B20 / 0x532360): `[0x5B3F58]` 4.94 for the chase
                        // (0x462345…0x46236F), `[0x5B37B0]` 6.32 for the heli (0x462900…0x46292A) —
                        // the length of the HeliCam0 / 1 offset (0, 2, −6)
                        if self.cam.chase_settle == 0 {
                            let min = if heli { 6.32 } else { 4.94 };
                            let len = cur.length();
                            if len > 0.0 && len < min {
                                cur *= min / len;
                            }
                        }
                    }
                    self.cam.chase_offset = Some((cur, view));
                    follow_off = Some(cur);
                    self.camera.position = car_pos + cur;
                    self.camera.target = car_pos + aim_lift;
                    self.camera.up = Vec3::Y;
                }
            } else if chases {
                // **BlimpCam 0x0D**, inline in the dispatcher (0x463F11…0x463FF8): the table offset in world
                // axes — not turned by the car — its y the lower of the two ceiling tests 0x461370 at the
                // car and at the old eye; the eye's offset from the car `D = eye − car` (0x532330) moves
                // toward it by `[0x5B2604]` 0.1 per axis each update and the eye is `car + D` (0x532300).
                // Its flags 0x14 give `+0xC0` 0x28 — the aim bit without the lift — so it looks at the
                // car's origin with the world's up.
                let mut target = off;
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.cam.slot_eye.unwrap_or(car_pos + off);
                    target.y = ceiling(w, car_pos, off.y).min(ceiling(w, old_eye, off.y));
                }
                let mut d = self.cam.slot_eye.map_or(target, |e| e - car_pos);
                for _ in 0..steps {
                    d += (target - d) * 0.1;
                }
                follow_off = Some(d);
                self.camera.position = car_pos + d;
                self.camera.target = car_pos;
                self.camera.up = Vec3::Y;
            } else if rides_car {
                // bumper: the table's offset in the car's own frame, looking along it
                self.camera.position = car_pos + car_rot * off;
                self.camera.target = self.camera.position + car_rot * (look * Vec3::Z) * 20.0;
                // `BumperCam` (view 0, 0x461D50): the slot's matrix is the car's own `car+0x364` copied
                // whole (0x461DAF…0x461DC7), so the view rolls and pitches with the car; this port kept
                // the world's up; 0x4611C0 then turns it by the look-around yaw (`look` above).
                self.camera.up = car_rot * Vec3::Y;
            } else if view == 0x0B
                && let (Some(t), Some(v), Some(tc)) = (self.slices.as_ref(), cam_car, self.track_cameras.as_mut())
                && !t.slices.is_empty()
            {
                // **SplineCam 0x463410** (race-logic §11.5), slot state in `TvSlot`: `+0xCC` the pace,
                // `+0x60` the offset, `+0x30` the lift (3.0 when 0, `[0x5B2600]`).
                let n = t.slices.len();
                let car_slice = v.slice.min(n - 1);
                let lift = if tc.tv.lift != 0.0 { tc.tv.lift } else { 3.0 };
                let mut eye = tc.tv.spline_eye.or(self.cam.slot_eye).unwrap_or(car_pos + Vec3::Y * lift);
                // the watched car's distance (0x5323A0) and its way along the slices: the sign of its
                // forward · the slice's forward, turned round when it rolls backwards (0x463488…0x463501)
                let d = car_pos.distance(eye);
                let mut dir: i32 = if fwd.dot(t.slices[car_slice].forward) < 0.0 { -1 } else { 1 };
                if v.v_local.z < 0.0 {
                    dir = -dir;
                }
                // the eye's tracker (0x474440) and the circular slice gap to the car (0x463537…0x463584)
                let mut eye_slice = t.find_slice(eye, tc.tv.spline_slice.unwrap_or(car_slice));
                let raw = car_slice as i32 - eye_slice as i32;
                let gap = if raw.unsigned_abs() as usize > n / 2 { n as i32 - raw.abs() } else { raw.abs() };
                let pace = tc.tv.pace;
                let k = ((1.0 - pace) * 15.0) as i32;
                if gap > k + 3 && pace < 1.0 && d > 25.0 && !tv_mode {
                    // the jump (0x4635E3…0x4636CE): `min(k + 1, 8)` slices ahead in the car's way, cut at
                    // the first junction 0x4816F0 finds; the eye goes to the centre of the slice that far
                    // less one step, raised by the lift, and the offset `+0x60` is cleared
                    let m = (k + 1).min(8) * dir;
                    let mut e = m;
                    let mut j = if m > 0 { 0 } else { m };
                    while if m > 0 { j < m } else { j <= 0 } {
                        if t.junction_within(car_slice, j).is_some() {
                            e = j;
                            break;
                        }
                        j += 1;
                    }
                    eye_slice = t.add_graph(car_slice, e - dir, None);
                    eye = t.slices[eye_slice].centre + Vec3::Y * lift;
                    tc.tv.spline_offset = Vec3::ZERO;
                }
                // the point it heads for (0x4636EF…0x463941): the next slice in its way — through a
                // junction the last link going that way — clamped to the table, its centre raised by
                // the lift plus `+0x60` along its lateral (× the way) / up / forward; from the eye, or in
                // the TV mode from its own slice's raised centre
                let base = if tv_mode { t.slices[eye_slice].centre + Vec3::Y * lift } else { eye };
                let next = if t.junction_within(eye_slice, dir).is_some() {
                    t.add_graph(eye_slice, dir, None)
                } else {
                    (eye_slice as i32 + dir).clamp(0, n as i32 - 1) as usize
                };
                let sl = &t.slices[next];
                let o = tc.tv.spline_offset;
                let q = sl.centre + Vec3::Y * lift + sl.lateral * (dir as f32 * o.x) + sl.up * o.y + sl.forward * o.z;
                let mut to = q - base;
                if to != Vec3::ZERO {
                    to = to.normalize();
                }
                // the pace (0x46398F…0x463A39): the car's velocity along its slice, × the way, × `+0xCC`,
                // a sixty-fourth a step
                let speed = v.velocity.dot(t.slices[car_slice].forward) * dir as f32 * pace;
                // (per tick: this frame's `steps` updates move it along the one heading)
                eye += to * speed * (1.0 / 64.0) * steps as f32;
                tc.tv.spline_eye = Some(eye);
                tc.tv.spline_slice = Some(eye_slice);
                for _ in 0..steps {
                    tc.tv.update(d);
                }
                self.camera.fov_y_radians = fov_of(tc.tv.half_angle);
                slot_near = tc.tv.near;
                self.camera.position = eye;
                self.camera.target = car_pos;
                self.camera.up = Vec3::Y;
            } else if view == 0x0A {
                // **CircleCam 0x462980** (`camera::CircleCam`): the eye circles the car on a 4.5 × 6.0
                // ellipse in its own frame, a lap per 1024 updates, at the heights 1 / 2 / 3 in turn
                // (0x4629E9…0x462ABC). Turned by the car's matrix, its y bounded by the two ceiling
                // tests, the eye is the car plus it (0x462AC0…0x462B72); flags 0x3C give the aim bit, so
                // it looks at the car's origin. It does not call 0x462B80: the half-angle stays
                // SetSlotView's 37.5. SetSlotView zeroes the count — a new view, or in the TV modes
                // every enabled trackside switch (0x4139DD)
                let switches = if tv_mode { self.track_cameras.as_ref().map_or(0, |t| t.tv.switches) } else { 0 };
                let (mut circle, v0, s0) = self.cam.circle;
                if v0 != view || s0 != switches {
                    circle.reset();
                }
                let local = circle.step_count(steps);
                self.cam.circle = (circle, view, switches);
                let mut off = car_rot * local;
                if let Some(w) = self.world.as_ref() {
                    let old_eye = self.cam.slot_eye.unwrap_or(car_pos + off);
                    off.y = ceiling(w, car_pos, off.y).min(ceiling(w, old_eye, off.y));
                }
                self.camera.position = car_pos + off;
                self.camera.target = car_pos;
                self.camera.up = Vec3::Y;
            } else if view == 0x0E
                && let Some((eye, m)) = anim_pose
            {
                // **AnimCam 0x462F30** (`camera_anim_step`): the eye and the whole matrix from the
                // animation; the table's flags for 0x0E (0x10, reloaded at 0x463D3A…0x463DC4 since
                // 0x4640B0 left `+0xC0` = 0xFF) carry neither the aim nor the pushes, so only the
                // ground clamp below acts on it
                self.camera.position = eye;
                self.camera.target = eye + m.row(2) * 20.0;
                self.camera.up = m.row(1);
            } else if view == nfs_game::camera::VIEW_DRIVER as u16
                && let Some(dc) = self.driver_cam
            {
                // driver (`BumperCam2`): the car's own `drivercampos` / `drivercamrot`, in the same body
                // frame as the table, turned by the file's x / y / z in degrees: 0x5563D0 / 0x5564A0 /
                // 0x556560 each pre-multiply the slot matrix by 0x532600's family `M_x` / `M_y` / `M_z`
                // (0x461B5A…0x461B8E), so the car's matrix ends up `M_z·M_y·M_x·M` — in this port's
                // column form `car · Rx · Ry · Rz`, the roll innermost
                let aim = Quat::from_rotation_x(dc.rotation.x.to_radians())
                    * Quat::from_rotation_y(dc.rotation.y.to_radians())
                    * Quat::from_rotation_z(dc.rotation.z.to_radians());
                // 0x461A30 does not put the eye at the offset outright: the slot's world offset `+0x08`
                // moves toward `car matrix · offset` by `[0x5B23F4]` 0.2 in x and y and `[0x5B23F0]` 0.5 in
                // z each update (0x461BC5…0x461C25), so the head lags the body a little.
                // with look-back held the table offset's x is 0 (0x461B96…0x461B9F)
                let mut pos = dc.position;
                if rear_view {
                    pos.x = 0.0;
                }
                let target = car_rot * pos;
                // **Looking left or right** (bit 0x14 `[0x5E99B4]` or 0x15 `[0x5E99AC]`, tested at
                // 0x461A90 / 0x461A9D) takes the rigid path 0x461C74: the car's matrix, the look-around
                // turn 0x4611C0, the look-back's x = 0 (0x461CD0), and the eye at `car + M·offset`
                // outright (0x461CE4…0x461D40) — none of the file's `drivercamrot` turns, and the
                // slot's lagged offset `+0x08` neither followed nor moved
                let rigid = look_left || look_right;
                let frame = if rigid {
                    self.camera.position = car_pos + target;
                    car_rot * look
                } else {
                    // **SetSlotView of a view outside 2…7 calls 0x4618B0** (0x465997…0x4659D4: `[0x5E99F4] < 2`), which
                    // overwrites the slot's `+0x08` with the view's table offset turned by the car's matrix (0x4619B1…
                    // 0x4619BA) — after the seed 0x465CF0 — so the first update of the driver view lags from its own
                    // target, not from the seed's 4 m behind. PA-SCENES, gdb on the original: the offset reads
                    // (−3.617, 0.270, 1.686) at 0x465790's entry and (0.037, 0.405, 0.405) at the update's entry, the
                    // first cockpit tick 274 shows the settled eye. (The ceiling test on y, 0x461370, is not applied.)
                    let mut cur = self.cam.chase_offset.map_or(target, |(o, _)| o);
                    if view_entered && upd == anim_done {
                        cur = target;
                    }
                    for _ in 0..steps {
                        cur.x += (target.x - cur.x) * 0.2;
                        cur.y += (target.y - cur.y) * 0.2;
                        cur.z += (target.z - cur.z) * 0.5;
                    }
                    self.cam.chase_offset = Some((cur, view));
                    self.camera.position = car_pos + cur;
                    // the look-around yaw turns the car's matrix before the slot's own turns (0x461B07
                    // is ahead of 0x5563D0…0x556560)
                    car_rot * look * aim
                };
                // the view is the slot matrix whole, so its up row carries the turns too (the roll)
                self.camera.target = self.camera.position + frame * Vec3::Z * 20.0;
                self.camera.up = frame * Vec3::Y;
            } else {
                // `TVCam` / `DirectorsCam`: the camera the last camera trigger switched to
                // (0x413B80 → 0x413960), with its own field of view; before any trigger has fired, the
                // nearest element
                let slot_eye = self.cam.slot_eye;
                let cam = self
                    .track_cameras
                    .as_mut()
                    .and_then(|t| {
                        // 0x462E30: the eye rises above where the switch put it by the element's `+0x84`
                        // as the car moves off, `d` measured from last update's eye; then 0x462B80(d)
                        // steps the half-angle (`TvSlot`)
                        let base = t.tv.eye?;
                        let last = slot_eye.filter(|e| e.x == base.x && e.z == base.z).unwrap_or(base);
                        let d = car_pos.distance(last);
                        let lift = t.tv.eye_lift(d);
                        for _ in 0..steps {
                            t.tv.update(d);
                        }
                        Some((base + Vec3::Y * lift, t.tv.half_angle, t.tv.fixed_view, t.tv.frozen, t.tv.near))
                    })
                    .or_else(|| {
                        self.track.cameras.as_ref().and_then(|c| {
                            c.of_kind(&nfs_formats::scn::ElementKind::Camera)
                                .map(|e| Vec3::from(e.position))
                                .min_by(|a, b| {
                                    a.distance_squared(self.car_pos)
                                        .partial_cmp(&b.distance_squared(self.car_pos))
                                        .unwrap()
                                })
                                .map(|p| (p, nfs_game::camera::mode_half_angle(), None, false, nfs_game::camera::DEFAULT_NEAR))
                        })
                    });
                match cam {
                    Some((p, fov, fixed, frozen, near)) if p.distance(car_pos) < TV_REACH => {
                        // view 12's flags 0x14 carry the aim bit, so 0x464BF0 points it at the car's
                        // origin; a camera whose `+0x88` is 0 instead gets the element's own matrix and
                        // loses the aim bit (0x413A4B…0x413A7F)
                        self.camera.position = p;
                        match fixed {
                            Some((f, u)) => {
                                self.camera.target = p + f * 20.0;
                                self.camera.up = u;
                            }
                            // a disabled `+0x88` 0 camera leaves the last matrix in place
                            None if frozen => {
                                let dir = self.camera.target - self.camera.position;
                                self.camera.target = p + dir;
                            }
                            None => {
                                self.camera.target = car_pos;
                                aimed = true;
                            }
                        }
                        self.camera.fov_y_radians = fov_of(fov);
                        slot_near = near;
                    }
                    _ => {
                        self.camera.position = car_pos - fwd * TV_FALLBACK_BACK + Vec3::Y * TV_FALLBACK_UP;
                        self.camera.target = car_pos + Vec3::Y * TV_FALLBACK_AIM;
                    }
                }
            }
            // PORT-DEVIATION D8: the follower's offsets of the previous and of this tick (reset on a view switch, a
            // SetSlotView re-entry, the look-back's rigid branch and anything that is not a follower)
            if let (Some(off), true) = (follow_off, follower_view) {
                let fresh = view_entered && upd == anim_done;
                self.cam.cam_follow = Some(CamFollow::update(self.cam.cam_follow, view, off, (car_pos, car_rot), steps > 0, fresh));
                followed = last_upd;
            } else {
                self.cam.cam_follow = None;
            }
            if !last_upd {
                self.cam.slot_eye = Some(self.camera.position);
            }
        }
        // PORT-DEVIATION D8: the eye the follower updates built is the tick's (`car_tick + offset`, also the slot's
        // eye `+0x24` for the next tick's ceiling tests); the frame draws it at the interpolated car plus the
        // offset blended by `alpha`, the aim point at the interpolated car. At `alpha` 1 (and in the audio slot's
        // pass, which reads the tick's own eye) nothing moves.
        let mut eye_shift = Vec3::ZERO;
        if followed && !audio && let Some(f) = self.cam.cam_follow {
            let want = frame_car_pos + f.draw_offset(alpha);
            eye_shift = want - self.camera.position;
            self.camera.position = want;
            self.camera.target += frame_car_pos - f.pose.0;
        }
        // diagnostic (`RUST_LOG=camslot=debug`): the slot's eye `+0x24` after this frame's updates
        tracing::debug!(target: "camslot", tick = self.tick, view, updates = frame_steps, eye = ?self.camera.position, at = ?self.camera.target, up = ?self.camera.up, car = ?car_pos, "camera slot");
        // the ticks this frame's updates consumed are done with
        while self.cam.cam_hist.front().is_some_and(|h| h.tick <= self.tick) {
            self.cam.cam_hist.pop_front();
        }
        self.camera_build(view, car_pos, aimed, prev_up, eye_shift, audio);
        // (the render's pass alone sets the scene's beacon)
        if !audio {
            self.scene.beacon = self.police_beacon();
        }
        (view, slot_near)
    }


    /// The camera slot's state as it is now (the render's, the `App`'s own fields).
    pub(crate) fn cam_slot_state(&self) -> CamSlotState {
        CamSlotState { camera: self.camera, slot: self.cam.clone(), tv: self.track_cameras.as_ref().map(|t| t.tv) }
    }

    /// Exchanges the slot state the `App` holds with `s` (a pass runs on whichever is in the fields).
    pub(crate) fn swap_cam_slot(&mut self, s: &mut CamSlotState) {
        use std::mem::swap;
        swap(&mut self.camera, &mut s.camera);
        swap(&mut self.cam, &mut s.slot);
        if let Some(tc) = self.track_cameras.as_mut() {
            let theirs = s.tv.unwrap_or(tc.tv);
            s.tv = Some(tc.tv);
            tc.tv = theirs;
        }
    }

    /// **The listener at the audio update's tick.** The original steps the camera slot in the `Sc64` table
    /// (0x463C30, `GameTick` 0x4150C8) on every tick, and the odd tick's audio update 0x4A8120 (0x4150E3)
    /// comes after it in the same tick: its 0x4654A0 (0x4A8150) copies the slot's eye `+0x24` (0x627EA4) into
    /// the listener `[0x655E50]`, so the listener is the slot as that tick's step left it, and the car it
    /// follows is that tick's. The port steps the render's slot once per drawn frame, the frames' ticks one
    /// after the other (`camera_pass`); the audio read the last drawn frame's camera, a frame or more behind.
    /// This runs the same pass on a second copy of the slot ([`App::audio_cam`]), once per audio update, over
    /// the ticks since the last one (the history it keeps, `CamSlotState::cam_hist`, is pushed by `simulate`),
    /// at the tick's own pose (alpha 1, no frame interpolation: the original draws that pose, PORT-DEVIATION
    /// D8) and with no side effect on the render's slot, the scene or the particles' RNG. Returns the listener's
    /// eye and aim point; the render's camera is unchanged. Called inside `simulate`, before the tick count.
    pub(crate) fn audio_listener_pass(&mut self) -> Option<(Vec3, Vec3)> {
        // the pose of the tick just simulated (`simulate` tags it with the count after the tick)
        let h = *self.cam.cam_hist.back().filter(|h| h.tick == self.tick + 1)?;
        self.vehicle.as_ref()?;
        let mut slot = match self.audio_cam.take() {
            Some(s) => s,
            None => Box::new(self.cam_slot_state()),
        };
        let rear_view = self.input.held(Action::RearView) && self.vehicle.is_some();
        self.swap_cam_slot(&mut slot);
        // `camera_pass` counts the ticks done: the update of this tick is the one that brings the count to `tick + 1`
        self.tick += 1;
        self.camera_pass(h.pos, h.rot, rear_view, true, 1.0);
        self.tick -= 1;
        let out = (self.camera.position, self.camera.target);
        self.swap_cam_slot(&mut slot);
        self.audio_cam = Some(slot);
        Some(out)
    }
}
