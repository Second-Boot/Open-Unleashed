//! Diagnostic (SUN-ALL): per track named on the command line (none = every race track), the lights in the data a shadow
//! could be cast with: the sky's sun (the cars' light, 0x43AC8D), the `sh80` decals' sun (`nfs_game::decal_sun`), the light
//! of the baked vertex colours (`nfs_game::bake_light`, with the cast-shadow cue round it), and the sky's weather along the
//! lap (track B: field 5 rain, 6 lightning, 7 snow; their largest value over the lap's slices).
//! Azimuths are `atan2(x, z)` in degrees of the direction the light travels; elevations over the horizon.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::bake_light::{cast_contrast, fit, fit_quantile, travel};

const ALL: [&str; 15] = ["alps", "autobahn", "canyon", "castle", "coastal", "farmland", "foothills", "forest", "skidpad", "industrial", "monaco1", "monaco2", "monaco3", "monaco4", "monaco5"];

fn main() {
    let paths = GamePaths::locate().unwrap();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let names: Vec<String> = if args.is_empty() { ALL.iter().map(|s| s.to_string()).collect() } else { args };
    for name in names {
        let t = TrackAssets::load(&paths, &name).unwrap();
        let g = t.build_geometry().unwrap();
        let m = t.materials().unwrap();
        let sky = t.sky_bin.as_ref().map(|b| {
            let s = b.sun_position();
            let l = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
            (b.sun_active(), (-s[0]).atan2(-s[2]).to_degrees(), (s[1] / l).asin().to_degrees())
        });
        let weather = t.sky_bin.as_ref().map(|b| [5usize, 6, 7].map(|f| (0..4000).map(|s| b.sample_track_b(f, s, 4000)).fold(0.0f32, f32::max)));
        println!("{name}: night {}; sky sun (active, azimuth, elevation) {sky:?}; weather max (rain, lightning, snow) {weather:?}", t.is_night());
        if let Some(b) = t.sky_bin.as_ref().filter(|_| weather.is_some_and(|w| w[0] > 0.0)) {
            // the rain along the lap: its track-B keys (slice, rain)
            let keys: Vec<String> = b.track_b.iter().map(|k| format!("{}:{:.2}", k.0, k.1[5])).collect();
            println!("  rain keys (slice:rain): {}", keys.join(" "));
        }
        let decal = nfs_game::decal_sun::measure(&g, &m).0;
        println!("  decals' sun: {}", decal.map_or("none".to_string(), |d| format!("azimuth {:.0}, {} decals, concentration {:.2}", d.azimuth, d.decals, d.concentration)));
        let Some(f) = fit(&g, &m) else {
            println!("  bake: no scenery bins");
            continue;
        };
        let q = fit_quantile(&g, &m, 0.8);
        println!(
            "  bake light (median bins): azimuth {:.0}, elevation {:.0}, r2 {:.3} (overhead {:.3}), {} bins of {} faces -> {}",
            f.azimuth,
            f.elevation,
            f.r2,
            f.r2_overhead,
            f.bins,
            f.faces,
            if f.accepted().is_some() { "TAKEN" } else { "not directional enough" }
        );
        if let Some(q) = q {
            println!("  bake light (80th-percentile bins): azimuth {:.0}, elevation {:.0}, r2 {:.3} (overhead {:.3})", q.azimuth, q.elevation, q.r2, q.r2_overhead);
        }
        // the cast cue: an azimuth sweep at 35 degrees up, plus the bake's and the sky's own lights
        let mut lights: Vec<(String, glam::Vec3)> = (0..12).map(|k| (format!("{}", -180 + k * 30), travel(-180.0 + k as f32 * 30.0, 35.0))).collect();
        lights.push(("bake".into(), f.direction));
        if let Some((true, a, e)) = sky {
            lights.push(("sky".into(), travel(a, e.max(2.0))));
        }
        let c = cast_contrast(&g, &m, &lights.iter().map(|l| l.1).collect::<Vec<_>>(), 3);
        let sweep: Vec<String> = lights.iter().zip(&c).map(|(l, c)| format!("{} {:.3}", l.0, c.0)).collect();
        println!("  cast cue (lit - shaded ground luma; sweep at 35 deg up, then the bake's, the sky's light): {}", sweep.join(", "));
    }
}
