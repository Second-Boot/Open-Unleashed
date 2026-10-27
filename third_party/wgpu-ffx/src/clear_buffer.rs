use bytemuck::{Pod, Zeroable};
use std::{borrow::Cow, convert::TryFrom};
use wgpu::util::DeviceExt;

const WORDS_PER_INVOCATION: u32 = 4;
const WORDS_PER_INVOCATION_USIZE: usize = WORDS_PER_INVOCATION as usize;
const WORKGROUP_SIZE_X: u32 = 128;
const WORDS_PER_WORKGROUP: u32 = WORDS_PER_INVOCATION * WORKGROUP_SIZE_X;

fn clear_buffer_shader_source() -> String {
    format!(
        r#"
const WORDS_PER_INVOCATION: u32 = {WORDS_PER_INVOCATION}u;
const LAST_WORD_OFFSET: u32 = WORDS_PER_INVOCATION - 1u;

struct ClearUniforms {{
    total_words: u32,
    start_word: u32,
    _pad0: u32,
    _pad1: u32,
    clear_value: vec4u,
}};

@group(0) @binding(0)
var<storage, read_write> buf: array<u32>;

@group(0) @binding(1)
var<uniform> uniforms: ClearUniforms;

@compute @workgroup_size({WORKGROUP_SIZE_X}, 1, 1)
fn main(@builtin(global_invocation_id) global_id: vec3u) {{
    let index = uniforms.start_word + global_id.x * WORDS_PER_INVOCATION;
    if (index >= uniforms.total_words) {{
        return;
    }}

    if (index + LAST_WORD_OFFSET >= uniforms.total_words) {{
        for (var i: u32 = index; i < uniforms.total_words; i = i + 1u) {{
            buf[i] = uniforms.clear_value[i - index];
        }}
        return;
    }}

    for (var lane: u32 = 0u; lane < WORDS_PER_INVOCATION; lane = lane + 1u) {{
        buf[index + lane] = uniforms.clear_value[lane];
    }}
}}
"#,
    )
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ClearUniformRaw {
    total_words: u32,
    start_word: u32,
    _pad: [u32; 2],
    clear_value: [u32; WORDS_PER_INVOCATION_USIZE],
}

pub struct BufferClearer {
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
}

impl BufferClearer {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader_source = clear_buffer_shader_source();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BufferClearer::shader"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(shader_source)),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("BufferClearer::pipeline"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let bind_group_layout = pipeline.get_bind_group_layout(0);
        Self {
            bind_group_layout,
            pipeline,
        }
    }

    pub fn dispatch(
        &self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        encoder: &mut wgpu::CommandEncoder,
        clear_value: [u32; WORDS_PER_INVOCATION_USIZE],
    ) {
        let total_bytes = buffer.size();
        if total_bytes == 0 {
            return;
        }
        let total_words =
            u32::try_from(total_bytes / 4).expect("BufferClearer buffer exceeds supported size");
        let total_workgroups = total_words.div_ceil(WORDS_PER_WORKGROUP);
        let max_workgroups_per_dimension = device.limits().max_compute_workgroups_per_dimension;
        assert!(
            max_workgroups_per_dimension > 0,
            "Device reports zero max compute workgroups per dimension"
        );
        let mut dispatched_workgroups = 0;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("BufferClearer::pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        while dispatched_workgroups < total_workgroups {
            let workgroups_this_dispatch =
                (total_workgroups - dispatched_workgroups).min(max_workgroups_per_dimension);
            let start_word = dispatched_workgroups * WORDS_PER_WORKGROUP;
            let uniforms = ClearUniformRaw {
                total_words,
                start_word,
                _pad: [0; 2],
                clear_value,
            };
            let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("BufferClearer::uniform_buffer"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("BufferClearer::bind_group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: uniform_buffer.as_entire_binding(),
                    },
                ],
            });
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups_this_dispatch, 1, 1);
            dispatched_workgroups += workgroups_this_dispatch;
        }
    }
}
