//! Diagnostic: can a car model (including traffic / police) be loaded and its texture pages built?
use nfs_assets::{car::{CarAssets, CarStyle}, GamePaths};

fn main() {
    let paths = GamePaths::locate().unwrap();
    for m in std::env::args().skip(1) {
        match CarAssets::load(&paths, &m) {
            Ok(c) => {
                let n = c.crp.articles(&c.crp_data).map(|a| a.len()).unwrap_or(0);
                let pages = c.build_pages(&paths, [200, 30, 30], &CarStyle::default());
                println!("{m}: ok, articles {n}, pages {:?}", pages.map(|p| p.len()).map_err(|e| e.to_string()));
            }
            Err(e) => println!("{m}: ERROR {e}"),
        }
    }
}
