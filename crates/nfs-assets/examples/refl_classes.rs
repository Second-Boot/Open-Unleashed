//! Diagnostic (ENHANCEMENT E8): per article of a car at LOD 1 — type id, each material's render
//! method, reflection strength and texture page. The data the E8 material-class table is read from.
use nfs_assets::{car::CarAssets, GamePaths};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let c = CarAssets::load(&paths, &a[0]).unwrap();
    let mats = c.crp.materials(&c.crp_data).unwrap();
    for art in c.crp.articles(&c.crp_data).unwrap() {
        let Ok(g) = c.crp.article_geometry(&c.crp_data, &art) else { continue };
        let ty = g.base.as_ref().map(|b| b.tag[0]).unwrap_or(0);
        let mut skipped = 0;
        for mm in nfs_assets::mesh::article_meshes(&g, 1, &mut skipped) {
            let m = &mats[mm.material as usize];
            println!("{:<22} type {ty:#04x} mat {:>2} {:<12} refl {:.2} page {}", g.name, mm.material, m.render_method, m.reflection, m.page_index());
        }
    }
}
