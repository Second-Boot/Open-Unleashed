//! The device and the platform glue: the instance (DXC), the adapter choice, the device with its limits and
//! the surface's configuration (`Core::create_device`), and the wgpu error handler.

use super::*;

/// How many uncaptured wgpu errors the handler has seen; ENHANCEMENT: [`Core::apply`] sees the count rise and forces every
/// enhancement off for the rest of the run (the original path), so an enhancement's pipeline or texture the device refuses never
/// stops the game.
pub(super) static GPU_ERRORS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// PLATFORM GLUE (robustness, no visual change): wgpu's default for an error no scope caught is a panic ("Handling wgpu errors as
/// fatal by default"), which on a GUI Windows release closes the game silently. The handler installed in [`Core::new`] logs the
/// error (the first 40, then every 1000th) and the game goes on.
fn gpu_error(e: wgpu::Error) {
    let n = GPU_ERRORS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 40 || n.is_multiple_of(1000) {
        // (its Display carries the cause: "Validation Error / Caused by: In Device::create_texture ...")
        tracing::error!(count = n, "wgpu error (uncaptured): {e}");
    }
}

/// PLATFORM GLUE (Windows): `dxcompiler.dll` + `dxil.dll` next to the exe (the Windows archive ships both; DXC needs dxil.dll to sign
/// its shaders): the path to hand `Dx12Compiler::DynamicDxc`, or `None` (FXC) when either is missing or off Windows.
fn win_dxc_path() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let (dxc, dxil) = (dir.join("dxcompiler.dll"), dir.join("dxil.dll"));
    (dxc.is_file() && dxil.is_file()).then(|| dxc.to_string_lossy().into_owned())
}

/// PLATFORM GLUE: the instance for `backends`; `dxc` = the DXC library path for the Direct3D 12 backend, `None` = FXC.
/// `WGPU_DX12_COMPILER` still wins over it.
fn new_instance(window: &Arc<Window>, backends: wgpu::Backends, dxc: Option<&str>) -> wgpu::Instance {
    let dx12 = wgpu::Dx12BackendOptions {
        shader_compiler: match dxc {
            Some(p) => wgpu::Dx12Compiler::DynamicDxc { dxc_path: p.to_owned() },
            None => wgpu::Dx12Compiler::Fxc,
        },
        ..Default::default()
    };
    wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        backend_options: wgpu::BackendOptions { dx12, ..Default::default() }.with_env(),
        // EGL needs the display before any surface exists
        ..wgpu::InstanceDescriptor::new_with_display_handle(Box::new(window.clone()))
    })
}

/// PLATFORM GLUE (Windows): an Intel (vendor 0x8086) integrated GPU on a non-Direct3D backend is moved to its Direct3D 12 adapter.
pub(crate) fn prefer_dx12_over(vendor: u32, device_type: wgpu::DeviceType, backend: wgpu::Backend) -> bool {
    vendor == 0x8086 && device_type == wgpu::DeviceType::IntegratedGpu && backend != wgpu::Backend::Dx12
}

impl Core {
    /// The instance, the surface and the adapter, the device and its queue, the surface's configuration and what the adapter
    /// can do (E1's sample counts, E7's float target, E33's extended range).
    pub(super) fn create_device(window: Arc<Window>) -> Result<DeviceSetup> {
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
                Ok(found) if found.2.get_info().device_type == wgpu::DeviceType::Cpu => {
                    // PLATFORM GLUE: the only Vulkan adapter that can present to this window is a software one (Mesa's lavapipe /
                    // llvmpipe: the hardware driver missing or unable to present, e.g. no DRI3), which draws every pixel on the CPU.
                    // OpenGL may still reach the GPU: take it when it gives a hardware adapter, else keep the software one
                    let soft = found.2.get_info().name;
                    drop(found);
                    match open(wgpu::Backends::GL, dxc_path.as_deref()) {
                        Ok(gl) if gl.2.get_info().device_type != wgpu::DeviceType::Cpu => {
                            tracing::warn!(software = %soft, gl = %gl.2.get_info().name, "the Vulkan adapter for the window is a software rasteriser: using the hardware OpenGL adapter instead (WGPU_BACKEND overrides)");
                            gl
                        }
                        _ => {
                            tracing::info!(software = %soft, "the Vulkan adapter for the window is a software rasteriser and OpenGL has no hardware one either: keeping it");
                            open(wgpu::Backends::METAL | wgpu::Backends::PRIMARY, dxc_path.as_deref())?
                        }
                    }
                }
                Ok(found) => found,
                Err(e) => {
                    tracing::warn!(error = format!("{e:#}"), "no Vulkan / Metal / Direct3D 12 for the window: trying OpenGL");
                    open(wgpu::Backends::GL, dxc_path.as_deref())?
                }
            },
        };
        // diagnostics (port-own, `open-unleashed.log`): every adapter the instance sees, the chosen one with
        // its features / limits, so a failure on another machine can be read from the log
        let mut hardware: Vec<String> = Vec::new();
        for a in pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())) {
            let i = a.get_info();
            if i.device_type != wgpu::DeviceType::Cpu {
                hardware.push(format!("{} ({:?})", i.name, i.backend));
            }
            tracing::info!(name = %i.name, backend = ?i.backend, device_type = ?i.device_type, driver = %i.driver, driver_info = %i.driver_info, vendor = format_args!("{:#06x}", i.vendor), device = format_args!("{:#06x}", i.device), "gpu adapter available");
        }
        tracing::info!(info = ?adapter.get_info(), features = ?adapter.features(), "chosen adapter");
        tracing::info!(limits = ?adapter.limits(), downlevel = ?adapter.get_downlevel_capabilities(), "chosen adapter limits");
        let adapter_info = adapter.get_info();
        let gpu_class = crate::classify_gpu(adapter_info.device_type, adapter_info.backend, &adapter_info.name);
        tracing::info!(adapter = ?adapter_info.name, backend = ?adapter_info.backend, device_type = ?adapter_info.device_type, gpu_class = ?gpu_class, "gpu adapter");
        // PLATFORM GLUE: a software rasteriser (Mesa's llvmpipe, WARP) draws every pixel on the CPU: tens of milliseconds a frame at
        // 1920x1080 where the machine's own GPU takes a few. It is what the window gets when no hardware adapter can present to it
        // (an X server without DRI3 - Xvfb, some remote sessions -, a missing Vulkan driver), so the log says it in one line, with
        // the hardware adapters that were passed over
        if adapter_info.device_type == wgpu::DeviceType::Cpu {
            tracing::warn!(adapter = %adapter_info.name, passed_over = ?hardware, "the chosen adapter is a software rasteriser (the CPU draws every frame): expect a low frame rate; no hardware adapter can present to this window");
        }
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
            required_features: if minimal_profile { wgpu::Features::empty() } else { (adapter.features() & (wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM | wgpu::Features::FLOAT32_FILTERABLE | wgpu::Features::CLEAR_TEXTURE)) | if adapter.get_info().backend == wgpu::Backend::Vulkan && crate::dbg::flags().fsr_passthrough { adapter.features() & wgpu::Features::PASSTHROUGH_SHADERS } else { wgpu::Features::empty() } | if crate::prof::request_timestamps() { adapter.features() & wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() } },
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
        if crate::dbg::flags().test_gpu_error {
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
            // the byte pipeline's surface (`RENDERING.md` "Colour space"): no colour-space conversion (ENHANCEMENT E33's extended
            // range is a surface of its own, `edr.rs`)
            color_space: wgpu::SurfaceColorSpace::default(),
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
}

/// What [`Core::create_device`] makes.
pub(super) struct DeviceSetup {
    /// kept until `Core::new` returns, as when it was one of `new`'s locals
    pub(super) _instance: wgpu::Instance,
    pub(super) surface: wgpu::Surface<'static>,
    pub(super) adapter: wgpu::Adapter,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    pub(super) config: wgpu::SurfaceConfiguration,
    pub(super) edr: crate::edr::Edr,
    pub(super) present_modes: Vec<wgpu::PresentMode>,
    pub(super) ms_supported: u32,
    pub(super) hdr_ok: bool,
    pub(super) ms_supported_hdr: u32,
    pub(super) gpu_class: crate::GpuClass,
}

#[cfg(test)]
mod adapter_choice_tests {
    use super::prefer_dx12_over;

    /// Only an Intel integrated GPU on a non-Direct3D backend is moved to Direct3D 12.
    #[test]
    fn dx12_preferred_for_intel_integrated_only() {
        use wgpu::{Backend as B, DeviceType as D};
        assert!(prefer_dx12_over(0x8086, D::IntegratedGpu, B::Vulkan));
        assert!(prefer_dx12_over(0x8086, D::IntegratedGpu, B::Gl));
        assert!(!prefer_dx12_over(0x8086, D::IntegratedGpu, B::Dx12));
        assert!(!prefer_dx12_over(0x8086, D::DiscreteGpu, B::Vulkan)); // Arc
        assert!(!prefer_dx12_over(0x10de, D::DiscreteGpu, B::Vulkan)); // NVIDIA
        assert!(!prefer_dx12_over(0x1002, D::IntegratedGpu, B::Vulkan)); // AMD APU
    }
}
