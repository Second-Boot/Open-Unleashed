//! The tyre block of `Car_physicsStep` (0x49F160): the wheels' demands from the drive split (0x49F7E4…0x49F8E1),
//! `Tyre` 0x4A1C70…0x4A26C0 (with `Slide` 0x4A1C70, `FrictionCircle` 0x4A1DC0, the lateral curves) on each axle in
//! contact, and the tyre-force sums into the body frame (0x49F941…0x49F9E3). A move out of `vehicle.rs`: no operation,
//! operand order or rounding point changed.

use super::*;

impl Vehicle {
    /// Slip velocities are ×(−16) in the steered-wheel frame; forces are m/s² of car acceleration.
    /// Not modelled: the `.ABS` arm (0x4A2308 / 0x4A23CC — `carinfo+0x24` is 0 in every race,
    /// `physics.md` §5.4). `traction_control` stands for the AI-driven flag `car+0xDA8`, `tcs` for
    /// `.TRACTION` (`carinfo+0x20`), `rally` for `RALLY` (`[0x6573F8]`).
    pub(super) fn tyre(&mut self, i: usize, specs: &CarSpecs, tyre_damage: f64) {
        let front = i < 2;
        let ai = self.traction_control;
        let rally = self.rally;
        let tcs = self.tcs;
        let slip_tangent = self.slip_tangent;
        let rear_spin = self.rear_spin;
        let handbrake = self.handbrake;
        let vz = self.v_local.z;
        let throttle = self.throttle;
        let throttle_byte = self.throttle_byte;
        let brake_byte = self.brake_byte;
        let gear = self.gear;
        let class = specs.class;
        let k_class = CIRCLE_SOFTNESS[class];
        let w = &mut self.wheels[i];
        let ang = steer_angle_256(specs.max_steer, w.steer);
        let (mut u, mut v) = (w.slip_vel[0], w.slip_vel[2]);
        if ang != 0.0 {
            // 0x4A2159 / 0x4A2163 with the image's 2π double [0x5BB2C0] (x87.rs); the pair is rotated in place
            let (nu, nv) = rotate_turns(ang / 256.0, u, v);
            u = nu;
            v = nv;
            // The rotation is **in place** (0x4A2159…0x4A219x store back into `wheel+0x9C` / `+0xA4`): a steered
            // wheel keeps the slip velocity in the steered frame until the next `Car_physicsStep` recomputes it
            // (PA-TRACE item 7; the original's `su` / `sv` columns of the front wheels in s3).
            w.slip_vel[0] = u;
            w.slip_vel[2] = v;
        }
        w.slip = 0.0;
        w.locked = false;
        w.lateral = 0.0;
        w.longitudinal = 0.0;
        // §2.2 demand gate: only a demand opposing the wheel motion is limited to the slip speed
        let mut d = w.demand;
        let mut opposing = false;
        if d < 0.0 && v < 0.0 {
            if throttle <= 0.25 || gear != 0 {
                d = d.max(v);
                opposing = true;
            }
        } else if d > 0.0 && v > 0.0 && (throttle <= 0.25 || gear <= 1) {
            d = d.min(v);
            opposing = true;
        }
        d *= specs.grip;
        // `Tyre` itself works on the **raw** `wheel+0xAC`, which 0x4A20DF copies to a local before
        // anything else: the over-limit test (0x4A22A7), the lock speed `2·(16 − LIM0)`
        // (0x4A2313), the clamp to `±LIM0` (0x4A238E…0x4A23C2) and both lateral curves
        // (`fmul [esp+0x18]`, 0x4A2512 / 0x4A262C) read it. Tyre damage (`0x493980`,
        // `1 − 0.005·d`) enters only `FrictionCircle` (0x4A1E62…0x4A1E84) and `Slide`
        // (0x4A1C77…0x4A1C95). *(Corrected 2026-09-26: the port used the damaged limit throughout.)*
        let lim0 = w.limit;
        // 0x4A1E62…0x4A1E84: `(1 − 0.005·damage) · wheel+0xAC` is one register chain and one `fstp [esp+0x14]`
        let limit = ((1.0 - tyre_damage * f64::from(0.005f32)) * f64::from(lim0)) as f32;
        // §2.3 over-limit decision: lock (Slide) or clamp, for braking-type demands / handbrake only
        if (opposing && d.abs() > lim0) || (handbrake && !front && vz.abs() > 0.5) {
            let lock = if handbrake {
                true
            } else if 2.0 * (16.0 - lim0) < vz || brake_byte < 0xEC || (vz.abs() < 5.0 && !rear_spin) {
                false
            } else if !ai {
                true
            } else {
                // 0x4A2373: the whole word against 1
                w.zone <= 1 && class != 2
            };
            if lock {
                // §4 Slide: kinetic friction opposing the slip velocity, no back-rotation (original)
                let (a, b) = (u.abs(), v.abs());
                // Register chains of `Slide` 0x4A1C70 (control word 0x27F): `comb = max + ¼·min` stays in `st0`
                // (0x4A1CD3…0x4A1CEE) with only a copy stored (`fst [esp+0xC]`, 0x4A1CF5); `lim2 = limit − limit/k`
                // is one chain, one `fstp` (0x4A1CFF…0x4A1D0E); the slip `((comb − limit) + 3·old)·¼` takes the
                // unrounded `comb` (0x4A1D1D…0x4A1D37; `old` is the 0 set above); `|comb| > 0.01` tests the register
                // (0x4A1D70…0x4A1D7F) and `s = lim2 / comb` divides by the **stored** `f32` comb (0x4A1D92); `s` stays
                // unrounded into `Flat = s·U`, `Flon = s·V` (0x4A1D96…0x4A1DAA), one `fstp` each.
                let (aw, bw) = (f64::from(a), f64::from(b));
                let comb_wide = if aw > bw { 0.25 * bw + aw } else { bw + 0.25 * aw };
                let comb = comb_wide as f32;
                let lim2 = (f64::from(limit) - f64::from(limit) / f64::from(k_class)) as f32;
                w.slip = if comb_wide > f64::from(limit) { ((comb_wide - f64::from(limit)) * 0.25) as f32 } else { 0.0 };
                w.slip = (0.1 * w.slip).min(1.0);
                // 0x4A1D70…0x4A1D96: `s = LIM2 / comb` above 0.01, and at or below it the `|U|`
                // still on the stack; then `Flat = s·U`, `Flon = s·V` (0x4A1D96…0x4A1DAA) with no
                // clamp. *(Corrected 2026-09-26: the port took `s = 0` and clamped each component
                // to twice its slip speed, a "safety net" with no counterpart in the binary.)*
                let s = if comb_wide > f64::from(0.01f32) { f64::from(lim2) / f64::from(comb) } else { aw };
                w.lateral = (s * f64::from(u)) as f32;
                w.longitudinal = (s * f64::from(v)) as f32;
                w.locked = true;
                w.sliding = true;
                w.demand = d;
                return;
            }
            d = tyre_demand_clamp(d, lim0);
        }
        w.demand = d;
        // §2.4 lateral force from the slip angle (1/256 turn). Extended precision (control word 0x27F, 53-bit mantissa), read at
        // 0x4A2441…0x4A26C0: `0x532860` is `fpatan · [0x5BB2C8]` (the **double** 0.1591549515724182 = 1 / 6.28318452835083, not
        // 1/τ), the branch arithmetic `(±0.5 − a) · 256` stays in the register and only `fst [esp+0x10]` stores a rounded copy.
        let lat_wide: f64 = if v == 0.0 {
            0.0
        } else {
            // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable atan2, not std; original x87 `fpatan` in 0x532860.
            let a = nfs_math::portable::atan2(f64::from(u), f64::from(-0.5 * v)) * TURN_PER_RAD_IMAGE;
            256.0 * if v < 0.0 { a } else if u > 0.0 { 0.5 - a } else if u < 0.0 { -0.5 - a } else { a }
        };
        let lat = lat_wide as f32;
        // 0x4A24E8 branches on `wheel+0xC0`, which is **not** a sliding flag: 0x410BB2 (car init)
        // and 0x49E345 (reset) set it to 1 for wheels 0 and 1 and 0 for wheels 2 and 3 and nothing
        // in the image ever writes it again, so it selects the lateral curve **by axle**. The front
        // pair follows `min(|lat|, 16)·load·(1/12)` (`[0x5B49B0]`, a multiply by the `f32` 1/12), the rear
        // `min(|lat|, 2)·load·0.5` with a floor of one unit — a deliberate understeer bias of about six to one at
        // small slip (`physics.md` §5.2). The limit `F` stays in the register (the front's `|lat|` too, unrounded; the rear's
        // `|lat|` is stored to `f32` first, 0x4A25FA) and `lateral = clamp(u, ±|F|)` is stored once (0x4A25BB, 0x4A25E8,
        // 0x4A2684, 0x4A26B6).
        let clamp_to = |f: f64| -> f32 {
            let ff = f.abs();
            if u > 0.0 {
                if f64::from(u) > ff { ff as f32 } else { u }
            } else if f64::from(u) <= -ff {
                (-ff) as f32
            } else {
                u
            }
        };
        if front {
            let m = lat_wide.abs().min(16.0);
            let mag = (m * f64::from(lim0)) * f64::from(ONE_TWELFTH);
            let f = if lat < 0.0 { -mag } else { mag };
            w.lateral = if f64::from(u.abs()) + f64::from(v.abs()) < 32.0 { clamp_to(f) } else { f as f32 };
        } else {
            let la = lat_wide.abs() as f32;
            let m = if la <= 1.0 { 1.0 } else { la.min(2.0) };
            let mag = (f64::from(m) * f64::from(lim0)) * 0.5;
            let f = if lat < 0.0 { -mag } else { mag };
            w.lateral = clamp_to(f);
        }
        w.longitudinal = d;
        // §3 friction circle (`FrictionCircle` 0x4A1DC0). 0x4A1E14…0x4A1E5C: `|lon| · 0.9` (`[0x5B25E8]`, the `f32`; 0.5 under RALLY)
        // stays in the register, `comb = max + 0.25·min` is one chain, one `fstp [esp+0x18]`.
        let lon_w = f64::from(w.longitudinal.abs()) * f64::from(if rally { 0.5f32 } else { 0.9f32 });
        let lat_a = f64::from(w.lateral.abs());
        let comb_wide = if lat_a > lon_w { lat_a + 0.25 * lon_w } else { lon_w + 0.25 * lat_a };
        let comb = comb_wide as f32;
        let spin = (w.longitudinal.abs() > limit && throttle_byte > 0x80) || (!front && rear_spin);
        let k = if !front && spin { 4.0 } else { k_class };
        // 0x4A1F25 tests `comb > LIM` alone; `comb > 0` is the port's own guard, for a case **never observed**
        // (2026-10-02 SRV-B, `RE/notes/REFERENCE-2026-10-02-SRVB.md` §5: 4 races, ≥ 350 000 tyre evaluations, a roll onto the roof —
        // `comb == 0` and `LIM < 0` never once; min LIM 0.21, min comb 0.33). Kept as a NaN guard.
        // With a limit driven below zero (`LoadTransfer` has no floor) and no force at all
        // (`comb` exactly 0: needs `d = 0` and a front `v == 0.0` or `u == 0.0`), the original
        // divides by 0 here (0x4A1F95 / 0x4A2007) and gets ±inf / NaN in both arms — `lateral` and
        // `longitudinal` are `0 · inf`. Triaged 2026-09-30 (batch A6): STILL OPEN, B-runtime. A
        // limit `μ·share·g_eff/2 − dz` is negative only when the transfer `dz` outgrows the static
        // load; the debug counters put into this branch (removed again) saw **no negative limit** in the three
        // race goldens (8 s each) and a 60 s alps race with three opponents (1921 ticks, all four
        // wheels each): the case was never reached, so nothing shows what the original's NaN would
        // do to the car (it would poison every later tick). **Re-observed on the original
        // (2026-10-01 SV, Wine, `fe = 0` alps race with three opponents, the car idling at the grid, gdb at
        // 0x4A1F25 printing `limit` = `[esp+0x18]` and `comb` = `[esp+0x14]`): 402 000 evaluations (all
        // wheels) over ~10 min of race time, minimum limit 0.0, none below it and no `comb == 0 && limit <=
        // 0` — the case is not reached by a car sitting or rolling gently; a hard landing / roll was not
        // produced (the original ran at well under 1 frame a second under the breakpoint). STILL OPEN.
        if comb > limit && comb > 0.0 {
            let hard = (tcs && !front && slip_tangent.abs() < 0.15) || (ai && (w.zone > 1 || class == 2));
            if hard {
                // 0x4A1F91…0x4A1FCB: `s = limit / comb` stays in the register; `lateral` / `longitudinal` = `s · (that)`, one `fstp` each
                let s = f64::from(limit) / f64::from(comb);
                w.slip = 0.0;
                if gear > 2 || ai {
                    w.lateral = (s * f64::from(w.lateral)) as f32;
                }
                w.longitudinal = (s * f64::from(w.longitudinal)) as f32;
            } else {
                // 0x4A1FD6…0x4A2057: `X = comb − limit` and `X / k` in the register, `limit / k` rounded to `f32` when it is the smaller
                // (`fst [esp+0xc]`, reloaded), `s = (limit − e) / comb` stored to `f32`; the slip is `(old·3 + X·c) · ¼` in the register
                let x = f64::from(comb) - f64::from(limit);
                let xk = x / f64::from(k);
                let lk = f64::from(limit) / f64::from(k);
                let e = if lk > xk { xk } else { f64::from(lk as f32) };
                let s = ((f64::from(limit) - e) / f64::from(comb)) as f32;
                w.slip = ((f64::from(w.slip) * 3.0 + x * if front { 0.5 } else { 12.0 }) * 0.25) as f32;
                w.lateral *= s;
                w.longitudinal *= s;
            }
        }
        if rally && front {
            w.lateral *= 1.2;
        }
        if !front && spin && !tcs {
            w.slip = comb;
        }
        w.slip = w.slip.min(1.0);
        if tcs {
            w.slip *= 0.5;
        }
        w.sliding = w.slip > 0.5;
        if ang != 0.0 {
            // 0x4A2717…0x4A2774 (`[0x5B4AA0]` = −1/256): the same rotation back, by −angle
            let (nx, nz) = rotate_turns(-ang / 256.0, w.lateral, w.longitudinal);
            w.lateral = nx;
            w.longitudinal = nz;
        }
    }

    /// `Car_physicsStep` with ground contact: the brake force, `LoadTransfer` and each wheel's
    /// demand from the drive split (0x49F7E4…0x49F894); clears the tyre forces.
    pub(super) fn wheel_demands(&mut self, specs: &CarSpecs, vl: Vec3, a_drive: f32) {
        // 0x49F752…0x49F76D: `(1 − 0.005·wear) · brake_decel · pedal` in the register, one `fstp [ebp+8]`
        let mut a_brake = ((1.0 - self.brakes_wear_wide(specs) * f64::from(0.005f32)) * f64::from(specs.brake_decel) * f64::from(self.brake)) as f32;
        a_brake = a_brake.min(vl.z.abs() * 32.0);
        let sign = -vl.z.signum();
        let a_brake = a_brake * if vl.z == 0.0 { 0.0 } else { sign };
        let a_bf = a_brake * specs.front_brake;
        self.load_transfer(specs);
        let f = specs.front_drive;
        let (l0, l1, l2, l3) = (self.wheels[0].limit, self.wheels[1].limit, self.wheels[2].limit, self.wheels[3].limit);
        // The drive split, read to the instruction 2026-09-26 (0x49F7E4…0x49F894): a plain
        // `fdiv` by `L0 + L1` (`wheel+0xAC` at 0x864 / 0x928) and `L2 + L3` (0x9EC / 0xAB0),
        // no floor — `a·specs[0x5C]·L0`, `L1·specs[0x5C]·a`, `(1 − specs[0x5C])·L2·a`,
        // `(1 − specs[0x5C])·L3·a` in that operand order. Since `LoadTransfer` no longer floors
        // the limits (b9141f7) a sum can be negative, and the original divides by it (history:
        // `physics-history.md` §1.7). An exactly zero sum gives the axle no drive share here, where the
        // original's x87 divide makes ±inf / NaN of it — **unreachable** (settled 2026-09-30,
        // batch A6): a sum is `2·μ·share·g_eff/2 ± dz` with `μ ≥ 0.5` in both grip tables
        // (0x5D0D50 / 0x5D0E4C), `share` the front weight 0.44…0.485 of all 86 shipped
        // `.sim`s (or its complement), `grip ≥ 1.1`, so zero needs `g_eff = up.y·grip·g` to be
        // exactly 0 (a car on its side to the last bit) or an exact cancellation with `dz`; a
        // zero or negative-sum tick was not seen in 84 s of races (the three goldens and a 60 s
        // alps race) either.
        // 0x49F7E4…0x49F8D5: each share `a · f · L / (L_pair)` is one register chain (the product left to right, the pair sum
        // `L + L'` kept as it came, the `fdiv`) and one `fstp` to `wheel+0xA8`; then `+ ½·a_bf` / `+ ½·(a_brake − a_bf)`
        // is added to that stored `f32` in the register and stored again (0x49F8A1…0x49F8E1).
        let d = x87::wide;
        let fs_wide = d(l1) + d(l0);
        let rs_wide = d(l2) + d(l3);
        let half_front = 0.5 * d(a_bf);
        let half_rear = 0.5 * (d(a_brake) - d(a_bf));
        let sh = |x: f64, s: f64| if s == 0.0 { 0.0f32 } else { (x / s) as f32 };
        self.wheels[0].demand = (d(sh(d(a_drive) * d(f) * d(l0), fs_wide)) + half_front) as f32;
        self.wheels[1].demand = (d(sh(d(l1) * d(f) * d(a_drive), fs_wide)) + half_front) as f32;
        self.wheels[2].demand = (d(sh((1.0 - d(f)) * d(l2) * d(a_drive), rs_wide)) + half_rear) as f32;
        self.wheels[3].demand = (d(sh((1.0 - d(f)) * d(l3) * d(a_drive), rs_wide)) + half_rear) as f32;
        for w in &mut self.wheels {
            w.lateral = 0.0;
            w.longitudinal = 0.0;
        }
    }

    /// `Car_physicsStep`: `Tyre` on each axle in contact.
    pub(super) fn axle_tyres(&mut self, specs: &CarSpecs, front_contact: bool, rear_contact: bool) {
        // 0x493980, the tyre figure: the zones and the part list do not change inside the step, so
        // the four tyres share one evaluation
        let tyre_damage = if front_contact || rear_contact { self.tyres_wear_wide(specs) } else { 0.0 };
        if front_contact {
            self.tyre(0, specs, tyre_damage);
            self.tyre(1, specs, tyre_damage);
        }
        if rear_contact {
            self.tyre(2, specs, tyre_damage);
            self.tyre(3, specs, tyre_damage);
        }
    }

    /// `Car_physicsStep` …0x49F9E3: the body-frame acceleration `car+0xD4C…+0xD54` from the tyre
    /// forces, and the yaw torque it returns.
    pub(super) fn tyre_force_sums(&mut self, specs: &CarSpecs) -> f32 {
        let w = &self.wheels;
        let d = x87::wide;
        // Extended precision (REFERENCE-2026-10-31-HUMAN-DRIFT.md): 0x49F941…0x49F9E3 keep the sums in the 80-bit
        // register, evaluated here in `f64`. The rear pair's lateral sum is stored once (`[ebp+8]`, 0x49F980) and
        // is an `f32` below. `car+0xD4C = f32((l1 + l0) + [ebp+8])` (0x49F983…0x49F992); `car+0xD54` is the
        // longitudinal sum `((lo3 + lo2) + lo1) + lo0` (0x49F941…0x49F968) times `specs[0x1BC]`, rounded once (0x49F994).
        let rear_lat = (d(w[3].lateral) + d(w[2].lateral)) as f32;
        let fx = ((d(w[1].lateral) + d(w[0].lateral)) + d(rear_lat)) as f32;
        let fz = (((d(w[3].longitudinal) + d(w[2].longitudinal)) + d(w[1].longitudinal)) + d(w[0].longitudinal)) * d(specs.inv_grip);
        self.accel_local = Vec3::new(fx, 0.0, fz as f32);
        // yaw torque, 0x49F9A0…0x49F9E3 re-read 2026-09-26: `A − B` with
        // `A = ((w1.lat + w0.lat) − (w2.lat + w3.lat)) · specs[0x1B8]` and
        // `B = ((w2.long + w0.long) − w1.long − w3.long) · specs[0x1B8]` (the wheels' `+0x90` /
        // `+0x98` at 0x848 / 0x850, 0x90C / 0x914, 0x9D0 / 0x9D8, 0xA94 / 0xA9C) — the sum is
        // exact; MEDIUM only on the sign conventions the tyre model gives `+0x90` / `+0x98`.
        // `A` subtracts the stored `[ebp+8]`; `A` and `B` stay in the register up to the `fsubp` and the `fstp` (0x49F9E3).
        let a = ((d(w[1].lateral) + d(w[0].lateral)) - d(rear_lat)) * d(specs.inv_yaw_lever);
        let b = (((d(w[2].longitudinal) + d(w[0].longitudinal)) - d(w[1].longitudinal)) - d(w[3].longitudinal)) * d(specs.inv_yaw_lever);
        (a - b) as f32
    }
}
