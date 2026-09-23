//! Camera views: the offset table the original keeps in `.data`, and what the rest of the game
//! keys off the active view id.
//!
//! The active view id is `word[0x627F3C]` (0..0x11). Five call sites turn it into an eye point the
//! same way (0x4618C7, 0x461A49, 0x461D67, 0x461EB7, 0x462401):
//!
//! ```text
//! offset = [0x5CF610 + view*16]                 ; vec3 + pad
//! eye    = car+0x330 + M(car+0x364) · offset    ; 0x461903 rotate, 0x46196F..0x4619A1 add
//! ```
//!
//! `car+0x364` has the car's axes in its rows (`physics.md` §2: +0x364 x-axis, +0x370 up,
//! +0x37C forward) and `car+0x330` is the position the body is drawn at, which the ground code
//! keeps at `ground + 0.05` (`physics.md` §6). So the offsets below are metres in the car's body
//! frame — +x right, +y up, +z forward — measured from a point 5 cm above the road.

use glam::Vec3;

/// The table at **0x5CF610**, 16 bytes per view, read out of the shipped `nfs5.exe`, with the name
/// the game knows each view by — the array at **0x5D6470** pairs those names with these ids, and
/// `fe.txt` names the cameras a car cycles through (`car[0].camera`). CONFIRMED.
///
/// Entry 1 is a placeholder: loading a car overwrites it with that car's own `drivercampos`
/// (0x46427F copies `carInfo+0x10E8` into `0x5CF620..0x5CF628` and `carInfo+0x10F4` into
/// `0x627ECC..0x627ED4`), which is why it ships with the same value as the bumper view. The views
/// whose offset is zero take their position from somewhere else: `TVCam` from the `.scn` camera
/// element a trigger switched to (§12), `AnimCam` from a `CmAn` animation, the cop cameras from the
/// car they follow.
/// A camera slot's field-of-view half-angle as SetSlotView 0x465790 leaves it (`+0x58` = 37.5,
/// 0x46584F…0x465886), in degrees.
pub const DEFAULT_HALF_ANGLE: f32 = 37.5;

/// The vertical field of view, in radians, of a slot half-angle `h` (degrees). The build 0x464F80
/// hands the view `+0x130 = h` (0x4653E9…0x46540B) and the projection 0x5391B0 takes `2·h` as the
/// **horizontal** angle with the viewport's aspect in 640 × 480 units (0x466FD7…0x46700C:
/// `m00 = cot h`, `m11 = m00 · 640/480`), so the full-screen view is `2·atan(tan h · 0.75)` high —
/// 59.8° for the default. (`[0x657D5C]`'s ×1.2 is not carried.)
pub fn fov_y(half_angle_deg: f32) -> f32 {
    2.0 * (half_angle_deg.to_radians().tan() * 0.75).atan()
}

/// `[0x657D5C]` — the front end's "wide screen" (`Graphics.WideScreen`). The main view's viewport is
/// then `y` 0.15…0.85 of the screen (0x468E96…0x468EA4 / 0x468E30 → 0x466EB0) and the half-angle ×1.2
/// (`[0x5B2480]`, 0x4653E9) unless the slot's view is 1. The projection's aspect is the viewport's,
/// so the band shows `2·atan(tan h · 0.75 / 0.7)` of the full window's height.
pub const WIDESCREEN_BAND: f32 = 0.7;
pub const WIDESCREEN_ZOOM: f32 = 1.2;

/// The slot half-angle with the wide-screen factor applied (`view` is the slot's view id).
pub fn half_angle(base: f32, widescreen: bool, view: u16) -> f32 {
    if widescreen && view != 1 { base * WIDESCREEN_ZOOM } else { base }
}

/// The full-window vertical field of view that puts the wide-screen band's projection on the
/// band: the viewport is `WIDESCREEN_BAND` of the height, so the whole window spans `1/0.7` of it.
pub fn fov_y_window(half_angle_deg: f32, widescreen: bool) -> f32 {
    let band = if widescreen { WIDESCREEN_BAND } else { 1.0 };
    2.0 * (half_angle_deg.to_radians().tan() * 0.75 / band).atan()
}

pub const VIEW_OFFSETS: [Vec3; 20] = [
    Vec3::new(0.0, 0.40, 0.90),   // 0x00 BumperCam
    Vec3::new(0.0, 0.40, 0.90),   // 0x01 BumperCam2 — replaced per car
    Vec3::new(0.0, 1.15, -3.30),  // 0x02 TailCam0
    Vec3::new(0.0, 1.35, -4.50),  // 0x03 TailCam1
    Vec3::new(0.0, 1.15, -4.80),  // 0x04 TailCam2
    Vec3::new(0.0, 2.00, -6.00),  // 0x05 HeliCam0
    Vec3::new(0.0, 2.00, -6.00),  // 0x06 HeliCam1
    Vec3::new(0.0, 4.90, -6.70),  // 0x07 HeliCam2
    Vec3::new(-1.75, 0.25, -3.75), // 0x08 ShotgunCam2
    Vec3::new(0.0, 1.53, 4.10),   // 0x09 ShotgunCam3 — ahead of the car
    Vec3::ZERO,                   // 0x0A CircleCam
    Vec3::ZERO,                   // 0x0B SplineCam
    Vec3::ZERO,                   // 0x0C TVCam
    Vec3::new(0.0, 15.0, -2.0),   // 0x0D BlimpCam
    Vec3::ZERO,                   // 0x0E AnimCam
    Vec3::new(0.0, 1.00, -0.50),  // 0x0F ReverseCam1
    Vec3::ZERO,                   // 0x10 (unnamed)
    Vec3::ZERO,                   // 0x11 CopCam1
    Vec3::ZERO,                   // 0x12 CopCam2
    Vec3::ZERO,                   // 0x13 CopCam3
];

/// The name the game knows each view by (`0x5D6470`, pairs of `{name, id}`; `DirectorsCam` is 0x14
/// and `LastCam` 0x15 terminates the list). CONFIRMED.
pub const VIEW_NAMES: [(&str, u16); 21] = [
    ("BumperCam", 0x00),
    ("BumperCam2", 0x01),
    ("TailCam0", 0x02),
    ("TailCam1", 0x03),
    ("TailCam2", 0x04),
    ("HeliCam0", 0x05),
    ("HeliCam1", 0x06),
    ("HeliCam2", 0x07),
    ("ShotgunCam2", 0x08),
    ("ShotgunCam3", 0x09),
    ("CircleCam", 0x0A),
    ("SplineCam", 0x0B),
    ("TVCam", 0x0C),
    ("BlimpCam", 0x0D),
    ("AnimCam", 0x0E),
    ("ReverseCam1", 0x0F),
    ("CopCam1", 0x11),
    ("CopCam2", 0x12),
    ("CopCam3", 0x13),
    ("DirectorsCam", 0x14),
    ("LastCam", 0x15),
];

/// The view a camera name stands for.
pub fn view_by_name(name: &str) -> Option<u16> {
    VIEW_NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, v)| *v)
}

/// The cameras a car cycles through, from the registry's `car[<n>].camera` list — the shipped
/// `fe.txt` gives the player `BumperCam2, BumperCam, TailCam0, HeliCam2`. Unknown names are
/// dropped; an empty or missing list falls back to that same four.
pub fn cycle_from_registry(r: &nfs_formats::fetxt::Registry, car: usize) -> Vec<u16> {
    let key = format!("car[{car}].camera");
    let names = r.get(&key).unwrap_or_default();
    let v: Vec<u16> = names.split(',').filter_map(view_by_name).collect();
    if v.is_empty() { vec![0x01, 0x00, 0x02, 0x07] } else { v }
}

/// The view whose offset the car's `.tpg` replaces.
pub const VIEW_DRIVER: usize = 1;

/// Whether the camera's own car is drawn in this view — **it is not**, in the bumper and driver
/// views (0x4383FD: when `carInfo+0x520` is the car the camera follows and the view id is 0 or 1,
/// the per-car render call `0x43EF80` is skipped). CONFIRMED.
pub fn draws_own_car(view: u16) -> bool {
    view > 1
}

/// The engine-audio mode for a view (`engine-audio.md` §4.2, the 16-entry jump table at
/// `0x4AA95C`): 1 for the bumper view, 2 for the three chase views, and for everything else 3 when
/// the camera is detached from the car, 0 (the `.cct`/`.clt` cockpit layers) when it is attached
/// and interior. MEDIUM.
/// `attached` is `[0x627F43] & 8`, which 0x46596B…0x465986 computes as **`viewId <= 1`** (`cmp word
/// [0x627F3C], 1 ; setle ; shl 3`), and `interior` is `!0x443EE0(cam)` — the camera's own exterior
/// flag, MEDIUM (`engine-audio.md` §4.2), which this port reads as "the driver view".
pub fn audio_mode(view: u16, attached: bool, interior: bool) -> u8 {
    match view {
        0 | 15 => 1,
        2..=4 => 2,
        _ if attached => {
            if interior {
                0
            } else {
                1
            }
        }
        _ => 3,
    }
}

/// **The look-around step 0x4611C0**, run by the bumper (0x461D50) and driver (0x461A30) updates
/// once the slot's matrix is the car's: the target is 0°, −90° / +90° for input bits 0x14 / 0x15,
/// `180 − that/2` with 0x13 (look back), halved again with 0x16; the slot's yaw `+0xC6` (a word,
/// reset to 0 by SetSlotView 0x4658B2 and by 0x465A60) moves toward it by a quarter of the
/// difference, truncated, the short way round, and wraps at 360 (a value of exactly 360 is kept,
/// 0x4612CE `jle`). A quarter truncates to 0 inside 4°, so the yaw settles 1–3° short of the target
/// (180 is reached as 183 from above, 0 as 357 coming back). Returns the new yaw and whether the
/// matrix is turned by it: the update skips the turn only when the yaw already equals the target
/// (0x46123D), so a settled yaw still turns the view every update.
pub fn look_step(cur: i16, target: i16) -> (i16, bool) {
    if target == cur {
        return (cur, false);
    }
    let mut si = target as i32;
    if si < 0 {
        si += 360;
    }
    let mut bp = si + 180;
    if bp >= 360 {
        bp -= 360;
    }
    let cx = cur as i32;
    // `(d + (d < 0 ? 3 : 0)) >> 2`, made positive (0x46126A…0x461284)
    let q = |d: i32| (d / 4).abs();
    let diff = si - cx;
    let step = q(diff);
    let new = if si < 180 {
        if cx > si {
            if cx <= bp { cx - step } else { cx + q(diff + 360) }
        } else if cx == si {
            cx
        } else {
            cx + step
        }
    } else if cx > si {
        cx - step
    } else if cx == si {
        cx
    } else if cx > bp {
        cx + step
    } else {
        cx - q(diff - 360)
    };
    let mut ax = new as i16;
    if ax > 360 {
        ax -= 360;
    } else if ax < 0 {
        ax += 360;
    }
    (ax, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_exe() {
        // the bumper view sits 0.9 m ahead of the car origin at bumper height, the chase views
        // behind and above it, the furthest 6 m back
        assert_eq!(VIEW_OFFSETS[0], Vec3::new(0.0, 0.4, 0.9));
        assert_eq!(VIEW_OFFSETS[VIEW_DRIVER], VIEW_OFFSETS[0]);
        assert!(VIEW_OFFSETS[2..8].iter().all(|v| v.z < 0.0 && v.y > 1.0), "the chase and heli views sit behind and above");
        assert_eq!(VIEW_OFFSETS[7], Vec3::new(0.0, 4.9, -6.7), "HeliCam2 is the high one");
        assert_eq!(VIEW_OFFSETS[13], Vec3::new(0.0, 15.0, -2.0), "the blimp");
        // the names the game knows the views by
        assert_eq!(view_by_name("TVCam"), Some(0x0C));
        assert_eq!(view_by_name("DirectorsCam"), Some(0x14));
        assert_eq!(view_by_name("nope"), None);
    }

    #[test]
    fn own_car_hidden_in_the_two_attached_front_views() {
        assert!(!draws_own_car(0));
        assert!(!draws_own_car(1));
        assert!(draws_own_car(2));
        assert!(draws_own_car(12));
    }

    #[test]
    fn audio_modes() {
        assert_eq!(audio_mode(0, true, false), 1);
        assert_eq!(audio_mode(1, true, true), 0);
        assert_eq!(audio_mode(3, true, false), 2);
        assert_eq!(audio_mode(12, false, false), 3);
    }

    #[test]
    fn look_back_swings_the_short_way_and_settles_short() {
        // 0 → 180 goes down through 315 and stops at 183; back to 0 climbs to 357
        let mut y = 0i16;
        let mut seen = vec![];
        for _ in 0..40 {
            y = look_step(y, 180).0;
            seen.push(y);
        }
        assert_eq!(seen[0], 315);
        assert_eq!(y, 183);
        for _ in 0..40 {
            y = look_step(y, 0).0;
        }
        assert_eq!(y, 357);
        assert_eq!(look_step(0, 0), (0, false));
        assert_eq!(look_step(357, 0), (357, true));
    }
}
