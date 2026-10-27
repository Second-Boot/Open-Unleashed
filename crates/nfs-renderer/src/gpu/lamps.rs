//! ENHANCEMENT E23: the street lamps as point lights.
//!
//! The porsche side hands the frame the nearest street lamps ([`crate::Lamps`], up to
//! [`crate::MAX_LAMPS`]: positions out of the track's kind-2 flares, the colour of their
//! `flare.dat` record). The main shader (`shader.wgsl`, `lamp_light`) adds, to every receiver E15
//! has (the track's items and the cars), `texture * E`: a point light with a smooth falloff to the
//! radius and a downward cone (a lamp lights what lies below it, not the sky or a bridge's
//! underside). The track has no lighting model (baked vertex colour), so as in E15 the term is a
//! multiply-add on the texture with a floor under `n.l`. No shadows, no new pass: one small uniform.

use super::*;
use bytemuck::Zeroable;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct LampUniform {
    /// xyz the lamp, w its radius
    pos: [[f32; 4]; crate::MAX_LAMPS],
    /// rgb the colour with the brightness folded in
    col: [[f32; 4]; crate::MAX_LAMPS],
    /// x the lamp count (0 = off), z the gain
    params: [f32; 4],
}

pub(super) struct LampState {
    pub uniform: wgpu::Buffer,
    active: bool,
}

impl LampState {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("lamp uniform"), size: std::mem::size_of::<LampUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true });
        // zeroed: lamp count 0, the shader's "off"
        uniform.unmap();
        Self { uniform, active: false }
    }
}

pub(super) fn layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 20,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

impl Core {
    /// Whether this frame has street lamps (E23 on, some lamp, not the rear-view mirror).
    pub(super) fn lamps_on(&self, scene: &Scene) -> bool {
        self.enhancements.is_on(crate::E23) && !self.no_reflections && scene.lamps.n > 0
    }

    /// Write this frame's uniform; without lamps it is zeroed once if it was on.
    pub(super) fn lamp_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene) {
        let size = wgpu::BufferSize::new(std::mem::size_of::<LampUniform>() as u64).expect("lamp size");
        if !self.lamps_on(scene) {
            if self.lamp.active {
                self.lamp.active = false;
                self.belt.write_buffer(encoder, &self.lamp.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&LampUniform::zeroed()));
            }
            return;
        }
        let l = &scene.lamps;
        let mut u = LampUniform::zeroed();
        for (i, s) in l.lamps.iter().take(l.n as usize).enumerate() {
            u.pos[i] = [s.pos.x, s.pos.y, s.pos.z, s.radius];
            u.col[i] = [s.color[0], s.color[1], s.color[2], 0.0];
        }
        u.params = [l.n as f32, 0.0, l.gain, 0.0];
        self.lamp.active = true;
        self.belt.write_buffer(encoder, &self.lamp.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&u));
    }
}
