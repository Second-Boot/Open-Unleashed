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
///
/// PERF (picture identical): naga's forced loop bounding is off (`force_loop_bounding: false`; the bounds checks of every index
/// and buffer access stay on). With it every loop of every shader carried a hidden 64-bit iteration counter and a break test
/// (so a malformed loop cannot hang the GPU, which Metal / Vulkan compilers may otherwise treat as unreachable code): on an
/// M1 Pro at 2560x1600 the frame's GPU time went down by 0.3-0.9 ms with the default set (alps 8.5 -> 7.8, industrial 13.2 ->
/// 12.3, monaco1 8.9 -> 8.6 ms), 0.1-0.4 ms with MetalFX. The renderer's own shaders are trusted for this: every loop in them
/// is a `for (var i = a; i < n; i++)`-style loop over an integer counter that only its update changes, so it ends for any
/// value of a uniform bound (`renderer_shaders_have_only_counted_loops` checks the shape of every loop of the sources; a
/// `loop` / `while` is refused). Backend independent: it holds on Vulkan / Android as on Metal.
pub(crate) fn wgsl_module(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    let desc = wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) };
    let checks = wgpu::ShaderRuntimeChecks { force_loop_bounding: false, ..wgpu::ShaderRuntimeChecks::checked() };
    // SAFETY: the only check given up is the loop bounding, and every loop of the renderer's shaders terminates (above)
    unsafe { device.create_shader_module_trusted(desc, checks) }
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

impl OverScene<'_> {
    /// The colour's store op: a multisampled colour no later pass reads (`!keep`) is discarded once resolved.
    pub fn store(&self) -> wgpu::StoreOp {
        if self.samples > 1 && !self.keep { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store }
    }
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
    /// the frame's staging belt: the passes' uniforms go through it into `encoder` ([`belt_write`])
    pub belt: &'a mut wgpu::util::StagingBelt,
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

    /// The value the last [`Self::get`] gave (none before the first).
    pub fn value(&self) -> Option<&V> {
        self.0.as_ref().map(|(_, v)| v)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The post-pass kit: what every full-screen pass of the post chain and of the passes over the scene repeated.

/// The blend states the full-screen passes share.
pub(crate) mod blend {
    use wgpu::{BlendComponent, BlendFactor, BlendOperation, BlendState};

    /// `src + dst`.
    pub const ADD_COMPONENT: BlendComponent = BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
    /// The destination's own value: the target's alpha kept.
    pub const KEEP: BlendComponent = BlendComponent { src_factor: BlendFactor::Zero, dst_factor: BlendFactor::One, operation: BlendOperation::Add };
    /// Additive colour, the target's alpha kept (the bloom's adds).
    pub const ADD_KEEP_ALPHA: BlendState = BlendState { color: ADD_COMPONENT, alpha: KEEP };
    /// Additive colour and alpha (the light shafts' and the haze's composites, which write the colour channels only).
    pub const ADD: BlendState = BlendState { color: ADD_COMPONENT, alpha: ADD_COMPONENT };
    /// Premultiplied "over" (`src + dst (1 - src alpha)`), the target's alpha kept: the original's `SRCALPHA` /
    /// `INVSRCALPHA` with the colour already multiplied by its alpha (the lighthouse beam).
    pub const OVER_KEEP_ALPHA: BlendState = BlendState { color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::OneMinusSrcAlpha, operation: BlendOperation::Add }, alpha: KEEP };
    /// `src * dst`, the target's alpha kept (the corner shading's apply).
    pub const MULTIPLY_KEEP_ALPHA: BlendState = BlendState { color: BlendComponent { src_factor: BlendFactor::Dst, dst_factor: BlendFactor::Zero, operation: BlendOperation::Add }, alpha: KEEP };
}

/// A bilinear sampler clamped to the edge (the passes' own samplers of an image of theirs).
pub(crate) fn linear_clamp_sampler(device: &wgpu::Device, label: Option<&str>) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor { label, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() })
}

/// `bytes` into `buf` from offset 0 through the frame's staging belt: a copy recorded in `encoder`, so it lands in order with
/// the passes around it (a `Queue::write_buffer` lands before the whole submit: two of them in one frame's command buffer would
/// leave the first pass reading the second value).
pub(crate) fn belt_write(belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, buf: &wgpu::Buffer, bytes: &[u8]) {
    let size = wgpu::BufferSize::new(bytes.len() as u64).expect("non-empty write");
    belt.write_buffer(encoder, buf, 0, size).copy_from_slice(bytes);
}

/// A uniform buffer of one `T`, written only when the value changed (the buffer keeps the last one).
pub(crate) struct UniformBuf<T> {
    pub buf: wgpu::Buffer,
    last: Option<T>,
}

impl<T: bytemuck::Pod> UniformBuf<T> {
    pub fn new(device: &wgpu::Device, label: &str) -> Self {
        Self { buf: uniform_buf::<T>(device, label), last: None }
    }

    /// `v` into the buffer through the frame's staging belt (a copy recorded in `encoder`, in order with the passes), skipped
    /// when the buffer already holds these bytes.
    pub fn write(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, v: &T) {
        if self.last.as_ref().is_some_and(|l| bytemuck::bytes_of(l) == bytemuck::bytes_of(v)) {
            return;
        }
        belt_write(belt, encoder, &self.buf, bytemuck::bytes_of(v));
        self.last = Some(*v);
    }
}

/// Values built when a key is first asked for and kept from then on: a pass's pipelines per variant (depth kind,
/// sample count, format, ...), so a variant that never runs is never compiled.
pub(crate) struct Variants<K, V>(std::collections::HashMap<K, V>);

impl<K, V> Default for Variants<K, V> {
    fn default() -> Self {
        Self(std::collections::HashMap::new())
    }
}

impl<K: std::hash::Hash + Eq, V> Variants<K, V> {
    /// The value of `key`, `make()`'s at the first ask.
    pub fn get(&mut self, key: K, make: impl FnOnce() -> V) -> &V {
        self.0.entry(key).or_insert_with(make)
    }
}

/// Record a full-screen draw: `pipeline`, `groups` as groups 0, 1, ..., one triangle.
pub(crate) fn draw_fullscreen(pass: &mut wgpu::RenderPass, pipeline: &wgpu::RenderPipeline, groups: &[&wgpu::BindGroup]) {
    pass.set_pipeline(pipeline);
    for (i, g) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *g, &[]);
    }
    pass.draw(0..3, 0..1);
}

/// One full-screen pass into `view` ([`color_pass`], timed as `prof`) drawing `pipeline` with `groups`.
pub(crate) fn fullscreen_pass(encoder: &mut wgpu::CommandEncoder, label: &str, prof: &'static str, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>, pipeline: &wgpu::RenderPipeline, groups: &[&wgpu::BindGroup]) {
    let mut pass = color_pass(encoder, label, prof, view, load);
    draw_fullscreen(&mut pass, pipeline, groups);
}

/// An image a pass renders into, `1 / div` of the size it is made for in each direction (rounded up, at least 1),
/// remade only when that size changes.
pub(crate) struct PostTarget {
    /// the size it was made for (before the division)
    pub size: (u32, u32),
    /// (the view keeps its texture alive)
    pub view: wgpu::TextureView,
}

impl PostTarget {
    /// The scale of a half-size image (the light shafts' rays, the corner shading's occlusion).
    pub const HALF: u32 = 2;
    /// The scale of a quarter-size image (the haze's march, ENHANCEMENT E36).
    pub const QUARTER: u32 = 4;

    /// `slot`'s image for `size` (made when missing or of another size).
    pub fn ensure<'s>(slot: &'s mut Option<PostTarget>, device: &wgpu::Device, label: &str, size: (u32, u32), div: u32, format: wgpu::TextureFormat) -> &'s PostTarget {
        if slot.as_ref().is_none_or(|i| i.size != size) {
            let view = texture_2d(device, label, size.0.div_ceil(div).max(1), size.1.div_ceil(div).max(1), format, RT_USAGE).create_view(&Default::default());
            *slot = Some(PostTarget { size, view });
        }
        slot.as_ref().expect("post target")
    }
}

/// The frame's depth bound as group 1, binding 0 (`wgsl::dtex`) of the post chain's passes (the motion blur, the depth of
/// field, the windscreen drops): a layout per kind (plain / multisampled) and the bind group, kept while the depth holds.
pub(crate) struct DepthBinding {
    label: &'static str,
    /// `[plain, multisampled]`
    layouts: [wgpu::BindGroupLayout; 2],
    bg: Keyed<(wgpu::TextureView, bool), wgpu::BindGroup>,
}

impl DepthBinding {
    pub fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let layout = |multisampled: bool| bgl(device, label, &[bgl_tex(0, wgpu::TextureSampleType::Depth, multisampled)]);
        Self { label, layouts: [layout(false), layout(true)], bg: Keyed::default() }
    }

    /// The layout of a plain or a `multisampled` depth.
    pub fn layout(&self, multisampled: bool) -> &wgpu::BindGroupLayout {
        &self.layouts[multisampled as usize]
    }

    /// The bind group of `depth`.
    pub fn bind(&mut self, device: &wgpu::Device, depth: DepthSrc) -> &wgpu::BindGroup {
        let (label, layout) = (self.label, &self.layouts[depth.multisampled as usize]);
        self.bg.get((depth.view.clone(), depth.multisampled), || view_bg(device, label, layout, depth.view))
    }
}

#[cfg(test)]
mod loop_tests {
    /// The WGSL of the crate's sources: the `.wgsl` files and the raw strings of the `.rs` files that hold a shader.
    fn shader_sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).expect("read src").flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut out = Vec::new();
        for f in files {
            let name = f.display().to_string();
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            match f.extension().and_then(|e| e.to_str()) {
                Some("wgsl") => out.push((name, text)),
                Some("rs") => {
                    let mut rest = text.as_str();
                    while let Some(a) = rest.find("r#\"") {
                        let body = &rest[a + 3..];
                        let Some(b) = body.find("\"#") else { break };
                        let s = &body[..b];
                        if s.contains("@fragment") || s.contains("@compute") || s.contains("@vertex") || (s.contains("fn ") && s.contains("vec")) {
                            out.push((name.clone(), s.to_string()));
                        }
                        rest = &body[b + 2..];
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The loop bounding is off for the renderer's shaders (`wgsl_module`), which is sound only while every loop in them ends:
    /// no `loop` / `while`, and every `for` is `for (var x = ..; x <op> ..; x++ / x = x + 1 / x = x >> 1u ...)`.
    #[test]
    fn renderer_shaders_have_only_counted_loops() {
        let srcs = shader_sources();
        assert!(srcs.len() > 20, "found only {} shader sources", srcs.len());
        let mut fors = 0;
        for (file, src) in &srcs {
            for (n, line) in src.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                let words: Vec<&str> = code.split(|c: char| !(c.is_alphanumeric() || c == '_')).collect();
                assert!(!words.contains(&"loop") && !words.contains(&"while"), "{file}:{}: a `loop` / `while` in a shader: {}", n + 1, code.trim());
                let Some(at) = code.find("for (") else { continue };
                fors += 1;
                let head = &code[at + 5..];
                let head = &head[..head.find(") {").unwrap_or_else(|| panic!("{file}:{}: a `for` header on more than one line", n + 1))];
                let parts: Vec<&str> = head.split(';').map(str::trim).collect();
                assert_eq!(parts.len(), 3, "{file}:{}: {head}", n + 1);
                let var = parts[0].strip_prefix("var ").and_then(|v| v.split([' ', ':', '=']).next()).unwrap_or_else(|| panic!("{file}:{}: no `var` counter: {head}", n + 1));
                let cond_ok = ["<", "<=", ">", ">="].iter().any(|op| parts[1].starts_with(&format!("{var} {op} ")));
                let upd = parts[2].replace(' ', "");
                let upd_ok = upd == format!("{var}++") || upd == format!("{var}--") || upd == format!("{var}={var}+1") || upd == format!("{var}={var}+1u") || upd == format!("{var}={var}-1") || upd == format!("{var}={var}>>1u");
                assert!(cond_ok && upd_ok, "{file}:{}: not a counted loop: for ({head})", n + 1);
                // the counter is not assigned in the body (checked on the lines up to the loop's closing brace at its indent)
                let indent = line.len() - line.trim_start().len();
                for body in src.lines().skip(n + 1) {
                    let bi = body.len() - body.trim_start().len();
                    if bi <= indent && body.trim_start().starts_with('}') {
                        break;
                    }
                    let b = body.split("//").next().unwrap_or("").trim_start();
                    assert!(!(b.starts_with(&format!("{var} =")) || b.starts_with(&format!("{var} +=")) || b.starts_with(&format!("{var} -=")) || b.starts_with(&format!("{var}++")) || b.starts_with(&format!("{var}--"))), "{file}: the counter `{var}` of the loop at line {} is changed in its body", n + 1);
                }
            }
        }
        assert!(fors > 40, "found only {fors} loops");
    }
}
