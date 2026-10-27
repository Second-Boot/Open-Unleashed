//! ENHANCEMENT ENH-FE3D: the enhanced game's switches in the front end's 3-D views (the garage, the dealership, the
//! victory room and every screen that shows a car through the room object or an offscreen car view).
//!
//! Which switch does what here (each one follows its own switch; all off = master's front end):
//! * E1 (MSAA), E2 (anisotropy, the cars' generated mips), E7 (float image + tone map) and E10 (bloom) already take the
//!   room's frame (the renderer's `encode_enhanced`); this change gives the offscreen car views (carcompare, the
//!   `positiontype` cars) the contrasty tone map as well so a car and the room behind it share one curve.
//! * E8 body reflections: the car's paint and glass reflect the room — the room's own pieces are drawn from the car's
//!   position into the six faces of the environment map (the same machinery as the race's, a few faces a frame) — or,
//!   where the screen has no room round the car (`ID_NONE`, the offscreen views), a soft studio the app paints once.
//!   E16 (the car's ambient from the surroundings) follows the map.
//! * E9 shadow of the car on the floor: one cascade fitted to a sphere round the car, a soft jittered edge; the room's
//!   pieces receive it (`CSM_ROOM`) and never cast it. The light is the port's own key light from above (the original's
//!   rooms have no shadow-casting light: the car's block is a fixed grey), the original's blob under the garage car stays.
//! * E14 corner shading: the screen-space occlusion pass on the room's frame and the offscreen views.
//!
//! Nothing here reaches the simulation, the network or a replay: it reads the front end's scene and draws.

use crate::*;
use nfs_renderer::{DrawItem, ShadowFocus, SunShadow};

/// What the front end's 3-D scene takes from the enhanced game this frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FeEnh {
    /// `Scene::refl`
    pub refl: f32,
    /// `Scene::ao`
    pub ao: bool,
    /// `Scene::sun_shadow`, for the room's own car only
    pub shadow: Option<SunShadow>,
}

/// The key light's travel direction in the front end's frame (x right, y up, z away from the camera): from above and a
/// little behind the car's far side, so its soft shadow falls toward the viewer and shows under and in front of the car.
const KEY_LIGHT: Vec3 = Vec3::new(0.2, -1.0, -0.5);

/// Sphere round the room's car the shadow map covers (m): a car is ~4.5 m long.
/// How strongly the front end's paint takes its environment. ENHANCEMENT ENH-COAT: 1.0 like the race (it was 0.7 for the old
/// additive lacquer, which washed a red car to pink; the physical coat keeps the colour and needs no weight).
const REFL_WEIGHT: f32 = 1.0;

const SHADOW_RADIUS: f32 = 3.4;

/// How dark the shadow gets on a receiver at full shadow, and its edge (metres).
const SHADOW_STRENGTH: f32 = 0.78;
/// ENHANCEMENT ENH-GARAGE: the contact core's radius (m), the penumbra's share of the strength, how much of the original's
/// garage blob gives way (2026-10-04: one shadow, the car must not float).
const SHADOW_CORE: f32 = 0.04;
const SHADOW_SOFT: f32 = 0.55;
const BLOB_FADE: f32 = 1.0;
const SHADOW_BLUR: f32 = 0.14;

/// Size of the studio faces.
const STUDIO_SIZE: u32 = 64;

/// A room model's items of this frame: marked as the room's own (they receive the car's shadow, never cast it) and,
/// for the environment faces, copied into `faces` (`shadow` / `env` = E9 / E8 on).
pub(crate) fn tag_room_piece(items: &mut [DrawItem], faces: &mut Vec<DrawItem>, shadow: bool, env: bool) {
    if shadow {
        for it in items.iter_mut() {
            it.csm |= nfs_renderer::CSM_ROOM;
        }
    }
    if env {
        faces.extend_from_slice(items);
    }
}

impl App {
    /// Whether the front end's 3-D views take this switch now: the enhanced game is on (not `NFS_ENH=off`, whose
    /// switches are all off anyway) and the front end is what is showing.
    pub(crate) fn fe_enh_on(&self, sw: nfs_renderer::EnhancementSwitch) -> bool {
        self.vehicle.is_none() && self.enh.is_on(sw)
    }

    /// The frame's enhancements for the room's scene (and the offscreen views): the environment map made / refreshed,
    /// the weights returned. `car` is where the room's own car stands, `None` with no room car.
    pub(crate) fn fe_enh_prepare(&mut self, car: Option<Vec3>, ambient: f32, sun: Vec3) -> FeEnh {
        let mut out = FeEnh::default();
        if self.vehicle.is_some() || self.renderer.is_none() {
            self.fe.enh_refl = 0.0;
            return out;
        }
        out.ao = self.fe_enh_on(nfs_renderer::E14);
        if self.fe_enh_on(nfs_renderer::E9)
            && let Some(p) = car
        {
            out.shadow = Some(SunShadow { direction: KEY_LIGHT.normalize(), lit: true, focus: Some(ShadowFocus { centre: p + Vec3::Y * 0.5, radius: SHADOW_RADIUS, blur: SHADOW_BLUR, strength: SHADOW_STRENGTH, core: SHADOW_CORE, soft: SHADOW_SOFT, blob_fade: BLOB_FADE }) });
        }
        if self.fe_enh_on(nfs_renderer::E8) {
            let rooms = !self.fe.enh_room.is_empty() && car.is_some();
            if rooms {
                self.fe_env_room_faces(car.unwrap_or_default(), ambient, sun);
            } else {
                self.fe_env_studio();
            }
            out.refl = REFL_WEIGHT;
        }
        self.fe.enh_refl = out.refl;
        tracing::debug!(refl = out.refl, ao = out.ao, shadow = out.shadow.is_some(), car = ?car, pieces = self.fe.enh_room.len(), kind = self.fe.enh_env_kind, "FE 3D enhancements");
        out
    }

    /// The room's pieces seen from the car into the environment faces, a few a frame like the race (all six at the
    /// first), the whole map again when the car moved far (a dealership slide) or the map was the studio's.
    fn fe_env_room_faces(&mut self, car: Vec3, ambient: f32, sun: Vec3) {
        let (size, per_frame) = crate::app::reflections::level_params(self.enh.level(nfs_renderer::E8)).unwrap_or((256, 2));
        let eye = car + Vec3::Y * 0.7;
        if self.fe.enh_env_kind != 1 || size != self.fe.enh_env_size || eye.distance(self.fe.enh_env_eye) > 1.5 {
            self.fe.enh_env_done = 0;
            self.fe.enh_env_kind = 1;
            self.fe.enh_env_size = size;
        }
        self.fe.enh_env_eye = eye;
        let n = if self.fe.enh_env_done == 0x3F { per_frame } else { 6 };
        let items = std::mem::take(&mut self.fe.enh_room);
        let scene = Scene { items, ambient, sun_direction: sun, clear_color: [0.02, 0.025, 0.03, 1.0], ..Scene::default() };
        let tan = nfs_renderer::REFL_TAN_HALF;
        for _ in 0..n {
            let face = self.fe.enh_env_next % 6;
            self.fe.enh_env_next = (face + 1) % 6;
            let (dir, up) = nfs_renderer::REFL_FACES[face as usize];
            let (forward, up) = (Vec3::from(dir), Vec3::from(up));
            let cam = Camera { position: eye, target: eye + forward, up, fov_y_radians: 2.0 * tan.atan(), near: 0.1, far: 200.0 };
            if let Some(r) = self.renderer.as_mut() {
                r.render_refl_face(&scene, &cam, face, size, scene.clear_color, nfs_renderer::VIEW_FAR);
            }
            self.fe.enh_env_done |= 1 << face;
        }
        // handed back for the next frame's pieces to be pushed again
        self.fe.enh_room = scene.items;
        self.fe.enh_room.clear();
    }

    /// A studio for a car with no room round it: a dark neutral room, a large soft box above and in front, two long strip
    /// lights at the sides. Painted once (the map is the same every frame).
    fn fe_env_studio(&mut self) {
        if self.fe.enh_env_kind == 2 {
            return;
        }
        self.fe.enh_env_kind = 2;
        self.fe.enh_env_done = 0;
        let faces = studio_faces(STUDIO_SIZE);
        if let Some(r) = self.renderer.as_mut() {
            r.upload_refl_faces(STUDIO_SIZE, faces);
        }
    }
}

/// The studio's radiance toward direction `d` (display values 0..1).
fn studio(d: Vec3) -> [f32; 3] {
    let smooth = |a: f32, b: f32, x: f32| {
        let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    // walls: a dark cool grey, lighter toward the ceiling; the floor darker
    let mut c = Vec3::new(0.035, 0.045, 0.05) * (0.7 + 0.6 * (d.y * 0.5 + 0.5));
    if d.y < -0.1 {
        c *= 0.55;
    }
    // the big soft box above and toward the viewer (-z): a wide, soft-edged rectangle
    let key = Vec3::new(0.0, 0.85, -0.5).normalize();
    let kd = d.dot(key);
    c += Vec3::new(1.0, 0.97, 0.92) * (0.55 * smooth(0.90, 0.975, kd));
    // two strip lights at the sides, tall and narrow
    for sx in [-1.0f32, 1.0] {
        let s = Vec3::new(0.95 * sx, 0.1, -0.2).normalize();
        let dh = (d.x * s.z - d.z * s.x).abs(); // across the strip (horizontal angle)
        let along = d.y;
        let w = smooth(0.20, 0.08, dh) * smooth(-0.35, -0.05, along) * smooth(0.85, 0.55, along) * (d.dot(s) > 0.0) as u32 as f32;
        c += Vec3::new(0.85, 0.92, 1.0) * (0.4 * w);
    }
    [c.x, c.y, c.z]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The studio's faces are the size asked, opaque, and the soft box above the car is the brightest part of the map.
    #[test]
    fn studio_faces_shape() {
        let faces = studio_faces(16);
        assert_eq!(faces.len(), 6);
        assert!(faces.iter().all(|f| f.len() == 16 * 16 * 4 && f.chunks_exact(4).all(|p| p[3] == 1.0)));
        let lum = |f: &Vec<f32>| f.chunks_exact(4).map(|p| p[0] + p[1] + p[2]).fold(0.0f32, f32::max);
        assert!(lum(&faces[4]) > lum(&faces[5]), "the ceiling face holds the light, the floor face does not");
    }
}

/// Six `size x size` RGBA f32 faces of [`studio`], the layout the shader reads (`refl_sample`, `REFL_FACES`).
fn studio_faces(size: u32) -> Vec<Vec<f32>> {
    let tan = nfs_renderer::REFL_TAN_HALF;
    nfs_renderer::REFL_FACES
        .iter()
        .map(|&(f, u)| {
            let (fwd, up) = (Vec3::from(f), Vec3::from(u));
            let right = up.cross(fwd);
            let mut px: Vec<f32> = Vec::with_capacity((size * size * 4) as usize);
            for j in 0..size {
                for i in 0..size {
                    let (su, sv) = ((i as f32 + 0.5) / size as f32, (j as f32 + 0.5) / size as f32);
                    // u = 0.5 + 0.5 (d.right / z) / tan  ->  d.right / z = (2u - 1) tan; v the other way up
                    let d = (fwd + right * ((2.0 * su - 1.0) * tan) + up * ((1.0 - 2.0 * sv) * tan)).normalize();
                    let c = studio(d);
                    px.extend_from_slice(&[c[0], c[1], c[2], 1.0]);
                }
            }
            px
        })
        .collect()
}
