//! The mesh pipelines: the sets keyed by sample count / float scene / temporal view (`ensure_pipelines`, `HDR_KEY`,
//! `TEMPORAL_KEY`) and `build_mesh_pipelines` with the original's Thrash state tables. Moved out of gpu.rs as it was.

use super::*;

/// ENHANCEMENT E7: set in a pipeline-set key for the float set (the key's low bits are the sample count).
pub(super) const HDR_KEY: u32 = 1 << 8;

/// ENHANCEMENT ENH-TEMPORAL: the key bit of the temporal pipeline set (the main view's, with the vector and mask targets)
const TEMPORAL_KEY: u32 = 1 << 9;
/// The sets built from the full mesh shader (any switch on); key 1 etc. without it are the all-off shader's, which lacks the
/// inert blocks of ENH-DBLSH / E26 (`classes::mesh_shader_source`)
const FULL_KEY: u32 = 1 << 10;

/// The mesh pipelines (54: blend group × cull × depth, see `Core::new`'s notes; with `samples > 1`
/// six more, the alpha-to-coverage opaque group 9 — ENHANCEMENT E1; then the two layered groups, 12 — master's 9 / 10)
/// for a target of `samples`.
pub(super) fn build_mesh_pipelines(device: &wgpu::Device, shader: &wgpu::ShaderModule, pipeline_layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat, samples: u32, temporal: bool) -> Vec<wgpu::RenderPipeline> {
    // Thrash state 0x38's pairs (`dx7z.dll` 0x600046BB): 1 `SRCALPHA`/`ONE` (the general arm;
    // two device kinds take `ONE`/`ONE`), 2 `ZERO`/`INVSRCALPHA`, 3 `DESTCOLOR`/`ZERO`
    let pair_state = |pair: u8| {
        use wgpu::BlendFactor as F;
        let (src, dst) = match pair {
            1 => (F::SrcAlpha, F::One),
            2 => (F::Zero, F::OneMinusSrcAlpha),
            3 => (F::Dst, F::Zero),
            _ => return wgpu::BlendState::ALPHA_BLENDING,
        };
        let c = wgpu::BlendComponent { src_factor: src, dst_factor: dst, operation: wgpu::BlendOperation::Add };
        wgpu::BlendState { color: c, alpha: c }
    };
    let make_pipeline = |blend: bool, pair: u8, cull: Option<wgpu::Face>, depth: bool, zwrite: bool, a2c: bool, layers: bool| {
        // ENHANCEMENT ENH-TEMPORAL: next to the colour, the motion vector (replaced by an opaque or depth-writing draw;
        // a translucent one keeps the surface behind it, but for a sky layer, which its `.w` lets through), the reactive
        // mask and the transparency-and-composition mask (replaced by an opaque draw, the larger value kept by a
        // translucent one, untouched by a depth-writing blended one)
        let mut targets = vec![Some(wgpu::ColorTargetState {
            format,
            blend: Some(if layers {
                // `fs_layers` writes the premultiplied sum of the passes and their coverage
                let c = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add };
                wgpu::BlendState { color: c, alpha: c }
            } else if blend {
                pair_state(pair)
            } else {
                wgpu::BlendState::REPLACE
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        if temporal {
            use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
            let comp = |src, dst, op| wgpu::BlendState { color: C { src_factor: src, dst_factor: dst, operation: op }, alpha: C { src_factor: F::Zero, dst_factor: F::One, operation: O::Add } };
            let (mv, re) = if !blend {
                ((None, wgpu::ColorWrites::ALL), (None, wgpu::ColorWrites::ALL))
            } else if zwrite {
                ((None, wgpu::ColorWrites::ALL), (None, wgpu::ColorWrites::empty()))
            } else {
                ((Some(comp(F::SrcAlpha, F::OneMinusSrcAlpha, O::Add)), wgpu::ColorWrites::ALL), (Some(comp(F::One, F::One, O::Max)), wgpu::ColorWrites::ALL))
            };
            targets.push(Some(wgpu::ColorTargetState { format: crate::temporal::MV_FORMAT, blend: mv.0, write_mask: mv.1 }));
            targets.push(Some(wgpu::ColorTargetState { format: crate::temporal::MASK_FORMAT, blend: re.0, write_mask: re.1 }));
            targets.push(Some(wgpu::ColorTargetState { format: crate::temporal::MASK_FORMAT, blend: re.0, write_mask: re.1 }));
        }
        let pair_const = [("PAIR", pair as f64)];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(match (temporal, blend) {
                    (true, true) if layers => "fs_layers_t",
                    (false, true) if layers => "fs_layers",
                    (true, true) => "fs_main_t",
                    (true, false) => "fs_opaque_t",
                    (false, true) => "fs_main",
                    (false, false) if a2c => "fs_opaque_a2c",
                    (false, false) => "fs_opaque",
                }),
                compilation_options: wgpu::PipelineCompilationOptions { constants: if temporal && blend { &pair_const } else { &[] }, ..Default::default() },
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // mirrored clip x (see Camera::view_projection) flips the apparent winding
                front_face: wgpu::FrontFace::Cw,
                cull_mode: cull,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(depth && (!blend || zwrite)),
                // `ZFUNC` `LESSEQUAL`: `dx7z.dll` makes the device with it (0x6000A545) and the
                // game sets it again through the view (Thrash 0x28 ← 3: the race pass
                // 0x468B54, the front end's 3D pass 0x4D301D). The front-end rooms' layers —
                // light pools and cones on the floor and walls — share their surface's depth
                // and pass because of it, drawn after it in `Base+70` order
                // (`RE/notes/frontend-3d.md` §9). *(The port kept `LESS` until 2026-09-25,
                // for a fight that was the rooms' own states — file order, no pair — not the
                // depth test.)*
                depth_compare: Some(if depth { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always }),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            // ENHANCEMENT E1: `samples` 1 (the original) is `Default::default()`; the alpha-test
            // variants (`a2c`) turn the alpha into coverage
            multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: a2c },
            multiview_mask: None,
            cache: None,
        })
    };
    // six a group — the cull (back, none, front) × depth (on, off), `2·cull + !depth` — and the
    // groups: 0 blend off, 1 the plain alpha pair, 2…4 the other pairs, 5…8 every pair again
    // with depth writes on (`device_states`), then the layered passes (see below)
    const CULLS: [Option<wgpu::Face>; 3] = [Some(wgpu::Face::Back), None, Some(wgpu::Face::Front)];
    // (the 54 pipelines compile independently: built on worker threads, kept in the same order)
    let mut specs = Vec::with_capacity(72);
    for (blend, pair, zwrite) in [(false, 0, false), (true, 0, false), (true, 1, false), (true, 2, false), (true, 3, false), (true, 0, true), (true, 1, true), (true, 2, true), (true, 3, true)] {
        for i in 0..6 {
            specs.push((blend, pair, CULLS[i / 2], i % 2 == 0, zwrite, false, false));
        }
    }
    // ENHANCEMENT E1: the opaque pipelines again with alpha-to-coverage (group 9), for the
    // alpha-TESTED opaque draws (foliage, fences) — only when multisampled
    if samples > 1 {
        for i in 0..6 {
            specs.push((false, 0, CULLS[i / 2], i % 2 == 0, false, true, false));
        }
    }
    // the last two groups (always, after E1's group 9 where it exists): the layered passes of a blended material
    // (`fs_layers`, plain pair) without and with depth writes (`Core::layered_group`)
    for zwrite in [false, true] {
        for i in 0..6 {
            specs.push((true, 0, CULLS[i / 2], i % 2 == 0, zwrite, false, true));
        }
    }
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8);
    let pipelines: Vec<wgpu::RenderPipeline> = std::thread::scope(|sc| {
        let make_pipeline = &make_pipeline;
        let handles: Vec<_> = specs
            .chunks(specs.len().div_ceil(workers))
            .map(|c| sc.spawn(move || c.iter().map(|&(blend, pair, cull, depth, zwrite, a2c, layers)| make_pipeline(blend, pair, cull, depth, zwrite, a2c, layers)).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|h| h.join().expect("pipeline worker")).collect()
    });
    pipelines
}

impl Core {
    /// ENHANCEMENT E1: make `pipelines` the set built for `samples` (built once, kept).
    pub(super) fn ensure_pipelines(&mut self, samples: u32, hdr: bool, temporal: bool) {
        let full = self.enhancements.any_on();
        let key = samples | if full { FULL_KEY } else { 0 } | if hdr { HDR_KEY } else { 0 } | if temporal { TEMPORAL_KEY } else { 0 };
        if self.pipeline_samples == key {
            return;
        }
        if full && self.mesh_shader_full.is_none() {
            self.mesh_shader_full = Some(crate::gfx::wgsl_module(&self.device, "mesh shader (full)", &crate::classes::mesh_shader_source(true)));
        }
        let shader = if full { self.mesh_shader_full.as_ref().expect("full mesh shader") } else { &self.mesh_shader };
        let set = match self.pipeline_sets.remove(&key) {
            Some(set) => set,
            // ENHANCEMENT ENH-TEMPORAL: the float set again with the vector / mask targets and the object layout that has the matrices
            None if temporal => build_mesh_pipelines(&self.device, shader, &self.temporal_gpu.as_ref().expect("temporal gpu").pipeline_layout, HDR_FORMAT, 1, true),
            None => build_mesh_pipelines(&self.device, shader, &self.mesh_layout, if hdr { HDR_FORMAT } else { self.config.format }, samples, false),
        };
        let old = std::mem::replace(&mut self.pipelines, set);
        self.pipeline_sets.insert(self.pipeline_samples, old);
        self.pipeline_samples = key;
    }
}
