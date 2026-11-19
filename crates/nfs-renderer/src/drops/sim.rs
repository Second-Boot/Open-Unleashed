//! The water on the glass: the CPU simulation ([`Sim`]); render side only, never the game's RNG.

use super::*;

/// Standard gravity and the two constants that make the contact line hold a bead: a drop runs when
/// `|g_glass G r^2 + wind c_q v^2 r| > C` (weight ~ r^3 over r, the airflow's push ~ r^2 over r, the pull of the contact
/// line ~ constant), so a 2.5 mm drop runs down a 33 degree windscreen at rest and a 1 mm one is blown up at 80 km/h.
pub(super) const G: f32 = 9.81;
pub(super) const CONTACT: f32 = 5.0e-5;
pub(super) const AERO: f32 = 8.0e-5;
/// Run speed per unit of `|F| / C - 1` (m/s) and its cap.
pub(super) const RUN_GAIN: f32 = 0.04;
pub(super) const RUN_MAX: f32 = 0.45;
/// The volume a running drop leaves per metre (m^3).
pub(super) const LOSS_PER_M: f32 = 5.0e-8;
/// A trail piece lives this long (s).
pub(super) const TRAIL_LIFE: f32 = 2.4;
/// Length of the piece a running drop lays down (m).
pub(super) const TRAIL_STEP: f32 = 0.02;
/// Beads per square metre of glass per second at full wetness and at rest.
pub(super) const SPAWN: f32 = 320.0;
/// Seconds between two wipes at this wetness: 7 s in a drizzle, 3 s in a downpour.
pub(super) fn wipe_period(wet: f32) -> f32 {
    WIPE_PERIOD_DRIZZLE - WIPE_PERIOD_SPAN * wet.clamp(0.0, 1.0)
}
pub(super) const WIPE_PERIOD_DRIZZLE: f32 = 7.0;
pub(super) const WIPE_PERIOD_SPAN: f32 = 4.0;

/// The wipe: blades' length over the glass's width, duration, the pivots' x (over the width) and y (m, below the glass).
pub(super) const WIPE_TIME: f32 = 0.9;
pub(super) const WIPE_REACH: f32 = 0.7;
pub(super) const WIPE_PIVOTS: [f32; 2] = [0.30, 0.72];
pub(super) const WIPE_BELOW: f32 = 0.05;

/// The water's tuned numbers (an enhancement's look, tuned by eye).
/// Births: the rain's wetness below which nothing is born; the speed factor `SPEED_K_BASE + SPEED_K_GAIN * min(speed /
/// SPEED_K_REF, SPEED_K_CAP)`; the wetness exponent; a bead's radius `BEAD_R_MIN + BEAD_R_SPAN * a^2` (m), at rest (below
/// FAT_SPEED m/s) with the chance FAT_CHANCE a fat one `FAT_R_MIN + FAT_R_SPAN * c`.
pub(super) const DRY: f32 = 0.02;
pub(super) const SPEED_K_BASE: f32 = 0.6;
pub(super) const SPEED_K_GAIN: f32 = 0.8;
pub(super) const SPEED_K_REF: f32 = 25.0;
pub(super) const SPEED_K_CAP: f32 = 1.6;
pub(super) const WET_EXPONENT: f32 = 1.2;
pub(super) const BEAD_R_MIN: f32 = 0.0015;
pub(super) const BEAD_R_SPAN: f32 = 0.0032;
pub(super) const FAT_SPEED: f32 = 8.0;
pub(super) const FAT_CHANCE: f32 = 0.12;
pub(super) const FAT_R_MIN: f32 = 0.0045;
pub(super) const FAT_R_SPAN: f32 = 0.0025;
/// The airflow's outward share from the glass's middle.
pub(super) const SIDE_PUSH: f32 = 0.7;
/// The meander of a running drop: its rate `WOB_RATE + WOB_RATE_R * r * 100` (rad/s, r in m) and its swing (rad).
pub(super) const WOB_RATE: f32 = 0.7;
pub(super) const WOB_RATE_R: f32 = 3.0;
pub(super) const WOB_SWING: f32 = 0.18;
/// The smallest a running drop shrinks to, the trail's radius (a share of the drop's, clamped) (m).
pub(super) const MIN_R: f32 = 0.0004;
pub(super) const TRAIL_R_SHARE: f32 = 0.45;
pub(super) const TRAIL_R_MAX: f32 = 0.0011;
/// Merging: the sweep's reach along x (m), the touch distance as a share of the radii, the biggest drop (m).
pub(super) const MERGE_REACH: f32 = 0.02;
pub(super) const MERGE_TOUCH: f32 = 0.85;
pub(super) const MAX_R: f32 = 0.009;
/// A drop this far (m) off the glass is gone.
pub(super) const OFF_GLASS: f32 = 0.01;
/// The wipe: the covered value above which the glass is cleared at once, below which it may be again; the seconds after the
/// rain stopped; the blade's dead zone at the pivot (m) and its angular margin (rad).
pub(super) const COVER_WIPE: f32 = 0.5;
pub(super) const COVER_REARM: f32 = 0.2;
pub(super) const DRY_WIPE_AFTER: f32 = 1.5;
pub(super) const BLADE_HUB: f32 = 0.02;
pub(super) const BLADE_MARGIN: f32 = 0.03;
/// A fresh glass: the pre-roll in wipe periods (at least `PRE_ROLL_MIN_PERIOD` s each); the clock jump that starts afresh (s);
/// the first wipe's time before the start (s).
pub(super) const PRE_ROLL_PERIODS: f32 = 2.8;
pub(super) const PRE_ROLL_MIN_PERIOD: f32 = 1.5;
pub(super) const CLOCK_JUMP: f64 = 3.0;
pub(super) const FIRST_WIPE: f32 = -2.5;
/// The instances: the grid cell margin round a capsule (m); a trail's alpha and its end's radius share; a running drop's
/// tail length (radii, capped, m) and its end's radius share.
pub(super) const CELL_MARGIN: f32 = 0.001;
pub(super) const TRAIL_ALPHA: f32 = 0.45;
pub(super) const TRAIL_TAPER: f32 = 0.8;
pub(super) const TAIL_RADII: f32 = 2.2;
pub(super) const TAIL_MAX: f32 = 0.012;
pub(super) const TAIL_TAPER: f32 = 0.55;

#[derive(Clone, Copy, Debug)]
pub(super) struct Drop {
    pub(super) pos: Vec2,
    /// radius (m)
    pub(super) r: f32,
    /// the direction it runs (unit) and whether it does
    pub(super) dir: Vec2,
    pub(super) run: bool,
    /// the last trail piece's end
    pub(super) mark: Vec2,
    /// a slow sideways meander (rad), drifting
    pub(super) wob: f32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Trail {
    pub(super) a: Vec2,
    pub(super) b: Vec2,
    pub(super) r: f32,
    pub(super) born: f32,
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
    pub(super) rng: u32,
    /// the sim's own clock (s since it started) and the scene clock it follows
    pub(super) t: f32,
    pub(super) last_scene: f64,
    pub(super) drops: Vec<Drop>,
    pub(super) trails: std::collections::VecDeque<Trail>,
    pub(super) spawn_acc: f32,
    pub(super) last_wipe: f32,
    /// seconds into the running wipe
    pub(super) wipe: Option<f32>,
    pub(super) cover_cleared: bool,
    /// the part of a step the scene's clock has advanced but no step has run for
    pub(super) carry: f32,
    /// the steps run so far: the instances change only when this does
    pub(super) steps: u64,
    /// the merge's scratch: the drops in x order, which of them merged away (kept between steps)
    pub(super) order: Vec<usize>,
    pub(super) dead: Vec<bool>,
    /// the build's scratch: the instances per grid cell
    pub(super) fill: Vec<u8>,
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

    pub(super) fn rand(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// The force that works on a drop of radius `r` at `pos`, as (along u, along v).
    pub(super) fn force(&self, d: &Drop, p: &Params, q: f32) -> Vec2 {
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
            let ease = |x: f32| nfs_math::smooth01(x.clamp(0.0, 1.0));
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
