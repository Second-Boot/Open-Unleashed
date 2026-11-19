//! The track's lights and emitters from its data: projectors, flares, particle emitters and the `.map` light sections.
//! Moved out of `world.rs` as it was.

use super::*;

pub use nfs_assets::track_lights::{track_projectors, Projector};

/// One flare a track article carries: its `ef` part's data from `+8`, taken through the article's
/// transform.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackFlare {
    /// the `.crp` index of the article carrying it, and the part's own index (the instance drawer
    /// runs only the parts of its LOD's range)
    pub article: usize,
    pub part: u16,
    /// `+0…+8`
    pub position: Vec3,
    /// `+0x10…+0x18` — what the draw turns towards the camera for its angle (0x451A0C…0x451A90)
    pub direction: Vec3,
    /// `+0x1C` — the `flare.dat` id
    pub id: u32,
    /// `+0x20` — the part's own blink phase (flag 0x10)
    pub phase: u32,
}

/// The kind-3 `ef` parts of every placed article, their data from `+8` through the article's
/// transform.
pub fn track_emitters(track: &TrackAssets) -> Vec<crate::track_particles::Emitter> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for (article, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x34 || bytes::read_u32(d, 0) != 3 {
                continue;
            }
            let f = |o: usize| bytes::read_f32(d, o);
            // the point through `tr` is where the running system emits (0x450980 → slot 4
            // 0x4550B0 with the placement's point, 0x450B0D…0x450B22); the prime's point and the
            // two directions are the part's own — 0x455030 copies the part's 48 bytes and
            // re-seats only the first 16 (0x455069, 0x4550B0), and the static creator hands the
            // constructor the part itself. On `tr`-less articles the two are the same numbers.
            let local = Vec3::new(f(8), f(0xC), f(0x10));
            out.push(crate::track_particles::Emitter {
                article,
                part: p.part_index(),
                position: tr.transform_point3(local),
                local,
                dir_a: Vec3::new(f(0x18), f(0x1C), f(0x20)),
                dir_b: Vec3::new(f(0x24), f(0x28), f(0x2C)),
                tag: bytes::read_u32(d, 0x30),
            });
        }
    }
    out
}

/// The kind-2 `ef` parts of every placed article (library articles are drawn only through
/// scenes — [`TrackWorld::running_flares`]; 0x450D10 runs for `RDrawArticleBundle`, the placed ones).
pub fn track_flares(track: &TrackAssets) -> Vec<TrackFlare> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for (article, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        out.extend(article_flares(track, article, a, tr));
    }
    out
}

/// The kind-2 `ef` parts of one article, their data from `+8` through `tr`.
pub(super) fn article_flares(track: &TrackAssets, article: usize, a: &nfs_formats::crp::Article, tr: Mat4) -> Vec<TrackFlare> {
    let mut out = Vec::new();
    for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
        let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
        if d.len() < 0x2C || bytes::read_u32(d, 0) != 2 {
            continue;
        }
        let f = |o: usize| bytes::read_f32(d, o);
        let w = |o: usize| bytes::read_u32(d, o);
        out.push(TrackFlare {
            article,
            part: p.part_index(),
            position: tr.transform_point3(Vec3::new(f(8), f(0xC), f(0x10))),
            direction: tr.transform_vector3(Vec3::new(f(0x18), f(0x1C), f(0x20))),
            id: w(0x24),
            phase: w(0x28),
        });
    }
    out
}

/// One `[sectionN]` of a track's `.map`: the slice range it covers and the six environment-map ids
/// a car's `R3DSkyMapTextureLayer` takes inside it, open and covered (`RE/notes/lighting.md` §3.2 —
/// *corrected 2026-09-26*: these are the **sky-map** pass's textures, `[0x6115CC]+0xD0…+0xD8`; the
/// specular layer reads `+0xDC…+0xE4`, `carcmn.fsh`'s `SPC1…SPC3`).
///
/// The record is 0x48 bytes in the game (`race-logic.md` §6.4); `EnvSet::select(slice)` 0x441140 is
/// a linear search for the first section with `startslice ≤ slice ≤ endslice` and leaves the
/// current section **null** when none matches, and `EnvSet::texture(covered, layer)` 0x441180 reads
/// `ids[layer + 3·covered]`. The shipped sections mark exactly the covered stretches — monaco1's
/// (141, 244) is its tunnel — so a car's sky-map pass draws inside them and **nothing** anywhere else:
/// with no section `EnvSet::texture` answers 0 (0x4411AD), and the layer skips its pass's draw when
/// it has no texture (0x445B86…0x445BB1).
#[derive(Debug, Clone, Copy)]
pub struct MapSection {
    pub start: u32,
    pub end: u32,
    /// `[texture1.open, texture2.open, texture3.open, texture1.covered, …]` as `.env` SHPI names
    /// packed little-endian into a u32
    pub ids: [u32; 6],
    pub texture_size: f32,
    pub aspect: f32,
}

impl MapSection {
    /// All the sections of a `.map`, in file order.
    pub fn all(map: &nfs_formats::ini::Ini) -> Vec<Self> {
        let count = map.section("header").and_then(|h| h.get_i64("count")).unwrap_or(0);
        (0..count)
            .filter_map(|i| {
                let s = map.section(&format!("section{i}"))?;
                let id = |k: &str| s.get_i64(k).unwrap_or(0) as u32;
                Some(Self {
                    start: id("startslice"),
                    end: id("endslice"),
                    ids: [
                        id("texture1.open"),
                        id("texture2.open"),
                        id("texture3.open"),
                        id("texture1.covered"),
                        id("texture2.covered"),
                        id("texture3.covered"),
                    ],
                    texture_size: s.get_f32("texturesize").unwrap_or(6.0),
                    aspect: s.get_f32("aspect").unwrap_or(1.0),
                })
            })
            .collect()
    }
}

/// The five colours in a track's `.map` `[header]` (0x4409D0 parses them into the environment-map
/// object at `+0x00`, `+0x10`, `+0x20`, `+0x30`, `+0x40`). Each key's lookup is preceded by the
/// default it falls back to (0x440B1C…, 0x440B9F…, 0x440C41…, 0x440CCD…, 0x440D5C…): 1 for `tint`,
/// `interior`, `interiordiffuse` and `lightdecal`, 0.3 for `ambient` only (`RE/notes/lighting.md` §6a —
/// every shipped file lacks `interiordiffuse`, so the cockpit's directional slot is `g · 1`, which the
/// running original shows).
///
/// They are the multipliers of the **car** light block: 0x43A8A6 samples the colour of the ground
/// patch under the car (0x4746A0 → 0x474700, a blend of the patch's vertex colours, each a 5-bit-per-channel word)
/// and multiplies one slot of the car's light block by each of them (`RE/notes/lighting.md`).
/// Every shipped file has r = g = b, so each is really a scalar; `tint` runs 0.25…1.80 and
/// `ambient` 0.29…1.23.
#[derive(Debug, Clone, Copy)]
pub struct MapLight {
    pub tint: [f32; 3],
    pub ambient: [f32; 3],
    pub interior: [f32; 3],
    pub interior_diffuse: [f32; 3],
    pub light_decal: [f32; 3],
}

impl Default for MapLight {
    fn default() -> Self {
        Self { tint: [1.0; 3], ambient: [0.3; 3], interior: [1.0; 3], interior_diffuse: [1.0; 3], light_decal: [1.0; 3] }
    }
}

impl MapLight {
    /// Read the header. The files write each colour as **four** floats, alpha first.
    pub fn from_map(map: &nfs_formats::ini::Ini) -> Self {
        let mut out = Self::default();
        let Some(h) = map.section("header") else { return out };
        let get = |key: &str| -> Option<[f32; 3]> {
            let v = h.get_f32_list(key)?;
            (v.len() == 4).then(|| [v[1], v[2], v[3]])
        };
        if let Some(v) = get("tint") {
            out.tint = v;
        }
        for (dst, key) in [
            (&mut out.ambient, "ambient"),
            (&mut out.interior, "interior"),
            (&mut out.interior_diffuse, "interiordiffuse"),
            (&mut out.light_decal, "lightdecal"),
        ] {
            if let Some(v) = get(key) {
                *dst = v;
            }
        }
        out
    }
}
