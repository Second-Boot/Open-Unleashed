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
//! ENHANCEMENT E47: the cars' shadows from these lamps. Of the frame's lamps that have a car in their light the
//! [`SHADOW_SLOTS`] whose light on the cars matters most ([`lamp_score`], LAMP-TUNE) get a perspective depth map each, one layer of a
//! [`SHADOW_SIZE`]^2 `Depth32Float` array: the frustum looks from the lamp at the car parts in its reach and is just wide enough
//! for their bounding spheres (a caster's shadow lies inside the caster's own cone as seen from the lamp, and the cars are the
//! only casters, so nothing outside the cone can be shadowed), its far plane at the lamp's radius. Drawn with E9's two depth-only
//! pipelines and `shadow.wgsl` like E15's map. The shader multiplies only that lamp's own term by the visibility (`lamp_visibility`:
//! normal offset, a metre-based bias, 3 x 3 hardware PCF with a four-tap early-out), so the baked night, the other lamps and E9 /
//! E43 stay as they are. A slot's strength fades in from the score of the first lamp left out (the one after the last slot) to 1.5
//! times it ([`slot_strength`]), so a lamp taking or losing a slot does not pop; a lamp with no car in its reach costs nothing.

use super::shadowmap::{Caster, DepthArray, LightMats, UniformSlot};
use super::*;
use bytemuck::Zeroable;
use glam::Vec3;

/// ENHANCEMENT E47: how many lamps of a frame cast the cars' shadows (the layers of the map array).
pub(super) const SHADOW_SLOTS: usize = crate::classes::LAMP_SHADOW_SLOTS;
/// ENHANCEMENT E47: one layer's size; the frustum is fitted to the cars in the lamp's reach, so a texel is a few cm.
pub(super) const SHADOW_SIZE: u32 = 512;
/// ENHANCEMENT E47: the widest half angle of a lamp's frustum (a car right under a low lamp); beyond it the edge fades to lit.
const MAX_HALF_DEG: f32 = 72.0;
/// ENHANCEMENT E47: a car part whose bounding sphere is larger than this (m) is no caster (a wrongly flagged huge item).
const LAMP_MAX_CASTER_RADIUS: f32 = 6.0;
/// ENHANCEMENT E47: the frustum's widening beyond the casters' widest cone (a share, and an angle in degrees), so the edge fade (the
/// outer 15 %) does not touch a caster's silhouette.
const FRUSTUM_MARGIN: f32 = 1.08;
const FRUSTUM_MARGIN_DEG: f32 = 1.0;
/// ENHANCEMENT E47: the near plane is this share of the nearest caster's distance, held to `NEAR_RANGE` (m).
const NEAR_SHARE: f32 = 0.9;
const NEAR_RANGE: (f32, f32) = (0.1, 4.0);
/// ENHANCEMENT E47: the far plane beyond the lamp's radius (m).
const LAMP_FAR_MARGIN: f32 = 1.0;
/// ENHANCEMENT E47 (LAMP-TUNE): [`lamp_score`]'s eye distance (m) at which a car counts half, its cone's least weight, and the
/// shader's downward cone (`lamp_light`: `smoothstep(0.1, 0.55, l.y)`, its start and width). APPROXIMATION (the 40 m).
const SCORE_EYE: f32 = 40.0;
const SCORE_MIN_CONE: f32 = 0.05;
const CONE_START: f32 = crate::classes::LAMP_CONE_START;
/// (= `LAMP_CONE_END - LAMP_CONE_START`, written out: that subtraction rounds to the next float up, not the 0.45 the score
/// always used)
const CONE_WIDTH: f32 = 0.45;
/// ENHANCEMENT E47 (LAMP-TUNE): a slot's strength is full at this many times the first left-out lamp's score ([`slot_strength`]).
const SLOT_FULL: f32 = 1.5;
/// The least distance (m) a direction to a caster is taken over (nearer: no direction).
const MIN_DIST: f32 = 1e-3;
/// The debug log's scores and strengths in thousandths.
const LOG_SCALE: f32 = 1000.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct LampUniform {
    /// xyz the lamp, w its radius (MONACO-TUNNEL: negative for a [`crate::StreetLamp::baked`] lamp)
    pos: [[f32; 4]; crate::MAX_LAMPS],
    /// rgb the colour with the brightness folded in; w (ENHANCEMENT E47) the lamp's shadow slot + 1 (0 = no shadow)
    col: [[f32; 4]; crate::MAX_LAMPS],
    /// x the lamp count (0 = off), y (MONACO-TUNNEL) [`crate::BAKED_SHADOW`], z the gain, w (ENHANCEMENT E47) one shadow texel in map coordinates
    params: [f32; 4],
    /// ENHANCEMENT E47: per slot the lamp's shadow frustum
    smat: [[[f32; 4]; 4]; SHADOW_SLOTS],
    /// ENHANCEMENT E47: per slot x the world size of a texel at 1 m, y near, z far, w the shadow's strength (0..1)
    sinfo: [[f32; 4]; SHADOW_SLOTS],
}

pub(super) struct LampState {
    pub uniform: UniformSlot<LampUniform>,
    /// ENHANCEMENT E47: one light matrix per slot for the depth-only pipelines
    lights: LightMats,
    dummy_view: wgpu::TextureView,
    map: Option<DepthArray>,
    /// ENHANCEMENT E47: the caster lists of this frame's slots (kept across frames: no allocation per frame)
    casters: Vec<Vec<u32>>,
    /// ENHANCEMENT E47: the pass's working lists, kept across frames like `casters`
    scratch: SlotScratch,
}

/// [`Core::lamp_pass`]'s lists, refilled each frame.
#[derive(Default)]
struct SlotScratch {
    /// the frame's car parts that can cast into a lamp's map
    cars: Vec<Caster>,
    /// the lamps with a car in their light: (index, `lamp_score`)
    ranked: Vec<(usize, f32)>,
    /// the slots: (the lamp's index, its frustum, its score)
    slots: Vec<(usize, LampFrustum, f32)>,
}

impl LampState {
    pub(super) fn new(device: &wgpu::Device, light_layout: &wgpu::BindGroupLayout) -> Self {
        let dummy_view = DepthArray::placeholder(device, "lamp shadow placeholder");
        Self { uniform: UniformSlot::new(device, "lamp uniform"), lights: LightMats::new(device, light_layout, "lamp shadow vp", SHADOW_SLOTS), dummy_view, map: None, casters: vec![Vec::new(); SHADOW_SLOTS], scratch: SlotScratch::default() }
    }

    /// ENHANCEMENT E47: the view the main shader binds (the placeholder until a map exists).
    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.array_view)
    }
}

pub(super) fn layout_entry() -> wgpu::BindGroupLayoutEntry {
    crate::gfx::bgl_uniform(crate::classes::binding::LAMPS, wgpu::ShaderStages::FRAGMENT)
}

/// ENHANCEMENT E47: the lamp shadows' depth array.
pub(super) fn shadow_layout_entry() -> wgpu::BindGroupLayoutEntry {
    super::shadowmap::array_layout_entry(crate::classes::binding::LAMP_SHADOW)
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
        if d > MIN_DIST {
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
        let off = if d > MIN_DIST { (v.dot(dir) / d).clamp(-1.0, 1.0).acos() } else { 0.0 };
        half = half.max(off + spread);
    }
    if near == f32::MAX {
        return None;
    }
    // a little margin so the edge fade (the outer 15 %) does not touch a caster's silhouette
    let half = (half * FRUSTUM_MARGIN + FRUSTUM_MARGIN_DEG.to_radians()).min(max_half);
    let near = (near * NEAR_SHARE).clamp(NEAR_RANGE.0, NEAR_RANGE.1);
    Some(LampFrustum { dir, tan_half: half.tan(), near, far: radius + LAMP_FAR_MARGIN })
}

/// ENHANCEMENT E47 (LAMP-TUNE): how much a lamp's shadow matters this frame: the largest, over the car parts in its reach, of its
/// light at the part (the shader's falloff `smoothstep(1 - d / radius)` and downward cone, `d` from the lamp to the part's sphere,
/// times the colour's brightness) weighted by the part's nearness to the camera (`1 / (1 + (d_eye / 40 m)^2)`: a car far ahead
/// under a lamp counts less than the one under the camera). 0 = no car in its light. Smooth in the cars' and the camera's
/// positions, so the ranking and [`slot_strength`] change without steps. APPROXIMATION (the 40 m).
pub(super) fn lamp_score(lamp: Vec3, radius: f32, color: [f32; 3], eye: Vec3, casters: &[Caster]) -> f32 {
    let bright = color[0].max(color[1]).max(color[2]);
    let mut best: f32 = 0.0;
    for c in casters.iter().filter(|c| in_reach(lamp, radius, c)) {
        let v = c.centre - lamp;
        let d = (v.length() - c.radius).max(0.0);
        let r = (1.0 - d / radius).clamp(0.0, 1.0);
        let fall = nfs_math::smooth01(r);
        // the cone toward the part's lowest point (the lamp lights what is below it)
        let low = c.centre - Vec3::Y * c.radius - lamp;
        let cone = nfs_math::smooth01(((-low.y / low.length().max(MIN_DIST) - CONE_START) / CONE_WIDTH).clamp(0.0, 1.0));
        let e = c.centre.distance(eye) / SCORE_EYE;
        best = best.max(bright * fall * cone.max(SCORE_MIN_CONE) / (1.0 + e * e));
    }
    best
}

/// ENHANCEMENT E47 (LAMP-TUNE): a slot's strength by its lamp's [`lamp_score`] and the first left-out lamp's `cut` (0 = none left
/// out): full from 1.5 x the cut, 0 at the cut, so a lamp taking or losing the last slot fades instead of popping. (Was by the
/// lamp's distance from the camera, which did not follow the cars: a lamp entering the set as a car drove into its light could
/// push another out at full strength.)
pub(super) fn slot_strength(score: f32, cut: f32) -> f32 {
    if cut <= 0.0 {
        return 1.0;
    }
    nfs_math::smooth01(((score - cut) / ((SLOT_FULL - 1.0) * cut)).clamp(0.0, 1.0))
}

impl Core {
    /// Whether this frame has street lamps (E23 on, some lamp, not the rear-view mirror).
    pub(super) fn lamps_on(&self, scene: &Scene) -> bool {
        self.enhancements.is_on(crate::E23) && !self.modes.no_reflections && scene.lamps.n > 0
    }

    /// ENHANCEMENT E47: whether this frame draws lamp shadow maps (E47 on, lamps; not the environment faces, which take the lamps'
    /// light without their maps: a pass per lamp per face).
    pub(super) fn lamp_shadows_on(&self, scene: &Scene) -> bool {
        self.lamps_on(scene) && self.enhancements.is_on(crate::E47) && !self.env.building
    }

    /// Write this frame's uniform; without lamps it is zeroed once if it was on. ENHANCEMENT E47: with E47 on, the maps of the
    /// lamps that cast the cars' shadows are drawn first (`casters`: the frame's; the car parts among them cast here).
    pub(super) fn lamp_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, casters: &[Caster], camera: &Camera) {
        if !self.lamps_on(scene) {
            self.lamp.uniform.clear(&mut self.belt, encoder);
            return;
        }
        let l = &scene.lamps;
        let mut u = LampUniform::zeroed();
        for (i, s) in l.lamps.iter().take(l.n as usize).enumerate() {
            // MONACO-TUNNEL: a baked lamp's radius is stored negative (the shader's flag for the bake-darkening shadow)
            u.pos[i] = [s.pos.x, s.pos.y, s.pos.z, if s.baked { -s.radius } else { s.radius }];
            u.col[i] = [s.color[0], s.color[1], s.color[2], 0.0];
        }
        // (y: MONACO-TUNNEL, the baked lamps' shadow share of the baked colour)
        u.params = [(l.n as usize).min(crate::MAX_LAMPS) as f32, crate::BAKED_SHADOW, l.gain, 0.0];
        // ENHANCEMENT E47: the lamps with a car in their light, the ones whose light on the cars matters most first (LAMP-TUNE: was
        // nearest to the camera first, so the three heads of one post or lamps lighting nothing near the camera took the slots)
        let mut sc = std::mem::take(&mut self.lamp.scratch);
        sc.slots.clear();
        // ENHANCEMENT E49: the environment faces take the lamps' light without their shadow maps (a pass per lamp per face)
        if self.lamp_shadows_on(scene) {
            sc.cars.clear();
            sc.cars.extend(casters.iter().filter(|c| c.car && c.radius <= LAMP_MAX_CASTER_RADIUS));
            let mut cut = 0.0;
            if !sc.cars.is_empty() {
                let cars = &sc.cars;
                sc.ranked.clear();
                sc.ranked.extend(
                    l.lamps
                        .iter()
                        .take(l.n as usize)
                        .enumerate()
                        .filter(|(_, s)| s.color != [0.0; 3] && s.radius > 0.0)
                        .map(|(i, s)| (i, lamp_score(s.pos, s.radius, s.color, camera.position, cars)))
                        .filter(|(_, score)| *score > 0.0),
                );
                // by score, ties by the lamps' order (stable)
                sc.ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
                for &(i, score) in &sc.ranked {
                    let s = &l.lamps[i];
                    if sc.slots.len() == SHADOW_SLOTS {
                        cut = score;
                        break;
                    }
                    let near = || cars.iter().copied().filter(|c| in_reach(s.pos, s.radius, c));
                    if let Some(f) = fit_frustum(s.pos, s.radius, near()) {
                        // (the slot's caster list straight into the kept one)
                        let list = &mut self.lamp.casters[sc.slots.len()];
                        list.clear();
                        list.extend(near().map(|c| c.idx));
                        sc.slots.push((i, f, score));
                    }
                }
            }
            for (k, &(i, f, score)) in sc.slots.iter().enumerate() {
                let p = l.lamps[i].pos;
                let m = spot::shadow_matrix_tan(p, f.dir, f.near, f.far, f.tan_half);
                u.smat[k] = m.to_cols_array_2d();
                u.sinfo[k] = [2.0 * f.tan_half / SHADOW_SIZE as f32, f.near, f.far, slot_strength(score, cut)];
                u.col[i][3] = (k + 1) as f32;
            }
            u.params[3] = 1.0 / SHADOW_SIZE as f32;
            tracing::debug!(slots = ?sc.slots.iter().map(|&(i, _, score)| (l.lamps[i].pos.x.round() as i32, l.lamps[i].pos.z.round() as i32, (score * LOG_SCALE).round() as i32, (slot_strength(score, cut) * LOG_SCALE).round() as i32)).collect::<Vec<_>>(), casters = sc.cars.len(), cut, "lamp shadows (ENHANCEMENT E47)");
        }
        self.lamp.uniform.write(&mut self.belt, encoder, &u);
        let used = sc.slots.len();
        self.lamp.scratch = sc;
        if used == 0 {
            return;
        }
        if self.lamp.map.is_none() {
            self.lamp.map = Some(DepthArray::new(&self.device, "lamp shadow maps (ENHANCEMENT E47)", SHADOW_SIZE, SHADOW_SLOTS as u32));
            self.rebuild_globals_bg();
        }
        for k in 0..used {
            self.lamp.lights.write(&mut self.belt, encoder, k, &u.smat[k]);
        }
        for k in 0..used {
            let map = &self.lamp.map.as_ref().expect("lamp map made").layer_views[k];
            let mut pass = super::shadowmap::depth_pass(encoder, "lamp shadow pass", "lamp shadow", map);
            pass.set_bind_group(0, self.lamp.lights.bind_group(k), &[]);
            self.draw_shadow_casters(&mut pass, scene, &self.lamp.casters[k]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mesh shader's `Lamps` is this uniform, byte for byte in size (its arrays' lengths are emitted from the Rust ones).
    #[test]
    fn the_shader_reads_the_uniform() {
        assert_eq!(crate::classes::tests::mesh_struct_size("Lamps") as usize, std::mem::size_of::<LampUniform>());
    }

    fn car(x: f32, z: f32) -> Caster {
        Caster { idx: 0, centre: Vec3::new(x, 0.7, z), radius: 2.4, car: true, shape: csm::MeshShape::default() }
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
        assert!(!in_reach(lamp, 25.0, &Caster { centre: Vec3::new(3.0, 16.0, 0.0), ..car(0.0, 0.0) }));
        assert!((CONE_WIDTH - (crate::classes::LAMP_CONE_END - CONE_START)).abs() < 1e-6);
        assert_eq!(slot_strength(0.3, 0.0), 1.0);
        assert_eq!(slot_strength(0.4, 0.4), 0.0);
        assert_eq!(slot_strength(0.7, 0.4), 1.0);
        assert!((slot_strength(0.5, 0.4) - 0.5).abs() < 1e-6);
        // the score follows the light at the car: brighter under the lamp, 0 out of its reach, less for a car far from the eye
        let eye = Vec3::new(0.0, 2.0, -6.0);
        let under = lamp_score(lamp, 25.0, [1.0; 3], eye, &[car(2.0, 0.0)]);
        let edge = lamp_score(lamp, 25.0, [1.0; 3], eye, &[car(20.0, 0.0)]);
        assert!(under > edge && edge > 0.0, "{under} {edge}");
        assert_eq!(lamp_score(lamp, 25.0, [1.0; 3], eye, &[car(40.0, 0.0)]), 0.0);
        let far_eye = Vec3::new(0.0, 2.0, -120.0);
        assert!(lamp_score(lamp, 25.0, [1.0; 3], far_eye, &[car(2.0, 0.0)]) < under);
    }
}
