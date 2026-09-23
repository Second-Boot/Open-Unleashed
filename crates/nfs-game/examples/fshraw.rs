//! Dump every entry of an `.fsh` pack as raw RGBA (`<out>/<n>_<name>_<w>x<h>.rgba`), top mip only —
//! for looking at HUD textures with an image tool.
//!
//! `cargo run -q -p nfs-game --example fshraw -- <file.fsh> <out dir>`
fn main() -> anyhow::Result<()> {
    let a: Vec<String> = std::env::args().collect();
    let (src, out) = (std::path::Path::new(&a[1]), std::path::Path::new(&a[2]));
    std::fs::create_dir_all(out)?;
    let data = std::fs::read(src)?;
    for (i, t) in nfs_assets::texture::decode_pack(src, &data)?.iter().enumerate() {
        let name = t.name.replace(['/', '.', ' '], "_");
        let p = out.join(format!("{i}_{name}_{}x{}.rgba", t.width, t.height));
        std::fs::write(&p, &t.mips[0])?;
        println!("{}", p.display());
    }
    Ok(())
}
