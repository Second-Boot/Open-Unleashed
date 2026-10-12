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

impl App {
    /// The optional **frame cap**: `NFS_FPS_CAP=<n>` holds the redraw loop to at most `n` frames a
    /// second by sleeping out the rest of the frame (default off: the loop runs at the display's
    /// pace, as before). Called at the top of every redraw arm, before the frame's `dt` is read, so
    /// the sleep is part of that frame's time.
    // PORT-DEVIATION D4 (RE/notes/PORT-DEVIATIONS.md): NFS_FPS_CAP, opt-in and off by default; the original has no frame cap.
    pub(crate) fn pace_frame(&self) {
        let Some(cap) = nfs_game::config::cfg().nfs_fps_cap.as_str().and_then(|v| v.trim().parse::<f64>().ok()).filter(|c| *c >= 1.0) else { return };
        let want = Duration::from_secs_f64(1.0 / cap);
        let spent = self.last_frame.elapsed();
        if spent < want {
            std::thread::sleep(want - spent);
        }
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
        let Scene { items: _, sprites, clear_color, sun_direction, ambient, mirror, headlight, headlight_decal, beacon, fog, sky_fog, lens_probe: _ } = &self.scene;
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
        let Some(r) = self.renderer.as_mut() else { return };
        let img = r.render_to_image(&self.scene, &self.camera, w, h).expect("verify render");
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
        let want = Duration::from_millis(8);
        let spent = self.last_frame.elapsed();
        if spent < want {
            std::thread::sleep(want - spent);
        }
    }
}
