//! Diagnostic (CORSICA-SUN): the sun a track's `sh80` ground-shadow decals were drawn for (`nfs_game::decal_sun::measure`, the
//! rule E51 uses on a track whose sky has no sun), next to the sky's own sun, per track named on the command line.
//! Azimuths are `atan2(x, z)` in degrees of the direction the shade / the light travels.
use nfs_assets::{track::TrackAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for name in std::env::args().skip(1) {
        let t = TrackAssets::load(&paths, &name).unwrap();
        let (sun, per) = nfs_game::decal_sun::measure(&t.build_geometry().unwrap(), &t.materials().unwrap());
        let mut hist = [0u32; 12];
        for (az, _) in &per {
            hist[(((az + 180.0) / 30.0) as usize).min(11)] += 1;
        }
        println!("{name}: {} decals paired; azimuth histogram (30-degree bins from -180): {hist:?}", per.len());
        let mut r: Vec<f32> = per.iter().map(|p| p.1).collect();
        r.sort_by(f32::total_cmp);
        println!("{name}: offset / height sorted: {:.2?}", r);
        println!("{name}: decal sun {sun:?}");
        if let Some(b) = t.sky_bin.as_ref() {
            let s = b.sun_position();
            let l = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
            println!("{name}: sky sun active {} -> travel azimuth {:.0} deg, elevation {:.0} deg", b.sun_active(), (-s[0]).atan2(-s[2]).to_degrees(), (s[1] / l).asin().to_degrees());
        }
    }
}
