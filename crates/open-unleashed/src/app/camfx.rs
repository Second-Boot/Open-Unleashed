//! ENHANCEMENTS E27 (motion blur) and E29 (camera shake) on the application's side: what the
//! renderer's camera-effect passes (`nfs-renderer/src/camfx.rs`) are given each frame. **Not in the
//! original, render-only**: this reads the render camera and the viewed car's state (read-only) and
//! writes two fields of the frame's `Scene` (`motion`, `shake`); nothing here is read by the
//! simulation, the AI, the sound, the culling, the network or a replay, and `self.camera` — what all
//! of those read — is never changed. `RE/notes/ENHANCEMENTS.md`.
//!
//! * **E27** needs the previous frame's camera and the time between the two: the sim clock
//!   `(tick + alpha) / 64 s` (D8's interpolation fraction included), so a frame drawn between two
//!   ticks blurs by what it moved and a paused game (the clock stands) draws none. A jump of the
//!   camera (a view change, a reset) draws no blur for that frame.
//! * **E29** is a function of the sim clock and the viewed car's own state: the road roughness the
//!   wheels already carry (`Wheel::rough_*`, the original's `Roughness` 0x499730, 0.03..0.1 m by
//!   surface), the wheels' compression change per tick (landings, kerbs), and the contact word the
//!   collision code leaves (`take_contact`, the sparks' own trigger). The noise is a hash of the sim
//!   clock, so a replay (which re-simulates the same state) shakes the same way every time.

use crate::*;
use glam::Vec2;
use nfs_renderer::MotionPrev;

/// The exposure time the blur's streak covers, by level (light / medium), seconds.
const BLUR_EXPOSURE: [f32; 3] = [0.0, 1.0 / 160.0, 1.0 / 80.0];
/// The shake at amplitude 1 (the render camera's own delta, in its local axes): the angles are the old
/// image-space shake's offset (0.35 % of the screen height) times the field of view, the roll in radians,
/// the position offsets in metres (right, up).
const SHAKE_ANGLE: f32 = 0.0035;
const SHAKE_ROLL: f32 = 0.0022;
const SHAKE_POS: [f32; 2] = [0.015, 0.03];
/// The shake's amplitude by level.
const SHAKE_LEVEL: [f32; 3] = [0.0, 1.0, 1.8];

/// What the effects remember between frames.
#[derive(Default)]
pub(crate) struct CamFxState {
    /// E27: the main view's camera of the frame before the last distinct frame time, and of the last one
    prev: Option<(Camera, f64)>,
    cur: Option<(Camera, f64)>,
    /// E29: the amplitude envelope, the collision kick (set by the sim tick, decays with the clock), the
    /// wheels' compression of the last tick and its change
    env: f32,
    pub(crate) kick: f32,
    comp_prev: [f32; 4],
    comp_tick: u64,
    comp_delta: f32,
    /// the last frame's clock and shake (a frozen clock shows the same)
    t: Option<f64>,
    delta: Option<Delta>,
    /// this frame's shaken view for the renderer and its sun / flare, the cockpit rig and the culling margin
    /// (`None` = no shake: everything is the unshaken camera)
    pub(crate) view: Option<ShakeView>,
}

/// The shake in the camera's own axes: angles (radians) about its up / right / forward and the position offset (m).
#[derive(Clone, Copy, Debug)]
struct Delta {
    yaw: f32,
    pitch: f32,
    roll: f32,
    right: f32,
    up: f32,
}

impl Delta {
    /// The shaken camera, the rig and the culling margin for `cam`.
    fn apply(&self, cam: &Camera) -> ShakeView {
        let dir = cam.target - cam.position;
        let (f, dist) = (dir.normalize_or_zero(), dir.length().max(1.0));
        let r = f.cross(cam.up).normalize_or_zero();
        let u = r.cross(f);
        // yaw about the camera's up, pitch about its right, roll about its forward axis
        let q = (Quat::from_axis_angle(u, self.yaw) * Quat::from_axis_angle(r, self.pitch) * Quat::from_axis_angle(f, self.roll)).normalize();
        let after = cam.position + r * self.right + u * self.up;
        let shaken = Camera { position: after, target: after + q * f * dist, up: q * u, ..*cam };
        // the corners of the picture turn by the roll about the diagonal; the position offset is centimetres
        let margin = self.yaw.abs().max(self.pitch.abs()) + self.roll.abs() * 0.6 + 0.004;
        ShakeView { cam: shaken, rot: q, before: cam.position, after, margin }
    }
}

/// ENHANCEMENT E29: one frame's shake of the RENDER camera (the camera the game reads is never changed).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ShakeView {
    /// the shaken camera the renderer draws from
    pub cam: Camera,
    /// the rotation of the shake in the world and the camera's position before / after: a point rigidly
    /// attached to the camera (the cockpit, the dash) moves to `after + rot * (p - before)`, so it stays
    /// where it was on the screen while the world shakes
    rot: Quat,
    before: Vec3,
    after: Vec3,
    /// extra half-angle (radians) the culling frustum is widened by so the shaken view shows no hole
    pub margin: f32,
}

impl ShakeView {
    /// A point / pose attached to the camera (the cockpit's car), carried with it.
    pub(crate) fn attach(&self, pos: Vec3, rot: Quat) -> (Vec3, Quat) {
        (self.after + self.rot * (pos - self.before), (self.rot * rot).normalize())
    }
    /// A cull frustum's half-angle tangent widened by the margin.
    pub(crate) fn widen(&self, tan: f32) -> f32 {
        (tan.atan() + self.margin).min(1.5).tan()
    }
}

impl CamFxState {
    /// The tangent of a culling half-angle widened for this frame's shake (unchanged without one).
    pub(crate) fn widen(&self, tan: f32) -> f32 {
        self.view.map_or(tan, |v| v.widen(tan))
    }
}

impl CamFxState {
    /// Forget the history (a new race, the clock going backwards).
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Smooth value noise in -1..1 at integer lattice points hashed from `seed`, cubic between them.
fn noise(t: f64, seed: u32) -> f32 {
    let (i, f) = (t.floor(), (t - t.floor()) as f32);
    let h = |k: i64| {
        let mut x = (k as u32).wrapping_mul(0x9e37_79b1) ^ seed.wrapping_mul(0x85eb_ca6b);
        x ^= x >> 15;
        x = x.wrapping_mul(0x2c1b_3c6d);
        x ^= x >> 12;
        x = x.wrapping_mul(0x297a_2d39);
        x ^= x >> 15;
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    };
    let u = f * f * (3.0 - 2.0 * f);
    let (a, b) = (h(i as i64), h(i as i64 + 1));
    a + (b - a) * u
}

/// Two octaves (the second 2.3 times faster, half as strong), kept within -1..1.
fn fbm(t: f64, freq: f64, seed: u32) -> f32 {
    0.67 * noise(t * freq, seed) + 0.33 * noise(t * freq * 2.3, seed ^ 0x55)
}

impl App {
    /// A frame's camera effects, after the camera slot has built `self.camera` (the unshaken camera the
    /// game reads). `car_pos` / `car_rot` are the viewed car's drawn pose, `alpha` this frame's
    /// interpolation fraction, `view` the camera slot's view.
    pub(crate) fn camfx_frame(&mut self, car_pos: Vec3, car_rot: Quat, alpha: f32, view: u16, aspect: f32) {
        self.scene.motion = None;
        self.camfx.view = None;
        if self.vehicle.is_none() {
            self.camfx.reset();
            return;
        }
        let blur = self.enh.level(nfs_renderer::E27);
        let shake_level = self.enh.level(nfs_renderer::E29);
        if blur == 0 && shake_level == 0 {
            self.camfx.reset();
            return;
        }
        let t = (self.tick as f64 + alpha as f64) / 64.0;
        let dt = match self.camfx.t {
            Some(p) if t >= p => t - p,
            Some(_) => {
                // the clock went backwards (a replay seek, a restart): start over
                self.camfx.reset();
                0.0
            }
            None => 0.0,
        };
        // ---- E29: the render camera's shake (first: the blur reprojects the camera that is drawn)
        if shake_level > 0 && matches!(view, 0x00..=0x07 | 0x0D) {
            if dt > 0.0 {
                self.camfx.delta = self.camfx_shake(t, dt, view, shake_level);
            }
            self.camfx.view = self.camfx.delta.map(|d| d.apply(&self.camera));
        } else {
            self.camfx.env = 0.0;
            self.camfx.delta = None;
        }
        let render_cam = self.render_camera();
        // ---- E27: the previous distinct frame
        if blur > 0 {
            let cam = render_cam;
            match self.camfx.cur {
                Some((_, tc)) if t > tc + 1e-6 => {
                    self.camfx.prev = self.camfx.cur;
                    self.camfx.cur = Some((cam, t));
                }
                None => self.camfx.cur = Some((cam, t)),
                // the same frame time again (the scene rebuilt): the history stays
                Some(_) => self.camfx.cur = Some((cam, t)),
            }
            if let (Some((pc, pt)), Some((_, ct))) = (self.camfx.prev, self.camfx.cur) {
                let span = ct - pt;
                let moved = (cam.position - pc.position).length() as f64;
                let fwd = |c: &Camera| (c.target - c.position).normalize_or_zero();
                let turned = fwd(&cam).dot(fwd(&pc)).clamp(-1.0, 1.0).acos();
                // a cut (a view change, a reset): further than any car goes in the span, or turned by 25 degrees
                if span > 1e-4 && span < 0.2 && moved < 1.0 + 150.0 * span && turned < 0.44 {
                    let shutter = (BLUR_EXPOSURE[blur.min(2) as usize] as f64 / span).min(1.0) as f32;
                    let keep = self.camfx_car_keep(&cam, car_pos, car_rot, view, aspect);
                    tracing::debug!(target: "camfx", moved, span, shutter, speed = moved / span, "motion blur frame");
                    self.scene.motion = Some(MotionPrev { camera: pc, shutter, keep });
                }
            }
        } else {
            self.camfx.prev = None;
            self.camfx.cur = None;
        }
        if dt > 0.0 || self.camfx.t.is_none() {
            self.camfx.t = Some(t);
        }
    }

    /// The camera the renderer draws from, the sun / lens flare are laid out on and the blur reprojects: the
    /// shaken one with E29 on and a shake running, otherwise `self.camera` itself. **`self.camera` - what the
    /// sound, the AI, the LOD pick and the camera slot read - is never the shaken one.**
    pub(crate) fn render_camera(&self) -> Camera {
        self.camfx.view.filter(|_| self.vehicle.is_some()).map_or(self.camera, |v| v.cam)
    }

    /// The player's car on the screen as an ellipse `[cx, cy, rx, ry]` in uv units (chase views only: the
    /// bumper and driver views have no car in the picture; the near-depth fade covers the hood and cabin).
    fn camfx_car_keep(&self, cam: &Camera, car_pos: Vec3, car_rot: Quat, view: u16, aspect: f32) -> [f32; 4] {
        if view <= 1 {
            return [0.0; 4];
        }
        let Some(h) = self.vehicle.as_ref().map(|v| v.half_extents) else { return [0.0; 4] };
        let vp = cam.view_projection_mirrored(aspect, self.scene.mirror);
        let (mut lo, mut hi, mut n) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN), 0);
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    let p = car_pos + car_rot * Vec3::new(sx * h.x, sy * h.y, sz * h.z);
                    let c = vp * p.extend(1.0);
                    if c.w > 0.05 {
                        let uv = Vec2::new(c.x / c.w * 0.5 + 0.5, 0.5 - c.y / c.w * 0.5);
                        lo = lo.min(uv);
                        hi = hi.max(uv);
                        n += 1;
                    }
                }
            }
        }
        if n < 4 {
            return [0.0; 4];
        }
        let (c, r) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
        [c.x, c.y, (r.x * 1.25 + 0.01).max(0.02), (r.y * 1.25 + 0.01).max(0.02)]
    }

    /// The shake of this frame from the viewed car's own state, `dt` seconds of sim clock after the last.
    fn camfx_shake(&mut self, t: f64, dt: f64, view: u16, level: u8) -> Option<Delta> {
        let seat = Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view);
        let v = match seat {
            Some(k) => self.road.opponents.get(k).map(|o| &o.vehicle),
            None => self.vehicle.as_ref(),
        }?;
        // per tick: how much the wheels' compression changed (landings, kerbs, bumps)
        if self.tick != self.camfx.comp_tick {
            let now: [f32; 4] = std::array::from_fn(|k| v.wheels[k].compression);
            self.camfx.comp_delta = (0..4).map(|k| (now[k] - self.camfx.comp_prev[k]).abs()).sum();
            self.camfx.comp_prev = now;
            self.camfx.comp_tick = self.tick;
        }
        // the road's roughness the wheels carry (0.1 m on the rough surfaces, 0.03 on the kerb-like ones)
        let (mut rough, mut on) = (0.0, 0);
        for w in &v.wheels {
            if w.contact && !w.airborne {
                rough += (w.rough_prev.abs().max(w.rough_target.abs()) / 0.1).min(1.0).sqrt();
                on += 1;
            }
        }
        let rough = if on > 0 { rough / on as f32 } else { 0.0 };
        let speed = (v.velocity.length() / 20.0).clamp(0.0, 1.0);
        // the ordinary body motion of driving is below 0.25 of the scale; landings and kerbs reach it
        let comp = ((self.camfx.comp_delta / 0.08 - 0.25) / 0.75).clamp(0.0, 1.0);
        let kick = {
            self.camfx.kick *= (-(dt / 0.30)).exp() as f32;
            self.camfx.kick
        };
        let target = (rough * speed * 0.8 + comp * 0.5 * speed.max(0.3) + kick).min(1.5);
        // fast to rise, slow to fall: the edges-hiding zoom follows the same envelope, so it never pops
        let k = if target > self.camfx.env { 1.0 - (-(dt / 0.04)).exp() } else { 1.0 - (-(dt / 0.25)).exp() } as f32;
        self.camfx.env += (target - self.camfx.env) * k;
        // the bumper (view 0) and the driver (1) feel the road most, the chase cameras less
        let amp = self.camfx.env * SHAKE_LEVEL[level.min(2) as usize] * if view <= 1 { 1.0 } else { 0.5 };
        tracing::trace!(target: "camfx", tick = self.tick, view, amp, rough, comp, kick, "shake");
        if amp < 0.002 {
            return None;
        }
        let (dx, dy, dr) = (fbm(t, 24.0, 1), fbm(t, 27.0, 2), fbm(t, 19.0, 3));
        // the body's own motion at other frequencies: the camera moves a little as well as turns (parallax)
        let (px, py) = (fbm(t, 21.0, 4), fbm(t, 26.0, 5));
        let fov = self.camera.fov_y_radians;
        Some(Delta {
            yaw: dx * 0.45 * SHAKE_ANGLE * fov * amp,
            pitch: dy * SHAKE_ANGLE * fov * amp,
            roll: dr * SHAKE_ROLL * amp,
            right: px * SHAKE_POS[0] * amp,
            up: py * SHAKE_POS[1] * amp,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ENHANCEMENT E29: the shake's noise is a function of the clock alone (the same in a replay), bounded, and smooth.
    #[test]
    fn the_noise_is_deterministic_bounded_and_smooth() {
        let mut prev = fbm(0.0, 24.0, 1);
        for i in 1..2000 {
            let t = i as f64 / 640.0;
            let v = fbm(t, 24.0, 1);
            assert_eq!(v, fbm(t, 24.0, 1));
            assert!(v.abs() <= 1.0 + 1e-6, "{v}");
            // 27 samples per lattice cell at 24 Hz: neighbouring samples differ by a fraction of the range
            assert!((v - prev).abs() < 0.5, "{prev} -> {v}");
            prev = v;
        }
        assert_ne!(fbm(0.37, 24.0, 1), fbm(0.37, 27.0, 2));
    }
}
