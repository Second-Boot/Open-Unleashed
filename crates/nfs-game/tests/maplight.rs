//! The `.map` header colours and sections of the shipped tracks (`RE/notes/lighting.md`).
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_game::world::{MapLight, MapSection};

fn load(name: &str) -> Option<TrackAssets> {
    let paths = GamePaths::locate().ok()?;
    TrackAssets::load(&paths, name).ok()
}

#[test]
fn header_colours() {
    let Some(t) = load("monaco1") else {
        eprintln!("skipping: no game data");
        return;
    };
    let l = MapLight::from_map(&t.map);
    // every shipped file writes r = g = b, alpha first, and monaco is the darkest ambient with the
    // strongest tint
    assert!((l.ambient[0] - 0.304636).abs() < 1e-4, "{:?}", l.ambient);
    assert_eq!(l.ambient[0], l.ambient[2]);
    assert!((l.tint[0] - 1.801324).abs() < 1e-4);
    // `interiordiffuse` is in no shipped file, so it keeps the loader's 0.3 default
    assert_eq!(l.interior_diffuse, [0.3; 3]);
}

#[test]
fn sections_mark_the_covered_stretches() {
    let Some(t) = load("monaco1") else {
        eprintln!("skipping: no game data");
        return;
    };
    let secs = MapSection::all(&t.map);
    assert_eq!(secs.len(), 4, "count from the header");
    // the tunnel: the same 141..244 the enclosure test finds from the patch flags
    let tunnel = secs.iter().find(|s| s.start == 141).expect("the tunnel section");
    assert_eq!(tunnel.end, 244);
    // its six ids are `.env` entry names, and the covered variant differs from the open one
    let name = |id: u32| String::from_utf8_lossy(&id.to_le_bytes()).to_string();
    let names: Vec<String> = t.env_textures.iter().map(|e| format!("{:<4}", e.name.trim())).collect();
    for id in tunnel.ids {
        assert!(names.contains(&name(id)), "{} not in {:?}", name(id), names);
    }
    // layer 0 is the same map open or covered here — it is layer 2 (`texture3`) that the shipped
    // files vary between the two, e.g. coastal's section 0
    assert_eq!(tunnel.ids[0], tunnel.ids[3]);
    let Some(c) = load("coastal") else { return };
    let cs = MapSection::all(&c.map);
    assert!(cs.iter().any(|s| s.ids[2] != s.ids[5]), "some section varies layer 2 with covered");
}
