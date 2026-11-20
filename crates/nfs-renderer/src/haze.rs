//! ENHANCEMENT E36 — haze and headlight beams: a screen-space ray march through the air of the scene, added into
//! the float image before the tone map (the cheap variant of REMASTER-IDEAS 2c, no froxels).
//!
//! The air has two parts. (1) The **cones**: along every view ray (8 / 14 steps, a quadratic distribution so the
//! near part of a beam is sampled finely, a static per-pixel jitter) the light of E15's lit headlight spots
//! (`gpu/spot.rs`: the same cone, reach and falloff as on the surfaces, the camera car's pair also through E15's shadow
//! map, so a post or a car in the beam leaves a shadow in the air) scatters toward the eye with a forward-leaning phase
//! function, in proportion to the air's density. (2) The **tunnel lamps** (E23's classified lamp list, brought by the
//! app) light a faint warm haze round themselves, in a covered stretch (`TrackWorld::in_tunnel`, the held value E24
//! uses) and, at night, in fog or rain. The density is the track's own: the scene fog's density, the rain wetness
//! (the sky `.bin`'s track B field 5) and the covered flag; a clear dry open road gives none, and then the pass is not
//! run at all (the original's air is clean there).
//!
//! The march is done at quarter size ([`MARCH_DIV`]; the alpha channel keeps the distance to the scene), the composite
//! upsamples with a four-tap depth-aware (edge-aware) blur so a near silhouette (the own car, a post) does not take the haze
//! of what lies behind it, and adds over the full-size scene. Additive, `Rgba16Float`.

use crate::gfx::{self, Keyed, OverScene, PostTarget, RaysLabels, RaysPipes, UniformBuf, Variants};
use crate::wgsl::{self, DepthKind};
use glam::Vec3;

/// The most lamps the haze knows per frame.
pub const HAZE_LAMPS: usize = 16;

/// One lamp's glow in the air.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct HazeLamp {
    pub pos: Vec3,
    /// the lamp's colour with the brightness (and the fade) folded in
    pub color: [f32; 3],
    /// where its glow has faded to nothing, metres
    pub radius: f32,
}

/// ENHANCEMENT E36: what the air of the main race view holds (read only with E36 and E7 on).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Haze {
    /// the scene fog's density (1/m) at the camera, 0 = none
    pub fog: f32,
    /// the rain's wetness at the camera, 0..1 (the original's field, whether E26 is on or not)
    pub wet: f32,
    /// 0..1: how far inside a covered stretch the camera is (the held, ramped value)
    pub cover: f32,
    /// 0..1: how dark the sky is (1 night, near 0 a bright day): a headlight cone is not seen against daylight
    pub dark: f32,
    /// a day track (`TrackAssets::is_night` false): the covered stretch's beams are scaled by [`DAY_TUNNEL_AIR`] (a night
    /// track's by [`NIGHT_TUNNEL_AIR`])
    pub day: bool,
    pub n: u8,
    pub lamps: [HazeLamp; HAZE_LAMPS],
}

impl Default for Haze {
    fn default() -> Self {
        Self { fog: 0.0, wet: 0.0, cover: 0.0, dark: 1.0, day: false, n: 0, lamps: [HazeLamp::default(); HAZE_LAMPS] }
    }
}

/// The beams' and the lamps' glow's overall gain (before the level's strength).
const BEAM_GAIN: f32 = 4.5;
const LAMP_GAIN: f32 = 4.0;

/// ENHANCEMENT E36: the part of the beams' air density kept inside a covered stretch on a DAY track.
/// Inside a tunnel the beams' air was the scene fog + rain + the tunnel's own 0.015, all at full (night) weight
/// (`max(dark, cover)` = 1), and E15 / E24 light the player's pair at full strength there too; seen end-on from the
/// chase camera the whole beam (3..70 m) lies on a few hundred pixels, so a dry day tunnel got a clipped white wash ahead
/// of the car (alps 0.034 = fog 0.018 + tunnel 0.015, the cap; coastal 0.017). A dry road tunnel by day shows barely
/// visible beams; open roads (fog, rain) keep their density (night tunnels: [`NIGHT_TUNNEL_AIR`] since MONACO-TUNNEL). APPROXIMATION (by
/// the stills' luminance: swept 0.1 / 0.2 / 0.35, `RE/notes/ENHANCEMENTS.md` E36).
pub const DAY_TUNNEL_AIR: f32 = 0.15;

/// ENHANCEMENT E36 (MONACO-TUNNEL: the headlight hot spot was very dense and hid the road): the part of
/// the beams' air density kept inside a covered stretch on a NIGHT track. HAZE-TUNNEL left night tunnels at the whole density
/// (0.015): monaco1's harbour tunnel showed the same end-on wash over the bonnet as alps' by day (the patch ahead of the car, x 0.4-0.6,
/// y 0.42-0.58 of a 2560 x 1600 chase still: mean luma 84 with `NFS_ENH=off`, 158 and 21 % clipped by default; in the cockpit view
/// a white wall across the road). At 0.15 (the day value; 0.3 kept 6 % clipped, 0.05 hardly leaves a beam) the patch is 115 / 1.2 %
/// and the beam still reads as a faint pool and shaft ahead. APPROXIMATION (by the stills, `RE/notes/ENHANCEMENTS.md` E36).
pub const NIGHT_TUNNEL_AIR: f32 = 0.15;

/// The march's steps: the subtle level's and the medium level's (its gain above [`STRONG_GAIN`]).
pub(crate) const STEPS_SUBTLE: u32 = 10;
pub(crate) const STEPS_STRONG: u32 = 14;
const STRONG_GAIN: f32 = 0.8;

/// How far the march goes at the most, metres (the spot's reach is 70).
pub const MAX_DISTANCE: f32 = 72.0;

/// PERF (RENDER-AUDIT phase 3): the march's image is this fraction of the scene's size in each direction
/// (it was [`PostTarget::HALF`]): a sixteenth of the pixels march, with the same step count; the composite's depth-aware
/// upsample keeps the silhouettes. The haze is a smooth glow, so the picture changes only in its fine noise.
const MARCH_DIV: u32 = PostTarget::QUARTER;

impl Haze {
    /// The air's densities (1 / m): the beams' (`.0`) and the lamps' glow (`.1`). The beams' density is the track's
    /// fog, the rain and the covered stretch together, the beam seen against a dark sky only (inside a tunnel always);
    /// the lamps' glow is the tunnel's own air plus a part of the fog and the rain at night. Both 0 on a clear dry
    /// open road: nothing is drawn.
    pub fn air(&self, level: u8) -> (f32, f32) {
        // subtle: half the tunnel's own air (checked by eye: the tunnel was too strong)
        let tk = if level >= 2 { 1.0 } else { SUBTLE_TUNNEL };
        let fog = (self.fog * FOG_SCALE).min(FOG_MAX);
        let rain = RAIN_AIR * self.wet.clamp(0.0, 1.0);
        let tunnel = self.cover.clamp(0.0, 1.0) * tk;
        let beam = ((fog + rain + TUNNEL_BEAM_AIR * tunnel) * self.dark.max(self.cover.clamp(0.0, 1.0))).min(BEAM_AIR_MAX);
        // a covered stretch keeps only a part of that, ramped with the covered value: DAY_TUNNEL_AIR on a day track,
        // NIGHT_TUNNEL_AIR on a night one (MONACO-TUNNEL: the hot spot hid the road)
        let kept = if self.day { DAY_TUNNEL_AIR } else { NIGHT_TUNNEL_AIR };
        let beam = beam * (1.0 - self.cover.clamp(0.0, 1.0) * (1.0 - kept));
        let lamp = TUNNEL_LAMP_AIR * tunnel + 0.5 * (fog + rain) * self.dark;
        (beam, lamp)
    }
}

/// The level's strength: subtle 0.6, medium 1.
pub fn level_gain(level: u8) -> f32 {
    if level >= 2 { 1.0 } else { SUBTLE_GAIN }
}
const SUBTLE_GAIN: f32 = 0.6;

/// The air's densities (1 / m; tuned by eye, `RE/notes/ENHANCEMENTS.md` E36): the scene fog's share and cap, the rain's at full
/// wetness, the tunnel's own air for the beams and for the lamps' glow, the beams' cap; the subtle level's share of the tunnel.
const FOG_SCALE: f32 = 8.0;
const FOG_MAX: f32 = 0.03;
const RAIN_AIR: f32 = 0.02;
const TUNNEL_BEAM_AIR: f32 = 0.015;
const TUNNEL_LAMP_AIR: f32 = 0.010;
const BEAM_AIR_MAX: f32 = 0.035;
const SUBTLE_TUNNEL: f32 = 0.5;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    eye: gfx::EyeUniform,
    /// x the beams' density, y the lamps' density, z the march's end, w the lamp count
    air: [f32; 4],
    /// x the beams' gain, y the lamps' gain
    gain: [f32; 4],
    lamp_pos: [[f32; 4]; HAZE_LAMPS],
    lamp_col: [[f32; 4]; HAZE_LAMPS],
}

/// One frame's inputs of the pass.
pub(crate) struct Frame {
    pub inv_vp: glam::Mat4,
    pub eye: Vec3,
    pub fwd: Vec3,
    pub planes: [f32; 2],
    pub beam: f32,
    pub lamp: f32,
    pub gain: f32,
    pub haze: Haze,
}

const WGSL: &str = include_str!("haze.wgsl");

/// The pass's shader for a depth `kind` and `steps` march steps; the arrays' sizes are the Rust ones ([`HAZE_LAMPS`], E15's
/// `MAX_SPOTS`), written as the literals they were.
pub(crate) fn source(kind: DepthKind, steps: u32) -> String {
    let body = WGSL.replace("STEPS_N", &steps.to_string()).replace("HAZE_LAMPS_N", &HAZE_LAMPS.to_string()).replace("MAX_SPOTS_N", &crate::MAX_SPOTS.to_string()).replace("MARCH_DIV_N", &format!("{:?}", MARCH_DIV as f32));
    wgsl::compose_scene_depth(kind, &[wgsl::VIEW_DIR, &body])
}

type Pipes = RaysPipes;

const LABELS: RaysLabels = RaysLabels { rays_pass: "haze (march, quarter size)", rays_prof: "haze march (quarter)", comp_pass: "haze (composite)", comp_prof: "haze composite" };

/// What [`HazePass`]'s bind groups are made of: the pipelines' key, the depth, the rays image, E15's spot uniform and map.
type HazeBgKey = ((DepthKind, u32, u32), wgpu::TextureView, wgpu::TextureView, wgpu::Buffer, wgpu::TextureView);

/// The pipelines (one per depth kind and sample count of the target, built when first needed), the march's image and
/// the uniform.
pub(crate) struct HazePass {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    ubuf: UniformBuf<Uniform>,
    shadow_sampler: wgpu::Sampler,
    pipes: Variants<(DepthKind, u32, u32), Pipes>,
    /// the march's and the composite's bind groups, kept while the pipelines, the depth, the rays image and E15's spot
    /// uniform and map hold
    bgs: Keyed<HazeBgKey, (wgpu::BindGroup, wgpu::BindGroup)>,
    image: Option<PostTarget>,
}

impl HazePass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ubuf = UniformBuf::new(device, "haze");
        // the same comparison sampler E9's shadows use: hardware bilinear compare, clamped
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self { device: device.clone(), format, ubuf, shadow_sampler, pipes: Variants::default(), bgs: Keyed::default(), image: None }
    }

    /// `kind`: what the depth is; `samples`: the colour's sample count (1 when the scene is already resolved); `steps`
    /// the march's step count.
    fn build(device: &wgpu::Device, format: wgpu::TextureFormat, kind: DepthKind, samples: u32, steps: u32) -> Pipes {
        let module = gfx::wgsl_module(device, "haze", &source(kind, steps));
        let depth = kind.layout_entry();
        let uniform = |binding: u32| gfx::bgl_uniform(binding, wgpu::ShaderStages::FRAGMENT);
        let rays_tex = gfx::bgl_float(3, false);
        let spot_map = gfx::bgl_depth(7, false);
        let shadow_samp = gfx::bgl_sampler(8, wgpu::SamplerBindingType::Comparison);
        let rays_bgl = gfx::bgl(device, "haze rays", &[depth, uniform(2), uniform(6), spot_map, shadow_samp]);
        let comp_bgl = gfx::bgl(device, "haze composite", &[depth, uniform(2), rays_tex]);
        RaysPipes::build(device, &module, "haze", rays_bgl, comp_bgl, format, samples)
    }

    /// Record the pass into `encoder`: the march at 1 / [`MARCH_DIV`] size, then added over `colour` (loaded; `resolve` when it is
    /// multisampled; `keep` = store the multisampled samples for a later pass), reading `depth` and E15's spot uniform
    /// and shadow map.
    pub(crate) fn draw(&mut self, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, f: &Frame, at: &OverScene, spot_uniform: &wgpu::Buffer, spot_map: &wgpu::TextureView) {
        let OverScene { depth, kind, .. } = *at;
        let samples = at.samples.max(1);
        let steps = if f.gain > STRONG_GAIN { STEPS_STRONG } else { STEPS_SUBTLE };
        let key = (kind, samples, steps);
        let (device, format) = (&self.device, self.format);
        let pipes = self.pipes.get(key, || Self::build(device, format, kind, samples, steps));
        let size = (depth.texture().width(), depth.texture().height());
        let rays_view = &PostTarget::ensure(&mut self.image, device, "haze rays", size, MARCH_DIV, gfx::RAYS_FORMAT).view;
        let mut u = Uniform {
            eye: gfx::EyeUniform::new(&f.inv_vp, f.eye, f.fwd, f.planes),
            air: [f.beam, f.lamp, MAX_DISTANCE, (f.haze.n as usize).min(HAZE_LAMPS) as f32],
            gain: [f.gain * BEAM_GAIN, f.gain * LAMP_GAIN, 0.0, 0.0],
            lamp_pos: [[0.0; 4]; HAZE_LAMPS],
            lamp_col: [[0.0; 4]; HAZE_LAMPS],
        };
        for (i, l) in f.haze.lamps.iter().take(f.haze.n as usize).enumerate() {
            u.lamp_pos[i] = [l.pos.x, l.pos.y, l.pos.z, l.radius];
            u.lamp_col[i] = [l.color[0], l.color[1], l.color[2], 0.0];
        }
        self.ubuf.write(belt, encoder, &u);
        let (ubuf_res, shadow_sampler) = (&self.ubuf.buf, &self.shadow_sampler);
        let (bg1, bg2) = self.bgs.get((key, depth.clone(), rays_view.clone(), spot_uniform.clone(), spot_map.clone()), || {
            let depth_entry = gfx::bg_view(kind.binding(), depth);
            let ubuf = gfx::bg_buf(2, ubuf_res);
            let bg1 = gfx::bind_group(device, "haze rays", &pipes.rays_bgl, &[
                    depth_entry.clone(),
                    ubuf.clone(),
                    gfx::bg_buf(6, spot_uniform),
                    gfx::bg_view(7, spot_map),
                    gfx::bg_sampler(8, shadow_sampler),
                ]);
            let bg2 = gfx::bind_group(device, "haze composite", &pipes.comp_bgl, &[depth_entry, ubuf, gfx::bg_view(3, rays_view)]);
            (bg1, bg2)
        });
        pipes.record(encoder, LABELS, rays_view, at, bg1, bg2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clear_dry_open_road_has_no_air() {
        let h = Haze::default();
        assert_eq!(h.air(2), (0.0, 0.0));
    }

    #[test]
    fn fog_rain_and_cover_make_air_and_daylight_hides_the_beam() {
        let night_fog = Haze { fog: 0.0015, dark: 1.0, ..Haze::default() };
        let day_fog = Haze { fog: 0.0015, dark: 0.1, ..Haze::default() };
        assert!(night_fog.air(1).0 > 0.0 && night_fog.air(1).0 > day_fog.air(1).0 * 5.0);
        let rain = Haze { wet: 1.0, ..Haze::default() };
        assert!(rain.air(1).0 > 0.0 && rain.air(1).1 > 0.0);
        // inside a tunnel the beam shows even against a bright sky
        let tunnel = Haze { cover: 1.0, dark: 0.0, ..Haze::default() };
        assert!(tunnel.air(1).0 > 0.0 && tunnel.air(1).1 > 0.0);
        // the density is capped
        let thick = Haze { fog: 1.0, wet: 1.0, cover: 1.0, ..Haze::default() };
        assert!(thick.air(2).0 <= 0.035 + 1e-6);
    }

    #[test]
    fn a_tunnel_keeps_a_faint_beam_by_day_and_at_night() {
        // alps' tunnel by day: fog 0.00227, dark 0.6 (NFS_HAZE_LOG): the beam's air was at the cap, 0.0332
        let alps_day = Haze { fog: 0.00227, cover: 1.0, dark: 0.6, day: true, ..Haze::default() };
        assert!((alps_day.air(2).0 - 0.0332 * DAY_TUNNEL_AIR).abs() < 1e-3);
        // MONACO-TUNNEL: monaco1's night tunnel (no fog, dark 1): the tunnel's own 0.015, kept at NIGHT_TUNNEL_AIR
        let monaco = Haze { cover: 1.0, dark: 1.0, ..Haze::default() };
        assert!((monaco.air(2).0 - 0.015 * NIGHT_TUNNEL_AIR).abs() < 1e-6);
        // the lamps' glow is not these factors'
        assert_eq!(alps_day.air(2).1, Haze { day: false, ..alps_day }.air(2).1);
        assert!((monaco.air(2).1 - 0.010).abs() < 1e-6);
        // open roads (cover 0): unchanged by day and at night (fog, rain)
        let rain = Haze { fog: 0.001, wet: 1.0, dark: 0.78, ..Haze::default() };
        assert_eq!(rain.air(2), Haze { day: true, ..rain }.air(2));
        assert!((rain.air(2).0 - (0.008 + 0.02) * 0.78).abs() < 1e-6);
        // half way into the tunnel: half way between (the scale ramps with the covered value)
        let half = Haze { cover: 0.5, ..monaco };
        let unscaled = (0.015 * 0.5) * 1.0;
        assert!((half.air(2).0 - unscaled * (1.0 - 0.5 * (1.0 - NIGHT_TUNNEL_AIR))).abs() < 1e-6);
    }

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        // mat4 + 4 vec4 + 2 x 16 vec4
        assert_eq!(std::mem::size_of::<Uniform>(), 64 + 4 * 16 + 2 * 16 * 16);
    }
}
