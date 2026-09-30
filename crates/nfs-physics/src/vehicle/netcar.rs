//! **The remote car between packets** — the receiver's dead reckoning, `RE/notes/network.md` §8.4:
//! `0x4A0A40` (the apply, from `Car_integrateOrient` 0x49B8E0 at 64 Hz), `0x4A04A0` (snap or blend),
//! `0x4A02A0` (the snap's tail) and `0x4A03D0` (the store's own snap). Read to the instruction.
//!
//! The lag book the store 0x4A0DF0 keeps (`+0x5C…+0x70`) is the network layer's — `porsche::netplay`;
//! this file is what those routines do to the car.

use glam::{Mat3, Vec3};

use super::Vehicle;
use crate::{collision::TriangleGrid, specs::CarSpecs};

/// One `0xBE` block as the store keeps it in the car (`car+0x38…+0x5B`), already decoded the way the
/// apply reads it: the yaw rate `× 0.001` (`+0x40`), the matrix from the quaternion by 0x4A0020
/// `× 2⁻¹⁶` (`+0x42`), the velocity `× 0.01` (`+0x4A`) and the position (`+0x50`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    /// `+0x3C`, the sender's tick
    pub tick: i32,
    /// `+0x39`: the sender's body is clear of the ground — 0x4A0A40 then skips the re-seat
    pub airborne: bool,
    pub yaw_rate: f32,
    /// rows of `car+0x364`: body x, up, forward
    pub matrix: [[f32; 3]; 3],
    pub velocity: Vec3,
    pub position: Vec3,
}

/// `[0x5BB2C0]` — 0x401921FB20000000, the turn-to-radian factor of 0x5327E0 / 0x5327F0 (`fsin` /
/// `fcos` of `x · 6.283185005…`); the angles here are in turns.
const TURN: f64 = f64::from_bits(0x4019_21FB_2000_0000);

/// 0x5327E0 / 0x5327F0 — `(sin, cos)` of an angle in turns, in double precision as the x87 does.
fn sin_cos(turns: f32) -> (f64, f64) {
    let x = turns as f64 * TURN;
    (x.sin(), x.cos())
}

fn rows(m: Mat3) -> [[f32; 3]; 3] {
    [m.row(0).to_array(), m.row(1).to_array(), m.row(2).to_array()]
}

fn from_rows(r: [[f32; 3]; 3]) -> Mat3 {
    Mat3::from_cols(Vec3::from(r[0]), Vec3::from(r[1]), Vec3::from(r[2])).transpose()
}

/// 0x532470(a, b, out): the row-major product `a · b`.
fn mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut d = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            d[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    d
}

/// 0x532600 — the Y rotation `[[c, 0, −s], [0, 1, 0], [s, 0, c]]` (0x53261B…0x53264A: `[0] = c`,
/// `[2] = −s`, `[6] = s`, `[8] = c`, `[4] = 1`), the angle in turns.
fn rot_y(turns: f32) -> [[f32; 3]; 3] {
    let (s, c) = sin_cos(turns);
    let (s, c) = (s as f32, c as f32);
    [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
}

impl Vehicle {
    /// **0x4A0A40** for a car that has a block (`+0x3C != 0`, the caller's gate: a network race,
    /// `car+0xDAC`, class ≤ 2 and `car+0x70` clear). `now` is `gGameTick`; `centre` is the
    /// tracker's slice centre `slice + 4` (the road-slice table at `[0x628BC0] + 0xC`, stride 0x50)
    /// or `None` when the port has no slice for the car. Returns whether the car has gone stale —
    /// `|age| > 0x100` sets `car+0x70` (0x4A0DC3).
    ///
    /// 1. `age = now − tick`; `θ = −trunc(age/2) · ω/64` turns turns the velocity's x / z
    ///    (`x' = cos θ·x − sin θ·z`, `z' = sin θ·x + cos θ·z`); `P += v' · (1/64) · age`;
    /// 2. `M ← M · Ry(age · ω/64)` — 0x49B870 with `x = z = 0`, then 0x532470;
    /// 3. the tracker `car+8` is copied and retracked at `P` (0x474060); while it reports no patch
    ///    (`word == 0`) `P` steps by the unit vector toward the slice's centre until it does, and takes
    ///    two more such steps (0x4A0C2B…0x4A0D23);
    /// 4. 0x4A04A0: `d = |P − pos|`; above 20 a snap ([`Self::net_snap`]), else a blend;
    /// 5. unless the block says airborne: the re-seat ([`Self::net_reseat`]).
    ///
    /// **Not carried**: `car+0xEC4 = |v'|` (0x4A0D5B…0x4A0D61; no reader in the port) and `car+0x6C`'s
    /// counter, which the store resets and nothing reads.
    pub fn net_dead_reckon(&mut self, b: &Block, now: i32, grid: &TriangleGrid, specs: &CarSpecs, centre: Option<Vec3>) -> bool {
        self.net_car = true;
        let age = now.wrapping_sub(b.tick);
        let w = b.yaw_rate;
        let wd = w * (1.0 / 64.0); // `fStack_84`, `[0x5B2828]`
        // 0x4A0B3A…0x4A0B4E: `−(age / 2)` (a `cdq; sub; sar` — truncation toward zero) times ω/64
        let theta = -((age / 2) as f32) * wd;
        let (s, c) = sin_cos(theta);
        let (vx, vy, vz) = (b.velocity.x as f64, b.velocity.y as f64, b.velocity.z as f64);
        let v2 = Vec3::new((c * vx - s * vz) as f32, vy as f32, (c * vz + s * vx) as f32);
        let a = age as f64;
        let k = (1.0f32 / 64.0) as f64;
        let mut p = Vec3::new(
            ((v2.x as f64 * k) * a + b.position.x as f64) as f32,
            ((v2.y as f64 * k) * a + b.position.y as f64) as f32,
            ((v2.z as f64 * k) * a + b.position.z as f64) as f32,
        );
        // 0x4A0BDC: `age · (ω/64)`, the yaw the block's heading has turned since it was sent
        let m = mul(&b.matrix, &rot_y(age as f32 * wd));
        // 0x4A0C0B…0x4A0D23: the retrack on a copy of the body tracker
        if let Some(centre) = centre {
            let mut t = self.tracker;
            t.update_zone(grid, p);
            if t.word == 0 {
                let mut d = centre - p;
                if d != Vec3::ZERO {
                    // 0x5328D0: `[0x5B24A8] / |d|`, 1.0
                    d *= 1.0 / d.length();
                }
                let mut n = 0;
                while t.word == 0 && n < 4096 {
                    p += d;
                    t.update_zone(grid, p);
                    n += 1;
                }
                p += d;
                p += d;
            }
        }
        self.net_blend_or_snap(w, p, v2, &m, grid, specs);
        // 0x4A0D47…0x4A0D5B: `car+0xD7E = car+0xD82 = 1` — the wanted and the engaged gear are neutral, so
        // the remote car's own `Car_simulate` puts no drive into it (`car+0xEC4 = |v'|` has no reader here)
        self.requested_gear = 1;
        self.gear = 1;
        // 0x4A0D5B: the unless-airborne re-seat (`car+0x39`)
        if !b.airborne {
            self.net_reseat(grid, specs);
        }
        age.abs() > 0x100
    }

    /// **0x4A04A0(car, ω, P, v', M)**.
    fn net_blend_or_snap(&mut self, w: f32, p: Vec3, v: Vec3, m: &[[f32; 3]; 3], grid: &TriangleGrid, specs: &CarSpecs) {
        let dv = p - self.position;
        let d = dv.length();
        // 0x4A04C0: `d <= [0x5B2470]` = 20.0 blends
        if d <= 20.0 {
            // `[0x5B2460]` = 6.0: above it `d · [0x5B4A8C]` (1/75), else the constant 0x3DA3D70A = 0.08
            let k = if 6.0 < d { d * f32::from_bits(0x3C5A_740D) } else { f32::from_bits(0x3DA3_D70A) };
            let (ax, az) = (v.x.abs(), v.z.abs());
            // 0x4A04E0…0x4A0511: `max(|x|, |z|) + ¼ · min` (`[0x5B2468]`)
            self.speed_est = if ax <= az { ax * 0.25 + az } else { az * 0.25 + ax };
            self.position += dv * k;
            let o = rows(self.orientation);
            // 0x4A0529: the forward rows' dot, `car+0x37C` against `M + 6`; `0 <= dot` blends
            let dot = Vec3::from(o[2]).dot(Vec3::from(m[2]));
            let out = if dot >= 0.0 {
                let mut r = [[0.0f32; 3]; 3];
                for i in 0..3 {
                    for j in 0..3 {
                        // `M · [0x5B23F4] (0.2) + old · [0x5B4A88] (0.8)` (0x4A0540…0x4A055A)
                        r[i][j] = m[i][j] * f32::from_bits(0x3E4C_CCCD) + o[i][j] * f32::from_bits(0x3F4C_CCCD);
                    }
                }
                r
            } else {
                *m
            };
            self.orientation = from_rows(out);
            self.renormalise();
        } else {
            // the snap: `car+0x388 / +0x38C / +0x390 = (0, ω, 0)`, velocity, matrix, position, then 0x4A02A0
            self.angular_velocity = Vec3::new(0.0, w, 0.0);
            self.velocity = v;
            self.orientation = from_rows(*m);
            self.position = p;
            self.net_snap_tail(grid, specs);
        }
    }

    /// **0x4A02A0** — the snap's tail: the tracker (0x49AF70), the up row `car+0x370` from the plane
    /// normal `car+0x3E8`, the other rows rebuilt from it (`row0 = up × fwd`, `fwd = row0 × up`,
    /// 0x532880 twice), 0x4A4810, the speed estimate, the `Susp_update` re-seat, then
    /// `car+0xC = −1` (the tracker forgets its patch) and 0x49AE30.
    pub fn net_snap_tail(&mut self, grid: &TriangleGrid, specs: &CarSpecs) {
        self.tracker.track(grid, self.position);
        let up = self.tracker.plane_normal;
        let fwd = self.orientation.row(2);
        let r0 = up.cross(fwd);
        let r2 = r0.cross(up);
        self.orientation = from_rows([r0.to_array(), up.to_array(), r2.to_array()]);
        self.renormalise();
        let (ax, az) = (self.velocity.x.abs(), self.velocity.z.abs());
        self.speed_est = if ax <= az { ax * 0.25 + az } else { az * 0.25 + ax };
        self.net_reseat(grid, specs);
        self.tracker.forget_patch();
    }

    /// The re-seat 0x4A0D6B…0x4A0DB2 (and 0x4A02A0's identical tail): `car+0x41C = Susp_update`,
    /// `car+0x334 −= car+0x424`, `car+0x424 = 0`, and each wheel's `+0x68` (compression), `+0x70`
    /// (render angle — the port has no such field) and `+0x84` (airborne) cleared.
    pub fn net_reseat(&mut self, grid: &TriangleGrid, specs: &CarSpecs) {
        self.ground_h = self.susp_update(grid, specs, false);
        self.position.y -= self.body_height;
        self.body_height = 0.0;
        for w in &mut self.wheels {
            w.compression = 0.0;
            w.airborne = false;
        }
    }

    /// **The two snap arms of 0x4A0680** (0x4A0803…0x4A086B and 0x4A0930…0x4A0994): the stale remote
    /// car is put on the slice the road walk chose — `car+0x388 / +0x38C / +0x390 = 0`, the velocity
    /// `0x532360(1, slice+0x1C, along, car+0x33C)` = the slice's forward times the block's speed along it,
    /// the forward row `car+0x37C` = the slice's forward, the position `car+0x330` = the block's `P` (`+0x50`)
    /// — and then 0x4A02A0 ([`Self::net_snap_tail`]).
    pub fn net_stale_place(&mut self, p: Vec3, forward: Vec3, along: f32, grid: &TriangleGrid, specs: &CarSpecs) {
        self.net_car = true;
        self.angular_velocity = Vec3::ZERO;
        self.velocity = forward * along;
        let mut r = rows(self.orientation);
        r[2] = forward.to_array();
        self.orientation = from_rows(r);
        self.position = p;
        self.net_snap_tail(grid, specs);
    }

    /// **0x4A03D0**, the store's snap when the lag has run past its limits (`car+0x70` was clear): the
    /// block's velocity (`+0x4A`) straight into `car+0x33C`, its matrix into `car+0x364` (0x4A4810), the
    /// tracker (0x49AF70) and `car+0xC = −1`. `car+0xE94 = 0` (unread here) and 0x49AE30 are not carried.
    pub fn net_store_snap(&mut self, velocity: Vec3, matrix: &[[f32; 3]; 3], grid: &TriangleGrid) {
        self.net_car = true;
        self.velocity = velocity;
        self.orientation = from_rows(*matrix);
        self.renormalise();
        self.tracker.track(grid, self.position);
        self.tracker.forget_patch();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collision::Triangle;

    fn flat() -> TriangleGrid {
        let s = 500.0;
        let (a, b, c, d) = (Vec3::new(-s, 0.0, -s), Vec3::new(s, 0.0, -s), Vec3::new(s, 0.0, s), Vec3::new(-s, 0.0, s));
        TriangleGrid::build(vec![Triangle::new(a, b, c, 1), Triangle::new(a, c, d, 1)], 100.0)
    }

    fn block(tick: i32, pos: Vec3, vel: Vec3, yaw: f32) -> Block {
        Block { tick, airborne: false, yaw_rate: yaw, matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], velocity: vel, position: pos }
    }

    fn car() -> (Vehicle, CarSpecs) {
        let specs = super::super::tests::specs_993();
        let mut v = Vehicle::bare(Vec3::new(0.0, 0.5, 0.0), Mat3::IDENTITY, 2.3, 1300.0, 0.8, 0.3);
        v.game_tick = 100;
        (v, specs)
    }

    /// 0x4A04A0: within 20 m the position moves `k = 0.08` of the way (`d / 75` above 6 m), never the
    /// whole gap; above 20 m it snaps to the block's point and takes its velocity and yaw rate.
    #[test]
    fn a_small_gap_blends_and_a_large_one_snaps() {
        let grid = flat();
        let (mut v, specs) = car();
        // age 0: P = the block's position
        let b = block(100, Vec3::new(3.0, 0.0, 0.0), Vec3::ZERO, 0.0);
        v.net_dead_reckon(&b, 100, &grid, &specs, None);
        assert!((v.position.x - 0.24).abs() < 1e-3, "0.08 of 3 m: {}", v.position.x);
        let (mut v, specs) = car();
        let b = block(100, Vec3::new(12.0, 0.0, 0.0), Vec3::ZERO, 0.0);
        v.net_dead_reckon(&b, 100, &grid, &specs, None);
        // the gap is 3-D: the car stands 0.5 m above the block's point
        let d = (144.0f32 + 0.25).sqrt();
        assert!((v.position.x - 12.0 * d / 75.0).abs() < 1e-3, "d/75 of the gap above 6 m: {}", v.position.x);
        let (mut v, specs) = car();
        let b = block(100, Vec3::new(30.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 10.0), 0.25);
        v.net_dead_reckon(&b, 100, &grid, &specs, None);
        assert!((v.position.x - 30.0).abs() < 1e-4, "snapped: {}", v.position.x);
        assert_eq!(v.velocity.z, 10.0);
        assert_eq!(v.angular_velocity, Vec3::new(0.0, 0.25, 0.0));
    }

    /// The extrapolation is `P + v'·age/64` with the velocity turned by half the yaw: with no yaw
    /// rate a car 8 ticks late sits `8/64 · v` ahead of its block.
    #[test]
    fn the_block_is_carried_forward_by_its_age() {
        let grid = flat();
        let (mut v, specs) = car();
        v.position = Vec3::new(0.0, 0.5, 0.0);
        let b = block(92, Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 16.0), 0.0);
        v.net_dead_reckon(&b, 100, &grid, &specs, None);
        // P.z = 16 · 8/64 = 2; the blend takes 0.08 of the 2 m
        assert!((v.position.z - 0.16).abs() < 1e-3, "{}", v.position.z);
    }

    /// The heading carries on by the yaw rate: `M · Ry(age · ω/64)`, in turns.
    #[test]
    fn the_heading_turns_by_the_yaw_rate_times_the_age() {
        let grid = flat();
        let (mut v, specs) = car();
        // ω = 1 turn/s, 16 ticks late → a quarter turn of heading; the blend keeps 0.8 of the old
        // orientation, so the forward row swings by ≥ the blend's 20 % of the quarter turn
        let b = block(84, Vec3::ZERO, Vec3::ZERO, 1.0);
        let before = v.orientation.row(2);
        v.net_dead_reckon(&b, 100, &grid, &specs, None);
        let after = v.orientation.row(2);
        assert!((after - before).length() > 0.1, "{after:?}");
        // Ry(a) = [[c, 0, −s], [0, 1, 0], [s, 0, c]]: a quarter turn puts forward (0,0,1) on (+1, 0, 0)
        let m = mul(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], &rot_y(0.25));
        assert!((m[2][0] - 1.0).abs() < 1e-6 && m[2][2].abs() < 1e-6);
    }

    /// A tracker copy off every patch walks the point toward the slice centre until it is on one, then
    /// two more unit steps (0x4A0CC0…0x4A0D23).
    #[test]
    fn a_point_off_the_patches_walks_toward_the_slice_centre() {
        let grid = flat();
        let (mut v, specs) = car();
        v.position = Vec3::new(0.0, 0.5, 0.0);
        v.tracker.track(&grid, v.position);
        // 600 m out on x is off the 500 m ground; the centre is the origin
        let b = block(100, Vec3::new(600.0, 0.0, 0.0), Vec3::ZERO, 0.0);
        v.net_dead_reckon(&b, 100, &grid, &specs, Some(Vec3::ZERO));
        // the walk ends within 500 − 2 m of the centre, so the gap to the car is above 20: a snap
        assert!(v.position.x < 500.0 && v.position.x > 490.0, "{}", v.position.x);
    }
}
