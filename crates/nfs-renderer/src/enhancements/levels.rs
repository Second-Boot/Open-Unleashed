//! The level -> value maps of the switches and the E34 / E37 / E38 / E39 upscaler rules (ENHANCEMENT ENH-UPSCALER).

/// ENHANCEMENT E34: its levels 1..=this are the temporal ones (MetalFX temporal scaler); the levels above are the spatial ('simple') ones
pub const E34_TEMPORAL_LEVELS: u8 = 4;

// ---- ENHANCEMENT ENH-UPSCALER: the upscaler choice and the settings rows of the chosen backend ----

/// E38's levels.
pub const UP_OFF: u8 = 0;
pub const UP_METALFX: u8 = 1;
pub const UP_FSR: u8 = 2;

/// The E38 levels this platform offers, in the combo's order: macOS off / MetalFX / FSR 3.1, elsewhere off / FSR 3.1.
pub fn upscaler_choices() -> &'static [u8] {
    if cfg!(target_os = "macos") { &[UP_OFF, UP_METALFX, UP_FSR] } else { &[UP_OFF, UP_FSR] }
}

/// MetalFX's mode, the first of its two rows: E34 levels 1..=4 are the temporal scaler, 5..=7 the spatial («простой») one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfxMode {
    Temporal,
    Simple,
}

/// E34's stored level -> the mode row (level 0, "off", counts as the default temporal quality level 2, see [`Enhancements::effective`]).
pub fn mfx_mode(level: u8) -> MfxMode {
    if level > E34_TEMPORAL_LEVELS { MfxMode::Simple } else { MfxMode::Temporal }
}

/// E34's stored level -> the scale row's index: temporal 1:1 / 67 / 59 / 50 % = 0..=3 (levels 1..=4), simple 67 / 59 / 50 % = 0..=2
/// (levels 5..=7). Level 0 reads as the default (67 %, temporal).
pub fn mfx_scale_index(level: u8) -> usize {
    match level {
        0 => 1,
        1..=4 => level as usize - 1,
        _ => (level - E34_TEMPORAL_LEVELS - 1) as usize,
    }
}

/// The number of choices of the scale row of a mode: temporal 4 (1:1 too), simple 3.
pub fn mfx_scale_count(mode: MfxMode) -> usize {
    match mode {
        MfxMode::Temporal => 4,
        MfxMode::Simple => 3,
    }
}

/// The E34 level of a (mode, scale index); an index past the mode's last is clamped; the simple mode has no 1:1, so temporal's
/// 1:1 (index 0) becomes simple's 67 % (index 0 there is already 67 %: the two scale lists are offset by the 1:1 entry).
pub fn mfx_level(mode: MfxMode, scale: usize) -> u8 {
    match mode {
        MfxMode::Temporal => 1 + scale.min(3) as u8,
        MfxMode::Simple => E34_TEMPORAL_LEVELS + 1 + scale.min(2) as u8,
    }
}

/// Switching the mode keeps the scale where it exists: temporal 67 / 59 / 50 % (levels 2..=4) <-> simple 67 / 59 / 50 % (5..=7); the
/// temporal 1:1 (level 1) goes to simple 67 %.
pub fn mfx_switch_mode(level: u8, to: MfxMode) -> u8 {
    let l = if level == 0 { 2 } else { level };
    match (mfx_mode(l), to) {
        (MfxMode::Temporal, MfxMode::Simple) => E34_TEMPORAL_LEVELS + l.max(2) - 1,
        (MfxMode::Simple, MfxMode::Temporal) => l - E34_TEMPORAL_LEVELS + 1,
        _ => l,
    }
}

/// FSR's scale row: E37 levels 1..=4 = 1:1 / 1.5x / 1.7x / 2x = indices 0..=3 (level 0 reads as 1.5x, the default).
pub fn fsr_scale_index(level: u8) -> usize {
    if level == 0 { 1 } else { level as usize - 1 }
}

/// The RCAS strength of E39's level; `None` = sharpening off.
pub fn fsr_sharpness(level: u8) -> Option<f32> {
    match level {
        0 => None,
        1 => Some(0.2),
        2 => Some(0.3),
        3 => Some(0.5),
        _ => Some(0.8),
    }
}

// ---- the level -> value maps of the other entries, pure, beside the table. The renderer's `enh_*` and
// the app's `enh_*` keep the gates (is the switch on, does the device / the scene allow it) and call these. The values are the
// authored ones, unchanged. ----

/// ENHANCEMENT E1: the sample count a level asks for (the renderer takes the highest the adapter has that is `<=` it); 1 at
/// level 0.
pub fn msaa_samples_wanted(level: u8) -> u32 {
    match level {
        0 => 1,
        1 => 2,
        2 => 4,
        _ => 8,
    }
}

/// ENHANCEMENT E3: the render scale of a level (level 0 = 1.0, the original 1:1).
pub fn render_scale(level: u8) -> f32 {
    match level {
        0 => 1.0,
        1 => 0.5,
        2 => 0.75,
        3 => 1.25,
        4 => 1.5,
        _ => 2.0,
    }
}

/// ENHANCEMENT E4: degrees added to the original's horizontal view angle (0 at level 0, then 5 per level).
pub fn fov_bonus_deg(level: u8) -> f32 {
    level as f32 * 5.0
}

/// ENHANCEMENT E5: the draw-distance factor of the main view (1.0, the original, at level 0; +25 / +50 / +100 %).
pub fn draw_reach(level: u8) -> f32 {
    match level {
        1 => 1.25,
        2 => 1.5,
        3 => 2.0,
        _ => 1.0,
    }
}

/// ENHANCEMENT E7: the tone-map mode of a level that is on: 0 "as the original" (level 1), 1 ACES (level 2).
pub fn tonemap_mode(level: u8) -> u32 {
    if level >= 2 { 1 } else { 0 }
}

/// ENHANCEMENT E13: the light shafts' strength factor at level 1 (subtle) and level 2 (medium).
pub(super) const SHAFT_STRENGTH_SUBTLE: f32 = 0.45;
pub(super) const SHAFT_STRENGTH_MEDIUM: f32 = 0.8;

/// ENHANCEMENT E13: the factor on the scene's own shaft strength of a level that is on.
pub fn shaft_strength(level: u8) -> f32 {
    if level >= 2 { SHAFT_STRENGTH_MEDIUM } else { SHAFT_STRENGTH_SUBTLE }
}

/// ENHANCEMENT E26: the wet road's reflection gain (`globals.rain.x`) at level 1 and at level 2 and up.
pub(super) const RAIN_GAIN: f32 = 1.0;
pub(super) const RAIN_GAIN_L2: f32 = 1.5;

/// ENHANCEMENT E26: the wet road's reflection gain of a level that is on.
pub fn rain_gain(level: u8) -> f32 {
    if level >= 2 { RAIN_GAIN_L2 } else { RAIN_GAIN }
}

/// ENHANCEMENT E23: a street lamp's pool scale (on its radius) and light gain of a level that is on: soft (level 1) a smaller,
/// dimmer pool, normal the authored default.
pub fn street_lamp_pool(level: u8) -> (f32, f32) {
    if level == 1 { (0.75, 0.7) } else { (1.0, 1.0) }
}
