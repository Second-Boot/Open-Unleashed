//! Per-track AI speed zones and the road-width normaliser (`0x4A2DC0`, CONFIRMED).
//!
//! `0x4A2DC0(family)` resets ten zone slots (`[0x5E74AC]` ranges, `[0x5E74FC]` multipliers, all 1.0)
//! and `[0x5E6E34] = 12.0`, then fills the slots for the current track name (`[0x657444]`) and
//! direction (`[0x657430]`). The multiplier scales the bend angle in `AI_targetSpeed` 0x40B650
//! (`ang = (maxL + maxR) · carSpecs[0x144] · zoneMult`), so a value above 1 makes the AI treat the
//! stretch as a tighter bend (slower), below 1 as straighter (faster). The tables below were
//! extracted from the disassembly of 0x4A2DC0 (`RE/notes/ai.md` §2.3).
//!
//! **All twenty-eight arms are in, 2026-09-21.** Every track has a direction test — one
//! `[0x657430]` compare inside each block, at 0x4A2E26, 0x4A3016, 0x4A3109, 0x4A332A, 0x4A35C7,
//! 0x4A374F, 0x4A3872, 0x4A3AEA, 0x4A3C81, 0x4A3EF5, 0x4A40A8, 0x4A4184, 0x4A42AB and 0x4A44EE —
//! so no track is direction-independent, and the five rows that used to read `"both"` (monaco4,
//! alps, castle, coastal, forest) were the **forward** arm only.
//!
//! The arms were extracted by walking each block's prologue for the shared stores, then following
//! one side of the branch. Two things made a first pass get it wrong and are worth knowing: the
//! flag is loaded into a register and the `cmp` can be several instructions later, with shared
//! slot stores in between (alps: `eax = [0x657430]` at 0x4A374F, `[0x5E74AC] = 300` at 0x4A3754,
//! `cmp` at 0x4A375E); and a few bounds come from a register rather than an immediate. The
//! extraction reproduces every previously verified forward row byte for byte, which is what
//! validates the backward ones.
//!
//! monaco4's old row was a **merge**: eight forward slots plus the backward arm's `440–480` and
//! `5–80`, with slot 5's multiplier taken from the wrong arm — it is **1.8** (0x4A3507 stores
//! 0x3FE66666), not 1.4.
//!
//! What *is* settled and applied: the bounds are strict and the last matching slot wins (see
//! [`zone_mult`]), and foothills' inverted slot is kept inverted.

use crate::track_id::TrackId;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone(pub u32, pub u32, pub f32);

/// Which direction arm of a track's block (`[0x657430]`, the compare inside each block).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dir {
    Fwd,
    Back,
}

const ZONES: &[(TrackId, Dir, &[Zone])] = &[
    (TrackId::Monaco1, Dir::Fwd, &[Zone(30, 70, 1.15), Zone(90, 110, 1.2), Zone(425, 525, 1.15), Zone(525, 557, 1.13), Zone(200, 300, 0.7)]),
    (TrackId::Monaco1, Dir::Back, &[Zone(20, 50, 1.1), Zone(90, 130, 1.1), Zone(390, 557, 1.15), Zone(139, 150, 25.0), Zone(580, 592, 30.0), Zone(200, 300, 0.7)]),
    (TrackId::Monaco2, Dir::Fwd, &[Zone(480, 609, 1.125), Zone(255, 275, 1.125), Zone(80, 120, 0.8)]),
    (TrackId::Monaco2, Dir::Back, &[Zone(0, 70, 0.1), Zone(70, 140, 0.7), Zone(267, 350, 1.7), Zone(480, 609, 1.05)]),
    (TrackId::Monaco3, Dir::Fwd, &[Zone(65, 145, 1.1), Zone(491, 505, 1.15), Zone(570, 627, 1.15), Zone(225, 280, 0.8)]),
    (TrackId::Monaco3, Dir::Back, &[Zone(185, 200, 1.1), Zone(340, 415, 1.15), Zone(506, 530, 1.25), Zone(200, 300, 1.15), Zone(130, 170, 1.1), Zone(7, 45, 1.4), Zone(683, 696, 1.5), Zone(490, 507, 1.25), Zone(70, 90, 1.15)]),
    // monaco4's row used to be a **merge** of both arms with slot 5's multiplier taken from the
    // wrong one: the forward arm is eight slots ending 60–114, and `274–300` is **1.8**
    // (0x4A3507 stores 0x3FE66666). The `440–480` and `5–80` that used to sit here are the
    // backward arm's slots 8 and 9.
    (TrackId::Monaco4, Dir::Fwd, &[Zone(600, 696, 1.4), Zone(300, 313, 1.25), Zone(112, 125, 0.1), Zone(760, 835, 0.1), Zone(260, 275, 1.3), Zone(274, 300, 1.8), Zone(405, 470, 1.15), Zone(60, 114, 1.1)]),
    (TrackId::Monaco4, Dir::Back, &[Zone(136, 235, 1.2), Zone(301, 314, 1.2), Zone(112, 136, 0.1), Zone(313, 355, 1.25), Zone(290, 302, 1.3), Zone(277, 291, 1.5), Zone(480, 510, 0.1), Zone(695, 750, 1.2), Zone(440, 480, 1.2), Zone(5, 80, 1.15)]),
    (TrackId::Monaco5, Dir::Fwd, &[Zone(278, 300, 0.1), Zone(590, 609, 0.1), Zone(530, 600, 0.7), Zone(250, 278, 1.1), Zone(400, 470, 1.25)]),
    (TrackId::Monaco5, Dir::Back, &[Zone(0, 20, 0.1), Zone(278, 303, 0.1), Zone(455, 491, 1.15), Zone(490, 520, 1.3), Zone(60, 140, 0.85)]),
    (TrackId::Alps, Dir::Fwd, &[Zone(300, 330, 1.6), Zone(350, 400, 1.2), Zone(750, 840, 1.5), Zone(900, 1350, 0.85), Zone(1500, 1700, 0.7)]),
    (TrackId::Alps, Dir::Back, &[Zone(300, 450, 1.4), Zone(800, 900, 1.5), Zone(900, 1350, 0.85), Zone(1500, 1700, 0.7)]),
    (TrackId::Autobahn, Dir::Fwd, &[Zone(575, 605, 1.2), Zone(875, 920, 1.2), Zone(660, 695, 1.2), Zone(805, 855, 1.3), Zone(1470, 1520, 1.3), Zone(1415, 1465, 1.1), Zone(1810, 1858, 1.1), Zone(2750, 2787, 1.1)]),
    (TrackId::Autobahn, Dir::Back, &[Zone(565, 610, 1.5), Zone(875, 920, 1.2), Zone(660, 695, 1.2), Zone(805, 855, 1.1), Zone(1470, 1520, 1.3), Zone(420, 470, 1.2), Zone(705, 745, 1.2), Zone(1857, 1905, 1.1), Zone(938, 965, 1.5), Zone(1410, 1460, 1.2)]),
    (TrackId::Canyon, Dir::Fwd, &[Zone(400, 500, 1.15), Zone(930, 1030, 1.1), Zone(1080, 1130, 1.1), Zone(1170, 1270, 1.25), Zone(1600, 1670, 1.1), Zone(55, 370, 0.95)]),
    (TrackId::Canyon, Dir::Back, &[Zone(730, 830, 1.15), Zone(1170, 1270, 1.25), Zone(1600, 1670, 1.1), Zone(1680, 1730, 1.1), Zone(55, 250, 0.85)]),
    (TrackId::Castle, Dir::Fwd, &[Zone(50, 175, 0.95), Zone(290, 310, 1.5), Zone(2405, 2940, 0.9), Zone(2960, 3000, 1.3), Zone(3313, 3325, 1.5), Zone(3380, 3390, 1.4), Zone(3120, 3240, 0.85), Zone(380, 1000, 0.9), Zone(1000, 1020, 1.3), Zone(3428, 3442, 1.3)]),
    (TrackId::Castle, Dir::Back, &[Zone(50, 175, 0.9), Zone(290, 310, 1.5), Zone(2405, 2740, 0.9), Zone(2960, 3000, 1.3), Zone(3313, 3325, 1.5), Zone(3380, 3390, 1.4), Zone(3120, 3240, 0.85), Zone(380, 1000, 0.9), Zone(1000, 1020, 1.3)]),
    (TrackId::Coastal, Dir::Fwd, &[Zone(270, 440, 1.1), Zone(540, 560, 1.05), Zone(630, 800, 1.15), Zone(950, 970, 1.1), Zone(970, 1050, 2.0), Zone(970, 1050, 1.2), Zone(1400, 1462, 1.2)]),
    (TrackId::Coastal, Dir::Back, &[Zone(270, 320, 1.15), Zone(430, 500, 1.25), Zone(520, 560, 1.05), Zone(660, 800, 1.2), Zone(820, 870, 1.5), Zone(970, 1050, 1.2), Zone(1400, 1462, 1.2)]),
    (TrackId::Farmland, Dir::Fwd, &[Zone(800, 1270, 0.85), Zone(1300, 1450, 0.85), Zone(1830, 1855, 1.15)]),
    (TrackId::Farmland, Dir::Back, &[Zone(370, 550, 1.2), Zone(970, 1450, 0.85)]),
    (TrackId::Foothills, Dir::Fwd, &[Zone(1050, 1100, 1.2), Zone(1180, 1320, 1.2), Zone(870, 920, 0.7)]),
    // Slot 0 is **620 → 600** in the shipped code — 0x4A418C stores `from = 0x26C` = 620 and
    // 0x4A4196 `to = 0x258` = 600 — so with the strict test it can never match and the 1.2 is
    // dead. It is kept inverted here on purpose: the port used to "fix" it to 620–660, which
    // handed the AI a zone the original never applies.
    (TrackId::Foothills, Dir::Back, &[Zone(620, 600, 1.2), Zone(680, 720, 1.2), Zone(1240, 1320, 1.1), Zone(870, 970, 0.7)]),
    (TrackId::Forest, Dir::Fwd, &[Zone(50, 150, 0.8), Zone(200, 280, 0.9), Zone(820, 1050, 0.9), Zone(1090, 1120, 1.15), Zone(1250, 1270, 1.1), Zone(1350, 1470, 0.7), Zone(1470, 1750, 0.9), Zone(1850, 1945, 0.4)]),
    (TrackId::Forest, Dir::Back, &[Zone(50, 150, 0.8), Zone(200, 280, 0.9), Zone(850, 900, 0.9), Zone(950, 1050, 0.9), Zone(1100, 1150, 1.15), Zone(1290, 1350, 1.5), Zone(1350, 1450, 0.9), Zone(1520, 1750, 0.9), Zone(1850, 1945, 0.4)]),
    (TrackId::Industrial, Dir::Fwd, &[Zone(820, 880, 0.6), Zone(990, 1400, 0.75), Zone(1540, 1600, 1.35)]),
    (TrackId::Industrial, Dir::Back, &[Zone(610, 680, 1.3), Zone(820, 880, 0.6), Zone(990, 1400, 0.75)]),
];

/// Zones that apply to `track` in the given direction (a name that is not a track has none).
pub fn zones_for(track: &str, backwards: bool) -> &'static [Zone] {
    zones_of(TrackId::from_name(track), backwards)
}

/// [`zones_for`] for a track parsed once (`None`: no zones).
pub fn zones_of(track: Option<TrackId>, backwards: bool) -> &'static [Zone] {
    let Some(track) = track else { return &[] };
    let want = if backwards { Dir::Back } else { Dir::Fwd };
    for (id, dir, zs) in ZONES {
        if *id == track && *dir == want {
            return zs;
        }
    }
    &[]
}

/// Bend-angle multiplier for a slice (the last matching zone wins, as the original scans all ten).
pub fn zone_mult(track: &str, backwards: bool, slice: usize) -> f32 {
    zone_mult_of(TrackId::from_name(track), backwards, slice)
}

/// [`zone_mult`] for a track parsed once.
pub fn zone_mult_of(track: Option<TrackId>, backwards: bool, slice: usize) -> f32 {
    let s = slice as u32;
    let mut m = 1.0;
    for Zone(from, to, mult) in zones_of(track, backwards) {
        // **Strictly inside**, and the **last** matching slot wins. 0x40BA75 is
        // `cmp edx, [eax-4]; jle skip` and 0x40BA7A `cmp edx, [eax]; jge skip`, so a slice equal
        // to either bound does not match; and the body is `fstp st(0); fld [ecx]`, which
        // *replaces* the running multiplier rather than stopping — with ten slots walked in order,
        // the last hit is the one applied. The port had the bounds inclusive; last-wins it already
        // had, and it matters because coastal forward carries 970–1050 twice (2.0 then 1.2, so the
        // effective value is 1.2). Corrected 2026-09-21.
        if s > *from && s < *to {
            m = *mult;
        }
    }
    m
}

/// Road-width normaliser `[0x5E6E34]`: 12 by default, 10 on castle, 9 on forest (0x4A2DC0).
/// **Why a slot can read `Zone(0, …)`.** 0x4A2DC0 opens with a reset (0x4A2DE1…0x4A2E02): the ten
/// multipliers at `0x5E74FC` are `rep stosd`-filled with **1.0** and the twenty range dwords at
/// `0x5E74AC` are zeroed. A track arm then stores only the slots it uses, so a slot whose `from` is
/// never stored is **0**, and a slot with no multiplier is **1.0** — which is what
/// `monaco5 back`'s first zone is: only its `to` (0x14 = 20) is written, at 0x4A35D9.
/// Re-verified against the immediates 2026-09-21, all 28 arms; the four `from`s a scan could not
/// resolve are `monaco2 back` 0 (`ebx`, xor-zeroed at 0x4A2DF0, with `to` = `eax` = 0x46 set at
/// 0x4A301E **after** the direction branch), `monaco3 back` **7** (0x4A31A9), `monaco4 back` **5**
/// (0x4A3446) and `monaco5 back` 0 (never stored).
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

    /// Coastal carries 970–1050 twice, 2.0 then 1.2, and the loop replaces rather than stops —
    /// so the second one is what applies (0x40BA7E).
    #[test]
    fn the_last_matching_zone_wins() {
        assert_eq!(zone_mult("coastal", false, 1000), 1.2);
    }

    /// Foothills backwards slot 0 is stored as 620 → 600 (0x4A418C / 0x4A4196), which the strict
    /// test can never satisfy — the 1.2 is dead in the shipped game.
    #[test]
    fn an_inverted_zone_never_matches() {
        for s in [600, 610, 620, 630, 660] {
            assert_eq!(zone_mult("foothills", true, s), 1.0, "slice {s}");
        }
        // the arm's other slots still work
        assert_eq!(zone_mult("foothills", true, 700), 1.2);
    }

    /// **Every** track has two arms — there is no direction-independent one. Alps slice 320 is
    /// inside the forward arm's 300–330 ×1.6 and the backward arm's 300–450 ×1.4.
    #[test]
    fn every_track_has_two_arms() {
        assert_eq!(zone_mult("alps", false, 320), 1.6);
        assert_eq!(zone_mult("alps", true, 320), 1.4);
        // the five tracks that used to be one shared row all differ somewhere
        assert_eq!((zone_mult("castle", false, 51), zone_mult("castle", true, 51)), (0.95, 0.9));
        assert_eq!((zone_mult("coastal", false, 271), zone_mult("coastal", true, 271)), (1.1, 1.15));
        assert_eq!((zone_mult("forest", false, 821), zone_mult("forest", true, 821)), (0.9, 1.0));
        assert_eq!((zone_mult("monaco4", false, 6), zone_mult("monaco4", true, 6)), (1.0, 1.15));
        // monaco4's own slot 5 is 1.8 (0x4A3507), not the 1.4 the merged row carried
        assert_eq!(zone_mult("monaco4", false, 280), 1.8);
        // and its 440–480 belongs to the backward arm only — 475 is past the forward arm's
        // 405–470 and inside the backward arm's 440–480
        assert_eq!(zone_mult("monaco4", false, 475), 1.0);
        assert_eq!(zone_mult("monaco4", true, 475), 1.2);
    }

    #[test]
    fn divisors_and_unknown_tracks() {
        // 3385 is inside 3380–3390; **3390 itself is not**, because the bounds are strict
        assert_eq!(zone_mult("castle", true, 3385), 1.4);
        assert_eq!(zone_mult("castle", true, 3390), 1.0);
        assert_eq!(zone_mult("castle", true, 3380), 1.0);
        assert_eq!(width_divisor("castle"), 10.0);
        assert_eq!(width_divisor("forest"), 9.0);
        assert_eq!(width_divisor("alps"), 12.0);
        assert!(zones_for("skidpad", false).is_empty());
    }
}
