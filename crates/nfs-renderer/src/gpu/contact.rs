//! ENHANCEMENT E43: the soft contact shadow under the cars.
//!
//! The original has one shadow under a car, the `sh80` blob (a projector texture, 0x446130) and the car's `SHAD` ground patch
//! (0x43B820). E9's sun shadow replaces them where it is drawn (a sun, in the open); under cover, at night, under E25's overcast
//! light and with E9 off the blob stays, and at a tunnel mouth the soft shadow vanished and the old blob appeared. This is the
//! shadow for those places: an ambient-occlusion-like darkening of the road under and round each near car, evaluated analytically
//! per fragment of the track (`shader.wgsl`, `contact_occlusion`), from the car's pose alone.
//!
//! The model (per car, in the car's own frame, so it follows slopes and banked roads and a car in the air):
//! * the body is a rounded rectangle of the car's box (`half` x, z) hovering `ride` (half a tyre radius) over the road: the road
//!   under it and beside it is occluded by `k(gap) * profile(distance outside the footprint)`, where `gap` = the receiver's
//!   clearance under the body's underside (the road's height over the car's ground plane, so a jump or a ramp thins it out) and the
//!   profile is a short exponential plus a wider tail whose length grows with the gap;
//! * each of the four tyres adds a tighter patch under the tyre and a halo round it (the crease where the rubber meets the road);
//! * the receiver must face up in the car's frame (a wall or a kerb face beside the car is not darkened).
//! The result multiplies the light of the track item (like E9's track receiving: the baked colour has no light term to split), and
//! the `sh80` blob fades out in the shader where a car's contact shadow stands (the footprint's `presence`), and the app does not lay the car's
//! `SHAD` patch at all while the car has a contact shadow (`render_scene.rs`), the way both fade under E9's cover gate.
//!
//! Where E9's sun shadow is drawn the contact shadow is weaker (`OPEN_SHARE`: a sunlit car already has a cast shadow, the contact
//! only grounds it), and it grows to full as E9's cover gate falls: a tunnel mouth crossfades with no pop. Cost: one uniform
//! (16 cars x 6 vec4), a loop with an early-out in the fragment shader of the track's items; no pass, no texture.

use super::*;
use bytemuck::Zeroable;

/// How strongly the contact shadow darkens at its darkest (under the body, at the tyres), 0..1.
pub(super) const STRENGTH: f32 = 0.9;
/// The share of the contact shadow that stays where E9's sun shadow is drawn in full (in the open, sunlit).
pub(super) const OPEN_SHARE: f32 = 0.55;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ContactUniform {
    /// per car: xyz the model origin on the road, w the half width
    pos: [[f32; 4]; crate::MAX_CONTACTS],
    /// xyz the body's right axis, w the half length
    ax: [[f32; 4]; crate::MAX_CONTACTS],
    /// xyz the up axis, w the tyre radius
    ay: [[f32; 4]; crate::MAX_CONTACTS],
    /// xyz the forward axis, w unused
    az: [[f32; 4]; crate::MAX_CONTACTS],
    /// the tyres' patch centres: (x0, z0, x1, z1) of the front pair, then the rear pair
    wheels: [[f32; 4]; crate::MAX_CONTACTS * 2],
    /// x the car count (0 = off), y the strength
    params: [f32; 4],
}

pub(super) struct ContactState {
    pub uniform: wgpu::Buffer,
    active: bool,
}

impl ContactState {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("contact uniform"), size: std::mem::size_of::<ContactUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true });
        // zeroed: car count 0, the shader's "off"
        uniform.unmap();
        Self { uniform, active: false }
    }
}

pub(super) fn layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 24,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

impl Core {
    /// Whether this frame has contact shadows (E43 on, some car, not the rear-view mirror / an offscreen view).
    pub(super) fn contact_on(&self, scene: &Scene) -> bool {
        self.enhancements.is_on(crate::E43) && !self.no_reflections && scene.contacts.n > 0
    }

    /// Write this frame's uniform; without cars it is zeroed once if it was on. `sun_share` is how much of the car's shadow E9's sun
    /// shadow carries this frame (0 = none drawn: night, cover, E9 off, overcast; 1 = in the open): the contact shadow weakens to
    /// [`OPEN_SHARE`] as it grows.
    pub(super) fn contact_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, sun_share: f32) {
        let size = wgpu::BufferSize::new(std::mem::size_of::<ContactUniform>() as u64).expect("contact size");
        if !self.contact_on(scene) {
            if self.contact.active {
                self.contact.active = false;
                self.belt.write_buffer(encoder, &self.contact.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&ContactUniform::zeroed()));
            }
            return;
        }
        let c = &scene.contacts;
        let mut u = ContactUniform::zeroed();
        for (i, car) in c.cars.iter().take(c.n as usize).enumerate() {
            u.pos[i] = [car.origin.x, car.origin.y, car.origin.z, car.half[0]];
            u.ax[i] = [car.axes[0].x, car.axes[0].y, car.axes[0].z, car.half[1]];
            u.ay[i] = [car.axes[1].x, car.axes[1].y, car.axes[1].z, car.wheel_r];
            u.az[i] = [car.axes[2].x, car.axes[2].y, car.axes[2].z, 0.0];
            u.wheels[2 * i] = [car.wheels[0][0], car.wheels[0][1], car.wheels[1][0], car.wheels[1][1]];
            u.wheels[2 * i + 1] = [car.wheels[2][0], car.wheels[2][1], car.wheels[3][0], car.wheels[3][1]];
        }
        u.params = [c.n as f32, STRENGTH * (1.0 - sun_share.clamp(0.0, 1.0) * (1.0 - OPEN_SHARE)), 0.0, 0.0];
        self.contact.active = true;
        self.belt.write_buffer(encoder, &self.contact.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&u));
    }

    /// Whether an item is a receiver of the contact shadow: a track item the E9 receiving also takes (drivable ground, walls ...; the
    /// shader keeps only faces that look up in the car's frame), never a front-end room's piece.
    pub(super) fn contact_receive(&self, item: &crate::DrawItem) -> bool {
        item.csm & crate::CSM_ROOM == 0 && item.layer_light.is_none_or(|l| l.direction == [0.0; 3]) && self.csm_receive(item) > 1.5
    }
}
