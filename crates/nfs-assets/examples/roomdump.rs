//! Diagnostic: the front end's room models (`FEData/Models/<name>.crp`) — each article's `Base+70`
//! draw key, and each LOD-1 primitive's `pr` flags (bit 0 = `R3DCarLOD::draw`'s second walk,
//! 0x43CB55), material, render method, `Rs` block, page and the page's `.tpg` `bpp`, with the
//! primitive's y range (a flat layer has one y).
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    let dir = paths.fe_data().join("Models");
    for name in std::env::args().skip(1) {
        let car = CarAssets::load_in(&dir, &name).unwrap();
        let mats = car.crp.materials(&car.crp_data).unwrap();
        println!("# {name}");
        for (k, data) in car.crp.embedded_texture_files(&car.crp_data).unwrap().into_iter().enumerate() {
            let Ok(f) = nfs_formats::fsh::Fsh::parse(data) else { continue };
            let fm: Vec<String> = (0..f.entries.len()).filter_map(|i| f.decode(data, i).ok().map(|d| format!("{}:{:?}", f.entries[i].name.trim(), d.source_format))).collect();
            println!("  sf{k}: {}", fm.join(" "));
        }
        for (i, m) in mats.iter().enumerate() {
            let page = m.page_index() + 1;
            let bpp = car.tpg.section(&format!("tpage{page}.details")).and_then(|d| d.get_i64("bpp")).unwrap_or(1555);
            println!("  mt{i:<2} {:<10} page {page} bpp {bpp:<4} rs {:?} blocks {:?} addr {:?}", m.render_method, m.rs, m.blocks.iter().map(|b| b.0).collect::<Vec<_>>(), m.address);
        }
        for a in car.crp.articles(&car.crp_data).unwrap() {
            let Ok(g) = car.crp.article_geometry(&car.crp_data, &a) else { continue };
            let key = g.base.as_ref().map_or(0, |b| b.tag[2] as i8);
            let db = g.base.as_ref().map(|b| b.draw_bytes);
            let t = g.transform_for_lod(1);
            for p in g.primitives.iter().filter(|p| p.lod == 1) {
                let (lo, hi) = p.positions.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(v[1]), h.max(v[1])));
                println!(
                    "  {:<16} key {key:>3} db {db:?} pr flags {:#06x} mt {:<2} y {lo:.4}..{hi:.4} n {} ty {:.3}",
                    g.name,
                    p.flags,
                    p.material,
                    p.positions.len(),
                    t.map_or(0.0, |t| t[13])
                );
            }
        }
    }
}
