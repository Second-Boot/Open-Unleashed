//! Extract original assets to standard formats (never writes into the game directory).
//!
//! ```text
//! asset-extractor viv <file.viv> <outdir>        # BIGF entries → files (RefPack-decompressed if needed)
//! asset-extractor fsh <file.fsh> <outdir>        # SHPI entries → PNG (base mip)
//! asset-extractor crp <file.crp> <out.obj>       # lowest-LOD geometry → Wavefront OBJ (+ .mtl with texture names)
//! asset-extractor unpack <file> <out>            # RefPack-decompress a single file
//! ```
use anyhow::{bail, Context, Result};
use nfs_formats::{bigf::Bigf, crp::Crp, fsh::Fsh, refpack};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(input), Some(out)) = (args.first(), args.get(1), args.get(2)) else {
        bail!("usage: asset-extractor <viv|fsh|crp|unpack> <input> <output>");
    };
    let raw = std::fs::read(input).with_context(|| format!("reading {input}"))?;
    let data = refpack::maybe_decompress(&raw)?.into_owned();
    match cmd.as_str() {
        "unpack" => {
            std::fs::write(out, &data)?;
            println!("wrote {} bytes to {out}", data.len());
        }
        "viv" => {
            let b = Bigf::parse(&data)?;
            let dir = PathBuf::from(out);
            std::fs::create_dir_all(&dir)?;
            for e in &b.entries {
                let d = refpack::maybe_decompress(b.data(&data, e))?;
                std::fs::write(dir.join(safe_name(&e.name)), d.as_ref())?;
            }
            println!("extracted {} entries to {}", b.entries.len(), dir.display());
        }
        "fsh" => {
            let f = Fsh::parse(&data)?;
            let dir = PathBuf::from(out);
            std::fs::create_dir_all(&dir)?;
            let stem = Path::new(input).file_stem().unwrap().to_string_lossy();
            let mut n = 0;
            for i in 0..f.entries.len() {
                match f.decode(&data, i) {
                    Ok(img) => {
                        image::save_buffer(dir.join(format!("{stem}_{i:03}_{}.png", safe_name(&f.entries[i].name))), &img.rgba, img.width, img.height, image::ColorType::Rgba8)?;
                        n += 1;
                    }
                    Err(e) => eprintln!("entry {i}: {e}"),
                }
            }
            println!("wrote {n} PNGs to {}", dir.display());
        }
        "crp" => {
            let c = Crp::parse(&data)?;
            let mats = c.materials(&data)?;
            let mut obj = String::new();
            let mut mtl = String::new();
            let mtl_name = Path::new(out).with_extension("mtl");
            writeln!(obj, "mtllib {}", mtl_name.file_name().unwrap().to_string_lossy())?;
            for (i, m) in mats.iter().enumerate() {
                writeln!(mtl, "newmtl mat{i}\n# render method {}\nmap_Kd {}.png", m.render_method, safe_name(&m.texture_name()))?;
            }
            let mut base_v = 1usize;
            let mut base_t = 1usize;
            let mut tris = 0usize;
            for a in c.articles(&data)? {
                let g = c.article_geometry(&data, &a)?;
                let Some(lod) = g.primitives.iter().map(|p| p.lod).min() else { continue };
                writeln!(obj, "o {}", g.name.replace(' ', "_"))?;
                for p in g.primitives.iter().filter(|p| p.lod == lod && p.prim_type == 3 && !p.vertex_indices.is_empty()) {
                    let m = g.transform.map(glam_mat).unwrap_or_default();
                    for v in &p.positions {
                        let w = m.map(|m| m.transform_point3(glam::Vec3::from(*v)).to_array()).unwrap_or(*v);
                        writeln!(obj, "v {} {} {}", w[0], w[1], w[2])?;
                    }
                    for t in &p.uvs {
                        writeln!(obj, "vt {} {}", t[0], 1.0 - t[1])?;
                    }
                    writeln!(obj, "usemtl mat{}", p.material)?;
                    for k in 0..p.vertex_indices.len() / 3 {
                        let vi = |j: usize| base_v + p.vertex_indices[k * 3 + j] as usize;
                        let ti = |j: usize| base_t + p.uv_indices.get(k * 3 + j).copied().unwrap_or(0) as usize;
                        // file winding is clockwise from +Y → emit v0 v2 v1
                        writeln!(obj, "f {}/{} {}/{} {}/{}", vi(0), ti(0), vi(2), ti(2), vi(1), ti(1))?;
                        tris += 1;
                    }
                    base_v += p.positions.len();
                    base_t += p.uvs.len();
                }
            }
            std::fs::write(out, obj)?;
            std::fs::write(&mtl_name, mtl)?;
            println!("wrote {tris} triangles to {out}");
        }
        other => bail!("unknown command {other}"),
    }
    Ok(())
}

fn glam_mat(m: [f32; 16]) -> Option<glam::Mat4> {
    Some(glam::Mat4::from_cols_array(&m))
}

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}
