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
//! The march is done at half size (the alpha channel keeps the distance to the scene), the composite upsamples
//! with a four-tap depth-aware blur so a near silhouette (the own car, a post) does not take the haze of what lies
//! behind it, and adds over the full-size scene. Additive, `Rgba16Float`.

use crate::gfx::{self, Fullscreen, Keyed, OverScene};
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
    /// a day track (`TrackAssets::is_night` false): the covered stretch's beams are scaled by [`DAY_TUNNEL_AIR`]
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
/// visible beams; night tracks (monaco1's tunnel) and open roads (fog, rain) keep their density. APPROXIMATION (by
/// the stills' luminance: swept 0.1 / 0.2 / 0.35, `RE/notes/ENHANCEMENTS.md` E36).
pub const DAY_TUNNEL_AIR: f32 = 0.15;

/// How far the march goes at the most, metres (the spot's reach is 70).
pub const MAX_DISTANCE: f32 = 72.0;

impl Haze {
    /// The air's densities (1 / m): the beams' (`.0`) and the lamps' glow (`.1`). The beams' density is the track's
    /// fog, the rain and the covered stretch together, the beam seen against a dark sky only (inside a tunnel always);
    /// the lamps' glow is the tunnel's own air plus a part of the fog and the rain at night. Both 0 on a clear dry
    /// open road: nothing is drawn.
    pub fn air(&self, level: u8) -> (f32, f32) {
        // subtle: half the tunnel's own air (checked by eye: the tunnel was too strong)
        let tk = if level >= 2 { 1.0 } else { 0.5 };
        let fog = (self.fog * 8.0).min(0.03);
        let rain = 0.02 * self.wet.clamp(0.0, 1.0);
        let tunnel = self.cover.clamp(0.0, 1.0) * tk;
        let beam = ((fog + rain + 0.015 * tunnel) * self.dark.max(self.cover.clamp(0.0, 1.0))).min(0.035);
        // a day track's covered stretch keeps only a part of that (DAY_TUNNEL_AIR), ramped with the covered value
        let beam = if self.day { beam * (1.0 - self.cover.clamp(0.0, 1.0) * (1.0 - DAY_TUNNEL_AIR)) } else { beam };
        let lamp = 0.010 * tunnel + 0.5 * (fog + rain) * self.dark;
        (beam, lamp)
    }
}

/// The level's strength: subtle 0.6, medium 1.
pub fn level_gain(level: u8) -> f32 {
    if level >= 2 { 1.0 } else { 0.6 }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    inv_vp: [[f32; 4]; 4],
    /// xyz the eye, w the near plane
    cam: [f32; 4],
    /// xyz the view's forward axis, w the far plane
    fwd: [f32; 4],
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

const WGSL: &str = r#"
struct U {
    inv_vp: mat4x4<f32>,
    cam: vec4<f32>,
    fwd: vec4<f32>,
    air: vec4<f32>,
    gain: vec4<f32>,
    lamp_pos: array<vec4<f32>, 16>,
    lamp_col: array<vec4<f32>, 16>,
};
struct Spots {
    pos: array<vec4<f32>, 8>,
    dir: array<vec4<f32>, 8>,
    params: vec4<f32>,
    shadow: vec4<f32>,
    mat: mat4x4<f32>,
    origin: vec4<f32>,
};
@group(0) @binding(2) var<uniform> u: U;
@group(0) @binding(3) var rays: texture_2d<f32>;
@group(0) @binding(6) var<uniform> spots: Spots;
@group(0) @binding(7) var t_spot: texture_depth_2d;
@group(0) @binding(8) var s_shadow: sampler_comparison;

const STEPS: i32 = STEPS_N;
const CONE_COLOUR: vec3<f32> = vec3<f32>(1.0, 0.9, 0.7);

// the distance along the view ray to the scene, from the stored depth (the sky: a long way)
fn scene_dist(d: f32, dir: vec3<f32>) -> f32 {
    let near = u.cam.w;
    let far = u.fwd.w;
    if d >= 0.99999 {
        return 1000.0;
    }
    let z = near * far / max(far - d * (far - near), 0.0001);
    return z / max(dot(dir, u.fwd.xyz), 0.05);
}

fn view_dir(uv: vec2<f32>) -> vec3<f32> {
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let pn = u.inv_vp * vec4<f32>(ndc, 0.0, 1.0);
    return normalize(pn.xyz / pn.w - u.cam.xyz);
}

// E15's shadow map of the player's lamp pair at a point of the air: the same lookup as the surfaces', the point
// pushed toward the lamp by a texel at its distance
fn spot_visibility(world: vec3<f32>) -> f32 {
    let o = spots.origin.xyz;
    let to = normalize(o - world);
    let dist = length(world - o);
    let texel = spots.shadow.y * dist;
    let p = spots.mat * vec4<f32>(world + to * texel, 1.0);
    let near = spots.shadow.z;
    let far = spots.shadow.w;
    if p.w <= near {
        return 1.0;
    }
    let ndc = p.xyz / p.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 {
        return 1.0;
    }
    let bias = (0.05 + 1.5 * texel) * far * near / ((far - near) * p.w * p.w);
    let depth = ndc.z - bias;
    let st = spots.origin.w;
    var sum = 0.0;
    for (var k = 0; k < 4; k++) {
        sum += textureSampleCompareLevel(t_spot, s_shadow, uv + vec2<f32>(f32(k & 1) * 2.0 - 1.0, f32(k >> 1) * 2.0 - 1.0) * st, depth);
    }
    let edge = max(abs(ndc.x), abs(ndc.y));
    return mix(sum * 0.25, 1.0, smoothstep(0.85, 1.0, edge));
}

// the scattering phase of a headlight beam toward the eye: a flat part and a forward-leaning Henyey-Greenstein lobe
// (g 0.45); `c` is the cosine between the light's travel and the direction to the eye
fn phase(c: f32) -> f32 {
    let g = 0.45;
    let hg = (1.0 - g * g) / pow(1.0 + g * g - 2.0 * g * c, 1.5);
    return 0.5 + 0.5 * hg;
}

// PERF: the march at half size; the alpha keeps the distance to the scene for the composite's depth-aware blur
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    let fp = floor(pos.xy) * 2.0;
    let uv = (fp + 0.5) / dims;
    let dir = view_dir(uv);
    let ts = scene_dist(depth_at(vec2<i32>(fp)), dir);
    let tmax = min(ts, u.air.z);
    var sum = vec3<f32>(0.0);
    if tmax > 0.05 {
        // a static per-pixel offset hides the banding of the few steps (nothing crawls)
        let jit = ign(pos.xy);
        let eye = u.cam.xyz;
        let n_spots = i32(spots.params.x);
        let n_lamps = i32(u.air.w);
        let sig_beam = u.air.x;
        let sig_lamp = u.air.y;
        for (var i = 0; i < STEPS; i++) {
            let s = (f32(i) + jit) / f32(STEPS);
            let t = tmax * s * s;
            let dt = tmax * 2.0 * s / f32(STEPS);
            let p = eye + dir * t;
            var e = vec3<f32>(0.0);
            if sig_beam > 0.0 && n_spots > 0 {
                var vis = -1.0;
                for (var k = 0; k < n_spots; k++) {
                    let lv = p - spots.pos[k].xyz;
                    let d = max(length(lv), 0.3);
                    let l = lv / d;
                    let cone = smoothstep(spots.params.y, spots.params.z, dot(l, spots.dir[k].xyz));
                    if cone <= 0.0 {
                        continue;
                    }
                    let r = clamp(1.0 - d / spots.params.w, 0.0, 1.0);
                    if r <= 0.0 {
                        continue;
                    }
                    let d25 = d / 25.0;
                    // no halo on the bonnet: the beam's air starts a few metres ahead of the lamp and fades in over 3-9 m
                    var w = spots.pos[k].w * cone * r * r / (1.0 + d25 * d25) * smoothstep(3.0, 9.0, d);
                    if f32(k) < spots.shadow.x {
                        if vis < 0.0 {
                            vis = spot_visibility(p);
                        }
                        w = w * vis;
                    }
                    // light travels along l; it reaches the eye along -dir
                    e += CONE_COLOUR * (w * phase(dot(l, -dir)));
                }
                e = e * (sig_beam * u.gain.x * exp(-sig_beam * 0.5 * t));
            }
            var g = vec3<f32>(0.0);
            if sig_lamp > 0.0 {
                for (var k = 0; k < n_lamps; k++) {
                    let d = length(p - u.lamp_pos[k].xyz);
                    let r = clamp(1.0 - d / u.lamp_pos[k].w, 0.0, 1.0);
                    g += u.lamp_col[k].rgb * (r * r);
                }
                g = g * (sig_lamp * u.gain.y);
            }
            sum += (e + g) * dt;
        }
    }
    return vec4<f32>(sum, min(ts, 1000.0));
}

// the composite: the four nearest half-size texels weighted bilinearly and by how well their distance matches this
// pixel's, over the full-size scene; the cockpit's own panels and pillars (closer than 1.5 m) take none
@fragment fn fs_comp(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(depth_dims());
    let p = vec2<i32>(pos.xy);
    let dir = view_dir(pos.xy / dims);
    let ts = scene_dist(depth_at(p), dir);
    if ts < 1.5 {
        return vec4<f32>(0.0);
    }
    let half_dims = vec2<i32>(textureDimensions(rays));
    // this pixel's place in half-size texel space (texel centres at integer + 0.5)
    let hp = pos.xy * 0.5 - 0.5;
    let base = vec2<i32>(floor(hp));
    let f = hp - floor(hp);
    var acc = vec3<f32>(0.0);
    var wsum = 0.0;
    for (var k = 0; k < 4; k++) {
        let o = vec2<i32>(k & 1, k >> 1);
        let q = clamp(base + o, vec2<i32>(0), half_dims - 1);
        let tex = textureLoad(rays, q, 0);
        let bl = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let dd = abs(min(tex.a, 80.0) - min(ts, 80.0)) / (0.15 * min(ts, 80.0) + 0.3);
        let w = bl * exp(-dd * dd) + 1e-4;
        acc += tex.rgb * w;
        wsum += w;
    }
    return vec4<f32>(acc / wsum, 0.0);
}
"#;

/// The pass's shader for a depth `kind` and `steps` march steps.
pub(crate) fn source(kind: DepthKind, steps: u32) -> String {
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::IGN, kind.wgsl(), &WGSL.replace("STEPS_N", &steps.to_string())])
}

const RAYS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

struct Pipes {
    rays: wgpu::RenderPipeline,
    rays_bgl: wgpu::BindGroupLayout,
    comp: wgpu::RenderPipeline,
    comp_bgl: wgpu::BindGroupLayout,
}

/// What [`HazePass`]'s bind groups are made of: the pipelines' key, the depth, the rays image, E15's spot uniform and map.
type HazeBgKey = ((DepthKind, u32, u32), wgpu::TextureView, wgpu::TextureView, wgpu::Buffer, wgpu::TextureView);

/// The pipelines (one per depth kind and sample count of the target, built when first needed), the half-size image and
/// the uniform.
pub(crate) struct HazePass {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    ubuf: wgpu::Buffer,
    shadow_sampler: wgpu::Sampler,
    pipes: std::collections::HashMap<(DepthKind, u32, u32), Pipes>,
    /// the march's and the composite's bind groups, kept while the pipelines, the depth, the rays image and E15's spot
    /// uniform and map hold
    bgs: Keyed<HazeBgKey, (wgpu::BindGroup, wgpu::BindGroup)>,
    image: Option<((u32, u32), wgpu::Texture, wgpu::TextureView)>,
}

impl HazePass {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ubuf = gfx::uniform_buf::<Uniform>(device, "haze");
        // the same comparison sampler E9's shadows use: hardware bilinear compare, clamped
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Self { device: device.clone(), format, ubuf, shadow_sampler, pipes: Default::default(), bgs: Keyed::default(), image: None }
    }

    /// `kind`: what the depth is; `samples`: the colour's sample count (1 when the scene is already resolved); `steps`
    /// the march's step count.
    fn build(&self, kind: DepthKind, samples: u32, steps: u32) -> Pipes {
        let module = gfx::wgsl_module(&self.device, "haze", &source(kind, steps));
        let depth = kind.layout_entry();
        let uniform = |binding: u32| gfx::bgl_uniform(binding, wgpu::ShaderStages::FRAGMENT);
        let rays_tex = gfx::bgl_tex(3, wgpu::TextureSampleType::Float { filterable: false }, false);
        let spot_map = gfx::bgl_tex(7, wgpu::TextureSampleType::Depth, false);
        let shadow_samp = gfx::bgl_sampler(8, wgpu::SamplerBindingType::Comparison);
        let layout = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| gfx::bgl(&self.device, label, entries);
        let rays_bgl = layout("haze rays", &[depth, uniform(2), uniform(6), spot_map, shadow_samp]);
        let comp_bgl = layout("haze composite", &[depth, uniform(2), rays_tex]);
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        let pipe = |label: &str, fs: &str, bgl: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>, samples: u32, mask: wgpu::ColorWrites| {
            let pl = gfx::pipeline_layout(&self.device, label, &[bgl]);
            Fullscreen::new(label, &pl, &module).fs(fs).samples(samples).build(&self.device, &[Some(wgpu::ColorTargetState { format, blend, write_mask: mask })])
        };
        let rays = pipe("haze rays", "fs", &rays_bgl, RAYS_FORMAT, None, 1, wgpu::ColorWrites::ALL);
        let comp = pipe("haze composite", "fs_comp", &comp_bgl, self.format, Some(wgpu::BlendState { color: add, alpha: add }), samples, wgpu::ColorWrites::COLOR);
        Pipes { rays, rays_bgl, comp, comp_bgl }
    }

    /// Record the pass into `encoder`: the march at half size, then added over `colour` (loaded; `resolve` when it is
    /// multisampled; `keep` = store the multisampled samples for a later pass), reading `depth` and E15's spot uniform
    /// and shadow map.
    pub(crate) fn draw(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, f: &Frame, at: &OverScene, spot_uniform: &wgpu::Buffer, spot_map: &wgpu::TextureView) {
        let OverScene { depth, kind, colour, resolve, samples, keep } = *at;
        let samples = samples.max(1);
        let steps = if f.gain > 0.8 { 14 } else { 10 };
        let key = (kind, samples, steps);
        if !self.pipes.contains_key(&key) {
            let p = self.build(kind, samples, steps);
            self.pipes.insert(key, p);
        }
        let (w, h) = (depth.texture().width(), depth.texture().height());
        if self.image.as_ref().is_none_or(|i| i.0 != (w, h)) {
            let t = gfx::texture_2d(&self.device, "haze rays", w.div_ceil(2).max(1), h.div_ceil(2).max(1), RAYS_FORMAT, gfx::RT_USAGE);
            let v = t.create_view(&Default::default());
            self.image = Some(((w, h), t, v));
        }
        let rays_view = &self.image.as_ref().expect("haze image").2;
        let pipes = &self.pipes[&key];
        let mut u = Uniform {
            inv_vp: f.inv_vp.to_cols_array_2d(),
            cam: [f.eye.x, f.eye.y, f.eye.z, f.planes[0]],
            fwd: [f.fwd.x, f.fwd.y, f.fwd.z, f.planes[1]],
            air: [f.beam, f.lamp, MAX_DISTANCE, (f.haze.n as usize).min(HAZE_LAMPS) as f32],
            gain: [f.gain * BEAM_GAIN, f.gain * LAMP_GAIN, 0.0, 0.0],
            lamp_pos: [[0.0; 4]; HAZE_LAMPS],
            lamp_col: [[0.0; 4]; HAZE_LAMPS],
        };
        for (i, l) in f.haze.lamps.iter().take(f.haze.n as usize).enumerate() {
            u.lamp_pos[i] = [l.pos.x, l.pos.y, l.pos.z, l.radius];
            u.lamp_col[i] = [l.color[0], l.color[1], l.color[2], 0.0];
        }
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&u));
        let (device, ubuf_res, shadow_sampler) = (&self.device, &self.ubuf, &self.shadow_sampler);
        let (bg1, bg2) = self.bgs.get((key, depth.clone(), rays_view.clone(), spot_uniform.clone(), spot_map.clone()), || {
            let depth_entry = wgpu::BindGroupEntry { binding: kind.binding(), resource: wgpu::BindingResource::TextureView(depth) };
            let ubuf = wgpu::BindGroupEntry { binding: 2, resource: ubuf_res.as_entire_binding() };
            let bg1 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("haze rays"),
                layout: &pipes.rays_bgl,
                entries: &[
                    depth_entry.clone(),
                    ubuf.clone(),
                    wgpu::BindGroupEntry { binding: 6, resource: spot_uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(spot_map) },
                    wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(shadow_sampler) },
                ],
            });
            let bg2 = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("haze composite"),
                layout: &pipes.comp_bgl,
                entries: &[depth_entry, ubuf, wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(rays_view) }],
            });
            (bg1, bg2)
        });
        {
            let mut pass = gfx::color_pass(encoder, "haze (march, half size)", "haze march (half)", rays_view, wgpu::LoadOp::Clear(wgpu::Color::BLACK));
            pass.set_pipeline(&pipes.rays);
            pass.set_bind_group(0, bg1, &[]);
            pass.draw(0..3, 0..1);
        }
        let mut pass = gfx::color_pass_resolve(encoder, "haze (composite)", "haze composite", colour, resolve, wgpu::LoadOp::Load, if samples > 1 && !keep { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store });
        pass.set_pipeline(&pipes.comp);
        pass.set_bind_group(0, bg2, &[]);
        pass.draw(0..3, 0..1);
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
    fn a_day_tunnel_keeps_a_faint_beam_and_a_night_tunnel_its_own() {
        // alps' tunnel by day: fog 0.00227, dark 0.6 (NFS_HAZE_LOG): the beam's air was at the cap
        let alps = Haze { fog: 0.00227, cover: 1.0, dark: 0.6, ..Haze::default() };
        let alps_day = Haze { day: true, ..alps };
        assert!((alps.air(2).0 - 0.0332).abs() < 1e-3);
        assert!((alps_day.air(2).0 - 0.0332 * DAY_TUNNEL_AIR).abs() < 1e-3);
        // the lamps' glow is not this factor's
        assert_eq!(alps.air(2).1, alps_day.air(2).1);
        // open road by day (cover 0): unchanged (fog, rain)
        let rain = Haze { fog: 0.001, wet: 1.0, dark: 0.78, ..Haze::default() };
        assert_eq!(rain.air(2), Haze { day: true, ..rain }.air(2));
        // half way into the tunnel: half way between
        let half = Haze { day: true, cover: 0.5, ..alps };
        let open = Haze { cover: 0.5, ..alps }.air(2).0;
        assert!((half.air(2).0 - open * (1.0 - 0.5 * (1.0 - DAY_TUNNEL_AIR))).abs() < 1e-6);
    }

    #[test]
    fn the_uniform_matches_the_shader_layout() {
        // mat4 + 4 vec4 + 2 x 16 vec4
        assert_eq!(std::mem::size_of::<Uniform>(), 64 + 4 * 16 + 2 * 16 * 16);
    }
}
