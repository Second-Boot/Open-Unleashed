//! ENHANCEMENT E12 — soft particles: the depth of the opaque part of a 3D pass, handed to the
//! blended particle draws as a readable image (`soft.wgsl` converts it, `shader.wgsl` fades with it).

use crate::gfx::{self, Fullscreen};
use crate::wgsl;

pub(crate) const SOFT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// The converter pipelines (single-sample depth / multisampled depth, one each) and the images they
/// write, one per pass size (kept; a resize or a scale change makes another).
/// The pass's shader: `soft.wgsl` with the full-screen triangle (`wgsl.rs`).
pub(crate) fn source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, include_str!("soft.wgsl")])
}

pub(crate) struct SoftDepth {
    pipe_1: wgpu::RenderPipeline,
    pipe_ms: wgpu::RenderPipeline,
    layout_1: wgpu::BindGroupLayout,
    layout_ms: wgpu::BindGroupLayout,
    images: std::collections::HashMap<(u32, u32), (wgpu::Texture, wgpu::TextureView)>,
}

impl SoftDepth {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = gfx::wgsl_module(device, "soft depth", &source());
        let layout = |multisampled: bool| {
            gfx::bgl(device, "soft depth", &[gfx::bgl_tex(if multisampled { 0 } else { 1 }, wgpu::TextureSampleType::Depth, multisampled)])
        };
        let (layout_1, layout_ms) = (layout(false), layout(true));
        let pipe = |label: &str, fs: &str, bgl: &wgpu::BindGroupLayout| {
            let pl = gfx::pipeline_layout(device, label, &[bgl]);
            Fullscreen::new(label, &pl, &module).fs(fs).build(device, &[gfx::target(SOFT_FORMAT, None)])
        };
        let (pipe_1, pipe_ms) = (pipe("soft depth 1", "fs_1", &layout_1), pipe("soft depth ms", "fs_ms", &layout_ms));
        Self { pipe_1, pipe_ms, layout_1, layout_ms, images: Default::default() }
    }

    /// Convert `depth` (the pass's depth, `multisampled` or not) into the image of `width × height`
    /// and return that image's view. Records one small full-screen pass into `encoder`.
    pub(crate) fn convert(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, depth: &wgpu::TextureView, multisampled: bool, width: u32, height: u32) -> wgpu::TextureView {
        if self.images.len() > 6 && !self.images.contains_key(&(width, height)) {
            self.images.clear();
        }
        let (_, view) = self.images.entry((width, height)).or_insert_with(|| {
            let t = gfx::texture_2d(device, "soft depth", width, height, SOFT_FORMAT, gfx::RT_USAGE);
            let v = t.create_view(&Default::default());
            (t, v)
        });
        let view = view.clone();
        let (pipe, layout, binding) = if multisampled { (&self.pipe_ms, &self.layout_ms, 0) } else { (&self.pipe_1, &self.layout_1, 1) };
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("soft depth"), layout, entries: &[wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(depth) }] });
        let mut pass = gfx::color_pass(encoder, "soft depth pass", "soft depth convert", &view, wgpu::LoadOp::Clear(wgpu::Color::WHITE));
        pass.set_pipeline(pipe);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        view
    }
}
