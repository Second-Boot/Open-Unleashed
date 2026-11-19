//! ENHANCEMENT E40: raindrops on the windscreen of the driver view.
//!
//! What the renderer needs (`nfs_renderer::Drops`): the glass's four corners in the world (the car's `Wndo` points the
//! original builds the splat plane from, 0x457920, through the pose the cockpit's own items use — so under E29's shake the
//! glass moves with the cabin), the rain's wetness (the sky `.bin` track B field 5 at the camera's slice, the value E26 and E36
//! read), how far inside a covered stretch the car is (E24's held, ramped value, smoothstepped), the car's speed and the
//! sim clock `(tick + alpha) / 64 s` (the render clock: a paused game's stands, a replay's is the replay's). Nothing here
//! reads the simulation's RNG or writes anything but `scene.drops`; the water itself lives in the renderer.

use super::*;

impl App {
    /// ENHANCEMENT E40: this frame's `scene.drops` (it is `None` already: `rebuild_scene` clears it). Called from the car's
    /// draw, right after the original's own drops (`splat_items`), with the car's drawn pose.
    pub(crate) fn drops_items(&mut self, car_rot: Quat, car_draw: Vec3, alpha: f32) {
        if !self.enh.is_on(nfs_renderer::E40) || !self.enh.is_on(nfs_renderer::E7) || self.camera_view() != 1 || self.photo.active {
            return;
        }
        let Some(v) = self.vehicle.as_ref() else { return };
        let speed = v.velocity.length();
        let wet = self.camera_wet().clamp(0.0, 1.0);
        let t = (self.tick as f64 + alpha as f64) / 64.0;
        let cover = self.tunnel_cover_smooth(super::car_lights::cover_key::DROPS, self.camera.position, t * 1000.0);
        // a dry track: nothing on the glass; after the rain the renderer is kept running a few seconds so the wipe clears it
        if wet > 0.0 {
            self.ambient.drops_until = t + 4.0;
        } else if t > self.ambient.drops_until {
            return;
        }
        let Some(quad) = self.windscreen_quad_cached() else {
            tracing::debug!(target: "drops", "no windscreen corners");
            return;
        };
        let m = Mat4::from_rotation_translation(car_rot, car_draw) * nfs_game::world::MODEL_TO_BODY;
        tracing::debug!(target: "drops", wet, cover, speed, t, ?quad, "windscreen drops (ENHANCEMENT E40)");
        self.scene.drops = Some(nfs_renderer::Drops { corners: quad.map(|p| m.transform_point3(p)), wet, cover, speed, time: t, up: Vec3::Y });
    }

    /// The `Wndo` corners, parsed once per car (the model and its style).
    fn windscreen_quad_cached(&mut self) -> Option<[Vec3; 4]> {
        let key = (self.car.model.clone(), self.car.style_index, self.cheat_bits & (1 << 1) != 0);
        if let Some((k, q)) = &self.ambient.drops_quad
            && *k == key
        {
            return *q;
        }
        let q = self.windscreen_quad();
        self.ambient.drops_quad = Some((key, q));
        q
    }
}
