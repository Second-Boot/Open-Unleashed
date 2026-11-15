//! The frame's globals and the items' object uniforms as functions of their inputs: `encode_frame`
//! gathers what they read of the `Core` (its enhancement set, the float scene, the cascades of this frame, ...) and these fill
//! the structs, so the unit tests below can check without a GPU that with [`crate::Enhancements::ORIGINAL`] every field an
//! enhancement owns holds master's value. The expressions are `encode_frame`'s, moved; `self.<x>` became the input `<x>`.

use super::*;
use crate::{DrawItem, Enhancements, RainEnv, WaterEnv, classes};

/// The enhancement inputs the globals and the objects share, derived once per frame from the scene and the switches.
#[derive(Clone, Copy)]
pub(super) struct FrameEnv<'a> {
    /// ENHANCEMENT E17: the water's sky / sun, only with the switch on
    pub water: Option<WaterEnv>,
    /// ENHANCEMENT E26: the wet road's sky and lamps, only with the switch on
    pub rain: Option<&'a RainEnv>,
    /// ENHANCEMENT E18: the particles' HDR / sun lighting, on the float scene only
    pub p_fx: bool,
    /// ENHANCEMENT E11: the float scene and the switch
    pub hdr_boost: bool,
}

impl<'a> FrameEnv<'a> {
    /// `hdr`: `Core::cur_hdr` (the float scene's level, `None` = master's 8-bit target).
    pub(super) fn new(scene: &'a Scene, enh: &Enhancements, hdr: Option<u32>) -> Self {
        Self {
            water: scene.water.filter(|_| enh.is_on(crate::E17)),
            rain: scene.rain.as_ref().filter(|_| enh.is_on(crate::E26)),
            p_fx: hdr.is_some() && enh.is_on(crate::E18),
            hdr_boost: hdr.is_some() && enh.is_on(crate::E11),
        }
    }
}

/// What [`globals_uniform`] reads besides the scene and the camera.
#[derive(Clone, Copy)]
pub(super) struct GlobalsFrame<'a> {
    pub scene: &'a Scene,
    pub camera: &'a Camera,
    /// the main view's clip transform (ENHANCEMENT ENH-TEMPORAL: with the sub-pixel jitter in a temporal main view)
    pub vp_j: glam::Mat4,
    pub enh: &'a Enhancements,
    /// `Core::cur_hdr`
    pub hdr: Option<u32>,
    pub no_reflections: bool,
    /// ENHANCEMENT E8: all six faces of the environment cube are drawn (`ReflCube::faces_done == ALL_FACES`)
    pub refl_faces_done: bool,
    /// ENHANCEMENT E16: `Core::probe_strength` of the scene
    pub probe_strength: f32,
    /// ENHANCEMENT E12: the main pass is split for the soft sprites
    pub soft_split: bool,
    pub env: FrameEnv<'a>,
    /// ENHANCEMENT ENH-TEMPORAL: the jitter in NDC and the render size of a temporal main view (`None` = master's frame)
    pub temporal: Option<([f32; 2], (u32, u32))>,
    /// ENHANCEMENT E49: the near lookup (`None` = off, master's zeros)
    pub near: Option<NearFrame>,
}

/// `GlobalsUniform::ambient.w`: whether the fragment output keeps values above 1 — ENHANCEMENT E7's ACES level (`hdr` 1), or
/// the float scene with E10 (bloom), E11 (bright sun) or E18 (particles in HDR), which read the overshoots of the additive
/// layers. `false` (master's 0, `hdr` `None`): the output is held to 0...1 as an 8-bit target does.
fn keeps_overbright(hdr: Option<u32>, enh: &Enhancements, p_fx: bool) -> bool {
    hdr == Some(1) || (hdr.is_some() && (enh.is_on(crate::E10) || enh.is_on(crate::E11) || p_fx))
}

/// The frame's [`GlobalsUniform`].
pub(super) fn globals_uniform(f: &GlobalsFrame) -> GlobalsUniform {
    let GlobalsFrame { scene, camera, vp_j, enh, hdr, no_reflections, refl_faces_done, probe_strength, soft_split, env, temporal, near } = *f;
    let (water_env, rain_env, p_fx) = (env.water, env.rain, env.p_fx);
    GlobalsUniform {
        view_proj: vp_j.to_cols_array_2d(),
        view: camera.view().to_cols_array_2d(),
        sun_dir: scene.sun_direction.extend(0.0).to_array(),
        // .y the view's near plane (no longer read: the vertex stage holds a depth-bias pull at the clip's own near plane, the
        // original's screen z going to 0 or below — in front of everything, `RE/notes/thrash.md` §6)
        // .z 1 in the rear-view mirror's view: no specular or sky-map pass (`[0x6115CC]+0x190`)
        // .w ENHANCEMENT E7: 1 = a float target that keeps values above 1 (the ACES tone map);
        // 0 = the fragment output is held to 0...1 as an 8-bit target does
        // ENHANCEMENT E10 / E11: bloom and the bright sun need the values above 1 as well, so the
        // clamp is released at level 1 too (an 8-bit surface clamps them again at the end: only the
        // overshoots of the additive layers differ, and the bloom reads them)
        ambient: [scene.ambient, camera.near, no_reflections as u32 as f32, keeps_overbright(hdr, enh, p_fx) as u32 as f32],
        cam_pos: camera.position.extend(1.0).to_array(),
        headlight: scene.headlight.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
        headlight_on: [scene.headlight.is_some() as u32 as f32, (scene.headlight.is_some() && scene.headlight_decal) as u32 as f32, 1.0 - scene.headlight_gain.clamp(0.0, 1.0), 0.0],
        beacon: scene.beacon.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d(),
        beacon_on: [scene.beacon.is_some() as u32 as f32, 0.0, 0.0, 0.0],
        fog: scene.fog.map_or([0.0; 4], |f| [f.colour[0], f.colour[1], f.colour[2], f.density]),
        sky_fog: scene.sky_fog.map_or([0.0; 4], |f| [f.colour[0], f.colour[1], f.colour[2], f.density]),
        // ENHANCEMENT E8: only with the switch on and a face drawn; 0 is master's value
        refl: if enh.is_on(crate::E8) && refl_faces_done { [scene.refl, 1.0 / REFL_TAN_HALF, probe_strength, crate::coat::level_alpha(enh.level(crate::E8))] } else { [0.0; 4] },
        // ENHANCEMENT E12: the planes the stored depth is read through
        soft: [camera.near, camera.far, soft_split as u32 as f32, 0.0],
        // ENHANCEMENT E17: only with the switch on and the app's sky / sun for the water; 0 is master's value
        water: water_env.map_or([0.0; 4], |w| [1.0, (w.sun != glam::Vec3::ZERO) as u32 as f32, w.use_env_map as u32 as f32, (enh.level(crate::E17) >= crate::Enhancements::E17_STRONGER) as u32 as f32]),
        water_zenith: water_env.map_or([0.0; 4], |w| [w.zenith[0], w.zenith[1], w.zenith[2], 0.0]),
        water_horizon: water_env.map_or([0.0; 4], |w| [w.horizon[0], w.horizon[1], w.horizon[2], 0.0]),
        water_sun: water_env.map_or([0.0; 4], |w| w.sun.extend(0.0).to_array()),
        // ENHANCEMENT E18: the sun the cars and shadows use (`ShadowLights::sun`, a day track's main view)
        fx_sun: scene.shadows.sun.filter(|s| p_fx && s.focus.is_none()).map_or([0.0; 4], |s| (-s.direction).extend(1.0).to_array()),
        // ENHANCEMENT E19: the fog's sun glow, only with the switch on and a sun (`ShadowLights::sun`: none on a night /
        // sunless track, and not in the environment faces); 0 is master's value
        fogsun: scene.shadows.sun.filter(|s| enh.is_on(crate::E19) && s.focus.is_none()).map_or([0.0; 4], |s| (-s.direction).normalize_or_zero().extend(FOG_SUN_STRENGTH).to_array()),
        // ENHANCEMENT E26: only with the switch on and a wet track; 0 is master's value
        rain: rain_env.map_or([0.0; 4], |r| [crate::rain_gain(enh.level(crate::E26)), r.time, r.n_lamps.min(crate::RAIN_LAMPS) as f32, r.n_edges.min(crate::RAIN_EDGES) as f32]),
        rain_zenith: rain_env.map_or([0.0; 4], |r| [r.zenith[0], r.zenith[1], r.zenith[2], 0.0]),
        rain_horizon: rain_env.map_or([0.0; 4], |r| [r.horizon[0], r.horizon[1], r.horizon[2], 0.0]),
        rain_lamp_pos: {
            let mut a = [[0.0; 4]; crate::RAIN_LAMPS];
            // w (AUTOBAHN-LAMPS): the light's reach squared, 0 = the shader's default (`RAIN_LAMP_REACH2`)
            for (d, (p, _, reach)) in a.iter_mut().zip(rain_env.iter().flat_map(|r| r.lamps[..r.n_lamps.min(crate::RAIN_LAMPS)].iter())) {
                *d = [p.x, p.y, p.z, reach * reach];
            }
            a
        },
        rain_lamp_col: {
            let mut a = [[0.0; 4]; crate::RAIN_LAMPS];
            for (d, (_, c, _)) in a.iter_mut().zip(rain_env.iter().flat_map(|r| r.lamps[..r.n_lamps.min(crate::RAIN_LAMPS)].iter())) {
                *d = [c[0], c[1], c[2], 0.0];
            }
            a
        },
        rain_edge_pos: {
            let mut a = [[0.0; 4]; crate::RAIN_EDGES];
            for (d, e) in a.iter_mut().zip(rain_env.iter().flat_map(|r| r.edges[..r.n_edges.min(crate::RAIN_EDGES)].iter())) {
                *d = [e.pos.x, e.pos.y, e.pos.z, e.half];
            }
            a
        },
        rain_edge_dir: {
            let mut a = [[0.0; 4]; crate::RAIN_EDGES];
            for (d, e) in a.iter_mut().zip(rain_env.iter().flat_map(|r| r.edges[..r.n_edges.min(crate::RAIN_EDGES)].iter())) {
                *d = [e.dir.x, e.dir.y, e.dir.z, e.before];
            }
            a
        },
        rain_edge_val: {
            let mut a = [[0.0; 4]; crate::RAIN_EDGES];
            for (d, e) in a.iter_mut().zip(rain_env.iter().flat_map(|r| r.edges[..r.n_edges.min(crate::RAIN_EDGES)].iter())) {
                *d = [e.after, 0.0, 0.0, 0.0];
            }
            a
        },
        // ENHANCEMENT ENH-TEMPORAL: 0 = master's value
        temporal: temporal.map_or([0.0; 4], |(jitter_ndc, size)| [jitter_ndc[0], jitter_ndc[1], size.0 as f32, size.1 as f32]),
        // ENHANCEMENT E49: only with the switch on and the twelve-layer cube drawn; 0 is master's value
        // (.x 1 the original's sky in the overlay, 2 NEAR-REFL3's live sky (a night track), 3 NEAR-REFL4's live hue (a day / dusk
        // track); .w TEST GLUE `NFS_NEAR_DEBUG`: the paint shows the reflection alone)
        near: near.map_or([0.0; 4], |n| [match n.sky { crate::NearSky::Original => classes::NEAR_SKY_ORIGINAL, crate::NearSky::Night => classes::NEAR_SKY_NIGHT, crate::NearSky::Hue => classes::NEAR_SKY_HUE }, n.own as u32 as f32, n.proxy.map_or(0.0, |p| p.1), n.debug as f32]),
        near_box: near.and_then(|n| n.proxy).map_or([[0.0; 4]; 4], |(b, _)| {
            [b.centre.extend(b.half_len).to_array(), b.forward.extend(b.half_wid).to_array(), b.lateral.extend(NEAR_BOX_FLOOR).to_array(), b.up.extend(b.ceil).to_array()]
        }),
        near_eye: near.map_or([[0.0; 4]; 7], |n| std::array::from_fn(|k| n.eyes.get(k).copied().unwrap_or(n.eye).extend(1.0).to_array())),
    }
}

/// What the object fill reads of a material: [`GpuMaterial`] without its GPU objects.
#[derive(Clone, Copy)]
pub(super) struct MatFill {
    pub env_mix: f32,
    pub lod_bias: f32,
    pub device_states: bool,
    pub depth_bias: i32,
    pub blend_pair: u8,
    pub reflect: crate::Reflect,
}

impl GpuMaterial {
    pub(super) fn fill(&self) -> MatFill {
        MatFill { env_mix: self.env_mix, lod_bias: self.lod_bias, device_states: self.device_states, depth_bias: self.depth_bias, blend_pair: self.blend_pair, reflect: self.reflect }
    }
}

/// ENHANCEMENT E9: what an item's uniform takes from this frame's cascades (`CsmFrame::car_patch_gate` / `decal_fades`).
#[derive(Clone, Copy)]
pub(super) struct CsmFill {
    /// how much of a car's `SHAD` patch the sun's shadow takes in the open (the cover gate; 0 = the patch stays whole); SHADOW-REAL:
    /// scaled by the patch's own openness, `1 - DrawItem::cover`
    pub car_patch_gate: f32,
    pub decal_fades: bool,
}

/// What [`object_uniform`] reads besides the item and its material.
#[derive(Clone, Copy)]
pub(super) struct ObjectFrame<'a> {
    pub camera: &'a Camera,
    /// the far plane the game's depth bias is scaled with (`encode_frame`'s `depth_far`)
    pub depth_far: f32,
    /// ENHANCEMENT E9: this frame's cascades (`None` = master's frame, no pass)
    pub csm: Option<CsmFill>,
    /// ENHANCEMENT E12: the main pass is split for the soft sprites
    pub soft_split: bool,
    /// ENHANCEMENT E15 / E23: this frame has headlight spots or street lamps
    pub spots: bool,
    /// ENHANCEMENT E43: this frame's contact shadows
    pub contact_on: bool,
    pub env: FrameEnv<'a>,
    /// ENHANCEMENT ENH-TEMPORAL: the items' roles for the masks and their reprojection matrices (empty in master's frame)
    pub roles: &'a [f32],
    pub motion_index: &'a [u32],
    /// the frame's clip transform ([`ObjectUniform::mvp`], PORT-DEVIATION D37): `globals.view_proj`, in `f64`
    pub vp_j: glam::DMat4,
    /// ENHANCEMENT UPSCALE-SOFT: added to the materials' LOD bias in an upscaler's main view (`upscale::mip_bias`); 0 = master's
    pub lod_shift: f32,
    /// ENHANCEMENT E49: the player's faces (the near cars) are drawn: a [`DrawItem::near_env`] item's `refl.w` is 1 (false = 0, master's)
    pub near_own: bool,
}

/// ENHANCEMENT E49: the near lookup's inputs of a frame (`Core::near_frame`).
#[derive(Clone, Copy, Debug)]
pub(super) struct NearFrame {
    /// all six of the player's faces (layers 6..11) are drawn
    pub own: bool,
    /// the eye each face was drawn from, and the newest
    pub eyes: [glam::Vec3; 6],
    pub eye: glam::Vec3,
    /// the covered proxy box at the eye and its weight (`gpu::near_proxy`); `None` in the open
    pub proxy: Option<(crate::CoverBox, f32)>,
    /// NEAR-REFL3 / NEAR-REFL4: what the overlay's sky parts show (`Scene::near_sky`)
    pub sky: crate::NearSky,
    /// TEST GLUE (`NFS_NEAR_DEBUG`): the paint shows the reflection alone (a still that makes the lookup visible)
    pub debug: u8,
}

/// The per-item receivers the fill asks only when their effect is on this frame (`Core::csm_receive`: E9's role, which E15 / E23
/// take too, asked at most once an item; `contact_receive`).
pub(super) struct Receivers<'a> {
    pub csm: &'a dyn Fn(&DrawItem) -> f32,
    pub contact: &'a dyn Fn(&DrawItem) -> bool,
}

/// Item `i`'s [`ObjectUniform`]; `sky_tex`: the item's sky-map pass has its section map (`sky_tex_of`).
pub(super) fn object_uniform(f: &ObjectFrame, i: usize, item: &DrawItem, mat: Option<MatFill>, sky_tex: bool, rx: &Receivers) -> ObjectUniform {
    let ObjectFrame { camera, depth_far, csm: csm_frame, soft_split, spots, contact_on, env, roles, motion_index, vp_j, lod_shift, near_own } = *f;
    let (water_env, rain_env, p_fx, hdr_boost) = (env.water, env.rain, env.p_fx, env.hdr_boost);
    let mat = mat.as_ref();
    let env_mix = mat.map_or(0.0, |m| m.env_mix);
    // ENHANCEMENT UPSCALE-SOFT: plus the upscaler's shift (none = the material's own value, bit for bit)
    let lod_bias = mat.map_or(0.0, |m| if lod_shift == 0.0 { m.lod_bias } else { m.lod_bias + lod_shift });
    // the alpha test: the material's race states, or the draw's own Thrash 0x24 ← 10
    let alpha_test = mat.is_some_and(|m| m.device_states) || item.alpha_test;
    // `.w` below: 0 no test, else the reference + ½ — a texel alpha under it (≤ the
    // reference in bytes) fails `GREATER`
    let alpha_ref = if alpha_test { item.alpha_ref as f32 + 0.5 } else { 0.0 };
    // state 0x18 as an offset of the screen z: `b · 2^-15` (0x540500's `× [0x5B3E10]`)
    // added to the game's `sz = far/(far − near) · (1 − near / Z)` (0x540CE0) — `b` the
    // material's block plus the draw's own (`DrawItem::depth_bias`) — with near / far the
    // pass's own planes, which 0x466F30 hands the projection 0x5391B0 (`cam+0xCC` /
    // `+0xD0`, folded into `cam+0xDC` / `+0xE0`): the main view's near and the camera
    // object's far 700 (0x4677FC), the rear view's 1 / 65000 (0x4691D0;
    // `RE/notes/thrash.md` §6d.3). The shader adds it to the port's own clip z, so it is
    // scaled by the ratio of the two `far / (far − near)` — exactly 1 where the port's
    // planes are the pass's (the race's main view and its mirror), and keeping the same
    // view-depth step `≈ b · 2^-15 · Z² / (near · A)` where they are not.
    let a_game = depth_far / (depth_far - camera.near);
    let a_port = camera.far / (camera.far - camera.near);
    let pull = mat.map_or(0.0, |m| depth_bias_offset(m.depth_bias.saturating_add(item.depth_bias)) * (a_port / a_game));
    // E9's receiver role, asked once for its two fields (`lit_d.w`, E15 / E23's `soft.z`)
    let role = std::cell::OnceCell::new();
    let receives = || *role.get_or_init(|| (rx.csm)(item));
    ObjectUniform {
        model: item.transform.to_cols_array_2d(),
        // .x is the environment-map mix, .zw a uv offset (the sky layers scroll with it)
        // .y flags an item the headlight projector lights
        env_mix: [env_mix, item.headlit as u32 as f32, item.uv_offset[0], item.uv_offset[1]],
        // ENHANCEMENT E9: the car's `SHAD` patch gives way to the sun's shadow of the car (cover gate; SHADOW-REAL: by the car's own cover)
        light: [item.light[0], item.light[1], item.light[2], match &csm_frame {
            Some(f) if item.csm & crate::CSM_CAR_BLOB != 0 => item.alpha * (1.0 - f.car_patch_gate * (1.0 - item.cover.clamp(0.0, 1.0))),
            _ => item.alpha,
        }],
        lit_a: item.layer_light.map_or([0.0; 4], |l| [l.ambient[0], l.ambient[1], l.ambient[2], 1.0]),
        // .w ENHANCEMENT E9: 1 a car (its direct term takes the sun shadow), 2 a track item (its baked colour does), else 0
        lit_d: item.layer_light.map_or([0.0; 4], |l| [l.diffuse[0], l.diffuse[1], l.diffuse[2], match &csm_frame {
            // ENHANCEMENT ENH-DBLSH: 3 = a track `sh80` decal under a caster (the shader fades it out with the shadow)
            Some(f) if item.csm & crate::CSM_DECAL != 0 && f.decal_fades => classes::RECV_DECAL,
            Some(_) => receives(),
            None => classes::RECV_NONE,
        }]),
        // .w the depth bias's pull, in view depth
        lit_dir: item.layer_light.map_or([0.0, 0.0, 0.0, pull], |l| [l.direction[0], l.direction[1], l.direction[2], pull]),
        shadow_u: item.shadow.map_or([0.0; 4], |s| s.0),
        shadow_v: item.shadow.map_or([0.0; 4], |s| s.1),
        // .y flags the item `Headlights::LayerUVLight` lays the `HEAD` decal over
        // .w the alpha test (a material with `device_states`, or the draw's own): its
        // reference + ½ in bytes, 0 for none
        shadow_on: [item.shadow.is_some() as u32 as f32, item.uv_lit as u32 as f32, lod_bias, alpha_ref],
        sky_u: item.sky.map_or([0.0; 4], |s| s.u),
        sky_v: item.sky.map_or([0.0; 4], |s| s.v),
        // .y the item is drawn in the sky pass (its fog is the sky's)
        sky_on: [sky_tex as u32 as f32, item.sky_pass as u32 as f32, item.fog_w, if contact_on && (rx.contact)(item) { 1.0 } else { 0.0 }],
        // ENHANCEMENT E8
        // .z ENHANCEMENT E16: 1 = the car ambient takes the probe (not the cabin's materials)
        // ENHANCEMENT E17: class 3 = water (a horizontal-water item while the switch and the sky are there)
        // ENHANCEMENT E26: class 4 = a wet road part (`.w` its wetness; a dry one keeps its own class), class 5 = a skid strip on one
        refl: if item.water && water_env.is_some() { [classes::REFL_WATER, 1.0, 0.0, 0.0] } else if rain_env.is_some() && item.csm & crate::CSM_ROAD != 0 { [classes::REFL_WET_ROAD, 0.0, 0.0, item.wet] } else if rain_env.is_some() && item.csm & crate::CSM_SKID != 0 { [classes::REFL_SKID, 0.0, 0.0, item.wet] } else { mat.map_or([0.0; 4], |m| [m.reflect.class as f32, m.reflect.strength, (!m.reflect.interior && item.layer_light.is_some_and(|l| l.direction != [0.0; 3])) as u32 as f32, (near_own && item.near_env) as u32 as f32]) },
        // ENHANCEMENT E12: the fade distance; the multiplying pair (3) fades toward white
        // ENHANCEMENT E15: .z the item's role for the headlight spots (1 car, 2 track, 0 none)
        // ENHANCEMENT ENH-TEMPORAL: .w the item's role for the masks (0 opaque-like, 1 a translucent effect, 2 a sky layer)
        soft: [if soft_split && item.soft > 0.0 { item.soft } else { 0.0 }, if soft_split && item.soft > 0.0 && mat.is_some_and(|m| m.blend_pair == 3) { classes::SOFT_FADE_WHITE } else { classes::SOFT_FADE_ALPHA }, if spots { receives() } else { classes::RECV_NONE }, roles.get(i).copied().unwrap_or(classes::ROLE_OPAQUE)],
        // ENHANCEMENT E11: only on the float scene with the switch on; 0 = none
        // ENHANCEMENT E18: .y a spark's gain, .z the lit particle kind
        // ENHANCEMENT ENH-TEMPORAL: .w the index of the item's reprojection matrix
        hdr: [if hdr_boost { item.hdr_boost } else { 0.0 }, if p_fx && item.particle == crate::PARTICLE_SPARK { SPARK_GAIN } else { 0.0 }, if p_fx && item.particle >= crate::PARTICLE_LIT { item.particle as f32 } else { 0.0 }, motion_index.get(i).map_or(0.0, |&k| k as f32)],
        // PORT-DEVIATION D37: master's clip transform (with the jitter in `vp_j` only while ENH-TEMPORAL is on)
        mvp: (vp_j * glam::DMat4::from_cols_array(&item.transform.to_cols_array().map(f64::from))).as_mat4().to_cols_array_2d(),
    }
}

/// PERF (PERF-B): which per-item parts of the mesh shader an item's uniform can enter, by the shader's own tests of those parts:
/// bit 0 the surface parts (`classes::feat::SURFACE`) — a reflection class (`refl.x` > 0.5: E8's paint / glass, E17's water,
/// E26's wet road / skid); bit 1 the effect parts (`classes::feat::EFFECT`) — a spark or a lit particle (`hdr.y` > 0, `hdr.z` >
/// PART_LIT - 0.5, E18), a soft fade (`soft.x` > 0, E12), an HDR boost (`hdr.x` > 1, E11). Without a bit the item draws the
/// same with a shader built without those parts. The value is the variant's bits (`as usize` indexes `Core::var_set`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum ItemVariant {
    /// neither: the shader without the per-item parts draws it the same
    Plain = 0,
    /// a reflection class (`classes::feat::SURFACE`)
    Surface = 1,
    /// a spark / lit particle, a soft fade or an HDR boost (`classes::feat::EFFECT`)
    Effect = 2,
    Both = 3,
}

impl ItemVariant {
    /// The parts an item's uniform can enter, by the shader's own tests of those parts.
    pub(super) fn of(o: &ObjectUniform) -> Self {
        let surface = o.refl[0] > classes::REFL_NONE + 0.5;
        let effect = o.hdr[1] > 0.0 || o.hdr[2] > classes::PART_LIT - 0.5 || o.soft[0] > 0.0 || o.hdr[0] > 1.0;
        match (surface, effect) {
            (false, false) => Self::Plain,
            (true, false) => Self::Surface,
            (false, true) => Self::Effect,
            (true, true) => Self::Both,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Fog, LayerLight, RainEdge, SkyMap, SunShadow};
    use glam::{Mat4, Vec3};

    /// A scene with every input an enhancement reads set (wet, water, sun, flash, fog, headlight at full gain).
    fn busy_scene() -> Scene {
        let mut lamps = [(Vec3::ZERO, [0.0; 3], 0.0); crate::RAIN_LAMPS];
        lamps[0] = (Vec3::new(5.0, 6.0, 7.0), [1.0, 0.9, 0.8], 0.0);
        let mut edges = [RainEdge::default(); crate::RAIN_EDGES];
        edges[0] = RainEdge { pos: Vec3::new(1.0, 0.0, 2.0), dir: Vec3::Z, before: 1.0, after: 0.0, half: 16.0 };
        Scene {
            ambient: 0.42,
            headlight: Some(Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0))),
            headlight_decal: true,
            beacon: Some(Mat4::from_rotation_y(0.3)),
            fog: Some(Fog { colour: [0.5, 0.6, 0.7], density: 0.01 }),
            sky_fog: Some(Fog { colour: [0.2, 0.3, 0.4], density: 0.02 }),
            refl: 0.8,
            shadows: crate::ShadowLights { sun: Some(SunShadow { direction: Vec3::new(0.3, -0.9, 0.2), lit: true, focus: None }), ..crate::ShadowLights::NONE },
            water: Some(WaterEnv { zenith: [0.1, 0.2, 0.3], horizon: [0.4, 0.5, 0.6], sun: Vec3::new(0.0, 0.7, 0.7), use_env_map: true }),
            rain: Some(RainEnv { zenith: [0.3; 3], horizon: [0.4; 3], camera_wet: 1.0, time: 12.5, lamps, n_lamps: 1, edges, n_edges: 1 }),
            ..Scene::default()
        }
    }

    fn camera() -> Camera {
        Camera { position: Vec3::new(10.0, 2.0, -4.0), target: Vec3::new(10.0, 1.5, 6.0), up: Vec3::Y, fov_y_radians: 0.9, near: 0.5, far: 700.0 }
    }

    /// `master`'s globals (its `GlobalsUniform`'s 11 fields are the first 11 of ours), verbatim from its `encode_frame`.
    fn master_globals(scene: &Scene, camera: &Camera, vp: Mat4, no_reflections: bool) -> Vec<u8> {
        let a: [[[f32; 4]; 4]; 2] = [vp.to_cols_array_2d(), camera.view().to_cols_array_2d()];
        let b: [[f32; 4]; 3] = [scene.sun_direction.extend(0.0).to_array(), [scene.ambient, camera.near, no_reflections as u32 as f32, 0.0], camera.position.extend(1.0).to_array()];
        let c: [[f32; 4]; 4] = scene.headlight.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d();
        let d: [f32; 4] = [scene.headlight.is_some() as u32 as f32, (scene.headlight.is_some() && scene.headlight_decal) as u32 as f32, 0.0, 0.0];
        let e: [[f32; 4]; 4] = scene.beacon.unwrap_or(glam::Mat4::IDENTITY).to_cols_array_2d();
        let f: [[f32; 4]; 3] = [[scene.beacon.is_some() as u32 as f32, 0.0, 0.0, 0.0], scene.fog.map_or([0.0; 4], |f| [f.colour[0], f.colour[1], f.colour[2], f.density]), scene.sky_fog.map_or([0.0; 4], |f| [f.colour[0], f.colour[1], f.colour[2], f.density])];
        [bytemuck::bytes_of(&a), bytemuck::bytes_of(&b), bytemuck::bytes_of(&c), bytemuck::bytes_of(&d), bytemuck::bytes_of(&e), bytemuck::bytes_of(&f)].concat()
    }

    /// `master`'s object uniform (its 12 fields are the first 12 of ours), verbatim from its `encode_frame`.
    fn master_object(item: &DrawItem, mat: Option<&MatFill>, sky_tex: bool, camera: &Camera, depth_far: f32) -> Vec<u8> {
        let env_mix = mat.map_or(0.0, |m| m.env_mix);
        let lod_bias = mat.map_or(0.0, |m| m.lod_bias);
        let alpha_test = mat.is_some_and(|m| m.device_states) || item.alpha_test;
        let alpha_ref = if alpha_test { item.alpha_ref as f32 + 0.5 } else { 0.0 };
        let a_game = depth_far / (depth_far - camera.near);
        let a_port = camera.far / (camera.far - camera.near);
        let pull = mat.map_or(0.0, |m| depth_bias_offset(m.depth_bias.saturating_add(item.depth_bias)) * (a_port / a_game));
        let model = item.transform.to_cols_array_2d();
        let rest: [[f32; 4]; 11] = [
            [env_mix, item.headlit as u32 as f32, item.uv_offset[0], item.uv_offset[1]],
            [item.light[0], item.light[1], item.light[2], item.alpha],
            item.layer_light.map_or([0.0; 4], |l| [l.ambient[0], l.ambient[1], l.ambient[2], 1.0]),
            item.layer_light.map_or([0.0; 4], |l| [l.diffuse[0], l.diffuse[1], l.diffuse[2], 0.0]),
            item.layer_light.map_or([0.0, 0.0, 0.0, pull], |l| [l.direction[0], l.direction[1], l.direction[2], pull]),
            item.shadow.map_or([0.0; 4], |s| s.0),
            item.shadow.map_or([0.0; 4], |s| s.1),
            [item.shadow.is_some() as u32 as f32, item.uv_lit as u32 as f32, lod_bias, alpha_ref],
            item.sky.map_or([0.0; 4], |s| s.u),
            item.sky.map_or([0.0; 4], |s| s.v),
            [sky_tex as u32 as f32, item.sky_pass as u32 as f32, item.fog_w, 0.0],
        ];
        [bytemuck::bytes_of(&model), bytemuck::bytes_of(&rest)].concat()
    }

    /// with [`Enhancements::ORIGINAL`] (`NFS_ENH=off`, master's frame: no float scene, no temporal view) the
    /// globals master has are master's bytes, and every enhancement-owned field is 0 — or, for the two E12 planes, a value
    /// only the E12 branch reads (`soft.z` 0 = no split). The scene carries every enhancement input (wet, water, sun, flash):
    /// none of them may leak through. `headlight_on.z` is E24's `1 - gain`: master's 0 at the original's gain 1.0.
    #[test]
    fn original_globals_are_masters() {
        let scene = busy_scene();
        let cam = camera();
        let enh = Enhancements::ORIGINAL;
        let vp = cam.view_projection_mirrored(16.0 / 10.0, false);
        for no_reflections in [false, true] {
            let env = FrameEnv::new(&scene, &enh, None);
            assert!(env.water.is_none() && env.rain.is_none() && !env.p_fx && !env.hdr_boost);
            // the inputs `encode_frame` passes with the original set: cube faces drawn or not, a probe strength (E16's gate is
            // inside `probe_strength`, but the E8 gate here must hold it off too), no split (E12 off), no temporal view
            for refl_faces_done in [false, true] {
                let g = globals_uniform(&GlobalsFrame { scene: &scene, camera: &cam, vp_j: vp, enh: &enh, hdr: None, no_reflections, refl_faces_done, probe_strength: 0.6, soft_split: false, env, temporal: None, near: None });
                let bytes = bytemuck::bytes_of(&g);
                let master = master_globals(&scene, &cam, vp, no_reflections);
                assert_eq!(&bytes[..master.len()], &master[..], "the fields master has");
                let zero = [0.0f32; 4];
                assert_eq!(g.soft, [cam.near, cam.far, 0.0, 0.0]);
                for (name, v) in [("refl", g.refl), ("water", g.water), ("water_zenith", g.water_zenith), ("water_horizon", g.water_horizon), ("water_sun", g.water_sun), ("fx_sun", g.fx_sun), ("fogsun", g.fogsun), ("rain", g.rain), ("rain_zenith", g.rain_zenith), ("rain_horizon", g.rain_horizon), ("temporal", g.temporal), ("near", g.near)] {
                    assert_eq!(v, zero, "{name}");
                }
                for (name, a) in [("rain_lamp_pos", &g.rain_lamp_pos[..]), ("rain_lamp_col", &g.rain_lamp_col[..]), ("rain_edge_pos", &g.rain_edge_pos[..]), ("rain_edge_dir", &g.rain_edge_dir[..]), ("rain_edge_val", &g.rain_edge_val[..]), ("near_box", &g.near_box[..]), ("near_eye", &g.near_eye[..])] {
                    assert!(a.iter().all(|v| *v == zero), "{name}");
                }
            }
        }
        // and the same inputs with every switch on do reach the enhancement fields (the test sees them)
        let all = Enhancements::all();
        let env = FrameEnv::new(&scene, &all, Some(2));
        let g = globals_uniform(&GlobalsFrame { scene: &scene, camera: &cam, vp_j: vp, enh: &all, hdr: Some(2), no_reflections: false, refl_faces_done: true, probe_strength: 0.6, soft_split: true, env, temporal: Some(([0.1, -0.1], (640, 400))), near: None });
        assert!(g.refl[0] > 0.0 && g.water[0] == 1.0 && g.rain[0] > 0.0 && g.fogsun[3] > 0.0 && g.temporal[2] == 640.0 && g.ambient[3] == 1.0);
    }

    /// with [`Enhancements::ORIGINAL`] every item's first 12 fields are master's bytes and `soft` / `hdr` are 0;
    /// `refl` is the material's own class 0..2 and strength, read by the shader only under the globals' `refl.x` (0 above),
    /// never the water / wet road / skid classes 3..5. The receiver would put 7 into the fields if it were asked.
    #[test]
    fn original_objects_are_masters() {
        let scene = busy_scene();
        let cam = camera();
        let enh = Enhancements::ORIGINAL;
        let env = FrameEnv::new(&scene, &enh, None);
        let csm = |_: &DrawItem| 7.0;
        let contact = |_: &DrawItem| true;
        let rx = Receivers { csm: &csm, contact: &contact };
        let mats = [
            None,
            Some(MatFill { env_mix: 0.3, lod_bias: -0.5, device_states: true, depth_bias: -4, blend_pair: 3, reflect: crate::Reflect { class: 1, strength: 0.6, interior: false } }),
            Some(MatFill { env_mix: 0.0, lod_bias: 0.0, device_states: false, depth_bias: 2, blend_pair: 0, reflect: crate::Reflect { class: 2, strength: 1.0, interior: true } }),
        ];
        let mut items = Vec::new();
        for flags in [0u8, crate::CSM_CAR_BLOB | crate::CSM_RECEIVE, crate::CSM_DECAL | crate::CSM_ROAD, crate::CSM_SKID | crate::CSM_ROOM] {
            let mut it = DrawItem::new(crate::MeshId(1), crate::MaterialId(0), Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)));
            it.csm = flags;
            it.alpha = 0.75;
            it.light = [0.9, 0.8, 0.7];
            it.water = flags & crate::CSM_ROAD != 0;
            it.wet = 0.9;
            it.soft = 2.5;
            it.particle = flags % 4;
            it.hdr_boost = 3.0;
            it.headlit = true;
            it.uv_lit = flags != 0;
            it.depth_bias = 3;
            it.uv_offset = [0.25, 0.5];
            it.fog_w = 0.4;
            it.sky_pass = flags == 0;
            it.layer_light = (flags != 0).then_some(LayerLight { ambient: [0.2, 0.3, 0.4], diffuse: [0.5, 0.6, 0.7], direction: [0.0, -1.0, 0.0] });
            it.shadow = (flags & crate::CSM_RECEIVE != 0).then_some(([1.0, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]));
            it.sky = (flags == 0).then_some(SkyMap { textures: [None; 3], u: [0.1, 0.2, 0.3, 0.4], v: [0.5, 0.6, 0.7, 0.8] });
            items.push(it);
        }
        for depth_far in [700.0, 65000.0] {
            let of = ObjectFrame { camera: &cam, depth_far, csm: None, soft_split: false, spots: false, contact_on: false, env, roles: &[], motion_index: &[], vp_j: glam::DMat4::from_cols_array(&cam.view_projection_mirrored(16.0 / 10.0, false).to_cols_array().map(f64::from)), lod_shift: 0.0, near_own: false };
            for (i, item) in items.iter().enumerate() {
                for mat in mats {
                    for sky_tex in [false, true] {
                        let o = object_uniform(&of, i, item, mat, sky_tex, &rx);
                        let bytes = bytemuck::bytes_of(&o);
                        let master = master_object(item, mat.as_ref(), sky_tex, &cam, depth_far);
                        assert_eq!(&bytes[..master.len()], &master[..], "item {i}");
                        assert_eq!((o.soft, o.hdr), ([0.0; 4], [0.0; 4]), "item {i}");
                        let own = mat.map_or([0.0; 4], |m| [m.reflect.class as f32, m.reflect.strength, (!m.reflect.interior && item.layer_light.is_some_and(|l| l.direction != [0.0; 3])) as u32 as f32, 0.0]);
                        assert_eq!(o.refl, own, "item {i}");
                        assert!(o.refl[0] < classes::REFL_WATER);
                    }
                }
            }
        }
    }
}
