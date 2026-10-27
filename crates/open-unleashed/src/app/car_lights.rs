//! Car lamps, reflections and shadows.

use crate::*;

impl App {
    /// Which lamp slots are lit and how brightly, 0x439360 for a racing car: 0 / 1 the headlights
    /// (`car+0xD87 & 1`, this port's night flag); 2 / 3 the brake lamps by the effective brake
    /// `car+0xD80` (full before clock 0x1C0 and for a dead car, 0x49E00D…0x49E02E), and with the
    /// headlights on and no brake they still glow at `[0x5B35A4]` 0.6 (0x4396F0…0x43979A); 4…7
    /// the indicators (bits 3 / 4 of `car+0xD87`, blinking; both in the car state 3): left lights
    /// 4 on the odd phase and 6 on the even, right 5 and 7 (0x4393C8…0x439483); 8 reverse
    /// (`car+0xD82` gear 0); 9 never; 0xA / 0xB and 0xC / 0xD below. Then a damage zone over
    /// `[0x5B2408]` 30 breaks its lamps (0x4398C6…0x4399E6): zone 0 → slots 0 / 4, zone 2 → 1 / 5,
    /// zone 4 → 3 / 7 / 0xB, zone 6 → 2 / 6 / 0xA, zone 5 → 8, zone 8 → 0xC / 0xD. *(Until
    /// 2026-09-26 the port kept ten slots, so the combined lamps and the sirens never lit.)* The
    /// indicator bits are the player's commands 10 / 11 / 12 (SIGNALLEFT / SIGNALRIGHT / SIREN =
    /// hazards, `Car_applyInput` 0x499365…0x499402, `lamp_command`) on the W / E / T keys 0x51D1D0
    /// binds (*corrected 2026-09-26: this said the shipped bindings left them unbound*), and an
    /// eliminated car (`car+0xB50 == 3`) blinks both; the phase is
    /// `TIMER_getTicks() / (ticksPerSecond/2)` odd (0x4393C8…0x4393D8), half a second each at
    /// 128 Hz — `hazard` carries it. `fade` is the headlights' ramp (`[esi+0x64]+0x110`,
    /// 0x43951A…0x4396A0): a change of the flag `+0x567` starts `{now, 0x3E8 ms, from, to}` with
    /// `to` = `[esi+0x70]` or 0 (ignored while a ramp still runs), and slots 0 / 1 take the ramp's
    /// value over `[esi+0x70]` — so a night race opens with the lamps rising from 0 to full over a
    /// second, linearly. The brake pedal is `Vehicle::brake_lamp` (`car+0xD98` picks D7F or D80).
    pub(crate) fn lamp_slots(headlights: bool, fade: f32, brake: bool, reverse: bool, signals: (bool, bool, bool), damage: Option<&[f32; 10]>) -> [(bool, f32); 14] {
        let tail = if brake { (true, 1.0) } else if headlights { (true, 0.6) } else { (false, 1.0) };
        let (left, right, phase) = signals;
        // 0xA / 0xB, the combined brake-and-indicator lamps (the 356A's and the 550's
        // `GlareBrakeSig`, `GlareSignalRearBrake`; 0xA on the left with 2 / 6, 0xB on the right
        // with 3 / 7 — the damage zones 6 / 4): lit by the indicator's phase while its side's bit
        // is set, else by the brake (0x439485…0x4394BD); with the headlights on and no brake, at
        // 1.0 while the bit is set, else lit at `[0x5B35A8]` 0.6 (0x43971B…0x4397BE); otherwise
        // 1.0 (0x4397DF…0x4397F6). 0xC / 0xD — the cops' `GlareSiren` — are lit every frame
        // (0x4394F8…0x439509) at the constructor's 1.0 (0x4416C4), as every slot's intensity
        // not written here.
        let combo = |bit: bool| match (bit, headlights && !brake) {
            (true, _) => (phase, 1.0),
            (false, true) => (true, 0.6),
            (false, false) => (brake, 1.0),
        };
        let mut s = [
            (headlights, fade),
            (headlights, fade),
            tail,
            tail,
            (left && phase, 1.0),
            (right && phase, 1.0),
            (left && !phase, 1.0),
            (right && !phase, 1.0),
            (reverse, 1.0),
            (false, 1.0),
            combo(left),
            combo(right),
            (true, 1.0),
            (true, 1.0),
        ];
        if let Some(d) = damage {
            // 0x4398C6…0x4399E6: zones 0, 2, 4, 6, 5 and 8 (`car+0x4E4 + 4k`)
            for (zone, slots) in [(0usize, &[0usize, 4][..]), (2, &[1, 5]), (4, &[3, 7, 0xB]), (6, &[2, 6, 0xA]), (5, &[8]), (8, &[0xC, 0xD])] {
                if d[zone] > 30.0 {
                    for &k in slots {
                        s[k].0 = false;
                    }
                }
            }
        }
        s
    }

    /// ENHANCEMENT E24: the render-side "lit by the tunnel" value of one car, 0..1, or 0 with the switch off, on a night
    /// track (its lamps are lit already, and a player who put them out keeps that) or outside a covered stretch
    /// (`TrackWorld::in_tunnel`, the original's bit 15 of the road's overhead flags, the one the tunnel reverb reads).
    /// It rises over [`TUNNEL_LAMP_IN`] ms after the car's position is covered and falls over [`TUNNEL_LAMP_OUT`] ms
    /// after it leaves; `now_ms` is the lamp clock of the draw, so the value is the same for the mirror's pass and
    /// the first call of a car snaps to its target (a still taken in a tunnel has its lamps lit). Render-side only:
    /// the car's lamp byte (`car+0xD87`, `lamp_bits`), the lights key, the replay and the net messages are not read
    /// or written here, the value only widens the slots [`Self::lamp_slots_tunnel`] hands the glare / beam / spot draws.
    pub(crate) fn tunnel_lit(&self, key: u32, pos: Vec3, now_ms: f64) -> f32 {
        if !self.enh.is_on(nfs_renderer::E24) || self.track.is_night() {
            return 0.0;
        }
        let covered = self.world.as_ref().is_some_and(|w| w.in_tunnel(pos));
        self.tunnel_lamps.borrow_mut().value_held(key, covered, now_ms)
    }

    /// ENHANCEMENT E24: [`Self::lamp_slots`] with the tunnel's lit value `f` (0..1, [`Self::tunnel_lit`]) laid over: a
    /// slot the car's own state leaves dark but its headlights-on state would light (the headlights, the tail
    /// glow, the combined lamps) is lit at `f` times that intensity; what the car's own state lights is as it was.
    /// `f == 0` is exactly `lamp_slots`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn lamp_slots_tunnel(f: f32, headlights: bool, fade: f32, brake: bool, reverse: bool, signals: (bool, bool, bool), damage: Option<&[f32; 10]>) -> [(bool, f32); 14] {
        let mut s = Self::lamp_slots(headlights, fade, brake, reverse, signals, damage);
        if f > 0.0 && !headlights {
            let on = Self::lamp_slots(true, 1.0, brake, reverse, signals, damage);
            for (a, b) in s.iter_mut().zip(on) {
                if !a.0 && b.0 {
                    *a = (true, b.1 * f);
                }
            }
        }
        s
    }

    /// Which lamp slots a **front-end car** lights: the lamp function 0x439360 sends the front end
    /// (`[0x65B928]` set, 0x43936E `jne 0x439800`) to its own arm, which lights slots 0 and 1 at
    /// `[0x5B359C]` 0.8, 2 and 3 and 0xA and 0xB at `[0x5B35A4]` 0.6, 8 at `[0x5B35A0]` 0 and 9 at
    /// 1.0 (0x439800…0x4398BF), leaving 4…7, 0xC and 0xD as the constructor's unlit (0x4416B1), and
    /// then breaks the lamps of a zone over 30 as the race arm does (0x4398C6…0x4399E6). The
    /// zones are the car's own (`car+0x4E4 + 4k`), here the garage car's.
    pub(crate) fn fe_lamp_slots(damage: &[f32; 10]) -> [(bool, f32); 14] {
        let mut s = [(false, 1.0f32); 14];
        for (k, v) in [(0, 0.8f32), (1, 0.8), (2, 0.6), (3, 0.6), (0xA, 0.6), (0xB, 0.6), (8, 0.0), (9, 1.0)] {
            s[k] = (true, v);
        }
        for (zone, slots) in [(0usize, &[0usize, 4][..]), (2, &[1, 5]), (4, &[3, 7, 0xB]), (6, &[2, 6, 0xA]), (5, &[8]), (8, &[0xC, 0xD])] {
            if damage[zone] > 30.0 {
                for &k in slots {
                    s[k].0 = false;
                }
            }
        }
        s
    }

    /// The effect pass 0x43D800 of one front-end model standing at `place` (its glares: the
    /// first sprite `CARG` and the star `SPIK` per lit lamp, 0x43CDD0), appended to `items`.
    /// A **room** model has every lamp lit at 1.0 — the model constructor's loop over all twenty
    /// slots when its kind is 2 (0x4D1397…0x4D13AA, the rooms' `0x4D1160(2, …)` calls) — which is
    /// what makes the dealership's two ceiling spots white discs; a car has [`Self::fe_lamp_slots`].
    /// The camera must already be the front end's (origin, looking +z).
    pub(crate) fn fe_lamp_items(&self, model: &CarModel, place: Mat4, slots: [(bool, f32); 14], items: &mut Vec<nfs_renderer::DrawItem>) {
        let (_, rot, pos) = place.to_scale_rotation_translation();
        let mut draws = Vec::new();
        self.lamp_draws_in(model, (pos, rot), 0.0, slots, true, &mut draws);
        for d in draws {
            if let LampDraw::Glare(mut item) = d {
                item.early = true;
                items.push(item);
            }
        }
    }

    /// `CARG`, `SPIK` and `GLNT` of `Render/common.fsh` for the glares (blend pair 1, additive,
    /// 0x43CF5B), when no race has loaded them yet — the front end draws the same effect pass.
    pub(crate) fn fe_load_lamp_sprites(&mut self) {
        if self.glare_draw.is_some() {
            return;
        }
        let Some(renderer) = self.renderer.as_mut() else { return };
        let Some(path) = nfs_assets::GamePaths::find_file(&self.paths.render_dir(), "common.fsh") else { return };
        let Ok(data) = std::fs::read(&path) else { return };
        let Ok(images) = nfs_assets::texture::decode_pack(&path, &data) else { return };
        let mut quad = nfs_renderer::MeshData::default();
        for (x, y, u, v) in [(-0.5f32, -0.5f32, 0.0f32, 1.0f32), (0.5, -0.5, 1.0, 1.0), (0.5, 0.5, 1.0, 0.0), (-0.5, 0.5, 0.0, 0.0)] {
            quad.vertices.push(nfs_renderer::Vertex { position: [x, y, 0.0], normal: [0.0, 1.0, 0.0], uv: [u, v], color: [1.0; 4] });
        }
        quad.indices = vec![0, 1, 2, 0, 2, 3];
        let mut sprite = |name: &str| -> Option<(nfs_renderer::MeshId, nfs_renderer::MaterialId)> {
            let img = images
                .iter()
                .find(|t| t.name.trim() == name)
                .or_else(|| images.iter().find(|t| t.name.trim().eq_ignore_ascii_case(name)))?;
            let tex = renderer.upload_texture_mips(img.width, img.height, &img.cache_mips());
            let mesh = renderer.upload_mesh(&quad);
            let material = renderer.create_material(nfs_renderer::MaterialDesc {
                alpha_blend: true,
                cull_back: false,
                depth_test: true,
                blend_pair: 1,
                ..nfs_renderer::MaterialDesc::textured(tex)
            });
            Some((mesh, material))
        };
        self.glare_draw = sprite("CARG");
        self.glare_star = [sprite("SPIK"), sprite("GLNT")];
    }

    /// The car's effect pass `R3DCarLOD` slot 3, **0x43D800** (vtable 0x5B36AC), which the car draw
    /// 0x4432B0 runs right after the body's LODs (slot 14 at 0x44344B, then `[car+0xA4]`'s slot 3
    /// at 0x443464) — so a car's glares and beams follow its own body and precede the next car's,
    /// and the whole car list (0x443610, after the track 0x47FB40 at 0x468B20 / 0x468B2F) precedes
    /// the skid marks and the rest of the R3D objects (0x467290 → 0x466660), the particles and the
    /// flares of the race pass 0x468A30. The pass sets the view's **depth writes off** (0x537870(0)
    /// at 0x43D81F, on again at 0x43DE76) and the alpha reference 10 (Thrash 0x24, 0x43D840), then
    /// per `ef` record whose slot is lit (0x443660): the glare 0x43CDD0, the night beam 0x43D4F0,
    /// and the same for the mirrored copy (0x43DAB6…0x43DD9E) — the order the draws come back in.
    ///
    /// **The glare 0x43CDD0** (`RE/notes/particles.md` §4): each lamp gets its first sprite, `CARG`
    /// in every row of the table at 0x5CD73C, with the strength `b = min(dot(lamp normal, view
    /// axis), 1)` (the kinds 4 and 5 sharpened to `(b − 0.95)·20`), cut off at 0.1, in the lamp's
    /// own colour `L+0x40` with alpha `b` (0x43D3BA…0x43D3FD). It is a camera-facing quad at the
    /// lamp's centre — the camera's rows `+0xF4` / `+0xE8` span it (0x43CEA7…0x43CEE8,
    /// 0x43D301…0x43D399) — shaped like the lamp's rectangle seen from the camera: the two
    /// half-axes taken into the view plane, scaled by `b` (MEDIUM: the original turns and
    /// stretches a unit quad by the rectangle's projected axes, 0x43D1A9…0x43D2F4, with one more
    /// ratio term not carried), and the second sprite on the same quad. Its states are the
    /// primitive helper's (0x59F070: the state object 0x5379F0's gouraud, blending on, cull 0,
    /// bias 0) with the blend pair **1**, additive (0x43CF5B), and the **depth bias**
    /// `fistp(b · [0x5B36EC] −20000 · cam[+0xC] / |centre − eye|²)` (0x43D419…0x43D45C → 0x537A60,
    /// Thrash 0x18): `b · 20000 · K / d²` units of `2^-15` toward the eye on the game's screen z
    /// `far/(far − near) · (1 − near / Z)` (0x540CE0, `thrash.md` §6d.3), whose slope is `K / Z²` —
    /// so about `b · 0.61` m of view depth at the lamp, whatever its distance. The
    /// alpha test is the pass's `GREATER 10` on texel × `b`. Then the third draw, the
    /// lamp's reflection on the road 0x451D60 (0x43D486…0x43D4BE; `lamp_reflection`).
    ///
    /// **The night beam 0x43D4F0** of every lit lamp whose `L+0x4F` is set, at night (`word
    /// [0x628A28] > 0`, 0x43DA88…0x43DAAA): a strip of nine vertex pairs round the lamp — the near
    /// ring `centre + a2·t₀ + a1·t₁` with `(t₀, t₁)` the table at 0x5CD918 (radius 0.3 at 45°
    /// steps, closing on itself), the far ring the same with the axes ×`[0x5B2400]` 5 and pushed
    /// `[0x5B2608]` 8 along the car's forward (`(0, 0, −1)` through the car's matrix) — coloured
    /// `L+0x40` at alpha 0x10 near and 0 far (0x43D64D…0x43D7B1), **untextured** (the helper's
    /// texture slot is 0, its UV slot the zero table 0x6AF400, 0x43D531 / 0x43D53C). Its state
    /// object is set to the plain pair 0, blending on (2) and gouraud (0x43D547…0x43D563), cull 0
    /// and bias 0 from the helper; the alpha test is turned **off** around its draw (0x24 ← 0 at
    /// 0x43D506, ← 10 at 0x43D7DB). Both draw with the view's depth test `LESSEQUAL`, no fog
    /// (Thrash 0x15 is 0 in the view's settings, 0x537724, and 0x4691AF sets it 0 — `dx7z.dll`
    /// 0x600049E9 `FOGENABLE` off), and no depth writes. Neither has an `Am` op: the helper's
    /// layers are coords, colours, uv, texture, state and the draw (0x59F0A3…0x59F186), so
    /// `Headlights::LayerVertexLight_Ambient` is not on this path.
    pub(crate) fn lamp_draws(&self, model: &CarModel, pos: Vec3, rot: Quat, ground: f32, slots: [(bool, f32); 14], out: &mut Vec<LampDraw>) {
        self.lamp_draws_in(model, (pos, rot), ground, slots, false, out);
    }

    /// [`Self::lamp_draws`] for a race (`front_end` false) or for the front end's 3D views, where
    /// `[0x65B928]` is set: no road reflection (0x43D47D's gate), no night beam (`[0x628A28]` is
    /// the race's weather word, 0x43DA88), and the view's planes are the front end's.
    pub(crate) fn lamp_draws_in(&self, model: &CarModel, (pos, rot): (Vec3, Quat), ground: f32, slots: [(bool, f32); 14], front_end: bool, out: &mut Vec<LampDraw>) {
        out.clear();
        let Some((mesh, material)) = self.glare_draw else {
            return;
        };
        let view = (self.camera.target - self.camera.position).normalize_or_zero();
        // the reflection's inputs, once per car: the sky manager's track B field 5 at the camera's
        // slice (0x45D000 on `[0x627E48]`, whose `+0x50` is that slice), the camera's row `+0xE8`
        // (its right, 0x451DF9) and `[0x61BCBC]`, the road under the eye (`eye_road_height`)
        let lap = self.slices.as_ref().map_or(0, |t| t.slices.len() as i32);
        let wet = match (self.track.sky_bin.as_ref(), self.cam_slice) {
            (Some(b), Some(s)) if !front_end => b.sample_track_b(5, s as i32, lap),
            _ => 0.0,
        };
        let right = view.cross(self.camera.up).normalize_or_zero();
        let eye_ground = self.eye_road_height();
        // `cam[+0xC]` = `far · near / (far − near)` of the view's planes (0x467247…0x467261)
        let (far, z_near) = if front_end { (super::fe_room::FE_FAR, super::fe_room::FE_NEAR) } else { (self.view_far, self.camera.near) };
        let k = far * z_near / (far - z_near);
        const T: [(f32, f32); 9] = [(0.3, 0.0), (0.212, 0.212), (0.0, 0.3), (-0.212, 0.212), (-0.3, 0.0), (-0.212, -0.212), (0.0, -0.3), (0.212, -0.212), (0.3, 0.0)];
        for l in &model.lights {
            let Some(&(true, intensity)) = slots.get(l.slot as usize) else {
                continue;
            };
            // the lamp through its article's frame and channel (0x43D8D1…0x43DA46,
            // `CarModel::lamp_frames`): the glare's for the sprite, the reflection and the beam's
            // rings, the beam's own (no x turn) for its forward
            let (gf, bf) = model.lamp_frames(l);
            let (centre, ax1, ax2) = (gf.transform_point3(l.position), gf.transform_vector3(l.axis1), gf.transform_vector3(l.axis2));
            let world = pos + rot * centre;
            let to_cam = self.camera.position - world;
            let d2 = to_cam.length_squared();
            // the lamp's normal (`L+0x30`, the half-axes' cross product 0x532880, pointing into the
            // car) against the view's forward axis — the camera's, not the line to the lamp
            // (0x43D0BE…0x43D159)
            let facing = (rot * gf.transform_vector3(l.facing)).normalize_or_zero();
            let mut b = facing.dot(view).min(1.0);
            if matches!(l.kind, 4 | 5) {
                // `(b − [0x5B2610]) · [0x5B2470]` — 0.95 and 20 (0x43D186…0x43D18C); read as 0.7
                // until 2026-09-26
                b = (b - 0.95) * 20.0;
            }
            // `b *= car+0x2D8[slot]` (0x43D192, 0x443690)
            b *= intensity;
            if d2 >= 1e-6 && b > 0.1 {
                let depth_bias = (b * -20000.0 * k / d2).round() as i32;
                // the vertex alpha `ftol(b · 255) & 0xFF` (0x43D3BA…0x43D3C8): `b` is only clamped
                // to 1 before the sharpening, so a kind-4 / 5 glint past 1 wraps its byte
                let alpha = (((b * 255.0) as i32) & 0xFF) as f32 / 255.0;
                let c = l.colour[0];
                let tint = [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0];
                // ENHANCEMENT E26: a lit lamp near wet road is also a light the road shows a streak of (render only)
                if wet > 0.05 && self.enh.is_on(nfs_renderer::E26) {
                    let s = b.min(1.0) * 1.3;
                    self.rain.lamps.borrow_mut().push((world, [tint[0] * s, tint[1] * s, tint[2] * s]));
                }
                // a billboard: the corners are the lamp's centre plus the camera's two rows `+0xF4` /
                // `+0xE8` times the 2-D corner offsets (0x43D301…0x43D399), so the quad faces the
                // camera — the half-axes taken into that plane stand for the offsets' rotation and scale
                let flat = |a: Vec3| a - view * a.dot(view);
                let (a1, a2) = (flat(rot * ax1) * (2.0 * b), flat(rot * ax2) * (2.0 * b));
                let m = Mat4::from_cols(a2.extend(0.0), a1.extend(0.0), view.extend(0.0), world.extend(1.0));
                out.push(LampDraw::Glare(nfs_renderer::DrawItem {
                    light: tint,
                    alpha,
                    layer_light: Some(unlit(tint)),
                    depth_bias,
                    alpha_test: true,
                    ..nfs_renderer::DrawItem::new(mesh, material, m)
                }));
                // the second sprite on the same quad (0x43D3DD…0x43D477): the row's `+0x10` FourCC —
                // `GLNT` for kind 4, `SPIK` for the others, none for the front end's kind 10 — in
                // `L+0x44`, under the same bias
                if l.kind != 10
                    && let Some((m2, mat2)) = self.glare_star[usize::from(l.kind == 4)]
                {
                    let c = l.colour[1];
                    let tint = [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0];
                    out.push(LampDraw::Glare(nfs_renderer::DrawItem {
                        light: tint,
                        alpha,
                        layer_light: Some(unlit(tint)),
                        depth_bias,
                        alpha_test: true,
                        ..nfs_renderer::DrawItem::new(m2, mat2, m)
                    }));
                }
                // the third draw, the lamp's reflection on the road (0x43D47D…0x43D4BE): outside
                // the front end (`[0x65B928]`), `wet · b` over 0.1 calls 0x451D60 with `wet · b²`
                let wb = wet * b;
                if wb > 0.1 {
                    let len2 = ax1.length_squared();
                    let size = f32::from_bits(((len2.to_bits() as i32 >> 1) + 0x1FC0_0000) as u32);
                    out.push(Self::lamp_reflection(world, ground, size, l.colour[0], wb * b, ReflectionEye { eye: self.camera.position, right, eye_ground, k }));
                }
            }
            // the beam, whatever the glare's `b` (0x43DA7C…0x43DAAA)
            if l.night && !front_end && self.track.is_night() {
                let (a1, a2) = (rot * ax1, rot * ax2);
                let dir = rot * bf.transform_vector3(Vec3::Z);
                let rgb = [((l.colour[0] >> 16) & 0xFF) as f32 / 255.0, ((l.colour[0] >> 8) & 0xFF) as f32 / 255.0, (l.colour[0] & 0xFF) as f32 / 255.0];
                let mut v = [nfs_renderer::Vertex { position: [0.0; 3], normal: [0.0, 1.0, 0.0], uv: [0.0; 2], color: [0.0; 4] }; 18];
                for (k, &(t0, t1)) in T.iter().enumerate() {
                    let near = world + a2 * t0 + a1 * t1;
                    let far = world + a2 * (5.0 * t0) + a1 * (5.0 * t1) + dir * 8.0;
                    v[2 * k].position = near.into();
                    v[2 * k].color = [rgb[0], rgb[1], rgb[2], 16.0 / 255.0];
                    v[2 * k + 1].position = far.into();
                    v[2 * k + 1].color = [rgb[0], rgb[1], rgb[2], 0.0];
                }
                out.push(LampDraw::Beam(v, world));
            }
        }
    }

    /// ENHANCEMENT E15 (ENH-CONES): the original's translucent beam strip (0x43D4F0, alpha 16 / 255 at the lamp, 0 at its
    /// far ring) fades out where E15's own light takes its place: a lamp with a spot this frame (`lit`: every lit lamp
    /// with its intensity, `over_limit`: the lamps the spot limit left out, which keep the original's cone so no car
    /// goes dark) gets `alpha * (1 - k)`, `k` the lamp's ramp (up over 350 ms, down over 700 ms, so a lamp crossing the
    /// limit never pops) times the spot's own intensity (a pop-up car's lamps ramp in). The glare sprites are not touched.
    pub(crate) fn fade_cones(&mut self, log: &mut Vec<BeamLog>, lit: &[(Vec3, f32)], over_limit: &[Vec3], now_ms: f64) {
        for b in log.drain(..) {
            let spot = lit.iter().find(|(p, _)| *p == b.lamp).map_or(0.0, |&(_, i)| i.clamp(0.0, 1.0));
            let ramp = self.cone_fade.borrow_mut().value(b.key, spot > 0.0 && !over_limit.contains(&b.lamp), now_ms);
            let k = ramp * spot;
            if k <= 0.0 {
                continue;
            }
            let mut v = b.verts;
            for x in &mut v {
                x.color[3] *= 1.0 - k;
            }
            if let Some(r) = self.renderer.as_mut() {
                r.write_mesh_vertices(b.mesh, &v);
            }
        }
    }

    /// ENHANCEMENT E15: the lit headlights of a car as spotlights (`nfs_renderer::Spots`). Each lamp of the
    /// model's `CARG` list with the night-beam flag (`L+0x4F`, the headlights, slots 0 / 1) whose slot is lit
    /// gives its centre through its article's frame (the same `lamp_frames` the glare and the beam use) and the
    /// beam's forward axis `rot * bf * +Z` (0x43D4F0's far ring pushes along it), tipped [`SPOT_PITCH_DEG`]
    /// down about the car's right axis so the lower edge of the cone meets the road a few metres ahead.
    /// `gain` is how dark the place is (night or tunnel 1, open day road less).
    pub(crate) fn spot_lamps(model: &CarModel, pos: Vec3, rot: Quat, slots: &[(bool, f32); 14], gain: f32, out: &mut Vec<nfs_renderer::SpotLamp>) {
        let (sin, cos) = SPOT_PITCH_DEG.to_radians().sin_cos();
        for l in &model.lights {
            let Some(&(true, intensity)) = slots.get(l.slot as usize) else { continue };
            if !l.night {
                continue;
            }
            let (gf, bf) = model.lamp_frames(l);
            let world = pos + rot * gf.transform_point3(l.position);
            let fwd = (rot * bf.transform_vector3(Vec3::Z)).normalize_or_zero();
            if fwd == Vec3::ZERO {
                continue;
            }
            let dir = (fwd * cos - (rot * Vec3::Y) * sin).normalize_or_zero();
            out.push(nfs_renderer::SpotLamp { pos: world, dir, intensity: intensity * gain });
        }
    }

    /// **0x451D60**, the lamp's reflection on the road (read 2026-09-26), gated by `[0x6197AA]` (the
    /// `MODULATEALPHA` cap, taken as set, `thrash.md` §4). Arguments (0x43D4A2…0x43D4BE): the lamp's
    /// world centre `c`, the car's ground height `car+0x41C` (`[car+0xF4]+0x41C`), the size `|L+0x10|`
    /// (the image's root, `(bits >> 1) + 0x1FC00000`, 0x43CF93…0x43CFB7), the colour `L+0x40` and
    /// the strength `p = wet · b²`. With `e` the eye's height over the lamp (`eye.y − c.y`): below
    /// it (`e < 0`) `h = c.y − eye.y` and `e = 0`; else `h = ground − [0x61BCBC]` (0x451DCA…0x451DF6).
    /// The near edge lies across the road at `ground + 0.1` under the lamp: `c.xz, ground + 0.1 ∓
    /// r·size` with `r` the camera's row `+0xE8` (0x451DF9…0x451E91); the far edge is the near one
    /// less `(dx, h, dz) · 1.6·size` (`d = c − eye`, `[0x5B2B00]` 1.6) — toward the eye — scaled by
    /// the image's reciprocal root of `e` (`0x5F400000 − (bits >> 1)`) when `e > 1`
    /// (0x451EB2…0x451F56). The near pair's alpha is `ftol(p · 128) & 0xFF`, the far pair's
    /// `ftol(p · 64) & 0xFF`, the colour's low 24 bits (0x451F59…0x451F9B); UVs (0, 0) (1, 0) (1, 1)
    /// (0, 1); the depth bias `fistp(cam[+0xC] · −25000 / |c − eye|²)` (`[0x5B3D6C]`,
    /// 0x451FC7…0x451FFB) on `[0x61BCB4]`, blend pair 1 (additive), the alpha test off around it.
    fn lamp_reflection(c: Vec3, ground: f32, size: f32, colour: u32, p: f32, view: ReflectionEye) -> LampDraw {
        let ReflectionEye { eye, right, eye_ground, k } = view;
        let d = c - eye;
        let e = -d.y;
        let (h, e) = if e < 0.0 { (d.y, 0.0) } else { (ground - eye_ground, e) };
        let r = right * size;
        let y0 = ground + 0.1;
        let near = [Vec3::new(c.x - r.x, y0 - r.y, c.z - r.z), Vec3::new(c.x + r.x, y0 + r.y, c.z + r.z)];
        let s = size * 1.6;
        let mut off = Vec3::new(d.x * s, h * s, d.z * s);
        if e > 1.0 {
            off *= f32::from_bits(0x5F40_0000u32.wrapping_sub((e.to_bits() as i32 >> 1) as u32));
        }
        let rgb = [((colour >> 16) & 0xFF) as f32 / 255.0, ((colour >> 8) & 0xFF) as f32 / 255.0, (colour & 0xFF) as f32 / 255.0];
        let a_near = (((p * 128.0) as i32) & 0xFF) as f32 / 255.0;
        let a_far = (((p * 64.0) as i32) & 0xFF) as f32 / 255.0;
        let vert = |q: Vec3, uv: [f32; 2], a: f32| nfs_renderer::Vertex { position: q.into(), normal: [0.0, 1.0, 0.0], uv, color: [rgb[0], rgb[1], rgb[2], a] };
        let v = [
            vert(near[0], [0.0, 0.0], a_near),
            vert(near[1], [1.0, 0.0], a_near),
            vert(near[1] - off, [1.0, 1.0], a_far),
            vert(near[0] - off, [0.0, 1.0], a_far),
        ];
        let bias = (k * -25000.0 / d.length_squared().max(1e-6)).round_ties_even() as i32;
        LampDraw::Reflection(v, bias)
    }

    /// Lays a car's lamp draws into the scene in the order 0x43D800 made them — a beam takes the
    /// next mesh of the pool (as many beams as it holds), its vertices rewritten in place.
    pub(crate) fn push_lamp_draws(
        items: &mut Vec<nfs_renderer::DrawItem>,
        renderer: Option<&mut Renderer>,
        pools: &LampPools,
        used: &mut [usize; 2],
        draws: &mut Vec<LampDraw>,
        mut cones: Option<(&mut Vec<BeamLog>, u32)>,
    ) {
        let mut renderer = renderer;
        let mut n_beams = 0;
        for d in draws.drain(..) {
            match d {
                LampDraw::Glare(item) => items.push(item),
                LampDraw::Reflection(v, depth_bias) => {
                    let (Some(r), Some(mat), Some(&mesh)) = (renderer.as_deref_mut(), pools.reflection_material, pools.reflection.get(used[1])) else { continue };
                    used[1] += 1;
                    r.write_mesh_vertices(mesh, &v);
                    // texel × vertex colour, unlit; the alpha test off around it (0x24 ← 0 at
                    // 0x45200F, ← 10 at 0x452083) and the pass's depth writes off
                    items.push(nfs_renderer::DrawItem { layer_light: Some(unlit([1.0; 3])), depth_bias, ..nfs_renderer::DrawItem::new(mesh, mat, Mat4::IDENTITY) });
                }
                LampDraw::Beam(v, lamp) => {
                    let (Some(r), Some(mat), Some(&mesh)) = (renderer.as_deref_mut(), pools.beam_material, pools.beam.get(used[0])) else { continue };
                    used[0] += 1;
                    r.write_mesh_vertices(mesh, &v);
                    if let Some((log, key)) = cones.as_mut() {
                        log.push(BeamLog { key: *key * 16 + n_beams.min(15), mesh, verts: v, lamp });
                        n_beams += 1;
                    }
                    // the strip's colour is its vertices' (0x43D64D…0x43D7B1), unlit
                    items.push(nfs_renderer::DrawItem { layer_light: Some(unlit([1.0; 3])), ..nfs_renderer::DrawItem::new(mesh, mat, Mat4::IDENTITY) });
                }
            }
        }
    }

    /// The four corners of a car's shadow, world-space and draped on the ground — `R3DCar`'s
    /// slot 22, **0x43B220**, with the patch slot 23 (0x43B5B0) lays over them.
    ///
    /// 0x43B231…0x43B2F3 build the corners in body space out of nothing but the half extents at
    /// `car+0x3A8` and `car+0x3B0` — `(−ex, 0, −ez)`, `(+ex, 0, −ez)`, `(+ex, 0, +ez)`,
    /// `(−ex, 0, +ez)`, with no scale factor of any kind — and 0x532A80 turns them by the car's
    /// full basis `car+0x364` and moves them to `car+0x330`. The byte table at **0x5CD5E8** takes
    /// them in the order **3, 2, 0, 1** with the UVs at 0x5CD5F0, `(0,0) (1,0) (1,1) (0,1)`, and
    /// each corner's **y** is the ground under it: 0x43B6CF starts that query from the car's own
    /// `y` plus **`[0x5B36E8]` = 1.75**.
    ///
    /// **The patch** (0x43B8DC…0x43B957, slot 23 0x43B5B0): `k = fistp(dist² · [0x5B27A8] 0.01)` with
    /// `dist²` the car's square distance from the camera eye (`cam+0x10C`), capped at 6, and
    /// `n = 6 − k`, 1 when that is 0 — and always 6 in a replay (`[0x5E99F4] == 2`). With `n == 1`
    /// slot 22 draws the four corners; otherwise slot 23 lays an `(n+1) × (n+1)` grid over them:
    /// vertex `(i, j)` at `c0 + j·(c1 − c0)/n + i·(c3 − c0)/n` in x and z (0x43B619…0x43B665,
    /// 0x43B7B9…0x43B7F1), UV `(j/n, i/n)`, and its **y the ground under it** — the tracker at the
    /// vertex (0x474060) gives the patch plane (0x4745F0 / 0x474420 → 0x48A060) unless it is off
    /// the track or its height is 1.5 m (`[0x5B2CC0]`) or more from the car's own plane, which is
    /// then used instead (0x43B736…0x43B798). Returns `(n, vertices)`, `(n+1)²` of them row-major.
    ///
    /// **The heights, read 2026-09-26 (0x43B5E1…0x43B6C5, 0x43B731…0x43B79D).** The car's own
    /// plane is its tracker's patch — `Tracker::point(0)` 0x4745F0 and `Tracker::normal()`
    /// 0x474420 — or, with the tracker off every patch (`car+0x12`), the car's position under
    /// `(0, 1, 0)`. Each vertex `(x, car+0x334 + 1.75, z)` goes through its own tracker
    /// (`car+0x708 + 0x30·k`, `updateZone(…, 1)`); off every patch it takes the car's plane, else
    /// its patch's **reference point** `point(0)` is compared with the car's: within 1.5 m
    /// (`[0x5B2CC0]`, `|p0.y − car_p0.y|`) the vertex sits on that patch's plane, otherwise on the
    /// car's (0x48A060 evaluates `(n·p0 − n.x·x − n.z·z) / n.y`). MEDIUM: the port's patches are
    /// the primitives' triangles, so `point(0)` is a triangle's first corner, and the car's patch
    /// is looked up afresh at its position rather than read from its tracker.
    ///
    /// **The depth bias** (0x43B865…0x43B8BD): `fistp(−20000 · cam[+0xC] / d²)`, `d²` the car's
    /// square distance from the eye (all three axes), to Thrash state 0x18 — `b · 2^-15` on the
    /// game's screen z `far/(far − near) · (1 − near / Z)` (0x540CE0, `thrash.md` §6d.3),
    /// `cam[+0xC]` = `far · near / (far − near)` of the view's planes, that z's slope: some
    /// `20000 / 32768` ≈ 0.61 m of view depth toward the eye at the car. Returned with `n` and
    /// carried as `DrawItem::depth_bias` (`push_shadow`), as the skid marks' is. *(Until
    /// 2026-09-26 the port lifted the quad 0.04 m instead and compared the ground at the vertex
    /// with the car's plane; until 2026-10-02 (BIAS2) each vertex was pulled `b · (far − near) /
    /// 32768` of view depth, the linear reading of the screen z.)*
    pub(crate) fn shadow_quad(&self, pos: Vec3, rot: Quat, half: Vec3, out: &mut Vec<nfs_renderer::Vertex>) -> Option<(usize, i32)> {
        let w = self.world.as_ref()?;
        let body = [
            Vec3::new(-half.x, 0.0, -half.z),
            Vec3::new(half.x, 0.0, -half.z),
            Vec3::new(half.x, 0.0, half.z),
            Vec3::new(-half.x, 0.0, half.z),
        ];
        // the corner table 0x611570 holds the body corners in their own order (stride 0x10); the
        // 3, 2, 0, 1 of 0x5CD5E8 is only the order slot 22 draws them in
        let c: [Vec3; 4] = body.map(|b| pos + rot * b);
        let d2 = (pos - self.camera.position).length_squared();
        // 0x43B8DC…0x43B8EB: `fstp` to a float, then `fistp` — round to nearest even
        let k = ((d2 * 0.01).round_ties_even() as i64).min(6);
        let mut n = (6 - k).max(0) as usize;
        if n == 0 {
            n = 1;
        }
        // `cmp [0x5E99F4], 2 ; jne` (0x43B911): playing only — a stream run out (3) takes the
        // distance's level again. *(Until 2026-09-27 the port tested any playback.)*
        if self.replay.stream.as_ref().is_some_and(|r| r.mode == nfs_game::replay::Mode::Playing) {
            n = 6;
        }
        // a tracker's patch at `q`: the drivable triangle containing it in plan whose plane height
        // is nearest `q.y` (`updateZone`), as its reference point `point(0)` and its normal
        let patch_at = |q: Vec3| {
            w.collision
                .patches_at(q)
                .min_by(|x, y| (x.1 - q.y).abs().total_cmp(&(y.1 - q.y).abs()))
                .map(|(t, _)| {
                    let mut nrm = (t.b - t.a).cross(t.c - t.a).normalize_or_zero();
                    if nrm.y < 0.0 {
                        nrm = -nrm;
                    }
                    (t.a, nrm)
                })
        };
        let plane_y = |x: f32, z: f32, p: Vec3, nrm: Vec3| {
            if nrm.y.abs() > 1e-4 { p.y - ((x - p.x) * nrm.x + (z - p.z) * nrm.z) / nrm.y } else { p.y }
        };
        // the car's own plane: its patch's reference point and normal, else its position under
        // `(0, 1, 0)` (0x43B5E1…0x43B6C5)
        let (car_p0, car_n) = patch_at(pos).unwrap_or((pos, Vec3::Y));
        // the depth bias
        let z_near = self.camera.near;
        let far = self.view_far;
        // 0x43B89E…0x43B8B0: `cam[+0xC] · −20000 / d²` on the x87 stack (f64 stands for the extended
        // precision), `fstp` to a float, `fistp` — round to nearest even
        let bias = ((-20000.0 * (far * z_near / (far - z_near)) as f64 / d2.max(1e-6) as f64) as f32).round_ties_even();
        let n_f = n as f32;
        out.clear();
        // steps (entry 1 − entry 0) / n and (entry 3 − entry 0) / n (0x43B619…0x43B665); with n == 1
        // the grid is slot 22's four corners
        let (a, b) = (c[1] - c[0], c[3] - c[0]);
        let probe_y = pos.y + 1.75;
        for i in 0..=n {
            for j in 0..=n {
                let p = c[0] + a * (j as f32 / n_f) + b * (i as f32 / n_f);
                let y = match patch_at(Vec3::new(p.x, probe_y, p.z)) {
                    Some((p0, nrm)) if (p0.y - car_p0.y).abs() < 1.5 => plane_y(p.x, p.z, p0, nrm),
                    _ => plane_y(p.x, p.z, car_p0, car_n),
                };
                let q = Vec3::new(p.x, y, p.z);
                out.push(nfs_renderer::Vertex { position: q.into(), normal: [0.0, 1.0, 0.0], uv: [j as f32 / n_f, i as f32 / n_f], color: [1.0; 4] });
            }
        }
        Some((n, bias as i32))
    }

    /// Lays one car's shadow patch (`shadow_quad`) into the scene: each patch takes a dynamic mesh
    /// of its size from the pool for its `n` (1…6, each mesh `n × n` quads), rewritten in place —
    /// the grid moves with the car and with the ground under it, so a transform cannot carry it.
    /// 0x43B8C2…0x43B8D7: depth writes off (0x537870(0), Thrash 0x39) for the shadow, which the
    /// car's own parts, drawn after it, paint over — `early` keeps that order here.
    pub(crate) fn push_shadow(
        items: &mut Vec<nfs_renderer::DrawItem>,
        renderer: Option<&mut Renderer>,
        pools: &mut [Vec<nfs_renderer::MeshId>; 7],
        used: &mut [usize; 7],
        material: nfs_renderer::MaterialId,
        ((n, depth_bias), verts): ((usize, i32), &[nfs_renderer::Vertex]),
    ) {
        let Some(r) = renderer else { return };
        let pool = &mut pools[n];
        if pool.len() <= used[n] {
            let mut m = nfs_renderer::MeshData::default();
            m.vertices = verts.to_vec();
            let row = (n + 1) as u32;
            for i in 0..n as u32 {
                for j in 0..n as u32 {
                    let (v0, v1, v2, v3) = (i * row + j, i * row + j + 1, (i + 1) * row + j, (i + 1) * row + j + 1);
                    m.indices.extend_from_slice(&[v0, v2, v1, v1, v2, v3]);
                }
            }
            pool.push(r.upload_dynamic_mesh(&m));
        }
        let mesh = pool[used[n]];
        used[n] += 1;
        r.write_mesh_vertices(mesh, verts);
        // 0x43B820 builds its own primitive-helper `EAGL::Material` (0x59F070, vtable 0x5B3C94 —
        // the glares' and beams' helper, `particles.md` §4) with the default state object
        // `{1, 2, 0, 0, 0, −1}` from 0x5379F0 (gouraud, blending on) and a per-sprite depth bias at
        // its `+0x10` (0x537A60, the same `fistp(−20000 · cam[+0xC] / d²)` as the glare's), then
        // draws the "SHAD" texture (0x44D320 by FourCC) over vertices copied from a cached ring
        // table — no read anywhere in it of the car's light block or of any `.map` / ambient
        // global, so its colour and alpha are the material's own constant, not the car's light nor
        // the `.map` ambient (`nfs_game::world::TRACK_LIGHT`, for the same "no light of its own").
        // The strength is the `SHAD` texel's own alpha (at most 136 / 255, black texels) times
        // the vertex colour, which is white in both paths of 0x43B820 (`[ebp−0x2C…−0x20]` = −1,
        // 0x43B842…0x43B84E; the race's quad grid reads the same four words, 0x43B972 → `+0xB8`),
        // so `alpha` stays 1: the 0.9 this carried was the first shadow's stand-in (the HUD
        // commit's `shadow_item`), read from nowhere. The blend is the default pair 0
        // (`SRCALPHA` / `INVSRCALPHA`, state object `{1, 2, 0, 0, 0, −1}` of 0x5379F0 — it names
        // no pair of its own). The alpha test is the flush's `GREATER 10` (Thrash 0x24 ← 10 at
        // 0x466703 before the list `+0xC` the cars' slot 2 0x437AB0 is drawn from; 0x43B820 sets
        // no state of its own): a texel (× the white vertex) of alpha ≤ 10 is dropped.
        items.push(nfs_renderer::DrawItem {
            layer_light: Some(nfs_game::world::TRACK_LIGHT),
            alpha_test: true,
            alpha_ref: 10,
            early: true,
            depth_bias,
            // ENHANCEMENT E9: fades where the sun's shadow map draws the car's own shadow (inert without E9)
            csm: nfs_renderer::CSM_CAR_BLOB,
            ..nfs_renderer::DrawItem::new(mesh, material, Mat4::IDENTITY)
        });
    }

    /// `[0x61BCBC]`, which 0x4520A0 sets every race frame (0x468AE6): the camera tracker
    /// `0x627F5C` moved to the eye (0x474060), and unless it is off every patch (`[0x627F66]`, then
    /// 0) the height of its patch's plane at the eye (0x4745F0 / 0x474420 → 0x48A060) plus
    /// `[0x5B2604]` 0.1. MEDIUM: the tracker's patch is taken as the drivable triangle under the
    /// eye whose plane height is nearest the eye's, as `shadow_quad`'s.
    fn eye_road_height(&self) -> f32 {
        let Some(w) = self.world.as_ref() else { return 0.0 };
        let q = self.camera.position;
        w.collision
            .patches_at(q)
            .min_by(|x, y| (x.1 - q.y).abs().total_cmp(&(y.1 - q.y).abs()))
            .map_or(0.0, |(_, h)| h + 0.1)
    }
}

/// ENHANCEMENT E24: the fade-in time of a car's lamps at a tunnel mouth, ms.
pub(crate) const TUNNEL_LAMP_IN: f64 = 350.0;
/// ENHANCEMENT E24: the fade-out time after the car leaves the covered stretch, ms.
pub(crate) const TUNNEL_LAMP_OUT: f64 = 700.0;

/// a covered state that drops for less than this (ms) is a gap in the road's overhead flags, not a tunnel's
/// end (castle: one tick inside a tunnel reads uncovered), so the fade does not start.
pub(crate) const TUNNEL_HOLD: f64 = 250.0;

/// ENHANCEMENT E24: every car's render-side tunnel lamp value (key: 0 the camera's car, 1 + i opponent i, 0x100 + i
/// traffic, 0x200 + i police) and the lamp-clock time it was last stepped to. Never part of a car's state.
#[derive(Default)]
pub(crate) struct TunnelLamps {
    cars: std::collections::HashMap<u32, (f32, f64)>,
    /// the lamp-clock time each car was last seen covered ([`Self::value_held`])
    covered_at: std::collections::HashMap<u32, f64>,
}

impl TunnelLamps {
    /// [`Self::value`] with the covered state held for [`TUNNEL_HOLD`] ms after the last time it was true.
    pub(crate) fn value_held(&mut self, key: u32, covered: bool, now_ms: f64) -> f32 {
        if covered {
            self.covered_at.insert(key, now_ms);
        }
        let held = covered || self.covered_at.get(&key).is_some_and(|&t| (0.0..=TUNNEL_HOLD).contains(&(now_ms - t)));
        self.value(key, held, now_ms)
    }

    /// The car's value at `now_ms`, stepped from the last call toward its target (1 when `covered`, else 0). A first
    /// call, a clock that went back (a restart) or a jump of more than a second (a pause, a teleport) snaps to the target.
    pub(crate) fn value(&mut self, key: u32, covered: bool, now_ms: f64) -> f32 {
        let target = if covered { 1.0 } else { 0.0 };
        let e = self.cars.entry(key).or_insert((target, now_ms));
        let dt = now_ms - e.1;
        if !(0.0..=1000.0).contains(&dt) {
            e.0 = target;
        } else if covered {
            e.0 = (e.0 + (dt / TUNNEL_LAMP_IN) as f32).min(1.0);
        } else {
            e.0 = (e.0 - (dt / TUNNEL_LAMP_OUT) as f32).max(0.0);
        }
        e.1 = now_ms;
        e.0
    }
}

/// ENHANCEMENT E15: the spotlights' downward tilt about the car's right axis, degrees (a headlight's cut-off).
pub(crate) const SPOT_PITCH_DEG: f32 = 3.0;

/// ENHANCEMENT E15 (ENH-CONES): a beam strip written this frame, kept to be faded once the frame's spots are chosen.
pub(crate) struct BeamLog {
    /// the car's key * 16 + the beam's number in the car: the fade ramp's key
    pub(crate) key: u32,
    pub(crate) mesh: nfs_renderer::MeshId,
    pub(crate) verts: [nfs_renderer::Vertex; 18],
    /// the lamp's world centre, as `spot_lamps` computes it for the same lamp
    pub(crate) lamp: Vec3,
}

/// One draw of a car's effect pass 0x43D800, in the order it makes them (`App::lamp_draws`): a
/// lamp's glare sprite (0x43CDD0), its road reflection (0x451D60, the quad and its depth bias) or
/// its night beam's 18-vertex strip (0x43D4F0).
pub(crate) enum LampDraw {
    Glare(nfs_renderer::DrawItem),
    Reflection([nfs_renderer::Vertex; 4], i32),
    Beam([nfs_renderer::Vertex; 18], Vec3),
}

/// The camera's side of 0x451D60 (`App::lamp_reflection`): the eye, its row `+0xE8` (`right`), the
/// eye's road height `[0x61BCBC]` (`eye_ground`) and the depth-bias factor `cam[+0xC]` (`k`).
#[derive(Clone, Copy)]
struct ReflectionEye {
    eye: Vec3,
    right: Vec3,
    eye_ground: f32,
    k: f32,
}

/// One view pass's inputs for `App::dyn_items`: the frame's interpolation and the player's pose,
/// whether it is the mirror's pass (with its frustum, `CullView`) and the view's screen for the LOD
/// pick — the viewport's size and the render width the size is scaled by (0x43A2D7).
#[derive(Clone, Copy)]
pub(crate) struct DynPose {
    pub(crate) alpha: f32,
    pub(crate) local_draw: Vec3,
    pub(crate) local_pos: Vec3,
    pub(crate) local_rot: Quat,
    pub(crate) mirror_pass: bool,
    pub(crate) cull: Option<nfs_game::world::CullView>,
    pub(crate) lod: (f32, f32),
    pub(crate) ref_width: f32,
}

/// How many meshes of each dynamic pool a pass has taken (`App::dyn_items`): the mirror's pass
/// starts where the main one stopped, so no mesh is rewritten under an item the main view queued.
#[derive(Clone, Copy, Default)]
pub(crate) struct PoolUse {
    pub(crate) shadow: [usize; 7],
    pub(crate) lamp: [usize; 2],
    pub(crate) skid: usize,
}

/// The dynamic meshes and materials the lamp draws are laid into: the headlight beams' (0x43D4F0:
/// a strip of 18 vertices each, untextured and blended) and the road reflections' (0x451D60).
#[derive(Default)]
pub(crate) struct LampPools {
    pub(crate) beam: Vec<nfs_renderer::MeshId>,
    pub(crate) beam_material: Option<nfs_renderer::MaterialId>,
    pub(crate) reflection: Vec<nfs_renderer::MeshId>,
    pub(crate) reflection_material: Option<nfs_renderer::MaterialId>,
}

pub(crate) fn lamp_command(bits: u8, code: u8) -> u8 {
    match code {
        10 => {
            let mut b = bits;
            if b & 0x10 != 0 {
                b ^= 0x10;
                if b & 8 != 0 {
                    return b;
                }
            }
            b ^ 8
        }
        11 => {
            let mut b = bits;
            if b & 8 != 0 {
                b ^= 8;
                if b & 0x10 != 0 {
                    return b;
                }
            }
            b ^ 0x10
        }
        12 if bits & 0x18 == 0x18 => bits ^ 0x18,
        12 => bits | 0x18,
        13 => bits ^ 3,
        15 => bits ^ 4,
        _ => bits,
    }
}

#[cfg(test)]
mod lamp_tests {
    use super::{App, HeadRamp, TUNNEL_HOLD, TUNNEL_LAMP_IN, TUNNEL_LAMP_OUT, TunnelLamps, lamp_command};

    const N: (bool, bool, bool) = (false, false, false);

    /// 0x439360: the tail lamps glow at 0.6 at night without the brake, full with it; gear 0 lights
    /// the reverse slot; a damage zone over 30 breaks its lamps (0x4398C6…).
    #[test]
    fn tunnel_lamps_fade_in_and_out_and_snap() {
        let mut t = TunnelLamps::default();
        // the first call snaps to the target (a still taken inside has its lamps lit)
        assert_eq!(t.value(0, true, 1000.0), 1.0);
        assert_eq!(t.value(1, false, 1000.0), 0.0);
        // leaving: falls over TUNNEL_LAMP_OUT, no jump
        let v = t.value(0, false, 1000.0 + TUNNEL_LAMP_OUT / 2.0);
        assert!((v - 0.5).abs() < 1e-4, "{v}");
        assert_eq!(t.value(0, false, 1000.0 + TUNNEL_LAMP_OUT), 0.0);
        // entering: rises over TUNNEL_LAMP_IN; the same time again changes nothing (the mirror's pass)
        assert_eq!(t.value(0, false, 2500.0), 0.0);
        let a = t.value(0, true, 2500.0 + TUNNEL_LAMP_IN / 2.0);
        assert!((a - 0.5).abs() < 1e-4, "{a}");
        assert_eq!(t.value(0, true, 2500.0 + TUNNEL_LAMP_IN / 2.0), a);
        // a clock that went back snaps
        assert_eq!(t.value(0, false, 10.0), 0.0);
    }

    /// a one-tick gap in the covered bit inside a tunnel changes nothing; a real exit fades after the hold.
    #[test]
    fn tunnel_gap_is_held() {
        let mut t = TunnelLamps::default();
        assert_eq!(t.value_held(0, true, 1000.0), 1.0);
        // a gap of two frames (31 ms apart): still 1
        assert_eq!(t.value_held(0, false, 1031.0), 1.0);
        assert_eq!(t.value_held(0, false, 1062.0), 1.0);
        assert_eq!(t.value_held(0, true, 1093.0), 1.0);
        // a real exit: the value starts to fall only after TUNNEL_HOLD, then falls over TUNNEL_LAMP_OUT
        assert_eq!(t.value_held(0, false, 1093.0 + 200.0), 1.0);
        let v = t.value_held(0, false, 1093.0 + TUNNEL_HOLD + TUNNEL_LAMP_OUT / 2.0 + 1.0);
        assert!(v < 0.95 && v > 0.3, "{v}");
        assert_eq!(t.value_held(0, false, 1093.0 + TUNNEL_HOLD + 2.0 * TUNNEL_LAMP_OUT), 0.0);
    }

    #[test]
    fn tunnel_slots_light_only_what_the_car_leaves_dark() {
        const N: (bool, bool, bool) = (false, false, false);
        let off = App::lamp_slots(false, 0.0, false, false, N, None);
        assert_eq!(App::lamp_slots_tunnel(0.0, false, 0.0, false, false, N, None), off);
        let half = App::lamp_slots_tunnel(0.5, false, 0.0, false, false, N, None);
        assert_eq!(half[0], (true, 0.5));
        assert_eq!(half[2], (true, 0.3));
        // lamps already on by the car's own byte: untouched
        assert_eq!(App::lamp_slots_tunnel(1.0, true, 0.7, false, false, N, None), App::lamp_slots(true, 0.7, false, false, N, None));
        // a broken lamp stays dark, a lit brake lamp stays as it was
        let d = [40.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        assert!(!App::lamp_slots_tunnel(1.0, false, 0.0, false, false, N, Some(&d))[0].0);
        assert_eq!(App::lamp_slots_tunnel(1.0, false, 0.0, true, false, N, None)[2], (true, 1.0));
    }

    #[test]
    fn lamp_slots_glow_and_break() {
        let s = App::lamp_slots(true, 1.0, false, false, N, None);
        assert_eq!(s[0], (true, 1.0));
        // the ramp scales the headlights only
        let s = App::lamp_slots(true, 0.25, false, false, N, None);
        assert_eq!((s[0], s[1], s[2]), ((true, 0.25), (true, 0.25), (true, 0.6)));
        assert_eq!(s[2], (true, 0.6));
        assert_eq!(App::lamp_slots(false, 1.0, true, true, N, None)[3], (true, 1.0));
        assert!(App::lamp_slots(false, 1.0, true, true, N, None)[8].0);
        let mut d = [0.0f32; 10];
        d[0] = 31.0;
        let s = App::lamp_slots(true, 1.0, false, false, N, Some(&d));
        assert!(!s[0].0 && s[1].0);
        // an eliminated car's hazards: the front pair on one phase, the rear pair on the other
        let s = App::lamp_slots(false, 1.0, false, false, (true, true, true), None);
        assert!(s[4].0 && s[5].0 && !s[6].0 && !s[7].0);
        // 0xA / 0xB: the indicator's phase over the brake; 0.6 with the headlights and no brake
        assert_eq!((s[10], s[11]), ((true, 1.0), (true, 1.0)));
        let s = App::lamp_slots(false, 1.0, false, false, (true, true, false), None);
        assert!(!s[10].0 && !s[11].0);
        assert_eq!(App::lamp_slots(true, 1.0, false, false, N, None)[10], (true, 0.6));
        assert_eq!(App::lamp_slots(false, 1.0, true, false, N, None)[11], (true, 1.0));
        assert!(!App::lamp_slots(false, 1.0, false, false, N, None)[10].0);
        // the sirens 0xC / 0xD always, until zone 8 breaks them
        let s = App::lamp_slots(false, 1.0, false, false, N, None);
        assert_eq!((s[12], s[13]), ((true, 1.0), (true, 1.0)));
        let mut d = [0.0f32; 10];
        d[8] = 31.0;
        d[6] = 31.0;
        let s = App::lamp_slots(false, 1.0, true, false, N, Some(&d));
        assert!(!s[12].0 && !s[13].0 && !s[10].0 && s[11].0);
        // one indicator: 4 and 6 for the left, 5 and 7 for the right (0x43942A…0x439480)
        let s = App::lamp_slots(false, 1.0, false, false, (false, true, false), None);
        assert!(!s[4].0 && !s[5].0 && !s[6].0 && s[7].0);
    }

    /// 0x499365…0x49942F: the signals toggle and cancel each other, the hazards set or clear both,
    /// the lights flip bits 0 | 1.
    #[test]
    fn the_lamp_commands() {
        assert_eq!(lamp_command(0, 10), 8);
        assert_eq!(lamp_command(8, 10), 0);
        assert_eq!(lamp_command(0x10, 10), 8, "right on: off, then left toggled on");
        assert_eq!(lamp_command(0x18, 10), 8, "the hazards become the left signal");
        assert_eq!(lamp_command(8, 11), 0x10);
        assert_eq!(lamp_command(0x18, 11), 0x10);
        assert_eq!(lamp_command(0, 12), 0x18);
        assert_eq!(lamp_command(8, 12), 0x18);
        assert_eq!(lamp_command(0x18, 12), 0);
        assert_eq!(lamp_command(3, 13), 0);
        assert_eq!(lamp_command(0, 13), 3);
        assert_eq!(lamp_command(3, 15), 7);
        assert_eq!(lamp_command(3, 14), 3);
        assert_eq!(lamp_command(3, 2), 3);
    }

    /// 0x43950E…0x4396A1: a second toggle while the ramp runs changes nothing until the ramp is
    /// done; one after it ramps from where the lamps are.
    #[test]
    fn the_headlight_ramp() {
        let mut r = HeadRamp::default();
        assert_eq!(r.value(false, 0.0), 0.0);
        assert_eq!(r.value(true, 0.0), 0.0);
        assert_eq!(r.value(true, 500.0), 0.5);
        // off while it rises: the flag follows, the ramp does not turn
        assert_eq!(r.value(false, 600.0), 0.6);
        assert_eq!(r.value(false, 1500.0), 1.0);
        // back on after it ended: a new ramp from 1 to 1 — and off inside it is lost, so the lamps
        // stay lit with the bit clear until the bit changes again
        assert_eq!(r.value(true, 1600.0), 1.0);
        assert_eq!(r.value(false, 2000.0), 1.0);
        assert_eq!(r.value(false, 3500.0), 1.0);
        assert_eq!(r.value(true, 4000.0), 1.0);
        assert_eq!(r.value(false, 5100.0), 1.0);
        assert_eq!(r.value(false, 5600.0), 0.5);
        assert_eq!(r.value(false, 7000.0), 0.0);
    }
}
