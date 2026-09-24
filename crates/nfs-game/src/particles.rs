//! What `Car_wheelEffects` **0x4A2880** hands each wheel's mark emitter, and the surface colours.
//!
//! Once per 64 Hz position step (0x49B84F, skipped for traffic, `car+0x52C & 0x80`) the original
//! walks the four wheels and hands each one's emitter a strength, a kind and the car's motion:
//!
//! ```text
//! for w in 0..4:
//!     (a, b) = w < 2 ? (0.1, 1.1104) : (0.5, 2.0)          ; front / rear
//!     surf = wheel+0x8C
//!     if surf == 9:                       emit(pos, 0.7,  9, dir, speed, 0.0, 1.0)
//!     elif surf == 6 or (wheel+0x80 and surf == 1): emit(pos, 0.7, 6, dir, speed, 0.0, 1.0)
//!     else:                               emit(pos, wheel+0xB8, surf, dir, speed, a, b)
//! ```
//!
//! and the emitter (0x478F30) stops when `strength <= a` and otherwise works with
//! `(strength − a)·b`. So a front tyre starts marking the road at a tenth of slip while a rear one
//! needs half. CONFIRMED. What it lays is a **skid mark** on the ground (`skidmarks.rs`,
//! `RE/notes/particles.md` §0) — an earlier reading took it for dust and smoke puffs, and the port
//! drew floating puffs until 2026-09-24.

/// `0x5D00A8[surface & 15]` — the mark colour per surface kind, ARGB bytes, read straight out of
/// the image (0x479200). Only the alpha is scaled by the emitter's strength (0x479213…0x479234).
/// `0x5D0128` is the same table for the no-effects path (unreachable: the emitter returns first).
pub const SURFACE_ARGB: [[u8; 4]; 16] = [
    [255, 0, 0, 0],
    [255, 0, 0, 0],
    [255, 0, 0, 0],
    [255, 120, 92, 39],
    [255, 0, 0, 0],
    [255, 78, 51, 13],
    [182, 89, 89, 89],
    [255, 0, 0, 0],
    [150, 179, 179, 179],
    [150, 179, 179, 179],
    [255, 0, 0, 0],
    [255, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0],
];

/// 0x4A2880 for one wheel: `(strength, kind, a, b)` for the emitter 0x478F30.
pub fn wheel_input(front: bool, surface: usize, wet: bool, slip: f32) -> (f32, usize, f32, f32) {
    let (a, b) = if front { (0.1f32, 1.1104f32) } else { (0.5f32, 2.0f32) };
    if surface == 9 {
        (0.7, 9, 0.0, 1.0)
    } else if surface == 6 || (wet && surface == 1) {
        (0.7, 6, 0.0, 1.0)
    } else {
        (slip, surface, a, b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_are_per_axle() {
        // a front tyre marks the road from a tenth of slip, a rear one from a half (0x4A2880)
        assert_eq!(wheel_input(true, 1, false, 0.2), (0.2, 1, 0.1, 1.1104));
        assert_eq!(wheel_input(false, 1, false, 0.2), (0.2, 1, 0.5, 2.0));
    }

    #[test]
    fn water_and_surface_nine_always_mark() {
        // wet tarmac and surface 6 take kind 6 at a fixed 0.7 over no threshold
        assert_eq!(wheel_input(false, 1, true, 0.0), (0.7, 6, 0.0, 1.0));
        assert_eq!(wheel_input(false, 9, false, 0.0).1, 9);
    }
}
