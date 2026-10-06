//! Port-own diagnostics (not a rule of the original): a log file and a panic hook.
//!
//! A release build on Windows is a GUI-subsystem program (`windows_subsystem = "windows"`), so a
//! panic or an error return has no console to land on and the window just closes. Every release
//! run therefore writes `porsche.log` (tracing at INFO, still echoed to stderr) into the port's
//! data directory (`nfs_assets::platform::data_dir`, `%APPDATA%\PorscheUnleashed` on Windows),
//! keeping the previous run as `porsche.prev.log`; a panic writes its message and backtrace to the
//! log and, on a Windows release build, shows a native message box naming the log. Debug builds
//! (development, the golden runs) log to stderr only, unless `NFS_LOG_FILE=1`. PORT-DEVIATION: the
//! game's behaviour is untouched.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg_attr(not(all(windows, not(debug_assertions))), allow(dead_code))]
static LOG: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Where the log goes; `None` when no file is wanted.
fn log_path() -> Option<PathBuf> {
    let want = !cfg!(debug_assertions) || std::env::var_os("NFS_LOG_FILE").is_some_and(|v| v != "0");
    if !want || std::env::var_os("NFS_NO_LOG_FILE").is_some() {
        return None;
    }
    // `NFS5_SAVE_DIR` replaces `SaveData`; the log then sits beside it, never in the real data dir
    let dir = std::env::var_os("NFS5_SAVE_DIR")
        .and_then(|d| PathBuf::from(d).parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(nfs_assets::platform::data_dir);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("porsche.log");
    let _ = std::fs::rename(&path, dir.join("porsche.prev.log"));
    Some(path)
}

#[derive(Clone)]
struct Tee(Arc<Mutex<Option<std::fs::File>>>);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        if let Ok(mut g) = self.0.lock()
            && let Some(f) = g.as_mut()
        {
            let _ = f.write_all(buf);
            let _ = f.flush();
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Tee {
    type Writer = Tee;
    fn make_writer(&'a self) -> Tee {
        self.clone()
    }
}

/// Appends text to the log file directly (the panic hook must not depend on the subscriber).
#[cfg_attr(not(windows), allow(dead_code))]
fn append(path: &PathBuf, text: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(text.as_bytes());
    }
}

/// Initialises tracing (stderr + the log file) and installs the panic hook. Call first in `main`.
pub fn init() {
    let path = log_path();
    let _ = LOG.set(path.clone());
    let file = path.as_ref().and_then(|p| std::fs::File::create(p).ok());
    let tee = Tee(Arc::new(Mutex::new(file)));
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_ansi(false)
        .with_writer(tee)
        .init();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        log = ?path,
        "porsche start"
    );
    let shown = path.clone();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current();
        let text = format!("PANIC in thread {:?}: {info}\nbacktrace:\n{bt}\n", thread.name().unwrap_or("?"));
        eprintln!("{text}");
        if let Some(p) = &shown {
            append(p, &text);
        }
        #[cfg(all(windows, not(debug_assertions)))]
        {
            let where_ = shown.as_ref().map_or_else(|| "(журнал не записан)".to_string(), |p| p.display().to_string());
            message_box(&format!("Porsche Unleashed: ошибка запуска — подробности в {where_}\n\n{info}"));
        }
    }));
}

/// Reports a fatal error returned from `run` (the same box as a panic).
pub fn fatal(err: &anyhow::Error) {
    tracing::error!("fatal: {err:#}");
    #[cfg(all(windows, not(debug_assertions)))]
    {
        let where_ = LOG.get().cloned().flatten().unwrap_or_else(|| nfs_assets::platform::data_dir().join("porsche.log"));
        message_box(&format!("Porsche Unleashed: ошибка запуска — подробности в {}\n\n{err:#}", where_.display()));
    }
}

#[cfg(all(windows, not(debug_assertions)))]
fn message_box(text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (t, c) = (wide(text), wide("Porsche Unleashed"));
    // MB_OK | MB_ICONERROR
    unsafe { MessageBoxW(core::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10) };
}
