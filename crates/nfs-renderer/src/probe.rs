//! ENHANCEMENT E16 — the car's ambient from the surroundings: the faces of E8's environment map are
//! projected to spherical harmonics on the GPU (`probe.wgsl`), once a frame after the frame's faces, into a uniform
//! the shader's car lighting reads (`shader.wgsl` `sh_irradiance`). Needs E8 (its faces) and compute
//! shaders (not on the GL fallback: then E16 does nothing).

use crate::gfx;

/// How far the car's ambient moves from the original's scalar towards the probe's direction and colour
/// (0 = the original, 1 = the full probe). The mean luminance stays the original's at every value.
pub const STRENGTH: f32 = 0.6;
/// ENHANCEMENT E16 level 2 («сильнее»): a clearer sky-above / ground-below split and more surrounding colour.
pub const STRENGTH_STRONG: f32 = 1.5;
/// The share of a fresh projection taken per update of the front end's uploaded faces (`Core::upload_refl_faces`); it was
/// the drawn faces' too while the probe ran after every face (two a frame: ~6 updates a full map).
pub(crate) const BLEND: f32 = 0.12;
/// The share taken per update of the drawn faces, now once a frame (`Core::refl_probe`): the two per-face updates of a
/// frame together, `1 - (1 - BLEND)^2`, so the ambient follows the surroundings at the same rate per frame.
pub(crate) const BLEND_FRAME: f32 = 1.0 - (1.0 - BLEND) * (1.0 - BLEND);
/// Second-order SH: the coefficients of bands 0..2 (`probe.wgsl`, the car shader's `probe_sh`).
pub(crate) const SH_COEFFS: usize = 9;
/// The faces of the cube the probe projects (`gpu::REFL_FACES`).
const FACES: usize = 6;
/// One coefficient (rgb, 0) in the buffers.
const VEC4: u64 = 16;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    p: [f32; 4],
}

pub(crate) struct Probe {
    layout: wgpu::BindGroupLayout,
    pipe_partial: wgpu::ComputePipeline,
    pipe_final: wgpu::ComputePipeline,
    partial: wgpu::Buffer,
    state: wgpu::Buffer,
    params: wgpu::Buffer,
    /// what the car shader reads: 9 x vec4 (the normalised irradiance's SH coefficients)
    pub(crate) uniform: wgpu::Buffer,
    /// the next update takes the projection as it is (a new face array)
    reset: bool,
    /// a projection has been recorded
    done: bool,
    /// PERF: what `params` holds (written only when it changes: the reset flag, the face size)
    params_now: Option<[f32; 4]>,
    /// PERF: the bind group, kept while the face array's view is the same
    bg: crate::gfx::Keyed<wgpu::TextureView, wgpu::BindGroup>,
}

/// The probe's compute shader: the mesh shader's SH / luma constants and face table (`classes.rs`), then `probe.wgsl`.
pub(crate) fn source() -> String {
    crate::wgsl::compose(&[crate::classes::SH_LUMA, &crate::classes::faces_wgsl(), crate::wgsl::PI, include_str!("probe.wgsl")])
}

impl Probe {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = gfx::wgsl_module(device, "probe", &source());
        let buf = |ty: wgpu::BufferBindingType| wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None };
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let layout = gfx::bgl(
            device,
            "probe",
            &[
                entry(0, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }),
                entry(1, buf(wgpu::BufferBindingType::Storage { read_only: false })),
                entry(2, buf(wgpu::BufferBindingType::Storage { read_only: false })),
                entry(3, buf(wgpu::BufferBindingType::Uniform)),
            ],
        );
        let pl = gfx::pipeline_layout(device, "probe", &[&layout]);
        let pipe = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(entry), layout: Some(&pl), module: &module, entry_point: Some(entry), compilation_options: Default::default(), cache: None })
        };
        let (pipe_partial, pipe_final) = (pipe("sh_partial"), pipe("sh_final"));
        let make = |label, size: u64, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        Self {
            layout,
            pipe_partial,
            pipe_final,
            partial: make("probe partial", (FACES * SH_COEFFS) as u64 * VEC4, wgpu::BufferUsages::STORAGE),
            // (and one more: the "initialised" flag)
            state: make("probe state", (SH_COEFFS + 1) as u64 * VEC4, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC),
            params: make("probe params", std::mem::size_of::<Params>() as u64, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            uniform: make("probe sh", SH_COEFFS as u64 * VEC4, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            reset: true,
            done: false,
            params_now: None,
            bg: Default::default(),
        }
    }

    /// True once an update has been recorded (the uniform holds a projection).
    pub(crate) fn ready(&self) -> bool {
        self.done
    }

    /// The face array changed: the next projection replaces the state.
    pub(crate) fn reset(&mut self) {
        self.reset = true;
    }

    /// Record one update: project the six faces (`faces`, `size`², seen through `REFL_TAN_HALF`) and
    /// blend `blend` of it into the state ([`BLEND`] / [`BLEND_FRAME`]), then copy the state into the uniform. The faces must
    /// all have been drawn.
    pub(crate) fn update(&mut self, device: &wgpu::Device, belt: &mut wgpu::util::StagingBelt, encoder: &mut wgpu::CommandEncoder, faces: &wgpu::TextureView, size: u32, blend: f32) {
        let tan_inv = 1.0 / crate::REFL_TAN_HALF;
        // PERF (picture identical): the 16 bytes are written only when they change (a belt copy in the frame's encoder, in
        // order with the faces' passes; the buffer keeps the last one): the reset flag's update, then its clear, are writes of
        // their own
        let p = [size as f32, tan_inv, blend, self.reset as u32 as f32];
        if self.params_now != Some(p) {
            gfx::belt_write(belt, encoder, &self.params, bytemuck::bytes_of(&Params { p }));
            self.params_now = Some(p);
        }
        self.reset = false;
        self.done = true;
        let (layout, partial, state, params) = (&self.layout, &self.partial, &self.state, &self.params);
        let bg = self.bg.get(faces.clone(), || {
            gfx::bind_group(device, "probe", layout, &[
                    gfx::bg_view(0, faces),
                    gfx::bg_buf(1, partial),
                    gfx::bg_buf(2, state),
                    gfx::bg_buf(3, params),
                ])
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("probe"), timestamp_writes: crate::prof::cp("E16 probe SH compute") });
            pass.set_bind_group(0, bg, &[]);
            pass.set_pipeline(&self.pipe_partial);
            pass.dispatch_workgroups(FACES as u32, 1, 1);
            pass.set_pipeline(&self.pipe_final);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.state, 0, &self.uniform, 0, SH_COEFFS as u64 * VEC4);
    }
}
