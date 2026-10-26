//! Start-up crash guard (PLATFORM GLUE: host-side robustness, not a rule of the original, which is DirectX 7).
//!
//! Some failures leave no panic to catch: a graphics driver that faults while it creates the device or compiles a
//! pipeline takes the process down, and on a Windows GUI release the window just disappears. The guard makes the
//! next start ask for a plain device instead:
//!
//! * [`begin`] writes `startup.guard` into the per-OS data directory (beside `open-unleashed.log`, never the game
//!   folder). A start that finds the file already there knows the previous one never reached [`CONFIRM_SECS`] of life
//!   nor a clean exit; it writes `gpu-safe.flag` and runs in safe mode: `NFS_GPU_PROFILE=minimal` (a device with no
//!   optional feature) and, on Windows, `WGPU_BACKEND=dx12`, unless the player set either variable.
//! * safe mode lasts ONE start: a safe start that survives (lives [`CONFIRM_SECS`], or ends cleanly) drops the flag,
//!   so the next start tries the normal path again; if that start dies too, the guard puts the following one in safe
//!   mode again.
//! * a clean end of the run ([`end`]: the window closed, or an error was reported) removes the guard; a panic or a
//!   kill does not.
//!
//! Only a release build runs it (a debug / scripted run - `NFS5_SAVE_DIR`, `NFS_NO_FOCUS` - never touches the
//! files); `NFS_GUARD=1` forces it for a test, `NFS_GUARD=0` turns it off. With `NFS5_SAVE_DIR` set the files sit
//! beside it, as the log does.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// A start that lives this long is a start that worked.
const CONFIRM_SECS: u64 = 30;

static SAFE: AtomicBool = AtomicBool::new(false);

fn enabled() -> bool {
    match std::env::var("NFS_GUARD").ok().as_deref() {
        Some("0") => false,
        Some(_) => true,
        None => !cfg!(debug_assertions) && std::env::var_os("NFS5_SAVE_DIR").is_none() && std::env::var_os("NFS_NO_FOCUS").is_none(),
    }
}

/// The data directory (beside `NFS5_SAVE_DIR` when that is set, as the log is: a test never touches the real one).
fn dir() -> PathBuf {
    std::env::var_os("NFS5_SAVE_DIR")
        .and_then(|d| PathBuf::from(d).parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(nfs_assets::platform::data_dir)
}

/// Called once at start-up, before any GPU work and before other threads exist. Sets up the plain-device environment
/// when the run is in safe mode.
pub fn begin() {
    if !enabled() {
        return;
    }
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    let guard = d.join("startup.guard");
    let flag = d.join("gpu-safe.flag");
    if guard.exists() {
        tracing::warn!("the previous start did not finish (a crash?): safe mode for this start");
        let _ = std::fs::write(&flag, b"the previous start did not finish; this start asks for a plain GPU device\n");
    }
    let safe = flag.exists();
    SAFE.store(safe, Ordering::Relaxed);
    if safe {
        // SAFETY: start-up, before any GPU / audio thread reads these variables
        unsafe {
            if std::env::var_os("NFS_GPU_PROFILE").is_none() {
                std::env::set_var("NFS_GPU_PROFILE", "minimal");
            }
            if cfg!(windows) && std::env::var_os("WGPU_BACKEND").is_none() {
                std::env::set_var("WGPU_BACKEND", "dx12");
            }
        }
        tracing::warn!(flag = %flag.display(), "safe mode: a plain GPU device (no optional features){} for this start; the flag is dropped once it survives", if cfg!(windows) { ", Direct3D 12" } else { "" });
    }
    let _ = std::fs::write(&guard, b"start in progress\n");
    std::thread::Builder::new()
        .name("start-guard".into())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(CONFIRM_SECS));
            let _ = std::fs::remove_file(&guard);
            if SAFE.swap(false, Ordering::Relaxed) {
                let _ = std::fs::remove_file(&flag);
                tracing::info!("safe start survived: the next start tries the normal path again");
            }
        })
        .ok();
}

/// The run ended cleanly (or with a reported error): this start counts as survived.
pub fn end() {
    if enabled() {
        let d = dir();
        let _ = std::fs::remove_file(d.join("startup.guard"));
        if SAFE.swap(false, Ordering::Relaxed) {
            let _ = std::fs::remove_file(d.join("gpu-safe.flag"));
        }
    }
}
