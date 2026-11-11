//! wgpu (Metal on macOS) renderer with an engine-level API.
//!
//! Nothing here knows about CRP/SHPI files; `nfs-assets` converts original data into
//! [`MeshData`] / [`TextureData`] and this crate uploads and draws them.

/// The chosen GPU and graphics API (`"Apple M1 Pro · Metal"`), set once at start-up — for diagnostics and the demo title.
pub static ADAPTER_LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

static POINT_FILTER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `nfs5.ini` `Filtering=0` (the Verok patch, dll 0x100035F0 over the Thrash state setter, `RE/notes/nfs5-dll.md`
/// §3.3): every texture sampler takes `POINT` magnification and minification instead of `LINEAR`. Read once by
/// `Renderer::new`, so call it before the renderer is made. Off by default (`Filtering=1`).
pub fn set_point_filter(on: bool) {
    POINT_FILTER.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn point_filter() -> bool {
    POINT_FILTER.load(std::sync::atomic::Ordering::Relaxed)
}

mod adapt;
mod ao;
mod bloom;
mod camera;
mod camfx;
mod classes;
mod coat;
// every diagnostic it holds is macOS-only (EDR)
#[cfg(target_os = "macos")]
mod dbg;
mod dof;
mod drops;
mod edr;
mod enhancements;
mod front;
mod gfx;
mod gpu;
mod mipgen;
mod probe;
mod prof;
mod haze;
mod lbeam;
mod shafts;
mod sharpen;
pub use prof::{LiveStats, live_set, live_stats, timestamps_supported};
mod smaa;
mod soft;
mod temporal;
mod upscale;
mod wgsl;

pub use camera::Camera;
pub use camfx::MotionPrev;
pub use dof::{Dof, F_MAX as DOF_F_MAX, F_MIN as DOF_F_MIN};
pub use drops::Drops;
pub use haze::{HAZE_LAMPS, Haze, HazeLamp};
pub use lbeam::{LBeam, LBeams, MAX_LBEAMS};
pub use shafts::Shafts;
pub use enhancements::{E1, E2, E3, E4, E5, E6, E7, E8, E9, E12, E10, E11, E16, E15, E13, E17, E14, E18, E19, E20, E21, E23, E24, E25, E22, E27, E28, E29, E26, E31, E32, E33, E34, E34_TEMPORAL_LEVELS, E35, E36, E37, E38, E39, E40, E41, E42, E43, E44, E47, UP_OFF, UP_METALFX, UP_FSR, upscaler_choices, MfxMode, mfx_mode, mfx_scale_index, mfx_scale_count, mfx_level, mfx_switch_mode, fsr_scale_index, fsr_sharpness, msaa_samples_wanted, render_scale, fov_bonus_deg, draw_reach, tonemap_mode, shaft_strength, rain_gain, street_lamp_pool, Cond as EnhancementCond, hd_textures_available, set_hd_textures_available, DEPENDS as ENHANCEMENT_DEPENDS, Dependency as EnhancementDependency, Blocked as EnhancementBlocked, Enhancements, GpuClass, classify_gpu, LITE_PRESET, Entry as EnhancementEntry, visible_count as enhancement_visible_count, visible_index as enhancement_visible_index, MAX_LEVELS as MAX_ENHANCEMENT_LEVELS, MAX_SWITCHES as MAX_ENHANCEMENTS, ON_OFF as ENHANCEMENT_ON_OFF, REGISTRY as ENHANCEMENT_REGISTRY, Stored as EnhancementStored, Switch as EnhancementSwitch};
pub use front::Renderer;
pub use mipgen::MipAlpha;
/// ENHANCEMENT E41: the E2 mip chain generator (levels 1.. of a base image), for the HD textures' own chains
pub use mipgen::chain as mip_chain;
pub use gpu::{MaterialId, MeshId, OffscreenSpec, ResourceStats, REFL_FACES, REFL_TAN_HALF, STILL_MAX_PIXELS, TextureId, VIEW_FAR, still_size};

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
    /// 0x452085, 0x45D68D, 0x466703, `thrash.md` §6d.4). *(The front-end rooms were among them until 2026-09-25,
    /// on the reading that their views set 0x24 to 1 — 0x4C6696 / 0x4DA923 are 2D draws.)*
    /// Every other blended draw the race makes — the particle systems (0x454456 sets the alpha
    /// ref 1; the depth writes off/on bracket is 0x45477C / 0x45484E, `thrash.md` §6d.6), the skid
    /// marks (0x47A078 / 0x47A0B8), the weather's rain and snow (0x45919F / 0x4591CA), the
    /// windscreen drops (0x459200), the car's blob shadow (0x43B820) and the lamp glare, beam and
    /// road-reflection quad (0x43D81F…0x43DE76, `DrawItem::alpha_test`) — turns depth writes off
    /// around its own draw in the original too, confirmed to the instruction (`thrash.md` §6d.6,
    /// 2026-09-28): `device_states` defaulting false for all of them is the settled state, not a
    /// stand-in for an order not yet read, and their relative order does not read back through the
    /// depth buffer. What is still the port's own: the **mutual** order of the `device_states:
    /// true` items (cars, the skid marks and the track's flag-1 / flag-4 parts share one
    /// distance-sorted list, `+0xC`, in the original — `RENDERING.md` "The view's lists" — but
    /// draw in separate blocks here), which does write depth and so does matter; that gap is
    /// unchanged by this reading. The alpha a texture keeps is the decoded file's — the loader's
    /// pick of the surface format is not read.
    pub device_states: bool,
    /// The cull follows the view's: the `Rs` cull 1 hands Thrash state 2 the view's own mode
    /// (0x540590 → `[[[0x69ED0C]]+4]+0x10`), which the race pass sets to `CW` (2), or to `CCW` (1)
    /// under `MIRROR` (0x468C2C…0x468C55) — so in the mirrored world the other winding is culled,
    /// the same back faces. Without it a mirrored draw culls nothing (the port's rule for the
    /// car, whose own flips 0x443385 / 0x44347F are not carried).
    pub view_cull: bool,
    /// Thrash state 0x18, the depth bias the `Rs` block hands the driver (0x540500 adds the view's
    /// own, `[[[0x69ED0C]]+4]+0x30`, which stays 0): `b · 2^-15` added to the screen z of the
    /// game's own pre-transformed vertex, `far/(far − near) · (1 − near / Z)` ([`depth_bias_offset`],
    /// 0x540CE0; `RE/notes/thrash.md` §6d.3).
    pub depth_bias: i32,
    /// `RCustomLayers::LayerFalseSort1` / `2` (0x449AA0 / 0x449AF0, the `FakeSort` method's ops
    /// 0x800F / 0x8010): at `SPECIALFX` 2 or 3 the material is drawn twice, first with depth
    /// writes off (Thrash 0x39 ← 0), then on (← 1) — blended both times. Below 2 (`Renderer::
    /// set_fakesort_mode`), 0x44EC80's other arm draws it once, `MAG`/`MINFILTER` forced
    /// `POINT` for that draw (`RE/notes/thrash.md` §6d.4). Which of the two happens at `SPECIALFX`
    /// 2 is a runtime pick, [`FakeSortMode`].
    pub false_sort: bool,
    /// ENHANCEMENT E8: the material's class for the dynamic body reflection ([`Reflect`]);
    /// [`Reflect::NONE`] = untouched (every material but a car's paint and glass).
    pub reflect: Reflect,
}

/// ENHANCEMENT E8: how a material takes the player's dynamic environment reflection
/// (`RE/notes/ENHANCEMENTS.md`, E8 class table). `class` 0 = none, 1 = body paint (clear coat),
/// 2 = glass; `strength` is the material's own reflection strength from the file (`mt+284`, 0.5 on
/// the paint, 1.0 on the glass — `nfs_formats::crp::Material::reflection`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reflect {
    pub class: u8,
    pub strength: f32,
    /// ENHANCEMENT E16: a cabin material (`CarInt`, `CarIntShd`): its ambient stays the original's scalar
    pub interior: bool,
}

impl Reflect {
    pub const NONE: Reflect = Reflect { class: 0, strength: 0.0, interior: false };
    /// `class` of a car's body paint (`CarExtEnv`, the clear coat)
    pub const PAINT: u8 = 1;
    /// `class` of a car's glass (`CarWindow`)
    pub const GLASS: u8 = 2;
}

/// `0x44EC80`'s per-frame pick for a `false_sort` material, read to the instruction 2026-09-27
/// (`RE/notes/thrash.md` §6d.4). The jump table (0x44ECF4) on `SPECIALFX` `[0x657D6C]`: 0 and 1 →
/// always [`Single`](Self::Single) (0x44EC91); 3 → always [`Double`](Self::Double) (0x44ECE7); 2 →
/// two independent tests, **not** one falling through to the other:
///
/// * `[0x657424]` (`REPLAY`) **≥ 2** (playing back or exhausted) picks between `Double` (the
///   view slot 0's half-angle `[0x627ED8]` in degrees — the trackside camera's zoom, not a frame
///   time (corrected 2026-09-30) — under 37.5) and [`PerItem`](Self::PerItem) (0x44ECAA…0x44ECDC) —
///   the player-speed test below is *not* reached on this path;
/// * `REPLAY` **< 2** picks between `Single` (the player car's `+0x35C` ≥ 15.0) and `PerItem`
///   (0x44ECC4…0x44ECDC) — the frame-rate test is *not* reached on this path.
///
/// *(Corrects `thrash.md`'s 2026-09-27 summary, which read the two tests as one chain with the
/// speed check as the frame-rate check's "else": the disassembly shows `REPLAY < 2` jumping clear
/// over the frame-rate block, so a replay's frame rate never falls back to the speed test and a
/// live race's speed never falls back to the frame-rate test.)*
///
/// `PerItem` is `LayerFalseSort1`/`2`'s own fold (0x449AA0/0x449AF0, `mode == 1`): per drawn
/// article, `[0x61975C]` (an integer, `cmp …, 0x2710` **unsigned** — `DrawItem::fakesort_near`)
/// under 10000 draws it like `Double`, else like `Single`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FakeSortMode {
    /// `LayerFalseSort1`'s point-filter arm: one draw, `MAG`/`MINFILTER` forced `POINT`.
    Single,
    /// The depth-toggled pair: blended both times, first with depth writes off.
    #[default]
    Double,
    /// `mode 1`: `Double` while the drawn item's own `[0x61975C]` equivalent
    /// ([`DrawItem::fakesort_near`]) is set, `Single` otherwise.
    PerItem,
}

impl MaterialDesc {
    pub fn textured(texture: TextureId) -> Self {
        Self { texture: Some(texture), alpha_blend: false, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false, reflect: Reflect::NONE }
    }
    pub fn flat() -> Self {
        Self { texture: None, alpha_blend: false, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, sky_map: false, blend_pair: 0, clamp: [false; 2], lod_bias: 0.0, device_states: false, view_cull: false, depth_bias: 0, false_sort: false, reflect: Reflect::NONE }
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
    /// material's `depth_bias` and offset the screen z the same way ([`depth_bias_offset`],
    /// `RE/notes/thrash.md` §6d.3).
    pub depth_bias: i32,
    /// The alpha test `GREATER 10` for this draw alone — Thrash 0x24 ← 10 at the start of the
    /// car's effect pass (0x43D83E), which the lamp glare 0x43CDD0 draws under (its texel alpha
    /// times the vertex alpha `b`), where the night beam 0x43D4F0 puts 0 around its own draw
    /// (0x43D506 / 0x43D7DB). A material with `device_states` is tested whatever this says.
    pub alpha_test: bool,
    /// The reference of that test, `ALPHAREF` (Thrash 0x24's value, `dx7z.dll` 0x60003F9C): 10 as
    /// the race sets it (0x443615, 0x45D68D …), 1 after a particle system's draw (0x454456).
    pub alpha_ref: u8,
    /// `R3DSkyMapTextureLayer` (0x445B10) for this car's items whose material has the pass
    /// ([`MaterialDesc::sky_map`]); `None` when the car binds no section maps.
    pub sky: Option<SkyMap>,
    /// Drawn only in the main race view — the view whose `+0x12C` is 2 (0x468BEF), not the
    /// rear-view mirror's (1, 0x468D2F): the weather's draw is gated on it (0x45913C).
    pub main_view_only: bool,
    /// Drawn in the sky pass (0x45D4F0): fogged with [`Scene::sky_fog`] over that pass's planes
    /// 0.15 / 700 rather than with [`Scene::fog`].
    pub sky_pass: bool,
    /// The W the fog reads for this item, in metres, when it is not the geometric view depth: the
    /// sun's screen quad (0x451710, `rhw` 1) has W = 1 whatever the plane it is laid in — the port
    /// lays it at twice the mirror's near plane, the mirror's projection keeping near 1 where the
    /// sky pass's own is 0.15 (0x45D51D). 0 = the geometric depth.
    pub fog_w: f32,
    /// [`FakeSortMode::PerItem`]'s own test (`[0x61975C]` under 10000, `LayerFalseSort1`/`2`'s
    /// `mode 1` fold, `RE/notes/thrash.md` §6d.4): the value is the queued entry's own `+0xC` key
    /// (`round(d²)` / `round(v)`, `RE/RENDERING.md` "The view's lists"). Read to the instruction
    /// (2026-09-30, `xrefs 0x61975C`): the global has exactly two writers — `RDrawAnimArticleBundle`'s
    /// draw 0x450010 (`mov [0x61975C], eax` at 0x4500BE, `eax = [item+0xC]`) and
    /// `RDrawArticleBundle`'s draw 0x450CA0 (0x450CE2, `edx = [ebx+0xC]`) — and two readers, the
    /// `LayerFalseSort` fold 0x449AB2 / 0x449B02. The only materials with the fold are the track's
    /// (`world.rs`'s `false_sort`), whose every draw comes through one of the two writers just before
    /// it, so the item's own queue key is exactly what the fold reads. Unused outside a
    /// `false_sort` material.
    pub fakesort_near: bool,
    /// Drawn after the whole 3-D pass and the frame read-back ([`LensProbe`]), under no depth test:
    /// the lens flare's elements, which 0x460460 lays flat on the screen (screen z 0, 0x451710)
    /// after reading the finished frame's pixels at the sun (`RE/notes/lighting.md` §7). The
    /// item's material must have no depth test.
    pub overlay: bool,
    /// Drawn through a reflecting transform (a negative determinant): the winding flips, so the
    /// item culls as a draw in a mirrored world does ([`Scene::mirror`]). `R3DCar` slot 3
    /// **0x443490**, the front end's reflection draw (`car+0x471`, 0x4432E5), puts **0x467F90**
    /// (`3 − cull` on the view) around itself for the same reason (0x4434A2 / 0x4435F0).
    pub mirrored: bool,
    /// ENHANCEMENT E11: a boost above 1 for the item's bright part in the float scene (the sun's and the
    /// lens flare's layers); 1 = none. Ignored with E11 or E7 off.
    pub hdr_boost: f32,
    /// ENHANCEMENT E9: the sun shadows' roles of a track item, [`CSM_RECEIVE`] | [`CSM_CAST`]: every
    /// track item takes a car's shadow on its baked colour, scenery (a track article part whose
    /// material is not drivable ground, an instance prop, a smackable, a breakable's piece) also
    /// casts. Cars are recognised by their own `R3DLightLayer` (a nonzero light direction) and need
    /// no flag; effects, the sky and the front end leave it 0. Read only by the renderer.
    pub csm: u8,
    /// ENHANCEMENT E12: a blended world sprite that fades out where it meets geometry — the
    /// distance in metres over which its alpha (colour, for the multiplying blend) goes from 0 at
    /// the surface behind it to full (`saturate((scene depth − own depth) / soft)`); 0 = a hard
    /// edge as the original draws it. Read only while E12 is on.
    pub soft: f32,
    /// ENHANCEMENT E17: the item is water (a horizontal-water material, `nfs_assets::track::is_water`); read only with E17 on.
    pub water: bool,
    /// ENHANCEMENT E18: the kind of a particle draw, [`PARTICLE_SPARK`] / [`PARTICLE_LIT`] / [`PARTICLE_LIT_WORLD`]; 0 = none.
    /// Read only with E18 and E7 on.
    pub particle: u8,
    /// ENHANCEMENT E26: the road's wetness 0..1 at this part's slice (the sky `.bin`'s field 5; an item with
    /// [`CSM_ROAD`] set). Read only with E26 on and `Scene::rain` set.
    pub wet: f32,
    /// ENHANCEMENT E9 (SHADOWFIX): the item only casts a sun shadow — it is not drawn in any pass. The race scene
    /// appends the roadside scenery the view walk left out (outside the frustum, behind an occluder), so a tree or
    /// a cliff just past the screen edge keeps its shadow instead of popping it in. Read only by the renderer.
    pub shadow_only: bool,
    /// ENHANCEMENT E37 (the temporal upscaler's motion vectors): a stable identity of the item across frames -
    /// the car and the part of it (a wheel, the body, a lamp quad), a prop - so the previous frame's transform is
    /// found exactly instead of by the (mesh, material, order) pairing of `temporal.rs`. 0 = none (the pairing).
    /// Never read by the original's path: only the temporal pass looks at it. See [`assign_motion_ids`].
    pub motion_id: u32,
    /// ENHANCEMENT E9 (SHADOW-REAL): a car's `SHAD` patch ([`CSM_CAR_BLOB`]): how covered the car stands, 0..1 ([`cover_weight`] of
    /// `Scene::cover` at the patch): the patch gives way to the sun's shadow by its own place, not by the camera's. 0 elsewhere. Read
    /// only by the renderer while E9 draws shadows.
    pub cover: f32,
}

/// ENHANCEMENT E37: give every item of one object (a car's parts) a [`DrawItem::motion_id`]: a hash of `group` (the
/// object's own number: the player, an opponent, a traffic / police car), the item's mesh and the count of items of
/// that mesh before it in `items` (a car's four wheels share one mesh and keep their order). Never 0.
pub fn assign_motion_ids(items: &mut [DrawItem], group: u32) {
    let mut seen: Vec<(u32, u32)> = Vec::new();
    for it in items.iter_mut() {
        let k = match seen.iter_mut().find(|(m, _)| *m == it.mesh.0) {
            Some(e) => {
                e.1 += 1;
                e.1
            }
            None => {
                seen.push((it.mesh.0, 0));
                0
            }
        };
        // FNV-1a over the three numbers
        let mut h = 0x811C_9DC5u32;
        for v in [group, it.mesh.0, k] {
            for b in v.to_le_bytes() {
                h = (h ^ b as u32).wrapping_mul(0x0100_0193);
            }
        }
        it.motion_id = MOTION_CAR | (h & MOTION_PAYLOAD).max(1);
    }
}

/// ENHANCEMENT E37 / ENH-MOTIONID: the top four bits of a [`DrawItem::motion_id`] name the kind of object, the
/// low 28 its identity inside the kind, so two kinds never collide: [`MOTION_CAR`] (a hash, [`assign_motion_ids`]),
/// [`MOTION_ITEM`] (an instance item that moves: a smackable's or an animated prop's, exact: its index in the
/// world's item list), [`MOTION_ARTICLE`] (a `WAnimBundle` article's part, exact: `motion_article`),
/// [`MOTION_PIECE`] (a breakable's flying piece, exact: `motion_piece`).
pub const MOTION_CAR: u32 = 1 << 28;
pub const MOTION_ITEM: u32 = 2 << 28;
pub const MOTION_ARTICLE: u32 = 3 << 28;
pub const MOTION_PIECE: u32 = 4 << 28;
const MOTION_PAYLOAD: u32 = 0x0FFF_FFFF;

/// The id of an instance item (index `k` in the world's item list); 0 (no id) past the 28 bits.
pub fn motion_item(k: usize) -> u32 {
    if k as u64 <= MOTION_PAYLOAD as u64 { MOTION_ITEM | k as u32 } else { 0 }
}

/// The id of part `part` of LOD `lod` of the animated article drawn as entry `g` of the track's article list;
/// 0 when a number does not fit its field (`g` < 4096, `lod` < 8, `part` < 256).
pub fn motion_article(g: usize, lod: usize, part: usize) -> u32 {
    if g < 4096 && lod < 8 && part < 256 { MOTION_ARTICLE | (g as u32) << 11 | (lod as u32) << 8 | part as u32 } else { 0 }
}

/// The id of the `n`-th item of the pieces of breakable `b` (`b` < 65536, `n` < 4096); 0 when it does not fit.
pub fn motion_piece(b: usize, n: usize) -> u32 {
    if b < 65536 && n < 4096 { MOTION_PIECE | (b as u32) << 12 | n as u32 } else { 0 }
}

/// [`DrawItem::csm`]: the item is road (surface type 1, tarmac), which the rain (E26) wets.
pub const CSM_ROAD: u8 = 4;

/// [`DrawItem::particle`]: a spark streak, drawn far above white so the bloom makes it glow.
pub const PARTICLE_SPARK: u8 = 1;
/// [`DrawItem::particle`]: a billboard of smoke / dust / spray, lit by the sun's side and the sun's shadow at its centre
/// (the item's translation).
pub const PARTICLE_LIT: u8 = 2;
/// [`DrawItem::particle`]: world-space triangles of snow, lit and shadowed per fragment.
pub const PARTICLE_LIT_WORLD: u8 = 3;

/// [`DrawItem::csm`]: the item's colour is darkened where a sun shadow falls on it.
pub const CSM_RECEIVE: u8 = 1;
/// [`DrawItem::csm`]: the item casts a sun shadow.
pub const CSM_CAST: u8 = 2;
/// [`DrawItem::csm`] ENHANCEMENT ENH-FE3D: a front-end room's own piece (floor, walls, platform). It receives the car's
/// shadow like the track does and never casts one (its own `R3DLightLayer` would otherwise make it a car).
pub const CSM_ROOM: u8 = 8;
/// [`DrawItem::csm`] ENHANCEMENT E9: the car's own `SHAD` ground patch (0x43B820). Where the sun's shadow map draws the
/// car's shadow (a frame with cascades, in the open) it fades out with the cover gate, as the `sh80` blob does —
/// otherwise the car has two shadows (2026-10-03). Inert without E9.
pub const CSM_CAR_BLOB: u8 = 16;
/// [`DrawItem::csm`] ENHANCEMENT ENH-DBLSH: a track's `sh80` ground-shadow decal (`S_Normal` / `S_UVAnimate`, depth
/// biased) with a sun-shadow caster standing over it. Where the sun's shadow map draws that object's shadow the
/// decal fades out with the cover gate (as the car's `sh80` blob does); otherwise two shadows lie on the ground.
pub const CSM_DECAL: u8 = 32;
/// [`DrawItem::csm`] ENHANCEMENT E26: a skid-mark strip (`SKDP`) on a track with a wet stretch; `wet` is its wetness. On wet
/// tarmac the rubber shows lighter and glossy (the tyre wiped the water film). Read only with E26 on and `Scene::rain` set.
pub const CSM_SKID: u8 = 64;

impl DrawItem {
    pub fn new(mesh: MeshId, material: MaterialId, transform: Mat4) -> Self {
        Self { mesh, material, transform, light: [1.0; 3], alpha: 1.0, uv_offset: [0.0; 2], layer_light: None, headlit: false, uv_lit: false, shadow: None, early: false, depth_bias: 0, alpha_test: false, alpha_ref: 10, sky: None, main_view_only: false, sky_pass: false, fog_w: 0.0, fakesort_near: false, overlay: false, mirrored: false, csm: 0, soft: 0.0, hdr_boost: 1.0, water: false, particle: 0, wet: 0.0, shadow_only: false, motion_id: 0, cover: 0.0 }
    }
}

/// Thrash state 0x18's `b` as the offset it makes in the screen z: `b · 2^-15` (0x540500 /
/// 0x5404A0: `fild`, `× [0x5B3E10]` = 2^-15, into `ctx+0x1180C`; the DX7 driver's own copy
/// 0x60005177 scales by the same `[0x60015450]`). With the DX7 driver the game adds it to the
/// screen z of the vertex it hands the driver (0x540CE0), `sz = far/(far − near) · (1 − near / Z)`
/// — the Direct3D depth of a perspective projection with the view's planes (0x5391B0 folds them
/// into `cam+0xDC` = `far / (far − near)` and `cam+0xE0` = `−near · far / (far − near)`; 0x540820
/// loads those as `ctx+0x34` / `+0x40`, and the kernel 0x5B0730 gives `rhw = 1 / Z`). A positive
/// `b` pushes away, a negative one pulls toward the eye; `RE/notes/thrash.md` §6d.3.
pub fn depth_bias_offset(b: i32) -> f32 {
    b as f32 / 32768.0
}

/// The view depth whose screen z is that of `z` with the bias `b` added — what the offset of
/// [`depth_bias_offset`] amounts to in metres for the view's `near` / `far`:
/// `sz(z') = sz(z) + b · 2^-15` with `sz(z) = far/(far − near) · (1 − near / z)`, so
/// `1/z' = 1/z − b · 2^-15 · (far − near) / (far · near)`. `None` where the biased screen z reaches
/// `far / (far − near)` (no finite depth has it) or falls to 0 or below (in front of the near plane).
pub fn biased_view_depth(z: f64, b: i32, near: f64, far: f64) -> Option<f64> {
    let a = far / (far - near);
    let sz = a * (1.0 - near / z) + b as f64 / 32768.0;
    if sz <= 0.0 || sz >= a {
        return None;
    }
    Some(near / (1.0 - sz / a))
}

/// The `+0xC` list's flush, farthest first (`0x466660`'s walk of the tree the original builds with
/// `0x466A90`, `RE/RENDERING.md` "The view's lists"): each entry keyed by its own `round(d²)` /
/// `round(v)` (a `fistp`-rounded, unsigned-compared `u32` — `RE/notes/thrash.md` §6d.6, 0x466ad3's
/// `jbe`/`ja`, never signed), sorted descending, **ties kept in the order they were given here** —
/// the original's tree inserts an equal key to the right, so it keeps its queueing order
/// (0x466933 / 0x466AD3, and `Vec::sort_by_key`'s own stability). One list holds every kind's
/// entries in the original — the track's flag-1 / flag-4 parts, the cars and the skid marks
/// (`RE/notes/thrash.md` §6d.6) — so the caller merges them into one `Vec` before calling this,
/// rather than sorting each kind's block on its own.
pub fn farthest_first(mut items: Vec<(u32, DrawItem)>) -> Vec<DrawItem> {
    items.sort_by_key(|(k, _)| std::cmp::Reverse(*k));
    items.into_iter().map(|(_, i)| i).collect()
}

/// The entries of `items` in the order [`farthest_first`] (`descending`) or an ascending stable sort
/// by key gives, as `(key, index)` pairs in `order` (cleared first) — the fat items are not moved.
/// The index breaks every tie, so equal keys keep the order they were given in, as a stable sort
/// does (`sort_unstable` on the pairs is therefore the same order); a descending key is sorted as
/// its bitwise complement.
pub fn sort_order(items: &[(u32, DrawItem)], descending: bool, order: &mut Vec<(u32, u32)>) {
    order.clear();
    order.extend(items.iter().enumerate().map(|(i, (k, _))| (if descending { !*k } else { *k }, i as u32)));
    order.sort_unstable();
}

/// [`farthest_first`] without consuming or moving the items: `items` stays as it was, the run is
/// appended to `out`, and `order` is the scratch the sort uses.
pub fn farthest_first_into(items: &[(u32, DrawItem)], order: &mut Vec<(u32, u32)>, out: &mut Vec<DrawItem>) {
    sort_order(items, true, order);
    out.extend(order.iter().map(|&(_, i)| items[i as usize].1));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(n: u32) -> DrawItem {
        // `mesh`/`material` carry no data of their own here — only `depth_bias` distinguishes
        // one item from another once the key is stripped off
        DrawItem { depth_bias: n as i32, ..DrawItem::new(MeshId::default(), MaterialId::default(), Mat4::IDENTITY) }
    }

    /// ENHANCEMENT E9 (SHADOW-REAL): the CPU cover weight is the shader's `cover_at`: whole inside a run, ramping at its end face,
    /// nothing beside, above the ceiling or below the road.
    #[test]
    fn cover_weight_follows_the_box_and_its_ramp() {
        let b = CoverBox { centre: Vec3::ZERO, forward: Vec3::Z, lateral: Vec3::X, up: Vec3::Y, half_len: 6.0, half_nom: 3.0, half_wid: 8.0, ceil: 6.5, w_back: 2.0, w_front: 0.0 };
        let boxes = [b];
        assert_eq!(cover_weight(&boxes, Vec3::new(0.0, 1.0, -3.0)), 1.0);
        assert!((cover_weight(&boxes, Vec3::new(0.0, 1.0, 1.5)) - 0.5).abs() < 1e-5);
        assert_eq!(cover_weight(&boxes, Vec3::new(0.0, 1.0, 3.0)), 0.0);
        assert_eq!(cover_weight(&boxes, Vec3::new(9.0, 1.0, -3.0)), 0.0);
        assert_eq!(cover_weight(&boxes, Vec3::new(0.0, 7.0, -3.0)), 0.0);
        assert_eq!(cover_weight(&boxes, Vec3::new(0.0, -2.5, -3.0)), 0.0);
        assert_eq!(cover_weight(&[], Vec3::ZERO), 0.0);
    }

    #[test]
    fn farthest_first_sorts_descending() {
        let out = farthest_first(vec![(10, item(1)), (30, item(2)), (20, item(3))]);
        assert_eq!(out.iter().map(|i| i.depth_bias).collect::<Vec<_>>(), vec![2, 3, 1]);
    }

    /// Equal keys keep the order they were given in — the original's tree insert goes right on
    /// an equal key, so it never reorders same-key entries (`RE/RENDERING.md` "The view's lists").
    /// The index-sorted run is the stable sort's, whatever the keys and ties.
    #[test]
    fn farthest_first_into_matches_farthest_first() {
        let mut seed = 12345u32;
        let mut next = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            seed >> 16
        };
        for n in [0usize, 1, 2, 7, 64, 300] {
            let items: Vec<(u32, DrawItem)> = (0..n).map(|i| (if next() % 5 == 0 { u32::MAX - next() % 3 } else { next() % 17 }, item(i as u32))).collect();
            let mut order = Vec::new();
            let mut out = Vec::new();
            farthest_first_into(&items, &mut order, &mut out);
            let expect = farthest_first(items.clone());
            assert_eq!(out.iter().map(|i| i.depth_bias).collect::<Vec<_>>(), expect.iter().map(|i| i.depth_bias).collect::<Vec<_>>());
            // the ascending order is the stable ascending sort
            sort_order(&items, false, &mut order);
            let mut asc = items.clone();
            asc.sort_by_key(|(k, _)| *k);
            assert_eq!(order.iter().map(|&(_, i)| items[i as usize].1.depth_bias).collect::<Vec<_>>(), asc.iter().map(|(_, i)| i.depth_bias).collect::<Vec<_>>());
        }
    }

    #[test]
    fn farthest_first_is_stable_on_equal_keys() {
        let out = farthest_first(vec![(10, item(1)), (10, item(2)), (10, item(3)), (20, item(4))]);
        assert_eq!(out.iter().map(|i| i.depth_bias).collect::<Vec<_>>(), vec![4, 1, 2, 3]);
    }

    /// A merged list interleaves two kinds queued in different blocks (as the cars and the
    /// track's far list are) purely by key, not by which block queued them first.
    #[test]
    fn farthest_first_interleaves_two_kinds_by_key() {
        // "track" queues 40 and 20; "car" queues 30 and 10 — queued in two separate blocks, as
        // the port's cars and track items are collected, but merged here into one list first
        let track = vec![(40, item(1)), (20, item(2))];
        let car = vec![(30, item(3)), (10, item(4))];
        let mut merged = track;
        merged.extend(car);
        let out = farthest_first(merged);
        assert_eq!(out.iter().map(|i| i.depth_bias).collect::<Vec<_>>(), vec![1, 3, 2, 4]);
    }

    /// State 0x18 in metres: the original's `sz + b · 2^-15` (0x540CE0) against the first-order
    /// step `b · 2^-15 · z² · (far − near) / (far · near)` and the old linear reading
    /// `b · (far − near) / 32768`; then the shader's arithmetic — the port's projection's clip z
    /// plus `b · 2^-15 · w` — read back as a view depth, which must be the original's.
    /// `cargo test -p nfs-renderer depth_bias_metres -- --nocapture` prints the table.
    #[test]
    fn depth_bias_metres() {
        for &(near, far) in &[(0.15f64, 700.0f64), (0.5, 700.0)] {
            println!("near {near} far {far}");
            println!("{:>6} {:>4} {:>14} {:>14} {:>12}", "z", "b", "dz original", "dz 1st order", "dz linear");
            for &z in &[0.5f64, 5.0, 50.0, 500.0] {
                for &b in &[1i32, 3, -1, -10] {
                    let dz = biased_view_depth(z, b, near, far).map(|z2| z2 - z);
                    let first = b as f64 / 32768.0 * z * z * (far - near) / (far * near);
                    let linear = b as f64 * (far - near) / 32768.0;
                    println!("{z:>6} {b:>4} {:>14} {first:>14.6} {linear:>12.4}", dz.map_or("past ∞ / near".to_string(), |d| format!("{d:.6}")));
                    // near the eye the step is the first-order one (within 2 %: the second-order term is 1 % at 5 m, b = −10)
                    if let (Some(d), true) = (dz, z <= 5.0) {
                        assert!((d - first).abs() <= 0.02 * first.abs(), "z {z} b {b}: {d} vs {first}");
                    }
                    // the shader: clip = P · view, clip.z += b · 2^-15 · clip.w, then back to a depth
                    let cam = Camera { position: glam::Vec3::ZERO, target: glam::Vec3::new(0.0, 0.0, -1.0), up: glam::Vec3::Y, fov_y_radians: 1.0, near: near as f32, far: far as f32 };
                    let clip = cam.projection(1.0) * glam::Vec4::new(0.0, 0.0, -z as f32, 1.0);
                    let ndc = ((clip.z + depth_bias_offset(b) * clip.w) / clip.w) as f64;
                    let a = far / (far - near);
                    if let Some(z2) = dz.map(|d| z + d) {
                        if ndc > 0.0 && ndc < 1.0 {
                            let back = near / (1.0 - ndc / a);
                            // f32 depth: compare as screen z, not metres (500 m is past f32's step)
                            let sz2 = a * (1.0 - near / z2);
                            assert!((ndc - sz2).abs() < 2e-6, "z {z} b {b}: port {ndc} original {sz2} ({back} m vs {z2} m)");
                        }
                    }
                }
            }
        }
        // the old linear pull is the original's step (to first order) only at z = sqrt(far · near): 0.15 / 700 → 10.2 m
        let z = (700.0f64 * 0.15).sqrt();
        let d = biased_view_depth(z, 1, 0.15, 700.0).unwrap() - z;
        assert!((d - 699.85 / 32768.0).abs() < 0.01 * d, "{d}");
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
    /// One colour per corner of the quad — upper-left, upper-right, lower-right, lower-left, the
    /// order of `HGraphicVisual`'s vertex slots `+0x5C…+0x68` (0x423D70) — instead of the single
    /// `color`, interpolated across the textured, rotated quad (the map's cop blips, 0x433110).
    pub corner_colors: Option<[[f32; 4]; 4]>,
    /// `None`: the port's plain 2D draw — no depth test, no depth write, no alpha test, which is
    /// the original's pixel for every sprite not drawn under a depth mask. `Some`: the front end's
    /// own depth state for this draw ([`SpriteDepth`]).
    pub depth: Option<SpriteDepth>,
}

/// The depth state the front end's 2D kit draws a sprite under, for its depth masks (0x4DAA10 /
/// 0x4DAAC0 / 0x4DAB00, `RE/notes/layouts.md` "Clip = a depth mask"): the sprite's screen z is the
/// kit's current depth `[0x65B954]` (every enqueuer reads it, 0x4D9286 / 0x4D9B36 / 0x4DA374 …),
/// depth writes are on (the view's Thrash 0x39 ← 1 before the 2D pass, 0x4D30BE), the compare is
/// Thrash 0x28 (`ZFUNC` = value + 1: 3 `LESSEQUAL` from 0x4D30C9, 6 `GREATEREQUAL` inside a mask,
/// 0x4DAAC0), and the device's alpha test `GREATER alpha_test` drops a pixel before it writes
/// anything (`dx7z.dll` 0x6000A4FF…0x6000A51C; Thrash 0x24). The sprite pass starts with the depth
/// cleared to 1.0 (0x4DA890 → `THRASH_clearwindow` on the depth window, `dx7z.dll` 0x60001230 →
/// 0x60008380, z = the view's `+0x34`, 1.0 from 0x537741).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpriteDepth {
    /// screen z, 0 (near) … 1 (far)
    pub z: f32,
    pub compare: SpriteCompare,
    /// Thrash 0x24 at the draw's flush (`dx7z.dll` 0x60003F9C): `Some(ref)` — `ALPHATESTENABLE` 1,
    /// `ALPHAFUNC` `GREATER`, `ALPHAREF` = ref: the pixel's 8-bit alpha (texture × vertex colour,
    /// `ALPHAOP` `MODULATE`) must be greater; `None` — the value 0, `ALPHATESTENABLE` 0: every
    /// pixel passes (the list box's stencil, 0x4C6683)
    pub alpha_test: Option<u8>,
}

/// Thrash 0x28's two values the front end's 2D kit uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteCompare {
    /// 3 → `D3DCMP_LESSEQUAL` (0x4D30C9)
    LessEqual,
    /// 6 → `D3DCMP_GREATEREQUAL` (0x4DAAC0)
    GreaterEqual,
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
    /// ENHANCEMENT E24: how strongly the headlight projector and the `HEAD` decal count, 0..1; 1 = the original
    /// (always, but while a car's lamps fade in or out at a tunnel mouth). Read with the projector lit.
    pub headlight_gain: f32,
    /// World into the nearest lit police car's spinning beacon frame — the second projector on the
    /// same path (0x452A50, `RE/notes/lighting.md` §5).
    pub beacon: Option<Mat4>,
    /// The weather fog the sky manager sets for the scene after its own pass (0x45D3B0, Thrash
    /// 0x15 `EXP`, `nfs_formats::sky::SkyBin::fog_at`), over every 3-D item but the sky's; `None`
    /// is off. The sprites (the HUD) are not fogged.
    pub fog: Option<Fog>,
    /// The sky pass's own fog (0x45D2E0), over the items marked [`DrawItem::sky_pass`].
    pub sky_fog: Option<Fog>,
    /// The lens flare's frame read-backs for this frame ([`LensProbe`]), if any.
    pub lens_probe: Option<LensProbe>,
    /// ENHANCEMENT E9: the sun the main race view's shadows come from; `None` (the front end, a
    /// night track, an offscreen view) draws none. Read only with E9 on.
    pub sun_shadow: Option<SunShadow>,
    /// ENHANCEMENT E25: on a track whose sky configuration has no sun disc (the sun is straight overhead, so the cars'
    /// light travels straight down: autobahn, canyon, foothills, forest) the same shadow pass runs with that vertical
    /// light, softer and weaker. Read only with E9 and E25 on, and only while `sun_shadow` is `None`. Nothing else
    /// (fog glow, water, particles, shafts) reads it.
    pub overcast_shadow: Option<SunShadow>,
    /// ENHANCEMENT E17: the sky the water items reflect and the sun that glints on them (the main race
    /// view and its mirror; `None` = no water reflection, the environment faces and the front end). Read only with E17 on.
    pub water: Option<WaterEnv>,
    /// ENHANCEMENT E8: the weight of the dynamic body reflection in this scene (0 = none: the
    /// original). Only the race's main view sets it; the front end's 3-D views and the mirror stay 0.
    pub refl: f32,
    /// ENHANCEMENT E14: this scene takes the corner shading (the race's main view only; the front
    /// end, the mirror and the environment faces stay `false`). Read only with E14 on.
    pub ao: bool,
    /// ENHANCEMENT E20: the lightning flash's strength 0..1 for this view (0 = none). Read only with E20 on.
    pub flash: f32,
    /// ENHANCEMENT E21: this scene's float image feeds the eye adaptation (the race's main view only). Read only with E21 and E7 on.
    pub eye: bool,
    /// ENHANCEMENT E13: the sun's screen point for the light shafts of the main race view (read only
    /// with E13 and E7 on); `None` = none (the front end, night, a sunless track, the sun behind the eye)
    pub shafts: Option<Shafts>,
    /// ENHANCEMENT E15: the lit lamps of this view as spotlights (none = the original's frame). Read
    /// only with E15 on.
    pub spots: Spots,
    /// ENHANCEMENT E27: the previous frame of the race's main view, for the motion blur (`None` = none: the
    /// front end, the mirror, the first frame, a cut). Read only with E27 and E7 on.
    pub motion: Option<MotionPrev>,
    /// ENHANCEMENT E23: the street lamps of this view as point lights (none = the original's frame).
    /// Read only with E23 on.
    pub lamps: Lamps,
    /// ENHANCEMENT E26: the rain's look of this view (the main race view and its mirror of a track whose wetness is
    /// nonzero somewhere; `None` = dry track, the front end, the environment faces). Read only with E26 on.
    pub rain: Option<RainEnv>,
    /// ENHANCEMENTS E7 / E31: the lens of the photo mode (PORT-DEVIATION D46; its depth of field is an extra of the float
    /// scene, E7) or of a replay (E31) (`None` = none). Read only with the float scene (E7).
    pub dof: Option<Dof>,
    /// ENHANCEMENT E36: the air of the main race view for the haze and the visible headlight beams (`None` = none: the front
    /// end, the mirror, a clear dry open road). Read only with E36 and E7 on.
    pub haze: Option<Haze>,
    /// ENHANCEMENT E40: the windscreen's glass of the driver view of a rainy track (`None` = none: every other view, the
    /// mirror, a dry track, under cover). Read only with E40 and E7 on.
    pub drops: Option<Drops>,
    /// ENHANCEMENT E43: the near cars' poses for the soft contact shadow on the road under them (empty = none: the front end,
    /// the mirror, the environment faces, E43 off). Read only with E43 on.
    pub contacts: Contacts,
    /// ENHANCEMENT E9 (ENH-TUNPOOL): the covered stretches of the road (tunnels, galleries) as boxes along the slices, the whole
    /// track's, shared; the renderer sends the nearest ones to the shader, which skips E9's sun-shadow darkening of the
    /// track's baked colour for a fragment inside one (the baked colour there has no sun in it already).
    pub cover: std::sync::Arc<Vec<CoverBox>>,
    /// ENHANCEMENT E44: the lighthouse beam(s) of the main race view as soft volumetric light (`None` = none: every other view, E44
    /// or E7 off, a track without one, none within the view's reach). Read only with E44 and E7 on.
    pub lbeam: Option<LBeams>,
}

/// ENHANCEMENT E9 (ENH-TUNPOOL): one road slice's covered volume: a box round the slice (its own axes), `half_len` along the
/// road (`half_len`: the tested extent, a little more than the slice's own share so the boxes of a bend overlap), `half_wid`
/// across it, from just under the road to `ceil` over it. `w_back` / `w_front` are the cover weights at the faces of the slice's
/// own share of the road (`half_nom` either side of the centre), NOT clamped: above 1 inside a covered run, ramping through 0..1
/// over the first metres of a run's ends, so the portal has no seam; the shader interpolates (and extrapolates) them along the
/// box and clamps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverBox {
    pub centre: Vec3,
    pub forward: Vec3,
    pub lateral: Vec3,
    pub up: Vec3,
    pub half_len: f32,
    pub half_nom: f32,
    pub half_wid: f32,
    pub ceil: f32,
    pub w_back: f32,
    pub w_front: f32,
}

impl CoverBox {
    /// How covered `p` is by this box, 0..1 (0 outside it): the shader's `cover_at` for one box.
    pub fn weight(&self, p: Vec3) -> f32 {
        let r = p - self.centre;
        let l = r.dot(self.forward);
        if l.abs() > self.half_len || r.dot(self.lateral).abs() > self.half_wid {
            return 0.0;
        }
        let h = r.dot(self.up);
        if !(-2.0..=self.ceil).contains(&h) {
            return 0.0;
        }
        (self.w_back + (self.w_front - self.w_back) * ((l + self.half_nom) / (2.0 * self.half_nom))).clamp(0.0, 1.0)
    }
}

/// ENHANCEMENT E9 (SHADOW-REAL): how covered `p` is by the track's covered boxes, 0..1 (the highest of the boxes'; the shader's
/// `cover_at`, over every box rather than the nearest 64).
pub fn cover_weight(boxes: &[CoverBox], p: Vec3) -> f32 {
    let mut w = 0.0f32;
    for b in boxes {
        w = w.max(b.weight(p));
        if w >= 1.0 {
            break;
        }
    }
    w
}

/// ENHANCEMENT E26: what the wet road reflects and the lights it shows streaks of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RainEnv {
    /// The sky straight up (the sky pass's clear colour) and at the horizon.
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    /// The wetness at the camera (0..1), drives the splashes' density; the road parts carry their own ([`DrawItem::wet`]).
    pub camera_wet: f32,
    /// Seconds on the race clock (the splash ripples' phase; frozen while the race is paused).
    pub time: f32,
    /// Lights near the camera that reflect in the road as vertical streaks (at most [`RAIN_LAMPS`]): position and
    /// linear colour x intensity.
    pub lamps: [(Vec3, [f32; 3]); RAIN_LAMPS],
    /// How many of `lamps` are used.
    pub n_lamps: usize,
    /// The nearest wetness edges (where the original's field 5 steps between slices), at most [`RAIN_EDGES`]: the road
    /// parts carry one value each, so near an edge the shader takes the wetness from these instead, ramped over the edge's width.
    pub edges: [RainEdge; RAIN_EDGES],
    pub n_edges: usize,
}

/// ENHANCEMENT E26: the most lights the road reflects per frame.
pub const RAIN_LAMPS: usize = 12;

/// ENHANCEMENT E26: the most wetness edges the shader knows per frame.
pub const RAIN_EDGES: usize = 8;

/// ENHANCEMENT E26: a step of the wetness along the track: the plane through `pos` with the normal `dir` (toward the
/// higher slices), the wetness before it (`before`) and after it (`after`), ramped over `half` metres each side.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RainEdge {
    pub pos: Vec3,
    pub dir: Vec3,
    pub before: f32,
    pub after: f32,
    pub half: f32,
}

/// ENHANCEMENT E23: one street lamp as a point light.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct StreetLamp {
    pub pos: Vec3,
    /// the lamp's colour (the flare's), brightness folded in; may be 0 (faded out)
    pub color: [f32; 3],
    /// the reach of this lamp, metres
    pub radius: f32,
}

/// The most street lamps a frame lights.
pub const MAX_LAMPS: usize = 32;

/// ENHANCEMENT E23: the street lamps of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamps {
    pub n: u8,
    /// the strength of the added light
    pub gain: f32,
    pub lamps: [StreetLamp; MAX_LAMPS],
}

impl Default for Lamps {
    fn default() -> Self {
        Self { n: 0, gain: 1.0, lamps: [StreetLamp::default(); MAX_LAMPS] }
    }
}

/// ENHANCEMENT E43: the most cars a frame's contact shadow knows.
pub const MAX_CONTACTS: usize = 16;

/// ENHANCEMENT E43: one car for the contact shadow. The pose is the one the original's blob quad (`shadow_quad`, 0x43B820) is
/// laid from: the body box `half` (LOD 5's, `car+0x3A8` / `+0x3B0`) round the model origin, which stands on the ground
/// (`origin`), in the body's axes (right, up, forward).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ContactCar {
    /// the model origin on the road under the car (the box's bottom centre), world
    pub origin: Vec3,
    /// the body's right, up and forward axes in the world (unit)
    pub axes: [Vec3; 3],
    /// the box's half width (x) and half length (z), metres
    pub half: [f32; 2],
    /// the tyres' contact patch centres in the body's (x, z), metres: FL, FR, RL, RR
    pub wheels: [[f32; 2]; 4],
    /// the tyre radius, metres
    pub wheel_r: f32,
}

/// ENHANCEMENT E43: the cars of a frame (the nearest to the camera first, at most [`MAX_CONTACTS`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contacts {
    pub n: u8,
    pub cars: [ContactCar; MAX_CONTACTS],
}

impl Default for Contacts {
    fn default() -> Self {
        Self { n: 0, cars: [ContactCar::default(); MAX_CONTACTS] }
    }
}

impl Contacts {
    /// Append a car (ignored past [`MAX_CONTACTS`]).
    pub fn push(&mut self, c: ContactCar) {
        if (self.n as usize) < MAX_CONTACTS {
            self.cars[self.n as usize] = c;
            self.n += 1;
        }
    }
}

/// ENHANCEMENT E15: one lit headlight as a spotlight.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SpotLamp {
    /// the lamp's centre in the world
    pub pos: Vec3,
    /// the way it shines (unit)
    pub dir: Vec3,
    /// 0..1: the lamp slot's ramp (`car+0x2D8`) times how dark the place is
    pub intensity: f32,
}

/// The most spotlights a frame lights.
pub const MAX_SPOTS: usize = 8;

/// ENHANCEMENT E15: the spotlights of a frame. The first `shadowed` lamps are the camera's own car's
/// pair, which share one shadow map (the rest are lights without shadows).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Spots {
    pub n: u8,
    pub shadowed: u8,
    pub lamps: [SpotLamp; MAX_SPOTS],
}

impl Spots {
    pub fn push(&mut self, l: SpotLamp) -> bool {
        if (self.n as usize) < MAX_SPOTS {
            self.lamps[self.n as usize] = l;
            self.n += 1;
            true
        } else {
            false
        }
    }
}

/// ENHANCEMENT E9: the sun of a race scene's shadows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SunShadow {
    /// The direction the light travels (from the sun to the ground) — the one the cars' `R3DLightLayer`
    /// is given (`-SkyBin::sun_position`, 0x43AC8D), so the shadows agree with the car shading.
    pub direction: Vec3,
    /// ENHANCEMENT E9 (SHADOWFIX): false = the frame-wide cover gate shuts (the renderer fades the shadows out over half a
    /// second and back in). SHADOW-REAL: the race keeps it true whenever `Scene::cover` holds the track's covered boxes, so the
    /// shadow ends at a tunnel's mouth per surface (the shader's `cover_at`); the gate is the fallback of a track without them
    /// (false while the camera or the player's car is under cover).
    pub lit: bool,
    /// ENHANCEMENT ENH-FE3D: the front end's 3-D views fit ONE cascade to a sphere (centre, radius) round the car
    /// instead of the race's view-depth cascades, and soften the edge (a jittered disc of that many texels); `None` =
    /// the race's.
    pub focus: Option<ShadowFocus>,
}

/// ENHANCEMENT ENH-FE3D: a front-end car's shadow volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowFocus {
    pub centre: Vec3,
    pub radius: f32,
    /// the soft edge's radius in metres
    pub blur: f32,
    /// how strongly the shadow darkens a receiver at full shadow, 0..1
    pub strength: f32,
    /// ENHANCEMENT ENH-GARAGE: the contact core's radius in metres (a second, sharper disc; 0 = none): the shadow is
    /// the darker of the soft penumbra (weighted by `soft`) and the core, so the car sits in a tight dark footprint
    pub core: f32,
    /// the penumbra's share of the full strength, 0..1 (the core is always whole)
    pub soft: f32,
    /// how much of the original's `FESH` blob under the garage car gives way to this shadow, 0..1
    pub blob_fade: f32,
}

impl SunShadow {
    /// The race's sun / overcast light (the view-depth cascades).
    pub const fn race(direction: Vec3) -> Self {
        Self { direction, lit: true, focus: None }
    }
}

/// ENHANCEMENT E17: what the water reflects when there is no environment map (E8 off), and the sun
/// that glints on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterEnv {
    /// The sky straight up (the sky pass's clear colour) and at the horizon (the scene's fog colour, else the same).
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    /// The unit vector from the ground to the sun (`SkyBin::sun_position`, the sun of E9 and the lens
    /// flare); zero = no sun (night, overcast tracks): no glint.
    pub sun: Vec3,
    /// Reflect the E8 environment map (taken at the car, so only right where the car is close to the water: the
    /// night harbour) instead of the sky colours alone (the sea seen from a road far above it).
    pub use_env_map: bool,
}

/// The sun's lens flare reads a 4 × 4 block of the frame at the sun's screen point
/// (`THRASH_readrect`, 0x460525 and 0x4603FF; `RE/notes/lighting.md` §7): once right after the sky
/// pass has drawn the sun (the reference texels, `+0x148`), then every frame (200 ms in the file,
/// `nfs5.dll` NOPs the throttle) from the finished 3-D pass (the test). The renderer copies the block into a staging buffer and hands the pixels
/// back a frame or two later ([`gpu::Renderer::lens_results`]) — **a latency the original does
/// not have** (its read is a blocking lock of the back buffer), the port's own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensProbe {
    /// the sun's point on the 640 × 480 frame (`+0x12C`, `+0x130`); the renderer takes the block's
    /// top-left pixel as `round(x · W / 640) − 2`, `round(y · H / 480) − 2` of the target it draws
    /// into — the window's surface, or a still's own image, which can differ in size (a Retina
    /// window is twice a 1280 × 800 still)
    pub x: f32,
    pub y: f32,
    /// read the block after the no-depth (sky) stage alone — the reference
    pub reference: bool,
    /// read the block from the finished 3-D pass, before the overlay items — the test
    pub test: bool,
}

/// One finished lens-flare read-back: the block's sixteen pixels as the target stores them
/// (row-major, 4 bytes each, little-endian), and what collecting it cost the CPU, in whole
/// milliseconds (the original times its blocking read with `GetTickCount`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LensRead {
    pub reference: bool,
    pub pixels: [u32; 16],
    pub cost_ms: u32,
}

/// Direct3D 7 table fog as `dx7z.dll` sets it for Thrash 0x15 = 4 (`FOGTABLEMODE` `EXP`, 0x600048B7):
/// `f = exp(−density · d)` and the pixel `f · colour + (1 − f) · fog` before the blend. The driver
/// turns table fog on only on a card with `D3DPRASTERCAPS_WFOG` (`[0x60033F6C]`, bit 0x100000 of the
/// triangle raster caps, 0x60008E4C) — without it the state fails and nothing is fogged — so `d` is
/// the **W** of the game's pre-transformed vertices, `1 / rhw` = the view depth in metres
/// (0x5B12AE…, `rhw = 1 / z`). approximation (CLOSED on the running original, 2026-10-01 SV, below) of a device behaviour the image cannot settle: whether
/// the runtime fogs on that W or on the screen z `sz` (the game never hands the device a projection
/// matrix — no Thrash 0x1A7 in the image). On `sz`, which runs 0…1 over the view, the shipped
/// densities (at most 0.28³ ≈ 0.02) would fog nothing visibly; on W they are the tracks' haze.
/// **Observed on the running original (2026-10-01 SV, Wine d3d7 under `Drivers/dx7z.dll`, gdb on
/// `_THRASH_setstate` 0x60003AC0, alps with `skyrendering = 2`):** the device reports WFOG
/// (`[0x60033F6C]` = 1), the game sets 0x15 ← 4, 0x0E ← 0.003171 and 0.001409 (the cubed weather
/// densities), 0x0F ← 0x8A6656, 0x15 ← 1, and the countdown frame shows the blue-grey haze over the far
/// hills that density gives on W (`exp(−0.0032 · 100 m)` = 0.73); on the screen z the same density
/// would be 0.997 — invisible. So the runtime fogs on the W, as the port does. Still Wine's
/// d3d7 (`fog_coord` / W), not a 2000-era card's driver: the row is closed on that reading.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub colour: [f32; 3],
    pub density: f32,
}

impl Scene {
    /// A copy with the same items and view state but no sprites — what an offscreen 3-D pass
    /// (`Renderer::render_offscreen`, which draws none) needs, without cloning the 2-D list.
    /// Copy this scene into `dst`, keeping `dst`'s allocations (the render thread's hand-off).
    pub fn copy_into(&self, dst: &mut Scene) {
        let (mut items, mut sprites) = (std::mem::take(&mut dst.items), std::mem::take(&mut dst.sprites));
        items.clear();
        items.extend_from_slice(&self.items);
        sprites.clear();
        sprites.extend_from_slice(&self.sprites);
        *dst = Self { items, sprites, cover: self.cover.clone(), ..*self };
    }

    pub fn clone_without_sprites(&self) -> Self {
        Self { items: self.items.clone(), sprites: Vec::new(), cover: self.cover.clone(), ..*self }
    }

    /// This scene's view state with `items` as its items and no sprites: [`Self::clone_without_sprites`]
    /// without copying the items, for a caller that lays the offscreen pass's items out itself.
    pub fn clone_header(&self, items: Vec<DrawItem>) -> Self {
        Self { items, sprites: Vec::new(), cover: self.cover.clone(), ..*self }
    }
}

impl Default for Scene {
    fn default() -> Self {
        Self { items: Vec::new(), sprites: Vec::new(), clear_color: [0.35, 0.55, 0.85, 1.0], sun_direction: Vec3::new(0.4, 1.0, 0.3).normalize(), ambient: 0.35, mirror: false, headlight: None, headlight_decal: false, headlight_gain: 1.0, beacon: None, fog: None, sky_fog: None, lens_probe: None, refl: 0.0, sun_shadow: None, overcast_shadow: None, water: None, spots: Spots::default(), lamps: Lamps::default(), shafts: None, ao: false, flash: 0.0, eye: false, motion: None, rain: None, dof: None, haze: None, drops: None, contacts: Contacts::default(), cover: Default::default(), lbeam: None }
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
