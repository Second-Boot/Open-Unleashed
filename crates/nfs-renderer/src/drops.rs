//! ENHANCEMENT E40 — raindrops on the windscreen of the cockpit (driver) view. **Not in the original, render-only.**
//!
//! The original's drops (`SPLT`, 0x4577A0 …) are two faint additive sprites on the glass plane and do not move; they
//! stay as they are. This adds refractive drops over the float scene (E7): the app hands a [`Drops`] (the glass quad
//! in the world, the rain's wetness, the car's speed, the render clock) with the scene of the driver view while it
//! rains and the car is not under cover; the renderer keeps a small CPU simulation of the water on the glass
//! ([`Sim`]), turns it into capsule instances (a drop is a disc, a running drop a teardrop, a trail a thin faded
//! capsule) binned in a grid, and one full-screen pass over the resolved float image looks each glass pixel up in
//! that grid and, inside a drop, samples the scene through the drop's lens (an inverted, wide-angle view of the
//! point behind it, a little blur, a dark rim, a small highlight).
//!
//! * **The windscreen region** is the original's own: the `Wndo` corners the splat plane is built from
//!   (`weather::SplatPlane`), projected with the frame's matrices. The screen position of a pixel on that plane (and
//!   the glass's distance there) comes from a homography of the quad's corners, so a drop sits at a fixed place on the
//!   glass whatever the camera does. A pixel counts only when the scene's depth there is at least the glass's
//!   distance (minus 3 cm): the dashboard, the pillars, the mirror, the wipers' rest pose stay clean.
//! * **The water** (all in metres on the glass): beads are born at a rate that grows with the wetness and the speed
//!   (a car drives into the rain); two beads that touch merge by volume; a drop that is heavy enough runs, down the
//!   glass by its weight (the in-plane component of gravity, so a rolled car runs sideways), UP it by the airflow
//!   (an aerodynamic force growing with the speed squared, the faster the more the small ones go), a little
//!   outward from the glass's middle; it leaves a trail that fades, loses water as it runs and merges with what it
//!   meets. Everything below the contact-line force sticks. **Wipers:** the original's are only in the model (not
//!   animated), so a soft periodic wipe clears the glass: two blades that sweep once over 0.9 s, every 3.5-9 s (the
//!   heavier the rain the oftener), at once under cover, and soon after the rain stops.
//! * **Deterministic from the render clock, never the sim RNG:** the state is stepped by the scene's clock in fixed
//!   1/60 s steps from a private xorshift generator with a constant seed; the first frame (or a jump of the clock)
//!   starts from a pre-roll of 2.8 wipe periods at the current rain and speed so the glass is never empty at the first look.

use crate::gfx::{self, DepthBinding, DepthSrc, Fullscreen, PostCtx, UniformBuf, Variants};
use crate::wgsl;
use crate::Camera;
use crate::gpu::HDR_FORMAT;
use glam::{Mat3, Vec2, Vec3};

/// What the app hands the renderer for the driver view of a rainy track (read only with E40 and E7 on; `None` = none).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drops {
    /// The windscreen's four corners in the world, in any order (the splat plane's `Wndo` points through the car's pose).
    pub corners: [Vec3; 4],
    /// The rain's wetness at the car, 0..1 (the sky `.bin`'s track B field 5), already faded by the cover.
    pub wet: f32,
    /// How far inside a covered stretch the car is, 0..1 (E24's held, ramped value).
    pub cover: f32,
    /// The car's speed, m/s.
    pub speed: f32,
    /// The sim clock, seconds (a paused game's stands, a replay's is the replay's).
    pub time: f64,
    /// The world's up.
    pub up: Vec3,
}

/// Most drops / trail pieces the water keeps.
const MAX_DROPS: usize = 560;
/// The wobble phase range of a new drop (one turn, as tuned; kept as written so the drop pattern stays the same).
#[allow(clippy::approx_constant)]
const WOBBLE_TURN: f32 = 6.283;
const MAX_TRAILS: usize = 640;
/// The instance texture: two texels per instance.
const INST_W: u32 = 512;
const INST_H: u32 = 8;
pub(crate) const MAX_INSTANCES: usize = (INST_W * INST_H / 2) as usize;
/// The grid over the glass: cells and the instance slots of a cell.
const GRID_COLS: usize = 48;
const GRID_ROWS: usize = 24;
const CELL_SLOTS: usize = 16;
const NO_INSTANCE: u32 = 0xFFFF_FFFF;
/// The data textures' texel sizes: `Rgba32Float`, `R32Uint`.
const INST_TEXEL_BYTES: u32 = 16;
const GRID_TEXEL_BYTES: u32 = 4;
/// The refraction's reach: the sample point lies this many radii on the other side of the drop's centre.
const REFRACT: f32 = 2.4;
/// The step of the simulation, seconds.
const STEP: f32 = 1.0 / 60.0;
/// The step of the pre-roll of a fresh glass (coarser than the live step).
const PRE_STEP: f32 = 1.0 / 20.0;

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
const MIN_GLASS_DEPTH: f32 = 0.05;
const MIN_GLASS_SIDE: f32 = 0.05;
/// The glass box's padding on the screen (px).
const BOX_PAD: f32 = 3.0;
/// The scratch copy's slack (px) so the box's wobble (the camera shake) does not reallocate.
const SCRATCH_SLACK: u32 = 64;

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

// ---------------------------------------------------------------------------------------------------------------
// the water

/// Standard gravity and the two constants that make the contact line hold a bead: a drop runs when
/// `|g_glass G r^2 + wind c_q v^2 r| > C` (weight ~ r^3 over r, the airflow's push ~ r^2 over r, the pull of the contact
/// line ~ constant), so a 2.5 mm drop runs down a 33 degree windscreen at rest and a 1 mm one is blown up at 80 km/h.
const G: f32 = 9.81;
const CONTACT: f32 = 5.0e-5;
const AERO: f32 = 8.0e-5;
/// Run speed per unit of `|F| / C - 1` (m/s) and its cap.
const RUN_GAIN: f32 = 0.04;
const RUN_MAX: f32 = 0.45;
/// The volume a running drop leaves per metre (m^3).
const LOSS_PER_M: f32 = 5.0e-8;
/// A trail piece lives this long (s).
const TRAIL_LIFE: f32 = 2.4;
/// Length of the piece a running drop lays down (m).
const TRAIL_STEP: f32 = 0.02;
/// Beads per square metre of glass per second at full wetness and at rest.
const SPAWN: f32 = 320.0;
/// Seconds between two wipes at this wetness: 7 s in a drizzle, 3 s in a downpour.
fn wipe_period(wet: f32) -> f32 {
    WIPE_PERIOD_DRIZZLE - WIPE_PERIOD_SPAN * wet.clamp(0.0, 1.0)
}
const WIPE_PERIOD_DRIZZLE: f32 = 7.0;
const WIPE_PERIOD_SPAN: f32 = 4.0;

/// The wipe: blades' length over the glass's width, duration, the pivots' x (over the width) and y (m, below the glass).
const WIPE_TIME: f32 = 0.9;
const WIPE_REACH: f32 = 0.7;
const WIPE_PIVOTS: [f32; 2] = [0.30, 0.72];
const WIPE_BELOW: f32 = 0.05;

/// The water's tuned numbers (an enhancement's look, tuned by eye).
/// Births: the rain's wetness below which nothing is born; the speed factor `SPEED_K_BASE + SPEED_K_GAIN * min(speed /
/// SPEED_K_REF, SPEED_K_CAP)`; the wetness exponent; a bead's radius `BEAD_R_MIN + BEAD_R_SPAN * a^2` (m), at rest (below
/// FAT_SPEED m/s) with the chance FAT_CHANCE a fat one `FAT_R_MIN + FAT_R_SPAN * c`.
const DRY: f32 = 0.02;
const SPEED_K_BASE: f32 = 0.6;
const SPEED_K_GAIN: f32 = 0.8;
const SPEED_K_REF: f32 = 25.0;
const SPEED_K_CAP: f32 = 1.6;
const WET_EXPONENT: f32 = 1.2;
const BEAD_R_MIN: f32 = 0.0015;
const BEAD_R_SPAN: f32 = 0.0032;
const FAT_SPEED: f32 = 8.0;
const FAT_CHANCE: f32 = 0.12;
const FAT_R_MIN: f32 = 0.0045;
const FAT_R_SPAN: f32 = 0.0025;
/// The airflow's outward share from the glass's middle.
const SIDE_PUSH: f32 = 0.7;
/// The meander of a running drop: its rate `WOB_RATE + WOB_RATE_R * r * 100` (rad/s, r in m) and its swing (rad).
const WOB_RATE: f32 = 0.7;
const WOB_RATE_R: f32 = 3.0;
const WOB_SWING: f32 = 0.18;
/// The smallest a running drop shrinks to, the trail's radius (a share of the drop's, clamped) (m).
const MIN_R: f32 = 0.0004;
const TRAIL_R_SHARE: f32 = 0.45;
const TRAIL_R_MAX: f32 = 0.0011;
/// Merging: the sweep's reach along x (m), the touch distance as a share of the radii, the biggest drop (m).
const MERGE_REACH: f32 = 0.02;
const MERGE_TOUCH: f32 = 0.85;
const MAX_R: f32 = 0.009;
/// A drop this far (m) off the glass is gone.
const OFF_GLASS: f32 = 0.01;
/// The wipe: the covered value above which the glass is cleared at once, below which it may be again; the seconds after the
/// rain stopped; the blade's dead zone at the pivot (m) and its angular margin (rad).
const COVER_WIPE: f32 = 0.5;
const COVER_REARM: f32 = 0.2;
const DRY_WIPE_AFTER: f32 = 1.5;
const BLADE_HUB: f32 = 0.02;
const BLADE_MARGIN: f32 = 0.03;
/// A fresh glass: the pre-roll in wipe periods (at least `PRE_ROLL_MIN_PERIOD` s each); the clock jump that starts afresh (s);
/// the first wipe's time before the start (s).
const PRE_ROLL_PERIODS: f32 = 2.8;
const PRE_ROLL_MIN_PERIOD: f32 = 1.5;
const CLOCK_JUMP: f64 = 3.0;
const FIRST_WIPE: f32 = -2.5;
/// The instances: the grid cell margin round a capsule (m); a trail's alpha and its end's radius share; a running drop's
/// tail length (radii, capped, m) and its end's radius share.
const CELL_MARGIN: f32 = 0.001;
const TRAIL_ALPHA: f32 = 0.45;
const TRAIL_TAPER: f32 = 0.8;
const TAIL_RADII: f32 = 2.2;
const TAIL_MAX: f32 = 0.012;
const TAIL_TAPER: f32 = 0.55;

#[derive(Clone, Copy, Debug)]
struct Drop {
    pos: Vec2,
    /// radius (m)
    r: f32,
    /// the direction it runs (unit) and whether it does
    dir: Vec2,
    run: bool,
    /// the last trail piece's end
    mark: Vec2,
    /// a slow sideways meander (rad), drifting
    wob: f32,
}

#[derive(Clone, Copy, Debug)]
struct Trail {
    a: Vec2,
    b: Vec2,
    r: f32,
    born: f32,
}

/// What a step needs of the frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Params {
    pub wet: f32,
    pub cover: f32,
    pub speed: f32,
    pub gravity: [f32; 2],
    pub size: [f32; 2],
}

/// The water on the glass.
#[derive(Clone, Debug)]
pub(crate) struct Sim {
    rng: u32,
    /// the sim's own clock (s since it started) and the scene clock it follows
    t: f32,
    last_scene: f64,
    drops: Vec<Drop>,
    trails: std::collections::VecDeque<Trail>,
    spawn_acc: f32,
    last_wipe: f32,
    /// seconds into the running wipe
    wipe: Option<f32>,
    cover_cleared: bool,
    /// the part of a step the scene's clock has advanced but no step has run for
    carry: f32,
    /// the steps run so far: the instances change only when this does
    steps: u64,
    /// the merge's scratch: the drops in x order, which of them merged away (kept between steps)
    order: Vec<usize>,
    dead: Vec<bool>,
    /// the build's scratch: the instances per grid cell
    fill: Vec<u8>,
}

impl Sim {
    /// A fresh glass at the scene's clock, rolled forward by the pre-roll at the current conditions.
    pub fn new(scene_time: f64, p: &Params) -> Self {
        let mut s = Self { rng: 0x9E37_79B9, t: 0.0, last_scene: scene_time, drops: Vec::new(), trails: Default::default(), spawn_acc: 0.0, last_wipe: FIRST_WIPE, wipe: None, cover_cleared: false, carry: 0.0, steps: 0, order: Vec::new(), dead: Vec::new(), fill: Vec::new() };
        // two wipe cycles and most of a third: the glass is first seen well wetted, not just wiped
        let mut left = PRE_ROLL_PERIODS * wipe_period(p.wet * (1.0 - p.cover)).max(PRE_ROLL_MIN_PERIOD);
        while left > 0.0 {
            s.step(PRE_STEP, p);
            left -= PRE_STEP;
        }
        s
    }

    /// Follow the scene's clock: fixed steps; a clock that went back or jumped over 3 s starts the glass afresh.
    pub fn advance(&mut self, scene_time: f64, p: &Params) -> bool {
        let dt = scene_time - self.last_scene;
        if !(0.0..=CLOCK_JUMP).contains(&dt) {
            *self = Self::new(scene_time, p);
            return true;
        }
        self.last_scene = scene_time;
        let mut left = dt as f32 + self.carry;
        while left >= STEP {
            self.step(STEP, p);
            left -= STEP;
        }
        self.carry = left;
        false
    }

    /// Whether anything is on the glass (the pass is skipped on a clean glass with no rain).
    pub fn busy(&self, p: &Params) -> bool {
        !self.drops.is_empty() || !self.trails.is_empty() || p.wet * (1.0 - p.cover) > DRY
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// The force that works on a drop of radius `r` at `pos`, as (along u, along v).
    fn force(&self, d: &Drop, p: &Params, q: f32) -> Vec2 {
        let weight = G * d.r * d.r;
        let push = q * d.r;
        // the airflow runs up the glass and, from the middle, a little outward
        let side = (d.pos.x / p.size[0] - 0.5) * SIDE_PUSH;
        Vec2::new(p.gravity[0] * weight + side * push, p.gravity[1] * weight + push)
    }

    pub fn step(&mut self, dt: f32, p: &Params) {
        self.steps += 1;
        self.t += dt;
        let t = self.t;
        let (w, h) = (p.size[0], p.size[1]);
        let wet = (p.wet * (1.0 - p.cover)).clamp(0.0, 1.0);
        let q = AERO * p.speed * p.speed;
        // ---- birth: a car drives into the rain, so more and smaller drops with speed; at rest a few fat ones
        if wet > DRY {
            let speed_k = SPEED_K_BASE + SPEED_K_GAIN * (p.speed / SPEED_K_REF).min(SPEED_K_CAP);
            self.spawn_acc += SPAWN * wet.powf(WET_EXPONENT) * speed_k * w * h * dt;
            while self.spawn_acc >= 1.0 {
                self.spawn_acc -= 1.0;
                let (a, b, c, e, y) = (self.rand(), self.rand(), self.rand(), self.rand(), self.rand());
                let mut r = BEAD_R_MIN + BEAD_R_SPAN * a * a;
                if p.speed < FAT_SPEED && e < FAT_CHANCE {
                    r = FAT_R_MIN + FAT_R_SPAN * c;
                }
                if self.drops.len() < MAX_DROPS {
                    self.drops.push(Drop { pos: Vec2::new(b * w, y * h), r, dir: Vec2::NEG_Y, run: false, mark: Vec2::ZERO, wob: c * WOBBLE_TURN });
                }
            }
        }
        // ---- motion
        for i in 0..self.drops.len() {
            let d = self.drops[i];
            let f = self.force(&d, p, q);
            let fm = f.length();
            let mut d = d;
            if fm > CONTACT {
                let dir = f / fm;
                let speed = (RUN_GAIN * (fm / CONTACT - 1.0)).min(RUN_MAX);
                // a slow meander so the runs are not ruler-straight
                d.wob += dt * (WOB_RATE + WOB_RATE_R * d.r * 100.0);
                let wob = d.wob.sin() * WOB_SWING;
                let dir = Vec2::new(dir.x * wob.cos() - dir.y * wob.sin(), dir.x * wob.sin() + dir.y * wob.cos());
                if !d.run {
                    d.run = true;
                    d.mark = d.pos;
                }
                d.dir = dir;
                let step = speed * dt;
                d.pos += dir * step;
                // the volume left behind
                let vol = (d.r * d.r * d.r - LOSS_PER_M * step).max(1e-12);
                d.r = vol.cbrt().max(MIN_R);
                if (d.pos - d.mark).length() >= TRAIL_STEP {
                    if self.trails.len() >= MAX_TRAILS {
                        self.trails.pop_front();
                    }
                    self.trails.push_back(Trail { a: d.mark, b: d.pos, r: (TRAIL_R_SHARE * d.r).clamp(MIN_R, TRAIL_R_MAX), born: t });
                    d.mark = d.pos;
                }
            } else {
                d.run = false;
            }
            self.drops[i] = d;
        }
        // ---- merging: touching drops become one by volume; the running one keeps going. Swept along x so it is not O(n^2)
        let n = self.drops.len();
        let (mut order, mut dead) = (std::mem::take(&mut self.order), std::mem::take(&mut self.dead));
        order.clear();
        order.extend(0..n);
        order.sort_by(|&a, &b| self.drops[a].pos.x.total_cmp(&self.drops[b].pos.x));
        dead.clear();
        dead.resize(n, false);
        for oi in 0..n {
            let i = order[oi];
            if dead[i] {
                continue;
            }
            for &j in &order[oi + 1..] {
                if self.drops[j].pos.x - self.drops[i].pos.x > MERGE_REACH {
                    break;
                }
                if dead[j] {
                    continue;
                }
                let (a, b) = (self.drops[i], self.drops[j]);
                if (a.pos - b.pos).length() < (a.r + b.r) * MERGE_TOUCH {
                    let (va, vb) = (a.r * a.r * a.r, b.r * b.r * b.r);
                    let keep = if a.run || !b.run { a } else { b };
                    let mut m = keep;
                    m.r = (va + vb).cbrt().min(MAX_R);
                    m.pos = (a.pos * va + b.pos * vb) / (va + vb);
                    if a.run != b.run {
                        // the bead the runner touched joins its run
                        m.run = true;
                        m.dir = keep.dir;
                    }
                    self.drops[i] = m;
                    dead[j] = true;
                }
            }
        }
        let mut k = 0;
        self.drops.retain(|_| {
            k += 1;
            !dead[k - 1]
        });
        (self.order, self.dead) = (order, dead);
        // ---- gone off the glass
        self.drops.retain(|d| d.pos.x > -OFF_GLASS && d.pos.x < w + OFF_GLASS && d.pos.y > -OFF_GLASS && d.pos.y < h + OFF_GLASS);
        // ---- trails fade
        self.trails.retain(|tr| t - tr.born < TRAIL_LIFE);
        // ---- the wipe: soon after the rain stops, at once under cover, otherwise by the rain's weight
        let period = wipe_period(wet);
        let want = if p.cover > COVER_WIPE {
            !self.cover_cleared && (!self.drops.is_empty() || !self.trails.is_empty())
        } else if wet < DRY {
            t - self.last_wipe > DRY_WIPE_AFTER && (!self.drops.is_empty() || !self.trails.is_empty())
        } else {
            t - self.last_wipe > period
        };
        if p.cover > COVER_WIPE {
            self.cover_cleared = true;
        } else if p.cover < COVER_REARM {
            self.cover_cleared = false;
        }
        if self.wipe.is_none() && want {
            self.wipe = Some(0.0);
            self.last_wipe = t;
        }
        if let Some(e) = self.wipe {
            let ease = |x: f32| {
                let x = x.clamp(0.0, 1.0);
                x * x * (3.0 - 2.0 * x)
            };
            let (a0, a1) = (std::f32::consts::PI * (1.0 - ease(e / WIPE_TIME)), std::f32::consts::PI * (1.0 - ease((e + dt) / WIPE_TIME)));
            let reach = WIPE_REACH * w;
            let swept = |pt: Vec2| -> bool {
                WIPE_PIVOTS.iter().any(|&px| {
                    let rel = pt - Vec2::new(px * w, -WIPE_BELOW);
                    let l = rel.length();
                    // a hair of margin on both ends so the sweep leaves no seam between steps
                    l < reach && l > BLADE_HUB && {
                        let ang = rel.y.atan2(rel.x);
                        ang >= a1 - BLADE_MARGIN && ang <= a0 + BLADE_MARGIN
                    }
                })
            };
            self.drops.retain(|d| !swept(d.pos));
            self.trails.retain(|tr| !swept(tr.a) && !swept(tr.b));
            let e = e + dt;
            self.wipe = if e >= WIPE_TIME { None } else { Some(e) };
        }
    }

    /// The instances for the pass: `(data, grid)` with two RGBA32F texels per instance
    /// `[ax, ay, bx, by] [r_a, r_b, alpha, 0]` and the grid's slots (instance index or `NO_INSTANCE`).
    pub fn build(&mut self, size: [f32; 2], inst: &mut Vec<[f32; 4]>, grid: &mut Vec<u32>) -> usize {
        inst.clear();
        grid.clear();
        grid.resize(GRID_COLS * GRID_ROWS * CELL_SLOTS, NO_INSTANCE);
        let mut fill = std::mem::take(&mut self.fill);
        fill.clear();
        fill.resize(GRID_COLS * GRID_ROWS, 0);
        let t = self.t;
        let mut add = |inst: &mut Vec<[f32; 4]>, a: Vec2, b: Vec2, ra: f32, rb: f32, alpha: f32| {
            if inst.len() / 2 >= MAX_INSTANCES {
                return;
            }
            let id = (inst.len() / 2) as u32;
            inst.push([a.x, a.y, b.x, b.y]);
            inst.push([ra, rb, alpha, 0.0]);
            let rm = ra.max(rb) + CELL_MARGIN;
            let (lo, hi) = (a.min(b) - Vec2::splat(rm), a.max(b) + Vec2::splat(rm));
            let cell = |v: f32, n: usize, s: f32| ((v / s * n as f32).floor() as i32).clamp(0, n as i32 - 1) as usize;
            let (c0, c1) = (cell(lo.x, GRID_COLS, size[0]), cell(hi.x, GRID_COLS, size[0]));
            let (r0, r1) = (cell(lo.y, GRID_ROWS, size[1]), cell(hi.y, GRID_ROWS, size[1]));
            for r in r0..=r1 {
                for c in c0..=c1 {
                    let ci = r * GRID_COLS + c;
                    if (fill[ci] as usize) < CELL_SLOTS {
                        grid[ci * CELL_SLOTS + fill[ci] as usize] = id;
                        fill[ci] += 1;
                    }
                }
            }
        };
        for tr in &self.trails {
            let life = 1.0 - (t - tr.born) / TRAIL_LIFE;
            // a trail is a film, not a drop: faint, and it thins out
            add(inst, tr.a, tr.b, tr.r, tr.r * TRAIL_TAPER, TRAIL_ALPHA * life.clamp(0.0, 1.0).sqrt());
        }
        for d in &self.drops {
            if d.run {
                // a teardrop: the head, and a tail of the same water behind it
                let len = (TAIL_RADII * d.r).min(TAIL_MAX);
                add(inst, d.pos, d.pos - d.dir * len, d.r, d.r * TAIL_TAPER, 1.0);
            } else {
                add(inst, d.pos, d.pos, d.r, d.r, 1.0);
            }
        }
        self.fill = fill;
        inst.len() / 2
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the pass

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DropsUniform {
    /// NDC -> (u w, v w, w), three columns (the 4th lane unused)
    h0: [f32; 4],
    h1: [f32; 4],
    h2: [f32; 4],
    /// 1 / distance = a x + b y + c, the glass's width (m) in the 4th
    inv: [f32; 4],
    /// image width, height, glass height (m), strength
    a: [f32; 4],
    /// `far / (near - far)`, near, half the image height times the projection's `1 / tan(fov / 2)`, refraction reach
    b: [f32; 4],
    /// the sky's colour (the scene's clear colour): what the drops' highlights reflect
    sky: [f32; 4],
    /// the colour source's origin in image pixels and its size (the scratch copy of the glass's box, or the whole image)
    bx: [f32; 4],
    /// the part of it that holds the scene: width, height
    bz: [f32; 4],
}

/// The two ways the pass runs. `Full`: a new image the size of the frame (the scene was already put in an image of its own
/// by an earlier pass: the blur, the depth of field, FSR). `InPlace`: the scene's own image, a scratch copy of the glass's
/// box as the source, a draw over that box only (a pixel without a drop is discarded = keeps the scene).
const MODE_FULL: &str = r#"
fn keep(p: vec2<f32>) -> vec4<f32> { return base(p); }
"#;
const MODE_IN_PLACE: &str = r#"
fn keep(p: vec2<f32>) -> vec4<f32> { discard; }
"#;

const DROPS_WGSL: &str = r#"
struct U { h0: vec4<f32>, h1: vec4<f32>, h2: vec4<f32>, inv: vec4<f32>, a: vec4<f32>, b: vec4<f32>, sky: vec4<f32>, bx: vec4<f32>, bz: vec4<f32> };
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@group(2) @binding(0) var<uniform> u: U;
@group(2) @binding(1) var inst: texture_2d<f32>;
@group(2) @binding(2) var grid: texture_2d<u32>;
fn base(p: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t, s, (p - u.bx.xy) / u.bx.zw, 0.0);
}
//MODE
fn done(p: vec2<f32>, col: vec3<f32>, k: f32) -> vec4<f32> {
    let c = base(p);
    return vec4<f32>(mix(c.rgb, col, k), c.a);
}
fn lin(z: f32) -> f32 {
    return proj_dist(z, u.b.x, u.b.y);
}
//SIZES
// (the drops' tuned look, ENHANCEMENT E40; not the original's)
// a segment's least squared length (m^2); the depth a surface may sit in front of the glass before it hides the drops (m)
const MIN_SEG2: f32 = 1e-12;
const GLASS_SLACK: f32 = 0.03;
// the drop's height to its coverage, the border of the glass the drops thin out over (m), the least coverage drawn
const HEIGHT_COVER: f32 = 4.0;
const EDGE_FADE: f32 = 0.015;
const MIN_INSIDE: f32 = 0.002;
// the lens blur: its radius per drop radius (pixels) held to [BLUR_MIN, BLUR_MAX]; the brightest a tap counts (float scene)
const BLUR_PER_R: f32 = 0.22;
const BLUR_MIN: f32 = 0.5;
const BLUR_MAX: f32 = 2.5;
const TAP_MAX: f32 = 24.0;
// the pixel and its 6 taps
const N_TAPS: i32 = 6;
const TAP_WEIGHTS: f32 = 7.0;
// the little lens: the rim's darkening (its curve, depth), the middle's gain and cool tint
const RIM_POW: f32 = 1.6;
const RIM_DARK: f32 = 0.55;
const MID_GAIN: f32 = 1.12;
const MID_TINT: vec3<f32> = vec3<f32>(0.018, 0.024, 0.03);
// the sky over the car (up and to the left, as a direction on the drop), its gain and the scene light's share
const LIGHT_DIR: vec2<f32> = vec2<f32>(-0.5, 0.7);
const SKY_GAIN: f32 = 1.3;
const SCENE_SHARE: f32 = 0.6;
// the highlight's radius / offset, the crescent's (outer, inner) radius / offset and its strength, the body's sky light
// (at the top, more toward the bottom), the highlight's and the crescent's shares
const HL_R: f32 = 0.42;
const CR_R: vec2<f32> = vec2<f32>(0.6, 0.1);
const CR_GAIN: f32 = 0.4;
const BODY_SKY: f32 = 0.10;
const HL_SHARE: f32 = 0.9;
const CR_SHARE: f32 = 0.45;
const TAPS = array<vec2<f32>, 6>(vec2<f32>(0.2041, 0.0), vec2<f32>(-0.2898, 0.2652), vec2<f32>(0.0302, -0.5138), vec2<f32>(0.4167, 0.4366), vec2<f32>(-0.7137, -0.1052), vec2<f32>(0.6573, -0.5909));
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<i32>(i32(u.a.x), i32(u.a.y));
    let ndc = vec2<f32>(pos.x / u.a.x * 2.0 - 1.0, 1.0 - pos.y / u.a.y * 2.0);
    let q = u.h0.xyz * ndc.x + u.h1.xyz * ndc.y + u.h2.xyz;
    if q.z <= 0.0 { return keep(pos.xy); }
    let uv = q.xy / q.z;
    if uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 { return keep(pos.xy); }
    let gw = u.inv.w;
    let gh = u.a.z;
    // the grid cell and its instances; a cell with none needs nothing else (no depth read)
    let cell = clamp(vec2<i32>(floor(uv * vec2<f32>(GRID))), vec2<i32>(0), GRID - vec2<i32>(1));
    if textureLoad(grid, vec2<i32>(cell.x * SLOTS, cell.y), 0).x == NONE { return keep(pos.xy); }
    let inv_d = u.inv.x * ndc.x + u.inv.y * ndc.y + u.inv.z;
    if inv_d <= 0.0 { return keep(pos.xy); }
    let wg = 1.0 / inv_d;
    let pm = vec2<f32>(uv.x * gw, uv.y * gh);
    var best_h = 0.0;
    var best_n = vec2<f32>(0.0);
    var best_p = vec2<f32>(0.0);
    var best_r = 0.0;
    for (var k = 0; k < SLOTS; k = k + 1) {
        let id = textureLoad(grid, vec2<i32>(cell.x * SLOTS + k, cell.y), 0).x;
        if id == NONE { break; }
        let i0 = i32(id) * 2;
        let g0 = textureLoad(inst, vec2<i32>(i0 % INST_W, i0 / INST_W), 0);
        let g1 = textureLoad(inst, vec2<i32>((i0 + 1) % INST_W, (i0 + 1) / INST_W), 0);
        let a = g0.xy;
        let ab = g0.zw - g0.xy;
        let l2 = max(dot(ab, ab), MIN_SEG2);
        let tt = clamp(dot(pm - a, ab) / l2, 0.0, 1.0);
        let off = pm - (a + ab * tt);
        let r = mix(g1.x, g1.y, tt);
        let dd = dot(off, off) / (r * r);
        if dd < 1.0 {
            let h = sqrt(1.0 - dd) * g1.z;
            if h > best_h {
                best_h = h;
                best_n = off;
                best_p = off / r;
                best_r = r;
            }
        }
    }
    if best_h <= 0.0 { return keep(pos.xy); }
    // the glass's distance here; a nearer surface (dash, pillar, mirror) is not seen through the glass
    let d0 = lin(dload(vec2<i32>(pos.xy), dims));
    if d0 < wg - GLASS_SLACK { return keep(pos.xy); }
    // the border of the glass: the drops thin out over the last 1.5 cm
    let edge = min(min(pm.x, gw - pm.x), min(pm.y, gh - pm.y));
    let inside = clamp(best_h * HEIGHT_COVER, 0.0, 1.0) * clamp(edge / EDGE_FADE, 0.0, 1.0) * u.a.w;
    if inside <= MIN_INSIDE { return keep(pos.xy); }
    // pixels per metre on the glass here
    let ppm = u.b.z / wg;
    // the lens: the point on the other side of the drop's centre, inverted and wider; screen y is down, glass v is up
    let so = vec2<f32>(-best_n.x, best_n.y) * u.b.w * ppm;
    let rpx = best_r * ppm;
    let blur = clamp(BLUR_PER_R * rpx, BLUR_MIN, BLUR_MAX);
    let c = base(pos.xy);
    // 6 taps of a fixed Vogel disc, turned per pixel by interleaved gradient noise (no trig per tap)
    let rot = ign(pos.xy) * TAU;
    let rc = vec2<f32>(cos(rot), sin(rot));
    var sum = min(c.rgb, vec3<f32>(TAP_MAX));
    for (var i = 0; i < N_TAPS; i = i + 1) {
        let v = TAPS[i];
        let sp = pos.xy + so + vec2<f32>(v.x * rc.x - v.y * rc.y, v.x * rc.y + v.y * rc.x) * blur;
        // a tap that falls off the glass (onto a pillar, the dash) is not the world behind it: it keeps the pixel's own colour
        let nt = vec2<f32>(sp.x / u.a.x * 2.0 - 1.0, 1.0 - sp.y / u.a.y * 2.0);
        let qt = u.h0.xyz * nt.x + u.h1.xyz * nt.y + u.h2.xyz;
        let ut = qt.xy / qt.z;
        var tc = c.rgb;
        if qt.z > 0.0 && ut.x >= 0.0 && ut.x <= 1.0 && ut.y >= 0.0 && ut.y <= 1.0 {
            tc = textureSampleLevel(t, s, (sp - u.bx.xy) / u.bx.zw, 0.0).rgb;
        }
        sum = sum + min(tc, vec3<f32>(TAP_MAX));
    }
    let wsum = TAP_WEIGHTS;
    var col = sum / wsum;
    // a drop is a little lens: the rim is dark (total reflection), the middle a touch brighter and cooler, a highlight sits up
    // and to the left (the sky over the car) and a faint bright crescent opposite it (the light that came through)
    let rim = pow(clamp(1.0 - best_h, 0.0, 1.0), RIM_POW);
    col = col * (1.0 - RIM_DARK * rim) * MID_GAIN + MID_TINT * (1.0 - rim);
    let l = normalize(LIGHT_DIR);
    let lum = dot(col, LUMA_709);
    // what they reflect: the sky over the car and, a little, the light in the scene itself (a lamp, a bright road)
    let sky = max(u.sky.xyz, vec3<f32>(0.0)) * SKY_GAIN + vec3<f32>(SCENE_SHARE * min(lum, 1.0));
    let hl = smoothstep(HL_R, 0.0, length(best_p - l * HL_R));
    let cr = smoothstep(CR_R.x, CR_R.y, length(best_p + l * CR_R.x)) * CR_GAIN;
    // the whole drop gathers a little sky light too, most toward the lower part
    let body = (1.0 - rim) * (BODY_SKY + BODY_SKY * (0.5 - 0.5 * best_p.y));
    col = col + sky * (hl * HL_SHARE + cr * CR_SHARE + body);
    return done(pos.xy, col, inside);
}
"#;

/// The frame's numbers the pass needs besides the glass.
#[derive(Clone, Copy)]
pub(crate) struct Look {
    pub strength: f32,
    /// the sky's colour (the scene's clear colour)
    pub sky: [f32; 3],
    /// the frame's size in pixels
    pub width: u32,
    pub height: u32,
}

/// The pass's shader over a plain or a `multisampled` depth, the full image or `in_place` (the box over the scene).
pub(crate) fn source(multisampled: bool, in_place: bool) -> String {
    let sizes = format!("const GRID: vec2<i32> = vec2<i32>({GRID_COLS}, {GRID_ROWS});\nconst SLOTS: i32 = {CELL_SLOTS};\nconst NONE: u32 = {NO_INSTANCE:#X}u;\nconst INST_W: i32 = {INST_W};");
    let body = DROPS_WGSL.replace("//MODE", if in_place { MODE_IN_PLACE } else { MODE_FULL }).replace("//SIZES", &sizes);
    wgsl::compose(&[wgsl::FULLSCREEN_VS, wgsl::dtex(multisampled), wgsl::IGN, wgsl::DLOAD, wgsl::ANGLES, wgsl::LUMA_709, wgsl::PROJ_DIST, &body])
}

pub(crate) struct DropsPass {
    /// the pipeline per `(in place, multisampled depth)`, built at its first use
    pipes: Variants<(bool, bool), wgpu::RenderPipeline>,
    image_layout: wgpu::BindGroupLayout,
    depth: DepthBinding,
    u_layout: wgpu::BindGroupLayout,
    u_buf: UniformBuf<DropsUniform>,
    u_bg: wgpu::BindGroup,
    inst_tex: wgpu::Texture,
    grid_tex: wgpu::Texture,
    /// `InPlace`: the scratch copy of the glass's box (size, texture, bind group), grown when the box outgrows it
    scratch: Option<(u32, u32, wgpu::Texture, wgpu::BindGroup)>,
    /// the water, kept while the view shows the windscreen
    pub sim: Option<Sim>,
    inst: Vec<[f32; 4]>,
    grid: Vec<u32>,
    /// what `inst` / `grid` were built from (the water's step count, the glass's size) and their count; `None` = build again
    built: Option<((u64, [u32; 2]), usize)>,
    /// `inst` / `grid` were rebuilt and are not on the GPU yet
    upload_due: bool,
}

impl DropsPass {
    pub fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let u_layout = gfx::bgl(
            device,
            "drops",
            &[gfx::bgl_uniform(0, wgpu::ShaderStages::FRAGMENT), gfx::bgl_tex(1, wgpu::TextureSampleType::Float { filterable: false }, false), gfx::bgl_tex(2, wgpu::TextureSampleType::Uint, false)],
        );
        let u_buf = UniformBuf::<DropsUniform>::new(device, "drops");
        let data_tex = |label: &'static str, w: u32, h: u32, format: wgpu::TextureFormat| {
            gfx::texture_2d(device, label, w, h, format, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST)
        };
        let inst_tex = data_tex("drops instances", INST_W, INST_H, wgpu::TextureFormat::Rgba32Float);
        let grid_tex = data_tex("drops grid", (GRID_COLS * CELL_SLOTS) as u32, GRID_ROWS as u32, wgpu::TextureFormat::R32Uint);
        let (iv, gv) = (inst_tex.create_view(&Default::default()), grid_tex.create_view(&Default::default()));
        let u_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("drops"),
            layout: &u_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: u_buf.buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&iv) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&gv) },
            ],
        });
        let depth = DepthBinding::new(device, "drops depth");
        Self { pipes: Variants::default(), image_layout: layout.clone(), depth, u_layout, u_buf, u_bg, inst_tex, grid_tex, scratch: None, sim: None, inst: Vec::new(), grid: Vec::new(), built: None, upload_due: false }
    }

    /// The pass's pipeline (`in_place` or the full image) and bind groups 1 (the depth) and 2 (the uniform and the data).
    fn bind(&mut self, device: &wgpu::Device, depth: DepthSrc, in_place: bool) -> (&wgpu::RenderPipeline, &wgpu::BindGroup, &wgpu::BindGroup) {
        let ms = depth.multisampled;
        let (image_layout, depth_layout, u_layout) = (&self.image_layout, self.depth.layout(ms), &self.u_layout);
        let pipe = self.pipes.get((in_place, ms), || {
            let module = gfx::wgsl_module(device, "drops", &source(ms, in_place));
            let pl = gfx::pipeline_layout(device, "drops", &[image_layout, depth_layout, u_layout]);
            Fullscreen::new("drops", &pl, &module).build(device, &[gfx::target(HDR_FORMAT, None)])
        });
        (pipe, self.depth.bind(device, depth), &self.u_bg)
    }

    /// Step the water to the scene's clock and say whether there is anything to draw (`None` = skip the pass; the water
    /// is dropped by the caller, so the next look at the windscreen starts from a fresh pre-roll).
    pub fn prepare(&mut self, d: &Drops, g: &Glass) -> Option<usize> {
        let p = Params { wet: d.wet, cover: d.cover, speed: d.speed, gravity: g.gravity, size: g.size };
        // a fresh glass (the first look, a jump of the clock) is built again whatever its step count
        let fresh = match self.sim.as_mut() {
            Some(s) => s.advance(d.time, &p),
            None => {
                self.sim = Some(Sim::new(d.time, &p));
                true
            }
        };
        if fresh {
            self.built = None;
        }
        let s = self.sim.as_mut().expect("drops sim");
        if !s.busy(&p) {
            return None;
        }
        // PERF: the instances are a function of the water and the glass's size: built (and uploaded) only when either changed
        let key = (s.steps, g.size.map(f32::to_bits));
        let n = match self.built {
            Some((k, n)) if k == key => n,
            _ => {
                let n = s.build(g.size, &mut self.inst, &mut self.grid);
                self.built = Some((key, n));
                self.upload_due = true;
                n
            }
        };
        (n > 0).then_some(n)
    }

    /// The glass's box on the screen in pixels `(x0, y0, x1, y1)` (padded, clamped to the frame); `None` when it is empty.
    fn pixel_box(g: &Glass, width: u32, height: u32) -> Option<[u32; 4]> {
        let [nx0, ny0, nx1, ny1] = g.ndc_box;
        let (w, h) = (width as f32, height as f32);
        let px = |v: f32, n: f32| v.clamp(0.0, n);
        let x0 = px((nx0 * 0.5 + 0.5) * w - BOX_PAD, w).floor() as u32;
        let x1 = px((nx1 * 0.5 + 0.5) * w + BOX_PAD, w).ceil() as u32;
        let y0 = px((0.5 - ny1 * 0.5) * h - BOX_PAD, h).floor() as u32;
        let y1 = px((0.5 - ny0 * 0.5) * h + BOX_PAD, h).ceil() as u32;
        (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
    }

    /// The data of this frame: the instance texels and the grid (when [`Self::prepare`] rebuilt them; queued writes, once a
    /// frame: only this pass reads them), and the uniform (through the belt, `cx.encoder`).
    fn upload(&mut self, cx: &mut PostCtx, glass: &Glass, look: Look, bx: [f32; 4], bz: [f32; 4]) {
        let (queue, cam) = (cx.queue, cx.camera);
        if std::mem::take(&mut self.upload_due) {
            self.inst.resize((INST_W * INST_H) as usize, [0.0; 4]);
            let write = |tex: &wgpu::Texture, bytes: &[u8], w: u32, h: u32, bpp: u32| {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    bytes,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * bpp), rows_per_image: Some(h) },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            };
            write(&self.inst_tex, bytemuck::cast_slice(&self.inst), INST_W, INST_H, INST_TEXEL_BYTES);
            write(&self.grid_tex, bytemuck::cast_slice(&self.grid), (GRID_COLS * CELL_SLOTS) as u32, GRID_ROWS as u32, GRID_TEXEL_BYTES);
        }
        let (cols, inv) = (glass.to_uv.to_cols_array_2d(), glass.inv_dist);
        let h = look.height as f32;
        let u = DropsUniform {
            h0: [cols[0][0], cols[0][1], cols[0][2], 0.0],
            h1: [cols[1][0], cols[1][1], cols[1][2], 0.0],
            h2: [cols[2][0], cols[2][1], cols[2][2], 0.0],
            inv: [inv.x, inv.y, inv.z, glass.size[0]],
            a: [look.width as f32, h, glass.size[1], look.strength],
            b: [cam.far / (cam.near - cam.far), cam.near, 0.5 * h / (0.5 * cam.fov_y_radians).tan(), REFRACT],
            sky: [look.sky[0], look.sky[1], look.sky[2], 0.0],
            bx,
            bz,
        };
        self.u_buf.write(cx.belt, cx.encoder, &u);
    }

    /// Run the pass over `src` (`look`'s size, its bind group `src_bg`) into `out` (the chain's image of that size), reading
    /// `cx.depth`. `prepare` has just said there are instances.
    pub fn run(&mut self, cx: &mut PostCtx, src_bg: &wgpu::BindGroup, glass: &Glass, look: Look, out: &wgpu::TextureView) {
        let PostCtx { device, depth, .. } = *cx;
        let (w, h) = (look.width as f32, look.height as f32);
        self.upload(cx, glass, look, [0.0, 0.0, w, h], [w, h, 0.0, 0.0]);
        let (pipe, depth_bg, u_bg) = self.bind(device, depth, false);
        gfx::fullscreen_pass(cx.encoder, "drops", "windscreen drops", out, wgpu::LoadOp::Clear(wgpu::Color::BLACK), pipe, &[src_bg, depth_bg, u_bg]);
    }

    /// Run the pass over the scene's own image `scene` (`COPY_SRC`, the float image): a copy of the glass's box as the colour
    /// source and a blended draw over that box only - the rest of the frame is not touched, read or written.
    pub fn run_in_place(&mut self, cx: &mut PostCtx, scene: &wgpu::Texture, scene_view: &wgpu::TextureView, glass: &Glass, look: Look) {
        let PostCtx { device, layout, sampler, depth, .. } = *cx;
        let Some([x0, y0, x1, y1]) = Self::pixel_box(glass, look.width, look.height) else { return };
        let (bw, bh) = (x1 - x0, y1 - y0);
        if !self.scratch.as_ref().is_some_and(|(w, h, ..)| *w >= bw && *h >= bh) {
            let (w, h) = (bw + SCRATCH_SLACK, bh + SCRATCH_SLACK);
            let texture = gfx::texture_2d(device, "drops scratch", w, h, HDR_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST);
            let bg = gfx::tex_sampler_bg(device, "drops scratch", layout, &texture.create_view(&Default::default()), sampler);
            self.scratch = Some((w, h, texture, bg));
        }
        let (sw, sh, scratch, scratch_bg) = self.scratch.as_ref().expect("drops scratch");
        cx.encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: scene, mip_level: 0, origin: wgpu::Origin3d { x: x0, y: y0, z: 0 }, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyTextureInfo { texture: scratch, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::Extent3d { width: bw, height: bh, depth_or_array_layers: 1 },
        );
        let bx = [x0 as f32, y0 as f32, *sw as f32, *sh as f32];
        let bz = [bw as f32, bh as f32, 0.0, 0.0];
        let scratch_bg = scratch_bg.clone();
        self.upload(cx, glass, look, bx, bz);
        let (pipe, depth_bg, u_bg) = self.bind(device, depth, true);
        let mut pass = gfx::color_pass(cx.encoder, "drops", "windscreen drops", scene_view, wgpu::LoadOp::Load);
        pass.set_scissor_rect(x0, y0, bw, bh);
        gfx::draw_fullscreen(&mut pass, pipe, &[&scratch_bg, depth_bg, u_bg]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad() -> [Vec3; 4] {
        // an upright 1.2 x 0.6 m pane two metres ahead of a camera at the origin looking down -z (right-handed look-at)
        [Vec3::new(-0.6, 0.3, -2.0), Vec3::new(0.6, 0.3, -2.0), Vec3::new(0.6, -0.3, -2.0), Vec3::new(-0.6, -0.3, -2.0)]
    }

    fn cam() -> Camera {
        Camera { position: Vec3::ZERO, target: Vec3::new(0.0, 0.0, -1.0), up: Vec3::Y, fov_y_radians: 1.0, near: 0.1, far: 700.0 }
    }

    fn drops(corners: [Vec3; 4]) -> Drops {
        Drops { corners, wet: 1.0, cover: 0.0, speed: 0.0, time: 0.0, up: Vec3::Y }
    }

    #[test]
    fn the_glass_maps_its_corners_to_the_unit_square_and_knows_its_distance() {
        let c = cam();
        let g = glass(&drops(quad()), &c, 1.6).expect("glass");
        let vp = c.view_projection(1.6);
        let uv_of = |p: Vec3| {
            let q = vp * p.extend(1.0);
            let ndc = Vec2::new(q.x, q.y) / q.w;
            let h = g.to_uv * Vec3::new(ndc.x, ndc.y, 1.0);
            (Vec2::new(h.x, h.y) / h.z, 1.0 / (g.inv_dist.dot(Vec3::new(ndc.x, ndc.y, 1.0))))
        };
        let mut uvs: Vec<Vec2> = quad().iter().map(|&p| uv_of(p).0).collect();
        uvs.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
        for (want, got) in [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0)].iter().zip(&uvs) {
            assert!((*want - *got).length() < 1e-3, "{want:?} vs {got:?}");
        }
        // the centre of the pane is the middle of the glass and two metres away
        let (mid, dist) = uv_of(Vec3::new(0.0, 0.0, -2.0));
        assert!((mid - Vec2::splat(0.5)).length() < 1e-3, "{mid:?}");
        assert!((dist - 2.0).abs() < 1e-3, "{dist}");
        assert!((g.size[0] - 1.2).abs() < 1e-4 && (g.size[1] - 0.6).abs() < 1e-4);
        // an upright pane: gravity runs straight down it
        assert!(g.gravity[0].abs() < 1e-4 && (g.gravity[1] + 1.0).abs() < 1e-4, "{:?}", g.gravity);
    }

    #[test]
    fn a_tilted_pane_has_a_gravity_component_and_a_pane_behind_the_camera_is_none() {
        let c = cam();
        // the top leaned back 35 degrees
        let t = 35f32.to_radians();
        let mut q = quad();
        for p in q.iter_mut() {
            let top = p.y > 0.0;
            p.z = -2.0 + if top { -0.3 * t.sin() } else { 0.3 * t.sin() };
            p.y = if top { 0.3 * t.cos() } else { -0.3 * t.cos() };
        }
        let g = glass(&drops(q), &c, 1.6).expect("glass");
        assert!((g.gravity[1] + t.cos()).abs() < 0.02, "{:?}", g.gravity);
        let behind = quad().map(|p| Vec3::new(p.x, p.y, 2.0));
        assert!(glass(&drops(behind), &c, 1.6).is_none());
    }

    fn params(wet: f32, speed: f32) -> Params {
        Params { wet, cover: 0.0, speed, gravity: [0.0, -0.57], size: [1.13, 0.65] }
    }

    fn bead(x: f32, y: f32, r: f32) -> Drop {
        Drop { pos: Vec2::new(x, y), r, dir: Vec2::NEG_Y, run: false, mark: Vec2::ZERO, wob: 0.0 }
    }

    #[test]
    fn the_water_is_a_function_of_the_clock_alone() {
        let p = params(1.0, 12.0);
        let (mut a, mut b) = (Sim::new(5.0, &p), Sim::new(5.0, &p));
        for k in 1..=200 {
            a.advance(5.0 + k as f64 / 30.0, &p);
            b.advance(5.0 + k as f64 / 30.0, &p);
        }
        let (mut ia, mut ga, mut ib, mut gb) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        a.build(p.size, &mut ia, &mut ga);
        b.build(p.size, &mut ib, &mut gb);
        assert!(!ia.is_empty());
        assert_eq!((ia, ga), (ib, gb));
    }

    #[test]
    fn big_drops_run_down_at_rest_and_small_ones_are_blown_up_at_speed() {
        let mut s = Sim::new(0.0, &params(0.0, 0.0));
        s.drops.clear();
        s.trails.clear();
        s.last_wipe = s.t;
        s.drops.push(bead(0.5, 0.4, 0.0045));
        s.drops.push(bead(0.9, 0.3, 0.0010));
        for _ in 0..30 {
            s.step(STEP, &params(0.0, 0.0));
        }
        assert!(s.drops.iter().any(|d| d.r > 0.003 && d.pos.y < 0.38), "a fat drop runs down: {:?}", s.drops);
        assert!(s.drops.iter().any(|d| d.r < 0.0015 && (d.pos - Vec2::new(0.9, 0.3)).length() < 1e-4), "a bead sticks");
        // at 40 m/s the airflow wins over the weight, even for a small drop
        let mut s = Sim::new(0.0, &params(0.0, 40.0));
        s.drops.clear();
        s.last_wipe = s.t;
        s.drops.push(bead(0.5, 0.2, 0.0020));
        for _ in 0..30 {
            s.step(STEP, &params(0.0, 40.0));
        }
        assert!(s.drops.iter().any(|d| d.pos.y > 0.22), "blown up the glass: {:?}", s.drops);
    }

    #[test]
    fn touching_drops_merge_by_volume() {
        let mut s = Sim::new(0.0, &params(0.0, 0.0));
        s.drops.clear();
        s.last_wipe = s.t;
        s.drops.push(bead(0.5, 0.3, 0.0020));
        s.drops.push(bead(0.5033, 0.3, 0.0020));
        s.step(STEP, &params(0.0, 0.0));
        assert_eq!(s.drops.len(), 1);
        assert!((s.drops[0].r - 0.0020 * 2f32.cbrt()).abs() < 1e-5);
    }

    #[test]
    fn the_wipe_clears_the_glass_and_a_dry_glass_goes_idle() {
        let mut s = Sim::new(0.0, &params(1.0, 10.0));
        assert!(!s.drops.is_empty());
        // the rain stops: within a few seconds the wipe has cleared everything and the pass has nothing to do
        let dry = params(0.0, 10.0);
        for _ in 0..(5.0 / STEP) as usize {
            s.step(STEP, &dry);
        }
        assert!(s.drops.is_empty() && s.trails.is_empty(), "{} drops left", s.drops.len());
        assert!(!s.busy(&dry));
        assert!(s.busy(&params(0.5, 10.0)));
        // under cover the glass is cleared at once and nothing is born
        let mut s = Sim::new(0.0, &params(1.0, 10.0));
        let cover = Params { cover: 1.0, ..params(1.0, 10.0) };
        for _ in 0..(2.0 / STEP) as usize {
            s.step(STEP, &cover);
        }
        assert!(s.drops.is_empty());
    }

    #[test]
    fn a_jump_of_the_clock_starts_a_fresh_glass_and_the_instances_fit_the_budget() {
        let p = params(1.0, 25.0);
        let mut s = Sim::new(0.0, &p);
        assert!(!s.advance(0.5, &p));
        assert!(s.advance(100.0, &p), "a jump over 3 s starts afresh");
        assert!(s.advance(50.0, &p), "a clock that went back starts afresh");
        let (mut inst, mut grid) = (Vec::new(), Vec::new());
        let n = s.build(p.size, &mut inst, &mut grid);
        assert!(n > 0 && n <= MAX_INSTANCES && inst.len() == 2 * n);
        assert_eq!(grid.len(), GRID_COLS * GRID_ROWS * CELL_SLOTS);
        assert!(grid.iter().all(|&i| i == NO_INSTANCE || (i as usize) < n));
        assert!(MAX_DROPS + MAX_TRAILS <= MAX_INSTANCES);
    }
}
