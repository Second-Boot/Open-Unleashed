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
    sun_dir: [f32; 4],
    ambient: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
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
    cull_back: bool,
    depth_test: bool,
}

pub struct Renderer {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth_view: wgpu::TextureView,
    pipelines: Vec<wgpu::RenderPipeline>, // index = blend*4 + nocull*2 + nodepth
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    object_buf: wgpu::Buffer,
    object_bg: wgpu::BindGroup,
    object_stride: u64,
    meshes: Vec<GpuMesh>,
    textures: Vec<GpuTexture>,
    materials: Vec<GpuMaterial>,
    white: TextureId,
    sprite_pipeline: wgpu::RenderPipeline,
    sprite_layout: wgpu::BindGroupLayout,
    sprite_buf: wgpu::Buffer,
}

const MAX_SPRITES: u64 = 512;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_OBJECTS: u64 = 8192;

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
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
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
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
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
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[&globals_layout, &object_layout, &material_layout],
            push_constant_ranges: &[],
        });
        let make_pipeline = |blend: bool, cull: bool, depth: bool| {
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
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(if blend { wgpu::BlendState::ALPHA_BLENDING } else { wgpu::BlendState::REPLACE }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: if cull { Some(wgpu::Face::Back) } else { None },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: !blend && depth,
                    depth_compare: if depth { wgpu::CompareFunction::Less } else { wgpu::CompareFunction::Always },
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let mut pipelines = Vec::with_capacity(8);
        for i in 0..8u8 {
            pipelines.push(make_pipeline(i & 4 != 0, i & 2 == 0, i & 1 == 0));
        }

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<GlobalsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
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
            object_layout,
            material_layout,
            sampler,
            object_buf,
            object_bg,
            object_stride,
            meshes: Vec::new(),
            textures: Vec::new(),
            materials: Vec::new(),
            white: TextureId(0),
            sprite_pipeline,
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

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    pub fn upload_mesh(&mut self, mesh: &MeshData) -> MeshId {
        let vertex_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh vb"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh ib"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.push(GpuMesh { vertex_buf, index_buf, index_count: mesh.indices.len() as u32 });
        MeshId(self.meshes.len() as u32 - 1)
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
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        }));
        self.textures.push(GpuTexture { _texture: texture, view, sprite_bg });
        TextureId(self.textures.len() as u32 - 1)
    }

    pub fn create_material(&mut self, desc: MaterialDesc) -> MaterialId {
        let tex = &self.textures[desc.texture.unwrap_or(self.white).0 as usize];
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.materials.push(GpuMaterial { bind_group, alpha_blend: desc.alpha_blend, cull_back: desc.cull_back, depth_test: desc.depth_test });
        MaterialId(self.materials.len() as u32 - 1)
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

        let vp = camera.view_projection(aspect);
        self.queue.write_buffer(
            &self.globals_buf,
            0,
            bytemuck::bytes_of(&GlobalsUniform {
                view_proj: vp.to_cols_array_2d(),
                sun_dir: scene.sun_direction.extend(0.0).to_array(),
                ambient: [scene.ambient, 0.0, 0.0, 0.0],
            }),
        );
        let n = scene.items.len().min(MAX_OBJECTS as usize);
        let mut staging = vec![0u8; self.object_stride as usize * n];
        for (i, item) in scene.items.iter().take(n).enumerate() {
            let o = ObjectUniform { model: item.transform.to_cols_array_2d() };
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
                    let pipeline = &self.pipelines[(mat.alpha_blend as usize) * 4 + (!mat.cull_back as usize) * 2 + (!mat.depth_test as usize)];
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(1, &self.object_bg, &[(i as u64 * self.object_stride) as u32]);
                    pass.set_bind_group(2, &mat.bind_group, &[]);
                    pass.set_vertex_buffer(0, mesh.vertex_buf.slice(..));
                    pass.set_index_buffer(mesh.index_buf.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
            // HUD sprites
            let n_sprites = scene.sprites.len().min(MAX_SPRITES as usize);
            if n_sprites > 0 {
                let mut verts: Vec<SpriteVertex> = Vec::with_capacity(n_sprites * 6);
                for sp in scene.sprites.iter().take(n_sprites) {
                    verts.extend_from_slice(&sprite_quad(sp, aspect));
                }
                self.queue.write_buffer(&self.sprite_buf, 0, bytemuck::cast_slice(&verts));
                pass.set_pipeline(&self.sprite_pipeline);
                pass.set_vertex_buffer(0, self.sprite_buf.slice(..));
                for (i, sp) in scene.sprites.iter().take(n_sprites).enumerate() {
                    if let Some(bg) = self.textures[sp.texture.0 as usize].sprite_bg.as_ref() {
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
