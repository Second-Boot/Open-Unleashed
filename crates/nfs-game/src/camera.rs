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
//! (history: `game-history.md` §15.1)
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
    // det-allow: camera shake offset, drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
    2.0 * (half_angle_deg.to_radians().tan() * 0.75).atan()
}

/// `[0x657D5C]` — the front end's "wide screen" (`Graphics.WideScreen`). The main view's viewport is
/// then `y` 0.15…0.85 of the screen (0x468E96…0x468EA4 / 0x468E30 → 0x466EB0) and the half-angle ×1.2
/// (`[0x5B2480]`, 0x4653E9) unless the slot's view is 1. The projection's aspect is the viewport's
/// (0x466F30: `A = dx·640 / (dy·480)`, `dy` 0.7), and 0x5391B0 multiplies `m11` by it, so the band's
/// own vertical tangent is `tan h · 0.75 · 0.7` over its 0.7 of the screen — the full window's
/// `tan h · 0.75` cut to the band, the horizontal field unchanged (the viewport 0x538DA0
/// maps NDC onto the band itself; its `+0x200` / `+0x204` scales are 1 for a viewport inside the
/// screen, 0x538FBF…0x538FFE). (history: `game-history.md` §15.2)
pub const WIDESCREEN_BAND: f32 = 0.7;
pub const WIDESCREEN_ZOOM: f32 = 1.2;

/// The slot half-angle with the wide-screen factor applied (`view` is the slot's view id).
pub fn half_angle(base: f32, widescreen: bool, view: u16) -> f32 {
    if widescreen && view != 1 { base * WIDESCREEN_ZOOM } else { base }
}

/// The full-window vertical field of view of the main view. The port draws the scene over the
/// whole window and the wide-screen bars over it: the band viewport's projection (aspect
/// `r / 0.7`) is the full window's cut to the band, so the band needs no term here
/// ([`WIDESCREEN_BAND`]) — only the ×1.2 of [`half_angle`].
/// The aspect is the current mode's ([`set_mode`]): the shipped game's `nfs5.dll` replaces the
/// 640 / 480 of 0x466F30's aspect (0x466FE7 / 0x466FF4) by its virtual screen, whose ratio is the
/// mode's `W / H` on both sides of 4:3 ([`fov_y_window_at`]).
pub fn fov_y_window(half_angle_deg: f32) -> f32 {
    fov_y_window_at(half_angle_deg, mode_aspect())
}

/// [`fov_y_window`] for a mode of aspect `r = W / H`. 0x5391B0 takes `m00 = cot h`,
/// `m11 = cot h · A` (0x5391C1…0x5391F7) with 0x466F30's `A = dx·640 / (dy·480)` (0x466FD7…
/// 0x466FF8), whose 640 / 480 `nfs5.dll` rewrites to its virtual width / height (0x466FE7 /
/// 0x466FF4, dll 0x10003840: `virtW / virtH = r` above and below 4:3). So the vertical half-angle's
/// tangent is `tan h / r` — `tan h · 0.75` at 4:3, written `· 0.75 · (4/3) / r` so that a 4:3 mode
/// (640 × 480, 800 × 600, 1024 × 768 in `f32`) keeps the exact factor 1.
pub fn fov_y_window_at(half_angle_deg: f32, aspect: f32) -> f32 {
    let k = (4.0f32 / 3.0) / aspect;
    // det-allow: camera shake offset, drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
    2.0 * (half_angle_deg.to_radians().tan() * 0.75 * k).atan()
}

/// **`nfs5.dll` 0x10003840**, run on every SetMode with the mode `W × H` (`[0x1000611C]` /
/// `[0x10006120]`): the camera half-angle cell `[0x10006080]` that replaces the literal 37.5 at
/// every patched site (SetSlotView 0x46584F, the race init 0x46414B, the cycle key 0x465AE8, the
/// TV zoom 0x462B80 / 0x462BFC / 0x462C26 / 0x462CAB / 0x462CC4 / 0x462DD7 / 0x462DF0, the
/// `FakeSort` / view-distance gates 0x44ECB2 / 0x44F1B1, the view 0x4692E2 and the FE rooms
/// 0x4E8B7F / 0x4E9900 / 0x4E9BAA; `RE/notes/nfs5-dll.md` §3.5). `r = W / H` (`fild` / `fdiv`,
/// 0x1000384C…0x10003868); 4/3 is the double 1.3333333730697632 at 0x10005600, and `fcomi` / `jb`
/// (0x10003872) takes the wide branch only when `4/3 < r`:
/// * `r ≤ 4/3`: `[0x5B3C9C]` = 37.5 (0x10003896) — the **horizontal** half-angle stays, so a
///   window taller than 4:3 sees more above and below;
/// * `r > 4/3`: `atan2(3·r, 4 / tan(37.5 · π/180)) / (π/180)` (`_CItan` 0x100038D8, `_CIatan2`
///   0x100038E9; 3.0 / 4.0 / π/180 the doubles at 0x10005608 / 0x10005610 / 0x100055F8) — the
///   half-angle that keeps the 4:3 vertical field.
pub fn dll_half_angle(w: f32, h: f32) -> f32 {
    /// The dll's π/180 double (0x100055F8), as the port has always spelled it.
    const DLL_DEG_TO_RAD: f64 = 0.017_453_292_f64;
    let r = w as f64 / h as f64;
    if 1.333_333_373_069_763_2 < r {
        let rad = DLL_DEG_TO_RAD;
        // det-allow: the dll's mode set, drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
        ((3.0 * r).atan2(4.0 / (DEFAULT_HALF_ANGLE as f64 * rad).tan()) / rad) as f32
    } else {
        DEFAULT_HALF_ANGLE
    }
}

/// `[0x10006080]` (37.5 until the first mode set)
static MODE_HALF_ANGLE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x4216_0000);
/// the mode's `W / H` (4/3 in `f32` until the first mode set)
static MODE_ASPECT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3FAA_AAAB);

/// The SetMode hook's cells for a window of `w × h`: the dll keeps them as globals written once per
/// mode set; the port's resizable window sets them on every size. 0 × 0 (minimised) is ignored.
pub fn set_mode(w: u32, h: u32) {
    if w == 0 || h == 0 {
        return;
    }
    use std::sync::atomic::Ordering::Relaxed;
    MODE_HALF_ANGLE.store(dll_half_angle(w as f32, h as f32).to_bits(), Relaxed);
    MODE_ASPECT.store((w as f32 / h as f32).to_bits(), Relaxed);
}

/// The dll's `[0x10006080]` for the current mode — the default slot half-angle every patched 37.5
/// reads ([`dll_half_angle`]).
pub fn mode_half_angle() -> f32 {
    f32::from_bits(MODE_HALF_ANGLE.load(std::sync::atomic::Ordering::Relaxed))
}

/// The current mode's `W / H` ([`set_mode`]).
pub fn mode_aspect() -> f32 {
    f32::from_bits(MODE_ASPECT.load(std::sync::atomic::Ordering::Relaxed))
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
    // det-allow: camera shake offset, drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
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
/// plane out: 9.0 at 3.75°. `nfs5.dll` rewrites the compare and the minuend `[0x5B3C9C]` (0x462CAB /
/// 0x462CC4, 0x462DD7 / 0x462DF0) to its cell `[0x10006080]` ([`mode_half_angle`]).
pub fn zoom_near(a: f32) -> f32 {
    zoom_near_at(a, mode_half_angle())
}

/// [`zoom_near`] against a given default half-angle `hd` (the dll's cell).
pub fn zoom_near_at(a: f32, hd: f32) -> f32 {
    if a < hd { ((hd - a) / a).max(0.2) } else { DEFAULT_NEAR }
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
        // det-allow: camera shake offset, drawing only (PORT-DEVIATIONS D1: drawing sites keep std)
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

/// Whether the camera's own car runs its particle effects' drive in this view — **it does not**
/// in the bumper and driver views (0x4383FD: when `carInfo+0x520` is the car the camera follows and
/// the view id is 0 or 1, `[car+0x704] → 0x43EF80(1)` is skipped; `nfs_game::car_effects`).
/// CONFIRMED. (history: `game-history.md` §15.3) The gate
/// is slot 7's only: the odd tick's walk 0x443640 drives the same car through slot 8 (0x438420)
/// with no view test, so the own car's effects still run at 32 Hz in these views.
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

/// **The playback's wander of the aim point** — 0x464BF0's `[0x5E99F4] == 2` branch
/// (0x464C2B…0x464E42), read 2026-09-27. When the build aims a view at its car (no target handed
/// in: 0x46527F / 0x46560C, not view 0x11's 0x4652A7 / 0x465634) while a replay **plays** (`== 2`:
/// not once the stream has run out, 3) and the slot's view word `+0xBC` is not 2…7, 0x0A or 0x0B,
/// the aim point is `car+0x330` plus this offset `[0x6280F8…0x628100]` instead of the car's origin:
///
/// * every time `gGameTick − [0x6280E0] > [0x6280E4]` (`jle`, signed) four draws of the track
///   particles' generator `[0x5CEAE0]` (0x464C8B…0x464D32) give a new goal `[0x6280E8…0x6280F0]`,
///   each axis `r · s / 65536 − s / 2` with `s = car+0x35C · [0x5B3F6C] 0.05` (the speed estimate,
///   so the spread grows with speed), and the next interval `fistp(r₄ · 3 / 65536) + 3` ticks (3…6);
///   `[0x6280E0]` = the tick;
/// * each build, unless paused (`[0x606860]`, `[0x606A90]`, `[0x606A98]` all 0, 0x464DA4…0x464DBD),
///   the offset moves toward the goal by `k`: `[0x606864]` 1 (quarter speed) → 0.0025, 0 (half)
///   → 0.005, else 0.01 (0x464D7E…0x464DA4).
///
/// SetSlotView 0x465790 zeroes all of it (0x4657A0…0x4657F3). The branch's other half, 0x464ED6,
/// takes the up's x from the slot's `+0xA0` `[0x627F20]` in a playback: no writer of it was found
/// (the one reference in `.text` is that read; it lies in `.bss`), so the up stays (0, 1, 0) and
/// nothing is carried for it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayWander {
    /// `[0x6280E0]`, the tick of the last goal
    pub last: i32,
    /// `[0x6280E4]`, the ticks until the next
    pub interval: i32,
    /// `[0x6280E8…0x6280F0]`
    pub goal: Vec3,
    /// `[0x6280F8…0x628100]`, added to the aim point
    pub offset: Vec3,
}

impl ReplayWander {
    /// Whether a view word takes the wander (0x464C45…0x464C6A).
    pub fn applies(view: u16) -> bool {
        !matches!(view, 0x02..=0x07 | 0x0A | 0x0B)
    }

    /// One aim: `tick` is `gGameTick`, `speed` the viewed car's `car+0x35C`, `k` the rate
    /// `[0x606864]` picks (`replay::Speed::camera_lag`), `paused` any of the three pause flags.
    /// Returns the offset to add to the aim point. The x87 work is done in f64 and stored as the
    /// original stores (`fstp dword`).
    pub fn step(&mut self, tick: i32, speed: f32, k: f32, paused: bool, rng: &mut crate::track_particles::ParticleRng) -> Vec3 {
        if tick.wrapping_sub(self.last) > self.interval {
            let s = speed as f64 * 0.05f32 as f64;
            let h = s * 0.5;
            let scale = 1.0 / 65536.0;
            let mut axis = || (rng.next() as f64 * s * scale - h) as f32;
            let (x, y, z) = (axis(), axis(), axis());
            self.goal = Vec3::new(x, y, z);
            // `fstp dword` then `fistp` (round to nearest even)
            let t = (rng.next() as f64 * 3.0 * scale) as f32;
            self.interval = t.round_ties_even() as i32 + 3;
            self.last = tick;
        }
        let k = k as f64;
        if !paused {
            let f = |old: f32, goal: f32| ((1.0 - k) * old as f64 + goal as f64 * k) as f32;
            self.offset = Vec3::new(f(self.offset.x, self.goal.x), f(self.offset.y, self.goal.y), f(self.offset.z, self.goal.z));
        }
        self.offset
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

/// **`AnimCam`, view 0x0E — the race's opening camera path** (`race-logic.md` §13).
///
/// A track's `.crp` carries five `CmAn` level entries (every shipped track: 6340 bytes each, named
/// `camera00` … `camera04` at `+0`). The track load 0x471850 (called at 0x48779C) makes one record
/// per entry, in table order, into the set `[0x628938]` — the set 0x471700 made at 0x48778A with
/// the clock `&gGameTick` (`[0x606AC4]`) and the step `0x3C800000` = 1/64 s. The entry's `+0x28` is
/// the key count (121 in every shipped one) and its keys start at `+0x2C`, 0x34 bytes each: the
/// quaternion `+0x00`, the position `+0x10`, the scale `+0x20` — the `Anqt` layout the article
/// animations use, read by the same sampler.
///
/// `0x471A50(set, 0)`, which the view's update 0x462F30 calls when its slot `+0x0C` holds no
/// animation: **the counter `[0x62893C]` is stepped first**, `(c + 1) mod count` (0x471A70…
/// 0x471A90), and the record at that index is played. The counter is a `.bss` word no other code
/// writes, so the rotation runs on through the process — the first race after launch plays
/// `camera01`. The player it builds (outer vtable 0x5B4634, inner record at `+0x10`) has flags 0,
/// **`0x3D124925` = 1/28 s a key** (0x471AEC — not the entry's `+0x24` 0.1, which 0x471850 keeps
/// but nothing reads), no drift, state 1 and the start `+0x08` = `gGameTick` at the pick.
pub mod anim_cam {
    use glam::{Mat3, Vec3};
    use nfs_formats::bytes;
    use nfs_formats::crp::{AnimFrame, AnimTrack};

    /// 0x471AEC: the player's seconds per key, `0x3D124925`.
    pub const SECONDS_PER_KEY: f32 = f32::from_bits(0x3D12_4925);
    /// `[0x628938]+8`, 0x471700's step (`0x3C800000`): a game tick.
    pub const STEP: f32 = 1.0 / 64.0;

    /// The five `CmAn` animations of a track, in table order (0x471850 walks the level entries from
    /// the first `CmAn`, 0x59F700, `ebx += 0x10`). An entry too short for its count is skipped.
    pub fn load(crp: &nfs_formats::crp::Crp, data: &[u8]) -> Vec<AnimTrack> {
        let f = |b: &[u8], o: usize| bytes::read_f32(b, o);
        crp.level
            .iter()
            .filter(|e| e.is_level(b"CmAn"))
            .filter_map(|e| {
                let b = crp.entry_data(data, e).ok()?;
                if b.len() < 0x2C {
                    return None;
                }
                let n = bytes::read_u32(b, 0x28) as usize;
                if n < 2 || b.len() < 0x2C + n * 0x34 {
                    return None;
                }
                let frames = (0..n)
                    .map(|k| {
                        let o = 0x2C + k * 0x34;
                        AnimFrame {
                            rotation: [f(b, o), f(b, o + 4), f(b, o + 8), f(b, o + 12)],
                            position: [f(b, o + 0x10), f(b, o + 0x14), f(b, o + 0x18)],
                            scale: [f(b, o + 0x20), f(b, o + 0x24), f(b, o + 0x28)],
                        }
                    })
                    .collect();
                Some(AnimTrack { frames })
            })
            .collect()
    }

    /// `0x471A50(set, 0)`: steps the rotation counter `[0x62893C]` and answers the index to play,
    /// `None` with no animation (the counter then stays: `test edx, ~3` on the vector's byte size,
    /// 0x471A68).
    pub fn pick(counter: &mut u32, count: usize) -> Option<usize> {
        if count == 0 {
            return None;
        }
        *counter = (counter.wrapping_add(1)) % count as u32;
        Some(*counter as usize)
    }

    /// The player's slot 1, 0x471670 → `AnimPlayer::sample` 0x470E90 → 0x470F70, `ticks` after
    /// its start (`max(0, gGameTick − start)`, 0x4716A2…0x4716B6): `rate = (1/28) / (1/64)` ticks
    /// a key (stored as a float, 0x470EAB), `f = ticks / rate`, `i = _ftol(f)`, `frac = f − i`; no
    /// loop flag, so `i >= count − 2` (unsigned, 0x470EF2) holds `i = count − 1`, `frac = 0`, and
    /// the answer `i + 1 < count` is false — status 2, the animation over (0x4716C4…0x4716CB).
    /// Otherwise the key pair's quaternion and position lerped per component (0x48A150), the
    /// matrix of the unnormalised quaternion as 0x48A1E0 fills it — rows of `R(q)ᵀ` — each row
    /// scaled by the lerped scale (0x5329A0), the position the translation (0x539CD0); the drift
    /// `+0x14` is zero. Answers `(A, t)`: `A` in the original's row-major sense (row `k` is the
    /// animation's axis `k`), `t` in the car's body frame.
    pub fn sample(track: &AnimTrack, ticks: i64) -> Option<(Mat3, Vec3)> {
        let n = track.frames.len();
        if n < 2 {
            return None;
        }
        let rate = SECONDS_PER_KEY / STEP;
        // `fild` / `fdiv` on the x87's wide stack, then `_ftol`
        let f = ticks.max(0) as f64 / rate as f64;
        let mut i = f as usize;
        let mut frac = (f - i as f64) as f32;
        if i >= n - 2 {
            i = n - 1;
            frac = 0.0;
        }
        if i + 1 >= n {
            return None;
        }
        let (a, b) = (&track.frames[i], &track.frames[i + 1]);
        let l = |x: f32, y: f32| (y - x) * frac + x;
        let q: [f32; 4] = std::array::from_fn(|k| l(a.rotation[k], b.rotation[k]));
        let p = Vec3::new(l(a.position[0], b.position[0]), l(a.position[1], b.position[1]), l(a.position[2], b.position[2]));
        let s = Vec3::new(l(a.scale[0], b.scale[0]), l(a.scale[1], b.scale[1]), l(a.scale[2], b.scale[2]));
        let [x, y, z, w] = q;
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (wx, wy, wz) = (x2 * w, y2 * w, z2 * w);
        let (xx, xy, xz) = (x2 * x, y2 * x, z2 * x);
        let (yy, yz, zz) = (y2 * y, z2 * y, z2 * z);
        let rows = [
            Vec3::new(1.0 - yy - zz, xy + wz, xz - wy) * s.x,
            Vec3::new(xy - wz, 1.0 - xx - zz, yz + wx) * s.y,
            Vec3::new(xz + wy, yz - wx, 1.0 - xx - yy) * s.z,
        ];
        Some((Mat3::from_cols(rows[0], rows[1], rows[2]).transpose(), p))
    }

    /// `0x4A4810`, the re-orthonormalisation 0x462F30 runs on the slot matrix last (0x463071):
    /// four passes of `D = MᵀM − I`, `S = I − D/8 + 3D²/32 − 5D³/64`, `M ← M·S` — the same
    /// routine `nfs_physics::vehicle::Vehicle::renormalise` ports for the car, on a plain matrix.
    pub fn renormalise(m: Mat3) -> Mat3 {
        nfs_physics::vehicle::renormalise_matrix(m)
    }

    /// **0x462F30's pose** from a sample `(A, t)` and the car's matrix `C` (`car+0x364`, row-major
    /// in the original's sense — this port's `car_rot` is `Cᵀ`) and origin `car+0x330`:
    ///
    /// * the eye `+0x10` = `car + t·C` (0x539D40 splits the 4×4, 0x532A40 turns `t`, 0x532300 adds);
    /// * the matrix `+0x68` = `R · A · C` (0x532470 at 0x46302E for `A·C`, then 0x539BD0(90°, (1, 0,
    ///   0)) through 0x5326C0 — rows `(1,0,0)`, `(0,0,1)`, `(0,−1,0)` — multiplied on the left at
    ///   0x46306B), re-orthonormalised (0x4A4810).
    ///
    /// So the camera's up row is the animation's `z` axis and its forward row the animation's
    /// `−y`, carried by the car. Answers the eye and the matrix (rows: right, up, forward).
    pub fn pose(a: Mat3, t: Vec3, car: Mat3, car_pos: Vec3) -> (Vec3, Mat3) {
        let eye = car_pos + car.transpose() * t;
        let r = Mat3::from_cols(Vec3::X, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
        (eye, renormalise(r * a * car))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_wander_draws_then_follows() {
        let mut w = ReplayWander::default();
        let mut rng = crate::track_particles::ParticleRng::new(7);
        // tick 1 − 0 > 0: a goal and an interval of 3…6, the offset `k` of the way to it
        let o = w.step(1, 40.0, 0.01, false, &mut rng);
        assert_eq!(w.last, 1);
        assert!((3..=6).contains(&w.interval));
        for a in [w.goal.x, w.goal.y, w.goal.z] {
            assert!((-1.0..1.0).contains(&a)); // ±s/2, s = 40 · 0.05
        }
        assert!((o - w.goal * 0.01).length() < 1e-6);
        // paused: no draw before the interval, no move
        let goal = w.goal;
        assert_eq!(w.step(2, 40.0, 0.01, true, &mut rng), o);
        assert_eq!(w.goal, goal);
        assert!(ReplayWander::applies(0x0C) && ReplayWander::applies(0x08) && !ReplayWander::applies(0x0A) && !ReplayWander::applies(3));
    }

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
        assert_eq!(zoom_near_at(37.5, 37.5), 0.5);
        assert_eq!(zoom_near_at(40.0, 37.5), 0.5);
        assert!((zoom_near_at(3.75, 37.5) - 9.0).abs() < 1e-5);
        assert_eq!(zoom_near_at(35.0, 37.5), 0.2, "(37.5 − 35) / 35 is under the 0.2 floor");
        // a wide mode's cell: 40° is now under it
        assert_eq!(zoom_near_at(40.0, 46.0), 0.2);
        assert!((zoom_near_at(20.0, 46.0) - 1.3).abs() < 1e-6);
    }

    #[test]
    fn the_dll_half_angle_by_aspect() {
        // 4:3 and taller keep the literal: the horizontal half-angle stays
        assert_eq!(dll_half_angle(640.0, 480.0), 37.5);
        assert_eq!(dll_half_angle(1024.0, 768.0), 37.5);
        assert_eq!(dll_half_angle(1280.0, 1024.0), 37.5);
        // wider: the vertical field stays — tan(hd) / r = tan(37.5°) · 0.75
        for (w, h) in [(1280.0f32, 800.0f32), (1920.0, 1080.0)] {
            let hd = dll_half_angle(w, h);
            assert!(hd > 37.5);
            assert!((fov_y_window_at(hd, w / h) - fov_y(37.5)).abs() < 1e-6, "{w}x{h}: {hd}");
        }
        // 5:4: the vertical field grows past 4:3's
        let v = fov_y_window_at(dll_half_angle(1280.0, 1024.0), 1.25);
        assert!((v - 2.0 * (37.5f32.to_radians().tan() / 1.25).atan()).abs() < 1e-6);
        assert!(v > fov_y(37.5));
        // a 4:3 mode gives exactly the number the port had before
        assert_eq!(fov_y_window_at(37.5, 640.0 / 480.0), 2.0 * (37.5f32.to_radians().tan() * 0.75).atan());
        // the wide-screen band is the full view cut to 0.7 of the height — no band term
        assert_eq!(fov_y_window_at(45.0, 1024.0 / 768.0), fov_y(45.0));
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

    #[test]
    fn anim_cam_rotation_steps_before_it_picks() {
        let mut c = 0;
        assert_eq!(anim_cam::pick(&mut c, 5), Some(1));
        assert_eq!(anim_cam::pick(&mut c, 5), Some(2));
        c = 4;
        assert_eq!(anim_cam::pick(&mut c, 5), Some(0));
        assert_eq!(anim_cam::pick(&mut c, 0), None);
        assert_eq!(c, 0);
    }

    #[test]
    fn anim_cam_runs_119_keys_of_121_and_stops_at_tick_273() {
        use nfs_formats::crp::{AnimFrame, AnimTrack};
        let frame = |z: f32| AnimFrame { rotation: [0.0, 0.0, 0.0, 1.0], position: [0.0, 0.0, z], scale: [1.0; 3] };
        let track = AnimTrack { frames: (0..121).map(|k| frame(k as f32)).collect() };
        // 2.2857144 ticks a key: tick 272 is key 118.99…, tick 273 key 119.4 → held, status 2
        let (a, t) = anim_cam::sample(&track, 272).unwrap();
        assert!(t.z > 118.9 && t.z < 119.0);
        assert_eq!(a, glam::Mat3::IDENTITY);
        assert!(anim_cam::sample(&track, 273).is_none());
        assert_eq!(anim_cam::sample(&track, -5).unwrap().1, Vec3::ZERO);
        let (_, t) = anim_cam::sample(&track, 16).unwrap();
        assert!((t.z - 7.0).abs() < 1e-4, "{t}");
    }

    #[test]
    fn anim_cam_looks_down_the_animations_minus_y_with_its_z_up() {
        // an identity key and an unturned car: the eye at the key's position, looking along −y
        let (eye, m) = anim_cam::pose(glam::Mat3::IDENTITY, Vec3::new(1.0, 2.0, 3.0), glam::Mat3::IDENTITY, Vec3::new(10.0, 0.0, 0.0));
        assert_eq!(eye, Vec3::new(11.0, 2.0, 3.0));
        assert!((m.row(2) - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-6);
        assert!((m.row(1) - Vec3::Z).length() < 1e-6);
        // a car turned a quarter about y (rows: x-axis, up, forward) carries both
        let car = glam::Mat3::from_cols(Vec3::new(0.0, 0.0, 1.0), Vec3::Y, Vec3::new(-1.0, 0.0, 0.0));
        let (eye, m) = anim_cam::pose(glam::Mat3::IDENTITY, Vec3::new(0.0, 0.0, 3.0), car, Vec3::ZERO);
        assert!((eye - car.row(2) * 3.0).length() < 1e-6);
        assert!((m.row(1) - car.row(2)).length() < 1e-6);
    }
}
