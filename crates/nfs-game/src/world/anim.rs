//! The track's animated articles and props: the animation matrix (0x48A1E0's quaternion rows), the animated article / prop
//! records and the animated sound. Moved out of `world.rs` as it was.

use super::*;

/// The default `secondsPerFrame`/loop rule `WAnimBundle::Load` 0x4777F0 gives a `WAnimBundle`
/// article's `AnimPlayer` (`0x471490`, vtable `0x5B460C`) when the article has no
/// `animdefs.txt SECTION:ANIM` piece of its own — the literal `0x3DCCCCCD` and the global constant
/// at `[0x5B45A4]` (`= 1`, its bit 0 the loop flag `AnimPlayer::sample` 0x470E90 tests) passed at
/// 0x4779DF..0x4779F7. CONFIRMED by direct disassembly, `COVERAGE-2026-09-26.md` §14.42 — see
/// [`TrackWorld::upload`]'s own note at the construction sites below for the full address trail
/// and the correction of an earlier pass's `SmackAnim` misidentification.
pub(super) const WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME: f32 = 0.1;

/// See [`WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME`].
pub(super) const WANIM_BUNDLE_DEFAULT_LOOPING: bool = true;

/// An `Anqt` track's matrix at `seconds`: `AnimPlayer::sample` 0x470E90's frame pair and blend
/// (0x470F70: the quaternion **lerped** component by component by 0x48A150 — no sign fix, no
/// renormalisation, so the blend of two distant unit quaternions is shorter than 1 — the position
/// and scale lerped), the rotation matrix of that quaternion by 0x48A1E0 (its formula, which
/// assumes a unit quaternion and is applied to whatever the lerp gave), each of the three rows
/// scaled by the frame's scale component (0x5329A0 — the local axes, i.e. `T · R · S` here) and
/// the position in the fourth row (0x539CD0). (AI 2026-09-30: this used a normalised
/// slerp before; the two differ where consecutive frames turn far — the rotor blades, the train.)
pub fn anim_matrix(track: &nfs_formats::crp::AnimTrack, seconds: f32, seconds_per_frame: f32, looping: bool) -> Mat4 {
    let (i, j, t) = track.sample(seconds, seconds_per_frame, looping);
    let (f0, f1) = (&track.frames[i], &track.frames[j]);
    let q: [f32; 4] = std::array::from_fn(|k| (f1.rotation[k] - f0.rotation[k]) * t + f0.rotation[k]);
    let pos = Vec3::from(f0.position).lerp(Vec3::from(f1.position), t);
    let scale = Vec3::from(f0.scale).lerp(Vec3::from(f1.scale), t);
    let scale = if scale.min_element() > 1e-4 { scale } else { Vec3::ONE };
    let rows = quat_rows_0x48a1e0(q);
    let m = glam::Mat3::from_cols(rows[0] * scale.x, rows[1] * scale.y, rows[2] * scale.z);
    Mat4::from_translation(pos) * Mat4::from_mat3(m)
}

/// 0x48A1E0: the three rows of the 3×3 matrix of `q = (x, y, z, w)` — the images of the local
/// axes — from its own `2·` products, without normalising `q`.
pub(super) fn quat_rows_0x48a1e0(q: [f32; 4]) -> [Vec3; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let (x2, y2, z2) = (x + x, y + y, z + z);
    let xw = x2 * w;
    [
        Vec3::new(1.0 - y2 * y - z2 * z, y2 * x + z2 * w, z2 * x - y2 * w),
        Vec3::new(y2 * x - z2 * w, 1.0 - x2 * x - z2 * z, z2 * y + xw),
        Vec3::new(z2 * x + y2 * w, z2 * y - xw, 1.0 - x2 * x - y2 * y),
    ]
}

/// One `WAnimBundle` article the port moves ([`TrackWorld::animate_articles`]).
pub(super) struct AnimArticle {
    /// its `.crp` article index (an entry of [`TrackWorld::cull_articles`])
    pub(super) article: usize,
    pub(super) track: nfs_formats::crp::AnimTrack,
    /// `Base`'s sphere in the article's own frame (0x477DB0's `Base+0x2C…`, `+0x3C`)
    pub(super) centre: Vec3,
    pub(super) radius: f32,
    /// its [`TrackWorld::track_lods`] entry, when it has draw items
    pub(super) gpu: Option<usize>,
    /// this frame's matrix (the flares of its `ef` parts go through it, 0x450380)
    pub(super) matrix: Mat4,
    /// the sphere centre the ref was last registered / compared with (record `+4…+0xC`)
    pub(super) registered: Vec3,
}

/// One prop playing its own looping animation.
pub(super) struct AnimatedProp {
    pub(super) item: usize,
    /// the transform the article is placed with; the sampled frame is applied inside it
    pub(super) base: Mat4,
    pub(super) track: nfs_formats::crp::AnimTrack,
    pub(super) seconds_per_frame: f32,
    pub(super) looping: bool,
}

/// The `sound_articles` object's own `Anqt` track and `SoundDef`, kept alongside `TrackWorld` so
/// its caller (the game's audio update) can sample the *same* path every tick and feed
/// `nfs_audio::ambience::Ambience::sync_animated` — `engine-audio.md` §7.4's `p1 = obj.EvalPos(tick)`,
/// `p2 = obj.EvalPos(tick + 0x20)`. `ambience_index` is `None` until the caller has registered it
/// with `Ambience::add_animated` (track load and ambience load are two separate steps in the port).
pub struct AnimatedSound {
    pub def: nfs_formats::animdefs::SoundDef,
    pub label: String,
    pub track: nfs_formats::crp::AnimTrack,
    /// CONFIRMED, [`WANIM_BUNDLE_DEFAULT_SECONDS_PER_FRAME`] for a `WAnimBundle` article without
    /// its own `animdefs.txt` piece — the *same clock* [`TrackWorld::animate`] draws this article
    /// with, so the visible motion and the ambient sound's doppler never drift apart.
    pub seconds_per_frame: f32,
    pub looping: bool,
    pub ambience_index: Option<usize>,
}

impl AnimatedSound {
    /// `p1 = EvalPos(tick)`, `p2 = EvalPos(tick + 0x20)` (§7.4) on this object's own path, in
    /// world units of ticks at the game's 64 Hz.
    pub fn eval_pair(&self, tick: u64) -> ([f32; 3], [f32; 3]) {
        let eval = |t: u64| {
            let (i, j, w) = self.track.sample(t as f32 / 64.0, self.seconds_per_frame, self.looping);
            let (f0, f1) = (&self.track.frames[i], &self.track.frames[j]);
            Vec3::from(f0.position).lerp(Vec3::from(f1.position), w)
        };
        (eval(tick).into(), eval(tick.wrapping_add(0x20)).into())
    }
}

#[cfg(test)]
mod anim_article_tests {
    use super::*;
    use nfs_formats::crp::{AnimFrame, AnimTrack};

    fn track() -> AnimTrack {
        let f = |x: f32, s: f32| AnimFrame { rotation: [0.0, 0.0, 0.0, 1.0], position: [x, 2.0 * x, 3.0], scale: [s; 3] };
        AnimTrack { frames: vec![f(10.0, 1.5), f(20.0, 1.5), f(30.0, 1.5), f(40.0, 1.5)] }
    }

    /// 0x470E90 / 0x470F70: at second 0 the frame-0 pose; half a frame on, the midpoint; the
    /// scale multiplies the rotation's rows (0x5329A0), the position is the fourth row (0x539CD0).
    #[test]
    fn matrix_is_frame_pose_blended_by_the_players_clock() {
        let t = track();
        let m0 = anim_matrix(&t, 0.0, 0.1, true);
        assert!((m0.w_axis.truncate() - Vec3::new(10.0, 20.0, 3.0)).length() < 1e-5);
        assert!((m0.x_axis.truncate() - Vec3::new(1.5, 0.0, 0.0)).length() < 1e-5);
        let m = anim_matrix(&t, 0.05, 0.1, true);
        assert!((m.w_axis.truncate() - Vec3::new(15.0, 30.0, 3.0)).length() < 1e-4);
        // a looping track wraps at `frames − 2` (0.2 s = frame 2 → frame 0)
        let w = anim_matrix(&t, 0.2, 0.1, true);
        assert!((w.w_axis.truncate() - Vec3::new(10.0, 20.0, 3.0)).length() < 1e-5);
    }

    /// 0x48A150 / 0x48A1E0: the quaternion is lerped, not slerped and not renormalised — a half-way
    /// blend of two frames 90° apart about Z is `(0, 0, sin 45°/2, (1 + cos 45°)/2)`-ish and its
    /// matrix is shorter than unit, unlike the slerp's; for a unit quaternion the formula is the
    /// ordinary rotation.
    #[test]
    fn rotation_is_the_lerped_quaternion_through_0x48a1e0() {
        let q = glam::Quat::from_rotation_y(0.7) * glam::Quat::from_rotation_x(0.3);
        let r = quat_rows_0x48a1e0([q.x, q.y, q.z, q.w]);
        let g = glam::Mat3::from_quat(q);
        assert!((r[0] - g.x_axis).length() < 1e-5 && (r[1] - g.y_axis).length() < 1e-5 && (r[2] - g.z_axis).length() < 1e-5);
        let z90 = glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let f = |q: glam::Quat| AnimFrame { rotation: [q.x, q.y, q.z, q.w], position: [0.0; 3], scale: [1.0; 3] };
        let t = AnimTrack { frames: vec![f(glam::Quat::IDENTITY), f(z90), f(z90), f(z90)] };
        let m = anim_matrix(&t, 0.05, 0.1, true);
        // lerp: q = (0, 0, 0.35355, 0.85355) → x row length < 1 (the slerp's would be 1)
        assert!(m.x_axis.truncate().length() < 0.99, "{}", m.x_axis.truncate().length());
    }
}
