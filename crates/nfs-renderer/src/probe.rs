//! ENHANCEMENT E16 — the car's ambient from the surroundings: the faces of E8's environment map are
//! projected to spherical harmonics on the GPU (`probe.wgsl`), after every face update, into a uniform
//! the shader's car lighting reads (`shader.wgsl` `sh_irradiance`). Needs E8 (its faces) and compute
//! shaders (not on the GL fallback: then E16 does nothing).

use crate::gfx;

/// How far the car's ambient moves from the original's scalar towards the probe's direction and colour
/// (0 = the original, 1 = the full probe). The mean luminance stays the original's at every value.
pub const STRENGTH: f32 = 0.6;
/// ENHANCEMENT E16 level 2 («сильнее»): a clearer sky-above / ground-below split and more surrounding colour.
pub const STRENGTH_STRONG: f32 = 1.5;
/// The share of a fresh projection taken per update (one per face drawn: ~6 updates a full map).
const BLEND: f32 = 0.12;

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
}

impl Probe {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = gfx::wgsl_module(device, "probe", include_str!("probe.wgsl"));
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
            partial: make("probe partial", 54 * 16, wgpu::BufferUsages::STORAGE),
            state: make("probe state", 10 * 16, wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC),
            params: make("probe params", 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            uniform: make("probe sh", 9 * 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            reset: true,
            done: false,
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

    /// Record one update: project the six faces (`faces`, `size`², `tan_inv` = 1 / tan(half fov)) and
    /// blend into the state, then copy the state into the uniform. The faces must all have been drawn.
    pub(crate) fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, faces: &wgpu::TextureView, size: u32, tan_inv: f32) {
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params { p: [size as f32, tan_inv, BLEND, self.reset as u32 as f32] }));
        self.reset = false;
        self.done = true;
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("probe"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(faces) },
                wgpu::BindGroupEntry { binding: 1, resource: self.partial.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.state.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.params.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("probe"), timestamp_writes: crate::prof::cp("E16 probe SH compute") });
            pass.set_bind_group(0, &bg, &[]);
            pass.set_pipeline(&self.pipe_partial);
            pass.dispatch_workgroups(6, 1, 1);
            pass.set_pipeline(&self.pipe_final);
            pass.dispatch_workgroups(1, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.state, 0, &self.uniform, 0, 9 * 16);
    }
}
