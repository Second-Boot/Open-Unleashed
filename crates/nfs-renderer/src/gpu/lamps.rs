//! ENHANCEMENT E23: the street lamps as point lights.
//!
//! The porsche side hands the frame the nearest street lamps ([`crate::Lamps`], up to
//! [`crate::MAX_LAMPS`]: positions out of the track's kind-2 flares, the colour of their
//! `flare.dat` record). The main shader (`shader.wgsl`, `lamp_light`) adds, to every receiver E15
//! has (the track's items and the cars), `texture * E`: a point light with a smooth falloff to the
//! radius and a downward cone (a lamp lights what lies below it, not the sky or a bridge's
//! underside). The track has no lighting model (baked vertex colour), so as in E15 the term is a
//! multiply-add on the texture with a floor under `n.l`. One small uniform.
//!
//! ENHANCEMENT E47: the cars' shadows from these lamps. Of the frame's lamps (nearest to the camera first, the order the porsche
//! side hands them in) the first [`SHADOW_SLOTS`] that have a car in their light get a perspective depth map each, one layer of a
//! [`SHADOW_SIZE`]^2 `Depth32Float` array: the frustum looks from the lamp at the car parts in its reach and is just wide enough
//! for their bounding spheres (a caster's shadow lies inside the caster's own cone as seen from the lamp, and the cars are the
//! only casters, so nothing outside the cone can be shadowed), its far plane at the lamp's radius. Drawn with E9's two depth-only
//! pipelines and `shadow.wgsl` like E15's map. The shader multiplies only that lamp's own term by the visibility (`lamp_visibility`:
//! normal offset, a metre-based bias, 3 x 3 hardware PCF with a four-tap early-out), so the baked night, the other lamps and E9 /
//! E43 stay as they are. A slot's strength fades with the lamp's distance between 75 % of the first lamp left out (the one after
//! the last slot) and that lamp, so a lamp taking or losing a slot does not pop; a lamp with no car in its reach costs nothing.

use super::*;
use bytemuck::Zeroable;
use glam::Vec3;

/// ENHANCEMENT E47: how many lamps of a frame cast the cars' shadows (the layers of the map array).
pub(super) const SHADOW_SLOTS: usize = 4;
/// ENHANCEMENT E47: one layer's size; the frustum is fitted to the cars in the lamp's reach, so a texel is a few cm.
pub(super) const SHADOW_SIZE: u32 = 512;
/// ENHANCEMENT E47: the widest half angle of a lamp's frustum (a car right under a low lamp); beyond it the edge fades to lit.
const MAX_HALF_DEG: f32 = 72.0;
/// ENHANCEMENT E47: a car part whose bounding sphere is larger than this (m) is no caster (a wrongly flagged huge item).
const MAX_CASTER_RADIUS: f32 = 6.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct LampUniform {
    /// xyz the lamp, w its radius
    pos: [[f32; 4]; crate::MAX_LAMPS],
    /// rgb the colour with the brightness folded in; w (ENHANCEMENT E47) the lamp's shadow slot + 1 (0 = no shadow)
    col: [[f32; 4]; crate::MAX_LAMPS],
    /// x the lamp count (0 = off), z the gain, w (ENHANCEMENT E47) one shadow texel in map coordinates
    params: [f32; 4],
    /// ENHANCEMENT E47: per slot the lamp's shadow frustum
    smat: [[[f32; 4]; 4]; SHADOW_SLOTS],
    /// ENHANCEMENT E47: per slot x the world size of a texel at 1 m, y near, z far, w the shadow's strength (0..1)
    sinfo: [[f32; 4]; SHADOW_SLOTS],
}

/// ENHANCEMENT E47: the lamp shadows' array texture.
struct LampShadowMap {
    _texture: wgpu::Texture,
    array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
}

pub(super) struct LampState {
    pub uniform: wgpu::Buffer,
    active: bool,
    /// ENHANCEMENT E47: one light matrix per slot for the depth-only pipelines
    light_bufs: Vec<wgpu::Buffer>,
    light_bgs: Vec<wgpu::BindGroup>,
    _dummy: wgpu::Texture,
    dummy_view: wgpu::TextureView,
    map: Option<LampShadowMap>,
    /// ENHANCEMENT E47: the caster lists of this frame's slots (kept across frames: no allocation per frame)
    casters: Vec<Vec<u32>>,
}

impl LampState {
    pub(super) fn new(device: &wgpu::Device, light_layout: &wgpu::BindGroupLayout) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("lamp uniform"), size: std::mem::size_of::<LampUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true });
        // zeroed: lamp count 0, the shader's "off"
        uniform.unmap();
        let light_bufs: Vec<wgpu::Buffer> = (0..SHADOW_SLOTS).map(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some("lamp shadow vp"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })).collect();
        let light_bgs = light_bufs.iter().map(|b| crate::gfx::buffer_bg(device, "lamp shadow light", light_layout, b)).collect();
        let dummy = crate::gfx::texture_2d(device, "lamp shadow placeholder", 1, 1, DEPTH_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        let dummy_view = dummy.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        Self { uniform, active: false, light_bufs, light_bgs, _dummy: dummy, dummy_view, map: None, casters: vec![Vec::new(); SHADOW_SLOTS] }
    }

    /// ENHANCEMENT E47: the view the main shader binds (the placeholder until a map exists).
    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.array_view)
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

/// ENHANCEMENT E47: the lamp shadows' depth array (binding 25).
pub(super) fn shadow_layout_entry() -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding: 25,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
        count: None,
    }
}

/// ENHANCEMENT E47: one caster: a car part's index in the frame's items and its world bounding sphere.
#[derive(Clone, Copy, Debug)]
pub(super) struct Caster {
    pub idx: u32,
    pub centre: Vec3,
    pub radius: f32,
}

/// ENHANCEMENT E47: one lamp's shadow frustum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LampFrustum {
    pub dir: Vec3,
    pub tan_half: f32,
    pub near: f32,
    pub far: f32,
}

/// ENHANCEMENT E47: whether a caster stands in a lamp's light: its sphere reaches into the lamp's radius and is not all above the
/// lamp (the light's cone points down; nothing above the lamp's horizon is lit, so nothing there casts into the light).
pub(super) fn in_reach(lamp: Vec3, radius: f32, c: &Caster) -> bool {
    c.centre.distance(lamp) - c.radius < radius && c.centre.y - c.radius < lamp.y
}

/// ENHANCEMENT E47: the frustum from `lamp` that holds every caster of `cs` (all `in_reach`): its axis the mean of the directions
/// to them, its half angle the widest caster cone round it (`asin(r / d)` of each sphere, held to [`MAX_HALF_DEG`]), near short of
/// the nearest sphere, far at the lamp's radius (beyond it the lamp lights nothing).
pub(super) fn fit_frustum(lamp: Vec3, radius: f32, cs: impl Iterator<Item = Caster> + Clone) -> Option<LampFrustum> {
    let mut sum = Vec3::ZERO;
    let mut near = f32::MAX;
    for c in cs.clone() {
        let v = c.centre - lamp;
        let d = v.length();
        if d > 1e-3 {
            sum += v / d;
        }
        near = near.min(d - c.radius);
    }
    let dir = sum.try_normalize().unwrap_or(Vec3::NEG_Y);
    let max_half = MAX_HALF_DEG.to_radians();
    let mut half: f32 = 0.0;
    for c in cs {
        let v = c.centre - lamp;
        let d = v.length();
        let spread = if d <= c.radius { max_half } else { (c.radius / d).asin() };
        let off = if d > 1e-3 { (v.dot(dir) / d).clamp(-1.0, 1.0).acos() } else { 0.0 };
        half = half.max(off + spread);
    }
    if near == f32::MAX {
        return None;
    }
    // a little margin so the edge fade (the outer 15 %) does not touch a caster's silhouette
    let half = (half * 1.08 + 1.0_f32.to_radians()).min(max_half);
    let near = (near * 0.9).clamp(0.1, 4.0);
    Some(LampFrustum { dir, tan_half: half.tan(), near, far: radius + 1.0 })
}

/// ENHANCEMENT E47: a slot's strength by its lamp's distance `d` from the camera and the first left-out lamp's `cut` (fades over
/// the last quarter before it, the same rule as the lamps' own selection).
pub(super) fn slot_strength(d: f32, cut: f32) -> f32 {
    let f = 1.0 - ((d - 0.75 * cut) / (0.25 * cut).max(1e-3)).clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

impl Core {
    /// Whether this frame has street lamps (E23 on, some lamp, not the rear-view mirror).
    pub(super) fn lamps_on(&self, scene: &Scene) -> bool {
        self.enhancements.is_on(crate::E23) && !self.no_reflections && scene.lamps.n > 0
    }

    /// ENHANCEMENT E47: the frame's car parts that cast the lamp shadows (E9's casters whose light has a direction: the cars).
    fn lamp_casters(&self, scene: &Scene, n: usize) -> Vec<Caster> {
        let mut out = Vec::new();
        for (i, item) in scene.items.iter().take(n).enumerate() {
            if !item.layer_light.is_some_and(|l| l.direction != [0.0; 3]) || !self.csm_caster(item) {
                continue;
            }
            let mesh = &self.meshes[item.mesh.0 as usize];
            if mesh.index_count == 0 || mesh.bounds.1 <= 0.0 {
                continue;
            }
            let t = &item.transform;
            let scale = t.x_axis.truncate().length().max(t.y_axis.truncate().length()).max(t.z_axis.truncate().length());
            let (centre, radius) = (t.transform_point3(Vec3::from(mesh.bounds.0)), mesh.bounds.1 * scale);
            if radius <= MAX_CASTER_RADIUS {
                out.push(Caster { idx: i as u32, centre, radius });
            }
        }
        out
    }

    /// Write this frame's uniform; without lamps it is zeroed once if it was on. ENHANCEMENT E47: with E47 on, the maps of the
    /// lamps that cast the cars' shadows are drawn first.
    pub(super) fn lamp_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, n: usize, camera: &Camera) {
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
        // ENHANCEMENT E47: the lamps with a car in their light, nearest to the camera first
        let mut slots: Vec<(usize, LampFrustum, Vec<u32>)> = Vec::new();
        if self.enhancements.is_on(crate::E47) {
            let casters = self.lamp_casters(scene, n);
            let mut cut = f32::MAX;
            if !casters.is_empty() {
                for (i, s) in l.lamps.iter().take(l.n as usize).enumerate() {
                    if s.color == [0.0; 3] || s.radius <= 0.0 {
                        continue;
                    }
                    let near = || casters.iter().copied().filter(|c| in_reach(s.pos, s.radius, c));
                    if near().next().is_none() {
                        continue;
                    }
                    if slots.len() == SHADOW_SLOTS {
                        cut = s.pos.distance(camera.position);
                        break;
                    }
                    if let Some(f) = fit_frustum(s.pos, s.radius, near()) {
                        slots.push((i, f, near().map(|c| c.idx).collect()));
                    }
                }
            }
            for (k, (i, f, _)) in slots.iter().enumerate() {
                let p = l.lamps[*i].pos;
                let m = spot::shadow_matrix_tan(p, f.dir, f.near, f.far, f.tan_half);
                u.smat[k] = m.to_cols_array_2d();
                u.sinfo[k] = [2.0 * f.tan_half / SHADOW_SIZE as f32, f.near, f.far, slot_strength(p.distance(camera.position), cut)];
                u.col[*i][3] = (k + 1) as f32;
            }
            u.params[3] = 1.0 / SHADOW_SIZE as f32;
            tracing::debug!(slots = slots.len(), casters = casters.len(), cut, "lamp shadows (ENHANCEMENT E47)");
        }
        self.lamp.active = true;
        self.belt.write_buffer(encoder, &self.lamp.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&u));
        if slots.is_empty() {
            return;
        }
        if self.lamp.map.is_none() {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("lamp shadow maps"),
                size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: SHADOW_SLOTS as u32 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let array_view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            let layer_views = (0..SHADOW_SLOTS as u32)
                .map(|i| texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i, array_layer_count: Some(1), ..Default::default() }))
                .collect();
            tracing::info!(size = SHADOW_SIZE, layers = SHADOW_SLOTS, "lamp shadow maps (ENHANCEMENT E47)");
            self.lamp.map = Some(LampShadowMap { _texture: texture, array_view, layer_views });
            self.rebuild_globals_bg();
        }
        let lsize = wgpu::BufferSize::new(64).expect("light size");
        let used = slots.len();
        for (k, (_, _, list)) in slots.into_iter().enumerate() {
            self.belt.write_buffer(encoder, &self.lamp.light_bufs[k], 0, lsize).copy_from_slice(bytemuck::bytes_of(&u.smat[k]));
            self.lamp.casters[k] = list;
        }
        for k in 0..used {
            let map = &self.lamp.map.as_ref().expect("lamp map made").layer_views[k];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lamp shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: map,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: crate::prof::rp("lamp shadow"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.lamp.light_bgs[k], &[]);
            self.draw_shadow_casters(&mut pass, scene, &self.lamp.casters[k]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn car(x: f32, z: f32) -> Caster {
        Caster { idx: 0, centre: Vec3::new(x, 0.7, z), radius: 2.4 }
    }

    /// the frustum holds every caster's sphere, points at them, and its far plane is the lamp's reach
    #[test]
    fn the_frustum_holds_the_cars_in_the_lamps_reach() {
        let lamp = Vec3::new(0.0, 12.0, 0.0);
        let cs = [car(6.0, 2.0), car(9.0, -3.0)];
        let f = fit_frustum(lamp, 25.0, cs.iter().copied()).expect("frustum");
        assert!(f.dir.y < -0.5, "{f:?}");
        assert_eq!(f.far, 26.0);
        let half = f.tan_half.atan();
        for c in &cs {
            let v = c.centre - lamp;
            let off = (v.normalize().dot(f.dir)).acos();
            assert!(off + (c.radius / v.length()).asin() <= half + 1e-4, "{c:?} {f:?}");
            assert!(f.near < v.length() - c.radius);
        }
        // a car right under a low lamp: the widest frustum, not more
        let low = Vec3::new(0.0, 2.0, 0.0);
        let f = fit_frustum(low, 10.0, [car(0.0, 0.0)].into_iter()).expect("frustum");
        assert!((f.tan_half.atan().to_degrees() - MAX_HALF_DEG).abs() < 1e-3, "{f:?}");
        assert!(fit_frustum(lamp, 25.0, std::iter::empty()).is_none());
    }

    #[test]
    fn reach_and_strength() {
        let lamp = Vec3::new(0.0, 12.0, 0.0);
        assert!(in_reach(lamp, 25.0, &car(10.0, 0.0)));
        assert!(!in_reach(lamp, 25.0, &car(40.0, 0.0)));
        // a car on a deck above the lamp
        assert!(!in_reach(lamp, 25.0, &Caster { idx: 0, centre: Vec3::new(3.0, 16.0, 0.0), radius: 2.4 }));
        assert_eq!(slot_strength(10.0, f32::MAX), 1.0);
        assert_eq!(slot_strength(40.0, 40.0), 0.0);
        assert_eq!(slot_strength(20.0, 40.0), 1.0);
        assert!((slot_strength(35.0, 40.0) - 0.5).abs() < 1e-6);
    }
}
