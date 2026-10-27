//! Small wgpu helpers the full-screen passes share: the descriptors every pass
//! used to spell out field by field. Each helper fills the fields with the values the passes had —
//! no vertex buffers, a triangle list, no depth, one view, no pipeline cache, write mask ALL,
//! `StoreOp::Store`, no occlusion query — so a pass built through them gets the same objects.

/// A colour target with every channel written.
pub(crate) fn target(format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>) -> Option<wgpu::ColorTargetState> {
    Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })
}

/// A full-screen pipeline: the vertex shader draws its own triangle (no buffers), the default primitive
/// state, no depth / stencil, `samples` per pixel, no multiview, no cache. `constants` go to the
/// fragment stage only (the vertex stage keeps the default options).
pub(crate) struct Fullscreen<'a> {
    pub label: &'a str,
    pub layout: &'a wgpu::PipelineLayout,
    pub module: &'a wgpu::ShaderModule,
    pub vs: &'a str,
    pub fs: &'a str,
    pub constants: &'a [(&'a str, f64)],
    pub samples: u32,
}

impl<'a> Fullscreen<'a> {
    /// Entry points `vs` / `fs`, no constants, one sample.
    pub fn new(label: &'a str, layout: &'a wgpu::PipelineLayout, module: &'a wgpu::ShaderModule) -> Self {
        Self { label, layout, module, vs: "vs", fs: "fs", constants: &[], samples: 1 }
    }

    pub fn fs(self, fs: &'a str) -> Self {
        Self { fs, ..self }
    }

    pub fn constants(self, constants: &'a [(&'a str, f64)]) -> Self {
        Self { constants, ..self }
    }

    pub fn samples(self, samples: u32) -> Self {
        Self { samples, ..self }
    }

    pub fn build(self, device: &wgpu::Device, targets: &[Option<wgpu::ColorTargetState>]) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(self.label),
            layout: Some(self.layout),
            vertex: wgpu::VertexState { module: self.module, entry_point: Some(self.vs), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: self.module,
                entry_point: Some(self.fs),
                compilation_options: wgpu::PipelineCompilationOptions { constants: self.constants, ..Default::default() },
                targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: self.samples, mask: !0, alpha_to_coverage_enabled: false },
            multiview_mask: None,
            cache: None,
        })
    }
}

/// A WGSL module.
pub(crate) fn wgsl_module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) })
}

/// A pipeline layout of these bind group layouts, no immediates.
pub(crate) fn pipeline_layout(device: &wgpu::Device, label: &str, layouts: &[&wgpu::BindGroupLayout]) -> wgpu::PipelineLayout {
    let groups: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(|l| Some(*l)).collect();
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &groups, immediate_size: 0 })
}

/// `LoadOp::Clear(BLACK)` or `LoadOp::Load`.
pub(crate) fn load_or_clear(load: bool) -> wgpu::LoadOp<wgpu::Color> {
    if load { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) }
}

/// A render pass with one colour attachment (`view`, no resolve, stored), no depth, timed as `prof`.
pub(crate) fn color_pass<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, prof: &'static str, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'e> {
    pass_with(encoder, label, crate::prof::rp(prof), view, load)
}

/// [`color_pass`] that the profiler does not time.
pub(crate) fn color_pass_untimed<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'e> {
    pass_with(encoder, label, None, view, load)
}

/// [`color_pass`] with a resolve target and a store op of its own (a pass over a multisampled colour).
pub(crate) fn color_pass_resolve<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, prof: &'static str, view: &wgpu::TextureView, resolve_target: Option<&wgpu::TextureView>, load: wgpu::LoadOp<wgpu::Color>, store: wgpu::StoreOp) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target, ops: wgpu::Operations { load, store } })],
        depth_stencil_attachment: None,
        timestamp_writes: crate::prof::rp(prof),
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

fn pass_with<'e>(encoder: &'e mut wgpu::CommandEncoder, label: &str, timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
        depth_stencil_attachment: None,
        timestamp_writes,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A bind group of `layout`'s binding 0 = `view`, binding 1 = `sampler` (the sprite layout's shape).
pub(crate) fn tex_sampler_bg(device: &wgpu::Device, label: &str, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView, sampler: &wgpu::Sampler) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) }],
    })
}

/// A bind group of one texture view at binding 0.
pub(crate) fn view_bg(device: &wgpu::Device, label: &str, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }] })
}

/// A bind group of the whole of `buf` at binding 0.
pub(crate) fn buffer_bg(device: &wgpu::Device, label: &str, layout: &wgpu::BindGroupLayout, buf: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some(label), layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }] })
}

/// A uniform buffer the size of `T`, written with `queue.write_buffer`.
pub(crate) fn uniform_buf<T>(device: &wgpu::Device, label: &str) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: std::mem::size_of::<T>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
}

/// A bind group layout of these entries.
pub(crate) fn bgl(device: &wgpu::Device, label: &str, entries: &[wgpu::BindGroupLayoutEntry]) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries })
}

/// A layout entry: a uniform buffer (no dynamic offset, no minimum size) seen by `visibility`.
pub(crate) fn bgl_uniform(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }
}

/// A layout entry: a 2-D texture of `sample_type` seen by the fragment stage.
pub(crate) fn bgl_tex(binding: u32, sample_type: wgpu::TextureSampleType, multisampled: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled }, count: None }
}

/// A layout entry: a sampler of `kind` seen by the fragment stage.
pub(crate) fn bgl_sampler(binding: u32, kind: wgpu::SamplerBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(kind), count: None }
}

/// A plain 2-D texture of one mip and one sample.
pub(crate) fn texture_2d(device: &wgpu::Device, label: &str, width: u32, height: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// A render target a pass writes and the next one samples: the texture, its view and its bind group in the
/// sprite layout (texture + sampler), rebuilt only when the size changes.
pub(crate) struct RtImage {
    pub tex: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bg: wgpu::BindGroup,
    pub size: (u32, u32),
}

impl RtImage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(device: &wgpu::Device, label: &str, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> Self {
        let tex = texture_2d(device, label, width, height, format, usage);
        let view = tex.create_view(&Default::default());
        let bg = tex_sampler_bg(device, label, layout, &view, sampler);
        Self { tex, view, bg, size: (width, height) }
    }

    /// `slot`'s image, (re)made when it is missing or of another size.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure<'s>(slot: &'s mut Option<RtImage>, device: &wgpu::Device, label: &str, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler, width: u32, height: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> &'s RtImage {
        if slot.as_ref().is_none_or(|i| i.size != (width, height)) {
            *slot = Some(Self::new(device, label, layout, sampler, width, height, format, usage));
        }
        slot.as_ref().expect("render target")
    }
}

/// The usage of an image a pass renders and a later pass samples.
pub(crate) const RT_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);

/// Where a pass over the finished (or still multisampled) float scene draws and which depth it reads (ENHANCEMENTS E13 /
/// E36 / E44): `depth` of `kind`; added over `colour` (loaded; `resolve` when it is multisampled, `samples` per pixel; `keep`
/// = store the multisampled samples for a later pass).
#[derive(Clone, Copy)]
pub(crate) struct OverScene<'a> {
    pub depth: &'a wgpu::TextureView,
    pub kind: crate::wgsl::DepthKind,
    pub colour: &'a wgpu::TextureView,
    pub resolve: Option<&'a wgpu::TextureView>,
    pub samples: u32,
    pub keep: bool,
}

/// The depth image a pass over the scene reads: the frame's, `multisampled` (ENHANCEMENT E1) or not.
#[derive(Clone, Copy)]
pub(crate) struct DepthSrc<'a> {
    pub view: &'a wgpu::TextureView,
    pub multisampled: bool,
}

/// What every pass of the post chain (ENHANCEMENTS E27 / the depth of field (photo mode, E31) / E40 / E10 / E21, ENH-TEMPORAL's debug pass) works
/// with: the device and queue, the sprite layout and sampler every image of the chain is bound
/// through, the frame's encoder, its depth, the size the chain works at, and the frame's camera (`aspect`, `mirror`: the
/// view's).
pub(crate) struct PostCtx<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub layout: &'a wgpu::BindGroupLayout,
    pub sampler: &'a wgpu::Sampler,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub depth: DepthSrc<'a>,
    /// the post chain's size (`width`, `height`)
    pub size: (u32, u32),
    pub camera: &'a crate::Camera,
    pub aspect: f32,
    pub mirror: bool,
}

/// A value made from a key and kept while the key holds: the passes' bind groups over the frame's depth
/// (and the other images they bind) are remade only when one of those images changes — a resize, another sample count —
/// not every frame. wgpu's views and buffers compare by identity, and the key holds them, so a dropped and remade image
/// can never pass for the old one.
pub(crate) struct Keyed<K, V>(Option<(K, V)>);

impl<K, V> Default for Keyed<K, V> {
    fn default() -> Self {
        Self(None)
    }
}

impl<K: PartialEq, V> Keyed<K, V> {
    /// The value for `key`: the kept one when the key is the same, else `make()`'s (kept from now on).
    pub fn get(&mut self, key: K, make: impl FnOnce() -> V) -> &V {
        if self.0.as_ref().is_none_or(|(k, _)| *k != key) {
            self.0 = Some((key, make()));
        }
        &self.0.as_ref().expect("keyed value").1
    }
}
