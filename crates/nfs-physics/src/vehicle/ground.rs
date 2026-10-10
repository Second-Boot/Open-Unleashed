//! `Vehicle` methods split out of the parent module (a move, no logic changed).

use super::*;

impl Vehicle {
    /// `Roughness` 0x499730 (car, i) — per-wheel random road-height offset, CONFIRMED:
    /// amplitude and period by surface type, a new target drawn every `period` metres of travel and
    /// interpolated linearly in between. Returns 0 while the car is tilted (`cosTilt < 0.86`).
    pub(super) fn roughness(&mut self, i: usize) -> f32 {
        if self.cos_tilt < 0.86 {
            return 0.0;
        }
        let (amp, period) = match self.wheels[i].surface & 0xF {
            3 | 5 | 9 => (0.1, 2.5),
            2 | 7 => (0.03, 0.25),
            4 | 10 => (0.03, 0.5),
            _ => (0.0, 0.5),
        };
        let speed_est = self.speed_est;
        let w = &mut self.wheels[i];
        // `phase += car+0x35C · 1/32` (0x4997CC…0x4997DE): the held-over speed estimate
        w.rough_phase += speed_est * (1.0 / 32.0);
        // 0x4997E4…0x49980B: a new target is drawn when the phase is **past** the period
        // (`test ah,0x41; jne` keeps it at `phase <= period`) and positive, and also on every
        // tick of a smooth surface (amp < 0.01) — which keeps `prev`/`target` at ~0 there, so
        // a wheel leaving a rough patch does not carry a stale offset.
        if (w.rough_phase > period && w.rough_phase > 0.0) || amp < 0.01 {
            w.rough_phase = 0.0;
            w.rough_prev = w.rough_target;
            let r = self.rand16() as f32;
            let w = &mut self.wheels[i];
            // `amp · [0x5B49D4] − r16 · amp · [0x5B253C]` (1/6, 2⁻¹⁶), in that order
            // (0x49982D…0x499864)
            w.rough_target = amp * f32::from_bits(0x3E2A_AAAB) - r * amp * 1.525_878_9e-5;
        }
        let w = &self.wheels[i];
        w.rough_prev + (w.rough_target - w.rough_prev) * (w.rough_phase / period)
    }

    /// `BodyGroundCollide` 0x496610 + `GroundImpulse` 0x495F30 (`RE/notes/suspension-collision.md`
    /// §6.3, read to the instruction 2026-09-26 — CONFIRMED): body corners below the contact plane
    /// take a normal impulse with surface friction `k`. `mass` is in kg; `invM = 1/m`, `invI =
    /// 1/(6m)` with `m = mass / 128`, `car+0x354` (`Car_physInit` 0x49B510, CONFIRMED). The stored
    /// angular velocity is in turns/s and is converted to rad/s the way the original does.
    /// `car+0x428` is [`Self::min_height`] from [`Self::body_height_on`].
    ///
    /// Answers the effect `car+0x430 = 4·|vel·n|` it leaves (0x496807…0x496844), `None` when no
    /// point is under the plane (no impulse, nothing written).
    pub(crate) fn body_ground_collide_mass(&mut self, mass: f32, n: Vec3, plane_pt: Vec3, k: f32) -> Option<f32> {
        let m = self.orientation;
        let (hw, hh, hl) = (self.half_extents.x, self.half_extents.y, self.half_extents.z);
        let (bx, by, bz) = (m.row(0), m.row(1), m.row(2));
        let (mut count, mut depth, mut centroid) = (0u32, 0.0f32, Vec3::ZERO);
        let contact = |p: Vec3, count: &mut u32, depth: &mut f32, centroid: &mut Vec3| {
            let d = (p - plane_pt).dot(n);
            if d < 0.01 {
                *count += 1;
                *centroid += p;
                *depth = depth.min(d);
            }
        };
        if self.body_points.is_empty() {
            for sz in [-1.0, 1.0] {
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        contact(self.position + by * (sy * hh) + bz * (sz * hl) + bx * (sx * hw), &mut count, &mut depth, &mut centroid);
                    }
                }
            }
        } else {
            let mt = m.transpose();
            for p in &self.body_points {
                contact(self.position + mt * *p, &mut count, &mut depth, &mut centroid);
            }
        }
        if count == 0 {
            return None;
        }
        let c = centroid / count as f32;
        // `GroundImpulse` 0x4960FE: `car+0x440 = C`
        self.last_contact = c;
        // `car+0x358 = 1/m`, `car+0x398 = 1/(6m)` with `m = car+0x354`, the mass in the original's
        // units, kg / 128 (0x4107CD / 0x4A12B5, `RigidBody_init` 0x49B652…0x49B672): the velocity
        // changes do not depend on the unit, but `j` is compared with 0.001 and 10 below
        let mu = mass.max(1e-6) / 128.0;
        let inv_m = 1.0 / mu;
        let inv_i = 1.0 / (6.0 * mu);
        let mut w = self.angular_velocity * TAU; // rad/s
        let r = c - self.position;
        let v_c = self.velocity + w.cross(r);
        let vrel = v_c.dot(n);
        // GroundImpulse 0x495F30: `j = −vrel / (0.25·invM + invI·|r × n|)` — the length, not its
        // square (0x495FD4…0x49602E)
        let denom = 0.25 * inv_m + inv_i * r.cross(n).length();
        let mut j = if denom > 1e-12 { -vrel / denom } else { 0.0 };
        let min_extent = hw.min(hh).min(hl);
        // a smackable's `j` is scaled by 0.3 whatever its size (0x495F30, `flags & 0x80`)
        j *= if self.smackable {
            0.3
        } else if min_extent > 1.5 {
            0.6 / min_extent
        } else {
            0.4
        };
        self.impact = self.impact.max(self.velocity.dot(n).abs());
        // 0x496303: `j < 0.001` — the contact is not closing: `vel *= 0.9`, `ω *= 0.9` and nothing
        // else, no friction and no normal impulse (0x496317…0x49633D). *(Corrected 2026-09-26: the
        // port tested `k < 0.001` here and otherwise applied the friction and `n·j` whatever the
        // sign of `j`.)*
        if j < 0.001 {
            self.velocity *= 0.9;
            w *= 0.9;
        } else {
            // the friction (0x496125…0x496300), only with `k > 0`: `t = −vt/|vt|` (just `−vt`
            // when `|vt| ≤ 0.01`, 0x4961E8), `jf = k·j`, cut to `|vt|/d` when the contact point's
            // own change along `t`, `d = (t·jf·½invM + ((t × r) × r)·2jf·invI)·t`, is above `|vt|`
            // (0x496201…0x4962DE — the angular term as the original builds it, whose sign makes
            // `d` fall with `|r|`; *corrected 2026-09-26*: the port cut to `|vt|/(½invM)`, the
            // linear term only). `k ≤ 0` would leave the contact velocity itself as the friction
            // vector (0x49611F → 0x496303); no surface of `0x5D0B94` and no caller has it.
            let mut p = n * j;
            if k > 0.0 && v_c != Vec3::ZERO {
                let vt = v_c - n * vrel;
                let len = vt.length();
                let t = if len > 0.01 { -vt / len } else { -vt };
                let mut jf = j * k;
                let d = (t * (jf * 0.5 * inv_m) + t.cross(r).cross(r) * (2.0 * jf * inv_i)).dot(t);
                if d > len {
                    jf *= len / d;
                }
                p += t * jf;
            }
            // 0x49633E…0x4963DC: `vel += P·½invM`, `ω += (r × P)·2invI`, `P = n·j + t·jf`
            self.velocity += p * (0.5 * inv_m);
            w += r.cross(p) * (2.0 * inv_i);
            // settle a body resting on its points (0x4963E1…0x4964C0): `(flags & 0x80 or cosTilt
            // < 0.2) and car+0x428 < 0.005 and j < 10 and every |ω| < 1 rad/s` → `ω *= 0.75`,
            // `vel *= 0.95`. `car+0x428` is [`Self::min_height`] (`BodyHeight` 0x49A900). The
            // third test is on **`j`** in the original's units (0x49641C reads the slot 0x4960B2
            // stored `j` in), not on `|vrel|` as the port had it — so a heavy body settles only
            // on a slow contact (a 7000 kg truck: `j ≈ 16·|vrel|`).
            if (self.cos_tilt < 0.2 || self.smackable) && self.min_height < 0.005 && j < 10.0 && w.abs().max_element() < 1.0 {
                w *= 0.75;
                self.velocity *= 0.95;
            }
        }
        let lift = (-depth).max(0.0);
        self.position += n * lift;
        for wh in &mut self.wheels {
            wh.world_pos += n * lift;
        }
        // ClampAngVel 0x4964D0: each component to ±0.75 turns/s for a normal car, ±2.25 for a
        // `.TINYCAR` (0x4964E0…0x4964EC, `[0x5B49C0]`)
        let lim = self.ang_limit();
        self.angular_velocity = (w / TAU).clamp(Vec3::splat(-lim), Vec3::splat(lim));
        let effect = 4.0 * self.velocity.dot(n).abs();
        self.damage_by_normal(effect, n, 0, mass);
        Some(effect)
    }

    /// `BodyHeight` 0x49A900 (car, &n, &q) — read 2026-09-25, CONFIRMED: the height of the body's
    /// lowest points above the plane through `q` with normal `n`, measured on the world collision
    /// points `car+0x1DC` (`0x49AE30`: `M·car+0x8C[i] + pos`). `d = (p − q)·n` (0x532330 /
    /// 0x532910) goes into a four-slot ascending list that starts at 1000 (0x49A917…0x49A92F,
    /// `0x447A0000`).
    ///
    /// * **Upright** — `car+0x3A0 > 0.939` (`[0x5B4A0C]`, the previous `Car_ground`'s) and not a
    ///   smackable (0x49A90A…0x49A949): the **first four** points only (the counter at
    ///   0x49A95D), which 0x49ABC0 leaves as the four lowest `BndP` points — the tyre contact set.
    ///   `car+0x424` **and** `car+0x428` both get `½·(mean + min)` of them
    ///   (`[0x5B2468]` = 0.25, `[0x5B23F0]` = 0.5, 0x49AA23…0x49AA4C).
    /// * **Tilted, or a smackable** (0x49AA57): all `car+0x32C` points; `car+0x424` = the lowest,
    ///   `car+0x428` = the mean of the three lowest (`[0x5B2680]` = 1/3, 0x49AB33…0x49AB52).
    ///
    /// `Susp_update` 0x499A70 hands it its own ground normal and `Σc/4`, the mean of the four
    /// wheel contact points (0x49A8CF…0x49A8DD; the same vector whose `.y` it returns);
    /// `Susp_AI` 0x4998C0 the tracker's normal and plane point `car+0x400` (0x499934).
    ///
    /// A body with no [`Self::body_points`] (the port's scenarios) measures its four tyre-bottom
    /// points, which is what the first four `BndP` points are on a real car (port fallback).
    pub(crate) fn body_height_on(&mut self, n: Vec3, q: Vec3) {
        let mt = self.orientation.transpose();
        let upright = self.cos_tilt > UPRIGHT_COS_TILT && !self.smackable;
        let fallback = self.probe_points();
        let pts: &[Vec3] = if self.body_points.is_empty() { &fallback } else { &self.body_points };
        let pts = if upright { &pts[..pts.len().min(4)] } else { pts };
        let mut low = [1000.0f32; 4];
        for p in pts {
            let d = (self.position + mt * *p - q).dot(n);
            // 0x49A998…0x49AA0F: shift down from the first slot `d` is not above
            if let Some(k) = low.iter().position(|l| d <= *l) {
                low[k..].rotate_right(1);
                low[k] = d;
            }
        }
        if upright {
            let v = ((low[0] + low[1] + low[2] + low[3]) * 0.25 + low[0]) * 0.5;
            self.body_height = v;
            self.min_height = v;
        } else {
            self.body_height = low[0];
            self.min_height = (low[0] + low[1] + low[2]) * (1.0 / 3.0);
        }
    }

    /// `ClampAngVel` 0x49BBA0: each component of the world spin `car+0x388` to ±[`Self::ang_limit`]
    /// (0.75; 2.25 for a `.TINYCAR`; 6.75 for a smackable lighter than 0.75, 0x49BBAA…0x49BBE2) —
    /// per component, not on the length.
    fn clamp_ang_vel(&mut self) {
        let lim = self.ang_limit();
        self.angular_velocity = self.angular_velocity.clamp(Vec3::splat(-lim), Vec3::splat(lim));
    }

    /// `Susp_applyLift` 0x49AEC0 (car, dy): after `Car_ground` moved the body by `dy`, each wheel's
    /// compression `wheel+0x68` (`car+0x820 + 0xC4·i`) becomes `c = compr − dy·M[1].y` (`car+0x374`),
    /// clamped to `(i < 2 ? 0.9 : 0.75)·(specs[0x124] − specs[0x120])` above 0 and to
    /// `−specs[0x124]` below (0x49AEE0…0x49AF50). `travel` is `(specs[0x120], specs[0x124])`;
    /// `None` is the smackable's early return (0x49AEC5).
    fn susp_apply_lift(&mut self, dy: f32, travel: Option<(f32, f32)>) {
        let Some((probe, bump)) = travel else { return };
        let up_y = self.orientation.row(1).y;
        for (i, w) in self.wheels.iter_mut().enumerate() {
            let c = w.compression - dy * up_y;
            w.compression = if c > 0.0 { c.min(if i < 2 { 0.9 } else { 0.75 } * (bump - probe)) } else { c.max(-bump) };
        }
    }

    /// `Car_ground` 0x49BD40 from the gravity on (0x49BE53…0x49C344), the part a car and a
    /// smackable share — read 2026-09-25, CONFIRMED. The caller has run `Susp_update` 0x499A70 (a
    /// car) or `Susp_AI` 0x4998C0 (a smackable, `car+0x52C & 0x80`), set [`Self::cos_tilt`]
    /// (`car+0x3A0 = N·M[1]`), [`Self::body_height`] (`car+0x424`) and [`Self::min_height`]
    /// (`car+0x428`), and works out `dh = (h − car+0x41C)·rateHz`. `n` / `q` are the tracker's
    /// patch normal and plane point (`car+0x400`), `surf` the surface nibble `car+0x2C & 0xF`,
    /// `mass` the body's for `BodyGroundCollide`, `travel` [`Self::susp_apply_lift`]'s.
    ///
    /// * gravity `0.30625 · nTicks · car+0x84` while `car+0x428 > 0.005` or the car is a
    ///   `.TINYCAR` (0x49BE59…0x49BE97); `vy` is the velocity after it, `closing = dh − vy`
    ///   (0x49BF36);
    /// * the **tilted test** — `cosTilt < 0.939` (0x49BF48), or any smackable (0x49BF55) — asks
    ///   `car+0x424 < 0.001` (0x49BF62). When it holds: `BodyGroundCollide` 0x496610 with the
    ///   surface's friction `0x5D0B94[surf][0]`, **0.1** instead when `cosTilt > 0.707` on a car
    ///   that is not a smackable (0x49BFBA…0x49BFE8: `test ah, 0x41` jumps over the 0.1 for
    ///   `cosTilt ≤ 0.707` — *corrected 2026-09-26*, the port had it the other way round); a car left upside down (`cosTilt < 0`) dies
    ///   (`car+0xDB0 = 0`, 0x49C01B); and when `cosTilt < 0.707` and `vy < −2` a bounce
    ///   `vel.y −= 0x5D0B98[surf] · vy` with ×0.95 on `vel.x`, `vel.z` and the spin
    ///   (0x49C038…0x49C08D) — `vy` is reloaded at 0x49BF87 and is the value **before** the
    ///   impulse, not `closing`;
    /// * otherwise (upright, or the test failed) the **upright arm** 0x49C19A: `closing > 6` and
    ///   `car+0x424 < −0.05` is a hard landing — `lift = −0.05 − car+0x424`, `car+0x424 = −0.05`,
    ///   `pos.y += lift`, `Susp_applyLift(lift)`, `car+0xAD0 = closing`,
    ///   `vel.y = 0.4 · 0x5D0B98[surf] · closing + dh`, `ClampAngVel` (0x49C1D6…0x49C270);
    ///   `closing ≤ 6` and `car+0x424 < 0.005` sets the body on the plane — `pos.y −= car+0x424`,
    ///   `car+0x424 = 0`, `vel.y = dh` unless both `vy` and `dh` are positive, then the larger —
    ///   and `Susp_applyLift(−car+0x424_old)` (0x49C27D…0x49C321);
    /// * then always (0x49C0F4…0x49C149) the **8 m ceiling rule**: when the enclosure test
    ///   0x473330 finds a patch more than 3.5 m overhead with a non-zero word, the body is rising
    ///   and `pos.y − car+0x404 > 8`, `vel.y = −vel.y`.
    ///
    /// Also (read 2026-09-25): the hard landing runs 0x493DD0 for a racer (`racer`, corrected 2026-09-26)
    /// ([`Self::landing_reset`]) and pushes the landing event ([`Self::landing_sound`]); both
    /// landings clear [`Self::airborne_count`] when upright; in the tilted arm a car upside down
    /// (`cosTilt < −0.86`) with damage on, roof damage `car+0x504` under 40 and still racing
    /// (`car+0xB50 == 0`) pushes the roll-over event and has its roof damage **set to 40**
    /// (0x49C095…0x49C0E6); last, [`Self::travel_dir`] (`car+0x348`). Dead, so not carried:
    /// `car+0x40C = (dh − car+0x420 + 7·car+0x40C)/8`, `car+0x420 = dh` (0x49BF08…0x49BF30) and
    /// `car+0xAD0 = closing` — nothing but the resets (0x40F461 / 0x40F473, 0x49B6BC / 0x49B6D4,
    /// 0x410D10…0x410D1C) touches them; nor is `car+0x88 = tick` (0x49C105), which has no reader found.
    ///
    /// `n_ticks` is the LOD gate's `nTicks` ([`ground_due`]): 1 for every racer, 1 / 2 / 4 for a
    /// cop, a traffic car or a smackable by its distance from the nearest racer.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn car_ground_body(&mut self, dh: f32, n: Vec3, q: Vec3, surf: usize, mass: f32, travel: Option<(f32, f32)>, grid: &TriangleGrid, n_ticks: i32, racer: bool) {
        if self.min_height > 0.005 || self.tiny {
            self.velocity.y -= GRAVITY / 32.0 * self.gravity_scale * n_ticks as f32;
        }
        // `car+0x35C` (0x49BEA9…0x49BF02), held for next tick's `Roughness`
        self.speed_est = self.horizontal_speed();
        let vy = self.velocity.y;
        let closing = dh - vy;
        let surf = surf & 15;
        if (self.cos_tilt < UPRIGHT_COS_TILT || self.smackable) && self.body_height < 0.001 {
            let k = if self.cos_tilt > 0.707 && !self.smackable { 0.1 } else { SURFACE_BOUNCE[surf][0] };
            // `GroundImpulse` leaves the event word `car+0x438 = 0x30000 | car+0x464` (0x4960D6…
            // 0x496104) and `BodyGroundCollide` the magnitude `car+0x430 = 4·|vel·n|` (0x496844)
            if let Some(effect) = self.body_ground_collide_mass(mass, n, q, k) {
                self.ground_sound = Some((effect, self.surface_nibble()));
                self.contact_mag = effect;
                self.contact_kind = 0x30000;
            }
            // 0x49C005…0x49C01B: left upside down by the contact, the car is dead
            if self.cos_tilt < 0.0 {
                if self.engine_alive != 0 {
                    self.engine_kill = 2;
                }
                self.engine_alive = 0;
            }
            if self.cos_tilt < 0.707 && vy < -2.0 {
                self.velocity.y -= SURFACE_BOUNCE[surf][1] * vy;
                self.velocity.x *= 0.95;
                self.velocity.z *= 0.95;
                self.angular_velocity *= 0.95;
            }
            // 0x49C095…0x49C0E6: rolled over with damage on (`[0x657A2C]`), roof under 40, still
            // racing — the roll-over event, and the roof damage is set to 40
            if self.damage_enabled && self.cos_tilt < -0.86 && self.damage[8] < 40.0 && self.race_state == 0 {
                self.rollover_sound = true;
                self.damage[8] = 40.0;
            }
        } else if closing > 6.0 {
            if self.body_height < -0.05 {
                // hard landing: 0x493DD0 for a **racer** (`car+0x52C & 1`, 0x49C1C4 — set on the
                // human's 0x105 and the opponents' 0x10B alike), then the event. *(Corrected
                // 2026-09-26: the port ran it for the human only.)*
                if racer {
                    self.landing_reset();
                }
                self.landing_sound = Some(closing);
                let lift = -0.05 - self.body_height;
                self.body_height = -0.05;
                self.position.y += lift;
                self.susp_apply_lift(lift, travel);
                // 0x49C223…0x49C23B
                if self.cos_tilt > 0.9 {
                    self.airborne_count = 0;
                }
                // `car+0x430` is not written here — the original stores the closing speed in the
                // dead `car+0xAD0`; the event is [`Self::landing_sound`]
                self.velocity.y = 0.4 * SURFACE_BOUNCE[surf][1] * closing + dh;
                self.clamp_ang_vel();
            }
        } else if self.body_height < 0.005 {
            // resting / soft landing: follow the ground
            let old = self.body_height;
            self.position.y -= old;
            self.body_height = 0.0;
            // 0x49C2B1…0x49C2C4
            if self.cos_tilt > 0.86 {
                self.airborne_count = 0;
            }
            self.height_marker = 0.0;
            self.velocity.y = if vy <= 0.0 || dh <= 0.0 { dh } else { vy.max(dh) };
            self.susp_apply_lift(-old, travel);
        }
        // the 8 m ceiling rule; the enclosure test walks the whole quadtree cell, so no depth limit
        if self.velocity.y > 0.0
            && self.position.y - q.y > 8.0
            && grid.ceiling(self.position.x, self.position.y, self.position.z, 3.5, 1.0e6).is_some_and(|h| h.surface & 0xFFFF != 0)
        {
            self.velocity.y = -self.velocity.y;
        }
        // `car+0x348` (0x49C14F…0x49C341, 0x5328D0)
        self.travel_dir = if self.velocity == Vec3::ZERO { self.orientation.row(2) } else { self.velocity.normalize() };
    }

    /// `flags & 1`, the human-driven car — in this port's fields, one that is neither AI-driven,
    /// traffic nor a smackable (the port keeps no flags word).
    pub fn is_human(&self) -> bool {
        !self.ai_driven && !self.is_traffic && !self.smackable
    }

    /// `PostHit` 0x493E40 (car) — read 2026-09-25, CONFIRMED: after a wall (0x494351) or a solid
    /// prop (0x498588) hit, `vel *= 0.998` (`0x3F7F7CEE`, 0x493E4D), the body-frame velocity
    /// `car+0xD58…+0xD60` from the body rows `car+0x364…` (0x493E5A…0x493E8D), the speed estimate
    /// `car+0x35C` (the port works it out where it is read), and **for the human**
    /// (`car+0x52C & 4`, 0x493EF1 — the human's flags 0x105 carry it, an opponent's 0x10B does not)
    /// 0x493DD0 ([`Self::landing_reset`]). *(Corrected 2026-09-25: the notes had "AI cars call
    /// 0x493DD0".)*
    pub(super) fn post_hit(&mut self) {
        self.velocity *= 0.998;
        self.speed_est = self.horizontal_speed();
        self.v_local = self.orientation * self.velocity;
        if self.is_human() {
            self.landing_reset();
        }
    }

    /// 0x493DD0 (car) — read 2026-09-25, CONFIRMED: the body-frame velocity `car+0xD58` /
    /// `+0xD60` from the physics frame rows `car+0x490` / `+0x4A8` (0x493DD6…0x493DFC; the `|vz|`
    /// it works out goes nowhere), the slip tangent `car+0xDC0 = 0`, every wheel's slip fraction
    /// `wheel+0xB8 = 0` (`car+0x870 + 0xC4·i`) and the wheelspin state `car+0xDB8 = 0`
    /// (0x493E1A…0x493E37). Callers: `Car_ground`'s hard landing for the human (0x49C1CE),
    /// [`Self::post_hit`] for the human (0x493EF6) and the crash counter 0x40F740 (0x40F8DB) —
    /// the last not wired: it runs for the human only when the impact `car+0x430` is non-zero, no
    /// other car is recorded (`car+0x434 == 0`) and the impact is above `[0x5B2730]` =
    /// **6553600.0** (0x40F891…0x40F8DB), which no hit in the game reaches.
    pub fn landing_reset(&mut self) {
        let f = self.frame;
        self.v_local.x = f.row(0).dot(self.velocity);
        self.v_local.z = f.row(2).dot(self.velocity);
        self.slip_tangent = 0.0;
        // `car+0x870 + 0xC4·i` is `wheel+0xB8`, the slip fraction (wheel blocks from
        // `car+0x7B8`, 0x49F8F7) — not the airborne flag `wheel+0x84` (`car+0x83C`, 0x49F186).
        // *(Corrected 2026-09-26: the port cleared the airborne flags and set `contact`, so the
        // next step saw both axles on the ground after a hard landing or a wall hit.)*
        for w in &mut self.wheels {
            w.slip = 0.0;
        }
        self.rear_spin = false;
    }

    /// `Susp_update` 0x499A70 + `Car_ground` 0x49BD40 (CONFIRMED, `RE/notes/suspension-collision.md` §3–4).
    /// Wheel reference points (tyre bottoms) probe the ground; pitch/roll rates are driven by the
    /// front/rear and left/right contact height differences; the body follows the contact plane.
    pub(super) fn ground_contact(&mut self, grid: &TriangleGrid, specs: &CarSpecs) {
        let h = self.susp_update(grid, specs, false);
        let up = self.orientation.row(1);
        self.car_ground_rest(h, up, grid, specs, 32.0, 1, true);
    }

    /// `Car_ground` 0x49BD40 for a car whose `car+0xE20` it is but which is not a racer — a cop
    /// (`nfs_game::police`): the same `Susp_update` 0x499A70 (only `car+0x52C & 0x80` forks, at
    /// 0x49BD9C) and the same arms, at the rate the LOD gate hands it — `(rate_hz, n_ticks)` from
    /// [`ground_due`], the racers' (32, 1) otherwise. The caller has tracked [`Self::tracker`]
    /// (0x49AF70, priority 6) and set the AI's `car+0xD4C…` / `car+0xD40` ([`Self::accel_local`],
    /// [`Self::accel_world`]) and `car+0xD58…` ([`Self::v_local`]) from its drive.
    ///
    /// `police` is `car+0x52C & 0x20`, set only on a cop (0x40F3C9…0x40F3F3, `traffic.md` §7):
    /// `Susp_update` gives such a car its own dive / roll filters (0x49A4CD, 0x49A61E).
    pub fn ai_ground_step(&mut self, grid: &TriangleGrid, specs: &CarSpecs, rate_hz: f32, n_ticks: i32, police: bool) {
        let h = self.susp_update(grid, specs, police);
        let up = self.orientation.row(1);
        self.car_ground_rest(h, up, grid, specs, rate_hz, n_ticks, false);
    }

    /// `Susp_update` 0x499A70 (car, &normalOut) → the ground height `(Σc/4).y`
    /// (`RE/notes/suspension-collision.md` §3): the four wheel probes, the compression, the
    /// suspension vectors, the ground normal ([`Self::ground_normal`]), the pitch / roll rates,
    /// `car+0x418` and `BodyHeight` 0x49A900 into `car+0x424` / `+0x428`.
    pub(super) fn susp_update(&mut self, grid: &TriangleGrid, specs: &CarSpecs, police: bool) -> f32 {
        let m = self.orientation; // rows = body axes in world space
        let mt = m.transpose();
        let up = m.row(1);
        let s_sign = if self.cos_tilt > 0.0 { 1.0 } else { -1.0 };
        let mut contacts = [Vec3::ZERO; 4];
        let mut susp_y = [0f32; 4];
        let mut n_air = 0;
        let mut heights = [0f32; 4];
        let probes = self.probe_points();
        for i in 0..4 {
            // the world collision point `car+0x1DC + 12i` (0x49AE30: `pos + Σ p_k·M[k]`) of body
            // point i, which 0x49ABC0 left as wheel i's ground reference (0x499AD4…0x499B13)
            let world_pt = self.position + mt * probes[i];
            let probe = world_pt + Vec3::new(0.0, specs.probe_offset * up.y, 0.0);
            // the wheel's tracker (0x499B64 `updateZone(probe, 1)`); its zone word — 0 with no
            // patch — is the wheel's surface `wheel+0x8C` on **both** branches (0x499B86…0x499BA7,
            // `RALLY` turning nibbles 1 and 10 into 2)
            self.wheels[i].tracker.update_zone(grid, probe);
            let nibble = (self.wheels[i].tracker.word & 0xF) as usize;
            let zone = if rally_reads_as_two(nibble, self.rally) { 2 } else { self.wheels[i].tracker.word & 0xFFFF };
            self.wheels[i].zone = zone;
            self.wheels[i].surface = (zone & 0xF) as usize;
            // 0x494480: no surface nibble ⇒ 1; the patch plane more than 4 m from the body's
            // `car+0x334` ⇒ 2 (0x4944CB…0x4944F0, `[0x5B2404]`); else 0, the patch branch
            let patch_h = if nibble == 0 {
                None
            } else {
                self.wheels[i].tracker.patch_height(probe.x, probe.z).filter(|h| (h - self.position.y).abs() <= 4.0)
            };
            let h_i = match patch_h {
                Some(h) => {
                    // road roughness (0x499730), added on the patch branch only; its speed is
                    // `car+0x35C` ([`Self::speed_est`]) — *corrected 2026-09-26*: the port took
                    // the estimate of the physics-frame `car+0xD58` / `+0xD60`
                    let mut h = h + self.roughness(i);
                    // the front wheels' wobble from suspension damage (0x499C8B…0x499CC1):
                    // `SuspDamage(car)·sin(6.28318·(0.5·i + wheel+0x88))·(1/1500)` — 0x493BC0,
                    // `[0x5B23F0]` 0.5, `[0x5B4A08]` 6.28318, the double `[0x5B4A00]` 1/1500
                    if i < 2 {
                        // the original's constant is 6.28318, not TAU
                        #[allow(clippy::approx_constant)]
                        let arg = (0.5 * i as f32 + self.wheels[i].rotation) * 6.283_18;
                        // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable sin, not std; original inline x87 `fsin` at 0x499CB3.
                        h += self.suspension_wear(specs) * nfs_math::portable::sinf(arg) * (1.0 / 1500.0);
                    }
                    h
                }
                // no usable patch: the kept plane `car+0x3E8` / `car+0x400` (0x499BC4…0x499C2E),
                // with no roughness — the branch jumps straight to the sum at 0x499CC9
                None => self.tracker.plane_height(probe.x, probe.z),
            };
            // the plane's normal the branch used, `wheel+0x4C` (0x499D0F…0x499D12): the tracker's
            // on the patch branch, `car+0x3E8` or `(0, 1, 0)` on the fallback
            let n_i = if patch_h.is_some() { self.wheels[i].tracker.normal } else { self.tracker.fallback_normal() };
            let (vel, acc, row0) = (self.velocity, self.accel_world, m.row(0));
            let w = &mut self.wheels[i];
            heights[i] = h_i;
            contacts[i] = Vec3::new(probe.x, h_i, probe.z);
            w.world_pos = Vec3::new(probe.x, h_i + self.ride_height, probe.z);
            // compression (feeds the wheel render height only), and the wheel's stiffness: 1.0,
            // or **1.25** when the compression is past twice its cap (0x499DD6…0x499DE7 front,
            // 0x499E14…0x499E25 rear; `stiff[4]` at `[esp+0x9C]`, 1.0 each at 0x499A81)
            // `c = (contact − probe)·M[1]`, and −0.1 when `M[1].y < 0.3` (0x499D78…0x499D90,
            // `[0x5B25F0]` / `[0x5B2444]`)
            let c = if up.y < 0.3 { -0.1 } else { (h_i - probe.y) * up.y };
            let mut stiff = 1.0;
            // 0x499D15: before game tick 2 (`[0x606AC4] < 2`) the block is skipped — the
            // compression `wheel+0x68` and the render angle `wheel+0x70` are zeroed (0x499EE4…
            // 0x499EE9) and the stiffness stays 1.0 — and so for a network car (`[0x657408]` and
            // `car+0xDAC`, 0x499D22…0x499D34: `net_car`, a remote car in a positional race).
            if self.game_tick < 2 || self.net_car {
                w.compression = 0.0;
            } else if c > 0.0 {
                let cmax = if i < 2 { 0.9 } else { 0.75 } * (specs.bump_travel - specs.probe_offset);
                if c > 2.0 * cmax {
                    stiff = 1.25;
                }
                w.compression = c.min(cmax);
            } else {
                let c = c.max(-(if w.airborne { 1.65 } else { 1.6 }) * specs.bump_travel);
                w.compression = 0.5 * (c + w.compression);
            }
            w.height_d = h_i - probe.y;
            let d_i = w.height_d.max(-0.30625);
            let v_y = s_sign * d_i * up.y;
            if d_i < -0.2041667 {
                w.airborne = true;
                n_air += 1;
                // `.TINYCAR` keeps the whole of it (0x49A01B skips the ×0.8)
                susp_y[i] = stiff * if self.tiny { v_y } else { v_y * 0.8 };
            } else {
                // **The lateral term** (0x49A050…0x49A1A7): with the velocity's `|x|` and `|z|`
                // both over 0.01 (`[0x5B27A8]`, `test ah,0x41; jne` skips at ≤), `u = normalize(−vx,
                // 0, −vz)`, `u' = (n_i × u) × n_i` (0x532880 twice — the ground-plane direction
                // against the motion), `k = 0.0666667·|vel·M[0]|` (`[0x5B49EC]`) when the wheel
                // hung last tick (`wheel+0x84` is read before 0x49A1AC clears it) or
                // `0.0625·|car+0xD40·M[0]|` (`[0x5B26A4]`) — positive either way, the `fchs` at
                // 0x49A11F / 0x49A163 takes the absolute value — and `(k·u')·M[1]` is added to
                // `susp.y` **after** the stiffness (0x49A1DA `fadd st(1)`). Anything else leaves the
                // 0.0 pushed at 0x499F60.
                let mut lat = 0.0;
                if vel.x.abs() > 0.01 && vel.z.abs() > 0.01 {
                    let u = Vec3::new(-vel.x, 0.0, -vel.z).normalize();
                    let u2 = n_i.cross(u).cross(n_i);
                    let k = if w.airborne { 0.066_666_67 * vel.dot(row0).abs() } else { 0.0625 * acc.dot(row0).abs() };
                    lat = (u2 * k).dot(up);
                }
                w.airborne = false;
                susp_y[i] = stiff * v_y + lat;
            }
            w.contact = !w.airborne;
        }
        let h = (heights[0] + heights[1] + heights[2] + heights[3]) * 0.25;
        // ground normal from the contact points
        let f = ((contacts[0] + contacts[1]) - (contacts[2] + contacts[3])) * 0.5;
        let r = ((contacts[1] + contacts[3]) - (contacts[0] + contacts[2])) * 0.5;
        let mut n = f.normalize_or_zero().cross(r.normalize_or_zero()).normalize_or_zero();
        // 0x49A36C…0x49A3CF: `*normalOut` gets `N`, negated when `N.y < 0`, but the `N.y < 0.5`
        // test that sets `nAir = 4` reads the stack copy **before** the negation (`[esp+0x30]`),
        // so a normal that pointed down (a car on its roof) always counts as all four wheels
        // hanging. *(Corrected 2026-09-26: the port tested the negated normal.)*
        let steep = n.y < 0.5;
        if n.y < 0.0 {
            n = -n;
        }
        if n.length_squared() < 0.5 {
            n = Vec3::Y;
        } else if steep {
            n_air = 4;
        }
        self.ground_normal = n;
        // 0x49A8AA…0x49A8C0: `word car+0x418 = nAir > 1` (after `nAir = 4` for a steep normal)
        self.airborne_count = u16::from(n_air > 1);
        // pitch / roll rate kinematics
        if self.cos_tilt < 0.707 {
            self.dive *= 0.5;
            self.roll_lp *= 0.5;
        } else if n_air < 4 {
            // 8, or 4 for `.TINYCAR` (0x49A438…0x49A44A)
            let div = if self.tiny { 4.0 } else { 8.0 };
            let w_body = m * self.angular_velocity; // (pitch, yaw, roll) rates in body axes
            let p_in = ((susp_y[2] + susp_y[3]) - (susp_y[0] + susp_y[1])) / div;
            let a_l = self.accel_local.z;
            // `.TINYCAR` takes the gain unfiltered (0x49A522…0x49A53E, 0x49A647…0x49A655)
            // A cop (`car+0x52C & 0x20`, 0x49A4CD) clamps `aL` to ±10 (`[0x5B241C]`, `[0x5B49E8]`)
            // and takes 0.125 either way (0x49A4D6…0x49A51A); `.TINYCAR` the gain unfiltered;
            // anyone else 0.125 accelerating, 0.175 braking (0x49A540…0x49A57F), plus 0.8 of the
            // old value (0x49A585). *(Corrected 2026-09-26: the port had no police arm.)*
            // a network car (`car+0xDAC`, 0x49A4AB…0x49A4C8) holds `car+0xDC4` at 0 and skips the filter
            self.dive = if self.net_car {
                0.0
            } else if police {
                a_l.clamp(-10.0, 10.0) * specs.dive_gain * 0.125 + self.dive * 0.8
            } else if self.tiny {
                specs.dive_gain * a_l
            } else {
                0.8 * self.dive + specs.dive_gain * a_l * if a_l > 0.0 { 0.125 } else { 0.175 }
            };
            let p = sign_clamp(p_in - self.dive, specs.pitch_clamp);
            let r_in = ((susp_y[1] + susp_y[3]) - (susp_y[0] + susp_y[2])) / div;
            // the cop's roll filter takes 0.15 (`[0x5B2638]`, 0x49A627…0x49A639), anyone else 0.25
            self.roll_lp = if police {
                specs.roll_gain * self.accel_local.x * 0.15 + self.roll_lp * 0.8
            } else if self.tiny {
                specs.roll_gain * self.accel_local.x
            } else {
                0.8 * self.roll_lp + 0.25 * specs.roll_gain * self.accel_local.x
            };
            // the roll target is scaled by `1 − car+0xD7F · [0x5B49E0]` (1/1275, `0x3A4D9A67`): the
            // throttle **byte** as 0x49DD20 left it (0x49A671 `mov al, [esi+0xD7F]` / `fild`),
            // 0…254, so full throttle leaves 0.8 of it (0x49A671…0x49A693). *(Corrected
            // 2026-09-26: the port took the pedal float `[0x628FB0]` · 254, which is 254 at a
            // byte of 252 and follows the automatic's reverse swap of the floats.)*
            let thr = self.throttle_byte as f32 * f32::from_bits(0x3A4D_9A67);
            let rr = sign_clamp(r_in + (1.0 - thr) * self.roll_lp, specs.roll_clamp);
            // `fld specs[0x114]; fmul [car+0xD64]; fmul [0x5B49DC]` (0.925), then `+ p`, in that
            // order (0x49A6DC…0x49A6EB); the roll `D6C · specs[0x118] + r` (0x49A6F1…0x49A6F9);
            // then `ClampAngVel`'s sibling 0x49BC80 clamps `D64` / `D68` / `D6C` to ±0.75 each
            let wp = (specs.pitch_ret * w_body.x * 0.925 + p).clamp(-0.75, 0.75);
            let wr = (specs.roll_ret * w_body.z + rr).clamp(-0.75, 0.75);
            let wy = w_body.y.clamp(-0.75, 0.75);
            self.angular_velocity = mt * Vec3::new(wp, wy, wr);
            // 0x49A756…0x49A8A4: on two wheels — some wheel hangs but not a wheel on **each** side
            // (left = wheels 0 / 2, `car+0x83C` / `+0x9C4`; right = 1 / 3, `+0x900` / `+0xA88`) —
            // the body is shifted by `Mᵀ·v/32` (`[0x5B24B8]`), `v = (−2·sin(2π·wr)·halfW,
            // −2·cos(2π·wr)·halfH, 0)` (`[0x5B2628]` = −2, 0x5327E0 / 0x5327F0 take turns, the
            // clamped roll rate `car+0xD6C`, the half extents `car+0x3A8` / `+0x3AC`), `v.y` negated
            // when the left side hangs and `wr < 0` or the right side hangs and `wr > 0`
            let left_air = self.wheels[0].airborne || self.wheels[2].airborne;
            let right_air = self.wheels[1].airborne || self.wheels[3].airborne;
            if n_air > 0 && !(left_air && right_air) {
                let a = wr * 6.283_184_5;
                // PORT-DEVIATION D1 (RE/notes/PORT-DEVIATIONS.md): portable sin/cos, not std; original 0x5327E0 / 0x5327F0 (x87) at 0x49A7AB / 0x49A7C7.
                let mut v = Vec3::new(-2.0 * nfs_math::portable::sinf(a) * self.half_extents.x, -2.0 * nfs_math::portable::cosf(a) * self.half_extents.y, 0.0);
                if (left_air && wr < 0.0) || (right_air && wr > 0.0) {
                    v.y = -v.y;
                }
                self.position += mt * v * (1.0 / 32.0);
            }
        }
        // body height above the contact plane: `BodyHeight` 0x49A900 on Susp_update's normal and
        // `Σc/4` (0x49A8CF…0x49A8DD), over the body points (`Self::body_height_on`)
        let centre = (contacts[0] + contacts[1] + contacts[2] + contacts[3]) * 0.25;
        self.body_height_on(n, centre);
        self.height_marker = if self.body_height > 0.05 { self.body_height } else { 0.0 };
        h
    }

    /// `Car_ground` 0x49BD40 after its `Susp_update` call, `h` that call's height, `rate_hz` /
    /// `n_ticks` the LOD gate's answer ([`ground_due`]).
    /// `racer` is `car+0x52C & 1`, set on the human (0x105) and the opponents (0x10B) and on no
    /// cop, traffic car or smackable — in this port, a car stepped by [`Self::step32`].
    #[allow(clippy::too_many_arguments)]
    fn car_ground_rest(&mut self, h: f32, up: Vec3, grid: &TriangleGrid, specs: &CarSpecs, rate_hz: f32, n_ticks: i32, racer: bool) {
        // ---- Car_ground 0x49BD40 from 0x49BDBD on (`RE/notes/suspension-collision.md` §4.2) ----
        // `Lod_shouldUpdate` 0x414B10 hands a racer the full rate: 0x417190 leaves `car+0x78` 0
        // for the human (`flags & 1`, 0x4171A5) and the octagonal distance to the nearest racer
        // `0x5E8E2C[]` for anyone else, which for a racer is itself (0); the player also has the
        // force flag `car+0x10B8` (0x406E7F). Every racer therefore runs at `rateHz = 32`,
        // `nTicks = 1` (0x5CC700 / 0x5CC71C at index 6); a cop takes the gate's answer.
        let dh = (h - self.ground_h) * rate_hz;
        self.ground_h = h;
        // `car+0x2C & 0xF`, the car tracker's zone word (0x49BF96, 0x49C244)
        let surf = (self.tracker.word & 15) as usize;
        self.cos_tilt = self.ground_normal.dot(up);
        // the tilted arm's `BodyGroundCollide` takes the car tracker's normal (0x474420,
        // 0x49BFB3) and the patch centre `car+0x400` (0x49BF77); the ceiling rule `car+0x404`
        let (tn, tq) = (self.tracker.normal, self.tracker.plane_point);
        // the physics frame 0x49BA30 comes before the arms (0x49BE4E), so 0x493DD0 reads it
        // The physics frame `car+0x490`, 0x49BA30(car, &N, cosTilt) — read to the instruction
        // 2026-09-26. Row 1 is always the normal `N`. A car that is **not** a racer
        // (`car+0x52C & 1` clear, 0x49BA37) with `cosTilt > 0.9` (0x49BA44…0x49BA4F) keeps the
        // body's rows 0 and 2 as they are. Otherwise the frame is orthonormalised: with
        // `cosTilt < 0.5` and `|M[0]·N| > 0.5` (0x49BAC5…0x49BB2C) `x = N × M[2]`, `z = x × N`
        // (0x49BB2E…0x49BB59), else `z = M[0] × N`, `x = N × z` (0x49BB65…0x49BB90), each
        // normalised (0x5328D0; `0x532880(a, b)` is `a × b`, 0x56E8F5). *(Corrected 2026-09-26:
        // the port kept the body rows for every car above 0.9, racers included, and below it
        // took `x = M[2] × N` — the negated lateral axis — and `z = N × x`.)*
        let n = self.ground_normal;
        let (m0, m2) = (self.orientation.row(0), self.orientation.row(2));
        let cos = self.cos_tilt;
        let (x, z) = if !racer && cos > 0.9 {
            (m0, m2)
        } else if cos < 0.5 && m0.dot(n).abs() > 0.5 {
            let x = n.cross(m2).normalize_or_zero();
            (x, x.cross(n).normalize_or_zero())
        } else {
            let z = m0.cross(n).normalize_or_zero();
            (n.cross(z).normalize_or_zero(), z)
        };
        self.frame = Mat3::from_cols(x, n, z).transpose();
        self.car_ground_body(dh, tn, tq, surf, specs.mass, Some((specs.probe_offset, specs.bump_travel)), grid, n_ticks, racer);
    }
}
