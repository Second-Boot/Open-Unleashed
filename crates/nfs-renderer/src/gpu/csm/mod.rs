//! ENHANCEMENT E9: cascaded shadow maps from the sun.
//!
//! What it does. The sun the cars are lit with (`-SkyBin::sun_position`, the `R3DLightLayer`
//! direction of 0x43AC8D, passed in [`crate::SunShadow`]) casts shadows from the cars (player,
//! opponents, traffic, police: every opaque item with a car light) and from roadside scenery
//! ([`crate::CSM_CAST`] items: non-drivable track article parts, instance props, smackables,
//! breakables) into a depth map, cascaded over the camera's view depth. The main pass reads it
//! (`shader.wgsl`, `csm_visibility`) and darkens only the **direct** share of the light:
//!
//! * a car: the `R3DLightLayer`'s directional term, `min(ambient + diffuse · n·l · visibility, 1)`;
//!   the ambient slot (the ground patch colour × the `.map` ambient) is untouched;
//! * the track (no direct term of its own — its colour is baked vertex light × texture, no
//!   `R3DLightLayer`): the product is scaled by `1 − s · (1 − visibility)`, `s` = [`GROUND_STRENGTH`]
//!   × how much the surface faces the sun (a face turned away is already dark in the bake) × how bright
//!   the baked colour is (an already baked shadow is not darkened twice). The track itself casts nothing.
//!
//! The original's `sh80` blob under the car is left as it is, drawn over the result as before. Not
//! used at night (a night track has no sun to cast with), in the rear-view mirror, in the front end.
//!
//! Stable cascades: each slice of the view frustum is fitted with a sphere whose radius depends on the
//! field of view, the aspect and the split only (not on the camera's heading), and the box's centre
//! is snapped to whole shadow texels in light space, so the map does not swim or shimmer as the
//! camera turns or moves at 64 Hz or between ticks.
//!
//! SHADOW-REFACTOR: one frame of cascades has ONE [`ShadowMode`] (the sky's sun or a sunless track's full data sun, E51's
//! cars-only data sun, E50's moon, a front-end car's focus) picked from [`crate::ShadowLights`] by
//! [`ShadowMode::pick`], and ONE [`CarLayers`] (E51's cars-only layers); the shader reads the mode as a class constant
//! (`misc.y`, `classes::SHADOW_*`) instead of decoding it from the kernel spacing and the strengths.

use super::shadowmap::{Caster, DepthArray, LightMats, UniformSlot};
use super::*;
use crate::{classes, gfx};
use bytemuck::Zeroable;
use glam::{Mat4, Vec3, Vec4};

mod cover;
mod fit;
mod mode;
mod shape;

pub(super) use fit::*;
pub(super) use mode::*;
pub(super) use shape::{mesh_bounds, MeshShape};
pub(crate) use shape::{part_casts, part_casts_vertices};
pub(super) use cover::MAX_COVER;
use cover::{COVER_BYTES, COVER_VEC4S};

/// How far toward the sun a caster may stand and still shade a cascade (m), beyond the cascade's sphere.
const CASTER_REACH: f32 = 120.0;
/// A light direction shorter than this (squared) is none: no frame.
const MIN_DIRECTION_SQ: f32 = 0.5;
/// ENHANCEMENT E9 (SHADOWFIX): how fast the cover gate eases toward its target, per second (fully in or out in 0.5 s). SHADOW-REAL:
/// only a fallback now: the race holds the gate open while it hands the covered boxes (`SunShadow::lit`), and the shader decides
/// per surface (`cover_at`), so a tunnel exit has no wall-clock ramp.
const GATE_RATE: f32 = 2.0;
/// ENHANCEMENT E9 (SHADOWFIX): the longest step (s) the gate takes in one frame (a stalled frame does not jump it), and the gate
/// below which the shadows are out (no pass at all).
const GATE_MAX_STEP: f32 = 0.25;
const GATE_SHUT: f32 = 0.002;
/// ENHANCEMENT ENH-FE3D: how far toward the sun a caster may stand and still shade the front-end car's one cascade (m,
/// [`fit_focus`]'s `ext`), beyond its focus sphere.
const FOCUS_CASTER_REACH: f32 = 12.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CsmUniform {
    mats: [[[f32; 4]; 4]; MAX_CASCADES],
    /// per cascade: x a texel (m), y the depth range (m), z the view depth it ends at
    info: [[f32; 4]; MAX_CASCADES],
    /// xyz the light's travel direction, w the cascade count (0 = off)
    dir: [f32; 4],
    /// [`ShadowMode::params`]
    params: [f32; 4],
    /// ENHANCEMENT E9 (SHADOWFIX): xyz the light the cars are really lit with (the shadow's own is elevated), w the
    /// gate 0..1 (shadows faded out under cover)
    orig: [f32; 4],
    /// ENHANCEMENT ENH-FE3D: the front-end car's soft edge's radius (x) and contact core's radius (y) in texels, the penumbra's
    /// share (z), the `FESH` blob's fade (w)
    extra: [f32; 4],
    /// x (ENHANCEMENT E9, ENH-TUNPOOL) the count of covered boxes below (the nearest to the camera; 0 = none), y the frame's
    /// [`ShadowMode`] class (`classes::SHADOW_*`)
    misc: [f32; 4],
    /// GATE-REL ([`bake_uniform`]): xyz toward the track's bake light, w its base; then x its gain, y the drivable ground's factor
    /// (zw 0). Zero unless the frame's gate is relative.
    bake: [f32; 4],
    bake_k: [f32; 4],
    /// ENHANCEMENT E9 (FOREST-E9PERF): the world box round every covered box below (xyz least, xyz most; empty with no boxes): a
    /// point outside it is in none of them (`cover_at` skips its loop)
    cover_lo: [f32; 4],
    cover_hi: [f32; 4],
    /// per box, 5 vec4: (centre, half length), (forward, half width), (lateral, weight at the back face),
    /// (up, weight at the front face), (ceiling height, nominal half length, 0, 0)
    cover: [[f32; 4]; MAX_COVER * COVER_VEC4S],
    /// ENHANCEMENT E50: x the moon's track strength (0 = not a moon frame), y the cars' cold term, z the soft edge's radius in texels
    moon: [f32; 4],
    /// ENHANCEMENT E50: rgb the moon's colour
    moon_col: [f32; 4],
    /// ENHANCEMENT E51: x how many near cascades have a cars-only layer (0 = none), y the first of those layers in the map,
    /// z 1 when each car's shadow is scaled by its share of the sun (level 1, CARSH-FADE), w how many cars `carsh_cars` lists
    carsh: [f32; 4],
    /// ENHANCEMENT E51 (CARSH-FADE): per measured car its body's centre (xyz) and the radius (w) within which a receiver's ray
    /// toward the sun passes the car
    carsh_cars: [[f32; 4]; crate::MAX_CONTACTS],
    /// ENHANCEMENT E51 (CARSH-FADE): the cars' eased shares of their own shadow (1 all .. 0 none), four to a vec4; written on the
    /// GPU by `carsun.rs` (copied in after the shadow passes)
    carsh_k: [[f32; 4]; crate::MAX_CONTACTS / 4],
}

/// The depth-only shadow passes' shader (E9's cascades, E15's headlight map, E47's lamp maps): the items' `struct Object`
/// (`classes.rs` `OBJECT_WGSL`), then `shadow.wgsl`.
pub(crate) fn shadow_source() -> String {
    crate::wgsl::compose(&[crate::wgsl::BYTE, crate::classes::OBJECT_WGSL, include_str!("../shadow.wgsl")])
}

/// Everything E9 keeps: the pass's pipelines, the uniform the main shader reads, the map.
pub(super) struct CsmState {
    pub(super) pipe_opaque: wgpu::RenderPipeline,
    pub(super) pipe_cut: wgpu::RenderPipeline,
    /// the one-matrix light group the depth-only pipelines read (E15's and E47's maps use it too)
    pub(super) light_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub uniform: UniformSlot<CsmUniform>,
    lights: LightMats,
    dummy_view: wgpu::TextureView,
    pub map: Option<DepthArray>,
    /// ENHANCEMENT E9 (SHADOWFIX): the eased cover gate (1 = in the open) and when it was last stepped
    gate: f32,
    gate_at: Option<std::time::Instant>,
    /// ENHANCEMENT ENH-FE3D: this frame's shadow is a front-end car's (`SunShadow::focus`)
    focus: bool,
    /// a cascade's caster list, kept between frames (no allocation per pass)
    list: Vec<u32>,
    /// the frame's casters the mode's rule keeps ([`part_casts`]), kept between frames
    ours: Vec<Caster>,
    /// ENHANCEMENT E9 (ENH-TUNPOOL): the covered boxes in reach this frame, (distance squared, index), kept between frames
    cover_near: Vec<(f32, usize)>,
}

/// What a frame with shadows needs, from [`Core::csm_frame`].
pub(super) struct CsmFrame {
    cascades: Vec<Cascade>,
    dir: Vec3,
    /// ENHANCEMENT E9 (SHADOWFIX): the light the cars are lit with, and the eased cover gate
    orig: Vec3,
    gate: f32,
    mode: ShadowMode,
    /// ENHANCEMENT E51: the cars-only layers and how many near cascades have one
    car_layers: CarLayers,
    car_cascades: usize,
}

impl CsmFrame {
    /// ENHANCEMENT ENH-DBLSH: whether a track's `sh80` decals give way to this frame's shadows ([`ShadowMode::decal_fades`])
    pub(super) fn decal_fades(&self) -> bool {
        self.mode.decal_fades()
    }

    /// ENHANCEMENT E50: whether this is the moon's frame (the mesh shader's MOON part runs)
    pub(super) fn is_moon(&self) -> bool {
        matches!(self.mode, ShadowMode::Moon { .. })
    }

    /// ENHANCEMENT ENH-FE3D: whether this is a front-end car's focus frame (the mesh shader's FOCUS part runs)
    pub(super) fn is_focus(&self) -> bool {
        matches!(self.mode, ShadowMode::Focus(_))
    }

    /// ENHANCEMENT E51: whether this frame has the cars-only layers (the mesh shader's CARSH part runs)
    pub(super) fn has_car_layers(&self) -> bool {
        self.car_layers != CarLayers::None
    }

    /// ENHANCEMENT E9: how much of the car's `SHAD` patch the sun's shadow takes in the open (the cover gate; 0 where the mode's car
    /// shadow does not stand in for it, [`ShadowMode::takes_car_patch`]). SHADOW-REAL: the fill scales it by the patch's own
    /// openness (`DrawItem::cover`), so a car in a tunnel keeps its patch whatever the camera does.
    pub(super) fn car_patch_gate(&self) -> f32 {
        if self.mode.takes_car_patch() { self.gate } else { 0.0 }
    }
}

impl CsmState {
    pub(super) fn new(device: &wgpu::Device, object_layout: &wgpu::BindGroupLayout, material_layout: &wgpu::BindGroupLayout) -> Self {
        let light_layout = gfx::bgl(device, "shadow light", &[gfx::bgl_uniform(0, wgpu::ShaderStages::VERTEX)]);
        let layout = gfx::pipeline_layout(device, "shadow pipeline layout", &[&light_layout, object_layout, material_layout]);
        let module = gfx::wgsl_module(device, "shadow shader", &shadow_source());
        let make = |label: &str, fragment: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
                    })],
                },
                fragment: fragment.then_some(wgpu::FragmentState { module: &module, entry_point: Some("fs_cut"), compilation_options: Default::default(), targets: &[] }),
                // both faces cast (thin foliage, open meshes); the receiver's normal offset and this slope-scaled bias keep the surfaces clean
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 },
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let (pipe_opaque, pipe_cut) = (make("shadow opaque", false), make("shadow cut-out", true));
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let lights = LightMats::new(device, &light_layout, "shadow light vp", MAX_CASCADES);
        let dummy_view = DepthArray::placeholder(device, "shadow map placeholder");
        Self { pipe_opaque, pipe_cut, light_layout, sampler, uniform: UniformSlot::new(device, "csm uniform"), lights, dummy_view, map: None, gate: 1.0, gate_at: None, focus: false, list: Vec::new(), ours: Vec::new(), cover_near: Vec::new() }
    }

    /// The view the main shader binds (the placeholder until a map exists).
    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.array_view)
    }
}

/// The three bindings E9 adds to the globals group (the map, the comparison sampler, the uniform).
pub(super) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    use crate::classes::binding as b;
    [super::shadowmap::array_layout_entry(b::CSM_MAP), gfx::bgl_sampler(b::CSM_SAMPLER, wgpu::SamplerBindingType::Comparison), gfx::bgl_uniform(b::CSM, wgpu::ShaderStages::FRAGMENT)]
}

impl Core {
    /// The cascades of this frame, or `None` when no shadows are drawn: E9 off, no light the switches allow ([`ShadowMode::pick`]),
    /// the cover gate shut, or the rear-view mirror's pass. Makes the map when the level's size or layers changed.
    pub(super) fn csm_frame(&mut self, scene: &Scene, camera: &Camera, aspect: f32) -> Option<CsmFrame> {
        self.csm.focus = scene.shadows.sun.is_some_and(|s| s.focus.is_some());
        if !self.enhancements.is_on(crate::E9) || self.modes.no_reflections {
            return None;
        }
        let (sun, mode) = ShadowMode::pick(&scene.shadows, &self.enhancements)?;
        if sun.direction.length_squared() < MIN_DIRECTION_SQ {
            return None;
        }
        // ENHANCEMENT E9 (SHADOWFIX): the cover gate eases (0.5 s) toward its target; fully out = no pass at all
        let now = std::time::Instant::now();
        let dt = self.csm.gate_at.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(GATE_MAX_STEP));
        self.csm.gate_at = Some(now);
        let target = if sun.lit { 1.0 } else { 0.0 };
        self.csm.gate += (target - self.csm.gate).clamp(-dt * GATE_RATE, dt * GATE_RATE);
        if self.csm.gate <= GATE_SHUT {
            return None;
        }
        // ENHANCEMENT E9 / E51 (SHADOW-OVERCAST, "overcast during precipitation: no direct sun"): the sun's shadows are drawn at
        // `1 - precipitation` of their share, through the gate every part of them reads (`csm_share`, the car's `SHAD` patch); fully
        // overcast = no pass. The eased gate itself is not touched (the precipitation is smooth along the road already).
        let gate = if mode.fades_with_precipitation() { self.csm.gate * (1.0 - scene.shadows.precipitation.clamp(0.0, 1.0)) } else { self.csm.gate };
        if gate <= GATE_SHUT {
            return None;
        }
        // ENHANCEMENT E9 (SHADOWFIX): a low sun (coastal's is 3 degrees up) throws shadows hundreds of metres long: hills and
        // the tunnel's mass would blacken whole stretches with hard edges where the map ends. The shadow's own light is
        // lifted to at least `MIN_ELEVATION` over the horizon, keeping its azimuth; the cars' shading keeps the original's.
        let orig = sun.direction.normalize();
        let shadow_dir = lift_elevation(orig);
        let (size, splits) = level_config(self.enhancements.level(crate::E9));
        // ENHANCEMENT E51: a sun frame of the race with E51 on has a cars-only layer per near cascade after the cascades' own; the
        // cars-only frame (CORSICA-SUN) keeps the near cascades only (E51's own reach: a car's shadow farther away is a few pixels),
        // and they are the cars-only layers themselves
        let e51 = self.enhancements.is_on(crate::E51);
        let car_layers = match mode {
            ShadowMode::CarsOnly { .. } => CarLayers::Own,
            // (CARSH-FADE: level 1 asks per car how much of the sun reaches it past the scenery; without compute shaders it draws
            // as level 2)
            ShadowMode::Sun { .. } if e51 => CarLayers::Extra { sun_share: self.compute_supported && self.enhancements.level(crate::E51) == crate::Enhancements::E51_WHERE_SUN },
            _ => CarLayers::None,
        };
        let splits = if car_layers == CarLayers::Own { &splits[..CAR_CASCADES.min(splits.len())] } else { splits };
        // ENHANCEMENT ENH-FE3D: a front-end car's volume: one cascade round the car
        let cascades = match mode {
            ShadowMode::Focus(f) => vec![fit_focus(f.centre, f.radius, shadow_dir, size, FOCUS_CASTER_REACH)],
            _ => fit(camera, aspect, shadow_dir, splits, size, CASTER_REACH),
        };
        if cascades.is_empty() {
            return None;
        }
        let car_cascades = if car_layers == CarLayers::None { 0 } else { CAR_CASCADES.min(splits.len()) };
        let layers = match (mode, car_layers) {
            (ShadowMode::Focus(_), _) => 1,
            (_, CarLayers::Extra { .. }) => (splits.len() + car_cascades) as u32,
            _ => splits.len() as u32,
        };
        if self.csm.map.as_ref().is_none_or(|m| m.size != size || m.layers != layers) {
            self.csm.map = Some(DepthArray::new(&self.device, "sun shadow map (ENHANCEMENT E9)", size, layers));
            self.rebuild_globals_bg();
        }
        Some(CsmFrame { cascades, dir: shadow_dir, orig, gate, mode, car_layers, car_cascades })
    }

    /// An item the shadow passes draw: a car's opaque part (its own light's direction is the sun's),
    /// or scenery flagged [`crate::CSM_CAST`]; never a sky / overlay / additive / ground-decal item.
    pub(super) fn csm_caster(&self, item: &crate::DrawItem) -> bool {
        let Some(mat) = self.res.materials.get(item.material.0 as usize) else { return false };
        if item.overlay || item.sky_pass || !mat.depth_test || item.alpha < 0.5 || mat.blend_pair != 0 {
            return false;
        }
        // ENHANCEMENT ENH-FE3D: a front-end room's own pieces never cast
        if item.csm & crate::CSM_ROOM != 0 {
            return false;
        }
        let car = item.car_lit();
        if !car && item.csm & crate::CSM_CAST == 0 {
            return false;
        }
        // a blended ground decal (the `S_Normal` shadow, depth-biased) is not an object; `early` marks one only in a race: the front end
        // submits every item early (its materials all blend), its car's body included, and its blob quad is a flat mesh
        !(mat.alpha_blend && (mat.depth_bias != 0 || item.depth_bias != 0 || item.early && car && !self.csm.focus))
    }

    /// What an item does with the shadow map in the main pass, for `ObjectUniform::lit_d.w`: 1 a car
    /// (the direct term is scaled), 2 a track item (the baked colour is scaled), 0 neither.
    pub(super) fn csm_receive(&self, item: &crate::DrawItem) -> f32 {
        use crate::classes::{RECV_CAR, RECV_NONE, RECV_TRACK};
        let Some(mat) = self.res.materials.get(item.material.0 as usize) else { return RECV_NONE };
        let plain = mat.depth_test && !item.sky_pass && !item.overlay;
        // ENHANCEMENT ENH-FE3D: a front-end room's piece takes the shadow like the track (its baked / lit colour scaled)
        if item.csm & crate::CSM_ROOM != 0 {
            // ENHANCEMENT ENH-GARAGE: the garage's lift platform (`CarIntShd`, the material that carries the `FESH` projector)
            // is a blend-pair-1 material: it takes the shadow too, or the car's contact on it is the blob alone
            return if plain && (mat.blend_pair == 0 || item.shadow.is_some()) { RECV_TRACK } else { RECV_NONE };
        }
        if item.car_lit() {
            return if item.overlay || item.sky_pass || mat.blend_pair != 0 { RECV_NONE } else { RECV_CAR };
        }
        if item.csm & crate::CSM_RECEIVE != 0 && plain && mat.blend_pair == 0 {
            return RECV_TRACK;
        }
        RECV_NONE
    }

    /// Write the frame's uniform and draw the cascades' casters (`casters`: the frame's, [`Core::frame_casters`]). Without a frame
    /// the uniform is set to "off" once if it was on.
    pub(super) fn csm_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, casters: &[Caster], camera: &Camera, vp: Mat4, frame: Option<&CsmFrame>) {
        let Some(frame) = frame else {
            self.csm.uniform.clear(&mut self.belt, encoder);
            return;
        };
        let cascades = &frame.cascades;
        let mut u = fill_uniform(frame, &scene.shadows);
        // ENHANCEMENT E51: the cars-only layers follow the cascades' own in the map (CORSICA-SUN: the cars-only frame's cascades are
        // the cars-only layers: the shader's `car_visibility` reads them from 0)
        let car_layers = frame.car_cascades.min(cascades.len());
        let car_base = match frame.car_layers {
            CarLayers::Extra { .. } => self.csm.map.as_ref().map_or(0, |m| m.layers as usize - frame.car_cascades),
            _ => 0,
        };
        // ENHANCEMENT E51 (CARSH-FADE): level 1: the measured cars (their shares come from the compute after the passes)
        let mut sun_cars = 0;
        if car_layers > 0 {
            let sun_share = frame.car_layers == CarLayers::Extra { sun_share: true };
            if sun_share {
                let cs = self.car_sun.get_or_insert_with(|| super::carsun::CarSun::new(&self.device));
                sun_cars = cs.prepare(carsun::Measure { belt: &mut self.belt, encoder: &mut *encoder, cascades, view: camera.view(), cars: &scene.sun_cars, car_layers, car_base }, &mut u.carsh_cars);
                u.carsh_k = [[1.0; 4]; crate::MAX_CONTACTS / 4];
            }
            u.carsh = [car_layers as f32, car_base as f32, f32::from(u8::from(sun_share)), sun_cars as f32];
        }
        if !scene.cover.is_empty() && !matches!(frame.mode, ShadowMode::Focus(_)) {
            cover::write(&mut u, &scene.cover, camera.position, vp, &mut self.csm.cover_near);
        }
        // PERF (picture identical): the covered boxes past the count are not written (`cover_at` reads `misc.x` of them; the
        // rest of the 5 KB keep an earlier frame's)
        let used = std::mem::offset_of!(CsmUniform, cover) + u.misc[0] as usize * COVER_BYTES;
        self.csm.uniform.write_except(&mut self.belt, encoder, &u, used..std::mem::offset_of!(CsmUniform, moon));
        for (i, c) in cascades.iter().enumerate() {
            self.csm.lights.write(&mut self.belt, encoder, i, &c.mat.to_cols_array_2d());
        }

        // the cascades, then (ENHANCEMENT E51) the near ones again with the cars alone into the cars-only layers (CORSICA-SUN: the
        // cars-only frame draws its cascades with the cars alone and has no extra layers)
        let own_cars = frame.car_layers == CarLayers::Own;
        let extra = if matches!(frame.car_layers, CarLayers::Extra { .. }) { car_layers } else { 0 };
        let passes = (0..cascades.len()).map(|ci| (ci, ci, own_cars)).chain((0..extra).map(|ci| (ci, car_base + ci, true)));
        let axes = { let (ax, ay) = light_axes(frame.dir); (ax, ay, frame.dir) };
        // (the mode's casters: E9 leaves the flat sheets out and, SHADOW-UNIFY, the terrain-like broad parts, `part_casts`)
        let rule = frame.mode.casters();
        // (CANYON-SMALLSH: the cars-only frame's scenery is the small parts alone: a car casts as before, a scenery part by `Small`)
        let small = matches!(frame.mode, ShadowMode::CarsOnly { small: Some(_) });
        // PERF (picture identical): the rule is the frame's, so each caster is asked once, not once per cascade pass (4 + 2 of them)
        let mut ours = std::mem::take(&mut self.csm.ours);
        ours.clear();
        ours.extend(casters.iter().copied().filter(|k| part_casts(rule, k.radius, k.shape)));
        let mut list = std::mem::take(&mut self.csm.list);
        for (ci, layer, cars_only) in passes {
            let c = &cascades[ci];
            list.clear();
            list.extend(ours.iter().filter(|k| (k.car || !cars_only || small) && c.reaches(axes, k)).map(|k| k.idx));
            let map = self.csm.map.as_mut().expect("shadow map made");
            // PERF (SHADOW-REFACTOR): a layer the last frame left cleared, with no caster now, holds what this pass would write
            // (E51's far cars-only layer with no car in it, most frames)
            if !map.needs_pass(layer, list.is_empty()) {
                continue;
            }
            let map = &*map;
            let (label, prof) = if cars_only { ("sun shadow cars-only pass", ["csm cars 0", "csm cars 1+"][ci.min(1)]) } else { ("sun shadow pass", ["csm cascade 0", "csm cascade 1", "csm cascade 2", "csm cascade 3+"][ci.min(3)]) };
            let mut pass = super::shadowmap::depth_pass(encoder, label, prof, &map.layer_views[layer]);
            pass.set_bind_group(0, self.csm.lights.bind_group(ci), &[]);
            self.draw_shadow_casters(&mut pass, scene, &list);
        }
        self.csm.list = list;
        self.csm.ours = ours;
        // ENHANCEMENT E51 (CARSH-FADE): the cars' shares of the sun, from the map just drawn, into the uniform the main pass reads
        if sun_cars > 0
            && let (Some(cs), Some(map)) = (self.car_sun.as_mut(), self.csm.map.as_ref())
        {
            let offset = std::mem::offset_of!(CsmUniform, carsh_k) as u64;
            cs.record(&self.device, encoder, sun_cars, (&map.array_view, &self.csm.sampler), (&self.csm.uniform.buf, offset));
        }
    }
}

/// The frame's uniform but the parts that need the scene's cars / covered boxes (the cars' share of the sun, E51; the cover, E9):
/// the cascades' matrices and ends, the light, the mode's strengths and class, E9's floor and GATE-REL's bake, E50's moon and the
/// front end's focus. Pure: the frame and the scene's shadow lights decide it.
fn fill_uniform(frame: &CsmFrame, shadows: &crate::ShadowLights) -> CsmUniform {
    let cascades = &frame.cascades;
    let mut u = CsmUniform::zeroed();
    for (i, c) in cascades.iter().enumerate() {
        u.mats[i] = c.mat.to_cols_array_2d();
        u.info[i] = [c.texel, c.range, c.split, 0.0];
    }
    u.dir = [frame.dir.x, frame.dir.y, frame.dir.z, cascades.len() as f32];
    u.orig = [frame.orig.x, frame.orig.y, frame.orig.z, frame.gate];
    u.params = frame.mode.params(cascades.last().map_or(0.0, |c| c.split));
    // ENHANCEMENT E9 (SHADOW-FLOOR): a sun frame's darkness held to the bake's own ambient share
    if matches!(frame.mode, ShadowMode::Sun { .. } | ShadowMode::CarsOnly { .. }) {
        u.params[0] = floored_strength(u.params[0], shadows.bake.as_ref());
    }
    u.misc[1] = frame.mode.shader_class();
    if let ShadowMode::Sun { baked_gate: crate::BakedGate::Relative, .. } = frame.mode {
        [u.bake, u.bake_k] = bake_uniform(shadows.bake.as_ref());
    }
    match frame.mode {
        // ENHANCEMENT ENH-FE3D: the front-end car's shadow: a soft jittered disc and (ENH-GARAGE) a contact core
        ShadowMode::Focus(f) => u.extra = [f.blur / cascades[0].texel, f.core / cascades[0].texel, f.soft, f.blob_fade],
        // ENHANCEMENT E50: the moon: its own strength and soft disc, a cold term on the cars
        ShadowMode::Moon { level } => {
            let l = usize::from(level);
            u.moon = [MOON_STRENGTH[l], MOON_CAR[l], MOON_SOFT, 0.0];
            u.moon_col = [MOON_COLOUR[0], MOON_COLOUR[1], MOON_COLOUR[2], 0.0];
        }
        _ => {}
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mesh shader's `Csm` is this uniform, byte for byte in size.
    #[test]
    fn the_shader_reads_the_uniform() {
        assert_eq!(crate::classes::tests::mesh_struct_size("Csm") as usize, std::mem::size_of::<CsmUniform>());
    }

    /// The frame's uniform from a frame: the cascades' own fields, the mode's class and params, E50's moon, E9's floor.
    #[test]
    fn the_uniform_follows_the_frame() {
        let cascade = fit_focus(Vec3::new(1.0, 0.0, 2.0), 3.0, Vec3::new(0.0, -1.0, 0.2), 1024, 12.0);
        let frame = |mode| CsmFrame { cascades: vec![cascade], dir: Vec3::new(0.0, -1.0, 0.0), orig: Vec3::new(0.1, -0.9, 0.0), gate: 0.75, mode, car_layers: CarLayers::None, car_cascades: 0 };
        let none = crate::ShadowLights::NONE;
        let u = fill_uniform(&frame(ShadowMode::Moon { level: 2 }), &none);
        assert_eq!(u.dir, [0.0, -1.0, 0.0, 1.0], "the light and the cascade count");
        assert_eq!(u.orig, [0.1, -0.9, 0.0, 0.75], "the cars' light and the gate");
        assert_eq!(u.info[0], [cascade.texel, cascade.range, cascade.split, 0.0]);
        assert_eq!(u.mats[0], cascade.mat.to_cols_array_2d());
        assert_eq!(u.misc[1], crate::classes::SHADOW_MOON);
        assert_eq!(u.params, ShadowMode::Moon { level: 2 }.params(cascade.split));
        assert_eq!((u.moon, u.moon_col), ([MOON_STRENGTH[2], MOON_CAR[2], MOON_SOFT, 0.0], [MOON_COLOUR[0], MOON_COLOUR[1], MOON_COLOUR[2], 0.0]));
        let sun = ShadowMode::Sun { casters: crate::SceneryCasters::Shape, baked_gate: crate::BakedGate::Absolute };
        let u = fill_uniform(&frame(sun), &none);
        assert_eq!((u.misc[1], u.moon), (crate::classes::SHADOW_SUN, [0.0; 4]), "no moon in a sun frame");
        assert_eq!(u.params[0], GROUND_STRENGTH, "no bake light: no floor");
        // E9's floor: a bake with much ambient light holds the darkness down
        let bake = crate::BakeLight { to_light: Vec3::Y, base: 0.6, gain: 0.2, ground: 1.0 };
        let u = fill_uniform(&frame(sun), &crate::ShadowLights { bake: Some(bake), ..none });
        assert!(u.params[0] < GROUND_STRENGTH);
        let focus = ShadowMode::Focus(crate::ShadowFocus { centre: Vec3::ZERO, radius: 2.0, strength: 0.5, blur: 4.0, core: 2.0, soft: 0.3, blob_fade: 0.1 });
        let u = fill_uniform(&frame(focus), &none);
        assert_eq!(u.extra, [4.0 / cascade.texel, 2.0 / cascade.texel, 0.3, 0.1]);
    }
}
