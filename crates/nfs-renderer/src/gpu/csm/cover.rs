//! ENHANCEMENT E9 (ENH-TUNPOOL): the covered boxes in the sun-shadow uniform ([`write`]).

use super::*;

/// The most covered boxes the shader knows (the nearest to the camera go in).
pub(in crate::gpu) const MAX_COVER: usize = 64;
/// A covered box's vec4s in the uniform, and its bytes.
pub(super) const COVER_VEC4S: usize = 5;
pub(super) const COVER_BYTES: usize = COVER_VEC4S * std::mem::size_of::<[f32; 4]>();
/// Covered boxes farther than this from the camera are left out (m); past it a tunnel's interior is shaded as open ground, which
/// E9's own far fade has reached (the last cascade ends at 400 m / 520 m).
pub(super) const COVER_REACH: f32 = 420.0;
/// PERF (FOREST-E9PERF): how far outside the view volume (m) a covered box may stand and still be handed to the shader: a
/// particle's light reads E9 at the particle's centre (`particle_fx`), which may be off the screen while its sprite is on it.
pub(super) const COVER_VIEW_SLACK: f32 = 30.0;
/// PERF (FOREST-E9PERF): how much wider than its boxes' corners (m) the world box round the covered boxes is: room for the
/// rounding of the corners and of the shader's own tests.
pub(super) const COVER_BOUND_PAD: f32 = 0.05;


/// The covered boxes the shader knows this frame (ENHANCEMENT E9, ENH-TUNPOOL): the nearest to the camera within
/// [`COVER_REACH`], at most [`MAX_COVER`]; a fragment inside one takes no sun-shadow darkening (the shader's `cover_at`),
/// whatever the camera is doing: a tunnel's pools look the same from outside and inside.
/// `near`: the pass's kept list (refilled).
pub(super) fn write(u: &mut CsmUniform, cover: &[crate::CoverBox], eye: Vec3, vp: Mat4, near: &mut Vec<(f32, usize)>) {
    near.clear();
    near.extend(cover.iter().enumerate().map(|(i, b)| (b.centre.distance_squared(eye), i)).filter(|(d, _)| *d < COVER_REACH * COVER_REACH));
    if near.len() > MAX_COVER {
        near.select_nth_unstable_by(MAX_COVER - 1, |a, b| a.0.total_cmp(&b.0));
        near.truncate(MAX_COVER);
    }
    // PERF (FOREST-E9PERF, picture identical): of those, only the boxes a fragment of this view can fall in (`cover_at` loops
    // over every box for every fragment). Culled after the nearest 64 are picked, so the boxes that remain are exactly the ones
    // that could change a pixel before.
    near.retain(|&(_, i)| cover[i].may_cover_view(vp, COVER_VIEW_SLACK));
    u.misc[0] = near.len() as f32;
    // PERF (FOREST-E9PERF, picture identical): the world box round them all (a degenerate box: round everything), widened a
    // little against rounding; a point outside it skips the loop
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &(_, i) in near.iter() {
        match cover[i].corners() {
            Some(cs) => cs.iter().for_each(|&c| (lo, hi) = (lo.min(c), hi.max(c))),
            None => (lo, hi) = (Vec3::splat(f32::MIN), Vec3::splat(f32::MAX)),
        }
    }
    let pad = Vec3::splat(COVER_BOUND_PAD);
    u.cover_lo = (lo - pad).extend(0.0).to_array();
    u.cover_hi = (hi + pad).extend(0.0).to_array();
    for (box_, b) in u.cover.as_chunks_mut::<COVER_VEC4S>().0.iter_mut().zip(near.iter().map(|&(_, i)| &cover[i])) {
        box_.copy_from_slice(&[
            [b.centre.x, b.centre.y, b.centre.z, b.half_len],
            [b.forward.x, b.forward.y, b.forward.z, b.half_wid],
            [b.lateral.x, b.lateral.y, b.lateral.z, b.w_back],
            [b.up.x, b.up.y, b.up.z, b.w_front],
            [b.ceil, b.half_nom, 0.0, 0.0],
        ]);
    }
}
