//! Scene assembly: the per-frame draw lists (`rebuild_scene`), the dynamic items and the HUD geometry.

use crate::*;
use super::car_draw::*;


/// `fe.txt`'s `skyoff` row (`[0x657DB4]` SKYOFF, config id 0x5A, 0x4B14C4): the registry key [`App::sky_off`] reads.
const SKYOFF_KEY: &str = "skyoff";

/// The options and per-track facts the race's scene build reads, looked up ONCE a frame by the main pass
/// ([`RenderOptions::read`] in `rebuild_scene`) and handed to the mirror's and the environment faces' passes in
/// [`DynPose::opts`], instead of a string-keyed lookup (`choice_index`, the registry) at every use in every pass. Read a
/// frame at a time, not a race: the options can change while a race runs (the pause menu, the race panel).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RenderOptions {
    /// `CARDETAIL` `[0x6197C4]`, the `Graphics.CarDetail` choice (the LOD pick, the crush 0x43E390, slot 9's driver)
    pub(crate) car_detail: u8,
    /// `SKYRENDERING` `[0x657D9C]` ([`App::sky_level`])
    pub(crate) sky_level: usize,
    /// `[0x657DB4]` SKYOFF ([`App::sky_off`])
    pub(crate) sky_off: bool,
    /// ENHANCEMENT E23 (AUTOBAHN-LAMPS): the track lights its open street lamps by day (`street_lamps::dusk_lamps`)
    pub(crate) dusk_lamps: bool,
}

impl RenderOptions {
    pub(crate) fn read(app: &App) -> Self {
        RenderOptions {
            car_detail: app.car_detail(),
            sky_level: app.sky_level(),
            sky_off: app.sky_off(),
            dusk_lamps: super::street_lamps::dusk_lamps(app.world.as_ref()),
        }
    }
}

/// The front end's far plane (its rooms; no draw-distance key there).
const FE_FAR: f32 = 20000.0;


/// The skid strip's UV table `[0x628B64]`, `(0,0) (1,0) (0,1) (1,1)` over and over (0x478930).
const SKID_UV: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
/// A skid mesh's vertex before it is written: at the origin, facing up, no colour.
const SKID_VERTEX: nfs_renderer::Vertex = nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] };
/// The skid strips' depth bias `−80000 · cam[+0xC] / d²`, `fistp`-rounded and clamped to −10…−1 (0x478B51…0x478BA2).
const SKID_BIAS_NUM: f64 = -80000.0;
const SKID_BIAS_MIN: i32 = -10;
const SKID_BIAS_MAX: i32 = -1;
/// The flush's alpha test `GREATER 10` the skid cells are drawn under (0x24 ← 10, 0x466703).
const SKID_ALPHA_REF: u8 = 10;

impl App {
    /// Repaint the car's texture pages when its body damage changes (`RE/notes/car-model.md` §11):
    /// the `<model>D.fsh` / `<model>L.fsh` decals are blended over the pages with an alpha that
    /// follows each zone's damage. `CarModel::update_damage` is 0x438230, which does nothing until
    /// a zone's value changes, so this is cheap to call every frame. Every car with its own model
    /// takes it — the player (the car the camera follows) and each opponent; the traffic and the
    /// cops share one model per type and keep their pages clean (gap: the original's are per car).
    ///
    /// It also runs slot 9's and slot 6's per-frame animations (0x438440, 0x43950E): the wing, the
    /// pop-up lamps, the driver's gestures, the race channels and the siren. Their clocks are the
    /// original's real-time ones — `GetTickCount` for the easers (0x438A50), `rdtsc` for the driver's
    /// players (0x438B92), the 128 Hz `TIMER_getTicks` for the siren (0x53C2E0) — none stops for a
    /// pause, a replay or the fly-in, so `anim_ms` is the frame's real time in whole ms with the
    /// fraction carried (`fe_step_ms`, the front end's clock step). A scripted run, which draws no
    /// frame, passes 16 per call (TEST GLUE, `NFS_SHOT_SETTLE`). *(Until 2026-11-16 every rendered
    /// frame passed 16 ms: at 120 Hz the animations ran 1.9× too fast.)*
    // PORT-DEVIATION D66 (RE/notes/PORT-DEVIATIONS.md): the step is capped at 250 ms a frame; the original's clocks are not
    /// ENHANCEMENT E43 / E51: `car` (standing at `origin`) onto a list sorted later by its squared distance from `eye`.
    pub(super) fn push_near<T>(list: &mut Vec<(f32, T)>, eye: Vec3, origin: Vec3, car: T) {
        list.push((origin.distance_squared(eye), car));
    }

    /// The cars' and the skid marks' own `+0xC` entries for one view's pass: the main pass's
    /// (`Self::rebuild_scene`) or the mirror's (`Self::render_mirror`, `PassKind::Mirror`), each from
    /// the pass's own eye — `self.camera` is the view's camera while this runs (the mirror swaps its
    /// own in), its far plane `self.view_far`. The draw 0x4430A0 runs in both passes over the same
    /// car list (0x443610): in the mirror's, slot 0's flag bit 4 is set (0x468DA8…0x468E1F) and the
    /// camera's own car (`carInfo+0x520 == 0x465D80(0)`) returns at 0x4430EC — nothing of it is queued,
    /// no shadow, body or lamps; the others go through the frustum test 0x539480 (`car+0x330`, radius
    /// `car+0x3B4`) and the LOD pick of the mirror's view. Difference (not an approximation, see below): the draw's last act, the
    /// effect drive (slot 7, 0x443239), runs in the mirror's pass too in the original, with that
    /// pass's eye; the port drives the effects once a frame, from the main pass. **Settled
    /// 2026-09-30: equivalent.** 0x43EF80 measures `d²` from the slot's eye
    /// `[0x627EA4…0x627EAC]`, which the mirror pass does not touch (its own eye is the view's
    /// camera object), and every write it makes — `+0x2C = 1`, `+8 = 100`, `+0x10 = d²`, the
    /// emitter points and directions, `+0x28` behind an `!=` guard (0x454AC0), the draw set's
    /// `std::set` insert — is a function of the car's state and that eye, so a second call in the
    /// frame writes what the first did. No count or timer moves in the drive. The own car returns
    /// before it in the mirror (0x4430E5) and the main pass has no frustum test, so the main drive
    /// already reaches every car the mirror's would. The mirror's meshes come after the main pass's in
    /// each dynamic pool (`PoolUse`), so nothing the main view queued is rewritten.
    ///
    /// The car list `[0x6115F0]` is walked the other way round — the traffic from its last slot, the cops, the opponents from the last, the player last
    /// (`track_and_car_items`) — each car one [`Self::draw_car`], then the main view's lights and shadows of the cars
    /// ([`Self::finish_main_pass`]) and the skid strips ([`Self::skid_items`]), appended to `queue` (the caller clears it).
    pub(crate) fn dyn_items(&mut self, p: &DynPose, pools: &mut PoolUse, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        let DynPose { alpha, local_draw, local_pos, local_rot, .. } = *p;
        let main = p.pass == PassKind::Main;
        let lamp_ms = (self.tick as f64 + self.pause_offset + self.pause_ticks) * 1000.0 / TICKS_PER_SEC_F64 + self.glue.shot_settle_ms;
        // The LOD pick, `R3DCar` slot 13 0x439F90, which the car's draw 0x4430A0 asks first
        // (0x443192): each car's class from its box on the screen `[0x657A48]` wide and `CARDETAIL`
        // (`[0x6197C4]`, the `Graphics.CarDetail` choice, five tiers under `nfs5.dll`, which also
        // cuts the replay test out of the pick, 0x43A08A); `None` — 3 pixels or less — draws nothing of it, lamps included
        // (`CarModel::lod_class`). Then the car draw (0x444E00, the dll's 0x10001090) makes its passes from the class
        // (`race_passes`). *(Until 2026-09-26 every car was LOD 1 at every distance, every part
        // of it drawn.)* The mirror's pass asks again for its own view (`Self::render_mirror`).
        // the pick's screen: the view's own viewport (the mirror's tray, 0x539700 through its
        // camera), the size scaled to the virtual reference width (640 at 4:3) by the render width `[0x657A48]`
        let lod_view = self.renderer.as_ref().map(|r| {
            let (w, h) = p.lod;
            // `nfs5.dll` replaces the 640 of 0x43A2DD with its virtual 2-D width for the mode (window)
            let (win_w, win_h) = r.size();
            let virtual_width = nfs_game::world::virtual_width(win_w.max(1) as f32, win_h.max(1) as f32);
            nfs_game::world::LodView { view_projection: self.camera.view_projection(w / h), width: w, height: h, ref_width: p.ref_width, virtual_width }
        });
        let counts = (self.road.traffic.as_ref().map_or(0, |t| t.cars.len()), self.road.police.as_ref().map_or(0, |p| p.cars.len()), self.road.opponents.len());
        let ctx = CarPassCtx {
            main,
            cull: p.cull,
            local: (local_draw, local_pos, local_rot),
            counts,
            // **0x465D80(0)**: the car the model's "followed car" arms follow (`None` = the local car)
            followed: self.followed_seat(),
            own_view: self.camera_view(),
            replaying: self.replay_view().is_some(),
            // the indicators' phase: `TIMER_getTicks() / (ticksPerSecond/2)` odd (0x4393C8…0x4393D8), `lamp_slots`
            blink: (self.tick / BLINK_TICKS) % 2 == 1,
            // ENHANCEMENT E15: the other cars' lit headlights (the player's are made in `finish_main_pass`)
            spots_on: main && self.enh.is_on(nfs_renderer::E15),
            // ENHANCEMENT E43: the cars' contact shadows of this view (the main view only); a car that has one has no `SHAD` patch laid
            contact_on: main && self.enh.is_on(nfs_renderer::E43),
            // ENHANCEMENT E51 (CARSH-FADE): the cars whose share of the sun the renderer measures (the main view only)
            sun_cars_on: main && self.enh.level(nfs_renderer::E51) == nfs_renderer::Enhancements::E51_WHERE_SUN,
            // (both lists are sorted nearest to the eye first: each car's pose is made once for both)
            eye: self.camera.position,
            // the cars' effect drive 0x43EF80(1), which the car's draw 0x4430A0 runs last (slot 7
            // 0x4383C0 at 0x443239) — so for every car drawn: the eye `0x627EA4`, the sky's field 5 at
            // the camera's slice (0x45DD80 → 0x45D000, the rain's value, `weather_strengths`) and
            // `[0x6197AA]` taken as set (`thrash.md` §4). The effect sets are keyed player 0, the
            // opponents, the cops, the traffic (`CarSlot::effect_index`)
            fx_wet: self.weather_strengths().1 > FX_WET_RAIN,
            alpha,
            lamp_ms,
            fade: (lamp_ms / 1000.0).min(1.0) as f32,
            detail: p.opts.car_detail,
            lod_view,
            dusk_lamps: p.opts.dusk_lamps,
            mirror: self.mirror,
            looking: self.input.held(Action::RearView) && self.vehicle.is_some(),
            // every car drops a blob shadow on the ground under it (0x43B820), off `car+0x330` and cut
            // to `car+0x3A8…` like every car's (0x43B220)
            shadow_material: self.draws.shadow_draw.map(|(_, m)| m),
        };
        // the per-car buffers live in `self.scratch` between frames: taken here, put back at the end
        let mut acc = PassLists {
            car_items: std::mem::take(&mut self.scratch.car_items),
            shadow_verts: std::mem::take(&mut self.scratch.shadow_verts),
            lamp_draws: std::mem::take(&mut self.scratch.lamp_draws),
            contact_cars: Vec::new(),
            sun_cars: Vec::new(),
            spot_others: Vec::new(),
            // ENHANCEMENT E15 (ENH-CONES): the original's beam strips of this view, faded once the spots are chosen
            cone_log: Vec::new(),
            lamp_used: pools.lamp,
            shadow_used: pools.shadow,
        };
        // a mirrored race turns every car model back across its x (0x4433A1…0x4433BC)
        for m in self.car_model.iter_mut().chain(self.road.opponent_models.iter_mut()).chain(self.road.traffic_models.iter_mut()).chain(self.road.police_models.iter_mut()) {
            m.mirror = ctx.mirror;
        }
        self.road.traffic_lods.resize(counts.0, Default::default());
        self.road.police_lods.resize(counts.1, Default::default());
        self.road.opponent_lods.resize(counts.2, Default::default());
        self.traffic_cars(&ctx, &mut acc, queue);
        self.police_cars(&ctx, &mut acc, queue);
        self.opponent_cars(&ctx, &mut acc, queue);
        self.player_car(&ctx, &mut acc, queue);
        if main {
            self.finish_main_pass(&ctx, &mut acc, local_draw, local_rot);
        }
        (pools.lamp, pools.shadow) = (acc.lamp_used, acc.shadow_used);
        (self.scratch.car_items, self.scratch.shadow_verts, self.scratch.lamp_draws) = (acc.car_items, acc.shadow_verts, acc.lamp_draws);
        self.skid_items(p, pools, queue);
    }

    /// The skid strips of one pass, each its own `+0xC` entry (see the notes below), into `queue` after the cars'.
    fn skid_items(&mut self, p: &DynPose, pools: &mut PoolUse, queue: &mut Vec<(u32, nfs_renderer::DrawItem)>) {
        // the skid marks: 0x478AA0 draws a strip of more than two points when both of its ends
        // lie within 100 of the camera, as a strip with the UV table `[0x628B64]` — `(0,0)
        // (1,0) (0,1) (1,1)` over and over (0x478930) — its colours and `SKDP`, unlit.
        // The depth bias (0x478B51…0x478BA2): `−80000 · cam[+0xC] / d²` with `d²` the nearer
        // end's horizontal square distance, rounded by `fistp` and clamped to −10…−1, handed to
        // Thrash state 0x18 (0x537A60), `b · 2^-15` on the game's screen z `far/(far − near) ·
        // (1 − near / Z)` (0x540CE0, `RE/notes/thrash.md` §6d.3) — `DrawItem::depth_bias`, which
        // the shader adds to the clip z. `cam[+0xC]` is `far · near / (far − near)`
        // (0x467247…0x467261) of the view's planes — near the slot's `+0x5C`
        // (`camera::near_plane`: 0.5, 0.15 in the driver view), far 700 — the slope of that z,
        // so the bias is a near-constant `80000 / 32768` ≈ 2.4 m of view depth toward the eye
        // until the clamp (−10…−1) holds it. The render states of 0x468240 are not read
        // (alpha blended)
        if let (Some((_, material)), Some(r)) = (self.draws.particle_draw, self.renderer.as_mut()) {
            let cam = self.camera.position;
            let near = |p: Vec3| (p.x - cam.x).powi(2) + (p.z - cam.z).powi(2) <= nfs_game::skidmarks::DRAW_DISTANCE.powi(2);
            let mut used = pools.skid;
            let skid_wet_on = self.enh.is_on(nfs_renderer::E26) && self.ambient.rain.any != Some(false);
            let mut skid_verts = std::mem::take(&mut self.scratch.skid_verts);
            // one queue entry per cell in the original (0x44FFC0 with the walk's value for the
            // cell), whose draw 0x47A060 then lays the cell's strips in array order; every strip
            // of a cell carries the cell's key here, in that order
            // the walk's gates for a cell (node pass, frustum, `Vis `, `s² < far²`) with
            // the pass's own view — the main one's, or the mirror's
            let cells: Vec<(u32, &nfs_game::skidmarks::Strip)> = match (self.world.as_ref(), p.cull.as_ref()) {
                (Some(w), Some(cv)) => {
                    let walk = w.cell_walk(cv);
                    self.skid.cells_walked(|cx, cy, cz, r, nodes| walk.visit(cx, cy, cz, r, nodes))
                }
                _ => self.skid.cells(cam).flat_map(|(k, strips)| strips.into_iter().map(move |s| (k, s))).collect(),
            };
            for (key, strip) in cells {
                let (Some(&first), Some(&last)) = (strip.points.first(), strip.points.last()) else { continue };
                if !near(first) || !near(last) {
                    continue;
                }
                // the pool grows as more strips come into range, so none is left out
                if self.draws.skid_pool.len() <= used {
                    let cap = nfs_game::skidmarks::MAX_CAPACITY as u32;
                    let mut skid = nfs_renderer::MeshData::default();
                    skid.vertices = vec![SKID_VERTEX; cap as usize];
                    skid.indices = (0..cap - 2).flat_map(|k| [k, k + 1, k + 2]).collect();
                    self.draws.skid_pool.push(r.upload_dynamic_mesh(&skid));
                }
                let mesh = self.draws.skid_pool[used];
                used += 1;
                let z_near = self.camera.near;
                let far = self.view_far;
                let k = far * z_near / (far - z_near);
                let d2 = |p: Vec3| ((p.x - cam.x).powi(2) + (p.z - cam.z).powi(2)).max(1e-6);
                // 0x478B57…0x478B84: the quotient on the x87 stack, `fstp` to a float, then
                // `fistp` — round to nearest even (f64 stands for the extended precision)
                let bias = (((SKID_BIAS_NUM * k as f64 / d2(first).min(d2(last)) as f64) as f32).round_ties_even() as i32).clamp(SKID_BIAS_MIN, SKID_BIAS_MAX);
                // the strip's vertices, built and uploaded only when this mesh does not hold this strip already (`SkidWritten`)
                let written = &mut self.draws.lamp_pools.skid_written;
                if written.len() <= used {
                    written.resize_with(used, || None);
                }
                let slot = &mut written[used - 1];
                if !slot.as_ref().is_some_and(|w| w.holds(mesh, &strip.points, &strip.colours)) {
                    skid_verts.clear();
                    skid_verts.resize(nfs_game::skidmarks::MAX_CAPACITY, SKID_VERTEX);
                    for (k, v) in skid_verts.iter_mut().enumerate() {
                        // past the strip's end every vertex sits on its last point: no area
                        let i = k.min(strip.points.len() - 1);
                        v.position = strip.points[i].into();
                        v.uv = SKID_UV[k % SKID_UV.len()];
                        v.color = argb_rgba(strip.colours[i]);
                    }
                    r.write_mesh_vertices(mesh, &skid_verts);
                    *slot = Some(SkidWritten { mesh, points: strip.points.clone(), colours: strip.colours.clone() });
                }
                // the alpha test `GREATER 10`: the skid object's drawer (`RDrawLineBundle`, vtable
                // 0x5B3D28, type 0xE) queues each cell through slot 1 0x44FFC0 → 0x466A90 (list
                // `+0xC`) — from the track walk 0x47F890's step 6 (`drawer slot 1(index, value)`,
                // the cell being an item of `WLineBundle`, `RE/RENDERING.md` "The walk") with the
                // value of a non-`bl` item, `s²` from the cell's sphere ([`SkidMarks::cell_key`],
                // 0x479FC0) — and the flush 0x466660 draws that list (slot 2 0x451420 → 0x47A060)
                // after its 0x24 ← 10 (0x466703), not under the particles' 1, which come later in
                // the pass. Merged with the track's far list and the cars' own `+0xC` entries,
                // farthest first (`Self::blended_dyn_items`). *(Until 2026-09-30 the key was the
                // strip's own midpoint square distance to the eye.)* **Closed 2026-10-02, the chain read
                // to the instruction:** 0x44FFC0 `fistp`s the walk's float (round to nearest even, the x87 default,
                // 0x44FFC9), packs the item (0x44FE70: `[+0]` the bundle, `[+4]` 1, `[+6]` the ref's index),
                // copies it through 0x443250 / 0x443270 with the key as the entry's last dword `+0xC`, takes the
                // view `[0x628130]` (0x44FE90) and its list pointer `+0x54` (0x4432A0) and calls 0x466A90, whose tree
                // insert compares that dword against each node's `+0x1C` **unsigned** (`jbe` / `ja`, 0x466AD3) and
                // goes right for greater, left for equal-or-less — the key is exactly `round(s²)`. The walk's gates
                // are the port's (`TrackWorld::cell_walk`). Residual, class B (no longer an unread call):
                // a tie between two cells' keys (every cell whose sphere holds the eye has `s² = 0`) is ordered in
                // the original by the node pass and the registration order of the cell records (the table sorted by
                // the node addresses, `RE/notes/particles.md` §9 — the pool's runtime order); the port
                // queues the cells in its table order, after the track's far list.
                // ENHANCEMENT E26: on a track with a wet stretch the strip carries the wetness at its slice (read only with E26 on)
                let (csm, wet) = if skid_wet_on { crate::app::rain::strip_wet(self.slices.as_ref(), self.track.sky_bin.as_ref(), self.cam_slice, first) } else { (0, 0.0) };
                queue.push((key, nfs_renderer::DrawItem { layer_light: Some(unlit([1.0; 3])), alpha_test: true, alpha_ref: SKID_ALPHA_REF, depth_bias: bias, csm, wet, ..nfs_renderer::DrawItem::new(mesh, material, Mat4::IDENTITY) }));
            }
            pools.skid = used;
            self.scratch.skid_verts = skid_verts;
        }
    }

    /// ENHANCEMENT E49 (NEAR-REFL3): the player's section sky map (`R3DSkyMapTextureLayer`, `RE/notes/lighting.md` §3.2: in a
    /// covered stretch the lamp rows' light blobs) slid across the car by the car's real lateral offset from the slice's
    /// centre line. The original slides the map along the road by the full offset (`v += (o − c)·fwd / 6`, `+0x148`) but
    /// across it by a tenth (`u += 0.1·(o − c)·lat / 6`, 0x445D2C: `0.1 · [+0x144]`), so a car near a wall passes under the
    /// lamps' glints as if it drove down the centre; here the lateral term takes the forward one's rate, `(o − c)·lat / 6`
    /// (the planes of `world::sky_planes` with the 0.1 dropped: `u.w += 0.9·(o − c)·lat / 6`). The live faces' overlay
    /// (NEAR-REFL2) already shows the lamps where they are; this moves the original's glints with them. APPROXIMATION: the
    /// map is not a scaled picture of the ceiling (its lateral scale is the original's 3.33 · width / `texturesize`), so the
    /// along-road rate is the only one the layer itself gives; by eye.
    pub(super) fn sky_map_lateral(&self, mut map: nfs_renderer::SkyMap, origin: Vec3, slice: usize) -> nfs_renderer::SkyMap {
        if let Some(r) = self.slices.as_ref().and_then(|t| t.slices.get(slice)) {
            map.u[3] += 0.9 * (origin - r.centre).dot(r.lateral) / 6.0;
        }
        map
    }

    pub(crate) fn rebuild_scene(&mut self) {
        // PORT-DEVIATION D64: the pad button icons for the pad connected now (before any screen draws them)
        self.pad_icons_refresh();
        // **TEST GLUE** (PORT-DEVIATION D63) `NFS_RACE_PANEL`
        self.panel_script_step();
        // PORT-DEVIATION D46: the camera slot's camera and slice as its own last pass left them (see `photo.rs`)
        self.photo_frame_begin();
        let (lw, lh) = self.hud_logical();
        Hud::set_screen(lw, lh);
        self.hud_stored_sync(lw, lh); // N01a: an existing HUD record is used as stored
        self.fix_mission_result();
        // the port's own overlays lay their text out in screen units, so the fonts need to know
        // what shape the window is (the HUD and the front end place glyphs in 640×480 themselves)
        let a = self.hud_aspect();
        if let Some(h) = self.hud.as_mut() {
            for f in [&mut h.font, &mut h.font_small, &mut h.font_italic].into_iter().flatten() {
                f.set_aspect(a);
            }
        }
        if let Some(f) = self.fe.menu.as_mut().and_then(|m| m.font.as_mut()) {
            f.set_aspect(a);
        }
        if let Some(w) = self.world.as_mut() {
            for a in self.world_actions.drain(..) {
                match a {
                    WorldAction::Show(all, shown) => w.set_shown(&all, &shown),
                }
            }
        }
        self.scene.items.clear();
        self.scene.lens_probe = None;
        self.scene.shafts = None;
        self.scene.dof = None;
        self.scene.drops = None; // ENHANCEMENT E40
        self.scene.sprites.clear();
        self.scene.mirror = self.mirror && self.vehicle.is_some();
        // the sky's own colour, from the track's `Sky/<Track>.bin` (`RE/RENDERING.md`): black on the
        // Monaco circuits, sunset on coastal, plain blue on forest. It shows wherever the sky layers
        // do not cover, and the layers are drawn over it.
        // The record is the `SKYRENDERING` level's (`[0x657D9C]`, the `Graphics.SkyRendering`
        // choice): 0x459970 addresses `bin + 0xCE8 + 60·level` and steps down past unused records
        // (0x459983…0x45998E). The port read level 0 whatever the option said.
        // the frame's options, read once (`RenderOptions`)
        let opts = RenderOptions::read(self);
        // `[0x627E1C]`, the record 0x459950 / `I_HORIZON` (0x4599E0 / 0x459A20) left (`Sky::colour_level`)
        let sky_level = self.sky.as_ref().map_or(opts.sky_level, |s| s.colour_level(opts.sky_level));
        // 0x459A70 at the race pass's start (0x468C63): the record's gradient flag sends the
        // colour to the sky manager's track B at the camera's slice, else track A blends the
        // record's top and bottom (`SkyBin::background_at`), group A — the main view
        let cam_slice = self.cam_slice.or(self.race.as_ref().map(|r| r.slice)).unwrap_or(0) as i32;
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        if self.vehicle.is_some()
            && let Some(c) = self.track.sky_bin.as_ref().and_then(|b| b.background_at(sky_level, 0, cam_slice, lap))
        {
            self.scene.clear_color = [c[0] as f64, c[1] as f64, c[2] as f64, 1.0];
        }
        // the weather fog the sky manager sets at the camera's slice: the sky pass's own (0x45D2E0)
        // and, as that pass ends, the scene's (0x45D3B0) over everything drawn after it
        // (`SkyBin::fog_at`); the front end has none
        // `nfs5.ini` `Fog=0` (dll 0x100035F0 over the Thrash setter 0x5752D5): states 0x0E / 0x0F / 0x16 / 0x17 are
        // dropped and 0x14 / 0x15 / 0x69 are sent as 0, so no fog is ever enabled
        let fog = |sky: bool| {
            if self.vehicle.is_none() || !self.ini.fog {
                return None;
            }
            let (colour, density) = self.track.sky_bin.as_ref()?.fog_at(sky, cam_slice, lap)?;
            // ENHANCEMENT E5: the scene's fog ends `reach` times farther (`exp(-d·density/reach)`, the
            // original's curve stretched), so the farther geometry still fades into the fog colour; the
            // sky pass's own fog (the backdrop, no distance) is left as it is
            let density = if sky { density } else { density / self.enh_reach() };
            Some(nfs_renderer::Fog { colour, density })
        };
        (self.scene.sky_fog, self.scene.fog) = (fog(true), fog(false));
        // ENHANCEMENT E9: the sun the cars are lit with (`-SkyBin::sun_position`, 0x43AC8D; `TrackWorld::sun_direction`)
        // casts the main race view's shadows; a night track has no sun to cast with, so none (the renderer
        // reads this only while E9 is on)
        // (a track whose sky configuration has no sun — the port's straight-down fallback, the overcast and
        // rainy tracks: autobahn, canyon, foothills, forest — has none either; canyon / forest have a data sun)
        // ENHANCEMENT E14: the corner shading is for the race's main view
        self.scene.ao = self.vehicle.is_some();
        // ENHANCEMENT E21: the eye adaptation is for the race's main view
        self.scene.eye = self.vehicle.is_some();
        // ENHANCEMENT E9 (ENH-TUNPOOL): the covered stretches, so the sun's shadow does not darken a tunnel's baked light by the camera's place
        // (ENHANCEMENT E50: on a night track too while the moonlight is on; SHADOW-OVERCAST: and the track has a moon)
        let moon_on = self.has_moon() && self.enh.is_on(nfs_renderer::E50);
        self.scene.cover = if self.vehicle.is_some() && (!self.track.is_night() || moon_on) && self.enh.is_on(nfs_renderer::E9) { self.cover_boxes() } else { Default::default() };
        // ENHANCEMENT E9 (SHADOWFIX): under cover (the enclosure test the car's light uses, 0x473330 flags & 0xC000) the sun
        // does not reach: the camera or the player's car in a tunnel / gallery fades the shadows out
        // SHADOW-REAL: that was only the fallback of a track without covered boxes; with them the shadow is decided per surface
        // (`cover_at`: it ends at the tunnel's mouth in space, the sun shadow no longer comes in late on a tunnel exit), and the
        // gate only switches the pass off while the camera is deep inside a covered run (`cover_open_view`)
        let lit = if self.scene.cover.is_empty() {
            self.world.as_ref().is_none_or(|w| w.overhead_flags(self.camera.position) & 0xC000 == 0 && self.vehicle.as_ref().is_none_or(|v| w.overhead_flags(v.position) & 0xC000 == 0))
        } else {
            super::cover::cover_open_view(&self.scene.cover, self.camera.position)
        };
        self.scene.shadows = self.shadow_lights(lit, moon_on);
        // ENHANCEMENT E17: what the water reflects without an environment map (the sky pass's clear colour: the sky
        // the player sees at the horizon is that pale colour; the scene fog's colour is a saturated tone that only
        // tints distant geometry, so it is not used) and the sun that glints on it (the same sun as E9's shadows and the lens flare;
        // none on a night track); the renderer reads it only while E17 is on
        self.scene.water = self.vehicle.is_some().then(|| {
            let zenith = [self.scene.clear_color[0] as f32, self.scene.clear_color[1] as f32, self.scene.clear_color[2] as f32];
            nfs_renderer::WaterEnv { zenith, horizon: zenith, sun: self.scene.shadows.sun.map_or(Vec3::ZERO, |s| -s.direction), use_env_map: self.track.is_night() }
        });
        if self.vehicle.is_none() && self.fe.menu.is_some() {
            self.fe_scene();
            return;
        }
        let (hud_sprites_from, mut hud_hover) = self.hud_pass();
        // render pose = previous tick → current tick, by the accumulator fraction
        let mut alpha = (self.pace.accumulator.as_secs_f32() / GAME_TICK.as_secs_f32()).clamp(0.0, 1.0);
        // **TEST GLUE** (not in the original): `test_alpha` fixes the interpolation fraction, so a scripted run (one frame per tick,
        // where the accumulator reads 0) can draw the frames a windowed run draws between two ticks
        if let Some(a) = self.glue.test_alpha {
            // (`NFS_TEMPORAL_SEQ` extrapolates over several ticks: its frames pass fractions above 1)
            alpha = a.clamp(0.0, 64.0);
        }
        // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: every moving thing is drawn at `lerp(previous
        // tick, this tick, alpha)` (cars, camera, shadows, smackables, `WAnimBundle` articles); the
        // original draws this tick's pose (alpha 1) — see `RE/notes/PORT-DEVIATIONS.md` D8
        let (car_pos, car_rot) = Self::blend_pose(self.render_prev.as_ref(), self.car_pos, self.car_rot, alpha);
        // `self.car_pos` is where the body is drawn, `car+0x330 − M[1]·car+0x3AC` (0x43A410 at
        // 0x4434D7); the cameras (0x461903), the headlight projector (0x452749), the beacon and
        // the shadow (0x43B220) hang off `car+0x330` itself, half the model box higher
        let car_draw = car_pos;
        // the viewed car's own lift (a network replay may view another human's car)
        let viewed_seat = Self::viewed_seat_of(self.replay_net.as_ref(), self.replay_view);
        let viewed_lift = match viewed_seat {
            Some(k) => self.road.opponents.get(k).map_or(0.0, |o| o.vehicle.body_lift),
            None => self.vehicle.as_ref().map_or(0.0, |v| v.body_lift),
        };
        let car_pos = car_pos + car_rot * Vec3::Y * viewed_lift;
        // the recorded machine's own car, slot 0 of the draw: where it is, whichever car the camera views
        // (`car_pos` / `car_rot` are the camera's car's)
        let (local_draw, local_rot) = if viewed_seat.is_some() {
            Self::blend_pose(self.local_prev.as_ref(), self.local_pose.0, self.local_pose.1, alpha)
        } else {
            (car_draw, car_rot)
        };
        let local_pos = local_draw + local_rot * Vec3::Y * self.vehicle.as_ref().map_or(0.0, |v| v.body_lift);
        self.headlight_projector(car_pos, car_rot, local_draw);
        let rear_view = self.input.held(Action::RearView)
            && self.vehicle.is_some();
        self.refl.rear_mirror = self.rear_mirror_camera(car_pos, car_rot);
        self.horn_frame();
        // the port's dialogs over the race, after the HUD's own draw
        self.replay_save_overlay(&mut hud_hover);
        self.pause_overlay(&mut hud_hover);
        self.net_notice_overlay();
        // every string still pending is an unanchored element's: its pen is its own pixel
        Hud::resolve_text(&mut self.scene.sprites[hud_sprites_from..], (0.0, 0.0));
        PENDING_TEXT.with(|p| p.borrow_mut().clear());
        if self.hud_hidden || self.photo.hides_hud() {
            self.scene.sprites.truncate(hud_sprites_from);
        }
        // PORT-DEVIATION D46: photo mode's hint, and the pointer to it on the pause menu / in a replay
        self.photo_hint();
        // PORT-DEVIATION D63: the race panel and the A/B key's (`0`) message, after the hint (a photo-mode still cuts the sprites off there)
        self.panel_overlay();
        // `+0x100` for the next release, and sound 9 when the pointer crosses into a button
        // (0x41B3BA…0x41B3C6)
        let hover = hud_hover.flatten();
        if hover.is_some() && hover != self.hud_state.hud_hover {
            self.fe_sound(9);
        }
        self.hud_state.hud_hover = hover;
        let (view, slot_near) = self.camera_pass(car_pos, car_rot, rear_view, false, alpha);
        // PORT-DEVIATION D46: the free camera draws instead of the slot's (which is kept for the next pass)
        self.photo_apply();
        // draw distance from `<track>_{forward,backwards}.key` for the camera's slice, scaled by the
        // detail setting (0x44F1A0: `value × {0.5, 0.67, 0.83, 1.0}`, at least 50 m). The sky is drawn
        // around the camera, so distant terrain simply stops at this range as in the original.
        let keys = if self.backwards {
            &self.track.draw_keys_backward
        } else {
            &self.track.draw_keys_forward
        };
        // 0x4689C0 keys on the camera slot's own tracker slice `[0x627F5C]`, which the dispatcher moves
        // with the eye every update (0x474440 at 0x463C61, except in SplineCam, which runs it itself)
        let car_slice = self.race.as_ref().map(|r| r.slice).unwrap_or(0);
        let cam_slice = match self.slices.as_ref() {
            Some(t) if !t.slices.is_empty() => t.find_slice(self.camera.position, self.cam_slice.unwrap_or(car_slice)),
            _ => car_slice,
        };
        self.cam_slice = Some(cam_slice);
        // the near plane: the camera object's 0.15 (0x4677F5) is only the main view's creation value
        // (0x468E6A) — the build 0x464F80 overwrites the view's `+0x18` every frame with the slot's
        // `+0x5C`, less 0.35 in view 1 (0x465413…0x465439): 0.5, the driver view 0.15, a TV zoom
        // further out (`camera::near_plane`). The far plane is the camera object's 700 (0x4677FC,
        // `RE/notes/thrash.md` §6); the draw distance `[0x6197CC]` is the walk's (the view's
        // `+0x124`: the footprint and the non-mid-ground articles' distance rule, `RE/RENDERING.md`
        // "The track draw's culling"), not a clip plane. *(Until 2026-09-26 the port clipped at the
        // draw distance, cutting the mid-ground's 600 / 700 m short.)*
        self.camera.near = nfs_game::camera::near_plane(slot_near, view);
        // 0x44F1A0 (from the frame's 0x468A63): the factor is 1.0 instead of the option's table entry
        // while a replay plays with the main slot zoomed in (`replay_zoom`: `[0x627ED8]` under the
        // dll's cell, 0x44F1A9…0x44F1BC)
        let factor = if self.replay_zoom() == Some(true) { 1.0 } else { self.detail };
        self.draw_distance = keys.far_distance(cam_slice, factor);
        // ENHANCEMENT E5: the far plane scaled by the draw-distance factor (1.0 = off = 700)
        self.camera.far = if self.vehicle.is_some() { nfs_renderer::VIEW_FAR * self.enh_reach() } else { FE_FAR };
        // ENHANCEMENTS E27 / E29: this frame's motion blur and shake (the camera above is left as it is)
        if self.photo.active {
            // PORT-DEVIATION D46: the free camera replaces the render camera: no shake, no motion blur (E27 / E29); its lens below
            self.camfx.view = None;
            self.scene.motion = None;
        } else {
            self.camfx_frame(car_pos, car_rot, alpha, view, a);
        }
        // ENHANCEMENTS E7 / E31: the photo mode's lens (its depth of field), the replay's lens
        self.photo_scene_fx();
        self.main_sky_items(cam_slice, opts.sky_off);
        self.camera_diagnostics();
        // the main view's frustum: the track walk's and the cars' (0x4430A0's test)
        let cull = self.main_cull_view();
        self.track_walk(alpha, &cull);
        self.ambient.rain.lamps.borrow_mut().clear();
        // ENHANCEMENT E29: in the bumper / driver views the car is rigid with the camera: it is carried with the shaken
        // camera, so the cockpit stays where it is on the screen while the world shakes (parallax)
        let (local_draw, local_pos, local_rot) = match self.camfx.view {
            Some(sv) if view <= 1 => {
                let (d, r) = sv.attach(local_draw, local_rot);
                (d, sv.attach(local_pos, local_rot).0, r)
            }
            _ => (local_draw, local_pos, local_rot),
        };
        self.track_and_car_items(opts, cull, alpha, (local_draw, local_pos, local_rot));
        self.rain_frame();
        // ENHANCEMENT E36: the air of the view
        self.haze_frame();
        // ENHANCEMENT E44: the lighthouse beam
        self.lbeam_frame();
    }

    /// **The rear-view mirror** (AUDIT §O): the pass 0x468CA0 renders the view 0x4691D0 made — near
    /// 1, far 65000 (the projection; the scene stops at the cull distance below), cleared 0xFF77BBFF — with camera slot 0 in its type-1 form (0x464F80's branch at
    /// 0x46503B): the eye at the car's position raised by the current view's height (`[0x5CF614 +
    /// 16·view]`, y only), the car's matrix with its right and forward rows negated — a half turn
    /// about the car's up axis; the left-right reflection comes after, in 0x46537B (`Self::render_mirror`). Skipped in a replay (`[0x657424]` 2/3) and
    /// unless the tray is in (0x4275A0); `REARVIEWMIRRROR` gates the tray (0x41E9A0). The field of
    /// view is the mirror view's own `+0x130`, 45.0 from its constructor (0x4673D5,
    /// `camera::VIEW_HALF_ANGLE`): the build 0x464F80 writes slot 0's `+0x58` / `+0x5C` into the
    /// scene manager's first view `[[0x628140] + 4]` (0x464F91), the main one, even for this type-1
    /// build — so no zoom and no wide-screen ×1.2 here. The projection 0x5391B0 takes `2·+0x130`
    /// as the **horizontal** angle with the viewport's aspect in 640 × 480 units (0x466FD7…
    /// 0x46700C): 90° across the 240 × 84 tray, `2·atan(84/240)` high. The pass's view distances (read
    /// 2026-09-24, 0x468D3B…0x468D82): `+0x120` = 200 (the main view's 0x468BF8 has 400), `+0x124` =
    /// `max([0x6197CC]/3, 50)` (`[0x5B2680]`, `[0x5B251C]`; the main view the whole draw distance)
    /// and `+0x128` its square. The one reader of the view's `+0x124` is 0x47FB57 → 0x47F550, which builds
    /// the view's footprint on the ground out to that distance (`+0xA0` = its square) to pick the track
    /// sections to draw — the draw distance, which the mirror's own walk takes (`render_mirror`);
    /// its far plane is the view's own 65000 (0x4691D0). (0x499D9C / 0x49AEE0 / 0x49C96A read
    /// `+0x124` off `car+0x760`, another struct.) The 0x537890 state is not carried.
    /// The pass's gates, read 2026-09-27 (0x468CA0…0x468CD1): `REPLAY` neither 2 nor 3, 0x4275A0
    /// (`[0x60C970]` and the mirror element in state 3 — REARVIEWMIRRROR opens it,
    /// `mirror_slide`) and the view `[0x628144]` made (its one writer, 0x469225, is the view's
    /// creation). *(Until 2026-09-27 the port tested `Playing` only and stopped the view at the
    /// player's finish, which nothing in the pass or its gates does.)*
    fn rear_mirror_camera(&self, car_pos: Vec3, car_rot: Quat) -> Option<Camera> {
        let playback_now = self.playback_running();
        let fwd = car_rot * Vec3::Z;
        (self.hud_state.hud_keys.live
            && self.hud_state.slides.mirror.0 == 3
            && self.vehicle.is_some()
            && !playback_now
            && self.race.is_some())
        .then(|| {
            let h = nfs_game::camera::VIEW_OFFSETS.get(self.camera_view() as usize).map_or(0.0, |o| o.y);
            let position = car_pos + Vec3::new(0.0, h, 0.0);
            Camera { position, target: position - fwd * 20.0, up: car_rot * Vec3::Y, fov_y_radians: nfs_game::camera::fov_y_viewport(nfs_game::camera::VIEW_HALF_ANGLE, super::mirror::MIRROR_TRAY.0, super::mirror::MIRROR_TRAY.1), near: 1.0, far: super::mirror::MIRROR_FAR }
        })
    }

    /// The horn's frame (`engine-audio.md` §6): its loop on the car's own engine bank, at the camera's distance and pan.
    fn horn_frame(&mut self) {
        // the horn is a loop on the car's own engine bank (`engine-audio.md` §6)
        let horn_view = self.camera_view();
        let to_car = self.car_pos - self.camera.position;
        let horn_dist = to_car.length();
        // the pan is 0x4A7660's `ebp` (0x4A7CAB → `Horn_start`'s fourth argument → `Voice_play`'s
        // pan, 0x4AACD8): 0 on view 0 (0x4A7855) and negated under `MIRROR` (0x4A78C1) — the port
        // had neither until 2026-09-26
        let horn_pan = if horn_view == 0 { 0 } else { listener_pan_of(self.camera.position, self.camera.target, self.mirror)(to_car) };
        let sfx_level = self.snd.audio_settings.sfx_level;
        // the horn is 0x4A7660's (0x4A7CBF / 0x4A7CE6), part of the audio update that does not run
        // while paused: a held horn is not released by the pause but silenced with the voice table
        // (0x4AB150), and plays on at volume 0 after the resume until it is let go
        let horn_now = self.live_horn();
        if let (Some(a), Some(e)) = (&self.snd.audio, self.snd.engine_sound.as_mut())
            && !(self.paused || self.replay.frozen)
        {
            e.horn(
                a,
                horn_now && self.vehicle.is_some(),
                horn_view,
                horn_dist,
                horn_pan,
                sfx_level,
            );
        }
    }

    /// The front end's scene: its room and the screens' sprites over a black clear (the loading / start-up stills in place of
    /// them), the A/B key's message last.
    fn fe_scene(&mut self) {
        self.scene.clear_color = [0.0, 0.0, 0.0, 1.0];
        self.load_fe_room();
        self.fe_room_items();
        // a screen with a room behind it must not paint its own black fill over it
        let room = !self.fe.room.is_empty() || !self.fe.cars.is_empty();
        if let Some(l) = self.fe.layout_preview.as_mut() {
            l.set_overlay(room);
        }
        // the background drawn is the bottom layer's (`fe_sprites`), so its fill
        // gives way to the room the same way
        if let Some((_, u)) = self.fe.layout_under.first_mut() {
            u.set_overlay(room);
        }
        self.scene.sprites = self.fe_sprites();
        if let Some(step) = self.fe.loading_still {
            self.scene.items.clear();
            self.scene.sprites = self.fe_loading_sprites(step);
        }
        if let Some(l) = self.fe.startup_still.as_ref() {
            let aspect = self.renderer.as_ref().map(|r| r.aspect()).unwrap_or(1.6);
            self.scene.items.clear();
            self.scene.sprites = l.mosaic_sprites(aspect);
        }
        if let Some(step) = self.fe.race_loading_still
            && let Some(mut r) = self.renderer.take()
        {
            for k in 0..=step.clamp(0, 9) {
                self.race_loading_step(&mut r, k);
            }
            self.scene.items.clear();
            let (lw, lh) = self.hud_logical();
            self.scene.sprites = self.fe.race_loading.as_ref().map(|l| l.sprites(lw as f32, lh as f32)).unwrap_or_default();
            self.renderer = Some(r);
        }
        // PORT-DEVIATION D63: the A/B key's message, over every screen of the front end (the panel is a race's)
        self.panel_overlay();
    }

    /// The sky manager's update 0x45D030 for the main view and its draw items (the sky, the sun,
    /// the ring): the first range of `scene.items` (`sky_item_range`).
    fn main_sky_items(&mut self, cam_slice: usize, sky_off: bool) {
        // `[0x657DB4]` SKYOFF (the config row `SKYOFF`, id 0x5A, 0x4B14C4 — `fe.txt`'s `skyoff`, absent
        // in the shipped one): 0x468AF9 skips the whole sky pass 0x45D7D0, update and draw, for every
        // view; `[0x619799]`, the third gate there, is only ever written 0 (0x44E89D)
        if sky_off {
            let n = self.scene.items.len();
            self.sky_item_range = (n, n);
            return;
        }
        // the sky manager's update (0x45D030): the camera's eye and slice drive the layers
        if let (Some(s), Some(r)) = (self.sky.as_mut(), self.renderer.as_mut()) {
            let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
            s.update(r, self.camera.position, cam_slice as i32, lap, self.tick, &mut self.fe_rng);
        }
        if let Some(s) = &self.sky {
            // in metres around the eye, clipped by the far plane 700 as the sky pass 0x45D4F0 does
            // (`Sky::draw_items`); it draws first without a depth test, so the mid-ground shows over
            // it — the levels that let the sun through, the sun, then the rest and the ring
            let aspect = self.renderer.as_ref().map_or(1.6, |r| r.aspect());
            // ENHANCEMENT E29: the sun is laid out on the camera that is drawn (the shaken one), so it sits where the sky pass puts it
            let sun = self.sun_items(&self.render_camera(), aspect, SUN_VIEW_MAIN);
            self.scene.shafts = self.light_shafts(aspect);
            let view = nfs_game::sky::SkyView { eye: self.camera.position, forward: (self.camera.target - self.camera.position).normalize_or_zero(), up: self.camera.up, main: true };
            let start = self.scene.items.len();
            self.scene.items.extend(s.draw_items(&view, false));
            self.scene.items.extend(sun);
            self.scene.items.extend(s.draw_items(&view, true));
            self.sky_item_range = (start, self.scene.items.len());
        }
    }

    /// `[0x657DB4]`, `SKYOFF`: set from the config (`fe.txt`), 0 as shipped.
    pub(crate) fn sky_off(&self) -> bool {
        self.registry.u32(SKYOFF_KEY, 0) != 0
    }

    /// `CARDETAIL` `[0x6197C4]`: the `Graphics.CarDetail` choice's index (0 without one).
    pub(crate) fn car_detail(&self) -> u8 {
        self.choice_index(choice::GRAPHICS_CAR_DETAIL).map_or(0, |(i, _)| i as u8)
    }

    /// The port's `NFS_CAM_AT` camera diagnostic (no original counterpart): it moves the camera before
    /// the track walk culls from it.
    fn camera_diagnostics(&mut self) {
        // diagnostic: `NFS_CAM_AT=px,py,pz,tx,ty,tz` puts the camera at a world point looking at
        // another (before / after comparisons at a fixed place)
        if let Some(v) = nfs_game::config::cfg().nfs_cam_at.as_str() {
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if n.len() == 3 {
                // three numbers = move the camera (target with it) to this point, keeping its aim
                // (with NFS_SUN_VIEW: the sun seen from another place)
                let d = Vec3::new(n[0], n[1], n[2]) - self.camera.position;
                self.camera.position += d;
                self.camera.target += d;
            }
            if n.len() == 6 {
                self.camera.position = Vec3::new(n[0], n[1], n[2]);
                self.camera.target = Vec3::new(n[3], n[4], n[5]);
            }
        }
    }

    /// The main view's track walk 0x47F890 and what the walk's draws feed (smackables, projectors,
    /// the kind-3 emitters): `article_draws` for this frame.
    fn track_walk(&mut self, alpha: f32, view: &nfs_game::world::CullView) {
        if let Some(w) = self.world.as_mut() {
            // each smackable's article is drawn at its body's pose (0x483590, `skill-tests.md` §5.3)
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: at the pose blended by `alpha` between the
            // previous tick and this one, like every car (the original draws this tick's, alpha 1)
            for (owner, pose) in self.smackables.poses_blended(alpha) {
                w.place_smackable(owner, pose);
            }
            // the track draw's prep 0x488CC0 → 0x476810: a smackable a draw found pushed gets its
            // record's `+8` (`suspension-collision.md` §9 "The track's smackables")
            w.smackables_prep();
            // the looping props (`RE/notes/crp-geometry.md` §7) walk on the race clock
            // 0x488CC0 heads every walk (0x47FB49) and counts them in `[0x628C4C]`, never reset: the
            // eighth call re-registers the moving refs; the main view's walk is one
            // count, the mirror's (`render_mirror`) the next
            self.walk_calls = self.walk_calls.wrapping_add(1);
            // PORT-DEVIATION D8 (PORT-DEVIATIONS.md) [the original draws the last tick, 0x483590 / 0x470E90]: the race time of the drawn pose — this tick's
            // `tick / 64` at `alpha` 1 (all the original draws), a fraction of a tick earlier
            // for a frame between two ticks, as the cars are blended; this tick's time at tick 0
            w.animate((self.tick as f32 - 1.0 + alpha).max(0.0).min(self.tick as f32) / TICKS_PER_SEC_F32, self.walk_calls & 7 == 0);
            // water/flag UV scroll (`RCustomLayers::LayerUVAnimate`, the EAGL
            // layers): `TIMER_getTicks()` is two of this 64 Hz race tick, as the beacon uses below
            w.animate_uv((self.tick * 2) as u32);
            // ENHANCEMENT E32: the track's parts on their bounce-lit copies (or back on the originals)
            if let Some(r) = self.renderer.as_mut() {
                w.gi_update(r, self.enh.level(nfs_renderer::E32));
            }
            // the breakable props' pieces (`RE/notes/suspension-collision.md` §9)
            w.break_props(self.tick as u32);
            // the track walk 0x47F890 over the articles that carry `ef` parts (`RE/RENDERING.md`
            // "The track draw's culling") in `view` ([`Self::main_cull_view`]); the projectors of
            // the drawn articles go into the map `[0x627DFC]` for good (0x450DBD → 0x456090)
            w.article_draws_into(view, &mut self.draws.article_draws);
            w.register_projectors(&self.draws.article_draws);
            // 0x450010's entry loop samples each drawn smackable's player (0x4711E0)
            w.smackables_sample(&self.draws.article_draws);
            // and kind 3: each drawn emitter's system made or kept alive with its article's value,
            // and triggered where 0x450980 runs it (a struck item's)
            if let Some(p) = self.track_particles.as_mut() {
                p.draw(self.draw_distance, |e| w.effect_run(&self.draws.article_draws, e.article, e.part, 3));
            }
        }
    }

    /// The main view's frustum: the eye, the view's axes, the half-angles' tangents of the port's own frustum, its near plane and
    /// the draw distance — what the track walk 0x47F890 culls with and 0x4430A0's test 0x539480 takes for every car but the
    /// camera's own (0x44311F). ENHANCEMENT E29: the frustum of the UNSHAKEN camera, widened by the shake's angle so the
    /// shaken view shows no hole; E5: the draw distance times the reach.
    fn main_cull_view(&self) -> nfs_game::world::CullView {
        let reach = self.enh_reach();
        let forward = (self.camera.target - self.camera.position).normalize_or_zero();
        let right = forward.cross(self.camera.up).normalize_or_zero();
        let tan_v = self.camfx.widen((self.camera.fov_y_radians * 0.5).tan());
        nfs_game::world::CullView {
            eye: self.camera.position,
            forward,
            right,
            up: right.cross(forward).normalize_or_zero(),
            tan_h: self.camfx.widen((self.camera.fov_y_radians * 0.5).tan() * self.renderer.as_ref().map_or(4.0 / 3.0, |r| r.aspect())),
            tan_v,
            near: self.camera.near,
            draw: self.draw_distance * reach,
            reach,
        }
    }

    /// The track's near list, the cars and the skid marks with the track's far list in one
    /// farthest-first run, then the effects and the weather: the rest of `scene.items`.
    fn track_and_car_items(&mut self, opts: RenderOptions, cull: nfs_game::world::CullView, alpha: f32, (local_draw, local_pos, local_rot): (Vec3, Vec3, Quat)) {
        // The track's near list (`+4`, flag-2 geometry, nearest first) and its own `+0xC` entries
        // (flag-1 / flag-4 parts, instance items, a breakable's pieces) are kept apart here now: the
        // original's `+0xC` also holds the cars (0x4430A0 → 0x443229) and the skid marks (0x44FFC0),
        // one distance-sorted list flushed farthest first (0x466660, `RE/RENDERING.md` "The view's
        // lists"), where the port drew three separate blocks until this reading
        // (`RE/notes/thrash.md` §6d.6). `track_far` and `track_rest` are folded into `blended`
        // below, once the cars' and the skid marks' own keyed entries have joined it, and pushed as
        // one farthest-first run after `track_near`. The mirror's pass builds its own cars' and skid
        // marks' entries for its eye (`Self::dyn_items`, called from `Self::render_mirror`).
        // (the lists, the sort's pairs and the merge's buffer are `self.scratch`'s, kept between frames; the
        // near list is sorted as `(key, index)` pairs — a stable ascending order, the index breaking ties —
        // and only then are the items copied out, instead of moving the whole items in a stable sort)
        let mut sc = std::mem::take(&mut self.scratch);
        let track_range_start = self.scene.items.len();
        if let Some(w) = &self.world {
            w.track_draw_lists_into(&self.draws.article_draws, self.camera.position, &mut sc.lists);
            nfs_renderer::sort_order(&sc.lists.near, false, &mut sc.order);
            let near = &sc.lists.near;
            self.scene.items.extend(sc.order.iter().map(|&(_, i)| nfs_renderer::DrawItem { early: true, layer_light: Some(nfs_game::world::TRACK_LIGHT), ..near[i as usize].1 }));
            tracing::trace!(articles = self.draws.article_draws.iter().flatten().count(), items = self.scene.items.len() - track_range_start, draw = self.draw_distance, "track walk");
            self.scene.ambient = w.ambient;
        } else {
            (sc.lists.near.clear(), sc.lists.far.clear(), sc.lists.rest.clear());
        }
        // The cars, in the order of the original's car list `[0x6115F0]`, which 0x443610 walks at
        // 0x468B2F (through `+0x474`) calling each `R3DCar`'s draw 0x4430A0: its shadow (0x437AE2 →
        // 0x43B820), its body and then its effect pass 0x43D800 — the lamps' glares and beams
        // (`lamp_draws`). The list is built by `R3DObject`'s constructor 0x4414C0, which puts every
        // new car at its **head** (0x4416E0…0x4416EB, the old head its `+0x474`), and the cars are
        // made in slot order: `Car_installHandlers` 0x410D60 builds the `R3DCar` (0x410F08) and is
        // called for `0x5E8B34[i]`, `i < [0x658080]` (0x41127C…0x41135C), whose order is
        // `Cars_create`'s over the racer slots (0x411130, `Car_register` 0x40F160 appending each),
        // and `Race_setCars` 0x4ECF00 lays the slots out as the player, the opponents, the cops,
        // then the traffic (`modes.md` §1). So the list — and the draw — runs the other way: the
        // traffic from its last slot, the cops, the opponents from the last, the player last. (The
        // world props that follow, 0x411370…, carry `+4 == 1` and return at 0x410EB6 before any
        // `R3DCar` is made.) MEDIUM: each of the port's traffic and police vectors is taken to be in
        // its slots' order. *(Until 2026-09-26 the port drew police, traffic, opponents, player,
        // and laid every shadow after the AI cars' lamps.)*
        //
        // The headlights' ramp runs from the race's first tick: the flag `+0x567` is taken as clear
        // when the car is built (its constructor is not read), so the first lamp update at night
        // starts it from 0. It runs on `GetTickCount` (0x53BFD0), which a pause does not stop —
        // the port's wall clock `tick + pause_offset + pause_ticks` (64 Hz units) does the same
        // *(until 2026-09-26 the world tick, which a pause halts)*; every car's first lamp update
        // is the same frame, so the traffic and the police share the race's ramp. Only a car with
        // pop-up headlights takes it (`CarModel::has_headlight_extent`, 0x439627): with
        // `R3DCar+0x70` = 0 the slots are 1.0 from the first frame (0x43972D) *(until 2026-09-26
        // every car faded in)*
        // the main pass takes 0x4430A0's frustum test 0x539480 on every car but the camera's own
        // (0x44311F): a car outside the view queues nothing — no shadow, no lamps, no effect drive
        // (the walk's own frustum, `main_cull_view`)
        let dyn_pose = DynPose { opts, alpha, local_draw, local_pos, local_rot, pass: PassKind::Main, cull: Some(cull), lod: self.renderer.as_ref().map(|r| { let (w, h) = r.size(); (w.max(1) as f32, h.max(1) as f32) }).unwrap_or((640.0, 480.0)), ref_width: self.renderer.as_ref().map_or(640.0, |r| r.size().0.max(1) as f32) };
        let mut pools = PoolUse::default();
        // the merge's list: the track's far list (flag-1 / flag-4 parts, instance items, a breakable's pieces) first, the cars' and
        // the skid marks' own `+0xC` entries appended to it by `dyn_items` (no copy of them in between)
        let mut blended = std::mem::take(&mut self.blended_dyn_items);
        blended.clear();
        blended.extend(sc.lists.far.iter().map(|&(k, i)| (k, nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i })));
        self.scratch = sc;
        self.dyn_items(&dyn_pose, &mut pools, &mut blended);
        let mut sc = std::mem::take(&mut self.scratch);
        self.pool_used = pools;
        self.dyn_pose = Some(dyn_pose);
        {
            let fwd = (self.camera.target - self.camera.position).normalize_or_zero();
            let right = fwd.cross(self.camera.up).normalize_or_zero();
            let up = right.cross(fwd).normalize_or_zero();
            // the merge: the track's far list with the cars' and the skid marks' own `+0xC` entries, one
            // farthest-first run (`nfs_renderer::farthest_first`, stable on equal keys, as the
            // original's tree insert is — 0x466933 / 0x466AD3). Then the port's no-`Base`
            // fallback, as before. `RE/RENDERING.md` "The view's lists"; `RE/notes/thrash.md`
            // §6d.6 — until now the port drew these three kinds in separate blocks instead of
            // this one list.
            nfs_renderer::farthest_first_into(&blended, &mut sc.order, &mut self.scene.items);
            self.blended_dyn_items = blended;
            self.scene.items.extend(sc.lists.rest.iter().map(|&i| nfs_renderer::DrawItem { layer_light: Some(nfs_game::world::TRACK_LIGHT), ..i }));
            self.track_item_range = (track_range_start, self.scene.items.len());
            // the flares, the particles and the streaks: this view's own entries (the mirror's pass
            // builds its own, `Self::effect_items` from `Self::render_mirror`)
            let effects_start = self.scene.items.len();
            let draws = std::mem::take(&mut self.draws.article_draws);
            self.effect_items(fwd, right, up, &draws, true, &mut sc.effects);
            self.draws.article_draws = draws;
            self.scene.items.extend(sc.effects.drain(..));
            self.effect_item_range = (effects_start, self.scene.items.len());
            self.weather_items();
            // the sun's lens flare (0x45D25C): last, after the world and the cars, as 0x468B89 runs it;
            // SKYOFF skips that pass too (0x468B80)
            if !opts.sky_off {
                self.lens_flare_items();
            }
            // ENHANCEMENT E9 (SHADOWFIX): casters from a sphere round the camera, not the view's walk (which drops what is
            // outside the frustum or behind an occluder: shadows popped in at the screen edge and out with the occlusion);
            // `shadow_only` items, drawn into the shadow map alone, after everything else
            // (SUN-FULL: and the full frame along a sunless track's data sun)
            if self.scene.shadows.casts_scenery() && self.enh.is_on(nfs_renderer::E9) {
                if let Some(w) = &self.world {
                    w.shadow_casters(&self.draws.article_draws, self.camera.position, super::lights::SHADOW_CASTER_REACH, &mut self.scene.items);
                }
            }
        }
        self.scratch = sc;
    }
}
