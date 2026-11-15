//! Frame pacing: the optional frame cap and the front end's idle redraw (performance work B,
//! 2026-10-01). Neither is a game rule — both only decide **when the port submits a frame**.

use crate::*;
use nfs_renderer::{FakeSortMode, Fog, Scene, Sprite};

/// What the window last showed, kept so an unchanged front-end frame can be recognised exactly.
pub(crate) struct Presented {
    sprites: Vec<Sprite>,
    clear_color: [f64; 4],
    sun_direction: Vec3,
    ambient: f32,
    fog: Option<Fog>,
    sky_fog: Option<Fog>,
    /// the render target `(physical W, physical H, scale)`, the mip promotion and the fake-sort
    /// mode — every renderer state the front end's frame depends on besides the scene
    target: (u32, u32, u32, bool, FakeSortMode),
}

/// Whether two sprites are the same draw, field by field, floats by their bits (so a NaN or a
/// `-0.0` counts as a change: the safe direction). Written as a full destructure so a field added
/// to [`Sprite`] stops compiling here instead of being ignored.
fn same_sprite(a: &Sprite, b: &Sprite) -> bool {
    let Sprite { texture, pos, size, uv_min, uv_max, color, rotation, pivot, tri, corner_colors, depth } = a;
    let bits = |x: &[f32], y: &[f32]| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits());
    *texture == b.texture
        && bits(pos, &b.pos)
        && bits(size, &b.size)
        && bits(uv_min, &b.uv_min)
        && bits(uv_max, &b.uv_max)
        && bits(color, &b.color)
        && rotation.to_bits() == b.rotation.to_bits()
        && bits(pivot, &b.pivot)
        && match (tri, &b.tri) {
            (None, None) => true,
            (Some((p, c)), Some((q, d))) => p.iter().flatten().zip(q.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()) && c.iter().flatten().zip(d.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()),
            _ => false,
        }
        && match (corner_colors, &b.corner_colors) {
            (None, None) => true,
            (Some(p), Some(q)) => p.iter().flatten().zip(q.iter().flatten()).all(|(x, y)| x.to_bits() == y.to_bits()),
            _ => false,
        }
        && *depth == b.depth
}

/// ENHANCEMENT E22 «auto»: the cap on the battery (frames a second).
const BATTERY_FPS: f64 = 60.0;
/// ENHANCEMENT E6 «immediate»: the front end's cap without a vsync (frames a second).
const IMMEDIATE_FE_FPS: f64 = 120.0;

impl App {
    /// The frame cap in frames a second, `None` = uncapped. In priority order: `NFS_FPS_CAP=<n>`
    /// (the port's own switch, D4: tests rely on it); ENHANCEMENT E22 (the menu row «ограничение
    /// FPS»: 30 / 60 / 120 / 240 or the display's refresh rate); ENHANCEMENT E6 level 2
    /// (immediate): the front end, which would otherwise spin a core without a vsync, at 120.
    fn fps_cap(&self) -> Option<f64> {
        if let Some(c) = nfs_game::config::cfg().nfs_fps_cap.as_str().and_then(|v| v.trim().parse::<f64>().ok()).filter(|c| *c >= 1.0) {
            return Some(c);
        }
        // ENHANCEMENT E22: the menu's cap. "Display" with the display's own sync on (E6 off) is the
        // sync itself: capping at exactly the refresh rate would only add a beat against it.
        use nfs_renderer::Enhancements as Lvl;
        let own_sync = self.enh.level(nfs_renderer::E6) != 0;
        match self.enh.level(nfs_renderer::E22) {
            l @ 1..=4 => return Some(Lvl::E22_FPS[l as usize - 1]),
            Lvl::E22_DISPLAY if own_sync => return Some(self.display_refresh_hz()),
            // ENHANCEMENT E22 «auto»: on the battery 60 frames a second (the GPU is what heats the machine), on mains
            // power as «display»
            Lvl::E22_AUTO if self.on_battery() => return Some(BATTERY_FPS),
            Lvl::E22_AUTO if own_sync => return Some(self.display_refresh_hz()),
            _ => {}
        }
        // ENHANCEMENT E6 (immediate): without a vsync the present does not pace the loop, so the
        // front end (a 640x480 menu) would spin a core at full speed on every animated screen;
        // hold it to 120 frames a second. The race stays uncapped (that is the point of the level).
        (self.enh.level(nfs_renderer::E6) == Lvl::E6_IMMEDIATE && self.race.is_none() && self.vehicle.is_none()).then_some(IMMEDIATE_FE_FPS)
    }

    /// ENHANCEMENT E22 «auto»: whether the machine runs on its battery, asked of the OS every five seconds (a
    /// platform that cannot say counts as mains power).
    fn on_battery(&self) -> bool {
        if let Some((at, b)) = self.pace_battery.get()
            && at.elapsed() < Duration::from_secs(5)
        {
            return b;
        }
        let b = crate::power::on_battery().unwrap_or(false);
        if self.pace_battery.get().is_none_or(|(_, old)| old != b) {
            tracing::info!(battery = b, "power source (E22 auto)");
        }
        self.pace_battery.set(Some((Instant::now(), b)));
        b
    }

    /// ENHANCEMENT E22: the refresh rate of the monitor the window is on (winit's
    /// `refresh_rate_millihertz`), 60 when the platform does not say. Re-read every two seconds so
    /// a window dragged to another display follows.
    fn display_refresh_hz(&self) -> f64 {
        if let Some((at, hz)) = self.pace_refresh.get()
            && at.elapsed() < Duration::from_secs(2)
        {
            return hz;
        }
        let hz = self
            .window
            .as_ref()
            .and_then(|w| w.current_monitor())
            .and_then(|m| m.refresh_rate_millihertz())
            .map(|mhz| mhz as f64 / 1000.0)
            .filter(|hz| *hz >= 1.0)
            .unwrap_or(60.0);
        if self.pace_refresh.get().is_none_or(|(_, old)| old != hz) {
            tracing::info!(hz, "display refresh rate (E22)");
        }
        self.pace_refresh.set(Some((Instant::now(), hz)));
        hz
    }

    /// The optional **frame cap** (see [`fps_cap`](Self::fps_cap)): holds the redraw loop to at most
    /// the cap's frames a second (default off: the loop runs at the display's pace, as before).
    /// Called at the top of every redraw arm, before the frame's `dt` is read, so the wait is part
    /// of that frame's time. A **deadline** scheduler: each frame's start is the previous deadline
    /// plus one period (so a late wake-up does not stretch the next frame), sleeping to ~1 ms
    /// before it and spinning the rest — the OS sleep alone overshoots by up to a millisecond or
    /// more. A frame that ran more than a period late restarts the schedule (no burst to catch up).
    // PORT-DEVIATION D4 (RE/notes/PORT-DEVIATIONS.md): NFS_FPS_CAP, opt-in and off by default; the original has no frame cap. ENHANCEMENT E22 is the menu row on the same pacer.
    pub(crate) fn pace_frame(&self) {
        // PORT-DEVIATION D45 (RE/notes/PORT-DEVIATIONS.md): a race whose window is hidden presents at most 30 fps;
        // with E22's cap the slower of the two
        let hidden = self.vehicle.is_some() && self.window_hidden();
        let Some(period) = frame_floor(self.fps_cap(), hidden) else {
            self.pace_next.set(None);
            return;
        };
        let now = Instant::now();
        let mut target = self.pace_next.get().map_or(now, |t| t + period);
        if now > target + period {
            target = now;
        }
        self.pace_next.set(Some(target));
        // the spin margin follows how far the OS sleep overshot lately (1 ms to start; 0.3..3 ms,
        // decaying 2 % a frame), so the spin is as short as the timer allows
        let spin = self.pace_spin.get();
        if let Some(sleep) = target.checked_duration_since(now).and_then(|d| d.checked_sub(spin)) {
            std::thread::sleep(sleep);
            let over = Instant::now().saturating_duration_since(target.checked_sub(spin).unwrap_or(now));
            self.pace_spin.set(spin.mul_f64(0.98).max(over.mul_f64(1.5)).clamp(Duration::from_micros(300), Duration::from_millis(3)));
        }
        while Instant::now() < target {
            std::hint::spin_loop();
        }
    }

    /// Whether the window is not on screen: winit's `Occluded(true)` (covered by other windows
    /// entirely, another Space, a hidden app) or minimised. A window that is only unfocused, or
    /// partly covered, is visible and not counted.
    pub(crate) fn window_hidden(&self) -> bool {
        self.occluded || self.window.as_ref().is_some_and(|w| w.is_minimized() == Some(true))
    }

    /// **The front end's idle redraw.** `true` when this frame would put exactly the pixels on the
    /// window that the last presented frame did, so the render and the present can be skipped.
    /// "Exactly" is by construction, not by a hash: the sprite list is compared field by field with
    /// the one last presented, and the frame is never idle when anything that can change the image
    /// without changing the sprites is live — a 3D room or car (`items`, the offscreen car views),
    /// a playing movie, the credits roll or an end-of-era show (textures rewritten in place), a new
    /// render target size, mip promotion or fake-sort mode. Movies, slides, glints, the pointer
    /// and a blinking caret all change the sprites or the textures, so they keep it redrawing.
    /// `NFS_FE_REDRAW_ALWAYS=1` turns it off.
    // PORT-DEVIATION D3 (RE/notes/PORT-DEVIATIONS.md): an unchanged FE frame is not redrawn; the original redraws every FE frame (main loop 0x4B6A50). Pixels are proven equal by NFS_FE_IDLE_VERIFY.
    pub(crate) fn fe_frame_unchanged(&mut self) -> bool {
        let eligible = self.vehicle.is_none()
            && self.race.is_none()
            && self.fe.menu.is_some()
            && (self.shot_target.is_none() || nfs_game::config::cfg().nfs_fe_idle_verify.is_set())
            && self.scene.items.is_empty()
            && self.fe.room.is_empty()
            && self.fe.cars.is_empty()
            && self.fe.loading_still.is_none()
            && self.fe.startup_still.is_none()
            && self.fe.race_loading_still.is_none()
            && !nfs_game::config::cfg().nfs_fe_redraw_always.is_set()
            && self.fe.layout_preview.as_ref().is_some_and(|l| !l.has_live_textures())
            && self.fe.layout_under.iter().all(|(_, u)| !u.has_live_textures())
            && self.fe.layout_bottom.as_ref().is_none_or(|b| !b.has_live_textures());
        if !eligible {
            self.presented = None;
            return false;
        }
        let (w, h, s) = self.hud_geometry();
        // the front end has no `SPECIALFX` level: no mip promotion, the fake-sort mode of level `None`
        let target = (w, h, s.to_bits(), false, self.fakesort_mode(None));
        let Scene { items: _, sprites, clear_color, sun_direction, ambient, mirror, headlight, headlight_decal, headlight_gain: _, beacon, fog, sky_fog, lens_probe: _, refl: _, shadows: _, spots: _, lamps: _, shafts: _, water: _, ao: _, eye: _, motion: _, rain: _, dof: _, haze: _, drops: _, contacts: _, sun_cars: _, cover: _, lbeam: _, near_sky: _ } = &self.scene;
        let plain = !mirror && headlight.is_none() && !headlight_decal && beacon.is_none();
        if plain
            && let Some(p) = &self.presented
            && p.target == target
            && p.clear_color.map(f64::to_bits) == clear_color.map(f64::to_bits)
            && p.sun_direction == *sun_direction
            && p.ambient.to_bits() == ambient.to_bits()
            && p.fog == *fog
            && p.sky_fog == *sky_fog
            && p.sprites.len() == sprites.len()
            && p.sprites.iter().zip(sprites).all(|(a, b)| same_sprite(a, b))
        {
            return true;
        }
        self.presented = plain.then(|| Presented { sprites: sprites.clone(), clear_color: *clear_color, sun_direction: *sun_direction, ambient: *ambient, fog: *fog, sky_fog: *sky_fog, target });
        false
    }

    /// **TEST GLUE** `NFS_FE_IDLE_VERIFY=1`: prove the idle decision on the real frames. Every eligible
    /// front-end frame is also rendered to an image; the image of the last frame that **was**
    /// rendered is kept, and a frame the idle test skipped must equal it byte for byte (it panics
    /// otherwise). `idle` is the decision [`fe_frame_unchanged`](Self::fe_frame_unchanged) just made.
    pub(crate) fn fe_idle_verify(&mut self, idle: bool) {
        if self.presented.is_none() {
            self.idle_check = None;
            return;
        }
        let (w, h, _) = self.hud_geometry();
        self.sync_gamma();
        let cam = self.render_camera();
        let Some(r) = self.renderer.as_mut() else { return };
        let img = r.render_to_image(&self.scene, &cam, w, h).expect("verify render");
        if idle {
            assert!(self.idle_check.as_deref() == Some(img.as_slice()), "front-end idle redraw skipped a frame that differs from the one shown");
            tracing::debug!("front-end idle frame verified identical");
        } else {
            self.idle_check = Some(img);
        }
    }

    /// An idle front-end frame did no GPU work, so nothing paces the loop the way the present's
    /// vsync does: sleep out the rest of an 8 ms frame (120 Hz — never slower than the display's
    /// own cadence on a 60 Hz one would have been).
    pub(crate) fn idle_wait(&self) {
        // a frame cap (D4 / E22) paces at the top of the next frame
        if self.fps_cap().is_some() {
            return;
        }
        let want = Duration::from_millis(8);
        let spent = self.last_frame.elapsed();
        if spent < want {
            std::thread::sleep(want - spent);
        }
    }
}

/// The frame rate a race presents at most while its window is hidden.
// PORT-DEVIATION D45 (RE/notes/PORT-DEVIATIONS.md): host-side timing only; the 64 Hz tick loop is not touched.
pub(crate) const HIDDEN_FPS: f64 = 30.0;

/// The shortest frame time the host holds the redraw loop to: the slower of `NFS_FPS_CAP` (D4) and,
/// while the window is hidden, [`HIDDEN_FPS`]; `None` = no cap. (`user_cap` is
/// `fps_cap`: `NFS_FPS_CAP`, else ENHANCEMENT E22's menu cap.)
pub(crate) fn frame_floor(user_cap: Option<f64>, hidden: bool) -> Option<Duration> {
    let rate = match (user_cap, hidden) {
        (Some(c), true) => Some(c.min(HIDDEN_FPS)),
        (Some(c), false) => Some(c),
        (None, true) => Some(HIDDEN_FPS),
        (None, false) => None,
    };
    rate.map(|r| Duration::from_secs_f64(1.0 / r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_cap_is_30_fps_and_lifts_when_visible() {
        assert_eq!(frame_floor(None, false), None);
        let h = frame_floor(None, true).unwrap();
        assert!((h.as_secs_f64() - 1.0 / 30.0).abs() < 1e-9);
        // the slower of the two caps wins
        assert_eq!(frame_floor(Some(60.0), true), frame_floor(None, true));
        assert_eq!(frame_floor(Some(20.0), true), Some(Duration::from_secs_f64(1.0 / 20.0)));
        assert_eq!(frame_floor(Some(60.0), false), Some(Duration::from_secs_f64(1.0 / 60.0)));
    }
}
