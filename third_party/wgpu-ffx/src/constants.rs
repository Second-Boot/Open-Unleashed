#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct Constants {
    pub fsr: FsrConstants,
    pub generate_auto_reactive: GenerateAutoReactiveConstants,
    pub rcas: RcasConstants,
    pub generate_reactive: GenerateReactiveConstants,
    pub spd: SpdConstants,
}

#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct FsrConstants {
    pub render_size: [u32; 2],
    pub previous_frame_render_size: [u32; 2],

    pub upscale_size: [u32; 2],
    pub previous_frame_upscale_size: [u32; 2],

    pub max_render_size: [u32; 2],
    pub max_upscale_size: [u32; 2],

    pub device_to_view_depth: [f32; 4],

    pub jitter_offset: [f32; 2],
    pub previous_frame_jitter_offset: [f32; 2],

    pub motion_vector_scale: [f32; 2],
    pub downscale_factor: [f32; 2],

    pub motion_vector_jitter_cancellation: [f32; 2],
    pub tan_half_fov: f32,
    pub jitter_phase_count: f32,

    pub delta_time: f32,
    pub delta_pre_exposure: f32,
    pub view_space_to_meters_factor: f32,
    pub frame_index: f32,

    pub velocity_factor: f32,
    pub reactiveness_scale: f32,
    pub shading_change_scale: f32,
    pub accumulation_added_per_frame: f32,
    pub min_disocclusion_accumulation: f32,
    pub _padding: [u32; 3],
}

#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct GenerateAutoReactiveConstants {
    pub tc_threshold: f32, // 0.1 is a good starting value, lower will result in more TC pixels
    pub tc_scale: f32,
    pub reactive_scale: f32,
    pub reactive_max: f32,
}

#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct RcasConstants {
    pub rcas_config: [u32; 4],
}

#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct GenerateReactiveConstants {
    pub gen_reactive_scale: f32,
    pub gen_reactive_threshold: f32,
    pub gen_reactive_binary_value: f32,
    pub gen_reactive_flags: u32,
}

#[derive(Debug, Default, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub(crate) struct SpdConstants {
    pub mips: u32,
    pub num_work_groups: u32,
    pub work_group_offset: [u32; 2],
    pub render_size: [u32; 2],
    pub _padding: [u32; 2],
}
