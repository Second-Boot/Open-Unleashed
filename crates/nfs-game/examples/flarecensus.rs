//! Diagnostic: every kind-2 flare id of a placed article on a track, with its `flare.dat` record, count, height over the
//! nearest road slice and lateral offset.
use nfs_assets::{track::TrackAssets, GamePaths};
use std::collections::BTreeMap;

fn main() {
    let track = std::env::args().nth(1).unwrap_or_else(|| "autobahn".into());
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &track).unwrap();
    let defs = nfs_formats::flare::parse(&std::fs::read(paths.render_dir().join("flare.dat")).unwrap());
    let slices = nfs_game::race::SliceTable::from_crp(&t.crp, &t.crp_data).unwrap();
    let flares = nfs_game::world::track_flares(&t);
    let mut by: BTreeMap<String, Vec<(f32, f32, usize)>> = BTreeMap::new();
    for f in &flares {
        let (i, s) = slices.slices.iter().enumerate().min_by(|a, b| a.1.centre.distance_squared(f.position).total_cmp(&b.1.centre.distance_squared(f.position))).unwrap();
        let side = s.forward.cross(glam::Vec3::Y).normalize_or_zero();
        let d = f.position - s.centre;
        by.entry(String::from_utf8_lossy(&f.id.to_le_bytes()).into_owned()).or_default().push((d.y, d.dot(side), i));
    }
    println!("{track}: night {} flares {}", t.is_night(), flares.len());
    {
        let geometry = t.build_geometry().unwrap();
        let materials = t.materials().unwrap();
        let flags: Vec<u32> = materials.iter().map(|m| m.flags).collect();
        let grid = geometry.collision_grid(20.0, &|mat| flags.get(mat as usize).copied().unwrap_or(0));
        let ml = nfs_game::world::MapLight::from_map(&t.map);
        let mut gs = Vec::new();
        for s in &slices.slices {
            if let Some(h) = grid.ground(s.centre.x, s.centre.y + 2.0, s.centre.z, 20.0) {
                gs.push(0.2126 * h.color[0] + 0.7152 * h.color[1] + 0.0722 * h.color[2]);
            }
        }
        // per classified flare: is its nearest slice covered (the slice's enclosure test), the ceiling over the slice, and the
        // flare's height / lateral offset there
        if std::env::var_os("CENSUS_COVER").is_some() {
            let cov: Vec<bool> = slices
                .slices
                .iter()
                .map(|s| {
                    let p = s.centre + s.up * 0.5;
                    grid.ceiling(p.x, p.y, p.z, 3.5, 200.0).is_some_and(|h| h.surface & 0xC000 != 0)
                })
                .collect();
            let mut runs = Vec::new();
            let mut k = 0;
            while k < cov.len() {
                if cov[k] {
                    let a = k;
                    while k + 1 < cov.len() && cov[k + 1] {
                        k += 1;
                    }
                    runs.push(format!("{a}-{k}"));
                }
                k += 1;
            }
            println!("  covered runs {}", runs.join(" "));
            for f in &flares {
                let id = String::from_utf8_lossy(&f.id.to_le_bytes()).into_owned();
                if !["ypml", "pmal", "yrts", "grts", "ydlb", "onut", "bnut", "ynut", "tnac", "thws", "rleh"].contains(&id.as_str()) {
                    continue;
                }
                let (i, s) = slices.slices.iter().enumerate().min_by(|a, b| a.1.centre.distance_squared(f.position).total_cmp(&b.1.centre.distance_squared(f.position))).unwrap();
                let p = s.centre + s.up * 0.5;
                let c = grid.ceiling(p.x, p.y, p.z, 3.5, 200.0);
                let cov = c.as_ref().is_some_and(|h| h.surface & 0xC000 != 0);
                let ceil = c.map(|h| (h.point - s.centre).dot(s.up));
                let side = s.forward.cross(glam::Vec3::Y).normalize_or_zero();
                let d = f.position - s.centre;
                let fc = grid.ceiling(f.position.x, f.position.y - 0.3, f.position.z, 0.1, 50.0).map(|h| (h.surface & 0xC000, h.point.y - f.position.y));
                println!("  {id} slice {i} covered {cov} ceil {ceil:?} h {:.1} lat {:.1} over-lamp {fc:?}", d.y, d.dot(side));
            }
        }
        gs.sort_by(f32::total_cmp);
        let mean = gs.iter().sum::<f32>() / gs.len().max(1) as f32;
        let med = gs.get(gs.len() / 2).copied().unwrap_or(0.0);
        println!("  road patch luma mean {mean:.3} median {med:.3} p10 {:.3} p90 {:.3}; x tint {:.3}, x ambient {:.3}", gs[gs.len() / 10], gs[gs.len() * 9 / 10], med * ml.tint[0], med * ml.ambient[0]);
    }
    if let Some(b) = t.sky_bin.as_ref() {
        let lap = slices.slices.len() as i32;
        for level in 0..4 {
            let mut v = Vec::new();
            for s in (0..lap).step_by((lap as usize / 12).max(1)) {
                if let Some(c) = b.background_at(level, 0, s, lap) {
                    let lum = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                    v.push(format!("{:.2}", (1.25 - 1.5 * lum).clamp(0.1, 1.0)));
                }
            }
            println!("  sky level {level} dark {}", v.join(" "));
        }
    }
    for (id, v) in &by {
        let def = defs.iter().find(|d| String::from_utf8_lossy(&d.id.to_le_bytes()) == *id);
        let (hmin, hmax) = v.iter().fold((f32::MAX, f32::MIN), |a, x| (a.0.min(x.0), a.1.max(x.0)));
        let (lmin, lmax) = v.iter().fold((f32::MAX, f32::MIN), |a, x| (a.0.min(x.1.abs()), a.1.max(x.1.abs())));
        let mut sl: Vec<usize> = v.iter().map(|x| x.2).collect();
        sl.sort();
        println!(
            "{id} n {:4} h {hmin:6.1}..{hmax:6.1} lat {lmin:5.1}..{lmax:5.1} slices {}..{} rec {:?}",
            v.len(),
            sl[0],
            sl[sl.len() - 1],
            def.map(|d| (d.textures.clone(), format!("{:08X} {:08X}", d.colours[0], d.colours[1]), d.sizes, d.flags, d.on, d.off))
        );
    }
}
