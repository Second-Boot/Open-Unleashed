//! Diagnostic: which translation channels (`Base+72`, signed, −1 = none) the shipped car models
//! bind parts to, with the part names and rotation channels (`RE/notes/car-model.md`, audit
//! 2026-09-26). `R3DCarLOD::draw` offsets a part by `object + 0x6D0 + 12n` (0x43C945…0x43C982),
//! and 0x43E670 writes channel `i`'s `.y` from wheel `[0x5B36F0 + 4i]`'s `+0x68`.
use nfs_formats::refpack;
fn main() {
    let mut by: std::collections::BTreeMap<(i8, u8, String), usize> = Default::default();
    let mut files = 0;
    for p in std::env::args().skip(1) {
        let Ok(raw) = std::fs::read(&p) else { continue };
        let Ok(data) = refpack::maybe_decompress(&raw) else { continue };
        let Ok(crp) = nfs_formats::crp::Crp::parse(&data) else { continue };
        files += 1;
        for a in crp.articles(&data).unwrap_or_default() {
            let Ok(g) = crp.article_geometry(&data, &a) else { continue };
            if let Some(b) = &g.base
                && b.move_channel >= 0
            {
                *by.entry((b.move_channel, b.rot_channel, g.name.clone())).or_default() += 1;
            }
        }
    }
    println!("{files} files");
    for ((m, r, n), c) in by {
        println!("move {m} rot {r} {n}: {c}");
    }
}
