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

use super::shadowmap::{Caster, DepthArray, LightMats, UniformSlot};
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
/// A caster within [`SELF_CLEAR`] smaller than this (m) is the own car's part (or at the lamp): left out.
const SELF_RADIUS: f32 = 4.0;
/// The shadow frustum's far plane beyond the lamps' reach (m).
const FAR_MARGIN: f32 = 10.0;
/// A caster is left out beyond this many of the frustum's half angles off its axis (plus its own angular radius).
const CONE_SLACK: f32 = 1.45;

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
    pub uniform: UniformSlot<SpotUniform>,
    light: LightMats,
    dummy_view: wgpu::TextureView,
    /// the map: one layer, read through its 2-D layer view (the mesh shader's and E36's `t_spot: texture_depth_2d`)
    map: Option<DepthArray>,
    /// the frame's caster list, kept between frames
    list: Vec<u32>,
}

pub(super) struct SpotFrame {
    uniform: SpotUniform,
    /// the shadow pass's matrix, origin and axis when the pair casts
    shadow: Option<(Mat4, Vec3, Vec3)>,
}

impl SpotFrame {
    /// Whether the camera car's pair draws its shadow map this frame (level 2, a lit pair).
    pub(super) fn casts(&self) -> bool {
        self.shadow.is_some()
    }
}

impl SpotState {
    pub(super) fn new(device: &wgpu::Device, light_layout: &wgpu::BindGroupLayout) -> Self {
        let dummy_view = DepthArray::placeholder_2d(device, "spot map placeholder");
        Self { uniform: UniformSlot::new(device, "spot uniform"), light: LightMats::new(device, light_layout, "spot light vp", 1), dummy_view, map: None, list: Vec::new() }
    }

    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.layer_views[0])
    }
}

pub(super) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        gfx::bgl_tex(crate::classes::binding::SPOT_MAP, wgpu::TextureSampleType::Depth, false),
        gfx::bgl_uniform(crate::classes::binding::SPOTS, wgpu::ShaderStages::FRAGMENT),
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
        let far = RANGE + FAR_MARGIN;
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
            self.spot.map = Some(DepthArray::new(&self.device, "headlight shadow map (ENHANCEMENT E15)", SHADOW_SIZE, 1));
            self.rebuild_globals_bg();
        }
        Some(SpotFrame { uniform: u, shadow })
    }

    /// Write the frame's uniform and draw the shadow map of the camera's own car's pair (`casters`: the frame's). Without a frame
    /// the uniform is zeroed once if it was on.
    pub(super) fn spot_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, casters: &[Caster], frame: Option<&SpotFrame>) {
        let Some(frame) = frame else {
            self.spot.uniform.clear(&mut self.belt, encoder);
            return;
        };
        self.spot.uniform.write(&mut self.belt, encoder, &frame.uniform);
        let Some((m, origin, dir)) = frame.shadow else { return };
        self.spot.light.write(&mut self.belt, encoder, 0, &m.to_cols_array_2d());
        let half = (FOV_DEG * 0.5).to_radians();
        let mut list = std::mem::take(&mut self.spot.list);
        list.clear();
        list.extend(
            casters
                .iter()
                .filter(|c| {
                    if c.shape.flat || c.radius > MAX_CASTER_RADIUS {
                        return false;
                    }
                    let v = c.centre - origin;
                    let dist = v.length();
                    // the own car (and anything at the lamp) casts nothing; beyond the reach or out of the frustum neither
                    if dist < SELF_CLEAR && c.radius < SELF_RADIUS || dist - c.radius > RANGE + FAR_MARGIN {
                        return false;
                    }
                    let along = v.dot(dir);
                    if along + c.radius < 0.0 {
                        return false;
                    }
                    dist <= c.radius || (along / dist).clamp(-1.0, 1.0).acos() <= half * CONE_SLACK + (c.radius / dist).asin()
                })
                .map(|c| c.idx),
        );
        let map = &self.spot.map.as_ref().expect("spot map made").layer_views[0];
        let mut pass = super::shadowmap::depth_pass(encoder, "headlight shadow pass", "headlight shadow", map);
        pass.set_bind_group(0, self.spot.light.bind_group(0), &[]);
        self.draw_shadow_casters(&mut pass, scene, &list);
        drop(pass);
        self.spot.list = list;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mesh shader's `Spots` is this uniform, byte for byte in size (its arrays' lengths are emitted from the Rust ones).
    #[test]
    fn the_shader_reads_the_uniform() {
        assert_eq!(crate::classes::tests::mesh_struct_size("Spots") as usize, std::mem::size_of::<SpotUniform>());
    }

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
