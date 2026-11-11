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

use crate::gfx;
use super::*;
use bytemuck::Zeroable;
use glam::{Mat4, Vec3, Vec4};

/// How many cascades the maps hold at most (the shader's arrays).
pub(super) const MAX_CASCADES: usize = 4;
/// How strongly a shadow darkens the track's baked colour at full visibility loss (0..1): the track has
/// no separate direct light, so this stands for the sun's share of the baked brightness.
pub(super) const GROUND_STRENGTH: f32 = 0.6;
/// ENHANCEMENT E25: the overcast variant's track strength, kernel spacing (texels, of a 5 x 5 grid) and car floor.
pub(super) const OVERCAST_STRENGTH: f32 = 0.6;
pub(super) const OVERCAST_SOFT: f32 = 4.0;
pub(super) const OVERCAST_FLOOR: f32 = 0.45;
/// How far toward the sun a caster may stand and still shade a cascade (m), beyond the cascade's sphere.
const CASTER_REACH: f32 = 120.0;
/// A caster whose bounding sphere is larger than this (m) is left out: a hillside or a cliff face would
/// fill the map with geometry whose shade the track's own baked light already carries.
const MAX_CASTER_RADIUS: f32 = 60.0;

/// ENHANCEMENT ENH-SHPOP: where the last cascade starts to fade out, as a share of its end (was 0.85: a 33 m ramp at 220 m)
const FADE_START: f32 = 0.6;

/// ENHANCEMENT E9 (SHADOWFIX): how fast the cover gate eases toward its target, per second (fully in or out in 0.5 s). SHADOW-REAL:
/// only a fallback now: the race holds the gate open while it hands the covered boxes (`SunShadow::lit`), and the shader decides
/// per surface (`cover_at`), so a tunnel exit has no wall-clock ramp.
const GATE_RATE: f32 = 2.0;
/// ENHANCEMENT ENH-FE3D: how far toward the sun a caster may stand and still shade the front-end car's one cascade (m,
/// [`fit_focus`]'s `ext`), beyond its focus sphere.
const FOCUS_CASTER_REACH: f32 = 12.0;
/// ENHANCEMENT ENH-FE3D: the front-end cascade's fade start, as a share of its split (`params.y`), and its tap spacing in
/// texels (`params.z`: the soft jittered disc instead of the race's 3 x 3 kernel).
const FOCUS_FADE_START: f32 = 0.85;
const FOCUS_TAP_SPACING: f32 = 2.0;

/// The level's map size, cascade ends (view depth, m).
pub(super) fn level_config(level: u8) -> (u32, &'static [f32]) {
    match level {
        // ENHANCEMENT ENH-SHPOP: the last cascade reaches far (the old 140 / 220 m ends made shadows pop in as they came in range)
        1 => (1024, &[40.0, 140.0, 400.0]),
        _ => (2048, &[25.0, 80.0, 220.0, 520.0]),
    }
}

/// One fitted cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cascade {
    /// world → the map's clip space (x, y in -1..1, z in 0..1 along the light)
    pub mat: Mat4,
    /// a texel's size in the world (m)
    pub texel: f32,
    /// the depth range the 0..1 of z covers (m)
    pub range: f32,
    /// the box's half width (m) and its centre in light space (x, y, and the near depth)
    pub half: f32,
    pub centre: [f32; 2],
    pub zmin: f32,
    /// the view depth where this cascade ends
    pub split: f32,
}

/// The light's axes for a travel direction `d` (unit): the map's x and y, deterministic in `d`.
pub(super) fn light_axes(d: Vec3) -> (Vec3, Vec3) {
    let r = if d.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let x = r.cross(d).normalize();
    let y = d.cross(x).normalize();
    (x, y)
}

/// ENHANCEMENT ENH-FE3D: the one cascade of a front-end car's shadow — a sphere round the car, the box snapped to whole
/// texels like [`fit`]'s; its split is far beyond any view depth, so the map is read for every pixel (the cascade's own
/// box says where a shadow can fall, outside it the point is lit).
pub(super) fn fit_focus(centre: Vec3, radius: f32, dir: Vec3, size: u32, ext: f32) -> Cascade {
    let d = dir.normalize();
    let (ax, ay) = light_axes(d);
    let r = (radius * 8.0).ceil() / 8.0;
    let texel = 2.0 * r / size as f32;
    let (lx, ly, lz) = (ax.dot(centre), ay.dot(centre), d.dot(centre));
    let (cx, cy) = ((lx / texel).round() * texel, (ly / texel).round() * texel);
    let range = (2.0 * r + ext).ceil() + 2.0;
    let zmin = (lz - r - ext).floor();
    let m = Mat4::from_cols(
        Vec4::new(ax.x / r, ay.x / r, d.x / range, 0.0),
        Vec4::new(ax.y / r, ay.y / r, d.y / range, 0.0),
        Vec4::new(ax.z / r, ay.z / r, d.z / range, 0.0),
        Vec4::new(-cx / r, -cy / r, -zmin / range, 1.0),
    );
    Cascade { mat: m, texel, range, half: r, centre: [cx, cy], zmin, split: 1.0e6 }
}

/// ENHANCEMENT E9 (SHADOWFIX): the lowest elevation the shadows' light has over the horizon (degrees).
const MIN_ELEVATION_DEG: f32 = 30.0;

/// `d` (a travel direction) with its elevation raised to at least [`MIN_ELEVATION_DEG`], azimuth kept.
pub(super) fn lift_elevation(d: Vec3) -> Vec3 {
    let (s, c) = MIN_ELEVATION_DEG.to_radians().sin_cos();
    let h = Vec3::new(d.x, 0.0, d.z);
    if -d.y >= s || h.length_squared() < 1e-6 {
        return d;
    }
    let h = h.normalize() * c;
    Vec3::new(h.x, -s, h.z)
}

/// The cascade's world -> map matrix for the light axes of `d`: the box `half` wide round `centre`, depth
/// `zmin .. zmin + range` along the light.
fn cascade_matrix(d: Vec3, half: f32, centre: [f32; 2], zmin: f32, range: f32) -> Mat4 {
    let (ax, ay) = light_axes(d);
    let r = half;
    Mat4::from_cols(
        Vec4::new(ax.x / r, ay.x / r, d.x / range, 0.0),
        Vec4::new(ax.y / r, ay.y / r, d.y / range, 0.0),
        Vec4::new(ax.z / r, ay.z / r, d.z / range, 0.0),
        Vec4::new(-centre[0] / r, -centre[1] / r, -zmin / range, 1.0),
    )
}

/// The cascades for the camera: slice `[prev end, end]` of the view depth each, a sphere around the
/// slice, the box snapped to texels. `ext` is the depth reserved toward the sun for casters.
pub(super) fn fit(camera: &Camera, aspect: f32, dir: Vec3, splits: &[f32], size: u32, ext: f32) -> Vec<Cascade> {
    let d = dir.normalize();
    let (ax, ay) = light_axes(d);
    let fwd = (camera.target - camera.position).normalize();
    let ty = (camera.fov_y_radians * 0.5).tan();
    let tx = ty * aspect;
    let k2 = tx * tx + ty * ty;
    let mut out = Vec::new();
    let mut near = camera.near;
    for &end in splits {
        let far = end.min(camera.far);
        if far <= near + 0.25 {
            break;
        }
        // the smallest sphere on the view axis holding the slice's eight corners (centre at depth
        // `(n + f)(1 + k²)/2`, held at the far plane's centre when that lies beyond it)
        let c = (0.5 * (near + far) * (1.0 + k2)).min(far);
        let r = (far * far * k2 + (far - c) * (far - c)).sqrt();
        // a radius that depends on nothing the camera's heading changes, rounded up so the box keeps its size
        let r = (r * 8.0).ceil() / 8.0;
        let centre = camera.position + fwd * c;
        let texel = 2.0 * r / size as f32;
        let (lx, ly, lz) = (ax.dot(centre), ay.dot(centre), d.dot(centre));
        let (cx, cy) = ((lx / texel).round() * texel, (ly / texel).round() * texel);
        let range = (2.0 * r + ext).ceil() + 2.0;
        let zmin = (lz - r - ext).floor();
        let m = cascade_matrix(d, r, [cx, cy], zmin, range);
        out.push(Cascade { mat: m, texel, range, half: r, centre: [cx, cy], zmin, split: far });
        near = far;
    }
    out
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct CsmUniform {
    mats: [[[f32; 4]; 4]; MAX_CASCADES],
    /// per cascade: x a texel (m), y the depth range (m), z the view depth it ends at
    info: [[f32; 4]; MAX_CASCADES],
    /// xyz the light's travel direction, w the cascade count (0 = off)
    dir: [f32; 4],
    /// x the track's shadow strength, y where the last cascade starts to fade out (view depth)
    params: [f32; 4],
    /// ENHANCEMENT E9 (SHADOWFIX): xyz the light the cars are really lit with (the shadow's own is elevated), w the
    /// gate 0..1 (shadows faded out under cover)
    orig: [f32; 4],
    /// ENHANCEMENT ENH-FE3D: x the soft edge's radius in texels of the front-end car's shadow (0 = the race's kernels)
    extra: [f32; 4],
    /// ENHANCEMENT E9 (ENH-TUNPOOL): x the count of covered boxes below (the nearest to the camera; 0 = none)
    cover_n: [f32; 4],
    /// per box, 5 vec4: (centre, half length), (forward, half width), (lateral, weight at the back face),
    /// (up, weight at the front face), (ceiling height, nominal half length, 0, 0)
    cover: [[f32; 4]; MAX_COVER * 5],
}

/// The most covered boxes the shader knows (the nearest to the camera go in).
pub(super) const MAX_COVER: usize = 64;
/// Covered boxes farther than this from the camera are left out (m); past it a tunnel's interior is shaded as open ground, which
/// E9's own far fade has reached (the last cascade ends at 400 m / 520 m).
const COVER_REACH: f32 = 420.0;

/// The shadow maps' array texture and its views.
pub(super) struct ShadowMap {
    pub size: u32,
    pub layers: u32,
    _texture: wgpu::Texture,
    pub array_view: wgpu::TextureView,
    pub layer_views: Vec<wgpu::TextureView>,
}

/// Everything E9 keeps: the pass's pipelines, the uniform the main shader reads, the map.
pub(super) struct CsmState {
    pub(super) pipe_opaque: wgpu::RenderPipeline,
    pub(super) pipe_cut: wgpu::RenderPipeline,
    /// the one-matrix light group the depth-only pipelines read (E15's spot map uses it too)
    pub(super) light_layout: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    pub uniform: wgpu::Buffer,
    light_bufs: Vec<wgpu::Buffer>,
    light_bgs: Vec<wgpu::BindGroup>,
    _dummy: wgpu::Texture,
    dummy_view: wgpu::TextureView,
    pub map: Option<ShadowMap>,
    /// the uniform currently says "on" (so the next frame without shadows writes the count 0 once)
    active: bool,
    /// ENHANCEMENT E9 (SHADOWFIX): the eased cover gate (1 = in the open) and when it was last stepped
    gate: f32,
    gate_at: Option<std::time::Instant>,
    /// ENHANCEMENT ENH-FE3D: this frame's shadow is a front-end car's (`SunShadow::focus`)
    focus: bool,
}

/// What a frame with shadows needs, from [`Core::csm_frame`].
pub(super) struct CsmFrame {
    cascades: Vec<Cascade>,
    dir: Vec3,
    /// ENHANCEMENT E9 (SHADOWFIX): the light the cars are lit with, and the eased cover gate
    orig: Vec3,
    gate: f32,
    /// ENHANCEMENT E25: the overcast variant (weaker, softer)
    overcast: bool,
    /// ENHANCEMENT ENH-FE3D: a front-end car's one-cascade soft shadow
    focus: Option<crate::ShadowFocus>,
}

impl CsmFrame {
    /// ENHANCEMENT ENH-DBLSH: whether a track's `sh80` decals give way to this frame's shadows (a sun frame; not the front
    /// end's focus, not E25's overcast light, whose shadows are too faint to stand in for them)
    pub(super) fn decal_fades(&self) -> bool {
        self.focus.is_none() && !self.overcast
    }

    /// ENHANCEMENT E9: how much of the car's `SHAD` patch the sun's shadow takes in the open (the cover gate; 0 in the front end's
    /// focus and under E25's overcast light, whose car shadow is too faint to stand in for it). SHADOW-REAL: the fill scales it by
    /// the patch's own openness (`DrawItem::cover`), so a car in a tunnel keeps its patch whatever the camera does.
    pub(super) fn car_patch_gate(&self) -> f32 {
        if self.focus.is_some() || self.overcast { 0.0 } else { self.gate }
    }
}

impl CsmState {
    pub(super) fn new(device: &wgpu::Device, object_layout: &wgpu::BindGroupLayout, material_layout: &wgpu::BindGroupLayout) -> Self {
        let light_layout = gfx::bgl(device, "shadow light", &[gfx::bgl_uniform(0, wgpu::ShaderStages::VERTEX)]);
        let layout = gfx::pipeline_layout(device, "shadow pipeline layout", &[&light_layout, object_layout, material_layout]);
        let module = gfx::wgsl_module(device, "shadow shader", include_str!("shadow.wgsl"));
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
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("csm uniform"), size: std::mem::size_of::<CsmUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true });
        // zeroed: cascade count 0, the shader's "off"
        uniform.unmap();
        let light_bufs: Vec<wgpu::Buffer> = (0..MAX_CASCADES).map(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some("shadow light vp"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })).collect();
        let light_bgs = light_bufs
            .iter()
            .map(|b| gfx::buffer_bg(device, "shadow light", &light_layout, b))
            .collect();
        let dummy = gfx::texture_2d(device, "shadow map placeholder", 1, 1, DEPTH_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        let dummy_view = dummy.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        Self { pipe_opaque, pipe_cut, light_layout, sampler, uniform, light_bufs, light_bgs, _dummy: dummy, dummy_view, map: None, active: false, gate: 1.0, gate_at: None, focus: false }
    }

    /// The view the main shader binds (the placeholder until a map exists).
    pub(super) fn view(&self) -> &wgpu::TextureView {
        self.map.as_ref().map_or(&self.dummy_view, |m| &m.array_view)
    }
}

/// The three bindings E9 adds to the globals group (6 map, 7 comparison sampler, 8 the uniform).
pub(super) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
            count: None,
        },
        gfx::bgl_sampler(7, wgpu::SamplerBindingType::Comparison),
        gfx::bgl_uniform(8, wgpu::ShaderStages::FRAGMENT),
    ]
}

/// A mesh's bounding sphere (the box's centre, the farthest vertex from it) and whether it is a flat
/// sheet lying level (thinner than half a metre with mostly upward normals: a road layer, a marking, a
/// ground decal), which casts nothing worth the coplanar acne it would give the road under it.
pub(super) fn mesh_bounds(vertices: &[Vertex]) -> ([f32; 3], f32, bool) {
    if vertices.is_empty() {
        return ([0.0; 3], 0.0, false);
    }
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in vertices {
        for i in 0..3 {
            lo[i] = lo[i].min(v.position[i]);
            hi[i] = hi[i].max(v.position[i]);
        }
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let r2 = vertices.iter().map(|v| (0..3).map(|i| (v.position[i] - c[i]) * (v.position[i] - c[i])).sum::<f32>()).fold(0.0, f32::max);
    let up = vertices.iter().map(|v| v.normal[1].abs()).sum::<f32>() / vertices.len() as f32;
    (c, r2.sqrt(), hi[1] - lo[1] < 0.5 && up > 0.85)
}

impl Core {
    /// The cascades of this frame, or `None` when no shadows are drawn: E9 off, no sun in the scene
    /// (the front end, a night track), or the rear-view mirror's pass. Makes the map when the level's
    /// size changed.
    pub(super) fn csm_frame(&mut self, scene: &Scene, camera: &Camera, aspect: f32) -> Option<CsmFrame> {
        self.csm.focus = scene.sun_shadow.is_some_and(|s| s.focus.is_some());
        if !self.enhancements.is_on(crate::E9) || self.no_reflections {
            return None;
        }
        // ENHANCEMENT E25: no sun disc -> the overcast light (straight down), with the soft / faint parameters
        let (sun, overcast) = match scene.sun_shadow {
            Some(s) => (s, false),
            None => (scene.overcast_shadow.filter(|_| self.enhancements.is_on(crate::E25))?, true),
        };
        if sun.direction.length_squared() < 0.5 {
            return None;
        }
        // ENHANCEMENT E9 (SHADOWFIX): the cover gate eases (0.5 s) toward its target; fully out = no pass at all
        let now = std::time::Instant::now();
        let dt = self.csm.gate_at.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.25));
        self.csm.gate_at = Some(now);
        let target = if sun.lit { 1.0 } else { 0.0 };
        self.csm.gate += (target - self.csm.gate).clamp(-dt * GATE_RATE, dt * GATE_RATE);
        if self.csm.gate <= 0.002 {
            return None;
        }
        // ENHANCEMENT E9 (SHADOWFIX): a low sun (coastal's is 3 degrees up) throws shadows hundreds of metres long: hills and
        // the tunnel's mass would blacken whole stretches with hard edges where the map ends. The shadow's own light is
        // lifted to at least `MIN_ELEVATION` over the horizon, keeping its azimuth; the cars' shading keeps the original's.
        let orig = sun.direction.normalize();
        let shadow_dir = lift_elevation(orig);
        let (size, splits) = level_config(self.enhancements.level(crate::E9));
        // ENHANCEMENT ENH-FE3D: a front-end car's volume: one cascade round the car
        let cascades = match sun.focus {
            Some(f) => vec![fit_focus(f.centre, f.radius, shadow_dir, size, FOCUS_CASTER_REACH)],
            None => fit(camera, aspect, shadow_dir, splits, size, CASTER_REACH),
        };
        if cascades.is_empty() {
            return None;
        }
        let layers = if sun.focus.is_some() { 1 } else { splits.len() as u32 };
        if self.csm.map.as_ref().is_none_or(|m| m.size != size || m.layers != layers) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("sun shadow map"),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: layers },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let array_view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
            let layer_views = (0..layers)
                .map(|i| texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i, array_layer_count: Some(1), ..Default::default() }))
                .collect();
            tracing::info!(size, layers, "sun shadow map (ENHANCEMENT E9)");
            self.csm.map = Some(ShadowMap { size, layers, _texture: texture, array_view, layer_views });
            self.rebuild_globals_bg();
        }
        Some(CsmFrame { cascades, dir: shadow_dir, orig, gate: self.csm.gate, overcast, focus: sun.focus })
    }

    /// An item the shadow pass draws: a car's opaque part (its own light's direction is the sun's),
    /// or scenery flagged [`crate::CSM_CAST`]; never a sky / overlay / additive / ground-decal item.
    pub(super) fn csm_caster(&self, item: &crate::DrawItem) -> bool {
        let Some(mat) = self.materials.get(item.material.0 as usize) else { return false };
        if item.overlay || item.sky_pass || !mat.depth_test || item.alpha < 0.5 || mat.blend_pair != 0 {
            return false;
        }
        // ENHANCEMENT ENH-FE3D: a front-end room's own pieces never cast
        if item.csm & crate::CSM_ROOM != 0 {
            return false;
        }
        let car = item.layer_light.is_some_and(|l| l.direction != [0.0; 3]);
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
        let Some(mat) = self.materials.get(item.material.0 as usize) else { return RECV_NONE };
        // ENHANCEMENT ENH-FE3D: a front-end room's piece takes the shadow like the track (its baked / lit colour scaled)
        if item.csm & crate::CSM_ROOM != 0 {
            // ENHANCEMENT ENH-GARAGE: the garage's lift platform (`CarIntShd`, the material that carries the `FESH` projector)
            // is a blend-pair-1 material: it takes the shadow too, or the car's contact on it is the blob alone
            return if mat.depth_test && !item.sky_pass && !item.overlay && (mat.blend_pair == 0 || item.shadow.is_some()) { RECV_TRACK } else { RECV_NONE };
        }
        if item.layer_light.is_some_and(|l| l.direction != [0.0; 3]) {
            return if item.overlay || item.sky_pass || mat.blend_pair != 0 { RECV_NONE } else { RECV_CAR };
        }
        if item.csm & crate::CSM_RECEIVE != 0 && mat.depth_test && !item.sky_pass && !item.overlay && mat.blend_pair == 0 {
            return RECV_TRACK;
        }
        RECV_NONE
    }

    /// Write the frame's uniform and draw the cascades' casters. Without a frame the uniform is set to
    /// "off" once if it was on.
    pub(super) fn csm_pass(&mut self, encoder: &mut wgpu::CommandEncoder, scene: &Scene, n: usize, camera: &Camera, frame: Option<&CsmFrame>) {
        let Some(frame) = frame else {
            if self.csm.active {
                self.csm.active = false;
                let size = wgpu::BufferSize::new(std::mem::size_of::<CsmUniform>() as u64).expect("csm size");
                self.belt.write_buffer(encoder, &self.csm.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&CsmUniform::zeroed()));
            }
            return;
        };
        // the casters: item index, its world bounding sphere (centre, radius), whether it needs the alpha test
        let mut casters: Vec<(u32, Vec3, f32, bool)> = Vec::new();
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
            let mat = &self.materials[item.material.0 as usize];
            casters.push((i as u32, centre, radius, mat.alpha_blend || mat.device_states || item.alpha_test));
        }
        let cascades = &frame.cascades;
        let (ax, ay) = light_axes(frame.dir);
        self.csm.active = true;
        let mut u = CsmUniform::zeroed();
        for (i, c) in cascades.iter().enumerate() {
            u.mats[i] = c.mat.to_cols_array_2d();
            u.info[i] = [c.texel, c.range, c.split, 0.0];
        }
        u.dir = [frame.dir.x, frame.dir.y, frame.dir.z, cascades.len() as f32];
        u.orig = [frame.orig.x, frame.orig.y, frame.orig.z, frame.gate];
        let last = cascades.last().map_or(0.0, |c| c.split);
        // params: x the track's strength, y the fade start, z the tap spacing (0 = the original 3 x 3 at one texel), w the
        // car's floor of the direct term (0 = 0.25); ENHANCEMENT E25 lowers the strength, widens the kernel, lifts the floor
        u.params = if frame.overcast { [OVERCAST_STRENGTH, last * FADE_START, OVERCAST_SOFT, OVERCAST_FLOOR] } else { [GROUND_STRENGTH, last * FADE_START, 0.0, 0.0] };
        // ENHANCEMENT ENH-FE3D: the front-end car's shadow: its own strength, a soft jittered disc, no race kernel; the
        // dark-baked gate of the track's receiving is eased the way the overcast variant's is (the rooms are dim)
        if let Some(f) = frame.focus {
            u.params = [f.strength, last * FOCUS_FADE_START, FOCUS_TAP_SPACING, 0.0];
            u.extra = [f.blur / cascades[0].texel, f.core / cascades[0].texel, f.soft, f.blob_fade];
        }
        // ENHANCEMENT E9 (ENH-TUNPOOL): the covered stretches near the camera; a fragment inside one takes no sun-shadow darkening
        // (the shader's `cover_at`), whatever the camera is doing: a tunnel's pools look the same from outside and inside
        if !scene.cover.is_empty() && frame.focus.is_none() {
            let eye = camera.position;
            let mut near: Vec<(f32, &crate::CoverBox)> = scene.cover.iter().map(|b| (b.centre.distance_squared(eye), b)).filter(|(d, _)| *d < COVER_REACH * COVER_REACH).collect();
            if near.len() > MAX_COVER {
                near.select_nth_unstable_by(MAX_COVER - 1, |a, b| a.0.total_cmp(&b.0));
                near.truncate(MAX_COVER);
            }
            u.cover_n[0] = near.len() as f32;
            for (i, (_, b)) in near.iter().enumerate() {
                u.cover[5 * i] = [b.centre.x, b.centre.y, b.centre.z, b.half_len];
                u.cover[5 * i + 1] = [b.forward.x, b.forward.y, b.forward.z, b.half_wid];
                u.cover[5 * i + 2] = [b.lateral.x, b.lateral.y, b.lateral.z, b.w_back];
                u.cover[5 * i + 3] = [b.up.x, b.up.y, b.up.z, b.w_front];
                u.cover[5 * i + 4] = [b.ceil, b.half_nom, 0.0, 0.0];
            }
        }
        let size = wgpu::BufferSize::new(std::mem::size_of::<CsmUniform>() as u64).expect("csm size");
        self.belt.write_buffer(encoder, &self.csm.uniform, 0, size).copy_from_slice(bytemuck::bytes_of(&u));
        let size = wgpu::BufferSize::new(64).expect("light size");
        for (i, c) in cascades.iter().enumerate() {
            self.belt.write_buffer(encoder, &self.csm.light_bufs[i], 0, size).copy_from_slice(bytemuck::bytes_of(&c.mat.to_cols_array_2d()));
        }

        let map = self.csm.map.as_ref().expect("shadow map made");
        for (ci, c) in cascades.iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sun shadow pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &map.layer_views[ci],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: crate::prof::rp(["csm cascade 0", "csm cascade 1", "csm cascade 2", "csm cascade 3+"][ci.min(3)]),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.csm.light_bgs[ci], &[]);
            pass.set_bind_group(1, &self.object_bg, &[]);
            let (mut cur_pipe, mut cur_mat, mut cur_mesh) = (u8::MAX, u32::MAX, u32::MAX);
            for &(idx, centre, radius, cut) in &casters {
                // the sphere against the cascade's box in light space (toward the sun it is open)
                let (lx, ly, lz) = (ax.dot(centre), ay.dot(centre), frame.dir.dot(centre));
                let lim = c.half + radius;
                if (lx - c.centre[0]).abs() > lim || (ly - c.centre[1]).abs() > lim || lz - radius > c.zmin + c.range {
                    continue;
                }
                let item = &scene.items[idx as usize];
                if cur_pipe != cut as u8 {
                    pass.set_pipeline(if cut { &self.csm.pipe_cut } else { &self.csm.pipe_opaque });
                    cur_pipe = cut as u8;
                    cur_mat = u32::MAX;
                }
                if cut && cur_mat != item.material.0 {
                    pass.set_bind_group(2, &self.materials[item.material.0 as usize].bind_group, &[]);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(pos: Vec3, target: Vec3) -> Camera {
        Camera { position: pos, target, up: Vec3::Y, fov_y_radians: 45f32.to_radians(), near: 0.3, far: 700.0 }
    }

    #[test]
    fn cascade_size_does_not_depend_on_the_heading() {
        let d = Vec3::new(0.3, -0.8, 0.2);
        let a = fit(&cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(10.0, 2.0, 105.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
        let b = fit(&cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(110.0, 2.0, 5.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
        assert_eq!(a.len(), 2);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((x.half, x.texel, x.range), (y.half, y.texel, y.range));
        }
    }

    #[test]
    fn box_centre_is_snapped_to_whole_texels() {
        let d = Vec3::new(0.3, -0.8, 0.2);
        for k in 0..20 {
            let p = Vec3::new(3.0 + k as f32 * 0.0137, 2.0, 5.0 + k as f32 * 0.0291);
            let cs = fit(&cam(p, p + Vec3::new(0.2, 0.0, 1.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
            for c in &cs {
                for v in c.centre {
                    let t = v / c.texel;
                    assert!((t - t.round()).abs() < 1e-3, "{t}");
                }
            }
        }
    }

    #[test]
    fn a_point_in_the_slice_lands_inside_the_map() {
        let d = Vec3::new(0.3, -0.8, 0.2).normalize();
        let c = cam(Vec3::new(100.0, 5.0, -40.0), Vec3::new(130.0, 4.0, 10.0));
        let cs = fit(&c, 1.6, d, &[40.0, 140.0], 1024, 120.0);
        let fwd = (c.target - c.position).normalize();
        let p = c.position + fwd * 20.0;
        let q = cs[0].mat * p.extend(1.0);
        assert!(q.x.abs() < 1.0 && q.y.abs() < 1.0 && (0.0..1.0).contains(&q.z), "{q:?}");
    }

    #[test]
    fn bounds_cover_the_vertices() {
        let v = |x: f32, y: f32, z: f32| Vertex { position: [x, y, z], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] };
        let (c, r, flat) = mesh_bounds(&[v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), v(1.0, 2.0, 0.0)]);
        assert_eq!(c, [1.0, 1.0, 0.0]);
        assert!((r - 2f32.sqrt()).abs() < 1e-6);
        assert!(!flat);
    }
}
