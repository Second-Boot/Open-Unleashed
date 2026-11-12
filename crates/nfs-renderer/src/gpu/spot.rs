//! ENHANCEMENT E15: the lit headlights as spotlights.
//!
//! What it does. The porsche side hands the frame the lit lamps of the cars in view (the camera's
//! own car, the opponents, the traffic, the police: [`crate::Spots`]) at the lamp centres of the car
//! model's `CARG` slots 0 / 1, shining along the car's forward axis pitched a little down. The main
//! shader (`shader.wgsl`, `spot_light`) adds, to every receiver, `texture * E` where `E` is the sum of
//! the lamps' irradiance: a smooth cone ([`CONE_OUTER_DEG`] / [`CONE_INNER_DEG`] half angles), a range
//! falloff to [`RANGE`] metres, a warm white, `n.l` with a floor on the track (it has no lighting
//! model of its own: its colour is baked vertex light x texture, so the road lit at a grazing angle is
//! given the floor instead of a physical 0.02). It is laid **over** what the original made - the
//! projected `night.psh` beam, the `HEAD` decal and the glare sprites stay as they are - so the spot
//! only adds. Receivers are the ones E9 has: the track's items ([`crate::CSM_RECEIVE`]) and the cars
//! (a `R3DLightLayer` with a direction).
//!
//! Shadows (level 2): the camera's own car's lamp pair shares one perspective shadow map, 1024^2, from
//! the pair's midpoint along its axis ([`FOV_DEG`] wide, near [`NEAR`], far [`RANGE`] + 10). The casters
//! are E9's (`csm_caster`: the cars, the roadside scenery); the car itself is left out (it stands at the
//! lamp). The other cars' lamps are lights without shadows. Reuses E9's two shadow pipelines and the
//! `shadow.wgsl` vertex stage: the matrix is a perspective one, the vertex stage's `z` pancake
//! only matters for a caster nearer than the lamp, which the near plane clips anyway.

use crate::gfx;
use super::*;
use bytemuck::Zeroable;
use glam::{Mat4, Vec3, Vec4};

/// Half angle of the cone where it has faded to nothing, degrees.
pub(super) const CONE_OUTER_DEG: f32 = 17.0;
/// Half angle up to which it is at full strength, degrees.
pub(super) const CONE_INNER_DEG: f32 = 6.0;
/// The reach of a lamp, metres.
pub(super) const RANGE: f32 = 70.0;
/// The shadow frustum's full angle: the cone and the offset between the two lamps.
const FOV_DEG: f32 = 44.0;
const NEAR: f32 = 0.8;
const SHADOW_SIZE: u32 = 1024;
/// The first two lamps' casters are left out within this distance of the pair's midpoint (the own car).
const SELF_CLEAR: f32 = 2.8;
/// A caster whose bounding sphere is larger than this (m) is left out (a hillside would fill the map).
const MAX_CASTER_RADIUS: f32 = 40.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct SpotUniform {
    /// xyz the lamp, w its intensity
    pos: [[f32; 4]; crate::MAX_SPOTS],
    /// xyz the way it shines
    dir: [[f32; 4]; crate::MAX_SPOTS],
    /// x the lamp count (0 = off), y cos of the outer half angle, z cos of the inner, w the range
    params: [f32; 4],
    /// x 1 while the first two lamps cast a shadow, y the world size of a shadow texel at 1 m,
    /// z the shadow frustum's near plane, w its far plane
    shadow: [f32; 4],
    mat: [[f32; 4]; 4],
    /// xyz the shadow's origin, w one texel in map coordinates
    origin: [f32; 4],
}

pub(super) struct SpotState {
    pub uniform: wgpu::Buffer,
    light_buf: wgpu::Buffer,
    light_bg: wgpu::BindGroup,
    _dummy: wgpu::Texture,
    dummy_view: wgpu::TextureView,
    map: Option<(wgpu::Texture, wgpu::TextureView)>,
    active: bool,
}

pub(super) struct SpotFrame {
    uniform: SpotUniform,
    /// the shadow pass's matrix, origin and axis when the pair casts
    shadow: Option<(Mat4, Vec3, Vec3)>,
}

impl SpotState {
    pub(super) fn new(device: &wgpu::Device, light_layout: &wgpu::BindGroupLayout) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("spot uniform"), size: std::mem::size_of::<SpotUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true });
        // zeroed: lamp count 0, the shader's "off"
        uniform.unmap();
        let light_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("spot light vp"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let light_bg = gfx::buffer_bg(device, "spot light", light_layout, &light_buf);
        let dummy = gfx::texture_2d(device, "spot map placeholder", 1, 1, DEPTH_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        let dummy_view = dummy.create_view(&Default::default());
        Self { uniform, light_buf, light_bg, _dummy: dummy, dummy_view, map: None, active: false }
    }

    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.1)
    }
}

pub(super) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        gfx::bgl_tex(10, wgpu::TextureSampleType::Depth, false),
        gfx::bgl_uniform(11, wgpu::ShaderStages::FRAGMENT),
    ]
}

/// The shadow frustum's matrix for a lamp pair at `origin` shining along `dir` (unit): x, y the
/// light's axes over `tan(fov / 2)`, `w` the distance along the axis, `z / w` 0 at `near`, 1 at `far`.
pub(super) fn shadow_matrix(origin: Vec3, dir: Vec3, near: f32, far: f32) -> Mat4 {
    shadow_matrix_tan(origin, dir, near, far, (FOV_DEG.to_radians() * 0.5).tan())
}

/// [`shadow_matrix`] with the frustum's `tan(half angle)` given (ENHANCEMENT E47's lamp maps fit theirs per frame).
pub(super) fn shadow_matrix_tan(origin: Vec3, dir: Vec3, near: f32, far: f32, t: f32) -> Mat4 {
    let (ax, ay) = csm::light_axes(dir);
    let a = far / (far - near);
    let b = -far * near / (far - near);
    let (ox, oy, od) = (ax.dot(origin) / t, ay.dot(origin) / t, dir.dot(origin));
    Mat4::from_cols(
        Vec4::new(ax.x / t, ay.x / t, dir.x * a, dir.x),
        Vec4::new(ax.y / t, ay.y / t, dir.y * a, dir.y),
        Vec4::new(ax.z / t, ay.z / t, dir.z * a, dir.z),
        Vec4::new(-ox, -oy, -od * a + b, -od),
    )
}

impl Core {
    /// This frame's spotlights, or `None` when E15 draws nothing (off, no lit lamp, the rear-view mirror).
    pub(super) fn spot_frame(&mut self, scene: &Scene) -> Option<SpotFrame> {
        let level = self.enhancements.level(crate::E15);
        let s = &scene.spots;
        if level == 0 || self.no_reflections || s.n == 0 {
            return None;
        }
        let mut u = SpotUniform::zeroed();
        for (i, l) in s.lamps.iter().take(s.n as usize).enumerate() {
            u.pos[i] = [l.pos.x, l.pos.y, l.pos.z, l.intensity];
            let d = l.dir.normalize_or_zero();
            u.dir[i] = [d.x, d.y, d.z, 0.0];
        }
        u.params = [(s.n as usize).min(crate::MAX_SPOTS) as f32, CONE_OUTER_DEG.to_radians().cos(), CONE_INNER_DEG.to_radians().cos(), RANGE];
        let far = RANGE + 10.0;
        let mut shadow = None;
        if level >= 2 && s.shadowed > 0 {
            let (a, b) = (&s.lamps[0], if s.shadowed > 1 { &s.lamps[1] } else { &s.lamps[0] });
            let origin = (a.pos + b.pos) * 0.5;
            let dir = (a.dir + b.dir).normalize_or_zero();
            if dir != Vec3::ZERO {
                let m = shadow_matrix(origin, dir, NEAR, far);
                u.shadow = [s.shadowed as f32, 2.0 * (FOV_DEG.to_radians() * 0.5).tan() / SHADOW_SIZE as f32, NEAR, far];
                u.mat = m.to_cols_array_2d();
                u.origin = [origin.x, origin.y, origin.z, 1.0 / SHADOW_SIZE as f32];
                shadow = Some((m, origin, dir));
            }
        }
        if shadow.is_some() && self.spot.map.is_none() {
            let texture = gfx::texture_2d(&self.device, "spot shadow map", SHADOW_SIZE, SHADOW_SIZE, DEPTH_FORMAT, gfx::RT_USAGE);
            let view = texture.create_view(&Default::default());
            tracing::info!(size = SHADOW_SIZE, "headlight shadow map (ENHANCEMENT E15)");
            self.spot.map = Some((texture, view));
            self.rebuild_globals_bg();
        }
        Some(SpotFrame { uniform: u, shadow })
    }

    /// What an item does with the spotlights (`ObjectUniform::soft.z`): the same roles as E9's
    /// receivers, 1 a car, 2 a track item, 0 neither.
    pub(super) fn spot_receive(&self, item: &crate::DrawItem) -> f32 {
        self.csm_receive(item)
    }

    /// Write the frame's uniform and draw the shadow map of the camera's own car's pair. Without a
    /// frame the uniform is zeroed once if it was on.
    pub(super) fn spot_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, n: usize, frame: Option<&SpotFrame>) {
        let size = wgpu::BufferSize::new(std::mem::size_of::<SpotUniform>() as u64).expect("spot size");
        let Some(frame) = frame else {
            if self.spot.active {
                self.spot.active = false;
                self.belt.write_buffer(encoder, &self.spot.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&SpotUniform::zeroed()));
            }
            return;
        };
        self.spot.active = true;
        self.belt.write_buffer(encoder, &self.spot.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&frame.uniform));
        let Some((m, origin, dir)) = frame.shadow else { return };
        let lsize = wgpu::BufferSize::new(64).expect("light size");
        self.belt.write_buffer(encoder, &self.spot.light_buf, 0, lsize).copy_from_slice(bytemuck::bytes_of(&m.to_cols_array_2d()));
        let half = (FOV_DEG * 0.5).to_radians();
        let mut casters: Vec<u32> = Vec::new();
        for (i, item) in scene.items.iter().take(n).enumerate() {
            if !self.csm_caster(item) {
                continue;
            }
            let mesh = &self.meshes[item.mesh.0 as usize];
            if mesh.index_count == 0 || mesh.bounds.1 <= 0.0 || mesh.flat {
                continue;
            }
            let t = &item.transform;
            let scale = t.x_axis.truncate().length().max(t.y_axis.truncate().length()).max(t.z_axis.truncate().length());
            let (centre, radius) = (t.transform_point3(Vec3::from(mesh.bounds.0)), mesh.bounds.1 * scale);
            if radius > MAX_CASTER_RADIUS {
                continue;
            }
            let v = centre - origin;
            let dist = v.length();
            // the own car (and anything at the lamp) casts nothing; beyond the reach or out of the frustum too
            if dist < SELF_CLEAR && radius < 4.0 || dist - radius > RANGE + 10.0 {
                continue;
            }
            let along = v.dot(dir);
            if along + radius < 0.0 {
                continue;
            }
            if dist > radius {
                let ang = (along / dist).clamp(-1.0, 1.0).acos();
                if ang > half * 1.45 + (radius / dist).asin() {
                    continue;
                }
            }
            casters.push(i as u32);
        }
        let map = &self.spot.map.as_ref().expect("spot map made").1;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("headlight shadow pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: map,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: crate::prof::rp("headlight shadow"),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.spot.light_bg, &[]);
        self.draw_shadow_casters(&mut pass, scene, &casters);
    }

    /// Draw `casters` (indices into `scene.items`) into a depth-only shadow pass whose light group (0) is set: E9's opaque /
    /// cut-out pipelines, the object buffer as group 1 (shared by E15's headlight map and ENHANCEMENT E47's lamp maps).
    pub(super) fn draw_shadow_casters(&self, pass: &mut wgpu::RenderPass<'_>, scene: &Scene, casters: &[u32]) {
        pass.set_bind_group(1, &self.object_bg, &[]);
        let (mut cur_pipe, mut cur_mat, mut cur_mesh) = (u8::MAX, u32::MAX, u32::MAX);
        for &idx in casters {
            let item = &scene.items[idx as usize];
            let mat = &self.materials[item.material.0 as usize];
            let cut = mat.alpha_blend || mat.device_states || item.alpha_test;
            if cur_pipe != cut as u8 {
                pass.set_pipeline(if cut { &self.csm.pipe_cut } else { &self.csm.pipe_opaque });
                cur_pipe = cut as u8;
                cur_mat = u32::MAX;
            }
            if cut && cur_mat != item.material.0 {
                pass.set_bind_group(2, &mat.bind_group, &[]);
                cur_mat = item.material.0;
            }
            let mesh = &self.meshes[item.mesh.0 as usize];
            if cur_mesh != item.mesh.0 {
                pass.set_vertex_buffer(0, mesh.vertex_buf.slice(mesh.vertex_offset..mesh.vertex_offset + mesh.vertex_len.max(4)));
                pass.set_index_buffer(mesh.index_buf.slice(mesh.index_offset..mesh.index_offset + mesh.index_len.max(4)), wgpu::IndexFormat::Uint32);
                cur_mesh = item.mesh.0;
            }
            crate::prof::count_draw(u64::from(mesh.index_count / 3));
            pass.draw_indexed(0..mesh.index_count, 0, idx..idx + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_matrix_maps_the_axis_to_the_centre_and_depth_to_0_1() {
        let o = Vec3::new(10.0, 0.7, -3.0);
        let d = Vec3::new(0.2, -0.05, 1.0).normalize();
        let m = shadow_matrix(o, d, 0.8, 80.0);
        let near = m * (o + d * 0.8).extend(1.0);
        let far = m * (o + d * 80.0).extend(1.0);
        assert!((near.z / near.w).abs() < 1e-4, "{near:?}");
        assert!((far.z / far.w - 1.0).abs() < 1e-4, "{far:?}");
        let mid = m * (o + d * 30.0).extend(1.0);
        assert!((mid.x / mid.w).abs() < 1e-4 && (mid.y / mid.w).abs() < 1e-4);
        // a point at the frustum's edge angle lands on the border
        let (ax, _) = csm::light_axes(d);
        let edge = o + d * 30.0 + ax * (30.0 * (FOV_DEG.to_radians() * 0.5).tan());
        let e = m * edge.extend(1.0);
        assert!(((e.x / e.w).abs() - 1.0).abs() < 1e-3, "{e:?}");
    }
}
