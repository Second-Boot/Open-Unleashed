//! The renderer's `NFS_*` diagnostic read in a per-frame path, read **once**: `NFS_EDR_PROBE` (the occluded-window check asks
//! every frame; `std::env::var` takes the process's environment lock and allocates). [`flags`] snapshots it on first use into
//! one [`Flags`]. It is not meant to be toggled while the game runs: the only writer of the environment is `main`, before the
//! renderer exists. The start-up reads (device creation, `NFS_TEMPORAL_*`, FSR's context flags, `NFS_EDR_HEADROOM`) stay where
//! they are.

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
}

impl Flags {
    fn from_env() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            edr_probe: std::env::var("NFS_EDR_PROBE").ok(),
            #[cfg(target_os = "macos")]
            edr_probe_set: std::env::var_os("NFS_EDR_PROBE").is_some(),
        }
    }
}

static FLAGS: OnceLock<Flags> = OnceLock::new();

/// The process-wide snapshot (taken on the first call).
pub(crate) fn flags() -> &'static Flags {
    FLAGS.get_or_init(Flags::from_env)
}
