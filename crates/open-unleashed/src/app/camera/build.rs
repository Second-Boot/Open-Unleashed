//! The camera build 0x464F80 after the slot's updates (`App::camera_build`) and the police beacon's projector. Moved out of
//! `render_scene.rs` unchanged.

use crate::*;

/// The police beacon's projector 0x4526C0: the nearest lit cop within this octagonal distance (m) of the camera (`0x43340000`,
/// 0x4527AF), its frame turned by this many turns a 128 Hz `TIMER` tick (`[0x5B3714]`).
const BEACON_REACH: f32 = 180.0;
const BEACON_TURNS_PER_TIMER_TICK: f32 = 0.02;

/// **The ceiling test 0x461370(slot, p, &y)**: `y` is an eye height over the car's y. Over the
/// ceiling patches at `p` in plan (0x473420 from `(p.x, car.y, p.z)`), skipping one whose corners
/// are all within 1.0 of both the eye `car.y + y` and `p.y` or below (0x461408…0x461450) and one
/// whose centre (vt+4, the corners' mean) is within 2.0 of the car's y (0x461462…0x461494): when
/// the plane's height `h` at `p` less 1.0 is below the eye, `y = max(h − car.y − 1, 1.0)`
/// (0x4614B6…0x4614F9) — the last such patch wins; the eye it compares with stays the first.
pub(super) fn ceiling_at(w: &nfs_game::world::TrackWorld, p: Vec3, car_pos: Vec3, y: f32) -> f32 {
    let eye = car_pos.y + y;
    let mut out = y;
    for (t, h) in w.collision.patches_where(p, |word| word & 0x8000 != 0 && word & 0xF == 0) {
        if [t.a, t.b, t.c].iter().all(|v| v.y - 1.0 <= eye && v.y - 1.0 <= p.y) {
            continue;
        }
        let c = (t.a + t.b + t.c) * (1.0 / 3.0);
        if c.y >= car_pos.y - 2.0 && c.y <= car_pos.y + 2.0 {
            continue;
        }
        if h - 1.0 < eye {
            out = (h - car_pos.y - 1.0).max(1.0);
        }
    }
    out
}

impl App {
    /// The camera build 0x464F80 after the slot's updates: the pause orbit, the slot's eye, the pushes, the ground clamp, the
    /// replay's wander and the aim, in that order (each reads what the one before wrote).
    pub(crate) fn camera_build(&mut self, view: u16, car_pos: Vec3, aimed: bool, prev_up: Vec3, mut eye_shift: Vec3, audio: bool) {
        // the build's ceiling test runs at the frame's pose
        let ceiling = |w: &nfs_game::world::TrackWorld, p: Vec3, y: f32| -> f32 { ceiling_at(w, p, car_pos, y) };
        // **The ground clamp 0x464330**, which the build 0x464F80 runs on the eye for views above 1
        // other than 0x0C (0x46524C…0x465261), before it aims: of the drivable patches under the eye
        // (0x473650), one whose three corners are all more than `[0x5B24A8]` 1.0 below the eye is
        // passed over (0x464397…0x4643C2); otherwise when its height `h` at the eye (0x48A060) plus
        // 1.0 is above the eye, the eye is lifted to `h + 1` — but only when `h` is within
        // `[0x5B38EC]` 3.5 of the car's ground height `car+0x41C`, unless `[0x5CF584]` is 0, which
        // 0x4642CC sets from `strcmp(track, "castle")`: on Castle any patch lifts it.
        // **The pause orbit** (the dispatcher 0x463C66…0x463C99): while the HUD's dialog queue is
        // up (0x426A60 — the pause menu, a page) outside a replay (`[0x5E99F4] < 2`), the slot runs
        // 0x4617C0 in place of its view's update — except in views 0, 1 and 0x0E, which keep their
        // eye: the yaw `+0xC6` (a word) steps by `+0xC8` (0x46181E) on every update, and the eye
        // is the car plus `M_y(yaw / 1024 turn) · +0x08` (0x461838…0x46189E), the slot's offset
        // from the car as its view left it. The steps are the keys' (`orbit`: +3, −3 or 0) and
        // the update runs once a frame while paused (0x415C69). The aim that follows (0x464BF0)
        // looks at the car again, so an aimed view's target turns with the eye; the other views'
        // matrix is left as it was. `+0x08` (slot `+0x08` = 0x627E88) is the slot's world offset car →
        // eye that the view updates keep — 0x461590 (chase) writes `[+0x24] = car+0x330 + [+0x08]`
        // (0x4616D3…0x461708); the sibling view updates 0x461A30 / 0x461E80 / 0x4623D0 take its address
        // too, and SetSlotView's 0x465CF0 seeds it — so the eye minus the car before the build's pushes
        // is that offset (CONFIRMED 2026-09-30, for the chase-type views). A view whose update does not
        // write it (CircleCam, SplineCam, TVCam, BlimpCam, 0x10…0x13) leaves in the slot what the last writer or
        // the seed put there: `chase_offset`, which the seed 0x465CF0 (see the SetSlotView note above) fills with
        // `-4 · car forward` on leaving a view outside 2…7 (CLOSED 2026-10-02, WP-R) — the orbit turns that, not the
        // eye's own offset.
        // (`cmp [0x5E99F4], 2 ; jge`, 0x463C75: a stream run out is a replay too)
        let replaying = self.playback_running();
        if self.vehicle.is_some() && self.hud_dialog_up() && !replaying && !matches!(view, 0x00 | 0x01 | 0x0E) {
            let base = *self.cam.orbit.base.get_or_insert(self.cam.chase_offset.map_or(self.camera.position - car_pos, |(o, _)| o));
            self.cam.orbit.yaw = self.cam.orbit.yaw.wrapping_add(self.cam.orbit.step);
            let turn = Quat::from_rotation_y(self.cam.orbit.yaw as f32 / 1024.0 * std::f32::consts::TAU);
            let eye = car_pos + turn * base;
            eye_shift = Vec3::ZERO;
            if aimed {
                self.camera.target = car_pos + turn * (self.camera.target - car_pos);
            } else {
                self.camera.target += eye - self.camera.position;
            }
            self.camera.position = eye;
        } else {
            self.cam.orbit.base = None;
        }
        // the slot keeps its own eye `+0x24`; the pushes and the clamp below act on the copy the build
        // hands out (0x4651BF…0x4651DB)
        self.cam.slot_eye = Some(self.camera.position - eye_shift);
        // **The build's pushes** (0x4651E4…0x465233), for views whose slot flags carry 0x10 — the
        // table's 0x08: views 2…0x0A, 0x10 and 0x11 (`race-logic.md` §11): the prop push 0x4649E0,
        // then the wall push 0x464500
        // (`WallSet::camera_push`), and when that moved the eye, the ceiling test 0x461370 at the eye
        // on its height over the car
        // `[0x5CF584]` = `strcmp(track, "castle")` (0x4642CC): the wall push's and the ground clamp's Castle rule below
        let castle = self.track.name.eq_ignore_ascii_case(nfs_game::track_id::TrackId::Castle.name());
        if matches!(view, 0x02..=0x0A | 0x10 | 0x11)
            && let (Some(w), Some(v)) = (self.world.as_ref(), self.vehicle.as_ref())
        {
            let mut eye = self.camera.position;
            // **0x4649E0, the props**: each tree object near the segment car → eye (0x4739E0) whose
            // radius (slot 5) is 2.0 or more and whose slot-8 answer to the car's velocity is not
            // 3 is cut by the segment (slot 7 — only boxes pass the radius here, the cylinders'
            // being 0.25: 0x486FB0, `box_edge_test`); a cut puts the eye's x and z at the cut point
            // plus the horizontal part of the unit `car − eye` (0x464AD4…0x464B58). The answer is
            // the collision's own, so a crate stack in the speed band starts here as it would there.
            // The tree query 0x4739E0 walks the quadtree cells that overlap the segment's x-z box
            // (0x4848B0 on the min / max of its ends) and keeps each object whose own slot 7 cuts
            // the segment (0x473AB6) — the same cut test as below, so an x-z box test over every prop
            // gives the same set: the tree's insert **0x484AE0** descends into every child that
            // **0x484320** accepts — the object's centre (vtable +4) inside the cell, or else its
            // box (vtable +8) overlapping it — so a prop sits in every cell its box covers
            // (read 2026-09-25; it had been left open whether only the centre's cell held it).
            // Every object is cut by the segment from the car to the eye **as the push found it**:
            // 0x4649E0 copies the eye into a local on entry (0x4649EA…0x4649FC) and hands that copy
            // to the query and to each slot-7 test (0x464AB3…0x464AC9), and takes the direction from
            // it (0x532330 at 0x464AE5: car − entry eye); only the output is written per cut. So a
            // prop listed twice (the tree keeps it in every cell it covers) pushes to the same place
            // twice, and of several props that cut, the last one listed wins — the tree's cell
            // order there, this port's now-real tree query order here (wired 2026-09-28,
            // `RE/RENDERING.md` "The tree, built"). The query also keeps only cell entries whose
            // object byte `+4` is 0 (0x473A95; the constructor 0x486340 writes 0 at 0x486389, no
            // other writer traced) — every `Prop` this port builds qualifies (nothing else writes
            // that byte here either), so no extra filter is needed.
            let e0 = eye;
            for item in w.tree.query_segment((e0.x, e0.z), (v.position.x, v.position.z)) {
                let nfs_physics::track_tree::TreeItem::Prop(pi) = item else { continue };
                let p = &w.props[pi];
                let nfs_physics::props::PropShape::Box { corners, radius } = &p.shape else { continue };
                if *radius < 2.0 || p.answer(self.tick as u32, v.velocity) == 3 {
                    continue;
                }
                let (count, point, _) = nfs_physics::props::box_edge_test(corners, v.position, e0);
                if count == 0 {
                    continue;
                }
                let d = (v.position - e0).normalize_or_zero();
                eye.x = point.x + d.x;
                eye.z = point.z + d.z;
            }
            if w.walls.camera_push(&mut eye, v.position, v.ground_h, castle, &w.tree) {
                eye.y = v.position.y + ceiling(w, eye, eye.y - v.position.y);
                // the target stays where the view put it: the aim 0x464BF0 looks from the moved eye
            }
            self.camera.position = eye;
        }
        if view > 1
            && view != 0x0C
            && let (Some(w), Some(v)) = (self.world.as_ref(), self.vehicle.as_ref())
        {
            let any_height = castle;
            let mut eye = self.camera.position;
            for (t, h) in w.collision.patches_at(eye) {
                if t.a.y.max(t.b.y).max(t.c.y) + 1.0 < eye.y {
                    continue;
                }
                if h + 1.0 > eye.y && (any_height || (h - v.ground_h).abs() <= 3.5) {
                    eye.y = h + 1.0;
                }
            }
            self.camera.position = eye;
        }
        // **The playback's wander** (0x464C2B…0x464E42, `camera::ReplayWander`): while a replay
        // plays (`[0x5E99F4] == 2`, not run out) an aimed view whose word `+0xBC` is not 2…7,
        // 0x0A or 0x0B looks at the car's origin plus the wandering offset — the trackside
        // cameras, 8 and 0x0D. The word is what SetSlotView last set: the trackside element's
        // view, or 3 for `DirectorsCam` (0x14) before any trigger (0x414953 → 0x4659E0(0, 3)).
        {
            let word = match (self.camera_view(), self.track_cameras.as_ref().and_then(|t| t.tv.view)) {
                (0x0C | 0x14, Some(v)) => v,
                (0x14, None) => 3,
                (v, _) => v,
            };
            let key = (self.camera_view(), word, self.track_cameras.as_ref().map_or(0, |t| t.tv.switches).wrapping_add(self.slot_key_enters));
            if self.cam.replay_wander.1 != key {
                self.cam.replay_wander = (nfs_game::camera::ReplayWander::default(), key);
            }
            let plays = self.replay.stream.as_ref().is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing);
            // (the audio slot's pass leaves the wander out: it draws on the particles' RNG, which the render's
            // pass alone may advance, and the listener's position, which the audio reads, does not move with it)
            if aimed
                && !audio
                && plays
                && nfs_game::camera::ReplayWander::applies(word)
                && let (Some(p), Some(v)) = (self.track_particles.as_mut(), self.vehicle.as_ref())
            {
                // the pause flags `[0x606860]` / `[0x606A98]` (the replay's) and `[0x606A90]`
                let paused = self.paused || self.replay.frozen;
                let off = self.cam.replay_wander.0.step(self.tick as i32, v.horizontal_speed(), self.replay.speed.camera_lag(), paused, p.rng_mut());
                self.camera.target += off;
                tracing::debug!(tick = self.tick, word, ?off, goal = ?self.cam.replay_wander.0.goal, "replay aim wander (0x464C2B)");
            }
        }
        // **The aim 0x464BF0** (`+0xC0 & 0x08`, 0x465269…0x46527F), last in the build: the view
        // looks from the eye at its aim point with the world's up, or — within 0.99 of straight
        // down or up — with last build's up row (`camera::aim_up`)
        if aimed {
            let f = (self.camera.target - self.camera.position).normalize_or_zero();
            if f != Vec3::ZERO {
                self.camera.up = nfs_game::camera::aim_up(f, prev_up);
            }
        }
        // diagnostic (`RUST_LOG=camslot=debug`): the view the build hands out — the orbit, the pushes,
        // the clamp and the aim applied
        tracing::debug!(target: "camslot", tick = self.tick, view, eye = ?self.camera.position, at = ?self.camera.target, up = ?self.camera.up, dialog = self.hud_dialog_up(), "camera built");
    }

    /// The police beacon's projector, world into its frame (see below), or `None`.
    pub(crate) fn police_beacon(&self) -> Option<Mat4> {
        // **The police beacon's projector** 0x4526C0's second half: on a night track
        // `max(|dx|,|dz|) + 0.25·min(|dx|,|dz|)` (0x4527DB…0x45284D), is smallest — starting from
        // **180** (`0x43340000`, 0x4527AF), so a farther one lights nothing. Its frame is the car's
        // own (`car+0x330` / `car+0x364`) turned about the car's **y** by `TIMER_getTicks() × 0.02`
        // **turns** (`[0x5B3714]`, through 0x5564A0 whose sine takes turns, `[0x5BB2C0]` = 2π), and
        // 0x539CD0 stores world → that frame in `[0x61E184]`. The 128 Hz TIMER is two of this
        // port's 64 Hz game ticks.
        self.track.is_night().then(|| {
            let cam = self.camera.position;
            let pol = self.road.police.as_ref()?;
            let (car, _) = pol
                .cars
                .iter()
                .filter(|c| c.active && c.lights)
                .filter_map(|c| {
                    let (dx, dz) = ((c.position.x - cam.x).abs(), (c.position.z - cam.z).abs());
                    let d = dx.max(dz) + 0.25 * dx.min(dz);
                    (d < BEACON_REACH).then_some((c, d))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())?;
            let turns = (self.tick * 2) as f32 * BEACON_TURNS_PER_TIMER_TICK;
            let rot = Quat::from_mat3(&car.orientation) * Quat::from_rotation_y(turns * std::f32::consts::TAU);
            Some(Mat4::from_rotation_translation(rot, car.position).inverse())
        }).flatten()
    }

}
