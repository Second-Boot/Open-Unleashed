//! The renderer's `NFS_*` diagnostics and test switches, read **once**: [`flags`] snapshots them on first use into one
//! [`Flags`] (`std::env::var` takes the process's environment lock and allocates; `NFS_EDR_PROBE` is asked every frame of an
//! occluded window). They are not meant to be toggled while the game runs: the only writer of the environment is `main`,
//! before the renderer exists. Every `NFS_*` the renderer reads is here (the test-only `WGSL_DUMP` of `wgsl.rs` aside).

use std::sync::OnceLock;

/// TEST GLUE `NFS_NEAR_DEBUG=<level>` (ENHANCEMENT E49), the level decoded: `1` shows the reflection alone on the paint, `2` also without
/// the proxy box (the plain lookup), `3` / `4` the same as a mirror (mip 0, with / without the box), `5` (NEAR-REFL2) the sphere-map
/// overlay's geometry weights alone; a higher level reads like `3` (odd) or `4` (even).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NearDebug {
    #[default]
    Off,
    Refl,
    ReflPlain,
    Sharp,
    SharpPlain,
    Overlay,
}

impl NearDebug {
    fn from_level(level: u8) -> Self {
        match level {
            0 => Self::Off,
            1 => Self::Refl,
            2 => Self::ReflPlain,
            5 => Self::Overlay,
            n if n.is_multiple_of(2) => Self::SharpPlain,
            _ => Self::Sharp,
        }
    }

    /// The lookup without the proxy box (the even levels).
    pub(crate) fn plain(self) -> bool {
        matches!(self, Self::ReflPlain | Self::SharpPlain)
    }
}

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
    /// ENHANCEMENT ENH-TEMPORAL diagnostic `NFS_TEMPORAL_DEBUG=<view>` (`temporal::Config::from_env`)
    pub temporal_debug: Option<String>,
    /// TEST GLUE `NFS_NEAR_DEBUG=<level>` (ENHANCEMENT E49): the paint shows the reflection alone, 0 = off
    pub near_debug: NearDebug,
    /// ENHANCEMENT E33 diagnostic `NFS_EDR_HEADROOM=<x>`: the display's headroom pretended
    pub edr_headroom: Option<f32>,
    /// ENHANCEMENT E21 TEST GLUE `NFS_EYE_DT=<s>`: the eye adaptation's step fixed (stills independent of the frame rate)
    pub eye_dt: Option<f32>,
}

impl Flags {
    fn from_env() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            edr_probe: std::env::var("NFS_EDR_PROBE").ok(),
            #[cfg(target_os = "macos")]
            edr_probe_set: std::env::var_os("NFS_EDR_PROBE").is_some(),
            gpu_profile: std::env::var("NFS_GPU_PROFILE").ok(),
            gpu_time: std::env::var("NFS_GPU_TIME").is_ok(),
            test_gpu_error: std::env::var_os("NFS_TEST_GPU_ERROR").is_some(),
            fsr_passthrough: std::env::var_os("NFS_FSR_PASSTHROUGH").is_some(),
            temporal_debug: std::env::var("NFS_TEMPORAL_DEBUG").ok(),
            near_debug: NearDebug::from_level(std::env::var("NFS_NEAR_DEBUG").ok().and_then(|v| v.parse().ok()).unwrap_or(0)),
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
