//! The renderer's `NFS_*` diagnostics and test switches, read **once**: [`flags`] snapshots them on first use into one
//! [`Flags`] (`std::env::var` takes the process's environment lock and allocates; `NFS_EDR_PROBE` is asked every frame of an
//! occluded window). They are not meant to be toggled while the game runs: the only writer of the environment is `main`,
//! before the renderer exists. Every `NFS_*` the renderer reads is here (the test-only `WGSL_DUMP` of `wgsl.rs` aside).

use std::sync::OnceLock;

/// The snapshot.
#[derive(Debug, Clone, Default)]
pub(crate) struct Flags {
    /// ENHANCEMENT E33 diagnostic `NFS_EDR_PROBE=<path prefix>` (`var(..).ok()`; the occluded-window check asks
    /// `var_os(..).is_some()`, which [`Flags::edr_probe_set`] keeps)
    #[cfg(target_os = "macos")]
    pub edr_probe: Option<String>,
    #[cfg(target_os = "macos")]
    pub edr_probe_set: bool,
    /// TEST GLUE `NFS_GPU_PROFILE` (`prof.rs`): `minimal` = the plain device, anything else but `0` = the per-pass timer
    pub gpu_profile: Option<String>,
    /// diagnostic `NFS_GPU_TIME`: the frame's encode + GPU time logged
    pub gpu_time: bool,
    /// TEST GLUE `NFS_TEST_GPU_ERROR`: one invalid texture at start-up (the uncaptured-error path)
    pub test_gpu_error: bool,
    /// ENHANCEMENT E37 `NFS_FSR_PASSTHROUGH` (Vulkan): FSR's own SPIR-V through wgpu's passthrough shaders
    pub fsr_passthrough: bool,
    /// ENHANCEMENT ENH-TEMPORAL diagnostic `NFS_TEMPORAL_NOID` (not `0`): the temporal pass made without the object ids
    pub temporal_noid: bool,
    /// ENHANCEMENT ENH-TEMPORAL diagnostic `NFS_TEMPORAL_DEBUG=<view>` (`temporal::Config::from_env`) and its controls
    /// `NFS_TEMPORAL_NOOBJ` / `NFS_TEMPORAL_NOMV` (not `0`)
    pub temporal_debug: Option<String>,
    pub temporal_noobj: bool,
    pub temporal_nomv: bool,
    /// ENHANCEMENT E37 `NFS_FSR_NAGA` (set, not `0`): FSR's SPIR-V on naga's front end on Vulkan too
    pub fsr_naga: bool,
    /// TEST GLUE `NFS_UPSCALE_MIP_EXTRA=<f>`: the temporal upscalers' extra LOD bias replaced (UPSCALE-SOFT A/B stills)
    pub upscale_mip_extra: Option<f32>,
    /// TEST GLUE `NFS_UPSCALE_MIP=0`: no upscaler LOD bias at all
    pub upscale_mip_off: bool,
    /// TEST GLUE `NFS_NEAR_DEBUG=<level>` (ENHANCEMENT E49): the paint shows the reflection alone, 0 = off
    pub near_debug: u8,
    /// ENHANCEMENT E33 diagnostic `NFS_EDR_HEADROOM=<x>`: the display's headroom pretended
    pub edr_headroom: Option<f32>,
    /// ENHANCEMENT E21 TEST GLUE `NFS_EYE_DT=<s>`: the eye adaptation's step fixed (stills independent of the frame rate)
    pub eye_dt: Option<f32>,
}

impl Flags {
    fn from_env() -> Self {
        // set and not `0`; a number, trimmed
        let on = |name| std::env::var(name).is_ok_and(|v| v != "0");
        let parsed = |name| std::env::var(name).ok().and_then(|v| v.trim().parse().ok());
        Self {
            #[cfg(target_os = "macos")]
            edr_probe: std::env::var("NFS_EDR_PROBE").ok(),
            #[cfg(target_os = "macos")]
            edr_probe_set: std::env::var_os("NFS_EDR_PROBE").is_some(),
            gpu_profile: std::env::var("NFS_GPU_PROFILE").ok(),
            gpu_time: std::env::var("NFS_GPU_TIME").is_ok(),
            test_gpu_error: std::env::var_os("NFS_TEST_GPU_ERROR").is_some(),
            fsr_passthrough: std::env::var_os("NFS_FSR_PASSTHROUGH").is_some(),
            temporal_noid: on("NFS_TEMPORAL_NOID"),
            temporal_debug: std::env::var("NFS_TEMPORAL_DEBUG").ok(),
            temporal_noobj: on("NFS_TEMPORAL_NOOBJ"),
            temporal_nomv: on("NFS_TEMPORAL_NOMV"),
            fsr_naga: std::env::var("NFS_FSR_NAGA").is_ok_and(|v| !v.is_empty() && v != "0"),
            upscale_mip_extra: parsed("NFS_UPSCALE_MIP_EXTRA"),
            upscale_mip_off: std::env::var("NFS_UPSCALE_MIP").is_ok_and(|v| v.trim() == "0"),
            near_debug: std::env::var("NFS_NEAR_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0),
            edr_headroom: std::env::var("NFS_EDR_HEADROOM").ok().and_then(|v| v.parse().ok()),
            eye_dt: std::env::var("NFS_EYE_DT").ok().and_then(|v| v.parse().ok()),
        }
    }
}

static FLAGS: OnceLock<Flags> = OnceLock::new();

/// The process-wide snapshot (taken on the first call).
pub(crate) fn flags() -> &'static Flags {
    FLAGS.get_or_init(Flags::from_env)
}
