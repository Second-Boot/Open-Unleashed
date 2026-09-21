//! How many distinct 3D models the playable catalogue has, and how many cars share each —
//! the answer to "switching cars only changes the paint".
fn main() {
    let paths = nfs_assets::GamePaths::locate().expect("game");
    let raw = std::fs::read(paths.fe_data().join("Data").join("nfs5.car")).unwrap();
    let cat = nfs_formats::fedata::parse_car_catalog(&raw).unwrap();
    let mut by: std::collections::BTreeMap<String, Vec<(String, i16)>> = Default::default();
    for c in cat.iter().filter(|c| c.kind == 0 && !c.sim.is_empty()) {
        by.entry(c.model.clone()).or_default().push((c.display_name.clone(), c.variant));
    }
    let n: usize = by.values().map(|v| v.len()).sum();
    println!("{n} playable cars, {} distinct models", by.len());
    for (m, cars) in &by {
        let variants: std::collections::BTreeSet<i16> = cars.iter().map(|c| c.1).collect();
        println!("  {m:10} {:2} cars, {} body styles  e.g. {}", cars.len(), variants.len(), cars[0].0);
    }
}
