use crate::front::{Cmd, CmdList};
use crate::{depth_bias_offset, Camera, FakeSortMode, LensProbe, LensRead, MaterialDesc, MeshData, Scene, Sprite, SpriteCompare, TextureData, Vertex};
use anyhow::{Context, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

mod csm;
mod spot;
mod lamps;
mod contact;
mod fill;
mod init;
mod resources;
mod materials;
mod refl;
mod readback;
mod enhanced;
mod frame;
mod pipelines;
mod post;
mod chain;
#[cfg(target_os = "macos")]
mod diag;

// the items the rest of gpu/ names from the two modules above (their own glob `use super::*` sees these)
use pipelines::{build_mesh_pipelines, HDR_KEY};
use post::{GammaPass, ScaleBlit};
#[cfg(test)]
pub(crate) use post::{blit_source, gamma_source};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MeshId(pub(crate) u32);

impl MeshId {
    /// ENHANCEMENT ENH-LIVELEAK: an id that names no mesh (the `n`-th), for a caller that pairs parts by identity without having uploaded
    /// them (a live twin build leaves the unused original copy out). Never given to the renderer.
    pub fn placeholder(n: u32) -> Self {
        MeshId(0x8000_0000 | n)
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureId(pub(crate) u32);

/// What an offscreen pass (`Renderer::render_offscreen`) renders into: the size, an earlier texture
/// to reuse, the clear colour (`None` = transparent) and the far plane of the pass's own screen z
/// (`None` = [`VIEW_FAR`]).
#[derive(Clone, Copy, Debug)]
pub struct OffscreenSpec {
    pub width: u32,
    pub height: u32,
    pub reuse: Option<TextureId>,
    pub clear_colour: Option<[f64; 4]>,
    pub depth_far: Option<f32>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MaterialId(pub(crate) u32);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobalsUniform {
    view_proj: [[f32; 4]; 4],
    /// the camera's view matrix alone — `R3DSpecularTextureLayer` needs the normal in eye space
    view: [[f32; 4]; 4],
    sun_dir: [f32; 4],
    ambient: [f32; 4],
    cam_pos: [f32; 4],
    /// world → headlight frame; `headlight_on.x` 1 while it lights, `.y` 1 while the `HEAD`
    /// decal's own lamp-0 gate (0x453CA3) holds too
    headlight: [[f32; 4]; 4],
    headlight_on: [f32; 4],
    /// world → the police beacon's frame, and `beacon_on.x` 1 while one spins
    beacon: [[f32; 4]; 4],
    beacon_on: [f32; 4],
    /// the scene's fog (`Scene::fog`): `.rgb` its colour, `.w` its density (0 off)
    fog: [f32; 4],
    /// the sky pass's fog (`Scene::sky_fog`)
    sky_fog: [f32; 4],
    /// ENHANCEMENT E8: `.x` the scene's reflection weight (0 = off), `.y` the face map's
    /// `1 / tan(half fov)`
    refl: [f32; 4],
    /// ENHANCEMENT E12: `.xy` the pass's near / far planes, `.z` 1 while the soft depth image is bound
    soft: [f32; 4],
    /// ENHANCEMENT E17: `.x` 1 while the water reflection is on, `.y` the sun's glint gain (0 = no sun)
    water: [f32; 4],
    /// the sky straight up / at the horizon the water reflects without an environment map
    water_zenith: [f32; 4],
    water_horizon: [f32; 4],
    /// `.xyz` the unit vector to the sun
    water_sun: [f32; 4],
    /// ENHANCEMENT E18: `.xyz` the unit vector to the sun, `.w` 1 while the lit particles take it (0 = off, or no sun)
    fx_sun: [f32; 4],
    /// ENHANCEMENT E19: `.xyz` the unit vector to the sun, `.w` the sun in-scatter strength of the fog (0 = off)
    fogsun: [f32; 4],
    /// ENHANCEMENT E20: `.x` the lightning flash's strength 0..1 (0 = none, master's value)
    flash: [f32; 4],
    /// ENHANCEMENT E26: `.x` the rain's strength (0 = off, master's value), `.y` the clock, `.z` the lamp count, `.w` the wetness at the camera
    rain: [f32; 4],
    rain_zenith: [f32; 4],
    rain_horizon: [f32; 4],
    rain_lamp_pos: [[f32; 4]; crate::RAIN_LAMPS],
    rain_lamp_col: [[f32; 4]; crate::RAIN_LAMPS],
    /// the wetness edges: `pos.xyz` + half width, `dir.xyz` + the value before, `val.x` the value after
    rain_edge_pos: [[f32; 4]; crate::RAIN_EDGES],
    rain_edge_dir: [[f32; 4]; crate::RAIN_EDGES],
    rain_edge_val: [[f32; 4]; crate::RAIN_EDGES],
    /// ENHANCEMENT ENH-TEMPORAL: `.xy` the jitter in NDC (already in `view_proj`), `.zw` the render size in pixels (0 = off)
    temporal: [f32; 4],
}

/// ENHANCEMENT E18: a spark's colour is multiplied by this (the hot core of its ramp ends well above 1, over the bloom's threshold).
const SPARK_GAIN: f32 = 6.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    env_mix: [f32; 4],
    light: [f32; 4],
    /// `R3DLightLayer` (`RE/notes/lighting.md` §6): `lit_a.xyz` the slot's colour × p0 and
    /// `lit_a.w` 1 when the layer lights this item; `lit_d.xyz` the `slot + 4` partner × p1;
    /// `lit_dir.xyz` the direction the light travels, in world space
    lit_a: [f32; 4],
    lit_d: [f32; 4],
    lit_dir: [f32; 4],
    /// `R3DShadowMapTextureLayer` (0x446130): `u = shadow_u.xyz · world + shadow_u.w`, likewise
    /// `v`; `shadow_on.x` 1 when the item takes the projector's texture
    shadow_u: [f32; 4],
    shadow_v: [f32; 4],
    shadow_on: [f32; 4],
    /// `R3DSkyMapTextureLayer` (0x445B10): `u = sky_u.xyz · p + sky_u.w`, likewise `v`, over the
    /// world point `p` of `normal + 3.33 · position`; `sky_on.x` 1 when the item draws the pass
    sky_u: [f32; 4],
    sky_v: [f32; 4],
    sky_on: [f32; 4],
    /// ENHANCEMENT E8: `.x` the material's reflection class (0 none, 1 paint, 2 glass), `.y` its strength
    refl: [f32; 4],
    /// ENHANCEMENT E12: `.x` the fade distance in metres (0 = a hard edge), `.y` how the fade is laid
    /// on the colour (0 alpha only, 2 toward white for the multiplying blend)
    soft: [f32; 4],
    /// ENHANCEMENT E11: `.x` the item's HDR boost (> 1 = a flare layer drawn above white in the float scene)
    /// ENHANCEMENT E18: `.y` a spark's gain above white (0 = none), `.z` the lit particle kind ([`crate::PARTICLE_LIT`] / `_WORLD`, 0 = none)
    hdr: [f32; 4],
    /// `view_proj · model` multiplied out in `f64` and rounded once (PORT-DEVIATION D37): the clip
    /// position of a vertex, so that a car 4 km from the world origin does not lose its sub-millimetre
    /// depth order to `f32` world coordinates. Last in the struct, so the fields master has keep their offsets.
    /// `view_proj` is this frame's (with ENHANCEMENT ENH-TEMPORAL's jitter when that is on).
    mvp: [[f32; 4]; 4],
}

/// A mesh's vertices and indices: a byte range of a vertex buffer and of an index buffer. A static
/// mesh ([`Core::upload_mesh`]) is carved out of a shared arena buffer — a Metal buffer is a
/// page-aligned allocation of its own and a track has thousands of meshes — a dynamic one has its
/// own buffers (offset 0).
struct GpuMesh {
    vertex_buf: wgpu::Buffer,
    vertex_offset: u64,
    vertex_len: u64,
    index_buf: wgpu::Buffer,
    index_offset: u64,
    index_len: u64,
    index_count: u32,
    /// The vertex and the index buffer's serial ([`Core::buffer_serial`]): the meshes of one arena
    /// share theirs, so a draw that moves to another mesh of the same arena binds nothing
    vb_serial: u32,
    ib_serial: u32,
    /// How many vertex writes this mesh has had since its upload (every `write_mesh_vertices` /
    /// `update_mesh_vertices` that queued bytes): lets a caller that keeps the last vertices it
    /// wrote tell whether anyone else wrote the mesh since ([`Core::mesh_writes`]).
    writes: u32,
    /// ENHANCEMENT E9: the bounding sphere (centre, radius) of the vertices at upload, in mesh space
    bounds: ([f32; 3], f32),
    /// ENHANCEMENT E9: a level flat sheet (a road layer, a decal): no caster
    flat: bool,
    /// ENHANCEMENT ENH-LIVELEAK: carved out of a shared arena (its ranges go to the free list on release), the reserved byte sizes of
    /// the two ranges (the slot a later mesh may take over), and whether the mesh was released (an empty stub now)
    arena: bool,
    cap: (u64, u64),
    released: bool,
}

/// ENHANCEMENT ENH-LIVELEAK: the arena ranges of a released static mesh (a vertex range and an index range, with their reserved sizes),
/// kept for the next [`Core::upload_mesh`] they fit.
struct FreeMeshSlot {
    /// the buffers' serial ([`GpuMesh::vb_serial`]): the arena's they were carved from
    serial: u32,
    vertex_buf: wgpu::Buffer,
    vertex_offset: u64,
    vertex_cap: u64,
    index_buf: wgpu::Buffer,
    index_offset: u64,
    index_cap: u64,
}

/// ENHANCEMENT ENH-LIVELEAK: what the GPU side holds, for the live-build log and the leak checks (`Renderer::resource_stats`).
/// `*_live` leave out the released slots; `*_bytes` are the live textures' base + mip levels and the live meshes' vertex + index bytes;
/// `arena_bytes` is every arena buffer ever made (the shared chunks the static meshes are carved from) plus the dynamic meshes' own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceStats {
    pub textures: usize,
    pub textures_live: usize,
    pub texture_bytes: u64,
    pub meshes: usize,
    pub meshes_live: usize,
    pub mesh_bytes: u64,
    pub arena_bytes: u64,
    pub materials: usize,
}

/// The arena the static meshes are carved from: a vertex and an index buffer, filled front to back.
// PORT-DEVIATION D6 (RE/notes/PORT-DEVIATIONS.md): static meshes in shared arenas and instanced draws from one object storage buffer; draw order and state as before, a polygon seam can land one pixel differently (not D3D7-exact, never was).
struct MeshArena {
    /// the two buffers' serial ([`GpuMesh::vb_serial`])
    serial: u32,
    vertex_buf: wgpu::Buffer,
    vertex_used: u64,
    index_buf: wgpu::Buffer,
    index_used: u64,
    /// Created mapped, so a mesh is a plain copy into it; unmapped ([`Core::seal_arena`]) before
    /// the first frame that can draw from it, after which a mesh is a queued write.
    mapped: bool,
}

/// One arena's size for each of the two buffers; a larger mesh gets an arena of its own size.
const ARENA_CHUNK: u64 = 4 << 20;

/// [`Vertex`]'s size: the vertex buffer's stride
const VERTEX_STRIDE: u64 = std::mem::size_of::<Vertex>() as u64;
const _: () = assert!(VERTEX_STRIDE % 8 == 0, "a vertex range stays 8-byte aligned");

#[derive(Clone)]
struct GpuTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    sprite_bg: Option<wgpu::BindGroup>,
    /// ENHANCEMENT E2: the texture came with one level and carries a generated chain (levels 1..
    /// are ours, the alpha treatment is the value); `None` is every texture as the original has it.
    /// While E2 is off such a texture is sampled at level 0 only, so it looks as before.
    generated: Option<crate::mipgen::MipAlpha>,
    size: (u32, u32),
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteVertex {
    /// clip space; z the screen depth of a sprite under a depth state (`Sprite::depth`), else 0
    pos: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
    /// the alpha test's reference (`SpriteDepth::alpha_test`); −1 when the test is off (every
    /// pixel's alpha passes), 0 for a plain sprite (unused there)
    alpha_ref: f32,
}

struct GpuMaterial {
    bind_group: wgpu::BindGroup,
    /// `false_sort`'s single-draw arm (`SPECIALFX` below 2, `RE/notes/thrash.md` §6d.4): the same
    /// bind group with the point-filtered sampler (Thrash 7 ← 0, `LayerFalseSort1` 0x449ABE),
    /// built alongside `bind_group` and kept only for a `FakeSort` material
    bind_group_point: Option<wgpu::BindGroup>,
    alpha_blend: bool,
    blend_pair: u8,
    device_states: bool,
    /// the address-mode half of the index into `samplers` (`clamp_u | clamp_v << 1`)
    sampler: usize,
    /// Thrash state 8, `MIPMAPLODBIAS` (`RE/notes/thrash.md` §7)
    lod_bias: f32,
    cull_back: bool,
    view_cull: bool,
    /// Thrash state 0x18 (`MaterialDesc::depth_bias`)
    depth_bias: i32,
    false_sort: bool,
    depth_test: bool,
    env_mix: f32,
    /// the bucket of the material's reflection strength — which of the car's three section maps
    /// its `R3DSkyMapTextureLayer` samples
    env_variant: usize,
    /// the method has a built `R3DSkyMapTextureLayer` pass (`MaterialDesc::sky_map`)
    sky_map: bool,
    /// the diffuse texture, kept so the bind group can be rebuilt (`set_trilinear`)
    texture: TextureId,
    /// the specular map
    env: TextureId,
    /// ENHANCEMENT E8
    reflect: crate::Reflect,
}

/// ENHANCEMENT E2: a scene sampler by address mode — linear magnification / minification, the
/// given mip filter, `anisotropy_clamp` (wgpu wants every filter linear for anything above 1) and an
/// optional `lod_max_clamp`.
fn make_scene_sampler(device: &wgpu::Device, clamp_u: bool, clamp_v: bool, mip: wgpu::MipmapFilterMode, anisotropy: u16, lod_max: Option<f32>) -> wgpu::Sampler {
    let mode = |c: bool| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("scene E2"),
        address_mode_u: mode(clamp_u),
        address_mode_v: mode(clamp_v),
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: mip,
        lod_max_clamp: lod_max.unwrap_or(32.0),
        anisotropy_clamp: anisotropy,
        ..Default::default()
    })
}

pub(crate) struct Core {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// the present modes the surface supports (ENHANCEMENT E6 picks among them)
    present_modes: Vec<wgpu::PresentMode>,
    depth_view: wgpu::TextureView,
    pipelines: Vec<wgpu::RenderPipeline>, // see `make_pipeline`'s loops for the index
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    globals_layout: wgpu::BindGroupLayout,
    /// the projector's sampler, trilinear and with the `POINT` mip filter (`trilinear` picks)
    proj_samplers: [wgpu::Sampler; 2],
    _proj_default: wgpu::Texture,
    proj_default_view: wgpu::TextureView,
    headlight_map_buf: wgpu::Buffer,
    proj_tex: Option<TextureId>,
    head_tex: Option<TextureId>,
    object_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    /// the car's section map for `R3DSkyMapTextureLayer`, one bind group per texture (group 3)
    sky_layout: wgpu::BindGroupLayout,
    sky_bgs: Vec<Option<wgpu::BindGroup>>,
    /// ENHANCEMENT E8: the six environment faces (a `D2` array of `refl_size`²), the 1 × 1 black array
    /// the binding holds while there is none, and the render target a face is drawn into before it is
    /// copied into its layer (a layer cannot be an attachment while the array is bound)
    refl: Option<ReflCube>,
    refl_dummy_view: wgpu::TextureView,
    _refl_dummy: wgpu::Texture,
    /// ENHANCEMENT E16: the SH projection of the faces (built on first use; `None` without compute
    /// shaders), and the zero uniform the binding holds until then
    probe: Option<crate::probe::Probe>,
    /// ENHANCEMENT ENH-COAT: the prefilter of the environment cube's mips (made with the first cube), and whether an
    /// environment face is being drawn now (its frame must not run the prefilter: the faces are not all in yet)
    coat: Option<crate::coat::CoatFilter>,
    refl_building: bool,
    probe_dummy: wgpu::Buffer,
    compute_supported: bool,
    /// ENHANCEMENT E12: the depth converter (built on first use), and the 1 x 1 image the binding
    /// holds while no soft depth is bound
    soft: Option<crate::soft::SoftDepth>,
    /// ENHANCEMENT E14: the corner-shading passes (built on first use)
    ssao: Option<crate::ao::Ssao>,
    /// ENHANCEMENT E13: the light-shafts pass (built on first use)
    shafts: Option<crate::shafts::ShaftsPass>,
    /// ENHANCEMENT E36: the haze pass (built on first use)
    haze: Option<crate::haze::HazePass>,
    /// ENHANCEMENT E44: the lighthouse beam pass (built on first use)
    lbeam: Option<crate::lbeam::LBeamPass>,
    soft_dummy_view: wgpu::TextureView,
    _soft_dummy: wgpu::Texture,
    /// `[0x6115CC]+0x190`: set while the view being drawn is the rear-view mirror's (type 1,
    /// 0x43AD39…0x43AD4E), when the specular and sky-map layers take no texture and skip their passes
    no_reflections: bool,
    /// the adapter draws with a base vertex (`DownlevelFlags::BASE_VERTEX`; every Metal, Vulkan and
    /// Direct3D 12 device, not every OpenGL one): the draws of one arena bind its buffers once and
    /// address a mesh by `first_index` / `base_vertex` ([`Core::draw_run`])
    base_vertex: bool,
    /// the next serial for a vertex / index buffer pair ([`GpuMesh::vb_serial`])
    buffer_serial: u32,
    /// the scene samplers by address mode and mip filter — index `clamp_u | clamp_v << 1 |
    /// point_mip << 2`
    samplers: [wgpu::Sampler; 8],
    /// the `FakeSort` single-draw arm's samplers (Thrash 7 ← 0, `MAG`/`MINFILTER` `POINT`), by
    /// address mode alone — index `clamp_u | clamp_v << 1` — used only for a single-draw item
    point_samplers: [wgpu::Sampler; 4],
    /// ENHANCEMENT E2, E2 off: `samplers` with the level range cut to level 0 — for a texture with
    /// a generated chain, which then samples exactly as the original's single level, by address mode
    level0_samplers: [wgpu::Sampler; 4],
    /// the same for the `FakeSort` single-draw arm's point samplers
    point_level0_samplers: [wgpu::Sampler; 4],
    /// ENHANCEMENT E2, E2 on: trilinear + anisotropic scene samplers by address mode, built for
    /// `aniso_clamp` (rebuilt when the level changes)
    aniso_samplers: [wgpu::Sampler; 4],
    aniso_clamp: u16,
    /// the adapter's `ANISOTROPIC_FILTERING` (downlevel flag); without it E2 still gives the
    /// generated mips and trilinear, with the clamp at 1
    aniso_supported: bool,
    /// ENH-LITE: how strong the adapter is (integrated / software = weak), for the first-run set
    gpu_class: crate::GpuClass,
    /// the mip filter's `POINT` → `LINEAR` promotion, `[0x69DD1D]` (`set_trilinear`); clear at start, as the image has it
    trilinear: bool,
    /// `LayerFalseSort1`/`2`'s per-frame pick, [`FakeSortMode`] — `main.rs` sets it from
    /// `SPECIALFX` and, at level 2, the runtime globals 0x44EC80 reads to the instruction
    /// (`RE/notes/thrash.md` §6d.4).
    fakesort_mode: FakeSortMode,
    /// The display gamma, state **0x2E** (`setstatef`, `[0x6BD918]`, with the value
    /// `max(0.25, Graphics.GammaCorrection · 0.01)`, 0x44ED10 / 0x4DED10 / 0x4E5A70 / 0x511900).
    /// `dx7z.dll`'s handler 0x600051D4 clamps it to `0…4` and multiplies the primary surface's
    /// gamma ramp — the one it read at the mode set (0x600032B5, `GetGammaRamp`) — by it, each
    /// word truncated (0x6000CB74) and capped at 0xFFFF, then `SetGammaRamp` (vtable `+0x10`). So it
    /// is a scale of every displayed channel after the whole frame is drawn: [`Self::set_gamma`],
    /// applied by [`GammaPass`]. 1.0 (the default 100) draws straight into the target as before.
    gamma: f32,
    gamma_pass: Option<GammaPass>,
    /// The enhanced game's switches ([`crate::Enhancements`]), set through the command list; the
    /// core starts on [`crate::Enhancements::ORIGINAL`], whose every GPU-side branch is `master`'s
    /// code path. An enhancement tests `self.enhancements.is_on(..)` and nothing else.
    pub(crate) enhancements: crate::Enhancements,
    /// WIN-STARTUP: a GPU error forced every enhancement off for this run ([`GPU_ERRORS`]); later sets are ignored
    enh_forced_off: bool,
    gpu_errors_seen: u32,
    /// ENHANCEMENT E1: the sample counts (2, 4, 8) the colour format and the depth format both
    /// support multisampled and resolved, from the adapter (bit `n.trailing_zeros()`)
    ms_supported: u32,
    /// ENHANCEMENT E7: the device can draw and blend into `Rgba16Float` and filter it
    hdr_ok: bool,
    /// ENHANCEMENT E33: the EDR output's state (macOS; a stub elsewhere that is never on)
    edr: crate::edr::Edr,
    /// ENHANCEMENT E33 diagnostic: EDR frames drawn (the probe measures the 20th)
    #[cfg(target_os = "macos")]
    edr_frames: u32,
    /// ENHANCEMENT E7: [`Self::ms_supported`] for `Rgba16Float`
    ms_supported_hdr: u32,
    /// ENHANCEMENT E7: the tone-map mode of the 3D pass being encoded (0 = as the original, 1 =
    /// ACES); `None` = the original's 8-bit target. Set around `encode_frame` by the E7 paths only.
    cur_hdr: Option<u32>,
    /// ENHANCEMENT E7: the rear-view mirror's own float image + tone map (its size differs from the
    /// main view's)
    scale_blit_mirror: Option<ScaleBlit>,
    /// ENHANCEMENT E1 / E7: the key (sample count, | [`HDR_KEY`] for the float set) `pipelines` were built for (1 = master's 66)
    pipeline_samples: u32,
    /// ENHANCEMENT E1 / E7: the sets built for other keys, kept so a switch back is free
    pipeline_sets: std::collections::HashMap<u32, Vec<wgpu::RenderPipeline>>,
    mesh_shader: wgpu::ShaderModule,
    /// the mesh shader with the inert-at-level-0 blocks, built when the first switch goes on
    mesh_shader_full: Option<wgpu::ShaderModule>,
    mesh_layout: wgpu::PipelineLayout,
    /// ENHANCEMENT E1: multisampled colour + depth targets by (width, height, samples)
    ms_cache: std::collections::HashMap<(u32, u32, u32), (wgpu::TextureView, wgpu::TextureView)>,
    /// ENHANCEMENT E3: the scaled 3D image and the blit that lays it over the window-sized image
    scale_blit: Option<ScaleBlit>,
    /// ENHANCEMENT ENH-TEMPORAL: the temporal foundation (jitter, motion vectors, masks, the debug pass), made when
    /// `NFS_TEMPORAL_DEBUG` is set or [`Self::temporal_flag`] is raised; `None` = off, master's frame
    temporal: Option<crate::temporal::Temporal>,
    /// ENHANCEMENT ENH-TEMPORAL: a temporal upscaler (phase B: FSR 3.1 / MetalFX temporal) is active: the main view is
    /// jittered, writes the vectors and masks, and renders at 1 sample. `false` until one is wired.
    temporal_flag: bool,
    /// ENHANCEMENT ENH-TEMPORAL: the object pass's second layout (the reprojection matrices), made at first use
    temporal_gpu: Option<crate::temporal::Gpu>,
    /// ENHANCEMENT ENH-TEMPORAL: the targets and bind group of the frame being encoded (set around `encode_frame`)
    cur_temporal: Option<crate::temporal::Cur>,
    /// ENHANCEMENT ENH-TEMPORAL: the reprojection matrices of the frame being encoded (kept between frames)
    motion: crate::temporal::Motion,
    /// ENHANCEMENT ENH-TEMPORAL: the last temporal main view's inputs for an upscaler (phase B)
    #[allow(dead_code)]
    temporal_inputs: Option<crate::temporal::Inputs>,
    /// ENHANCEMENT E37: the FSR 3.1 backend (built at first use) and its state, 0 = not tried, 1 = ready, 2 = unavailable (logged once)
    fsr: Option<Box<dyn crate::upscale::Upscaler>>,
    fsr_state: u8,
    /// ENHANCEMENT E34 (temporal levels): the same slot also holds MetalFX's temporal scaler (macOS): which backend `fsr` is, and E34's own
    /// state (0 = not tried, 1 = ready, 2 = unavailable / failed)
    fsr_kind: Option<crate::upscale::UpscalerKind>,
    #[cfg(target_os = "macos")]
    mfxt_state: u8,
    /// ENHANCEMENT E37: the frame being encoded goes through FSR
    fsr_active: bool,
    /// ENHANCEMENT E37: FSR's float output (display size) and the bind group the post chain reads it by: `(size, texture, view, bind group)`
    fsr_out: Option<crate::gfx::RtImage>,
    /// ENHANCEMENT E37: `encode_with_gamma_inner` calls so far, and the call number of the last FSR frame (a frame in between
    /// that was not one - a menu, a loading screen, another view - drops the history)
    frame_no: u64,
    fsr_last_frame: u64,
    /// ENHANCEMENT E34: the upscaler backend (built at first use), 0 = not tried, 1 = ready, 2 = unavailable (logged once)
    upscaler: Option<Box<dyn crate::upscale::Upscaler>>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    upscale_state: u8,
    /// ENHANCEMENT E34: the frame being encoded goes through the upscaler (set by `encode_with_gamma_inner`)
    upscale_active: bool,
    /// ENHANCEMENT E34: the scaler's input / output images and the plain blit that lays its output over the window image
    upscale_images: Option<crate::upscale::UpscaleImages>,
    upscale_present: Option<ScaleBlit>,
    /// ENHANCEMENT E39 (UPSCALE-SOFT): the sharpening after the MetalFX scalers (built at first use)
    sharpen: Option<crate::sharpen::Sharpen>,
    /// ENHANCEMENT UPSCALE-SOFT: the texture LOD bias added to every material's own while an upscaler's main view is encoded
    /// (`upscale::mip_bias`); 0 otherwise, and then the object uniforms are master's bytes
    cur_lod_shift: f32,
    /// ENHANCEMENT E35: SMAA 1x over the tone-mapped image, made at first use
    smaa: Option<crate::smaa::Smaa>,
    /// ENHANCEMENTS E10 / E21 / E27 / the depth of field (photo mode, E31) / E40 / E28: the post chain's passes (each built at its first use)
    post: chain::PostPasses,
    sprite_sampler: wgpu::Sampler,
    object_buf: wgpu::Buffer,
    object_bg: wgpu::BindGroup,
    meshes: Vec<GpuMesh>,
    arena: Option<MeshArena>,
    /// ENHANCEMENT ENH-LIVELEAK: the ranges of released static meshes, taken by a later upload that fits (best fit)
    free_slots: Vec<FreeMeshSlot>,
    /// ENHANCEMENT ENH-LIVELEAK: the empty buffers every released mesh slot points at, and their serial ([`GpuMesh::vb_serial`])
    mesh_stub: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    /// ENHANCEMENT ENH-LIVELEAK: the bytes of every arena chunk and dynamic mesh buffer made so far (`ResourceStats::arena_bytes`)
    arena_bytes: u64,
    textures: Vec<GpuTexture>,
    /// ENHANCEMENT ENH-LIVELEAK: the 1 x 1 white image every released texture slot shares, and the slots released
    stub_texture: Option<GpuTexture>,
    released_textures: std::collections::HashSet<u32>,
    /// ENHANCEMENT ENH-LIVELEAK: released texture / mesh slots. They wait in `pending_*` until [`Self::recycle_released`] (the end of a live
    /// pass: every model holding the old id has let go of it by then); a later upload takes a slot from `free_*` instead of growing the table.
    pending_tex: Vec<u32>,
    free_tex: Vec<u32>,
    pending_mesh: Vec<u32>,
    free_mesh: Vec<u32>,
    /// ENHANCEMENT ENH-LIVETEX (E41): a texture's HD twin, by the original's index; a material samples the twin while E41 is above 0
    /// (`Self::shown_texture`), the original otherwise, so F12 / the menu flip between the two without a reload
    tex_twin: Vec<Option<TextureId>>,
    /// ENHANCEMENT ENH-LIVETEX (E42): a mesh's refined twin (a car's rounded part), drawn in place of the original while E42 is above 0
    mesh_twin: Vec<Option<MeshId>>,
    materials: Vec<GpuMaterial>,
    white: TextureId,
    sprite_pipeline: wgpu::RenderPipeline,
    /// The front end's depth-state sprites (`Sprite::depth`): depth writes on, the alpha test, the
    /// compare `LESSEQUAL` \[0\] / `GREATEREQUAL` \[1\].
    sprite_depth_pipelines: [wgpu::RenderPipeline; 2],
    /// offscreen render targets handed out by [`Core::render_offscreen`], by texture index
    offscreen: std::collections::HashMap<u32, (u32, u32)>,
    sprite_layout: wgpu::BindGroupLayout,
    sprite_buf: wgpu::Buffer,
    /// the per-frame upload path: every buffer write of a frame (globals, objects, sprites, the
    /// dynamic meshes' vertices) is a copy out of a recycled mapped chunk, recorded into the
    /// frame's own encoder — `Queue::write_buffer` makes a fresh staging buffer per call
    belt: wgpu::util::StagingBelt,
    /// `write_mesh_vertices` / `update_mesh_vertices` calls waiting for the next frame's encoder,
    /// in call order (the last write to a mesh wins, as with `write_buffer`)
    pending_writes: Vec<PendingWrite>,
    pending_bytes: Vec<u8>,
    /// `update_texture` calls waiting for the next frame's encoder, in call order, their rows padded
    /// to the copy's 256-byte alignment (`Queue::write_texture` makes a fresh staging buffer per call)
    pending_textures: Vec<PendingTextureWrite>,
    pending_texture_bytes: Vec<u8>,
    /// depth targets by size, shared by every offscreen pass of that size (each pass clears it)
    depth_cache: std::collections::HashMap<(u32, u32), wgpu::TextureView>,
    /// the pass's item indices by stage (sky, opaque, blended, overlay), kept between frames
    buckets: [Vec<u32>; 4],
    /// the per-item arrays `encode_frame` builds every frame, kept between frames (no allocation per frame)
    frame_scratch: FrameScratch,
    /// the surface carries `COPY_SRC`, so the lens flare's frame read-back can copy from it
    /// ([`Self::set_lens_enabled`])
    lens_enabled: bool,
    /// read-backs issued and not yet collected, oldest first
    lens_pending: Vec<LensPending>,
    /// read-backs collected, for the front to hand to the game
    lens_done: Vec<LensRead>,
    /// blocks copied into the frame being encoded, mapped after its submit
    lens_staged: Vec<(wgpu::Buffer, bool, bool)>,
    /// `NFS_GPU_TIME`, read once, and its running (sum ms, frames)
    gpu_time_log: bool,
    gpu_time_acc: (f64, u32),
    /// ENHANCEMENT E9: the sun shadow maps' pipelines, uniform and map
    csm: csm::CsmState,
    /// ENHANCEMENT E15: the headlight spots' uniform and shadow map
    spot: spot::SpotState,
    /// ENHANCEMENT E23: the street lamps' uniform
    lamp: lamps::LampState,
    /// ENHANCEMENT E43: the cars' contact shadow uniform
    contact: contact::ContactState,
    /// the last submission of each of the frames still in flight, oldest first, and the newest
    /// submission of the frame being built ([`Self::frame_begin`])
    frames_in_flight: std::collections::VecDeque<wgpu::SubmissionIndex>,
    last_submit: Option<wgpu::SubmissionIndex>,
}

/// At most this many frames are queued to the GPU ahead of the one being built (host pacing, no
/// counterpart in the game's code: a driver's render-ahead). With vsync the swap chain's own
/// three drawables already hold the loop there, so this never waits; it
/// matters when presenting does not block (an occluded window on macOS hands out drawables without
/// waiting), where wgpu-hal's Metal queue (4096 command buffers) would let thousands of frames —
/// each holding its staging chunks — pile up (`RE/notes/PERF.md` PERF2).
const MAX_FRAMES_IN_FLIGHT: usize = 3;

/// One lens-flare read-back in flight: the 4 × 4 block copied into `buf` (rows `LENS_ROW` bytes
/// apart), mapped once the copy has run.
struct LensPending {
    buf: wgpu::Buffer,
    reference: bool,
    /// ENHANCEMENT E7: the block was copied from the float image (8 bytes a pixel)
    half: bool,
    rx: std::sync::mpsc::Receiver<std::result::Result<(), wgpu::BufferAsyncError>>,
}

/// The copy's row pitch: `COPY_BYTES_PER_ROW_ALIGNMENT`.
const LENS_ROW: u64 = 256;

/// One deferred vertex write: `len` bytes at `pending_bytes[start..]` into mesh `mesh`'s buffer.
struct PendingWrite {
    mesh: u32,
    start: usize,
    len: usize,
}

/// One deferred texture write: one mip level, `height` rows of `bytes_per_row` bytes at
/// `pending_texture_bytes[start..]`, of which each row's first `4 · width` are pixels.
struct PendingTextureWrite {
    texture: u32,
    level: u32,
    width: u32,
    height: u32,
    bytes_per_row: u32,
    start: usize,
}

/// What the render pass has bound, so a draw sets only what changed.
struct DrawState {
    pipeline: usize,
    /// what the bound material bind group holds ([`bind_key`]): two materials over the same
    /// textures and sampler bind the same thing, so the second binds nothing
    material: u64,
    sky: u32,
    mesh: u32,
    /// the bound vertex / index buffer's serial (the base-vertex path)
    vb: u32,
    ib: u32,
}

impl DrawState {
    const NONE: Self = Self { pipeline: usize::MAX, material: u64::MAX, sky: u32::MAX, mesh: u32::MAX, vb: u32::MAX, ib: u32::MAX };
}

/// A run of consecutive items drawn as one instanced draw: same mesh, pipeline and bind groups,
/// object indices `first..first + count`.
struct Run {
    first: u32,
    count: u32,
    pipeline: usize,
    /// `material * 2 + point-filtered bind group`
    material: u32,
    sky: u32,
    mesh: u32,
}

/// What a material's bind group (`point`: its point-filtered one) is made of — the diffuse texture,
/// the sampler's address modes, the specular map and the filter — as one number. Every bind group of
/// the frame is built under the same `trilinear` ([`Core::set_trilinear`] rebuilds them all), so
/// two equal keys are two bind groups over the same views and the same sampler. The enhanced game's
/// rebuilds keep that: ENHANCEMENT E2 rebuilds every material, ENHANCEMENT ENH-LIVETEX every material
/// of the one original texture whose twin changed, and a material's own texture is never a twin's
/// (recycled) slot.
fn bind_key(m: &GpuMaterial, point: bool) -> u64 {
    m.texture.0 as u64 | (m.env.0 as u64) << 28 | (m.sampler as u64) << 56 | (point as u64) << 60
}

/// The section map an item's sky-map pass samples, by its material's bucket.
fn sky_tex_of(materials: &[GpuMaterial], no_reflections: bool, item: &crate::DrawItem) -> Option<TextureId> {
    let m = materials.get(item.material.0 as usize)?;
    if !m.sky_map || no_reflections {
        return None;
    }
    item.sky.and_then(|s| s.textures[m.env_variant])
}

/// The front-end screens draw a few thousand quads (every gouraud box is a grid of them).
const MAX_SPRITES: u64 = 8192;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// ENHANCEMENT E19: how far the fog colour is pulled toward the sun's warm colour straight at the sun (a phase
/// function normalised to 1 there and 0 from 90 degrees away on); modest, the glow only reads as a glow.
const FOG_SUN_STRENGTH: f32 = 0.5;
/// ENHANCEMENT E7, the extended-range (EDR / HDR) output seam — NOT enabled. The surface is the
/// original's byte surface (`Bgra8Unorm`, `Default` = wgpu's sRGB) and the tone map above writes
/// display-referred 0...1 values into it. EDR on macOS would be a surface of `Rgba16Float` configured
/// with `wgpu::SurfaceColorSpace::ExtendedSrgbLinear` (the caps list it per format,
/// `SurfaceCapabilities::format_capabilities`) and a last pass that writes linear light (the tone
/// map's output decoded with 2.2, white scaled to the display's headroom) instead of the encoded
/// value; this function and the `format` choice in `Core::new` are the two places that would select
/// it, and `cfg(target_os = "macos")` is where it would start.
fn surface_color_space() -> wgpu::SurfaceColorSpace {
    wgpu::SurfaceColorSpace::default()
}
/// ENHANCEMENT E7: the 3D scene's float target format.
pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MAX_OBJECTS: u64 = 8192;
/// PORT-DEVIATION D46 (photo mode): the most pixels a still has (an 8-bit target, its depth and the read-back buffer of
/// 40 MP are ~0.5 GB; ENHANCEMENT E7's float image and its post chain at that size fit a 6 GB device).
pub const STILL_MAX_PIXELS: u64 = 40_000_000;

/// PORT-DEVIATION D46: the size a still of `width x height` gets on a device whose largest 2D texture is `max_dim` and
/// largest buffer `max_buffer` bytes — the request itself when it fits, else scaled down with its aspect kept.
pub fn still_size(width: u32, height: u32, max_dim: u32, max_buffer: u64) -> (u32, u32) {
    let (w, h) = (width.max(1), height.max(1));
    // the read-back buffer's rows are padded to 256 bytes: keep a margin under the limit
    let max_px = (STILL_MAX_PIXELS as f64).min(max_buffer as f64 / 4.0 * 0.85);
    let k = (max_dim as f64 / w.max(h) as f64).min((max_px / (w as f64 * h as f64)).sqrt()).min(1.0);
    if k < 1.0 { (((w as f64 * k).floor() as u32).max(1), ((h as f64 * k).floor() as u32).max(1)) } else { (w, h) }
}

/// The staging belt's chunk: larger than the biggest single frame upload of an ordinary scene.
const BELT_CHUNK: u64 = 1 << 22;
/// The main view's far plane — the camera object's `+8`, 700.0 (0x4677FC) — of the screen z the
/// depth bias offsets (`RE/notes/thrash.md` §6d.3). An offscreen pass names its own
/// (`render_offscreen`'s `depth_far`).
pub const VIEW_FAR: f32 = 700.0;

/// ENHANCEMENT E8: `tan` of half the environment faces' field of view — 48 degrees, so 96 degrees across:
/// a face is a little wider than the 90 degrees that tile the sphere, and the shader reads only the middle
/// 90 of it, so bilinear filtering never reaches a face's edge.
pub const REFL_TAN_HALF: f32 = 1.110_612_5;

/// ENHANCEMENT E8: the environment faces' view directions and up vectors, the order of the array's
/// layers: +X, -X, +Z, -Z, +Y, -Y (`shader.wgsl` `refl_face` holds the same table).
pub const REFL_FACES: [([f32; 3], [f32; 3]); 6] = [
    ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
    ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
    ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ([0.0, -1.0, 0.0], [0.0, 0.0, -1.0]),
];

/// ENHANCEMENT E8: the player's car's environment — six faces in one array texture, plus the target a
/// face is drawn into.
struct ReflCube {
    size: u32,
    /// the cube's texel format: `Rgba16Float` with the float scene (the sun and the bright lamps keep their radiance above 1
    /// for the clear coat to reflect), else the surface's
    format: wgpu::TextureFormat,
    array: wgpu::Texture,
    /// all mips (the shader's) and mip 0 alone (the probe's)
    array_view: wgpu::TextureView,
    base_view: wgpu::TextureView,
    /// ENHANCEMENT ENH-COAT: faces drawn since their prefiltered mips were made
    stale: u8,
    face: wgpu::Texture,
    face_view: wgpu::TextureView,
    /// bit `i` set once face `i` has been drawn; the shader takes the map only when all six are
    faces_done: u8,
}

/// How many uncaptured wgpu errors the handler has seen; ENHANCEMENT: [`Core::apply`] sees the count rise and forces every
/// enhancement off for the rest of the run (the original path), so an enhancement's pipeline or texture the device refuses never
/// stops the game.
static GPU_ERRORS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// PLATFORM GLUE (robustness, no visual change): wgpu's default for an error no scope caught is a panic ("Handling wgpu errors as
/// fatal by default"), which on a GUI Windows release closes the game silently. The handler installed in [`Core::new`] logs the
/// error (the first 40, then every 1000th) and the game goes on.
fn gpu_error(e: wgpu::Error) {
    let n = GPU_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 40 || n % 1000 == 0 {
        // (its Display carries the cause: "Validation Error / Caused by: In Device::create_texture ...")
        tracing::error!(count = n, "wgpu error (uncaptured): {e}");
    }
}

/// PLATFORM GLUE (Windows): `dxcompiler.dll` + `dxil.dll` next to the exe (the Windows archive ships both; DXC needs dxil.dll to sign
/// its shaders): the path to hand `Dx12Compiler::DynamicDxc`, or `None` (FXC) when either is missing or off Windows.
fn win_dxc_path() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let (dxc, dxil) = (dir.join("dxcompiler.dll"), dir.join("dxil.dll"));
    (dxc.is_file() && dxil.is_file()).then(|| dxc.to_string_lossy().into_owned())
}

/// PLATFORM GLUE: the instance for `backends`; `dxc` = the DXC library path for the Direct3D 12 backend, `None` = FXC.
/// `WGPU_DX12_COMPILER` still wins over it.
fn new_instance(window: &Arc<Window>, backends: wgpu::Backends, dxc: Option<&str>) -> wgpu::Instance {
    let mut dx12 = wgpu::Dx12BackendOptions::default();
    dx12.shader_compiler = match dxc {
        Some(p) => wgpu::Dx12Compiler::DynamicDxc { dxc_path: p.to_owned() },
        None => wgpu::Dx12Compiler::Fxc,
    };
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        backend_options: wgpu::BackendOptions { dx12, ..Default::default() }.with_env(),
        // EGL needs the display before any surface exists
        ..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(window.clone()))
    })
}

/// PLATFORM GLUE (Windows): an Intel (vendor 0x8086) integrated GPU on a non-Direct3D backend is moved to its Direct3D 12 adapter.
pub(crate) fn prefer_dx12_over(vendor: u32, device_type: wgpu::DeviceType, backend: wgpu::Backend) -> bool {
    vendor == 0x8086 && device_type == wgpu::DeviceType::IntegratedGpu && backend != wgpu::Backend::Dx12
}

/// The per-item arrays of one `encode_frame`, kept between frames so a frame allocates none.
#[derive(Default)]
struct FrameScratch {
    /// the items' object uniforms, uploaded in one copy
    objects: Vec<ObjectUniform>,
    /// ENHANCEMENT ENH-TEMPORAL: the items' sky flags and roles of a temporal pass (empty otherwise)
    sky_flags: Vec<bool>,
    roles: Vec<f32>,
    /// the 2D pass's sprite vertices, built here and copied into the belt (wgpu's mapped views are write-only)
    sprites: Vec<SpriteVertex>,
}

/// ENHANCEMENT E1: the multisampled colour / depth a 3D pass draws into (resolved into the pass's
/// own `view`).
struct Ms {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    samples: u32,
}

/// IEEE half to single (the lens probe's read of the float image).
fn f16_to_f32(h: u16) -> f32 {
    let (sign, exp, man) = ((h >> 15) as u32, ((h >> 10) & 0x1F) as i32, (h & 0x3FF) as u32);
    let v = match exp {
        0 => man as f32 * 2f32.powi(-24),
        31 => {
            if man == 0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        e => (1.0 + man as f32 / 1024.0) * 2f32.powi(e - 15),
    };
    if sign == 1 { -v } else { v }
}

/// Single to IEEE half (round to nearest even; overflow to infinity, tiny values to zero / subnormals).
pub(crate) fn f32_to_f16(v: f32) -> u16 {
    let b = v.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32;
    let man = b & 0x7F_FFFF;
    if exp == 0xFF {
        return sign | 0x7C00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7C00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = (man | 0x80_0000) >> (1 - e);
        let round = (m >> 12) & 1 != 0 && ((m & 0xFFF) != 0 || (m >> 13) & 1 != 0);
        return sign | ((m >> 13) as u16 + round as u16);
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let round = (man >> 12) & 1 != 0 && ((man & 0xFFF) != 0 || (man >> 13) & 1 != 0);
    h += round as u32;
    sign | h as u16
}

#[cfg(test)]
mod tests {
    use super::{f16_to_f32, GlobalsUniform, ObjectUniform};
    use std::mem::size_of;

    /// The body of `struct <name> { ... }` in a source text: the `(field, type)` pairs in order (comments dropped).
    fn struct_fields(src: &str, name: &str) -> Option<Vec<(String, String)>> {
        let head = format!("struct {name} {{");
        let start = src.find(&head)? + head.len();
        let body = &src[start..src[start..].find('}')? + start];
        let mut fields = Vec::new();
        for line in body.lines() {
            let line = line.split("//").next().unwrap_or("").trim();
            // one field per line (a WGSL `array<vec4<f32>, 12>` has a comma of its own)
            let decl = line.trim_end_matches(',');
            if !decl.is_empty() {
                let (n, t) = decl.split_once(':').expect("a `name: type` field");
                fields.push((n.trim().to_string(), t.split_whitespace().collect::<String>()));
            }
        }
        Some(fields)
    }

    /// WGSL type -> bytes in a uniform / storage struct (every member here is a multiple of 16 bytes).
    fn wgsl_size(t: &str) -> usize {
        match t {
            "mat4x4<f32>" => 64,
            "vec4<f32>" => 16,
            _ => {
                let inner = t.strip_prefix("array<").and_then(|r| r.strip_suffix('>')).unwrap_or_else(|| panic!("WGSL type `{t}` unknown to the layout test"));
                let (elem, n) = inner.rsplit_once(',').expect("array<T,N>");
                wgsl_size(elem) * n.parse::<usize>().unwrap_or_else(|_| panic!("array length `{n}` is not a literal"))
            }
        }
    }

    /// Rust field type -> bytes; `crate::RAIN_LAMPS` / `RAIN_EDGES` are resolved by name.
    fn rust_size(t: &str) -> usize {
        match t {
            "[[f32;4];4]" => 64,
            "[f32;4]" => 16,
            "[[f32;4];crate::RAIN_LAMPS]" => 16 * crate::RAIN_LAMPS,
            "[[f32;4];crate::RAIN_EDGES]" => 16 * crate::RAIN_EDGES,
            _ => panic!("Rust type `{t}` unknown to the layout test"),
        }
    }

    /// A WGSL module that declares one of the uniform / storage structs the Rust side fills must match it field for
    /// field (names, order, total size). SHADOWFIX: `gpu/shadow.wgsl`'s `Object` lacked the `soft` / `hdr` vec4s that
    /// were appended to `ObjectUniform`, so its array stride was wrong and the shadows flickered. Append a field in one
    /// place only and this fails.
    #[test]
    fn wgsl_structs_match_the_rust_uniforms() {
        let rust_src = include_str!("gpu.rs");
        let cases: [(&str, &str, usize); 2] = [("Globals", "GlobalsUniform", size_of::<GlobalsUniform>()), ("Object", "ObjectUniform", size_of::<ObjectUniform>())];
        // every .wgsl file under src/ that declares one of the two structs (a new module is picked up by itself)
        let mut files = Vec::new();
        let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "wgsl") {
                    files.push(p);
                }
            }
        }
        let mut checked = 0;
        for (wgsl_name, rust_name, size) in cases {
            let rust = struct_fields(rust_src, rust_name).expect("the Rust struct");
            assert_eq!(rust.iter().map(|(_, t)| rust_size(t)).sum::<usize>(), size, "{rust_name}: the parsed fields do not add up to size_of (padding?)");
            for f in &files {
                let src = std::fs::read_to_string(f).unwrap();
                let Some(wgsl) = struct_fields(&src, wgsl_name) else { continue };
                checked += 1;
                let (rn, wn): (Vec<_>, Vec<_>) = (rust.iter().map(|(n, _)| n.as_str()).collect(), wgsl.iter().map(|(n, _)| n.as_str()).collect());
                assert_eq!(rn, wn, "{}: `struct {wgsl_name}` fields differ from {rust_name}", f.display());
                assert_eq!(wgsl.iter().map(|(_, t)| wgsl_size(t)).sum::<usize>(), size, "{}: `struct {wgsl_name}` is not {size} bytes ({rust_name})", f.display());
            }
        }
        assert!(checked >= 3, "expected shader.wgsl's Globals + Object and shadow.wgsl's Object, checked {checked}");
    }

    /// ENHANCEMENT E7: the lens probe's half-float decode (IEEE binary16).
    #[test]
    fn half_decode() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x7BFF), 65504.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24)); // the smallest subnormal
        assert_eq!(f16_to_f32(0x0400), 2f32.powi(-14)); // the smallest normal
        assert_eq!(f16_to_f32(0x3555), 0.333_251_95); // 1/3 rounded
        assert_eq!(f16_to_f32(0x7C00), f32::INFINITY);
        assert!(f16_to_f32(0x7E00).is_nan());
    }
}

#[cfg(test)]
mod still_tests {
    use super::*;

    #[test]
    fn a_still_fits_the_device() {
        // a 1x / 2x still of a 1280 x 800 window fits as asked
        assert_eq!(still_size(1280, 800, 8192, 1 << 28), (1280, 800));
        assert_eq!(still_size(2560, 1600, 8192, 1 << 28), (2560, 1600));
        // 4x of 1920 x 1080 fits the 8192 side and the pixel cap
        assert_eq!(still_size(7680, 4320, 8192, 1 << 28), (7680, 4320));
        // 4x of 2560 x 1600 is past the 8192 texture side and the 40 MP cap: scaled down, the aspect kept
        let (w, h) = still_size(10240, 6400, 8192, 1 << 28);
        assert!(w <= 8192 && w as u64 * h as u64 <= STILL_MAX_PIXELS && (w as f32 / h as f32 - 1.6).abs() < 0.01, "{w}x{h}");
        // the buffer limit: 256 MB at 4 bytes a pixel, less the margin
        let (w, h) = still_size(8000, 5000, 16384, 1 << 28);
        assert!((w as u64 * h as u64 * 4) as f64 <= (1u64 << 28) as f64 * 0.85 && (w as f32 / h as f32 - 1.6).abs() < 0.01, "{w}x{h}");
        // the pixel cap
        let (w, h) = still_size(16000, 10000, 16384, 1 << 34);
        assert!(w as u64 * h as u64 <= STILL_MAX_PIXELS, "{w}x{h}");
        assert_eq!(still_size(0, 0, 8192, 1 << 28), (1, 1));
    }
}

#[cfg(test)]
mod adapter_choice_tests {
    use super::prefer_dx12_over;

    /// Only an Intel integrated GPU on a non-Direct3D backend is moved to Direct3D 12.
    #[test]
    fn dx12_preferred_for_intel_integrated_only() {
        use wgpu::{Backend as B, DeviceType as D};
        assert!(prefer_dx12_over(0x8086, D::IntegratedGpu, B::Vulkan));
        assert!(prefer_dx12_over(0x8086, D::IntegratedGpu, B::Gl));
        assert!(!prefer_dx12_over(0x8086, D::IntegratedGpu, B::Dx12));
        assert!(!prefer_dx12_over(0x8086, D::DiscreteGpu, B::Vulkan)); // Arc
        assert!(!prefer_dx12_over(0x10de, D::DiscreteGpu, B::Vulkan)); // NVIDIA
        assert!(!prefer_dx12_over(0x1002, D::IntegratedGpu, B::Vulkan)); // AMD APU
    }
}
