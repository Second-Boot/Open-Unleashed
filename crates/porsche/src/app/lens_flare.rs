//! The sun's lens flare, **0x460460** (`RE/notes/lighting.md` §7), called from 0x45D1A0 at the end
//! of the main view's pass (0x468B89 → 0x45D7F0, the view whose `+0x12C` is 2 — the rear-view
//! mirror's pass never runs it, 0x468B7B…0x468B87) after the world and the cars are drawn.
//!
//! **Gate** (0x460471…0x4604BE): the sun object is on (`+0x140`, [`SkyBin::sun_active`]), its
//! state `+0x144` is 1 (0 = the reference not taken yet, 2 = off for good), the sun is on the
//! screen (`+0x18D`, the sky pass's projection, `sun_point`), `LENSFLARE` `[0x657D7C]` is set, and
//! the sky's `+0xDD8` ([`SkyBin::sun_readback`]) is not 0.
//!
//! **Who sets `[0x657D7C]`.** Nothing writes it by address but the whole-block snapshot copy
//! 0x413CF0. `Graphics.LensFlare` is an FE accessor with record id 0x4C = `LENSFLARE` (0x4E5921,
//! `fcn.004bb680(name, 0x4C, …)`, no commit callback — the callbacks 0x4E5710 / 0x4E56D0 are
//! `CarChrome` / `CarDetail`'s) and the race launch **dumps every accessor** with a registry id
//! (0x4ED5D0 → 0x4ED653 → 0x4B7F10, the accessor map `[0x65B3A8]`, each one's slot 1 = 0x4BC3D0:
//! `(id, selection)` appended to the option stream), after which 0x4B4DA0 clears the option block
//! (`memset(0x6573E8, 0, 0x3E24)`, 0x4B4DB6) and applies the stream with the setter 0x4B4B80 (the
//! id band below 0x10E: `*row.ptr = value`, 0x4B4BCF; the row `{0x4C, 0x657D7C, "LENSFLARE"}`
//! is built at 0x4B1324). So a race started from the front end has `[0x657D7C]` = the accessor's
//! selection — 1 with "блики" on — whatever the globals read before the launch (0 at the first
//! front-end frame and after a profile load, which only touch the accessor objects). A race with
//! no front end (`fe = 0`) has no dump and `[0x657D7C]` = 0.
//!
//! **The test** (0x4604C4…0x46063F). Every 200 ms of `GetTickCount` (`+0x194`, set at the object's
//! creation) the 4 × 4 block of the frame at `(round(+0x12C) − 2, round(+0x130) − 2)` is read
//! (`THRASH_readrect`, 0x460525; on `dx7z.dll` 0x60003990 a lock of the back buffer and a row copy)
//! and compared texel by texel with the block stored at `+0x148`; each equal texel adds 1.0 to
//! `+0x190`; more than 0.1 sets the visible byte `+0x18C` (which nothing else reads). **The stored
//! block** is taken by the sun's own draw, 0x460290 (0x4603C3…0x460446): the first time the sun is
//! on screen with `+0x144` = 0, right after the sun's sprite is drawn in the sky pass, with
//! `[0x5DEB18]` (the CPU's MMX bit, `ISMMX` / cpuid 0x555DDC) and `[0x6197AA]` (the device takes
//! Thrash state 0x1C3, 0x44EB98) set; it stores the block, sets `+0x144` = 1 and the pixel size
//! (`+0x188` = 2, or 4 when `+0x184`), else `+0x144` = 2 for good. So "equal to the stored block"
//! means *nothing was drawn over the sun's pixels after the sky pass* — the sun is not hidden by
//! the mountains, a car or the weather. **The cost cut-off** (0x460533…0x4605D0): `+0x198` counts
//! the reads, `+0x19C` adds their milliseconds; past 10 reads and under 300, an average above
//! 5 ms (`[0x5B241C]` = 10.0, `[0x5B2524]` = 300.0, `[0x5B2400]` = 5.0) sets `+0x144` = 2 and
//! returns; above 300 both restart.
//!
//! **The draw** (0x460646…0x460765), every frame the gate holds, whatever the test said: the 2D
//! projection (0x467030), then for the thirteen records `bnus`…`nnus` (the dwords at 0x5CF530; the
//! shipped `flare.dat` has `bnus`…`inus`, the other five are not found, 0x452260) found once
//! (+0xA9 + e, 0x452100 drops one with both textures null) the record drawn by 0x451710 at the
//! screen centre `(W/2, H/2)` + `(sun − centre) · t[e + 1]`, `t` the 14 floats of
//! `GameData/Render/flarepos.dat` (`+0xB8`; the defaults 1, 0.75, −0.75, 0.5, −0.5, 0.25, −0.25,
//! 0, −1, then zeros, 0x4609F0, when the file is missing), with the scale `+0x190 · 0.0625`
//! (`[0x5B26A4]`), which 0x451710 turns into skipped layers under 0.05 and an alpha byte
//! `trunc(alpha · scale)` under 1 (`effects.rs` `flare_quads`).
//!
//! **PORT-DEVIATION D9 (PORT-DEVIATIONS.md) [0x460460, `THRASH_readrect` dx7z 0x60003990]: the frame read-back's latency.** The original's read is a blocking
//! lock of the back buffer inside the frame; the port copies the block into a staging buffer
//! (`nfs_renderer::LensProbe`) and gets the pixels a frame or two later (more with the render
//! thread), so the elements follow the sun's visibility that much late and the stored block is
//! captured that much after the first frame the sun is on screen. The compare itself is exact
//! (the target's own 32-bit texels against the stored 32-bit texels of the same copy), not the
//! original's 16-bit one (`+0x188` = 2 on a 16-bit mode). The cost cut-off times what collecting
//! a read costs the CPU, not a blocking stall, so it stays dormant.
//!
//! **APPROXIMATION.** `THRASH_readrect` does no clipping; a block that would leave the frame
//! (the sun within two pixels of an edge) is moved inside it here.

use crate::*;
use std::time::{Duration, Instant};

/// The thirteen records the elements are drawn from: the dwords at 0x5CF530, `bnus` … `nnus`.
const ELEMENTS: [u32; 13] = {
    let mut e = [0u32; 13];
    let mut i = 0;
    while i < 13 {
        e[i] = u32::from_le_bytes([b'b' + i as u8, b'n', b'u', b's']);
        i += 1;
    }
    e
};

/// `flarepos.dat`'s fourteen floats when the file is missing (0x460995…0x4609F9: nine stored,
/// the rest of the 56 bytes zero).
const DEFAULT_POSITIONS: [f32; 14] = [1.0, 0.75, -0.75, 0.5, -0.5, 0.25, -0.25, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0];

/// The test's cadence, `+0x194 + 0xC8` (0x4604D1).
const TEST_EVERY: Duration = Duration::from_millis(200);

/// How long a read-back asked for may take to come back before it is asked for again.
const ASK_LAPSES: Duration = Duration::from_millis(500);

/// `+0x144`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum LensMode {
    /// 0: the stored block is not taken yet
    #[default]
    Waiting,
    /// 1
    Active,
    /// 2: off for good
    Off,
}

/// The sun object's lens-flare fields (0x460780's constructor), per race.
#[derive(Debug)]
pub(crate) struct LensState {
    /// `LENSFLARE` `[0x657D7C]` ≠ 0
    pub enabled: bool,
    pub mode: LensMode,
    /// `+0x148`: the sixteen texels the sun's own draw left
    reference: Option<[u32; 16]>,
    /// when the reference was last asked for and has not come back; a scene built and never drawn
    /// (a scripted run builds one per tick) must not hold the request, so it lapses
    reference_asked: Option<Instant>,
    /// likewise a test
    test_asked: Option<Instant>,
    /// `+0x194`
    last_test: Instant,
    /// `+0x190`: how many of the sixteen texels still equal the stored ones
    pub matches: u32,
    /// `+0x198`, `+0x19C`
    reads: f32,
    cost_ms: f32,
    /// `+0xB8`
    positions: [f32; 14],
}

impl Default for LensState {
    fn default() -> Self {
        Self { enabled: false, mode: LensMode::Waiting, reference: None, reference_asked: None, test_asked: None, last_test: Instant::now(), matches: 0, reads: 0.0, cost_ms: 0.0, positions: DEFAULT_POSITIONS }
    }
}

impl LensState {
    /// 0x460533…0x460632 for one finished test read of `cost_ms` milliseconds: the cost
    /// bookkeeping and its cut-off, then the texel count.
    pub(crate) fn apply_test(&mut self, cost_ms: u32, pixels: &[u32; 16]) {
        let Some(reference) = self.reference else { return };
        self.matches = 0;
        self.reads += 1.0;
        self.cost_ms += cost_ms as f32;
        if self.reads > 10.0 && self.reads < 300.0 && self.cost_ms / self.reads > 5.0 {
            self.mode = LensMode::Off;
            return;
        }
        if self.reads > 300.0 {
            self.reads = 0.0;
            self.cost_ms = 0.0;
        }
        self.matches = reference.iter().zip(pixels).filter(|(a, b)| a == b).count() as u32;
    }

    /// The scale 0x460684 hands 0x451710: `+0x190 · [0x5B26A4]`.
    pub(crate) fn scale(&self) -> f32 {
        self.matches as f32 * 0.0625
    }
}

impl App {
    /// A race's lens-flare object (0x4601F0 → 0x460780): the state at its constructor values,
    /// `LENSFLARE` as the launch's accessor dump left it (module doc) and `flarepos.dat`.
    pub(crate) fn lens_reset(&mut self) {
        let enabled = self.choice_index("Graphics.LensFlare").is_some_and(|(i, _)| i != 0);
        let mut positions = DEFAULT_POSITIONS;
        if let Ok(d) = std::fs::read(self.paths.render_dir().join("flarepos.dat")) {
            positions = [0.0; 14];
            for (p, c) in positions.iter_mut().zip(d.chunks_exact(4)) {
                *p = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            }
        }
        self.lens = LensState { enabled, positions, ..Default::default() };
    }

    /// The read-backs that finished: the stored block (0x460290) or a test (0x460460).
    fn lens_collect(&mut self) {
        let Some(r) = self.renderer.as_mut() else { return };
        for read in r.lens_results() {
            let l = &mut self.lens;
            if read.reference {
                l.reference_asked = None;
                if l.mode == LensMode::Waiting {
                    l.reference = Some(read.pixels);
                    l.mode = LensMode::Active;
                    l.last_test = Instant::now();
                    tracing::debug!(texels = ?read.pixels, "lens flare: the sun's block stored");
                }
            } else {
                l.test_asked = None;
                if l.mode == LensMode::Active {
                    l.apply_test(read.cost_ms, &read.pixels);
                    tracing::debug!(matches = l.matches, mode = ?l.mode, "lens flare: test");
                }
            }
        }
    }

    /// 0x460460 and the sun's block capture for this frame: collect what finished, ask for the
    /// frame's read-backs ([`nfs_renderer::Scene::lens_probe`]) and lay the elements out as
    /// overlay items. Called last in the race scene's build, so the ranges the mirror splices by
    /// are not moved.
    pub(crate) fn lens_flare_items(&mut self) {
        if !self.lens.enabled || self.vehicle.is_none() || self.sky.is_none() {
            return;
        }
        self.lens_collect();
        let Some(bin) = self.track.sky_bin.as_ref().filter(|b| b.sun_active()) else { return };
        let readback = bin.sun_readback;
        let Some(r) = self.renderer.as_ref() else { return };
        let (aspect, (w, h)) = (r.aspect(), r.size());
        let cam = self.camera;
        // `+0x18D`: the sky pass's projection of the sun on the window
        let pt = self.sun_point(&cam, aspect, SUN_VIEW_MAIN);
        let Some(pt) = pt else { return };
        let (x, y) = ((pt.px / 640.0 * w as f32).round_ties_even() as i32 - 2, (pt.py / 480.0 * h as f32).round_ties_even() as i32 - 2);
        let mut probe = nfs_renderer::LensProbe { x, y, reference: false, test: false };
        let now = Instant::now();
        // 0x460290: the block right after the sun is drawn, once
        if self.lens.mode == LensMode::Waiting && !self.lens.reference_asked.is_some_and(|t| now.duration_since(t) < ASK_LAPSES) {
            probe.reference = true;
            self.lens.reference_asked = Some(now);
        }
        if self.lens.mode != LensMode::Active || !readback {
            self.scene.lens_probe = (probe.reference).then_some(probe);
            return;
        }
        // 0x4604C4…0x4604D9: every 200 ms, `+0x194 + 200 < now`
        if now.duration_since(self.lens.last_test) > TEST_EVERY && !self.lens.test_asked.is_some_and(|t| now.duration_since(t) < ASK_LAPSES) {
            self.lens.last_test = now;
            self.lens.test_asked = Some(now);
            probe.test = true;
        }
        self.scene.lens_probe = (probe.reference || probe.test).then_some(probe);
        // 0x460646…0x460765: the elements
        let scale = self.lens.scale();
        let (fwd, right, up) = self.flare_basis(&cam);
        let tan_y = (cam.fov_y_radians * 0.5).tan();
        let tan_x = tan_y * aspect;
        let ahead = if cam.near < 1.0 { 1.0 } else { cam.near * 2.0 };
        let mut items = Vec::new();
        for (e, id) in ELEMENTS.iter().enumerate() {
            // 0x452260, 0x452100: a record with no texture at all is dropped
            let Some(def) = self.flare_defs.iter().find(|d| d.id == *id && d.textures.iter().any(|t| t.is_some())) else { continue };
            if !def.lit(0, self.tick as u32) {
                continue;
            }
            // `centre + (sun − centre) · t`, in the window's pixels (0x4606FD…0x460736)
            let t = self.lens.positions[e + 1];
            let (ex, ey) = (320.0 + (pt.px - 320.0) * t, 240.0 + (pt.py - 240.0) * t);
            let (sx, sy) = (ex / 640.0, ey / 480.0);
            let dir = fwd + right * ((sx * 2.0 - 1.0) * tan_x) + up * ((1.0 - sy * 2.0) * tan_y);
            let centre = cam.position + dir * ahead;
            self.flare_quads(&mut items, &cam, aspect, SUN_VIEW_MAIN, def, (ex, ey), centre, ahead, scale, &self.lens_sprites, true);
        }
        self.scene.items.extend(items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(reference: [u32; 16]) -> LensState {
        LensState { mode: LensMode::Active, reference: Some(reference), ..Default::default() }
    }

    #[test]
    fn counts_equal_texels() {
        let mut l = active([7; 16]);
        let mut px = [7u32; 16];
        px[3] = 1;
        px[9] = 2;
        l.apply_test(0, &px);
        assert_eq!(l.matches, 14);
        assert!((l.scale() - 0.875).abs() < 1e-6);
        l.apply_test(0, &[0; 16]);
        assert_eq!(l.matches, 0);
    }

    /// 0x46055B…0x4605A0: past 10 reads and under 300 an average above 5 ms switches it off for good.
    #[test]
    fn slow_reads_switch_it_off() {
        let mut l = active([0; 16]);
        for _ in 0..10 {
            l.apply_test(6, &[0; 16]);
            assert_eq!(l.mode, LensMode::Active, "the first ten reads are not judged");
        }
        l.apply_test(6, &[0; 16]);
        assert_eq!(l.mode, LensMode::Off);
        assert_eq!(l.matches, 0);
    }

    #[test]
    fn fast_reads_restart_after_three_hundred() {
        let mut l = active([0; 16]);
        for _ in 0..300 {
            l.apply_test(5, &[0; 16]);
        }
        assert_eq!(l.mode, LensMode::Active, "an average of exactly 5 ms passes");
        l.apply_test(5, &[0; 16]);
        assert_eq!(l.mode, LensMode::Active);
        assert_eq!((l.reads, l.cost_ms), (0.0, 0.0), "the 301st read restarts the window");
    }

    #[test]
    fn element_names() {
        assert_eq!(ELEMENTS[0], u32::from_le_bytes(*b"bnus"));
        assert_eq!(ELEMENTS[12], u32::from_le_bytes(*b"nnus"));
    }
}
