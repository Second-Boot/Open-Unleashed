//! Inspect original game files: auto-detects RefPack, BIGF (.viv), SHPI (.fsh), CRP, .sim.
//!
//! ```text
//! asset-inspector <file>                 summarize
//! asset-inspector <file> --png <outdir>  also export FSH images as PNG
//! ```
use anyhow::{bail, Context, Result};
use nfs_formats::{bigf::Bigf, crp::{Crp, CrpEntry}, fsh::Fsh, refpack, sim::CarSim};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        bail!("usage: asset-inspector <file> [--png <outdir>]");
    }
    let path = PathBuf::from(&args[0]);
    let png_dir = args.iter().position(|a| a == "--png").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let raw = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    println!("{}: {} bytes", path.display(), raw.len());
    let data = if refpack::is_refpack(&raw) {
        let h = refpack::read_header(&raw)?;
        let d = refpack::decompress(&raw)?;
        println!("  RefPack: flags={:#04x} uncompressed={} (ratio {:.2})", h.flags, d.len(), raw.len() as f64 / d.len() as f64);
        d
    } else {
        raw
    };
    match &data[..4.min(data.len())] {
        b"BIGF" => inspect_bigf(&data),
        b"SHPI" => inspect_fsh(&data, png_dir.as_deref(), &path),
        b"karT" | b" raC" => inspect_crp(&data),
        _ if data.len() == nfs_formats::sim::SIM_SIZE => inspect_sim(&data),
        other => {
            println!("  unknown format, first bytes: {:02x?}", other);
            Ok(())
        }
    }
}

fn inspect_bigf(data: &[u8]) -> Result<()> {
    let b = Bigf::parse(data)?;
    println!("  BIGF: {} entries, declared size {}, header {}", b.entries.len(), b.declared_size, b.header_size);
    for e in &b.entries {
        let d = b.data(data, e);
        println!("    {:32} off={:#8x} size={:8} head={:02x?}", e.name, e.offset, e.size, &d[..8.min(d.len())]);
    }
    Ok(())
}

fn inspect_fsh(data: &[u8], png_dir: Option<&Path>, src: &Path) -> Result<()> {
    let f = Fsh::parse(data)?;
    println!("  SHPI: {} entries, dir id {:?}", f.entries.len(), std::str::from_utf8(&f.directory_id).unwrap_or("?"));
    let mut codes: BTreeMap<u8, usize> = BTreeMap::new();
    for (i, e) in f.entries.iter().enumerate() {
        let b = &e.blocks[0];
        *codes.entry(b.code).or_default() += 1;
        if i < 40 {
            println!(
                "    {:4} off={:#8x} code={:#04x} {}x{} misc={:?} blocks={:?} len={}",
                e.name, e.offset, b.code, b.width, b.height, b.misc,
                e.blocks.iter().map(|b| format!("{:#x}", b.code)).collect::<Vec<_>>(),
                f.image_block_len(data.len(), i)
            );
        }
    }
    println!("  format codes: {:?}", codes.iter().map(|(k, v)| (format!("{k:#x}"), *v)).collect::<Vec<_>>());
    if let Some(dir) = png_dir {
        std::fs::create_dir_all(dir)?;
        let stem = src.file_stem().unwrap().to_string_lossy();
        let mut ok = 0;
        for i in 0..f.entries.len() {
            match f.decode(data, i) {
                Ok(img) => {
                    let name = f.entries[i].name.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
                    let out = dir.join(format!("{stem}_{i:03}_{name}.png"));
                    image::save_buffer(&out, &img.rgba, img.width, img.height, image::ColorType::Rgba8)?;
                    ok += 1;
                }
                Err(e) => println!("    entry {i}: {e}"),
            }
        }
        println!("  wrote {ok} PNGs to {}", dir.display());
    }
    Ok(())
}

fn inspect_crp(data: &[u8]) -> Result<()> {
    let c = Crp::parse(data)?;
    println!("  CRP kind={:?} articles(header)={} other level entries={} level entries={} material entries={}", c.kind, c.article_count(), c.header_b, c.level.len(), c.material_entries.len());
    let mut level: BTreeMap<String, usize> = BTreeMap::new();
    for e in &c.level {
        *level.entry(e.tag_str()).or_default() += 1;
    }
    println!("  level tags: {level:?}");
    if let Some(sn) = c.texture_file_name(data)? {
        println!("  texture file: {sn}");
    }
    let mats = c.materials(data)?;
    println!("  {} materials; first: {:?}", mats.len(), mats.iter().take(4).map(|m| (m.render_method.clone(), m.texture_name())).collect::<Vec<_>>());
    let arts = c.articles(data)?;
    let mut part: BTreeMap<String, usize> = BTreeMap::new();
    let (mut tris, mut verts, mut skipped) = (0usize, 0usize, 0usize);
    for a in &arts {
        for p in &a.parts {
            *part.entry(p.tag_str()).or_default() += 1;
        }
        let g = c.article_geometry(data, a)?;
        for p in &g.primitives {
            if p.lod == g.primitives.iter().map(|q| q.lod).min().unwrap_or(0) {
                if p.prim_type == 3 && !p.vertex_indices.is_empty() {
                    tris += p.vertex_indices.len() / 3;
                    verts += p.positions.len();
                } else {
                    skipped += 1;
                }
            }
        }
    }
    println!("  {} articles, part tags: {part:?}", arts.len());
    println!("  lowest LOD: {tris} triangles over {verts} windowed vertices, {skipped} non-indexed/other primitives skipped");
    for a in arts.iter().take(2) {
        let g = c.article_geometry(data, a)?;
        println!("    article {:?}: {} primitives, base={:?}", g.name, g.primitives.len(), g.base.as_ref().map(|b| (b.lod_count, b.bbox_min, b.bbox_max)));
        for p in g.primitives.iter().take(3) {
            println!("      lod{} type={} mat={} ({}) verts={} uvs={} idx={} first pos={:?}", p.lod, p.prim_type, p.material, mats.get(p.material as usize).map(|m| m.texture_name()).unwrap_or_default(), p.positions.len(), p.uvs.len(), p.vertex_indices.len(), p.positions.first());
        }
    }
    Ok(())
}

fn inspect_sim(data: &[u8]) -> Result<()> {
    let s = CarSim::parse(data)?;
    println!("  .sim: {s:#?}");
    Ok(())
}
