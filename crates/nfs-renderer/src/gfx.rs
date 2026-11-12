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
