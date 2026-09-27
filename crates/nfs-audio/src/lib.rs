//! Native audio: a small mixer on a cpal output stream playing decoded original samples.
//!
//! Engine sound (`engine`): the car's sound set (`Sounds/<set>.viv`) holds an EA-XA bank plus
//! `CRDl` tables that map a 512-bin engine variable to per-sample volume windows
//! (`RE/notes/audio.md` §4, tables HYPOTHESIS). The native model plays every loop of the load
//! (`.elt`) or coast (`.ect`) table with volume = table A at the current bin and pitch =
//! current bin / window centre bin, with bin = rpm / 20 (HYPOTHESIS: 512 bins ≈ 0..10240 rpm,
//! which places the top window 271..511 at 5.4–10.2 krpm). Not the original DirectSound path.

pub mod ai_engine;
pub mod ambience;
pub mod engine;
pub mod fesfx;
pub mod librand;
pub mod mixer;
pub mod music;
pub mod resample;
pub mod reverb;
pub mod sfx;
pub mod slots;
pub mod speech;

pub use mixer::{AudioOutput, StopHandle, VoiceId};

/// The distance attenuations every audio pass builds the same way: the listener distance in 16.16
/// (`ftol(|p − 0x655E50| · 65536)`, 0x5A0F98 truncating), `fixdiv(num, d16 + add)` 0x4A6160 — a
/// true 16.16 divide, `(a/b) << 16 | ((a%b) << 16)/b` — shifted left by `shl`, then `>> 16` rounded
/// toward zero (`cdq ; and edx, 0xFFFF ; add ; sar 16`). So `2048/(d+2)` is taken of the
/// **fractional** distance, and a `· 4` inside the shift (view 0x0C) is applied before the floor.
/// Used by `AiEngine` 0x4AA973…0x4AA9B5, `Audio_playerEngineUpdate` 0x4AA072…0x4AA0C8,
/// `Horn_start` 0x4AAC10…0x4AAC5E and 0x4A93C0's 0x4A93D0…0x4A9420.
pub fn fix_atten(num: i32, add: i32, shl: u32, distance: f32) -> i32 {
    let d16 = (distance.max(0.0) * 65536.0) as i64;
    let q = ((num as i64) << 16) / (d16 + add as i64);
    ((q << shl) >> 16) as i32
}

#[cfg(test)]
mod tests {
    use super::fix_atten;

    #[test]
    fn the_attenuation_takes_the_fractional_distance() {
        // 2048/(d+2): 2048/12.7 = 161.26, not 2048/12 = 170 of the metre-truncated distance
        assert_eq!(fix_atten(0x800_0000, 0x2_0000, 0, 10.7), 161);
        assert_eq!(fix_atten(0x800_0000, 0x2_0000, 0, 0.0), 1024);
        // view 0x0C: four times 2048/(d+1), the ·4 before the floor — 114 at 70.3 m, not 4·28 = 112
        assert_eq!(fix_atten(0x800_0000, 0x1_0000, 2, 2.5), 2340);
        assert_eq!(fix_atten(0x800_0000, 0x1_0000, 2, 70.3), 114);
        // 512/(d+2) for the collision one-shots
        assert_eq!(fix_atten(0x200_0000, 0x2_0000, 0, 3.0), 102);
    }
}
