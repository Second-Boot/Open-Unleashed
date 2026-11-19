//! The drive model traffic and police cars actually run (`RE/notes/traffic.md` §27.2).
//!
//! §14.5 described `0x408840`, which writes the velocity vector outright, and concluded that traffic is
//! kinematic. It is not: the fork at 0x4083F8 only reaches `0x408840` when `car+0x7E != 0`, and that
//! byte is written `0` at 0x41719D, 0x49B409, 0x49B51F and 0x4A1664 and nowhere else in the image. Every
//! AI car therefore takes the other branch — a **point mass with a yaw degree of freedom**, driven by
//! two P-cascades whose gains come from `trafcfg.dat`:
//!
//! 1. `0x4090C0` picks the point the car is aiming at, either an explicit world aim point or a node on
//!    the road `round((0.7·|v| + 3) / 6)` slices ahead, offset by the lane target;
//! 2. `0x408F20` turns that into a heading error in **turns**;
//! 3. `0x409570` decides whether the car is in control;
//! 4. `0x409720` (out of control) or `0x409C70` (in control) writes a body-frame force and a yaw torque;
//! 5. `0x40A1D0` rotates them to world space and integrates `v += dt·F`, `ω += dt·T`.
//!
//! All of it is ported here and all of it is CONFIRMED: the aim point, the heading error, the
//! in-control test, both controllers and the integrator. `traffic.rs` and `police.rs` drive every
//! AI car through them.

use crate::race::SliceTable;
use glam::{Mat3, Vec3};
use nfs_formats::trafcfg::{AiPhysics, TrafficConfig};
use nfs_physics::x87::dot_x87;

/// The state of one AI-driven car the model reads and writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiCarState {
    pub position: Vec3,
    /// rows are the body's right / up / forward axes in world space (`car+0x364/0x370/0x37C`)
    pub orientation: Mat3,
    pub velocity: Vec3,
    /// turns per second, as the rest of the port stores it
    pub angular_velocity: Vec3,
    /// `car+0x08`
    pub slice: usize,
    /// `car+0xEB0`: +1 along the route, −1 against it
    pub dir: f32,
    /// `car+0xEAC`: the direction the behaviour has *asked* for. 0x409AAF gates the throttle on it
    /// matching `dir`, so a car that has been told to turn round coasts until it has.
    pub requested_dir: f32,
    /// `car+0x104C`: the gear / heading sense, +1 forward and −1 reversing out of trouble
    pub sense: f32,
    /// `car+0xEC0` signed speed along the route
    pub speed: f32,
    /// `car+0xEC4` octagonal speed magnitude
    pub speed_est: f32,
    /// `car+0xEB8` target speed
    pub target_speed: f32,
    /// `car+0xED0` lateral offset from the centreline
    pub lateral: f32,
    /// `car+0x1074` lateral target
    pub lateral_target: f32,
    /// `car+0x1064..0x106C` explicit world aim point, zero when following the road
    pub aim_point: Vec3,
    /// `car+0x1088` the yaw-acceleration clamp (2.6 from 0x406F80)
    pub max_yaw_acc: f32,
    /// `car+0x1084` the lateral-acceleration clamp (grip · 9.8 from 0x406FA2)
    pub max_lat_acc: f32,
}

/// `0x4090C0` — where the car is aiming, in world space (`car+0x102C`), read to the instruction
/// 2026-10-22 (`traffic.md` §27.2c):
///
/// * an explicit aim point `car+0x1064…` wins outright (0x4090DB…0x409127);
/// * otherwise `n = fistp(f32((|car+0xEC0| · 0.7 + 3) · ⅙))` slices (0x409148…0x40916A,
///   `[0x5B2618]`, `[0x5B2600]`, `[0x5B2420]`), raised to `min_lookahead / 6` when it is below it
///   (0x40916D…0x409199 — **sixths**, the integer `imul 0x2AAAAAAB`, not thirds) and cut to
///   `max_lookahead / 6` (0x40919B…0x4091B3);
/// * on a narrow road ahead — `lanesA·laneWidthA + lanesB·laneWidthB < 18` (`[0x5B2614]`) of the
///   slice `n` along (0x4091F8…0x40922A) — a bend `|0x40EDD0(car, step)|` past 0.95 cuts `n` to
///   `n · 0xCA3D >> 16`, and then the first of past 0.9 / 0.8 / 0.75 to `· 0xD70A / 0xE666 / 0xF333 >> 16`
///   (0x409230…0x409313);
/// * the loop (0x409329…0x4094F5, at most five passes): the point is the slice `n` along's node
///   `+0x04` plus its lateral `+0x28` (unnormalised) times `car+0x1074`; its offset across the car's
///   **own** slice (`dot(+0x28, point − node)`) past that slice's lane extent on the side away from
///   `car+0xED0` retries unless the lanes 8 m and 4 m from `car+0xED0` on that side both exist
///   (`0x40EB20` / `0x40EA00`); every pass then sets `n = max(n · 0xCCCC >> 16, 4)`.
///
/// The port's earlier version clamped to thirds (4…10 slices), so a traffic car at 15 m/s aimed 24 m
/// ahead instead of 12 (live, `REFERENCE-2026-10-22-PA-TRAFFIC3.md`).
pub fn aim_direction(table: &SliceTable, cfg: &TrafficConfig, car: &AiCarState) -> Vec3 {
    if car.aim_point != Vec3::ZERO {
        return car.aim_point - car.position;
    }
    let v = car.speed.abs();
    // the product is stored as a float (0x409161) and then rounded to nearest by `fistp`
    let x = ((f64::from(v) * f64::from(0.7f32) + 3.0) * f64::from(1.0f32 / 6.0)) as f32;
    let mut n = fistp(x);
    let (min6, max6) = (cfg.min_lookahead / 6, cfg.max_lookahead / 6);
    if n >= 0 && n < min6 {
        n = min6;
    }
    if n > max6 {
        n = max6;
    }
    let sense = (car.dir as i32) * (car.sense as i32);
    let shrink = |n: i32, k: i64| ((i64::from(n) * k) / 65536) as i32;
    let ahead = &table.slices[table.add(car.slice, sense * n)];
    let road = f64::from(ahead.lane_width_b) * f64::from(ahead.lanes_b) + f64::from(ahead.lane_width_a) * f64::from(ahead.lanes_a);
    if road < 18.0 {
        let bend = road_bend_steps(table, car.slice, sense * n).abs();
        if bend > 0.95 {
            n = shrink(n, 0xCA3D);
        }
        if bend > 0.9 {
            n = shrink(n, 0xD70A);
        } else if bend > 0.8 {
            n = shrink(n, 0xE666);
        } else if bend > 0.75 {
            n = shrink(n, 0xF333);
        }
    }
    let own = &table.slices[car.slice];
    let mut point;
    let mut pass = 0;
    loop {
        let sl = &table.slices[table.add(car.slice, sense * n)];
        point = sl.centre + sl.lateral * car.lateral_target;
        let d = point - own.centre;
        let off = f64::from(own.lateral.x) * f64::from(d.x) + f64::from(own.lateral.y) * f64::from(d.y) + f64::from(own.lateral.z) * f64::from(d.z);
        let ed0 = f64::from(car.lateral);
        let edge_b = f64::from(own.lane_width_b) * f64::from(own.lanes_b);
        let edge_a = f64::from(own.lane_width_a) * f64::from(own.lanes_a);
        let ok = if (off > edge_b && off > ed0) || (off < -edge_a && off < ed0) {
            let side = if ed0 <= off { 1.0 } else { -1.0 };
            crate::lanes::lane_exists(own, crate::lanes::lane_of(own, car.lateral + side * 8.0))
                && crate::lanes::lane_exists(own, crate::lanes::lane_of(own, car.lateral + side * 4.0))
        } else {
            true
        };
        pass += 1;
        n = shrink(n, 0xCCCC).max(4);
        if pass > 4 || ok {
            break;
        }
    }
    point - car.position
}

/// `fistp` under the x87's default rounding: to nearest, ties to even.
fn fistp(v: f32) -> i32 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 && (r as i32) % 2 != 0 { r as i32 - v.signum() as i32 } else { r as i32 }
}

/// `0x408F20` — the heading error in **turns**, in −0.5..0.5. Positive means the car points to the
/// right of where it wants to go.
pub fn heading_error(car: &AiCarState, aim: Vec3) -> f32 {
    use nfs_physics::x87::TURN_PER_RAD_IMAGE;
    let fwd = car.orientation.row(2);
    // 0x408FFD…0x40903D: `0x532860(x, z)` = `fpatan · [0x5BB2C8]` (the **double** 0.1591549515724182 = 1 / 6.28318452835083, not
    // 1/τ — PORT-DEVIATION D1: portable atan2), the heading's copy stored to `f32` (`fst [esp+0x28]`, `fstp car+0x1054`)
    // and the aim's (0x40905A) kept in the register; `e = a_f32 − b` (0x40905F `fld a; fsub st(1)`), `+1` below `−0.5`
    // (`[0x5B2448]`, 0x409068…0x409075) or `−1` above `0.5` (0x40908F…0x40909C), one `fstp car+0x1038`.
    let a = if car.sense >= 0.0 {
        nfs_math::portable::atan2(f64::from(fwd.x), f64::from(fwd.z))
    } else {
        nfs_math::portable::atan2(f64::from(-fwd.x), f64::from(-fwd.z))
    } * TURN_PER_RAD_IMAGE;
    let b = nfs_math::portable::atan2(f64::from(aim.x), f64::from(aim.z)) * TURN_PER_RAD_IMAGE;
    let mut e = f64::from(a as f32) - b;
    if e < -0.5 {
        e += 1.0;
    } else if e > 0.5 {
        e -= 1.0;
    }
    e as f32
}

/// `0x409570` — is the car out of control? `road` is the slice's A/B road extents. The speed floor
/// comes from the **out-of-control** chain's `vel_limit_range` (0x409637 reads `[0x5E6D68]`, which is
/// `0x5E6D48 + 0x20`), so with the shipped file it is 10 − 3 = 7 m/s. `bend` is [`road_bend`].
///
/// The clauses are in the binary's own order, which is also the order they short-circuit in:
/// heading error (0x4095FE), the reverse sense (0x40960F), the speed floor (0x409645), the two road
/// edges (0x409675 / 0x409699), **the bend ahead (0x4096A6)** and the lateral-speed pair
/// (0x4096DC / 0x409700).
///
/// **Not modelled, and it is shut in the original too:** 0x4095A3 puts a branch *in front* of all of
/// them — while `gGameTick < car+0x10A4` the only test is `|car+0xEC0| < 3.0` and the car is
/// otherwise in control. Both controllers pin `car+0x10A4` to the current tick every tick
/// (0x409788 unconditionally, 0x409DCC within 0x40 ticks of `car+0x45C`), so the window only opens
/// for the ten ticks-times-sixty-four a cop's bust state grants at 0x4051F7, which this port has no
/// equivalent of. `traffic.md` §17.8a.
pub fn out_of_control(car: &AiCarState, ooc: &AiPhysics, heading_err: f32, road: (f32, f32), bend: f32) -> bool {
    heading_err.abs() > 0.25
        || car.sense < 0.0
        || car.speed.abs() < ooc.vel_limit_range - 3.0
        || car.lateral < -road.0
        || car.lateral > road.1
        || bend.abs() > 0.8
        || (body_lateral_speed(car).abs() > 0.4 * car.speed_est && car.speed.abs() >= 15.0)
}

/// The road edges [`out_of_control`] tests `car+0xED0` against: `laneWidthA · lanesA` and
/// `laneWidthB · lanesB` of the car's slice (`+0x48 · +0x38`, `+0x4C · +0x3C`, `fimul`;
/// 0x40966D…0x409699) — not the slice's `+0x40` / `+0x44` widths, which the port passed until
/// 2026-10-22.
pub fn lane_extent(sl: &crate::race::Slice) -> (f32, f32) {
    (sl.lane_width_a * sl.lanes_a as f32, sl.lane_width_b * sl.lanes_b as f32)
}

/// **How sharply the road turns in the next eight slices** — `0x40EDD0(car, car+0xEB0 · 8)`, the
/// measure [`out_of_control`]'s sixth clause tests against `[0x5B24E8]` = 0.8.
///
/// It is `dot` of the **lateral** of the car's own slice with the **forward** of the slice eight
/// along its direction of travel, in **x and z only** — 0x40EDFE multiplies `+0x30` by `+0x24` and
/// `+0x28` by `+0x1C`, which are `lateral.z · forward.z + lateral.x · forward.x`. Zero on a straight
/// road and ±1 at a right-angle turn, so `> 0.8` is a bend of about 53° over the 48 m the eight
/// slices span. The slice arithmetic is `addSlices` with `link = −1` (0x40EDEA).
pub fn road_bend(table: &SliceTable, slice: usize, dir: f32) -> f32 {
    road_bend_steps(table, slice, if dir < 0.0 { -8 } else { 8 })
}

/// `0x40EDD0(car, steps)` for any step count: the car's slice's lateral `+0x28` against the forward
/// `+0x1C` of the slice `steps` along (`addSlices` with `link = −1`), in x and z.
pub fn road_bend_steps(table: &SliceTable, slice: usize, steps: i32) -> f32 {
    let ahead = table.add_graph(slice, steps, None);
    let (a, b) = (&table.slices[slice.min(table.slices.len() - 1)], &table.slices[ahead.min(table.slices.len() - 1)]);
    a.lateral.x * b.forward.x + a.lateral.z * b.forward.z
}

/// `car+0xD58` — the body-frame lateral speed.
pub fn body_lateral_speed(car: &AiCarState) -> f32 {
    // 0x408F39…0x408F3E: the dot 0x532910 (`fmul` ×3, `faddp` ×2 in the register), one `fstp [car+0xD58]`
    dot_x87(car.velocity, car.orientation.row(0))
}

/// `car+0xD60` — the body-frame forward speed.
pub fn body_forward_speed(car: &AiCarState) -> f32 {
    // 0x408F5D…0x408F64 (`car+0xD60`)
    dot_x87(car.velocity, car.orientation.row(2))
}

/// The body-frame command the controllers produce: a lateral and forward force and a yaw torque.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DriveCommand {
    pub lateral: f32,
    pub forward: f32,
    pub yaw: f32,
    /// Which longitudinal arm ran: `Some(false)` the throttle, `Some(true)` the brake, `None`
    /// neither (the out-of-control controller at a standstill) — what the brake light follows.
    pub braking: Option<bool>,
}

/// `car+0xD80`, the AI brake-light counter, inside both controllers (0x409720 / 0x409C70): the
/// brake arm adds `4 · [0x5E6D78]` saturating at 0xFF (0x409B0E…), the throttle arm takes
/// `[0x5E6D78]` off down to 0 (0x409ABD…); `[0x5E6D78]` is the **32 Hz** ticks the drive step
/// covers — `0x5CC71C[k]` from the drive's gate 0x414B10 (row 1: 1 within 160 m of a racer, 2
/// beyond; `traffic.md` §14.3). (history: `game-history.md` §5.1)
/// 0x439360 lights the brake lamps while the byte is non-zero (0x43938C…0x43939B).
pub fn brake_light(counter: u8, braking: Option<bool>, ticks: u8) -> u8 {
    match braking {
        Some(true) => counter.saturating_add(ticks.saturating_mul(4)),
        Some(false) => counter.saturating_sub(ticks),
        None => counter,
    }
}

/// `0x409720` — the out-of-control controller, in full.
pub fn out_of_control_command(car: &AiCarState, cfg: &AiPhysics, heading_err: f32, accel_rate: f32, brake_force: f32) -> DriveCommand {
    let wrong_way = car.dir * car.speed < 0.0 && car.sense >= 0.0;
    // Register chains of `0x409720` (control word 0x27F): `dav = (err · [0x5E6D58]) · −2` (0x40978E…0x40979A) is clamped to
    // `±max_dav` **in the register** (`fcom`: kept below the bound, else the bound) and stays unrounded through the rest of the
    // routine; `dlv = 10 · dav` is compared unrounded but stored to `f32` (`fst [esp+0x20]`, 0x4097D4…0x4097DA) and then clamped
    // as a stored value to `±max_dlvel` (0x4097DE…0x40980F) and `±speed_est` (0x409811…0x409847); under `vel_limit_range`
    // (0x409849…0x4098B6) `dlv` is clamped to the **unrounded** `|lat_factor · speed_est|` (the bound stored as `f32` when it
    // wins) and `dav` to the `f32` `|ang_factor · speed_est|` (0x4098B8…0x409907); `aa = −((yaw − dav) · [0x5E6D60])`
    // takes the unrounded `dav` (0x40991E…0x40992E), `la = −((lat − dlv) · [0x5E6D50])` the stored `dlv` (0x4099A2…0x4099B4).
    let up = |x: f64, m: f64| if x < m { x } else { m };
    let dn = |x: f64, m: f64| if x > -m { x } else { -m };
    let mut dav = dn(up(-2.0 * f64::from(heading_err) * f64::from(cfg.dangle_to_dav), f64::from(cfg.max_dav)), f64::from(cfg.max_dav));
    let dlv_w = 10.0 * dav;
    let mut dlv = dlv_w as f32;
    if !(dlv_w < f64::from(cfg.max_dlvel)) {
        dlv = cfg.max_dlvel;
    }
    if !(dlv > -cfg.max_dlvel) {
        dlv = -cfg.max_dlvel;
    }
    // …and then to ±`car+0xEC4`, the speed estimate. Both halves are real: 0x409811 is
    // `min(dlv, speed_est)` and **0x40982C is `max(dlv, −speed_est)`** — `traffic.md` §27.14 called
    // that second one "a compiler no-op" and it is not, because `max_dlvel` can exceed the estimate
    // and leave `dlv` below `−speed_est` for the first clamp to pass through. Corrected 2026-09-21.
    if car.speed_est <= dlv {
        dlv = car.speed_est;
    }
    if !(dlv > -car.speed_est) {
        dlv = -car.speed_est;
    }
    if car.speed.abs() < cfg.vel_limit_range {
        let l = (f64::from(cfg.lat_vel_limit_factor) * f64::from(car.speed_est)).abs();
        if !(f64::from(dlv) < l) {
            dlv = l as f32;
        }
        if !(f64::from(dlv) > -l) {
            dlv = (-l) as f32;
        }
        let a = (cfg.ang_vel_limit_factor * car.speed_est).abs();
        dav = dn(up(dav, f64::from(a)), f64::from(a));
    }
    // `car+0x10AC == 2 → dav *= 5` (0x40990F) is **not** modelled, because it cannot fire: that
    // `cmp` is the only reference to `car+0x10AC` anywhere in the image and nothing writes it, so it
    // holds the zero `Car_tObj`'s allocation left there.
    let yaw_rate = dot_x87(car.angular_velocity, car.orientation.row(1)); // 0x408F7D…0x408F8A (`car+0xD68`)
    let mut aa = (-((f64::from(yaw_rate) - dav) * f64::from(cfg.dav_to_aa))) as f32;
    if car.speed.abs() > 18.0 {
        aa *= 0.5;
    }
    aa = aa.clamp(-car.max_yaw_acc, car.max_yaw_acc);
    let la = (-((f64::from(body_lateral_speed(car)) - f64::from(dlv)) * f64::from(cfg.dlvel_to_clacc)) as f32).clamp(-car.max_lat_acc, car.max_lat_acc);
    // **Longitudinal, read out in full 2026-09-21** (0x4099F0…0x409B0E). The target:
    //
    // * reversing (`car+0x104C == −1`) with **no aim point at all** — all three of `car+0x1064…+0x106C`
    //   exactly zero, 0x409A09/0x409A1C/0x409A2F — asks for `−vel_limit_range` (0x409A42,
    //   `[0x5B2624]` = −1); the port had no such arm;
    // * otherwise `sense · target_speed · dir` (0x409A50).
    //
    // And the window is a **±2 m/s dead band stretched to the target**, not a fixed one:
    // `want >= 0` accelerates while `−2 <= v <= want` (0x409A7D / 0x409A8C) and `want < 0` while
    // `want <= v <= 2` (0x409A99 / 0x409AA2), both bounds inclusive. The port had
    // `v > −2 && v < max(want, 2)`, which for a small positive target keeps the throttle on past it
    // and for a negative one cuts it at −2 instead of following the target down. `want == 0` takes
    // the first arm in the original, so the port's extra `want != 0` test was its own.
    let want = if car.sense < 0.0 && car.aim_point == Vec3::ZERO { -cfg.vel_limit_range } else { car.sense * car.target_speed * car.dir };
    let v = body_forward_speed(car);
    let in_band = if want >= 0.0 { (-2.0..=want).contains(&v) } else { (want..=2.0).contains(&v) };
    // `car+0x544 == 0` (0x409A5E) is the post-finish forced-brake state, which no AI car this port
    // drives ever has; `car+0xEAC == car+0xEB0` (0x409AAF) is the gate below.
    let (mut fwd, braking) = if in_band && car.requested_dir == car.dir {
        (accel_rate * car.sense, Some(false))
    } else if v.abs() > 0.0 {
        (-v.signum() * brake_force, Some(true))
    } else {
        (0.0, None)
    };
    let mut la = la;
    let mut aa = aa;
    if wrong_way {
        let k = if car.speed_est > 20.0 && car.speed_est < 50.0 { 0.25 } else { 0.75 };
        fwd *= k;
        aa *= k;
        la *= 0.375;
    }
    DriveCommand { lateral: la, forward: fwd, yaw: aa, braking }
}

/// The gear / heading sense an AI car carries (`car+0x104C`, `+0x1048`, `+0x1050`, `+0x1080`), and
/// the "back out of trouble" oscillator that drives it (`traffic.md` §27.2a).
///
/// `car+0x104C` is **the gear**: `+1` forward, `−1` reversing. It starts at `+1` (0x4049A8,
/// 0x406CF9, 0x406DAC) and only the routines here move it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gear {
    /// `car+0x104C`
    pub sense: f32,
    /// `car+0x1050` — the tick the current gear started at
    pub since: i64,
    /// `car+0x1048` — the period the flip is timed against
    pub period: i64,
    /// `car+0x1080` — 32 Hz ticks spent reversing (`+= [0x5E6D78]`), reset whenever the gear is forward
    pub reverse_ticks: i32,
}

impl Default for Gear {
    fn default() -> Self {
        Self { sense: 1.0, since: 0, period: 0, reverse_ticks: 0 }
    }
}

/// `0x40A5A0(car)` — "is this car in trouble?", the gate on the oscillator.
///
/// True when the car has left the slice's lane extent **and** either it has been reversing for more
/// than eight 32 Hz ticks — `car+0x1080 += [0x5E6D78]` (0x40A5E7), the drive step's `nTicks`, so a
/// quarter of a second — or its up axis has tipped past 0.85 (`car+0x3C8`, the y of the frame's up
/// row). An upright car that merely wandered off the lanes is *not* in trouble, so the oscillator
/// only ever starts on a car that is tipped over.
pub fn in_trouble(gear: &mut Gear, lane: i32, lanes_a: i32, lanes_b: i32, up_y: f32, ai_ticks: i32) -> bool {
    if (7 - lanes_a..=lanes_b + 6).contains(&lane) {
        return false;
    }
    if gear.sense < 0.0 {
        gear.reverse_ticks += ai_ticks;
    } else {
        gear.reverse_ticks = 0;
    }
    gear.reverse_ticks > 8 || up_y <= 0.85
}

/// `0x40A850(car, period)` — flip the gear every `period / 2` ticks, the forward → reverse flip only
/// while the car is below 20 m/s along the road. `AI_drive` 0x4086A0 passes **0x40**, so the flip is
/// every 32 ticks.
pub const GEAR_PERIOD: i64 = 0x40;

pub fn toggle_gear(gear: &mut Gear, now: i64, road_speed: f32, period: i64) {
    let elapsed = now - gear.since;
    let flip = if gear.sense < 0.0 {
        elapsed > gear.period / 2
    } else {
        elapsed > gear.period / 2 && road_speed.abs() < 20.0
    };
    if flip {
        gear.sense = -gear.sense;
        gear.since = now;
        gear.period = period;
    }
}

/// `0x408E30(car)` — the trim applied while the gear is reversed, before the drive chain.
///
/// Returns the target speed to use (clamped to ±20) and clears the lane target, since a reversing
/// car is not trying to hold a lane. The timer is pushed back to `now − 24` whenever the lane target
/// is more than ten metres away on the side the lateral velocity is already going, which keeps the
/// car in reverse for longer when it is still heading away from where it wants to be.
pub fn reverse_trim(gear: &mut Gear, now: i64, lateral: f32, lateral_target: f32, lateral_vel: f32, target_speed: f32) -> (f32, f32) {
    if gear.sense >= 0.0 {
        return (target_speed, lateral_target);
    }
    if (lateral_target - 10.0 > lateral && lateral_vel > 0.0) || (lateral_target + 10.0 < lateral && lateral_vel < 0.0) {
        gear.since = now - 24;
    }
    if now - gear.since > gear.period {
        gear.sense = 1.0;
        gear.since = now;
    }
    (target_speed.clamp(-20.0, 20.0), 0.0)
}

/// `(1 + 0.0015 · car+0xD60) · car+0x10BC` of `0x409C70` (0x409D1A…0x409D32, `[0x5B2634]` the `f32` 0.0015): one register chain,
/// one `fstp`; `car+0x10BC` is 1.0 here (see [`in_control_command`]).
pub fn grip_factor(car: &AiCarState) -> f32 {
    (f64::from(body_forward_speed(car)) * f64::from(0.0015f32) + 1.0) as f32
}

/// `0x409C70` — the in-control controller (§27.14).
///
/// The surprise here is that the lateral half is **not** a lane-position controller. The lane error
/// `car+0xED0 − car+0x1074` is computed and then discarded (the value never leaves the FPU stack), and
/// the heading error does not enter the lateral force at all. What the controller actually does is damp
/// the velocity component **perpendicular to the desired direction**, so the lane is held only through
/// the aim point: `car+0x1074` moves the point `0x4090C0` aims at, and this kills whatever velocity is
/// not pointing at it. `dlpos_to_dlvel`, `max_dlvel` and `lat_vel_limit_factor` therefore have no
/// effect on an in-control car.
///
/// `grip` is `(1 + 0.0015·forwardSpeed) · car+0x10BC`, the post-collision ramp; `boost` is
/// `car+0x108C` when it exceeds 1.
pub fn in_control_command(car: &AiCarState, cfg: &AiPhysics, heading_err: f32, aim: Vec3, grip: f32, boost: f32, accel_rate: f32, brake_force: f32) -> DriveCommand {
    // 0x409C93…0x409CE5: `car+0x102C` normalised by 0x5328D0 (the reciprocal in the register, one rounding per component), the
    // perpendicular `(z, y, −x)` and `velocity · perp` through 0x532910, `· dir` (`fimul car+0xEB0`) stored to `f32` (0x409D06)
    let d = nfs_physics::x87::normalize_x87(aim);
    // the desired direction rotated a quarter turn in the ground plane
    let perp = Vec3::new(d.z, d.y, -d.x);
    let k = if boost > 1.0 { f64::from(boost) } else { 1.0 };
    // `grip` is §27.14's `gripK` **less its second factor**: the original is
    // `(1 + 0.0015·car+0xD60) · car+0x10BC`, and `car+0x10BC` is the post-collision recovery ramp
    // 0x40A900 winds down, which this port does not model — it is 1.0 for an undamaged car, and the
    // cases that move it ((ii) the scripted spin, (iii) the cop-collision window) are not ported
    // either. 0x409D1A…0x409D32 is one chain with one `fstp` ([`grip_factor`]).
    // 0x409E81…0x409EB2: `a = (grip · car+0x1084) · boost`, `b = ([0x5E6D24] · grip) · boost` stay in the register.
    let (a, b) = (f64::from(grip) * f64::from(car.max_lat_acc) * k, f64::from(cfg.dlvel_to_clacc) * f64::from(grip) * k);
    // 0x40A032…0x40A06C: `(−dir) · (dot · dir)` is `−dot_f32` (`dir` = ±1), `· b` rounded once, then `a` (unrounded) as the bound:
    // kept below it, otherwise `a` stored as `f32`; and the same against `−a`
    let dot = f64::from(dot_x87(car.velocity, perp));
    let mut lateral = (-dot * b) as f32;
    if !(f64::from(lateral) < a) {
        lateral = a as f32;
    }
    if !(f64::from(lateral) > -a) {
        lateral = (-a) as f32;
    }

    // 0x409EB8…0x40A002 as in the out-of-control controller: `dav` stays unrounded in the register through `±max_dav` and
    // (inside `vel_limit_range`) `±|ang_factor · speed_est|` (the `f32` product), and `aa = −((yaw − dav) · [0x5E6D34])`
    let up = |x: f64, m: f64| if x < m { x } else { m };
    let dn = |x: f64, m: f64| if x > -m { x } else { -m };
    let mut dav = dn(up(-2.0 * f64::from(heading_err) * f64::from(cfg.dangle_to_dav), f64::from(cfg.max_dav)), f64::from(cfg.max_dav));
    if car.speed.abs() < cfg.vel_limit_range {
        let l = (cfg.ang_vel_limit_factor * car.speed_est).abs();
        dav = dn(up(dav, f64::from(l)), f64::from(l));
    }
    let yaw_rate = dot_x87(car.angular_velocity, car.orientation.row(1)); // 0x408F7D…0x408F8A (`car+0xD68`)
    let yaw = ((-((f64::from(yaw_rate) - dav) * f64::from(cfg.dav_to_aa))) as f32).clamp(-car.max_yaw_acc, car.max_yaw_acc);

    // the throttle window is a plain comparison of the route-signed speeds, with no dead band and no
    // gear factor (`traffic.md` §27.14 (x)) — but it carries the same two gates the out-of-control
    // one does, and the port had neither: `car+0x544 == 0` (0x40A093), the post-finish forced brake
    // no AI car this port drives ever has, and `car+0xEAC == car+0xEB0` (0x40A09B), so a car asked to
    // turn round coasts until it has. The brake, like the drive force, acts along the car's own
    // forward axis, so its sign comes from the body-frame speed and not from the route-signed one.
    let (forward, braking) = if car.dir * car.speed < car.dir * car.target_speed && car.requested_dir == car.dir {
        (accel_rate, false)
    } else {
        (-body_forward_speed(car).signum() * brake_force, true)
    };
    DriveCommand { lateral, forward, yaw, braking: Some(braking) }
}

/// `0x40A1D0` — rotate the body-frame command into world space and integrate it. `dt` is the AI dt.
///
/// `v += dt · Mᵀ·car+0xD4C` (+ `car+0xD70` in the `car+0x3B8` basis, zero from both controllers;
/// 0x40A201…0x40A349) and `ω += dt · Mᵀ·car+0xD34` (0x40A2AD…0x40A3C0); then the walking-pace yaw
/// damping on the **world** `ω.y` `car+0x38C`, keyed on `car+0xD60` — the body-frame forward speed
/// 0x408F20 stored before the controllers ran, not the integrated one: `|D60| < 1` (`[0x5B24A8]`)
/// zeroes it (0x40A3EE; 0x532970 then renormalises the forward row, a unit vector already), otherwise
/// it is scaled by `min(0.15 · |D60|, 1)` (`[0x5B2638]`, 0x40A437…0x40A459). The port damped the
/// component along the body's up axis by the post-step speed until 2026-10-22.
pub fn integrate(car: &mut AiCarState, cmd: &DriveCommand, dt: f32) {
    let m = car.orientation;
    // `car+0xD60`, written by 0x408F20 from the velocity the controllers saw
    let d60 = body_forward_speed(car);
    // 0x40A20F…0x40A258: `car+0xD40…D48` = three dots 0x532910 of `car+0xD4C` (lateral, 0, forward) with the transposed rows (one
    // rounding each); 0x40A301…0x40A349: `v += dt · that` is `fld dt; fmul; fadd v; fstp`, one rounding (control word 0x27F).
    // 0x40A2AD…0x40A3C0 the same for `ω += dt · Mᵀ·car+0xD34` with the yaw command in `car+0xD38`.
    let local = Vec3::new(cmd.lateral, 0.0, cmd.forward);
    let ang = Vec3::new(0.0, cmd.yaw, 0.0);
    let step = |v: f32, a: Vec3, c: Vec3| (f64::from(dt) * f64::from(dot_x87(a, c)) + f64::from(v)) as f32;
    car.velocity = Vec3::new(step(car.velocity.x, local, m.col(0)), step(car.velocity.y, local, m.col(1)), step(car.velocity.z, local, m.col(2)));
    car.angular_velocity = Vec3::new(step(car.angular_velocity.x, ang, m.col(0)), step(car.angular_velocity.y, ang, m.col(1)), step(car.angular_velocity.z, ang, m.col(2)));
    if d60.abs() < 1.0 {
        car.angular_velocity.y = 0.0;
    } else {
        car.angular_velocity.y *= (0.15 * d60.abs()).min(1.0);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brake_light_counter() {
        // a 16 Hz drive step covers two 32 Hz ticks: +8 braking, saturating; −2 on the throttle,
        // down to 0
        assert_eq!(brake_light(0, Some(true), 2), 8);
        assert_eq!(brake_light(250, Some(true), 2), 255);
        assert_eq!(brake_light(8, Some(false), 2), 6);
        assert_eq!(brake_light(1, Some(false), 2), 0);
        assert_eq!(brake_light(5, None, 2), 5);
    }
    use crate::race::SLICE_SPACING;
    use crate::test_support::Road;

    fn straight(n: usize) -> SliceTable {
        crate::test_support::straight_of(n, Road::two_lanes(11.0))
    }

    fn car() -> AiCarState {
        AiCarState {
            position: Vec3::ZERO,
            orientation: Mat3::IDENTITY,
            velocity: Vec3::new(0.0, 0.0, 16.0),
            angular_velocity: Vec3::ZERO,
            slice: 10,
            dir: 1.0,
            requested_dir: 1.0,
            sense: 1.0,
            speed: 16.0,
            speed_est: 16.0,
            target_speed: 16.5,
            lateral: 0.0,
            lateral_target: 0.0,
            aim_point: Vec3::ZERO,
            max_yaw_acc: 2.6,
            max_lat_acc: 9.8,
        }
    }

    #[test]
    fn the_aim_point_walks_the_road_and_an_explicit_one_overrides_it() {
        let t = straight(60);
        let cfg = TrafficConfig::default();
        let mut c = car();
        c.position = t.slices[10].centre;
        // at 16 m/s the lookahead is fistp((0.7·16 + 3)/6) = 2 slices, inside the 2..5 clamp
        // (`min_lookahead / 6` … `max_lookahead / 6`, 0x40916D…0x4091B3)
        let aim = aim_direction(&t, &cfg, &c);
        assert!(aim.z > 0.0 && aim.x.abs() < 1e-3, "{aim:?}");
        assert!((aim.length() - 2.0 * SLICE_SPACING).abs() < 1e-3, "two slices ahead: {}", aim.length());
        // at 30 m/s: fistp(4.0) = 4; at 60 m/s 7.5 → 8, cut to 5
        c.speed = 30.0;
        assert!((aim_direction(&t, &cfg, &c).length() - 4.0 * SLICE_SPACING).abs() < 1e-3);
        c.speed = 60.0;
        assert!((aim_direction(&t, &cfg, &c).length() - 5.0 * SLICE_SPACING).abs() < 1e-3);
        c.speed = 16.0;
        // a lane target moves it sideways
        c.lateral_target = 3.0;
        assert!((aim_direction(&t, &cfg, &c).x - 3.0).abs() < 1e-3);
        // an explicit aim point wins outright
        c.aim_point = Vec3::new(100.0, 0.0, 100.0);
        assert_eq!(aim_direction(&t, &cfg, &c), Vec3::new(100.0, 0.0, 100.0) - c.position);
    }

    #[test]
    fn the_heading_error_is_in_turns_and_signed() {
        let c = car();
        // aiming straight ahead is no error
        assert!(heading_error(&c, Vec3::Z).abs() < 1e-6);
        // aiming to the car's right is a negative error (the car must turn right to match)
        let e = heading_error(&c, Vec3::new(1.0, 0.0, 1.0));
        assert!((e + 0.125).abs() < 1e-5, "eighth of a turn: {e}");
        // and it wraps rather than running to ±1
        let back = heading_error(&c, Vec3::new(0.0, 0.0, -1.0));
        assert!(back.abs() <= 0.5 + 1e-6);
    }

    #[test]
    fn control_is_lost_by_heading_speed_or_leaving_the_road() {
        // the test reads the out-of-control chain's vel_limit_range, so the floor is 10 − 3 = 7 m/s
        let cfg = TrafficConfig::default().out_of_control;
        let c = car();
        assert!(!out_of_control(&c, &cfg, 0.1, (11.0, 11.0), 0.0));
        assert!(out_of_control(&c, &cfg, 0.3, (11.0, 11.0), 0.0), "a quarter turn off is out of control");
        let mut slow = c;
        slow.speed = 7.5;
        assert!(!out_of_control(&slow, &cfg, 0.0, (11.0, 11.0), 0.0));
        slow.speed = 6.5;
        assert!(out_of_control(&slow, &cfg, 0.0, (11.0, 11.0), 0.0), "below vel_limit_range − 3");
        let mut off = c;
        off.lateral = 12.0;
        assert!(out_of_control(&off, &cfg, 0.0, (11.0, 11.0), 0.0), "off the road");
        let mut sliding = c;
        sliding.velocity = Vec3::new(8.0, 0.0, 16.0);
        assert!(out_of_control(&sliding, &cfg, 0.0, (11.0, 11.0), 0.0), "sliding sideways at speed");
        // the sixth clause, 0x4096A6 against `[0x5B24E8]` = 0.8: a bend of more than about 53° in the
        // next eight slices, and the sign does not matter
        assert!(!out_of_control(&c, &cfg, 0.0, (11.0, 11.0), 0.79));
        assert!(out_of_control(&c, &cfg, 0.0, (11.0, 11.0), 0.81), "a sharp bend ahead");
        assert!(out_of_control(&c, &cfg, 0.0, (11.0, 11.0), -0.81), "either way round");
    }

    /// The out-of-control throttle window, 0x409A6A…0x409AA2: a ±2 m/s dead band **stretched to the
    /// target**, both bounds inclusive, and a target of 0 still accelerates a car that is rolling
    /// backwards.
    #[test]
    fn the_throttle_window_follows_the_target_past_the_dead_band() {
        let cfg = TrafficConfig::default().out_of_control;
        let mut c = car();
        c.target_speed = 1.0; // want = +1, so the window is −2 ..= 1
        let fwd = |v: f32, c: &AiCarState| {
            let mut c = *c;
            c.velocity = Vec3::new(0.0, 0.0, v);
            out_of_control_command(&c, &cfg, 0.0, 3.0, 9.0).forward
        };
        assert_eq!(fwd(0.5, &c), 3.0, "inside the window");
        assert!(fwd(1.5, &c) < 0.0, "past a small target it brakes, where the old bound was 2.0");
        assert_eq!(fwd(-1.5, &c), 3.0, "and the dead band still reaches −2");
        c.target_speed = 0.0;
        assert_eq!(fwd(-1.0, &c), 3.0, "a zero target is the `want >= 0` arm, not a special case");
        // reversing with no aim point asks for −vel_limit_range, so the window is that ..= 2
        let mut back = car();
        back.sense = -1.0;
        back.aim_point = Vec3::ZERO;
        assert_eq!(fwd(-8.0, &back), -3.0, "well inside the reverse window, and the sense flips the sign");
        assert!(fwd(-12.0, &back).abs() > 3.0, "past −vel_limit_range it brakes");
        // the gate of 0x409AAF: a car that has been asked to turn round coasts until it has
        let mut turning = c;
        turning.requested_dir = -1.0;
        assert!(fwd(0.5, &turning) < 0.0, "no throttle while the requested direction differs");
    }

    /// §27.2a: an upright car that has merely wandered off the lanes is not in trouble; a tipped one
    /// is, and then the gear flips every half period while the car is slow.
    #[test]
    fn the_gear_only_oscillates_for_a_car_that_is_actually_stuck() {
        let mut g = Gear::default();
        // inside the lane extent: never
        assert!(!in_trouble(&mut g, 7, 2, 2, 1.0, 2));
        // outside it but upright: still not
        assert!(!in_trouble(&mut g, 20, 2, 2, 1.0, 2));
        // outside it and tipped: yes
        assert!(in_trouble(&mut g, 20, 2, 2, 0.4, 2));

        // the first flip needs half a period to pass
        g = Gear { sense: 1.0, since: 0, period: GEAR_PERIOD, reverse_ticks: 0 };
        toggle_gear(&mut g, 30, 5.0, GEAR_PERIOD);
        assert_eq!(g.sense, 1.0, "32 ticks have not passed");
        toggle_gear(&mut g, 40, 5.0, GEAR_PERIOD);
        assert_eq!(g.sense, -1.0);
        assert_eq!(g.since, 40);
        // and a fast car never drops into reverse
        let mut f = Gear { sense: 1.0, since: 0, period: GEAR_PERIOD, reverse_ticks: 0 };
        toggle_gear(&mut f, 400, 25.0, GEAR_PERIOD);
        assert_eq!(f.sense, 1.0);

        // while reversing the target speed is clamped and the lane target dropped
        let mut r = Gear { sense: -1.0, since: 100, period: GEAR_PERIOD, reverse_ticks: 0 };
        let (v, t) = reverse_trim(&mut r, 110, 0.0, 0.0, 0.0, 30.0);
        assert_eq!((v, t), (20.0, 0.0));
        // a target ten metres away on the side the car is already sliding pushes the timer back
        let mut r2 = Gear { sense: -1.0, since: 100, period: GEAR_PERIOD, reverse_ticks: 0 };
        reverse_trim(&mut r2, 200, 0.0, 20.0, 1.0, 5.0);
        assert_eq!(r2.since, 176);
        // and past the full period it goes back into forward
        let mut r3 = Gear { sense: -1.0, since: 100, period: GEAR_PERIOD, reverse_ticks: 0 };
        reverse_trim(&mut r3, 200, 0.0, 0.0, 0.0, 5.0);
        assert_eq!(r3.sense, 1.0);
    }

    #[test]
    fn the_out_of_control_controller_steers_toward_the_aim() {
        let cfg = TrafficConfig::default().out_of_control;
        let c = car();
        // pointing left of the aim: the yaw torque pushes the nose back
        let right = out_of_control_command(&c, &cfg, -0.1, 0.25, 8.0);
        let left = out_of_control_command(&c, &cfg, 0.1, 0.25, 8.0);
        assert!(right.yaw * left.yaw < 0.0, "the torque reverses with the error");
        assert!(right.yaw.abs() <= c.max_yaw_acc && left.yaw.abs() <= c.max_yaw_acc);
        // below the target speed the car accelerates, above it the car brakes
        assert!(out_of_control_command(&c, &cfg, 0.0, 0.25, 8.0).forward > 0.0);
        let mut fast = c;
        fast.velocity = Vec3::new(0.0, 0.0, 40.0);
        fast.speed = 40.0;
        assert!(out_of_control_command(&fast, &cfg, 0.0, 0.25, 8.0).forward < 0.0);
    }

    #[test]
    fn the_in_control_controller_damps_sideways_velocity() {
        let cfg = TrafficConfig::default().in_control;
        let mut c = car();
        // aiming straight down the road with a pure forward velocity: nothing to damp
        let straight_cmd = in_control_command(&c, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0);
        assert!(straight_cmd.lateral.abs() < 1e-5, "{straight_cmd:?}");
        // sliding to the car's right: the force pushes back to the left, and is clamped
        c.velocity = Vec3::new(5.0, 0.0, 16.0);
        let sliding = in_control_command(&c, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0);
        assert!(sliding.lateral < 0.0, "{sliding:?}");
        assert!(sliding.lateral >= -c.max_lat_acc);
        c.velocity = Vec3::new(-5.0, 0.0, 16.0);
        assert!(in_control_command(&c, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).lateral > 0.0);
        // the lane target does not enter the lateral force at all — only the aim point does
        let mut off_lane = car();
        off_lane.lateral = 4.0;
        off_lane.lateral_target = -4.0;
        assert_eq!(in_control_command(&off_lane, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).lateral, straight_cmd.lateral);
        // below the target speed it accelerates, above it it brakes
        assert_eq!(in_control_command(&car(), &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).forward, 0.25);
        let mut fast = car();
        fast.speed = 20.0;
        assert_eq!(in_control_command(&fast, &cfg, 0.0, Vec3::Z, 1.0, 1.0, 0.25, 8.0).forward, -8.0);
    }

    #[test]
    fn the_integrator_moves_velocity_and_spin_by_dt() {
        let mut c = car();
        let cmd = DriveCommand { lateral: 2.0, forward: 1.0, yaw: 0.5, braking: None };
        let v0 = c.velocity;
        integrate(&mut c, &cmd, 1.0 / 32.0);
        assert!((c.velocity.x - (v0.x + 2.0 / 32.0)).abs() < 1e-6);
        assert!((c.velocity.z - (v0.z + 1.0 / 32.0)).abs() < 1e-6);
        // at 16 m/s the yaw damping factor is 1.0, so the whole torque lands
        assert!((c.angular_velocity.y - 0.5 / 32.0).abs() < 1e-6);
        // at walking pace the yaw rate is zeroed instead
        let mut slow = car();
        slow.velocity = Vec3::new(0.0, 0.0, 0.5);
        slow.angular_velocity = Vec3::new(0.0, 0.3, 0.0);
        integrate(&mut slow, &cmd, 1.0 / 32.0);
        assert!(slow.angular_velocity.y.abs() < 1e-6);
    }
}
