//! Runtime car specification = the `.sim` file after `CarSim_load` (0x49C6C0) unit conversions
//! and the derived tables built by 0x49CB50 / 0x49D6xx (`RE/notes/loaders.md`, `RE/notes/physics.md` §3).

use nfs_formats::sim::CarSim;

pub const GRAVITY: f32 = 9.8;

#[derive(Debug, Clone, PartialEq)]
pub struct CarSpecs {
    pub name: String,
    pub mass: f32,
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
    /// `ratio'[slot] = final * ratio * 60 / circumference` = rpm per m/s; slot 0 = reverse (negative), 1 = neutral (0)
    pub ratio_rpm_per_mps: [f32; 8],
    /// top speed m/s (+0x90 ×0.44643)
    pub top_speed: f32,
    pub redline: f32,
    /// engine rev-rate factor (+0x98 ×0.01)
    pub rev_rate: f32,
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
    /// grip-table column: 0 for GT1/GT2/GT3/935 race cars, 2 otherwise (+0x1C0)
    pub class: usize,
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
        let mut ratio_rpm_per_mps = [0f32; 8];
        let mut mps_per_rpm = [0.1f32; 8];
        let mut accel_per_torque = [0f32; 8];
        for i in 0..8 {
            let r = sim.final_drive * ratio[i] * 60.0 / circumference;
            ratio_rpm_per_mps[i] = r;
            if r != 0.0 {
                mps_per_rpm[i] = 1.0 / r;
            }
            // class multiplier {1.2 SILSPD | 1.05 manual | 1}: manual/AI variants not modelled → 1
            accel_per_torque[i] = r * 0.1 * efficiency / mass;
        }
        let torque = sim.torque_curve;
        let top = gear_slots - 1;
        let rpm_top = (ratio_rpm_per_mps[top] * top_speed).clamp(0.0, sim.redline_rpm);
        let drag = torque_at(&torque, rpm_top) * accel_per_torque[top] / (top_speed * top_speed * top_speed);
        let yaw_lever = 3.2 * sim.wheelbase_mm * 0.001;
        Self {
            name: sim.name.clone(),
            mass,
            wheelbase: sim.wheelbase_mm * 0.001,
            induction: sim.induction,
            gear_slots,
            shift_ticks: sim.unknown_54,
            efficiency,
            front_drive: sim.unknown_5c_pct * 0.01,
            ratio_rpm_per_mps,
            top_speed,
            redline: sim.redline_rpm,
            rev_rate: sim.unknown_98_pct * 0.01,
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
            mps_per_rpm,
            accel_per_torque,
            drag,
            yaw_lever,
            inv_yaw_lever: 1.0 / yaw_lever,
            inv_grip: 1.0 / t[1],
            class: if race_class { 0 } else { 2 },
        }
    }

    pub fn torque_at(&self, rpm: f32) -> f32 {
        torque_at(&self.torque, rpm)
    }
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
