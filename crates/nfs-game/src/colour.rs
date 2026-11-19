//! The `0xAARRGGBB` colour dword as floats, in one place (each byte over 255).

/// A colour byte's full scale.
const BYTE_MAX: f32 = 255.0;

/// An `ARGB` dword as `[r, g, b, a]` in 0..1.
pub fn argb_rgba(c: u32) -> [f32; 4] {
    [((c >> 16) & 0xFF) as f32 / BYTE_MAX, ((c >> 8) & 0xFF) as f32 / BYTE_MAX, (c & 0xFF) as f32 / BYTE_MAX, (c >> 24) as f32 / BYTE_MAX]
}

/// An `ARGB` dword's `[r, g, b]` in 0..1 ([`argb_rgba`] without the alpha).
pub fn argb_rgb(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xFF) as f32 / BYTE_MAX, ((c >> 8) & 0xFF) as f32 / BYTE_MAX, (c & 0xFF) as f32 / BYTE_MAX]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channels_are_bytes_over_255() {
        assert_eq!(argb_rgba(0x80FF_0010), [255.0 / 255.0, 0.0, 16.0 / 255.0, 128.0 / 255.0]);
        assert_eq!(argb_rgb(0x80FF_0010), [1.0, 0.0, 16.0 / 255.0]);
        assert_eq!(argb_rgba(0xFF00_0000), [0.0, 0.0, 0.0, 1.0]);
    }
}
