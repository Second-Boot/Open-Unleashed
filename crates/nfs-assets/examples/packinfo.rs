//! Print the entries of an `.fsh`/`.qfs` pack: index, size and tag.
fn main() {
    for p in std::env::args().skip(1) {
        let path = std::path::PathBuf::from(&p);
        let Ok(d) = std::fs::read(&path) else { continue };
        match nfs_assets::texture::decode_pack(&path, &d) {
            Ok(v) => {
                println!("{p}: {} entries", v.len());
                for (i, t) in v.iter().enumerate() {
                    println!("  [{i}] {}x{}  {:?}", t.width, t.height, t.name.trim());
                }
            }
            Err(e) => println!("{p}: {e}"),
        }
    }
}
