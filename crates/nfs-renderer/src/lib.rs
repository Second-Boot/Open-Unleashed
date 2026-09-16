//! wgpu (Metal on macOS) renderer with an engine-level API.
//!
//! Nothing here knows about CRP/SHPI files; `nfs-assets` converts original data into
//! [`MeshData`] / [`TextureData`] and this crate uploads and draws them.

mod camera;
mod gpu;

pub use camera::Camera;
pub use gpu::{MaterialId, MeshId, Renderer, TextureId};

use glam::{Mat4, Vec3};

/// CPU-side vertex layout used for uploads.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    /// RGBA8, row-major, top-left origin; level 0 first, then optional pre-built mips.
    pub mips: Vec<Vec<u8>>,
}

impl TextureData {
    pub fn single(width: u32, height: u32, rgba: Vec<u8>) -> Self {
        Self { width, height, mips: vec![rgba] }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MaterialDesc {
    pub texture: Option<TextureId>,
    pub alpha_blend: bool,
    pub cull_back: bool,
    /// When false the item is drawn first without depth test/write (sky, backgrounds).
    pub depth_test: bool,
}

impl MaterialDesc {
    pub fn textured(texture: TextureId) -> Self {
        Self { texture: Some(texture), alpha_blend: false, cull_back: true, depth_test: true }
    }
    pub fn flat() -> Self {
        Self { texture: None, alpha_blend: false, cull_back: true, depth_test: true }
    }
}

/// One draw: a mesh with a material and a world transform.
#[derive(Clone, Copy, Debug)]
pub struct DrawItem {
    pub mesh: MeshId,
    pub material: MaterialId,
    pub transform: Mat4,
}

/// 2D overlay quad in normalised screen units (0..1, origin top-left), optionally rotated about `pivot`.
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub texture: TextureId,
    /// top-left corner in screen units (0..1)
    pub pos: [f32; 2],
    /// size in screen units (relative to the height for square pixels: x is multiplied by 1/aspect)
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    pub color: [f32; 4],
    /// rotation in turns about `pivot` (in sprite-local 0..1 coordinates)
    pub rotation: f32,
    pub pivot: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub items: Vec<DrawItem>,
    pub sprites: Vec<Sprite>,
    pub clear_color: [f64; 4],
    pub sun_direction: Vec3,
    pub ambient: f32,
}

impl Default for Scene {
    fn default() -> Self {
        Self { items: Vec::new(), sprites: Vec::new(), clear_color: [0.35, 0.55, 0.85, 1.0], sun_direction: Vec3::new(0.4, 1.0, 0.3).normalize(), ambient: 0.35 }
    }
}

impl MeshData {
    /// Simple checker-grid ground plane used by the M1 smoke test.
    pub fn grid(half: f32, step: f32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let n = ((2.0 * half) / step) as i32;
        for iz in 0..n {
            for ix in 0..n {
                let x0 = -half + ix as f32 * step;
                let z0 = -half + iz as f32 * step;
                let shade = if (ix + iz) % 2 == 0 { 0.55 } else { 0.35 };
                let base = vertices.len() as u32;
                for (dx, dz) in [(0.0, 0.0), (step, 0.0), (step, step), (0.0, step)] {
                    vertices.push(Vertex {
                        position: [x0 + dx, 0.0, z0 + dz],
                        normal: [0.0, 1.0, 0.0],
                        uv: [dx / step, dz / step],
                        color: [shade, shade, shade, 1.0],
                    });
                }
                indices.extend_from_slice(&[base, base + 2, base + 1, base, base + 3, base + 2]);
            }
        }
        Self { vertices, indices }
    }
}
