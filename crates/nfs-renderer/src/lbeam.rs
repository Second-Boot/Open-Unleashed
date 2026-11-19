//! ENHANCEMENT E44 — the lighthouse beam as soft volumetric light.
//!
//! The original draws the coastal lighthouse's beam as two crossed translucent sheets (`BOX01` / `OBJECT01`, material
//! `lths`, texture alpha 12..116 of 255, `SRCALPHA` / `INVSRCALPHA`, a 401-frame rotation about the lamp): from outside a flat
//! box with straight borders and a flat cut-off end. With E44 and E7 on the sheets are not drawn and this pass paints the same
//! beam in the air instead: a full-screen fragment per view ray integrates the in-scattered light through a soft cone round
//! the beam's own axis (the sheets' apex = the lamp, their long axis, their length), with a Gaussian cross-section (a narrow
//! core and a wide faint halo), a falloff along the axis that ends smoothly beyond the sheets' length, a forward-leaning phase
//! function (brighter when the beam points at the eye) and the scene's depth (the beam stops at a hill or a car, so it is hidden
//! behind them). The result is laid over the float scene with the original's own blend (`SRCALPHA` / `INVSRCALPHA`: the beam
//! never adds more than its own colour), so it is as soft against a bright dusk sky as the original's texture was.
//!
//! The beam's TRANSFORM (apex, axis, rotation) is the original's: the app takes it from the animated article's matrix each frame
//! (`TrackWorld::lbeams`). Only the picture changes; nothing here reads or writes the simulation, a replay or the net.

use crate::gfx::{self, Fullscreen, Keyed, OverScene, UniformBuf, Variants};
use crate::wgsl::{self, DepthKind};
use glam::Vec3;

/// The most beams a track has (the coastal lighthouse: one, built of two crossed sheets).
pub const MAX_LBEAMS: usize = 2;

/// One beam, in world space for this frame.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LBeam {
    /// the lamp: the sheets' apex
    pub apex: Vec3,
    /// unit, the beam's long axis
    pub axis: Vec3,
    /// the sheets' length along the axis (m)
    pub length: f32,
    /// the core's standard deviation at the lamp (m)
    pub sigma0: f32,
    /// how fast it grows along the axis (m per m)
    pub spread: f32,
}

/// ENHANCEMENT E44: the beams of the main race view (read only with E44 and E7 on).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LBeams {
    pub n: u8,
    pub beams: [LBeam; MAX_LBEAMS],
    /// 0..1: how dark the sky is (the beam shows a touch stronger at dusk and at night)
    pub dark: f32,
}

/// How far past the sheets' length the beam reaches before it has faded to nothing, in lengths.
const REACH: f32 = 3.0;
/// The on-axis opacity seen from the side near the lamp (the texture's own peak 116 / 255 = 0.45, two sheets in series ~0.7; the
/// soft cone's opacity through its core is this number times the profile) and the dusk gain (1 + this x darkness).
const PEAK: f32 = 1.5;
const DUSK_GAIN: f32 = 0.5;

impl LBeams {
    /// The strength factor of this frame (dusk / night a touch stronger).
    pub fn strength(&self) -> f32 {
        1.0 + DUSK_GAIN * self.dark.clamp(0.0, 1.0)
    }
}

/// The halo's sigma in core sigmas (`profile`'s wide term) and the bounding cylinder's radius in halo sigmas (`beam_tau`'s `rb`).
const HALO: f32 = 2.6;
const BOUND_SIGMAS: f32 = 3.0;

/// Pixels of margin round the projected box of the beams (the shader rebuilds its rays from the inverse matrix; this side
/// projects with the matrix itself).
const SCISSOR_PAD: f32 = 4.0;

impl Frame {
    /// PERF (picture identical): the pixels whose view ray can meet a beam's bounding cylinder (`beam_tau`: outside it the optical
    /// depth is 0 and the fragment discards), as a scissor box `(x, y, w, h)` of a `width x height` target; `None` = the whole
    /// target (a box corner at or behind the eye's plane) and `Some(None)` = no pixel. Conservative: the cylinder's world box,
    /// its eight corners projected with the frame's matrix, padded by [`SCISSOR_PAD`].
    fn scissor(&self, width: u32, height: u32) -> Option<Option<[u32; 4]>> {
        let vp = self.inv_vp.inverse();
        let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
        for b in self.beams.beams.iter().take(self.beams.n as usize) {
            let reach = REACH * b.length;
            let rb = BOUND_SIGMAS * HALO * (b.sigma0 + b.spread * reach);
            // the cylinder's axis-aligned box: the two cap centres, widened per axis by the cap's extent rb * sqrt(1 - a_i^2)
            let a = b.axis;
            let ext = Vec3::new((1.0 - a.x * a.x).max(0.0).sqrt(), (1.0 - a.y * a.y).max(0.0).sqrt(), (1.0 - a.z * a.z).max(0.0).sqrt()) * rb;
            let (c0, c1) = (b.apex, b.apex + a * reach);
            let (bmin, bmax) = (c0.min(c1) - ext, c0.max(c1) + ext);
            for i in 0..8 {
                let p = Vec3::new(if i & 1 == 0 { bmin.x } else { bmax.x }, if i & 2 == 0 { bmin.y } else { bmax.y }, if i & 4 == 0 { bmin.z } else { bmax.z });
                let q = vp * p.extend(1.0);
                // a corner near or behind the eye: the box's projection is unbounded, draw everything
                if q.w <= self.planes[0] * 0.5 {
                    return None;
                }
                let ndc = glam::Vec2::new(q.x, q.y) / q.w;
                lo = lo.min(ndc);
                hi = hi.max(ndc);
            }
        }
        let (w, h) = (width as f32, height as f32);
        // NDC to pixels (y down), padded and clamped to the target
        let x0 = ((lo.x * 0.5 + 0.5) * w - SCISSOR_PAD).floor().clamp(0.0, w);
        let x1 = ((hi.x * 0.5 + 0.5) * w + SCISSOR_PAD).ceil().clamp(0.0, w);
        let y0 = ((0.5 - hi.y * 0.5) * h - SCISSOR_PAD).floor().clamp(0.0, h);
        let y1 = ((0.5 - lo.y * 0.5) * h + SCISSOR_PAD).ceil().clamp(0.0, h);
        Some((x1 > x0 && y1 > y0).then(|| [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]))
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    eye: gfx::EyeUniform,
    /// x the beam count, y the strength, z the opacity peak, w the reach in lengths
    air: [f32; 4],
    /// xyz the apex, w the length
    apex: [[f32; 4]; MAX_LBEAMS],
    /// xyz the axis, w the core sigma at the lamp
    axis: [[f32; 4]; MAX_LBEAMS],
    /// x the sigma's growth per metre
    shape: [[f32; 4]; MAX_LBEAMS],
}

/// One frame's inputs of the pass.
pub(crate) struct Frame {
    pub inv_vp: glam::Mat4,
    pub eye: Vec3,
    pub fwd: Vec3,
    pub planes: [f32; 2],
    pub beams: LBeams,
}

const WGSL: &str = include_str!("lbeam.wgsl");

/// The fragment's `tau` by depth kind: one depth value, the sample the fragment is for (`sample_index`), or the mean of the pixel's samples.
const TAU_1: &str = "tau = tau_at(depth_at(vec2<i32>(pos.xy)), dir, eye, jit);";
const TAU_SAMPLE: &str = "tau = tau_at(textureLoad(depth_ms, vec2<i32>(pos.xy), i32(si)), dir, eye, jit);";
const TAU_MEAN: &str = "let n = i32(textureNumSamples(depth_ms));
    var dmin = 1.0;
    var dmax = 0.0;
    for (var i = 0; i < n; i++) {
        let d = textureLoad(depth_ms, vec2<i32>(pos.xy), i);
        dmin = min(dmin, d);
        dmax = max(dmax, d);
    }
    if dmin == dmax {
        tau = tau_at(dmin, dir, eye, jit);
    } else {
        for (var i = 0; i < n; i++) {
            tau += tau_at(textureLoad(depth_ms, vec2<i32>(pos.xy), i), dir, eye, jit);
        }
        tau = tau / f32(n);
    }";

/// The pass's shader for a depth `kind` and a target of `samples` samples.
pub(crate) fn source(kind: DepthKind, samples: u32) -> String {
    let tau = match kind {
        DepthKind::Multi if samples > 1 => TAU_SAMPLE,
        DepthKind::Multi => TAU_MEAN,
        _ => TAU_1,
    };
    // the cylinder's numbers written as the literals they were (the text is the hand-written one's)
    let src = WGSL.replace("MAX_LBEAMS_N", &MAX_LBEAMS.to_string()).replace("HALO_N", &format!("{HALO:?}")).replace("BOUND_N", &format!("{BOUND_SIGMAS:?}")).replace("//TAU", tau).replace("//SAMPLE_ARG", if kind == DepthKind::Multi && samples > 1 { ", @builtin(sample_index) si: u32" } else { "" });
    wgsl::compose_scene_depth(kind, &[wgsl::VIEW_DIR, &src])
}

struct Pipes {
    pipe: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

/// The pipelines (one per depth kind and sample count of the target, built when first needed) and the uniform.
pub(crate) struct LBeamPass {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    ubuf: UniformBuf<Uniform>,
    pipes: Variants<(DepthKind, u32), Pipes>,
    /// the pass's bind group, kept while the pipelines and the depth hold
    bg: Keyed<((DepthKind, u32), wgpu::TextureView), wgpu::BindGroup>,
}

impl LBeamPass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ubuf = UniformBuf::new(device, "lbeam");
        Self { device: device.clone(), format, ubuf, pipes: Variants::default(), bg: Keyed::default() }
    }

    fn build(device: &wgpu::Device, format: wgpu::TextureFormat, kind: DepthKind, samples: u32) -> Pipes {
        let module = gfx::wgsl_module(device, "lbeam", &source(kind, samples));
        let depth = kind.layout_entry();
        let uniform = gfx::bgl_uniform(2, wgpu::ShaderStages::FRAGMENT);
        let bgl = gfx::bgl(device, "lbeam", &[depth, uniform]);
        // the original's blend (SRCALPHA / INVSRCALPHA, the colour premultiplied) on the colour; the target's alpha is left alone
        let pl = gfx::pipeline_layout(device, "lbeam", &[&bgl]);
        let pipe = Fullscreen::new("lbeam", &pl, &module).samples(samples).build(device, &[gfx::target(format, Some(gfx::blend::OVER_KEEP_ALPHA))]);
        Pipes { pipe, bgl }
    }

    /// Record the pass into `encoder`: over `colour` (loaded; `resolve` when it is multisampled; `keep` = store the multisampled
    /// samples for a later pass), reading `depth`.
    pub(crate) fn draw(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, f: &Frame, at: &OverScene) {
        let OverScene { depth, kind, colour, resolve, .. } = *at;
        let samples = at.samples.max(1);
        let key = (kind, samples);
        let (device, format) = (&self.device, self.format);
        let pipes = self.pipes.get(key, || Self::build(device, format, kind, samples));
        let mut u = Uniform {
            eye: gfx::EyeUniform::new(&f.inv_vp, f.eye, f.fwd, f.planes),
            air: [f.beams.n as f32, f.beams.strength(), PEAK, REACH],
            apex: [[0.0; 4]; MAX_LBEAMS],
            axis: [[0.0; 4]; MAX_LBEAMS],
            shape: [[0.0; 4]; MAX_LBEAMS],
        };
        for (i, b) in f.beams.beams.iter().take(f.beams.n as usize).enumerate() {
            u.apex[i] = [b.apex.x, b.apex.y, b.apex.z, b.length];
            u.axis[i] = [b.axis.x, b.axis.y, b.axis.z, b.sigma0];
            u.shape[i] = [b.spread, 0.0, 0.0, 0.0];
        }
        self.ubuf.write(belt, encoder, &u);
        let ubuf = &self.ubuf.buf;
        let bg = self.bg.get((key, depth.clone()), || {
            gfx::bind_group(device, "lbeam", &pipes.bgl, &[
                    gfx::bg_view(kind.binding(), depth),
                    gfx::bg_buf(2, ubuf),
                ])
        });
        let size = colour.texture().size();
        let scissor = f.scissor(size.width, size.height);
        let mut pass = gfx::color_pass_resolve(encoder, "lighthouse beam", "lighthouse beam", colour, resolve, wgpu::LoadOp::Load, at.store());
        match scissor {
            // no pixel can see a beam: the pass only loads, (resolves) and stores the colour, as one of discarded fragments did
            Some(None) => return,
            Some(Some([x, y, w, h])) => pass.set_scissor_rect(x, y, w, h),
            None => {}
        }
        gfx::draw_fullscreen(&mut pass, &pipes.pipe, &[bg]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        // mat4 + 3 vec4 + 3 x 2 vec4
        assert_eq!(std::mem::size_of::<Uniform>(), 64 + 3 * 16 + 3 * 2 * 16);
    }

    #[test]
    fn dusk_is_a_touch_stronger() {
        let day = LBeams { dark: 0.1, ..Default::default() };
        let dusk = LBeams { dark: 0.6, ..Default::default() };
        assert!(dusk.strength() > day.strength() && dusk.strength() <= 1.5);
    }
}
