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
//! +0x37C forward) and `car+0x330` is the body's origin — half the model box (`car+0x3AC`,
//! 0x445090) above the contact plane; the body is drawn `M[1]·car+0x3AC` below it (0x43A410).
//! *(Corrected 2026-09-25: this said `car+0x330` is where the body is drawn, 5 cm above the road.)*
//! So the offsets below are metres in the car's body frame — +x right, +y up, +z forward —
//! measured from that origin.

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
/// 59.8° for the default. (`[0x657D5C]`'s ×1.2 is [`half_angle`]'s, with [`fov_y_window`] for its band.)
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

/// A scene view's own half-angle `+0x130` as its constructor 0x4672F0 sets it (0x4673D5, `0x42340000`).
/// The build 0x464F80 overwrites it only in the scene manager's **first** view — `[[0x628140] + 4]`
/// (0x464F91…0x464FA0), the main view 0x468E60 makes (0x468E93) — even when it builds the rear
/// camera (type 1, 0x4611A0 from the mirror pass 0x468D94), so the rear-view mirror `[0x628144]`
/// keeps this: 90° across, and no wide-screen ×1.2.
pub const VIEW_HALF_ANGLE: f32 = 45.0;

/// The vertical field of view (radians) of a horizontal half-angle `h` (degrees) on a viewport of
/// `w` × `h_px` in 640 × 480 units — 0x466F30's aspect (0x466FD7…0x466FF8) into 0x5391B0's
/// `m11 = cot h · aspect`.
pub fn fov_y_viewport(half_angle_deg: f32, w: f32, h_px: f32) -> f32 {
    2.0 * (half_angle_deg.to_radians().tan() * h_px / w).atan()
}

/// A camera slot's `+0x5C` as SetSlotView leaves it (0x46588C, `0x3F000000`): the **near plane**
/// the build 0x464F80 gives the main view. It stores `+0x5C` in the view's `+0x18` (0x465425 /
/// 0x46542D → 0x465439), which 0x466F30 hands the projection 0x5391B0 as its near (0x466FE0; the
/// camera's `+0xCC`, where the depth range starts).
pub const DEFAULT_NEAR: f32 = 0.5;

/// The main view's near plane from the slot's `+0x5C`: less `[0x5B3EB0]` 0.35 when the slot's view
/// is 1 (the driver view) — and in the rear build, type 1, whose write lands on the main view after
/// it is drawn — else as it is (0x465413…0x465439). So the driver view has 0.15, the camera
/// object's own near (0x4677F5), and every other view 0.5.
pub fn near_plane(slot_near: f32, view: u16) -> f32 {
    if view == 1 { slot_near - 0.35 } else { slot_near }
}

/// The near plane 0x462B80 writes to `+0x5C` with a new half-angle `a` (degrees): below 37.5
/// (`[0x5B3C9C]`), `max((37.5 − a) / a, [0x5B23F4] 0.2)`, else `[0x5B23F0]` 0.5 — the dolly branch
/// 0x462CA5…0x462CE3 and the stepping one 0x462DC7…0x462E09 alike. A narrow zoom pushes the near
/// plane out: 9.0 at 3.75°.
pub fn zoom_near(a: f32) -> f32 {
    if a < DEFAULT_HALF_ANGLE { ((DEFAULT_HALF_ANGLE - a) / a).max(0.2) } else { DEFAULT_NEAR }
}

/// **CircleCam 0x462980** (view 0x0A), the slot's two words: the angle count `+0xC6` and the
/// height step `+0xC4`. `+0xC6` is the same word the look-around yaw uses (0x4611C0); SetSlotView
/// zeroes it (0x4658B2, and 0x465BB8), while `+0xC4` has no writer but this update's `inc`
/// (0x462A47 — a byte scan of `0x627F44` finds nothing else), so the height step carries over from
/// one circling to the next.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CircleCam {
    /// `+0xC6`, a signed word, one up per update
    pub count: i16,
    /// `+0xC4`, a signed word, one up whenever `(count − 0x100) & 0x1FF` is 0
    pub step: i16,
}

impl CircleCam {
    /// The three eye heights, built on the stack at 0x462988…0x46299A (1.0, 2.0, 3.0).
    pub const HEIGHTS: [f32; 3] = [1.0, 2.0, 3.0];
    /// The ellipse's half-axes across and along the car: `[0x5B369C]` 4.5 on x and `[0x5B2460]`
    /// 6.0 on z (0x462A17 / 0x462A35).
    pub const RADII: (f32, f32) = (4.5, 6.0);

    /// SetSlotView's reset of `+0xC6` (0x4658B2); `+0xC4` is kept.
    pub fn reset(&mut self) {
        self.count = 0;
    }

    /// One update of 0x462980 (0x4629E9…0x462AD8): the eye's offset from `car+0x330` in the car's
    /// body frame, before the matrix `car+0x364` turns it (0x532A40) and the two ceiling tests
    /// 0x461370 at the car and at the old eye bound its y (the lower wins, 0x462B15…0x462B35).
    ///
    /// `+0xC6` goes up one; the angle is `count · [0x5B25B8]` 1/1024 of a turn, whose sine and
    /// cosine 0x532800 returns (its second and third arguments), and the offset is
    /// `(cos · 4.5, y, sin · 6.0)`. `+0xC4` steps when `(count − 0x100) & 0x1FF == 0`; `y` is
    /// `HEIGHTS[step mod 3]`, and while `(count − 0x80) & 0x1FF < 0x80` it blends toward
    /// `HEIGHTS[(step + 1) mod 3]` by `(count & 0x7F) · [0x5B273C]` 1/128 (0x462A4E…0x462ABC). So one
    /// lap is 1024 updates, the height changes twice a lap, and each change is a 128-update ramp
    /// that ends where `+0xC4` steps.
    pub fn step(&mut self) -> Vec3 {
        self.step_count(1)
    }

    /// `n` updates (one per game tick, 0x463C30 being a 64 Hz scheduler entry), then the offset
    /// the last of them leaves; with `n` = 0 the offset of the current count.
    pub fn step_count(&mut self, n: u32) -> Vec3 {
        for _ in 0..n {
            self.count = self.count.wrapping_add(1);
            if (self.count as i32 - 0x100) & 0x1FF == 0 {
                self.step = self.step.wrapping_add(1);
            }
        }
        let c = self.count as i32;
        let (s, co) = (c as f32 * (1.0 / 1024.0) * std::f32::consts::TAU).sin_cos();
        // `idiv 3` is a signed remainder; `+0xC4` would need 2^15 steps (2^24 updates) to go
        // negative, so the euclidean one is the same here
        let h = |k: i32| Self::HEIGHTS[k.rem_euclid(3) as usize];
        let k = self.step as i32;
        let y = if (c - 0x80) & 0x1FF < 0x80 {
            h(k) + (h(k + 1) - h(k)) * (c & 0x7F) as f32 * (1.0 / 128.0)
        } else {
            h(k)
        };
        Vec3::new(co * Self::RADII.0, y, s * Self::RADII.1)
    }
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

/// The up row of an aimed view's matrix (0x464BF0, the build's aim for `+0xC0 & 0x08`):
/// `fwd` is the unit direction from the eye to the aim point. The up starts as the world's
/// (0, 1, 0) (0x464C0B) and is kept when `|up · fwd|` is at most `[0x5B3F80]` 0.99
/// (0x464EE8…0x464F31, C0|C3 → keep); otherwise — looking nearly straight down or up, as the
/// BlimpCam always does from 15 m over and 2 m behind — the matrix's **previous** up row
/// `[out + 0xC]` is taken instead (0x464F33…0x464F45). Then `right = up × fwd` and
/// `up = fwd × right`, each normalised (0x532880 / 0x5328D0 at 0x464F48…0x464F67): the result is
/// that up with its part along `fwd` removed.
pub fn aim_up(fwd: Vec3, prev_up: Vec3) -> Vec3 {
    let up = if Vec3::Y.dot(fwd).abs() > 0.99 { prev_up } else { Vec3::Y };
    let right = up.cross(fwd).normalize_or_zero();
    fwd.cross(right).normalize_or_zero()
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
    fn circle_cam_laps_the_car_on_its_ellipse() {
        let mut c = CircleCam::default();
        // the first update is one 1024th of a turn: nearly straight out along +x at 4.5
        let o = c.step();
        assert!((o.x - 4.5).abs() < 1e-3 && o.z > 0.0 && o.z < 0.05, "{o}");
        assert_eq!(o.y, 1.0);
        // count 0x100 is a quarter lap: along +z at the ellipse's 6.0
        let o = CircleCam { count: 0xFF, step: 0 }.step();
        assert!(o.x.abs() < 1e-3 && (o.z - 6.0).abs() < 1e-3, "{o}");
        // half a lap: behind on −x
        let o = CircleCam { count: 0x1FF, step: 0 }.step();
        assert!((o.x + 4.5).abs() < 1e-3 && o.z.abs() < 1e-3, "{o}");
    }

    #[test]
    fn circle_cam_height_ramps_then_steps() {
        let mut c = CircleCam::default();
        let ys: Vec<f32> = (0..0x300).map(|_| c.step().y).collect();
        // counts 1…0x7F: flat at 1.0; 0x80…0xFF: the ramp toward 2.0 by (count & 0x7F)/128
        assert_eq!(ys[0x7E], 1.0);
        assert_eq!(ys[0x7F], 1.0, "count 0x80 starts the ramp at 0");
        assert!((ys[0xBF] - 1.5).abs() < 1e-6, "count 0xC0 is half way");
        assert!((ys[0xFE] - (1.0 + 127.0 / 128.0)).abs() < 1e-6);
        // count 0x100: `+0xC4` steps, the height is 2.0 and stays until the next ramp at 0x280
        assert_eq!(ys[0xFF], 2.0);
        assert_eq!(ys[0x27E], 2.0);
        assert!((ys[0x2BF] - 2.5).abs() < 1e-6);
        assert_eq!(c.step, 2, "stepped at counts 0x100 and 0x300");
        // the third height ramps back to the first
        let mut d = CircleCam { count: 0x4BF, step: 2 };
        assert!((d.step().y - 2.0).abs() < 1e-6, "3.0 halfway back to 1.0");
        // SetSlotView restarts the angle but keeps the height step
        c.reset();
        assert_eq!((c.count, c.step), (0, 2));
    }

    #[test]
    fn the_mirror_keeps_its_own_45_degrees() {
        // 90° across the 240 × 84 tray is 2·atan(84/240) = 38.6° high
        let v = fov_y_viewport(VIEW_HALF_ANGLE, 240.0, 84.0).to_degrees();
        assert!((v - 38.58).abs() < 0.01, "{v}");
        // the full-screen main view agrees with `fov_y`
        assert!((fov_y_viewport(DEFAULT_HALF_ANGLE, 640.0, 480.0) - fov_y(DEFAULT_HALF_ANGLE)).abs() < 1e-6);
    }

    #[test]
    fn near_planes() {
        assert_eq!(near_plane(DEFAULT_NEAR, 2), 0.5);
        assert!((near_plane(DEFAULT_NEAR, 1) - 0.15).abs() < 1e-6, "the driver view");
        assert_eq!(zoom_near(37.5), 0.5);
        assert_eq!(zoom_near(40.0), 0.5);
        assert!((zoom_near(3.75) - 9.0).abs() < 1e-5);
        assert_eq!(zoom_near(35.0), 0.2, "(37.5 − 35) / 35 is under the 0.2 floor");
    }

    #[test]
    fn aim_keeps_the_world_up_unless_looking_nearly_straight_down() {
        // a chase view: the world's up, made square to the view
        let f = Vec3::new(0.0, -0.3, 1.0).normalize();
        let u = aim_up(f, Vec3::X);
        assert!(u.dot(f).abs() < 1e-5 && u.y > 0.9);
        // the BlimpCam, 15 over and 2 behind: |up·fwd| = 0.991 > 0.99, the old up row is kept
        let f = Vec3::new(0.0, -15.0, 2.0).normalize();
        let prev = Vec3::new(0.0, 0.0, 1.0);
        let u = aim_up(f, prev);
        assert!(u.dot(f).abs() < 1e-5);
        assert!(u.z > 0.9, "{u}");
        // the car slides sideways under it: the world-up rule would turn the picture, the old
        // row does not
        let f2 = Vec3::new(2.0, -15.0, 0.0).normalize();
        let u2 = aim_up(f2, u);
        assert!(u2.z > 0.9, "{u2}");
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
