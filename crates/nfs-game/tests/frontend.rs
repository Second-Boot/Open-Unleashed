//! The front end's screens against the shipped `FEData/Layouts` (`RE/notes/layouts.md`).
use nfs_assets::GamePaths;

/// The front end's own controls, read from the shipped `QuickRace.lay`
/// (`RE/notes/layouts.md` §2.1): three combo sliders bound to accessors and three buttons that
/// lead to other screens.
#[test]
fn quick_race_screen_exposes_its_controls() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let path = GamePaths::find_file(&paths.fe_data().join("Layouts"), "QuickRace.lay").expect("QuickRace.lay");
    let bytes = std::fs::read(path).unwrap();
    let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let layout = nfs_formats::lay::Layout::parse(&text);
    let accessors: Vec<&str> = layout.elements.iter().filter(|e| e.class == "NFS5.ComboBoxSlider").filter_map(|e| e.get("accessor")).collect();
    assert_eq!(accessors, ["Track1.Track(SHORT)", "OpponentCar.Car(SHORT)", "Game.NumOpponents", "zMergedList.Car(SHORT)"]);
    let dests: Vec<&str> = layout.elements.iter().filter(|e| e.class.contains("BUTTON")).filter_map(|e| e.get("destination")).collect();
    assert_eq!(dests, ["garage", "location", "settings", "carcompare"]);
    // the screen says a race may be started from it
    assert_eq!(layout.screen().and_then(|s| s.int("canplaygame")), Some(1));
}
