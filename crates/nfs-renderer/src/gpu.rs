use crate::front::{Cmd, CmdList};
use crate::{depth_bias_offset, Camera, FakeSortMode, LensProbe, LensRead, MaterialDesc, MeshData, Scene, Sprite, SpriteCompare, TextureData, Vertex};
use anyhow::{Context, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

mod csm;
mod shadowmap;
mod carsun;
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
mod kit;
mod setup;
mod state;
#[cfg(target_os = "macos")]
mod diag;

// the items the rest of gpu/ names from the two modules above (their own glob `use super::*` sees these)
use pipelines::{build_mesh_pipelines, HDR_KEY};
pub(crate) use csm::part_casts_vertices;
pub use refl::{NEAR_BOX_FLOOR, REFL_FACES, REFL_TAN_HALF};
use refl::{NearPass, ReflCube};
use kit::{f16_to_f32, is_bgra};
pub(crate) use kit::f32_to_f16;
pub use readback::{still_size, STILL_MAX_PIXELS};
use setup::{DeviceSetup, GPU_ERRORS};
use post::{GammaPass, ScaleBlit, ToneKey};
use frame::FrameDesc;
#[cfg(test)]
pub(crate) use post::{blit_source, gamma_source};
#[cfg(test)]
pub(crate) use refl::near_mask_source;
#[cfg(test)]
pub(crate) use carsun::source as carsun_source;
#[cfg(test)]
pub(crate) use csm::shadow_source;

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
    /// ENHANCEMENT E49: `.x` 1 = the clear coat reads the near lookup (0 = off, master's value), `.y` 1 = the player's faces (the
    /// near cars, layers 6..11) are all drawn, `.z` the proxy box's weight at the cube's eye (0 = no box: the plain lookup)
    near: [f32; 4],
    /// ENHANCEMENT E49: the proxy box (a covered stretch's): `[0]` its centre + half length, `[1]` forward + half width, `[2]`
    /// lateral + the floor's height, `[3]` up + the ceiling's height
    near_box: [[f32; 4]; 4],
    /// ENHANCEMENT E49: each face's eye (`.xyz`; the faces are drawn a few a frame as the car moves), `[6]` the newest
    near_eye: [[f32; 4]; 7],
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
    /// ENHANCEMENT E9: the shape the casters' rule reads (a level flat sheet: no caster; SHADOW-UNIFY: upright / built)
    shape: csm::MeshShape,
    /// ENHANCEMENT E1 (FOLIAGE-OVERDRAW): every vertex has colour alpha 1 (`solid.rs`; once a write breaks it, `false` for good)
    alpha_one: bool,
    /// ENHANCEMENT ENH-LIVELEAK: carved out of a shared arena (its ranges go to the free list on release), the reserved byte sizes of
    /// the two ranges (the slot a later mesh may take over), and whether the mesh was released (an empty stub now)
    arena: bool,
    cap: (u64, u64),
    released: bool,
}

/// A byte range of a buffer a [`GpuMesh`] is made from: `len` is the mesh's own bytes, `cap` the bytes reserved for it (the slot a
/// later mesh may take over).
struct BufRange {
    buf: wgpu::Buffer,
    offset: u64,
    len: u64,
    cap: u64,
}

impl GpuMesh {
    /// A static mesh carved out of an arena buffer ([`Core::upload_mesh`]): the ranges, the buffers' serial, its bounds and shape
    /// (ENHANCEMENT E9) and whether every vertex has colour alpha 1 (ENHANCEMENT E1).
    fn in_arena(vertex: BufRange, index: BufRange, serial: u32, index_count: u32, bounds: ([f32; 3], f32), shape: csm::MeshShape, alpha_one: bool) -> Self {
        Self {
            cap: (vertex.cap, index.cap),
            vertex_buf: vertex.buf,
            vertex_offset: vertex.offset,
            vertex_len: vertex.len,
            index_buf: index.buf,
            index_offset: index.offset,
            index_len: index.len,
            index_count,
            vb_serial: serial,
            ib_serial: serial,
            writes: 0,
            bounds,
            shape,
            alpha_one,
            arena: true,
            released: false,
        }
    }

    /// Bind this mesh's own slices of its buffers (vertices and indices) in slot 0 / as the index buffer: an empty range binds 4
    /// bytes, as a zero-size slice cannot be bound.
    fn bind(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_vertex_buffer(0, self.vertex_buf.slice(self.vertex_offset..self.vertex_offset + self.vertex_len.max(4)));
        pass.set_index_buffer(self.index_buf.slice(self.index_offset..self.index_offset + self.index_len.max(4)), wgpu::IndexFormat::Uint32);
    }

    /// A dynamic mesh ([`Core::upload_dynamic_mesh`]): buffers of its own, from offset 0, the whole buffers its range and reserve.
    fn dynamic(vertex_buf: wgpu::Buffer, index_buf: wgpu::Buffer, serial: u32, index_count: u32, bounds: ([f32; 3], f32), alpha_one: bool) -> Self {
        let (vertex_len, index_len) = (vertex_buf.size(), index_buf.size());
        Self {
            vertex_buf,
            vertex_offset: 0,
            vertex_len,
            index_buf,
            index_offset: 0,
            index_len,
            index_count,
            vb_serial: serial,
            ib_serial: serial,
            writes: 0,
            bounds,
            shape: csm::MeshShape::default(),
            alpha_one,
            arena: false,
            cap: (vertex_len, index_len),
            released: false,
        }
    }

    /// What a released mesh becomes: the shared 8-byte stub buffers, nothing to draw ([`Core::release_mesh`]).
    fn stub(vertex_buf: wgpu::Buffer, index_buf: wgpu::Buffer, serial: u32) -> Self {
        Self {
            vertex_buf,
            vertex_offset: 0,
            vertex_len: 0,
            index_buf,
            index_offset: 0,
            index_len: 0,
            index_count: 0,
            vb_serial: serial,
            ib_serial: serial,
            writes: 0,
            bounds: ([0.0; 3], 0.0),
            shape: csm::MeshShape::default(),
            alpha_one: true,
            arena: false,
            cap: (0, 0),
            released: true,
        }
    }
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

/// `night.psh`'s `nght` bitmap (the headlight projector's map): 128 x 128 bytes.
const NIGHT_MAP_BYTES: usize = 128 * 128;

/// One arena's size for each of the two buffers; a larger mesh gets an arena of its own size.
const ARENA_CHUNK: u64 = 4 << 20;

/// [`Vertex`]'s size: the vertex buffer's stride
const VERTEX_STRIDE: u64 = std::mem::size_of::<Vertex>() as u64;
const _: () = assert!(VERTEX_STRIDE.is_multiple_of(8), "a vertex range stays 8-byte aligned");

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
    /// ENHANCEMENT E1 (FOLIAGE-OVERDRAW): the smallest alpha (bytes) any texel of any level has, or a bound below it (`solid.rs`; 0
    /// when not known)
    min_alpha: u8,
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

#[derive(Clone)]
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

/// A scene sampler: the address modes (`clamp_*`: clamp to the edge, else repeat; `w` always repeats), the magnification /
/// minification and mip filters, `anisotropy_clamp` and `lod_max_clamp`.
#[derive(Clone, Copy)]
struct SamplerSpec {
    clamp_u: bool,
    clamp_v: bool,
    filter: wgpu::FilterMode,
    mip: wgpu::MipmapFilterMode,
    anisotropy: u16,
    lod_max: f32,
}

impl SamplerSpec {
    /// wgpu's default `lod_max_clamp`: every level.
    const ALL_LEVELS: f32 = 32.0;

    /// The sampler of key `k`'s address modes, every level, no anisotropy.
    fn of_key(k: materials::SamplerKey, filter: wgpu::FilterMode, mip: wgpu::MipmapFilterMode) -> Self {
        Self { clamp_u: k.clamp[0], clamp_v: k.clamp[1], filter, mip, anisotropy: 1, lod_max: Self::ALL_LEVELS }
    }

    /// The same sampling level 0 only.
    fn level0(self) -> Self {
        Self { lod_max: 0.0, ..self }
    }

    fn build(self, device: &wgpu::Device, label: &str) -> wgpu::Sampler {
        let mode = |c: bool| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
        device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some(label),
            address_mode_u: mode(self.clamp_u),
            address_mode_v: mode(self.clamp_v),
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: self.filter,
            min_filter: self.filter,
            mipmap_filter: self.mip,
            lod_max_clamp: self.lod_max,
            anisotropy_clamp: self.anisotropy,
            ..Default::default()
        })
    }
}

/// ENHANCEMENT E2: a scene sampler by address mode — linear magnification / minification, the
/// given mip filter, `anisotropy_clamp` (wgpu wants every filter linear for anything above 1) and an
/// optional `lod_max_clamp`.
fn make_scene_sampler(device: &wgpu::Device, clamp_u: bool, clamp_v: bool, mip: wgpu::MipmapFilterMode, anisotropy: u16, lod_max: Option<f32>) -> wgpu::Sampler {
    SamplerSpec { clamp_u, clamp_v, filter: wgpu::FilterMode::Linear, mip, anisotropy, lod_max: lod_max.unwrap_or(SamplerSpec::ALL_LEVELS) }.build(device, "scene E2")
}

pub(crate) struct Core {
    /// the meshes, textures, materials and the writes deferred to the encoder
    res: state::Resources,
    /// the mesh pipelines and the shader modules they are built from
    pipes: state::PipelineCache,
    /// the environment cube and what is made from it
    env: state::ReflState,
    /// the sampler tables, the sprite pass's and the projector's
    samplers: state::Samplers,
    /// what the bindings hold until the real resource exists
    dummies: state::Dummies,
    /// the bind group layouts of the 3-D passes and the sprite pass
    layouts: state::Layouts,
    /// the lens flare's frame read-backs
    lens: state::Lens,
    /// diagnostics and the error fallback
    diag: state::Diag,
    /// the draw state the command list sets: trilinear, FakeSort mode, gamma, the mirror view
    modes: state::DrawModes,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    /// the present modes the surface supports (ENHANCEMENT E6 picks among them)
    present_modes: Vec<wgpu::PresentMode>,
    depth_view: wgpu::TextureView,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    headlight_map_buf: wgpu::Buffer,
    proj_tex: Option<TextureId>,
    head_tex: Option<TextureId>,
    sky_bgs: Vec<Option<wgpu::BindGroup>>,
    compute_supported: bool,
    /// ENHANCEMENT E12: the depth converter (built on first use), and the 1 x 1 image the binding
    /// holds while no soft depth is bound
    soft: Option<crate::soft::SoftDepth>,
    /// ENHANCEMENT E12: the split pass's globals bind group with the soft depth image bound, and the image and the plain globals
    /// bind group it was made from ([`Core::soft_globals_bg`])
    soft_globals: Option<(wgpu::TextureView, wgpu::BindGroup, wgpu::BindGroup)>,
    /// ENHANCEMENT E14: the corner-shading passes (built on first use)
    ssao: Option<crate::ao::Ssao>,
    /// ENHANCEMENT E13: the light-shafts pass (built on first use)
    shafts: Option<crate::shafts::ShaftsPass>,
    /// ENHANCEMENT E36: the haze pass (built on first use)
    haze: Option<crate::haze::HazePass>,
    /// ENHANCEMENT E44: the lighthouse beam pass (built on first use)
    lbeam: Option<crate::lbeam::LBeamPass>,
    /// the adapter draws with a base vertex (`DownlevelFlags::BASE_VERTEX`; every Metal, Vulkan and
    /// Direct3D 12 device, not every OpenGL one): the draws of one arena bind its buffers once and
    /// address a mesh by `first_index` / `base_vertex` ([`Core::draw_run`])
    base_vertex: bool,
    /// ENH-LITE: how strong the adapter is (integrated / software = weak), for the first-run set
    gpu_class: crate::GpuClass,
    gamma_pass: Option<GammaPass>,
    /// The enhanced game's switches ([`crate::Enhancements`]), set through the command list; the
    /// core starts on [`crate::Enhancements::ORIGINAL`], whose every GPU-side branch is `master`'s
    /// code path. An enhancement tests `self.enhancements.is_on(..)` and nothing else.
    pub(crate) enhancements: crate::Enhancements,
    /// ENHANCEMENT E1: the sample counts (2, 4, 8) the colour format and the depth format both
    /// support multisampled and resolved, from the adapter (bit `n.trailing_zeros()`)
    ms_supported: u32,
    /// ENHANCEMENT E7: the device can draw and blend into `Rgba16Float` and filter it
    hdr_ok: bool,
    /// ENHANCEMENT E33: the EDR output's state (macOS; a stub elsewhere that is never on)
    edr: crate::edr::Edr,
    /// ENHANCEMENT E7: [`Self::ms_supported`] for `Rgba16Float`
    ms_supported_hdr: u32,
    /// ENHANCEMENT E7: the tone-map mode of the 3D pass being encoded (0 = as the original, 1 =
    /// ACES); `None` = the original's 8-bit target. Set around `encode_frame` by the E7 paths only.
    cur_hdr: Option<u32>,
    /// ENHANCEMENT E7: the rear-view mirror's own float image + tone map (its size differs from the
    /// main view's)
    scale_blit_mirror: Option<ScaleBlit>,
    /// PERF (PERF-B): per item of the frame, its variant ([`fill::ItemVariant::of`])
    item_var: Vec<fill::ItemVariant>,
    /// ENHANCEMENT E1 (FOLIAGE-OVERDRAW): per item of the frame, whether its alpha test can never fail ([`Core::solid_draws`])
    solid: Vec<bool>,
    /// ENHANCEMENT E1: multisampled colour + depth targets by (width, height, samples)
    ms_cache: std::collections::HashMap<(u32, u32, u32), (wgpu::TextureView, wgpu::TextureView)>,
    /// ENHANCEMENT E3: the scaled 3D image and the blit that lays it over the window-sized image
    scale_blit: Option<ScaleBlit>,
    /// ENHANCEMENT ENH-TEMPORAL: the temporal foundation (jitter, motion vectors, masks, the debug pass), made when
    /// `NFS_TEMPORAL_DEBUG` is set or a temporal upscaler is on ([`Self::temporal_want`]); `None` = off, master's frame
    temporal: Option<crate::temporal::Temporal>,
    /// ENHANCEMENT ENH-TEMPORAL: the object pass's second layout (the reprojection matrices), made at first use
    temporal_gpu: Option<crate::temporal::Gpu>,
    /// ENHANCEMENT ENH-TEMPORAL: the targets and bind group of the frame being encoded (set around `encode_frame`)
    cur_temporal: Option<crate::temporal::Cur>,
    /// ENHANCEMENT ENH-TEMPORAL: the reprojection matrices of the frame being encoded (kept between frames)
    motion: crate::temporal::Motion,
    /// ENHANCEMENT ENH-TEMPORAL: the last temporal main view's inputs for an upscaler
    temporal_inputs: Option<crate::temporal::Inputs>,
    /// ENHANCEMENTS E37 / E34 / E39: the upscalers ([`Upscaling`])
    up: Upscaling,
    /// ENHANCEMENT UPSCALE-SOFT: the texture LOD bias added to every material's own while an upscaler's main view is encoded
    /// (`upscale::mip_bias`); 0 otherwise, and then the object uniforms are master's bytes
    cur_lod_shift: f32,
    /// ENHANCEMENT E35: SMAA 1x over the tone-mapped image, made at first use
    smaa: Option<crate::smaa::Smaa>,
    /// ENHANCEMENTS E10 / E21 / E27 / the depth of field (photo mode, E31) / E40 / E28: the post chain's passes (each built at its first use)
    post: chain::PostPasses,
    object_buf: wgpu::Buffer,
    object_bg: wgpu::BindGroup,
    sprite_pipeline: wgpu::RenderPipeline,
    /// `sprite_pipeline` for a sprite pass with no depth image (no sprite of the frame has a depth state)
    sprite_flat_pipeline: wgpu::RenderPipeline,
    /// The front end's depth-state sprites (`Sprite::depth`): depth writes on, the alpha test, the
    /// compare `LESSEQUAL` \[0\] / `GREATEREQUAL` \[1\].
    sprite_depth_pipelines: [wgpu::RenderPipeline; 2],
    /// offscreen render targets handed out by [`Core::render_offscreen`], by texture index
    offscreen: std::collections::HashMap<u32, (u32, u32)>,
    sprite_buf: wgpu::Buffer,
    /// the per-frame upload path: every buffer write of a frame (globals, objects, sprites, the
    /// dynamic meshes' vertices) is a copy out of a recycled mapped chunk, recorded into the
    /// frame's own encoder — `Queue::write_buffer` makes a fresh staging buffer per call
    belt: wgpu::util::StagingBelt,
    /// the frame's command buffers not yet submitted (PERF, picture identical): the mirror, the environment faces, their
    /// coat prefilter and probe and the main frame are recorded in call order into one encoder and submitted in up to three
    /// parts ([`Core::submit_frame`]): ahead of the main view, ahead of its post chain, at the frame's end
    frame_cmds: FrameCmds,
    /// the main view is being recorded ([`Core::whole_frame`]): its post chain is submitted apart from its 3-D image
    frame_main: bool,
    /// depth targets by size, shared by every offscreen pass of that size (each pass clears it)
    depth_cache: std::collections::HashMap<(u32, u32), wgpu::TextureView>,
    /// the pass's item indices by stage (sky, opaque, blended, overlay), kept between frames
    buckets: [Vec<u32>; 4],
    /// the per-item arrays `encode_frame` builds every frame, kept between frames (no allocation per frame)
    frame_scratch: FrameScratch,
    /// ENHANCEMENT E9: the sun shadow maps' pipelines, uniform and map
    csm: csm::CsmState,
    /// ENHANCEMENT E51 (CARSH-FADE): the near cars' measured share of the sun (built on first use; level 1 with compute shaders)
    car_sun: Option<carsun::CarSun>,
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

/// The frame's recorded work in submit order: the command buffers finished so far (an upscaler's own sits between two of
/// ours) and the encoder still open, whose copies come from the staging belt. Empty between frames.
#[derive(Default)]
pub(crate) struct FrameCmds {
    done: Vec<wgpu::CommandBuffer>,
    open: Option<wgpu::CommandEncoder>,
}

impl FrameCmds {
    /// The frame's open encoder (a new one when none is open), taken to record into; hand it back with [`Self::park`].
    pub(crate) fn take(&mut self, device: &wgpu::Device) -> wgpu::CommandEncoder {
        self.open.take().unwrap_or_else(|| device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") }))
    }

    /// The encoder back, still open: what is recorded next follows what it holds.
    pub(crate) fn park(&mut self, encoder: wgpu::CommandEncoder) {
        debug_assert!(self.open.is_none(), "one open encoder a frame");
        self.open = Some(encoder);
    }

    /// Close `encoder` (the frame's so far) ahead of a command buffer recorded elsewhere that must run after it.
    pub(crate) fn close(&mut self, encoder: wgpu::CommandEncoder) {
        self.done.push(crate::prof::finish(encoder));
    }

    /// Close the frame's encoder `encoder` and carry on in a new one named `label`: what is recorded next runs after the
    /// command buffers pushed in between.
    pub(crate) fn split(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, label: &'static str) {
        let first = std::mem::replace(encoder, device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) }));
        self.close(first);
    }

    /// A command buffer recorded elsewhere (FSR, MetalFX), run after what is closed so far.
    pub(crate) fn push(&mut self, cb: wgpu::CommandBuffer) {
        self.done.push(cb);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.open.is_none() && self.done.is_empty()
    }
}

/// ENHANCEMENTS E34 / E37: an upscaler backend's state, made at its first use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BackendState {
    /// not tried yet
    #[default]
    Untried,
    Ready,
    /// it could not be made, or it failed: the switch behaves as off for the rest of the run
    Unavailable,
}

/// ENHANCEMENTS E37 / E34 / E39: the upscalers' backends (each built at its first use), their images and the frame's choice
/// (`encode_with_gamma_inner`).
#[derive(Default)]
struct Upscaling {
    /// ENHANCEMENT E37: the temporal upscaler's backend (FSR 3.1, or MetalFX's temporal scaler: `temporal_kind`; built at first use) and its state (unavailable: logged once)
    temporal: Option<Box<dyn crate::upscale::Upscaler>>,
    temporal_state: BackendState,
    /// ENHANCEMENT E34 (temporal levels): the same slot also holds MetalFX's temporal scaler (macOS): which backend `temporal` is, and E34's own
    /// state (unavailable / failed)
    temporal_kind: Option<crate::upscale::UpscalerKind>,
    #[cfg(target_os = "macos")]
    mfxt_state: BackendState,
    /// ENHANCEMENT E37: the frame being encoded goes through FSR
    temporal_active: bool,
    /// ENHANCEMENT E37: FSR's float output (display size) and the bind group the post chain reads it by
    temporal_out: Option<crate::gfx::RtImage>,
    /// ENHANCEMENT E37: `encode_with_gamma_inner` calls so far, and the call number of the last FSR frame (a frame in between
    /// that was not one - a menu, a loading screen, another view - drops the history)
    frame_no: u64,
    temporal_last_frame: u64,
    /// ENHANCEMENT E34: the upscaler backend (built at first use) and its state (unavailable: logged once)
    upscaler: Option<Box<dyn crate::upscale::Upscaler>>,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    upscale_state: BackendState,
    /// ENHANCEMENT E34: the frame being encoded goes through the upscaler (set by `encode_with_gamma_inner`)
    upscale_active: bool,
    /// ENHANCEMENT E34: the scaler's input / output images and the plain blit that lays its output over the window image
    upscale_images: Option<crate::upscale::UpscaleImages>,
    upscale_present: Option<ScaleBlit>,
    /// ENHANCEMENT E39 (UPSCALE-SOFT): the sharpening after the MetalFX scalers (built at first use)
    sharpen: Option<crate::sharpen::Sharpen>,
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

/// How many collected lens read-back buffers are kept for the next copies (a frame copies at most two blocks).
const LENS_POOL: usize = 4;

/// The most lens read-backs left waiting; older ones are dropped (a read-back that never completes does not pile up).
const LENS_PENDING_MAX: usize = 8;

/// The size-keyed target caches (E1's multisampled targets, the depth targets) hold at most this many sizes before they are
/// emptied (a window being resized asks for a new size every frame).
const TARGET_CACHE_SIZES: usize = 6;

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
/// PERF (PERF-B): a [`Run`]'s pipeline index carries its set above this bit (0 = `Core::pipelines`, k = `pipeline_vars[k - 1]`)
const SET_SHIFT: usize = 20;

/// One item's draw as `encode_frame`'s walk ([`Core::bucket_items`]) records it, for every pass of the frame that draws it.
#[derive(Clone, Copy)]
struct ItemDraw {
    /// the pipeline without depth writes (a `FakeSort` item's first draw) and the one it draws with
    plain: pipelines::PipelineKey,
    group: pipelines::PipelineKey,
    /// `LayerFalseSort1` / `2`'s double draw: `plain`, then `group`, never merged into a run
    twice: bool,
    /// `material * 2 + point-filtered bind group`
    material: u32,
    /// the sky-map pass's section map (the white texture when there is none), and whether there is one
    sky: u32,
    sky_on: bool,
    /// the mesh drawn (ENHANCEMENT ENH-LIVETEX: a refined twin, `Core::shown_mesh`)
    mesh: u32,
}

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
/// The 8-bit texture format of the game's textures, the projector default and the dummy images (UNORM, not sRGB: the game's
/// surface is a byte surface).
const TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// ENHANCEMENT E9: a dynamic mesh's bounding sphere (the shadow casters' cull) is the uploaded vertices' grown by this, as its
/// vertices are rewritten every frame (`write_mesh_vertices`) and move a little around where they started.
const DYNAMIC_BOUNDS_SLACK: f32 = 1.25;

/// ENHANCEMENT E8: the bits of all six environment faces (`ReflCube::faces_done`).
const ALL_FACES: u8 = 0x3F;

/// ENHANCEMENT E7: the 3D scene's float target format.
pub(crate) const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const MAX_OBJECTS: u64 = 8192;
/// The staging belt's chunk: larger than the biggest single frame upload of an ordinary scene.
const BELT_CHUNK: u64 = 1 << 22;
/// The main view's far plane — the camera object's `+8`, 700.0 (0x4677FC) — of the screen z the
/// depth bias offsets (`RE/notes/thrash.md` §6d.3). An offscreen pass names its own
/// (`render_offscreen`'s `depth_far`).
pub const VIEW_FAR: f32 = 700.0;

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
    /// the items' draw records ([`ItemDraw`])
    draws: Vec<ItemDraw>,
    /// ENHANCEMENT E9 / E15 / E47: the frame's casters ([`Core::frame_casters`])
    casters: Vec<shadowmap::Caster>,
}

/// Where a window-sized frame ([`Core::encode_with_gamma`]) goes: `view` of `tex` (`width x height`, the surface's format), the
/// depth image of that size, whether `tex` may be copied from (`can_copy`: the lens flare's read-back) and the 3-D view's aspect.
#[derive(Clone, Copy)]
struct FrameOut<'a> {
    view: &'a wgpu::TextureView,
    tex: &'a wgpu::Texture,
    depth: &'a wgpu::TextureView,
    can_copy: bool,
    width: u32,
    height: u32,
    aspect: f32,
}

impl<'a> FrameOut<'a> {
    /// A frame into `tex` (its own size; the aspect is the image's).
    fn of(view: &'a wgpu::TextureView, tex: &'a wgpu::Texture, depth: &'a wgpu::TextureView) -> Self {
        let (width, height) = (tex.width(), tex.height());
        Self { view, tex, depth, can_copy: true, width, height, aspect: width as f32 / height as f32 }
    }
}

/// ENHANCEMENT E1 / E3 / E7 / ENH-TEMPORAL: how [`Core::encode_enhanced`] draws the 3-D image: its sample count, its scale of the
/// window's size, the float scene's tone-map mode (`None` = 8-bit) and whether it is a temporal main view.
#[derive(Clone, Copy)]
struct EnhancedFrame {
    samples: u32,
    scale: f32,
    hdr: Option<u32>,
    temporal: bool,
}

/// ENHANCEMENT E1: the multisampled colour / depth a 3D pass draws into (resolved into the pass's
/// own `view`).
struct Ms {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    samples: u32,
}

#[cfg(test)]
mod tests {
    use super::{GlobalsUniform, ObjectUniform};
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
            // ENHANCEMENT E49: the faces' eyes
            "[[f32;4];7]" => 16 * 7,
            // ENHANCEMENT E9 / E51 (SHADOW-REFACTOR): the cascades' uniform
            "[[[f32;4];4];MAX_CASCADES]" => 64 * super::csm::MAX_CASCADES,
            "[[f32;4];MAX_CASCADES]" => 16 * super::csm::MAX_CASCADES,
            "[[f32;4];MAX_COVER*COVER_VEC4S]" => 16 * super::csm::MAX_COVER * 5,
            "[[f32;4];crate::MAX_CONTACTS]" => 16 * crate::MAX_CONTACTS,
            "[[f32;4];crate::MAX_CONTACTS/4]" => 16 * (crate::MAX_CONTACTS / 4),
            _ => panic!("Rust type `{t}` unknown to the layout test"),
        }
    }

    /// A WGSL module that declares one of the uniform / storage structs the Rust side fills must match it field for
    /// field (names, order, total size). SHADOWFIX: `gpu/shadow.wgsl`'s `Object` lacked the `soft` / `hdr` vec4s that
    /// were appended to `ObjectUniform`, so its array stride was wrong and the shadows flickered. Append a field in one
    /// place only and this fails.
    #[test]
    fn wgsl_structs_match_the_rust_uniforms() {
        let gpu_src = include_str!("gpu.rs");
        // (SHADOW-REFACTOR: and E9's cascades, `Csm` / `CsmUniform`)
        let cases: [(&str, &str, usize, &str); 3] = [
            ("Globals", "GlobalsUniform", size_of::<GlobalsUniform>(), gpu_src),
            ("Object", "ObjectUniform", size_of::<ObjectUniform>(), gpu_src),
            ("Csm", "CsmUniform", size_of::<super::csm::CsmUniform>(), include_str!("gpu/csm/mod.rs")),
        ];
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
        // (and the WGSL pieces the Rust side holds: the items' `struct Object`, `classes.rs`)
        let mut sources: Vec<(String, String)> = files.iter().map(|f| (f.display().to_string(), std::fs::read_to_string(f).unwrap())).collect();
        sources.push(("classes.rs OBJECT_WGSL".into(), crate::classes::OBJECT_WGSL.into()));
        let mut checked = 0;
        for (wgsl_name, rust_name, size, rust_src) in cases {
            let rust = struct_fields(rust_src, rust_name).expect("the Rust struct");
            assert_eq!(rust.iter().map(|(_, t)| rust_size(t)).sum::<usize>(), size, "{rust_name}: the parsed fields do not add up to size_of (padding?)");
            for (name, src) in &sources {
                let Some(wgsl) = struct_fields(src, wgsl_name) else { continue };
                checked += 1;
                let (rn, wn): (Vec<_>, Vec<_>) = (rust.iter().map(|(n, _)| n.as_str()).collect(), wgsl.iter().map(|(n, _)| n.as_str()).collect());
                assert_eq!(rn, wn, "{name}: `struct {wgsl_name}` fields differ from {rust_name}");
                assert_eq!(wgsl.iter().map(|(_, t)| wgsl_size(t)).sum::<usize>(), size, "{name}: `struct {wgsl_name}` is not {size} bytes ({rust_name})");
            }
        }
        assert!(checked >= 3, "expected shader.wgsl's Globals + Csm and classes.rs's Object, checked {checked}");
    }

}
