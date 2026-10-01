//! Front-end screens from `FEData/Layouts/*.lay`.
//!
//! The `.lay` file is the screen's *appearance*: a list of widgets with a class, a position and size in
//! the original 640×480 design space, colours, and for the picture widgets an `.fsh` in `FEData/Art`
//! plus the entry index inside it. What a widget *does* — and, for a few of them, where it is placed —
//! lives in the executable and is not in the file.
//!
//! This module draws the widgets the file fully describes: the screen background, the solid, outlined
//! and gouraud boxes, the glinty lines and rectangles, the pictures, and every text widget — each in
//! its own `fontPathName` font, its own `foreColor` and its own `align`. `NFS5.HEADER` carries no
//! `pos`, but the `pos` attribute is registered with a zero default (0x524735), so the class default
//! *is* the screen origin and the header draws there like any other label. A widget whose `stringID`
//! is 0 draws nothing: that id is the shipped "Hello, World!" placeholder. The widgets whose
//! *content* comes from the executable (`NFS5.3DCAR`, `NFS5.ListBox`, `NFS5.CONTROLLERSETUP`, and
//! an `NFS5.AccessorWatch` whose accessor the port cannot resolve) are counted and skipped rather
//! than guessed at; `unrendered()` reports them. `NFS5.CarLogo` *is* drawn once the caller binds a
//! car to it with [`LayoutScreen::set_car_logo`]: the pack and the entry pair both come from the
//! car's own `nfs5.car` record (+0x147 and +0x1F6).
//!
//! **Alignment** is the original's (`FeLabel::draw` 0x5232AD and `FeText_draw` 0x4DA3D7, CONFIRMED):
//! `align` 0/1/2 anchors the text at the widget box's left edge / midpoint / right edge, and the
//! string is then drawn at `anchor`, `anchor − width/2` or `anchor − width`, with the result rounded
//! to whole pixels. The front end's default font is `Univers8b.ffn` (loaded at FE start-up,
//! 0x4C72B1).
//!
//! The design space is 4:3, so the screen is letterboxed inside the window rather than stretched.

use nfs_assets::GamePaths;
use nfs_assets::texture::decode_pack;
use nfs_formats::ctr::ControlLibrary;
use nfs_formats::lay::Layout;
use nfs_formats::locale::Strings;
use nfs_renderer::{Renderer, Sprite, TextureData, TextureId};
use std::collections::HashMap;
use std::sync::Arc;

use crate::text::BitmapFont;

mod bar_oval;
mod bevel;
mod career_tabs;
mod chronicle;
mod combo;
mod deadzone;
mod dialog_draw;
mod end_of_era_draw;
pub mod fe_depth;
mod geometry;
pub mod glints;
mod input;
mod lists;
mod movie;
pub mod loading;
pub mod nav;
mod parts;
mod picker;
mod text_draw;
mod transition;
mod wrap_glint;
mod widgets;
use widgets::WidgetDraw;
pub use geometry::*;
pub use wrap_glint::*;
use movie::{filename_illegal_char, MoviePlayer};
#[cfg(test)]
use movie::movie_advance_frames;

/// The front end's and the HUD's string tables, as the original loads them (`RE/notes/hud.md`
/// "FEStrings.loc"):
///
/// ```text
/// 0x4E04F0 (FE start-up, 0x4D366E)
///   sprintf(buf, "%sFEStrings.loc", [0x65B340] = install.txt line 40, FEData\locale\)  0x4E0532
///   file = 0x59D8E0(buf, 0x4E07B0())       ; 0x4E07B0 is the FEMemory pool [0x5D9B48], not a language
///   push file onto [0x690714]             ; the only table 0x4E06D0 walks
///   lang = 0x4B73F0([0x65B39C])           ; the registry string "Language"
///   lang >= 7 -> 0, lang < 0 -> 6; [0x690710] = lang   (0x4E0586…0x4E0598; read back by 0x4E0750)
///   0x536D80(file, 0, lang) for every table
/// 0x41E170 (HUD start-up): sprintf("%shud.loc", [0x65B310] = line 28, gamedata\dashhud\),
///   0x59D8E0, [0x60C974] = file, 0x536D80(file, 0, 0x4B73F0())       (0x41E1BF…0x41E1FE)
/// ```
///
/// 0x4B73F0 maps the registry value `Language` (HKLM, REG_SZ; written as `"English"` when it is
/// missing, 0x4B7434…0x4B7474) by exact, case-sensitive compare: `English` 0, `German` 1,
/// `French` 2, `Spanish` 3, `Italian` 4, `Swedish` 5, `PortBrzl` 6, anything else 0
/// (0x4B74DD…0x4B76E6). The port has no Windows registry; it reads the value from the install's
/// registry exports (`GamePaths::language`, `game_info/regs/*.reg`), with the original's default.
/// The same index is what the original copies into the `LANGUAGE` option (id 0x32, `[0x657C84]`,
/// 0x4E0750 → 0x4D53A9 / 0x4ED6CB) that the loading screen's `loadtext.fsh` reads.
/// *(Until 2026-09-27 the port took `fe.txt`'s `language` instead — an APPROXIMATION.)*
///
/// **Fallback (the port's, not the original's):** without `FEStrings.loc` the port reads that
/// language's column of `FEStrings.csv` (English first; the original has no CSV reader and would
/// dereference the null table). Without `hud.loc` the HUD reads the CSV's `hud` rows, else the
/// front end's table.
///
/// The front end also needs the table for one geometry rule: a `TEXT` or `LOCALIZEDTEXT` part's
/// `size` is the **extent of the string it draws**, which several templates then position other
/// parts from (`BottomBarButton.ctr`'s own `pos.value` is
/// `(data.align&1)?data.posx-(label.size.width+picture.size.width+data.space):data.posx`).
pub fn load_strings(paths: &GamePaths) -> Strings {
    let raw = paths.language();
    // 0x4E0586: 7 and above fold to 0 (0x4B73F0 answers 0…6, so the fold never acts)
    let lang = if raw >= 7 { 0 } else { raw };
    let table = |dir: std::path::PathBuf, name: &str| {
        let mut t = nfs_formats::loc::LocTable::parse(std::fs::read(GamePaths::find_file(&dir, name)?).ok()?).ok()?;
        t.set_language(lang);
        Some(t)
    };
    let hud = table(paths.game_data().join("DashHud"), "hud.loc");
    if let Some(fe) = table(paths.fe_data().join("Locale"), "FEStrings.loc") {
        return Strings::from_loc(&fe, hud.as_ref());
    }
    tracing::warn!("no FEStrings.loc: falling back to FEStrings.csv");
    let csv = GamePaths::find_file(&paths.fe_data().join("Locale"), "FEStrings.csv").and_then(|p| std::fs::read(p).ok());
    let mut s = csv.map(|b| Strings::parse(&b, lang as usize)).unwrap_or_default();
    if let Some(h) = &hud {
        s.hud_by_id = Strings::from_loc(h, None).fe_by_id;
        s.hud_table = true;
    }
    s
}

/// Read `FEData/Controls/*.ctr` — the widget class templates that hold every attribute's default
/// and the pieces each class is made of (`RE/notes/controls.md`).

pub fn load_controls(paths: &GamePaths) -> ControlLibrary {
    let dir = paths.fe_data().join("Controls");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        tracing::debug!(dir = %dir.display(), "no control templates");
        return ControlLibrary::default();
    };
    // `.ctr` is a control template and **`.scr` a screen one** — the directory ships exactly one,
    // `Black Screen 640x480.scr`, which is the template for `REAL.BLACK_640x480`, the id every
    // layout's `[section.0]` carries. It has the same shape as a `.ctr` and the port used to skip
    // it, so the one class that every screen is built on had no template at all here.
    let files = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("ctr") || x.eq_ignore_ascii_case("scr"))).filter_map(|e| {
        let bytes = std::fs::read(e.path()).ok()?;
        Some(bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect::<String>())
    });
    let lib = ControlLibrary::from_files(files);
    tracing::debug!(classes = lib.len(), "control templates loaded");
    lib
}

/// A widget the player can operate: a button that leads somewhere or runs a callback, or a combo
/// slider that edits the accessor it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// index into `layout.elements`
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub destination: Option<String>,
    pub callback: Option<String>,
    pub accessor: Option<String>,
    pub string_id: Option<u32>,
    /// `data.offset`, which is **what the button does to the screen stack**, not a position:
    /// `NFS5.GOTOBUTTON`'s bind takes it into `[this+0x144]` (0x4C8F55) and its slot 9 hands the
    /// attribute's own int, `[+0x38]`, to `0x4D2100(destination, offset, 1)` as the second
    /// argument (0x4C94BD…0x4C94CF). `RE/notes/layouts.md`.
    pub offset: i32,
    /// `data.step` — what `NFS5.CARSETTINGBUTTON` adds to its accessor's index when it is pressed.
    /// Its bind takes it into `[this+0x144]` (0x4C1ED7) and its **slot 27**, 0x4C1F20, hands it to
    /// `accessor->slot19(step)`; slot 5 (0x4C1F6C) takes focus and starts 0x529220's 150 ms
    /// repeat, and slot 10 (0x4C1F80) runs slot 27 again on every tick of it. Seventeen of the 34
    /// shipped buttons carry **1** and seventeen **−1** — the left and right arrow of each pair —
    /// so the sign is the whole point. §4.84.
    pub step: i32,
}

/// `NFS5.WORDWRAPACCESSORWATCH`'s own state beyond the accessor watch's (vtable 0x5BAEF4): the
/// text its word-wrap object was built from and the change it is fading out of.
///
/// ```text
/// 0x522EC0  tick: the watch's own tick 0x4BD230 (the label's text from the accessor), then
///           not changing and the label's text != the copy [+0x198]  -> changing [+0x1A0] = 1,
///                                                                        [+0x1A4] = now
///           changing and (now − [+0x1A4] > 0x40, or not animating)  -> rebuild the wrap from the
///           new text (0x523130 / 0x523080), copy it, [+0x184] = [+0x188] = now, changing = 0
/// 0x522DC0  draw: changing -> the colour's alpha times clamp(([+0x1A4] − now)·4 + 0x100, 0, 0xFF)
///           (0x4D87F0) — the old text fades out over 64 ticks
/// ```
///
/// So an animating watch (`data.animating`, `[+0x180]`) whose text changes fades the old text out
/// in half a second and types the new one from its first letter (0x5232E5's budget
/// `([+0x188] − [+0x184]) / 3` starts again from 0); one that does not animate takes the new text on
/// the same tick. The shipped animating ones are `tournament`'s description and the four narrative
/// screens'.
#[derive(Debug, Clone, PartialEq)]
pub struct WrapWatch {
    /// `[+0x198]` — the text the wrap was built from
    pub text: String,
    /// `[+0x1A4]` while `[+0x1A0]` is set — the tick the change was seen
    pub changing: Option<i64>,
    /// `[+0x184]` after a rebuild — the tick the typing starts again from; `None` until the first
    /// change, while the activation's wait (`typewriter`) still rules
    pub restart: Option<i64>,
}

impl WrapWatch {
    /// One tick of 0x522EC0 against the label's current `text` at tick `now`.
    pub fn tick(&mut self, text: &str, now: i64, animating: bool) {
        if self.changing.is_none() && self.text != text {
            self.changing = Some(now);
        }
        if let Some(at) = self.changing
            && (now - at > 0x40 || !animating)
        {
            self.text = text.to_string();
            self.restart = Some(now);
            self.changing = None;
        }
    }

    /// The draw's fade while a change runs (0x522DC4…0x522E10), as a factor on the alpha.
    pub fn fade(&self, now: i64) -> f32 {
        match self.changing {
            Some(at) => ((at - now) * 4 + 0x100).clamp(0, 0xFF) as f32 / 255.0,
            None => 1.0,
        }
    }
}

/// The typing of an animating word-wrap widget — `NFS5.WORDWRAPCONTROL`'s `[+0x14C]`, `[+0x150]`
/// and `[+0x158]`, `NFS5.WORDWRAPACCESSORWATCH`'s `[+0x184]`, `[+0x188]` and `[+0x18C]`; the two
/// classes run the same code on their own offsets:
///
/// ```text
/// 0x5229A0 / 0x522CB4  activate: start = last = now; wait = animating ? 0x97 : 0
/// 0x522A52 / 0x522E14  each draw: 0x4C8820 (the transition's fade) == 0 -> last = now
/// 0x522A6E / 0x522E30  wait != 0: now − start <= wait -> draw nothing
///                                  else start = last = now, wait = 0, and draw nothing
/// 0x522AE2 / 0x522EA4  else the text object 0x523290 gets (start, last): with the flag set it
///                      reveals (last − start) / 3 characters (0x5232FB, a signed divide)
/// 0x52301A             the watch's rebuild on a new text: start = last = now (wait untouched)
/// ```
///
/// So the typing starts from **the tick of the first draw that finds the wait over**, not from
/// tick 152 — a frame that spans several ticks moves it — and it **stops growing while the screen
/// is in a transition**, since `last` is only restamped when the fade is 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Typing {
    pub start: i64,
    pub last: i64,
    pub wait: i64,
}

impl Typing {
    /// The activation (0x5229A0 / 0x522CB4) at tick `now`.
    pub fn new(now: i64, animating: bool) -> Self {
        Typing { start: now, last: now, wait: if animating { 0x97 } else { 0 } }
    }

    /// One draw at tick `now`: `None` draws nothing, `Some(n)` reveals `n` characters (the caller
    /// shows the whole text when the widget does not animate).
    pub fn draw(&mut self, now: i64, transitioning: bool) -> Option<usize> {
        if !transitioning {
            self.last = now;
        }
        if self.wait != 0 {
            if now - self.start > self.wait {
                self.start = now;
                self.last = now;
                self.wait = 0;
            }
            return None;
        }
        Some(((self.last - self.start) / 3).max(0) as usize)
    }

    /// The watch's rebuild (0x52301A).
    pub fn restart(&mut self, now: i64) {
        self.start = now;
        self.last = now;
    }
}

/// The design space every `.lay` is authored in.
pub const DESIGN: (f32, f32) = (640.0, 480.0);

/// One event of the tournament a `NFS5.TrackSmall2dSpline` watches, as the widget needs it.
///
/// `FETrackSmall2dSpline::update` (0x4FCB40) asks its `Tournament.Tournament` accessor which
/// tournament is selected (0x4FC9A6 registers it, 0x4FC9B9 keeps the object), takes the event
/// count from `trn+0x39C` (0x503370) and, for event `i`, resolves `trn+0x3A0+i*0xA4` — the track
/// id — into the `nfs5.trk` record (0x5033D0 → 0x4FBF30). From that record it takes **`+0x58`**,
/// the small art name, and loads `<TrackArt>/<name>.fsh` (0x4FCC7E, `"%s%s.fsh"`), keeping its
/// entries **0, 1 and 2** (0x4FCCA5, 0x4FCCB4, 0x4FCCC0).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackMapEvent {
    /// the `nfs5.trk` record's `+0x58` — `alpsSpSm` and the like, without the extension
    pub art: String,
    /// the event's `+4`: with it set the widget draws entry **2** (`f`) instead of entry 1 (`s`),
    /// because a point-to-point track driven backwards starts where it normally finishes
    /// (0x4FCDFC…0x4FCE0F). The shipped `nfs5.trn` sets it on 39 of its 130 events.
    pub backwards: bool,
    /// the event's `+5`, handed to the blit as its fifth argument: when set the marker's x is
    /// mirrored across the map, `map.x + map.w − marker.x − marker.w` (0x4FCE9F…0x4FCEC8). No
    /// shipped event sets it.
    pub mirror: bool,
    /// the track's name in caps — `FEStrings[trk+4 + 20]`, which is what 0x4FCFAD…0x4FCFBE looks
    /// up for the list beside the map
    pub name: String,
}

/// A resolved [`TrackMapEvent`]: the art, already uploaded, at the place its own header gives.
struct TrackMapArt {
    /// texture, width, height, x, y
    map: Option<(TextureId, f32, f32, f32, f32)>,
    marker: Option<(TextureId, f32, f32, f32, f32)>,
    name: String,
}

pub struct LayoutScreen {
    pub layout: Layout,
    /// The background screen this one is laid over (`screen=` → [`background_layout`]).
    background: Option<Box<LayoutScreen>>,
    /// `("Extras.fsh", 3)` → texture and the entry's own pixel size
    shapes: HashMap<(String, usize), (TextureId, f32, f32)>,
    /// `fontPathName` (lower-cased) → font, plus the front end's default
    fonts: HashMap<String, BitmapFont>,
    white: TextureId,
    /// how long the cursor has been animating, for the frame of a multi-frame shape (§4.152).
    /// `u32::MAX` marks a screen that is somebody's background and draws no pointer.
    cursor_ms: u32,
    /// `[0x690794]` and `[0x690790]`: the shape the clock belongs to and the millisecond it was
    /// first requested, restamped when the request changes (0x4E1CCD…0x4E1CE9)
    cursor_shape_at: std::cell::Cell<(usize, u32)>,
    /// the stamp the two arms of 0x4E1D3E keep for each other: the button state and the
    /// millisecond it last changed
    cursor_btn_at: std::cell::Cell<(bool, u32)>,
    /// `[0x69CAE8]` itself — whether a mouse button is down (0x5299C0)
    cursor_btn: bool,
    /// where the 3D room's turntable stands, in turns — what `FECar3DControl`'s zone test reads
    /// out of the room through `[[0x65B928]+0xE8]`'s slot 0xB4 (§4.153)
    car_spin: f32,
    /// each `NFS5.3DCAR`'s opening parts — `FECar3DControl`'s slot 9 and its step
    /// (`crate::carparts`, §4.153), by widget index
    car_parts: HashMap<usize, crate::carparts::CarParts>,
    /// the `NFS5.3DCAR` widgets whose car has a roof animation (`set_car_roof`)
    car_roof: std::collections::HashSet<usize>,
    /// `[screen+0x261]`, "covered" — set by the manager's 0x4D2000 while a dialog is up, and by
    /// 0x4D1E70 on every layer but the top; a covered screen's widgets answer `isDisabled` and draw
    /// `[state.disabled]` where their template has it (§4.147)
    covered: bool,
    /// `(combo element, row)` — the open drop-down's hovered row, kept from the last frame the
    /// pointer was actually on the list. The original keeps it in `[this+0x1A8]` with
    /// `[this+0x1A4]` as "there is one", and **`FEComboBox`'s leave handler 0x4C6F00 does not
    /// clear either when the pointer has gone onto the list's own scrollbar** — 0x4C6F21 asks the
    /// scrollbar at `[this+0x1B8]` whether it has the pointer now and returns before the
    /// `mov byte [esi+0x1A4], 0` at 0x4C6F31. So the row stays lit while you reach for the bar.
    combo_hover: std::cell::Cell<Option<(usize, usize)>>,
    /// the open drop-down's depth mask in the frame's `overlay` list — the stencil
    /// `[.0 .. .1)`, the masked draws `[.1 .. .2)` — given its depth when the list is appended,
    /// which is where the original's later pass draws it (`drop_down`)
    overlay_mask: std::cell::Cell<Option<(usize, usize, usize)>>,
    /// widget classes the file does not place, with how many of each were skipped
    unrendered: Vec<(String, usize)>,
    /// element index of an `NFS5.CarLogo` → the `.fsh` pack and the first of its two entries
    logos: HashMap<usize, (String, usize)>,
    /// `NFS5.PANEL` element indices the screen deliberately leaves **closed** — the tabs that are
    /// not the selected one, and the CD player. A closed panel draws nothing (0x4CEBC0 returns at
    /// once when `[panel+0x14C]` is null), so it is not a gap; a panel whose file simply would not
    /// load is not in here and is still counted.
    closed: std::collections::HashSet<usize>,
    /// A tab strip's pending button, `FERadioGroup`'s `[+0x158]`: `(strip, button)` — the strip's
    /// index here and the button's in the strip. Its tick (`tick_panels`) makes it current once
    /// the current button is no longer selected.
    pending_tab: Option<(usize, usize)>,
    /// The transition a panel's owner hands its sub-screen for the draw — `(going out, progress)`
    /// of the screen above, `(false, 0)` on a top-level one (see `widget_state`).
    inherited: std::cell::Cell<(bool, i32)>,
    /// `NFS5.RADIOGROUP` or `NFS5.PANEL` element index → the layout its `filename` names, loaded
    /// and drawn inside it (`RE/notes/controls.md` §4.1). Four screens embed a tab strip this way:
    /// `Controllers` takes `controllertabs`, `Options` `optiontabs`, `graphics` `graphicstabs` and
    /// `showcase` `showcasetabs`. `Radio Group.Ctr` gives the class one attribute of its own,
    /// `filename`, and nothing drawable; a panel is the container those strips switch between.
    groups: HashMap<usize, Box<LayoutScreen>>,
    /// element index of a `NFS5.CheckBox` → whether its `boolAccessor` is true. A box with no
    /// entry is **unticked**, which is what the original's own registration says: a bool accessor
    /// is a two-entry list (0x4B8720 hands 0x4BC1F0 a count of 2) and every `QUICKFIX.*` is built
    /// with a default index of 0 (0x4F2584).
    checks: HashMap<usize, bool>,
    /// element index of a `NFS5.CarSettingSlider` → the accessor's index and its list's length,
    /// which is what puts the knob along the track (0x4D0C0C…0x4D0C68)
    knobs: HashMap<usize, (i32, i32)>,
    /// `NFS5.GouraudIndicatorBar` element index → the 0..100 value its `othersource` resolved to.
    /// A bar with no entry here draws nothing, the way an unresolved `AccessorWatch` shows nothing.
    bars: HashMap<usize, i32>,
    /// element index of an `NFS5.ListBox` → its rows and the accessor's current row
    lists: HashMap<usize, (Vec<ListRow>, Option<usize>)>,
    /// element index of an `NFS5.ColorBox` → its accessor's current colour, `[+0x2C]` — what the
    /// closed box shows as a swatch (0x4C60FA…0x4C618B, §4.171.4)
    swatches: HashMap<usize, [u8; 4]>,
    /// element index of an `NFS5.3DCAR` → the offscreen texture its car was rendered into, drawn
    /// full-screen in the widget's own place in the drawing order
    car_views: HashMap<usize, TextureId>,
    /// the widget class templates (`FEData/Controls/*.ctr`), shared with the background screen
    controls: Arc<ControlLibrary>,
    /// the string table, shared the same way — a text part's `size` is its string's extent
    fe_strings: Arc<Strings>,
    /// element index of a `NFS5.TrackSmall2dSpline` → the events of the tournament it watches
    track_maps: HashMap<usize, Vec<TrackMapArt>>,
    /// element index of a `NFS5.MOVIEBOX` → the `.mad` it is playing
    movies: HashMap<usize, MoviePlayer>,
    /// element index of a `NFS5.KNOCKOUTTRACKLIST` → how many rows it has and which one is current
    ko_strip: HashMap<usize, (usize, usize)>,
    /// element index of a `NFS5.BADGE` → the player's name, the rank line under it and the colour
    /// the name is printed in
    badges: HashMap<usize, (String, String, [f32; 4])>,
    /// milliseconds this screen has been up, for the map widget's own cycle (0x4FCB50)
    clock_ms: u32,
    /// each `NFS5.MOVIECIRCLE`'s overlay (0x4CDC80), made on its first draw
    movie_overlays: std::cell::RefCell<HashMap<usize, crate::movie_overlay::MovieOverlay>>,
    /// the credits roll the movie circle draws on the credits screen (0x4CE0F0)
    credits_roll: std::cell::RefCell<Option<crate::movie_overlay::CreditsRoll>>,
    /// each `NFS5.WORDWRAPACCESSORWATCH`'s text as it last laid it out, and its change fade
    /// (`WrapWatch`, 0x522EC0 / 0x522D60), made on its first draw
    wrap_watch: std::cell::RefCell<HashMap<usize, WrapWatch>>,
    /// each animating word-wrap widget's typing stamps ([`Typing`]), made on its first draw
    typing: std::cell::RefCell<HashMap<usize, Typing>>,
    /// each glinting widget's own timer (`glints`), and whether a draw has stepped it yet
    glints: std::cell::RefCell<HashMap<usize, (glints::Glint, bool)>>,
    /// the ticks `NFS_FE_CLOCK` preloaded the clock with — 0 in a game
    preload_ticks: i64,
    /// each `NFS5.AccessorWatch`'s `label.text` as its tick last wrote it (0x4BD32C, 0x526980) —
    /// what the part measures its `size` over (0x4E7160); before the first draw, the template's
    watch_text: std::cell::RefCell<HashMap<usize, String>>,
    /// each widget's last state name the template actually had an entry for ([`drawn_state`](Self::drawn_state)) —
    /// 0x5271F0 (§"drawn_state") copies nothing when the live state has no entry, so the part
    /// list it draws is whatever the last successful match left it as, not the current state
    last_drawn_state: std::cell::RefCell<HashMap<usize, String>>,
    /// `FEEndOfEraCallBackList`'s show, when this is the end-of-era screen (`set_end_of_era`)
    end_of_era: Option<end_of_era_draw::EndOfEraScreen>,
    /// the bottom bar's career era tabs, loaded by its slot 21 0x4D7FD0 (`load_career_tabs`)
    career_tabs: Option<career_tabs::CareerTabs>,
    /// the bottom bar's oval flash `[bar+0x278]` (0x520AD0 / 0x520B80, `load_bar_oval`)
    bar_oval: Option<bar_oval::BarOval>,
    /// `[0x65B940]`, the name-box codes the player has typed (`nfs_game::cheats`) — bit 6 is read
    /// by the credits roll
    pub cheat_bits: u32,
    /// `[0x5D8F2C]`, the driver picture the front end has chosen: 8 in the image, written by
    /// `CREATEPLAYERENTER`'s 0x4CF790(8) and by `NFS5.PLAYERDRIVERPICTURE`'s release 0x4CF940.
    /// The app owns the global and hands it to each screen it opens; the pictures' draw 0x4CF7C0
    /// compares it against their own `data.ournumber` (0x4CF83B, 0x4CF8E1).
    pub driver_picture: i32,
    /// the `weeicons.fsh` entry tagged `GLNT`, when this screen has a `NFS5.HEADER` to sweep it
    /// across — `RE/notes/controls.md` §4.25
    glint: Option<usize>,
    /// the `weeicons.fsh` entry tagged `COLB`, the one sprite `NFS5.ColorBox`'s picker draws
    colb: Option<usize>,
    /// the `weeicons.fsh` entries tagged `arc0`…`arc7`, which `NFS5.CONTROLLERSETUP` looks up by
    /// name (`"arc%d"` 0x51C9DA → 0x4D7B20 over the `"%sweeicons.fsh"` its constructor 0x51BFEE
    /// names) — `RE/notes/controls.md` §4.117
    arcs: Option<[usize; 8]>,
    /// where the pointer is, in design coordinates, when it is over the window at all
    /// (`RE/notes/controls.md` §4.39)
    cursor: Option<(f32, f32)>,
    /// The pointer the **cursor sprite** is drawn at, when it is not `cursor`. The cursor draw
    /// 0x4E1C90 takes the device's position straight from 0x5372A0(6) (0x4D32D9…0x4D32EF), not
    /// from an event, so the pointer moves while the input dispatch 0x4D2850 is dropping the
    /// move events the widgets' hover comes from (`set_pointer`). `RE/notes/controls.md` §4.171.
    pointer: Option<Option<(f32, f32)>>,
    /// whether the left button is held (`RE/notes/controls.md` §4.40 — type 4 down, type 8 up)
    pressed: bool,
    /// the widget that holds the mouse, if one does — `[window+0x120]`, which a widget takes when
    /// it gains focus and gives back when it loses it (`RE/notes/controls.md` §4.57)
    capture: Option<usize>,
    /// how long the pointer has rested on one widget, and which — the tooltip of §4.52 needs
    /// `[widget+0x12C]`, the tick the hover started
    hover_ms: u32,
    hover_of: Option<usize>,
    /// the first row each list draws — `[this+0x1BC]->slot10()` in the original
    /// (`RE/notes/controls.md` §4.45)
    scroll: HashMap<usize, usize>,
    /// the combo slider whose drop-down list is open, by this screen's own element index
    /// (`RE/notes/controls.md` §4.43 — a combo's list is open exactly while it holds focus)
    open_combo: Option<usize>,
    /// `NFS5.ColorBox`'s latch `[0x65B710]` — raised by 0x4C39F0 when the row chosen in a colour
    /// box's list is the custom row (0x4C6D63…0x4C6D6C), read by 0x4C3A00; the editor draws while
    /// the box holds focus and this is up (0x4C3D0C). §4.171.
    picker_latch: bool,
    /// `NFS5.ColorBox`'s picker state — `RE/notes/controls.md` §4.66
    picker: Picker,
    /// the analogue devices the front end can see, in enumeration order — the port's own stand-in
    /// for the record table at `[0x69C9EC]` (`RE/notes/controls.md` §4.17)
    pads: Vec<FePad>,
    /// which pad the `Controller.AllPads` accessor is on (`[accessor+0x28 + pad*4]` selects the
    /// stored value inside every one of the sixteen dead-zone accessors)
    pad_sel: usize,
    /// the sixteen `FEControllerDeadZoneAccessor` values, `[min, max]` per axis per pad — range
    /// 0…0x80, and **0 / 0x80 out of the factory** (0x51E546, 0x51E550)
    deadzone: Vec<[[i32; 2]; 8]>,
    /// the bar and handle a press grabbed — `[this+0x140]`, which slot 5 sets and slot 9 clears
    dz_grab: Option<(usize, u32)>,
    /// `[this+0x144]` — the handle the pointer is **over**, which slot 8 writes whenever nothing
    /// is held (0x51BD02). The draw paints that one amber and the rest dark red.
    dz_hover: Option<(usize, u32)>,
    /// `Camera.Current` — which of the four camera slots is on, for `NFS5.CAMERASELECT` (§4.89)
    camera_current: usize,
    /// `Camera.1 … Camera.4` — the view each of the four camera slots holds (0x4E4D40 registers
    /// them as 0, 1, 2, 3), which `NFS5.CAMERAPICTURE`'s draw 0x4BF870 compares with its own
    /// `data.picturenum`
    camera_list: [i32; 4],
    /// which of an open drop-down's scrollbar regions the pointer is on, and whether it is
    /// held — 1 up arrow, 2 down, 3 knob, 4 trough, the numbers 0x4DB2F0 writes (§4.101)
    combo_scroll: Option<(usize, u32, bool)>,
    /// The input boxes that have taken focus — `[widget+0x10C] & 0x40` (§4.109). A set, not one
    /// index, because that is what the binary does: `FeWidget::setFocused` 0x4C7D70 sets its own
    /// flag and calls 0x4C82A0, which is `setState`, and neither it nor anything on that path
    /// clears a sibling's. Cleared when the screen is rebuilt, as the widgets are.
    focused: std::collections::BTreeSet<usize>,
    /// What has been typed into each input box, before it is committed. The original keeps it in
    /// the **accessor** — `FEStringAccessor`'s own `char*` buffer, sized `maxLen + 1` by
    /// `0x59F020` in its ctor 0x4BC8B0 — and the widget only ever calls `vt[0x30]` to insert,
    /// `vt[0x38]` to erase and `vt[0x0C]` to commit. Keeping it beside the widget here is the same
    /// state in a different place: nothing else in this port reads a half-typed value.
    edits: HashMap<usize, String>,
    /// The widgets carrying the **`selected`** flag, `[widget+0x10C] & 8` (§4.109, §4.111). The
    /// front end sets it by name through the helper 0x50D9A0, not from any pointer state.
    selected_flags: std::collections::BTreeSet<usize>,
    /// The widgets whose `selected` flag has been **cleared** since the screen was built — a tab
    /// the block starts as `state=selected` (the bind's own flag) that another tab's click has
    /// since deselected (0x4CFF30 → the old tab's slot 28, 0x4D0310)
    unselected: std::collections::BTreeSet<usize>,
    /// The widgets the screen has **hidden** — `[widget+0x10C] & 0x20` (§4.112). A hidden widget
    /// draws `[state.hidden]`, whose part list in every shipped template is the non-drawable
    /// `data` alone, so it puts nothing on the screen.
    hidden_flags: std::collections::BTreeSet<usize>,
    /// The widgets the screen has **disabled** — `[widget+0x10C] & 0x10` (§4.113), which
    /// 0x4C831E turns into `[state.disabled]`.
    disabled_flags: std::collections::BTreeSet<usize>,
    /// `NFS5.ChroniclePicture`'s pan: where each widget's picture has been dragged to
    /// (`[widget+0x14C]`, `[+0x150]`), and, while a drag is on, which widget it is, where the
    /// pointer took hold (`[+0x144]`, `[+0x148]`) and the offset it had then (`[+0x154]`,
    /// `[+0x158]`). §4.119.
    picture_pan: std::collections::BTreeMap<usize, (f32, f32)>,
    picture_drag: Option<(usize, f32, f32, f32, f32)>,
    /// The five year-and-variant columns the chronicle's timeline is showing — what 0x507B4F's
    /// walk settles on and writes into `[this+0x160]` and `[this+0x174]` (§4.118, §4.122).
    chronicle_columns: Vec<(u32, usize)>,
    /// The timeline's icon pack, and the entry each (column, row) cell shows in its two states —
    /// `0x507340(year, row, variant, hot)`'s two slots (§4.118, §4.123).
    chronicle_icons: (String, std::collections::BTreeMap<(usize, usize), (usize, usize)>),
    /// One caption per timeline column — the `фото`-row (kind 0) entry's own `0x6F` text
    /// attachment (`nfs_formats::fsh::Fsh::entry_caption`), always the **resting** frame's
    /// (0x507340's fourth argument, `hot`, is a hardcoded `0` at the caption's own call site,
    /// 0x507C3B — §4.173), `None` when there is no picture-kind entry for the column or its name
    /// is empty (0x507CB9's `je`).
    chronicle_captions: Vec<Option<String>>,
    /// the six `WeeIcons.fsh` entries the combo scrollbar's two arrows cycle (§4.90, §4.101)
    combo_arrows: Option<[usize; 6]>,
    /// `Parts.AllParts`'s own collapsible-header icon — the ctor **0x515160** builds
    /// `"%sWeeIcons.fsh"` (0x5151C0: string 0x5D976C) and caches four entries by index —
    /// `pls1`(3)→`[this+0xA4]`, `min4`(0xB)→`[this+0xA8]`, `pls2`(4)→`[this+0xAC]`,
    /// `pls3`(5)→`[this+0xB0]` — plus a fifth, `BarD`(0xE)→`[this+0xB4]`, for the list's own
    /// scrollbar (already the generic `NFS5.ListBox` art above). The row draw **0x515620**
    /// picks one of the first four by `(al = [this+0x2C]`/`[this+0x2D]`, this section's own
    /// **open** flag) and a second flag at `[esp+0xcc]`, read to the instruction 2026-09-29: the
    /// row loop, `0x4C64FE`, calls the accessor's own vtable slot 15 (`call [eax+0x3C]` at
    /// `0x4C68F5`) with exactly 8 stack args (`0x515620` itself ends `ret 0x20`); the first-
    /// pushed (`[esp+0xcc]`, the 8th/last arg) is `ebx`, **0** by default (`0x4C672E`) and
    /// overwritten only when this row is the list's **selected-and-focused** row (`0x4C674C`'s
    /// `cmp edi,[this+0x1A8]` gated by the byte at `[this+0x1A4]`) or the row the accessor's own
    /// hover query (its vtable slot 10, called at `0x4C678E`) reports — the same two conditions
    /// this port already computes as `hover_row`/`current` for the row's highlight bar. The flag
    /// is that bar's own presence: `pls1` closed/`min4` open when neither holds, `pls2`
    /// closed/`pls3` open when either does. `[pls1, min4, pls2, pls3]`.
    parts_glyphs: Option<[usize; 4]>,
    /// a list's scrollbar knob parked at the pointer by a press on the **track**, until the first
    /// move puts the model somewhere — `RE/notes/controls.md` §4.46, the tail of 0x4D0995
    knob_at: Option<(usize, f32)>,
    /// where this screen is in its transition
    transition: Transition,
    /// drawn over another screen (the bottom bar, 0x4D39D3) — an overlay must not paint its own fill
    overlay: bool,
}

/// One analogue device as the front end sees it — this port's stand-in for a record of the
/// 0x104-byte table at `[0x69C9EC]`, which 0x51EDEA caches from `0x532E10(i, 6)`.
///
/// `analogue` is `[rec+0] == 4` and the info block's `[+4] & 1`; `axes` is that block's `[+8]`,
/// the **axis bitmask** `0x51F6F0` tests bit by bit; `values` are `[rec+4 + 4n]`, which
/// `0x51F7A0(device, 0x24 + n, &out)` answers. Axis `n` is bit `n`, in the order the sixteen
/// accessors are registered at 0x51E4C0 — `XAxis`, `YAxis`, `ZAxis`, `XRot`, `YRot`, `ZRot`,
/// `SliderA`, `SliderB`, which is `DIJOYSTATE`'s own field order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FePad {
    pub analogue: bool,
    pub axes: u8,
    pub values: [i32; 8],
}

/// The bits a `transon` / `transoff` name maps to (`FE_transitionBits` 0x4C88C0 — the name is
/// lower-cased first, and anything unrecognised is 0, "no transition").
pub mod trans {
    pub const SAME: u32 = 1;
    pub const LEFT: u32 = 2;
    pub const RIGHT: u32 = 4;
    pub const TOP: u32 = 8;
    pub const BOTTOM: u32 = 0x10;
    pub const FADE: u32 = 0x20;
}

/// `transon` / `transoff` → bits. CONFIRMED (0x4C88C0).
pub fn transition_bits(name: &str) -> u32 {
    match name.trim().to_lowercase().as_str() {
        "same" => trans::SAME,
        "left" => trans::LEFT,
        "right" => trans::RIGHT,
        "top" => trans::TOP,
        "bottom" => trans::BOTTOM,
        "fade" => trans::FADE,
        _ => 0,
    }
}

/// Progress runs from 0 — the widget in place — to 512, a whole screen away and fully faded
/// ([0x5B6B2C], 0x4C87CD). CONFIRMED.
pub const TRANS_FULL: i32 = 512;
/// The sweep takes half of the timer's ticks-per-second (`[0x5DEB48] / 2`, 0x4E0A36), and that is
/// **128**, not 1000: `TIMER_init` (0x565030) stores its own frequency argument into `[0x5DEB48]`
/// — defaulting to 100 and range-checked to 1…10000 — and `main` calls it as `TIMER_init(0x80)`
/// (`timing.md`, via 0x4A5410). So the fade is **64 ticks of the 128 Hz `TIMER_getTicks`**, which
/// is half a second.
///
/// *This comment used to say "the timer is the 1 kHz `timeSetEvent` set up at 0x564F2D, so half a
/// second". The number below was right and the reasoning was not: 1000/2 ticks at 1 kHz and 128/2
/// ticks at 128 Hz are both 0.5 s, so the error cancelled. Corrected 2026-09-21, because the next
/// person to touch either half would have had the two disagree.* CONFIRMED.
pub const TRANS_MS: u32 = 500;
/// …after three frames of nothing (`this+0x124` starts at 3, 0x4E09A0 / 0x4E09E8). CONFIRMED.
pub const TRANS_DELAY_FRAMES: u32 = 3;

/// Where a screen is in its transition. `In` is the original's state 1 (the progress falls to 0 and
/// the screen settles), `Out` its state 3 (the progress climbs to 512 and the screen is gone),
/// `Idle` its state 2 (0x4E0A3F…0x4E0AB4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Idle,
    In { frames: u32, ms: u32 },
    Out { frames: u32, ms: u32 },
}

impl Transition {
    /// 0 = in place, 512 = a whole screen away.
    pub fn progress(self) -> i32 {
        let t = |frames: u32, ms: u32| if frames > 0 { 0 } else { (ms as i64 * TRANS_FULL as i64 / TRANS_MS as i64) as i32 };
        match self {
            Transition::Idle => 0,
            Transition::In { frames, ms } => (TRANS_FULL - t(frames, ms)).max(0),
            Transition::Out { frames, ms } => t(frames, ms).min(TRANS_FULL),
        }
    }

    pub fn done(self) -> bool {
        match self {
            Transition::Idle => true,
            Transition::In { .. } => self.progress() == 0,
            Transition::Out { .. } => self.progress() == TRANS_FULL,
        }
    }

    /// One frame of the transition clock: the first [`TRANS_DELAY_FRAMES`] frames only count down,
    /// and the timer starts after them (0x4E09E8…0x4E0A16).
    pub fn advance(self, dt_ms: u32) -> Self {
        let step = |frames: u32, ms: u32| if frames > 0 { (frames - 1, ms) } else { (0, ms + dt_ms) };
        match self {
            Transition::Idle => Transition::Idle,
            Transition::In { frames, ms } => {
                let (frames, ms) = step(frames, ms);
                let next = Transition::In { frames, ms };
                if next.done() { Transition::Idle } else { next }
            }
            Transition::Out { frames, ms } => {
                let (frames, ms) = step(frames, ms);
                Transition::Out { frames, ms }
            }
        }
    }
}

/// The height of one list-box row and the width of its scrollbar, both constants in the original:
/// the accessor base class answers 14 to "how tall is row n" (0x4BC700) and the scrollbar is 16
/// wide (0x4DB030).
pub const LIST_ROW_H: f32 = 14.0;
pub const LIST_SCROLLBAR_W: f32 = 16.0;

/// `"%sshow.fsh"` — the pack `NFS5.ANIMSHOWCASEPICTURE` cycles, four pictures and four labels
/// (0x4BDBC5, §4.127).
pub const SHOWCASE_ANIM: &str = "show.fsh";

/// `CameraPicture.ctr`'s `shp` part — `optart.fsh`, indexed by `data.picturenum` (§4.129).
pub const CAMERA_PICTURES: &str = "optart.fsh";
/// `VerticalSlider.ctr`'s `[part.knob]` is `decrement_button.size.width,15`
pub const LIST_KNOB_H: f32 = 15.0;

/// A list's scrollbar, as the port hit-tests it — see [`LayoutScreen::list_scrollbar`].
#[derive(Clone, Copy, Debug)]
pub struct ListScrollbar {
    /// the decrement button, region 1 of 0x4D0CC0
    pub up: (f32, f32, f32, f32),
    /// the increment button, region 2
    pub down: (f32, f32, f32, f32),
    /// the knob, region 3
    pub knob: (f32, f32, f32, f32),
    /// where the knob's travel starts
    pub track_top: f32,
    /// how far it travels — track height less the knob's
    pub span: f32,
    /// how many first rows there are
    pub positions: usize,
    /// which one it is on
    pub at: usize,
}

/// An `NFS5.ListBox` widget and the accessor whose rows fill it.
#[derive(Debug, Clone, PartialEq)]
pub struct ListBoxWidget {
    pub index: usize,
    pub accessor: Option<String>,
}

/// One piece of a column-laid-out list row: a string whose x comes from **another widget of the
/// same screen**, found by name, plus the accessor's own offset from it. `Track.Records` draws its
/// four columns that way (0x4FB2A0, anchors `ID_RECORDPLAYERPOS` −10 and +4, `ID_RECORDCARPOS` +4,
/// `ID_RECORDTIMEPOS` +4).
#[derive(Debug, Clone, PartialEq)]
pub struct ListCell {
    /// the `name` of the widget whose x this column sits at
    pub anchor: String,
    /// the offset from the anchor. Its **sign** is the alignment: a negative offset — `-0.0`
    /// included — right-aligns the text there (align 2), any other left-aligns it, so `-0.0` is
    /// "right-aligned at the anchor itself" (the parts basket's `ID_PRICEPOS`, 0x516051)
    pub dx: f32,
    pub text: String,
    /// the level 0x4D87F0 scales the row colour's alpha by — `a · level / 255`, floored (the
    /// `0x80808081` divide by 255 at 0x4D8812…) — 0xFF for the colour as it is (0x4D87F4 returns
    /// it untouched)
    pub fade: u8,
    /// An explicit alignment (0 left, 1 centre, 2 right) where the offset's sign is not what the
    /// row draw passes — `Comms.PlayerList`'s row number is drawn with align 1 at `x − 4`
    /// (0x4BBF7B…0x4BBF91).
    pub align: Option<u8>,
}

/// The height of a parts list's category heading (0x518EA0) and of one of its rows (0x515FD4).
pub const PART_HEADER_H: f32 = 13.0;
pub const PART_ROW_H: f32 = 13.0;

/// A list-box row. The base accessor draws the whole row as one string (0x4BC350); the accessors
/// that override the row draw lay it out in columns instead, and the parts lists put a category
/// heading above the first row of each group.
#[derive(Debug, Clone, PartialEq)]
pub enum ListRow {
    Text(String),
    Columns(Vec<ListCell>),
    /// A parts list's category heading: a black rule above and below and the name centred between
    /// them (`drawPartsHeading` 0x518DE0), 13 px tall. The `bool` is whether the section this
    /// heading opens is expanded — meaningless here (category headings are not collapsible), but
    /// `Parts.AllParts`'s **own** two section headers reuse this variant and do read it, to pick
    /// the `+`/`−` icon (`LayoutScreen::parts_glyphs`'s own doc, ctor 0x515160 / row draw
    /// 0x515620).
    Header(String, bool),
    /// The garage list's own grouping heading — two rules **13 px** apart with the name between
    /// them, drawn by `FEOwnedCarAccessor`'s row painter 0x4F5D54…0x4F5DB3 and costing **14** px
    /// of row height. Its height comes out of slot 26 (0x4F60F0): a row whose era differs from
    /// the one above it is **42** — two headings and the row — one whose class differs is **28**,
    /// and any other row is **14**. `RE/notes/controls.md` §4.144.
    GroupHeading(String),
    /// A colour list's colour row — `FEColourAccessor`'s row draw **0x4B9390** (slot 15 of vtable
    /// 0x5B4E60): over the row's bar, a **20-wide swatch** at `(x + 3, y + 2)`, `h − 4` tall
    /// (`0x4D9280(x + inset, y + 2, 20.0, h − 4, 0x4D87F0(c, alpha))`, the inset the combo's
    /// 3.0 at 0x4C688F), outlined in black at the same alpha (0x4D9200 of 0x4D87E0). No text.
    /// The colour is the accessor's `[this+0x28](row)` — the byte-swapped dword of the car
    /// record's entry, `b0` as alpha — or, past the custom row, a saved custom colour.
    Swatch([u8; 4]),
    /// The row **at** the colour count — `FEStrings` **0x263** (`custom...`) at `(x + 3, y + 2)`,
    /// align 0, in **black** whatever the row's own text colour (0x4B945C…0x4B949F).
    CustomColour(String),
    /// `Comms.PlayerList`'s row (`FEPlayerListAccessor` 0x4BBEB0): columns like [`ListRow::Columns`]
    /// plus the player's car colour as a 15 × 10 swatch with a black outline at `(anchor x + dx,
    /// row y)` — `0x4D9280(x + 4, y, 15, 10, 0x4D87F0(colour, alpha))` then the outline
    /// `0x4D9200` of `0x4D87E0(alpha)` (0x4BC0C8…0x4BC116). The swatch is `(anchor, dx, rgba)`.
    Racer(Vec<ListCell>, Option<(String, f32, [u8; 4])>),
}

impl ListRow {
    /// How tall this row is. The base accessor answers 14 for every row (0x4BC700); a parts list's
    /// rows are 13 and its headings another 13 (0x516080).
    pub fn height(&self) -> f32 {
        match self {
            ListRow::Text(_) => LIST_ROW_H,
            ListRow::Columns(_) | ListRow::Racer(..) => LIST_ROW_H,
            ListRow::Header(..) => PART_HEADER_H,
            ListRow::GroupHeading(_) => LIST_ROW_H,
            // `FEColourAccessor` keeps the base slot 26, 0x4BC700
            ListRow::Swatch(_) | ListRow::CustomColour(_) => LIST_ROW_H,
        }
    }
}

impl From<String> for ListRow {
    fn from(s: String) -> Self {
        ListRow::Text(s)
    }
}

/// An `NFS5.3DCAR` widget: which car it shows and where it puts it. `positiontype` 0 means the
/// widget has no car of its own and the screen's 3D room owns it (0x4BFEEC); 1, 2 and 3 place a car
/// of their own at a fixed spot in front of the camera (`RE/notes/frontend-3d.md` §4).
#[derive(Debug, Clone, PartialEq)]
pub struct Car3dWidget {
    pub index: usize,
    pub accessor: Option<String>,
    pub othersource: Option<String>,
    pub position_type: i64,
    /// the widget's `damage` key: draw the car with its damage state
    pub damage: bool,
    /// the widget's transition alpha, 0…255 — `FeWidget::transitionAlpha` 0x4C8840, which the
    /// widget's slot 25 0x4C0A90 lights its own car by
    pub alpha: i32,
}

impl Car3dWidget {
    /// Where `positiontype` puts the car, camera-relative (0x4BFF28…0x4BFF75). `positiontype` 0
    /// has none of its own.
    pub fn position(&self) -> Option<[f32; 3]> {
        match self.position_type {
            1 => Some([2.4, 0.2, 8.1]),
            2 => Some([-2.4, 0.2, 8.1]),
            3 => Some([2.4, -1.2, 6.2]),
            _ => None,
        }
    }
}

/// An `NFS5.CarLogo` widget and the car source it names. The original resolves the car in this
/// order (0x4C14E0, CONFIRMED): `othersource` `FactoryDriver` → the `FactoryDriver.Car` accessor,
/// `EndOfEra` → `PrizeCars.Car`, `FactoryDriverPrize` → the factory-drive prize car (0x4F4360),
/// then the widget's own `accessor`, then `CarCompare1` → the compare screen's left car (0x50E6B0).
/// `othersource=Chron` is handled before all of that (0x4C1660) and takes its pack and entry from
/// the chronicle record instead of from `nfs5.car`.
#[derive(Debug, Clone, PartialEq)]
pub struct CarLogoWidget {
    pub index: usize,
    pub accessor: Option<String>,
    pub othersource: Option<String>,
}

/// The front end's default font, loaded at FE start-up (0x4C72B1).
pub const DEFAULT_FONT: &str = "univers8b.ffn";

/// The font `FeComboBox` keeps at `[this+0x184]` and hands both its strings when
/// `data.usesmallfont` is set (0x4C4E8E, 0x4C555E) — the same file as [`DEFAULT_FONT`], named
/// separately because the two are the same only by coincidence.
pub const SMALL_FONT: &str = "univers8b.ffn";
/// `NFS5.ColorBox`'s picker keeps two fonts of its own — `[this+0x294]` and `[this+0x298]`
pub const PICKER_BUTTON_FONT: &str = "universc11b.ffn";
/// The dialogs' two fonts, loaded once for all of them at 0x5121EB / 0x512227 into `[0x69A060]`
/// (the text) and `[0x69A064]` (the buttons) — §4.147.
pub const DIALOG_TEXT_FONT: &str = "univers10b.ffn";
pub const DIALOG_BUTTON_FONT: &str = "universc11m.ffn";
pub const PICKER_TITLE_FONT: &str = "univers7b.ffn";

/// The screen block's `screen=` id → the layout drawn underneath it (`FE_loadBackground` 0x4E1470,
/// CONFIRMED). The original has a second, reduced table — `ID_FULLOVAL` → `screenfulloval` and
/// everything else → `screenveryempty` — behind a memory/option gate (`[0x5D9D38]`, set at 0x4E3584
/// only when the device's texture memory `[0x619790]` is above 4 000 000 **and** the `FELOWENDCARD`
/// option `[0x657E5C]` is **clear**), and swaps `screenoptionoval` for `screenoptionovallite` on a
/// screen with `canplaygame=1` when `FENOUSELESSART` `[0x657E60]` is set or the machine has under
/// 0x2100000 bytes (33 MiB) of **total physical** memory (0x4E35D0 → 0x53C3D0, `GlobalMemoryStatus`'s
/// `dwTotalPhys`). A port with memory to spare always takes the full table below.
pub fn background_layout(screen_id: &str) -> Option<&'static str> {
    Some(match screen_id {
        "ID_SCREEN" => "screenempty",
        "ID_DARKOVAL" => "screendarkoval",
        "ID_FULLOVAL" => "screenfulloval",
        "ID_MOVIEOVAL" => "screenmovieoval",
        "ID_OPTIONOVAL" => "screenoptionoval",
        _ => return None,
    })
}

/// An `0xAARRGGBB` string from a `.ctr` template as `[r, g, b, a]`.
fn parse_color(v: &str) -> Option<[u8; 4]> {
    let v = v.trim();
    let n = u64::from_str_radix(v.strip_prefix("0x").or_else(|| v.strip_prefix("0X"))?, 16).ok()?;
    Some([((n >> 16) & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, (n & 0xFF) as u8, ((n >> 24) & 0xFF) as u8])
}

/// A front-end colour, as the shader wants it: the byte over 255.
///
/// The original writes these bytes into an 8-bit frame buffer with no colour management at all:
/// a `NFS5.GOTOBUTTON`'s `0xFFBC0000` is red **188** on the player's screen. The renderer is a
/// byte pipeline now (no sRGB surface or textures, `RE/RENDERING.md` "Colour space"), so the byte
/// goes through as it is. (Until 2026-09-24 this undid the sRGB encoding of the surface, which
/// fixed flat colours but left blends and gradients in linear light.)
fn tint(c: [u8; 4]) -> [f32; 4] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]
}

/// An `0xAARRGGBB` colour word as [`tint`] takes it.
fn argb(c: u32) -> [f32; 4] {
    tint([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8])
}

/// `NFS5.MOVIECIRCLE`'s flash frames and its two run fonts (0x4CDE99, 0x4CDED3).
const MOVIE_FLASHES: &str = "circles.fsh";
const RUN_FONTS: [&str; 2] = ["univers7b.ffn", "univers11b.ffn"];
/// The credits roll's body `[+0x21C]` and heading `[+0x220]` fonts.
const ROLL_FONTS: [&str; 2] = ["univers11b.ffn", "univers8b.ffn"];

/// The tag the chronicle's picture tiles carry, which the control's own name repeats at 0x5DBBFE.
const CHRONICLE_PICTURE_TAG: &str = "1111";
/// The shape-cache entry a chronicle picture's composed mosaic lives under. Entry 0 of the same
/// pack is the thumbnail's `bi` icon (`set_pack_picture` keeps it under `(file, 0)`): sharing the
/// key made a picture opened after its own thumbnail draw the 64×36 icon instead of the mosaic
/// (found when the thumbnails could first select a file, 0x5089C0).
const CHRONICLE_MOSAIC_ENTRY: usize = usize::MAX;

/// The two fonts `NFS5.BADGE` loads for itself (0x4CF25C, 0x4CF2A0).
const BADGE_NAME_FONT: &str = "univers14b.ffn";
const BADGE_RANK_FONT: &str = "univers10b.ffn";
/// The drop shadow `NFS5.BADGE` lays over the licence card (0x4CF208).
const LICENSE_SHADOW: &str = "LicenseShadow.fsh";

/// The front end draws its own pointer, out of `FEData/Art/Pointer.fsh` — `controls.md` §4.152.
const POINTER: &str = "Pointer.fsh";

/// The ten shapes, as `{firstFrame, lastFrame}` over `Pointer.fsh`'s 34 entries. This is the table
/// at **0x5D9C8C**, read out pair by pair; every pair lands exactly on a tag boundary in the file
/// (`00`…`05`, `watc`, `mags`, `magP`, `pick`, `Lift`×6, `Pan.`×6, `Poin`×6, `grab`×6).
pub const CURSOR_SHAPES: [(usize, usize); 10] =
    [(0, 0), (1, 5), (9, 9), (10, 15), (16, 21), (28, 33), (22, 27), (7, 7), (8, 8), (6, 6)];

/// How the original steps a shape's frames — **0x4E1C90, read 2026-09-21**, which retires the
/// `CURSOR_FRAME_MS` approximation that used to stand here. The clock is `TIMER_getTicks()`
/// (0x53C2E0 → `[0x6B7C7C]`, **128 Hz**; `RE/notes/timing.md`), and there are two rules:
///
/// * **shape 1**, the hovered arrow (0x4E1CFA): `frame = last − ((t − t_shape) / 16 mod
///   (last − first))`. Sixteen ticks is **125 ms**; the count runs **backwards** from `last`; and
///   the span is `last − first`, not `last − first + 1`, so **`first` is never drawn** — shape 1
///   shows 5, 4, 3, 2, 5, 4, … and never 1. That also makes the `jl` at 0x4E1D31, which would
///   clamp to `first`, unreachable.
/// * **every other shape** (0x4E1D3E), gated on 0x5299C0 = `[0x69CAE8]`, which 0x5297E3 sets from
///   whether a mouse button changed to down: while it is **down** the frame ramps *up* from
///   `first` and clamps at `last`; while it is **up** it ramps *down* from `last` and clamps at
///   `first`. Four ticks is **31.25 ms**. Each arm restamps the *other* one's timer with `now`
///   (0x4E1D68, 0x4E1DB3), so the ramp always measures from the moment the button last changed —
///   which is why `grab` closes as it is pressed and opens as it is let go.
///
/// Both divisors are exact in milliseconds: `16 ticks = ms / 125` and `4 ticks = ms × 32 / 1000`,
/// since flooring the tick count before dividing by an integer is the same as dividing the real
/// tick count.
const CURSOR_SLOW_MS: u32 = 125;

/// Whether one row of a combo scrollbar's sliver falls inside 0x4D94A0's clip box — the trough's
/// **interior**, between the two arrows (0x4DB633…0x4DB653; see `combo_scrollbar`). The arrows are
/// `COMBO_ARROW_H` tall at either end of the bar, so the box is `y + h_arrow … y + h − h_arrow`,
/// and a sliver beside a knob at either extreme is clipped away rather than drawn over an arrow.
pub fn combo_sliver_visible(sy: f32, y: f32, h: f32) -> bool {
    sy >= y + COMBO_ARROW_H && sy < y + h - COMBO_ARROW_H
}

/// The height of each of a combo scrollbar's two arrows, from `weeicons.fsh` (§4.90).
pub const COMBO_ARROW_H: f32 = 13.0;

/// The offset applied to **both** of the pointer's coordinates before the cursor is blitted —
/// 0x4E1DF1…0x4E1E42. Only the requested shape **2** is moved, by `[0x5B2460]` = 6.0; there is no
/// per-shape table in the original, and the `jne` arm passes the coordinates through untouched.
pub fn cursor_hot_spot(shape: usize) -> f32 {
    if shape == 2 { -6.0 } else { 0.0 }
}

/// The frame of a cursor shape, 0x4E1C90's two rules — see [`CURSOR_SLOW_MS`] for where each
/// number comes from. `since_shape` and `since_btn` are milliseconds since the shape and the
/// button state last changed, which is what the original's two mutually-restamped timers measure.
pub fn cursor_frame(shape: usize, (first, last): (usize, usize), since_shape: u32, since_btn: u32, btn: bool) -> usize {
    if shape == 1 {
        // 0x4E1CFA: backwards from `last`, modulo `last − first`, so `first` never shows
        let span = last - first;
        if span == 0 { first } else { last - (since_shape / CURSOR_SLOW_MS) as usize % span }
    } else {
        // 0x4E1D3E: up while the button is down, down while it is up, clamped at both ends
        let d = (since_btn as u64 * 32 / 1000) as usize;
        if btn { (first + d).min(last) } else { last.saturating_sub(d).max(first) }
    }
}

/// Classes whose content comes from the executable and which the port does not draw. `NFS5.3DCAR`
/// is not one of them any more: the car it shows belongs to the screen's 3D room
/// (`RE/notes/frontend-3d.md`), which the port renders behind the screen.
fn code_placed(_class: &str) -> bool {
    false
}

/// The source a widget names, for the blank-widget report.
fn source_of(e: &nfs_formats::lay::Element) -> String {
    for k in ["accessor", "othersource", "listaccessor"] {
        if let Some(v) = e.get(k).filter(|v| !v.is_empty()) {
            return format!("{k}={v}");
        }
    }
    e.string_id().map(|id| format!("stringID={id}")).unwrap_or_else(|| "(none)".to_string())
}

impl LayoutScreen {
    /// Load `FEData/Layouts/<name>.lay`, the background screen it names, and every `.fsh` entry
    /// they reference.
    pub fn load(r: &mut Renderer, paths: &GamePaths, name: &str) -> anyhow::Result<Self> {
        let controls = Arc::new(load_controls(paths));
        let fe_strings = Arc::new(load_strings(paths));
        let mut screen = Self::load_one(r, paths, name, controls.clone(), fe_strings.clone())?;
        // `NFS_FE_BG=<layout>` puts a background under a **page** — one of the 28 layouts whose
        // `screen=` is empty because in the game it is drawn over whatever opened it. Without it
        // `--fe=<page>` shows the page over black, which is `RE/AUDIT.md` F2.
        // …and only for the screen being opened: `bottombar` is a page too, and the front end draws
        // it *after* the screen, so giving it a background put a second copy of that background
        // over everything. That artefact cost four passes of `RE/AUDIT.md` F2.
        let forced = std::env::var("NFS_FE_BG")
            .ok()
            .filter(|_| !name.eq_ignore_ascii_case("bottombar"))
            .filter(|_| screen.layout.screen().and_then(|s| s.get("screen")).is_none_or(str::is_empty));
        if let Some(bg) = forced.as_deref().or_else(|| screen.layout.screen().and_then(|s| s.get("screen")).and_then(background_layout)) {
            match Self::load_one(r, paths, bg, controls, fe_strings) {
                Ok(mut b) => {
                    // a background screen never draws the pointer — the screen in front of it does
                    b.cursor_ms = u32::MAX;
                    screen.background = Some(Box::new(b));
                }
                Err(e) => tracing::debug!(background = bg, error = %e, "background screen unavailable"),
            }
        }
        Self::load_children(r, paths, &mut screen, 0);
        Ok(screen)
    }

    /// The layouts a screen's widgets embed, loaded into [`groups`](Self::groups).
    ///
    /// Two classes carry a `filename`. `NFS5.RADIOGROUP` has nothing else — `Radio Group.Ctr`
    /// gives it that one attribute and no drawable part — and `NFS5.PANEL` is the container the
    /// tabbed screens are built from; `Panel.ctr`'s own comment says so ("A lot of the work for
    /// this is done through code. It will load up and display the lay file given").
    ///
    /// The original's panel: **0x4CEA40** reads `data.filename` into `[panel+0x148]`; **0x4CEA90**
    /// calls the layout loader 0x4E1200 with it, keeps the screen in `[panel+0x14C]`, parents it
    /// (`[loaded+0xD0] = panel`) and forces its `moveable` attribute to 1; **0x4CEBC0**, the
    /// per-frame tick, does *nothing at all* when `[panel+0x14C]` is null and otherwise runs the
    /// loaded screen's own tick and carries the parent's transition into it; **0x4CECE0** keeps it
    /// at the panel's position — it moves the panel by the delta (0x525310 / 0x4C8760) and then
    /// hands the panel's new position to the loaded screen (0x525330 and its vtable +0x3C).
    ///
    /// That last one is why the sub-layout is drawn **offset by the panel's `pos`**, and the files
    /// agree: `usedcarparts.lay` is a 448×183 screen whose widgets run 0..444 in x and 0..179 in y,
    /// and `usedcars.lay` §19 places it at (96, 182) with `size=440+8,175+8` — the same box.
    ///
    /// Two levels deep in the shipped data: `Options.lay` embeds `controllers`, which embeds
    /// `controllertabs` and six more. The depth cap is this port's own guard, not a rule of the
    /// original.
    /// Open or close the `NFS5.PANEL` named `name`, here or in any panel this screen holds —
    /// a panel's slot 27 load (0x4CEA90, `[panel+0x14C]` from the layout loader) the first time it
    /// opens, then 0x4CEC60 (show) or 0x4CEC50 (hide). Answers whether the panel was found.
    pub fn set_panel(&mut self, r: &mut Renderer, paths: &GamePaths, name: &str, open: bool) -> bool {
        let found = self.layout.elements.iter().position(|e| e.class == "NFS5.PANEL" && e.get("name").is_some_and(|n| n.eq_ignore_ascii_case(name)));
        if let Some(i) = found {
            if !open {
                self.closed.insert(i);
                return true;
            }
            self.closed.remove(&i);
            if !self.groups.contains_key(&i)
                && let Some(file) = self.layout.elements[i].get("filename").map(str::to_string)
            {
                match Self::load_one(r, paths, &file, self.controls.clone(), self.fe_strings.clone()) {
                    Ok(mut g) => {
                        g.set_overlay(true);
                        Self::load_children(r, paths, &mut g, 1);
                        self.groups.insert(i, Box::new(g));
                    }
                    Err(e) => {
                        tracing::debug!(file, error = %e, "panel layout unavailable");
                        self.closed.insert(i);
                    }
                }
            }
            return true;
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        keys.into_iter().any(|k| self.groups.get_mut(&k).is_some_and(|g| g.set_panel(r, paths, name, open)))
    }

    fn load_children(r: &mut Renderer, paths: &GamePaths, screen: &mut Self, depth: usize) {
        if depth >= 4 {
            return;
        }
        let embedded = |class: &str| class == "NFS5.RADIOGROUP" || class == "NFS5.PANEL";
        let want: Vec<(usize, String, String)> = screen
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| embedded(&e.class))
            .filter_map(|(i, e)| e.get("filename").filter(|f| !f.is_empty()).map(|f| (i, e.class.clone(), f.to_string())))
            .collect();

        // the tab strips first: a strip's buttons are what say which panel of *this* screen is open
        for (i, class, file) in want.iter().filter(|(_, c, _)| c == "NFS5.RADIOGROUP") {
            let _ = class;
            match Self::load_one(r, paths, file, screen.controls.clone(), screen.fe_strings.clone()) {
                Ok(mut g) => {
                    g.set_overlay(true);
                    Self::load_children(r, paths, &mut g, depth + 1);
                    screen.groups.insert(*i, Box::new(g));
                }
                Err(e) => tracing::debug!(file, error = %e, "tab strip unavailable"),
            }
        }

        // every `panel=` a tab button carries, here or in a strip this screen embeds, and whether
        // that button is the selected one
        let mut claimed: HashMap<String, bool> = HashMap::new();
        let mut note = |es: &[nfs_formats::lay::Element]| {
            for e in es {
                if let Some(name) = e.get("panel").filter(|n| !n.is_empty()) {
                    let sel = e.get("state") == Some("selected");
                    let slot = claimed.entry(name.to_ascii_uppercase()).or_insert(false);
                    *slot |= sel;
                }
            }
        };
        note(&screen.layout.elements);
        for g in screen.groups.values() {
            note(&g.layout.elements);
        }

        for (i, class, file) in want.iter().filter(|(_, c, _)| c == "NFS5.PANEL") {
            let _ = class;
            let name = screen.layout.elements[*i].get("name").unwrap_or("").to_ascii_uppercase();
            // a panel a tab button owns is up only while that button is the selected one
            // (0x4D02E0: selecting a tab calls its panel's load, vtable +0x6C = 0x4CEA90, and then
            // shows it, 0x4CEC60). Exactly one button per shipped strip carries `state=selected`.
            if claimed.get(&name) == Some(&false) {
                screen.closed.insert(*i);
                continue;
            }
            // and the one panel the *screen* hides on entry: `Audio.lay`'s CD player, hidden at
            // 0x511A85 (`0x4CEC50` on the `ID_PLAYLISTPANEL` it looks up at 0x511A45). Its own
            // `audioreal.lay` carries the `SHOWPLAYLISTPANEL` callback that raises it.
            if name == "ID_PLAYLISTPANEL" {
                screen.closed.insert(*i);
                continue;
            }
            match Self::load_one(r, paths, file, screen.controls.clone(), screen.fe_strings.clone()) {
                Ok(mut g) => {
                    g.set_overlay(true);
                    Self::load_children(r, paths, &mut g, depth + 1);
                    screen.groups.insert(*i, Box::new(g));
                }
                // A panel whose file will not load draws **nothing, correctly**: 0x4CEA90 hands
                // the name to the loader 0x4E1200 and keeps what comes back in `[panel+0x14C]`,
                // and 0x4CEBC0 returns at once when that is null. `Controllers.lay` §6 is the
                // shipped case — `filename=predefkeys` and no `predefkeys.lay` in the install,
                // and no tab button names that panel either, so nothing ever selects it.
                Err(e) => {
                    tracing::debug!(file, error = %e, "panel layout unavailable");
                    screen.closed.insert(*i);
                }
            }
        }
        // a group is loaded after its parent read `NFS_FE_CURSOR`, and it read the same absolute
        // pair for itself — push the parent's pointer down again, now that the offsets are known
        let at = screen.cursor;
        screen.place_cursor(at);
    }

    fn load_one(r: &mut Renderer, paths: &GamePaths, name: &str, controls: Arc<ControlLibrary>, fe_strings: Arc<Strings>) -> anyhow::Result<Self> {
        let dir = paths.fe_data().join("Layouts");
        // 0x4E1281…0x4E12EF: the loader searches the name for `.lay` and appends it only when it
        // is not already there. Both spellings ship — `filename=audio` and `filename=gearratio.lay`.
        let file = if name.to_ascii_lowercase().contains(".lay") { name.to_string() } else { format!("{name}.lay") };
        let path = GamePaths::find_file(&dir, &file).ok_or_else(|| anyhow::anyhow!("no layout {file} in {}", dir.display()))?;
        let bytes = std::fs::read(&path)?;
        let text: String = bytes.iter().map(|&b| nfs_formats::locale::cp1252(b)).collect();
        let layout = Layout::parse(&text);

        let white = r.upload_texture(&TextureData::single(1, 1, vec![255, 255, 255, 255]));
        let art = paths.fe_data().join("Art");
        let mut shapes: HashMap<(String, usize), (TextureId, f32, f32)> = HashMap::new();
        let mut packs: HashMap<String, Vec<nfs_assets::texture::TextureImage>> = HashMap::new();
        // the widget's own `shapePathName`/`shapeIndex`, and every `SHAPE` part its class template
        // draws — the second is where `NFS5.CARSETTINGBUTTON` and its like keep all of their art
        let mut wanted: Vec<(String, usize)> = Vec::new();
        for e in &layout.elements {
            // `NFS5.AnimatingPicture` cycles a whole run of frames rather than drawing one
            // (§4.53), so its pack has to give up all of them
            if e.class == "NFS5.AnimatingPicture"
                && let Some(file) = e.get("shapePathName").filter(|f| !f.is_empty())
            {
                let a = e.int("start_frame").unwrap_or(0).max(0) as usize;
                let b = e.int("end_frame").unwrap_or(0).max(0) as usize;
                for n in a.min(b)..=a.max(b) {
                    wanted.push((file.to_string(), n));
                }
            }
            // and a bottom-bar button has three runs of its own (§4.53)
            if e.class == "NFS5.BOTTOMBARBUTTON"
                && let Some(file) = e.get("shapePathName").filter(|f| !f.is_empty())
            {
                for (s, t) in [("startframe", "endframe"), ("hoverstartframe", "hoverendframe"), ("hovermousedownstartframe", "hovermousedownendframe")] {
                    let a = e.int(s).unwrap_or(0).max(0) as usize;
                    let b = e.int(t).unwrap_or(0).max(0) as usize;
                    for n in a.min(b)..=a.max(b) {
                        wanted.push((file.to_string(), n));
                    }
                }
            }
            if let Some((file, index)) = e.shape() {
                wanted.push((file.to_string(), index));
            }
            // a `NFS5.CheckBox` keeps both of its entries: the template's part names only the
            // ticked one (`data.onIndex`) and the code swaps in `data.offIndex` (0x4C3730)
            if e.class == "NFS5.CheckBox"
                && let Some(file) = e.get("shapePathName").filter(|v| !v.is_empty())
            {
                for key in ["onIndex", "offIndex"] {
                    if let Some(n) = e.int(key).filter(|n| *n >= 0) {
                        wanted.push((file.to_string(), n as usize));
                    }
                }
            }
            let Some(c) = controls.get(&e.class) else { continue };
            // **every** part of the class, not only the ones its current state draws: a
            // template's geometry can measure a part that is not on screen —
            // `Graphic Tab Button.ctr` centres its caption over `selectedbg.size.width`, which
            // only `[state.selected]` lists, so without the art of every state the four unopened
            // tabs could not place their own captions
            for p in c.parts.values() {
                if p.kind() != nfs_formats::ctr::PartKind::Shape {
                    continue;
                }
                // the same resolution `shape_parts` applies: the part attribute's screen key
                // against the `.lay` block, else the template's own value
                let attr = |key: &str| -> Option<&str> {
                    if let Some(k) = p.screen_key(key)
                        && let Some(v) = e.get(&k).filter(|v| !v.is_empty())
                    {
                        return Some(v);
                    }
                    p.value(key)
                };
                // the same `%<part>.<key>%` indirection the draw resolves: without it the
                // loader asked the art packs for a file literally called
                // `"%picnonsel.shapePathName%"` and quietly got nothing
                let file = match attr("shapePathName").filter(|v| !v.is_empty()) {
                    Some(v) => match v.strip_prefix('%').and_then(|v| v.strip_suffix('%')) {
                        Some(inner) => match inner.split_once('.') {
                            Some(("data", k)) => match e.get(k).filter(|v| !v.is_empty()) {
                                Some(v) => v,
                                None => continue,
                            },
                            Some((part, k)) => match c.part(part).and_then(|q| q.value(k)).filter(|v| !v.is_empty()) {
                                Some(v) => v,
                                None => continue,
                            },
                            None => continue,
                        },
                        None => v,
                    },
                    None => continue,
                };
                let index = match attr("shapeIndex") {
                    Some(v) => match v.trim().strip_prefix("data.") {
                        Some(k) => match e.int(k) {
                            Some(n) if n >= 0 => n as usize,
                            _ => continue,
                        },
                        None => match v.trim().parse::<i64>() {
                            Ok(n) if n >= 0 => n as usize,
                            _ => continue,
                        },
                    },
                    None => 0,
                };
                wanted.push((file.to_string(), index));
            }
        }
        // A `NFS5.HEADER` sweeps a glint across the screen (§4.25). Its activate builds
        // `"%sweeicons.fsh"` from the art directory and looks the entry up **by its tag**,
        // `"GLNT"` (0x4CADC0), so the port does the same rather than carry the index.
        // The bottom bar's buttons (0x4C95E4…0x4C95FE) and the word-wrap text object (0x5230E1…
        // 0x523105, the typewriter's leading letter) look up the same entry.
        let glint = layout.elements.iter().any(|e| matches!(e.class.as_str(), "NFS5.HEADER" | "NFS5.CarLogo" | "NFS5.BOTTOMBARBUTTON") || e.class.to_ascii_lowercase().contains("wordwrap")).then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let n = pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case("GLNT"))?;
            wanted.push(("weeicons.fsh".to_string(), n));
            Some(n)
        }).flatten();
        // The combo's drop-down grows a scrollbar of its own, and it is **not** the list box's
        // `NFS5.VerticalSlider`: 0x4DADB0 builds a class of its own (vtable 0x5B6A64) and looks
        // `WeeIcons.fsh` up **by tag** for six arrows — `ARup`/`ARov`/`ARin` into
        // `[this+0x13C…0x144]` and `A2up`/`A2ov`/`A2in` into `[+0x148…0x150]` (0x4DAE22…0x4DAE91).
        // This port draws the resting pair. §4.90.
        let combo_arrows = layout
            .elements
            .iter()
            .any(|e| matches!(e.class.as_str(), "NFS5.ComboBoxSlider" | "NFS5.ColorBox"))
            .then(|| {
                let p = GamePaths::find_file(&art, "weeicons.fsh")?;
                let d = std::fs::read(p).ok()?;
                let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
                let at = |tag: &str| pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case(tag));
                let six = [at("ARup")?, at("ARov")?, at("ARin")?, at("A2up")?, at("A2ov")?, at("A2in")?];
                for n in six {
                    wanted.push(("weeicons.fsh".to_string(), n));
                }
                Some(six)
            })
            .flatten();
        // `NFS5.ColorBox`'s picker draws one sprite, the `weeicons.fsh` entry tagged **`COLB`**
        // that its factory looks up at 0x4C390C — the same by-tag lookup as the glint's
        // (`RE/notes/controls.md` §4.47, §4.48)
        let colb = layout.elements.iter().any(|e| e.class == "NFS5.ColorBox").then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let n = pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case("COLB"))?;
            wanted.push(("weeicons.fsh".to_string(), n));
            Some(n)
        }).flatten();
        // `Parts.AllParts`'s two section headers (`КУПИТЬ ЗАПЧАСТИ ДЛЯ …`/`ПРОДАТЬ ЗАПЧАСТИ`) draw
        // a `+`/`−` collapse icon from the same `weeicons.fsh` the ctor **0x515160** caches by
        // index (`pls1`=3, `min4`=0xB, `pls2`=4, `pls3`=5 — see the field doc above)
        let parts_glyphs = layout.elements.iter().any(|e| e.get("accessor").is_some_and(|a| a.starts_with("Parts.AllParts"))).then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let at = |tag: &str| pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case(tag));
            let four = [at("pls1")?, at("min4")?, at("pls2")?, at("pls3")?];
            for n in four {
                wanted.push(("weeicons.fsh".to_string(), n));
            }
            Some(four)
        }).flatten();
        let arcs = layout.elements.iter().any(|e| e.class == "NFS5.CONTROLLERSETUP").then(|| {
            let p = GamePaths::find_file(&art, "weeicons.fsh")?;
            let d = std::fs::read(p).ok()?;
            let pack = nfs_formats::fsh::Fsh::parse(&d).ok()?;
            let mut arcs = [0; 8];
            for (k, a) in arcs.iter_mut().enumerate() {
                let tag = format!("arc{k}");
                *a = pack.entries.iter().position(|e| e.name.trim_end_matches('\0').eq_ignore_ascii_case(&tag))?;
                wanted.push(("weeicons.fsh".to_string(), *a));
            }
            Some(arcs)
        }).flatten();
        // A `NFS5.ListBox` that overflows grows a scrollbar of its own — a `NFS5.VerticalSlider`,
        // which is no widget of any layout, so its art has to be asked for here
        if layout.elements.iter().any(|e| e.class == "NFS5.ListBox")
            && let Some(sb) = controls.get("NFS5.VerticalSlider")
        {
            for p in sb.state_parts("normal").iter().filter_map(|n| sb.part(n)) {
                if p.kind() != nfs_formats::ctr::PartKind::Shape {
                    continue;
                }
                let (Some(f), Some(n)) = (p.value("shapePathName"), p.value("shapeIndex").and_then(|v| v.trim().parse::<usize>().ok())) else { continue };
                wanted.push((f.to_string(), n));
            }
        }
        // The pointer belongs to no widget — the front end draws it itself out of
        // `FEData/Art/Pointer.fsh` every frame (§4.152), so its 34 entries have to be asked for
        // here or nothing would ever load them.
        // `NFS5.MOVIECIRCLE`'s flashes are the five entries of `circles.fsh` its constructor loads
        // (0x52069C…0x5206D5)
        if layout.elements.iter().any(|e| e.class == "NFS5.MOVIECIRCLE") {
            for i in 0..5 {
                wanted.push((MOVIE_FLASHES.to_string(), i));
            }
        }
        for i in 0..34 {
            wanted.push((POINTER.to_string(), i));
        }
        for (file, index) in wanted {
            let file = file.as_str();
            let key = (file.to_string(), index);
            if shapes.contains_key(&key) {
                continue;
            }
            let entry = packs.entry(file.to_string()).or_insert_with(|| {
                GamePaths::find_file(&art, file)
                    .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                    .and_then(|(p, d)| decode_pack(&p, &d).ok())
                    .unwrap_or_default()
            });
            let Some(img) = entry.get(index) else {
                tracing::debug!(file, index, "layout art entry missing");
                continue;
            };
            let Some(level0) = img.mips.first() else { continue };
            let id = r.upload_texture(&TextureData::single(img.width, img.height, level0.clone()));
            shapes.insert(key, (id, img.width as f32, img.height as f32));
        }

        // every `fontPathName` the file names, every one its widgets' class templates name, plus
        // the FE default
        let mut fonts: HashMap<String, BitmapFont> = HashMap::new();
        // the FE default, and the dialogs' two fonts: a dialog can come up over any screen
        let mut want: Vec<String> = vec![DEFAULT_FONT.to_string(), DIALOG_TEXT_FONT.to_string(), DIALOG_BUTTON_FONT.to_string()];
        // `NFS5.BADGE` builds its own two fonts rather than taking one from the file: 0x4CF25C
        // loads `Univers14b.ffn` for the player's name and 0x4CF2A0 `Univers10b.ffn` for the rank
        // and `NFS5.MOVIECIRCLE`'s overlay two for its runs, `Univers7b.ffn` and `Univers11b.ffn`
        // (0x4CDE99, 0x4CDED3)
        // (`NFS5.ANIMSHOWCASEPICTURE`'s runs load the same pair, 0x4BDD48 / 0x4BDD5E)
        if layout.elements.iter().any(|e| e.class == "NFS5.ANIMSHOWCASEPICTURE") {
            want.push(RUN_FONTS[0].to_string());
            want.push(RUN_FONTS[1].to_string());
        }
        if layout.elements.iter().any(|e| e.class == "NFS5.MOVIECIRCLE") {
            want.push(RUN_FONTS[0].to_string());
            want.push(RUN_FONTS[1].to_string());
            // …and the roll's two, `Univers11B` and `Univers8B` (0x4CDF01, 0x4CDF0E)
            want.push(ROLL_FONTS[0].to_string());
            want.push(ROLL_FONTS[1].to_string());
        }
        if layout.elements.iter().any(|e| e.class == "NFS5.BADGE") {
            want.push(BADGE_NAME_FONT.to_string());
            want.push(BADGE_RANK_FONT.to_string());
        }
        // The timeline's caption is smaller than its year header — `Univers7b.ffn` against the
        // year's own `Univers8b.ffn` — read off `FEChronicleMainBoxControl`'s bind, 0x507F10:
        // `[this+0x158]` ("Univers8b") and `[this+0x15c]` ("Univers7b") are two distinct fonts,
        // not one reused (§4.173). Nothing in the `.lay`/`.ctr` files names the caption's font, so
        // without this rule it is never loaded and the caption never draws.
        if layout.elements.iter().any(|e| e.class == "NFS5.ChronicleMainBoxControl") {
            want.push(PICKER_TITLE_FONT.to_string());
        }
        // and `NFS5.ColorBox`'s picker builds two of its own: 0x4C3A3A loads `Universc11b.ffn`
        // into `[this+0x294]` for the buttons and 0x4C3A76 `Univers7b.ffn` into `[+0x298]` for the
        // title (`RE/notes/controls.md` §4.48)
        if layout.elements.iter().any(|e| e.class == "NFS5.ColorBox") {
            want.push(PICKER_BUTTON_FONT.to_string());
            want.push(PICKER_TITLE_FONT.to_string());
        }
        for e in &layout.elements {
            if let Some(f) = e.get("fontPathName").filter(|f| !f.is_empty()) {
                want.push(f.to_lowercase());
            }
            if let Some(c) = controls.get(&e.class) {
                for part in c.parts.values() {
                    if let Some(f) = part.value("fontPathName").filter(|f| !f.is_empty()) {
                        want.push(f.to_lowercase());
                    }
                }
                if let Some(f) = c.default_of("fontpath").filter(|f| !f.is_empty()) {
                    want.push(f.to_lowercase());
                }
            }
        }
        want.sort();
        want.dedup();
        for name in want {
            let Some(path) = GamePaths::find_file(&art, &name) else {
                tracing::debug!(name, "layout font missing");
                continue;
            };
            match std::fs::read(&path).ok().and_then(|d| nfs_formats::fnt::Font::parse(&d).ok()) {
                Some(f) => {
                    fonts.insert(name, BitmapFont::upload(r, f));
                }
                None => tracing::debug!(name, "layout font unreadable"),
            }
        }

        let mut counts: HashMap<String, usize> = HashMap::new();
        for e in &layout.elements {
            if code_placed(&e.class) {
                *counts.entry(e.class.clone()).or_default() += 1;
            }
        }
        let mut unrendered: Vec<(String, usize)> = counts.into_iter().collect();
        unrendered.sort();
        tracing::info!(name, elements = layout.elements.len(), art = shapes.len(), fonts = fonts.len(), skipped = unrendered.len(), "layout loaded");
        let screen = Self { layout, background: None, shapes, fonts, white, cursor_ms: 0, cursor_shape_at: std::cell::Cell::new((usize::MAX, 0)), cursor_btn_at: std::cell::Cell::new((false, 0)), cursor_btn: false, car_spin: 0.0, car_parts: HashMap::new(), car_roof: Default::default(), covered: false, combo_hover: std::cell::Cell::new(None), overlay_mask: std::cell::Cell::new(None), unrendered, logos: HashMap::new(), checks: HashMap::new(), knobs: HashMap::new(), closed: std::collections::HashSet::new(), groups: HashMap::new(), bars: HashMap::new(), lists: HashMap::new(), swatches: HashMap::new(), car_views: HashMap::new(), track_maps: HashMap::new(), movies: HashMap::new(), ko_strip: HashMap::new(), badges: HashMap::new(),
            // `NFS_FE_CLOCK` preloads the screen clock in milliseconds, so a still can be taken
            // part way through an animation the front end would otherwise only show in motion
            clock_ms: std::env::var("NFS_FE_CLOCK").ok().and_then(|v| v.parse().ok()).unwrap_or(0), glint, colb, arcs, parts_glyphs,
            movie_overlays: Default::default(),
            credits_roll: Default::default(),
            wrap_watch: Default::default(),
            typing: Default::default(),
            pending_tab: None,
            inherited: std::cell::Cell::new((false, 0)),
            glints: Default::default(),
            preload_ticks: std::env::var("NFS_FE_CLOCK").ok().and_then(|v| v.parse::<u64>().ok()).map_or(0, |ms| (ms * 128 / 1000) as i64),
            watch_text: Default::default(),
            last_drawn_state: Default::default(),
            end_of_era: None,
            career_tabs: None,
            bar_oval: None,
            cheat_bits: 0,
            driver_picture: crate::save::DEFAULT_PICTURE as i32,
            // `NFS_FE_CURSOR=x,y` parks the pointer for a still, the way `NFS_FE_CLOCK` parks the
            // clock — a screenshot has no pointer of its own
            cursor: std::env::var("NFS_FE_CURSOR").ok().and_then(|v| {
                let (a, b) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
            }),
            pointer: None,
            // `NFS_FE_PRESS=1` holds the button down for a still, the pair of `NFS_FE_CURSOR`
            pressed: std::env::var("NFS_FE_PRESS").ok().is_some_and(|v| v != "0"),
            capture: None,
            // `NFS_FE_OPEN=<index>` drops a combo's list open for a still
            open_combo: std::env::var("NFS_FE_OPEN").ok().and_then(|v| v.parse().ok()),
            // `NFS_FE_PICKER=1` with `NFS_FE_OPEN` raises the latch for a still of the editor
            picker_latch: std::env::var("NFS_FE_PICKER").is_ok_and(|v| v != "0"),
            picker: Picker::default(),
            pads: Vec::new(),
            pad_sel: 0,
            deadzone: Vec::new(),
            dz_grab: None,
            dz_hover: None,
            camera_current: 0,
            camera_list: [0, 1, 2, 3],
            combo_scroll: None,
            focused: Default::default(),
            edits: HashMap::new(),
            selected_flags: Default::default(),
            unselected: Default::default(),
            hidden_flags: Default::default(),
            disabled_flags: Default::default(),
            picture_pan: Default::default(),
            picture_drag: None,
            chronicle_columns: Vec::new(),
            chronicle_icons: (String::new(), Default::default()),
            chronicle_captions: Vec::new(),
            combo_arrows,
            knob_at: None,
            scroll: HashMap::new(),
            hover_ms: 0,
            hover_of: None,
            controls, fe_strings, transition: Transition::Idle, overlay: false };
        screen.activate_glints();
        Ok(screen)
    }

    /// The activates that draw from `rand` — `NFS5.HEADER` (0x4CAE0D), `NFS5.CarLogo`
    /// (0x4C1409), `NFS5.GLINTYLINE` (0x5223F0 → 0x51F9A0), `NFS5.AnimatingPicture`'s bind
    /// (0x4BD8C7) and `NFS5.BOTTOMBARBUTTON`'s own sweep bind (0x4C9616, `widgets.rs`'s
    /// `draw_bar_or_graphic_tab`) — each stamping the screen's open, tick 0 of its clock. They are
    /// made in the widgets' own order: slot 21 is the **`INUSE`** handler — `FEControl`'s slot 12
    /// 0x4C7760 registers 0x4C7B50 (`call [vtbl+0x54]`) under `"INUSE"` through the message-map
    /// insert 0x526000 (0x4C7916…0x4C792A), and the screen's slot 38 0x4E0C60 broadcasts `INUSE`
    /// to its children in the child vector's order (read 2026-09-27) — which for the bottom bar's
    /// own four buttons is *this* screen (the bar loads and binds them, `bar_oval.rs`'s marker,
    /// before the front end's first layout). The flourish objects draw nothing when they are built
    /// and are made on their first draw (`glinty_flourish`).
    fn activate_glints(&self) {
        let mut g = self.glints.borrow_mut();
        for (i, e) in self.layout.elements.iter().enumerate() {
            let made = match e.class.as_str() {
                "NFS5.HEADER" => glints::Glint::Sweep(glints::Sweep::new(0, false)),
                "NFS5.CarLogo" | "NFS5.BOTTOMBARBUTTON" => glints::Glint::Sweep(glints::Sweep::new(0, true)),
                "NFS5.GLINTYLINE" => {
                    // 0x51F960 parks the widget's box: with `horizontal` set the bar runs top…bottom
                    // and the sweep left…right, and the other way round without it
                    let horizontal = self.attr_num(e, "horizontal").unwrap_or(1.0) != 0.0;
                    let (w, h) = self.box_of(e, 0);
                    let (short, long) = if horizontal { (h, w) } else { (w, h) };
                    glints::Glint::Line(glints::Line::new(0, short as i32, long as i64))
                }
                "NFS5.AnimatingPicture" => {
                    let lo = e.int("mindelay").unwrap_or(0);
                    let hi = e.int("maxdelay").unwrap_or(0);
                    glints::Glint::Anim(glints::Anim::new(0, lo, hi))
                }
                _ => continue,
            };
            g.insert(i, (made, false));
        }
    }

    /// Step widget `i`'s glint to tick `now`. A preloaded clock (`NFS_FE_CLOCK`) has no frames
    /// behind it, so the first step first runs the ticks it skipped, one a tick, the screen at
    /// rest — the diagnostic's stand-in; in a game the first draw is at tick 0…2.
    fn step_glint<T>(&self, i: usize, now: i64, make: impl FnOnce() -> glints::Glint, mut step: impl FnMut(&mut glints::Glint, i64, bool) -> T) -> T {
        let mut all = self.glints.borrow_mut();
        let (g, seen) = all.entry(i).or_insert_with(|| (make(), false));
        if !*seen {
            *seen = true;
            for t in 1..self.preload_ticks.min(now) {
                step(g, t, true);
            }
        }
        step(g, now, false)
    }

    /// The widgets the screen does not draw: the classes whose content comes from the executable,
    /// plus any `NFS5.CarLogo` the caller has not bound a car to yet.
    pub fn unrendered(&self) -> Vec<(String, usize)> {
        let mut out = self.unrendered.clone();
        for (class, bound) in [("NFS5.CarLogo", &self.logos.keys().copied().collect::<Vec<_>>()), ("NFS5.ListBox", &self.lists.keys().copied().collect::<Vec<_>>())] {
            let n = self.layout.elements.iter().enumerate().filter(|(i, e)| e.class == class && !bound.contains(i)).count();
            if n > 0 {
                out.push((class.to_string(), n));
            }
        }
        out.sort();
        out
    }

    /// The same, but naming each widget's own `accessor` / `othersource` so a sweep can tell an
    /// unimplemented source from a list that is simply empty in the current state.
    pub fn unrendered_sources(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (class, bound) in [("NFS5.CarLogo", self.logos.keys().copied().collect::<Vec<_>>()), ("NFS5.ListBox", self.lists.keys().copied().collect::<Vec<_>>())] {
            for (i, e) in self.layout.elements.iter().enumerate() {
                if e.class == class && !bound.contains(&i) {
                    let src = self
                        .attr(e, "accessor")
                        .filter(|v| !v.is_empty())
                        .or_else(|| self.attr(e, "othersource").filter(|v| !v.is_empty()))
                        .unwrap_or("(none)")
                        .to_string();
                    out.push((class.to_string(), src));
                }
            }
        }
        out.sort();
        out
    }

    /// The value of a widget attribute: the `.lay` block's own key when it carries one, otherwise
    /// the default in the class's `.ctr` template (`RE/notes/controls.md` §2). A key the block
    /// leaves empty counts as absent, which is how the shipped files spell "use the default"
    /// (`selectmode=`, `disablesound=`).
    pub fn attr<'a>(&'a self, e: &'a nfs_formats::lay::Element, key: &str) -> Option<&'a str> {
        if let Some(v) = e.get(key).filter(|v| !v.is_empty()) {
            return Some(v);
        }
        self.controls.get(&e.class)?.default_of(key).filter(|v| !v.is_empty())
    }

    fn attr_num(&self, e: &nfs_formats::lay::Element, key: &str) -> Option<f64> {
        let v = self.attr(e, key)?.trim();
        match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
            Some(h) => u64::from_str_radix(h, 16).ok().map(|n| n as f64),
            None => v.parse().ok(),
        }
    }

    /// One name of a `.ctr` expression, answered the way the macro dictionary `[0x69CAF8]`
    /// answers it: with the **text** a part published, which the evaluator splices in and expands
    /// again (`nfs_formats::expr`). What is in the dictionary:
    ///
    /// * **`<part>.<attr>`** for every attribute of every part — `Attribute::slot3` 0x52C140
    ///   defines it as the attribute's string `[attr+0x10]` (0x52C1E3), which is the `.lay`
    ///   block's value for the attribute's screen key when it may be overridden (0x52C000) and
    ///   the template's `<attr>.value` otherwise. The `DATA` part is called `data`, so this is
    ///   `data.width` too — and only for an attribute its `attributes.value` **declares**;
    /// * a **point** attribute (`pos`, `size`) instead defines `<part>.<attr>.x` and `.width` as
    ///   the text before its first comma and `.y` and `.height` as all of the text after it, and
    ///   **nothing** — not even the bare name — when the text has no comma (`PointAttribute::slot3`
    ///   0x5277F0: `strchr` at 0x527814, `je 0x527B68`; the four `0x52DC20` at 0x52794F,
    ///   0x527A81, 0x527AE4, 0x527B43).
    ///
    /// A name that is not there is [`Macro::Undefined`](nfs_formats::ctr::Macro::Undefined) and
    /// reads 0: `BottomBarButton.ctr`'s `data.area`, `ChatDisplayBox.ctr`'s `backrect.*`,
    /// `Tab Button.ctr`'s `data.height`.
    ///
    /// Three answers are the port's own, as before: `control.size.*` is the widget's box as this
    /// port computes it (which prefix the original gives the control's own attributes was not
    /// traced); a `SHAPE` part's `size` the template leaves empty is its art's (the pack loader's
    /// 0x5276E0 at 0x4E6EE2) — and a **text** part's `size` is always its text's truncated measure,
    /// which its load 0x4E7160 writes over the attribute and the next update publishes
    /// (`part_extent`, `part_text`; CONFIRMED 2026-09-26, correcting "no code publishes a
    /// measured size"); and a name the port
    /// cannot answer — that extent when it cannot measure (a chain deeper than five: the
    /// **APPROXIMATION** that breaks the cycle a `control.size.*` → `size.value` → part extent →
    /// part text → `control.size.*` walk would make — no shipped template has one; the original
    /// has no recursion, its attributes hold the values of the last update), a part attribute the template leaves
    /// empty whose constructor default the port does not keep, a chain deeper than five — is
    /// [`Macro::Unanswerable`](nfs_formats::ctr::Macro::Unanswerable), so the caller keeps its
    /// fallback. Names are matched without regard to case; the original's dictionary is
    /// case-sensitive (`std::map` over `memcmp`, 0x4242C0), and every reference in the 72
    /// shipped templates matches its definition exactly, so the two agree on the data.
    fn reference(&self, e: &nfs_formats::lay::Element, name: &str, depth: u32) -> nfs_formats::ctr::Macro {
        use nfs_formats::ctr::Macro;
        let name = name.to_lowercase();
        // every identifier of the text comes here, `x1f` of a `0x1f` included
        let Some((head, rest)) = name.split_once('.') else { return Macro::Undefined };
        if head == "control" {
            if depth > 4 {
                return Macro::Unanswerable;
            }
            let (bw, bh) = self.box_of(e, depth + 1);
            return match rest {
                "size.width" => Macro::Text((bw as i32).to_string()),
                "size.height" => Macro::Text((bh as i32).to_string()),
                _ => Macro::Unanswerable,
            };
        }
        let Some(c) = self.controls.get(&e.class) else { return Macro::Unanswerable };
        let Some(part) = c.part(head) else { return Macro::Undefined };
        let point = |key: &str, field: &str, text: &str| -> Macro {
            let Some((x, y)) = text.split_once(',') else { return Macro::Undefined };
            match field {
                "x" | "width" => Macro::Text(x.to_string()),
                "y" | "height" => Macro::Text(y.to_string()),
                _ => {
                    let _ = key;
                    Macro::Undefined
                }
            }
        };
        if part.kind() == nfs_formats::ctr::PartKind::Data {
            // `data.<attr>`: declared attributes only — every shipped one is an `INT`, a `COLOR`
            // or a `STRING`, none a point, so a dotted field after it names nothing
            if rest.contains('.') || !part.attributes.iter().any(|a| a == rest) {
                return Macro::Undefined;
            }
            return match self.part_attr(e, part, rest) {
                Some(t) => Macro::Text(t.to_string()),
                // declared with no value anywhere: the attribute's constructor string — `"%d"` of
                // 0 for an INT (0x52B744 → 0x527140 → 0x5242E0), taken as empty for the others
                None if part.ty(rest).is_some_and(|t| t.trim().eq_ignore_ascii_case("INT")) => Macro::Text("0".into()),
                None => Macro::Text(String::new()),
            };
        }
        let (key, field) = match rest.split_once('.') {
            Some((k, f)) => (k, Some(f)),
            None => (rest, None),
        };
        let is_point = key == "pos" || key == "size";
        let text_part = matches!(part.kind(), nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText);
        match (field, is_point) {
            // a text part's `text` is what it holds — for `LOCALIZEDTEXT` the string its load
            // wrote from `stringID` (0x4E66E0 → `[+0x170]`, the string `Attribute::slot3`
            // publishes)
            (None, false) if text_part && key == "text" => self.part_text(e, part, depth + 1).map_or(Macro::Unanswerable, Macro::Text),
            // …and its `size` is always that text's truncated measure: 0x4E7160 writes it over
            // whatever the template or the block gave (0x4E72A0), and the update 0x5256F0
            // publishes it (`Part::slot3`, 0x5257CD) before the parts are evaluated again — which
            // is why every tick that changes a watch's text runs the update **twice** (0x4BD349,
            // 0x4BD350), and the input box's bind too (0x4CC577 / 0x4CC57E)
            (Some(f), true) if text_part && key == "size" => match (depth <= 4).then(|| self.part_extent(e, part, depth)).flatten() {
                Some((w, h)) => point(key, f, &format!("{},{}", w as i32, h as i32)),
                None => Macro::Unanswerable,
            },
            (None, false) => self.part_attr(e, part, key).map_or(Macro::Unanswerable, |t| Macro::Text(t.to_string())),
            (Some(f), true) => match self.part_attr(e, part, key) {
                Some(t) => point(key, f, t),
                // A drawable part's `size` is the extent of **what it draws** when the template
                // does not give it one: `Picture.ctr`'s picture part and `BottomBarButton.ctr`'s
                // label both carry `size.flags=HIDDEN|READONLY` and no `size.value` at all, and
                // the bar button's own position is written over `label.size.width +
                // picture.size.width`. A measured extent is only for an explicit
                // `<part>.size.*` reference, never for the widget's own box: `Label.ctr`'s
                // `size.value` is `label.size.width*2,…`, and letting that resolve would give
                // every label a box twice its text and move it, because this port implements the
                // alignment rule as an anchor in the box (`layouts.md` §2.1).
                None if key == "size" && depth <= 4 => match self.part_extent(e, part, depth) {
                    Some((w, h)) => point(key, f, &format!("{},{}", w as i32, h as i32)),
                    None => Macro::Unanswerable,
                },
                None => Macro::Unanswerable,
            },
            _ => Macro::Undefined,
        }
    }

    /// Every part expression on this screen that does **not** resolve, as
    /// `(class, part, key, expression)`.
    ///
    /// `reference` answers `Unanswerable` for a name the port cannot resolve and every caller
    /// falls back to a zero or to the widget's box, so a template the port cannot evaluate is
    /// silent. (A name the original does not define is not one of these: it reads 0.) This is the
    /// measurement of that: `--layout-sweep` with `NFS_EXPR_SWEEP=1` runs it over all 98 layouts.
    pub fn expr_failures(&self) -> Vec<(String, String, String, String)> {
        let mut out = Vec::new();
        for e in self.layout.elements.iter() {
            let Some(c) = self.controls.get(&e.class) else { continue };
            let state = e.get("state").unwrap_or("normal");
            for name in c.state_parts(state) {
                let Some(p) = c.part(name) else { continue };
                for key in ["pos", "size"] {
                    // a value with no comma is never evaluated (0x527B80), so it cannot fail
                    let Some(v) = self.part_attr(e, p, key).filter(|v| v.contains(',')) else { continue };
                    if nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)).is_none() {
                        out.push((e.class.clone(), name.clone(), key.to_string(), v.to_string()));
                    }
                }
            }
        }
        out
    }

    /// Every `NFS5.GouraudIndicatorBar` and the source it reads. `FEGouraudIndicatorBar`'s
    /// constructor takes both keys (0x4CA24F `data.othersource`, 0x4CA27E `data.accessor`) and its
    /// value getter tries the `othersource` names first, falling through to the accessor at
    /// 0x4CA974. The flag says which of the two the name came from.
    /// How many index slots this screen occupies: its own widgets, then every layout its panels
    /// and tab strips embed, in widget order. Binding a screen's content is keyed by widget index
    /// (`set_bar`, `set_list_rows`, …), and a panel's sub-layout is as much a part of the screen as
    /// the widgets beside it — the audio sliders of `Options.lay` live in `audioreal.lay`, three
    /// files down. This flattening is what lets one index reach them.
    fn span(&self) -> usize {
        self.layout.elements.len() + self.groups.values().map(|g| g.span()).sum::<usize>()
    }

    /// Every embedded screen with the index its own slot 0 sits at, in widget order.
    fn group_spans(&self) -> Vec<(usize, &LayoutScreen)> {
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        let mut out = Vec::new();
        for k in keys {
            let g = &self.groups[&k];
            out.push((base, g.as_ref()));
            base += g.span();
        }
        out
    }

    /// Which embedded screen an index belongs to, and its index there. `None` means this screen's
    /// own — an index past the end routes nowhere and every setter drops it.
    fn route(&self, index: usize) -> Option<(usize, usize)> {
        if index < self.layout.elements.len() {
            return None;
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let span = self.groups[&k].span();
            if index < base + span {
                return Some((k, index - base));
            }
            base += span;
        }
        None
    }

    /// The flat index (the one [`Self::tab_click`] takes) of the `nth` (0-based, in file order,
    /// depth-first) widget named `name`, here or in an embedded layout — for the `NFS_FE_TAB`
    /// diagnostic. `nth` exists because several shipped tab strips give every button in the
    /// strip the same `name` — `controllertabs.lay`'s five buttons are all `ID_TABBUTTON`,
    /// `graphicstabs.lay`'s two are too — where only `showcasetabs.lay` and `optiontabs.lay`
    /// name each of their buttons uniquely. The original never needs to pick one by name; its
    /// own click is a screen coordinate, so a name collision is invisible to it.
    pub fn flat_index_of(&self, name: &str, nth: usize) -> Option<usize> {
        self.flat_index_of_skip(name, nth).1
    }

    /// `flat_index_of`'s recursion: the index found, or how many more matches were skipped over
    /// without yet reaching `nth`, so the caller above can keep counting into the next group.
    fn flat_index_of_skip(&self, name: &str, mut skip: usize) -> (usize, Option<usize>) {
        for (i, e) in self.layout.elements.iter().enumerate() {
            if e.name.eq_ignore_ascii_case(name) {
                if skip == 0 {
                    return (0, Some(i));
                }
                skip -= 1;
            }
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            let (rem, found) = g.flat_index_of_skip(name, skip);
            if let Some(j) = found {
                return (0, Some(base + j));
            }
            skip = rem;
            base += g.span();
        }
        (skip, None)
    }

    pub fn indicator_bars(&self) -> Vec<(usize, String, bool)> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.GouraudIndicatorBar")
            .filter_map(|(index, e)| {
                match e.get("othersource").filter(|a| !a.is_empty()) {
                    Some(a) => Some((index, a.to_string(), true)),
                    None => e.get("accessor").filter(|a| !a.is_empty()).map(|a| (index, a.to_string(), false)),
                }
            })
            .chain(self.group_spans().into_iter().flat_map(|(base, g)| g.indicator_bars().into_iter().map(move |(i, a, o)| (i + base, a, o))))
            .collect()
    }

    /// Every `NFS5.CarSettingSlider` and the accessor it watches, in the flattened index space.
    pub fn car_setting_sliders(&self) -> Vec<(usize, String)> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CarSettingSlider")
            .filter_map(|(index, e)| e.get("accessor").filter(|a| !a.is_empty()).map(|a| (index, a.to_string())))
            .chain(self.group_spans().into_iter().flat_map(|(base, g)| g.car_setting_sliders().into_iter().map(move |(i, a)| (i + base, a))))
            .collect()
    }

    /// The screen's `NFS5.CheckBox`es and the `data.boolAccessor` each one watches — the string
    /// its bind looks up at 0x4C3639 and keeps in `[this+0x144]`, whose value `FECheckBox::refresh`
    /// 0x4C3730 turns into the `onIndex` / `offIndex` entry it draws. §4.96.
    pub fn check_boxes(&self) -> Vec<(usize, String)> {
        let mut out: Vec<(usize, String)> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CheckBox")
            .filter_map(|(i, e)| e.get("boolaccessor").filter(|a| !a.is_empty()).map(|a| (i, a.to_string())))
            .collect();
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            for (i, a) in self.groups[&k].check_boxes() {
                out.push((base + i, a));
            }
            base += self.groups[&k].span();
        }
        out
    }

    /// Tick or untick one `NFS5.CheckBox`.
    pub fn set_check(&mut self, index: usize, value: Option<bool>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_check(j, value);
            }
            return;
        }
        match value {
            Some(v) => self.checks.insert(index, v),
            None => self.checks.remove(&index),
        };
    }

    /// Put one slider's knob where its accessor is: the index it is on and how many entries its
    /// list has. Without it the knob sits at the track's left end.
    pub fn set_knob(&mut self, index: usize, value: Option<(i32, i32)>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_knob(j, value);
            }
            return;
        }
        match value {
            Some(v) => self.knobs.insert(index, v),
            None => self.knobs.remove(&index),
        };
    }

    /// Give one indicator bar its value, or `None` to leave it blank.
    pub fn set_bar(&mut self, index: usize, value: Option<i32>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_bar(j, value);
            }
            return;
        }
        match value {
            Some(v) => self.bars.insert(index, v),
            None => self.bars.remove(&index),
        };
    }

    /// The texture of any font this screen loaded, for the sprite self-test.
    pub fn a_font_texture(&self) -> Option<TextureId> {
        self.fonts.values().next().map(|f| f.texture())
    }

    /// The 1×1 white texture this screen uploaded, for callers that want to draw a plain quad.
    pub fn white_texture(&self) -> TextureId {
        self.white
    }

    /// Every widget of `class` and the `number` it carries, in element order.
    pub fn numbered_widgets(&self, class: &str) -> Vec<(usize, i64)> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == class)
            .map(|(index, e)| (index, e.int("number").unwrap_or(0)))
            .chain(self.group_spans().into_iter().flat_map(|(base, g)| g.numbered_widgets(class).into_iter().map(move |(i, n)| (i + base, n))))
            .collect()
    }

    /// Draw one entry of a `.fsh` pack from `dir` at the widget's box.
    ///
    /// This is how the classes that are only a **place** get their picture. Two of them say so in
    /// their own template: `ChroniclePictureIcon.ctr`'s header is "Simply used for the x,y and to
    /// grab input events", and `ShowcasePicture.ctr` has no drawable part either. What goes there
    /// is picked from code, and in both cases the `.lay` block's `number` is the entry:
    ///
    /// * the chronicle packs open with two 41×25 entries, `bi` and `ci`, which is exactly the
    ///   icon widget's `size=41,25`;
    /// * a showcase pack holds eight, and `general.lay` and `glancestats.lay` between them give
    ///   `number` 0…7 to widgets whose `size` matches entry `number`'s dimensions on all eight
    ///   (128×90, 127×90, 128×91, 127×91, twice). CONFIRMED by that match.
    pub fn set_pack_picture(&mut self, r: &mut Renderer, paths: &GamePaths, dir: &str, index: usize, file: &str, entry: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_pack_picture(r, paths, dir, j, file, entry);
            }
            return;
        }
        if file.is_empty() {
            self.logos.remove(&index);
            return;
        }
        let key = (file.to_string(), entry);
        if !self.shapes.contains_key(&key) {
            let dir = paths.fe_data().join(dir);
            let entries = GamePaths::find_file(&dir, file)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            let Some(img) = entries.get(entry) else {
                self.logos.remove(&index);
                return;
            };
            let Some(px) = img.mips.first() else { return };
            let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
            self.shapes.insert(key.clone(), (id, img.width as f32, img.height as f32));
        }
        self.logos.insert(index, key);
    }

    /// Load one entry of a pack into the shape cache, without binding it to a widget — the
    /// timeline's icons are ten cells inside a single widget, not ten widgets (§4.123).
    pub fn load_pack_entry(&mut self, r: &mut Renderer, paths: &GamePaths, dir: &str, file: &str, entry: usize) {
        let key = (file.to_string(), entry);
        if self.shapes.contains_key(&key) {
            return;
        }
        let dir = paths.fe_data().join(dir);
        let entries = GamePaths::find_file(&dir, file)
            .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
            .and_then(|(p, d)| decode_pack(&p, &d).ok())
            .unwrap_or_default();
        let Some(img) = entries.get(entry) else { return };
        let Some(px) = img.mips.first() else { return };
        let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
        self.shapes.insert(key, (id, img.width as f32, img.height as f32));
    }

    /// Bind a `NFS5.MOVIEBOX` to a `.mad` and start it playing.
    ///
    /// The whole file is kept, its video chunks indexed, and one texture allocated to decode into.
    /// [`LayoutScreen::advance_movies`] steps it; the period is the header's own (see
    /// [`MoviePlayer`]). `audio` is the widget's own `data.audio` (default 1, `MovieBox.ctr` /
    /// `MovieCircle.ctr`) — whether the tick may skip frames to catch up after a stall, and
    /// (`mad.md` §14) whether it has its own `SC**` track at all; that track is decoded here but
    /// only started by the first [`LayoutScreen::advance_movies`] call that has a sound device.
    pub fn set_movie(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, dir: &str, file: &str, audio: bool) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_movie(r, paths, j, dir, file, audio);
            }
            return;
        }
        if self.movies.contains_key(&index) {
            return;
        }
        if let Some(m) = MoviePlayer::open(r, paths, dir, file, audio) {
            self.movies.insert(index, m);
        }
    }

    /// True when this screen — or the one behind it — has a movie playing.
    pub fn has_movies(&self) -> bool {
        !self.movies.is_empty() || self.background.as_ref().is_some_and(|b| b.has_movies())
    }

    /// Step every `NFS5.MOVIEBOX` on this screen by `dt_ms`, decoding and uploading the frames its
    /// own period calls for. A movie that has run out starts again, which is what a front-end loop
    /// does.
    /// Step the cursor's animation clock and take the button state 0x5299C0 reads out of
    /// `[0x69CAE8]`. The rates are the original's, off `TIMER_getTicks()` — see
    /// [`CURSOR_SLOW_MS`]; the clock used to carry a rate of this port's own.
    ///
    /// The period is a whole number of both animations' cycles so the wrap is invisible: 125 ms a
    /// frame over at most five frames, and 31.25 ms a frame over at most six, divide into
    /// 1000 × 125 ms.
    pub fn advance_cursor(&mut self, dt_ms: u32, mouse_down: bool) {
        if self.cursor_ms != u32::MAX {
            self.cursor_ms = self.cursor_ms.wrapping_add(dt_ms) % (CURSOR_SLOW_MS * 1000);
        }
        self.cursor_btn = mouse_down;
    }

    /// Which cursor shape this screen asks for, and the frame of it to draw.
    ///
    /// **A shape is requested, not set**: 0x4E1E50 is `[0x690720] = max([0x690720], n)` and
    /// 0x4D2F3D clears the slot at the top of the next frame, so the highest number anyone asked
    /// for during a frame is the one drawn (§4.152). The two requesters the notes have read are
    /// `FEControl`'s slot 24 — 0x4C7F40, `push 1`, dispatched from the per-tick slot 16 at
    /// 0x4C7FDF while a widget is hovered — and `FECar3DControl`'s slot 15, `push 7`, the
    /// magnifier over a hovered 3D car (`frontend-3d.md` §5).
    fn cursor_shape(&self, hovered: Option<usize>) -> usize {
        let mut want = 0usize;
        if let Some(h) = hovered {
            want = want.max(1);
            // `FEChroniclePicture::draw` asks for shape **4** while it is hovered (0x4C7C90) and
            // its picture is bigger than its box on either axis (0x50851C…0x508531)
            if self.layout.elements.get(h).is_some_and(|e| e.class == "NFS5.ChroniclePicture")
                && let Some((_, _, pw, ph)) = self.chronicle_pan(h)
                && let Some(e) = self.layout.elements.get(h)
                && {
                    let (bw, bh) = self.box_of(e, 0);
                    pw - bw > 0.0 || ph - bh > 0.0
                }
            {
                want = want.max(4);
            }
            if let Some(e) = self.layout.elements.get(h)
                && e.class.eq_ignore_ascii_case("NFS5.3DCAR")
            {
                // A **focused** 3D car is the magnifier, shape 7 — 0x4C09F7 tests 0x4C7DD0,
                // bit 0x40 of `[widget+0x10C]` (§4.109), and only then pushes 7. The car holds
                // focus while zoomed in on an open door (slot 9, 0x4C1002). A merely *hovered*
                // one — 0x4C7C90, bit 0x04 — asks per zone.
                let parts = self.car_parts.get(&h);
                if parts.is_some_and(|p| p.zoomed) {
                    return 7;
                }
                want = want.max(match self.car_zone(h, e) {
                    Some(0) => 6,
                    // zones 1 and 2 read `byte[this+0x1AE + (zone != 1)]` at 0x4C0A41: a door
                    // clicked open is `magP`, a closed one `grab`
                    Some(z @ (1 | 2)) => {
                        if parts.is_some_and(|p| p.door_flag[usize::from(z != 1)]) { 8 } else { 5 }
                    }
                    Some(_) => 3,
                    // hovered but off the box — the plain hovered arrow slot 24 already asked for
                    None => 1,
                });
            }
        }
        want
    }

    /// Which of the five zones of a `NFS5.3DCAR` the pointer is in — `FECar3DControl`'s
    /// **0x4C0D10**, `RE/notes/controls.md` §4.153. `None` when the pointer is not on it.
    ///
    /// The zone is the turntable angle's fraction crossed with the horizontal third of the
    /// widget's box, and the answers are the ones 0x4C0DCD…0x4C0E89 write into `edi`.
    fn car_zone(&self, index: usize, e: &nfs_formats::lay::Element) -> Option<u32> {
        let (x, y) = self.cursor?;
        let (bx, by) = self.position_of(e);
        let (w, h) = self.box_of(e, 0);
        let roof = self.car_roof.contains(&index);
        car_zone_of(self.car_spin, roof, x, y, bx, by, w, h)
    }

    /// Whether the car a `NFS5.3DCAR` shows has a roof animation — `0x438190(model, 1)` non-zero,
    /// the test 0x4C0D10 makes before its roof zone.
    pub fn set_car_roof(&mut self, index: usize, roof: bool) {
        if roof {
            self.car_roof.insert(index);
        } else {
            self.car_roof.remove(&index);
        }
    }

    /// A click released on a `NFS5.3DCAR` — slot 9, 0x4C0EB0: the zone under the pointer
    /// (0x4C0D10 on the event's own x, y) and the parts state machine of `crate::carparts`.
    /// Returns what the car and the sound layer are asked to do.
    pub fn car_click(&mut self, index: usize) -> Vec<crate::carparts::PartEvent> {
        let mut out = Vec::new();
        let Some(e) = self.layout.elements.get(index) else { return out };
        if !e.class.eq_ignore_ascii_case("NFS5.3DCAR") {
            return out;
        }
        let zone = self.car_zone(index, e);
        let now = self.ticks();
        let parts = self.car_parts.entry(index).or_default();
        let focused = parts.zoomed;
        if let Some(z) = zone {
            parts.click(z, now, focused, &mut out);
        }
        out
    }

    /// A click on `zone` of 3D car `index` at tick `now`, as `car_click` makes one — for
    /// `NFS_FE_PARTS`, which opens parts for a still.
    pub fn car_click_zone(&mut self, index: usize, zone: u32, now: u32) -> Vec<crate::carparts::PartEvent> {
        let mut out = Vec::new();
        let parts = self.car_parts.entry(index).or_default();
        let focused = parts.zoomed;
        parts.click(zone, now, focused, &mut out);
        out
    }

    /// The parts' per-frame step (0x4C0696…0x4C07CF) for every 3D car on the screen.
    pub fn step_car_parts(&mut self) -> Vec<crate::carparts::PartEvent> {
        let now = self.ticks();
        let mut out = Vec::new();
        for p in self.car_parts.values_mut() {
            p.step(now, &mut out);
        }
        out
    }

    /// The widget's `Car.Roof` check each tick (0x4C05E7…0x4C063F; the bind 0x4BFE71 on the first).
    pub fn sync_car_roof(&mut self, index: usize, down: bool) -> Vec<crate::carparts::PartEvent> {
        let now = self.ticks();
        let mut out = Vec::new();
        self.car_parts.entry(index).or_default().sync_roof(down, now, &mut out);
        out
    }

    /// The room zoom a 3D car's tick hands on (`CarParts::zoom`), and whether it is zoomed in —
    /// while it is, the tick covers the screens every frame (0x4C052F → 0x4D2000).
    pub fn car_zoom(&self, index: usize) -> (f32, bool) {
        let now = self.ticks();
        self.car_parts.get(&index).map(|p| (p.zoom(now), p.zoomed)).unwrap_or((0.0, false))
    }

    /// Zone 0's state, 0…255 — what 0x4C07E8 hands 0x4D1990(1, ·) after every step.
    pub fn car_roof_state(&self, index: usize) -> i32 {
        self.car_parts.get(&index).map(|p| p.state[0]).unwrap_or(0)
    }

    /// Channels 6 and 7 of the car a `NFS5.3DCAR` widget shows — its two doors, in turns.
    pub fn car_doors(&self, index: usize) -> (f32, f32) {
        self.car_parts.get(&index).map(|p| p.doors()).unwrap_or((0.0, 0.0))
    }

    /// `TIMER_getTicks()` (0x53C2E0) — the 128 Hz count — from the screen's clock.
    pub fn ticks(&self) -> u32 {
        (self.clock_ms as u64 * 128 / 1000) as u32
    }

    /// Where the 3D room's turntable stands, in turns, for [`Self::car_zone`].
    pub fn set_car_spin(&mut self, turns: f32) {
        self.car_spin = turns;
    }
}

/// The body of [`LayoutScreen::car_zone`], free of the screen so it can be tested on its own.
pub fn car_zone_of(spin: f32, roof: bool, x: f32, y: f32, bx: f32, by: f32, w: f32, h: f32) -> Option<u32> {
    {
        if x < bx || y < by || x >= bx + w || y >= by + h {
            return None;
        }
        // 0x4C0D5D: the top **5/16** of the box is zone 0 whatever the angle — reached only when
        // the car's roof animation has a length, `0x438190(model, 1)` non-zero
        if roof && y <= by + 5.0 * h / 16.0 {
            return Some(0);
        }
        let t = spin.rem_euclid(1.0);
        let third = w / 3.0;
        let left = x < bx + third;
        let right = x > bx + w - third;
        // 0x4C0DD2…0x4C0E89, in the order the compares run
        Some(match t {
            t if t <= 0.1 || t > 0.9 => 4,
            t if t <= 0.4 =>
            // 0x4C0E57: left third 4, middle 2, right 3
            {
                if left {
                    4
                } else if right {
                    3
                } else {
                    2
                }
            }
            t if t <= 0.6 => 3,
            // 0x4C0E0F: left third 3, middle 1, right 4
            _ => {
                if left {
                    3
                } else if right {
                    4
                } else {
                    1
                }
            }
        })
    }
}

impl LayoutScreen {

    /// The cursor itself, drawn over everything. Nothing else in the screen may come after it.
    fn cursor_sprite(&self, out: &mut Vec<Sprite>, aspect: f32, hovered: Option<usize>) {
        // `u32::MAX` marks a screen that is somebody's background; only the screen in front draws
        if self.cursor_ms == u32::MAX {
            return;
        }
        let Some((cx, cy)) = self.pointer.unwrap_or(self.cursor) else { return };
        let shape = self.cursor_shape(hovered);
        let Some(&(first, last)) = CURSOR_SHAPES.get(shape) else { return };
        // the two stamps of 0x4E1C90, restamped here because this is the only place that knows
        // which shape won the frame's request and what the button is doing
        if self.cursor_shape_at.get().0 != shape {
            self.cursor_shape_at.set((shape, self.cursor_ms));
        }
        if self.cursor_btn_at.get().0 != self.cursor_btn {
            self.cursor_btn_at.set((self.cursor_btn, self.cursor_ms));
        }
        let frame = cursor_frame(
            shape,
            (first, last),
            self.cursor_ms.wrapping_sub(self.cursor_shape_at.get().1),
            self.cursor_ms.wrapping_sub(self.cursor_btn_at.get().1),
            self.cursor_btn,
        );
        let Some(&(tex, w, h)) = self.shapes.get(&(POINTER.to_string(), frame)) else { return };
        // **The hot spot, settled 2026-09-21 (0x4E1DF1…0x4E1E42).** The drawing function has no
        // per-shape table: it compares the requested shape against **2** and, only then, subtracts
        // `[0x5B2460]` = 6.0 from **both** coordinates before the blit — the two `fld [esp+0x54]`
        // at 0x4E1E03 and 0x4E1E11 read slots four bytes apart because a push sits between them,
        // and the `jne` arm at 0x4E1E2C passes the same two through untouched in the same order.
        // So every other shape really is drawn with its top-left corner on the pointer, and this
        // retires §4.152's last approximation. The port used to offset `x` alone.
        let o = cursor_hot_spot(shape);
        let (pos, size) = self.place(aspect, cx + o, cy + o, w, h);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
    }

    /// True once any movie on this screen has played through (0x4E2960).
    pub fn movie_finished(&self) -> bool {
        self.movies.values().any(|m| m.finished)
    }

    pub fn advance_movies(&mut self, r: &mut Renderer, dt_ms: u32, audio_out: Option<&nfs_audio::AudioOutput>, music_level: u8) {
        for m in self.movies.values_mut() {
            m.advance(r, dt_ms, audio_out, music_level);
        }
        // the end-of-era movie runs from the moment the show starts it (0x4FF76F) — its audio
        // track (`mad.md` §14) was decoded at `set_end_of_era` time but only starts here, on the
        // first tick this is true, the same as any movie's own not-yet-started case
        // (`MoviePlayer::advance`) — the video and the audio start together, matching the video's
        // own gate rather than the widget movies' (ticked from the same frame as `open`)
        if let Some(e) = self.end_of_era.as_mut()
            && e.show.borrow().movie_started().is_some()
            && let Some(m) = e.movie.as_mut()
        {
            m.advance(r, dt_ms, audio_out, music_level);
        }
        if let Some(b) = self.background.as_mut() {
            b.advance_movies(r, dt_ms, audio_out, music_level);
        }
    }

    /// Bind a `NFS5.KNOCKOUTTRACKLIST`: how many rows the knockout has and which one is current.
    pub fn set_ko_strip(&mut self, index: usize, rows: usize, current: usize) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_ko_strip(j, rows, current);
            }
            return;
        }
        self.ko_strip.insert(index, (rows, current));
    }

    /// Bind the events a `NFS5.TrackSmall2dSpline` shows. Each one's `<art>.fsh` is loaded and its
    /// entries 0 (the map) and 1/2 (the start and finish markers) are uploaded at the position the
    /// entry header itself carries — see [`nfs_assets::texture::TextureImage::place`].
    pub fn set_track_map(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, events: &[TrackMapEvent]) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_track_map(r, paths, j, events);
            }
            return;
        }
        let dir = paths.fe_data().join("TrackArt");
        let mut out = Vec::with_capacity(events.len());
        for ev in events {
            let entries = GamePaths::find_file(&dir, &format!("{}.fsh", ev.art))
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            let mut upload = |k: usize| -> Option<(TextureId, f32, f32, f32, f32)> {
                let img = entries.get(k)?;
                let px = img.mips.first()?;
                let key = (ev.art.clone(), k);
                let &mut (tex, w, h) = self.shapes.entry(key).or_insert_with(|| {
                    let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
                    (id, img.width as f32, img.height as f32)
                });
                Some((tex, w, h, img.place[0] as f32, img.place[1] as f32))
            };
            let map = upload(0);
            let mut marker = upload(if ev.backwards { 2 } else { 1 });
            if ev.mirror
                && let (Some(m), Some(k)) = (map, marker.as_mut())
            {
                k.3 = m.3 + m.1 - k.3 - k.1;
            }
            out.push(TrackMapArt { map, marker, name: ev.name.clone() });
        }
        self.track_maps.insert(index, out);
    }

    /// Bind a `NFS5.BADGE` — the Factory Driver licence card.
    ///
    /// `pack` is the card (`license`, `license2`, `license3`, `license4` by stage, 0x4CF1C8), `name`
    /// the player's name (0x501270 returns the global the front end keeps it in), `rank` the line
    /// under it and `colour` the colour the name is printed in. The drop shadow is loaded here too.
    pub fn set_badge(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, pack: &str, name: &str, rank: &str, colour: [f32; 4]) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_badge(r, paths, j, pack, name, rank, colour);
            }
            return;
        }
        self.set_pack_picture(r, paths, "Art", index, pack, 0);
        for entry in [0usize, 1] {
            if self.shapes.contains_key(&(LICENSE_SHADOW.to_string(), entry)) {
                continue;
            }
            let dir = paths.fe_data().join("Art");
            let Some(img) = GamePaths::find_file(&dir, LICENSE_SHADOW)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .and_then(|v| v.into_iter().nth(entry))
            else {
                continue;
            };
            let Some(px) = img.mips.first() else { continue };
            let id = r.upload_texture(&TextureData::single(img.width, img.height, px.clone()));
            self.shapes.insert((LICENSE_SHADOW.to_string(), entry), (id, img.width as f32, img.height as f32));
        }
        self.badges.insert(index, (name.to_string(), rank.to_string(), colour));
    }

    /// Point a `NFS5.CarLogo` at a **chronicle** picture instead of a car logo.
    ///
    /// The chronicle's own art is not in `FEData/Art` and is not a logo pair: `0x50757F` builds the
    /// name with `"%s%c%c%02d%d%c.fsh"` — the directory, `'M'` for a picture (`'p'` is the other
    /// kind), the model-group letter from the table at **`0x5B8B18` = `"CBADE"`** indexed by the
    /// group at `[0x5DBA44]`, the year mod 100, a 1-based index within the year, and a variant
    /// `'a' + n`. `.mad` is the same name for the movie. So one whole image, drawn on its own.
    pub fn set_chronicle_picture(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, file: &str) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_chronicle_picture(r, paths, j, file);
            }
            return;
        }
        if file.is_empty() {
            self.logos.remove(&index);
            return;
        }
        if self.logos.get(&index).is_some_and(|(f, b)| f == file && *b == CHRONICLE_MOSAIC_ENTRY) {
            return;
        }
        if !self.shapes.contains_key(&(file.to_string(), CHRONICLE_MOSAIC_ENTRY)) {
            let dir = paths.fe_data().join("Chron");
            let entries = GamePaths::find_file(&dir, file)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            // The picture is a **grid**, and `FEChroniclePicture::draw` (0x508310) measures it:
            // it scans the pack from entry **2** — 0 and 1 are the `bi` and `ci` icons — counting
            // the leading run of entries tagged `1111` as the number of columns (0x5083EF…0x508443),
            // summing those widths for the picture's width and the height of every row's first
            // entry for its height, and then blits entry `2 + row*cols + col` at the running
            // column x and row y (0x50856A…0x50861E). A pack's second row is tagged `Laye`, not
            // `1111`, which is why only the first row is matched by name. The widget on both
            // `chroniclepicture.lay` and `chroniclemedia.lay` is `size=458,286`, which is the
            // picture in 140 of the 183 packs; the other 43 are bigger — `MA601a.fsh` is **five**
            // rows, 4 × (128+128+128+74) by (148+138+148+138+78) = 458×650 (this said 458×286
            // until 2026-09-26, counting only its first two rows) — and the draw shows a
            // box-sized window of them that the player drags (`chronicle_pan`).
            //
            // The measure's own row height is the height of entry `2 + r·cols + 1` for r ≥ 1 (the
            // first `Laye` entry resets the column without adding, 0x50844D…0x50847F) and the
            // draw advances by the row's **last** tile (0x5085FE); every shipped row is one
            // height, so all three agree.
            let tiles: Vec<&nfs_assets::texture::TextureImage> = entries.iter().skip(2).collect();
            let cols = tiles.iter().take_while(|t| t.name.trim() == CHRONICLE_PICTURE_TAG).count();
            if cols == 0 || tiles.is_empty() {
                tracing::debug!(file, entries = entries.len(), "chronicle picture has no `1111` tiles");
                self.logos.remove(&index);
                return;
            }
            let rows = tiles.len() / cols;
            let w: u32 = tiles[..cols].iter().map(|t| t.width).sum();
            let h: u32 = (0..rows).map(|r| tiles[r * cols].height).sum();
            if w == 0 || h == 0 {
                self.logos.remove(&index);
                return;
            }
            let mut rgba = vec![0u8; (w * h * 4) as usize];
            let mut y0 = 0u32;
            for r in 0..rows {
                let mut x0 = 0u32;
                for c in 0..cols {
                    let t = tiles[r * cols + c];
                    if let Some(px) = t.mips.first() {
                        for y in 0..t.height.min(h.saturating_sub(y0)) {
                            let src = (y * t.width * 4) as usize;
                            let dst = (((y + y0) * w + x0) * 4) as usize;
                            let n = (t.width.min(w.saturating_sub(x0)) * 4) as usize;
                            if src + n <= px.len() && dst + n <= rgba.len() {
                                rgba[dst..dst + n].copy_from_slice(&px[src..src + n]);
                            }
                        }
                    }
                    x0 += t.width;
                }
                y0 += tiles[r * cols].height;
            }
            let id = r.upload_texture(&TextureData::single(w, h, rgba));
            self.shapes.insert((file.to_string(), CHRONICLE_MOSAIC_ENTRY), (id, w as f32, h as f32));
        }
        self.logos.insert(index, (file.to_string(), CHRONICLE_MOSAIC_ENTRY));
    }

    /// Point a `NFS5.CarLogo` at a car's logo: `pack` is the car record's +0x147 (`finallogos1`)
    /// and `pair` its +0x1F6. The widget draws entries `2*pair` and `2*pair + 1` of
    /// `FEData/Art/<pack>.fsh` (`NFS5.CarLogo::update` 0x4C1A63 — the index is the pair doubled,
    /// and the pair past the end of the pack draws nothing, 0x4C1AF6). CONFIRMED.
    pub fn set_car_logo(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, pack: &str, pair: i16) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_car_logo(r, paths, j, pack, pair);
            }
            return;
        }
        if pack.is_empty() || pair < 0 {
            self.logos.remove(&index);
            return;
        }
        let file = format!("{pack}.fsh");
        let base = pair as usize * 2;
        if self.logos.get(&index).is_some_and(|(f, b)| f == &file && *b == base) {
            return;
        }
        if !self.shapes.contains_key(&(file.clone(), base)) {
            let art = paths.fe_data().join("Art");
            let images = GamePaths::find_file(&art, &file)
                .and_then(|p| std::fs::read(&p).ok().map(|d| (p, d)))
                .and_then(|(p, d)| decode_pack(&p, &d).ok())
                .unwrap_or_default();
            for entry in [base, base + 1] {
                let Some(img) = images.get(entry) else {
                    tracing::debug!(file, entry, "car logo entry missing");
                    continue;
                };
                let Some(level0) = img.mips.first() else { continue };
                let id = r.upload_texture(&TextureData::single(img.width, img.height, level0.clone()));
                self.shapes.insert((file.clone(), entry), (id, img.width as f32, img.height as f32));
            }
        }
        // the original draws nothing unless both entries of the pair resolved (0x4C1B76)
        if self.shapes.contains_key(&(file.clone(), base)) && self.shapes.contains_key(&(file.clone(), base + 1)) {
            self.logos.insert(index, (file, base));
        } else {
            self.logos.remove(&index);
        }
    }

    /// `canplaygame` on the screen block: the bottom bar's PLAYGAME button is live on this screen.
    /// (The shipped files carry 0, 1, 2 or nothing; 0x4D3410 hands the value to the background
    /// chooser, so it is per-screen state, HIGH.)
    pub fn can_play(&self) -> bool {
        self.layout.screen().and_then(|s| s.int("canplaygame")).unwrap_or(0) != 0
    }

    /// The widgets a player can operate, in reading order: the buttons that lead somewhere and the
    /// combo sliders, each with the accessor it edits.
    pub fn controls(&self) -> Vec<Control> {
        let mut out: Vec<Control> = Vec::new();
        for (i, e) in self.layout.elements.iter().enumerate() {
            let (x, y) = e.position();
            let dest = e.get("destination").filter(|d| !d.is_empty()).map(str::to_string);
            let callback = e.get("callback").filter(|c| !c.is_empty()).map(str::to_string);
            let accessor = e.get("accessor").filter(|a| !a.is_empty()).map(str::to_string);
            let is_button = e.class.contains("BUTTON");
            if !(is_button || e.class == "NFS5.ComboBoxSlider") {
                continue;
            }
            if dest.is_none() && callback.is_none() && accessor.is_none() {
                continue;
            }
            let offset = self.attr_num(e, "offset").unwrap_or(0.0) as i32;
            let step = e.int("step").unwrap_or(0) as i32;
            out.push(Control { index: i, x, y, destination: dest, callback, accessor, string_id: e.string_id(), offset, step });
        }
        out.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    /// The elements that name another screen in `destination` — the screen's navigation.
    pub fn buttons(&self) -> Vec<usize> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.get("destination").is_some_and(|d| !d.is_empty()) && e.string_id().is_some())
            .map(|(i, _)| i)
            .collect()
    }

    pub fn destination(&self, index: usize) -> Option<&str> {
        self.layout.elements.get(index)?.get("destination")
    }

    pub fn label(&self, index: usize, strings: &Strings) -> String {
        self.layout.elements.get(index).and_then(|e| e.string_id()).and_then(|id| strings.get(id)).unwrap_or("").to_string()
    }

    /// The screen's music pattern (`musicfile=menu*`), for `MusicPlayer::play_pattern`.
    pub fn music_pattern(&self) -> Option<&str> {
        self.layout.screen().and_then(|s| s.get("musicfile")).filter(|s| !s.is_empty())
    }

    /// Map a design-space rectangle into sprite coordinates, letterboxing the 4:3 design inside a
    /// window of aspect `aspect`.
    fn place(&self, aspect: f32, x: f32, y: f32, w: f32, h: f32) -> ([f32; 2], [f32; 2]) {
        let design_aspect = DESIGN.0 / DESIGN.1;
        let (fw, x0) = if aspect > design_aspect { (design_aspect / aspect, (1.0 - design_aspect / aspect) * 0.5) } else { (1.0, 0.0) };
        ([x0 + x / DESIGN.0 * fw, y / DESIGN.1], [w / DESIGN.1, h / DESIGN.1])
    }

    fn quad(&self, aspect: f32, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) -> Sprite {
        let (pos, size) = self.place(aspect, x, y, w, h);
        Sprite { texture: self.white, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None }
    }

    /// One gouraud quad of the front end's own primitive **0x4D98D0**, whose four `(x, y, colour)`
    /// triples arrive in the order upper-left, upper-right, lower-left, lower-right (§4.7).
    ///
    /// **The node's own stored order, read to the instruction (2026-09-27):** 0x4D98D0 does not
    /// draw — it copies its twelve floats into a node and inserts it into a sorted `_Rb_tree`
    /// rooted at `[0x65B958]` (compare 0x4D9450, node alloc 0x4DAD30, insert 0x4314C0). Its own
    /// fill (0x4D993F…0x4D99DA) does **not** keep the caller's four corners — bottom-right,
    /// bottom-left, top-right, top-left (controls.md §4.7) — in that order: it swaps the last two,
    /// storing pos/colour slot 0 = the caller's corner 0 (BR), slot 1 = corner 1 (BL), slot 2 =
    /// corner **3** (TL), slot 3 = corner **2** (TR) — traced by which argument offset (`[esp+…]`)
    /// feeds which output slot. The flush **0x4DA5F0** copies a node's 64 B pos / 16 B colour / 32 B
    /// uv blocks verbatim (0x4DA79E/0x4DA7BD/0x4DA7D1) into the batch arrays, so this is also the
    /// order the device sees: **BR, BL, TL, TR**.
    ///
    /// **The diagonal.** Which two triangles the device call actually cuts from those four
    /// vertices was still not read this pass — past the batch-array copy the call goes through an
    /// `EAGL::Material` (`[0x65B978]`, vtable 0x5B3C94) whose own "apply" (0x53F1B0) only stages a
    /// render-context and walks a chain of state-layer objects (`LayerColours` 0x5B3BF0 among
    /// them), and the actual `DrawPrimitive` is somewhere past that chain or inside `dx7z.dll`
    /// itself — unread. But the stored order settles it anyway: a **strip** split (`(v0,v1,v2)` +
    /// `(v1,v2,v3)`) of BR, BL, TL, TR shares the edge BL–TL, which is a side of the quad, not a
    /// diagonal — the two triangles would overlap and not tile the quad at all (checked against
    /// coordinates). Only a split that shares **v0–v2** (a fan, or any index pattern built on that
    /// diagonal) tiles the quad correctly for this vertex order — the **BR–TL** diagonal, not the
    /// BL–TR one this port drew before. *(Until this pass the port kept the caller's own BR, BL,
    /// TR, TL order and read it as a strip, splitting BL–TR — wrong on two counts now traced: the
    /// node does not keep that order, and that order read as a strip is not even a valid tiling.)*
    /// This port used to step the quad into a grid of flat cells (up to 8 × 8), since its sprites had
    /// one colour a quad; the renderer's per-corner triangles make that unnecessary.
    fn gouraud(&self, out: &mut Vec<Sprite>, aspect: f32, p: [[f32; 2]; 4], c: [[f32; 4]; 4]) {
        let design_aspect = DESIGN.0 / DESIGN.1;
        let (fw, x0) = if aspect > design_aspect { (design_aspect / aspect, (1.0 - design_aspect / aspect) * 0.5) } else { (1.0, 0.0) };
        let at = |k: usize| [x0 + p[k][0] / DESIGN.0 * fw, p[k][1] / DESIGN.1];
        let tri = |a: usize, b: usize, d: usize| Sprite {
            texture: self.white,
            pos: [0.0; 2],
            size: [0.0; 2],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [1.0; 4],
            rotation: 0.0,
            pivot: [0.5, 0.5],
            tri: Some(([at(a), at(b), at(d)], [c[a], c[b], c[d]])), corner_colors: None,
            depth: None,
        };
        // UL 0, UR 1, BL 2, BR 3 — 0x4D98D0's node stores them BR, BL, TL, UR (swapping the
        // caller's TR/TL), which tiles the quad only along the BR–UL diagonal: (BR, BL, UL) and
        // (BR, UL, UR).
        out.push(tri(3, 2, 0));
        out.push(tri(3, 0, 1));
    }

    /// The state a widget draws in, which `FeWidget::updateState` **0x4C82A0** decides from its
    /// flag word `[this+0x10C]` and the mask `[this+0x110]` of states its template defines. The
    /// cascade is first-match-wins, each arm jumping to the end at 0x4C842E:
    ///
    /// | | condition | state |
    /// |---|---|---|
    /// | 0x4C82E5 | flags & 0x02 | keep whatever it has — no update |
    /// | 0x4C82FA | flags & 0x20 | `hidden` |
    /// | 0x4C831E | flags & 0x10, or `[+0x114]` set with `[+0x110] & 0x10` | `disabled` |
    /// | 0x4C835E | `[+0x110] & 0x80` and flags & 0x40 and flags & 0x04 | `focusedhover` |
    /// | 0x4C839A | flags & 0x40 | `focused` |
    /// | 0x4C83C1 | `[+0x110] & 0x100` and flags & 0x08 and flags & 0x04 | `selectedhover` |
    /// | 0x4C83EE | flags & 0x08 | `selected` |
    /// | 0x4C840F | flags & 0x04 | `hover` |
    /// | | otherwise | `normal` |
    ///
    /// So **0x04 is hover, 0x08 selected and 0x40 focused**, and hover is the weakest of the
    /// three — it only combines with the other two when the template names the combination.
    ///
    /// The glinty box a widget builds out of `data.glintybox` — `RE/notes/controls.md` §4.60.
    ///
    /// `0x51FDA0(0xFF000000, data.width, data.height, data.glintybox)` (0x4C9077 for a button,
    /// from the three the bind read at 0x4C8FCF…0x4C8FF5) and `0x51FDE0` draws it: a one-pixel
    /// outline of the box in that colour through the fade, and then §4.27's flourish with the
    /// style — 1…4 a fixed corner and **5 a fresh `rand() % 4 + 1` each run** (0x51FE86).
    ///
    /// It is the same painter, behind the same `THRASH_setstate(0x1C3, 0)` gate (§4.61), that
    /// `NFS5.GlintyRectangle` uses for its own 181 widgets — and this port draws those. Drawing
    /// one set and not the other would be the port disagreeing with itself, so it draws both; the
    /// gate is one uncertainty, not two, and it is `RE/AUDIT.md` §E's.
    #[allow(clippy::too_many_arguments)]
    fn glinty_box(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, i: usize, x: f32, y: f32, selected: Option<usize>, hovered: Option<usize>, tint: impl Fn([u8; 4]) -> [f32; 4]) {
        let style = match self.attr_num(e, "glintybox") {
            Some(v) if v as i32 != -1 => v as i32,
            _ => return,
        };
        // each class sizes its box its own way, and all three read the same three arguments into
        // `0x51FDA0(0xFF000000, w, h, style)`:
        //
        // * `NFS5.GOTOBUTTON` — `data.width` by `data.height` (0x4C8FEA, 0x4C8FF5);
        // * the tab buttons — `data.width` by the literal **18**, `0x12` at 0x4D0108;
        // * the combo family — **`data.width`** (`[this+0x1C8]`, 0x4C593B) by the
        //   `backrect`'s height, which is `data.height` (0x4C594B on `[this+0x17C]`), and it is
        //   built lazily in the draw, only once `[+0x20C]` is still null (0x4C5937).
        let (gw, gh) = match e.class.as_str() {
            "NFS5.TABBUTTON" | "NFS5.GRAPHICTABBUTTON" => (self.attr_num(e, "width").unwrap_or(0.0) as f32, 18.0),
            "NFS5.ComboBoxSlider" | "NFS5.ColorBox" | "NFS5.ListBox" => (
                // `[this+0x1C8]` is `data.width`, not `data.selectionboxwidth`: 0x4C4FCB…0x4C5002
                // pushes each attribute name *before* storing the previous call's answer, so the
                // whole run is one slot along from what an earlier reading of it had. With
                // `selectionboxwidth` here the box came out too narrow for 43 of the 86 combo
                // sliders and had **no width at all** for the 11 colour boxes, which all carry 0.
                self.attr_num(e, "width").unwrap_or(0.0) as f32,
                self.attr_num(e, "height").unwrap_or(0.0) as f32,
            ),
            _ => (self.attr_num(e, "width").unwrap_or(0.0) as f32, self.attr_num(e, "height").unwrap_or(0.0) as f32),
        };
        if gw < 1.0 || gh < 1.0 {
            return;
        }
        let c = tint([0, 0, 0, 0xff]);
        for (bx, by, bw, bh) in [(x, y, gw, 1.0), (x, y + gh - 1.0, gw, 1.0), (x, y, 1.0, gh), (x + gw - 1.0, y, 1.0, gh)] {
            out.push(self.quad(aspect, bx, by, bw, bh, c));
        }
        // the buttons' *may start* — the latch of 0x4C93DC / 0x4D04C0 / 0x4C6415 over the
        // widget's flags: disabled (0x4C7E40), hovered (0x4C7C90), selected (0x4C7D50), focused
        // (0x4C7DD0), and the fade (0x4C8820)
        let disabled = self.disabled_flags.contains(&i)
            || e.get("state") == Some("disabled")
            || (self.covered && self.controls.get(&e.class).is_some_and(|c| c.has_state("disabled")));
        let hot = hovered == Some(i);
        let sel = selected == Some(i) || self.selected_flags.contains(&i) || (e.get("state") == Some("selected") && !self.unselected.contains(&i));
        let focus = self.attr_num(e, "alwaysfocused").unwrap_or(0.0) != 0.0 || self.open_combo == Some(i) || self.focused.contains(&i);
        let fade_zero = self.widget_progress() * 255 / TRANS_FULL == 0;
        // the combo family builds its object in its draw (0x4C5937), the others in their bind
        let combo = !matches!(e.class.as_str(), "NFS5.GOTOBUTTON" | "NFS5.TABBUTTON" | "NFS5.GRAPHICTABBUTTON");
        let may = |f: &mut glints::Flourish| f.button_may_start(disabled, hot, sel, focus, fade_zero);
        if (1..=5).contains(&style) {
            self.glinty_flourish(i, if combo { None } else { Some(0) }, &may, style, out, aspect, x, y, gw as i32, gh as i32);
        }
    }

    /// `FeWidget::hoveredWithButtonDown`, **0x4C7CB0** — the test almost every decoration in the
    /// front end hangs off, and which these notes read wrong until §4.55.
    ///
    /// It is `flags & 0x04` (hover) **and** `0x5299C0`, which is one byte, `[0x69CAE8]`, written in
    /// exactly one place — `EventPump::poll` at 0x5297E5, with whether the button that just
    /// changed is now **down** — and read in exactly one place, 0x5299C1. So the test is
    /// *hovered while a mouse button is held*, not "hovered and the screen settled", and the last
    /// three conditions are the ordinary ones: not hidden (0x20), not disabled (0x10), and not
    /// disabled by its accessor (0x4E08E0 on `[+0x114]`).
    fn pressed_on(&self, i: usize, hovered: Option<usize>) -> bool {
        (hovered == Some(i) || self.capture == Some(i)) && self.pressed && !self.state_hidden_or_disabled(i)
    }

    /// The last two arms of 0x4C7CB0 — hidden (0x20) and disabled (0x10).
    ///
    /// **Both are read out of `[this+0x10C]`, the live flag word, not out of the file**: 0x4C7CC5
    /// loads that dword and 0x4C7CCB / 0x4C7CCF test it. A `.lay` block's `state=` is only how the
    /// word *starts*; 0x50D910 sets and clears the hidden bit by name at fifty-one call sites and
    /// 0x4C7E80 / 0x4C7EB0 do it directly (§4.112), and the port keeps both as
    /// [`Self::hidden_flags`] and [`Self::disabled_flags`]. Reading the attribute alone left a
    /// widget the screen had hidden at run time still pressable. *(The comment here used to say
    /// "nothing in this port disables a widget", which `disabled_flags` and `state_for`'s own
    /// first arm had already made false. Corrected 2026-09-21.)*
    ///
    /// Not covered: 0x4C7CD3's third way out — `[this+0x114]` set, the template carrying a
    /// `disabled` state (`[this+0x110] & 0x10`) and 0x4E08E0 agreeing — the accessor-driven
    /// disable, which is the same condition §4.110 reads at 0x4C831E.
    /// The state a widget is drawn in right now, by name — what `FEControl::onMouseEnter`
    /// 0x4C7BC3…0x4C7BFC compares with `"hover"`. `None` off this screen's own widgets.
    pub fn state_name(&self, i: usize, selected: Option<usize>) -> Option<String> {
        let e = self.layout.elements.get(i)?;
        Some(self.state_for(e, i, selected, self.hovered()).to_string())
    }

    /// 0x4C7E40 — the disabled bit (0x10) of the live flag word, or a `.lay` block that starts
    /// `state=disabled`.
    pub fn widget_disabled(&self, i: usize) -> bool {
        self.disabled_flags.contains(&i)
            || self.layout.elements.get(i).and_then(|e| e.get("state")).is_some_and(|s| s == "disabled")
            || self.covered_disables(i)
    }

    /// 0x4C7E4C…0x4C7E62, `isDisabled`'s third arm: the owning screen is covered and the
    /// widget's template has a `disabled` state.
    fn covered_disables(&self, i: usize) -> bool {
        self.covered
            && self.layout.elements.get(i).is_some_and(|e| self.controls.get(&e.class).is_some_and(|c| c.has_state("disabled")))
    }

    /// Cover or uncover this screen (`[+0x261]`, 0x4E08C0 / 0x4E08A0).
    pub fn set_covered(&mut self, on: bool) {
        self.covered = on;
    }

    fn state_hidden_or_disabled(&self, i: usize) -> bool {
        if self.is_hidden(i) || self.disabled_flags.contains(&i) {
            return true;
        }
        self.layout
            .elements
            .get(i)
            .and_then(|e| e.get("state"))
            .is_some_and(|s| s == "hidden" || s == "disabled")
    }

    /// The entry a `NFS5.BOTTOMBARBUTTON` shows — its tick, **0x4C9980** (§4.53).
    ///
    /// It keeps **three** six-field blocks, laid out `start, end, totalticks, delay, mindelay,
    /// maxdelay` and bound at 0x4C9778 onwards: the plain one at `[this+0x18C]`, the hover one at
    /// `[+0x1A4]` and the hovermousedown one at `[+0x1BC]`. The tick picks between them at
    /// 0x4C99C7: when `data.seperatehovermousedown` (`[+0x180]`) is set **and** 0x4C7CB0 says the
    /// widget is hovered and the screen settled, the **hovermousedown** block; otherwise, when the
    /// plain hover flag is up (0x4C7C90), the hover block; otherwise the plain one. All four
    /// shipped buttons carry `seperatehovermousedown=1`, so hovering shows the *hovermousedown*
    /// run and the `hover…` one is only reached while a screen is still moving.
    ///
    /// Within a block the machine is `NFS5.AnimatingPicture`'s, and a `totalticks` of 0 is forced
    /// to **1** at 0x4C97DF, which with `start == end` is simply a still frame — which is what
    /// three of the four are at rest.
    fn bar_button_frame(&self, e: &nfs_formats::lay::Element, hot: bool, down: bool) -> Option<usize> {
        let sep = self.attr_num(e, "seperatehovermousedown").unwrap_or(0.0) != 0.0;
        let p = match (hot, down && sep) {
            (_, true) => "hovermousedown",
            (true, false) => "hover",
            _ => "",
        };
        let key = |k: &str| e.int(&format!("{p}{k}")).unwrap_or(0).max(0);
        let (a, b) = (key("startframe"), key("endframe"));
        let n = e.int(&format!("{p}totalticks")).filter(|v| *v > 0).unwrap_or(1);
        let lo = key("mindelay");
        let hi = key("maxdelay").max(lo);
        let ticks = (self.clock_ms as u64 * 128 / 1000) as i64;
        let period = n + (lo + hi) / 2;
        let t = if period > 0 { ticks.rem_euclid(period) } else { 0 };
        Some((a + (t.min(n) * (b - a) / n)).clamp(a.min(b), a.max(b)) as usize)
    }

    /// The tooltip a widget with `data.helptext` puts up when the pointer rests on it —
    /// `FeWidget`'s tick, 0x4C7FE2…0x4C8226 (`RE/notes/controls.md` §4.52).
    ///
    /// ```text
    /// 0x4C7FE2  no `[this+0x128]` (the string id) -> nothing
    /// 0x4C7FEE  not hovered -> nothing, and 0x4C824D restamps `[this+0x12C]` with the tick
    /// 0x4C8000  hovered for fewer than 0x80 ticks — **one second** at 128 Hz -> nothing
    /// 0x4C8057  wrap `FEStrings[[+0x128]]` to **310.0f** in `Univers8b.ffn` (0x4C72B1)
    /// 0x4C808A  take the pointer from the mouse device (0x5372A0(6), x at +4 and y at +8)
    /// 0x4C80C7  x = pointer.x, pushed to 636 - width when x + width + 8 would pass 640
    /// 0x4C80EE  y = pointer.y - height - 4, floored at 0
    /// 0x4C8112  a = clamp((tick - [+0x12C] - 0x80) * 4, 0, 255) — half a second of fade-in
    /// 0x4C8193  a **white** panel, `width + 8` by `height + 4`, at `a · 0xC0 / 255`
    /// 0x4C81E0  the same box outlined black at `a`
    /// 0x4C8226  the wrapped text at (+4, +2), black at `a`
    /// ```
    fn tooltip(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, strings: &Strings) {
        let Some((mx, my)) = self.cursor else { return };
        let id = self.attr_num(e, "helptext").unwrap_or(0.0) as u32;
        if id == 0 {
            return;
        }
        // the port counts milliseconds where the original counts 128 Hz ticks
        let ticks = self.hover_ms() * 128 / 1000;
        if ticks < 0x80 {
            return;
        }
        let Some(text) = strings.get(id) else { return };
        let Some(f) = self.fonts.get(SMALL_FONT) else { return };
        let lines = wrap_lines(f, text, 310.0);
        if lines.is_empty() {
            return;
        }
        // 0x4C809A: 0x523260's `[+8]`, `_ftol` — the widest **measured** line, and only of the
        // lines that fit: 0x5237A3 raises it on the `<` branch alone, so a word too long for the
        // box (taken whole at 0x52379F) does not count
        let tw = lines.iter().map(|l| f.measure(l).0).filter(|w| *w < 310.0).fold(0.0f32, f32::max).trunc();
        // 0x4C80AF: 0x523250's total, `_ftol`; the lines step by the last one's pitch (0x523290)
        let (pitch, total) = wrap_metrics(f, &lines);
        let th = total.trunc();
        let x = if mx + tw + 8.0 > 640.0 { 636.0 - tw } else { mx };
        let y = (my - th - 4.0).max(0.0);
        let a = (((ticks - 0x80) * 4).min(255) as f32) / 255.0;
        out.push(self.quad(aspect, x, y, tw + 8.0, th + 4.0, [1.0, 1.0, 1.0, a * 192.0 / 255.0]));
        for (bx, by, bw, bh) in [(x, y, tw + 8.0, 1.0), (x, y + th + 3.0, tw + 8.0, 1.0), (x, y, 1.0, th + 4.0), (x + tw + 7.0, y, 1.0, th + 4.0)] {
            out.push(self.quad(aspect, bx, by, bw.max(1.0), bh.max(1.0), [0.0, 0.0, 0.0, a]));
        }
        let mut ly = y + 2.0;
        for l in &lines {
            self.text(out, aspect, f, l, x + 4.0, ly, 0.0, 0, [0.0, 0.0, 0.0, a]);
            ly += pitch;
        }
    }

    /// How long the pointer has rested where it is. A still parks the pointer with
    /// `NFS_FE_CURSOR`, and then the screen clock is the rest — so `NFS_FE_CLOCK=1500` with a
    /// parked pointer shows the tooltip half way through its fade.
    fn hover_ms(&self) -> u32 {
        if std::env::var("NFS_FE_CURSOR").is_ok() { self.clock_ms } else { self.hover_ms }
    }

    /// The class of the widget at the flattened index, following a group the same way the hit
    /// tests do.
    pub fn class_of(&self, index: usize) -> Option<String> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.class_of(j);
        }
        self.layout.elements.get(index).map(|e| e.class.clone())
    }

    /// `NFS5.PLAYERDRIVERPICTURE`'s slot 9, **0x4CF940** — the release on one of
    /// `createplayer.lay`'s eight pictures answers its `data.ournumber` (`[this+0x140]`, bound by
    /// slot 20 0x4CF7A0 through 0x4C8B00), which the caller stores in `[0x5D8F2C]`. No sound, no
    /// other state; the slot returns 1. `None` for any other widget.
    pub fn driver_picture_of(&self, index: usize) -> Option<i32> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.driver_picture_of(j);
        }
        let e = self.layout.elements.get(index)?;
        (e.class == "NFS5.PLAYERDRIVERPICTURE").then(|| self.attr_num(e, "ournumber").unwrap_or(0.0) as i32)
    }

    /// `NFS5.CAMERASELECT`'s slot 9, **0x4BF5F0** — the release hands `Camera.Current` (`[this+
    /// 0x140]`, bound at 0x4BF2DB) its `SetSelection`, slot 0x2C, with the widget's own
    /// `data.accessornum` (`[this+0x14C]`, 0x4BF2C7), and answers 1. No hit test, no sound. The
    /// number, for any other widget `None`.
    pub fn camera_select_of(&self, index: usize) -> Option<i32> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.camera_select_of(j);
        }
        let e = self.layout.elements.get(index)?;
        (e.class == "NFS5.CAMERASELECT").then(|| self.attr_num(e, "accessornum").unwrap_or(0.0) as i32)
    }

    /// `NFS5.CAMERAPICTURE`'s `data.picturenum` (`[this+0x144]`, bound at 0x4BF730) — what its slot 9
    /// **0x4BF920** hands `Camera.<Camera.Current + 1>`'s `SetSelection` when the release is the
    /// first of its widget for half a second. The number, for any other widget `None`.
    pub fn camera_picture_of(&self, index: usize) -> Option<i32> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.camera_picture_of(j);
        }
        let e = self.layout.elements.get(index)?;
        (e.class == "NFS5.CAMERAPICTURE").then(|| self.attr_num(e, "picturenum").unwrap_or(0.0) as i32)
    }

    /// An activate on an input box takes focus, as `FeInputBox`'s slot 9 does:
    ///
    /// ```text
    /// 0x4CCB73  if (!isFocused())            ; 0x4C7DD0, [this+0x10C] & 0x40
    /// 0x4CCB7E      setFocused()             ; 0x4C7D70 — only when [this+0x110] & 0x40 allows it
    /// 0x4CCB85      [this+0x148] = 1         ; the editing latch
    /// 0x4CCB9B  else if (![this+0x148]) clearFocused()   ; 0x4C7DA0
    /// ```
    ///
    /// so the first click focuses the box and it stays focused: the latch is 1 from then on, and
    /// the `else` arm cannot fire. `NFS5.CHATINPUTBOX` shares that slot, and its own bind
    /// 0x4C32D3 starts by calling the input box's 0x4CC430, so `data.alwaysfocused` reaches it
    /// too (§4.109).
    /// Every class on this screen with the parts its template declares that **no state lists**.
    ///
    /// The part walk can never reach one, so only the class's own draw can put it down — and
    /// `NFS5.CAMERAPICTURE`'s `shp` (§4.129) is exactly that: a part the bind looks up by name.
    /// A measurement axis for the same kind of gap: a part here that the port draws nowhere is a
    /// thing the original puts on the screen and this port does not.
    pub fn unlisted_parts(&self) -> Vec<(String, Vec<String>)> {
        let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for e in &self.layout.elements {
            let Some(c) = self.controls.get(&e.class) else { continue };
            let used: std::collections::BTreeSet<&str> = c.states.values().flatten().map(String::as_str).collect();
            let dead: Vec<String> = c.parts_order.iter().filter(|p| p.as_str() != "data" && !used.contains(p.as_str())).cloned().collect();
            if !dead.is_empty() {
                out.insert(e.class.clone(), dead);
            }
        }
        for (_, g) in self.group_spans() {
            out.extend(g.unlisted_parts());
        }
        out.into_iter().collect()
    }

    /// Every class on this screen with the states its `.ctr` template declares — the very set
    /// `FeWidget`'s slot 12 (0x4C7760) turns into `[widget+0x110]`, and therefore the complete
    /// list of looks the original can ever put a widget of that class into (§4.109, §4.110).
    /// A measurement axis: a state a template declares and this port never returns is a look the
    /// original has and the port does not, and no blank count can see it.
    pub fn declared_states(&self) -> Vec<(String, Vec<String>)> {
        let mut out: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for e in &self.layout.elements {
            if let Some(c) = self.controls.get(&e.class) {
                let mut v: Vec<String> = c.states.keys().cloned().collect();
                v.sort();
                out.insert(e.class.clone(), v);
            }
        }
        for (_, g) in self.group_spans() {
            out.extend(g.declared_states());
        }
        out.into_iter().collect()
    }

    /// The flattened indices of the widgets whose `.lay` `name` is one of `names`, with the
    /// position of the name that matched — what 0x50D9A0's `FeScreen::findWidget` does.
    pub fn widgets_named(&self, names: &[&str]) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let n = e.get("name")?;
                names.iter().position(|w| w.eq_ignore_ascii_case(n)).map(|k| (i, k))
            })
            .collect();
        for (base, g) in self.group_spans() {
            out.extend(g.widgets_named(names).into_iter().map(|(i, k)| (i + base, k)));
        }
        out
    }

}

/// `FEChronicleMainBoxControl`'s grid, from its slot 8 — **0x508020**:
///
/// ```text
/// 0x50802D  p = the widget's position (0x525310)
/// 0x508042  [this+0x154] = (ev.x - p.x) / 91          ; 0xB40B40B5, add, sar 6
/// 0x508055  [this+0x150] = (ev.y - p.y - 0x26) / 61   ; 0x4325C53F, sar 4
/// 0x50807A  [this+0x14C] = 0 <= col < 5 && 0 <= row < 3
/// ```
///
/// The two magic constants are emulated rather than guessed: 91 and 61 are the only divisors that
/// reproduce them over the whole range. Slot 7 (0x508010) clears the flag when the pointer leaves,
/// and slot 9 (0x5080C0) is the click — **row 0 goes to `chroniclepicture` and row 1 to
/// `chroniclemedia`**, each through `0x5074A0(row, [this+0x160 + 4*col], [this+0x174 + 4*col])`.
/// So the row is the kind and the column the year-and-variant. `RE/notes/controls.md` §4.120.
pub fn chronicle_cell(px: f32, py: f32, x: f32, y: f32) -> Option<(usize, usize)> {
    let (dx, dy) = (px - x, py - y - 0x26 as f32);
    if dx < 0.0 || dy < 0.0 {
        return None;
    }
    let (col, row) = ((dx as i32 / 91) as usize, (dy as i32 / 61) as usize);
    (col < 5 && row < 3).then_some((col, row))
}

/// The chronicle slider's geometry, the numbers `FEHorizontalSlider`'s helper works with
/// (0x4D0B90, 0x4D0CC0, 0x4D0F20): the track (`size=342,21`, the `track` RECT part) and the knob
/// (`weeicons.fsh` 38, its own size), both at the widget's position — the knob's base offset is the
/// template's `knob.pos.value` 0,0 (`[helper+0x114]`, 0x4D08BA).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChronicleSliderGeom {
    /// the widget's index in the layout
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub knob_w: f32,
    pub knob_h: f32,
}

impl ChronicleSliderGeom {
    /// The knob's left edge for accessor index `idx` of `count` — 0x4D0C0C…0x4D0C68:
    /// `idx * (track - knob) / (count - 1)`, integer, `0` for a single entry.
    pub fn knob_left(&self, idx: i32, count: i32) -> f32 {
        let dx = if count > 1 { idx * (self.w - self.knob_w).max(0.0) as i32 / (count - 1) } else { 0 };
        self.x + dx as f32
    }

    /// Whether `(px, py)` is on the knob part's rectangle (0x4E2970: left/top inclusive,
    /// right/bottom exclusive).
    pub fn on_knob(&self, px: f32, py: f32, idx: i32, count: i32) -> bool {
        let kx = self.knob_left(idx, count);
        px >= kx && px < kx + self.knob_w && py >= self.y && py < self.y + self.knob_h
    }

    /// Whether `(px, py)` is on the `track` part's rectangle.
    pub fn on_track(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// The accessor index a drag has reached — 0x4D0F20: `(mx - anchor) * (count - 1) / (track -
    /// knob) + start`, `idiv` truncating, before the accessor's setter (0x4BC240) clamps it.
    pub fn dragged_index(&self, mx: f32, anchor: i32, start: i32, count: i32) -> i32 {
        let span = (self.w - self.knob_w) as i32;
        if span == 0 {
            return start;
        }
        (mx as i32 - anchor) * (count - 1) / span + start
    }
}

#[cfg(test)]
mod chronicle_slider_tests {
    use super::ChronicleSliderGeom;

    fn geom() -> ChronicleSliderGeom {
        // chronicle.lay: pos 150,350 size 342,21; the knob is `weeicons.fsh` 38, 45 wide, 11 high
        ChronicleSliderGeom { index: 0, x: 150.0, y: 350.0, w: 342.0, h: 21.0, knob_w: 45.0, knob_h: 11.0 }
    }

    #[test]
    fn the_knob_sits_at_index_times_travel_over_count() {
        let g = geom();
        // 0x4D0C0C…0x4D0C68: idx * (342 - 45) / (count - 1), integer
        assert_eq!(g.knob_left(0, 49), 150.0);
        assert_eq!(g.knob_left(48, 49), 150.0 + 297.0);
        assert_eq!(g.knob_left(12, 49), 150.0 + (12 * 297 / 48) as f32);
        assert_eq!(g.knob_left(5, 1), 150.0);
    }

    #[test]
    fn a_drag_is_the_inverse_and_truncates_toward_zero() {
        let g = geom();
        // 0x4D0F20: (x - anchor) * (count - 1) / (track - knob) + start
        assert_eq!(g.dragged_index(232.0, 172, 0, 49), 60 * 48 / 297);
        assert_eq!(g.dragged_index(172.0, 232, 9, 49), (-60 * 48 / 297) + 9);
        assert_eq!(g.dragged_index(200.0, 200, 7, 49), 7);
    }

    #[test]
    fn regions_are_the_knob_and_the_track() {
        let g = geom();
        assert!(g.on_knob(150.0, 350.0, 0, 49));
        assert!(!g.on_knob(195.0, 350.0, 0, 49), "right edge exclusive");
        assert!(!g.on_knob(160.0, 361.0, 0, 49), "below the knob's 11 rows");
        assert!(g.on_track(160.0, 361.0));
        assert!(!g.on_track(160.0, 371.0));
    }
}

impl LayoutScreen {
    /// This screen's `NFS5.CHRONICLESLIDER`, if it has one.
    pub fn chronicle_slider(&self) -> Option<ChronicleSliderGeom> {
        let index = self.layout.elements.iter().position(|e| e.class == "NFS5.CHRONICLESLIDER")?;
        let e = &self.layout.elements[index];
        let (x, y) = self.position_of(e);
        let (w, h) = e.pair("size")?;
        let (knob_w, knob_h) = self
            .shape_parts_at(e, "normal", None)
            .into_iter()
            .find_map(|(_, _, file, i, _)| self.shapes.get(&(file, i)).map(|&(_, dw, dh)| (dw, dh)))?;
        Some(ChronicleSliderGeom { index, x, y, w, h, knob_w, knob_h })
    }

    /// Which timeline cell the pointer is on, against this screen's own
    /// `NFS5.ChronicleMainBoxControl` (§4.120).
    pub fn chronicle_cell_at(&self, px: f32, py: f32) -> Option<(usize, usize)> {
        let i = self.layout.elements.iter().position(|e| e.class == "NFS5.ChronicleMainBoxControl")?;
        let (x, y) = self.position_of(&self.layout.elements[i]);
        chronicle_cell(px, py, x, y)
    }
}

impl LayoutScreen {
    /// The state whose part list a draw ending in **0x525830** walks (`[[this+0xD4]] +
    /// 0x525570()·4`, 0x525876…0x525886): [`state_for`](Self::state_for) when the template
    /// carries that state. Settled 2026-09-28: a name the template lacks (the port's keyboard
    /// `selected` on a class with no `[state.selected]`) leaves the list as it was — 0x5271F0
    /// finds no entry and copies nothing (0x527214, confirmed by reading it: the "not found" arm
    /// jumps straight past the `memmove` at 0x527216…0x527263) — so the object goes on drawing
    /// whichever state it last *did* find an entry for, however many ticks ago that was, not the
    /// `.lay` block's static `state=`. `last_drawn_state` is that per-widget memory; before the
    /// first successful match it falls back to the `.lay` block's `state=`, since that is what a
    /// freshly-bound widget's part list holds before any draw has run.
    fn drawn_state(&self, e: &nfs_formats::lay::Element, i: usize, selected: Option<usize>, hovered: Option<usize>) -> String {
        let live = self.state_for(e, i, selected, hovered);
        match self.controls.get(&e.class) {
            Some(c) if c.has_state(live) => {
                self.last_drawn_state.borrow_mut().insert(i, live.to_string());
                live.to_string()
            }
            _ => self
                .last_drawn_state
                .borrow()
                .get(&i)
                .cloned()
                .unwrap_or_else(|| e.get("state").unwrap_or("normal").to_string()),
        }
    }

    /// `focused` is reached here by holding the button down over a widget whose class takes focus
    /// on the press (§4.40, `can_press`) — for `NFS5.CARSETTINGBUTTON` that is `picseldown` at
    /// `data.seldownindex`, the pressed art. `focusedhover` needs `[+0x110] & 0x80`, which no
    /// shipped template of those three sets, so the cascade falls to `focused`.
    fn state_for<'a>(&self, e: &'a nfs_formats::lay::Element, i: usize, selected: Option<usize>, hovered: Option<usize>) -> &'a str {
        // a widget that has the mouse keeps its pressed look wherever the pointer goes (§4.57)
        let hot = hovered == Some(i) || self.capture == Some(i);
        // 0x4C831E comes second, after `hidden` and before everything else (§4.110), and a
        // disabled widget shows `[state.disabled]` whatever the pointer is doing
        // a covered screen (§4.147) disables it the same way — but `hidden` is tested first, so a
        // widget the screen has hidden stays hidden under a dialog
        if (self.disabled_flags.contains(&i) || (self.covered && !self.is_hidden(i)))
            && self.controls.get(&e.class).is_some_and(|c| c.has_state("disabled"))
        {
            return "disabled";
        }
        // `data.alwaysfocused` (read at 0x4CC537 into `[this+0x156]`): the input box's bind calls
        // `setFocused` outright when it is set (0x4CC56E…0x4CC570), so the box is focused with no
        // pointer involved at all. Two shipped screens use it, `accountlogin` and `createplayer`.
        if self.attr_num(e, "alwaysfocused").unwrap_or(0.0) != 0.0 || self.open_combo == Some(i) || self.focused.contains(&i) {
            // 0x4C835E comes before 0x4C839A: a focused widget that is also hovered takes
            // `focusedhover` when its template has one (§4.110)
            if hot && self.controls.get(&e.class).is_some_and(|c| c.has_state("focusedhover")) {
                return "focusedhover";
            }
            return "focused";
        }
        // **`NFS5.CAMERASELECT` highlights the camera that is on.** Its bind takes three
        // accessors, not one: `data.accessornum` into `[this+0x14C]` (0x4BF2C7), `Camera.Current`
        // into `[+0x140]` (0x4BF2DB) and `Camera.<accessornum + 1>` into `[+0x144]` — the four
        // arms at 0x4BF2F8, 0x4BF32C, 0x4BF35F and 0x4BF392. Its draw 0x4BF410 then asks the
        // first of those for its value and compares:
        //
        // ```text
        // 0x4BF428  v = [this+0x140]->slot10()
        // 0x4BF42B  v == [this+0x14C] -> setState("highlighty")   (0x4BF433, 0x4C7970)
        // 0x4BF441  else the ordinary state
        // ```
        //
        // and `CameraSelect.ctr`'s `[state.highlighty]` swaps `bg` for `hbg`. The four shipped
        // widgets are `cameras.lay`'s, carrying `accessornum` 0…3 and `text` 1…4. §4.89.
        if e.class == "NFS5.CAMERASELECT"
            && self.attr_num(e, "accessornum").unwrap_or(-1.0) == self.camera_current as f64
            && self.controls.get(&e.class).is_some_and(|c| c.has_state("highlighty"))
        {
            return "highlighty";
        }
        // **`NFS5.CAMERAPICTURE`'s draw 0x4BF870**: `[this+0x144]` (`data.picturenum`) is compared
        // with each of `Camera.1 … Camera.4` (the loop 0x4BF883…0x4BF89D over `[this+0x154 … 0x160]`,
        // slot 10) and, when any one holds it and the widget is not focused (0x4BF8A5 `0x4C7DD0`,
        // `[+0x10C] & 0x40` — set only by 0x4C7D70 for a widget whose `[+0x110]` has 0x40, which
        // the base ctor 0x4C7480 clears and the base bind 0x4C7530 never sets), the state is
        // `highlighty` (0x4BF8AE, `0x4C7970` at 0x4BF8B5); otherwise the ordinary one (0x4BF8BE
        // `0x4C7B70`, 0x4BF8C9 `0x4C7A00`)
        if e.class == "NFS5.CAMERAPICTURE"
            && self.camera_list.iter().any(|&v| f64::from(v) == self.attr_num(e, "picturenum").unwrap_or(-1.0))
            && self.controls.get(&e.class).is_some_and(|c| c.has_state("highlighty"))
        {
            return "highlighty";
        }
        if hot && self.pressed && self.can_press(&e.class) {
            if self.controls.get(&e.class).is_some_and(|c| c.has_state("focusedhover")) {
                return "focusedhover";
            }
            return "focused";
        }
        // 0x4C83C1 and 0x4C83EE: the `selected` flag, `[this+0x10C] & 8`. The port's own
        // keyboard cursor stands in for it where nothing else sets it (§4.110).
        if selected == Some(i) || self.selected_flags.contains(&i) {
            if hot && self.controls.get(&e.class).is_some_and(|c| c.has_state("selectedhover")) {
                return "selectedhover";
            }
            return "selected";
        }
        if hot && self.can_hover(&e.class) {
            return "hover";
        }
        match e.get("state").unwrap_or("normal") {
            "selected" if self.unselected.contains(&i) => "normal",
            s => s,
        }
    }

    /// A release on a tab button — `FERadioItem`'s slot 9, **0x4D0290**: the widget's window's
    /// own parent (`[[this+0xD0]+0xD0]`) cast to `FERadioGroup` (0x5A134B with 0x5D9030) and, when
    /// it is one, its slot 29 **0x4CFF30** with this button; then sound 5 (0x4AADB0) whatever the
    /// cast gave — the caller plays it. Answers whether `index` is a tab button.
    ///
    /// 0x4CFF30: the button already current (`[group+0x154]`) does nothing; otherwise the current
    /// one's slot 28 **0x4D0310** hides its panel (0x4CEC50) and the button becomes the pending one
    /// `[group+0x158]`, which the group's tick **0x4CFEB0** makes current — its slot 27 **0x4D02E0**:
    /// the `selected` flag (0x4C7D00), the panel's load (+0x6C, 0x4CEA90) and show (0x4CEC60) —
    /// once the old button is no longer selected and the panel's screen is neither going out
    /// (0x4E0E00 ≠ 3) nor closed (0x4E0E10) — `tick_panels`. So the old panel goes out and only
    /// then does the new one come in. *(Until 2026-09-26 the switch was made at once, an
    /// APPROXIMATION.)* A button's `panel` names a `NFS5.PANEL` of the screen that holds the strip
    /// (0x4D01F0 looks it up three windows up).
    pub fn tab_click(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize) -> bool {
        let Some((k, j)) = self.route(index) else { return false };
        let is_tab = |c: &str| c == "NFS5.TABBUTTON" || c == "NFS5.GRAPHICTABBUTTON";
        let strip_here = self.layout.elements.get(k).is_some_and(|e| e.class == "NFS5.RADIOGROUP")
            && self.groups.get(&k).is_some_and(|g| g.layout.elements.get(j).is_some_and(|e| is_tab(&e.class)));
        if !strip_here {
            // deeper: a strip inside a panel this screen holds
            return match self.groups.get_mut(&k) {
                Some(g) => g.tab_click(r, paths, j),
                None => false,
            };
        }
        let (old, new_panel, old_panel) = {
            let g = &self.groups[&k];
            let current = g.layout.elements.iter().enumerate().find(|(n, e)| {
                is_tab(&e.class) && (g.selected_flags.contains(n) || (e.get("state") == Some("selected") && !g.unselected.contains(n)))
            });
            let old = current.map(|(n, _)| n);
            if old == Some(j) {
                return true;
            }
            let panel_of = |n: usize| g.layout.elements.get(n).and_then(|e| e.get("panel")).filter(|p| !p.is_empty()).map(str::to_string);
            (old, panel_of(j), old.and_then(panel_of))
        };
        // 0x4CFF30: the current button's slot 28, 0x4D0310, hides its panel (0x4CEC50, the
        // sub-screen's +0x7C: out), and the button becomes the pending one
        if let Some(i) = old_panel.and_then(|p| self.panel_index(&p))
            && !self.closed.contains(&i)
            && let Some(p) = self.groups.get_mut(&i)
        {
            p.transition_out();
        }
        self.pending_tab = Some((k, j));
        // the new panel's layout, loaded now so the strip's tick can open it (0x4CEA90)
        if let Some(p) = new_panel
            && let Some(i) = self.panel_index(&p)
            && !self.groups.contains_key(&i)
        {
            self.set_panel(r, paths, &p, true);
            self.closed.insert(i);
        }
        let _ = old;
        true
    }

    /// Put the pointer somewhere, in design coordinates, or take it away.
    ///
    /// `RE/notes/controls.md` §4.39: the front end is mouse-driven, and hover is a **flag** —
    /// bit **0x04** of `[widget+0x10C]`, set by the base vtable's slot 6 (0x4C7B90) and cleared by
    /// slot 7 (0x4C7C50), each of which then re-runs `updateState` (0x4C82A0) to turn the flags
    /// into the state name the widget draws.
    /// Tooling: move the pointer for a scripted click (`NFS_FE_CLICKS`), even while
    /// `NFS_FE_CURSOR` parks the live one.
    pub fn script_cursor(&mut self, at: Option<(f32, f32)>) {
        self.pointer = None;
        self.place_cursor(at);
    }

    pub fn set_cursor(&mut self, at: Option<(f32, f32)>) {
        // a still parked its own pointer with `NFS_FE_CURSOR`; the live one must not clobber it
        if std::env::var("NFS_FE_CURSOR").is_ok() {
            return;
        }
        self.pointer = None;
        self.place_cursor(at);
    }

    /// Move only the cursor sprite: the widgets keep the pointer they last saw. What the front end
    /// does while 0x4D2850 swallows its input (a screen in transition, the room busy): the move
    /// events never reach the windows, whose hover `[+0x110]` / `[+0x114]` stay put (0x5289D9),
    /// while the cursor draw reads the device (0x4D32DB).
    pub fn set_pointer(&mut self, at: Option<(f32, f32)>) {
        if std::env::var("NFS_FE_CURSOR").is_ok() {
            return;
        }
        self.pointer = Some(at);
    }

    /// 0x4E0DE0 — `[+0x12C]` is 1 or 3: this screen, or the background it carries (`[fe+0xE4]`
    /// in the original, which the port keeps per screen), is sliding in or out.
    pub fn in_transition(&self) -> bool {
        let moving = |t: Transition| matches!(t, Transition::In { .. } | Transition::Out { .. });
        moving(self.transition) || self.background.as_ref().is_some_and(|b| moving(b.transition))
    }

    /// Hand every group the pointer in **its own** coordinates. `embedded` draws a panel's
    /// sub-layout offset by the panel's position, because 0x4CEBC0 hands the loaded screen the
    /// panel's place (§4.12) — so the hit test inside it has to move the same way.
    fn place_cursor(&mut self, at: Option<(f32, f32)>) {
        self.cursor = at;
        let offsets: Vec<(usize, (f32, f32))> = self
            .groups
            .keys()
            .copied()
            .filter_map(|k| self.layout.elements.get(k).map(|e| (k, self.position_of(e))))
            .collect();
        for (k, (gx, gy)) in offsets {
            let inner = at.map(|(x, y)| (x - gx, y - gy));
            if let Some(g) = self.groups.get_mut(&k) {
                g.place_cursor(inner);
            }
        }
    }

    /// Whether this class can hover at all: 0x4C7B90 sets the bit only when `[this+0x110] & 4`,
    /// and that mask is built from the template's own state list (0x4C7784, 0x4C7800) — so it is
    /// exactly "the `.ctr` has a `[state.hover]`". Checked against the classes whose draws gate
    /// something on hover: all seven have one, and `BevelOptionBox.ctr`, whose bevel is *not*
    /// gated, has none.
    fn can_hover(&self, class: &str) -> bool {
        self.controls.get(class).is_some_and(|c| c.has_state("hover"))
    }

    /// Hold the left button down, or let it up.
    ///
    /// `RE/notes/controls.md` §4.40: `EventPump::poll` 0x529700 stamps a `MouseButtonEvent` with
    /// type **4** when the button goes down and **8** when it comes up (0x529806), and
    /// `Control::handleEvent` 0x525F80 sends them to **slot 5** and **slot 9**.
    pub fn set_pressed(&mut self, down: bool) {
        if std::env::var("NFS_FE_PRESS").is_ok() {
            return;
        }
        self.pressed = down;
        // §4.57: a widget that takes focus takes the mouse with it (0x4C8486 → 0x528CB0), and
        // `Window::handleEvent`'s button arm then hands it every button event wherever the pointer
        // is (0x528AD5). The classes that take focus on the press are §4.40's three.
        self.capture = if down {
            self.hovered().filter(|i| self.layout.elements.get(*i).is_some_and(|e| self.can_press(&e.class)))
        } else {
            None
        };
        for g in self.groups.values_mut() {
            g.set_pressed(down);
        }
    }

    /// The classes that take **focus on the press** — the ones whose slot 5 calls
    /// `FeWidget::setFocused` 0x4C7D70, which is what makes the held-down art appear:
    /// `NFS5.CARSETTINGBUTTON` at 0x4C1F6C, `NFS5.DEADZONE` at 0x51BB83 and
    /// `NFS5.GouraudIndicatorBar` at 0x4CA556. The other three callers
    /// (`NFS5.CAMERAPICTURE` 0x4BF9BF, `NFS5.3DCAR` 0x4C1002, `NFS5.CONTROLLERSETUP` 0x51C5F0)
    /// take it in **slot 9**, on the release, and keep it — that is a selection, not a press, and
    /// the port does not raise it.
    const PRESS_FOCUS: [&'static str; 3] = ["NFS5.CARSETTINGBUTTON", "NFS5.DEADZONE", "NFS5.GouraudIndicatorBar"];

    /// Whether this widget shows a held-down state: its class must set focus on the press, and
    /// `setFocused` only sets the bit when `[this+0x110] & 0x40`, the mask bit the template's own
    /// `[state.focused]` puts there.
    fn can_press(&self, class: &str) -> bool {
        Self::PRESS_FOCUS.contains(&class) && self.controls.get(class).is_some_and(|c| c.has_state("focused"))
    }

    /// The widget the pointer is over — the **first** section whose box contains it and whose
    /// class can hover, **unless** an open combo has the window's capture, in which case only the
    /// combo itself can ever hover.
    ///
    /// `Window::handleEvent`'s move arm **0x5289D9** (§4.56) is the enter/leave machine, and it is
    /// capture-aware in a way this file had not read until it was reported: after the raw hit
    /// test (`0x528C00`) it does not always send the **enter** (type 5) to what it found —
    ///
    /// ```text
    /// 0x528a3a  [this+0x110] = hit                       ; tracked regardless, for the next leave
    /// 0x528a38  if (captured == 0)                 -> send enter to hit
    /// 0x528a46      al = captured->slot18()             ; "accepts mouse input" (§4.38)
    /// 0x528a49      if (!al)                       -> send enter to hit
    /// 0x528a4d      if (captured == hit)           -> send enter to hit (to itself)
    /// 0x528a4f      else                           -> **no enter sent at all**
    /// ```
    ///
    /// (read directly from the image, `fn.py --asm 0x5289d9`, since the prose notes stopped short
    /// of the exact branch). `[window+0x120]` is the **captured** widget — the one holding focus,
    /// which a widget takes and keeps for as long as it is focused (§4.57's `setCapture` /
    /// `releaseCapture`) — and **slot 18** is *accepts mouse input*: the base answers false
    /// (0x50DD60), `NFS5.DEADZONE` and `NFS5.ComboBoxSlider`/`NFS5.ColorBox` answer true (0x510E90,
    /// §4.38). A combo's list is open exactly while it holds focus (§4.43), so **while any combo
    /// is open, it holds the window's capture and its slot 18 is true — and no other widget's
    /// hover flag is ever set**, anywhere on the screen, until it closes. The combo's own internal
    /// row highlight (§4.42, `combo_row_at`) is unrelated machinery and keeps working underneath
    /// this — only the *window-level* `[state.hover]` this function drives is gated.
    ///
    /// *(User report, 2026-09-30 (c): hovering a row of an open drop-down also hovered whatever was
    /// underneath it, including through a tab panel's own drop-down — the raw scan below is
    /// unchanged and still finds that widget, so the fix is the capture filter `hovered` applies
    /// to its answer.)*
    pub fn hovered(&self) -> Option<usize> {
        let hit = self.hovered_scan();
        match self.any_open_combo() {
            // the combo itself is exempt (0x528a4d: captured == hit still sends the enter) —
            // harmless either way, since ComboBox.ctr's `[state.hover]` draws exactly what
            // `[state.normal]` does (§4.4, item (d) below)
            Some(oc) => hit.filter(|h| *h == oc),
            None => hit,
        }
    }

    /// The combo whose drop-down is open, here or in a visible embedded screen, by its flattened
    /// index — `None` while nothing is open. The same routing `combo_row_at` and
    /// `combo_scroll_at` use for a nested panel's own combo.
    fn any_open_combo(&self) -> Option<usize> {
        if let Some(i) = self.open_combo {
            return Some(i);
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            if !self.closed.contains(&k)
                && let Some(j) = self.groups[&k].any_open_combo()
            {
                return Some(base + j);
            }
            base += self.groups[&k].span();
        }
        None
    }

    /// `hovered`'s own raw scan, before the capture filter above — the **first** section whose box
    /// contains it and whose class can hover.
    ///
    /// `Window::hitTest` **0x528C00** walks the child vector forwards and returns the **first**
    /// child that contains the point, and that vector is in section order (§4.67) with the draw
    /// walking it backwards — so the first section is both the topmost and the first tested.
    ///
    /// The box is the widget's own, the same `0x4C8670` gives the original. A widget with no size
    /// of its own has a box of (0, 0) here (§4.37) and so can never be hit; those are the 27
    /// bottom-bar buttons and graphic tabs, whose classes carry no hover state anyway.
    fn hovered_scan(&self) -> Option<usize> {
        let (cx, cy) = self.cursor?;
        // the bases the flattened index space gives each embedded screen, by the element the
        // panel sits at, so a sub-layout is tested exactly where the draw reaches it
        let bases: std::collections::HashMap<usize, usize> = {
            let mut keys: Vec<usize> = self.groups.keys().copied().collect();
            keys.sort_unstable();
            let mut base = self.layout.elements.len();
            let mut m = std::collections::HashMap::new();
            for k in keys {
                m.insert(k, base);
                base += self.groups[&k].span();
            }
            m
        };
        for (i, e) in self.layout.elements.iter().enumerate().skip(1) {
            // a panel draws a whole screen of its own here, at the panel's own depth
            if let Some(base) = bases.get(&i) {
                // a hidden panel takes no pointer either
                if !self.closed.contains(&i)
                    && let Some(j) = self.groups[&i].hovered()
                {
                    return Some(base + j);
                }
                continue;
            }
            // The window's hit test (0x528C00) asks every child's containsPoint (0x525A90) and
            // knows nothing of a hover state; `can_hover` stands in for "a widget that reacts to
            // the pointer" and is right for the buttons. The three text-entry classes react to a
            // click without ever having a `[state.hover]` (their `.ctr` states are `normal,
            // hidden, focused[, disabled]`): the release goes to their slot 9 (0x4CCB70,
            // 0x4C2220) which takes the focus. Without them here a click on `Comms.IP` (or the
            // chat line) never reached the box and nothing could be typed into it.
            let takes_click = matches!(e.class.as_str(), "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHARVECTACCESSORINPUTBOX");
            if !(self.can_hover(&e.class) || takes_click) || self.state_draws_nothing(e) {
                continue;
            }
            // **A hidden widget contributes no box** (`[this+0x10C] & 0x20`, the bit 0x50D910's helper
            // sets through 0x4C7E80): slot 13 (0x4C86F0) writes four zeros for it, and 0x525A90 fails on
            // an empty box (`left == right`, 0x525AAF). So the garage button `ID_MERGEDLISTCARSETUP`
            // hidden under the paint box `ID_MERGEDLISTCOLOR` (same `pos`, and the earlier section, so
            // the first the hit test tries) takes no pointer — until 2026-09-30 a click on the colour box
            // fell through to it and opened `garage`.
            if self.hidden_flags.contains(&i) {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_hit(e);
            // `Control::containsPoint` 0x525A90 takes the widget's box (slot 13 → 0x4C8670) and
            // fails only on `x < left`, `x > right`, `y < top` or `y > bottom` — so both edges are
            // **inclusive** (§4.56), where this port used to be half-open.
            if w >= 1.0 && h >= 1.0 && cx >= x && cx <= x + w && cy >= y && cy <= y + h && self.tab_slant(e, cx, cy) {
                return Some(i);
            }
        }
        None
    }

    /// `NFS5.GRAPHICTABBUTTON`'s own hit test, **0x4CABA0** — the one class whose clickable area is
    /// not its box. Its bind takes four ints (0x4CAB40): `data.TLoffsetx` into `[this+0x15C]`,
    /// `TRoffsetx` into `[+0x160]`, `BLoffsetx` into `[+0x164]` and `BRoffsetx` into `[+0x168]`,
    /// and the test slants each vertical edge between the top value and the bottom one:
    ///
    /// ```text
    /// 0x4CABCD  y outside the box -> false
    /// 0x4CABDE  dx = TL ? (h - dy)·TL / h : BL·dy / h        ; the left edge
    /// 0x4CAC09  x <  box.l + dx -> false
    /// 0x4CAC19  dx = TR ? (h - dy)·TR / h : BR·dy / h        ; the right edge
    /// 0x4CAC3C  x >  box.r + dx -> false
    /// ```
    ///
    /// so each side is a straight line from `top ± T` to `bottom ± B`, and a zero at the top means
    /// "take the bottom one" rather than "no slant". All five shipped buttons are on
    /// `optiontabs.lay` and they carry the pairs (0, −60, 94, 0), (0, −20, 60, 0), (0, 0, 20, −20),
    /// (20, 0, 0, −60) and (60, 0, 0, −94) — a strip of parallelograms whose rectangles overlap,
    /// which is why the box alone picks the wrong tab. §4.82.
    fn tab_slant(&self, e: &nfs_formats::lay::Element, cx: f32, cy: f32) -> bool {
        if e.class != "NFS5.GRAPHICTABBUTTON" {
            return true;
        }
        let (x, y) = self.position_of(e);
        let (w, h) = self.box_hit(e);
        let (h, dy) = (h as i64, (cy - y) as i64);
        if h <= 0 {
            return true;
        }
        let at = |top: &str, bottom: &str| {
            let t = self.attr_num(e, top).unwrap_or(0.0) as i64;
            if t != 0 { (h - dy) * t / h } else { self.attr_num(e, bottom).unwrap_or(0.0) as i64 * dy / h }
        };
        let cx = cx as i64;
        cx >= x as i64 + at("TLoffsetx", "BLoffsetx") && cx <= (x + w) as i64 + at("TRoffsetx", "BRoffsetx")
    }

    /// The glint that sweeps the screen, shared by `NFS5.HEADER` (0x4CB024…0x4CB0F6) and
    /// `NFS5.CarLogo` (0x4C1CE0…0x4C1DAF) — `RE/notes/controls.md` §4.25. Both take the travel
    /// from 0x4E3550 — `[0x6907E0]`, set to **640.0f** by 0x4D35B4 — and keep their own timer and
    /// random wait ([`glints::Sweep`]): the header's `rand() % 128 + 128`, the logo's
    /// `rand() % 128`. The sprite's **x is the elapsed tick count itself**, at 128 Hz, so it
    /// crosses the whole screen at a pixel a tick, and its y is the widget's own. The header's
    /// timer only runs with the fade at 0 (0x4CB01C); the logo's always does.
    fn sweep_glint(&self, i: usize, logo: bool, out: &mut Vec<Sprite>, aspect: f32, y: f32, alpha: f32) {
        if !logo && self.widget_progress() * 255 / TRANS_FULL != 0 {
            return;
        }
        let now = self.ticks() as i64;
        let elapsed = self.step_glint(
            i,
            now,
            || glints::Glint::Sweep(glints::Sweep::new(0, logo)),
            |g, t, _| match g {
                glints::Glint::Sweep(s) => s.step(t),
                _ => None,
            },
        );
        let Some(elapsed) = elapsed else { return };
        let Some(n) = self.glint else { return };
        let Some(&(tex, aw, ah)) = self.shapes.get(&("weeicons.fsh".to_string(), n)) else { return };
        let (pos, size) = self.place(aspect, elapsed as f32, y, aw, ah);
        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
    }

}

impl LayoutScreen {
    /// Draw the flourish of `glint_pieces` at the widget's origin, on the widget's own timer
    /// ([`glints::Flourish`]): a run of `rand() % 32 + 64` ticks, then `rand() % 256 + 256` of
    /// rest (0x51FE6C, 0x51FEC2), the variant redrawn each run for `animate` 5 (0x51FE8E), and a
    /// run starts only when the caller's `may_start` says so. `built` is the tick the object was
    /// made at — the screen's open for the ones a bind builds, the first draw for the combo
    /// family's, which its draw builds (0x4C5937).
    #[allow(clippy::too_many_arguments)]
    fn glinty_flourish(&self, i: usize, built: Option<i64>, may_start: &dyn Fn(&mut glints::Flourish) -> bool, animate: i32, out: &mut Vec<Sprite>, aspect: f32, px: f32, py: f32, w: i32, h: i32) {
        let now = self.ticks() as i64;
        let run = self.step_glint(
            i,
            now,
            || glints::Glint::Flourish(glints::Flourish::new(built.unwrap_or(now))),
            |g, t, _| match g {
                glints::Glint::Flourish(f) => {
                    let ok = may_start(f);
                    f.step(t, ok, animate)
                }
                _ => None,
            },
        );
        let Some((elapsed, duration, variant)) = run else { return };
        if w < 2 || h < 2 {
            return;
        }
        let band = glint_band(w, h);
        if band <= 0 {
            return;
        }
        // 0x51FF32…0x51FF4D: `(band + w + h)·elapsed / duration − band`, a signed `idiv`
        let p = ((band + w + h) as i64 * elapsed / duration) as i32 - band;
        for g in glint_pieces(w, h, p, band, variant) {
            let (a, b) = (tint(g.c0), tint(g.c1));
            let (x, y) = (px + g.x, py + g.y);
            let pts = [[x, y], [x + g.w, y], [x, y + g.h], [x + g.w, y + g.h]];
            let cols = if g.along_x { [a, b, a, b] } else { [a, a, b, b] };
            self.gouraud(out, aspect, pts, cols);
        }
    }
}

impl LayoutScreen {
    /// Every sprite of the screen, back to front in file order. `accessor` resolves an
    /// `NFS5.AccessorWatch`'s `accessor` name to the text it shows; a watch it cannot resolve is
    /// counted as unrendered instead of being filled with something invented.
    pub fn sprites(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        self.sprites_reporting(aspect, strings, selected, accessor, &mut Vec::new())
    }

    /// Every widget that produced **no sprite at all** — its class and the source it names. This is
    /// the port's own measure of what it does not draw: a widget whose content the original places
    /// from code, and for which the file carries neither a shape nor a string, leaves nothing
    /// behind here. `examples/layout_sweep.rs` runs it over every `.lay` in the game.
    pub fn blank_widgets(&self, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
        let mut blank = Vec::new();
        self.sprites_reporting(aspect, strings, None, accessor, &mut blank);
        blank
    }

    /// The screen **without** the background screen it names — how a screen draws when it is
    /// one layer of several: the front end's draw 0x4D2F30 paints the one background object
    /// `[fe+0xE4]` (the top screen's `data.screen`, swapped by 0x4D1BD0) and then the layers'
    /// screens, bottom first (0x4D30F8, 0x4D323E), so a covered screen shows between the two.
    pub fn sprites_own(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        self.sprites_reporting_bg(aspect, strings, selected, accessor, &mut Vec::new(), false)
    }

    /// The background screen alone (`[fe+0xE4]`), empty when the screen names none.
    pub fn background_sprites(&self, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>) -> Vec<Sprite> {
        self.background.as_ref().map(|bg| bg.sprites(aspect, strings, None, accessor)).unwrap_or_default()
    }

    fn sprites_reporting(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>) -> Vec<Sprite> {
        self.sprites_reporting_bg(aspect, strings, selected, accessor, blank, true)
    }

    fn sprites_reporting_bg(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>, with_bg: bool) -> Vec<Sprite> {
        let mut out = Vec::new();
        if let Some(bg) = self.background.as_ref().filter(|_| with_bg) {
            // the background's own gaps are not this screen's: every background layout is one of
            // the 98 in `FEData/Layouts` and the sweep measures it in its own right, so reporting
            // it here would count it once per screen that sits on it
            out.extend(bg.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new()));
        }
        let bg_end = out.len();
        // `FEEndOfEraCallBackList`'s slot 15 (0x4FF1F0) is the screen's whole draw: it does not
        // call `Window::draw` (0x528760), so neither the fill of section 0 nor any child is painted
        if let Some(e) = &self.end_of_era {
            self.end_of_era_sprites(e, aspect, &mut out);
            // the pointer is the front end's own, drawn over every screen (§4.152)
            self.cursor_sprite(&mut out, aspect, None);
            return out;
        }
        // which widget the pointer is over, once for the whole screen (§4.39)
        let hovered = self.hovered();
        // `FeComboBox` paints an open drop-down from **slot 17** (0x4C6A88), the pass after the
        // ordinary draw, and steps the sprite depth for it (0x4DAA10 / 0x4DAAC0) — so it lands over
        // every widget on the screen. The port collects it here and appends it at the end.
        let mut overlay: Vec<Sprite> = Vec::new();
        self.overlay_mask.set(None);
        // **The draw order is the section order, backwards** — `RE/notes/controls.md` §4.67.
        // `Window::load` (slot 20, 0x5282E0) walks the layout's sections **1 … count-1 forwards**
        // and `Window::addChild` (slot 22, 0x528120) is a `push_back`, so the child vector is in
        // section order; `Window::draw` (slot 15, 0x528760) then walks it from `_Last` down to
        // `_First` (0x528785…0x528799). So the **last** section is painted first and **section 1
        // is on top**. Section **0** is not a child at all: 0x5285D3 reads it once for the screen
        // descriptor, and it is the screen's own fill, painted before everything.
        let order: Vec<usize> = std::iter::once(0).chain((1..self.layout.elements.len()).rev()).collect();
        for i in order {
            let e = &self.layout.elements[i];
            let ((tdx, tdy), talpha) = self.transition_of(e);
            let (x, y) = self.position_of(e);
            let (x, y) = (x + tdx, y + tdy);
            let (w, h) = self.box_of(e, 0);
            let first = out.len();
            // the widget's `state` selects a part list that draws nothing at all — it is erased,
            // not undrawn, so it is not counted as a gap either
            if self.state_draws_nothing(e) || self.hidden_flags.contains(&i) {
                continue;
            }
            let mut closed = false;
            // what every per-class draw in `layout/widgets.rs` reads
            let d = WidgetDraw { i, e, x, y, w, h, talpha, aspect, strings, selected, hovered, accessor };
            match e.class.as_str() {
                c if c.starts_with("REAL.BLACK") => self.draw_screen_fill(d, &mut out),
                // `NFS_NO_GOURAUD=1` skips them — the probe for which sprites bury the text
                "NFS5.GOURAUDBOX" if std::env::var("NFS_NO_GOURAUD").is_ok() => {}
                "NFS5.GOURAUDBOX" => self.draw_gouraud_box(d, &mut out),
                "NFS5.GouraudIndicatorBar" => self.draw_gouraud_indicator_bar(d, &mut out),
                "NFS5.GlintyRectangle" => self.draw_glinty_rectangle(d, &mut out),
                "NFS5.PANEL" => {
                    closed = self.draw_panel(d, &mut out, blank);
                }
                "NFS5.3DCAR" => self.draw_3d_car(d, &mut out),
                "NFS5.DEADZONE" => {
                    closed = self.draw_deadzone(d, &mut out);
                }
                "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHATDISPLAYBOX"
                | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP" => self.draw_text_boxes(d, &mut out),
                "NFS5.ListBox" => self.draw_list_box(d, &mut out),
                "NFS5.RADIOGROUP" => self.draw_radio_group(d, &mut out, blank),
                "NFS5.TrackSmall2dSpline" => self.draw_track_small_spline(d, &mut out),
                "NFS5.PRIZERACEMAP" | "NFS5.FACTORYMAP" => self.draw_race_or_factory_map(d, &mut out),
                "NFS5.PLAYERPICTURE" | "NFS5.MINIPLAYERPICTURE" | "NFS5.BADGE" | "NFS5.MINIBADGE" | "NFS5.FACTORYPIC" | "NFS5.HUD" => self.draw_pack_pictures(d, &mut out),
                "NFS5.MOVIEBOX" | "NFS5.MOVIECIRCLE" => self.draw_movie(d, &mut out),
                "NFS5.ACCOUNTTHING" => self.draw_account_thing(d, &mut out),
                "NFS5.ANIMSHOWCASEPICTURE" => self.draw_anim_showcase_picture(d, &mut out),
                "NFS5.ChroniclePictureIcon" | "NFS5.ShowcasePicture" | "NFS5.ChroniclePicture" => self.draw_chronicle_or_showcase_picture(d, &mut out),
                "NFS5.CarLogo" => self.draw_car_logo(d, &mut out),
                "NFS5.KNOCKOUTTRACKLIST" => self.draw_knockout_track_list(d, &mut out),
                "NFS5.ChronicleMainBoxControl" => self.draw_chronicle_main_box(d, &mut out),
                "NFS5.AnimatingPicture" => self.draw_animating_picture(d, &mut out),
                "NFS5.HEADER" | "NFS5.SUBHEADER"
                | "REAL.LABEL" | "NFS5.PICTURE" | "NFS5.NonLocalizedLabel" | "NFS5.ExtraCircle"
                | "NFS5.OutlineRectangle" | "NFS5.Rectangle" | "NFS5.BEVELEDOPTIONBOX" | "NFS5.GLINTYLINE" => {
                    closed = self.draw_template_classes(d, &mut out);
                }
                "NFS5.CheckBox" => self.draw_check_box(d, &mut out),
                // `NFS5.CHRONICLESLIDER` is the same helper (0x4D0B90, `[helper+0x130] == 0`)
                "NFS5.CarSettingSlider" | "NFS5.CHRONICLESLIDER" => self.draw_car_setting_slider(d, &mut out),
                "NFS5.ComboBoxSlider" | "NFS5.ColorBox" => self.draw_combo(d, &mut out, &mut overlay),
                "NFS5.GOTOBUTTON" => self.draw_goto_button(d, &mut out),
                "NFS5.BOTTOMBARBUTTON" | "NFS5.GRAPHICTABBUTTON" => self.draw_bar_or_graphic_tab(d, &mut out),
                "NFS5.TABBUTTON" | "NFS5.CAMERASELECT" | "NFS5.CAMERAPICTURE" => self.draw_tab_or_camera(d, &mut out),
                _ => {
                    closed = self.draw_default(d, &mut out);
                }
            }
            // every sprite this widget produced fades with it (0x4C8840)
            if talpha < 1.0 {
                for s in &mut out[first..] {
                    s.color[3] *= talpha;
                    // a per-corner triangle carries its own colours
                    if let Some((_, cols)) = s.tri.as_mut() {
                        for c in cols {
                            c[3] *= talpha;
                        }
                    }
                }
            }
            // the first block is the screen's own descriptor, not a widget: it is the one that
            // carries `screen=`, `room=`, `musicfile=` and `callbacklist=`, and its class
            // (`REAL.BLACK_640x480`) brings a `color` and a `size` with it. Painting that colour
            // would cover the background layout the same block names, which every screen visibly
            // has, so it is metadata and not counted as undrawn.
            // `NFS_WIDGET_DUMP=1` prints, per widget, how many sprites it produced, their extent
            // and the transition alpha they carry — the diagnostic that found the faint-text bug
            // recorded in `RE/AUDIT.md`
            if std::env::var("NFS_WIDGET_DUMP").is_ok() {
                let n = out.len() - first;
                let b = out[first..].iter().fold((9.0f32, 9.0f32, -9.0f32, -9.0f32), |a, s| (a.0.min(s.pos[0]), a.1.min(s.pos[1]), a.2.max(s.pos[0]), a.3.max(s.pos[1])));
                let f0: Vec<(String, [f32; 4])> = { let mut v: Vec<(String, [f32; 4])> = out[first..].iter().map(|s| (format!("{:?}", s.texture), s.color)).collect(); v.dedup(); v.truncate(3); v };
                let hot = match self.state_for(e, i, selected, hovered) {
                    "normal" => String::new(),
                    st => format!(" [{st}]"),
                };
                println!("  #{i:3} {:<26} pos=({x:.0},{y:.0}) size=({w:.0}x{h:.0}) sprites={n}{hot} box=({:.3},{:.3})-({:.3},{:.3}) alpha={talpha:.2} first={f0:?}", e.class, b.0, b.1, b.2, b.3);
            }
            // **`NFS5.3DCAR` is never a sprite, in either build.** Its draw, `FECar3DControl`'s
            // slot 15 = **0x4C0990**, calls none of the painter family: 0x4C09A3 places the car
            // through 0x4C0D10 and the only other thing it can do is 0x4E1E50(7), a sound, when
            // the widget is hovered (0x4C7DD0) and its accessor answers (0x4F09C0). The car
            // itself belongs to the 3D pass — `positiontype` 0 hands it to the screen's room
            // (0x4BFEEC) and 1…3 place one of the widget's own at a fixed camera-relative spot
            // (`RE/notes/frontend-3d.md` §4). This port puts that car into an offscreen view so
            // it lands in the right place in the 2D layering, which is a porting device and not
            // something the original does; a sweep that counts 2D sprites therefore has nothing
            // to see here, the same way `deadzone.lay`'s axis bars have nothing to show without
            // a pad. Reported on its own line rather than as a blank.
            if out.len() == first && i > 0 && !closed && e.class != "NFS5.3DCAR" {
                blank.push((e.class.clone(), source_of(e)));
            }
        }
        // the tooltip last of all: the original builds it in the widget's own tick, and it has to
        // be over the screen it describes
        if let Some(h) = hovered
            && let Some(e) = self.layout.elements.get(h)
        {
            self.tooltip(&mut overlay, aspect, e, strings);
        }
        // the drop-down's depth mask is stepped here, after every widget's own draw, where its
        // sprites go (0x4DAA10 at 0x4C6516 in the later pass)
        if let Some((a, b, c)) = self.overlay_mask.take() {
            let m = fe_depth::mask_begin();
            fe_depth::apply(&mut overlay[a..b], m.stencil);
            fe_depth::apply(&mut overlay[b..c], m.inside);
        }
        out.append(&mut overlay);
        // last of all, over the tooltip and the drop-down alike
        self.cursor_sprite(&mut out, aspect, hovered);
        if std::env::var("NFS_LAYER_DUMP").is_ok() {
            println!("layer: background {} sprites, own {} sprites", bg_end, out.len() - bg_end);
        }
        out
    }
}

#[cfg(test)]
mod tests;
