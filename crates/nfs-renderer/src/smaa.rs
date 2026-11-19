//! ENHANCEMENT E35 — SMAA 1x as a post pass over the tone-mapped image (
//! `RE/notes/ENHANCEMENTS.md`). **Not in the original**; render-only (reads the finished 8-bit frame
//! of the main view, writes it back; nothing reaches the simulation, the network or a replay).
//!
//! The method is SMAA by Jorge Jimenez, Jose I. Echevarria, Tiago Sousa and Diego Gutierrez
//! ("SMAA: Enhanced Subpixel Morphological Antialiasing", Eurographics 2012; reference
//! implementation <https://github.com/iryoku/smaa>, MIT licence: "Permission is hereby granted, free
//! of charge, to any person obtaining a copy of this software ... to deal in the Software without
//! restriction ... The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT
//! WARRANTY OF ANY KIND ..."). Three passes: luma **edges** (threshold 0.1, local contrast
//! adaptation 2.0), **blending weights** (the run of the edge is searched to both ends, the ends'
//! crossing pattern and the pixel's distances index an *area table*, corner rounding 25 %), and the
//! **neighbourhood blend** (bilinear taps mix the pixel with the chosen neighbour by the weight).
//!
//! What differs from the reference code (no code or table of it is copied; this file and
//! `smaa.wgsl` are written from the method):
//! * the **area table is generated here** ([`area_table`]) by integrating the reconstructed edge
//!   line of each end-pattern pair (L, U / V and Z shapes, the line passing through the middle of
//!   a crossing edge) over the pixel's column, with integer distances (up to 32) instead of the
//!   reference's square-root-compressed ones;
//! * the **search texture is not used**: the reference's search table only lets a bilinear fetch
//!   test two pixels at once; this pass steps one pixel at a time with `textureLoad` (exact, up to
//!   32 pixels each way);
//! * **diagonal patterns are not handled** (SMAA's separate diagonal search / table); a crossing
//!   with both sides ("+") gets no blending. The image is the 8-bit display-referred one (the
//!   surface is a non-sRGB format on every adapter we prefer; with an sRGB one the luma is of the
//!   linear values, which still finds the same edges).
//!
//! Place: `encode_enhanced` (`gpu.rs`) — the tone-map blit writes the window-sized image into
//! [`Smaa::ldr`] instead of the destination, [`Smaa::run`] writes the smoothed image over the
//! destination, then the 2D sprites and the gamma ramp as before. With E35 off none of this exists.

use crate::gfx::{self, Fullscreen};
use crate::wgsl;
use wgpu::util::DeviceExt as _;

/// The run lengths of the area table (distances 0..=32 pixels).
const RUN: usize = 33;
/// End patterns: bit 0 = a crossing edge on the - side (the row above / the column to the left), bit 1 = on the + side.
const PATTERNS: usize = 16;
/// The images' formats: the edges (left, top), the blend weights, the area table (the two coverages).
const EDGES_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
const WEIGHTS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const AREA_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;

/// The reconstructed edge line's height, relative to the aliased edge (positive = toward the + side
/// of the pixel), at `x` along a run of length `d`, for the end patterns `pl` / `pr` (0 none, 1 the
/// crossing on the - side, 2 on the + side, 3 both = no line) and the pixel `l` / `r` pixels from
/// the near / far end (the L shapes only blend the half of the run nearer to the crossing end).
fn line(pl: usize, pr: usize, l: usize, r: usize, x: f32) -> f32 {
    let d = (l + r + 1) as f32;
    let end = |p: usize| match p {
        1 => -0.5,
        2 => 0.5,
        _ => 0.0,
    };
    if pl == 3 || pr == 3 {
        return 0.0;
    }
    let (yl, yr) = (end(pl), end(pr));
    let half = d / 2.0;
    match (pl != 0, pr != 0) {
        (false, false) => 0.0,
        // an L: the line runs from the crossing's middle to the run's middle and stops there
        (true, false) => {
            if l <= r && x < half {
                yl * (1.0 - x / half)
            } else {
                0.0
            }
        }
        (false, true) => {
            if r <= l && x > half {
                yr * ((x - half) / half)
            } else {
                0.0
            }
        }
        // a U (both ends on one side): a V line; a Z (opposite sides): one straight line
        (true, true) => {
            if pl == pr {
                if x < half { yl * (1.0 - x / half) } else { yr * ((x - half) / half) }
            } else {
                yl + (yr - yl) * (x / d)
            }
        }
    }
}

/// The area table: `RUN` wide (the pixel's distance from the near end), `PATTERNS * RUN` tall (the
/// end patterns `pl * 4 + pr`, then the distance from the far end), two bytes a texel = the
/// weights (+ side, - side), doubled (an area is at most 0.5): the mean over the pixel's column of
/// the line's height on each side of the aliased edge.
pub(crate) fn area_table() -> Vec<u8> {
    const SAMPLES: usize = 64;
    let mut out = vec![0u8; RUN * PATTERNS * RUN * 2];
    for pl in 0..4 {
        for pr in 0..4 {
            for r in 0..RUN {
                for l in 0..RUN {
                    let (mut pos, mut neg) = (0.0f32, 0.0f32);
                    for s in 0..SAMPLES {
                        let y = line(pl, pr, l, r, l as f32 + (s as f32 + 0.5) / SAMPLES as f32);
                        pos += y.max(0.0);
                        neg += (-y).max(0.0);
                    }
                    let q = |v: f32| ((v / SAMPLES as f32 * 2.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                    let i = ((pl * 4 + pr) * RUN + r) * RUN + l;
                    out[i * 2] = q(pos);
                    out[i * 2 + 1] = q(neg);
                }
            }
        }
    }
    out
}

/// The three passes, their images (kept while the window size holds) and the area table.
/// The pass's shader: `smaa.wgsl` with the full-screen triangle (`wgsl.rs`).
pub(crate) fn source() -> String {
    // the area table's run length is the Rust one (`RUN`), written as the line it was
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::LUMA_709, &include_str!("smaa.wgsl").replace("//AREA_RUN", &format!("const AREA_RUN: i32 = {RUN};"))])
}

pub(crate) struct Smaa {
    edges_pipe: wgpu::RenderPipeline,
    weights_pipe: wgpu::RenderPipeline,
    blend_pipe: wgpu::RenderPipeline,
    edges_layout: wgpu::BindGroupLayout,
    weights_layout: wgpu::BindGroupLayout,
    blend_layout: wgpu::BindGroupLayout,
    area_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    images: Option<Images>,
}

struct Images {
    size: (u32, u32),
    ldr_view: wgpu::TextureView,
    edges_view: wgpu::TextureView,
    weights_view: wgpu::TextureView,
    edges_bg: wgpu::BindGroup,
    weights_bg: wgpu::BindGroup,
    blend_bg: wgpu::BindGroup,
}


impl Smaa {
    pub(crate) fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let module = gfx::wgsl_module(device, "smaa", &source());
        let edges_layout = gfx::bgl(device, "smaa edges", &[gfx::bgl_float(0, false)]);
        let weights_layout = gfx::bgl(device, "smaa weights", &[gfx::bgl_float(0, false), gfx::bgl_float(1, false)]);
        let blend_layout = gfx::bgl(device, 
            "smaa blend",
            &[gfx::bgl_float(0, true), gfx::bgl_float(1, false), gfx::bgl_sampler(2, wgpu::SamplerBindingType::Filtering)],
        );
        let pipe = |label: &str, bgl: &wgpu::BindGroupLayout, entry: &str, target: wgpu::TextureFormat| {
            let pl = gfx::pipeline_layout(device, label, &[bgl]);
            Fullscreen::new(label, &pl, &module).fs(entry).build(device, &[gfx::target(target, None)])
        };
        let edges_pipe = pipe("smaa edges", &edges_layout, "fs_edges", EDGES_FORMAT);
        let weights_pipe = pipe("smaa weights", &weights_layout, "fs_weights", WEIGHTS_FORMAT);
        let blend_pipe = pipe("smaa blend", &blend_layout, "fs_blend", format);
        let area = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("smaa area"),
                size: wgpu::Extent3d { width: RUN as u32, height: (PATTERNS * RUN) as u32, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: AREA_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &area_table(),
        );
        let sampler = gfx::linear_clamp_sampler(device, Some("smaa"));
        Self { edges_pipe, weights_pipe, blend_pipe, edges_layout, weights_layout, blend_layout, area_view: area.create_view(&Default::default()), sampler, format, images: None }
    }

    /// The `width x height` image the tone map writes into (the surface's format), made again when the size changes.
    pub(crate) fn ldr(&mut self, device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        if !self.images.as_ref().is_some_and(|i| i.size == (width, height)) {
            let make = |label: &str, format| gfx::texture_2d(device, label, width, height, format, gfx::RT_USAGE).create_view(&Default::default());
            let ldr_view = make("smaa ldr", self.format);
            let edges_view = make("smaa edges", EDGES_FORMAT);
            let weights_view = make("smaa weights", WEIGHTS_FORMAT);
            let bg = |label: &str, layout: &wgpu::BindGroupLayout, entries: &[wgpu::BindGroupEntry]| gfx::bind_group(device, label, layout, entries);
            let edges_bg = bg("smaa edges", &self.edges_layout, &[gfx::bg_view(0, &ldr_view)]);
            let weights_bg = bg(
                "smaa weights",
                &self.weights_layout,
                &[gfx::bg_view(0, &edges_view), gfx::bg_view(1, &self.area_view)],
            );
            let blend_bg = bg(
                "smaa blend",
                &self.blend_layout,
                &[
                    gfx::bg_view(0, &ldr_view),
                    gfx::bg_view(1, &weights_view),
                    gfx::bg_sampler(2, &self.sampler),
                ],
            );
            self.images = Some(Images { size: (width, height), ldr_view, edges_view, weights_view, edges_bg, weights_bg, blend_bg });
        }
        self.images.as_ref().map(|i| i.ldr_view.clone()).expect("smaa images")
    }

    /// Edges, weights and the blend of the image [`Self::ldr`] gave out, written over all of `dest`.
    pub(crate) fn run(&self, encoder: &mut wgpu::CommandEncoder, dest: &wgpu::TextureView) {
        let Some(im) = self.images.as_ref() else { return };
        let pass = |encoder: &mut wgpu::CommandEncoder, label: &'static str, view: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, bg: &wgpu::BindGroup| {
            gfx::fullscreen_pass(encoder, label, label, view, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), pipe, &[bg]);
        };
        pass(encoder, "smaa edges", &im.edges_view, &self.edges_pipe, &im.edges_bg);
        pass(encoder, "smaa weights", &im.weights_view, &self.weights_pipe, &im.weights_bg);
        pass(encoder, "smaa blend", dest, &self.blend_pipe, &im.blend_bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &[u8], pl: usize, pr: usize, l: usize, r: usize) -> (f32, f32) {
        let i = ((pl * 4 + pr) * RUN + r) * RUN + l;
        (t[i * 2] as f32 / 255.0 / 2.0, t[i * 2 + 1] as f32 / 255.0 / 2.0)
    }

    #[test]
    fn area_table_shapes() {
        let t = area_table();
        assert_eq!(t.len(), RUN * PATTERNS * RUN * 2);
        // no crossing at either end, or a "+" end: nothing to blend
        assert_eq!(at(&t, 0, 0, 3, 4), (0.0, 0.0));
        assert_eq!(at(&t, 3, 1, 2, 2), (0.0, 0.0));
        // an L on the - side over two pixels (d = 2): the line -0.5 -> 0 across the first pixel, mean 0.25
        let (pos, neg) = at(&t, 1, 0, 0, 1);
        assert!(pos == 0.0 && (neg - 0.25).abs() < 0.01, "{pos} {neg}");
        // ... and the far pixel of the L (l > r) is not blended
        assert_eq!(at(&t, 1, 0, 1, 0), (0.0, 0.0));
        // an L on the + side swaps the sides
        let (pos, neg) = at(&t, 2, 0, 0, 1);
        assert!(neg == 0.0 && (pos - 0.25).abs() < 0.01);
        // a Z (- side, then + side) over 4 pixels: the first pixel is above the edge, the last below
        let (p0, n0) = at(&t, 1, 2, 0, 3);
        let (p3, n3) = at(&t, 1, 2, 3, 0);
        assert!(p0 == 0.0 && n0 > 0.1 && n3 == 0.0 && p3 > 0.1);
        assert!((n0 - p3).abs() < 0.01, "a Z is symmetric");
    }
}
