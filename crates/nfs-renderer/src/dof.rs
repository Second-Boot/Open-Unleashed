//! ENHANCEMENTS E31 / the photo mode (D46) — depth of field on the float scene (E7), before the bloom. **Not in the
//! original, render-only**: the app hands a [`Dof`] (the focus distance and the lens's f-number) with the
//! scene of the photo mode (PORT-DEVIATION D46) or of a replay (E31); this pass only reads the resolved float image and
//! the frame's depth and writes a new image, which the bloom, the eye adaptation and the tone map then take
//! as the scene. Without a `Dof` (the race, the front end, every switch off) nothing runs.
//!
//! The blur is a gather: per pixel a circle of confusion from the linear depth `d`,
//! `coc = height * C / N * |1 - F / d|` pixels (focus `F` metres, f-number `N`, `C` = [`COC_PER_HEIGHT`]),
//! clamped to [`MAX_COC_PER_HEIGHT`] of the height, and a Vogel disc of taps around the pixel (rotated per
//! pixel by interleaved gradient noise) that count when the tap's own circle reaches the pixel. A tap behind
//! a sharper pixel cannot blur it more than the pixel's own circle (a sharp car keeps its edge against a
//! blurred background), a nearer blurred tap spreads over what is behind it (a blurred foreground bleeds
//! over a sharp background). The taps are clamped in brightness so a lit window or the sun gives a bokeh
//! disc and not a sparkle; the float image keeps its highlights, so the discs are bright.

use crate::gfx::{self, DepthBinding, DepthSrc, Fullscreen, PostCtx, UniformBuf, Variants};
use crate::wgsl;
use crate::gpu::HDR_FORMAT;

/// A lens setting for one frame (focus distance, f-number); `Copy` like the rest of the scene's header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dof {
    /// the distance in metres (along the view) that is sharp
    pub focus: f32,
    /// the lens's f-number: the smaller, the shallower (the blur grows as `1 / N`)
    pub f_number: f32,
}

/// The circle of confusion at an infinitely far point, in screen heights, at f/1 (so `C / N` at f/N).
pub const COC_PER_HEIGHT: f32 = 0.035;
/// The largest circle (screen heights) a pixel is ever blurred by.
pub const MAX_COC_PER_HEIGHT: f32 = 0.04;
/// The smallest and largest f-number the app offers.
pub const F_MIN: f32 = 1.2;
pub const F_MAX: f32 = 16.0;

/// The circle's radius in pixels for a depth `d` (what the shader does; the tests check the curve).
#[cfg(test)]
fn coc_px(dof: Dof, d: f32, height: f32) -> f32 {
    (height * COC_PER_HEIGHT / dof.f_number * (1.0 - dof.focus / d.max(0.01)).abs()).min(height * MAX_COC_PER_HEIGHT)
}

/// Taps for an image of this height: more for a hi-res still, whose circles are wider in pixels.
pub fn taps_for(height: u32) -> u32 {
    (48 + (height / 1000) * 24).clamp(48, 128)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DofUniform {
    /// focus (m), pixels of circle per unit `|1 - F / d|`, the largest circle (px), taps
    a: [f32; 4],
    /// `far / (near - far)`, near, width, height
    b: [f32; 4],
}

const DOF_WGSL: &str = r#"
struct U { a: vec4<f32>, b: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(2) @binding(0) var<uniform> u: U;
// the distance in front of the camera of a depth-buffer value (the projection's own inverse)
fn lin(z: f32) -> f32 {
    return proj_dist(z, u.b.x, u.b.y);
}
fn coc_of(d: f32) -> f32 {
    return min(u.a.y * abs(1.0 - u.a.x / max(d, 0.01)), u.a.z);
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(i32(u.b.z), i32(u.b.w));
    let c = textureSampleLevel(t, s, in.uv, 0.0);
    let d0 = lin(dload(vec2<i32>(in.pos.xy), dims));
    let c0 = coc_of(d0);
    let reach = u.a.z;
    let n = i32(u.a.w);
    var sum = min(c.rgb, vec3<f32>(24.0));
    var wsum = 1.0;
    // the disc's turn of this pixel, applied to the table's unit directions (PERF: no per-tap cos / sin)
    let rot = ign(in.pos.xy) * TAU;
    let rc = cos(rot);
    let rs = sin(rot);
    for (var i = 0; i < n; i = i + 1) {
        let fi = f32(i) + 0.5;
        let r = sqrt(fi / f32(n)) * reach;
        let v = VOGEL_DIR[i];
        let sp = in.pos.xy + vec2<f32>(v.x * rc - v.y * rs, v.x * rs + v.y * rc) * r;
        let sc = clamp(vec2<i32>(sp), vec2<i32>(0), dims - vec2<i32>(1));
        let dt = lin(dload(sc, dims));
        var ct = coc_of(dt);
        // a tap behind this pixel cannot blur it more than the pixel's own circle
        if dt > d0 {
            ct = min(ct, c0 + 0.5);
        }
        let w = clamp(ct - r + 0.5, 0.0, 1.0);
        if w > 0.0 {
            let tc = textureSampleLevel(t, s, sp / vec2<f32>(dims), 0.0).rgb;
            sum = sum + min(tc, vec3<f32>(24.0)) * w;
            wsum = wsum + w;
        }
    }
    return vec4<f32>(sum / wsum, c.a);
}
"#;

/// The most taps [`taps_for`] ever asks for: the size of the direction table.
const MAX_TAPS: usize = 128;

/// The WGSL table of the Vogel disc's unit directions, tap `i` at the angle `(i + 0.5)` golden angles (before the pixel's turn):
/// `cos` / `sin` computed once here, in double precision, instead of per tap and pixel in the shader.
fn vogel_table() -> String {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let entries: Vec<String> = (0..MAX_TAPS)
        .map(|i| {
            let a = (i as f64 + 0.5) * golden;
            format!("vec2<f32>({:?}, {:?})", a.cos() as f32, a.sin() as f32)
        })
        .collect();
    format!("const VOGEL_DIR = array<vec2<f32>, {MAX_TAPS}>({});
", entries.join(", "))
}

/// The pass's shader over a plain or a `multisampled` depth.
pub(crate) fn source(multisampled: bool) -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::dtex(multisampled), wgsl::DLOAD, wgsl::IGN, wgsl::ANGLES, &vogel_table(), wgsl::PROJ_DIST, DOF_WGSL])
}

pub(crate) struct DofPass {
    /// the pipeline per depth kind (`true` = multisampled), built at its first use
    pipes: Variants<bool, wgpu::RenderPipeline>,
    image_layout: wgpu::BindGroupLayout,
    depth: DepthBinding,
    u_layout: wgpu::BindGroupLayout,
    u_buf: UniformBuf<DofUniform>,
    u_bg: wgpu::BindGroup,
}

impl DofPass {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let u_layout = gfx::bgl(device, "dof", &[gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT)]);
        let u_buf = UniformBuf::new(device, "dof");
        let u_bg = gfx::buffer_bg(device, "dof", &u_layout, &u_buf.buf);
        Self { pipes: Variants::default(), image_layout: layout.clone(), depth: DepthBinding::new(device, "dof depth"), u_layout, u_buf, u_bg }
    }

    /// Blur `src` (`cx.size`, its bind group `src_bg`) by the lens `dof`, reading `cx.depth`, into `out` (the chain's image of
    /// `cx.size`).
    pub fn run(&mut self, cx: &mut PostCtx, src_bg: &wgpu::BindGroup, dof: Dof, out: &wgpu::TextureView) {
        let PostCtx { device, depth: DepthSrc { view: depth, multisampled }, size: (width, height), camera: cam, .. } = *cx;
        let h = height as f32;
        let u = DofUniform {
            a: [dof.focus.max(0.05), h * COC_PER_HEIGHT / dof.f_number.clamp(F_MIN, F_MAX), h * MAX_COC_PER_HEIGHT, taps_for(height) as f32],
            b: [cam.far / (cam.near - cam.far), cam.near, width as f32, h],
        };
        self.u_buf.write(cx.belt, cx.encoder, &u);
        let (image_layout, depth_layout, u_layout) = (&self.image_layout, self.depth.layout(multisampled), &self.u_layout);
        let pipe = self.pipes.get(multisampled, || {
            let module = gfx::wgsl_module(device, "dof", &source(multisampled));
            let pl = gfx::pipeline_layout(device, "dof", &[image_layout, depth_layout, u_layout]);
            Fullscreen::new("dof", &pl, &module).build(device, &[gfx::target(HDR_FORMAT, None)])
        });
        let depth_bg = self.depth.bind(device, DepthSrc { view: depth, multisampled });
        gfx::fullscreen_pass(cx.encoder, "dof", "depth of field", out, gfx::load_or_clear(false), pipe, &[src_bg, depth_bg, &self.u_bg]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_circle_is_zero_at_the_focus_and_grows_with_the_distance_and_the_aperture() {
        let d = Dof { focus: 10.0, f_number: 2.8 };
        assert_eq!(coc_px(d, 10.0, 1000.0), 0.0);
        assert!(coc_px(d, 20.0, 1000.0) < coc_px(d, 100.0, 1000.0));
        assert!(coc_px(d, 5.0, 1000.0) > coc_px(d, 8.0, 1000.0), "nearer than the focus blurs too");
        let wide = Dof { focus: 10.0, f_number: 1.4 };
        assert!((coc_px(wide, 700.0, 1000.0) / coc_px(d, 700.0, 1000.0) - 2.0).abs() < 0.01, "f/1.4 is twice f/2.8");
        // never wider than the cap
        assert!(coc_px(wide, 0.05, 1000.0) <= 1000.0 * MAX_COC_PER_HEIGHT + 1e-3);
        // a hi-res still has the same blur relative to its height
        assert!((coc_px(d, 50.0, 2400.0) / 2400.0 - coc_px(d, 50.0, 1200.0) / 1200.0).abs() < 1e-6);
    }

    #[test]
    fn the_direction_table_covers_every_tap_count() {
        assert!(taps_for(u32::MAX) as usize <= MAX_TAPS);
        assert_eq!(vogel_table().matches("vec2<f32>(").count(), MAX_TAPS);
    }

    #[test]
    fn taps_grow_with_the_image() {
        assert_eq!(taps_for(800), 48);
        assert!(taps_for(2400) > taps_for(1200));
        assert_eq!(taps_for(20000), 128);
    }
}
