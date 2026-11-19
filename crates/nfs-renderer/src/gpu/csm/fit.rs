//! ENHANCEMENT E9: the cascades' fit: [`Cascade`], the light's axes, the snapped sphere round a view slice ([`fit`], [`fit_focus`])
//! and the lifted elevation of the shadow's light. Pure maths on a camera and a direction.

use super::*;

/// A cascade's sphere radius is rounded up to whole steps of this many per metre, so the box keeps its size as the camera turns.
pub(super) const RADIUS_STEPS_PER_M: f32 = 8.0;
/// The depth margin (m) added to a cascade's range beyond its sphere and the casters' reach.
pub(super) const RANGE_MARGIN: f32 = 2.0;
/// A slice of the view depth thinner than this (m) gets no cascade (the camera's far plane cut it off).
pub(super) const MIN_SLICE: f32 = 0.25;
/// The light's travel direction counts as vertical (the map's axes then start from X instead of Y) above this `|y|`.
pub(super) const VERTICAL_Y: f32 = 0.99;

/// ENHANCEMENT ENH-FE3D: the front-end cascade's split: beyond any view depth, so the map is read for every pixel.
pub(super) const FOCUS_SPLIT: f32 = 1.0e6;

/// One fitted cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::gpu) struct Cascade {
    /// world → the map's clip space (x, y in -1..1, z in 0..1 along the light)
    pub mat: Mat4,
    /// a texel's size in the world (m)
    pub texel: f32,
    /// the depth range the 0..1 of z covers (m)
    pub range: f32,
    /// the box's half width (m) and its centre in light space (x, y, and the near depth)
    pub half: f32,
    pub centre: [f32; 2],
    pub zmin: f32,
    /// the view depth where this cascade ends
    pub split: f32,
}

impl Cascade {
    /// Whether a caster's sphere can shade the cascade: against the box in light space (toward the sun it is open).
    pub(super) fn reaches(&self, axes: (Vec3, Vec3, Vec3), c: &Caster) -> bool {
        let (ax, ay, d) = axes;
        let (lx, ly, lz) = (ax.dot(c.centre), ay.dot(c.centre), d.dot(c.centre));
        let lim = self.half + c.radius;
        (lx - self.centre[0]).abs() <= lim && (ly - self.centre[1]).abs() <= lim && lz - c.radius <= self.zmin + self.range
    }
}

/// The light's axes for a travel direction `d` (unit): the map's x and y, deterministic in `d`.
pub(in crate::gpu) fn light_axes(d: Vec3) -> (Vec3, Vec3) {
    let r = if d.y.abs() > VERTICAL_Y { Vec3::X } else { Vec3::Y };
    let x = r.cross(d).normalize();
    let y = d.cross(x).normalize();
    (x, y)
}

/// The cascade of a sphere (`centre`, radius `r` rounded up) along the light `d` (unit): the box snapped to whole texels of a
/// `size` map, `ext` metres of depth reserved toward the sun for casters, ending at view depth `split`.
pub(super) fn snapped(d: Vec3, centre: Vec3, r: f32, size: u32, ext: f32, split: f32) -> Cascade {
    let (ax, ay) = light_axes(d);
    let r = (r * RADIUS_STEPS_PER_M).ceil() / RADIUS_STEPS_PER_M;
    let texel = 2.0 * r / size as f32;
    let (lx, ly, lz) = (ax.dot(centre), ay.dot(centre), d.dot(centre));
    let (cx, cy) = ((lx / texel).round() * texel, (ly / texel).round() * texel);
    let range = (2.0 * r + ext).ceil() + RANGE_MARGIN;
    let zmin = (lz - r - ext).floor();
    let mat = Mat4::from_cols(
        Vec4::new(ax.x / r, ay.x / r, d.x / range, 0.0),
        Vec4::new(ax.y / r, ay.y / r, d.y / range, 0.0),
        Vec4::new(ax.z / r, ay.z / r, d.z / range, 0.0),
        Vec4::new(-cx / r, -cy / r, -zmin / range, 1.0),
    );
    Cascade { mat, texel, range, half: r, centre: [cx, cy], zmin, split }
}

/// ENHANCEMENT ENH-FE3D: the one cascade of a front-end car's shadow — a sphere round the car, the box snapped to whole
/// texels like [`fit`]'s; its split is far beyond any view depth, so the map is read for every pixel (the cascade's own
/// box says where a shadow can fall, outside it the point is lit).
pub(in crate::gpu) fn fit_focus(centre: Vec3, radius: f32, dir: Vec3, size: u32, ext: f32) -> Cascade {
    snapped(dir.normalize(), centre, radius, size, ext, FOCUS_SPLIT)
}

/// ENHANCEMENT E9 (SHADOWFIX): the lowest elevation the shadows' light has over the horizon (degrees).
pub(super) const MIN_ELEVATION_DEG: f32 = 30.0;
/// A direction's least horizontal length squared to have an azimuth to keep.
pub(super) const MIN_HORIZONTAL_SQ: f32 = 1e-6;

/// `d` (a travel direction) with its elevation raised to at least [`MIN_ELEVATION_DEG`], azimuth kept.
pub(in crate::gpu) fn lift_elevation(d: Vec3) -> Vec3 {
    let (s, c) = MIN_ELEVATION_DEG.to_radians().sin_cos();
    let h = Vec3::new(d.x, 0.0, d.z);
    if -d.y >= s || h.length_squared() < MIN_HORIZONTAL_SQ {
        return d;
    }
    let h = h.normalize() * c;
    Vec3::new(h.x, -s, h.z)
}

/// The cascades for the camera: slice `[prev end, end]` of the view depth each, a sphere around the
/// slice, the box snapped to texels. `ext` is the depth reserved toward the sun for casters.
pub(in crate::gpu) fn fit(camera: &Camera, aspect: f32, dir: Vec3, splits: &[f32], size: u32, ext: f32) -> Vec<Cascade> {
    let d = dir.normalize();
    let fwd = (camera.target - camera.position).normalize();
    let ty = (camera.fov_y_radians * 0.5).tan();
    let tx = ty * aspect;
    let k2 = tx * tx + ty * ty;
    let mut out = Vec::new();
    let mut near = camera.near;
    for &end in splits {
        let far = end.min(camera.far);
        if far <= near + MIN_SLICE {
            break;
        }
        // the smallest sphere on the view axis holding the slice's eight corners (centre at depth
        // `(n + f)(1 + k²)/2`, held at the far plane's centre when that lies beyond it); its radius depends on nothing the
        // camera's heading changes
        let c = (0.5 * (near + far) * (1.0 + k2)).min(far);
        let r = (far * far * k2 + (far - c) * (far - c)).sqrt();
        out.push(snapped(d, camera.position + fwd * c, r, size, ext, far));
        near = far;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam(pos: Vec3, target: Vec3) -> Camera {
        Camera { position: pos, target, up: Vec3::Y, fov_y_radians: 45f32.to_radians(), near: 0.3, far: 700.0 }
    }

    #[test]
    fn cascade_size_does_not_depend_on_the_heading() {
        let d = Vec3::new(0.3, -0.8, 0.2);
        let a = fit(&cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(10.0, 2.0, 105.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
        let b = fit(&cam(Vec3::new(10.0, 2.0, 5.0), Vec3::new(110.0, 2.0, 5.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
        assert_eq!(a.len(), 2);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!((x.half, x.texel, x.range), (y.half, y.texel, y.range));
        }
    }

    #[test]
    fn box_centre_is_snapped_to_whole_texels() {
        let d = Vec3::new(0.3, -0.8, 0.2);
        for k in 0..20 {
            let p = Vec3::new(3.0 + k as f32 * 0.0137, 2.0, 5.0 + k as f32 * 0.0291);
            let cs = fit(&cam(p, p + Vec3::new(0.2, 0.0, 1.0)), 1.6, d, &[40.0, 140.0], 1024, 120.0);
            for c in &cs {
                for v in c.centre {
                    let t = v / c.texel;
                    assert!((t - t.round()).abs() < 1e-3, "{t}");
                }
            }
        }
    }

    #[test]
    fn a_point_in_the_slice_lands_inside_the_map() {
        let d = Vec3::new(0.3, -0.8, 0.2).normalize();
        let c = cam(Vec3::new(100.0, 5.0, -40.0), Vec3::new(130.0, 4.0, 10.0));
        let cs = fit(&c, 1.6, d, &[40.0, 140.0], 1024, 120.0);
        let fwd = (c.target - c.position).normalize();
        let p = c.position + fwd * 20.0;
        let q = cs[0].mat * p.extend(1.0);
        assert!(q.x.abs() < 1.0 && q.y.abs() < 1.0 && (0.0..1.0).contains(&q.z), "{q:?}");
    }
}
