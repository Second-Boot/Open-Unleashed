//! The application's handle on the event loop: the real winit loop of a windowed run, or none at all for the server's headless
//! re-simulation (`--resim`, `app/resim.rs`), which has no display server to open a loop on. The application only ever exits the loop and
//! creates its one window through it, so these two are all the handle offers.
//!
//! **TOOLING, not the original's.**

use std::cell::Cell;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

pub(crate) struct LoopCtl<'a> {
    real: Option<&'a ActiveEventLoop>,
    exited: Cell<bool>,
}

impl<'a> LoopCtl<'a> {
    /// The handle on winit's running loop.
    pub(crate) fn real(el: &'a ActiveEventLoop) -> Self {
        Self { real: Some(el), exited: Cell::new(false) }
    }

    /// No loop: a headless run.
    pub(crate) fn headless() -> Self {
        Self { real: None, exited: Cell::new(false) }
    }

    /// End the run (winit's `exit`; headless: only remembered, see [`Self::exited`]).
    pub(crate) fn exit(&self) {
        self.exited.set(true);
        if let Some(el) = self.real {
            el.exit();
        }
    }

    /// winit's `exiting`: the loop is shutting down (never, headless).
    pub(crate) fn exiting(&self) -> bool {
        self.real.is_some_and(|el| el.exiting())
    }

    /// Whether [`Self::exit`] was called on this handle.
    pub(crate) fn exited(&self) -> bool {
        self.exited.get()
    }

    /// The window of a windowed run. A headless run never asks for one (`start_race` takes none).
    pub(crate) fn create_window(&self, attrs: WindowAttributes) -> Result<Window, winit::error::OsError> {
        self.real.expect("a headless run opens no window").create_window(attrs)
    }
}
