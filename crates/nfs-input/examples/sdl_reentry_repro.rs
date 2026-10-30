//! TEST GLUE: reproduction of the macOS winit re-entrancy panic (SDL's HIDAPI detect pumps the main
//! run loop inside `RedrawRequested`). Every frame it resizes the window by a pixel (winit queues the
//! resulting Cocoa event as a run-loop block, because its handler is running), then polls the pads
//! the way the game does. `FORCE_ENUMERATE` stands for a HID device change.
//!
//! `cargo run -p nfs-input --example sdl_reentry_repro -- inline|observer [frames]`
//! `inline` = the 0.1.5 behaviour (panics), `observer` = the fix (survives).
#[cfg(target_os = "macos")]
mod repro {
    use nfs_input::gamepad::Gamepads;
    use winit::application::ApplicationHandler;
    use winit::dpi::LogicalSize;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, EventLoop};
    use winit::window::{Window, WindowId};

    struct App {
        window: Option<Window>,
        pads: Gamepads,
        input: nfs_input::InputState,
        observer: bool,
        frames: u32,
        max: u32,
        flip: bool,
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_none() {
                self.window = Some(el.create_window(Window::default_attributes().with_inner_size(LogicalSize::new(320, 200))).unwrap());
                if self.observer {
                    self.pads.pump_outside_handler();
                }
                nfs_input::runloop_pump::FORCE_ENUMERATE.store(true, std::sync::atomic::Ordering::Relaxed);
                self.window.as_ref().unwrap().request_redraw();
            }
        }
        fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, ev: WindowEvent) {
            if let WindowEvent::RedrawRequested = ev {
                let w = self.window.as_ref().unwrap();
                self.flip = !self.flip;
                let _ = w.request_inner_size(LogicalSize::new(320 + u32::from(self.flip), 200));
                self.pads.poll(&mut self.input);
                self.frames += 1;
                if self.frames >= self.max {
                    println!("OK: {} frames, no panic", self.frames);
                    el.exit();
                } else {
                    w.request_redraw();
                }
            }
        }
    }

    pub fn main() {
        let mut a = std::env::args().skip(1);
        let observer = a.next().as_deref() == Some("observer");
        let max = a.next().and_then(|s| s.parse().ok()).unwrap_or(200);
        let el = EventLoop::new().unwrap();
        let mut app = App { window: None, pads: Gamepads::new(), input: Default::default(), observer, frames: 0, max, flip: false };
        el.run_app(&mut app).unwrap();
    }
}

fn main() {
    #[cfg(target_os = "macos")]
    repro::main();
}
