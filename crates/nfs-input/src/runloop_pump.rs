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
use sdl3_sys::events::{SDL_EVENT_GAMEPAD_AXIS_MOTION, SDL_EVENT_JOYSTICK_AXIS_MOTION, SDL_Event, SDL_EventType, SDL_PollEvent};
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
/// (set by `examples/sdl_reentry_repro.rs`; the `NFS_SDL_FORCE_ENUM` env read was removed 2026-11-19, REFACTOR-0119).
pub static FORCE_ENUMERATE: AtomicBool = AtomicBool::new(false);

/// Whether [`install`] ran on this thread (then `Gamepads::poll` reads the queue).
pub(crate) fn installed() -> bool {
    INSTALLED.with(Cell::get)
}

/// Most events the queue keeps while no frame drains it (PLATFORM GLUE). A pad sends about 1000 axis
/// events a second, and 4096 is about four seconds of them: far more than a frame gap (~17 ms at
/// 60 Hz) or a stall of the run loop, so a live queue is never trimmed. Beyond it the oldest axis
/// motion is dropped (see [`cap_queue`]); at 128 bytes an event the cap bounds the axis backlog to
/// about 512 KB, where an unbounded queue grew for as long as the intro movies or a hidden window ran.
pub(crate) const QUEUE_CAP: usize = 4096;

/// Whether an event is axis motion (a stick or trigger sample): the only kind [`cap_queue`] may drop.
/// Device added / removed and button events are never dropped: the pad list and the presses must
/// stay right however long the queue waited.
fn is_axis_motion(ev: &SDL_Event) -> bool {
    // SAFETY: the type field is valid in every union member
    let ty = SDL_EventType(unsafe { ev.r#type });
    ty == SDL_EVENT_GAMEPAD_AXIS_MOTION || ty == SDL_EVENT_JOYSTICK_AXIS_MOTION
}

/// Trim `q` to at most `cap` entries by dropping the **oldest** axis-motion entries first, keeping
/// the order of what stays. Entries that `is_axis` rejects are never dropped, so the queue may stay
/// longer than `cap` when it holds nothing else to drop. Only a backlog of over `cap` events is
/// trimmed (seconds of samples), so the newest samples survive; an axis whose last sample is older
/// than all the dropped ones can lose it — the race reads the pad's axis state from SDL, not from
/// these events. Returns how many entries were dropped.
fn cap_queue<T>(q: &mut VecDeque<T>, cap: usize, is_axis: impl Fn(&T) -> bool) -> usize {
    if q.len() <= cap {
        return 0;
    }
    let mut excess = q.len() - cap;
    let mut dropped = 0;
    // `retain` visits the entries front (oldest) to back, so the oldest axis entries go first
    q.retain(|e| {
        if excess > 0 && is_axis(e) {
            excess -= 1;
            dropped += 1;
            false
        } else {
            true
        }
    });
    dropped
}

/// Drain SDL's event queue into ours. Called by the observer, and by `Gamepads::poll` itself when
/// no observer is installed (the inline path).
pub(crate) fn pump() {
    if PUMPING.with(|p| p.replace(true)) {
        return;
    }
    if FORCE_ENUMERATE.load(Ordering::Relaxed) {
        // SAFETY: plain enumeration; the list is freed at once
        unsafe { sdl3_sys::hidapi::SDL_hid_free_enumeration(sdl3_sys::hidapi::SDL_hid_enumerate(0, 0)) };
    }
    let mut ev = SDL_Event::default();
    // SAFETY: SDL_PollEvent fills `ev`
    // the borrow is taken per push, not across SDL_PollEvent (SDL may run nested loops in it)
    while unsafe { SDL_PollEvent(&mut ev) } {
        QUEUE.with(|q| q.borrow_mut().push_back(ev));
    }
    QUEUE.with(|q| cap_queue(&mut q.borrow_mut(), QUEUE_CAP, is_axis_motion));
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

#[cfg(test)]
mod tests {
    use super::*;
    use sdl3_sys::events::{SDL_EVENT_GAMEPAD_ADDED, SDL_EVENT_GAMEPAD_BUTTON_DOWN, SDL_EVENT_JOYSTICK_ADDED, SDL_EVENT_JOYSTICK_REMOVED};

    /// (is an axis sample, a tag for the order checks)
    fn ev(axis: bool, tag: u32) -> (bool, u32) {
        (axis, tag)
    }

    #[test]
    fn a_queue_within_the_cap_is_untouched() {
        let mut q: VecDeque<_> = (0..10).map(|i| ev(true, i)).collect();
        assert_eq!(cap_queue(&mut q, 10, |e| e.0), 0);
        assert_eq!(q.len(), 10);
    }

    #[test]
    fn the_oldest_axis_samples_go_first_and_the_order_is_kept() {
        let mut q: VecDeque<_> = (0..10).map(|i| ev(true, i)).collect();
        assert_eq!(cap_queue(&mut q, 4, |e| e.0), 6);
        let tags: Vec<u32> = q.iter().map(|e| e.1).collect();
        assert_eq!(tags, vec![6, 7, 8, 9]);
    }

    #[test]
    fn events_that_are_not_axis_motion_are_never_dropped() {
        // buttons / device events at the front (the oldest) and between the axis samples
        let mut q: VecDeque<_> = VecDeque::new();
        q.push_back(ev(false, 100)); // oldest: a press
        for i in 0..6 {
            q.push_back(ev(true, i));
            if i == 2 {
                q.push_back(ev(false, 200)); // a device added in the middle
            }
        }
        // 8 entries, cap 4: 4 axis samples must go, the two non-axis entries stay
        assert_eq!(cap_queue(&mut q, 4, |e| e.0), 4);
        let tags: Vec<u32> = q.iter().map(|e| e.1).collect();
        assert_eq!(tags, vec![100, 200, 4, 5]);
    }

    #[test]
    fn a_queue_of_only_other_events_stays_long() {
        let mut q: VecDeque<_> = (0..10).map(|i| ev(false, i)).collect();
        assert_eq!(cap_queue(&mut q, 4, |e| e.0), 0);
        assert_eq!(q.len(), 10);
    }

    #[test]
    fn only_axis_motion_events_are_droppable() {
        let mut e = SDL_Event::default();
        let mut kind = |t: SDL_EventType| {
            e.r#type = t.0;
            is_axis_motion(&e)
        };
        assert!(kind(SDL_EVENT_GAMEPAD_AXIS_MOTION));
        assert!(kind(SDL_EVENT_JOYSTICK_AXIS_MOTION));
        assert!(!kind(SDL_EVENT_GAMEPAD_BUTTON_DOWN));
        assert!(!kind(SDL_EVENT_GAMEPAD_ADDED));
        assert!(!kind(SDL_EVENT_JOYSTICK_ADDED));
        assert!(!kind(SDL_EVENT_JOYSTICK_REMOVED));
    }
}
