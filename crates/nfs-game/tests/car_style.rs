//! The `.tpg` body-style array — `applyStyle` 0x437B00 and the loader's filter at 0x4420B9
//! (`RE/notes/car-model.md` §2.2, §3).
use nfs_assets::{car::CarStyle, GamePaths};

/// **Type 0 is not a wildcard.** 0x437B00 zeroes all 120 slots of `car+0x56C` before it reads
/// `[styleN]`, so a type-0 article shows only when its variant is 0 — and on the 993 that is the
/// difference between one hood decal and two in the same place.
#[test]
fn a_type_zero_article_shows_only_at_variant_zero() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let assets = match nfs_assets::car::CarAssets::load(&paths, "993") {
        Ok(a) => a,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    // every style of the 993, all nine of them
    for i in 1..=9 {
        let st = CarStyle::from_tpg(&assets.tpg, i);
        assert!(!st.name.is_empty(), "style{i} has no name");
        // no `[styleN]` on this car names slot 0, so slot 0 keeps the memset's zero
        assert!(st.geometry.get(&0).is_none(), "style{i} names slot 0");
        assert!(st.article_enabled(0, 0), "DecalHood, type 0 variant 0");
        assert!(!st.article_enabled(0, 1), "DecalHood1, type 0 variant 1, must not be drawn");
    }
    // and the articles really are a pair at one pivot, which is why drawing both is visible
    let arts = assets.crp.articles(&assets.crp_data).unwrap();
    let mut found = Vec::new();
    for a in &arts {
        let Ok(g) = assets.crp.article_geometry(&assets.crp_data, a) else { continue };
        if g.name.starts_with("DecalHood") && g.base.as_ref().is_some_and(|b| b.tag[0] == 0) {
            found.push((g.name.clone(), g.base.as_ref().unwrap().tag[1]));
        }
    }
    found.sort();
    assert_eq!(found, vec![("DecalHood".to_string(), 0u8), ("DecalHood1".to_string(), 1u8)]);
}

/// One `[styleN]` entry fills **one** slot: `geometry%d` defaults to −1 (0x437B97) and only a
/// negative one falls through to `texture%d` + 60 (0x437BA3 / 0x437BC8).
#[test]
fn a_style_entry_names_geometry_or_texture_but_never_both() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.game_data().join("CarModel");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("skipping: no CarModel");
        return;
    };
    let mut checked = 0;
    for e in entries.filter_map(|e| e.ok()) {
        if !e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("tpg")) {
            continue;
        }
        let Ok(bytes) = std::fs::read(e.path()) else { continue };
        let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let ini = nfs_formats::ini::Ini::parse(&text);
        for i in 1..=16 {
            let Some(sec) = ini.section(&format!("style{i}")) else { continue };
            let count = sec.get_i64("count").unwrap_or(0);
            for k in 1..=count {
                let g = sec.get_i64(&format!("geometry{k}"));
                let t = sec.get_i64(&format!("texture{k}"));
                assert!(!(g.is_some() && t.is_some()), "{:?} style{i} entry {k} names both", e.path());
                checked += 1;
            }
        }
    }
    assert!(checked > 500, "only {checked} style entries seen");
}

/// Slot 19's overlay picks the **driver**, not an "effect budget": slot 9 is `DriverBody1…10`,
/// slot 43 the arms and legs, slot 56 the hands (`car-model.md`, 0x43AEA5…0x43AF6B).
#[test]
fn the_driver_detail_overlay_names_driver_articles() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let Ok(assets) = nfs_assets::car::CarAssets::load(&paths, "993") else {
        eprintln!("skipping: no 993");
        return;
    };
    // the articles those three slots select, straight out of the model
    let mut by_slot: std::collections::BTreeMap<u8, Vec<(String, u8)>> = Default::default();
    for a in assets.crp.articles(&assets.crp_data).unwrap() {
        let Ok(g) = assets.crp.article_geometry(&assets.crp_data, &a) else { continue };
        let Some(b) = g.base.as_ref() else { continue };
        if matches!(b.tag[0], 9 | 43 | 56) {
            by_slot.entry(b.tag[0]).or_default().push((g.name.clone(), b.tag[1]));
        }
    }
    // `DriverBody1…10` at the near LODs and `LODDriver1…10` at the far ones — nothing else
    assert!(
        by_slot[&9].iter().all(|(n, _)| n.starts_with("DriverBody") || n.starts_with("LODDriver")),
        "slot 9: {:?}",
        by_slot[&9]
    );
    assert!(
        by_slot[&43].iter().all(|(n, _)| n.starts_with("LeftArm") || n.starts_with("RightArm") || n.starts_with("Legs")),
        "slot 43: {:?}",
        by_slot[&43]
    );
    assert!(by_slot[&56].iter().all(|(n, _)| n.ends_with("Hand1") || n.ends_with("Hand2")), "slot 56: {:?}", by_slot[&56]);

    // and the overlay at the port's detail keeps exactly one driver body and one set of limbs
    let mut st = CarStyle::from_tpg(&assets.tpg, 1);
    st.apply_driver_detail(8, false, false);
    let mut bodies: Vec<&str> = by_slot[&9].iter().filter(|(_, v)| st.article_enabled(9, *v)).map(|(n, _)| n.as_str()).collect();
    bodies.sort_unstable();
    // one per LOD family, and both are variant 1
    assert_eq!(bodies, vec!["DriverBody1", "LODDriver1"], "the overlay picks variant 1");
    // no driver at all when `car+0x52C & 0x10` (0x43AED4)
    let mut none = CarStyle::from_tpg(&assets.tpg, 1);
    none.apply_driver_detail(8, true, false);
    assert!(by_slot[&9].iter().all(|(_, v)| !none.article_enabled(9, *v)), "a driver survived");
}

/// The convertible roof. Slot 37 is the roof family and slot 41 the position, and slot 19
/// recomputes `slot41 = 2·slot37 − (roof down ? 1 : 0)` over whatever the style authored
/// (`car-model.md`). What has to hold for that to be sound is that **both** of those variants
/// exist in the model — and they do, on every cabriolet style in the game.
#[test]
fn slot_19s_roof_arithmetic_always_lands_on_geometry_that_exists() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let mut seen = 0;
    for model in ["356_1", "356a", "356b", "964", "993", "996", "gt3"] {
        let Ok(assets) = nfs_assets::car::CarAssets::load(&paths, model) else { continue };
        // which slot-41 variants the model carries
        let mut have: std::collections::BTreeSet<u32> = Default::default();
        for a in assets.crp.articles(&assets.crp_data).unwrap() {
            let Ok(g) = assets.crp.article_geometry(&assets.crp_data, &a) else { continue };
            if let Some(b) = g.base.as_ref()
                && b.tag[0] == 41
            {
                have.insert(b.tag[1] as u32);
            }
        }
        for i in 1..=16 {
            if assets.tpg.section(&format!("style{i}")).is_none() {
                continue;
            }
            let st = CarStyle::from_tpg(&assets.tpg, i);
            let Some(&family) = st.geometry.get(&37) else { continue };
            if have.is_empty() {
                continue; // a model with a roof cover but no folding roof (the 944, the Boxster)
            }
            assert!(have.contains(&(2 * family)), "{model} style{i} ({}): no slot-41 variant {}", st.name, 2 * family);
            assert!(have.contains(&(2 * family - 1)), "{model} style{i} ({}): no slot-41 variant {}", st.name, 2 * family - 1);
            seen += 1;
        }
    }
    assert!(seen >= 10, "only {seen} cabriolet styles checked");
}

/// The animation channels: `Base+75` is the rotation channel and the map is the same on every
/// shipped car — wheels 1/3, `Steer` 5, doors 6/7, hood 8, engine lid 9, needles 11/12,
/// `MoveSpoiler` **16**, the pop-up `HeadLight` **17** (`car-model.md`).
#[test]
fn the_rotation_channels_bind_the_parts_the_notes_name() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // the three cars with `headlightextent` are exactly the three with a channel-17 article,
    // and the four with `movespoilerextent` exactly those with a channel-16 one
    let mut with17: Vec<&str> = Vec::new();
    let mut with16: Vec<&str> = Vec::new();
    for model in ["356_1", "914", "928", "944", "959", "964", "993", "996", "boxster", "gt1", "gt2", "gt3"] {
        let Ok(assets) = nfs_assets::car::CarAssets::load(&paths, model) else { continue };
        let mut seen16 = false;
        let mut seen17 = false;
        for a in assets.crp.articles(&assets.crp_data).unwrap() {
            let Ok(g) = assets.crp.article_geometry(&assets.crp_data, &a) else { continue };
            let Some(b) = g.base.as_ref() else { continue };
            match b.rot_channel {
                16 => {
                    seen16 = true;
                    assert!(g.name.starts_with("MoveSpoiler"), "{model}: channel 16 on {}", g.name);
                }
                17 => {
                    seen17 = true;
                    assert!(g.name.contains("HeadLight"), "{model}: channel 17 on {}", g.name);
                }
                5 => assert!(g.name.starts_with("Steer"), "{model}: channel 5 on {}", g.name),
                _ => {}
            }
        }
        if seen16 {
            with16.push(model);
        }
        if seen17 {
            with17.push(model);
        }
    }
    assert_eq!(with17, vec!["914", "928", "944"], "the pop-up headlight cars");
    assert_eq!(with16, vec!["996", "boxster"], "the moving-spoiler cars in this set");
}
