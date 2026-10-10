use super::*;

/// `FeListScrollBar`'s knob placement (0x4DB220) and its drag (0x4DB9B0) are exact inverses,
/// both dividing by the position **count**, not `count − 1` — verified 2026-09-30 against the
/// live game's own `paintshop` exterior-colour list (`count` 4, `travel` 4 design px there,
/// too small a range for a screenshot to show the fix — `combo_drag_layout.diff`'s own
/// before/after screenshots of that list are visually identical for exactly this reason). A
/// wider list makes the bug concrete: with `count = 4` (as in that live list) the old,
/// wrong `count − 1 = 3` divisor gives a knob position one whole design pixel further down
/// than the original at `first = 3` — small, but the same off-by-a-row error compounds on a
/// list with dozens of rows (a settings/profile list), where `count − 1` drifts the knob and
/// the drag noticeably short of the original's own placement.
#[test]
fn the_combo_knob_and_its_drag_divide_by_count_not_count_minus_one() {
    // 0x4DB220 on a wide list: h=200, count=50, first=25 — knobH=max(16,200-26-50)=124,
    // travel=200-26-124=50, top=y+13+25*50/50=y+13+25
    let (top, knob_h, travel) = combo_knob(100.0, 200.0, 50, 25);
    assert_eq!(knob_h, 124.0);
    assert_eq!(travel, 50);
    assert_eq!(top, 100.0 + 13.0 + 25.0);
    // 0x4DB9B0, its exact inverse: pos = grab_at + (py-grab_y)*count/travel
    assert_eq!(combo_drag_pos(150.0, 100.0, 25, 50, travel), Some(25 + 50 * 50 / 50));

    // the live `paintshop` ExtColour list's own numbers: h=160, count=4 — knobH=max(16,160-
    // 26-4)=130, travel=160-26-130=4, so a whole drop-down's worth of scroll is 4 design px
    let (_, knob_h, travel) = combo_knob(0.0, 160.0, 4, 0);
    assert_eq!((knob_h, travel), (130.0, 4));
    // at first=3 (the last position) the correct divisor (4) gives 3*4/4=3; the bug this
    // fix replaced (dividing by count-1=3) gave 3*4/3=4 — one design px further down than
    // the original, and forced full-value clamps to look identical between the two, exactly
    // what the screenshots on this same list show
    assert_eq!(3 * travel / 4, 3);
    assert_eq!(3 * travel / 3, 4);
}

/// `0x529220(this, 0x96, 1)`'s own rule — a 150 ms period, one step per period elapsed, not
/// per game tick — read at the combo drop-down's own scrollbar arrow (`FeListScrollBar`
/// `0x4DB040`/`0x4DB1D0`, §4.101), the same shape `NFS5.VerticalSlider`'s bar and
/// `NFS5.CARSETTINGBUTTON` already use for their own held arrows.
#[test]
fn the_scrollbar_arrow_repeats_every_150_ms_not_once_per_tick() {
    // under one period: no step yet, and the whole elapsed time is banked
    assert_eq!(repeat_steps(0, 149, 150), (0, 149));
    // exactly one period: one step, nothing left over
    assert_eq!(repeat_steps(0, 150, 150), (1, 0));
    // several periods in one long frame (a stall, or a coarse tick): one step per period
    // completed, not one step per call — the timer would have fired three times
    assert_eq!(repeat_steps(0, 449, 150), (2, 149));
    // the accumulator carries a partial period across frames, exactly like the already-
    // ported `list_repeat`/`setting_repeat` — two 100 ms frames (200 ms banked) step once
    // and carry 50 ms, not zero
    let (steps, acc) = repeat_steps(0, 100, 150);
    assert_eq!((steps, acc), (0, 100));
    assert_eq!(repeat_steps(acc, 100, 150), (1, 50));
}

/// `FEStringAccessor::insert` 0x4BCA90's own forbidden set — the nine bytes Windows refuses
/// in a file name, `\ / : * ? " < > |` — found on `UserList.NameInput`'s profile-name box,
/// the port's own save filename source (`layout.rs::type_char`'s doc).
#[test]
fn the_nine_filename_illegal_bytes_and_nothing_else() {
    for c in ['\\', '/', ':', '*', '?', '"', '<', '>', '|'] {
        assert!(filename_illegal_char(c as u32), "{c:?} is Windows-illegal in a file name");
    }
    // every ordinary letter/digit, the space, and '%' (already refused one layer up by the
    // shared key handler, so this layer does not need to name it too) all pass this filter
    for c in ['a', 'Z', '5', ' ', '%', '\'', '-', '_', '.', ',', '!'] {
        assert!(!filename_illegal_char(c as u32), "{c:?} is not one of the nine");
    }
}

/// 0x4E25A0: a silent movie (no `data.audio`) only clamps a genuine backlog — more than one
/// frame due — to one and drops the rest; the ordinary one-frame case carries its true
/// remainder exactly as the audio path does (`ebp == edi + 1` skips the clamp, 0x4e2634); one
/// with audio always catches up in full.
#[test]
fn a_silent_movie_only_clamps_a_genuine_backlog() {
    let period = 1u64 << 16; // 1 ms, for round numbers
    // under a period: no frame either way
    assert_eq!(movie_advance_frames(0, period, false), (0, 0));
    assert_eq!(movie_advance_frames(0, period, true), (0, 0));
    // one frame due, with a remainder: carried both ways — the common case under a fixed
    // small step, where the clamp must not fire on every tick
    assert_eq!(movie_advance_frames(period + 5000, period, false), (5000, 1));
    assert_eq!(movie_advance_frames(period + 5000, period, true), (5000, 1));
    // a stall worth 5 periods: with audio all five decode and the remainder carries; without
    // it, one frame only, and the other four (and their remainder) are dropped, not queued
    // for the next call
    assert_eq!(movie_advance_frames(period * 5 + 3, period, true), (3, 5));
    assert_eq!(movie_advance_frames(period * 5 + 3, period, false), (0, 1));
}

#[test]
fn the_typing_starts_on_the_first_draw_past_the_wait_and_holds_in_a_transition() {
    let mut t = Typing::new(0, true);
    assert_eq!(t.draw(2, true), None);
    assert_eq!(t.draw(151, false), None, "151 is not more than 0x97");
    // a frame that spans ticks 152…155 lands on 155: the typing counts from there
    assert_eq!(t.draw(155, false), None, "the reset draws nothing either");
    assert_eq!(t, Typing { start: 155, last: 155, wait: 0 });
    assert_eq!(t.draw(164, false), Some(3));
    // a transition stops `last`, so the budget holds
    assert_eq!(t.draw(200, true), Some(3));
    assert_eq!(t.draw(200, false), Some(15));
    t.restart(210);
    assert_eq!(t.draw(212, false), Some(0));
    // one that does not animate has no wait
    assert_eq!(Typing::new(0, false).draw(1, false), Some(0));
}

#[test]
fn a_wordwrap_watch_fades_its_old_text_out_before_typing_the_new_one() {
    let mut w = WrapWatch { text: "old".into(), changing: None, restart: None };
    w.tick("old", 10, true);
    assert_eq!(w.fade(10), 1.0);
    w.tick("new", 20, true);
    assert_eq!((w.text.as_str(), w.changing), ("old", Some(20)));
    // (20 − 52)·4 + 0x100 = 128
    assert!((w.fade(52) - 128.0 / 255.0).abs() < 1e-6);
    w.tick("new", 84, true);
    assert_eq!(w.text, "old", "64 ticks is not yet more than 0x40");
    w.tick("new", 85, true);
    assert_eq!((w.text.as_str(), w.restart, w.changing), ("new", Some(85), None));
    // one that does not animate takes the new text on the same tick
    let mut q = WrapWatch { text: "a".into(), changing: None, restart: None };
    q.tick("b", 5, false);
    assert_eq!((q.text.as_str(), q.changing), ("b", None));
}

/// `FE_transitionBits` 0x4C88C0 — the six names, lower-cased, and 0 for anything else.
#[test]
fn transition_names_map_to_their_bits() {
    assert_eq!(transition_bits("SAME"), trans::SAME);
    assert_eq!(transition_bits("LEFT"), trans::LEFT);
    assert_eq!(transition_bits("RIGHT"), trans::RIGHT);
    assert_eq!(transition_bits("TOP"), trans::TOP);
    assert_eq!(transition_bits("BOTTOM"), trans::BOTTOM);
    assert_eq!(transition_bits("FADE"), trans::FADE);
    assert_eq!(transition_bits("fade"), trans::FADE);
    assert_eq!(transition_bits(""), 0);
}

/// The clock: three dead frames, then 0..512 over half a second (0x4E09E8…0x4E0AB4).
#[test]
fn the_transition_clock_runs_for_half_a_second_after_three_frames() {
    let mut t = Transition::In { frames: TRANS_DELAY_FRAMES, ms: 0 };
    for _ in 0..TRANS_DELAY_FRAMES {
        assert_eq!(t.progress(), TRANS_FULL, "the widget is a whole screen away while it waits");
        t = t.advance(16);
    }
    assert_eq!(t.progress(), TRANS_FULL);
    t = t.advance(250);
    assert_eq!(t.progress(), TRANS_FULL / 2, "half the time, half the way in");
    t = t.advance(250);
    assert_eq!(t, Transition::Idle, "and it settles exactly at the half second");
    assert!(t.done());

    // going out climbs the other way and stops at 512
    let mut t = Transition::Out { frames: 0, ms: 0 };
    assert_eq!(t.progress(), 0);
    t = t.advance(500);
    assert_eq!(t.progress(), TRANS_FULL);
    assert!(t.done());
    assert_eq!(t.advance(500).progress(), TRANS_FULL, "and stays gone");
}

/// What the progress is *worth* — the two formulas `FeWidget` applies to it, read 2026-09-21.
///
/// The slide is **0x4C8760**, four arms off the bit word:
///
/// ```text
/// 0x4C87BF  bl & 2  -> [ecx]   -= [0x5DEAC8] * p / [0x5B6B2C]     ; left
/// 0x4C87D5  bl & 4  -> [ecx]   += [0x5DEAC8] * p / [0x5B6B2C]     ; right
/// 0x4C87EB  bl & 8  -> [ecx+4] -= [0x5DEACC] * p / [0x5B6B2C]     ; top
/// 0x4C8802  bl & 16 -> [ecx+4] += [0x5DEACC] * p / [0x5B6B2C]     ; bottom
/// ```
///
/// `[0x5B6B2C]` is **512** in the image, which is where `TRANS_FULL` comes from, and
/// `[0x5DEAC8]` / `[0x5DEACC]` are the **live screen width and height** — zero in the image
/// and written at **0x556C8A** / 0x556C90 from the selected video mode's record at 0x5DE654 /
/// 0x5DE658, the same pair the platform layer clips against at 0x53550E and 0x536F8A. So a
/// sliding widget travels exactly **one screen**, and this port multiplying by its design size
/// in design space is that same distance. (The port cited 0x4C87CD for `TRANS_FULL` but had
/// never pinned the multiplier.)
///
/// The fade is **0x4C8820**: `p << 8` minus one copy of `p` — the ×255 idiom — over the same
/// 512, an integer divide.
#[test]
fn a_sliding_widget_travels_exactly_one_screen_and_fades_by_p_255_over_512() {
    // the four arms, at half of the way
    let half = TRANS_FULL / 2;
    let f = half as f32 / TRANS_FULL as f32;
    assert_eq!(f, 0.5);
    assert_eq!(DESIGN.0 * f, DESIGN.0 / 2.0, "left/right move by half the screen's width");
    assert_eq!(DESIGN.1 * f, DESIGN.1 / 2.0, "top/bottom by half its height");
    // and the whole way is the whole screen, which is what 512 means
    assert_eq!(DESIGN.0 * (TRANS_FULL as f32 / TRANS_FULL as f32), DESIGN.0);
    // the fade, exactly as 0x4C8834…0x4C883C computes it
    let fade = |p: i32| ((p << 8) - p) / TRANS_FULL;
    assert_eq!(fade(0), 0, "in place, nothing is taken off");
    assert_eq!(fade(TRANS_FULL), 255, "a whole screen away, all of it");
    assert_eq!(fade(half), 127, "and the integer divide truncates at the halfway point");
    // which is the alpha the draw uses
    for p in [0, 1, half, TRANS_FULL] {
        assert_eq!(1.0 - fade(p) as f32 / 255.0, 1.0 - (p * 255 / TRANS_FULL) as f32 / 255.0);
    }
}

/// The draw dispatch of `sprites_reporting_bg` as it was when it matched `e.class` as a string:
/// one label per arm, in the arm order. `WidgetClass` must reproduce it.
fn old_draw_arm(class: &str) -> &'static str {
    match class {
        c if c.starts_with("REAL.BLACK") => "screen_fill",
        "NFS5.GOURAUDBOX" => "gouraud_box",
        "NFS5.GouraudIndicatorBar" => "gouraud_indicator_bar",
        "NFS5.GlintyRectangle" => "glinty_rectangle",
        "NFS5.PANEL" => "panel",
        "NFS5.3DCAR" => "3d_car",
        "NFS5.DEADZONE" => "deadzone",
        "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHATDISPLAYBOX" | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP" => "text_boxes",
        "NFS5.ListBox" => "list_box",
        "NFS5.RADIOGROUP" => "radio_group",
        "NFS5.TrackSmall2dSpline" => "track_small_spline",
        "NFS5.PRIZERACEMAP" | "NFS5.FACTORYMAP" => "race_or_factory_map",
        "NFS5.PLAYERPICTURE" | "NFS5.MINIPLAYERPICTURE" | "NFS5.BADGE" | "NFS5.MINIBADGE" | "NFS5.FACTORYPIC" | "NFS5.HUD" => "pack_pictures",
        "NFS5.MOVIEBOX" | "NFS5.MOVIECIRCLE" => "movie",
        "NFS5.ACCOUNTTHING" => "account_thing",
        "NFS5.ANIMSHOWCASEPICTURE" => "anim_showcase_picture",
        "NFS5.ChroniclePictureIcon" | "NFS5.ShowcasePicture" | "NFS5.ChroniclePicture" => "chronicle_or_showcase_picture",
        "NFS5.CarLogo" => "car_logo",
        "NFS5.KNOCKOUTTRACKLIST" => "knockout_track_list",
        "NFS5.ChronicleMainBoxControl" => "chronicle_main_box",
        "NFS5.AnimatingPicture" => "animating_picture",
        "NFS5.HEADER" | "NFS5.SUBHEADER" | "REAL.LABEL" | "NFS5.PICTURE" | "NFS5.NonLocalizedLabel" | "NFS5.ExtraCircle" | "NFS5.OutlineRectangle" | "NFS5.Rectangle" | "NFS5.BEVELEDOPTIONBOX" | "NFS5.GLINTYLINE" => "template_classes",
        "NFS5.CheckBox" => "check_box",
        "NFS5.CarSettingSlider" | "NFS5.CHRONICLESLIDER" => "car_setting_slider",
        "NFS5.ComboBoxSlider" | "NFS5.ColorBox" => "combo",
        "NFS5.GOTOBUTTON" => "goto_button",
        "NFS5.BOTTOMBARBUTTON" | "NFS5.GRAPHICTABBUTTON" => "bar_or_graphic_tab",
        "NFS5.TABBUTTON" | "NFS5.CAMERASELECT" | "NFS5.CAMERAPICTURE" => "tab_or_camera",
        _ => "default",
    }
}

/// The literals the dispatch names, and near misses (case, prefix).
const DISPATCH_PROBES: &[&str] = &[
    "REAL.BLACK_640x480", "REAL.BLACK", "REAL.BLACKX", "REAL.black_640x480", "REAL.LABEL", "REAL.label", "NFS5.GOURAUDBOX", "NFS5.GouraudBox",
    "NFS5.GouraudIndicatorBar", "NFS5.GlintyRectangle", "NFS5.PANEL", "nfs5.panel", "NFS5.3DCAR", "NFS5.3dcar", "NFS5.DEADZONE", "NFS5.INPUTBOX",
    "NFS5.CHATINPUTBOX", "NFS5.CHATDISPLAYBOX", "NFS5.CHARVECTACCESSORINPUTBOX", "NFS5.CONTROLLERSETUP", "NFS5.ListBox", "NFS5.LISTBOX",
    "NFS5.RADIOGROUP", "NFS5.TrackSmall2dSpline", "NFS5.PRIZERACEMAP", "NFS5.FACTORYMAP", "NFS5.PLAYERPICTURE", "NFS5.MINIPLAYERPICTURE",
    "NFS5.BADGE", "NFS5.MINIBADGE", "NFS5.FACTORYPIC", "NFS5.HUD", "NFS5.MOVIEBOX", "NFS5.MOVIECIRCLE", "NFS5.ACCOUNTTHING",
    "NFS5.ANIMSHOWCASEPICTURE", "NFS5.ChroniclePictureIcon", "NFS5.ShowcasePicture", "NFS5.ChroniclePicture", "NFS5.CarLogo",
    "NFS5.KNOCKOUTTRACKLIST", "NFS5.ChronicleMainBoxControl", "NFS5.AnimatingPicture", "NFS5.HEADER", "NFS5.SUBHEADER", "NFS5.PICTURE",
    "NFS5.NonLocalizedLabel", "NFS5.ExtraCircle", "NFS5.OutlineRectangle", "NFS5.Rectangle", "NFS5.BEVELEDOPTIONBOX", "NFS5.GLINTYLINE",
    "NFS5.CheckBox", "NFS5.CarSettingSlider", "NFS5.CHRONICLESLIDER", "NFS5.ComboBoxSlider", "NFS5.ColorBox", "NFS5.GOTOBUTTON",
    "NFS5.BOTTOMBARBUTTON", "NFS5.GRAPHICTABBUTTON", "NFS5.TABBUTTON", "NFS5.CAMERASELECT", "NFS5.CAMERAPICTURE", "NFS5.CARSETTINGBUTTON",
    "NFS5.VerticalSlider", "", "NFS5.", "something.else",
];

/// Every class of every shipped layout (empty when the game is not installed).
fn shipped_classes() -> std::collections::BTreeSet<String> {
    let mut set = std::collections::BTreeSet::new();
    if let Ok(paths) = GamePaths::locate()
        && let Ok(dir) = std::fs::read_dir(paths.fe_data().join("Layouts"))
    {
        for f in dir.flatten() {
            if f.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lay"))
                && let Ok(bytes) = std::fs::read(f.path())
            {
                for e in Layout::parse(&String::from_utf8_lossy(&bytes)).elements {
                    set.insert(e.class);
                }
            }
        }
    }
    set
}

/// T1(d), the guard written before the refactor: the table class -> draw arm of the string
/// `match`, hashed (FNV-1a 64) over the probe literals, and the shipped classes all land in an
/// arm that exists.
#[test]
fn the_string_dispatch_table_is_pinned() {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in DISPATCH_PROBES {
        for b in format!("{c}={}\n", old_draw_arm(c)).bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01b3);
        }
    }
    assert_eq!(h, 14941361031669849365, "pinned hash of the old dispatch table");
    for c in shipped_classes() {
        assert!(!old_draw_arm(&c).is_empty());
    }
}

/// The draw arm the new `sprites_reporting_bg` takes for a [`WidgetClass`] — its `match`, with
/// the arm labels of `old_draw_arm`.
fn new_draw_arm(c: WidgetClass) -> &'static str {
    use WidgetClass::*;
    match c {
        ScreenFill => "screen_fill",
        GouraudBox => "gouraud_box",
        GouraudIndicatorBar => "gouraud_indicator_bar",
        GlintyRectangle => "glinty_rectangle",
        Panel => "panel",
        Car3d => "3d_car",
        DeadZone => "deadzone",
        InputBox | ChatInputBox | ChatDisplayBox | CharVectAccessorInputBox | ControllerSetup => "text_boxes",
        ListBox => "list_box",
        RadioGroup => "radio_group",
        TrackSmall2dSpline => "track_small_spline",
        PrizeRaceMap | FactoryMap => "race_or_factory_map",
        PlayerPicture | MiniPlayerPicture | Badge | MiniBadge | FactoryPic | Hud => "pack_pictures",
        MovieBox | MovieCircle => "movie",
        AccountThing => "account_thing",
        AnimShowcasePicture => "anim_showcase_picture",
        ChroniclePictureIcon | ShowcasePicture | ChroniclePicture => "chronicle_or_showcase_picture",
        CarLogo => "car_logo",
        KnockoutTrackList => "knockout_track_list",
        ChronicleMainBoxControl => "chronicle_main_box",
        AnimatingPicture => "animating_picture",
        Header | SubHeader | RealLabel | Picture | NonLocalizedLabel | ExtraCircle | OutlineRectangle | Rectangle | BeveledOptionBox | GlintyLine => "template_classes",
        CheckBox => "check_box",
        CarSettingSlider | ChronicleSlider => "car_setting_slider",
        ComboBoxSlider | ColorBox => "combo",
        GotoButton => "goto_button",
        BottomBarButton | GraphicTabButton => "bar_or_graphic_tab",
        TabButton | CameraSelect | CameraPicture => "tab_or_camera",
        CarSettingButton | Other => "default",
    }
}

/// T1(d): for every probe literal and every class of the shipped layouts, the parsed enum lands
/// in the arm the old string `match` took, and the click / press facts agree with the old lists.
#[test]
fn the_widget_class_enum_dispatches_like_the_string_match() {
    let mut all: std::collections::BTreeSet<String> = DISPATCH_PROBES.iter().map(|s| s.to_string()).collect();
    all.extend(shipped_classes());
    for c in &all {
        assert_eq!(new_draw_arm(WidgetClass::from_class(c)), old_draw_arm(c), "class {c:?}");
        let old_click = matches!(c.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX");
        assert_eq!(WidgetClass::from_class(c).takes_click(), old_click, "takes_click {c:?}");
        let old_press = ["NFS5.CARSETTINGBUTTON", "NFS5.DEADZONE", "NFS5.GouraudIndicatorBar"].contains(&c.as_str());
        assert_eq!(WidgetClass::from_class(c).takes_focus_on_press(), old_press, "press {c:?}");
    }
}
