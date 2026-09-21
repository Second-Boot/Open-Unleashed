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

/// Every shipped car material carries `R3DLightLayer`'s block (`lighting.md` §6): the exterior on
/// slot 0, the interior on slot 1, and slot 2 only on env-mapped or window materials.
#[test]
fn car_materials_carry_their_light_layer() {
    let r = need_game!();
    let mut n = 0;
    for e in std::fs::read_dir(r.join("GameData/CarModel")).unwrap().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()).map(|x| x.eq_ignore_ascii_case("crp")) != Some(true) {
            continue;
        }
        let raw = std::fs::read(&p).unwrap();
        let d = refpack::maybe_decompress(&raw).unwrap();
        let c = Crp::parse(&d).unwrap();
        for m in c.materials(&d).unwrap() {
            let l = m.light.unwrap_or_else(|| panic!("{}: {} has no light block", p.display(), m.render_method));
            match m.render_method.as_str() {
                "CarExt" => assert_eq!(l.slot, 0),
                "CarInt" | "CarIntShd" => assert_eq!(l.slot, 1),
                _ => assert!(l.slot <= 2, "{} slot {}", m.render_method, l.slot),
            }
            n += 1;
        }
    }
    assert!(n > 1000, "{n} materials");
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
    // the car-compare ratings: acceleration, top speed, braking, handling, overall (+0x324 …)
    assert_eq!(cars[0].compare, [30, 40, 56, 30, 41]);
    // the widget that draws them has stops `c0`, `c50` and `c100`, so the scale is 0..100 — and
    // every record in the file keeps to it, traffic and cops included (they are a flat 50)
    for c in &cars {
        assert!(c.compare.iter().all(|v| (0..=100).contains(v)), "{} {:?}", c.display_name, c.compare);
    }
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

/// `GameData/Track/animdefs.txt` — the shipped object definitions (`RE/notes/crp-geometry.md` §7).
#[test]
fn animdefs_parses_the_shipped_file() {
    let Some(root) = game_root() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = root.join("GameData").join("Track").join("animdefs.txt");
    let Ok(bytes) = std::fs::read(&p) else {
        eprintln!("skipping: no animdefs.txt");
        return;
    };
    let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let defs = nfs_formats::animdefs::AnimDefs::parse(&text);
    // 64 `SECTION:ANIM` blocks but three tags appear twice (`WELD`, `GATE`, `FTRK`), and the last
    // of a repeated tag is the one the game keeps
    assert_eq!(defs.len(), 61);

    // the cone the skill tests count (`skill-tests.md` §5.3)
    let cone = defs.get("CONE").expect("CONE");
    assert_eq!(cone.collide_type, "kSmackable");
    assert_eq!(cone.audio_type, "kAudPylon");
    assert_eq!(cone.mass, 20.0);

    // the barrier boxes the port already puts in the wall set
    assert_eq!(defs.get("ENDW").map(|d| d.collide_type.as_str()), Some("kBox"));

    // the animated props: a docker loops at the default rate, the sign has two speed groups
    let doc = defs.get("DOC2").expect("DOC2");
    let p = doc.resting_pieces().next().expect("a resting piece");
    assert!(p.flags.looping);
    assert_eq!(p.playback_rate, 1.0);
    assert_eq!(doc.ticks_per_vert_frame, Some(3));

    let rates: Vec<f32> = defs.iter().flat_map(|d| d.pieces.iter().map(|(_, p)| p.playback_rate)).collect();
    assert!(rates.iter().any(|r| *r == 3.0) && rates.iter().any(|r| *r == 5.0) && rates.iter().any(|r| *r == 8.0));
    assert!(rates.iter().all(|r| *r > 0.0), "every piece has a usable rate");

    // every piece names the article whose track it plays
    let pieces: Vec<_> = defs.iter().flat_map(|d| d.pieces.iter().map(|(_, p)| p)).collect();
    assert!(pieces.len() >= 20);
    assert!(pieces.iter().all(|p| p.quat_group.is_some() || p.vert_group.is_some()));
}

/// The animated articles of a track: `Anim` gives the frame count, `Anqt` exactly 52 bytes a frame,
/// and every rotation is a unit quaternion (`RE/notes/crp-geometry.md` §7).
#[test]
fn track_animation_tracks_decode() {
    let Some(root) = game_root() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = root.join("GameData").join("Track").join("alps.crp");
    let Ok(raw) = std::fs::read(&p) else {
        eprintln!("skipping: no alps.crp");
        return;
    };
    let data = nfs_formats::refpack::maybe_decompress(&raw).unwrap().into_owned();
    let crp = nfs_formats::crp::Crp::parse(&data).unwrap();
    let arts = crp.articles(&data).unwrap();

    let mut animated = 0;
    let mut heli = None;
    for a in &arts {
        let Some(track) = crp.article_animation(&data, a) else { continue };
        animated += 1;
        assert!(track.frames.len() >= 2);
        for f in &track.frames {
            let q = f.rotation;
            let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
            assert!((n - 1.0).abs() < 1e-3, "a frame's rotation is a unit quaternion, got {n}");
        }
        let g = crp.article_geometry(&data, a).unwrap();
        if g.name == "RESCUEHELI" {
            heli = Some(track);
        }
    }
    assert_eq!(animated, 32, "alps has 32 animated articles");

    // the rescue helicopter's own track, and how the sampler walks it at the default ten frames a
    // second (`animdefs.txt` seeds `playbackrate` with 1.0)
    let heli = heli.expect("RESCUEHELI");
    assert_eq!(heli.frames.len(), 101);
    assert_eq!(heli.sample(0.0, 0.1, true), (0, 1, 0.0));
    assert_eq!(heli.sample(0.15, 0.1, true), (1, 2, 0.5));
    // looping wraps at frames − 2, so the last two frames are the seam
    assert_eq!(heli.sample(99.0 * 0.1, 0.1, true).0, 0);
    // a one-shot holds the last frame instead
    assert_eq!(heli.sample(99.0 * 0.1, 0.1, false), (100, 100, 0.0));
}

/// Every shipped `Track/Sky/*.txt` parses, and the two block shapes both appear.
#[test]
fn sky_definitions() {
    let Some(r) = game_root() else { return };
    let dir = r.join("GameData/Track/Sky");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("txt")).unwrap_or(false))
        .collect();
    files.sort();
    assert!(files.len() >= 15, "{} sky files", files.len());
    let (mut basic, mut multi) = (0, 0);
    for p in &files {
        let text = String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned();
        let f = nfs_formats::sky::SkyFile::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        assert!(!f.blocks.is_empty(), "{}", p.display());
        for b in &f.blocks {
            match b.kind {
                nfs_formats::sky::SkyKind::Basic { .. } => basic += 1,
                nfs_formats::sky::SkyKind::Multi { .. } => multi += 1,
                _ => {}
            }
            assert_eq!(b.regions[0].layers.len(), b.kind.layers());
            // a region always names at least the base layer
            assert!(b.regions[0].layers[0].map.is_some(), "{} has no base sky map", p.display());
        }
    }
    assert!(basic > 0 && multi > 0, "basic {basic}, multi {multi}");
    // coastal is the sunset track and asks for `sm1b`, not the `st1a` everything else uses
    let coastal = String::from_utf8_lossy(&std::fs::read(dir.join("Coastal.txt")).unwrap()).into_owned();
    let f = nfs_formats::sky::SkyFile::parse(&coastal).unwrap();
    assert_eq!(f.for_capability("HIGH").unwrap().regions[0].layers[0].map.as_deref(), Some("sm1b"));
}

/// `BndP` gives every car the same body: a wheel-contact quad on the ground, two end boxes and a
/// roof quad, plus whatever its style adds (`RE/notes/car-model.md` §12).
#[test]
fn car_body_points() {
    let Some(r) = game_root() else { return };
    let dir = r.join("GameData/CarModel");
    // expected wheel-quad span from the model itself; the `.sim` agrees for the road cars but not
    // for the GT1, whose simulation block carries 2400 mm against the model's 2658
    for (model, sim, quad_mm, sim_agrees) in [("993", "993coupe36", 2304.0, true), ("Boxster", "Boxster25", 2434.0, true), ("GT1", "GT1race", 2658.0, false)] {
        let Ok(raw) = std::fs::read(dir.join(format!("{model}.crp"))) else { continue };
        let data = nfs_formats::refpack::maybe_decompress(&raw).unwrap().into_owned();
        let crp = nfs_formats::crp::Crp::parse(&data).unwrap();
        let recs = crp.bound_points(&data).unwrap();
        let default: Vec<[f32; 3]> = recs.iter().filter(|r| r.style_slot == 0).flat_map(|r| r.points.iter().copied()).collect();
        assert_eq!(default.len(), 24, "{model}: default body point count");
        // the four lowest points are the wheel contact set, on the ground plane
        let mut by_y = default.clone();
        by_y.sort_by(|a, b| a[1].total_cmp(&b[1]));
        let ground = &by_y[..4];
        assert!(ground.iter().all(|p| p[1].abs() < 0.01), "{model}: the lowest four are not on y = 0");
        let (zmin, zmax) = ground.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[2]), b.max(p[2])));
        let span = (zmax - zmin) * 1000.0;
        assert!((span - quad_mm).abs() < 20.0, "{model}: wheel quad spans {span} mm, expected {quad_mm}");
        // and it is a quad, symmetric about the centreline
        let (xmin, xmax) = ground.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        assert!((xmin + xmax).abs() < 0.05, "{model}: wheel quad is not centred ({xmin}..{xmax})");
        // the road cars' `.sim` wheelbase matches the model's wheel quad to a few centimetres
        let sim_path = r.join("GameData/Simulation/CarData").join(format!("{sim}.sim"));
        if let Ok(d) = std::fs::read(&sim_path) {
            let s = nfs_formats::sim::CarSim::parse(&d).unwrap();
            let close = (s.wheelbase_mm - span).abs() < 60.0;
            assert_eq!(close, sim_agrees, "{sim}: sim {} mm against a {span} mm wheel quad", s.wheelbase_mm);
        }
    }
}

/// The sky colour records of `Track/Sky/<Track>.bin` read as the sky each track actually has.
#[test]
fn sky_background_colours() {
    let Some(r) = game_root() else { return };
    let dir = r.join("GameData/Track/Sky");
    let want: [(&str, [u8; 3]); 6] = [
        ("Monaco1.bin", [0, 0, 0]),          // the night circuits
        ("Coastal.bin", [254, 210, 196]),    // sunset
        ("Forest.bin", [119, 187, 254]),     // plain sky blue
        ("Castle.bin", [254, 194, 125]),     // late afternoon
        ("Industrial.bin", [93, 63, 21]),    // night, sodium-lit
        ("alps.bin", [128, 128, 144]),       // overcast, and the one track with a gradient
    ];
    for (file, rgb) in want {
        let Ok(d) = std::fs::read(dir.join(file)) else { continue };
        let s = nfs_formats::sky::SkyBin::parse(&d).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(s.version, if file.starts_with("Monaco.") { 7 } else { 9 }, "{file}");
        let bg = s.background().unwrap_or_else(|| panic!("{file}: no colour record"));
        let got = [(bg[0] * 254.0).round() as u8, (bg[1] * 254.0).round() as u8, (bg[2] * 254.0).round() as u8];
        assert_eq!(got, rgb, "{file}");
    }
    // alps is the only shipped track whose sky colour is a gradient, and its two weather regions differ
    let d = std::fs::read(dir.join("alps.bin")).unwrap();
    let s = nfs_formats::sky::SkyBin::parse(&d).unwrap();
    let c = s.colours[0].unwrap();
    assert!(c.gradient[0] && c.gradient[1]);
    assert_ne!(c.rgb[0][0], c.rgb[0][1], "alps sky is a ramp");
}

/// The sun 0x461090 places from the sky configuration (`RE/notes/lighting.md` §6): overhead where
/// the offset is zero (autobahn), off to one side on coastal.
#[test]
fn sky_bin_sun() {
    let Some(r) = game_root() else { return };
    let dir = r.join("GameData/Track/Sky");
    let sun = |f: &str| nfs_formats::sky::SkyBin::parse(&std::fs::read(dir.join(f)).unwrap()).unwrap().sun_position();
    let a = sun("Autobahn.bin");
    // (0, 76.724·0.576, 0): only the dome's height, scaled
    assert!(a[0].abs() < 1e-4 && a[2].abs() < 1e-4 && (a[1] - 76.724 * 0.5760).abs() < 0.05, "{a:?}");
    let c = sun("Coastal.bin");
    // a = −289.1, b = −204.1, turned by 0.6224 turn, raised 44.39 − 0.0002·(a² + b²), × 0.236
    let (a0, b0, t, g) = (-289.116f32, -204.082f32, 0.6224f32 * std::f32::consts::TAU, 0.2360f32);
    let want = [(t.cos() * a0 + t.sin() * b0) * g, (44.388 - 0.0002 * (a0 * a0 + b0 * b0)) * g, (t.cos() * b0 - t.sin() * a0) * g];
    for k in 0..3 {
        assert!((c[k] - want[k]).abs() < 0.2, "{c:?} vs {want:?}"); // the constants above are rounded
    }
}

/// The rest of `Track/Sky/<Track>.bin`: the ring geometry, the sixteen horizon quads and the three
/// per-slice keyframe tracks (`RE/RENDERING.md`, sky `.bin`).
#[test]
fn sky_bin_rings_and_tracks() {
    let Some(r) = game_root() else { return };
    let dir = r.join("GameData/Track/Sky");
    let Ok(d) = std::fs::read(dir.join("Coastal.bin")) else { return };
    let s = nfs_formats::sky::SkyBin::parse(&d).unwrap();
    // region A's ring: 1500 m across, 44.4 m up, turned 0.6224 of a turn (224°)
    assert!((s.rings[0].radius - 1500.0).abs() < 0.5, "{:?}", s.rings[0]);
    assert!((s.rings[0].height - 44.4).abs() < 0.5, "{:?}", s.rings[0]);
    assert!((s.rings[0].angle_turns - 0.6224).abs() < 1e-3, "{:?}", s.rings[0]);
    // the `horz` texture is a 4 × 2 grid, and Coastal mirrors the second half of the ring
    assert_eq!((s.tile_cols, s.tile_rows), (4, 2));
    assert_eq!(s.segments[0].tile, 0);
    assert_eq!(s.segments[7].tile, 7);
    assert_eq!((s.segments[8].tile, s.segments[8].mirror), (7, 1));
    // tile 5 of a 4 × 2 grid is column 1 of row 1
    let uv = s.segment_uv(5);
    assert!((uv[0] - 0.25).abs() < 1e-6 && (uv[1] - 0.5).abs() < 1e-6, "{uv:?}");
    assert!((uv[2] - 0.505).abs() < 1e-3 && (uv[3] - 0.99).abs() < 1e-3, "{uv:?}");
    // a mirrored quad swaps its two u's
    let uv8 = s.segment_uv(8);
    assert!(uv8[0] > uv8[1], "{uv8:?}");
    // the three tracks stop at the first negative slice
    assert_eq!(s.track_a.len(), 13);
    assert_eq!(s.track_b.len(), 17);
    assert_eq!(s.track_c.len(), 13);
    assert_eq!(s.track_a[0].0, 0);
    assert_eq!(s.track_a[1], (100, 1.0));
    // and the sampler interpolates between them
    let lap = 1500;
    assert!((s.sample_track_a(100, lap).unwrap() - 1.0).abs() < 1e-6);
    let mid = s.sample_track_a(50, lap).unwrap();
    assert!((0.0..=1.0).contains(&mid) && mid > 0.4 && mid < 0.6, "{mid}");

    // Monaco's horizon is one row of four tiles, hand-placed, and its ring is rotated 0.223
    let Ok(d) = std::fs::read(dir.join("Monaco1.bin")) else { return };
    let m = nfs_formats::sky::SkyBin::parse(&d).unwrap();
    assert_eq!((m.tile_cols, m.tile_rows), (4, 1));
    assert_eq!(m.colour_rotation, 5);
    assert_eq!(m.segment_colour(0), [5, 6]);
    assert_eq!(m.segment_colour(11), [0, 1]);
    assert!((m.rings[0].angle_turns - 0.223).abs() < 1e-3);
    // region B is the same default everywhere it was never authored
    assert!((m.rings[1].radius - 1000.0).abs() < 0.5 && (m.rings[1].height + 75.0).abs() < 0.5);
}

/// The sky colour record is chosen by the `SKYRENDERING` option, stepping down past the records the
/// track left disabled (`0x459950`).
#[test]
fn the_sky_colour_record_steps_down_to_an_enabled_one() {
    let Some(r) = game_root() else { return };
    let dir = r.join("GameData/Track/Sky");
    // monaco enables record 0 only, so every detail level lands on it
    let Ok(d) = std::fs::read(dir.join("Monaco1.bin")) else { return };
    let m = nfs_formats::sky::SkyBin::parse(&d).unwrap();
    for detail in 0..4 {
        assert_eq!(m.colour_index(detail), 0, "detail {detail}");
    }
    // coastal enables 0, 1 and 2 but not 3, so 3 falls back to 2 and the rest are themselves
    let Ok(d) = std::fs::read(dir.join("Coastal.bin")) else { return };
    let c = nfs_formats::sky::SkyBin::parse(&d).unwrap();
    assert_eq!((c.colour_index(0), c.colour_index(1), c.colour_index(2), c.colour_index(3)), (0, 1, 2, 2));
    // and the two records really are different skies
    assert_ne!(c.background_for(0), c.background_for(1));
}

/// A car's own `.loc`: the strings the showcase screens read (`nfs_formats::loc`).
#[test]
fn car_locale_files() {
    let r = need_game!();
    let dir = r.join("FEData/Locale");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("loc")))
        // `festrings.loc` and `hud.loc` are the game's own string tables, not a car's: their
        // header names seven language offsets but only the first is a `LOCL` block, so they are a
        // different arrangement and `locale::Strings` reads them instead.
        .filter(|p| !["festrings.loc", "hud.loc"].iter().any(|n| p.file_name().is_some_and(|f| f.eq_ignore_ascii_case(n))))
        .collect();
    files.sort();
    assert_eq!(files.len(), 97);
    let d = std::fs::read(dir.join("1948356.loc")).unwrap();
    let l = nfs_formats::loc::LocFile::parse(&d, 0).unwrap();
    assert_eq!(l.entries.len(), 44);
    assert_eq!(l.get(1), Some("American"));
    assert_eq!(l.get(2), Some("4 cylinder \"Boxer\""));
    assert_eq!(l.get(3), Some("1131cc"));
    assert_eq!(l.get(7), Some("84 mph"));
    assert_eq!(l.get(11), Some("3860 mm"));
    // an entry the car has nothing for is empty, not missing
    assert_eq!(l.get(8), None);
    // every shipped file parses, in every one of the seven languages the header declares. A few
    // are stubs — `1953356Cabrio13S.loc` is 172 bytes, seven blocks of nothing — so an empty list
    // is a valid answer; what is checked is that every one of the 99 files decodes.
    let mut filled = 0;
    for f in &files {
        let d = std::fs::read(f).unwrap();
        for lang in 0..7 {
            let l = nfs_formats::loc::LocFile::parse(&d, lang).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
            if lang == 0 && !l.entries.is_empty() {
                filled += 1;
            }
        }
    }
    assert!(filled >= 90, "only {filled} of {} .loc files carry strings", files.len());
}

/// The `.mad` container walks to the last byte of every shipped movie, and every video chunk's
/// bitstream is consumed to its payload length (`RE/notes/mad.md` §10).
#[test]
fn mad_frames_consume_their_payload() {
    let root = need_game!();
    let exe = match std::fs::read(root.join("nfs5.exe")) {
        Ok(d) => d,
        Err(_) => {
            eprintln!("skipping: nfs5.exe not found");
            return;
        }
    };
    let dir = root.join("FEData").join("Movies");
    let mut files = 0;
    for e in std::fs::read_dir(&dir).expect("Movies") {
        let p = e.expect("entry").path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mad")) {
            continue;
        }
        let d = std::fs::read(&p).expect("mad");
        // the chunk walk covers the file exactly
        let covered: usize = nfs_formats::mad::chunks(&d).map(|c| c.data.len()).sum();
        assert_eq!(covered, d.len(), "{p:?}: chunks cover {covered} of {}", d.len());

        let tables = nfs_formats::mad::MadTables::from_exe(&exe).expect("mad tables");
        let video: Vec<_> = nfs_formats::mad::chunks(&d).filter(|c| c.is_video()).take(120).map(|c| c.data.to_vec()).collect();
        let h = nfs_formats::mad::MadHeader::parse(&video[0]).expect("header");
        let mut dec = nfs_formats::mad::MadDecoder::new(tables, &h);
        let mut tight = 0usize;
        for c in &video {
            let payload = c.len() - nfs_formats::mad::MAD_HEADER_SIZE;
            let used = dec.decode_chunk(c).expect("decode");
            // the reader keeps its 32-bit window full, so `8·pos − avail` is the bit count with
            // `avail` 16…32 — up to four bytes ahead. A frame read at the wrong length lands well
            // outside that.
            assert!(used >= payload && used <= payload + 16, "{p:?}: consumed {used} of {payload}");
            tight += usize::from(used <= payload + 4);
        }
        // 3346 of the 3357 frames across the twelve movies land inside that bound; the threshold
        // guards against a regression rather than claiming perfection (`RE/notes/mad.md` §10)
        assert!(tight * 20 >= video.len() * 19, "{p:?}: only {tight} of {} frames landed exactly", video.len());
        files += 1;
    }
    assert!(files >= 10, "expected the twelve shipped movies, found {files}");
}
