use crate::{Camera, MaterialDesc, MeshData, Scene, Sprite, TextureData, Vertex};
use anyhow::{Context, Result};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::window::Window;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(u32);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MaterialId(u32);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GlobalsUniform {
    view_proj: [[f32; 4]; 4],
    /// the camera's view matrix alone — `R3DSpecularTextureLayer` needs the normal in eye space
    view: [[f32; 4]; 4],
    sun_dir: [f32; 4],
    ambient: [f32; 4],
    cam_pos: [f32; 4],
    /// world → headlight frame; `headlight_on.x` 1 while it lights
    headlight: [[f32; 4]; 4],
    headlight_on: [f32; 4],
    /// world → the police beacon's frame, and `beacon_on.x` 1 while one spins
    beacon: [[f32; 4]; 4],
    beacon_on: [f32; 4],
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
}

struct GpuMesh {
    vertex_buf: wgpu::Buffer,
    index_buf: wgpu::Buffer,
    index_count: u32,
}

struct GpuTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    sprite_bg: Option<wgpu::BindGroup>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

struct GpuMaterial {
    bind_group: wgpu::BindGroup,
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
    env_variant: usize,
    /// the diffuse texture, kept so the material can be rebound to another environment map
    texture: TextureId,
    /// the environment map bound now
    env: TextureId,
}

pub struct Renderer {
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
    /// the scene samplers by address mode and mip filter — index `clamp_u | clamp_v << 1 |
    /// point_mip << 2`
    samplers: [wgpu::Sampler; 8],
    /// the mip filter's `POINT` → `LINEAR` promotion, `[0x69DD1D]` (`set_trilinear`); clear at start, as the image has it
    trilinear: bool,
    sprite_sampler: wgpu::Sampler,
    object_buf: wgpu::Buffer,
    object_bg: wgpu::BindGroup,
    object_stride: u64,
    meshes: Vec<GpuMesh>,
    textures: Vec<GpuTexture>,
    materials: Vec<GpuMaterial>,
    white: TextureId,
    sprite_pipeline: wgpu::RenderPipeline,
    /// offscreen render targets handed out by [`Renderer::render_offscreen`], by texture index
    offscreen: std::collections::HashMap<u32, (u32, u32)>,
    sprite_layout: wgpu::BindGroupLayout,
    sprite_buf: wgpu::Buffer,
}

/// The front-end screens draw a few thousand quads (every gouraud box is a grid of them).
const MAX_SPRITES: u64 = 8192;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_OBJECTS: u64 = 8192;
/// The main view's far plane — the camera object's `+8`, 700.0 (0x4677FC) — over which the screen
/// z the depth bias moves is linear (`RE/notes/thrash.md` §6).
const VIEW_FAR: f32 = 700.0;

impl Renderer {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL | wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).context("create surface")?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
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
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[&globals_layout, &object_layout, &material_layout],
            push_constant_ranges: &[],
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
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
                    }],
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
                    depth_write_enabled: depth && (!blend || zwrite),
                    // `ZFUNC` `LESSEQUAL`: `dx7z.dll` makes the device with it (0x6000A545) and the
                    // game sets it again through the view (Thrash 0x28 ← 3: the race pass
                    // 0x468B54, the front end's 3D pass 0x4D301D). The front-end rooms' layers —
                    // light pools and cones on the floor and walls — share their surface's depth
                    // and pass because of it, drawn after it in `Base+70` order
                    // (`RE/notes/frontend-3d.md` §9). *(The port kept `LESS` until 2026-09-25,
                    // for a fight that was the rooms' own states — file order, no pair — not the
                    // depth test.)*
                    depth_compare: if depth { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always },
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        // six a group — the cull (back, none, front) × depth (on, off), `2·cull + !depth` — and the
        // groups: 0 blend off, 1 the plain alpha pair, 2…4 the other pairs, 5…8 every pair again
        // with depth writes on (`device_states`)
        const CULLS: [Option<wgpu::Face>; 3] = [Some(wgpu::Face::Back), None, Some(wgpu::Face::Front)];
        let mut pipelines = Vec::with_capacity(54);
        for (blend, pair, zwrite) in [(false, 0, false), (true, 0, false), (true, 1, false), (true, 2, false), (true, 3, false), (true, 0, true), (true, 1, true), (true, 2, true), (true, 3, true)] {
            for i in 0..6 {
                pipelines.push(make_pipeline(blend, pair, CULLS[i / 2], i % 2 == 0, zwrite));
            }
        }

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
        // 2026-09-25 this sampler clamped as an APPROXIMATION and read the top level only.) The
        // `HEAD` decal samples its top level explicitly through the same sampler.
        let make_proj_sampler = |mip: wgpu::FilterMode| {
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
        let proj_samplers = [make_proj_sampler(wgpu::FilterMode::Linear), make_proj_sampler(wgpu::FilterMode::Nearest)];
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
        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let object_stride = (std::mem::size_of::<ObjectUniform>() as u64).div_ceil(align) * align;
        let object_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: object_stride * MAX_OBJECTS,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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
                    size: wgpu::BufferSize::new(std::mem::size_of::<ObjectUniform>() as u64),
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
        let make_sampler = |clamp_u: bool, clamp_v: bool, mip: wgpu::FilterMode| {
            let mode = |c: bool| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene"),
                address_mode_u: mode(clamp_u),
                address_mode_v: mode(clamp_v),
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: mip,
                ..Default::default()
            })
        };
        let samplers = std::array::from_fn(|i| {
            let mip = if i & 4 == 0 { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest };
            make_sampler(i & 1 != 0, i & 2 != 0, mip)
        });

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
            mipmap_filter: wgpu::FilterMode::Nearest,
            lod_max_clamp: 0.0,
            ..Default::default()
        });

        // 2D sprite pipeline (HUD)
        let sprite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("sprite"), source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()) });
        let sprite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let sprite_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sprite"), bind_group_layouts: &[&sprite_layout], push_constant_ranges: &[] });
        let sprite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&sprite_pl),
            vertex: wgpu::VertexState { module: &sprite_shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<SpriteVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4] }] },
            fragment: Some(wgpu::FragmentState { module: &sprite_shader, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: false, depth_compare: wgpu::CompareFunction::Always, stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
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
            samplers,
            trilinear: false,
            sprite_sampler,
            object_buf,
            object_bg,
            object_stride,
            meshes: Vec::new(),
            textures: Vec::new(),
            materials: Vec::new(),
            white: TextureId(0),
            sprite_pipeline,
            offscreen: std::collections::HashMap::new(),
            sprite_layout,
            sprite_buf,
        };
        let _ = &r.object_layout;
        r.white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Ok(r)
    }

    fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
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
        self.meshes.push(GpuMesh { vertex_buf, index_buf, index_count: mesh.indices.len() as u32 });
        MeshId(self.meshes.len() as u32 - 1)
    }

    /// Rewrite a dynamic mesh's vertices. The slice must be no longer than the one it was uploaded
    /// with; anything else is ignored.
    pub fn write_mesh_vertices(&mut self, id: MeshId, vertices: &[Vertex]) {
        let Some(m) = self.meshes.get(id.0 as usize) else { return };
        let bytes: &[u8] = bytemuck::cast_slice(vertices);
        if bytes.len() as u64 > m.vertex_buf.size() {
            return;
        }
        self.queue.write_buffer(&m.vertex_buf, 0, bytes);
    }

    pub fn upload_mesh(&mut self, mesh: &MeshData) -> MeshId {
        let vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vb"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let index_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh ib"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.push(GpuMesh { vertex_buf, index_buf, index_count: mesh.indices.len() as u32 });
        MeshId(self.meshes.len() as u32 - 1)
    }

    /// New vertices for a mesh already uploaded, the same count — the morphing parts of a car
    /// (its roof) rewrite theirs when the animation moves.
    pub fn update_mesh_vertices(&mut self, id: MeshId, vertices: &[crate::Vertex]) {
        if let Some(m) = self.meshes.get(id.0 as usize)
            && (vertices.len() * std::mem::size_of::<crate::Vertex>()) as u64 == m.vertex_buf.size()
        {
            self.queue.write_buffer(&m.vertex_buf, 0, bytemuck::cast_slice(vertices));
        }
    }

    pub fn upload_texture(&mut self, tex: &TextureData) -> TextureId {
        let size = wgpu::Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 };
        let mip_level_count = tex.mips.len().max(1) as u32;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("texture"),
            size,
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (mut w, mut h) = (tex.width, tex.height);
        for (level, data) in tex.mips.iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
        let view = texture.create_view(&Default::default());
        let sprite_bg = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite tex"),
            layout: &self.sprite_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sprite_sampler) }],
        }));
        tracing::info!(id = self.textures.len(), w = tex.width, h = tex.height, "texture uploaded");
        self.textures.push(GpuTexture { _texture: texture, view, sprite_bg });
        TextureId(self.textures.len() as u32 - 1)
    }

    /// Overwrite an existing texture's pixels, keeping its id and every bind group that points at
    /// it. The new data must have the same dimensions and mip count as the original. Used for the
    /// car's texture pages, which are recomposed when the damage decals change
    /// (`RE/notes/car-model.md` §11).
    pub fn update_texture(&mut self, id: TextureId, tex: &TextureData) {
        let Some(gpu) = self.textures.get(id.0 as usize) else { return };
        let (mut w, mut h) = (tex.width, tex.height);
        for (level, data) in tex.mips.iter().enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &gpu._texture, mip_level: level as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            w = (w / 2).max(1);
            h = (h / 2).max(1);
        }
    }

    pub fn create_material(&mut self, desc: MaterialDesc) -> MaterialId {
        let texture = desc.texture.unwrap_or(self.white);
        let env = desc.env_texture.unwrap_or(self.white);
        let sampler = desc.clamp[0] as usize | (desc.clamp[1] as usize) << 1;
        let bind_group = self.material_bind_group(texture, sampler, env);
        self.materials.push(GpuMaterial {
            bind_group,
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

    /// The three environment textures the frame binds — `EnvSet::texture` 0x441180 with the layers
    /// 0, 1 and 2 (0x43A73D…0x43A778) — each material taking the one its reflection strength
    /// buckets to (`RE/notes/lighting.md` §3.1). A `None` leaves that variant on the one before it.
    pub fn set_env_textures(&mut self, env: [Option<TextureId>; 3]) {
        let pick = |v: usize| -> Option<TextureId> { (0..=v).rev().find_map(|i| env[i]) };
        let views: Vec<Option<TextureId>> = (0..3).map(pick).collect();
        for i in 0..self.materials.len() {
            if self.materials[i].env_mix <= 0.0 {
                continue;
            }
            let Some(e) = views[self.materials[i].env_variant] else { continue };
            self.materials[i].env = e;
            self.materials[i].bind_group = self.material_bind_group(self.materials[i].texture, self.materials[i].sampler, e);
        }
    }

    pub fn set_env_texture(&mut self, env: TextureId) {
        for i in 0..self.materials.len() {
            if self.materials[i].env_mix > 0.0 {
                self.materials[i].env = env;
                self.materials[i].bind_group = self.material_bind_group(self.materials[i].texture, self.materials[i].sampler, env);
            }
        }
    }

    /// A material's bind group: its texture, the sampler for its address modes under the current
    /// mip filter, and the environment map.
    fn material_bind_group(&self, texture: TextureId, sampler: usize, env: TextureId) -> wgpu::BindGroup {
        let sampler = &self.samplers[sampler | (!self.trilinear as usize) << 2];
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.textures[texture.0 as usize].view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.textures[env.0 as usize].view) },
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

    pub fn render(&mut self, scene: &Scene, camera: &Camera) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        };
        let view = frame.texture.create_view(&Default::default());
        let aspect = self.aspect();
        self.encode_frame(scene, camera, &view, &self.depth_view.clone(), aspect);
        frame.present();
        Ok(())
    }

    /// Render one frame into an offscreen RGBA8 image (for screenshots and automated checks).
    /// Render a scene's meshes into an offscreen texture that the sprite pass can then draw, so a
    /// 3D view can sit **inside** the 2D screen in the right drawing order — what the front end's
    /// `NFS5.3DCAR` widgets need (`RE/notes/frontend-3d.md` §4). The clear is transparent, so only
    /// the geometry shows, unless `clear` gives an opaque colour (the in-race rear-view mirror).
    /// Passing the id of a texture this returned earlier reuses it.
    pub fn render_offscreen(&mut self, scene: &Scene, camera: &Camera, width: u32, height: u32, reuse: Option<TextureId>, clear_colour: Option<[f64; 4]>) -> TextureId {
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
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
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
        let mut cfg = self.config.clone();
        cfg.width = width;
        cfg.height = height;
        let depth = Self::make_depth(&self.device, &cfg);
        let view = self.textures[id.0 as usize].view.clone();
        let mut clear = scene.clone();
        clear.clear_color = clear_colour.unwrap_or([0.0, 0.0, 0.0, 0.0]);
        clear.sprites.clear();
        self.encode_frame(&clear, camera, &view, &depth, width as f32 / height as f32);
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
        let mut cfg = self.config.clone();
        cfg.width = width;
        cfg.height = height;
        let depth = Self::make_depth(&self.device, &cfg);
        self.encode_frame(scene, camera, &view, &depth, width as f32 / height as f32);
        let bytes_per_row = (4 * width).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: (bytes_per_row * height) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
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
        let data = slice.get_mapped_range();
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

    fn encode_frame(&mut self, scene: &Scene, camera: &Camera, view: &wgpu::TextureView, depth_view: &wgpu::TextureView, aspect: f32) {

        let vp = camera.view_projection_mirrored(aspect, scene.mirror);
        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&GlobalsUniform {
                view_proj: vp.to_cols_array_2d(),
                view: camera.view().to_cols_array_2d(),
                sun_dir: scene.sun_direction.extend(0.0).to_array(),
                ambient: [scene.ambient, 0.0, 0.0, 0.0],
                cam_pos: camera.position.extend(1.0).to_array(),
                headlight: scene.headlight.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
                headlight_on: [scene.headlight.is_some() as u32 as f32, 0.0, 0.0, 0.0],
                beacon: scene.beacon.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
                beacon_on: [scene.beacon.is_some() as u32 as f32, 0.0, 0.0, 0.0],
            }),
        );
        let n = scene.items.len().min(MAX_OBJECTS as usize);
        let mut staging = vec![0u8; self.object_stride as usize * n];
        for (i, item) in scene.items.iter().take(n).enumerate() {
            let env_mix = self.materials.get(item.material.0 as usize).map(|m| m.env_mix).unwrap_or(0.0);
            let lod_bias = self.materials.get(item.material.0 as usize).map(|m| m.lod_bias).unwrap_or(0.0);
            let alpha_test = self.materials.get(item.material.0 as usize).is_some_and(|m| m.device_states);
            // state 0x18 as a pull toward the eye in view depth: `−b · (far − near) / 32768` over
            // the main view's planes — its near, and the camera object's far 700 (0x4677FC), which
            // no later write changes (`RE/notes/thrash.md` §6). APPROXIMATION in the rear view, whose
            // planes are 1 / 65000 in the original (AUDIT.md §O) and whose far the port shortens
            let pull = self.materials.get(item.material.0 as usize).map_or(0.0, |m| -(m.depth_bias as f32) * (VIEW_FAR - camera.near) / 32768.0);
            let o = ObjectUniform {
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
                // .w the alpha test of a material with `device_states`
                shadow_on: [item.shadow.is_some() as u32 as f32, item.uv_lit as u32 as f32, lod_bias, alpha_test as u32 as f32],
            };
            let off = i * self.object_stride as usize;
            staging[off..off + std::mem::size_of::<ObjectUniform>()].copy_from_slice(bytemuck::bytes_of(&o));
        }
        if n > 0 {
            self.queue.write_buffer(&self.object_buf, 0, &staging);
        }

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        {
            let c = scene.clear_color;
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
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.globals_bg, &[]);
            // no-depth (sky) first, then opaque, then blended
            for stage in 0..3u8 {
                for (i, item) in scene.items.iter().take(n).enumerate() {
                    let mat = &self.materials[item.material.0 as usize];
                    let item_stage = if !mat.depth_test { 0 } else if mat.alpha_blend { 2 } else { 1 };
                    if item_stage != stage {
                        continue;
                    }
                    let mesh = &self.meshes[item.mesh.0 as usize];
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
                    pass.set_bind_group(1, &self.object_bg, &[(i as u64 * self.object_stride) as u32]);
                    pass.set_bind_group(2, &mat.bind_group, &[]);
                    pass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
                    pass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    // `LayerFalseSort1` / `2`: the same draw first with depth writes off, then on
                    // (0x449ADB / 0x449B2A, `RE/notes/thrash.md` §6d.4)
                    if mat.false_sort && mat.alpha_blend {
                        pass.set_pipeline(&self.pipelines[plain * 6 + sub]);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                    }
                    pass.set_pipeline(&self.pipelines[group * 6 + sub]);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
            // HUD sprites
            let n_sprites = scene.sprites.len().min(MAX_SPRITES as usize);
            if scene.sprites.len() > MAX_SPRITES as usize {
                tracing::warn!(have = scene.sprites.len(), cap = MAX_SPRITES, "sprite budget exceeded — the rest of the frame is dropped");
            }
            if scene.sprites.len() > n_sprites {
                tracing::warn!(wanted = scene.sprites.len(), drawn = n_sprites, "sprite buffer full");
            }
            if n_sprites > 0 {
                let mut verts: Vec<SpriteVertex> = Vec::with_capacity(n_sprites * 6);
                for sp in scene.sprites.iter().take(n_sprites) {
                    verts.extend_from_slice(&sprite_quad(sp, aspect));
                }
                self.queue.write_buffer(&self.sprite_buf, 0, bytemuck::cast_slice(&verts));
                pass.set_pipeline(&self.sprite_pipeline);
                pass.set_vertex_buffer(0, self.sprite_buf.slice(..));
                let log = std::env::var("NFS_SPRITE_LOG").is_ok();
                for (i, sp) in scene.sprites.iter().take(n_sprites).enumerate() {
                    let bg = self.textures[sp.texture.0 as usize].sprite_bg.as_ref();
                    if log {
                        let v = &verts[i * 6];
                        let w = &verts[i * 6 + 1];
                        tracing::info!(i, tex = sp.texture.0, bound = bg.is_some(), x0 = v.pos[0], y0 = v.pos[1], x1 = w.pos[0], uv0 = ?v.uv, color = ?v.color, "sprite");
                    }
                    if let Some(bg) = bg {
                        pass.set_bind_group(0, bg, &[]);
                        pass.draw((i * 6) as u32..(i * 6 + 6) as u32, 0..1);
                    }
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
    }
}

/// Build the two triangles of a sprite in clip space (y up), rotating about its pivot in screen space.
fn sprite_quad(sp: &Sprite, aspect: f32) -> [SpriteVertex; 6] {
    if let Some((t, col)) = sp.tri {
        let v = |p: [f32; 2], c: [f32; 4]| SpriteVertex { pos: [p[0] * 2.0 - 1.0, 1.0 - p[1] * 2.0], uv: sp.uv_min, color: c };
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
            pos: [x * 2.0 - 1.0, 1.0 - y * 2.0],
            uv: [sp.uv_min[0] + (sp.uv_max[0] - sp.uv_min[0]) * u, sp.uv_min[1] + (sp.uv_max[1] - sp.uv_min[1]) * v],
            color: sp.color,
        }
    };
    let (a, b, c, d) = (corner(0.0, 0.0), corner(1.0, 0.0), corner(1.0, 1.0), corner(0.0, 1.0));
    [a, b, c, a, c, d]
}
