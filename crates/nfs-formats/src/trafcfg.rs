//! `Simulation/AI/trafcfg.dat` — the gains of the AI drive model that traffic and police cars run
//! (`RE/notes/traffic.md` §16 and §27.2).
//!
//! A flat array of 27 little-endian `i32` tokens read by `0x406BA0`: three plain integers, two 16.16
//! fixed values, then **two identical 11-field chains**, "In Control" at `[0x5E6D1C]` and "Out Of
//! Control" at `[0x5E6D48]`. Every field is 16.16 fixed (`0x4A53F0` multiplies by `1/65536`) except
//! `max_dav`, `max_aa` and `ang_vel_limit_factor`, which the reader scales by `[0x5B25B8] = 1/1024`
//! (0x406C58, 0x406C7E). The shipped file's field names come from `TRAFCFG.DES` beside it.

use crate::{u32_le, FormatError};

/// One of the two gain chains.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AiPhysics {
    /// `+0x00` lateral position error → desired lateral velocity (0 in the out-of-control chain, which
    /// the shipped `.DES` marks "not used for ooc")
    pub dlpos_to_dlvel: f32,
    /// `+0x04` maximum desired lateral velocity
    pub max_dlvel: f32,
    /// `+0x08` lateral velocity error → lateral acceleration
    pub dlvel_to_clacc: f32,
    /// `+0x0C` marked NOT USED in the shipped description
    pub max_clacc: f32,
    /// `+0x10` heading error (turns) → desired angular velocity
    pub dangle_to_dav: f32,
    /// `+0x14` maximum desired angular velocity, scaled by 1/1024
    pub max_dav: f32,
    /// `+0x18` angular velocity error → angular acceleration
    pub dav_to_aa: f32,
    /// `+0x1C` marked NOT USED, scaled by 1/1024
    pub max_aa: f32,
    /// `+0x20` below this speed the lateral and angular commands are limited
    pub vel_limit_range: f32,
    /// `+0x24` lateral limit factor
    pub lat_vel_limit_factor: f32,
    /// `+0x28` angular limit factor, scaled by 1/1024
    pub ang_vel_limit_factor: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficConfig {
    /// `[0x5E6D08]` how far ahead the lateral velocity is measured, in slices
    pub latvelcalc_lookahead: i32,
    /// `[0x5E6D0C]` / `[0x5E6D10]` the lookahead clamps `0x4090C0` divides by 3
    pub min_lookahead: i32,
    pub max_lookahead: i32,
    /// `[0x5E6D14]` speed → slices ahead
    pub look_ahead_factor: f32,
    /// `[0x5E6D18]` front/rear skid threshold
    pub skid_value: f32,
    pub in_control: AiPhysics,
    pub out_of_control: AiPhysics,
}

impl Default for TrafficConfig {
    /// The shipped `TRAFCFG.DAT`, in case the file is missing.
    fn default() -> Self {
        Self {
            latvelcalc_lookahead: 4,
            min_lookahead: 12,
            max_lookahead: 30,
            look_ahead_factor: 45875.0 / 65536.0,
            skid_value: 11.0,
            in_control: AiPhysics {
                dlpos_to_dlvel: 8.0,
                max_dlvel: 32.0,
                dlvel_to_clacc: 8.0,
                max_clacc: 24.0,
                dangle_to_dav: 1.25,
                max_dav: 52428.0 / 1024.0,
                dav_to_aa: 5.0,
                max_aa: 52428.0 / 1024.0,
                vel_limit_range: 20.0,
                lat_vel_limit_factor: 8519.0 / 65536.0,
                ang_vel_limit_factor: 851.0 / 1024.0,
            },
            out_of_control: AiPhysics {
                dlpos_to_dlvel: 0.0,
                max_dlvel: 16.0,
                dlvel_to_clacc: 8.0,
                max_clacc: 24.0,
                dangle_to_dav: 0.625,
                max_dav: 52428.0 / 1024.0,
                dav_to_aa: 10.0,
                max_aa: 52428.0 / 1024.0,
                vel_limit_range: 10.0,
                lat_vel_limit_factor: 20971.0 / 65536.0,
                ang_vel_limit_factor: 851.0 / 1024.0,
            },
        }
    }
}

impl TrafficConfig {
    pub fn parse(data: &[u8]) -> Result<Self, FormatError> {
        let what = "trafcfg.dat";
        let t = |i: usize| -> Result<i32, FormatError> { Ok(u32_le(data, i * 4, what)? as i32) };
        let fx = |i: usize| -> Result<f32, FormatError> { Ok(t(i)? as f32 / 65536.0) };
        let k = |i: usize| -> Result<f32, FormatError> { Ok(t(i)? as f32 / 1024.0) };
        let chain = |b: usize| -> Result<AiPhysics, FormatError> {
            Ok(AiPhysics {
                dlpos_to_dlvel: fx(b)?,
                max_dlvel: fx(b + 1)?,
                dlvel_to_clacc: fx(b + 2)?,
                max_clacc: fx(b + 3)?,
                dangle_to_dav: fx(b + 4)?,
                max_dav: k(b + 5)?,
                dav_to_aa: fx(b + 6)?,
                max_aa: k(b + 7)?,
                vel_limit_range: fx(b + 8)?,
                lat_vel_limit_factor: fx(b + 9)?,
                ang_vel_limit_factor: k(b + 10)?,
            })
        };
        Ok(Self {
            latvelcalc_lookahead: t(0)?,
            min_lookahead: t(1)?,
            max_lookahead: t(2)?,
            look_ahead_factor: fx(3)?,
            skid_value: fx(4)?,
            in_control: chain(5)?,
            out_of_control: chain(16)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_shipped_file() {
        // the 27 tokens of the shipped TRAFCFG.DAT
        let tokens: [i32; 27] = [
            4, 12, 30, 45875, 720896, 524288, 2097152, 524288, 1572864, 81920, 52428, 327680, 52428, 1310720, 8519, 851, 0, 1048576, 524288, 1572864, 40960, 52428, 655360, 52428, 655360, 20971, 851,
        ];
        let mut d = Vec::new();
        for t in tokens {
            d.extend_from_slice(&t.to_le_bytes());
        }
        let c = TrafficConfig::parse(&d).unwrap();
        assert_eq!(c, TrafficConfig::default());
        // the out-of-control chain really does have a zero position gain, which is what the shipped
        // TRAFCFG.DES says ("not used for ooc") — a good check that the field order is right
        assert_eq!(c.out_of_control.dlpos_to_dlvel, 0.0);
        assert_eq!(c.in_control.dlpos_to_dlvel, 8.0);
        assert_eq!(c.in_control.vel_limit_range, 20.0);
        assert_eq!(c.out_of_control.vel_limit_range, 10.0);
        // and the three fields the reader scales by 1/1024 rather than 1/65536
        assert!((c.in_control.max_dav - 51.2).abs() < 1e-3);
        assert!((c.in_control.ang_vel_limit_factor - 0.831).abs() < 1e-3);
        assert!(TrafficConfig::parse(&d[..40]).is_err());
    }
}
