//! The groups of [`Core`]'s fields that belong together: the sampler tables, the dummy resources the bindings hold until the real
//! ones exist, the bind group / pipeline layouts, the lens flare's read-backs, the diagnostics and the draw-state switches. Plain
//! data: the code that fills and reads them stays in the `Core` modules.

use super::*;

/// The scene's samplers and the sprite pass's, by address mode, mip filter and ENHANCEMENT E2's level.
pub(super) struct Samplers {
    /// the scene samplers by address mode and mip filter — index `clamp_u | clamp_v << 1 |
    /// point_mip << 2`
    pub(super) scene: [wgpu::Sampler; 8],
    /// the `FakeSort` single-draw arm's samplers (Thrash 7 ← 0, `MAG`/`MINFILTER` `POINT`), by
    /// address mode alone — index `clamp_u | clamp_v << 1` — used only for a single-draw item
    pub(super) point: [wgpu::Sampler; 4],
    /// ENHANCEMENT E2, E2 off: `scene` with the level range cut to level 0 — for a texture with
    /// a generated chain, which then samples exactly as the original's single level, by address mode
    pub(super) level0: [wgpu::Sampler; 4],
    /// the same for the `FakeSort` single-draw arm's point samplers
    pub(super) point_level0: [wgpu::Sampler; 4],
    /// ENHANCEMENT E2, E2 on: trilinear + anisotropic scene samplers by address mode, built for
    /// `aniso_clamp` (rebuilt when the level changes)
    pub(super) aniso: [wgpu::Sampler; 4],
    pub(super) aniso_clamp: u16,
    /// the adapter's `ANISOTROPIC_FILTERING` (downlevel flag); without it E2 still gives the
    /// generated mips and trilinear, with the clamp at 1
    pub(super) aniso_supported: bool,
    /// the 2D pass's sampler
    pub(super) sprite: wgpu::Sampler,
    /// the projector's sampler, trilinear and with the `POINT` mip filter (`modes.trilinear` picks)
    pub(super) proj: [wgpu::Sampler; 2],
}

/// What a binding holds until the real resource exists (never sampled / read then), with the textures the views keep alive.
pub(super) struct Dummies {
    pub(super) _proj_default: wgpu::Texture,
    pub(super) proj_default_view: wgpu::TextureView,
    pub(super) refl_view: wgpu::TextureView,
    pub(super) _refl: wgpu::Texture,
    pub(super) soft_view: wgpu::TextureView,
    pub(super) _soft: wgpu::Texture,
    /// ENHANCEMENT E16: the probe's uniform while there is none
    pub(super) probe: wgpu::Buffer,
}

/// The bind group layouts of the 3-D passes and the sprite pass, and the mesh pipelines' layout.
pub(super) struct Layouts {
    pub(super) globals: wgpu::BindGroupLayout,
    pub(super) material: wgpu::BindGroupLayout,
    /// the car's section map for `R3DSkyMapTextureLayer`, one bind group per texture (group 3)
    pub(super) sky: wgpu::BindGroupLayout,
    pub(super) sprite: wgpu::BindGroupLayout,
    pub(super) mesh: wgpu::PipelineLayout,
}

/// ENHANCEMENT E7 / the lens flare's frame read-backs ([`Core::set_lens_enabled`]).
pub(super) struct Lens {
    /// the surface carries `COPY_SRC`, so the lens flare's frame read-back can copy from it
    /// ([`Core::set_lens_enabled`])
    pub(super) enabled: bool,
    /// read-backs issued and not yet collected, oldest first
    pub(super) pending: Vec<LensPending>,
    /// read-backs collected, for the front to hand to the game
    pub(super) done: Vec<LensRead>,
    /// blocks copied into the frame being encoded, mapped after its submit
    pub(super) staged: Vec<(wgpu::Buffer, bool, bool)>,
    /// read-back buffers collected and unmapped, for the next copies ([`LENS_POOL`])
    pub(super) free: Vec<wgpu::Buffer>,
}

/// Diagnostics and the error fallback: nothing here draws.
pub(super) struct Diag {
    /// `NFS_GPU_TIME` (read once) and its running average
    pub(super) gpu_time: crate::prof::WallTimer,
    /// TEST GLUE (`--perf-bench`): the bench frames' target, kept between frames (width, height, texture, view)
    pub(super) bench_target: Option<(u32, u32, wgpu::Texture, wgpu::TextureView)>,
    /// ENHANCEMENT E33 diagnostic: EDR frames drawn (the probe measures the 20th)
    #[cfg(target_os = "macos")]
    pub(super) edr_frames: u32,
    pub(super) gpu_errors_seen: u32,
    /// WIN-STARTUP: a GPU error forced every enhancement off for this run ([`GPU_ERRORS`]); later sets are ignored
    pub(super) enh_forced_off: bool,
}

/// The draw state the command list sets (`set_trilinear`, `set_fakesort_mode`, `set_gamma`, `set_mirror_view`).
pub(super) struct DrawModes {
    /// the mip filter's `POINT` → `LINEAR` promotion, `[0x69DD1D]` (`set_trilinear`); clear at start, as the image has it
    pub(super) trilinear: bool,
    /// `LayerFalseSort1`/`2`'s per-frame pick, [`FakeSortMode`] — `main.rs` sets it from
    /// `SPECIALFX` and, at level 2, the runtime globals 0x44EC80 reads to the instruction
    /// (`RE/notes/thrash.md` §6d.4).
    pub(super) fakesort_mode: FakeSortMode,
    /// The display gamma, state **0x2E** (`setstatef`, `[0x6BD918]`, with the value
    /// `max(0.25, Graphics.GammaCorrection · 0.01)`, 0x44ED10 / 0x4DED10 / 0x4E5A70 / 0x511900).
    /// `dx7z.dll`'s handler 0x600051D4 clamps it to `0…4` and multiplies the primary surface's
    /// gamma ramp — the one it read at the mode set (0x600032B5, `GetGammaRamp`) — by it, each
    /// word truncated (0x6000CB74) and capped at 0xFFFF, then `SetGammaRamp` (vtable `+0x10`). So it
    /// is a scale of every displayed channel after the whole frame is drawn: [`Core::set_gamma`],
    /// applied by [`GammaPass`]. 1.0 (the default 100) draws straight into the target as before.
    pub(super) gamma: f32,
    /// `[0x6115CC]+0x190`: set while the view being drawn is the rear-view mirror's (type 1,
    /// 0x43AD39…0x43AD4E), when the specular and sky-map layers take no texture and skip their passes
    pub(super) no_reflections: bool,
}

/// The meshes and textures the GPU holds and their tables: the mesh arena and its free slots, the texture list, the released /
/// twin bookkeeping (ENHANCEMENT ENH-LIVELEAK, E41 / E42), the materials, and the writes deferred to the frame's encoder.
pub(super) struct Resources {
    pub(super) meshes: Vec<GpuMesh>,
    pub(super) arena: Option<MeshArena>,
    /// ENHANCEMENT ENH-LIVELEAK: the ranges of released static meshes, taken by a later upload that fits (best fit)
    pub(super) free_slots: Vec<FreeMeshSlot>,
    /// ENHANCEMENT ENH-LIVELEAK: the empty buffers every released mesh slot points at, and their serial ([`GpuMesh::vb_serial`])
    pub(super) mesh_stub: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    /// ENHANCEMENT ENH-LIVELEAK: the bytes of every arena chunk and dynamic mesh buffer made so far (`ResourceStats::arena_bytes`)
    pub(super) arena_bytes: u64,
    pub(super) textures: Vec<GpuTexture>,
    /// ENHANCEMENT ENH-LIVELEAK: the 1 x 1 white image every released texture slot shares, and the slots released
    pub(super) stub_texture: Option<GpuTexture>,
    pub(super) released_textures: std::collections::HashSet<u32>,
    /// ENHANCEMENT ENH-LIVELEAK: released texture / mesh slots. They wait in `pending_*` until [`Self::recycle_released`] (the end of a live
    /// pass: every model holding the old id has let go of it by then); a later upload takes a slot from `free_*` instead of growing the table.
    pub(super) pending_tex: Vec<u32>,
    pub(super) free_tex: Vec<u32>,
    pub(super) pending_mesh: Vec<u32>,
    pub(super) free_mesh: Vec<u32>,
    /// ENHANCEMENT ENH-LIVETEX (E41): a texture's HD twin, by the original's index; a material samples the twin while E41 is above 0
    /// (`Self::shown_texture`), the original otherwise, so the A/B key / the menu flip between the two without a reload
    pub(super) tex_twin: Vec<Option<TextureId>>,
    /// ENHANCEMENT ENH-LIVETEX (E42): a mesh's refined twin (a car's rounded part), drawn in place of the original while E42 is above 0
    pub(super) mesh_twin: Vec<Option<MeshId>>,
    pub(super) materials: Vec<GpuMaterial>,
    pub(super) white: TextureId,
    /// `write_mesh_vertices` / `update_mesh_vertices` calls waiting for the next frame's encoder,
    /// in call order (the last write to a mesh wins, as with `write_buffer`)
    pub(super) pending_writes: Vec<PendingWrite>,
    pub(super) pending_bytes: Vec<u8>,
    /// `update_texture` calls waiting for the next frame's encoder, in call order, their rows padded
    /// to the copy's 256-byte alignment (`Queue::write_texture` makes a fresh staging buffer per call)
    pub(super) pending_textures: Vec<PendingTextureWrite>,
    pub(super) pending_texture_bytes: Vec<u8>,
    /// the next serial for a vertex / index buffer pair ([`GpuMesh::vb_serial`])
    pub(super) buffer_serial: u32,
}

/// The mesh pipelines: the current set, the sets built so far by key, the per-class variants, the shader modules they are built from.
pub(super) struct PipelineCache {
    /// the mesh pipelines of the set the frame draws with, by [`pipelines::PipelineKey::index`]
    pub(super) pipelines: Vec<wgpu::RenderPipeline>,
    /// ENHANCEMENT E1 / E7: every set built (key: the sample count, | [`HDR_KEY`] for the float set, the shader's parts above
    /// bit 16; 1 = master's 66), kept so a switch back is free
    pub(super) sets: std::collections::HashMap<u64, Vec<wgpu::RenderPipeline>>,
    /// PERF (PERF-B): the sets of item variants 1..=3 (`classes::feat::SURFACE` / `EFFECT` parts, `ensure_pipelines`; empty =
    /// the variant draws with an earlier set, `var_set`); variant 0 is `pipelines`
    pub(super) vars: [Vec<wgpu::RenderPipeline>; 3],
    /// PERF (PERF-D): the keys of the sets the four item variants draw with (the wanted ones, or their stand-ins while those
    /// are built)
    pub(super) cur: [u64; 4],
    /// PERF (PERF-B): an item variant's set (0 = `pipelines`, k = `vars[k - 1]`)
    pub(super) var_set: [u8; 4],
    /// PERF (PERF-D): the sets built off the frame (`pipelines.rs`)
    pub(super) builder: pipelines::SetBuilder,
    /// PERF (PERF-D): a frame waits for its exact sets instead of drawing with a stand-in (`Renderer::set_exact_pipelines`)
    pub(super) exact: bool,
    /// the mesh shader of the original's set (no part, `classes::mesh_shader_source(0)`)
    pub(super) shader: wgpu::ShaderModule,
    /// PERF (PERF-B): the mesh shader by its parts (`classes::feat`), built when a set of switches first needs it
    pub(super) shaders: std::collections::HashMap<u32, wgpu::ShaderModule>,
    /// ENHANCEMENT E1: the sets drawn with are multisampled (they have the alpha-to-coverage group)
    pub(super) multisampled: bool,
}

/// ENHANCEMENT E8 / E16 / E49 / ENH-COAT: the environment cube and what is made from it (the car-ambient probe, the clear-coat
/// prefilter, the near-car lookup's mask).
pub(super) struct ReflState {
    /// ENHANCEMENT E8: the six environment faces (a `D2` array of `refl_size`²), the 1 × 1 black array
    /// the binding holds while there is none, and the render target a face is drawn into before it is
    /// copied into its layer (a layer cannot be an attachment while the array is bound)
    pub(super) cube: Option<ReflCube>,
    pub(super) building: bool,
    /// ENHANCEMENT E49: the environment face being drawn keeps its depth for the near cars' pass that follows (`Keep`), or is that
    /// pass, drawn over the face's colour and depth (`Over`); `None` = every other pass (E8's faces as they were)
    pub(super) near_pass: Option<NearPass>,
    /// ENHANCEMENT E49: the near cars' pass first copies the face into this layer (the face without them)
    pub(super) pre_copy: Option<u32>,
    /// ENHANCEMENT E49 (NEAR-REFL2): the pass that marks a face's drawn geometry in its alpha (`refl::NEAR_MASK_WGSL`), made on first
    /// use for the cube's format
    pub(super) near_mask: Option<(wgpu::TextureFormat, wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    /// ENHANCEMENT E16: the SH projection of the faces (built on first use; `None` without compute
    /// shaders), and the zero uniform the binding holds until then
    pub(super) probe: Option<crate::probe::Probe>,
    /// ENHANCEMENT ENH-COAT: the prefilter of the environment cube's mips (made with the first cube), and whether an
    /// environment face is being drawn now (its frame must not run the prefilter: the faces are not all in yet)
    pub(super) coat: Option<crate::coat::CoatFilter>,
}
