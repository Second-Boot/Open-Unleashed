//! Regression tests against the original game data. Skipped (with a message) when the
//! installation is not present; see GAME_PATH.md.
use nfs_formats::{bigf::Bigf, crp::Crp, fsh::Fsh, ini::Ini, refpack, scn::{ElementKind, Scene}, sim::CarSim};
use std::path::PathBuf;

fn game_root() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NFS5_GAME_PATH") {
        return Some(PathBuf::from(p));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let p = home.join("Games/NFS-Porsche-Unleashed/game_info/data");
    p.join("GameData").is_dir().then_some(p)
}

macro_rules! need_game {
    () => {
        match game_root() {
            Some(r) => r,
            None => {
                eprintln!("skipping: original game data not found");
                return;
            }
        }
    };
}

#[test]
fn refpack_roundtrip_sizes() {
    let r = need_game!();
    for (f, expect) in [("GameData/Track/alps.crp", 6_168_576usize), ("GameData/Track/skidpad.crp", 2_596_864), ("GameData/CarModel/356_1.crp", 2_686_976)] {
        let d = std::fs::read(r.join(f)).unwrap();
        assert!(refpack::is_refpack(&d));
        assert_eq!(refpack::decompress(&d).unwrap().len(), expect, "{f}");
    }
}

#[test]
fn bigf_camera_viv() {
    let r = need_game!();
    let d = std::fs::read(r.join("GameData/Track/Data/camera.viv")).unwrap();
    let b = Bigf::parse(&d).unwrap();
    assert_eq!(b.entries.len(), 21);
    assert_eq!(b.entries[0].name, "TR00.rho");
    assert_eq!(b.get(&d, "tr00.rho").unwrap().len(), 2048);
}

#[test]
fn fsh_car_and_track() {
    let r = need_game!();
    let d = std::fs::read(r.join("GameData/CarModel/356_1D.fsh")).unwrap();
    let f = Fsh::parse(&d).unwrap();
    assert_eq!(f.entries.len(), 43);
    let img = f.decode(&d, 0).unwrap();
    assert_eq!((img.width, img.height), (58, 64));
    let t = refpack::decompress(&std::fs::read(r.join("GameData/Track/alps.fsh")).unwrap()).unwrap();
    let f = Fsh::parse(&t).unwrap();
    assert_eq!(f.entries.len(), 169);
    assert_eq!(f.entries[0].name, "cl10");
    let img = f.decode(&t, 0).unwrap();
    assert_eq!((img.width, img.height), (256, 256));
}

#[test]
fn crp_articles() {
    let r = need_game!();
    let d = refpack::decompress(&std::fs::read(r.join("GameData/CarModel/356_1.crp")).unwrap()).unwrap();
    let c = Crp::parse(&d).unwrap();
    assert_eq!(c.level.len(), 127);
    let arts = c.articles(&d).unwrap();
    assert_eq!(arts.len(), 115);
    assert_eq!(arts[0].parts.len(), 45);
}

#[test]
fn sim_993() {
    let r = need_game!();
    let s = CarSim::parse(&std::fs::read(r.join("GameData/Simulation/CarData/993coupe36.sim")).unwrap()).unwrap();
    assert_eq!(s.name, "1994 993 Carrera 3.6L");
    assert_eq!(s.mass_kg, 1370.0);
    assert_eq!(s.gear_count, 6);
    assert_eq!(s.gear_ratios, [3.818, 2.15, 1.56, 1.242, 1.024, 0.82]);
    assert_eq!(s.final_drive, 3.444);
    assert_eq!(s.redline_rpm, 6700.0);
}

#[test]
fn scn_all_track_files_parse() {
    let r = need_game!();
    let mut n = 0;
    for e in std::fs::read_dir(r.join("GameData/Track")).unwrap().flatten() {
        let p = e.path();
        if p.extension().map(|x| x.eq_ignore_ascii_case("scn")).unwrap_or(false) {
            let txt = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).to_string();
            let s = Scene::parse(&txt).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            assert!(!s.elements.is_empty(), "{}", p.display());
            n += s.elements.len();
        }
    }
    assert_eq!(n, 4513, "parsed {n} elements (1645 triggers + 1538 audio + 716 cameras + 614 geom)");
    let txt = std::fs::read_to_string(r.join("GameData/Track/alps_fstart.scn")).unwrap();
    let s = Scene::parse(&txt).unwrap();
    let t = s.of_kind(&ElementKind::Trigger).next().unwrap();
    assert_eq!(t.position, [-143.124573, 0.340156, 235.736008]);
}

#[test]
fn map_ini() {
    let r = need_game!();
    let ini = Ini::parse(&std::fs::read_to_string(r.join("GameData/Track/alps.map")).unwrap());
    assert_eq!(ini.section("header").unwrap().get_i64("count"), Some(39));
    assert_eq!(ini.section("section1").unwrap().get_i64("startslice"), Some(414));
}

#[test]
fn car_catalog() {
    let r = need_game!();
    let cars = nfs_formats::fedata::parse_car_catalog(&std::fs::read(r.join("FEData/Data/nfs5.car")).unwrap()).unwrap();
    assert_eq!(cars.len(), 109);
    assert_eq!(cars[0].display_name, "'50 356 1100 Cabriolet");
    assert_eq!(cars[0].model, "356_1");
    assert_eq!(cars[0].sim, "356cabrio11");
    assert_eq!(cars[0].sound_set, "sd1b");
    assert_eq!(cars[0].year, 1950);
    assert_eq!(cars[0].paint_sets.len(), 19);
    assert_eq!(cars[0].paint_sets[0].colours[1], [0xFF, 0x00, 0x00]);
    // every model and sim referenced must exist on disk
    for c in &cars {
        assert!(r.join("GameData/CarModel").join(format!("{}.crp", c.model)).exists() || std::fs::read_dir(r.join("GameData/CarModel")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&format!("{}.crp", c.model))), "model {}", c.model);
    }
    // .sim variants may be missing on disk: the loader falls back to default.sim (RE/notes/loaders.md)
    let empty = cars.iter().filter(|c| c.sim.is_empty()).count();
    eprintln!("catalogue entries without a sim name: {empty}");
    let missing: Vec<&str> = cars.iter().filter(|c| !c.sim.is_empty()).filter(|c| !std::fs::read_dir(r.join("GameData/Simulation/CarData")).unwrap().flatten().any(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(&format!("{}.sim", c.sim)))).map(|c| c.sim.as_str()).collect();
    assert!(missing.len() <= 5, "missing sims: {missing:?}");
    eprintln!("catalogue sims without a file (default.sim fallback): {missing:?}");
}

#[test]
fn fntf_fonts() {
    let r = need_game!();
    let f = nfs_formats::fnt::Font::parse(&std::fs::read(r.join("GameData/DashHud/UniversC12b.ffn")).unwrap()).unwrap();
    assert_eq!((f.atlas_width, f.atlas_height), (240, 85));
    let g = f.glyph('0').unwrap();
    assert_eq!((g.width, g.height, g.x, g.y), (8, 13, 166, 44));
    // the '0' glyph has ink in its interior rows and none in its last row
    assert!(f.texel(g.x as u32, g.y as u32 + 5) > 9);
    assert_eq!((0..g.width as u32).map(|dx| f.texel(g.x as u32 + dx, g.y as u32 + 12) as u32).sum::<u32>(), 0);
    let d = nfs_formats::fnt::Font::parse(&std::fs::read(r.join("GameData/DashHud/debug.ffn")).unwrap()).unwrap();
    assert_eq!((d.atlas_width, d.atlas_height, d.glyphs.len()), (256, 60, 224));
}

#[test]
fn career_tables() {
    use nfs_formats::career::*;
    let r = need_game!();
    let d = r.join("FEData/Data");
    let tracks = parse_track_table(&std::fs::read(d.join("nfs5.trk")).unwrap()).unwrap();
    assert_eq!(tracks.len(), 15);
    let coastal = tracks.iter().find(|t| t.file == "coastal").unwrap();
    assert_eq!((coastal.id, coastal.lock_mode, coastal.circuit), (8, 0, false));
    assert!((300..320).contains(&coastal.name_string_id));
    assert_eq!(coastal.records.len(), 8);
    assert_eq!(coastal.records[0].time, 10496);
    assert!(coastal.records.windows(2).all(|w| w[0].time <= w[1].time));
    assert!(tracks.iter().filter(|t| t.circuit).count() >= 6);

    let trn = parse_tournaments(&std::fs::read(d.join("nfs5.trn")).unwrap()).unwrap();
    assert_eq!(trn.len(), 35);
    let first = trn.iter().find(|t| t.id == 1).unwrap();
    assert_eq!(first.kind, TournamentKind::Era(Era::Classic));
    assert_eq!((first.start_year, first.name_string_id, first.title_string_id), (1950, 2000, 2018));
    assert_eq!(first.opponents.len(), 7);
    assert!(!first.events.is_empty() && first.events.iter().all(|e| tracks.iter().any(|t| t.id as i16 == e.track_id)));
    assert!(first.events.iter().all(|e| (0.3..=1.0).contains(&e.difficulty)));
    assert_eq!(trn.iter().filter(|t| t.is_hidden()).count(), 3);
    assert!(trn.iter().all(|t| t.opponents.iter().all(|o| o.car_id > 0)));

    let parts = parse_parts(&std::fs::read(d.join("nfs5.prt")).unwrap()).unwrap();
    assert_eq!(parts.len(), 683);
    assert!(parts.iter().any(|p| p.name_string_id == 5016 && p.kind == 3));
    assert!(parts.iter().filter(|p| (0.0..=100.0).contains(&p.condition)).count() >= 682, "one record (#347) stores -65");
    let mut ids: Vec<i16> = parts.iter().map(|p| p.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), parts.len(), "part ids are unique");

    let per = parse_personalities(&std::fs::read(d.join("nfs5.per")).unwrap()).unwrap();
    assert_eq!(per.len(), 35);
    assert!(per.iter().all(|p| p.skill.iter().all(|s| (0.0..=1.5).contains(&s[0]))));

    let fac = parse_missions(&std::fs::read(d.join("nfs5.fac")).unwrap()).unwrap();
    assert_eq!(fac.len(), 34);
    assert!(fac.iter().all(|m| (1..=3).contains(&m.stage)));
    let m0 = &fac[0];
    assert_eq!((m0.code.as_str(), m0.track_id, m0.scene, m0.car_id, m0.time_limit_s, m0.narrator), ("0M01", 32, 1, 161, Some(27), 5));
    assert_eq!(m0.cone_rule, Some(nfs_formats::career::ConeRule::PenaltySeconds(1)));
    assert_eq!(m0.scene_file("skidpad").as_deref(), Some("skidpad_st1.scn"));
    let race = fac.iter().find(|m| m.code == "2m05").unwrap();
    assert!(race.is_race() && race.min_rank == Some(1) && race.opponents[0].0 == 24);
    let delivery = fac.iter().find(|m| m.code == "1m06").unwrap();
    assert!(delivery.backwards && delivery.police && delivery.damage_limit_pct == Some(5) && delivery.traffic_level == 4 / 2);

    let strings = nfs_formats::locale::Strings::parse(&std::fs::read(r.join("FEData/Locale/FEStrings.csv")).unwrap(), 0);
    assert_eq!(strings.get(521), Some("CLASSIC ERA"));
    assert_eq!(strings.get(2018), Some("356 Challenge"));
    assert_eq!(strings.get(0), Some("Hello, World!"));
    assert!(trn.iter().all(|t| strings.get(t.title_string_id).is_some()));
}

/// `RE/notes/savegame.md` §7: the shipped `nfs5.trk` seeds `SaveData\nfs5.trk`, the fixup fills
/// every zero-time slot with `"n/a"` / 359 999, and insertion is a strict `<` with a shift.
#[test]
fn track_record_file_fixup_and_insert() {
    let r = need_game!();
    let d = std::fs::read(r.join("FEData/Data/nfs5.trk")).unwrap();
    assert_eq!(d.len(), 15 * 1192);
    let mut f = nfs_formats::career::TrackRecordFile::load(&d).unwrap();
    assert_eq!(f.count(), 15);

    // Coastal (id 8) ships with the fastest time 10496 held by Alberta.
    let coastal = f.index_of(8).expect("track 8");
    let e = f.entries(coastal);
    assert_eq!((e[0].driver.as_str(), e[0].time), ("Alberta", 10_496));
    assert_eq!(e[7].time, 18_432);

    // skidpad ships with all-zero entries, so every slot becomes the sentinel.
    let skid = (0..f.count()).find(|i| f.entries(*i)[0].time == nfs_formats::career::NO_RECORD_TIME).expect("an empty track");
    assert!(f.entries(skid).iter().all(|e| e.driver == "n/a" && e.time == nfs_formats::career::NO_RECORD_TIME));

    assert_eq!(f.insert(8, "Player", "911 GT1", 10_496), Some(1), "an equal time goes behind the holder");
    assert_eq!(f.insert(8, "Player", "911 GT1", 10_000), Some(0));
    let e = f.entries(coastal);
    assert_eq!(e[0].time, 10_000);
    assert_eq!((e[1].driver.as_str(), e[1].time), ("Alberta", 10_496));
    assert_eq!(e[2].driver, "Player");
    assert_eq!(e[7].time, 15_000, "the two slowest holders were pushed off");
    assert_eq!(f.insert(8, "Player", "911 GT1", 999_999), None);
    assert_eq!(f.raw.len(), d.len(), "the array keeps its size");
}
