//! Diagnostic (CANYON-SMALLSH, ENHANCEMENT E9 / E51): for a track with a decals' sun, which small upright scenery parts (by texture
//! kind) have their shade already in the data: an `sh80` decal on the sun's side of the part's foot, or a darker ground bake
//! beside it. usage: `canyon_smallsh <track> [max radius m] [rows|summary]`
use glam::{Vec2, Vec3};
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

struct Part {
    kind: String,
    inst: bool,
    lo: Vec3,
    hi: Vec3,
    r: f32,
    up: f32,
    tris: usize,
}

fn luma(c: [f32; 4]) -> f32 {
    0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let track = a.first().map_or("canyon", String::as_str);
    let max_r: f32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(15.0);
    let rows = a.get(2).is_some_and(|s| s == "rows");
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, track).unwrap();
    let mats = t.materials().unwrap();
    let g = t.build_geometry().unwrap();
    let tex = |m: u16| mats.get(m as usize).map_or(String::from("?"), |m| m.texture_name());
    let surf = |m: u16| mats.get(m as usize).map_or(0, |m| m.flags & 0xF);
    let (sun, _) = nfs_game::decal_sun::measure(&g, &mats);
    let sun = sun.expect("a decals' sun");
    let dir = Vec2::new(sun.direction.x, sun.direction.z).normalize();
    let tan_el = sun.elevation.to_radians().tan();
    println!("decals' sun: azimuth {:.1} elevation {:.1} (decals {}, concentration {:.2})", sun.azimuth, sun.elevation, sun.decals, sun.concentration);
    // decal boxes, ground vertices (road = rd*, any ground = surf != 0), parts
    let mut decals: Vec<(Vec3, Vec3)> = Vec::new();
    let mut ground: Vec<(Vec3, f32)> = Vec::new();
    let mut parts: Vec<Part> = Vec::new();
    let add =|kind: String, inst: bool, ps: &[(Vec3, Vec3)], tris: usize, parts: &mut Vec<Part>| {
        if ps.is_empty() {
            return;
        }
        let (lo, hi) = ps.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p.0), h.max(p.0)));
        let c = (lo + hi) * 0.5;
        let r = ps.iter().map(|p| (p.0 - c).length()).fold(0.0, f32::max);
        let up = ps.iter().map(|p| p.1.y.abs()).sum::<f32>() / ps.len() as f32;
        parts.push(Part { kind, inst, lo, hi, r, up, tris });
    };
    for art in &g.world {
        let Some((_, meshes)) = art.lods.first() else { continue };
        let tr = art.transform;
        for m in meshes {
            if m.mesh.vertices.is_empty() {
                continue;
            }
            let name = tex(m.material);
            let ps: Vec<(Vec3, Vec3)> = m.mesh.vertices.iter().map(|v| (tr.transform_point3(Vec3::from(v.position)), tr.transform_vector3(Vec3::from(v.normal)).normalize_or_zero())).collect();
            if name.starts_with("sh80") {
                let (lo, hi) = ps.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p.0), h.max(p.0)));
                decals.push((lo, hi));
            } else if surf(m.material) != 0 {
                ground.extend(m.mesh.vertices.iter().zip(&ps).map(|(v, p)| (p.0, luma(v.color))));
            } else {
                add(name, false, &ps, m.mesh.indices.len() / 3, &mut parts);
            }
        }
    }
    for inst in &g.instances {
        let Some(art) = g.library.iter().find(|l| l.tag == inst.tag) else { continue };
        let Some((_, meshes)) = art.lods.first() else { continue };
        let m = inst.transform * glam::Mat4::from_scale(inst.scale);
        for mm in meshes {
            let ps: Vec<(Vec3, Vec3)> = mm.mesh.vertices.iter().map(|v| (m.transform_point3(Vec3::from(v.position)), m.transform_vector3(Vec3::from(v.normal)).normalize_or_zero())).collect();
            add(format!("inst:{}", tex(mm.material)), true, &ps, mm.mesh.indices.len() / 3, &mut parts);
        }
    }
    // ground grid (5 m cells -> vertex lists)
    let mut grid: std::collections::HashMap<(i32, i32), Vec<(Vec3, f32)>> = Default::default();
    for (p, l) in &ground {
        grid.entry(((p.x / 5.0).floor() as i32, (p.z / 5.0).floor() as i32)).or_default().push((*p, *l));
    }
    let near = |c: Vec2, y: f32, r0: f32, r1: f32| -> Vec<f32> {
        let mut out = Vec::new();
        let (x0, x1, z0, z1) = (((c.x - r1) / 5.0).floor() as i32, ((c.x + r1) / 5.0).floor() as i32, ((c.y - r1) / 5.0).floor() as i32, ((c.y + r1) / 5.0).floor() as i32);
        for ix in x0..=x1 {
            for iz in z0..=z1 {
                for (p, l) in grid.get(&(ix, iz)).into_iter().flatten() {
                    let d = (Vec2::new(p.x, p.z) - c).length();
                    if d >= r0 && d <= r1 && (p.y - y).abs() < 6.0 {
                        out.push(*l);
                    }
                }
            }
        }
        out
    };
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    if rows {
        for (lo, hi) in &decals {
            let c = (*lo + *hi) * 0.5;
            let mut ks: BTreeMap<String, usize> = BTreeMap::new();
            for p in &parts {
                if p.hi.x >= lo.x - 1.5 && p.lo.x <= hi.x + 1.5 && p.hi.z >= lo.z - 1.5 && p.lo.z <= hi.z + 1.5 && p.hi.y >= lo.y - 6.0 && p.lo.y <= hi.y + 6.0 && p.r <= max_r {
                    *ks.entry(p.kind.clone()).or_default() += 1;
                }
            }
            println!("DECAL c {:8.1} {:6.1} {:8.1} size {:5.1} {:4.1} {:5.1} parts inside {:?}", c.x, c.y, c.z, hi.x - lo.x, hi.y - lo.y, hi.z - lo.z, ks);
        }
    }
    println!("parts {} decals {} ground vertices {}", parts.len(), decals.len(), ground.len());
    #[derive(Default)]
    struct K {
        n: usize,
        r: Vec<f32>,
        h: Vec<f32>,
        decal: usize,
        decal_tight: usize,
        bake_dark: usize,
        bake_eval: usize,
        bake_ratio: Vec<f32>,
        own_decal_only: usize,
    }
    let mut kinds: BTreeMap<String, K> = BTreeMap::new();
    for p in &parts {
        let h = p.hi.y - p.lo.y;
        // small and upright-ish: bounding radius up to max_r
        if p.r > max_r {
            continue;
        }
        let foot = Vec2::new((p.lo.x + p.hi.x) * 0.5, (p.lo.z + p.hi.z) * 0.5);
        let len = (h / tan_el).clamp(1.0, 25.0);
        let seg: Vec<Vec2> = (0..=5).map(|i| foot + dir * (len * i as f32 / 5.0)).collect();
        let hit = |tight: bool| {
            decals.iter().any(|(lo, hi)| {
                (!tight || (*hi - *lo).length() <= 20.0)
                    && hi.y >= p.lo.y - 6.0
                    && lo.y <= p.lo.y + 6.0
                    && seg.iter().any(|s| s.x >= lo.x - 1.5 && s.x <= hi.x + 1.5 && s.y >= lo.z - 1.5 && s.y <= hi.z + 1.5)
            })
        };
        let (d, dt) = (hit(false), hit(true));
        // bake: ground luma within 3 m of the shade segment vs the mirrored segment on the sun's side
        let mut zone = Vec::new();
        for s in &seg {
            zone.extend(near(*s, p.lo.y, 0.0, 3.0));
        }
        // the control: the same segment on the sun's side of the foot (the light's source side: no shade of this part there)
        let mut ring = Vec::new();
        for s in &seg {
            ring.extend(near(foot - (*s - foot), p.lo.y, 0.0, 3.0));
        }
        let (mut ratio, mut dark) = (None, false);
        if zone.len() >= 2 && ring.len() >= 2 && mean(&ring) > 0.02 {
            let rr = mean(&zone) / mean(&ring);
            ratio = Some(rr);
            dark = rr < 0.9;
        }
        let k = kinds.entry(p.kind.clone()).or_default();
        k.n += 1;
        k.r.push(p.r);
        k.h.push(h);
        k.decal += usize::from(d);
        k.decal_tight += usize::from(dt);
        if let Some(rr) = ratio {
            k.bake_eval += 1;
            k.bake_dark += usize::from(dark);
            k.bake_ratio.push(rr);
        }
        k.own_decal_only += usize::from(!d && !dark);
        if rows {
            println!("{:12} inst {} r {:5.1} h {:5.1} up {:.2} tris {:4} foot {:8.1} {:6.1} {:8.1} decal {} tight {} bake {:?}", p.kind, u8::from(p.inst), p.r, h, p.up, p.tris, foot.x, p.lo.y, foot.y, u8::from(d), u8::from(dt), ratio);
        }
    }
    let med = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    println!("kind        parts  med_r med_h | decal(any) decal(tight) | bake_eval bake_dark med_ratio | neither");
    for (name, k) in &mut kinds {
        let (mr, mh, mb) = (med(&mut k.r), med(&mut k.h), med(&mut k.bake_ratio));
        println!("{:12} {:5} {:6.1} {:5.1} | {:5} {:5} | {:5} {:5} {:6.2} | {:5}", name, k.n, mr, mh, k.decal, k.decal_tight, k.bake_eval, k.bake_dark, mb, k.own_decal_only);
    }
}
