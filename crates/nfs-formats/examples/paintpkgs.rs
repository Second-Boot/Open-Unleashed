//! Diagnostic: the `package` values of every catalogue paint set, tallied.
fn main() {
    let p = std::env::args().nth(1).expect("nfs5.car");
    let d = std::fs::read(p).unwrap();
    let cars = nfs_formats::fedata::parse_car_catalog(&d).unwrap();
    let mut t = std::collections::BTreeMap::<u32, usize>::new();
    for c in &cars {
        for s in &c.paint_sets {
            *t.entry(s.package).or_default() += 1;
        }
    }
    println!("{t:?}");
}
