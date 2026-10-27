//! Diagnostic: per sky `.bin`, where the sun sits (region A dome offset, scale, ring), whether the sun object is
//! on (`SkyBin::sun_active`), the car light direction the race takes from it (0x43AC8D) and the weather fields.
fn main() {
    for p in std::env::args().skip(1) {
        let d = std::fs::read(&p).unwrap();
        let name = std::path::Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
        let Ok(b) = nfs_formats::sky::SkyBin::parse(&d) else {
            println!("{name}: parse failed");
            continue;
        };
        let s = b.sun_position();
        let len = (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt();
        let dir = if len > 0.0 { [-s[0] / len, -s[1] / len, -s[2] / len] } else { [0.0; 3] };
        let elev = if len > 0.0 { (s[1] / len).asin().to_degrees() } else { 0.0 };
        println!(
            "{name:<16} sun_offset {:?} scale {:.2} ring A h {:.1} small {:.5} turns {:.3} -> pos {:.1?} elev {elev:.1} deg, light dir {:.3?}, sun_active {}, readback {}",
            b.sun_offset, b.scalar_ce4, b.rings[0].height, b.rings[0].small, b.rings[0].angle_turns, s, dir, b.sun_active(), b.sun_readback
        );
        for (i, c) in b.colours.iter().enumerate() {
            if let Some(c) = c {
                let f = |v: [f32; 3]| format!("({:.0},{:.0},{:.0})", v[0] * 254.0, v[1] * 254.0, v[2] * 254.0);
                println!("   colour rec {i}: grad {:?} A top {} bot {} | B top {} bot {}", c.gradient, f(c.rgb[0][0]), f(c.rgb[0][1]), f(c.rgb[1][0]), f(c.rgb[1][1]));
            }
        }
        let n = b.track_b.len();
        // the scene fog (0x45D3B0) at each key: density = field^3, and the clear colour of the key
        let mut fog: Vec<(f32, [f32; 3])> = Vec::new();
        for k in &b.track_b {
            if let Some((c, dn)) = b.fog_at(false, k.0, 1_000_000) {
                fog.push((dn, c));
            }
        }
        if fog.is_empty() {
            println!("   scene fog: none at any key");
        } else {
            let lo = fog.iter().map(|f| f.0).fold(f32::MAX, f32::min);
            let hi = fog.iter().map(|f| f.0).fold(f32::MIN, f32::max);
            let c = fog[0].1;
            println!("   scene fog: density {lo:.5}..{hi:.5} ({} of {n} keys), first colour ({:.0},{:.0},{:.0})", fog.len(), c[0] * 255.0, c[1] * 255.0, c[2] * 255.0);
        }
        for (label, f) in [("rain", 5), ("snow", 7)] {
            let v: Vec<f32> = if n < 2 { vec![0.0] } else { b.track_b.iter().map(|k| k.1[f]).collect() };
            let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, c), x| (a.min(*x), c.max(*x)));
            println!("   {label}: {lo:.3}..{hi:.3} ({n} keys)");
        }
    }
}
