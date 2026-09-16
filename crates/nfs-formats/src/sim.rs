//! `GameData/Simulation/CarData/*.sim` — per-car simulation parameters (328 bytes).
//!
//! Status: layout derived from side-by-side comparison against published Porsche specifications
//! and CONFIRMED by the loader `CarSim_load` at 0x49C6C0 (`RE/notes/loaders.md`), which copies the
//! 328 bytes into a 452-byte runtime block and converts units in place:
//! `+0x90` mph → m/s (×0.44643); `+0x58,+0x5C,+0x98,+0xF0,+0xF8,+0x108,+0x114,+0x118` percent → fraction
//! (×0.01); `+0x44` and `+0x10C,+0x110,+0x120,+0x124,+0x128,+0x12C` mm → m (×0.001); `+0x50` gear count
//! += 2 (adds R and N slots). Derived runtime fields: `1/redline`, tyre circumference =
//! π/1000·(2·width·aspect/100 + rim·25.4) m. This struct keeps the **file** units.

use crate::{f32_le, fixed_str, u32_le, FormatError};

pub const SIM_SIZE: usize = 328;

#[derive(Debug, Clone, PartialEq)]
pub struct CarSim {
    /// +0x00, 64 bytes.
    pub name: String,
    /// +0x40 — HIGH CONFIDENCE: mass in kg (993 Carrera = 1370).
    pub mass_kg: f32,
    /// +0x44 — HIGH CONFIDENCE: wheelbase in mm (993 = 2272, Boxster = 2418).
    pub wheelbase_mm: f32,
    /// +0x48 — u32; 0 for rear-engined 356/993/996, 1 for mid-engined Boxster/GT1 (HYPOTHESIS: engine placement).
    pub unknown_48: u32,
    /// +0x4C — induction type: 0 naturally aspirated, 1 turbo, 2 supercharged (CONFIRMED: written by the
    /// parts pass 0x49CB50).
    pub induction: u32,
    /// +0x50 — HIGH CONFIDENCE: number of forward gears.
    pub gear_count: u32,
    /// +0x54 — u32 (6,6,8,4,6): meaning unknown.
    pub unknown_54: u32,
    /// +0x58 — f32 percent (90, 90, 95, 70); loader converts to a fraction. Meaning unknown.
    pub unknown_58_pct: f32,
    /// +0x5C — f32 percent (0 in all samples); loader converts to a fraction.
    pub unknown_5c_pct: f32,
    /// +0x60 — HIGH CONFIDENCE: reverse gear ratio (negative).
    pub reverse_ratio: f32,
    /// +0x64 — 0 (neutral slot).
    pub neutral: f32,
    /// +0x68..+0x7C — HIGH CONFIDENCE: forward gear ratios 1..6 (unused = 0). 993: 3.818/2.15/1.56/1.242/1.024/0.82.
    /// The executable's "Gear0".."Gear7" strings match the 8 slots reverse/neutral/1..6.
    pub gear_ratios: [f32; 6],
    /// +0x80 — HIGH CONFIDENCE: final drive ratio (993 = 3.444).
    pub final_drive: f32,
    /// +0x84 — HIGH CONFIDENCE: tyre width mm (993 rear = 245).
    pub tyre_width_mm: f32,
    /// +0x88 — HIGH CONFIDENCE: tyre aspect ratio percent.
    pub tyre_aspect: f32,
    /// +0x8C — HIGH CONFIDENCE: rim diameter inches.
    pub rim_inches: f32,
    /// +0x90 — HIGH CONFIDENCE: top speed in mph (993 = 168 ≈ 270 km/h).
    pub top_speed_mph: f32,
    /// +0x94 — HIGH CONFIDENCE: redline rpm.
    pub redline_rpm: f32,
    /// +0x98 — f32 percent (20, 20, 20, 10, 25); loader converts to a fraction. Meaning unknown.
    pub unknown_98_pct: f32,
    /// +0x9C..+0xEC — engine torque curve (lb·ft in file, no conversion in the loader), **21** slots
    /// (CONFIRMED count from the loader), unused = 0. The rpm spacing is not yet established.
    pub torque_curve: [f32; 21],
    /// +0xF0 .. +0x148 — 22 floats. Loader units: +0xF0/+0xF8/+0x108/+0x114/+0x118 are percents,
    /// +0x10C/+0x110/+0x120/+0x124/+0x128/+0x12C are millimetres; the rest unscaled. Semantics open.
    pub unknown_tail: [f32; 22],
}

impl CarSim {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        if data.len() < SIM_SIZE {
            return Err(FormatError::Truncated { what: ".sim", offset: data.len(), needed: SIM_SIZE - data.len() });
        }
        let f = |o: usize| f32_le(data, o, ".sim field");
        let mut gear_ratios = [0f32; 6];
        for (i, g) in gear_ratios.iter_mut().enumerate() {
            *g = f(0x68 + i * 4)?;
        }
        let mut torque_curve = [0f32; 21];
        for (i, t) in torque_curve.iter_mut().enumerate() {
            *t = f(0x9C + i * 4)?;
        }
        let mut unknown_tail = [0f32; 22];
        for (i, t) in unknown_tail.iter_mut().enumerate() {
            *t = f(0xF0 + i * 4)?;
        }
        Ok(Self {
            name: fixed_str(data, 0, 64, ".sim name")?,
            mass_kg: f(0x40)?,
            wheelbase_mm: f(0x44)?,
            unknown_48: u32_le(data, 0x48, ".sim +0x48")?,
            induction: u32_le(data, 0x4C, ".sim induction")?,
            gear_count: u32_le(data, 0x50, ".sim gear count")?,
            unknown_54: u32_le(data, 0x54, ".sim +0x54")?,
            unknown_58_pct: f(0x58)?,
            unknown_5c_pct: f(0x5C)?,
            reverse_ratio: f(0x60)?,
            neutral: f(0x64)?,
            gear_ratios,
            final_drive: f(0x80)?,
            tyre_width_mm: f(0x84)?,
            tyre_aspect: f(0x88)?,
            rim_inches: f(0x8C)?,
            top_speed_mph: f(0x90)?,
            redline_rpm: f(0x94)?,
            unknown_98_pct: f(0x98)?,
            torque_curve,
            unknown_tail,
        })
    }
}
