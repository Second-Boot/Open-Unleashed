//! Diagnostic: library article tags of a track vs GEOM ids used by a skill-test scene.
use nfs_assets::{track::TrackAssets, GamePaths};
use nfs_formats::scn::{ElementKind, Scene};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = GamePaths::locate().unwrap();
    let t = TrackAssets::load(&paths, &args[0]).unwrap();
    let g = t.build_geometry().unwrap();
    let mut tags: Vec<(u32, String)> = g.library.iter().map(|a| (a.tag, a.name.clone())).collect();
    tags.sort();
    println!("library articles: {}", tags.len());
    for (tag, name) in &tags {
        let b = tag.to_le_bytes();
        println!("  {tag:#010x} {:?} {name}", String::from_utf8_lossy(&b));
    }
    let scn = std::fs::read(paths.track_dir().join(&args[1])).unwrap();
    let s = Scene::parse(&String::from_utf8_lossy(&scn)).unwrap();
    for e in s.of_kind(&ElementKind::Geom) {
        let id = Scene::param_u32(e, 0, 1).unwrap_or(0);
        let b = id.to_le_bytes();
        println!("GEOM id {id:#010x} {:?} name={:?} params={:?}", String::from_utf8_lossy(&b), e.name, e.params_text.first());
    }
    for e in s.of_kind(&ElementKind::Trigger) {
        println!("TRIG name={:?} pos={:?} params={:?}", e.name, e.position, e.params_text);
    }
}
