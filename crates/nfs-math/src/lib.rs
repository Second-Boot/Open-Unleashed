//! Math helpers shared by the native reimplementation.
//!
//! Coordinate conventions are recorded in `RE/FORMATS.md` once established from evidence.
pub use glam;

pub mod portable;
pub mod ray;

/// The box a fold over no point leaves: `min` = `f32::MAX`, `max` = `f32::MIN` in every axis. [`aabb_of`] is `None` instead.
pub const EMPTY_AABB: (glam::Vec3, glam::Vec3) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));

/// The cubic smoothstep of an already clamped `t` (0..=1): `t * t * (3 - 2t)`, the expression every ramp of the port writes inline.
#[inline]
pub fn smooth01(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// The axis-aligned box `(min, max)` of a point set, `None` for an empty one. The same fold, from [`EMPTY_AABB`], that the loaders
/// wrote out by hand (`min` / `max` per point in order), so the bits are the same.
pub fn aabb_of(points: impl IntoIterator<Item = glam::Vec3>) -> Option<(glam::Vec3, glam::Vec3)> {
    let mut any = false;
    let (lo, hi) = points.into_iter().fold(EMPTY_AABB, |(l, h), p| {
        any = true;
        (l.min(p), h.max(p))
    });
    any.then_some((lo, hi))
}

#[cfg(test)]
mod aabb_tests {
    #[test]
    fn smooth01_is_the_inline_expression() {
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            assert_eq!(super::smooth01(t).to_bits(), (t * t * (3.0 - 2.0 * t)).to_bits());
        }
        assert_eq!((super::smooth01(0.0), super::smooth01(1.0), super::smooth01(0.5)), (0.0, 1.0, 0.5));
    }

    use super::*;
    use glam::Vec3;

    #[test]
    fn the_box_of_some_points() {
        let pts = [Vec3::new(1.0, -2.0, 3.0), Vec3::new(-4.0, 5.0, 0.5), Vec3::new(0.0, 0.0, -7.0)];
        assert_eq!(aabb_of(pts), Some((Vec3::new(-4.0, -2.0, -7.0), Vec3::new(1.0, 5.0, 3.0))));
        assert_eq!(aabb_of(pts[..1].iter().copied()), Some((pts[0], pts[0])));
    }

    #[test]
    fn no_points_no_box_and_the_empty_fold_is_the_sentinel() {
        assert_eq!(aabb_of(std::iter::empty()), None);
        let fold = std::iter::empty::<Vec3>().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p), h.max(p)));
        assert_eq!(fold, EMPTY_AABB);
    }
}
