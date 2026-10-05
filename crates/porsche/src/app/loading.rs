//! The loading screens: the front end's and the race's.

use crate::*;

impl App {
    /// The sprites of 0x4DDA00(`step`)'s frame (`nfs_game::layout::loading`), the packs loaded
    /// on first use with `LANGUAGE` (`[0x657C84]`, `App::language`) choosing the caption.
    pub(crate) fn fe_loading_sprites(&mut self, step: u32) -> Vec<Sprite> {
        let Some(r) = self.renderer.as_mut() else { return Vec::new() };
        if self.fe.loading.is_none() {
            let language = u32::from(self.language);
            self.fe.loading = Some(nfs_game::layout::loading::LoadingScreen::load(r, &self.paths, language));
        }
        let aspect = r.aspect();
        self.fe.loading.as_ref().map(|l| l.sprites(aspect, step)).unwrap_or_default()
    }

    /// **0x4DDA00(`step`)** — one whole loading-screen frame, drawn and presented at once, as
    /// 0x4D3420 does between the stages of its load (`controls.md` §4.165.3). It first stops the
    /// music (`MusicStop(1)`, 0x4DDA16). The port's own front-end load has no stages in 0x4D3420's
    /// order, so only the first (0) and last (9) frames are drawn — APPROXIMATION: steps 1…8,
    /// which grow the bar 13.78 pixels each, are not. As in the original the screen stays up
    /// only while the load runs; nothing holds it longer.
    /// What 0x4A4A70 reads: `Game.GameType` (the launch's), the career era `0x5032D0`, the
    /// track, `MIRROR` / `REVERSE`, a network session (APPROXIMATION of the four session tests,
    /// as §4.163's) and `LANGUAGE`.
    fn race_loading_inputs(&self) -> nfs_game::layout::loading::RaceLoadingInputs {
        use nfs_formats::career::Era;
        nfs_game::layout::loading::RaceLoadingInputs {
            game_type: self.fe.race_game_type.unwrap_or(self.fe.game_type),
            era: self.career.as_ref().map_or(0, |c| match c.career.era {
                Era::Classic => 1,
                Era::Golden => 2,
                Era::Modern => 3,
            }),
            track: self.track.name.clone(),
            mirror: self.mirror,
            reverse: self.backwards,
            network: self.net.session.is_some(),
            language: u32::from(self.language),
            rows: Vec::new(),
        }
    }

    /// One call of **0x4A4A70(`step`)** into the surface `RaceLoading` keeps.
    pub(crate) fn race_loading_step(&mut self, r: &mut Renderer, step: i32) -> bool {
        let mut input = self.race_loading_inputs();
        // 0x4A4AD4…0x4A4B4F: the progress record, while the screen is up (`[0x655A28]`, set by step
        // 0 and cleared by step 10), and 0x4A5149…0x4A5353: every player's row, on every call
        let active = step == 0 || (step != 10 && self.fe.race_loading.as_ref().is_some_and(|l| l.active));
        if active && let Some(n) = self.net.session.as_mut().filter(|n| n.in_race != 0) {
            n.send_loading_step(step);
            input.rows = n.loading_rows(r);
        }
        let rl = self.fe.race_loading.get_or_insert_with(|| nfs_game::layout::loading::RaceLoading::new(r));
        rl.step(r, &self.paths, step, &input)
    }

    /// **0x4A4A70(`step`)**, drawn and presented at once — the loading screen between the front
    /// end and the race (`controls.md` §4.167). The port's race load is `start_race`, whose
    /// stages are not 0x4B67B0's, so each step is drawn where the port's work of the original's
    /// step has finished, in the original's order (NM, 2026-09-30): 0 as it starts (main's 0x4B6CCA),
    /// 1 and 2 after the picture post (0x427A60), 3 after `TrackWorld::upload` (0x469065, the track
    /// data), 4 and 5 after the sky (the end of 0x468FB0), 6 after the camera data (0x471B70, moved up
    /// for the order), 7 after the collision sounds (0x4A8CD0), 8 before and 9 after the cars
    /// (0x414E63, 0x414E74), 10 after the barrier (0x4B68CB). APPROXIMATION: the *work* between two
    /// steps is the port's, not 0x4B67B0's — the bar moves when the port's equivalent finishes.
    pub(crate) fn race_loading_frame(&mut self, r: &mut Renderer, step: i32) {
        tracing::debug!(target: "loading", step, "0x4A4A70");
        if !self.race_loading_step(r, step) {
            return;
        }
        let (lw, lh) = self.hud_logical_on(r);
        let sprites = self.fe.race_loading.as_ref().map(|l| l.sprites(lw as f32, lh as f32)).unwrap_or_default();
        let scene = Scene { sprites, clear_color: [0.0, 0.0, 0.0, 1.0], ..Scene::default() };
        if let Err(e) = r.render(&scene, &self.camera) {
            tracing::error!(error = %e, "race loading screen render failed");
        }
        // **TEST GLUE** (not in the original): `NFS_LOADING_SHOT=<path>` writes the loading screen of
        // step 9 as a still after `NFS_LOADING_WAIT` ms (default 3000) of draining the session — the
        // other machine's `0x256` records and its picture arrive meanwhile — redrawn with step −1
        // (0x4A4A70 with −1 draws the rows again and leaves the bar as it was)
        if step == 9
            && let Ok(path) = nfs_game::config::cfg().nfs_loading_shot.var()
        {
            let wait = nfs_game::config::cfg().nfs_loading_wait.var().ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(3000);
            let until = Instant::now() + Duration::from_millis(wait);
            while Instant::now() < until {
                if let Some(n) = self.net.session.as_mut() {
                    let _ = n.poll_lobby();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            self.race_loading_step(r, -1);
            let sprites = self.fe.race_loading.as_ref().map(|l| l.sprites(lw as f32, lh as f32)).unwrap_or_default();
            let scene = Scene { sprites, clear_color: [0.0, 0.0, 0.0, 1.0], ..Scene::default() };
            match r.render_to_image(&scene, &self.camera, shot_geometry().0, shot_geometry().1) {
                Ok(rgba) => {
                    let _ = image::save_buffer(&path, &rgba, shot_geometry().0, shot_geometry().1, image::ColorType::Rgba8);
                    tracing::info!(path, "loading screen still written (NFS_LOADING_SHOT)");
                }
                Err(e) => tracing::error!(error = %e, "loading still failed"),
            }
        }
    }

    pub(crate) fn fe_loading_frame(&mut self, step: u32) {
        // 0x4DDA00: `MusicStop(1)` (0x4DDA14) — a fade of one step, then the update stops it
        self.fe_music(|m, a| m.stop_fade(a, 1));
        let sprites = self.fe_loading_sprites(step);
        let scene = Scene { sprites, clear_color: [0.0, 0.0, 0.0, 1.0], ..Scene::default() };
        if let Some(r) = self.renderer.as_mut()
            && let Err(e) = r.render(&scene, &self.camera)
        {
            tracing::error!(error = %e, "loading screen render failed");
        }
    }
}
