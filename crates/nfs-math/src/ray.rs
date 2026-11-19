//! Ray-triangle test shared by the render-side bakes (`nfs_game::gi`, `nfs_game::bake_light`). The physics' own test
//! (`nfs-physics` collision) is a different, simulation-side rule and stays apart.
use glam::Vec3;

/// Moeller-Trumbore, both faces: the ray `o + t d` against the triangle `a, a + e1, a + e2`. `None` when the ray is parallel
/// (`|det| < eps`) or outside the triangle; else `(t, u, v)` with `u`, `v` the barycentrics of corners 1 and 2. `t` is NOT range
/// checked (the caller decides which distances count).
#[inline]
pub fn moller_trumbore(o: Vec3, d: Vec3, a: Vec3, e1: Vec3, e2: Vec3, eps: f32) -> Option<(f32, f32, f32)> {
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < eps {
        return None;
    }
    let id = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * id;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * id;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some((e2.dot(q) * id, u, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_both_faces_and_misses_outside_or_parallel() {
        let (a, e1, e2) = (Vec3::ZERO, Vec3::X, Vec3::Z);
        let down = moller_trumbore(Vec3::new(0.25, 2.0, 0.25), -Vec3::Y, a, e1, e2, 1e-12).unwrap();
        assert_eq!(down, (2.0, 0.25, 0.25));
        assert_eq!(moller_trumbore(Vec3::new(0.25, -3.0, 0.25), Vec3::Y, a, e1, e2, 1e-12).unwrap().0, 3.0);
        assert!(moller_trumbore(Vec3::new(2.0, 2.0, 2.0), -Vec3::Y, a, e1, e2, 1e-12).is_none());
        assert!(moller_trumbore(Vec3::new(0.25, 2.0, 0.25), Vec3::X, a, e1, e2, 1e-12).is_none());
    }
}
