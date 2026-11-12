//! The device, the surface and the start-up: `Core::new` (adapter, limits, every layout / pipeline / dummy the renderer starts with),
//! the instance and the surface's size / present mode / EDR switch. Moved out of gpu.rs as it was.

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
            size: 128 * 128,
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
        let proj_default = gfx::texture_2d(&device, "projector default", 1, 1, wgpu::TextureFormat::Rgba8Unorm, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &proj_default, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            &[255, 255, 255, 0],
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
            wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let refl_dummy_view = refl_dummy.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let soft_dummy = gfx::texture_2d(&device, "soft dummy", 1, 1, crate::soft::SOFT_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        let soft_dummy_view = soft_dummy.create_view(&Default::default());
        // ENHANCEMENT E16: the probe's uniform while there is none (never read: `globals.refl.z` is 0 then)
        let probe_dummy = device.create_buffer(&wgpu::BufferDescriptor { label: Some("probe sh dummy"), size: 9 * 16, usage: wgpu::BufferUsages::UNIFORM, mapped_at_creation: false });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: headlight_map_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&proj_default_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&proj_samplers[0]) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&proj_default_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&refl_dummy_view) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(csm_state.view()) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::Sampler(&csm_state.sampler) },
                wgpu::BindGroupEntry { binding: 8, resource: csm_state.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::TextureView(&soft_dummy_view) },
                wgpu::BindGroupEntry { binding: 16, resource: probe_dummy.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(spot_state.view()) },
                wgpu::BindGroupEntry { binding: 11, resource: spot_state.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 20, resource: lamp_state.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 25, resource: wgpu::BindingResource::TextureView(lamp_state.view()) },
                wgpu::BindGroupEntry { binding: 24, resource: contact_state.uniform.as_entire_binding() },
            ],
        });
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
        let SpriteSetup { sprite_layout, sprite_pipeline, sprite_depth_pipelines, belt, sprite_buf } = Self::create_sprite_setup(&device, format);
        let mut r = Self {
            surface,
            device,
            queue,
            config,
            depth_view,
            pipelines: pipelines.clone(),
            globals_buf,
            globals_bg,
            globals_layout,
            proj_samplers,
            _proj_default: proj_default,
            proj_default_view,
            headlight_map_buf,
            proj_tex: None,
            head_tex: None,
            object_layout,
            material_layout,
            sky_layout,
            sky_bgs: Vec::new(),
            refl: None,
            refl_dummy_view,
            _refl_dummy: refl_dummy,
            probe: None,
            coat: None,
            refl_building: false,
            refl_near_pass: None,
            refl_pre_copy: None,
            near_mask: None,
            probe_dummy,
            compute_supported,
            soft: None,
            ssao: None,
            shafts: None,
            haze: None,
            lbeam: None,
            soft_dummy_view,
            _soft_dummy: soft_dummy,
            no_reflections: false,
            base_vertex: adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::BASE_VERTEX),
            buffer_serial: 0,
            samplers,
            point_samplers,
            level0_samplers,
            point_level0_samplers,
            aniso_samplers,
            aniso_clamp: 1,
            aniso_supported,
            gpu_class,
            trilinear: false,
            fakesort_mode: FakeSortMode::default(),
            gamma: 1.0,
            gamma_pass: None,
            enhancements: crate::Enhancements::ORIGINAL,
            enh_forced_off: false,
            gpu_errors_seen: 0,
            present_modes,
            ms_supported,
            hdr_ok,
            edr,
            #[cfg(target_os = "macos")]
            edr_frames: 0,
            ms_supported_hdr,
            cur_hdr: None,
            scale_blit_mirror: None,
            temporal: crate::temporal::Config::from_env().map(crate::temporal::Temporal::new),
            temporal_flag: false,
            temporal_gpu: None,
            cur_temporal: None,
            motion: Default::default(),
            temporal_inputs: None,
            pipeline_sets: std::collections::HashMap::from([(1, pipelines.clone())]),
            mesh_shader: shader,
            mesh_shaders: std::collections::HashMap::new(),
            pipeline_vars: [Vec::new(), Vec::new(), Vec::new()],
            pipeline_cur: [1; 4],
            set_builder: Default::default(),
            exact_sets: false,
            var_set: [0; 4],
            item_var: Vec::new(),
            mesh_layout: pipeline_layout,
            ms_cache: std::collections::HashMap::new(),
            scale_blit: None,
            fsr: None,
            fsr_state: 0,
            fsr_kind: None,
            #[cfg(target_os = "macos")]
            mfxt_state: 0,
            fsr_active: false,
            fsr_out: None,
            frame_no: 0,
            fsr_last_frame: 0,
            upscaler: None,
            upscale_state: 0,
            upscale_active: false,
            upscale_images: None,
            upscale_present: None,
            sharpen: None,
            cur_lod_shift: 0.0,
            smaa: None,
            post: Default::default(),
            sprite_sampler,
            object_buf,
            object_bg,
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
            sprite_pipeline,
            sprite_depth_pipelines,
            offscreen: std::collections::HashMap::new(),
            sprite_layout,
            sprite_buf,
            belt,
            pending_writes: Vec::new(),
            pending_bytes: Vec::new(),
            pending_textures: Vec::new(),
            pending_texture_bytes: Vec::new(),
            depth_cache: std::collections::HashMap::new(),
            buckets: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            frame_scratch: FrameScratch::default(),
            lens_enabled: false,
            lens_pending: Vec::new(),
            lens_done: Vec::new(),
            lens_staged: Vec::new(),
            gpu_time_log: std::env::var("NFS_GPU_TIME").is_ok(),
            gpu_time_acc: (0.0, 0),
            bench_target: None,
            csm: csm_state,
            spot: spot_state,
            lamp: lamp_state,
            contact: contact_state,
            frames_in_flight: std::collections::VecDeque::with_capacity(MAX_FRAMES_IN_FLIGHT + 1),
            last_submit: None,
        };
        let _ = &r.object_layout;
        r.white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        Ok(r)
    }

    /// The instance, the surface and the adapter, the device and its queue, the surface's configuration and what the adapter
    /// can do (E1's sample counts, E7's float target, E33's extended range). Moved out of `new` as it was.
    fn create_device(window: Arc<Window>) -> Result<DeviceSetup> {
        let size = window.inner_size();
        // PLATFORM GLUE: the graphics API. Vulkan / Metal / Direct3D 12 first; OpenGL (EGL / WGL) only when none of
        // them gives an adapter for this window (a machine without a Vulkan driver). `WGPU_BACKEND` (wgpu's own
        // variable, e.g. `gl` or `vulkan`) picks the API instead, for testing the fallback.
        // PLATFORM GLUE (Windows): the Direct3D 12 backend compiles with DXC from beside the exe, else FXC; an Intel integrated
        // GPU takes its Direct3D 12 adapter instead of Vulkan (see `win_dxc_path`, `prefer_dx12_over`).
        let backend_forced = wgpu::Backends::from_env().is_some();
        let open = |backends: wgpu::Backends, dxc: Option<&str>| -> Result<(wgpu::Instance, wgpu::Surface<'static>, wgpu::Adapter)> {
            let mut instance = new_instance(&window, backends, dxc);
            let mut dxc = dxc;
            if dxc.is_some() && !backend_forced && backends.contains(wgpu::Backends::DX12) && pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12)).is_empty() {
                // DXC was asked for but the Direct3D 12 backend did not come up (the instance drops a backend whose shader compiler failed to load): FXC
                tracing::warn!("DXC: dxcompiler.dll / dxil.dll next to the exe did not load (no Direct3D 12 adapter with it) - falling back to FXC");
                dxc = None;
                instance = new_instance(&window, backends, None);
            }
            if let Some(p) = dxc {
                tracing::info!(path = %p, "Direct3D 12 shader compiler: DXC (dxcompiler.dll + dxil.dll next to the exe; FXC if it fails to load)");
            } else if cfg!(windows) {
                tracing::info!("Direct3D 12 shader compiler: FXC (no dxcompiler.dll + dxil.dll next to the exe, or DXC did not load)");
            }
            // with no driver of these APIs installed at all, already the surface fails
            let surface = instance.create_surface(window.clone()).context("create surface")?;
            let mut adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            }))
            .context("no suitable GPU adapter")?;
            // the UHD 600's Vulkan driver died inside `request_device`: an Intel integrated GPU on Windows uses its Direct3D 12 adapter
            // (the same device); discrete GPUs keep wgpu's choice, and so does everything when `WGPU_BACKEND` is set
            if cfg!(windows) && !backend_forced {
                let first = adapter.get_info();
                if prefer_dx12_over(first.vendor, first.device_type, first.backend) {
                    let alt = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12)).into_iter().find(|a| {
                        let i = a.get_info();
                        i.vendor == first.vendor && i.device == first.device && a.is_surface_supported(&surface)
                    });
                    match alt {
                        Some(a) => {
                            tracing::info!(name = %first.name, from = ?first.backend, "Intel integrated GPU on Windows: using its Direct3D 12 adapter instead (WGPU_BACKEND overrides)");
                            adapter = a;
                        }
                        None => tracing::warn!(name = %first.name, backend = ?first.backend, "Intel integrated GPU on Windows: no Direct3D 12 adapter for it, keeping the chosen one"),
                    }
                }
            }
            Ok((instance, surface, adapter))
        };
        let dxc_path = win_dxc_path();
        let (instance, surface, adapter) = match wgpu::Backends::from_env() {
            Some(chosen) => open(chosen, dxc_path.as_deref())?,
            None => match open(wgpu::Backends::METAL | wgpu::Backends::PRIMARY, dxc_path.as_deref()) {
                Ok(found) => found,
                Err(e) => {
                    tracing::warn!(error = format!("{e:#}"), "no Vulkan / Metal / Direct3D 12 for the window: trying OpenGL");
                    open(wgpu::Backends::GL, dxc_path.as_deref())?
                }
            },
        };
        // diagnostics (port-own, `open-unleashed.log`): every adapter the instance sees, the chosen one with
        // its features / limits, so a failure on another machine can be read from the log
        for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
            let i = a.get_info();
            tracing::info!(name = %i.name, backend = ?i.backend, device_type = ?i.device_type, driver = %i.driver, driver_info = %i.driver_info, vendor = format_args!("{:#06x}", i.vendor), device = format_args!("{:#06x}", i.device), "gpu adapter available");
        }
        tracing::info!(info = ?adapter.get_info(), features = ?adapter.features(), "chosen adapter");
        tracing::info!(limits = ?adapter.limits(), downlevel = ?adapter.get_downlevel_capabilities(), "chosen adapter limits");
        let adapter_info = adapter.get_info();
        let gpu_class = crate::classify_gpu(adapter_info.device_type, adapter_info.backend, &adapter_info.name);
        tracing::info!(adapter = ?adapter_info.name, backend = ?adapter_info.backend, device_type = ?adapter_info.device_type, gpu_class = ?gpu_class, "gpu adapter");
        let _ = crate::ADAPTER_LABEL.set(format!("{} · {:?}", adapter_info.name, adapter_info.backend));
        // TEST GLUE `NFS_GPU_PROFILE=minimal` (WIN-STARTUP): a device with no optional feature, the default limits and no multisample
        // count above 1 nor a float scene target — what a weak or old adapter reports — to see the fall-backs act on any machine
        let minimal_profile = crate::prof::minimal();
        if minimal_profile {
            tracing::warn!("NFS_GPU_PROFILE=minimal: no optional device features, default limits, no multisampling, no float scene target");
        }
        // The device request is logged first: a driver that crashes inside it (the UHD 600's Vulkan one did) leaves nothing else to
        // read, and the next start then runs in safe mode (`guard.rs` in the application). PASSTHROUGH_SHADERS (E37 FSR's SPIR-V passthrough)
        // is no longer requested by default (opt in with `NFS_FSR_PASSTHROUGH=1`): FSR runs through naga, bit-identical (ENH-FSR3)
        tracing::info!(minimal = minimal_profile, "requesting the device (if the log ends here, the driver crashed creating it: the next start runs in safe mode)");
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("nfs-renderer"),
            // ENHANCEMENT E1 asks the adapter which sample counts a format has (2× and 8× need this
            // feature to be reported); it changes nothing a draw does. ENHANCEMENT E41: `TEXTURE_COMPRESSION_BC` (Metal on Apple Silicon and
            // Intel Macs, Vulkan / Direct3D 12 on desktop GPUs report it) lets the HD pack's BC7 textures upload as they are; without it the
            // game decodes them on the CPU
            required_features: if minimal_profile { wgpu::Features::empty() } else { (adapter.features() & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM | wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::CLEAR_TEXTURE)) | if adapter.get_info().backend == wgpu::Backend::Vulkan && !crate::upscale::fsr_naga_forced() && std::env::var_os("NFS_FSR_PASSTHROUGH").is_some() { adapter.features() & wgpu::Features::PASSTHROUGH_SHADERS } else { wgpu::Features::empty() } | if crate::prof::request_timestamps() { adapter.features() & wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() } },
            // ENHANCEMENT E37: FSR 3.1 also takes `TEXTURE_FORMAT_16BIT_NORM` (its Lanczos LUT is `R16Snorm`) and `FLOAT32_FILTERABLE`
            // (its `R32Float` / `Rg32Float` images are bound filterable) and `CLEAR_TEXTURE` (the history is cleared on a reset), asked for when the adapter has them: they change nothing a draw does.
            // Its SPD passes bind 8 storage images in one stage (wgpu's default is 4) and its
            // luma pyramid needs more group memory than the default 16 KiB; asked up to what the adapter has (nothing the
            // game draws uses them)
            required_limits: {
                let have = adapter.limits();
                let mut l = wgpu::Limits::default();
                if !minimal_profile {
                    l.max_storage_textures_per_shader_stage = have.max_storage_textures_per_shader_stage.min(8);
                    l.max_compute_workgroup_storage_size = have.max_compute_workgroup_storage_size.min(32768);
                }
                l
            },
            ..Default::default()
        }))
        .context("request device")?;
        // A GPU error never stops the game: wgpu's default for an error no scope caught is a panic, which closes a GUI release silently.
        device.on_uncaptured_error(Arc::new(gpu_error));
        device.set_device_lost_callback(|reason, msg| tracing::error!(?reason, "wgpu device lost: {msg}"));
        // TEST GLUE `NFS_TEST_GPU_ERROR=1`: one invalid resource, to see the handler log it (and, on the enhanced game, every enhancement fall back)
        if std::env::var_os("NFS_TEST_GPU_ERROR").is_some() {
            let _ = device.create_texture(&wgpu::TextureDescriptor { label: Some("test glue: invalid"), size: wgpu::Extent3d { width: 0, height: 0, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
        }
        // ENHANCEMENT E41: the HD pack's BC7 textures upload as they are only where the device takes BC (see `Core::supports_bc7`)
        tracing::info!(adapter_has_bc = adapter.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC), device_has_bc = device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC), "texture compression BC (ENHANCEMENT E41)");
        crate::prof::init(&device, &queue);

        let caps = surface.get_capabilities(&adapter);
        // a byte surface, as Direct3D 7's was: colours, blends and interpolation work on the stored
        // bytes, not in linear light (`RE/RENDERING.md`, "Colour space")
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or_else(|| caps.formats.iter().copied().find(|f| !f.is_srgb()))
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            // the byte pipeline's surface (`RENDERING.md` "Colour space"): no colour-space conversion
            color_space: surface_color_space(),
        };
        // ENHANCEMENT E33: does the surface / display do extended-range output (macOS only; elsewhere never)
        let edr = crate::edr::Edr::probe(&surface, &adapter, &caps);
        let present_modes = caps.present_modes.clone();
        tracing::info!(present_modes = ?present_modes, "surface present modes");
        surface.configure(&device, &config);
        let ms_supported = {
            let col = adapter.get_texture_format_features(format).flags;
            let dep = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
            let mut m = 0u32;
            for n in [2u32, 4, 8] {
                if col.sample_count_supported(n) && dep.sample_count_supported(n) {
                    m |= n;
                }
            }
            tracing::info!(supported = m, "multisample counts (bit set of 2 / 4 / 8)");
            if minimal_profile { 0 } else { m }
        };
        // ENHANCEMENT E7: the float colour format of the HDR scene target
        let (hdr_ok, ms_supported_hdr) = {
            let f = adapter.get_texture_format_features(HDR_FORMAT);
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC;
            let ok = f.allowed_usages.contains(usage) && f.flags.contains(wgpu::TextureFormatFeatureFlags::BLENDABLE) && f.flags.contains(wgpu::TextureFormatFeatureFlags::FILTERABLE);
            let dep = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
            let mut m = 0u32;
            for n in [2u32, 4, 8] {
                if f.flags.sample_count_supported(n) && f.flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE) && dep.sample_count_supported(n) {
                    m |= n;
                }
            }
            tracing::info!(hdr = ok, supported = m, "E7 Rgba16Float target (multisample counts as a bit set)");
            (ok && !minimal_profile, if minimal_profile { 0 } else { m })
        };
        Ok(DeviceSetup { _instance: instance, surface, adapter, device, queue, config, edr, present_modes, ms_supported, hdr_ok, ms_supported_hdr, gpu_class })
    }

    /// The mesh shader, the four bind group layouts of the 3-D passes, the shadow / spot / lamp / contact states and the
    /// mesh pipelines of the 8-bit set. Moved out of `new` as it was.
    fn create_mesh_setup(device: &wgpu::Device, format: wgpu::TextureFormat) -> MeshSetup {
        let shader = gfx::wgsl_module(device, "mesh shader", &crate::classes::mesh_shader_source(0));
        let csm_entries = csm::layout_entries();
        let spot_entries = spot::layout_entries();
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                gfx::bgl_uniform(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                // the 128 × 128 `nght` bytes, four to a u32
                gfx::bgl_uniform(1, wgpu::ShaderStages::VERTEX),
                // the car shadow projector's texture (`[0x6115CC]+0x18C`) and its sampler
                gfx::bgl_tex(2, wgpu::TextureSampleType::Float { filterable: true }, false),
                gfx::bgl_sampler(3, wgpu::SamplerBindingType::Filtering),
                // `HEAD` of `Render/common.fsh`, the headlight decal `LayerUVLight` projects
                gfx::bgl_tex(4, wgpu::TextureSampleType::Float { filterable: true }, false),
                // ENHANCEMENT E8: the six environment faces of the player's car
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                // ENHANCEMENT E9: the sun shadow map (6), its comparison sampler (7) and the cascades (8)
                csm_entries[0],
                csm_entries[1],
                csm_entries[2],
                // ENHANCEMENT E12: the opaque part's raw depth (R32Float, read with textureLoad)
                gfx::bgl_tex(9, wgpu::TextureSampleType::Float { filterable: false }, false),
                // ENHANCEMENT E16: the car ambient probe's SH coefficients (9 x vec4)
                gfx::bgl_uniform(16, wgpu::ShaderStages::FRAGMENT),
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
                gfx::bgl_tex(0, wgpu::TextureSampleType::Float { filterable: true }, false),
                gfx::bgl_sampler(1, wgpu::SamplerBindingType::Filtering),
                gfx::bgl_tex(2, wgpu::TextureSampleType::Float { filterable: true }, false),
                // the specular texture's sampler: `WRAP` both ways, set on each map by the binder
                // 0x440940 (0x537A70(1) → `tex+0x38` block `+0x14` / `+0x18`, Thrash 0x41 / 0x42)
                gfx::bgl_sampler(3, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let sky_layout = gfx::bgl(device, "sky map", &[gfx::bgl_tex(0, wgpu::TextureSampleType::Float { filterable: true }, false)]);
        let csm_state = csm::CsmState::new(device, &object_layout, &material_layout);
        let spot_state = spot::SpotState::new(device, &csm_state.light_layout);
        let lamp_state = lamps::LampState::new(device, &csm_state.light_layout);
        let contact_state = contact::ContactState::new(device);
        let pipeline_layout = gfx::pipeline_layout(device, "mesh pipeline layout", &[&globals_layout, &object_layout, &material_layout, &sky_layout]);
        let pipelines = build_mesh_pipelines(device, &shader, &pipeline_layout, format, 1, false);
        MeshSetup { shader, globals_layout, object_layout, material_layout, sky_layout, csm_state, spot_state, lamp_state, contact_state, pipeline_layout, pipelines }
    }

    /// The scene's samplers (by address mode, filter and E2's level) and the sprite pass's. Moved out of `new` as it was.
    fn create_samplers(device: &wgpu::Device, adapter: &wgpu::Adapter) -> SceneSamplers {
        // A texture's own state block (0x5379D0, `RE/notes/thrash.md` §7) asks for linear
        // magnification and minification (Thrash 7 = 1 → `MAG`/`MINFILTER` `LINEAR`), and a mip
        // filter that is `POINT` for a texture with mips (0x44D571…0x44D587) and becomes `LINEAR`
        // at `SPECIALFX` 3, the default (0x535A60). No Thrash state reaches `MAXANISOTROPY` or an
        // anisotropic filter, so there is none here (until 2026-09-24 this sampler clamped
        // anisotropy at 8, the port's own). The promotion to `LINEAR` is `[0x69DD1D]`, which
        // 0x44ED10 clears and sets again only at `SPECIALFX` 3 (0x44EDD1, 0x44EF04) and the front
        // end's 0x44F020 clears (0x44F080) — `set_trilinear`; below it the mip filter is `POINT`.
        let make_sampler = |clamp_u: bool, clamp_v: bool, mag_min: wgpu::FilterMode, mip: wgpu::MipmapFilterMode| {
            let mode = |c: bool| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene"),
                address_mode_u: mode(clamp_u),
                address_mode_v: mode(clamp_v),
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: mag_min,
                min_filter: mag_min,
                mipmap_filter: mip,
                ..Default::default()
            })
        };
        // `nfs5.ini` `Filtering=0`: `POINT` magnification / minification (see the projector sampler)
        let mag_min_linear = if crate::point_filter() { wgpu::FilterMode::Nearest } else { wgpu::FilterMode::Linear };
        let samplers = std::array::from_fn(|i| {
            let mip = if i & 4 == 0 { wgpu::MipmapFilterMode::Linear } else { wgpu::MipmapFilterMode::Nearest };
            make_sampler(i & 1 != 0, i & 2 != 0, mag_min_linear, mip)
        });
        // `LayerFalseSort1`/`2`'s other arm sets Thrash 7 ← 0 for the one draw (`MAG`/`MINFILTER`
        // `POINT`) and never touches state 0x0B, so the mip filter here follows the same `POINT`
        // default as every other sampler below `SPECIALFX` 3 (`trilinear` is false whenever
        // `fakesort_mode` can pick `Single`/`PerItem` — both are set from the same level, `main.rs`)
        let point_samplers = std::array::from_fn(|i| make_sampler(i & 1 != 0, i & 2 != 0, wgpu::FilterMode::Nearest, wgpu::MipmapFilterMode::Nearest));
        let level0_samplers = std::array::from_fn(|i| make_scene_sampler(device, i & 1 != 0, i & 2 != 0, wgpu::MipmapFilterMode::Nearest, 1, Some(0.0)));
        let point_level0_samplers = std::array::from_fn(|i| {
            let mode = |c: bool| if c { wgpu::AddressMode::ClampToEdge } else { wgpu::AddressMode::Repeat };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene point E2 level 0"),
                address_mode_u: mode(i & 1 != 0),
                address_mode_v: mode(i & 2 != 0),
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                lod_max_clamp: 0.0,
                ..Default::default()
            })
        });
        let compute_supported = adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
        let aniso_supported = adapter.get_downlevel_capabilities().flags.contains(wgpu::DownlevelFlags::ANISOTROPIC_FILTERING);
        tracing::info!(aniso_supported, "anisotropic filtering (ENHANCEMENT E2)");
        let aniso_samplers = std::array::from_fn(|i| make_scene_sampler(device, i & 1 != 0, i & 2 != 0, wgpu::MipmapFilterMode::Linear, 1, None));

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

    /// The 2D sprite pass: its layout, pipelines, vertex buffer, and the frame's staging belt. Moved out of `new` as it was.
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
        let sprite_layout = gfx::bgl(device, "sprite", &[gfx::bgl_tex(0, wgpu::TextureSampleType::Float { filterable: true }, false), gfx::bgl_sampler(1, wgpu::SamplerBindingType::Filtering)]);
        let sprite_pl = gfx::pipeline_layout(device, "sprite", &[&sprite_layout]);
        let sprite_vertex = [Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<SpriteVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x4, 3 => Float32] })];
        let make_sprite_pipeline = |label: &str, fs: &str, depth: wgpu::DepthStencilState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&sprite_pl),
                vertex: wgpu::VertexState { module: &sprite_shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &sprite_vertex },
                fragment: Some(wgpu::FragmentState { module: &sprite_shader, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
                depth_stencil: Some(depth),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let sprite_pipeline = make_sprite_pipeline("sprite", "fs_main", wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() });
        // A front-end sprite under the kit's depth state (`crate::SpriteDepth`): Thrash 0x39 ← 1
        // (0x4D30BE) and the compare Thrash 0x28 sets — 3 `LESSEQUAL` (0x4D30C9), 6 `GREATEREQUAL`
        // inside a mask (0x4DAAC0 → 0x537830) — with the device's alpha test in the shader.
        let depth_state = |compare| wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(true), depth_compare: Some(compare), stencil: Default::default(), bias: Default::default() };
        let sprite_depth_pipelines = [
            make_sprite_pipeline("sprite lessequal", "fs_alpha_test", depth_state(wgpu::CompareFunction::LessEqual)),
            make_sprite_pipeline("sprite greaterequal", "fs_alpha_test", depth_state(wgpu::CompareFunction::GreaterEqual)),
        ];
        let belt = wgpu::util::StagingBelt::new(device.clone(), BELT_CHUNK);
        let sprite_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("sprites"), size: MAX_SPRITES * 6 * std::mem::size_of::<SpriteVertex>() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        SpriteSetup { sprite_layout, sprite_pipeline, sprite_depth_pipelines, belt, sprite_buf }
    }

    pub(super) fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
        Self::make_depth_sized(device, config.width, config.height)
    }

    pub(super) fn make_depth_sized(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
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
        let (mode, latency) = match self.enhancements.level(crate::E6) {
            1 if self.present_modes.contains(&wgpu::PresentMode::Mailbox) => (wgpu::PresentMode::Mailbox, 1),
            1 => (wgpu::PresentMode::AutoVsync, 2),
            2 if self.present_modes.contains(&wgpu::PresentMode::Immediate) => (wgpu::PresentMode::Immediate, 2),
            2 => (wgpu::PresentMode::AutoNoVsync, 2),
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
            c.format = wgpu::TextureFormat::Rgba16Float;
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

/// What [`Core::create_device`] makes.
struct DeviceSetup {
    /// kept until `Core::new` returns, as when it was one of `new`'s locals
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    edr: crate::edr::Edr,
    present_modes: Vec<wgpu::PresentMode>,
    ms_supported: u32,
    hdr_ok: bool,
    ms_supported_hdr: u32,
    gpu_class: crate::GpuClass,
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
    sprite_depth_pipelines: [wgpu::RenderPipeline; 2],
    belt: wgpu::util::StagingBelt,
    sprite_buf: wgpu::Buffer,
}
