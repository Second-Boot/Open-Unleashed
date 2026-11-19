//! The device, the surface and the start-up: `Core::new` (adapter, limits, every layout / pipeline / dummy the renderer starts with),
//! the instance and the surface's size / present mode / EDR switch.

use crate::gfx;
use super::*;

impl Core {
    pub fn new(window: Arc<Window>) -> Result<Self> {
        let DeviceSetup { _instance, surface, adapter, device, queue, config, edr, present_modes, ms_supported, hdr_ok, ms_supported_hdr, gpu_class } = Self::create_device(window)?;
        let format = config.format;
        let depth_view = Self::make_depth(&device, &config);

        let MeshSetup { shader, globals_layout, object_layout, material_layout, sky_layout, csm_state, spot_state, lamp_state, contact_state, pipeline_layout, pipelines } = Self::create_mesh_setup(&device, format);

        let globals_buf = gfx::uniform_buf::<GlobalsUniform>(&device, "globals");
        let headlight_map_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headlight map"),
            size: NIGHT_MAP_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // The projector's texture is the one its track's `sh80` material loaded: 0x44B000 →
        // 0x44D320 only looks it up, by the part's key `(+0x18, +0x1C)`, which on each of the five
        // tracks with projectors is that material's `(mt+0x24, mt+0x28)` (`texblock` example). The
        // material's block is `{…, bias −1, 0, 0}` on every one — `ADDRESSU` / `ADDRESSV` `CLAMP`
        // (`RE/notes/thrash.md` §7) — and `sh80` has 7 mips, so the mip filter is the loader's
        // `POINT`, `LINEAR` at `SPECIALFX` 3 (0x44D571, 0x535A60); the bias is the shader's. (Until
        // 2026-09-25 this sampler clamped as an approximation and read the top level only.) The
        // `HEAD` decal samples its top level explicitly through the same sampler: `common.fsh`
        // loads with no state block (0x44D270 → 0x44D2E9 pushes 0), so `HEAD` keeps the
        // constructor's default 0x5379D0 — `CLAMP` / `CLAMP`, `LINEAR`, mip `NONE`
        // (`RE/notes/lighting.md` §5.2).
        // `nfs5.ini` `Filtering=0` (dll 0x100035F0): the wrapper sends Thrash 7 (`MAG`/`MINFILTER`) as 0 =
        // `POINT` whatever the state block says (the block's value is 1, `LINEAR`) - every sampler below
        // takes it; the mip filters (state 0x0B) are untouched
        let mag_min_linear = if crate::point_filter() { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
        let make_proj_sampler = |mip: wgpu::MipmapFilterMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("projector clamp"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: mag_min_linear,
                min_filter: mag_min_linear,
                mipmap_filter: mip,
                ..Default::default()
            })
        };
        let proj_samplers = [make_proj_sampler(wgpu::MipmapFilterMode::Linear), make_proj_sampler(wgpu::MipmapFilterMode::Nearest)];
        let proj_default = gfx::texture_2d(&device, "projector default", 1, 1, TEXTURE_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            kit::level_copy(&proj_default, 0),
            &[255, 255, 255, 0],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            kit::extent(1, 1),
        );
        let proj_default_view = proj_default.create_view(&wgpu::TextureViewDescriptor::default());
        // ENHANCEMENT E8: the array the binding holds until the first face is drawn (never sampled:
        // `Scene::refl` is 0 then)
        let refl_dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("refl dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 6 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TEXTURE_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let refl_dummy_view = refl_dummy.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let soft_dummy = gfx::texture_2d(&device, "soft dummy", 1, 1, crate::soft::SOFT_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        let soft_dummy_view = soft_dummy.create_view(&Default::default());
        // ENHANCEMENT E16: the probe's uniform while there is none (never read: `globals.refl.z` is 0 then)
        let probe_dummy = device.create_buffer(&wgpu::BufferDescriptor { label: Some("probe sh dummy"), size: crate::probe::SH_COEFFS as u64 * 16, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: false });
        // (the projector sampler is the trilinear one here: `trilinear` starts false and every rebuild takes its sampler,
        // `make_globals_bg`; this first group is the one the frames before the first rebuild bind, as it always was)
        let globals_bg = materials::GlobalsSet {
            globals: &globals_buf,
            night_map: &headlight_map_buf,
            proj: &proj_default_view,
            proj_sampler: &proj_samplers[0],
            head: &proj_default_view,
            refl: &refl_dummy_view,
            csm: &csm_state,
            soft: &soft_dummy_view,
            probe_sh: &probe_dummy,
            spot: &spot_state,
            lamp: &lamp_state,
            contacts: &contact_state.buf,
        }
        .bind_group(&device, &globals_layout);
        let object_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: std::mem::size_of::<ObjectUniform>() as u64 * MAX_OBJECTS,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let object_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("objects"),
            layout: &object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &object_buf,
                    offset: 0,
                    size: None,
                }),
            }],
        });
        let SceneSamplers { samplers, point_samplers, level0_samplers, point_level0_samplers, compute_supported, aniso_supported, aniso_samplers, sprite_sampler } = Self::create_samplers(&device, &adapter);
        let SpriteSetup { sprite_layout, sprite_pipeline, sprite_flat_pipeline, sprite_depth_pipelines, belt, sprite_buf } = Self::create_sprite_setup(&device, format);
        let mut r = Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            res: state::Resources {
                meshes: Vec::new(),
                arena: None,
                free_slots: Vec::new(),
                mesh_stub: None,
                arena_bytes: 0,
                textures: Vec::new(),
                stub_texture: None,
                released_textures: std::collections::HashSet::new(),
                pending_tex: Vec::new(),
                free_tex: Vec::new(),
                pending_mesh: Vec::new(),
                free_mesh: Vec::new(),
                tex_twin: Vec::new(),
                mesh_twin: Vec::new(),
                materials: Vec::new(),
                white: TextureId(0),
                pending_writes: Vec::new(),
                pending_bytes: Vec::new(),
                pending_textures: Vec::new(),
                pending_texture_bytes: Vec::new(),
                buffer_serial: 0,
            },
            pipes: state::PipelineCache {
                pipelines: pipelines.clone(),
                sets: std::collections::HashMap::from([(1, pipelines.clone())]),
                vars: [Vec::new(), Vec::new(), Vec::new()],
                cur: [1; 4],
                var_set: [0; 4],
                builder: Default::default(),
                exact: false,
                shader,
                shaders: std::collections::HashMap::new(),
                multisampled: false,
            },
            env: state::ReflState { cube: None, building: false, near_pass: None, pre_copy: None, near_mask: None, probe: None, coat: None },
            samplers: state::Samplers {
                scene: samplers,
                point: point_samplers,
                level0: level0_samplers,
                point_level0: point_level0_samplers,
                aniso: aniso_samplers,
                aniso_clamp: 1,
                aniso_supported,
                sprite: sprite_sampler,
                proj: proj_samplers,
            },
            dummies: state::Dummies { _proj_default: proj_default, proj_default_view, refl_view: refl_dummy_view, _refl: refl_dummy, soft_view: soft_dummy_view, _soft: soft_dummy, probe: probe_dummy },
            layouts: state::Layouts { globals: globals_layout, material: material_layout, sky: sky_layout, sprite: sprite_layout, mesh: pipeline_layout },
            lens: state::Lens { enabled: false, pending: Vec::new(), done: Vec::new(), staged: Vec::new(), free: Vec::new() },
            diag: state::Diag {
                gpu_time: crate::prof::WallTimer::new(crate::dbg::flags().gpu_time),
                bench_target: None,
                #[cfg(target_os = "macos")]
                edr_frames: 0,
                gpu_errors_seen: 0,
                enh_forced_off: false,
            },
            modes: state::DrawModes { trilinear: false, fakesort_mode: FakeSortMode::default(), gamma: 1.0, no_reflections: false },
            globals_buf,
            globals_bg,
            headlight_map_buf,
            proj_tex: None,
            head_tex: None,
            sky_bgs: Vec::new(),
            compute_supported,
            soft: None,
            soft_globals: None,
            ssao: None,
            shafts: None,
            haze: None,
            lbeam: None,
            base_vertex: adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::BASE_VERTEX),
            gpu_class,
            gamma_pass: None,
            enhancements: crate::Enhancements::ORIGINAL,
            present_modes,
            ms_supported,
            hdr_ok,
            edr,
            ms_supported_hdr,
            cur_hdr: None,
            scale_blit_mirror: None,
            temporal: crate::temporal::Config::from_env().map(crate::temporal::Temporal::new),
            temporal_gpu: None,
            cur_temporal: None,
            motion: Default::default(),
            temporal_inputs: None,
            item_var: Vec::new(),
            ms_cache: std::collections::HashMap::new(),
            scale_blit: None,
            up: Upscaling::default(),
            cur_lod_shift: 0.0,
            smaa: None,
            post: Default::default(),
            object_buf,
            object_bg,
            sprite_pipeline,
            sprite_flat_pipeline,
            sprite_depth_pipelines,
            offscreen: std::collections::HashMap::new(),
            sprite_buf,
            belt,
            frame_cmds: FrameCmds::default(),
            frame_main: false,
            depth_cache: std::collections::HashMap::new(),
            buckets: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            solid: Vec::new(),
            frame_scratch: FrameScratch::default(),
            csm: csm_state,
            car_sun: None,
            spot: spot_state,
            lamp: lamp_state,
            contact: contact_state,
            frames_in_flight: std::collections::VecDeque::with_capacity(MAX_FRAMES_IN_FLIGHT + 1),
            last_submit: None,
        };
        r.res.white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Ok(r)
    }

    /// The mesh shader, the four bind group layouts of the 3-D passes, the shadow / spot / lamp / contact states and the
    /// mesh pipelines of the 8-bit set.
    fn create_mesh_setup(device: &wgpu::Device, format: wgpu::TextureFormat) -> MeshSetup {
        let shader = gfx::wgsl_module(device, "mesh shader", &crate::classes::mesh_shader_source(0));
        let csm_entries = csm::layout_entries();
        let spot_entries = spot::layout_entries();
        use crate::classes::binding as b;
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                gfx::bgl_uniform(b::GLOBALS, wgpu::ShaderStages::VERTEX_FRAGMENT),
                // the 128 × 128 `nght` bytes, four to a u32
                gfx::bgl_uniform(b::NIGHT_MAP, wgpu::ShaderStages::VERTEX),
                // the car shadow projector's texture (`[0x6115CC]+0x18C`) and its sampler
                gfx::bgl_float(b::PROJ_TEX, true),
                gfx::bgl_sampler(b::PROJ_SAMPLER, wgpu::SamplerBindingType::Filtering),
                // `HEAD` of `Render/common.fsh`, the headlight decal `LayerUVLight` projects
                gfx::bgl_float(b::HEAD_TEX, true),
                // ENHANCEMENT E8: the six environment faces of the player's car
                wgpu::BindGroupLayoutEntry {
                    binding: b::REFL,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                // ENHANCEMENT E9: the sun shadow map (6), its comparison sampler (7) and the cascades (8)
                csm_entries[0],
                csm_entries[1],
                csm_entries[2],
                // ENHANCEMENT E12: the opaque part's raw depth (R32Float, read with textureLoad)
                gfx::bgl_float(b::SOFT, false),
                // ENHANCEMENT E16: the car ambient probe's SH coefficients (9 x vec4)
                gfx::bgl_uniform(b::PROBE_SH, wgpu::ShaderStages::FRAGMENT),
                // ENHANCEMENT E15: the headlight spots' shadow map (10) and the lamps (11)
                spot_entries[0],
                spot_entries[1],
                // ENHANCEMENT E23: the street lamps
                lamps::layout_entry(),
                // ENHANCEMENT E47: the lamps' car shadow maps
                lamps::shadow_layout_entry(),
                // ENHANCEMENT E43: the cars' contact shadow
                contact::layout_entry(),
            ],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                // every object of a pass in one buffer, indexed by `instance_index`
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<ObjectUniform>() as u64),
                },
                count: None,
            }],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                gfx::bgl_float(0, true),
                gfx::bgl_sampler(1, wgpu::SamplerBindingType::Filtering),
                gfx::bgl_float(2, true),
                // the specular texture's sampler: `WRAP` both ways, set on each map by the binder
                // 0x440940 (0x537A70(1) → `tex+0x38` block `+0x14` / `+0x18`, Thrash 0x41 / 0x42)
                gfx::bgl_sampler(3, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let sky_layout = gfx::bgl(device, "sky map", &[gfx::bgl_float(0, true)]);
        let csm_state = csm::CsmState::new(device, &object_layout, &material_layout);
        let spot_state = spot::SpotState::new(device, &csm_state.light_layout);
        let lamp_state = lamps::LampState::new(device, &csm_state.light_layout);
        let contact_state = contact::ContactState::new(device, "contact uniform");
        let pipeline_layout = gfx::pipeline_layout(device, "mesh pipeline layout", &[&globals_layout, &object_layout, &material_layout, &sky_layout]);
        let pipelines = build_mesh_pipelines(device, &shader, &pipeline_layout, format, 1, false);
        MeshSetup { shader, globals_layout, object_layout, material_layout, sky_layout, csm_state, spot_state, lamp_state, contact_state, pipeline_layout, pipelines }
    }

    /// The scene's samplers (by address mode, filter and E2's level) and the sprite pass's.
    fn create_samplers(device: &wgpu::Device, adapter: &wgpu::Adapter) -> SceneSamplers {
        // A texture's own state block (0x5379D0, `RE/notes/thrash.md` §7) asks for linear
        // magnification and minification (Thrash 7 = 1 → `MAG`/`MINFILTER` `LINEAR`), and a mip
        // filter that is `POINT` for a texture with mips (0x44D571…0x44D587) and becomes `LINEAR`
        // at `SPECIALFX` 3, the default (0x535A60). No Thrash state reaches `MAXANISOTROPY` or an
        // anisotropic filter, so there is none here (until 2026-09-24 this sampler clamped
        // anisotropy at 8, the port's own). The promotion to `LINEAR` is `[0x69DD1D]`, which
        // 0x44ED10 clears and sets again only at `SPECIALFX` 3 (0x44EDD1, 0x44EF04) and the front
        // end's 0x44F020 clears (0x44F080) — `set_trilinear`; below it the mip filter is `POINT`.
        // `nfs5.ini` `Filtering=0`: `POINT` magnification / minification (see the projector sampler)
        let mag_min_linear = if crate::point_filter() { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
        use materials::SamplerKey;
        let samplers = std::array::from_fn(|i| {
            let k = SamplerKey::of_index(i);
            let mip = if k.point_mips { wgpu::MipmapFilterMode::Nearest } else { wgpu::MipmapFilterMode::Linear };
            SamplerSpec::of_key(k, mag_min_linear, mip).build(device, "scene")
        });
        // `LayerFalseSort1`/`2`'s other arm sets Thrash 7 ← 0 for the one draw (`MAG`/`MINFILTER`
        // `POINT`) and never touches state 0x0B, so the mip filter here follows the same `POINT`
        // default as every other sampler below `SPECIALFX` 3 (`trilinear` is false whenever
        // `fakesort_mode` can pick `Single`/`PerItem` — both are set from the same level, `main.rs`)
        let point = |i| SamplerSpec::of_key(SamplerKey::of_address(i), wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest);
        let point_samplers = std::array::from_fn(|i| point(i).build(device, "scene"));
        let level0_samplers = std::array::from_fn(|i| SamplerSpec::of_key(SamplerKey::of_address(i), wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Nearest).level0().build(device, "scene E2"));
        let point_level0_samplers = std::array::from_fn(|i| point(i).level0().build(device, "scene point E2 level 0"));
        let compute_supported = adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
        let aniso_supported = adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::ANISOTROPIC_FILTERING);
        tracing::info!(aniso_supported, "anisotropic filtering (ENHANCEMENT E2)");
        let aniso_samplers = std::array::from_fn(|i| SamplerSpec::of_key(SamplerKey::of_address(i), wgpu::FilterMode::Linear, wgpu::MipmapFilterMode::Linear).build(device, "scene E2"));

        // The 2D pass needs its own sampler. The 3D one once asked for `anisotropy_clamp: 8` and
        // `mipmap_filter: Linear`, which on a single-level texture — every sprite atlas is one —
        // sampled by a quad a dozen pixels across returns nothing on Metal. That is the fault
        // recorded as `RE/AUDIT.md` F2: a glyph quad drawn with a 1×1 texture composites fine while
        // the same quad with the font atlas draws nothing at all, whatever its UV window.
        let sprite_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: mag_min_linear,
            min_filter: mag_min_linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            lod_max_clamp: 0.0,
            ..Default::default()
        });
        SceneSamplers { samplers, point_samplers, level0_samplers, point_level0_samplers, compute_supported, aniso_supported, aniso_samplers, sprite_sampler }
    }

    /// The 2D sprite pass: its layout, pipelines, vertex buffer, and the frame's staging belt.
    fn create_sprite_setup(device: &wgpu::Device, format: wgpu::TextureFormat) -> SpriteSetup {
        // 2D sprite pipeline (HUD)
        //
        // Blended for every sprite, the front end's included (`RE/notes/thrash.md` §8, settled
        // 2026-09-26): the FE kit 0x4D8E10 has an opaque `Rs` block `[0x65B948]` (Thrash 0x0A ← 0)
        // that the flush 0x4DA5F0 picks by the queued item's byte `+8`, but only the untextured
        // fills set it — 0x4D9280 / 0x4D96B0 / 0x4D98D0 from colours that are all `0xFF` alpha,
        // where blending gives the same pixel — while every image draw takes it from its caller
        // (0x4D9B30's arg 8, also via 0x4D9AF0's arg 6; 0x4D9E70's arg 12, also via 0x4D9E10's
        // arg 10), and all 72 call sites pass 0. `dx7z.dll` turns `ALPHABLENDENABLE` on or off
        // only for Thrash 0x0A (0x60003EFE) and at device creation; binding a texture
        // (0x60008000 → 0x60007F30) sets nothing else. The device's alpha test (`GREATER`,
        // reference 0 / 1 / 10 by what the front end drew last) is carried only for a sprite
        // under a depth state (`Sprite::depth`), where it decides which pixels write depth; on a
        // plain sprite it would drop texels of alpha ≤ 10 that blending lays at ≤ 4 %.
        let sprite_shader = gfx::wgsl_module(device, "sprite", include_str!("../sprite.wgsl"));
        let sprite_layout = gfx::bgl(device, "sprite", &[gfx::bgl_float(0, true), gfx::bgl_sampler(1, wgpu::SamplerBindingType::Filtering)]);
        let sprite_pl = gfx::pipeline_layout(device, "sprite", &[&sprite_layout]);
        let sprite_vertex = [Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<SpriteVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32] })];
        let make_sprite_pipeline = |label: &str, fs: &str, depth: Option<wgpu::DepthStencilState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&sprite_pl),
                vertex: wgpu::VertexState { module: &sprite_shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &sprite_vertex },
                fragment: Some(wgpu::FragmentState { module: &sprite_shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: depth,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sprite_pipeline = make_sprite_pipeline("sprite", "fs_main", Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }));
        // the same for a pass with no depth image: a frame with no depth-state sprite (the race's HUD) tests and writes no depth
        let sprite_flat_pipeline = make_sprite_pipeline("sprite (no depth)", "fs_main", None);
        // A front-end sprite under the kit's depth state (`crate::SpriteDepth`): Thrash 0x39 ← 1
        // (0x4D30BE) and the compare Thrash 0x28 sets — 3 `LESSEQUAL` (0x4D30C9), 6 `GREATEREQUAL`
        // inside a mask (0x4DAAC0 → 0x537830) — with the device's alpha test in the shader.
        let depth_state = |compare| Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true), depth_compare: Some(compare), stencil: Default::default(), bias: Default::default() });
        let sprite_depth_pipelines = [
            make_sprite_pipeline("sprite lessequal", "fs_alpha_test", depth_state(wgpu::CompareFunction::LessEqual)),
            make_sprite_pipeline("sprite greaterequal", "fs_alpha_test", depth_state(wgpu::CompareFunction::GreaterEqual)),
        ];
        let belt = wgpu::util::StagingBelt::new(device.clone(), BELT_CHUNK);
        let sprite_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("sprites"), size: MAX_SPRITES * 6 * std::mem::size_of::<SpriteVertex>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        SpriteSetup { sprite_layout, sprite_pipeline, sprite_flat_pipeline, sprite_depth_pipelines, belt, sprite_buf }
    }

    pub(super) fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
        Self::make_depth_sized(device, config.width, config.height)
    }

    pub(super) fn make_depth_sized(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: kit::extent(width, height),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                // ENHANCEMENT E12 reads the opaque part's depth (the converter pass)
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// ENHANCEMENT E6 (frame output): the surface's present mode and frame latency for the current
    /// level of [`crate::E6`]. Level 0 is `master`'s configuration exactly (`AutoVsync`, latency 2).
    /// Level 1 asks for `Mailbox` (replace the queued frame, no tearing, short queue) and, where the
    /// surface does not offer it, takes `AutoVsync`; level 2 asks for `Immediate` and falls back to
    /// `AutoNoVsync`. Reconfigures the surface only when the pair changes; the device, the
    /// textures and the depth target are untouched.
    // ENHANCEMENT E6
    pub(super) fn apply_present_mode(&mut self) {
        use crate::Enhancements as E;
        let (mode, latency) = match self.enhancements.level(crate::E6) {
            E::E6_MAILBOX if self.present_modes.contains(&wgpu::PresentMode::Mailbox) => (wgpu::PresentMode::Mailbox, 1),
            E::E6_MAILBOX => (wgpu::PresentMode::AutoVsync, 2),
            E::E6_IMMEDIATE if self.present_modes.contains(&wgpu::PresentMode::Immediate) => (wgpu::PresentMode::Immediate, 2),
            E::E6_IMMEDIATE => (wgpu::PresentMode::AutoNoVsync, 2),
            _ => (wgpu::PresentMode::AutoVsync, 2),
        };
        if self.config.present_mode == mode && self.config.desired_maximum_frame_latency == latency {
            return;
        }
        self.config.present_mode = mode;
        self.config.desired_maximum_frame_latency = latency;
        self.configure_surface();
        tracing::info!(level = self.enhancements.level(crate::E6), ?mode, latency, supported = ?self.present_modes, "E6 frame output");
    }

    /// Configure the window's surface from `self.config`. ENHANCEMENT E33: with the EDR output on, the surface is an
    /// extended-range `Rgba16Float` one (`self.config` keeps the 8-bit format every pipeline of the byte path is built for).
    pub(super) fn configure_surface(&self) {
        #[cfg(target_os = "macos")]
        if self.edr.on {
            let mut c = self.config.clone();
            c.format = HDR_FORMAT;
            c.color_space = wgpu::SurfaceColorSpace::ExtendedSrgbLinear;
            c.usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
            self.surface.configure(&self.device, &c);
            return;
        }
        self.surface.configure(&self.device, &self.config);
    }

    /// ENHANCEMENT E33: E33 on (level 1) and the display can do it: switch the surface to the EDR configuration, or back.
    pub(super) fn apply_edr(&mut self) {
        if self.edr.apply(self.enhancements.level(crate::E33)) {
            self.configure_surface();
            self.edr.log_output();
        }
    }

    /// ENH-LITE: the adapter's class ([`crate::classify_gpu`]).
    pub fn gpu_class(&self) -> crate::GpuClass {
        self.gpu_class
    }

    /// ENHANCEMENT E33: the display does extended-range output (the row of the switch is live).
    pub fn edr_available(&self) -> bool {
        self.edr.available()
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.configure_surface();
        self.depth_view = Self::make_depth(&self.device, &self.config);
    }

    /// The surface's pixel size.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width.max(1), self.config.height.max(1))
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }
}

/// What [`Core::create_mesh_setup`] makes.
struct MeshSetup {
    shader: wgpu::ShaderModule,
    globals_layout: wgpu::BindGroupLayout,
    object_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    sky_layout: wgpu::BindGroupLayout,
    csm_state: csm::CsmState,
    spot_state: spot::SpotState,
    lamp_state: lamps::LampState,
    contact_state: contact::ContactState,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: Vec<wgpu::RenderPipeline>,
}

/// What [`Core::create_samplers`] makes.
struct SceneSamplers {
    samplers: [wgpu::Sampler; 8],
    point_samplers: [wgpu::Sampler; 4],
    level0_samplers: [wgpu::Sampler; 4],
    point_level0_samplers: [wgpu::Sampler; 4],
    compute_supported: bool,
    aniso_supported: bool,
    aniso_samplers: [wgpu::Sampler; 4],
    sprite_sampler: wgpu::Sampler,
}

/// What [`Core::create_sprite_setup`] makes.
struct SpriteSetup {
    sprite_layout: wgpu::BindGroupLayout,
    sprite_pipeline: wgpu::RenderPipeline,
    sprite_flat_pipeline: wgpu::RenderPipeline,
    sprite_depth_pipelines: [wgpu::RenderPipeline; 2],
    belt: wgpu::util::StagingBelt,
    sprite_buf: wgpu::Buffer,
}
