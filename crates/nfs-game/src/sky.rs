//! Sky: the layers `Track/Sky/<Track>.txt` asks for, on a dome (`SkyLevel_*`, RE/RENDERING.md).
//!
//! The layer list, the textures, their UV repeats, the wind and the warp all come from the track's
//! own sky file now (`nfs_formats::sky`): the block whose capability matches is taken, weather
//! region A is used, and each layer that names a sky map is drawn as one dome, the first opaque and
//! the rest alpha-blended (the shipped tracks are all `MULTINORMAL` or `basic`). The `horz` horizon
//! strips keep their ring.
//!
//! **APPROXIMATION** in what is left: the dome itself (the original's ring geometry lives in
//! `Sky/<Track>.bin`, unread), the mapping from "wind speed 0..10" and "move ratio 50 fast, 400
//! slow" to a scroll rate, and the warp, which is ignored. Weather region B is parsed but never
//! selected, since what picks between A and B has not been traced.

use glam::Vec3;
use nfs_assets::track::TrackAssets;
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};

pub struct Sky {
    /// mesh, material and the layer's own scroll velocity in UV units per second
    parts: Vec<(MeshId, MaterialId, [f32; 2])>,
}

const RADIUS: f32 = 3000.0;

fn dome_uv(radius: f32, rings: u32, segs: u32, uv_scale: [f32; 2], y_min: f32) -> MeshData {
    let mut m = MeshData::default();
    for r in 0..=rings {
        let t = r as f32 / rings as f32; // 0 = horizon, 1 = zenith
        let el = y_min + (std::f32::consts::FRAC_PI_2 - y_min) * t;
        for s in 0..=segs {
            let az = s as f32 / segs as f32 * std::f32::consts::TAU;
            let p = Vec3::new(az.cos() * el.cos(), el.sin(), az.sin() * el.cos()) * radius;
            m.vertices.push(Vertex { position: p.to_array(), normal: [0.0, 1.0, 0.0], uv: [s as f32 / segs as f32 * uv_scale[0], (1.0 - t) * uv_scale[1]], color: [1.0; 4] });
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

        let file = nfs_formats::sky::SkyFile::parse(&track.sky_params.join("\n")).ok();
        let block = file.as_ref().and_then(|f| f.for_capability("HIGH"));
        if let Some(b) = block {
            let region = &b.regions[0];
            for (i, layer) in region.layers.iter().enumerate() {
                let Some(map) = layer.map.as_deref().and_then(find) else { continue };
                let id = tex(r, map);
                let m = sky_mat(r, id, i > 0);
                let scale = [layer.scale[0].max(0.01), layer.scale[1].max(0.01)];
                // the higher layers sit slightly closer so they sort in front of the base
                let radius = RADIUS * (1.0 - 0.02 * i as f32);
                parts.push((r.upload_mesh(&dome_uv(radius, 12, 48, scale, if i == 0 { -0.15 } else { 0.0 })), m, scroll(layer, region.move_ratio)));
            }
        }
        if let Some(hz) = find("horz") {
            let id = tex(r, hz);
            let m = sky_mat(r, id, true);
            // two 256×128 strips: top half = far layer, bottom half = near layer (HYPOTHESIS)
            parts.push((r.upload_mesh(&ring(RADIUS * 0.96, -RADIUS * 0.02, RADIUS * 0.12, 64, 4.0, 0.0, 0.5)), m, [0.0, 0.0]));
            parts.push((r.upload_mesh(&ring(RADIUS * 0.94, -RADIUS * 0.02, RADIUS * 0.10, 64, 5.0, 0.5, 1.0)), m, [0.0, 0.0]));
        }
        if parts.is_empty() { None } else { Some(Self { parts }) }
    }

    /// Draw items centred on the camera (sky follows the viewer). `far` is the camera's far plane —
    /// the dome is scaled to sit just inside it, since the draw distance the track's `.key` file
    /// asks for is far shorter than the sky's nominal radius. `seconds` scrolls the layers that the
    /// file gives a wind (APPROXIMATION of the original's rate, see the module note).
    pub fn draw_items(&self, camera_pos: Vec3, far: f32, seconds: f32) -> impl Iterator<Item = DrawItem> + '_ {
        let k = (far * 0.9 / RADIUS).clamp(0.02, 4.0);
        let t = glam::Mat4::from_translation(camera_pos) * glam::Mat4::from_scale(Vec3::splat(k));
        self.parts.iter().map(move |(mesh, material, v)| {
            let mut item = DrawItem::new(*mesh, *material, t);
            item.uv_offset = [v[0] * seconds, v[1] * seconds];
            item
        })
    }
}

/// Wind speed ("0 for none, 10 for lots") and direction (degrees, north 0, east 90) into a UV
/// velocity, damped by the region's move ratio ("50 is fast and 400 is slow", 0 = still).
/// APPROXIMATION: the original's units were not traced, only the sense of each field.
fn scroll(layer: &nfs_formats::sky::SkyLayer, move_ratio: f32) -> [f32; 2] {
    if layer.wind_speed == 0.0 || move_ratio == 0.0 {
        return [0.0, 0.0];
    }
    let rate = layer.wind_speed / 400.0 * (400.0 / move_ratio.max(1.0));
    let a = layer.wind_direction.to_radians();
    [rate * a.sin() * 0.01, rate * a.cos() * 0.01]
}
