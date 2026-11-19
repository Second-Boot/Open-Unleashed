//! The scene's environment types: the covered boxes (E9 ENH-TUNPOOL), the rain (E26) and the water (E17). Re-exported from the crate root.

use glam::{Mat4, Vec3};

/// ENHANCEMENT E9 (ENH-TUNPOOL): how far under a covered slice's road its box reaches (m; the shader's `cover_at` has the same)
pub const COVER_FLOOR: f32 = -2.0;
/// ENHANCEMENT E9 (FOREST-E9PERF): the least determinant of a covered box's axes for its corners to be solved (else it is kept)
const COVER_MIN_DET: f32 = 1e-6;

/// ENHANCEMENT E9 (ENH-TUNPOOL): one road slice's covered volume: a box round the slice (its own axes), `half_len` along the
/// road (`half_len`: the tested extent, a little more than the slice's own share so the boxes of a bend overlap), `half_wid`
/// across it, from just under the road to `ceil` over it. `w_back` / `w_front` are the cover weights at the faces of the slice's
/// own share of the road (`half_nom` either side of the centre), NOT clamped: above 1 inside a covered run, ramping through 0..1
/// over the first metres of a run's ends, so the portal has no seam; the shader interpolates (and extrapolates) them along the
/// box and clamps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverBox {
    pub centre: Vec3,
    pub forward: Vec3,
    pub lateral: Vec3,
    pub up: Vec3,
    pub half_len: f32,
    pub half_nom: f32,
    pub half_wid: f32,
    pub ceil: f32,
    pub w_back: f32,
    pub w_front: f32,
}

impl CoverBox {
    /// ENHANCEMENT E9 (FOREST-E9PERF): the eight corners of the volume `weight` tests (the parallelepiped of its three tests,
    /// `|r . forward| <= half_len`, `|r . lateral| <= half_wid`, `COVER_FLOOR <= r . up <= ceil`, solved for `r`), or `None` when
    /// the axes have no inverse.
    pub fn corners(&self) -> Option<[Vec3; 8]> {
        let m = glam::Mat3::from_cols(self.forward, self.lateral, self.up).transpose();
        if m.determinant().abs() < COVER_MIN_DET {
            return None;
        }
        let inv = m.inverse();
        Some(std::array::from_fn(|c| {
            let b = Vec3::new(
                if c & 1 == 0 { -self.half_len } else { self.half_len },
                if c & 2 == 0 { -self.half_wid } else { self.half_wid },
                if c & 4 == 0 { COVER_FLOOR } else { self.ceil },
            );
            self.centre + inv * b
        }))
    }

    /// ENHANCEMENT E9 (FOREST-E9PERF): whether any point this box covers (`weight` above 0) can lie within `slack` metres of the
    /// view volume of `vp` (its four sides and its near plane; `0 <= z`, `-w <= x, y <= w`). False only when the box's weight is
    /// 0 everywhere or its whole volume ([`Self::corners`]) lies farther than `slack` outside one of those planes, so no point the
    /// view shades can fall in it (`slack` holds a particle's centre, which E9 reads for the whole sprite, off the screen's edge).
    /// A degenerate box is kept.
    pub fn may_cover_view(&self, vp: Mat4, slack: f32) -> bool {
        // the weight at the box's two ends (it is linear along `l`, then clamped: 0 at both = 0 everywhere; a NaN keeps the box)
        let at = |l: f32| self.w_back + (self.w_front - self.w_back) * ((l + self.half_nom) / (2.0 * self.half_nom));
        if at(-self.half_len) <= 0.0 && at(self.half_len) <= 0.0 {
            return false;
        }
        let Some(corners) = self.corners() else { return true };
        // the view volume's planes from the rows of `vp` (w + x, w - x, w + y, w - y, z), as world distances (inside > 0)
        let (rx, ry, rz, rw) = (vp.row(0), vp.row(1), vp.row(2), vp.row(3));
        let planes = [rw + rx, rw - rx, rw + ry, rw - ry, rz].map(|p| p / p.truncate().length().max(f32::MIN_POSITIVE));
        !planes.iter().any(|p| corners.iter().all(|c| p.dot(c.extend(1.0)) < -slack))
    }

    /// How covered `p` is by this box, 0..1 (0 outside it): the shader's `cover_at` for one box.
    pub fn weight(&self, p: Vec3) -> f32 {
        let r = p - self.centre;
        let l = r.dot(self.forward);
        if l.abs() > self.half_len || r.dot(self.lateral).abs() > self.half_wid {
            return 0.0;
        }
        let h = r.dot(self.up);
        if !(COVER_FLOOR..=self.ceil).contains(&h) {
            return 0.0;
        }
        (self.w_back + (self.w_front - self.w_back) * ((l + self.half_nom) / (2.0 * self.half_nom))).clamp(0.0, 1.0)
    }
}

/// ENHANCEMENT E9 (SHADOW-REAL): how covered `p` is by the track's covered boxes, 0..1 (the highest of the boxes'; the shader's
/// `cover_at`, over every box rather than the nearest 64).
pub fn cover_weight(boxes: &[CoverBox], p: Vec3) -> f32 {
    let mut w = 0.0f32;
    for b in boxes {
        w = w.max(b.weight(p));
        if w >= 1.0 {
            break;
        }
    }
    w
}

/// ENHANCEMENT E26: what the wet road reflects and the lights it shows streaks of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RainEnv {
    /// The sky straight up (the sky pass's clear colour) and at the horizon.
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    /// The wetness at the camera (0..1), drives the splashes' density; the road parts carry their own ([`DrawItem::wet`]).
    pub camera_wet: f32,
    /// Seconds on the race clock (the splash ripples' phase; frozen while the race is paused).
    pub time: f32,
    /// Lights near the camera that reflect in the road as vertical streaks (at most [`RAIN_LAMPS`]): position,
    /// linear colour x intensity and (AUTOBAHN-LAMPS) the light's reach in metres, the distance from the lamp at which
    /// its streak is down to half (`1 / (1 + d² / reach²)`); 0 = [`RAIN_LAMP_REACH`], the street lamps' reach.
    pub lamps: [(Vec3, [f32; 3], f32); RAIN_LAMPS],
    /// How many of `lamps` are used.
    pub n_lamps: usize,
    /// The nearest wetness edges (where the original's field 5 steps between slices), at most [`RAIN_EDGES`]: the road
    /// parts carry one value each, so near an edge the shader takes the wetness from these instead, ramped over the edge's width.
    pub edges: [RainEdge; RAIN_EDGES],
    pub n_edges: usize,
}

/// ENHANCEMENT E26: the most lights the road reflects per frame.
pub const RAIN_LAMPS: usize = 12;

/// ENHANCEMENT E26: the reach (m) of a light whose [`RainEnv::lamps`] entry gives none: the street and tunnel lamps' streak
/// falls to half 63 m from the lamp (`d² = 4000`, the first version's constant for every light).
pub const RAIN_LAMP_REACH: f32 = 63.245_55;

/// ENHANCEMENT E26: the most wetness edges the shader knows per frame.
pub const RAIN_EDGES: usize = 8;

/// ENHANCEMENT E26: a step of the wetness along the track: the plane through `pos` with the normal `dir` (toward the
/// higher slices), the wetness before it (`before`) and after it (`after`), ramped over `half` metres each side.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RainEdge {
    pub pos: Vec3,
    pub dir: Vec3,
    pub before: f32,
    pub after: f32,
    pub half: f32,
}

/// ENHANCEMENT E17: what the water reflects when there is no environment map (E8 off), and the sun
/// that glints on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterEnv {
    /// The sky straight up (the sky pass's clear colour) and at the horizon (the scene's fog colour, else the same).
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    /// The unit vector from the ground to the sun (`SkyBin::sun_position`, the sun of E9 and the lens
    /// flare); zero = no sun (night, overcast tracks): no glint.
    pub sun: Vec3,
    /// Reflect the E8 environment map (taken at the car, so only right where the car is close to the water: the
    /// night harbour) instead of the sky colours alone (the sea seen from a road far above it).
    pub use_env_map: bool,
}

