//! ENHANCEMENTS E27 motion blur, E28 lens dirt, E29 camera shake — the camera effects of the
//! enhanced game (`RE/notes/ENHANCEMENTS.md`). **None of them is in the original**
//! and all three are render-only: they read the finished image (and, for the blur, the depth the
//! frame drew) and write another image; nothing reaches the simulation, the camera the game uses,
//! the network or a replay.
//!
//! * **E27** — a camera-motion blur on the float scene (E7), before the bloom: every pixel is
//!   reprojected with the previous frame's view-projection (`prev VP * inverse(VP)`, built in f64 on
//!   the CPU, applied to the pixel's depth) and the image is averaged along that screen motion; no
//!   per-object vectors, so a car that moves with the camera is kept sharp by the app's mask
//!   ([`MotionPrev::keep`]) and by a fade of everything nearer than 1.5-3.5 m (the cockpit, the
//!   hood). The streak is capped (a fraction of the screen height) and taps nearer than the pixel
//!   do not bleed over it.
//! * **E28** — a procedural dirt texture ([`generate_dirt`], made at first use, no asset) that
//!   multiplies the bloom's last add (`bloom.rs`): where nothing is bright the bloom is black, so the
//!   dirt shows only around a bright light.
//! * **E29** — a real camera shake: the app moves the RENDER camera (a small position offset and
//!   rotation of the view) and the renderer simply draws from it; there is no renderer code for it.
//!   (Until ENH-SHAKE2 it was an image-space offset / roll / zoom in the tone-map pass.)

use crate::Camera;
use crate::gfx::{self, DepthSrc, Fullscreen, Keyed, PostCtx, RtImage};
use crate::wgsl;
use crate::gpu::HDR_FORMAT;
use glam::{DMat4, DVec3, DVec4, Mat4};

/// ENHANCEMENT E27: what the blur needs of the previous frame, set by the app for the race's main view.
#[derive(Clone, Copy, Debug)]
pub struct MotionPrev {
    /// the main view's camera of the previous frame (the renderer builds the same projection from it)
    pub camera: Camera,
    /// the fraction of a frame's screen motion the streak covers (the exposure over the frame time, 0..1)
    pub shutter: f32,
    /// the player's own car on the screen, `[cx, cy, rx, ry]` in uv units (ellipse); `rx <= 0` = none.
    /// The car moves with the camera, so it is not blurred by the camera's motion
    pub keep: [f32; 4],
}

/// The blur's longest streak as a fraction of the screen height, by level (1 light, 2 medium).
pub(crate) fn blur_max_len(level: u8) -> f32 {
    match level {
        0 => 0.0,
        1 => 0.015,
        _ => 0.03,
    }
}

/// How strongly the dirt lights where the bloom is: `bloom * (1 + DIRT_GAIN * dirt)`.
pub(crate) const DIRT_GAIN: f32 = 70.0;

// ---------------------------------------------------------------------------------------------
// E28: the dirt texture

/// Width and height of the generated dirt texture (2:1; it is stretched over the screen).
pub(crate) const DIRT_SIZE: (u32, u32) = (1024, 512);

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = hash(self.0.wrapping_add(0x9e37_79b9));
        (self.0 >> 8) as f32 / 16_777_216.0
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smooth value noise on an integer lattice, 0..1.
fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let h = |dx: i32, dy: i32| (hash(seed ^ hash((xi as i32 + dx) as u32).wrapping_add(hash((yi as i32 + dy) as u32).wrapping_mul(31))) >> 8) as f32 / 16_777_216.0;
    let (a, b, c, d) = (h(0, 0), h(1, 0), h(0, 1), h(1, 1));
    (a + (b - a) * ux) + ((c + (d - c) * ux) - (a + (b - a) * ux)) * uy
}

/// ENHANCEMENT E28: a procedural lens-dirt mask, `width x height` values (0 = clean, 1 = the dirtiest
/// spot): soft smudges (fractal noise, thresholded), a few greasy blobs, many dust specks and dried
/// droplets (a faint disc with a brighter rim), and some wipe arcs, a little stronger toward the edges
/// (a lens collects its dirt at the rim). Deterministic: the same texture on every run.
pub(crate) fn generate_dirt(width: u32, height: u32) -> Vec<f32> {
    dirt_layers(width, height, true)
}

/// [`generate_dirt`]; `all` false = only the soft layers (smudges and greasy blobs), for the test that they have no
/// straight edge.
fn dirt_layers(width: u32, height: u32, all: bool) -> Vec<f32> {
    let (w, h) = (width as usize, height as usize);
    let mut img = vec![0.0f32; w * h];
    let mut rng = Rng(0x1d17_c0de);
    // smudges: thresholded fractal noise
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32 * 4.0, y as f32 / h as f32 * 2.0);
            let mut f = 0.0;
            let mut a = 0.5;
            let mut fr = 1.0;
            for o in 0..4 {
                f += a * vnoise(u * fr, v * fr, 11 + o);
                a *= 0.5;
                fr *= 2.0;
            }
            let m = smoothstep(0.42, 0.78, f / 0.9375);
            img[y * w + x] = 0.75 * m * m;
        }
    }
    let put = |img: &mut [f32], cx: f32, cy: f32, r: f32, amp: f32, rim: f32| {
        let (x0, x1) = (((cx - r - 1.0).floor().max(0.0)) as usize, ((cx + r + 1.0).ceil().min(w as f32 - 1.0)) as usize);
        let (y0, y1) = (((cy - r - 1.0).floor().max(0.0)) as usize, ((cy + r + 1.0).ceil().min(h as f32 - 1.0)) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                // a faint disc and a brighter rim just inside its edge
                let disc = (1.0 - smoothstep(0.85, 1.0, d)) * (1.0 - rim);
                let ring = smoothstep(0.55, 0.9, d) * (1.0 - smoothstep(0.9, 1.05, d)) * rim;
                let v = amp * (disc + ring);
                let p = &mut img[y * w + x];
                *p = p.max(v);
            }
        }
    };
    // greasy blobs (large, faint, soft)
    for _ in 0..14 {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let r = 40.0 + rng.next() * 70.0;
        let amp = 0.12 + rng.next() * 0.2;
        // ENH-SUNEDGE: the blob is written over 2 r round its centre, where the Gaussian is already below half a byte
        // (exp(-12) x 0.32 x 1.5 x 255 < 0.001). It was cut at 1 r, a square box: at its sides the mask jumped by
        // exp(-3) x amp (up to 6 of 255), and the bloom's last add multiplies the mask by DIRT_GAIN (70) x the glow,
        // so round the HDR sun a blob near the screen's left / right edge drew a hard straight-sided square
        let reach = 2.0 * r;
        let (x0, x1) = (((cx - reach).floor().max(0.0)) as usize, ((cx + reach).ceil().min(w as f32 - 1.0)) as usize);
        let (y0, y1) = (((cy - reach).floor().max(0.0)) as usize, ((cy + reach).ceil().min(h as f32 - 1.0)) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                let g = (-d * d * 3.0).exp() * amp;
                let p = &mut img[y * w + x];
                *p = (*p + g).min(1.0);
            }
        }
    }
    if !all {
        return img;
    }
    // dust specks and dried droplets
    for _ in 0..420 {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let r = 1.3 + 7.0 * rng.next().powi(3);
        let amp = 0.25 + 0.75 * rng.next();
        let rim = if r > 3.5 { 0.6 } else { 0.0 };
        put(&mut img, cx, cy, r, amp, rim);
    }
    // a few big dried droplets
    for _ in 0..6 {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        put(&mut img, cx, cy, 14.0 + rng.next() * 12.0, 0.30 + rng.next() * 0.25, 0.8);
    }
    // wipe arcs: soft curved lines
    for _ in 0..5 {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let rad = 150.0 + rng.next() * 300.0;
        let a0 = rng.next() * std::f32::consts::TAU;
        let sweep = 0.4 + rng.next() * 0.7;
        let width_px = 2.0 + rng.next() * 4.0;
        let amp = 0.15 + rng.next() * 0.2;
        let steps = (rad * sweep) as i32;
        for s in 0..steps {
            let a = a0 + sweep * s as f32 / steps as f32;
            let fade = (std::f32::consts::PI * s as f32 / steps as f32).sin();
            put(&mut img, cx + rad * a.cos(), cy + rad * a.sin(), width_px, amp * fade, 0.0);
        }
    }
    // a little stronger toward the edges
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32 * 2.0 - 1.0, y as f32 / h as f32 * 2.0 - 1.0);
            let e = 0.6 + 0.9 * ((u * u + v * v) * 0.5).min(1.0);
            let p = &mut img[y * w + x];
            *p = (*p * e).clamp(0.0, 1.0);
        }
    }
    img
}

/// The dirt texture on the GPU and the bind group the bloom's last add samples it through (the
/// sprite layout: a texture and a linear sampler).
pub(crate) struct Dirt {
    _texture: wgpu::Texture,
    pub bind_group: wgpu::BindGroup,
}

impl Dirt {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout, sampler: &wgpu::Sampler) -> Self {
        let (w, h) = DIRT_SIZE;
        // ENH-SUNEDGE: half floats, not bytes. The bloom's last add multiplies the mask by DIRT_GAIN x INTENSITY
        // (17.5) x the wide glow, which round the HDR sun is several times white: one byte step (1/255) of a
        // smudge's or blob's soft tail was a visible hard contour there (on an EDR screen nothing clips it).
        // `R16Float` is filterable on every backend (core WebGPU).
        let texture = gfx::texture_2d(device, "lens dirt", w, h, wgpu::TextureFormat::R16Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &generate_dirt(w, h).iter().flat_map(|&v| crate::gpu::f32_to_f16(v).to_le_bytes()).collect::<Vec<u8>>(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 2), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&Default::default());
        let bind_group = gfx::tex_sampler_bg(device, "lens dirt", layout, &view, sampler);
        Self { _texture: texture, bind_group }
    }
}

// ---------------------------------------------------------------------------------------------
// E27: the motion blur pass

/// `prev VP * inverse(VP)` in f64 (the world's coordinates are in the thousands of metres, so the two
/// matrices' product is cancelled out in double precision), for the main view of `aspect` (`mirror`
/// = the `MIRROR` race option's flip, which both views share).
pub(crate) fn reprojection(cur: &Camera, prev: &Camera, aspect: f32, mirror: bool) -> Mat4 {
    let r = vp64(prev, aspect, mirror) * vp64(cur, aspect, mirror).inverse();
    Mat4::from_cols_array(&r.to_cols_array().map(|v| v as f32))
}

/// `Camera::view_projection_mirrored` in f64.
pub(crate) fn vp64(c: &Camera, aspect: f32, mirror: bool) -> DMat4 {
    vp64_at(c, aspect, mirror, false)
}

/// [`vp64`], or with `rotation_only` the same view's matrix with the eye at the origin (ENHANCEMENT
/// ENH-TEMPORAL: the sky moves with the camera's rotation alone).
pub(crate) fn vp64_at(c: &Camera, aspect: f32, mirror: bool, rotation_only: bool) -> DMat4 {
    let (pos, target, up) = (c.position.as_dvec3(), c.target.as_dvec3(), c.up.as_dvec3());
    let f = (target - pos).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    let eye = if rotation_only { DVec3::ZERO } else { pos };
    let view = DMat4::from_cols(DVec4::new(s.x, u.x, -f.x, 0.0), DVec4::new(s.y, u.y, -f.y, 0.0), DVec4::new(s.z, u.z, -f.z, 0.0), DVec4::new(-eye.dot(s), -eye.dot(u), eye.dot(f), 1.0));
    let (near, far) = (c.near as f64, c.far as f64);
    let (sin_fov, cos_fov) = (0.5 * c.fov_y_radians as f64).sin_cos();
    let h = cos_fov / sin_fov;
    let w = h / aspect as f64;
    let r = far / (near - far);
    let proj = DMat4::from_cols(DVec4::new(w, 0.0, 0.0, 0.0), DVec4::new(0.0, h, 0.0, 0.0), DVec4::new(0.0, 0.0, r, -1.0), DVec4::new(0.0, 0.0, r * near, 0.0));
    DMat4::from_scale(DVec3::new(if mirror { 1.0 } else { -1.0 }, 1.0, 1.0)) * proj * view
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniform {
    reproj: [[f32; 4]; 4],
    /// shutter, longest streak (screen heights), `far / (near - far)`, near
    p: [f32; 4],
    /// the player's car ellipse (uv): centre, radii
    keep: [f32; 4],
    /// aspect, width, height
    q: [f32; 4],
}

const BLUR_WGSL: &str = r#"
struct U { reproj: mat4x4<f32>, p: vec4<f32>, keep: vec4<f32>, q: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(2) @binding(0) var<uniform> u: U;
// the distance in front of the camera of a depth-buffer value (the projection's own inverse)
fn lin(z: f32) -> f32 {
    return u.p.z * u.p.w / (z + u.p.z);
}
// 0 inside the player's car, 1 outside (a soft edge)
fn keep_w(uv: vec2<f32>) -> f32 {
    if u.keep.z <= 0.0 {
        return 1.0;
    }
    return smoothstep(0.85, 1.15, length((uv - u.keep.xy) / u.keep.zw));
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let c = textureSampleLevel(t, s, uv, 0.0);
    let dims = u.q.yz;
    let z = dload(vec2<i32>(in.pos.xy), vec2<i32>(dims));
    let dl = lin(z);
    // nearer than 1.5..3.5 m is the cockpit / hood, which moves with the camera
    let k = smoothstep(1.5, 3.5, dl) * keep_w(uv);
    if k <= 0.0 {
        return c;
    }
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    let q = u.reproj * ndc;
    if q.w < 0.0001 {
        return c;
    }
    let pn = q.xy / q.w;
    // where the pixel came from, as a move of the uv
    let v = vec2<f32>(ndc.x - pn.x, -(ndc.y - pn.y)) * 0.5;
    let len = length(vec2<f32>(v.x * u.q.x, v.y)) * u.p.x * k;
    if len < 0.00001 {
        return c;
    }
    let ve = v * (u.p.x * k * min(1.0, u.p.y / len));
    if length(ve * dims) < 0.6 {
        return c;
    }
    var sum = c.rgb;
    var wsum = 1.0;
    for (var i = 0; i < 12; i = i + 1) {
        let tt = (f32(i) + 0.5) / 12.0 - 0.5;
        let suv = uv + ve * tt;
        let sc = clamp(vec2<i32>(suv * dims), vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
        let dt = lin(dload(sc, vec2<i32>(dims)));
        // a tap nearer than the pixel does not smear over it; one inside the car is not taken
        let w = smoothstep(0.55, 0.95, dt / max(dl, 0.05)) * keep_w(suv);
        sum = sum + textureSampleLevel(t, s, suv, 0.0).rgb * w;
        wsum = wsum + w;
    }
    return vec4<f32>(sum / wsum, c.a);
}
"#;

/// The pass's shader over a plain or a `multisampled` depth.
pub(crate) fn blur_source(multisampled: bool) -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::dtex(multisampled), wgsl::DLOAD, BLUR_WGSL])
}

pub(crate) struct MotionBlur {
    pipe_1: wgpu::RenderPipeline,
    pipe_ms: wgpu::RenderPipeline,
    depth_layout_1: wgpu::BindGroupLayout,
    depth_layout_ms: wgpu::BindGroupLayout,
    u_buf: wgpu::Buffer,
    u_bg: wgpu::BindGroup,
    /// the blurred image, kept while the size holds
    image: Option<RtImage>,
    /// the depth's bind group, kept while the depth image holds
    depth_bg: Keyed<(wgpu::TextureView, bool), wgpu::BindGroup>,
}

impl MotionBlur {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let depth_layout = |multisampled: bool| gfx::bgl(device, "motion blur depth", &[gfx::bgl_tex(0, wgpu::TextureSampleType::Depth, multisampled)]);
        let (depth_layout_1, depth_layout_ms) = (depth_layout(false), depth_layout(true));
        let u_layout = gfx::bgl(device, "motion blur", &[gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT)]);
        let u_buf = gfx::uniform_buf::<BlurUniform>(device, "motion blur");
        let u_bg = gfx::buffer_bg(device, "motion blur", &u_layout, &u_buf);
        let pipe = |multisampled: bool, dl: &wgpu::BindGroupLayout| {
            let module = gfx::wgsl_module(device, "motion blur", &blur_source(multisampled));
            let pl = gfx::pipeline_layout(device, "motion blur", &[layout, dl, &u_layout]);
            Fullscreen::new("motion blur", &pl, &module).build(device, &[gfx::target(HDR_FORMAT, None)])
        };
        let (pipe_1, pipe_ms) = (pipe(false, &depth_layout_1), pipe(true, &depth_layout_ms));
        Self { pipe_1, pipe_ms, depth_layout_1, depth_layout_ms, u_buf, u_bg, image: None, depth_bg: Keyed::default() }
    }

    /// Blur `src` (`cx.size`, its bind group `src_bg`) by the camera's move since `prev`, reading `cx.depth`; returns the
    /// blurred image's bind group and view (the bloom and the tone map take it from here).
    pub fn run(&mut self, cx: &mut PostCtx, src_bg: &wgpu::BindGroup, prev: &MotionPrev, level: u8) -> (&wgpu::BindGroup, &wgpu::TextureView) {
        let PostCtx { device, queue, layout, sampler, depth: DepthSrc { view: depth, multisampled }, size: (width, height), camera: cur, aspect, mirror, .. } = *cx;
        RtImage::ensure(&mut self.image, device, "motion blur image", layout, sampler, width, height, HDR_FORMAT, gfx::RT_USAGE);
        let (near, far) = (cur.near, cur.far);
        let u = BlurUniform {
            reproj: reprojection(cur, &prev.camera, aspect, mirror).to_cols_array_2d(),
            p: [prev.shutter, blur_max_len(level), far / (near - far), near],
            keep: prev.keep,
            q: [aspect, width as f32, height as f32, 0.0],
        };
        queue.write_buffer(&self.u_buf, 0, bytemuck::bytes_of(&u));
        let (pipe, dl) = if multisampled { (&self.pipe_ms, &self.depth_layout_ms) } else { (&self.pipe_1, &self.depth_layout_1) };
        let depth_bg = self.depth_bg.get((depth.clone(), multisampled), || gfx::view_bg(device, "motion blur depth", dl, depth));
        let RtImage { view, bg, .. } = self.image.as_ref().expect("blur image");
        {
            let mut pass = gfx::color_pass(cx.encoder, "motion blur", "motion blur", view, gfx::load_or_clear(false));
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, src_bg, &[]);
            pass.set_bind_group(1, depth_bg, &[]);
            pass.set_bind_group(2, &self.u_bg, &[]);
            pass.draw(0..3, 0..1);
        }
        (bg, view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn the_dirt_is_deterministic_and_sparse() {
        let a = generate_dirt(DIRT_SIZE.0, DIRT_SIZE.1);
        assert_eq!(a, generate_dirt(DIRT_SIZE.0, DIRT_SIZE.1));
        assert_eq!(a.len(), (DIRT_SIZE.0 * DIRT_SIZE.1) as usize);
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        // mostly clean glass with a few smudges and specks: neither empty nor a veil
        assert!(mean > 0.01 && mean < 0.25, "mean {mean}");
        assert!(a.iter().any(|&v| v > 150.0 / 255.0), "some spot is dirty");
        assert!(a.iter().filter(|&&v| v < 8.0 / 255.0).count() > a.len() / 3, "much of it is clean");
    }

    #[test]
    fn reprojecting_a_still_camera_is_the_identity() {
        let c = Camera { position: Vec3::new(1234.5, 3.0, -5678.9), target: Vec3::new(1234.5, 3.0, -5600.0), up: Vec3::Y, fov_y_radians: 0.9, near: 0.5, far: 700.0 };
        let m = reprojection(&c, &c, 1.6, false);
        for (a, b) in m.to_cols_array().iter().zip(Mat4::IDENTITY.to_cols_array()) {
            assert!((a - b).abs() < 1e-4, "{m:?}");
        }
    }

    #[test]
    fn a_point_straight_ahead_moves_with_the_camera() {
        // the camera moved 1 m forward: a point 11 m ahead is now 10 m ahead, so on the screen centre it stays,
        // and its depth (ndc z) maps to the previous frame's value
        let prev = Camera { position: Vec3::new(1000.0, 1.0, 1000.0), target: Vec3::new(1000.0, 1.0, 1010.0), up: Vec3::Y, fov_y_radians: 0.9, near: 0.5, far: 700.0 };
        let cur = Camera { position: Vec3::new(1000.0, 1.0, 1001.0), target: Vec3::new(1000.0, 1.0, 1011.0), ..prev };
        let m = reprojection(&cur, &prev, 1.6, false);
        let z_ndc = |d: f32| (cur.far / (cur.near - cur.far)) * (cur.near - d) / d;
        let q = m * glam::Vec4::new(0.0, 0.0, z_ndc(10.0), 1.0);
        assert!(q.x.abs() < 1e-4 && q.y.abs() < 1e-4, "{q:?}");
        assert!((q.z / q.w - z_ndc(11.0)).abs() < 1e-4, "{}", q.z / q.w);
        // an off-centre point streams outward as the camera advances: its previous position is nearer the centre
        let q = m * glam::Vec4::new(0.5, 0.0, z_ndc(10.0), 1.0);
        assert!(q.x / q.w < 0.5 && q.x / q.w > 0.0, "{q:?}");
    }

    #[test]
    fn the_soft_dirt_has_no_box_edge() {
        // ENH-SUNEDGE: the greasy blobs were cut by a square of 1 r: at its sides the mask jumped by exp(-3) x amp
        // (>= 0.006), a kink in the otherwise smooth soft layers (smudges, blobs) that ran straight for 2 r (80..220
        // pixels), which the dirt's gain round the HDR sun showed as a straight-sided glow. A jump is a large second
        // difference across the edge; none may run straight for 24 pixels
        let (w, h) = (DIRT_SIZE.0 as usize, DIRT_SIZE.1 as usize);
        let a = dirt_layers(DIRT_SIZE.0, DIRT_SIZE.1, false);
        let at = |x: usize, y: usize| a[y * w + x];
        let kink = |a0: f32, a1: f32, a2: f32, a3: f32| ((a2 - a1) - 0.5 * ((a1 - a0) + (a3 - a2))).abs() > 0.004;
        for x in 2..w - 1 {
            let mut run = 0;
            for y in 0..h {
                run = if kink(at(x - 2, y), at(x - 1, y), at(x, y), at(x + 1, y)) { run + 1 } else { 0 };
                assert!(run < 24, "a straight vertical edge at x {x}, y {y}");
            }
        }
        for y in 2..h - 1 {
            let mut run = 0;
            for x in 0..w {
                run = if kink(at(x, y - 2), at(x, y - 1), at(x, y), at(x, y + 1)) { run + 1 } else { 0 };
                assert!(run < 24, "a straight horizontal edge at x {x}, y {y}");
            }
        }
    }
}
