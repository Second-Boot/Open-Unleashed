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
    /// Environment (reflection) map blended by `env_mix` using a view-space sphere mapping of the
    /// normal — the original's `CarExtEnv`/`R3DSpecularTextureLayer` uses the track `.env` maps.
    pub env_texture: Option<TextureId>,
    pub env_mix: f32,
    /// Which of the three environment textures this material reflects — `R3DSpecularTextureLayer`
    /// buckets its own reflection strength at 0.05 and 0.95 (`RE/notes/lighting.md` §3.1).
    pub env_variant: usize,
}

impl MaterialDesc {
    pub fn textured(texture: TextureId) -> Self {
        Self { texture: Some(texture), alpha_blend: false, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0 }
    }
    pub fn flat() -> Self {
        Self { texture: None, alpha_blend: false, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0 }
    }
}

/// One draw: a mesh with a material and a world transform.
#[derive(Clone, Copy, Debug)]
pub struct DrawItem {
    pub mesh: MeshId,
    pub material: MaterialId,
    pub transform: Mat4,
    /// Per-object light multiplier, white for anything that takes the scene light as it is. Cars
    /// use it to carry the colour of the ground they stand on (`RE/notes/lighting.md`).
    pub light: [f32; 3],
    /// Per-object opacity, multiplying the material's own alpha — 1 for everything but the
    /// particles, which fade out with it.
    pub alpha: f32,
    /// Added to every vertex's uv before sampling; the sky layers drift with it.
    pub uv_offset: [f32; 2],
    /// The car materials' `R3DLightLayer` (0x8104, `RE/notes/lighting.md` §6), when the item is
    /// lit by one; it then takes the place of the scene's ambient and sun.
    pub layer_light: Option<LayerLight>,
    /// The item's render method builds `Headlights::LayerVertexLight` (0x8002 / 0x8003), so the
    /// scene's headlight projector adds to its vertex colours (`RE/notes/lighting.md` §5).
    pub headlit: bool,
    /// `Headlights::LayerUVLight` (0x453CA0, class code 0x8004): the `HEAD` decal of
    /// `Render/common.fsh` projected from the same headlight frame (`RE/notes/lighting.md` §5.2).
    pub uv_lit: bool,
    /// The car shadow projector's `(u, v)` planes over world positions — `R3DShadowMapTextureLayer`
    /// (0x446130), for an item whose material carries layer 0x8101 while its car stands near a
    /// kind-4 `ef` part (`RE/notes/crp-geometry.md` §8).
    pub shadow: Option<([f32; 4], [f32; 4])>,
}

impl DrawItem {
    pub fn new(mesh: MeshId, material: MaterialId, transform: Mat4) -> Self {
        Self { mesh, material, transform, light: [1.0; 3], alpha: 1.0, uv_offset: [0.0; 2], layer_light: None, headlit: false, uv_lit: false, shadow: None }
    }
}

/// What `R3DLightLayer` 0x446530 computes a vertex's colour from: `ambient + diffuse · d`, with `d`
/// the normal against the reversed light direction and the ambient alone where that is negative,
/// each channel clamped to 1 (0x446944…0x446A17).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerLight {
    /// the light block's `slot` × `p0`
    pub ambient: [f32; 3],
    /// its `slot + 4` × the block's `+0x74` × `p1`
    pub diffuse: [f32; 3],
    /// the block's `+0x30`, the way the light travels, in world space
    pub direction: [f32; 3],
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
    /// `MIRROR` race option: the world is drawn x-flipped (sprites unaffected).
    pub mirror: bool,
    /// The headlight projector, world → the player car's light frame (0x4526C0), when the car's
    /// lamps are lit; `headlit` items then take 0x452910's weight from `Renderer::set_headlight_map`.
    pub headlight: Option<Mat4>,
    /// World into the nearest lit police car's spinning beacon frame — the second projector on the
    /// same path (0x452A50, `RE/notes/lighting.md` §5).
    pub beacon: Option<Mat4>,
}

impl Default for Scene {
    fn default() -> Self {
        Self { items: Vec::new(), sprites: Vec::new(), clear_color: [0.35, 0.55, 0.85, 1.0], sun_direction: Vec3::new(0.4, 1.0, 0.3).normalize(), ambient: 0.35, mirror: false, headlight: None, beacon: None }
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
