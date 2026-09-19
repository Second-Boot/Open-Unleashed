//! Assembling renderer scenes from loaded assets (shared by the game app and the viewer tools).

use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, track::TrackAssets};
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshData, MeshId, Renderer, TextureData, Vertex};
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
    pub walls: nfs_physics::walls::WallSet,
    pub start: Vec3,
    pub ambient: f32,
    /// The `.map` header's five light colours (`RE/notes/lighting.md`).
    pub map_light: MapLight,
    /// Mean vertex colour of the drivable patches, the reference `light_at` normalises against.
    mean_ground: [f32; 3],
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
    /// The light a car standing at `pos` picks up: the vertex colour of the ground patch under it
    /// (`RE/notes/lighting.md`, 0x4746A0). White when there is no ground under it.
    ///
    /// The `.map` header's `ambient`, which the original multiplies this by for the car's ambient
    /// slot, is already the scene's ambient level (`TrackWorld::ambient`), so the renderer applies
    /// it once, to the ambient term only.
    pub fn light_at(&self, pos: Vec3) -> [f32; 3] {
        let Some(h) = self.collision.ground(pos.x, pos.y + 2.0, pos.z, 20.0) else { return [1.0; 3] };
        // APPROXIMATION: the original's ambient slot is `patch colour × map.ambient` feeding a D3D
        // light, where the port's shader splits a fixed ambient/diffuse ratio. Taking the patch
        // colour relative to the track's own mean keeps the port's exposure and still gives a car
        // the hue and the shading of the ground it is on.
        let m = self.mean_ground;
        let f = |c: f32, m: f32| if m > 0.01 { (c / m).clamp(0.0, 2.0) } else { 1.0 };
        [f(h.color[0], m[0]), f(h.color[1], m[1]), f(h.color[2], m[2])]
    }

    /// The environment map a car reflects at `slice`: the `.map` section covering it, layer 0, in
    /// its covered or open variant (`RE/notes/lighting.md` §3). `None` outside every section, where
    /// the original leaves the car on the default maps.
    pub fn env_for(&self, slice: u32, covered: bool) -> Option<nfs_renderer::TextureId> {
        let sec = self.map_sections.iter().find(|s| slice >= s.start && slice <= s.end)?;
        self.env_by_id.get(&sec.ids[3 * covered as usize]).copied()
    }

    /// The original's enclosure test (0x473330, `race-logic.md` §12): the flags word of the lowest
    /// track patch more than **3.5 m** above the point, or 0 when there is none. `Tracker` patches
    /// carry the CRP `mt` flags word at `patch+0xA` (`suspension-collision.md` §7), which is the
    /// same word the port keeps in the low 16 bits of a collision triangle's surface id.
    pub fn overhead_flags(&self, pos: Vec3) -> u16 {
        self.collision.ceiling(pos.x, pos.y, pos.z, 3.5, 200.0).map(|h| (h.surface & 0xFFFF) as u16).unwrap_or(0)
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
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back: true, depth_test: true, env_texture: None, env_mix: 0.0 })
            })
        };
        // Static world: merge every article's meshes per material into one GPU mesh each
        // (transforms baked on the CPU) — 4k draw calls → ~200.
        let mut merged: HashMap<u16, nfs_renderer::MeshData> = HashMap::new();
        for art in &geometry.world {
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
        // the drivable patches only (surface nibble != 0), as `Tracker::findPatch` does
        let mean_ground = collision.mean_color(&|s| s & 0xF != 0);
        let map_sections = MapSection::all(&track.map);
        Ok(Self { items, item_element, hidden: std::collections::HashSet::new(), env_map, collision, walls, start, ambient, map_light, mean_ground, map_sections, env_by_id, knocked: Vec::new(), animated })
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
        self.items.iter().zip(self.item_element.iter()).filter(|(_, e)| e.map(|e| !self.hidden.contains(&e)).unwrap_or(true)).map(|(i, _)| *i)
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

/// GPU-resident car model at one LOD, split into body parts (with their pivots applied) and
/// wheels (kept separate so they can spin/steer). Model space (RE/notes/car-model.md §3) is
/// +Y up, −Z front, +X left; `MODEL_TO_BODY` rotates it into the physics body frame (+Z forward).
pub struct CarModel {
    pub body: Vec<(MeshId, MaterialId, Mat4)>,
    /// (pivot in body space, is_front, is_left, parts)
    pub wheels: Vec<CarWheelModel>,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub wheel_radius: f32,
    pub half_track: f32,
    /// Where the car's lamps sit in the body frame and which way they face: the `Light1` (0x13,
    /// headlights), `Light3` (0x15, front indicators) and `Light2` (0x14, tail lights) articles
    /// (`RE/notes/car-model.md` §3). The original hangs a glare sprite on each
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

#[derive(Debug, Clone, Copy)]
pub struct CarLight {
    /// centre of the lamp's geometry, in the body frame
    pub position: Vec3,
    /// +1 for a lamp that faces the car's front, −1 for the tail lights
    pub facing: f32,
}

pub struct CarWheelModel {
    pub pivot: Vec3,
    pub is_front: bool,
    pub is_left: bool,
    pub parts: Vec<(MeshId, MaterialId, Mat4)>,
}

/// The car's collision body points in the physics body frame, `y = 0` on the wheel contact plane
/// (`BndP`, `RE/notes/car-model.md` §12), sorted ascending by height the way 0x49ABC0 does — so the
/// first four are the wheel contact set. Only the records every car carries are taken: a record
/// naming a style slot is part of the body only when that slot holds its value, and the port does
/// not track the style array yet, so spoilers and wings are left out.
pub fn car_body_points(car: &CarAssets) -> Vec<Vec3> {
    let Ok(recs) = car.crp.bound_points(&car.crp_data) else { return Vec::new() };
    let mut pts: Vec<Vec3> = recs
        .iter()
        .filter(|r| r.style_slot == 0 && r.style_value == 0)
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
    pub fn upload(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: [u8; 3]) -> anyhow::Result<Self> {
        Self::upload_with_env(r, car, paths, lod, paint, None, false)
    }

    /// `env`: track environment map (`.env` entry, e.g. "SKY ") for `CarExtEnv` paint reflections.
    pub fn upload_with_env(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: [u8; 3], env: Option<nfs_renderer::TextureId>, night: bool) -> anyhow::Result<Self> {
        let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, 1);
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
        let mut material_for = |r: &mut Renderer, id: u16| {
            *mat_ids.entry(id).or_insert_with(|| {
                let m = materials.get(id as usize);
                let page = m.map(|m| m.page_index() as usize).and_then(|p| page_tex.get(p));
                let blend = m.map(|m| matches!(m.render_method.as_str(), "CarWindow" | "CarWheel")).unwrap_or(false) || page.map(|p| p.1).unwrap_or(false);
                let reflective = m.map(|m| m.render_method == "CarExtEnv").unwrap_or(false);
                r.create_material(MaterialDesc { texture: page.map(|p| p.0), alpha_blend: blend, cull_back: !blend, depth_test: true, env_texture: if reflective { env } else { None }, env_mix: 0.3 })
            })
        };
        let arts = car.crp.articles(&car.crp_data)?;
        let mut body = Vec::new();
        let mut wheels = Vec::new();
        let mut skipped = 0;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        let mut lights: Vec<CarLight> = Vec::new();
        let mut wheel_radius: f32 = 0.3;
        let mut half_track: f32 = 0.7;
        for a in &arts {
            let g = car.crp.article_geometry(&car.crp_data, a)?;
            if !g.primitives.iter().any(|p| p.lod == lod) {
                continue;
            }
            let (ty, variant) = g.base.as_ref().map(|b| (b.tag[0], b.tag[1])).unwrap_or((0, 0));
            if !style.article_enabled(ty, variant) {
                continue;
            }
            let is_wheel = matches!(ty, 0x0D | 0x0E);
            let is_light = matches!(ty, 0x13 | 0x14 | 0x15);
            let local = g.transform_for_lod(lod).map(|m| Mat4::from_cols_array(&m)).unwrap_or(Mat4::IDENTITY);
            let mut parts = Vec::new();
            let (mut lamp_sum, mut lamp_n) = (Vec3::ZERO, 0.0f32);
            for mm in article_meshes(&g, lod, &mut skipped) {
                let mut mesh = mm.mesh;
                for v in &mut mesh.vertices {
                    let p = (MODEL_TO_BODY * local).transform_point3(Vec3::from(v.position));
                    if is_light {
                        lamp_sum += p;
                        lamp_n += 1.0;
                    }
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
                parts.push((r.upload_mesh(&mesh), material, local));
            }
            if is_light && lamp_n > 0.0 {
                // 0x13 headlights and 0x15 front indicators face the front, 0x14 tail lights the back
                lights.push(CarLight { position: lamp_sum / lamp_n, facing: if ty == 0x14 { -1.0 } else { 1.0 } });
            }
            if is_wheel {
                let pivot = MODEL_TO_BODY.transform_point3(local.w_axis.truncate());
                half_track = half_track.max(pivot.x.abs());
                wheels.push(CarWheelModel { pivot, is_front: ty == 0x0D, is_left: local.w_axis.x > 0.0, parts });
            } else {
                body.extend(parts);
            }
        }
        tracing::info!(body_parts = body.len(), wheels = wheels.len(), lights = lights.len(), skipped, pages = pages.len(), wheel_radius, half_track, "car uploaded");
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
            let mat = r.create_material(MaterialDesc { texture: Some(tex), alpha_blend: true, cull_back: false, depth_test: true, env_texture: None, env_mix: 0.0 });
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
            body,
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

    /// Draw items for a car at `car_to_world` (body frame → world). `wheel_spin` in turns,
    /// `steer` in turns for the front wheels.
    pub fn draw_items(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: f32) -> Vec<DrawItem> {
        self.draw_items_lit(car_to_world, wheel_spin, steer, [1.0; 3])
    }

    /// `draw_items` with the light the car picks up from the ground (`TrackWorld::light_at`).
    pub fn draw_items_lit(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: f32, light: [f32; 3]) -> Vec<DrawItem> {
        let mut out = Vec::with_capacity(self.body.len() + 4 * 8);
        for (mesh, material, local) in &self.body {
            out.push(DrawItem { light, ..DrawItem::new(*mesh, *material, car_to_world * MODEL_TO_BODY * *local) });
        }
        for w in &self.wheels {
            let idx = match (w.is_front, w.is_left) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            };
            let spin = Mat4::from_rotation_x(-wheel_spin[idx] * std::f32::consts::TAU);
            let st = if w.is_front { Mat4::from_rotation_y(-steer * std::f32::consts::TAU) } else { Mat4::IDENTITY };
            for (mesh, material, local) in &w.parts {
                // local has translation = pivot; rotate about the pivot in model space
                let pivot = local.w_axis.truncate();
                let m = Mat4::from_translation(pivot) * MODEL_TO_BODY * st * spin * MODEL_TO_BODY.inverse() * Mat4::from_translation(-pivot) * *local;
                out.push(DrawItem { light, ..DrawItem::new(*mesh, *material, car_to_world * MODEL_TO_BODY * m) });
            }
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
