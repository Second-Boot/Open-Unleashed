//! The project's own user-facing name (window title, error boxes) and icon. A working name: the packaging scripts
//! read the same one from `packaging/brand.sh`. It names no trademark of the original game's owners;
//! descriptions say which game it runs ("for Need for Speed: Porsche Unleashed (2000)").

/// The window title and the caption of the start-up error box.
pub const TITLE: &str = "OpenUnleashed";

/// **PLATFORM GLUE** (not in the original): the window's icon — the title bar and the task switcher on Windows, the
/// window list on Linux / X11 (macOS takes the app bundle's icon and ignores it). The picture is
/// `packaging/brand/icon.svg`, rendered by `packaging/brand/render_icons.py`. `None` if it fails to decode.
pub fn window_icon() -> Option<winit::window::Icon> {
    let rgba = image::load_from_memory(include_bytes!("../../../packaging/brand/icon-256.png")).ok()?.into_rgba8();
    let (w, h) = rgba.dimensions();
    winit::window::Icon::from_rgba(rgba.into_raw(), w, h).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_window_icon_decodes() {
        assert!(super::window_icon().is_some());
    }
}
