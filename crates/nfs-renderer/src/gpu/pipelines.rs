//! The mesh pipelines: the sets keyed by sample count / float scene / temporal view (`ensure_pipelines`, `HDR_KEY`,
//! `TEMPORAL_KEY`) and `build_mesh_pipelines` with the original's Thrash state tables. Moved out of gpu.rs as it was.

use super::*;

/// ENHANCEMENT E7: set in a pipeline-set key for the float set (the key's low bits are the sample count).
pub(super) const HDR_KEY: u64 = 1 << 8;

/// ENHANCEMENT ENH-TEMPORAL: the key bit of the temporal pipeline set (the main view's, with the vector and mask targets)
const TEMPORAL_KEY: u64 = 1 << 9;
/// PERF (PERF-B): the mesh shader's parts (`classes::feat`) sit in a key from this bit up; key 1 etc. without any are the
/// original's shader's (`classes::mesh_shader_source(0)`)
const FEATS_SHIFT: u32 = 16;

/// The cull mode of a mesh pipeline: `Back` the view's own, `None` both faces, `Front` the other face (a material whose cull is
/// the view's, drawn in a mirrored world, 0x468C45).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Cull {
    Back,
    None,
    Front,
}

impl Cull {
    const ALL: [Cull; 3] = [Cull::Back, Cull::None, Cull::Front];

    fn face(self) -> Option<wgpu::Face> {
        match self {
            Cull::Back => Some(wgpu::Face::Back),
            Cull::None => None,
            Cull::Front => Some(wgpu::Face::Front),
        }
    }
}

/// The blend group of a mesh pipeline.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Group {
    /// blend off
    Opaque,
    /// Thrash state 0x38's pair `pair` (0 the plain alpha pair, 1..=3 the others, `pair_state`); `zwrite`: with depth writes
    /// (the material's `device_states`)
    Blend { pair: u8, zwrite: bool },
    /// ENHANCEMENT E1: the opaque pipeline with alpha-to-coverage, for the alpha-TESTED opaque draws (foliage, fences); only in a
    /// multisampled set (a 1-sample set draws them with `Opaque`)
    AlphaToCoverage,
    /// the layered passes of a blended material (`fs_layers`, the plain pair), without / with depth writes
    Layered { zwrite: bool },
}

/// The Thrash 0x38 pairs a [`Group::Blend`] takes (0 = the plain alpha pair).
const BLEND_PAIRS: u8 = 4;

impl Group {
    /// The group's place in a set: 0 opaque, 1..=4 the pairs, 5..=8 the pairs with depth writes, then E1's alpha-to-coverage
    /// group (`multisampled` sets only), then the two layered groups (master's 9 / 10).
    fn slot(self, multisampled: bool) -> usize {
        let layered = if multisampled { 10 } else { 9 };
        match self {
            Group::Opaque => 0,
            Group::Blend { pair, zwrite } => 1 + pair as usize + if zwrite { BLEND_PAIRS as usize } else { 0 },
            Group::AlphaToCoverage if multisampled => 9,
            Group::AlphaToCoverage => 0,
            Group::Layered { zwrite } => layered + zwrite as usize,
        }
    }

    /// The groups of a set in their order ([`Self::slot`]).
    fn all(multisampled: bool) -> impl Iterator<Item = Group> {
        let blends = |zwrite| (0..BLEND_PAIRS).map(move |pair| Group::Blend { pair, zwrite });
        std::iter::once(Group::Opaque)
            .chain(blends(false))
            .chain(blends(true))
            .chain(multisampled.then_some(Group::AlphaToCoverage))
            .chain([false, true].map(|zwrite| Group::Layered { zwrite }))
    }
}

/// One mesh pipeline of a set: its blend group, cull mode and depth test. A set holds every key, six a group (cull × depth).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct PipelineKey {
    pub group: Group,
    pub cull: Cull,
    pub depth_test: bool,
}

/// A pipeline set's cull × depth combinations per group.
const PER_GROUP: usize = 6;

impl PipelineKey {
    /// The pipeline's index in a set (`multisampled`: the set has E1's alpha-to-coverage group).
    pub(super) fn index(self, multisampled: bool) -> usize {
        self.group.slot(multisampled) * PER_GROUP + self.cull as usize * 2 + !self.depth_test as usize
    }

    /// Every key of a set, in index order.
    fn set(multisampled: bool) -> impl Iterator<Item = PipelineKey> {
        Group::all(multisampled).flat_map(|group| Cull::ALL.into_iter().flat_map(move |cull| [true, false].map(|depth_test| PipelineKey { group, cull, depth_test })))
    }
}

/// The mesh pipelines (54: blend group × cull × depth, [`PipelineKey`]; with `samples > 1` six more, the alpha-to-coverage
/// opaque group 9 — ENHANCEMENT E1; then the two layered groups, 12 — master's 9 / 10) for a target of `samples`.
pub(super) fn build_mesh_pipelines(device: &wgpu::Device, shader: &wgpu::ShaderModule, pipeline_layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat, samples: u32, temporal: bool) -> Vec<wgpu::RenderPipeline> {
    build_mesh_pipelines_on(device, shader, pipeline_layout, format, samples, temporal, std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 8))
}

/// [`build_mesh_pipelines`] on `workers` threads.
fn build_mesh_pipelines_on(device: &wgpu::Device, shader: &wgpu::ShaderModule, pipeline_layout: &wgpu::PipelineLayout, format: wgpu::TextureFormat, samples: u32, temporal: bool, workers: usize) -> Vec<wgpu::RenderPipeline> {
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
    // (the pipelines compile independently: built on worker threads, kept in the set's order)
    let specs: Vec<_> = PipelineKey::set(samples > 1)
        .map(|k| {
            let (blend, pair, zwrite, a2c, layers) = match k.group {
                Group::Opaque => (false, 0, false, false, false),
                Group::Blend { pair, zwrite } => (true, pair, zwrite, false, false),
                Group::AlphaToCoverage => (false, 0, false, true, false),
                Group::Layered { zwrite } => (true, 0, zwrite, false, true),
            };
            (blend, pair, k.cull.face(), k.depth_test, zwrite, a2c, layers)
        })
        .collect();
    super::kit::par_build_on(&specs, workers, |&(blend, pair, cull, depth, zwrite, a2c, layers)| make_pipeline(blend, pair, cull, depth, zwrite, a2c, layers))
}


/// PERF (PERF-D): how far from a family's current light state the prewarm reaches (the number of light groups — cascades, spots,
/// lamps, contact — whose state differs): the states one light away. Every state (16 per family, four variants each) was
/// measured on the Mac: ~100 sets, +230 MB resident and a minute and a half of four cores, for states a race mostly never
/// meets; a farther state is built when met (its stand-in draws meanwhile), so this is only how often a stand-in draws.
const PREWARM_DEPTH: u32 = 1;

/// PERF (PERF-D): the light parts a frame can leave out (`shut` of `ensure_pipelines`), as the groups that go together.
/// (ENHANCEMENT E50 / E51: the moon's part and the cars-only layers' part are groups of their own: by day the first is off, at night the second)
const LIGHT_GROUPS: [u32; 8] = [
    crate::classes::feat::CSM,
    crate::classes::feat::SPOT,
    crate::classes::feat::LAMPS | crate::classes::feat::LSHAD,
    crate::classes::feat::CONTACT,
    crate::classes::feat::MOON,
    crate::classes::feat::CARSH,
    // (SHADOW-REFACTOR: the front end's focus kernel, E25's overcast kernel)
    crate::classes::feat::FOCUS,
    crate::classes::feat::OVERCAST,
];

/// PERF (PERF-D): a mesh pipeline set's build parameters (the key's low bits, the parts above them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct SetSpec {
    feats: u32,
    samples: u32,
    hdr: bool,
    temporal: bool,
}

impl SetSpec {
    fn key(self) -> u64 {
        u64::from(self.samples) | (u64::from(self.feats) << FEATS_SHIFT) | if self.hdr { HDR_KEY } else { 0 } | if self.temporal { TEMPORAL_KEY } else { 0 }
    }
}

/// The low bits of a set's key (sample count, float, temporal): its family without the parts.
const BASE_MASK: u64 = (1 << FEATS_SHIFT) - 1;

/// PERF (PERF-D): the parts of item variant `v` of the parts `feats` (`fill::ItemVariant`: bit 0 the surface parts, bit 1 the
/// effect parts).
fn variant_feats(feats: u32, v: usize) -> u32 {
    use crate::classes::feat;
    feats & !(if v & 1 == 0 { feat::SURFACE } else { 0 }) & !(if v & 2 == 0 { feat::EFFECT } else { 0 })
}

/// PERF (PERF-D): the built set of the family of `want` with every part of `want`, none outside `fam` and the fewest others,
/// among `keys` (`None`: there is none).
fn superset_of(keys: impl Iterator<Item = u64>, want: SetSpec, fam: u32) -> Option<u64> {
    let base = want.key() & BASE_MASK;
    keys.filter(|&k| {
        let f = (k >> FEATS_SHIFT) as u32;
        k & BASE_MASK == base && f & want.feats == want.feats && f & !fam == 0
    })
    .min_by_key(|&k| ((k >> FEATS_SHIFT) as u32).count_ones())
}

/// PERF (PERF-D): one set for the builder thread.
struct SetJob {
    spec: SetSpec,
    device: wgpu::Device,
    module: Option<wgpu::ShaderModule>,
    layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    /// a frame wants it (built on more worker threads than a prewarmed one)
    need: bool,
}

/// PERF (PERF-D): a set the builder thread made.
pub(super) struct BuiltSet {
    spec: SetSpec,
    module: wgpu::ShaderModule,
    set: Vec<wgpu::RenderPipeline>,
    ms: f32,
}

#[derive(Default)]
struct BuilderQueue {
    jobs: std::collections::VecDeque<SetJob>,
    closed: bool,
}

type Shared = std::sync::Arc<(std::sync::Mutex<BuilderQueue>, std::sync::Condvar)>;

/// PERF (PERF-D): the mesh pipeline sets built off the frame. One thread takes the jobs in queue order (a set a frame wants goes
/// to the front, a prewarmed one to the back) and builds each on a few worker threads (`create_render_pipeline` is thread-safe:
/// `wgpu::Device` is `Send + Sync`); the sets come back over a channel and are taken in at the next frame.
pub(super) struct SetBuilder {
    shared: Shared,
    tx: std::sync::mpsc::Sender<BuiltSet>,
    rx: std::sync::mpsc::Receiver<BuiltSet>,
    /// the keys queued or being built
    queued: std::collections::HashSet<u64>,
    /// false: the thread could not be started (every set is then built in the frame)
    running: Option<bool>,
}

impl Default for SetBuilder {
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { shared: Default::default(), tx, rx, queued: Default::default(), running: None }
    }
}

impl Drop for SetBuilder {
    fn drop(&mut self) {
        // the thread ends after the set it is building (not joined: the device it holds stays alive until then)
        if let Ok(mut q) = self.shared.0.lock() {
            q.closed = true;
            q.jobs.clear();
        }
        self.shared.1.notify_all();
    }
}

fn builder_thread(shared: Shared, tx: std::sync::mpsc::Sender<BuiltSet>) {
    // a wanted set on half the cores (at most four), a prewarmed one on a quarter: the frame keeps the rest
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let (need_workers, warm_workers) = ((cores / 2).clamp(1, 4), (cores / 4).clamp(1, 2));
    loop {
        let job = {
            let (lock, cv) = &*shared;
            let Ok(mut q) = lock.lock() else { return };
            loop {
                if q.closed {
                    return;
                }
                if let Some(j) = q.jobs.pop_front() {
                    break j;
                }
                match cv.wait(q) {
                    Ok(g) => q = g,
                    Err(_) => return,
                }
            }
        };
        let t0 = std::time::Instant::now();
        let s = job.spec;
        let module = job.module.unwrap_or_else(|| crate::gfx::wgsl_module(&job.device, "mesh shader (parts)", &crate::classes::mesh_shader_source(s.feats)));
        let set = build_mesh_pipelines_on(&job.device, &module, &job.layout, job.format, s.samples, s.temporal, if job.need { need_workers } else { warm_workers });
        if tx.send(BuiltSet { spec: s, module, set, ms: crate::prof::elapsed_ms(t0) as f32 }).is_err() {
            return;
        }
    }
}

impl SetBuilder {
    fn running(&mut self) -> bool {
        *self.running.get_or_insert_with(|| {
            let (shared, tx) = (self.shared.clone(), self.tx.clone());
            match std::thread::Builder::new().name("pipeline sets".into()).spawn(move || builder_thread(shared, tx)) {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!(error = %e, "pipeline set thread not started: the sets are built in the frame");
                    false
                }
            }
        })
    }

    fn push(&mut self, job: SetJob, front: bool) {
        self.queued.insert(job.spec.key());
        let (lock, cv) = &*self.shared;
        if let Ok(mut q) = lock.lock() {
            if front {
                q.jobs.push_front(job);
            } else {
                q.jobs.push_back(job);
            }
        }
        cv.notify_one();
    }

    /// Move a queued set to the front of the queue (nothing when it is being built already).
    fn promote(&mut self, key: u64) {
        let Ok(mut q) = self.shared.0.lock() else { return };
        if let Some(i) = q.jobs.iter().position(|j| j.spec.key() == key)
            && let Some(mut j) = q.jobs.remove(i)
        {
            j.need = true;
            q.jobs.push_front(j);
        }
    }
}

impl Core {
    /// ENHANCEMENT E1: make `pipelines` the set built for `samples` (built once, kept).
    /// PERF (PERF-B): `shut` = the parts whose light this frame has none of (`classes::feat`; their uniforms are cleared), left out.
    ///
    /// PERF (PERF-D): a set not built yet does not hold the frame. The frame draws with a built set of the same family that has
    /// every part of the wanted one and more (the fewest more), while the wanted set is built on the builder thread; the first
    /// frame after it is done takes it. The picture is the wanted set's: the parts beyond it are ones this frame's uniforms
    /// switch off — a shut light's uniform is zeroed (`csm.dir.w`, `spots.params.x`, `lamps.params.x`, `contacts.params.x` 0:
    /// `csm_pass`, `spot_pass`, `lamp_pass`, `contact_pass`; LSHAD runs inside the lamps' loop only), a per-item part tests the
    /// item's own uniform (`fill::ItemVariant::of` is those tests) — which is how the one uber shader drew every frame before
    /// PERF-B. The family's whole set (every part its switches can run: the superset of all its sets) is the one set a frame
    /// waits for, the first time the family (sample count, float / temporal target, the mirror's cut, the switches) is drawn:
    /// built in the frame, or waited for when the race load's prewarm (`prewarm_pipelines`) has it queued.
    pub(super) fn ensure_pipelines(&mut self, samples: u32, hdr: bool, temporal: bool, shut: u32) {
        self.take_built_sets();
        // PERF (PERF-B): the shader has only the parts the switches can run, less those the mirror's pass never runs (its own set)
        let mut fam = crate::classes::mesh_features(&self.enhancements);
        if self.no_reflections {
            fam &= !crate::classes::feat::NOT_IN_MIRROR;
        }
        let spec = |feats: u32| SetSpec { feats, samples, hdr, temporal };
        let full = spec(fam);
        if !self.pipeline_sets.contains_key(&full.key()) {
            self.wait_or_build(full);
            self.prewarm_family(full, shut, PREWARM_DEPTH);
        }
        // PERF (PERF-B): the per-item parts (`classes::feat::SURFACE`: a reflection class; `classes::feat::EFFECT`: a particle /
        // soft / HDR boost role) only in the sets of the items that can take them (`fill::ItemVariant`): variant `v` has the
        // surface parts with bit 0, the effect parts with bit 1. A variant whose set is an earlier one's draws with that set.
        let feats = fam & !shut;
        let mut chosen = [0u64; 4];
        for (v, c) in chosen.iter_mut().enumerate() {
            let want = spec(variant_feats(feats, v));
            if self.exact_sets && !self.pipeline_sets.contains_key(&want.key()) {
                self.queue_set(want, true);
                self.wait_or_build(want);
            }
            *c = if self.pipeline_sets.contains_key(&want.key()) {
                want.key()
            } else {
                self.queue_set(want, true);
                superset_of(self.pipeline_sets.keys().copied(), want, fam).unwrap_or(full.key())
            };
        }
        if chosen == self.pipeline_cur {
            return;
        }
        let mut var_set = [0u8; 4];
        for v in 0..4 {
            var_set[v] = (0..v).find(|&u| chosen[u] == chosen[v]).map_or(v as u8, |u| var_set[u]);
        }
        self.pipelines = self.pipeline_sets[&chosen[0]].clone();
        self.multisampled_set = samples > 1;
        for v in 1..4 {
            self.pipeline_vars[v - 1] = if var_set[v] as usize == v { self.pipeline_sets[&chosen[v]].clone() } else { Vec::new() };
        }
        self.var_set = var_set;
        self.pipeline_cur = chosen;
    }

    /// PERF (PERF-D): take in the sets the builder thread has finished.
    fn take_built_sets(&mut self) {
        while let Ok(b) = self.set_builder.rx.try_recv() {
            self.keep_built(b);
        }
    }

    fn keep_built(&mut self, b: BuiltSet) {
        let key = b.spec.key();
        self.set_builder.queued.remove(&key);
        if b.spec.feats != 0 {
            self.mesh_shaders.entry(b.spec.feats).or_insert(b.module);
        }
        tracing::info!(samples = b.spec.samples, hdr = b.spec.hdr, temporal = b.spec.temporal, parts = format_args!("{:#06x}", b.spec.feats), ms = b.ms, "mesh pipeline set built (builder thread)");
        self.pipeline_sets.entry(key).or_insert(b.set);
    }

    /// PERF (PERF-D): the set `spec` now: waited for when it is queued (moved to the front), else built in the frame.
    fn wait_or_build(&mut self, spec: SetSpec) {
        let key = spec.key();
        if self.pipeline_sets.contains_key(&key) {
            return;
        }
        let t0 = std::time::Instant::now();
        if self.set_builder.queued.contains(&key) {
            self.set_builder.promote(key);
            while !self.pipeline_sets.contains_key(&key) {
                match self.set_builder.rx.recv() {
                    Ok(b) => self.keep_built(b),
                    Err(_) => break,
                }
            }
            if self.pipeline_sets.contains_key(&key) {
                tracing::info!(parts = format_args!("{:#06x}", spec.feats), ms = crate::prof::elapsed_ms(t0) as f32, "mesh pipeline set waited for");
                return;
            }
        }
        if spec.feats != 0 && !self.mesh_shaders.contains_key(&spec.feats) {
            let module = crate::gfx::wgsl_module(&self.device, "mesh shader (parts)", &crate::classes::mesh_shader_source(spec.feats));
            self.mesh_shaders.insert(spec.feats, module);
        }
        let shader = if spec.feats != 0 { &self.mesh_shaders[&spec.feats] } else { &self.mesh_shader };
        let (layout, format) = self.set_target(spec);
        let set = build_mesh_pipelines(&self.device, shader, &layout, format, spec.samples, spec.temporal);
        tracing::info!(samples = spec.samples, hdr = spec.hdr, temporal = spec.temporal, parts = format_args!("{:#06x}", spec.feats), ms = crate::prof::elapsed_ms(t0) as f32, "mesh pipeline set built (in the frame)");
        self.pipeline_sets.insert(key, set);
    }

    /// The layout and colour format of a set: the temporal set's object layout has the matrices and its vector / mask targets
    /// (ENHANCEMENT ENH-TEMPORAL), the float set the scene's float format (ENHANCEMENT E7).
    fn set_target(&self, spec: SetSpec) -> (wgpu::PipelineLayout, wgpu::TextureFormat) {
        if spec.temporal {
            (self.temporal_gpu.as_ref().expect("temporal gpu").pipeline_layout.clone(), HDR_FORMAT)
        } else {
            (self.mesh_layout.clone(), if spec.hdr { HDR_FORMAT } else { self.config.format })
        }
    }

    /// PERF (PERF-D): queue the set `spec` on the builder thread unless it is built or queued (`need`: a frame wants it — to the
    /// front of the queue; else a prewarm — to the back).
    fn queue_set(&mut self, spec: SetSpec, need: bool) {
        let key = spec.key();
        if self.pipeline_sets.contains_key(&key) || (spec.temporal && self.temporal_gpu.is_none()) {
            return;
        }
        if self.set_builder.queued.contains(&key) {
            if need {
                self.set_builder.promote(key);
            }
            return;
        }
        if !self.set_builder.running() {
            // no thread: a wanted set is built now (the frame draws its stand-in until the next), a prewarm is dropped
            if need {
                self.wait_or_build(spec);
            }
            return;
        }
        let (layout, format) = self.set_target(spec);
        let module = if spec.feats == 0 { Some(self.mesh_shader.clone()) } else { self.mesh_shaders.get(&spec.feats).cloned() };
        self.set_builder.push(SetJob { spec, device: self.device.clone(), module, layout, format, need }, need);
    }

    /// PERF (PERF-D): queue (at the back) the sets of the family whose whole set is `full` for the light states at most `depth`
    /// light groups away from `shut`, nearest first, each with its four item variants.
    fn prewarm_family(&mut self, full: SetSpec, shut: u32, depth: u32) {
        let groups: Vec<u32> = LIGHT_GROUPS.iter().copied().filter(|g| full.feats & g != 0).collect();
        let now = groups.iter().enumerate().fold(0u32, |m, (i, g)| if shut & g != 0 { m | 1 << i } else { m });
        let mut states: Vec<u32> = (0..1u32 << groups.len()).filter(|s| (s ^ now).count_ones() <= depth).collect();
        states.sort_by_key(|s| (s ^ now).count_ones());
        for s in states {
            let off = groups.iter().enumerate().fold(0, |m, (i, g)| if s & 1 << i != 0 { m | g } else { m });
            for v in 0..4 {
                self.queue_set(SetSpec { feats: variant_feats(full.feats & !off, v), ..full }, false);
            }
        }
    }

    /// PERF (PERF-D): at a race's load, queue the whole sets of the families the race will draw — the main view, the rear-view
    /// mirror and the environment faces, as their passes will pick them (`encode_with_gamma_inner`, `render_offscreen`,
    /// `refl.rs`) — then the sets of their light states ([`Self::prewarm_family`], from a start in the open by day: every light
    /// on but the street lamps), so the builder thread makes them while the track loads. A family guessed wrong costs only its
    /// build: a frame still waits for what it needs.
    pub(super) fn prewarm_pipelines(&mut self) {
        let fam = crate::classes::mesh_features(&self.enhancements);
        if fam == 0 {
            return;
        }
        let hdr = self.enhancements.is_on(crate::E7) && self.hdr_ok;
        let temporal = hdr && self.temporal_gpu.is_some() && (self.temporal.is_some() || self.temporal_want().is_some());
        let mirror_hdr = hdr && crate::tonemap_mode(self.enhancements.level(crate::E7)) != 0;
        let mut fams = vec![
            SetSpec { feats: fam, samples: if temporal { 1 } else { self.enh_samples(hdr) }, hdr, temporal },
            SetSpec { feats: fam & !crate::classes::feat::NOT_IN_MIRROR, samples: self.enh_samples(mirror_hdr), hdr: mirror_hdr, temporal: false },
        ];
        if self.enhancements.is_on(crate::E8) {
            fams.push(SetSpec { feats: fam, samples: 1, hdr, temporal: false });
        }
        for f in &fams {
            self.queue_set(*f, false);
        }
        // (the main view meets the most light states; the others are prewarmed in their start state only)
        // (ENHANCEMENT E50: and no moon)
        // (SHADOW-REFACTOR: nor the front end's focus or E25's overcast kernels)
        let start = crate::classes::feat::LAMPS | crate::classes::feat::LSHAD | crate::classes::feat::MOON | crate::classes::feat::FOCUS | crate::classes::feat::OVERCAST;
        for (i, f) in fams.into_iter().enumerate() {
            self.prewarm_family(f, start, if i == 0 { PREWARM_DEPTH } else { 0 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classes::feat;

    /// PERF-D: the stand-in has every wanted part, none outside the family, the fewest others; the family's whole set qualifies.
    #[test]
    fn the_stand_in_is_the_smallest_superset() {
        let fam = feat::CSM | feat::SPOT | feat::COAT | feat::PFX;
        let s = |feats| SetSpec { feats, samples: 4, hdr: true, temporal: false };
        let other_family = SetSpec { feats: feat::COAT, samples: 1, hdr: true, temporal: false };
        let keys = [s(fam).key(), s(feat::CSM | feat::COAT | feat::PFX).key(), s(feat::COAT | feat::FOGSUN).key(), other_family.key()];
        assert_eq!(superset_of(keys.iter().copied(), s(feat::COAT), fam), Some(s(feat::CSM | feat::COAT | feat::PFX).key()));
        assert_eq!(superset_of(keys.iter().copied(), s(feat::SPOT), fam), Some(s(fam).key()));
        assert_eq!(superset_of(keys[2..].iter().copied(), s(feat::COAT), fam), None);
        // variant 0 has no per-item part, variant 3 all of them
        assert_eq!(variant_feats(fam, 0), feat::CSM | feat::SPOT);
        assert_eq!(variant_feats(fam, 3), fam);
    }

    /// The keys' indices are the hand-written scheme they replaced: `group * 6 + 2 * cull + !depth`, the blend groups `1 + pair`
    /// and `5 + pair` (with depth writes), E1's alpha-to-coverage group 9 when the set has more than 66 pipelines, the layered
    /// groups `len / 6 - 2` and one after it; and `PipelineKey::set` lists every key once, in index order.
    #[test]
    fn the_pipeline_keys_are_the_old_index_scheme() {
        for ms in [false, true] {
            let len = if ms { 72 } else { 66 };
            let keys: Vec<_> = PipelineKey::set(ms).collect();
            assert_eq!(keys.len(), len);
            for (i, k) in keys.iter().enumerate() {
                assert_eq!(k.index(ms), i, "{k:?}");
            }
            for (ci, cull) in Cull::ALL.into_iter().enumerate() {
                for depth_test in [true, false] {
                    let sub = ci * 2 + !depth_test as usize;
                    let idx = |group| PipelineKey { group, cull, depth_test }.index(ms);
                    assert_eq!(idx(Group::Opaque), sub);
                    for pair in 0..4u8 {
                        let plain = if pair > 0 { 1 + pair as usize } else { 1 };
                        assert_eq!(idx(Group::Blend { pair, zwrite: false }), plain * 6 + sub);
                        assert_eq!(idx(Group::Blend { pair, zwrite: true }), (5 + pair as usize) * 6 + sub);
                    }
                    assert_eq!(idx(Group::AlphaToCoverage), if len > 66 { 9 * 6 + sub } else { sub });
                    let first = len / 6 - 2;
                    assert_eq!(idx(Group::Layered { zwrite: false }), first * 6 + sub);
                    assert_eq!(idx(Group::Layered { zwrite: true }), (first + 1) * 6 + sub);
                }
            }
        }
    }
}
