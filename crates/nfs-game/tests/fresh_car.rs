//! What 0x49CB50 folds into a freshly built `FECar`'s `carSpecs` block — its stock parts (0x4EE9A0)
//! and the constructor's neutral sliders (0x4EED2D) — against the shipped catalogue
//! (`RE/notes/network.md` §11.12b, `loaders.md` §1.3).
use nfs_assets::car::CarAssets;
use nfs_assets::GamePaths;
use nfs_game::career::{fold_fresh_car, CareerTables};
use nfs_physics::specs::CarSpecs;

/// The neutral sliders are not the identity: shock rate 1 gives `transfer_div ← (1+2)·td/3` and then
/// `grip ×(td+9)·0.1`, `roll_gain ÷ (td+1)·0.5` (0x49D3F3…), 50/50 downforce and 30 psi are exact, so
/// for a car with no stock effect the fold is exactly those two factors — and the top speed, the
/// final drive, the gears, the front weight stay as the file says, and the ride height is overwritten with 0.
#[test]
fn the_neutral_fold_scales_grip_and_roll_gain_by_the_transfer_divisor_for_every_car() {
    let Ok(paths) = GamePaths::locate() else { return };
    let Ok(t) = CareerTables::load(&paths) else { return };
    let mut checked = 0;
    for entry in &t.catalog {
        if entry.sim.is_empty() {
            continue;
        }
        let Ok(sim) = CarAssets::load_sim(&paths, &entry.sim) else { continue };
        let race_class = entry.sim.to_lowercase().starts_with("gt") || entry.sim.starts_with("935");
        let raw = CarSpecs::from_sim(&sim, race_class);
        // parts alone, then the neutral sliders on top, as 0x49CB50 orders them
        let stock: Vec<&nfs_formats::career::Part> = entry.stock_parts.iter().filter_map(|id| t.parts.iter().find(|p| p.id == *id)).collect();
        let mut parts_only = raw.clone();
        parts_only.apply_parts(&stock, &entry.sim_family);
        let mut folded = raw.clone();
        fold_fresh_car(&mut folded, &entry.stock_parts, &t.parts, &entry.sim_family, true);
        let td = parts_only.transfer_div;
        let shock = (1.0f32 + 2.0) * td * (1.0 / 3.0);
        assert!((folded.transfer_div - shock).abs() <= td.abs() * 1e-6, "{}: transfer_div", entry.sim);
        assert!((folded.grip / parts_only.grip - (shock + 9.0) * 0.1).abs() < 1e-5, "{}: grip {} -> {}", entry.sim, parts_only.grip, folded.grip);
        assert!(
            (folded.roll_gain / parts_only.roll_gain - 1.0 / ((shock + 1.0) * 0.5)).abs() < 1e-4 * (1.0 / ((shock + 1.0) * 0.5)),
            "{}: roll_gain {} -> {}",
            entry.sim,
            parts_only.roll_gain,
            folded.roll_gain
        );
        assert_eq!(folded.top_speed, parts_only.top_speed, "{}: top speed (50/50 downforce divides by 1)", entry.sim);
        assert_eq!(folded.final_drive, parts_only.final_drive, "{}: final drive", entry.sim);
        assert_eq!(folded.gear_ratio, parts_only.gear_ratio, "{}: gears", entry.sim);
        // the ride-height slider is an **overwrite** (0x49D555: `+0x120 = v·0.001`), so the neutral 0 takes
        // the wheel probe's offset to 0 whatever the file said (`Boxster25`'s carries 0.015)
        assert_eq!(folded.probe_offset, 0.0, "{}: ride height", entry.sim);
        assert!(!folded.parts.is_empty());
        checked += 1;
        if std::env::var_os("NK_SURVEY").is_some() {
            eprintln!(
                "NKSURVEY {:<14} fam={:<4} stock={:>2}  parts: mass {:+.1} top {:+.3}% grip {:+.4}% td {:+.4}% roll_gain {:+.3}% torque_pk {:+.3}%  | setup: grip {:+.3}% roll_gain {:+.3}%",
                entry.sim,
                entry.sim_family,
                stock.len(),
                parts_only.mass - raw.mass,
                (parts_only.top_speed / raw.top_speed - 1.0) * 100.0,
                (parts_only.grip / raw.grip - 1.0) * 100.0,
                (parts_only.transfer_div / raw.transfer_div - 1.0) * 100.0,
                (parts_only.roll_gain / raw.roll_gain - 1.0) * 100.0,
                (parts_only.torque.iter().copied().fold(0.0, f32::max) / raw.torque.iter().copied().fold(0.0, f32::max) - 1.0) * 100.0,
                (folded.grip / parts_only.grip - 1.0) * 100.0,
                (folded.roll_gain / parts_only.roll_gain - 1.0) * 100.0,
            );
        }
    }
    if checked == 0 {
        eprintln!("skipping: no game data");
    }
}

/// A traffic car's block is `default.sim` (its sim name is empty, `traffic.md` §21.1) folded like any
/// racer's — the neutral sliders alone, when its record carries no stock parts.
#[test]
fn a_traffic_car_block_is_default_sim_through_the_neutral_fold() {
    let Ok(paths) = GamePaths::locate() else { return };
    let Ok(t) = CareerTables::load(&paths) else { return };
    let Ok(sim) = CarAssets::load_sim(&paths, "default") else { return };
    let raw = CarSpecs::from_sim(&sim, false);
    let mut folded = raw.clone();
    fold_fresh_car(&mut folded, &[], &t.parts, "", true);
    let shock = (1.0f32 + 2.0) * raw.transfer_div * (1.0 / 3.0);
    assert!((folded.grip / raw.grip - (shock + 9.0) * 0.1).abs() < 1e-5);
    assert_eq!(folded.probe_offset, 0.0);
    if std::env::var_os("NK_SURVEY").is_some() {
        eprintln!(
            "NKSURVEY default.sim td {} grip {} -> {} roll_gain {} -> {} probe {} -> {} pitch {} roll {}",
            raw.transfer_div, raw.grip, folded.grip, raw.roll_gain, folded.roll_gain, raw.probe_offset, folded.probe_offset, folded.pitch_ret, folded.roll_ret
        );
    }
}

/// The traffic cars' catalogue records (`nfs5.trk +0x7C`) carry no stock parts, so the traffic's fold
/// is the neutral sliders alone (`network.md` §11.12c).
#[test]
fn the_traffic_records_carry_no_stock_parts() {
    let Ok(paths) = GamePaths::locate() else { return };
    let Ok(t) = CareerTables::load(&paths) else { return };
    let Ok(trk) = std::fs::read(paths.fe_data().join("Data").join("nfs5.trk")) else { return };
    let Ok(tracks) = nfs_formats::career::parse_track_table(&trk) else { return };
    for r in &tracks {
        for id in r.traffic_cars.iter().copied().filter(|i| *i != 0) {
            let Some(e) = t.catalog.iter().find(|c| c.id == id) else { continue };
            if std::env::var_os("NK_SURVEY").is_some() {
                eprintln!("NKTRAFFIC {} id {} model {} sim {:?} family {:?} stock {}", r.file, id, e.model, e.sim, e.sim_family, e.stock_parts.len());
            }
            assert!(e.stock_parts.is_empty(), "traffic record {id} ({}) carries stock parts", e.model);
        }
    }
}
