//! The windscreen glass as the screen sees it ([`glass`]).

use super::*;

/// The part of the glass, as the screen sees it: the homography from NDC to glass `(u, v)` (0..1, `u` to the screen's
/// right, `v` up), the affine `1 / distance` over NDC and the glass's size and in-plane axes in the world.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Glass {
    /// NDC (x, y, 1) -> (u w, v w, w)
    pub to_uv: Mat3,
    /// `1 / distance = a x + b y + c` at NDC (x, y)
    pub inv_dist: Vec3,
    /// metres: width, height
    pub size: [f32; 2],
    /// gravity's in-plane components along (u, v): (0, -sin of the tilt) for an upright car
    pub gravity: [f32; 2],
    /// the quad's box in NDC: min x, min y, max x, max y
    pub ndc_box: [f32; 4],
}

/// A corner nearer the eye than this (clip w, m) and a glass narrower than this (m) give no glass.
pub(super) const MIN_GLASS_DEPTH: f32 = 0.05;
pub(super) const MIN_GLASS_SIDE: f32 = 0.05;
/// The glass box's padding on the screen (px).
pub(super) const BOX_PAD: f32 = 3.0;
/// The scratch copy's slack (px) so the box's wobble (the camera shake) does not reallocate.
pub(super) const SCRATCH_SLACK: u32 = 64;

/// Project the quad with the frame's camera (camera-relative, like the haze: a world position thousands of metres
/// out has no precision left in a f32 matrix) and work out the glass; `None` when a corner is behind the camera or the
/// quad is degenerate on the screen.
pub(crate) fn glass(d: &Drops, camera: &Camera, aspect: f32) -> Option<Glass> {
    let rel_cam = Camera { position: Vec3::ZERO, target: camera.target - camera.position, ..*camera };
    let vp = rel_cam.view_projection(aspect);
    let mut ndc = [Vec2::ZERO; 4];
    let mut dist = [0.0f32; 4];
    for (i, c) in d.corners.iter().enumerate() {
        let p = vp * (*c - camera.position).extend(1.0);
        if p.w < MIN_GLASS_DEPTH {
            return None;
        }
        ndc[i] = Vec2::new(p.x, p.y) / p.w;
        dist[i] = p.w;
    }
    // BL, BR, TR, TL by screen position: the two highest are the top, the left of each pair
    let mut idx = [0usize, 1, 2, 3];
    idx.sort_by(|&a, &b| ndc[b].y.total_cmp(&ndc[a].y));
    let (top, bot) = ([idx[0], idx[1]], [idx[2], idx[3]]);
    let (tl, tr) = if ndc[top[0]].x <= ndc[top[1]].x { (top[0], top[1]) } else { (top[1], top[0]) };
    let (bl, br) = if ndc[bot[0]].x <= ndc[bot[1]].x { (bot[0], bot[1]) } else { (bot[1], bot[0]) };
    let (p00, p10, p11, p01) = (ndc[bl], ndc[br], ndc[tr], ndc[tl]);
    // the unit square to the quad (Heckbert), then its inverse
    let d1 = p10 - p11;
    let d2 = p01 - p11;
    let d3 = p00 - p10 + p11 - p01;
    let den = d1.x * d2.y - d2.x * d1.y;
    if den.abs() < 1e-9 {
        return None;
    }
    let g = (d3.x * d2.y - d2.x * d3.y) / den;
    let h = (d1.x * d3.y - d3.x * d1.y) / den;
    let a = Mat3::from_cols(
        Vec3::new(p10.x - p00.x + g * p10.x, p10.y - p00.y + g * p10.y, g),
        Vec3::new(p01.x - p00.x + h * p01.x, p01.y - p00.y + h * p01.y, h),
        Vec3::new(p00.x, p00.y, 1.0),
    );
    if a.determinant().abs() < 1e-12 {
        return None;
    }
    let to_uv = a.inverse();
    // 1 / distance is affine in NDC over a plane: fit it on three corners
    let rows = Mat3::from_cols(Vec3::new(p00.x, p10.x, p01.x), Vec3::new(p00.y, p10.y, p01.y), Vec3::ONE);
    if rows.determinant().abs() < 1e-12 {
        return None;
    }
    let inv_dist = rows.inverse() * Vec3::new(1.0 / dist[bl], 1.0 / dist[br], 1.0 / dist[tl]);
    let w = d.corners;
    let u_axis = ((w[tr] - w[tl]) + (w[br] - w[bl])).normalize_or_zero();
    let v_axis = ((w[tl] - w[bl]) + (w[tr] - w[br])).normalize_or_zero();
    let width = ((w[tr] - w[tl]).length() + (w[br] - w[bl]).length()) * 0.5;
    let height = ((w[tl] - w[bl]).length() + (w[tr] - w[br]).length()) * 0.5;
    if width < MIN_GLASS_SIDE || height < MIN_GLASS_SIDE {
        return None;
    }
    let down = -d.up.normalize_or_zero();
    let ndc_box = [ndc.iter().map(|p| p.x).fold(f32::MAX, f32::min), ndc.iter().map(|p| p.y).fold(f32::MAX, f32::min), ndc.iter().map(|p| p.x).fold(f32::MIN, f32::max), ndc.iter().map(|p| p.y).fold(f32::MIN, f32::max)];
    Some(Glass { to_uv, inv_dist, size: [width, height], gravity: [down.dot(u_axis), down.dot(v_axis)], ndc_box })
}
