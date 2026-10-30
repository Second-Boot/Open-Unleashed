//! PLATFORM GLUE (macOS): SDL's event pump runs outside winit's event handler.
//!
//! `SDL_PollEvent` -> `SDL_UpdateJoysticks` -> the HIDAPI joystick detect -> `SDL_hid_enumerate` ->
//! `process_pending_events` (SDL `hidapi/mac/hid.c`) runs `CFRunLoopRunInMode(kCFRunLoopDefaultMode,
//! 0.001, ...)` on the **main** run loop until it is drained. That happens whenever the IOKit
//! notification port reports any HID device arriving or leaving (`HIDAPI_UpdateDiscovery`; any HID
//! device of the machine counts, a joystick is not needed). winit queues the Cocoa events that
//! arrive while its handler runs as `CFRunLoopPerformBlock` blocks in the default mode
//! (`observer.rs` `queue_closure`); a block pending at that moment runs inside SDL's nested loop,
//! *inside* `window_event(RedrawRequested)`, and winit's re-entrancy guard panics ("tried to handle
//! event while another event is currently being handled"). Seen once in ~8 returns from a race to
//! the front end (the resize / focus events of the mode change are the pending blocks).
//!
//! Moving the call into `about_to_wait` would not help: that is a winit handler too. The pump has
//! to run where winit's handler is not borrowed: a run-loop observer of our own, installed by
//! [`install`], calls `SDL_PollEvent` once per run-loop pass (before the sources, and after
//! waking) and parks the events in a queue; `Gamepads::poll` then consumes the queue at the same
//! place as before (the frame), so the pad's keys, the 32 Hz latch and the values read per tick are
//! unchanged — SDL's state is refreshed by the pump of the same pass, one pass earlier at most.
//! Windows and Linux keep the inline `SDL_PollEvent`.

use core_foundation_sys::base::{CFAllocatorRef, kCFAllocatorDefault};
use core_foundation_sys::runloop::{
    CFRunLoopActivity, CFRunLoopAddObserver, CFRunLoopGetMain, CFRunLoopObserverContext, CFRunLoopObserverCreate, CFRunLoopObserverRef,
    kCFRunLoopAfterWaiting, kCFRunLoopBeforeSources, kCFRunLoopCommonModes,
};
use sdl3_sys::events::{SDL_Event, SDL_PollEvent};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

thread_local! {
    /// the events the observer drained, in SDL's order; consumed by `Gamepads::poll`
    static QUEUE: RefCell<VecDeque<SDL_Event>> = const { RefCell::new(VecDeque::new()) };
    /// the observer fires again for the run-loop passes nested inside the pump (HIDAPI's own
    /// `CFRunLoopRunInMode`): no pump inside the pump
    static PUMPING: Cell<bool> = const { Cell::new(false) };
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// TEST GLUE: make every pump run `SDL_hid_enumerate` first, the call a HID device change makes
/// (`HIDAPI_UpdateDeviceList`), so the front-end return can be reproduced without plugging anything
/// (also `NFS_SDL_FORCE_ENUM=1` in the environment).
pub static FORCE_ENUMERATE: AtomicBool = AtomicBool::new(false);

/// Whether [`install`] ran on this thread (then `Gamepads::poll` reads the queue).
pub(crate) fn installed() -> bool {
    INSTALLED.with(Cell::get)
}

/// Drain SDL's event queue into ours. Called by the observer, and by `Gamepads::poll` itself when
/// no observer is installed (the inline path).
pub(crate) fn pump() {
    if PUMPING.with(|p| p.replace(true)) {
        return;
    }
    static ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if FORCE_ENUMERATE.load(Ordering::Relaxed) || *ENV.get_or_init(|| std::env::var_os("NFS_SDL_FORCE_ENUM").is_some_and(|v| v == "1")) {
        // SAFETY: plain enumeration; the list is freed at once
        unsafe { sdl3_sys::hidapi::SDL_hid_free_enumeration(sdl3_sys::hidapi::SDL_hid_enumerate(0, 0)) };
    }
    let mut ev = SDL_Event::default();
    // SAFETY: SDL_PollEvent fills `ev`
    while unsafe { SDL_PollEvent(&mut ev) } {
        QUEUE.with(|q| q.borrow_mut().push_back(ev));
    }
    PUMPING.with(|p| p.set(false));
}

/// The next parked event.
pub(crate) fn next() -> Option<SDL_Event> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}

extern "C" fn observer(_o: CFRunLoopObserverRef, _activity: CFRunLoopActivity, _info: *mut c_void) {
    pump();
}

/// Install the observer on the main run loop (call on the main thread, after SDL is initialised).
/// Returns `false` when it could not be created (the inline pump stays).
pub fn install() -> bool {
    if installed() {
        return true;
    }
    // TEST GLUE: `NFS_SDL_PUMP=inline` keeps the 0.1.5 behaviour (pump in the frame), for A/B runs
    if std::env::var_os("NFS_SDL_PUMP").is_some_and(|v| v == "inline") {
        return true;
    }
    let ctx = CFRunLoopObserverContext { version: 0, info: std::ptr::null_mut(), retain: None, release: None, copyDescription: None };
    // SAFETY: documented CoreFoundation calls; the observer lives as long as the process (the run loop
    // retains it), the callout touches only thread-locals and SDL on the main thread
    unsafe {
        let obs = CFRunLoopObserverCreate(
            kCFAllocatorDefault as CFAllocatorRef,
            kCFRunLoopBeforeSources | kCFRunLoopAfterWaiting,
            1,
            0,
            observer,
            &ctx as *const _ as *mut _,
        );
        if obs.is_null() {
            return false;
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), obs, kCFRunLoopCommonModes);
    }
    INSTALLED.with(|i| i.set(true));
    true
}
