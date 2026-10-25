//! Diagnostic (BEAM-EDGE): the lighthouse beam's materials and textures on coastal.
use nfs_assets::{track::TrackAssets, GamePaths};
fn main() {
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, "coastal").unwrap();
    let arts = t.crp.articles(&t.crp_data).unwrap();
    let mats = t.materials().unwrap();
    let cull = nfs_game::world::cull_articles(&t);
    for (i, a) in arts.iter().enumerate() {
        let Some(c) = &cull[i] else { continue };
        if !(c.name.eq_ignore_ascii_case("BOX01") || c.name.eq_ignore_ascii_case("OBJECT01")) { continue; }
        println!("article {:?}", c.name);
        let Ok(g) = t.crp.article_geometry(&t.crp_data, a) else { continue };
        for p in &g.primitives {
            let m = &mats[p.material as usize];
            println!(" lod {} prim mat {} method {} tex {} flags {:#x} class {} opmask {:#x} lod_bias {} addr {:?} verts {}", p.lod, p.material, m.render_method, m.texture_name(), m.flags, m.class, m.op_mask, m.lod_bias, m.address, p.positions.len());
            if let Some(x) = t.textures.iter().find(|x| x.name == m.texture_name()) {
                let hist = { let mut h = [0u32;256]; for c in x.mips[0].chunks_exact(4) { h[c[3] as usize]+=1; } h };
                let distinct = hist.iter().filter(|&&n| n>0).count();
                if std::env::var("BEAM_OUT").is_ok() { let mut o = format!("P6\n{} {}\n255\n", x.width, x.height).into_bytes(); for c in x.mips[0].chunks_exact(4) { for k in 0..3 { o.push(((c[k] as u32 * c[3] as u32 + 240 * (255 - c[3] as u32)) / 255) as u8); } } std::fs::write(format!("{}/{}.ppm", std::env::var("BEAM_OUT").unwrap(), x.name), o).unwrap(); let mut al = format!("P5\n{} {}\n255\n", x.width, x.height).into_bytes(); for c in x.mips[0].chunks_exact(4) { al.push(c[3]); } std::fs::write(format!("{}/{}_a.pgm", std::env::var("BEAM_OUT").unwrap(), x.name), al).unwrap(); }
                println!("   tex {}x{} mips {} has_alpha {} distinct alpha {}", x.width, x.height, x.mips.len(), x.has_alpha, distinct);
            }
        }
    }
}
