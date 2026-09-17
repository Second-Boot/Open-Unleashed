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

/// `NFS5.CarLogo` takes its art entirely from the car's own catalogue record: the pack from +0x147
/// and the entry pair from +0x1F6, drawn as entries `2*pair` and `2*pair + 1`
/// (`NFS5.CarLogo::update` 0x4C1A63, `RE/notes/layouts.md` §4).
///
/// The `.fsh` packs confirm the reading from the other side: the two entries of a pair carry the
/// same four-character tag, and the second one's own signed x offset is the first one's minus the
/// first one's width (to the pixel on 48 of the 50 pairs) — the side-by-side placement the draw
/// performs (0x4C1CBE).
#[test]
fn car_logo_pairs_resolve_in_the_shipped_packs() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let art = paths.fe_data().join("Art");
    let cars = nfs_formats::fedata::parse_car_catalog(&std::fs::read(GamePaths::find_file(&paths.fe_data().join("Data"), "nfs5.car").expect("nfs5.car")).unwrap()).unwrap();
    assert_eq!(cars.len(), 109);

    let mut packs: std::collections::HashMap<String, nfs_formats::fsh::Fsh> = std::collections::HashMap::new();
    let mut with_logo = 0;
    for c in &cars {
        if c.logo.is_empty() {
            continue;
        }
        let file = format!("{}.fsh", c.logo);
        let pack = packs.entry(file.clone()).or_insert_with(|| {
            let p = GamePaths::find_file(&art, &file).expect("logo pack");
            nfs_formats::fsh::Fsh::parse(&std::fs::read(&p).unwrap()).expect("logo pack parses")
        });
        let base = c.logo_pair as usize * 2;
        // both halves exist — the original draws nothing unless they do (0x4C1B76)
        let (a, b) = (pack.entries.get(base).expect("first half"), pack.entries.get(base + 1).expect("second half"));
        assert_eq!(a.name, b.name, "{}: the pair's two entries belong to the same logo", c.display_name);
        let (ab, bb) = (&a.blocks[0], &b.blocks[0]);
        let (ax, bx) = (ab.misc[0] as i16, bb.misc[0] as i16);
        // the draw always butts the second half against the first (`x + widthA`, 0x4C1CBE); the
        // art's own authoring offsets agree to the pixel on 48 of the 50 pairs and are 1 and 2
        // pixels out on `finallogos2` pairs 24 and 20, so this only checks they agree that closely
        assert!((bx - (ax - ab.width as i16)).abs() <= 2, "{}: the second half sits beside the first ({bx} vs {ax} - {})", c.display_name, ab.width);
        assert_eq!(ab.misc[1], bb.misc[1], "{}: both halves share a baseline", c.display_name);
        with_logo += 1;
    }
    assert_eq!(with_logo, 93); // 64 cars use finallogos2, 29 finallogos1, 16 have no logo at all
    assert_eq!(packs["finallogos1.fsh"].entries.len(), 42);
    assert_eq!(packs["finallogos2.fsh"].entries.len(), 58);

    // the '96 911 Carrera Coupé (993) — pair 17 of `finallogos1`, the `Carr` script
    let c = cars.iter().find(|c| c.display_name.contains("'96 911 Carrera Coup")).expect("993 carrera");
    assert_eq!((c.logo.as_str(), c.logo_pair), ("finallogos1", 17));
    assert_eq!(packs["finallogos1.fsh"].entries[34].name.trim(), "Carr");
}

/// `FEData/Controls/*.ctr` — the widget class templates (`RE/notes/controls.md`). Every shipped
/// file parses, and the three the port leans on say what the disassembly said they would.
#[test]
fn the_control_templates_carry_the_class_defaults() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lib = nfs_game::layout::load_controls(&paths);
    assert_eq!(lib.len(), 72, "one `[control]` per shipped .ctr, `Control Template.txt` aside");

    // the list box: the panel is the `selectionbox` part, one pixel below the widget's own
    // position and as wide and tall as the two keys the .lay carries (`FeListBox::draw` 0x4C64FE)
    let lb = lib.get("NFS5.ListBox").expect("ListBox.ctr");
    assert_eq!(lb.value("size"), Some("data.width,data.selectionboxmaxheight"));
    let sb = lb.part("selectionbox").expect("selectionbox part");
    assert_eq!(sb.kind(), nfs_formats::ctr::PartKind::Rect);
    assert_eq!(sb.value("pos"), Some("0,1"));
    assert_eq!(sb.value("size"), Some("data.width,data.selectionboxmaxheight"));
    // the three colours the row loop uses (0x4C6722…0x4C67A6); the code's own fallbacks have the
    // two highlights the other way round, so the template is what actually shows
    assert_eq!(lb.default_of("textcolor"), Some("0x00000000"));
    assert_eq!(lb.default_of("highlightcolorhover"), Some("0x00FFBB00"));
    assert_eq!(lb.default_of("highlightcolorselected"), Some("0x00BC0000"));
    assert_eq!(lb.default_of("fontpath"), Some("Univers8b.ffn"));

    // the header: it carries no geometry in any .lay because its class already has it
    let h = lib.get("NFS5.HEADER").expect("Header.ctr");
    assert_eq!(h.value("pos"), Some("6,4"));
    let label = h.part("label").expect("label part");
    assert_eq!(label.kind(), nfs_formats::ctr::PartKind::LocalizedText);
    assert_eq!(label.value("fontPathName"), Some("Univers17b.ffn"));
    assert_eq!(label.value("foreColor"), Some("0xffffffff"));

    // the car logo: `align` defaults to 0 and the class is 100×60, as every shipped instance says
    let cl = lib.get("NFS5.CarLogo").expect("CarLogo.ctr");
    assert_eq!(cl.default_of("align"), Some("0"));
    assert_eq!(cl.value("size"), Some("100,60"));

    // every expression in every template evaluates once its references are known
    let mut exprs = 0;
    for class in ["NFS5.GOTOBUTTON", "NFS5.AccessorWatch", "NFS5.ComboBoxSlider", "NFS5.ListBox"] {
        let c = lib.get(class).expect(class);
        for p in c.parts.values() {
            for key in ["pos", "size"] {
                let Some(v) = p.value(key).filter(|v| !v.is_empty()) else { continue };
                let all = |_: &str| Some(10.0);
                assert!(nfs_formats::ctr::eval_pair(v, &all).is_some(), "{class}.{}.{key} = {v}", p.name);
                exprs += 1;
            }
        }
    }
    assert!(exprs >= 8, "checked {exprs} expressions");
}

/// The track list on `testdriveoval.lay`: the widget carries only `pos`, `width` and
/// `selectionboxmaxheight`, and the rows are 14 px each, so the panel holds eight whole rows of the
/// port's own track list and the scrollbar takes 16 px off the width.
#[test]
fn a_list_box_is_sized_by_its_own_two_keys() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let path = GamePaths::find_file(&paths.fe_data().join("Layouts"), "testdriveoval.lay").expect("testdriveoval.lay");
    let bytes = std::fs::read(path).unwrap();
    let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let layout = nfs_formats::lay::Layout::parse(&text);
    let e = layout.of_class("NFS5.ListBox").next().expect("the track list");
    assert_eq!(e.get("accessor"), Some("Track1.Track"));
    assert_eq!((e.int("width"), e.int("selectionboxmaxheight")), (Some(191), Some(124)));
    // min(rows·14, 124): nine tracks fill 126 px, so the panel clamps and a scrollbar appears
    let rows = 9.0f32;
    assert!(rows * nfs_game::layout::LIST_ROW_H > 124.0);
    assert_eq!(191.0 - nfs_game::layout::LIST_SCROLLBAR_W, 175.0);
}

/// The parts lists group their rows under category headings, and which heading a part falls under
/// is `partCategoryString` 0x518D60 — with kinds 6 and 7 sharing one (`RE/notes/controls.md` §4).
#[test]
fn part_categories_map_to_their_headings() {
    use nfs_formats::career::part_category_string;
    assert_eq!(part_category_string(1), Some(2554)); // ENGINE
    assert_eq!(part_category_string(5), Some(2558)); // SHOCKS
    assert_eq!(part_category_string(6), Some(2559)); // WEIGHT REDUCTION
    assert_eq!(part_category_string(7), Some(2559)); // …and so is 7
    assert_eq!(part_category_string(8), Some(2560)); // SWAY BARS
    assert_eq!(part_category_string(9), Some(2561)); // TIRES
    assert_eq!(part_category_string(10), Some(2562)); // BODYWORK
    assert_eq!(part_category_string(0), None);
    assert_eq!(part_category_string(11), None);

    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // the headings are real strings, and the two that share one really do
    let path = GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").expect("FEStrings.csv");
    let strings = nfs_formats::locale::Strings::parse(&std::fs::read(path).unwrap(), 0);
    assert_eq!(strings.get(2554), Some("ENGINE"));
    assert_eq!(strings.get(2559), Some("WEIGHT REDUCTION"));
    assert_eq!(strings.get(2562), Some("BODYWORK"));

    // a heading is 13 px and so is a row, which is why a parts list's rows are not all the same
    assert_eq!(nfs_game::layout::PART_HEADER_H, 13.0);
    assert_eq!(nfs_game::layout::ListRow::Header("ENGINE".into()).height(), 13.0);
    assert_eq!(nfs_game::layout::ListRow::Text("ALPS".into()).height(), nfs_game::layout::LIST_ROW_H);
}
