//! The pan: the 16-bit angle, the pan byte, the voice's pan and the two channel gains (0x598760).

/// **0x565BB0**, the game's `atan2` on a 1024-step circle, of `x` (the side) and `y` (forward):
/// the octant from the signs and the larger magnitude, then `0x5E24AC[(x·256 + y/2) / y]` (x ≤ y,
/// `div`, so rounded) — the table is `round(atan(i/256) · 1024/2π)` for `i` in 0…256 (checked
/// against all 257 bytes of the file: every one within 0.4999) — or, for `x ≥ 0x800000`, the
/// 64-bit quotient `(x << 32) / y` rounded at bit 23 (0x565C18…0x565C3E). `x == y` is 0x80.
pub fn atan2_1024(x: i32, y: i32) -> i32 {
    let (mut esi, mut ecx) = (y, x);
    let (mut neg, mut base) = (false, 0i32);
    if esi < 0 {
        esi = esi.wrapping_neg();
        neg = true;
        base = 0x200;
    }
    if ecx < 0 {
        neg = !neg;
        ecx = ecx.wrapping_neg();
        base = -base;
    }
    if ecx > esi {
        std::mem::swap(&mut ecx, &mut esi);
        base += if neg { -0x100 } else { 0x100 };
        neg = !neg;
    }
    let t = if ecx == esi {
        0x80
    } else {
        let i = if ecx as u32 & 0xFF80_0000 != 0 {
            let q = ((ecx as u32 as u64) << 32) / esi as u32 as u64;
            (q >> 24) as u32 + ((q >> 23) & 1) as u32
        } else {
            ((esi as u32 >> 1) + ((ecx as u32) << 8)) / esi as u32
        };
        ((i as f64 / 256.0).atan() * 1024.0 / std::f64::consts::TAU).round() as i32
    };
    if neg { base - t } else { base + t }
}

/// **0x4A6FE0**'s angle of a sound source: `side` and `fwd` are the offset from the listener
/// dotted with the listener's row `+0xC` (`[0x655E5C]`) and row `+0x24` (`[0x655E74]`), each
/// `· 65536` and `_ftol`'d (0x5A0F98, toward zero), `sar 8`, then
/// `(atan2_1024(side, fwd) & 0x3FF) << 6` — a full-circle 16-bit angle, 0x4000 on the `+side`
/// hand, 0x8000 behind. `MIRROR` `[0x65742C]` negates `side` (0x4A704D): the caller passes it
/// negated. Without 3-D positioning (`[0x5D1760] == 0`, never in the shipped `fe.txt`) it is 0.
pub fn pan_angle(side: f32, fwd: f32) -> u16 {
    let sx = (side * 65536.0) as i32 >> 8;
    let sy = (fwd * 65536.0) as i32 >> 8;
    ((atan2_1024(sx, sy) & 0x3FF) << 6) as u16
}

/// The **pan byte** `Voice_play` 0x4A98E0 stores in the request's `+7` (0x4A9B4B…0x4A9B84) and
/// the loops' update hands to `Voice_setPan8` 0x566100 (0x4A9F3D…0x4A9F79, 0x4AB0F1…0x4AB13D),
/// because `[0x655D40]` — the 3-D pan path, `Voice_setPanAngle` — is always 0 (only 0x4A66B0's dead
/// branch sets it): the rear half folds onto the front, `(0xBFFF − a) >> 8` for
/// `0x3FFF < a < 0xC000`, else `((a + 0x4000) >> 8) & 0xFF` — 0x40 ahead, 0x7F at 0x4000 (and just
/// behind it), 0 at 0xC000, 0x3F dead behind.
pub fn pan_byte(a: u16) -> u8 {
    let a = a as i32;
    if a > 0x3FFF && a < 0xC000 { ((0xBFFF - a) >> 8) as u8 } else { (((a + 0x4000) >> 8) & 0xFF) as u8 }
}

/// The library's pan for a voice from the caller's byte (0x590560…0x5905A3 at the start,
/// 0x566171…0x5661A2 on `Voice_setPan8`): scaled about 0x40 by the patch's code 0x25 unless 1,
/// plus the patch's code 0x0C less 0x40, clamped to 0…127. The angle the backend is handed is then
/// `(p − 0x40) << 8` (0x59070C…0x590716, `(p + 0xC0) << 8` at 0x5661A4) — the byte back on the
/// front half circle, 0x4000 at 0x80. `patch_pan` is the start's `voice+0x3E`, code 0x0C plus
/// its random spread (code 0x0D, [`crate::librand::patch_start`]).
pub fn voice_pan(byte: u8, patch_pan: u8, scale: i8) -> u8 {
    let mut p = byte as i8 as i32;
    if scale != 1 {
        p = (p - 0x40) * scale as i32 + 0x40;
    }
    (p + patch_pan as i32 - 0x40).clamp(0, 127) as u8
}

/// The software mixer's stereo law (`RE/notes/engine-audio.md`, "The pan law"): 0x58E0E0 builds a
/// 256-step table over the voice's angle (`[0x6B1820 + 4·(angle >> 8)]`, 0x58E290) with channel 0
/// at 0x4000 and channel 1 at 0xC000 (`[0x6B48FC]` / `[0x6B48FE]`, 0x58A5DF / 0x58A5E8); between
/// them the angle splits **linearly**, and each channel's share `x` (0…127, `idiv`) becomes
/// `f(x) = 2x − x²/127` (0x58E0B0), scaled by 1/127 (`[0x5C1788]`). Front and back mirror.
///
/// The angle is the library's `(p − 0x40) << 8` of the voice's pan byte `p` ([`voice_pan`]), so
/// the step is `(p − 0x40) & 0xFF` and the two shares come out as `127·p/128` for channel 0 and
/// `127·(128 − p)/128` for channel 1 — 0x7F: 127 / 0, 0x40: 95 / 95, 0: 0 / 127. Channel 0 is the
/// first of the stereo pair the buffer holds (`nChannels` 2, 0x58ED6A) — the port's first output
/// channel; a source on the `+side` hand of [`pan_angle`] is heard there.
///
/// This is the path of every voice the game starts: the hardware-3D voice count `[0x6B48DF]` is
/// 0 whatever the card — 0x4A66B0 configures it only when `[0x657C9C]` is set, and both start-ups
/// clear that just before the call (0x4A684C, 0x4A6A61) — so 0x58A7C4…0x58A7D6 drops every 3D
/// type and the one voice type left is the configuration's 0x24 (0x4A670B), never flag 0x10. A
/// hardware-3D voice would be positioned with `SetPosition` (0x596370) instead. *(Until
/// 2026-09-27 this was an approximation "for a card without DirectSound3D buffers".)*
/// (Until 2026-09-25 this was an equal-power cos / sin law of the port's own.)
/// (Until 2026-09-27 the port fed this law the sine of the angle, taken back to an angle and
/// rounded onto the grid, instead of the pan byte.)
pub(super) fn pan_gains(p: u8) -> [f32; 2] {
    // the table's step and the angle it stands for
    let x = ((p as i32 - 0x40) & 0xFF) << 8;
    let (s0, s1) = (0x4000, 0xC000);
    // (distance to channel 0, distance to channel 1) along the arc between them (0x58E137…0x58E24C)
    let (d0, d1) = if x <= s0 {
        (s0 - x, x - s1 + 0x10000)
    } else if x >= s1 {
        (s0 - x + 0x10000, x - s1)
    } else {
        (x - s0, s1 - x)
    };
    // the byte · `[0x5C1788]` (f32 1/127) stored as an f32 (0x59694C)
    let f = |x: i32| ((2 * x - x * x / 127) as f64 * (1.0f32 / 127.0) as f64) as f32;
    // each channel's share grows with the distance to the other one
    [f(127 * d1 / (d0 + d1)), f(127 * d0 / (d0 + d1))]
}

#[cfg(test)]
mod pan_tests {
    use super::{atan2_1024, pan_angle, pan_byte, pan_gains, voice_pan};

    #[test]
    fn the_angle_is_the_games_atan2_on_the_listener_axes() {
        // (side, forward): ahead 0, the +side hand 0x100, behind 0x200, the other hand −0x100
        assert_eq!(atan2_1024(0, 256), 0);
        assert_eq!(atan2_1024(256, 0), 0x100);
        assert_eq!(atan2_1024(0, -256), 0x200);
        assert_eq!(atan2_1024(-256, 0), -0x100);
        assert_eq!(atan2_1024(256, 256), 0x80);
        // x/y = 0.5: step (100·256 + 100) / 200 = 128 → atan(0.5) in 1024ths = 75.56 → 76; the
        // 64-bit path for x ≥ 0x800000 lands on the same step
        assert_eq!(atan2_1024(100, 200), 76);
        assert_eq!(atan2_1024(0x90_0000, 0x120_0000), 76);
        // the 16-bit angle: ·65536, toward zero, sar 8, & 0x3FF, << 6
        assert_eq!(pan_angle(1.0, 0.0), 0x4000);
        assert_eq!(pan_angle(-1.0, 0.0), 0xC000);
        assert_eq!(pan_angle(0.0, -3.0), 0x8000);
        assert_eq!(pan_angle(2.0, 2.0), 0x2000);
    }

    #[test]
    fn the_pan_byte_folds_the_rear_onto_the_front() {
        // 0x4A9B4B…0x4A9B84: 0x40 ahead, 0x7F at the +side hand, 0 at the other
        assert_eq!(pan_byte(0), 0x40);
        assert_eq!(pan_byte(0x2000), 0x60);
        assert_eq!(pan_byte(0x3FFF), 0x7F);
        assert_eq!(pan_byte(0x4000), 0x7F);
        // dead behind is 0x3F, one step off the front's 0x40
        assert_eq!(pan_byte(0x8000), 0x3F);
        assert_eq!(pan_byte(0xC000), 0);
        assert_eq!(pan_byte(0xFFFF), 0x3F);
    }

    #[test]
    fn the_patch_scales_and_offsets_the_pan_byte() {
        // no code 0x0C / 0x25: the caller's byte as it is
        assert_eq!(voice_pan(0x7F, 64, 1), 0x7F);
        // SND_coll's pan 40 on a centred call: 0x40 + 40 − 0x40
        assert_eq!(voice_pan(0x40, 40, 1), 40);
        assert_eq!(voice_pan(0, 90, 1), 26);
        // fesfx's scale 4 about 0x40, clamped to 0…127
        assert_eq!(voice_pan(0x48, 64, 4), 96);
        assert_eq!(voice_pan(0x60, 64, 4), 127);
        assert_eq!(voice_pan(0x20, 64, 4), 0);
    }

    #[test]
    fn the_pan_law_is_the_software_mixers() {
        // one channel at 127/127, the other silent (0x58E0E0)
        assert_eq!(pan_gains(0x7F), [1.0, 0.0]);
        assert_eq!(pan_gains(0), [0.0, 1.0]);
        // centre: x = 63, f = 126 − 31 = 95 → 95/127 on both (0x58E0B0)
        let c = pan_gains(0x40);
        assert!((c[0] - 95.0 / 127.0).abs() < 1e-6 && (c[1] - 95.0 / 127.0).abs() < 1e-6);
        // 0x60 (angle 0x2000): shares 127·96/128 = 95 and 127·32/128 = 31 → f = 119 and 55
        let g = pan_gains(0x60);
        assert!((g[0] - 119.0 / 127.0).abs() < 1e-6 && (g[1] - 55.0 / 127.0).abs() < 1e-6);
    }
}
