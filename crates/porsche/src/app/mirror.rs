//! The rear-view mirror pass.

use crate::*;

impl App {
    /// The slice the mirror's sky pass saves into `[sky+0x50]` (0x45D587 → 0x481400 from the
    /// mirror's eye, the search starting at the slice the main pass saved) — what its fog
    /// (0x45D2E0 / 0x45D3B0) and the next update read.
    fn mirror_slice(&self, eye: Vec3) -> i32 {
        let main = self.sky.as_ref().map_or(self.cam_slice.unwrap_or(0) as i32, |s| s.pass_slice());
        match self.slices.as_ref() {
            Some(t) if !t.slices.is_empty() => t.find_slice(eye, main.max(0) as usize) as i32,
            _ => main,
        }
    }

    /// The mirror pass's sky update (0x468AA1 → 0x468CA0 → 0x468AE0 → 0x45D7D0 → 0x45D030), after
    /// the main pass's in the same frame: 0 ticks, from the main pass's eye and slice, leaving the
    /// mirror's eye and slice (0x481400 from the main one's) for the next main update
    /// (`Sky::update`). It runs after the main view is drawn, as in the original, so the main view
    /// shows the sky as its own update left it; the port draws the mirror's texture before the main
    /// view, from the main update's state — which for the mirror, whose view shows the ring alone
    /// (0x45D470), is the same picture.
    pub(crate) fn mirror_sky_update(&mut self) {
        let Some(cam) = self.rear_mirror else { return };
        let slice = self.mirror_slice(cam.position);
        let (Some(s), Some(r)) = (self.sky.as_mut(), self.renderer.as_mut()) else { return };
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        s.update(r, cam.position, slice, lap, self.tick, &mut self.fe_rng);
    }

    /// The mirror's view (AUDIT §O), rendered before the frame into its own texture at the tray's
    /// 240 × 84 on the window's scale, cleared to the sky record's group B (0x459A70 under
    /// `[0x627F43] & 0x10`; 0xFF77BBFF, the view's constructor colour, only without a `.bin`).
    pub(crate) fn render_mirror(&mut self) {
        let Some(cam) = self.rear_mirror else { return };
        // the tray's 240 × 84 **pixels** (constant, the port's HiDPI mapping), times the HiDPI scale
        let k = self.hud_geometry().2;
        let (w, h) = ((240.0 * k).round() as u32, (84.0 * k).round() as u32);
        // the cars' and skid marks' entries for the mirror's own eye: the same draw 0x4430A0 in its
        // pass (`dyn_items`) — the camera's own car is not queued, the others are culled and picked
        // by the mirror's view, their shadows and skid marks pulled toward its eye with its far 65000
        let mirror_dyn: Vec<(u32, nfs_renderer::DrawItem)> = match self.dyn_pose {
            Some(pose) if self.vehicle.is_some() => {
                let forward = (cam.target - cam.position).normalize_or_zero();
                let right = forward.cross(cam.up).normalize_or_zero();
                let tan_v = (cam.fov_y_radians * 0.5).tan();
                let cull = nfs_game::world::CullView { eye: cam.position, forward, right, up: right.cross(forward).normalize_or_zero(), tan_h: tan_v * (240.0 / 84.0), tan_v, near: cam.near, draw: (self.draw_distance / 3.0).max(50.0) };
                let render_w = self.renderer.as_ref().map_or(640.0, |r| r.size().0.max(1) as f32);
                let p = DynPose { mirror_pass: true, cull: Some(cull), lod: (w.max(1) as f32, h.max(1) as f32), ref_width: render_w, ..pose };
                let (saved_cam, saved_far) = (std::mem::replace(&mut self.camera, cam), std::mem::replace(&mut self.view_far, 65000.0));
                let mut pools = self.pool_used;
                let items = self.dyn_items(&p, &mut pools);
                (self.camera, self.view_far) = (saved_cam, saved_far);
                items
            }
            _ => Vec::new(),
        };
        // the pass 0x468CA0 draws the scene through the generic 0x468AE0 with the view's own
        // walk 0x47FB40: the footprint and the distance rule out to `+0x124` = `max(D/3, 50)`
        // (0x468D3B…0x468D82), the eye and axes the rear build gives
        // and then, from that walk's own draws, the pass's flares (0x451940, out of the walk's
        // drawers), the kind-3 systems the walk refreshes (0x450D10 → 0x455150: `+8 = 100`,
        // `+0x10` the article's `d²` from the *mirror's* eye) and the particle draws 0x455260 /
        // 0x43F6E0 with the mirror's eye, axes and far 65000 (`Self::effect_items`)
        let mirror_fx = if self.world.is_some() {
            let forward = (cam.target - cam.position).normalize_or_zero();
            let right = forward.cross(cam.up).normalize_or_zero();
            let up = right.cross(forward).normalize_or_zero();
            let tan_v = (cam.fov_y_radians * 0.5).tan();
            let view = nfs_game::world::CullView { eye: cam.position, forward, right, up, tan_h: tan_v * (240.0 / 84.0), tan_v, near: cam.near, draw: (self.draw_distance / 3.0).max(50.0) };
            // the mirror's walk is 0x488CC0's next count (0x47FB49): the eighth re-registers
            self.walk_calls = self.walk_calls.wrapping_add(1);
            if self.walk_calls & 7 == 0
                && let Some(w) = self.world.as_mut()
            {
                w.reregister_articles();
            }
            let draws = self.world.as_ref().map(|w| w.article_draws(&view)).unwrap_or_default();
            if let (Some(w), Some(p)) = (self.world.as_ref(), self.track_particles.as_mut()) {
                p.draw(self.draw_distance, |e| w.effect_run(&draws, e.article, e.part, 3));
            }
            let (saved_cam, saved_far) = (std::mem::replace(&mut self.camera, cam), std::mem::replace(&mut self.view_far, 65000.0));
            let effects = self.effect_items(forward, right, up, &draws, false);
            (self.camera, self.view_far) = (saved_cam, saved_far);
            Some((view, draws, effects))
        } else {
            None
        };
        let scene = if let (Some(w), Some((_view, draws, effects))) = (&self.world, mirror_fx) {
            let (a, b) = self.track_item_range;
            // the same merge the main pass makes (`Self::rebuild_scene`, `RE/RENDERING.md` "The
            // view's lists"): the mirror's own near/far/rest list, freshly culled from its own
            // eye, with the cars' and the skid marks' `+0xC` entries of the mirror's own pass
            // (`mirror_dyn`, `Self::dyn_items`)
            let (mut near, far, rest) = w.track_draw_lists(&draws, cam.position);
            near.sort_by_key(|(k, _)| *k);
            let mut blended: Vec<(u32, nfs_renderer::DrawItem)> = far.into_iter().map(|(k, i)| (k, nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i })).collect();
            blended.extend(mirror_dyn);
            let merged: Vec<nfs_renderer::DrawItem> = near
                .into_iter()
                .map(|(_, i)| nfs_renderer::DrawItem { early: true, layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i })
                .chain(nfs_renderer::farthest_first(blended))
                .chain(rest.into_iter().map(|i| nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i }))
                .collect();
            let mut scene = self.scene.clone();
            // the main pass's flares / particles / streaks (after the track's range) give way to the
            // mirror's own first, so the track's range is still where the main pass left it
            let (ea, eb) = self.effect_item_range;
            scene.items.splice(ea.min(scene.items.len())..eb.min(scene.items.len()), effects);
            scene.items.splice(a.min(scene.items.len())..b.min(scene.items.len()), merged);
            scene
        } else {
            self.scene.clone()
        };
        // the mirror's view runs the sky pass too (0x468AE0 → 0x45D7D0 while `+0x12C` is set): its
        // own eye and heading for the ring, and the sun between the two level runs (0x45D6F6)
        // through the mirror's view (`sun_items`). The range lies before the track's, which is
        // replaced first.
        let mut scene = scene;
        if let Some(s) = &self.sky {
            let (a, b) = self.sky_item_range;
            let forward = (cam.target - cam.position).normalize_or_zero();
            let view = nfs_game::sky::SkyView { eye: cam.position, forward, up: cam.up, main: false };
            let sun = self.sun_items(&cam, w.max(1) as f32 / h.max(1) as f32, SUN_VIEW_MIRROR);
            let items: Vec<_> = s.draw_items(&view, false).chain(sun).chain(s.draw_items(&view, true)).collect();
            scene.items.splice(a.min(scene.items.len())..b.min(scene.items.len()), items);
        }
        // the clear: 0x459A70 at 0x468DE8, before the mirror's sky pass (so at the slice the main
        // pass saved), with `[0x627F43] & 0x10` set for the pass (0x468DA8…0x468E1F) — the colour
        // record's group B (`SkyBin::background_at`); the view's constructor colour 0xFF77BBFF
        // (0x46920B) is overwritten every pass (0x466E60). The fog: the sky pass's two
        // (0x45D2E0 before the levels, 0x45D3B0 after) at the slice the mirror's pass saves
        // (0x45D587), which the main view's scene does not share.
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        let main_slice = self.cam_slice.or(self.race.as_ref().map(|r| r.slice)).unwrap_or(0) as i32;
        let mirror_slice = self.mirror_slice(cam.position);
        let mut clear = [0x77 as f64 / 255.0, 0xBB as f64 / 255.0, 1.0, 1.0];
        if let Some(bin) = self.track.sky_bin.as_ref() {
            if let Some(c) = bin.background_at(self.sky_level(), 1, main_slice, lap) {
                clear = [c[0] as f64, c[1] as f64, c[2] as f64, 1.0];
            }
            let fog = |sky: bool| bin.fog_at(sky, mirror_slice, lap).map(|(colour, density)| nfs_renderer::Fog { colour, density });
            (scene.sky_fog, scene.fog) = (fog(true), fog(false));
        }
        if let Some(r) = self.renderer.as_mut() {
            let sky = clear;
            // the mirror's view is type 1, which sets `[0x6115CC]+0x190` (0x43AD4E): no specular
            // or sky-map pass on the cars in it. Its own far plane, 65000 (0x4691D0), scales its
            // depth bias and is the camera's clip
            r.set_mirror_view(true);
            self.mirror_tex = Some(r.render_offscreen(&scene, &cam, w, h, self.mirror_tex, Some(sky), Some(65000.0)));
            r.set_mirror_view(false);
        }
    }

    /// 0x459020 again in the mirror's pass: it is the last call of the scene draw 0x468AE0
    /// (0x468B94), which the mirror's pass 0x468CA0 runs too (0x468E04), and only its draw is gated
    /// on the main view (`+0x12C == 2`, 0x45913C). In the second branch the mirror's call finds no
    /// tick since the main one's (`[this]` just set, `n < 1` at 0x45909D) and does nothing. In the
    /// first — the dialog queue up — it steps both emitters by 1 once more (0x459075 / 0x459086),
    /// through the current view's camera (0x4567D0 reads `[[0x628130]+0x10]+0xB8`): the mirror's,
    /// so the step's turn and move run main → mirror here and mirror → main on the next frame; the
    /// strengths at the slice the mirror's sky pass saved (0x45CFD0 / 0x45D000 read `[sky+0x50]`).
    /// The counter `[0x627E10]` it bumps has no reader. Called after the mirror's sky update, as
    /// the pass order has it.
    pub(crate) fn mirror_weather_step(&mut self) {
        let Some(cam) = self.rear_mirror else { return };
        if self.weather.is_none() {
            return;
        }
        let frozen = self.weather_frozen();
        let slice = self.sky.as_ref().map_or_else(|| self.mirror_slice(cam.position), |s| s.pass_slice());
        let (snow, rain) = self.weather_strengths_at(slice);
        let view = nfs_game::weather::View::new(cam.position, (cam.target - cam.position).normalize_or_zero(), cam.up);
        let tick = self.tick as u32;
        let (Some(w), Some(p)) = (self.weather.as_mut(), self.track_particles.as_mut()) else { return };
        w.frame(tick, snow, rain, view, frozen, p.rng_mut(), &mut self.fe_rng);
    }
}
