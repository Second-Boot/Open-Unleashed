//! `Vehicle` methods split out of the parent module (a move, no logic changed).

use super::*;

impl Vehicle {
    /// The controller latch, which runs **ahead of** `Car_simulate` and outside it: for the human
    /// `Car_applyInput` 0x4991E0 (Sc32 prio 0x15) stores the bytes `car+0xD7C` / `+0xD7D`, the
    /// steering request `car+0xD90`, the handbrake `car+0xD85` and the shift codes; for an AI car
    /// its pedal routine 0x40BFE0 and `AIState_Racing::update`'s byte copies (0x40CA3E…0x40CA65)
    /// do. So it runs on a tick `Car_simulate` skips the step (`car+0x3A0 < 0.5`, 0x49FD46) too.
    pub(super) fn latch_input(&mut self, c: &Controls, specs: &CarSpecs) {
        self.throttle_raw = c.throttle;
        self.brake_raw = c.brake;
        self.steer_target = c.steer.clamp(-126, 126);
        self.handbrake = c.handbrake;
        // gear selection. An AI car leaves neutral or reverse in its own pedal routine
        // (0x40C051…0x40C0AB, no alive test): past tick 0x1C0 it is put straight into 1st —
        // `car+0xD82` and `+0xD7E` both 2 — with a shift timer of `4 + (rand·8 >> 16)`, one draw;
        // a human car's automatic is 0x49DD20's (in [`Self::process_input`], after the request)
        if self.ai_driven && self.gear <= 1 && self.game_tick > TICK_RACE_START {
            self.shift_timer = 4 + match self.shift_roll.take() {
                Some(r) => (r * 8) >> 16,
                None => self.rand_below(8),
            };
            self.gear = 2;
            self.requested_gear = 2;
        }
        // `Car_applyInput`'s tail (0x499435…0x499475): 0x413890 turns the packet's code into a
        // gear — code 2 (`& 0xF8 == 0x10`) one up while below `specs[0x50] − 1`, code 3 (`== 0x18`)
        // one down while above 0 — counted from the **current** gear, and a downshift is refused
        // while the rpm `car+0xDB4` is past the redline `specs[0x94]`
        if !self.ai_driven {
            let g = self.gear;
            let req = match c.command {
                2 if g + 1 < specs.gear_slots => g + 1,
                3 if g > 0 => g - 1,
                _ => g,
            };
            if !(self.rpm > specs.redline && req < g) {
                self.requested_gear = req;
            }
        }
    }

    /// `Car_processInput` 0x49DD20 (CONFIRMED). Its one call site is inside the step, 0x49F3D4
    /// (no pointer to it in the image), after the body-frame velocity `car+0xD58…` and the slip
    /// tangent `car+0xDC0` are worked out — so a car `Car_simulate` finds tilted
    /// (`car+0x3A0 < 0.5`) does not run it at all: no alive count, no steering ramp, no gearbox.
    pub(super) fn process_input(&mut self, specs: &CarSpecs) {
        let c = Controls { throttle: self.throttle_raw, brake: self.brake_raw, ..Default::default() };
        // 0x49DD29…0x49DD78: the alive count steps on; a dead car — or a human one that has
        // finished (`car+0xB50 >= 2`) — gets throttle 0, the brake full, the downshift flag, the
        // shift timer and the reverse latch cleared, and no gearbox
        if self.engine_alive != 0 {
            self.engine_alive += 1;
        }
        // (`>= 2` over the three states 0 / 2 / 3 = not racing)
        let dead = self.engine_alive == 0 || (!self.ai_driven && self.race_state != RaceState::Racing);
        (self.throttle_byte, self.brake_byte) = if dead || self.engine_alive < 0x40 {
            (0, 0xFF)
        } else if !self.race_started {
            (c.throttle, 0xFF)
        } else {
            (c.throttle, c.brake)
        };
        // The pedal floats `[0x628FB0]` / `[0x628FAC]` are `min(byte / 252, 1)` of those bytes
        // **after** the overrides (0x49E0E3…0x49E147; 240 under `[0x657404] > 0`, not carried).
        // So the hold after a car's (re)birth is `car+0xDB0 < 0x40` — 63 ticks for a new or a
        // dead-and-reset car (`DB0 = 1`), 4 for a live one reset (`0x3C`, 0x49E31E…) — and before
        // the start only the brake is held: the throttle revs the engine on the grid.
        // *(Corrected 2026-09-26: the port zeroed the throttle and held the brake while its own
        // birth counter `ticks_alive` was under 0x40 or the race had not started, so a reset car
        // was never held and a car on the grid could not rev. This 2 s birth hold is also the
        // "dead zone" `physics.md` §4.1b-bis measured with `launch_profile`.)*
        let mut thr = (self.throttle_byte as f32 / 252.0).min(1.0);
        let mut brk = (self.brake_byte as f32 / 252.0).min(1.0);
        if dead {
            self.downshift_blip = false;
            self.shift_timer = 0;
            self.reverse_latch = false;
            self.throttle = 0.0;
            self.brake = 1.0;
            return;
        }
        // 0x49DD7D…0x49DDA2: past the dead exit the shift timer `car+0xD84` counts down, before
        // the gearbox below can load it — its one decrement in the image
        if self.shift_timer > 0 {
            self.shift_timer -= 1;
        }
        // the steering `car+0xD94` (0x49DF81…0x49E000) — reached only past the dead / finished
        // exit above, whose `jmp 0x49E006` (0x49DD78) jumps over it, so a dead car's wheel stays
        // where it was. An AI-driven car (`car+0xDA8`, 0x49DF88) takes the **mean** of the request
        // and the current value, `(0xD90 + 0xD94) / 2` truncated (cdq/sub/sar, 0x49DF98…0x49DF9F);
        // a human one ramps by at most 12 a tick, 18 under `RALLY` or `.TINYCAR`.
        // *(Until the audit of 2026-09-26 every car ramped, AI included, and before the exit.)*
        // Not carried: car-info `+0x14 == 0`'s direct copy for a human car (0x49DFAD).
        if self.ai_driven {
            self.steer = (self.steer_target + self.steer) / 2;
        } else {
            let d = self.steer_target - self.steer;
            let ramp = if self.rally || self.tiny { 18 } else { 12 };
            self.steer += d.clamp(-ramp, ramp);
        }
        self.steer = self.steer.clamp(-127, 127);
        // 0x49DD20 for a manual human car (0x49DEDC / 0x49DEE1), after the start tick (0x49DDAE):
        // the request is taken as it stands — the previous gear kept, the downshift flag set when
        // going down into a forward gear, the shift timer loaded with `specs[0x54]`
        if !self.ai_driven && self.game_tick > TICK_RACE_START {
            if self.automatic {
                self.automatic_request();
            }
            let (g, req) = (self.gear, self.requested_gear);
            if req != g {
                if !self.automatic {
                    self.downshift_blip = g > req && req > 1;
                    self.prev_gear = g;
                    self.gear = req;
                    self.shift_timer = specs.shift_ticks;
                } else if req < 2 {
                    // 0x49DEFA: reverse or neutral is taken as it stands
                    self.downshift_blip = false;
                    self.prev_gear = g;
                    self.gear = req;
                    self.shift_timer = specs.shift_ticks;
                } else if req == 2 && g < 2 {
                    // 0x49DF24…0x49DF7B: out of reverse / neutral, the highest gear whose
                    // `redline · specs[0x170 + 4g]` is below the forward speed `car+0xD60`, else 1st
                    let mut pick = 2;
                    for k in 2..specs.gear_slots {
                        if specs.redline * specs.mps_per_rpm[k] < self.v_local.z {
                            pick = k;
                        }
                    }
                    self.downshift_blip = false;
                    self.prev_gear = g;
                    self.gear = pick;
                    self.shift_timer = specs.shift_ticks;
                }
            }
        }
        // 0x49E178…0x49E212: an automatic (`carinfo+0xC == 1`) in reverse (`car+0xD82 == 0`)
        // **with the latch `car+0xD98` set** swaps the pedal floats — the brake key drives it
        // backwards. A car shifted into reverse by a code (the latch clear) keeps them.
        // *(Corrected 2026-09-26: the port swapped on the gear alone, `physics.md` §5.5's gap.)*
        if self.automatic && self.gear == 0 && self.reverse_latch {
            std::mem::swap(&mut thr, &mut brk);
        }
        self.throttle = thr;
        self.brake = brk;
    }

    /// 0x49DDDE…0x49DEC2 — an automatic human car's own requests: first gear until tick 0x1C4,
    /// then, while the request stands and the car is racing (`car+0xB50 == 0`), reverse when it is
    /// stopped (`car+0x35C < 0.2`) with the brake past 0x80 and no throttle in a forward gear, and
    /// first again when it is stopped in reverse with the throttle past 0x80 — each once, through
    /// the latch `car+0xD98`, which a request that differs from the gear clears.
    /// `car+0x540` / `+0x544` are not carried.
    fn automatic_request(&mut self) {
        if self.game_tick < TICK_AUTOMATIC {
            self.requested_gear = 2;
            return;
        }
        if self.requested_gear != self.gear || self.race_state != RaceState::Racing {
            self.reverse_latch = false;
            return;
        }
        // `|car+0x35C| < [0x5B23F4]` = 0.2 (0x49DE2C…0x49DE52 and again 0x49DE7E…0x49DEA4): the
        // world estimate [`Self::horizontal_speed`], not the physics-frame `vL`. *(Corrected
        // 2026-09-26: the port took the same estimate of `car+0xD58` / `+0xD60`.)*
        let stopped = self.horizontal_speed() < 0.2;
        if stopped && self.brake_raw > 0x80 && self.throttle_raw == 0 && self.gear > 1 && !self.reverse_latch {
            self.requested_gear = 0;
            self.reverse_latch = true;
        } else if stopped && self.throttle_raw > 0x80 && self.gear == 0 && self.reverse_latch {
            self.requested_gear = 2;
            self.reverse_latch = false;
        }
    }

    /// `AutoShift` 0x49DAF0, read to the instruction 2026-09-26 (`physics.md` §4.5). Called from
    /// `Engine_update` (0x49E890) for an automatic or AI car on the drive path.
    ///
    /// * One draw of the LCG: `up = (r · 0.1 · redline) · 2⁻¹⁶ + redline · 0.9`, `down =
    ///   redline · 0.8` (0x49DAF3…0x49DB6A); nothing in reverse or neutral (0x49DB6E).
    /// * `top` = the last slot, one lower when `redline · specs[0x170 + 4·(slots − 2)]` is past the
    ///   top speed (0x49DB8F…0x49DBB7); the up target `gear + 1` below it, the down target
    ///   `gear − 1` above first (0x49DBBA…0x49DBE0).
    /// * The speed is **`car+0x35C`**, the world estimate, signed by `car+0xD60` (0x49DBE0…
    ///   0x49DBF9: the sign test is on `D60`, the `fchs` on the `0x35C` below it); `x` = that ·
    ///   `ratio'[gear]`, and `now` = the rpm when `1.2·x > rpm`, else `x` (0x49DBFB…0x49DC1E);
    ///   `xd` = that speed · `ratio'[down target]` (0x49DC26).
    /// * Throttle `[0x628FB0]` **at or below 0.5** (0x49DC38 `test ah,0x41; je` at `> 0.5`) with
    ///   `|now| <= redline`: only the kick-down — `xd < down`, the target above first,
    ///   `|car+0xDC0| < 0.1`, and not the gear itself — sets `D83 = 1`, `D82`, `D84` and **not
    ///   `D81`** (0x49DC92…0x49DCA4); any other case returns without a shift.
    /// * Otherwise (above 0.5, or `|now|` past the redline): up when `now > up` and the gear is
    ///   below `top` (`D83 = 0`, `D81`, 0x49DCCC…); else down when `xd < down` and the target is
    ///   not the gear (`D83 = 1`, `D81`, 0x49DCEE…). Every shift loads `D84 = specs[0x54]`.
    ///
    /// *(Corrected: the port had the kick-down at a throttle of 0.5 or more, fell through to
    /// the upshift below it, shifted up at `now >= up`, measured `|vL.z|` of the physics frame,
    /// wrote `D81` on the kick-down and built `up` as `redline · (0.9 + 0.1·r01)`.)*
    pub(super) fn auto_shift(&mut self, specs: &CarSpecs) {
        let r = self.rand16() as f32;
        let up = r * 0.1 * specs.redline * 1.525_878_9e-5 + specs.redline * 0.9;
        let down = specs.redline * 0.8;
        let g = self.gear;
        if g < 2 {
            return;
        }
        let dc0 = self.slip_tangent.abs();
        let top = if specs.mps_per_rpm[specs.gear_slots - 2] * specs.redline > specs.top_speed {
            specs.gear_slots - 2
        } else {
            specs.gear_slots - 1
        };
        let up_to = if g < top { g + 1 } else { g };
        let down_to = if g > 2 { g - 1 } else { g };
        let s = if self.v_local.z < 0.0 { -self.horizontal_speed() } else { self.horizontal_speed() };
        let x = s * specs.ratio_rpm_per_mps[g];
        let now = if x * 1.2 > self.rpm { self.rpm } else { x };
        let xd = s * specs.ratio_rpm_per_mps[down_to];
        if self.throttle <= 0.5 && now.abs() <= specs.redline {
            if xd < down && down_to > 1 && dc0 < 0.1 && down_to != g {
                self.downshift_blip = true;
                self.gear = down_to;
                self.shift_timer = specs.shift_ticks;
            }
            return;
        }
        if now > up && g < top {
            if up_to != g {
                self.downshift_blip = false;
                self.prev_gear = g;
                self.gear = up_to;
                self.shift_timer = specs.shift_ticks;
            }
        } else if xd < down && down_to != g {
            self.downshift_blip = true;
            self.prev_gear = g;
            self.gear = down_to;
            self.shift_timer = specs.shift_ticks;
        }
    }

    /// `0x49E6C4` — the induction boost state `car+0x54C`, read to the instruction 2026-09-26. It
    /// runs **after** the rev limiter and the ignition cut, so it sees the cut's shut throttle.
    /// With `lo` / `hi` = `redline·¼` / `redline·½` for a turbo (`induction == 1`) and
    /// `redline/6` / `redline·⅔` for a supercharger (`== 2`): `q = min((rpm − lo) / (hi − lo), 1)`,
    /// 0 when `rpm − lo <= 0` (0x49E70C…0x49E74C — the divisor is `hi − lo`, 0x49E723
    /// `fsubrp st(2)`, `DE E2`); the cap `rpm/redline·½`, ×throttle for a turbo; and the goal is
    /// the **larger** of `q·throttle` and the cap (0x49E781 `fcomp`, `test ah,0x41; jne` keeps
    /// `q·throttle` at `cap <= q·throttle`). Nothing more while the shift timer runs (0x49E794).
    /// A supercharger takes a higher goal at once and falls at most 0.1; a turbo moves at most
    /// 0.05 either way. *(Corrected: the port ran it before the limiter, divided by `hi` and took
    /// the smaller of the two.)* Nothing in the drive path reads it; its consumers are the
    /// turbo sound (0x4AA72E) and the bleed-off at 0x49F075.
    pub(super) fn boost_update(&mut self, specs: &CarSpecs) {
        let (lo, hi) = match specs.induction {
            1 => (specs.redline * 0.25, specs.redline * 0.5),
            2 => (specs.redline * (1.0 / 6.0), specs.redline * (2.0 / 3.0)),
            _ => return,
        };
        let x = self.rpm - lo;
        let q = if x <= 0.0 { 0.0 } else { (x / (hi - lo)).min(1.0) };
        let mut cap = self.rpm / specs.redline * 0.5;
        if specs.induction == 1 {
            cap *= self.throttle;
        }
        let goal = (q * self.throttle).max(cap);
        if self.shift_timer > 0 {
            return;
        }
        self.boost = if specs.induction == 2 {
            if goal > self.boost { goal } else { goal.max(self.boost - 0.1) }
        } else if goal > self.boost {
            goal.min(self.boost + 0.05)
        } else {
            goal.max(self.boost - 0.05)
        };
    }

    /// `Engine_update` 0x49E440 → drive − drag, read to the instruction 2026-09-26
    /// (`physics.md` §4.5).
    pub(super) fn engine_update(&mut self, specs: &CarSpecs, rear_contact: bool) -> f32 {
        let vz = self.v_local.z;
        let a_drag = specs.drag * vz * vz * vz * self.draft;
        let gear = self.gear;
        let neutral = gear == 1;
        let rpm_target = if neutral || !rear_contact {
            ((specs.redline + 250.0) * self.throttle).min(specs.redline + 250.0)
        } else {
            ((specs.redline + 100.0) * self.throttle).min(specs.redline + 100.0)
        };
        // The rev limiter of `physics.md` §4.1: crossing the redline pins the rpm just above it and
        // arms an ignition cut of a few ticks, whose length is a small random draw — longer at a
        // standing start. Read to the instruction 2026-09-26: the window is `redline <= rpm <
        // redline + 250` (0x49E58B `test ah,1; jne` skips only below the redline, 0x49E5A8
        // `test ah,0x41; jne` at `redline + 250 <= rpm`); the cut is `(r·2 >> 16) + 3·gear` in a
        // forward gear with the rear down, else `(r·20 >> 16) + 10` for a car **without** the
        // human bit (`car+0x52C & 4`, 0x49E616 `jne` to the short arm) while the **game tick**
        // `[0x606AC4]` is under 0x1A0 (0x49E61F), else `(r·2 >> 16) + 2`. *(Corrected: the port
        // had `rpm > redline`, so a blip that stopped at the redline exactly never cut, and gave
        // the long cut to the human, counted on its own `ticks_alive`.)*
        if self.rpm >= specs.redline && self.rpm < specs.redline + 250.0 {
            self.rpm = specs.redline + 50.0;
            self.ignition_cut = if gear > 1 && rear_contact {
                self.rand_below(2) + 3 * gear as u32
            } else if self.ai_driven && self.game_tick < TICK_AI_MISFIRE {
                self.rand_below(20) + 10
            } else {
                self.rand_below(2) + 2
            };
        }
        // The cut writes 0 into the pedal float **`[0x628FB0]` itself** (0x49E6A5), not a local: so
        // for the rest of this step `AutoShift` (0x49DC2A), the clutch arm (0x49EB36, 0x49EB67),
        // the drive (0x49EDE8), `Tyre`'s demand gate (0x4A21C2, 0x4A222A) and the tail's coast arm
        // (0x49FBD2) all see a shut throttle. *(Corrected 2026-09-26: the port zeroed a local.)*
        let mut rpm_target = rpm_target;
        if self.ignition_cut > 0 {
            self.throttle = 0.0;
            rpm_target = 0.0;
            self.ignition_cut -= 1;
        }
        self.boost_update(specs);
        let throttle = self.throttle;
        // 0x49E843 / 0x49E849: both axles' clutch-slip states are cleared at the top of every call,
        // **before** the neutral and shift early-outs — so the flag is per-tick, and a car sitting
        // in neutral or mid-shift is never "spinning". This port used to set it only inside the
        // block below, so on those paths it kept the previous tick's value.
        self.rear_spin = false;
        // `misfire = engineDamage/300 > rand01` (§4.3), drawn at the top of the routine (0x49E4BD)
        let engine_wear = self.engine_wear(specs);
        let misfire = engine_wear / 300.0 > self.misfire_roll;
        // **The no-drive path, 0x49EF36…0x49F075**, entered in neutral, mid-shift or with the rear
        // off the ground (0x49E84F / 0x49E85D / 0x49E869):
        //
        // * a misfire takes 100 off (0x49EF40);
        // * below the target and not shifting, +250, up to the target (0x49EF66…0x49EF87);
        // * shifting, unless it is out of neutral (`car+0xD81 == 1`, 0x49EF94): with the downshift
        //   flag the blip `[0x5D0C70 + 4·req]` (250, 200, 175, 150, 125) — `[0x5D0C90]` (200 … 100)
        //   with the brake past 0x40 — up to the redline (0x49EFA1…0x49EFFD), without it 100 off, or
        //   200 in fourth and up (0x49EFFF…0x49F018);
        // * otherwise, at or above the target, 200 off, down to the target (0x49F03B…0x49F06F);
        //
        // the misfire and the plain fall floored at 0 (0x49F024), and the boost bleeds 0.1 on the
        // way out (0x49F075). The shift timer is only **read** here (0x49E85D, 0x49EF57): it is
        // counted down in `Car_processInput` (0x49DD9E) — [`Self::process_input`]. The blip table
        // is picked by the brake byte `car+0xD80` as 0x49DD20 left it (0x49EFAB), not the
        // controller's. *(Corrected 2026-09-26: the port counted the timer down here, so an
        // automatic upshift, loaded inside this routine, lost the drive one tick longer than the
        // original's, and read the raw brake byte.)*
        if neutral || self.shift_timer > 0 || !rear_contact {
            let shifting = self.shift_timer > 0;
            let mut rpm = self.rpm;
            if misfire {
                rpm = (rpm - 100.0).max(0.0);
            } else if rpm < rpm_target && !shifting {
                rpm = (rpm + 250.0).min(rpm_target);
            } else if shifting && self.prev_gear != 1 {
                if self.downshift_blip {
                    const BLIP: [f32; 8] = [0.0, 0.0, 250.0, 200.0, 175.0, 150.0, 125.0, 0.0];
                    const BLIP_BRAKING: [f32; 8] = [0.0, 0.0, 200.0, 175.0, 150.0, 125.0, 100.0, 0.0];
                    let t = if self.brake_byte > 0x40 { &BLIP_BRAKING } else { &BLIP };
                    rpm = (rpm + t[self.requested_gear.min(7)]).min(specs.redline);
                } else {
                    rpm = (rpm - if gear >= 4 { 200.0 } else { 100.0 }).max(0.0);
                }
            } else if rpm >= rpm_target {
                rpm = (rpm - 200.0).max(rpm_target);
            }
            self.rpm = rpm;
            self.boost = (self.boost - 0.1).max(0.0);
            return -a_drag;
        }
        // 0x49E87E…0x49E88D: `carinfo+0xC == 1` **or** an AI-driven car (`car+0xDA8 != 0`) runs AutoShift
        if self.automatic || self.ai_driven {
            self.auto_shift(specs);
        }
        let gear = self.gear;
        // 0x49E898…0x49E8C4: while a shift runs without the downshift flag the wheel rpm is still
        // the **previous** gear's
        let ratio_gear = if self.shift_timer > 0 && !self.downshift_blip { self.prev_gear } else { gear };
        let rpm_wheel = specs.ratio_rpm_per_mps[ratio_gear] * vz;
        let rpm_c = self.rpm.clamp(0.0, specs.redline);
        let t = specs.torque_at(rpm_c);
        let gearbox_wear = self.gearbox_wear(specs);
        let mut a_drive = t * specs.accel_per_torque[gear] * (1.0 - engine_wear / 300.0) * (1.0 - 0.005 * gearbox_wear);
        // `diff = target − |wheel rpm|` (0x49E9A7…0x49E9AD, `fsub st(1)`), zeroed when it is under
        // 125 (`[0x5B4A58]`, 0x49E9CA `test ah,1; je` skips at `>=`) **and the target is below
        // `redline − 300`** (0x49E9D7: `fld redline−300; fcomp target; test ah,0x41; jne` skips the
        // zeroing when `redline − 300 <= target`). *(Corrected 2026-09-26: the port and §4.1 had
        // `target >= redline − 300`, the other way round — so a part-throttle car never locked
        // onto its target and a full-throttle one did near the redline.)*
        let mut diff = rpm_target - rpm_wheel.abs();
        if diff.abs() < 125.0 && specs.redline - 300.0 > rpm_target {
            diff = 0.0;
        }
        let slip = self.rpm - rpm_wheel.abs();
        // (`misfire`, above: a damaged engine stumbles, which stops the clutch model pulling the revs
        // down and kills the drive when it is over its target. The draw comes from the shared LCG,
        // so the caller fills `misfire_roll` each tick.)
        // **The clutch-slip block of `Engine_update` 0x49E440, read to the instruction on
        // 2026-09-21** (`physics.md` §4.1a). The note this used to follow had three comparisons the
        // wrong way round, and the port had simplified the rest; together they left the rear axle
        // "spinning" on about 80 % of full-throttle ticks at every speed up to 197 km/h in seventh,
        // which is the constant wheelspin squeal that was reported in play.
        //
        // Entered on any of (0x49EA1B…0x49EA8B):
        //
        // * `slip > 250` (`[0x5B4A60]`), the shift timer `car+0xD84` at 0 (0x49EA2C) and not in
        //   neutral (0x49EA3F) — the timer is read **again** here because `AutoShift`, called
        //   above, may have just loaded it. *(Corrected 2026-09-26: the port took the early-outs
        //   as covering it, so the tick of an automatic upshift could enter the clutch arm.)*
        // * a forward gear while rolling **backwards**, `v < −0.1` (0x49EA52 against `[0x5B2444]`,
        //   `test ah,1 ; jne` — taken when st0 < arg);
        // * reverse while rolling **forwards**, `v > 0.1` (0x49EA69, `test ah,0x41 ; je`).
        //
        // The note had the last two as `v ≥ −0.1` / `v ≤ 0.1` — "moving with the gear", which is
        // nearly always. A fourth disjunct, `DB8 == 1 && ignitionCut == 0` (0x49EA76), is dead:
        // 0x49E849 cleared `DB8` at the top of this very call and nothing in the image writes 1.
        let clutch = (slip > 250.0 && self.shift_timer == 0 && gear != 1) || (gear > 1 && vz < -0.1) || (gear == 0 && vz > 0.1);
        self.engine_arm = if clutch {
            EngineArm::Clutch
        } else if diff < 0.0 {
            EngineArm::Overrun
        } else if diff == 0.0 {
            EngineArm::AtTarget
        } else if misfire {
            EngineArm::Misfire
        } else {
            EngineArm::Driving
        };
        if clutch {
            // `allow`, 0x49EA91…0x49EB44: the most the revs may fall this tick. The loads are the
            // wheels' grip limits `+0xAC` (the wheel block is at `car+0x7B8`, so wheel 0's is
            // `+0x864`) — all four for a car with any front drive (`specs[0x5C] > 0`, against 120),
            // the rear two otherwise (against 100) — and the raw throttle byte `[0xD7C]` divides,
            // floored at 0.3. The port had `100·(1 − throttle) + 100`.
            let drive = (15.0 * a_drive).abs();
            let base = if specs.front_drive > 0.0 {
                // summed `+0xAB0 + +0x9EC + +0x928 + +0x864`, wheel 3 first (0x49EABE…0x49EAD0)
                120.0 - (drive - (self.wheels[3].limit + self.wheels[2].limit + self.wheels[1].limit + self.wheels[0].limit))
            } else {
                100.0 - (drive - (self.wheels[3].limit + self.wheels[2].limit))
            };
            // the byte times `[0x5B4A54]` = 1/254 as a float (0x49EB0B), not a division
            let allow = base / (self.throttle_raw as f32 * (1.0 / 254.0)).max(0.3) + (1.0 - throttle) * 100.0;
            // 0x49EB2C…0x49EB61: a quick gearbox (`specs[0x54] ≤ 4` ticks) above first gear bleeds
            // a sixth of the slip off first, whichever branch follows
            // (both sixths are `fmul [0x5B49D4]` = 1/6 as a float, 0x49EB55 / 0x49EBE7)
            if specs.shift_ticks <= 4 && gear > 2 {
                self.rpm -= slip * (1.0 / 6.0);
            }
            // 0x49EB67: **on the throttle** (≥ 0.5) and not misfiring, the clutch pulls the revs
            // down hard — by `min(slip, allow)` — and the axle is marked spinning only when the
            // slip is past an eighth of the redline (`specs[0x94] · [0x5B272C]`, 0x49E473),
            // not past 250. **Off** the throttle the revs only drift down by a sixth (0x49EBE3).
            // The port had these two branches swapped, so at full throttle the revs never came
            // down to the wheels.
            if throttle >= 0.5 && !misfire {
                if slip > specs.redline * 0.125 {
                    self.rear_spin = true;
                }
                self.rpm -= slip.min(allow);
            } else {
                self.rpm -= slip * (1.0 / 6.0);
            }
        } else if diff < 0.0 {
            // **The three arms, read to the instruction 2026-09-26.** 0x49EBFE loads the local
            // `[esp+0x14]`, which is `diff = target − |wheel rpm|` (0x49E9AD), and its
            // `test ah,1; je 0x49ED4A` sends `diff >= 0` on: so **this** arm, 0x49EC13…0x49ED45, is
            // the one with the wheels **above** the target — the engine over-run, engine braking.
            // *(Corrected: the port and §4.1-bis took it for "under target, rev up" and ran the
            // ladder below on over-run, so lifting off gave no engine braking at all.)*
            //
            // The revs step toward the wheels by `min(−slip, specs[0x170 + 4·gear] · 81920)`
            // (`[0x5B4A50]`; a `max` in reverse, `test cl,cl` at 0x49EC4C, the ratio there being
            // negative) — 0x49EC4E…0x49EC85 and the twin 0x49ECE7…0x49ED1E. The drive (the st0
            // carried through from 0x49E98C) is:
            // * `|wheel rpm| > redline + 500` (0x49EC3E `fcompp`, `test ah,0x41; jne` at `<=`):
            //   `−a` (0x49EC4A `fchs`);
            // * otherwise `−(a · 4? · specs[0x98]) · min(1 + 0.02·gearboxDamage, 5)` — ×4 while the
            //   ignition cut `car+0x548 > 0` runs (0x49EC90…0x49EC9A), `specs[0x98]` (0x49ECA0,
            //   the `.sim` `+0x98` ×0.01 of 0x49C880), 0x493AA0 ×`[0x5B3714]` = 0.02 + 1, capped
            //   at `[0x5B2400]` = 5 (0x49ECAD…0x49ECDE) — and the revs never end below the target
            //   (0x49ED24…0x49ED3F: `rpm <= target` → `rpm = target`).
            let over = rpm_wheel.abs() > specs.redline + 500.0;
            if over {
                a_drive = -a_drive;
            } else {
                let k4 = if self.ignition_cut > 0 { 4.0 } else { 1.0 };
                let neg = -(a_drive * k4 * specs.engine_brake);
                a_drive = (gearbox_wear * 0.02 + 1.0).min(5.0) * neg;
            }
            let cap = specs.mps_per_rpm[gear] * 81920.0;
            let step = if gear == 0 { (-slip).max(cap) } else { (-slip).min(cap) };
            self.rpm += step;
            if !over && self.rpm <= rpm_target {
                self.rpm = rpm_target;
            }
        } else if diff == 0.0 {
            // 0x49ED4A…0x49ED6B (`test ah,0x40`, equal): the revs take the **signed** wheel rpm and
            // the st0 becomes the drag `[esp+0x18]`, so the step returns `drag − drag` = 0 — the
            // car holds its speed. *(The port kept the drive here and took `|wheel rpm|`.)*
            self.rpm = rpm_wheel;
            a_drive = a_drag;
        } else {
            // **Under the target, 0x49ED70…0x49EE52 — the driving arm.** A misfire takes 100 off
            // and the drive is 0 (0x49ED78…0x49ED8C). Otherwise a ladder of three:
            // `slip > [0x5B2484]` (200) takes 200 off; `slip < [0x5B4A4C]` (−200) **adds** 200;
            // between the two the revs take the **signed** wheel rpm (0x49EDDE); the drive is
            // scaled by the throttle `[0x628FB0]` (0x49EDE8). Both then cap the revs at the target
            // (0x49EDEE…0x49EE05, `min`), and under `RALLY` `[0x6573F8]` the drive is
            // ×1.25·min(1 + |slip tangent|, 2) (0x49EE0B…0x49EE52) — in this arm only.
            if misfire {
                self.rpm -= 100.0;
                a_drive = 0.0;
            } else {
                if slip > 200.0 {
                    self.rpm -= 200.0;
                } else if slip < -200.0 {
                    self.rpm += 200.0;
                } else {
                    self.rpm = rpm_wheel;
                }
                a_drive *= throttle;
            }
            self.rpm = self.rpm.min(rpm_target);
            if self.rally {
                a_drive = a_drive * 1.25 * (self.slip_tangent.abs() + 1.0).min(2.0);
            }
        }
        // 0x49EE54…0x49EEFB, revs below zero (`test ah,1; je` at `>= 0`): `t = −vL.z · 32`
        // (0x49EE6B…0x49EE79), the pull that stops the car in one tick. The drive stands — and the
        // revs stay negative — when it and `t` have the same sign and the drive is the larger
        // (`d > 0, t > 0, d − t > 0`, 0x49EE7D…0x49EEAC; `d < 0, t < 0, d − t < 0`,
        // 0x49EEB2…0x49EEE1); otherwise the revs are 0 and the step returns `t − drag`.
        // *(The port always took the second, so a car creeping backwards at 0 rpm in first could
        // not pull away while the revs, now signed like the original's, sat below zero.)*
        if self.rpm < 0.0 {
            let t = -vz * 32.0;
            let keep = (a_drive > 0.0 && t > 0.0 && a_drive - t > 0.0) || (a_drive < 0.0 && t < 0.0 && a_drive - t < 0.0);
            if keep {
                return a_drive - a_drag;
            }
            self.rpm = 0.0;
            return t - a_drag;
        }
        self.rpm = self.rpm.min(1.25 * specs.redline);
        // 0x49EF02…0x49F0B9: the st0 each arm leaves, less the drag. The clutch arm's is the
        // drive as it stands — no throttle factor (0x49EBDE / 0x49EBF9 jump straight here).
        a_drive - a_drag
    }

    /// `LoadTransfer` 0x4A1900 (CONFIRMED).
    pub(super) fn load_transfer(&mut self, specs: &CarSpecs) {
        let g_eff = self.frame.row(1).y * specs.grip * GRAVITY;
        let wet = self.wet_track;
        for (i, w) in self.wheels.iter_mut().enumerate() {
            // 0x4A1995 / 0x4A19C1: `wheel+0x80` = 1 / 0 with the table. *(Corrected 2026-09-26:
            // nothing set the flag, so every car drove the dry table, `autobahn` included.)*
            w.wet = wet;
            let tbl = if w.wet { &GRIP_WET } else { &GRIP_DRY };
            // AI cars (`0x52C & 8` / `0xDA8`) read a remapped μ column (0x4A196F..0x4A19BA, HIGH):
            // dry class 1 → column 0, wet class 0 → column 1
            let col = match (self.traction_control, w.wet, specs.class) {
                (true, false, 1) => 0,
                (true, true, 0) => 1,
                (_, _, c) => c,
            };
            let mu = tbl[w.surface & 15][col];
            let share = if i < 2 { specs.front_weight } else { 1.0 - specs.front_weight };
            w.limit = mu * share * g_eff * 0.5;
        }
        let mut dz = self.accel_local.z * specs.weight_transfer * (-1.0 / GRAVITY) * g_eff * (-0.2) / specs.transfer_div;
        // 0x4A1A20 `mov al, [car+0xD7F]; test al, al; … jbe` asks for the throttle **byte**
        // (not the pedal float, which an automatic swaps in reverse), and 0x4A1A4C…0x4A1A5D's
        // `test ah,0x41; jne` for `vL.z > 0`, strictly. *(Corrected 2026-09-26: the port had
        // `throttle > 0.0` and `vz >= 0`.)*
        if self.throttle_byte != 0 && self.v_local.z > 0.0 {
            dz *= (1.0 - 10.0 * self.slip_tangent.abs()).max(0.0);
        }
        self.wheels[0].limit -= dz;
        self.wheels[1].limit -= dz;
        self.wheels[2].limit += dz;
        self.wheels[3].limit += dz;
        if self.rally {
            self.wheels[0].limit *= 1.1;
            self.wheels[1].limit *= 1.1;
        }
        let dx = self.accel_local.x * specs.weight_transfer * (-1.0 / GRAVITY) * g_eff * 0.2 / specs.transfer_div;
        let dx2 = dx / specs.lateral_div;
        // 0x4A1B34 `fcom 0; test ah,0x41; jne 0x4A1B71` picks the arm by the sign of `dx`, and
        // `specs[0x130]` divides the **unloaded** side in both: `dx > 0` takes `dx'` off the
        // left pair (0, 2) and adds `dx` to the right (1, 3) (0x4A1B47…0x4A1B69, 0x4A1B9D);
        // `dx <= 0` takes `dx` off the left and adds `dx'` to the right (0x4A1B71…0x4A1B99).
        // *(Corrected 2026-09-26: the port always took `dx` from the left and gave `dx'` to the
        // right, which is only the second arm; `physics.md` §5.1's "left −= dx / right += dx'
        // (sign by dx)" was read without the sign.)*
        let (left, right) = if dx > 0.0 { (dx2, dx) } else { (dx, dx2) };
        self.wheels[0].limit -= left;
        self.wheels[2].limit -= left;
        self.wheels[1].limit += right;
        self.wheels[3].limit += right;
        self.gravity_scale = GRAVITY_SCALE;
        let vz = self.v_local.z;
        if vz > 5.0 {
            let aero = 1.0 - 0.005 * self.aero_wear(specs);
            let df = specs.downforce_front * vz * aero;
            let dr = specs.downforce_rear * vz * aero;
            self.wheels[0].limit += df;
            self.wheels[1].limit += df;
            self.wheels[2].limit += dr;
            self.wheels[3].limit += dr;
            // 0x4A1C35 `fld [car+0x3A0]; fcomp 0.939; test ah,0x41; jne 0x4A1C62` skips the add
            // when `cosTilt <= 0.939`, so the rear downforce presses an **upright** car down.
            // *(Corrected 2026-09-26: `physics.md` §5.1 and the port had `< 0.939`.)*
            if self.cos_tilt > UPRIGHT_COS_TILT {
                self.gravity_scale += 8.0 * dr;
            }
        }
        // No floor: 0x4A1C48…0x4A1C6B return straight after the downforce, so a limit the
        // transfer drives below zero stays negative. *(Corrected 2026-09-26: the port clamped
        // every limit at 0, which the binary does not.)*
    }
}
