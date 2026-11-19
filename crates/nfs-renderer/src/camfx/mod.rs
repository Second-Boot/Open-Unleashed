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

mod dirt;
pub(crate) use dirt::{DIRT_SIZE, generate_dirt};

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

pub(crate) use crate::camera::reprojection;

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

const BLUR_WGSL: &str = include_str!("blur.wgsl");

/// The pass's shader over a plain or a `multisampled` depth.
pub(crate) fn blur_source(multisampled: bool) -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::dtex(multisampled), wgsl::DLOAD, wgsl::PROJ_DIST, wgsl::NDC_OF_UV, BLUR_WGSL])
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
    use crate::camera::camera_still;
    use glam::{Mat4, Vec3};

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
}
