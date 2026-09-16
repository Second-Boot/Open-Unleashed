//! Assembling renderer scenes from loaded assets (shared by the game app and the viewer tools).

use glam::{Mat4, Vec3};
use nfs_assets::{car::CarAssets, mesh::article_meshes, track::TrackAssets};
use nfs_renderer::{DrawItem, MaterialDesc, MaterialId, MeshId, Renderer, TextureData};
use std::collections::HashMap;

/// GPU-resident track: static draw items plus the collision grid.
pub struct TrackWorld {
    pub items: Vec<DrawItem>,
    pub collision: nfs_physics::collision::TriangleGrid,
    pub walls: nfs_physics::walls::WallSet,
    pub start: Vec3,
    pub ambient: f32,
}

impl TrackWorld {
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
                r.create_material(MaterialDesc { texture: tex.map(|t| t.0), alpha_blend: alpha, cull_back: true, depth_test: true })
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
            items.push(DrawItem { mesh: r.upload_mesh(&merged[&m]), material, transform: Mat4::IDENTITY });
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
        for inst in &geometry.instances {
            if let Some(parts) = lib_gpu.get(&inst.tag) {
                let lib_tr = geometry.library.iter().find(|a| a.tag == inst.tag).map(|a| a.transform).unwrap_or(Mat4::IDENTITY);
                for (mesh, material) in parts {
                    items.push(DrawItem { mesh: *mesh, material: *material, transform: inst.transform * lib_tr });
                }
            }
        }
        let collision = geometry.collision_grid(20.0);
        let walls = nfs_physics::walls::WallSet::build(track.edges.iter().flat_map(|e| e.segments.iter().map(|s| (s.p1, s.p2))), 30.0);
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
        Ok(Self { items, collision, walls, start, ambient })
    }
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
}

pub struct CarWheelModel {
    pub pivot: Vec3,
    pub is_front: bool,
    pub is_left: bool,
    pub parts: Vec<(MeshId, MaterialId, Mat4)>,
}

pub const MODEL_TO_BODY: Mat4 = Mat4::from_cols(
    glam::Vec4::new(-1.0, 0.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
    glam::Vec4::new(0.0, 0.0, -1.0, 0.0),
    glam::Vec4::new(0.0, 0.0, 0.0, 1.0),
);

impl CarModel {
    pub fn upload(r: &mut Renderer, car: &CarAssets, paths: &nfs_assets::GamePaths, lod: u8, paint: [u8; 3]) -> anyhow::Result<Self> {
        let style = nfs_assets::car::CarStyle::from_tpg(&car.tpg, 1);
        tracing::info!(style = style.name, "car style");
        let pages = car.build_pages(paths, paint, &style)?;
        let page_tex: Vec<(nfs_renderer::TextureId, bool)> = pages
            .iter()
            .map(|p| (r.upload_texture(&TextureData::single(p.width, p.height, p.rgba.clone())), p.is_window || p.is_alpha))
            .collect();
        let materials = car.crp.materials(&car.crp_data)?;
        let mut mat_ids: HashMap<u16, MaterialId> = HashMap::new();
        let mut material_for = |r: &mut Renderer, id: u16| {
            *mat_ids.entry(id).or_insert_with(|| {
                let m = materials.get(id as usize);
                let page = m.map(|m| m.page_index() as usize).and_then(|p| page_tex.get(p));
                let blend = m.map(|m| matches!(m.render_method.as_str(), "CarWindow" | "CarWheel")).unwrap_or(false) || page.map(|p| p.1).unwrap_or(false);
                r.create_material(MaterialDesc { texture: page.map(|p| p.0), alpha_blend: blend, cull_back: !blend, depth_test: true })
            })
        };
        let arts = car.crp.articles(&car.crp_data)?;
        let mut body = Vec::new();
        let mut wheels = Vec::new();
        let mut skipped = 0;
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
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
            let local = g.transform_for_lod(lod).map(|m| Mat4::from_cols_array(&m)).unwrap_or(Mat4::IDENTITY);
            let mut parts = Vec::new();
            for mm in article_meshes(&g, lod, &mut skipped) {
                let mut mesh = mm.mesh;
                for v in &mut mesh.vertices {
                    let p = (MODEL_TO_BODY * local).transform_point3(Vec3::from(v.position));
                    if !is_wheel {
                        min = min.min(p);
                        max = max.max(p);
                    } else {
                        let lp = Vec3::from(v.position);
                        wheel_radius = wheel_radius.max((lp.y * lp.y + lp.z * lp.z).sqrt());
                    }
                }
                let material = material_for(r, mm.material);
                parts.push((r.upload_mesh(&mesh), material, local));
            }
            if is_wheel {
                let pivot = MODEL_TO_BODY.transform_point3(local.w_axis.truncate());
                half_track = half_track.max(pivot.x.abs());
                wheels.push(CarWheelModel { pivot, is_front: ty == 0x0D, is_left: local.w_axis.x > 0.0, parts });
            } else {
                body.extend(parts);
            }
        }
        tracing::info!(body_parts = body.len(), wheels = wheels.len(), skipped, pages = pages.len(), wheel_radius, half_track, "car uploaded");
        Ok(Self { body, wheels, bbox_min: min, bbox_max: max, wheel_radius, half_track })
    }

    /// Draw items for a car at `car_to_world` (body frame → world). `wheel_spin` in turns,
    /// `steer` in turns for the front wheels.
    pub fn draw_items(&self, car_to_world: Mat4, wheel_spin: [f32; 4], steer: f32) -> Vec<DrawItem> {
        let mut out = Vec::with_capacity(self.body.len() + 4 * 8);
        for (mesh, material, local) in &self.body {
            out.push(DrawItem { mesh: *mesh, material: *material, transform: car_to_world * MODEL_TO_BODY * *local });
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
                out.push(DrawItem { mesh: *mesh, material: *material, transform: car_to_world * MODEL_TO_BODY * m });
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
