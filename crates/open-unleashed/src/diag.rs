//! Port-own diagnostics (not a rule of the original): a log file and a panic hook.
//!
//! A release build on Windows is a GUI-subsystem program (`windows_subsystem = "windows"`), so a
//! panic or an error return has no console to land on and the window just closes. Every release
//! run therefore writes `open-unleashed.log` (tracing at INFO, still echoed to stderr) into the port's
//! data directory (`nfs_assets::platform::data_dir`, `%APPDATA%\PorscheUnleashed` on Windows),
//! keeping the previous run as `open-unleashed.prev.log`; a panic writes its message and backtrace to the
//! log. A release build also shows a panic or a start-up error (no game data, ...) in a native message box
//! naming the log (Windows `MessageBoxW`, macOS `CFUserNotificationDisplayAlert`, Linux `zenity` / `kdialog`
//! when installed): started from the Dock, a desktop entry or Explorer there is no terminal to read.
//! Debug builds (development, the golden runs) log to stderr only, unless `NFS_LOG_FILE=1`, and show no box.
//! PORT-DEVIATION: the game's behaviour is untouched.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

static LOG: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Where the log goes; `None` when no file is wanted.
fn log_path() -> Option<PathBuf> {
    let want = !cfg!(debug_assertions) || std::env::var_os("NFS_LOG_FILE").is_some_and(|v| v != "0");
    if !want || std::env::var_os("NFS_NO_LOG_FILE").is_some() {
        return None;
    }
    // (read directly, not through `nfs_game::config`: `diag::init` runs before `main`'s `set_var`s, which the snapshot must see)
    // `NFS5_SAVE_DIR` replaces `SaveData`; the log then sits beside it, never in the real data dir
    let dir = std::env::var_os("NFS5_SAVE_DIR")
        .and_then(|d| PathBuf::from(d).parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(nfs_assets::platform::data_dir);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("open-unleashed.log");
    let _ = std::fs::rename(&path, dir.join("open-unleashed.prev.log"));
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
        "{} start",
        crate::brand::TITLE
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
        report(&format!("{} stopped with an error.", crate::brand::TITLE), &info.to_string(), shown.as_ref());
    }));
}

/// Reports a fatal error returned from `run` (the same box as a panic).
pub fn fatal(err: &anyhow::Error) {
    tracing::error!("fatal: {err:#}");
    let log = LOG.get().cloned().flatten();
    report(&format!("{} could not start.", crate::brand::TITLE), &format!("{err:#}"), log.as_ref());
}

/// The native box of a release build; never in a scripted run (`NFS_NO_FOCUS`: nobody would close it).
fn report(headline: &str, detail: &str, log: Option<&PathBuf>) {
    if cfg!(debug_assertions) || std::env::var_os("NFS_NO_FOCUS").is_some() {
        return;
    }
    let log = log.map_or_else(|| "(no log file was written)".to_string(), |p| p.display().to_string());
    message_box(&format!("{headline}\n\n{detail}\n\nLog: {log}"));
}

#[cfg(windows)]
fn message_box(text: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, ty: u32) -> i32;
    }
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (t, c) = (wide(text), wide(crate::brand::TITLE));
    // MB_OK | MB_ICONERROR
    unsafe { MessageBoxW(core::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10) };
}

/// macOS: a Core Foundation alert, which needs no running `NSApplication` (the error comes before the window exists).
#[cfg(target_os = "macos")]
fn message_box(text: &str) {
    use std::ffi::c_void;
    type CFStringRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithBytes(alloc: *const c_void, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CFStringRef;
        fn CFRelease(cf: *const c_void);
        #[allow(clippy::too_many_arguments)]
        fn CFUserNotificationDisplayAlert(
            timeout: f64,
            flags: usize,
            icon: *const c_void,
            sound: *const c_void,
            localization: *const c_void,
            header: CFStringRef,
            message: CFStringRef,
            default_button: CFStringRef,
            alternate_button: CFStringRef,
            other_button: CFStringRef,
            response: *mut usize,
        ) -> i32;
    }
    // kCFStringEncodingUTF8; flags 0 = kCFUserNotificationStopAlertLevel; null buttons = one "OK"
    const UTF8: u32 = 0x0800_0100;
    let cf = |s: &str| unsafe { CFStringCreateWithBytes(std::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) };
    let (header, message) = (cf(crate::brand::TITLE), cf(text));
    if header.is_null() || message.is_null() {
        return;
    }
    let null = std::ptr::null();
    let mut response = 0usize;
    unsafe {
        CFUserNotificationDisplayAlert(0.0, 0, null, null, null, header, message, null, null, null, &mut response);
        CFRelease(header);
        CFRelease(message);
    }
}

/// Linux and the other Unixes: the desktop's own dialog tool when one is installed (GNOME / KDE), else only the log and stderr.
#[cfg(all(unix, not(target_os = "macos")))]
fn message_box(text: &str) {
    use std::process::Command;
    let title = crate::brand::TITLE;
    if Command::new("zenity").args(["--error", "--no-markup", "--title", title, "--text", text]).status().is_ok() {
        return;
    }
    let _ = Command::new("kdialog").args(["--title", title, "--error", text]).status();
}
