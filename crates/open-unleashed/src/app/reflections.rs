//! ENHANCEMENT E8: the dynamic environment map of the player's car.
//!
//! The race's main scene is drawn without the player's car from the car's position into the six faces of an
//! environment map, one face a frame in turn (all six at the start), and the paint and glass of every car in the main view take
//! it through a Schlick-Fresnel clear coat (`shader.wgsl` `shade`, `Scene::refl`) on top of the original's
//! sphere-map highlight, which stays. Nothing here feeds the simulation: it reads the world and draws.
//!
//! What a face holds: the sky (ring, layers, sun) and the track's geometry as the main walk lists it
//! (`World::article_draws_into` / `track_draw_lists_into`, the same lists the rear-view mirror builds for its
//! own eye), under the main view's light, fog and clear colour — no cars, skid marks, particles or weather
//! (an opponent is not in the player's reflection; the clear coat of an opponent's own paint reads the same
//! map: the surroundings, not the other cars).
//!
//! ENHANCEMENT E49 (reflections of nearby objects): a face also takes the main view's street / tunnel lamps (E23, without E47's
//! shadow maps) and headlight spots (E15, without their shadow map) on its surfaces, and the track's lamp flares (the walk's
//! `flare_items`, as the main view draws them) so the lamps glint; the track is drawn only to [`NEAR_DRAW`] (the faces' frustum
//! cull already per face). The cube has twelve layers: after a face is drawn and kept for every car's coat, the near cars
//! (within [`NEAR_CARS`], the mirror pass's own draw of them: the player's car is not queued, lamps and blob shadows are) are
//! drawn over the same face for the player's own coat (`DrawItem::near_env`), so the opponents keep a reflection without cars
//! and none reflects itself. The renderer reads the cube from the shading point through the covered box (`gpu::near_proxy`).

use crate::*;

/// ENHANCEMENT E49: how far the environment faces draw the track (m; E8's faces reach `max(draw distance x 0.5, 100)`). The
/// far scenery in a reflection is a few texels of a 256 px face behind the sky and the fog; the horizon mountains are the sky's.
/// APPROXIMATION: by eye (NFSU2's faces stop at 64 m).
const NEAR_DRAW: f32 = 100.0;
/// ENHANCEMENT E49: the cars within this distance of the cube's eye are drawn into the player's faces (m). APPROXIMATION: by eye.
const NEAR_CARS: f32 = 50.0;

/// ENHANCEMENT ENH-COAT: the cube's face size at every level of E8 (px; the lacquer's roughness picks a mip of it).
const FACE_SIZE: u32 = 256;
/// ENHANCEMENT E8: the faces drawn a frame once the cube is complete: one (RENDER-AUDIT §8 item 2, owner-approved 2026-11-15:
/// even frame pacing and half the faces' cost; the cube is renewed in 6 frames, the reflections lag up to 6 frames instead of
/// 3 with the two a frame before).
const FACES_PER_FRAME: u32 = 1;

/// The levels of E8 (the clear coat's: 1 new, 2 normal, 3 worn; `ENHANCEMENTS.md`): the faces' size and how many are drawn per
/// frame ([`FACE_SIZE`], [`FACES_PER_FRAME`] at every level).
pub(super) fn level_params(level: u8) -> Option<(u32, u32)> {
    match level {
        0 => None,
        _ => Some((FACE_SIZE, FACES_PER_FRAME)),
    }
}

impl App {
    /// Draws this frame's faces of the environment map and says whether the main scene takes it
    /// (`Scene::refl`). Called after the mirror pass, before the main view is rendered.
    pub(crate) fn render_reflections(&mut self) {
        self.scene.refl = 0.0;
        // ENHANCEMENT ENH-FE3D: the front end's scene takes the weight its own map was made for (`fe_enh.rs`)
        if self.vehicle.is_none() {
            self.scene.refl = self.fe.enh_refl;
            return;
        }
        let Some((size, per_frame)) = level_params(self.enh.level(nfs_renderer::E8)) else {
            self.refl_done = 0;
            return;
        };
        let (Some(pose), true, true) = (self.dyn_pose, self.vehicle.is_some(), self.world.is_some()) else { return };
        if self.renderer.is_none() {
            return;
        }
        let eye = pose.local_pos + Vec3::Y * 0.3;
        // ENHANCEMENT E49: the near objects (the renderer makes the cube again with its twelve layers when this changes)
        let near = self.enh.is_on(nfs_renderer::E49);
        // a new size, or a car that jumped (a new race, a reset): every face again
        if size != self.refl_size || eye.distance(self.refl_eye) > 300.0 || near != self.refl_near {
            self.refl_done = 0;
            self.refl_size = size;
            self.refl_near = near;
        }
        self.refl_eye = eye;
        let n = if self.refl_done == 0x3F { per_frame } else { 6 };
        for _ in 0..n {
            let face = self.refl_next % 6;
            self.refl_next = (face + 1) % 6;
            self.render_refl_face(eye, face, size, near.then_some(pose));
            self.refl_done |= 1 << face;
        }
        self.scene.refl = 1.0;
        // ENHANCEMENT E49: a night track (the original's night flag) shows the live sky in the sphere-map overlay (NEAR-REFL3),
        // every other track the live sky's hue at the painted sky's strength (NEAR-REFL4)
        self.scene.near_sky = if self.track.is_night() { nfs_renderer::NearSky::Night } else { nfs_renderer::NearSky::Hue };
    }

    /// One face: the sky and the track from `eye` along `REFL_FACES[face]`. ENHANCEMENT E49 (`near`: the player's pose): with the
    /// lamps, the flares and, over it for the player's layer, the near cars.
    fn render_refl_face(&mut self, eye: Vec3, face: u32, size: u32, near: Option<crate::app::car_lights::DynPose>) {
        let (dir, up) = nfs_renderer::REFL_FACES[face as usize];
        let (forward, up) = (Vec3::from(dir), Vec3::from(up));
        let tan = nfs_renderer::REFL_TAN_HALF;
        let cam = Camera { position: eye, target: eye + forward, up, fov_y_radians: 2.0 * tan.atan(), near: 0.3, far: self.camera.far };
        let right = forward.cross(up).normalize_or_zero();
        let reach = self.enh_reach();
        let draw = if near.is_some() { NEAR_DRAW } else { (self.draw_distance * reach * 0.5).max(100.0) };
        let cull = nfs_game::world::CullView { eye, forward, right, up: right.cross(forward).normalize_or_zero(), tan_h: tan, tan_v: tan, near: cam.near, draw, reach };
        // the frame's options as the main pass read them (`RenderOptions`)
        let opts = self.dyn_pose.map_or_else(|| crate::app::render_scene::RenderOptions::read(self), |p| p.opts);
        let mut items = std::mem::take(&mut self.scratch.mirror_items);
        items.clear();
        // the sky first (no depth test), the track after it, as the main scene lays them out
        if let Some(s) = self.sky.as_ref().filter(|_| !opts.sky_off) {
            let view = nfs_game::sky::SkyView { eye, forward, up, main: true };
            let sun = self.sun_items(&cam, 1.0, SUN_VIEW_MAIN);
            items.extend(s.draw_items(&view, false));
            items.extend(sun);
            items.extend(s.draw_items(&view, true));
        }
        if let Some(world) = self.world.as_ref() {
            let sc = &mut self.scratch;
            let mut draws = std::mem::take(&mut sc.mirror_draws);
            draws.clear();
            world.article_draws_into(&cull, &mut draws);
            world.track_draw_lists_into(&draws, eye, &mut sc.mirror_lists);
            nfs_renderer::sort_order(&sc.mirror_lists.near, false, &mut sc.order);
            sc.mirror_merged.clear();
            let near = &sc.mirror_lists.near;
            sc.mirror_merged.extend(sc.order.iter().map(|&(_, i)| nfs_renderer::DrawItem { early: true, layer_light: Some(nfs_game::world::TRACK_LIGHT), ..near[i as usize].1 }));
            sc.blended.clear();
            sc.blended.extend(sc.mirror_lists.far.iter().map(|&(k, i)| (k, nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i })));
            nfs_renderer::farthest_first_into(&sc.blended, &mut sc.order, &mut sc.mirror_merged);
            sc.mirror_merged.extend(sc.mirror_lists.rest.iter().map(|&i| nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i }));
            items.extend_from_slice(&sc.mirror_merged);
            sc.mirror_draws = draws;
        }
        // ENHANCEMENT E49: the track's lit flares (the lamp heads) of this face's walk, laid out toward the face's eye
        if near.is_some() {
            let mut flares = std::mem::take(&mut self.scratch.effects);
            flares.clear();
            let saved = std::mem::replace(&mut self.camera, cam);
            self.flare_items(&mut flares, forward, right, right.cross(forward).normalize_or_zero(), &self.scratch.mirror_draws);
            self.camera = saved;
            items.extend_from_slice(&flares);
            flares.clear();
            self.scratch.effects = flares;
        }
        // the main scene's light, fog and clear colour, in the true (un-mirrored) world; no reflection in it
        let mut scene = self.scene.clone_header(items);
        scene.mirror = false;
        scene.refl = 0.0;
        scene.ao = false;
        scene.eye = false;
        // ENHANCEMENTS E27 / E29: nor motion blur or shake
        scene.motion = None;
        // ENHANCEMENT E9: the environment faces are drawn without sun shadows (a shadow pass per face would cost six)
        scene.shadows = nfs_renderer::ShadowLights::NONE;
        // ENHANCEMENT E17: no water reflection inside the environment faces (it would reflect the previous map)
        scene.water = None;
        if near.is_some() {
            // ENHANCEMENT E49: the main view's lamps and headlight spots light the face (no spot shadow map: a pass per face)
            scene.spots.shadowed = 0;
        } else {
            // ENHANCEMENT E15: nor with the headlight spots
            scene.spots = nfs_renderer::Spots::default();
            scene.lamps = nfs_renderer::Lamps::default();
        }
        // ENHANCEMENT E43: nor the cars' contact shadows
        scene.contacts = nfs_renderer::Contacts::default();
        scene.haze = None;
        scene.lbeam = None;
        scene.drops = None;
        scene.lens_probe = None;
        // ENHANCEMENT ENH-COAT: the faces are float images now (the float scene's format): no light shafts / depth of field in them
        scene.shafts = None;
        scene.dof = None;
        let clear = scene.clear_color;
        match near {
            Some(pose) => {
                let cars = self.near_car_items(&cam, &cull, pose, size);
                let cars = scene.clone_header(cars);
                if let Some(r) = self.renderer.as_mut() {
                    r.render_refl_face_near(&scene, &cars, &cam, nfs_renderer::EnvFace { face, size, clear, depth_far: nfs_renderer::VIEW_FAR });
                }
            }
            None => {
                if let Some(r) = self.renderer.as_mut() {
                    r.render_refl_face(&scene, &cam, nfs_renderer::EnvFace { face, size, clear, depth_far: nfs_renderer::VIEW_FAR });
                }
            }
        }
        self.scratch.mirror_items = scene.items;
    }

    /// ENHANCEMENT E49: the near cars of one environment face: the mirror pass's own draw of the cars (`dyn_items` with
    /// `PassKind::EnvFace`: the player's car is not queued, no effect drive, no spots, no contact shadows), culled by the face's frustum to
    /// [`NEAR_CARS`] and picked for the face's size, farthest first. The cars' level-of-detail state the pick writes is put back
    /// (the main view's pick of the next frame starts from its own).
    fn near_car_items(&mut self, cam: &Camera, cull: &nfs_game::world::CullView, pose: crate::app::car_lights::DynPose, size: u32) -> Vec<nfs_renderer::DrawItem> {
        // no car in reach (the cars' own spheres are tested by the walk): nothing to draw, and the walk over every car is not run
        let reach = (NEAR_CARS + 8.0).powi(2);
        let near = |p: Vec3| p.distance_squared(cam.position) < reach;
        let any = self.road.opponents.iter().any(|o| near(o.vehicle.position))
            || self.road.traffic.as_ref().is_some_and(|t| t.cars.iter().any(|c| c.active && near(c.position)))
            || self.road.police.as_ref().is_some_and(|p| p.cars.iter().any(|c| c.active && near(c.position)));
        if !any {
            return Vec::new();
        }
        let render_w = self.renderer.as_ref().map_or(640.0, |r| r.size().0.max(1) as f32);
        let p = crate::app::car_lights::DynPose { pass: crate::app::car_lights::PassKind::EnvFace, cull: Some(nfs_game::world::CullView { draw: NEAR_CARS, reach: 1.0, ..*cull }), lod: (size as f32, size as f32), ref_width: render_w, ..pose };
        let lods = (self.road.opponent_lods.clone(), self.road.traffic_lods.clone(), self.road.police_lods.clone());
        let saved_cam = std::mem::replace(&mut self.camera, *cam);
        let mut pools = self.pool_used;
        let mut queue = Vec::new();
        self.dyn_items(&p, &mut pools, &mut queue);
        self.camera = saved_cam;
        (self.road.opponent_lods, self.road.traffic_lods, self.road.police_lods) = lods;
        let mut order = Vec::new();
        let mut out = Vec::with_capacity(queue.len());
        nfs_renderer::farthest_first_into(&queue, &mut order, &mut out);
        out
    }
}
