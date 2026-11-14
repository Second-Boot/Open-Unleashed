//! Diagnostic (CORSICA-SUN): which way the shade baked into a track's ground vertex colours lies from the trees and posts
//! standing on it, i.e. the light direction the track's lighting bake used. For every upright object (a placed library prop, or
//! a non-ground world part 2.5-30 m tall and under 12 m across) the ground vertices round its foot (drivable-ground materials,
//! `mt` surface type `flags & 0xF` != 0, within 1.2 x its height + 4 m and 3 m of its foot's level) are binned by azimuth; each
//! bin's mean luma less the ring's mean is summed over all objects. The darkest bin and the darkness-weighted mean direction say
//! where the bake put the objects' shade; the darkening along that azimuth by distance / height says how long it is.
//! Azimuth convention: atan2(x, z) in degrees of the direction the shade lies (from the object outward), the same as
//! `decal_sun`'s and of the sky sun's travel direction.
use glam::{Vec2, Vec3};
use nfs_assets::{track::TrackAssets, GamePaths};

const BINS: usize = 24;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    for name in &a {
        let t = TrackAssets::load(&paths, name).unwrap();
        let mats = t.materials().unwrap();
        let g = t.build_geometry().unwrap();
        let ground = |i: u16| mats.get(i as usize).is_some_and(|m| m.flags & 0xF != 0);
        let is_sh = |i: u16| mats.get(i as usize).is_some_and(|m| m.texture_name().starts_with("sh80"));
        let bounds = |ps: &mut dyn Iterator<Item = Vec3>| ps.fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(l, h), p| (l.min(p), h.max(p)));
        // ground vertices: (xz, y, luma)
        let mut verts: Vec<(Vec2, f32, f32)> = Vec::new();
        // upright objects: (foot xz, foot y, height)
        let mut objs: Vec<(Vec2, f32, f32)> = Vec::new();
        for art in &g.world {
            let Some((_, meshes)) = art.lods.first() else { continue };
            for m in meshes {
                if m.mesh.vertices.is_empty() || is_sh(m.material) {
                    continue;
                }
                if ground(m.material) {
                    for v in &m.mesh.vertices {
                        let p = art.transform.transform_point3(Vec3::from(v.position));
                        let l = 0.299 * v.color[0] + 0.587 * v.color[1] + 0.114 * v.color[2];
                        verts.push((Vec2::new(p.x, p.z), p.y, l));
                    }
                } else {
                    let (lo, hi) = bounds(&mut m.mesh.vertices.iter().map(|v| art.transform.transform_point3(Vec3::from(v.position))));
                    let (w, h) = ((hi.x - lo.x).max(hi.z - lo.z), hi.y - lo.y);
                    if (2.5..30.0).contains(&h) && w < 12.0 {
                        objs.push((Vec2::new((lo.x + hi.x) * 0.5, (lo.z + hi.z) * 0.5), lo.y, h));
                    }
                }
            }
        }
        for inst in &g.instances {
            let Some(art) = g.library.iter().find(|a| a.tag == inst.tag) else { continue };
            let Some((_, meshes)) = art.lods.first() else { continue };
            let (lo, hi) = bounds(&mut meshes.iter().flat_map(|m| m.mesh.vertices.iter()).map(|v| inst.transform.transform_point3(Vec3::from(v.position))));
            let (w, h) = ((hi.x - lo.x).max(hi.z - lo.z), hi.y - lo.y);
            if lo.x <= hi.x && (2.5..30.0).contains(&h) && w < 12.0 {
                objs.push((Vec2::new((lo.x + hi.x) * 0.5, (lo.z + hi.z) * 0.5), lo.y, h));
            }
        }
        // a coarse grid of the ground vertices for the ring queries
        let cell = 16.0f32;
        let mut grid: std::collections::HashMap<(i32, i32), Vec<usize>> = std::collections::HashMap::new();
        for (i, v) in verts.iter().enumerate() {
            grid.entry(((v.0.x / cell).floor() as i32, (v.0.y / cell).floor() as i32)).or_default().push(i);
        }
        let mut sum = [0.0f64; BINS];
        let mut cnt = [0u32; BINS];
        // along-azimuth profile (filled in a second pass): darkening by distance / height, 0..3 in steps of 0.25
        let mut used = Vec::new();
        for o in &objs {
            let reach = 1.2 * o.2 + 4.0;
            let (cx, cz) = ((o.0.x / cell).floor() as i32, (o.0.y / cell).floor() as i32);
            let k = (reach / cell).ceil() as i32;
            let mut ring: Vec<(f32, f32, f32)> = Vec::new(); // (azimuth deg, d / h, luma)
            for gx in cx - k..=cx + k {
                for gz in cz - k..=cz + k {
                    let Some(list) = grid.get(&(gx, gz)) else { continue };
                    for &i in list {
                        let v = verts[i];
                        let d = v.0 - o.0;
                        let r = d.length();
                        if r < 1.5 || r > reach || (v.1 - o.1).abs() > 3.0 {
                            continue;
                        }
                        ring.push((d.x.atan2(d.y).to_degrees(), r / o.2, v.2));
                    }
                }
            }
            if ring.len() < 12 {
                continue;
            }
            let mean = ring.iter().map(|r| r.2).sum::<f32>() / ring.len() as f32;
            let mut s = [0.0f64; BINS];
            let mut c = [0u32; BINS];
            for r in &ring {
                let b = (((r.0 + 360.0) % 360.0) / (360.0 / BINS as f32)) as usize % BINS;
                s[b] += f64::from(r.2 - mean);
                c[b] += 1;
            }
            for b in 0..BINS {
                if c[b] > 0 {
                    sum[b] += s[b] / f64::from(c[b]);
                    cnt[b] += 1;
                }
            }
            used.push((o, ring, mean));
        }
        let mut dir = Vec2::ZERO;
        let mut tot = 0.0;
        let means: Vec<f64> = (0..BINS).map(|b| if cnt[b] > 0 { sum[b] / f64::from(cnt[b]) } else { 0.0 }).collect();
        for (b, m) in means.iter().enumerate() {
            let az = ((b as f32 + 0.5) * 360.0 / BINS as f32).to_radians();
            dir += Vec2::new(az.sin(), az.cos()) * (-*m as f32);
            tot += m.abs() as f32;
        }
        let (darkest, _) = means.iter().enumerate().fold((0, f64::MAX), |acc, (b, m)| if *m < acc.1 { (b, *m) } else { acc });
        let az_mean = dir.x.atan2(dir.y).to_degrees();
        println!(
            "{name}: {} ground verts, {} upright objects, {} with a ring; darkest bin az {:.0} deg; weighted mean az {az_mean:.0} deg, concentration {:.2}",
            verts.len(),
            objs.len(),
            used.len(),
            (darkest as f32 + 0.5) * 360.0 / BINS as f32,
            dir.length() / tot.max(1e-6)
        );
        println!("{name}: per-bin luma vs ring mean x1000: {}", means.iter().map(|m| format!("{:.0}", m * 1000.0)).collect::<Vec<_>>().join(" "));
        // the profile along the mean azimuth (+-30 deg) vs the opposite side, by distance / height
        let mut prof = [[0.0f64; 2]; 12];
        let mut pc = [[0u32; 2]; 12];
        for (_, ring, mean) in &used {
            for r in ring {
                let da = ((r.0 - az_mean + 540.0) % 360.0) - 180.0;
                let side = if da.abs() < 30.0 { 0 } else if da.abs() > 150.0 { 1 } else { continue };
                let k = ((r.1 / 0.25) as usize).min(11);
                prof[k][side] += f64::from(r.2 - mean);
                pc[k][side] += 1;
            }
        }
        println!(
            "{name}: along the mean az / opposite, by distance/height (0.25 steps) x1000: {}",
            (0..12).map(|k| format!("{:.0}/{:.0}", prof[k][0] * 1000.0 / f64::from(pc[k][0].max(1)), prof[k][1] * 1000.0 / f64::from(pc[k][1].max(1)))).collect::<Vec<_>>().join(" ")
        );
        if let Ok(d) = std::fs::read(paths.root.join(format!("GameData/Track/Sky/{name}.bin"))) {
            if let Ok(b) = nfs_formats::sky::SkyBin::parse(&d) {
                let s = b.sun_position();
                let l = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
                println!("{name}: sky sun active {} -> travel azimuth {:.0} deg, elevation {:.0} deg", b.sun_active(), (-s[0]).atan2(-s[2]).to_degrees(), (s[1] / l).asin().to_degrees());
            }
        }
    }
}
