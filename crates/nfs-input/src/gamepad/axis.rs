//! The four axis scalers of the pad code, side by side: SDL's 16-bit values as the unit floats the menu stick and the photo camera
//! read ([`unit`], [`trigger`]) and as the DirectInput integers of the original's device record ([`di_value`], [`trigger_value`]).

/// An SDL axis, −1…1.
pub(super) fn unit(v: i16) -> f32 {
    (f32::from(v) / 32767.0).clamp(-1.0, 1.0)
}

/// PORT-DEVIATION D46: an SDL trigger (0…32767), 0…1.
pub(super) fn trigger(v: i16) -> f32 {
    (f32::from(v.max(0)) / 32767.0).clamp(0.0, 1.0)
}

/// An SDL axis value (−32768…32767) as DirectInput's default 0…65535 range, the one the image
/// keeps (it sets no `DIPROP_RANGE`, 0x56FD70).
pub(super) fn di_value(v: i16) -> i32 {
    i32::from(v) + 32768
}

/// A trigger (SDL 0…32767) as a DirectInput axis value resting at the **centre**: 0x8000 released,
/// 0xFFFF fully pressed. PORT-DEVIATION D44 (PAD-MODERN): the trigger's travel is the upper half
/// of the axis, so `Analog`'s reader 0x411B80 under a `dir` +1 binding — whose band 0x51CEA4 /
/// 0x51CED1 bake as `0x80 + <Axis>.Min` … `0x80 + <Axis>.Max` — reads **0 at rest and 255 fully
/// pressed** over the whole travel, the factory `Min` 0 being no dead zone and a `Min` set on the
/// dead-zone screen one at the released end. Had it rested at 0 (the axis's own end), a `+1`
/// binding would read nothing over the first half of the travel and a `−1` one 255 at rest.
pub(super) fn trigger_value(v: i16) -> i32 {
    0x8000 + i32::from(v.max(0))
}
