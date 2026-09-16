//! Sky: APPROXIMATION of the original layered sky (`SkyLevel_MultiSky`, `Track/Sky/*.txt|.bin|.fsh`).
//!
//! What is used from the original data: the `st1a` sky colour tile stretched over a dome, the
//! `sc1a` cloud tile blended on top, and the `horz` horizon strips (mountains with alpha) on a
//! ring around the camera. The `.bin` ring geometry, scroll/wind parameters and weather regions
//! from the `.txt` are not reproduced yet (see RE/RENDERING.md).

use glam::Vec3;
use nfs_assets::track::TrackAssets;
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};

pub struct Sky {
    parts: Vec<(MeshId, MaterialId)>,
}

const RADIUS: f32 = 3000.0;

fn dome(radius: f32, rings: u32, segs: u32, uv_scale: f32, y_min: f32) -> MeshData {
    let mut m = MeshData::default();
    for r in 0..=rings {
        let t = r as f32 / rings as f32; // 0 = horizon, 1 = zenith
        let el = y_min + (std::f32::consts::FRAC_PI_2 - y_min) * t;
        for s in 0..=segs {
            let az = s as f32 / segs as f32 * std::f32::consts::TAU;
            let p = Vec3::new(az.cos() * el.cos(), el.sin(), az.sin() * el.cos()) * radius;
            m.vertices.push(Vertex { position: p.to_array(), normal: [0.0, 1.0, 0.0], uv: [s as f32 / segs as f32 * uv_scale, (1.0 - t) * uv_scale], color: [1.0; 4] });
        }
    }
    for r in 0..rings {
        for s in 0..segs {
            let a = r * (segs + 1) + s;
            let b = a + segs + 1;
            // inward-facing (viewed from inside): reverse winding
            m.indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    m
}

fn ring(radius: f32, y0: f32, y1: f32, segs: u32, repeats: f32, v0: f32, v1: f32) -> MeshData {
    let mut m = MeshData::default();
    for s in 0..=segs {
        let az = s as f32 / segs as f32 * std::f32::consts::TAU;
        let (x, z) = (az.cos() * radius, az.sin() * radius);
        let u = s as f32 / segs as f32 * repeats;
        m.vertices.push(Vertex { position: [x, y0, z], normal: [0.0, 1.0, 0.0], uv: [u, v1], color: [1.0; 4] });
        m.vertices.push(Vertex { position: [x, y1, z], normal: [0.0, 1.0, 0.0], uv: [u, v0], color: [1.0; 4] });
    }
    for s in 0..segs {
        let a = s * 2;
        m.indices.extend_from_slice(&[a, a + 1, a + 2, a + 1, a + 3, a + 2]);
    }
    m
}

impl Sky {
    pub fn upload(r: &mut Renderer, track: &TrackAssets) -> Option<Self> {
        let find = |n: &str| track.sky_textures.iter().find(|t| t.name.trim() == n);
        fn tex(r: &mut Renderer, t: &nfs_assets::texture::TextureImage) -> nfs_renderer::TextureId {
            r.upload_texture(&TextureData { width: t.width, height: t.height, mips: t.mips.clone() })
        }
        let mut parts = Vec::new();
        let sky_mat = |r: &mut Renderer, id, blend| r.create_material(MaterialDesc { texture: Some(id), alpha_blend: blend, cull_back: false, depth_test: false, env_texture: None, env_mix: 0.0 });
        if let Some(st) = find("st1a") {
            let id = tex(r, st);
            let m = sky_mat(r, id, false);
            parts.push((r.upload_mesh(&dome(RADIUS, 12, 48, 1.0, -0.15)), m));
        }
        if let Some(sc) = find("sc1a") {
            let id = tex(r, sc);
            let m = sky_mat(r, id, true);
            parts.push((r.upload_mesh(&dome(RADIUS * 0.98, 12, 48, 2.0, 0.0)), m));
        }
        if let Some(hz) = find("horz") {
            let id = tex(r, hz);
            let m = sky_mat(r, id, true);
            // two 256×128 strips: top half = far layer, bottom half = near layer (HYPOTHESIS)
            parts.push((r.upload_mesh(&ring(RADIUS * 0.96, -RADIUS * 0.02, RADIUS * 0.12, 64, 4.0, 0.0, 0.5)), m));
            parts.push((r.upload_mesh(&ring(RADIUS * 0.94, -RADIUS * 0.02, RADIUS * 0.10, 64, 5.0, 0.5, 1.0)), m));
        }
        if parts.is_empty() { None } else { Some(Self { parts }) }
    }

    /// Draw items centred on the camera (sky follows the viewer).
    pub fn draw_items(&self, camera_pos: Vec3) -> impl Iterator<Item = DrawItem> + '_ {
        let t = glam::Mat4::from_translation(camera_pos);
        self.parts.iter().map(move |(mesh, material)| DrawItem { mesh: *mesh, material: *material, transform: t })
    }
}
