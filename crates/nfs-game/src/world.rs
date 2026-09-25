//! Assembling renderer scenes from loaded assets (shared by the game app and the viewer tools).

use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, track::TrackAssets};
use nfs_renderer::{DrawItem, LayerLight, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};
use std::collections::HashMap;

/// GPU-resident track: static draw items plus the collision grid.
pub struct TrackWorld {
    /// Per item: the mission-scene element it belongs to (None for track geometry).
    pub item_element: Vec<Option<usize>>,
    /// Mission elements currently hidden (arrow markers not yet reached).
    pub hidden: std::collections::HashSet<usize>,
    pub items: Vec<DrawItem>,
    /// environment map for car reflections (`.env` "SKY " entry, else the first)
    pub env_map: Option<nfs_renderer::TextureId>,
    pub collision: nfs_physics::collision::TriangleGrid,
    /// The collision props the loader 0x4750B0 builds from the track's tagged articles
    /// ([`track_props`]) — `Car_propPass` 0x498DE0's objects.
    pub props: Vec<nfs_physics::props::Prop>,
    pub walls: nfs_physics::walls::WallSet,
    pub start: Vec3,
    pub ambient: f32,
    /// The `.map` header's five light colours (`RE/notes/lighting.md`).
    pub map_light: MapLight,
    /// The way the sun's light travels, `−normalize(SkyBin::sun_position)` (0x43AC8D…0x43AD36);
    /// straight down when the track has no sky configuration.
    pub sun_direction: Vec3,
    /// Smackable props knocked loose and still moving.
    knocked: Vec<KnockedProp>,
    /// `.map` sections, and every `.env` entry by its SHPI name (`RE/notes/lighting.md` §3).
    pub map_sections: Vec<MapSection>,
    env_by_id: HashMap<u32, nfs_renderer::TextureId>,
    /// Props whose article carries an `Anim` / `Anqt` track and whose `animdefs.txt` definition
    /// loops it (`RE/notes/crp-geometry.md` §7). The shipped file loops exactly two — the two
    /// dockers — every other animation there is played on impact, which needs the smackable bodies
    /// the port does not simulate.
    animated: Vec<AnimatedProp>,
    /// The breakable props drawn in the world, with their pieces once a car has started them.
    broken: Vec<BrokenProp>,
    /// The kind-2 `ef` parts of the track's own articles — the flares 0x451940 draws
    /// (`RE/notes/crp-geometry.md` §8).
    pub flares: Vec<TrackFlare>,
    /// The kind-3 `ef` parts — the particle emitters 0x455150 starts (`particles.md` §6).
    pub emitters: Vec<crate::track_particles::Emitter>,
    /// The kind-4 `ef` parts — the car shadow projectors 0x456090 registers (`crp-geometry.md` §8)
    /// — and the one texture every shipped one names (`sh80`), uploaded.
    pub projectors: Vec<Projector>,
    pub projector_texture: Option<nfs_renderer::TextureId>,
}

/// One kind-4 `ef` part: its sixteen floats from `+8`.
#[derive(Debug, Clone, PartialEq)]
pub struct Projector {
    /// `[0…2]`, `[3]`
    pub point: Vec3,
    pub radius: f32,
    /// `[6…8]`, `[10]` and `[11…13]`, `[15]`: `u = u.xyz · world + u.w`, likewise `v` (0x446130)
    pub u: [f32; 4],
    pub v: [f32; 4],
    /// `+0x1C` of the part, the texture's name (0x44B000)
    pub texture: String,
}

impl TrackWorld {
    /// 0x456160: the projector nearest to `pos` by `max(|dx|, |dz|) + 0.25 · min(|dx|, |dz|)`,
    /// below the running best from **400** and below its radius + 2.5.
    pub fn projector_for(&self, pos: Vec3) -> Option<&Projector> {
        let mut best = 400.0f32;
        let mut found = None;
        for p in &self.projectors {
            let (dx, dz) = ((pos.x - p.point.x).abs(), (pos.z - p.point.z).abs());
            let d = dx.max(dz) + 0.25 * dx.min(dz);
            if d < best && d < p.radius + 2.5 {
                best = d;
                found = Some(p);
            }
        }
        found
    }
}

/// The kind-4 `ef` parts of every placed article. The planes are taken as the file has them: the
/// `Shadow` articles that carry them have no `tr` on any shipped track (canyon 59, farmland 18,
/// castle 13), so they are in world space already; the point still goes through the transform.
pub fn track_projectors(track: &TrackAssets) -> Vec<Projector> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for a in &arts {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x48 || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != 4 {
                continue;
            }
            let f = |i: usize| {
                let o = 8 + 4 * i;
                f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
            };
            out.push(Projector {
                point: tr.transform_point3(Vec3::new(f(0), f(1), f(2))),
                radius: f(3),
                u: [f(6), f(7), f(8), f(10)],
                v: [f(11), f(12), f(13), f(15)],
                texture: d[0x1C..0x20].iter().map(|&c| c as char).collect(),
            });
        }
    }
    out
}

/// One flare a track article carries: its `ef` part's data from `+8`, taken through the article's
/// transform.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackFlare {
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
    for a in &arts {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x34 || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != 3 {
                continue;
            }
            let f = |o: usize| f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
            out.push(crate::track_particles::Emitter {
                position: tr.transform_point3(Vec3::new(f(8), f(0xC), f(0x10))),
                dir_a: tr.transform_vector3(Vec3::new(f(0x18), f(0x1C), f(0x20))),
                dir_b: tr.transform_vector3(Vec3::new(f(0x24), f(0x28), f(0x2C))),
                tag: u32::from_le_bytes([d[0x30], d[0x31], d[0x32], d[0x33]]),
            });
        }
    }
    out
}

/// The kind-2 `ef` parts of every placed article (library articles are drawn only through
/// scenes; 0x450D10 runs for `RDrawArticleBundle`, the placed ones).
pub fn track_flares(track: &TrackAssets) -> Vec<TrackFlare> {
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    for a in &arts {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        if g.base.as_ref().is_some_and(|b| b.is_library()) {
            continue;
        }
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        for p in a.parts.iter().filter(|p| p.is_part(b"ef")) {
            let Ok(d) = track.crp.entry_data(&track.crp_data, p) else { continue };
            if d.len() < 0x2C || u32::from_le_bytes([d[0], d[1], d[2], d[3]]) != 2 {
                continue;
            }
            let f = |o: usize| f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
            let w = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
            out.push(TrackFlare {
                position: tr.transform_point3(Vec3::new(f(8), f(0xC), f(0x10))),
                direction: tr.transform_vector3(Vec3::new(f(0x18), f(0x1C), f(0x20))),
                id: w(0x24),
                phase: w(0x28),
            });
        }
    }
    out
}

/// One `[sectionN]` of a track's `.map`: the slice range it covers and the six environment-map ids
/// the car's three reflection layers take inside it, open and covered.
///
/// The record is 0x48 bytes in the game (`race-logic.md` §6.4); `EnvSet::select(slice)` 0x441140 is
/// a linear search for the first section with `startslice ≤ slice ≤ endslice` and leaves the
/// current section **null** when none matches, and `EnvSet::texture(covered, layer)` 0x441180 reads
/// `ids[layer + 3·covered]`. The shipped sections mark exactly the covered stretches — monaco1's
/// (141, 244) is its tunnel — so a car reflects the section's map inside them and the default
/// everywhere else.
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
/// object at `+0x00`, `+0x10`, `+0x20`, `+0x30`, `+0x40`, defaulting to 1 or 0.3).
///
/// They are the multipliers of the **car** light block: 0x43A8A6 samples the colour of the ground
/// patch under the car (0x4746A0 → 0x474700, a bilinear blend of the patch's four vertex colours)
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
        Self { tint: [1.0; 3], ambient: [0.3; 3], interior: [0.3; 3], interior_diffuse: [0.3; 3], light_decal: [0.3; 3] }
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

impl TrackWorld {
    /// The environment map a car reflects at `slice`: the `.map` section covering it, layer 0, in
    /// its covered or open variant (`RE/notes/lighting.md` §3). `None` outside every section, where
    /// the original leaves the car on the default maps.
    pub fn env_for(&self, slice: u32, covered: bool) -> Option<nfs_renderer::TextureId> {
        self.env_layers_for(slice, covered)[0]
    }

    /// All three of the section's environment textures — `EnvSet::texture` 0x441180 answers
    /// `ids[layer + 3·covered]` for the layers 0, 1 and 2, and the frame binds all three
    /// (0x43A73D…0x43A778, `RE/notes/lighting.md` §3.1).
    pub fn env_layers_for(&self, slice: u32, covered: bool) -> [Option<nfs_renderer::TextureId>; 3] {
        let Some(sec) = self.map_sections.iter().find(|s| slice >= s.start && slice <= s.end) else {
            return [None; 3];
        };
        std::array::from_fn(|l| self.env_by_id.get(&sec.ids[l + 3 * covered as usize]).copied())
    }

    /// The original's enclosure test (0x473330, `race-logic.md` §12): the flags word of the lowest
    /// track patch more than **3.5 m** above the point, or 0 when there is none. `Tracker` patches
    /// carry the CRP `mt` flags word at `patch+0xA` (`suspension-collision.md` §7), which is the
    /// same word the port keeps in the low 16 bits of a collision triangle's surface id.
    pub fn overhead_flags(&self, pos: Vec3) -> u16 {
        self.collision.ceiling(pos.x, pos.y, pos.z, 3.5, 200.0).map(|h| (h.surface & 0xFFFF) as u16).unwrap_or(0)
    }

    /// The race car's light block, 0x43A8A6…0x43AD36 (`RE/notes/lighting.md` §6): the ground
    /// patch's colour `g` under `pos` (0x4746A0) times the `.map` header colours, slot by slot —
    /// 0 `g·ambient`, 1 `g·ambient·interior`, 2 `g·ambient·lightdecal`, 4 `g·tint`, 5
    /// `g·interiordiffuse`, 6 `g·tint·lightdecal` — `+0x74` white (0x43A5AC), and the direction
    /// straight down under cover (0x473330's flags `& 0xC000`, 0x43A6E8 / 0x43AB19) or else along
    /// the negated sun of the track's sky configuration (0x43AC8D, `SkyBin::sun_position`).
    pub fn car_light(&self, pos: Vec3) -> LightBlock {
        let g = self.collision.ground(pos.x, pos.y + 2.0, pos.z, 20.0).map(|h| [h.color[0], h.color[1], h.color[2]]).unwrap_or([1.0; 3]);
        let m = &self.map_light;
        let mul = |a: [f32; 3], b: [f32; 3]| [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
        let amb = mul(g, m.ambient);
        let tint = mul(g, m.tint);
        let covered = self.overhead_flags(pos) & 0xC000 != 0;
        Self::car_light_block(amb, tint, g, m, covered, self.sun_direction)
    }

    fn car_light_block(amb: [f32; 3], tint: [f32; 3], g: [f32; 3], m: &MapLight, covered: bool, sun: Vec3) -> LightBlock {
        let mul = |a: [f32; 3], b: [f32; 3]| [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
        LightBlock {
            colours: [amb, mul(amb, m.interior), mul(amb, m.light_decal), [0.0; 3], tint, mul(g, m.interior_diffuse), mul(tint, m.light_decal)],
            direction: if covered { Vec3::NEG_Y } else { sun },
            material: [1.0; 3],
        }
    }

    /// Whether the car is under a tunnel or bridge deck — bit 15 of the patch overhead
    /// (`engine-audio.md` §4.7). Bits 14/15 mark the **drivable road** materials
    /// (`suspension-collision.md` §7), so a rock overhang or a roof does not count; road above you
    /// does.
    pub fn in_tunnel(&self, pos: Vec3) -> bool {
        self.overhead_flags(pos) & 0x8000 != 0
    }

    pub fn upload(r: &mut Renderer, track: &TrackAssets) -> anyhow::Result<Self> {
        let geometry = track.build_geometry()?;
        let materials = track.materials()?;
        let mut tex_ids = HashMap::new();
        for tex in &track.textures {
            let id = r.upload_texture(&TextureData { width: tex.width, height: tex.height, mips: tex.mips.clone() });
            tex_ids.insert(tex.name.clone(), (id, tex.has_alpha));
        }
        let mut mat_ids: HashMap<u16, MaterialId> = HashMap::new();
        let mut material_for = |r: &mut Renderer, id: u16| {
            *mat_ids.entry(id).or_insert_with(|| {
                let m = materials.get(id as usize);
                let tex = m.and_then(|m| tex_ids.get(&m.texture_name()));
                let alpha = tex.map(|t| t.1).unwrap_or(false) && m.map(|m| m.render_method != "Opaque").unwrap_or(true);
                // the material's address modes, states 0x41 / 0x42 (`thrash.md` §7): 0 clamps
                let clamp = m.map_or([false; 2], |m| m.address.map(|a| a == 0));
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, blend_pair: 0, clamp, lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: false })
            })
        };
        // Static world: merge every article's meshes per material into one GPU mesh each
        // (transforms baked on the CPU) — 4k draw calls → ~200.
        let props = track_props(track);
        // a breakable's own article is drawn on its own so that its pieces can replace it
        let breakable_articles: std::collections::HashSet<usize> = props.iter().filter_map(|p| p.breakable.as_ref().map(|b| b.article)).collect();
        let mut merged: HashMap<u16, nfs_renderer::MeshData> = HashMap::new();
        for art in &geometry.world {
            if breakable_articles.contains(&art.index) {
                continue;
            }
            for mm in &art.meshes {
                let dst = merged.entry(mm.material).or_default();
                let base = dst.vertices.len() as u32;
                let nrm = art.transform.inverse().transpose();
                dst.vertices.extend(mm.mesh.vertices.iter().map(|v| {
                    let mut o = *v;
                    o.position = art.transform.transform_point3(Vec3::from(v.position)).to_array();
                    o.normal = nrm.transform_vector3(Vec3::from(v.normal)).normalize_or_zero().to_array();
                    o
                }));
                dst.indices.extend(mm.mesh.indices.iter().map(|i| i + base));
            }
        }
        let mut items = Vec::new();
        let mut mats: Vec<u16> = merged.keys().copied().collect();
        mats.sort_unstable();
        for m in mats {
            let material = material_for(r, m);
            items.push(DrawItem::new(r.upload_mesh(&merged[&m]), material, Mat4::IDENTITY));
        }
        let broken = Self::breakables(r, track, &geometry, &props, &mut items, &mut material_for);
        let mut lib_gpu: HashMap<u32, Vec<(MeshId, MaterialId)>> = HashMap::new();
        for art in &geometry.library {
            let entry = lib_gpu.entry(art.tag).or_default();
            if !entry.is_empty() {
                continue;
            }
            for mm in &art.meshes {
                let material = material_for(r, mm.material);
                entry.push((r.upload_mesh(&mm.mesh), material));
            }
        }
        let mut item_element = vec![None; items.len()];
        let mut missing_tags = std::collections::HashSet::new();
        // `ENDW` barrier boxes of a skill test (`animdefs.txt` collideType=kBox): their footprint
        // edges join the wall set so the course is closed for the physics too
        let mut extra_walls: Vec<([f32; 3], [f32; 3], u32)> = Vec::new();
        let mut animated: Vec<AnimatedProp> = Vec::new();
        for inst in &geometry.instances {
            if let Some(parts) = lib_gpu.get(&inst.tag) {
                let art = geometry.library.iter().find(|a| a.tag == inst.tag);
                let lib_tr = art.map(|a| a.transform).unwrap_or(Mat4::IDENTITY);
                let world_tr = inst.transform * Mat4::from_scale(inst.scale) * lib_tr;
                // a looping `animdefs.txt` piece makes this prop play its own track
                let loop_piece = track
                    .animdefs
                    .by_fourcc(inst.tag)
                    .and_then(|d| d.resting_pieces().find(|p| p.flags.looping).cloned());
                for (mesh, material) in parts {
                    items.push(DrawItem::new(*mesh, *material, world_tr));
                    item_element.push((inst.source == 1).then_some(inst.element));
                    if let (Some(p), Some(t)) = (loop_piece.as_ref(), art.and_then(|a| a.animation.as_ref())) {
                        animated.push(AnimatedProp {
                            item: items.len() - 1,
                            base: world_tr,
                            track: t.clone(),
                            seconds_per_frame: p.seconds_per_frame(),
                            looping: p.flags.looping,
                        });
                    }
                }
                if inst.source == 1 && inst.tag == u32::from_le_bytes(*b"ENDW") && let Some(a) = art {
                    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
                    for mm in &a.meshes {
                        for v in &mm.mesh.vertices {
                            let p = Vec3::from(v.position);
                            lo = lo.min(p);
                            hi = hi.max(p);
                        }
                    }
                    if lo.x <= hi.x {
                        let local = Mat4::from_scale(inst.scale) * lib_tr;
                        let corners = [Vec3::new(lo.x, lo.y, lo.z), Vec3::new(hi.x, lo.y, lo.z), Vec3::new(hi.x, lo.y, hi.z), Vec3::new(lo.x, lo.y, hi.z)]
                            .map(|c| inst.transform.transform_point3(local.transform_point3(c)));
                        for k in 0..4 {
                            extra_walls.push((corners[k].to_array(), corners[(k + 1) % 4].to_array(), 0x60));
                        }
                    }
                }
            } else if inst.source == 1 {
                missing_tags.insert(inst.tag);
            }
        }
        if !missing_tags.is_empty() {
            tracing::warn!(tags = ?missing_tags.iter().map(|t| String::from_utf8_lossy(&t.to_le_bytes()).into_owned()).collect::<Vec<_>>(), "mission props without a library article");
        }
        // every `.env` entry, keyed by its SHPI name as the `.map` sections spell it
        let mut env_by_id = HashMap::new();
        for t in &track.env_textures {
            let id = r.upload_texture(&TextureData { width: t.width, height: t.height, mips: t.mips.clone() });
            let mut name = [b' '; 4];
            for (i, c) in t.name.bytes().take(4).enumerate() {
                name[i] = c;
            }
            env_by_id.insert(u32::from_le_bytes(name), id);
        }
        let env_map = track
            .env_textures
            .iter()
            .position(|t| t.name.trim() == "SKY")
            .or(track.env_textures.first().map(|_| 0))
            .and_then(|i| {
                let mut name = [b' '; 4];
                for (k, c) in track.env_textures[i].name.bytes().take(4).enumerate() {
                    name[k] = c;
                }
                env_by_id.get(&u32::from_le_bytes(name)).copied()
            });
        let flags: Vec<u32> = materials.iter().map(|m| m.flags).collect();
        let collision = geometry.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
        let n_extra = extra_walls.len();
        let walls = nfs_physics::walls::WallSet::build(track.edges.iter().flat_map(|e| e.segments.iter().map(|s| (s.p1, s.p2, s.tag))).chain(extra_walls), 30.0);
        if n_extra > 0 {
            tracing::info!(segments = n_extra, "barrier boxes added to the wall set");
        }
        let start = track
            .start_forward
            .as_ref()
            .and_then(|s| s.of_kind(&nfs_formats::scn::ElementKind::Trigger).next())
            .map(|e| Vec3::from(e.position))
            .unwrap_or(Vec3::ZERO);
        let ambient = track
            .map
            .section("header")
            .and_then(|h| h.get_f32_list("ambient"))
            .filter(|a| a.len() == 4)
            .map(|a| ((a[1] + a[2] + a[3]) / 3.0).clamp(0.2, 0.9))
            .unwrap_or(0.35);
        tracing::info!(items = items.len(), triangles = collision.triangle_count(), walls = walls.len(), "track uploaded");
        if !animated.is_empty() {
            tracing::info!(props = animated.len(), "animated props");
        }
        let map_light = MapLight::from_map(&track.map);
        let sun_direction = track.sky_bin.as_ref().map(|b| -Vec3::from(b.sun_position()).normalize_or_zero()).filter(|d| *d != Vec3::ZERO).unwrap_or(Vec3::NEG_Y);
        let map_sections = MapSection::all(&track.map);
        // the items whose render method builds `Headlights::LayerVertexLight` (0x8002 / 0x8003)
        let lit_methods = nfs_assets::track::vertex_light_methods(&track.crp, &track.crp_data);
        let lit: std::collections::HashSet<MaterialId> = mat_ids
            .iter()
            .filter(|(id, _)| materials.get(**id as usize).is_some_and(|m| lit_methods.contains(&m.render_method)))
            .map(|(_, mid)| *mid)
            .collect();
        // the items whose render method builds `Headlights::LayerUVLight` (0x8004) — the `HEAD`
        // decal of `lighting.md` §5.2, two layers in every night track's `Opaque` method
        let uv_methods = nfs_assets::track::uv_light_methods(&track.crp, &track.crp_data);
        let uv: std::collections::HashSet<MaterialId> = mat_ids
            .iter()
            .filter(|(id, _)| materials.get(**id as usize).is_some_and(|m| uv_methods.contains(&m.render_method)))
            .map(|(_, mid)| *mid)
            .collect();
        for it in &mut items {
            it.headlit = lit.contains(&it.material);
            it.uv_lit = uv.contains(&it.material);
        }
        tracing::info!(props = props.len(), breakable = props.iter().filter(|p| p.breakable.is_some()).count(), "track props");
        let projectors = track_projectors(track);
        let projector_texture = projectors.first().and_then(|p| tex_ids.get(&p.texture)).map(|t| t.0);
        Ok(Self { items, item_element, hidden: std::collections::HashSet::new(), env_map, collision, props, walls, start, ambient, map_light, sun_direction, map_sections, env_by_id, knocked: Vec::new(), animated, broken, flares: track_flares(track), emitters: track_emitters(track), projectors, projector_texture })
    }

    /// Advance every looping prop animation to `seconds` since the race started
    /// (`AnimPlayer::sample` 0x470E90: the frame pair and the blend between them, then the frame's
    /// own rotation, offset and scale inside the placement the article was instanced with).
    pub fn animate(&mut self, seconds: f32) {
        for a in &self.animated {
            let (i, j, t) = a.track.sample(seconds, a.seconds_per_frame, a.looping);
            let (f0, f1) = (&a.track.frames[i], &a.track.frames[j]);
            let q0 = glam::Quat::from_xyzw(f0.rotation[0], f0.rotation[1], f0.rotation[2], f0.rotation[3]).normalize();
            let q1 = glam::Quat::from_xyzw(f1.rotation[0], f1.rotation[1], f1.rotation[2], f1.rotation[3]).normalize();
            let rot = q0.slerp(q1, t);
            let pos = Vec3::from(f0.position).lerp(Vec3::from(f1.position), t);
            let scale = Vec3::from(f0.scale).lerp(Vec3::from(f1.scale), t);
            let local = Mat4::from_scale_rotation_translation(if scale.min_element() > 1e-4 { scale } else { Vec3::ONE }, rot, pos);
            if let Some(item) = self.items.get_mut(a.item) {
                item.transform = a.base * local;
            }
        }
    }

    /// Draw items with the hidden mission elements filtered out.
    pub fn visible_items(&self) -> impl Iterator<Item = DrawItem> + '_ {
        self.items
            .iter()
            .enumerate()
            .zip(self.item_element.iter())
            .filter(|((k, _), e)| e.map(|e| !self.hidden.contains(&e)).unwrap_or(true) && !self.broken.iter().any(|b| b.hides(*k)))
            .map(|((_, i), _)| *i)
            .chain(self.broken.iter().flat_map(|b| b.pieces.iter().copied()))
    }

    /// The breakable props of the world (`BBOX`, the crate stacks): each world article whose prop
    /// has a `Breakable` is drawn on its own, and every piece of its definition gets its template
    /// — the `BaseGroup` article's meshes (0x478640 by tag, else the record's entry 0: the prop's own
    /// article for the first piece, the first piece's afterwards — 0x476471) and the track it plays:
    /// the `QuatGroup` article's `Anqt`, or the base article's when the `QuatGroup`'s `Anim` has
    /// vertex frames (its second word, 0x476525…0x476552).
    fn breakables(
        r: &mut Renderer,
        track: &TrackAssets,
        geometry: &nfs_assets::track::TrackGeometry,
        props: &[nfs_physics::props::Prop],
        items: &mut Vec<DrawItem>,
        material_for: &mut dyn FnMut(&mut Renderer, u16) -> MaterialId,
    ) -> Vec<BrokenProp> {
        let Ok(arts) = track.crp.articles(&track.crp_data) else { return Vec::new() };
        let tags = article_tags(track, &arts);
        let by_index: HashMap<usize, &nfs_assets::track::ArticleMeshes> = geometry.world.iter().chain(geometry.library.iter()).map(|a| (a.index, a)).collect();
        let mut uploaded: HashMap<usize, Vec<(MeshId, MaterialId)>> = HashMap::new();
        let mut upload = |r: &mut Renderer, k: usize| -> Vec<(MeshId, MaterialId)> {
            uploaded
                .entry(k)
                .or_insert_with(|| by_index.get(&k).map(|a| a.meshes.iter().map(|mm| (r.upload_mesh(&mm.mesh), material_for(r, mm.material))).collect()).unwrap_or_default())
                .clone()
        };
        let mut out = Vec::new();
        for (pi, p) in props.iter().enumerate() {
            let Some(b) = p.breakable.as_ref() else { continue };
            let Some(own) = geometry.world.iter().find(|a| a.index == b.article) else { continue };
            let Some(def) = track.animdefs.lookup(own.tag) else { continue };
            let first = items.len();
            for (mesh, material) in upload(r, own.index) {
                items.push(DrawItem::new(mesh, material, own.transform));
            }
            let mut entry0 = own.index;
            let mut templates = Vec::new();
            for (k, (_, piece)) in def.pieces.iter().enumerate() {
                let base = first_by_tag(&tags, group_tag(&piece.base_group)).unwrap_or(entry0);
                if k == 0 {
                    entry0 = base;
                }
                let quat = first_by_tag(&tags, group_tag(&piece.quat_group));
                let vertex_frames = quat.and_then(|q| tags[q].1).is_some_and(|w| w[1] > 0);
                let source = if vertex_frames { Some(base) } else { quat };
                let track_of = source.and_then(|k| by_index.get(&k)).and_then(|a| a.animation.clone());
                templates.push(PieceTemplate {
                    meshes: upload(r, base),
                    track: track_of,
                    seconds_per_frame: piece.seconds_per_frame(),
                    flags: piece.flags,
                });
            }
            out.push(BrokenProp {
                prop: pi,
                items: first..items.len(),
                instance: own.transform,
                velocity_factor: def.velocity_factor,
                templates,
                players: Vec::new(),
                pieces: Vec::new(),
                state: BreakState::Whole,
            });
        }
        out
    }

    /// The breakables' side of the anim world on game tick `tick` (64 Hz): a prop a car has just
    /// started (`Breakable::started`) builds a player for each piece of its speed group (0x476380 —
    /// up to the first that does not resolve) and stops drawing its own article (the record's
    /// entry 0 now holds the first piece); each frame every player is sampled (0x4769E0: the
    /// piece's matrix, then the instance's); once the lead player — the first piece's twin at
    /// `rec+0x1C` — has run out (0x4715A0 answers 2 when the draw samples it through 0x450010's
    /// `rec+8` branch), the next update 0x476810 tears the pieces down (0x476710) and nothing of
    /// the prop is drawn again; its answer stays 0, so cars pass.
    pub fn break_props(&mut self, tick: u32) {
        for b in &mut self.broken {
            if let BreakState::Whole = b.state
                && let Some((start, velocity)) = self.props.get(b.prop).and_then(|p| p.breakable.as_ref()).and_then(|x| x.started.get())
            {
                let Some(br) = self.props[b.prop].breakable.as_ref() else { continue };
                let group = nfs_formats::animdefs::band_group(&br.bands, &br.band_pieces, velocity.length());
                // 0x476588…0x4765E0 / 0x476618…0x476641: the velocity in the instance's frame
                // (0x532A00 transposes its rotation, 0x532A40 applies it), its heading
                // `atan2(x, z)` in turns (0x532860) and the piece's drift `v·velocityFactor`
                let local = b.instance.inverse().transform_vector3(velocity);
                let heading = local.x.atan2(local.z);
                for i in group {
                    if !br.resolved.get(i).copied().unwrap_or(false) {
                        break;
                    }
                    b.players.push(PiecePlayer { template: i, heading, drift: local * b.velocity_factor });
                }
                b.state = BreakState::Playing { start, lead_done: false };
            }
            if let BreakState::Playing { start, lead_done } = b.state {
                if lead_done {
                    b.state = BreakState::Gone;
                    b.players.clear();
                    b.pieces.clear();
                    continue;
                }
                let ticks = tick.saturating_sub(start);
                b.pieces.clear();
                let mut lead = true;
                for (k, pl) in b.players.iter().enumerate() {
                    let t = &b.templates[pl.template];
                    let Some(track) = t.track.as_ref() else { continue };
                    let (m, playing) = sample_piece(track, t.seconds_per_frame, t.flags, pl.heading, pl.drift, ticks);
                    if k == 0 {
                        lead = playing;
                    }
                    for (mesh, material) in &t.meshes {
                        b.pieces.push(DrawItem::new(*mesh, *material, b.instance * m));
                    }
                }
                b.state = BreakState::Playing { start, lead_done: !lead };
            }
        }
    }

    /// Show exactly the mission elements in `shown` among `all` (arrow markers, 0x482F70).
    pub fn set_shown(&mut self, all: &[usize], shown: &[usize]) {
        for e in all {
            self.hidden.insert(*e);
        }
        for e in shown {
            self.hidden.remove(e);
        }
    }

    /// Knock a smackable prop loose: it leaves with `launch` and tumbles until it settles.
    ///
    /// The original hands a smackable to the car physics — 0x4A17F0 gives every one a car-like
    /// record simulated as an extra car with the `animdefs.txt` mass (`CONE` = 20 kg,
    /// `skill-tests.md` §5.3). APPROXIMATION: the port integrates a point body with a fixed spin
    /// instead, with the same launch direction and a ground bounce.
    pub fn knock(&mut self, element: usize, away: Vec3, launch: Vec3) {
        if self.knocked.iter().any(|k| k.element == element) {
            return;
        }
        let items: Vec<(usize, Mat4)> = self
            .item_element
            .iter()
            .enumerate()
            .filter(|(_, e)| **e == Some(element))
            .map(|(i, _)| (i, self.items[i].transform))
            .collect();
        if items.is_empty() {
            return;
        }
        let origin = items[0].1.transform_point3(Vec3::ZERO);
        let ground = self.collision.ground(origin.x, origin.y + 2.0, origin.z, 20.0).map(|h| h.point.y).unwrap_or(origin.y);
        // it tips about the edge it was struck on and keeps turning that way
        let axis = Vec3::Y.cross(away).normalize_or_zero();
        let axis = if axis.length_squared() > 1e-6 { axis } else { Vec3::X };
        self.knocked.push(KnockedProp {
            element,
            items,
            origin,
            offset: Vec3::ZERO,
            velocity: launch,
            axis,
            angle: 0.0,
            rate: 6.0 + launch.length() * 0.6,
            ground,
            resting: false,
        });
    }

    /// Advance the knocked props (gravity, ground bounce, drag) and write their transforms.
    pub fn step_props(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        let mut knocked = std::mem::take(&mut self.knocked);
        for k in knocked.iter_mut().filter(|k| !k.resting) {
            k.velocity.y -= 9.8 * dt;
            k.offset += k.velocity * dt;
            k.angle += k.rate * dt;
            if k.origin.y + k.offset.y <= k.ground {
                k.offset.y = k.ground - k.origin.y;
                if k.velocity.y < -0.5 {
                    k.velocity.y = -k.velocity.y * 0.3;
                    k.velocity.x *= 0.6;
                    k.velocity.z *= 0.6;
                    k.rate *= 0.5;
                } else {
                    // it comes to rest lying on its side
                    k.velocity = Vec3::ZERO;
                    k.angle = std::f32::consts::FRAC_PI_2;
                    k.rate = 0.0;
                    k.resting = true;
                }
            }
            let m = Mat4::from_translation(k.origin + k.offset) * Mat4::from_axis_angle(k.axis, k.angle) * Mat4::from_translation(-k.origin);
            for (i, base) in &k.items {
                self.items[*i].transform = m * *base;
            }
        }
        self.knocked = knocked;
    }
}

/// A smackable prop that has been knocked loose (`TrackWorld::knock`).
struct KnockedProp {
    element: usize,
    /// (scene item index, its transform when it was still standing)
    items: Vec<(usize, Mat4)>,
    origin: Vec3,
    offset: Vec3,
    velocity: Vec3,
    axis: Vec3,
    angle: f32,
    rate: f32,
    ground: f32,
    resting: bool,
}

/// One prop playing its own looping animation.
struct AnimatedProp {
    item: usize,
    /// the transform the article is placed with; the sampled frame is applied inside it
    base: Mat4,
    track: nfs_formats::crp::AnimTrack,
    seconds_per_frame: f32,
    looping: bool,
}

/// A breakable prop drawn in the world (`TrackWorld::break_props`).
struct BrokenProp {
    /// its index in `TrackWorld::props`
    prop: usize,
    /// its own article's draw items, drawn while it is whole
    items: std::ops::Range<usize>,
    /// the instance's matrix, `world+0x40 + 56·id` (0x539CD0 at 0x476A54)
    instance: Mat4,
    /// `def+0x80`, already scaled by 0.001
    velocity_factor: f32,
    /// one per piece of the definition
    templates: Vec<PieceTemplate>,
    /// the pieces started (the record's entries `rec+0x34`, `rec+0x14` of them)
    players: Vec<PiecePlayer>,
    /// this frame's draw items of the pieces
    pieces: Vec<DrawItem>,
    state: BreakState,
}

impl BrokenProp {
    fn hides(&self, item: usize) -> bool {
        !matches!(self.state, BreakState::Whole) && self.items.contains(&item)
    }
}

#[derive(Debug, Clone, Copy)]
enum BreakState {
    Whole,
    /// started on game tick `start`; `lead_done` once the lead player has run out
    Playing { start: u32, lead_done: bool },
    Gone,
}

struct PieceTemplate {
    meshes: Vec<(MeshId, MaterialId)>,
    track: Option<nfs_formats::crp::AnimTrack>,
    seconds_per_frame: f32,
    flags: nfs_formats::animdefs::PieceFlags,
}

/// 0x471490's player: the track's `+0x10` heading (radians here) and `+0x14` drift.
struct PiecePlayer {
    template: usize,
    heading: f32,
    drift: Vec3,
}

/// `AnimPlayer::sample` 0x470E90 with 0x470F70 for a piece `ticks` game ticks after its start:
/// `rate = secondsPerFrame / (1/64)` ticks a frame (`[0x628938]+8`, 0x471700's 0.015625);
/// `f = ticks / rate`; a looping track wraps at `frames − 2`, a one-shot holds `frames − 1`; the
/// quaternion and the position are lerped component by component (0x48A150, **not** normalised),
/// the matrix built from that quaternion as it is (0x48A1E0) with the lerped scale; `DIRECTED`
/// (flag 8) turns it after about y by the heading (0x47108A…0x4710C0, `M·R`), `LANDFLAT` (flag
/// 0x10) before about y by `−heading·(1 − (i + frac)/frames)` (0x4710C3…0x47112C, `R·M`); the
/// translation then gains `drift · ticks · rate` (0x470F17…0x470F51). The answer's flag is
/// `i + 1 < frames`, the player still running.
fn sample_piece(track: &nfs_formats::crp::AnimTrack, seconds_per_frame: f32, flags: nfs_formats::animdefs::PieceFlags, heading: f32, drift: Vec3, ticks: u32) -> (Mat4, bool) {
    let n = track.frames.len();
    if n == 0 {
        return (Mat4::IDENTITY, false);
    }
    let rate = seconds_per_frame / 0.015625;
    let f = ticks as f32 / rate;
    let mut i = f as usize;
    let mut frac = f - i as f32;
    if flags.looping && n > 2 {
        i %= n - 2;
    } else if i + 2 >= n {
        i = n - 1;
        frac = 0.0;
    }
    let (a, b) = (&track.frames[i], &track.frames[(i + 1).min(n - 1)]);
    let lerp = |x: f32, y: f32| x + (y - x) * frac;
    let q = glam::Quat::from_xyzw(lerp(a.rotation[0], b.rotation[0]), lerp(a.rotation[1], b.rotation[1]), lerp(a.rotation[2], b.rotation[2]), lerp(a.rotation[3], b.rotation[3]));
    let pos = Vec3::from(a.position).lerp(Vec3::from(b.position), frac);
    let scale = Vec3::from(a.scale).lerp(Vec3::from(b.scale), frac);
    let mut m = Mat4::from_translation(pos) * Mat4::from_mat3(glam::Mat3::from_quat(q)) * Mat4::from_scale(scale);
    if flags.directed {
        m = Mat4::from_rotation_y(heading) * m;
    }
    if flags.landflat {
        m *= Mat4::from_rotation_y(-heading * (1.0 - (i as f32 + frac) / n as f32));
    }
    let off = drift * ticks as f32 * rate;
    m.w_axis += off.extend(0.0);
    (m, i + 1 < n)
}

/// GPU-resident car model at one LOD, split into body parts (with their pivots applied) and
/// wheels (kept separate so they can spin/steer). Model space (RE/notes/car-model.md §3) is
/// +Y up, −Z front, +X left; `MODEL_TO_BODY` rotates it into the physics body frame (+Z forward).
pub struct CarModel {
    /// The model's scale in its render matrix: 1, or **0.4** for a `.TINYCAR` (`[0x5B37C8]`, the
    /// scale 0x4433D2 builds into the car's matrix through 0x539B30)
    pub scale: f32,
    pub body: Vec<(MeshId, MaterialId, Mat4)>,
    /// Each material's `R3DLightLayer` parameters (`RE/notes/lighting.md` §6) — every shipped car
    /// material has them.
    material_light: HashMap<MaterialId, nfs_formats::crp::LayerLight>,
    /// The materials whose render method takes the shadow projector (layer 0x8101).
    shadowed: std::collections::HashSet<MaterialId>,
    /// The meshes each **model animation** morphs (`Base+77`, index 1…7), with every frame's
    /// positions: 1 the convertible's roof, 2 the `SpoilerW` wing, 3 the wipers, 4 / 5 / 6 the
    /// driver's right arm, left arm and body, 7 the aerial (`anims` example over every car).
    morph: [Vec<MorphPart>; ANIMS],
    /// `0x438190(n)`: animation `n`'s length in frames, 0 for a car without it
    anim_length: [u32; ANIMS],
    /// the time each animation was last set to (0x4380D0), so an unchanged one is not re-uploaded
    anim_time: [f32; ANIMS],
    roof_state: i32,
    driver: DriverAnim,
    /// the `{frame, next, frac}` each animation's meshes were last blended to
    blended: [Option<(usize, usize, f32)>; ANIMS],
    /// The **rotation channel** of each `body` entry — the article's `Base+75`, 0 for a part that
    /// never moves (`car-model.md`). `R3DCarLOD::draw` picks it at 0x43C98A and turns
    /// `channel_turns[n]` into `Rx·Ry·Rz` about the part's own origin.
    body_channel: Vec<u8>,
    /// The **draw order** key of each `body` entry — the article's `Base+70` byte. 0x43C1B0 lists
    /// a LOD's parts as `{article, lod, Base+70}` and `std::sort`s them by that byte, signed and
    /// ascending (0x43C35B → 0x43E710 / 0x43E910, `cmp bl, cl` on `+8`), and `R3DCarLOD::draw`
    /// 0x43C830 walks them in that order. On the 993 the wheels are 0x0A and the wheel shadows,
    /// the `Shadow.fsh` quads over the wheels' outer faces, 0x0B — drawn after the wheel they
    /// darken.
    body_key: Vec<i8>,
    /// `.tpg` `movespoilerextent` → `R3DCar+0x74` (0x441D08), in **turns**: −0.04 on the 996 and
    /// the GT3, −0.08 on the two Boxsters, and absent everywhere else.
    move_spoiler_extent: f32,
    /// `.tpg` `headlightextent` → `R3DCar+0x70` (0x441CF7), in **turns**: 0.19 on the 914, −0.18
    /// on the 928, 0.14 on the 944 — the three Porsches with pop-up headlights, and no others.
    headlight_extent: f32,
    spoiler: ChannelEase,
    headlight: ChannelEase,
    /// `.tpg` `dooraxis` → `R3DCar+0x2B0` (0x441C92), default (0, 1, 0): the axis channels 6 and 7
    /// turn about, x negated for 7 (0x43CA5A…0x43CA7D)
    door_axis: Vec3,
    /// `.tpg` `hoodextent` → `+0x68` (0x441CCC), default **0.15** turn; `trunkextent` → `+0x6C`
    /// (0x441CE6), default **−0.15** — the targets of the two lid easers
    hood_extent: f32,
    trunk_extent: f32,
    /// the lid easers at `car+0x64 + 0x80` (hood) and `+ 0x90` (engine lid) that slot 9 writes
    /// into channels 8 and 9 (0x4387CE…0x43891C)
    hood: ChannelEase,
    trunk: ChannelEase,
    /// `object + 0x2DC + 12n` — three floats a channel, **rotations in turns** about x, y and z
    /// (0x43C9C0 / 0x43C9D7 / 0x43C9F8 each multiply by `[0x5B25CC]` = 360.0 before the
    /// axis-angle build 0x539BD0). Only the x column is used so far: channel **16** is
    /// `MoveSpoiler` and **17** the pop-up `HeadLight`.
    pub channel_turns: [f32; 32],
    /// the same channels' `y` and `z` turns — the wheels' steer and camber, the steering wheel's
    /// and the dial needles' `z`, the siren's `y` (slot 9, `car-model.md` "Slot 9")
    channel_yz: [[f32; 2]; 32],
    /// `nfs5.car +0x2F0…+0x2FC` (the options 0x251…0x254 `.TACH0RPM`, `.TACH1000RPM`,
    /// `.SPEEDO0KM`, `.SPEEDO100KM`, sent at 0x4EF331…0x4EF37B into `carinfo+0x524…+0x530`): the
    /// dash needles' rest angle and scale, in turns
    dash: [f32; 4],
    /// the frame clock the race channels run on, ms (`TIMER_getTicks` is 128 Hz)
    clock_ms: u64,
    /// (pivot in body space, is_front, is_left, parts)
    pub wheels: Vec<CarWheelModel>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub wheel_radius: f32,
    pub half_track: f32,
    /// The car's lamps: every glare article's `ef` record and its mirrored copy
    /// (`RE/notes/particles.md` §4).
    pub lights: Vec<CarLight>,
    /// The car's texture pages before any damage, the decals of `<model>D.fsh` / `<model>L.fsh` and
    /// the page textures to rewrite when they change (`RE/notes/car-model.md` §11).
    damage_pages: Vec<nfs_assets::car::TexturePage>,
    damage_patches: Vec<nfs_assets::car::DamagePatch>,
    page_textures: Vec<nfs_renderer::TextureId>,
    /// last damage the pages were composed for, rounded, so a page is only rebuilt when it changes
    damage_shown: [u8; 10],
    /// The windscreen quad (`Wndo`) with the `INTGLASS` glass texture, drawn only from inside the
    /// car (`RE/notes/car-model.md` §12).
    pub window: Option<(MeshId, MaterialId)>,
}

/// A lamp's `ef` record (`RE/notes/particles.md` §4), in the body frame.
#[derive(Debug, Clone, Copy)]
pub struct CarLight {
    /// `L+0x00`, the lamp's centre
    pub position: Vec3,
    /// `L+0x10` / `L+0x20`, its two half-axes
    pub axis1: Vec3,
    pub axis2: Vec3,
    /// `L+0x30`, the way it shines
    pub facing: Vec3,
    /// `L+0x40` / `L+0x44`, the two sprites' ARGB colours
    pub colour: [u32; 2],
    /// `L+0x4C`: 0 headlight, 1 brake, 2 reverse, 3 indicator, 4 glint (the 0x5CD73C row)
    pub kind: u8,
    /// the lamp slot `car+0x2D4[slot]` that switches it (0x443660)
    pub slot: u8,
    /// `L+0x4F`: the headlight that also throws the night beam 0x43D4F0
    pub night: bool,
}

/// Model animations are numbered 1…7 in the shipped cars; slot 0 is unused.
const ANIMS: usize = 8;

/// One of slot 9's eight keyframe players, `R3DCar+0x484 + 0x1C·n` (0x438B66…0x438C3D) — player
/// `n` drives animation `n`: `{count, table, start, duration, frame, next, frac}`.
#[derive(Debug, Clone, Copy, Default)]
struct KeyPlayer {
    table: &'static [u32],
    start_ms: u64,
    duration_ms: u64,
    frame: u32,
    next: u32,
    frac: f32,
}

impl KeyPlayer {
    fn active(&self) -> bool {
        !self.table.is_empty()
    }

    /// A gesture starting now: frame and next are the table's first two when it has two
    /// (0x438DF1…0x438E18), the fraction 0.
    fn start(&mut self, table: &'static [u32], now: u64, duration_ms: u64) {
        self.table = table;
        self.start_ms = now;
        self.duration_ms = duration_ms;
        if table.len() > 1 {
            self.frame = table[0];
            self.next = table[1];
        }
        self.frac = 0.0;
    }

    /// 0x438B73…0x438C30: while the elapsed time is under the duration, `p = elapsed / duration ·
    /// (count − 1)`, the frame `table[fistp(p − 0.5)]`, the next `table[fistp(p + 0.5)]` and the
    /// fraction `p − fistp(p − 0.5)`; past it the player stops, its last frame kept.
    fn step(&mut self, now: u64) {
        if !self.active() {
            return;
        }
        let elapsed = now.saturating_sub(self.start_ms);
        if elapsed >= self.duration_ms {
            self.table = &[];
            return;
        }
        let p = elapsed as f32 / self.duration_ms as f32 * (self.table.len() as f32 - 1.0);
        let i = (p - 0.5).round_ties_even().max(0.0) as usize;
        let j = (p + 0.5).round_ties_even().max(0.0) as usize;
        self.frame = self.table[i.min(self.table.len() - 1)];
        self.next = self.table[j.min(self.table.len() - 1)];
        self.frac = p - i as f32;
    }
}

/// The right arm's gear-change gestures, by the steering frame 0…9 (`0x5CD3AC`…`0x5CD4C0`, the
/// ten `{count, table}` pairs 0x438CD1…0x438D42 builds): from the wheel out to the lever's frames
/// 10…15 and back.
const GEAR_GESTURES: [&[u32]; 10] = [
    &[0, 10, 11, 12, 14, 15, 14, 12, 11, 10, 0],
    &[1, 10, 11, 12, 14, 15, 14, 12, 11, 10, 1],
    &[2, 11, 12, 14, 15, 14, 12, 11, 2],
    &[3, 11, 12, 14, 15, 14, 12, 11, 3],
    &[4, 12, 14, 15, 14, 12, 4],
    &[5, 12, 14, 15, 14, 12, 5],
    &[6, 13, 15, 13, 6],
    &[7, 13, 15, 13, 7],
    &[8, 13, 15, 13, 8],
    &[9, 13, 15, 13, 9],
];

/// The handbrake's, three frames each (`0x5CD2BC` pulling, `0x5CD334` letting go, 12 bytes apart,
/// 0x438EB0…0x438F85): to the lever at 16…18 and the held frame 19.
const HANDBRAKE_ON: [[u32; 3]; 10] =
    [[0, 16, 19], [1, 16, 19], [2, 16, 19], [3, 16, 19], [4, 16, 19], [5, 17, 19], [6, 17, 19], [7, 18, 19], [8, 18, 19], [9, 18, 19]];
const HANDBRAKE_OFF: [[u32; 3]; 10] =
    [[19, 16, 0], [19, 16, 1], [19, 16, 2], [19, 16, 3], [19, 16, 4], [19, 17, 5], [19, 17, 6], [19, 18, 7], [19, 18, 8], [19, 18, 9]];

/// The body's turn to look back, by the body frame 0…4 (`0x5CD568` into reverse, `0x5CD590` out
/// of it, `{count, table}` pairs, 0x43914F / 0x4391A3): toward frame 7 and back.
const REVERSE_ON: [&[u32]; 5] = [&[0, 1, 2, 6, 7], &[1, 2, 6, 7], &[2, 6, 7], &[3, 6, 7], &[4, 6, 7]];
const REVERSE_OFF: [&[u32]; 5] = [&[7, 6, 2, 1, 0], &[7, 6, 2, 1], &[7, 6, 2], &[7, 6, 3], &[7, 6, 4]];

/// What slot 9's driver half reads off the car each frame (0x438B2C…0x439352).
#[derive(Debug, Clone, Copy, Default)]
pub struct DriverInputs {
    /// `[0x6197C4]`, the car detail (`CARDETAIL`, 0x44EF80…0x44EFA7): under 1 the driver is posed,
    /// not animated
    pub detail: u8,
    /// channel 5's `.z`: `ch1.y + ch2.y` — the two front wheels' `+0xB4` steering values over 216,
    /// negated in a mirrored race (0x438461…0x4384EB) — clamped to ±4.5/23 (0x4385AF…0x4385EB)
    pub wheel_turns: f32,
    /// `car+0xD84`, the shift timer (ticks)
    pub shift_timer: u32,
    /// `car+0xB50 == 0` and neither pause flag (`[0x606A90]`, `[0x606A98]`) set (0x438CA0…0x438CC2)
    pub racing: bool,
    /// `car+0xD86`
    pub handbrake: bool,
    /// `car+0xD82 == 0`
    pub reverse: bool,
    /// `car+0xE04` / `+0xE08`, the filtered longitudinal and lateral accelerations
    pub long_acc: f32,
    pub lat_acc: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct DriverAnim {
    clock_ms: u64,
    players: [KeyPlayer; ANIMS],
    /// `R3DCar+0x564` / `+0x566`: the handbrake and reverse states the gestures were last started for
    handbrake: bool,
    reverse: bool,
}

/// One `{start, duration, from, to}` easer — the 16-byte record the original keeps per channel at
/// `object+0x64 + 16n` (`car-model.md`), with the clock kept as elapsed rather than a start tick.
#[derive(Debug, Clone, Copy, Default)]
struct ChannelEase {
    from: f32,
    to: f32,
    elapsed: u32,
    duration: u32,
}

/// One float key out of a `.tpg` `[header]`, `default` when absent.
fn tpg_f32_or(tpg: &nfs_formats::ini::Ini, key: &str, default: f32) -> f32 {
    tpg.section("header").and_then(|h| h.get_f32_list(key)).and_then(|v| v.first().copied()).unwrap_or(default)
}

/// One float key out of a `.tpg` `[header]`, 0.0 when absent.
fn tpg_f32(tpg: &nfs_formats::ini::Ini, key: &str) -> f32 {
    tpg.section("header").and_then(|h| h.get_f32_list(key)).and_then(|v| v.first().copied()).unwrap_or(0.0)
}

pub struct CarWheelModel {
    pub pivot: Vec3,
    pub is_front: bool,
    pub is_left: bool,
    /// The article's `Base+75` rotation channel. Every wheel-typed article of every shipped car is
    /// bound to **1…4** — the wheels that spin — or to **18…21**, the tread that takes the steer
    /// but not the spin, because 0x438669…0x438726 copy channels 1…4 into 18…21 with `.x` cleared
    /// (`cargo run -p nfs-formats --example rotchan`).
    pub channel: u8,
    /// the article's `Base+70` draw-order key (see `CarModel::body_key`)
    pub key: i8,
    pub parts: Vec<(MeshId, MaterialId, Mat4)>,
}

/// The car's collision body points in the physics body frame, `y = 0` on the wheel contact plane
/// (`BndP`, `RE/notes/car-model.md` §12), sorted ascending by height the way 0x49ABC0 does — so the
/// first four are the wheel contact set. Only the records every car carries are taken: a record
/// naming a style slot is part of the body only when that slot holds its value, and the port does
/// not track the style array yet, so spoilers and wings are left out.
pub fn car_body_points(car: &CarAssets) -> Vec<Vec3> {
    let Ok(recs) = car.crp.bound_points(&car.crp_data) else { return Vec::new() };
    // The loader's filter is `carinfo[0x56C + 4·slot] == value` (0x4420B9), i.e. the body style's
    // own array — not "slot 0 only", which is what this used to be and which left every
    // style-selected record out (the spoilers, the GT1 wing).
    let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
    let mut pts: Vec<Vec3> = recs
        .iter()
        .filter(|r| style.article_enabled(r.style_slot as u8, r.style_value as u8))
        .flat_map(|r| r.points.iter().map(|p| MODEL_TO_BODY.transform_point3(Vec3::from(*p))))
        .collect();
    pts.sort_by(|a, b| a.y.total_cmp(&b.y));
    pts
}

pub const MODEL_TO_BODY: Mat4 = Mat4::from_cols(
    glam::Vec4::new(-1.0, 0.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 0.0, -1.0, 0.0),
    glam::Vec4::new(0.0, 0.0, 0.0, 1.0),
);

impl CarModel {
    pub fn upload(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: impl Into<nfs_assets::car::Paint>) -> anyhow::Result<Self> {
        Self::upload_with_env(r, car, paths, lod, paint, None, false, None, None)
    }

    /// `env`: track environment map (`.env` entry, e.g. "SKY ") for `CarExtEnv` paint reflections.
    ///
    /// `race_roof`: in a race, the car's `.CABRIODOWN` option for slot 19's roof line
    /// ([`nfs_assets::car::CarStyle::apply_race_roof`]); `None` in the front end, whose branch
    /// (0x43B003) does not take it.
    ///
    /// `skidpad`: the `.tpg` entry test's `testdrive` context — `Some(Factory Driver mission)` on
    /// `skidpad`, `None` elsewhere ([`nfs_assets::car::CarStyle::skidpad`]).
    #[allow(clippy::too_many_arguments)]
    pub fn upload_with_env(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: impl Into<nfs_assets::car::Paint>, env: Option<nfs_renderer::TextureId>, night: bool, race_roof: Option<u32>, skidpad: Option<bool>) -> anyhow::Result<Self> {
        // `[styleN]` is **1-based** and `CarAssets::style_index` is the raw 0-based `nfs5.car
        // +0x1C0`, which is what 0x437B33's `inc esi` turns into the section name.
        let mut style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, car.style_index + 1);
        // …and after the section `applyStyle` writes slot 10 (`car+0x594`) from the race number:
        // 1 when car-info `+0x510` is 0 or more, else 0 (0x437C09…0x437C2B; the front end's
        // 0x4D1800 does the same from the car's `DecalNum`). A car without page text is the
        // front end's or one whose number was not read, and −1 is what both hold.
        style.geometry.insert(10, u32::from(car.page_text.as_ref().is_some_and(|t| t.race_number >= 0)));
        // Slot 19 overlays the driver's variants onto the same array at model load — the port
        // draws at the best detail the table carries, so `slot9 = 1, slot43 = 1`.
        //
        // The other two things that block writes (`car-model.md`) are **left to the style**, and
        // deliberately: slot 42 (`SpoilerDown` / `SpoilerUp`) is zeroed by 0x43AEB2, which hides
        // both halves of the pop-up spoiler until the animation channel picks one, and the port
        // does not run that channel — so zeroing it would leave a hole in the engine lid where
        // the style's own `42 = 1`, the retracted spoiler, belongs. Slot 41 is recomputed from
        // the roof option in a race (0x43AE7A).
        style.apply_driver_detail(8, false, false);
        match race_roof {
            Some(v) => style.apply_race_roof(v),
            // slot 19's front-end branch, 0x43B003: slot 41 and slot 6 to 0
            None => {
                style.geometry.insert(41, 0);
                style.geometry.insert(6, 0);
            }
        }
        // and both branches end in the overlay, 0x43B0B7…0x43B0DC, and in `slot58 =
        // carinfo+0x518`, the `.TINYCAR` flag (0x43B0E5)
        style.apply_overlay(&car.style_overlay);
        style.geometry.insert(58, u32::from(car.tiny));
        // the entry test's context (0x439C60): the front-end branch is the one without a race roof
        style.front_end = race_roof.is_none();
        style.night = night;
        style.skidpad = skidpad;
        tracing::info!(style = style.name, "car style");
        let pages = car.build_pages(paths, paint, &style)?;
        let page_tex: Vec<(nfs_renderer::TextureId, bool)> = pages
            .iter()
            .map(|p| (r.upload_texture(&TextureData::single(p.width, p.height, p.rgba.clone())), p.is_window || p.is_alpha))
            .collect();
        // the damage decals; `night` follows the same `car+0xAA <= 0x50` rule the original uses
        let damage_patches = nfs_assets::car::damage_patches(paths, &car.model, night).unwrap_or_default();
        let page_textures: Vec<nfs_renderer::TextureId> = page_tex.iter().map(|p| p.0).collect();
        let materials = car.crp.materials(&car.crp_data)?;
        let mut mat_ids: HashMap<u16, MaterialId> = HashMap::new();
        let mut material_light: HashMap<MaterialId, nfs_formats::crp::LayerLight> = HashMap::new();
        let mut shadowed: std::collections::HashSet<MaterialId> = std::collections::HashSet::new();
        let mut material_for = |r: &mut Renderer, id: u16| {
            if let Some(m) = mat_ids.get(&id) {
                return *m;
            }
            let m = materials.get(id as usize);
            let page = m.map(|m| m.page_index() as usize).and_then(|p| page_tex.get(p));
            let blend = m.map(|m| matches!(m.render_method.as_str(), "CarWindow" | "CarWheel")).unwrap_or(false) || page.map(|p| p.1).unwrap_or(false);
            let reflective = m.map(|m| m.render_method == "CarExtEnv").unwrap_or(false);
            // `R3DSpecularTextureLayer` buckets the material's own reflection strength to pick
            // which of the three environment textures it reflects (`lighting.md` §3.1); the 0.3
            // blend stays the APPROXIMATION it was, since the pass's combine is AUDIT.md §E
            let variant = nfs_formats::crp::env_variant(m.map(|m| m.reflection).unwrap_or(0.0));
            // the cull mode is the material's own `Rs` block, Thrash state 2 at `mt+272`: 1 `CCW`
            // (the back faces), 0 none (`thrash.md` §6) — the port had culled every opaque one and
            // none of the blended, which with depth writes on let a wheel well's outer faces cover
            // the wheel
            let cull = m.and_then(|m| m.rs).filter(|_| !car.front_end_room).map_or(!blend, |rs| rs[3] == 1);
            let mid = r.create_material(MaterialDesc { texture: page.map(|p| p.0), alpha_blend: blend, cull_back: cull, depth_test: true, env_texture: if reflective { env } else { None }, env_mix: 0.3, env_variant: variant, blend_pair: 0, clamp: m.map_or([false; 2], |m| m.address.map(|a| a == 0)), lod_bias: m.map_or(0.0, |m| m.lod_bias), device_states: !car.front_end_room });
            if let Some(l) = m.and_then(|m| m.light) {
                material_light.insert(mid, l);
            }
            // the methods whose second pass lists layer 0x8101, `R3DShadowMapTextureLayer`
            // (`RE/notes/lighting.md` §3)
            if m.is_some_and(|m| matches!(m.render_method.as_str(), "CarExtEnv" | "CarExtEnvNS" | "CarWindow")) {
                shadowed.insert(mid);
            }
            mat_ids.insert(id, mid);
            mid
        };
        let arts = car.crp.articles(&car.crp_data)?;
        let mut body = Vec::new();
        let mut body_channel: Vec<u8> = Vec::new();
        let mut body_key: Vec<i8> = Vec::new();
        let mut wheels = Vec::new();
        let mut skipped = 0;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut lights: Vec<CarLight> = Vec::new();
        let mut morph: [Vec<MorphPart>; ANIMS] = Default::default();
        let mut anim_length = [0u32; ANIMS];
        let mut wheel_radius: f32 = 0.3;
        let mut half_track: f32 = 0.7;
        for a in &arts {
            // the glare articles hold no geometry, only an `ef` record (`particles.md` §4): its
            // lamp, the slot `Base[0x50]` that switches it (0x43C427 / 0x43D8C0), and a mirrored copy
            // across x with the slot `L+0x4E` (0x43DAC5…0x43DB32). The two LOD copies are the same.
            if let Some(ef) = a.parts.iter().find(|p| p.is_part(b"ef"))
                && let Ok(b) = car.crp.entry_data(&car.crp_data, ef)
                && b.len() >= 88
                && u32::from_le_bytes([b[0], b[1], b[2], b[3]]) == 5
            {
                let f = |o: usize| f32::from_le_bytes([b[8 + o], b[9 + o], b[10 + o], b[11 + o]]);
                let v = |o: usize| Vec3::new(f(o), f(o + 4), f(o + 8));
                let u = |o: usize| u32::from_le_bytes([b[8 + o], b[9 + o], b[10 + o], b[11 + o]]);
                let slot = a
                    .level(b"Base")
                    .and_then(|e| car.crp.entry_data(&car.crp_data, e).ok())
                    .and_then(|d| d.get(0x50).copied())
                    .unwrap_or(9);
                let to_body = |p: Vec3| MODEL_TO_BODY.transform_point3(p);
                let dir_body = |d: Vec3| MODEL_TO_BODY.transform_vector3(d);
                let lamp = CarLight {
                    position: to_body(v(0)),
                    axis1: dir_body(v(0x10)),
                    axis2: dir_body(v(0x20)),
                    facing: dir_body(v(0x30)),
                    colour: [u(0x40), u(0x44)],
                    kind: b[8 + 0x4C],
                    slot,
                    night: b[8 + 0x4F] != 0,
                };
                if !lights.iter().any(|l| l.position == lamp.position && l.slot == lamp.slot) {
                    let flip = |d: Vec3| Vec3::new(-d.x, d.y, d.z);
                    let mirror = CarLight {
                        position: flip(lamp.position),
                        axis1: -lamp.axis1,
                        axis2: flip(lamp.axis2),
                        facing: flip(lamp.facing),
                        slot: b[8 + 0x4E],
                        ..lamp
                    };
                    lights.push(lamp);
                    lights.push(mirror);
                }
                continue;
            }
            let g = car.crp.article_geometry(&car.crp_data, a)?;
            if !g.primitives.iter().any(|p| p.lod == lod) {
                continue;
            }
            let (ty, variant) = g.base.as_ref().map(|b| (b.tag[0], b.tag[1])).unwrap_or((0, 0));
            if !style.article_enabled(ty, variant) {
                continue;
            }
            let is_wheel = matches!(ty, 0x0D | 0x0E);
            let local = g.transform_for_lod(lod).map(|m| Mat4::from_cols_array(&m)).unwrap_or(Mat4::IDENTITY);
            let mut parts = Vec::new();
            let mut part_channels: Vec<u8> = Vec::new();
            // an animated article's morph frames: each frame's meshes, in frame 0's order —
            // 0x43C4FB probes `vt` part `frame·0x10 + index` until one is missing, and the count
            // is the animation's length when it is the largest (0x43C54B…0x43C56F)
            let anim = g.base.as_ref().map(|b| b.anim as usize).filter(|&n| n > 0 && n < ANIMS).unwrap_or(0);
            let roof_frames: Vec<Vec<MeshData>> = if !is_wheel && anim != 0 {
                let mut frames = Vec::new();
                for f in 1u16.. {
                    let Ok(gf) = car.crp.article_geometry_frame(&car.crp_data, a, f) else { break };
                    let mut sk = 0;
                    frames.push(article_meshes(&gf, lod, &mut sk).into_iter().map(|m| m.mesh).collect());
                }
                frames
            } else {
                Vec::new()
            };
            if !roof_frames.is_empty() {
                anim_length[anim] = anim_length[anim].max(roof_frames.len() as u32 + 1);
            }
            for (k, mm) in article_meshes(&g, lod, &mut skipped).into_iter().enumerate() {
                let mut mesh = mm.mesh;
                for v in &mut mesh.vertices {
                    let p = (MODEL_TO_BODY * local).transform_point3(Vec3::from(v.position));
                    if !is_wheel {
                        min = min.min(p);
                        max = max.max(p);
                    } else {
                        // the tyre's rolling radius is its extent about the hub, not the distance
                        // to a corner of the wheel's quad: the 993's wheel mesh spans ±0.3148 in
                        // both y and z, and the `.sim` tyre (16\" rim, 205/55) has a radius of
                        // 16·12.7 + 205·0.55 = 315.95 mm
                        let lp = Vec3::from(v.position);
                        wheel_radius = wheel_radius.max(lp.y.abs()).max(lp.z.abs());
                    }
                }
                let material = material_for(r, mm.material);
                let id = r.upload_mesh(&mesh);
                if !roof_frames.is_empty() {
                    let mut positions = vec![mm_positions(&mesh)];
                    for f in &roof_frames {
                        match f.get(k) {
                            Some(m) if m.vertices.len() == mesh.vertices.len() => positions.push(mm_positions(m)),
                            _ => break,
                        }
                    }
                    morph[anim].push(MorphPart { mesh: id, base: mesh.vertices.clone(), frames: positions });
                }
                parts.push((id, material, local));
                part_channels.push(g.base.as_ref().map(|b| b.rot_channel).unwrap_or(0));
            }
            if is_wheel {
                let pivot = MODEL_TO_BODY.transform_point3(local.w_axis.truncate());
                half_track = half_track.max(pivot.x.abs());
                let channel = g.base.as_ref().map(|b| b.rot_channel).unwrap_or(0);
                let key = g.base.as_ref().filter(|_| !car.front_end_room).map_or(0, |b| b.tag[2] as i8);
                wheels.push(CarWheelModel { pivot, is_front: ty == 0x0D, is_left: local.w_axis.x > 0.0, channel, key, parts });
            } else {
                body_key.extend(std::iter::repeat_n(g.base.as_ref().filter(|_| !car.front_end_room).map_or(0, |b| b.tag[2] as i8), parts.len()));
                body.extend(parts);
                body_channel.extend(part_channels);
            }
        }
        tracing::info!(body_parts = body.len(), wheels = wheels.len(), lights = lights.len(), roof_parts = morph[1].len(), roof_length = anim_length[1], anims = ?anim_length, skipped, pages = pages.len(), wheel_radius, half_track, "car uploaded");
        // the windscreen seen from the driver's seat: the `Wndo` quad with `INTGLASS`'s `glai`
        // entry. 0x43A490 draws it whenever the camera's view id is 0 or 1 — the two views that do
        // not draw the car itself — and skips it in a replay.
        let window = (|| {
            let quad = car.crp.window_points(&car.crp_data).ok()?.into_iter().find(|w| w.style_slot == 0 && w.style_value == 0)?;
            if quad.points.len() < 4 {
                return None;
            }
            let p: Vec<Vec3> = quad.points.iter().take(4).map(|q| MODEL_TO_BODY.transform_point3(Vec3::from(*q))).collect();
            let g = nfs_assets::car::global_texture(paths, "INTGLASS.FSH", "glai").ok()??;
            let tex = r.upload_texture(&TextureData { width: g.width, height: g.height, mips: g.mips.clone() });
            // the batch's state object (0x5379F0 at 0x4578AB) is set to blend 1 at 0x4578B4 and
            // handed to the batch with the uv and colour buffers (0x4578F2 → 0x4685E0): Thrash
            // 0x38 = 1, `SRCALPHA` / `ONE` — the glass adds its light (`RE/notes/thrash.md` §6)
            let mat = r.create_material(MaterialDesc { texture: Some(tex), alpha_blend: true, cull_back: false, depth_test: true, env_texture: None, env_mix: 0.0, env_variant: 0, blend_pair: 1, clamp: [false; 2], lod_bias: 0.0, device_states: false });
            // **The quad is mapped to the whole texture**, and that is traced now. The batcher's
            // three parallel buffers are positions at `acc+0x0C` (stride 0x10), **texture
            // coordinates at `acc+0x25BC` (stride 8)** and colours at `acc+0x3894` (stride 4), 201
            // triangles of three vertices each. The whole image holds **three** references to
            // `+0x25BC`: two are the flush in 0x457E60, and the third, 0x4578EB, registers the
            // buffer as a stream. Nothing ever writes it per quad — it is filled **once**, at
            // 0x458150 from the accumulator's setup (0x457864), with a fixed pattern that
            // alternates on the triangle index:
            //
            // ```text
            // even: (1,0) (1,1) (0,0)      odd: (0,1) (0,0) (1,1)
            // ```
            //
            // so each **pair** of triangles covers the unit square. There is no generated mapping
            // to find, which is what an earlier note had guessed at.
            //
            // What is left is which corner takes which of the four: the original's vertices reach
            // the buffer through 0x4583F0 (`origin + (a·len)·u + b·v` on the plane 0x457920 sorts
            // by y and then x), and this port uses the record's own order. That ordering is the
            // one residual here.
            let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
            let mut mesh = MeshData::default();
            for (i, q) in p.iter().enumerate() {
                mesh.vertices.push(Vertex { position: q.to_array(), normal: [0.0, 1.0, 0.0], uv: uv[i], color: [1.0; 4] });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            Some((r.upload_mesh(&mesh), mat))
        })();
        tracing::info!(patches = damage_patches.len(), night, glass = window.is_some(), "damage decals");
        Ok(Self {
            scale: 1.0,
            body,
            body_channel,
            body_key,
            material_light,
            shadowed,
            morph,
            anim_length,
            anim_time: [-1.0; ANIMS],
            roof_state: 0,
            driver: DriverAnim::default(),
            blended: [None; ANIMS],
            channel_turns: [0.0; 32],
            channel_yz: [[0.0; 2]; 32],
            dash: [0.0; 4],
            clock_ms: 0,
            move_spoiler_extent: tpg_f32(&car.tpg, "movespoilerextent"),
            headlight_extent: tpg_f32(&car.tpg, "headlightextent"),
            spoiler: ChannelEase::default(),
            headlight: ChannelEase::default(),
            door_axis: car
                .tpg
                .section("header")
                .and_then(|h| h.get_f32_list("dooraxis"))
                .filter(|v| v.len() >= 3)
                .map(|v| Vec3::new(v[0], v[1], v[2]))
                .unwrap_or(Vec3::Y),
            hood_extent: tpg_f32_or(&car.tpg, "hoodextent", 0.15),
            trunk_extent: tpg_f32_or(&car.tpg, "trunkextent", -0.15),
            hood: ChannelEase::default(),
            trunk: ChannelEase::default(),
            wheels,
            bbox_min: min,
            bbox_max: max,
            wheel_radius,
            half_track,
            lights,
            damage_pages: pages,
            damage_patches,
            page_textures,
            damage_shown: [0; 10],
            window,
        })
    }

    /// Recompose the texture pages the damage decals touch (`RE/notes/car-model.md` §11). Cheap to
    /// call every frame: it does nothing until a zone's damage changes by a whole per cent.
    pub fn update_damage(&mut self, r: &mut Renderer, damage: &[f32; 10]) {
        if self.damage_patches.is_empty() {
            return;
        }
        let now: [u8; 10] = std::array::from_fn(|i| damage[i].clamp(0.0, 100.0) as u8);
        if now == self.damage_shown {
            return;
        }
        self.damage_shown = now;
        let touched: Vec<usize> = {
            let mut v: Vec<usize> = self.damage_patches.iter().map(|p| p.page).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        for page_index in touched {
            let Some(base) = self.damage_pages.get(page_index) else { continue };
            let Some(&tex) = self.page_textures.get(page_index) else { continue };
            let mut page = base.clone();
            for patch in self.damage_patches.iter().filter(|p| p.page == page_index) {
                nfs_assets::car::blend_patch(&mut page, patch, damage[patch.zone as usize]);
            }
            r.update_texture(tex, &TextureData::single(page.width, page.height, page.rgba));
        }
    }

    /// Slot 9's race branch for the parts this port animates (0x43891E…0x438B27, the race only —
    /// `[0x65B928]` set skips it) and slot 6's headlight easer.
    ///
    /// **The pop-up wing is animation 2**, the `SpoilerW` morph — not channel 16, which this said
    /// until 2026-09-22 (channel 16 is written only in the front end, from the roof: see
    /// [`Self::follow_roof`]). Above `[0x5B3698]` = 22.222 m/s (**80 km/h**) of `car+0xEC4`, and
    /// only once the easer at `car+0x64 + 0x170` has run its course (0x438950), it restarts from
    /// where it is toward animation 2's length (`[car+0x59C]`, 0x43895E) over **2000 ms**; below
    /// `[0x5B3694]` = 13.889 (**50**) the same toward 0 (0x4389D1…0x438A57). It re-arms whenever
    /// the gate holds and the last travel is over, even toward the value it already has — so a
    /// retract can wait out an idle travel. The value (0x438A5E…0x438AD1: `from + (to − from)·t`,
    /// `to` once `t` passes 1) goes to 0x4380D0(2, ·).
    ///
    /// **The aerial is animation 7**, and only a `.TINYCAR` moves it (0x438ADE…0x438B27):
    /// `min(0.067 · car+0xEC4, 1)` (`[0x5B3690]`), 0 in reverse (`car+0xD82 == 0`).
    ///
    /// **The headlights**, channel 17, are slot 6's easer at 0x43950E…0x439620: a **1000 ms**
    /// travel from the car's headlight bit — which `Car_init` freezes at 0x410C74 as
    /// `word[0x628A28] ? 3 : 0`, so it is the track's night flag and never changes during a race.
    pub fn update_animation(&mut self, r: &mut Renderer, dt_ms: u32, speed_mps: f32, headlights_on: bool, tiny: bool, reverse: bool) {
        // the wing's easer: advance, then (0x43892B) re-arm when the gate holds and it is done
        let e = &mut self.spoiler;
        if e.elapsed < e.duration {
            e.elapsed = e.elapsed.saturating_add(dt_ms);
        }
        let len = self.anim_length[2] as f32;
        let want = if speed_mps > 22.222 {
            Some(len)
        } else if speed_mps < 13.889 {
            Some(0.0)
        } else {
            None
        };
        if let Some(want) = want
            && e.elapsed >= e.duration
        {
            // 0x438970…0x4389CC: the travel's current value becomes the new start
            e.from = e.to;
            e.to = want;
            e.elapsed = 0;
            e.duration = 2000;
        }
        let v = if e.duration == 0 || e.elapsed >= e.duration {
            e.to
        } else {
            e.from + (e.to - e.from) * (e.elapsed as f32 / e.duration as f32)
        };
        self.set_anim(r, 2, v);
        if tiny {
            let t = if reverse { 0.0 } else { (speed_mps * 0.067).min(1.0) };
            self.set_anim(r, 7, t);
        }
        if self.headlight_extent != 0.0 {
            let e = &mut self.headlight;
            let want = if headlights_on { 1.0 } else { 0.0 };
            if e.elapsed < e.duration {
                e.elapsed = e.elapsed.saturating_add(dt_ms);
            } else if e.to != want {
                e.from = e.to;
                e.to = want;
                e.elapsed = 0;
                e.duration = 1000;
            }
            let t = if e.duration == 0 { 1.0 } else { (e.elapsed as f32 / e.duration as f32).min(1.0) };
            let v = e.from + (e.to - e.from) * t;
            let x = self.headlight_extent;
            self.set_channel(17, v * x);
        }
    }

    /// Slot 9's driver half (0x438B2C…0x439352, the race only): the right arm (animation 4), the
    /// left arm (5) and the body (6), `CarModel`'s eight keyframe players, and `R3DCar+0x564` /
    /// `+0x566`. `RE/notes/car-model.md` "Slot 9" has the whole of it.
    pub fn update_driver(&mut self, r: &mut Renderer, dt_ms: u32, d: &DriverInputs) {
        // under detail 1 the driver is posed and nothing else runs (0x438B2C…0x438B5A)
        if d.detail < 1 {
            self.set_anim(r, 4, 4.5);
            self.set_anim(r, 5, 4.5);
            self.set_anim(r, 6, 5.0);
            return;
        }
        self.driver.clock_ms += dt_ms as u64;
        let now = self.driver.clock_ms;
        for p in &mut self.driver.players {
            p.step(now);
        }
        // the steering frame: 23·ch5.z + 4.5 (0x438C4F…0x438C6B), 0…9 as a table index
        // (0x438CC8…0x438D7E: under 0 → 0, over 9 → 9, else fistp(f − 0.5))
        let f = d.wheel_turns.clamp(-4.5 / 23.0, 4.5 / 23.0) * 23.0 + 4.5;
        let index = |f: f32| -> usize {
            if f < 0.0 {
                0
            } else if f > 9.0 {
                9
            } else {
                (f - 0.5).round_ties_even().max(0.0) as usize
            }
        };
        self.set_anim(r, 5, f);
        // a gesture already running is copied into the arm's record (0x438C7A → 0x439035);
        // otherwise the gear change may start one (0x438C92…0x438E18), then the handbrake
        // (0x438E3A…0x438FEA), and on the frame one starts the arm is not written (0x438FF4)
        let arm = self.driver.players[4];
        if arm.active() {
            self.set_anim_keys(r, 4, arm.frame, arm.next, arm.frac);
        } else {
            if d.shift_timer > 0 && d.racing {
                let ms = (d.shift_timer as i64 * 2000 / 32) as u64;
                self.driver.players[4].start(GEAR_GESTURES[index(f)], now, ms);
            }
            if !self.driver.players[4].active() && d.handbrake != self.driver.handbrake {
                self.driver.handbrake = d.handbrake;
                let i = index(f);
                let t: &'static [u32] = if d.handbrake { &HANDBRAKE_ON[i] } else { &HANDBRAKE_OFF[i] };
                self.driver.players[4].start(t, now, 125);
            }
            if !self.driver.players[4].active() {
                self.set_anim(r, 4, if self.driver.handbrake { 19.0 } else { f });
            }
        }
        // the body: 2 − 0.1·car+0xE08, the lateral acceleration's dead band 0.1 (0x439091…0x4390D5)
        let lat = if d.lat_acc.abs() < 0.1 { 0.0 } else { -d.lat_acc };
        let b = 0.1 * lat + 2.0;
        let body = self.driver.players[6];
        if body.active() {
            // 0x4390DA → 0x4392F1
            self.set_anim_keys(r, 6, body.frame, body.next, body.frac);
            return;
        }
        if d.reverse != self.driver.reverse {
            self.driver.reverse = d.reverse;
            let i = if b < 0.0 {
                0
            } else if b > 4.0 {
                4
            } else {
                (b + 0.5) as usize
            };
            let t = if d.reverse { REVERSE_ON[i] } else { REVERSE_OFF[i] };
            self.driver.players[6].start(t, now, 250);
        }
        if self.driver.players[6].active() {
            // 0x439216: started this frame, not written
        } else if self.driver.reverse {
            // looking back over the shoulder (0x43922E…0x439241)
            self.set_anim(r, 6, 7.0);
        } else if d.long_acc <= -0.1 {
            // braking: the body frame toward frame 5 by 0.1·|car+0xE04| (0x43924D…0x4392D9)
            let frame = (b - 0.5).round_ties_even().max(0.0) as u32;
            self.set_anim_keys(r, 6, frame, 5, -0.1 * d.long_acc);
        } else {
            self.set_anim(r, 6, b);
        }
    }

    /// Slot 9's front-end branch for channel **16**, `MoveSpoiler` (0x438751…0x4387CC): it
    /// follows the **roof**, animation 1 (`car+0x57C` frame + `+0x584` fraction = its time `v`,
    /// `+0x588` its length `L`): `v · extent` over the first frame, the full `movespoilerextent`
    /// until the last, `(L − v) · extent` over it. So the 996's and the Boxster's rear lid turns
    /// out of the way while the roof folds, and back.
    pub fn follow_roof(&mut self) {
        if self.move_spoiler_extent == 0.0 || self.anim_length[1] == 0 {
            return;
        }
        let v = self.anim_time[1].max(0.0);
        let l = self.anim_length[1] as f32;
        let e = self.move_spoiler_extent;
        let t = if (0.0..1.0).contains(&v) {
            v * e
        } else if v < l - 1.0 {
            e
        } else {
            (l - v) * e
        };
        self.set_channel(16, t);
    }

    /// The rotation a channel's three floats compose to: `Rx·Ry·Rz`, each angle in **turns**
    /// (0x43C9C0 / 0x43C9D7 / 0x43C9F8, all `× 360` into the axis-angle build 0x539BD0, and
    /// accumulated in that order by the two 0x539AD0 multiplies at 0x43CA26 / 0x43CA38).
    ///
    /// The port only ever fills the x term, so the y and z rotations are identity here; they are
    /// written this way round so a later channel that uses them lands in the right order.
    fn channel_matrix(&self, n: u8) -> Mat4 {
        let t = self.channel_turns.get(n as usize).copied().unwrap_or(0.0);
        let [y, z] = self.channel_yz.get(n as usize).copied().unwrap_or([0.0; 2]);
        if t == 0.0 && y == 0.0 && z == 0.0 {
            return Mat4::IDENTITY;
        }
        // **Channels 6 and 7 are the doors** (0x43CA5A): they turn about the `.tpg` `dooraxis`,
        // normalised (0x5328D0), with its x negated for 7, by the channel's *y* in turns — through
        // 0x5326C0, the same axis-angle build 0x539BD0 uses for the others, so the same
        // convention. The door value is kept in `channel_turns` like the rest.
        if n == 6 || n == 7 {
            let mut a = self.door_axis;
            if n == 7 {
                a.x = -a.x;
            }
            return Mat4::from_axis_angle(a.normalize_or(Vec3::Y), t * std::f32::consts::TAU);
        }
        // Rx·Ry·Rz, in that order (0x43CA26 / 0x43CA38); the y and z senses follow the x one,
        // MEDIUM until the needles and the steering wheel have been seen turning the right way
        use std::f32::consts::TAU;
        Mat4::from_rotation_x(t * TAU) * Mat4::from_rotation_y(y * TAU) * Mat4::from_rotation_z(z * TAU)
    }

    /// The dash needles' calibration from the car's `nfs5.car` record (see `dash`).
    pub fn set_dash(&mut self, tach0: f32, tach1000: f32, speedo0: f32, speedo100: f32) {
        self.dash = [tach0, tach1000, speedo0, speedo100];
    }

    /// Slot 9's race channels (0x438461…0x43874C) other than the wheels' own draw: the steering
    /// wheel, the two needles, the non-spinning wheel parts and the siren.
    ///
    /// `steer` is each front wheel's `+0xB4` over 216 (turns, negated in a mirrored race); `rpm`
    /// is `car+0xDB4`, `speed` `car+0xEC4`; `dt_ms` advances the clock 0x53C2E0 stands for.
    pub fn update_race_channels(&mut self, dt_ms: u32, steer: [f32; 2], spin: [f32; 4], rpm: f32, speed: f32) {
        self.clock_ms += dt_ms as u64;
        // `TIMER_getTicks` (0x53C2E0 → `[0x6B7C7C]`) counts at 128 Hz (`timing.md`)
        let clock = (self.clock_ms * 128 / 1000) as u32;
        let yz = &mut self.channel_yz;
        // 1, 2 `.y` = the fronts' steer; 1…4 `.x` = −the wheel's turn (`+0x88`); `.z` is the
        // wheel's `+0x70`, which this port's suspension does not produce (left at 0)
        for (k, s) in steer.iter().enumerate() {
            yz[1 + k][0] = *s;
        }
        for k in 0..4 {
            self.channel_turns[1 + k] = -spin[k];
        }
        // 11 `.z`: `.TACH0RPM + .TACH1000RPM · rpm · 0.001` (0x43853C…0x438561); 12 `.z`:
        // `.SPEEDO0KM + .SPEEDO100KM · car+0xEC4 · 0.144` (0x43856C…0x438593)
        let [t0, t1, s0, s1] = self.dash;
        yz[11][1] = t0 + t1 * rpm * 0.001;
        yz[12][1] = s0 + s1 * speed * 0.144;
        // 5 `.z`: the steering wheel, `ch1.y + ch2.y` within ±4.5/23 (0x43858F…0x4385FB)
        yz[5][1] = (steer[0] + steer[1]).clamp(-4.5 / 23.0, 4.5 / 23.0);
        // 18…21: channels 1…4 with `.x` = 0 — the tread and the shadow turn but do not spin
        // (0x438669…0x438726)
        for k in 0..4 {
            yz[18 + k] = yz[1 + k];
            self.channel_turns[18 + k] = 0.0;
        }
        // 22 `.y`: 0x53C2E0() · 1/128 — the cop's siren glare, a turn a second (0x43872C…0x43874C)
        yz[22][0] = (clock as f32 * 0.007_812_5).fract();
    }

    /// Channel 22 alone — the one slot 9 channel that reads no car state, only the clock — for the
    /// models every car of a type shares (traffic, the cops): their sirens turn as one, which is
    /// what the original's per-car copies do too.
    pub fn update_clock_channels(&mut self, dt_ms: u32) {
        self.clock_ms += dt_ms as u64;
        let clock = (self.clock_ms * 128 / 1000) as u32;
        self.channel_yz[22][0] = (clock as f32 * 0.007_812_5).fract();
    }

    /// The lids' two easers, 0x439A00 / 0x439AA0 (hood) and 0x439B30 / 0x439BD0 (engine lid):
    /// a travel already running is left alone (0x439A1E); otherwise it restarts from where it
    /// ended toward the extent (`open`) or 0, over **1000 ms** (0x439A81).
    pub fn set_lid(&mut self, hood: bool, open: bool) {
        let (e, target) = if hood {
            (&mut self.hood, if open { self.hood_extent } else { 0.0 })
        } else {
            (&mut self.trunk, if open { self.trunk_extent } else { 0.0 })
        };
        if e.elapsed < e.duration {
            return;
        }
        e.from = e.to;
        e.to = target;
        e.elapsed = 0;
        e.duration = 1000;
    }

    /// Advance the lid easers and write channels 8 and 9 (slot 9, 0x4387CE…0x43891C): the easer's
    /// `from + (to − from)·t`, and `to` once it has run its course. Front-end cars only; the race
    /// never opens a lid.
    pub fn update_lids(&mut self, dt_ms: u32) {
        for (n, e) in [(8u8, self.hood), (9u8, self.trunk)] {
            let v = if e.duration == 0 || e.elapsed >= e.duration {
                e.to
            } else {
                e.from + (e.to - e.from) * (e.elapsed as f32 / e.duration as f32)
            };
            self.set_channel(n, v);
        }
        for e in [&mut self.hood, &mut self.trunk] {
            if e.elapsed < e.duration {
                e.elapsed = e.elapsed.saturating_add(dt_ms);
            }
        }
    }

    /// Set one animation channel, in turns. Channel **16** is `MoveSpoiler`, **17** the pop-up
    /// `HeadLight`; both are `Base+75` bindings confirmed against every shipped model.
    pub fn set_channel(&mut self, n: u8, turns: f32) {
        if let Some(slot) = self.channel_turns.get_mut(n as usize) {
            *slot = turns;
        }
    }

    /// Draw items for a car at `car_to_world` (body frame → world). `wheel_spin` in turns,
    /// `steer` in turns for the front wheels, positive to the right — take it from
    /// `Vehicle::wheel_steer_turns`, not from the raw `Wheel::steer`.
    pub fn draw_items(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2]) -> Vec<DrawItem> {
        let car_to_world = car_to_world * Mat4::from_scale(Vec3::splat(self.scale));
        let mut out = Vec::with_capacity(self.body.len() + 4 * 8);
        let mut keys: Vec<i8> = Vec::with_capacity(self.body.len() + 4 * 8);
        for (i, (mesh, material, local)) in self.body.iter().enumerate() {
            // A part bound to a rotation channel turns about **its own origin**: 0x43CA19…
            // 0x43CA38 compose Rx·Ry·Rz and 0x43CA43 multiplies that into the part's transform,
            // so the rotation is on the right of the part's own matrix.
            let m = match self.body_channel.get(i).copied().unwrap_or(0) {
                0 => *local,
                // 18…21 are channels 1…4 with `.x` cleared (0x438669…0x438726) — the wheel shadow
                // and the parts that turn with the steer but do not spin. Taking them from the
                // caller's `steer` instead of this model's own channels is what lets the traffic
                // and the cops have them: those share one upload per car type.
                n @ 18..=21 if n <= 19 => *local * Mat4::from_rotation_y(steer[(n - 18) as usize] * std::f32::consts::TAU),
                20..=21 => *local,
                // 5, the steering wheel: `.z = ch1.y + ch2.y` within ±4.5/23 (0x43858F…0x4385FB).
                // Off the caller's `steer` for the same reason as 18…21 — the traffic and the cops
                // share one model per car type and would otherwise hold the wheel straight.
                5 => *local * Mat4::from_rotation_z((steer[0] + steer[1]).clamp(-4.5 / 23.0, 4.5 / 23.0) * std::f32::consts::TAU),
                n => *local * self.channel_matrix(n),
            };
            out.push(DrawItem::new(*mesh, *material, car_to_world * MODEL_TO_BODY * m));
            keys.push(self.body_key.get(i).copied().unwrap_or(0));
        }
        for w in &self.wheels {
            let idx = match (w.is_front, w.is_left) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            };
            // The original draws its wheels through channels 1…4 like every other moving part:
            // `.x = −(wheel+0x88)` (0x438600…0x438660), in **model** space, which is the body
            // frame turned half a turn about y — so `+turn` about the body's x. This read
            // `−turn` until 2026-09-22 and rolled the wheels backwards.
            // a wheel article on channels 18…21 takes the steer but **not** the spin: `.x` of those
            // channels is cleared at 0x438669…0x438726
            let spin = if (18..=21).contains(&w.channel) {
                Mat4::IDENTITY
            } else {
                Mat4::from_rotation_x(wheel_spin[idx] * std::f32::consts::TAU)
            };
            // **Positive steer is a right turn** (the right arrow gives +1, and the car yaws right
            // on it), and in the body frame +X is right and +Z forward — so the wheel turns by
            // `+steer`: `from_rotation_y(θ)` carries forward (0,0,1) to (sin θ, 0, cos θ), toward
            // +X. This said `-steer` until 2026-09-21 and drew the front wheels turned the wrong
            // way while the car steered correctly — found in play.
            // `.y` is the wheel's own `+0xB4` over 216 (channels 1 and 2, 0x438461…0x4384EB); a
            // turn about y is the same in both frames
            let st = if w.is_front { Mat4::from_rotation_y(steer[if w.is_left { 0 } else { 1 }] * std::f32::consts::TAU) } else { Mat4::IDENTITY };
            for (mesh, material, local) in &w.parts {
                // local has translation = pivot; rotate about the pivot in model space
                let pivot = local.w_axis.truncate();
                let m = Mat4::from_translation(pivot) * MODEL_TO_BODY * st * spin * MODEL_TO_BODY.inverse() * Mat4::from_translation(-pivot) * *local;
                out.push(DrawItem::new(*mesh, *material, car_to_world * MODEL_TO_BODY * m));
                keys.push(w.key);
            }
        }
        // the parts in `Base+70` order (0x43C1B0's sort). APPROXIMATION within one key: the
        // original's `std::sort` is not stable, the port keeps the file's order there
        let mut order: Vec<usize> = (0..out.len()).collect();
        order.sort_by_key(|&k| keys[k]);
        let mut slots: Vec<Option<DrawItem>> = out.into_iter().map(Some).collect();
        order.into_iter().filter_map(|k| slots[k].take()).collect()
    }
}

/// One mesh of a morphing article: its frame-0 vertices and every frame's positions.
struct MorphPart {
    mesh: MeshId,
    base: Vec<Vertex>,
    frames: Vec<Vec<[f32; 3]>>,
}

fn mm_positions(m: &MeshData) -> Vec<[f32; 3]> {
    m.vertices.iter().map(|v| v.position).collect()
}

impl CarModel {
    /// **0x4D1990(1, state)**: the roof animation to `length · state / 255` frames (0x4D199F…
    /// 0x4D19BA), which 0x4380D0 splits as `frame = fistp(t − 0.5)`, the fraction the rest, and
    /// the draw (0x43E2FD…0x43E35B) blends each vertex `A + (B − A)·frac` between that frame and
    /// the next — the frame alone where there is no next. Past the last frame the part keeps it.
    pub fn set_roof(&mut self, r: &mut Renderer, state: i32) {
        if self.anim_length[1] == 0 || state == self.roof_state {
            return;
        }
        self.roof_state = state;
        let t = self.anim_length[1] as f32 * state as f32 * (1.0 / 255.0);
        self.set_anim(r, 1, t);
    }

    /// `0x438190(n)`: animation `n`'s length in frames, 0 when the car has none.
    pub fn anim_length(&self, n: usize) -> u32 {
        self.anim_length.get(n).copied().unwrap_or(0)
    }

    /// **0x4380D0(n, t)**: animation `n` to time `t` frames, clamped to `[0, length]`.
    pub fn set_anim(&mut self, r: &mut Renderer, n: usize, t: f32) {
        let len = self.anim_length(n);
        if len == 0 {
            return;
        }
        let t = t.clamp(0.0, len as f32);
        if t == self.anim_time[n] {
            return;
        }
        self.anim_time[n] = t;
        let frame = (t - 0.5).round_ties_even().max(0.0) as usize;
        let frac = t - frame as f32;
        self.blend(r, n, frame, frame + 1, frac);
    }

    /// Write animation `n`'s `{frame, next, frac}` straight into its record, as slot 9's keyframe
    /// copies do (0x439035…0x43908D for the right arm, 0x4392F1…0x43933C for the body): the draw
    /// (0x43E2FD…0x43E35B) blends each vertex from `frame` toward `next` by `frac`.
    fn set_anim_keys(&mut self, r: &mut Renderer, n: usize, frame: u32, next: u32, frac: f32) {
        if self.anim_length(n) == 0 {
            return;
        }
        // a key write is not a time: forget the last one so a later 0x4380D0 re-uploads
        self.anim_time[n] = -1.0;
        self.blend(r, n, frame as usize, next as usize, frac);
    }

    fn blend(&mut self, r: &mut Renderer, n: usize, frame: usize, next: usize, frac: f32) {
        if self.blended[n] == Some((frame, next, frac)) {
            return;
        }
        self.blended[n] = Some((frame, next, frac));
        for p in &self.morph[n] {
            let Some(a) = p.frames.get(frame.min(p.frames.len() - 1)) else { continue };
            let b = p.frames.get(next);
            let frac = if frame >= p.frames.len() { 0.0 } else { frac };
            let mut v = p.base.clone();
            for (i, out) in v.iter_mut().enumerate() {
                let pa = Vec3::from(a[i]);
                out.position = match b {
                    Some(b) => (pa + (Vec3::from(b[i]) - pa) * frac).to_array(),
                    None => a[i],
                };
            }
            r.update_mesh_vertices(p.mesh, &v);
        }
    }
}

/// A render object's light block, `obj+0x2F4` (`RE/notes/lighting.md` §6): seven colour slots —
/// slot 3 is the direction — and the material colour at `+0x74` that scales the directional half.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightBlock {
    /// slots 0, 1, 2, (3 unused here), 4, 5, 6 — rgb, the alpha the original carries is 1
    pub colours: [[f32; 3]; 7],
    /// slot 3, the way the light travels, in world space
    pub direction: Vec3,
    /// `+0x74`, rgb
    pub material: [f32; 3],
}

impl LightBlock {
    /// The 3D room's block, 0x4E84C0: with `flag` clear (the room's own models) slots 0–2 are the
    /// rig's ambient and 4–6 its sun colour, `/ 255` and faded, and the direction the rig's sun
    /// direction normalised (0x4E876D…0x4E87D7); with `flag` set (the car, 0x4EA08A / 0x4E8CB2)
    /// they are 0.3 and 0.7 grey, faded, and the direction (0, −0.707, 0.707) (0x4E85FE…). `+0x74`
    /// is white either way (0x4E8513). `fade` is `1 − progress / 512` (0x4E84DE).
    pub fn room(ambient: [f32; 3], sun: [f32; 3], sun_dir: Vec3, fade: f32, flag: bool) -> Self {
        let (a, s, dir) = if flag {
            ([0.3 * fade; 3], [0.7 * fade; 3], Vec3::new(0.0, f32::from_bits(0xbf34fdf4), f32::from_bits(0x3f34fdf4)))
        } else {
            (ambient.map(|c| c * fade / 255.0), sun.map(|c| c * fade / 255.0), sun_dir.normalize_or_zero())
        };
        Self { colours: [a, a, a, [0.0; 3], s, s, s], direction: dir, material: [1.0; 3] }
    }

    /// What `R3DLightLayer` 0x446530 makes of a material's `{p0, p1, slot}`: the slot's colour
    /// × p0, and its `slot + 4` partner × `+0x74` × p1 — nothing directional when `p1 <= 0`
    /// (0x446724). A slot past 2 would read the direction as a colour; none ships.
    pub fn layer_light(&self, l: nfs_formats::crp::LayerLight) -> Option<LayerLight> {
        let s = l.slot as usize;
        if s > 2 {
            return None;
        }
        let a = self.colours[s].map(|c| c * l.p0);
        let d = if l.p1 > 0.0 { [0, 1, 2].map(|k| self.colours[s + 4][k] * self.material[k] * l.p1) } else { [0.0; 3] };
        Some(LayerLight { ambient: a, diffuse: d, direction: self.direction.to_array() })
    }
}

impl CarModel {
    /// `draw_items` lit through each material's `R3DLightLayer` from `block`.
    /// `R3DShadowMapTextureLayer` for this car's items: the projector's planes on every item whose
    /// material carries the layer (0x440920 / 0x440910 set it per car at 0x43A70E).
    pub fn apply_projector(&self, items: &mut [DrawItem], planes: Option<([f32; 4], [f32; 4])>) {
        for it in items {
            it.shadow = if self.shadowed.contains(&it.material) { planes } else { None };
        }
    }

    pub fn draw_items_block(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: [f32; 2], block: &LightBlock) -> Vec<DrawItem> {
        let mut out = self.draw_items(car_to_world, wheel_spin, steer);
        for it in &mut out {
            it.layer_light = self.material_light.get(&it.material).and_then(|l| block.layer_light(*l));
        }
        out
    }
}

#[allow(dead_code)]
fn flat_colour(i: u16) -> [f32; 4] {
    let h = (i as f32 * 0.618_034) % 1.0;
    let (s, v) = (0.45, 0.9);
    let k = (h * 6.0).floor();
    let f = h * 6.0 - k;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match k as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r, g, b, 1.0]
}

/// The track loader's collision objects (0x4750B0 at 0x475152…0x475302): every article of the
/// track's `.crp` whose `Base` byte 0 lacks 0x40 (0x47515A) and whose tag is set, through
/// `AnimDefs::lookup` (0x47ED90 — an unknown tag gets the file's first definition, `ENDW`):
///
/// * `kCylinder` → a cylinder whose radius is the constant **0.25** the loader pushes, with
///   `int(def.radius)` — `_ftol` 0x5A0F98 — where the base keeps the event class `+0x20`
///   (0x4751D5…0x47520B; the arguments are read as the image passes them);
/// * `kBox` → a box on 0x474CB0's footprint of the article's vertices, through its `tr`, class 0
///   (0x475241…0x475302), radius `|(max.x − c.x, 0, max.z − c.z)| × 1.05`.
///
/// The AABB is the article's world-space vertex box at its lowest LOD — MEDIUM: the original takes
/// eight corners from the bundle's box slot, whose frame was not settled.
pub fn track_props(track: &TrackAssets) -> Vec<nfs_physics::props::Prop> {
    use nfs_physics::props::{Breakable, Prop, PropShape, box_footprint};
    let mut out = Vec::new();
    let Ok(arts) = track.crp.articles(&track.crp_data) else { return out };
    let tags = article_tags(track, &arts);
    for (index, a) in arts.iter().enumerate() {
        let Ok(g) = track.crp.article_geometry(&track.crp_data, a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if b.flags & 0x40 != 0 || b.tag_u32() == 0 {
            continue;
        }
        let Some(def) = track.animdefs.lookup(b.tag_u32()) else { continue };
        let Some(lod) = g.primitives.iter().map(|p| p.lod).min() else { continue };
        let tr = g.transform.as_ref().map(Mat4::from_cols_array).unwrap_or(Mat4::IDENTITY);
        let mut skipped = 0;
        let local: Vec<Vec3> = article_meshes(&g, lod, &mut skipped)
            .iter()
            .flat_map(|m| m.mesh.vertices.iter().map(|v| Vec3::from(v.position)))
            .collect();
        if local.is_empty() {
            continue;
        }
        let world: Vec<Vec3> = local.iter().map(|p| tr.transform_point3(*p)).collect();
        let (mn, mx) = world.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| (a.min(*p), b.max(*p)));
        // 0x476380 finds each piece's `QuatGroup` among the instances by tag (0x478640 / 0x478670)
        // and gives up at one whose `Anim` part is missing or has both words 0 (0x476501…0x47651F)
        let breakable = (!def.pieces.is_empty()).then(|| Breakable {
            bands: def.bands.clone(),
            band_pieces: def.band_pieces.clone(),
            resolved: def
                .pieces
                .iter()
                .map(|(_, p)| first_by_tag(&tags, group_tag(&p.quat_group)).and_then(|k| tags[k].1).is_some_and(|w| w != [0, 0]))
                .collect(),
            started: std::cell::Cell::new(None),
            article: index,
        });
        let shape = match def.collide_type.as_str() {
            "kCylinder" => PropShape::Cylinder { radius: 0.25 },
            "kBox" => {
                let corners = box_footprint(&local).map(|c| tr.transform_point3(c));
                let c = (mn + mx) * 0.5;
                let radius = Vec3::new(mx.x - c.x, 0.0, mx.z - c.z).length() * 1.05;
                PropShape::Box { corners, radius }
            }
            _ => continue,
        };
        let class = if matches!(shape, PropShape::Cylinder { .. }) { def.radius as i32 as u32 } else { 0 };
        out.push(Prop { shape, aabb_min: mn, aabb_max: mx, class, breakable, drive_through: def.drive_through });
    }
    out
}

/// Every article's `Base+0x44` tag and its `Anim` part's first two words (the frame count and the
/// vertex-frame count), in `.crp` order — what 0x478640 / 0x478670 search and 0x476380 reads.
fn article_tags(track: &TrackAssets, arts: &[nfs_formats::crp::Article]) -> Vec<(u32, Option<[u32; 2]>)> {
    arts.iter()
        .map(|art| {
            let tag = track.crp.article_geometry(&track.crp_data, art).ok().and_then(|g| g.base.map(|b| b.tag_u32())).unwrap_or(0);
            let anim = art.level(b"Anim").and_then(|e| track.crp.entry_data(&track.crp_data, e).ok()).filter(|d| d.len() >= 8).map(|d| {
                [u32::from_le_bytes([d[0], d[1], d[2], d[3]]), u32::from_le_bytes([d[4], d[5], d[6], d[7]])]
            });
            (tag, anim)
        })
        .collect()
}

/// 0x478640: the first instance whose tag is `tag` (a linear search, `-1` when none).
fn first_by_tag(tags: &[(u32, Option<[u32; 2]>)], tag: u32) -> Option<usize> {
    tags.iter().position(|t| t.0 == tag)
}

/// A piece's group name as the parser keeps it: four characters upper-cased (0x5A262C at 0x47E8D7)
/// packed into the dword the search compares, 0 when the key is absent.
fn group_tag(name: &Option<String>) -> u32 {
    let Some(n) = name else { return 0 };
    let mut b = [0u8; 4];
    for (k, c) in n.bytes().take(4).enumerate() {
        b[k] = c.to_ascii_uppercase();
    }
    u32::from_le_bytes(b)
}

#[cfg(test)]
mod driver_tests {
    use super::*;

    /// 0x470E90 for a one-shot piece: 31 frames at `playbackrate=5` run 1.28 ticks a frame and stop
    /// once frame 29 is reached; the drift grows as `ticks · rate`.
    #[test]
    fn piece_sampler_runs_out_and_drifts() {
        use nfs_formats::crp::{AnimFrame, AnimTrack};
        let frames = (0..31).map(|k| AnimFrame { rotation: [0.0, 0.0, 0.0, 1.0], position: [0.0, k as f32, 0.0], scale: [1.0; 3] }).collect();
        let track = AnimTrack { frames };
        let flags = nfs_formats::animdefs::PieceFlags::default();
        let (m, playing) = sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::X, 0);
        assert!(playing && m.w_axis.truncate().length() < 1e-6);
        // 16 ticks = 12.5 frames: y halfway between 12 and 13, x drift 16 · 1.28
        let (m, playing) = sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::X, 16);
        assert!(playing);
        assert!((m.w_axis.y - 12.5).abs() < 1e-4 && (m.w_axis.x - 16.0 * 1.28).abs() < 1e-3, "{m:?}");
        // 29 frames = 37.12 ticks: the last frame, held, and the player done
        let (m, playing) = sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::ZERO, 38);
        assert!(!playing && (m.w_axis.y - 30.0).abs() < 1e-6);
        assert!(sample_piece(&track, 0.1 / 5.0, flags, 0.0, Vec3::ZERO, 37).1);
    }

    /// 0x438B73…0x438C30: a player walks its table over the duration — frame, next and the
    /// fraction between — and stops, keeping its last frame, once the duration is out.
    #[test]
    fn key_player_walks_its_table() {
        let mut p = KeyPlayer::default();
        p.start(GEAR_GESTURES[0], 1000, 500);
        assert_eq!((p.frame, p.next, p.frac), (0, 10, 0.0));
        // half way: p = 0.5 · 10 = 5 → table[fistp(4.5)] = table[4], table[fistp(5.5)] = table[6]
        p.step(1250);
        assert_eq!((p.frame, p.next), (GEAR_GESTURES[0][4], GEAR_GESTURES[0][6]));
        assert!((p.frac - 1.0).abs() < 1e-6);
        p.step(1499);
        assert!(p.active());
        p.step(1500);
        assert!(!p.active());
        assert_eq!(p.frame, GEAR_GESTURES[0][9]);
    }

    /// Every gesture starts and ends on its steering frame, and the handbrake and reverse tables
    /// meet in the held frame (19 / 7) — the tables as the image has them.
    #[test]
    fn gesture_tables_round_trip() {
        for (i, g) in GEAR_GESTURES.iter().enumerate() {
            assert_eq!((g[0], *g.last().unwrap()), (i as u32, i as u32));
        }
        for i in 0..10 {
            assert_eq!(HANDBRAKE_ON[i][0], i as u32);
            assert_eq!(HANDBRAKE_ON[i][2], 19);
            assert_eq!(HANDBRAKE_OFF[i][0], 19);
            assert_eq!(HANDBRAKE_OFF[i][2], i as u32);
        }
        for i in 0..5 {
            assert_eq!(*REVERSE_ON[i].last().unwrap(), 7);
            assert_eq!(REVERSE_OFF[i][0], 7);
        }
    }
}
