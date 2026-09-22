//! Diagnostic: the blend mode and depth bias of every shipped particle system.
fn main() {
    let p = std::env::args().nth(1).unwrap();
    let d = std::fs::read(&p).unwrap();
    let defs = nfs_formats::particle_dat::parse(&d);
    let mut by: std::collections::BTreeMap<(u32, i32), Vec<String>> = Default::default();
    for s in &defs {
        by.entry((s.blend, s.zbias)).or_default().push(s.material.clone());
    }
    for ((b, z), mats) in by {
        println!("blend {b} zbias {z}: {} systems, e.g. {:?}", mats.len(), &mats[..mats.len().min(6)]);
    }
}
