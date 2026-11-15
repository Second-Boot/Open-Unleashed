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
use crate::gfx::{self, DepthBinding, DepthSrc, Fullscreen, PostCtx, UniformBuf, Variants};
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
        1 => BLUR_LEN_LIGHT,
        _ => BLUR_LEN_MEDIUM,
    }
}
const BLUR_LEN_LIGHT: f32 = 0.015;
const BLUR_LEN_MEDIUM: f32 = 0.03;

/// How strongly the dirt lights where the bloom is: `bloom * (1 + DIRT_GAIN * dirt)`.
pub(crate) const DIRT_GAIN: f32 = 70.0;

// ---------------------------------------------------------------------------------------------
// E28: the dirt texture

/// Width and height of the generated dirt texture (2:1; it is stretched over the screen).
pub(crate) const DIRT_SIZE: (u32, u32) = (1024, 512);

/// The dirt's layers (tuned by eye). Smudges: fractal value noise over `SMUDGE_CELLS` lattice cells, `SMUDGE_OCTAVES` octaves
/// (seeds from `SMUDGE_SEED`), normalised by the octaves' amplitude sum, thresholded by a smoothstep over `SMUDGE_EDGE`, times
/// `SMUDGE_GAIN`.
const SMUDGE_CELLS: [f32; 2] = [4.0, 2.0];
const SMUDGE_OCTAVES: u32 = 4;
const SMUDGE_SEED: u32 = 11;
const SMUDGE_NORM: f32 = 0.9375;
const SMUDGE_EDGE: [f32; 2] = [0.42, 0.78];
const SMUDGE_GAIN: f32 = 0.75;
/// A speck's disc fades over `DISC_EDGE` of its radius; its rim rises over `RIM_IN` and falls over `RIM_OUT`.
const DISC_EDGE: [f32; 2] = [0.85, 1.0];
const RIM_IN: [f32; 2] = [0.55, 0.9];
const RIM_OUT: [f32; 2] = [0.9, 1.05];
/// Greasy blobs: the count, the radius `BLOB_R + r BLOB_R_SPAN` (texels), the amplitude, the Gaussian's falloff `exp(-d^2 k)`,
/// the reach in radii.
const BLOBS: usize = 14;
const BLOB_R: f32 = 40.0;
const BLOB_R_SPAN: f32 = 70.0;
const BLOB_AMP: f32 = 0.12;
const BLOB_AMP_SPAN: f32 = 0.2;
const BLOB_FALLOFF: f32 = 3.0;
const BLOB_REACH: f32 = 2.0;
/// Dust specks and dried droplets: the count, the radius `SPECK_R + SPECK_R_SPAN r^3`, the amplitude, the rim of one wider
/// than `RIM_FROM_R` texels.
const SPECKS: usize = 420;
const SPECK_R: f32 = 1.3;
const SPECK_R_SPAN: f32 = 7.0;
const SPECK_AMP: f32 = 0.25;
const SPECK_AMP_SPAN: f32 = 0.75;
const RIM_FROM_R: f32 = 3.5;
const SPECK_RIM: f32 = 0.6;
/// A few big dried droplets: the count, radius, amplitude, rim.
const BIG_DROPS: usize = 6;
const BIG_R: f32 = 14.0;
const BIG_R_SPAN: f32 = 12.0;
const BIG_AMP: f32 = 0.30;
const BIG_AMP_SPAN: f32 = 0.25;
const BIG_RIM: f32 = 0.8;
/// Wipe arcs: the count, the arc's radius, sweep (rad), line width (texels), amplitude.
const ARCS: usize = 5;
const ARC_R: f32 = 150.0;
const ARC_R_SPAN: f32 = 300.0;
const ARC_SWEEP: f32 = 0.4;
const ARC_SWEEP_SPAN: f32 = 0.7;
const ARC_WIDTH: f32 = 2.0;
const ARC_WIDTH_SPAN: f32 = 4.0;
const ARC_AMP: f32 = 0.15;
const ARC_AMP_SPAN: f32 = 0.2;
/// The rim of the lens collects more: the gain `EDGE_GAIN + EDGE_GAIN_SPAN * min(r^2 / 2, 1)` over the screen's radius `r`.
const EDGE_GAIN: f32 = 0.6;
const EDGE_GAIN_SPAN: f32 = 0.9;
/// The generator's seed, its step (the golden ratio's 32-bit constant) and `2^24` (a 24-bit sample to 0..1).
const DIRT_SEED: u32 = 0x1d17_c0de;
const RNG_STEP: u32 = 0x9e37_79b9;
const U24: f32 = 16_777_216.0;

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
        self.0 = hash(self.0.wrapping_add(RNG_STEP));
        (self.0 >> 8) as f32 / U24
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
    let h = |dx: i32, dy: i32| (hash(seed ^ hash((xi as i32 + dx) as u32).wrapping_add(hash((yi as i32 + dy) as u32).wrapping_mul(31))) >> 8) as f32 / U24;
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
    let mut rng = Rng(DIRT_SEED);
    // smudges: thresholded fractal noise
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32 * SMUDGE_CELLS[0], y as f32 / h as f32 * SMUDGE_CELLS[1]);
            let mut f = 0.0;
            let mut a = 0.5;
            let mut fr = 1.0;
            for o in 0..SMUDGE_OCTAVES {
                f += a * vnoise(u * fr, v * fr, SMUDGE_SEED + o);
                a *= 0.5;
                fr *= 2.0;
            }
            let m = smoothstep(SMUDGE_EDGE[0], SMUDGE_EDGE[1], f / SMUDGE_NORM);
            img[y * w + x] = SMUDGE_GAIN * m * m;
        }
    }
    let put = |img: &mut [f32], cx: f32, cy: f32, r: f32, amp: f32, rim: f32| {
        let (x0, x1) = (((cx - r - 1.0).floor().max(0.0)) as usize, ((cx + r + 1.0).ceil().min(w as f32 - 1.0)) as usize);
        let (y0, y1) = (((cy - r - 1.0).floor().max(0.0)) as usize, ((cy + r + 1.0).ceil().min(h as f32 - 1.0)) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                // a faint disc and a brighter rim just inside its edge
                let disc = (1.0 - smoothstep(DISC_EDGE[0], DISC_EDGE[1], d)) * (1.0 - rim);
                let ring = smoothstep(RIM_IN[0], RIM_IN[1], d) * (1.0 - smoothstep(RIM_OUT[0], RIM_OUT[1], d)) * rim;
                let v = amp * (disc + ring);
                let p = &mut img[y * w + x];
                *p = p.max(v);
            }
        }
    };
    // greasy blobs (large, faint, soft)
    for _ in 0..BLOBS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let r = BLOB_R + rng.next() * BLOB_R_SPAN;
        let amp = BLOB_AMP + rng.next() * BLOB_AMP_SPAN;
        // ENH-SUNEDGE: the blob is written over 2 r round its centre, where the Gaussian is already below half a byte
        // (exp(-12) x 0.32 x 1.5 x 255 < 0.001). It was cut at 1 r, a square box: at its sides the mask jumped by
        // exp(-3) x amp (up to 6 of 255), and the bloom's last add multiplies the mask by DIRT_GAIN (70) x the glow,
        // so round the HDR sun a blob near the screen's left / right edge drew a hard straight-sided square
        let reach = BLOB_REACH * r;
        let (x0, x1) = (((cx - reach).floor().max(0.0)) as usize, ((cx + reach).ceil().min(w as f32 - 1.0)) as usize);
        let (y0, y1) = (((cy - reach).floor().max(0.0)) as usize, ((cy + reach).ceil().min(h as f32 - 1.0)) as usize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r;
                let g = (-d * d * BLOB_FALLOFF).exp() * amp;
                let p = &mut img[y * w + x];
                *p = (*p + g).min(1.0);
            }
        }
    }
    if !all {
        return img;
    }
    // dust specks and dried droplets
    for _ in 0..SPECKS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let r = SPECK_R + SPECK_R_SPAN * rng.next().powi(3);
        let amp = SPECK_AMP + SPECK_AMP_SPAN * rng.next();
        let rim = if r > RIM_FROM_R { SPECK_RIM } else { 0.0 };
        put(&mut img, cx, cy, r, amp, rim);
    }
    // a few big dried droplets
    for _ in 0..BIG_DROPS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        put(&mut img, cx, cy, BIG_R + rng.next() * BIG_R_SPAN, BIG_AMP + rng.next() * BIG_AMP_SPAN, BIG_RIM);
    }
    // wipe arcs: soft curved lines
    for _ in 0..ARCS {
        let (cx, cy) = (rng.next() * w as f32, rng.next() * h as f32);
        let rad = ARC_R + rng.next() * ARC_R_SPAN;
        let a0 = rng.next() * std::f32::consts::TAU;
        let sweep = ARC_SWEEP + rng.next() * ARC_SWEEP_SPAN;
        let width_px = ARC_WIDTH + rng.next() * ARC_WIDTH_SPAN;
        let amp = ARC_AMP + rng.next() * ARC_AMP_SPAN;
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
            let e = EDGE_GAIN + EDGE_GAIN_SPAN * ((u * u + v * v) * 0.5).min(1.0);
            let p = &mut img[y * w + x];
            *p = (*p * e).clamp(0.0, 1.0);
        }
    }
    img
}

/// The dirt texture on the GPU, which the bloom's last add (in the tone map, ENHANCEMENT E10's `bloom::TONE_WGSL`) samples.
pub(crate) struct Dirt {
    /// (the view keeps the texture alive)
    pub view: wgpu::TextureView,
}

impl Dirt {
    /// PERF: the mask's generation (tens of ms of CPU) on a thread of its own, started before the dirt is first needed;
    /// [`Self::new`] takes its result.
    pub fn generate() -> std::thread::JoinHandle<Vec<f32>> {
        std::thread::spawn(|| generate_dirt(DIRT_SIZE.0, DIRT_SIZE.1))
    }

    /// The texture of `mask` ([`generate_dirt`]'s, `DIRT_SIZE`).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, mask: &[f32]) -> Self {
        let (w, h) = DIRT_SIZE;
        // ENH-SUNEDGE: half floats, not bytes. The bloom's last add multiplies the mask by DIRT_GAIN x INTENSITY
        // (17.5) x the wide glow, which round the HDR sun is several times white: one byte step (1/255) of a
        // smudge's or blob's soft tail was a visible hard contour there (on an EDR screen nothing clips it).
        // `R16Float` is filterable on every backend (core WebGPU).
        let texture = gfx::texture_2d(device, "lens dirt", w, h, wgpu::TextureFormat::R16Float, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &mask.iter().flat_map(|&v| crate::gpu::f32_to_f16(v).to_le_bytes()).collect::<Vec<u8>>(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 2), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&Default::default());
        Self { view }
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

/// PERF: the largest difference from the identity of a reprojection that cannot move any pixel. The shader returns the
/// pixel's own colour when the uv motion's length, `|(ndc - reprojected ndc) / 2|` (x by the aspect) times the shutter (<= 1),
/// is under 1e-5; an error of `e` in each matrix entry moves a reprojected NDC point (|x|, |y|, |z| <= 1, w = 1) by at most
/// about `8 e`, so 1e-7 keeps the length below 1e-5 at any aspect up to 10 with room for the shader's own f32 rounding.
const STILL_REPROJECTION: f32 = 1e-7;

/// PERF: the blur has nothing to do this frame: the camera has not moved since `prev` (the reprojection is the identity
/// within [`STILL_REPROJECTION`]: paused, the photo mode, a standstill). Every pixel would come out as the pass's input,
/// so the chain skips the pass and the passes after it take the input itself.
pub(crate) fn camera_still(cur: &Camera, prev: &Camera, aspect: f32, mirror: bool) -> bool {
    let m = reprojection(cur, prev, aspect, mirror).to_cols_array();
    m.iter().zip(Mat4::IDENTITY.to_cols_array()).all(|(a, b)| (a - b).abs() < STILL_REPROJECTION)
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
// (the blur's tuned values, ENHANCEMENT E27; not the original's)
// the player's car ellipse's soft edge (its radii's share), the cockpit / hood's depth ramp (m: nearer moves with the camera)
const KEEP_EDGE: vec2<f32> = vec2<f32>(0.85, 1.15);
const COCKPIT_DEPTH: vec2<f32> = vec2<f32>(1.5, 3.5);
// the least clip w of a reprojected pixel, the least move (uv) and streak (pixels) worth a blur
const MIN_W: f32 = 0.0001;
const MIN_MOVE: f32 = 0.00001;
const MIN_STREAK_PX: f32 = 0.6;
// the taps along the streak
const TAPS: i32 = 12;
// a tap's weight by its depth over the pixel's (nearer does not smear over it), the least pixel depth of the ratio (m)
const TAP_DEPTH: vec2<f32> = vec2<f32>(0.55, 0.95);
const MIN_DEPTH: f32 = 0.05;
// the distance in front of the camera of a depth-buffer value (the projection's own inverse)
fn lin(z: f32) -> f32 {
    return proj_dist(z, u.p.z, u.p.w);
}
// 0 inside the player's car, 1 outside (a soft edge)
fn keep_w(uv: vec2<f32>) -> f32 {
    if u.keep.z <= 0.0 {
        return 1.0;
    }
    return smoothstep(KEEP_EDGE.x, KEEP_EDGE.y, length((uv - u.keep.xy) / u.keep.zw));
}
@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let c = textureSampleLevel(t, s, uv, 0.0);
    let dims = u.q.yz;
    let z = dload(vec2<i32>(in.pos.xy), vec2<i32>(dims));
    let dl = lin(z);
    // nearer than 1.5..3.5 m is the cockpit / hood, which moves with the camera
    let k = smoothstep(COCKPIT_DEPTH.x, COCKPIT_DEPTH.y, dl) * keep_w(uv);
    if k <= 0.0 {
        return c;
    }
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    let q = u.reproj * ndc;
    if q.w < MIN_W {
        return c;
    }
    let pn = q.xy / q.w;
    // where the pixel came from, as a move of the uv
    let v = vec2<f32>(ndc.x - pn.x, -(ndc.y - pn.y)) * 0.5;
    let len = length(vec2<f32>(v.x * u.q.x, v.y)) * u.p.x * k;
    if len < MIN_MOVE {
        return c;
    }
    let ve = v * (u.p.x * k * min(1.0, u.p.y / len));
    if length(ve * dims) < MIN_STREAK_PX {
        return c;
    }
    var sum = c.rgb;
    var wsum = 1.0;
    for (var i = 0; i < TAPS; i = i + 1) {
        let tt = (f32(i) + 0.5) / f32(TAPS) - 0.5;
        let suv = uv + ve * tt;
        let sc = clamp(vec2<i32>(suv * dims), vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
        let dt = lin(dload(sc, vec2<i32>(dims)));
        // a tap nearer than the pixel does not smear over it; one inside the car is not taken
        let w = smoothstep(TAP_DEPTH.x, TAP_DEPTH.y, dt / max(dl, MIN_DEPTH)) * keep_w(suv);
        sum = sum + textureSampleLevel(t, s, suv, 0.0).rgb * w;
        wsum = wsum + w;
    }
    return vec4<f32>(sum / wsum, c.a);
}
"#;

/// The pass's shader over a plain or a `multisampled` depth.
pub(crate) fn blur_source(multisampled: bool) -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::dtex(multisampled), wgsl::DLOAD, wgsl::PROJ_DIST, BLUR_WGSL])
}

pub(crate) struct MotionBlur {
    /// the pipeline per depth kind (`true` = multisampled), built at its first use
    pipes: Variants<bool, wgpu::RenderPipeline>,
    image_layout: wgpu::BindGroupLayout,
    depth: DepthBinding,
    u_layout: wgpu::BindGroupLayout,
    u_buf: UniformBuf<BlurUniform>,
    u_bg: wgpu::BindGroup,
}

impl MotionBlur {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let u_layout = gfx::bgl(device, "motion blur", &[gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT)]);
        let u_buf = UniformBuf::new(device, "motion blur");
        let u_bg = gfx::buffer_bg(device, "motion blur", &u_layout, &u_buf.buf);
        Self { pipes: Variants::default(), image_layout: layout.clone(), depth: DepthBinding::new(device, "motion blur depth"), u_layout, u_buf, u_bg }
    }

    /// Blur `src` (`cx.size`, its bind group `src_bg`) by the camera's move since `prev`, reading `cx.depth`, into `out` (the
    /// chain's image of `cx.size`; the bloom and the tone map take it from here).
    pub fn run(&mut self, cx: &mut PostCtx, src_bg: &wgpu::BindGroup, prev: &MotionPrev, level: u8, out: &wgpu::TextureView) {
        let PostCtx { device, depth: DepthSrc { view: depth, multisampled }, size: (width, height), camera: cur, aspect, mirror, .. } = *cx;
        let (near, far) = (cur.near, cur.far);
        let u = BlurUniform {
            reproj: reprojection(cur, &prev.camera, aspect, mirror).to_cols_array_2d(),
            p: [prev.shutter, blur_max_len(level), far / (near - far), near],
            keep: prev.keep,
            q: [aspect, width as f32, height as f32, 0.0],
        };
        self.u_buf.write(cx.belt, cx.encoder, &u);
        let (image_layout, depth_layout, u_layout) = (&self.image_layout, self.depth.layout(multisampled), &self.u_layout);
        let pipe = self.pipes.get(multisampled, || {
            let module = gfx::wgsl_module(device, "motion blur", &blur_source(multisampled));
            let pl = gfx::pipeline_layout(device, "motion blur", &[image_layout, depth_layout, u_layout]);
            Fullscreen::new("motion blur", &pl, &module).build(device, &[gfx::target(HDR_FORMAT, None)])
        });
        let depth_bg = self.depth.bind(device, DepthSrc { view: depth, multisampled });
        gfx::fullscreen_pass(cx.encoder, "motion blur", "motion blur", out, gfx::load_or_clear(false), pipe, &[src_bg, depth_bg, &self.u_bg]);
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
        // a camera that has not moved is still (the blur is skipped), one that moved a centimetre is not
        assert!(camera_still(&c, &c, 1.6, false));
        let moved = Camera { position: c.position + Vec3::new(0.01, 0.0, 0.0), ..c };
        assert!(!camera_still(&moved, &c, 1.6, false));
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
