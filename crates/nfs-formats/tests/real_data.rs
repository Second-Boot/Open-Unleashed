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
