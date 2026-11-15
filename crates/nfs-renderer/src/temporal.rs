//! ENHANCEMENT ENH-TEMPORAL — the temporal foundation of the main race view: what a temporal upscaler
//! (FSR 3.1 through `wgpu-ffx`, MetalFX temporal) needs per frame at the INTERNAL (render) resolution.
//! Nothing here is on by default: the whole module is reached only when the renderer's temporal flag
//! ([`Core::temporal_flag`](crate::gpu), for the upscalers) or the debug env var `NFS_TEMPORAL_DEBUG` is set;
//! with both off the frame is master's, pixel for pixel. **Render-only**: nothing reaches the simulation,
//! `nfs-net`, the replay data or the camera the game uses.
//!
//! What it writes (the full conventions are in `RE/notes/ENHANCEMENTS.md`, "Temporal foundation"):
//!
//! * a **sub-pixel jitter** (Halton 2, 3, the phase count and offsets of `wgpu-ffx`'s `jitter.rs`, i.e.
//!   FSR's `ffxFsr3GetJitterPhaseCount` / `ffxFsr3GetJitterOffset`) applied to the main view's clip
//!   transform only: `view_proj' = T(2 jx / w, -2 jy / h) * view_proj`;
//! * **motion vectors** (`Rg16Float`, pixels at the render size, `previous - current`, y down: FSR's
//!   `motionVectorScale = 1`) from the UN-jittered current and previous view-projections, per object
//!   (a previous transform is found for every item, see [`Temporal::motion`]), the sky by the camera's
//!   rotation alone;
//! * the **reactive** mask and the **transparency-and-composition** mask (`R8Unorm`);
//! * the depth and the float scene the frame already has.
//!
//! `NFS_TEMPORAL_DEBUG=<view>` (`1` = just the targets and the jitter, `velocity`, `reactive`, `jitter`, `taa`,
//! `reproj`) also runs a debug pass over the float scene; `taa` is a minimal TAA resolve (history reprojected by the
//! vectors, Catmull-Rom, a 3 x 3 neighbourhood clamp) used only to prove the vectors; `reproj` shows the colour
//! difference between the frame and the previous one reprojected by the vectors (no blending, no clamp).
//! `NFS_TEMPORAL_NOOBJ=1` leaves the objects' own motion out (the camera's alone) and `NFS_TEMPORAL_NOMV=1` makes the debug pass read
//! a zero vector: controls, to show what a wrong vector does.

use crate::gfx::{self, Fullscreen, PostCtx};
use crate::wgsl;
use crate::camfx::{vp64, vp64_at};
use crate::{Camera, DrawItem};
use glam::{DMat4, Mat4};

/// The motion vectors' format (what `wgpu-ffx`'s `FsrDispatchInfo::motion_vectors` takes).
pub(crate) const MV_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;
/// The reactive / transparency-and-composition masks' format.
pub(crate) const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// What the debug pass shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    /// the targets are written and the jitter applied, the frame is shown as it is
    Plain,
    /// the motion vector as a colour (hue = direction, brightness = length)
    Velocity,
    /// the reactive mask (red) and the transparency-and-composition mask (green)
    Reactive,
    /// the jittered frame with a swatch of this frame's jitter
    Jitter,
    /// the built-in TAA resolve
    Taa,
    /// the difference between the frame and the previous raw frame reprojected by the vectors
    Reproj,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Config {
    pub view: View,
    /// the objects' own motion left out of the vectors (the camera's alone)
    pub no_obj: bool,
    /// the debug pass reads a zero vector everywhere (a control: what the history looks like with no reprojection)
    pub no_mv: bool,
    /// `NFS_TEMPORAL_NOID=1` (ENH-MOTIONID control): every item's `motion_id` is ignored, the (mesh, material, order)
    /// pairing alone finds the previous transform: what the vectors were before the ids
    pub no_id: bool,
}

impl Config {
    /// `NFS_TEMPORAL_DEBUG` (and `NFS_TEMPORAL_NOOBJ`); `None` = the foundation is off.
    pub fn from_env() -> Option<Self> {
        let f = crate::dbg::flags();
        let v = f.temporal_debug.as_deref()?;
        let view = match v.trim().to_ascii_lowercase().as_str() {
            "" | "0" | "off" => return None,
            "1" | "on" | "plain" => View::Plain,
            "velocity" | "mv" => View::Velocity,
            "reactive" => View::Reactive,
            "jitter" => View::Jitter,
            "taa" => View::Taa,
            "reproj" => View::Reproj,
            other => {
                tracing::warn!("NFS_TEMPORAL_DEBUG={other}: unknown view (1, velocity, reactive, jitter, taa, reproj), taking 1");
                View::Plain
            }
        };
        Some(Self { view, no_obj: f.temporal_noobj, no_mv: f.temporal_nomv, no_id: f.temporal_noid })
    }
}

// ---------------------------------------------------------------------------------------------
// the jitter (the same maths as `wgpu-ffx`'s `jitter.rs`)

fn halton(index: i32, base: i32) -> f32 {
    let mut f = 1.0f32;
    let mut result = 0.0f32;
    let mut i = index;
    while i > 0 {
        f /= base as f32;
        result += f * (i % base) as f32;
        i /= base;
    }
    result
}

/// FSR's recommended number of jitter phases for a render width and a display width: 8 at 1:1, growing with
/// the square of the ratio (32 at 2:1).
pub(crate) fn jitter_phase_count(render_width: u32, display_width: u32) -> i32 {
    ((8.0 * (display_width as f32 / render_width.max(1) as f32).powf(2.0)) as i32).max(8)
}

/// The sub-pixel offset of frame `index`, in pixels of the render size, each in `[-0.5, 0.5]`; the image is
/// shifted by `(+x, +y)` pixels on the screen (x right, y DOWN), FSR's convention.
pub(crate) fn jitter_offset(index: u32, phase_count: i32) -> [f32; 2] {
    let i = (index % phase_count.max(1) as u32) as i32 + 1;
    [halton(i, 2) - 0.5, halton(i, 3) - 0.5]
}

/// The clip-space translation of a jitter of `j` pixels at a render size: the NDC shift of the image
/// (`+x` right, `+y` UP in NDC, so a shift DOWN on the screen is a negative `y`).
pub(crate) fn jitter_ndc(j: [f32; 2], size: (u32, u32)) -> [f32; 2] {
    [2.0 * j[0] / size.0 as f32, -2.0 * j[1] / size.1 as f32]
}

/// `T(jitter) * view_proj`: the jittered clip transform.
pub(crate) fn jittered(vp: Mat4, ndc: [f32; 2]) -> Mat4 {
    Mat4::from_translation(glam::Vec3::new(ndc[0], ndc[1], 0.0)) * vp
}

// ---------------------------------------------------------------------------------------------
// the previous frame and the motion of the items

/// A camera that moved further, or turned more, than a car can in a frame between two frames of the main view
/// is a cut (a view change, a restart): no history.
const CUT_DISTANCE: f32 = 40.0;
const CUT_TURN: f32 = 0.5;
/// An item without an identical previous transform is the one of its (mesh, material) that was left unpaired
/// before it, when that was this close (metres): a car does not move further between two frames.
const PAIR_REACH: f32 = 12.0;

struct Prev {
    cam: Camera,
    aspect: f32,
    mirror: bool,
    size: (u32, u32),
    /// `(key, transform)` of the previous frame's items, sorted by key (the draw order kept within a key)
    items: Vec<(u64, [f32; 16])>,
    /// `(motion_id, transform)` of the previous frame's items that had an id, sorted by id
    ids: Vec<(u32, [f32; 16])>,
}

/// What [`Temporal::motion`] hands the object pass: the reprojection matrices (index 0 the static world's, 1 the
/// sky's, then one for each item that moved of its own) and the index of each item's.
#[derive(Default)]
pub(crate) struct Motion {
    pub mats: Vec<[f32; 16]>,
    pub index: Vec<u32>,
}

pub(crate) struct Temporal {
    pub cfg: Config,
    /// the jitter sequence's frame counter
    pub frame: u32,
    prev: Option<Prev>,
    cur_items: Vec<(u64, [f32; 16])>,
    /// this frame's items with a `motion_id`: `(id, transform)` and `(id, item index)`
    cur_ids: Vec<(u32, [f32; 16])>,
    id_keys: Vec<(u32, u32)>,
    keys: Vec<(u64, u32)>,
    claimed: Vec<bool>,
    pub targets: Option<Targets>,
    pub pass: Option<Pass>,
    /// the pending state of this frame, committed once the frame's motion is known
    pending: Option<(Camera, f32, bool, (u32, u32))>,
}

/// One frame's temporal parameters.
pub(crate) struct FrameInfo {
    /// the frame's number in the jitter sequence
    pub frame: u32,
    /// the jitter in render pixels (FSR's `jitter_offset`)
    pub jitter_px: [f32; 2],
    pub jitter_ndc: [f32; 2],
    pub phase_count: i32,
    /// the history is not valid (the first frame, a cut, a new size)
    pub reset: bool,
}

impl Temporal {
    pub fn new(cfg: Config) -> Self {
        Self { cfg, frame: 0, prev: None, cur_items: Vec::new(), cur_ids: Vec::new(), id_keys: Vec::new(), keys: Vec::new(), claimed: Vec::new(), targets: None, pass: None, pending: None }
    }

    /// ENHANCEMENT E37: a camera cut: the previous frame is forgotten (the next frame resets the history).
    pub fn cut(&mut self) {
        self.prev = None;
    }

    /// Start a main-view frame: the jitter of this frame and whether the history holds.
    pub fn begin(&mut self, camera: &Camera, aspect: f32, mirror: bool, size: (u32, u32), display_width: u32) -> FrameInfo {
        let reset = match &self.prev {
            None => true,
            Some(p) => p.size != size || (p.aspect - aspect).abs() > 1e-4 || p.mirror != mirror || {
                let fwd = |c: &Camera| (c.target - c.position).normalize_or_zero();
                (camera.position - p.cam.position).length() > CUT_DISTANCE || fwd(camera).dot(fwd(&p.cam)).clamp(-1.0, 1.0).acos() > CUT_TURN || (camera.fov_y_radians - p.cam.fov_y_radians).abs() > 1e-4
            },
        };
        if reset {
            self.prev = None;
        }
        let phase_count = jitter_phase_count(size.0, display_width);
        let jitter_px = jitter_offset(self.frame, phase_count);
        tracing::trace!(target: "temporal", frame = self.frame, phase_count, jx = jitter_px[0], jy = jitter_px[1], reset, px = camera.position.x, py = camera.position.y, pz = camera.position.z, tx = camera.target.x, ty = camera.target.y, tz = camera.target.z, "temporal frame");
        let frame = self.frame;
        self.frame = self.frame.wrapping_add(1);
        self.pending = Some((*camera, aspect, mirror, size));
        FrameInfo { frame, jitter_px, jitter_ndc: jitter_ndc(jitter_px, size), phase_count, reset }
    }

    /// The reprojection matrices of this frame's items — clip of this frame (UN-jittered) to clip of the previous
    /// one — and which item takes which. `sky[i]` marks the items of the no-depth stage (the sky pass: the camera's
    /// rotation alone moves them).
    ///
    /// A static item (the same transform, bit for bit, as an item of the same mesh and material last frame — and so
    /// every track item) takes the static world's matrix `VP_prev * VP_cur^-1`, built in f64. An item whose transform
    /// changed is paired with the previous frame's unpaired item of the same (mesh, material) in draw order (a car's
    /// four wheels share a mesh and keep their order; a part that left the view shifts the order, and the reach test
    /// [`PAIR_REACH`] keeps it from pairing with another car's) and gets `VP_prev * M_prev * M_cur^-1 * VP_cur^-1`: the
    /// motion of the object AND of the camera. An item with no partner (new, or rewritten every frame — a skinned /
    /// dynamic mesh such as the shadow patches and the skid strips draws with an identity transform and its vertices
    /// in the world, so it is the static case) has the camera's motion alone.
    pub fn motion(&mut self, items: &[DrawItem], sky: &[bool], cam: &Camera, aspect: f32, mirror: bool, out: &mut Motion) {
        let n = items.len();
        out.mats.clear();
        out.index.clear();
        out.index.resize(n, 0);
        let cur = vp64(cam, aspect, mirror);
        let inv_cur = cur.inverse();
        let (prev_vp, prev_rot_vp) = match &self.prev {
            Some(p) => (vp64(&p.cam, p.aspect, p.mirror), vp64_at(&p.cam, p.aspect, p.mirror, true)),
            None => (cur, vp64_at(cam, aspect, mirror, true)),
        };
        let f32s = |m: DMat4| m.to_cols_array().map(|v| v as f32);
        out.mats.push(f32s(prev_vp * inv_cur));
        out.mats.push(f32s(prev_rot_vp * vp64_at(cam, aspect, mirror, true).inverse()));
        // this frame's items by key (draw order kept within a key)
        self.keys.clear();
        self.cur_items.clear();
        self.cur_ids.clear();
        self.id_keys.clear();
        for (i, it) in items.iter().enumerate() {
            if sky.get(i).copied().unwrap_or(false) {
                out.index[i] = 1;
                continue;
            }
            // ENHANCEMENT E37: an item with an identity of its own is matched by it, not paired
            if it.motion_id != 0 && !self.cfg.no_id {
                self.id_keys.push((it.motion_id, i as u32));
                continue;
            }
            self.keys.push((key_of(it), i as u32));
        }
        self.keys.sort_unstable();
        self.id_keys.sort_unstable();
        tracing::trace!(target: "temporal", items = n, with_id = self.id_keys.len(), pieces = self.id_keys.iter().filter(|k| k.0 & 0xF000_0000 == crate::MOTION_PIECE).count(), movers = self.id_keys.iter().filter(|k| k.0 & 0xF000_0000 != crate::MOTION_CAR).count(), "temporal motion ids");
        let dmat = |m: &[f32; 16]| DMat4::from_cols_array(&m.map(f64::from));
        let have_prev = self.prev.is_some() && !self.cfg.no_obj;
        // ENHANCEMENT E37: the items with an id: the previous frame's transform of the same id, the same transform =
        // static (the camera's motion alone), else the object's own motion too (a partner further than `PAIR_REACH`
        // is a respawn / a cut of that object: the camera's alone)
        for &(id, i) in &self.id_keys {
            let m = items[i as usize].transform;
            self.cur_ids.push((id, m.to_cols_array()));
            if !have_prev {
                continue;
            }
            let prev = &self.prev.as_ref().expect("prev").ids;
            let Ok(k) = prev.binary_search_by_key(&id, |p| p.0) else { continue };
            let pm = prev[k].1;
            if pm == m.to_cols_array() {
                continue;
            }
            let (pt, ct) = (glam::Vec3::new(pm[12], pm[13], pm[14]), m.w_axis.truncate());
            if pt.distance(ct) > PAIR_REACH {
                continue;
            }
            let r = prev_vp * dmat(&pm) * dmat(&m.to_cols_array()).inverse() * inv_cur;
            out.index[i as usize] = out.mats.len() as u32;
            out.mats.push(f32s(r));
        }
        let mut j0 = 0usize;
        let (mut a, n_keys) = (0usize, self.keys.len());
        while a < n_keys {
            let key = self.keys[a].0;
            let mut b = a;
            while b < n_keys && self.keys[b].0 == key {
                b += 1;
            }
            for &(_, i) in &self.keys[a..b] {
                self.cur_items.push((key, items[i as usize].transform.to_cols_array()));
            }
            if have_prev {
                let prev = &self.prev.as_ref().expect("prev").items;
                while j0 < prev.len() && prev[j0].0 < key {
                    j0 += 1;
                }
                let mut j1 = j0;
                while j1 < prev.len() && prev[j1].0 == key {
                    j1 += 1;
                }
                if j1 > j0 {
                    let pg = &prev[j0..j1];
                    let cg = &self.keys[a..b];
                    self.claimed.clear();
                    self.claimed.resize(pg.len(), false);
                    // pass 1: the same transform as a previous item (the aligned partner first): static
                    let mut matched = vec![false; cg.len()];
                    for (ci, &(_, i)) in cg.iter().enumerate() {
                        let m = items[i as usize].transform.to_cols_array();
                        let hit = if ci < pg.len() && !self.claimed[ci] && pg[ci].1 == m { Some(ci) } else { (0..pg.len()).find(|&k| !self.claimed[k] && pg[k].1 == m) };
                        if let Some(k) = hit {
                            self.claimed[k] = true;
                            matched[ci] = true;
                        }
                    }
                    // pass 2: the rest, in draw order, against the previous frame's unpaired ones
                    let mut free = (0..pg.len()).filter(|&k| !self.claimed[k]);
                    for (ci, &(_, i)) in cg.iter().enumerate() {
                        if matched[ci] {
                            continue;
                        }
                        let Some(k) = free.next() else { break };
                        let m = items[i as usize].transform;
                        let pm = pg[k].1;
                        let (pt, ct) = (glam::Vec3::new(pm[12], pm[13], pm[14]), m.w_axis.truncate());
                        if pt.distance(ct) > PAIR_REACH {
                            continue;
                        }
                        let r = prev_vp * dmat(&pm) * dmat(&m.to_cols_array()).inverse() * inv_cur;
                        out.index[i as usize] = out.mats.len() as u32;
                        out.mats.push(f32s(r));
                    }
                }
            }
            a = b;
        }
        // `cur_items` was pushed in key order: it is next frame's `prev.items` (`commit`)
    }

    /// The frame's motion is done: this frame is the previous one of the next.
    pub fn commit(&mut self) {
        if let Some((cam, aspect, mirror, size)) = self.pending.take() {
            self.prev = Some(Prev { cam, aspect, mirror, size, items: std::mem::take(&mut self.cur_items), ids: std::mem::take(&mut self.cur_ids) });
        }
    }
}

fn key_of(it: &DrawItem) -> u64 {
    ((it.mesh.0 as u64) << 32) | it.material.0 as u64
}

// ---------------------------------------------------------------------------------------------
// the targets

/// The three images of a size the main passes write next to the scene. (The textures themselves are what the
/// upscalers of the next phase take, `wgpu-ffx`'s `FsrDispatchInfo` wants `wgpu::Texture`s.)
pub(crate) struct Targets {
    pub size: (u32, u32),
    pub mv: wgpu::Texture,
    pub mv_view: wgpu::TextureView,
    pub reactive: wgpu::Texture,
    pub reactive_view: wgpu::TextureView,
    pub tc: wgpu::Texture,
    pub tc_view: wgpu::TextureView,
}

impl Targets {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let make = |label, format| {
            let t = gfx::texture_2d(device, label, size.0, size.1, format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
            let v = t.create_view(&Default::default());
            (t, v)
        };
        let (mv, mv_view) = make("temporal motion vectors", MV_FORMAT);
        let (reactive, reactive_view) = make("temporal reactive mask", MASK_FORMAT);
        let (tc, tc_view) = make("temporal transparency and composition mask", MASK_FORMAT);
        Self { size, mv, mv_view, reactive, reactive_view, tc, tc_view }
    }
}

/// What `encode_frame` needs of the frame: the views to attach, the object bind group and the jitter.
#[derive(Clone)]
pub(crate) struct Cur {
    pub mv: wgpu::TextureView,
    pub reactive: wgpu::TextureView,
    pub tc: wgpu::TextureView,
    /// PERF (picture identical): the transparency-and-composition mask has a reader this frame (FSR 3.1, the debug pass);
    /// MetalFX's temporal scaler takes no such input, so for it the mask stays in tile memory (cleared, never stored)
    pub tc_read: bool,
    pub objects_bg: wgpu::BindGroup,
    pub jitter_ndc: [f32; 2],
    pub jitter_px: [f32; 2],
    pub phase_count: i32,
    pub size: (u32, u32),
    pub reset: bool,
    pub frame: u32,
}

/// What a temporal upscaler takes of the frame just drawn (phase B: `wgpu-ffx`'s `FsrDispatchInfo`, MetalFX temporal's
/// inputs), all at the render (internal) size. The textures are the frame's own (clones of the handles).
#[allow(dead_code)]
pub(crate) struct Inputs {
    /// the float scene (`Rgba16Float`), before the post chain and the tone map
    pub color: wgpu::Texture,
    /// `Depth32Float`, standard (0 near .. 1 far), finite: the planes below
    pub depth: wgpu::Texture,
    /// `Rg16Float`, pixels of the render size, previous - current, y down
    pub motion_vectors: wgpu::Texture,
    /// `R8Unorm`
    pub reactive_mask: wgpu::Texture,
    /// `R8Unorm`
    pub transparency_and_composition: wgpu::Texture,
    /// pixels, the image shifted by `(+x, +y)` (y down)
    pub jitter_offset: [f32; 2],
    pub render_size: [u32; 2],
    pub reset_history: bool,
    pub camera_near: f32,
    pub camera_far: f32,
    pub camera_fov_y: f32,
    /// the colour is not pre-exposed (the exposure is applied by the tone map after the upscale)
    pub pre_exposure: f32,
}

/// The GPU objects of the temporal object pass: the layout with the reprojection matrices, the pipeline layout
/// for the temporal pipeline set, the matrices' buffer and the object bind group.
pub(crate) struct Gpu {
    pub pipeline_layout: wgpu::PipelineLayout,
    pub motion_buf: wgpu::Buffer,
    pub objects_bg: wgpu::BindGroup,
}

impl Gpu {
    pub fn new(device: &wgpu::Device, globals: &wgpu::BindGroupLayout, material: &wgpu::BindGroupLayout, sky: &wgpu::BindGroupLayout, object_buf: &wgpu::Buffer, object_size: u64, max_objects: u64) -> Self {
        let entry = |binding, visibility, min: u64| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: wgpu::BufferSize::new(min) },
            count: None,
        };
        let layout = gfx::bgl(device, "object + motion", &[entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, object_size), entry(1, wgpu::ShaderStages::FRAGMENT, 64)]);
        let pipeline_layout = gfx::pipeline_layout(device, "mesh pipeline layout (temporal)", &[globals, &layout, material, sky]);
        let motion_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("motion matrices"), size: 64 * (max_objects + 2), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let objects_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("objects + motion"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: object_buf.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: motion_buf.as_entire_binding() }],
        });
        Self { pipeline_layout, motion_buf, objects_bg }
    }
}

// ---------------------------------------------------------------------------------------------
// the debug pass: the TAA resolve and the debug views

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PassUniform {
    /// width, height, 1 / width, 1 / height
    size: [f32; 4],
    /// the history's weight of the resolve's blend (the current frame's is 1 - it), 1 = no history this frame, the frame, 0
    a: [f32; 4],
    /// the jitter (pixels), the phase count, the frame's phase
    j: [f32; 4],
}

/// The debug pass's shader.
pub(crate) fn pass_source() -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS_UV, wgsl::ANGLES, PASS_WGSL])
}

struct Imgs {
    size: (u32, u32),
    /// the resolve's history, ping-pong
    tex: [wgpu::Texture; 2],
    view: [wgpu::TextureView; 2],
    /// what the later passes (bloom, tone map) take as the scene: a copy of the resolve, so their in-place adds
    /// never reach the history
    out: wgpu::Texture,
    out_view: wgpu::TextureView,
    out_bg: wgpu::BindGroup,
    /// the previous frame as drawn (the reprojection-error view's history)
    raw: wgpu::Texture,
    raw_view: wgpu::TextureView,
}

pub(crate) struct Pass {
    view: View,
    layout: wgpu::BindGroupLayout,
    pipe: wgpu::RenderPipeline,
    buf: wgpu::Buffer,
    imgs: Option<Imgs>,
    flip: usize,
    have_history: bool,
    no_mv: bool,
}

/// The history's share of the resolve (the current frame takes the rest): 0.9 is the common TAA feedback.
const TAA_FEEDBACK: f32 = 0.9;

const PASS_WGSL: &str = r#"
// 1 TAA resolve, 2 velocity, 3 reactive, 4 jitter, 5 reprojection error
override MODE: u32 = 1u;
struct P { size: vec4<f32>, a: vec4<f32>, j: vec4<f32> };
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var t_cur: texture_2d<f32>;
@group(0) @binding(2) var t_hist: texture_2d<f32>;
@group(0) @binding(3) var t_mv: texture_2d<f32>;
@group(0) @binding(4) var t_depth: texture_depth_2d;
@group(0) @binding(5) var t_re: texture_2d<f32>;
@group(0) @binding(6) var t_tc: texture_2d<f32>;
@group(0) @binding(7) var s: sampler;
// TEST GLUE (the debug views; not the original's):
// the Catmull-Rom weights' terms (`w1 = 1 + f^2 (-B + C f)`, `w2 = f (1/2 + f (2 - C f))`)
const CR_B: f32 = 2.5;
const CR_C: f32 = 1.5;
// the hue's wheel (`hsv`): the channels' offsets, six sectors, three of them to the peak
const HSV_K: vec3<f32> = vec3<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0);
const HSV_SECTORS: f32 = 6.0;
const HSV_HALF: f32 = 3.0;
// the velocity view's full brightness (px)
const MV_FULL_PX: f32 = 8.0;
// the jitter view: the swatch cell (px), its cross's grey, the background, the dot's radius (px) and colour, the phase bar's top
// (px) and colour
const SWATCH_PX: f32 = 96.0;
const SWATCH_BG: f32 = 0.08;
const SWATCH_CROSS: f32 = 0.3;
const DOT_R_PX: f32 = 4.0;
const DOT_COLOUR: vec3<f32> = vec3<f32>(1.0, 0.9, 0.1);
const BAR_TOP_PX: f32 = 90.0;
const BAR_COLOUR: vec3<f32> = vec3<f32>(0.2, 0.9, 0.3);
// the reprojection error view: off the history (blue), the error's gain
const OFF_HISTORY: vec4<f32> = vec4<f32>(0.0, 0.0, 0.6, 1.0);
const ERR_GAIN: f32 = 4.0;
// the TAA's neighbourhood clamp, widened by this much
const CLAMP_SLACK: f32 = 0.002;

fn clamp_px(q: vec2<i32>) -> vec2<i32> {
    return clamp(q, vec2<i32>(0), vec2<i32>(p.size.xy) - vec2<i32>(1));
}

// Catmull-Rom through five bilinear taps (Jimenez / Filmic SMAA t2x)
fn sample_hist(uv: vec2<f32>) -> vec3<f32> {
    let pos = uv * p.size.xy;
    let c = floor(pos - 0.5) + 0.5;
    let f = pos - c;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-CR_B + CR_C * f);
    let w2 = f * (0.5 + f * (2.0 - CR_C * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let o12 = w2 / w12;
    let t0 = (c - 1.0) * p.size.zw;
    let t3 = (c + 2.0) * p.size.zw;
    let t12 = (c + o12) * p.size.zw;
    var r = textureSampleLevel(t_hist, s, vec2<f32>(t12.x, t0.y), 0.0).rgb * (w12.x * w0.y);
    r += textureSampleLevel(t_hist, s, vec2<f32>(t0.x, t12.y), 0.0).rgb * (w0.x * w12.y);
    r += textureSampleLevel(t_hist, s, vec2<f32>(t12.x, t12.y), 0.0).rgb * (w12.x * w12.y);
    r += textureSampleLevel(t_hist, s, vec2<f32>(t3.x, t12.y), 0.0).rgb * (w3.x * w12.y);
    r += textureSampleLevel(t_hist, s, vec2<f32>(t12.x, t3.y), 0.0).rgb * (w12.x * w3.y);
    return max(r, vec3<f32>(0.0));
}

fn hsv(h: f32, v: f32) -> vec3<f32> {
    let k = HSV_K;
    let q = abs(fract(vec3<f32>(h) + k) * HSV_SECTORS - HSV_HALF);
    return v * clamp(q - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let px = vec2<i32>(in.pos.xy);
    let cur = textureLoad(t_cur, px, 0);
    let uv = (vec2<f32>(px) + 0.5) * p.size.zw;
    if MODE == 2u {
        // velocity: hue = the direction of the motion (previous - current, y down), brightness = its length (8 px = full)
        let mv = textureLoad(t_mv, px, 0).rg;
        let len = length(mv);
        let h = fract(atan2(mv.y, mv.x) / TAU + 1.0);
        return vec4<f32>(hsv(h, clamp(sqrt(len / MV_FULL_PX), 0.0, 1.0)), 1.0);
    }
    if MODE == 3u {
        // reactive (red), transparency and composition (green), the scene faintly under them
        let r = textureLoad(t_re, px, 0).r;
        let g = textureLoad(t_tc, px, 0).r;
        return vec4<f32>(cur.rgb * 0.25 + vec3<f32>(r, g, 0.0), 1.0);
    }
    if MODE == 4u {
        // the frame and a swatch of the jitter: a dot at (0.5 + jx, 0.5 + jy) of a 96 px cell, the frame's phase as a bar
        let q = vec2<f32>(px);
        if q.x < SWATCH_PX && q.y < SWATCH_PX {
            let d = length(q - (vec2<f32>(0.5) + p.j.xy) * SWATCH_PX);
            var c = vec3<f32>(SWATCH_BG);
            if abs(q.x - 0.5 * SWATCH_PX) < 0.5 || abs(q.y - 0.5 * SWATCH_PX) < 0.5 {
                c = vec3<f32>(SWATCH_CROSS);
            }
            if d < DOT_R_PX {
                c = DOT_COLOUR;
            }
            if q.y > BAR_TOP_PX && q.x < SWATCH_PX * (p.j.w + 1.0) / p.j.z {
                c = BAR_COLOUR;
            }
            return vec4<f32>(c, 1.0);
        }
        return cur;
    }
    // the nearest depth of the 3 x 3 around the pixel gives the vector (a moving thing's edge keeps its own motion)
    var best = 2.0;
    var bp = px;
    var mn = cur.rgb;
    var mx = cur.rgb;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let q = clamp_px(px + vec2<i32>(dx, dy));
            let d = textureLoad(t_depth, q, 0);
            if d < best {
                best = d;
                bp = q;
            }
            let c = textureLoad(t_cur, q, 0).rgb;
            mn = min(mn, c);
            mx = max(mx, c);
        }
    }
    var mv = textureLoad(t_mv, bp, 0).rg;
    if p.a.w > 0.5 {
        mv = vec2<f32>(0.0);
    }
    let huv = uv + mv * p.size.zw;
    let inside = all(huv > vec2<f32>(0.0)) && all(huv < vec2<f32>(1.0));
    if MODE == 5u {
        // the reprojection error: the frame against the previous raw frame fetched through the vector, no blending
        if p.a.y > 0.5 || !inside {
            return OFF_HISTORY;
        }
        let h = textureSampleLevel(t_hist, s, huv, 0.0).rgb;
        let e = abs(cur.rgb - h);
        return vec4<f32>(vec3<f32>(clamp(max(e.r, max(e.g, e.b)) * ERR_GAIN, 0.0, 1.0)), 1.0);
    }
    // TAA: the history through the vector, clamped to the neighbourhood of the frame, blended
    var w = 1.0 - p.a.x;
    if p.a.y > 0.5 || !inside {
        return cur;
    }
    let h = clamp(sample_hist(huv), mn - vec3<f32>(CLAMP_SLACK), mx + vec3<f32>(CLAMP_SLACK));
    // a reactive pixel (a particle) takes the frame
    w = max(w, textureLoad(t_re, px, 0).r);
    return vec4<f32>(mix(h, cur.rgb, w), 1.0);
}
"#;

fn tv(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(v) }
}

impl Pass {
    pub fn new(device: &wgpu::Device, view: View, no_mv: bool) -> Self {
        let tex = |binding, filterable| gfx::bgl_tex(binding, wgpu::TextureSampleType::Float { filterable }, false);
        let layout = gfx::bgl(
            device,
            "temporal pass",
            &[
                gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT),
                tex(1, false),
                tex(2, true),
                tex(3, false),
                gfx::bgl_tex(4, wgpu::TextureSampleType::Depth, false),
                tex(5, false),
                tex(6, false),
                gfx::bgl_sampler(7, wgpu::SamplerBindingType::Filtering),
            ],
        );
        let module = gfx::wgsl_module(device, "temporal pass", &pass_source());
        let pl = gfx::pipeline_layout(device, "temporal pass", &[&layout]);
        let mode = match view {
            View::Taa | View::Plain => 1.0,
            View::Velocity => 2.0,
            View::Reactive => 3.0,
            View::Jitter => 4.0,
            View::Reproj => 5.0,
        };
        let pipe = Fullscreen::new("temporal pass", &pl, &module).constants(&[("MODE", mode)]).build(device, &[gfx::target(crate::gpu::HDR_FORMAT, None)]);
        let buf = gfx::uniform_buf::<PassUniform>(device, "temporal pass");
        Self { view, layout, pipe, buf, imgs: None, flip: 0, have_history: false, no_mv }
    }

    /// Run the pass over the float scene `cur` and return the image later passes should take as the scene
    /// (`None` for [`View::Plain`], which leaves it alone).
    /// `depth`: the scene's (single-sample: a temporal main view renders at one sample).
    pub fn run(&mut self, cx: &mut PostCtx, t: &Cur, cur_tex: &wgpu::Texture, cur_view: &wgpu::TextureView, depth: &wgpu::TextureView) -> Option<(wgpu::BindGroup, wgpu::TextureView)> {
        let PostCtx { device, layout: image_layout, sampler, .. } = *cx;
        if self.view == View::Plain {
            return None;
        }
        if self.imgs.as_ref().is_none_or(|i| i.size != t.size) {
            let make = |label| {
                gfx::texture_2d(device, label, t.size.0, t.size.1, crate::gpu::HDR_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST)
            };
            let tex = [make("temporal history a"), make("temporal history b")];
            let view = [tex[0].create_view(&Default::default()), tex[1].create_view(&Default::default())];
            let out = make("temporal output");
            let out_view = out.create_view(&Default::default());
            let out_bg = gfx::tex_sampler_bg(device, "temporal image", image_layout, &out_view, sampler);
            let raw = make("temporal previous frame");
            let raw_view = raw.create_view(&Default::default());
            self.imgs = Some(Imgs { size: t.size, tex, view, out, out_view, out_bg, raw, raw_view });
            self.have_history = false;
        }
        let imgs = self.imgs.as_ref().expect("temporal images");
        let reset = t.reset || !self.have_history;
        let u = PassUniform {
            size: [t.size.0 as f32, t.size.1 as f32, 1.0 / t.size.0 as f32, 1.0 / t.size.1 as f32],
            a: [TAA_FEEDBACK, reset as u32 as f32, t.frame as f32, self.no_mv as u32 as f32],
            j: [t.jitter_px[0], t.jitter_px[1], t.phase_count as f32, (t.frame % t.phase_count.max(1) as u32) as f32],
        };
        gfx::belt_write(cx.belt, cx.encoder, &self.buf, bytemuck::bytes_of(&u));
        let encoder = &mut *cx.encoder;
        let dest = self.flip;
        let hist = match self.view {
            View::Taa => &imgs.view[1 - dest],
            View::Reproj => &imgs.raw_view,
            _ => cur_view,
        };
        let target = if self.view == View::Taa { &imgs.view[dest] } else { &imgs.out_view };
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("temporal pass"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.buf.as_entire_binding() }, tv(1, cur_view), tv(2, hist), tv(3, &t.mv), tv(4, depth), tv(5, &t.reactive), tv(6, &t.tc), wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(sampler) }],
        });
        gfx::fullscreen_pass(encoder, "temporal pass", "temporal pass", target, wgpu::LoadOp::Clear(wgpu::Color::BLACK), &self.pipe, &[&bg]);
        let extent = wgpu::Extent3d { width: t.size.0, height: t.size.1, depth_or_array_layers: 1 };
        match self.view {
            View::Reproj => encoder.copy_texture_to_texture(cur_tex.as_image_copy(), imgs.raw.as_image_copy(), extent),
            // the scene the later passes take is a copy of the resolve, so their adds (the bloom) stay out of the history
            View::Taa => encoder.copy_texture_to_texture(imgs.tex[dest].as_image_copy(), imgs.out.as_image_copy(), extent),
            _ => {}
        }
        let out = (imgs.out_bg.clone(), imgs.out_view.clone());
        if self.view == View::Reproj {
            self.have_history = true;
        }
        if self.view == View::Taa {
            self.have_history = true;
            self.flip = 1 - self.flip;
        }
        Some(out)
    }
}

/// `previous - current` of a world point, in pixels of the render size (y down), by the matrices alone: the
/// reference the GPU's vectors are tested against.
#[cfg(test)]
fn project_px(vp: DMat4, p: glam::DVec3, size: (u32, u32)) -> glam::DVec2 {
    let c = vp * p.extend(1.0);
    let n = glam::DVec2::new(c.x / c.w, c.y / c.w);
    glam::DVec2::new((n.x * 0.5 + 0.5) * size.0 as f64, (0.5 - n.y * 0.5) * size.1 as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{DVec3, Vec3};

    #[test]
    fn halton_is_the_ffx_sequence() {
        // the first Halton(2, 3) points less 0.5, as `wgpu-ffx`'s `get_jitter_offset` gives them
        let j: Vec<[f32; 2]> = (0..4).map(|i| jitter_offset(i, 8)).collect();
        assert_eq!(j[0], [0.0, 1.0 / 3.0 - 0.5]);
        assert_eq!(j[1], [0.25 - 0.5, 2.0 / 3.0 - 0.5]);
        assert!(j.iter().flatten().all(|v| v.abs() <= 0.5));
        // a sequence of `phase` frames, then it repeats
        assert_eq!(jitter_offset(0, 8), jitter_offset(8, 8));
        assert_eq!(jitter_phase_count(1280, 1280), 8);
        assert_eq!(jitter_phase_count(640, 1280), 32);
        assert!((8..=32).contains(&jitter_phase_count(1706, 2560)));
    }

    #[test]
    fn jitter_shifts_the_image_by_plus_x_plus_y_pixels_y_down() {
        let cam = Camera { position: Vec3::new(10.0, 2.0, -30.0), target: Vec3::new(10.0, 2.0, 0.0), ..Camera::default() };
        let size = (1600u32, 900u32);
        let vp = cam.view_projection(size.0 as f32 / size.1 as f32);
        let p = Vec3::new(11.0, 3.0, 0.0);
        let at = |m: Mat4| {
            let c = m * p.extend(1.0);
            [(c.x / c.w * 0.5 + 0.5) * size.0 as f32, (0.5 - c.y / c.w * 0.5) * size.1 as f32]
        };
        let (a, b) = (at(vp), at(jittered(vp, jitter_ndc([0.25, -0.375], size))));
        assert!((b[0] - a[0] - 0.25).abs() < 1e-3, "x {} -> {}", a[0], b[0]);
        assert!((b[1] - a[1] + 0.375).abs() < 1e-3, "y {} -> {}", a[1], b[1]);
    }

    fn item(mesh: u32, material: u32, m: Mat4) -> DrawItem {
        DrawItem::new(crate::MeshId(mesh), crate::MaterialId(material), m)
    }

    /// The reprojection of an object that moved: a point of it, taken from the current clip to the previous one, lands
    /// where the previous frame drew it; the same point of a static item moves by the camera alone.
    #[test]
    fn vectors_follow_the_object_and_the_camera() {
        let size = (1280u32, 800u32);
        let aspect = size.0 as f32 / size.1 as f32;
        let prev_cam = Camera { position: Vec3::new(100.0, 5.0, -50.0), target: Vec3::new(100.0, 4.0, -20.0), ..Camera::default() };
        let cur_cam = Camera { position: Vec3::new(100.0, 5.0, -48.0), target: Vec3::new(100.5, 4.0, -18.0), ..Camera::default() };
        let mut t = Temporal::new(Config { view: View::Plain, no_obj: false, no_mv: false, no_id: false });
        let mut m = Motion::default();
        let prev_car = Mat4::from_translation(Vec3::new(101.0, 0.0, -10.0));
        let cur_car = Mat4::from_translation(Vec3::new(101.0, 0.0, -9.0));
        let wall = Mat4::from_translation(Vec3::new(95.0, 0.0, 0.0));
        // frame 0 (no history), frame 1
        t.begin(&prev_cam, aspect, false, size, size.0);
        t.motion(&[item(1, 1, prev_car), item(2, 1, wall)], &[false, false], &prev_cam, aspect, false, &mut m);
        assert_eq!(m.index, vec![0, 0], "no history: the camera's motion alone");
        t.commit();
        let info = t.begin(&cur_cam, aspect, false, size, size.0);
        assert!(!info.reset);
        t.motion(&[item(1, 1, cur_car), item(2, 1, wall)], &[false, false], &cur_cam, aspect, false, &mut m);
        assert_eq!(m.index[1], 0, "the wall did not move: the static matrix");
        assert!(m.index[0] >= 2, "the car moved: its own matrix");
        let to_mat = |a: &[f32; 16]| DMat4::from_cols_array(&a.map(f64::from));
        // a point of the car's body, in object space
        let local = DVec3::new(0.3, 0.7, 1.1);
        let world_cur = (DMat4::from_cols_array(&cur_car.to_cols_array().map(f64::from)) * local.extend(1.0)).truncate();
        let world_prev = (DMat4::from_cols_array(&prev_car.to_cols_array().map(f64::from)) * local.extend(1.0)).truncate();
        let (vp_cur, vp_prev) = (vp64(&cur_cam, aspect, false), vp64(&prev_cam, aspect, false));
        let want = project_px(vp_prev, world_prev, size) - project_px(vp_cur, world_cur, size);
        let clip_cur = vp_cur * world_cur.extend(1.0);
        let prev_clip = to_mat(&m.mats[m.index[0] as usize]) * clip_cur;
        let n = glam::DVec2::new(prev_clip.x / prev_clip.w, prev_clip.y / prev_clip.w);
        let got_prev = glam::DVec2::new((n.x * 0.5 + 0.5) * size.0 as f64, (0.5 - n.y * 0.5) * size.1 as f64);
        let got = got_prev - project_px(vp_cur, world_cur, size);
        assert!((got - want).length() < 0.01, "got {got:?} want {want:?}");
        // the wall's point: the camera's motion alone
        let wp = DVec3::new(95.0, 2.0, 0.0);
        let want = project_px(vp_prev, wp, size) - project_px(vp_cur, wp, size);
        let pc = to_mat(&m.mats[0]) * (vp_cur * wp.extend(1.0));
        let n = glam::DVec2::new(pc.x / pc.w, pc.y / pc.w);
        let got = glam::DVec2::new((n.x * 0.5 + 0.5) * size.0 as f64, (0.5 - n.y * 0.5) * size.1 as f64) - project_px(vp_cur, wp, size);
        assert!((got - want).length() < 0.01, "got {got:?} want {want:?}");
        assert!(want.length() > 1.0, "the camera did move the wall's image");
    }

    #[test]
    fn wheels_of_one_mesh_keep_their_partners_and_a_cut_clears_the_history() {
        let size = (640u32, 400u32);
        let cam = Camera { position: Vec3::new(0.0, 2.0, -10.0), target: Vec3::new(0.0, 1.0, 0.0), ..Camera::default() };
        let mut t = Temporal::new(Config { view: View::Plain, no_obj: false, no_mv: false, no_id: false });
        let mut m = Motion::default();
        // four wheels, one mesh and material, moving forward 1.4 m a frame: the nearest previous wheel of the rear
        // ones is a front wheel (1.0 m away), the draw order pairs them right
        let wheels = |z: f32| [(-0.8, 1.3), (0.8, 1.3), (-0.8, -1.2), (0.8, -1.2)].map(|(x, dz)| item(7, 3, Mat4::from_translation(Vec3::new(x, 0.3, z + dz))));
        t.begin(&cam, 1.6, false, size, size.0);
        t.motion(&wheels(0.0), &[false; 4], &cam, 1.6, false, &mut m);
        t.commit();
        t.begin(&cam, 1.6, false, size, size.0);
        t.motion(&wheels(1.4), &[false; 4], &cam, 1.6, false, &mut m);
        assert_eq!(m.index, vec![2, 3, 4, 5], "each wheel has its own matrix");
        // the rear-left wheel's matrix maps its current position to its previous one, not a front wheel's
        let cur = vp64(&cam, 1.6, false);
        let p_cur = DVec3::new(-0.8, 0.3, 1.4 - 1.2);
        let c = DMat4::from_cols_array(&m.mats[4].map(f64::from)) * (cur * p_cur.extend(1.0));
        let back = cur.inverse() * c;
        let back = back.truncate() / back.w;
        assert!((back - DVec3::new(-0.8, 0.3, -1.2)).length() < 1e-3, "{back:?}");
        t.commit();
        // a cut: the camera jumps
        let far = Camera { position: Vec3::new(500.0, 2.0, -10.0), target: Vec3::new(500.0, 1.0, 0.0), ..cam };
        assert!(t.begin(&far, 1.6, false, size, size.0).reset);
    }

    /// ENHANCEMENT E37: an item with a `motion_id` finds its own previous transform whatever the frame's draw order or
    /// the other items of its mesh do (the rear wheel of a car whose front wheels were culled this frame); an id with no
    /// partner, or one that jumped over the reach, has the camera's motion alone; `assign_motion_ids` numbers a car's
    /// wheels (one mesh) by their order and never gives 0.
    #[test]
    fn motion_ids_pair_by_identity_not_by_order() {
        let size = (640u32, 400u32);
        let cam = Camera { position: Vec3::new(0.0, 2.0, -10.0), target: Vec3::new(0.0, 1.0, 0.0), ..Camera::default() };
        let mut t = Temporal::new(Config { view: View::Plain, no_obj: false, no_mv: false, no_id: false });
        let mut m = Motion::default();
        let wheels = |z: f32, keep: &[usize]| {
            let mut v: Vec<DrawItem> = [(-0.8, 1.3), (0.8, 1.3), (-0.8, -1.2), (0.8, -1.2)].map(|(x, dz)| item(7, 3, Mat4::from_translation(Vec3::new(x, 0.3, z + dz)))).to_vec();
            crate::assign_motion_ids(&mut v, 5);
            assert!(v.iter().all(|i| i.motion_id != 0));
            assert_eq!(v.iter().map(|i| i.motion_id).collect::<std::collections::BTreeSet<_>>().len(), 4, "four wheels, four ids");
            keep.iter().map(|&k| v[k]).collect::<Vec<_>>()
        };
        t.begin(&cam, 1.6, false, size, size.0);
        t.motion(&wheels(0.0, &[0, 1, 2, 3]), &[false; 4], &cam, 1.6, false, &mut m);
        t.commit();
        t.begin(&cam, 1.6, false, size, size.0);
        // the front wheels are gone this frame: the order pairing would give the rear-left wheel the front-left's partner
        let now = wheels(1.4, &[2, 3]);
        t.motion(&now, &[false; 2], &cam, 1.6, false, &mut m);
        assert!(m.index.iter().all(|&i| i >= 2) && m.index[0] != m.index[1], "both rear wheels have a matrix of their own");
        let cur = vp64(&cam, 1.6, false);
        let p_cur = DVec3::new(-0.8, 0.3, 1.4 - 1.2);
        let c = DMat4::from_cols_array(&m.mats[m.index[0] as usize].map(f64::from)) * (cur * p_cur.extend(1.0));
        let back = cur.inverse() * c;
        let back = back.truncate() / back.w;
        assert!((back - DVec3::new(-0.8, 0.3, -1.2)).length() < 1e-3, "{back:?}");
        t.commit();
        // a respawn (the same id 30 m away) has no partner
        t.begin(&cam, 1.6, false, size, size.0);
        t.motion(&wheels(31.4, &[2]), &[false], &cam, 1.6, false, &mut m);
        assert_eq!(m.index, vec![0]);
    }

    /// ENHANCEMENT E37 (ENH-MOTIONID): the ids of the moving track objects are exact and never meet: the kinds do not
    /// overlap, the numbers do not wrap, and what does not fit gets 0 (the pairing).
    #[test]
    fn track_motion_ids_are_exact_and_distinct() {
        use crate::{motion_article, motion_item, motion_piece, MOTION_ARTICLE, MOTION_CAR, MOTION_ITEM};
        let mut all = std::collections::BTreeSet::new();
        for k in [0usize, 1, 2, 4095, 100_000] {
            assert!(all.insert(motion_item(k)));
        }
        for (g, l, p) in [(0usize, 0usize, 0usize), (0, 0, 1), (0, 1, 0), (1, 0, 0), (4095, 7, 255)] {
            assert!(all.insert(motion_article(g, l, p)));
        }
        for (b, n) in [(0usize, 0usize), (0, 1), (1, 0), (65535, 4095)] {
            assert!(all.insert(motion_piece(b, n)));
        }
        assert!(all.iter().all(|&i| i != 0 && i & 0xF000_0000 != 0 && i & 0xF000_0000 != MOTION_CAR));
        assert_eq!((motion_item(7) & 0xF000_0000, motion_article(1, 1, 1) & 0xF000_0000), (MOTION_ITEM, MOTION_ARTICLE));
        assert_eq!((motion_article(4096, 0, 0), motion_article(0, 8, 0), motion_article(0, 0, 256), motion_piece(65536, 0), motion_piece(0, 4096)), (0, 0, 0, 0, 0));
        // a car's hash keeps to its own kind
        let mut v = vec![item(1, 1, Mat4::IDENTITY); 3];
        crate::assign_motion_ids(&mut v, 9);
        assert!(v.iter().all(|i| i.motion_id & 0xF000_0000 == MOTION_CAR));
    }

    /// ENHANCEMENT E37 (ENH-MOTIONID): two identical cones both flying (they cross, so the farthest-first order of the
    /// draw flips between two frames): with the ids each keeps its own partner; with the ids ignored (`NFS_TEMPORAL_NOID`,
    /// the (mesh, material, order) pairing) the flip hands each the other's previous place: the case the ids are for.
    #[test]
    fn two_flying_cones_swap_by_order_but_not_by_id() {
        let size = (640u32, 400u32);
        let cam = Camera { position: Vec3::new(0.0, 2.0, -10.0), target: Vec3::new(0.0, 1.0, 0.0), ..Camera::default() };
        let cone = |k: usize, at: Vec3| DrawItem { motion_id: crate::motion_item(k), ..item(7, 3, Mat4::from_translation(at)) };
        let cur = vp64(&cam, 1.6, false);
        // A goes +x, B goes -x, 0.4 m a frame each: they meet at frame 5
        let pos_a = |f: f32| Vec3::new(-1.0 + 0.4 * f, 0.0, 4.0);
        let pos_b = |f: f32| Vec3::new(1.0 - 0.4 * f, 0.0, 4.3);
        let errors = |no_id: bool| {
            let mut t = Temporal::new(Config { view: View::Plain, no_obj: false, no_mv: false, no_id });
            let mut m = Motion::default();
            let mut worst = 0.0f64;
            for frame in 0..11 {
                let f = frame as f32;
                t.begin(&cam, 1.6, false, size, size.0);
                // the order is by the x of the item: it flips when they cross
                let (a, b) = (cone(1, pos_a(f)), cone(2, pos_b(f)));
                let items = if pos_a(f).x <= pos_b(f).x { [a, b] } else { [b, a] };
                t.motion(&items, &[false; 2], &cam, 1.6, false, &mut m);
                if frame > 0 {
                    for (i, it) in items.iter().enumerate() {
                        let prev = if it.motion_id == crate::motion_item(1) { pos_a(f - 1.0) } else { pos_b(f - 1.0) };
                        let p = it.transform.w_axis.truncate();
                        let c = DMat4::from_cols_array(&m.mats[m.index[i] as usize].map(f64::from)) * (cur * p.as_dvec3().extend(1.0));
                        let b = cur.inverse() * c;
                        worst = worst.max((b.truncate() / b.w - prev.as_dvec3()).length());
                    }
                }
                t.commit();
            }
            worst
        };
        assert!(errors(false) < 1e-3, "with the ids every cone maps to its own previous place");
        assert!(errors(true) > 0.5, "without them the crossing hands a cone the other's place");
    }

    /// ENHANCEMENT E37 (ENH-MOTIONID): two identical cones (one mesh, one material) never swap: the one that is knocked
    /// finds its own previous transform while the other stands, whichever the draw order, and when the other is culled.
    #[test]
    fn two_identical_cones_never_swap() {
        let size = (640u32, 400u32);
        let cam = Camera { position: Vec3::new(0.0, 2.0, -10.0), target: Vec3::new(0.0, 1.0, 0.0), ..Camera::default() };
        let cone = |k: usize, at: Vec3| DrawItem { motion_id: crate::motion_item(k), ..item(7, 3, Mat4::from_translation(at)) };
        let stand = Vec3::new(-1.0, 0.0, 4.0);
        let fly = |f: f32| Vec3::new(1.0 + 0.5 * f, 0.4 * f, 4.0 + 0.8 * f);
        let cur = vp64(&cam, 1.6, false);
        // where a matrix of `m` sends the world point `p` (now): its place a frame ago
        let back_of = |m: &Motion, i: usize, p: Vec3| {
            let c = DMat4::from_cols_array(&m.mats[m.index[i] as usize].map(f64::from)) * (cur * p.as_dvec3().extend(1.0));
            let b = cur.inverse() * c;
            b.truncate() / b.w
        };
        let mut t = Temporal::new(Config { view: View::Plain, no_obj: false, no_mv: false, no_id: false });
        let mut m = Motion::default();
        for frame in 0..6 {
            t.begin(&cam, 1.6, false, size, size.0);
            // the draw order flips every other frame
            let (a, b) = (cone(10, stand), cone(11, fly(frame as f32)));
            let (items, ia, ib) = if frame % 2 == 0 { ([a, b], 0, 1) } else { ([b, a], 1, 0) };
            t.motion(&items, &[false; 2], &cam, 1.6, false, &mut m);
            if frame > 0 {
                assert_eq!(m.index[ia], 0, "frame {frame}: the standing cone has the static matrix");
                assert_ne!(m.index[ib], 0, "frame {frame}: the flying cone has a matrix of its own");
                assert!((back_of(&m, ib, fly(frame as f32)) - fly(frame as f32 - 1.0).as_dvec3()).length() < 1e-3, "frame {frame}: it is mapped to its own previous place");
            }
            t.commit();
        }
        // one of the two is not drawn this frame (culled): the other keeps its own partner
        let mut t = Temporal::new(Config { view: View::Plain, no_obj: false, no_mv: false, no_id: false });
        t.begin(&cam, 1.6, false, size, size.0);
        t.motion(&[cone(10, stand), cone(11, fly(0.0))], &[false; 2], &cam, 1.6, false, &mut m);
        t.commit();
        t.begin(&cam, 1.6, false, size, size.0);
        t.motion(&[cone(11, fly(1.0))], &[false], &cam, 1.6, false, &mut m);
        assert!((back_of(&m, 0, fly(1.0)) - fly(0.0).as_dvec3()).length() < 1e-3);
    }
}
