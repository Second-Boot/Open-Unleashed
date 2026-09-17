//! Per-track AI speed zones and the road-width normaliser (`0x4A2DC0`, CONFIRMED).
//!
//! `0x4A2DC0(family)` resets ten zone slots (`[0x5E74AC]` ranges, `[0x5E74FC]` multipliers, all 1.0)
//! and `[0x5E6E34] = 12.0`, then fills the slots for the current track name (`[0x657444]`) and
//! direction (`[0x657430]`). The multiplier scales the bend angle in `AI_targetSpeed` 0x40B650
//! (`ang = (maxL + maxR) · carSpecs[0x144] · zoneMult`), so a value above 1 makes the AI treat the
//! stretch as a tighter bend (slower), below 1 as straighter (faster). The tables below were
//! extracted from the disassembly of 0x4A2DC0 (`RE/notes/ai.md` §2.3).
//!
//! Tracks listed as `Both` have no direction test in the routine, so their zones apply either way.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone(pub u32, pub u32, pub f32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Fwd,
    Back,
    Both,
}

const ZONES: &[(&str, &str, &[Zone])] = &[
    ("monaco1", "fwd", &[Zone(30, 70, 1.15), Zone(90, 110, 1.2), Zone(425, 525, 1.15), Zone(525, 557, 1.13), Zone(200, 300, 0.7)]),
    ("monaco1", "back", &[Zone(20, 50, 1.1), Zone(90, 130, 1.1), Zone(390, 557, 1.15), Zone(139, 150, 25.0), Zone(580, 592, 30.0), Zone(200, 300, 0.7)]),
    ("monaco2", "fwd", &[Zone(480, 609, 1.125), Zone(255, 275, 1.125), Zone(80, 120, 0.8)]),
    ("monaco2", "back", &[Zone(0, 70, 0.1), Zone(70, 140, 0.7), Zone(267, 350, 1.7), Zone(480, 609, 1.05)]),
    ("monaco3", "fwd", &[Zone(65, 145, 1.1), Zone(491, 505, 1.15), Zone(570, 627, 1.15), Zone(225, 280, 0.8)]),
    ("monaco3", "back", &[Zone(185, 200, 1.1), Zone(340, 415, 1.15), Zone(506, 530, 1.25), Zone(200, 300, 1.15), Zone(130, 170, 1.1), Zone(7, 45, 1.4), Zone(683, 696, 1.5), Zone(490, 507, 1.25), Zone(70, 90, 1.15)]),
    ("monaco4", "both", &[Zone(600, 696, 1.4), Zone(300, 313, 1.25), Zone(112, 125, 0.1), Zone(760, 835, 0.1), Zone(260, 275, 1.3), Zone(274, 300, 1.4), Zone(405, 470, 1.15), Zone(60, 114, 1.1), Zone(440, 480, 1.2), Zone(5, 80, 1.15)]),
    ("monaco5", "fwd", &[Zone(278, 300, 0.1), Zone(590, 609, 0.1), Zone(530, 600, 0.7), Zone(250, 278, 1.1), Zone(400, 470, 1.25)]),
    ("monaco5", "back", &[Zone(0, 20, 0.1), Zone(278, 303, 0.1), Zone(455, 491, 1.15), Zone(490, 520, 1.3), Zone(60, 140, 0.85)]),
    ("alps", "both", &[Zone(300, 330, 1.6), Zone(350, 400, 1.2), Zone(750, 840, 1.5), Zone(900, 1350, 0.85), Zone(1500, 1700, 0.7)]),
    ("autobahn", "fwd", &[Zone(575, 605, 1.2), Zone(875, 920, 1.2), Zone(660, 695, 1.2), Zone(805, 855, 1.3), Zone(1470, 1520, 1.3), Zone(1415, 1465, 1.1), Zone(1810, 1858, 1.1), Zone(2750, 2787, 1.1)]),
    ("autobahn", "back", &[Zone(565, 610, 1.5), Zone(875, 920, 1.2), Zone(660, 695, 1.2), Zone(805, 855, 1.1), Zone(1470, 1520, 1.3), Zone(420, 470, 1.2), Zone(705, 745, 1.2), Zone(1857, 1905, 1.1), Zone(938, 965, 1.5), Zone(1410, 1460, 1.2)]),
    ("canyon", "fwd", &[Zone(400, 500, 1.15), Zone(930, 1030, 1.1), Zone(1080, 1130, 1.1), Zone(1170, 1270, 1.25), Zone(1600, 1670, 1.1), Zone(55, 370, 0.95)]),
    ("canyon", "back", &[Zone(730, 830, 1.15), Zone(1170, 1270, 1.25), Zone(1600, 1670, 1.1), Zone(1680, 1730, 1.1), Zone(55, 250, 0.85)]),
    ("castle", "both", &[Zone(50, 175, 0.95), Zone(290, 310, 1.5), Zone(2405, 2940, 0.9), Zone(2960, 3000, 1.3), Zone(3313, 3325, 1.5), Zone(3380, 3390, 1.4), Zone(3120, 3240, 0.85), Zone(380, 1000, 0.9), Zone(1000, 1020, 1.3), Zone(3428, 3442, 1.3)]),
    ("coastal", "both", &[Zone(270, 440, 1.1), Zone(540, 560, 1.05), Zone(630, 800, 1.15), Zone(950, 970, 1.1), Zone(970, 1050, 2.0), Zone(970, 1050, 1.2), Zone(1400, 1462, 1.2)]),
    ("farmland", "fwd", &[Zone(800, 1270, 0.85), Zone(1300, 1450, 0.85), Zone(1830, 1855, 1.15)]),
    ("farmland", "back", &[Zone(370, 550, 1.2), Zone(970, 1450, 0.85)]),
    ("foothills", "fwd", &[Zone(1050, 1100, 1.2), Zone(1180, 1320, 1.2), Zone(870, 920, 0.7)]),
    ("foothills", "back", &[Zone(620, 660, 1.2), Zone(680, 720, 1.2), Zone(1240, 1320, 1.1), Zone(870, 970, 0.7)]),
    ("forest", "both", &[Zone(50, 150, 0.8), Zone(200, 280, 0.9), Zone(820, 1050, 0.9), Zone(1090, 1120, 1.15), Zone(1250, 1270, 1.1), Zone(1350, 1470, 0.7), Zone(1470, 1750, 0.9), Zone(1850, 1945, 0.4)]),
    ("industrial", "fwd", &[Zone(820, 880, 0.6), Zone(990, 1400, 0.75), Zone(1540, 1600, 1.35)]),
    ("industrial", "back", &[Zone(610, 680, 1.3), Zone(820, 880, 0.6), Zone(990, 1400, 0.75)]),
];

/// Zones that apply to `track` in the given direction.
pub fn zones_for(track: &str, backwards: bool) -> &'static [Zone] {
    let want = if backwards { "back" } else { "fwd" };
    for (name, dir, zs) in ZONES {
        if name.eq_ignore_ascii_case(track) && (*dir == "both" || *dir == want) {
            return zs;
        }
    }
    &[]
}

/// Bend-angle multiplier for a slice (the last matching zone wins, as the original scans all ten).
pub fn zone_mult(track: &str, backwards: bool, slice: usize) -> f32 {
    let s = slice as u32;
    let mut m = 1.0;
    for Zone(from, to, mult) in zones_for(track, backwards) {
        if s >= *from && s <= *to {
            m = *mult;
        }
    }
    m
}

/// Road-width normaliser `[0x5E6E34]`: 12 by default, 10 on castle, 9 on forest (0x4A2DC0).
pub fn width_divisor(track: &str) -> f32 {
    let t = track.to_ascii_lowercase();
    if t.starts_with("castle") {
        10.0
    } else if t.starts_with("forest") {
        9.0
    } else {
        12.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monaco_zones_differ_by_direction() {
        // forward: 30..70 is a 1.15 zone; backwards that stretch is 20..50 ×1.1
        assert_eq!(zone_mult("monaco1", false, 50), 1.15);
        assert_eq!(zone_mult("monaco1", true, 30), 1.1);
        // the backwards-only hairpin markers
        assert_eq!(zone_mult("monaco1", true, 145), 25.0);
        assert_eq!(zone_mult("monaco1", true, 585), 30.0);
        // a stretch the AI may take faster
        assert_eq!(zone_mult("monaco1", false, 250), 0.7);
        // outside every zone
        assert_eq!(zone_mult("monaco1", false, 1000), 1.0);
    }

    #[test]
    fn direction_independent_tracks_and_divisors() {
        assert_eq!(zone_mult("alps", false, 320), 1.6);
        assert_eq!(zone_mult("alps", true, 320), 1.6);
        assert_eq!(zone_mult("castle", true, 3390), 1.4);
        assert_eq!(width_divisor("castle"), 10.0);
        assert_eq!(width_divisor("forest"), 9.0);
        assert_eq!(width_divisor("alps"), 12.0);
        assert!(zones_for("skidpad", false).is_empty());
    }
}
