//! Halton-sequence jitter for temporal anti-aliasing.
//!
//! FSR3 requires sub-pixel jitter applied to the projection matrix each frame.
//! This module provides helpers to compute the jitter phase count and per-frame
//! offsets using a Halton sequence (bases 2 and 3).

/// Calculate Halton number for the given index and base.
fn halton(index: i32, base: i32) -> f32 {
    let mut f = 1.0f32;
    let mut result = 0.0f32;
    let mut current_index = index;

    while current_index > 0 {
        f /= base as f32;
        result += f * (current_index % base) as f32;
        current_index /= base;
    }

    result
}

/// Return the number of jitter phases for the given render and display widths.
///
/// The phase count scales quadratically with the upscale ratio
/// (`display_width / render_width`), starting from a base of 8 at 1:1.
pub fn get_jitter_phase_count(render_width: i32, display_width: i32) -> i32 {
    const BASE_PHASE_COUNT: f32 = 8.0;

    (BASE_PHASE_COUNT * f32::powf(display_width as f32 / render_width as f32, 2.0)) as i32
}

/// Compute the sub-pixel jitter offset for the given frame index.
///
/// Returns `[x, y]` offsets in the range `[-0.5, 0.5]`, suitable for passing
/// directly to [`FsrDispatchInfo::jitter_offset`](crate::FsrDispatchInfo::jitter_offset).
/// The `phase_count` should come from [`get_jitter_phase_count`].
pub fn get_jitter_offset(index: i32, phase_count: i32) -> [f32; 2] {
    let x = halton((index % phase_count) + 1, 2) - 0.5f32;
    let y = halton((index % phase_count) + 1, 3) - 0.5f32;

    [x, y]
}
