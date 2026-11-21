//! The enhanced game's switches in the application (not the original):
//! pushing them to the renderer, the «Улучшения» sub-tab's accessors and its «как в оригинале» /
//! «по умолчанию» / page buttons, and the debug A/B key. `nfs_game::enhanced` is the storage,
//! `nfs_renderer::Enhancements` the set. `RE/notes/ENHANCEMENTS.md`.

use crate::*;
use nfs_game::enhanced::{self, Mode};
use nfs_renderer::{ENHANCEMENT_REGISTRY, EnhancementSwitch, Enhancements};

impl App {
    /// ENHANCEMENT E37 / E34 (temporal levels): tell the renderer about a camera cut it cannot see in the camera's numbers (it drops FSR's history
    /// and the vectors' previous frame): the view changed (chase / bumper / driver / replay cameras: `camera_view`), photo mode went
    /// in or out, the pause began or ended, or the tick went backwards / jumped (a new race, a replay seek). A big jump of
    /// the camera itself and a frame without the 3D scene are the renderer's own checks. Does nothing with both off.
    pub(crate) fn temporal_cut_check(&mut self) {
        // ENHANCEMENT ENH-UPSCALER: the effective set (only the chosen backend's level counts)
        if !self.enh.temporal_upscaler() {
            self.temporal_sig = None;
            return;
        }
        let sig = (self.camera_view(), self.photo.active, self.paused, self.tick);
        if let Some(prev) = self.temporal_sig
            && (prev.0 != sig.0 || prev.1 != sig.1 || prev.2 != sig.2 || sig.3 < prev.3 || sig.3 > prev.3 + 16)
            && let Some(r) = self.renderer.as_mut()
        {
            r.temporal_cut();
        }
        self.temporal_sig = Some(sig);
    }

    /// The registry index of an `Enh.<token>` accessor.
    fn enh_index(name: &str) -> Option<usize> {
        let tok = name.strip_prefix(enhanced::ACCESSOR_PREFIX)?;
        ENHANCEMENT_REGISTRY.iter().position(|e| e.token == tok)
    }

    /// An `Enh.*` accessor's combo state: the level is the row index (0 = off), the rows are the
    /// entry's level captions (`enhanced::level_ids`).
    pub(crate) fn enh_choice(&self, name: &str) -> Option<(usize, &'static [u32])> {
        // ENHANCEMENT ENH-UPSCALER: the settings rows that edit a part of E34's / E37's stored level
        // ENH-VIEW: in the original-look view the rows show (and edit) the player's own levels, not the all-off set on the screen
        let p = self.enh_player();
        if let Some(v) = enhanced::VRow::from_accessor(name) {
            let ids = enhanced::vrow_ids(v);
            return Some(match v {
                enhanced::VRow::View => (usize::from(!self.enh.any_on()), ids), // what the screen draws
                enhanced::VRow::MfxMode => (matches!(nfs_renderer::mfx_mode(p.level(nfs_renderer::E34)), nfs_renderer::MfxMode::Simple) as usize, ids),
                // the simple mode has no 1:1: its list is the same captions from the second
                enhanced::VRow::MfxScale => {
                    let l = p.level(nfs_renderer::E34);
                    let simple = matches!(nfs_renderer::mfx_mode(l), nfs_renderer::MfxMode::Simple);
                    (nfs_renderer::mfx_scale_index(l), if simple { &ids[1..] } else { ids })
                }
                enhanced::VRow::FsrScale => (nfs_renderer::fsr_scale_index(p.level(nfs_renderer::E37)), ids),
            });
        }
        let i = Self::enh_index(name)?;
        let ids = enhanced::level_ids(i);
        // ENHANCEMENT ENH-UPSCALER: the upscaler row lists the backends this platform has
        if i == nfs_renderer::E38.0 as usize {
            return Some((nfs_renderer::upscaler_choices().iter().position(|&c| c == p.level(nfs_renderer::E38)).unwrap_or(0), ids));
        }
        // ENHANCEMENT E33: the row is on / off where the display does EDR, otherwise it only says «недоступно»
        if i == nfs_renderer::E33.0 as usize {
            return Some(if self.enh_edr_available() { ((p.level(nfs_renderer::E33) as usize).min(1), &ids[..2]) } else { (0, &ids[2..3]) });
        }
        Some((p.shown_level(EnhancementSwitch(i as u8)) as usize, ids))
    }

    /// ENH-VIEW: the player's own set — the one the original-look view keeps aside, else the one on the screen.
    pub(crate) fn enh_player(&self) -> Enhancements {
        self.enh_stash.unwrap_or(self.enh)
    }

    /// ENHANCEMENT E33: the display does extended-range output.
    fn enh_edr_available(&self) -> bool {
        self.renderer.as_ref().is_some_and(|r| r.edr_available())
    }

    /// A step of an `Enh.*` combo: the level moves by `delta`, wrapping over the entry's levels
    /// like every combo.
    pub(crate) fn enh_step(&mut self, name: &str, delta: i32) -> bool {
        // ENHANCEMENT ENH-UPSCALER: a settings row edits its part of the stored level
        let p = self.enh_player();
        if let Some(v) = enhanced::VRow::from_accessor(name) {
            if v == enhanced::VRow::View {
                self.enh_toggle_all();
                return true;
            }
            let mut e = p;
            let (e34, e37) = (e.level(nfs_renderer::E34), e.level(nfs_renderer::E37));
            let mode = nfs_renderer::mfx_mode(e34);
            match v {
                enhanced::VRow::MfxMode => {
                    let to = if (matches!(mode, nfs_renderer::MfxMode::Simple) as i32 + delta).rem_euclid(2) == 1 { nfs_renderer::MfxMode::Simple } else { nfs_renderer::MfxMode::Temporal };
                    e.set_level(nfs_renderer::E34, nfs_renderer::mfx_switch_mode(e34, to));
                }
                enhanced::VRow::MfxScale => {
                    let n = nfs_renderer::mfx_scale_count(mode) as i32;
                    e.set_level(nfs_renderer::E34, nfs_renderer::mfx_level(mode, (nfs_renderer::mfx_scale_index(e34) as i32 + delta).rem_euclid(n) as usize));
                }
                enhanced::VRow::FsrScale => e.set_level(nfs_renderer::E37, (nfs_renderer::fsr_scale_index(e37) as i32 + delta).rem_euclid(4) as u8 + 1),
                enhanced::VRow::View => {}
            }
            self.enh_apply(e);
            return true;
        }
        let Some(i) = Self::enh_index(name) else { return false };
        // ENHANCEMENT ENH-UPSCALER: an inactive row (another setting overrides / disables it, `Enhancements::blocked`) does not change
        if p.blocked(ENHANCEMENT_REGISTRY[i].token).is_some() {
            return true;
        }
        // ENHANCEMENT ENH-UPSCALER: the choice of the backend; a backend chosen for the first time takes its default scale
        if i == nfs_renderer::E38.0 as usize {
            let choices = nfs_renderer::upscaler_choices();
            let cur = choices.iter().position(|&c| c == p.level(nfs_renderer::E38)).unwrap_or(0) as i32;
            let to = choices[(cur + delta).rem_euclid(choices.len() as i32) as usize];
            let mut e = p;
            e.set_level(nfs_renderer::E38, to);
            if to == nfs_renderer::UP_METALFX && e.level(nfs_renderer::E34) == 0 {
                e.set_level(nfs_renderer::E34, 2);
            }
            if to == nfs_renderer::UP_FSR && e.level(nfs_renderer::E37) == 0 {
                e.set_level(nfs_renderer::E37, 2);
            }
            self.enh_apply(e);
            return true;
        }
        let s = EnhancementSwitch(i as u8);
        let mut n = ENHANCEMENT_REGISTRY[i].levels.len() as i32;
        // ENHANCEMENT E33: two choices (the third caption is the «недоступно» row), none on a display without EDR
        if i == nfs_renderer::E33.0 as usize {
            if !self.enh_edr_available() {
                return true;
            }
            n = 2;
        }
        let mut e = p;
        e.set_level(s, (p.level(s) as i32 + delta).rem_euclid(n) as u8);
        self.enh_apply(e);
        true
    }

    /// Take a new set the player chose (the panel's rows and buttons): the renderer gets it through its command
    /// list (so the next frame, with the render thread too), and it is saved — unless this run is the hard
    /// original (`NFS_ENH=off`), which keeps no state and ignores every change. A change made while the A/B view (the `0` key) is
    /// showing another set commits what the panel shows and ends the A/B.
    pub(crate) fn enh_apply(&mut self, e: Enhancements) {
        if self.enh_mode == Mode::HardOriginal {
            return;
        }
        self.enh_stash = None;
        self.enh_set(e, true);
    }

    /// Show a set; `save` stores it (a player's choice) or not (the A/B view).
    pub(crate) fn enh_set(&mut self, e: Enhancements, save: bool) {
        if e == self.enh {
            return;
        }
        self.enh = e;
        // ENHANCEMENT ENH-LIVETEX: the twins the new levels need are checked from the next frame on
        self.enh_live_dirty = true;
        // ENHANCEMENT ENH-UPSCALER: the panel is generated from the set (settings rows of the chosen upscaler, inactive rows); when
        // its lines changed it is rebuilt at the start of the next frame (`enh_panel_refresh`), not inside the click that did it
        if enhanced::set_panel_state(self.enh_stash.unwrap_or(e)) {
            self.enh_panel_dirty = true;
        }
        if let Some(r) = self.renderer.as_mut() {
            r.set_enhancements(e);
        }
        if save {
            enhanced::save(&e, false);
            crate::guard::clear_safe();
        }
        tracing::info!(enhancements = %e.describe(), saved = save, "enhancements changed");
        self.bind_lists();
    }

    /// ENHANCEMENT ENH-LIVETEX: make the loaded track, sky and cars carry what E41 (HD textures) and E42 (rounded parts) are at NOW. The
    /// originals are always resident; the HD textures and refined meshes sit next to them as twins and the renderer shows them while the
    /// switch is on, so a flip (the `0` key, a level to / from 0) needs nothing here. Only a level whose twins are not resident yet (first use after
    /// a load made with the switch off, x2 <-> x4, soft <-> strong) is loaded now: the track and the sky at once, the cars one model per
    /// step until `budget_ms` is used (`None`: all). With both switches at 0, or after the work is done, this is a flag test.
    pub(crate) fn enh_live_step(&mut self, budget_ms: Option<u64>) {
        if !self.enh_live_dirty {
            return;
        }
        let Some(mut r) = self.renderer.take() else { return };
        let started = std::time::Instant::now();
        if let Some(w) = self.world.as_mut()
            && w.hd_pending(&r, &self.track)
        {
            // the track's CPU pixels were dropped after the upload (D7): read them again for the twins, then drop them
            if self.track.ensure_textures().is_ok() {
                w.sync_hd(&mut r, &self.track);
                self.track.release_textures();
            }
        }
        if let Some(s) = self.sky.as_mut() {
            s.sync_hd(&mut r, &self.track);
        }
        let mut models: Vec<&mut nfs_game::world::CarModel> = Vec::new();
        models.extend(self.car_model.as_mut());
        models.extend(self.road.opponent_models.iter_mut());
        models.extend(self.road.traffic_models.iter_mut());
        models.extend(self.road.police_models.iter_mut());
        models.extend(self.fe.cars.iter_mut().map(|c| &mut c.4));
        models.extend(self.fe.dealer_prev.as_mut());
        let mut pending = false;
        for m in models {
            if !m.live_pending(&r) {
                continue;
            }
            if budget_ms.is_some_and(|b| started.elapsed().as_millis() as u64 >= b) {
                pending = true;
                break;
            }
            m.sync_live(&mut r, &self.paths, &mut self.enh_live_cache);
        }
        if !pending {
            self.enh_live_dirty = false;
            self.enh_live_cache = Default::default();
            // ENHANCEMENT ENH-LIVELEAK: every model has let go of the twins the pass replaced; their slots can be used again
            r.recycle_released();
            // ENHANCEMENT ENH-LIVELEAK: what is resident after a pass, to see that a level change leaves nothing behind
            let st = r.resource_stats();
            tracing::info!(ms = started.elapsed().as_millis() as u64, textures = st.textures, textures_live = st.textures_live, texture_kib = st.texture_bytes >> 10, meshes = st.meshes, meshes_live = st.meshes_live, mesh_kib = st.mesh_bytes >> 10, arena_kib = st.arena_bytes >> 10, materials = st.materials, "live twins resident (ENHANCEMENT ENH-LIVELEAK)");
        }
        self.renderer = Some(r);
    }

    /// ENHANCEMENT ENH-UPSCALER: rebuild the «Улучшения» panel after a change of the set that altered its lines (the upscaler's rows
    /// appeared / went, a row became inactive or active again): the current page is loaded again, clamped to the pages that exist now
    /// (paging keeps working: the page button's callback follows the same page names). Nothing when the panel is not on screen.
    pub(crate) fn enh_panel_refresh(&mut self) {
        if !std::mem::take(&mut self.enh_panel_dirty) || !enhanced::enabled() {
            return;
        }
        let file = enhanced::page_name(enhanced::current_page());
        // a combo's list that was open belongs to the old widgets
        let rebuilt = match (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
            (Some(l), Some(r)) => l.swap_panel(r, &self.paths, enhanced::PANEL, &file),
            _ => false,
        };
        if rebuilt {
            self.fe.layout_open_combo = None;
            if let Some(l) = self.fe.layout_preview.as_mut() {
                l.set_open_combo(None);
            }
            self.bind_screen_widgets();
        }
    }

    /// ENHANCEMENT E5: the draw-distance factor of the main view — 1.0 (the original) at level 0,
    /// then +25 % / +50 % / +100 %. The view's walk, far plane and scene fog take it; the mirror,
    /// the simulation's own `draw_distance` readers (track particles) and the front end do not.
    pub(crate) fn enh_reach(&self) -> f32 {
        nfs_renderer::draw_reach(self.enh.level(nfs_renderer::E5))
    }

    /// ENHANCEMENT E4: degrees added to the original's horizontal view angle (0 at level 0);
    /// the half-angle gets half of it.
    pub(crate) fn enh_fov_bonus(&self) -> f32 {
        nfs_renderer::fov_bonus_deg(self.enh.level(nfs_renderer::E4))
    }

    /// The A/B key (and the panel's «графика» row): the player's set <-> every switch off (`enhanced::ab_toggle`). A view: the
    /// player's levels are never changed by it, so they survive any number of presses (ENH-SAVE); ENH-VIEW: whether the view is on is
    /// stored beside them, so the next start keeps it (not for an `NFS_ENH` set, nor in safe mode, whose set is not the player's).
    pub(crate) fn enh_toggle_all(&mut self) {
        if self.enh_mode == Mode::HardOriginal {
            return;
        }
        let (show, stash) = enhanced::ab_toggle(self.enh, self.enh_stash);
        self.enh_stash = stash;
        // PORT-DEVIATION D63: the message says what the press left on the screen
        self.panel_toast(show.any_on(), false);
        self.enh_set(show, false);
        if !enhanced::env_chosen() && !crate::guard::safe() {
            enhanced::save(&self.enh_player(), self.enh_stash.is_some() && !self.enh.any_on());
        }
    }

    /// A release on a button of the «Улучшения» panel (`callback_at`, the panel is not in `controls()`):
    /// «по умолчанию», «как в оригинале», or the page button `ENHPAGE<n>` (1-based), which makes the
    /// panel's content page `n` (`LayoutScreen::swap_panel`).
    pub(crate) fn enh_panel_callback(&mut self, cb: &str) {
        if cb == enhanced::CB_ORIGINAL {
            self.enh_all_original();
        } else if cb == enhanced::CB_DEFAULTS {
            self.enh_all_defaults();
        } else if cb == enhanced::CB_LITE {
            self.enh_all_lite();
        } else if let Some(page) = cb.strip_prefix(enhanced::CB_PAGE).and_then(|n| n.parse::<usize>().ok()) {
            let file = enhanced::page_name(page.saturating_sub(1));
            if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
                l.swap_panel(r, &self.paths, enhanced::PANEL, &file);
            }
            self.bind_screen_widgets();
        }
    }

    /// The menu's «как в оригинале».
    pub(crate) fn enh_all_original(&mut self) {
        self.enh_apply(Enhancements::ORIGINAL);
    }

    /// ENH-LITE: the menu's «лёгкий» — the light preset (`Enhancements::lite`).
    pub(crate) fn enh_all_lite(&mut self) {
        self.enh_apply(Enhancements::lite());
    }

    /// The menu's «по умолчанию»: every entry at its default level.
    pub(crate) fn enh_all_defaults(&mut self) {
        self.enh_apply(Enhancements::all());
    }
}
