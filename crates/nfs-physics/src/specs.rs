//! Runtime car specification = the `.sim` file after `CarSim_load` (0x49C6C0) unit conversions
//! and the derived tables built by 0x49CB50 / 0x49D6xx (`RE/notes/loaders.md`, `RE/notes/physics.md` §3).

use nfs_formats::sim::CarSim;

pub const GRAVITY: f32 = 9.8;

/// One entry of the car's installed-part list `car+0x768` (twenty pointers to 0x168-byte part
/// records, `loaders.md` §1) as the part-damage getters read it (`physics.md` §10, CONFIRMED —
/// 0x4938A0, 0x493980, 0x493A10, 0x493AA0, 0x493B30, 0x493BC0 read to the instruction 2026-09-26):
/// each part contributes `scale · mean(zones under mask) + state` over **its own** mask.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InstalledPart {
    /// `part+0x26`, the part type the getters select on (1 engine, 2 brakes, 3 gearbox, 4/5/8
    /// suspension, 6 aero, 9 tyres).
    pub kind: u16,
    /// `part+0x48[10]`, one byte per damage zone; non-zero bytes are the zones this part averages.
    pub mask: [bool; 10],
    /// `1.5 − 0.005·part+0x60` (the condition %), `[0x5B246C]` / `[0x5B3284]`.
    pub scale: f32,
    /// `part+0x164` as the race reads it: [`Self::carried`] less the unfold of the body damage
    /// the car brought in ([`Self::unfold`]).
    pub state: f32,
    /// `carInfo+0x78 + 4i` (`.PARTDAMAGE`), what `Car_raceInit` copies into `part+0x164`
    /// (0x41085F…0x410886) — the owned part's saved damage, which already holds the fold of the
    /// zones the car brings in.
    pub carried: f32,
}

impl InstalledPart {
    pub fn new(kind: u16, mask: [bool; 10], condition: f32, state: f32) -> Self {
        Self { kind, mask, scale: 1.5 - 0.005 * condition, state, carried: state }
    }

    /// **`0x493CF0(car, 1)`** at 0x41088B, the end of `Car_raceInit` — every pass, so the live race,
    /// a restart and the automatic replay alike (`Cars_resetAll` 0x4110A0 → 0x4110D7): for a part of
    /// type 1…10 (`test ax, ax; jle` / `cmp ax, 0xB; jge`, 0x493D0D…0x493D1A) the scale is negated
    /// (`fchs`, 0x493D37) and `part+0x164 = mean·(−scale) + carried` (0x493D6F…0x493D7D), with no
    /// clamp and no copy to `car+0xC48` (the `jne` at 0x493D85). `mean` is over the part's own mask
    /// of the zones `car+0x4E4` the car starts with (`.DAMAGEZONES`, 0x41083A). The race-over fold
    /// `0x493CF0(car, 0)` adds the end zones' back, so a part gains only what this race's damage
    /// adds (`Career::record_damage`). A part of another type keeps `carried`.
    pub fn unfold(&mut self, mean: f32) {
        self.state = if (1..=10).contains(&self.kind) { mean * -self.scale + self.carried } else { self.carried };
    }
}

/// The part list [`CarSpecs::from_sim`] starts from — **not** the original's (its list is the
/// twenty zeroed records 0x49C4C0 fills from `.PARTINDEX`, so a car with no parts has none). Every
/// racing path of the port overwrites it (`fold_fresh_car`, `installed_parts`,
/// `layout_race_parts`; an empty result stays empty since 2026-09-30) and only the physics
/// tests and a bare `from_sim` see it: one part per getter with the masks of the standard
/// parts of a rear-engined 911 in `nfs5.prt` (brakes / suspension / tyres on the four corners,
/// engine and gearbox at the rear, aero front and rear), `scale` 1.5 and no carried state.
pub fn fallback_parts() -> Vec<InstalledPart> {
    let corners = [true, false, true, false, true, false, true, false, false, false];
    let rear = [false, false, false, false, true, true, true, false, false, false];
    let aero = [true, true, true, false, true, true, true, false, false, false];
    [(1, rear), (2, corners), (3, rear), (4, corners), (6, aero), (9, corners)]
        .into_iter()
        .map(|(kind, mask)| InstalledPart { kind, mask, scale: 1.5, state: 0.0, carried: 0.0 })
        .collect()
}

/// The installed-part list `car+0x768` for a racer with no career state to carry — a quick race
/// or an opponent outside the career: `FECar::init` **0x4EEBA0** (car construction) ends,
/// unconditionally, in **0x4EE9A0** (`career.md` §11.2d, `controls.md` §4.169.6, CONFIRMED), which
/// fills the car's installed list `+0x758` with a fresh copy of every part on its **stock** list
/// `+0x75C` — itself built from the `nfs5.car` record's own stock-part ids (`+0x3BC` count,
/// `+0x3C2[i]` ids). `CarSim_applyParts` 0x49CB50 then reads `+0x758` into the race's `car+0x768`
/// (`career.md` §11.2d: "the race's parts … see the stock copies because they read `+0x758`").
/// So **every** racer — the career's, the market's, an opponent's, and (since 0x4EE9A0 runs
/// unconditionally in the constructor, not gated on any career state) a quick race's — starts a
/// race with its stock parts installed unless the career swapped one out.
///
/// This port has no career state for a quick race or a non-career opponent, so it has nothing to
/// swap out: `ids` is the car's `nfs5.car` record's stock-part ids as-is (`CarCatalogEntry::
/// stock_parts`, nfs-formats). `table` is the `nfs5.prt` records (`nfs_formats::career::Part`),
/// resolved by id; an id the table has no record for (or an empty/unavailable table) is skipped.
/// Not carried: `+0x758`'s exchange sort (0x49C350) — order does not change any getter's mean over
/// a mask, and at most one part occupies a given type+category slot in a shipped stock list, so no
/// getter's answer depends on it. Nothing resolved is an empty list (0x49C4C0's zeroed records match no getter kind).
pub fn stock_parts(ids: &[i16], table: &[nfs_formats::career::Part]) -> Vec<InstalledPart> {
    ids.iter()
        .filter_map(|id| table.iter().find(|p| p.id == *id))
        .map(|p| InstalledPart::new(p.kind, p.zone_mask.map(|b| b != 0), p.condition, p.state as f32))
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct CarSpecs {
    pub name: String,
    pub mass: f32,
    /// The installed-part list `car+0x768` the part-damage getters walk ([`InstalledPart`]):
    /// [`stock_parts`] for a racer with no career state (a quick race, a non-career opponent), the
    /// career's own installed parts when it has them (an empty result is an empty list).
    pub parts: Vec<InstalledPart>,
    /// wheelbase in metres (+0x44 ×0.001)
    pub wheelbase: f32,
    /// 0 NA, 1 turbo, 2 supercharged (+0x4C)
    pub induction: u32,
    /// forward gears + 2 (reverse, neutral) (+0x50 after +2)
    pub gear_slots: usize,
    /// shift duration in physics ticks (+0x54)
    pub shift_ticks: u32,
    /// drivetrain efficiency (+0x58 ×0.01)
    pub efficiency: f32,
    /// front drive fraction (+0x5C ×0.01): 0 RWD, 1 FWD, 0.5 AWD
    pub front_drive: f32,
    /// final drive (+0x80), and the raw per-slot gear ratios (+0x60..+0x7C) **before** 0x49D634
    /// overwrites them in place with `ratio'`. The original keeps only the overwritten form, but
    /// the car-setup sliders add to the raw ratios and to the final drive (`apply_setup`), so the
    /// port keeps both and derives `ratio_rpm_per_mps` from them.
    pub final_drive: f32,
    pub gear_ratio: [f32; 8],
    /// `ratio'[slot] = final * ratio * 60 / circumference` = rpm per m/s; slot 0 = reverse (negative), 1 = neutral (0)
    pub ratio_rpm_per_mps: [f32; 8],
    /// top speed m/s (+0x90 ×0.44643)
    pub top_speed: f32,
    pub redline: f32,
    /// engine-braking factor (+0x98 ×0.01, 0x49C880): `Engine_update`'s over-run arm returns
    /// `−a · specs[0x98] · …` (0x49ECA0, its only reader) — not a rev rate
    pub engine_brake: f32,
    /// torque curve, 21 points at 0, 500, … 10000 rpm (file units, lb·ft)
    pub torque: [f32; 21],
    /// front static load fraction (+0xF0 ×0.01)
    pub front_weight: f32,
    /// grip multiplier (+0xF4)
    pub grip: f32,
    /// weight-transfer factor (+0xF8 ×0.01)
    pub weight_transfer: f32,
    /// max steering angle in 1/256 turn (+0xFC)
    pub max_steer: f32,
    /// steering asymmetry (+0x100)
    pub steer_offset: f32,
    /// brake deceleration m/s² (+0x104)
    pub brake_decel: f32,
    /// front brake share (+0x108 ×0.01)
    pub front_brake: f32,
    /// downforce front/rear, m/s² per m/s (+0x10C/+0x110 ×0.001)
    pub downforce_front: f32,
    pub downforce_rear: f32,
    /// weight-transfer divisor (+0x11C)
    pub transfer_div: f32,
    /// lateral transfer divisor (+0x130)
    pub lateral_div: f32,
    /// tyre circumference m (derived +0x148)
    pub circumference: f32,
    /// tyre width in mm (+0x84), which also sizes the skid marks (0x478EA0)
    pub tyre_width_mm: f32,
    /// 1 / ratio' per slot (m/s per rpm) (+0x170); 0.1 when ratio is 0
    pub mps_per_rpm: [f32; 8],
    /// acceleration per unit torque per slot (+0x190)
    pub accel_per_torque: [f32; 8],
    /// drag coefficient (+0x1B0) = a_top / v_top³
    pub drag: f32,
    /// yaw lever (+0x1B4) = 3.2 * wheelbase, and its inverse (+0x1B8)
    pub yaw_lever: f32,
    pub inv_yaw_lever: f32,
    /// 1 / grip (+0x1BC)
    pub inv_grip: f32,
    /// grip-table column: **2** for the GT1/GT2/GT3/935 race cars, **0** otherwise (+0x1C0)
    pub class: usize,
    /// pitch / roll rate retention per tick (+0x114/+0x118 ×0.01) — `RE/notes/suspension-collision.md` §8
    pub pitch_ret: f32,
    pub roll_ret: f32,
    /// wheel probe offset m (+0x120), bump travel m (+0x124)
    pub probe_offset: f32,
    pub bump_travel: f32,
    /// dive gain (+0x128 ×0.001), roll gain (+0x12C ×0.001)
    pub dive_gain: f32,
    pub roll_gain: f32,
    /// pitch / roll injection clamps, turns/s (+0x134/+0x138)
    pub pitch_clamp: f32,
    pub roll_clamp: f32,
    /// a driver-name cheat already folded in (`apply_driver_name`)
    pub driver_cheat: bool,
}

impl CarSpecs {
    /// Apply the loader's conversions to a parsed `.sim`. `race_class` selects the grip column
    /// (true for GT1/GT2/GT3/935 — decided by the game from the car family; HIGH).
    pub fn from_sim(sim: &CarSim, race_class: bool) -> Self {
        let t = &sim.unknown_tail; // +0xF0 ..
        let mass = sim.mass_kg;
        let circumference = std::f32::consts::PI / 1000.0 * (2.0 * sim.tyre_width_mm * sim.tyre_aspect / 100.0 + sim.rim_inches * 25.4);
        let top_speed = sim.top_speed_mph * 0.44643;
        let efficiency = sim.unknown_58_pct * 0.01;
        let gear_slots = (sim.gear_count + 2) as usize;
        let mut ratio = [0f32; 8];
        ratio[0] = sim.reverse_ratio;
        for (i, g) in sim.gear_ratios.iter().enumerate() {
            ratio[2 + i] = *g;
        }
        let torque = sim.torque_curve;
        let mut out = Self {
            name: sim.name.clone(),
            parts: fallback_parts(),
            mass,
            wheelbase: sim.wheelbase_mm * 0.001,
            induction: sim.induction,
            gear_slots,
            shift_ticks: sim.unknown_54,
            efficiency,
            front_drive: sim.unknown_5c_pct * 0.01,
            final_drive: sim.final_drive,
            gear_ratio: ratio,
            ratio_rpm_per_mps: [0.0; 8],
            top_speed,
            redline: sim.redline_rpm,
            engine_brake: sim.unknown_98_pct * 0.01,
            torque,
            front_weight: t[0] * 0.01,
            grip: t[1],
            weight_transfer: t[2] * 0.01,
            max_steer: t[3],
            steer_offset: t[4],
            brake_decel: t[5],
            front_brake: t[6] * 0.01,
            downforce_front: t[7] * 0.001,
            downforce_rear: t[8] * 0.001,
            transfer_div: t[11],
            lateral_div: t[16],
            circumference,
            tyre_width_mm: sim.tyre_width_mm,
            mps_per_rpm: [0.1; 8],
            accel_per_torque: [0.0; 8],
            drag: 0.0,
            yaw_lever: 0.0,
            inv_yaw_lever: 0.0,
            inv_grip: 0.0,
            // **2 for a race car, 0 for everything else** — this was the wrong way round here
            // and in three notes. `CarSim_applyParts` 0x49CB50 compares `carInfo+0x2A8` with
            // "GT1", "GT2", "GT3" and "935" through 0x5AE3C0, which returns **0 on a match**, so
            // each `je 0x49CBE3` is the *match* arm and 0x49CBE9 stores **2**; the fall-through
            // at 0x49CBD1, reached only when none matched, stores **0**. A race car therefore
            // takes grip column 2 and friction-circle softness `{8, 8, 6}[2]` = 6, and a road car
            // column 0 and 8 — the port had them swapped.
            class: if race_class { 2 } else { 0 },
            pitch_ret: t[9] * 0.01,
            roll_ret: t[10] * 0.01,
            probe_offset: t[12] * 0.001,
            bump_travel: t[13] * 0.001,
            dive_gain: t[14] * 0.001,
            roll_gain: t[15] * 0.001,
            pitch_clamp: t[17],
            roll_clamp: t[18],
            driver_cheat: false,
        };
        // 0x49CB50's tail, which the loader always runs: the three gear tables, the drag term, the
        // yaw lever and 1/grip. `automatic` = `carinfo+0xC != 0`, which the `.sim` does not carry —
        // the gearbox is the player's, so the loader's own pass is the automatic one and a manual
        // box re-derives the tables through [`CarSpecs::rebuild_tables`].
        out.rebuild_tables(true);
        out
    }

    pub fn torque_at(&self, rpm: f32) -> f32 {
        torque_at(&self.torque, rpm)
    }

    /// **The purchased parts, folded into the block** — `0x49CB50 … 0x49D28A`, the first half of
    /// `CarSim_applyParts`. CONFIRMED, read instruction by instruction 2026-09-21.
    ///
    /// Runs on the block the `.sim` loader has just filled, **before** [`CarSpecs::apply_setup`].
    /// Returns the ratio grip ended at over the grip it started with — `car+0x10D8`, which the
    /// `.ais` loader rescales the AI's cornering table by (`loaders.md` §1.4).
    ///
    /// Two arms come before the loop. `family == "959"` sets `front_weight = 0.4675` outright
    /// (0x49CCAB), and **families `959`, `928`, `GT2` and `GT3` skip the parts entirely**
    /// (0x49CCC5…0x49CD07) — for those four the purchased list is ignored, whatever is in it.
    ///
    /// Then, for each of the twenty slots whose `kind` is not 0: `mass += +0x54`,
    /// `top_speed /= 1 + 0.01·(+0x58)`, and one arm by `kind`. `+0xC0` and `+0xC4` are the two
    /// tuning values, and in every arm **the value that leaves the block unchanged is 1** — which
    /// is a good check on the record layout, and is what the standard brake, spring and shock
    /// carry (`nfs5.prt`: kind 2 and 4 are 1…2.5, kind 5 is 0.8…2, kind 8 is 1.2…3).
    ///
    /// | kind | name | effect |
    /// |---|---|---|
    /// | 1 engine | `"Engine"` | replaces the whole 21-point torque curve with the part's own (`+0x110`), and **stops there** |
    /// | 1 engine | `"Turbo"` / `"Supercharged"` | `induction = 1` / `2`, then falls through |
    /// | 1 engine | anything else | `torque[i] += part_curve[i] · 0.67` |
    /// | 2 brakes | | `brake_decel *= (c0 + 3)·0.25` |
    /// | 3 gearbox | `"Gear Box"` | `shift_ticks = (i16)+0xEC`; `gear_slots = (i16)+0xEA + 2`; `final_drive = c0`; the eight raw ratios from `+0xF0` |
    /// | 3 gearbox | anything else (`"Shifter"`) | `shift_ticks += (i16)+0xEC` |
    /// | 4 springs | | `transfer_div = (c0 + 4)·transfer_div·0.2`; `lateral_div` likewise; `grip = (c0 + 15)·grip·0.0625`; `roll_gain /= (c0 + 3)·0.25`; **`probe_offset = c4·0.001`**, an overwrite — the spring's ride height in millimetres, 0/5/15/20 in the file |
    /// | 5 shocks | | `pitch_ret /= (c0 + 4)·0.2`; `roll_ret` likewise; `grip = (c0 + 11)·grip/12` |
    /// | 8 sway bars | | `transfer_div = (c0 + 4)·transfer_div·0.2`; `grip = (c0 + 11)·grip/12`; `roll_gain /= (c0 + 3)·0.25` |
    /// | 6 aero | | `downforce_front *= 1 + c0·0.005`; `downforce_rear *= 1 + c4·0.005` |
    /// | 9 tyres | | `G = 3·grip`, `u = (G + c0)/(G + 1)`, `v = (G + c4)/(G + 1)`; `grip = (u + v + 1)·grip/3`; `front_weight *= (2u/(u+v) + 1)·0.5`; and **`class = +0xC8`**, the grip-table column, which is 0, 1 or 2 in the file |
    /// | 7, 10 | weight reduction, bodywork | nothing beyond the mass and the top-speed per cent — every one of those 108 records has `+0xC0 = +0xC4 = 0` |
    ///
    /// So `+0xC8` is the **tyre grip class** and not an induction type: the induction comes from the
    /// engine part's *name*, and the four values `+0xC8` takes on kind-9 records are 0, 1 and 2.
    ///
    /// After the loop (0x49D212…0x49D28A) the top speed is rescaled by how much the torque peak
    /// moved — `top_speed *= (peak_after/peak_before + 3)·0.25`, both peaks being `max(0, curve)`
    /// over the 21 points — and the AI's steering lookahead by the grip ratio,
    /// `bend_lookahead /= (ratio + 2)/3`. The port has no `+0x140` field, so that last division is
    /// returned to the caller with the ratio rather than applied here.
    ///
    /// Not modelled: `specs[0x14C] = 1/redline` (0x49CC1A), which nothing reads back; and an engine
    /// part's `+0xC0`, which holds a redline (5500…7300 in the file) that this arm never consumes.
    pub fn apply_parts(&mut self, parts: &[&nfs_formats::career::Part], family: &str) -> f32 {
        let peak_before = self.torque.iter().copied().fold(0.0, f32::max);
        let grip_before = self.grip;
        if family == "959" {
            self.front_weight = 0.4675;
        }
        if !NO_PARTS_FAMILIES.contains(&family) {
            for p in parts {
                if p.kind == 0 {
                    continue;
                }
                self.mass += p.mass_delta;
                self.top_speed /= p.top_speed_pct * 0.01 + 1.0;
                let (c0, c4) = (p.value_c0, p.boost);
                match p.kind {
                    1 => {
                        if p.kind_name == "Engine" {
                            self.torque = p.torque_curve;
                        } else {
                            match p.kind_name.as_str() {
                                "Turbo" => self.induction = 1,
                                "Supercharged" => self.induction = 2,
                                _ => {}
                            }
                            for (t, a) in self.torque.iter_mut().zip(p.torque_curve) {
                                *t += a * 0.67;
                            }
                        }
                    }
                    2 => self.brake_decel *= (c0 + 3.0) * 0.25,
                    3 => {
                        if p.kind_name == "Gear Box" {
                            self.shift_ticks = p.gearbox_param as u32;
                            self.gear_slots = (p.forward_gears + 2) as usize;
                            self.final_drive = c0;
                            self.gear_ratio = p.gear_ratios;
                        } else {
                            self.shift_ticks = self.shift_ticks.wrapping_add(p.gearbox_param as u32);
                        }
                    }
                    4 => {
                        self.transfer_div = (c0 + 4.0) * self.transfer_div * 0.2;
                        self.lateral_div = (c0 + 4.0) * self.lateral_div * 0.2;
                        self.grip = (c0 + 15.0) * self.grip * 0.0625;
                        self.roll_gain /= (c0 + 3.0) * 0.25;
                        self.probe_offset = c4 * 0.001;
                    }
                    5 => {
                        self.pitch_ret /= (c0 + 4.0) * 0.2;
                        self.roll_ret /= (c0 + 4.0) * 0.2;
                        self.grip = (c0 + 11.0) * self.grip * (1.0 / 12.0);
                    }
                    8 => {
                        self.transfer_div = (c0 + 4.0) * self.transfer_div * 0.2;
                        self.grip = (c0 + 11.0) * self.grip * (1.0 / 12.0);
                        self.roll_gain /= (c0 + 3.0) * 0.25;
                    }
                    6 => {
                        self.downforce_front *= c0 * 0.005 + 1.0;
                        self.downforce_rear *= c4 * 0.005 + 1.0;
                    }
                    9 => {
                        let g = self.grip * 3.0;
                        let u = (g + c0) / (g + 1.0);
                        let v = (g + c4) / (g + 1.0);
                        self.grip = (u + v + 1.0) * self.grip * (1.0 / 3.0);
                        self.front_weight *= (2.0 * u / (u + v) + 1.0) * 0.5;
                        self.class = p.grip_class as usize;
                    }
                    _ => {}
                }
            }
        }
        let peak_after = self.torque.iter().copied().fold(0.0, f32::max);
        self.top_speed *= (peak_after / peak_before + 3.0) * 0.25;
        self.rebuild_tables(true);
        self.grip / grip_before
    }

    /// **The seventeen car-setup sliders, folded into the block** — `0x49D290 … 0x49D62C`,
    /// the second half of `CarSim_applyParts` 0x49CB50. CONFIRMED, read instruction by
    /// instruction 2026-09-21.
    ///
    /// The sliders do not reach the physics as floats. The car-setup screen stores them in the
    /// `FECar` at `+0x6D0 … +0x710`, seventeen consecutive floats in [`CAR_SETUP_NAMES`] order
    /// (the constructor 0x4EED2D writes 50/50/50, three zeroes, 1.0, 30/30 and eight zeroes
    /// there, and copies them again to `+0x714` as the reset copy); race setup 0x4EF188…0x4EF32F
    /// pushes each through **0x4A5400** = `(int)(f × 65536)` — `_ftol`, so a **truncation** — into
    /// the named registry parameter, and this block reads them back out of the per-player
    /// `carinfo` record with `fild` and `× 2⁻¹⁶` ([0x5B253C]). [`fixed16`] is that round trip.
    ///
    /// | carinfo | slider | effect on the block |
    /// |---|---|---|
    /// | +0x114 / +0x118 `.FRONT/REARDOWNFORCE` | `FDownForce`, `RDownForce` | `pf = (v·0.02 + 1)·0.5` and `pr` likewise; `top_speed /= (pf + pr + 3)·0.2`; `downforce_front *= pf`; `downforce_rear *= pr` |
    /// | +0xF0 `.FINALRATIO` | `Gear7` | `final_drive += v` |
    /// | +0xF4, +0xFC…+0x110 `.REVERSERATIO`, `.FIRST…SIXTHRATIO` | `Gear0`, `Gear1…Gear6` | `gear_ratio[slot] += v`; the loop steps the block's own slots and **skips slot 1**, so carinfo `+0xF8` — the neutral — is the one dword of the eight nothing ever reads |
    /// | +0x11C `.BRAKEBIAS` | `BrakeBias` | `front_brake += (trunc(fixed/100)·2⁻¹⁶ − 0.5)·0.5`, an **integer** divide of the fixed-point word (0x49D38C `imul 0x51EB851F`), not a float one |
    /// | +0x128 `.STEERINGTOE` | `SteeringToe` | `steer_offset += v` |
    /// | +0x134 `.SHOCKRATE` | `ShockRate` | read three times: `transfer_div = (v + 2)·transfer_div/3`; `grip = (transfer_div + 9)·0.1·grip`; `pitch_ret += (1 − v)·0.2` and `roll_ret` likewise; `roll_gain /= (transfer_div + 1)·0.5` |
    /// | +0x124 `.SUSPENSIONTRAVEL` | `Suspension` | `r = v·0.001`; `bump_travel += r`; `grip *= 1 + r`; `roll_gain /= 1 + r` |
    /// | +0x120 `.RIDEHEIGHT` | `RideHeight` | `probe_offset = v·0.001` — an **overwrite**, mm to metres, which is why every shipped `.sim` carries 0 there and the wheel probe still has an offset; then `roll_gain /= 6·probe_offset + 1` |
    /// | +0x13C / +0x140 `.FRONT/REARTIREPRESSURE` | `FTirePressure`, `RTirePressure` | `a = v/30`, `b` likewise, and **each below 1.0 is multiplied by 10/11** (0x5B3354); `grip = (a + b + 8)·grip·0.1`; `front_weight *= (2a/(a+b) + 1)·0.5` |
    ///
    /// Every default is neutral: 50/50 gives `pf = pr = 1` and a divisor of 1, bias 50 gives
    /// exactly 0, shock rate 1 leaves `transfer_div` alone and `grip ×(1+9)·0.1`, and 30 psi gives
    /// `a = b = 1` and `grip ×(1+1+8)·0.1`.
    ///
    /// Not modelled, and deliberately: the two driver-name cheats that follow at 0x49D6F3 /
    /// 0x49D84B (`"SILSPD"` multiplies every gear's acceleration by 1.2 and then grip by 1.2,
    /// `roll_gain` by 0.8 and `roll_ret` by 10/11; `"ROLLEY"` grip by 1.5 and `roll_gain` by 1.33),
    /// because this port has no typed driver name to trigger them. They are **not** exclusive with
    /// the manual-gearbox 1.05 — the SILSPD arm falls through into it.
    pub fn apply_setup(&mut self, setup: &[f32; 17], automatic: bool) {
        // 0x49D290
        let pf = (fixed16(setup[0]) * 0.02 + 1.0) * 0.5;
        let pr = (fixed16(setup[1]) * 0.02 + 1.0) * 0.5;
        self.top_speed /= (pf + pr + 3.0) * 0.2;
        self.downforce_front *= pf;
        self.downforce_rear *= pr;
        // 0x49D32E, then the slot loop 0x49D346 — slot 1 (neutral) is skipped
        self.final_drive += fixed16(setup[16]);
        // `Gear0` is the reverse ratio and `Gear1 … Gear6` are first to sixth, so slider 9 + n
        // lands on block slot n + 1 from first gear on, and the neutral slot has no slider at all.
        for (slot, slider) in [(0usize, 9usize), (2, 10), (3, 11), (4, 12), (5, 13), (6, 14), (7, 15)] {
            self.gear_ratio[slot] += fixed16(setup[slider]);
        }
        // 0x49D386 — the fixed-point word divided by 100 as an integer
        let bias = (setup[2] * 65536.0).trunc() as i32 / 100;
        self.front_brake += (bias as f32 / 65536.0 - 0.5) * 0.5;
        // 0x49D3CF
        self.steer_offset += fixed16(setup[5]);
        // 0x49D3F3 — shock rate, read three times
        let shock = fixed16(setup[6]);
        self.transfer_div = (shock + 2.0) * self.transfer_div * (1.0 / 3.0);
        self.grip *= (self.transfer_div + 9.0) * 0.1;
        self.pitch_ret += (1.0 - shock) * 0.2;
        self.roll_ret += (1.0 - shock) * 0.2;
        self.roll_gain /= (self.transfer_div + 1.0) * 0.5;
        // 0x49D4CB — suspension travel, also read three times
        let travel = fixed16(setup[4]) * 0.001;
        self.bump_travel += travel;
        self.grip *= travel + 1.0;
        self.roll_gain /= travel + 1.0;
        // 0x49D555 — ride height, an overwrite
        self.probe_offset = fixed16(setup[3]) * 0.001;
        self.roll_gain /= self.probe_offset * 6.0 + 1.0;
        // 0x49D597 — the tyre pressures
        let mut a = fixed16(setup[7]) * (1.0 / 30.0);
        let mut b = fixed16(setup[8]) * (1.0 / 30.0);
        if a < 1.0 {
            a *= 0.909_091;
        }
        if b < 1.0 {
            b *= 0.909_091;
        }
        self.grip = (a + b + 8.0) * self.grip * 0.1;
        self.front_weight *= (2.0 * a / (a + b) + 1.0) * 0.5;
        self.rebuild_tables(automatic);
    }

    /// **The derived tables, rebuilt** — `0x49D634 … 0x49D99D`, the tail every path through
    /// `CarSim_applyParts` runs once the parts and the sliders have moved the block. CONFIRMED.
    ///
    /// * 0x49D634: `ratio'[slot] = final_drive · ratio · 60 / circumference` for all eight slots;
    /// * 0x49D67D, for the `gear_slots` live slots only: `mps_per_rpm = 1/ratio'`, or **0.1** when
    ///   the ratio is exactly 0 (0x49D6A4 stores `0x3DCCCCCD`), and
    ///   `accel_per_torque = efficiency · (ratio'/mass · 0.1)`, times **1.05 when the gearbox is
    ///   manual** — `carinfo+0xC == 0`, 0x49D733;
    /// * 0x49D757 writes a fourth table, `ratio'[i]/redline` at `+0x150`, which **nothing in the
    ///   image ever reads back**; it is not modelled;
    /// * 0x49D87B: `drag = T(rpm_top) · accel_per_torque[top] / v_top³`, with
    ///   `rpm_top = clamp(trunc(ratio'[top] · v_top), 0, redline)` — the truncation is `_ftol`
    ///   at 0x49D88E and happens **before** the clamp;
    /// * 0x49D934: `yaw_lever = wheelbase · 0.5 · 0.025 · 256` (= 3.2·wheelbase) and its inverse;
    /// * 0x49D991: `inv_grip = 1 / grip`, which is why grip has to be settled first.
    /// **The driver-name cheats** of the same tail (0x49D6E7…0x49D875), which compare the car's
    /// typed driver name `carinfo+0x17C` with `"SILSPD"` (0x5D0CF0) and `"ROLLEY"` (0x5D0CE8):
    ///
    /// * `SILSPD`: in the gear loop each live slot's `accel_per_torque` takes **×1.2 in place of**
    ///   the manual ×1.05 (0x49D71F — the match skips the `carinfo+0xC` test); after it `grip`
    ///   ×1.2, `roll_gain` (+0x12C) ×0.8 and `roll_ret` (+0x118) ×10/11 (0x49D7C9…0x49D80B);
    /// * `ROLLEY`: `grip` ×1.5 and `roll_gain` ×1.33 (0x49D84B…0x49D875).
    ///
    /// The drag term and `1/grip` that follow are taken again. Applied once per car build; the
    /// flag keeps a restarted race from compounding it.
    pub fn apply_driver_name(&mut self, name: &str, automatic: bool) {
        if self.driver_cheat {
            return;
        }
        match name {
            "SILSPD" => {
                let f = if automatic { 1.2 } else { 1.2 / 1.05 };
                for a in self.accel_per_torque.iter_mut().take(self.gear_slots.min(8)) {
                    *a *= f;
                }
                self.grip *= 1.2;
                self.roll_gain *= 0.8;
                self.roll_ret *= f32::from_bits(0x3f68ba2e);
            }
            "ROLLEY" => {
                self.grip *= 1.5;
                self.roll_gain *= 1.33;
            }
            _ => return,
        }
        self.driver_cheat = true;
        let top = self.gear_slots - 1;
        let rpm_top = (self.ratio_rpm_per_mps[top] * self.top_speed).trunc().min(self.redline).max(0.0);
        self.drag = torque_at(&self.torque, rpm_top) * self.accel_per_torque[top] / (self.top_speed * self.top_speed * self.top_speed);
        self.inv_grip = 1.0 / self.grip;
    }

    /// `.TINYCAR` (`carinfo+0x518`, the Gulliver code, cheat bit 1 — on every car but traffic,
    /// 0x4EF46D…0x4EF489) as the `.sim` loader applies it, 0x49C9B2…0x49CA10: the wheelbase ×0.8,
    /// the pitch and roll retentions (+0x114 / +0x118) ×`[0x5B4A24]` = 0.8333 and the grip ×1.25,
    /// ahead of the tables that derive from them (the yaw lever, `1/grip`).
    pub fn apply_tinycar(&mut self) {
        let k = f32::from_bits(0x3F555555);
        self.wheelbase *= 0.8;
        self.pitch_ret *= k;
        self.roll_ret *= k;
        self.grip *= 1.25;
        self.yaw_lever = self.wheelbase * 0.5 * 0.025 * 256.0;
        self.inv_yaw_lever = 1.0 / self.yaw_lever;
        self.inv_grip = 1.0 / self.grip;
    }

    /// `carSpecs[0x60 + 4·slot]` as the tables leave it: slots 0…7 are `ratio'` — 0x49D634's loop
    /// overwrites all eight, `+0x60 … +0x7C`, whatever the gear count (`cmp ecx, 0x80; jl`) — and
    /// slot 8 is `+0x80` itself, the final drive, which that loop multiplies by and leaves as it
    /// is. What 0x408495…0x4084BD reads for the traffic / cop engine speed with 0x408130's gear,
    /// which reaches slot 8 on a six-speed car past its top gear. Past slot 8 (not reachable:
    /// `carSpecs[0x50]` is at most 8) 0.
    pub fn ratio_slot(&self, slot: usize) -> f32 {
        match slot {
            0..=7 => self.ratio_rpm_per_mps[slot],
            8 => self.final_drive,
            _ => 0.0,
        }
    }

    pub fn rebuild_tables(&mut self, automatic: bool) {
        for i in 0..8 {
            self.ratio_rpm_per_mps[i] = self.final_drive * self.gear_ratio[i] * 60.0 / self.circumference;
        }
        for i in 0..self.gear_slots.min(8) {
            let r = self.ratio_rpm_per_mps[i];
            self.mps_per_rpm[i] = if r == 0.0 { 0.1 } else { 1.0 / r };
            let mut a = self.efficiency * (r / self.mass * 0.1);
            if !automatic {
                a *= 1.05;
            }
            self.accel_per_torque[i] = a;
        }
        let top = self.gear_slots - 1;
        let rpm_top = (self.ratio_rpm_per_mps[top] * self.top_speed).trunc().min(self.redline).max(0.0);
        self.drag = torque_at(&self.torque, rpm_top) * self.accel_per_torque[top] / (self.top_speed * self.top_speed * self.top_speed);
        self.yaw_lever = self.wheelbase * 0.5 * 0.025 * 256.0;
        self.inv_yaw_lever = 1.0 / self.yaw_lever;
        self.inv_grip = 1.0 / self.grip;
    }
}

/// The round trip every car-setup slider makes on its way into the physics: `0x4A5400` turns the
/// float into 16.16 fixed point by truncating `f × 65536`, and 0x49D29C and its thirteen siblings
/// read it back with `fild` and `× [0x5B253C] = 2⁻¹⁶`. Integer sliders come back exactly; a shock
/// rate of 0.7 comes back as 0.699996948.
/// The four AI families whose purchased parts `CarSim_applyParts` ignores outright — 0x49CCC5,
/// 0x49CCDB, 0x49CCF1 and 0x49CD07, each an `je` straight past the loop to the tail.
pub const NO_PARTS_FAMILIES: [&str; 4] = ["959", "928", "GT2", "GT3"];

pub fn fixed16(v: f32) -> f32 {
    (v * 65536.0).trunc() / 65536.0
}

/// Torque lookup: `idx = int(rpm*0.002)`, linear interpolation (CONFIRMED, 0x49E90C).
pub fn torque_at(curve: &[f32; 21], rpm: f32) -> f32 {
    let r = rpm.max(0.0);
    let idx = ((r * 0.002) as usize).min(19);
    let t = (r - 500.0 * idx as f32) * 0.002;
    curve[idx] + t * (curve[idx + 1] - curve[idx])
}

/// Dry grip table `0x5D0D50` float[16][3] (rows = surface type, cols = class 0/1/2).
pub const GRIP_DRY: [[f32; 3]; 16] = [
    [0.5, 0.5, 0.5], [1.0, 0.9, 1.1], [0.85, 0.9, 0.85], [0.7, 0.775, 0.625], [0.95, 0.9, 1.05], [0.85, 0.875, 0.8], [0.7, 0.8, 0.6], [0.9, 0.9, 1.0],
    [0.6, 0.75, 0.5], [0.675, 0.75, 0.6], [0.85, 0.8, 0.95], [0.9, 0.85, 0.9], [1.0; 3], [1.0; 3], [1.0; 3], [1.0; 3],
];
/// Wet grip table `0x5D0E4C`.
pub const GRIP_WET: [[f32; 3]; 16] = [
    [0.5, 0.5, 0.5], [0.9, 1.0, 1.1], [0.9, 0.85, 0.85], [0.775, 0.7, 0.625], [0.9, 0.95, 1.05], [0.875, 0.85, 0.8], [0.8, 0.7, 0.6], [0.9, 0.9, 1.0],
    [0.75, 0.6, 0.5], [0.75, 0.675, 0.6], [0.8, 0.85, 0.95], [0.85, 0.9, 0.9], [1.0; 3], [1.0; 3], [1.0; 3], [1.0; 3],
];
/// Surface landing table `0x5D0B94` float[16][2].
pub const SURFACE_BOUNCE: [[f32; 2]; 16] = [
    [0.5, 0.5], [0.4, 0.7], [0.6, 0.5], [0.6, 0.5], [0.4, 0.7], [0.6, 0.5], [0.25, 0.5], [0.4, 0.4], [0.1, 0.7], [0.2, 0.5], [0.55, 0.6], [0.4, 0.5], [1.0, 1.0], [1.0, 1.0], [1.0, 1.0], [1.0, 1.0],
];
/// Friction-circle softness per class `0x5D0F58`.
pub const CIRCLE_SOFTNESS: [f32; 3] = [8.0, 8.0, 6.0];

#[cfg(test)]
mod stock_parts_tests {
    use super::*;
    use nfs_formats::career::Part;

    fn part(id: i16, kind: u16, mask: [u8; 10], condition: f32, state: u32) -> Part {
        Part {
            id,
            name_string_id: 0,
            family: String::new(),
            unknown_24: 0,
            kind,
            kind_name: String::new(),
            year: 0,
            zone_mask: mask,
            mass_delta: 0.0,
            top_speed_pct: 0.0,
            price: 0,
            condition,
            geometry_swaps: Vec::new(),
            value_c0: 1.0,
            boost: 0.0,
            grip_class: 0,
            forward_gears: 0,
            gearbox_param: 0,
            gear_ratios: [0.0; 8],
            torque_curve: [0.0; 21],
            state,
        }
    }

    /// `specs.rs`'s `stock_parts`, the port of `FECar::init` 0x4EEBA0's unconditional
    /// `fit_stock` 0x4EE9A0: given the record's stock ids and the `nfs5.prt` table, resolves to
    /// one [`InstalledPart`] per id, in the record's own order, each taking that record's own
    /// `kind` / mask / condition / carried state — not the six-getter, scale-1.5,
    /// no-carried-state placeholder [`fallback_parts`] is.
    #[test]
    fn resolves_the_records_stock_ids() {
        let table = vec![
            part(10, 2, [1, 0, 1, 0, 1, 0, 1, 0, 0, 0], 100.0, 0),
            part(20, 9, [1, 0, 1, 0, 1, 0, 1, 0, 0, 0], 90.0, 5),
            part(30, 1, [0, 0, 0, 0, 1, 1, 1, 0, 0, 0], 100.0, 0),
        ];
        let got = stock_parts(&[10, 30], &table);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].kind, 2);
        // condition 100 -> scale 1.5 - 0.005*100 = 1.0, not fallback_parts' hardcoded 1.5
        assert!((got[0].scale - 1.0).abs() < 1e-6, "scale = {}", got[0].scale);
        assert_eq!(got[0].state, 0.0);
        assert_eq!(got[1].kind, 1);
    }

    /// A record's own carried damage (`+0x164`, `Part::state`) comes through, unlike
    /// [`fallback_parts`]'s hardcoded 0.
    #[test]
    fn carries_the_records_own_state() {
        let table = vec![part(20, 9, [1; 10], 90.0, 5)];
        let got = stock_parts(&[20], &table);
        assert_eq!(got.len(), 1);
        assert!((got[0].scale - (1.5 - 0.005 * 90.0)).abs() < 1e-6);
        assert_eq!(got[0].state, 5.0);
        assert_eq!(got[0].carried, 5.0);
    }

    /// No id resolves (a traffic car's empty stock list): an empty list, as 0x49C4C0's twenty
    /// zeroed records match no getter kind.
    #[test]
    fn stays_empty_when_nothing_resolves() {
        assert!(stock_parts(&[1, 2, 3], &[]).is_empty());
        assert!(stock_parts(&[], &[]).is_empty());
    }
}
