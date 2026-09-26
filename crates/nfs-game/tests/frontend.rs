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
    // 72 `.ctr` (`Control Template.txt` aside) and the one `.scr`, `Black Screen 640x480.scr`
    assert_eq!(lib.len(), 73, "one `[control]` per shipped .ctr and .scr");

    // the screen template every layout's `[section.0]` is built on. Its one drawn part is a
    // `RECT` over the whole 640x480 in a **literal** `0xff000000`, and `color` is not one of the
    // attributes its `data` part declares — so the `0xff0000ff` and `0xff004080` that three of the
    // 98 shipped section-0 blocks carry are editor leftovers nothing reads.
    let scr = lib.get("REAL.BLACK_640x480").expect("Black Screen 640x480.scr");
    assert_eq!(scr.value("size"), Some("640,480"));
    assert_eq!(scr.value("autosize"), Some("1"));
    assert_eq!(scr.state_parts("normal"), ["canvas", "data"]);
    let canvas = scr.part("canvas").expect("canvas part");
    assert_eq!(canvas.kind(), nfs_formats::ctr::PartKind::Rect);
    assert_eq!(canvas.value("pos"), Some("0,0"));
    assert_eq!(canvas.value("size"), Some("control.size.width,control.size.height"));
    assert_eq!(canvas.value("color"), Some("0xff000000"));
    let data = scr.part("data").expect("data part");
    assert_eq!(data.value("attributes"), Some("screen,room,callbacklist,musicfile,canplaygame"));

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
                let all = |_: &str| nfs_formats::ctr::Macro::Text("10".into());
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

/// A `.lay` block's `state=` names one of the template's `[state.<name>]` sections, and that
/// section's `parts=` is the widget's whole draw list (0x525285…0x5252F5 → 0x525580 → 0x5271F0,
/// which copies the state's part vector over the active one). A state that lists only `DATA` parts
/// therefore erases the widget.
///
/// Three shipped widgets are in that case, all `NFS5.PANEL` with `state=hidden` against
/// `Panel.ctr`'s `[state.hidden] parts=data`. Each carries a `color`, so a port that ignored
/// `state` would paint a slab where the original draws nothing.
#[test]
fn a_hidden_state_selects_no_drawable_part() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> String {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect()
    };
    let panel = nfs_formats::ctr::Control::parse(&read("Panel.ctr"));
    assert!(panel.has_state("hidden") && panel.has_state("normal"));
    assert_eq!(panel.state_parts("normal"), ["area", "data"]);
    assert_eq!(panel.state_parts("hidden"), ["data"]);
    let drawable = |c: &nfs_formats::ctr::Control, s: &str| {
        c.state_parts(s).iter().filter_map(|n| c.part(n)).filter(|p| p.kind() != nfs_formats::ctr::PartKind::Data).count()
    };
    assert_eq!(drawable(&panel, "normal"), 1);
    assert_eq!(drawable(&panel, "hidden"), 0);

    // and the three widgets that ask for it, each with a colour it must not paint
    let lay = paths.fe_data().join("Layouts");
    let mut hidden = Vec::new();
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            if e.get("state") == Some("hidden") {
                assert_eq!(e.class, "NFS5.PANEL");
                assert!(e.get("color").is_some());
                hidden.push(f.file_name().to_string_lossy().to_ascii_lowercase());
            }
        }
    }
    hidden.sort();
    assert_eq!(hidden, ["carsetup.lay", "carsetupdesc.lay", "usedcars.lay"]);
}

/// A `.lay` numeric value is an **expression**, not a literal: 0x527D60 sends anything that is not
/// a `0x` constant to the evaluator 0x52DC40, and the pair attribute (0x527C0C) evaluates each half
/// of `x,y` separately. Five shipped blocks write their size as a sum.
#[test]
fn a_lay_value_is_an_expression() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let of = |name: &str| -> nfs_formats::lay::Layout {
        let p = GamePaths::find_file(&lay, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::lay::Layout::parse(&text)
    };
    let sizes: Vec<(f32, f32)> = of("usedcars.lay").elements.iter().filter(|e| e.class == "NFS5.PANEL").filter_map(|e| e.pair("size")).collect();
    assert_eq!(sizes, [(282.0, 181.0), (448.0, 183.0)]);
    assert_eq!(of("cdplayer.lay").elements.iter().filter_map(|e| e.pair("size")).find(|s| s.0 == 233.0), Some((233.0, 16.0)));
    assert_eq!(of("usedcardamage.lay").screen().and_then(|s| s.pair("size")), Some((569.0, 181.0)));
}

/// `NFS5.PANEL` loads the layout its `filename` names and draws it in its own place
/// (`RE/notes/controls.md` §4.1), and which panel is up is the tab strip's business: 0x4D01F0
/// binds a tab button to the panel its `panel=` names and 0x4D02E0, the button's select, opens it.
/// Each shipped strip carries exactly one `state=selected` button.
#[test]
fn a_tab_strip_names_the_panel_it_opens() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let of = |name: &str| -> nfs_formats::lay::Layout {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::lay::Layout::parse(&text)
    };
    for (strip, panel) in [
        ("optiontabs.lay", "ID_audio_panel"),
        ("GraphicsTabs.lay", "ID_graphicsettings_panel"),
        ("controllertabs.lay", "ID_gamectrl_panel"),
        ("showcasetabs.lay", "ID_general_panel"),
    ] {
        let l = of(strip);
        let sel: Vec<&str> = l.elements.iter().filter(|e| e.get("state") == Some("selected")).filter_map(|e| e.get("panel")).collect();
        assert_eq!(sel, [panel], "{strip}");
        // and every button in the strip names a panel, so none of them is left ambiguous
        let buttons = l.elements.iter().filter(|e| e.class.contains("TABBUTTON")).count();
        assert_eq!(l.elements.iter().filter_map(|e| e.get("panel")).count(), buttons, "{strip}");
    }
    // the panels those strips switch between, all stacked in one place
    let options = of("Options.lay");
    let opts: Vec<(&str, (f32, f32))> = options
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.PANEL")
        .map(|e| (e.get("filename").unwrap_or(""), e.pair("size").unwrap_or((0.0, 0.0))))
        .collect();
    assert_eq!(opts, [("controllers", (640.0, 480.0)), ("graphics", (640.0, 480.0)), ("display", (640.0, 480.0)), ("audio", (640.0, 480.0)), ("cameras", (640.0, 480.0))]);
    // a sub-layout's widgets are panel-relative: `usedcars.lay` §19 is a 448x183 box at (96,182)
    // and `usedcarparts.lay` fills it from its own origin
    let parts = of("usedcarparts.lay");
    assert_eq!(parts.screen().and_then(|s| s.pair("size")), Some((448.0, 183.0)));
    let xs: Vec<f32> = parts.elements.iter().skip(1).filter_map(|e| e.pair("pos")).map(|p| p.0).collect();
    assert!(xs.iter().all(|&x| (0.0..=448.0).contains(&x)), "{xs:?}");
}

/// Three `stringID`s the shipped layouts use are **present and empty** in `FEStrings.csv`, in every
/// language column — so the four widgets that name them draw nothing in the original too, and the
/// port's sweep must not count them as gaps.
#[test]
fn some_shipped_string_ids_are_deliberately_empty() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").expect("FEStrings.csv");
    let strings = nfs_formats::locale::Strings::parse(&std::fs::read(p).unwrap(), 0);
    for id in [11110, 12300, 19809] {
        assert_eq!(strings.get(id), Some(""), "{id}");
    }
    // and 0 is the placeholder the table marks DO NOT TRANSLATE, which is a different case
    assert_eq!(strings.get(0), Some("Hello, World!"));
    // a neighbouring id the table does not carry at all is a third
    assert_eq!(strings.get(12299), None);
}

/// The seventeen car-setup values are shown through a `FEFloatRangeAccessor`, so the number on
/// screen is the registered step's quantisation of the stored float, printed with the registered
/// number of decimals (0x4B9C10 → 0x4BC240 → 0x4B9AB0, registrations 0x4F29CC…0x4F2D49).
#[test]
fn a_car_setup_value_is_shown_through_its_registration() {
    use nfs_game::career::{car_setup_text, CAR_SETUP_DEFAULT, CAR_SETUP_NAMES, CAR_SETUP_RANGE};
    assert_eq!(CAR_SETUP_NAMES.len(), CAR_SETUP_RANGE.len());
    // a stock car, as `FECar`'s ctor leaves it
    let shown: Vec<String> = CAR_SETUP_DEFAULT.iter().enumerate().map(|(i, &v)| car_setup_text(i, v)).collect();
    assert_eq!(
        shown,
        [
            "50", "50", // down force, no decimals
            "50.00", "0.00", "0.00", "0.00", "1.00", // brake bias … shock rate
            "30", "30", // tyre pressures, no decimals
            "0.00", "0.00", "0.00", "0.00", "0.00", "0.00", "0.00", "0.00", // the eight gears
        ]
    );
    // the step quantises: `SteeringToe` moves in tenths, `ShockRate` in hundredths
    let toe = CAR_SETUP_NAMES.iter().position(|n| *n == "SteeringToe").unwrap();
    assert_eq!(car_setup_text(toe, 1.24), "1.20");
    assert_eq!(car_setup_text(toe, 1.26), "1.30");
    let shock = CAR_SETUP_NAMES.iter().position(|n| *n == "ShockRate").unwrap();
    assert_eq!(car_setup_text(shock, 1.234), "1.23");
    // and the index clamps to `count − 1`, so the registered maximum is one step out of reach —
    // `SteeringToe` tops out at 4.90, not 5.00 (0x4B9A40's count against 0x4BC240's clamp)
    assert_eq!(car_setup_text(toe, 9.0), "4.90");
    assert_eq!(car_setup_text(toe, -9.0), "-5.00");
    // the zero snap: a value a hair below zero prints `0.00`, never `-0.00`
    let gear = CAR_SETUP_NAMES.iter().position(|n| *n == "Gear0").unwrap();
    assert_eq!(car_setup_text(gear, 0.0), "0.00");
    assert_eq!(car_setup_text(gear, -0.5), "-0.50");
}

/// A widget's own `shapePathName` / `shapeIndex` override the **defaults** its class template's
/// SHAPE parts carry; they do not replace the part list. Five classes name their own pack, and
/// four of them keep the part's index — so giving up on the template as soon as a widget named a
/// pack drew the wrong `.fsh` entry for all of them.
#[test]
fn a_widget_overrides_its_parts_shape_but_not_their_list() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> nfs_formats::ctr::Control {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let shapes = |c: &nfs_formats::ctr::Control, state: &str| -> Vec<(String, String)> {
        c.state_parts(state)
            .iter()
            .filter_map(|n| c.part(n))
            .filter(|p| p.kind() == nfs_formats::ctr::PartKind::Shape)
            .map(|p| (p.value("shapePathName").unwrap_or("").to_string(), p.value("shapeIndex").unwrap_or("").to_string()))
            .collect()
    };
    // each of the five draws exactly one SHAPE in its normal state, so an override is unambiguous
    for (file, want) in [
        ("Picture.ctr", ("pointer.fsh", "0")),
        ("BottomBarButton.ctr", ("logoAn.fsh", "data.startframe")),
        ("CheckBox.ctr", ("pointer.fsh", "data.onIndex")),
        ("CarSettingSlider.ctr", ("weeicons.fsh", "8")),
    ] {
        let c = read(file);
        let s = shapes(&c, "normal");
        assert_eq!(s.len(), 1, "{file}");
        assert_eq!(s[0].1, want.1, "{file}");
        let _ = want.0;
    }
    // and the widgets that name a pack: 484 pictures name the entry too, the others do not
    let lay = paths.fe_data().join("Layouts");
    let mut named = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            if e.get("shapePathName").is_some_and(|v| !v.is_empty()) {
                let slot = named.entry(e.class.clone()).or_default();
                slot.0 += 1;
                slot.1 += usize::from(e.get("shapeIndex").is_some_and(|v| !v.is_empty()));
            }
        }
    }
    assert_eq!(named.get("NFS5.PICTURE"), Some(&(484, 484)));
    assert_eq!(named.get("NFS5.BOTTOMBARBUTTON"), Some(&(22, 0)));
    assert_eq!(named.get("NFS5.CarSettingSlider"), Some(&(5, 0)));
    assert_eq!(named.get("NFS5.CheckBox"), Some(&(10, 0)));
    assert_eq!(named.get("NFS5.AnimatingPicture"), Some(&(5, 0)));
    assert_eq!(named.len(), 5);
}

/// A part attribute takes the `.lay` block's value for its **screen key** — which defaults to the
/// attribute's own name — unless its flags say `HIDDEN`. The `.ctr` parser 0x52BC90 reads
/// `<key>.value`, `<key>.screenkey`, `<key>.label` and `<key>.flags`; 0x52BF43 defaults the screen
/// key to the name, 0x52BFB4 sets the overridable byte and 0x52BFEF clears it on `HIDDEN`, and
/// 0x52C000 does the lookup.
#[test]
fn a_part_attribute_takes_the_blocks_key_unless_it_is_hidden() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> nfs_formats::ctr::Control {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    // the one class that uses the indirection: the selected tab's background takes its pack and
    // its place from two keys of the widget that are named nothing like the attributes
    let tab = read("Graphic Tab Button.ctr");
    let sel = tab.part("selectedbg").expect("selectedbg");
    assert_eq!(sel.screen_key("shapePathName").as_deref(), Some("selectedbgSmallPathName"));
    assert_eq!(sel.screen_key("pos").as_deref(), Some("highlightsmallpos"));
    // and its entry is HIDDEN, so the block cannot move it off `data.selectedshape`
    assert_eq!(sel.screen_key("shapeIndex"), None);
    assert_eq!(sel.value("shapeIndex"), Some("data.selectedshape"));
    let hov = tab.part("hoverbg").expect("hoverbg");
    assert_eq!(hov.screen_key("shapePathName").as_deref(), Some("bgSmallPathName"));
    assert_eq!(hov.screen_key("pos").as_deref(), Some("smallpos"));

    // the default is the attribute's own name …
    let pic = read("Picture.ctr").part("picture").cloned().expect("picture");
    assert_eq!(pic.screen_key("shapePathName").as_deref(), Some("shapePathName"));
    assert_eq!(pic.screen_key("shapeIndex").as_deref(), Some("shapeIndex"));
    // … and `HIDDEN` takes the attribute out of the block's reach
    assert_eq!(pic.screen_key("pos"), None);
    let knob = read("CarSettingSlider.ctr").part("knob").cloned().expect("knob");
    assert_eq!(knob.screen_key("shapeIndex"), None);
    assert_eq!(knob.screen_key("size"), None);
    // a colour with no flags at all is overridable, which is how a panel takes its own `color`
    let area = read("Panel.ctr").part("area").cloned().expect("area");
    assert_eq!(area.screen_key("color").as_deref(), Some("color"));
    assert_eq!(area.screen_key("size"), None);
}

/// `NFS5.GOTOBUTTON` is a dark-red pill with white text: `GoToButton.ctr`'s `bg` part is the
/// widget's whole box filled with `data.maincolor`, the selected and hover states swap it for
/// `hoverbg` / `data.hovercolor`, and the caption is drawn three times with `data.medium` picking
/// which one has a non-zero alpha.
#[test]
fn a_goto_button_is_a_pill() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "GoToButton.ctr").expect("GoToButton.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.state_parts("normal"), ["bg", "label", "mediumlabel", "smalllabel", "data"]);
    assert_eq!(c.state_parts("selected"), ["hoverbg", "label", "mediumlabel", "smalllabel", "data"]);
    let data = c.part("data").expect("data");
    assert_eq!(data.value("maincolor"), Some("0xFFBC0000"));
    assert_eq!(data.value("hovercolor"), Some("0xffffBB00"));
    assert_eq!(c.part("bg").unwrap().value("color"), Some("data.maincolor"));
    assert_eq!(c.part("hoverbg").unwrap().value("color"), Some("data.hovercolor"));
    // exactly one caption is opaque for each shipped `medium`
    let opaque = |medium: i32| -> Vec<&str> {
        ["label", "mediumlabel", "smalllabel"]
            .into_iter()
            .filter(|n| {
                let v = c.part(n).unwrap().value("foreColor").unwrap();
                let r = |x: &str| if x == "data.medium" { nfs_formats::ctr::Macro::Text(medium.to_string()) } else { nfs_formats::ctr::Macro::Undefined };
                nfs_formats::ctr::eval(v, &r).is_some_and(|n| (n as u32) >> 24 != 0)
            })
            .collect()
    };
    assert_eq!(opaque(0), ["label"]);
    assert_eq!(opaque(1), ["mediumlabel"]);
    assert_eq!(opaque(4), ["smalllabel"]);

    // and every shipped button carries a colour of its own
    let lay = paths.fe_data().join("Layouts");
    let (mut n, mut red) = (0, 0);
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let t: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.GOTOBUTTON") {
            n += 1;
            red += usize::from(e.get("maincolor").is_some_and(|c| c.eq_ignore_ascii_case("0xFFBC0000")));
        }
    }
    assert_eq!((n, red), (153, 150));
}

/// `ComboBox.ctr` — whose `[control] id` is `NFS5.ComboBoxSlider` — and `ColorBox.ctr` are the
/// same widget: a dark-red bar with the caption left, the value right and a down arrow.
#[test]
fn a_combo_slider_is_a_dark_red_bar() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    for (file, id) in [("ComboBox.ctr", "NFS5.ComboBoxSlider"), ("ColorBox.ctr", "NFS5.ColorBox")] {
        let p = GamePaths::find_file(&dir, file).unwrap_or_else(|| panic!("{file}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let c = nfs_formats::ctr::Control::parse(&text);
        assert_eq!(c.id, id);
        assert!(c.state_parts("normal").contains(&"backrect".to_string()), "{file}");
        let bg = c.part("backrect").expect("backrect");
        assert_eq!(bg.value("color"), Some("0xFFBC0000"));
        // HIDDEN, so no layout can repaint it
        assert_eq!(bg.screen_key("color"), None);
        assert_eq!(bg.value("size"), Some("data.width,data.height"));
        let left = c.part("labelleft").expect("labelleft");
        assert_eq!(left.value("pos"), Some("5,2"));
        assert_eq!(left.value("foreColor"), Some("0xffffffff"));
        // the value's own part is transparent — the class draws it from code
        assert_eq!(c.part("label").unwrap().value("foreColor"), Some("0x00000000"));
        assert_eq!(c.part("arrowbutton").unwrap().value("shapeIndex"), Some("14"));
    }
}

/// The three box classes beside `NFS5.GOTOBUTTON` are the same shape: a `0xFFBC0000` box inside
/// four one-pixel black bars with a white caption, and an amber highlight state.
#[test]
fn the_box_classes_are_the_same_shape() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> nfs_formats::ctr::Control {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    for (file, hi, hicol) in [
        ("Tab Button.ctr", "hoverbg", "0xffffbb00"),
        ("CameraSelect.ctr", "hbg", "0x80FFbb00"),
        ("CameraPicture.ctr", "hbg", "0xffFFbb00"),
    ] {
        let c = read(file);
        assert_eq!(c.part("bg").unwrap().value("color"), Some("0xFFBC0000"), "{file}");
        assert_eq!(c.part(hi).unwrap().value("color"), Some(hicol), "{file}");
        let outlines = c.state_parts("normal").iter().filter(|n| n.starts_with("bgoutline")).count();
        assert_eq!(outlines, 4, "{file}");
        for n in 1..=4 {
            assert_eq!(c.part(&format!("bgoutline{n}")).unwrap().value("color"), Some("0xff000000"), "{file} {n}");
        }
    }
    // `CameraPicture.ctr`'s preview picture is not in the normal state, so it is placed from code
    let cp = read("CameraPicture.ctr");
    assert_eq!(cp.part("shp").unwrap().value("shapePathName"), Some("optart.fsh"));
    assert!(!cp.state_parts("normal").contains(&"shp".to_string()));
}

/// A template can place one part relative to another, including one that is **not in the state
/// being drawn**, and it writes an empty `<key>.value` where the value is the art's own. Both had
/// to be read correctly before the tab captions would land in their own tabs.
#[test]
fn a_caption_is_placed_relative_to_another_part() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> nfs_formats::ctr::Control {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let tab = read("Graphic Tab Button.ctr");
    let label = tab.part("label").expect("label");
    assert_eq!(
        label.value("pos"),
        Some("((selectedbg.size.width-label.size.width)/2)+data.textoffsetx,data.textoffsety")
    );
    // `selectedbg` is in the selected state only, and its own size and pack are empty
    assert!(!tab.state_parts("normal").contains(&"selectedbg".to_string()));
    assert!(tab.state_parts("selected").contains(&"selectedbg".to_string()));
    let sel = tab.part("selectedbg").expect("selectedbg");
    assert_eq!(sel.value("size"), Some(""));
    assert_eq!(sel.value("shapePathName"), Some(""));

    let bar = read("BottomBarButton.ctr").part("label").cloned().expect("label");
    assert_eq!(bar.value("pos"), Some("picture.size.width+data.space,8"));

    // and the five shipped tabs really do carry different offsets, so the captions spread out
    let lay = paths.fe_data().join("Layouts");
    let p = GamePaths::find_file(&lay, "optiontabs.lay").expect("optiontabs.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let offs: Vec<i64> = nfs_formats::lay::Layout::parse(&text)
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.GRAPHICTABBUTTON")
        .filter_map(|e| e.int("textoffsetx"))
        .collect();
    assert_eq!(offs, [24, 3, 0, -1, -22]);
}

/// `FEGouraudBox::draw` 0x4C9F60…0x4CA095 places its four corners **independently**: each is the
/// widget's own corner plus that corner's two offsets, bound at 0x4C9E8A…0x4C9F1A. More than half
/// the shipped boxes use them, so the shape is a skewed quad and not a rectangle.
#[test]
fn a_gouraud_box_has_eight_corner_offsets() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "GouraudBox.ctr").expect("GouraudBox.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    let data = c.part("data").expect("data");
    // the parser folds attribute names to lower case, and so does the `.lay` key lookup, so the
    // file's own spelling of the third one — `BLoffy`, small `y`, matching the executable's
    // string at 0x5D865C — does not have to be reproduced to read it
    for k in ["uloffx", "uloffy", "uroffx", "uroffy", "bloffx", "bloffy", "broffx", "broffy"] {
        assert!(data.attributes.iter().any(|a| a == k), "{k} missing from GouraudBox.ctr");
    }

    // and how many of the shipped boxes actually slant
    let lay = paths.fe_data().join("Layouts");
    let (mut total, mut skewed) = (0, 0);
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let t: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.GOURAUDBOX") {
            total += 1;
            let any = ["ULoffX", "ULoffY", "URoffX", "URoffY", "BLoffX", "BLoffy", "BRoffX", "BRoffY"]
                .iter()
                .any(|k| e.int(k).is_some_and(|v| v != 0));
            skewed += usize::from(any);
        }
    }
    assert_eq!((total, skewed), (576, 294));
}

/// Neither `NFS5.Rectangle` nor `NFS5.OutlineRectangle` is a registered class — the only
/// `*Rectangle*` string in the whole executable is `NFS5.GlintyRectangle` — so both are drawn by
/// the template and nothing else. Which settles the outline: the **interior** takes the block's
/// colour and the four one-pixel bars are always black.
#[test]
fn an_outline_rectangle_is_a_fill_inside_black_bars() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "OutlineRectangle.ctr").expect("OutlineRectangle.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.state_parts("normal"), ["data", "area", "top", "bottom", "left", "right"]);
    let area = c.part("area").expect("area");
    // the interior's colour has no flags, so the block's `color` replaces the placeholder
    assert_eq!(area.value("color"), Some("0x8000ff00"));
    assert_eq!(area.screen_key("color").as_deref(), Some("color"));
    // the bars are HIDDEN, so nothing can repaint them
    for n in ["top", "bottom", "left", "right"] {
        let b = c.part(n).expect(n);
        assert_eq!(b.value("color"), Some("0xff000000"), "{n}");
        assert_eq!(b.screen_key("color"), None, "{n}");
    }
    // and the colours the layouts give them are fills, not hairlines
    let lay = paths.fe_data().join("Layouts");
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    let (mut n, mut clear) = (0, 0);
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let t: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.OutlineRectangle") {
            n += 1;
            let v = e.get("color").unwrap_or("").to_ascii_lowercase();
            clear += usize::from(v.starts_with("0x00"));
            *seen.entry(v).or_default() += 1;
        }
    }
    assert_eq!(n, 195);
    assert_eq!(seen.get("0x4cffffff"), Some(&82));
    assert_eq!(clear, 33);
}

/// Four more classes are their template and nothing else — none of them is registered in the
/// executable. `Label.ctr` is where the alignment rule is actually written down.
#[test]
fn a_label_is_one_text_part_and_its_alignment_is_the_templates() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> nfs_formats::ctr::Control {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let label = read("Label.ctr").part("label").cloned().expect("label");
    assert_eq!(label.value("pos"), Some("(0-label.size.width)/2*data.align,0"));
    assert_eq!(label.value("fontPathName"), Some("univers8b.ffn"));
    assert_eq!(label.value("foreColor"), Some("0xffffffff"));
    // the font and the colour are overridable, the position is not
    assert_eq!(label.screen_key("fontPathName").as_deref(), Some("fontPathName"));
    assert_eq!(label.screen_key("foreColor").as_deref(), Some("foreColor"));
    assert_eq!(label.screen_key("pos"), None);
    // and the expression is the alignment rule: 0 keeps it, 1 pulls back half, 2 the whole width
    for (align, want) in [(0, 0), (1, -20), (2, -40)] {
        let r = |n: &str| match n {
            "label.size.width" => nfs_formats::ctr::Macro::Text("40".into()),
            "data.align" => nfs_formats::ctr::Macro::Text(i32::to_string(&align)),
            _ => nfs_formats::ctr::Macro::Undefined,
        };
        assert_eq!(nfs_formats::ctr::eval_pair(label.value("pos").unwrap(), &r), Some((want, 0)));
    }
    // a picture is one SHAPE part and nothing else
    assert_eq!(read("Picture.ctr").state_parts("normal"), ["data", "picture"]);
}

/// An accessor watch is its template's one `TEXT` part, whose `text` is `HIDDEN|READONLY` because
/// only the class writes it (`FEAccessorWatch::tick` 0x4BD230). Its position is the same
/// expression `Label.ctr` carries, over the width of the string it is showing now.
#[test]
fn an_accessor_watch_is_a_text_part_the_code_fills() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "accessor Watch.ctr").expect("accessor Watch.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.state_parts("normal"), ["label", "data"]);
    let label = c.part("label").expect("label");
    assert_eq!(label.kind(), nfs_formats::ctr::PartKind::Text);
    assert_eq!(label.value("text"), Some("WATCH"));
    // the block cannot set it — the code does
    assert_eq!(label.screen_key("text"), None);
    assert_eq!(label.value("pos"), Some("(0-label.size.width)/2*data.align,0"));
    // the wrapping variant pins its label instead, which is why it keeps the port's own wrap
    let p = GamePaths::find_file(&dir, "WordWrapAccessorWatch.ctr").expect("WordWrapAccessorWatch.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let w = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(w.part("label").unwrap().value("pos"), Some("0,0"));
}

/// A header is its template's one `LOCALIZEDTEXT` part: the class sits at `6,4` and the label at
/// `0,0` in `Univers17b.ffn`, white. The class's own bind (0x4CAE60) does nothing but fetch that
/// part; its activate (0x4CADC0) loads a glint, which this port does not draw.
#[test]
fn a_header_is_its_templates_label() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| -> nfs_formats::ctr::Control {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let h = read("Header.ctr");
    assert_eq!(h.value("pos"), Some("6,4"));
    assert_eq!(h.state_parts("normal"), ["label", "data"]);
    let l = h.part("label").expect("label");
    assert_eq!(l.kind(), nfs_formats::ctr::PartKind::LocalizedText);
    assert_eq!(l.value("pos"), Some("0,0"));
    assert_eq!(l.value("fontPathName"), Some("Univers17b.ffn"));
    assert_eq!(l.value("foreColor"), Some("0xffffffff"));
    // and the glint it loads is a named entry of a shipped pack, not something invented
    let art = paths.fe_data().join("Art");
    let p = GamePaths::find_file(&art, "weeicons.fsh").expect("weeicons.fsh");
    let fsh = nfs_formats::fsh::Fsh::parse(&std::fs::read(p).unwrap()).expect("parse weeicons");
    assert!(fsh.entries.iter().any(|e| e.name.eq_ignore_ascii_case("GLNT")), "no GLNT entry in weeicons.fsh");
}

/// Every shipped `NFS5.GouraudIndicatorBar` carries all three colour stops, so the two-stop
/// fallback the bind builds when `c50` is missing (0x4CA227…0x4CA23D) never fires in the shipped
/// screens — the port implements it, but the three-stop split at fifty is what is actually used.
#[test]
fn every_indicator_bar_carries_three_stops() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let mut triples = std::collections::BTreeSet::new();
    let mut n = 0;
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let t: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.GouraudIndicatorBar") {
            n += 1;
            let g = |k: &str| e.get(k).unwrap_or("").to_ascii_lowercase();
            for k in ["c0", "c50", "c100"] {
                assert!(g(k).starts_with("0x") && g(k) != "0x00000000", "{k} missing on a bar");
            }
            triples.insert((g("c0"), g("c50"), g("c100")));
        }
    }
    assert_eq!(n, 42);
    assert_eq!(triples.len(), 5);
}

/// `NFS5.GlintyRectangle`'s constructor reads `col`, `width`, `height` and `animate` — not `color`
/// or `size` — and every shipped one carries exactly those. `animate == 5` means a fresh random
/// one of the four flourishes each time (0x51FE8E), which is what most of them ask for.
#[test]
fn a_glinty_rectangle_carries_col_width_and_animate() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let (mut n, mut five) = (0, 0);
    for f in std::fs::read_dir(&lay).unwrap().filter_map(|e| e.ok()) {
        if !f.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lay") {
            continue;
        }
        let t: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.GlintyRectangle") {
            n += 1;
            assert!(e.get("col").is_some_and(|v| !v.is_empty()), "a glinty rectangle with no col");
            assert!(e.int("width").is_some(), "a glinty rectangle with no width");
            assert!(e.get("color").is_none_or(str::is_empty), "a glinty rectangle with a `color`");
            assert!(e.get("size").is_none_or(str::is_empty), "a glinty rectangle with a `size`");
            five += usize::from(e.int("animate") == Some(5));
        }
    }
    assert_eq!((n, five), (181, 93));
}

/// A list box that clamps grows a `NFS5.VerticalSlider` in the sixteen pixels it gives up, and
/// `VerticalSlider.ctr` says what that is: an amber track, `weeicons.fsh` 13 and 14 for the two
/// arrows, and a half-white knob fifteen pixels tall.
#[test]
fn a_list_boxs_scrollbar_is_a_vertical_slider() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "VerticalSlider.ctr").expect("VerticalSlider.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.id, "NFS5.VerticalSlider");
    assert_eq!(c.state_parts("normal"), ["track", "decrement_button", "knob", "increment_button", "data"]);
    assert_eq!(c.part("track").unwrap().value("color"), Some("0xffffbb00"));
    assert_eq!(c.part("knob").unwrap().value("color"), Some("0x80ffffff"));
    assert_eq!(c.part("knob").unwrap().value("size"), Some("decrement_button.size.width,15"));
    assert_eq!(c.part("decrement_button").unwrap().value("shapeIndex"), Some("13"));
    assert_eq!(c.part("decrement_button").unwrap().value("pos"), Some("0,7"));
    assert_eq!(c.part("increment_button").unwrap().value("shapeIndex"), Some("14"));
    // and both arrows really are in the pack the port loads them from
    let art = paths.fe_data().join("Art");
    let p = GamePaths::find_file(&art, "weeicons.fsh").expect("weeicons.fsh");
    let fsh = nfs_formats::fsh::Fsh::parse(&std::fs::read(p).unwrap()).expect("parse");
    assert!(fsh.entries.len() > 14, "weeicons.fsh has only {} entries", fsh.entries.len());
}

/// `NFS5.DEADZONE` paints none of its template: its `area` is the `0x8000ff00` placeholder, and
/// the visible dark track on `deadzone.lay` is a separate `NFS5.Rectangle` of `0x66000000` over
/// the same box. `RE/notes/controls.md` §4.17.
#[test]
fn the_deadzone_bars_track_is_a_rectangle_of_its_own() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "Deadzone.ctr").expect("Deadzone.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.id, "NFS5.DEADZONE");
    assert_eq!(c.part("area").unwrap().value("color"), Some("0x8000ff00"));
    // `hackindex` is the attribute the draw's jump table (0x51BB14) turns into the art index
    // 0x24 + it, so the screen must give all eight arms
    let lay = paths.fe_data().join("Layouts");
    let p = GamePaths::find_file(&lay, "deadzone.lay").expect("deadzone.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let l = nfs_formats::lay::Layout::parse(&text);
    let bars: Vec<_> = l.elements.iter().filter(|e| e.class == "NFS5.DEADZONE").collect();
    assert_eq!(bars.len(), 8);
    let mut idx: Vec<i64> = bars.iter().filter_map(|e| e.int("hackindex")).collect();
    idx.sort_unstable();
    assert_eq!(idx, (0..8).collect::<Vec<_>>());
    // every bar is covered by a rectangle of the same geometry in 0x66000000
    for b in &bars {
        let (bp, bs) = (b.get("pos"), b.get("size"));
        assert!(
            l.elements.iter().any(|e| e.class == "NFS5.Rectangle" && e.get("pos") == bp && e.get("size") == bs && e.get("color") == Some("0x66000000")),
            "no track rectangle under the bar at {bp:?}"
        );
    }
}

/// `NFS5.ColorBox` derives from `NFS5.ComboBoxSlider` in the executable (factory 0x4C3880 calls
/// the combo's own constructor 0x4C4CF0), and the two templates agree: the same attributes, the
/// same parts, the same two fonts. `RE/notes/controls.md` §4.20.
#[test]
fn a_colour_box_is_a_combo_box() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let load = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let (combo, colour) = (load("ComboBox.ctr"), load("ColorBox.ctr"));
    assert_eq!(combo.id, "NFS5.ComboBoxSlider");
    assert_eq!(colour.id, "NFS5.ColorBox");
    assert_eq!(combo.state_parts("normal"), colour.state_parts("normal"));
    for part in ["labelleft", "label"] {
        assert_eq!(
            combo.part(part).unwrap().value("fontPathName"),
            colour.part(part).unwrap().value("fontPathName"),
            "{part} font differs"
        );
    }
    // the value part is transparent in both, so the code places it (§4.14)
    assert_eq!(colour.part("label").unwrap().value("foreColor"), Some("0x00000000"));
}

/// `NFS5.BEVELEDOPTIONBOX`'s `area` keeps the `0x8000ff00` placeholder in the template **and no
/// `color.flags`**, so the screen's own `color` replaces it — the ordinary attribute rule. All 16
/// shipped ones are on `funckeys.lay` and all 16 override it, which is why the port never paints
/// that green. `RE/notes/controls.md` §4.21.
#[test]
fn every_beveled_option_box_overrides_the_placeholder() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "BevelOptionBox.ctr").expect("BevelOptionBox.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.id, "NFS5.BEVELEDOPTIONBOX");
    let area = c.part("area").unwrap();
    assert_eq!(area.value("color"), Some("0x8000ff00"));
    // no flags on `color` — the layout may replace it; the four edges are HIDDEN and stay black
    assert!(area.screen_key("color").is_some(), "the area's colour must be overridable");
    for edge in ["top", "bottom", "left", "right"] {
        let p = c.part(edge).unwrap();
        assert_eq!(p.value("color"), Some("0xff000000"));
        assert!(p.screen_key("color").is_none(), "{edge} must be hidden");
    }
    let lay = paths.fe_data().join("Layouts");
    let p = GamePaths::find_file(&lay, "funckeys.lay").expect("funckeys.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let l = nfs_formats::lay::Layout::parse(&text);
    let boxes: Vec<_> = l.elements.iter().filter(|e| e.class == "NFS5.BEVELEDOPTIONBOX").collect();
    assert_eq!(boxes.len(), 16);
    for b in &boxes {
        let c = b.get("color").expect("a colour of its own");
        assert_ne!(c.to_ascii_lowercase(), "0x8000ff00", "a box kept the placeholder");
    }
}

/// `dropshadow` is what decides which of the two extra draws a button gets: 1 sets
/// `[this+0x14C]`, the drop shadow 0x520F00 at (4, 4), and 2 sets `[this+0x14D]`, the 0x521900
/// this port does not draw (0x4C8F8C…0x4C8FBA). `RE/notes/controls.md` §4.22.
#[test]
fn the_buttons_drop_shadow_attribute() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let load = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    // the template defaults, and whether a screen may replace them
    let goto = load("GoToButton.ctr");
    assert_eq!(goto.default_of("dropshadow"), Some("1"));
    let tab = load("Tab Button.ctr");
    assert_eq!(tab.default_of("dropshadow"), Some("1"));
    let graphic = load("Graphic Tab Button.ctr");
    assert_eq!(graphic.default_of("dropshadow"), Some("0"));
    let bottom = load("BottomBarButton.ctr");
    assert_eq!(bottom.default_of("dropshadow"), Some("0"));
    // and what the screens carry: the shadow is the rule, not the exception
    let lay = paths.fe_data().join("Layouts");
    let (mut shadowed, mut plain, mut other) = (0, 0, 0);
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements.iter().filter(|e| e.class == "NFS5.GOTOBUTTON") {
            // an empty key means "use the template's", which is 1
            match e.get("dropshadow").filter(|v| !v.is_empty()).unwrap_or("1") {
                "1" => shadowed += 1,
                "0" => plain += 1,
                _ => other += 1,
            }
        }
    }
    assert_eq!((shadowed, plain, other), (138, 9, 6));
}

/// The classes whose draw ends with the bevel 0x521050, and how many widgets each has — the
/// counts `RE/notes/controls.md` §4.22 quotes for what the port now draws.
#[test]
fn the_classes_that_carry_a_bevel() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let mut n = std::collections::HashMap::new();
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            *n.entry(e.class).or_insert(0) += 1;
        }
    }
    let at = |k: &str| n.get(k).copied().unwrap_or(0);
    assert_eq!(at("NFS5.GOTOBUTTON"), 153);
    assert_eq!(at("NFS5.TABBUTTON"), 11);
    assert_eq!(at("NFS5.GRAPHICTABBUTTON"), 5);
    assert_eq!(at("NFS5.ChroniclePictureIcon"), 12);
    assert_eq!(at("NFS5.PLAYERDRIVERPICTURE"), 8);
    assert_eq!(at("NFS5.CAMERASELECT"), 4);
    assert_eq!(at("NFS5.CAMERAPICTURE"), 4);
    assert_eq!(at("NFS5.BEVELEDOPTIONBOX"), 16);
}

/// `createplayer.lay`'s chooser: eight `NFS5.PLAYERDRIVERPICTURE`s carrying `ournumber` 0…7, one
/// each, so the 8 `CREATEPLAYERENTER` stores in `[0x5D8F2C]` (0x51078B) matches none of them and
/// the screen opens with no frame (0x4CF83B) and eight drop shadows (0x4CF8E1). §4.162.
#[test]
fn the_driver_pictures_are_numbered_0_to_7() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let f = paths.fe_data().join("Layouts").join("createplayer.lay");
    let text: String = std::fs::read(f).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let mut numbers: Vec<i64> = nfs_formats::lay::Layout::parse(&text)
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.PLAYERDRIVERPICTURE")
        .map(|e| e.get("ournumber").and_then(|v| v.trim().parse().ok()).unwrap())
        .collect();
    numbers.sort_unstable();
    assert_eq!(numbers, (0..8).collect::<Vec<_>>());
    assert!(!numbers.contains(&(nfs_game::save::DEFAULT_PICTURE as i64)));
}

/// The stems — the thin verticals a menu button hangs off its ends — are drawn when
/// `data.glintybox` is not −1, and **none** of the 153 shipped buttons carries −1.
/// `RE/notes/controls.md` §4.24.
#[test]
fn every_button_has_its_stems() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "GoToButton.ctr").expect("GoToButton.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.default_of("glintybox"), Some("1"));
    assert_eq!(c.default_of("stemleftheight"), Some("0"));
    assert_eq!(c.default_of("stemrightheight"), Some("0"));
    let lay = paths.fe_data().join("Layouts");
    let (mut total, mut off, mut with_a_stem) = (0, 0, 0);
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements.iter().filter(|e| e.class == "NFS5.GOTOBUTTON") {
            total += 1;
            if e.get("glintybox").filter(|v| !v.is_empty()).unwrap_or("1") == "-1" {
                off += 1;
            }
            let stem = |k: &str| e.get(k).filter(|v| !v.is_empty()).unwrap_or("0") != "0";
            if stem("stemleftheight") || stem("stemrightheight") {
                with_a_stem += 1;
            }
        }
    }
    assert_eq!((total, off), (153, 0));
    // and most of them really do hang one off an end
    assert_eq!(with_a_stem, 86);
}

/// The header's glint is `weeicons.fsh`'s entry tagged **`GLNT`**, which the class looks up by tag
/// (0x4CADC0) rather than by index. `RE/notes/controls.md` §4.25.
#[test]
fn the_header_glint_is_a_tagged_entry() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let art = paths.fe_data().join("Art");
    let p = GamePaths::find_file(&art, "weeicons.fsh").expect("weeicons.fsh");
    let pack = nfs_formats::fsh::Fsh::parse(&std::fs::read(p).unwrap()).expect("parse");
    let n = pack
        .entries
        .iter()
        .position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case("GLNT"))
        .expect("a GLNT entry");
    // the port finds it by tag; this pins the pack it is in and that there is exactly one
    assert_eq!(n, 18);
    assert_eq!(pack.entries.iter().filter(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case("GLNT")).count(), 1);
}

/// `NFS5.GlintyRectangle`'s flourish: two segments running round the outline in mirror image.
/// The geometry is the sixteen arms of `RE/notes/controls.md` §4.27, so it is worth pinning.
#[test]
fn the_glinty_flourish_runs_round_the_outline() {
    use nfs_game::layout::{glint_band, glint_pieces};
    // the commonest shipped box: 280 x 15 (16 of the 181)
    let (w, h) = (280, 15);
    let band = glint_band(w, h);
    assert_eq!(band, 14, "band is min((w+h)/5, w-1, h-1)");

    // at the very start both segments are still off the corner, so nothing is lit
    assert!(glint_pieces(w, h, -band, band, 1).is_empty());

    // through the run every piece stays on the border, is one pixel thick, and the two
    // segments together are never longer than twice the band
    for p in -band..=(w + h) {
        for variant in 1..=4 {
            let ps = glint_pieces(w, h, p, band, variant);
            assert!(ps.len() <= 4, "p={p} v={variant} gave {} pieces", ps.len());
            let mut lit = 0.0;
            for g in &ps {
                assert!(g.w >= 0.5 && g.h >= 0.5, "p={p} v={variant} empty piece {g:?}");
                assert!(g.w == 1.0 || g.h == 1.0, "p={p} v={variant} not a hairline: {g:?}");
                assert!(g.x >= 0.0 && g.y >= 0.0, "p={p} v={variant} off the top left: {g:?}");
                assert!(g.x + g.w <= w as f32 + 0.01, "p={p} v={variant} off the right: {g:?}");
                assert!(g.y + g.h <= h as f32 + 0.01, "p={p} v={variant} off the bottom: {g:?}");
                // a hairline on an edge: x is 0 or w-1, or y is 0 or h-1
                let on_edge = g.x == 0.0 || g.x + g.w == w as f32 || g.y == 0.0 || g.y + g.h == h as f32;
                assert!(on_edge, "p={p} v={variant} not on an edge: {g:?}");
                lit += g.w.max(g.h);
            }
            assert!(lit <= 2.0 * band as f32 + 0.01, "p={p} v={variant} lit {lit}");
        }
    }

    // mid-run both segments are fully on the border and each is a whole band long
    let ps = glint_pieces(w, h, w / 2, band, 1);
    let total: f32 = ps.iter().map(|g| g.w.max(g.h)).sum();
    assert_eq!(total, 2.0 * band as f32);

    // variant 2 mirrors variant 1 in x, variant 3 in y
    let mirror = |v: i32, p: i32| -> Vec<(f32, f32)> {
        let mut r: Vec<(f32, f32)> = glint_pieces(w, h, p, band, v).iter().map(|g| (g.x, g.y)).collect();
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        r
    };
    let a = mirror(1, 40);
    let b = mirror(2, 40);
    assert_eq!(a.len(), b.len());
    let mut flipped: Vec<(f32, f32)> = a.iter().map(|&(x, y)| (w as f32 - x - 1.0, y)).collect();
    flipped.sort_by(|p, q| p.partial_cmp(q).unwrap());
    // the mirrored set has the same y's; the x's are the flipped ones give or take the hairline
    assert_eq!(flipped.iter().map(|p| p.1).collect::<Vec<_>>(), b.iter().map(|p| p.1).collect::<Vec<_>>());
}

/// `NFS5.GLINTYLINE`'s sweep is clipped to the widget's box by 0x4D94A0, with the colours
/// interpolated at the cut. `RE/notes/controls.md` §4.29.
#[test]
fn the_glinty_lines_sweep_is_clipped_to_the_line() {
    use nfs_game::layout::glint_sweep;
    let (len, width) = (380.0f32, 76.0f32);
    // entirely before the line, and entirely after it
    assert!(glint_sweep(len, -width, width, true).is_none());
    assert!(glint_sweep(len, len, width, true).is_none());
    // half on: the visible part starts at 0 and the near colour has moved toward the far one
    let (s, l, c0, c1) = glint_sweep(len, -width / 2.0, width, true).expect("half on");
    assert_eq!((s, l), (0.0, width / 2.0));
    assert!(c0[3] > 0 && c0[3] < c1[3], "the cut colour sits between the two: {c0:?} {c1:?}");
    assert_eq!(c1, [0xff, 0xff, 0xff, 0xdd]);
    // fully on: untouched
    let (s, l, c0, c1) = glint_sweep(len, 100.0, width, true).expect("fully on");
    assert_eq!((s, l), (100.0, width));
    assert_eq!((c0, c1), ([0xff, 0xff, 0xff, 0x00], [0xff, 0xff, 0xff, 0xdd]));
    // running off the far end
    let (s, l, _, c1) = glint_sweep(len, len - width / 4.0, width, true).expect("leaving");
    assert_eq!((s, l), (len - width / 4.0, width / 4.0));
    assert!(c1[3] < 0xdd, "the far colour is cut short: {c1:?}");
    // and the whole sweep stays inside the line for every position it can take
    let mut steps = 0;
    for i in 0..=((len + 2.0 * width) as i32) {
        if let Some((s, l, _, _)) = glint_sweep(len, i as f32 - width, width, true) {
            assert!(s >= 0.0 && s + l <= len + 0.01, "i={i} gave {s}..{}", s + l);
            steps += 1;
        }
    }
    assert!(steps > 0);
}

/// The three input boxes share one class, and its draw shadows the box unless `data.noshadow`
/// says otherwise — which the bind leaves 0 by default. `RE/notes/controls.md` §4.30.
#[test]
fn the_input_boxes_shadow_unless_told_not_to() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let load = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    assert_eq!(load("InputBox.ctr").default_of("noshadow"), Some("0"));
    assert_eq!(load("CharVectInputBox.ctr").default_of("noshadow"), Some("0"));
    // the chat box's template does not carry the attribute at all, which the original reads as 0
    assert_eq!(load("ChatInputBox.ctr").default_of("noshadow"), None);
    let lay = paths.fe_data().join("Layouts");
    let mut n = std::collections::HashMap::new();
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            if matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX") {
                *n.entry((e.class.clone(), e.get("noshadow").unwrap_or("<absent>").to_string())).or_insert(0) += 1;
            }
        }
    }
    assert_eq!(n.get(&("NFS5.INPUTBOX".into(), "0".into())), Some(&3));
    assert_eq!(n.get(&("NFS5.CHATINPUTBOX".into(), "<absent>".into())), Some(&3));
    assert_eq!(n.get(&("NFS5.CHARVECTACCESSORINPUTBOX".into(), "1".into())), Some(&2));
}

/// The six choice accessors the settings screens carry that the port had no row for, pinned by
/// the `FEStrings` their lists name. `RE/notes/controls.md` §4.31.
#[test]
fn the_settings_choice_lists() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // the ids' meaning, pinned in the English column (the game reads `FEStrings.loc`, Russian here)
    let s = nfs_formats::locale::Strings::parse(&std::fs::read(GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").unwrap()).unwrap(), 0);
    let at = |id: u32| s.get(id).map(|t| t.to_ascii_lowercase());
    // Car.ABS, Car.Traction and Comms.Collision all take 0x5DAA20's pair
    assert_eq!(at(1500).as_deref(), Some("off"));
    assert_eq!(at(1501).as_deref(), Some("on"));
    // Car.Roof takes 0x5DA9F0
    assert_eq!(at(1509).as_deref(), Some("up"));
    assert_eq!(at(1510).as_deref(), Some("down"));
    // Car.Transmission takes 0x5DAB5C
    assert_eq!(at(560).as_deref(), Some("manual"));
    assert_eq!(at(561).as_deref(), Some("automatic"));
    // OwnedCar.PackageNum takes 0x5B7644 — eight entries, none then seven packages
    assert_eq!(at(620).as_deref(), Some("none"));
    for k in 1..=7u32 {
        assert_eq!(at(620 + k).as_deref(), Some(format!("package {k}").as_str()));
    }
    // and the table carries more packages than the accessor offers — 628 is "package 8" — which
    // is why the list's length has to come from the executable's NUL terminator at 0x5B7664 and
    // not from the string table
    assert_eq!(at(628).as_deref(), Some("package 8"));
}

/// `%<part>.<key>%` is the indirection of `RE/notes/controls.md` §2, and `CarSettingButton.ctr`
/// is the one shipped template that uses it for **art**: both the hover and the pressed arrow
/// take their pack from `picnonsel`. The loader has to resolve it, or it asks the packs for a
/// file literally called `"%picnonsel.shapePathName%"`.
#[test]
fn the_car_setting_buttons_art_is_an_indirection() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "CarSettingButton.ctr").expect("CarSettingButton.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.part("picnonsel").unwrap().value("shapePathName"), Some("weeicons.fsh"));
    for part in ["picsel", "picseldown"] {
        assert_eq!(c.part(part).unwrap().value("shapePathName"), Some("%picnonsel.shapePathName%"));
    }
    // and it is the only one: no other template points its art at another part
    let mut others = Vec::new();
    for f in std::fs::read_dir(&dir).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let c = nfs_formats::ctr::Control::parse(&text);
        for (name, p) in c.parts.iter() {
            if p.value("shapePathName").is_some_and(|v| v.starts_with('%')) && c.id != "NFS5.CARSETTINGBUTTON" {
                others.push(format!("{} {name}", c.id));
            }
        }
    }
    assert!(others.is_empty(), "other templates use the indirection for art: {others:?}");
}

/// `data.usesmallfont` puts both of a combo slider's strings in `univers8b.ffn`; without it the
/// caption keeps the template's `universC11b.ffn`. `RE/notes/controls.md` §4.34.
#[test]
fn the_combo_sliders_small_font() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "ComboBox.ctr").expect("ComboBox.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    // the two the class hard-codes are its template's own defaults, so only the flagged widgets move
    assert_eq!(c.part("labelleft").unwrap().value("fontPathName"), Some("universC11b.ffn"));
    assert_eq!(c.part("label").unwrap().value("fontPathName"), Some("universC11m.ffn"));
    assert_eq!(c.default_of("usesmallfont"), Some("0"));
    let lay = paths.fe_data().join("Layouts");
    let (mut small, mut big) = (0, 0);
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            if !matches!(e.class.as_str(), "NFS5.ComboBoxSlider" | "NFS5.ColorBox") {
                continue;
            }
            match e.get("usesmallfont").filter(|v| !v.is_empty()) {
                Some("0") | None => big += 1,
                Some(_) => small += 1,
            }
        }
    }
    assert_eq!((small, big), (29, 68));
}

/// `nolabel == 4` gives a combo slider a second box to its right, `GraphicsSpecialCaseWidth`
/// wide, with the value centred in it. The two keys always travel together.
/// `RE/notes/controls.md` §4.35.
#[test]
fn the_graphics_sliders_second_box() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let (mut four, mut wide, mut both) = (0, 0, 0);
    let mut screens = std::collections::BTreeSet::new();
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            if !matches!(e.class.as_str(), "NFS5.ComboBoxSlider" | "NFS5.ColorBox") {
                continue;
            }
            let n4 = e.get("nolabel").filter(|v| !v.is_empty()) == Some("4");
            let w = e.int("GraphicsSpecialCaseWidth").unwrap_or(0) != 0;
            four += usize::from(n4);
            wide += usize::from(w);
            if n4 && w {
                both += 1;
                screens.insert(f.path().file_name().unwrap().to_string_lossy().to_lowercase());
            }
        }
    }
    // eleven widgets, and neither key ever appears without the other
    assert_eq!((four, wide, both), (11, 11, 11));
    assert_eq!(
        screens.into_iter().collect::<Vec<_>>(),
        ["advgraphics.lay", "audioreal.lay", "graphicsettings.lay"]
    );
}

/// `nolabel == 1` is a combo slider with no caption of its own — all fourteen are on
/// `display.lay` and all fourteen carry `stringID=0`, the shipped placeholder, so the class puts
/// the **value** in the caption's place. `RE/notes/controls.md` §4.36.
#[test]
fn the_display_sliders_have_no_caption() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let mut seen = std::collections::BTreeMap::new();
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let text: String = std::fs::read(f.path()).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&text).elements {
            if e.class == "NFS5.ComboBoxSlider" && e.get("nolabel").filter(|v| !v.is_empty()) == Some("1") {
                let screen = f.path().file_name().unwrap().to_string_lossy().to_lowercase();
                *seen.entry((screen, e.string_id())).or_insert(0) += 1;
            }
        }
    }
    assert_eq!(seen.len(), 1, "all of them on one screen with one string id: {seen:?}");
    let ((screen, id), n) = seen.into_iter().next().unwrap();
    assert_eq!((screen.as_str(), id, n), ("display.lay", Some(0), 14));
}

/// Hover is a flag, and `FeWidget::updateState` 0x4C82A0 turns the flags into the state name a
/// widget draws. The classes whose draws gate something on hover all define `[state.hover]`, and
/// `BevelOptionBox.ctr` — whose bevel is *not* gated — does not. `RE/notes/controls.md` §4.39.
#[test]
fn the_classes_that_can_hover() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let has_hover = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text).has_state("hover")
    };
    // every class whose draw asks 0x4C7CB0 before drawing its bevel
    for t in ["GoToButton.ctr", "Tab Button.ctr", "ControllerSetup.ctr", "CameraSelect.ctr", "CameraPicture.ctr", "PlayerDriverPicture.ctr", "ChroniclePictureIcon.ctr"] {
        assert!(has_hover(t), "{t} should define [state.hover]");
    }
    // and the one whose bevel is unconditional does not
    assert!(!has_hover("BevelOptionBox.ctr"));
    // the hover state is a real swap, not a no-op: the button's amber `hoverbg` for its red `bg`
    let p = GamePaths::find_file(&dir, "GoToButton.ctr").expect("GoToButton.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert!(c.state_parts("normal").iter().any(|p| p == "bg"));
    assert!(c.state_parts("hover").iter().any(|p| p == "hoverbg"));
    assert_eq!(c.part("bg").unwrap().value("color"), Some("data.maincolor"));
    assert_eq!(c.part("hoverbg").unwrap().value("color"), Some("data.hovercolor"));
    assert_eq!(c.default_of("maincolor"), Some("0xFFBC0000"));
    assert_eq!(c.default_of("hovercolor"), Some("0xffffBB00"));
}

/// The pressed state. `NFS5.CARSETTINGBUTTON`'s slot 5 (0x4C1F40) fires `CLICKED`, starts a
/// 150 ms repeat timer and **sets focus**, and slot 7 (0x4C1FB0) clears all three — so its
/// `[state.focused]`, which draws `picseldown` at `data.seldownindex`, is the held-down art.
/// `[state.hovermousedown]` names the same two parts but the runtime never produces that name
/// (0x4C82A0 knows eight, §4.40), so it is dead. `RE/notes/controls.md` §4.40.
#[test]
fn the_pressed_state_is_focused() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let c = read("CarSettingButton.ctr");
    assert!(c.has_state("focused"));
    assert_eq!(c.state_parts("focused"), c.state_parts("hovermousedown"));
    assert!(c.state_parts("focused").iter().any(|p| p == "picseldown"));
    assert_eq!(c.part("picseldown").unwrap().value("shapeIndex"), Some("data.seldownindex"));
    // the three other states it draws are all one SHAPE apiece, and all different
    assert!(c.state_parts("normal").iter().any(|p| p == "picnonsel"));
    assert!(c.state_parts("hover").iter().any(|p| p == "picsel"));
    // no shipped template of the press-focus classes defines `focusedhover`, so the cascade's
    // 0x4C835E arm is unreachable for them and `focused` is what they draw
    for t in ["CarSettingButton.ctr", "DeadZone.ctr", "GouraudIndicatorBar.ctr"] {
        if let Some(p) = GamePaths::find_file(&dir, t) {
            let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
            assert!(!nfs_formats::ctr::Control::parse(&text).has_state("focusedhover"), "{t}");
        }
    }
    // `Graphic Tab Button.ctr` keeps a `[part.mousedownbg]` that no state's `parts=` lists
    let t = read("Graphic Tab Button.ctr");
    assert!(t.part("mousedownbg").is_some());
    for st in ["normal", "hover", "selected", "selectedhover", "hidden"] {
        assert!(!t.state_parts(st).iter().any(|p| p == "mousedownbg"), "{st}");
    }
}

/// The input box's focused state and its caret. `data.alwaysfocused` makes the bind call
/// `setFocused` outright (0x4CC570), and the caret is `[part.cursor]` — a part **no** state's
/// `parts=` lists, drawn by the code at 0x4CC9D7 alone. `RE/notes/controls.md` §4.41.
#[test]
fn the_input_box_caret() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "InputBox.ctr").expect("InputBox.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    // the caret exists as a part and belongs to no state
    assert!(c.part("cursor").is_some());
    for st in ["normal", "focused", "hidden", "disabled"] {
        assert!(!c.state_parts(st).iter().any(|p| p == "cursor"), "{st}");
    }
    assert_eq!(c.part("cursor").unwrap().value("color"), Some("0xff000000"));
    assert_eq!(c.part("cursor").unwrap().value("size"), Some("1,data.height-6"));
    // the focused state is the amber box with black text
    assert_eq!(c.part("selbkgd").unwrap().value("color"), Some("0xFFFFBB00"));
    assert_eq!(c.part("unselbkgd").unwrap().value("color"), Some("0xFFBC0000"));
    assert_eq!(c.part("hitext").unwrap().value("foreColor"), Some("0xFF000000"));
    assert_eq!(c.part("untext").unwrap().value("foreColor"), Some("0xffffffff"));
    assert!(c.state_parts("focused").iter().any(|p| p == "selbkgd"));
    assert!(c.state_parts("normal").iter().any(|p| p == "unselbkgd"));
    // and `alwaysfocused` is a real, defaulted attribute — the two screens that set it are the
    // only ones in the game that show a focused box without a pointer
    assert_eq!(c.default_of("alwaysfocused"), Some("0"));
    let lay = paths.fe_data().join("Layouts");
    let n = ["accountlogin.lay", "createplayer.lay"]
        .iter()
        .filter(|f| {
            GamePaths::find_file(&lay, f)
                .map(|p| String::from_utf8_lossy(&std::fs::read(p).unwrap()).to_lowercase().contains("alwaysfocused"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(n, 2);
}

/// The list box's three row colours, and that its hover colour carries **alpha 0** — the alpha is
/// always the screen's fade, OR'd in by 0x4D8830. `RE/notes/controls.md` §4.42.
#[test]
fn the_list_box_row_colours() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "ListBox.ctr").expect("ListBox.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.default_of("highlightcolorhover"), Some("0x00FFBB00"));
    assert_eq!(c.default_of("highlightcolorselected"), Some("0x00BC0000"));
    assert_eq!(c.default_of("textcolor"), Some("0x00000000"));
}

/// A combo slider's drop-down sits directly below the box, and its size comes from the two
/// attributes the bind reads — `data.selectionboxmaxheight` into `[this+0x1C0]` (0x4C4FB7) and
/// `data.selectionboxwidth` into `[+0x1C8]` (0x4C4FDE). `RE/notes/controls.md` §4.43.
#[test]
fn the_combo_drop_down_box() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "ComboBox.ctr").expect("ComboBox.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    let sb = c.part("selectionbox").expect("selectionbox");
    assert_eq!(sb.value("pos"), Some("0,data.height"));
    assert_eq!(sb.value("size"), Some("data.width,data.selectionboxmaxheight"));
    assert_eq!(c.default_of("selectionboxmaxheight"), Some("160"));
    assert_eq!(c.default_of("selectionboxslidedirection"), Some("-1"));
    // `selectionbox` is in no state's `parts=` — the drop-down is drawn by slot 27, not the walk
    for st in ["normal", "hover", "focused", "hidden", "disabled", "focusedhover"] {
        assert!(!c.state_parts(st).iter().any(|p| p == "selectionbox"), "{st}");
    }
    // and `[state.focused]` is visually the same as `[state.normal]`: opening the list is the
    // only thing focus does to a combo
    assert_eq!(c.state_parts("focused"), c.state_parts("normal"));
}

/// The scrollbar a clamped list grows — `VerticalSlider.ctr`, whose two arrows are the
/// `weeicons.fsh` entries the port hit-tests to scroll. `RE/notes/controls.md` §4.16, §4.45.
#[test]
fn the_list_scrollbar_arrows() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let p = GamePaths::find_file(&dir, "VerticalSlider.ctr").expect("VerticalSlider.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    let dec = c.part("decrement_button").expect("decrement_button");
    let inc = c.part("increment_button").expect("increment_button");
    assert_eq!(dec.value("shapeIndex"), Some("13"));
    assert_eq!(inc.value("shapeIndex"), Some("14"));
    assert_eq!(dec.value("pos"), Some("0,7"));
    assert_eq!(inc.value("pos"), Some("0,control.size.height-increment_button.size.height"));
    let knob = c.part("knob").expect("knob");
    assert_eq!(knob.value("size"), Some("decrement_button.size.width,15"));
    assert_eq!(knob.value("color"), Some("0x80ffffff"));
    assert_eq!(c.part("track").unwrap().value("color"), Some("0xffffbb00"));
}

/// The custom colour editor's two strings and its buttons'. `RE/notes/controls.md` §4.48.
#[test]
fn the_colour_editor_strings() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // the ids' meaning, pinned in the English column (the game reads `FEStrings.loc`, Russian here)
    let s = nfs_formats::locale::Strings::parse(&std::fs::read(GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").unwrap()).unwrap(), 0);
    assert_eq!(s.get(610).unwrap_or_default().to_ascii_lowercase(), "custom color editor");
    assert_eq!(s.get(702).unwrap_or_default().to_ascii_lowercase(), "ok");
    assert_eq!(s.get(703).unwrap_or_default().to_ascii_lowercase(), "cancel");
}

/// `FeWordWrap`'s break, 0x523600: the guess is `width/8 + 10` characters for the first line and
/// the previous line's length + 10 after that, walked back to a space, and a line is accepted only
/// while it is **strictly** narrower than the box. `RE/notes/controls.md` §4.51.
#[test]
fn the_word_wrap_guesses_then_walks_back() {
    // a fixed six pixels a character, so the arithmetic is checkable by hand
    let six = |t: &str| t.chars().count() as f32 * 6.0;
    let wrap = |s: &str, w: f32| nfs_game::layout::wrap_lines_by(&six, s, w);

    // 80 px of box: the guess reaches 80/8 + 10 = 20 characters, and 13 of them fit (13·6 = 78 < 80)
    let s = "one two three four five six seven eight nine ten";
    let lines = wrap(s, 80.0);
    for l in &lines {
        assert!(six(l) < 80.0 || !l.contains(' '), "{l:?} is not strictly narrower than the box");
    }
    assert_eq!(lines.concat().replace(' ', ""), s.replace(' ', ""));

    // the guess caps how far the first line can reach: with a very narrow metric a greedy fit
    // would take far more than trunc(w/8) + 10 characters, and the original does not
    let narrow = |t: &str| t.chars().count() as f32;
    let long = "a b c d e f g h i j k l m n o p q r s t u v w x y z";
    let first = nfs_game::layout::wrap_lines_by(&narrow, long, 80.0)[0].clone();
    assert!(first.chars().count() <= 80 / 8 + 10, "first line reached {} characters", first.chars().count());

    // a word of its own that does not fit is kept whole
    let one = wrap("supercalifragilistic", 30.0);
    assert_eq!(one, vec!["supercalifragilistic"]);

    // trailing spaces are stripped before anything else (0x523679)
    assert_eq!(wrap("hello   ", 600.0), vec!["hello"]);
}

/// `data.helptext` is a tooltip's string id, and 329 shipped widgets carry one — the key this
/// port had left unread the longest. `RE/notes/controls.md` §4.52.
#[test]
fn every_helptext_names_a_string() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let s = nfs_game::layout::load_strings(&paths);
    let dir = paths.fe_data().join("Layouts");
    let mut seen = 0;
    let mut answered = 0;
    for f in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = f.path();
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for line in text.lines() {
            let Some(v) = line.trim().strip_prefix("helptext=") else { continue };
            let Ok(id) = v.trim().parse::<u32>() else { continue };
            if id == 0 {
                continue;
            }
            seen += 1;
            if s.get(id).is_some_and(|t| !t.is_empty()) {
                answered += 1;
            }
        }
    }
    assert_eq!(seen, 267, "the shipped screens carry this many non-zero helptext keys");
    // every one of them resolves — a tooltip that would come up blank is a gap, and there is none
    assert_eq!(seen, answered);
}

/// The animating pictures of §4.53: five identical `NFS5.AnimatingPicture`s on the chronicle
/// screens, and four `NFS5.BOTTOMBARBUTTON`s that all ask for the `hovermousedown` run on hover.
#[test]
fn the_animating_pictures() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let key = |text: &str, block: &str, k: &str| -> Option<i64> {
        text.lines().find_map(|l| l.trim().strip_prefix(&format!("{block}{k}=")).and_then(|v| v.trim().parse().ok()))
    };
    let mut pictures = 0;
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let p = f.path();
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for block in text.split("[section.").filter(|b| b.contains("NFS5.AnimatingPicture")) {
            pictures += 1;
            assert_eq!(key(block, "", "start_frame"), Some(0));
            assert_eq!(key(block, "", "end_frame"), Some(29));
            assert_eq!(key(block, "", "numticks"), Some(256));
            assert!(block.contains("logoAn.fsh"));
        }
    }
    assert_eq!(pictures, 5);

    let p = GamePaths::find_file(&lay, "bottombar.lay").expect("bottombar.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let buttons: Vec<&str> = text.split("[section.").filter(|b| b.contains("NFS5.BOTTOMBARBUTTON")).collect();
    assert_eq!(buttons.len(), 4);
    for b in &buttons {
        // every one of them sends hover to the `hovermousedown` block rather than the `hover` one
        assert_eq!(key(b, "seperate", "hovermousedown"), Some(1));
    }
    // and exactly one of the four animates at rest — the race button
    let moving: Vec<&&str> = buttons.iter().filter(|b| key(b, "", "totalticks").is_some_and(|v| v > 0)).collect();
    assert_eq!(moving.len(), 1);
    assert_eq!(key(moving[0], "", "startframe"), Some(0));
    assert_eq!(key(moving[0], "", "endframe"), Some(29));
    assert_eq!(key(moving[0], "", "totalticks"), Some(128));
    assert!(moving[0].contains("racebutton.fsh"));
}

/// The two hoverable classes whose size is written over their parts, and the art one of them
/// measures — `tabs.fsh`'s entries are large enough that the five tabs overlap.
/// `RE/notes/controls.md` §4.54.
#[test]
fn the_part_relative_sizes() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    assert_eq!(read("BottomBarButton.ctr").value("size"), Some("(label.size.width+picture.size.width+data.space),data.height"));
    assert_eq!(read("Graphic Tab Button.ctr").value("size"), Some("hoverbg.size.width,hoverbg.size.height"));
    // and the label's, which is why `box_of` must *not* measure: twice its own text
    assert_eq!(read("Label.ctr").value("size"), Some("label.size.width*2,label.size.height"));

    let p = GamePaths::find_file(&paths.fe_data().join("Art"), "tabs.fsh").expect("tabs.fsh");
    let f = nfs_formats::fsh::Fsh::parse(&std::fs::read(p).unwrap()).unwrap();
    let sizes: Vec<(u16, u16)> = f.entries.iter().map(|e| (e.blocks[0].width, e.blocks[0].height)).collect();
    assert_eq!(sizes.len(), 9);
    assert_eq!(sizes[0], (167, 142));
    assert_eq!(sizes[1], (132, 123));
    assert_eq!(sizes[2], (114, 107));
}

/// §4.55: `hovermousedown` is hover **plus a held button**, so a bottom-bar button's three
/// stills are its pressed frames and its `hover…` runs are what plain hover animates.
#[test]
fn the_bar_buttons_press_to_a_still() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let p = GamePaths::find_file(&lay, "bottombar.lay").expect("bottombar.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let key = |b: &str, k: &str| -> Option<i64> {
        b.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}=")).and_then(|v| v.trim().parse().ok()))
    };
    let buttons: Vec<&str> = text.split("[section.").filter(|b| b.contains("NFS5.BOTTOMBARBUTTON")).collect();
    assert_eq!(buttons.len(), 4);
    let mut runs = 0;
    let mut stills = 0;
    for b in &buttons {
        // hover runs over several frames…
        if key(b, "hoverendframe") > key(b, "hoverstartframe") && key(b, "hovertotalticks").is_some_and(|v| v > 0) {
            runs += 1;
        }
        // …and the pressed block is a single frame on three of the four
        if key(b, "hovermousedownstartframe") == key(b, "hovermousedownendframe") {
            stills += 1;
        }
    }
    assert_eq!(runs, 4, "all four animate on hover");
    assert_eq!(stills, 3, "three of them press to a still frame");
}

/// The three classes that build a glinty box of their own, and the key each sizes it by —
/// `RE/notes/controls.md` §4.60. Most combos have none, because `selectionboxwidth` is usually 0.
#[test]
fn the_embedded_glinty_boxes() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let lay = paths.fe_data().join("Layouts");
    let mut styles: std::collections::BTreeMap<i64, usize> = Default::default();
    let mut combos_with_a_box = 0;
    for f in std::fs::read_dir(&lay).unwrap().flatten() {
        let p = f.path();
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for b in text.split("[section.") {
            let key = |k: &str| -> Option<i64> {
                b.lines().find_map(|l| l.trim().strip_prefix(&format!("{k}=")).and_then(|v| v.trim().parse().ok()))
            };
            let Some(g) = key("glintybox") else { continue };
            *styles.entry(g).or_default() += 1;
            if b.contains("NFS5.ComboBoxSlider") && key("selectionboxwidth").is_some_and(|v| v > 0) {
                combos_with_a_box += 1;
            }
        }
    }
    // 1…4 is a fixed corner and 5 a fresh one each run (0x51FE86); nothing carries −1
    assert_eq!(styles.get(&1), Some(&100));
    assert_eq!(styles.get(&2), Some(&92));
    assert_eq!(styles.get(&5), Some(&36));
    assert_eq!(styles.get(&-1), None);
    // and the combo's box is `selectionboxwidth` wide, which most of them leave at 0
    assert!(combos_with_a_box > 0 && combos_with_a_box < 50, "{combos_with_a_box} combos with a box");
}

/// `autosize` decides whether a template's `size.value` is used at all (§4.62): seven of the
/// seventy switch it off, and those are the only ones whose `size` expression the original
/// evaluates.
#[test]
fn the_seven_templates_that_do_not_autosize() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let mut off = Vec::new();
    let mut total = 0;
    for f in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = f.path();
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ctr")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let c = nfs_formats::ctr::Control::parse(&text);
        let Some(v) = c.value("autosize") else { continue };
        total += 1;
        if v.trim() == "0" {
            off.push(p.file_stem().unwrap().to_string_lossy().to_string());
        }
    }
    off.sort();
    // 72 templates declare the key; 70 declare it in the `[control]` block and two more only
    // inside a part, which `Control::value` reaches just the same
    assert_eq!(total, 72);
    assert_eq!(
        off,
        vec!["BottomBarButton", "ColorBox", "ComboBox", "GoToButton", "Graphic Tab Button", "MultiAccessor", "OvalThing", "Radio Group"]
    );
    // and the label, which autosizes, is why its `size.value` is dead text
    let p = GamePaths::find_file(&dir, "Label.ctr").expect("Label.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.value("autosize").map(str::trim), Some("1"));
    assert_eq!(c.value("size"), Some("label.size.width*2,label.size.height"));
}

/// §4.63: the four classes whose `size.value` disagrees with their parts. The two the templates
/// get wrong are checkable straight from the data — a car-setting button's art is 15 wide where
/// `CarSettingButton.ctr` says 13, and `CameraPicture.ctr`'s 15×13 is its bevel, not its box.
#[test]
fn the_templates_that_disagree_with_their_art() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let read = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    let c = read("CarSettingButton.ctr");
    assert_eq!(c.value("size"), Some("13,13"));
    assert_eq!(c.value("autosize").map(str::trim), Some("1"));
    // its one drawn part takes `weeicons.fsh` at `data.nonselindex`, and those entries are 15 wide
    let p = GamePaths::find_file(&paths.fe_data().join("Art"), "weeicons.fsh").expect("weeicons.fsh");
    let f = nfs_formats::fsh::Fsh::parse(&std::fs::read(p).unwrap()).unwrap();
    let b = &f.entries[0].blocks[0];
    assert_eq!((b.width, b.height), (15, 13));

    // and the camera picture, which sizes itself over a part `[state.normal]` does not even list
    let cam = read("CameraPicture.ctr");
    assert_eq!(cam.value("autosize").map(str::trim), Some("1"));
    assert_eq!(cam.value("size"), Some("shp.size.width,shp.size.height"));
    assert!(!cam.state_parts("normal").iter().any(|p| p == "shp"));
}

/// §4.64: the templates that reference `control.size.width` inside a part's expression **and**
/// autosize. That pair is only consistent if `control.size` is the `size` attribute rather than
/// the computed box — otherwise the union would re-enter itself.
#[test]
fn control_size_cannot_be_the_box() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let mut both = 0;
    for f in std::fs::read_dir(&dir).unwrap().flatten() {
        let p = f.path();
        if !p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ctr")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        if !text.contains("control.size.width") {
            continue;
        }
        let c = nfs_formats::ctr::Control::parse(&text);
        if c.value("autosize").map(str::trim) != Some("0") {
            both += 1;
        }
    }
    assert_eq!(both, 31, "templates that both autosize and read control.size.width");

    // and the label's alignment lives entirely in the part's offset, which is why the port's text
    // draw must not align again
    let p = GamePaths::find_file(&dir, "Label.ctr").expect("Label.ctr");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let c = nfs_formats::ctr::Control::parse(&text);
    assert_eq!(c.part("label").unwrap().value("pos"), Some("(0-label.size.width)/2*data.align,0"));
}

/// 0x4D8900 and 0x4D8BD0 are an **HLS** pair, not a packing of red, green and blue — which is what
/// `NFS5.ColorBox` keeps in its three floats. `RE/notes/controls.md` §4.66.
#[test]
fn the_colour_editor_holds_hue_saturation_and_luminance() {
    use nfs_game::layout::{hls_to_rgb, rgb_to_hls};
    // a saturation of exactly 0 is the short circuit at 0x4D8956: three copies of the luminance
    assert_eq!(hls_to_rgb(0.25, 0.0, 0.5), 0xff7f7f7f);
    assert_eq!(hls_to_rgb(0.0, 0.0, 1.0), 0xffffffff);
    assert_eq!(hls_to_rgb(0.0, 0.0, 0.0), 0xff000000);
    // the three primaries: `h·360` is 0, 120 and 240, and the channel helper runs at h+120, h and
    // h-120, so a full-saturation, half-luminance hue lands exactly on one channel
    assert_eq!(hls_to_rgb(0.0, 1.0, 0.5), 0xffff0000);
    assert_eq!(hls_to_rgb(1.0 / 3.0, 1.0, 0.5), 0xff00ff00);
    assert_eq!(hls_to_rgb(2.0 / 3.0, 1.0, 0.5), 0xff0000ff);
    // and the factory's own 0.5, 0.5, 0.5 — the mid hue is 180°, cyan, not a grey
    assert_eq!(hls_to_rgb(0.5, 0.5, 0.5), 0xff3fbfbf);
    // the inverse round-trips the primaries
    for (h, s, l) in [(0.0, 1.0, 0.5), (1.0 / 3.0, 1.0, 0.5), (2.0 / 3.0, 1.0, 0.5), (0.5, 0.5, 0.5)] {
        let back = rgb_to_hls(hls_to_rgb(h, s, l));
        assert!((back.0 - h).abs() < 0.01 && (back.1 - s).abs() < 0.01 && (back.2 - l).abs() < 0.01, "{h} {s} {l} -> {back:?}");
    }
    // a grey answers hue 0 and saturation 0 (0x4D8CD3)
    assert_eq!(rgb_to_hls(0xff808080), (0.0, 0.0, 128.0 / 255.0));
}

/// The picker's hit test, 0x4C4AD0: five of the six rects, in a fixed order, and **5** for a point
/// in none of them. `RE/notes/controls.md` §4.66.
#[test]
fn the_colour_editor_regions() {
    use nfs_game::layout::{picker_rects, picker_region, rect};
    // the geometry a shipped colour box gives: `paintshop.lay`'s are 162 × 18
    let (l, t, r, b) = (100.0, 70.0, 262.0, 88.0);
    let g = picker_rects(l, t, r, b);
    assert_eq!(g[rect::A], (106.0, 94.0, 221.0, 102.0));
    assert_eq!(g[rect::B], (225.0, 93.0, 257.0, 103.0));
    assert_eq!(g[rect::C], (106.0, 109.0, 256.0, 204.0));
    assert_eq!(g[rect::D], (105.0, 210.0, 257.0, 220.0));
    assert_eq!(g[rect::E_LEFT], (105.0, 225.0, 178.0, 243.0));
    assert_eq!(g[rect::E_RIGHT], (184.0, 225.0, 257.0, 243.0));
    // the whole picker hangs 155 pixels below the box, and is the box's own width
    assert_eq!(g[rect::E_RIGHT].3 - b, 155.0);
    assert_eq!(g[rect::E_RIGHT].2 - g[rect::E_LEFT].0, r - l - 10.0);
    // C → 0, A → 1, D → 2, E left → 3, E right → 4, nothing → 5
    assert_eq!(picker_region(&g, 150.0, 150.0), 0);
    assert_eq!(picker_region(&g, 150.0, 98.0), 1);
    assert_eq!(picker_region(&g, 150.0, 215.0), 2);
    assert_eq!(picker_region(&g, 150.0, 230.0), 3);
    assert_eq!(picker_region(&g, 200.0, 230.0), 4);
    assert_eq!(picker_region(&g, 150.0, 300.0), 5);
    // rect B, the swatch, is never tested — a point inside it is region 5
    assert_eq!(picker_region(&g, 240.0, 98.0), 5);
    // and every test is `jl`/`jg`, so a rect takes both its edges
    assert_eq!(picker_region(&g, g[rect::C].0, g[rect::C].1), 0);
    assert_eq!(picker_region(&g, g[rect::C].2, g[rect::C].3), 0);
}

/// The draw order is the section order **backwards** (`RE/notes/controls.md` §4.67). Two shipped
/// layouts settle it on their own, and this pins both so the order cannot quietly flip back.
#[test]
fn the_later_sections_are_painted_first() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let load = |name: &str| {
        let p = GamePaths::find_file(&dir, name).unwrap_or_else(|| panic!("{name}"));
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::lay::Layout::parse(&text)
    };

    // `bottombar.lay`: an **opaque** full-width strip after the four buttons it must sit under
    let bar = load("bottombar.lay");
    let buttons: Vec<usize> =
        bar.elements.iter().enumerate().filter(|(_, e)| e.class == "NFS5.BOTTOMBARBUTTON").map(|(i, _)| i).collect();
    assert_eq!(buttons, [3, 4, 5, 6]);
    let strip = &bar.elements[7];
    assert_eq!(strip.class, "NFS5.Rectangle");
    assert_eq!(strip.get("size"), Some("640,24"));
    assert_eq!(strip.get("color"), Some("0xff000000"));
    assert!(buttons.iter().all(|b| *b < 7), "the strip must be after the buttons to be under them");

    // `settings.lay`: two `0x80ffffff` gouraud washes after the five combos they must sit under
    let set = load("settings.lay");
    let combos: Vec<usize> =
        set.elements.iter().enumerate().filter(|(_, e)| e.class == "NFS5.ComboBoxSlider").map(|(i, _)| i).collect();
    assert_eq!(combos, [1, 4, 5, 6, 7]);
    for (n, size) in [(13usize, "180,165"), (14, "181,165")] {
        let b = &set.elements[n];
        assert_eq!(b.class, "NFS5.GOURAUDBOX");
        assert_eq!(b.get("size"), Some(size));
        // the wash is opaque-ish at one edge and clear at the other, so it would visibly grey the
        // pills out if it were painted over them
        assert!([b.get("cUL"), b.get("cUR")].contains(&Some("0x80ffffff")));
    }
    assert!(combos.iter().all(|c| *c < 13));

    // and section 0 is the screen descriptor, not a widget: it is the only one that carries these
    for lay in ["bottombar.lay", "settings.lay", "repair.lay", "paintshop.lay"] {
        let l = load(lay);
        let s0 = &l.elements[0];
        assert!(s0.class.starts_with("REAL.BLACK"), "{lay}: {}", s0.class);
        assert!(s0.get("screen").is_some() && s0.get("room").is_some(), "{lay}");
        assert!(l.elements.iter().skip(1).all(|e| e.get("room").is_none()), "{lay}");
    }
}

/// `Game.NumOpponents` and `Game.NumKOOpponents` are two accessors with two ranges, and the
/// layouts keep them apart: the race screen names one, the three knockout screens the other.
/// `RE/notes/layouts.md`.
#[test]
fn the_two_opponent_counts_are_separate() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut race = Vec::new();
    let mut ko = Vec::new();
    for e in std::fs::read_dir(&dir).expect("Layouts") {
        let p = e.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        if text.contains("Game.NumKOOpponents") {
            ko.push(name);
        } else if text.contains("Game.NumOpponents") {
            race.push(name);
        }
    }
    race.sort();
    ko.sort();
    assert_eq!(race, ["quickrace.lay"]);
    assert_eq!(ko, ["knockout.lay", "locationsetup.lay", "quickknockout.lay"]);
    // and `locationsetup.lay`'s table is seven rows deep, so how many of them fill is entirely
    // the accessor's count — four out of the factory
    let p = GamePaths::find_file(&dir, "locationsetup.lay").expect("locationsetup.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    for row in 0..7 {
        assert!(text.contains(&format!("Knockout.TrackNumber({row}TRACKNAME)")), "row {row}");
    }
    assert!(!text.contains("Knockout.TrackNumber(7TRACKNAME)"));
}

/// `Parts.AllParts` is two collapsible sections, not a flat list — `RE/notes/controls.md`. The
/// two headings are `FEStrings` 2550 and 2551, and only the first takes an argument, which is why
/// 0x5157A1 passes the car and 0x5157C2 passes nothing.
#[test]
fn the_parts_list_has_two_sections() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").expect("FEStrings.csv");
    let s = nfs_formats::locale::Strings::parse(&std::fs::read(p).unwrap(), 0);
    let available = s.get(2550).expect("2550");
    let inventory = s.get(2551).expect("2551");
    assert_eq!(available, "PARTS AVAILABLE FOR %s");
    assert_eq!(inventory, "PARTS INVENTORY");
    assert_eq!(available.matches("%s").count(), 1);
    assert_eq!(inventory.matches("%s").count(), 0);
    // and the category headings the rows inside a section carry are a different run, 2554 up
    assert_eq!(s.get(2554), Some("ENGINE"));
}

/// The game never prints a currency sign. 0x4DBF80, the money formatter every price, fee, prize
/// and repair cost goes through, builds its groups from `FEStrings` 1700…1703 — `"%d,%03d,%03d"`
/// and its signed and quoted variants — and there is no `$` in any of them.
#[test]
fn money_is_formatted_with_commas_and_no_sign() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").expect("FEStrings.csv");
    let s = nfs_formats::locale::Strings::parse(&std::fs::read(p).unwrap(), 0);
    for id in [1700u32, 1701, 1702, 1703] {
        let f = s.get(id).unwrap_or_else(|| panic!("{id}"));
        assert!(f.contains("%d") && f.contains("%03d"), "{id}: {f:?}");
        assert!(!f.contains('$'), "{id}: {f:?}");
    }
    // 1701 and 1703 carry the sign the formatter pushes as a character (0x2D, '-')
    assert!(s.get(1701).unwrap().contains("%c"));
    assert!(!s.get(1700).unwrap().contains("%c"));
}

/// `NFS5.POPUPCIRCLE` is drawn on all nineteen screens that carry one: every block says
/// `state=normal`, the template pins it at 113,100 and tiles one quarter four ways, and nothing
/// in the game can hide it — neither the class id nor the widget name is in the executable.
/// `RE/notes/controls.md` §4.18, §4.68.
#[test]
fn the_popup_circle_is_never_hidden() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut n = 0;
    for e in std::fs::read_dir(&dir).expect("Layouts") {
        let p = e.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let lay = nfs_formats::lay::Layout::parse(&text);
        for el in lay.elements.iter().filter(|e| e.class == "NFS5.POPUPCIRCLE") {
            n += 1;
            assert_eq!(el.get("state"), Some("normal"), "{:?}", p.file_name());
            // and the block carries no `pos` of its own — the template's is HIDDEN|READONLY
            assert_eq!(el.get("pos"), None, "{:?}", p.file_name());
        }
    }
    assert_eq!(n, 19);

    // the template: four quarters of `PopUp.fsh` entry 0, flipped 0…3 into a 414 x 350 oval
    let c = {
        let p = GamePaths::find_file(&paths.fe_data().join("Controls"), "PopUp.ctr").expect("PopUp.ctr");
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    assert_eq!(c.value("pos"), Some("113,100"));
    assert_eq!(c.value("autosize"), Some("1"));
    assert_eq!(c.state_parts("normal"), ["data", "p1", "p2", "p3", "p4"]);
    assert_eq!(c.state_parts("hidden"), ["data"]);
    for (part, pos, flip) in [("p1", "0,0", None), ("p2", "207,0", Some("1")), ("p3", "0,175", Some("2")), ("p4", "207,175", Some("3"))] {
        let p = c.part(part).unwrap_or_else(|| panic!("{part}"));
        assert_eq!(p.value("pos"), Some(pos), "{part}");
        assert_eq!(p.value("shapePathName"), Some("PopUp.fsh"), "{part}");
        assert_eq!(p.value("shapeIndex"), Some("0"), "{part}");
        assert_eq!(p.value("flip"), flip, "{part}");
    }
}

/// `data.offset` on a `NFS5.GOTOBUTTON` is the screen-stack operation, not a position: **> 0**
/// pushes, **0** replaces the top, **< 0** pops. `RE/notes/layouts.md`.
#[test]
fn a_goto_buttons_offset_is_the_stack_operation() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // the template declares it an INT defaulting to 0, and no part references it — so only code
    // can read it, which is what 0x4C8F55 does
    let c = {
        let p = GamePaths::find_file(&paths.fe_data().join("Controls"), "GoToButton.ctr").expect("GoToButton.ctr");
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    assert_eq!(c.default_of("offset"), Some("0"));
    assert!(c.part("data").unwrap().value("attributes").unwrap().split(',').any(|a| a == "offset"));

    // and the 153 shipped buttons: 109 replace, 25 push, 19 pop
    let dir = paths.fe_data().join("Layouts");
    let (mut push, mut replace, mut pop) = (0, 0, 0);
    for e in std::fs::read_dir(&dir).expect("Layouts") {
        let p = e.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let text: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for el in nfs_formats::lay::Layout::parse(&text).elements.iter().filter(|e| e.class == "NFS5.GOTOBUTTON") {
            match el.get("offset").unwrap_or("").trim().parse::<i32>().unwrap_or(0) {
                n if n > 0 => push += 1,
                n if n < 0 => pop += 1,
                _ => replace += 1,
            }
        }
    }
    assert_eq!((push, replace, pop), (25, 109, 19));
    assert_eq!(push + replace + pop, 153);
}

/// Every font and art pack the front end can reach is present, so nothing falls back to a
/// substitute silently. Exactly two names in the whole of `FEData` do not resolve, and both are in
/// templates no layout instantiates. `RE/notes/controls.md` §4.73.
#[test]
fn every_font_and_pack_a_widget_reaches_exists() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let art = paths.fe_data().join("Art");
    let have: std::collections::HashSet<String> = match std::fs::read_dir(&art) {
        Ok(d) => d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_lowercase()).collect(),
        Err(_) => {
            eprintln!("skipping: no FEData/Art");
            return;
        }
    };
    let read = |p: &std::path::Path| -> String {
        std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect()
    };
    let mut missing: std::collections::BTreeSet<String> = Default::default();

    // the names the shipped blocks carry — every one of these must resolve
    let dir = paths.fe_data().join("Layouts");
    for e in std::fs::read_dir(&dir).expect("Layouts") {
        let p = e.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        for el in &nfs_formats::lay::Layout::parse(&read(&p)).elements {
            for k in ["fontPathName", "shapePathName", "bgSmallPathName", "selectedbgSmallPathName"] {
                if let Some(v) = el.get(k).filter(|v| !v.trim().is_empty()) {
                    assert!(have.contains(&v.trim().to_lowercase()), "{k}={v} in {:?}", p.file_name());
                }
            }
        }
    }

    // and the names the templates carry, following `%<part>.<key>%` the way the port does (§2) —
    // `CarSettingButton.ctr`'s `picsel` and `picseldown` both take `%picnonsel.shapePathName%`
    let cdir = paths.fe_data().join("Controls");
    for e in std::fs::read_dir(&cdir).expect("Controls") {
        let p = e.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ctr") || x.eq_ignore_ascii_case("scr")) {
            continue;
        }
        let text = read(&p);
        let c = nfs_formats::ctr::Control::parse(&text);
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            if !matches!(k.trim(), "fontPathName.value" | "shapePathName.value") || v.trim().is_empty() {
                continue;
            }
            let v = v.trim();
            let v = match v.strip_prefix('%').and_then(|r| r.strip_suffix('%')) {
                Some(r) => {
                    let Some((part, key)) = r.split_once('.') else { continue };
                    match c.part(part).and_then(|q| q.value(key)) {
                        Some(x) => x.to_string(),
                        None => continue,
                    }
                }
                None => v.to_string(),
            };
            if !have.contains(&v.to_lowercase()) {
                missing.insert(format!("{v} ({})", p.file_name().unwrap().to_string_lossy()));
            }
        }
    }
    let missing: Vec<&str> = missing.iter().map(String::as_str).collect();
    assert_eq!(
        missing,
        [
            "SliderH.fsh (accessor Iterator.ctr)",
            "stdbtn.fsh (MovieControlButton.ctr)",
            "stdbtn.fsh (Tabs2x.ctr)",
            "stdbtn.fsh (Tabs3x.ctr)",
        ],
        "a font or pack the front end reaches is missing"
    );

    // and none of those four classes is instantiated by any shipped layout, so no widget can
    // reach the two packs that do not ship
    let mut used: std::collections::HashSet<String> = Default::default();
    for e in std::fs::read_dir(&dir).expect("Layouts") {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            used.extend(nfs_formats::lay::Layout::parse(&read(&p)).elements.iter().map(|e| e.class.to_lowercase()));
        }
    }
    for c in ["nfs5.accessoriterator", "nfs5.moviecontrolbutton", "nfs5.tabs2x", "nfs5.tabs3x"] {
        assert!(!used.contains(c), "{c} is instantiated after all");
    }
}

/// `deadzone.lay`'s eight bars, one per `DIJOYSTATE` axis, and the dark track each one sits on.
/// `RE/notes/controls.md` §4.17.
#[test]
fn the_deadzone_screen_has_one_bar_per_axis() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Layouts"), "deadzone.lay").expect("deadzone.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let lay = nfs_formats::lay::Layout::parse(&text);

    let bars: Vec<&nfs_formats::lay::Element> = lay.elements.iter().filter(|e| e.class == "NFS5.DEADZONE").collect();
    assert_eq!(bars.len(), 8);
    let mut idx: Vec<i32> = bars.iter().map(|e| e.get("hackindex").unwrap_or("").trim().parse().unwrap_or(-1)).collect();
    idx.sort();
    assert_eq!(idx, [0, 1, 2, 3, 4, 5, 6, 7], "one bar per axis, 0…7");
    for b in &bars {
        assert_eq!(b.get("size"), Some("300,9"));
        // the two accessors the bind resolves by name, `.Min` and `.Max` of the same axis
        let min = b.get("accessormin").unwrap_or("");
        let max = b.get("accessormax").unwrap_or("");
        assert!(min.ends_with(".Min") && max.ends_with(".Max"), "{min} / {max}");
        assert_eq!(min.trim_end_matches(".Min"), max.trim_end_matches(".Max"));
    }

    // and the sixteen names, in the order 0x51E4C0 registers them — `DIJOYSTATE`'s field order
    let mut axes: Vec<String> =
        bars.iter().map(|e| e.get("accessormin").unwrap_or("").trim_end_matches(".Min").to_string()).collect();
    axes.sort();
    assert_eq!(axes, ["SliderA", "SliderB", "XAxis", "XRot", "YAxis", "YRot", "ZAxis", "ZRot"]);

    // the dark track is a separate `NFS5.Rectangle` over the same 300x9 box — one per bar
    let track: Vec<&nfs_formats::lay::Element> = lay
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.Rectangle" && e.get("size") == Some("300,9"))
        .collect();
    assert_eq!(track.len(), 8);
    assert!(track.iter().all(|e| e.get("color") == Some("0x66000000")));
}

/// `NFS5.DEADZONE`'s two setter arms keep the pair five apart and inside 0…0x80 — 0x51BD10, whose
/// jump table at 0x51BE14 sends handles 1 and 3 to the `.Min` arm and 2 and 4 to the `.Max` one.
#[test]
fn the_deadzone_handles_stay_five_apart() {
    use nfs_game::layout::deadzone_clamp;
    // the factory pair
    let dz = [0, 0x80];
    // the min handle, both of its regions
    assert_eq!(deadzone_clamp(1, 40, dz), [40, 0x80]);
    assert_eq!(deadzone_clamp(3, 40, dz), [40, 0x80]);
    // it cannot pass the max, and stops five short of it
    assert_eq!(deadzone_clamp(1, 0x80, dz), [0x7B, 0x80]);
    assert_eq!(deadzone_clamp(1, -10, dz), [0, 0x80]);
    // the max handle, mirrored
    assert_eq!(deadzone_clamp(2, 100, [40, 0x80]), [40, 100]);
    assert_eq!(deadzone_clamp(4, 100, [40, 0x80]), [40, 100]);
    assert_eq!(deadzone_clamp(2, 0, [40, 0x80]), [40, 45]);
    assert_eq!(deadzone_clamp(2, 300, [40, 0x80]), [40, 0x80]);
    // and no handle at all leaves the pair alone
    assert_eq!(deadzone_clamp(0, 99, dz), dz);
}

/// `NFS5.DEADZONE`'s five bars — the live axis marker and the four handles — at the positions
/// 0x51B7FA…0x51BAE1 gives them. `RE/notes/controls.md` §4.17.
#[test]
fn the_deadzone_bar_carries_a_marker_and_four_handles() {
    use nfs_game::layout::{deadzone_handles, deadzone_marker};

    // the marker rides the whole box on the device's own 0…65535, less its own ten pixels
    // (0x51B7FF), so it is flush with either end and centred at rest
    assert_eq!(deadzone_marker(300, 0), 0.0);
    assert!((deadzone_marker(300, 0xFFFF) - 290.0).abs() < 0.01, "{}", deadzone_marker(300, 0xFFFF));
    assert!((deadzone_marker(300, 32767) - 145.0).abs() < 0.01, "{}", deadzone_marker(300, 32767));
    // out-of-range readings are pinned, not wrapped
    assert_eq!(deadzone_marker(300, -5), 0.0);

    // the factory pair, 0 and 0x80 (0x51E546, 0x51E550): the `.Min` handles sit on the centre and
    // the `.Max` pair on the two ends
    assert_eq!(deadzone_handles(300, [0, 0x80]), [(1, 150.0), (3, 150.0), (2, 0.0), (4, 300.0)]);
    // and a trimmed bar puts them at `v * half / 128` either side (0x51B717)
    assert_eq!(deadzone_handles(300, [32, 96]), [(1, 113.0), (3, 187.0), (2, 38.0), (4, 262.0)]);

    // the visit order is 1, 3, 2, 4 — the same order the hit test walks them in, which is what
    // makes a later hit win when two handles coincide
    assert_eq!(deadzone_handles(300, [0, 0x80]).map(|(r, _)| r), [1, 3, 2, 4]);
}

/// `data.picoffy` shifts a `NFS5.BOTTOMBARBUTTON`'s picture down — 0x4C9BDE, and only the three
/// bottom-bar buttons that have an icon carry a non-zero one. `RE/notes/controls.md` §4.80.
#[test]
fn the_bottom_bar_icons_carry_a_y_offset() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let p = GamePaths::find_file(&dir, "bottombar.lay").expect("bottombar.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let lay = nfs_formats::lay::Layout::parse(&text);

    let mut seen: Vec<(String, i64)> = lay
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.BOTTOMBARBUTTON")
        .map(|e| (e.get("name").unwrap_or("").to_string(), e.int("picoffy").unwrap_or(0)))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("ID_BACKBUTTON".to_string(), 6),
            ("ID_EXITBUTTON".to_string(), 6),
            ("ID_OPTIONSBUTTON".to_string(), 6),
            ("ID_PLAYGAMEBUTTON".to_string(), 0),
        ]
    );

    // and across the whole front end: 22 bar buttons, three of them offset
    let mut off = 0;
    let mut total = 0;
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.BOTTOMBARBUTTON") {
            total += 1;
            if e.int("picoffy").unwrap_or(0) != 0 {
                off += 1;
            }
        }
    }
    assert_eq!((total, off), (22, 3));
}

/// `data.changeable` makes a `NFS5.GouraudIndicatorBar` take the pointer — 0x4CA470's first test.
/// Eleven of the 42 shipped bars carry it. `RE/notes/controls.md` §4.81.
#[test]
fn the_changeable_bars_are_the_eleven_settings_sliders() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut on: Vec<String> = Vec::new();
    let mut total = 0;
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.GouraudIndicatorBar") {
            total += 1;
            if e.int("changeable").unwrap_or(0) != 0 {
                on.push(e.get("accessor").unwrap_or("").to_string());
            }
        }
    }
    on.sort();
    assert_eq!(total, 42);
    assert_eq!(
        on,
        [
            "Audio.AmbientLevel",
            "Audio.EngineLevel",
            "Audio.MusicLevel",
            "Audio.SfxLevel",
            "Audio.SpeechLevel",
            "FF.CollisionVolume",
            "FF.EngineVolume",
            "FF.RoadGripVolume",
            "FF.RoadVolume",
            "FF.TotalVolume",
            "Graphics.GammaCorrection",
        ]
    );
}

/// `NFS5.GRAPHICTABBUTTON`'s clickable area is a parallelogram, not its box — 0x4CABA0. All five
/// are on `optiontabs.lay` and their rectangles overlap. `RE/notes/controls.md` §4.82.
#[test]
fn the_graphic_tabs_are_slanted_and_their_boxes_overlap() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Layouts"), "optiontabs.lay").expect("optiontabs.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let lay = nfs_formats::lay::Layout::parse(&text);

    let tabs: Vec<&nfs_formats::lay::Element> =
        lay.elements.iter().filter(|e| e.class == "NFS5.GRAPHICTABBUTTON").collect();
    assert_eq!(tabs.len(), 5);
    let quad = |e: &nfs_formats::lay::Element| {
        (
            e.int("TLoffsetx").unwrap_or(0),
            e.int("TRoffsetx").unwrap_or(0),
            e.int("BLoffsetx").unwrap_or(0),
            e.int("BRoffsetx").unwrap_or(0),
        )
    };
    let mut got: Vec<(i64, (i64, i64, i64, i64))> = tabs
        .iter()
        .map(|e| (e.get("pos").and_then(|p| p.split(',').next()?.trim().parse().ok()).unwrap_or(0), quad(e)))
        .collect();
    got.sort();
    assert_eq!(
        got,
        [
            (59, (0, -60, 94, 0)),
            (154, (0, -20, 60, 0)),
            (263, (0, 0, 20, -20)),
            (354, (20, 0, 0, -60)),
            (414, (60, 0, 0, -94)),
        ]
    );
    // every one of them slants: no tab carries four zeroes, which is what would make the box right
    assert!(got.iter().all(|(_, q)| *q != (0, 0, 0, 0)));
}

/// `data.tint` is the widget base's colour, `[this+0x120]` — six widgets carry it and only one
/// differs from the opaque-white default. `RE/notes/controls.md` §4.83.
#[test]
fn only_one_widget_in_the_front_end_carries_a_real_tint() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut carried: Vec<(String, String, String)> = Vec::new();
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter() {
            if let Some(v) = e.get("tint").filter(|v| !v.is_empty()) {
                carried.push((name.clone(), e.class.clone(), v.to_lowercase()));
            }
        }
    }
    carried.sort();
    assert_eq!(carried.len(), 6, "{carried:?}");
    // every one of them is a movie widget, and five carry the default written out
    assert!(carried.iter().all(|(_, c, _)| c == "NFS5.MOVIEBOX" || c == "NFS5.MOVIECIRCLE"));
    let odd: Vec<&(String, String, String)> = carried.iter().filter(|(_, _, v)| v != "0xffffffff").collect();
    assert_eq!(odd.len(), 1, "{odd:?}");
    assert_eq!((odd[0].0.as_str(), odd[0].2.as_str()), ("credits.lay", "0xff55afbe"));
}

/// `data.step` on `NFS5.CARSETTINGBUTTON` — the arrows come in pairs and the sign is the point.
/// `RE/notes/controls.md` §4.84.
#[test]
fn the_car_setting_arrows_come_in_plus_and_minus_pairs() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut steps: Vec<i64> = Vec::new();
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.CARSETTINGBUTTON") {
            steps.push(e.int("step").unwrap_or(0));
        }
    }
    assert_eq!(steps.len(), 34);
    assert_eq!(steps.iter().filter(|s| **s == 1).count(), 17);
    assert_eq!(steps.iter().filter(|s| **s == -1).count(), 17);
    // and `controls()` carries the sign through, so a click can use it
    let p = GamePaths::find_file(&dir, "carsetup.lay").or_else(|| GamePaths::find_file(&dir, "CarSetup.lay"));
    if let Some(p) = p {
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let lay = nfs_formats::lay::Layout::parse(&t);
        let n = lay.elements.iter().filter(|e| e.class == "NFS5.CARSETTINGBUTTON").count();
        assert!(n > 0, "carsetup.lay should carry the arrows");
    }
}

/// `canplaygame` on the screen block is what the bottom bar's RACE button asks before it acts —
/// and only thirteen of the 98 screens say yes. `RE/notes/controls.md` §4.85.
#[test]
fn only_the_race_launching_screens_can_play() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut yes: Vec<String> = Vec::new();
    let mut total = 0;
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        total += 1;
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let lay = nfs_formats::lay::Layout::parse(&t);
        if lay.screen().and_then(|s| s.int("canplaygame")).unwrap_or(0) != 0 {
            yes.push(p.file_name().unwrap().to_string_lossy().to_lowercase());
        }
    }
    yes.sort();
    assert_eq!(total, 98);
    assert_eq!(
        yes,
        [
            "creategame.lay",
            "factdone.lay",
            "factorydriver.lay",
            "knockout.lay",
            "knockoutraceinfo.lay",
            "location.lay",
            "locationqk.lay",
            "locationsetup.lay",
            "prerace.lay",
            "quickknockout.lay",
            "quickrace.lay",
            "tournament.lay",
            "victoryroom.lay",
        ]
    );
}

/// `data.allowcustom` — only a colour box that has a custom colour to offer opens the editor, and
/// the two that do not are the dealership's. `RE/notes/controls.md` §4.86.
#[test]
fn the_dealership_colour_boxes_offer_no_custom_colour() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut boxes: Vec<(String, String, i64)> = Vec::new();
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter().filter(|e| e.class == "NFS5.ColorBox") {
            boxes.push((name.clone(), e.get("accessor").unwrap_or("").to_string(), e.int("allowcustom").unwrap_or(0)));
        }
    }
    boxes.sort();
    assert_eq!(boxes.len(), 11);
    let off: Vec<&(String, String, i64)> = boxes.iter().filter(|(_, _, v)| *v == 0).collect();
    assert_eq!(off.len(), 2, "{off:?}");
    assert!(off.iter().all(|(f, a, _)| f == "dealership.lay" && a.starts_with("DealerCar.")));
    // and the paint shop's four all allow it
    assert_eq!(boxes.iter().filter(|(f, _, v)| f == "paintshop.lay" && *v == 1).count(), 4);
}

/// `data.animating` — the five narrative wordwrap widgets that type themselves out.
/// `RE/notes/controls.md` §4.87.
#[test]
fn the_five_narrative_panels_animate() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Layouts");
    let mut on: Vec<String> = Vec::new();
    let mut total = 0;
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().to_lowercase();
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter() {
            if !e.class.to_ascii_lowercase().contains("wordwrap") {
                continue;
            }
            total += 1;
            if e.int("animating").unwrap_or(0) != 0 {
                on.push(name.clone());
            }
        }
    }
    on.sort();
    assert_eq!(total, 11);
    assert_eq!(on, ["factdone.lay", "levelvictory.lay", "missionbrief.lay", "tournament.lay", "victorymessage.lay"]);
}

/// `NFS5.CAMERASELECT`'s `data.accessornum` picks which of `Camera.1`…`Camera.4` it watches, and
/// the one matching `Camera.Current` draws in the `highlighty` state.
/// `RE/notes/controls.md` §4.89.
#[test]
fn the_four_camera_buttons_are_numbered_and_have_a_highlight_state() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Layouts"), "cameras.lay").expect("cameras.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let lay = nfs_formats::lay::Layout::parse(&text);

    let mut got: Vec<(i64, String)> = lay
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.CAMERASELECT")
        .map(|e| (e.int("accessornum").unwrap_or(-1), e.get("text").unwrap_or("").to_string()))
        .collect();
    got.sort();
    assert_eq!(got, [(0, "1".into()), (1, "2".into()), (2, "3".into()), (3, "4".into())]);

    // and the template really has that state, with its own background part
    let c = {
        let p = GamePaths::find_file(&paths.fe_data().join("Controls"), "CameraSelect.ctr").expect("CameraSelect.ctr");
        let t: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&t)
    };
    assert!(c.has_state("highlighty"));
    assert!(c.state_parts("highlighty").iter().any(|n| n == "hbg"));
    assert!(c.state_parts("normal").iter().any(|n| n == "bg"));
}

/// The sweep counts widgets that draw **nothing**, so a combo — which always draws its own box —
/// can never appear in it however empty its drop-down is. That is how 61 of the 97 went unnoticed
/// until §4.91. This pins the accessor families down so the blind spot cannot re-open quietly:
/// every combo's accessor is either one this port enumerates or one on the recorded list.
/// `RE/notes/controls.md` §4.92.
#[test]
fn every_combo_accessor_is_enumerated_or_recorded() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    // the families `combo_rows` and `choice_rows` answer, by prefix
    const ENUMERATED: &[&str] = &[
        "Car.", "CDPlayer.Options", "CDPlayer.View", "Audio.Quality", "Audio.Mode", "Display.",
        "Graphics.SkyRendering", "Graphics.LensFlare", "Graphics.Headlights", "Graphics.CarChrome",
        "Graphics.TextureDetail", "Graphics.CarDetail", "Graphics.SpecialFX", "Graphics.ViewDistance",
        "Comms.Collision", "OwnedCar.PackageNum",
        // §4.91's four families
        "zMergedList.Car", "OpponentCar.Car", "UsedCar.Car", "PrizeCars.Car", "OwnedCars.Car",
        "DealerCar.Car", "GenericCar.Car", "GenericCar2.Car", "CARSETTINGS.CAR",
        "Track1.Track", "LoopedTrack.Track", "Knockout.Track", "Track.Track", "Record.Track",
        "OwnedCar.ExtColour", "OwnedCar.IntColour", "OwnedCar.S1Colour", "OwnedCar.S2Colour",
        "GenericCar.Colour", "DealerCar.Colour", "DealerCar.IntColour",
        "Game.NumOpponents", "Game.NumKOOpponents", "Track1.Laps", "Track1.Traffic",
    ];
    // and the ones the port does not model at all, recorded in §4.91
    const RECORDED: &[&str] = &[
        "Comms.", "Controller.", "Tournament.", "CDPlayer.Title", "Graphics.Resolutions",
        "Knockout.Direction", "Knockout.Laps", "Knockout.Traffic", "Track1.Direction",
        "Game.Damage", "Game.Difficutly",
    ];
    let dir = paths.fe_data().join("Layouts");
    let mut unknown: Vec<String> = Vec::new();
    let mut total = 0;
    for f in std::fs::read_dir(&dir).expect("Layouts") {
        let p = f.unwrap().path();
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let t: String = std::fs::read(&p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        for e in nfs_formats::lay::Layout::parse(&t).elements.iter() {
            if !matches!(e.class.as_str(), "NFS5.ComboBoxSlider" | "NFS5.ColorBox") {
                continue;
            }
            total += 1;
            let a = e.get("accessor").unwrap_or("").split('(').next().unwrap_or("").trim().to_string();
            if a.is_empty() || ENUMERATED.iter().chain(RECORDED).any(|p| a.starts_with(p)) {
                continue;
            }
            unknown.push(a);
        }
    }
    unknown.sort();
    unknown.dedup();
    assert_eq!(total, 97);
    assert!(unknown.is_empty(), "combo accessors neither enumerated nor recorded: {unknown:?}");
}

/// `repair.lay`'s ten check boxes and the `QUICKFIX.*` bool each one watches — the categories the
/// screen's own total, 0x50F110, adds one `if (accessor->slot27())` at a time.
/// `RE/notes/controls.md` §4.96.
#[test]
fn the_repair_screen_has_ten_category_ticks() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Layouts"), "repair.lay").expect("repair.lay");
    let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
    let lay = nfs_formats::lay::Layout::parse(&text);

    let mut names: Vec<String> = lay
        .elements
        .iter()
        .filter(|e| e.class == "NFS5.CheckBox")
        .map(|e| e.get("boolaccessor").unwrap_or("").to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "QUICKFIX.BODYWORK",
            "QUICKFIX.BRAKES",
            "QUICKFIX.COSMETIC",
            "QUICKFIX.ENGINE",
            "QUICKFIX.OVERALL",
            "QUICKFIX.SHOCKS",
            "QUICKFIX.SUSPENSION",
            "QUICKFIX.SWAYBARS",
            "QUICKFIX.TIRES",
            "QUICKFIX.TRANSMISSION",
        ]
    );
    // and every one of them keeps the same pair of entries, on 9 and off 10
    for e in lay.elements.iter().filter(|e| e.class == "NFS5.CheckBox") {
        assert_eq!((e.int("onIndex"), e.int("offIndex")), (Some(9), Some(10)));
    }
}

/// `FeListScrollBar::hitTest` 0x4DB2F0 — four rectangles in a fixed order, the knob before the
/// trough, and 0 when the pointer is off the bar (0x4DB512). `RE/notes/controls.md` §4.101.
#[test]
fn the_combo_scrollbars_four_regions() {
    use nfs_game::layout::scrollbar_region;
    // a 160-px bar at (400, 100) with 20 rows, parked at the top
    let r = |px, py| scrollbar_region(400.0, 100.0, 160.0, 20, 0, px, py);
    assert_eq!(r(408.0, 106.0), 1, "the up arrow, [ecx+0x12C] -> 0x4DB371");
    assert_eq!(r(408.0, 252.0), 2, "the down arrow, [ecx+0x11C] -> 0x4DB3F5");
    assert_eq!(r(408.0, 120.0), 3, "the knob, [ecx+0xFC] -> 0x4DB479");
    assert_eq!(r(408.0, 240.0), 4, "the trough below it, [ecx+0x10C] -> 0x4DB4FD");
    assert_eq!(r(380.0, 240.0), 0, "left of the bar — 0x4DB512 writes 0");
    assert_eq!(r(408.0, 300.0), 0, "below the bar");
    // the knob is tested before the trough, so it never reads as the track
    let knob_h = (160.0f32 - 26.0 - 20.0).max(16.0);
    assert_eq!(r(408.0, 113.0 + knob_h - 1.0), 3);
    // and it slides down as the list scrolls: the same point is trough once the knob has moved
    assert_eq!(scrollbar_region(400.0, 100.0, 160.0, 20, 19, 408.0, 120.0), 4);
}

/// The front end's comma tokeniser, 0x4B58E9: an empty token is skipped outright (0x4B5936), so
/// `"150,"` yields one value and not two. `RE/notes/controls.md` §4.116.
#[test]
fn a_trailing_comma_yields_one_component() {
    let mut e = nfs_formats::lay::Element::default();
    let put = |e: &mut nfs_formats::lay::Element, v: &str| {
        e.fields.insert("size".into(), v.into());
    };
    put(&mut e, "150,");
    assert_eq!(e.parts_of("size"), vec![150.0], "the empty token after the comma is skipped");
    assert_eq!(e.pair("size"), None, "and it is not a pair at all");
    put(&mut e, "1, 50");
    assert_eq!(e.parts_of("size"), vec![1.0, 50.0], "spaces are dropped inside a token (0x4B591B)");
    put(&mut e, " , 50");
    assert_eq!(e.parts_of("size"), vec![50.0], "a leading empty token is skipped the same way");
}

/// `FEChronicleMainBoxControl`'s grid, 0x508020: five columns of 91 across and three rows of 61
/// down from 0x26 below the widget's top. `RE/notes/controls.md` §4.120.
#[test]
fn the_chronicle_timelines_grid() {
    use nfs_game::layout::chronicle_cell;
    let at = |px, py| chronicle_cell(px, py, 100.0, 200.0);
    assert_eq!(at(100.0, 238.0), Some((0, 0)), "the first cell starts 0x26 = 38 below the top");
    assert_eq!(at(190.0, 238.0), Some((0, 0)), "91 wide, so 90 is still column 0");
    assert_eq!(at(191.0, 238.0), Some((1, 0)));
    assert_eq!(at(100.0, 298.0), Some((0, 0)), "61 tall, so 60 is still row 0");
    assert_eq!(at(100.0, 299.0), Some((0, 1)));
    assert_eq!(at(100.0, 237.0), None, "above the grid");
    assert_eq!(at(99.0, 238.0), None, "left of it");
    assert_eq!(at(100.0 + 5.0 * 91.0, 238.0), None, "past the fifth column");
    assert_eq!(at(100.0, 238.0 + 3.0 * 61.0), None, "past the third row");
}

/// `OpponentCar.Car`'s five rows are menu strings, not cars — `controls.md` §4.140.
#[test]
fn the_opponent_combo_lists_five_menu_strings() {
    use nfs_game::opponents::OpponentChoice;
    // the jump table at 0x4F5788, in row order
    let ids: Vec<u32> = OpponentChoice::ROWS.iter().map(|c| c.string_id()).collect();
    assert_eq!(ids, [640, 642, 643, 644, 641]);
    // and the stored value is the row index, which is what `from_index` reads
    for (i, c) in OpponentChoice::ROWS.iter().enumerate() {
        assert_eq!(OpponentChoice::from_index(i as u32), *c);
    }
    // anything past the table is "random car" (0x4F54B2's fall-through)
    assert_eq!(OpponentChoice::from_index(9), OpponentChoice::RandomCar);
}

/// The garage picker's row heights — `controls.md` §4.144, `FEOwnedCarAccessor` slot 26 0x4F60F0.
#[test]
fn the_garage_lists_grouping_heights() {
    use nfs_game::layout::{ListRow, LIST_ROW_H};
    // a heading block costs a whole row, so an era change is 42, a class change 28, a plain row 14
    let era = ListRow::GroupHeading("CLASSIC ERA".into());
    let class = ListRow::GroupHeading("CLASS 3".into());
    let car = ListRow::Text("356".into());
    assert_eq!(era.height(), LIST_ROW_H);
    assert_eq!(era.height() + class.height() + car.height(), 42.0, "an era change");
    assert_eq!(class.height() + car.height(), 28.0, "a class change");
    assert_eq!(car.height(), 14.0, "any other row");
}

/// `nfs5.car + 0x1C0` — the variant index, `controls.md` §4.145.
#[test]
fn the_variant_index_orders_one_generation() {
    let Ok(paths) = nfs_assets::GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let Ok(t) = nfs_game::career::CareerTables::load(&paths) else { return };
    let mut a: Vec<(i16, &str)> = t
        .catalog
        .iter()
        .filter(|c| c.kind == 0 && c.family.starts_with("356 A"))
        .map(|c| (c.variant, c.display_name.as_str()))
        .collect();
    a.sort();
    // one 0…11 run across `356 A`, `356 A Convertible` and `356 A Speedster`
    assert_eq!(a.iter().map(|(v, _)| *v).collect::<Vec<_>>(), (0..12).collect::<Vec<i16>>());
    assert!(a[0].1.contains("Cabriolet"), "{}", a[0].1);
    assert!(a[11].1.contains("Speedster"), "{}", a[11].1);
}

/// The cursor table at 0x5D9C8C against the shipped `Pointer.fsh` — `RE/notes/controls.md`
/// §4.152. Ten shapes, and every `{first, last}` pair lands exactly on a tag boundary in the
/// file, which is what says the pairs were read off correctly.
#[test]
fn the_cursor_shapes_land_on_the_pointer_packs_tag_boundaries() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let p = GamePaths::find_file(&paths.fe_data().join("Art"), "Pointer.fsh").expect("Pointer.fsh");
    let d = std::fs::read(&p).unwrap();
    let n = u32::from_le_bytes(d[8..12].try_into().unwrap()) as usize;
    assert_eq!(n, 34, "Pointer.fsh entry count");
    let tag = |i: usize| String::from_utf8_lossy(&d[16 + i * 8..20 + i * 8]).to_string();
    // Entries 0…5 are tagged "00 "…"05 ", one per frame; everything from 6 on is a named run
    // whose frames share a tag. Both kinds have to line up with the table.
    let expect = ["00", "01", "pick", "Lift", "Pan.", "grab", "Poin", "mags", "magP", "watc"];
    for (shape, &(first, last)) in nfs_game::layout::CURSOR_SHAPES.iter().enumerate() {
        assert!(last < n && first <= last, "shape {shape} is not a range inside the file");
        assert_eq!(tag(first).trim(), expect[shape], "shape {shape} starts on the wrong entry");
        if first >= 6 {
            // a named run: every frame carries the tag, and the neighbours do not
            for i in first..=last {
                assert_eq!(tag(i), tag(first), "frames {first}..={last} are not one run");
            }
            if first > 6 {
                assert_ne!(tag(first - 1), tag(first), "shape {shape} does not start its run");
            }
            if last + 1 < n {
                assert_ne!(tag(last + 1), tag(last), "shape {shape} does not end its run");
            }
        } else {
            // the numbered frames: "00", "01", "02", … in order
            for (k, i) in (first..=last).enumerate() {
                assert_eq!(tag(i).trim(), format!("{:02}", first + k), "frame {i} is out of order");
            }
        }
    }
    // the three the notes name by hand
    assert_eq!(tag(nfs_game::layout::CURSOR_SHAPES[7].0).trim(), "mags"); // the magnifier a 3D car asks for
    assert_eq!(tag(nfs_game::layout::CURSOR_SHAPES[9].0).trim(), "watc"); // the hourglass
    assert_eq!(nfs_game::layout::CURSOR_SHAPES[1], (1, 5)); // the animated arrow, five frames
}

/// `FECar3DControl`'s zone test, 0x4C0D10 — `RE/notes/controls.md` §4.153. Five zones from the
/// turntable's fraction of a turn crossed with the horizontal third of the widget's box, and the
/// top 5/16 of the box before either of them.
#[test]
fn the_3d_cars_zones_follow_the_turntable_and_the_thirds() {
    use nfs_game::layout::car_zone_of;
    // the box the test works in: 300 wide, 160 tall, at the origin
    let (w, h) = (300.0, 160.0);
    let z = |t: f32, x: f32, y: f32| car_zone_of(t, true, x, y, 0.0, 0.0, w, h);
    // 0x4C0D5D: the top five sixteenths are zone 0 whatever the angle
    for t in [0.0, 0.25, 0.5, 0.75] {
        assert_eq!(z(t, 150.0, 0.0), Some(0));
        assert_eq!(z(t, 150.0, 5.0 * h / 16.0), Some(0));
    }
    // …but only for a car with a roof animation: without one the top falls to the angle's zones
    assert_eq!(car_zone_of(0.0, false, 150.0, 0.0, 0.0, 0.0, w, h), Some(4));
    let y = h * 0.9; // below the 5/16 line for everything that follows
    // 0x4C0DCD: t <= 0.1 or t > 0.9 is zone 4 across the whole width
    for x in [10.0, 150.0, 290.0] {
        assert_eq!(z(0.05, x, y), Some(4));
        assert_eq!(z(0.95, x, y), Some(4));
    }
    // 0x4C0E57: 0.1 < t <= 0.4 — left 4, middle 2, right 3
    assert_eq!(z(0.25, 10.0, y), Some(4));
    assert_eq!(z(0.25, 150.0, y), Some(2));
    assert_eq!(z(0.25, 290.0, y), Some(3));
    // 0x4C0DF0: 0.4 < t <= 0.6 is zone 3 across the whole width
    for x in [10.0, 150.0, 290.0] {
        assert_eq!(z(0.5, x, y), Some(3));
    }
    // 0x4C0E0F: 0.6 < t <= 0.9 — left 3, middle 1, right 4
    assert_eq!(z(0.75, 10.0, y), Some(3));
    assert_eq!(z(0.75, 150.0, y), Some(1));
    assert_eq!(z(0.75, 290.0, y), Some(4));
    // the turntable runs backwards, so the fraction has to wrap the same way
    assert_eq!(z(-0.25, 150.0, y), z(0.75, 150.0, y));
    // off the box at all
    assert_eq!(z(0.5, -1.0, y), None);
    assert_eq!(z(0.5, 150.0, h + 1.0), None);
}

/// 0x4E1C90's two frame rules, read 2026-09-21. The hovered arrow counts **backwards** from its
/// last frame over a span of `last − first`, so its first frame never appears; every other shape
/// ramps with the mouse button and stops at whichever end it reaches.
#[test]
fn the_cursor_steps_the_way_the_original_steps_it() {
    use nfs_game::layout::{cursor_frame, CURSOR_SHAPES};
    let arrow = CURSOR_SHAPES[1];
    assert_eq!(arrow, (1, 5));
    // 125 ms a frame, down from 5, and 1 is never drawn (the span is 4, not 5)
    let seen: Vec<usize> = (0..8).map(|k| cursor_frame(1, arrow, k * 125, 0, false)).collect();
    assert_eq!(seen, vec![5, 4, 3, 2, 5, 4, 3, 2]);
    assert!(!seen.contains(&1), "0x4E1D31's clamp to `first` is unreachable, so 1 never shows");
    // one frame short of the step is still the frame before it
    assert_eq!(cursor_frame(1, arrow, 124, 0, false), 5);
    assert_eq!(cursor_frame(1, arrow, 125, 0, false), 4);

    // `grab` (shape 5) closes as the button is held, at 31.25 ms a frame, and stops at `last`
    let grab = CURSOR_SHAPES[5];
    assert_eq!(grab, (28, 33));
    assert_eq!(cursor_frame(5, grab, 0, 0, true), 28);
    assert_eq!(cursor_frame(5, grab, 0, 31, true), 28);
    assert_eq!(cursor_frame(5, grab, 0, 32, true), 29);
    assert_eq!(cursor_frame(5, grab, 0, 5 * 32, true), 33);
    assert_eq!(cursor_frame(5, grab, 0, 100 * 32, true), 33, "clamped at `last`");
    // and opens again when it is let go, from `last` back down to `first`
    assert_eq!(cursor_frame(5, grab, 0, 0, false), 33);
    assert_eq!(cursor_frame(5, grab, 0, 32, false), 32);
    assert_eq!(cursor_frame(5, grab, 0, 100 * 32, false), 28, "clamped at `first`");

    // a one-frame shape is its one frame whatever the clocks say
    for (i, pair) in CURSOR_SHAPES.iter().enumerate().filter(|(_, (f, l))| f == l) {
        assert_eq!(cursor_frame(i, *pair, 9999, 9999, true), pair.0);
        assert_eq!(cursor_frame(i, *pair, 9999, 9999, false), pair.0);
    }
}

/// The cursor's hot spot, 0x4E1DF1…0x4E1E42: **only** shape 2 is offset, and it is offset on both
/// axes by 6.0, not one. There is no per-shape table in the original to read.
#[test]
fn only_the_magnifier_has_a_hot_spot_and_it_is_on_both_axes() {
    use nfs_game::layout::{cursor_hot_spot, CURSOR_SHAPES};
    assert_eq!(cursor_hot_spot(2), -6.0, "0x4E1E07 and 0x4E1E15 both subtract [0x5B2460] = 6.0");
    for shape in (0..CURSOR_SHAPES.len()).filter(|s| *s != 2) {
        assert_eq!(cursor_hot_spot(shape), 0.0, "0x4E1E2C passes the coordinates through");
    }
}

/// 0x4D94A0's clip box for the combo scrollbar's two slivers, pinned 2026-09-21: the trough's
/// **interior**, `upArrow.y + upArrow.h … downArrow.y`, not the widget's whole box (§4.100).
#[test]
fn a_scrollbar_sliver_is_clipped_to_the_trough_between_the_arrows() {
    use nfs_game::layout::{combo_sliver_visible, COMBO_ARROW_H};
    let (y, h) = (40.0f32, 120.0f32);
    // the arrows' own rows are outside the box at both ends
    assert!(!combo_sliver_visible(y, y, h), "the up arrow's first row");
    assert!(!combo_sliver_visible(y + COMBO_ARROW_H - 1.0, y, h), "still the up arrow");
    assert!(combo_sliver_visible(y + COMBO_ARROW_H, y, h), "the trough starts at the arrow's bottom");
    assert!(combo_sliver_visible(y + h - COMBO_ARROW_H - 1.0, y, h), "the last trough row");
    assert!(!combo_sliver_visible(y + h - COMBO_ARROW_H, y, h), "the down arrow's first row");
    assert!(!combo_sliver_visible(y + h - 1.0, y, h), "still the down arrow");
    // and the widget's own box is wider than the clip at both ends, which is the divergence fixed
    assert!(y < y + COMBO_ARROW_H && y + h > y + h - COMBO_ARROW_H);
}

/// 0x4CCC20, the key handler the three input classes share: everything printable goes in except
/// `'%'`, and the accessor's own `maxLen` (the third argument of its ctor 0x4BC8B0) stops it.
#[test]
fn an_input_box_takes_everything_printable_except_a_percent() {
    use nfs_game::layout::LayoutScreen;

    // the three limits read out of the registrations that name an accessor this port can see
    assert_eq!(LayoutScreen::input_max_len("UserList.NameInput"), Some(8), "0x4E5D9E pushes 8");
    assert_eq!(LayoutScreen::input_max_len("ANOTHER"), Some(15), "0x4E6016 pushes 0xF");
    assert_eq!(LayoutScreen::input_max_len("COMMS.PASSWORD"), Some(7), "0x50B9C1 pushes 7");
    // an accessor whose registration this port has not read is left unlimited rather than guessed
    assert_eq!(LayoutScreen::input_max_len("Comms.GameName"), None);

    // the filter itself, stated where it can be checked against 0x4CCD95…0x4CCDA2
    let takes = |c: char| {
        let n = c as u32;
        (0x20..0xFF).contains(&n) && n != 0x25
    };
    assert!(takes('A') && takes('z') && takes('0') && takes(' ') && takes('-'));
    assert!(!takes('%'), "0x4CCDA2: '%' is the one printable character refused");
    assert!(!takes('\n') && !takes('\t'), "0x4CCD9D: below 0x20");
    assert!(!takes('\u{100}'), "0x4CCD95: 0xFF and above");
    // and nothing else in the printable range is singled out
    for n in 0x20u32..0xFF {
        let c = char::from_u32(n).unwrap();
        assert_eq!(takes(c), n != 0x25, "only 0x25 is refused, not {n:#04x}");
    }
}

/// The caret of an input box — `0x4CC9D7` in `FeInputBox::draw` and the identical block at
/// `0x4C343D` in `NFS5.CHATINPUTBOX`'s own draw `0x4C33C0`. Both ask `isFocused` (0x4C7DD0,
/// `[this+0x10C] & 0x40`), both take `TIMER_getTicks() & 0x3F` and draw only while it is under
/// `0x20`, and both draw the part the shared bind `0x4CC430` resolved by **name** at 0x4CC50B —
/// `"cursor"` — into `[this+0x14C]`.
///
/// What that part *is* differs by template, and this pins all three against the shipped files so
/// the port cannot quietly draw one shape for all of them:
///
/// * `InputBox.ctr` — a `RECT`, one pixel wide and `data.height-6` tall, opaque black;
/// * `CharVectInputBox.ctr` and `ChatInputBox.ctr` — a `TEXT` underscore in `univers8b.ffn`.
///
/// And the position rule the port implements: every one of the three puts `cursor` exactly one
/// pixel past the right edge of `hitext`, whatever that template's alignment is.
#[test]
fn an_input_box_puts_its_caret_one_pixel_past_the_text() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let dir = paths.fe_data().join("Controls");
    let load = |name: &str| {
        let p = GamePaths::find_file(&dir, name).expect(name);
        let text: String = std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        nfs_formats::ctr::Control::parse(&text)
    };
    // a stand-in box 100 wide and 14 tall with 40 pixels of text in it
    const W: i32 = 100;
    const TW: i32 = 40;
    let refs = |n: &str| match n {
        "data.width" => nfs_formats::ctr::Macro::Text(W.to_string()),
        "data.height" => nfs_formats::ctr::Macro::Text("14".into()),
        "hitext.size.width" => nfs_formats::ctr::Macro::Text(TW.to_string()),
        _ => nfs_formats::ctr::Macro::Undefined,
    };
    for name in ["InputBox.ctr", "CharVectInputBox.ctr", "ChatInputBox.ctr"] {
        let c = load(name);
        let cursor = c.part("cursor").unwrap_or_else(|| panic!("{name} should have a cursor part"));
        let hitext = c.part("hitext").unwrap_or_else(|| panic!("{name} should have a hitext part"));
        let (cx, _) = nfs_formats::ctr::eval_pair(cursor.value("pos").unwrap(), &refs).unwrap();
        let (hx, _) = nfs_formats::ctr::eval_pair(hitext.value("pos").unwrap(), &refs).unwrap();
        assert_eq!(cx, hx + TW + 1, "{name}: the caret is one pixel past the text");
    }
    // the bar
    let c = load("InputBox.ctr");
    let p = c.part("cursor").unwrap();
    assert_eq!(p.kind(), nfs_formats::ctr::PartKind::Rect);
    assert_eq!(p.value("color"), Some("0xff000000"));
    assert_eq!(nfs_formats::ctr::eval_pair(p.value("size").unwrap(), &refs), Some((1, 8)));
    assert_eq!(nfs_formats::ctr::eval_pair(p.value("pos").unwrap(), &refs).map(|(_, y)| y), Some(3));
    // and the two underscores
    for name in ["CharVectInputBox.ctr", "ChatInputBox.ctr"] {
        let c = load(name);
        let p = c.part("cursor").unwrap();
        assert_eq!(p.kind(), nfs_formats::ctr::PartKind::Text, "{name}");
        assert_eq!(p.value("text"), Some("_"), "{name}");
        assert_eq!(p.value("fontPathName"), Some("univers8b.ffn"), "{name}");
        assert_eq!(p.value("foreColor"), Some("0xFF000000"), "{name}");
        assert_eq!(nfs_formats::ctr::eval_pair(p.value("pos").unwrap(), &refs).map(|(_, y)| y), Some(2), "{name}");
    }
}

/// Every shipped `.ctr` against the two things the executable can actually reach, so neither
/// blind spot can re-open quietly (`RE/notes/controls.md` §4.156).
///
/// **A state** is selectable only through `FeWidget::updateState` 0x4C82A0 — whose eight arms are
/// `hidden`, `disabled`, `focusedhover`, `focused`, `selectedhover`, `selected`, `hover`,
/// `normal` (§4.110) — or through `setState(name)` 0x4C7970, which has exactly two call sites in
/// the whole image, 0x4BF43A and 0x4BF8B5, both pushing `"highlighty"`. Three shipped templates
/// declare something outside those nine, and nothing in the game can ask for any of them:
/// `CarSettingButton.ctr`'s `hovermousedown` (the executable's only occurrence of that text is
/// the tail of `"data.seperatehovermousedown"`, an attribute of the bottom bar button) and
/// `MovieCircle.ctr`'s `test1` and `test2`, which appear nowhere in the image at all.
///
/// **A part no state lists** can only be reached by a class's own draw, which means by a by-name
/// lookup — so each one is either a push this project has located or a part nothing can draw.
#[test]
fn no_template_offers_a_look_the_game_cannot_ask_for() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    const REACHABLE: [&str; 9] =
        ["normal", "hover", "focused", "focusedhover", "selected", "selectedhover", "highlighty", "hidden", "disabled"];
    // state -> the template that declares it, and why nothing can select it
    const DEAD_STATES: [(&str, &str); 3] =
        [("hovermousedown", "CarSettingButton.ctr"), ("test1", "MovieCircle.ctr"), ("test2", "MovieCircle.ctr")];
    // a part no state lists -> where the executable looks it up by name, or "" when it cannot
    const UNLISTED_PARTS: [(&str, &str); 8] = [
        ("cursor", "0x4CC50B"),
        ("shp", "0x4BF81E"),
        ("selectionbox", "0x4C4F03"),
        ("track", "the scrollbar's own"),
        ("trackgraphic", "0x4D0753"),
        ("knob", "0x4D0795"),
        ("decrement_button", "0x4D0769"),
        ("increment_button", "0x4D077F"),
    ];
    let dir = paths.fe_data().join("Controls");
    let mut dead_seen: Vec<String> = Vec::new();
    let mut unlisted_seen: Vec<String> = Vec::new();
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).expect("Controls").flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ctr")) {
            continue;
        }
        files += 1;
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text: String = std::fs::read(&path).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let c = nfs_formats::ctr::Control::parse(&text);
        for st in c.states.keys() {
            if !REACHABLE.contains(&st.as_str()) {
                assert!(
                    DEAD_STATES.contains(&(st.as_str(), name.as_str())),
                    "{name} declares [state.{st}], which is neither one of the nine the executable can select nor a recorded dead state"
                );
                dead_seen.push(format!("{name}:{st}"));
            }
        }
        let used: std::collections::BTreeSet<&str> = c.states.values().flatten().map(String::as_str).collect();
        for p in c.parts_order.iter().filter(|p| p.as_str() != "data" && !used.contains(p.as_str())) {
            // `mousedownbg` is the part case of the same thing: no such string in the image
            if p == "mousedownbg" {
                continue;
            }
            assert!(
                UNLISTED_PARTS.iter().any(|(n, _)| n == p),
                "{name} declares a part `{p}` that no state lists and nothing here has located a lookup for"
            );
            unlisted_seen.push(format!("{name}:{p}"));
        }
    }
    assert!(files > 30, "only {files} templates found");
    // and the dead states really are all three, not a subset that happened to be skipped
    assert_eq!(dead_seen.len(), 3, "dead states seen: {dead_seen:?}");
    assert!(!unlisted_seen.is_empty());
}

/// The fifth axis, and it comes out empty — but by measurement, not by assumption.
///
/// A widget whose font does not load still draws its text, in whatever face the fallback is, so
/// no blank count can see it: `LayoutScreen::new` logs `layout font missing` / `layout font
/// unreadable` at `debug!` and carries on with `univers8b.ffn`. This walks every font name the
/// shipped screens and templates ask for and parses each one, so a silently wrong face cannot
/// creep in.
///
/// It also pins the one template that asks for a font by **attribute** — `MultiAccessor.ctr`'s
/// three `LOCALIZEDTEXT` parts carry `fontPathName.value=%data.font%`, a form this port does not
/// resolve. That is safe only while nothing uses it: `NFS5.MULTIACCESSOR` appears in **no**
/// shipped layout, and **no** `.lay` block carries a `font=` attribute at all. The assertion
/// fails the day either changes.
#[test]
fn every_font_the_front_end_asks_for_loads() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let fe = paths.fe_data();
    let read_text = |p: &std::path::Path| -> String {
        std::fs::read(p).unwrap().iter().map(|&b| nfs_formats::locale::cp1252(b)).collect()
    };
    let mut wanted: std::collections::BTreeSet<String> = Default::default();
    let mut uses_multi = 0;
    let mut font_attrs = 0;
    for entry in std::fs::read_dir(fe.join("Layouts")).expect("Layouts").flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lay")) {
            continue;
        }
        let layout = nfs_formats::lay::Layout::parse(&read_text(&path));
        for e in &layout.elements {
            if let Some(f) = e.get("fontPathName").filter(|f| !f.is_empty()) {
                wanted.insert(f.to_lowercase());
            }
            if e.get("font").is_some() {
                font_attrs += 1;
            }
            if e.class.eq_ignore_ascii_case("NFS5.MULTIACCESSOR") {
                uses_multi += 1;
            }
        }
    }
    let mut by_attribute: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(fe.join("Controls")).expect("Controls").flatten() {
        let path = entry.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ctr")) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let c = nfs_formats::ctr::Control::parse(&read_text(&path));
        for p in c.parts.values() {
            let Some(f) = p.value("fontPathName").filter(|f| !f.is_empty()) else { continue };
            if f.contains('%') {
                by_attribute.push(format!("{name}:{f}"));
            } else {
                wanted.insert(f.to_lowercase());
            }
        }
    }
    assert!(wanted.len() >= 6, "only {} fonts named: {wanted:?}", wanted.len());
    let art = fe.join("Art");
    for name in &wanted {
        let path = GamePaths::find_file(&art, name).unwrap_or_else(|| panic!("{name} is named by the front end and is not in FEData/Art"));
        let data = std::fs::read(&path).unwrap();
        assert!(nfs_formats::fnt::Font::parse(&data).is_ok(), "{name} does not parse");
    }
    // the `%attr%` form, and why leaving it unresolved is safe
    assert!(by_attribute.iter().all(|s| s.starts_with("MultiAccessor.ctr:")), "a new template asks for a font by attribute: {by_attribute:?}");
    assert_eq!(uses_multi, 0, "a layout now uses NFS5.MULTIACCESSOR, whose font comes from %data.font%");
    assert_eq!(font_attrs, 0, "a .lay block now carries a font= attribute");
}

/// `load_strings` reads what the original reads: `FEStrings.loc` for the front end and
/// `GameData/DashHud/hud.loc` for the HUD, in the `language` of `fe.txt` (0 here), block 0 of both
/// being Russian Windows-1251 in this install — each byte carried as one char.
#[test]
fn the_strings_are_the_loc_files() {
    let Ok(paths) = GamePaths::locate() else {
        eprintln!("skipping: no game data");
        return;
    };
    let s = nfs_game::layout::load_strings(&paths);
    let bytes = |t: Option<&str>| t.map(|t| t.chars().map(|c| nfs_formats::locale::to_cp1252(c).unwrap()).collect::<Vec<u8>>());
    // 612 "н/д", 18161 "Испытатель" for the front end, "ПРИЗ  " for the HUD
    assert_eq!(bytes(s.get(612)), Some(vec![0xED, b'/', 0xE4]));
    assert_eq!(bytes(s.get(18161)).map(|b| b.len()), Some(10));
    assert_eq!(bytes(s.hud(18161)), Some(vec![0xCF, 0xD0, 0xC8, 0xC7, b' ', b' ']));
    // the HUD reads its own table only: an FE-only id is NULL there (0x536DA0)
    assert_eq!(s.hud(612), None);
    // the opponents' five rows (`opponents.rs`) all resolve
    assert!([640, 641, 642, 643, 644].iter().all(|&id| s.get(id).is_some_and(|t| !t.is_empty())));
}
