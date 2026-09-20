//! A sweep over `nfs5.car` the way `--layout-sweep` is one over `FEData/Layouts`: for every
//! catalogue car, does everything the port derives from it actually resolve?
//!
//! The axes are the ones the traced rules give (`RE/notes/car-model.md`):
//! * the `.crp` / `.tpg` / `.sim` files the record names,
//! * the `[styleN]` section its `+0x1C0` variant selects (`applyStyle` 0x437B00 looks up
//!   `"style%d"` with the variant **plus one**),
//! * how many articles that style leaves on and off at LOD 1,
//! * articles **no** style of the model can ever enable — geometry the original never draws,
//! * the `BndP` records the style admits (`car_body_points`).
//!
//! Anything in the "problems" list at the end is either a port bug or a note that is wrong.
use nfs_assets::{car::CarAssets, GamePaths};
use std::collections::{BTreeMap, BTreeSet};

fn main() {
    let paths = GamePaths::locate().expect("game data");
    let cars = nfs_formats::fedata::parse_car_catalog(
        &std::fs::read(GamePaths::find_file(&paths.fe_data().join("Data"), "nfs5.car").unwrap()).unwrap(),
    )
    .unwrap();
    let verbose = std::env::args().any(|a| a == "-v");
    let mut problems: Vec<String> = Vec::new();
    // every (type, variant) pair any style of a model can enable, so the leftovers can be named
    let mut assets_cache: BTreeMap<String, Option<CarAssets>> = BTreeMap::new();
    let mut rows = 0usize;
    let mut decal_total = 0usize;
    let mut unreachable = 0usize;
    for (i, c) in cars.iter().enumerate() {
        if c.model.is_empty() {
            continue;
        }
        let key = c.model.to_lowercase();
        let entry = assets_cache.entry(key).or_insert_with(|| CarAssets::load(&paths, &c.model).ok());
        let Some(assets) = entry.as_ref() else {
            problems.push(format!("{i:3} {:<12} the model files do not load", c.model));
            continue;
        };
        rows += 1;
        let style_no = c.variant.max(0) as usize + 1;
        let Some(sec) = assets.tpg.section(&format!("style{style_no}")) else {
            // Not a fault when the file carries no styles at all: 0x437B5A bails on a missing
            // section and leaves the `memset`'s zeroes, so the car is all variant 0. The eleven
            // `EAS*` show cars are like that.
            if assets.tpg.section("style1").is_some() {
                problems.push(format!("{i:3} {:<12} variant {} names style{style_no}, which the .tpg has not", c.model, c.variant));
            }
            continue;
        };
        let name = sec.get("name").unwrap_or("").to_string();
        if name.is_empty() {
            problems.push(format!("{i:3} {:<12} style{style_no} has no name=", c.model));
        }
        // the named file, not what `load_sim` ends up with — it falls back to `default.sim`
        // the way 0x49C7D4 does, and the point here is to see the fallback being used
        if !c.sim.is_empty()
            && GamePaths::find_file(&paths.car_data_dir(), &format!("{}.sim", c.sim)).is_none()
        {
            problems.push(format!("{i:3} {:<12} .sim {:?} is not on disk — falls back to default.sim (0x49C7D4)", c.model, c.sim));
        }
        let mut style = nfs_assets::car::CarStyle::from_tpg(&assets.tpg, style_no);
        style.apply_driver_detail(8, false, false);
        // every style of this model, for the "no style can enable it" test
        let mut reachable: BTreeSet<(u8, u8)> = BTreeSet::new();
        for s in 1..=16 {
            if assets.tpg.section(&format!("style{s}")).is_none() {
                continue;
            }
            let st = nfs_assets::car::CarStyle::from_tpg(&assets.tpg, s);
            for slot in 0u8..=60 {
                reachable.insert((slot, st.geometry.get(&(slot as u32)).copied().unwrap_or(0) as u8));
            }
        }
        // …and the slots the **code** writes over the style at run time, which no `[styleN]`
        // needs to name: slot 9 / 43 / 56 are the driver's detail (slot 19, every arm of
        // 0x43AEA5…0x43AF6B), and 41 / 42 are the convertible roof and the pop-up spoiler.
        for v in [0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10] {
            reachable.insert((9, v));
            reachable.insert((43, v));
            reachable.insert((56, v));
            reachable.insert((41, v));
            reachable.insert((42, v));
        }
        let (mut on, mut off) = (0usize, 0usize);
        let mut decals = 0usize;
        let mut dead: Vec<String> = Vec::new();
        for a in assets.crp.articles(&assets.crp_data).unwrap() {
            let Ok(g) = assets.crp.article_geometry(&assets.crp_data, &a) else { continue };
            if !g.primitives.iter().any(|p| p.lod == 1) {
                continue;
            }
            let (ty, var) = g.base.as_ref().map(|b| (b.tag[0], b.tag[1])).unwrap_or((0, 0));
            if style.article_enabled(ty, var) {
                on += 1;
            } else {
                off += 1;
                    if !reachable.contains(&(ty, var)) {
                    // Slot 10 is the `Decal*` articles and **no** `[styleN]` in the game names
                    // it; the only thing that can turn them on is `applyStyle`'s tail,
                    // `slot10 = [[car+0x558]+0x510] >= 0` (0x437C17…0x437C2B). Counted on its own
                    // line rather than as a fault — `car-model.md`.
                    if ty == 0x0A {
                        decals += 1;
                    } else {
                        dead.push(format!("{} ({ty:#04x}/{var})", g.name));
                    }
                }
            }
        }
        let pts = nfs_game::world::car_body_points(&CarAssets::load(&paths, &c.model).unwrap().with_style(c.variant)).len();
        if pts < 24 {
            problems.push(format!("{i:3} {:<12} only {pts} body points", c.model));
        }
        // **Not a fault.** A `.crp` shared between catalogue cars carries every one of their
        // parts, and a car whose `.tpg` has one style can never reach the others' — the GT2 and
        // the two 993 police cars all drive the 993's model with a single style each. Reported as
        // a count so the number is visible without pretending it is wrong.
        unreachable += dead.len();
        if verbose && !dead.is_empty() {
            println!("      unreachable: {}", dead.join(", "));
        }
        decal_total += decals;
        if verbose {
            println!("{i:3} {:<12} style{style_no:<2} {name:<20} on {on:3} off {off:3} pts {pts}", c.model);
        }
    }
    println!("\n{rows} catalogue cars swept, {} problems", problems.len());
    println!("  {decal_total} Decal* articles held back by slot 10 (0x437C17 — nothing else can turn them on)");
    println!("  {unreachable} articles no style of their own model can reach (shared .crp files)");
    for p in &problems {
        println!("  {p}");
    }
}
