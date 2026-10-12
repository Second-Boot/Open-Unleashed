//! The keyboard ring the blocking waits read (0x55FF60).

use super::*;

/// **0x55FF60** — the next key of the keyboard ring, which the blocking waits test against ESC (0x1B):
/// PLAYGAME's wait (0x50CFD7) and the barrier's four (0x48CE6F, 0x48CFA7, 0x48D0C1, 0x48D248). The
/// original's loops pump its own window's messages; the port's do not run the event loop, so this is
/// **platform glue**: on macOS the OS key state of Escape (`CGEventSourceKeyState`) — not in a scripted
/// `NFS_NO_FOCUS` run, which has no user at the keyboard — and, for the tests, **TEST GLUE**
/// `NFS_NET_ESC_MS=<ms>` (an ESC that arrives that long after the wait began). Elsewhere: never.
#[derive(Debug, Clone, Copy)]
pub struct KeyRing {
    since: Instant,
}

impl KeyRing {
    pub fn begin() -> Self {
        Self { since: Instant::now() }
    }

    /// Whether the next key is ESC.
    pub fn esc(&self) -> bool {
        if let Some(ms) = std::env::var("NFS_NET_ESC_MS").ok().and_then(|v| v.parse::<u64>().ok())
            && self.since.elapsed() >= Duration::from_millis(ms)
        {
            return true;
        }
        os_escape_down()
    }
}

#[cfg(target_os = "macos")]
fn os_escape_down() -> bool {
    unsafe extern "C" {
        #[link_name = "CGEventSourceKeyState"]
        fn key_state(state_id: i32, key: u16) -> bool;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {}
    if crate::NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    // kCGEventSourceStateCombinedSessionState = 0, kVK_Escape = 0x35
    unsafe { key_state(0, 0x35) }
}

/// Windows: `GetAsyncKeyState(VK_ESCAPE)`, the high bit = key currently down (user32, always linked
/// by winit).
#[cfg(windows)]
fn os_escape_down() -> bool {
    unsafe extern "system" {
        fn GetAsyncKeyState(v_key: i32) -> i16;
    }
    if crate::NO_FOCUS.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    // VK_ESCAPE = 0x1B
    unsafe { GetAsyncKeyState(0x1B) < 0 }
}

/// Linux and the rest: no global key state is readable without the window system (X11 / Wayland
/// give no portable poll); the Esc that reaches the window is handled by the event loop, and the
/// `NFS_NET_ESC_MS` test glue works everywhere. So `false`.
#[cfg(not(any(target_os = "macos", windows)))]
fn os_escape_down() -> bool {
    false
}
