//! ENHANCEMENT E51 (CARSH-FADE, level 1 «где светит солнце»): the near cars' share of the sun, per car, from E9's shadow map
//! (`carsun.wgsl`), eased in time, into `Csm::carsh_k`: the track receiver scales a car's own ungated shadow by it
//! (`shader.wgsl` `car_sun_share`). Replaces ALPS-CARSH's per-pixel cut (a car's shadow dropped texel by texel wherever the map
//! had scenery nearer the sun than the car: in patches, and under a house or a post whose own shadow E9's "baked dark" gate hides,
//! so the car's shadow went with no visible cause). Needs compute shaders (not on the GL fallback: level 1 then draws as level 2).

use glam::Vec3;

use super::csm::Cascade;
use crate::gfx;

/// ENHANCEMENT E51 (CARSH-FADE): the least distance (m) toward the sun an occluder must stand from a sample point (or from the car's
/// own surface over it) to shade it. APPROXIMATION (enhancement content, by eye on alps / coastal / castle): over the bank a ring
/// point may sink into beside a road cut (a point up to about a metre inside the rock reads lit) and the passes' depth noise.
pub(super) const MIN_REACH: f32 = 1.5;
/// ENHANCEMENT E51 (CARSH-FADE): the rings' height over the car's road point (m). APPROXIMATION: over a car's roof (1.3-1.4 m), so no
/// ring point's ray toward the sun meets the car itself, low enough to sit under a gorge's or a wall's shade.
pub(super) const RING_HEIGHT: f32 = 2.0;
/// ENHANCEMENT E51 (CARSH-FADE): the three rings' radii (m). APPROXIMATION: the broad shade of a gorge, a rock wall or a cliff covers
/// all three; a house's, a tree's or a post's covers the car and a part of the inner ring.
pub(super) const RINGS: [f32; 3] = [3.5, 6.5, 10.0];
/// ENHANCEMENT E51 (CARSH-FADE): the spacing (m) of each sample's four comparison taps (a shade's edge is crossed over this much
/// travel, not over a texel). APPROXIMATION.
pub(super) const TAP_SPACING: f32 = 0.6;
/// ENHANCEMENT E51 (CARSH-FADE): the body's share of shade that starts / completes the fade, and the rings'. APPROXIMATION: the car
/// mostly in the shade, and most of the ground round it (two thirds and more fades it out entirely).
pub(super) const FADE: [f32; 4] = [0.35, 0.8, 0.45, 0.85];
/// ENHANCEMENT E51 (CARSH-FADE): the ease's time constant (s): entering or leaving a gorge fades the shadow over about 0.4 s
/// (90 %), whatever the speed. APPROXIMATION.
pub(super) const EASE_TAU: f32 = 0.17;
/// ENHANCEMENT E51 (CARSH-FADE): a car that moved farther than this (m) since the last frame (a reset, a teleport, a camera cut to
/// another car's key) takes its new target at once.
const JUMP: f32 = 20.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    mats: [[[f32; 4]; 4]; 4],
    info: [[f32; 4]; 4],
    misc: [f32; 4],
    consts: [f32; 4],
    fade: [f32; 4],
    rings: [f32; 4],
    cars: [[f32; 4]; 4 * crate::MAX_CONTACTS],
}

pub(super) struct CarSun {
    layout: wgpu::BindGroupLayout,
    pipe: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    state: wgpu::Buffer,
    pub(super) out: wgpu::Buffer,
    /// the keys' road points of the last measured frame (`None` = not in it) and its draw clock (s, `SunCars::clock`)
    prev: Vec<Option<Vec3>>,
    prev_at: Option<f64>,
    /// PERF: the bind group, kept while the map's view is the same
    bg: gfx::Keyed<wgpu::TextureView, wgpu::BindGroup>,
}

impl CarSun {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let module = gfx::wgsl_module(device, "car sun", include_str!("carsun.wgsl"));
        let buf = |ty: wgpu::BufferBindingType| wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None };
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let layout = gfx::bgl(
            device,
            "car sun",
            &[
                entry(0, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }),
                entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
                entry(2, buf(wgpu::BufferBindingType::Uniform)),
                entry(3, buf(wgpu::BufferBindingType::Storage { read_only: false })),
                entry(4, buf(wgpu::BufferBindingType::Storage { read_only: false })),
            ],
        );
        let pl = gfx::pipeline_layout(device, "car sun", &[&layout]);
        let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("car sun"), layout: Some(&pl), module: &module, entry_point: Some("car_sun"), compilation_options: Default::default(), cache: None });
        let make = |label, size: u64, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        Self {
            layout,
            pipe,
            params: make("car sun params", std::mem::size_of::<Params>() as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            state: make("car sun state", (crate::SUN_CAR_KEYS * 4) as u64, wgpu::BufferUsages::STORAGE),
            out: make("car sun out", (crate::MAX_CONTACTS * 4) as u64, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC),
            prev: vec![None; crate::SUN_CAR_KEYS],
            prev_at: None,
            bg: Default::default(),
        }
    }


    /// Write this frame's measure (the cascades; the cars, each one's cascade by its view depth, blended into the next over the
    /// last eighth as the receiver's are; whether a car is new; the ease's share). Returns the cars' (centre, reach) for the track
    /// receiver (`Csm::carsh_cars`); empty = no car (then nothing is to be recorded).
    pub(super) fn prepare(&mut self, queue: &wgpu::Queue, cascades: &[Cascade], view: glam::Mat4, cars: &crate::SunCars, layers: (usize, usize)) -> Vec<[f32; 4]> {
        let (car_layers, car_base) = layers;
        let dt = self.prev_at.map_or(-1.0, |t| (cars.clock - t) as f32);
        self.prev_at = Some(cars.clock);
        // a new race (the clock went back) or a long gap: every car takes its target as it is
        let stale = !(0.0..=1.0).contains(&dt);
        let mut u = Params::zeroed();
        for (i, c) in cascades.iter().take(4).enumerate() {
            u.mats[i] = c.mat.to_cols_array_2d();
            u.info[i] = [c.texel, c.range, c.split, 0.0];
        }
        let n = (cars.n as usize).min(crate::MAX_CONTACTS);
        u.misc = [n as f32, car_layers as f32, car_base as f32, 1.0 - (-dt.clamp(0.0, 1.0) / EASE_TAU).exp()];
        u.consts = [MIN_REACH, RING_HEIGHT, TAP_SPACING, 0.0];
        u.fade = FADE;
        u.rings = [RINGS[0], RINGS[1], RINGS[2], 0.0];
        let count = cascades.len().min(4);
        let mut seen = vec![None; crate::SUN_CAR_KEYS];
        let mut centres = Vec::with_capacity(n);
        for (i, sc) in cars.cars.iter().take(n).enumerate() {
            let c = &sc.pose;
            let key = usize::from(sc.key) % crate::SUN_CAR_KEYS;
            let centre = c.origin + c.axes[1] * 0.7;
            let vd = -(view * centre.extend(1.0)).z;
            let mut ci = 0;
            for (k, cascade) in cascades.iter().enumerate().take(count.saturating_sub(1)) {
                if vd >= cascade.split {
                    ci = k + 1;
                }
            }
            let end = cascades[ci].split;
            let t = if ci + 1 < count { ((vd - end * 0.875) / (end * 0.125)).clamp(0.0, 1.0) } else { 0.0 };
            let fresh = stale || self.prev[key].is_none_or(|p| p.distance(c.origin) > JUMP);
            seen[key] = Some(c.origin);
            u.cars[4 * i] = [c.origin.x, c.origin.y, c.origin.z, key as f32];
            u.cars[4 * i + 1] = [c.axes[2].x, c.axes[2].y, c.axes[2].z, c.half[1]];
            u.cars[4 * i + 2] = [c.axes[0].x, c.axes[0].y, c.axes[0].z, c.half[0]];
            u.cars[4 * i + 3] = [ci as f32, t, if fresh { 1.0 } else { 0.0 }, 0.0];
            let reach = (c.half[0] * c.half[0] + c.half[1] * c.half[1]).sqrt() + 0.5;
            centres.push([centre.x, centre.y, centre.z, reach]);
        }
        self.prev = seen;
        if !centres.is_empty() {
            queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&u));
        }
        centres
    }

    /// Record the measure of `n` cars (after the shadow passes; `map` the shadow map's array view and comparison sampler) and copy
    /// their shares into `dst` (the csm uniform, at `Csm::carsh_k`'s offset).
    pub(super) fn record(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, n: usize, map: (&wgpu::TextureView, &wgpu::Sampler), dst: (&wgpu::Buffer, u64)) {
        let (layout, params, state, out) = (&self.layout, &self.params, &self.state, &self.out);
        let bg = self.bg.get(map.0.clone(), || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("car sun"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(map.0) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(map.1) },
                    wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: state.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: out.as_entire_binding() },
                ],
            })
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("car sun"), timestamp_writes: crate::prof::cp("E51 car sun compute") });
            pass.set_bind_group(0, bg, &[]);
            pass.set_pipeline(&self.pipe);
            pass.dispatch_workgroups(n as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.out, 0, dst.0, dst.1, (crate::MAX_CONTACTS * 4) as u64);
    }
}

use bytemuck::Zeroable;
