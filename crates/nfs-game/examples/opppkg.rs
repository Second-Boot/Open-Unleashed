//! Diagnostic: the quick-race opponent picks over many simulated races.
use nfs_game::opponents::{fill, FeRng, OpponentChoice, CRand};
use std::collections::BTreeMap;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let d = std::fs::read(&a[1]).unwrap();
    let catalog = nfs_formats::fedata::parse_car_catalog(&d).unwrap();
    let cands: Vec<usize> = catalog.iter().enumerate().filter(|(_, c)| c.kind == 0 && !c.sim.is_empty()).map(|(i, _)| i).collect();
    let races: u32 = a[2].parse().unwrap();
    let mode: &str = &a[3]; // "clock": FeRng seeded t, C rand from a stream seeded t+k ; "rand": independent hashed seeds
    let sel = 53usize; // 993 coupe
    let (mut pk, mut dup_pkg, mut dup_look, mut dup_pkg_nz, mut any_gd) = (BTreeMap::<u32, u64>::new(), 0u64, 0u64, 0u64, 0u64);
    let mut n = 0u64;
    let mut st = 0x1234_5678u64;
    for r in 0..races {
        let t = 1_800_000_000u32 + r * 37; // one quick race every 37 s
        let mut fe = FeRng::new(t);
        let cs = match mode { "clock" => t.wrapping_mul(1) , _ => { st = st.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); (st >> 32) as u32 } };
        let mut cr = CRand::new(cs);
        // the glints / race draws before the pick move the C state by an unknown count
        for _ in 0..(r % 5000) { cr.next(); }
        let p = fill(&catalog, &cands, sel, OpponentChoice::RandomCar, &mut fe, &mut cr, 8);
        for x in &p { *pk.entry(x.package).or_default() += 1; n += 1; }
        let mut seen = std::collections::HashSet::new(); let mut seen_l = std::collections::HashSet::new();
        let (mut dp, mut dl) = (false, false);
        for x in &p {
            if x.package != 0 && !seen.insert((x.package, x.paint_set)) { dp = true; }
            // the exact look: same car model (texture) + same set colours + same package
            let e = &catalog[x.car];
            let k = (e.model.clone(), e.paint_sets[x.paint_set].colours, x.package);
            if !seen_l.insert(k) { dl = true; }
            if x.package != 0 && e.paint_sets[x.paint_set].colours[1] == [255, 198, 0] && e.paint_sets[x.paint_set].colours[0] == [171, 0, 0] { any_gd += 1; }
        }
        if dp { dup_pkg += 1; } if dl { dup_look += 1; }
        let _ = &mut dup_pkg_nz;
    }
    println!("races {races} opponents {n}");
    for (k, v) in &pk { println!("package {k}: {:.3}", *v as f64 / n as f64); }
    println!("races with two opponents of the same (package!=0, paint colours-slot): {:.4}", dup_pkg as f64 / races as f64);
    println!("races with two opponents of the identical look (model, colours, package): {:.4}", dup_look as f64 / races as f64);
    println!("opponents dark-red/gold set + package!=0: {:.4} per opponent; per race(approx) {:.4}", any_gd as f64 / n as f64, any_gd as f64 / races as f64);
    println!("candidates {}", cands.len());
}
