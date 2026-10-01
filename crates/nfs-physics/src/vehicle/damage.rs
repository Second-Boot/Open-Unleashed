//! `Vehicle` methods split out of the parent module (a move, no logic changed).

use super::*;

impl Vehicle {
    /// The part-damage getters' loop (`0x4938A0` family, CONFIRMED — read to the instruction
    /// 2026-09-26): over the twenty slots of `car+0x768`, every part whose type `+0x26` is one of
    /// `kinds` adds `(1.5 − 0.005·part+0x60) · mean + part+0x164`, where `mean` is the zones whose
    /// byte in **that part's own** `+0x48[10]` is non-zero, summed and divided by their count when
    /// it is above 1 (`fidiv` only past `cmp edi, 1; jle`, e.g. 0x493912…0x49391B), 0 with none.
    /// Returns the sum and the number of parts matched. *(Until 2026-09-26 the port used one mask
    /// per group — the family's first part of the type — and one summed scale and offset.)*
    pub fn part_sum(&self, specs: &crate::specs::CarSpecs, kinds: &[u16]) -> (f32, u32) {
        let (mut sum, mut n) = (0.0f32, 0u32);
        for p in specs.parts.iter().filter(|p| kinds.contains(&p.kind)) {
            sum += p.scale * self.part_damage(&p.mask) + p.state;
            n += 1;
        }
        (sum, n)
    }

    /// The mean zone damage under one part's mask (the inner loop of the getters, 0x493903…0x49391B).
    pub fn part_damage(&self, mask: &[bool; 10]) -> f32 {
        let (mut sum, mut n) = (0.0, 0);
        for (d, m) in self.damage.iter().zip(mask.iter()) {
            if *m {
                sum += d;
                n += 1;
            }
        }
        if n > 1 { sum / n as f32 } else { sum }
    }

    /// `0x4938A0`, the engine figure: unlike the rest of the family it starts from **zone 1**
    /// (`fld [car+0x4E8]`, 0x4938AE) and counts from 1 (0x4938A9, `inc ebp` per type-1 part at
    /// 0x4938E2), so it is `(zone1 + Σ part terms) / (1 + n)` and always divides (0x493946). The
    /// misfire, the drive force and HTextDamage's "drivetrain" row read it.
    pub fn engine_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        let (sum, n) = self.part_sum(specs, &[1]);
        (self.damage[1] + sum) / (1 + n) as f32
    }

    /// `0x493A10`, the brakes figure: the type-2 part terms summed, no outer division.
    pub fn brakes_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        self.part_sum(specs, &[2]).0
    }

    /// `0x493AA0`, the gearbox figure: the type-3 part terms summed, no outer division.
    pub fn gearbox_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        self.part_sum(specs, &[3]).0
    }

    /// `0x493B30`, the aero figure: the type-6 part terms summed, no outer division.
    pub fn aero_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        self.part_sum(specs, &[6]).0
    }

    /// `0x493980`, the tyre figure: the type-9 part terms summed, no outer division.
    pub fn tyres_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        self.part_sum(specs, &[9]).0
    }

    /// `0x493BC0`, the suspension figure: the part terms of types 4, 5 and 8 summed, then divided by
    /// their count when it is above 1 (0x493C78…0x493C7E).
    pub fn suspension_wear(&self, specs: &crate::specs::CarSpecs) -> f32 {
        let (w, n) = self.part_sum(specs, &[4, 5, 8]);
        if n > 1 { w / n as f32 } else { w }
    }

    /// `0x493960`, the steering figure: `(zone2 + zone0) · 0.5` (`car+0x4EC`, `+0x4E4`, `[0x5B23F0]`).
    pub fn steering_damage(&self) -> f32 {
        (self.damage[2] + self.damage[0]) * 0.5
    }

    /// `0x493C90`, the body figure: zones 0…8 summed and scaled by `[0x5B49B0]` = 1/12 — nine zones
    /// over twelve, as the binary has it (0x493C94 `mov ecx, 9`).
    pub fn body_damage(&self) -> f32 {
        self.damage[..9].iter().sum::<f32>() * (1.0f32 / 12.0)
    }

    /// `0x493CC0`, the mean body damage in per cent: the ten zones summed and scaled by
    /// `[0x5B49B4]` = 0.1 (0x493CC4…0x493CDC) — the figure HGraphicDamage formats at 0x422B26.
    /// *(Audit 2026-09-26: this named 0x434FA0, which is the mission's damage-limit test,
    /// `nfs_game::skill::damage_over_limit`.)*
    pub fn damage_avg(&self) -> f32 {
        self.damage.iter().sum::<f32>() * 0.1
    }

    /// `Damage_byNormal` 0x494DF0 (CONFIRMED): needs an effect > 10; the zone is chosen from the
    /// contact normal in the body frame (`x = n·right, y = n·up, z = n·forward`).
    ///
    /// The roof zone is gated: 0x494E69 takes it only when `car+0x52C & 0x80` is **clear**, and
    /// that bit marks a smackable ([`Self::smackable`]; it read "traffic" here until 2026-09-25 —
    /// the traffic's bit is 0x10, `ai.md` §6).
    ///
    /// `flag` is `Damage`'s fourth argument — 0 from the body-ground contact (0x49684A), 1 from the
    /// wall collider (0x495563), 2 from the car-car solver (0x496F8E) — and `mass` is the car's
    /// collision mass `car+0x354`; both only feed [`Self::crash_response`].
    ///
    /// Read to the instruction in the 2026-09-26 audit (0x494DF0…0x49501F). The three dot
    /// products are taken once, before anything moves (0x494E03…0x494E32). The roof call
    /// (0x494E54…0x494E84) is **not** an alternative: it returns to 0x494E87 and the ring tests run
    /// on, so a contact from below that also leans far enough to one side damages the roof and a
    /// ring zone — two `Damage` calls, two crash responses, both on the same `car+0x430`. The ring
    /// tests, in order, each a strict comparison with ±0.1 (`[0x5B2604]` / `[0x5B2444]`):
    /// `|x| < 0.1 ∧ z < −0.1` → 1, `|x| < 0.1 ∧ z > 0.1` → 5, `|z| < 0.1 ∧ x > 0.1` → 7,
    /// `|z| < 0.1 ∧ x < −0.1` → 3, `z < −0.1`: `x > 0.1` → 0, `x < −0.1` → 2; `z > 0.1`: `x > 0.1` →
    /// 6, `x < −0.1` → 4; anything else (a component exactly on ±0.1) no ring zone (0x495002: `esi`
    /// is still 9, or 8 after the roof call — both skip). *(The port had the roof and the ring as
    /// alternatives and let the ±0.1 boundaries fall into a zone.)*
    pub fn damage_by_normal(&mut self, effect: f32, n_world: Vec3, flag: u8, mass: f32) {
        if effect <= 10.0 {
            return;
        }
        let m = self.orientation;
        let (x, y, z) = (n_world.dot(m.row(0)), n_world.dot(m.row(1)), n_world.dot(m.row(2)));
        if y < -0.5 && !self.smackable {
            self.damage_zone(effect, 8);
            self.crash_response(effect, 8, flag, mass);
        }
        let zone = if x.abs() < 0.1 && z < -0.1 {
            1
        } else if x.abs() < 0.1 && z > 0.1 {
            5
        } else if z.abs() < 0.1 && x > 0.1 {
            7
        } else if z.abs() < 0.1 && x < -0.1 {
            3
        } else if z < -0.1 && x > 0.1 {
            0
        } else if z < -0.1 && x < -0.1 {
            2
        } else if z > 0.1 && x > 0.1 {
            6
        } else if z > 0.1 && x < -0.1 {
            4
        } else {
            return;
        };
        self.damage_zone(effect, zone);
        self.crash_response(effect, zone, flag, mass);
    }

    /// `Damage` 0x494500's tail (0x49481A…0x494DD9), which runs whatever the damage switch says
    /// and only for a non-zero `flag`: a knock to the body's spin, and on a heavy impact a hop.
    ///
    /// `a` is the amount, doubled for a `.TINYCAR` (0x494863) — the ×0.5 of a knockout run over
    /// the network (`[0x6573E8] == 3 && [0x657408]`, 0x494826) has no race in this port. Past 80
    /// (`[0x5B26D4]`) the amount is taken ×0.125 (`[0x5B272C]`), the body is lifted 0.1 m when it
    /// sits within 0.1 of the ground (`car+0x424`, 0x4948FA), the velocity's `y` gains `a/3` —
    /// for a smackable of collision mass under 1, `max(mass, 0.1 + 0.05·rand) · a · 0.33`
    /// (0x494964…0x4949D7) — the knock is `min(a/32, 2)` with each axis ×1.2 (×3.0 tiny,
    /// 0x49494E), and a car-car hit (`flag == 2`) with the world spin under 0.5 turns/s on `x`
    /// and `z` draws a new one (0x494A0A…0x494C14): `x, z ∈ [−2.5, 2.5)`, `y ∈ [−1, 1)`, or
    /// `[−10, 10)` and `[−5, 5)` for a smackable or a tiny car. Otherwise the knock is `min(a/32, 0.33)`.
    ///
    /// The knock goes on in the body frame (0x494C45…0x494D16): pitch `−k/2` for the front zones
    /// 0…2 and `+k/2` for the rest, yaw and roll `+k/2` for zones 2…4 and `−k/2` for the rest.
    /// Unless the other body was a car (`car+0x434`), the body then drops by the height of
    /// `(−sin 2πroll · hx, −cos 2πroll · hy, 0)` over 32 (0x494D3C…0x494D92). The wall's
    /// call ends in `ClampAngVel` 0x49BBA0 (0x494DD4); the car-car solver clamps on its own.
    ///
    /// It also loads the reset countdown `car+0x1100 = 0xC0` (0x4948E6) and, past 120 on a car
    /// that is not tiny, kills the car (`car+0xDB0 = 0`, 0x4948A6…0x4948C1), sets
    /// [`Self::airborne_count`] to 1 (0x4948D5) and, on a car that is **not** a smackable,
    /// `car+0x42C = 1` ([`Self::knocked_over`], 0x49488C…0x49489A, `ebx` = 1 from 0x494516 — read
    /// 2026-09-25; `traffic.md` §21.7 listed it as a clear; carried since 2026-09-26: its reader is
    /// the AI drive 0x408360's 0.98 exit at 0x4084C3, which a cop runs). A smackable's `+0x42C` is
    /// the solver's. Not carried, each without a counterpart in this port: `+0x1104 = 1`,
    /// `+0x450 = 0` (0x4948D5…0x4948F0).
    ///
    /// `car+0x434 != 0` is exactly `flag == 2` (CONFIRMED in the 2026-09-26 audit, byte scan of
    /// `+0x434`): its non-zero writers are the car-car solver's 0x496CC9 / 0x496DD5; every other
    /// caller clears it on the way in — the wall and prop passes through `WallSpin`, whose first
    /// store is `car+0x434 = 0` whatever it decides after (0x493F1F; called from 0x494348 and
    /// 0x498582), the ground through `GroundImpulse` (0x4960DC) — and 0x410408 clears it after
    /// each tick's impact pass.
    pub(super) fn crash_response(&mut self, amount: f32, zone: usize, flag: u8, mass: f32) {
        if flag == 0 {
            return;
        }
        let mut a = amount;
        if self.tiny {
            a *= 2.0;
        }
        let (k, m) = if a > 80.0 {
            if !self.tiny && a > 120.0 {
                if self.engine_alive != 0 {
                    self.engine_kill = 1;
                }
                self.engine_alive = 0;
            }
            a *= 0.125;
            if self.height_marker < 0.1 {
                self.position.y += 0.1;
            }
            let k = (a * 0.031_25).min(2.0);
            let m = if self.tiny { 3.0 } else { 1.2 };
            self.velocity.y += if self.smackable && mass < 1.0 {
                let t = self.rand16() as f32 * 0.05 * 1.525_878_9e-5 + 0.1;
                mass.max(t) * a * 0.33
            } else {
                a * 0.333_333_34
            };
            self.reset_countdown = 0xC0;
            // 0x49488C…0x49489A: `car+0x42C = 1` (`ebx`) unless `car+0x52C & 0x80`
            if !self.smackable {
                self.knocked_over = true;
            }
            // 0x4948D5: `word car+0x418 = 1` (`ebx`)
            self.airborne_count = 1;
            let w = self.angular_velocity;
            if flag == 2 && w.x.abs() < 0.5 && w.z.abs() < 0.5 {
                let big = self.smackable || self.tiny;
                let (h, v) = if big { (20.0, 10.0) } else { (5.0, 2.0) };
                let x = self.rand16() as f32 * h * 1.525_878_9e-5 - h * 0.5;
                let y = self.rand16() as f32 * v * 1.525_878_9e-5 - v * 0.5;
                let z = self.rand16() as f32 * h * 1.525_878_9e-5 - h * 0.5;
                self.angular_velocity = Vec3::new(x, y, z);
            }
            (k, m)
        } else {
            ((a * 0.031_25).min(0.33), 1.0)
        };
        let o = self.orientation;
        let w = self.angular_velocity;
        let (mut pitch, mut yaw, mut roll) = (w.dot(o.row(0)), w.dot(o.row(1)), w.dot(o.row(2)));
        let d = m * k * 0.5;
        pitch += if zone <= 2 { -d } else { d };
        if (2..=4).contains(&zone) {
            yaw += d;
            roll += d;
        } else {
            yaw -= d;
            roll -= d;
        }
        if flag != 2 {
            let (sr, cr) = (roll * std::f32::consts::TAU).sin_cos();
            let u = Vec3::new(-sr * self.half_extents.x, -cr * self.half_extents.y, 0.0);
            self.position.y += (u.x * o.row(0).y + u.y * o.row(1).y) * 0.031_25;
        }
        self.angular_velocity = o.transpose() * Vec3::new(pitch, yaw, roll);
        if flag != 2 {
            let lim = self.ang_limit();
            self.angular_velocity = self.angular_velocity.clamp(Vec3::splat(-lim), Vec3::splat(lim));
        }
    }

    /// `Damage` 0x494500 (CONFIRMED): `a = min(amount·0.5, 100)` — ×1.5 under `DERBY`
    /// ([`Self::derby`], 0x49452F…0x49457E); the zone takes the maximum and each ring
    /// neighbour is raised to the mean of its two neighbours.
    ///
    /// Only with `Game.Damage` on (`[0x657A2C]`, 0x494500…0x49451B) **and a car still racing**:
    /// `car+0xB50 != 0` — finished or out — jumps past the zones (0x494521…0x494529) to the crash
    /// response, like the switch. *(Audit 2026-09-26: the port had no `+0xB50` gate here.)*
    pub fn damage_zone(&mut self, amount: f32, zone: usize) {
        if !self.damage_enabled || self.race_state != 0 || zone > 9 {
            return;
        }
        let a = (amount * if self.derby { 1.5 } else { 0.5 }).min(100.0);
        // crash sound for the corner / roof zones when the damage crosses 30
        if matches!(zone, 0 | 2 | 4 | 6 | 8) && a > 30.0 && self.damage[zone] < 30.0 {
            self.crash_sound = true;
        }
        self.damage[zone] = self.damage[zone].max(a);
        if zone <= 7 {
            for j in [(zone + 7) % 8, (zone + 1) % 8] {
                let other = if j == (zone + 1) % 8 { (j + 1) % 8 } else { (j + 7) % 8 };
                let v = 0.5 * (self.damage[zone] + self.damage[other]);
                self.damage[j] = self.damage[j].max(v);
            }
        }
    }
}
