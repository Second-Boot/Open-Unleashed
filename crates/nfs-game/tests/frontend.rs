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
    let opaque = |medium: f64| -> Vec<&str> {
        ["label", "mediumlabel", "smalllabel"]
            .into_iter()
            .filter(|n| {
                let v = c.part(n).unwrap().value("foreColor").unwrap();
                let r = |x: &str| if x == "data.medium" { Some(medium) } else { None };
                nfs_formats::ctr::eval(v, &r).is_some_and(|n| (n as u64) >> 24 != 0)
            })
            .collect()
    };
    assert_eq!(opaque(0.0), ["label"]);
    assert_eq!(opaque(1.0), ["mediumlabel"]);
    assert_eq!(opaque(4.0), ["smalllabel"]);

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
    for (align, want) in [(0.0, 0.0), (1.0, -20.0), (2.0, -40.0)] {
        let r = |n: &str| match n {
            "label.size.width" => Some(40.0),
            "data.align" => Some(align),
            _ => None,
        };
        assert_eq!(nfs_formats::ctr::eval_pair(label.value("pos").unwrap(), &r), Some((want, 0.0)));
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
