//! wgpu (Metal on macOS) renderer with an engine-level API.
//!
//! Nothing here knows about CRP/SHPI files; `nfs-assets` converts original data into
//! [`MeshData`] / [`TextureData`] and this crate uploads and draws them.

mod camera;
mod front;
mod gpu;

pub use camera::Camera;
pub use front::Renderer;
pub use gpu::{MaterialId, MeshId, TextureId, VIEW_FAR};

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
    /// own, `[[[0x69ED0C]]+4]+0x30`, which stays 0): `dx7z.dll` adds `b / 32768` to the vertex's
    /// linear screen z `(z − near) / (far − near)`, so a vertex moves `−b · (far − near) / 32768`
    /// of view depth toward the eye along its own ray (`RE/notes/thrash.md` §6d.3).
    pub depth_bias: i32,
    /// `RCustomLayers::LayerFalseSort1` / `2` (0x449AA0 / 0x449AF0, the `FakeSort` method's ops
    /// 0x800F / 0x8010): at `SPECIALFX` 2 or 3 the material is drawn twice, first with depth
    /// writes off (Thrash 0x39 ← 0), then on (← 1) — blended both times. Below 2 (`Renderer::
    /// set_fakesort_mode`), 0x44EC80's other arm draws it once, `MAG`/`MINFILTER` forced
    /// `POINT` for that draw (`RE/notes/thrash.md` §6d.4). Which of the two happens at `SPECIALFX`
    /// 2 is a runtime pick, [`FakeSortMode`].
    pub false_sort: bool,
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
}

impl DrawItem {
    pub fn new(mesh: MeshId, material: MaterialId, transform: Mat4) -> Self {
        Self { mesh, material, transform, light: [1.0; 3], alpha: 1.0, uv_offset: [0.0; 2], layer_light: None, headlit: false, uv_lit: false, shadow: None, early: false, depth_bias: 0, alpha_test: false, alpha_ref: 10, sky: None, main_view_only: false, sky_pass: false, fog_w: 0.0, fakesort_near: false }
    }
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
        *dst = Self { items, sprites, ..*self };
    }

    pub fn clone_without_sprites(&self) -> Self {
        Self { items: self.items.clone(), sprites: Vec::new(), ..*self }
    }

    /// This scene's view state with `items` as its items and no sprites: [`Self::clone_without_sprites`]
    /// without copying the items, for a caller that lays the offscreen pass's items out itself.
    pub fn clone_header(&self, items: Vec<DrawItem>) -> Self {
        Self { items, sprites: Vec::new(), ..*self }
    }
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
