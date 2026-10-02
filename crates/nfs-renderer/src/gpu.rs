use crate::front::{Cmd, CmdList};
use crate::{depth_bias_offset, Camera, FakeSortMode, LensProbe, LensRead, MaterialDesc, MeshData, Scene, Sprite, SpriteCompare, TextureData, Vertex};
use anyhow::{Context, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MeshId(pub(crate) u32);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextureId(pub(crate) u32);
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
}

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
    /// How many vertex writes this mesh has had since its upload (every `write_mesh_vertices` /
    /// `update_mesh_vertices` that queued bytes): lets a caller that keeps the last vertices it
    /// wrote tell whether anyone else wrote the mesh since ([`Core::mesh_writes`]).
    writes: u32,
}

/// The arena the static meshes are carved from: a vertex and an index buffer, filled front to back.
// PORT-DEVIATION D6 (RE/notes/PORT-DEVIATIONS.md): static meshes in shared arenas and instanced draws from one object storage buffer; draw order and state as before, a polygon seam can land one pixel differently (not D3D7-exact, never was).
struct MeshArena {
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

struct GpuTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    sprite_bg: Option<wgpu::BindGroup>,
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
}

pub(crate) struct Core {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
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
    /// `[0x6115CC]+0x190`: set while the view being drawn is the rear-view mirror's (type 1,
    /// 0x43AD39…0x43AD4E), when the specular and sky-map layers take no texture and skip their passes
    no_reflections: bool,
    /// the scene samplers by address mode and mip filter — index `clamp_u | clamp_v << 1 |
    /// point_mip << 2`
    samplers: [wgpu::Sampler; 8],
    /// the `FakeSort` single-draw arm's samplers (Thrash 7 ← 0, `MAG`/`MINFILTER` `POINT`), by
    /// address mode alone — index `clamp_u | clamp_v << 1` — used only for a single-draw item
    point_samplers: [wgpu::Sampler; 4],
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
    sprite_sampler: wgpu::Sampler,
    object_buf: wgpu::Buffer,
    object_bg: wgpu::BindGroup,
    meshes: Vec<GpuMesh>,
    arena: Option<MeshArena>,
    textures: Vec<GpuTexture>,
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
    /// the surface carries `COPY_SRC`, so the lens flare's frame read-back can copy from it
    /// ([`Self::set_lens_enabled`])
    lens_enabled: bool,
    /// read-backs issued and not yet collected, oldest first
    lens_pending: Vec<LensPending>,
    /// read-backs collected, for the front to hand to the game
    lens_done: Vec<LensRead>,
    /// blocks copied into the frame being encoded, mapped after its submit
    lens_staged: Vec<(wgpu::Buffer, bool)>,
    /// `NFS_SPRITE_LOG`, read once
    sprite_log: bool,
}

/// One lens-flare read-back in flight: the 4 × 4 block copied into `buf` (rows `LENS_ROW` bytes
/// apart), mapped once the copy has run.
struct LensPending {
    buf: wgpu::Buffer,
    reference: bool,
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
    material: u32,
    sky: u32,
    mesh: u32,
}

impl DrawState {
    const NONE: Self = Self { pipeline: usize::MAX, material: u32::MAX, sky: u32::MAX, mesh: u32::MAX };
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
const MAX_OBJECTS: u64 = 8192;
/// The staging belt's chunk: larger than the biggest single frame upload of an ordinary scene.
const BELT_CHUNK: u64 = 1 << 22;
/// The main view's far plane — the camera object's `+8`, 700.0 (0x4677FC) — of the screen z the
/// depth bias offsets (`RE/notes/thrash.md` §6d.3). An offscreen pass names its own
/// (`render_offscreen`'s `depth_far`).
pub const VIEW_FAR: f32 = 700.0;

impl Core {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL | wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = instance.create_surface(window.clone()).context("create surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .context("no suitable GPU adapter")?;
        tracing::info!(adapter = ?adapter.get_info().name, backend = ?adapter.get_info().backend, "gpu adapter");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nfs-renderer"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
        .context("request device")?;

        let caps = surface.get_capabilities(&adapter);
        // a byte surface, as Direct3D 7's was: colours, blends and interpolation work on the stored
        // bytes, not in linear light (`RE/RENDERING.md`, "Colour space")
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            // the byte pipeline's surface (`RENDERING.md` "Colour space"): no colour-space conversion
            color_space: Default::default(),
        };
        surface.configure(&device, &config);
        let depth_view = Self::make_depth(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                // the 128 × 128 `nght` bytes, four to a u32
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                // the car shadow projector's texture (`[0x6115CC]+0x18C`) and its sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // `HEAD` of `Render/common.fsh`, the headlight decal `LayerUVLight` projects
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
            ],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                // every object of a pass in one buffer, indexed by `instance_index`
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<ObjectUniform>() as u64),
                },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                // the specular texture's sampler: `WRAP` both ways, set on each map by the binder
                // 0x440940 (0x537A70(1) → `tex+0x38` block `+0x14` / `+0x18`, Thrash 0x41 / 0x42)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sky_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sky map"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&object_layout), Some(&material_layout), Some(&sky_layout)],
            immediate_size: 0,
        });
        // Thrash state 0x38's pairs (`dx7z.dll` 0x600046BB): 1 `SRCALPHA`/`ONE` (the general arm;
        // two device kinds take `ONE`/`ONE`), 2 `ZERO`/`INVSRCALPHA`, 3 `DESTCOLOR`/`ZERO`
        let pair_state = |pair: u8| {
            use wgpu::BlendFactor as F;
            let (src, dst) = match pair {
                1 => (F::SrcAlpha, F::One),
                2 => (F::Zero, F::OneMinusSrcAlpha),
                3 => (F::Dst, F::Zero),
                _ => return wgpu::BlendState::ALPHA_BLENDING,
            };
            let c = wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
            wgpu::BlendState { color: c, alpha: c }
        };
        let make_pipeline = |blend: bool, pair: u8, cull: Option<wgpu::Face>, depth: bool, zwrite: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mesh pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(if blend { "fs_main" } else { "fs_opaque" }),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(if blend { pair_state(pair) } else { wgpu::BlendState::REPLACE }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // mirrored clip x (see Camera::view_projection) flips the apparent winding
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth && (!blend || zwrite)),
                    // `ZFUNC` `LESSEQUAL`: `dx7z.dll` makes the device with it (0x6000A545) and the
                    // game sets it again through the view (Thrash 0x28 ← 3: the race pass
                    // 0x468B54, the front end's 3D pass 0x4D301D). The front-end rooms' layers —
                    // light pools and cones on the floor and walls — share their surface's depth
                    // and pass because of it, drawn after it in `Base+70` order
                    // (`RE/notes/frontend-3d.md` §9). *(The port kept `LESS` until 2026-09-25,
                    // for a fight that was the rooms' own states — file order, no pair — not the
                    // depth test.)*
                    depth_compare: Some(if depth { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // six a group — the cull (back, none, front) × depth (on, off), `2·cull + !depth` — and the
        // groups: 0 blend off, 1 the plain alpha pair, 2…4 the other pairs, 5…8 every pair again
        // with depth writes on (`device_states`)
        const CULLS: [Option<wgpu::Face>; 3] = [Some(wgpu::Face::Back), None, Some(wgpu::Face::Front)];
        // (the 54 pipelines compile independently: built on worker threads, kept in the same order)
        let mut specs = Vec::with_capacity(54);
        for (blend, pair, zwrite) in [(false, 0, false), (true, 0, false), (true, 1, false), (true, 2, false), (true, 3, false), (true, 0, true), (true, 1, true), (true, 2, true), (true, 3, true)] {
            for i in 0..6 {
                specs.push((blend, pair, CULLS[i / 2], i % 2 == 0, zwrite));
            }
        }
        let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
        let pipelines: Vec<wgpu::RenderPipeline> = std::thread::scope(|sc| {
            let make_pipeline = &make_pipeline;
            let handles: Vec<_> = specs
                .chunks(specs.len().div_ceil(workers))
                .map(|c| sc.spawn(move || c.iter().map(|&(blend, pair, cull, depth, zwrite)| make_pipeline(blend, pair, cull, depth, zwrite)).collect::<Vec<_>>()))
                .collect();
            handles.into_iter().flat_map(|h| h.join().expect("pipeline worker")).collect()
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let headlight_map_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headlight map"),
            size: 128 * 128,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // The projector's texture is the one its track's `sh80` material loaded: 0x44B000 →
        // 0x44D320 only looks it up, by the part's key `(+0x18, +0x1C)`, which on each of the five
        // tracks with projectors is that material's `(mt+0x24, mt+0x28)` (`texblock` example). The
        // material's block is `{…, bias −1, 0, 0}` on every one — `ADDRESSU` / `ADDRESSV` `CLAMP`
        // (`RE/notes/thrash.md` §7) — and `sh80` has 7 mips, so the mip filter is the loader's
        // `POINT`, `LINEAR` at `SPECIALFX` 3 (0x44D571, 0x535A60); the bias is the shader's. (Until
        // 2026-09-25 this sampler clamped as an approximation and read the top level only.) The
        // `HEAD` decal samples its top level explicitly through the same sampler: `common.fsh`
        // loads with no state block (0x44D270 → 0x44D2E9 pushes 0), so `HEAD` keeps the
        // constructor's default 0x5379D0 — `CLAMP` / `CLAMP`, `LINEAR`, mip `NONE`
        // (`RE/notes/lighting.md` §5.2).
        let make_proj_sampler = |mip: wgpu::MipmapFilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("projector clamp"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: mip,
                ..Default::default()
            })
        };
        let proj_samplers = [make_proj_sampler(wgpu::MipmapFilterMode::Linear), make_proj_sampler(wgpu::MipmapFilterMode::Nearest)];
        let proj_default = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("projector default"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &proj_default, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &[255, 255, 255, 0],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        );
        let proj_default_view = proj_default.create_view(&wgpu::TextureViewDescriptor::default());
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: headlight_map_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&proj_default_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&proj_samplers[0]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&proj_default_view) },
            ],
        });
        let object_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: std::mem::size_of::<ObjectUniform>() as u64 * MAX_OBJECTS,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let object_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("objects"),
            layout: &object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &object_buf,
                    offset: 0,
                    size: None,
                }),
            }],
        });
        // A texture's own state block (0x5379D0, `RE/notes/thrash.md` §7) asks for linear
        // magnification and minification (Thrash 7 = 1 → `MAG`/`MINFILTER` `LINEAR`), and a mip
        // filter that is `POINT` for a texture with mips (0x44D571…0x44D587) and becomes `LINEAR`
        // at `SPECIALFX` 3, the default (0x535A60). No Thrash state reaches `MAXANISOTROPY` or an
        // anisotropic filter, so there is none here (until 2026-09-24 this sampler clamped
        // anisotropy at 8, the port's own). The promotion to `LINEAR` is `[0x69DD1D]`, which
        // 0x44ED10 clears and sets again only at `SPECIALFX` 3 (0x44EDD1, 0x44EF04) and the front
        // end's 0x44F020 clears (0x44F080) — `set_trilinear`; below it the mip filter is `POINT`.
        let make_sampler = |clamp_u: bool, clamp_v: bool, mag_min: wgpu::FilterMode, mip: wgpu::MipmapFilterMode| {
            let mode = |c: bool| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene"),
                address_mode_u: mode(clamp_u),
                address_mode_v: mode(clamp_v),
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: mag_min,
                min_filter: mag_min,
                mipmap_filter: mip,
                ..Default::default()
            })
        };
        let samplers = std::array::from_fn(|i| {
            let mip = if i & 4 == 0 { wgpu::MipmapFilterMode::Linear } else { wgpu::MipmapFilterMode::Nearest };
            make_sampler(i & 1 != 0, i & 2 != 0, wgpu::FilterMode::Linear, mip)
        });
        // `LayerFalseSort1`/`2`'s other arm sets Thrash 7 ← 0 for the one draw (`MAG`/`MINFILTER`
        // `POINT`) and never touches state 0x0B, so the mip filter here follows the same `POINT`
        // default as every other sampler below `SPECIALFX` 3 (`trilinear` is false whenever
        // `fakesort_mode` can pick `Single`/`PerItem` — both are set from the same level, `main.rs`)
        let point_samplers = std::array::from_fn(|i| make_sampler(i & 1 != 0, i & 2 != 0, wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest));

        // The 2D pass needs its own sampler. The 3D one once asked for `anisotropy_clamp: 8` and
        // `mipmap_filter: Linear`, which on a single-level texture — every sprite atlas is one —
        // sampled by a quad a dozen pixels across returns nothing on Metal. That is the fault
        // recorded as `RE/AUDIT.md` F2: a glyph quad drawn with a 1×1 texture composites fine while
        // the same quad with the font atlas draws nothing at all, whatever its UV window.
        let sprite_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_max_clamp: 0.0,
            ..Default::default()
        });

        // 2D sprite pipeline (HUD)
        //
        // Blended for every sprite, the front end's included (`RE/notes/thrash.md` §8, settled
        // 2026-09-26): the FE kit 0x4D8E10 has an opaque `Rs` block `[0x65B948]` (Thrash 0x0A ← 0)
        // that the flush 0x4DA5F0 picks by the queued item's byte `+8`, but only the untextured
        // fills set it — 0x4D9280 / 0x4D96B0 / 0x4D98D0 from colours that are all `0xFF` alpha,
        // where blending gives the same pixel — while every image draw takes it from its caller
        // (0x4D9B30's arg 8, also via 0x4D9AF0's arg 6; 0x4D9E70's arg 12, also via 0x4D9E10's
        // arg 10), and all 72 call sites pass 0. `dx7z.dll` turns `ALPHABLENDENABLE` on or off
        // only for Thrash 0x0A (0x60003EFE) and at device creation; binding a texture
        // (0x60008000 → 0x60007F30) sets nothing else. The device's alpha test (`GREATER`,
        // reference 0 / 1 / 10 by what the front end drew last) is carried only for a sprite
        // under a depth state (`Sprite::depth`), where it decides which pixels write depth; on a
        // plain sprite it would drop texels of alpha ≤ 10 that blending lays at ≤ 4 %.
        let sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sprite"), source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()) });
        let sprite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let sprite_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sprite"), bind_group_layouts: &[Some(&sprite_layout)], immediate_size: 0 });
        let sprite_vertex = [Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<SpriteVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32] })];
        let make_sprite_pipeline = |label: &str, fs: &str, depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&sprite_pl),
                vertex: wgpu::VertexState { module: &sprite_shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &sprite_vertex },
                fragment: Some(wgpu::FragmentState { module: &sprite_shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sprite_pipeline = make_sprite_pipeline("sprite", "fs_main", wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() });
        // A front-end sprite under the kit's depth state (`crate::SpriteDepth`): Thrash 0x39 ← 1
        // (0x4D30BE) and the compare Thrash 0x28 sets — 3 `LESSEQUAL` (0x4D30C9), 6 `GREATEREQUAL`
        // inside a mask (0x4DAAC0 → 0x537830) — with the device's alpha test in the shader.
        let depth_state = |compare| wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true), depth_compare: Some(compare), stencil: Default::default(), bias: Default::default() };
        let sprite_depth_pipelines = [
            make_sprite_pipeline("sprite lessequal", "fs_alpha_test", depth_state(wgpu::CompareFunction::LessEqual)),
            make_sprite_pipeline("sprite greaterequal", "fs_alpha_test", depth_state(wgpu::CompareFunction::GreaterEqual)),
        ];
        let belt = wgpu::util::StagingBelt::new(device.clone(), BELT_CHUNK);
        let sprite_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("sprites"), size: MAX_SPRITES * 6 * std::mem::size_of::<SpriteVertex>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let mut r = Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            pipelines,
            globals_buf,
            globals_bg,
            globals_layout,
            proj_samplers,
            _proj_default: proj_default,
            proj_default_view,
            headlight_map_buf,
            proj_tex: None,
            head_tex: None,
            object_layout,
            material_layout,
            sky_layout,
            sky_bgs: Vec::new(),
            no_reflections: false,
            samplers,
            point_samplers,
            trilinear: false,
            fakesort_mode: FakeSortMode::default(),
            gamma: 1.0,
            gamma_pass: None,
            sprite_sampler,
            object_buf,
            object_bg,
            meshes: Vec::new(),
            arena: None,
            textures: Vec::new(),
            materials: Vec::new(),
            white: TextureId(0),
            sprite_pipeline,
            sprite_depth_pipelines,
            offscreen: std::collections::HashMap::new(),
            sprite_layout,
            sprite_buf,
            belt,
            pending_writes: Vec::new(),
            pending_bytes: Vec::new(),
            pending_textures: Vec::new(),
            pending_texture_bytes: Vec::new(),
            depth_cache: std::collections::HashMap::new(),
            buckets: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            lens_enabled: false,
            lens_pending: Vec::new(),
            lens_done: Vec::new(),
            lens_staged: Vec::new(),
            sprite_log: std::env::var("NFS_SPRITE_LOG").is_ok(),
        };
        let _ = &r.object_layout;
        r.white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Ok(r)
    }

    fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
        Self::make_depth_sized(device, config.width, config.height)
    }

    fn make_depth_sized(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// Execute a frame's command list (`front::CmdList`) in call order — the start of the frame's
    /// encode: every mutation the scene build made since the last one. A queued vertex / texture write
    /// lands in the next encoder (`flush_mesh_writes` / `flush_texture_writes`), as it did when the
    /// callers wrote to the renderer directly; an offscreen pass encodes and submits here, in its place
    /// in the order, so the writes before it are in its encoder and those after it are not.
    pub(crate) fn apply(&mut self, list: &mut CmdList) {
        let CmdList { cmds, bytes, .. } = list;
        for c in cmds.drain(..) {
            match c {
                Cmd::WriteMesh { mesh, start, len } => self.defer_mesh_write(MeshId(mesh), &bytes[start..start + len]),
                Cmd::UpdateTextureLevel { texture, level, width, height, start, len } => self.queue_texture_level(TextureId(texture), level, width, height, &bytes[start..start + len]),
                Cmd::HeadlightMap(map) => self.set_headlight_map(&map),
                Cmd::Projector(t) => self.set_projector_texture(t),
                Cmd::HeadlightTexture(t) => self.set_headlight_texture(t),
                Cmd::MirrorView(on) => self.set_mirror_view(on),
                Cmd::Trilinear(on) => self.set_trilinear(on),
                Cmd::FakeSort(m) => self.set_fakesort_mode(m),
                Cmd::Gamma(g) => self.set_gamma(g),
                Cmd::Resize(w, h) => self.resize(w, h),
                Cmd::LensProbe(on) => self.set_lens_enabled(on),
                Cmd::CreateMaterial(d) => {
                    self.create_material(d);
                }
                Cmd::DuplicateMaterial { id, texture } => {
                    self.duplicate_material_texture(id, texture);
                }
                Cmd::DynamicMesh(m) => {
                    self.upload_dynamic_mesh(&m);
                }
                Cmd::Offscreen(j) => {
                    let id = self.render_offscreen(&j.scene, &j.camera, j.width, j.height, j.reuse, j.clear, j.depth_far);
                    debug_assert_eq!(id, j.id, "offscreen id allocated on the front differs from the core's");
                }
            }
        }
        bytes.clear();
    }

    pub(crate) fn white(&self) -> TextureId {
        self.white
    }
    pub(crate) fn texture_count(&self) -> u32 {
        self.textures.len() as u32
    }
    /// `(vertex bytes, writes)` per mesh, for the front's mirror.
    pub(crate) fn mesh_infos(&self) -> Vec<(u64, u32)> {
        self.meshes.iter().map(|m| (m.vertex_len, m.writes)).collect()
    }
    pub(crate) fn mesh_vertex_len(&self, id: MeshId) -> u64 {
        self.meshes[id.0 as usize].vertex_len
    }
    pub(crate) fn material_textures(&self) -> Vec<TextureId> {
        self.materials.iter().map(|m| m.texture).collect()
    }
    pub(crate) fn offscreen_sizes(&self) -> std::collections::HashMap<u32, (u32, u32)> {
        self.offscreen.clone()
    }

    /// Let the surface be copied from, for the lens flare's frame read-back. Off (the default) the
    /// surface is a render attachment alone.
    pub fn set_lens_enabled(&mut self, on: bool) {
        if self.lens_enabled == on {
            return;
        }
        self.lens_enabled = on;
        self.config.usage = if on { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::RENDER_ATTACHMENT };
        self.surface.configure(&self.device, &self.config);
        if !on {
            self.lens_pending.clear();
        }
    }

    /// Collect the lens-flare read-backs that have finished (their copies ran and their buffers are
    /// mapped) into [`Self::lens_done`]; the cost of each is what this takes, in whole milliseconds.
    fn collect_lens(&mut self) {
        if self.lens_pending.is_empty() {
            return;
        }
        let t0 = std::time::Instant::now();
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut keep = Vec::new();
        for p in std::mem::take(&mut self.lens_pending) {
            match p.rx.try_recv() {
                Ok(Ok(())) => {
                    let mut pixels = [0u32; 16];
                    {
                        let data = p.buf.slice(..).get_mapped_range().expect("mapped range");
                        for (row, chunk) in pixels.chunks_exact_mut(4).enumerate() {
                            for (i, px) in chunk.iter_mut().enumerate() {
                                let o = row * LENS_ROW as usize + i * 4;
                                *px = u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
                            }
                        }
                    }
                    p.buf.unmap();
                    self.lens_done.push(LensRead { reference: p.reference, pixels, cost_ms: t0.elapsed().as_millis() as u32 });
                }
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
                Err(std::sync::mpsc::TryRecvError::Empty) => keep.push(p),
            }
        }
        // a read-back that never completes does not pile up
        let n = keep.len();
        if n > 8 {
            keep.drain(..n - 8);
        }
        self.lens_pending = keep;
    }

    /// The finished read-backs, taken.
    pub(crate) fn take_lens(&mut self) -> Vec<LensRead> {
        std::mem::take(&mut self.lens_done)
    }

    /// PORT-DEVIATION D9 (PORT-DEVIATIONS.md) [0x460460 / dx7z 0x60003990, a blocking lock in the frame]:
    /// Copy the 4 × 4 block at the probe's pixel from `target` into a fresh staging buffer and map
    /// it; [`Self::collect_lens`] picks it up when the copy has run. The block's corner is clamped as
    /// `nfs5.dll`'s 0x10003320 does for every `THRASH_readrect` (the call at 0x4603FF / 0x460525 is
    /// routed through it): not below 0, and `corner + 4` below the width / height — else `W − 5`
    /// / `H − 5` (`dx7z.dll` 0x60003990 itself does no clipping).
    fn lens_copy(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Texture, probe: LensProbe, reference: bool) {
        let (w, h) = (target.width() as i32, target.height() as i32);
        if w < 5 || h < 5 {
            return;
        }
        let (px, py) = ((probe.x / 640.0 * w as f32).round_ties_even() as i32 - 2, (probe.y / 480.0 * h as f32).round_ties_even() as i32 - 2);
        // 0x10003320: `max(v, 0)`, then `v + 4 >= size` → `size − 5`
        let clamp = |v: i32, size: i32| {
            let v = v.max(0);
            if v + 4 >= size { size - 5 } else { v }
        };
        let (x, y) = (clamp(px, w), clamp(py, h));
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("lens read-back"), size: LENS_ROW * 4, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d { x: x as u32, y: y as u32, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(LENS_ROW as u32), rows_per_image: Some(4) } },
            wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
        );
        // mapped after the frame's submit ([`Self::lens_map_staged`])
        self.lens_staged.push((buf, reference));
    }

    /// Ask for the mapping of every block copied this frame; call after the submit.
    fn lens_map_staged(&mut self) {
        for (buf, reference) in std::mem::take(&mut self.lens_staged) {
            let (tx, rx) = std::sync::mpsc::channel();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            self.lens_pending.push(LensPending { buf, reference, rx });
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth_view = Self::make_depth(&self.device, &self.config);
    }

    /// The surface's pixel size.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width.max(1), self.config.height.max(1))
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    /// A mesh whose vertices are rewritten every frame — [`Self::write_mesh_vertices`]. The index
    /// buffer and the index count are fixed at upload; only the vertex data moves.
    pub fn upload_dynamic_mesh(&mut self, mesh: &MeshData) -> MeshId {
        let vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dynamic mesh vb"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dynamic mesh ib"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let (vertex_len, index_len) = (vertex_buf.size(), index_buf.size());
        self.meshes.push(GpuMesh { vertex_buf, vertex_offset: 0, vertex_len, index_buf, index_offset: 0, index_len, index_count: mesh.indices.len() as u32, writes: 0 });
        MeshId(self.meshes.len() as u32 - 1)
    }

    /// Queue a vertex write for the next frame's encoder (`flush_mesh_writes`); empty writes are
    /// dropped as `write_buffer` drops them.
    pub(crate) fn defer_mesh_write(&mut self, id: MeshId, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.meshes[id.0 as usize].writes = self.meshes[id.0 as usize].writes.wrapping_add(1);
        self.pending_writes.push(PendingWrite { mesh: id.0, start: self.pending_bytes.len(), len: bytes.len() });
        self.pending_bytes.extend_from_slice(bytes);
    }

    /// Record the queued vertex writes into `encoder`, from the staging belt, in call order.
    fn flush_mesh_writes(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for w in &self.pending_writes {
            let size = wgpu::BufferSize::new(w.len as u64).expect("non-empty write");
            let m = &self.meshes[w.mesh as usize];
            self.belt.write_buffer(encoder, &m.vertex_buf, m.vertex_offset, size).copy_from_slice(&self.pending_bytes[w.start..w.start + w.len]);
        }
        self.pending_writes.clear();
        self.pending_bytes.clear();
    }

    pub fn upload_mesh(&mut self, mesh: &MeshData) -> MeshId {
        let vbytes: &[u8] = bytemuck::cast_slice(&mesh.vertices);
        let ibytes: &[u8] = bytemuck::cast_slice(&mesh.indices);
        let (vlen, ilen) = (vbytes.len() as u64, ibytes.len() as u64);
        // every range is rounded to 8 bytes (a mapped range starts on a multiple of 8, a slice on 4;
        // and an empty one still reserves 8, so it can be sliced)
        let (vreserve, ireserve) = (vlen.next_multiple_of(8).max(8), ilen.next_multiple_of(8).max(8));
        let fits = self.arena.as_ref().is_some_and(|a| a.vertex_used + vreserve <= a.vertex_buf.size() && a.index_used + ireserve <= a.index_buf.size());
        if !fits {
            // a full arena goes to the queue now, so the host copies of a whole track's meshes do not pile up
            if self.arena.as_ref().is_some_and(|a| a.mapped) {
                self.seal_arena();
                self.queue.submit(std::iter::empty());
            }
            let make = |label: &'static str, usage: wgpu::BufferUsages, need: u64| {
                self.device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: ARENA_CHUNK.max(need), usage: usage | wgpu::BufferUsages::COPY_DST, mapped_at_creation: true })
            };
            self.arena = Some(MeshArena {
                vertex_buf: make("mesh arena vb", wgpu::BufferUsages::VERTEX, vreserve),
                vertex_used: 0,
                index_buf: make("mesh arena ib", wgpu::BufferUsages::INDEX, ireserve),
                index_used: 0,
                mapped: true,
            });
        }
        let a = self.arena.as_mut().expect("arena made");
        let (vertex_offset, index_offset) = (a.vertex_used, a.index_used);
        if a.mapped {
            if !vbytes.is_empty() {
                a.vertex_buf.slice(vertex_offset..vertex_offset + vlen).get_mapped_range_mut().expect("mapped arena").copy_from_slice(vbytes);
            }
            if !ibytes.is_empty() {
                a.index_buf.slice(index_offset..index_offset + ilen).get_mapped_range_mut().expect("mapped arena").copy_from_slice(ibytes);
            }
        } else {
            if !vbytes.is_empty() {
                self.queue.write_buffer(&a.vertex_buf, vertex_offset, vbytes);
            }
            if !ibytes.is_empty() {
                self.queue.write_buffer(&a.index_buf, index_offset, ibytes);
            }
        }
        a.vertex_used += vreserve;
        a.index_used += ireserve;
        self.meshes.push(GpuMesh { vertex_buf: a.vertex_buf.clone(), vertex_offset, vertex_len: vlen, index_buf: a.index_buf.clone(), index_offset, index_len: ilen, index_count: mesh.indices.len() as u32, writes: 0 });
        MeshId(self.meshes.len() as u32 - 1)
    }

    /// Unmap the mesh arena so the GPU can read it: what [`Self::upload_mesh`] copied into the mapped
    /// buffers becomes visible to the next frame. A later mesh that still fits goes in as a queued write.
    fn seal_arena(&mut self) {
        if let Some(a) = self.arena.as_mut().filter(|a| a.mapped) {
            a.vertex_buf.unmap();
            a.index_buf.unmap();
            a.mapped = false;
        }
    }

    pub fn upload_texture(&mut self, tex: &TextureData) -> TextureId {
        self.upload_texture_mips(tex.width, tex.height, &tex.mips)
    }

    /// [`Self::upload_texture`] over borrowed levels, so a caller that keeps its pixels does not
    /// clone a whole mip chain just to hand it over.
    pub fn upload_texture_mips(&mut self, width: u32, height: u32, mips: &[Vec<u8>]) -> TextureId {
        let t = Self::make_texture(&self.device, &self.queue, &self.sprite_layout, &self.sprite_sampler, width, height, mips);
        self.textures.push(t);
        TextureId(self.textures.len() as u32 - 1)
    }

    /// Several textures at once, the ids in the order given (the next free ones, as the same
    /// number of [`Self::upload_texture_mips`] calls would hand them). The device and the queue
    /// are thread-safe, so the textures are built on worker threads and pushed in order.
    pub fn upload_textures(&mut self, items: &[(u32, u32, &[Vec<u8>])]) -> Vec<TextureId> {
        let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8).min(items.len());
        let built: Vec<GpuTexture> = if workers <= 1 {
            items.iter().map(|&(w, h, m)| Self::make_texture(&self.device, &self.queue, &self.sprite_layout, &self.sprite_sampler, w, h, m)).collect()
        } else {
            let (device, queue, layout, sampler) = (&self.device, &self.queue, &self.sprite_layout, &self.sprite_sampler);
            let chunk = items.len().div_ceil(workers);
            std::thread::scope(|s| {
                let handles: Vec<_> = items
                    .chunks(chunk)
                    .map(|c| s.spawn(move || c.iter().map(|&(w, h, m)| Self::make_texture(device, queue, layout, sampler, w, h, m)).collect::<Vec<_>>()))
                    .collect();
                handles.into_iter().flat_map(|h| h.join().expect("texture worker")).collect()
            })
        };
        let first = self.textures.len() as u32;
        self.textures.extend(built);
        (first..self.textures.len() as u32).map(TextureId).collect()
    }

    fn make_texture(device: &wgpu::Device, queue: &wgpu::Queue, sprite_layout: &wgpu::BindGroupLayout, sprite_sampler: &wgpu::Sampler, width: u32, height: u32, mips: &[Vec<u8>]) -> GpuTexture {
        let size = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let mip_level_count = mips.len().max(1) as u32;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture"),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h) = (width, height);
        for (level, data) in mips.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let view = texture.create_view(&Default::default());
        let sprite_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite tex"),
            layout: sprite_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sprite_sampler) }],
        }));
        tracing::debug!(w = width, h = height, "texture uploaded");
        GpuTexture { _texture: texture, view, sprite_bg }
    }


    /// One mip level of `Renderer::update_texture`: `data` holds `h` rows of `4 · w` bytes (or more; the
    /// rest is ignored; less and the level is skipped).
    pub(crate) fn queue_texture_level(&mut self, id: TextureId, level: u32, w: u32, h: u32, data: &[u8]) {
        if self.textures.get(id.0 as usize).is_none() {
            return;
        }
        // queued for the next frame's encoder (`flush_texture_writes`), where `write_texture` would
        // have executed it (before that submit's commands); the rows are padded to 256 bytes
        let row = 4 * w as usize;
        let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        if data.len() >= row * h as usize {
            // a queued write of the same whole level is overwritten in place (the last wins, as the
            // later `write_texture` would), so frames that are not rendered do not pile writes up
            let same = self.pending_textures.iter().find(|q| q.texture == id.0 && q.level == level && q.width == w && q.height == h).map(|q| q.start);
            let start = same.unwrap_or(self.pending_texture_bytes.len());
            if same.is_none() {
                self.pending_texture_bytes.resize(start + padded * h as usize, 0);
                self.pending_textures.push(PendingTextureWrite { texture: id.0, level, width: w, height: h, bytes_per_row: padded as u32, start });
            }
            for y in 0..h as usize {
                self.pending_texture_bytes[start + y * padded..start + y * padded + row].copy_from_slice(&data[y * row..(y + 1) * row]);
            }
        }
    }

    /// Record the queued texture writes into `encoder`, from the staging belt, in call order.
    fn flush_texture_writes(&mut self, encoder: &mut wgpu::CommandEncoder) {
        for w in &self.pending_textures {
            let Some(gpu) = self.textures.get(w.texture as usize) else { continue };
            let len = w.bytes_per_row as usize * w.height as usize;
            let size = wgpu::BufferSize::new(len as u64).expect("non-empty write");
            let slice = self.belt.allocate(size, wgpu::BufferSize::new(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64).unwrap());
            slice.get_mapped_range_mut().expect("belt slice").copy_from_slice(&self.pending_texture_bytes[w.start..w.start + len]);
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo { buffer: slice.buffer(), layout: wgpu::TexelCopyBufferLayout { offset: slice.offset(), bytes_per_row: Some(w.bytes_per_row), rows_per_image: Some(w.height) } },
                wgpu::TexelCopyTextureInfo { texture: &gpu._texture, mip_level: w.level, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::Extent3d { width: w.width, height: w.height, depth_or_array_layers: 1 },
            );
        }
        self.pending_textures.clear();
        self.pending_texture_bytes.clear();
    }

    pub fn create_material(&mut self, desc: MaterialDesc) -> MaterialId {
        let texture = desc.texture.unwrap_or(self.white);
        let env = desc.env_texture.unwrap_or(self.white);
        let sampler = desc.clamp[0] as usize | (desc.clamp[1] as usize) << 1;
        let bind_group = self.material_bind_group(texture, sampler, env);
        let bind_group_point = desc.false_sort.then(|| self.material_bind_group_point(texture, sampler, env));
        self.materials.push(GpuMaterial {
            bind_group,
            bind_group_point,
            alpha_blend: desc.alpha_blend,
            blend_pair: desc.blend_pair.min(3),
            device_states: desc.device_states,
            sampler,
            lod_bias: desc.lod_bias,
            cull_back: desc.cull_back,
            view_cull: desc.view_cull,
            depth_bias: desc.depth_bias,
            false_sort: desc.false_sort,
            depth_test: desc.depth_test,
            env_mix: if desc.env_texture.is_some() { desc.env_mix } else { 0.0 },
            env_variant: desc.env_variant.min(2),
            sky_map: desc.sky_map,
            texture,
            env,
        });
        MaterialId(self.materials.len() as u32 - 1)
    }
    /// A new material identical to `id` but sampling `texture` in its place — same blend/cull/lod
    /// state and specular map, a fresh bind group. Used to give one traffic/cop car instance its
    /// own decal-page material when `nfs_game::world::CarModel::instantiate` clones the shared
    /// model's damage resources so two differently-damaged instances in one frame no longer alias
    /// (`RE/notes/car-model.md` §13.1: this renderer batches every frame's `DrawItem`s and reads
    /// each `MeshId`/`TextureId`/`MaterialId` once at `render()`, unlike the original's per-car
    /// synchronous blit 0x572B40).
    pub fn duplicate_material_texture(&mut self, id: MaterialId, texture: TextureId) -> MaterialId {
        let Some(m) = self.materials.get(id.0 as usize) else { return id };
        let (sampler, env, false_sort) = (m.sampler, m.env, m.false_sort);
        let (alpha_blend, blend_pair, device_states, lod_bias, cull_back, view_cull, depth_bias, depth_test, env_mix, env_variant, sky_map) =
            (m.alpha_blend, m.blend_pair, m.device_states, m.lod_bias, m.cull_back, m.view_cull, m.depth_bias, m.depth_test, m.env_mix, m.env_variant, m.sky_map);
        let bind_group = self.material_bind_group(texture, sampler, env);
        let bind_group_point = false_sort.then(|| self.material_bind_group_point(texture, sampler, env));
        self.materials.push(GpuMaterial {
            bind_group,
            bind_group_point,
            alpha_blend,
            blend_pair,
            device_states,
            sampler,
            lod_bias,
            cull_back,
            view_cull,
            depth_bias,
            false_sort,
            depth_test,
            env_mix,
            env_variant,
            sky_map,
            texture,
            env,
        });
        MaterialId(self.materials.len() as u32 - 1)
    }

    /// Point every reflective material at another environment map — the original re-binds the car's
    /// environment layers whenever the `.map` section under it changes (`RE/notes/lighting.md` §3).
    /// `night.psh`'s `nght` bitmap, 128 × 128 bytes, for the headlight projector.
    pub fn set_headlight_map(&mut self, map: &[u8]) {
        if map.len() == 128 * 128 {
            self.queue.write_buffer(&self.headlight_map_buf, 0, map);
        }
    }

    /// The car shadow projector's texture, `[0x6115CC]+0x18C` (`RE/notes/crp-geometry.md` §8);
    /// `None` for none.
    pub fn set_projector_texture(&mut self, tex: Option<TextureId>) {
        self.proj_tex = tex;
        self.rebuild_globals_bg();
    }

    /// `HEAD` of `Render/common.fsh`, what `Headlights::LayerUVLight` projects (0x452645 fetches it
    /// by that four-character code); `None` for none.
    pub fn set_headlight_texture(&mut self, tex: Option<TextureId>) {
        self.head_tex = tex;
        self.rebuild_globals_bg();
    }

    fn rebuild_globals_bg(&mut self) {
        let pick = |t: Option<TextureId>| match t.and_then(|t| self.textures.get(t.0 as usize)) {
            Some(t) => &t.view,
            None => &self.proj_default_view,
        };
        let (proj, head) = (pick(self.proj_tex), pick(self.head_tex));
        self.globals_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &self.globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.headlight_map_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(proj) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.proj_samplers[!self.trilinear as usize]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(head) },
            ],
        });
    }

    /// The view about to be drawn is the rear-view mirror's: `[0x6115CC]+0x190` is set from the
    /// view's type (`[[0x628130]+0x10]+0x2C == 1`, 0x43AD39…0x43AD4E; 0x4691D0 makes the mirror's
    /// view with type 1), and the specular and sky-map layers then bind no texture and skip their
    /// passes (0x445578, 0x445B69).
    /// The same flag keeps out the draws only the main view makes (`DrawItem::main_view_only`).
    pub fn set_mirror_view(&mut self, on: bool) {
        self.no_reflections = on;
    }

    /// The bind group for a section map, made the first time a car binds it.
    fn sky_bind_group(&mut self, tex: TextureId) {
        let i = tex.0 as usize;
        if self.sky_bgs.get(i).is_some_and(|b| b.is_some()) {
            return;
        }
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sky map"),
            layout: &self.sky_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.textures[tex.0 as usize].view) }],
        });
        if self.sky_bgs.len() <= i {
            self.sky_bgs.resize_with(i + 1, || None);
        }
        self.sky_bgs[i] = Some(bg);
    }

    /// A material's bind group: its texture, the sampler for its address modes under the current
    /// mip filter, and the environment map.
    fn material_bind_group(&self, texture: TextureId, sampler: usize, env: TextureId) -> wgpu::BindGroup {
        let sampler = &self.samplers[sampler | (!self.trilinear as usize) << 2];
        self.material_bind_group_with(texture, sampler, env)
    }

    /// The same bind group with `MAG`/`MINFILTER` `POINT` (Thrash 7 ← 0) — `LayerFalseSort1`'s
    /// other arm, for a `FakeSort` material only.
    fn material_bind_group_point(&self, texture: TextureId, sampler: usize, env: TextureId) -> wgpu::BindGroup {
        self.material_bind_group_with(texture, &self.point_samplers[sampler], env)
    }

    fn material_bind_group_with(&self, texture: TextureId, sampler: &wgpu::Sampler, env: TextureId) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.textures[texture.0 as usize].view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.textures[env.0 as usize].view) },
                // `WRAP` / `WRAP` (index 0: no clamp); sampled at level 0, so the mip half is moot
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.samplers[0]) },
            ],
        })
    }

    /// `[0x69DD1D]`, the mip filter's promotion (0x535B40 / 0x535A60, `RE/notes/thrash.md` §4):
    /// set, every texture with mips is sampled trilinear; clear, `POINT` between the levels.
    /// 0x44ED10 sets it only at `SPECIALFX` 3 on a card that took `setstate(0x0B, 3)`
    /// (`[0x6197A6]`, taken as accepted); the front end's 0x44F020 clears it.
    pub fn set_trilinear(&mut self, on: bool) {
        if self.trilinear == on {
            return;
        }
        self.trilinear = on;
        for i in 0..self.materials.len() {
            let m = &self.materials[i];
            self.materials[i].bind_group = self.material_bind_group(m.texture, m.sampler, m.env);
        }
        self.rebuild_globals_bg();
    }

    /// `LayerFalseSort1`/`2`'s per-frame pick (`main.rs`'s `App::fakesort_mode`, [`FakeSortMode`]).
    pub fn set_fakesort_mode(&mut self, mode: FakeSortMode) {
        self.fakesort_mode = mode;
    }

    /// State 0x2E's value as the game passes it; `dx7z.dll` 0x600051EE…0x6000522E: `≤ 0` → 0,
    /// not `< 4` → 4.
    pub fn set_gamma(&mut self, g: f32) {
        self.gamma = if g <= 0.0 { 0.0 } else if g < 4.0 { g } else { 4.0 };
    }

    /// Draw a frame into `target` (`width × height`, the surface's format): straight in at gamma
    /// 1, otherwise into the gamma pass's own image and then through the ramp into `target`.
    #[allow(clippy::too_many_arguments)]
    /// `can_copy`: `target_tex` (or the gamma pass's own image) may be copied from — the lens
    /// flare's blocks read the frame **before** the ramp, as `THRASH_readrect` reads the back
    /// buffer and the ramp is the display's.
    fn encode_with_gamma(&mut self, scene: &Scene, camera: &Camera, target: &wgpu::TextureView, target_tex: &wgpu::Texture, can_copy: bool, depth: &wgpu::TextureView, width: u32, height: u32, aspect: f32) {
        if self.gamma == 1.0 {
            self.encode_frame(scene, scene.clear_color, true, camera, target, target_tex, can_copy, depth, aspect, VIEW_FAR);
            return;
        }
        let mut pass = match self.gamma_pass.take() {
            Some(p) => p,
            None => GammaPass::new(&self.device, self.config.format),
        };
        let (src_tex, src) = pass.source(&self.device, self.config.format, width, height);
        self.encode_frame(scene, scene.clear_color, true, camera, &src, &src_tex, true, depth, aspect, VIEW_FAR);
        pass.resolve(&self.device, &self.queue, target, self.gamma);
        self.gamma_pass = Some(pass);
    }

    pub fn render(&mut self, scene: &Scene, camera: &Camera) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            // a timeout or an occluded window: skip the frame (wgpu 30)
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Ok(()),
            other => anyhow::bail!("surface texture: {other:?}"),
        };
        let view = frame.texture.create_view(&Default::default());
        let aspect = self.aspect();
        let depth = self.depth_view.clone();
        let (w, h) = (frame.texture.width(), frame.texture.height());
        let can_copy = self.lens_enabled;
        self.encode_with_gamma(scene, camera, &view, &frame.texture, can_copy, &depth, w, h, aspect);
        self.queue.present(frame);
        self.collect_lens();
        Ok(())
    }

    /// The depth target for a pass of this size: one per size, kept (every pass clears it first,
    /// so sharing it between offscreen passes is the same as a fresh texture each).
    fn depth_for(&mut self, width: u32, height: u32) -> wgpu::TextureView {
        let device = &self.device;
        self.depth_cache.entry((width, height)).or_insert_with(|| Self::make_depth_sized(device, width, height)).clone()
    }

    /// Render one frame into an offscreen RGBA8 image (for screenshots and automated checks).
    /// Render a scene's meshes into an offscreen texture that the sprite pass can then draw, so a
    /// 3D view can sit **inside** the 2D screen in the right drawing order — what the front end's
    /// `NFS5.3DCAR` widgets need (`RE/notes/frontend-3d.md` §4). The clear is transparent, so only
    /// the geometry shows, unless `clear` gives an opaque colour (the in-race rear-view mirror).
    /// Passing the id of a texture this returned earlier reuses it.
    /// `depth_far` is the far plane of the pass's own screen z — the scale of the state 0x18 depth
    /// bias (`encode_frame`): `None` for the main view's 700, the rear view's 65000 (0x4691D0).
    pub fn render_offscreen(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32, reuse: Option<TextureId>, clear_colour: Option<[f64; 4]>, depth_far: Option<f32>) -> TextureId {
        let (width, height) = (width.max(1), height.max(1));
        let id = match reuse.filter(|t| self.offscreen.get(&t.0).is_some_and(|(w, h)| *w == width && *h == height)) {
            Some(t) => t,
            None => {
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("offscreen 3d"),
                    size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let sprite_bg = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("offscreen tex"),
                    layout: &self.sprite_layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sprite_sampler) }],
                }));
                self.textures.push(GpuTexture { _texture: texture, view, sprite_bg });
                let id = TextureId(self.textures.len() as u32 - 1);
                self.offscreen.insert(id.0, (width, height));
                id
            }
        };
        let depth = self.depth_for(width, height);
        let view = self.textures[id.0 as usize].view.clone();
        // the scene's items and lights as they are, its own clear colour replaced and no sprites
        let target = self.textures[id.0 as usize]._texture.clone();
        self.encode_frame(scene, clear_colour.unwrap_or([0.0, 0.0, 0.0, 0.0]), false, camera, &view, &target, false, &depth, width as f32 / height as f32, depth_far.unwrap_or(VIEW_FAR));
        id
    }

    pub fn render_to_image(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32) -> Result<Vec<u8>> {
        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let depth = self.depth_for(width, height);
        self.encode_with_gamma(scene, camera, &view, &target, true, &depth, width, height, width as f32 / height as f32);
        let px = self.readback(&target, width, height);
        // the read-back above waited for the device, so the frame's lens blocks are mapped
        self.collect_lens();
        px
    }

    /// An offscreen texture's pixels as RGBA8 rows (diagnostics: `NFS_MIRROR_DUMP`).
    pub fn read_texture(&self, id: TextureId) -> Result<(u32, u32, Vec<u8>)> {
        let (w, h) = *self.offscreen.get(&id.0).ok_or_else(|| anyhow::anyhow!("texture {} is not an offscreen target", id.0))?;
        let px = self.readback(&self.textures[id.0 as usize]._texture, w, h)?;
        Ok((w, h, px))
    }

    /// Copies a `COPY_SRC` texture to the host as tightly packed RGBA8.
    fn readback(&self, target: &wgpu::Texture, width: u32, height: u32) -> Result<Vec<u8>> {
        let bytes_per_row = (4 * width).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (bytes_per_row * height) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely())?;
        rx.recv()??;
        let data = slice.get_mapped_range().expect("mapped range");
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        let bgra = matches!(self.config.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        for row in 0..height {
            let r = &data[(row * bytes_per_row) as usize..(row * bytes_per_row + width * 4) as usize];
            if bgra {
                for px in r.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                out.extend_from_slice(r);
            }
        }
        drop(data);
        buf.unmap();
        Ok(out)
    }

    /// Draw one stage's items into `pass`, in order: consecutive items of one mesh under one state
    /// are one instanced draw. `st` is what the pass already has bound.
    fn draw_bucket(&self, pass: &mut wgpu::RenderPass<'_>, scene: &Scene, bucket: &[u32], st: &mut DrawState, white: TextureId) {
        // consecutive items of one mesh under one state are one instanced draw
        let mut run: Option<Run> = None;
        for &idx in bucket {
            let item = &scene.items[idx as usize];
            let mat = &self.materials[item.material.0 as usize];
            // mirrored world: the winding flips, so a material whose cull is the view's culls
            // the other one (0x468C45); any other draws both faces rather than the wrong one
            let cull = match (mat.cull_back, scene.mirror, mat.view_cull) {
                (false, _, _) | (true, true, false) => 1,
                (true, false, _) => 0,
                (true, true, true) => 2,
            };
            let sub = cull * 2 + (!mat.depth_test as usize);
            // the blend group without depth writes, and with them (`device_states`)
            let plain = if mat.alpha_blend && mat.blend_pair > 0 { 1 + mat.blend_pair as usize } else { mat.alpha_blend as usize };
            let group = if mat.alpha_blend && mat.device_states { 5 + mat.blend_pair as usize } else { plain };
            let sky = sky_tex_of(&self.materials, self.no_reflections, item).unwrap_or(white).0;
            // `LayerFalseSort1` / `2`: at `SPECIALFX` 3, always, and at 2 by the runtime
            // pick (`FakeSortMode`, `RE/notes/thrash.md` §6d.4), the same draw first with
            // depth writes off, then on (0x449ADB / 0x449B2A). Otherwise (below 2, or 2's
            // `Single` pick) the other arm never draws twice: one draw, `MAG`/`MINFILTER`
            // forced `POINT` for it (0x449ABE), depth writes untouched.
            let fakesort_single = match self.fakesort_mode {
                FakeSortMode::Single => true,
                FakeSortMode::Double => false,
                // `LayerFalseSort1`/`2`'s own fold, per drawn item (0x449AB2, `[0x61975C]
                // < 10000` unsigned)
                FakeSortMode::PerItem => !item.fakesort_near,
            };
            let material = item.material.0 * 2;
            if mat.false_sort && mat.alpha_blend && !fakesort_single {
                // two pipelines in turn on one item: never merged, and the run before it
                // is drawn first
                if let Some(r) = run.take() {
                    self.draw_run(pass, st, &r);
                }
                let mut r = Run { first: idx, count: 1, pipeline: plain * 6 + sub, material, sky, mesh: item.mesh.0 };
                self.draw_run(pass, st, &r);
                r.pipeline = group * 6 + sub;
                self.draw_run(pass, st, &r);
                continue;
            }
            let material = if mat.false_sort && mat.alpha_blend && mat.bind_group_point.is_some() { material | 1 } else { material };
            let next = Run { first: idx, count: 1, pipeline: group * 6 + sub, material, sky, mesh: item.mesh.0 };
            match &mut run {
                Some(r) if r.first + r.count == idx && r.pipeline == next.pipeline && r.material == next.material && r.sky == next.sky && r.mesh == next.mesh => r.count += 1,
                _ => {
                    if let Some(r) = run.replace(next) {
                        self.draw_run(pass, st, &r);
                    }
                }
            }
        }
        if let Some(r) = run.take() {
            self.draw_run(pass, st, &r);
        }
    }

    /// Set what a run needs that the pass does not already hold, then draw it: one instanced draw
    /// of `r.count` consecutive objects (`first_instance` = the first object's index).
    fn draw_run(&self, pass: &mut wgpu::RenderPass<'_>, st: &mut DrawState, r: &Run) {
        if st.pipeline != r.pipeline {
            pass.set_pipeline(&self.pipelines[r.pipeline]);
            st.pipeline = r.pipeline;
        }
        if st.material != r.material {
            let m = &self.materials[(r.material >> 1) as usize];
            let bg = if r.material & 1 == 1 { m.bind_group_point.as_ref().unwrap_or(&m.bind_group) } else { &m.bind_group };
            pass.set_bind_group(2, bg, &[]);
            st.material = r.material;
        }
        if st.sky != r.sky {
            pass.set_bind_group(3, self.sky_bgs[r.sky as usize].as_ref().expect("sky bind group made"), &[]);
            st.sky = r.sky;
        }
        let mesh = &self.meshes[r.mesh as usize];
        if st.mesh != r.mesh {
            pass.set_vertex_buffer(0, mesh.vertex_buf.slice(mesh.vertex_offset..mesh.vertex_offset + mesh.vertex_len.max(4)));
            pass.set_index_buffer(mesh.index_buf.slice(mesh.index_offset..mesh.index_offset + mesh.index_len.max(4)), wgpu::IndexFormat::Uint32);
            st.mesh = r.mesh;
        }
        pass.draw_indexed(0..mesh.index_count, 0, r.first..r.first + r.count);
    }

    /// Draw `scene` into `view` / `depth_view`: the 3-D items (sky, opaque, blended), then — when
    /// `with_sprites` — the 2D sprites. Every buffer write of the frame goes through the staging
    /// belt into this frame's encoder, so nothing allocates a staging buffer per call. Neither
    /// pass stores its depth: nothing reads it afterwards (the sprite pass clears it to 1.0).
    #[allow(clippy::too_many_arguments)]
    fn encode_frame(&mut self, scene: &Scene, clear_color: [f64; 4], with_sprites: bool, camera: &Camera, view: &wgpu::TextureView, target: &wgpu::Texture, can_copy: bool, depth_view: &wgpu::TextureView, aspect: f32, depth_far: f32) {
        let n = scene.items.len().min(MAX_OBJECTS as usize);
        let white = self.white;
        self.sky_bind_group(white);
        // one walk over the items: each goes into its stage's list, in submission order — no-depth
        // (sky) first, then opaque, then blended; an `early` blended item goes with the opaque
        // ones (`DrawItem::early`) — and the section maps its sky-map pass samples get their bind
        // groups
        let mut buckets = std::mem::take(&mut self.buckets);
        for b in &mut buckets {
            b.clear();
        }
        for (i, item) in scene.items.iter().take(n).enumerate() {
            let (stage, skip) = {
                let mat = &self.materials[item.material.0 as usize];
                (if item.overlay { 3 } else if !mat.depth_test { 0 } else if mat.alpha_blend && !item.early { 2 } else { 1 }, item.main_view_only && self.no_reflections)
            };
            if let Some(t) = sky_tex_of(&self.materials, self.no_reflections, item) {
                self.sky_bind_group(t);
            }
            if !skip {
                buckets[stage].push(i as u32);
            }
        }

        self.seal_arena();
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        self.flush_texture_writes(&mut encoder);
        self.flush_mesh_writes(&mut encoder);
        let vp = camera.view_projection_mirrored(aspect, scene.mirror);
        let globals = GlobalsUniform {
            view_proj: vp.to_cols_array_2d(),
            view: camera.view().to_cols_array_2d(),
            sun_dir: scene.sun_direction.extend(0.0).to_array(),
            // .y the view's near plane: a depth-bias pull past it stops there (the original's
            // screen z goes to 0 or below — in front of everything, `RE/notes/thrash.md` §6)
            // .z 1 in the rear-view mirror's view: no specular or sky-map pass (`[0x6115CC]+0x190`)
            ambient: [scene.ambient, camera.near, self.no_reflections as u32 as f32, 0.0],
            cam_pos: camera.position.extend(1.0).to_array(),
            headlight: scene.headlight.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
            headlight_on: [scene.headlight.is_some() as u32 as f32, (scene.headlight.is_some() && scene.headlight_decal) as u32 as f32, 0.0, 0.0],
            beacon: scene.beacon.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
            beacon_on: [scene.beacon.is_some() as u32 as f32, 0.0, 0.0, 0.0],
            fog: scene.fog.map_or([0.0; 4], |f| [f.colour[0], f.colour[1], f.colour[2], f.density]),
            sky_fog: scene.sky_fog.map_or([0.0; 4], |f| [f.colour[0], f.colour[1], f.colour[2], f.density]),
        };
        let size = wgpu::BufferSize::new(std::mem::size_of::<GlobalsUniform>() as u64).expect("globals size");
        self.belt.write_buffer(&mut encoder, &self.globals_buf, 0, size).copy_from_slice(bytemuck::bytes_of(&globals));
        if n > 0 {
            let size = wgpu::BufferSize::new((n * std::mem::size_of::<ObjectUniform>()) as u64).expect("objects size");
            // built on the CPU and copied in once: wgpu 30's mapped views are write-only
            // (`WriteOnly`), not a `&mut [u8]` to cast in place
            let mut objects: Vec<ObjectUniform> = vec![bytemuck::Zeroable::zeroed(); n];
            for (o, item) in objects.iter_mut().zip(scene.items.iter().take(n)) {
                let mat = self.materials.get(item.material.0 as usize);
                let env_mix = mat.map_or(0.0, |m| m.env_mix);
                let lod_bias = mat.map_or(0.0, |m| m.lod_bias);
                // the alpha test: the material's race states, or the draw's own Thrash 0x24 ← 10
                let alpha_test = mat.is_some_and(|m| m.device_states) || item.alpha_test;
                // `.w` below: 0 no test, else the reference + ½ — a texel alpha under it (≤ the
                // reference in bytes) fails `GREATER`
                let alpha_ref = if alpha_test { item.alpha_ref as f32 + 0.5 } else { 0.0 };
                // state 0x18 as an offset of the screen z: `b · 2^-15` (0x540500's `× [0x5B3E10]`)
                // added to the game's `sz = far/(far − near) · (1 − near / Z)` (0x540CE0) — `b` the
                // material's block plus the draw's own (`DrawItem::depth_bias`) — with near / far the
                // pass's own planes, which 0x466F30 hands the projection 0x5391B0 (`cam+0xCC` /
                // `+0xD0`, folded into `cam+0xDC` / `+0xE0`): the main view's near and the camera
                // object's far 700 (0x4677FC), the rear view's 1 / 65000 (0x4691D0;
                // `RE/notes/thrash.md` §6d.3). The shader adds it to the port's own clip z, so it is
                // scaled by the ratio of the two `far / (far − near)` — exactly 1 where the port's
                // planes are the pass's (the race's main view and its mirror), and keeping the same
                // view-depth step `≈ b · 2^-15 · Z² / (near · A)` where they are not.
                let a_game = depth_far / (depth_far - camera.near);
                let a_port = camera.far / (camera.far - camera.near);
                let pull = mat.map_or(0.0, |m| depth_bias_offset(m.depth_bias.saturating_add(item.depth_bias)) * (a_port / a_game));
                *o = ObjectUniform {
                    model: item.transform.to_cols_array_2d(),
                    // .x is the environment-map mix, .zw a uv offset (the sky layers scroll with it)
                    // .y flags an item the headlight projector lights
                    env_mix: [env_mix, item.headlit as u32 as f32, item.uv_offset[0], item.uv_offset[1]],
                    light: [item.light[0], item.light[1], item.light[2], item.alpha],
                    lit_a: item.layer_light.map_or([0.0; 4], |l| [l.ambient[0], l.ambient[1], l.ambient[2], 1.0]),
                    lit_d: item.layer_light.map_or([0.0; 4], |l| [l.diffuse[0], l.diffuse[1], l.diffuse[2], 0.0]),
                    // .w the depth bias's pull, in view depth
                    lit_dir: item.layer_light.map_or([0.0, 0.0, 0.0, pull], |l| [l.direction[0], l.direction[1], l.direction[2], pull]),
                    shadow_u: item.shadow.map_or([0.0; 4], |s| s.0),
                    shadow_v: item.shadow.map_or([0.0; 4], |s| s.1),
                    // .y flags the item `Headlights::LayerUVLight` lays the `HEAD` decal over
                    // .w the alpha test (a material with `device_states`, or the draw's own): its
                    // reference + ½ in bytes, 0 for none
                    shadow_on: [item.shadow.is_some() as u32 as f32, item.uv_lit as u32 as f32, lod_bias, alpha_ref],
                    sky_u: item.sky.map_or([0.0; 4], |s| s.u),
                    sky_v: item.sky.map_or([0.0; 4], |s| s.v),
                    // .y the item is drawn in the sky pass (its fog is the sky's)
                    sky_on: [sky_tex_of(&self.materials, self.no_reflections, item).is_some() as u32 as f32, item.sky_pass as u32 as f32, item.fog_w, 0.0],
                };
            }
            self.belt.write_buffer(&mut encoder, &self.object_buf, 0, size).copy_from_slice(bytemuck::cast_slice(&objects));
        }

        // the 2D pass's vertices go into the belt before any pass opens (a copy cannot be
        // recorded inside one)
        let n_sprites = if with_sprites { scene.sprites.len().min(MAX_SPRITES as usize) } else { 0 };
        if with_sprites && scene.sprites.len() > MAX_SPRITES as usize {
            tracing::warn!(have = scene.sprites.len(), cap = MAX_SPRITES, "sprite budget exceeded — the rest of the frame is dropped");
            tracing::warn!(wanted = scene.sprites.len(), drawn = n_sprites, "sprite buffer full");
        }
        if n_sprites > 0 {
            let size = wgpu::BufferSize::new((n_sprites * 6 * std::mem::size_of::<SpriteVertex>()) as u64).expect("sprites size");
            let mut verts: Vec<SpriteVertex> = vec![bytemuck::Zeroable::zeroed(); n_sprites * 6];
            for (i, sp) in scene.sprites.iter().take(n_sprites).enumerate() {
                verts[i * 6..i * 6 + 6].copy_from_slice(&sprite_quad(sp, aspect));
                if self.sprite_log {
                    let v = &verts[i * 6];
                    let w = &verts[i * 6 + 1];
                    tracing::info!(i, tex = sp.texture.0, bound = self.textures[sp.texture.0 as usize].sprite_bg.is_some(), x0 = v.pos[0], y0 = v.pos[1], x1 = w.pos[0], uv0 = ?v.uv, color = ?v.color, "sprite");
                }
            }
            self.belt.write_buffer(&mut encoder, &self.sprite_buf, 0, size).copy_from_slice(bytemuck::cast_slice(&verts));
        }

        let c = clear_color;
        let probe = scene.lens_probe.filter(|_| can_copy);
        let overlay = !buckets[3].is_empty();
        // The lens flare's reference (0x460290 at 0x4603FF): the block as the sky pass leaves it with
        // the sun drawn — the stage with no depth test (the sky layers and the sun) alone, drawn
        // ahead of the frame proper into the same target, which the main pass then clears.
        if let Some(p) = probe.filter(|p| p.reference) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lens reference pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: c[0], g: c[1], b: c[2], a: c[3] }), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(1, &self.object_bg, &[]);
            let mut st = DrawState::NONE;
            self.draw_bucket(&mut pass, scene, &buckets[0], &mut st, white);
            drop(pass);
            self.lens_copy(&mut encoder, target, p, true);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: c[0], g: c[1], b: c[2], a: c[3] }), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: if overlay { wgpu::StoreOp::Store } else { wgpu::StoreOp::Discard } }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(1, &self.object_bg, &[]);
            let mut st = DrawState::NONE;
            for bucket in &buckets[..3] {
                self.draw_bucket(&mut pass, scene, bucket, &mut st, white);
            }
        }
        // The lens flare's test (0x460525): the block of the finished 3-D pass, before the flare's
        // own elements are laid over it
        if let Some(p) = probe.filter(|p| p.test) {
            self.lens_copy(&mut encoder, target, p, false);
        }
        // the lens flare's elements (0x460460 → 0x451710): flat on the screen over the finished
        // frame, under no depth test
        if overlay {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }, depth_slice: None })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            pass.set_bind_group(1, &self.object_bg, &[]);
            let mut st = DrawState::NONE;
            self.draw_bucket(&mut pass, scene, &buckets[3], &mut st, white);
        }
        // The 2D pass, over the 3-D one. Its depth starts at 1.0: the front end clears the
        // depth window before its 2D draws (0x4DA890 at 0x4D30AC / 0x4D3182: `THRASH_window(3)`
        // then `THRASH_clearwindow`, which on `dx7z.dll` clears the depth buffer alone —
        // 0x60001230 → 0x60008380, `D3DCLEAR_ZBUFFER` without the target when the depth window
        // is current — to the view's `+0x34`, 1.0 (0x537741, Thrash 0x43, 0x60004C90)). Only a
        // sprite with a depth state (`Sprite::depth`) tests or writes it; every other draws
        // as before. With no sprite the pass would draw nothing, so it is not opened.
        if n_sprites > 0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sprite pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            multiview_mask: None,
            });
            // HUD sprites; consecutive ones on one texture and one pipeline are one draw (their
            // vertices are contiguous in the buffer, in order)
            pass.set_vertex_buffer(0, self.sprite_buf.slice(..));
            let mut bound_pipeline: Option<Option<SpriteCompare>> = None;
            let mut bound_tex = u32::MAX;
            let mut run: Option<(usize, usize, u32, Option<SpriteCompare>)> = None;
            let mut flush = |pass: &mut wgpu::RenderPass<'_>, r: (usize, usize, u32, Option<SpriteCompare>)| {
                let (first, end, tex, pipeline) = r;
                let Some(bg) = self.textures[tex as usize].sprite_bg.as_ref() else { return };
                if bound_pipeline != Some(pipeline) {
                    pass.set_pipeline(match pipeline {
                        None => &self.sprite_pipeline,
                        Some(SpriteCompare::LessEqual) => &self.sprite_depth_pipelines[0],
                        Some(SpriteCompare::GreaterEqual) => &self.sprite_depth_pipelines[1],
                    });
                    bound_pipeline = Some(pipeline);
                }
                if bound_tex != tex {
                    pass.set_bind_group(0, bg, &[]);
                    bound_tex = tex;
                }
                pass.draw((first * 6) as u32..(end * 6) as u32, 0..1);
            };
            for (i, sp) in scene.sprites.iter().take(n_sprites).enumerate() {
                if self.textures[sp.texture.0 as usize].sprite_bg.is_none() {
                    continue;
                }
                let pipeline = sp.depth.map(|d| d.compare);
                match &mut run {
                    Some((_, end, tex, pl)) if *end == i && *tex == sp.texture.0 && *pl == pipeline => *end += 1,
                    _ => {
                        if let Some(r) = run.replace((i, i + 1, sp.texture.0, pipeline)) {
                            flush(&mut pass, r);
                        }
                    }
                }
            }
            if let Some(r) = run.take() {
                flush(&mut pass, r);
            }
        }
        self.buckets = buckets;
        self.belt.finish();
        self.queue.submit(Some(encoder.finish()));
        self.belt.recall();
        self.lens_map_staged();
    }
}

/// Build the two triangles of a sprite in clip space (y up), rotating about its pivot in screen space.
fn sprite_quad(sp: &Sprite, aspect: f32) -> [SpriteVertex; 6] {
    let (z, alpha_ref) = sp.depth.map_or((0.0, 0.0), |d| (d.z, d.alpha_test.map_or(-1.0, f32::from)));
    if let Some((t, col)) = sp.tri {
        let v = |p: [f32; 2], c: [f32; 4]| SpriteVertex { pos: [p[0] * 2.0 - 1.0, 1.0 - p[1] * 2.0, z], uv: sp.uv_min, color: c, alpha_ref };
        let (a, b, c) = (v(t[0], col[0]), v(t[1], col[1]), v(t[2], col[2]));
        return [a, b, c, c, c, c];
    }
    let (sx, sy) = (sp.size[0] / aspect, sp.size[1]);
    let (px, py) = (sp.pos[0] + sp.pivot[0] * sx, sp.pos[1] + sp.pivot[1] * sy);
    let (sn, cs) = (sp.rotation * std::f32::consts::TAU).sin_cos();
    let corner = |u: f32, v: f32| -> SpriteVertex {
        // local offset from pivot in screen units (aspect-corrected so rotation is not sheared)
        let lx = (u - sp.pivot[0]) * sx * aspect;
        let ly = (v - sp.pivot[1]) * sy;
        let rx = lx * cs - ly * sn;
        let ry = lx * sn + ly * cs;
        let x = px + rx / aspect;
        let y = py + ry;
        SpriteVertex {
            pos: [x * 2.0 - 1.0, 1.0 - y * 2.0, z],
            uv: [sp.uv_min[0] + (sp.uv_max[0] - sp.uv_min[0]) * u, sp.uv_min[1] + (sp.uv_max[1] - sp.uv_min[1]) * v],
            color: sp.corner_colors.map_or(sp.color, |k| match (u > 0.5, v > 0.5) {
                (false, false) => k[0],
                (true, false) => k[1],
                (true, true) => k[2],
                (false, true) => k[3],
            }),
            alpha_ref,
        }
    };
    let (a, b, c, d) = (corner(0.0, 0.0), corner(1.0, 0.0), corner(1.0, 1.0), corner(0.0, 1.0));
    [a, b, c, a, c, d]
}

/// The display ramp of state 0x2E as a last full-screen pass: `ramp[i] = min(0xFFFF, trunc(i ·
/// 257 · g))` — `dx7z.dll` scales the ramp it read at the mode set, which is the identity `i · 257`
/// on a primary surface nobody has touched — shown as `ramp / 0xFFFF` of full scale. The original
/// needs `DDCAPS2_PRIMARYGAMMA` (`[0x60033FA0]`, caps bit 0x20000, 0x60009115…0x60009123) and
/// otherwise ignores the state; the port, windowed, always applies it.
struct GammaPass {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// the frame's own image, kept while the size holds
    target: Option<(u32, u32, wgpu::Texture, wgpu::TextureView, wgpu::BindGroup)>,
}

const GAMMA_WGSL: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<uniform> g: vec4<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(pos.xy), 0);
    let i = round(c.rgb * 255.0);
    let w = min(floor(i * 257.0 * g.x), vec3<f32>(65535.0));
    return vec4<f32>(w / 65535.0, c.a);
}
"#;

impl GammaPass {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gamma"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gamma"), source: wgpu::ShaderSource::Wgsl(GAMMA_WGSL.into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("gamma"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gamma"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("gamma"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { pipeline, layout, uniform, target: None }
    }

    /// The image the frame is drawn into before the ramp, `width × height`.
    fn source(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        if !self.target.as_ref().is_some_and(|(w, h, ..)| *w == width && *h == height) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gamma source"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gamma"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: self.uniform.as_entire_binding() }],
            });
            self.target = Some((width, height, texture, view, bg));
        }
        self.target.as_ref().map(|(_, _, t, v, _)| (t.clone(), v.clone())).expect("gamma source")
    }

    fn resolve(&self, device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::TextureView, g: f32) {
        let Some((_, _, _, _, bg)) = self.target.as_ref() else { return };
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&[g, 0.0f32, 0.0, 0.0]));
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gamma"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(enc.finish()));
    }
}
