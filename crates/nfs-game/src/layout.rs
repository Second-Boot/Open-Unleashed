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

/// Read `FEData/Controls/*.ctr` — the widget class templates that hold every attribute's default
/// and the pieces each class is made of (`RE/notes/controls.md`).
/// The front end's string table, for the one geometry rule that needs it: a `TEXT` or
/// `LOCALIZEDTEXT` part's `size` is the **extent of the string it draws**, which several templates
/// then position other parts from (`BottomBarButton.ctr`'s own `pos.value` is
/// `(data.align&1)?data.posx-(label.size.width+picture.size.width+data.space):data.posx`).
pub fn load_strings(paths: &GamePaths) -> Strings {
    let dir = paths.fe_data().join("Locale");
    match GamePaths::find_file(&dir, "FEStrings.csv").and_then(|p| std::fs::read(p).ok()) {
        Some(b) => Strings::parse(&b, 0),
        None => Strings::default(),
    }
}

pub fn load_controls(paths: &GamePaths) -> ControlLibrary {
    let dir = paths.fe_data().join("Controls");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        tracing::debug!(dir = %dir.display(), "no control templates");
        return ControlLibrary::default();
    };
    let files = entries.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("ctr"))).filter_map(|e| {
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
    /// the track's name in caps — `FEStrings[trk+4 + 20]`, which is what 0x4FCFAD…0x4FCFBE looks
    /// up for the list beside the map
    pub name: String,
}

/// A `.mad` playing inside a `NFS5.MOVIEBOX`.
///
/// The frame period is the video header's `+0x0C` as **16.16 fixed point**: 0x4DCB01 masks its low
/// sixteen bits off as the fraction and 0x4DCB06 shifts the rest down to whole units, which it adds
/// to the player's own clock. Every shipped movie carries 4369067, so 66.67 ms a frame — fifteen a
/// second.
struct MoviePlayer {
    /// the whole file, and where its video chunks are
    data: Vec<u8>,
    chunks: Vec<(usize, usize)>,
    decoder: nfs_formats::mad::MadDecoder,
    /// the next chunk to decode
    next: usize,
    period_q16: u64,
    clock_q16: u64,
    tex: TextureId,
    size: (f32, f32),
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
    /// widget classes the file does not place, with how many of each were skipped
    unrendered: Vec<(String, usize)>,
    /// element index of an `NFS5.CarLogo` → the `.fsh` pack and the first of its two entries
    logos: HashMap<usize, (String, usize)>,
    /// `NFS5.PANEL` element indices the screen deliberately leaves **closed** — the tabs that are
    /// not the selected one, and the CD player. A closed panel draws nothing (0x4CEBC0 returns at
    /// once when `[panel+0x14C]` is null), so it is not a gap; a panel whose file simply would not
    /// load is not in here and is still counted.
    closed: std::collections::HashSet<usize>,
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
    /// where this screen is in its transition
    transition: Transition,
    /// drawn over another screen (the bottom bar, 0x4D39D3) — an overlay must not paint its own fill
    overlay: bool,
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
/// The sweep takes half of the timer's ticks-per-second ([0x5DEB48] / 2, 0x4E0A36); the timer is
/// the 1 kHz `timeSetEvent` set up at 0x564F2D, so half a second. CONFIRMED.
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
    pub dx: f32,
    pub text: String,
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
    /// them (`drawPartsHeading` 0x518DE0), 13 px tall.
    Header(String),
}

impl ListRow {
    /// How tall this row is. The base accessor answers 14 for every row (0x4BC700); a parts list's
    /// rows are 13 and its headings another 13 (0x516080).
    pub fn height(&self) -> f32 {
        match self {
            ListRow::Text(_) => LIST_ROW_H,
            ListRow::Columns(_) => LIST_ROW_H,
            ListRow::Header(_) => PART_HEADER_H,
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

/// The screen block's `screen=` id → the layout drawn underneath it (`FE_loadBackground` 0x4E1470,
/// CONFIRMED). The original has a second, reduced table — `ID_FULLOVAL` → `screenfulloval` and
/// everything else → `screenveryempty` — behind a memory/option gate (`[0x5D9D38]`, set at 0x4E3584
/// only when more than 4 MB are free and `[0x657E5C]` is set), and swaps `screenoptionoval` for
/// `screenoptionovallite` on a screen with `canplaygame=1` when memory is short. A port with memory
/// to spare always takes the full table below.
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

/// A front-end colour, as the shader wants it.
///
/// The original writes these bytes into an 8-bit frame buffer with no colour management at all:
/// a `NFS5.GOTOBUTTON`'s `0xFFBC0000` is red **188** on the player's screen. This port's surface
/// is sRGB, so a value handed to the shader is treated as *linear* and encoded on the way out —
/// 188 came back as **223**, and every flat colour in the front end was that much too light.
/// Undoing the encoding here puts the byte back where the file put it. Measured, not assumed:
/// sampling a pill over black gave 223 before and 188 after.
///
/// Only the three colour channels; alpha is not encoded.
fn tint(c: [u8; 4]) -> [f32; 4] {
    let lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    [lin(c[0]), lin(c[1]), lin(c[2]), c[3] as f32 / 255.0]
}

/// The tag the chronicle's picture tiles carry, which the control's own name repeats at 0x5DBBFE.
const CHRONICLE_PICTURE_TAG: &str = "1111";

/// The two fonts `NFS5.BADGE` loads for itself (0x4CF25C, 0x4CF2A0).
const BADGE_NAME_FONT: &str = "univers14b.ffn";
const BADGE_RANK_FONT: &str = "univers10b.ffn";
/// The drop shadow `NFS5.BADGE` lays over the licence card (0x4CF208).
const LICENSE_SHADOW: &str = "LicenseShadow.fsh";

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
                Ok(b) => screen.background = Some(Box::new(b)),
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
                Err(e) => tracing::debug!(file, error = %e, "panel layout unavailable"),
            }
        }
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
                let Some(file) = attr("shapePathName").filter(|v| !v.is_empty()) else { continue };
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
        let mut want: Vec<String> = vec![DEFAULT_FONT.to_string()];
        // `NFS5.BADGE` builds its own two fonts rather than taking one from the file: 0x4CF25C
        // loads `Univers14b.ffn` for the player's name and 0x4CF2A0 `Univers10b.ffn` for the rank
        if layout.elements.iter().any(|e| e.class == "NFS5.BADGE") {
            want.push(BADGE_NAME_FONT.to_string());
            want.push(BADGE_RANK_FONT.to_string());
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
        Ok(Self { layout, background: None, shapes, fonts, white, unrendered, logos: HashMap::new(), checks: HashMap::new(), knobs: HashMap::new(), closed: std::collections::HashSet::new(), groups: HashMap::new(), bars: HashMap::new(), lists: HashMap::new(), car_views: HashMap::new(), track_maps: HashMap::new(), movies: HashMap::new(), ko_strip: HashMap::new(), badges: HashMap::new(), clock_ms: 0, controls, fe_strings, transition: Transition::Idle, overlay: false })
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

    /// Resolve one `.ctr` expression reference for `e`: `data.<key>` is the widget's own attribute,
    /// `control.size.width` its box, and `<part>.<key>` another piece of the same template. A
    /// reference the port cannot answer — a text part's width, which depends on the string — is
    /// left unresolved so the whole expression falls back rather than silently reading zero.
    fn reference(&self, e: &nfs_formats::lay::Element, name: &str, depth: u32, measure: bool) -> Option<f64> {
        let (head, rest) = name.split_once('.')?;
        if head == "data" {
            return self.attr_num(e, rest);
        }
        let (bw, bh) = self.box_of(e, depth + 1);
        if head == "control" {
            return match rest {
                "size.width" => Some(bw as f64),
                "size.height" => Some(bh as f64),
                _ => None,
            };
        }
        if depth > 4 {
            return None;
        }
        let part = self.controls.get(&e.class)?.part(head)?;
        let (key, field) = rest.split_once('.')?;
        // A drawable part's `size` is the extent of **what it draws** when the template does not
        // give it one: `Picture.ctr`'s picture part and `BottomBarButton.ctr`'s label both carry
        // `size.flags=HIDDEN|READONLY` and no `size.value` at all, and the bar button's own
        // position is written over `label.size.width + picture.size.width`. HIGH — the rule is
        // what the expressions require, the point at which the original measures was not traced.
        let pair = match part.value(key).filter(|v| !v.trim().is_empty()) {
            Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1, measure))?,
            // a measured extent is only for an explicit `<part>.size.*` reference, never for the
            // widget's own box: `Label.ctr`'s `size.value` is `label.size.width*2,…`, and letting
            // that resolve would give every label a box twice its text and move it, because this
            // port implements the alignment rule as an anchor in the box rather than as the part
            // offset the template writes (`(0-label.size.width)/2*data.align`, `layouts.md` §2.1)
            None if key == "size" && measure => self.part_extent(e, part)?,
            None => return None,
        };
        match field {
            "width" | "x" => Some(pair.0),
            "height" | "y" => Some(pair.1),
            _ => None,
        }
    }

    /// What one part of a widget actually covers: a `SHAPE`'s `.fsh` entry at its own pixel size,
    /// or a `TEXT`/`LOCALIZEDTEXT`'s string measured in the font that part draws it in.
    fn part_extent(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part) -> Option<(f64, f64)> {
        match p.kind() {
            nfs_formats::ctr::PartKind::Shape => {
                // the same resolution `shape_parts` uses — the part attribute's screen key against
                // the block, else the template's own value. `Graphic Tab Button.ctr`'s
                // `selectedbg` has an **empty** `shapePathName.value` and only the screen key
                // `selectedbgSmallPathName` reaches its pack, so without this the four unopened
                // tabs could not measure the art they centre their captions over.
                let file = self.part_attr(e, p, "shapePathName").filter(|v| !v.is_empty())?;
                let index = match self.part_attr(e, p, "shapeIndex") {
                    Some(v) => nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true)).filter(|n| *n >= 0.0)? as usize,
                    None => 0,
                };
                let &(_, w, h) = self.shapes.get(&(file.to_string(), index))?;
                Some((w as f64, h as f64))
            }
            nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText => {
                let text = match e.get("text").filter(|t| !t.is_empty()) {
                    Some(t) => t.to_string(),
                    None => self.fe_strings.get(e.string_id()?).filter(|t| !t.is_empty())?.to_string(),
                };
                let named = e
                    .get("fontPathName")
                    .filter(|f| !f.is_empty())
                    .or_else(|| p.value("fontPathName").filter(|f| !f.is_empty()))
                    .map(|f| f.to_lowercase());
                let f = named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))?;
                let (_, w) = f.place(&text);
                Some((w as f64, f.line_px() as f64))
            }
            _ => None,
        }
    }

    /// The widget's box. The `.lay` block's `size` wins; failing that the class template's own
    /// `size.value`, which for most classes is an expression over the block's keys
    /// (`data.width,data.height` for a button, `data.width,data.selectionboxmaxheight` for a list).
    fn box_of(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
        if let Some(p) = e.pair("size") {
            return p;
        }
        if depth <= 4
            && let Some(c) = self.controls.get(&e.class)
            && let Some(v) = c.value("size")
            && let Some((w, h)) = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1, false))
        {
            return (w as f32, h as f32);
        }
        (e.float("width").unwrap_or(0.0), e.float("height").unwrap_or(0.0))
    }

    /// The widget's position: the block's own `pos`, or the class template's `[control] pos.value`
    /// — `NFS5.HEADER` carries no `pos` at all and `Header.ctr` puts it at (6, 4).
    fn position_of(&self, e: &nfs_formats::lay::Element) -> (f32, f32) {
        if let Some(p) = e.pair("pos") {
            return p;
        }
        if let Some(c) = self.controls.get(&e.class)
            && let Some(v) = c.value("pos")
            && let Some((x, y)) = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true))
        {
            return (x as f32, y as f32);
        }
        (0.0, 0.0)
    }

    /// Whether the widget's `state` selects **no drawable part at all**, in which case it draws
    /// nothing however much its `.lay` block carries.
    ///
    /// A `.lay` block's `state=` is not decoration: it is the name of one of the template's
    /// `[state.<name>]` sections, and that section's `parts=` *is* the widget's draw list.
    /// The chain in the original:
    ///
    /// * the nine state names live in a 25-byte-stride table at **0x5D81FC**, ending at 0x5D82DD —
    ///   `normal`, `stringnotused`, `hover`, `selected`, `disabled`, `hidden`, `focused`,
    ///   `focusedhover`, `selectedhover`, in that order;
    /// * **0x4C778F…0x4C7818** walks that table and, for each name the template's `states=` list
    ///   carries, sets bit `1 << i` in `[widget+0x110]` — the states the class supports;
    /// * **0x4C7820…0x4C78B2** clears `[widget+0x10C]` and matches the widget's own `state`
    ///   attribute against names 2…8, setting that name's bit on a hit;
    /// * **0x4C82A0** turns those flags back into a name by priority — 0x20 → `"hidden"`
    ///   (0x4C82FE), 0x10 → `"disabled"`, 0x40+0x04 → `"focusedhover"`, 0x40 → `"focused"`,
    ///   0x08+0x04 → `"selectedhover"`, 0x08 → `"selected"`, 0x04 → `"hover"`, else `"normal"`;
    /// * **0x525285…0x5252F5** matches that name against the loaded `[state.*]` sections and hands
    ///   the index to 0x525580 → **0x5271F0**, which *copies that state's `parts` vector over the
    ///   widget's active part list* (`[[widget+0xF4]+0x10]`).
    ///
    /// So when the chosen section lists only `DATA` parts — and `DATA` draws nothing, it is the
    /// attribute block (`RE/notes/controls.md` §1) — the widget is erased. Three shipped widgets
    /// are in exactly that case, all `NFS5.PANEL` with `state=hidden` against `Panel.ctr`'s
    /// `[state.hidden] parts=data`: `carsetup.lay` §77, `carsetupdesc.lay` §11 and
    /// `usedcars.lay` §19. Each carries `color=0x80ffFF00`, so drawing them anyway paints a
    /// half-transparent yellow slab over the middle of those three screens.
    fn state_draws_nothing(&self, e: &nfs_formats::lay::Element) -> bool {
        let Some(c) = self.controls.get(&e.class) else { return false };
        let Some(state) = e.get("state") else { return false };
        // an unknown state name leaves the ctor's own list in place, which is every part
        if !c.has_state(state) {
            return false;
        }
        !c.state_parts(state).iter().filter_map(|n| c.part(n)).any(|p| p.kind() != nfs_formats::ctr::PartKind::Data)
    }

    /// An embedded layout's sprites, moved into the widget's own place. The offset is applied in
    /// the same normalised space [`place`](Self::place) maps into, so it carries the letterbox
    /// factor with it.
    #[allow(clippy::too_many_arguments)]
    fn embedded(&self, g: &LayoutScreen, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>, x: f32, y: f32) -> Vec<Sprite> {
        // an embedded layout is one of the 98 the sweep already walks on its own, so its gaps are
        // reported there and not again here — see the note on the background above
        let _ = &blank;
        let mut inner = g.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new());
        if x != 0.0 || y != 0.0 {
            let design_aspect = DESIGN.0 / DESIGN.1;
            let fw = if aspect > design_aspect { design_aspect / aspect } else { 1.0 };
            let (dx, dy) = (x / DESIGN.0 * fw, y / DESIGN.1);
            for sp in &mut inner {
                sp.pos[0] += dx;
                sp.pos[1] += dy;
            }
        }
        inner
    }

    /// The `RECT` parts of the widget's class template that its `state` draws, each resolved to a
    /// rectangle and a colour (`RE/notes/controls.md` §1). This is how a class that is nothing but
    /// boxes — `NFS5.CONTROLLERSETUP` is a filled box with a one-pixel border — draws without the
    /// port knowing anything about it.
    fn rect_parts(&self, e: &nfs_formats::lay::Element) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let state = e.get("state").unwrap_or("normal");
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let Some((x, y)) = at("pos") else { continue };
            let Some((w, h)) = at("size") else { continue };
            // a colour may be a literal or a reference to one of the `data` attributes
            let colour = self
                .part_attr(e, p, "color")
                .and_then(|v| parse_color(v).or_else(|| v.split_once('.').and_then(|(_, key)| self.attr(e, key)).and_then(parse_color)));
            let Some(colour) = colour else { continue };
            out.push((x as f32, y as f32, w as f32, h as f32, colour));
        }
        out
    }

    /// A part's colour attribute: a literal `0x…`, or an **expression** over the widget's own
    /// attributes — `GoToButton.ctr` writes its caption's colour as
    /// `data.medium?0x00000000:0xffffffff`, so which of its three labels is opaque is arithmetic.
    fn part_colour(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, key: &str) -> Option<[u8; 4]> {
        let v = self.part_attr(e, p, key)?;
        if let Some(c) = parse_color(v) {
            return Some(c);
        }
        let n = nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true))?;
        if !(0.0..=4294967295.0).contains(&n) {
            return None;
        }
        let n = n as u32;
        Some([((n >> 16) & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, (n & 0xFF) as u8, (n >> 24) as u8])
    }

    /// The `RECT` parts of one state of the widget's class, each resolved to a rectangle and a
    /// colour, with the fully transparent ones dropped. Unlike [`rect_parts`](Self::rect_parts)
    /// this takes the state by name — a button's highlight is `[state.selected]`, which the `.lay`
    /// block never says — and it evaluates the colour as an expression.
    fn state_rects(&self, e: &nfs_formats::lay::Element, state: &str) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let (Some((px, py)), Some((pw, ph))) = (at("pos"), at("size")) else { continue };
            let Some(col) = self.part_colour(e, p, "color").filter(|c| c[3] != 0) else { continue };
            out.push((px as f32, py as f32, pw as f32, ph as f32, col));
        }
        out
    }

    /// One part attribute with the `%…%` indirection resolved: `%<part>.<key>%` is that part's
    /// value for that key, which is how `GoToButton.ctr`'s two spare captions say
    /// `stringID.value=%label.stringID%` — "the same string the first one shows".
    fn part_ref<'a>(&'a self, e: &'a nfs_formats::lay::Element, p: &'a nfs_formats::ctr::Part, key: &str) -> Option<String> {
        let v = self.part_attr(e, p, key)?;
        let Some(inner) = v.strip_prefix('%').and_then(|v| v.strip_suffix('%')) else {
            return Some(v.to_string());
        };
        let (part, k) = inner.split_once('.')?;
        if part == "data" {
            return e.get(k).map(str::to_string);
        }
        let q = self.controls.get(&e.class)?.part(part)?;
        self.part_attr(e, q, k).map(str::to_string)
    }

    /// Draw a widget entirely out of its class template: the `RECT` parts of the state, then its
    /// `SHAPE` parts, then its `TEXT` / `LOCALIZEDTEXT` parts — each where the template puts it,
    /// in the font and colour it names, and skipping any whose colour is fully transparent. That
    /// last rule is how a class carries several captions and shows one (`controls.md` §4.3).
    #[allow(clippy::too_many_arguments)]
    fn template_widget(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, state: &str, x: f32, y: f32, strings: &Strings) {
        for (px, py, pw, ph, col) in self.state_rects(e, state) {
            out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(col)));
        }
        for (px, py, pw, ph, file, index, flip) in self.shape_parts(e) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            let (dw, dh) = (if pw > 0.0 { pw } else { aw }, if ph > 0.0 { ph } else { ah });
            let (pos, size) = self.place(aspect, x + px, y + py, dw, dh);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
        }
        let Some(c) = self.controls.get(&e.class) else { return };
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            let text = match p.kind() {
                nfs_formats::ctr::PartKind::LocalizedText => {
                    let id = self.part_ref(e, p, "stringID").and_then(|v| v.trim().parse::<u32>().ok());
                    match id.filter(|n| *n != 0).and_then(|n| strings.get(n)) {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => continue,
                    }
                }
                nfs_formats::ctr::PartKind::Text => match self.part_ref(e, p, "text").filter(|t| !t.is_empty()) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            };
            let Some(col) = self.part_colour(e, p, "foreColor").filter(|c| c[3] != 0) else { continue };
            let (px, py) = self
                .part_attr(e, p, "pos")
                .and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)))
                .unwrap_or((0.0, 0.0));
            let Some(f) = self
                .part_attr(e, p, "fontPathName")
                .map(|f| f.to_lowercase())
                .and_then(|f| self.fonts.get(&f))
                .or_else(|| self.fonts.get(DEFAULT_FONT))
            else {
                continue;
            };
            self.text(out, aspect, f, &text, x + px as f32, y + py as f32, 0.0, 0, tint(col));
        }
    }

    /// One attribute of one part, resolved the way the original does: the `.lay` block's value for
    /// the attribute's **screen key** when it has one, else the template's `<key>.value`
    /// (`nfs_formats::ctr::Part::screen_key`, 0x52BC90 / 0x52BFB4 / 0x52C000).
    fn part_attr<'a>(&self, e: &'a nfs_formats::lay::Element, p: &'a nfs_formats::ctr::Part, key: &str) -> Option<&'a str> {
        if let Some(k) = p.screen_key(key)
            && let Some(v) = e.get(&k).filter(|v| !v.is_empty())
        {
            return Some(v);
        }
        // an **empty** `<key>.value` is not a value: `Graphic Tab Button.ctr` writes `size.value=`
        // on the parts whose size is their art's, and `shapePathName.value=` on the two whose pack
        // only the screen key names. Reading either as a value can only fail.
        p.value(key).filter(|v| !v.trim().is_empty())
    }

    /// The `SHAPE` parts of the widget's class template that its `state` draws, each resolved to a
    /// place and a `.fsh` pack entry (`RE/notes/controls.md` §1).
    ///
    /// This is the other half of [`rect_parts`](Self::rect_parts): a class whose art is entirely in
    /// its template draws through here without the port knowing anything about it.
    /// `NFS5.CARSETTINGBUTTON` is the plainest case — `CarSettingButton.ctr` gives its `picnonsel`
    /// part `shapePathName=weeicons.fsh` and `shapeIndex=data.nonselindex`, so the widget's own
    /// `nonselindex` picks the arrow. The same shape covers `NFS5.POPUPCIRCLE`,
    /// `NFS5.ExtraCircle`, the tab buttons and the rest of the art-from-template classes.
    fn shape_parts(&self, e: &nfs_formats::lay::Element) -> Vec<(f32, f32, f32, f32, String, usize, i64)> {
        // `NFS5.POPUPCIRCLE` is a **modal panel the game raises**, not part of the screen behind
        // it, and it is skipped until this port has somewhere to raise one from. Three measurements
        // say so, and drawing it anyway was tried and looks exactly as they predict:
        //
        // * `PopUp.ctr` pins it at `pos=113,100` (`HIDDEN|READONLY`) and tiles **one** 207×175
        //   quarter four ways — `p1`…`p4` at (0,0), (207,0), (0,175), (207,175) with `flip` 0…3 —
        //   so it is a 414×350 oval over the middle of a 640×480 screen, not a border around it;
        // * the template's states are `normal` and **`hidden`**, and `[state.hidden]` draws only
        //   `data`, which is nothing;
        // * in **18 of the 19** layouts that carry one it is the **last** section, so it draws over
        //   everything else. (The exception is `factdone.lay`, where a `NFS5.HEADER` follows it.)
        //
        // Drawn unconditionally on `selectname.lay` it covers the profile list and the buttons with
        // a translucent oval, which is what a panel meant to be raised over them would do.
        //
        // **Not** skipped because of `swrenderoff`: that key reaches `[widget+0x13C]` at 0x4C7733
        // and 0x4C7750 and a scan of the whole `.text` for the displacement finds those two writes
        // and **no read at all**, so it is dead in this build (CONFIRMED by exhaustion).
        if e.class == "NFS5.POPUPCIRCLE" {
            return Vec::new();
        }
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let state = e.get("state").unwrap_or("normal");
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Shape {
                continue;
            }
            // A part's `shapePathName.value` and `shapeIndex.value` are that part's **defaults**,
            // and the `.lay` block's own keys override them — the ordinary attribute rule of
            // `RE/notes/controls.md` §1, nothing special about these two. Five classes name their
            // own pack: `NFS5.PICTURE` (484 of them) also names the entry, so its single part
            // resolves to exactly the pair the widget carries; `NFS5.BOTTOMBARBUTTON`,
            // `NFS5.CheckBox`, `NFS5.AnimatingPicture` and `NFS5.CarSettingSlider` name only the
            // pack and keep the part's index — `data.startframe`, `data.onIndex`,
            // `data.start_frame` and the literal 8 of `CarSettingSlider.ctr`'s knob.
            //
            // The port used to give up on the whole part list as soon as a widget named a pack and
            // draw the widget's own pair instead, which is the same picture for a `NFS5.PICTURE`
            // and the **wrong entry** for the other four: every bottom-bar button drew `logoAn`
            // entry 0 where its `startframe` says 11, and every car-setting slider entry 0 of
            // `weeicons.fsh` where its knob is entry 8.
            let Some(file) = self.part_attr(e, p, "shapePathName").filter(|v| !v.is_empty()) else { continue };
            // `shapeIndex` is a literal or a `data.<key>` reference to the widget's own attribute
            let index = match self.part_attr(e, p, "shapeIndex") {
                Some(v) => match nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true)) {
                    Some(n) if n >= 0.0 => n as usize,
                    _ => continue,
                },
                None => 0,
            };
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)));
            let (px, py) = at("pos").unwrap_or((0.0, 0.0));
            let (pw, ph) = at("size").unwrap_or((0.0, 0.0));
            // `flip` is the part's own attribute — `Graphic Tab Button.ctr` writes
            // `flip.value=data.flipit`, so the widget's `flipit` mirrors the tab's art
            let flip = self
                .part_attr(e, p, "flip")
                .and_then(|v| nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0, true)))
                .filter(|n| *n >= 0.0)
                .map(|n| n as i64)
                .unwrap_or_else(|| e.int("flip").unwrap_or(0).max(0));
            out.push((px as f32, py as f32, pw as f32, ph as f32, file.to_string(), index, flip));
        }
        out
    }

    /// The first drawable part of the widget's class template that carries `key`, e.g. the
    /// `fontPathName` of a header's `label` part.
    fn part_value<'a>(&'a self, e: &'a nfs_formats::lay::Element, key: &str) -> Option<&'a str> {
        let c = self.controls.get(&e.class)?;
        let state = e.get("state").unwrap_or("normal");
        c.state_parts(state).iter().filter_map(|n| c.part(n)).find_map(|p| p.value(key)).filter(|v| !v.is_empty())
    }

    /// The black bars that keep the 4:3 design area square in a wider window. The 2D screen is
    /// letterboxed by [`place`], but a 3D room behind it fills the whole window, so the bars have to
    /// be drawn over it. APPROXIMATION — the original's front end is always 4:3.
    pub fn letterbox_bars(&self, aspect: f32) -> Vec<Sprite> {
        let design = DESIGN.0 / DESIGN.1;
        if aspect <= design + 1e-3 {
            return Vec::new();
        }
        let w = (1.0 - design / aspect) * 0.5;
        let bar = |x: f32| Sprite {
            texture: self.white,
            pos: [x, 0.0],
            size: [w * aspect, 1.0],
            uv_min: [0.0, 0.0],
            uv_max: [1.0, 1.0],
            color: [0.0, 0.0, 0.0, 1.0],
            rotation: 0.0,
            pivot: [0.5, 0.5],
        };
        vec![bar(0.0), bar(1.0 - w)]
    }

    /// Mark this screen as drawn over something else — the bottom bar the front end lays over
    /// every screen (0x4D39D3), or any screen with a 3D room behind it
    /// (`RE/notes/frontend-3d.md`). Such a screen draws its widgets but not the screen fill, which
    /// would otherwise paint over what is behind. The background screen is marked with it.
    pub fn set_overlay(&mut self, overlay: bool) {
        self.overlay = overlay;
        if let Some(b) = self.background.as_mut() {
            b.set_overlay(overlay);
        }
    }

    /// Start this screen sliding and fading in, the way the original does when a screen is pushed
    /// (state 1, 0x4E09A0). The background screen transitions with it.
    pub fn transition_in(&mut self) {
        self.transition = Transition::In { frames: TRANS_DELAY_FRAMES, ms: 0 };
        if let Some(b) = self.background.as_mut() {
            b.transition_in();
        }
    }

    /// Start it going out (state 3, 0x4E09C0).
    pub fn transition_out(&mut self) {
        self.transition = Transition::Out { frames: TRANS_DELAY_FRAMES, ms: 0 };
        if let Some(b) = self.background.as_mut() {
            b.transition_out();
        }
    }

    pub fn transition(&self) -> Transition {
        self.transition
    }

    /// Advance the transition clock by one frame; `true` once the screen has settled or gone.
    pub fn advance_transition(&mut self, dt_ms: u32) -> bool {
        self.clock_ms = self.clock_ms.wrapping_add(dt_ms);
        self.transition = self.transition.advance(dt_ms);
        if let Some(b) = self.background.as_mut() {
            b.advance_transition(dt_ms);
        }
        self.transition.done()
    }

    /// The offset and the alpha a widget draws with at this point in the transition
    /// (`FeWidget::transitionOffset` 0x4C8760 and `::transitionAlpha` 0x4C8840): the slide is the
    /// screen size scaled by the progress, the fade is the progress itself, and the bits come from
    /// `transon` while the screen comes in and from `transoff` while it goes out.
    fn transition_of(&self, e: &nfs_formats::lay::Element) -> ((f32, f32), f32) {
        let p = self.transition.progress();
        if p == 0 {
            return ((0.0, 0.0), 1.0);
        }
        let out = matches!(self.transition, Transition::Out { .. });
        let (key, other) = if out { ("transoff", "transon") } else { ("transon", "transoff") };
        // `SAME` means "use the other one", and both `SAME` means no transition at all
        // (0x4C75D4…0x4C7601)
        let (mut bits, ob) = (transition_bits(self.attr(e, key).unwrap_or("")), transition_bits(self.attr(e, other).unwrap_or("")));
        if bits & trans::SAME != 0 {
            bits = if ob & trans::SAME != 0 { 0 } else { ob };
        }
        let f = p as f32 / TRANS_FULL as f32;
        let mut d = (0.0f32, 0.0f32);
        if bits & trans::LEFT != 0 {
            d.0 -= DESIGN.0 * f;
        }
        if bits & trans::RIGHT != 0 {
            d.0 += DESIGN.0 * f;
        }
        if bits & trans::TOP != 0 {
            d.1 -= DESIGN.1 * f;
        }
        if bits & trans::BOTTOM != 0 {
            d.1 += DESIGN.1 * f;
        }
        let alpha = if bits & trans::FADE != 0 { 1.0 - (p * 255 / TRANS_FULL) as f32 / 255.0 } else { 1.0 };
        (d, alpha)
    }

    /// The x of the widget named `name` on this screen — the column anchor an accessor's row draw
    /// looks up (`FE_widgetPos` 0x4D6410 searches the current screen's widget list from the end).
    fn anchor_x(&self, name: &str) -> Option<f32> {
        let e = self.layout.elements.iter().rev().find(|e| e.name.eq_ignore_ascii_case(name))?;
        Some(self.position_of(e).0)
    }

    /// The `NFS5.ListBox` widgets on this screen, with the accessor each one lists.
    pub fn list_boxes(&self) -> Vec<ListBoxWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.ListBox")
            .map(|(index, e)| ListBoxWidget { index, accessor: self.attr(e, "accessor").map(str::to_string) })
            .collect()
    }

    /// Fill a list box with the rows its accessor would hand out, and say which row the accessor
    /// has selected (`current` — the row the original paints with `highlightcolorselected`).
    pub fn set_list_rows(&mut self, index: usize, rows: Vec<ListRow>, current: Option<usize>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_list_rows(j, rows, current);
            }
            return;
        }
        if rows.is_empty() {
            self.lists.remove(&index);
        } else {
            self.lists.insert(index, (rows, current));
        }
    }

    /// Hand an `NFS5.3DCAR` the offscreen view its car was rendered into. The view covers the whole
    /// screen — the car's place in it comes from the camera, not from the widget's rectangle — and
    /// is drawn in the widget's own place in the layout order, which is what puts it over the art
    /// behind it and under the art in front.
    pub fn set_car_view(&mut self, index: usize, view: Option<TextureId>) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_car_view(j, view);
            }
            return;
        }
        match view {
            Some(t) => self.car_views.insert(index, t),
            None => self.car_views.remove(&index),
        };
    }

    /// The offscreen view already handed to this widget, so the caller can render into it again.
    pub fn car_view(&self, index: usize) -> Option<TextureId> {
        self.car_views.get(&index).copied()
    }

    /// The `NFS5.3DCAR` widgets on this screen.
    pub fn car_3d(&self) -> Vec<Car3dWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.3DCAR")
            .map(|(index, e)| Car3dWidget {
                index,
                accessor: self.attr(e, "accessor").map(str::to_string),
                othersource: self.attr(e, "othersource").map(str::to_string),
                position_type: self.attr_num(e, "positiontype").unwrap_or(0.0) as i64,
                damage: self.attr_num(e, "damage").unwrap_or(0.0) != 0.0,
            })
            .collect()
    }

    /// The `NFS5.CarLogo` widgets on this screen, with the car source each one names.
    pub fn car_logos(&self) -> Vec<CarLogoWidget> {
        self.layout
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.class == "NFS5.CarLogo")
            .map(|(index, e)| CarLogoWidget {
                index,
                accessor: e.get("accessor").filter(|a| !a.is_empty()).map(str::to_string),
                othersource: e.get("othersource").filter(|a| !a.is_empty()).map(str::to_string),
            })
            .collect()
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

    /// Bind a `NFS5.MOVIEBOX` to a `.mad` and start it playing.
    ///
    /// The whole file is kept, its video chunks indexed, and one texture allocated to decode into.
    /// [`LayoutScreen::advance_movies`] steps it; the period is the header's own (see
    /// [`MoviePlayer`]).
    pub fn set_movie(&mut self, r: &mut Renderer, paths: &GamePaths, index: usize, dir: &str, file: &str) {
        if let Some((k, j)) = self.route(index) {
            if let Some(g) = self.groups.get_mut(&k) {
                g.set_movie(r, paths, j, dir, file);
            }
            return;
        }
        if self.movies.contains_key(&index) {
            return;
        }
        let Some(path) = GamePaths::find_file(&paths.fe_data().join(dir), file) else { return };
        let (Ok(exe), Ok(data)) = (std::fs::read(paths.root.join("nfs5.exe")), std::fs::read(&path)) else { return };
        let Ok(tables) = nfs_formats::mad::MadTables::from_exe(&exe) else { return };
        let chunks: Vec<(usize, usize)> = {
            let mut at = 0usize;
            let mut out = Vec::new();
            for c in nfs_formats::mad::chunks(&data) {
                if c.is_video() {
                    out.push((at, c.data.len()));
                }
                at += c.data.len();
            }
            out
        };
        let Some(&(off, len)) = chunks.first() else { return };
        let Ok(header) = nfs_formats::mad::MadHeader::parse(&data[off..off + len]) else { return };
        let mut decoder = nfs_formats::mad::MadDecoder::new(tables, &header);
        let f = decoder.decode_frame(&data[off..off + len]);
        if f.width == 0 || f.height == 0 {
            return;
        }
        let tex = r.upload_texture(&TextureData::single(f.width as u32, f.height as u32, f.rgba));
        self.movies.insert(
            index,
            MoviePlayer {
                data,
                chunks,
                decoder,
                next: 1,
                period_q16: header.period.max(1) as u64,
                clock_q16: 0,
                tex,
                size: (f.width as f32, f.height as f32),
            },
        );
    }

    /// True when this screen — or the one behind it — has a movie playing.
    pub fn has_movies(&self) -> bool {
        !self.movies.is_empty() || self.background.as_ref().is_some_and(|b| b.has_movies())
    }

    /// Step every `NFS5.MOVIEBOX` on this screen by `dt_ms`, decoding and uploading the frames its
    /// own period calls for. A movie that has run out starts again, which is what a front-end loop
    /// does.
    pub fn advance_movies(&mut self, r: &mut Renderer, dt_ms: u32) {
        for m in self.movies.values_mut() {
            m.clock_q16 += (dt_ms as u64) << 16;
            let mut drawn = None;
            while m.clock_q16 >= m.period_q16 {
                m.clock_q16 -= m.period_q16;
                if m.next >= m.chunks.len() {
                    m.next = 0;
                }
                let (off, len) = m.chunks[m.next];
                m.next += 1;
                let _ = m.decoder.decode_chunk(&m.data[off..off + len]);
                drawn = Some(());
            }
            if drawn.is_some() {
                let (w, h) = (m.size.0 as u32, m.size.1 as u32);
                r.update_texture(m.tex, &TextureData::single(w, h, m.decoder.rgba()));
            }
        }
        if let Some(b) = self.background.as_mut() {
            b.advance_movies(r, dt_ms);
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
            let marker = upload(if ev.backwards { 2 } else { 1 });
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
        if self.logos.get(&index).is_some_and(|(f, b)| f == file && *b == 0) {
            return;
        }
        if !self.shapes.contains_key(&(file.to_string(), 0)) {
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
            // `1111`, which is why only the first row is matched by name. CONFIRMED by the size:
            // `MA601a.fsh` measures 4 × (128+128+128+74) by (148+138) = 458×286, and both
            // `chroniclepicture.lay` and `chroniclemedia.lay` give the widget `size=458,286`.
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
            self.shapes.insert((file.to_string(), 0), (id, w as f32, h as f32));
        }
        self.logos.insert(index, (file.to_string(), 0));
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
            out.push(Control { index: i, x, y, destination: dest, callback, accessor, string_id: e.string_id() });
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
        Sprite { texture: self.white, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5] }
    }

    /// The font a widget draws its text in: its own `fontPathName`, or the one its class template
    /// gives the part it draws (a header's is `Univers17b.ffn`), or the front end's default.
    fn font_for(&self, e: &nfs_formats::lay::Element) -> Option<&BitmapFont> {
        let named = e
            .get("fontPathName")
            .filter(|f| !f.is_empty())
            .or_else(|| self.part_value(e, "fontPathName"))
            .map(|f| f.to_lowercase());
        named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))
    }

    /// Draw one line of text inside the widget box `(x, y, w, h)` with the original's alignment
    /// rule (0x5232AD → 0x4DA3D7): the anchor is the box's left edge / midpoint / right edge and the
    /// string is placed at the anchor, half a width back, or a full width back, rounded to pixels.
    /// Glyphs are drawn at their native size in the 640×480 design space.
    /// Lay `s` out inside `w`, breaking between words, and draw the lines `line_px` apart. Used by
    /// `NFS5.WORDWRAPACCESSORWATCH` — the class the front end uses where a value is a sentence
    /// rather than a figure: a tournament's description, a part's, a mission briefing.
    ///
    /// **APPROXIMATION**: breaking on spaces, keeping a word that does not fit on its own line, is
    /// the obvious rule but is not one that was read out of the original.
    #[allow(clippy::too_many_arguments)]
    fn wrapped_text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        if w <= 0.0 {
            self.text(out, aspect, f, s, x, y, w, align, color);
            return;
        }
        let mut line = String::new();
        let mut ly = y;
        for word in s.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if f.place(&candidate).1 > w && !line.is_empty() {
                self.text(out, aspect, f, &line, x, ly, w, align, color);
                ly += f.line_px();
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            self.text(out, aspect, f, &line, x, ly, w, align, color);
        }
    }

    fn text(&self, out: &mut Vec<Sprite>, aspect: f32, f: &BitmapFont, s: &str, x: f32, y: f32, w: f32, align: i64, color: [f32; 4]) {
        let (glyphs, width) = f.place(s);
        let anchor = match align {
            1 => x + w * 0.5,
            2 => x + w,
            _ => x,
        };
        let left = match align {
            1 => anchor - width * 0.5,
            2 => anchor - width,
            _ => anchor,
        }
        .round();
        let top = y.round();
        // `NFS_TEXT_PROBE=1` draws the same glyphs in white — the A/B that characterised the
        // faint-text fault in `RE/AUDIT.md` F2.
        let probe = std::env::var("NFS_TEXT_PROBE").is_ok();
        let color = if probe { [1.0, 1.0, 1.0, 1.0] } else { color };
        let solid = std::env::var("NFS_TEXT_SOLID").is_ok();
        for g in &glyphs {
            let (pos, size) = self.place(aspect, left + g.x, top + g.y, g.w, g.h);
            // `NFS_TEXT_FULLUV=1` opens the glyph's UV window to the whole sheet — with
            // `NFS_TEXT_BIG` it shows the sheet, without it nothing (`AUDIT.md` F2)
            let full = std::env::var("NFS_TEXT_FULLUV").is_ok();
            let (uv0, uv1) = if full {
                ([0.0, 0.0], [1.0, 1.0])
            } else if solid {
                let c = [(g.uv_min[0] + g.uv_max[0]) * 0.5, (g.uv_min[1] + g.uv_max[1]) * 0.5];
                ([c[0] - 0.0001, c[1] - 0.0001], [c[0] + 0.0001, c[1] + 0.0001])
            } else {
                (g.uv_min, g.uv_max)
            };
            // `NFS_TEXT_SOLID=1` swaps the atlas for the 1×1 white texture and collapses the UV
            // window to one texel, so a glyph quad becomes a plain filled rectangle — the probe
            // that showed the quads composite at about 8 % whatever they sample (`AUDIT.md` F2).
            // `NFS_TEXT_BIG=1` draws the quads four times the size
            let size = if std::env::var("NFS_TEXT_BIG").is_ok() { [size[0] * 4.0, size[1] * 4.0] } else { size };
            let tex = if solid { self.white } else { f.texture() };
            out.push(Sprite { texture: tex, pos, size, uv_min: uv0, uv_max: uv1, color, rotation: 0.0, pivot: [0.0, 0.0] });
            // `NFS_TEXT_AB=1` repeats each glyph one line lower **in red**, same texture, same UV
            // window, same size — so one image carries the widget's text twice and the only
            // difference between the two is `color.rgb`. The red copy draws and the black original
            // does not (`AUDIT.md` F2).
            if std::env::var("NFS_TEXT_AB").is_ok() {
                out.push(Sprite { texture: f.texture(), pos: [pos[0], pos[1] + size[1] * 1.2], size, uv_min: uv0, uv_max: uv1, color: [1.0, 0.0, 0.0, 1.0], rotation: 0.0, pivot: [0.0, 0.0] });
            }
        }
    }

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

    fn sprites_reporting(&self, aspect: f32, strings: &Strings, selected: Option<usize>, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>) -> Vec<Sprite> {
        let mut out = Vec::new();
        if let Some(bg) = &self.background {
            // the background's own gaps are not this screen's: every background layout is one of
            // the 98 in `FEData/Layouts` and the sweep measures it in its own right, so reporting
            // it here would count it once per screen that sits on it
            out.extend(bg.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new()));
        }
        let bg_end = out.len();
        // `NFS_REVERSE_ORDER=1` draws the widgets back to front instead of file order (probe for
        // `RE/AUDIT.md` F2). The screen block, element 0, is metadata and is never drawn.
        let order: Vec<usize> = if std::env::var("NFS_REVERSE_ORDER").is_ok() {
            (1..self.layout.elements.len()).rev().collect()
        } else {
            (0..self.layout.elements.len()).collect()
        };
        for i in order {
            let e = &self.layout.elements[i];
            let ((tdx, tdy), talpha) = self.transition_of(e);
            let (x, y) = self.position_of(e);
            let (x, y) = (x + tdx, y + tdy);
            let (w, h) = self.box_of(e, 0);
            let first = out.len();
            // the widget's `state` selects a part list that draws nothing at all — it is erased,
            // not undrawn, so it is not counted as a gap either
            if self.state_draws_nothing(e) {
                continue;
            }
            let mut closed = false;
            'elem: {
            match e.class.as_str() {
                // the screen itself: a full-screen fill
                c if c.starts_with("REAL.BLACK") => {
                    // The screen's own `color` is not the fill (the shipped files carry
                    // 0xff000000, 0xff0000ff and 0xff004080 without looking any different), so the
                    // class name is taken at its word — black. Only the bottom-most screen fills,
                    // and only when there is nothing behind it: a screen with a 3D room behind it
                    // (`RE/notes/frontend-3d.md`) would otherwise paint the room out.
                    // APPROXIMATION.
                    if self.background.is_none() && !self.overlay {
                        out.push(self.quad(aspect, 0.0, 0.0, DESIGN.0, DESIGN.1, [0.0, 0.0, 0.0, 1.0]));
                    }
                }
                // `NFS_NO_GOURAUD=1` skips them — the probe for which sprites bury the text
                "NFS5.GOURAUDBOX" if std::env::var("NFS_NO_GOURAUD").is_ok() => {}
                // `FEGouraudBox::draw` **0x4C9F60…0x4CA095**. It is one gouraud-shaded quad whose
                // four corners are placed **independently**, and the port had been drawing an
                // axis-aligned rectangle — wrong for the 270-odd of the 576 shipped boxes that
                // carry a corner offset.
                //
                // The draw takes the widget's position (0x525360) and box, and its fade
                // (0x4C8840), then pushes four `(colour, x, y)` triples to the quad primitive
                // 0x4D98D0 — bottom-right first, then bottom-left, top-right, top-left:
                //
                // ```text
                //   UL = (x + ULoffX,     y + ULoffY)          colour cUL   [this+0x150/0x154]
                //   UR = (x + URoffX + w, y + URoffY)                 cUR   [this+0x158/0x15C]
                //   BL = (x + BLoffX,     y + BLoffy + h)             cBL   [this+0x160/0x164]
                //   BR = (x + BRoffX + w, y + BRoffY + h)             cBR   [this+0x168/0x16C]
                // ```
                //
                // — the eight offsets bound at 0x4C9E8A…0x4C9F1A, and the file really does spell
                // the third one `BLoffy` with a small `y` (the string at 0x5D865C). Each colour
                // goes through 0x4D87F0 first, which leaves it alone at a fade of 0xFF and
                // otherwise scales *its own alpha byte* by the fade — the same thing this port
                // does to every sprite after the fact.
                //
                // The sprite pipeline has one colour per axis-aligned quad, so the box is still a
                // grid of cells, but now bilinear in **position** as well as colour, which gets
                // the skew. APPROXIMATION, and the grid is only subdivided along an axis that
                // actually varies.
                "NFS5.GOURAUDBOX" => {
                    let c = |k: &str| e.color(k).map(tint).unwrap_or([0.0, 0.0, 0.0, 0.0]);
                    let (ul, ur, bl, br) = (c("cUL"), c("cUR"), c("cBL"), c("cBR"));
                    let off = |k: &str| e.int(k).unwrap_or(0) as f32;
                    let corner = [
                        [x + off("ULoffX"), y + off("ULoffY")],
                        [x + off("URoffX") + w, y + off("URoffY")],
                        [x + off("BLoffX"), y + off("BLoffy") + h],
                        [x + off("BRoffX") + w, y + off("BRoffY") + h],
                    ];
                    let differs = |a: [f32; 4], b: [f32; 4]| (0..4).any(|j| (a[j] - b[j]).abs() > 1.0 / 255.0);
                    let moved = |a: [f32; 2], b: [f32; 2], d: usize| (a[d] - b[d]).abs() > 0.5;
                    // x varies if the two top corners differ from each other, or the sides slant
                    let skew_x = moved(corner[0], corner[2], 0) || moved(corner[1], corner[3], 0);
                    let skew_y = moved(corner[0], corner[1], 1) || moved(corner[2], corner[3], 1);
                    let nx = if differs(ul, ur) || differs(bl, br) || skew_x || skew_y { 16 } else { 1 };
                    let ny = if differs(ul, bl) || differs(ur, br) || skew_x || skew_y { 16 } else { 1 };
                    let at = |u: f32, v: f32, d: usize| {
                        let top = corner[0][d] + (corner[1][d] - corner[0][d]) * u;
                        let bot = corner[2][d] + (corner[3][d] - corner[2][d]) * u;
                        top + (bot - top) * v
                    };
                    for gy in 0..ny {
                        for gx in 0..nx {
                            let (u0, v0) = (gx as f32 / nx as f32, gy as f32 / ny as f32);
                            let (u1, v1) = ((gx + 1) as f32 / nx as f32, (gy + 1) as f32 / ny as f32);
                            let (u, v) = ((u0 + u1) * 0.5, (v0 + v1) * 0.5);
                            let mut col = [0.0f32; 4];
                            for j in 0..4 {
                                let top = ul[j] + (ur[j] - ul[j]) * u;
                                let bot = bl[j] + (br[j] - bl[j]) * u;
                                col[j] = top + (bot - top) * v;
                            }
                            // the cell's own corners, so a slanted edge is followed a step at a time
                            let (px, py) = (at(u0, v0, 0).min(at(u0, v1, 0)), at(u0, v0, 1).min(at(u1, v0, 1)));
                            let cw = (at(u1, v0, 0).max(at(u1, v1, 0)) - px).max(0.5);
                            let ch = (at(u0, v1, 1).max(at(u1, v1, 1)) - py).max(0.5);
                            out.push(self.quad(aspect, px, py, cw, ch, col));
                        }
                    }
                }
                // `FEGouraudIndicatorBar::draw` (0x4CA690) asks its source for a number, then at
                // 0x4CA71A picks the span it paints: with `goingleft` clear it is `0 ..= value`,
                // and with it set `100 - value ..= 100`, so the mirrored column of the car-compare
                // screen fills from its right edge. The value is a per cent because the widget's
                // own colour stops are named `c0`, `c50` and `c100`.
                "NFS5.GouraudIndicatorBar" => {
                    let Some(&value) = self.bars.get(&i) else { break 'elem };
                    let value = value.clamp(0, 100) as f32;
                    let going_left = e.get("goingleft").is_some_and(|v| v.trim() != "0");
                    let (lo, hi) = if going_left { (100.0 - value, 100.0) } else { (0.0, value) };
                    if hi <= lo {
                        break 'elem;
                    }
                    // The three stops are the colour at 0, 50 and 100 per cent of the **box**, not
                    // of the filled part, and the span really is split at fifty: the shared bar
                    // painter **0x521CB0** draws it as two gouraud pieces, `c0`→`c50` up to the
                    // 50.0f at [0x5B251C] and `c50`→`c100` past it, each clamped to the filled
                    // part (0x521D5A…0x521E3D). CONFIRMED against 0x4CA690, which hands 0x521CB0
                    // the box, the span and the three colours, and against the bind 0x4CA1C0.
                    //
                    // One rule the shipped screens never exercise, kept because it is the class's:
                    // if the widget gives **no `c50`** the bind promotes the third stop into the
                    // second and clears the third (0x4CA227…0x4CA23D), and 0x521CB0's `test ebx,
                    // ebx` at 0x521CE1 then takes the two-stop path, blending its own midpoint
                    // with `0x4D8850(c0, c100, 0x80)` — half way. All 42 shipped bars carry all
                    // three, so this is the port being right rather than lucky.
                    let stop = |k: &str| e.color(k).map(tint);
                    let c0 = stop("c0").unwrap_or([1.0; 4]);
                    // the promotion: no `c50` and the third stop becomes the second, leaving two
                    let (c50, c100) = match (stop("c50"), stop("c100")) {
                        (Some(a), Some(b)) => (a, Some(b)),
                        (None, Some(b)) | (Some(b), None) => (b, None),
                        (None, None) => (c0, None),
                    };
                    let at = |p: f32| -> [f32; 4] {
                        // two stops left: one ramp end to end, which is what blending the middle
                        // one half way (`0x80`) comes to
                        let Some(c100) = c100 else {
                            return std::array::from_fn(|j| c0[j] + (c50[j] - c0[j]) * (p / 100.0));
                        };
                        let (a, b, t) = if p < 50.0 { (c0, c50, p / 50.0) } else { (c50, c100, (p - 50.0) / 50.0) };
                        std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
                    };
                    let cell = w / 100.0;
                    for p in lo as u32..hi as u32 {
                        out.push(self.quad(aspect, x + p as f32 * cell, y, cell, h.max(1.0), at(p as f32 + 0.5)));
                    }
                }
                // `FEGlintyRectangle`'s constructor (0x522530) reads **`width`, `height`, `col` and
                // `animate`** — not `size` and `color`, which is why one of these draws nothing when
                // it is read like a plain rectangle. It hands them to 0x51FDA0 in that order, and
                // the draw (0x51FDE0) passes the widget's own position and that width and height to
                // **0x4D9200, which is an outline**: four calls to the one-bar primitive 0x4D9280
                // with a thickness of `1.0f` — top, bottom, left and right. So this is a one-pixel
                // frame, which is what makes it a frame around each bar of `repair.lay` (92×8 over
                // a 90×6 bar) and around the whole list (261×163 over the panel), rather than a
                // fill that would bury them.
                //
                // The outline is **verified** against the code: `FEGlintyRectangle::draw` 0x51FDE0
                // blends `col` with the fade (0x4D87F0) and hands the widget's position and that
                // width and height to **0x4D9200**, which is four calls to the one-bar primitive
                // 0x4D9280 — `(x, y, w, 1)`, `(x, y + h − 1, w, 1)`, `(x, y, 1, h)` and
                // `(x + w − 1, y, 1, h)`, the inset coming from the 1.0f at [0x5B24A8]. That is
                // this arm, bar for bar.
                //
                // Not drawn: the flourish. Its **timing** is settled — 0x51FE65…0x51FEDA: when it
                // is not running and `now − [this+0x14] > [this+0x18]` it starts, runs for
                // `rand()%32 + 64` ticks, then waits `rand()%256 + 256` before the next. `animate`
                // picks which of four to play and **5 means a fresh random one of the four each
                // time** (0x51FE8E) — which is what 93 of the 181 shipped ones carry. The four
                // flourishes' own shapes were not traced, so they are left out rather than guessed
                // at, exactly as the `NFS5.CarLogo` glint is.
                //
                // One more thing the draw does that this port does not: 0x51FDE9 returns before
                // **anything** when 0x4E35E0 answers true — `[0x6197AA] == 0`, a global that also
                // gates the glinty line (0x51FAA9) and the bar primitives. With it clear the whole
                // widget disappears, outline and all.
                "NFS5.GlintyRectangle" => {
                    // exactly the three keys the ctor reads, and no fallback: `col`, `width` and
                    // `height`. All 181 shipped ones carry those three and **none** carries
                    // `color` or `size`, so a fallback could only ever disagree with the class.
                    let Some(c) = e.color("col") else { break 'elem };
                    let gw = e.int("width").map(|v| v as f32).unwrap_or(0.0).max(1.0);
                    let gh = e.int("height").map(|v| v as f32).unwrap_or(0.0).max(1.0);
                    let (c, t) = (tint(c), 1.0);
                    out.push(self.quad(aspect, x, y, gw, t, c));
                    out.push(self.quad(aspect, x, y + gh - t, gw, t, c));
                    out.push(self.quad(aspect, x, y, t, gh, c));
                    out.push(self.quad(aspect, x + gw - t, y, t, gh, c));
                }
                // A panel draws **only** the layout its `filename` names, in the panel's own place.
                // Its slot-4 0x4CEBC0 — the same slot that is `FeListBox`'s combined tick-and-draw
                // 0x4C5460…0x4C691B — returns at once when `[panel+0x14C]` is null and otherwise
                // ticks the loaded screen, runs its draw and unloads it once it is down
                // (0x4E0E10 → `[panel+0x70]` = 0x4CEB00). It never reaches the base part draw, so
                // the `area` RECT of `Panel.ctr` is not painted — which the files agree with:
                // 22 of the 24 shipped panels carry the template's own `color.value=0x8000ff00`
                // unchanged, a half-transparent green that is nowhere on screen in the game.
                "NFS5.PANEL" => {
                    // a closed tab draws nothing and is not a gap
                    let Some(g) = self.groups.get(&i) else {
                        if self.closed.contains(&i) {
                            closed = true;
                        }
                        break 'elem;
                    };
                    out.extend(self.embedded(g, aspect, strings, accessor, blank, x, y));
                }
                // `NFS5.3DCAR` is drawn after the screen, below — its car is a 3D view, not a
                // sprite in the layout's own order
                "NFS5.3DCAR" => {}
                // The text boxes: `InputBox.ctr`'s `state.normal` is `unselbkgd` — a
                // `data.width`×14 rect filled `0xFFBC0000`, the same dark red `ControllerSetup.ctr`
                // gives its rows as `regcolor` — four one-pixel black `bgoutline` rects, and
                // `untext`, whose text is the widget's accessor. The chat boxes are built the same
                // way. Their `state.focused` swaps in `selbkgd`/`hitext` and a `cursor`, which this
                // port has no focus or caret for.
                "NFS5.INPUTBOX" | "NFS5.CHATINPUTBOX" | "NFS5.CHATDISPLAYBOX"
                | "NFS5.CHARVECTACCESSORINPUTBOX" | "NFS5.CONTROLLERSETUP" => {
                    for (px, py, pw, ph, c) in self.rect_parts(e) {
                        out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(c)));
                    }
                    let Some(f) = self.font_for(e) else { break 'elem };
                    let Some(v) = e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) else { break 'elem };
                    self.text(&mut out, aspect, f, &v, x, y, w, e.int("align").unwrap_or(0), [1.0, 1.0, 1.0, 1.0]);
                }
                "NFS5.ListBox" => {
                    // `FeListBox::draw` 0x4C64FE…0x4C691B with `ListBox.ctr`. The panel is the
                    // template's `selectionbox` part — `pos=0,1`, `size=data.width,
                    // data.selectionboxmaxheight` — filled with 0x10FFFFFF (0x4C6636), and its
                    // height is clamped to the content: `min(rows·14, selectionboxmaxheight)`
                    // (0x4C53A0). When it clamps, a scrollbar appears and takes 16 px off the
                    // width (0x4C5421 with 0x4DB030). Rows are the accessor's own: 14 px each
                    // (0x4BC700), the string at (+3, +2) over a bar of the row's colour
                    // (0x4BC350). The original keeps drawing while the accumulated height is still
                    // within the panel and scissors the overflow; the port draws only the rows
                    // that fit whole (APPROXIMATION — the sprite pipeline has no scissor).
                    let Some((rows, current)) = self.lists.get(&i) else { break 'elem };
                    let Some(f) = self.font_for(e) else { break 'elem };
                    let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
                    let full: f32 = rows.iter().map(|r| r.height()).sum();
                    let (panel_h, scrolls) = if full < max_h { (full, false) } else { (max_h, true) };
                    let mut panel_w = self.attr_num(e, "width").unwrap_or(0.0) as f32;
                    if scrolls {
                        panel_w -= LIST_SCROLLBAR_W;
                    }
                    let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
                        Some(v) => nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)).map(|(a, b)| (x + a as f32, y + b as f32)).unwrap_or((x, y)),
                        None => (x, y),
                    };
                    out.push(self.quad(aspect, px, py, panel_w, panel_h, [1.0, 1.0, 1.0, 16.0 / 255.0]));
                    // The scrollbar the list grows when it clamps is a `NFS5.VerticalSlider`, and
                    // `VerticalSlider.ctr` says what it is: a `track` the full height in
                    // **0xffffbb00**, `weeicons.fsh` entry **13** at `0,7` for the up arrow, entry
                    // **14** at the bottom for the down arrow, and a `knob` of
                    // `decrement_button.size.width × 15` in **0x80ffffff** starting at
                    // `decrement_button.size.height`. The knob then moves by the scrollbar helper's
                    // own rule, the vertical branch of 0x4D0BBE:
                    // `dy = index · (trackHeight − knobHeight) / (count − 1)`.
                    //
                    // It lives in the sixteen pixels the panel gave up: 0x4C5421 takes exactly
                    // 0x4DB030's **16** off the selection box's width and leaves its x alone, so
                    // the strip freed is the one to its right. That the bar is drawn at the strip's
                    // left edge is this port's reading of that, not a traced placement —
                    // APPROXIMATION, as is using the selected row for the knob's index, since the
                    // port keeps no separate scroll position.
                    if scrolls {
                        let sx = px + panel_w;
                        let art = |n: usize| self.shapes.get(&("weeicons.fsh".to_string(), n)).copied();
                        let (up, down) = (art(13), art(14));
                        let track_w = down.map(|(_, w, _)| w).or(up.map(|(_, w, _)| w)).unwrap_or(LIST_SCROLLBAR_W);
                        out.push(self.quad(aspect, sx, py, track_w, panel_h, tint([0xff, 0xbb, 0x00, 0xff])));
                        if let Some((tex, aw, ah)) = up {
                            let (pos, size) = self.place(aspect, sx, py + 7.0, aw, ah);
                            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                        }
                        if let Some((tex, aw, ah)) = down {
                            let (pos, size) = self.place(aspect, sx, py + panel_h - ah, aw, ah);
                            out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                        }
                        let knob_h = 15.0;
                        let base = py + up.map(|(_, _, h)| h).unwrap_or(0.0);
                        let span = (panel_h - knob_h - (base - py)).max(0.0);
                        let count = rows.len().max(1) as f32;
                        let idx = current.unwrap_or(0) as f32;
                        let dy = if count > 1.0 { idx * span / (count - 1.0) } else { 0.0 };
                        let knob_w = up.map(|(_, w, _)| w).unwrap_or(track_w);
                        out.push(self.quad(aspect, sx, base + dy, knob_w, knob_h, tint([0xff, 0xff, 0xff, 0x80])));
                    }
                    // `textcolor` is black and the two highlights amber and dark red, all three
                    // straight out of `ListBox.ctr`; a highlighted row's own text turns white
                    // (0x4C67A6 → 0x4D87D0).
                    let text_c = self.attr(e, "textcolor").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0, 0, 0, 255]);
                    let sel_c = self.attr(e, "highlightcolorselected").and_then(parse_color).map(|c| [c[0], c[1], c[2], 255]).unwrap_or([0xBC, 0, 0, 255]);
                    let mut ry = py;
                    for (n, row) in rows.iter().enumerate() {
                        if ry + row.height() > py + panel_h {
                            break;
                        }
                        let (bar, fg) = if *current == Some(n) { (Some(sel_c), [1.0, 1.0, 1.0, 1.0]) } else { (None, tint(text_c)) };
                        match row {
                            ListRow::Text(t) => {
                                if let Some(c) = bar {
                                    out.push(self.quad(aspect, px, ry, panel_w, LIST_ROW_H, tint(c)));
                                }
                                self.text(&mut out, aspect, f, t, px + 3.0, ry + 2.0, 0.0, 0, fg);
                            }
                            // a columned row draws no bar of its own — `Track.Records` reduces the
                            // colour it is handed to its alpha alone (0x4FB328) and places each
                            // column at another widget's x, at the row's own y (no +2)
                            ListRow::Columns(cells) => {
                                if let Some(c) = bar {
                                    out.push(self.quad(aspect, px, ry, panel_w, row.height(), tint(c)));
                                }
                                for cell in cells {
                                    // a cell with no anchor of its own sits at the panel's left edge
                                    let ax = if cell.anchor.is_empty() { px } else { self.anchor_x(&cell.anchor).unwrap_or(px) };
                                    let align = if cell.dx < 0.0 { 2 } else { 0 };
                                    self.text(&mut out, aspect, f, &cell.text, ax + cell.dx, ry, 0.0, align, tint(text_c));
                                }
                            }
                            // `drawPartsHeading` 0x518DE0: a black rule at the top and another 13
                            // pixels down, with the name centred two pixels below the first
                            ListRow::Header(t) => {
                                let black = [0.0, 0.0, 0.0, 1.0];
                                out.push(self.quad(aspect, px, ry, panel_w, 1.0, black));
                                out.push(self.quad(aspect, px, ry + PART_HEADER_H, panel_w, 1.0, black));
                                self.text(&mut out, aspect, f, t, px, ry + 2.0, panel_w, 1, tint(text_c));
                            }
                        }
                        ry += row.height();
                    }
                }
                // a widget that is only a place, with its picture put there from code: one entry
                // of a `.fsh` pack — see [`set_pack_picture`](Self::set_pack_picture)
                // the tab strip this group embeds, drawn at the group's own place. Its `pos` is
                // 0,0 on all four screens that use one, so the strip lands in the screen's own
                // coordinates.
                "NFS5.RADIOGROUP" => {
                    let Some(g) = self.groups.get(&i) else { break 'elem };
                    out.extend(self.embedded(g, aspect, strings, accessor, blank, x, y));
                }
                // `FETrackSmall2dSpline::draw` (0x4FCD80). The widget cycles through the events of
                // the tournament its accessor names: `update` (0x4FCB50) divides the milliseconds
                // since the list was built by 256 for the index and restarts when that reaches the
                // count, and keeps a second counter for the fade — `m = elapsed % 256`, `m*8` while
                // `m < 0x20`, `0x7FF - m*8` while `m > 0xE0`, untouched in between, so it holds at
                // 0xF8 for the middle of each step (0x4FCB5D…0x4FCB91).
                //
                // Each step draws three things: the map, in amber (`0x4D87C0` is
                // `(alpha << 24) | 0xFFBB00`) at the fade's alpha; **one** marker, in white
                // (`0x4D87D0`, `0xFFFFFF`) at the widget's own alpha; and beside them the whole
                // event list, `"-"` at x=352 and the track's name at x=356, 15 pixels apart from
                // y=280, the current one amber and the rest white (0x4FCF20…0x4FCFE9). The map and
                // the marker take their positions from the `.fsh` entry, not from the `.lay`.
                "NFS5.TrackSmall2dSpline" => {
                    let Some(evs) = self.track_maps.get(&i).filter(|e| !e.is_empty()) else { break 'elem };
                    let cur = (self.clock_ms / 256) as usize % evs.len();
                    let m = self.clock_ms % 256;
                    let ramp = if m < 0x20 {
                        m * 8
                    } else if m > 0xE0 {
                        0x7FF - m * 8
                    } else {
                        0xF8
                    };
                    let ev = &evs[cur];
                    let picture = |out: &mut Vec<Sprite>, art: Option<(TextureId, f32, f32, f32, f32)>, color: [f32; 4]| {
                        let Some((tex, aw, ah, ax, ay)) = art else { return };
                        let (pos, size) = self.place(aspect, ax, ay, aw, ah);
                        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color, rotation: 0.0, pivot: [0.5, 0.5] });
                    };
                    picture(&mut out, ev.map, [1.0, 0xBB as f32 / 255.0, 0.0, talpha * ramp as f32 / 255.0]);
                    picture(&mut out, ev.marker, [1.0, 1.0, 1.0, talpha]);
                    if let Some(f) = self.font_for(e) {
                        for (row, ev) in evs.iter().enumerate() {
                            let ry = 280.0 + 15.0 * row as f32;
                            let c = if row == cur { [1.0, 0xBB as f32 / 255.0, 0.0, talpha] } else { [1.0, 1.0, 1.0, talpha] };
                            self.text(&mut out, aspect, f, "-", 352.0, ry, 0.0, 0, c);
                            self.text(&mut out, aspect, f, &ev.name, 356.0, ry, 0.0, 0, c);
                        }
                    }
                }
                // `NFS5.PRIZERACEMAP` and `NFS5.FACTORYMAP` share one draw (0x51A360, both vtables
                // at +0x3C): entry 0 of the pack the widget loaded, at the widget's own place and
                // the art's own size, in white at the transition alpha. Which pack that is comes
                // from the setter — the era for the prize map (0x506430: `mclass`, `mgold` or
                // `mmod` by the current tournament's `+4`, 0x504800) and the mission code at
                // `fac+0x3F` for the factory map (0x51A310, the first mission whose `+0xE8` is 0).
                "NFS5.PRIZERACEMAP" | "NFS5.FACTORYMAP" => {
                    let Some(key) = self.logos.get(&i) else { break 'elem };
                    let Some(&(tex, aw, ah)) = self.shapes.get(key) else { break 'elem };
                    let (pos, size) = self.place(aspect, x, y, aw, ah);
                    out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5] });
                }
                // The player's portrait and the Factory Driver licence card. `NFS5.PLAYERPICTURE`
                // (0x4CFC00) and `NFS5.BADGE` (0x4CF3E0) put their art down at the widget box's
                // top-left corner at the art's own size; the two `MINI` classes (0x4CFC90,
                // 0x4CF610) call the blit's eight-argument form (0x4D9B30) with the box's own
                // width and height instead, so they stretch. All four take the widget's box from
                // 0x4C8670, which answers `(x1, y1, x2, y2)`.
                // `NFS5.FACTORYPIC` (0x51A210) joins them: one face from `FactoryPeople.fsh` at the
                // widget's own place and the art's size.
                // `NFS5.HUD` (0x4CB970) joins them for its first piece: entry 0 of `hud.fsh`, the
                // 240×180 `disp` schematic, at the widget box's top-left at its own size.
                "NFS5.PLAYERPICTURE" | "NFS5.MINIPLAYERPICTURE" | "NFS5.BADGE" | "NFS5.MINIBADGE" | "NFS5.FACTORYPIC" | "NFS5.HUD" => {
                    let Some(key) = self.logos.get(&i) else { break 'elem };
                    let Some(&(tex, aw, ah)) = self.shapes.get(key) else { break 'elem };
                    let stretch = e.class.starts_with("NFS5.MINI");
                    let (dw, dh) = if stretch && w > 0.0 && h > 0.0 { (w, h) } else { (aw, ah) };
                    let (pos, size) = self.place(aspect, x, y, dw, dh);
                    out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5] });
                    let Some((name, rank, colour)) = self.badges.get(&i) else { break 'elem };
                    // the shadow: entry 0 at x+244 and entry 1 at y+131 (0x4CF499, 0x4CF4C4). The
                    // original also gates it on a start-up timer (0x4E35D0 wants the game to have
                    // been up 0x2100000 ticks) and on `[0x657E60]`; neither is state this port
                    // keeps, so the shadow is always drawn.
                    for (entry, dx, dy) in [(0usize, 244.0, 0.0), (1, 0.0, 131.0)] {
                        let Some(&(t, sw, sh)) = self.shapes.get(&(LICENSE_SHADOW.to_string(), entry)) else { continue };
                        let (pos, size) = self.place(aspect, x + dx, y + dy, sw, sh);
                        out.push(Sprite { texture: t, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    // the name, centred (the fourth argument of 0x4DA350 is the alignment: 1 pulls
                    // back half the text's width, 2 the whole of it — 0x4DA3D7…0x4DA405) at
                    // x+122, y+11 (0x4CF513, 0x4CF522), and the rank at x+98, y+50 in black
                    // (0x4CF5C1, 0x4CF5C4, 0x4CF5D0)
                    let mut c = *colour;
                    c[3] *= talpha;
                    if let Some(f) = self.fonts.get(BADGE_NAME_FONT).or_else(|| self.font_for(e)) {
                        self.text(&mut out, aspect, f, name, x + 122.0, y + 11.0, 0.0, 1, c);
                    }
                    if let Some(f) = self.fonts.get(BADGE_RANK_FONT).or_else(|| self.font_for(e)) {
                        self.text(&mut out, aspect, f, rank, x + 98.0, y + 50.0, 0.0, 0, [0.0, 0.0, 0.0, talpha]);
                    }
                }
                // `NFS5.ChroniclePicture` (0x508310) lays the grid out at the widget's box, clipped
                // to it and panned by the two offsets the drag keeps at `+0x14C` / `+0x150`. The
                // port composes the grid into one texture and draws it unpanned, which is the same
                // picture whenever it fits — and it does: the widget is `size=458,286` and so is
                // the picture.
                // `NFS5.MOVIEBOX` (0x4CD640) hands the drawing to the player object at `+0x1BC`;
                // the picture goes at the widget's own box.
                "NFS5.MOVIEBOX" => {
                    let Some(m) = self.movies.get(&i) else { break 'elem };
                    let (dw, dh) = (if w > 0.0 { w } else { m.size.0 }, if h > 0.0 { h } else { m.size.1 });
                    let (pos, size) = self.place(aspect, x, y, dw, dh);
                    out.push(Sprite { texture: m.tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, talpha], rotation: 0.0, pivot: [0.5, 0.5] });
                }
                "NFS5.ChroniclePictureIcon" | "NFS5.ShowcasePicture" | "NFS5.ChroniclePicture" => {
                    let Some(key) = self.logos.get(&i) else { break 'elem };
                    let Some(&(tex, aw, ah)) = self.shapes.get(key) else { break 'elem };
                    let (dw, dh) = (if w > 0.0 { w } else { aw }, if h > 0.0 { h } else { ah });
                    let (pos, size) = self.place(aspect, x, y, dw, dh);
                    out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                }
                "NFS5.CarLogo" => {
                    // `NFS5.CarLogo::draw` 0x4C1B70, CONFIRMED: the pair is drawn side by side —
                    // the first entry at the anchor, the second immediately to its right — and the
                    // whole thing is vertically centred in the widget's own box. `align` moves the
                    // anchor the same way the text rule does: 0 keeps `pos`, 1 pulls back half the
                    // combined width, 2 the full width (0x4C1BE9…0x4C1C22). A second entry only one
                    // pixel high is a placeholder: the original zeroes its width and skips it
                    // (0x4C1BDC). Nothing is drawn until the port has bound a car to the widget.
                    let Some((file, base)) = self.logos.get(&i) else { break 'elem };
                    let Some(&(ta, aw, ah)) = self.shapes.get(&(file.clone(), *base)) else { break 'elem };
                    let b = self.shapes.get(&(file.clone(), base + 1)).copied();
                    let (bw, bh, tb) = match b {
                        Some((t, w, h)) if h != 1.0 => (w, h, Some(t)),
                        _ => (0.0, 0.0, None),
                    };
                    let left = match e.int("align").unwrap_or(0) {
                        1 => x - ((aw + bw) * 0.5).trunc(),
                        2 => x - (aw + bw),
                        _ => x,
                    };
                    let top = y + ((h - ah) * 0.5).trunc();
                    let (pos, size) = self.place(aspect, left, top, aw, ah);
                    out.push(Sprite { texture: ta, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    if let Some(tb) = tb {
                        let (pos, size) = self.place(aspect, left + aw, top, bw, bh);
                        out.push(Sprite { texture: tb, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    // Not drawn: the glint that sweeps over the logo (the `GLNT` entry of
                    // `weeicons.fsh`, 0x4C1D26…0x4C1DB2) — its sweep is driven by the same global
                    // the front end sets to 640 at 0x4D35B4, but the shape's own travel was not
                    // traced, so it is left out rather than guessed at.
                }
                // `FEChronicleMainBoxControl::draw` (0x507990) starts with five bars, in this
                // order, all 454 wide from the widget box's own left edge and all but the outline
                // at half alpha (0x4D87F0 scales by the 0x80 pushed at 0x5079FB, 0x507A62,
                // 0x507AA7 and 0x507AEC):
                //
                // | y | size | what |
                // |---|---|---|
                // | +0x26 | 454×16 | white (0x4D87D0) |
                // | +0x26 | 455×16 | the outline of that bar, black (0x4D87E0 is `alpha << 24`), 0x4D9200 |
                // | +0x10 | 454×20 | 0xBC0000 (0x4D87B0) |
                // | +0x26 | 454×60 | 0xBC0000 |
                // | +0x64 | 454×60 | 0xBC0000 |
                //
                // Not drawn: the timeline over them. 0x507B4F walks the years from the accessor's
                // own (`(value − 0x76C) % 100`, so `+0x76C` is 1900), prints each as `"%d"` at
                // `x + col + 0x2D` (0x507BDF), outlines a 90×20 box per year (0x507C1A) and asks
                // 0x507340 for up to three chronicle entries a year to put icons in it. That loop
                // is read but not ported — the port has no chronicle year index to walk.
                // `FEKnockoutTrackList::draw` (0x4FA190) is a **seven-slot** strip: the widget's box
                // is divided by the `1/7` at 0x5B7E78 (0x4FA241), and the first `n` slots — `n` from
                // the accessor's vtable `+0x6C`, the opponent count — are filled with the literal
                // `0x80FEDD81` the draw pushes at 0x4FA1FA, which is also what the `.lay` block's
                // own `color` says. The row `Knockout.TrackNumber` is on is redrawn in the menu
                // amber `0x4D87C0` gives (0x4FA296), and half-alpha black rules (`0x4D87E0` at
                // `0x80`) separate the rows and mark two columns, at `x + 0x0B` and `x + 0x71`
                // (0x4FA35B, 0x4FA39F).
                "NFS5.KNOCKOUTTRACKLIST" => {
                    let Some(&(rows, current)) = self.ko_strip.get(&i) else { break 'elem };
                    if rows == 0 || w <= 0.0 || h <= 0.0 {
                        break 'elem;
                    }
                    let slot = h / 7.0;
                    let filled = slot * rows as f32;
                    let half = talpha * 128.0 / 255.0;
                    let fill = [0xFE as f32 / 255.0, 0xDD as f32 / 255.0, 0x81 as f32 / 255.0, half];
                    out.push(self.quad(aspect, x, y, w, filled, fill));
                    if current < rows {
                        out.push(self.quad(aspect, x, y + slot * current as f32, w, slot, [1.0, 0xBB as f32 / 255.0, 0.0, talpha]));
                    }
                    let rule = [0.0, 0.0, 0.0, half];
                    for r in 1..rows {
                        out.push(self.quad(aspect, x, y + slot * r as f32, w, 1.0, rule));
                    }
                    for dx in [11.0, 113.0] {
                        out.push(self.quad(aspect, x + dx, y, 1.0, filled, rule));
                    }
                }
                "NFS5.ChronicleMainBoxControl" => {
                    let half = |c: [f32; 3]| [c[0], c[1], c[2], talpha * 128.0 / 255.0];
                    let red = [0xBC as f32 / 255.0, 0.0, 0.0];
                    out.push(self.quad(aspect, x, y + 38.0, 454.0, 16.0, half([1.0, 1.0, 1.0])));
                    let black = [0.0, 0.0, 0.0, talpha];
                    for (bx, by, bw, bh) in [(x, y + 38.0, 455.0, 1.0), (x, y + 38.0 + 15.0, 455.0, 1.0), (x, y + 38.0, 1.0, 16.0), (x + 454.0, y + 38.0, 1.0, 16.0)] {
                        out.push(self.quad(aspect, bx, by, bw, bh, black));
                    }
                    out.push(self.quad(aspect, x, y + 16.0, 454.0, 20.0, half(red)));
                    out.push(self.quad(aspect, x, y + 38.0, 454.0, 60.0, half(red)));
                    out.push(self.quad(aspect, x, y + 100.0, 454.0, 60.0, half(red)));
                }
                // **None of these four is a registered class.** The only `*Rectangle*` string in
                // the whole executable is `NFS5.GlintyRectangle` (0x5DD847) — there is no
                // `NFS5.Rectangle` and no `NFS5.OutlineRectangle`, and no `NFS5.GLINTYLINE`
                // registration either beyond 0x5222E0's, whose class does nothing but the parts.
                // CONFIRMED by exhaustion: they are drawn by the template and nothing else.
                //
                // Which settles `NFS5.OutlineRectangle`, and the port had it **backwards**:
                // `OutlineRectangle.ctr`'s `area` is the whole box with `color.value=0x8000ff00`
                // and **no flags**, so the block's `color` fills the interior, while `top`,
                // `bottom`, `left` and `right` are one-pixel bars at `0xff000000`
                // `HIDDEN|READONLY` — always **black**. The port was drawing four hairlines in
                // the widget's colour and no fill. All 195 shipped ones carry a colour, and they
                // are fills: 82 a 30 % white wash, 31 fully transparent, 16 half grey, 12 amber,
                // 11 the dark red. `NFS5.BEVELEDOPTIONBOX` is the same shape.
                // The same exhaustion settles four more, 811 widgets between them: neither
                // `REAL.LABEL` (306), `NFS5.PICTURE` (484), `NFS5.NonLocalizedLabel` (11) nor
                // `NFS5.ExtraCircle` (10) is a registered class, so each is its template and
                // nothing else.
                //
                // `Label.ctr` is one `LOCALIZEDTEXT` part at
                // **`pos.value=(0-label.size.width)/2*data.align,0`** in `univers8b.ffn` and
                // `0xffffffff`, all three overridable — and that expression *is* the alignment
                // rule, written out: `align` 0 keeps the position, 1 pulls back half the string's
                // width, 2 the whole width. This port used to reproduce it by anchoring in a box,
                // which only agreed because the box came out zero; now it is the template's own
                // arithmetic over the measured extent. All 306 carry `stringID`, `align` and
                // `foreColor`; all 11 non-localised labels carry `text` and `foreColor`.
                // `NFS5.HEADER` (41) and `NFS5.SUBHEADER` (5) are their template's one
                // `LOCALIZEDTEXT` part as well: `Header.ctr` puts the class at `pos.value=6,4` and
                // the label at `0,0` in `Univers17b.ffn` and `0xffffffff`, and the class's own
                // slot 20 (0x4CAE60) does nothing but bind that part. Its slot 21 (0x4CADC0) loads
                // a glint — see `RE/notes/controls.md` §4.11 — which is not drawn here.
                "NFS5.HEADER" | "NFS5.SUBHEADER"
                | "REAL.LABEL" | "NFS5.PICTURE" | "NFS5.NonLocalizedLabel" | "NFS5.ExtraCircle"
                | "NFS5.OutlineRectangle" | "NFS5.Rectangle" | "NFS5.BEVELEDOPTIONBOX" | "NFS5.GLINTYLINE" => {
                    let state = if selected == Some(i) { "selected" } else { e.get("state").unwrap_or("normal") };
                    self.template_widget(&mut out, aspect, e, state, x, y, strings);
                    // Erased, not undrawn, in two cases the original agrees with:
                    //
                    // * a box whose every rectangle is **fully transparent** — four of the 362
                    //   `NFS5.Rectangle`s carry an alpha of zero;
                    // * a label whose `stringID` the shipped table carries **empty** — 12300 and
                    //   19809, the two `REAL.LABEL`s of `RE/AUDIT.md` F.
                    if out.len() == first
                        && let Some(c) = self.controls.get(&e.class)
                    {
                        let parts = c.state_parts(state);
                        let rects = parts.iter().filter_map(|n| c.part(n)).any(|p| p.kind() == nfs_formats::ctr::PartKind::Rect);
                        let blank_string = e.string_id().is_some_and(|id| strings.get(id) == Some(""));
                        if rects || blank_string {
                            closed = true;
                        }
                    }
                }
                // `NFS5.CarSettingSlider` is the template's transparent track (`color.value` is
                // `0x00000000`) and a 6×11 knob from `weeicons.fsh` entry 8, whose `pos.value` is
                // `0,0` and `HIDDEN|READONLY` because the code moves it. The scrollbar helper the
                // class owns does that at **0x4D0C0C…0x4D0C68**, the horizontal branch of
                // `[helper+0x130] == 0`:
                //
                // ```text
                // track = width(track part)          // 0x4E2B90
                // knob  = width(knob part)
                // dx    = count > 1 ? index * (track - knob) / (count - 1) : 0
                // knob.x = knobBase.x + dx           // [helper+0x114]
                // ```
                //
                // — and the vertical branch (0x4D0BBE…0x4D0C06) is the same on `y` over
                // `height(track) - height(knob)` (0x4E2BA0) against `[helper+0x118]`. All five
                // shipped sliders are `width=90 height=13` against a 6×11 knob, so they are
                // horizontal. The index and the count come from the accessor's own registration
                // (`career::car_setup_index`).
                // `NFS5.CheckBox` keeps **two** entries of its pack, `onIndex` and `offIndex`,
                // and `CheckBox.ctr`'s one SHAPE part is written `shapeIndex.value=data.onIndex`
                // — the ticked one, as a default. Which is drawn is the code's:
                // `FECheckBox::refresh` **0x4C3730** asks the `boolAccessor` (`[this+0x144]`,
                // vtable +0x6C) and writes `[this+0x14C]` (`data.onIndex`) or `[this+0x150]`
                // (`data.offIndex`) into `checkBoxPic.shapeIndex` (`[this+0x148]`, through
                // 0x5271F0); 0x4C36E0 is the per-frame watch that calls it when the bool moves.
                //
                // All ten shipped boxes are on `repair.lay` with a `QUICKFIX.<category>` bool, and
                // each of those is registered with a default index of **0** — false — at
                // 0x4F2584 and its nine neighbours, so the repair screen opens with every box
                // clear. The port has no per-category repair basket, so that is where they stay.
                "NFS5.CheckBox" => {
                    let on = self.checks.get(&i).copied().unwrap_or(false);
                    let key = if on { "onIndex" } else { "offIndex" };
                    let Some(index) = e.int(key).filter(|n| *n >= 0).map(|n| n as usize) else { break 'elem };
                    let Some(file) = e.get("shapePathName").filter(|v| !v.is_empty()) else { break 'elem };
                    let Some(&(tex, aw, ah)) = self.shapes.get(&(file.to_string(), index)) else { break 'elem };
                    let (pos, size) = self.place(aspect, x, y, aw, ah);
                    out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                }
                "NFS5.CarSettingSlider" => {
                    let (idx, count) = self.knobs.get(&i).copied().unwrap_or((0, 1));
                    for (px, py, pw, ph, file, index, _) in self.shape_parts(e) {
                        let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
                        let (dw, dh) = (if pw > 0.0 { pw } else { aw }, if ph > 0.0 { ph } else { ah });
                        let dx = if count > 1 { (idx * (w - dw).max(0.0) as i32 / (count - 1)) as f32 } else { 0.0 };
                        let (pos, size) = self.place(aspect, x + px + dx, y + py, dw, dh);
                        out.push(Sprite { texture: tex, pos, size, uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                }
                // `ComboBox.ctr` (the class is `NFS5.ComboBoxSlider`, 86 widgets) and
                // `ColorBox.ctr` (11) are the same widget, and it is a **dark-red bar**, not a
                // caption floating on the background:
                //
                // * `backrect` fills `0,0`…`data.width,data.height` with `0xFFBC0000`,
                //   `HIDDEN|READONLY` so no layout changes it — the same dark red as a
                //   `NFS5.GOTOBUTTON`'s pill (§4.3);
                // * `labelleft` puts the widget's own `stringID` at `5,2` in `universC11b.ffn`,
                //   white;
                // * `arrowbutton` is `weeicons.fsh` entry **14** at
                //   `data.width-arrowbutton.size.width-4,4`, which `shape_parts` already places;
                // * `selectionbox` is the drop-down and is not in `[state.normal]`.
                //
                // The value is the one piece the template does not settle: its `label` part is
                // `text.value=%data.accessor%` at `data.width-10-label.size.width-3,2` in
                // `universC11m.ffn` with `foreColor=0x00000000` — a **transparent** colour, so the
                // class draws the value from code (`FeComboBox`, the class `NFS5.ListBox` shares)
                // and the part is the editor's preview. The port keeps its own right-aligned draw
                // and its own white. APPROXIMATION, as before.
                "NFS5.ComboBoxSlider" | "NFS5.ColorBox" => {
                    let state = if selected == Some(i) { "selected" } else { e.get("state").unwrap_or("normal") };
                    for (px, py, pw, ph, col) in self.state_rects(e, state) {
                        out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(col)));
                    }
                    for (px, py, pw, ph, file, index, flip) in self.shape_parts(e) {
                        let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
                        let (dw, dh) = (if pw > 0.0 { pw } else { aw }, if ph > 0.0 { ph } else { ah });
                        let (pos, size) = self.place(aspect, x + px, y + py, dw, dh);
                        let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    // the caption, where the template puts it
                    if let Some(c) = self.controls.get(&e.class)
                        && let Some(p) = c.part("labelleft")
                        && let Some(t) = e.string_id().filter(|n| *n != 0).and_then(|n| strings.get(n)).filter(|t| !t.is_empty())
                        && let Some(col) = self.part_colour(e, p, "foreColor").filter(|c| c[3] != 0)
                    {
                        let (px, py) = self
                            .part_attr(e, p, "pos")
                            .and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0, true)))
                            .unwrap_or((0.0, 0.0));
                        let f = self
                            .part_attr(e, p, "fontPathName")
                            .map(|f| f.to_lowercase())
                            .and_then(|f| self.fonts.get(&f))
                            .or_else(|| self.fonts.get(DEFAULT_FONT));
                        if let Some(f) = f {
                            self.text(&mut out, aspect, f, t, x + px as f32, y + py as f32, 0.0, 0, tint(col));
                        }
                    }
                    // And the value, which the class draws itself — `FeComboBox`'s own draw has
                    // two text calls, and they settle both:
                    //
                    // * **0x4C6054** puts the caption at `(x + 5, y + 2)` with align **0** in the
                    //   font at `[this+0x190]` — the template's `labelleft`, same place;
                    // * **0x4C6237** puts the value with align **2** anchored at
                    //   `x + width(this) - 12` (0x4C6220, over 0x4E2B90's width) at the widget's
                    //   own `y`, in the second font `[this+0x194]`.
                    //
                    // The template's `label` part says `data.width-10-label.size.width-3,2`, which
                    // is the same right edge one pixel further in and two rows down; the code is
                    // what runs, so the port follows the code. That retires the approximation this
                    // arm used to carry.
                    if let Some(v) = e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) {
                        let f = self
                            .controls
                            .get(&e.class)
                            .and_then(|c| c.part("label"))
                            .and_then(|p| self.part_attr(e, p, "fontPathName"))
                            .map(|f| f.to_lowercase())
                            .and_then(|f| self.fonts.get(&f))
                            .or_else(|| self.font_for(e));
                        let Some(f) = f else { break 'elem };
                        let colour = if selected == Some(i) { [1.0, 0.85, 0.2, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
                        self.text(&mut out, aspect, f, &v, x + w - 12.0, y, 0.0, 2, colour);
                    }
                }
                // A `NFS5.GOTOBUTTON` is a **dark-red pill with white text**, not red text on
                // nothing. `GoToButton.ctr` says so outright and every one of the 153 shipped
                // buttons carries the colour:
                //
                // * `[state.normal] parts=bg,label,mediumlabel,smalllabel,data`, and `bg` is a
                //   RECT at `1,1` sized `data.width-2,data.height-2` filled with `data.maincolor`
                //   — whose default is `0xFFBC0000` and whose value in the files is that same dark
                //   red on 150 of the 153 (two carry `0x00000000`, which is transparent);
                // * `[state.selected]` and `[state.hover]` swap `bg` for `hoverbg`, the same
                //   rectangle filled with `data.hovercolor` — default `0xffffBB00`, amber. That is
                //   what the original highlights with, so this port no longer has to invent one;
                // * the caption is drawn **three times**, at three places in three fonts, and
                //   `data.medium` picks which one is opaque: `label` at `5,2` in `Universc11b` is
                //   `data.medium?0x00000000:0xffffffff`, `mediumlabel` centred in
                //   `Universc11m` is `(data.medium&1)?0xffffffff:0x00000000`, and `smalllabel` in
                //   `univers8b` — at `5,2` when `medium&4`, centred otherwise — is
                //   `(data.medium&4)?0xffffffff:(data.medium&2)?0xffffffff:0x00000000`. The
                //   shipped `medium` values are 0 (96), 1 (50) and 4 (6).
                //
                // An alpha of zero is invisible, which is how two of the three labels stay out of
                // the way; `BottomBarButton.ctr`'s labels, which really are coloured, all carry a
                // `0xff` alpha instead.
                "NFS5.GOTOBUTTON" => {
                    let state = if selected == Some(i) { "selected" } else { e.get("state").unwrap_or("normal") };
                    self.template_widget(&mut out, aspect, e, state, x, y, strings);
                }
                // The same shape three more times, 19 widgets: a `0xFFBC0000` box inside four
                // one-pixel black bars, with a white caption. `Tab Button.ctr` puts its
                // `LOCALIZEDTEXT` at `5,2` in `Universc11b.ffn` and swaps `bg` for `hoverbg`
                // (`0xffffbb00`) when it is the open tab; `CameraSelect.ctr` centres a `TEXT`
                // part in `Univers7b.ffn` — the camera number the block carries as `text` — and
                // `CameraPicture.ctr` adds `optart.fsh` entry `data.picturenum` over the box.
                // Two more that are only their template, and both had their caption in the wrong
                // place because the template puts it relative to a **part**, which needs the
                // measured extents of §2:
                //
                // * `BottomBarButton.ctr` (22): `label` sits at `picture.size.width+data.space,8`
                //   — to the right of the `logoAn.fsh` icon the button carries as `startframe`,
                //   by the block's own `space`, and eight pixels down. `bg` is `0x00000000`, so
                //   there is no bar behind it;
                // * `Graphic Tab Button.ctr` (5): `label` is
                //   `((selectedbg.size.width-label.size.width)/2)+data.textoffsetx,data.textoffsety`
                //   — centred over the tab's own highlight art and then nudged by the two offsets
                //   the block carries (`textoffsetx` is 24, 3, 0, −1 and −22 on the five shipped tabs).
                "NFS5.BOTTOMBARBUTTON" | "NFS5.GRAPHICTABBUTTON" => {
                    let state = if selected == Some(i) { "selected" } else { e.get("state").unwrap_or("normal") };
                    self.template_widget(&mut out, aspect, e, state, x, y, strings);
                }
                "NFS5.TABBUTTON" | "NFS5.CAMERASELECT" | "NFS5.CAMERAPICTURE" => {
                    let state = if selected == Some(i) { "selected" } else { e.get("state").unwrap_or("normal") };
                    self.template_widget(&mut out, aspect, e, state, x, y, strings);
                }
                _ => {
                    // Not drawn here: the template's `RECT` parts. They are the widget's regions,
                    // not its paint — `GoToButton.ctr`'s `bg` is `data.maincolor` over the whole
                    // box, which is the colour the button's *text* uses, and filling it puts a
                    // solid red bar behind every label on `QuickRace.lay`. A class that really is
                    // a box draws its rects from its own arm, as `NFS5.CONTROLLERSETUP` does.
                    // the art the widget's class template carries, at the places the template puts
                    // it — `NFS5.CARSETTINGBUTTON`'s arrows, the popup circles, the tab buttons
                    let mut from_template = false;
                    for (px, py, pw, ph, file, index, flip) in self.shape_parts(e) {
                        let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
                        let (dw, dh) = (if pw > 0.0 { pw } else { aw }, if ph > 0.0 { ph } else { ah });
                        let (pos, size) = self.place(aspect, x + px, y + py, dw, dh);
                        let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                        from_template = true;
                    }
                    // anything with a picture draws it; anything with a string draws that.
                    // Only when the class template placed none — otherwise the same art twice.
                    if !from_template
                        && let Some((file, index)) = e.shape()
                        && let Some(&(tex, aw, ah)) = self.shapes.get(&(file.to_string(), index))
                    {
                        // A picture is drawn at its art's own size: every piece of the oval frames
                        // carries `size=50,150` in the file while the `.fsh` entries are 256×256,
                        // 89×82 … and each entry's own signed offset is exactly minus the widget's
                        // `pos` (`CnstLyr.fsh` entry 4 is at (−131, −120), its widget at 131,120).
                        // HIGH, from the data.
                        let (pos, size) = self.place(aspect, x, y, aw, ah);
                        // `flip` mirrors the art: 1 in x, 2 in y, 3 in both. HIGH — the attribute is
                        // an int on the picture widget (0x52AA4E) whose consumer was not traced, but
                        // it is what makes the oval frames of the background screens close up: the
                        // four corners of `screenempty` are the same `.fsh` entry with flip 0/1/2/3.
                        let flip = e.int("flip").unwrap_or(0).max(0);
                        let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
                        out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5] });
                    }
                    if code_placed(&e.class) {
                        break 'elem;
                    }
                    let Some(f) = self.font_for(e) else { break 'elem };
                    // A combo slider carries both: its `stringID` is the label and its `accessor`
                    // holds the value. The original places the value with code; the port fills the
                    // widget's own box — label left, value right — which is the only geometry the
                    // file gives. APPROXIMATION.
                    if e.class == "NFS5.ComboBoxSlider"
                        && let Some(v) = e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor)
                    {
                        let colour = if selected == Some(i) { [1.0, 0.85, 0.2, 1.0] } else { [1.0, 1.0, 1.0, 1.0] };
                        let box_w = if w > 0.0 { w } else { 120.0 };
                        self.text(&mut out, aspect, f, &v, x, y, box_w, 2, colour);
                    }
                    // an accessor watch shows a value, everything else its own `stringID`
                    // case-insensitively: the class is `NFS5.AccessorWatch` on most screens but
                    // `NFS5.WORDWRAPACCESSORWATCH` where the text has to wrap, and a `contains`
                    // that respected case never matched the second one
                    let text = if e.class.to_ascii_lowercase().contains("accessorwatch") {
                        match e.get("accessor").filter(|a| !a.is_empty()).and_then(accessor) {
                            Some(v) => v,
                            None => break 'elem,
                        }
                    } else if let Some(t) = e.get("text").filter(|t| !t.is_empty()) {
                        // a literal string in the file rather than a `FEStrings` id.
                        // `Non-Localized Label.ctr` is nothing but a `TEXT` part whose `text.value`
                        // is the placeholder "Put Text Here", which the widget's own `text` key
                        // replaces — `audioreal.lay`'s per-cent signs are `text=%`.
                        t.to_string()
                    } else {
                        // `stringID=0` is the shipped "Hello, World!" placeholder (marked DO NOT
                        // TRANSLATE in `FEStrings.csv`): those widgets get their text from code.
                        match e.string_id().filter(|id| *id != 0).and_then(|id| strings.get(id)) {
                            Some(t) if !t.is_empty() => t.to_string(),
                            // the table **has** the id and its text is blank: the shipped
                            // `FEStrings.csv` leaves 11110, 12300 and 19809 empty in every
                            // language column, so those four widgets draw nothing in the original
                            // too. Erased, not undrawn.
                            Some(_) => {
                                closed = true;
                                break 'elem;
                            }
                            None => break 'elem,
                        }
                    };
                    // the highlighted button uses its own `hovercolor`, as the original does
                    let color = if selected == Some(i) {
                        // a widget with no `hovercolor` of its own is highlighted anyway, so the
                        // player can see where the selection is — APPROXIMATION
                        e.color("hovercolor").or_else(|| e.color("maincolor")).map(tint).unwrap_or([1.0, 0.85, 0.2, 1.0])
                    } else {
                        e.color("foreColor")
                            .or_else(|| e.color("maincolor"))
                            .or_else(|| e.color("color"))
                            .or_else(|| self.part_value(e, "foreColor").and_then(parse_color))
                            .map(tint)
                            .unwrap_or([1.0, 1.0, 1.0, 1.0])
                    };
                    // a widget with no box of its own anchors on its own text (align 0 then works
                    // out to the position in the file, which is what the button classes carry)
                    let box_w = if w > 0.0 { w } else { 0.0 };
                    // The text sits at the widget's own `pos`, not centred in its box. The files
                    // say so: on `tournament.lay` every value and its label share a `y` — the
                    // `REAL.LABEL` at 299,114 and the `NFS5.AccessorWatch` at 302,114, and so on
                    // down the panel — and the labels carry no `size` at all. Centring a watch in
                    // its `size=100,30` box pushed every value a row below its label, while
                    // `PRIZEMONEY`, whose box is `43,11` and so too short to centre in, lined up.
                    let _ = h;
                    // An accessor watch is its template's one `TEXT` part, and the class's only
                    // job is to keep that part's string up to date: `FEAccessorWatch::tick`
                    // **0x4BD230** updates the accessor (vtable +8), asks it for the plain text
                    // (+0x10) or, when the widget carries a modifier, the modified one (+0x14),
                    // and writes the answer into the part's `text` attribute with 0x526980 —
                    // which is why `accessor Watch.ctr` marks that attribute `HIDDEN|READONLY`
                    // and gives it the placeholder `WATCH`. An accessor that answers **null**
                    // gets the literal `CANTFINDSTRING` (0x4BD2B0); one that answers an empty
                    // string gets nothing, which is the knockout rows' case.
                    //
                    // So its position is the template's, `(0-label.size.width)/2*data.align,0`,
                    // over the width of the string it is showing *now* — the same expression
                    // `Label.ctr` carries (§4.8), and the same integer division. Everything else
                    // keeps the port's own anchor.
                    let watch_pos = self
                        .controls
                        .get(&e.class)
                        .filter(|_| {
                            let c = e.class.to_ascii_lowercase();
                            // not the wrapping one: `WordWrapAccessorWatch.ctr` pins its label at
                            // `0,0` and the wrap is the whole point of the class
                            c.contains("accessorwatch") && !c.contains("wordwrap")
                        })
                        .and_then(|c| c.state_parts(e.get("state").unwrap_or("normal")).iter().filter_map(|n| c.part(n)).find(|p| p.kind() == nfs_formats::ctr::PartKind::Text))
                        .and_then(|p| {
                            let tw = f.place(&text).1 as f64;
                            let v = p.value("pos")?;
                            nfs_formats::ctr::eval_pair(v, &|n| match n {
                                "label.size.width" => Some(tw),
                                other => self.reference(e, other, 0, true),
                            })
                        });
                    if let Some((px, py)) = watch_pos {
                        self.text(&mut out, aspect, f, &text, x + px as f32, y + py as f32, 0.0, 0, color);
                    } else if e.class.to_ascii_lowercase().contains("wordwrap") {
                        self.wrapped_text(&mut out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color);
                    } else {
                        self.text(&mut out, aspect, f, &text, x, y, box_w, e.int("align").unwrap_or(0), color);
                    }
                }
            }
            }
            // every sprite this widget produced fades with it (0x4C8840)
            if talpha < 1.0 {
                for s in &mut out[first..] {
                    s.color[3] *= talpha;
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
                let f0: Vec<(String, [f32; 4])> = { let mut v: Vec<(String, [f32; 4])> = out[first..].iter().map(|s| (format!("{:?}", s.texture), s.color)).collect(); v.dedup(); v };
                println!("  #{i:3} {:<26} pos=({x:.0},{y:.0}) sprites={n} box=({:.3},{:.3})-({:.3},{:.3}) alpha={talpha:.2} first={f0:?}", e.class, b.0, b.1, b.2, b.3);
            }
            if out.len() == first && i > 0 && !closed {
                blank.push((e.class.clone(), source_of(e)));
            }
        }
        if std::env::var("NFS_LAYER_DUMP").is_ok() {
            println!("layer: background {} sprites, own {} sprites", bg_end, out.len() - bg_end);
        }
        // The cars of the `NFS5.3DCAR` widgets that have a `positiontype` of their own, each
        // already rendered into an offscreen view the size of the screen — its place in that view
        // comes from the camera, not from the widget's rectangle. The original draws each one in
        // its widget's own place in the layout order, but two of the three screens that use them
        // (`carcompare`) then put `DarkOval.fsh` over the middle, which would hide them, so the
        // port draws them over the finished screen instead. APPROXIMATION.
        for (&i, &tex) in &self.car_views {
            let alpha = self.transition_of(&self.layout.elements[i]).1;
            let design = DESIGN.0 / DESIGN.1;
            let (fw, x0) = if aspect > design { (design / aspect, (1.0 - design / aspect) * 0.5) } else { (1.0, 0.0) };
            out.push(Sprite { texture: tex, pos: [x0, 0.0], size: [fw * aspect, 1.0], uv_min: [0.0, 0.0], uv_max: [1.0, 1.0], color: [1.0, 1.0, 1.0, alpha], rotation: 0.0, pivot: [0.5, 0.5] });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
