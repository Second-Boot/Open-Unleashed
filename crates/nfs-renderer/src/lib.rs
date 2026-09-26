//! wgpu (Metal on macOS) renderer with an engine-level API.
//!
//! Nothing here knows about CRP/SHPI files; `nfs-assets` converts original data into
//! [`MeshData`] / [`TextureData`] and this crate uploads and draws them.

mod camera;
mod gpu;

pub use camera::Camera;
pub use gpu::{MaterialId, MeshId, Renderer, TextureId, VIEW_FAR};

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
    /// `R3DSpecularTextureLayer`'s texture (0x445520), sphere-mapped by the eye-space normal —
    /// the car's own `[0x6115CC]+0xDC + 4·bucket`, which `RCar::load` fills with `SPC1` / `SPC2` /
    /// `SPC3` of `CarModel/carcmn.fsh` (0x441AB2 → 0x440940; `RE/notes/lighting.md` §3.1). `None`
    /// is no specular pass: the layer skips its pass's draw when it has no texture (0x4455A2).
    pub env_texture: Option<TextureId>,
    /// 1 when the material has the specular pass (with `env_texture`); the pass's combine is its
    /// `Rs` block, so this is only the switch.
    pub env_mix: f32,
    /// The bucket `R3DSpecularTextureLayer` and `R3DSkyMapTextureLayer` take from the material's
    /// reflection strength (0.05 / 0.95, 0x44553F / 0x445B33) — which of the three textures.
    pub env_variant: usize,
    /// The material's method carries a built `R3DSkyMapTextureLayer` pass (class 0x8101, 0x445B10):
    /// it then samples the car's section map `DrawItem::sky`'s `textures[env_variant]`.
    pub sky_map: bool,
    /// The frame-buffer blend of an `alpha_blend` material, as Thrash state 0x38 takes it
    /// (`dx7z.dll` 0x600046BB, `RE/notes/thrash.md` §6): 0 `SRCALPHA` / `INVSRCALPHA`, 1 `SRCALPHA` /
    /// `ONE`, 2 `ZERO` / `INVSRCALPHA`, 3 `DESTCOLOR` / `ZERO`.
    pub blend_pair: u8,
    /// Clamp the texture in u / v instead of repeating it — Thrash states 0x41 / 0x42 = 0
    /// (`ADDRESSU` / `ADDRESSV` `CLAMP`, `RE/notes/thrash.md` §7).
    pub clamp: [bool; 2],
    /// The texture's mip LOD bias — Thrash state 8, `D3DTSS_MIPMAPLODBIAS` (`thrash.md` §7).
    pub lod_bias: f32,
    /// The race's draw state, which the port gives the **car's** (and the front-end rooms', which
    /// are car models, `RE/notes/frontend-3d.md` §9) and the **track's** materials:
    /// depth writes even when blended and the alpha test `GREATER 10`. The device is made with
    /// `ZWRITEENABLE` 1, `ZFUNC` `LESSEQUAL`, `ALPHATESTENABLE` 1, `ALPHAFUNC` `GREATER`
    /// (0x6000A4FF…0x6000A549); the game keeps depth writes on through the view's settings (Thrash
    /// 0x39 ← 1, 0x468B39) and sets the alpha reference to 10 through Thrash 0x24 (0x443615, the
    /// car draw, and 0x43C876 at the start of `R3DCarLOD::draw`, which draws the rooms too;
    /// 0x452085, 0x45D68D, 0x466703, `thrash.md` §6d.4). APPROXIMATION for everything else: the
    /// port's other blended draws (particles, 0x454456 sets 1) keep depth writes off until
    /// their draw order is the original's; the lamp glare and beam draw with them off in the
    /// original too (0x43D81F, `DrawItem::alpha_test`). *(The front-end rooms were among them until 2026-09-25,
    /// on the reading that their views set 0x24 to 1 — 0x4C6696 / 0x4DA923 are 2D draws.)* The
    /// alpha a texture keeps is the decoded file's — the loader's pick of the surface format is
    /// not read.
    pub device_states: bool,
    /// The cull follows the view's: the `Rs` cull 1 hands Thrash state 2 the view's own mode
    /// (0x540590 → `[[[0x69ED0C]]+4]+0x10`), which the race pass sets to `CW` (2), or to `CCW` (1)
    /// under `MIRROR` (0x468C2C…0x468C55) — so in the mirrored world the other winding is culled,
    /// the same back faces. Without it a mirrored draw culls nothing (the port's rule for the
    /// car, whose own flips 0x443385 / 0x44347F are not carried).
    pub view_cull: bool,
    /// Thrash state 0x18, the depth bias the `Rs` block hands the driver (0x540500 adds the view's
    /// own, `[[[0x69ED0C]]+4]+0x30`, which stays 0): `dx7z.dll` adds `b / 32768` to the vertex's
    /// linear screen z `(z − near) / (far − near)`, so a vertex moves `−b · (far − near) / 32768`
    /// of view depth toward the eye along its own ray (`RE/notes/thrash.md` §6d.3).
    pub depth_bias: i32,
    /// `RCustomLayers::LayerFalseSort1` / `2` (0x449AA0 / 0x449AF0, the `FakeSort` method's ops
    /// 0x800F / 0x8010): at `SPECIALFX` 3 the material is drawn twice, first with depth writes off
    /// (Thrash 0x39 ← 0), then on (← 1) — blended both times (`RE/notes/thrash.md` §6d.4).
    pub false_sort: bool,
}

impl MaterialDesc {
    pub fn textured(texture: TextureId) -> Self {
        Self { texture: Some(texture), alpha_blend: false, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false }
    }
    pub fn flat() -> Self {
        Self { texture: None, alpha_blend: false, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false }
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
    /// (0x446130), for an item whose material carries layer class 0x8102 while its car stands near a
    /// kind-4 `ef` part (`RE/notes/crp-geometry.md` §8).
    pub shadow: Option<([f32; 4], [f32; 4])>,
    /// A blended item the original submits **before** the opaque draws that follow it in the
    /// scene — a car's blob shadow (slots 22 / 23, 0x43B820, ahead of the car's own parts): it is
    /// drawn in the opaque stage, in submission order, still blended and without depth writes, so
    /// the parts drawn after it paint over it whatever its depth bias pulled it to. The port's
    /// stage split (sky, opaque, blended) is its own; the original draws in queue order.
    pub early: bool,
    /// The draw's own Thrash state 0x18 — a state object's `+0x10` (0x537A60) set per draw, as the
    /// lamp glare's `fistp(b · −20000 · cam[+0xC] / d²)` (0x43D419…0x43D45C) — added to the
    /// material's `depth_bias`: `dx7z.dll` adds `b / 32768` to the vertex's linear screen z, so
    /// the vertex slides toward the eye along its own ray (`RE/notes/thrash.md` §6, §6d.3).
    pub depth_bias: i32,
    /// The alpha test `GREATER 10` for this draw alone — Thrash 0x24 ← 10 at the start of the
    /// car's effect pass (0x43D83E), which the lamp glare 0x43CDD0 draws under (its texel alpha
    /// times the vertex alpha `b`), where the night beam 0x43D4F0 puts 0 around its own draw
    /// (0x43D506 / 0x43D7DB). A material with `device_states` is tested whatever this says.
    pub alpha_test: bool,
    /// `R3DSkyMapTextureLayer` (0x445B10) for this car's items whose material has the pass
    /// ([`MaterialDesc::sky_map`]); `None` when the car binds no section maps.
    pub sky: Option<SkyMap>,
    /// Drawn only in the main race view — the view whose `+0x12C` is 2 (0x468BEF), not the
    /// rear-view mirror's (1, 0x468D2F): the weather's draw is gated on it (0x45913C).
    pub main_view_only: bool,
    /// Drawn in the sky pass (0x45D4F0): fogged with [`Scene::sky_fog`] over that pass's planes
    /// 0.15 / 700 rather than with [`Scene::fog`].
    pub sky_pass: bool,
}

impl DrawItem {
    pub fn new(mesh: MeshId, material: MaterialId, transform: Mat4) -> Self {
        Self { mesh, material, transform, light: [1.0; 3], alpha: 1.0, uv_offset: [0.0; 2], layer_light: None, headlit: false, uv_lit: false, shadow: None, early: false, depth_bias: 0, alpha_test: false, sky: None, main_view_only: false, sky_pass: false }
    }
}

/// One car's `R3DSkyMapTextureLayer` 0x445B10 (`RE/notes/lighting.md` §3.2): the three maps the
/// car binds for its slice's `.map` section (`EnvSet::texture` 0x441180, `ids[layer + 3·covered]`,
/// bound per car at 0x43A73D…0x43A778) and the two planes its coordinates come from. Per vertex
/// the layer takes `p = normal + 3.33 · position` in the part's own frame ([0x6115CC]+0, set once
/// by 0x440783), carries it to the world through the object's matrix and gives
/// `u = u.xyz · p_world + u.w`, `v` likewise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyMap {
    /// the section's three maps; a material samples the one its bucket names, and none is no pass
    pub textures: [Option<TextureId>; 3],
    pub u: [f32; 4],
    pub v: [f32; 4],
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
    /// A triangle instead of the quad: three corners in the same screen units as `pos`, each with
    /// its own colour (`pos` / `size` / `rotation` / `color` are then unused) — for the HUD's
    /// untextured polygons such as H321Go's sweep (0x466100) and the billboard's mitred frame.
    pub tri: Option<([[f32; 2]; 3], [[f32; 4]; 3])>,
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
    /// `Headlights::LayerUVLight`'s own gate (0x453CA3): lamp **0** of the camera's car lit —
    /// the vertex light takes lamp 0 or 1 (0x453177…0x4531BC), so with lamp 0 broken and lamp 1
    /// whole the beam still lights the road but the `HEAD` decal is not drawn.
    pub headlight_decal: bool,
    /// World into the nearest lit police car's spinning beacon frame — the second projector on the
    /// same path (0x452A50, `RE/notes/lighting.md` §5).
    pub beacon: Option<Mat4>,
    /// The weather fog the sky manager sets for the scene after its own pass (0x45D3B0, Thrash
    /// 0x15 `EXP`, `nfs_formats::sky::SkyBin::fog_at`), over every 3-D item but the sky's; `None`
    /// is off. The sprites (the HUD) are not fogged.
    pub fog: Option<Fog>,
    /// The sky pass's own fog (0x45D2E0), over the items marked [`DrawItem::sky_pass`].
    pub sky_fog: Option<Fog>,
}

/// Direct3D 7 table fog as `dx7z.dll` sets it for Thrash 0x15 = 4 (`FOGTABLEMODE` `EXP`, 0x600048B7):
/// `f = exp(−density · d)` and the pixel `f · colour + (1 − f) · fog` before the blend. The driver
/// turns table fog on only on a card with `D3DPRASTERCAPS_WFOG` (`[0x60033F6C]`, bit 0x100000 of the
/// triangle raster caps, 0x60008E4C) — without it the state fails and nothing is fogged — so `d` is
/// the **W** of the game's pre-transformed vertices, `1 / rhw` = the view depth in metres
/// (0x5B12AE…, `rhw = 1 / z`). APPROXIMATION of a device behaviour the image cannot settle: whether
/// the runtime fogs on that W or on the screen z `sz` (the game never hands the device a projection
/// matrix — no Thrash 0x1A7 in the image). On `sz`, which runs 0…1 over the view, the shipped
/// densities (at most 0.28³ ≈ 0.02) would fog nothing visibly; on W they are the tracks' haze.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub colour: [f32; 3],
    pub density: f32,
}

impl Default for Scene {
    fn default() -> Self {
        Self { items: Vec::new(), sprites: Vec::new(), clear_color: [0.35, 0.55, 0.85, 1.0], sun_direction: Vec3::new(0.4, 1.0, 0.3).normalize(), ambient: 0.35, mirror: false, headlight: None, headlight_decal: false, beacon: None, fog: None, sky_fog: None }
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
