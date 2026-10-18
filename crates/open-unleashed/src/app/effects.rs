//! Race effects: weather, streaks, splats, the sun and its flare, windscreen drops.

use crate::*;

impl App {
    /// The entries the scene draw 0x468AE0 makes after the track walk and the cars, for **one
    /// view** (`self.camera` is that view's camera, `self.view_far` its far plane): the track's
    /// flares (0x451940, from the walk's drawers — `draws` is that walk's), then the systems'
    /// particles (0x455260 the track's, 0x43F6E0 the cars': 0x454430's billboards and 0x454EC0's
    /// streaks). Each is built from the view's own eye and axes (`fwd`, `right`, `up`) with its
    /// own depth bias `round(zbias · cam[+0xC] / d²)`, `cam[+0xC]` the view's `far · near / (far −
    /// near)`, as the original's, whose camera `[0x628130]+0x10` is the pass's. *(Until
    /// 2026-09-30 the mirror's pass reused the main pass's entries: quads turned to the main
    /// view's axes and pulled toward the **main** eye — in a tunnel, a lamp every few metres, each
    /// pulled to the main near plane, so the glints sat beside the car and flew with it.)*
    /// The streaks' meshes hold world positions, the same in both passes, so the mirror rewrites the
    /// pool with what the main pass wrote and only the entries' bias differs.
    /// `main` also sets the alpha-test state the weather draw inherits (`particles_left_ref_one`)
    /// — the weather draw is the main view's alone.
    pub(crate) fn effect_items(&mut self, fwd: Vec3, right: Vec3, up: Vec3, draws: &[Option<nfs_game::world::ArticleDraw>], main: bool, out: &mut Vec<nfs_renderer::DrawItem>) {
        out.clear();
        self.flare_items(out, fwd, right, up, draws);
        if let Some(p) = self.track_particles.as_ref() {
            // 0x454430 → 0x44F6E0: a camera-facing billboard per particle, turned about the
            // view axis, under the definition's blend (`+0x28` → Thrash 0x38, above) and
            // depth bias: `round(zbias · cam[+0xC] / d²)` (0x45445C…0x454491), `d²` the
            // system's `+0x10` — its article's square distance to the camera, here the
            // emitter's — as Thrash state 0x18, `b · 2^-15` on the game's screen z `far/(far −
            // near) · (1 − near / Z)` (0x540CE0, `thrash.md` §6d.3, the view's own planes), carried
            // as `DrawItem::depth_bias`: with `cam[+0xC]` that z's slope, `zbias / 32768` metres of
            // view depth at the emitter — a positive bias moves the sprite away.
            // The `d²` is the system's `+0x10`, the value its article was last drawn with.
            // Then the cars' sets (0x43F6E0 at 0x468B6F, after 0x455260): the same draw, `d²`
            // the car's from its last drive
            let mut sprites = p.sprites();
            if main && nfs_game::config::cfg().nfs_skid_stats.is_set() {
                tracing::info!(drawn = sprites.len(), undrawn = p.undrawn_sprites(), "track particle sprites");
            }
            let mut streaks = Vec::new();
            if let Some(fx) = self.car_effects.as_ref() {
                let (sp, st) = fx.draw(|k| self.effect_car_position(k));
                sprites.extend(sp);
                streaks = st;
            }
            // 0x454430 sets Thrash 0x24 <- 1 when its system holds particles (`+0x20`,
            // 0x45443E…0x454456) — every system it draws, the track's (0x455260) and the
            // cars' (0x43F6E0), billboards (0x44F6E0 at 0x4547DF) and streaks (0x454EC0 at
            // 0x454844) alike, all under `GREATER 1` — and nothing after it in the scene draw
            // puts 10 back but the sun's pass (0x45D249): what the weather's draw inherits
            if main {
                self.particles_left_ref_one = p.any_particles() || self.car_effects.as_ref().is_some_and(|fx| fx.any_particles());
            }
            self.streak_items(out, &streaks);
            let z_near = self.camera.near;
            let far = self.view_far;
            for sp in sprites {
                let Some(&(mesh, material)) = self.track_particle_sprites.get(&(sp.material.clone(), sp.blend)) else { continue };
                let (s, c) = sp.degrees.to_radians().sin_cos();
                let (r, u) = (right * c + up * s, up * c - right * s);
                let d2 = sp.sqd.max(1e-6);
                // 0x45446B…0x454480: `fild`, `fmul`, `fdiv` on the x87 stack, `fstp` to a
                // float, `fistp` — round to nearest even
                let bias = ((sp.zbias as f64 * (far * z_near / (far - z_near)) as f64 / d2 as f64) as f32).round_ties_even();
                let (pos, size) = (sp.position, sp.size);
                let m = Mat4::from_cols((r * size).extend(0.0), (u * size).extend(0.0), right.cross(up).extend(0.0), pos.extend(1.0));
                let a = sp.argb;
                let tint = [((a >> 16) & 0xFF) as f32 / 255.0, ((a >> 8) & 0xFF) as f32 / 255.0, (a & 0xFF) as f32 / 255.0];
                out.push(nfs_renderer::DrawItem {
                    light: tint,
                    alpha: (a >> 24) as f32 / 255.0,
                    layer_light: Some(unlit(tint)),
                    alpha_test: true,
                    alpha_ref: 1,
                    depth_bias: bias as i32,
                    ..nfs_renderer::DrawItem::new(mesh, material, m)
                });
            }
        }
    }

    /// The streaks of 0x454EC0 (flag 4: the cars' sparks, `nfs_game::car_effects`): per particle
    /// the triangle `(head + (0, 0.025, 0), tail, head)` in its ramp colour on all three corners,
    /// untextured, unlit, under the definition's blend, with the draw's depth bias `round(zbias ·
    /// cam[+0xC] / d²)` (0x45446B…0x454491, as the billboards') on `DrawItem::depth_bias`; one
    /// draw per system (a run of equal blend, bias and `d²`), `STREAK_CAP` triangles each.
    fn streak_items(&mut self, out: &mut Vec<nfs_renderer::DrawItem>, streaks: &[nfs_game::track_particles::Streak]) {
        let Some((pool, materials)) = self.streak_draw.as_ref() else { return };
        let Some(r) = self.renderer.as_mut() else { return };
        let z_near = self.camera.near;
        let far = self.view_far;
        let mut used = 0;
        for run in streaks.chunk_by(|a, b| (a.blend, a.zbias, a.sqd) == (b.blend, b.zbias, b.sqd)) {
            for part in run.chunks(STREAK_CAP) {
                let Some(&mesh) = pool.get(used) else { return };
                used += 1;
                let first = part[0].head;
                let verts = &mut self.scratch.streak_verts;
                verts.clear();
                verts.resize(STREAK_CAP * 3, nfs_renderer::Vertex { position: first.into(), normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] });
                for (tri, st) in verts.chunks_mut(3).zip(part) {
                    let c = |sh: u32| ((st.argb >> sh) & 0xFF) as f32 / 255.0;
                    let color = [c(16), c(8), c(0), c(24)];
                    for (v, p) in tri.iter_mut().zip([st.head + Vec3::new(0.0, 0.025, 0.0), st.tail, st.head]) {
                        v.position = p.into();
                        v.color = color;
                    }
                }
                r.write_mesh_vertices(mesh, verts);
                let s = &part[0];
                let bias = ((s.zbias as f64 * (far * z_near / (far - z_near)) as f64 / s.sqd.max(1e-6) as f64) as f32).round_ties_even();
                // under the system's own `GREATER 1` (0x454456, set before 0x454844 calls this)
                out.push(nfs_renderer::DrawItem {
                    layer_light: Some(unlit([1.0; 3])),
                    depth_bias: bias as i32,
                    alpha_test: true,
                    alpha_ref: 1,
                    ..nfs_renderer::DrawItem::new(mesh, materials[(s.blend as usize).min(2)], Mat4::IDENTITY)
                });
            }
        }
    }

    /// 0x459020, the last call of the scene draw 0x468AE0: the weather's step and draw
    /// (`nfs_game::weather`, `RE/notes/particles.md` §7). With the HUD's dialog queue `[0x60C9C4]`
    /// up (0x426A60: a billboard or a results page) outside a replay (`[0x5E99F4] < 2`) both
    /// emitters step by one on every frame with no drift; otherwise by the ticks since the last
    /// frame. Drawn when either holds particles, rain first, from the camera-space triangles moved
    /// back into the world by this frame's camera (the original draws them with the camera put at
    /// the origin, 0x466220 / 0x466280) — additive, unlit, no depth writes, the view's depth test,
    /// no bias, and the alpha test the scene draw left (below).
    pub(crate) fn weather_items(&mut self) {
        if self.weather.is_none() {
            return;
        }
        let frozen = self.weather_frozen();
        let (snow, rain) = self.weather_strengths();
        let fwd = (self.camera.target - self.camera.position).normalize_or_zero();
        let view = nfs_game::weather::View::new(self.camera.position, fwd, self.camera.up);
        let tick = self.tick as u32;
        let (Some(w), Some(p)) = (self.weather.as_mut(), self.track_particles.as_mut()) else { return };
        if nfs_game::config::cfg().nfs_weather_view.as_str().is_some() {
            // `NFS_WEATHER_STEPS=<n>` (diagnostic) runs n one-tick frames instead of 40; a
            // `NFS_WEATHER_VIEW` that is not a slice number leaves the camera where it is
            let steps = nfs_game::config::cfg().nfs_weather_steps.as_str().and_then(|v| v.parse::<u32>().ok()).unwrap_or(40);
            for j in 0..steps {
                w.frame(tick.wrapping_add(j), snow, rain, view, false, p.rng_mut(), &mut self.fe_rng);
            }
        } else {
            w.frame(tick, snow, rain, view, frozen, p.rng_mut(), &mut self.fe_rng);
        }
        if !w.visible() {
            return;
        }
        // 0x459020 sets no alpha test of its own: it draws under the one the scene draw 0x468AE0
        // left — **10** from the sun's pass 0x45D1A0 (0x45D245…0x45D249) when the sun object is on
        // (`+0x140`, `SkyBin::sun_active`), called right before it for the main view (0x468B89);
        // else the particles' **1** (0x454456) when a track or car system drew (0x455260 at
        // 0x468B6A, 0x43F6E0 at 0x468B6F), else the list flush's 10 (0x466703; *corrected
        // 2026-09-27 from "the car draw's 0x443615", which runs before the flush). "Drew" is
        // 0x454430's own gate, the system's particle count `+0x20` (`any_particles`; until
        // 2026-09-30 "had a sprite or a streak", which misses a system whose sprites are off).
        let sun_on = self.track.sky_bin.as_ref().is_some_and(|b| b.sun_active());
        let alpha_ref = if !sun_on && self.particles_left_ref_one { 1 } else { 10 };
        let Some(r) = self.renderer.as_mut() else { return };
        for (k, e) in [(1, &w.rain), (0, &w.snow)] {
            let Some((mesh, material)) = self.weather_draws[k] else { continue };
            let tris = e.triangles();
            if tris.is_empty() {
                continue;
            }
            // past the frame's triangles every vertex sits on the first: no area
            let n = nfs_game::weather::CAPACITY * 3;
            let first = view.to_world(tris[0].0);
            let mut verts = vec![nfs_renderer::Vertex { position: first.into(), normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0; 4] }; n];
            for (v, (q, uv)) in verts.iter_mut().zip(tris) {
                v.position = view.to_world(*q).into();
                v.uv = *uv;
            }
            r.write_mesh_vertices(mesh, &verts);
            self.scene.items.push(nfs_renderer::DrawItem {
                layer_light: Some(unlit([1.0; 3])),
                main_view_only: true,
                alpha_test: true,
                alpha_ref,
                ..nfs_renderer::DrawItem::new(mesh, material, Mat4::IDENTITY)
            });
        }
    }

    /// 0x459020's first branch: the HUD's dialog queue up (0x426A60 — here a billboard or a results
    /// page on screen) outside a replay (`[0x5E99F4] < 2`).
    pub(crate) fn weather_frozen(&self) -> bool {
        let replaying = self.replay.stream.as_ref().is_some_and(|r| matches!(r.mode, nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted));
        let page_up = self.results.is_some() && self.result_pages().get(self.results_page as usize).is_some();
        (self.billboard_key != 0 || page_up) && !replaying
    }

    /// The player's car's `Wndo` corners as 0x43A490 hands them on: the loader 0x442200 appends
    /// every record the body style admits (the `BndP` filter, `carinfo[0x56C + 4·slot] == value`)
    /// at `car+0x280`, × 0.4 for `.TINYCAR`, no `y` shift; the first four, model frame, `w = 1`.
    fn windscreen_quad(&self) -> Option<[Vec3; 4]> {
        let recs = self.car.crp.window_points(&self.car.crp_data).ok()?;
        let style = nfs_assets::car::CarStyle::from_tpg(&self.car.tpg, self.car.style_index + 1);
        let scale = if self.cheat_bits & CHEAT_TINY != 0 { 0.4 } else { 1.0 };
        let pts: Vec<Vec3> = recs
            .iter()
            .filter(|r| style.article_enabled(r.style_slot as u8, r.style_value as u8))
            .flat_map(|r| r.points.iter().map(|p| Vec3::from(*p) * scale))
            .take(4)
            .collect();
        (pts.len() == 4).then(|| [pts[0], pts[1], pts[2], pts[3]])
    }

    /// The windscreen drops' draw (`particles.md` §7.5), from the player's car draw: 0x43A490 when
    /// the camera follows this car, it has four `Wndo` points and the view id is non-zero and at
    /// most 1 (0x465770(0), bit 3 of `[0x627F43]`); 0x459200 when drops are held and this is the
    /// main view (`+0x12C == 2`, not the mirror); 0x457E60 rebuilds the plane from the corners the
    /// first time and draws when the view id is 1. The triangles are in the model frame and go
    /// through the car's transform (0x443490: the `(−1, 1, −1)` flip, the rotation, the draw
    /// origin). Additive, unlit, the view's depth test, no depth writes.
    ///
    /// **The alpha test `GREATER 10`**: 0x457E60 sets none, and the car draw it runs in is not
    /// 0x443610's loop but the view's list flush — 0x4430A0 only queues the car (0x443205 →
    /// 0x443229, 0x466A90: list `+0xC`, the record's object `car + 0xC`, whose drawer 0x4432B0
    /// reaches slot 14 at 0x44344B / through 0x443490), and 0x466660 draws list `+0xC` right after
    /// its Thrash 0x24 ← 10 (0x466703). 0x43A490 draws the drops **before** the car's LODs
    /// (0x43A545, then 0x444E00 at 0x43A552), and every sub-draw in that list that changes the
    /// reference puts 10 back (0x43D506 / 0x43D7DB, 0x452011 / 0x452085). So the texel alpha
    /// (`SPLT`'s 0…239) times the drop's vertex alpha must pass `> 10`.
    pub(crate) fn splat_items(&mut self, car_rot: Quat, car_draw: Vec3) {
        if self.camera_view() != 1 || self.vehicle.is_none() {
            return;
        }
        let Some((mesh, material)) = self.splat_draw else { return };
        let Some(w) = self.weather.as_ref() else { return };
        if w.splat.count() == 0 {
            return;
        }
        // the corners only matter while the plane waits for them; a car without four never draws
        let quad = if w.splat.dirty() {
            let Some(q) = self.windscreen_quad() else { return };
            Some(q)
        } else {
            None
        };
        let Some(w) = self.weather.as_mut() else { return };
        if let Some(q) = quad {
            w.splat.attach(q);
        }
        let tris = w.splat.triangles();
        let n = nfs_game::weather::SPLAT_SLOTS * 3;
        // past the frame's triangles every vertex sits on the first: no area
        let first = tris[0].0;
        let mut verts = vec![nfs_renderer::Vertex { position: first.into(), normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [1.0, 1.0, 1.0, 0.0] }; n];
        for (v, (p, uv, argb)) in verts.iter_mut().zip(tris) {
            v.position = p.into();
            v.uv = uv;
            let c = |s: u32| ((argb >> s) & 0xFF) as f32 / 255.0;
            v.color = [c(16), c(8), c(0), c(24)];
        }
        let Some(r) = self.renderer.as_mut() else { return };
        r.write_mesh_vertices(mesh, &verts);
        let m = Mat4::from_rotation_translation(car_rot, car_draw) * nfs_game::world::MODEL_TO_BODY;
        self.scene.items.push(nfs_renderer::DrawItem {
            layer_light: Some(unlit([1.0; 3])),
            main_view_only: true,
            alpha_test: true,
            alpha_ref: 10,
            ..nfs_renderer::DrawItem::new(mesh, material, m)
        });
    }

    /// The sun, as the sky pass draws it between its two runs over the levels (0x45D6F0 →
    /// **0x460290**, `RE/notes/lighting.md` §7): only while the sun object is on (`+0x140`,
    /// `SkyBin::sun_active`) and `flare.dat` has the record `anus` (0x5CF52C, found by 0x452260
    /// and dropped by 0x452100 when both its textures are null), the point `+0x11C`
    /// (`SkyBin::sun_position`, around the eye) goes through the view's projection (0x539700 at
    /// 0x460306) and is drawn when it lands on the screen, `0 ≤ x < width`, `0 ≤ y < height`, with
    /// its screen z under 1 — in front of the far plane 700, not behind the eye (0x46030B…0x460367).
    /// Then **0x451710**, the flare record drawn flat on the screen: nothing while its blink is off
    /// (`FlareDef::lit`, no part phase); per layer with a texture a quad of `±size · 0.1 · W /
    /// virtW` by `±size · 0.1 · H / virtH` pixels (nfs5.dll's virtual screen, 640 × 480 up to 4:3) about the point, turned — flag 4 — by `(x + y) /
    /// 1440` of a turn of its screen position (0x45175C…0x451768, 0x45187E…0x4518DE: `x' = x cos +
    /// y sin`, `y' = y cos − x sin`, screen y down), UVs (0, 0) (1, 0) (1, 1) (0, 1) from the top
    /// left (`[0x61BC78]`, 0x4514F5…), the layer's ARGB on all four corners (alpha × 1.0, 0x460398),
    /// screen z 0 and `rhw` 1, additive. The port lays the quad in the view plane one metre ahead
    /// (its `W` is then the original's `1 / rhw` = 1 for the fog), the same size on screen. Its
    /// screen point leaves out the projection's `+0.01` pixel (0x539829).
    ///
    /// **The mirror** runs the same sky pass (0x468CA0 → 0x468AE0 → 0x45D7D0 → 0x45D6F6), so the
    /// sun is drawn there too, through the current view — the mirror's (`[[0x628130]+0x10]`,
    /// 0x4602EA…0x460306), whose planes the pass has set to 0.15 / 700 as the main one's
    /// (0x45D51D). The projection lands in the backbuffer's pixels inside the mirror's viewport
    /// (`+0x1F0…+0x1FC`, 0x53981D…0x539847), but the on-screen test is still the **window's**
    /// `[0x619784]` × `[0x619788]` (0x460321, 0x460343), and 0x451710's quad is sized by the
    /// window too — so the sun in the mirror is as big in pixels as in the main view, and a point
    /// just outside the mirror's rectangle still draws what of its quad the viewport keeps. `view`
    /// is the pass's viewport in the 640 × 480 frame: [`SUN_VIEW_MAIN`], [`SUN_VIEW_MIRROR`] (the
    /// tray's rectangle 0x4691D0 is handed, `RE/AUDIT.md` §O). The two passes share the object's
    /// `+0x12C` / `+0x18D` (the point, on screen), which only the lens flare 0x460460 reads — the
    /// main view's own (`lens_flare.rs`; the mirror's pass does not run it, 0x468B7B).
    pub(crate) fn sun_items(&self, cam: &Camera, aspect: f32, view: [f32; 4]) -> Vec<nfs_renderer::DrawItem> {
        let mut out = Vec::new();
        let Some(def) = self.sun_def() else { return out };
        if !def.lit(0, self.tick as u32) {
            return out;
        }
        let Some(pt) = self.sun_point(cam, aspect, view) else { return out };
        // one metre ahead — but the port's mirror camera keeps the view's near plane 1 (0x4691D0),
        // where the original's sky pass has put 0.15 / 700 on the view (0x45D51D) and the screen
        // quad needs no plane at all: there the quad goes to twice the near plane, and the fog
        // reads the W the original's quad has, 1 (`rhw` 1, 0x451710), from `DrawItem::fog_w`
        // *(until 2026-09-30 it read the quad's 2 m, ≤ 2 % of the fog mix)*.
        let ahead = if cam.near < 1.0 { 1.0 } else { cam.near * 2.0 };
        let centre = cam.position + pt.rel * (ahead / pt.depth);
        self.flare_quads(&mut out, cam, aspect, view, FlareQuad { def, at: (pt.px, pt.py), centre, ahead, scale: 1.0, sprites: &self.sun_sprites, overlay: false });
        out
    }

    /// The sun's `flare.dat` record `anus` (0x460290 at 0x4602B9) when the sun object is on and the
    /// record has a texture.
    fn sun_def(&self) -> Option<&nfs_formats::flare::FlareDef> {
        self.track.sky_bin.as_ref().filter(|b| b.sun_active())?;
        self.flare_defs.iter().find(|d| d.id == SUN_FLARE && d.textures.iter().any(|t| t.is_some()))
    }

    /// The view's basis as the flare quads lay it: forward, right (the data's frame is left-handed:
    /// `forward × up` points to the screen's **left**, the renderer flips clip x, `RE/notes/
    /// RENDERING.md` "Coordinate system"; under `MIRROR` it does not) and up.
    pub(crate) fn flare_basis(&self, cam: &Camera) -> (Vec3, Vec3, Vec3) {
        let fwd = (cam.target - cam.position).normalize_or_zero();
        let left = fwd.cross(cam.up).normalize_or_zero();
        let up = left.cross(fwd);
        (fwd, if self.scene.mirror { left } else { -left }, up)
    }

    /// 0x460290's projection of the sun point `+0x11C` (0x539700 at 0x460306) and its on-screen test
    /// (0x460310…0x460367, the byte `+0x18D` the sun's own draw reads): the point in the view's
    /// 640 × 480 frame, `None` when it is behind the eye or the far plane or off the **window**.
    pub(crate) fn sun_point(&self, cam: &Camera, aspect: f32, view: [f32; 4]) -> Option<SunPoint> {
        let pt = self.sun_point_any(cam, aspect, view)?;
        // screen z under 1: in front of the eye and of the far plane 700
        if pt.depth <= 0.0 || pt.depth >= nfs_renderer::VIEW_FAR {
            return None;
        }
        if !(0.0..640.0).contains(&pt.px) || !(0.0..480.0).contains(&pt.py) {
            return None;
        }
        Some(pt)
    }

    /// The same projection **without** the on-screen test: what 0x539700 leaves in `+0x12C` / `+0x130`
    /// (0x460306) whether or not the point is in the window — the lens flare 0x460460 reads it with
    /// `nfs5.dll` having removed its `+0x18D` gate (0x46049A, `RE/notes/nfs5-dll.md` §2.4). Behind the
    /// eye the divide by the negative depth flips the point, as the original's `rhw` does; `None`
    /// only when there is no sun or the depth is exactly 0 (the original's divide gives infinity).
    pub(crate) fn sun_point_any(&self, cam: &Camera, aspect: f32, view: [f32; 4]) -> Option<SunPoint> {
        let bin = self.track.sky_bin.as_ref().filter(|b| b.sun_active())?;
        self.sun_def()?;
        let [vx, vy, vw, vh] = view;
        let (fwd, right, up) = self.flare_basis(cam);
        let rel = Vec3::from(bin.sun_position());
        let depth = rel.dot(fwd);
        if depth == 0.0 {
            return None;
        }
        let tan_y = (cam.fov_y_radians * 0.5).tan();
        let tan_x = tan_y * aspect;
        // the point in the view's 0…1 square, y down, then in the window's 640 × 480 frame
        let (sx, sy) = ((rel.dot(right) / (depth * tan_x) + 1.0) * 0.5, (1.0 - rel.dot(up) / (depth * tan_y)) * 0.5);
        Some(SunPoint { px: vx + sx * vw, py: vy + sy * vh, rel, depth })
    }

    /// 0x451710 for one `flare.dat` record drawn flat on the screen at `(px, py)` (the view's
    /// 640 × 480 frame; `centre` is that point laid in the view plane `ahead` metres from the eye):
    /// per layer with a texture a quad of `±size · 0.1 · width / 640` by `±size · 0.1 · height /
    /// 480` pixels about the point, turned — flag 4 — by `(x + y) / 1440` of a turn of its screen
    /// position (0x45175C…0x451768, 0x45187E…0x4518DE: `x' = x cos + y sin`, `y' = y cos − x sin`,
    /// screen y down), UVs (0, 0) (1, 0) (1, 1) (0, 1) from the top left (`[0x61BC78]`,
    /// 0x4514F5…), the layer's ARGB on all four corners, screen z 0 and `rhw` 1, additive.
    ///
    /// `scale` is the call's third argument: **under 0.05** (`[0x5B26A0]`, 0x4517AF) the layer is
    /// not drawn; **under 1** (`[0x5B24A8]`, 0x4517C9) the colour's alpha byte becomes
    /// `trunc(alpha · scale)` (0x4517DF…0x4517F5). The sun passes 1.0 (0x460398), the lens flare's
    /// elements `matches / 16`. `overlay` draws the quad after the finished frame under no depth
    /// test ([`nfs_renderer::DrawItem::overlay`], the lens flare — drawn by 0x45D1A0 after the
    /// world, 0x45D25C) instead of in the sky pass.
    pub(crate) fn flare_quads(&self, out: &mut Vec<nfs_renderer::DrawItem>, cam: &Camera, aspect: f32, view: [f32; 4], quad: FlareQuad<'_>) {
        let FlareQuad { def, at: (px, py), centre, ahead, scale, sprites, overlay } = quad;
        if scale < 0.05 {
            return;
        }
        let [_, _, vw, vh] = view;
        let main = view == SUN_VIEW_MAIN;
        let (fwd, right, up) = self.flare_basis(cam);
        let tan_y = (cam.fov_y_radians * 0.5).tan();
        let tan_x = tan_y * aspect;
        // flag 4: `(x + y) · (1/1440)` turns of the pixel position, on the original's 640 × 480
        // screen (its `[0x619784]` / `[0x619788]`); here the window's own size stands for it
        let (w, h) = self.renderer.as_ref().map_or((640.0, 480.0), |r| {
            let s = r.size();
            (s.0 as f32, s.1 as f32)
        });
        let (virt_w, virt_h) = nfs_game::world::virtual_screen(w, h);
        let turns = if def.flags & 4 != 0 { ((px / 640.0 * w + py / 480.0 * h) as f64 * 0.000_694_444_444_444_444_5) as f32 } else { 0.0 };
        let a = (turns as f64 * 6.28318452835083) as f32;
        let (sn, cs) = a.sin_cos();
        // `x' = x cos + y sin` with y down turns the quad's axes by the angle counter-clockwise
        let (r, u) = (right * cs + up * sn, up * cs - right * sn);
        for k in 0..2 {
            let Some(name) = def.textures[k].as_ref() else { continue };
            let Some(&(mesh, material)) = sprites.get(name) else { continue };
            // `size · 0.1 · W / virtW` by `size · 0.1 · H / virtH` pixels each side (0x451710's
            // `fdiv virtW` / `fdiv virtH` at 0x451824 / 0x451834, nfs5.dll): in the 640 × 480
            // frame `size · 0.1 · 640 / virtW` (1 at 4:3), of the view's half-width `vw / 2`
            // (1/320 in the main view), `tan_x` of a metre at one metre ahead
            let size = def.sizes[k] * 0.1;
            let (half_x, half_y) = (size * (640.0 / virt_w) / (vw * 0.5) * tan_x * ahead, size * (480.0 / virt_h) / (vh * 0.5) * tan_y * ahead);
            let argb = def.colours[k];
            let m = Mat4::from_cols((r * (2.0 * half_x)).extend(0.0), (u * (2.0 * half_y)).extend(0.0), fwd.extend(0.0), centre.extend(1.0));
            let tint = [((argb >> 16) & 0xFF) as f32 / 255.0, ((argb >> 8) & 0xFF) as f32 / 255.0, (argb & 0xFF) as f32 / 255.0];
            let alpha_byte = if scale < 1.0 { (((argb >> 24) & 0xFF) as f64 * scale as f64) as u32 } else { argb >> 24 };
            out.push(nfs_renderer::DrawItem {
                light: tint,
                alpha: alpha_byte as f32 / 255.0,
                layer_light: Some(unlit(tint)),
                sky_pass: !overlay,
                main_view_only: main,
                fog_w: if ahead == 1.0 { 0.0 } else { 1.0 },
                overlay,
                // 0x45D245: Thrash 0x24 ← 10 before 0x460460 (the sun's pass runs under the sky
                // pass's own states)
                alpha_test: overlay,
                alpha_ref: 10,
                ..nfs_renderer::DrawItem::new(mesh, material, m)
            });
        }
    }

    /// The track's flares, `RDrawArticleBundle`'s slot 3 for kind 2 (0x451940,
    /// `RE/notes/crp-geometry.md` §8): the part's definition by its id (0x452510), shown while its
    /// blink says so (`FlareDef::lit` on the game tick), and for each of the two layers with a
    /// texture a camera-facing billboard (0x44F6E0, `RBillBoard`) at the part's point, a side of
    /// `size × 0.1`, the layer's ARGB colour, turned about the view axis by — flag 4 —
    /// `√d² · 3.6 + facing · 120` degrees, `d` from the camera and `facing = −dir · forward`
    /// (0x451A5A…0x451B62; the root is the image's own `(bits >> 1) + 0x1FC00000`). Every shipped
    /// definition carries flags 4 alone, so 0x4515F0's other fades (flags 1, 2, 8, 0x20, 0x40) have
    /// nothing to act on and are not carried; the fog's (`[0x61BCA8]`) has no fog here.
    /// Only the flares whose article the track walk draws this frame run (`TrackWorld::effect_runs`
    /// over `article_draws` — the static / mid-ground drawer's 0x450D10, the instance drawer's
    /// 0x450883 within its LOD's part range), the `.scn` GEOM instances' too, through their
    /// instance matrix and their walk entry (`TrackWorld::running_flares`); the blend is the state object's 1, additive (0x4514E7), and the
    /// depth bias is carried (below). MEDIUM: the row 0x451A57 dots is taken as the
    /// camera's forward and the turn's sense as the axis-angle build's (0x539BD0).
    fn flare_items(&self, out: &mut Vec<nfs_renderer::DrawItem>, fwd: Vec3, right: Vec3, up: Vec3, draws: &[Option<nfs_game::world::ArticleDraw>]) {
        let Some(w) = self.world.as_ref() else { return };
        if self.flare_defs.is_empty() {
            return;
        }
        let tick = self.tick as u32;
        let cam = self.camera.position;
        for f in &w.running_flares(draws) {
            let Some(def) = self.flare_defs.iter().find(|d| d.id == f.id) else { continue };
            if !def.lit(f.phase, tick) {
                continue;
            }
            let d2 = (f.position - cam).length_squared();
            let dist = f32::from_bits((d2.to_bits() >> 1).wrapping_add(0x1FC0_0000));
            let facing = -f.direction.dot(fwd);
            let angle = if def.flags & 4 != 0 { (dist * 3.6 + facing * 120.0).to_radians() } else { 0.0 };
            let (s, c) = angle.sin_cos();
            let (r, u) = (right * c + up * s, up * c - right * s);
            // the depth bias (0x451AD2…0x451B21): `round(−250000 · cam[+0xC] / d²)`, `d²` the
            // eye-to-flare square distance, unclamped, as Thrash state 0x18 — `b · 2^-15` on the
            // game's screen z `far/(far − near) · (1 − near / Z)` (0x540CE0, `thrash.md` §6d.3, the
            // view's own planes), carried as `DrawItem::depth_bias`: some 7.6 m of view depth toward
            // the eye (`cam[+0xC] / d²` is that z's slope). Where it passes the near plane the shader
            // holds the screen z at 0 (its APPROXIMATION, `shader.wgsl`: dx7z passes `sz < 0` to the device
            // unclipped and unclamped — 0x60006B41…, flush 0x60006F20, quad path 0x60007650, draw 0x60006A50 with
            // flags 0 — and the Direct3D runtime's own handling is not in a readable binary).
            let z_near = self.camera.near;
            let far = self.view_far;
            let bias = (-250000.0 * (far * z_near / (far - z_near)) / d2.max(1e-6)).round();
            let pos = f.position;
            for k in 0..2 {
                let Some(name) = def.textures[k].as_ref() else { continue };
                let Some(&(mesh, material)) = self.flare_sprites.get(name) else { continue };
                let size = def.sizes[k] * 0.1;
                let argb = def.colours[k];
                let m = Mat4::from_cols((r * size).extend(0.0), (u * size).extend(0.0), right.cross(up).extend(0.0), pos.extend(1.0));
                let tint = [((argb >> 16) & 0xFF) as f32 / 255.0, ((argb >> 8) & 0xFF) as f32 / 255.0, (argb & 0xFF) as f32 / 255.0];
                out.push(nfs_renderer::DrawItem {
                    light: tint,
                    alpha: (argb >> 24) as f32 / 255.0,
                    layer_light: Some(unlit(tint)),
                    depth_bias: bias as i32,
                    ..nfs_renderer::DrawItem::new(mesh, material, m)
                });
            }
        }
    }

    /// `SPECIALFX` `[0x657D6C]` as 0x44ED10 switches on it (0x44EDEA): 0…3, anything else 3.
    pub(crate) fn special_fx_level(&self) -> usize {
        match self.choice_index(choice::GRAPHICS_SPECIAL_FX).map(|(i, _)| i) {
            Some(i) if i < 3 => i,
            _ => 3,
        }
    }

    /// `0x44EC80`'s `LayerFalseSort1`/`2` pick, read to the instruction (`RE/notes/thrash.md`
    /// §6d.4, [`nfs_renderer::FakeSortMode`]'s doc): 0/1 always `Single`, 3 always `Double`; 2
    /// picks by `REPLAY [0x657424] ≥ 2` (`self.replay.stream`'s mode `Playing`/`Exhausted`) — the
    /// two tests are mutually exclusive, not one falling through to the other:
    ///
    /// * `REPLAY ≥ 2`: `Double` while `[0x627ED8]` is under 37.5 (0x44ECAA…0x44ECBB), else
    ///   `PerItem`. `[0x627ED8]` is **not** a frame time (corrected 2026-09-30): it is the
    ///   main view slot's `+0x58` (slot table base `0x627E80`, stride 0x10C — 0x462B80's
    ///   `iVar3 = slot · 0x10C`), the half-angle in degrees that SetSlotView resets to 37.5
    ///   (0x465886) and 0x462B80 (TVCam 0x462E30, SplineCam 0x463410) zooms — `TvSlot::half_angle`.
    ///   So a replay draws `FakeSort` twice while the trackside camera is zoomed in, once per item
    ///   at the default or wider zoom. *(Until 2026-09-30 the port compared the last frame's
    ///   `frame_dt` in ms with 37.5.)*
    /// * `REPLAY < 2`: `Single` while the player car's speed (`+0x35C`, `Vehicle::speed_est`) is
    ///   15 m/s or more, else `PerItem`.
    ///
    /// `level` is `None` off the race (`special_fx_level` not called): kept at the construction
    /// default, `Double` — the front end draws no `FakeSort` track material, so this never shows.
    pub(crate) fn fakesort_mode(&self, level: Option<usize>) -> nfs_renderer::FakeSortMode {
        use nfs_renderer::FakeSortMode::{Double, PerItem, Single};
        let Some(level) = level else { return Double };
        if level < 2 {
            return Single;
        }
        if level >= 3 {
            return Double;
        }
        if self.replay_zoom().is_some() {
            // `nfs5.dll` rewrites the compare's `[0x5B3C9C]` (0x44ECB2) to its cell
            if self.replay_zoom() == Some(true) { Double } else { PerItem }
        } else {
            let speed = self.vehicle.as_ref().map(|v| v.speed_est).unwrap_or(0.0);
            if speed >= 15.0 { Single } else { PerItem }
        }
    }

    /// The two replay gates on the main slot's half-angle `[0x627ED8]` (slot 0's `+0x58`):
    /// `None` unless `REPLAY [0x657424] ≥ 2` (a playing or exhausted replay), else whether the
    /// half-angle is under `[0x5B3C9C]` — the literal 37.5 the dll rewrites to its cell at both
    /// compares, 0x44ECB2 (`FakeSort`, 0x44EC80) and 0x44F1B1 (the view distance, 0x44F1A0).
    /// Only the trackside / spline zoom (0x462B80, `TvSlot::half_angle`) moves `+0x58` off the
    /// cell; every other view keeps SetSlotView's.
    pub(crate) fn replay_zoom(&self) -> Option<bool> {
        let replaying = matches!(
            self.replay.stream.as_ref().map(|r| r.mode),
            Some(nfs_game::replay::Mode::Playing | nfs_game::replay::Mode::Exhausted)
        );
        replaying.then(|| {
            let half = match self.track_cameras.as_ref() {
                Some(t) if matches!(self.camera_view(), 0x0C | 0x14) => t.tv.half_angle,
                _ => nfs_game::camera::mode_half_angle(),
            };
            half < nfs_game::camera::mode_half_angle()
        })
    }

    /// The `car+0x330` of the car whose effect set is `k` (`nfs_game::car_effects`): 0 the player,
    /// then the opponents, the cops and the traffic — the point a relative system's draw adds
    /// (its `+0x4C`, 0x4546A3).
    fn effect_car_position(&self, k: usize) -> Vec3 {
        let n_opp = self.road.opponents.len();
        let n_pol = self.road.police.as_ref().map_or(0, |p| p.cars.len());
        match k {
            0 => self.vehicle.as_ref().map_or(Vec3::ZERO, |v| v.position),
            k if k <= n_opp => self.road.opponents[k - 1].vehicle.position,
            k if k <= n_opp + n_pol => self.road.police.as_ref().map_or(Vec3::ZERO, |p| p.cars[k - 1 - n_opp].body.position),
            k => self.road.traffic.as_ref().and_then(|t| t.bodies.get(k - 1 - n_opp - n_pol)).map_or(Vec3::ZERO, |b| b.position),
        }
    }

    /// What 0x459020 hands its two emitters: the sky manager's track B at the camera's slice
    /// (`[0x627E48]+0x50`) — field 7 `+0x20` for the snow (0x45CFD0 → 0x45A940) and field 5 `+0x18`
    /// for the rain (0x45D000 → 0x45A8E0).
    pub(crate) fn weather_strengths(&self) -> (f32, f32) {
        self.weather_strengths_at(self.cam_slice.unwrap_or(0) as i32)
    }

    /// The same two fields at `slice`. The pass's prime 0x459260 (0x4690AF) reads them at slice
    /// **0**: 0x4690A0 rebuilds the sky right before it (0x4690AA → 0x45DD00 → 0x45DBD0 with 1: the
    /// old manager deleted, a new one made by 0x45D800, whose `+0x50` starts at 0, 0x45D81D), and
    /// only the sky pass writes `+0x50` (0x45D598, from 0x481400 at the eye).
    pub(crate) fn weather_strengths_at(&self, s: i32) -> (f32, f32) {
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        match self.track.sky_bin.as_ref() {
            Some(b) => (b.sample_track_b(7, s, lap), b.sample_track_b(5, s, lap)),
            None => (0.0, 0.0),
        }
    }
}

/// `[0x655F34]`, the crowding factor (`engine-audio.md` §4.8, 0x4A8199): start at 192.0 and take
/// `128 − d` off it for every audio car slot within 128 m of the **listener**, then clamp to 0…128.
/// With nothing near, that is 128 — the widest in-car stereo spread and full AI road noise. This port
/// used to pass 0 to the player's mixer and a constant 64 to the AI cars.
/// A sprite's shading: its colour and nothing else. The lamp glares carry their colour in the
/// vertices (`L+0x40` / `L+0x44` with the alpha `b`, 0x43D3BA…0x43D3FD), the beams theirs
/// (0x43D64D…0x43D7B1), the track flares one colour on all four corners (0x44F6E0) and the
/// particles their ramp's (0x454430 → 0x44F6E0); no light term is computed for any of them — the
/// port's sun-and-ambient term shaded each quad by its own orientation.
/// The sun's `flare.dat` record: the FourCC `anus` as stored (0x460289 pushes `[0x5CF52C]` =
/// 0x73756E61 to 0x452260), its layers `SUNA` and `BLUR`.
/// The sun's screen point (0x460290): `(px, py)` in the view's 640 × 480 frame, the sun's offset
/// from the eye `rel` and its depth along the view axis.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SunPoint {
    pub px: f32,
    pub py: f32,
    pub rel: Vec3,
    pub depth: f32,
}

/// One `flare.dat` record's screen quad for [`App::flare_quads`] (0x451710): the record, its point
/// `at` in the view's 640 × 480 frame, that point laid `ahead` metres from the eye (`centre`), the
/// call's third argument `scale`, the layers' sprites, and `overlay` (drawn after the finished frame).
#[derive(Clone, Copy)]
pub(crate) struct FlareQuad<'a> {
    pub def: &'a nfs_formats::flare::FlareDef,
    pub at: (f32, f32),
    pub centre: Vec3,
    pub ahead: f32,
    pub scale: f32,
    pub sprites: &'a std::collections::HashMap<String, (nfs_renderer::MeshId, nfs_renderer::MaterialId)>,
    pub overlay: bool,
}

pub(crate) const SUN_FLARE: u32 = 0x7375_6E61;

/// The sky pass's viewport in the 640 × 480 frame (`App::sun_items`): the main view's whole screen.
pub(crate) const SUN_VIEW_MAIN: [f32; 4] = [0.0, 0.0, 640.0, 480.0];

/// The rear-view mirror's: the tray's rectangle (185, 0) 240 × 84 that 0x42C510 / 0x42C680 hand
/// 0x4691D0 as screen fractions (`RE/AUDIT.md` §O).
pub(crate) const SUN_VIEW_MIRROR: [f32; 4] = [185.0, 0.0, 240.0, 84.0];

/// The triangles of one streak draw (`App::streak_items`); 0x454EC0 batches 80 (0x454FA1).
pub(crate) const STREAK_CAP: usize = 80;

pub(crate) fn unlit(tint: [f32; 3]) -> nfs_renderer::LayerLight {
    nfs_renderer::LayerLight { ambient: tint, diffuse: [0.0; 3], direction: [0.0; 3] }
}
