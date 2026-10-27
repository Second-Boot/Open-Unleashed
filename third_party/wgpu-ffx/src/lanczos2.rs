//! Utility module for generating lanczos2 LUT.
//!

use std::f32::consts::PI;

const FFX_EPSILON: f32 = 1e-06;
const LUT_SIZE: usize = 128;
pub type Lanczos2Lut = [i16; LUT_SIZE];

/// Generate the lanczos2 LUT
///
/// Translated from ffx/sdk/src/components/fsr3upscaler/ffx_fsr3upscaler.cpp:540-548
pub fn generate_lanczos2_lut() -> Lanczos2Lut {
    std::array::from_fn(|i| {
        let x = 2.0_f32 * i as f32 / (LUT_SIZE as f32 - 1.0);
        let y = lanczos2(x);
        (y * 32767.0f32).round() as i16
    })
}

/// Lanczos2 filter function
///
/// Translated from ffx/sdk/src/components/fsr3upscaler/ffx_fsr3upscaler.cpp:163-167
pub fn lanczos2(value: f32) -> f32 {
    if value.abs() < FFX_EPSILON {
        return 1.0;
    }

    let v_pi = PI * value;
    let half_v_pi = 0.5 * v_pi;
    (v_pi.sin() / v_pi) * (half_v_pi.sin() / half_v_pi)
}
