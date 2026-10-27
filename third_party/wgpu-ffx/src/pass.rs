use std::borrow::Cow;

use crate::{
    FrameKind, FsrContextFlags, FsrDispatchInfo,
    resources::{AccessType, FsrResourceName, FsrResources},
};

use wgpu_ffx_shaders_spv::fsr3upscaler::Shaders;

/// Drops every `OpDecorate <id> Coherent` / `OpMemberDecorate <id> <m> Coherent` of a SPIR-V module (decoration 23); every other
/// instruction is copied word for word.
fn strip_coherent(spv: &[u32]) -> Vec<u32> {
    const OP_DECORATE: u32 = 71;
    const OP_MEMBER_DECORATE: u32 = 72;
    const COHERENT: u32 = 23;
    let mut out = Vec::with_capacity(spv.len());
    out.extend_from_slice(&spv[..5.min(spv.len())]);
    let mut i = 5;
    while i < spv.len() {
        let wc = (spv[i] >> 16) as usize;
        let op = spv[i] & 0xFFFF;
        if wc == 0 || i + wc > spv.len() {
            out.extend_from_slice(&spv[i..]);
            break;
        }
        let drop = (op == OP_DECORATE && wc == 3 && spv[i + 2] == COHERENT) || (op == OP_MEMBER_DECORATE && wc == 4 && spv[i + 3] == COHERENT);
        if !drop {
            out.extend_from_slice(&spv[i..i + wc]);
        }
        i += wc;
    }
    out
}

pub(crate) struct FsrPass {
    kind: FsrPassKind,
    pipeline: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

impl FsrPass {
    pub fn new(
        device: &wgpu::Device,
        kind: FsrPassKind,
        flags: FsrContextFlags,
        shaders: &Shaders,
    ) -> Self {
        // vendored change: passthrough only when asked (Vulkan with PASSTHROUGH_SHADERS); wgpu 30 rejects SPIR-V passthrough on every
        // other backend, and a device that merely has the feature (the upstream tests request them all) must not take this path
        let shader_module = if flags.contains(FsrContextFlags::SPIRV_PASSTHROUGH)
            && device
                .features()
                .contains(wgpu::Features::PASSTHROUGH_SHADERS)
        {
            unsafe {
                device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
                    label: Some(kind.label()),
                    // wgpu 30: the workgroup size is per entry point (only read by Metal)
                    entry_points: Cow::Borrowed(&[wgpu::PassthroughShaderEntryPoint {
                        name: Cow::Borrowed("main"),
                        workgroup_size: (0, 0, 0),
                    }]),
                    spirv: Some(Cow::Borrowed(bytemuck::cast_slice(kind.shader(shaders)))),
                    dxil: None,
                    msl: None,
                    hlsl: None,
                    glsl: None,
                    wgsl: None,
                    metallib: None,
                })
            }
        } else {
            // vendored change: naga 30's validator rejects the `Coherent` decoration on the SPD image (`globallycoherent
            // RWTexture2D`), so it is stripped for the naga path
            let words = strip_coherent(bytemuck::cast_slice(kind.shader(shaders)));
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(kind.label()),
                source: wgpu::ShaderSource::SpirV(Cow::Owned(words)),
            })
        };

        let resources = kind.resources(flags);

        let bgl_entries: Vec<_> = resources
            .into_iter()
            .enumerate()
            .map(|(i, access)| access.name.to_bgl_entry(i as u32, access.access_type))
            .collect();

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{} Bind Group Layout", kind.label())),
            entries: &bgl_entries,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("{} Pipeline Layout", kind.label())),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(kind.label()),
            layout: Some(&pipeline_layout),
            module: &shader_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        Self {
            kind,
            pipeline: compute_pipeline,
            bgl,
        }
    }

    #[expect(clippy::too_many_arguments)]
    pub fn dispatch(
        &self,
        device: &wgpu::Device,
        pass: &mut wgpu::ComputePass<'_>,
        resources: &FsrResources,
        info: &FsrDispatchInfo,
        flags: FsrContextFlags,
        frame_kind: FrameKind,
        x: u32,
        y: u32,
    ) {
        let resources_list = self.kind.resources(flags);
        let resources: Vec<_> = resources_list
            .into_iter()
            .map(|access| resources.to_view(info, access, frame_kind))
            .collect();

        let bind_group_entries: Vec<_> = resources
            .iter()
            .enumerate()
            .map(|(i, owned_resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: owned_resource.into(),
            })
            .collect();

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(&format!("{} Bind Group", self.kind.label())),
            layout: &self.bgl,
            entries: &bind_group_entries,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.dispatch_workgroups(x, y, 1);
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FsrPassKind {
    /// A pass which prepares game inputs for later passes
    PrepareInputs,
    /// A pass which generates the luminance mipmap chain for the current frame.
    LumaPyramid,
    /// A pass which generates the shading change detection mipmap chain for the current frame.
    ShadingChangePyramid,
    /// A pass which estimates shading changes for the current frame
    ShadingChange,
    /// A pass which prepares accumulation relevant information
    PrepareReactivity,
    /// A pass which estimates temporal instability of the luminance changes.
    LumaInstability,
    /// A pass which performs upscaling.
    Accumulate,
    /// A pass which performs upscaling, without writing to the output color target.
    /// Used when RCAS sharpening is enabled — RCAS writes the final output instead.
    AccumulateSharpen,
    /// A pass which performs sharpening.
    Rcas,
    /// A pass which draws some internal resources, for debugging purposes
    DebugView,
    /// An optional pass to generate a reactive mask.
    GenerateReactive,
}

impl FsrPassKind {
    pub fn label(&self) -> &'static str {
        match self {
            FsrPassKind::PrepareInputs => "FSR3 Prepare Inputs",
            FsrPassKind::LumaPyramid => "FSR3 Luma Pyramid",
            FsrPassKind::ShadingChangePyramid => "FSR3 Shading Change Pyramid",
            FsrPassKind::ShadingChange => "FSR3 Shading Change",
            FsrPassKind::PrepareReactivity => "FSR3 Prepare Reactivity",
            FsrPassKind::LumaInstability => "FSR3 Luma Instability",
            FsrPassKind::Accumulate => "FSR3 Accumulate",
            FsrPassKind::AccumulateSharpen => "FSR3 Accumulate Sharpen",
            FsrPassKind::Rcas => "FSR3 RCAS",
            FsrPassKind::DebugView => "FSR3 Debug View",
            FsrPassKind::GenerateReactive => "FSR3 Generate Reactive",
        }
    }

    pub fn shader(&self, shaders: &Shaders) -> &'static [u8] {
        match self {
            FsrPassKind::PrepareInputs => shaders.prepare_inputs,
            FsrPassKind::LumaPyramid => shaders.luma_pyramid,
            FsrPassKind::ShadingChangePyramid => shaders.shading_change_pyramid,
            FsrPassKind::ShadingChange => shaders.shading_change,
            FsrPassKind::PrepareReactivity => shaders.prepare_reactivity,
            FsrPassKind::LumaInstability => shaders.luma_instability,
            FsrPassKind::Accumulate => shaders.accumulate,
            FsrPassKind::AccumulateSharpen => shaders.accumulate_sharpen,
            FsrPassKind::Rcas => shaders.rcas,
            FsrPassKind::DebugView => shaders.debug_view,
            FsrPassKind::GenerateReactive => todo!(),
        }
    }

    pub fn resources(&self, flags: FsrContextFlags) -> Vec<ResourceAccess> {
        use crate::resources::AccessType::*;
        use crate::resources::FsrResourceName::*;

        // vendored FIX: upstream had these two swapped. The accumulate shader is built with `LOW_RESOLUTION_MOTION_VECTORS` when the
        // flag is NOT set and then reads slot 2 as the DILATED motion vectors (`prepare_inputs` writes them already scaled to UV by
        // `motion_vector_scale / size`); with the raw input vectors bound there (pixels, read as UVs) the reprojected UV of every
        // pixel is off screen, `bIsExistingSample` is false, the history is never used (output = the current frame alone) and the
        // luma-instability path paints the frame dark as soon as the camera moves. Only with display-resolution vectors does the
        // accumulate pass read the input ones.
        let motion_vectors = if flags.contains(FsrContextFlags::DISPLAY_RESOLUTION_MOTION_VECTORS) {
            InputMotionVectors
        } else {
            OutputDilatedMotionVectors
        };

        #[rustfmt::skip]
        let mut ret = match self {
            FsrPassKind::Accumulate | FsrPassKind::AccumulateSharpen => vec![
                ResourceAccess { name: InputExposure, access_type: Srv, desc: None },
                ResourceAccess { name: DilatedReactiveMasks, access_type: Srv, desc: None },
                ResourceAccess { name: motion_vectors, access_type: Srv, desc: None },
                ResourceAccess { name: InternalUpscaledPrevious, access_type: Srv, desc: None },
                ResourceAccess { name: Lanczos2Lut, access_type: Srv, desc: None },
                ResourceAccess { name: FarthestDepthMip1, access_type: Srv, desc: None },
                ResourceAccess { name: Luma, access_type: Srv, desc: None },
                ResourceAccess { name: LumaInstability, access_type: Srv, desc: None },
                ResourceAccess { name: InputColor, access_type: Srv, desc: None },
                ResourceAccess { name: InternalUpscaledCurrent, access_type: Uav, desc: None },
                ResourceAccess { name: OutputColor, access_type: Uav, desc: None },
                ResourceAccess { name: NewLocks, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::GenerateReactive => todo!("Need to map FfxFsr3UpscalerGenerateReactiveDescription"),
            FsrPassKind::DebugView => vec![
                ResourceAccess { name: DilatedReactiveMasks, access_type: Srv, desc: None },
                ResourceAccess { name: OutputDilatedMotionVectors, access_type: Srv, desc: None },
                ResourceAccess { name: OutputDilatedDepth, access_type: Srv, desc: None },
                ResourceAccess { name: InternalUpscaledPrevious, access_type: Srv, desc: None },
                ResourceAccess { name: InputExposure, access_type: Srv, desc: None },
                ResourceAccess { name: OutputColor, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::LumaInstability => vec![
                ResourceAccess { name: InputExposure, access_type: Srv, desc: None },
                ResourceAccess { name: DilatedReactiveMasks, access_type: Srv, desc: None },
                ResourceAccess { name: OutputDilatedMotionVectors, access_type: Srv, desc: None },
                ResourceAccess { name: FrameInfo, access_type: Srv, desc: None },
                ResourceAccess { name: LumaHistoryPrevious, access_type: Srv, desc: None },
                ResourceAccess { name: FarthestDepthMip1, access_type: Srv, desc: None },
                ResourceAccess { name: Luma, access_type: Srv, desc: None },
                ResourceAccess { name: LumaHistoryCurrent, access_type: Uav, desc: None },
                ResourceAccess { name: LumaInstability, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::LumaPyramid => vec![
                ResourceAccess { name: Luma, access_type: Srv, desc: None },
                ResourceAccess { name: FarthestDepth, access_type: Srv, desc: None },
                ResourceAccess { name: SpdAtomicCount, access_type: Uav, desc: None },
                ResourceAccess { name: FrameInfo, access_type: Uav, desc: None },
                ResourceAccess {
                    name: SpdMips,
                    access_type: Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        aspect: wgpu::TextureAspect::All,
                        base_mip_level: 0,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: SpdMips,
                    access_type: Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        aspect: wgpu::TextureAspect::All,
                        base_mip_level: 1,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: SpdMips,
                    access_type: Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        aspect: wgpu::TextureAspect::All,
                        base_mip_level: 2,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: SpdMips,
                    access_type: Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        aspect: wgpu::TextureAspect::All,
                        base_mip_level: 3,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: SpdMips,
                    access_type: Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        aspect: wgpu::TextureAspect::All,
                        base_mip_level: 4,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: SpdMips,
                    access_type: Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        aspect: wgpu::TextureAspect::All,
                        base_mip_level: 5,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess { name: FarthestDepthMip1, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::PrepareInputs => vec![
                ResourceAccess { name: InputMotionVectors, access_type: Srv, desc: None },
                ResourceAccess { name: InputDepth, access_type: Srv, desc: None },
                ResourceAccess { name: InputColor, access_type: Srv, desc: None },
                ResourceAccess { name: OutputDilatedMotionVectors, access_type: Uav, desc: None },
                ResourceAccess { name: OutputDilatedDepth, access_type: Uav, desc: None },
                ResourceAccess { name: OutputReconstructedPreviousDepth, access_type: Uav, desc: None },
                ResourceAccess { name: FarthestDepth, access_type: Uav, desc: None },
                ResourceAccess { name: Luma, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::PrepareReactivity => vec![
                ResourceAccess { name: OutputReconstructedPreviousDepth, access_type: Srv, desc: None },
                ResourceAccess { name: OutputDilatedMotionVectors, access_type: Srv, desc: None },
                ResourceAccess { name: OutputDilatedDepth, access_type: Srv, desc: None },
                ResourceAccess { name: InputReactiveMask, access_type: Srv, desc: None },
                ResourceAccess { name: InputTransparencyAndComposition, access_type: Srv, desc: None },
                ResourceAccess { name: AccumulationPrevious, access_type: Srv, desc: None },
                ResourceAccess { name: ShadingChange, access_type: Srv, desc: None },
                ResourceAccess { name: Luma, access_type: Srv, desc: None },
                ResourceAccess { name: InputExposure, access_type: Srv, desc: None },

                ResourceAccess { name: DilatedReactiveMasks, access_type: Uav, desc: None },
                ResourceAccess { name: NewLocks, access_type: Uav, desc: None },
                ResourceAccess { name: AccumulationCurrent, access_type: Uav, desc: None },

                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::Rcas => vec![
                ResourceAccess { name: InputExposure, access_type: Srv, desc: None },
                ResourceAccess { name: InternalUpscaledCurrent, access_type: Srv, desc: None },
                ResourceAccess { name: OutputColor, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::ShadingChange => vec![
                ResourceAccess { name: SpdMips, access_type: Srv, desc: None },
                ResourceAccess { name: ShadingChange, access_type: Uav, desc: None },
                ResourceAccess { name: Constants, access_type: Srv, desc: None },
            ],
            FsrPassKind::ShadingChangePyramid => vec![
                // SRV bindings
                ResourceAccess { name: FsrResourceName::Luma, access_type: AccessType::Srv, desc: None },
                ResourceAccess { name: FsrResourceName::PreviousLuma, access_type: AccessType::Srv, desc: None },
                ResourceAccess { name: FsrResourceName::OutputDilatedMotionVectors, access_type: AccessType::Srv, desc: None },
                ResourceAccess { name: FsrResourceName::InputExposure, access_type: AccessType::Srv, desc: None },
                ResourceAccess { name: FsrResourceName::SpdAtomicCount, access_type: AccessType::Uav, desc: None },
                ResourceAccess {
                    name: FsrResourceName::SpdMips,
                    access_type: AccessType::Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        base_mip_level: 0,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: FsrResourceName::SpdMips,
                    access_type: AccessType::Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        base_mip_level: 1,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: FsrResourceName::SpdMips,
                    access_type: AccessType::Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        base_mip_level: 2,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: FsrResourceName::SpdMips,
                    access_type: AccessType::Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        base_mip_level: 3,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: FsrResourceName::SpdMips,
                    access_type: AccessType::Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        base_mip_level: 4,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess {
                    name: FsrResourceName::SpdMips,
                    access_type: AccessType::Uav,
                    desc: Some(wgpu::TextureViewDescriptor {
                        base_mip_level: 5,
                        mip_level_count: Some(1),
                        ..Default::default()
                    }),
                },
                ResourceAccess { name: FsrResourceName::Constants, access_type: AccessType::Srv, desc: None },
            ]
        };

        // All passes bind the samplers at the end

        ret.push(ResourceAccess {
            name: FsrResourceName::SamplerPointClamp,
            access_type: AccessType::Srv,
            desc: None,
        });
        ret.push(ResourceAccess {
            name: FsrResourceName::SamplerLinearClamp,
            access_type: AccessType::Srv,
            desc: None,
        });

        ret
    }
}

pub(crate) struct ResourceAccess {
    pub name: FsrResourceName,
    pub access_type: AccessType,
    pub desc: Option<wgpu::TextureViewDescriptor<'static>>,
}
