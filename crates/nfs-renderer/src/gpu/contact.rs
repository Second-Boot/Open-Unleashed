//! ENHANCEMENT E43: the soft contact shadow under the cars.
//!
//! The original has one shadow under a car, the `sh80` blob (a projector texture, 0x446130) and the car's `SHAD` ground patch
//! (0x43B820). E9's sun shadow replaces them where it is drawn (a sun, in the open); under cover, at night, under E25's overcast
//! light and with E9 off the blob stays, and at a tunnel mouth the soft shadow vanished and the old blob appeared. This is the
//! shadow for those places: an ambient-occlusion-like darkening of the road under and round each near car, evaluated analytically
//! per fragment of the track (`shader.wgsl`, `contact_keep`), from the car's pose alone.
//!
//! The model (per car, in the car's own frame, so it follows slopes and banked roads and a car in the air):
//! * the body's underside is a rounded rectangle of the car's box (`half` x, z) hovering half a tyre radius over the road: the
//!   road takes `k(gap) * occ(s)`, `gap` = the receiver's clearance under the underside (the road's height over the car's ground
//!   plane, so a jump or a ramp thins it out), `occ` the share of a cosine-weighted sky a half-plane at height `h = gap + SOFTEN`
//!   hides from a point `s` inside its edge (negative outside): `0.5 + (asin t + t sqrt(1 - t^2)) / pi`, `t = s / sqrt(s^2 + h^2)`
//!   (1 deep under the car, 0.5 at the outline, 0.09 at `h` outside, 0.02 at `2 h`), with the body sides' faint `SKIRT`
//!   (`exp(-d / 0.35 m)`) on top: the occlusion stays at the footprint (CONTACT-LAMPS, 2026-11-12: it was `0.7 exp(-d / L) +
//!   0.3 exp(-d / 3 L)` from the outline outwards, `L` ~0.27 m, so at full strength 0.4 at 0.3 m and 0.2 at 0.6 m out, the thick
//!   dark round patch at night between the street lamps);
//! * each of the four tyres adds a tight patch under the tyre with a short halo (the crease where the rubber meets the road);
//! * the receiver must face up in the car's frame (a wall or a kerb face beside the car is not darkened).
//! The result is the sky's occlusion: it multiplies the light of the track item where there is no sun (under cover, at night, E9
//! off, beyond the cascades); where E9 draws the sun's shadow the shader splits the baked light by E9's sun share `s` and the
//! occlusion takes only the ambient part, the sun's shadow only the direct part (`light ((1 - s) ao + s vis)`, SHADOW-REAL), and
//! the `sh80` blob fades out in the shader where a car's contact shadow stands (the footprint's `presence`), and the app does not lay the car's
//! `SHAD` patch at all while the car has a contact shadow (`render_scene.rs`), the way both fade under E9's cover gate.
//!
//! At night (no sun: monaco, industrial) it multiplies the baked light (the original's night bake, which holds the lamps' pools as
//! the scene's ambient) and not E23's added lamp light, whose car shadow is E47's map: under a lamp the car shows E47's shadow on
//! top of the same tight grounding it has between the lamps, so the two no longer alternate between a crisp shadow and a big dark
//! patch.
//!
//! In the sun the occlusion therefore shows only inside the car's sun shadow (deepening it under the body) and as a faint crease
//! at the tyres and sills on the sunlit side: one shadow, the car grounded. The split follows E9's per-pixel cover (the covered
//! boxes): in a tunnel it is the whole occlusion, at the mouth it turns into the split over the boxes' 6 m ramp, in space. Cost:
//! one uniform (16 cars x 6 vec4), a loop with an early-out in the fragment shader of the track's items; no pass, no texture.

use super::shadowmap::UniformSlot;
use super::*;

/// How strongly the contact shadow darkens at its darkest (deep under the body, at the tyres), 0..1, of the light it takes (all of
/// it without a sun, the ambient share in the sun). APPROXIMATION (enhancement content, by eye; was 0.9 before CONTACT-LAMPS).
pub(super) const STRENGTH: f32 = 0.85;
/// Added to the underside's clearance (m) as the height of the occluding half-plane: the width of the soft edge round the
/// footprint (about `clearance + SOFTEN` each side of the outline). APPROXIMATION (by eye: the body's underside is not flat, the
/// sills and bumpers curve up).
pub(super) const SOFTEN: f32 = 0.2;
/// The body sides' faint skirt beyond the outline: its darkening at the outline, falling off over 0.35 m. APPROXIMATION (by eye).
pub(super) const SKIRT: f32 = 0.35;

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
    /// x the car count (0 = off), y the strength, z the edge's softening, w the skirt
    params: [f32; 4],
}

pub(super) type ContactState = UniformSlot<ContactUniform>;

pub(super) fn layout_entry() -> wgpu::BindGroupLayoutEntry {
    crate::gfx::bgl_uniform(crate::classes::binding::CONTACTS, wgpu::ShaderStages::FRAGMENT)
}

impl Core {
    /// Whether this frame has contact shadows (E43 on, some car, not the rear-view mirror / an offscreen view).
    pub(super) fn contact_on(&self, scene: &Scene) -> bool {
        self.enhancements.is_on(crate::E43) && !self.no_reflections && scene.contacts.n > 0
    }

    /// Write this frame's uniform; without cars it is zeroed once if it was on. (SHADOW-REAL: no frame-wide weakening where E9's
    /// sun shadow is drawn: the shader splits the light per pixel.)
    pub(super) fn contact_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene) {
        if !self.contact_on(scene) {
            self.contact.clear(&mut self.belt, encoder);
            return;
        }
        let c = &scene.contacts;
        let mut u = <ContactUniform as bytemuck::Zeroable>::zeroed();
        for (i, car) in c.cars.iter().take(c.n as usize).enumerate() {
            u.pos[i] = [car.origin.x, car.origin.y, car.origin.z, car.half[0]];
            u.ax[i] = [car.axes[0].x, car.axes[0].y, car.axes[0].z, car.half[1]];
            u.ay[i] = [car.axes[1].x, car.axes[1].y, car.axes[1].z, car.wheel_r];
            u.az[i] = [car.axes[2].x, car.axes[2].y, car.axes[2].z, 0.0];
            u.wheels[2 * i] = [car.wheels[0][0], car.wheels[0][1], car.wheels[1][0], car.wheels[1][1]];
            u.wheels[2 * i + 1] = [car.wheels[2][0], car.wheels[2][1], car.wheels[3][0], car.wheels[3][1]];
        }
        u.params = [(c.n as usize).min(crate::MAX_CONTACTS) as f32, STRENGTH, SOFTEN, SKIRT];
        self.contact.write(&mut self.belt, encoder, &u);
    }

    /// Whether an item is a receiver of the contact shadow: a track item the E9 receiving also takes (drivable ground, walls ...; the
    /// shader keeps only faces that look up in the car's frame), never a front-end room's piece.
    pub(super) fn contact_receive(&self, item: &crate::DrawItem) -> bool {
        item.csm & crate::CSM_ROOM == 0 && !item.car_lit() && self.csm_receive(item) == crate::classes::RECV_TRACK
    }
}

#[cfg(test)]
mod tests {
    /// The mesh shader's `Contacts` is this uniform, byte for byte in size (its arrays' lengths are emitted from the Rust ones).
    #[test]
    fn the_shader_reads_the_uniform() {
        assert_eq!(crate::classes::tests::mesh_struct_size("Contacts") as usize, std::mem::size_of::<super::ContactUniform>());
    }
}
