//! Diagnostic: per sky `.bin`, the track-B fields the weather layer 0x459020 hands its two emitters —
//! the snow one `+0x20` (field 7, 0x45CFD0 → 0x45A940) and the rain one `+0x18` (field 5, 0x45D000 →
//! 0x45A8E0) — plus `+0x1C` (field 6, 0x45A910) for completeness: min / max over the keys and how many
//! keys are non-zero.
fn main() {
    for p in std::env::args().skip(1) {
        let d = std::fs::read(&p).unwrap();
        let name = std::path::Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
        let Ok(b) = nfs_formats::sky::SkyBin::parse(&d) else {
            println!("{name}: parse failed");
            continue;
        };
        // 0x45A490: fewer than two keys → two zero keys, so every field reads 0
        let n = b.track_b.len();
        let mut line = format!("{name:<26} v{} keys {n:>3}", b.version);
        for (label, f) in [("rain +0x18", 5), ("+0x1C", 6), ("snow +0x20", 7)] {
            let v: Vec<f32> = if n < 2 { vec![0.0] } else { b.track_b.iter().map(|k| k.1[f]).collect() };
            let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, c), x| (a.min(*x), c.max(*x)));
            let nz = v.iter().filter(|x| **x != 0.0).count();
            line += &format!(" | {label}: {lo:.3}..{hi:.3} ({nz} non-zero)");
        }
        println!("{line}");
    }
}
