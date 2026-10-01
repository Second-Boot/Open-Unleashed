//! **Portable transcendentals for the deterministic simulation** (PERF C, `RE/notes/PERF.md`).
//!
//! `f32::sin` / `cos` / `atan2` / `acos` / … call the operating system's libm (macOS libsystem,
//! glibc, Windows UCRT), whose last bit differs between them, so a race run on one OS could drift
//! from the same run on another. Everything the simulation (physics, AI, traffic, police, race,
//! net state, the scripted drive) takes from a transcendental goes through here instead: the
//! `libm` crate, pure Rust, the same bits on every OS. The width of each call is the caller's —
//! the `f32` functions take an `f32` (the std call it replaces was an `f32` one), the `f64` ones
//! an `f64`. `sqrt` is IEEE-exact on every platform and stays `f32::sqrt`.
//!
//! The glam constructors the simulation used (`Mat3::from_rotation_*`, `Quat::from_rotation_*`)
//! take their sine and cosine from std, so the same matrices / quaternions are built here from
//! the portable ones, with glam's own layout.
//!
//! PORT-DEVIATION D1 (`RE/notes/PORT-DEVIATIONS.md`): the original is x87 (`fsin` / `fcos` / `fpatan` at
//! 80 bits, the engine's helpers 0x5327E0 / 0x5327F0 / 0x532800 / 0x532860 / 0x532600 / 0x5325A0 /
//! 0x532660, CRT `acos` 0x5A1060). Neither this nor the OS libm is x87-exact. Do not switch the sites
//! back to std; an x87-exact emulation, if ever wanted, belongs here for every site at once.

use glam::{Mat3, Mat4, Quat, Vec3, Vec4};

pub use libm::{acos, acosf, asin, asinf, atan, atan2, atan2f, atanf, cos, cosf, sin, sinf, tan, tanf};

/// `(sin x, cos x)`, `f32` — `f32::sin_cos`.
#[inline]
pub fn sin_cosf(x: f32) -> (f32, f32) {
    (libm::sinf(x), libm::cosf(x))
}

/// `(sin x, cos x)`, `f64` — `f64::sin_cos`.
#[inline]
pub fn sin_cos(x: f64) -> (f64, f64) {
    (libm::sin(x), libm::cos(x))
}

/// `Mat3::from_rotation_x`.
pub fn mat3_rotation_x(angle: f32) -> Mat3 {
    let (s, c) = sin_cosf(angle);
    Mat3::from_cols(Vec3::X, Vec3::new(0.0, c, s), Vec3::new(0.0, -s, c))
}

/// `Mat3::from_rotation_y`.
pub fn mat3_rotation_y(angle: f32) -> Mat3 {
    let (s, c) = sin_cosf(angle);
    Mat3::from_cols(Vec3::new(c, 0.0, -s), Vec3::Y, Vec3::new(s, 0.0, c))
}

/// `Mat3::from_rotation_z`.
pub fn mat3_rotation_z(angle: f32) -> Mat3 {
    let (s, c) = sin_cosf(angle);
    Mat3::from_cols(Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0), Vec3::Z)
}

/// `Mat4::from_rotation_y`.
pub fn mat4_rotation_y(angle: f32) -> Mat4 {
    let (s, c) = sin_cosf(angle);
    Mat4::from_cols(Vec4::new(c, 0.0, -s, 0.0), Vec4::Y, Vec4::new(s, 0.0, c, 0.0), Vec4::W)
}

/// `Quat::from_rotation_x`.
pub fn quat_rotation_x(angle: f32) -> Quat {
    let (s, c) = sin_cosf(angle * 0.5);
    Quat::from_xyzw(s, 0.0, 0.0, c)
}

/// `Quat::from_rotation_y`.
pub fn quat_rotation_y(angle: f32) -> Quat {
    let (s, c) = sin_cosf(angle * 0.5);
    Quat::from_xyzw(0.0, s, 0.0, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The constructors agree with glam's where the two sines and cosines agree (exact angles).
    #[test]
    fn the_constructors_have_glams_layout() {
        for a in [0.0f32, 0.3, -1.2, 2.5, std::f32::consts::FRAC_PI_2] {
            let close = |x: Mat3, y: Mat3| (x.to_cols_array().iter().zip(y.to_cols_array())).all(|(p, q)| (p - q).abs() < 1e-6);
            assert!(close(mat3_rotation_x(a), Mat3::from_rotation_x(a)));
            assert!(close(mat3_rotation_y(a), Mat3::from_rotation_y(a)));
            assert!(close(mat3_rotation_z(a), Mat3::from_rotation_z(a)));
            assert!(mat4_rotation_y(a).abs_diff_eq(Mat4::from_rotation_y(a), 1e-6));
            assert!(quat_rotation_x(a).abs_diff_eq(Quat::from_rotation_x(a), 1e-6));
            assert!(quat_rotation_y(a).abs_diff_eq(Quat::from_rotation_y(a), 1e-6));
        }
    }

    /// The portable functions are fixed bit patterns: the same on every OS.
    #[test]
    fn the_values_are_pinned() {
        assert_eq!(sinf(1.0).to_bits(), 0x3F57_6AA4);
        assert_eq!(cosf(1.0).to_bits(), 0x3F0A_5140);
        assert_eq!(atan2f(1.0, 2.0).to_bits(), 0x3EED_6338);
        assert_eq!(acos(0.3).to_bits(), 0x3FF4_41F5_ECBE_EF59);
    }
}
